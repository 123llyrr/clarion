use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use crate::utils::get_monotonic_time;

/// Shareable lazy clock that can change rate.
///
/// The clock will fetch the time once and then retain it until explicitly cleared with
/// [`Clock::clear`].
#[derive(Debug, Default, Clone)]
pub struct Clock {
    inner: Rc<RefCell<AdjustableClock>>,
}

#[derive(Debug, Default, Clone)]
struct LazyClock {
    time: Option<Duration>,
}

/// Clock that can adjust its rate.
#[derive(Debug, Clone)]
struct AdjustableClock {
    inner: LazyClock,
    rate_clock: realand_core::rate_clock::RateClock,
}

impl Clock {
    /// Creates a new clock with the given time.
    pub fn with_time(time: Duration) -> Self {
        let clock = AdjustableClock::new(LazyClock::with_time(time));
        Self {
            inner: Rc::new(RefCell::new(clock)),
        }
    }

    /// Returns the current time.
    pub fn now(&self) -> Duration {
        self.inner.borrow_mut().now()
    }

    /// Returns the underlying time not adjusted for rate change.
    pub fn now_unadjusted(&self) -> Duration {
        self.inner.borrow_mut().inner.now()
    }

    /// Sets the unadjusted clock time.
    pub fn set_unadjusted(&mut self, time: Duration) {
        self.inner.borrow_mut().inner.set(time);
    }

    /// Project presentation reads to this output's target without advancing the
    /// event-time clock seen by input and animation lifecycle mutations.
    ///
    /// Drop the guard before changing the rate or instant-completion setting.
    /// Nested projections must be dropped in reverse construction order.
    pub fn project_to(&self, time: Duration) -> PresentationProjection {
        let mut current = self.inner.borrow_mut();
        let saved = current.clone();
        current.inner.set(time);
        PresentationProjection {
            clock: self.clone(),
            saved,
        }
    }

    /// Clears the stored time so it's re-fetched again next.
    pub fn clear(&mut self) {
        self.inner.borrow_mut().inner.clear();
    }

    /// Gets the clock rate.
    pub fn rate(&self) -> f64 {
        self.inner.borrow().rate()
    }

    /// Sets the clock rate.
    pub fn set_rate(&mut self, rate: f64) {
        self.inner.borrow_mut().set_rate(rate);
    }

    /// Returns whether animations should complete instantly.
    pub fn should_complete_instantly(&self) -> bool {
        self.inner.borrow().should_complete_instantly()
    }

    /// Sets whether animations should complete instantly.
    pub fn set_complete_instantly(&mut self, value: bool) {
        self.inner.borrow_mut().set_complete_instantly(value);
    }
}

/// A scoped, owner-thread presentation timestamp. Holds no RefCell borrow, so
/// shared animation clocks remain usable while a scene is prepared and drawn.
#[must_use = "dropping the projection restores the event-time clock"]
pub struct PresentationProjection {
    clock: Clock,
    saved: AdjustableClock,
}

impl Drop for PresentationProjection {
    fn drop(&mut self) {
        *self.clock.inner.borrow_mut() = self.saved.clone();
    }
}

impl PartialEq for Clock {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Eq for Clock {}

impl LazyClock {
    pub fn with_time(time: Duration) -> Self {
        Self { time: Some(time) }
    }

    pub fn clear(&mut self) {
        self.time = None;
    }

    pub fn set(&mut self, time: Duration) {
        self.time = Some(time);
    }

    pub fn now(&mut self) -> Duration {
        *self.time.get_or_insert_with(get_monotonic_time)
    }
}

impl AdjustableClock {
    pub fn new(mut inner: LazyClock) -> Self {
        let rate_clock = realand_core::rate_clock::RateClock::new(inner.now());
        Self { inner, rate_clock }
    }

    pub fn rate(&self) -> f64 {
        self.rate_clock.rate()
    }

    pub fn set_rate(&mut self, rate: f64) {
        self.rate_clock.set_rate_at(self.inner.now(), rate);
    }

    pub fn should_complete_instantly(&self) -> bool {
        self.rate_clock.should_complete_instantly()
    }

    pub fn set_complete_instantly(&mut self, value: bool) {
        self.rate_clock.set_complete_instantly(value);
    }

