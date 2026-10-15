//! Numerical animation state evaluated at explicit presentation timestamps.
//!
//! The core has no clock source, platform configuration or renderer access. A
//! platform adapter maps its configuration and scales real-time gesture velocity.

use std::time::Duration;

use crate::motion::CubicTiming;
use crate::spring::Spring;

#[derive(Debug, Clone)]
pub struct Trajectory {
    from: f64,
    to: f64,
    initial_velocity: f64,
    is_off: bool,
    duration: Duration,
    /// First target arrival, independently of the complete spring/easing timeline.
    clamped_duration: Duration,
    start_time: Duration,
    kind: Kind,
}

#[derive(Debug, Clone, Copy)]
enum Kind {
    Continuation,
    Easing {
        curve: Curve,
    },
    Spring(Spring),
    Deceleration {
        initial_velocity: f64,
        deceleration_rate: f64,
    },
}

#[derive(Debug, Clone, Copy)]
pub enum Curve {
    Linear,
    EaseOutQuad,
    EaseOutCubic,
    EaseOutExpo,
    CubicBezier(CubicTiming),
}

impl Trajectory {
    /// Disable motion while retaining its endpoints and current configuration.
    pub fn disable(&mut self) {
        self.is_off = true;
        self.duration = Duration::ZERO;
        self.clamped_duration = Duration::ZERO;
    }

    pub fn initial_velocity(&self) -> f64 {
        self.initial_velocity
    }
    pub fn clamped_duration(&self) -> Duration {
        self.clamped_duration
    }

    /// Restart with clock-domain velocity. Interrupted easing uses the Realand
    /// Hermite continuation so incoming momentum, including zero speed, is retained.
    pub fn restarted_at(
        &self,
        start_time: Duration,
        from: f64,
        to: f64,
        initial_velocity: f64,
    ) -> Self {
        self.restart_at(start_time, from, to, initial_velocity, false)
    }

    /// Deliberately replay the retained preset from new endpoints at rest. This
    /// differs from an interruption: waiting captures can begin their original
    /// transition after the snapshot becomes available.
    pub fn restarted_fresh_at(&self, start_time: Duration, from: f64, to: f64) -> Self {
        self.restart_at(start_time, from, to, 0., true)
    }

    fn restart_at(
        &self,
        start_time: Duration,
        from: f64,
        to: f64,
        initial_velocity: f64,
        fresh: bool,
    ) -> Self {
        if self.is_off {
            let mut trajectory =
                Self::ease_at(start_time, from, to, 0., Duration::ZERO, Curve::Linear);
            trajectory.disable();
            return trajectory;
        }
        match self.kind {
            Kind::Continuation => {
                Self::continuous_ease_at(start_time, from, to, initial_velocity, self.duration)
            }
            Kind::Easing { curve } => {
                let active = start_time > self.start_time && !self.is_done_at(start_time, false);
                if !fresh && (active || initial_velocity != 0.) {
                    Self::continuous_ease_at(start_time, from, to, initial_velocity, self.duration)
                } else {
                    Self::ease_at(start_time, from, to, initial_velocity, self.duration, curve)
                }
            }
            Kind::Spring(spring) => Self::spring_at(
                start_time,
                Spring {
                    from,
                    to,
                    initial_velocity,
                    params: spring.params,
                },
            ),
            Kind::Deceleration {
                deceleration_rate, ..
            } => {
                let threshold = 0.001; // Retain the established restart threshold.
                Self::decelerate_at(
                    start_time,
                    from,
                    initial_velocity,
                    deceleration_rate,
                    threshold,
                )
            }
        }
    }

