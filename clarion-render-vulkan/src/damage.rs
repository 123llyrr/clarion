//! Adapted from Nourish's damaged composition pass: preserve untouched pixels.
use ash::vk;
use smithay::utils::{Buffer, Rectangle};

/// Keep native frame coordinates in bounds before emitting Vulkan commands.
pub(super) fn clip_rect(x: i32, y: i32, w: i32, h: i32, width: u32, height: u32) -> vk::Rect2D {
    let width = i64::from(width.min(i32::MAX as u32));
    let height = i64::from(height.min(i32::MAX as u32));
    let x0 = i64::from(x).clamp(0, width);
    let y0 = i64::from(y).clamp(0, height);
    let x1 = (i64::from(x) + i64::from(w.max(0))).clamp(x0, width);
    let y1 = (i64::from(y) + i64::from(h.max(0))).clamp(y0, height);
    vk::Rect2D {
        offset: vk::Offset2D {
            x: x0 as i32,
            y: y0 as i32,
        },
        extent: vk::Extent2D {
            width: (x1 - x0) as u32,
            height: (y1 - y0) as u32,
        },
    }
}

pub(super) fn intersection(a: vk::Rect2D, b: vk::Rect2D) -> vk::Rect2D {
    let x = a.offset.x.max(b.offset.x);
    let y = a.offset.y.max(b.offset.y);
    let right = (i64::from(a.offset.x) + i64::from(a.extent.width))
        .min(i64::from(b.offset.x) + i64::from(b.extent.width));
    let bottom = (i64::from(a.offset.y) + i64::from(a.extent.height))
        .min(i64::from(b.offset.y) + i64::from(b.extent.height));
    vk::Rect2D {
        offset: vk::Offset2D { x, y },
        extent: vk::Extent2D {
            width: (right - i64::from(x)).max(0) as u32,
            height: (bottom - i64::from(y)).max(0) as u32,
        },
    }
}

// One clipped bounding rectangle avoids overlapping alpha draws and bounds
// command count even when a client submits thousands of tiny damage rectangles.
pub(super) fn bounding_rect(
    rects: &[Rectangle<i32, Buffer>],
    width: u32,
    height: u32,
) -> Option<vk::Rect2D> {
    let (mut x0, mut y0, mut x1, mut y1) = (i64::from(width), i64::from(height), 0, 0);
    for rect in rects {
        if rect.size.w <= 0 || rect.size.h <= 0 {
            continue;
        }
        let x = i64::from(rect.loc.x).clamp(0, i64::from(width));
        let y = i64::from(rect.loc.y).clamp(0, i64::from(height));
        let right = (i64::from(rect.loc.x) + i64::from(rect.size.w)).clamp(0, i64::from(width));
        let bottom = (i64::from(rect.loc.y) + i64::from(rect.size.h)).clamp(0, i64::from(height));
        if right <= x || bottom <= y {
            continue;
        }
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(right);
        y1 = y1.max(bottom);
    }
    (x1 > x0 && y1 > y0).then_some(vk::Rect2D {
        offset: vk::Offset2D {
            x: x0 as i32,
            y: y0 as i32,
        },
        extent: vk::Extent2D {
            width: (x1 - x0) as u32,
            height: (y1 - y0) as u32,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_clip_preserves_empty_regions_and_handles_extreme_coordinates() {
        let r = clip_rect(-5, -3, 9, 8, 16, 16);
        assert_eq!(
            r.extent,
            vk::Extent2D {
                width: 4,
                height: 5
            }
        );
        assert_eq!(clip_rect(i32::MAX, 0, i32::MAX, 5, 16, 16).extent.width, 0);
        assert_eq!(clip_rect(5, 5, -1, 4, 16, 16).extent.width, 0);
        let empty = intersection(clip_rect(10, 10, 4, 4, 16, 16), r);
        assert_eq!(
            empty.extent,
            vk::Extent2D {
                width: 0,
                height: 0
            }
        );
    }
    #[test]
    fn clips_and_merges_without_integer_overflow() {
        let rects = [
            Rectangle::new((-5, -5).into(), (10, 10).into()),
            Rectangle::new((4, 4).into(), (20, 20).into()),
            Rectangle::new((i32::MAX, 0).into(), (i32::MAX, 1).into()),
        ];
        let r = bounding_rect(&rects, 16, 16).unwrap();
        assert_eq!(r.offset, vk::Offset2D { x: 0, y: 0 });
        assert_eq!(
            r.extent,
            vk::Extent2D {
                width: 16,
                height: 16
            }
        );
        assert!(bounding_rect(&[], 16, 16).is_none());
        assert!(bounding_rect(&[Rectangle::new((20, 20).into(), (1, 1).into())], 16, 16).is_none());
    }
}
