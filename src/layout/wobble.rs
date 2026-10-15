use std::rc::Rc;
use std::time::Duration;

use realand_config::WindowWobble;
use smithay::backend::renderer::element::Id;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::utils::{Logical, Point, Rectangle, Size};

use crate::render_helpers::mesh::MeshVertex;
use crate::utils::ResizeEdge;

const STEP: f64 = 1. / 120.;
const MAX_ELAPSED: f64 = 0.05;
const MAX_SETTLE_TIME: f64 = 5.;

#[derive(Debug, Clone, Copy)]
pub enum WobbleAnchor {
    Point((f64, f64)),
    Edges {
        left: bool,
        right: bool,
        top: bool,
        bottom: bool,
    },
}

impl WobbleAnchor {
    pub fn from_resize_edges(edges: ResizeEdge) -> Self {
        Self::Edges {
            left: edges.contains(ResizeEdge::LEFT),
            right: edges.contains(ResizeEdge::RIGHT),
            top: edges.contains(ResizeEdge::TOP),
            bottom: edges.contains(ResizeEdge::BOTTOM),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Node {
    offset: [f64; 2],
    velocity: [f64; 2],
}

#[derive(Debug)]
pub struct WobbleState {
    id: Id,
    commit: CommitCounter,
    nodes: Vec<Node>,
    scratch: Vec<Node>,
    /// Reusable render-only state for the remainder of a fixed physics step.
    projected: Vec<Node>,
    points: Vec<(Point<f64, Logical>, [f32; 2])>,
    cached_mesh: Option<CachedMesh>,
    mesh: usize,
    anchor: Option<WobbleAnchor>,
    active: bool,
    dynamic: bool,
    last_area: Option<Rectangle<f64, Logical>>,
    /// Visual movement accumulated before the first rendered frame of a grab.
    ///
    /// Pointer grabs may recognize and apply several motion events before the compositor gets a
    /// chance to render. Without preserving that displacement, the first frame becomes the
    /// physics baseline and the window appears to jump before it starts wobbling.
    pending_displacement: Point<f64, Logical>,
    last_update: Option<Duration>,
    accumulator: f64,
    settle_elapsed: f64,
    source_state: Option<(CommitCounter, u64)>,
}

impl Default for WobbleState {
    fn default() -> Self {
        Self {
            id: Id::new(),
            commit: CommitCounter::default(),
            nodes: Vec::new(),
            scratch: Vec::new(),
            projected: Vec::new(),
            points: Vec::new(),
            cached_mesh: None,
            mesh: 0,
            anchor: None,
            active: false,
            dynamic: false,
            last_area: None,
            pending_displacement: Point::from((0., 0.)),
            last_update: None,
            accumulator: 0.,
            settle_elapsed: 0.,
            source_state: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WobbleMesh {
    pub id: Id,
    pub commit: CommitCounter,
    pub area: Rectangle<f64, Logical>,
    pub vertices: Rc<[MeshVertex]>,
}

#[derive(Debug)]
struct CachedMesh {
    source_area: Rectangle<f64, Logical>,
    area: Rectangle<f64, Logical>,
    vertices: Rc<[MeshVertex]>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ForecastKey {
    owner_commit: CommitCounter,
    area: Rectangle<f64, Logical>,
    config: WindowWobble,
    now: Duration,
    source_commit: CommitCounter,
    source_texture: u64,
}

/// Renderer-owned bounded scratch and damage cache. Forecast integration never
/// changes the owner-thread event simulation, including its settled deadline.
#[derive(Debug, Default)]
pub struct WobbleForecast {
    state: WobbleState,
    key: Option<ForecastKey>,
    cached: Option<WobbleMesh>,
    commit: CommitCounter,
}

impl WobbleForecast {
    pub fn clear(&mut self) {
        self.state.cancel();
        self.key = None;
        self.cached = None;
    }

    pub fn sample(
        &mut self,
        owner: &WobbleState,
        area: Rectangle<f64, Logical>,
        config: WindowWobble,
        now: Duration,
        source_commit: CommitCounter,
        source_texture: u64,
    ) -> Option<WobbleMesh> {
        let key = ForecastKey {
            owner_commit: owner.commit,
            area,
            config,
            now,
            source_commit,
            source_texture,
        };
        if self.key == Some(key) {
            return self.cached.clone();
        }
        self.state.copy_simulation_from(owner);
        // Translation, output-local rebases and texture/CSD padding are presentation
        // geometry. The actual movement/resize was already applied at event time.
        self.state.last_area = owner.last_area.map(|_| area);
        self.key = Some(key);
        self.commit.increment();
        self.cached = self
            .state
            .update(area, config, now, source_commit, source_texture)
            .map(|mut mesh| {
                mesh.commit = self.commit;
                mesh
            });
        self.cached.clone()
    }
}

impl WobbleState {
    pub fn begin(&mut self, anchor: WobbleAnchor, config: WindowWobble, now: Duration) {
        if config.off {
            self.cancel();
            return;
        }

        self.ensure_mesh(config.mesh as usize);
        self.anchor = Some(anchor);
        self.active = true;
        self.dynamic = false;
        self.last_area = None;
        self.pending_displacement = Point::from((0., 0.));
        self.last_update = Some(now);
        self.accumulator = 0.;
        self.settle_elapsed = 0.;
        self.source_state = None;
        self.commit.increment();
    }

    pub fn finish(&mut self) {
        if self.active {
            self.anchor = None;
            self.dynamic = true;
            self.commit.increment();
        }
    }

    pub fn cancel(&mut self) {
        self.nodes.clear();
        self.scratch.clear();
        self.projected.clear();
        self.cached_mesh = None;
        self.anchor = None;
        self.active = false;
        self.dynamic = false;
        self.last_area = None;
        self.pending_displacement = Point::from((0., 0.));
        self.last_update = None;
        self.accumulator = 0.;
        self.settle_elapsed = 0.;
        self.source_state = None;
        self.commit.increment();
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn is_ongoing(&self) -> bool {
        self.active && self.dynamic
    }

    /// Preserve motion that happens before the first wobble frame is rendered.
    pub fn add_pending_displacement(&mut self, delta: Point<f64, Logical>) {
        if self.active && self.last_area.is_none() && delta.x.is_finite() && delta.y.is_finite() {
            self.pending_displacement += delta;
        }
    }

    pub fn update(
        &mut self,
        area: Rectangle<f64, Logical>,
        config: WindowWobble,
        now: Duration,
        source_commit: CommitCounter,
        source_texture: u64,
    ) -> Option<WobbleMesh> {
        let render_changed =
            self.advance_simulation(area, config, now, source_commit, source_texture)?;
        if render_changed && self.dynamic && self.accumulator > 0. {
            // Fractional-step projection is render-only even inside a forecast copy.
            self.projected.copy_from_slice(&self.nodes);
            std::mem::swap(&mut self.nodes, &mut self.projected);
            self.integrate(self.accumulator, config);
            self.pin_anchor();
            self.constrain_offsets(area.size);
            let mesh = self.mesh_vertices(area, true);
            std::mem::swap(&mut self.nodes, &mut self.projected);
            Some(mesh)
        } else {
            Some(self.mesh_vertices(area, render_changed))
        }
    }

    /// Advance only from an actual owner-thread pose/time, before any output
    /// presentation clock projection. No texture vertices are allocated here.
    pub fn advance(&mut self, area: Rectangle<f64, Logical>, config: WindowWobble, now: Duration) {
        let (commit, texture) = self.source_state.unwrap_or_default();
        if self.advance_simulation(area, config, now, commit, texture) == Some(true) {
            self.cached_mesh = None;
        }
    }

    fn copy_simulation_from(&mut self, owner: &Self) {
        self.id.clone_from(&owner.id);
        self.commit = owner.commit;
        self.nodes.clone_from(&owner.nodes);
        self.scratch.clone_from(&owner.scratch);
        self.projected.clone_from(&owner.projected);
        self.points.clear();
        self.cached_mesh = None;
        self.mesh = owner.mesh;
        self.anchor = owner.anchor;
        self.active = owner.active;
        self.dynamic = owner.dynamic;
        self.last_area = owner.last_area;
        self.pending_displacement = owner.pending_displacement;
        self.last_update = owner.last_update;
        self.accumulator = owner.accumulator;
        self.settle_elapsed = owner.settle_elapsed;
        self.source_state = owner.source_state;
    }

    fn advance_simulation(
        &mut self,
        area: Rectangle<f64, Logical>,
        config: WindowWobble,
        now: Duration,
        source_commit: CommitCounter,
        source_texture: u64,
    ) -> Option<bool> {
        if !self.active {
            return None;
        }
        if config.off || area.size.w <= 0. || area.size.h <= 0. {
            self.cancel();
            return None;
        }

        self.ensure_mesh(config.mesh as usize);
        let geometry_changed = self.last_area != Some(area);

        let source_state = (source_commit, source_texture);
        let mut changed = self.source_state != Some(source_state);
        self.source_state = Some(source_state);
        if self.last_area.is_none() {
            let displacement =
                std::mem::replace(&mut self.pending_displacement, Point::from((0., 0.)));
            if displacement.x != 0. || displacement.y != 0. {
                for node in &mut self.nodes {
                    node.offset[0] -= displacement.x * config.intensity;
                    node.offset[1] -= displacement.y * config.intensity;
                }
                self.constrain_offsets(area.size);
                self.dynamic = true;
                changed = true;
            }
        }
        if let Some(previous) = self.last_area {
            let location_delta = area.loc - previous.loc;
            let size_delta = area.size.to_point() - previous.size.to_point();
            if location_delta.x != 0.
                || location_delta.y != 0.
                || size_delta.x != 0.
                || size_delta.y != 0.
            {
                if is_coordinate_discontinuity(previous, area, location_delta) {
                    // Interactive-move positions are output-local. Crossing an output can
                    // therefore look like a very large one-frame move even though the pointer is
                    // continuous in global space. Treat that as a coordinate-system rebase rather
                    // than injecting enough energy to fold the mesh over itself.
                    self.nodes.fill(Node::default());
                    self.scratch.fill(Node::default());
                    self.accumulator = 0.;
                } else {
                    let denom = (self.mesh - 1) as f64;
                    for y in 0..self.mesh {
                        for x in 0..self.mesh {
                            let idx = y * self.mesh + x;
                            let u = x as f64 / denom;
                            let v = y as f64 / denom;
                            self.nodes[idx].offset[0] -=
                                (location_delta.x + size_delta.x * u) * config.intensity;
                            self.nodes[idx].offset[1] -=
                                (location_delta.y + size_delta.y * v) * config.intensity;
                        }
                    }
                    self.constrain_offsets(area.size);
                }
                self.dynamic = true;
                changed = true;
            }
        }
        self.last_area = Some(area);

        if geometry_changed {
            self.pin_anchor();
            self.constrain_offsets(area.size);
        }

        let elapsed = self
            .last_update
            .map_or(0., |last| now.saturating_sub(last).as_secs_f64());
        // Different outputs can predict different presentation times. Do not count the
        // same interval twice after an older prediction from another output.
        self.last_update = Some(self.last_update.map_or(now, |last| last.max(now)));
        if self.dynamic {
            self.accumulator = (self.accumulator + elapsed).min(MAX_ELAPSED);
        } else {
            // No spring energy is present. Idle time must not be caught up when the next
            // pointer event starts a new deformation.
            self.accumulator = 0.;
        }
        if self.anchor.is_none() {
            self.settle_elapsed += elapsed;
            if self.settle_elapsed >= MAX_SETTLE_TIME {
                self.cancel();
                return None;
            }
        } else {
            self.settle_elapsed = 0.;
        }

        let mut stepped = false;
        while self.accumulator + 1e-12 >= STEP {
            self.integrate(STEP, config);
            // Pin every physics step, so batching two steps at 60 Hz has the same
            // boundary conditions as one step per frame at 120 Hz.
            self.pin_anchor();
            self.constrain_offsets(area.size);
            self.accumulator = (self.accumulator - STEP).max(0.);
            stepped = true;
        }
        if geometry_changed || stepped {
            self.pin_anchor();
            self.constrain_offsets(area.size);
        }

        let stable = self.is_stable(config);
        self.dynamic = !stable;
        if stable && self.anchor.is_none() {
            self.cancel();
            return None;
        }
        if stable {
            for node in &mut self.nodes {
                node.velocity = [0., 0.];
            }
        }
        let render_changed = geometry_changed || stepped || (self.dynamic && elapsed > 0.);
        if changed || render_changed {
            self.commit.increment();
        }

        Some(render_changed)
    }

    fn ensure_mesh(&mut self, mesh: usize) {
        let mesh = mesh.clamp(2, 32);
        if self.mesh == mesh && self.nodes.len() == mesh * mesh {
            return;
        }
        self.mesh = mesh;
        self.nodes = vec![Node::default(); mesh * mesh];
        self.scratch = vec![Node::default(); mesh * mesh];
        self.projected = vec![Node::default(); mesh * mesh];
        self.points = Vec::with_capacity(mesh * mesh);
        self.cached_mesh = None;
        self.last_area = None;
        self.pending_displacement = Point::from((0., 0.));
        self.accumulator = 0.;
        self.settle_elapsed = 0.;
        self.source_state = None;
        self.commit.increment();
    }

    fn pin_anchor(&mut self) {
        let Some(anchor) = self.anchor else {
            return;
        };
        match anchor {
            WobbleAnchor::Point((u, v)) => {
                let denom = (self.mesh - 1) as f64;
                let x = (u.clamp(0., 1.) * denom).round() as usize;
                let y = (v.clamp(0., 1.) * denom).round() as usize;
                self.nodes[y * self.mesh + x] = Node::default();
            }
            WobbleAnchor::Edges {
                left,
                right,
                top,
                bottom,
            } => {
                if top {
                    self.nodes[..self.mesh].fill(Node::default());
                }
                if bottom {
                    let start = self.nodes.len() - self.mesh;
                    self.nodes[start..].fill(Node::default());
                }
                for row in self.nodes.chunks_exact_mut(self.mesh) {
                    if left {
                        row[0] = Node::default();
                    }
                    if right {
                        row[self.mesh - 1] = Node::default();
                    }
                }
            }
        }
    }

    fn integrate(&mut self, dt: f64, config: WindowWobble) {
        self.scratch.copy_from_slice(&self.nodes);
        let previous = &self.scratch;
        let nodes = &mut self.nodes;
        let coupling = config.stiffness * 0.18;
        let inv_mass = 1. / config.mass.max(0.0001);
        let damping = (-config.damping * inv_mass * dt).exp();

        for y in 0..self.mesh {
            for x in 0..self.mesh {
                let idx = y * self.mesh + x;
                let node = previous[idx];
                let mut neighbor_sum = [0., 0.];
                let mut neighbors = 0.;
                for (nx, ny) in [
                    (x.wrapping_sub(1), y),
                    (x + 1, y),
                    (x, y.wrapping_sub(1)),
                    (x, y + 1),
                ] {
                    if nx < self.mesh && ny < self.mesh {
                        let other = previous[ny * self.mesh + nx];
                        neighbor_sum[0] += other.offset[0];
                        neighbor_sum[1] += other.offset[1];
                        neighbors += 1.;
                    }
                }

                for (axis, neighbor_sum) in neighbor_sum.into_iter().enumerate() {
                    let neighbor_force = if neighbors > 0. {
                        coupling * (neighbor_sum / neighbors - node.offset[axis])
                    } else {
                        0.
                    };
                    let spring_force = -config.stiffness * node.offset[axis] + neighbor_force;
                    let velocity = (node.velocity[axis] + spring_force * inv_mass * dt) * damping;
                    nodes[idx].velocity[axis] = velocity;
                    nodes[idx].offset[axis] = node.offset[axis] + velocity * dt;
                }
            }
        }
    }

    /// Keep neighboring grid vertices ordered along both axes.
    ///
    /// Without this bound a fast pointer update can move a vertex beyond its neighbor. The two
    /// triangles in that cell then become inverted and sample an unrelated part of the window,
    /// which presents as flashing wedges or duplicated window fragments while dragging.
    fn constrain_offsets(&mut self, size: Size<f64, Logical>) {
        let denom = (self.mesh - 1) as f64;
        let max_x = (size.w / denom * 0.45).max(0.);
        let max_y = (size.h / denom * 0.45).max(0.);

        for node in &mut self.nodes {
            for (axis, limit) in [max_x, max_y].into_iter().enumerate() {
                let unconstrained = node.offset[axis];
                node.offset[axis] = unconstrained.clamp(-limit, limit);
                if node.offset[axis] != unconstrained
                    && node.velocity[axis].signum() == unconstrained.signum()
                {
                    node.velocity[axis] = 0.;
                }
            }
        }

        // Component bounds keep rows and columns ordered, but a sufficiently diagonal shear can
        // still invert one of the two triangles in a cell. Reduce only genuinely unsafe frames;
        // normal wobble keeps its full amplitude.
        for _ in 0..8 {
            if self.mesh_triangles_are_valid(size) {
                return;
            }
            for node in &mut self.nodes {
                for axis in 0..2 {
                    node.offset[axis] *= 0.75;
                    node.velocity[axis] *= 0.75;
                }
            }
        }

        // The undeformed grid is always valid. This is a last-resort guard for non-finite input
        // or an unexpectedly tiny surface rather than a path reached during normal interaction.
        if !self.mesh_triangles_are_valid(size) {
            self.nodes.fill(Node::default());
            self.scratch.fill(Node::default());
        }
    }

    fn mesh_triangles_are_valid(&self, size: Size<f64, Logical>) -> bool {
        if self.mesh < 2 || self.nodes.len() != self.mesh * self.mesh {
            return false;
        }

        let denom = (self.mesh - 1) as f64;
        let cell_w = size.w / denom;
        let cell_h = size.h / denom;
        if !cell_w.is_finite() || !cell_h.is_finite() || cell_w <= 0. || cell_h <= 0. {
            return false;
        }
        let minimum_area = cell_w * cell_h * 0.001;
        let point = |x: usize, y: usize| {
            let offset = self.nodes[y * self.mesh + x].offset;
            [x as f64 * cell_w + offset[0], y as f64 * cell_h + offset[1]]
        };
        let signed_area = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| {
            (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
        };

        for y in 0..self.mesh - 1 {
            for x in 0..self.mesh - 1 {
                let top_left = point(x, y);
                let top_right = point(x + 1, y);
                let bottom_left = point(x, y + 1);
                let bottom_right = point(x + 1, y + 1);
                // mesh_vertices() emits both triangles clockwise in logical (Y-down) space.
                if signed_area(top_left, bottom_left, bottom_right) >= -minimum_area
                    || signed_area(top_left, bottom_right, top_right) >= -minimum_area
                {
                    return false;
                }
            }
        }
        true
    }

    fn is_stable(&self, config: WindowWobble) -> bool {
        self.nodes.iter().all(|node| {
            node.offset[0].abs() <= config.position_epsilon
                && node.offset[1].abs() <= config.position_epsilon
                && node.velocity[0].abs() <= config.velocity_epsilon
                && node.velocity[1].abs() <= config.velocity_epsilon
        })
    }

    fn mesh_vertices(
        &mut self,
        source_area: Rectangle<f64, Logical>,
        geometry_changed: bool,
    ) -> WobbleMesh {
        if geometry_changed || self.cached_mesh.is_none() {
            self.rebuild_mesh_vertices(source_area);
        }
        let cached = self.cached_mesh.as_ref().unwrap();
        debug_assert_eq!(cached.source_area, source_area);
        WobbleMesh {
            id: self.id.clone(),
            commit: self.commit,
            area: cached.area,
            vertices: cached.vertices.clone(),
        }
    }

    fn rebuild_mesh_vertices(&mut self, source_area: Rectangle<f64, Logical>) {
        let denom = (self.mesh - 1) as f64;
        let points = &mut self.points;
        points.clear();
        let mut min = Point::from((f64::INFINITY, f64::INFINITY));
        let mut max = Point::from((f64::NEG_INFINITY, f64::NEG_INFINITY));

        for y in 0..self.mesh {
            for x in 0..self.mesh {
                let u = x as f64 / denom;
                let v = y as f64 / denom;
                let node = self.nodes[y * self.mesh + x];
                let point = Point::from((
                    source_area.loc.x + source_area.size.w * u + node.offset[0],
                    source_area.loc.y + source_area.size.h * v + node.offset[1],
                ));
                min.x = min.x.min(point.x);
                min.y = min.y.min(point.y);
                max.x = max.x.max(point.x);
                max.y = max.y.max(point.y);
                points.push((point, [u as f32, v as f32]));
            }
        }

        let area = Rectangle::from_extremities(min, max);
        let normalize = |point: Point<f64, Logical>| {
            [
                ((point.x - area.loc.x) / area.size.w.max(f64::EPSILON)) as f32,
                ((point.y - area.loc.y) / area.size.h.max(f64::EPSILON)) as f32,
            ]
        };
        let mut vertices = Vec::with_capacity((self.mesh - 1) * (self.mesh - 1) * 6);
        for y in 0..self.mesh - 1 {
            for x in 0..self.mesh - 1 {
                let tl = y * self.mesh + x;
                let tr = tl + 1;
                let bl = (y + 1) * self.mesh + x;
                let br = bl + 1;
                for idx in [tl, bl, br, tl, br, tr] {
                    let (point, uv) = points[idx];
                    vertices.push(MeshVertex {
                        position: normalize(point),
                        uv,
                    });
                }
            }
        }

        self.cached_mesh = Some(CachedMesh {
            source_area,
            area,
            vertices: vertices.into(),
        });
    }
}

fn is_coordinate_discontinuity(
    previous: Rectangle<f64, Logical>,
    current: Rectangle<f64, Logical>,
    delta: Point<f64, Logical>,
) -> bool {
    let width = previous.size.w.max(current.size.w).max(1.);
    let height = previous.size.h.max(current.size.h).max(1.);
    delta.x.abs() > width || delta.y.abs() > height
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::utils::Size;

    fn simulate(frame_ms: u64) -> WobbleState {
        let config = WindowWobble::default();
        let mut state = WobbleState::default();
        state.begin(WobbleAnchor::Point((0.5, 0.5)), config, Duration::ZERO);
        let mut now = Duration::ZERO;
        let mut area = Rectangle::new(Point::from((0., 0.)), Size::from((800., 600.)));
        let source_commit = CommitCounter::default();
        let _ = state.update(area, config, now, source_commit, 1);
        area.loc.x = 160.;
        now += Duration::from_millis(frame_ms);
        let _ = state.update(area, config, now, source_commit, 1);
        state.finish();
        for _ in 0..400 {
            now += Duration::from_millis(frame_ms);
            if state.update(area, config, now, source_commit, 1).is_none() {
                break;
            }
        }
        state
    }

    #[test]
    fn spring_converges_after_release() {
        assert!(!simulate(8).is_active());
    }

    #[test]
    fn fixed_step_converges_at_different_frame_rates() {
        assert!(!simulate(7).is_active());
        assert!(!simulate(16).is_active());
        assert!(!simulate(33).is_active());
    }

    fn moving_state() -> (WobbleState, Rectangle<f64, Logical>, WindowWobble) {
        let config = WindowWobble::default();
        let mut state = WobbleState::default();
        state.begin(WobbleAnchor::Point((0.5, 0.5)), config, Duration::ZERO);
        state.add_pending_displacement((40., -20.).into());
        let area = Rectangle::new((40., 0.).into(), (800., 600.).into());
        state.update(area, config, Duration::ZERO, CommitCounter::default(), 1);
        (state, area, config)
    }

    fn rendered_points(mesh: &WobbleMesh) -> Vec<[f64; 2]> {
        mesh.vertices
            .iter()
            .map(|vertex| {
                [
                    mesh.area.loc.x + f64::from(vertex.position[0]) * mesh.area.size.w,
                    mesh.area.loc.y + f64::from(vertex.position[1]) * mesh.area.size.h,
                ]
            })
            .collect()
    }

    #[test]
    fn mixed_output_forecasts_are_order_independent_and_do_not_advance_owner() {
        let (mut owner, area, config) = moving_state();
        owner.advance(area, config, Duration::from_millis(20));
        let nodes = owner.nodes.clone();
        let last_update = owner.last_update;
        let accumulator = owner.accumulator;
        let settle_elapsed = owner.settle_elapsed;
        let owner_commit = owner.commit;
        let short = Duration::from_millis(20) + Duration::from_secs_f64(1. / 120.);
        let long = Duration::from_millis(20) + Duration::from_secs_f64(1. / 60.);
        let mut forward = WobbleForecast::default();
        let mut reverse = WobbleForecast::default();
        let first_short = forward
            .sample(&owner, area, config, short, CommitCounter::default(), 1)
            .unwrap();
        let first_long = forward
            .sample(&owner, area, config, long, CommitCounter::default(), 1)
            .unwrap();
        let second_long = reverse
            .sample(&owner, area, config, long, CommitCounter::default(), 1)
            .unwrap();
        let second_short = reverse
            .sample(&owner, area, config, short, CommitCounter::default(), 1)
            .unwrap();
        assert_eq!(
            rendered_points(&first_short),
            rendered_points(&second_short)
        );
        assert_eq!(rendered_points(&first_long), rendered_points(&second_long));
        assert_ne!(rendered_points(&first_short), rendered_points(&first_long));
        assert_eq!(owner.nodes, nodes);
        assert_eq!(owner.last_update, last_update);
        assert_eq!(owner.accumulator, accumulator);
        assert_eq!(owner.settle_elapsed, settle_elapsed);
        assert_eq!(owner.commit, owner_commit);
        assert_ne!(first_short.commit, first_long.commit);
        let repeated = reverse
            .sample(&owner, area, config, short, CommitCounter::default(), 1)
            .unwrap();
        assert_eq!(second_short.commit, repeated.commit);
        assert!(Rc::ptr_eq(&second_short.vertices, &repeated.vertices));
        // The same owner event history must remain unchanged after any render forecasts.
        let (mut reference, _, _) = moving_state();
        reference.advance(area, config, Duration::from_millis(20));
        reference.advance(area, config, Duration::from_millis(40));
        owner.advance(area, config, Duration::from_millis(40));
        assert_eq!(owner.nodes, reference.nodes);
        assert_eq!(owner.accumulator, reference.accumulator);
    }

    #[test]
    fn forecast_geometry_rebase_and_expiry_do_not_change_release_state() {
        let (mut owner, area, config) = moving_state();
        owner.finish();
        owner.advance(area, config, Duration::from_millis(10));
        let nodes = owner.nodes.clone();
        let mut forecast = WobbleForecast::default();
        let rebased = Rectangle::new((-1200., 700.).into(), area.size);
        let now = Duration::from_millis(18);
        let first = forecast
            .sample(&owner, area, config, now, CommitCounter::default(), 1)
            .unwrap();
        let second = forecast
            .sample(&owner, rebased, config, now, CommitCounter::default(), 1)
            .unwrap();
        for (point, moved) in rendered_points(&first).iter().zip(rendered_points(&second)) {
            assert!((moved[0] - point[0] - (rebased.loc.x - area.loc.x)).abs() < 1e-9);
            assert!((moved[1] - point[1] - (rebased.loc.y - area.loc.y)).abs() < 1e-9);
        }
        assert!(forecast
            .sample(
                &owner,
                rebased,
                config,
                Duration::from_secs(10),
                CommitCounter::default(),
                1
            )
            .is_none());
        assert!(owner.is_ongoing());
        assert_eq!(owner.nodes, nodes);
        assert_eq!(owner.last_area, Some(area));
        assert_eq!(owner.settle_elapsed, 0.01);
        // An earlier output is still able to render after the later forecast settled.
        assert!(forecast
            .sample(&owner, area, config, now, CommitCounter::default(), 1)
            .is_some());
        owner.advance(area, config, Duration::from_secs(10));
        assert!(!owner.is_active());
    }

    #[test]
    fn high_refresh_frames_move_between_fixed_physics_steps() {
        for rate in [144., 240., 360.] {
            let (mut state, area, config) = moving_state();
            let mut previous = state
                .update(area, config, Duration::ZERO, CommitCounter::default(), 1)
                .unwrap();
            for frame in 1..=24 {
                let now = Duration::from_secs_f64(f64::from(frame) / rate);
                let current = state
                    .update(area, config, now, CommitCounter::default(), 1)
                    .unwrap();
                assert_ne!(
                    rendered_points(&previous),
                    rendered_points(&current),
                    "repeated frame {frame} at {rate} Hz"
                );
                assert_ne!(previous.commit, current.commit);
                // Capturing the same presentation must reuse geometry and never advance physics twice.
                let nodes = state.nodes.clone();
                let repeated = state
                    .update(area, config, now, CommitCounter::default(), 1)
                    .unwrap();
                assert_eq!(state.nodes, nodes);
                assert_eq!(current.commit, repeated.commit);
                assert!(Rc::ptr_eq(&current.vertices, &repeated.vertices));
                previous = current;
            }
        }
    }

    #[test]
    fn presentation_projection_preserves_fixed_step_physics() {
        // Use 60 Hz as the reference (large pauses intentionally cap physics catch-up).
        let mut states = Vec::new();
        for rate in [60, 120, 144, 240, 360] {
            let (mut state, area, config) = moving_state();
            for frame in 1..=rate / 10 {
                state.update(
                    area,
                    config,
                    Duration::from_secs_f64(f64::from(frame) / f64::from(rate)),
                    CommitCounter::default(),
                    1,
                );
            }
            state.update(
                area,
                config,
                Duration::from_millis(100),
                CommitCounter::default(),
                1,
            );
            states.push(state);
        }
        for state in &states[1..] {
            for (actual, expected) in state.nodes.iter().zip(&states[0].nodes) {
                for axis in 0..2 {
                    assert!((actual.offset[axis] - expected.offset[axis]).abs() < 1e-8);
                    assert!((actual.velocity[axis] - expected.velocity[axis]).abs() < 1e-8);
                }
            }
        }
    }

    #[test]
    fn long_pause_does_not_replay_an_expired_release_animation() {
        let (mut state, area, config) = moving_state();
        state.finish();
        assert!(state
            .update(
                area,
                config,
                Duration::from_secs(10),
                CommitCounter::default(),
                1
            )
            .is_none());
        assert!(!state.is_active());
    }

    #[test]
    fn older_presentation_does_not_integrate_the_same_time_twice() {
        let (mut first, area, config) = moving_state();
        let (mut second, _, _) = moving_state();
        first.update(
            area,
            config,
            Duration::from_millis(12),
            CommitCounter::default(),
            1,
        );
        first.update(
            area,
            config,
            Duration::from_millis(4),
            CommitCounter::default(),
            1,
        );
        first.update(
            area,
            config,
            Duration::from_millis(20),
            CommitCounter::default(),
            1,
        );
        second.update(
            area,
            config,
            Duration::from_millis(20),
            CommitCounter::default(),
            1,
        );
        assert_eq!(first.nodes, second.nodes);
        assert!((first.accumulator - second.accumulator).abs() < 1e-12);
    }

    #[test]
    fn point_and_edge_anchors_pin_only_the_selected_nodes() {
        let mut state = WobbleState::default();
        state.ensure_mesh(8);
        let moving = Node {
            offset: [4., -3.],
            velocity: [2., 1.],
        };

        state.nodes.fill(moving);
        state.anchor = Some(WobbleAnchor::Point((0.4, 0.7)));
        state.pin_anchor();
        for y in 0..state.mesh {
            for x in 0..state.mesh {
                let expected = if (x, y) == (3, 5) {
                    Node::default()
                } else {
                    moving
                };
                assert_eq!(state.nodes[y * state.mesh + x], expected);
            }
        }

        state.nodes.fill(moving);
        state.anchor = Some(WobbleAnchor::Edges {
            left: true,
            right: true,
            top: false,
            bottom: true,
        });
        state.pin_anchor();
        for y in 0..state.mesh {
            for x in 0..state.mesh {
                let expected = if x == 0 || x + 1 == state.mesh || y + 1 == state.mesh {
                    Node::default()
                } else {
                    moving
                };
                assert_eq!(state.nodes[y * state.mesh + x], expected);
            }
        }
    }

    #[test]
    fn dense_mesh_stays_valid_and_settles_with_uneven_frames() {
        let config = WindowWobble {
            mesh: 32,
            ..WindowWobble::default()
        };
        let mut state = WobbleState::default();
        state.begin(WobbleAnchor::Point((0.5, 0.5)), config, Duration::ZERO);
        let points_storage = state.points.as_ptr();
        let mut area = Rectangle::new(Point::from((0., 0.)), Size::from((800., 600.)));
        let source_commit = CommitCounter::default();
        state.update(area, config, Duration::ZERO, source_commit, 1);
        area.loc.x = 160.;
        let mut now = Duration::from_millis(16);
        state.update(area, config, now, source_commit, 1);
        state.finish();

        for frame in 0..400 {
            now += Duration::from_millis(if frame % 2 == 0 { 8 } else { 33 });
            if state.update(area, config, now, source_commit, 1).is_none() {
                break;
            }
            assert_eq!(state.points.as_ptr(), points_storage);
            assert!(state.mesh_triangles_are_valid(area.size));
        }
        assert!(!state.is_active());
    }

    #[test]
    fn source_damage_updates_mesh_commit_while_geometry_is_stable() {
        let config = WindowWobble::default();
        let mut state = WobbleState::default();
        state.begin(WobbleAnchor::Point((0.5, 0.5)), config, Duration::ZERO);
        let area = Rectangle::new(Point::from((0., 0.)), Size::from((800., 600.)));
        let mut source_commit = CommitCounter::default();
        let first = state
            .update(area, config, Duration::ZERO, source_commit, 1)
            .unwrap();

        source_commit.increment();
        let second = state
            .update(area, config, Duration::from_millis(16), source_commit, 1)
            .unwrap();

        let third = state
            .update(area, config, Duration::from_millis(32), source_commit, 2)
            .unwrap();

        assert_ne!(first.commit, second.commit);
        assert_ne!(second.commit, third.commit);
        assert!(Rc::ptr_eq(&first.vertices, &second.vertices));
        assert!(Rc::ptr_eq(&second.vertices, &third.vertices));
        assert!(!state.is_ongoing());
    }

    #[test]
    fn motion_mesh_change_and_cancel_invalidate_cached_vertices() {
        let config = WindowWobble::default();
        let mut state = WobbleState::default();
        state.begin(WobbleAnchor::Point((0.5, 0.5)), config, Duration::ZERO);
        let mut area = Rectangle::new(Point::from((0., 0.)), Size::from((800., 600.)));
        let source_commit = CommitCounter::default();
        let first = state
            .update(area, config, Duration::ZERO, source_commit, 1)
            .unwrap();

        area.loc.x += 40.;
        let moved = state
            .update(area, config, Duration::from_millis(16), source_commit, 1)
            .unwrap();
        assert!(!Rc::ptr_eq(&first.vertices, &moved.vertices));

        let larger_mesh = WindowWobble { mesh: 16, ..config };
        let resized = state
            .update(
                area,
                larger_mesh,
                Duration::from_millis(32),
                source_commit,
                1,
            )
            .unwrap();
        assert_eq!(resized.vertices.len(), 6 * 15 * 15);
        assert!(!Rc::ptr_eq(&moved.vertices, &resized.vertices));

        state.cancel();
        state.begin(
            WobbleAnchor::Point((0.5, 0.5)),
            larger_mesh,
            Duration::from_millis(48),
        );
        let restarted = state
            .update(
                area,
                larger_mesh,
                Duration::from_millis(48),
                source_commit,
                1,
            )
            .unwrap();
        assert!(!Rc::ptr_eq(&resized.vertices, &restarted.vertices));
    }

    /// Run manually with `cargo test --release -p realand --lib
    /// layout::wobble::tests::stable_mesh_cache_microbenchmark -- --ignored --nocapture`.
    #[test]
    #[ignore = "manual CPU microbenchmark"]
    fn stable_mesh_cache_microbenchmark() {
        use std::hint::black_box;
        use std::time::Instant;

        for (mesh, frames) in [(12, 20_000), (32, 2_000)] {
            let config = WindowWobble {
                mesh,
                ..WindowWobble::default()
            };
            let mut state = WobbleState::default();
            state.begin(WobbleAnchor::Point((0.5, 0.5)), config, Duration::ZERO);
            let area = Rectangle::new(Point::from((0., 0.)), Size::from((800., 600.)));
            state.update(area, config, Duration::ZERO, CommitCounter::default(), 1);

            for repeat in 0..5 {
                let start = Instant::now();
                for _ in 0..frames {
                    state.rebuild_mesh_vertices(black_box(area));
                    let vertices = &state.cached_mesh.as_ref().unwrap().vertices;
                    black_box(&**vertices);
                }
                let rebuild = start.elapsed();

                let start = Instant::now();
                for _ in 0..frames {
                    let cached = state.mesh_vertices(black_box(area), false);
                    black_box(&*cached.vertices);
                }
                let cached = start.elapsed();
                eprintln!(
                    "mesh={mesh} repeat={repeat} frames={frames} rebuild={rebuild:?} cached={cached:?}"
                );
            }
        }
    }

    /// Synthetic CPU cost only; this excludes GPU rendering, input delivery and KMS.
    #[test]
    #[ignore = "manual CPU microbenchmark"]
    fn dynamic_mesh_frame_cost_microbenchmark() {
        use std::hint::black_box;
        use std::time::Instant;
        for mesh in [12, 32] {
            for rate in [60, 144, 240] {
                let config = WindowWobble {
                    mesh,
                    ..WindowWobble::default()
                };
                let mut state = WobbleState::default();
                state.begin(WobbleAnchor::Point((0.5, 0.5)), config, Duration::ZERO);
                let mut costs = Vec::with_capacity(1000);
                for frame in 0..1120 {
                    let seconds = f64::from(frame) / f64::from(rate);
                    let area = Rectangle::new(
                        (200. * seconds.sin(), 100. * (seconds * 0.7).cos()).into(),
                        (800., 600.).into(),
                    );
                    let started = Instant::now();
                    black_box(state.update(
                        black_box(area),
                        config,
                        Duration::from_secs_f64(seconds),
                        CommitCounter::default(),
                        1,
                    ));
                    if frame >= 120 {
                        costs.push(started.elapsed().as_secs_f64() * 1e6);
                    }
                }
                costs.sort_unstable_by(f64::total_cmp);
                eprintln!("wobble mesh={mesh} hz={rate} cpu_us_p50={:.2} cpu_us_p95={:.2} cpu_us_max={:.2}", costs[499], costs[949], costs[999]);
            }
        }
    }

    #[test]
    fn motion_before_first_render_deforms_the_first_frame() {
        let config = WindowWobble::default();
        let mut state = WobbleState::default();
        state.begin(WobbleAnchor::Point((0.5, 0.5)), config, Duration::ZERO);
        state.add_pending_displacement(Point::from((60., -30.)));
        state.add_pending_displacement(Point::from((40., 10.)));

        let area = Rectangle::new(Point::from((100., 50.)), Size::from((800., 600.)));
        let mesh = state
            .update(area, config, Duration::ZERO, CommitCounter::default(), 1)
            .unwrap();

        assert!(state.is_ongoing());
        assert!(state.nodes.iter().any(|node| *node != Node::default()));
        assert!(mesh.vertices.iter().any(|vertex| {
            let expected_x = area.loc.x + area.size.w * f64::from(vertex.uv[0]);
            let actual_x = mesh.area.loc.x + mesh.area.size.w * f64::from(vertex.position[0]);
            (actual_x - expected_x).abs() > 0.1
        }));
    }

    #[test]
    fn undamped_spring_stops_at_safety_limit() {
        let config = WindowWobble {
            damping: 0.,
            position_epsilon: 0.,
            velocity_epsilon: 0.,
            ..WindowWobble::default()
        };
        let mut state = WobbleState::default();
        state.begin(WobbleAnchor::Point((0.5, 0.5)), config, Duration::ZERO);
        let source_commit = CommitCounter::default();
        let mut area = Rectangle::new(Point::from((0., 0.)), Size::from((800., 600.)));
        let _ = state.update(area, config, Duration::ZERO, source_commit, 1);
        area.loc.x = 160.;
        let mut now = Duration::from_millis(16);
        let _ = state.update(area, config, now, source_commit, 1);
        state.finish();

        for _ in 0..400 {
            now += Duration::from_millis(16);
            if state.update(area, config, now, source_commit, 1).is_none() {
                break;
            }
        }

        assert!(!state.is_active());
    }

    #[test]
    fn fast_drag_cannot_fold_mesh_cells() {
        let config = WindowWobble::default();
        let mut state = WobbleState::default();
        state.begin(WobbleAnchor::Point((0.5, 0.5)), config, Duration::ZERO);
        let source_commit = CommitCounter::default();
        let mut area = Rectangle::new(Point::from((0., 0.)), Size::from((800., 600.)));
        let _ = state.update(area, config, Duration::ZERO, source_commit, 1);

        area.loc.x = 700.;
        area.loc.y = 500.;
        let _ = state.update(area, config, Duration::from_millis(16), source_commit, 1);

        let cell_w = area.size.w / (state.mesh - 1) as f64;
        let cell_h = area.size.h / (state.mesh - 1) as f64;
        for y in 0..state.mesh {
            for x in 0..state.mesh - 1 {
                let left = x as f64 * cell_w + state.nodes[y * state.mesh + x].offset[0];
                let right = (x + 1) as f64 * cell_w + state.nodes[y * state.mesh + x + 1].offset[0];
                assert!(left < right);
            }
        }
        for y in 0..state.mesh - 1 {
            for x in 0..state.mesh {
                let top = y as f64 * cell_h + state.nodes[y * state.mesh + x].offset[1];
                let bottom =
                    (y + 1) as f64 * cell_h + state.nodes[(y + 1) * state.mesh + x].offset[1];
                assert!(top < bottom);
            }
        }
        assert!(state.mesh_triangles_are_valid(area.size));
    }

    #[test]
    fn diagonal_shear_is_reduced_before_rendering() {
        let config = WindowWobble::default();
        let mut state = WobbleState::default();
        state.ensure_mesh(config.mesh as usize);
        let area = Rectangle::new(Point::from((0., 0.)), Size::from((800., 600.)));
        let cell_w = area.size.w / (state.mesh - 1) as f64;
        let cell_h = area.size.h / (state.mesh - 1) as f64;

        // Twist one cell while keeping each component within the ordinary 45% clamp.
        let top_left = 0;
        let top_right = 1;
        let bottom_left = state.mesh;
        let bottom_right = state.mesh + 1;
        state.nodes[top_left].offset = [-cell_w * 0.45, cell_h * 0.45];
        state.nodes[top_right].offset = [cell_w * 0.45, -cell_h * 0.45];
        state.nodes[bottom_left].offset = [cell_w * 0.45, -cell_h * 0.45];
        state.nodes[bottom_right].offset = [-cell_w * 0.45, cell_h * 0.45];
        assert!(!state.mesh_triangles_are_valid(area.size));

        state.constrain_offsets(area.size);
        assert!(state.mesh_triangles_are_valid(area.size));
    }

    #[test]
    fn output_local_coordinate_jump_rebases_without_deformation() {
        let config = WindowWobble::default();
        let mut state = WobbleState::default();
        state.begin(WobbleAnchor::Point((0.5, 0.5)), config, Duration::ZERO);
        let source_commit = CommitCounter::default();
        let mut area = Rectangle::new(Point::from((0., 0.)), Size::from((400., 300.)));
        let _ = state.update(area, config, Duration::ZERO, source_commit, 1);

        area.loc = Point::from((1200., 900.));
        let _ = state.update(area, config, Duration::from_millis(16), source_commit, 1);

        assert!(state.nodes.iter().all(|node| *node == Node::default()));
    }
}
