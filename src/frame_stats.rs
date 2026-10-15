//! Per-output timing with bounded storage and no allocations or logging in the render path.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use realand_core::scheduling::PresentationSequence;
use realand_ipc::diagnostics::{
    CpuFrameSample, GpuFrameSample, LongPresentationInterval, MainThreadCpuSample,
    OutputFrameTimings, TimingDistribution,
};

use crate::backend::RenderResult;
use crate::frame_clock::FrameClock;

const CAPACITY: usize = 240;

struct Samples {
    values: [f64; CAPACITY],
    len: usize,
    next: usize,
}

impl Default for Samples {
    fn default() -> Self {
        Self {
            values: [0.; CAPACITY],
            len: 0,
            next: 0,
        }
    }
}

impl Samples {
    fn record(&mut self, value: f64) {
        self.values[self.next] = value;
        self.next = (self.next + 1) % CAPACITY;
        self.len = (self.len + 1).min(CAPACITY);
    }

    fn cpu_samples(&self, last_sequence: u64) -> Vec<CpuFrameSample> {
        let start = if self.len == CAPACITY { self.next } else { 0 };
        let before_first = last_sequence.saturating_sub(self.len as u64);
        (0..self.len)
            .map(|offset| CpuFrameSample {
                sequence: before_first.saturating_add(offset as u64 + 1),
                wall_time_ms: self.values[(start + offset) % CAPACITY],
            })
            .collect()
    }

    fn snapshot(&self) -> Option<TimingDistribution> {
        if self.len == 0 {
            return None;
        }
        let mut sorted = self.values[..self.len].to_vec();
        sorted.sort_unstable_by(f64::total_cmp);
        Some(TimingDistribution {
            samples: self.len,
            last: self.values[(self.next + CAPACITY - 1) % CAPACITY],
            mean: sorted.iter().sum::<f64>() / self.len as f64,
            p50: sorted[(self.len * 50).div_ceil(100) - 1],
            p95: sorted[(self.len * 95).div_ceil(100) - 1],
            max: sorted[self.len - 1],
        })
    }
}

struct RecordRing<T: Copy> {
    values: [Option<T>; CAPACITY],
    len: usize,
    next: usize,
    overwritten: u64,
}

impl<T: Copy> Default for RecordRing<T> {
    fn default() -> Self {
        Self {
            values: [None; CAPACITY],
            len: 0,
            next: 0,
            overwritten: 0,
        }
    }
}

impl<T: Copy> RecordRing<T> {
    fn record(&mut self, value: T) {
        if self.len == CAPACITY {
            self.overwritten = self.overwritten.saturating_add(1);
        }
        self.values[self.next] = Some(value);
        self.next = (self.next + 1) % CAPACITY;
        self.len = (self.len + 1).min(CAPACITY);
    }

    fn snapshot(&self) -> Vec<T> {
        let start = if self.len == CAPACITY { self.next } else { 0 };
        (0..self.len)
            .map(|offset| self.values[(start + offset) % CAPACITY].unwrap())
            .collect()
    }
}

type LongIntervalSamples = RecordRing<LongPresentationInterval>;
type MainThreadCpuSamples = RecordRing<MainThreadCpuSample>;

#[derive(Default)]
struct GpuSamples {
    samples: RecordRing<GpuFrameSample>,
    retired_through: u64,
}

impl GpuSamples {
    fn record(&mut self, sample: GpuFrameSample) -> bool {
        if sample.redraw_sequence <= self.retired_through
            || self
                .samples
                .values
                .iter()
                .flatten()
                .any(|record| record.redraw_sequence == sample.redraw_sequence)
        {
            return false;
        }
        if self.samples.len == CAPACITY {
            let (slot, retiring) = self
                .samples
                .values
                .iter()
                .enumerate()
                .map(|(slot, record)| (slot, record.unwrap().redraw_sequence))
                .min_by_key(|(_, sequence)| *sequence)
                .unwrap();
            // Once an ID is retired, bounded storage cannot prove whether a
            // delayed older ID is fresh or a duplicate. Reject that old window
            // explicitly; never reject an out-of-order ID just for being below
            // the largest observed redraw sequence.
            let floor = self.retired_through.max(retiring);
            if sample.redraw_sequence <= floor {
                return false;
            }
            self.retired_through = floor;
            // Evict the oldest sequence, not the earliest worker completion.
            // A high sequence completed first must not retire later low IDs.
            self.samples.next = slot;
        }
        self.samples.record(sample);
        true
    }

    fn snapshot(&self) -> Vec<GpuFrameSample> {
        let mut values = self.samples.snapshot();
        values.sort_unstable_by_key(|sample| sample.redraw_sequence);
        values
    }
}

static NEXT_OUTPUT_GENERATION: AtomicU64 = AtomicU64::new(1);

struct OutputGeneration(Option<u64>);

impl Default for OutputGeneration {
    fn default() -> Self {
        // Allocate once per output owner, never in the frame path. Refuse to reuse
        // IDs after theoretical exhaustion instead of silently wrapping the guard.
        Self(
            NEXT_OUTPUT_GENERATION
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |generation| {
                    generation.checked_add(1)
                })
                .ok(),
        )
    }
}

