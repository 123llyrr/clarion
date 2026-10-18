//! Per-output redraw state machine. Timer registration stays on the compositor's
//! event loop; only this owner changes the state carried between those callbacks.
//!
//! Backends report submission/presentation events and execute returned actions.
//! They cannot overwrite redraw, retry or latch flags independently. An older
//! empty-frame estimate remains attached while a newer redraw waits for its latch;
//! failed final frames retain a retry after their immediately queued damage clears.

use std::mem;
use std::time::Duration;

use crate::frame_clock::FrameClock;

/// Maximum forward horizon for this backend's latch timer scheduling.
/// The same 125 ms cap is observed in Catalina's WSDisplayUpdateQueueSchedule;
/// sample and function evidence are in docs/macos-catalina-audit-20261011.md.
/// Prevents timer overflow or excessive sleep during clock/mode anomalies.
const MAX_LATCH_HORIZON: Duration = Duration::from_millis(125);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RedrawState<T> {
    /// No pending redraw or presentation.
    #[default]
    Idle,
    /// Ready for rendering in this event-loop iteration.
    Queued,
    /// Damage is queued, but physical KMS rendering waits for the latch deadline.
    WaitingForLatch {
        token: T,
        /// Retain older empty-frame callback/FIFO deadlines while waiting.
        estimated_vblank: Option<T>,
    },
    /// KMS owns a submitted frame; additional damage waits for its presentation.
    WaitingForVBlank { redraw_needed: bool },
    /// Empty or failed frame paced by the estimated refresh instead of hardware.
    WaitingForEstimatedVBlank(T),
    /// New damage arrived before that estimate fired.
    WaitingForEstimatedVBlankAndQueued(T),
}

impl<T: Copy> RedrawState<T> {
    fn queue_redraw(self) -> Self {
        match self {
            Self::Idle => Self::Queued,
            value @ Self::WaitingForLatch { .. } => value,
            Self::WaitingForEstimatedVBlank(token) => {
                Self::WaitingForEstimatedVBlankAndQueued(token)
            }
            value @ (Self::Queued | Self::WaitingForEstimatedVBlankAndQueued(_)) => value,
            Self::WaitingForVBlank { .. } => Self::WaitingForVBlank {
                redraw_needed: true,
            },
        }
    }
}

/// What the estimated refresh callback must do after advancing the scheduling state.
#[derive(Debug, PartialEq, Eq)]
pub enum EstimatedVblankAction {
    SendCallbacks,
    QueueRedraw,
    RedrawAlreadyQueued,
}

pub struct OutputScheduler<T> {
    redraw: RedrawState<T>,
    clock: FrameClock,
    latch_enabled: bool,
    latch_ready: bool,
    kms_retry: KmsRetry,
    animations_remain: bool,
    retry_after_estimated_vblank: bool,
    last_drm_sequence: Option<u32>,
    /// Throttle client callbacks to one per refresh. KMS advances this when buffers
    /// latch at submission, so clients may prepare their next frame while KMS waits.
    /// Empty frames advance only when their estimated deadline fires, avoiding a
    /// client busy loop that repeatedly commits without changing any damage.
    callback_sequence: u32,
    fifo_timer: Option<T>,
    nested_frame_timer: Option<T>,
}

impl<T: Copy> OutputScheduler<T> {
    pub fn new(refresh_interval: Option<Duration>, vrr: bool, latch_enabled: bool) -> Self {
        Self {
            redraw: RedrawState::Idle,
            clock: FrameClock::new(refresh_interval, vrr),
            latch_enabled,
            latch_ready: false,
            kms_retry: KmsRetry::default(),
            animations_remain: false,
            retry_after_estimated_vblank: false,
            last_drm_sequence: None,
            callback_sequence: 0,
            fifo_timer: None,
            nested_frame_timer: None,
        }
    }

    pub fn clock(&self) -> &FrameClock {
        &self.clock
    }

    pub fn reset_timing(&mut self, interval: Option<Duration>, vrr: bool) {
        self.clock = FrameClock::new(interval, vrr);
    }

