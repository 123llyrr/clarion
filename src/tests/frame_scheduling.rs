use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use smithay::reexports::calloop::timer::{TimeoutAction, Timer};

use super::Fixture;
use crate::backend::tty::{
    queue_kms_submission_retry, queue_retry_after_estimated_vblank, reset_kms_submission_backoff,
    take_redraw_state_on_vblank, Tty,
};
use crate::backend::RenderResult;
use crate::compositor::{PointerVisibility, RedrawState};
use crate::frame_clock::FrameClock;
use crate::utils::get_monotonic_time;

fn pending_empty_frame(
    estimated_delay: Duration,
) -> (
    Fixture,
    smithay::output::Output,
    smithay::reexports::calloop::RegistrationToken,
    Rc<Cell<usize>>,
    Duration,
) {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let output = f.clarion_output(1);
    let fired = Rc::new(Cell::new(0));
    let callback_fired = fired.clone();
    let callback_output = output.clone();
    let token = f
        .clarion()
        .event_loop
        .insert_source(Timer::from_duration(estimated_delay), move |_, _, state| {
            callback_fired.set(callback_fired.get() + 1);
            Tty::on_estimated_vblank_timer(
                &mut crate::platform::device_bridge::DeviceCoordinator::new(&mut state.clarion),
                callback_output.clone(),
            );
            TimeoutAction::Drop
        })
        .unwrap();
    let now = get_monotonic_time();
    let state = f.clarion().output_state.get_mut(&output).unwrap();
    state.schedule.test_enable_latch();
    state
        .schedule
        .test_set_clock(FrameClock::new(Some(Duration::from_millis(20)), false));
    state.schedule.presented(now, None);
    state
        .schedule
        .test_set_redraw(RedrawState::WaitingForEstimatedVBlank(token));
    (f, output, token, fired, now)
}

#[test]
fn empty_frame_commit_burst_is_coalesced_until_latch_and_keeps_its_callback_deadline() {
    let (mut f, output, estimated, fired, now) = pending_empty_frame(Duration::from_secs(60));
    f.clarion().queue_redraw(&output);
    assert!(f.clarion().defer_redraw_to_latch(&output, now));
    for _ in 0..100 {
        f.clarion().queue_redraw(&output);
        assert_eq!(f.clarion().next_queued_output(now), None);
    }
    assert_eq!(f.clarion().frame_timings()[0].kms_latch_deferrals, 1);

    let server = &mut f.state.server;
    server
        .event_loop
        .dispatch(Duration::from_millis(100), &mut server.state)
        .unwrap();
    assert!(
        matches!(server.state.clarion.output_state[&output].schedule.redraw_state(),
        RedrawState::WaitingForEstimatedVBlankAndQueued(token) if token == estimated)
    );
    assert!(server.state.clarion.output_state[&output]
        .schedule
        .latch_ready());
    assert_eq!(fired.get(), 0);
    // Even if this wake-up missed the original refresh, do not wait again.
    assert!(!f
        .clarion()
        .defer_redraw_to_latch(&output, now + Duration::from_millis(21)));
    f.clarion().event_loop.remove(estimated);
    Tty::on_estimated_vblank_timer(
        &mut crate::platform::device_bridge::DeviceCoordinator::new(f.clarion()),
        output.clone(),
    );
    assert!(matches!(
        f.clarion().output_state[&output].schedule.redraw_state(),
        RedrawState::Queued
    ));
    assert_eq!(
        f.clarion().output_state[&output]
            .schedule
            .callback_sequence(),
        1
    );
}

#[test]
fn estimated_deadline_before_latch_keeps_the_redraw_and_advances_callbacks() {
    let (mut f, output, estimated, _, now) = pending_empty_frame(Duration::from_secs(60));
    f.clarion().queue_redraw(&output);
    assert!(f.clarion().defer_redraw_to_latch(&output, now));
    let latch = match f.clarion().output_state[&output].schedule.redraw_state() {
        RedrawState::WaitingForLatch { token, .. } => token,
        _ => panic!("expected latch wait"),
    };
    // Deliver the old empty frame's deadline before the newer redraw's latch.
    f.clarion().event_loop.remove(estimated);
    Tty::on_estimated_vblank_timer(
        &mut crate::platform::device_bridge::DeviceCoordinator::new(f.clarion()),
        output.clone(),
    );
    assert!(
        matches!(f.clarion().output_state[&output].schedule.redraw_state(),
        RedrawState::WaitingForLatch { token, estimated_vblank: None } if token == latch)
    );
    assert_eq!(
        f.clarion().output_state[&output]
            .schedule
            .callback_sequence(),
        1
    );
    assert_eq!(f.clarion().next_queued_output(now), None);
    let server = &mut f.state.server;
    server
        .event_loop
        .dispatch(Duration::from_millis(100), &mut server.state)
        .unwrap();
    assert!(matches!(
        server.state.clarion.output_state[&output]
            .schedule
            .redraw_state(),
        RedrawState::Queued
    ));
    assert!(server.state.clarion.output_state[&output]
        .schedule
        .latch_ready());
}

