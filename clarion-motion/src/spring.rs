//! Device-independent spring trajectories and settling deadlines.
//!
//! This is Clarion's established solver, preserved across the platform boundary.

use std::time::Duration;

#[derive(Debug, Clone, Copy)]
pub struct SpringParams {
    pub damping: f64,
    pub mass: f64,
    pub stiffness: f64,
    pub epsilon: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct Spring {
    pub from: f64,
    pub to: f64,
    pub initial_velocity: f64,
    pub params: SpringParams,
}

impl SpringParams {
    pub fn new(damping_ratio: f64, stiffness: f64, epsilon: f64) -> Self {
        let damping_ratio = damping_ratio.max(0.);
        let stiffness = stiffness.max(0.);
        let epsilon = epsilon.max(0.);

        let mass = 1.;
        let critical_damping = 2. * (mass * stiffness).sqrt();
        let damping = damping_ratio * critical_damping;

        Self {
            damping,
            mass,
            stiffness,
            epsilon,
        }
    }
}

impl Spring {
    pub fn value_at(&self, t: Duration) -> f64 {
        self.oscillate(t.as_secs_f64())
    }

    /// Velocity in units per animation-clock second.
    pub fn velocity_at(&self, t: Duration) -> f64 {
        self.velocity_at_seconds(t.as_secs_f64())
    }

    fn velocity_at_seconds(&self, t: f64) -> f64 {
        // The derivative satisfies the same differential equation as displacement.
        let velocity = Self {
            from: self.initial_velocity,
            to: 0.,
            initial_velocity: -(self.params.damping * self.initial_velocity
                + self.params.stiffness * (self.from - self.to))
                / self.params.mass,
            params: self.params,
        };
        velocity.oscillate(t)
    }

    // Based on libadwaita (LGPL-2.1-or-later):
    // https://gitlab.gnome.org/GNOME/libadwaita/-/blob/1.4.4/src/adw-spring-animation.c,
    // which itself is based on (MIT):
    // https://github.com/robb/RBBAnimation/blob/master/RBBAnimation/RBBSpringAnimation.m
    //
    // Comparison with native macOS QuartzCore `durationForEpsilon:` (REA 0x6ded5-0x6e13d):
    // QuartzCore uses a logarithmic decay envelope for underdamped springs and a 0.1s step
    // evaluation loop for critical/overdamped modes. Clarion's implementation improves on this
    // by analytically constraining both position and velocity envelopes simultaneously for
    // underdamped springs, and bisecting monotonic bounds for critical/overdamped responses,
    // guaranteeing that the animation stays settled without stopping prematurely on an initial
    // target crossing.
    /// Computes and returns the duration until the spring is at rest.
    pub fn duration(&self) -> Duration {
        let x = self.from - self.to;
        let v = self.initial_velocity;
        if x == 0. && v == 0. {
            return Duration::ZERO;
        }
        if !x.is_finite()
            || !v.is_finite()
            || !self.params.mass.is_finite()
            || self.params.mass <= 0.
            || !self.params.stiffness.is_finite()
            || self.params.stiffness < 0.
            || !self.params.damping.is_finite()
            || !self.params.epsilon.is_finite()
        {
            return Duration::MAX;
        }
        let beta = self.params.damping / (2. * self.params.mass);
        if beta <= f64::EPSILON || self.params.epsilon <= 0. {
            return Duration::MAX;
        }
        let omega0 = (self.params.stiffness / self.params.mass).sqrt();
        if !beta.is_finite() || !omega0.is_finite() || omega0 <= f64::EPSILON {
            return Duration::MAX;
        }

        // Relative tolerances keep synchronized position/progress animations on
        // the same timeline. Include launch velocity for in-place reversals.
        let scale = x.abs().max(v.abs() / omega0);
        let position_epsilon = self.params.epsilon * scale;
        let velocity_epsilon = position_epsilon * omega0;
        let critical = (beta - omega0).abs() <= f64::from(f32::EPSILON);
        let c = beta * x + v;
        let omega = (omega0 * omega0 - beta * beta).abs().sqrt();
        let slow = -omega0 * omega0 / (beta + omega);
        let fast = -beta - omega;
        let slow_coefficient = if !critical && beta > omega0 {
            (v - fast * x) / (slow - fast)
        } else {
            0.
        };
        let fast_coefficient = x - slow_coefficient;
        let position_amplitude = if !critical && beta < omega0 {
            x.hypot(c / omega)
        } else {
            0.
        };
        let velocity_amplitude = if !critical && beta < omega0 {
            v.hypot((-beta * v - omega0 * omega0 * x) / omega)
        } else {
            0.
        };

        if !critical && beta < omega0 {
            // Both oscillating responses have an exponentially decaying amplitude.
            // Solve their envelopes directly rather than sampling zero crossings
            // or running a binary search when starting an interactive animation.
            // Log differences avoid overflowing amplitude / tolerance.
            let position_log_ratio = position_amplitude.ln() - position_epsilon.ln();
            let velocity_log_ratio = velocity_amplitude.ln() - velocity_epsilon.ln();
            let deadline = position_log_ratio.max(velocity_log_ratio).max(0.) / beta;
            if !deadline.is_finite() || deadline >= Duration::MAX.as_secs_f64() {
                return Duration::MAX;
            }
            // from_secs_f64 rounds to nanoseconds; never finish just before the
            // envelope crosses its tolerance because of downward rounding.
            return Duration::from_secs_f64(deadline).saturating_add(Duration::from_nanos(1));
        }

        // Bound both position and velocity, not a zero crossing. A spring must
        // remain settled after its deadline, including after a fast reversal.
        let settled = |t: f64| {
            let (position, velocity) = if critical {
                let envelope = (-beta * t).exp();
                (
                    (x.abs() + c.abs() * t) * envelope,
                    (v.abs() + beta * c.abs() * t) * envelope,
                )
            } else {
                let a = slow_coefficient.abs() * (slow * t).exp();
                let b = fast_coefficient.abs() * (fast * t).exp();
                (a + b, -slow * a - fast * b)
            };
            position <= position_epsilon && velocity <= velocity_epsilon
        };

        // Past 1/beta even the critical envelopes decrease monotonically.
        let mut low = if critical { 1. / beta } else { 0. };
        let mut high = low.max(1. / beta);
        while !settled(high) {
            high *= 2.;
            if !high.is_finite() || high >= Duration::MAX.as_secs_f64() {
                return Duration::MAX;
            }
        }
        for _ in 0..40 {
            let mid = (low + high) / 2.;
            if settled(mid) {
                high = mid;
            } else {
                low = mid;
            }
        }
        Duration::from_secs_f64(high).saturating_add(Duration::from_nanos(1))
    }