#[derive(Default)]
pub struct FrameStats {
    generation: OutputGeneration,
    redraws: u64,
    submitted: u64,
    no_damage: u64,
    skipped: u64,
    gpu_work_deferrals: u64,
    presentations: u64,
    presentation_sequence: PresentationSequence,
    presented_interval: Samples,
    continuous_active_interval: Samples,
    continuous_active_intervals: u64,
    continuous_active_long_intervals: u64,
    continuous_active_interval_max_ms: Option<f64>,
    cpu_frame_work_max_ms: Option<f64>,
    long_interval_samples: LongIntervalSamples,
    main_thread_cpu_last: Option<Duration>,
    main_thread_cpu: Samples,
    main_thread_cpu_samples: MainThreadCpuSamples,
    main_thread_cpu_max_ms: Option<f64>,
    main_thread_cpu_unavailable_intervals: u64,
    gpu_frame_work: Samples,
    gpu_frame_samples: GpuSamples,
    gpu_frame_work_max_ms: Option<f64>,
    fixed_refresh_presentations: u64,
    late_presentations: u64,
    kms_latch_deferrals: u64,
    render_time: Samples,
    presentation_error: Samples,
    kms_latch_delay: Samples,
    kms_latch_wakeup_lateness: Samples,
    kms_latch_to_render_delay: Samples,
    pending_latch_wakeup: Option<(Duration, Duration)>,
}

impl FrameStats {
    pub fn gpu_timing_identity(&self) -> Option<(u64, u64)> {
        Some((self.generation.0?, self.redraws.checked_add(1)?))
    }

    pub fn record_gpu_timing(
        &mut self,
        generation: u64,
        redraw_sequence: u64,
        elapsed: Duration,
    ) -> bool {
        if self.generation.0 != Some(generation)
            || redraw_sequence == 0
            || redraw_sequence > self.redraws
        {
            return false;
        }
        let gpu_time_ms = elapsed.as_secs_f64() * 1000.;
        if !self.gpu_frame_samples.record(GpuFrameSample {
            redraw_sequence,
            gpu_time_ms,
        }) {
            return false;
        }
        self.gpu_frame_work.record(gpu_time_ms);
        self.gpu_frame_work_max_ms =
            Some(self.gpu_frame_work_max_ms.unwrap_or(0.).max(gpu_time_ms));
        true
    }
    pub fn rendered(&mut self, elapsed: Duration, result: &RenderResult) {
        self.redraws = self.redraws.saturating_add(1);
        let counter = match result {
            RenderResult::Submitted => &mut self.submitted,
            RenderResult::NoDamage => &mut self.no_damage,
            RenderResult::Skipped | RenderResult::GpuDeferred => &mut self.skipped,
        };
        *counter = counter.saturating_add(1);
        if *result == RenderResult::GpuDeferred {
            self.gpu_work_deferrals = self.gpu_work_deferrals.saturating_add(1);
        }
        let elapsed_ms = elapsed.as_secs_f64() * 1000.;
        self.render_time.record(elapsed_ms);
        self.cpu_frame_work_max_ms = Some(self.cpu_frame_work_max_ms.unwrap_or(0.).max(elapsed_ms));
    }

    pub fn latch_deferred(&mut self, scheduled_delay: Duration) {
        self.kms_latch_deferrals = self.kms_latch_deferrals.saturating_add(1);
        self.kms_latch_delay
            .record(scheduled_delay.as_secs_f64() * 1000.);
    }

    pub fn latch_woke(&mut self, deadline: Duration, actual: Duration) {
        self.kms_latch_wakeup_lateness
            .record(actual.saturating_sub(deadline).as_secs_f64() * 1000.);
        self.pending_latch_wakeup = Some((deadline, actual));
    }

    pub fn latch_cancelled(&mut self) {
        self.pending_latch_wakeup = None;
    }

    /// Consume one scheduled latch and return all delay before rendering begins.
    pub fn redraw_started(&mut self, now: Duration) -> Option<Duration> {
        let (deadline, wakeup) = self.pending_latch_wakeup.take()?;
        self.kms_latch_to_render_delay
            .record(now.saturating_sub(wakeup).as_secs_f64() * 1000.);
        Some(now.saturating_sub(deadline))
    }

    pub fn reset_presentation_history(&mut self) {
        self.presentation_sequence.reset();
        self.main_thread_cpu_last = None;
        self.pending_latch_wakeup = None;
    }

    pub fn interrupt_presentation_continuity(&mut self) {
        self.presentation_sequence.interrupt();
    }

    #[cfg(test)]
    pub fn presented(&mut self, actual: Duration, target: Duration, clock: &FrameClock) {
        self.presented_with_continuation(actual, target, clock, false);
    }

    #[cfg(test)]
    pub fn presented_with_continuation(
        &mut self,
        actual: Duration,
        target: Duration,
        clock: &FrameClock,
        continue_after: bool,
    ) {
        self.presented_with_cpu_sampler(actual, target, clock, continue_after, || None);
    }