#[test]
fn canceling_latch_preserves_the_empty_frames_original_timer() {
    let (mut f, output, estimated, fired, now) = pending_empty_frame(Duration::from_millis(25));
    f.clarion().queue_redraw(&output);
    assert!(f.clarion().defer_redraw_to_latch(&output, now));
    assert!(f.clarion().cancel_latch_wait(&output));
    assert!(
        matches!(f.clarion().output_state[&output].schedule.redraw_state(),
        RedrawState::WaitingForEstimatedVBlankAndQueued(token) if token == estimated)
    );
    let server = &mut f.state.server;
    server
        .event_loop
        .dispatch(Duration::from_millis(100), &mut server.state)
        .unwrap();
    assert!(!server.state.clarion.output_state[&output]
        .schedule
        .latch_ready());
    assert_eq!(fired.get(), 1, "original empty-frame timer must still fire");
    assert!(matches!(
        server.state.clarion.output_state[&output]
            .schedule
            .redraw_state(),
        RedrawState::Queued
    ));
    assert_eq!(
        server.state.clarion.output_state[&output]
            .schedule
            .callback_sequence(),
        1
    );
}

#[test]
fn removing_output_cancels_both_pending_timers() {
    let (mut f, output, _, fired, now) = pending_empty_frame(Duration::from_millis(10));
    f.clarion().queue_redraw(&output);
    assert!(f.clarion().defer_redraw_to_latch(&output, now));
    f.clarion().remove_output(&output);
    let server = &mut f.state.server;
    server
        .event_loop
        .dispatch(Duration::from_millis(40), &mut server.state)
        .unwrap();
    assert_eq!(fired.get(), 0);
    assert!(!server.state.clarion.output_state.contains_key(&output));
}

fn dispatch_server_for(f: &mut Fixture, duration: Duration) {
    let deadline = std::time::Instant::now() + duration;
    let server = &mut f.state.server;
    while let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) {
        server
            .event_loop
            .dispatch(remaining, &mut server.state)
            .unwrap();
    }
}

#[test]
fn removing_output_cancels_throttled_vblank_callback() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let output = f.clarion_output(1);
    let fired = Rc::new(Cell::new(0));
    let callback_fired = fired.clone();
    let now = get_monotonic_time();
    let refresh = Some(Duration::from_millis(40));
    let throttle = &mut f
        .clarion()
        .output_state
        .get_mut(&output)
        .unwrap()
        .vblank_throttle;
    assert!(!throttle.throttle(refresh, now, |_| {}));
    assert!(throttle.throttle(refresh, now, move |_| {
        callback_fired.set(callback_fired.get() + 1);
    }));

    f.clarion().remove_output(&output);
    dispatch_server_for(&mut f, Duration::from_millis(60));
    assert_eq!(
        fired.get(),
        0,
        "removed output must not receive a stale VBlank"
    );
}

#[test]
fn early_vblank_bursts_do_not_postpone_the_pending_completion() {
    for hz in [60, 144] {
        let mut f = Fixture::new();
        f.add_output(1, (1920, 1080));
        let output = f.clarion_output(1);
        let completed = Rc::new(Cell::new(0));
        let duplicates = Rc::new(Cell::new(0));
        let refresh = Duration::from_nanos(1_000_000_000 / hz);
        let now = get_monotonic_time();
        let throttle = &mut f
            .clarion()
            .output_state
            .get_mut(&output)
            .unwrap()
            .vblank_throttle;
        assert!(!throttle.throttle(Some(refresh), now, |_| {}));

        let completion = completed.clone();
        assert!(throttle.throttle(Some(refresh), now, move |_| {
            completion.set(completion.get() + 1);
        }));
        // No new KMS frame can be submitted until this pending completion is
        // delivered. Rogue/duplicate driver events must not keep pushing it back.
        for _ in 0..32 {
            let duplicates = duplicates.clone();
            assert!(throttle.throttle(Some(refresh), now, move |_| {
                duplicates.set(duplicates.get() + 1);
            }));
        }

        dispatch_server_for(&mut f, refresh * 3);
        assert_eq!(completed.get(), 1, "{hz} Hz completion was starved");
        assert_eq!(duplicates.get(), 0, "duplicate frame completion delivered");
        let throttle = &mut f
            .clarion()
            .output_state
            .get_mut(&output)
            .unwrap()
            .vblank_throttle;
        assert!(!throttle.throttle(Some(refresh), get_monotonic_time(), |_| {}));
    }
}

