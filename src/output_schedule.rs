//! Event-loop adapter for the independent Clarion per-output scheduler.
//! State transitions and frame prediction live in clarion-core. This adapter owns
//! only platform timer removal, diagnostics, and conversion of backend outcomes.

#[cfg(test)]
use std::mem;
use std::time::Duration;

pub use clarion_timing::scheduling::EstimatedVblankAction;
use clarion_timing::scheduling::{OutputScheduler, RenderOutcome, VblankTransition};
use smithay::reexports::calloop::{LoopHandle, RegistrationToken};

use crate::backend::RenderResult;
use crate::frame_clock::FrameClock;
use crate::frame_stats::FrameStats;
use crate::render_helpers::vulkan::timing::{GpuTimingSample, GpuTimingTag};
pub type RedrawState = clarion_timing::scheduling::RedrawState<RegistrationToken>;

/// Thread CPU time excludes blocking and scheduler preemption. Called only
/// after fresh hardware feedback is accepted by FrameStats.
fn main_thread_cpu_time() -> Option<Duration> {
    let mut value = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: clock_gettime writes a valid, owned timespec when it succeeds.
    if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut value) } != 0
        || value.tv_sec < 0
        || !(0..1_000_000_000).contains(&value.tv_nsec)
    {
        return None;
    }
    Some(Duration::new(value.tv_sec as u64, value.tv_nsec as u32))
}

pub(crate) struct OutputSchedule {
    core: OutputScheduler<RegistrationToken>,
    stats: FrameStats,
    continue_after_presentation: bool,
}

