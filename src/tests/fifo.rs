use std::time::Duration;

use smithay::backend::renderer::element::{
    default_primary_scanout_output_compare, Id, RenderElementPresentationState, RenderElementState,
    RenderElementStates,
};
use smithay::desktop::utils::update_surface_primary_scanout_output;
use smithay::output::Output;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::wayland_protocols::wp::fifo::v1::client::wp_fifo_manager_v1;
use smithay::reexports::wayland_protocols::wp::fifo::v1::client::wp_fifo_v1::{self, WpFifoV1};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface as ServerSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::compositor::{with_states, SurfaceAttributes};
use wayland_backend::protocol::ProtocolError;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::Proxy;

use super::client::ClientId;
use super::Fixture;
use crate::backend::tty::Tty;
use crate::compositor::{PointerVisibility, RedrawState};
use crate::utils::get_monotonic_time;

fn setup() -> (Fixture, ClientId, WlSurface, ServerSurface, WpFifoV1) {
    let mut f = Fixture::new();
    // These tests advance FIFO frame events explicitly. An unrelated cursor
    // asset completion would queue a real headless Submitted frame and legally
    // release the barrier before the test's manual estimated-vblank step.
    f.clarion().pointer_visibility = PointerVisibility::Disabled;
    let id = f.add_client();
    let client = f.client(id);
    let surface = client
        .state
        .compositor
        .as_ref()
        .unwrap()
        .create_surface(&client.qh, ());
    let fifo = client
        .state
        .fifo_manager
        .as_ref()
        .expect("FIFO global is advertised")
        .get_fifo(&surface, &client.qh, ());
    surface.commit();
    f.double_roundtrip(id);
    let server = f
        .clarion()
        .root_surface
        .keys()
        .find(|s| s.id().protocol_id() == surface.id().protocol_id())
        .unwrap()
        .clone();
    (f, id, surface, server, fifo)
}

fn scale(surface: &ServerSurface) -> i32 {
    with_states(surface, |s| {
        s.cached_state
            .get::<SurfaceAttributes>()
            .current()
            .buffer_scale
    })
}

fn mark_visible(surface: &ServerSurface, output: &Output) {
    // Supply the same visibility state used by both GLES and Vulkan render paths.
    let mut states = RenderElementStates::default();
    states.states.insert(
        Id::from_wayland_resource(surface),
        RenderElementState {
            visible_area: 100,
            presentation_state: RenderElementPresentationState::Rendering { reason: None },
            needs_capture: false,
        },
    );
    with_states(surface, |s| {
        update_surface_primary_scanout_output(
            surface,
            output,
            s,
            None,
            &states,
            default_primary_scanout_output_compare,
        );
    });
}

fn frame(f: &mut Fixture, output: &Output, defer: bool) {
    let r = f.clarion();
    r.fifo.frame_rendered(output, defer, &r.blocker_cleared_tx);
}

fn expire(f: &mut Fixture, now: Duration) {
    let r = f.clarion();
    r.fifo.expire(now, &r.blocker_cleared_tx);
}

fn receive_protocol_error(f: &mut Fixture, id: ClientId) -> ProtocolError {
    f.client(id).connection.flush().unwrap();
    for _ in 0..10 {
        f.state.server.dispatch();
        let client = f.client(id);
        client
            .event_loop
            .dispatch(Duration::ZERO, &mut client.state)
            .unwrap();
        if let Some(error) = client.connection.protocol_error() {
            return error;
        }
    }
    panic!("server did not deliver the expected protocol error");
}

#[test]
fn duplicate_fifo_reports_error_on_the_manager_object() {
    let (mut f, id, surface, _server, _fifo) = setup();
    let client = f.client(id);
    client
        .state
        .fifo_manager
        .as_ref()
        .unwrap()
        .get_fifo(&surface, &client.qh, ());

    let error = receive_protocol_error(&mut f, id);
    assert_eq!(error.code, wp_fifo_manager_v1::Error::AlreadyExists as u32);
    assert_eq!(error.object_interface, "wp_fifo_manager_v1");
}