    /// Computes and returns the duration until the spring reaches its target position.
    #[cfg_attr(not(test), allow(dead_code))] // Millisecond compatibility API and scan benchmark.
    pub fn clamped_duration(&self) -> Option<Duration> {
        let beta = self.params.damping / (2. * self.params.mass);

        if beta < 0. {
            return Some(Duration::MAX);
        }

        if (self.to - self.from).abs() <= f64::EPSILON {
            return Some(Duration::ZERO);
        }

        let omega0 = (self.params.stiffness / self.params.mass).sqrt();
        if beta.is_finite()
            && omega0.is_finite()
            && self.params.mass > 0.
            && (self.to - self.from).is_finite()
            && self.initial_velocity.is_finite()
            && self.params.epsilon.is_finite()
            && self.params.epsilon >= 0.
        {
            // Nonoscillating responses cross the one-sided target tolerance at
            // most once. For an underdamped response, its analytic first zero
            // bounds that same monotonic predicate before the next half cycle.
            // Keep the historical integer-ms / 3001-ms clamping contract.
            let reached = |millis: u64| {
                let y = self.oscillate(millis as f64 / 1000.);
                if self.to > self.from {
                    self.to - y <= self.params.epsilon
                } else {
                    y - self.to <= self.params.epsilon
                }
            };
            if reached(1) {
                return Some(Duration::from_millis(1));
            }
            let critical = (beta - omega0).abs() <= f64::from(f32::EPSILON);
            let upper = if critical {
                Some(3001)
            } else if beta >= omega0 {
                // With an exact zero tolerance, rounded addition of decaying
                // modes to a nonzero target can alternate around equality.
                // Preserve the existing first-sample behavior in that case.
                (self.params.epsilon > 0.).then_some(3001)
            } else {
                let omega = (omega0 * omega0 - beta * beta).sqrt();
                // At very high frequencies a full half cycle fits between two
                // integer-ms samples. Retain the scan for that aliasing case.
                if omega <= std::f64::consts::PI * 1000. {
                    let x = self.from - self.to;
                    let c = (beta * x + self.initial_velocity) / omega;
                    let phase = (c * x.signum()).atan2(x.abs());
                    let first_zero = (phase + std::f64::consts::FRAC_PI_2) / omega;
                    Some((first_zero * 1000.).ceil().min(3001.) as u64)
                } else {
                    None
                }
            };
            if let Some(high) = upper {
                let (mut low, mut high) = (1, high.max(1));
                if !reached(high) {
                    return None;
                }
                while high - low > 1 {
                    let mid = (low + high) / 2;
                    if reached(mid) {
                        high = mid;
                    } else {
                        low = mid;
                    }
                }
                return Some(Duration::from_millis(high));
            }
        }

        // Zero damping has no settling deadline, but it still crosses the target. Include it in
        // this search so clamped animations stop at that first crossing instead of oscillating
        // forever. The full duration remains unbounded for users of the complete trajectory.
        // The first frame is not that important and we avoid finding the trivial 0 for in-place
        // animations.
        let mut i = 1u16;
        let mut y = self.oscillate(f64::from(i) / 1000.);

        while (self.to - self.from > f64::EPSILON && self.to - y > self.params.epsilon)
            || (self.from - self.to > f64::EPSILON && y - self.to > self.params.epsilon)
        {
            if i > 3000 {
                return None;
            }

            i += 1;
            y = self.oscillate(f64::from(i) / 1000.);
        }

        Some(Duration::from_millis(u64::from(i)))
    }