    pub fn set_vrr(&mut self, enabled: bool) {
        self.clock.set_vrr(enabled);
    }

    pub fn queue_redraw(&mut self) {
        self.redraw = self.redraw.queue_redraw();
    }

    pub fn is_queued(&self) -> bool {
        matches!(
            self.redraw,
            RedrawState::Queued | RedrawState::WaitingForEstimatedVBlankAndQueued(_)
        )
    }

    pub fn set_animations_remain(&mut self, remain: bool) {
        self.animations_remain = remain;
    }
    pub fn animations_remain(&self) -> bool {
        self.animations_remain
    }
    pub fn callback_sequence(&self) -> u32 {
        self.callback_sequence
    }

    pub fn begin_redraw(&mut self, now: Duration, delay: Option<Duration>) -> Duration {
        assert!(self.is_queued());
        self.latch_ready = false;
        if self.latch_enabled {
            self.clock.latch_started(delay.unwrap_or_default());
        }
        self.clock.next_presentation_time_at(now)
    }

    pub fn finish_redraw(&mut self, elapsed: Duration, result: &RenderOutcome) {
        if *result == RenderOutcome::Submitted && self.latch_enabled {
            self.clock.rendered(elapsed);
        }
        if *result == RenderOutcome::Skipped {
            self.redraw = match self.redraw {
                RedrawState::WaitingForEstimatedVBlank(token)
                | RedrawState::WaitingForEstimatedVBlankAndQueued(token) => {
                    RedrawState::WaitingForEstimatedVBlank(token)
                }
                _ => RedrawState::Idle,
            };
        } else {
            self.retry_after_estimated_vblank = false;
            self.kms_retry.reset();
        }
    }

    pub fn gpu_completed(&mut self, elapsed: Duration) {
        if self.latch_enabled {
            self.clock.gpu_completed(elapsed);
        }
    }

    pub fn latch_deadline(&self, now: Duration) -> Option<Duration> {
        (self.latch_enabled && !self.latch_ready && self.is_queued()).then(|| {
            let deadline = self.clock.next_latch_time_at(now);
            let max_horizon = now.saturating_add(MAX_LATCH_HORIZON);
            deadline.min(max_horizon)
        })
    }

    pub fn arm_latch(&mut self, token: T) {
        let estimated_vblank = match self.redraw {
            RedrawState::Queued => None,
            RedrawState::WaitingForEstimatedVBlankAndQueued(token) => Some(token),
            _ => unreachable!("latch requires queued damage"),
        };
        self.redraw = RedrawState::WaitingForLatch {
            token,
            estimated_vblank,
        };
    }

    pub fn latch_fired(&mut self) {
        if let RedrawState::WaitingForLatch {
            estimated_vblank, ..
        } = self.redraw
        {
            self.redraw = estimated_vblank.map_or(
                RedrawState::Queued,
                RedrawState::WaitingForEstimatedVBlankAndQueued,
            );
            self.latch_ready = true;
        }
    }

    /// Return the invalidated timer while preserving queued damage and any older estimate.
    pub fn cancel_latch(&mut self) -> Option<T> {
        self.latch_ready = false;
        if let RedrawState::WaitingForLatch {
            token,
            estimated_vblank,
        } = self.redraw
        {
            self.redraw = estimated_vblank.map_or(
                RedrawState::Queued,
                RedrawState::WaitingForEstimatedVBlankAndQueued,
            );
            Some(token)
        } else {
            None
        }
    }

    /// Keep the original empty-frame deadline instead of postponing its callbacks.
    pub fn reuse_estimated_timer(&mut self) -> bool {
        match self.redraw {
            RedrawState::Queued => false,
            RedrawState::WaitingForEstimatedVBlank(token)
            | RedrawState::WaitingForEstimatedVBlankAndQueued(token) => {
                self.redraw = RedrawState::WaitingForEstimatedVBlank(token);
                true
            }
            _ => unreachable!("an estimate requires queued damage or an existing estimate"),
        }
    }

    pub fn arm_estimated_timer(&mut self, token: T) {
        assert!(matches!(self.redraw, RedrawState::Queued));
        self.redraw = RedrawState::WaitingForEstimatedVBlank(token);
    }

