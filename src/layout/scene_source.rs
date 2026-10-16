//! Read-only render capabilities projected by the desktop owner.
//!
//! A source captures one output's geometry and flags, not a second desktop model.
//! Private references allow reuse of retained renderer caches without exposing
//! the complete layout, core, focus operations or other monitors to composition.

use smallvec::SmallVec;
use smithay::backend::renderer::element::utils::RescaleRenderElement;
use smithay::backend::renderer::Color32F;
use smithay::output::Output;
use smithay::utils::{Logical, Point, Rectangle, Scale};

use super::monitor::Monitor;
use super::tile::{Tile, TileRenderElement};
use super::workspace::WorkspaceId;
use super::{InteractiveMoveState, Layout, LayoutElement, MonitorRenderElement};
use crate::layer::mapped::LayerSurfaceRenderElement;
use crate::render_helpers::renderer::ClarionRenderer;
use crate::render_helpers::solid_color::SolidColorRenderElement;
use crate::render_helpers::xray::XrayPos;
use crate::render_helpers::RenderCtx;

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

pub struct MonitorScene<'a, W: LayoutElement> {
    monitor: &'a Monitor<W>,
    workspaces: SmallVec<[WorkspaceScene; 2]>,
    desktop_above_top: bool,
    stage: bool,
    overview_progress: f64,
    width: f64,
}

impl<W: LayoutElement> MonitorScene<'_, W> {
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
        self.monitor
            .render_insert_hint_between_workspaces(renderer, push);
    }

    pub fn render_workspaces<R: ClarionRenderer>(
        &self,
        ctx: RenderCtx<R>,
        focus_ring: bool,
        push: &mut dyn FnMut(MonitorRenderElement<R>),
    ) {
        self.monitor.render_workspaces(ctx, focus_ring, push);
    }

    pub fn render_stage<R: ClarionRenderer>(
        &self,
        ctx: RenderCtx<R>,
        focus_ring: bool,
        layers: &mut StageLayers<'_, R>,
        push: &mut dyn FnMut(MonitorRenderElement<R>),
    ) {
        self.monitor
            .render_stage_with_layers(ctx, focus_ring, layers, push);
    }

    pub fn render_shadows<R: ClarionRenderer>(
        &self,
        renderer: &mut R,
        push: &mut dyn FnMut(MonitorRenderElement<R>),
    ) {
        self.monitor.render_workspace_shadows(renderer, push);
    }
}

struct MovingTileScene<'a, W: LayoutElement> {
    tile: &'a Tile<W>,
    position: Point<f64, Logical>,
    scale: Scale<f64>,
    zoom: f64,
}

/// The scene can render this output's sources, never recover their layout owner.
pub struct OutputSceneSource<'a, W: LayoutElement> {
    monitor: Option<MonitorScene<'a, W>>,
    moving: Option<MovingTileScene<'a, W>>,
}

impl<W: LayoutElement> OutputSceneSource<'_, W> {
    /// Privacy routes do not acquire desktop or interaction sources at all.
    pub fn empty() -> Self {
        Self {
            monitor: None,
            moving: None,
        }
    }

    pub fn monitor(&self) -> Option<&MonitorScene<'_, W>> {
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
            // Stage renders complete cards in its own retained renderer. Only
            // ordinary duplicated lower-layer passes need these descriptors.
            let workspaces = if stage && !desktop_above_top {
                SmallVec::new()
            } else {
                monitor
                    .workspaces_with_render_geo()
                    .map(|(workspace, geometry)| WorkspaceScene {
                        namespace: workspace.id().get() as usize,
                        geometry,
                        zoom: monitor.workspace_render_zoom(geometry),
                        background: workspace.render_background(),
                    })
                    .collect()
            };
            MonitorScene {
                monitor,
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
                    tile: &moving.tile,
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