#[test]
fn removing_output_inside_throttled_vblank_callback_is_safe() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let output = f.clarion_output(1);
    let callback_output = output.clone();
    let fired = Rc::new(Cell::new(0));
    let callback_fired = fired.clone();
    let now = get_monotonic_time();
    let refresh = Some(Duration::from_millis(40));
    let throttle = &mut f
        .clarion()
        .output_state
        .get_mut(&output)
        .unwrap()
        .vblank_throttle;
    assert!(!throttle.throttle(refresh, now, |_| {}));
    assert!(throttle.throttle(refresh, now, move |state| {
        callback_fired.set(callback_fired.get() + 1);
        // Drop may remove the currently executing calloop source. It must not
        // panic or leave a second invocation pending after this callback returns.
        state.clarion.remove_output(&callback_output);
    }));

    dispatch_server_for(&mut f, Duration::from_millis(60));
    assert_eq!(fired.get(), 1);
    assert!(!f.clarion().output_state.contains_key(&output));
}

#[test]
fn hardware_vblank_cancels_estimated_callback_and_preserves_redraw() {
    for already_queued in [false, true] {
        let (mut f, output, _, fired, _) = pending_empty_frame(Duration::ZERO);
        if already_queued {
            f.clarion().queue_redraw(&output);
        }
        let clarion = f.clarion();
        let redraw = clarion
            .output_state
            .get_mut(&output)
            .unwrap()
            .schedule
            .hardware_vblank(&clarion.event_loop, &output.name());
        assert!(redraw);
        clarion.queue_redraw(&output);
        assert_eq!(
            clarion.next_queued_output(get_monotonic_time()),
            Some(output.clone())
        );

        // The old estimate is already due. A hardware event must take ownership
        // so its callback cannot run in the now-Queued state and panic.
        dispatch_server_for(&mut f, Duration::from_millis(1));
        assert_eq!(fired.get(), 0);
        let state = f.clarion_state();
        state.clarion.redraw_queued_outputs(&mut state.backend);
        assert_eq!(state.clarion.frame_timings()[0].submitted, 1);
        assert!(matches!(
            state.clarion.output_state[&output].schedule.redraw_state(),
            RedrawState::Idle
        ));
    }
}

#[test]
fn hardware_vblank_preserves_normal_submission_redraw_decision() {
    let mut f = Fixture::new();
    let event_loop = f.clarion().event_loop.clone();
    for redraw_needed in [false, true] {
        let mut state = RedrawState::WaitingForVBlank { redraw_needed };
        assert_eq!(
            take_redraw_state_on_vblank(&mut state, &event_loop, "test"),
            redraw_needed
        );
        assert!(matches!(state, RedrawState::Idle));
    }
}

#[test]
fn mixed_refresh_outputs_are_ordered_by_deadline_not_insertion_order() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    f.add_output(2, (1920, 1080));
    let slow = f.clarion_output(1);
    let fast = f.clarion_output(2);
    let last = Duration::from_secs(1);
    for (output, interval) in [(&slow, 16), (&fast, 7)] {
        let state = f.clarion().output_state.get_mut(output).unwrap();
        state.schedule.test_set_clock(FrameClock::new(
            Some(Duration::from_millis(interval)),
            false,
        ));
        state.schedule.presented(last, None);
        state.schedule.test_set_redraw(RedrawState::Queued);
    }
    assert_eq!(
        f.clarion()
            .next_queued_output(last + Duration::from_millis(1)),
        Some(fast.clone())
    );
    // A faster output can have a later deadline: frequency alone is not sufficient.
    assert_eq!(
        f.clarion()
            .next_queued_output(last + Duration::from_millis(15)),
        Some(slow.clone())
    );
    f.clarion()
        .output_state
        .get_mut(&fast)
        .unwrap()
        .schedule
        .test_set_redraw(RedrawState::WaitingForVBlank {
            redraw_needed: true,
        });
    assert_eq!(f.clarion().next_queued_output(last), Some(slow.clone()));
    f.clarion()
        .output_state
        .get_mut(&slow)
        .unwrap()
        .schedule
        .test_set_redraw(RedrawState::Idle);
    assert_eq!(f.clarion().next_queued_output(last), None);
}

#[test]
fn latch_to_redraw_delay_is_reserved_for_the_next_frame() {
    for hz in [60, 144] {
        let mut f = Fixture::new();
        f.add_output(1, (1920, 1080));
        f.clarion_complete_animations();
        let output = f.clarion_output(1);
        let refresh = Duration::from_nanos(1_000_000_000 / hz);
        let now = get_monotonic_time();
        let delay = refresh / 2;
        {
            let state = f.clarion().output_state.get_mut(&output).unwrap();
            state.schedule.test_enable_latch();
            state.schedule.test_latch_ready();
            state
                .schedule
                .test_set_clock(FrameClock::new(Some(refresh), false));
            state.schedule.presented(now - refresh, None);
            state.schedule.test_set_redraw(RedrawState::Queued);
            // Split a late timer and the dispatch work between that callback and
            // rendering. The renderer's CPU duration measures neither delay.
            state.schedule.test_latch_woke(now - delay, now - delay / 2);
        }
        let state = f.clarion_state();
        state.clarion.redraw_queued_outputs(&mut state.backend);
        assert!(
            state.clarion.output_state[&output]
                .schedule
                .clock()
                .latch_budget()
                .unwrap()
                >= delay,
            "{hz} Hz scheduler forgot the late latch before the next frame"
        );
    }
}

