//! Renderer-dependent composition of a borrowed, output-specific scene view.
//!
//! This module has no compositor, event-loop, input dispatch, protocol mutation or
//! device scheduling access. Only the adapter in `Clarion::scene_for_output`
//! projects those owners into the references required to build a frame.

pub mod closing_window;
pub(crate) mod cube_composition;
pub(crate) mod cube_geometry;
mod elements;
pub mod focus_ring;
pub(crate) mod genie;
pub mod insert_hint_element;
mod layers;
pub(crate) mod lifecycle;
pub(crate) mod mapped_resources;
pub(crate) mod monitor_resources;
pub(crate) mod opening_window;
mod pointer;
pub(crate) mod resize_snapshot;
pub mod resources;
pub(crate) mod retained;
pub mod shadow;
pub(crate) mod spread_composition;
pub(crate) mod stage_composition;
pub mod tab_indicator;
pub(crate) mod tile;
pub(crate) mod window_decorations;
pub mod window_resources;
pub(crate) mod window_snapshot;
pub(crate) mod wobble;
pub(crate) mod workspace_composition;
pub(crate) mod workspace_resources;

use clarion_config::debug::PreviewRender;
use elements::scale_relocate_crop;
pub use elements::{OutputRenderElements, PointerRenderElements, WindowScreenshotRenderElement};
pub(crate) use layers::SceneLayers;
pub(crate) use pointer::{SceneDndIcon, ScenePointer};
use smithay::backend::renderer::element::Kind;
use smithay::desktop::layer_map_for_output;
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Point, Scale};
use smithay::wayland::shell::wlr_layer::Layer;

use crate::layout::scene_source::{OutputSceneSource, WindowLookup};
use crate::render_helpers::debug::push_opaque_regions;
use crate::render_helpers::renderer::ClarionRenderer;
use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
use crate::render_helpers::surface::push_elements_from_surface_tree;
use crate::render_helpers::xray::{Xray, XrayPos};
use crate::render_helpers::{RenderCtx, RenderTarget};
use crate::ui::config_error_notification::ConfigErrorNotification;
use crate::ui::exit_confirm_dialog::ExitConfirmDialog;
use crate::ui::hotkey_overlay::HotkeyOverlay;
use crate::ui::mru::WindowMruUi;
use crate::ui::screen_transition::ScreenTransition;
use crate::ui::screenshot_ui::ScreenshotUi;
use crate::window::Mapped;

/// The output-local effect resources. Scheduling and device state are deliberately
/// absent; existing buffer identities and effect caches survive every snapshot.
pub(crate) struct OutputEffects<'a> {
    pub backdrop_buffer: &'a SolidColorBuffer,
    pub overview_shade_buffer: &'a SolidColorBuffer,
    pub lock_color_buffer: &'a SolidColorBuffer,
    pub lock_surface: Option<&'a WlSurface>,
    pub screen_transition: Option<&'a ScreenTransition>,
    pub xray: &'a Xray,
}

pub(crate) struct SceneUi<'a> {
    pub exit_dialog: &'a ExitConfirmDialog,
    pub config_notification: &'a ConfigErrorNotification,
    pub screenshot: &'a ScreenshotUi,
    pub hotkeys: &'a HotkeyOverlay,
    pub window_switcher: &'a WindowMruUi,
}

/// Restricted source view consumed synchronously to import client buffers. The
/// resulting snapshot owns its render elements and borrows no desktop state.
pub(crate) struct OutputScene<'a> {
    pub output: &'a Output,
    pub sources: OutputSceneSource<'a, Mapped>,
    pub windows: &'a dyn WindowLookup<Mapped>,
    pub layers: SceneLayers<'a>,
    pub effects: OutputEffects<'a>,
    pub ui: SceneUi<'a>,
    pub pointer: ScenePointer<'a>,
    pub locked: bool,
    pub transitions_allowed: bool,
    pub debug_draw_opaque_regions: bool,
    pub preview_render: Option<PreviewRender>,
}

/// Frozen render elements, shared by display, capture and headless backends.
/// No accessor permits mutation after scene preparation.
pub(crate) struct SceneSnapshot<R: ClarionRenderer> {
    elements: Vec<OutputRenderElements<R>>,
}

impl<R: ClarionRenderer> SceneSnapshot<R> {
    pub fn elements(&self) -> &[OutputRenderElements<R>] {
        &self.elements
    }

