//! Scene-owned minimization mesh identity, damage revisions and texture cache.
//! The numerical trajectory is sampled before entry and is never retargeted here.

use std::rc::Rc;

use clarion_core::deformation::MeshVertex;
use clarion_ipc::{DockEdge, MinimizeEffect};
use smithay::backend::renderer::element::Id;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::utils::{Logical, Point, Rectangle, Size};

use crate::layout::minimized::GeniePose;

#[derive(Debug, Clone, Copy, PartialEq)]
struct GenieFrameKey {
    pose: GeniePose,
    texture_area: Rectangle<f64, Logical>,
    tile_size: Size<f64, Logical>,
    source_commit: CommitCounter,
    source_texture: u64,
    physical_scale: f64,
}

#[derive(Debug, Clone)]
pub(crate) struct GenieMesh {
    pub(crate) id: Id,
    pub(crate) commit: CommitCounter,
    pub(crate) area: Rectangle<f64, Logical>,
    pub(crate) vertices: Rc<[MeshVertex]>,
    pub(crate) alpha: f32,
}

#[derive(Debug)]
pub(crate) struct GenieCache {
    id: Id,
    commit: CommitCounter,
    last_frame: Option<GenieFrameKey>,
    cached: Option<GenieMesh>,
}

impl Default for GenieCache {
    fn default() -> Self {
        Self {
            id: Id::new(),
            commit: CommitCounter::default(),
            last_frame: None,
            cached: None,
        }
    }
}

impl GenieCache {
    pub(crate) fn clear(&mut self) {
        if self.last_frame.take().is_some() {
            self.cached = None;
            self.commit.increment();
        }
    }

    pub(crate) fn sample(
        &mut self,
        pose: &GeniePose,
        texture_area: Rectangle<f64, Logical>,
        tile_size: Size<f64, Logical>,
        source_commit: CommitCounter,
        source_texture: u64,
        physical_scale: f64,
    ) -> GenieMesh {
        // Include the complete frozen pose: a reversal or layout reflow can
        // replace source geometry without changing this instant's progress.
        let frame = GenieFrameKey {
            pose: *pose,
            texture_area,
            tile_size,
            source_commit,
            source_texture,
            physical_scale,
        };
        if self.last_frame == Some(frame) {
            return self.cached.clone().unwrap();
        }
        self.last_frame = Some(frame);
        self.commit.increment();
        let base = pose.source;
        let sx = base.size.w / tile_size.w.max(1.);
        let sy = base.size.h / tile_size.h.max(1.);
        let source = Rectangle::new(
            base.loc + Point::from((texture_area.loc.x * sx, texture_area.loc.y * sy)),
            (texture_area.size.w * sx, texture_area.size.h * sy).into(),
        );
        let (area, vertices) = minimize_vertices_with_geometry(
            source,
            pose.target,
            pose.phase_source,
            pose.progress,
            pose.edge,
            pose.effect,
            physical_scale,
        );
        let mesh = GenieMesh {
            id: self.id.clone(),
            commit: self.commit,
            area,
            vertices: vertices.into(),
            // The recovered warp does not establish an extra fade-out.
            // Completion releases this mesh through the animation lifecycle.
            alpha: 1.,
        };
        self.cached = Some(mesh.clone());
        mesh
    }
}

/// Sample the common geometry at logical scale 1. Rendering uses the output
/// scale aware entry point below for the screen-space tessellation budget.
#[cfg(test)]
pub fn minimize_vertices(
    source: Rectangle<f64, Logical>,
    target: Rectangle<f64, Logical>,
    progress: f64,
    edge: DockEdge,
    effect: MinimizeEffect,
) -> (Rectangle<f64, Logical>, Vec<MeshVertex>) {
    minimize_vertices_with_geometry(source, target, source, progress, edge, effect, 1.)
}