impl OutputSchedule {
    pub(crate) fn new(refresh_interval: Option<Duration>, vrr: bool, latch_enabled: bool) -> Self {
        Self {
            core: OutputScheduler::new(refresh_interval, vrr, latch_enabled),
            stats: FrameStats::default(),
            continue_after_presentation: false,
        }
    }
    pub(crate) fn clock(&self) -> &FrameClock {
        self.core.clock()
    }
    pub(crate) fn reset_timing(&mut self, interval: Option<Duration>, vrr: bool) {
        self.core.reset_timing(interval, vrr);
        self.stats.reset_presentation_history();
        self.continue_after_presentation = false;
    }
    pub(crate) fn set_vrr(&mut self, enabled: bool) {
        if self.core.clock().vrr() != enabled {
            self.stats.reset_presentation_history();
            self.continue_after_presentation = false;
        }
        self.core.set_vrr(enabled);
    }
    pub(crate) fn begin_redraw(&mut self, now: Duration) -> Duration {
        self.core.begin_redraw(now, self.stats.redraw_started(now))
    }
    pub(crate) fn finish_redraw(&mut self, elapsed: Duration, result: &RenderResult) {
        self.stats.rendered(elapsed, result);
        let outcome = match result {
            RenderResult::Submitted => RenderOutcome::Submitted,
            RenderResult::NoDamage => RenderOutcome::NoDamage,
            RenderResult::Skipped | RenderResult::GpuDeferred => RenderOutcome::Skipped,
        };
        if *result == RenderResult::Skipped
            || (outcome == RenderOutcome::NoDamage && !self.core.animations_remain())
        {
            self.stats.interrupt_presentation_continuity();
            self.continue_after_presentation = false;
        }
        self.core.finish_redraw(elapsed, &outcome);
    }
    pub(crate) fn arm_latch(&mut self, token: RegistrationToken, delay: Duration) {
        self.core.arm_latch(token);
        self.stats.latch_deferred(delay);
    }
    pub(crate) fn latch_fired(&mut self, deadline: Duration, actual: Duration) {
        if matches!(
            self.core.redraw_state(),
            RedrawState::WaitingForLatch { .. }
        ) {
            self.stats.latch_woke(deadline, actual);
        }
        self.core.latch_fired();
    }
    pub(crate) fn cancel_latch(&mut self) -> Option<RegistrationToken> {
        self.stats.latch_cancelled();
        self.core.cancel_latch()
    }
    pub(crate) fn hardware_vblank<D>(
        &mut self,
        event_loop: &LoopHandle<'_, D>,
        name: &str,
    ) -> bool {
        let transition = self.core.hardware_vblank();
        self.continue_after_presentation = transition.unexpected_state.is_none()
            && (transition.redraw_needed || self.core.animations_remain());
        if transition.unexpected_state.is_some() {
            self.stats.interrupt_presentation_continuity();
        }
        apply_vblank_transition(transition, event_loop, name)
    }
    pub(crate) fn record_presentation_feedback(&mut self, actual: Duration, target: Duration) {
        self.stats.presented_with_cpu_sampler(
            actual,
            target,
            self.core.clock(),
            self.continue_after_presentation,
            main_thread_cpu_time,
        );
    }
    pub(crate) fn presented(&mut self, actual: Duration, target: Option<Duration>) {
        // Some backends have a physical timestamp without a submitted feedback object.
        // The sequence ignores the duplicate when record_presentation_feedback already ran.
        self.stats.presented_with_cpu_sampler(
            actual,
            target.unwrap_or_default(),
            self.core.clock(),
            self.continue_after_presentation,
            main_thread_cpu_time,
        );
        self.core.presented(actual, target);
    }
    pub(crate) fn freeze_timings(&self, name: String) -> crate::frame_stats::FrozenFrameStats {
        self.stats
            .freeze(name, self.core.clock(), self.core.latch_enabled())
    }
    #[cfg(test)]
    pub(crate) fn timings(&self, name: String) -> clarion_ipc::diagnostics::OutputFrameTimings {
        self.stats
            .snapshot(name, self.core.clock(), self.core.latch_enabled())
    }
    pub(crate) fn gpu_timing_tag(&self) -> Option<GpuTimingTag> {
        let (output_generation, redraw_sequence) = self.stats.gpu_timing_identity()?;
        Some(GpuTimingTag {
            output_generation,
            redraw_sequence,
        })
    }
    pub(crate) fn record_gpu_timing(&mut self, sample: GpuTimingSample) -> bool {
        self.stats.record_gpu_timing(
            sample.tag.output_generation,
            sample.tag.redraw_sequence,
            sample.gpu_time,
        )
    }
    pub(crate) fn cancel_timers<D>(&mut self, event_loop: &LoopHandle<'_, D>) {
        self.stats.latch_cancelled();
        self.stats.reset_presentation_history();
        self.continue_after_presentation = false;
        for token in self.core.take_timers().into_iter().flatten() {
            event_loop.remove(token);
        }
    }
    pub(crate) fn queue_redraw(&mut self) {
        self.core.queue_redraw()
    }
    pub(crate) fn is_queued(&self) -> bool {
        self.core.is_queued()
    }
    pub(crate) fn set_animations_remain(&mut self, remain: bool) {
        self.core.set_animations_remain(remain)
    }
    pub(crate) fn animations_remain(&self) -> bool {
        self.core.animations_remain()
    }
    pub(crate) fn callback_sequence(&self) -> u32 {
        self.core.callback_sequence()
    }
    pub(crate) fn latch_deadline(&self, now: Duration) -> Option<Duration> {
        self.core.latch_deadline(now)
    }
    pub(crate) fn reuse_estimated_timer(&mut self) -> bool {
        self.core.reuse_estimated_timer()
    }
    pub(crate) fn arm_estimated_timer(&mut self, token: RegistrationToken) {
        self.core.arm_estimated_timer(token)
    }
    pub(crate) fn estimated_vblank(&mut self) -> EstimatedVblankAction {
        self.core.estimated_vblank()
    }
    pub(crate) fn kms_submitted(&mut self) -> Option<RegistrationToken> {
        self.core.kms_submitted()
    }
    pub(crate) fn record_drm_sequence(&mut self, sequence: u32) -> Option<u32> {
        self.core.record_drm_sequence(sequence)
    }
    pub(crate) fn retry_after_estimated_vblank(&mut self) {
        self.core.retry_after_estimated_vblank()
    }
    pub(crate) fn kms_submission_failed(
        &mut self,
        now: Duration,
        target: Duration,
    ) -> (Duration, bool) {
        self.core.kms_submission_failed(now, target)
    }
    pub(crate) fn retry_deadline(&self, now: Duration) -> Option<Duration> {
        self.core.retry_deadline(now)
    }
    pub(crate) fn reset_kms_retry(&mut self) {
        self.core.reset_kms_retry()
    }
    pub(crate) fn finish_nested_redraw(&mut self) {
        self.core.finish_nested_redraw()
    }
    pub(crate) fn finish_headless_redraw(&mut self) -> Option<RegistrationToken> {
        self.core.finish_headless_redraw()
    }
    pub(crate) fn arm_headless_timer(&mut self, token: RegistrationToken) {
        self.core.arm_headless_timer(token)
    }
    pub(crate) fn headless_timer_fired(&mut self) {
        self.core.headless_timer_fired()
    }
    pub(crate) fn nested_submitted(&mut self) -> Option<RegistrationToken> {
        self.core.nested_submitted()
    }
    pub(crate) fn nested_frame_timer(&self) -> Option<RegistrationToken> {
        self.core.nested_frame_timer()
    }
    pub(crate) fn arm_nested_frame_timer(&mut self, token: RegistrationToken) {
        self.core.arm_nested_frame_timer(token)
    }
    pub(crate) fn nested_frame_timer_fired(&mut self) -> bool {
        self.core.nested_frame_timer_fired()
    }
    pub(crate) fn fifo_timer(&self) -> Option<RegistrationToken> {
        self.core.fifo_timer()
    }
    pub(crate) fn arm_fifo_timer(&mut self, token: RegistrationToken) {
        self.core.arm_fifo_timer(token)
    }
    pub(crate) fn take_fifo_timer(&mut self) -> Option<RegistrationToken> {
        self.core.take_fifo_timer()
    }
}

