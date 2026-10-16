//! Real mapped-window minimization and the shared GLES/Vulkan Genie mesh.
//!
//! A minimized Tile remains owned by its workspace, so commits, output changes,
//! toplevel destruction and IPC keep their normal lifetime. It is detached from
//! the visible layout and input until activation restores it. The animation only
//! deforms its presentation; it never unmaps a client's wl_surface.

use std::rc::Rc;

use clarion_ipc::{DockEdge, MinimizeEffect};

use smithay::backend::renderer::element::Id;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::utils::{Logical, Point, Rectangle, Size};

use crate::animation::{Animation, Clock, Curve};
use crate::render_helpers::mesh::MeshVertex;

fn duration_ms(effect: MinimizeEffect) -> u64 {
    // Reference-profile timings from the project design, not recovered
    // WindowServer defaults (REA evidence shows Catalina SkyLight
    // `_SLSCreateGenieWindowAnimation` at 0x4275e is an 11-byte return stub returning 0x3ee).
    match effect {
        MinimizeEffect::Genie => 450,
        MinimizeEffect::Scale => 300,
    }
}
#[cfg(test)]
use clarion_core::deformation::{COLUMNS, ROWS};

#[derive(Debug)]
pub struct Genie {
    animation: Animation,
    source: Rectangle<f64, Logical>,
    target: Rectangle<f64, Logical>,
    minimizing: bool,
    edge: DockEdge,
    effect: MinimizeEffect,
    destination: Option<(Rectangle<f64, Logical>, f64)>,
    id: Id,
    commit: CommitCounter,
    last_frame: Option<GenieFrameKey>,
    cached: Option<GenieMesh>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct GenieFrameKey {
    progress: f64,
    texture_area: Rectangle<f64, Logical>,
    tile_size: Size<f64, Logical>,
    source_commit: CommitCounter,
    source_texture: u64,
}

#[derive(Debug, Clone)]
pub struct GenieMesh {
    pub id: Id,
    pub commit: CommitCounter,
    pub area: Rectangle<f64, Logical>,
    pub vertices: Rc<[MeshVertex]>,
    pub alpha: f32,
}

impl Genie {
    pub fn new(
        clock: Clock,
        source: Rectangle<f64, Logical>,
        target: Rectangle<f64, Logical>,
        minimizing: bool,
    ) -> Self {
        Self::with_style(
            clock,
            source,
            target,
            minimizing,
            DockEdge::Bottom,
            MinimizeEffect::Genie,
        )
    }

    pub fn with_style(
        clock: Clock,
        source: Rectangle<f64, Logical>,
        target: Rectangle<f64, Logical>,
        minimizing: bool,
        edge: DockEdge,
        effect: MinimizeEffect,
    ) -> Self {
        let (from, to) = if minimizing { (0., 1.) } else { (1., 0.) };
        Self {
            animation: Animation::ease(clock, from, to, 0., duration_ms(effect), Curve::Linear),
            source,
            target,
            minimizing,
            edge,
            effect,
            destination: None,
            id: Id::new(),
            commit: CommitCounter::default(),
            last_frame: None,
            cached: None,
        }
    }

    /// Reverse from the exact visible progress. No endpoint restart or frame jump.
    pub fn reverse(&mut self, clock: Clock, minimizing: bool) {
        let from = self.progress();
        self.source = self.source_at(from);
        self.destination = None;
        let to = if minimizing { 1. } else { 0. };
        let duration = (duration_ms(self.effect) as f64 * (to - from).abs()).round() as u64;
        let velocity = if self.animation.is_done() {
            0.
        } else {
            self.animation.velocity()
        };
        self.animation = Animation::continuous_ease(clock, from, to, velocity, duration);
        self.minimizing = minimizing;
        self.last_frame = None;
    }

    pub fn progress(&self) -> f64 {
        self.animation.value().clamp(0., 1.)
    }
    pub fn is_done(&self) -> bool {
        self.animation.is_done()
    }
    fn source_at(&self, p: f64) -> Rectangle<f64, Logical> {
        let Some((destination, from)) = self.destination else {
            return self.source;
        };
        let t = if from <= f64::EPSILON {
            1.
        } else {
            smoothstep(1. - p / from)
        };
        Rectangle::new(
            self.source.loc + (destination.loc - self.source.loc).upscale(t),
            (
                self.source.size.w + (destination.size.w - self.source.size.w) * t,
                self.source.size.h + (destination.size.h - self.source.size.h) * t,
            )
                .into(),
        )
    }