    /// Sample a CPU clock only after accepting fresh hardware feedback. Keeping
    /// this lazy prevents duplicate feedback from corrupting the CPU baseline.
    pub fn presented_with_cpu_sampler(
        &mut self,
        actual: Duration,
        target: Duration,
        clock: &FrameClock,
        continue_after: bool,
        sample_cpu: impl FnOnce() -> Option<Duration>,
    ) {
        if actual.is_zero() {
            self.main_thread_cpu_last = None;
        }
        let fixed_interval = clock.refresh_interval().filter(|_| !clock.vrr());
        let Some(sample) =
            self.presentation_sequence
                .presented(actual, fixed_interval, continue_after)
        else {
            return;
        };
        let current_cpu = sample_cpu();
        let cpu_elapsed = current_cpu
            .zip(self.main_thread_cpu_last)
            .and_then(|(current, previous)| current.checked_sub(previous));
        self.main_thread_cpu_last = current_cpu;
        if let Some(interval) = sample.interval {
            self.presented_interval
                .record(interval.as_secs_f64() * 1000.);
        }
        if let Some(interval) = sample.active_interval {
            let interval_ms = interval.as_secs_f64() * 1000.;
            self.continuous_active_interval.record(interval_ms);
            self.continuous_active_interval_max_ms = Some(
                self.continuous_active_interval_max_ms
                    .unwrap_or(0.)
                    .max(interval_ms),
            );
            self.continuous_active_intervals = self.continuous_active_intervals.saturating_add(1);
            let cpu_time_ms = cpu_elapsed.map(|duration| duration.as_secs_f64() * 1000.);
            self.main_thread_cpu_samples.record(MainThreadCpuSample {
                sequence: self.continuous_active_intervals,
                cpu_time_ms,
            });
            if let Some(value) = cpu_time_ms {
                self.main_thread_cpu.record(value);
                self.main_thread_cpu_max_ms =
                    Some(self.main_thread_cpu_max_ms.unwrap_or(0.).max(value));
            } else {
                self.main_thread_cpu_unavailable_intervals =
                    self.main_thread_cpu_unavailable_intervals.saturating_add(1);
            }
            if sample.long_active_interval {
                self.continuous_active_long_intervals =
                    self.continuous_active_long_intervals.saturating_add(1);
                self.long_interval_samples.record(LongPresentationInterval {
                    sequence: self.continuous_active_long_intervals,
                    presentation_time_ns: actual.as_nanos().min(u128::from(u64::MAX)) as u64,
                    interval_ms,
                    refresh_interval_ms: fixed_interval.unwrap().as_secs_f64() * 1000.,
                });
            }
        }
        self.presentations = self.presentations.saturating_add(1);
        if target.is_zero() || !clock.has_presentation_history() {
            return;
        }
        // Subtract integer durations first to retain precision on long-running sessions.
        let error = if actual >= target {
            actual.saturating_sub(target).as_secs_f64()
        } else {
            -target.saturating_sub(actual).as_secs_f64()
        };
        self.presentation_error.record(error * 1000.);
        if let Some(interval) = clock.refresh_interval().filter(|_| !clock.vrr()) {
            self.fixed_refresh_presentations = self.fixed_refresh_presentations.saturating_add(1);
            if actual.saturating_sub(target) > interval / 2 {
                self.late_presentations = self.late_presentations.saturating_add(1);
            }
        }
    }

