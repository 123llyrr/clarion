use std::time::{Duration, Instant};

use smithay::reexports::wayland_protocols::wp::commit_timing::v1::client::{
    wp_commit_timer_v1::{self, WpCommitTimerV1},
    wp_commit_timing_manager_v1,
};
use smithay::reexports::wayland_server::{
    protocol::wl_surface::WlSurface as ServerSurface, Resource,
};
use smithay::wayland::compositor::{with_states, SurfaceAttributes};
use wayland_backend::protocol::ProtocolError;
use wayland_client::{protocol::wl_surface::WlSurface, Proxy};

use super::{client::ClientId, Fixture};
use crate::utils::get_monotonic_time;

fn setup() -> (Fixture, ClientId, WlSurface, ServerSurface, WpCommitTimerV1) {
    let mut fixture = Fixture::new();
    let id = fixture.add_client();
    let client = fixture.client(id);
    let surface = client
        .state
        .compositor
        .as_ref()
        .unwrap()
        .create_surface(&client.qh, ());
    surface.commit();
    fixture.double_roundtrip(id);
    let server_surface = fixture
        .realand()
        .root_surface
        .keys()
        .find(|candidate| candidate.id().protocol_id() == surface.id().protocol_id())
        .unwrap()
        .clone();
    let client = fixture.client(id);
    let timer = client
        .state
        .commit_timing_manager
        .as_ref()
        .expect("commit-timing global is advertised")
        .get_timer(&surface, &client.qh, ());
    fixture.double_roundtrip(id);
    (fixture, id, surface, server_surface, timer)
}

fn scale(surface: &ServerSurface) -> i32 {
    with_states(surface, |states| {
        states
            .cached_state
            .get::<SurfaceAttributes>()
            .current()
            .buffer_scale
    })
}

fn set_timestamp(timer: &WpCommitTimerV1, timestamp: Duration) {
    let seconds = timestamp.as_secs();
    timer.set_timestamp(
        (seconds >> 32) as u32,
        seconds as u32,
        timestamp.subsec_nanos(),
    );
}

fn dispatch_until(
    fixture: &mut Fixture,
    timeout: Duration,
    condition: impl Fn(&mut Fixture) -> bool,
) {
    let started = Instant::now();
    while !condition(fixture) && started.elapsed() < timeout {
        let server = &mut fixture.state.server;
        server
            .event_loop
            .dispatch(Duration::from_millis(10), &mut server.state)
            .unwrap();
        server.state.refresh_and_flush_clients();
    }
}

