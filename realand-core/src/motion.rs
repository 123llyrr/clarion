//! Device-independent cubic timing evaluation.
//!
//! QuartzCore sample evidence: CA::Render::TimingFunction::evaluate,
//! 0x1325b..0x1342a (Catalina recovery sample, see the 2026-10-13 evidence
//! index). It inverts x with at most eight Newton steps and a bisection
//! fallback. This implementation uses its own bounded solver and tolerance;
//! the generic mechanism does not identify a private Dock animation curve.

#[derive(Debug, Clone, Copy)]
pub struct CubicTiming {
    points: [f64; 4],
    x: [f64; 3],
    y: [f64; 3],
}

impl CubicTiming {
    pub const fn new(x1: f64, y1: f64, x2: f64, y2: f64) -> Self {
        const fn coefficients(a: f64, b: f64) -> [f64; 3] {
            [3. * a, 3. * b - 6. * a, 1. + 3. * a - 3. * b]
        }
        Self {
            points: [x1, y1, x2, y2],
            x: coefficients(x1, x2),
            y: coefficients(y1, y2),
        }
    }

    pub const fn control_points(self) -> ((f64, f64), (f64, f64)) {
        (
            (self.points[0], self.points[1]),
            (self.points[2], self.points[3]),
        )
    }

    pub fn x_at(self, t: f64) -> f64 {
        Self::polynomial(self.x, t)
    }
    pub fn y_at(self, t: f64) -> f64 {
        Self::polynomial(self.y, t)
    }

    fn polynomial(c: [f64; 3], t: f64) -> f64 {
        t * (c[0] + t * (c[1] + t * c[2]))
    }

    fn derivative(self, t: f64) -> f64 {
        self.x[0] + t * (2. * self.x[1] + t * 3. * self.x[2])
    }

    pub fn sample(self, fraction: f64) -> f64 {
        self.sample_with_epsilon(fraction, 1e-14)
    }

    pub fn sample_with_epsilon(self, fraction: f64, epsilon: f64) -> f64 {
        if fraction.is_nan() || fraction <= f64::EPSILON {
            return 0.;
        }
        if fraction >= 1. - f64::EPSILON {
            return 1.;
        }
        let tolerance = if epsilon.is_finite() {
            epsilon.abs().clamp(1e-15, 1e-6)
        } else {
            1e-14
        };
        let (mut lo, mut hi, mut t) = (0., 1., fraction);
        for _ in 0..8 {
            let error = self.x_at(t) - fraction;
            if error.abs() <= tolerance {
                return self.y_at(t);
            }
            if error < 0. {
                lo = t;
            } else {
                hi = t;
            }
            let slope = self.derivative(t);
            if slope.abs() < 1e-6 {
                break;
            }
            let next = t - error / slope;
            if !next.is_finite() || next <= lo || next >= hi {
                break;
            }
            t = next;
        }
        for _ in 0..64 {
            t = (lo + hi) * 0.5;
            let error = self.x_at(t) - fraction;
            if error.abs() <= tolerance || t == lo || t == hi {
                return self.y_at(t);
            }
            if error < 0. {
                lo = t;
            } else {
                hi = t;
            }
        }
        self.y_at((lo + hi) * 0.5)
    }

    /// First time fraction reaching y=1, including overshoot and tangency.
    pub fn first_target_fraction(self) -> f64 {
        let a = 1. + 3. * (self.points[1] - self.points[3]);
        let b = 1. - 3. * self.points[1];
        let mut roots = [1., 1.];
        if a == 0. {
            roots[0] = -1. / b;
        } else {
            let discriminant = b.mul_add(b, -4. * a);
            if discriminant >= 0. {
                let q = -0.5 * (b + discriminant.sqrt().copysign(b));
                roots = [q / a, 1. / q];
            }
        }
        let first = roots
            .into_iter()
            .filter(|t| *t > 0. && *t < 1.)
            .fold(1., f64::min);
        self.x_at(first)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inverse_matches_an_independent_binary_search() {
        for points in [
            [0.25, 0.1, 0.25, 1.],
            [0., 0., 0., 1.],
            [1., 0., 0., 1.],
            [0.68, -0.55, 0.265, 1.55],
        ] {
            let c = CubicTiming::new(points[0], points[1], points[2], points[3]);
            for n in 1..1000 {
                let x = f64::from(n) / 1000.;
                let (mut a, mut b) = (0., 1.);
                for _ in 0..64 {
                    let mid = (a + b) / 2.;
                    if c.x_at(mid) < x {
                        a = mid;
                    } else {
                        b = mid;
                    }
                }
                // At the central flat point floating-point x(t) has a plateau.
                // Use its analytic inverse rather than treating the left edge of
                // that rounding plateau as the unique mathematical solution.
                let expected_t = if points == [1., 0., 0., 1.] {
                    0.5 + ((x - 0.5) / 4.).cbrt()
                } else {
                    (a + b) / 2.
                };
                assert!(
                    (c.sample(x) - c.y_at(expected_t)).abs() < 1e-8,
                    "points={points:?}, x={x}, actual={}, expected={}",
                    c.sample(x),
                    c.y_at(expected_t)
                );
            }
        }
    }
    #[test]
    fn overshoot_is_preserved_and_invalid_time_is_bounded() {
        let c = CubicTiming::new(0.2, 1.5, 0.5, 1.5);
        assert!(c.sample(0.6) > 1.);
        assert!(c.first_target_fraction() < 1.);
        assert_eq!(c.sample(f64::NAN), 0.);
        assert_eq!(c.sample(f64::INFINITY), 1.);
        assert_eq!(c.sample(f64::NEG_INFINITY), 0.);
        assert_eq!(c.sample(-1.), 0.);
        assert_eq!(c.sample(2.), 1.);
    }
}