    /// First one-sided target arrival at animation-clock precision.
    ///
    /// The public millisecond-sampled helper above is retained for callers that
    /// need its historical contract. Rendering must use this deadline instead:
    /// a high-refresh frame can sample an overshoot before the next integer ms.
    /// Underdamped springs use their analytic first lobe, without sampling
    /// aliases even when an entire oscillation fits inside a millisecond.
    /// Invalid or noncrossing zero-tolerance responses return None, so Animation
    /// can retain its full settling deadline.
    pub(super) fn precise_clamped_duration(&self) -> Option<Duration> {
        let x = self.from - self.to;
        if x == 0. {
            return Some(Duration::ZERO);
        }
        if !x.is_finite()
            || !self.initial_velocity.is_finite()
            || !self.params.mass.is_finite()
            || self.params.mass <= 0.
            || !self.params.stiffness.is_finite()
            || self.params.stiffness <= 0.
            || !self.params.damping.is_finite()
            || self.params.damping < 0.
            || !self.params.epsilon.is_finite()
            || self.params.epsilon < 0.
        {
            return None;
        }
        let beta = self.params.damping / (2. * self.params.mass);
        let omega0 = (self.params.stiffness / self.params.mass).sqrt();
        if !beta.is_finite() || !omega0.is_finite() || omega0 <= 0. {
            return None;
        }
        // Evaluate displacement about zero so a large translated target does
        // not round a small residual to zero before it actually arrives.
        let normalized = Self {
            from: x.abs(),
            to: 0.,
            initial_velocity: self.initial_velocity * x.signum(),
            params: self.params,
        };
        let a = normalized.from;
        let v = normalized.initial_velocity;
        let epsilon = if a <= self.params.epsilon {
            if v == 0. {
                // Retain the distinct starting pose for this event-loop cycle.
                return Some(Duration::from_nanos(1));
            }
            // A retarget can start close to its endpoint while still moving
            // away from it. Position tolerance alone must not erase that
            // launch momentum. Wait for physical arrival (or full settling
            // for a nonoscillating response that never crosses the target).
            0.
        } else {
            self.params.epsilon
        };
        let critical = (beta - omega0).abs() <= f64::from(f32::EPSILON);
        let first_zero = if critical {
            let c = beta * a + v;
            (c < 0.).then(|| -a / c)
        } else if beta < omega0 {
            let omega = (omega0 * omega0 - beta * beta).sqrt();
            let b = (beta * a + v) / omega;
            // atan2(a, -b) avoids subtracting nearly equal pi/2 values
            // when a very fast launch crosses almost immediately.
            Some(a.atan2(-b) / omega)
        } else {
            let omega = (beta * beta - omega0 * omega0).sqrt();
            let slow = -omega0 * omega0 / (beta + omega);
            let fast = -beta - omega;
            let slow_coefficient = (v - fast * a) / (slow - fast);
            let fast_coefficient = a - slow_coefficient;
            (slow_coefficient < 0. && fast_coefficient > 0.)
                .then(|| (fast_coefficient.ln() - (-slow_coefficient).ln()) / (slow - fast))
        };
        let ceil_duration = |seconds: f64| {
            if !seconds.is_finite() || seconds < 0. || seconds >= Duration::MAX.as_secs_f64() {
                return None;
            }
            let duration = Duration::from_secs_f64(seconds);
            Some(
                if duration.as_secs_f64() < seconds {
                    duration.saturating_add(Duration::from_nanos(1))
                } else {
                    duration
                }
                .max(Duration::from_nanos(1)),
            )
        };
        if epsilon == 0. {
            return first_zero.and_then(ceil_duration);
        }
        let mut high = first_zero.map_or(1. / omega0, |zero| {
            // A few ulps past the analytic zero bound tolerates cancellation
            // in sin/cos or the two decaying modes for very large amplitudes.
            zero + (zero.abs() * 1e-14).max(1e-12)
        });
        while normalized.oscillate(high) > epsilon {
            high *= 2.;
            ceil_duration(high)?;
        }
        ceil_duration(high)?;
        let mut low = 0.;
        let mut candidate = high;
        // Newton normally takes only a handful of evaluations. Keep a bracket
        // for launches away from the target and near-critical/flat responses.
        for _ in 0..64 {
            let value = normalized.oscillate(candidate) - epsilon;
            if !value.is_finite() {
                return None;
            }
            if value > 0. {
                low = candidate;
            } else {
                high = candidate;
            }
            if high - low <= 0.25e-9 {
                break;
            }
            let derivative = normalized.velocity_at_seconds(candidate);
            if derivative < 0. && value.abs() <= derivative.abs() * 1e-9 {
                // Newton can converge on one side of the root while the other
                // bracket endpoint is still far away. Check the adjacent
                // integer nanoseconds directly rather than bisecting that
                // already-converged tail another twenty times.
                let nearest = ceil_duration(candidate)?;
                for deadline in [nearest, nearest.saturating_add(Duration::from_nanos(1))] {
                    let before = deadline.saturating_sub(Duration::from_nanos(1));
                    if normalized.oscillate(deadline.as_secs_f64()) <= epsilon
                        && normalized.oscillate(before.as_secs_f64()) > epsilon
                    {
                        return Some(deadline);
                    }
                }
            }
            let next = candidate - value / derivative;
            candidate = if derivative < 0. && next > low && next < high {
                next
            } else {
                (low + high) / 2.
            };
        }
        ceil_duration(high)
    }

