//! Finite logical geometry shared by independent layout policies.

/// Maximum logical magnitude accepted from an adapter or configuration.
/// This also prevents overflow in arithmetic involving otherwise finite input.
pub const MAX_LOGICAL: f64 = 1_000_000_000.0;

pub fn finite(value: f64, fallback: f64) -> f64 {
    if value.is_finite() {
        value.clamp(-MAX_LOGICAL, MAX_LOGICAL)
    } else {
        fallback
    }
}

pub fn nonnegative(value: f64) -> f64 {
    finite(value, 0.0).max(0.0)
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

impl Size {
    pub const fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }

    pub fn sanitized(self) -> Self {
        Self::new(nonnegative(self.width), nonnegative(self.height))
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn sanitized(self) -> Self {
        Self::new(
            finite(self.x, 0.0),
            finite(self.y, 0.0),
            nonnegative(self.width),
            nonnegative(self.height),
        )
    }

    pub fn size(self) -> Size {
        Size::new(self.width, self.height)
    }

    pub fn right(self) -> f64 {
        self.x + self.width
    }

    pub fn bottom(self) -> f64 {
        self.y + self.height
    }

    pub fn contains(self, x: f64, y: f64) -> bool {
        x.is_finite()
            && y.is_finite()
            && x >= self.x
            && x < self.right()
            && y >= self.y
            && y < self.bottom()
    }

    pub fn intersection(self, other: Self) -> Option<Self> {
        let a = self.sanitized();
        let b = other.sanitized();
        let x = a.x.max(b.x);
        let y = a.y.max(b.y);
        let width = a.right().min(b.right()) - x;
        let height = a.bottom().min(b.bottom()) - y;
        (width > 0.0 && height > 0.0).then(|| Self::new(x, y, width, height))
    }

    pub fn union(self, other: Self) -> Self {
        let a = self.sanitized();
        let b = other.sanitized();
        let x = a.x.min(b.x);
        let y = a.y.min(b.y);
        Self::new(
            x,
            y,
            a.right().max(b.right()) - x,
            a.bottom().max(b.bottom()) - y,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Horizontal,
    Vertical,
}

/// Clamp location to a working area, preserving the requested size. Oversized
/// windows align to the top/left so their title bar remains reachable.
pub fn clamp_rect(rect: Rect, bounds: Rect) -> Rect {
    let rect = rect.sanitized();
    let bounds = bounds.sanitized();
    Rect::new(
        rect.x.min(bounds.right() - rect.width).max(bounds.x),
        rect.y.min(bounds.bottom() - rect.height).max(bounds.y),
        rect.width,
        rect.height,
    )
}

/// Resolve size constraints. Zero maximum means unconstrained, as in Wayland.
pub fn constrain_size(size: Size, minimum: Size, maximum: Size) -> Size {
    let size = size.sanitized();
    let minimum = minimum.sanitized();
    let maximum = maximum.sanitized();
    fn dimension(value: f64, min: f64, max: f64) -> f64 {
        let upper = if max == 0.0 {
            MAX_LOGICAL
        } else {
            max.max(min)
        };
        value.max(min).min(upper)
    }
    Size::new(
        dimension(size.width, minimum.width, maximum.width),
        dimension(size.height, minimum.height, maximum.height),
    )
}

/// Split rounded logical rectangles using the Realand BSP ratio bounds. The
/// normal-size calculation matches the established 10%..90% split policy.
/// On undersized outputs the gap contracts rather than producing negative size.
pub fn split_rect(bounds: Rect, axis: Axis, ratio: f64, gap: f64) -> (Rect, Rect) {
    let bounds = bounds.sanitized();
    let span = match axis {
        Axis::Horizontal => bounds.width,
        Axis::Vertical => bounds.height,
    };
    let gap = nonnegative(gap).min(span);
    let available = span - gap;
    let ratio = finite(ratio, 0.5).clamp(0.1, 0.9);
    let first = (available * ratio).round().clamp(0.0, available);
    let second = available - first;
    match axis {
        Axis::Horizontal => (
            Rect::new(bounds.x, bounds.y, first, bounds.height),
            Rect::new(bounds.x + first + gap, bounds.y, second, bounds.height),
        ),
        Axis::Vertical => (
            Rect::new(bounds.x, bounds.y, bounds.width, first),
            Rect::new(bounds.x, bounds.y + first + gap, bounds.width, second),
        ),
    }
}

/// Stack windows in a column, distributing its height by positive weights.
/// Empty, non-finite and zero weights receive equal weight. Rounding is applied
/// to cumulative edges, avoiding drift and preserving the final bottom edge.
pub fn column_rects(bounds: Rect, gap: f64, weights: &[f64]) -> Vec<Rect> {
    if weights.is_empty() {
        return Vec::new();
    }
    let bounds = bounds.sanitized();
    let n = weights.len();
    let gap = if n == 1 {
        0.0
    } else {
        nonnegative(gap).min(bounds.height / (n - 1) as f64)
    };
    let available = (bounds.height - gap * (n - 1) as f64).max(0.0);
    // Normalise extreme weights together. Capping individual weights changes
    // their proportions, and a direct sum can overflow even for finite inputs.
    // Ordinary weights retain their existing arithmetic and rounding edges.
    let largest = weights
        .iter()
        .copied()
        .filter(|weight| weight.is_finite() && *weight > 0.0)
        .fold(1.0, f64::max);
    let scale = if largest > MAX_LOGICAL { largest } else { 1.0 };
    let weights: Vec<_> = weights
        .iter()
        .map(|&w| {
            if w.is_finite() && w > 0.0 {
                w / scale
            } else {
                1.0 / scale
            }
        })
        .collect();
    let total: f64 = weights.iter().sum();
    let mut prefix = 0.0;
    let mut edge = 0.0;
    weights
        .iter()
        .enumerate()
        .map(|(index, &weight)| {
            prefix += weight;
            let next = if index + 1 == n {
                available
            } else {
                (available * (prefix / total))
                    .round()
                    .clamp(edge, available)
            };
            let rect = Rect::new(
                bounds.x,
                bounds.y + edge + gap * index as f64,
                bounds.width,
                next - edge,
            );
            edge = next;
            rect
        })
        .collect()
}

/// Retained insertion-order spiral used inside a Dwindle scrolling column.
/// Ordinary splits preserve the established rounded half sizes. Exhausted
/// bounds contract the gap and permit empty cells instead of growing outside
/// the available rectangle. Callers reuse `result` across dirty arrangements.
pub fn spiral_rects_into(bounds: Rect, count: usize, gap: f64, result: &mut Vec<Rect>) {
    result.clear();
    if count == 0 {
        return;
    }
    result.reserve(count);
    result.push(bounds.sanitized());
    for index in 1..count {
        let parent = result[index - 1];
        let axis = if parent.width >= parent.height {
            Axis::Horizontal
        } else {
            Axis::Vertical
        };
        let (first, second) = split_rect(parent, axis, 0.5, gap);
        result[index - 1] = first;
        result.push(second);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spiral_preserves_ordinary_rounded_half_geometry_and_reuses_storage() {
        // The pre-migration spiral is an oracle only; production uses split_rect.
        fn old_spiral(bounds: Rect, count: usize, gap: f64) -> Vec<Rect> {
            let mut out = vec![bounds];
            for index in 1..count {
                let old = out[index - 1];
                let horizontal = old.width >= old.height;
                let total = ((if horizontal { old.width } else { old.height }) - gap).max(1.);
                assert!(
                    total >= 2.,
                    "ordinary fixture must have two positive halves"
                );
                let first = (total / 2.).round();
                let second = (total - first).max(1.);
                if horizontal {
                    out[index - 1] = Rect::new(old.x, old.y, first, old.height);
                    out.push(Rect::new(old.x + first + gap, old.y, second, old.height));
                } else {
                    out[index - 1] = Rect::new(old.x, old.y, old.width, first);
                    out.push(Rect::new(old.x, old.y + first + gap, old.width, second));
                }
            }
            out
        }
        let mut result = Vec::with_capacity(16);
        let storage = result.as_ptr();
        for bounds in [
            Rect::new(13.25, -4.5, 1920., 1080.),
            Rect::new(-37., 22.5, 1080.5, 1920.25),
        ] {
            for gap in [0., 1.25, 16.] {
                for count in 1..=10 {
                    spiral_rects_into(bounds, count, gap, &mut result);
                    assert_eq!(result, old_spiral(bounds, count, gap));
                    assert_eq!(
                        storage,
                        result.as_ptr(),
                        "dirty geometry should reuse retained storage"
                    );
                }
            }
        }
        spiral_rects_into(Rect::default(), 0, 0., &mut result);
        assert!(result.is_empty());
        assert_eq!(storage, result.as_ptr());
    }

    #[test]
    fn spiral_extreme_and_exhausted_bounds_stay_finite_contained_and_nonnegative() {
        let mut result = Vec::new();
        for bounds in [
            Rect::default(),
            Rect::new(-4., 8., 1., 0.25),
            Rect::new(0.5, 0.75, 2., 1.),
            Rect::new(f64::NAN, f64::INFINITY, f64::MAX, f64::NEG_INFINITY),
        ] {
            let bounded = bounds.sanitized();
            for gap in [-1., 0., 16., f64::INFINITY, f64::NAN] {
                spiral_rects_into(bounds, 64, gap, &mut result);
                for cell in &result {
                    assert!([cell.x, cell.y, cell.width, cell.height]
                        .iter()
                        .all(|n| n.is_finite()));
                    assert!(cell.width >= 0. && cell.height >= 0.);
                    assert!(cell.x >= bounded.x && cell.y >= bounded.y);
                    assert!(cell.right() <= bounded.right() + 1e-6);
                    assert!(cell.bottom() <= bounded.bottom() + 1e-6);
                }
            }
        }
    }

    #[test]
    fn clamp_preserves_oversized_reachable_geometry() {
        assert_eq!(
            clamp_rect(
                Rect::new(400.0, -300.0, 1200.0, 300.0),
                Rect::new(20.0, 40.0, 800.0, 600.0)
            ),
            Rect::new(20.0, 40.0, 1200.0, 300.0)
        );
    }

    #[test]
    fn splitting_conserves_edges_even_with_tiny_areas() {
        for span in [0.0, 0.2, 1.0, 10.0, 1001.0] {
            for axis in [Axis::Horizontal, Axis::Vertical] {
                for ratio in [-1.0, 0.1, 0.5, 0.9, f64::NAN] {
                    let bounds = Rect::new(-5.0, 20.0, span, span);
                    let (a, b) = split_rect(bounds, axis, ratio, 8.0);
                    assert!(a.width >= 0.0 && a.height >= 0.0 && b.width >= 0.0 && b.height >= 0.0);
                    match axis {
                        Axis::Horizontal => assert_eq!(b.right(), bounds.right()),
                        Axis::Vertical => assert_eq!(b.bottom(), bounds.bottom()),
                    }
                }
            }
        }
    }

    #[test]
    fn columns_finish_exactly_at_bottom_without_rounding_drift() {
        let bounds = Rect::new(4.5, 7.25, 200.0, 503.5);
        let rects = column_rects(bounds, 3.25, &[1.0, 2.0, f64::INFINITY, 4.0]);
        assert_eq!(rects.last().unwrap().bottom(), bounds.bottom());
        for pair in rects.windows(2) {
            assert_eq!(pair[1].y - pair[0].bottom(), 3.25);
        }
    }

    #[test]
    fn extreme_finite_column_weights_preserve_ratios_and_do_not_overflow() {
        let bounds = Rect::new(0.0, 0.0, 100.0, 600.0);
        let rows = column_rects(bounds, 0.0, &[1e308, 5e307]);
        assert_eq!(rows[0].height, 400.0);
        assert_eq!(rows[1].height, 200.0);
        let rows = column_rects(bounds, 0.0, &[1e308, 1.0, 1.0]);
        assert_eq!(rows.last().unwrap().bottom(), bounds.bottom());
        assert!(rows
            .iter()
            .all(|row| row.height.is_finite() && row.height >= 0.0));
        assert_eq!(rows.iter().map(|row| row.height).sum::<f64>(), 600.0);
    }

    #[test]
    fn geometry_sanitizes_invalid_values_and_conflicting_constraints() {
        let rect = clamp_rect(
            Rect::new(f64::INFINITY, f64::NAN, -4.0, f64::INFINITY),
            Rect::new(0.0, 0.0, 100.0, 100.0),
        );
        assert_eq!(rect, Rect::default());
        assert_eq!(
            constrain_size(
                Size::new(20.0, 1000.0),
                Size::new(100.0, 10.0),
                Size::new(50.0, 0.0)
            ),
            Size::new(100.0, 1000.0)
        );
    }
}