/// Freeze the phase geometry separately from texture bounds: output rounding
/// and changing shadow padding must not retime an existing animation.
fn minimize_vertices_with_geometry(
    source: Rectangle<f64, Logical>,
    target: Rectangle<f64, Logical>,
    phase_source: Rectangle<f64, Logical>,
    progress: f64,
    edge: DockEdge,
    effect: MinimizeEffect,
    physical_scale: f64,
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
    let (area, vertices) = clarion_core::deformation::minimize_vertices_with_geometry(
        convert(source),
        convert(target),
        convert(phase_source),
        progress,
        edge,
        effect,
        physical_scale,
    );
    (
        Rectangle::new((area.x, area.y).into(), (area.width, area.height).into()),
        vertices,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pose() -> GeniePose {
        GeniePose {
            progress: 0.3,
            source: Rectangle::new((20., 20.).into(), (640., 480.).into()),
            phase_source: Rectangle::new((20., 20.).into(), (640., 480.).into()),
            target: Rectangle::new((300., 680.).into(), (48., 48.).into()),
            edge: DockEdge::Bottom,
            effect: MinimizeEffect::Genie,
        }
    }

    #[test]
    fn identical_pose_reuses_mesh_but_source_handoff_invalidates_at_equal_progress() {
        let mut cache = GenieCache::default();
        let mut pose = pose();
        let area = Rectangle::from_size(pose.source.size);
        let first = cache.sample(
            &pose,
            area,
            pose.source.size,
            CommitCounter::default(),
            7,
            1.,
        );
        let repeated = cache.sample(
            &pose,
            area,
            pose.source.size,
            CommitCounter::default(),
            7,
            1.,
        );
        assert_eq!(first.commit, repeated.commit);
        assert!(Rc::ptr_eq(&first.vertices, &repeated.vertices));
        pose.source.loc.x += 100.;
        let moved = cache.sample(
            &pose,
            area,
            pose.source.size,
            CommitCounter::default(),
            7,
            1.,
        );
        assert_ne!(first.commit, moved.commit);
        assert_ne!(first.area, moved.area);
        assert!(!Rc::ptr_eq(&first.vertices, &moved.vertices));
        assert_eq!(first.id, moved.id);
        let mut source_commit = CommitCounter::default();
        source_commit.increment();
        let damaged = cache.sample(&pose, area, pose.source.size, source_commit, 7, 1.);
        assert_ne!(moved.commit, damaged.commit);
        assert_eq!(moved.area, damaged.area);
    }

    #[test]
    fn finishing_an_effect_releases_cached_geometry_before_the_next_animation() {
        let mut cache = GenieCache::default();
        let pose = pose();
        let area = Rectangle::from_size(pose.source.size);
        let first = cache.sample(
            &pose,
            area,
            pose.source.size,
            CommitCounter::default(),
            7,
            1.,
        );
        let previous_commit = first.commit;
        let vertices = Rc::downgrade(&first.vertices);
        drop(first);
        assert!(vertices.upgrade().is_some());
        cache.clear();
        assert!(vertices.upgrade().is_none());
        let next = cache.sample(
            &pose,
            area,
            pose.source.size,
            CommitCounter::default(),
            7,
            1.,
        );
        assert_ne!(next.commit, previous_commit);
        assert_eq!(cache.last_frame.unwrap().pose, pose);
    }

    #[test]
    fn output_scale_invalidates_the_frozen_mesh_identity_without_a_fade() {
        let mut cache = GenieCache::default();
        let mut pose = pose();
        let area = Rectangle::from_size(pose.source.size);
        let first = cache.sample(
            &pose,
            area,
            pose.source.size,
            CommitCounter::default(),
            7,
            1.,
        );
        let scaled = cache.sample(
            &pose,
            area,
            pose.source.size,
            CommitCounter::default(),
            7,
            1.5,
        );
        assert_ne!(first.commit, scaled.commit);
        assert_eq!(first.id, scaled.id);
        let repeat = cache.sample(
            &pose,
            area,
            pose.source.size,
            CommitCounter::default(),
            7,
            1.5,
        );
        assert_eq!(scaled.commit, repeat.commit);
        assert!(Rc::ptr_eq(&scaled.vertices, &repeat.vertices));
        pose.progress = 0.98;
        let finishing = cache.sample(
            &pose,
            area,
            pose.source.size,
            CommitCounter::default(),
            7,
            1.5,
        );
        assert_eq!(finishing.alpha, 1.);
    }

    #[test]
    fn texture_padding_cannot_retime_the_frozen_window_phases() {
        let mut cache = GenieCache::default();
        let mut pose = pose();
        pose.progress = 0.6;
        let bounds = Rectangle::from_size(pose.source.size);
        let unpadded = cache.sample(
            &pose,
            bounds,
            pose.source.size,
            CommitCounter::default(),
            7,
            1.,
        );
        // Exaggerated decoration bounds exercise phase thresholds as well as
        // the smaller rounding changes produced by fractional output scales.
        for texture_area in [
            Rectangle::new((-0.4, -0.8).into(), (640.8, 481.6).into()),
            Rectangle::new((-120., -250.).into(), (880., 980.).into()),
        ] {
            for scale in [1., 1.25, 1.5, 2.] {
                let padded = cache.sample(
                    &pose,
                    texture_area,
                    pose.source.size,
                    CommitCounter::default(),
                    7,
                    scale,
                );
                let source_y = pose.source.loc.y + texture_area.loc.y;
                let travel = (padded.area.loc.y - source_y) / (pose.target.loc.y - source_y);
                let expected = (unpadded.area.loc.y - pose.source.loc.y)
                    / (pose.target.loc.y - pose.source.loc.y);
                assert!((travel - expected).abs() < 1e-12);
                assert_ne!(padded.commit, unpadded.commit);
                assert_eq!(padded.id, unpadded.id);
            }
        }
    }
}