#[test]
fn delayed_vblank_callback_is_not_throttled_again() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let output = f.clarion_output(1);
    let refresh = Duration::from_millis(30);
    let last = get_monotonic_time();
    let throttle = &mut f
        .clarion()
        .output_state
        .get_mut(&output)
        .unwrap()
        .vblank_throttle;
    assert!(!throttle.throttle(Some(refresh), last, |_| {}));

    let called = Rc::new(Cell::new(0));
    let callback_called = Rc::clone(&called);
    let callback_output = output.clone();
    assert!(throttle.throttle(
        Some(refresh),
        last + Duration::from_millis(1),
        move |state| {
            let throttle = &mut state
                .clarion
                .output_state
                .get_mut(&callback_output)
                .unwrap()
                .vblank_throttle;
            assert!(!throttle.throttle(Some(refresh), get_monotonic_time(), |_| {}));
            callback_called.set(callback_called.get() + 1);
        }
    ));

    let server = &mut f.state.server;
    server
        .event_loop
        .dispatch(Duration::from_millis(100), &mut server.state)
        .unwrap();
    assert_eq!(called.get(), 1);
}

#[test]
fn querying_frame_timings_is_read_only_and_keeps_outputs_separate() {
    let mut f = Fixture::new();
    f.add_output(2, (1920, 1080));
    f.add_output(1, (1920, 1080));
    let output = f.clarion_output(1);
    for state in f.clarion().output_state.values_mut() {
        state.schedule.test_set_redraw(RedrawState::Idle);
    }
    f.clarion()
        .output_state
        .get_mut(&output)
        .unwrap()
        .schedule
        .test_record_render(Duration::from_millis(4), &RenderResult::Submitted);
    let first = f.clarion().frame_timings();
    assert_eq!(first, f.clarion().frame_timings());
    assert_eq!(
        first.iter().map(|t| t.output.as_str()).collect::<Vec<_>>(),
        vec!["headless-1", "headless-2"]
    );
    assert_eq!(first.iter().map(|t| t.submitted).sum::<u64>(), 1);
    assert_eq!(
        first.iter().filter(|t| t.render_time_ms.is_some()).count(),
        1
    );
    assert!(f
        .clarion()
        .output_state
        .values()
        .all(|s| matches!(s.schedule.redraw_state(), RedrawState::Idle)));
}

#[test]
fn kms_latch_timer_defers_and_coalesces_queued_redraws() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let output = f.clarion_output(1);
    let now = get_monotonic_time();
    let last = now - Duration::from_millis(1);
    {
        let state = f.clarion().output_state.get_mut(&output).unwrap();
        state.schedule.test_enable_latch();
        state
            .schedule
            .test_set_clock(FrameClock::new(Some(Duration::from_millis(10)), false));
        state.schedule.presented(last, None);
        state.schedule.test_set_redraw(RedrawState::Queued);
    }

    assert!(f.clarion().defer_redraw_to_latch(&output, now));
    assert!(matches!(
        f.clarion().output_state[&output].schedule.redraw_state(),
        RedrawState::WaitingForLatch { .. }
    ));
    assert_eq!(f.clarion().next_queued_output(now), None);

    // Additional damage is coalesced into the already pending latch wake-up.
    f.clarion().queue_redraw(&output);
    assert!(matches!(
        f.clarion().output_state[&output].schedule.redraw_state(),
        RedrawState::WaitingForLatch { .. }
    ));

    let server = &mut f.state.server;
    server
        .event_loop
        .dispatch(Duration::from_millis(30), &mut server.state)
        .unwrap();
    assert!(matches!(
        server.state.clarion.output_state[&output]
            .schedule
            .redraw_state(),
        RedrawState::Queued
    ));
    let timing = server.state.clarion.frame_timings().remove(0);
    assert_eq!(timing.kms_latch_deferrals, 1);
    assert!(timing.kms_latch_delay_ms.is_some());
    assert_eq!(timing.kms_latch_wakeup_lateness_ms.unwrap().samples, 1);
    assert!(timing.kms_latch_to_render_delay_ms.is_none());

    // Simulate dispatch work delaying this frame past its original vblank. It
    // must render now instead of sleeping until yet another latch deadline.
    let late_now = last + Duration::from_millis(11);
    assert!(!f.clarion().defer_redraw_to_latch(&output, late_now));
    f.clarion().queue_redraw(&output);
    assert!(!f.clarion().defer_redraw_to_latch(&output, late_now));
    assert_eq!(f.clarion().frame_timings()[0].kms_latch_deferrals, 1);

    // A render attempt consumes the ready state, including a skipped frame on
    // this renderer-less fixture. Later frames can use the latch timer again.
    let state = f.clarion_state();
    state.clarion.redraw_queued_outputs(&mut state.backend);
    assert!(!state.clarion.output_state[&output].schedule.latch_ready());
    assert_eq!(
        state.clarion.frame_timings()[0]
            .kms_latch_to_render_delay_ms
            .as_ref()
            .unwrap()
            .samples,
        1
    );
    state.clarion.queue_redraw(&output);
    assert!(state.clarion.defer_redraw_to_latch(&output, late_now));
}

