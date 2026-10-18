//! Geometry-dependent phase scheduling recovered from Big Sur 11.7.2's Dock.
//!
//! This independently written model covers phase timing, not the complete warp
//! contour. Degenerate geometry is left to the caller's existing fallback.

/// Half-cosine timing on the unit interval. Invalid time starts at rest.
pub fn half_cosine(u: f64) -> f64 {
    let u = if u.is_nan() { 0. } else { u.clamp(0., 1.) };
    0.5 - 0.5 * (std::f64::consts::PI * u).cos()
}

#[derive(Debug, Clone, Copy)]
pub struct GeniePhases {
    cross_end: f64,
    travel_duration: f64,
    travel_offset: f64,
    duration: f64,
}

impl GeniePhases {
    /// Construct from canonical, signed geometry: transverse edge deltas,
    /// inter-rectangle gap, longitudinal travel, and source longitudinal extent.
    /// The direction adapter owns that canonicalization. Invalid/degenerate
    /// geometry returns `None` rather than guessing a replacement warp.
    pub fn new(left: f64, right: f64, gap: f64, travel: f64, extent: f64) -> Option<Self> {
        if ![left, right, gap, travel, extent]
            .iter()
            .all(|x| x.is_finite())
        {
            return None;
        }
        let a = (1. + 0.5 * ((left.abs() + right.abs()) * 0.125).max(2. * gap.abs()))
            .ln()
            .trunc()
            * 50.;
        let b = (1. + 0.5 * travel.abs()).ln().trunc() * 50.;
        if travel == 0. || b == 0. {
            return None;
        }
        let argument = 1. - 2. * (travel - extent).abs() / travel.abs();
        if !(-1. ..=1.).contains(&argument) {
            return None;
        }
        // Retain the original scalar operation order, before cancellation.
        let argument = 1. - 2. * ((b * (travel - extent).abs() / travel.abs()) / b);
        if !(-1. ..=1.).contains(&argument) {
            return None;
        }
        let c = b * (argument.acos() / std::f64::consts::PI);
        let cross_end = a.max(c);
        let travel_offset = if a > c { a - c } else { 0. };
        let duration = b + travel_offset;
        if cross_end <= 0.
            || ![a, b, c, cross_end, travel_offset, duration]
                .iter()
                .all(|v| v.is_finite())
        {
            return None;
        }
        Some(Self {
            cross_end,
            travel_duration: b,
            travel_offset,
            duration,
        })
    }

    /// Returns `(transverse_deformation, longitudinal_travel)` at normalized
    /// progress. The original block stores progress as float32 before sampling.
    pub fn sample(self, progress: f64) -> (f64, f64) {
        let progress = if progress.is_nan() {
            0.
        } else {
            progress.clamp(0., 1.)
        };
        let q = self.duration * f64::from(progress as f32);
        (
            half_cosine(q / self.cross_end),
            half_cosine((q - self.travel_offset) / self.travel_duration),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn degeneracy_is_an_explicit_caller_policy() {
        for geometry in [
            [0., 0., 0., 0., 0.],
            [0., 0., 0., 1., 1.],
            [0., 0., 0., 100., 201.],
            [0., 0., f64::NAN, 100., 90.],
            [f64::MAX, f64::MAX, 0., 100., 90.],
        ] {
            assert!(GeniePhases::new(
                geometry[0],
                geometry[1],
                geometry[2],
                geometry[3],
                geometry[4]
            )
            .is_none());
        }
        assert_eq!(half_cosine(f64::NAN), 0.);
        assert_eq!(half_cosine(f64::NEG_INFINITY), 0.);
        assert_eq!(half_cosine(f64::INFINITY), 1.);
    }

    #[test]
    fn nonzero_gap_changes_the_phase_schedule_without_losing_endpoints() {
        let near = GeniePhases::new(100., -100., 0., 1000., 800.).unwrap();
        let far = GeniePhases::new(100., -100., 1000., 1000., 800.).unwrap();
        assert_ne!(near.sample(0.3), far.sample(0.3));
        for phases in [near, far] {
            assert_eq!(phases.sample(0.), (0., 0.));
            assert_eq!(phases.sample(1.), (1., 1.));
            assert_eq!(phases.sample(f64::NAN), (0., 0.));
        }
    }
}
