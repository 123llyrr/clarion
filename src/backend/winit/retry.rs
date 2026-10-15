//! Pace nested presentation readiness/failures without polling in a tight loop.

use std::time::{Duration, Instant};

const MAX_DELAY: Duration = Duration::from_millis(250);
const WARNING_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Default)]
pub(super) struct Retry {
    attempts: u32,
    failed: bool,
    deadline: Option<Instant>,
    last_warning: Option<Instant>,
}

impl Retry {
    pub fn waiting(&self, now: Instant) -> bool {
        self.deadline.is_some_and(|deadline| deadline > now)
    }

    pub fn pending(&self) -> bool {
        self.deadline.is_some() && !self.failed
    }

    /// Existing wakeups keep their first deadline despite input/commit bursts.
    pub fn defer(&mut self, now: Instant, interval: Duration, failed: bool) -> Instant {
        if let Some(deadline) = self.deadline.filter(|deadline| *deadline > now) {
            return deadline;
        }
        if self.failed != failed {
            self.attempts = 0;
            self.failed = failed;
        }
        let interval = interval.clamp(Duration::from_millis(1), MAX_DELAY);
        // Ordinary FIFO/image backpressure must resume promptly: quarter-frame
        // checks grow only to one refresh. Real failures use a longer backoff.
        let (base, cap) = if failed {
            (interval, MAX_DELAY)
        } else {
            (
                (interval / 4).clamp(Duration::from_millis(1), Duration::from_millis(4)),
                interval,
            )
        };
        let delay = base.saturating_mul(1 << self.attempts.min(8)).min(cap);
        self.attempts = self.attempts.saturating_add(1);
        let deadline = now.checked_add(delay).unwrap_or(now);
        self.deadline = Some(deadline);
        deadline
    }

    pub fn should_warn(&mut self, now: Instant) -> bool {
        if self
            .last_warning
            .is_some_and(|last| now.saturating_duration_since(last) < WARNING_INTERVAL)
        {
            return false;
        }
        self.last_warning = Some(now);
        true
    }

    pub fn recovered(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_readiness_timeouts_are_bounded_and_do_not_starve_recovery() {
        let start = Instant::now();
        let mut retry = Retry::default();
        let interval = Duration::from_micros(16_667);
        let mut now = start;
        let mut attempts = 0;
        while now < start + Duration::from_secs(5) {
            attempts += 1;
            let deadline = retry.defer(now, interval, true);
            assert!(deadline > now);
            assert!(deadline - now <= MAX_DELAY);
            // Many commits before the existing deadline cannot postpone it or
            // authorize another GPU/acquire query.
            for offset in [
                Duration::ZERO,
                Duration::from_micros(1),
                (deadline - now) / 2,
            ] {
                let early = now + offset;
                assert!(retry.waiting(early));
                assert_eq!(retry.defer(early, interval, true), deadline);
            }
            assert!(!retry.waiting(deadline));
            now = deadline;
        }
        assert!(attempts <= 24, "{attempts} attempts in five seconds");
        retry.recovered();
        assert!(!retry.waiting(now));
        assert_eq!(retry.defer(now, interval, true), now + interval);
    }

    #[test]
    fn ordinary_backpressure_stays_within_one_refresh_and_recovers_promptly() {
        for interval in [
            Duration::from_micros(6_944),
            Duration::from_micros(16_667),
            Duration::from_millis(40),
        ] {
            let start = Instant::now();
            let mut retry = Retry::default();
            let base = (interval / 4).clamp(Duration::from_millis(1), Duration::from_millis(4));
            let mut now = start;
            for _ in 0..500 {
                let deadline = retry.defer(now, interval, false);
                assert!(deadline - now >= Duration::from_millis(1));
                assert!(deadline - now <= interval);
                assert_eq!(retry.defer(now, interval, false), deadline);
                now = deadline;
            }
            retry.recovered();
            assert_eq!(retry.defer(now, interval, false), now + base);
            // A real failure uses its own backoff, not the saturated pending
            // attempt count; host readiness never inherits a 250ms error delay.
            now += interval;
            assert_eq!(retry.defer(now, interval, true), now + interval);
            now += interval;
            assert_eq!(retry.defer(now, interval, false), now + base);
        }
    }

    #[test]
    fn lock_completion_distinguishes_transient_waits_from_failure_backoff() {
        let mut retry = Retry::default();
        let now = Instant::now();
        let interval = Duration::from_micros(16_667);
        let deadline = retry.defer(now, interval, false);
        assert!(retry.waiting(now) && retry.pending());
        let deadline = retry.defer(deadline, interval, true);
        assert!(retry.waiting(deadline - Duration::from_micros(1)) && !retry.pending());
        retry.recovered();
        assert!(!retry.pending());
    }

    #[test]
    fn warning_frequency_and_recovery_are_independent_of_commit_bursts() {
        let now = Instant::now();
        let mut retry = Retry::default();
        assert!(retry.should_warn(now));
        for millis in 1..5000 {
            assert!(!retry.should_warn(now + Duration::from_millis(millis)));
        }
        assert!(retry.should_warn(now + WARNING_INTERVAL));
        retry.recovered();
        assert!(retry.should_warn(now + WARNING_INTERVAL));
    }
}