    pub fn ease_at(
        start_time: Duration,
        from: f64,
        to: f64,
        initial_velocity: f64,
        duration: Duration,
        curve: Curve,
    ) -> Self {
        let kind = Kind::Easing { curve };
        let clamped_duration = match curve {
            Curve::CubicBezier(bezier) => {
                let fraction = bezier.first_target_fraction();
                if fraction < 1. {
                    // Duration's floating-point scaling rounds to the nearest nanosecond.
                    // Wait one more nanosecond so clamping cannot precede the crossing.
                    duration
                        .mul_f64(fraction)
                        .saturating_add(Duration::from_nanos(1))
                        .min(duration)
                } else {
                    duration
                }
            }
            _ => duration,
        };

        Self {
            from,
            to,
            initial_velocity,
            is_off: false,
            duration,
            clamped_duration,
            start_time,
            kind,
        }
    }

    /// Realand continuation curve for an interrupted transition. A cubic
    /// Hermite segment retains the current position and velocity, then settles
    /// at the target with zero velocity. This is an engineering choice, not a
    /// recovered private Dock timing function. Existing easing curves are unchanged.
    pub fn continuous_ease_at(
        start_time: Duration,
        from: f64,
        to: f64,
        initial_velocity: f64,
        duration: Duration,
    ) -> Self {
        let initial_velocity = if initial_velocity.is_finite() {
            initial_velocity
        } else {
            0.
        };
        let delta = to - from;
        // The endpoint equation factors as (1-t)^2 times a linear term.
        // A fast incoming velocity can reach the target before the final
        // zero-velocity endpoint; retain that first-arrival deadline.
        let fraction = delta / (initial_velocity * duration.as_secs_f64() - 2. * delta);
        let clamped_duration = if fraction > 0. && fraction < 1. {
            duration
                .mul_f64(fraction)
                .saturating_add(Duration::from_nanos(1))
                .min(duration)
        } else {
            duration
        };
        Self {
            from,
            to,
            initial_velocity,
            is_off: false,
            duration,
            clamped_duration,
            start_time,
            kind: Kind::Continuation,
        }
    }

    pub fn spring_at(start_time: Duration, spring: Spring) -> Self {
        let duration = spring.duration();
        let clamped_duration = if spring.from == spring.to && spring.initial_velocity != 0. {
            // Starting at the target while still moving is not a settled pose.
            // Retain the launch and return until its full settling deadline.
            duration
        } else {
            spring
                .precise_clamped_duration()
                .unwrap_or(duration)
                .min(duration)
        };
        let kind = Kind::Spring(spring);

        Self {
            from: spring.from,
            to: spring.to,
            initial_velocity: spring.initial_velocity,
            is_off: false,
            duration,
            clamped_duration,
            start_time,
            kind,
        }
    }

    pub fn decelerate_at(
        start_time: Duration,
        from: f64,
        initial_velocity: f64,
        deceleration_rate: f64,
        threshold: f64,
    ) -> Self {
        // The discrete decay is v(t) = v₀ * r^(1000t), so the animation ends
        // when |v(t)| reaches the requested threshold.  Keep the rate in the
        // physically meaningful range; otherwise ln(rate) is zero/NaN and a
        // malformed config used to panic while constructing a Duration.
        const MAX_RATE: f64 = 1. - 1e-6;
        let deceleration_rate = if deceleration_rate.is_finite() {
            deceleration_rate.clamp(f64::MIN_POSITIVE, MAX_RATE)
        } else {
            MAX_RATE
        };
        let coeff = 1000. * deceleration_rate.ln();
        let speed = initial_velocity.abs();
        let duration = if speed == 0. || (threshold.is_finite() && threshold >= speed) {
            Duration::ZERO
        } else if threshold.is_nan() || threshold <= 0. {
            // Exponential decay reaches zero only asymptotically. A non-positive
            // or unknown threshold therefore has no finite completion deadline.
            Duration::MAX
        } else {
            // ln(threshold / speed) is negative when the threshold is below the
            // launch speed, and coeff is negative for a decaying rate.
            let duration_s = (threshold / speed).ln() / coeff;
            if duration_s.is_finite() && duration_s > 0. {
                Duration::from_secs_f64(duration_s)
            } else if duration_s <= 0. {
                Duration::ZERO
            } else {
                Duration::MAX
            }
        };

        let to = from - initial_velocity / coeff;

        let kind = Kind::Deceleration {
            initial_velocity,
            deceleration_rate,
        };

        Self {
            from,
            to,
            initial_velocity,
            is_off: false,
            duration,
            clamped_duration: duration,
            start_time,
            kind,
        }
    }

