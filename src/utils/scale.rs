//! Smithay-typed wrappers around the shared monitor scale calculation.

use clarion_config::scale;
use smithay::utils::{Physical, Raw, Size};

/// Calculates the ideal scale for a monitor.
pub fn guess_monitor_scale(size_mm: Size<i32, Raw>, resolution: Size<i32, Physical>) -> f64 {
    scale::guess_monitor_scale((size_mm.w, size_mm.h), (resolution.w, resolution.h))
}

/// Adjusts the scale to the closest exactly-representable value.
pub fn closest_representable_scale(scale: f64) -> f64 {
    scale::closest_representable_scale(scale)
}

/// Scales that keep a useable logical area at `resolution`.
pub fn supported_scales(resolution: Size<i32, Physical>) -> impl Iterator<Item = f64> {
    scale::supported_scales((resolution.w, resolution.h))
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;

    use super::*;

    fn check(size_mm: (i32, i32), resolution: (i32, i32)) -> f64 {
        guess_monitor_scale(Size::from(size_mm), Size::from(resolution))
    }

    #[test]
    fn test_guess_monitor_scale() {
        // Librem 5; not enough logical area when scaled
        assert_snapshot!(check((65, 129), (720, 1440)), @"1.5");
        // OnePlus 6
        assert_snapshot!(check((68, 144), (1080, 2280)), @"2.5");
        // Google Pixel 6a
        assert_snapshot!(check((64, 142), (1080, 2400)), @"2.5");
        // 13" MacBook Retina
        assert_snapshot!(check((286, 179), (2560, 1600)), @"1.75");
        // Surface Laptop Studio
        assert_snapshot!(check((303, 202), (2400, 1600)), @"1.5");
        // Dell XPS 9320
        assert_snapshot!(check((290, 180), (3840, 2400)), @"2.5");
        // Lenovo ThinkPad X1 Yoga Gen 6
        assert_snapshot!(check((300, 190), (3840, 2400)), @"2.5");
        // Generic 23" 1080p
        assert_snapshot!(check((509, 286), (1920, 1080)), @"1");
        // Generic 23" 4K
        assert_snapshot!(check((509, 286), (3840, 2160)), @"1.75");
        // Generic 27" 4K
        assert_snapshot!(check((598, 336), (3840, 2160)), @"1.5");
        // Generic 32" 4K
        assert_snapshot!(check((708, 398), (3840, 2160)), @"1.25");
        // Generic 25" 4K; ideal scale is 1.60, should round to 1.5 and 1.0
        assert_snapshot!(check((554, 312), (3840, 2160)), @"1.5");
        // Generic 23.5" 4K; ideal scale is 1.70, should round to 1.75 and 2.0
        assert_snapshot!(check((522, 294), (3840, 2160)), @"1.75");
        // Lenovo Legion 7 Gen 7 AMD 16"
        assert_snapshot!(check((340, 210), (2560, 1600)), @"1.5");
        // Acer Nitro XV320QU LV 31.5"
        assert_snapshot!(check((700, 390), (2560, 1440)), @"1");
        // Surface Pro 6
        assert_snapshot!(check((260, 170), (2736, 1824)), @"2");
    }

    #[test]
    fn guess_monitor_scale_unknown_size() {
        assert_eq!(check((0, 0), (1920, 1080)), 1.);
        assert_eq!(check((500, 300), (0, 1080)), 1.);
    }

    #[test]
    fn large_dimensions_do_not_overflow() {
        assert_eq!(check((50_000, 50_000), (60_000, 60_000)), 1.);
        assert_eq!(check((500, 300), (60_000, 60_000)), 4.);
    }

    #[test]
    fn test_round_scale() {
        assert_snapshot!(closest_representable_scale(1.3), @"1.3");
        assert_snapshot!(closest_representable_scale(1.31), @"1.3083333333333333");
        assert_snapshot!(closest_representable_scale(1.32), @"1.3166666666666667");
        assert_snapshot!(closest_representable_scale(1.33), @"1.3333333333333333");
        assert_snapshot!(closest_representable_scale(1.34), @"1.3416666666666666");
        assert_snapshot!(closest_representable_scale(1.35), @"1.35");
    }
}
