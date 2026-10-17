//! Read-only render capabilities projected by the desktop owner.
//!
//! A source captures one output's geometry and flags, not a second desktop model.
//! Private references allow reuse of retained renderer caches without exposing
//! the complete layout, core, focus operations or other monitors to composition.

use smallvec::SmallVec;
use smithay::backend::renderer::element::utils::{
    Relocate, RelocateRenderElement, RescaleRenderElement,
};
use smithay::backend::renderer::Color32F;
use smithay::output::Output;
use smithay::utils::{Logical, Point, Rectangle, Scale};

use super::monitor::MonitorInnerRenderElement;
use super::tile::{Tile, TileRenderElement};
use super::workspace::WorkspaceId;
use super::{InteractiveMoveState, Layout, LayoutElement, MonitorRenderElement};
use crate::layer::mapped::LayerSurfaceRenderElement;
use crate::render_helpers::offscreen::OffscreenData;
use crate::render_helpers::renderer::ClarionRenderer;
use crate::render_helpers::shadow::ShadowRenderElement;
use crate::render_helpers::solid_color::SolidColorRenderElement;
use crate::render_helpers::xray::XrayPos;
use crate::render_helpers::RenderCtx;
use crate::scene::closing_window::{ClosingWindow, ClosingWindowRenderElement};
use crate::scene::cube_composition::{CubeComposition, CubeLifecycle};
use crate::scene::insert_hint_element::InsertHintRenderElement;
use crate::scene::retained::ResourceLease;
use crate::scene::spread_composition::SpreadComposition;
use crate::scene::stage_composition::StageComposition;
use crate::scene::tab_indicator::{TabIndicator, TabIndicatorRenderElement};
use crate::scene::workspace_composition::Composition;
use crate::scene::workspace_resources::WorkspaceResources;

/// Sampling feedback is the sole operation available to offscreen composition.
/// The scene cannot recover the window or traverse its desktop owner.
pub(crate) struct WindowFeedbackSource<'a, W: LayoutElement> {
    window: &'a W,
}

impl<'a, W: LayoutElement> WindowFeedbackSource<'a, W> {
    pub(crate) fn new(window: &'a W) -> Self {
        Self { window }
    }

    pub(crate) fn set_offscreen_data(&self, data: Option<OffscreenData>) {
        self.window.set_offscreen_data(data);
    }
}

/// MRU needs a window's render source, never desktop traversal or focus state.
pub trait WindowLookup<W: LayoutElement> {
    fn window(&self, core_id: u64) -> Option<&W>;
}

impl<W: LayoutElement> WindowLookup<W> for Layout<W> {
    fn window(&self, core_id: u64) -> Option<&W> {
        self.windows()
            .find_map(|(_, window)| (window.core_id() == core_id).then_some(window))
    }
}

/// Stable geometry/identity for duplicated lower-layer effect namespaces.
/// Captured once per output preparation and reused by popup and normal passes.
pub struct WorkspaceScene {
    pub namespace: usize,
    pub geometry: Rectangle<f64, Logical>,
    pub zoom: f64,
    pub background: SolidColorRenderElement,
}

impl WorkspaceScene {
    pub fn xray_position(&self) -> XrayPos {
        XrayPos::new(self.geometry.loc, self.zoom)
    }
}

pub type StageLayers<'a, R> = dyn FnMut(
        RenderCtx<R>,
        WorkspaceId,
        Rectangle<f64, Logical>,
        bool,
        &mut dyn FnMut(LayerSurfaceRenderElement<R>),
    ) + 'a;