fn receive_protocol_error(fixture: &mut Fixture, id: ClientId) -> ProtocolError {
    fixture.client(id).connection.flush().unwrap();
    for _ in 0..10 {
        fixture.state.server.dispatch();
        let client = fixture.client(id);
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
fn future_timestamp_blocks_until_its_monotonic_deadline() {
    let (mut fixture, id, surface, server_surface, timer) = setup();
    let deadline = get_monotonic_time() + Duration::from_millis(120);
    set_timestamp(&timer, deadline);
    surface.set_buffer_scale(2);
    surface.commit();
    fixture.double_roundtrip(id);
    assert_eq!(scale(&server_surface), 1);

    dispatch_until(&mut fixture, Duration::from_secs(1), |_| {
        scale(&server_surface) == 2
    });
    assert_eq!(scale(&server_surface), 2);
    assert!(get_monotonic_time() >= deadline);
    assert!(fixture.realand().commit_timing.scheduled.is_empty());
}

#[test]
fn past_timestamp_and_untimed_followup_preserve_commit_order() {
    let (mut fixture, id, surface, server_surface, timer) = setup();
    set_timestamp(
        &timer,
        get_monotonic_time().saturating_sub(Duration::from_secs(1)),
    );
    surface.set_buffer_scale(2);
    surface.commit();
    surface.set_buffer_scale(3);
    surface.commit();
    fixture.double_roundtrip(id);
    dispatch_until(&mut fixture, Duration::from_secs(1), |_| {
        scale(&server_surface) == 3
    });
    assert_eq!(scale(&server_surface), 3);
}

#[test]
fn earlier_second_deadline_cannot_overtake_an_older_commit() {
    let (mut fixture, id, surface, server_surface, timer) = setup();
    let now = get_monotonic_time();
    let later = now + Duration::from_millis(180);
    set_timestamp(&timer, later);
    surface.set_buffer_scale(2);
    surface.commit();
    set_timestamp(&timer, now + Duration::from_millis(60));
    surface.set_buffer_scale(3);
    surface.commit();
    fixture.double_roundtrip(id);
    std::thread::sleep(Duration::from_millis(90));
    fixture.dispatch();
    fixture.state.server.state.refresh_and_flush_clients();
    assert_eq!(scale(&server_surface), 1);
    dispatch_until(&mut fixture, Duration::from_secs(1), |_| {
        scale(&server_surface) == 3
    });
    assert_eq!(scale(&server_surface), 3);
    assert!(get_monotonic_time() >= later);
}

#[test]
fn multiple_deadlines_share_one_timer_and_rearm_for_the_next_commit() {
    let (mut fixture, id, surface, server_surface, timer) = setup();
    let now = get_monotonic_time();
    let first = now + Duration::from_millis(80);
    let second = now + Duration::from_millis(180);
    set_timestamp(&timer, first);
    surface.set_buffer_scale(2);
    surface.commit();
    set_timestamp(&timer, second);
    surface.set_buffer_scale(3);
    surface.commit();
    fixture.double_roundtrip(id);
    assert_eq!(fixture.realand().commit_timing.scheduled.len(), 1);

    dispatch_until(&mut fixture, Duration::from_millis(140), |_| {
        scale(&server_surface) == 2
    });
    assert_eq!(scale(&server_surface), 2);
    assert_eq!(fixture.realand().commit_timing.scheduled.len(), 1);
    dispatch_until(&mut fixture, Duration::from_secs(1), |_| {
        scale(&server_surface) == 3
    });
    assert_eq!(scale(&server_surface), 3);
    assert!(fixture.realand().commit_timing.scheduled.is_empty());
}

#[test]
fn fifo_and_commit_timing_constraints_must_both_clear() {
    let (mut fixture, id, surface, server_surface, timer) = setup();
    let client = fixture.client(id);
    let fifo = client
        .state
        .fifo_manager
        .as_ref()
        .unwrap()
        .get_fifo(&surface, &client.qh, ());
    fifo.set_barrier();
    surface.commit();
    fixture.double_roundtrip(id);

    fifo.wait_barrier();
    set_timestamp(&timer, get_monotonic_time() + Duration::from_millis(80));
    surface.set_buffer_scale(2);
    surface.commit();
    fixture.double_roundtrip(id);
    dispatch_until(&mut fixture, Duration::from_millis(180), |_| {
        scale(&server_surface) == 2
    });
    assert_eq!(scale(&server_surface), 1, "FIFO remains pending");

    let realand = fixture.realand();
    realand.fifo.expire(
        get_monotonic_time() + Duration::from_secs(2),
        &realand.blocker_cleared_tx,
    );
    fixture.double_roundtrip(id);
    assert_eq!(scale(&server_surface), 2);
}

#[test]
fn destroying_timer_does_not_cancel_a_committed_constraint() {
    let (mut fixture, id, surface, server_surface, timer) = setup();
    let deadline = get_monotonic_time() + Duration::from_millis(100);
    set_timestamp(&timer, deadline);
    surface.set_buffer_scale(2);
    surface.commit();
    timer.destroy();
    fixture.double_roundtrip(id);
    assert_eq!(scale(&server_surface), 1);
    dispatch_until(&mut fixture, Duration::from_secs(1), |_| {
        scale(&server_surface) == 2
    });
    assert_eq!(scale(&server_surface), 2);
}

#[test]
fn destroying_surface_cancels_timer_and_releases_its_barriers() {
    let (mut fixture, id, surface, _server_surface, timer) = setup();
    set_timestamp(&timer, get_monotonic_time() + Duration::from_secs(60));
    surface.commit();
    fixture.double_roundtrip(id);
    assert_eq!(fixture.realand().commit_timing.scheduled.len(), 1);
    surface.destroy();
    timer.destroy();
    fixture.double_roundtrip(id);
    assert!(fixture.realand().commit_timing.scheduled.is_empty());
}

#[test]
fn invalid_nanoseconds_report_the_mandated_timer_error() {
    let (mut fixture, id, _surface, _server_surface, timer) = setup();
    let seconds = get_monotonic_time().as_secs();
    timer.set_timestamp((seconds >> 32) as u32, seconds as u32, 1_000_000_000);

    let error = receive_protocol_error(&mut fixture, id);
    assert_eq!(
        error.code,
        wp_commit_timer_v1::Error::InvalidTimestamp as u32
    );
    assert_eq!(error.object_interface, "wp_commit_timer_v1");
}

#[test]
fn duplicate_timer_reports_error_on_the_manager_object() {
    let (mut fixture, id, surface, _server_surface, _timer) = setup();
    let client = fixture.client(id);
    client
        .state
        .commit_timing_manager
        .as_ref()
        .unwrap()
        .get_timer(&surface, &client.qh, ());

    let error = receive_protocol_error(&mut fixture, id);
    assert_eq!(
        error.code,
        wp_commit_timing_manager_v1::Error::CommitTimerExists as u32
    );
    assert_eq!(error.object_interface, "wp_commit_timing_manager_v1");
}

#[test]
fn duplicate_timestamp_reports_error_on_the_timer_object() {
    let (mut fixture, id, _surface, _server_surface, timer) = setup();
    let now = get_monotonic_time();
    set_timestamp(&timer, now + Duration::from_millis(10));
    set_timestamp(&timer, now + Duration::from_millis(20));

    let error = receive_protocol_error(&mut fixture, id);
    assert_eq!(
        error.code,
        wp_commit_timer_v1::Error::TimestampExists as u32
    );
    assert_eq!(error.object_interface, "wp_commit_timer_v1");
}

#[test]
fn timer_can_be_recreated_after_destroy() {
    let (mut fixture, id, surface, server_surface, timer) = setup();
    timer.destroy();
    fixture.double_roundtrip(id);

    let client = fixture.client(id);
    let replacement = client
        .state
        .commit_timing_manager
        .as_ref()
        .unwrap()
        .get_timer(&surface, &client.qh, ());
    set_timestamp(
        &replacement,
        get_monotonic_time().saturating_sub(Duration::from_millis(1)),
    );
    surface.set_buffer_scale(2);
    surface.commit();
    fixture.double_roundtrip(id);
    dispatch_until(&mut fixture, Duration::from_secs(1), |_| {
        scale(&server_surface) == 2
    });
    assert_eq!(scale(&server_surface), 2);
}

#[test]
fn timestamp_after_surface_destroy_reports_the_timer_error() {
    let (mut fixture, id, surface, _server_surface, timer) = setup();
    surface.destroy();
    fixture.double_roundtrip(id);
    set_timestamp(&timer, get_monotonic_time());

    let error = receive_protocol_error(&mut fixture, id);
    assert_eq!(
        error.code,
        wp_commit_timer_v1::Error::SurfaceDestroyed as u32
    );
    assert_eq!(error.object_interface, "wp_commit_timer_v1");
}

#[test]
fn unsigned_seconds_cannot_wrap_into_an_expired_deadline() {
    let (mut fixture, id, surface, server_surface, timer) = setup();
    timer.set_timestamp(u32::MAX, u32::MAX, 0);
    surface.set_buffer_scale(2);
    surface.commit();
    fixture.double_roundtrip(id);

    assert_eq!(scale(&server_surface), 1);
    assert_eq!(fixture.realand().commit_timing.scheduled.len(), 1);

    surface.destroy();
    timer.destroy();
    fixture.double_roundtrip(id);
    assert!(fixture.realand().commit_timing.scheduled.is_empty());
}