#[test]
fn timing_change_invalidates_an_already_fired_latch() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let output = f.clarion_output(1);
    let now = get_monotonic_time();
    {
        let state = f.clarion().output_state.get_mut(&output).unwrap();
        state.schedule.test_enable_latch();
        state.schedule.test_latch_ready();
        state.schedule.test_set_redraw(RedrawState::Queued);
        state
            .schedule
            .test_set_clock(FrameClock::new(Some(Duration::from_millis(30)), false));
        state.schedule.presented(now, None);
    }
    f.clarion().cancel_latch_wait(&output);
    assert!(!f.clarion().output_state[&output].schedule.latch_ready());
    assert!(f.clarion().defer_redraw_to_latch(&output, now));
}

#[test]
fn timing_change_cancels_stale_latch_wait_without_losing_damage() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let output = f.clarion_output(1);
    let last = Duration::from_secs(1);
    let now = last + Duration::from_millis(1);
    {
        let state = f.clarion().output_state.get_mut(&output).unwrap();
        state.schedule.test_enable_latch();
        state
            .schedule
            .test_set_clock(FrameClock::new(Some(Duration::from_millis(30)), false));
        state.schedule.presented(last, None);
        state.schedule.test_set_redraw(RedrawState::Queued);
    }

    assert!(f.clarion().defer_redraw_to_latch(&output, now));
    assert!(f.clarion().cancel_latch_wait(&output));
    assert!(matches!(
        f.clarion().output_state[&output].schedule.redraw_state(),
        RedrawState::Queued
    ));
    assert!(!f.clarion().cancel_latch_wait(&output));

    // Prove that the removed timer cannot wake the output again later.
    f.clarion()
        .output_state
        .get_mut(&output)
        .unwrap()
        .schedule
        .test_set_redraw(RedrawState::Idle);
    let server = &mut f.state.server;
    server
        .event_loop
        .dispatch(Duration::from_millis(40), &mut server.state)
        .unwrap();
    assert!(matches!(
        server.state.clarion.output_state[&output]
            .schedule
            .redraw_state(),
        RedrawState::Idle
    ));
}

#[test]
fn failed_final_frame_retries_after_skipped_redraw_cleanup_without_more_input() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    f.clarion_complete_animations();
    let output = f.clarion_output(1);
    assert!(!f.clarion().animations_need_redraw(&output));

    // Both renderer fence-failure paths use this scheduler. The final frame has
    // no animation left to request a retry on its behalf.
    queue_retry_after_estimated_vblank(
        f.clarion(),
        output.clone(),
        get_monotonic_time() + Duration::from_secs(60),
    );
    assert!(!f.clarion().output_state[&output]
        .schedule
        .animations_remain());
    assert_eq!(f.clarion().next_queued_output(get_monotonic_time()), None);

    // Exercise the actual Skipped cleanup without requiring a GPU fence. An
    // update queued before the deadline cannot consume the pending retry.
    f.clarion().queue_redraw(&output);
    let state = f.clarion_state();
    state.clarion.monitors_active = false;
    state.clarion.redraw_queued_outputs(&mut state.backend);
    state.clarion.monitors_active = true;
    let estimated = match state.clarion.output_state[&output].schedule.redraw_state() {
        RedrawState::WaitingForEstimatedVBlank(token) => token,
        _ => panic!("a skipped frame must retain its estimated deadline"),
    };
    assert!(state.clarion.output_state[&output].schedule.retry_pending());
    assert_eq!(state.clarion.next_queued_output(get_monotonic_time()), None);

    state.clarion.event_loop.remove(estimated);
    Tty::on_estimated_vblank_timer(
        &mut crate::platform::device_bridge::DeviceCoordinator::new(&mut state.clarion),
        output.clone(),
    );
    assert_eq!(
        state.clarion.next_queued_output(get_monotonic_time()),
        Some(output.clone())
    );
    assert!(!state.clarion.output_state[&output].schedule.retry_pending());

    // The retry now renders successfully and settles, without another input,
    // client commit or ongoing animation scheduling the final frame again.
    state.clarion.redraw_queued_outputs(&mut state.backend);
    assert_eq!(state.clarion.frame_timings()[0].submitted, 1);
    assert!(matches!(
        state.clarion.output_state[&output].schedule.redraw_state(),
        RedrawState::Idle
    ));
}