/// A deliberately narrow remaining window presentation capability. It cannot
/// expose a layout owner; migrating Tile's internal effects is a separate step.
pub(crate) struct TileRenderSource<'a, W: LayoutElement> {
    scene: crate::scene::tile::TileScene<'a, W>,
}
impl<'a, W: LayoutElement> TileRenderSource<'a, W> {
    pub(crate) fn new(tile: &'a Tile<W>) -> Self {
        Self {
            scene: tile.scene(),
        }
    }
    pub(crate) fn render<R: ClarionRenderer>(
        &self,
        ctx: RenderCtx<R>,
        position: Point<f64, Logical>,
        xray: XrayPos,
        focused: bool,
        push: &mut dyn FnMut(TileRenderElement<R>),
    ) {
        self.scene.render(ctx, position, xray, focused, push);
    }
    pub(crate) fn render_genie<R: ClarionRenderer>(
        &self,
        ctx: RenderCtx<R>,
        xray: XrayPos,
        push: &mut dyn FnMut(TileRenderElement<R>),
    ) -> bool {
        self.scene.render_genie(ctx, xray, push)
    }
    #[cfg(test)]
    pub(crate) fn window(&self) -> &W {
        self.scene.window()
    }
}
pub(crate) struct ClosingRenderSource<'a> {
    closing: &'a ClosingWindow,
}
impl<'a> ClosingRenderSource<'a> {
    pub(crate) fn new(closing: &'a ClosingWindow) -> Self {
        Self { closing }
    }
    pub(crate) fn render<R: ClarionRenderer>(
        &self,
        ctx: RenderCtx<R>,
        view: Rectangle<f64, Logical>,
        scale: Scale<f64>,
    ) -> Vec<ClosingWindowRenderElement> {
        self.closing.render(ctx, view, scale)
    }
}
pub(crate) struct TabRenderSource<'a> {
    indicator: &'a TabIndicator,
}
impl<'a> TabRenderSource<'a> {
    pub(crate) fn new(indicator: &'a TabIndicator) -> Self {
        Self { indicator }
    }
    pub(crate) fn render<R: ClarionRenderer>(
        &self,
        renderer: &mut R,
        position: Point<f64, Logical>,
        push: &mut dyn FnMut(TabIndicatorRenderElement),
    ) {
        self.indicator.render(renderer, position, push);
    }
}
pub(crate) struct ShadowRenderSource<'a> {
    resources: ResourceLease<WorkspaceResources>,
    _phantom: std::marker::PhantomData<&'a ()>,
}
impl<'a> ShadowRenderSource<'a> {
    pub(crate) fn new(resources: ResourceLease<WorkspaceResources>) -> Self {
        Self {
            resources,
            _phantom: std::marker::PhantomData,
        }
    }
    pub(crate) fn render<R: ClarionRenderer>(
        &self,
        renderer: &mut R,
        push: &mut dyn FnMut(ShadowRenderElement),
    ) {
        self.resources
            .shadow
            .borrow()
            .render(renderer, Point::default(), push);
    }
}
pub(crate) struct InsertHintRenderSource<'a> {
    resources: ResourceLease<crate::scene::monitor_resources::MonitorResources>,
    _phantom: std::marker::PhantomData<&'a ()>,
}
impl<'a> InsertHintRenderSource<'a> {
    pub(crate) fn new(
        resources: ResourceLease<crate::scene::monitor_resources::MonitorResources>,
    ) -> Self {
        Self {
            resources,
            _phantom: std::marker::PhantomData,
        }
    }
    pub(crate) fn render<R: ClarionRenderer>(
        &self,
        renderer: &mut R,
        position: Point<f64, Logical>,
        push: &mut dyn FnMut(InsertHintRenderElement),
    ) {
        self.resources
            .insert_hint_element
            .borrow()
            .render(renderer, position, push);
    }
}

