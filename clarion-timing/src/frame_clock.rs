//! Per-output presentation prediction and adaptive frame budgets.
//! Callers supply monotonic timestamps; this module owns no clocks or GPU state.

use std::num::NonZeroU64;
use std::time::Duration;

const MIN_LATCH_BUDGET: Duration = Duration::from_micros(250);
const MAX_INITIAL_LATCH_BUDGET: Duration = Duration::from_millis(2);
const RENDER_SAFETY_MARGIN: Duration = Duration::from_micros(250);
const MIN_PRESENTATION_MARGIN_STEP: Duration = Duration::from_micros(250);
const PRESENTATION_MARGIN_DECAY: Duration = Duration::from_micros(2000);
const ON_TIME_PRESENTATIONS_TO_DECAY: u32 = 24;
const DECAY_PROBE_CONFIRMATIONS: u8 = 8;
// Eight cheap frames may fall between expensive parts of a periodic workload.
// Keep rollback history for a wall-time horizon independent of refresh rate.
const DECAY_ROLLBACK_HISTORY: Duration = Duration::from_secs(1);
// Clarion policy: bound fresh-probe waiting while spacing failed probes enough
// to keep their deliberate misses infrequent. This is not a macOS-derived value.
const MAX_DECAY_PROBE_INTERVAL: Duration = Duration::from_secs(3);

#[derive(Debug)]
struct DecayProbe {
    // The first working reserve survives every reduction in this history.
    previous_margin: Duration,
    on_time: u8,
    last_reduction: Duration,
    // Operational changes are provisional too: undo them on a later miss.
    rollback_interval: u32,
    rollback_step: Duration,
    provisionally_accepted: bool,
}

#[derive(Debug)]
pub struct FrameClock {
    last_presentation_time: Option<Duration>,
    refresh_interval_ns: Option<NonZeroU64>,
    latch_budget: Duration,
    latch_delay_budget: Duration,
    presentation_margin: Duration,
    on_time_presentations: u32,
    decay_probe: Option<DecayProbe>,
    decay_interval: u32,
    decay_step: Duration,
    late_presentation_streak: u8,
    vrr: bool,
}

impl FrameClock {
    pub fn new(refresh_interval: Option<Duration>, vrr: bool) -> Self {
        let refresh_interval_ns = refresh_interval
            .and_then(|interval| u64::try_from(interval.as_nanos()).ok())
            .and_then(NonZeroU64::new);

        let latch_budget = refresh_interval_ns
            .map(|interval| Duration::from_nanos(interval.get()))
            .map(|interval| (interval / 4).min(MAX_INITIAL_LATCH_BUDGET))
            .unwrap_or(MAX_INITIAL_LATCH_BUDGET)
            .max(MIN_LATCH_BUDGET);

        Self {
            last_presentation_time: None,
            refresh_interval_ns,
            latch_budget,
            latch_delay_budget: Duration::ZERO,
            presentation_margin: Duration::ZERO,
            on_time_presentations: 0,
            decay_probe: None,
            decay_interval: ON_TIME_PRESENTATIONS_TO_DECAY,
            decay_step: PRESENTATION_MARGIN_DECAY,
            late_presentation_streak: 0,
            vrr,
        }
    }

    pub fn refresh_interval(&self) -> Option<Duration> {
        self.refresh_interval_ns
            .map(|r| Duration::from_nanos(r.get()))
    }

    /// A presentation target is meaningful only after a hardware timestamp established its grid.
    pub fn has_presentation_history(&self) -> bool {
        self.last_presentation_time.is_some()
    }

    pub fn set_vrr(&mut self, vrr: bool) {
        if self.vrr == vrr {
            return;
        }

        self.vrr = vrr;
        self.last_presentation_time = None;
        self.latch_delay_budget = Duration::ZERO;
        self.presentation_margin = Duration::ZERO;
        self.on_time_presentations = 0;
        self.late_presentation_streak = 0;
        self.decay_probe = None;
        self.decay_interval = ON_TIME_PRESENTATIONS_TO_DECAY;
        self.decay_step = PRESENTATION_MARGIN_DECAY;
    }

    pub fn vrr(&self) -> bool {
        self.vrr
    }

    /// Conservative time reserved for waking, rendering and queueing the next KMS frame.
    pub fn latch_budget(&self) -> Option<Duration> {
        self.refresh_interval().map(|interval| {
            let max_budget = interval
                .saturating_sub(MIN_LATCH_BUDGET)
                .max(MIN_LATCH_BUDGET);
            self.latch_budget
                .saturating_add(self.latch_delay_budget)
                .saturating_add(self.presentation_margin)
                .min(max_budget)
        })
    }

    /// Account for a timer or main-loop delay before rendering starts. A delayed
    /// redraw recomputes its presentation target, so hardware feedback alone can
    /// call that frame on time even when its original refresh was already missed.
    pub fn latch_started(&mut self, lateness: Duration) {
        let max_budget = self
            .refresh_interval()
            .unwrap_or(Duration::ZERO)
            .saturating_sub(MIN_LATCH_BUDGET);
        let sample = lateness.min(max_budget);
        if sample >= self.latch_delay_budget {
            self.latch_delay_budget = sample;
        } else {
            // Use the same conservative decay as CPU rendering. Immediate
            // redraws feed zero so an isolated stall cannot pin rendering to
            // the start of every refresh after it no longer needs a latch wait.
            self.latch_delay_budget -= (self.latch_delay_budget - sample) / 16;
        }
    }

    /// Feed a successful KMS submission back into the latch estimator.
    pub fn rendered(&mut self, elapsed: Duration) {
        let sample = elapsed.saturating_add(RENDER_SAFETY_MARGIN);
        let max_budget = self
            .refresh_interval()
            .map(|interval| interval.saturating_sub(MIN_LATCH_BUDGET))
            .unwrap_or(Duration::MAX)
            .max(MIN_LATCH_BUDGET);
        let sample = sample.clamp(MIN_LATCH_BUDGET, max_budget);

        if sample >= self.latch_budget {
            // Grow immediately after a slow frame so the next frame starts early enough.
            self.latch_budget = sample;
        } else {
            // Decay slowly to avoid one cheap direct-scanout frame erasing the budget needed
            // for a following composited frame.
            self.latch_budget -= (self.latch_budget - sample) / 16;
        }
    }

