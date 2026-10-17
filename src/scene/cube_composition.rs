//! Cube presentation from captured 2D sources and numeric face poses.
//!
//! Retained targets, mesh identity, commits and source feedback belong to the
//! scene. Layout projects sources once and never receives a renderer back.
use smithay::backend::renderer::element::utils::{
    CropRenderElement, Relocate, RelocateRenderElement, RescaleRenderElement,
};
use smithay::backend::renderer::element::{Element as _, Kind};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::renderer::Color32F;
use smithay::utils::{Logical, Point, Rectangle, Size};

use crate::layout::monitor::{MonitorInnerRenderElement, MonitorRenderElement};
use crate::layout::scene_source::{WindowFeedbackSource, WorkspaceScene};
use crate::layout::workspace::WorkspaceId;
use crate::layout::LayoutElement;
use crate::render_helpers::mesh::MeshRenderElement;
use crate::render_helpers::offscreen::OffscreenData;
use crate::render_helpers::renderer::ClarionRenderer;
use crate::render_helpers::solid_color::SolidColorRenderElement;
use crate::render_helpers::vulkan::native::{NativeError, NativeRenderer};
use crate::render_helpers::xray::XrayPos;
use crate::render_helpers::RenderCtx;
use crate::scene::cube_geometry::face_mesh;
use crate::scene::monitor_resources::{CubeFaceRenderElement, CubeFaceState, MonitorResources};
use crate::scene::retained::ResourceLease;
use crate::scene::workspace_composition::{Channel, Composition};

#[derive(Debug, Clone, Copy)]
pub(crate) struct CubeFace {
    pub(crate) workspace_index: usize,
    pub(crate) workspace_id: WorkspaceId,
    pub(crate) angle: f64,
    pub(crate) active: bool,
}

/// A frame-scoped capability limited to offscreen callback visibility. It has
/// no access to focus, layout policy, window traversal or desktop mutation.
pub(crate) struct CubeLifecycle<'a, W: LayoutElement> {
    pub(crate) resources: ResourceLease<MonitorResources>,
    pub(crate) feedback: Vec<(WorkspaceId, WindowFeedbackSource<'a, W>)>,
}

impl<W: LayoutElement> CubeLifecycle<'_, W> {
    fn select(&self, visible: [Option<WorkspaceId>; 2]) {
        let previous = std::mem::replace(
            &mut *self.resources.cube_visible_workspaces.borrow_mut(),
            visible,
        );
        for id in previous.into_iter().flatten() {
            if !visible.contains(&Some(id)) {
                self.set_feedback(id, None);
            }
        }
    }

    fn set_feedback(&self, id: WorkspaceId, data: Option<OffscreenData>) {
        for (workspace, source) in &self.feedback {
            if *workspace == id {
                source.set_offscreen_data(data.clone());
            }
        }
    }

    pub(crate) fn record_feedback(&self, id: WorkspaceId, data: OffscreenData) {
        let visible = *self.resources.cube_visible_workspaces.borrow();
        let mut cache = self.resources.cube_feedback.borrow_mut();
        // Face front-order swaps slots halfway through a turn. Feedback is
        // keyed by workspace identity, so a successful first face cannot evict
        // the other face's last completed callback state before a Busy retry.
        let index = cache
            .iter()
            .position(|entry| entry.as_ref().is_some_and(|(cached, _)| *cached == id))
            .or_else(|| {
                cache.iter().position(|entry| {
                    entry
                        .as_ref()
                        .is_none_or(|(cached, _)| !visible.contains(&Some(*cached)))
                })
            })
            .expect("at most two visible Cube feedback identities");
        cache[index] = Some((id, data.clone()));
        drop(cache);
        self.set_feedback(id, Some(data));
    }

    pub(crate) fn restore_feedback(&self) {
        let visible = *self.resources.cube_visible_workspaces.borrow();
        for (id, data) in self.resources.cube_feedback.borrow().iter().flatten() {
            if visible.contains(&Some(*id)) {
                self.set_feedback(*id, Some(data.clone()));
            }
        }
    }

    pub(crate) fn leave_cube(&self) {
        // This also retires partially prepared targets after a fatal error. A
        // Busy result never takes this branch and preserves pending work.
        if self
            .resources
            .cube_visible_workspaces
            .borrow()
            .iter()
            .all(Option::is_none)
        {
            return;
        }
        self.select([None, None]);
        self.resources.clear_cube();
    }
}

