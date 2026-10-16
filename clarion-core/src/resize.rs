//! Numeric interactive resize state shared by floating and tiled policies.

use crate::geometry::Size;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Resize {
    pub id: u64,
    pub original_size: Size,
    /// TOP=1, BOTTOM=2, LEFT=4, RIGHT=8. Device bitflag types remain in adapters.
    pub edges: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResizeIntent {
    pub width: Option<i32>,
    pub height: Option<i32>,
}

impl Resize {
    pub fn new(id: u64, original_size: Size, edges: u32) -> Self {
        Self {
            id,
            original_size: original_size.sanitized(),
            edges: edges & 15,
        }
    }

    pub fn width_delta(&self, delta: f64, centered: bool) -> Option<f64> {
        if self.edges & 12 == 0 || !delta.is_finite() {
            return None;
        }
        let delta = if self.edges & 4 != 0 { -delta } else { delta };
        Some(delta * if centered { 2.0 } else { 1.0 })
    }

    pub fn height_delta(&self, delta: f64) -> Option<f64> {
        if self.edges & 3 == 0 || !delta.is_finite() {
            return None;
        }
        Some(if self.edges & 1 != 0 { -delta } else { delta })
    }

    pub fn intent(&self, delta: [f64; 2]) -> Option<ResizeIntent> {
        if !delta.iter().all(|delta| delta.is_finite()) {
            return None;
        }
        Some(ResizeIntent {
            width: self
                .width_delta(delta[0], false)
                .map(|delta| (self.original_size.width + delta).round() as i32),
            height: self
                .height_delta(delta[1])
                .map(|delta| (self.original_size.height + delta).round() as i32),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn centered_resize_and_edge_signs_preserve_original_anchor() {
        let resize = Resize::new(17, Size::new(400.0, 300.0), 5);
        assert_eq!(resize.width_delta(30.0, false), Some(-30.0));
        assert_eq!(resize.width_delta(30.0, true), Some(-60.0));
        assert_eq!(resize.height_delta(-20.0), Some(20.0));
        assert_eq!(
            resize.intent([30.0, -20.0]),
            Some(ResizeIntent {
                width: Some(370),
                height: Some(320)
            })
        );
    }
    #[test]
    fn absent_edges_and_invalid_pointer_samples_do_not_emit_size_requests() {
        let resize = Resize::new(17, Size::new(400.0, 300.0), 8);
        assert_eq!(resize.height_delta(20.0), None);
        assert_eq!(resize.intent([f64::NAN, 10.0]), None);
        assert_eq!(
            resize.intent([20.0, 10.0]),
            Some(ResizeIntent {
                width: Some(420),
                height: None
            })
        );
    }
}
