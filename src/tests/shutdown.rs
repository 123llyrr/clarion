//! Real wl_surface destruction must run before Display/State owners disappear.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use smithay::wayland::compositor::{add_destruction_hook, with_states};

use super::Fixture;

struct SurfaceCacheDrop(Arc<AtomicBool>);

impl Drop for SurfaceCacheDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

fn assert_surface_destroyed(teardown: impl FnOnce(Fixture)) {
    let mut fixture = Fixture::new();
    let client = fixture.add_client();
    let surface = {
        let client = fixture.client(client);
        client
            .state
            .compositor
            .as_ref()
            .unwrap()
            .create_surface(&client.qh, ())
    };
    surface.commit();
    fixture.double_roundtrip(client);

    assert_eq!(fixture.clarion().root_surface.len(), 1);
    let server_surface = fixture
        .clarion()
        .root_surface
        .keys()
        .next()
        .unwrap()
        .clone();
    let destroyed = Arc::new(AtomicBool::new(false));
    let saw_shutdown = Arc::new(AtomicBool::new(false));
    let cache_dropped = Arc::new(AtomicBool::new(false));
    let hook_destroyed = destroyed.clone();
    let hook_shutdown = saw_shutdown.clone();
    add_destruction_hook::<crate::compositor::State, _>(&server_surface, move |state, _| {
        hook_shutdown.store(state.clarion.shutting_down, Ordering::Release);
        hook_destroyed.store(true, Ordering::Release);
    });
    with_states(&server_surface, |states| {
        states
            .data_map
            .insert_if_missing(|| SurfaceCacheDrop(cache_dropped.clone()));
    });
    drop(server_surface);
    let loop_weak = fixture.state.server.event_loop.handle().downgrade();
    assert!(!destroyed.load(Ordering::Acquire));
    assert!(!cache_dropped.load(Ordering::Acquire));

    // The client never sends wl_surface.destroy or disconnects voluntarily.
    // Teardown must deliver Smithay cleanup, which removes its self-reference.
    teardown(fixture);

    assert!(destroyed.load(Ordering::Acquire));
    assert!(saw_shutdown.load(Ordering::Acquire));
    assert!(
        cache_dropped.load(Ordering::Acquire),
        "dropping Display without dispatching destroyed leaks surface-owned caches"
    );
    assert!(loop_weak.expired());
    drop(surface);
}

#[test]
fn explicit_shutdown_dispatches_surface_destruction_before_display_drop() {
    assert_surface_destroyed(|mut fixture| {
        fixture.clarion_state().prepare_shutdown();
        assert!(fixture.clarion().root_surface.is_empty());
        fixture.clarion_state().prepare_shutdown();
        drop(fixture);
    });
}

#[test]
fn state_drop_dispatches_surface_destruction_without_another_loop_turn() {
    assert_surface_destroyed(drop);
}

#[test]
fn unwind_dispatches_surface_destruction_once_while_state_is_alive() {
    assert_surface_destroyed(|fixture| {
        let result = catch_unwind(AssertUnwindSafe(move || {
            let _fixture = fixture;
            panic!("injected owner unwind with a live wl_surface");
        }));
        assert!(result.is_err());
    });
}