    pub fn estimated_vblank(&mut self) -> EstimatedVblankAction {
        self.callback_sequence = self.callback_sequence.wrapping_add(1);
        let retry = mem::take(&mut self.retry_after_estimated_vblank);
        match mem::replace(&mut self.redraw, RedrawState::Idle) {
            RedrawState::WaitingForLatch {
                token,
                estimated_vblank: Some(_),
            } => {
                self.redraw = RedrawState::WaitingForLatch {
                    token,
                    estimated_vblank: None,
                };
                return EstimatedVblankAction::SendCallbacks;
            }
            RedrawState::WaitingForEstimatedVBlankAndQueued(_) => {
                self.redraw = RedrawState::Queued;
                return EstimatedVblankAction::RedrawAlreadyQueued;
            }
            RedrawState::WaitingForEstimatedVBlank(_) => (),
            _ => unreachable!("estimated vblank without a pending estimate"),
        }
        if retry || self.animations_remain {
            EstimatedVblankAction::QueueRedraw
        } else {
            EstimatedVblankAction::SendCallbacks
        }
    }

    /// A submitted KMS frame supersedes an older empty-frame timer.
    pub fn kms_submitted(&mut self) -> Option<T> {
        let old = mem::replace(
            &mut self.redraw,
            RedrawState::WaitingForVBlank {
                redraw_needed: false,
            },
        );
        let timer = match old {
            RedrawState::Queued => None,
            RedrawState::WaitingForEstimatedVBlankAndQueued(token) => Some(token),
            _ => unreachable!("KMS submission without queued damage"),
        };
        self.callback_sequence = self.callback_sequence.wrapping_add(1);
        timer
    }

    /// Consume hardware feedback without performing platform timer operations.
    pub fn hardware_vblank(&mut self) -> VblankTransition<T> {
        let old = mem::replace(&mut self.redraw, RedrawState::Idle);
        take_redraw_state_on_vblank(old)
    }

    pub fn presented(&mut self, actual: Duration, target: Option<Duration>) {
        if let Some(target) = target {
            self.clock.presented_with_target(actual, target);
        } else {
            self.clock.presented(actual);
        }
    }
    pub fn record_drm_sequence(&mut self, sequence: u32) -> Option<u32> {
        self.last_drm_sequence
            .replace(sequence)
            .map(|last| sequence.wrapping_sub(last))
    }
    pub fn retry_after_estimated_vblank(&mut self) {
        self.retry_after_estimated_vblank = true;
    }
    pub fn kms_submission_failed(&mut self, now: Duration, target: Duration) -> (Duration, bool) {
        self.kms_retry.failed(now, target)
    }
    pub fn retry_deadline(&self, now: Duration) -> Option<Duration> {
        self.kms_retry.deadline_at(now)
    }
    pub fn reset_kms_retry(&mut self) {
        self.kms_retry.reset();
    }
    pub fn finish_nested_redraw(&mut self) {
        assert!(matches!(self.redraw, RedrawState::Queued));
        self.redraw = RedrawState::Idle;
    }
    pub fn finish_headless_redraw(&mut self) -> Option<T> {
        let timer = match mem::take(&mut self.redraw) {
            RedrawState::Queued => None,
            RedrawState::WaitingForEstimatedVBlankAndQueued(token) => Some(token),
            _ => unreachable!("headless redraw without queued damage"),
        };
        self.callback_sequence = self.callback_sequence.wrapping_add(1);
        timer
    }
    pub fn arm_headless_timer(&mut self, token: T) {
        assert!(matches!(self.redraw, RedrawState::Idle));
        self.redraw = RedrawState::WaitingForEstimatedVBlank(token);
    }
    pub fn headless_timer_fired(&mut self) {
        if matches!(
            self.redraw,
            RedrawState::WaitingForEstimatedVBlank(_)
                | RedrawState::WaitingForEstimatedVBlankAndQueued(_)
        ) {
            self.redraw = RedrawState::Queued;
        }
    }
    pub fn nested_submitted(&mut self) -> Option<T> {
        self.callback_sequence = self.callback_sequence.wrapping_add(1);
        self.nested_frame_timer.take()
    }
    pub fn nested_frame_timer(&self) -> Option<T> {
        self.nested_frame_timer
    }
    pub fn arm_nested_frame_timer(&mut self, token: T) {
        assert!(self.nested_frame_timer.is_none());
        self.nested_frame_timer = Some(token);
    }
    pub fn nested_frame_timer_fired(&mut self) -> bool {
        if self.nested_frame_timer.take().is_none() {
            return false;
        }
        self.callback_sequence = self.callback_sequence.wrapping_add(1);
        true
    }
    pub fn fifo_timer(&self) -> Option<T> {
        self.fifo_timer
    }
    pub fn arm_fifo_timer(&mut self, token: T) {
        assert!(self.fifo_timer.is_none());
        self.fifo_timer = Some(token);
    }
    pub fn take_fifo_timer(&mut self) -> Option<T> {
        self.fifo_timer.take()
    }