    pub fn is_done_at(&self, at: Duration, complete_instantly: bool) -> bool {
        if complete_instantly {
            return true;
        }

        at >= self.start_time.saturating_add(self.duration)
    }

    pub fn is_clamped_done_at(&self, at: Duration, complete_instantly: bool) -> bool {
        if complete_instantly {
            return true;
        }

        at >= self.start_time.saturating_add(self.clamped_duration)
    }

    pub fn value_at(&self, at: Duration, complete_instantly: bool) -> f64 {
        if at <= self.start_time {
            // Return from when at == start_time so that when the animations are off, the behavior
            // within a single event loop cycle (i.e. no time had passed since the start of an
            // animation) matches the behavior when the animations are on.
            return self.from;
        } else if self.start_time.saturating_add(self.duration) <= at {
            return self.to;
        }

        if complete_instantly {
            return self.to;
        }

        let passed = at.saturating_sub(self.start_time);

        match self.kind {
            Kind::Continuation => {
                let total = self.duration.as_secs_f64();
                let t = passed.as_secs_f64() / total;
                self.from
                    + (self.to - self.from) * (t * t * (3. - 2. * t))
                    + self.initial_velocity * total * (t * (1. - t) * (1. - t))
            }
            Kind::Easing { curve } => {
                let passed = passed.as_secs_f64();
                let total = self.duration.as_secs_f64();
                let x = (passed / total).clamp(0., 1.);
                curve.sample(x) * (self.to - self.from) + self.from
            }
            Kind::Spring(spring) => {
                let value = spring.value_at(passed);

                // Protect against numerical instability.
                let omega = (spring.params.stiffness / spring.params.mass).sqrt();
                let range = (self.to - self.from)
                    .abs()
                    .max(spring.initial_velocity.abs() / omega.max(0.001))
                    * 10.;
                value.clamp(
                    self.from.min(self.to) - range,
                    self.from.max(self.to) + range,
                )
            }
            Kind::Deceleration {
                initial_velocity,
                deceleration_rate,
            } => {
                let passed = passed.as_secs_f64();
                let coeff = 1000. * deceleration_rate.ln();
                self.from + (deceleration_rate.powf(1000. * passed) - 1.) / coeff * initial_velocity
            }
        }
    }

    /// Velocity in units per trajectory-clock second. Sampling never mutates it.
    pub fn velocity_at(&self, at: Duration, complete_instantly: bool) -> f64 {
        if at < self.start_time || self.is_off || self.is_done_at(at, complete_instantly) {
            return 0.;
        }
        let elapsed = at.saturating_sub(self.start_time);
        match self.kind {
            Kind::Continuation => {
                let total = self.duration.as_secs_f64();
                let t = elapsed.as_secs_f64() / total;
                (self.to - self.from) * (6. * t * (1. - t)) / total
                    + self.initial_velocity * (1. - 4. * t + 3. * t * t)
            }
            Kind::Spring(spring) => spring.velocity_at(elapsed),
            Kind::Deceleration {
                initial_velocity,
                deceleration_rate,
            } => initial_velocity * deceleration_rate.powf(1000. * elapsed.as_secs_f64()),
            Kind::Easing { curve } => {
                let total = self.duration.as_secs_f64();
                let x = elapsed.as_secs_f64() / total;
                // Only used when retargeting, not in the rendering hot path.
                // A bounded secant also handles vertical Bezier tangents.
                let left = (x - 0.00001).max(0.);
                let right = (x + 0.00001).min(1.);
                (curve.sample(right) - curve.sample(left)) / (right - left) * (self.to - self.from)
                    / total
            }
        }
    }