#[test]
fn fifo_can_be_recreated_after_destroy() {
    let (mut f, id, surface, server, fifo) = setup();
    fifo.destroy();
    f.double_roundtrip(id);

    let client = f.client(id);
    let replacement =
        client
            .state
            .fifo_manager
            .as_ref()
            .unwrap()
            .get_fifo(&surface, &client.qh, ());
    replacement.set_barrier();
    surface.commit();
    replacement.wait_barrier();
    surface.set_buffer_scale(2);
    surface.commit();
    f.double_roundtrip(id);
    assert_eq!(scale(&server), 1);
    expire(&mut f, get_monotonic_time() + Duration::from_secs(2));
    f.double_roundtrip(id);
    assert_eq!(scale(&server), 2);
}

#[test]
fn fifo_set_after_surface_destroy_reports_the_fifo_error() {
    let (mut f, id, surface, _server, fifo) = setup();
    surface.destroy();
    f.double_roundtrip(id);
    fifo.set_barrier();

    let error = receive_protocol_error(&mut f, id);
    assert_eq!(error.code, wp_fifo_v1::Error::SurfaceDestroyed as u32);
    assert_eq!(error.object_interface, "wp_fifo_v1");
}

#[test]
fn fifo_wait_after_surface_destroy_reports_the_fifo_error() {
    let (mut f, id, surface, _server, fifo) = setup();
    surface.destroy();
    f.double_roundtrip(id);
    fifo.wait_barrier();

    let error = receive_protocol_error(&mut f, id);
    assert_eq!(error.code, wp_fifo_v1::Error::SurfaceDestroyed as u32);
    assert_eq!(error.object_interface, "wp_fifo_v1");
}

#[test]
fn fifo_orders_multiple_commits_without_frame_callbacks() {
    let (mut f, id, surface, server, fifo) = setup();
    f.add_output(1, (1920, 1080));
    let output = f.clarion_output(1);
    fifo.set_barrier();
    surface.commit();
    for value in [2, 3] {
        fifo.wait_barrier();
        fifo.set_barrier();
        surface.set_buffer_scale(value);
        surface.commit();
    }
    f.double_roundtrip(id);
    assert_eq!(scale(&server), 1);
    mark_visible(&server, &output);
    frame(&mut f, &output, false);
    f.double_roundtrip(id);
    assert_eq!(scale(&server), 2);
    frame(&mut f, &output, false);
    f.double_roundtrip(id);
    assert_eq!(scale(&server), 3);
}

#[test]
fn empty_frame_waits_for_estimated_vblank_and_does_not_release_newer_barrier() {
    let (mut f, id, surface, server, fifo) = setup();
    f.add_output(1, (1920, 1080));
    let output = f.clarion_output(1);
    fifo.set_barrier();
    surface.commit();
    f.double_roundtrip(id);
    mark_visible(&server, &output);
    frame(&mut f, &output, true);
    // This barrier did not participate in the frame already awaiting its timer.
    fifo.set_barrier();
    surface.set_buffer_scale(2);
    surface.commit();
    fifo.wait_barrier();
    surface.set_buffer_scale(3);
    surface.commit();
    f.double_roundtrip(id);
    assert_eq!(scale(&server), 2);
    f.clarion().release_fifo_after_estimated_vblank(&output);
    assert!(
        f.clarion().blocker_cleared_rx.try_recv().is_err(),
        "a superseded barrier must not wake the transaction queue again"
    );
    f.double_roundtrip(id);
    assert_eq!(scale(&server), 2);
    frame(&mut f, &output, true);
    f.double_roundtrip(id);
    assert_eq!(scale(&server), 2);
    f.clarion().release_fifo_after_estimated_vblank(&output);
    f.double_roundtrip(id);
    assert_eq!(scale(&server), 3);
}