/// Integration-fixture fault injection, absent from production builds.
#[cfg(test)]
impl OutputSchedule {
    pub(crate) fn redraw_state(&self) -> RedrawState {
        self.core.redraw_state()
    }
    pub(crate) fn latch_ready(&self) -> bool {
        self.core.latch_ready()
    }
    pub(crate) fn retry_pending(&self) -> bool {
        self.core.retry_pending()
    }
    pub(crate) fn test_set_redraw(&mut self, state: RedrawState) {
        self.core.restore_redraw_state(state);
    }
    pub(crate) fn test_set_clock(&mut self, clock: FrameClock) {
        self.core.replace_clock(clock);
    }
    pub(crate) fn test_enable_latch(&mut self) {
        self.core.enable_latch();
    }
    pub(crate) fn test_latch_ready(&mut self) {
        self.core.mark_latch_ready();
    }
    pub(crate) fn test_latch_woke(&mut self, deadline: Duration, actual: Duration) {
        self.stats.latch_woke(deadline, actual);
    }
    pub(crate) fn test_record_render(&mut self, elapsed: Duration, result: &RenderResult) {
        self.stats.rendered(elapsed, result);
    }
}

fn apply_vblank_transition<D>(
    transition: VblankTransition<RegistrationToken>,
    event_loop: &LoopHandle<'_, D>,
    name: &str,
) -> bool {
    for token in transition.cancelled_timers.into_iter().flatten() {
        event_loop.remove(token);
    }
    if let Some(state) = transition.unexpected_state {
        error!("unexpected redraw state for output {name} (should be WaitingForVBlank); can happen when resuming from sleep or powering on monitors: {state:?}");
    }
    transition.redraw_needed
}