    /// Returns a value that stops at the target value after first reaching it.
    ///
    /// Best effort; not always exactly precise.
    pub fn clamped_value_at(&self, at: Duration, complete_instantly: bool) -> f64 {
        if self.is_clamped_done_at(at, complete_instantly) {
            return self.to;
        }

        let value = self.value_at(at, complete_instantly);
        if matches!(self.kind, Kind::Spring(_) | Kind::Continuation) && self.to != self.from {
            // The deadline rounds up to a nanosecond. Cap only the target side
            // against the final floating-point ulp; launch motion away from
            // the target and the complete spring trajectory remain available.
            if self.to >= self.from {
                return value.min(self.to);
            }
            return value.max(self.to);
        }
        value
    }

    pub fn to(&self) -> f64 {
        self.to
    }

    pub fn from(&self) -> f64 {
        self.from
    }

    pub fn start_time(&self) -> Duration {
        self.start_time
    }

    pub fn end_time(&self) -> Duration {
        self.start_time.saturating_add(self.duration)
    }

    pub fn duration(&self) -> Duration {
        self.duration
    }

    /// Delay the start while preserving the configured animation duration and curve.
    pub fn delay_by(&mut self, delay: Duration) {
        if !self.is_off {
            self.start_time = self.start_time.saturating_add(delay);
        }
    }

    pub fn offset(&mut self, offset: f64) {
        self.from += offset;
        self.to += offset;

        if let Kind::Spring(spring) = &mut self.kind {
            spring.from += offset;
            spring.to += offset;
        }
    }
}

