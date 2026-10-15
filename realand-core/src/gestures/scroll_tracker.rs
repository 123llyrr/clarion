pub struct ScrollTracker {
    tick: f64,
    last: f64,
    acc: f64,
}

impl ScrollTracker {
    #[allow(clippy::new_without_default)]
    pub fn new(tick: i8) -> Self {
        Self {
            tick: f64::from(tick.unsigned_abs().max(1)),
            last: 0.,
            acc: 0.,
        }
    }

    pub fn accumulate(&mut self, amount: f64) -> i8 {
        if !amount.is_finite() || !(self.acc + amount).is_finite() {
            return 0;
        }
        // An axis stop or an event for the other axis must not erase direction.
        if amount == 0. {
            return 0;
        }

        let changed_direction = (self.last > 0. && amount < 0.) || (self.last < 0. && amount > 0.);
        if changed_direction {
            self.acc = 0.
        }

        self.last = amount;
        self.acc += amount;

        let mut ticks = 0;
        if self.acc.abs() >= self.tick {
            let clamped = self.acc.clamp(-127. * self.tick, 127. * self.tick);
            ticks = (clamped as i16 / self.tick as i16) as i8;
            self.acc %= self.tick;
        }

        ticks
    }

    pub fn reset(&mut self) {
        self.last = 0.;
        self.acc = 0.;
    }
}

#[cfg(test)]
mod tests {
    use super::ScrollTracker;

    #[test]
    fn reversing_after_zero_samples_discards_the_previous_direction_remainder() {
        for tick in [10, 120] {
            for direction in [-1., 1.] {
                let mut tracker = ScrollTracker::new(tick);
                assert_eq!(tracker.accumulate(direction * (f64::from(tick) - 1.)), 0);
                assert_eq!(tracker.accumulate(0.), 0);
                assert_eq!(tracker.accumulate(0.), 0);
                assert_eq!(
                    tracker.accumulate(-direction * f64::from(tick)),
                    -direction as i8,
                    "tick = {tick}, previous direction = {direction}",
                );
            }
        }
    }

    #[test]
    fn zero_samples_preserve_remainders_in_the_same_direction() {
        for direction in [-1., 1.] {
            let mut tracker = ScrollTracker::new(10);
            assert_eq!(tracker.accumulate(direction * 6.), 0);
            assert_eq!(tracker.accumulate(0.), 0);
            assert_eq!(tracker.accumulate(direction * 4.), direction as i8);
        }
    }

    #[test]
    fn fractional_reversal_accumulates_without_the_old_remainder() {
        let mut tracker = ScrollTracker::new(10);
        assert_eq!(tracker.accumulate(9.5), 0);
        assert_eq!(tracker.accumulate(0.), 0);
        assert_eq!(tracker.accumulate(-4.5), 0);
        assert_eq!(tracker.accumulate(-5.5), -1);
        assert_eq!(tracker.accumulate(-10.), -1);
    }

    #[test]
    fn reset_discards_remainders() {
        let mut tracker = ScrollTracker::new(10);
        assert_eq!(tracker.accumulate(9.), 0);
        tracker.reset();
        assert_eq!(tracker.accumulate(1.), 0);
        assert_eq!(tracker.accumulate(9.), 1);
    }
}

#[cfg(test)]
mod finite_input_tests {
    use super::*;
    #[test]
    fn invalid_threshold_and_delta_never_erase_valid_motion_or_panic() {
        for threshold in [0, -1, i8::MIN] {
            let mut tracker = ScrollTracker::new(threshold);
            tracker.accumulate(10.);
            tracker.accumulate(f64::NAN);
            tracker.accumulate(f64::INFINITY);
            tracker.accumulate(1.);
            assert!(tracker.acc.is_finite());
        }
        let mut tracker = ScrollTracker::new(10);
        assert_eq!(tracker.accumulate(9.), 0);
        assert_eq!(tracker.accumulate(f64::NAN), 0);
        assert_eq!(tracker.accumulate(1.), 1);
    }
}