/// Compatibility adapter for callers which retain only the platform timer state.
#[cfg(test)]
pub(crate) fn take_redraw_state_on_vblank<D>(
    redraw_state: &mut RedrawState,
    event_loop: &LoopHandle<'_, D>,
    name: &str,
) -> bool {
    apply_vblank_transition(
        clarion_timing::scheduling::take_redraw_state_on_vblank(mem::take(redraw_state)),
        event_loop,
        name,
    )
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
    use smithay::reexports::calloop::EventLoop;

    use super::*;

    #[test]
    fn teardown_cancels_owned_deadlines_without_a_compositor() {
        // The scheduling owner needs no Wayland display, GPU, or Clarion/State
        // instance to retire every deadline, including an estimate behind a latch.
        let mut event_loop = EventLoop::<()>::try_new().unwrap();
        let handle = event_loop.handle();
        let fired = Rc::new(Cell::new(0));
        let timer = || {
            let fired = Rc::clone(&fired);
            handle
                .insert_source(Timer::immediate(), move |_, _, _| {
                    fired.set(fired.get() + 1);
                    TimeoutAction::Drop
                })
                .unwrap()
        };
        let mut schedule = OutputSchedule::new(Some(Duration::from_millis(16)), false, true);
        schedule.queue_redraw();
        schedule.arm_estimated_timer(timer());
        schedule.queue_redraw();
        schedule.arm_latch(timer(), Duration::ZERO);
        schedule.arm_fifo_timer(timer());
        schedule.arm_nested_frame_timer(timer());
        schedule.cancel_timers(&handle);
        event_loop.dispatch(Duration::ZERO, &mut ()).unwrap();
        assert_eq!(fired.get(), 0);
        assert!(!schedule.is_queued());
        assert!(schedule.fifo_timer().is_none());
        assert!(schedule.nested_frame_timer().is_none());
        // Teardown is idempotent even when called after the sources were removed.
        schedule.cancel_timers(&handle);
    }

    #[test]
    fn latch_deadline_is_clamped_to_max_forward_horizon() {
        let mut schedule = OutputSchedule::new(Some(Duration::from_millis(16)), false, true);
        schedule.queue_redraw();

        let now = Duration::from_secs(10);
        let deadline = schedule.latch_deadline(now).unwrap();
        // Deadline must never exceed now + 125ms
        assert!(deadline <= now + Duration::from_millis(125));
    }

    #[test]
    fn physical_feedback_counts_once_and_empty_final_frame_breaks_activity() {
        let event_loop = EventLoop::<()>::try_new().unwrap();
        let handle = event_loop.handle();
        let period = Duration::from_millis(10);
        let mut actual = Duration::from_secs(1);
        let mut schedule = OutputSchedule::new(Some(period), false, true);
        schedule.set_animations_remain(true);
        for _ in 0..2 {
            schedule.queue_redraw();
            assert_eq!(schedule.kms_submitted(), None);
            assert!(!schedule.hardware_vblank(&handle, "DP-1"));
            schedule.record_presentation_feedback(actual, actual);
            schedule.presented(actual, Some(actual));
            actual += period * 2;
        }
        let active = schedule.timings("DP-1".into());
        assert_eq!(active.presentations, 2);
        assert_eq!(active.continuous_active_intervals, 1);
        assert_eq!(active.continuous_active_long_intervals, 1);
        schedule.set_animations_remain(false);
        schedule.queue_redraw();
        schedule.finish_redraw(Duration::ZERO, &RenderResult::NoDamage);
        // A later client update after intentional idle does not count as dropped animation.
        actual += Duration::from_secs(30);
        schedule.record_presentation_feedback(actual, Duration::ZERO);
        schedule.presented(actual, None);
        let idle = schedule.timings("DP-1".into());
        assert_eq!(idle.presentations, 3);
        assert_eq!(idle.continuous_active_intervals, 1);
        assert_eq!(idle.continuous_active_long_intervals, 1);
    }

    #[test]
    fn reconnected_output_has_a_new_generation_even_with_identical_initial_counters() {
        let first = OutputSchedule::new(Some(Duration::from_millis(10)), false, true);
        let reconnected = OutputSchedule::new(Some(Duration::from_millis(10)), false, true);
        let before = first.timings("DP-1".into());
        let after = reconnected.timings("DP-1".into());
        assert!(before.output_generation.is_some());
        assert!(after.output_generation.is_some());
        assert_ne!(before.output_generation, after.output_generation);
        assert_eq!(
            before.output_generation,
            first.timings("DP-1".into()).output_generation
        );
        assert_eq!(
            (before.redraws, before.presentations),
            (after.redraws, after.presentations)
        );
    }

    #[test]
    fn gpu_deferral_keeps_the_real_long_interval_and_full_cpu_work_visible() {
        for result in [RenderResult::GpuDeferred, RenderResult::Skipped] {
            let event_loop = EventLoop::<()>::try_new().unwrap();
            let handle = event_loop.handle();
            let period = Duration::from_millis(10);
            let first = Duration::from_secs(1);
            let mut schedule = OutputSchedule::new(Some(period), false, true);
            schedule.set_animations_remain(true);
            schedule.queue_redraw();
            schedule.kms_submitted();
            schedule.hardware_vblank(&handle, "DP-1");
            schedule.stats.presented_with_cpu_sampler(
                first,
                first,
                schedule.core.clock(),
                schedule.continue_after_presentation,
                || Some(Duration::from_millis(100)),
            );
            schedule.core.presented(first, Some(first));

            schedule.queue_redraw();
            schedule.finish_redraw(Duration::from_millis(1), &result);
            assert!(
                !schedule.is_queued(),
                "deferred work must wait for a completion or paced retry"
            );
            schedule.queue_redraw();
            schedule.kms_submitted();
            schedule.hardware_vblank(&handle, "DP-1");
            let second = first + period * 8;
            schedule.stats.presented_with_cpu_sampler(
                second,
                second,
                schedule.core.clock(),
                schedule.continue_after_presentation,
                || Some(Duration::from_millis(140)),
            );
            let stats = schedule.timings("DP-1".into());
            assert_eq!(
                stats.skipped, 1,
                "legacy skipped counter includes GPU deferrals"
            );
            if result == RenderResult::GpuDeferred {
                assert_eq!(stats.gpu_work_deferrals, Some(1));
                assert_eq!(stats.continuous_active_intervals, 1);
                assert_eq!(stats.continuous_active_long_intervals, 1);
                assert_eq!(stats.continuous_active_interval_max_ms, Some(80.));
                assert_eq!(stats.compositor_main_thread_cpu_max_ms, Some(40.));
                assert_eq!(
                    stats.compositor_main_thread_cpu_samples[0].cpu_time_ms,
                    Some(40.)
                );
                assert_eq!(
                    stats.continuous_active_long_interval_samples[0].interval_ms,
                    80.
                );
            } else {
                assert_eq!(stats.gpu_work_deferrals, Some(0));
                assert_eq!(stats.continuous_active_intervals, 0);
                assert!(stats.compositor_main_thread_cpu_samples.is_empty());
            }
        }
    }

    #[test]
    fn gpu_tag_routes_only_to_its_completed_output_redraw() {
        let mut schedule = OutputSchedule::new(Some(Duration::from_millis(10)), false, true);
        let tag = schedule.gpu_timing_tag().unwrap();
        let sample = GpuTimingSample {
            tag,
            gpu_time: Duration::from_millis(2),
        };
        assert!(!schedule.record_gpu_timing(sample));
        schedule.finish_redraw(Duration::from_millis(1), &RenderResult::Submitted);
        assert!(schedule.record_gpu_timing(sample));
        assert!(!schedule.record_gpu_timing(sample));
        let snapshot = schedule.timings("DP-1".into());
        assert_eq!(snapshot.gpu_frame_work_ms.unwrap().last, 2.);
        assert_eq!(snapshot.cpu_frame_work_ms.unwrap().last, 1.);
        assert_eq!(schedule.gpu_timing_tag().unwrap().redraw_sequence, 2);
        let mut reconnected = OutputSchedule::new(Some(Duration::from_millis(10)), false, true);
        reconnected.finish_redraw(Duration::ZERO, &RenderResult::Submitted);
        assert!(!reconnected.record_gpu_timing(sample));
        assert!(reconnected
            .timings("DP-1".into())
            .gpu_frame_work_ms
            .is_none());
    }
}