    pub fn snapshot(
        &self,
        output: String,
        clock: &FrameClock,
        kms_latch_enabled: bool,
    ) -> OutputFrameTimings {
        OutputFrameTimings {
            output,
            output_generation: self.generation.0,
            refresh_interval_ms: clock.refresh_interval().map(|d| d.as_secs_f64() * 1000.),
            variable_refresh_rate: clock.vrr(),
            kms_latch_budget_ms: if kms_latch_enabled {
                clock.latch_budget().map(|d| d.as_secs_f64() * 1000.)
            } else {
                None
            },
            kms_latch_deferrals: self.kms_latch_deferrals,
            kms_latch_delay_ms: self.kms_latch_delay.snapshot(),
            kms_latch_wakeup_lateness_ms: self.kms_latch_wakeup_lateness.snapshot(),
            kms_latch_to_render_delay_ms: self.kms_latch_to_render_delay.snapshot(),
            redraws: self.redraws,
            submitted: self.submitted,
            no_damage: self.no_damage,
            skipped: self.skipped,
            gpu_work_deferrals: Some(self.gpu_work_deferrals),
            presentations: self.presentations,
            fixed_refresh_presentations: self.fixed_refresh_presentations,
            late_presentations: self.late_presentations,
            render_time_ms: self.render_time.snapshot(),
            cpu_frame_work_ms: self.render_time.snapshot(),
            cpu_frame_work_samples: self.render_time.cpu_samples(self.redraws),
            cpu_frame_work_max_ms: self.cpu_frame_work_max_ms,
            // Submission wall time cannot stand in for GPU completion. Genuine
            // device timestamp measurements will populate this only when supported.
            compositor_main_thread_cpu_scope: Some(
                "compositor_main_thread_between_output_presentations".into(),
            ),
            compositor_main_thread_cpu_ms: self.main_thread_cpu.snapshot(),
            compositor_main_thread_cpu_samples: self.main_thread_cpu_samples.snapshot(),
            compositor_main_thread_cpu_max_ms: self.main_thread_cpu_max_ms,
            compositor_main_thread_cpu_unavailable_intervals: self
                .main_thread_cpu_unavailable_intervals,
            compositor_main_thread_cpu_samples_overwritten: self
                .main_thread_cpu_samples
                .overwritten,
            gpu_frame_work_ms: self.gpu_frame_work.snapshot(),
            gpu_frame_work_scope: self
                .gpu_frame_work_max_ms
                .map(|_| "native_output_command_buffer".into()),
            gpu_frame_work_samples: self.gpu_frame_samples.snapshot(),
            gpu_frame_work_max_ms: self.gpu_frame_work_max_ms,
            gpu_frame_work_samples_overwritten: self.gpu_frame_samples.samples.overwritten,
            presented_interval_ms: self.presented_interval.snapshot(),
            continuous_active_interval_ms: self.continuous_active_interval.snapshot(),
            continuous_active_intervals: self.continuous_active_intervals,
            continuous_active_long_intervals: self.continuous_active_long_intervals,
            continuous_active_interval_max_ms: self.continuous_active_interval_max_ms,
            continuous_active_long_interval_samples: self.long_interval_samples.snapshot(),
            continuous_active_long_interval_samples_overwritten: self
                .long_interval_samples
                .overwritten,
            presentation_error_ms: self.presentation_error.snapshot(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_samples_require_current_generation_and_completed_nonzero_redraw() {
        let clock = FrameClock::new(Some(Duration::from_millis(10)), false);
        let mut stats = FrameStats::default();
        let (generation, sequence) = stats.gpu_timing_identity().unwrap();
        assert_eq!(sequence, 1);
        assert!(!stats.record_gpu_timing(generation, sequence, Duration::ZERO));
        stats.rendered(Duration::from_millis(1), &RenderResult::Submitted);
        assert!(!stats.record_gpu_timing(generation, 0, Duration::ZERO));
        assert!(!stats.record_gpu_timing(generation, 2, Duration::ZERO));
        assert!(!stats.record_gpu_timing(generation + 1, 1, Duration::ZERO));
        assert!(stats.record_gpu_timing(generation, 1, Duration::ZERO));
        assert!(!stats.record_gpu_timing(generation, 1, Duration::from_secs(1)));
        let result = stats.snapshot("DP-1".into(), &clock, false);
        assert_eq!(
            result.gpu_frame_work_scope.as_deref(),
            Some("native_output_command_buffer")
        );
        assert_eq!(result.gpu_frame_work_ms.unwrap().last, 0.);
        assert_eq!(result.gpu_frame_work_samples.len(), 1);
        assert_eq!(result.gpu_frame_work_max_ms, Some(0.));
        assert_eq!(stats.gpu_timing_identity(), Some((generation, 2)));
        let mut reconnected = FrameStats::default();
        reconnected.rendered(Duration::ZERO, &RenderResult::Submitted);
        assert!(!reconnected.record_gpu_timing(generation, 1, Duration::ZERO));
        assert!(reconnected
            .snapshot("DP-1".into(), &clock, false)
            .gpu_frame_work_scope
            .is_none());
        stats.redraws = u64::MAX;
        assert!(stats.gpu_timing_identity().is_none());
    }

    #[test]
    fn gpu_completion_order_is_independent_of_redraw_sequence() {
        let clock = FrameClock::new(None, false);
        let mut stats = FrameStats::default();
        let generation = stats.gpu_timing_identity().unwrap().0;
        for _ in 0..3 {
            stats.rendered(Duration::ZERO, &RenderResult::Submitted);
        }
        for sequence in [3, 1, 2] {
            assert!(stats.record_gpu_timing(generation, sequence, Duration::from_millis(sequence)));
        }
        let result = stats.snapshot("DP-1".into(), &clock, false);
        assert_eq!(
            result
                .gpu_frame_work_samples
                .iter()
                .map(|sample| sample.redraw_sequence)
                .collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert_eq!(result.gpu_frame_work_ms.unwrap().mean, 2.);
        assert_eq!(result.gpu_frame_work_max_ms, Some(3.));
    }

    #[test]
    fn gpu_retirement_floor_does_not_skip_unseen_lower_completions_in_the_window() {
        let mut stats = FrameStats::default();
        let generation = stats.gpu_timing_identity().unwrap().0;
        for _ in 0..500 {
            stats.rendered(Duration::ZERO, &RenderResult::Submitted);
        }
        assert!(stats.record_gpu_timing(generation, 300, Duration::from_millis(1)));
        for sequence in 1..=239 {
            assert!(stats.record_gpu_timing(generation, sequence, Duration::from_millis(1)));
        }
        // The first worker finished redraw 300 early. Eviction must retire 1,
        // not 300, so previously unseen redraw 240 can still be accepted.
        assert!(stats.record_gpu_timing(generation, 240, Duration::from_secs(1)));
        assert!(stats.record_gpu_timing(generation, 350, Duration::from_millis(1)));
        assert!(stats.record_gpu_timing(generation, 241, Duration::from_millis(1)));
        assert!(!stats.record_gpu_timing(generation, 1, Duration::from_secs(2)));
        assert!(!stats.record_gpu_timing(generation, 300, Duration::from_secs(2)));
        assert_eq!(stats.gpu_frame_samples.samples.overwritten, 3);
        let records = stats.gpu_frame_samples.snapshot();
        assert_eq!(records.len(), CAPACITY);
        assert_eq!(records.first().unwrap().redraw_sequence, 4);
        assert_eq!(records.last().unwrap().redraw_sequence, 350);
        assert_eq!(stats.gpu_frame_work_max_ms, Some(1000.));
    }

    #[test]
    fn gpu_lifetime_maximum_survives_bounded_raw_and_rolling_eviction() {
        let clock = FrameClock::new(None, false);
        let mut stats = FrameStats::default();
        let generation = stats.gpu_timing_identity().unwrap().0;
        for _ in 0..300 {
            stats.rendered(Duration::ZERO, &RenderResult::Submitted);
        }
        for sequence in 1..=300 {
            assert!(stats.record_gpu_timing(
                generation,
                sequence,
                if sequence == 1 {
                    Duration::from_secs(1)
                } else {
                    Duration::from_millis(1)
                }
            ));
        }
        let result = stats.snapshot("DP-1".into(), &clock, false);
        assert_eq!(result.gpu_frame_work_ms.unwrap().max, 1.);
        assert_eq!(result.gpu_frame_work_max_ms, Some(1000.));
        assert_eq!(result.gpu_frame_work_samples_overwritten, 60);
        assert_eq!(
            result
                .gpu_frame_work_samples
                .first()
                .unwrap()
                .redraw_sequence,
            61
        );
        assert!(!stats.record_gpu_timing(generation, 1, Duration::from_secs(2)));
    }

    #[test]
    fn main_cpu_clock_is_sampled_only_on_accepted_feedback() {
        let clock = FrameClock::new(Some(Duration::from_millis(10)), false);
        let mut stats = FrameStats::default();
        let calls = std::cell::Cell::new(0);
        for (actual, cpu) in [(1000, 100), (1000, 999), (999, 1), (1010, 102)] {
            stats.presented_with_cpu_sampler(
                Duration::from_millis(actual),
                Duration::ZERO,
                &clock,
                true,
                || {
                    calls.set(calls.get() + 1);
                    Some(Duration::from_millis(cpu))
                },
            );
        }
        assert_eq!(calls.get(), 2);
        let result = stats.snapshot("DP-1".into(), &clock, false);
        assert_eq!(
            result.compositor_main_thread_cpu_samples,
            [MainThreadCpuSample {
                sequence: 1,
                cpu_time_ms: Some(2.)
            }]
        );
        assert_eq!(result.compositor_main_thread_cpu_ms.unwrap().p95, 2.);
        assert_eq!(result.compositor_main_thread_cpu_unavailable_intervals, 0);
    }

    #[test]
    fn missing_and_backwards_cpu_clocks_are_unavailable_instead_of_zero() {
        let clock = FrameClock::new(Some(Duration::from_millis(10)), false);
        let mut stats = FrameStats::default();
        for (index, cpu) in [Some(10), None, Some(20), Some(19), Some(21)]
            .into_iter()
            .enumerate()
        {
            stats.presented_with_cpu_sampler(
                Duration::from_millis(1000 + index as u64 * 10),
                Duration::ZERO,
                &clock,
                true,
                || cpu.map(Duration::from_millis),
            );
        }
        let result = stats.snapshot("DP-1".into(), &clock, false);
        assert_eq!(result.compositor_main_thread_cpu_unavailable_intervals, 3);
        assert_eq!(
            result
                .compositor_main_thread_cpu_samples
                .iter()
                .map(|sample| sample.cpu_time_ms)
                .collect::<Vec<_>>(),
            [None, None, None, Some(2.)]
        );
        assert_eq!(result.compositor_main_thread_cpu_ms.unwrap().samples, 1);
    }

    #[test]
    fn idle_interrupt_vrr_and_resets_do_not_reuse_an_old_cpu_interval() {
        let fixed = FrameClock::new(Some(Duration::from_millis(10)), false);
        let vrr = FrameClock::new(fixed.refresh_interval(), true);
        let mut stats = FrameStats::default();
        let feedback = |stats: &mut FrameStats, actual, cpu, clock: &FrameClock, continue_after| {
            stats.presented_with_cpu_sampler(
                Duration::from_millis(actual),
                Duration::ZERO,
                clock,
                continue_after,
                || Some(Duration::from_millis(cpu)),
            );
        };
        feedback(&mut stats, 1000, 100, &fixed, false);
        feedback(&mut stats, 2000, 200, &fixed, true);
        feedback(&mut stats, 2010, 202, &fixed, true);
        stats.interrupt_presentation_continuity();
        feedback(&mut stats, 2020, 300, &fixed, true);
        stats.reset_presentation_history();
        feedback(&mut stats, 3000, 1000, &fixed, true);
        feedback(&mut stats, 3010, 1002, &fixed, true);
        feedback(&mut stats, 4000, 1100, &vrr, true);
        feedback(&mut stats, 4010, 1110, &fixed, true);
        feedback(&mut stats, 4020, 1112, &fixed, true);
        let result = stats.snapshot("DP-1".into(), &fixed, false);
        assert_eq!(
            result.compositor_main_thread_cpu_samples,
            (1..=3)
                .map(|sequence| MainThreadCpuSample {
                    sequence,
                    cpu_time_ms: Some(2.)
                })
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn strong_cpu_raw_ring_keeps_sequences_eviction_and_lifetime_maximum() {
        let clock = FrameClock::new(Some(Duration::from_millis(10)), false);
        let mut stats = FrameStats::default();
        stats.presented_with_cpu_sampler(
            Duration::from_secs(1),
            Duration::ZERO,
            &clock,
            true,
            || Some(Duration::ZERO),
        );
        for sequence in 1..=250 {
            stats.presented_with_cpu_sampler(
                Duration::from_millis(1000 + sequence * 10),
                Duration::ZERO,
                &clock,
                true,
                || Some(Duration::from_millis(999 + sequence)),
            );
        }
        let result = stats.snapshot("DP-1".into(), &clock, false);
        assert_eq!(result.compositor_main_thread_cpu_samples.len(), CAPACITY);
        assert_eq!(
            result
                .compositor_main_thread_cpu_samples
                .first()
                .unwrap()
                .sequence,
            11
        );
        assert_eq!(
            result
                .compositor_main_thread_cpu_samples
                .last()
                .unwrap()
                .sequence,
            250
        );
        assert_eq!(result.compositor_main_thread_cpu_samples_overwritten, 10);
        assert_eq!(result.compositor_main_thread_cpu_ms.unwrap().max, 1.);
        assert_eq!(result.compositor_main_thread_cpu_max_ms, Some(1000.));
        let fresh = FrameStats::default().snapshot("DP-1".into(), &clock, false);
        assert_ne!(fresh.output_generation, result.output_generation);
        assert!(fresh.compositor_main_thread_cpu_samples.is_empty());
        assert!(fresh.compositor_main_thread_cpu_ms.is_none());
    }

    #[test]
    fn zero_hardware_time_and_unknown_refresh_break_the_cpu_interval() {
        let fixed = FrameClock::new(Some(Duration::from_millis(10)), false);
        let unknown = FrameClock::new(None, false);
        let mut stats = FrameStats::default();
        let mut cpu_clock_reads = 0;
        for (actual, cpu, clock) in [
            (1000, 10, &fixed),
            (0, 999, &fixed),
            (1010, 100, &fixed),
            (1020, 102, &fixed),
            (1030, 200, &unknown),
            (1040, 300, &unknown),
            (1050, 400, &fixed),
            (1060, 402, &fixed),
        ] {
            stats.presented_with_cpu_sampler(
                Duration::from_millis(actual),
                Duration::ZERO,
                clock,
                true,
                || {
                    cpu_clock_reads += 1;
                    Some(Duration::from_millis(cpu))
                },
            );
        }
        let result = stats.snapshot("DP-1".into(), &fixed, false);
        assert_eq!(cpu_clock_reads, 7);
        assert_eq!(result.continuous_active_intervals, 2);
        assert_eq!(result.compositor_main_thread_cpu_unavailable_intervals, 0);
        assert_eq!(
            result.compositor_main_thread_cpu_samples,
            vec![
                MainThreadCpuSample {
                    sequence: 1,
                    cpu_time_ms: Some(2.)
                },
                MainThreadCpuSample {
                    sequence: 2,
                    cpu_time_ms: Some(2.)
                },
            ]
        );
    }

    #[test]
    fn samples_are_bounded_and_percentiles_use_only_recent_values() {
        let mut samples = Samples::default();
        assert!(samples.snapshot().is_none());
        samples.record(10000.);
        for value in 1..=CAPACITY {
            samples.record(value as f64);
        }
        let summary = samples.snapshot().unwrap();
        assert_eq!(summary.samples, CAPACITY);
        assert_eq!(summary.last, 240.);
        assert_eq!(summary.mean, 120.5);
        assert_eq!(summary.p50, 120.);
        assert_eq!(summary.p95, 228.);
        assert_eq!(summary.max, 240.);
        assert_eq!(samples.snapshot(), Some(summary));
    }

    #[test]
    fn independent_outputs_and_render_results_are_not_mixed() {
        let clock = FrameClock::new(None, false);
        let mut stats = FrameStats::default();
        stats.rendered(Duration::from_millis(1), &RenderResult::Submitted);
        stats.rendered(Duration::from_millis(2), &RenderResult::NoDamage);
        stats.rendered(Duration::from_millis(3), &RenderResult::Skipped);
        let result = stats.snapshot("DP-1".into(), &clock, false);
        assert_eq!(
            (
                result.redraws,
                result.submitted,
                result.no_damage,
                result.skipped
            ),
            (3, 1, 1, 1)
        );
        assert_eq!(result.render_time_ms.unwrap().p95, 3.);
        assert_eq!(
            result.cpu_frame_work_samples,
            vec![
                CpuFrameSample {
                    sequence: 1,
                    wall_time_ms: 1.
                },
                CpuFrameSample {
                    sequence: 2,
                    wall_time_ms: 2.
                },
                CpuFrameSample {
                    sequence: 3,
                    wall_time_ms: 3.
                },
            ]
        );
        assert!(result.presentation_error_ms.is_none());
        assert_eq!(result.kms_latch_deferrals, 0);
        assert!(result.kms_latch_delay_ms.is_none());
        assert!(FrameStats::default()
            .snapshot("DP-2".into(), &clock, false)
            .render_time_ms
            .is_none());
    }

    #[test]
    fn latch_deadline_statistics_report_waits_and_current_budget() {
        let clock = FrameClock::new(Some(Duration::from_millis(10)), false);
        let mut stats = FrameStats::default();
        stats.latch_deferred(Duration::from_millis(3));
        stats.latch_deferred(Duration::from_millis(5));
        assert_eq!(
            stats
                .snapshot("winit".into(), &clock, false)
                .kms_latch_budget_ms,
            None
        );
        let result = stats.snapshot("DP-1".into(), &clock, true);
        assert_eq!(result.kms_latch_budget_ms, Some(2.));
        assert_eq!(result.kms_latch_deferrals, 2);
        let delay = result.kms_latch_delay_ms.unwrap();
        assert_eq!(delay.samples, 2);
        assert_eq!(delay.last, 5.);
    }

    #[test]
    fn wake_to_render_delay_is_separate_from_timer_lateness_and_consumed_once() {
        let clock = FrameClock::new(Some(Duration::from_millis(10)), false);
        let mut stats = FrameStats::default();
        let deadline = Duration::from_secs(1);
        let wakeup = deadline + Duration::from_micros(200);
        stats.latch_woke(deadline, wakeup);
        assert_eq!(
            stats.redraw_started(wakeup + Duration::from_millis(3)),
            Some(Duration::from_micros(3200))
        );
        // An immediate redraw without a timer must not reuse the previous wakeup.
        assert_eq!(
            stats.redraw_started(wakeup + Duration::from_millis(4)),
            None
        );
        // A timing change invalidates a fired timer before it can render.
        stats.latch_woke(deadline, wakeup);
        stats.latch_cancelled();
        assert_eq!(
            stats.redraw_started(wakeup + Duration::from_millis(5)),
            None
        );

        let result = stats.snapshot("DP-1".into(), &clock, true);
        assert_eq!(result.kms_latch_wakeup_lateness_ms.unwrap().last, 0.2);
        let delay = result.kms_latch_to_render_delay_ms.unwrap();
        assert_eq!(delay.samples, 1);
        assert_eq!(delay.last, 3.);
    }

    #[test]
    fn early_latch_timestamps_do_not_create_negative_scheduler_delay() {
        let mut stats = FrameStats::default();
        let deadline = Duration::from_secs(1);
        let wakeup = deadline - Duration::from_micros(200);
        stats.latch_woke(deadline, wakeup);
        assert_eq!(stats.redraw_started(wakeup), Some(Duration::ZERO));
        assert_eq!(stats.redraw_started(deadline), None);
    }

    #[test]
    fn late_counter_excludes_unknown_timestamps_and_variable_refresh() {
        let target = Duration::from_secs(123456);
        let mut fixed = FrameClock::new(Some(Duration::from_millis(10)), false);
        fixed.presented(target - Duration::from_millis(10));
        let mut variable = FrameClock::new(fixed.refresh_interval(), true);
        variable.presented(target - Duration::from_millis(10));
        let mut unknown = FrameClock::new(None, false);
        unknown.presented(target - Duration::from_millis(10));
        let mut stats = FrameStats::default();
        stats.presented(Duration::ZERO, target, &fixed);
        stats.presented(target - Duration::from_millis(1), target, &fixed);
        stats.presented(target + Duration::from_millis(5), target, &fixed);
        stats.presented(target + Duration::from_millis(6), target, &fixed);
        stats.presented(target + Duration::from_secs(1), target, &variable);
        stats.presented(target + Duration::from_secs(2), target, &unknown);
        let result = stats.snapshot("DP-1".into(), &fixed, true);
        assert_eq!(result.presentations, 5);
        assert_eq!(result.fixed_refresh_presentations, 3);
        assert_eq!(result.late_presentations, 1);
        assert_eq!(result.presentation_error_ms.unwrap().samples, 5);
    }

    #[test]
    fn first_frame_and_missing_target_do_not_inflate_prediction_misses() {
        let mut clock = FrameClock::new(Some(Duration::from_millis(10)), false);
        let target = Duration::from_secs(5);
        let mut stats = FrameStats::default();
        stats.presented(target + Duration::from_millis(20), target, &clock);
        clock.presented(target);
        stats.presented(target + Duration::from_millis(30), Duration::ZERO, &clock);
        let before_prediction = stats.snapshot("DP-1".into(), &clock, true);
        assert_eq!(before_prediction.presentations, 2);
        assert_eq!(before_prediction.fixed_refresh_presentations, 0);
        assert!(before_prediction.presentation_error_ms.is_none());

        stats.presented(target + Duration::from_millis(40), target, &clock);
        let with_prediction = stats.snapshot("DP-1".into(), &clock, true);
        assert_eq!(with_prediction.fixed_refresh_presentations, 1);
        assert_eq!(with_prediction.late_presentations, 1);
        assert_eq!(with_prediction.presentation_error_ms.unwrap().samples, 1);
    }

    #[test]
    fn empty_and_populated_snapshots_roundtrip_through_ipc() {
        let clock = FrameClock::new(None, false);
        let mut stats = FrameStats::default();
        for populated in [false, true] {
            if populated {
                stats.rendered(Duration::from_micros(123), &RenderResult::Submitted);
            }
            let snapshot = stats.snapshot("DP-1".into(), &clock, false);
            let response = realand_ipc::Response::FrameTimings(vec![snapshot.clone()]);
            let json = serde_json::to_string(&response).unwrap();
            let realand_ipc::Response::FrameTimings(parsed) = serde_json::from_str(&json).unwrap()
            else {
                panic!("unexpected response");
            };
            assert_eq!(parsed, vec![snapshot]);
        }
    }

    #[test]
    fn active_presentation_interval_is_separate_from_prediction_error_and_cpu_work() {
        let period = Duration::from_millis(10);
        let start = Duration::from_secs(1);
        let mut clock = FrameClock::new(Some(period), false);
        let mut stats = FrameStats::default();
        stats.rendered(Duration::from_millis(2), &RenderResult::Submitted);
        stats.presented_with_continuation(start, start, &clock, true);
        clock.presented(start);
        // A newly recomputed target can be accurate despite missing an animation refresh.
        stats.presented_with_continuation(start + period * 2, start + period * 2, &clock, false);
        clock.presented(start + period * 2);
        stats.presented_with_continuation(
            start + Duration::from_secs(30),
            Duration::ZERO,
            &clock,
            true,
        );
        let result = stats.snapshot("DP-1".into(), &clock, true);
        assert_eq!(result.continuous_active_intervals, 1);
        assert_eq!(result.continuous_active_long_intervals, 1);
        assert_eq!(result.continuous_active_interval_ms.unwrap().last, 20.);
        assert_eq!(result.presented_interval_ms.unwrap().samples, 2);
        assert_eq!(result.late_presentations, 0);
        assert_eq!(result.presentation_error_ms.unwrap().last, 0.);
        assert_eq!(result.cpu_frame_work_ms, result.render_time_ms);
        assert_eq!(result.cpu_frame_work_ms.unwrap().last, 2.);
        assert!(result.gpu_frame_work_ms.is_none());
    }

    #[test]
    fn timing_resets_and_skipped_rendering_do_not_count_idle_as_long_frames() {
        let period = Duration::from_millis(10);
        let start = Duration::from_secs(1);
        let mut clock = FrameClock::new(Some(period), false);
        let mut stats = FrameStats::default();
        stats.presented_with_continuation(start, start, &clock, true);
        clock.presented(start);
        stats.interrupt_presentation_continuity();
        stats.presented_with_continuation(
            start + Duration::from_secs(30),
            Duration::ZERO,
            &clock,
            true,
        );
        stats.reset_presentation_history();
        clock = FrameClock::new(Some(period), false);
        stats.presented_with_continuation(start, start, &clock, true);
        clock.set_vrr(true);
        stats.reset_presentation_history();
        stats.presented_with_continuation(start + period, start + period, &clock, true);
        stats.presented_with_continuation(
            start + Duration::from_secs(60),
            Duration::ZERO,
            &clock,
            true,
        );
        let result = stats.snapshot("DP-1".into(), &clock, true);
        assert_eq!(result.continuous_active_intervals, 0);
        assert_eq!(result.continuous_active_long_intervals, 0);
        assert!(result.continuous_active_interval_ms.is_none());
    }

    #[test]
    fn duplicate_feedback_does_not_count_presentation_or_error_twice() {
        let period = Duration::from_millis(10);
        let start = Duration::from_secs(1);
        let mut clock = FrameClock::new(Some(period), false);
        clock.presented(start - period);
        let mut stats = FrameStats::default();
        stats.presented_with_continuation(start, start, &clock, true);
        stats.presented_with_continuation(start, start + period, &clock, false);
        stats.presented_with_continuation(start - period, start, &clock, false);
        let result = stats.snapshot("DP-1".into(), &clock, true);
        assert_eq!(result.presentations, 1);
        assert_eq!(result.fixed_refresh_presentations, 1);
        assert_eq!(result.presentation_error_ms.unwrap().samples, 1);
    }

    #[test]
    fn raw_anomalies_are_bounded_ordered_and_cumulative_maxima_survive_ring_expiry() {
        let period = Duration::from_millis(10);
        let mut actual = Duration::from_secs(1);
        let mut clock = FrameClock::new(Some(period), false);
        clock.presented(actual - period);
        let mut stats = FrameStats::default();
        stats.rendered(Duration::from_secs(1), &RenderResult::Submitted);
        stats.presented_with_continuation(actual, actual, &clock, true);
        clock.presented(actual);
        actual += Duration::from_secs(1);
        stats.presented_with_continuation(actual, actual, &clock, true);
        clock.presented(actual);
        for _ in 0..=CAPACITY {
            stats.rendered(Duration::from_millis(1), &RenderResult::Submitted);
            actual += period * 2;
            stats.presented_with_continuation(actual, actual, &clock, true);
            clock.presented(actual);
        }
        let result = stats.snapshot("DP-1".into(), &clock, true);
        assert_eq!(result.render_time_ms.as_ref().unwrap().max, 1.);
        assert_eq!(result.cpu_frame_work_max_ms, Some(1000.));
        assert_eq!(result.cpu_frame_work_samples.len(), CAPACITY);
        assert_eq!(
            result.cpu_frame_work_samples.first().unwrap(),
            &CpuFrameSample {
                sequence: 3,
                wall_time_ms: 1.
            }
        );
        assert_eq!(
            result.cpu_frame_work_samples.last().unwrap(),
            &CpuFrameSample {
                sequence: result.redraws,
                wall_time_ms: 1.
            }
        );
        assert_eq!(
            result.continuous_active_interval_ms.as_ref().unwrap().max,
            20.
        );
        assert_eq!(result.continuous_active_interval_max_ms, Some(1000.));
        assert_eq!(result.continuous_active_long_intervals, 242);
        assert_eq!(
            result.continuous_active_long_interval_samples.len(),
            CAPACITY
        );
        assert_eq!(
            result.continuous_active_long_interval_samples_overwritten,
            2
        );
        assert_eq!(
            result
                .continuous_active_long_interval_samples
                .first()
                .unwrap()
                .sequence,
            3
        );
        let last = result
            .continuous_active_long_interval_samples
            .last()
            .unwrap();
        assert_eq!(last.sequence, 242);
        assert_eq!(last.presentation_time_ns, actual.as_nanos() as u64);
        assert_eq!(last.interval_ms, 20.);
        assert_eq!(last.refresh_interval_ms, 10.);
        assert_eq!(
            result.continuous_active_long_interval_samples,
            stats
                .snapshot("DP-1".into(), &clock, true)
                .continuous_active_long_interval_samples
        );
        let roundtrip: OutputFrameTimings =
            serde_json::from_str(&serde_json::to_string(&result).unwrap()).unwrap();
        assert_eq!(result, roundtrip);
        stats.reset_presentation_history();
        let after_reset = stats.snapshot("DP-1".into(), &clock, true);
        assert_eq!(after_reset.cpu_frame_work_max_ms, Some(1000.));
        assert_eq!(after_reset.continuous_active_interval_max_ms, Some(1000.));
        assert_eq!(after_reset.continuous_active_long_intervals, 242);
    }
}