#[test]
fn estimated_vblank_releases_old_fifo_frame_while_new_latch_remains_pending() {
    let (mut f, id, surface, server, fifo) = setup();
    f.add_output(1, (1920, 1080));
    let output = f.clarion_output(1);
    fifo.set_barrier();
    surface.commit();
    fifo.wait_barrier();
    surface.set_buffer_scale(2);
    surface.commit();
    f.double_roundtrip(id);
    mark_visible(&server, &output);
    frame(&mut f, &output, true);
    assert_eq!(scale(&server), 1);

    // This second surface becomes visible after the empty-frame snapshot. Its
    // barrier must wait for a newer frame, even when the older deadline fires.
    let client = f.client(id);
    let newer_surface = client
        .state
        .compositor
        .as_ref()
        .unwrap()
        .create_surface(&client.qh, ());
    let newer_fifo =
        client
            .state
            .fifo_manager
            .as_ref()
            .unwrap()
            .get_fifo(&newer_surface, &client.qh, ());
    newer_fifo.set_barrier();
    newer_surface.commit();
    newer_fifo.wait_barrier();
    newer_surface.set_buffer_scale(3);
    newer_surface.commit();
    f.double_roundtrip(id);
    let newer_server = f
        .clarion()
        .root_surface
        .keys()
        .find(|s| s.id().protocol_id() == newer_surface.id().protocol_id())
        .unwrap()
        .clone();
    mark_visible(&newer_server, &output);
    assert_eq!(scale(&newer_server), 1);

    // Keep both sources far in the future and dispatch the older deadline
    // explicitly, so scheduler timing cannot accidentally fire the new latch.
    let handle = &f.clarion().event_loop;
    let estimated = handle
        .insert_source(Timer::from_duration(Duration::from_secs(60)), |_, _, _| {
            TimeoutAction::Drop
        })
        .unwrap();
    let latch = handle
        .insert_source(Timer::from_duration(Duration::from_secs(60)), |_, _, _| {
            TimeoutAction::Drop
        })
        .unwrap();
    f.clarion()
        .output_state
        .get_mut(&output)
        .unwrap()
        .schedule
        .test_set_redraw(RedrawState::WaitingForLatch {
            token: latch,
            estimated_vblank: Some(estimated),
        });
    f.clarion().event_loop.remove(estimated);
    Tty::on_estimated_vblank_timer(
        &mut crate::platform::device_bridge::DeviceCoordinator::new(f.clarion()),
        output.clone(),
    );
    f.double_roundtrip(id);

    assert_eq!(
        scale(&server),
        2,
        "the old frame must not wait for the new latch"
    );
    assert_eq!(scale(&newer_server), 1, "a newer barrier must stay blocked");
    assert!(matches!(
        f.clarion().output_state[&output].schedule.redraw_state(),
        RedrawState::WaitingForLatch {
            token,
            estimated_vblank: None,
        } if token == latch
    ));
    assert_eq!(f.clarion().next_queued_output(get_monotonic_time()), None);

    // A later rendered frame, rather than the older estimate, releases the
    // second surface. No barrier is lost while the two deadlines overlap.
    frame(&mut f, &output, false);
    f.double_roundtrip(id);
    assert_eq!(scale(&newer_server), 3);
}

#[test]
fn wrong_output_cannot_release_barrier() {
    let (mut f, id, surface, server, fifo) = setup();
    f.add_output(1, (1920, 1080));
    f.add_output(2, (1280, 720));
    let primary = f.clarion_output(1);
    let secondary = f.clarion_output(2);
    fifo.set_barrier();
    surface.commit();
    fifo.wait_barrier();
    surface.set_buffer_scale(2);
    surface.commit();
    f.double_roundtrip(id);
    mark_visible(&server, &primary);
    frame(&mut f, &secondary, false);
    f.double_roundtrip(id);
    assert_eq!(scale(&server), 1);
    frame(&mut f, &primary, false);
    f.double_roundtrip(id);
    assert_eq!(scale(&server), 2);
}

