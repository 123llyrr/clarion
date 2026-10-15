use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use smithay::backend::renderer::element::{
    Id, RenderElementPresentationState, RenderElementState, RenderElementStates,
};
use smithay::desktop::utils::send_frames_surface_tree;
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface as ServerSurface;
use wayland_client::protocol::wl_surface::WlSurface;

use super::client::{ClientId, SyncData};
use super::Fixture;
use crate::backend::winit::finish_frame;
use crate::backend::RenderResult;
use crate::compositor::RedrawState;
use crate::frame_clock::FrameClock;
use crate::utils::get_monotonic_time;

const REFRESH: Duration = Duration::from_millis(40);

fn setup() -> (Fixture, ClientId, WlSurface, ServerSurface, Output) {
    let mut f = Fixture::new();
    f.add_output(1, (1280, 720));
    let id = f.add_client();
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.double_roundtrip(id);
    let window = f.client(id).window(&surface);
    window.attach_new_buffer();
    window.set_size(100, 100);
    window.ack_last_and_commit();
    f.double_roundtrip(id);
    f.client(id).window(&surface).ack_last_and_commit();
    f.double_roundtrip(id);
    f.realand_complete_animations();
    let server = f
        .realand()
        .layout
        .windows()
        .next()
        .unwrap()
        .1
        .toplevel()
        .wl_surface()
        .clone();
    let output = f.realand_output(1);
    // Render completion is driven by the actual nested helper below. Keep the
    // fixture's lightweight headless backend from submitting unrelated frames.
    f.realand().monitors_active = false;
    f.realand()
        .output_state
        .get_mut(&output)
        .unwrap()
        .schedule
        .test_set_clock(FrameClock::new(Some(REFRESH), false));
    f.realand().update_primary_scanout_output(
        &output,
        &RenderElementStates {
            states: [(
                Id::from_wayland_resource(&server),
                RenderElementState {
                    visible_area: 10_000,
                    presentation_state: RenderElementPresentationState::Rendering { reason: None },
                    needs_capture: false,
                },
            )]
            .into_iter()
            .collect(),
        },
    );
    // Drain the configure allowance and mark this sequence already delivered.
    f.realand().send_frame_callbacks(&output);
    suppress_fallback(&server, &output);
    (f, id, surface, server, output)
}

fn suppress_fallback(surface: &ServerSurface, output: &Output) {
    // No callback is pending. Suppress the invisible-client timeout so a slow
    // test worker cannot accidentally satisfy a refresh-deadline assertion.
    send_frames_surface_tree(
        surface,
        output,
        get_monotonic_time() + Duration::from_secs(3600),
        None,
        |_, _| Some(output.clone()),
    );
}

fn request_frame(f: &mut Fixture, id: ClientId, surface: &WlSurface) -> Arc<SyncData> {
    let callback = Arc::new(SyncData::default());
    surface.frame(&f.client(id).qh, callback.clone());
    surface.commit();
    f.double_roundtrip(id);
    assert!(!callback.done.load(Ordering::Relaxed));
    callback
}

fn wait_for_deadline(f: &mut Fixture, output: &Output) {
    let started = Instant::now();
    while f.realand().output_state[output]
        .schedule
        .nested_frame_timer()
        .is_some()
        && started.elapsed() < Duration::from_secs(2)
    {
        let server = &mut f.state.server;
        server
            .event_loop
            .dispatch(Some(REFRESH), &mut server.state)
            .unwrap();
    }
    assert!(f.realand().output_state[output]
        .schedule
        .nested_frame_timer()
        .is_none());
}

fn dispatch_past_old_deadline(f: &mut Fixture) {
    let deadline = Instant::now() + REFRESH * 2;
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        let server = &mut f.state.server;
        server
            .event_loop
            .dispatch(Some(remaining), &mut server.state)
            .unwrap();
    }
}

#[test]
fn unchanged_frames_keep_one_deadline_and_send_one_callback() {
    let (mut f, id, surface, server, output) = setup();
    let callback = request_frame(&mut f, id, &surface);
    let sequence = f.realand().output_state[&output]
        .schedule
        .callback_sequence();
    let started = Instant::now();
    finish_frame(f.realand(), &output, &RenderResult::NoDamage, started);
    assert_eq!(
        f.realand().output_state[&output]
            .schedule
            .callback_sequence(),
        sequence
    );
    let timer = f.realand().output_state[&output]
        .schedule
        .nested_frame_timer()
        .unwrap();
    for _ in 0..50 {
        // Later empty frames cannot postpone the first frame's deadline.
        finish_frame(
            f.realand(),
            &output,
            &RenderResult::NoDamage,
            started + Duration::from_secs(60),
        );
        assert_eq!(
            f.realand().output_state[&output]
                .schedule
                .nested_frame_timer(),
            Some(timer)
        );
    }
    assert!(!callback.done.load(Ordering::Relaxed));
    wait_for_deadline(&mut f, &output);
    assert!(
        started.elapsed() >= REFRESH,
        "callback deadline fired early"
    );
    f.double_roundtrip(id);
    assert!(callback.done.load(Ordering::Relaxed));
    assert_eq!(
        f.realand().output_state[&output]
            .schedule
            .callback_sequence(),
        sequence.wrapping_add(1)
    );

    suppress_fallback(&server, &output);
    let next = request_frame(&mut f, id, &surface);
    dispatch_past_old_deadline(&mut f);
    f.double_roundtrip(id);
    assert!(!next.done.load(Ordering::Relaxed));
    assert_eq!(
        f.realand().output_state[&output]
            .schedule
            .callback_sequence(),
        sequence.wrapping_add(1)
    );
}

