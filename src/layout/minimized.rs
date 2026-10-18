//! Renderer-independent mapped-window minimization trajectory and geometry.
//!
//! A minimized Tile remains owned by its workspace, so commits, output changes,
//! toplevel destruction and IPC keep their normal lifetime. It is detached from
//! the visible layout and input until activation restores it. The animation only
//! deforms its presentation; it never unmaps a client's wl_surface.

use clarion_core::deformation::smoothstep;
use clarion_ipc::{DockEdge, MinimizeEffect};
use smithay::utils::{Logical, Rectangle};

use crate::animation::{Animation, Clock, Curve};

fn duration_ms(effect: MinimizeEffect) -> u64 {
    // Dock 2092.50.2 defaults recovered from the Big Sur 11.7.2 ARM64e path.
    // Geometry and interruption handling remain Clarion implementations.
    match effect {
        MinimizeEffect::Genie => 500,
        MinimizeEffect::Scale => 250,
    }
}

#[derive(Debug)]
pub struct Genie {
    animation: Animation,
    source: Rectangle<f64, Logical>,
    // Keep stage weights stable when restoring into a changed layout.
    phase_source: Rectangle<f64, Logical>,
    target: Rectangle<f64, Logical>,
    minimizing: bool,
    edge: DockEdge,
    effect: MinimizeEffect,
    duration_ms: u64,
    destination: Option<(Rectangle<f64, Logical>, f64)>,
}

