use std::hint::black_box;
use std::time::Instant;

use clarion_config::{Action, Config};
use wayland_client::protocol::wl_surface::WlSurface as ClientSurface;

use super::*;
use crate::tests::Fixture;

fn fixture(count: usize) -> (Fixture, Vec<ClientSurface>) {
    let mut config = Config::default();
    config.hotkey_overlay.skip_at_startup = true;
    config.layout.auto_stack_new_windows = false;
    config.animations.window_open.anim.off = true;
    config.animations.window_close.anim.off = true;
    let mut f = Fixture::with_config(config);
    f.add_output(1, (1920, 1080));
    let client = f.add_client();
    let mut surfaces = Vec::with_capacity(count);
    for index in 0..count {
        let window = f.client(client).create_window();
        window.set_title(&format!("window-{index}"));
        let surface = window.surface.clone();
        window.commit();
        f.double_roundtrip(client);
        let window = f.client(client).window(&surface);
        window.set_size(320, 240);
        window.attach_new_buffer();
        window.ack_last_and_commit();
        f.double_roundtrip(client);
        surfaces.push(surface);
    }
    f.clarion_complete_animations();
    assert_eq!(f.clarion().foreign_toplevel_state.toplevels.len(), count);
    (f, surfaces)
}

#[test]
fn unmapping_one_of_many_windows_only_removes_its_toplevel() {
    let (mut f, surfaces) = fixture(16);
    let client = f.state.clients[0].id;
    let remaining = f
        .clarion()
        .foreign_toplevel_state
        .toplevels
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    f.client(client).window(&surfaces[7]).attach_null();
    f.client(client).window(&surfaces[7]).commit();
    f.double_roundtrip(client);
    let state = f.clarion();
    assert_eq!(state.foreign_toplevel_state.toplevels.len(), 15);
    for surface in remaining {
        assert_eq!(
            state
                .foreign_toplevel_state
                .toplevels
                .contains_key(&surface),
            state.layout.find_window_and_output(&surface).is_some()
        );
    }
    // Mapping the same client surface again must recreate a tracked toplevel.
    let window = f.client(client).window(&surfaces[7]);
    window.commit();
    f.double_roundtrip(client);
    let window = f.client(client).window(&surfaces[7]);
    window.attach_new_buffer();
    window.ack_last_and_commit();
    f.double_roundtrip(client);
    assert_eq!(f.clarion().foreign_toplevel_state.toplevels.len(), 16);
}

#[test]
fn minimized_and_offscreen_windows_remain_tracked() {
    let (mut f, _) = fixture(16);
    let id = f.clarion().layout.windows().next().unwrap().1.id().get();
    f.clarion_state()
        .do_action(Action::MinimizeWindowById(id), false);
    f.clarion_complete_animations();
    refresh(f.clarion_state());
    let state = f.clarion();
    assert_eq!(state.foreign_toplevel_state.toplevels.len(), 16);
    assert!(state
        .layout
        .windows()
        .any(|(_, window)| window.is_minimized()));
    assert!(state
        .foreign_toplevel_state
        .toplevels
        .values()
        .all(|data| data.mapped));
}

#[test]
fn windows_survive_membership_refresh_without_an_output() {
    for count in [4, 8] {
        let (mut f, _) = fixture(count);
        let output = f.clarion_output(1);
        f.clarion().layout.remove_output(&output);
        refresh(f.clarion_state());
        let state = f.clarion();
        assert_eq!(state.foreign_toplevel_state.toplevels.len(), count);
        assert!(state
            .foreign_toplevel_state
            .toplevels
            .values()
            .all(|data| data.output.is_none() && data.mapped));
    }
}

#[test]
fn interactively_moved_window_remains_tracked_outside_workspaces() {
    for count in [4, 8] {
        let (mut f, _) = fixture(count);
        let output = f.clarion_output(1);
        let window = f.clarion().layout.active_window().unwrap().window.clone();
        let surface = window.toplevel().unwrap().wl_surface().clone();
        let identifier = f.clarion().foreign_toplevel_state.toplevels[&surface].identifier;
        assert!(f.clarion().layout.interactive_move_begin(
            window.clone(),
            &output,
            smithay::utils::Point::default(),
        ));
        assert!(f.clarion().layout.interactive_move_update(
            &window,
            smithay::utils::Point::from((1000., 0.)),
            output.clone(),
            smithay::utils::Point::default(),
        ));
        assert!(f
            .clarion()
            .layout
            .workspaces()
            .all(|(_, _, workspace)| !workspace.has_window(&window)));
        refresh(f.clarion_state());
        let state = f.clarion();
        assert_eq!(state.foreign_toplevel_state.toplevels.len(), count);
        let data = &state.foreign_toplevel_state.toplevels[&surface];
        assert!(data.mapped);
        assert_eq!(data.output.as_ref(), Some(&output));
        assert_eq!(data.identifier, identifier);
        state.layout.interactive_move_end(&window);
        refresh(f.clarion_state());
        assert_eq!(
            f.clarion().foreign_toplevel_state.toplevels[&surface].identifier,
            identifier
        );
    }
}

fn previous_membership_sweep(state: &mut State) {
    let layout = &state.clarion.layout;
    state
        .clarion
        .foreign_toplevel_state
        .toplevels
        .retain(|surface, _| layout.find_window_and_output(surface).is_some());
}

fn membership_sweep(state: &mut State) {
    remove_closed_toplevels(
        &mut state.clarion.foreign_toplevel_state,
        &state.clarion.layout,
    );
}

/// Actual mapped Wayland windows and the production sweep; excludes drawing and presentation.
#[test]
#[ignore = "CPU microbenchmark; run explicitly with --nocapture --test-threads=1"]
fn membership_sweep_cost_microbenchmark() {
    const ITERATIONS: usize = 256;
    for count in [1, 2, 4, 8, 16, 128] {
        let (mut f, _) = fixture(count);
        let mut before = Vec::new();
        let mut after = Vec::new();
        for round in 0..9 {
            for old in if round % 2 == 0 {
                [true, false]
            } else {
                [false, true]
            } {
                let sweep: fn(&mut State) = if old {
                    previous_membership_sweep
                } else {
                    membership_sweep
                };
                let state = f.clarion_state();
                let start = Instant::now();
                for _ in 0..ITERATIONS {
                    sweep(black_box(&mut *state));
                }
                let elapsed = start.elapsed().as_secs_f64() * 1e6 / ITERATIONS as f64;
                if old {
                    before.push(elapsed);
                } else {
                    after.push(elapsed);
                }
                assert_eq!(state.clarion.foreign_toplevel_state.toplevels.len(), count);
            }
        }
        before.sort_unstable_by(f64::total_cmp);
        after.sort_unstable_by(f64::total_cmp);
        println!(
            "membership windows={count} before_us={:.3} after_us={:.3}",
            before[4], after[4]
        );
    }
}