#[test]
fn kms_failure_backoff_coalesces_commits_without_blocking_other_outputs() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    f.add_output(2, (1920, 1080));
    f.clarion_complete_animations();
    let failing = f.clarion_output(1);
    let healthy = f.clarion_output(2);
    let deadline = get_monotonic_time() + Duration::from_secs(60);
    for _ in 0..2 {
        queue_kms_submission_retry(
            f.clarion(),
            failing.clone(),
            deadline,
            &"injected KMS denial",
        );
    }
    let timer = match f.clarion().output_state[&failing].schedule.redraw_state() {
        RedrawState::WaitingForEstimatedVBlank(token) => token,
        _ => panic!("expected a scheduled retry"),
    };
    for _ in 0..100 {
        f.clarion().queue_redraw(&failing);
        let state = f.clarion_state();
        state.clarion.redraw_queued_outputs(&mut state.backend);
        assert!(
            matches!(state.clarion.output_state[&failing].schedule.redraw_state(),
            RedrawState::WaitingForEstimatedVBlank(token) if token == timer)
        );
    }
    let snapshots = f.clarion().frame_timings();
    assert_eq!(
        snapshots
            .iter()
            .find(|s| s.output == failing.name())
            .unwrap()
            .redraws,
        0
    );
    assert_eq!(
        snapshots
            .iter()
            .find(|s| s.output == healthy.name())
            .unwrap()
            .submitted,
        1
    );

    // Session activation permits an immediate redraw, even before the retry deadline.
    reset_kms_submission_backoff(f.clarion());
    f.clarion().queue_redraw_all();
    assert!(!f
        .clarion()
        .defer_kms_submission_retry(&failing, get_monotonic_time()));
    // The headless test backend only accepts Queued. Consume the old callback before
    // using it to simulate success; physical KMS removes this timer on submission.
    f.clarion().event_loop.remove(timer);
    Tty::on_estimated_vblank_timer(
        &mut crate::platform::device_bridge::DeviceCoordinator::new(f.clarion()),
        failing.clone(),
    );
    let state = f.clarion_state();
    state.clarion.redraw_queued_outputs(&mut state.backend);
    assert!(matches!(
        state.clarion.output_state[&failing].schedule.redraw_state(),
        RedrawState::Idle
    ));
    assert!(!state.clarion.output_state[&failing]
        .schedule
        .retry_pending());
    assert!(state.clarion.output_state[&failing]
        .schedule
        .retry_deadline(get_monotonic_time())
        .is_none());
}

#[test]
fn kms_retry_keeps_older_callback_deadline_and_recovers_without_new_input() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    f.clarion_complete_animations();
    let output = f.clarion_output(1);
    let now = get_monotonic_time();
    queue_kms_submission_retry(f.clarion(), output.clone(), now, &"first KMS denial");
    let older_timer = match f.clarion().output_state[&output].schedule.redraw_state() {
        RedrawState::WaitingForEstimatedVBlank(token) => token,
        _ => panic!("expected an estimated callback"),
    };
    queue_kms_submission_retry(f.clarion(), output.clone(), now, &"repeated KMS denial");
    assert!(
        matches!(f.clarion().output_state[&output].schedule.redraw_state(),
        RedrawState::WaitingForEstimatedVBlank(token) if token == older_timer)
    );

    // Deliver the older callback before the backoff deadline. It must still advance
    // callbacks, but it cannot cause GPU rendering before the new retry is due.
    f.clarion().event_loop.remove(older_timer);
    Tty::on_estimated_vblank_timer(
        &mut crate::platform::device_bridge::DeviceCoordinator::new(f.clarion()),
        output.clone(),
    );
    assert_eq!(
        f.clarion().output_state[&output]
            .schedule
            .callback_sequence(),
        1
    );
    let state = f.clarion_state();
    state.clarion.redraw_queued_outputs(&mut state.backend);
    assert_eq!(state.clarion.frame_timings()[0].redraws, 0);
    assert!(
        matches!(state.clarion.output_state[&output].schedule.redraw_state(),
        RedrawState::WaitingForEstimatedVBlank(token) if token != older_timer)
    );

    // Only the retry timer supplies work from here: no input, commit, or animation.
    let server = &mut f.state.server;
    let timeout = std::time::Instant::now() + Duration::from_secs(2);
    while !matches!(
        server.state.clarion.output_state[&output]
            .schedule
            .redraw_state(),
        RedrawState::Queued
    ) {
        assert!(
            std::time::Instant::now() < timeout,
            "retry never became ready"
        );
        server
            .event_loop
            .dispatch(Duration::from_millis(100), &mut server.state)
            .unwrap();
    }
    server
        .state
        .clarion
        .redraw_queued_outputs(&mut server.state.backend);
    assert_eq!(server.state.clarion.frame_timings()[0].submitted, 1);
    assert!(!server.state.clarion.output_state[&output]
        .schedule
        .retry_pending());

    let now = get_monotonic_time();
    let target = now + Duration::from_millis(3);
    // Successful presentation attempts end the failure episode and its warning throttle.
    assert_eq!(
        server
            .state
            .clarion
            .output_state
            .get_mut(&output)
            .unwrap()
            .schedule
            .kms_submission_failed(now, target),
        (target, true)
    );
}

