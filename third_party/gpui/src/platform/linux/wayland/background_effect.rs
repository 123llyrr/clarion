//! Client bindings for the standard compositor background-effect protocol.
//! Keep these bindings on GPUI's existing wayland-client version; upgrading the
//! unrelated protocol collection would produce incompatible generated traits.

#[allow(dead_code, non_camel_case_types, unused_unsafe, unused_variables)]
#[allow(non_upper_case_globals, non_snake_case, unused_imports)]
#[allow(missing_docs, clippy::all)]
pub(crate) mod client {
    use wayland_client;
    use wayland_client::protocol::*;

    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!(
            "src/platform/linux/wayland/ext-background-effect-v1.xml"
        );
    }
    use self::__interfaces::*;
    wayland_scanner::generate_client_code!(
        "src/platform/linux/wayland/ext-background-effect-v1.xml"
    );
}

/// Both input and effect regions use the same bounded, outward-rounded logical
/// rectangles. Invalid geometry must not become huge protocol coordinates.
///
/// The cap is generous because scanline masks (a tilted Dock label follows its
/// own outline one logical row at a time) legitimately reach a few hundred
/// rectangles while still staying far below anything abusive.
pub(crate) fn rectangles(bounds: &[crate::Bounds<crate::Pixels>]) -> Vec<[i32; 4]> {
    bounds
        .iter()
        .take(4096)
        .filter_map(|bounds| {
            let (x, y, width, height) = (
                bounds.origin.x.0,
                bounds.origin.y.0,
                bounds.size.width.0,
                bounds.size.height.0,
            );
            if ![x, y, width, height]
                .iter()
                .all(|value| value.is_finite() && value.abs() < 1_000_000.)
                || width <= 0.
                || height <= 0.
            {
                return None;
            }
            let left = x.floor() as i32;
            let top = y.floor() as i32;
            Some([
                left,
                top,
                (x + width).ceil() as i32 - left,
                (y + height).ceil() as i32 - top,
            ])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::rectangles;
    use crate::{Bounds, point, px, size};

    #[::core::prelude::v1::test]
    fn region_preserves_fractional_coverage_and_rejects_nonfinite_geometry() {
        let bounds = [
            Bounds::new(point(px(2.4), px(3.8)), size(px(12.2), px(8.5))),
            Bounds::new(point(px(f32::NAN), px(0.)), size(px(4.), px(4.))),
            Bounds::new(point(px(0.), px(0.)), size(px(0.), px(4.))),
        ];
        assert_eq!(rectangles(&bounds), vec![[2, 3, 13, 10]]);
    }

    #[::core::prelude::v1::test]
    fn effect_and_input_regions_keep_holes_and_bound_protocol_work() {
        let bounds = [
            Bounds::new(point(px(8.), px(0.)), size(px(84.), px(2.))),
            Bounds::new(point(px(0.), px(2.)), size(px(100.), px(36.))),
            Bounds::new(point(px(8.), px(38.)), size(px(84.), px(2.))),
        ];
        let region = rectangles(&bounds);
        let contains = |x, y| {
            region
                .iter()
                .any(|[left, top, w, h]| x >= *left && x < left + w && y >= *top && y < top + h)
        };
        assert!(!contains(0, 0));
        assert!(contains(50, 0));
        assert!(contains(0, 20));
        assert!(!contains(99, 39));
        assert_eq!(rectangles(&vec![bounds[1]; 5000]).len(), 4096);
        assert!(rectangles(&[]).is_empty());
    }
}