    /// Returns the spring position at a given time in seconds.
    fn oscillate(&self, t: f64) -> f64 {
        let b = self.params.damping;
        let m = self.params.mass;
        let k = self.params.stiffness;
        let v0 = self.initial_velocity;

        let beta = b / (2. * m);
        let omega0 = (k / m).sqrt();

        let x0 = self.from - self.to;

        let envelope = (-beta * t).exp();

        // Solutions of the form C1*e^(lambda1*x) + C2*e^(lambda2*x)
        // for the differential equation m*ẍ+b*ẋ+kx = 0

        // f64::EPSILON is too small for this specific comparison, so we use
        // f32::EPSILON even though it's doubles.
        if (beta - omega0).abs() <= f64::from(f32::EPSILON) {
            // Critically damped.
            self.to + envelope * (x0 + (beta * x0 + v0) * t)
        } else if beta < omega0 {
            // Underdamped.
            let omega1 = ((omega0 * omega0) - (beta * beta)).sqrt();

            self.to
                + envelope
                    * (x0 * (omega1 * t).cos() + ((beta * x0 + v0) / omega1) * (omega1 * t).sin())
        } else {
            // Overdamped.
            let omega2 = ((beta * beta) - (omega0 * omega0)).sqrt();

            // Evaluate the two decaying modes directly. exp(-beta*t)*cosh()
            // otherwise becomes 0*infinity for heavily overdamped springs.
            let slow = -omega0 * omega0 / (beta + omega2);
            let fast = -beta - omega2;
            let a = (v0 - fast * x0) / (slow - fast);
            let b = x0 - a;
            self.to + a * (slow * t).exp() + b * (fast * t).exp()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Preserve the previous millisecond sampling contract while optimizing the search.
    fn scanned_clamped_duration(spring: &Spring) -> Option<Duration> {
        let beta = spring.params.damping / (2. * spring.params.mass);
        if beta < 0. {
            return Some(Duration::MAX);
        }
        if (spring.to - spring.from).abs() <= f64::EPSILON {
            return Some(Duration::ZERO);
        }
        for millis in 1..=3001 {
            let y = spring.value_at(Duration::from_millis(millis));
            if !((spring.to - spring.from > f64::EPSILON && spring.to - y > spring.params.epsilon)
                || (spring.from - spring.to > f64::EPSILON
                    && y - spring.to > spring.params.epsilon))
            {
                return Some(Duration::from_millis(millis));
            }
        }
        None
    }

    #[test]
    fn undamped_spring_crosses_the_target_without_ever_settling() {
        for stiffness in [1., 50., 400., 1000.] {
            for (from, to) in [(0., 1.), (1., 0.)] {
                let spring = Spring {
                    from,
                    to,
                    initial_velocity: 0.,
                    params: SpringParams::new(0., stiffness, 0.),
                };
                // With no initial velocity or damping, x(t) = x(0) * cos(sqrt(k) * t).
                let first_crossing =
                    Duration::from_secs_f64(std::f64::consts::FRAC_PI_2 / stiffness.sqrt());
                let clamped = spring.clamped_duration().unwrap();
                assert_eq!(spring.duration(), Duration::MAX);
                assert!(clamped >= first_crossing);
                assert!(clamped - first_crossing < Duration::from_millis(1));
                let before = spring.value_at(clamped - Duration::from_millis(1));
                let after = spring.value_at(clamped);
                if to > from {
                    assert!(before < to && after >= to);
                } else {
                    assert!(before > to && after <= to);
                }
            }
        }
    }

    #[test]
    fn clamped_search_preserves_millisecond_deadlines_with_reversals() {
        for damping in [1., 1.00001, 1.15, 6., 10.] {
            for stiffness in [1., 50., 800., 1000., 100_000.] {
                for distance in [-2400., -1., -0.001, 0.001, 1., 2400.] {
                    for velocity in [-6000., -20., 0., 20., 6000.] {
                        for epsilon in [0., 0.0001, 0.01, 1.] {
                            let spring = Spring {
                                from: 17. + distance,
                                to: 17.,
                                initial_velocity: velocity,
                                params: SpringParams::new(damping, stiffness, epsilon),
                            };
                            assert_eq!(
                                spring.clamped_duration(),
                                scanned_clamped_duration(&spring),
                                "{spring:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn clamped_search_keeps_oscillating_and_degenerate_spring_behavior() {
        for damping in [0., 0.1, 0.6, 0.99999, 1., 1.00001] {
            for stiffness in [0., 0.00001, 800., 100_000.] {
                for distance in [0., 0.000001, 1., -1.] {
                    for velocity in [-100., 0., 100.] {
                        let spring = Spring {
                            from: distance,
                            to: 0.,
                            initial_velocity: velocity,
                            params: SpringParams::new(damping, stiffness, 0.0001),
                        };
                        assert_eq!(
                            spring.clamped_duration(),
                            scanned_clamped_duration(&spring),
                            "{spring:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn clamped_search_keeps_the_3001_millisecond_boundary() {
        // A critically damped, stationary launch crosses the supplied tolerance at t.
        // Put t midway between integer samples to avoid floating-point tie ambiguity.
        for crossing_ms in [0.5, 2999.5, 3000.5, 3001.5] {
            let t = crossing_ms / 1000.;
            let mut spring = Spring {
                from: 1.,
                to: 0.,
                initial_velocity: 0.,
                params: SpringParams::new(1., 1., 0.0001),
            };
            spring.params.epsilon = spring.oscillate(t);
            let expected = if crossing_ms > 3001. {
                None
            } else {
                Some(Duration::from_millis(crossing_ms.ceil() as u64))
            };
            assert_eq!(scanned_clamped_duration(&spring), expected);
            assert_eq!(spring.clamped_duration(), expected);
        }
    }

    #[test]
    #[ignore = "CPU microbenchmark; run with an optimized test binary"]
    fn clamped_search_initialization_microbenchmark() {
        use std::hint::black_box;
        use std::time::Instant;

        let springs: Vec<_> = (0..128)
            .map(|i| Spring {
                from: 0.,
                to: 1. + f64::from(i % 8) * 100.,
                initial_velocity: f64::from(i % 5) * 20. - 40.,
                params: SpringParams::new(1., 800., 0.0001),
            })
            .collect();
        let mut scanned = Vec::new();
        let mut searched = Vec::new();
        for round in 0..10 {
            // Alternate order to reduce bias from warm caches and CPU frequency changes.
            for old in if round % 2 == 0 {
                [true, false]
            } else {
                [false, true]
            } {
                let start = Instant::now();
                for _ in 0..20 {
                    for spring in &springs {
                        black_box(if old {
                            scanned_clamped_duration(black_box(spring))
                        } else {
                            black_box(spring).clamped_duration()
                        });
                    }
                }
                let elapsed = start.elapsed().as_secs_f64() * 1_000_000. / 20.;
                if old {
                    scanned.push(elapsed);
                } else {
                    searched.push(elapsed);
                }
            }
        }
        scanned.sort_by(f64::total_cmp);
        searched.sort_by(f64::total_cmp);
        println!(
            "128 spring clamped deadlines, median us: scanned={:.3}, searched={:.3}",
            scanned[5], searched[5]
        );
    }

    #[test]
    fn spring_settles_in_position_and_velocity_and_stays_settled() {
        for damping in [0.1, 0.6, 0.99999, 1., 1.00001, 1.15, 6., 10.] {
            for distance in [0., 0.01, 1., 2400.] {
                for velocity in [-6000., -20., 0., 20., 6000.] {
                    let spring = Spring {
                        from: distance,
                        to: 0.,
                        initial_velocity: velocity,
                        params: SpringParams::new(damping, 800., 0.0001),
                    };
                    let duration = spring.duration();
                    assert!(duration < Duration::from_secs(60));
                    let omega = 800f64.sqrt();
                    let tolerance = 0.0001 * distance.abs().max(velocity.abs() / omega);
                    for extra_ms in [0, 1, 16, 100, 1000, 10_000] {
                        let t = duration + Duration::from_millis(extra_ms);
                        assert!(
                            spring.value_at(t).abs() <= tolerance * 1.000001 + 1e-10,
                            "damping={damping}, distance={distance}, velocity={velocity}, t={t:?}"
                        );
                        assert!(
                            spring.velocity_at(t).abs() <= tolerance * omega * 1.000001 + 1e-10
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn underdamped_deadline_is_the_analytic_position_and_velocity_envelope() {
        for damping in [0.1, 0.5, 0.92, 0.99999] {
            for stiffness in [1., 100., 300., 100_000.] {
                for (distance, velocity) in [(1., 0.), (-1., 7.), (0., -17.), (2400., -6000.)] {
                    let spring = Spring {
                        from: distance,
                        to: 0.,
                        initial_velocity: velocity,
                        params: SpringParams::new(damping, stiffness, 0.001),
                    };
                    let omega0 = stiffness.sqrt();
                    let beta = damping * omega0;
                    let omega = (stiffness - beta * beta).sqrt();
                    let scale = distance.abs().max(velocity.abs() / omega0);
                    let position = distance.hypot((beta * distance + velocity) / omega);
                    let speed = velocity.hypot((-beta * velocity - stiffness * distance) / omega);
                    let expected = (position / (scale * 0.001))
                        .ln()
                        .max((speed / (scale * 0.001 * omega0)).ln())
                        / beta;
                    let actual = spring.duration().as_secs_f64();
                    assert!(actual >= expected - 1e-12);
                    assert!((actual - expected).abs() < 2e-9, "{spring:?}");
                }
            }
        }
    }

    #[test]
    fn invalid_or_nonconverging_springs_have_no_finite_settling_deadline() {
        let spring = Spring {
            from: 0.,
            to: 1.,
            initial_velocity: 0.,
            params: SpringParams::new(0.92, 300., 0.001),
        };
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut s = spring;
            s.initial_velocity = bad;
            assert_eq!(s.duration(), Duration::MAX);
            s = spring;
            s.params.epsilon = bad;
            assert_eq!(s.duration(), Duration::MAX);
            s = spring;
            s.params.mass = bad;
            assert_eq!(s.duration(), Duration::MAX);
            s = spring;
            s.params.stiffness = bad;
            assert_eq!(s.duration(), Duration::MAX);
            s = spring;
            s.params.damping = bad;
            assert_eq!(s.duration(), Duration::MAX);
        }
        for mass in [0., -1.] {
            let mut s = spring;
            s.params.mass = mass;
            assert_eq!(s.duration(), Duration::MAX);
        }
    }

    #[test]
    fn precise_arrival_does_not_alias_high_frequency_zero_tolerance_springs() {
        for damping in [0., 0.2, 0.92, 0.99999] {
            for stiffness in [100f64, 300., 100_000_000., 4_000_000_000.] {
                let omega0 = stiffness.sqrt();
                let beta = damping * omega0;
                let omega = (omega0 * omega0 - beta * beta).sqrt();
                for velocity in [-1e10, -6000., 0., 6000.] {
                    let spring = Spring {
                        from: 1.,
                        to: 0.,
                        initial_velocity: velocity,
                        params: SpringParams::new(damping, stiffness, 0.),
                    };
                    let angle = 1f64.atan2(-(beta + velocity) / omega);
                    let expected = angle / omega;
                    let arrival = spring.precise_clamped_duration().unwrap();
                    assert!(
                        arrival.as_secs_f64() >= expected - 1e-15,
                        "{spring:?}, expected={expected}, arrival={arrival:?}"
                    );
                    assert!(arrival.as_secs_f64() - expected < 1e-9 + 1e-15);
                    assert!(arrival > Duration::ZERO);
                    assert!(arrival < Duration::from_secs_f64(std::f64::consts::PI / omega));
                }
            }
        }
    }

    #[test]
    fn precise_positive_tolerance_arrival_refines_between_integer_milliseconds() {
        for damping in [0., 0.2, 0.92, 0.99999, 1., 1.00001, 6., 10.] {
            for stiffness in [1., 300., 100_000_000.] {
                for (distance, velocity) in [(1., 0.), (-1., 6000.), (0.001, -6000.)] {
                    let spring = Spring {
                        from: distance,
                        to: 0.,
                        initial_velocity: velocity,
                        params: SpringParams::new(damping, stiffness, 0.0001),
                    };
                    let arrival = spring.precise_clamped_duration().unwrap();
                    let residual = spring.value_at(arrival) * distance.signum();
                    assert!(
                        residual <= spring.params.epsilon + 1e-9,
                        "{spring:?}, {arrival:?}, {residual}"
                    );
                    if arrival >= Duration::from_nanos(2) {
                        let before =
                            spring.value_at(arrival - Duration::from_nanos(2)) * distance.signum();
                        assert!(
                            before > spring.params.epsilon - 1e-9,
                            "{spring:?}, {arrival:?}, {before}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn precise_arrival_handles_stationary_and_nonoscillating_zero_tolerance_launches() {
        let mut spring = Spring {
            from: 1.,
            to: 0.,
            initial_velocity: 0.,
            params: SpringParams::new(1., 100., 0.),
        };
        assert_eq!(spring.precise_clamped_duration(), None);
        spring.initial_velocity = -30.;
        assert_eq!(
            spring.precise_clamped_duration(),
            Some(Duration::from_millis(50))
        );
        spring.params = SpringParams::new(2., 100., 0.);
        assert!(spring.precise_clamped_duration().is_none());
        spring.initial_velocity = -100.;
        assert!(spring.precise_clamped_duration().is_some());
        spring.from = 0.;
        assert_eq!(spring.precise_clamped_duration(), Some(Duration::ZERO));
        spring.from = 0.00001;
        spring.params.epsilon = 0.0001;
        assert!(spring.precise_clamped_duration().unwrap() > Duration::ZERO);
        spring.initial_velocity = 0.;
        assert_eq!(
            spring.precise_clamped_duration(),
            Some(Duration::from_nanos(1))
        );
    }

    #[test]
    fn near_target_arrival_preserves_nonzero_launch_momentum() {
        for damping in [0., 0.92, 1., 1.00001, 6.] {
            for distance in [f64::EPSILON / 2., 0.0004, -0.0004] {
                for speed in [-0.1, 0.1] {
                    let spring = Spring {
                        from: distance,
                        to: 0.,
                        initial_velocity: speed,
                        params: SpringParams::new(damping, 300., 0.001),
                    };
                    if let Some(arrival) = spring.precise_clamped_duration() {
                        assert!(arrival > Duration::ZERO, "{spring:?}");
                        if speed * distance > 0. {
                            assert!(
                                arrival > Duration::from_micros(1),
                                "{spring:?}, {arrival:?}"
                            );
                        }
                    } else {
                        assert!(damping >= 1. && speed * distance > 0.);
                    }
                }
            }
        }
    }

    #[test]
    fn stiff_overdamped_deadline_rounds_after_the_envelope_crossing() {
        let spring = Spring {
            from: 520_871.752_343_312_6,
            to: 685_774.328_709_996_9,
            initial_velocity: -68_398_510.883_382_25,
            params: SpringParams {
                damping: 175_990.727_653_380_43,
                mass: 1.,
                stiffness: 381_527_179.013_991_53,
                epsilon: 0.000_257_212_131_785_850_4,
            },
        };
        let tolerance = (spring.from - spring.to).abs() * spring.params.epsilon;
        let deadline = spring.duration();
        assert!((spring.value_at(deadline) - spring.to).abs() <= tolerance);
    }

    #[test]
    fn underdamped_clamped_search_keeps_first_sample_with_fast_launches() {
        for damping in [0., 0.1, 0.5, 0.92, 0.99999] {
            for stiffness in [0.00001, 100., 300., 100_000., 100_000_000.] {
                for distance in [-2400., -1., -0.001, 0.001, 1., 2400.] {
                    for velocity in [-6000., -20., 0., 20., 6000.] {
                        let spring = Spring {
                            from: distance,
                            to: 0.,
                            initial_velocity: velocity,
                            params: SpringParams::new(damping, stiffness, 0.001),
                        };
                        assert_eq!(
                            spring.clamped_duration(),
                            scanned_clamped_duration(&spring),
                            "{spring:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn spring_velocity_matches_position_derivative() {
        for damping in [0.6, 1., 1.15, 6., 10.] {
            let spring = Spring {
                from: -25.,
                to: 70.,
                initial_velocity: -90.,
                params: SpringParams::new(damping, 800., 0.0001),
            };
            assert!((spring.velocity_at(Duration::ZERO) + 90.).abs() < 1e-8);
            for milliseconds in [5, 30, 100, 500] {
                let t = Duration::from_millis(milliseconds);
                let delta = Duration::from_micros(1);
                let derivative = (spring.value_at(t + delta) - spring.value_at(t - delta))
                    / (2. * delta.as_secs_f64());
                assert!((spring.velocity_at(t) - derivative).abs() < 1e-4);
            }
        }
    }

    #[test]
    fn spring_duration_preserves_synchronized_motion_at_different_scales() {
        for damping in [0.6, 1., 1.15] {
            let mut spring = Spring {
                from: 0.,
                to: 1.,
                initial_velocity: 0.,
                params: SpringParams::new(damping, 800., 0.0001),
            };
            let duration = spring.duration();
            for to in [-2400., -0.1, 100., 3840.] {
                spring.to = to;
                assert!(spring.duration().abs_diff(duration) <= Duration::from_nanos(1));
            }
        }
    }

    #[test]
    fn overdamped_spring_equal_from_to_nan() {
        let spring = Spring {
            from: 0.,
            to: 0.,
            initial_velocity: 0.,
            params: SpringParams::new(1.15, 850., 0.0001),
        };
        let _ = spring.duration();
        let _ = spring.clamped_duration();
        let _ = spring.value_at(Duration::ZERO);
    }

    #[test]
    fn overdamped_spring_duration_panic() {
        let spring = Spring {
            from: 0.,
            to: 1.,
            initial_velocity: 0.,
            params: SpringParams::new(6., 1200., 0.0001),
        };
        let _ = spring.duration();
        let _ = spring.clamped_duration();
        let _ = spring.value_at(Duration::ZERO);
    }
}