#[test]
fn submitted_frame_cancels_an_empty_frames_old_deadline() {
    let (mut f, id, surface, server, output) = setup();
    let callback = request_frame(&mut f, id, &surface);
    let sequence = f.realand().output_state[&output]
        .schedule
        .callback_sequence();
    finish_frame(
        f.realand(),
        &output,
        &RenderResult::NoDamage,
        Instant::now(),
    );
    assert!(f.realand().output_state[&output]
        .schedule
        .nested_frame_timer()
        .is_some());
    finish_frame(
        f.realand(),
        &output,
        &RenderResult::Submitted,
        Instant::now(),
    );
    assert!(f.realand().output_state[&output]
        .schedule
        .nested_frame_timer()
        .is_none());
    assert_eq!(
        f.realand().output_state[&output]
            .schedule
            .callback_sequence(),
        sequence.wrapping_add(1)
    );
    f.double_roundtrip(id);
    assert!(callback.done.load(Ordering::Relaxed));

    suppress_fallback(&server, &output);
    let next = request_frame(&mut f, id, &surface);
    dispatch_past_old_deadline(&mut f);
    f.double_roundtrip(id);
    assert!(!next.done.load(Ordering::Relaxed));
    assert_eq!(
        f.realand().output_state[&output]
            .schedule
            .callback_sequence(),
        sequence.wrapping_add(1)
    );
}

#[test]
fn skipped_frame_does_not_advance_or_send_callbacks() {
    let (mut f, id, surface, _, output) = setup();
    let callback = request_frame(&mut f, id, &surface);
    let sequence = f.realand().output_state[&output]
        .schedule
        .callback_sequence();
    finish_frame(f.realand(), &output, &RenderResult::Skipped, Instant::now());
    f.double_roundtrip(id);
    assert!(!callback.done.load(Ordering::Relaxed));
    assert!(f.realand().output_state[&output]
        .schedule
        .nested_frame_timer()
        .is_none());
    assert_eq!(
        f.realand().output_state[&output]
            .schedule
            .callback_sequence(),
        sequence
    );
}

#[test]
fn removing_output_cancels_its_empty_frame_callback_deadline() {
    let (mut f, id, surface, _, output) = setup();
    let callback = request_frame(&mut f, id, &surface);
    finish_frame(
        f.realand(),
        &output,
        &RenderResult::NoDamage,
        Instant::now(),
    );
    assert!(f.realand().output_state[&output]
        .schedule
        .nested_frame_timer()
        .is_some());
    f.realand().remove_output(&output);
    dispatch_past_old_deadline(&mut f);
    f.double_roundtrip(id);
    assert!(!callback.done.load(Ordering::Relaxed));
    assert!(!f.realand().output_state.contains_key(&output));
}

#[test]
fn empty_frame_deadline_queues_the_next_animation_frame() {
    let (mut f, _, _, _, output) = setup();
    f.realand().config_error_notification.show();
    assert!(f.realand().animations_need_redraw(&output));
    f.realand()
        .output_state
        .get_mut(&output)
        .unwrap()
        .schedule
        .test_set_redraw(RedrawState::Idle);
    finish_frame(
        f.realand(),
        &output,
        &RenderResult::NoDamage,
        Instant::now(),
    );
    assert!(matches!(
        f.realand().output_state[&output].schedule.redraw_state(),
        RedrawState::Idle
    ));
    wait_for_deadline(&mut f, &output);
    assert!(matches!(
        f.realand().output_state[&output].schedule.redraw_state(),
        RedrawState::Queued
    ));
}