    /// Commit real layout reflow at event time while handing the old presentation
    /// rectangle to its new destination. The owner pose tick calls this before
    /// output presentation projections; render sampling must never retarget.
    pub fn retarget_destination(&mut self, destination: Rectangle<f64, Logical>) {
        if self.minimizing
            || self.destination.is_some_and(|(old, _)| old == destination)
            || (self.destination.is_none() && self.source == destination)
        {
            return;
        }
        let p = self.progress();
        self.source = self.source_at(p);
        self.destination = Some((destination, p));
        self.last_frame = None;
    }

    /// Sample an output's presentation time. Only resource-cache identity and
    /// damage commits change here; source, destination and trajectory remain
    /// event-owned even when a later output is sampled before an earlier one.
    pub fn mesh(
        &mut self,
        texture_area: Rectangle<f64, Logical>,
        tile_size: Size<f64, Logical>,
        source_commit: CommitCounter,
        source_texture: u64,
    ) -> GenieMesh {
        let p = self.progress();
        let frame = GenieFrameKey {
            progress: p,
            texture_area,
            tile_size,
            source_commit,
            source_texture,
        };
        if self.last_frame == Some(frame) {
            return self.cached.clone().unwrap();
        }
        self.last_frame = Some(frame);
        self.commit.increment();
        let (area, vertices) = self.sample_geometry(p, texture_area, tile_size);
        let mesh = GenieMesh {
            id: self.id.clone(),
            commit: self.commit,
            area,
            vertices: vertices.into(),
            // The final small rectangle disappears into its icon without a flash.
            alpha: (1. - smoothstep((p - 0.90) / 0.10)) as f32,
        };
        self.cached = Some(mesh.clone());
        mesh
    }