#[test]
fn no_output_fallback_and_fifo_destruction_preserve_progress() {
    let (mut f, id, surface, server, fifo) = setup();
    fifo.set_barrier();
    surface.commit();
    fifo.wait_barrier();
    surface.set_buffer_scale(2);
    surface.commit();
    fifo.destroy();
    f.double_roundtrip(id);
    assert_eq!(
        scale(&server),
        1,
        "destroy does not cancel committed constraints"
    );
    expire(&mut f, Duration::ZERO);
    f.double_roundtrip(id);
    assert_eq!(scale(&server), 1);
    expire(&mut f, get_monotonic_time() + Duration::from_secs(2));
    f.double_roundtrip(id);
    assert_eq!(scale(&server), 2);
}

#[test]
fn output_removal_releases_an_awaited_empty_frame() {
    let (mut f, id, surface, server, fifo) = setup();
    f.add_output(1, (1920, 1080));
    let output = f.clarion_output(1);
    fifo.set_barrier();
    surface.commit();
    fifo.wait_barrier();
    surface.set_buffer_scale(2);
    surface.commit();
    f.double_roundtrip(id);
    mark_visible(&server, &output);
    frame(&mut f, &output, true);
    f.clarion().remove_output(&output);
    f.double_roundtrip(id);
    assert_eq!(scale(&server), 2);
}

#[test]
fn requests_are_double_buffered_and_wait_without_barrier_is_ready() {
    let (mut f, id, surface, server, fifo) = setup();
    fifo.wait_barrier();
    surface.set_buffer_scale(2);
    surface.commit();
    f.double_roundtrip(id);
    assert_eq!(scale(&server), 2);
    fifo.set_barrier();
    surface.set_buffer_scale(3);
    f.double_roundtrip(id);
    assert_eq!(scale(&server), 2);
    surface.commit();
    fifo.wait_barrier();
    surface.set_buffer_scale(4);
    surface.commit();
    f.double_roundtrip(id);
    assert_eq!(scale(&server), 3);
    expire(&mut f, get_monotonic_time() + Duration::from_secs(2));
    f.double_roundtrip(id);
    assert_eq!(scale(&server), 4);
}

#[test]
fn repeated_commits_do_not_extend_the_fallback_deadline() {
    let (mut f, id, surface, server, fifo) = setup();
    fifo.set_barrier();
    surface.commit();
    f.double_roundtrip(id);
    let deadline = get_monotonic_time() + Duration::from_secs(1);
    // Simulate later non-barrier commits while keeping the test deterministic.
    f.clarion().fifo.committed(&server, deadline);
    fifo.wait_barrier();
    surface.set_buffer_scale(2);
    surface.commit();
    f.double_roundtrip(id);
    assert_eq!(scale(&server), 1);
    expire(&mut f, deadline);
    f.double_roundtrip(id);
    assert_eq!(scale(&server), 2);
}

#[test]
fn destroying_a_surface_releases_its_barrier_and_keeps_the_client_alive() {
    let (mut f, id, surface, server, fifo) = setup();
    fifo.set_barrier();
    surface.commit();
    fifo.wait_barrier();
    surface.set_buffer_scale(2);
    surface.commit();
    f.double_roundtrip(id);
    let barrier = with_states(&server, |s| {
        s.cached_state
            .get::<smithay::wayland::fifo::FifoBarrierCachedState>()
            .current()
            .barrier
            .clone()
            .unwrap()
    });
    assert!(!barrier.is_signaled());
    surface.destroy();
    fifo.destroy();
    f.double_roundtrip(id);
    assert!(barrier.is_signaled());
    expire(&mut f, get_monotonic_time() + Duration::from_secs(2));
    f.double_roundtrip(id);
}

