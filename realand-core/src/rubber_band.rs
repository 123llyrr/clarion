//! Pure bounded overscroll response, shared by every gesture adapter.

#[derive(Debug, Clone, Copy)]
pub struct RubberBand {
    pub stiffness: f64,
    pub limit: f64,
}

impl RubberBand {
    pub fn band(&self, x: f64) -> f64 {
        if !self.stiffness.is_finite()
            || self.stiffness <= 0.
            || !self.limit.is_finite()
            || self.limit <= 0.
            || x.is_nan()
            || x < 0.
        {
            return 0.;
        }
        let c = self.stiffness;
        let d = self.limit;

        (1. - (1. / (x * c / d + 1.))) * d
    }

    pub fn derivative(&self, x: f64) -> f64 {
        if !self.stiffness.is_finite()
            || self.stiffness <= 0.
            || !self.limit.is_finite()
            || self.limit <= 0.
            || !x.is_finite()
            || x < 0.
        {
            return 0.;
        }
        let c = self.stiffness;
        let d = self.limit;

        let value = c * d * d / (c * x + d).powi(2);
        if value.is_finite() {
            value
        } else {
            0.
        }
    }

    pub fn clamp(&self, min: f64, max: f64, x: f64) -> f64 {
        if !min.is_finite() || !max.is_finite() || min > max {
            return 0.;
        }
        if !x.is_finite() {
            return min;
        }
        let clamped = x.clamp(min, max);
        let sign = if x < clamped { -1. } else { 1. };
        let diff = (x - clamped).abs();

        clamped + sign * self.band(diff)
    }

    pub fn clamp_derivative(&self, min: f64, max: f64, x: f64) -> f64 {
        if !min.is_finite() || !max.is_finite() || min > max || !x.is_finite() {
            return 0.;
        }
        if min <= x && x <= max {
            return 1.;
        }

        let clamped = x.clamp(min, max);
        let diff = (x - clamped).abs();
        self.derivative(diff)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zero_limit_and_invalid_input_never_poison_animation_geometry() {
        for limit in [0., -1., f64::NAN, f64::INFINITY] {
            let band = RubberBand {
                stiffness: 0.55,
                limit,
            };
            assert_eq!(band.band(100.), 0.);
            assert_eq!(band.derivative(100.), 0.);
            assert_eq!(band.clamp(0., 10., 20.), 10.);
        }
        let band = RubberBand {
            stiffness: 0.55,
            limit: 100.,
        };
        assert_eq!(band.clamp(0., 10., f64::NAN), 0.);
        assert_eq!(band.clamp(10., 0., 20.), 0.);
        assert_eq!(band.clamp_derivative(0., 10., f64::INFINITY), 0.);
    }
}
