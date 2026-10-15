use smithay::utils::{Logical, Point, Rectangle, Size};

use crate::render_helpers::mesh::MeshVertex;

const TARGET_STRIP_HEIGHT: f64 = 6.;
const MIN_STRIPS: usize = 24;
const MAX_STRIPS: usize = 256;

fn strip_count(height: f64) -> usize {
    ((height / TARGET_STRIP_HEIGHT).ceil() as usize).clamp(MIN_STRIPS, MAX_STRIPS)
}

/// Project one workspace face of a vertical cube into output coordinates.
pub fn face_mesh(
    size: Size<f64, Logical>,
    angle: f64,
    perspective: f64,
) -> Option<(Rectangle<f64, Logical>, Vec<MeshVertex>)> {
    if size.w <= 0. || size.h <= 0. || angle.cos().abs() < 0.00001 {
        return None;
    }

    let radius = size.h / 2.;
    let focal = size.h / perspective.clamp(0.05, 1.);
    let camera = radius + focal;
    let near_plane_focal = camera - radius;
    let center_y = radius * angle.sin();
    let center_z = radius * angle.cos();
    // Texture coordinates are interpolated linearly inside each triangle while the cube
    // projection is non-linear. Keep each strip small enough that vertical window edges do not
    // visibly turn into a staircase on high-resolution outputs.
    let strips = strip_count(size.h);

    let project = |u: f64, v: f64| {
        let local_x = (u - 0.5) * size.w;
        let local_y = (v - 0.5) * size.h;
        let world_y = center_y + local_y * angle.cos();
        let world_z = center_z - local_y * angle.sin();
        let scale = near_plane_focal / (camera - world_z).max(1.);
        Point::from((size.w / 2. + local_x * scale, size.h / 2. + world_y * scale))
    };

    let mut points = Vec::with_capacity((strips + 1) * 2);
    let mut min = Point::from((f64::INFINITY, f64::INFINITY));
    let mut max = Point::from((f64::NEG_INFINITY, f64::NEG_INFINITY));
    for row in 0..=strips {
        let v = row as f64 / strips as f64;
        for column in 0..=1 {
            let u = column as f64;
            let point = project(u, v);
            min.x = min.x.min(point.x);
            min.y = min.y.min(point.y);
            max.x = max.x.max(point.x);
            max.y = max.y.max(point.y);
            points.push((point, [u as f32, v as f32]));
        }
    }

    let area = Rectangle::from_extremities(min, max);
    if area.size.w <= 0.01 || area.size.h <= 0.01 {
        return None;
    }
    let normalize = |point: Point<f64, Logical>| {
        [
            ((point.x - area.loc.x) / area.size.w) as f32,
            ((point.y - area.loc.y) / area.size.h) as f32,
        ]
    };

    let mut vertices = Vec::with_capacity(strips * 6);
    for row in 0..strips {
        let tl = row * 2;
        let tr = tl + 1;
        let bl = (row + 1) * 2;
        let br = bl + 1;
        for idx in [tl, bl, br, tl, br, tr] {
            let (point, uv) = points[idx];
            vertices.push(MeshVertex {
                position: normalize(point),
                uv,
            });
        }
    }

    Some((area, vertices))
}

#[cfg(test)]
mod tests {
    use std::f64::consts::FRAC_PI_2;

    use approx::assert_abs_diff_eq;

    use super::*;

    #[test]
    fn cube_geometry_at_zero_is_full_output() {
        let size = Size::from((1920., 1080.));
        let (area, _) = face_mesh(size, 0., 0.65).unwrap();
        assert_abs_diff_eq!(area.loc.x, 0., epsilon = 0.001);
        assert_abs_diff_eq!(area.loc.y, 0., epsilon = 0.001);
        assert_abs_diff_eq!(area.size.w, size.w, epsilon = 0.001);
        assert_abs_diff_eq!(area.size.h, size.h, epsilon = 0.001);
    }

    #[test]
    fn adjacent_faces_move_in_opposite_vertical_directions() {
        let size = Size::from((1920., 1080.));
        let (upper, _) = face_mesh(size, -FRAC_PI_2 / 2., 0.65).unwrap();
        let (lower, _) = face_mesh(size, FRAC_PI_2 / 2., 0.65).unwrap();
        assert!(upper.loc.y < lower.loc.y);
        assert!(upper.loc.y + upper.size.h >= lower.loc.y - 0.01);
    }

    #[test]
    fn ninety_degrees_is_edge_on() {
        let size = Size::from((1920., 1080.));
        assert!(face_mesh(size, FRAC_PI_2, 0.65).is_none());
        assert!(face_mesh(size, -FRAC_PI_2, 0.65).is_none());
    }

    #[test]
    fn high_resolution_faces_use_fine_adaptive_tessellation() {
        let size = Size::from((1920., 1200.));
        let (_, vertices) = face_mesh(size, FRAC_PI_2 / 2., 0.65).unwrap();
        let strips = vertices.len() / 6;

        assert_eq!(strips, strip_count(size.h));
        assert!(strips > MIN_STRIPS);
        assert!(size.h / strips as f64 <= TARGET_STRIP_HEIGHT);
    }

    #[test]
    fn tessellation_is_bounded_for_extreme_output_sizes() {
        assert_eq!(strip_count(1.), MIN_STRIPS);
        assert_eq!(strip_count(100_000.), MAX_STRIPS);
    }
}