/// A frozen output-presentation pose. Render caches and texture identities are
/// scene-owned and never mutate the event-time minimization trajectory.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct GeniePose {
    pub(crate) progress: f64,
    pub(crate) source: Rectangle<f64, Logical>,
    pub(crate) phase_source: Rectangle<f64, Logical>,
    pub(crate) target: Rectangle<f64, Logical>,
    pub(crate) edge: DockEdge,
    pub(crate) effect: MinimizeEffect,
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
        let duration_ms = duration_ms(effect);
        Self {
            animation: Animation::ease(clock, from, to, 0., duration_ms, Curve::Linear),
            source,
            phase_source: source,
            target,
            minimizing,
            edge,
            effect,
            duration_ms,
            destination: None,
        }
    }

    /// Reverse from the exact visible progress. No endpoint restart or frame jump.
    pub fn reverse(&mut self, clock: Clock, minimizing: bool) {
        let from = self.progress();
        self.source = self.source_at(from);
        self.destination = None;
        let to = if minimizing { 1. } else { 0. };
        let duration = (self.duration_ms as f64 * (to - from).abs()).round() as u64;
        let velocity = if self.animation.is_done() {
            0.
        } else {
            self.animation.velocity()
        };
        self.animation = Animation::continuous_ease(clock, from, to, velocity, duration);
        self.minimizing = minimizing;
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
    }

    /// Sample only numerical presentation values. Reversals and retargeting
    /// remain owner-thread operations even when outputs forecast out of order.
    pub(crate) fn pose(&self) -> GeniePose {
        let progress = self.progress();
        GeniePose {
            progress,
            source: self.source_at(progress),
            phase_source: self.phase_source,
            target: self.target,
            edge: self.edge,
            effect: self.effect,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use smithay::backend::renderer::utils::CommitCounter;
    use smithay::utils::Size;

    use super::*;
    use crate::scene::genie::{minimize_vertices, GenieCache, GenieMesh};

    #[test]
    fn minimize_timing_and_reversal_use_recovered_defaults() {
        for (effect, duration) in [(MinimizeEffect::Genie, 500), (MinimizeEffect::Scale, 250)] {
            let mut clock = Clock::with_time(Duration::ZERO);
            let source = Rectangle::new((20., 20.).into(), (640., 480.).into());
            let target = Rectangle::new((300., 680.).into(), (48., 48.).into());
            let mut genie = Genie::with_style(
                clock.clone(),
                source,
                target,
                true,
                DockEdge::Bottom,
                effect,
            );
            clock.set_unadjusted(Duration::from_millis(duration / 2));
            let before = genie.pose();
            assert!((before.progress - 0.5).abs() < 0.003);
            genie.reverse(clock.clone(), false);
            assert_eq!(before, genie.pose());
            clock.set_unadjusted(Duration::from_millis(duration * 2));
            assert!(genie.is_done());
            assert_eq!(genie.progress(), 0.);
        }
    }

    struct SampledGenie {
        model: Genie,
        cache: GenieCache,
    }

    impl std::ops::Deref for SampledGenie {
        type Target = Genie;
        fn deref(&self) -> &Genie {
            &self.model
        }
    }
    impl std::ops::DerefMut for SampledGenie {
        fn deref_mut(&mut self) -> &mut Genie {
            &mut self.model
        }
    }
    impl SampledGenie {
        fn new(
            clock: Clock,
            source: Rectangle<f64, Logical>,
            target: Rectangle<f64, Logical>,
            minimizing: bool,
        ) -> Self {
            Self {
                model: Genie::new(clock, source, target, minimizing),
                cache: GenieCache::default(),
            }
        }
        fn with_style(
            clock: Clock,
            source: Rectangle<f64, Logical>,
            target: Rectangle<f64, Logical>,
            minimizing: bool,
            edge: DockEdge,
            effect: MinimizeEffect,
        ) -> Self {
            Self {
                model: Genie::with_style(clock, source, target, minimizing, edge, effect),
                cache: GenieCache::default(),
            }
        }
        fn mesh(
            &mut self,
            texture_area: Rectangle<f64, Logical>,
            tile_size: Size<f64, Logical>,
            source_commit: CommitCounter,
            source_texture: u64,
        ) -> GenieMesh {
            self.cache.sample(
                &self.model.pose(),
                texture_area,
                tile_size,
                source_commit,
                source_texture,
                1.,
            )
        }
    }

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
                        assert_eq!(vertices.len() % 6, 0);
                        assert!((6..=clarion_core::deformation::GENIE_MAX_SEGMENTS * 6)
                            .contains(&vertices.len()));
                        if effect == MinimizeEffect::Scale {
                            assert_eq!(vertices.len(), 6);
                        }
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
    fn genie_corner_motion_is_continuous_across_phase_changes() {
        let source = Rectangle::new((80., 45.).into(), (800., 600.).into());
        let target = Rectangle::new((1040., 1000.).into(), (48., 48.).into());
        let h = 0.001;
        for edge in [
            DockEdge::Bottom,
            DockEdge::Left,
            DockEdge::Right,
            DockEdge::Top,
        ] {
            // Adaptive subdivision can change vertex count. Follow fixed UVs,
            // rather than comparing unrelated rows of consecutive meshes.
            let corners = |p| {
                let (area, vertices) =
                    minimize_vertices(source, target, p, edge, MinimizeEffect::Genie);
                [[0., 0.], [1., 0.], [0., 1.], [1., 1.]].map(|uv| {
                    let v = vertices.iter().find(|v| v.uv == uv).unwrap();
                    [
                        area.loc.x + f64::from(v.position[0]) * area.size.w,
                        area.loc.y + f64::from(v.position[1]) * area.size.h,
                    ]
                })
            };
            for p in (1..100).map(|n| f64::from(n) / 100.) {
                let before = corners(p - h);
                let middle = corners(p);
                let after = corners(p + h);
                for ((before, middle), after) in before.iter().zip(&middle).zip(&after) {
                    for axis in 0..2 {
                        let incoming = (middle[axis] - before[axis]) / h;
                        let outgoing = (after[axis] - middle[axis]) / h;
                        assert!(
                            (incoming - outgoing).abs() < 50.,
                            "velocity jump at {edge:?}/{p}, axis {axis}: {incoming} -> {outgoing}"
                        );
                    }
                }
            }
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
                    SampledGenie::with_style(clock.clone(), source, target, true, edge, effect);
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
            let mut genie = SampledGenie::new(clock.clone(), source, target, true);
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
        let mut genie = SampledGenie::new(clock.clone(), source, target, true);
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
        let mut genie = SampledGenie::new(clock.clone(), source, target, true);
        let texture_area = Rectangle::from_size(source.size);
        clock.set_unadjusted(Duration::from_millis(200));
        let before = genie.mesh(texture_area, source.size, CommitCounter::default(), 1);
        genie.reverse(clock.clone(), false);
        genie.retarget_destination(destination);
        let handoff = genie.mesh(texture_area, source.size, CommitCounter::default(), 1);
        assert_eq!(genie.pose().phase_source, source);
        assert_eq!(before.area, handoff.area);
        assert_eq!(before.vertices, handoff.vertices);
        assert_eq!(before.alpha, handoff.alpha);
        clock.set_unadjusted(Duration::from_millis(400));
        let final_frame = genie.mesh(texture_area, source.size, CommitCounter::default(), 1);
        assert_eq!(genie.pose().phase_source, source);
        assert_eq!(final_frame.area, destination);
        assert!(genie.is_done());
    }

    fn restoring_owner(
        clock: &mut Clock,
        rate: f64,
        effect: MinimizeEffect,
        edge: DockEdge,
    ) -> SampledGenie {
        clock.set_rate(rate);
        let source = Rectangle::new((20., 20.).into(), (640., 480.).into());
        let target = Rectangle::new((300., 680.).into(), (48., 48.).into());
        let mut genie = SampledGenie::with_style(clock.clone(), source, target, true, edge, effect);
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
                    let mut id = None;
                    let texture_area = Rectangle::new((-10., -14.).into(), (660., 504.).into());
                    let tile_size = Size::from((640., 480.));
                    let mut last_commit = CommitCounter::default();
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
                        assert_eq!(sample.id, *id.get_or_insert_with(|| sample.id.clone()));
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