    fn sample_geometry(
        &self,
        progress: f64,
        texture_area: Rectangle<f64, Logical>,
        tile_size: Size<f64, Logical>,
    ) -> (Rectangle<f64, Logical>, Vec<MeshVertex>) {
        // Include CSD shadows at both ends. All geometry comes from the
        // immutable owner pose and this sample, never a previous output sample.
        let base = self.source_at(progress);
        let sx = base.size.w / tile_size.w.max(1.);
        let sy = base.size.h / tile_size.h.max(1.);
        let source = Rectangle::new(
            base.loc + Point::from((texture_area.loc.x * sx, texture_area.loc.y * sy)),
            (texture_area.size.w * sx, texture_area.size.h * sy).into(),
        );
        minimize_vertices(source, self.target, progress, self.edge, self.effect)
    }
}

use clarion_core::deformation::smoothstep;

/// Dock-facing rows finish narrowing first; the remaining body follows.
/// Keep the trajectory continuous through the phase boundary. These splines
/// are our implementation, rather than a recovered SkyLight formula.
pub fn minimize_vertices(
    source: Rectangle<f64, Logical>,
    target: Rectangle<f64, Logical>,
    progress: f64,
    edge: DockEdge,
    effect: MinimizeEffect,
) -> (Rectangle<f64, Logical>, Vec<MeshVertex>) {
    use clarion_core::deformation::{Edge, Effect};
    let convert = |rect: Rectangle<f64, Logical>| {
        clarion_core::Rect::new(rect.loc.x, rect.loc.y, rect.size.w, rect.size.h)
    };
    let edge = match edge {
        DockEdge::Top => Edge::Top,
        DockEdge::Bottom => Edge::Bottom,
        DockEdge::Left => Edge::Left,
        DockEdge::Right => Edge::Right,
    };
    let effect = match effect {
        MinimizeEffect::Genie => Effect::Genie,
        MinimizeEffect::Scale => Effect::Scale,
    };
    let (area, vertices) = clarion_core::deformation::minimize_vertices(
        convert(source),
        convert(target),
        progress,
        edge,
        effect,
    );
    (
        Rectangle::new((area.x, area.y).into(), (area.width, area.height).into()),
        vertices,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn endpoints_bounds_and_nonfolding_are_exact_at_all_scales_and_edges() {
        for scale in [1., 1.25, 1.5, 2.] {
            let source = Rectangle::new(
                (80. / scale, 45. / scale).into(),
                (800. / scale, 600. / scale).into(),
            );
            for edge in [
                DockEdge::Bottom,
                DockEdge::Left,
                DockEdge::Right,
                DockEdge::Top,
            ] {
                let pos = match edge {
                    DockEdge::Bottom => (1040., 1000.),
                    DockEdge::Left => (0., 960.),
                    DockEdge::Right => (1040., 20.),
                    DockEdge::Top => (1040., 0.),
                };
                let target = Rectangle::new(
                    (pos.0 / scale, pos.1 / scale).into(),
                    (48. / scale, 48. / scale).into(),
                );
                for effect in [MinimizeEffect::Genie, MinimizeEffect::Scale] {
                    for p in [0., 0.05, 0.18, 0.25, 0.5, 0.75, 0.95, 1.] {
                        let (area, vertices) = minimize_vertices(source, target, p, edge, effect);
                        assert_eq!(
                            vertices.len(),
                            if effect == MinimizeEffect::Scale {
                                6
                            } else {
                                ROWS * COLUMNS * 6
                            }
                        );
                        assert!(area.size.w > 0. && area.size.h > 0.);
                        for v in &vertices {
                            assert!(v.position.iter().chain(v.uv.iter()).all(|x| x.is_finite()));
                            assert!(v.position[0] >= 0. && v.position[0] <= 1.001);
                            assert!(v.position[1] >= 0. && v.position[1] <= 1.001);
                        }
                        for tri in vertices.as_chunks::<3>().0 {
                            let a = tri[0].position;
                            let b = tri[1].position;
                            let c = tri[2].position;
                            let signed =
                                (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
                            assert!(
                                signed >= -0.001,
                                "folded mesh at {edge:?}/{effect:?}/{p}: {signed}"
                            );
                        }
                        if effect == MinimizeEffect::Scale {
                            approx::assert_abs_diff_eq!(
                                area.size.w / area.size.h,
                                source.size.w / source.size.h,
                                epsilon = 1e-9
                            );
                        }
                        if p == 0. {
                            approx::assert_abs_diff_eq!(area.loc.x, source.loc.x, epsilon = 1e-9);
                            approx::assert_abs_diff_eq!(area.loc.y, source.loc.y, epsilon = 1e-9);
                            approx::assert_abs_diff_eq!(area.size.w, source.size.w, epsilon = 1e-9);
                            approx::assert_abs_diff_eq!(area.size.h, source.size.h, epsilon = 1e-9);
                        }
                        if p == 1. {
                            if effect == MinimizeEffect::Genie {
                                approx::assert_abs_diff_eq!(
                                    area.loc.x,
                                    target.loc.x,
                                    epsilon = 1e-9
                                );
                                approx::assert_abs_diff_eq!(
                                    area.loc.y,
                                    target.loc.y,
                                    epsilon = 1e-9
                                );
                                approx::assert_abs_diff_eq!(
                                    area.size.w,
                                    target.size.w,
                                    epsilon = 1e-9
                                );
                                approx::assert_abs_diff_eq!(
                                    area.size.h,
                                    target.size.h,
                                    epsilon = 1e-9
                                );
                            } else {
                                assert!(
                                    area.loc.x + 1e-9 >= target.loc.x
                                        && area.loc.y + 1e-9 >= target.loc.y
                                );
                                assert!(
                                    area.loc.x + area.size.w <= target.loc.x + target.size.w + 1e-9
                                );
                                assert!(
                                    area.loc.y + area.size.h <= target.loc.y + target.size.h + 1e-9
                                );
                                approx::assert_abs_diff_eq!(
                                    area.loc.x + area.size.w / 2.,
                                    target.loc.x + target.size.w / 2.,
                                    epsilon = 1e-9
                                );
                                approx::assert_abs_diff_eq!(
                                    area.loc.y + area.size.h / 2.,
                                    target.loc.y + target.size.h / 2.,
                                    epsilon = 1e-9
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn neck_closes_from_the_explicit_dock_edge_without_rotating_uvs() {
        let source = Rectangle::new((80., 45.).into(), (800., 600.).into());
        // A strongly diagonal target cannot override the declared Dock edge.
        let target = Rectangle::new((1040., 1000.).into(), (48., 48.).into());
        for edge in [
            DockEdge::Bottom,
            DockEdge::Left,
            DockEdge::Right,
            DockEdge::Top,
        ] {
            let (area, vertices) =
                minimize_vertices(source, target, 0.25, edge, MinimizeEffect::Genie);
            let at = |uv| {
                let v = vertices.iter().find(|v| v.uv == uv).unwrap();
                [
                    area.loc.x + f64::from(v.position[0]) * area.size.w,
                    area.loc.y + f64::from(v.position[1]) * area.size.h,
                ]
            };
            let tl = at([0., 0.]);
            let tr = at([1., 0.]);
            let bl = at([0., 1.]);
            let br = at([1., 1.]);
            assert!(tl[0] < tr[0] && bl[0] < br[0]);
            assert!(tl[1] < bl[1] && tr[1] < br[1]);
            match edge {
                DockEdge::Bottom => assert!(br[0] - bl[0] < tr[0] - tl[0]),
                DockEdge::Top => assert!(tr[0] - tl[0] < br[0] - bl[0]),
                DockEdge::Left => assert!(bl[1] - tl[1] < br[1] - tr[1]),
                DockEdge::Right => assert!(br[1] - tr[1] < bl[1] - tl[1]),
            }
        }
    }

    #[test]
    fn genie_phase_boundary_has_continuous_position_and_velocity() {
        let source = Rectangle::new((80., 45.).into(), (800., 600.).into());
        let target = Rectangle::new((1040., 1000.).into(), (48., 48.).into());
        let h = 0.001;
        for edge in [
            DockEdge::Bottom,
            DockEdge::Left,
            DockEdge::Right,
            DockEdge::Top,
        ] {
            let world_positions = |p| {
                let (area, vertices) =
                    minimize_vertices(source, target, p, edge, MinimizeEffect::Genie);
                vertices
                    .into_iter()
                    .map(|v| {
                        [
                            area.loc.x + f64::from(v.position[0]) * area.size.w,
                            area.loc.y + f64::from(v.position[1]) * area.size.h,
                        ]
                    })
                    .collect::<Vec<_>>()
            };
            let before = world_positions(0.75 - h);
            let middle = world_positions(0.75);
            let after = world_positions(0.75 + h);
            for ((before, middle), after) in before.iter().zip(&middle).zip(&after) {
                for axis in 0..2 {
                    let incoming = (middle[axis] - before[axis]) / h;
                    let outgoing = (after[axis] - middle[axis]) / h;
                    assert!(
                        (incoming - outgoing).abs() < 30.,
                        "velocity jump at {edge:?}, axis {axis}: {incoming} -> {outgoing}"
                    );
                }
            }
            // Even the far edge must keep travelling at the phase transition.
            let axis = usize::from(matches!(edge, DockEdge::Bottom | DockEdge::Top));
            assert!(((after[0][axis] - before[0][axis]) / (2. * h)).abs() > 100.);
        }
    }

    #[test]
    fn both_effects_reverse_the_exact_mesh_for_every_edge_and_finish() {
        let source = Rectangle::new((80., 45.).into(), (800., 600.).into());
        let target = Rectangle::new((1040., 1000.).into(), (48., 48.).into());
        for edge in [
            DockEdge::Bottom,
            DockEdge::Left,
            DockEdge::Right,
            DockEdge::Top,
        ] {
            for effect in [MinimizeEffect::Genie, MinimizeEffect::Scale] {
                let mut clock = Clock::with_time(Duration::ZERO);
                let mut genie =
                    Genie::with_style(clock.clone(), source, target, true, edge, effect);
                clock.set_unadjusted(Duration::from_millis(170));
                let before = genie.mesh(
                    Rectangle::from_size(source.size),
                    source.size,
                    CommitCounter::default(),
                    1,
                );
                genie.reverse(clock.clone(), false);
                let after = genie.mesh(
                    Rectangle::from_size(source.size),
                    source.size,
                    CommitCounter::default(),
                    1,
                );
                assert_eq!(before.area, after.area);
                assert_eq!(before.vertices, after.vertices);
                assert_eq!(before.alpha, after.alpha);
                clock.set_unadjusted(Duration::from_secs(2));
                let final_frame = genie.mesh(
                    Rectangle::from_size(source.size),
                    source.size,
                    CommitCounter::default(),
                    1,
                );
                assert!(genie.is_done());
                assert_eq!(final_frame.area, source);
                assert_eq!(final_frame.alpha, 1.);
            }
        }
    }

    #[test]
    fn reversal_preserves_incoming_velocity_and_settles_at_the_new_endpoint() {
        for rate in [0.5, 1., 2.] {
            let mut clock = Clock::with_time(Duration::ZERO);
            clock.set_rate(rate);
            let source = Rectangle::new((20., 20.).into(), (640., 480.).into());
            let target = Rectangle::new((300., 680.).into(), (48., 48.).into());
            let mut genie = Genie::new(clock.clone(), source, target, true);
            clock.set_unadjusted(Duration::from_millis(190));
            let velocity = genie.animation.velocity();
            let position = genie.progress();
            genie.reverse(clock.clone(), false);
            assert_eq!(genie.progress(), position);
            approx::assert_abs_diff_eq!(genie.animation.velocity(), velocity, epsilon = 1e-9);
            clock.set_unadjusted(Duration::from_secs(2));
            assert!(genie.is_done());
            assert_eq!(genie.progress(), 0.);
            assert_eq!(genie.animation.velocity(), 0.);
        }
    }

    #[test]
    fn reverse_retains_current_progress_and_releases_timer_at_endpoint() {
        let mut clock = Clock::with_time(Duration::ZERO);
        let source = Rectangle::new((20., 20.).into(), (640., 480.).into());
        let target = Rectangle::new((300., 680.).into(), (48., 48.).into());
        let mut genie = Genie::new(clock.clone(), source, target, true);
        clock.set_unadjusted(Duration::from_millis(190));
        let before = genie.progress();
        genie.reverse(clock.clone(), false);
        assert_eq!(genie.progress(), before);
        clock.set_unadjusted(Duration::from_millis(300));
        assert!(genie.progress() < before);
        genie.reverse(clock.clone(), true);
        clock.set_unadjusted(Duration::from_secs(2));
        assert!(genie.is_done());
        assert_eq!(genie.progress(), 1.);
    }

    #[test]
    fn restored_geometry_retargets_from_current_mesh_without_a_frame_jump() {
        let mut clock = Clock::with_time(Duration::ZERO);
        let source = Rectangle::new((20., 20.).into(), (640., 480.).into());
        let target = Rectangle::new((300., 680.).into(), (48., 48.).into());
        let destination = Rectangle::new((120., 80.).into(), (800., 600.).into());
        let mut genie = Genie::new(clock.clone(), source, target, true);
        let texture_area = Rectangle::from_size(source.size);
        clock.set_unadjusted(Duration::from_millis(200));
        let before = genie.mesh(texture_area, source.size, CommitCounter::default(), 1);
        genie.reverse(clock.clone(), false);
        genie.retarget_destination(destination);
        let handoff = genie.mesh(texture_area, source.size, CommitCounter::default(), 1);
        assert_eq!(before.area, handoff.area);
        assert_eq!(before.vertices, handoff.vertices);
        assert_eq!(before.alpha, handoff.alpha);
        clock.set_unadjusted(Duration::from_millis(400));
        let final_frame = genie.mesh(texture_area, source.size, CommitCounter::default(), 1);
        assert_eq!(final_frame.area, destination);
        assert!(genie.is_done());
    }

    fn restoring_owner(
        clock: &mut Clock,
        rate: f64,
        effect: MinimizeEffect,
        edge: DockEdge,
    ) -> Genie {
        clock.set_rate(rate);
        let source = Rectangle::new((20., 20.).into(), (640., 480.).into());
        let target = Rectangle::new((300., 680.).into(), (48., 48.).into());
        let mut genie = Genie::with_style(clock.clone(), source, target, true, edge, effect);
        clock.set_unadjusted(Duration::from_millis(190));
        genie.reverse(clock.clone(), false);
        clock.set_unadjusted(Duration::from_millis(200));
        genie.retarget_destination(Rectangle::new((120., 80.).into(), (800., 600.).into()));
        clock.set_unadjusted(Duration::from_millis(240));
        genie
    }

    #[test]
    fn mixed_refresh_forecasts_leave_genie_geometry_and_trajectory_event_owned() {
        for rate in [0.5, 1., 2.] {
            for effect in [MinimizeEffect::Genie, MinimizeEffect::Scale] {
                for edge in [
                    DockEdge::Bottom,
                    DockEdge::Left,
                    DockEdge::Right,
                    DockEdge::Top,
                ] {
                    let mut clock = Clock::with_time(Duration::ZERO);
                    let mut genie = restoring_owner(&mut clock, rate, effect, edge);
                    let source = genie.source;
                    let target = genie.target;
                    let destination = genie.destination;
                    let trajectory = (
                        genie.animation.from(),
                        genie.animation.to(),
                        genie.animation.start_time(),
                        genie.animation.end_time(),
                        genie.progress(),
                        genie.animation.velocity(),
                    );
                    let id = genie.id.clone();
                    let texture_area = Rectangle::new((-10., -14.).into(), (660., 504.).into());
                    let tile_size = Size::from((640., 480.));
                    let mut last_commit = genie.commit;
                    // A slow 60 Hz output, faster 120 Hz output, event sample,
                    // and a sample beyond completion arrive in unrelated order.
                    for at in [256, 248, 1000, 240, 256, 248] {
                        let sample = {
                            let _projection = clock.project_to(Duration::from_millis(at));
                            let sample =
                                genie.mesh(texture_area, tile_size, CommitCounter::default(), 7);
                            if at == 1000 {
                                assert!(genie.is_done());
                            }
                            sample
                        };
                        assert_eq!(genie.source, source);
                        assert_eq!(genie.target, target);
                        assert_eq!(genie.destination, destination);
                        assert!(!genie.minimizing);
                        assert_eq!(genie.edge, edge);
                        assert_eq!(genie.effect, effect);
                        assert_eq!(genie.id, id);
                        assert!(!genie.is_done());
                        assert_eq!(
                            (
                                genie.animation.from(),
                                genie.animation.to(),
                                genie.animation.start_time(),
                                genie.animation.end_time(),
                                genie.progress(),
                                genie.animation.velocity(),
                            ),
                            trajectory
                        );
                        let mut expected_clock = Clock::with_time(Duration::ZERO);
                        let mut expected = restoring_owner(&mut expected_clock, rate, effect, edge);
                        let expected_sample = {
                            let _projection = expected_clock.project_to(Duration::from_millis(at));
                            expected.mesh(texture_area, tile_size, CommitCounter::default(), 7)
                        };
                        assert_eq!(sample.area, expected_sample.area);
                        assert_eq!(sample.vertices, expected_sample.vertices);
                        assert_eq!(sample.alpha, expected_sample.alpha);
                        last_commit.increment();
                        assert_eq!(sample.commit, last_commit);
                    }
                }
            }
        }
    }

    #[test]
    fn event_time_reflow_and_reversal_are_independent_of_prior_forecast_samples() {
        let mut sampled_clock = Clock::with_time(Duration::ZERO);
        let mut sampled = restoring_owner(
            &mut sampled_clock,
            1.,
            MinimizeEffect::Genie,
            DockEdge::Bottom,
        );
        let mut untouched_clock = Clock::with_time(Duration::ZERO);
        let mut untouched = restoring_owner(
            &mut untouched_clock,
            1.,
            MinimizeEffect::Genie,
            DockEdge::Bottom,
        );
        let area = Rectangle::from_size((640., 480.).into());
        let size = area.size;
        for at in [300, 248, 450, 256] {
            let _projection = sampled_clock.project_to(Duration::from_millis(at));
            sampled.mesh(area, size, CommitCounter::default(), 1);
        }
        let before = sampled.mesh(area, size, CommitCounter::default(), 1);
        let destination = Rectangle::new((200., 90.).into(), (1000., 700.).into());
        sampled.retarget_destination(destination);
        untouched.retarget_destination(destination);
        let handoff = sampled.mesh(area, size, CommitCounter::default(), 1);
        assert_eq!(handoff.area, before.area);
        assert_eq!(handoff.vertices, before.vertices);
        let progress = sampled.progress();
        let velocity = sampled.animation.velocity();
        sampled.reverse(sampled_clock.clone(), true);
        untouched.reverse(untouched_clock.clone(), true);
        assert_eq!(sampled.progress(), progress);
        approx::assert_abs_diff_eq!(sampled.animation.velocity(), velocity, epsilon = 1e-9);
        assert_eq!(sampled.source, untouched.source);
        assert_eq!(sampled.destination, untouched.destination);
        for at in [248, 256, 300, 1000] {
            let a = {
                let _projection = sampled_clock.project_to(Duration::from_millis(at));
                sampled.mesh(area, size, CommitCounter::default(), 1)
            };
            let b = {
                let _projection = untouched_clock.project_to(Duration::from_millis(at));
                untouched.mesh(area, size, CommitCounter::default(), 1)
            };
            assert_eq!(a.area, b.area);
            assert_eq!(a.vertices, b.vertices);
            assert_eq!(a.alpha, b.alpha);
        }
    }
}