    /// Retire all owned deadlines; the platform removes the returned handles.
    pub fn take_timers(&mut self) -> [Option<T>; 4] {
        let mut timers = [
            self.fifo_timer.take(),
            self.nested_frame_timer.take(),
            None,
            None,
        ];
        match mem::take(&mut self.redraw) {
            RedrawState::WaitingForLatch {
                token,
                estimated_vblank,
            } => {
                timers[2] = Some(token);
                timers[3] = estimated_vblank;
            }
            RedrawState::WaitingForEstimatedVBlank(token)
            | RedrawState::WaitingForEstimatedVBlankAndQueued(token) => timers[2] = Some(token),
            _ => (),
        }
        self.latch_ready = false;
        self.retry_after_estimated_vblank = false;
        timers
    }

    pub fn redraw_state(&self) -> RedrawState<T> {
        self.redraw
    }
    pub fn latch_ready(&self) -> bool {
        self.latch_ready
    }
    pub fn retry_pending(&self) -> bool {
        self.retry_after_estimated_vblank
    }

    /// Restore a platform scheduling checkpoint. Timer ownership must match the restored state.
    pub fn restore_redraw_state(&mut self, state: RedrawState<T>) {
        self.redraw = state;
    }
    pub fn replace_clock(&mut self, clock: FrameClock) {
        self.clock = clock;
    }
    pub fn enable_latch(&mut self) {
        self.latch_enabled = true;
    }
    pub fn mark_latch_ready(&mut self) {
        self.latch_ready = true;
    }
    pub fn latch_enabled(&self) -> bool {
        self.latch_enabled
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum RenderOutcome {
    Submitted,
    NoDamage,
    Skipped,
}

/// Platform actions returned by a hardware presentation state transition.
#[derive(Debug)]
pub struct VblankTransition<T> {
    pub redraw_needed: bool,
    pub cancelled_timers: [Option<T>; 2],
    pub unexpected_state: Option<RedrawState<T>>,
}

pub fn take_redraw_state_on_vblank<T: Copy>(state: RedrawState<T>) -> VblankTransition<T> {
    let (redraw_needed, cancelled_timers, unexpected_state) = match state {
        RedrawState::WaitingForVBlank { redraw_needed } => (redraw_needed, [None, None], None),
        RedrawState::WaitingForLatch {
            token,
            estimated_vblank,
        } => (true, [Some(token), estimated_vblank], Some(state)),
        RedrawState::WaitingForEstimatedVBlank(token)
        | RedrawState::WaitingForEstimatedVBlankAndQueued(token) => {
            (true, [Some(token), None], Some(state))
        }
        RedrawState::Idle | RedrawState::Queued => (true, [None, None], Some(state)),
    };
    VblankTransition {
        redraw_needed,
        cancelled_timers,
        unexpected_state,
    }
}

const INITIAL_BACKOFF: Duration = Duration::from_millis(50);
const MAX_BACKOFF: Duration = Duration::from_secs(1);
const WARNING_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Default)]
pub struct KmsRetry {
    failures: u32,
    deadline: Option<Duration>,
    last_warning: Option<Duration>,
}