    pub fn presented(&mut self, presentation_time: Duration) {
        if self.record_presentation(presentation_time) {
            // Without the submitted frame's target, this feedback cannot extend
            // a run of consecutive misses. Keep the learned reserve, but begin
            // the next probe with its smallest step.
            self.late_presentation_streak = 0;
            self.cancel_decay_probe();
        }
    }

    fn cancel_decay_probe(&mut self) {
        if let Some(probe) = self.decay_probe.take() {
            self.presentation_margin = self.presentation_margin.max(probe.previous_margin);
            self.decay_interval = probe.rollback_interval;
            self.decay_step = probe.rollback_step;
        }
        self.on_time_presentations = 0;
    }

    fn record_presentation(&mut self, presentation_time: Duration) -> bool {
        if presentation_time.is_zero() {
            // Not interested in these.
            return false;
        }

        // DRM events normally arrive in order, but a delayed callback can carry an older
        // timestamp than the one we already used to establish the refresh grid.  Rewinding the
        // grid would make the next redraw target an already passed vblank and produce a short
        // frame.  Keep the newest timestamp until a mode/VRR change explicitly resets history.
        if self
            .last_presentation_time
            .is_none_or(|last| presentation_time > last)
        {
            self.last_presentation_time = Some(presentation_time);
            true
        } else {
            false
        }
    }

    /// Reserve more KMS queueing time after a missed fixed-refresh presentation. Render-time
    /// samples alone cannot account for driver and atomic-commit latency after rendering ends.
    pub fn presented_with_target(&mut self, actual: Duration, target: Duration) {
        // Stale or duplicate callbacks must not change the adaptive budget either. Counting
        // them as fresh feedback can inflate the margin or prematurely decay it.
        // The first frame after startup or a mode change has no reliable presentation grid.
        let had_prediction = self.has_presentation_history();
        if !self.record_presentation(actual) {
            return;
        }
        if !had_prediction || target.is_zero() || self.vrr {
            self.late_presentation_streak = 0;
            self.cancel_decay_probe();
            return;
        }
        let Some(interval) = self.refresh_interval() else {
            return;
        };

        if actual.saturating_sub(target) > interval / 2 {
            if let Some(probe) = self.decay_probe.take() {
                // A stable GPU tail is quantized to vblank, so on-time feedback
                // does not prove a cheaper workload. A failed reserve reduction
                // must restore its last working value instead of producing a
                // new burst of misses every 24 frames while learning it again.
                let max_margin = interval
                    .saturating_sub(MIN_LATCH_BUDGET)
                    .saturating_sub(self.latch_budget)
                    .saturating_sub(self.latch_delay_budget);
                self.presentation_margin = self
                    .presentation_margin
                    .max(probe.previous_margin)
                    .min(max_margin);
                let max_probe_frames = (MAX_DECAY_PROBE_INTERVAL.as_nanos() / interval.as_nanos())
                    .clamp(
                        u128::from(ON_TIME_PRESENTATIONS_TO_DECAY),
                        u128::from(u32::MAX),
                    ) as u32;
                self.decay_interval = probe
                    .rollback_interval
                    .saturating_mul(2)
                    .min(max_probe_frames);
                self.decay_step = (probe.rollback_step / 2).max(MIN_PRESENTATION_MARGIN_STEP);
                self.on_time_presentations = 0;
                self.late_presentation_streak = 0;
                return;
            }
            // CPU submission does not reveal how long GPU work and the atomic
            // commit take. Probe that reserve faster when consecutive frames
            // keep missing their refresh: a fixed increment can drop many frames
            // at the start of a GPU-heavy transition. One isolated miss still
            // uses the smallest step, and the effective budget stays below a
            // refresh. Fresh on-time feedback ends the probe streak.
            let step = (interval / 16)
                .max(MIN_PRESENTATION_MARGIN_STEP)
                .saturating_mul(1 << self.late_presentation_streak.min(2))
                // Limit the last probe's overshoot, preserving input time even
                // when a large tail first needs several consecutive probes.
                .min((interval / 4).max(MIN_PRESENTATION_MARGIN_STEP));
            // An asynchronous renderer measures CPU submission, not GPU completion.
            // A fixed 4 ms (or quarter-frame) cap leaves GPU-heavy frames permanently
            // late even when they could finish within one refresh. Let feedback use
            // the remaining frame budget, while preserving a minimum input window.
            // Subtract the CPU budget to avoid accumulating margin we cannot use;
            // otherwise a CPU overload would leave unnecessary latency afterwards.
            let max_margin = interval
                .saturating_sub(MIN_LATCH_BUDGET)
                .saturating_sub(self.latch_budget)
                .saturating_sub(self.latch_delay_budget);
            self.presentation_margin = self
                .presentation_margin
                .saturating_add(step)
                .min(max_margin);
            self.on_time_presentations = 0;
            self.late_presentation_streak = self.late_presentation_streak.saturating_add(1);
        } else {
            self.late_presentation_streak = 0;
            if let Some(probe) = self.decay_probe.as_mut() {
                probe.on_time = probe.on_time.saturating_add(1);
                if probe.on_time >= DECAY_PROBE_CONFIRMATIONS && !probe.provisionally_accepted {
                    // Permit another provisional reduction after eight on-time
                    // samples, so genuinely cheaper scenes shed stale latency.
                    // Keep the original reserve and operational settings: a later
                    // heavy frame must still be able to roll these changes back.
                    probe.provisionally_accepted = true;
                    self.decay_interval =
                        (self.decay_interval / 2).max(ON_TIME_PRESENTATIONS_TO_DECAY);
                    self.decay_step = self
                        .decay_step
                        .saturating_mul(2)
                        .min(PRESENTATION_MARGIN_DECAY);
                }
                // Only a full quiet history horizon commits the reductions.
                // Each new reduction below restarts that horizon.
                if probe.provisionally_accepted
                    && actual.saturating_sub(probe.last_reduction) >= DECAY_ROLLBACK_HISTORY
                {
                    self.decay_probe = None;
                }
            }
            self.on_time_presentations = self.on_time_presentations.saturating_add(1);
            // Backoff spaces new explorations. Reductions within an existing
            // rollback history can continue after each eight-sample success.
            let provisional_reduction = self
                .decay_probe
                .as_ref()
                .is_some_and(|probe| probe.provisionally_accepted);
            if provisional_reduction
                || (self.on_time_presentations >= self.decay_interval && self.decay_probe.is_none())
            {
                if !self.presentation_margin.is_zero() {
                    if let Some(probe) = self.decay_probe.as_mut() {
                        // Retain the earliest anchor, not the latest cheap pose.
                        probe.on_time = 0;
                        probe.last_reduction = actual;
                        probe.provisionally_accepted = false;
                    } else {
                        self.decay_probe = Some(DecayProbe {
                            previous_margin: self.presentation_margin,
                            on_time: 0,
                            last_reduction: actual,
                            rollback_interval: self.decay_interval,
                            rollback_step: self.decay_step,
                            provisionally_accepted: false,
                        });
                    }
                    self.presentation_margin =
                        self.presentation_margin.saturating_sub(self.decay_step);
                }
                self.on_time_presentations = 0;
            }
        }
    }