#[test]
fn synchronized_subsurfaces_ignore_fifo_waits_but_desynchronized_ones_wait() {
    let (mut f, id, surface, server, fifo) = setup();
    let client = f.client(id);
    let parent = client
        .state
        .compositor
        .as_ref()
        .unwrap()
        .create_surface(&client.qh, ());
    let subsurface = client.state.subcompositor.as_ref().unwrap().get_subsurface(
        &surface,
        &parent,
        &client.qh,
        (),
    );
    fifo.set_barrier();
    surface.commit();
    parent.commit();
    f.double_roundtrip(id);
    fifo.wait_barrier();
    fifo.set_barrier();
    surface.set_buffer_scale(2);
    surface.commit();
    f.double_roundtrip(id);
    assert_eq!(scale(&server), 1, "sync child still waits for its parent");
    parent.commit();
    f.double_roundtrip(id);
    assert_eq!(
        scale(&server),
        2,
        "FIFO must not block synchronized commits"
    );
    subsurface.set_desync();
    fifo.wait_barrier();
    surface.set_buffer_scale(3);
    surface.commit();
    f.double_roundtrip(id);
    assert_eq!(scale(&server), 2);
    expire(&mut f, get_monotonic_time() + Duration::from_secs(2));
    f.double_roundtrip(id);
    assert_eq!(scale(&server), 3);
}

#[test]
fn skipped_redraw_does_not_release_but_submitted_redraw_does() {
    let (mut f, id, surface, server, fifo) = setup();
    f.add_output(1, (1920, 1080));
    let output = f.clarion_output(1);
    fifo.set_barrier();
    surface.commit();
    fifo.wait_barrier();
    surface.set_buffer_scale(2);
    surface.commit();
    f.double_roundtrip(id);
    mark_visible(&server, &output);
    f.clarion().monitors_active = false;
    f.clarion().queue_redraw(&output);
    f.double_roundtrip(id);
    assert_eq!(scale(&server), 1);
    f.clarion().monitors_active = true;
    f.clarion().queue_redraw(&output);
    f.double_roundtrip(id);
    assert_eq!(scale(&server), 2);
}

#[test]
fn real_fallback_timer_releases_a_surface_while_monitors_are_off() {
    let (mut f, id, surface, server, fifo) = setup();
    f.add_output(1, (1920, 1080));
    f.clarion().monitors_active = false;
    fifo.set_barrier();
    surface.commit();
    fifo.wait_barrier();
    surface.set_buffer_scale(2);
    surface.commit();
    f.double_roundtrip(id);
    assert_eq!(scale(&server), 1);
    let started = std::time::Instant::now();
    while scale(&server) == 1 && started.elapsed() < Duration::from_secs(4) {
        let server = &mut f.state.server;
        server
            .event_loop
            .dispatch(Duration::from_millis(100), &mut server.state)
            .unwrap();
        server.state.refresh_and_flush_clients();
    }
    assert_eq!(
        scale(&server),
        2,
        "the production fallback timer must notify cleared blockers"
    );
}

#[test]
fn nested_fifo_paces_empty_frames_with_one_timer() {
    use crate::backend::RenderResult;
    use crate::frame_clock::FrameClock;

    let result = RenderResult::NoDamage;
    let (mut f, id, surface, server, fifo) = setup();
    f.add_output(1, (1920, 1080));
    let output = f.clarion_output(1);
    f.clarion()
        .output_state
        .get_mut(&output)
        .unwrap()
        .schedule
        .test_set_clock(FrameClock::new(Some(Duration::from_millis(200)), false));
    // No FIFO work should allocate a timer.
    f.clarion()
        .finish_fifo_frame(&output, &result, true, std::time::Instant::now());
    assert!(f.clarion().output_state[&output]
        .schedule
        .fifo_timer()
        .is_none());
    fifo.set_barrier();
    surface.commit();
    fifo.wait_barrier();
    surface.set_buffer_scale(2);
    surface.commit();
    f.double_roundtrip(id);
    mark_visible(&server, &output);
    f.clarion().finish_fifo_frame(
        &output,
        &RenderResult::Skipped,
        true,
        std::time::Instant::now(),
    );
    assert!(f.clarion().output_state[&output]
        .schedule
        .fifo_timer()
        .is_none());
    assert_eq!(scale(&server), 1);
    f.clarion()
        .finish_fifo_frame(&output, &result, true, std::time::Instant::now());
    let token = f.clarion().output_state[&output]
        .schedule
        .fifo_timer()
        .unwrap();
    // Repeated redraws neither create extra timers nor postpone this deadline.
    for _ in 0..10 {
        f.clarion()
            .finish_fifo_frame(&output, &result, true, std::time::Instant::now());
        assert_eq!(
            f.clarion().output_state[&output].schedule.fifo_timer(),
            Some(token)
        );
    }
    assert_eq!(scale(&server), 1);
    let started = std::time::Instant::now();
    while scale(&server) == 1 && started.elapsed() < Duration::from_secs(3) {
        let server = &mut f.state.server;
        server
            .event_loop
            .dispatch(Duration::from_millis(20), &mut server.state)
            .unwrap();
        server.state.refresh_and_flush_clients();
    }
    assert_eq!(scale(&server), 2);
    assert!(f.clarion().output_state[&output]
        .schedule
        .fifo_timer()
        .is_none());
    assert!(!f.clarion().fifo.has_estimated_frame(&output));
}