impl KmsRetry {
    /// The first failure keeps the normal next-frame retry. Repeated failures back off,
    /// but never require new input or a session event to discover that KMS recovered.
    pub fn failed(&mut self, now: Duration, target: Duration) -> (Duration, bool) {
        self.failures = self.failures.saturating_add(1);
        let delay = if self.failures == 1 {
            Duration::ZERO
        } else {
            (INITIAL_BACKOFF * (1 << (self.failures - 2).min(5))).min(MAX_BACKOFF)
        };
        let deadline = target.max(now.saturating_add(delay));
        self.deadline = Some(deadline);

        let warn = self
            .last_warning
            .is_none_or(|last| now.saturating_sub(last) >= WARNING_INTERVAL);
        if warn {
            self.last_warning = Some(now);
        }
        (deadline, warn)
    }

    pub fn deadline_at(&self, now: Duration) -> Option<Duration> {
        self.deadline.filter(|deadline| *deadline > now)
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persistent_failure_limits_attempts_and_warnings_without_stopping_recovery() {
        let mut retry = KmsRetry::default();
        let start = Duration::from_secs(1);
        let end = start + Duration::from_secs(5);
        let mut now = start;
        let mut attempts = 0;
        let mut warnings = 0;
        while now < end {
            attempts += 1;
            let (deadline, warn) = retry.failed(now, now + Duration::from_millis(16));
            warnings += usize::from(warn);
            assert!(deadline > now);
            assert!(deadline - now <= MAX_BACKOFF);
            // An input/commit before the deadline must not permit a fresh attempt.
            assert_eq!(
                retry.deadline_at(deadline - Duration::from_nanos(1)),
                Some(deadline)
            );
            assert_eq!(retry.deadline_at(deadline), None);
            now = deadline;
        }
        assert!(
            attempts <= 12,
            "attempted {attempts} submissions in five seconds"
        );
        assert_eq!(warnings, 1);
        assert!(
            retry.failed(now, now).1,
            "ongoing failures must still be reported"
        );
    }

    #[test]
    fn first_failure_and_recovery_keep_the_normal_frame_deadline() {
        let mut retry = KmsRetry::default();
        let now = Duration::from_secs(1);
        let target = now + Duration::from_millis(3);
        assert_eq!(retry.failed(now, target), (target, true));
        for _ in 0..10 {
            retry.failed(now, target);
        }
        retry.reset();
        assert_eq!(retry.deadline_at(now), None);
        assert_eq!(retry.failed(now, target), (target, true));
    }

    #[test]
    fn outputs_back_off_independently_and_long_failure_runs_remain_bounded() {
        let mut failing = KmsRetry::default();
        let healthy = KmsRetry::default();
        let now = Duration::from_secs(1);
        for _ in 0..1000 {
            failing.failed(now, now);
        }
        assert_eq!(failing.deadline_at(now), Some(now + MAX_BACKOFF));
        assert_eq!(healthy.deadline_at(now), None);
    }
}

/// Monotonic presentation continuity. A previous presentation explicitly requesting
/// another frame qualifies an interval; idle wakeups and VRR do not count as drops.
#[derive(Debug, Default)]
pub struct PresentationSequence {
    last: Option<Duration>,
    continue_after_last: bool,
    fixed_interval: Option<Duration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentationSample {
    pub interval: Option<Duration>,
    pub active_interval: Option<Duration>,
    pub long_active_interval: bool,
}

impl PresentationSequence {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    pub fn interrupt(&mut self) {
        self.continue_after_last = false;
    }

