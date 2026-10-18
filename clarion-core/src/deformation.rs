//! Pure scene deformation shared by every rendering device.
//!
//! Genie uses recovered geometry-dependent temporal phases, with a separately
//! designed, monotonic spatial contour. It does not reproduce the complete
//! private Apple warp. Mesh generation allocates only its final triangle list.

use crate::geometry::Rect;

/// A tessellation target in physical output pixels. This is Clarion's rendering
/// policy, not a constant recovered from Dock. Extreme inputs can reach the
/// hard segment cap before meeting this target; their shape stays unchanged.
pub const GENIE_MAX_ERROR_PX: f64 = 0.5;
pub const GENIE_MAX_SEGMENTS: usize = 1024;

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

/// Sample at logical scale 1. Render adapters should use
/// [`minimize_vertices_with_scale`] for the actual output's pixel budget.
pub fn minimize_vertices(
    source: Rect,
    target: Rect,
    progress: f64,
    edge: Edge,
    effect: Effect,
) -> (Rect, Vec<MeshVertex>) {
    minimize_vertices_with_scale(source, target, progress, edge, effect, 1.)
}

/// The output scale affects tessellation, not animation state or UVs. Scale
/// retains its aspect fitting and existing smoothstep interpolation because the
/// report did not establish Big Sur Scale's complete timing function.
pub fn minimize_vertices_with_scale(
    source: Rect,
    target: Rect,
    progress: f64,
    edge: Edge,
    effect: Effect,
    physical_scale: f64,
) -> (Rect, Vec<MeshVertex>) {
    minimize_vertices_with_geometry(
        source,
        target,
        source,
        progress,
        edge,
        effect,
        physical_scale,
    )
}

/// Render the whole source texture while deriving temporal phases only from
/// the frozen window frame. Shadow padding and per-output texture rounding
/// must not change the geometry-dependent stage weights.
pub fn minimize_vertices_with_geometry(
    source: Rect,
    target: Rect,
    phase_source: Rect,
    progress: f64,
    edge: Edge,
    effect: Effect,
    physical_scale: f64,
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
    genie_vertices(
        source,
        target,
        phase_source.sanitized(),
        p,
        edge,
        physical_scale,
    )
}

/// Interpret the recovered signed geometry directly in Wayland's downwards Y
/// coordinates. Reflection maps Top/Bottom and Left/Right symmetrically; the
/// report's raw numeric direction values are not reused as platform enums.
fn genie_phases(
    source: Rect,
    target: Rect,
    edge: Edge,
) -> Option<clarion_motion::genie::GeniePhases> {
    let (delta_left, delta_right, gap, length, extent) = match edge {
        Edge::Top => {
            let gap = (target.bottom() - source.y).max(0.);
            (
                target.x - source.x,
                target.right() - source.right(),
                gap,
                if gap > 0. {
                    -source.height
                } else {
                    target.bottom() - source.bottom()
                },
                -source.height,
            )
        }
        Edge::Bottom => {
            let gap = (target.y - source.bottom()).min(0.);
            (
                target.x - source.x,
                target.right() - source.right(),
                gap,
                if gap < 0. {
                    source.height
                } else {
                    target.y - source.y
                },
                source.height,
            )
        }
        Edge::Left => {
            let gap = (target.right() - source.x).max(0.);
            (
                target.y - source.y,
                target.bottom() - source.bottom(),
                gap,
                if gap > 0. {
                    -source.width
                } else {
                    target.right() - source.right()
                },
                -source.width,
            )
        }
        Edge::Right => {
            let gap = (target.x - source.right()).min(0.);
            (
                target.y - source.y,
                target.bottom() - source.bottom(),
                gap,
                if gap < 0. {
                    source.width
                } else {
                    target.x - source.x
                },
                source.width,
            )
        }
    };
    clarion_motion::genie::GeniePhases::new(delta_left, delta_right, gap, length, extent)
}

#[derive(Clone, Copy, Default)]
struct ContourNode {
    along: f64,
    edges: [f64; 2],
}