#[test]
fn sustained_presentation_backpressure_preserves_callbacks_and_recovery_cancels_old_deadline() {
    let (mut f, id, surface, server, output) = setup();
    let callback = request_frame(&mut f, id, &surface);
    let sequence = f.realand().output_state[&output]
        .schedule
        .callback_sequence();
    finish_frame(
        f.realand(),
        &output,
        &RenderResult::NoDamage,
        Instant::now(),
    );
    let deadline = f.realand().output_state[&output]
        .schedule
        .nested_frame_timer()
        .unwrap();
    for _ in 0..1000 {
        finish_frame(f.realand(), &output, &RenderResult::Skipped, Instant::now());
        assert_eq!(
            f.realand().output_state[&output]
                .schedule
                .nested_frame_timer(),
            Some(deadline)
        );
        assert_eq!(
            f.realand().output_state[&output]
                .schedule
                .callback_sequence(),
            sequence
        );
    }
    // A temporary host wait cannot send a premature callback. Only successful
    // submission advances this sequence and replaces the older empty deadline.
    assert!(!callback.done.load(Ordering::Relaxed));
    finish_frame(
        f.realand(),
        &output,
        &RenderResult::Submitted,
        Instant::now(),
    );
    f.double_roundtrip(id);
    assert!(callback.done.load(Ordering::Relaxed));
    assert_eq!(
        f.realand().output_state[&output]
            .schedule
            .callback_sequence(),
        sequence.wrapping_add(1)
    );
    suppress_fallback(&server, &output);
    let next = request_frame(&mut f, id, &surface);
    dispatch_past_old_deadline(&mut f);
    f.double_roundtrip(id);
    assert!(!next.done.load(Ordering::Relaxed));
    assert!(f.realand().output_state[&output]
        .schedule
        .nested_frame_timer()
        .is_none());
}

fn locking_fixture() -> (Fixture, Output, smithay::reexports::wayland_protocols::ext::session_lock::v1::client::ext_session_lock_v1::ExtSessionLockV1){
    use crate::compositor::{LockRenderState, LockState};
    use smithay::reexports::wayland_protocols::ext::session_lock::v1::client::ext_session_lock_manager_v1::ExtSessionLockManagerV1;
    let (mut f, _, _, _, output) = setup();
    let id = f.add_client();
    let client = f.client(id);
    let global = client
        .state
        .globals
        .iter()
        .find(|g| g.interface == "ext_session_lock_manager_v1")
        .unwrap();
    let registry = client.display.get_registry(&client.qh, ());
    let manager: ExtSessionLockManagerV1 = registry.bind(global.name, 1, &client.qh, ());
    let lock = manager.lock(&client.qh, ());
    f.double_roundtrip(id);
    let realand = f.realand();
    let LockState::WaitingForSurfaces {
        confirmation,
        deadline_token,
    } = std::mem::take(&mut realand.lock_state)
    else {
        panic!("expected real pending SessionLocker");
    };
    realand.event_loop.remove(deadline_token);
    realand.lock_state = LockState::Locking(confirmation);
    realand.monitors_active = true;
    let output_state = realand.output_state.get_mut(&output).unwrap();
    output_state.lock_render_state = LockRenderState::Unlocked;
    output_state.schedule.test_set_redraw(RedrawState::Idle);
    (f, output, lock)
}

#[test]
fn deferred_native_presentation_preserves_real_locker_until_locked_pixels_submit() {
    use crate::compositor::{LockRenderState, LockState};
    let (mut f, output, _lock) = locking_fixture();
    for _ in 0..1000 {
        f.realand()
            .finish_lock_frame(&output, &RenderResult::Skipped, true, true);
        assert!(matches!(f.realand().lock_state, LockState::Locking(_)));
        assert!(matches!(
            f.realand().output_state[&output].lock_render_state,
            LockRenderState::Unlocked
        ));
    }
    // Only a successful fresh locked composition acknowledges the real
    // SessionLocker. Repeated readiness timers must neither unlock nor ack it.
    f.realand()
        .finish_lock_frame(&output, &RenderResult::Submitted, true, false);
    assert!(matches!(f.realand().lock_state, LockState::Locked(_)));
    assert!(matches!(
        f.realand().output_state[&output].lock_render_state,
        LockRenderState::Locked
    ));
}

#[test]
fn genuine_native_render_failure_still_fails_the_lock_request() {
    use crate::compositor::LockState;
    let (mut f, output, _lock) = locking_fixture();
    f.realand()
        .finish_lock_frame(&output, &RenderResult::Skipped, true, false);
    assert!(matches!(f.realand().lock_state, LockState::Unlocked));
}

#[test]
fn gpu_capacity_deferral_keeps_the_real_locker_pending_on_every_backend() {
    use crate::compositor::{LockRenderState, LockState};
    let (mut f, output, _lock) = locking_fixture();
    f.realand()
        .finish_lock_frame(&output, &RenderResult::GpuDeferred, true, false);
    assert!(matches!(f.realand().lock_state, LockState::Locking(_)));
    assert!(matches!(
        f.realand().output_state[&output].lock_render_state,
        LockRenderState::Unlocked
    ));
    f.realand()
        .finish_lock_frame(&output, &RenderResult::Submitted, true, false);
    assert!(matches!(f.realand().lock_state, LockState::Locked(_)));
}