#[test]
fn failed_kms_frame_retries_when_estimated_vblank_fires() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let output = f.clarion_output(1);
    let token = f
        .clarion()
        .event_loop
        .insert_source(Timer::from_duration(Duration::from_secs(60)), |_, _, _| {
            TimeoutAction::Drop
        })
        .unwrap();
    {
        let state = f.clarion().output_state.get_mut(&output).unwrap();
        state
            .schedule
            .test_set_redraw(RedrawState::WaitingForEstimatedVBlank(token));
        state.schedule.retry_after_estimated_vblank();
    }

    Tty::on_estimated_vblank_timer(
        &mut crate::platform::device_bridge::DeviceCoordinator::new(f.clarion()),
        output.clone(),
    );

    let state = &f.clarion().output_state[&output];
    assert!(matches!(state.schedule.redraw_state(), RedrawState::Queued));
    assert!(!state.schedule.retry_pending());
}

#[test]
fn idle_config_notification_finishes_without_input() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    f.clarion_complete_animations();
    let output = f.clarion_output(1);
    let clarion = f.clarion();
    clarion.last_user_activity = std::time::Instant::now() - Duration::from_secs(31);
    assert!(!clarion.animations_need_redraw(&output));

    // A failed hot reload can show a notification while the user remains idle.
    clarion.config_error_notification.show();
    assert!(clarion.animations_need_redraw(&output));
    clarion.clock.set_complete_instantly(true);
    clarion.config_error_notification.advance_animations();
    // The visible timeout must keep advancing, too.
    assert!(clarion.animations_need_redraw(&output));
    let deadline = clarion.clock.now_unadjusted() + Duration::from_secs(5);
    clarion.clock.set_unadjusted(deadline);
    clarion.config_error_notification.advance_animations();
    assert!(clarion.animations_need_redraw(&output));
    clarion.config_error_notification.advance_animations();
    assert!(!clarion.animations_need_redraw(&output));
}

fn animated_cursor_fixture() -> Fixture {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    f.add_output(2, (1920, 1080));
    f.clarion_complete_animations();
    // The later frame extends left of the hotspot. Scheduling based only on
    // the current frame or pointer position would miss a neighbouring output.
    for scale in [1, 2] {
        let frames = [0, 12].map(|xhot| xcursor::parser::Image {
            size: 32 * scale,
            width: 32 * scale,
            height: 32 * scale,
            xhot: xhot * scale,
            yhot: 0,
            delay: 40,
            pixels_rgba: vec![255; (32 * 32 * scale * scale * 4) as usize],
            pixels_argb: vec![],
        });
        f.clarion()
            .cursor_manager
            .set_named_cursor_for_test(scale as i32, Vec::from(frames));
    }
    f.clarion_state().move_cursor((100., 100.).into());
    f
}

#[test]
fn animated_cursor_only_drives_visible_intersecting_outputs() {
    let mut f = animated_cursor_fixture();
    let first = f.clarion_output(1);
    let second = f.clarion_output(2);
    for visibility in [PointerVisibility::Hidden, PointerVisibility::Disabled] {
        f.clarion().pointer_visibility = visibility;
        assert!(!f.clarion().animations_need_redraw(&first));
        assert!(!f.clarion().animations_need_redraw(&second));
    }
    f.clarion().pointer_visibility = PointerVisibility::Visible;
    assert!(f.clarion().animations_need_redraw(&first));
    assert!(!f.clarion().animations_need_redraw(&second));

    let origin = f
        .clarion()
        .global_space
        .output_geometry(&second)
        .unwrap()
        .loc;
    f.clarion_state()
        .move_cursor((origin.x as f64 + 8., origin.y as f64 + 100.).into());
    assert!(
        f.clarion().animations_need_redraw(&first),
        "later cursor frame crosses the edge"
    );
    assert!(f.clarion().animations_need_redraw(&second));
    f.clarion_state()
        .move_cursor((origin.x as f64 + 100., origin.y as f64 + 100.).into());
    assert!(!f.clarion().animations_need_redraw(&first));
    assert!(f.clarion().animations_need_redraw(&second));
}