/// A strip with an affine longitudinal coordinate cannot fold: every row stays
/// ordered and both boundary widths are convex combinations of positive source
/// and target widths. Cross-only narrowing must not move all rows to one place.
///
/// The cubic *spatial* contour below is an independent Clarion approximation.
/// Recovered half-cosines are evaluated only by the two temporal phases. Shared
/// rows are evaluated once, with two cross-axis vertices and original UVs.
fn genie_vertices(
    source: Rect,
    target: Rect,
    phase_source: Rect,
    progress: f64,
    edge: Edge,
    physical_scale: f64,
) -> (Rect, Vec<MeshVertex>) {
    let p = if progress.is_nan() {
        0.
    } else {
        progress.clamp(0., 1.)
    };
    // Rectangular endpoints need no tessellation and retain the original
    // geometry exactly, including negative/fractional output coordinates.
    if p == 0. {
        return rectangle_vertices(source);
    }
    if p == 1. {
        return rectangle_vertices(target);
    }
    let (cross, travel) = genie_phases(phase_source, target, edge)
        .map(|phases| phases.sample(p))
        // The recovered phase formula is undefined for certain overlapping or
        // zero-sized geometries. A single half-cosine is our finite fallback.
        .unwrap_or_else(|| {
            let t = clarion_motion::genie::half_cosine(p);
            (t, t)
        });
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
    let scale = if physical_scale.is_finite() && physical_scale > 0. {
        physical_scale
    } else {
        1.
    };
    let maximum_delta_px = delta[0].abs().max(delta[1].abs()) * scale;
    let width_delta_px = (delta[1] - delta[0]).abs() * scale;
    let amplitude = cross * (1. - cross);
    let baseline = cross * cross;
    let contour = |along: f64| {
        let towards = if positive { along } else { 1. - along };
        let blend = baseline + amplitude * smoothstep(towards);
        ContourNode {
            along,
            edges: [
                source_edges[0] + delta[0] * blend,
                source_edges[1] + delta[1] * blend,
            ],
        }
    };
    let error_bound = |left: f64, right: f64| {
        // The boundary error is M*h²/8. GPU triangles also approximate the
        // bilinear product of row position and across-axis UV: |Δwidth|/4.
        // Omitting that mixed term would validate the outline but allow visible
        // texture distortion inside otherwise straight-sided trapezoids.
        let curvature = (6. - 12. * left).abs().max((6. - 12. * right).abs());
        let boundary = maximum_delta_px * amplitude * curvature * (right - left).powi(2) / 8.;
        let mixed = width_delta_px * amplitude * (smoothstep(right) - smoothstep(left)).abs() / 4.;
        boundary + mixed
    };
    // Depth-first bisection produces ordered rows without a temporary Vec or
    // recursive allocations. At most 1024 segments fit the fixed stack scratch.
    // At the hard cap the original contour takes priority over the error target:
    // extreme inputs keep their shape and receive best-effort tessellation.
    let mut pending = [(0., 0.); 11];
    let mut pending_count = 1;
    pending[0] = (0., 1.);
    let mut nodes = [ContourNode::default(); GENIE_MAX_SEGMENTS + 1];
    nodes[0] = contour(0.);
    let mut node_count = 1;
    while pending_count > 0 {
        pending_count -= 1;
        let (left, right) = pending[pending_count];
        if error_bound(left, right) > GENIE_MAX_ERROR_PX
            && right - left > 1. / GENIE_MAX_SEGMENTS as f64
        {
            let middle = (left + right) / 2.;
            pending[pending_count] = (middle, right);
            pending[pending_count + 1] = (left, middle);
            pending_count += 2;
        } else {
            nodes[node_count] = contour(right);
            node_count += 1;
        }
    }
    let nodes = &nodes[..node_count];
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
    let vertex = |node: &ContourNode, across: usize| {
        let normalized_cross =
            (node.edges[across] - min_cross) / (max_cross - min_cross).max(f64::EPSILON);
        let (position, uv) = if horizontal {
            ([node.along, normalized_cross], [node.along, across as f64])
        } else {
            ([normalized_cross, node.along], [across as f64, node.along])
        };
        MeshVertex {
            position: position.map(|v| v as f32),
            uv: uv.map(|v| v as f32),
        }
    };
    let mut vertices = Vec::with_capacity((node_count - 1) * 6);
    let mut previous = [vertex(&nodes[0], 0), vertex(&nodes[0], 1)];
    for node in &nodes[1..] {
        // Adjacent strips share a row. Normalize it once without changing the
        // triangle order, UVs, or numerical calculation for either endpoint.
        let next = [vertex(node, 0), vertex(node, 1)];
        let [a, b] = previous;
        let [c, d] = next;
        if horizontal {
            vertices.extend([a, c, b, c, d, b]);
        } else {
            vertices.extend([a, b, c, b, d, c]);
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
                    assert!(vertices.len() <= GENIE_MAX_SEGMENTS * 6);
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
    fn adaptive_strip_stays_within_physical_pixel_budget() {
        let source = Rect::new(-480., -320., 1200., 800.);
        for (edge, target) in targets() {
            let horizontal = matches!(edge, Edge::Left | Edge::Right);
            let positive = matches!(edge, Edge::Bottom | Edge::Right);
            for scale in [1., 1.25, 1.5, 2.] {
                for progress in [0.1, 0.3, 0.5, 0.7, 0.9] {
                    let (cross, _) = genie_phases(source, target, edge).unwrap().sample(progress);
                    let (area, vertices) = sample(source, target, progress, edge, scale);
                    let source_edges = if horizontal {
                        [source.y, source.bottom()]
                    } else {
                        [source.x, source.right()]
                    };
                    let target_edges = if horizontal {
                        [target.y, target.bottom()]
                    } else {
                        [target.x, target.right()]
                    };
                    let amplitude = cross * (1. - cross);
                    for segment in vertices.as_chunks::<6>().0 {
                        // Locate both boundaries of this strip independent of
                        // the triangle winding chosen for horizontal outputs.
                        let longitudinal = if horizontal { 0 } else { 1 };
                        let cross_axis = 1 - longitudinal;
                        let left = segment
                            .iter()
                            .map(|v| v.uv[longitudinal] as f64)
                            .fold(f64::INFINITY, f64::min);
                        let right = segment
                            .iter()
                            .map(|v| v.uv[longitudinal] as f64)
                            .fold(f64::NEG_INFINITY, f64::max);
                        for boundary in [0, 1] {
                            let endpoint = |at: f64| {
                                *segment
                                    .iter()
                                    .find(|v| {
                                        v.uv[longitudinal] as f64 == at
                                            && v.uv[cross_axis] == boundary as f32
                                    })
                                    .unwrap()
                            };
                            let a = world(area, endpoint(left))[cross_axis];
                            let b = world(area, endpoint(right))[cross_axis];
                            for fraction in [0.25, 0.5, 0.75] {
                                let t = left + (right - left) * fraction;
                                let towards = if positive { t } else { 1. - t };
                                let k = cross * cross + amplitude * smoothstep(towards);
                                let exact = source_edges[boundary]
                                    + (target_edges[boundary] - source_edges[boundary]) * k;
                                let rendered = a + (b - a) * fraction;
                                assert!(
                                    (exact - rendered).abs() * scale <= GENIE_MAX_ERROR_PX + 0.0002,
                                    "error {} at {edge:?}, progress {progress}, scale {scale}",
                                    (exact - rendered).abs() * scale
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn interior_gpu_triangle_interpolation_obeys_the_pixel_budget() {
        let source = Rect::new(-480., -320., 1600., 1000.);
        for (edge, target) in targets() {
            let horizontal = matches!(edge, Edge::Left | Edge::Right);
            let positive = matches!(edge, Edge::Bottom | Edge::Right);
            let source_edges = if horizontal {
                [source.y, source.bottom()]
            } else {
                [source.x, source.right()]
            };
            let target_edges = if horizontal {
                [target.y, target.bottom()]
            } else {
                [target.x, target.right()]
            };
            let delta = [
                target_edges[0] - source_edges[0],
                target_edges[1] - source_edges[1],
            ];
            for scale in [1., 1.25, 1.5, 2.] {
                for progress in [0.1, 0.3, 0.5, 0.7, 0.9] {
                    let (cross, travel) =
                        genie_phases(source, target, edge).unwrap().sample(progress);
                    let amplitude = cross * (1. - cross);
                    let (area, vertices) = sample(source, target, progress, edge, scale);
                    for triangle in vertices.as_chunks::<3>().0 {
                        for weights in [
                            [1. / 3.; 3],
                            [0.5, 0.5, 0.],
                            [0.2, 0.3, 0.5],
                            [0.1, 0.8, 0.1],
                        ] {
                            let mut uv = [0.; 2];
                            let mut rendered = [0.; 2];
                            for (vertex, weight) in triangle.iter().zip(weights) {
                                let point = world(area, *vertex);
                                for axis in 0..2 {
                                    uv[axis] += vertex.uv[axis] as f64 * weight;
                                    rendered[axis] += point[axis] * weight;
                                }
                            }
                            let (along, across) = if horizontal {
                                (uv[0], uv[1])
                            } else {
                                (uv[1], uv[0])
                            };
                            let towards = if positive { along } else { 1. - along };
                            let k = cross * cross + amplitude * smoothstep(towards);
                            let left = source_edges[0] + delta[0] * k;
                            let right = source_edges[1] + delta[1] * k;
                            let cross_position = left + (right - left) * across;
                            let (source_start, source_length, target_start, target_length) =
                                if horizontal {
                                    (source.x, source.width, target.x, target.width)
                                } else {
                                    (source.y, source.height, target.y, target.height)
                                };
                            let long = source_start
                                + (target_start - source_start) * travel
                                + (source_length + (target_length - source_length) * travel)
                                    * along;
                            let exact = if horizontal {
                                [long, cross_position]
                            } else {
                                [cross_position, long]
                            };
                            let error = ((exact[0] - rendered[0]).powi(2)
                                + (exact[1] - rendered[1]).powi(2))
                            .sqrt()
                                * scale;
                            assert!(
                                error <= GENIE_MAX_ERROR_PX + 0.0005,
                                "interior {error}px at {edge:?}/{progress}/{scale}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn scale_is_part_of_adaptive_tessellation_and_extreme_inputs_are_bounded() {
        let source = Rect::new(0., 0., 1600., 900.);
        let target = Rect::new(750., 1100., 48., 48.);
        let counts: Vec<_> = [1., 1.25, 1.5, 2.]
            .map(|scale| sample(source, target, 0.1, Edge::Bottom, scale).1.len())
            .into();
        assert!(counts.windows(2).all(|w| w[0] <= w[1]));
        assert!(counts[3] > counts[0]);
        for source in [
            Rect::new(0., 0., 0., 0.),
            Rect::new(-1e9, -1e9, 1e9, 1e9),
            Rect::new(f64::NAN, f64::INFINITY, -2., 1.),
        ] {
            for p in [f64::NAN, f64::NEG_INFINITY, 0., 0.5, 1., f64::INFINITY] {
                for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
                    for scale in [f64::NAN, -1., 0., 2., 1e9] {
                        let (area, vertices) = sample(source, target, p, edge, scale);
                        assert!(vertices.len() <= GENIE_MAX_SEGMENTS * 6);
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
    fn fractional_outputs_share_the_same_spatial_contour_for_wide_windows() {
        let source = Rect::new(-2000., -300., 3200., 1800.);
        for (edge, target) in targets() {
            for progress in [0.1, 0.3, 0.5, 0.7, 0.9] {
                let (base_area, base_vertices) = sample(source, target, progress, edge, 1.);
                for scale in [1.25, 1.5, 2.] {
                    let (area, vertices) = sample(source, target, progress, edge, scale);
                    assert_eq!(base_area, area);
                    let by_uv: std::collections::HashMap<_, _> = vertices
                        .iter()
                        .map(|v| ([v.uv[0].to_bits(), v.uv[1].to_bits()], v))
                        .collect();
                    for base in &base_vertices {
                        let at_same_uv = by_uv[&[base.uv[0].to_bits(), base.uv[1].to_bits()]];
                        // Refinement may add rows but cannot change any existing
                        // point of the frozen shape, even near the node budget.
                        assert_eq!(base.position, at_same_uv.position);
                        assert_eq!(world(base_area, *base), world(area, *at_same_uv));
                    }
                }
            }
        }
    }
}
