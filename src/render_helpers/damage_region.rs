//! Platform rectangle conversion for Clarion's shared scene region policy.

use clarion_core::region::PixelRectangle;
use smithay::utils::{Rectangle, Size};

struct Coordinates<K>(std::marker::PhantomData<K>);
impl<K> PixelRectangle for Coordinates<K> {
    type Rect = Rectangle<i32, K>;
    fn xywh(rect: Self::Rect) -> [i32; 4] {
        [rect.loc.x, rect.loc.y, rect.size.w, rect.size.h]
    }
    fn from_xywh([x, y, w, h]: [i32; 4]) -> Self::Rect {
        Rectangle::new((x, y).into(), (w, h).into())
    }
}

pub(super) type DamageRegions<K> = clarion_core::region::DamageRegions<Rectangle<i32, K>>;

#[cfg(test)]
fn clipped<K>(rect: Rectangle<i32, K>, size: Size<i32, K>) -> Option<Rectangle<i32, K>> {
    clarion_core::region::clipped::<Coordinates<K>>(rect, [size.w, size.h])
}

pub(super) fn disjoint_damage<K>(
    damage: &[Rectangle<i32, K>],
    size: Size<i32, K>,
) -> DamageRegions<K> {
    clarion_core::region::disjoint_damage::<Coordinates<K>>(damage, [size.w, size.h])
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::utils::{Physical, Point};

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Physical> {
        let mut rectangle = Rectangle {
            loc: (x, y).into(),
            ..Default::default()
        };
        // Size::new rejects negatives; these tests deliberately exercise them.
        rectangle.size.w = w;
        rectangle.size.h = h;
        rectangle
    }

    fn verify(damage: &[Rectangle<i32, Physical>], size: Size<i32, Physical>) {
        let regions = disjoint_damage(damage, size);
        for (index, region) in regions.iter().enumerate() {
            assert!(Rectangle::from_size(size).contains_rect(*region));
            assert!(regions[index + 1..]
                .iter()
                .all(|other| !region.overlaps(*other)));
        }
        for y in 0..size.h {
            for x in 0..size.w {
                let point: Point<i32, Physical> = (x, y).into();
                let expected = damage
                    .iter()
                    .any(|&r| clipped(r, size).is_some_and(|r| r.contains(point)));
                assert_eq!(
                    regions.iter().filter(|r| r.contains(point)).count(),
                    usize::from(expected)
                );
            }
        }
    }

    #[test]
    fn clipped_union_preserves_alpha_coverage_and_extreme_coordinates() {
        let damage = [
            rect(-5, -3, 10, 8),
            rect(2, 2, 10, 10),
            rect(2, 2, 10, 10),
            rect(i32::MAX, 0, i32::MAX, 1),
            rect(i32::MIN, 0, i32::MAX, 5),
            rect(0, 0, -1, 5),
            rect(0, 0, 5, 0),
        ];
        verify(&damage, (16, 16).into());
        assert!(disjoint_damage::<Physical>(&[], (16, 16).into()).is_empty());
        assert!(disjoint_damage(&damage, (0, 16).into()).is_empty());
        assert_eq!(
            disjoint_damage(&[rect(1, 1, i32::MAX, i32::MAX)], (16, 16).into()).as_slice(),
            &[rect(1, 1, 15, 15)]
        );
    }

    #[test]
    fn adjacent_regions_coalesce_but_gaps_stay_untouched() {
        let damage = [
            rect(0, 0, 4, 4),
            rect(8, 0, 4, 4),
            rect(4, 0, 4, 4),
            rect(0, 4, 12, 4),
        ];
        assert_eq!(
            disjoint_damage(&damage, (16, 16).into()).as_slice(),
            &[rect(0, 0, 12, 8)]
        );
        let gap = [rect(0, 0, 4, 4), rect(8, 0, 4, 4)];
        let regions = disjoint_damage(&gap, (16, 16).into());
        assert_eq!(regions.len(), 2);
        assert!(!regions.spilled());
        verify(&gap, (16, 16).into());
    }

    #[test]
    fn overlapping_fragments_do_not_skip_a_region_after_splitting() {
        // The former subtract_rects loop visited slots forwards while appending
        // split pieces. swap_remove could replace the next original fragment
        // with an already-checked piece, leaving x=6..10, y=10..13 covered twice.
        let damage = [
            rect(15, 13, 4, 8),
            rect(14, 10, 7, 5),
            rect(1, 9, 9, 8),
            rect(6, 6, 9, 9),
        ];
        verify(&damage, (16, 16).into());
    }

    #[test]
    fn smithay_region_subtraction_removes_an_overlapping_region_from_itself() {
        // This goes through the dependency used by OutputDamageTracker, not our
        // effect-specific union helper. The former forward loop left phantom
        // damage at x=46..115, y=132..148 in an unchanged 24-window scene.
        let rectangles: Vec<_> = (0..24)
            .map(|i| rect((i % 6) * 23, (i / 6) * 16, 140, 100))
            .collect();
        assert!(Rectangle::subtract_rects_many(rectangles.clone(), rectangles).is_empty());
    }

    #[test]
    fn smithay_region_subtraction_matches_pixel_coverage() {
        let mut state = 0x9876abcdu32;
        for _ in 0..256 {
            let mut rectangles = || -> Vec<_> {
                (0..12)
                    .map(|_| {
                        let mut next = || {
                            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                            (state >> 16) as i32
                        };
                        rect(
                            next() % 24 - 4,
                            next() % 24 - 4,
                            next() % 12 + 1,
                            next() % 12 + 1,
                        )
                    })
                    .collect()
            };
            let source = rectangles();
            let removed = rectangles();
            let actual = Rectangle::subtract_rects_many(source.clone(), removed.clone());
            for y in -4..32 {
                for x in -4..32 {
                    let p: Point<i32, Physical> = (x, y).into();
                    let expected = source.iter().any(|r| r.contains(p))
                        && !removed.iter().any(|r| r.contains(p));
                    assert_eq!(
                        actual.iter().any(|r| r.contains(p)),
                        expected,
                        "source={source:?}, removed={removed:?}, point={p:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn seeded_fragmented_unions_cover_every_pixel_once() {
        let mut state = 0x1234_5678u32;
        for _ in 0..256 {
            let mut random = || {
                state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                (state >> 16) as i32
            };
            let damage: Vec<_> = (0..32)
                .map(|_| {
                    rect(
                        random() % 30 - 7,
                        random() % 30 - 7,
                        random() % 12 - 2,
                        random() % 12 - 2,
                    )
                })
                .collect();
            verify(&damage, (16, 16).into());
        }
    }

    #[test]
    fn full_coverage_shortcuts_and_large_region_sets_remain_exact() {
        let mut damage: Vec<_> = (0..100).map(|x| rect(x * 2, 0, 1, 4)).collect();
        verify(&damage, (200, 4).into());
        assert!(disjoint_damage(&damage, (200, 4).into()).spilled());
        damage.push(rect(-1, -1, 202, 6));
        let regions = disjoint_damage(&damage, (200, 4).into());
        assert_eq!(regions.as_slice(), &[rect(0, 0, 200, 4)]);
    }
}
