//! Bound each disconnected protocol glass surface independently. This keeps a
//! menu plus popup from capturing/shading the whole expanded Shell surface.
use std::sync::Arc;

use smithay::utils::{Logical, Rectangle};

use crate::utils::region::TransformedRegion;

type GlassRegion = (Rectangle<f64, Logical>, f32, TransformedRegion);

pub(super) fn split_regions(region: &TransformedRegion) -> Option<Vec<GlassRegion>> {
    // Protocol input is untrusted. Unusually fragmented masks retain ordinary
    // blur rather than triggering unbounded component analysis or GPU draws.
    if region.rects.len() > 256 {
        return None;
    }
    let mut groups: Vec<(Rectangle<f64, Logical>, Vec<usize>)> = Vec::new();
    for (index, rect) in region.rects.iter().enumerate() {
        if rect.is_empty() {
            continue;
        }
        let mut bounds = rect.to_f64();
        let mut members = vec![index];
        let mut cursor = 0;
        while cursor < groups.len() {
            let other = groups[cursor].0;
            let touching = bounds.loc.x <= other.loc.x + other.size.w
                && other.loc.x <= bounds.loc.x + bounds.size.w
                && bounds.loc.y <= other.loc.y + other.size.h
                && other.loc.y <= bounds.loc.y + bounds.size.h;
            if touching {
                let (other, mut indices) = groups.remove(cursor);
                bounds = bounds.merge(other);
                members.append(&mut indices);
                cursor = 0;
            } else {
                cursor += 1;
            }
        }
        groups.push((bounds, members));
    }
    if groups.len() > 16 {
        return None;
    }
    groups.sort_by(|a, b| {
        a.0.loc
            .y
            .total_cmp(&b.0.loc.y)
            .then(a.0.loc.x.total_cmp(&b.0.loc.x))
    });
    Some(
        groups
            .into_iter()
            .map(|(bounds, members)| {
                // Shell rounded masks have a full-width middle band. Its distance from
                // the top estimates the corner radius; the original protocol mask is
                // retained, so holes/gaps never become newly shaded pixels.
                let radius = members
                    .iter()
                    .filter_map(|&i| {
                        let r = region.rects[i].to_f64();
                        ((r.loc.x - bounds.loc.x).abs() < 0.01
                            && (r.size.w - bounds.size.w).abs() < 0.01)
                            .then_some(r.loc.y - bounds.loc.y)
                    })
                    .reduce(f64::min)
                    .unwrap_or(0.)
                    .max(0.);
                let mut geometry = bounds.upscale(region.scale);
                geometry.loc += region.offset;
                let radius = (radius * region.scale.x.min(region.scale.y))
                    .min(geometry.size.w.min(geometry.size.h) * 0.5)
                    as f32;
                let mask = TransformedRegion {
                    rects: Arc::new(members.into_iter().map(|i| region.rects[i]).collect()),
                    scale: region.scale,
                    offset: region.offset,
                };
                (geometry, radius, mask)
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use smithay::utils::Point;

    use super::*;
    #[test]
    fn menu_and_rounded_panel_have_separate_bounded_captures() {
        let region = TransformedRegion {
            rects: Arc::new(vec![
                Rectangle::new((0, 0).into(), (1000, 24).into()),
                Rectangle::new((208, 40).into(), (184, 8).into()),
                Rectangle::new((200, 48).into(), (200, 184).into()),
                Rectangle::new((208, 232).into(), (184, 8).into()),
            ]),
            scale: 1.25.into(),
            offset: Point::from((-10., 3.)),
        };
        let parts = split_regions(&region).unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(
            parts[0].0,
            Rectangle::new((-10., 3.).into(), (1250., 30.).into())
        );
        assert_eq!(
            parts[1].0,
            Rectangle::new((240., 53.).into(), (250., 250.).into())
        );
        assert_eq!(parts[1].1, 10.);
        assert_eq!(
            parts[0].2.rects.len() + parts[1].2.rects.len(),
            region.rects.len()
        );
    }
    #[test]
    fn fragmented_and_empty_masks_do_not_expand_to_full_screen() {
        let mut region = TransformedRegion {
            rects: Arc::new(Vec::new()),
            scale: 1.0.into(),
            offset: (0., 0.).into(),
        };
        assert!(split_regions(&region).unwrap().is_empty());
        region.rects = Arc::new(
            (0..300)
                .map(|x| Rectangle::new((x * 3, 0).into(), (1, 1).into()))
                .collect(),
        );
        assert!(split_regions(&region).is_none());
    }
}
