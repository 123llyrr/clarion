//! Device-independent exact scene regions. Surface adapters supply only
//! integer rectangles; no surface, renderer or output handles are stored here.

use smallvec::SmallVec;
use std::collections::BTreeSet;

pub trait PixelRectangle {
    type Rect: Copy + PartialEq;
    fn xywh(rect: Self::Rect) -> [i32; 4];
    fn from_xywh(coordinates: [i32; 4]) -> Self::Rect;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelRect(pub [i32; 4]);
impl PixelRectangle for PixelRect {
    type Rect = Self;
    fn xywh(rect: Self) -> [i32; 4] {
        rect.0
    }
    fn from_xywh(value: [i32; 4]) -> Self {
        Self(value)
    }
}

pub type DamageRegions<R> = SmallVec<[R; 8]>;

fn intersection<R: PixelRectangle>(a: R::Rect, b: R::Rect) -> Option<R::Rect> {
    let [ax, ay, aw, ah] = R::xywh(a);
    let [bx, by, bw, bh] = R::xywh(b);
    let x = ax.max(bx);
    let y = ay.max(by);
    let right = (ax + aw).min(bx + bw);
    let bottom = (ay + ah).min(by + bh);
    (x < right && y < bottom).then(|| R::from_xywh([x, y, right - x, bottom - y]))
}

fn union<R: PixelRectangle>(a: R::Rect, b: R::Rect) -> R::Rect {
    let [ax, ay, aw, ah] = R::xywh(a);
    let [bx, by, bw, bh] = R::xywh(b);
    let x = ax.min(bx);
    let y = ay.min(by);
    R::from_xywh([x, y, (ax + aw).max(bx + bw) - x, (ay + ah).max(by + bh) - y])
}

pub fn clipped<R: PixelRectangle>(rect: R::Rect, size: [i32; 2]) -> Option<R::Rect> {
    if R::xywh(rect)[2] <= 0 || R::xywh(rect)[3] <= 0 || size[0] <= 0 || size[1] <= 0 {
        return None;
    }
    // Client coordinates may overflow i32 before clipping. Once clipped, every
    // edge is in 0..=size and the subtraction below can use i32 safely.
    let left = i64::from(R::xywh(rect)[0]).clamp(0, i64::from(size[0]));
    let top = i64::from(R::xywh(rect)[1]).clamp(0, i64::from(size[1]));
    let right =
        (i64::from(R::xywh(rect)[0]) + i64::from(R::xywh(rect)[2])).clamp(0, i64::from(size[0]));
    let bottom =
        (i64::from(R::xywh(rect)[1]) + i64::from(R::xywh(rect)[3])).clamp(0, i64::from(size[1]));
    (left < right && top < bottom).then(|| {
        R::from_xywh([
            left as i32,
            top as i32,
            (right - left) as i32,
            (bottom - top) as i32,
        ])
    })
}

fn subtract<R: PixelRectangle>(
    rect: R::Rect,
    overlap: R::Rect,
    output: &mut DamageRegions<R::Rect>,
) {
    let mut push = |x, y, w, h| {
        if w > 0 && h > 0 {
            output.push(R::from_xywh([x, y, w, h]));
        }
    };
    push(
        R::xywh(rect)[0],
        R::xywh(rect)[1],
        R::xywh(rect)[2],
        R::xywh(overlap)[1] - R::xywh(rect)[1],
    );
    push(
        R::xywh(rect)[0],
        R::xywh(overlap)[1],
        R::xywh(overlap)[0] - R::xywh(rect)[0],
        R::xywh(overlap)[3],
    );
    let right = R::xywh(overlap)[0] + R::xywh(overlap)[2];
    push(
        right,
        R::xywh(overlap)[1],
        R::xywh(rect)[0] + R::xywh(rect)[2] - right,
        R::xywh(overlap)[3],
    );
    let bottom = R::xywh(overlap)[1] + R::xywh(overlap)[3];
    push(
        R::xywh(rect)[0],
        bottom,
        R::xywh(rect)[2],
        R::xywh(rect)[1] + R::xywh(rect)[3] - bottom,
    );
}

fn insert_coalesced<R: PixelRectangle>(regions: &mut DamageRegions<R::Rect>, mut rect: R::Rect) {
    // Coalescing is an extra search, useful for a handful of draw clips but not
    // worth another quadratic pass over heavily fragmented client damage.
    if regions.len() > 8 {
        regions.push(rect);
        return;
    }
    let mut index = 0;
    while index < regions.len() {
        let previous = regions[index];
        let vertical = R::xywh(rect)[0] == R::xywh(previous)[0]
            && R::xywh(rect)[2] == R::xywh(previous)[2]
            && (R::xywh(rect)[1] + R::xywh(rect)[3] == R::xywh(previous)[1]
                || R::xywh(previous)[1] + R::xywh(previous)[3] == R::xywh(rect)[1]);
        let horizontal = R::xywh(rect)[1] == R::xywh(previous)[1]
            && R::xywh(rect)[3] == R::xywh(previous)[3]
            && (R::xywh(rect)[0] + R::xywh(rect)[2] == R::xywh(previous)[0]
                || R::xywh(previous)[0] + R::xywh(previous)[2] == R::xywh(rect)[0]);
        if vertical || horizontal {
            rect = union::<R>(rect, regions.swap_remove(index));
            // The merged edge can now match a region visited earlier.
            index = 0;
        } else {
            index += 1;
        }
    }
    regions.push(rect);
}

/// Clip to destination-local bounds and cover each damaged pixel exactly once.
/// Small region sets coalesce full shared edges without adding undamaged pixels.
pub fn disjoint_damage<R: PixelRectangle>(
    damage: &[R::Rect],
    size: [i32; 2],
) -> DamageRegions<R::Rect> {
    let mut regions = DamageRegions::new();
    let bounds = R::from_xywh([0, 0, size[0], size[1]]);
    for &rect in damage {
        let Some(rect) = clipped::<R>(rect, size) else {
            continue;
        };
        if rect == bounds {
            regions.clear();
            regions.push(rect);
            return regions;
        }
        let Some(first_overlap) = regions
            .iter()
            .position(|previous| intersection::<R>(*previous, rect).is_some())
        else {
            insert_coalesced::<R>(&mut regions, rect);
            continue;
        };
        let mut fragments = DamageRegions::new();
        fragments.push(rect);
        // Earlier regions did not overlap `rect`, so they cannot overlap any
        // smaller fragments produced by subtracting the first overlap.
        for &previous in &regions[first_overlap..] {
            // Visit the original slots backwards: swap_remove can move a new
            // split piece into the removed slot, but cannot hide any original
            // fragment that still needs to be checked at a lower index.
            let count = fragments.len();
            for index in (0..count).rev() {
                if let Some(overlap) = intersection::<R>(fragments[index], previous) {
                    let fragment = fragments.swap_remove(index);
                    subtract::<R>(fragment, overlap, &mut fragments);
                }
            }
            if fragments.is_empty() {
                break;
            }
        }
        for fragment in fragments {
            insert_coalesced::<R>(&mut regions, fragment);
        }
    }
    regions
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionOp {
    Add,
    Subtract,
}

fn representable<R: PixelRectangle>(rect: R::Rect) -> Option<[i64; 4]> {
    let [x, y, w, h] = R::xywh(rect);
    if w <= 0 || h <= 0 {
        return None;
    }
    let right = (i64::from(x) + i64::from(w)).min(i64::from(i32::MAX));
    let bottom = (i64::from(y) + i64::from(h)).min(i64::from(i32::MAX));
    let x = i64::from(x);
    let y = i64::from(y);
    (x < right && y < bottom).then_some([x, y, right, bottom])
}

/// Resolve ordered add/subtract operations into disjoint bands. Edges use i64
/// before clipping to the representable coordinate plane; oversized merged
/// bands are split into valid i32 sizes instead of overflowing. Surface-local
/// coordinates outside that plane cannot belong to an i32-sized client buffer.
pub fn canonical_region<R, I>(operations: I, output: &mut Vec<R::Rect>)
where
    R: PixelRectangle,
    I: Iterator<Item = (RegionOp, R::Rect)> + Clone,
{
    output.clear();
    let ys = BTreeSet::from_iter(
        operations
            .clone()
            .filter_map(|(_, rect)| representable::<R>(rect))
            .flat_map(|r| [r[1], r[3]]),
    );
    let mut ys = ys.into_iter();
    let Some(mut lo) = ys.next() else {
        return;
    };
    let mut spans = Vec::<(i64, i64)>::new();
    for hi in ys {
        spans.clear();
        'region: for (kind, rect) in operations.clone() {
            let Some([mut x1, y1, mut x2, y2]) = representable::<R>(rect) else {
                continue;
            };
            if hi <= y1 || y2 <= lo {
                continue;
            }
            match kind {
                RegionOp::Add => {
                    for i in (0..spans.len()).rev() {
                        let (start, end) = spans[i];
                        if end < x1 {
                            spans.insert(i + 1, (x1, x2));
                            continue 'region;
                        }
                        if x2 < start {
                            continue;
                        }
                        spans.remove(i);
                        x1 = x1.min(start);
                        x2 = x2.max(end);
                    }
                    spans.insert(0, (x1, x2));
                }
                RegionOp::Subtract => {
                    for i in (0..spans.len()).rev() {
                        let (start, end) = spans[i];
                        if end <= x1 {
                            continue 'region;
                        }
                        if x2 <= start {
                            continue;
                        }
                        spans.remove(i);
                        if x2 < end {
                            spans.insert(i, (x2, end));
                        }
                        if start < x1 {
                            spans.insert(i, (start, x1));
                        }
                    }
                }
            }
        }
        for (left, right) in spans.drain(..) {
            let mut top = lo;
            while top < hi {
                let height = (hi - top).min(i64::from(i32::MAX));
                let mut x = left;
                while x < right {
                    let width = (right - x).min(i64::from(i32::MAX));
                    output.push(R::from_xywh([
                        x as i32,
                        top as i32,
                        width as i32,
                        height as i32,
                    ]));
                    x += width;
                }
                top += height;
            }
        }
        lo = hi;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn contains(rect: PixelRect, x: i32, y: i32) -> bool {
        let [left, top, w, h] = rect.0;
        w > 0
            && h > 0
            && x >= left
            && y >= top
            && i64::from(x) < i64::from(left) + i64::from(w)
            && i64::from(y) < i64::from(top) + i64::from(h)
    }

    #[test]
    fn extreme_client_regions_are_safe_and_preserve_add_subtract_coverage() {
        let ops = [
            (RegionOp::Add, PixelRect([i32::MAX - 2, 0, i32::MAX, 20])),
            (RegionOp::Add, PixelRect([-20, -20, 40, 40])),
            (RegionOp::Subtract, PixelRect([-5, -5, 10, 10])),
            (RegionOp::Add, PixelRect([i32::MIN, 0, i32::MAX, 20])),
            (RegionOp::Add, PixelRect([-1, 0, i32::MAX, 20])),
        ];
        let mut output = Vec::new();
        canonical_region::<PixelRect, _>(ops.into_iter(), &mut output);
        for x in [
            i32::MIN,
            i32::MIN + 1,
            -20,
            -6,
            -5,
            -1,
            0,
            4,
            5,
            19,
            20,
            i32::MAX - 2,
            i32::MAX - 1,
        ] {
            for y in [-20, -5, 0, 4, 5, 19, 20] {
                let mut expected = false;
                for (op, rect) in ops {
                    if contains(rect, x, y) {
                        expected = op == RegionOp::Add;
                    }
                }
                assert_eq!(
                    output.iter().filter(|r| contains(**r, x, y)).count(),
                    usize::from(expected),
                    "point=({x},{y})"
                );
            }
        }
        for rect in output {
            let [x, y, w, h] = rect.0;
            assert!(w > 0 && h > 0);
            assert!(x.checked_add(w).is_some() && y.checked_add(h).is_some());
        }
    }

    #[test]
    fn clipped_damage_has_exact_single_alpha_coverage() {
        let damage = [
            PixelRect([-5, -3, 10, 8]),
            PixelRect([2, 2, 10, 10]),
            PixelRect([2, 2, 10, 10]),
            PixelRect([i32::MAX, 0, i32::MAX, 1]),
        ];
        let result = disjoint_damage::<PixelRect>(&damage, [16, 16]);
        for y in 0..16 {
            for x in 0..16 {
                let expected = damage.iter().any(|r| contains(*r, x, y));
                assert_eq!(
                    result.iter().filter(|r| contains(**r, x, y)).count(),
                    usize::from(expected)
                );
            }
        }
        assert!(!result.spilled());
    }
}