#[test]
fn animated_cursor_uses_tablet_position_and_fractional_output_bounds() {
    let mut f = animated_cursor_fixture();
    let first = f.clarion_output(1);
    let second = f.clarion_output(2);
    let origin = f
        .clarion()
        .global_space
        .output_geometry(&second)
        .unwrap()
        .loc;
    f.clarion().tablet_cursor_location =
        Some((origin.x as f64 + 100., origin.y as f64 + 100.).into());
    assert!(!f.clarion().animations_need_redraw(&first));
    assert!(f.clarion().animations_need_redraw(&second));
    f.clarion().tablet_cursor_location = None;
    assert!(f.clarion().animations_need_redraw(&first));
    assert!(!f.clarion().animations_need_redraw(&second));

    // A non-integral output scale must preserve a cursor frame whose pixels
    // overlap its edge, even when the hotspot is on the neighbouring screen.
    first.change_current_state(
        None,
        None,
        Some(smithay::output::Scale::Fractional(1.25)),
        None,
    );
    let edge = 1920. / 1.25;
    f.clarion().tablet_cursor_location = Some((edge + 8., 100.).into());
    assert!(f.clarion().animations_need_redraw(&first));
    f.clarion().tablet_cursor_location = Some((edge + 40., 100.).into());
    assert!(!f.clarion().animations_need_redraw(&first));
}

#[test]
fn hidden_animated_cursor_stops_estimated_vblank_redraw_loop() {
    let mut f = animated_cursor_fixture();
    let output = f.clarion_output(1);
    for visibility in [
        PointerVisibility::Visible,
        PointerVisibility::Hidden,
        PointerVisibility::Disabled,
    ] {
        f.clarion().pointer_visibility = visibility;
        f.clarion().queue_redraw(&output);
        let state = f.clarion_state();
        state.clarion.redraw_queued_outputs(&mut state.backend);
        assert_eq!(
            state.clarion.output_state[&output]
                .schedule
                .animations_remain(),
            visibility.is_visible()
        );
        let token = state
            .clarion
            .event_loop
            .insert_source(Timer::from_duration(Duration::from_secs(60)), |_, _, _| {
                TimeoutAction::Drop
            })
            .unwrap();
        let output_state = state.clarion.output_state.get_mut(&output).unwrap();
        output_state
            .schedule
            .test_set_redraw(RedrawState::WaitingForEstimatedVBlank(token));
        let sequence = output_state.schedule.callback_sequence();
        Tty::on_estimated_vblank_timer(
            &mut crate::platform::device_bridge::DeviceCoordinator::new(&mut state.clarion),
            output.clone(),
        );
        let output_state = &state.clarion.output_state[&output];
        assert_eq!(
            matches!(output_state.schedule.redraw_state(), RedrawState::Queued),
            visibility.is_visible()
        );
        assert_eq!(
            output_state.schedule.callback_sequence(),
            sequence.wrapping_add(1)
        );
        state.clarion.event_loop.remove(token);
    }
}

#[test]
fn scene_effect_failure_wakes_one_owner_frame_after_static_render_without_input() {
    use crate::scene::window_resources::EffectRetirements;
    let mut fixture = Fixture::new();
    fixture.add_output(1, (1920, 1080));
    fixture.add_output(2, (1280, 720));
    let output = fixture.clarion_output(1);
    let registration = fixture
        .clarion()
        .scene_resources
        .allocator()
        .windows
        .allocate();
    let lease = registration.lease();
    let state = fixture.clarion().output_state.get_mut(&output).unwrap();
    state.schedule.test_set_redraw(RedrawState::Idle);
    state.schedule.set_animations_remain(false);
    let second = fixture.clarion_output(2);
    fixture
        .clarion()
        .output_state
        .get_mut(&second)
        .unwrap()
        .schedule
        .test_set_redraw(RedrawState::Idle);
    lease.request_effect_retirement(EffectRetirements {
        wobble: true,
        genie: false,
    });
    // Rendering happened after the false animation query. Cleanup must not
    // depend on a pointer event or an already-running numeric animation.
    fixture.clarion().finish_scene_effects();
    assert!(matches!(
        fixture.clarion().output_state[&output]
            .schedule
            .redraw_state(),
        RedrawState::Queued
    ));
    assert!(matches!(
        fixture.clarion().output_state[&second]
            .schedule
            .redraw_state(),
        RedrawState::Queued
    ));
    assert!(lease.take_effect_retirements().wobble);
    fixture
        .clarion()
        .output_state
        .get_mut(&output)
        .unwrap()
        .schedule
        .test_set_redraw(RedrawState::Idle);
    fixture.clarion().finish_scene_effects();
    assert!(matches!(
        fixture.clarion().output_state[&output]
            .schedule
            .redraw_state(),
        RedrawState::Idle
    ));
}
