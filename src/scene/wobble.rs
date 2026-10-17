//! Scene-owned deformation forecasts, mesh identity and bounded geometry cache.
//! The event simulation supplies numerical state and is never advanced here.

use std::rc::Rc;
use std::time::Duration;

use clarion_config::WindowWobble;
use clarion_core::deformation::MeshVertex;
use smithay::backend::renderer::element::Id;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::utils::{Logical, Point, Rectangle};

use crate::layout::wobble::WobbleState;

#[derive(Debug, Clone)]
pub(crate) struct WobbleMesh {
    pub(crate) id: Id,
    pub(crate) commit: CommitCounter,
    pub(crate) area: Rectangle<f64, Logical>,
    pub(crate) vertices: Rc<[MeshVertex]>,
}

#[derive(Debug)]
struct CachedMesh {
    source_area: Rectangle<f64, Logical>,
    area: Rectangle<f64, Logical>,
    vertices: Rc<[MeshVertex]>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ForecastKey {
    owner_revision: u64,
    area: Rectangle<f64, Logical>,
    config: WindowWobble,
    now: Duration,
    source_commit: CommitCounter,
    source_texture: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct GeometryKey {
    owner_revision: u64,
    area: Rectangle<f64, Logical>,
    config: WindowWobble,
    moving_at: Option<Duration>,
}

/// Bounded numerical scratch and render caches for one scene registration.
/// Texture damage changes damage revisions without rebuilding a stable mesh.
#[derive(Debug)]
pub(crate) struct WobbleForecast {
    id: Id,
    state: WobbleState,
    key: Option<ForecastKey>,
    geometry_key: Option<GeometryKey>,
    cached: Option<WobbleMesh>,
    cached_mesh: Option<CachedMesh>,
    commit: CommitCounter,
    points: Vec<(Point<f64, Logical>, [f32; 2])>,
}

impl Default for WobbleForecast {
    fn default() -> Self {
        Self {
            id: Id::new(),
            state: WobbleState::default(),
            key: None,
            geometry_key: None,
            cached: None,
            cached_mesh: None,
            commit: CommitCounter::default(),
            points: Vec::new(),
        }
    }
}

impl WobbleForecast {
    pub(crate) fn clear(&mut self) {
        self.state.cancel();
        self.key = None;
        self.geometry_key = None;
        self.cached = None;
        self.cached_mesh = None;
        self.points.clear();
    }

    pub(crate) fn sample(
        &mut self,
        owner: &WobbleState,
        area: Rectangle<f64, Logical>,
        config: WindowWobble,
        now: Duration,
        source_commit: CommitCounter,
        source_texture: u64,
    ) -> Option<WobbleMesh> {
        let key = ForecastKey {
            owner_revision: owner.revision(),
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
        self.state.rebase_forecast(area);
        self.key = Some(key);
        self.commit.increment();
        if !self.state.predict(area, config, now) {
            self.cached = None;
            self.cached_mesh = None;
            self.geometry_key = None;
            return None;
        }
        let geometry_key = GeometryKey {
            owner_revision: owner.revision(),
            area,
            config,
            moving_at: self.state.is_ongoing().then_some(now),
        };
        if self.geometry_key != Some(geometry_key) || self.cached_mesh.is_none() {
            self.rebuild_mesh_vertices(area);
            self.geometry_key = Some(geometry_key);
        }
        let geometry = self.cached_mesh.as_ref().unwrap();
        debug_assert_eq!(geometry.source_area, area);
        let mesh = WobbleMesh {
            id: self.id.clone(),
            commit: self.commit,
            area: geometry.area,
            vertices: geometry.vertices.clone(),
        };
        self.cached = Some(mesh.clone());
        Some(mesh)
    }

    #[cfg(test)]
    pub(crate) fn points_ptr_for_test(&self) -> *const (Point<f64, Logical>, [f32; 2]) {
        self.points.as_ptr()
    }
    #[cfg(test)]
    pub(crate) fn cached_for_test(&self) -> Option<&WobbleMesh> {
        self.cached.as_ref()
    }
    #[cfg(test)]
    pub(crate) fn rebuild_for_test(&mut self, owner: &WobbleState, area: Rectangle<f64, Logical>) {
        self.state.copy_simulation_from(owner);
        self.rebuild_mesh_vertices(area);
        let geometry = self.cached_mesh.as_ref().unwrap();
        self.cached = Some(WobbleMesh {
            id: self.id.clone(),
            commit: self.commit,
            area: geometry.area,
            vertices: geometry.vertices.clone(),
        });
    }

    fn rebuild_mesh_vertices(&mut self, source_area: Rectangle<f64, Logical>) {
        let side = self.state.mesh_side();
        let points = &mut self.points;
        points.clear();
        if points.capacity() < side * side {
            points.reserve(side * side);
        }
        let denom = (side - 1) as f64;
        let mut min = Point::from((f64::INFINITY, f64::INFINITY));
        let mut max = Point::from((f64::NEG_INFINITY, f64::NEG_INFINITY));

        for y in 0..self.state.mesh_side() {
            for x in 0..self.state.mesh_side() {
                let u = x as f64 / denom;
                let v = y as f64 / denom;
                let node = self.state.nodes()[y * self.state.mesh_side() + x];
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
        let mut vertices =
            Vec::with_capacity((self.state.mesh_side() - 1) * (self.state.mesh_side() - 1) * 6);
        for y in 0..self.state.mesh_side() - 1 {
            for x in 0..self.state.mesh_side() - 1 {
                let tl = y * self.state.mesh_side() + x;
                let tr = tl + 1;
                let bl = (y + 1) * self.state.mesh_side() + x;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::wobble::WobbleAnchor;

    fn moving_owner() -> (WobbleState, Rectangle<f64, Logical>, WindowWobble) {
        let mut owner = WobbleState::default();
        let config = WindowWobble::default();
        let area = Rectangle::new((40., 0.).into(), (800., 600.).into());
        owner.begin(WobbleAnchor::Point((0.5, 0.5)), config, Duration::ZERO);
        owner.add_pending_displacement((40., -20.).into());
        owner.advance(area, config, Duration::ZERO);
        owner.advance(area, config, Duration::from_millis(4));
        (owner, area, config)
    }

    #[test]
    fn same_timestamp_forecast_projects_the_owner_remainder_without_committing_it() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<WobbleState>();
        let (owner, area, config) = moving_owner();
        let nodes = owner.nodes().to_vec();
        let revision = owner.revision();
        let mut forecast = WobbleForecast::default();
        let first = forecast
            .sample(
                &owner,
                area,
                config,
                Duration::from_millis(4),
                CommitCounter::default(),
                1,
            )
            .unwrap();
        let first_point =
            first.area.loc.x + f64::from(first.vertices[0].position[0]) * first.area.size.w;
        let raw_point = area.loc.x + owner.nodes()[0].offset[0];
        assert!(
            (first_point - raw_point).abs() > 1e-4,
            "a nonzero fractional step must move before the next 120 Hz tick"
        );
        let later = forecast
            .sample(
                &owner,
                area,
                config,
                Duration::from_millis(8),
                CommitCounter::default(),
                1,
            )
            .unwrap();
        assert_ne!(first.vertices, later.vertices);
        let earlier = forecast
            .sample(
                &owner,
                area,
                config,
                Duration::from_millis(4),
                CommitCounter::default(),
                1,
            )
            .unwrap();
        assert_eq!(first.vertices, earlier.vertices);
        assert_eq!(first.area, earlier.area);
        let repeated = forecast
            .sample(
                &owner,
                area,
                config,
                Duration::from_millis(4),
                CommitCounter::default(),
                1,
            )
            .unwrap();
        assert_eq!(earlier.commit, repeated.commit);
        assert!(Rc::ptr_eq(&earlier.vertices, &repeated.vertices));
        assert_eq!(owner.nodes(), nodes);
        assert_eq!(owner.revision(), revision);
    }

    #[test]
    fn scene_clear_retires_meshes_and_scratch_stays_bounded_across_mesh_changes() {
        let (mut owner, area, mut config) = moving_owner();
        let mut forecast = WobbleForecast::default();
        for mesh in [12, 32, 2, 32] {
            config.mesh = mesh;
            owner.begin(WobbleAnchor::Point((0.5, 0.5)), config, Duration::ZERO);
            owner.advance(area, config, Duration::ZERO);
            let rendered = forecast
                .sample(
                    &owner,
                    area,
                    config,
                    Duration::ZERO,
                    CommitCounter::default(),
                    1,
                )
                .unwrap();
            assert_eq!(rendered.vertices.len(), 6 * (usize::from(mesh) - 1).pow(2));
            assert!(forecast.points.capacity() <= 32 * 32);
            let vertices = Rc::downgrade(&rendered.vertices);
            drop(rendered);
            assert!(vertices.upgrade().is_some());
            forecast.clear();
            assert!(vertices.upgrade().is_none());
            assert!(owner.is_active());
        }
    }
}
