//! Pure scene deformation shared by every rendering device.
//!
//! Genie uses Clarion's continuous, progressive row narrowing. Window travel
//! starts immediately and the far row finishes last, avoiding an early uniform
//! thin rectangle. A fixed grid bounds CPU work at every output scale. This is
//! an independent visual design, not a reconstruction of Apple's full warp.

use crate::geometry::Rect;

/// A bounded two-dimensional grid keeps texture interpolation smooth without
/// making per-frame CPU/GPU work depend on window size or output scaling.
pub const GENIE_LONG_SEGMENTS: usize = 32;
pub const GENIE_CROSS_SEGMENTS: usize = 4;
pub const GENIE_MAX_VERTICES: usize = GENIE_LONG_SEGMENTS * GENIE_CROSS_SEGMENTS * 6;

// Clarion's own progressive neck timing, not a recovered Apple warp. Rows at
// the Dock finish first, while the far row remains in motion until progress 1.
const NECK_COMPLETION: f64 = 0.75;
const ROW_COMPLETION_INVERSE: [f64; GENIE_LONG_SEGMENTS + 1] = {
    let mut inverse = [0.; GENIE_LONG_SEGMENTS + 1];
    let mut row = 0;
    while row <= GENIE_LONG_SEGMENTS {
        inverse[row] = 1. / (1. - (1. - NECK_COMPLETION) * row as f64 / GENIE_LONG_SEGMENTS as f64);
        row += 1;
    }
    inverse
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    Genie,
    Scale,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshVertex {
    pub position: [f32; 2],
    pub uv: [f32; 2],
}

pub fn smoothstep(t: f64) -> f64 {
    let t = t.clamp(0., 1.);
    t * t * (3. - 2. * t)
}

/// Sample the same geometry on every output.
pub fn minimize_vertices(
    source: Rect,
    target: Rect,
    progress: f64,
    edge: Edge,
    effect: Effect,
) -> (Rect, Vec<MeshVertex>) {
    minimize_vertices_with_scale(source, target, progress, edge, effect, 1.)
}

/// The fixed mesh is output-scale independent. Keep this adapter entry point so
/// callers can retain their frozen output/texture identity without changing the
/// shape or increasing the work at fractional and high-DPI scales.
pub fn minimize_vertices_with_scale(
    source: Rect,
    target: Rect,
    progress: f64,
    edge: Edge,
    effect: Effect,
    _physical_scale: f64,
) -> (Rect, Vec<MeshVertex>) {
    let source = source.sanitized();
    let target = target.sanitized();
    let p = if progress.is_nan() {
        0.
    } else {
        progress.clamp(0., 1.)
    };
    if effect == Effect::Scale {
        return scale_vertices(source, target, p);
    }
    genie_vertices(source, target, p, edge)
}

#[derive(Clone, Copy, Default)]
struct ContourNode {
    along: f64,
    edges: [f64; 2],
}

fn genie_vertices(source: Rect, target: Rect, p: f64, edge: Edge) -> (Rect, Vec<MeshVertex>) {
    // Exact rectangular endpoints do not need a deformation mesh.
    if p == 0. {
        return rectangle_vertices(source);
    }
    if p == 1. {
        return rectangle_vertices(target);
    }
    let travel = smoothstep(p);
    let horizontal = matches!(edge, Edge::Left | Edge::Right);
    let positive = matches!(edge, Edge::Bottom | Edge::Right);
    let (source_start, source_length, target_start, target_length, source_edges, target_edges) =
        if horizontal {
            (
                source.x,
                source.width,
                target.x,
                target.width,
                [source.y, source.bottom()],
                [target.y, target.bottom()],
            )
        } else {
            (
                source.y,
                source.height,
                target.y,
                target.height,
                [source.x, source.right()],
                [target.x, target.right()],
            )
        };
    let delta = [
        target_edges[0] - source_edges[0],
        target_edges[1] - source_edges[1],
    ];
    let mut nodes = [ContourNode::default(); GENIE_LONG_SEGMENTS + 1];
    for (row, node) in nodes.iter_mut().enumerate() {
        let along = row as f64 / GENIE_LONG_SEGMENTS as f64;
        let towards = if positive {
            row
        } else {
            GENIE_LONG_SEGMENTS - row
        };
        // One interpolation per row; division is precomputed and no transcendental
        // functions, recursive subdivision or temporary allocations are needed.
        // smoothstep reaches each row's completion with zero velocity, so there
        // is no motion kink when a Dock-facing row finishes before its neighbor.
        let narrow = smoothstep(p * ROW_COMPLETION_INVERSE[towards]);
        *node = ContourNode {
            along,
            edges: [
                source_edges[0] + delta[0] * narrow,
                source_edges[1] + delta[1] * narrow,
            ],
        };
    }
    let long_start = source_start + (target_start - source_start) * travel;
    let long_length = source_length + (target_length - source_length) * travel;
    let min_cross = nodes
        .iter()
        .map(|n| n.edges[0])
        .fold(f64::INFINITY, f64::min);
    let max_cross = nodes
        .iter()
        .map(|n| n.edges[1])
        .fold(f64::NEG_INFINITY, f64::max);
    let area = if horizontal {
        Rect::new(long_start, min_cross, long_length, max_cross - min_cross)
    } else {
        Rect::new(min_cross, long_start, max_cross - min_cross, long_length)
    };
    let cross_length = (max_cross - min_cross).max(f64::EPSILON);
    let row_vertices = |node: &ContourNode| {
        std::array::from_fn::<_, { GENIE_CROSS_SEGMENTS + 1 }, _>(|column| {
            let across = column as f64 / GENIE_CROSS_SEGMENTS as f64;
            let cross = node.edges[0] + (node.edges[1] - node.edges[0]) * across;
            let normalized_cross = (cross - min_cross) / cross_length;
            let (position, uv) = if horizontal {
                ([node.along, normalized_cross], [node.along, across])
            } else {
                ([normalized_cross, node.along], [across, node.along])
            };
            MeshVertex {
                position: position.map(|v| v as f32),
                uv: uv.map(|v| v as f32),
            }
        })
    };
    let mut vertices = Vec::with_capacity(GENIE_MAX_VERTICES);
    let mut previous = row_vertices(&nodes[0]);
    for node in &nodes[1..] {
        let next = row_vertices(node);
        for column in 0..GENIE_CROSS_SEGMENTS {
            let a = previous[column];
            let b = previous[column + 1];
            let c = next[column];
            let d = next[column + 1];
            if horizontal {
                vertices.extend([a, c, b, c, d, b]);
            } else {
                vertices.extend([a, b, c, b, d, c]);
            }
        }
        previous = next;
    }
    (area, vertices)
}

/// Uniform scaling keeps the window's original aspect ratio. The fitted final
/// rectangle is centered inside the icon, including a nonsquare target.
fn scale_vertices(source: Rect, target: Rect, progress: f64) -> (Rect, Vec<MeshVertex>) {
    let t = smoothstep(progress);
    let final_scale = (target.width / source.width.max(f64::EPSILON))
        .min(target.height / source.height.max(f64::EPSILON));
    let scale = 1. + (final_scale - 1.) * t;
    let source_center = [source.x + source.width / 2., source.y + source.height / 2.];
    let target_center = [target.x + target.width / 2., target.y + target.height / 2.];
    let center = [
        source_center[0] + (target_center[0] - source_center[0]) * t,
        source_center[1] + (target_center[1] - source_center[1]) * t,
    ];
    let width = source.width * scale;
    let height = source.height * scale;
    let area = if progress <= 0. {
        source
    } else {
        Rect::new(
            center[0] - width / 2.,
            center[1] - height / 2.,
            width,
            height,
        )
    };
    rectangle_vertices(area)
}

fn rectangle_vertices(area: Rect) -> (Rect, Vec<MeshVertex>) {
    let vertex = |position| MeshVertex {
        position,
        uv: position,
    };
    (
        area,
        vec![
            vertex([0., 0.]),
            vertex([1., 0.]),
            vertex([0., 1.]),
            vertex([1., 0.]),
            vertex([1., 1.]),
            vertex([0., 1.]),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_scene_inputs_cannot_reach_gpu_as_nonfinite_vertices() {
        for effect in [Effect::Genie, Effect::Scale] {
            for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
                for progress in [f64::NAN, f64::NEG_INFINITY, f64::INFINITY, 0.5] {
                    let (area, vertices) = minimize_vertices(
                        Rect::new(f64::INFINITY, f64::NAN, -10., f64::INFINITY),
                        Rect::new(100., 100., 32., 32.),
                        progress,
                        edge,
                        effect,
                    );
                    assert!([area.x, area.y, area.width, area.height]
                        .iter()
                        .all(|v| v.is_finite()));
                    assert!(vertices.iter().all(|v| v
                        .position
                        .iter()
                        .chain(&v.uv)
                        .all(|x| x.is_finite())));
                }
            }
        }
    }

    fn sample(
        source: Rect,
        target: Rect,
        p: f64,
        edge: Edge,
        scale: f64,
    ) -> (Rect, Vec<MeshVertex>) {
        minimize_vertices_with_scale(source, target, p, edge, Effect::Genie, scale)
    }

    fn world(area: Rect, vertex: MeshVertex) -> [f64; 2] {
        [
            area.x + area.width * vertex.position[0] as f64,
            area.y + area.height * vertex.position[1] as f64,
        ]
    }

    fn targets() -> [(Edge, Rect); 4] {
        [
            (Edge::Top, Rect::new(-200., -500., 64., 64.)),
            (Edge::Bottom, Rect::new(-200., 300., 64., 64.)),
            (Edge::Left, Rect::new(-700., -100., 64., 64.)),
            (Edge::Right, Rect::new(400., -100., 64., 64.)),
        ]
    }

    #[test]
    fn default_sampling_uses_the_same_geometry_as_scale_one() {
        let source = Rect::new(-480., -320., 700., 500.);
        for (edge, target) in targets() {
            for effect in [Effect::Genie, Effect::Scale] {
                for progress in [0., 0.15, 0.7, 0.75, 0.9, 1.] {
                    assert_eq!(
                        minimize_vertices(source, target, progress, edge, effect),
                        minimize_vertices_with_scale(source, target, progress, edge, effect, 1.),
                    );
                }
            }
        }
    }

    #[test]
    fn genie_strip_preserves_endpoints_uv_and_positive_winding_for_all_edges() {
        let source = Rect::new(-480., -320., 700., 500.);
        for (edge, target) in targets() {
            for scale in [1., 1.25, 1.5, 2.] {
                for progress in [0., 0.1, 0.3, 0.6, 0.85, 0.99, 1.] {
                    let (area, vertices) = sample(source, target, progress, edge, scale);
                    assert!(vertices.len() <= GENIE_MAX_VERTICES);
                    assert!(vertices.iter().all(|v| v
                        .position
                        .iter()
                        .chain(v.uv.iter())
                        .all(|n| n.is_finite() && (0. ..=1.).contains(n))));
                    for triangle in vertices.as_chunks::<3>().0 {
                        let a = triangle[0].position;
                        let b = triangle[1].position;
                        let c = triangle[2].position;
                        assert!(
                            (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]) > 0.,
                            "fold at {edge:?}, {progress}"
                        );
                    }
                    if progress == 0. || progress == 1. {
                        let expected = if progress == 0. { source } else { target };
                        assert_eq!(area, expected);
                        for vertex in vertices {
                            let actual = world(area, vertex);
                            assert!(
                                (actual[0] - expected.x - expected.width * vertex.uv[0] as f64)
                                    .abs()
                                    < 0.0001
                            );
                            assert!(
                                (actual[1] - expected.y - expected.height * vertex.uv[1] as f64)
                                    .abs()
                                    < 0.0001
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn progressive_neck_retains_a_taper_and_continues_traveling_until_completion() {
        let source = Rect::new(0., 0., 1000., 700.);
        let target = Rect::new(450., 1000., 48., 48.);
        let mut last_start = f64::NEG_INFINITY;
        for progress in [0.05, 0.25, 0.5, 0.65, 0.75, 0.85, 0.95] {
            let (area, vertices) = sample(source, target, progress, Edge::Bottom, 2.);
            let corner = |uv| world(area, *vertices.iter().find(|v| v.uv == uv).unwrap());
            let far_width = corner([1., 0.])[0] - corner([0., 0.])[0];
            let near_width = corner([1., 1.])[0] - corner([0., 1.])[0];
            assert!(
                far_width > near_width,
                "premature straight strip at {progress}"
            );
            assert!(
                far_width > target.width,
                "far row completed early at {progress}"
            );
            assert!(area.y > last_start, "travel stalled at {progress}");
            last_start = area.y;
        }
    }

    #[test]
    fn work_budget_is_fixed_for_large_windows_and_fractional_outputs() {
        let target = Rect::new(750., 1100., 48., 48.);
        for source in [
            Rect::new(0., 0., 1600., 900.),
            Rect::new(0., 0., 0., 0.),
            Rect::new(-1e9, -1e9, 1e9, 1e9),
            Rect::new(f64::NAN, f64::INFINITY, -2., 1.),
        ] {
            for p in [f64::NAN, f64::NEG_INFINITY, 0., 0.5, 1., f64::INFINITY] {
                for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
                    let reference = sample(source, target, p, edge, 1.);
                    for scale in [f64::NAN, -1., 0., 1.25, 1.5, 2., 1e9] {
                        let (area, vertices) = sample(source, target, p, edge, scale);
                        assert_eq!(reference.0, area);
                        assert_eq!(reference.1, vertices);
                        assert!(vertices.len() <= GENIE_MAX_VERTICES);
                        assert!([area.x, area.y, area.width, area.height]
                            .iter()
                            .all(|v| v.is_finite()));
                        assert!(vertices.iter().all(|v| v
                            .position
                            .iter()
                            .chain(v.uv.iter())
                            .all(|v| v.is_finite())));
                    }
                }
            }
        }
    }

    #[test]
    fn directional_mapping_agrees_under_reflection_and_axis_swap() {
        let source = Rect::new(-480., -320., 700., 500.);
        let target = Rect::new(-200., 300., 64., 64.);
        let transpose = |r: Rect| Rect::new(r.y, r.x, r.height, r.width);
        let reflect = |r: Rect| Rect::new(r.x, -r.bottom(), r.width, r.height);
        for p in [0., 0.1, 0.35, 0.6, 0.9, 1.] {
            let (bottom_area, bottom_vertices) = sample(source, target, p, Edge::Bottom, 1.5);
            let (right_area, right_vertices) =
                sample(transpose(source), transpose(target), p, Edge::Right, 1.5);
            let (top_area, top_vertices) =
                sample(reflect(source), reflect(target), p, Edge::Top, 1.5);
            for (actual, expected) in [
                (right_area, transpose(bottom_area)),
                (top_area, reflect(bottom_area)),
            ] {
                assert!((actual.x - expected.x).abs() < 1e-8);
                assert!((actual.y - expected.y).abs() < 1e-8);
                assert!((actual.width - expected.width).abs() < 1e-8);
                assert!((actual.height - expected.height).abs() < 1e-8);
            }
            for vertex in &bottom_vertices {
                let original = world(bottom_area, *vertex);
                let swapped = *right_vertices
                    .iter()
                    .find(|v| v.uv == [vertex.uv[1], vertex.uv[0]])
                    .unwrap();
                let swapped = world(right_area, swapped);
                assert!((swapped[0] - original[1]).abs() < 0.0001);
                assert!((swapped[1] - original[0]).abs() < 0.0001);
                let flipped = *top_vertices
                    .iter()
                    .find(|v| v.uv == [vertex.uv[0], 1. - vertex.uv[1]])
                    .unwrap();
                let flipped = world(top_area, flipped);
                assert!((flipped[0] - original[0]).abs() < 0.0001);
                assert!((flipped[1] + original[1]).abs() < 0.0001);
            }
        }
    }

    #[test]
    fn fractional_and_large_coordinate_endpoints_are_exact_rectangles() {
        let source = Rect::new(-123456.789, -0.125, 1720.375, 900.625);
        let target = Rect::new(0.1, 1200.3, 47.9, 48.1);
        for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
            for (progress, expected) in [(0., source), (1., target)] {
                let (area, vertices) = sample(source, target, progress, edge, 1.25);
                assert_eq!(area, expected);
                assert_eq!(vertices.len(), 6);
                assert!(vertices.iter().all(|v| v.position == v.uv));
            }
        }
    }

    #[test]
    fn fractional_outputs_share_the_exact_mesh_for_wide_windows() {
        let source = Rect::new(-2000., -300., 3200., 1800.);
        for (edge, target) in targets() {
            for progress in [0.1, 0.3, 0.5, 0.7, 0.9] {
                let reference = sample(source, target, progress, edge, 1.);
                for scale in [1.25, 1.5, 2.] {
                    assert_eq!(reference, sample(source, target, progress, edge, scale));
                }
            }
        }
    }

    #[test]
    fn each_row_finishes_narrowing_with_continuous_velocity() {
        let source = Rect::new(0., 0., 100., 100.);
        let target = Rect::new(45., 150., 10., 10.);
        let h = 0.0005;
        for row in 0..=GENIE_LONG_SEGMENTS {
            let along = row as f32 / GENIE_LONG_SEGMENTS as f32;
            let completion = 1. - (1. - NECK_COMPLETION) * row as f64 / GENIE_LONG_SEGMENTS as f64;
            let at = |p| {
                let (area, vertices) = sample(source, target, p, Edge::Bottom, 1.);
                world(
                    area,
                    *vertices.iter().find(|v| v.uv == [0., along]).unwrap(),
                )[0]
            };
            let before = at(completion - h);
            let middle = at(completion);
            let after = at(completion + h);
            assert!(
                ((middle - before) / h - (after - middle) / h).abs() < 1.,
                "row {row} has a velocity kink"
            );
        }
    }
}