    /// Missing hardware time breaks comparability; duplicate and out-of-order
    /// timestamps leave the accepted sequence unchanged.
    pub fn presented(
        &mut self,
        actual: Duration,
        fixed_interval: Option<Duration>,
        continue_after: bool,
    ) -> Option<PresentationSample> {
        if actual.is_zero() {
            // A hardware presentation happened, but its timestamp is unavailable.
            // Joining the surrounding samples would call that data gap a dropped frame.
            self.reset();
            return None;
        }
        if self.last.is_some_and(|last| actual <= last) {
            return None;
        }
        // A changed timing domain is not a dropped frame, even if the adapter
        // has not explicitly reset history yet.
        let interval = self
            .last
            .filter(|_| self.fixed_interval == fixed_interval)
            .map(|last| actual - last);
        let active_interval =
            interval.filter(|_| self.continue_after_last && fixed_interval.is_some());
        let long_active_interval = active_interval
            .zip(fixed_interval)
            .is_some_and(|(actual, budget)| actual > budget.saturating_add(budget / 2));
        self.last = Some(actual);
        self.continue_after_last = continue_after;
        self.fixed_interval = fixed_interval;
        Some(PresentationSample {
            interval,
            active_interval,
            long_active_interval,
        })
    }
}

#[cfg(test)]
mod scheduling_tests {
    use super::*;

    #[test]
    fn queued_damage_and_older_callbacks_survive_latch_transitions() {
        let mut scheduler =
            OutputScheduler::<u32>::new(Some(Duration::from_millis(16)), false, true);
        scheduler.queue_redraw();
        scheduler.arm_estimated_timer(1);
        scheduler.queue_redraw();
        scheduler.arm_latch(2);
        assert_eq!(
            scheduler.estimated_vblank(),
            EstimatedVblankAction::SendCallbacks
        );
        assert_eq!(
            scheduler.redraw_state(),
            RedrawState::WaitingForLatch {
                token: 2,
                estimated_vblank: None
            }
        );
        scheduler.latch_fired();
        assert!(scheduler.is_queued());
        assert!(scheduler.latch_ready());
        assert_eq!(scheduler.kms_submitted(), None);
        scheduler.queue_redraw();
        let action = scheduler.hardware_vblank();
        assert!(action.redraw_needed);
        assert!(action.unexpected_state.is_none());
        assert_eq!(action.cancelled_timers, [None, None]);
    }

    #[test]
    fn failed_final_frame_keeps_estimated_retry_until_recovery() {
        let mut scheduler =
            OutputScheduler::<u32>::new(Some(Duration::from_millis(16)), false, true);
        scheduler.queue_redraw();
        scheduler.arm_estimated_timer(1);
        scheduler.retry_after_estimated_vblank();
        assert_eq!(
            scheduler.estimated_vblank(),
            EstimatedVblankAction::QueueRedraw
        );
        scheduler.queue_redraw();
        scheduler.finish_redraw(Duration::ZERO, &RenderOutcome::NoDamage);
        assert!(!scheduler.retry_pending());
    }

    #[test]
    fn teardown_returns_every_owned_timer_once() {
        let mut scheduler = OutputScheduler::<u32>::new(None, false, true);
        scheduler.queue_redraw();
        scheduler.arm_estimated_timer(1);
        scheduler.queue_redraw();
        scheduler.arm_latch(2);
        scheduler.arm_fifo_timer(3);
        scheduler.arm_nested_frame_timer(4);
        assert_eq!(
            scheduler.take_timers(),
            [Some(3), Some(4), Some(2), Some(1)]
        );
        assert_eq!(scheduler.take_timers(), [None; 4]);
        assert_eq!(scheduler.redraw_state(), RedrawState::Idle);
    }

    #[test]
    fn unexpected_hardware_feedback_retires_stale_timers_and_requests_recovery() {
        let action = take_redraw_state_on_vblank(RedrawState::WaitingForLatch {
            token: 7,
            estimated_vblank: Some(8),
        });
        assert!(action.redraw_needed);
        assert_eq!(action.cancelled_timers, [Some(7), Some(8)]);
        assert!(action.unexpected_state.is_some());
    }

