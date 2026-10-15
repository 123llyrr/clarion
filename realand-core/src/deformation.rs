//! Pure scene deformation shared by every rendering device.
//!
//! These Genie/Scale splines and the 0.75 phase threshold are Realand parameters;
//! Catalina's protected Dock body does not establish a native formula. Output
//! topology uses fixed stack scratch, allocating only the immutable mesh result.

use crate::geometry::Rect;

pub const ROWS: usize = 32;
pub const COLUMNS: usize = 8;

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

pub fn minimize_vertices(
    source: Rect,
    target: Rect,
    progress: f64,
    edge: Edge,
    effect: Effect,
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
    // Stage the neck independently of travel. (REA evidence revealed that 0.75 in SkyLight
    // belonged to `_SLSCreateSheetWindowAnimationWithParent` rather than Genie; Realand uses
    // 0.75 as an internal mesh threshold where Dock-facing rows finish first).
    // Restarting smoothstep directly at 0.75 would cause a zero-derivative halt; our formulation
    // guarantees both $C^0$ position and $C^1$ velocity continuity through the phase boundary.
    const PHASE_THRESHOLD: f64 = 0.75;
    let travel = smoothstep(p);
    let horizontal = matches!(edge, Edge::Left | Edge::Right);
    let positive = matches!(edge, Edge::Bottom | Edge::Right);
    let (rows, columns) = if horizontal {
        (COLUMNS, ROWS)
    } else {
        (ROWS, COLUMNS)
    };
    let mut nodes = [([0.; 2], [0.; 2]); (ROWS + 1) * (COLUMNS + 1)];
    let mut node_count = 0;
    for row in 0..=rows {
        let v = row as f64 / rows as f64;
        for col in 0..=columns {
            let u = col as f64 / columns as f64;
            let along = if horizontal { u } else { v };
            let across = if horizontal { v } else { u };
            let towards = if positive { along } else { 1. - along };
            // Each row has its own completion time, from 0.75 at the Dock edge
            // to 1.0 at the far edge. Position and velocity agree on both sides
            // of every completion time, including the phase boundary.
            let neck_end = 1. - (1. - PHASE_THRESHOLD) * towards;
            let neck = smoothstep(p / neck_end);
            let narrow = neck.max(travel);
            let (source_cross, target_cross, source_center, target_center) = if horizontal {
                (
                    source.height,
                    target.height,
                    source.y + source.height / 2.,
                    target.y + target.height / 2.,
                )
            } else {
                (
                    source.width,
                    target.width,
                    source.x + source.width / 2.,
                    target.x + target.width / 2.,
                )
            };
            let width = source_cross + (target_cross - source_cross) * narrow;
            let center = source_center + (target_center - source_center) * narrow;
            let source_long = if horizontal {
                source.x + source.width * along
            } else {
                source.y + source.height * along
            };
            let target_long = if horizontal {
                target.x + target.width * along
            } else {
                target.y + target.height * along
            };
            let long = source_long + (target_long - source_long) * travel;
            let cross = center + (across - 0.5) * width;
            let position = if horizontal {
                [long, cross]
            } else {
                [cross, long]
            };
            nodes[node_count] = (position, [u as f32, v as f32]);
            node_count += 1;
        }
    }
    let nodes = &nodes[..node_count];
    let min_x = nodes.iter().map(|n| n.0[0]).fold(f64::INFINITY, f64::min);
    let min_y = nodes.iter().map(|n| n.0[1]).fold(f64::INFINITY, f64::min);
    let max_x = nodes
        .iter()
        .map(|n| n.0[0])
        .fold(f64::NEG_INFINITY, f64::max);
    let max_y = nodes
        .iter()
        .map(|n| n.0[1])
        .fold(f64::NEG_INFINITY, f64::max);
    let area = Rect::new(min_x, min_y, max_x - min_x, max_y - min_y);
    let vertex = |idx: usize| MeshVertex {
        position: [
            ((nodes[idx].0[0] - min_x) / area.width.max(f64::EPSILON)) as f32,
            ((nodes[idx].0[1] - min_y) / area.height.max(f64::EPSILON)) as f32,
        ],
        uv: nodes[idx].1,
    };
    let mut vertices = Vec::with_capacity(rows * columns * 6);
    for row in 0..rows {
        for col in 0..columns {
            let a = row * (columns + 1) + col;
            let b = a + 1;
            let c = a + columns + 1;
            let d = c + 1;
            vertices.extend([
                vertex(a),
                vertex(b),
                vertex(c),
                vertex(b),
                vertex(d),
                vertex(c),
            ]);
        }
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
}