    pub(crate) fn from_elements(elements: Vec<OutputRenderElements<R>>) -> Self {
        Self { elements }
    }
}

/// The sole renderer-dependent operation available to a device frame consumer.
pub(crate) trait ScenePreparation {
    fn prepare_scene<R: ClarionRenderer>(&mut self, renderer: &mut R) -> SceneSnapshot<R>;
}

impl OutputScene<'_> {
    pub fn render_to_vec<R: ClarionRenderer>(
        &self,
        ctx: RenderCtx<R>,
        include_pointer: bool,
    ) -> Vec<OutputRenderElements<R>> {
        // Internal scene preparation and capture completion retain VkFence
        // resources directly. Device adapters explicitly require sync-file
        // export for their final display submission.
        if let Some(native) = ctx.renderer.as_native_renderer() {
            native.set_completion_export_required(false);
        }

        let mut elements = Vec::new();
        self.render(ctx, include_pointer, &mut |elem| elements.push(elem));
        elements
    }

    pub fn render<R: ClarionRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        include_pointer: bool,
        push: &mut dyn FnMut(OutputRenderElements<R>),
    ) {
        let _span = clarion_tracy_span!("Clarion::render");

        if ctx.target == RenderTarget::Output {
            if let Some(preview) = self.preview_render {
                ctx.target = match preview {
                    PreviewRender::Screencast => RenderTarget::Screencast,
                    PreviewRender::ScreenCapture => RenderTarget::ScreenCapture,
                };
            }
        }

        self.fill_xray_elements(ctx.r());

        // Reborrow to shorten lifetime to be able to put in xray.
        let mut ctx = ctx.r();
        ctx.xray = Some(self.effects.xray);

        self.render_inner(ctx, include_pointer, push);

        self.clear_xray_elements();
    }

    fn render_inner<R: ClarionRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        include_pointer: bool,
        push: &mut dyn FnMut(OutputRenderElements<R>),
    ) {
        let output = self.output;
        let state = &self.effects;
        let output_scale = Scale::from(output.current_scale().fractional_scale());

        let push = if self.debug_draw_opaque_regions {
            &mut move |elem| {
                push_opaque_regions(&elem, output_scale, push);
                push(elem);
            }
        } else {
            push
        };

        use clarion_scene::{SceneInputs, ScenePass, ScenePlan};
        let layer_map = layer_map_for_output(output);
        let mon = self.sources.monitor();
        let overview_progress = mon.map(|mon| mon.overview_progress()).unwrap_or(0.);
        let plan = ScenePlan::new(SceneInputs {
            desktop_available: mon.is_some(),
            pointer: include_pointer && self.pointer.visible,
            screen_transition: self.transitions_allowed && state.screen_transition.is_some(),
            locked: self.locked,
            screenshot_ui: self.ui.screenshot.is_open(),
            desktop_above_top: mon.is_some_and(|mon| mon.desktop_above_top()),
            stage: mon.is_some_and(|mon| mon.stage()),
            overview_backdrop: overview_progress > 0.,
            explicit_backdrop_wallpaper: self
                .layers
                .in_render_order(&layer_map, Layer::Background, true)
                .next()
                .is_some(),
        });
        let mut backdrop: Option<OutputRenderElements<R>> = Some(
            SolidColorRenderElement::from_buffer(
                state.backdrop_buffer,
                (0., 0.),
                1.,
                Kind::Unspecified,
            )
            .into(),
        );
        let focus_ring = self.sources.focus_ring();
        macro_rules! push_popups_from_layer {
            ($layer:expr, $ns:expr, $xray_pos:expr, $backdrop:expr, $push:expr) => {{
                self.layers.render_popups(
                    ctx.r(),
                    $ns,
                    &layer_map,
                    $layer,
                    $xray_pos,
                    $backdrop,
                    $push,
                );
            }};
            ($layer:expr, true) => {{
                push_popups_from_layer!($layer, None, XrayPos::default(), true, &mut |elem| push(
                    elem.into()
                ));
            }};
            ($layer:expr, $ns:expr, $xray_pos:expr, $push:expr) => {{
                push_popups_from_layer!($layer, $ns, $xray_pos, false, $push);
            }};
            ($layer:expr) => {{
                push_popups_from_layer!($layer, None, XrayPos::default(), false, &mut |elem| push(
                    elem.into()
                ));
            }};
        }
        macro_rules! push_normal_from_layer {
            ($layer:expr, $ns:expr, $xray_pos:expr, $backdrop:expr, $push:expr) => {{
                self.layers.render_normal(
                    ctx.r(),
                    $ns,
                    &layer_map,
                    $layer,
                    $xray_pos,
                    $backdrop,
                    $push,
                );
            }};
            ($layer:expr, true) => {{
                push_normal_from_layer!($layer, None, XrayPos::default(), true, &mut |elem| {
                    push(elem.into())
                });
            }};
            ($layer:expr, $ns:expr, $xray_pos:expr, $push:expr) => {{
                push_normal_from_layer!($layer, $ns, $xray_pos, false, $push);
            }};
            ($layer:expr) => {{
                push_normal_from_layer!($layer, None, XrayPos::default(), false, &mut |elem| {
                    push(elem.into())
                });
            }};
        }

        macro_rules! process {
            ($geo:expr, $zoom:expr) => {{
                &mut |elem| {
                    if let Some(elem) = scale_relocate_crop(elem, output_scale, $zoom, $geo) {
                        push(elem.into());
                    }
                }
            }};
        }
        for pass in plan.passes() {
            match pass {
                ScenePass::Pointer => {
                    self.pointer
                        .render(ctx.renderer, output, &mut |elem| push(elem.into()));
                }
                ScenePass::ScreenTransition => {
                    if let Some(transition) = &state.screen_transition {
                        push(transition.render(ctx.target).into());
                    }
                }
                ScenePass::ExitDialog => {
                    self.ui
                        .exit_dialog
                        .render(ctx.renderer, output, &mut |elem| push(elem.into()));
                }
                ScenePass::ConfigNotification => {
                    if let Some(element) = self.ui.config_notification.render(ctx.renderer, output)
                    {
                        push(element.into());
                    }
                }
                ScenePass::LockSurface => {
                    if let Some(surface) = state.lock_surface {
                        push_elements_from_surface_tree(
                            ctx.renderer,
                            surface,
                            Point::new(0, 0),
                            output_scale,
                            1.,
                            Kind::ScanoutCandidate,
                            &mut |elem| push(elem.into()),
                        );
                    }
                }
                ScenePass::LockBackdrop => {
                    push(
                        SolidColorRenderElement::from_buffer(
                            state.lock_color_buffer,
                            (0., 0.),
                            1.,
                            Kind::Unspecified,
                        )
                        .into(),
                    );
                }
                ScenePass::ScreenshotUi => {
                    self.ui
                        .screenshot
                        .render_output(output, ctx.target, &mut |elem| push(elem.into()));
                }
                ScenePass::HotkeyOverlay => {
                    if let Some(element) = self.ui.hotkeys.render(ctx.renderer, output) {
                        push(element.into());
                    }
                }
                ScenePass::WindowSwitcher => {
                    self.ui.window_switcher.render_output(
                        self.windows,
                        output,
                        ctx.r(),
                        &mut |elem| push(elem.into()),
                    );
                }
                ScenePass::OverlayLayer => {
                    push_popups_from_layer!(Layer::Overlay);
                    push_normal_from_layer!(Layer::Overlay);
                }
                ScenePass::TopLayer => {
                    push_popups_from_layer!(Layer::Top);
                    push_normal_from_layer!(Layer::Top);
                }
                ScenePass::MovingWindow => {
                    self.sources
                        .render_moving(ctx.r(), &mut |elem| push(elem.into()));
                }
                ScenePass::InsertHint => {
                    let mon = mon.unwrap();
                    mon.render_insert_hint(ctx.renderer, &mut |elem| push(elem.into()));
                }
                ScenePass::Workspaces => {
                    let mon = mon.unwrap();
                    mon.render_workspaces(ctx.r(), focus_ring, &mut |elem| push(elem.into()));
                }
                ScenePass::LowerLayerPopups => {
                    push_popups_from_layer!(Layer::Bottom);
                    push_popups_from_layer!(Layer::Background);
                }
                ScenePass::LowerLayers => {
                    push_normal_from_layer!(Layer::Bottom);
                    push_normal_from_layer!(Layer::Background);
                }
                ScenePass::WorkspaceBackground => {
                    let mon = mon.unwrap();
                    if let Some(workspace) = mon.workspaces().first() {
                        push(workspace.background.clone().into());
                    }
                }
                ScenePass::WorkspaceLayerPopups => {
                    let mon = mon.unwrap();
                    for workspace in mon.workspaces() {
                        let geo = workspace.geometry;
                        let ns = Some(workspace.namespace);
                        let zoom = workspace.zoom;
                        let xray_pos = workspace.xray_position();
                        push_popups_from_layer!(Layer::Bottom, ns, xray_pos, process!(geo, zoom));
                        push_popups_from_layer!(
                            Layer::Background,
                            ns,
                            xray_pos,
                            process!(geo, zoom)
                        );
                    }
                }
                ScenePass::StageWithLayers => {
                    let mon = mon.unwrap();
                    mon.render_stage(
                        ctx.r(),
                        focus_ring,
                        &mut |mut ctx, id, geo, popups, card_push| {
                            let ns = Some(id.get() as usize);
                            let zoom = geo.size.w / mon.width().max(f64::EPSILON);
                            let xray_pos = XrayPos::new(geo.loc, zoom);
                            for layer in [Layer::Bottom, Layer::Background] {
                                if popups {
                                    self.layers.render_popups(
                                        ctx.r(),
                                        ns,
                                        &layer_map,
                                        layer,
                                        xray_pos,
                                        false,
                                        card_push,
                                    );
                                } else {
                                    self.layers.render_normal(
                                        ctx.r(),
                                        ns,
                                        &layer_map,
                                        layer,
                                        xray_pos,
                                        false,
                                        card_push,
                                    );
                                }
                            }
                        },
                        &mut |elem| push(elem.into()),
                    );
                }
                ScenePass::WorkspaceLowerLayers => {
                    let mon = mon.unwrap();
                    for workspace in mon.workspaces() {
                        let geo = workspace.geometry;
                        // The render element namespace. This will be set to the workspace index for
                        // elements duplicated across workspaces (i.e. background and bottom layers)
                        // in order to have their non-xray framebuffer
                        // effects separated from each other.
                        //
                        // This doesn't have to correspond exactly to workspace id or idx, the only
                        // requirement is that there's only one framebuffer effect element with a
                        // given id + namespace on the frame at once. Id +
                        // namespace is used as the cache key in the
                        // damage tracker.
                        let ns = Some(workspace.namespace);
                        let zoom = workspace.zoom;
                        let xray_pos = workspace.xray_position();
                        push_normal_from_layer!(Layer::Bottom, ns, xray_pos, process!(geo, zoom));
                        push_normal_from_layer!(
                            Layer::Background,
                            ns,
                            xray_pos,
                            process!(geo, zoom)
                        );

                        process!(geo, zoom)(workspace.background.clone());
                    }
                }
                ScenePass::WorkspaceShadows => {
                    let mon = mon.unwrap();
                    mon.render_shadows(ctx.renderer, &mut |elem| push(elem.into()));
                }
                ScenePass::OverviewShade => {
                    push(
                        SolidColorRenderElement::from_buffer(
                            state.overview_shade_buffer,
                            (0., 0.),
                            (0.25 * overview_progress) as f32,
                            Kind::Unspecified,
                        )
                        .into(),
                    );
                }
                ScenePass::BackdropLayers => {
                    push_popups_from_layer!(Layer::Background, true);
                    push_normal_from_layer!(Layer::Background, true);
                }
                ScenePass::WallpaperFallback => {
                    push_normal_from_layer!(Layer::Background);
                }
                ScenePass::Backdrop => {
                    push(backdrop.take().expect("scene backdrop pass appears once"));
                }
            }
        }
    }

    pub(crate) fn fill_xray_elements<R: ClarionRenderer>(&self, ctx: RenderCtx<R>) {
        self.layers
            .fill_xray_elements(self.output, self.effects.xray, ctx);
    }

    pub(crate) fn clear_xray_elements(&self) {
        clear_xray_elements(self.effects.xray);
    }
}

pub(crate) fn clear_xray_elements(xray: &Xray) {
    // Release imports after every target, retaining pooled buffer capacity.
    for buffer in xray.background.iter().chain(&xray.backdrop) {
        let mut buffer = buffer.borrow_mut();
        buffer.elements().clear();
        buffer.elements_native().clear();
    }
}