impl Curve {
    pub fn sample(self, x: f64) -> f64 {
        match self {
            Self::Linear => x,
            // Preserve the arithmetic of the established keyframe 1.1.1 curves.
            Self::EaseOutQuad => x * (2. - x),
            Self::EaseOutCubic => {
                let x_minus_one = x - 1.;
                1. + x_minus_one * x_minus_one * x_minus_one
            }
            // Normalize the endpoint to avoid a snap on the last frame.
            Self::EaseOutExpo => (1. - 2f64.powf(-10. * x)) * (1024. / 1023.),
            Self::CubicBezier(curve) => curve.sample(x),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spring::SpringParams;

    #[test]
    fn explicit_sampling_is_independent_of_order_and_frame_frequency() {
        let start = Duration::from_secs(1);
        let trajectory =
            Trajectory::ease_at(start, 10., 30., 0., Duration::from_secs(1), Curve::Linear);
        for millis in [500, 0, 1000, 250, 750, 500] {
            let at = start + Duration::from_millis(millis);
            assert_eq!(
                trajectory.value_at(at, false),
                10. + 20. * millis as f64 / 1000.
            );
        }
        assert_eq!(trajectory.value_at(Duration::ZERO, false), 10.);
        assert_eq!(trajectory.velocity_at(Duration::ZERO, false), 0.);
        assert_eq!(
            trajectory.velocity_at(start + Duration::from_millis(500), false),
            20.
        );
        assert!(trajectory.is_done_at(Duration::from_secs(2), false));
    }

    #[test]
    fn restart_offset_and_delay_preserve_pose_and_momentum() {
        let trajectory = Trajectory::spring_at(
            Duration::ZERO,
            Spring {
                from: 0.,
                to: 1.,
                initial_velocity: 0.,
                params: SpringParams::new(0.92, 300., 0.001),
            },
        );
        let at = Duration::from_millis(80);
        let pose = trajectory.value_at(at, false);
        let velocity = trajectory.velocity_at(at, false);
        let mut reversed = trajectory.restarted_at(at, pose, 0., velocity);
        reversed.offset(512.);
        assert_eq!(reversed.value_at(at, false), pose + 512.);
        assert!((reversed.velocity_at(at, false) - velocity).abs() < 1e-12);
        let next = at + Duration::from_nanos(100);
        let slope = (reversed.value_at(next, false) - reversed.value_at(at, false)) / 0.0000001;
        assert!((slope - velocity).abs() < 0.001);
        let end = reversed.end_time();
        reversed.delay_by(Duration::from_millis(50));
        assert_eq!(reversed.end_time(), end + Duration::from_millis(50));
        assert_eq!(reversed.velocity_at(at, false), 0.);
        assert_eq!(reversed.value_at(at, false), pose + 512.);
        assert_eq!(reversed.value_at(reversed.end_time(), false), 512.);
    }

    #[test]
    fn interrupted_presets_preserve_position_and_velocity() {
        for curve in [
            Curve::Linear,
            Curve::EaseOutQuad,
            Curve::EaseOutCubic,
            Curve::EaseOutExpo,
            Curve::CubicBezier(CubicTiming::new(0.25, 0.1, 0.25, 1.)),
            Curve::CubicBezier(CubicTiming::new(0.68, -0.55, 0.265, 1.55)),
        ] {
            let motion = Trajectory::ease_at(
                Duration::ZERO,
                10.,
                30.,
                0.,
                Duration::from_millis(400),
                curve,
            );
            let at = Duration::from_millis(80);
            let pose = motion.value_at(at, false);
            let velocity = motion.velocity_at(at, false);
            let reversed = motion.restarted_at(at, pose, 5., velocity);
            assert_eq!(reversed.value_at(at, false), pose);
            assert_eq!(reversed.velocity_at(at, false), velocity);
            let delta = Duration::from_nanos(100);
            let measured = (reversed.value_at(at + delta, false) - pose) / delta.as_secs_f64();
            assert!((measured - velocity).abs() < 0.001, "{curve:?}");
            assert_eq!(reversed.value_at(reversed.end_time(), false), 5.);
            assert_eq!(reversed.velocity_at(reversed.end_time(), false), 0.);
            // An explicit visible zero velocity must also survive an active retarget.
            let stationary = motion.restarted_at(at, pose, 5., 0.);
            assert_eq!(stationary.velocity_at(at, false), 0.);
        }
    }

    #[test]
    fn deliberate_fresh_and_settled_restarts_keep_preset_samples() {
        for curve in [
            Curve::Linear,
            Curve::EaseOutQuad,
            Curve::EaseOutCubic,
            Curve::EaseOutExpo,
            Curve::CubicBezier(CubicTiming::new(0.25, 0.1, 0.25, 1.)),
        ] {
            let duration = Duration::from_millis(400);
            let motion = Trajectory::ease_at(Duration::ZERO, 0., 1., 0., duration, curve);
            // Waiting closing-window captures deliberately restart their original
            // endpoints, even if the capture outlasted the original timeline.
            for at in [Duration::from_millis(80), Duration::from_secs(1)] {
                let restarted = motion.restarted_fresh_at(at, 0., 1.);
                let fresh = Trajectory::ease_at(at, 0., 1., 0., duration, curve);
                for elapsed in [0, 1, 16, 80, 200, 399, 400] {
                    let sample = at + Duration::from_millis(elapsed);
                    assert_eq!(
                        restarted.value_at(sample, false).to_bits(),
                        fresh.value_at(sample, false).to_bits()
                    );
                }
            }
            let at = Duration::from_secs(1);
            let restarted = motion.restarted_at(at, 1., 2., 0.);
            let fresh = Trajectory::ease_at(at, 1., 2., 0., duration, curve);
            assert_eq!(
                restarted.value_at(at + duration / 4, false).to_bits(),
                fresh.value_at(at + duration / 4, false).to_bits()
            );
        }
    }

    #[test]
    fn active_zero_speed_at_original_endpoints_is_still_an_interruption() {
        let motion = Trajectory::ease_at(
            Duration::ZERO,
            0.,
            1.,
            0.,
            Duration::from_secs(1),
            Curve::EaseOutCubic,
        );
        let at = Duration::from_millis(80);
        let interrupted = motion.restarted_at(at, 0., 1., 0.);
        let fresh = motion.restarted_fresh_at(at, 0., 1.);
        assert_eq!(interrupted.velocity_at(at, false), 0.);
        assert!(fresh.velocity_at(at, false) > 2.9);
        let next = at + Duration::from_micros(1);
        assert!(interrupted.value_at(next, false) < fresh.value_at(next, false) * 0.00001);
    }

    #[test]
    fn equal_endpoint_clamped_motion_keeps_both_launch_directions_until_settled() {
        for velocity in [-3., 3.] {
            for motion in [
                Trajectory::continuous_ease_at(
                    Duration::ZERO,
                    0.5,
                    0.5,
                    velocity,
                    Duration::from_millis(400),
                ),
                Trajectory::spring_at(
                    Duration::ZERO,
                    Spring {
                        from: 0.5,
                        to: 0.5,
                        initial_velocity: velocity,
                        params: SpringParams::new(0.92, 300., 0.0001),
                    },
                ),
            ] {
                assert!(motion.clamped_duration() > Duration::ZERO);
                let delta = Duration::from_nanos(100);
                let value = motion.clamped_value_at(delta, false);
                assert_eq!(value, motion.value_at(delta, false));
                let measured = (value - 0.5) / delta.as_secs_f64();
                assert!((measured - velocity).abs() < 0.00001);
                assert_eq!(motion.clamped_value_at(motion.end_time(), false), 0.5);
                assert_eq!(motion.velocity_at(motion.end_time(), false), 0.);
            }
        }
    }

    #[test]
    fn deceleration_restart_keeps_pose_and_launch_velocity() {
        let motion = Trajectory::decelerate_at(Duration::ZERO, 12., 10., 0.997, 0.001);
        let at = Duration::from_millis(80);
        let pose = motion.value_at(at, false);
        let velocity = motion.velocity_at(at, false);
        let restarted = motion.restarted_at(at, pose, 999., velocity);
        assert_eq!(restarted.value_at(at, false), pose);
        assert_eq!(restarted.velocity_at(at, false), velocity);
        let delta = Duration::from_nanos(100);
        let measured = (restarted.value_at(at + delta, false) - pose) / delta.as_secs_f64();
        assert!((measured - velocity).abs() < 0.00001);
        assert_eq!(
            restarted.value_at(restarted.end_time(), false),
            restarted.to()
        );
    }

    #[test]
    fn tangent_target_arrival_keeps_the_full_easing_trajectory() {
        let trajectory = Trajectory::ease_at(
            Duration::ZERO,
            0.,
            1.,
            0.,
            Duration::from_secs(1),
            Curve::CubicBezier(CubicTiming::new(1. / 3., 3., 2. / 3., -2.)),
        );
        let arrival = Duration::from_millis(250);
        assert_eq!(
            trajectory.clamped_duration(),
            arrival + Duration::from_nanos(1)
        );
        assert!(!trajectory.is_clamped_done_at(arrival, false));
        assert!(trajectory.is_clamped_done_at(arrival + Duration::from_nanos(1), false));
        let later = Duration::from_millis(500);
        assert!(trajectory.value_at(later, false) < 1.);
        assert_eq!(trajectory.clamped_value_at(later, false), 1.);
        assert!(!trajectory.is_done_at(later, false));
    }

    #[test]
    fn disable_and_instant_completion_keep_the_same_event_cycle_pose() {
        let start = Duration::from_secs(1);
        let mut trajectory =
            Trajectory::ease_at(start, 4., 8., 0., Duration::from_secs(1), Curve::Linear);
        assert_eq!(trajectory.value_at(start, true), 4.);
        assert_eq!(trajectory.clamped_value_at(start, true), 8.);
        assert_eq!(trajectory.velocity_at(start, true), 0.);
        trajectory.disable();
        trajectory.delay_by(Duration::from_secs(1));
        assert_eq!(trajectory.start_time(), start);
        assert_eq!(trajectory.value_at(start, false), 4.);
        assert_eq!(
            trajectory.value_at(start + Duration::from_nanos(1), false),
            8.
        );
        let restarted = trajectory.restarted_at(start, 20., 40., 10.);
        assert_eq!(restarted.velocity_at(start, false), 0.);
        assert_eq!(
            restarted.value_at(start + Duration::from_nanos(1), false),
            40.
        );
    }

    #[test]
    fn nonsettling_spring_deadlines_do_not_overflow() {
        let trajectory = Trajectory::spring_at(
            Duration::from_secs(1),
            Spring {
                from: 0.,
                to: 1.,
                initial_velocity: 0.,
                params: SpringParams::new(0., 800., 0.0001),
            },
        );
        assert_eq!(trajectory.duration(), Duration::MAX);
        assert_eq!(trajectory.end_time(), Duration::MAX);
        assert!(!trajectory.is_done_at(Duration::from_secs(2), false));
        assert!(trajectory
            .value_at(Duration::from_secs(2), false)
            .is_finite());
    }

    #[test]
    fn deceleration_stops_at_the_requested_speed_threshold() {
        for velocity in [-10., 10.] {
            let trajectory = Trajectory::decelerate_at(Duration::ZERO, 0., velocity, 0.99, 0.1);
            let coeff = 1000. * 0.99f64.ln();
            let expected = Duration::from_secs_f64((0.1f64 / velocity.abs()).ln() / coeff);
            assert!(trajectory.duration().abs_diff(expected) <= Duration::from_nanos(1));
            let last = trajectory.end_time() - Duration::from_nanos(1);
            assert!((trajectory.velocity_at(last, false).abs() - 0.1).abs() < 1e-8);
            assert_eq!(trajectory.velocity_at(trajectory.end_time(), false), 0.);
            assert_eq!(
                trajectory.value_at(trajectory.end_time(), false),
                trajectory.to()
            );
        }
    }
}

#[cfg(test)]
mod continuation_tests {
    use super::*;

    #[test]
    fn interrupted_motion_is_continuous_in_position_and_velocity() {
        for velocity in [-3., 0., 3.] {
            for target in [0., 1.] {
                let at = Duration::from_secs(1);
                let motion = Trajectory::continuous_ease_at(
                    at,
                    0.5,
                    target,
                    velocity,
                    Duration::from_millis(300),
                );
                assert_eq!(motion.value_at(at, false), 0.5);
                assert_eq!(motion.velocity_at(at, false), velocity);
                let sample = at + Duration::from_nanos(100);
                let measured = (motion.value_at(sample, false) - 0.5) / 1e-7;
                assert!((measured - velocity).abs() < 1e-5);
                assert_eq!(motion.value_at(motion.end_time(), false), target);
                assert_eq!(motion.velocity_at(motion.end_time(), false), 0.);
                let restart = motion.restarted_at(
                    sample,
                    motion.value_at(sample, false),
                    0.25,
                    motion.velocity_at(sample, false),
                );
                assert_eq!(
                    restart.value_at(sample, false),
                    motion.value_at(sample, false)
                );
                assert_eq!(
                    restart.velocity_at(sample, false),
                    motion.velocity_at(sample, false)
                );
            }
        }
    }

    #[test]
    fn early_target_arrival_and_instant_completion_are_explicit() {
        let motion =
            Trajectory::continuous_ease_at(Duration::ZERO, 0., 1., 10., Duration::from_secs(1));
        assert!(motion.clamped_duration() < motion.duration());
        assert_eq!(
            motion.clamped_value_at(Duration::from_millis(200), false),
            1.
        );
        assert_eq!(motion.value_at(Duration::from_millis(1), true), 1.);
        assert_eq!(motion.velocity_at(Duration::from_millis(1), true), 0.);
        let instant = Trajectory::continuous_ease_at(Duration::ZERO, 1., 0., 3., Duration::ZERO);
        assert!(instant.is_done_at(Duration::ZERO, false));
        assert_eq!(instant.velocity_at(Duration::ZERO, false), 0.);
    }
}
