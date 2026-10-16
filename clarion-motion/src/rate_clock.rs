//! Rate integration driven by explicit raw timestamps.
//!
//! Clock sourcing and event-loop time caching belong to the platform adapter.

use std::time::Duration;

#[derive(Debug, Clone)]
pub struct RateClock {
    current_time: Duration,
    last_seen_time: Duration,
    rate: f64,
    complete_instantly: bool,
}

impl RateClock {
    pub fn new(time: Duration) -> Self {
        Self {
            current_time: time,
            last_seen_time: time,
            rate: 1.,
            complete_instantly: false,
        }
    }

    pub fn rate(&self) -> f64 {
        self.rate
    }

    pub fn set_rate_at(&mut self, time: Duration, rate: f64) {
        // Apply all elapsed time using the old rate before changing it.  Otherwise a
        // config reload between two clock reads would retroactively scale the whole
        // interval since `last_seen_time`, which can make an animation jump backwards
        // (for example, changing slowdown from 1 to 2 while the compositor is idle).
        self.advance_to(time);
        // `Duration::mul_f64` rejects NaN.  A malformed config or an external
        // caller must not be able to poison the shared animation clock and make
        // the next frame panic, so fall back to the normal real-time rate.
        self.rate = if rate.is_finite() {
            rate.clamp(0., 1000.)
        } else {
            1.
        };
    }

    pub fn should_complete_instantly(&self) -> bool {
        self.complete_instantly
    }

    pub fn set_complete_instantly(&mut self, value: bool) {
        self.complete_instantly = value;
    }

    pub fn advance_to(&mut self, time: Duration) -> Duration {
        if self.last_seen_time == time {
            return self.current_time;
        }

        if self.last_seen_time < time {
            let delta = time - self.last_seen_time;
            let delta = self.scale_delta(delta);
            self.current_time = self.current_time.saturating_add(delta);
        } else {
            let delta = self.last_seen_time - time;
            let delta = self.scale_delta(delta);
            self.current_time = self.current_time.saturating_sub(delta);
        }

        self.last_seen_time = time;
        self.current_time
    }

    fn scale_delta(&self, delta: Duration) -> Duration {
        // Preserve Duration::mul_f64 rounding while also saturating a scaled
        // interval that overflows, before the saturating add/sub below.
        Duration::try_from_secs_f64(delta.as_secs_f64() * self.rate).unwrap_or(Duration::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_change_integrates_elapsed_time_using_the_old_rate() {
        let mut clock = RateClock::new(Duration::ZERO);
        clock.set_rate_at(Duration::from_millis(100), 0.5);
        assert_eq!(
            clock.advance_to(Duration::from_millis(100)),
            Duration::from_millis(100)
        );
        assert_eq!(
            clock.advance_to(Duration::from_millis(200)),
            Duration::from_millis(150)
        );
        clock.set_rate_at(Duration::from_millis(300), 2.);
        assert_eq!(
            clock.advance_to(Duration::from_millis(400)),
            Duration::from_millis(400)
        );
    }

    #[test]
    fn replay_can_move_backwards_and_repeated_sampling_is_stable() {
        let mut clock = RateClock::new(Duration::from_secs(1));
        clock.set_rate_at(Duration::from_secs(1), 0.5);
        assert_eq!(
            clock.advance_to(Duration::from_secs(2)),
            Duration::from_millis(1500)
        );
        assert_eq!(
            clock.advance_to(Duration::from_secs(2)),
            Duration::from_millis(1500)
        );
        assert_eq!(clock.advance_to(Duration::ZERO), Duration::from_millis(500));
        clock.set_rate_at(Duration::ZERO, 0.);
        assert_eq!(
            clock.advance_to(Duration::from_secs(3)),
            Duration::from_millis(500)
        );
    }

    #[test]
    fn malformed_rates_recover_without_poisoning_the_clock() {
        for rate in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut clock = RateClock::new(Duration::ZERO);
            clock.set_rate_at(Duration::ZERO, rate);
            assert_eq!(clock.rate(), 1.);
            assert_eq!(
                clock.advance_to(Duration::from_secs(1)),
                Duration::from_secs(1)
            );
        }
        let mut clock = RateClock::new(Duration::ZERO);
        clock.set_rate_at(Duration::ZERO, -1.);
        assert_eq!(clock.rate(), 0.);
        clock.set_rate_at(Duration::ZERO, 1001.);
        assert_eq!(clock.rate(), 1000.);
    }

    #[test]
    fn scaled_duration_overflow_saturates_in_both_directions() {
        let mut clock = RateClock::new(Duration::ZERO);
        clock.set_rate_at(Duration::ZERO, 1000.);
        assert_eq!(clock.advance_to(Duration::MAX), Duration::MAX);
        assert_eq!(clock.advance_to(Duration::ZERO), Duration::ZERO);
        assert_eq!(
            clock.advance_to(Duration::from_millis(1)),
            Duration::from_secs(1)
        );
    }
}