#[test]
fn consecutive_nested_empty_frames_keep_separate_fifo_deadlines() {
    let (mut f, id, surface, server, fifo) = setup();
    f.add_output(1, (1920, 1080));
    let output = f.clarion_output(1);
    // Keep the test's manually staged frames from being rendered by the headless backend.
    f.clarion().monitors_active = false;
    let interval = Duration::from_millis(80);
    f.clarion()
        .output_state
        .get_mut(&output)
        .unwrap()
        .schedule
        .test_set_clock(crate::frame_clock::FrameClock::new(Some(interval), false));

    fifo.set_barrier();
    surface.commit();
    f.double_roundtrip(id);
    mark_visible(&server, &output);
    let first = with_states(&server, |s| {
        s.cached_state
            .get::<smithay::wayland::fifo::FifoBarrierCachedState>()
            .current()
            .barrier
            .clone()
            .unwrap()
    });
    let started = std::time::Instant::now();
    f.clarion().finish_fifo_frame(
        &output,
        &crate::backend::RenderResult::NoDamage,
        true,
        started,
    );
    let timer = f.clarion().output_state[&output]
        .schedule
        .fifo_timer()
        .unwrap();

    // A second client surface commits during the first surface's estimated refresh. Replacing
    // one surface's barrier would signal the old one, so use independent live barriers here.
    let (second_surface, second_fifo) =
        {
            let client = f.client(id);
            let second_surface = client
                .state
                .compositor
                .as_ref()
                .unwrap()
                .create_surface(&client.qh, ());
            let second_fifo = client.state.fifo_manager.as_ref().unwrap().get_fifo(
                &second_surface,
                &client.qh,
                (),
            );
            (second_surface, second_fifo)
        };
    second_fifo.set_barrier();
    second_surface.commit();
    f.double_roundtrip(id);
    let second_server = f
        .clarion()
        .root_surface
        .keys()
        .find(|s| s.id().protocol_id() == second_surface.id().protocol_id())
        .unwrap()
        .clone();
    mark_visible(&second_server, &output);
    let second = with_states(&second_server, |s| {
        s.cached_state
            .get::<smithay::wayland::fifo::FifoBarrierCachedState>()
            .current()
            .barrier
            .clone()
            .unwrap()
    });
    assert!(!first.is_signaled());
    assert!(!second.is_signaled());
    let second_started = started + Duration::from_millis(200);
    f.clarion().finish_fifo_frame(
        &output,
        &crate::backend::RenderResult::NoDamage,
        true,
        second_started,
    );
    assert_eq!(
        f.clarion().output_state[&output].schedule.fifo_timer(),
        Some(timer)
    );
    assert_eq!(
        f.clarion().fifo.next_estimated_deadline(&output),
        Some(started + interval)
    );

    let first_wait = std::time::Instant::now();
    while !first.is_signaled() && first_wait.elapsed() < Duration::from_secs(1) {
        let server_loop = &mut f.state.server;
        server_loop
            .event_loop
            .dispatch(Duration::from_millis(20), &mut server_loop.state)
            .unwrap();
    }
    assert!(first.is_signaled());
    if std::time::Instant::now() < second_started + interval {
        assert!(
            !second.is_signaled(),
            "the later frame must wait for its own deadline"
        );
        assert!(f.clarion().output_state[&output]
            .schedule
            .fifo_timer()
            .is_some());
        assert_eq!(
            f.clarion().fifo.next_estimated_deadline(&output),
            Some(second_started + interval)
        );
    }

    let second_wait = std::time::Instant::now();
    while !second.is_signaled() && second_wait.elapsed() < Duration::from_secs(1) {
        let server_loop = &mut f.state.server;
        server_loop
            .event_loop
            .dispatch(Duration::from_millis(20), &mut server_loop.state)
            .unwrap();
    }
    assert!(second.is_signaled());
    assert!(f.clarion().output_state[&output]
        .schedule
        .fifo_timer()
        .is_none());
    assert!(!f.clarion().fifo.has_estimated_frame(&output));
}

