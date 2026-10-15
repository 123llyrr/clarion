use keyframe::EasingFunction;

#[derive(Debug, Clone, Copy)]
pub struct CubicBezier(realand_core::motion::CubicTiming);

impl CubicBezier {
    // Established Realand presets; decimal values preserve configuration behavior.
    // The recovered generic Core Animation table uses float32 values. These
    // names do not establish which timing function private Dock animations use.
    pub const MACOS_DEFAULT: Self = Self::new(0.25, 0.1, 0.25, 1.);
    pub const CA_DEFAULT: Self = Self::MACOS_DEFAULT;
    pub const MACOS_EASE_IN: Self = Self::new(0.42, 0., 1., 1.);
    pub const CA_EASE_IN: Self = Self::MACOS_EASE_IN;
    pub const MACOS_EASE_OUT: Self = Self::new(0., 0., 0.58, 1.);
    pub const CA_EASE_OUT: Self = Self::MACOS_EASE_OUT;
    pub const MACOS_EASE_IN_EASE_OUT: Self = Self::new(0.42, 0., 0.58, 1.);
    pub const CA_EASE_IN_EASE_OUT: Self = Self::MACOS_EASE_IN_EASE_OUT;
    pub const MACOS_LINEAR: Self = Self::new(0., 0., 1., 1.);
    pub const CA_LINEAR: Self = Self::MACOS_LINEAR;

    pub const fn new(x1: f64, y1: f64, x2: f64, y2: f64) -> Self {
        Self(realand_core::motion::CubicTiming::new(x1, y1, x2, y2))
    }

    pub const fn control_points(&self) -> ((f64, f64), (f64, f64)) {
        self.0.control_points()
    }

    pub(super) const fn into_core(self) -> realand_core::motion::CubicTiming {
        self.0
    }

    #[cfg(test)]
    fn x_for_t(&self, t: f64) -> f64 {
        self.0.x_at(t)
    }
    #[cfg(test)]
    fn y_for_t(&self, t: f64) -> f64 {
        self.0.y_at(t)
    }
}

impl EasingFunction for CubicBezier {
    fn y(&self, x: f64) -> f64 {
        self.0.sample(x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverse_matches_precise_bisection_for_flat_and_steep_curves() {
        let curves = [
            (0., 0., 0.58, 1.),
            (0.15, 0., 0.05, 1.),
            (0.68, -0.55, 0.265, 1.55),
            (0., 0., 0., 1.),
            (1., 0., 1., 1.),
        ];
        for (x1, y1, x2, y2) in curves {
            let bezier = CubicBezier::new(x1, y1, x2, y2);
            for index in 1..1000 {
                let x = f64::from(index) / 1000.;
                let mut low = 0.;
                let mut high = 1.;
                for _ in 0..60 {
                    let mid = (low + high) / 2.;
                    if bezier.x_for_t(mid) < x {
                        low = mid;
                    } else {
                        high = mid;
                    }
                }
                let expected = bezier.y_for_t((low + high) / 2.);
                assert!(
                    (bezier.y(x) - expected).abs() < 1e-8,
                    "curve=({x1}, {y1}, {x2}, {y2}), x={x}"
                );
            }
        }
    }

    #[test]
    fn inverse_handles_zero_derivative_in_the_middle() {
        let bezier = CubicBezier::new(1., 0., 0., 1.);
        for index in 1..1000 {
            let x = f64::from(index) / 1000.;
            // For these x control points, x(t) = 0.5 + 4 * (t - 0.5)^3.
            let expected_t = 0.5 + ((x - 0.5) / 4.).cbrt();
            let expected = bezier.y_for_t(expected_t);
            assert!((bezier.y(x) - expected).abs() < 1e-8, "x={x}");
        }
    }
}