    pub fn next_latch_time_at(&self, now: Duration) -> Duration {
        let target = self.next_presentation_time_at(now);
        let budget = self.latch_budget().unwrap_or(self.latch_budget);
        target.saturating_sub(budget).max(now)
    }

    /// Predict using a shared timestamp so outputs can be compared without clock-read skew.
    pub fn next_presentation_time_at(&self, now: Duration) -> Duration {
        let Some(refresh_interval_ns) = self.refresh_interval_ns else {
            return now;
        };
        let Some(last_presentation_time) = self.last_presentation_time else {
            return now;
        };

        let refresh_interval_ns = refresh_interval_ns.get();

        if now <= last_presentation_time {
            // Early callbacks and exact-boundary timestamps must target the refresh after the
            // reported presentation, without accidentally skipping another whole refresh.
            return last_presentation_time
                .saturating_add(Duration::from_nanos(refresh_interval_ns));
        }

        let since_last = now - last_presentation_time;
        // Work with the remainder instead of multiplying the number of missed refreshes.
        // This also handles very long idle periods without overflowing nanoseconds.
        let remainder_ns = (since_last.as_nanos() % u128::from(refresh_interval_ns)) as u64;
        let to_next = Duration::from_nanos(refresh_interval_ns - remainder_ns);

        // If VRR is enabled and more than one frame passed since last presentation, assume that we
        // can present immediately.
        if self.vrr && since_last >= Duration::from_nanos(refresh_interval_ns) {
            now
        } else {
            now.saturating_add(to_next)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REFRESH: Duration = Duration::from_millis(10);
    const LAST: Duration = Duration::from_secs(1);

    fn clock(vrr: bool) -> FrameClock {
        let mut clock = FrameClock::new(Some(REFRESH), vrr);
        clock.presented(LAST);
        clock
    }

    #[test]
    fn fixed_refresh_stays_on_the_presentation_grid() {
        let clock = clock(false);
        for (elapsed, expected) in [(0, 10), (1, 10), (9, 10), (10, 20), (35, 40), (999, 1000)] {
            assert_eq!(
                clock.next_presentation_time_at(LAST + Duration::from_millis(elapsed)),
                LAST + Duration::from_millis(expected)
            );
        }
    }

    #[test]
    fn variable_refresh_only_waits_for_the_minimum_interval() {
        let clock = clock(true);
        assert_eq!(
            clock.next_presentation_time_at(LAST + REFRESH / 2),
            LAST + REFRESH
        );
        for elapsed in [REFRESH, REFRESH * 4, Duration::from_secs(3600)] {
            assert_eq!(
                clock.next_presentation_time_at(LAST + elapsed),
                LAST + elapsed
            );
        }
    }

    #[test]
    fn latch_deadline_reserves_an_adaptive_render_budget() {
        let mut clock = clock(false);
        let now = LAST + Duration::from_millis(1);
        assert_eq!(clock.latch_budget(), Some(Duration::from_millis(2)));
        assert_eq!(
            clock.next_latch_time_at(now),
            LAST + Duration::from_millis(8)
        );

        clock.rendered(Duration::from_millis(4));
        assert_eq!(clock.latch_budget(), Some(Duration::from_micros(4_250)));
        assert_eq!(
            clock.next_latch_time_at(now),
            LAST + Duration::from_micros(5_750)
        );

        clock.rendered(Duration::from_secs(1));
        assert_eq!(clock.latch_budget(), Some(REFRESH - MIN_LATCH_BUDGET));
        assert_eq!(clock.next_latch_time_at(LAST), LAST + MIN_LATCH_BUDGET);
    }

    #[test]
    fn late_latches_converge_even_when_rendering_retargets_the_following_refresh() {
        for hz in [60, 144] {
            let interval = Duration::from_nanos(1_000_000_000 / hz);
            let mut clock = FrameClock::new(Some(interval), false);
            clock.presented(LAST);
            let cpu = interval / 10;
            let scheduling_delay = interval / 3;
            for _ in 0..128 {
                clock.rendered(cpu);
            }

            let mut misses = 0;
            for frame in 0..32 {
                let now = clock.last_presentation_time.unwrap();
                let original_target = clock.next_presentation_time_at(now);
                let scheduled = clock.next_latch_time_at(now);
                let started = scheduled + scheduling_delay;
                clock.latch_started(scheduling_delay);
                // This is the redraw path's fresh prediction: after a late wake,
                // it can already name a different refresh from the timer's one.
                let render_target = clock.next_presentation_time_at(started);
                let actual =
                    clock.next_presentation_time_at(started + cpu - Duration::from_nanos(1));
                clock.rendered(cpu);
                clock.presented_with_target(actual, render_target);
                misses += usize::from(actual > original_target);
                if frame > 0 {
                    assert_eq!(actual, original_target, "{hz} Hz frame {frame}");
                }
            }
            assert_eq!(
                misses, 1,
                "{hz} Hz should recover after the first late latch"
            );
        }
    }

    #[test]
    fn scheduling_reserve_does_not_count_cpu_time_twice() {
        let mut clock = clock(false);
        clock.rendered(Duration::from_millis(2));
        clock.latch_started(Duration::from_millis(3));
        assert_eq!(clock.latch_budget(), Some(Duration::from_micros(5250)));
        clock.rendered(Duration::from_millis(2));
        clock.latch_started(Duration::from_millis(3));
        assert_eq!(clock.latch_budget(), Some(Duration::from_micros(5250)));
    }

    #[test]
    fn isolated_scheduling_stalls_are_bounded_and_decay_on_immediate_redraws() {
        for hz in [60, 144] {
            let interval = Duration::from_nanos(1_000_000_000 / hz);
            let mut clock = FrameClock::new(Some(interval), false);
            let initial = clock.latch_budget().unwrap();
            clock.latch_started(Duration::from_secs(1));
            assert_eq!(clock.latch_budget(), Some(interval - MIN_LATCH_BUDGET));
            for _ in 0..128 {
                clock.latch_started(Duration::ZERO);
            }
            assert!(clock.latch_budget().unwrap() < initial + MIN_LATCH_BUDGET);

            clock.latch_started(interval);
            clock.set_vrr(true);
            assert_eq!(clock.latch_budget(), Some(initial));
            clock.presented(LAST);
            assert_eq!(
                clock.next_latch_time_at(LAST + interval * 2),
                LAST + interval * 2
            );
            clock.set_vrr(false);
            assert_eq!(clock.latch_budget(), Some(initial));
        }
    }

    #[test]
    fn missed_presentations_move_latch_earlier_without_unbounded_latency() {
        let mut clock = clock(false);
        let target = LAST + REFRESH;
        clock.presented_with_target(target + REFRESH, target);
        assert_eq!(clock.latch_budget(), Some(Duration::from_micros(2_625)));
        assert_eq!(
            clock.next_latch_time_at(LAST + REFRESH * 2 + Duration::from_millis(1)),
            LAST + REFRESH * 3 - Duration::from_micros(2_625)
        );

        for i in 3..100 {
            let target = LAST + REFRESH * i;
            clock.presented_with_target(target + REFRESH, target);
        }
        assert_eq!(clock.latch_budget(), Some(Duration::from_micros(9_750)));

        for i in 101..101 + ON_TIME_PRESENTATIONS_TO_DECAY {
            let target = LAST + REFRESH * i;
            clock.presented_with_target(target, target);
        }
        assert_eq!(
            clock.latch_budget(),
            Some(Duration::from_micros(9_750) - PRESENTATION_MARGIN_DECAY)
        );
    }

    // Model the hardware presentation grid without sleeping or requiring DRM master.
    // CPU submission can finish well before the GPU and KMS are ready to present.
    fn simulate_frame(clock: &mut FrameClock, cpu: Duration, tail: Duration) -> bool {
        let now = clock.last_presentation_time.unwrap();
        let start = clock.next_latch_time_at(now);
        let target = clock.next_presentation_time_at(start);
        let completed = start + cpu + tail;
        let actual = clock.next_presentation_time_at(completed - Duration::from_nanos(1));
        clock.rendered(cpu);
        clock.presented_with_target(actual, target);
        actual > target
    }

    #[test]
    fn asynchronous_frames_converge_when_gpu_time_exceeds_the_old_margin_cap() {
        for hz in [60, 120, 144, 240, 360] {
            let interval = Duration::from_nanos(1_000_000_000 / hz);
            let mut clock = FrameClock::new(Some(interval), false);
            clock.presented(LAST);
            let cpu = interval / 10;
            let tail = interval * 3 / 5;
            // Establish the cheap CPU-submission cost before GPU load increases.
            for _ in 0..128 {
                clock.rendered(cpu);
            }
            let mut misses = 0;
            for frame in 0..48 {
                let late = simulate_frame(&mut clock, cpu, tail);
                misses += usize::from(late);
                if frame >= 32 {
                    assert!(
                        !late,
                        "{hz} Hz: still missing frame {frame}, budget {:?}",
                        clock.latch_budget()
                    );
                }
                assert!(clock.latch_budget().unwrap() < interval);
            }
            assert!(misses > 0, "exercise late-presentation feedback");
            assert!(misses < 16, "{hz} Hz: did not converge promptly");
        }
    }

    // Keep these regressions expressed through the production scheduler's
    // methods. The same harness can be applied to the previous source; a short
    // 24/48-frame convergence test misses its repeated reserve-decay stalls.
    #[test]
    fn sustained_gpu_load_does_not_relearn_its_reserve_every_24_frames() {
        for hz in [60, 120, 144, 240, 360] {
            let interval = Duration::from_nanos(1_000_000_000 / hz);
            let mut clock = FrameClock::new(Some(interval), false);
            clock.presented(LAST);
            let cpu = interval / 10;
            for _ in 0..128 {
                clock.rendered(cpu);
            }
            let mut misses = 0;
            for frame in 0..4096 {
                misses += usize::from(simulate_frame(&mut clock, cpu, interval * 3 / 5));
                assert!(clock.latch_budget().unwrap() < interval);
                if frame < 24 {
                    assert!(misses <= 4, "startup probe regressed at {hz} Hz");
                }
            }
            eprintln!("STEADY: {hz} Hz, 4096 frames, {misses} missed refreshes");
            assert!(
                misses <= 32,
                "{hz} Hz repeatedly lost its learned GPU reserve: {misses}/4096"
            );
        }
    }

    #[test]
    fn jittered_gpu_reserve_stays_bounded_and_a_lighter_workload_recovers_input_budget() {
        for hz in [60, 120, 144, 240, 360] {
            let interval = Duration::from_nanos(1_000_000_000 / hz);
            let mut clock = FrameClock::new(Some(interval), false);
            clock.presented(LAST);
            let cpu = interval / 10;
            for _ in 0..128 {
                clock.rendered(cpu);
            }
            let jitter = [11, 12, 13, 12, 11, 13, 12];
            let mut misses = 0;
            for frame in 0..4096 {
                misses += usize::from(simulate_frame(
                    &mut clock,
                    cpu,
                    interval * jitter[frame % jitter.len()] / 20,
                ));
                assert!(clock.latch_budget().unwrap() < interval);
            }
            assert!(
                misses <= 32,
                "{hz} Hz jittered load missed {misses}/4096 refreshes"
            );
            let loaded = clock.latch_budget().unwrap();
            let mut lighter_misses = 0;
            for _ in 0..8192 {
                lighter_misses += usize::from(simulate_frame(&mut clock, cpu, Duration::ZERO));
            }
            assert_eq!(
                lighter_misses, 0,
                "{hz} Hz reduced work must not lose refreshes"
            );
            assert!(
                clock.latch_budget().unwrap() < loaded / 2,
                "{hz} Hz retained stale input latency after load fell"
            );
            assert_eq!(clock.presentation_margin, Duration::ZERO);
            eprintln!("JITTER: {hz} Hz, 4096 frames, {misses} missed refreshes; lighter 8192 frames missed {lighter_misses}");
        }
    }

    #[test]
    fn failed_decay_probe_restores_known_good_reserve_and_timing_changes_clear_probe_state() {
        let mut clock = clock(false);
        // Use real presentation feedback to establish a reserve and begin its
        // first reduction probe, then make that reduced reserve miss.
        let target = LAST + REFRESH;
        clock.presented_with_target(target + REFRESH, target);
        let good = clock.presentation_margin;
        for _ in 0..ON_TIME_PRESENTATIONS_TO_DECAY {
            let target = clock.last_presentation_time.unwrap() + REFRESH;
            clock.presented_with_target(target, target);
        }
        assert!(clock.decay_probe.is_some());
        assert!(clock.presentation_margin < good);
        let target = clock.last_presentation_time.unwrap() + REFRESH;
        clock.presented_with_target(target + REFRESH, target);
        assert_eq!(clock.presentation_margin, good);
        assert_eq!(clock.decay_interval, ON_TIME_PRESENTATIONS_TO_DECAY * 2);
        assert_eq!(clock.decay_step, PRESENTATION_MARGIN_DECAY / 2);
        let max_probe_frames = (MAX_DECAY_PROBE_INTERVAL.as_nanos() / REFRESH.as_nanos()) as u32;
        // Repeated unsuccessful reductions may make probes less frequent, but
        // never prevent another probe or shrink its step below the declared
        // bound. Each failure restores its previous reserve in one feedback.
        for _ in 0..16 {
            let spacing = clock.decay_interval;
            for _ in 0..spacing {
                let target = clock.last_presentation_time.unwrap() + REFRESH;
                clock.presented_with_target(target, target);
            }
            assert!(clock.decay_probe.is_some());
            let target = clock.last_presentation_time.unwrap() + REFRESH;
            clock.presented_with_target(target + REFRESH, target);
            assert_eq!(clock.presentation_margin, good);
            assert!(clock.decay_interval <= max_probe_frames);
            assert!(clock.decay_step >= MIN_PRESENTATION_MARGIN_STEP);
        }
        assert_eq!(clock.decay_interval, max_probe_frames);
        assert_eq!(clock.decay_step, MIN_PRESENTATION_MARGIN_STEP);
        // VRR is a new presentation regime and must not inherit learned fixed
        // refresh decay probes or a stale GPU reserve.
        clock.set_vrr(true);
        assert!(clock.decay_probe.is_none());
        assert_eq!(clock.decay_interval, ON_TIME_PRESENTATIONS_TO_DECAY);
        assert_eq!(clock.decay_step, PRESENTATION_MARGIN_DECAY);
        assert_eq!(clock.presentation_margin, Duration::ZERO);
        clock.set_vrr(false);
        assert_eq!(clock.latch_budget(), Some(Duration::from_millis(2)));
    }

    #[test]
    fn abrupt_gpu_load_recovers_within_four_missed_refreshes() {
        for hz in [60, 120, 144, 240, 360] {
            let interval = Duration::from_nanos(1_000_000_000 / hz);
            let mut clock = FrameClock::new(Some(interval), false);
            clock.presented(LAST);
            let cpu = interval / 10;
            for _ in 0..128 {
                clock.rendered(cpu);
            }

            // A cheap direct-scanout/composited workload suddenly gains a GPU
            // tail. Quantized presentation feedback supplies no exact GPU time;
            // the scheduler must discover a sufficient reserve from misses.
            let mut misses = 0;
            for frame in 0..24 {
                let late = simulate_frame(&mut clock, cpu, interval * 3 / 5);
                misses += usize::from(late);
                if frame >= 4 {
                    assert!(!late, "{hz} Hz frame {frame} still missed its refresh");
                }
                assert!(clock.latch_budget().unwrap() < interval);
            }
            assert!((1..=4).contains(&misses), "{hz} Hz missed {misses} frames");
        }
    }

    #[test]
    fn on_time_feedback_ends_the_late_presentation_probe() {
        let mut clock = clock(false);
        for i in 1..=3 {
            let target = LAST + REFRESH * (i * 2 - 1);
            clock.presented_with_target(target + REFRESH, target);
        }
        assert_eq!(clock.late_presentation_streak, 3);

        let on_time = LAST + REFRESH * 7;
        clock.presented_with_target(on_time, on_time);
        assert_eq!(clock.late_presentation_streak, 0);
        let previous = clock.presentation_margin;
        let target = on_time + REFRESH;
        clock.presented_with_target(target + REFRESH, target);
        assert_eq!(clock.presentation_margin - previous, REFRESH / 16);

        clock.set_vrr(true);
        assert_eq!(clock.late_presentation_streak, 0);
    }

    #[test]
    fn an_isolated_gpu_stall_does_not_leave_the_probe_budget_raised() {
        let mut clock = clock(false);
        let cpu = REFRESH / 10;
        for _ in 0..128 {
            clock.rendered(cpu);
        }
        let baseline = clock.latch_budget().unwrap();
        assert!(simulate_frame(&mut clock, cpu, REFRESH * 3 / 4));
        assert!(clock.latch_budget().unwrap() > baseline);

        for _ in 0..512 {
            assert!(!simulate_frame(&mut clock, cpu, Duration::ZERO));
        }
        assert_eq!(clock.presentation_margin, Duration::ZERO);
        assert_eq!(clock.late_presentation_streak, 0);
        // CPU estimates may continue to decay by a few nanoseconds while the
        // presentation reserve returns fully to zero.
        assert!(clock.latch_budget().unwrap() <= baseline);
    }

    #[test]
    fn untargeted_fresh_feedback_ends_a_probe_but_duplicates_do_not() {
        let mut clock = clock(false);
        let target = LAST + REFRESH;
        let late = target + REFRESH;
        clock.presented_with_target(late, target);
        assert_eq!(clock.late_presentation_streak, 1);
        clock.presented(late);
        clock.presented(Duration::ZERO);
        assert_eq!(clock.late_presentation_streak, 1);

        clock.presented(late + REFRESH);
        assert_eq!(clock.late_presentation_streak, 0);
        let target = late + REFRESH * 2;
        clock.presented_with_target(target + REFRESH, target);
        assert_eq!(clock.late_presentation_streak, 1);
        clock.presented_with_target(target + REFRESH * 2, Duration::ZERO);
        assert_eq!(clock.late_presentation_streak, 0);
    }

    #[test]
    fn asynchronous_budget_recovers_after_gpu_load_subsides() {
        let mut clock = FrameClock::new(Some(Duration::from_nanos(16_666_667)), false);
        clock.presented(LAST);
        let cpu = Duration::from_millis(1);
        for _ in 0..48 {
            simulate_frame(&mut clock, cpu, Duration::from_millis(8));
        }
        let loaded = clock.latch_budget().unwrap();
        assert!(loaded >= Duration::from_millis(9));

        for _ in 0..8192 {
            simulate_frame(&mut clock, cpu, Duration::ZERO);
        }
        assert!(clock.latch_budget().unwrap() < loaded / 2);
        assert_eq!(clock.presentation_margin, Duration::ZERO);
    }

    #[test]
    fn overloaded_frames_do_not_accumulate_unusable_presentation_margin() {
        let mut clock = clock(false);
        for _ in 0..64 {
            simulate_frame(&mut clock, REFRESH * 2, REFRESH);
        }
        assert_eq!(clock.latch_budget(), Some(REFRESH - MIN_LATCH_BUDGET));
        assert_eq!(clock.presentation_margin, Duration::ZERO);
    }

    #[test]
    fn presentation_feedback_ignores_unprimed_clocks_and_variable_refresh() {
        let mut first_frame = FrameClock::new(Some(REFRESH), false);
        first_frame.presented_with_target(LAST + REFRESH, LAST);
        assert_eq!(first_frame.latch_budget(), Some(Duration::from_millis(2)));

        let mut variable = clock(true);
        variable.presented_with_target(LAST + REFRESH * 2, LAST + REFRESH);
        assert_eq!(variable.latch_budget(), Some(Duration::from_millis(2)));

        let mut fixed = clock(false);
        fixed.presented_with_target(LAST + REFRESH * 2, LAST + REFRESH);
        fixed.set_vrr(true);
        fixed.set_vrr(false);
        assert_eq!(fixed.latch_budget(), Some(Duration::from_millis(2)));
    }

    #[test]
    fn variable_refresh_bypasses_latch_delay_after_idle() {
        let clock = clock(true);
        let before_minimum = LAST + REFRESH / 2;
        assert_eq!(
            clock.next_latch_time_at(before_minimum),
            LAST + REFRESH - Duration::from_millis(2)
        );

        let after_idle = LAST + REFRESH * 4;
        assert_eq!(clock.next_latch_time_at(after_idle), after_idle);
    }

    #[test]
    fn missing_timing_and_vrr_changes_do_not_reuse_old_predictions() {
        let mut clock = FrameClock::new(Some(REFRESH), false);
        clock.presented(Duration::ZERO);
        assert_eq!(clock.next_presentation_time_at(LAST), LAST);
        clock.presented(LAST);
        clock.set_vrr(true);
        assert_eq!(clock.next_presentation_time_at(LAST), LAST);
        clock.presented(LAST);
        clock.set_vrr(true);
        assert_eq!(
            clock.next_presentation_time_at(LAST + REFRESH / 2),
            LAST + REFRESH
        );
        assert_eq!(
            FrameClock::new(None, false).next_presentation_time_at(LAST),
            LAST
        );
    }

    #[test]
    fn early_vblank_predictions_remain_after_now() {
        let clock = clock(false);
        assert_eq!(
            clock.next_presentation_time_at(LAST - REFRESH / 2),
            LAST + REFRESH
        );
        assert!(clock.next_presentation_time_at(LAST - REFRESH * 3) > LAST);
    }

    #[test]
    fn stale_presentation_timestamps_do_not_rewind_the_refresh_grid() {
        let mut clock = clock(false);
        clock.presented(LAST + REFRESH * 2);
        clock.presented(LAST + REFRESH);

        assert_eq!(
            clock.next_presentation_time_at(LAST + REFRESH * 2),
            LAST + REFRESH * 3
        );
    }

    #[test]
    fn stale_and_duplicate_feedback_does_not_adjust_latch_budget() {
        let mut clock = clock(false);
        clock.presented_with_target(LAST + REFRESH * 2, LAST + REFRESH);
        let budget = clock.latch_budget();
        let on_time = clock.on_time_presentations;
        let late_streak = clock.late_presentation_streak;
        for _ in 0..ON_TIME_PRESENTATIONS_TO_DECAY {
            // Duplicate late feedback must not repeatedly inflate the margin.
            clock.presented_with_target(LAST + REFRESH * 2, LAST + REFRESH);
            // An older on-time callback must not decay the margin either.
            clock.presented_with_target(LAST + REFRESH, LAST + REFRESH);
        }
        assert_eq!(clock.latch_budget(), budget);
        assert_eq!(clock.on_time_presentations, on_time);
        assert_eq!(clock.late_presentation_streak, late_streak);
        assert_eq!(clock.last_presentation_time, Some(LAST + REFRESH * 2));
    }

    #[test]
    fn long_idle_time_does_not_overflow() {
        let now = Duration::from_secs(u64::MAX / 2);
        assert!(clock(false).next_presentation_time_at(now) > now);
        assert_eq!(clock(true).next_presentation_time_at(now), now);
    }

    #[test]
    fn invalid_or_slow_refresh_intervals_do_not_panic() {
        let now = Duration::from_secs(1);
        let invalid = FrameClock::new(Some(Duration::ZERO), false);
        assert_eq!(invalid.refresh_interval(), None);
        assert_eq!(invalid.next_presentation_time_at(now), now);

        let slow_interval = Duration::from_secs(2);
        let mut slow = FrameClock::new(Some(slow_interval), false);
        slow.presented(now);
        assert_eq!(slow.refresh_interval(), Some(slow_interval));
        assert_eq!(
            slow.next_presentation_time_at(now + Duration::from_millis(1)),
            now + slow_interval
        );
    }
    /// Synthetic CPU/GPU/KMS tails exercise the real FrameClock and the existing
    /// presentation-grid harness. They do not measure a physical GPU or desktop.
    fn assert_cheap_scene_recovers(
        clock: &mut FrameClock,
        cpu: Duration,
        refresh: Duration,
        limit: Duration,
    ) {
        let started = clock.last_presentation_time.unwrap();
        let mut recovered = None;
        for _ in 0..8192 {
            assert!(
                !simulate_frame(clock, cpu, Duration::ZERO),
                "cheaper work missed a refresh"
            );
            if recovered.is_none() && clock.latch_budget().unwrap() <= limit {
                recovered = Some(clock.last_presentation_time.unwrap() - started);
            }
        }
        // Fresh-probe backoff plus bounded eight-sample provisional reductions.
        // Eighty refreshes cover a full-frame reserve at the tested 60+ Hz rates.
        let bound = MAX_DECAY_PROBE_INTERVAL + refresh * 80;
        assert!(
            recovered.is_some_and(|elapsed| elapsed <= bound),
            "cheap-scene budget recovery {recovered:?} exceeded {bound:?}"
        );
    }

    #[test]
    fn periodic_gpu_tails_keep_misses_below_budget_and_recover() {
        for hz in [60u32, 120, 144] {
            let refresh = Duration::from_nanos(1_000_000_000 / u64::from(hz));
            // At 60 Hz: CPU 1 ms, heavy tail 8 ms, light tail 1 ms.
            let cpu = refresh * 6 / 100;
            let heavy = refresh * 48 / 100;
            let light = refresh * 6 / 100;
            for frames in [4096u32, 8192] {
                for period in [19u32, 31, 47] {
                    let mut clock = FrameClock::new(Some(refresh), false);
                    clock.presented(LAST);
                    for _ in 0..128 {
                        clock.rendered(cpu);
                    }
                    let mut postwarm_misses = 0;
                    for frame in 0..frames {
                        let tail = if frame % period < period * 6 / 19 {
                            heavy
                        } else {
                            light
                        };
                        let late = simulate_frame(&mut clock, cpu, tail);
                        postwarm_misses += u32::from(late && frame >= 256);
                    }
                    assert!(
                        postwarm_misses * 100 < frames - 256,
                        "{hz} Hz, period {period}: {postwarm_misses}/{} postwarm misses",
                        frames - 256
                    );
                    assert_cheap_scene_recovers(&mut clock, cpu, refresh, cpu * 2);
                }
            }
        }
    }

    #[test]
    fn time_based_and_random_gpu_tails_cover_high_refresh_and_idle() {
        for hz in [60u32, 120, 144, 240, 360] {
            let refresh = Duration::from_nanos(1_000_000_000 / u64::from(hz));
            let cpu = refresh * 6 / 100;
            let heavy = refresh * 48 / 100;
            let light = refresh * 6 / 100;
            // Include the fixed safety reserve: 2*CPU alone is below the steady
            // budget floor at 240/360 Hz, even for the original implementation.
            let recovery_limit = (cpu + RENDER_SAFETY_MARGIN) * 2;
            for profile in 0..6 {
                let mut clock = FrameClock::new(Some(refresh), false);
                clock.presented(LAST);
                for _ in 0..2048 {
                    simulate_frame(&mut clock, cpu, heavy);
                }
                let started = clock.last_presentation_time.unwrap();
                let mut random = 0x71a9_e23d_u64;
                let mut segment_end = Duration::ZERO;
                let mut heavy_segment = false;
                let mut random_tail = light;
                let mut misses = 0;
                for frame in 0..8192 {
                    let elapsed = clock.last_presentation_time.unwrap() - started;
                    let phase = |period: u128| elapsed.as_nanos() % period;
                    let tail = match profile {
                        0 => {
                            if frame % 19 < 6 {
                                heavy
                            } else {
                                light
                            }
                        }
                        1 => {
                            if phase(323_000_000) < 102_000_000 {
                                heavy
                            } else {
                                light
                            }
                        }
                        2 => {
                            if phase(731_000_000) < 231_000_000 {
                                heavy
                            } else {
                                light
                            }
                        }
                        3 => {
                            if phase(1_613_000_000) < 509_000_000 {
                                heavy
                            } else {
                                light
                            }
                        }
                        4 => {
                            if phase(3_000_000_000) < 900_000_000 {
                                heavy
                            } else {
                                light
                            }
                        }
                        5 => {
                            if elapsed >= segment_end {
                                random = random
                                    .wrapping_mul(6364136223846793005)
                                    .wrapping_add(1442695040888963407);
                                heavy_segment = !heavy_segment;
                                let ms = if heavy_segment {
                                    50 + (random >> 32) % 170
                                } else {
                                    100 + (random >> 32) % 850
                                };
                                segment_end = elapsed + Duration::from_millis(ms);
                                random_tail = if heavy_segment {
                                    heavy + refresh * ((random >> 40) % 5) as u32 / 100
                                } else {
                                    light
                                };
                            }
                            random_tail
                        }
                        _ => unreachable!(),
                    };
                    misses += u32::from(simulate_frame(&mut clock, cpu, tail));
                }
                assert!(
                    misses * 100 < 8192,
                    "{hz} Hz profile {profile}: {misses}/8192 misses"
                );
                assert_cheap_scene_recovers(&mut clock, cpu, refresh, recovery_limit);
            }
            let mut clock = FrameClock::new(Some(refresh), false);
            clock.presented(LAST);
            for _ in 0..2048 {
                simulate_frame(&mut clock, cpu, heavy);
            }
            let after_idle = clock.last_presentation_time.unwrap() + Duration::from_secs(60);
            let target = clock.next_presentation_time_at(after_idle);
            assert!(target > after_idle);
            clock.presented_with_target(target, target);
            assert_cheap_scene_recovers(&mut clock, cpu, refresh, recovery_limit);
        }
    }

    #[test]
    fn rollback_history_restores_budget_and_operational_settings() {
        let mut clock = clock(false);
        clock.presentation_margin = Duration::from_millis(4);
        clock.decay_interval = 128;
        clock.decay_step = Duration::from_micros(250);
        clock.decay_probe = Some(DecayProbe {
            previous_margin: Duration::from_millis(6),
            on_time: 0,
            last_reduction: LAST,
            rollback_interval: 128,
            rollback_step: Duration::from_micros(250),
            provisionally_accepted: false,
        });
        for _ in 0..DECAY_PROBE_CONFIRMATIONS {
            let target = clock.last_presentation_time.unwrap() + REFRESH;
            clock.presented_with_target(target, target);
        }
        let probe = clock
            .decay_probe
            .as_ref()
            .expect("eight cheap frames must retain history");
        assert_eq!(probe.previous_margin, Duration::from_millis(6));
        assert_eq!(probe.rollback_interval, 128);
        let target = clock.last_presentation_time.unwrap() + REFRESH;
        clock.presented_with_target(target + REFRESH, target);
        assert_eq!(clock.presentation_margin, Duration::from_millis(6));
        assert_eq!(clock.decay_interval, 256);
        assert_eq!(clock.decay_step, MIN_PRESENTATION_MARGIN_STEP);

        // Fresh feedback with no submitted target cannot validate provisional
        // operational changes. Restore their saved settings without a miss.
        clock.presentation_margin = Duration::from_millis(2);
        clock.decay_interval = 24;
        clock.decay_step = Duration::from_millis(2);
        clock.decay_probe = Some(DecayProbe {
            previous_margin: Duration::from_millis(6),
            on_time: 8,
            last_reduction: clock.last_presentation_time.unwrap(),
            rollback_interval: 128,
            rollback_step: Duration::from_micros(250),
            provisionally_accepted: true,
        });
        clock.presented(clock.last_presentation_time.unwrap() + REFRESH);
        assert!(clock.decay_probe.is_none());
        assert_eq!(clock.presentation_margin, Duration::from_millis(6));
        assert_eq!(clock.decay_interval, 128);
        assert_eq!(clock.decay_step, Duration::from_micros(250));
    }

    #[test]
    fn phase_offsets_and_long_quiet_bursts_do_not_hide_reserve_relearning() {
        for hz in [60u32, 120, 144, 240, 360] {
            let refresh = Duration::from_nanos(1_000_000_000 / u64::from(hz));
            let cpu = refresh * 6 / 100;
            let heavy = refresh * 48 / 100;
            let light = refresh * 6 / 100;
            for period_ms in [1200u64, 2000, 3000, 4500] {
                for offset_percent in [13u32, 37, 61, 89] {
                    let period = Duration::from_millis(period_ms);
                    let offset = period * offset_percent / 100;
                    let mut clock = FrameClock::new(Some(refresh), false);
                    clock.presented(LAST);
                    for _ in 0..2048 {
                        simulate_frame(&mut clock, cpu, heavy);
                    }
                    let started = clock.last_presentation_time.unwrap();
                    let mut misses = 0;
                    for _ in 0..8192 {
                        let phase = (clock.last_presentation_time.unwrap() - started + offset)
                            .as_nanos()
                            % period.as_nanos();
                        let tail = if phase < period.as_nanos() * 3 / 10 {
                            heavy
                        } else {
                            light
                        };
                        misses += u32::from(simulate_frame(&mut clock, cpu, tail));
                    }
                    assert!(
                        misses * 100 < 8192,
                        "{hz} Hz period {period_ms} offset {offset_percent}: {misses}/8192 misses"
                    );
                }
            }
            let mut clock = FrameClock::new(Some(refresh), false);
            clock.presented(LAST);
            for _ in 0..2048 {
                simulate_frame(&mut clock, cpu, heavy);
            }
            let started = clock.last_presentation_time.unwrap();
            let mut random = 0x61df_221a_u64;
            let mut segment_end = Duration::ZERO;
            let mut heavy_segment = false;
            let mut tail = light;
            let mut misses = 0;
            for _ in 0..32768 {
                let elapsed = clock.last_presentation_time.unwrap() - started;
                if elapsed >= segment_end {
                    random = random
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    heavy_segment = !heavy_segment;
                    let ms = if heavy_segment {
                        100 + (random >> 32) % 500
                    } else {
                        100 + (random >> 32) % 2500
                    };
                    segment_end = elapsed + Duration::from_millis(ms);
                    tail = if heavy_segment { heavy } else { light };
                }
                misses += u32::from(simulate_frame(&mut clock, cpu, tail));
            }
            assert!(
                misses * 100 < 32768,
                "{hz} Hz long-quiet random: {misses}/32768 misses"
            );
        }
    }
}
