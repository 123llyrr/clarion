//! Default monitor scale calculation.
//!
//! This module follows logic and tests from Mutter:
//! <https://gitlab.gnome.org/GNOME/mutter/-/blob/gnome-46/src/backends/meta-monitor.c>
//!
//! The compositor and the settings app must agree on the scale implied by an
//! automatic (`scale` unset) output, otherwise the settings app would place
//! outputs using a different logical size than the one actually mapped.

const MIN_SCALE: i32 = 1;
const MAX_SCALE: i32 = 4;
const STEPS: i32 = 4;
const MIN_LOGICAL_AREA: i64 = 800 * 480;

const MOBILE_TARGET_DPI: f64 = 135.;
const LARGE_TARGET_DPI: f64 = 110.;
const LARGE_MIN_SIZE_INCHES: f64 = 20.;

/// Calculates the ideal scale for a monitor.
///
/// `size_mm` is the physical panel size in millimetres and `resolution` the
/// mode in physical pixels.
pub fn guess_monitor_scale(size_mm: (i32, i32), resolution: (i32, i32)) -> f64 {
    if size_mm.0 <= 0 || size_mm.1 <= 0 || resolution.0 <= 0 || resolution.1 <= 0 {
        return 1.;
    }

    let diag_inches = f64::from(size_mm.0).hypot(f64::from(size_mm.1)) / 25.4;

    let target_dpi = if diag_inches < LARGE_MIN_SIZE_INCHES {
        MOBILE_TARGET_DPI
    } else {
        LARGE_TARGET_DPI
    };

    let physical_dpi = f64::from(resolution.0).hypot(f64::from(resolution.1)) / diag_inches;
    let perfect_scale = physical_dpi / target_dpi;

    supported_scales(resolution)
        .map(|scale| (scale, (scale - perfect_scale).abs()))
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
        .map_or(1., |(scale, _)| scale)
}

/// Scales that keep at least [`MIN_LOGICAL_AREA`] logical pixels at `resolution`.
pub fn supported_scales(resolution: (i32, i32)) -> impl Iterator<Item = f64> {
    (MIN_SCALE * STEPS..=MAX_SCALE * STEPS)
        .map(|x| f64::from(x) / f64::from(STEPS))
        .filter(move |scale| is_valid_for_resolution(resolution, *scale))
}

fn is_valid_for_resolution(resolution: (i32, i32), scale: f64) -> bool {
    if resolution.0 <= 0 || resolution.1 <= 0 {
        return false;
    }
    let logical = (
        (f64::from(resolution.0) / scale).round() as i32,
        (f64::from(resolution.1) / scale).round() as i32,
    );
    i64::from(logical.0) * i64::from(logical.1) >= MIN_LOGICAL_AREA
}

/// Adjusts the scale to the closest exactly-representable value.
pub fn closest_representable_scale(scale: f64) -> f64 {
    // Current fractional-scale Wayland protocol can only represent N / 120 scales.
    const FRACTIONAL_SCALE_DENOM: f64 = 120.;

    (scale * FRACTIONAL_SCALE_DENOM).round() / FRACTIONAL_SCALE_DENOM
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_physical_size_falls_back_to_one() {
        assert_eq!(guess_monitor_scale((0, 0), (1920, 1080)), 1.);
        assert_eq!(guess_monitor_scale((300, 200), (0, 0)), 1.);
    }

    #[test]
    fn fractional_scales_are_representable_twelfths() {
        assert_eq!(closest_representable_scale(1.5), 1.5);
        assert_eq!(closest_representable_scale(1.333), 80. / 60.);
        assert_eq!(closest_representable_scale(2.004), 2.);
    }

    #[test]
    fn supported_scales_keep_a_useable_logical_area() {
        let scales: Vec<_> = supported_scales((1920, 1080)).collect();
        assert_eq!(scales.first().copied(), Some(1.0));
        assert!(scales.iter().all(|scale| {
            let logical = (
                (1920. / scale).round() as i64,
                (1080. / scale).round() as i64,
            );
            logical.0 * logical.1 >= MIN_LOGICAL_AREA
        }));
    }
}