/// Exactly one captured presentation, kept inline without per-frame allocation.
/// Mutually exclusive modes share storage instead of carrying every mode's
/// inactive composition through the output preparation and drawing passes.
enum Presentation<'a, W: LayoutElement> {
    Ordinary(Composition<'a, W>),
    Stage(StageComposition<'a, W>),
    Spread(SpreadComposition<'a, W>),
    Cube(CubeComposition<'a, W>),
}

pub struct MonitorScene<'a, W: LayoutElement> {
    presentation: Presentation<'a, W>,
    cube_lifecycle: Option<CubeLifecycle<'a, W>>,
    between_hint: Option<(InsertHintRenderSource<'a>, Point<f64, Logical>)>,
    workspaces: SmallVec<[WorkspaceScene; 2]>,
    desktop_above_top: bool,
    stage: bool,
    overview_progress: f64,
    width: f64,
}

impl<'a, W: LayoutElement> MonitorScene<'a, W> {
    #[cfg(test)]
    pub(crate) fn ordinary_composition(&self) -> Option<&Composition<'a, W>> {
        match &self.presentation {
            Presentation::Ordinary(composition) => Some(composition),
            _ => None,
        }
    }

    pub fn desktop_above_top(&self) -> bool {
        self.desktop_above_top
    }
    pub fn stage(&self) -> bool {
        self.stage
    }
    pub fn overview_progress(&self) -> f64 {
        self.overview_progress
    }
    pub fn width(&self) -> f64 {
        self.width
    }
    pub fn workspaces(&self) -> &[WorkspaceScene] {
        &self.workspaces
    }

    pub fn render_insert_hint<R: ClarionRenderer>(
        &self,
        renderer: &mut R,
        push: &mut dyn FnMut(MonitorRenderElement<R>),
    ) {
        if let Some((source, position)) = &self.between_hint {
            source.render(renderer, *position, &mut |element| {
                let element = MonitorInnerRenderElement::UncroppedInsertHint(element);
                push(RelocateRenderElement::from_element(
                    RescaleRenderElement::from_element(element, Point::default(), 1.),
                    Point::default(),
                    Relocate::Relative,
                ));
            });
        }
    }

    pub fn render_workspaces<R: ClarionRenderer>(
        &self,
        ctx: RenderCtx<R>,
        focus_ring: bool,
        push: &mut dyn FnMut(MonitorRenderElement<R>),
    ) {
        if let Some(lifecycle) = &self.cube_lifecycle {
            lifecycle.leave_cube();
        }
        match &self.presentation {
            Presentation::Ordinary(composition) => composition.render(ctx, focus_ring, push),
            Presentation::Stage(composition) => {
                composition.render(ctx, focus_ring, &mut |_, _, _, _, _| {}, push)
            }
            Presentation::Spread(composition) => composition.render(ctx, push),
            Presentation::Cube(composition) => composition.render(ctx, focus_ring, push),
        }
    }

    pub fn render_stage<R: ClarionRenderer>(
        &self,
        ctx: RenderCtx<R>,
        focus_ring: bool,
        layers: &mut StageLayers<'_, R>,
        push: &mut dyn FnMut(MonitorRenderElement<R>),
    ) {
        if let Presentation::Stage(stage) = &self.presentation {
            if let Some(lifecycle) = &self.cube_lifecycle {
                lifecycle.leave_cube();
            }
            stage.render(ctx, focus_ring, layers, push);
        }
    }

    pub fn render_shadows<R: ClarionRenderer>(
        &self,
        renderer: &mut R,
        push: &mut dyn FnMut(MonitorRenderElement<R>),
    ) {
        match &self.presentation {
            Presentation::Ordinary(composition) => composition.render_shadows(renderer, push),
            Presentation::Spread(composition) => composition.render_shadows(renderer, push),
            Presentation::Stage(_) | Presentation::Cube(_) => {}
        }
    }
}

struct MovingTileScene<'a, W: LayoutElement> {
    tile: crate::scene::tile::TileScene<'a, W>,
    position: Point<f64, Logical>,
    scale: Scale<f64>,
    zoom: f64,
}

/// The scene can render this output's sources, never recover their layout owner.
pub struct OutputSceneSource<'a, W: LayoutElement> {
    monitor: Option<MonitorScene<'a, W>>,
    moving: Option<MovingTileScene<'a, W>>,
}

impl<'a, W: LayoutElement> OutputSceneSource<'a, W> {
    /// Privacy routes do not acquire desktop or interaction sources at all.
    pub fn empty() -> Self {
        Self {
            monitor: None,
            moving: None,
        }
    }

    pub fn monitor(&self) -> Option<&MonitorScene<'a, W>> {
        self.monitor.as_ref()
    }
    pub fn focus_ring(&self) -> bool {
        self.moving.is_none()
    }

    pub fn render_moving<R: ClarionRenderer>(
        &self,
        ctx: RenderCtx<R>,
        push: &mut dyn FnMut(RescaleRenderElement<TileRenderElement<R>>),
    ) {
        let Some(moving) = &self.moving else {
            return;
        };
        moving.tile.render(
            ctx,
            moving.position,
            XrayPos::new(moving.position, moving.zoom),
            true,
            &mut |element| {
                push(RescaleRenderElement::from_element(
                    element,
                    moving.position.to_physical_precise_round(moving.scale),
                    moving.zoom,
                ))
            },
        );
    }
}