#[test]
fn removing_nested_output_cancels_its_timer_and_releases_waiters() {
    let (mut f, id, surface, server, fifo) = setup();
    f.add_output(1, (1920, 1080));
    let output = f.clarion_output(1);
    fifo.set_barrier();
    surface.commit();
    fifo.wait_barrier();
    surface.set_buffer_scale(2);
    surface.commit();
    f.double_roundtrip(id);
    mark_visible(&server, &output);
    f.clarion().finish_fifo_frame(
        &output,
        &crate::backend::RenderResult::NoDamage,
        true,
        std::time::Instant::now(),
    );
    assert!(f.clarion().output_state[&output]
        .schedule
        .fifo_timer()
        .is_some());
    f.clarion().remove_output(&output);
    f.double_roundtrip(id);
    assert_eq!(scale(&server), 2);
    assert!(!f.clarion().fifo.has_estimated_frame(&output));
    let server = &mut f.state.server;
    server
        .event_loop
        .dispatch(Duration::from_millis(40), &mut server.state)
        .unwrap();
    server.state.refresh_and_flush_clients();
}

#[test]
fn nested_fifo_counts_time_already_spent_rendering_empty_frame() {
    let (mut f, id, surface, server, fifo) = setup();
    f.add_output(1, (1920, 1080));
    let output = f.clarion_output(1);
    f.clarion()
        .output_state
        .get_mut(&output)
        .unwrap()
        .schedule
        .test_set_clock(crate::frame_clock::FrameClock::new(
            Some(Duration::from_millis(500)),
            false,
        ));
    fifo.set_barrier();
    surface.commit();
    fifo.wait_barrier();
    surface.set_buffer_scale(2);
    surface.commit();
    f.double_roundtrip(id);
    mark_visible(&server, &output);
    f.clarion().finish_fifo_frame(
        &output,
        &crate::backend::RenderResult::NoDamage,
        true,
        std::time::Instant::now() - Duration::from_secs(1),
    );
    // The refresh deadline already passed. A nonblocking event-loop iteration
    // must suffice; no extra frame duration should be added after host submission.
    let server_loop = &mut f.state.server;
    server_loop
        .event_loop
        .dispatch(Duration::ZERO, &mut server_loop.state)
        .unwrap();
    server_loop.state.refresh_and_flush_clients();
    assert_eq!(scale(&server), 2);
    assert!(f.clarion().output_state[&output]
        .schedule
        .fifo_timer()
        .is_none());
}