pub(crate) struct CubeComposition<'a, W: LayoutElement> {
    pub(crate) fallback: Composition<'a, W>,
    pub(crate) faces: [Option<CubeFace>; 2],
    pub(crate) lifecycle: CubeLifecycle<'a, W>,
    pub(crate) view_size: Size<f64, Logical>,
    pub(crate) perspective: f64,
    pub(crate) background_color: Color32F,
}

impl<W: LayoutElement> CubeComposition<'_, W> {
    pub(crate) fn workspace_scenes(&self) -> impl Iterator<Item = WorkspaceScene> + '_ {
        self.fallback
            .geometry
            .placements
            .iter()
            .zip(&self.fallback.workspaces)
            .filter(|(placement, _)| placement.normal_visible)
            .map(|(placement, sources)| WorkspaceScene {
                namespace: placement.id.get() as usize,
                geometry: placement.geometry,
                zoom: placement.zoom,
                background: sources.background.clone(),
            })
    }

    pub(crate) fn render<R: ClarionRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        focus_ring: bool,
        push: &mut dyn FnMut(MonitorRenderElement<R>),
    ) {
        let _span = clarion_tracy_span!("Scene::compose_cube");
        let result = if let Some(gles) = ctx.as_gles() {
            MeshRenderElement::is_available(gles.renderer)
                .then(|| self.render_gles(gles, focus_ring))
        } else {
            ctx.as_native()
                .map(|native| self.render_native(native, focus_ring))
        };
        match result {
            Some(Ok(meshes)) => {
                for mesh in meshes.into_iter().flatten() {
                    push(untransformed(MonitorInnerRenderElement::Mesh(mesh)));
                }
                let mut background = self.lifecycle.resources.cube_background.borrow_mut();
                background.update(self.view_size, self.background_color);
                push(untransformed(MonitorInnerRenderElement::SolidColor(
                    SolidColorRenderElement::from_buffer(
                        &background,
                        Point::default(),
                        1.,
                        Kind::Unspecified,
                    ),
                )));
                return;
            }
            Some(Err(error)) if NativeError::is_backpressure(error.as_ref()) => {
                // Defer the complete output frame. Retained targets, source
                // callback visibility and pending allocation must survive.
                self.lifecycle.restore_feedback();
                return;
            }
            Some(Err(error)) => {
                warn!("error rendering workspace cube; falling back to 2D: {error:?}");
            }
            None => {}
        }
        self.lifecycle.leave_cube();
        self.fallback.render(ctx, focus_ring, push);
    }

    fn render_gles(
        &self,
        mut ctx: RenderCtx<GlesRenderer>,
        focus_ring: bool,
    ) -> anyhow::Result<[Option<MeshRenderElement>; 2]> {
        self.lifecycle
            .select(self.faces.map(|face| face.map(|face| face.workspace_id)));
        let mut meshes = [None, None];
        for (slot, face) in self.faces.into_iter().enumerate() {
            let Some(face) = face else {
                continue;
            };
            meshes[slot] = self.render_face_gles(ctx.r(), face, slot, focus_ring && face.active)?;
        }
        Ok(meshes)
    }

    fn render_native(
        &self,
        mut ctx: RenderCtx<NativeRenderer>,
        focus_ring: bool,
    ) -> anyhow::Result<[Option<MeshRenderElement>; 2]> {
        self.lifecycle
            .select(self.faces.map(|face| face.map(|face| face.workspace_id)));
        let mut meshes = [None, None];
        for (slot, face) in self.faces.into_iter().enumerate() {
            let Some(face) = face else {
                continue;
            };
            meshes[slot] =
                self.render_face_native(ctx.r(), face, slot, focus_ring && face.active)?;
        }
        Ok(meshes)
    }

    /// Both backends consume the same already projected four source channels.
    fn collect_face<R: ClarionRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        face: CubeFace,
        focus_ring: bool,
        elements: &mut Vec<CubeFaceRenderElement<R>>,
    ) {
        let scale = self.fallback.geometry.scale;
        let crop = Rectangle::from_size(self.view_size).to_physical_precise_round(scale);
        let sources = &self.fallback.workspaces[face.workspace_index];
        let xray = XrayPos::new(Point::default(), 1.);
        for channel in Channel::ORDER {
            for source in sources
                .items
                .iter()
                .filter(|source| source.channel == channel)
            {
                source
                    .item
                    .render(ctx.r(), xray, focus_ring, &mut |element| {
                        if let Some(element) = CropRenderElement::from_element(element, scale, crop)
                        {
                            elements.push(CubeFaceRenderElement::from(element));
                        }
                    });
            }
        }
        elements.push(CubeFaceRenderElement::SolidColor(
            sources.background.clone(),
        ));
    }

    fn render_face_gles(
        &self,
        mut ctx: RenderCtx<GlesRenderer>,
        face: CubeFace,
        slot: usize,
        focus_ring: bool,
    ) -> anyhow::Result<Option<MeshRenderElement>> {
        let Some((area, vertices)) = face_mesh(self.view_size, face.angle, self.perspective) else {
            return Ok(None);
        };
        let resources = &self.lifecycle.resources;
        let mut elements = resources.cube_elements[slot].borrow_mut();
        elements.clear();
        self.collect_face(ctx.r(), face, focus_ring, &mut elements);
        let result = resources.cube_buffers[slot].render(
            ctx.renderer,
            self.fallback.geometry.scale.into(),
            &elements,
        );
        elements.clear();
        drop(elements);
        let (texture, _sync, mut data) = result?;
        let state = self.face_state(
            face,
            texture.current_commit(),
            u64::from(texture.texture().tex_id()),
            focus_ring,
        );
        let commit = self.commit(slot, state);
        let element = MeshRenderElement::new(
            resources.cube_ids[slot].clone(),
            commit,
            texture.texture().clone(),
            texture.src().size,
            area,
            vertices,
            1.,
            Kind::Unspecified,
        );
        data.id = element.id().clone();
        self.lifecycle.record_feedback(face.workspace_id, data);
        Ok(Some(element))
    }

    fn render_face_native(
        &self,
        mut ctx: RenderCtx<NativeRenderer>,
        face: CubeFace,
        slot: usize,
        focus_ring: bool,
    ) -> anyhow::Result<Option<MeshRenderElement>> {
        let Some((area, vertices)) = face_mesh(self.view_size, face.angle, self.perspective) else {
            return Ok(None);
        };
        let mut elements = Vec::new();
        self.collect_face(ctx.r(), face, focus_ring, &mut elements);
        let resources = &self.lifecycle.resources;
        let (texture, _sync, geometry, states, source_commit) = resources.cube_native_buffers[slot]
            .render(ctx.renderer, self.fallback.geometry.scale.into(), &elements)?;
        // Shared-queue layout barriers order incremental rendering against the
        // previous frame's sampling; this path introduces no CPU fence wait.
        let state = self.face_state(face, source_commit, texture.cache_key(), focus_ring);
        let commit = self.commit(slot, state);
        let used = Size::from((f64::from(geometry.size.w), f64::from(geometry.size.h)));
        let element = MeshRenderElement::new_vulkan(
            resources.cube_ids[slot].clone(),
            commit,
            texture,
            used,
            area,
            vertices,
            1.,
            Kind::Unspecified,
        );
        let data = OffscreenData {
            id: element.id().clone(),
            states,
        };
        self.lifecycle.record_feedback(face.workspace_id, data);
        Ok(Some(element))
    }

    fn face_state(
        &self,
        face: CubeFace,
        source_commit: CommitCounter,
        source_texture: u64,
        focus_ring: bool,
    ) -> CubeFaceState {
        CubeFaceState {
            workspace_idx: face.workspace_index,
            source_commit,
            source_texture,
            angle_bits: face.angle.to_bits(),
            perspective_bits: self.perspective.to_bits(),
            width_bits: self.view_size.w.to_bits(),
            height_bits: self.view_size.h.to_bits(),
            scale_bits: self.fallback.geometry.scale.to_bits(),
            focus_ring,
        }
    }

    fn commit(&self, slot: usize, state: CubeFaceState) -> CommitCounter {
        let resources = &self.lifecycle.resources;
        let changed = {
            let mut states = resources.cube_face_states.borrow_mut();
            let changed = states[slot] != Some(state);
            states[slot] = Some(state);
            changed
        };
        let mut commits = resources.cube_commits.borrow_mut();
        if changed {
            commits[slot].increment();
        }
        commits[slot]
    }
}

fn untransformed<R: ClarionRenderer>(
    element: MonitorInnerRenderElement<R>,
) -> MonitorRenderElement<R> {
    RelocateRenderElement::from_element(
        RescaleRenderElement::from_element(element, Point::default(), 1.),
        Point::default(),
        Relocate::Relative,
    )
}