impl<W: LayoutElement> Layout<W> {
    pub fn scene_source(&self, output: &Output) -> OutputSceneSource<'_, W> {
        if self.update_render_elements_time != self.clock.now() {
            error!("clock moved between updating render elements and rendering");
        }
        let monitor = self.monitor_for_output(output).map(|monitor| {
            let desktop_above_top = monitor.render_above_top_layer();
            let stage = monitor.is_stage_visible();
            let presentation = if stage {
                Presentation::Stage(monitor.stage_composition())
            } else if monitor.is_spread_scene() {
                Presentation::Spread(monitor.spread_composition())
            } else if monitor.is_cube_scene() {
                Presentation::Cube(monitor.cube_composition())
            } else {
                Presentation::Ordinary(monitor.standard_composition())
            };
            let cube_lifecycle =
                (!matches!(&presentation, Presentation::Cube(_))).then(|| monitor.cube_lifecycle());
            let workspaces = if stage && !desktop_above_top {
                SmallVec::new()
            } else {
                match &presentation {
                    Presentation::Ordinary(captured) => captured
                        .geometry
                        .placements
                        .iter()
                        .zip(&captured.workspaces)
                        .filter(|(placement, _)| placement.normal_visible)
                        .map(|(placement, sources)| WorkspaceScene {
                            namespace: placement.id.get() as usize,
                            geometry: placement.geometry,
                            zoom: placement.zoom,
                            background: sources.background.clone(),
                        })
                        .collect(),
                    Presentation::Stage(captured) => captured.workspace_scenes().collect(),
                    Presentation::Spread(captured) => captured.workspace_scenes().collect(),
                    Presentation::Cube(captured) => captured.workspace_scenes().collect(),
                }
            };
            MonitorScene {
                presentation,
                cube_lifecycle,
                between_hint: monitor.scene_insert_hint_between(),
                workspaces,
                desktop_above_top,
                stage,
                overview_progress: monitor.overview_backdrop_progress(),
                width: monitor.view_size().w,
            }
        });
        let moving = match &self.interactive_move {
            Some(InteractiveMoveState::Moving(moving)) if moving.output == *output => {
                let zoom = self.interactive_move_zoom();
                Some(MovingTileScene {
                    tile: moving.tile.scene(),
                    position: moving.tile_render_location(zoom),
                    scale: output.current_scale().fractional_scale().into(),
                    zoom,
                })
            }
            _ => None,
        };
        OutputSceneSource { monitor, moving }
    }

    /// Pure geometry/color input to effect resource updates. No GPU or desktop
    /// owner enters the scene lifecycle module through this iterator.
    pub(crate) fn scene_backgrounds(
        &self,
        output: &Output,
    ) -> impl Iterator<Item = (Rectangle<f64, Logical>, Color32F)> + '_ {
        self.monitor_for_output(output)
            .into_iter()
            .flat_map(|monitor| {
                monitor
                    .workspaces_with_render_geo()
                    .map(|(workspace, geometry)| (geometry, workspace.render_background().color()))
            })
    }
}

#[cfg(test)]
mod presentation_storage_tests {
    use std::mem::{align_of, size_of};

    use super::*;
    use crate::window::mapped::Mapped;

    #[test]
    fn output_scene_keeps_only_one_inline_presentation_payload() {
        let alternatives = [
            size_of::<Composition<'static, Mapped>>(),
            size_of::<StageComposition<'static, Mapped>>(),
            size_of::<SpreadComposition<'static, Mapped>>(),
            size_of::<CubeComposition<'static, Mapped>>(),
        ];
        let largest = alternatives.into_iter().max().unwrap();
        let presentation = size_of::<Presentation<'static, Mapped>>();
        let monitor = size_of::<MonitorScene<'static, Mapped>>();
        let previous_options = size_of::<Option<Composition<'static, Mapped>>>()
            + size_of::<Option<StageComposition<'static, Mapped>>>()
            + size_of::<Option<SpreadComposition<'static, Mapped>>>()
            + size_of::<Option<CubeComposition<'static, Mapped>>>();
        // A pointer/box would also be small but would restore a per-frame
        // allocation; the active payload must fit inside the enum itself.
        assert!(presentation >= largest);
        assert!(presentation <= largest + align_of::<Presentation<'static, Mapped>>());
        // Even with all shared descriptors the new scene should remain smaller
        // than the old mutually exclusive payloads alone.
        assert!(monitor < previous_options);
        eprintln!(
            "scene storage bytes: alternatives={alternatives:?}, presentation={presentation}, \
             monitor={monitor}, previous_options={previous_options}"
        );
    }
}