#[test]
fn nested_submitted_frame_is_not_throttled_twice() {
    let (mut f, id, surface, server, fifo) = setup();
    f.add_output(1, (1920, 1080));
    let output = f.clarion_output(1);
    fifo.set_barrier();
    surface.commit();
    fifo.wait_barrier();
    surface.set_buffer_scale(2);
    surface.commit();
    f.double_roundtrip(id);
    mark_visible(&server, &output);
    f.clarion().finish_fifo_frame(
        &output,
        &crate::backend::RenderResult::Submitted,
        true,
        std::time::Instant::now(),
    );
    f.double_roundtrip(id);
    assert_eq!(scale(&server), 2);
    assert!(f.clarion().output_state[&output]
        .schedule
        .fifo_timer()
        .is_none());
}

// Stage a long nested estimate with no real redraw or wall-clock wait. Each
// cleanup path below must retire its timer immediately when it releases the work.
fn staged_nested_fifo_timer() -> (
    Fixture,
    ClientId,
    WlSurface,
    ServerSurface,
    WpFifoV1,
    Output,
) {
    let (mut f, id, surface, server, fifo) = setup();
    f.add_output(1, (1920, 1080));
    let output = f.clarion_output(1);
    f.clarion().monitors_active = false;
    f.clarion()
        .output_state
        .get_mut(&output)
        .unwrap()
        .schedule
        .test_set_clock(crate::frame_clock::FrameClock::new(
            Some(Duration::from_secs(2)),
            false,
        ));
    fifo.set_barrier();
    surface.commit();
    fifo.wait_barrier();
    surface.set_buffer_scale(2);
    surface.commit();
    f.double_roundtrip(id);
    assert_eq!(scale(&server), 1);
    mark_visible(&server, &output);
    f.clarion().finish_fifo_frame(
        &output,
        &crate::backend::RenderResult::NoDamage,
        true,
        std::time::Instant::now(),
    );
    assert!(f.clarion().output_state[&output]
        .schedule
        .fifo_timer()
        .is_some());
    assert!(f.clarion().fifo.has_estimated_frame(&output));
    (f, id, surface, server, fifo, output)
}

#[test]
fn non_nested_submitted_frame_cancels_superseded_nested_fifo_timer() {
    let (mut f, id, _surface, server, _fifo, output) = staged_nested_fifo_timer();
    f.clarion().finish_fifo_frame(
        &output,
        &crate::backend::RenderResult::Submitted,
        false,
        std::time::Instant::now(),
    );
    assert!(f.clarion().output_state[&output]
        .schedule
        .fifo_timer()
        .is_none());
    assert!(!f.clarion().fifo.has_estimated_frame(&output));
    f.double_roundtrip(id);
    assert_eq!(scale(&server), 2);
}

#[test]
fn estimated_vblank_cancels_the_released_nested_fifo_timer() {
    let (mut f, id, _surface, server, _fifo, output) = staged_nested_fifo_timer();
    f.clarion().release_fifo_after_estimated_vblank(&output);
    assert!(f.clarion().output_state[&output]
        .schedule
        .fifo_timer()
        .is_none());
    assert!(!f.clarion().fifo.has_estimated_frame(&output));
    f.double_roundtrip(id);
    assert_eq!(scale(&server), 2);
}

#[test]
fn fallback_expiry_cancels_the_released_nested_fifo_timer() {
    let (mut f, id, _surface, server, _fifo, output) = staged_nested_fifo_timer();
    f.clarion()
        .expire_fifo_barriers(get_monotonic_time() + Duration::from_secs(2));
    assert!(f.clarion().output_state[&output]
        .schedule
        .fifo_timer()
        .is_none());
    assert!(!f.clarion().fifo.has_estimated_frame(&output));
    f.double_roundtrip(id);
    assert_eq!(scale(&server), 2);
}

#[test]
fn destroyed_surface_cancels_its_last_nested_fifo_timer() {
    let (mut f, id, surface, _server, _fifo, output) = staged_nested_fifo_timer();
    surface.destroy();
    f.double_roundtrip(id);
    assert!(f.clarion().output_state[&output]
        .schedule
        .fifo_timer()
        .is_none());
    assert!(!f.clarion().fifo.has_estimated_frame(&output));
}