    #[test]
    fn active_interval_uses_previous_demand_and_strict_threshold() {
        let mut sequence = PresentationSequence::default();
        let start = Duration::from_secs(1);
        let period = Duration::from_millis(10);
        assert_eq!(
            sequence
                .presented(start, Some(period), true)
                .unwrap()
                .interval,
            None
        );
        let at_threshold = sequence
            .presented(start + period + period / 2, Some(period), true)
            .unwrap();
        assert_eq!(
            at_threshold.active_interval,
            Some(Duration::from_millis(15))
        );
        assert!(!at_threshold.long_active_interval);
        let long = sequence
            .presented(start + Duration::from_millis(31), Some(period), false)
            .unwrap();
        assert!(long.long_active_interval);
        let idle = sequence
            .presented(start + Duration::from_secs(20), Some(period), true)
            .unwrap();
        assert!(idle.active_interval.is_none());
        assert!(!idle.long_active_interval);
    }

    #[test]
    fn duplicate_out_of_order_and_unavailable_feedback_do_not_rewind_continuity() {
        let mut sequence = PresentationSequence::default();
        let start = Duration::from_secs(1);
        let period = Duration::from_millis(10);
        assert!(sequence
            .presented(Duration::ZERO, Some(period), true)
            .is_none());
        sequence.presented(start, Some(period), true);
        assert!(sequence.presented(start, Some(period), false).is_none());
        assert!(sequence
            .presented(start - period, Some(period), false)
            .is_none());
        let next = sequence
            .presented(start + period * 2, Some(period), false)
            .unwrap();
        assert_eq!(next.active_interval, Some(period * 2));
        assert!(next.long_active_interval);
    }

    #[test]
    fn variable_refresh_and_unknown_refresh_never_create_drop_denominators() {
        for unknown in [true, false] {
            let mut sequence = PresentationSequence::default();
            let start = Duration::from_secs(1);
            // None is deliberately used for both VRR and unknown refresh.
            sequence.presented(start, None, true);
            let sample = sequence
                .presented(start + Duration::from_secs(20), None, unknown)
                .unwrap();
            assert_eq!(sample.interval, Some(Duration::from_secs(20)));
            assert!(sample.active_interval.is_none());
            assert!(!sample.long_active_interval);
        }
    }

    #[test]
    fn mode_reset_and_known_render_interruption_exclude_idle_and_suspend() {
        let mut sequence = PresentationSequence::default();
        let start = Duration::from_secs(1);
        let period = Duration::from_millis(10);
        sequence.presented(start, Some(period), true);
        sequence.interrupt();
        assert!(sequence
            .presented(start + Duration::from_secs(1), Some(period), true)
            .unwrap()
            .active_interval
            .is_none());
        sequence.reset();
        assert_eq!(
            sequence
                .presented(Duration::from_millis(4), Some(period), true)
                .unwrap()
                .interval,
            None
        );
    }

    #[test]
    fn changed_refresh_domain_does_not_compare_to_the_old_frame_budget() {
        let mut sequence = PresentationSequence::default();
        let start = Duration::from_secs(1);
        sequence.presented(start, Some(Duration::from_millis(10)), true);
        let changed = sequence
            .presented(
                start + Duration::from_secs(1),
                Some(Duration::from_millis(20)),
                true,
            )
            .unwrap();
        assert!(changed.interval.is_none());
        assert!(changed.active_interval.is_none());
        let next = sequence
            .presented(
                start + Duration::from_secs(1) + Duration::from_millis(20),
                Some(Duration::from_millis(20)),
                true,
            )
            .unwrap();
        assert_eq!(next.active_interval, Some(Duration::from_millis(20)));
        assert!(!next.long_active_interval);
        let variable = sequence
            .presented(start + Duration::from_secs(2), None, true)
            .unwrap();
        assert!(variable.interval.is_none());
        assert!(variable.active_interval.is_none());
    }

    #[test]
    fn missing_hardware_time_breaks_interval_comparability_without_faking_a_drop() {
        let mut sequence = PresentationSequence::default();
        let start = Duration::from_secs(1);
        let period = Duration::from_millis(10);
        sequence.presented(start, Some(period), true);
        assert!(sequence
            .presented(Duration::ZERO, Some(period), true)
            .is_none());
        let next = sequence
            .presented(start + period * 2, Some(period), true)
            .unwrap();
        assert!(next.interval.is_none());
        assert!(next.active_interval.is_none());
        assert!(!next.long_active_interval);
    }
}