    pub fn now(&mut self) -> Duration {
        self.rate_clock.advance_to(self.inner.now())
    }
}

impl Default for AdjustableClock {
    fn default() -> Self {
        Self::new(LazyClock::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frozen_clock() {
        let mut clock = Clock::with_time(Duration::ZERO);
        assert_eq!(clock.now(), Duration::ZERO);

        clock.set_unadjusted(Duration::from_millis(100));
        assert_eq!(clock.now(), Duration::from_millis(100));

        clock.set_unadjusted(Duration::from_millis(200));
        assert_eq!(clock.now(), Duration::from_millis(200));
    }

    #[test]
    fn rate_change() {
        let mut clock = Clock::with_time(Duration::ZERO);
        clock.set_rate(0.5);

        clock.set_unadjusted(Duration::from_millis(100));
        assert_eq!(clock.now_unadjusted(), Duration::from_millis(100));
        assert_eq!(clock.now(), Duration::from_millis(50));

        clock.set_unadjusted(Duration::from_millis(200));
        assert_eq!(clock.now_unadjusted(), Duration::from_millis(200));
        assert_eq!(clock.now(), Duration::from_millis(100));

        clock.set_unadjusted(Duration::from_millis(150));
        assert_eq!(clock.now_unadjusted(), Duration::from_millis(150));
        assert_eq!(clock.now(), Duration::from_millis(75));

        clock.set_rate(2.0);

        clock.set_unadjusted(Duration::from_millis(250));
        assert_eq!(clock.now_unadjusted(), Duration::from_millis(250));
        assert_eq!(clock.now(), Duration::from_millis(275));
    }

    #[test]
    fn rate_change_does_not_rescale_time_since_last_read() {
        let mut clock = Clock::with_time(Duration::ZERO);

        clock.set_unadjusted(Duration::from_millis(100));
        clock.set_rate(0.5);
        // The 100 ms before changing the rate was already elapsed at rate 1.
        assert_eq!(clock.now(), Duration::from_millis(100));

        clock.set_unadjusted(Duration::from_millis(200));
        assert_eq!(clock.now(), Duration::from_millis(150));
    }

    #[test]
    fn non_finite_rate_falls_back_to_realtime() {
        let mut clock = Clock::with_time(Duration::ZERO);
        clock.set_rate(f64::NAN);
        clock.set_unadjusted(Duration::from_millis(100));
        assert_eq!(clock.now(), Duration::from_millis(100));

        clock.set_rate(f64::INFINITY);
        clock.set_unadjusted(Duration::from_millis(200));
        assert_eq!(clock.now(), Duration::from_millis(200));
    }

    #[test]
    fn mixed_refresh_forecasts_restore_the_event_clock_at_every_rate() {
        use realand_core::frame_clock::FrameClock;
        use realand_core::trajectory::{Curve, Trajectory};

        let base = Duration::from_secs(1);
        let period = Duration::from_nanos(8_333_333);
        for rate in [0.25, 0.73, 1., 2.] {
            let mut clock = Clock::with_time(base);
            clock.set_rate(rate);
            let trajectory = Trajectory::ease_at(
                clock.now(),
                0.,
                1.,
                0.,
                Duration::from_secs(2),
                Curve::Linear,
            );
            let mut fast = FrameClock::new(Some(period), false);
            fast.presented(base);
            let mut slow = FrameClock::new(Some(period * 2), false);
            slow.presented(base);
            let mut previous = -1.;

            for tick in 1..=120 {
                let event_time = base + period * (tick - 1) + Duration::from_nanos(1);
                clock.set_unadjusted(event_time);
                let event_adjusted = clock.now();
                let slow_target = slow.next_presentation_time_at(event_time);
                {
                    let _projection = clock.project_to(slow_target);
                    let _ = trajectory.value_at(clock.now(), false);
                }
                assert_eq!(clock.now_unadjusted(), event_time);
                assert_eq!(clock.now(), event_adjusted);

                let target = fast.next_presentation_time_at(event_time);
                {
                    let _projection = clock.project_to(target);
                    let value = trajectory.value_at(clock.now(), false);
                    let expected = (target - base).as_secs_f64() * rate / 2.;
                    assert!((value - expected).abs() < 1e-7);
                    assert!(value > previous);
                    previous = value;
                }
                assert_eq!(clock.now_unadjusted(), event_time);
                assert_eq!(clock.now(), event_adjusted);
                fast.presented(target);
                if tick % 2 == 0 {
                    slow.presented(slow_target);
                }
            }
        }
    }

    #[test]
    fn nested_projections_and_early_return_restore_lazy_and_rate_state() {
        let mut clock = Clock::with_time(Duration::from_secs(1));
        clock.set_rate(0.5);
        clock.set_complete_instantly(true);
        let adjusted = clock.now();
        {
            let _outer = clock.project_to(Duration::from_secs(3));
            assert_eq!(clock.now(), Duration::from_secs(2));
            let nested = || {
                let _inner = clock.project_to(Duration::from_secs(2));
                assert_eq!(clock.now(), Duration::from_millis(1500));
            };
            nested();
            assert_eq!(clock.now(), Duration::from_secs(2));
        }
        assert_eq!(clock.now_unadjusted(), Duration::from_secs(1));
        assert_eq!(clock.now(), adjusted);
        assert!(clock.should_complete_instantly());
        // Projection must not integrate its future delta into a later rate change.
        clock.set_unadjusted(Duration::from_secs(2));
        clock.set_rate(2.);
        assert_eq!(clock.now(), Duration::from_millis(1500));
        clock.set_unadjusted(Duration::from_secs(3));
        assert_eq!(clock.now(), Duration::from_millis(3500));
    }

    #[test]
    fn completion_at_foreign_future_target_does_not_escape_projection() {
        use realand_core::trajectory::{Curve, Trajectory};

        let clock = Clock::with_time(Duration::from_secs(1));
        let trajectory = Trajectory::ease_at(
            clock.now(),
            0.,
            1.,
            0.,
            Duration::from_millis(12),
            Curve::Linear,
        );
        {
            let _slow = clock.project_to(Duration::from_millis(1016));
            assert!(trajectory.is_done_at(clock.now(), false));
        }
        assert!(!trajectory.is_done_at(clock.now(), false));
        {
            let _fast = clock.project_to(Duration::from_millis(1008));
            assert!(!trajectory.is_done_at(clock.now(), false));
            assert!((trajectory.value_at(clock.now(), false) - 2. / 3.).abs() < 1e-12);
        }
        assert_eq!(clock.now_unadjusted(), Duration::from_secs(1));
    }
}
