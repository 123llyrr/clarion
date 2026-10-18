//! Window layout logic.
//!
//! Clarion implements scrollable tiling with dynamic workspaces. The scrollable tiling is mostly
//! orthogonal to any particular workspace system, though outputs living in separate coordinate
//! spaces suggest per-output workspaces.
//!
//! I chose a dynamic workspace system because I think it works very well. In particular, it works
//! naturally across outputs getting added and removed, since workspaces can move between outputs
//! as necessary.
//!
//! In the layout, one output (the first one to be added) is designated as *primary*. This is where
//! workspaces from disconnected outputs will move. Currently, the primary output has no other
//! distinction from other outputs.
//!
//! Where possible, clarion tries to follow these principles with regards to outputs:
//!
//! 1. Disconnecting and reconnecting the same output must not change the layout.
//!    * This includes both secondary outputs and the primary output.
//! 2. Connecting an output must not change the layout for any workspaces that were never on that
//!    output.
//!
//! Therefore, we implement the following logic: every workspace keeps track of which output it
//! originated on—its *original output*. When an output disconnects, its workspaces are appended to
//! the (potentially new) primary output, but remember their original output. Then, if the original
//! output connects again, all workspaces originally from there move back to that output.
//!
//! In order to avoid surprising behavior, if the user creates or moves any new windows onto a
//! workspace, it forgets its original output, and its current output becomes its original output.
//! Imagine a scenario: the user works with a laptop and a monitor at home, then takes their laptop
//! with them, disconnecting the monitor, and keeps working as normal, using the second monitor's
//! workspace just like any other. Then they come back, reconnect the second monitor, and now we
//! don't want an unassuming workspace to end up on it.

use std::mem;
use std::rc::Rc;
use std::time::Duration;

use clarion_config::utils::MergeWith as _;
use clarion_config::{
    Config, CornerRadius, LayoutPart, PresetSize, Workspace as WorkspaceConfig, WorkspaceReference,
};
use clarion_ipc::{ColumnDisplay, PositionChange, SizeChange, WindowLayout};
use monitor::{InsertHint, InsertPosition, InsertWorkspace, MonitorAddWindowTarget};
use scrolling::{Column, ColumnWidth};
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::output::{self, Output};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle, Scale, Serial, Size, Transform};
use tile::Tile;
use workspace::{WorkspaceAddWindowTarget, WorkspaceId};

pub use self::monitor::MonitorRenderElement;
use self::monitor::{Monitor, WorkspaceSwitch};
use self::wobble::WobbleAnchor;
use self::workspace::Workspace;
use crate::animation::{Animation, Clock};
use crate::clarion_render_elements;
use crate::layout::scrolling::ScrollDirection;
use crate::render_helpers::background_effect::BackgroundEffectElement;
use crate::render_helpers::offscreen::OffscreenData;
use crate::render_helpers::renderer::{AnyRenderer, ClarionRenderer};
use crate::render_helpers::snapshot::RenderSnapshot;
use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
use crate::render_helpers::texture::TextureBuffer;
use crate::render_helpers::xray::{Xray, XrayPos};
use crate::render_helpers::{BakedBuffer, RenderCtx};
use crate::scene::resources::SceneResourceAllocator;
use crate::utils::transaction::{Transaction, TransactionBlocker};
use crate::utils::{
    ensure_min_max_size_maybe_zero, output_matches_name, output_size,
    round_logical_in_physical_max1, ResizeEdge,
};
use crate::window::ResolvedWindowRules;

mod core_adapter;
pub(crate) use core_adapter::CoreProtocolOp;
pub mod dwindle;
pub mod floating;
mod management;
pub mod minimized;
pub mod monitor;
pub mod opening_window;
pub mod scene_source;
pub mod scrolling;
mod stage_drag;
pub mod tile;
pub(crate) mod window_presentation;
pub(crate) mod wobble;
pub mod workspace;

#[cfg(test)]
mod tests;

/// Size changes up to this many pixels don't animate.
pub const RESIZE_ANIMATION_THRESHOLD: f64 = 10.;

/// Opacity of interactively moved tiles targeting the scrolling layout.
const INTERACTIVE_MOVE_ALPHA: f64 = 0.75;

/// Size-relative units.
pub struct SizeFrac;

clarion_render_elements! {
    LayoutElementRenderElement<R> => {
        Wayland = WaylandSurfaceRenderElement<R>,
        SolidColor = SolidColorRenderElement,
        BackgroundEffect = BackgroundEffectElement,
    }
}

pub type LayoutElementRenderSnapshot<T = GlesTexture> =
    RenderSnapshot<BakedBuffer<TextureBuffer<T>>, BakedBuffer<SolidColorBuffer>, T>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizingMode {
    Normal,
    Maximized,
    Fullscreen,
}

pub trait LayoutElement {
    /// Type that can be used as a unique ID of this element.
    type Id: Eq + std::hash::Hash + std::fmt::Debug + Clone;

    /// Unique ID of this element.
    fn id(&self) -> &Self::Id;

    /// Stable numeric desktop identity. The independent core never receives
    /// protocol objects or GPU handles through its window ID type.
    fn core_id(&self) -> u64;

    /// Application and title, used for search and matching explicitly saved sessions.
    fn session_identity(&self) -> (String, String) {
        (String::new(), String::new())
    }

    /// Updates the config for the element.
    fn update_config(&mut self, blur_config: clarion_config::Blur) {
        let _ = blur_config;
    }

    /// Visual size of the element.
    ///
    /// This is what the user would consider the size, i.e. excluding CSD shadows and whatnot.
    /// Corresponds to the Wayland window geometry size.
    fn size(&self) -> Size<i32, Logical>;

    /// Location of the window buffer relative to the visual geometry.
    fn buf_loc(&self) -> Point<i32, Logical>;

    /// The client surface's committed DMA-BUF, if available.
    fn dmabuf(&self) -> Option<smithay::backend::allocator::dmabuf::Dmabuf> {
        None
    }

    /// Exact current attachment lease, if this platform can prove its source.
    fn native_dmabuf_source(
        &self,
    ) -> Option<crate::render_helpers::vulkan::native::OwnedDmabufSource> {
        None
    }

    /// Checks whether a point is in the element's input region.
    ///
    /// The point is relative to the element's visual geometry.
    fn is_in_input_region(&self, point: Point<f64, Logical>) -> bool;

    /// Renders the element at the given visual location.
    ///
    /// The element should be rendered in such a way that its visual geometry ends up at the given
    /// location.
    fn render<R: ClarionRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        location: Point<f64, Logical>,
        scale: Scale<f64>,
        alpha: f32,
        xray_pos: XrayPos,
        push: &mut dyn FnMut(LayoutElementRenderElement<R>),
    ) {
        self.render_popups(ctx.r(), location, scale, alpha, xray_pos, push);
        self.render_normal(ctx.r(), location, scale, alpha, push);
    }

    /// Renders the non-popup parts of the element.
    fn render_normal<R: ClarionRenderer>(
        &self,
        ctx: RenderCtx<R>,
        location: Point<f64, Logical>,
        scale: Scale<f64>,
        alpha: f32,
        push: &mut dyn FnMut(LayoutElementRenderElement<R>),
    ) {
        let _ = (ctx, location, scale, alpha, push);
    }

    /// Renders the popups of the element.
    fn render_popups<R: ClarionRenderer>(
        &self,
        ctx: RenderCtx<R>,
        location: Point<f64, Logical>,
        scale: Scale<f64>,
        alpha: f32,
        xray_pos: XrayPos,
        push: &mut dyn FnMut(LayoutElementRenderElement<R>),
    ) {
        let _ = (ctx, location, scale, alpha, xray_pos, push);
    }

    /// Renders the background effect behind the main surface of the element.
    #[allow(clippy::too_many_arguments)]
    fn render_background_effect<R: ClarionRenderer>(
        &self,
        _ctx: RenderCtx<R>,
        _geometry: Rectangle<f64, Logical>,
        _scale: f64,
        _clip_to_geometry: bool,
        _surface_anim_scale: Scale<f64>,
        _radius: CornerRadius,
        _xray_pos: XrayPos,
        _push: &mut dyn FnMut(BackgroundEffectElement),
    ) {
    }

    /// Requests the element to change its size.
    ///
    /// The size request is stored and will be continuously sent to the element on any further
    /// state changes.
    fn request_size(
        &mut self,
        size: Size<i32, Logical>,
        mode: SizingMode,
        animate: bool,
        transaction: Option<Transaction>,
    );

    /// Requests the element to change size once, clearing the request afterwards.
    fn request_size_once(&mut self, size: Size<i32, Logical>, animate: bool) {
        self.request_size(size, SizingMode::Normal, animate, None);
    }

    fn min_size(&self) -> Size<i32, Logical>;
    fn max_size(&self) -> Size<i32, Logical>;
    fn is_wl_surface(&self, wl_surface: &WlSurface) -> bool;
    fn has_ssd(&self) -> bool;
    fn set_preferred_scale_transform(&self, scale: output::Scale, transform: Transform);
    fn output_enter(&self, output: &Output);
    fn output_leave(&self, output: &Output);
    fn set_offscreen_data(&self, data: Option<OffscreenData>);
    /// Logical minimization is independent of wl_surface mapping and commit handling.
    fn set_minimized(&mut self, minimized: bool) {
        let _ = minimized;
    }

    fn set_activated(&mut self, active: bool);
    fn set_active_in_column(&mut self, active: bool);
    fn set_floating(&mut self, floating: bool);
    fn set_bounds(&self, bounds: Size<i32, Logical>);
    fn is_ignoring_opacity_window_rule(&self) -> bool;

    fn is_urgent(&self) -> bool;

    fn configure_intent(&self) -> ConfigureIntent;
    fn send_pending_configure(&mut self);

    /// The element's current sizing mode.
    ///
    /// This will *not* switch immediately after a [`LayoutElement::request_size()`] call.
    fn sizing_mode(&self) -> SizingMode;

    /// Whether this element is in real fullscreen mode.
    fn is_fullscreen(&self) -> bool {
        self.sizing_mode().is_fullscreen()
    }

    /// The sizing mode that we're requesting the element to assume.
    ///
    /// This *will* switch immediately after a [`LayoutElement::request_size()`] call.
    fn pending_sizing_mode(&self) -> SizingMode;

    /// Size previously requested through [`LayoutElement::request_size()`].
    fn requested_size(&self) -> Option<Size<i32, Logical>>;

    /// Non-fullscreen size that we expect this window has or will shortly have.
    ///
    /// This can be different from [`requested_size()`](LayoutElement::requested_size()). For
    /// example, for floating windows this will generally return the current window size, rather
    /// than the last size that we requested, since we want floating windows to be able to change
    /// size freely. But not always: if we just requested a floating window to resize and it hasn't
    /// responded to it yet, this will return the newly requested size.
    ///
    /// This function should never return a 0 size component. `None` means there's no known
    /// expected size (for example, the window is fullscreen).
    ///
    /// The default impl is for testing only, it will not preserve the window's own size changes.
    fn expected_size(&self) -> Option<Size<i32, Logical>> {
        if self.sizing_mode().is_fullscreen() {
            return None;
        }

        let mut requested = self.requested_size().unwrap_or_default();
        let current = self.size();
        if requested.w == 0 {
            requested.w = current.w;
        }
        if requested.h == 0 {
            requested.h = current.h;
        }
        Some(requested)
    }

    fn is_windowed_fullscreen(&self) -> bool {
        false
    }
    fn is_pending_windowed_fullscreen(&self) -> bool {
        false
    }
    fn request_windowed_fullscreen(&mut self, value: bool) {
        let _ = value;
    }

    /// The effective geometry corner radius for this element.
    ///
    /// Returns zero when the element is in windowed fullscreen, since fullscreen windows have
    /// square corners.
    ///
    /// This method only handles windowed fullscreen and not maximized/real fullscreen. This is
    /// because windowed fullscreen is handled by the element itself, whereas other sizing modes
    /// are handled externally by the Tile, so the corner radius changes for those modes is also
    /// handled externally.
    fn geometry_corner_radius(&self) -> CornerRadius {
        let rules = self.rules();

        // When windows think they're fullscreen, they square their corners.
        //
        // However, if the user is clipping the window to geometry, they are likely going for
        // consistent corner radius, and want this radius to remain in windowed fullscreen.
        if self.is_windowed_fullscreen() && rules.clip_to_geometry != Some(true) {
            return CornerRadius::default();
        }

        rules.geometry_corner_radius.unwrap_or_default()
    }

    fn is_child_of(&self, parent: &Self) -> bool;

    fn rules(&self) -> &ResolvedWindowRules;

    /// Runs periodic clean-up tasks.
    fn refresh(&self);

    fn take_animation_snapshot(&mut self) -> Option<LayoutElementRenderSnapshot>;
    fn take_native_animation_snapshot(
        &mut self,
    ) -> Option<LayoutElementRenderSnapshot<crate::render_helpers::vulkan::VulkanTexture>> {
        None
    }

    fn set_interactive_resize(&mut self, data: Option<InteractiveResizeData>);
    fn cancel_interactive_resize(&mut self);
    fn interactive_resize_data(&self) -> Option<InteractiveResizeData>;

    fn on_commit(&mut self, serial: Serial);
}

#[derive(Debug)]
pub struct Layout<W: LayoutElement> {
    scene_resources: SceneResourceAllocator,
    core: core_adapter::DesktopContext<W::Id>,
    desktop_transition_depth: usize,
    /// Monitors and workspaes in the layout.
    monitor_set: MonitorSet<W>,
    /// Whether the layout should draw as active.
    ///
    /// This normally indicates that the layout has keyboard focus, but not always. E.g. when the
    /// screenshot UI is open, it keeps the layout drawing as active.
    is_active: bool,
    /// Ongoing interactive move.
    interactive_move: Option<InteractiveMoveState<W>>,
    pending_move: Option<PendingMove<W::Id>>,
    /// Ongoing drag-and-drop operation.
    dnd: Option<DndData>,
    /// Clock for driving animations.
    clock: Clock,
    /// Time that we last updated render elements for.
    update_render_elements_time: Duration,
    /// Session lock suppresses future presentation effects as well as finishing
    /// existing ones; client configure requests can still update mapped state.
    minimize_animations_suspended: bool,
    /// The overview zoom progress.
    overview_progress: Option<OverviewProgress>,
    /// Configurable properties of the layout.
    options: Rc<Options>,
    // Test convenience owners follow every registration in drop order.
    #[cfg(test)]
    test_scene_resources: Option<crate::scene::resources::SceneResources>,
}

#[derive(Debug)]
enum MonitorSet<W: LayoutElement> {
    /// At least one output is connected.
    Normal {
        /// Connected monitors.
        monitors: Vec<Monitor<W>>,
    },
    /// No outputs are connected, and these are the workspaces.
    NoOutputs {
        /// The workspaces.
        workspaces: Vec<Workspace<W>>,
    },
}

impl<W: LayoutElement> MonitorSet<W> {
    fn monitors_mut(&mut self) -> impl Iterator<Item = &mut Monitor<W>> + '_ {
        let monitors = if let MonitorSet::Normal { monitors, .. } = self {
            &mut monitors[..]
        } else {
            &mut [][..]
        };

        monitors.iter_mut()
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Options {
    pub layout: clarion_config::Layout,
    pub animations: clarion_config::Animations,
    pub gestures: clarion_config::Gestures,
    pub overview: clarion_config::Overview,
    pub blur: clarion_config::Blur,
    pub renderer: clarion_config::RendererBackendConfig,
    // Debug flags.
    pub disable_resize_throttling: bool,
    pub disable_transactions: bool,
    pub deactivate_unfocused_windows: bool,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
enum InteractiveMoveState<W: LayoutElement> {
    /// Initial rubberbanding; the window remains in the layout.
    Starting {
        /// The window we're moving.
        window_id: W::Id,
    },
    /// Moving; the window is no longer in the layout.
    Moving(InteractiveMoveData<W>),
}

#[derive(Debug)]
struct PendingMove<I> {
    window: I,
    motion: clarion_core::input::MotionAccumulator<(Output, Point<f64, Logical>)>,
}

#[derive(Debug)]
struct InteractiveMoveData<W: LayoutElement> {
    /// The window being moved.
    pub(self) tile: Tile<W>,
    /// Output where the window is currently located/rendered.
    pub(self) output: Output,
    /// Current pointer position within output.
    pub(self) pointer_pos_within_output: Point<f64, Logical>,
    /// Shared unique desktop owner; this stores no copied move controls.
    core: core_adapter::DesktopContext<W::Id>,
    /// Config overrides for the output where the window is currently located.
    ///
    /// Cached here to be accessible while an output is removed.
    pub(self) output_config: Option<clarion_config::LayoutPart>,
    /// Config overrides for the workspace where the window is currently located.
    ///
    /// To avoid sudden window changes when starting an interactive move, it will remember the
    /// config overrides for the workspace where the move originated from. As soon as the window
    /// moves over some different workspace though, this override will reset.
    pub(self) workspace_config: Option<(WorkspaceId, clarion_config::LayoutPart)>,
}

#[derive(Debug)]
pub struct DndData {
    /// Output where the pointer is currently located.
    output: Output,
    /// Current pointer position within output.
    pointer_pos_within_output: Point<f64, Logical>,
}

#[derive(Debug, Clone, Copy)]
pub struct InteractiveResizeData {
    pub(self) edges: ResizeEdge,
}

#[derive(Debug, Clone, Copy)]
pub enum ConfigureIntent {
    /// A configure is not needed (no changes to server pending state).
    NotNeeded,
    /// A configure is throttled (due to resizing too fast for example).
    Throttled,
    /// Can send the configure if it isn't throttled externally (only size changed).
    CanSend,
    /// Should send the configure regardless of external throttling (something other than size
    /// changed).
    ShouldSend,
}

/// Tile that was just removed from the layout.
#[derive(Debug)]
pub struct RemovedTile<W: LayoutElement> {
    tile: Tile<W>,
    /// Width of the column the tile was in.
    width: ColumnWidth,
    /// Whether the column the tile was in was full-width.
    is_full_width: bool,
    /// Whether the tile was floating.
    is_floating: bool,
}

/// Whether to activate a newly added window.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ActivateWindow {
    /// Activate unconditionally.
    Yes,
    /// Activate based on heuristics.
    #[default]
    Smart,
    /// Do not activate.
    No,
}

/// Where to put a newly added window.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum AddWindowTarget<'a, W: LayoutElement> {
    /// No particular preference.
    #[default]
    Auto,
    /// On this output.
    Output(&'a Output),
    /// On this workspace.
    Workspace(WorkspaceId),
    /// Next to this existing window.
    NextTo(&'a W::Id),
}

/// Type of the window hit from `window_under()`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HitType {
    /// The hit is within a window's input region and can be used for sending events to it.
    Input {
        /// Position of the window's buffer.
        win_pos: Point<f64, Logical>,
    },
    /// The hit can activate a window, but it is not in the input region so cannot send events.
    ///
    /// For example, this could be clicking on a tile border outside the window.
    Activate {
        /// Whether the hit was on the tab indicator.
        is_tab_indicator: bool,
    },
}

pub use clarion_core::overview::OverviewMode;
use clarion_core::overview::{OverviewAction, OverviewSettlement};

#[derive(Debug)]
enum OverviewProgress {
    Animation(Animation),
    Gesture,
    Open,
}

/// Layer of windows to render.
#[derive(Clone, Copy)]
pub enum RenderLayer {
    Normal,
    /// Windows currently moving between workspaces.
    MovingBetweenWorkspaces,
}

impl SizingMode {
    #[must_use]
    pub fn is_normal(&self) -> bool {
        matches!(self, Self::Normal)
    }

    #[must_use]
    pub fn is_fullscreen(&self) -> bool {
        matches!(self, Self::Fullscreen)
    }

    #[must_use]
    pub fn is_maximized(&self) -> bool {
        matches!(self, Self::Maximized)
    }
}

impl<W: LayoutElement> InteractiveMoveState<W> {
    fn moving(&self) -> Option<&InteractiveMoveData<W>> {
        match self {
            InteractiveMoveState::Moving(move_) => Some(move_),
            _ => None,
        }
    }

    fn moving_mut(&mut self) -> Option<&mut InteractiveMoveData<W>> {
        match self {
            InteractiveMoveState::Moving(move_) => Some(move_),
            _ => None,
        }
    }
}

impl<W: LayoutElement> InteractiveMoveData<W> {
    fn controls(&self) -> clarion_core::move_controls::MoveControls {
        let core = self.core.borrow();
        let interaction = core.interaction().expect("physical move has a core owner");
        assert_eq!(interaction.id, self.tile.window().core_id());
        interaction.controls
    }

    fn placement(&self) -> clarion_core::move_controls::MovePlacement {
        self.controls().placement()
    }

    fn is_floating(&self) -> bool {
        self.placement().floating
    }

    fn tile_render_location(&self, zoom: f64) -> Point<f64, Logical> {
        let scale = Scale::from(self.output.current_scale().fractional_scale());
        let window_size = self.tile.window_size();
        let anchor = self.controls().anchor();
        let pointer_offset_within_window =
            Point::from((window_size.w * anchor[0], window_size.h * anchor[1]));
        let pos = self.pointer_pos_within_output
            - (pointer_offset_within_window + self.tile.window_loc() - self.tile.render_offset())
                .upscale(zoom);
        // Round to physical pixels.
        pos.to_physical_precise_round(scale).to_logical(scale)
    }
}

impl ActivateWindow {
    pub fn map_smart(self, f: impl FnOnce() -> bool) -> bool {
        match self {
            ActivateWindow::Yes => true,
            ActivateWindow::Smart => f(),
            ActivateWindow::No => false,
        }
    }
}

impl HitType {
    pub fn offset_win_pos(mut self, offset: Point<f64, Logical>) -> Self {
        match &mut self {
            HitType::Input { win_pos } => *win_pos += offset,
            HitType::Activate { .. } => (),
        }
        self
    }

    pub fn hit_tile<W: LayoutElement>(
        tile: &Tile<W>,
        tile_pos: Point<f64, Logical>,
        point: Point<f64, Logical>,
    ) -> Option<(&W, Self)> {
        let pos_within_tile = point - tile_pos;
        tile.hit(pos_within_tile)
            .map(|hit| (tile.window(), hit.offset_win_pos(tile_pos)))
    }

    pub fn to_activate(self) -> Self {
        match self {
            HitType::Input { .. } => HitType::Activate {
                is_tab_indicator: false,
            },
            HitType::Activate { .. } => self,
        }
    }
}

impl Options {
    fn from_config(config: &Config) -> Self {
        Self {
            layout: config.layout.clone(),
            animations: config.animations.clone(),
            gestures: config.gestures,
            overview: config.overview,
            blur: config.blur,
            renderer: config.renderer.unwrap_or_default(),
            disable_resize_throttling: config.debug.disable_resize_throttling,
            disable_transactions: config.debug.disable_transactions,
            deactivate_unfocused_windows: config.debug.deactivate_unfocused_windows,
        }
    }

    fn with_merged_layout(mut self, part: Option<&clarion_config::LayoutPart>) -> Self {
        if let Some(part) = part {
            self.layout.merge_with(part);
        }
        self
    }

    fn adjusted_for_scale(mut self, scale: f64) -> Self {
        self.layout.gaps = round_logical_in_physical_max1(scale, self.layout.gaps);
        self
    }
}

impl OverviewProgress {
    fn value(&self, gesture_value: f64) -> f64 {
        match self {
            OverviewProgress::Animation(anim) => anim.value(),
            OverviewProgress::Gesture => gesture_value,
            OverviewProgress::Open => 1.,
        }
    }

    fn is_animation(&self) -> bool {
        matches!(self, OverviewProgress::Animation(_))
    }
}

impl RenderLayer {
    /// Returns `true` if the render layer is [`Normal`].
    ///
    /// [`Normal`]: RenderLayer::Normal
    #[must_use]
    pub fn is_normal(&self) -> bool {
        matches!(self, Self::Normal)
    }
}

impl<W: LayoutElement> Layout<W> {
    fn rebind_detached_move_output(&mut self) {
        let Some(InteractiveMoveState::Moving(moving)) = &self.interactive_move else {
            return;
        };
        let target = self
            .monitor_for_output(&moving.output)
            .or_else(|| self.monitors().next());
        let Some(target) = target else {
            return;
        };
        let output = target.output().clone();
        let size = output_size(&output);
        let scale = output.current_scale().fractional_scale();
        if output == moving.output
            && moving.tile.view_size() == size
            && moving.tile.scale() == scale
        {
            return;
        }
        let output_config = target.layout_config().cloned();
        let Some(InteractiveMoveState::Moving(mut moving)) = self.interactive_move.take() else {
            unreachable!()
        };
        if moving.output != output {
            moving.tile.window().output_leave(&moving.output);
            moving.tile.window().output_enter(&output);
            moving
                .tile
                .window()
                .set_preferred_scale_transform(output.current_scale(), output.current_transform());
            moving.output = output.clone();
            moving.pointer_pos_within_output.x = moving
                .pointer_pos_within_output
                .x
                .clamp(0.0, size.w.max(1.0));
            moving.pointer_pos_within_output.y = moving
                .pointer_pos_within_output
                .y
                .clamp(0.0, size.h.max(1.0));
        }
        moving.output_config = output_config;
        let options = Options::clone(&self.options)
            .with_merged_layout(moving.output_config.as_ref())
            .with_merged_layout(moving.workspace_config.as_ref().map(|(_, config)| config))
            .adjusted_for_scale(scale);
        moving.tile.update_config(size, scale, Rc::new(options));
        self.interactive_move = Some(InteractiveMoveState::Moving(moving));
        self.commit_core_move_pose();
    }

    fn commit_core_move_pose(&self) {
        let Some(InteractiveMoveState::Moving(moving)) = &self.interactive_move else {
            return;
        };
        let zoom = self.interactive_move_zoom();
        let window_size = moving.tile.window_expected_or_current_size();
        let anchor = moving.controls().anchor();
        let offset = Point::from((window_size.w * anchor[0], window_size.h * anchor[1]));
        let location =
            moving.pointer_pos_within_output - (offset + moving.tile.window_loc()).upscale(zoom);
        let size = moving.tile.tile_expected_or_current_size().upscale(zoom);
        self.core
            .borrow_mut()
            .apply(clarion_core::CoreEvent::MovePose {
                id: moving.tile.window().core_id(),
                output: Some(moving.output.name()),
                geometry: clarion_core::Rect::new(location.x, location.y, size.w, size.h),
                pointer: [
                    moving.pointer_pos_within_output.x,
                    moving.pointer_pos_within_output.y,
                ],
            });
    }

    fn finish_core_move_if_placed(&self) {
        if self.interactive_move.is_none() {
            let id = self
                .core
                .borrow()
                .interaction()
                .map(|interaction| interaction.id);
            if let Some(id) = id {
                self.core
                    .borrow_mut()
                    .apply(clarion_core::CoreEvent::MovePlaced { id });
                // Placement can deliberately leave the destination policy without a selected
                // window. Acknowledge that final pure policy intent after transfer completion,
                // before activating its keyboard scope; explicit None must remain meaningful.
                let space = self.core.borrow().window(&id).map(|window| window.space);
                if let Some(space) = space {
                    if let Some((_, _, workspace)) = self
                        .workspaces()
                        .find(|(_, _, workspace)| workspace.id().get() == space.0)
                    {
                        workspace.record_core_focus(false);
                    }
                }
            }
        }
    }

    fn commit_core_focus_scope(&self) {
        if let Some(workspace) = self.active_workspace() {
            self.core
                .borrow_mut()
                .apply(clarion_core::CoreEvent::ActivateSpace {
                    id: clarion_core::SpaceId(workspace.id().get()),
                });
        } else {
            self.core
                .borrow_mut()
                .apply(clarion_core::CoreEvent::DeactivateSpace);
        }
    }

    fn with_desktop_transition<T>(&mut self, operation: impl FnOnce(&mut Self) -> T) -> T {
        self.desktop_transition_depth += 1;
        let result = operation(self);
        self.desktop_transition_depth -= 1;
        if self.desktop_transition_depth == 0 {
            self.rebind_detached_move_output();
            self.finish_core_move_if_placed();
            self.commit_core_focus_scope();
            // Logical search membership is acknowledged once at the desktop
            // mutation edge, never reconstructed by a render or frame clock.
            if self.is_window_spread_visible() {
                for monitor in self.monitors() {
                    monitor.reconcile_spread_candidates(false);
                }
            }
        }
        result
    }

    /// Tests own an independent scene beside the layout under construction.
    #[cfg(test)]
    pub fn new(clock: Clock, config: &Config) -> Self {
        let resources = crate::scene::resources::SceneResources::new();
        let mut layout = Self::new_with_resources(clock, config, resources.allocator());
        layout.test_scene_resources = Some(resources);
        layout
    }

    pub fn new_with_resources(
        clock: Clock,
        config: &Config,
        resources: SceneResourceAllocator,
    ) -> Self {
        Self::with_options_and_workspaces(clock, config, Options::from_config(config), resources)
    }

    #[cfg(test)]
    pub fn with_options(clock: Clock, options: Options) -> Self {
        let resources = crate::scene::resources::SceneResources::new();
        let mut layout = Self::with_options_and_resources(clock, options, resources.allocator());
        layout.test_scene_resources = Some(resources);
        layout
    }

    pub fn with_options_and_resources(
        clock: Clock,
        options: Options,
        scene_resources: SceneResourceAllocator,
    ) -> Self {
        Self {
            scene_resources,
            #[cfg(test)]
            test_scene_resources: None,
            core: core_adapter::new_context(),
            desktop_transition_depth: 0,
            monitor_set: MonitorSet::NoOutputs { workspaces: vec![] },
            is_active: true,
            interactive_move: None,
            pending_move: None,
            dnd: None,
            clock,
            update_render_elements_time: Duration::ZERO,
            minimize_animations_suspended: false,
            overview_progress: None,
            options: Rc::new(options),
        }
    }

    fn with_options_and_workspaces(
        clock: Clock,
        config: &Config,
        options: Options,
        scene_resources: SceneResourceAllocator,
    ) -> Self {
        let opts = Rc::new(options);
        let core = core_adapter::new_context();

        let workspaces = config
            .workspaces
            .iter()
            .map(|ws| {
                Workspace::new_with_config_no_outputs(
                    Some(ws.clone()),
                    clock.clone(),
                    opts.clone(),
                    scene_resources.clone(),
                )
                .with_core(core.clone())
            })
            .collect();

        Self {
            scene_resources,
            #[cfg(test)]
            test_scene_resources: None,
            core,
            desktop_transition_depth: 0,
            monitor_set: MonitorSet::NoOutputs { workspaces },
            is_active: true,
            interactive_move: None,
            pending_move: None,
            dnd: None,
            clock,
            update_render_elements_time: Duration::ZERO,
            minimize_animations_suspended: false,
            overview_progress: None,
            options: opts,
        }
    }

    pub fn add_output(&mut self, output: Output, layout_config: Option<LayoutPart>) {
        self.with_desktop_transition(|this| {
            let stage_drag_source = this.stage_drag_source_workspace();
            // Register the incoming owned facts before asking the core which
            // stable spaces should return. No payload or renderer enters it.
            core_adapter::connect_output(&this.core, &output);
            this.monitor_set = match mem::take(&mut this.monitor_set) {
                MonitorSet::Normal { mut monitors } => {
                    let selected_index = primary_monitor_idx(&monitors);
                    let primary = &mut monitors[selected_index];

                    let mut stopped_primary_ws_switch = false;
                    let returning: std::collections::HashSet<_> = this
                        .core
                        .borrow()
                        .returning_spaces(primary.output_name(), &output.name())
                        .into_iter()
                        .collect();

                    let mut workspaces = vec![];
                    for i in (0..primary.workspaces.len()).rev() {
                        if returning
                            .contains(&clarion_core::SpaceId(primary.workspaces[i].id().get()))
                        {
                            let prior_active = primary.active_workspace_idx();
                            let prefer_first_nonempty =
                                primary.options.layout.empty_workspace_above_first
                                    && prior_active == 1
                                    && i <= prior_active;
                            let mut ws = primary.workspaces.remove(i);
                            ws.set_output(None);
                            if prefer_first_nonempty && primary.workspaces.len() > 1 {
                                let id = clarion_core::SpaceId(primary.workspaces[1].id().get());
                                this.core.borrow_mut().select_output_space(
                                    primary.output_name(),
                                    id,
                                    false,
                                );
                            }

                            // FIXME: this can be coded in a way that the workspace switch won't be
                            // affected if the removed workspace is invisible. But this is good
                            // enough for now.
                            if primary.workspace_switch.is_some() {
                                primary.clear_workspace_switch();
                                stopped_primary_ws_switch = true;
                            }

                            // The user could've closed a window while remaining on this workspace,
                            // on another monitor. However, we will add
                            // an empty workspace in the end instead.
                            if ws.has_windows_or_name() || Some(ws.id()) == stage_drag_source {
                                workspaces.push(ws);
                            }
                        }
                    }

                    // If we stopped a workspace switch, then we might need to clean up workspaces.
                    // Also if empty_workspace_above_first is set and there are only 2 workspaces
                    // left, both will be empty and one of them needs to be
                    // removed. clean_up_workspaces takes care of this.

                    if stopped_primary_ws_switch
                        || (primary.options.layout.empty_workspace_above_first
                            && primary.workspaces.len() == 2)
                    {
                        primary.clean_up_workspaces();
                    }

                    workspaces.reverse();

                    let ws_id_to_activate = this
                        .core
                        .borrow_mut()
                        .take_remembered_space(&output.name())
                        .map(|id| WorkspaceId::specific(id.0));

                    let mut monitor = Monitor::new_with_stage_drag_source(
                        output,
                        workspaces,
                        ws_id_to_activate,
                        this.clock.clone(),
                        this.options.clone(),
                        layout_config,
                        stage_drag_source,
                        this.scene_resources.clone(),
                    );
                    monitor.set_core(this.core.clone());
                    monitor.set_overview_progress(this.overview_progress.as_ref());
                    monitors.push(monitor);

                    MonitorSet::Normal { monitors }
                }
                MonitorSet::NoOutputs { workspaces } => {
                    let ws_id_to_activate = this
                        .core
                        .borrow_mut()
                        .take_remembered_space(&output.name())
                        .map(|id| WorkspaceId::specific(id.0));

                    let mut monitor = Monitor::new_with_stage_drag_source(
                        output,
                        workspaces,
                        ws_id_to_activate,
                        this.clock.clone(),
                        this.options.clone(),
                        layout_config,
                        stage_drag_source,
                        this.scene_resources.clone(),
                    );
                    monitor.set_core(this.core.clone());
                    monitor.set_overview_progress(this.overview_progress.as_ref());

                    MonitorSet::Normal {
                        monitors: vec![monitor],
                    }
                }
            }
        })
    }

    pub fn remove_output(&mut self, output: &Output) {
        self.with_desktop_transition(|this| {
            this.flush_pending_move();
            this.core
                .borrow_mut()
                .apply(clarion_core::CoreEvent::OutputDisconnected {
                    connector: output.name(),
                });
            this.core
                .borrow_mut()
                .overview_action(OverviewAction::OutputRemoved(output.name()));
            this.monitor_set = match mem::take(&mut this.monitor_set) {
                MonitorSet::Normal { mut monitors } => {
                    let idx = monitors
                        .iter()
                        .position(|mon| &mon.output == output)
                        .expect("trying to remove non-existing output");
                    let monitor = monitors.remove(idx);

                    let mut workspaces = monitor.into_workspaces();

                    if monitors.is_empty() {
                        // Removed the last monitor.

                        for ws in &mut workspaces {
                            // Reset base options to layout ones.
                            ws.update_config(this.options.clone());
                        }

                        MonitorSet::NoOutputs { workspaces }
                    } else {
                        let selected_index = primary_monitor_idx(&monitors);
                        let primary = &mut monitors[selected_index];
                        primary.append_workspaces(workspaces);

                        MonitorSet::Normal { monitors }
                    }
                }
                MonitorSet::NoOutputs { .. } => {
                    panic!("tried to remove output when there were already none")
                }
            }
        })
    }

    pub fn add_column_by_idx(
        &mut self,
        monitor_idx: usize,
        workspace_idx: usize,
        column: Column<W>,
        activate: bool,
    ) {
        self.with_desktop_transition(|this| {
            let MonitorSet::Normal { monitors, .. } = &mut this.monitor_set else {
                panic!()
            };

            monitors[monitor_idx].add_column(workspace_idx, column, activate, None);

            if activate {
                monitors[monitor_idx].select_output();
            }
        })
    }

    /// Adds a new window to the layout.
    ///
    /// Returns an output that the window was added to, if there were any outputs.
    #[allow(clippy::too_many_arguments)]
    pub fn add_window(
        &mut self,
        window: W,
        target: AddWindowTarget<W>,
        width: Option<PresetSize>,
        height: Option<PresetSize>,
        is_full_width: bool,
        is_floating: bool,
        activate: ActivateWindow,
    ) -> Option<&Output> {
        let scrolling_height = height.map(SizeChange::from);
        let id = window.id().clone();

        match &mut self.monitor_set {
            MonitorSet::Normal { monitors, .. } => {
                let (mon_idx, target) = match target {
                    AddWindowTarget::Auto => {
                        (selected_monitor_idx(monitors), MonitorAddWindowTarget::Auto)
                    }
                    AddWindowTarget::Output(output) => {
                        let mon_idx = monitors
                            .iter()
                            .position(|mon| mon.output == *output)
                            .unwrap();

                        (mon_idx, MonitorAddWindowTarget::Auto)
                    }
                    AddWindowTarget::Workspace(ws_id) => {
                        let mon_idx = monitors.iter().position(|mon| mon.has_ws(ws_id)).unwrap();

                        (
                            mon_idx,
                            MonitorAddWindowTarget::Workspace {
                                id: ws_id,
                                column_idx: None,
                            },
                        )
                    }
                    AddWindowTarget::NextTo(next_to) => {
                        if let Some(output) = self
                            .interactive_move
                            .as_ref()
                            .and_then(|move_| {
                                if let InteractiveMoveState::Moving(move_) = move_ {
                                    Some(move_)
                                } else {
                                    None
                                }
                            })
                            .filter(|move_| next_to == move_.tile.window().id())
                            .map(|move_| move_.output.clone())
                        {
                            // The next_to window is being interactively moved.
                            let mon_idx = monitors
                                .iter()
                                .position(|mon| mon.output == output)
                                .unwrap_or(selected_monitor_idx(monitors));

                            (mon_idx, MonitorAddWindowTarget::Auto)
                        } else {
                            let mon_idx = monitors
                                .iter()
                                .position(|mon| {
                                    mon.workspaces.iter().any(|ws| ws.has_window(next_to))
                                })
                                .unwrap();
                            (mon_idx, MonitorAddWindowTarget::NextTo(next_to))
                        }
                    }
                };
                let mon = &mut monitors[mon_idx];

                let (ws_idx, _) = mon.resolve_add_window_target(target);
                let ws = &mon.workspaces[ws_idx];
                let scrolling_width = ws.resolve_scrolling_width(&window, width);

                mon.add_window(
                    window,
                    target,
                    activate,
                    scrolling_width,
                    is_full_width,
                    is_floating,
                );

                if activate.map_smart(|| false) {
                    mon.select_output();
                }

                // Set the default height for scrolling windows.
                if !is_floating {
                    if let Some(change) = scrolling_height {
                        let ws = mon
                            .workspaces
                            .iter_mut()
                            .find(|ws| ws.has_window(&id))
                            .unwrap();
                        ws.set_window_height(Some(&id), change);
                    }
                }

                if mon.is_selected_output() {
                    let id = clarion_core::SpaceId(mon.active_workspace_ref().id().get());
                    self.core
                        .borrow_mut()
                        .apply(clarion_core::CoreEvent::ActivateSpace { id });
                }
                Some(&mon.output)
            }
            MonitorSet::NoOutputs { workspaces } => {
                let (ws_idx, target) = match target {
                    AddWindowTarget::Auto => {
                        if workspaces.is_empty() {
                            workspaces.push(
                                Workspace::new_no_outputs(
                                    self.clock.clone(),
                                    self.options.clone(),
                                    self.scene_resources.clone(),
                                )
                                .with_core(self.core.clone()),
                            );
                        }

                        (
                            detached_workspace_idx(&self.core, workspaces),
                            WorkspaceAddWindowTarget::Auto,
                        )
                    }
                    AddWindowTarget::Output(_) => panic!(),
                    AddWindowTarget::Workspace(ws_id) => {
                        let ws_idx = workspaces.iter().position(|ws| ws.id() == ws_id).unwrap();
                        (ws_idx, WorkspaceAddWindowTarget::Auto)
                    }
                    AddWindowTarget::NextTo(next_to) => {
                        if self
                            .interactive_move
                            .as_ref()
                            .and_then(|move_| {
                                if let InteractiveMoveState::Moving(move_) = move_ {
                                    Some(move_)
                                } else {
                                    None
                                }
                            })
                            .filter(|move_| next_to == move_.tile.window().id())
                            .is_some()
                        {
                            // The next_to window is being interactively moved. If there are no
                            // other windows, we may have no workspaces at all.
                            if workspaces.is_empty() {
                                workspaces.push(
                                    Workspace::new_no_outputs(
                                        self.clock.clone(),
                                        self.options.clone(),
                                        self.scene_resources.clone(),
                                    )
                                    .with_core(self.core.clone()),
                                );
                            }

                            (
                                detached_workspace_idx(&self.core, workspaces),
                                WorkspaceAddWindowTarget::Auto,
                            )
                        } else {
                            let ws_idx = workspaces
                                .iter()
                                .position(|ws| ws.has_window(next_to))
                                .unwrap();
                            (ws_idx, WorkspaceAddWindowTarget::NextTo(next_to))
                        }
                    }
                };
                let ws = &mut workspaces[ws_idx];

                let scrolling_width = ws.resolve_scrolling_width(&window, width);

                let tile = ws.make_tile(window);
                ws.add_tile(
                    tile,
                    target,
                    activate,
                    scrolling_width,
                    is_full_width,
                    is_floating,
                    None,
                );

                // Set the default height for scrolling windows.
                if !is_floating {
                    if let Some(change) = scrolling_height {
                        ws.set_window_height(Some(&id), change);
                    }
                }

                self.core
                    .borrow_mut()
                    .apply(clarion_core::CoreEvent::DeactivateSpace);
                None
            }
        }
    }

    pub fn remove_window(
        &mut self,
        window: &W::Id,
        transaction: Transaction,
    ) -> Option<RemovedTile<W>> {
        self.with_desktop_transition(|this| {
            let removed = this.detach_window(window, transaction);
            if removed.is_some() {
                if let Some(core_id) = this.core.id(window) {
                    this.core.borrow_mut().remove_window(&core_id);
                }
                this.core.unregister(window);
            }
            removed
        })
    }

    /// Transfer a mapped tile without retiring its identity or layout history.
    fn detach_window(
        &mut self,
        window: &W::Id,
        transaction: Transaction,
    ) -> Option<RemovedTile<W>> {
        let result = (|| {
            if self
                .pending_move
                .as_ref()
                .is_some_and(|p| &p.window == window)
            {
                self.pending_move = None;
            }
            if let Some(state) = &self.interactive_move {
                match state {
                    InteractiveMoveState::Starting { window_id, .. } => {
                        if window_id == window {
                            self.interactive_move_end(window);
                        }
                    }
                    InteractiveMoveState::Moving(move_) => {
                        if move_.tile.window().id() == window {
                            let Some(InteractiveMoveState::Moving(move_)) =
                                self.interactive_move.take()
                            else {
                                unreachable!()
                            };
                            self.discard_stage_drag_origin(window);

                            for mon in self.monitors_mut() {
                                mon.dnd_scroll_gesture_end();
                            }

                            // Unlock the view on the workspaces.
                            for ws in self.workspaces_mut() {
                                ws.dnd_scroll_gesture_end();
                            }

                            let placement = move_.placement();
                            return Some(RemovedTile {
                                tile: move_.tile,
                                width: placement.width,
                                is_full_width: placement.full_width,
                                is_floating: false,
                            });
                        }
                    }
                }
            }

            match &mut self.monitor_set {
                MonitorSet::Normal { monitors, .. } => {
                    for mon in monitors {
                        let stage_drag_source = mon.stage_drag_source();
                        for (idx, ws) in mon.workspaces.iter_mut().enumerate() {
                            if ws.has_window(window) {
                                let removed = ws.remove_tile(window, transaction);

                                // Clean up empty workspaces that are not active and not last.
                                if !ws.has_windows_or_name()
                                    && Some(ws.id()) != stage_drag_source
                                    && idx != mon.active_workspace_idx()
                                    && idx != mon.workspaces.len() - 1
                                    && mon.workspace_switch.is_none()
                                {
                                    mon.workspaces.remove(idx);
                                }

                                // Special case handling when empty_workspace_above_first is set and
                                // all workspaces are empty.
                                if mon.options.layout.empty_workspace_above_first
                                    && mon.workspaces.len() == 2
                                    && mon.workspace_switch.is_none()
                                    && !mon
                                        .stage_drag_source()
                                        .is_some_and(|id| mon.idx_of_ws(id).is_some())
                                {
                                    assert!(!mon.workspaces[0].has_windows_or_name());
                                    assert!(!mon.workspaces[1].has_windows_or_name());
                                    mon.workspaces.remove(1);
                                }
                                return Some(removed);
                            }
                        }
                    }
                }
                MonitorSet::NoOutputs { workspaces, .. } => {
                    for (idx, ws) in workspaces.iter_mut().enumerate() {
                        if ws.has_window(window) {
                            let removed = ws.remove_tile(window, transaction);

                            // Clean up empty workspaces.
                            if !ws.has_windows_or_name()
                                && self
                                    .core
                                    .borrow()
                                    .stage_drag_source()
                                    .map(|id| WorkspaceId::specific(id.0))
                                    != Some(ws.id())
                            {
                                workspaces.remove(idx);
                            }

                            return Some(removed);
                        }
                    }
                }
            }

            None
        })();
        result
    }

    pub fn descendants_added(&mut self, id: &W::Id) -> bool {
        for ws in self.workspaces_mut() {
            if ws.descendants_added(id) {
                return true;
            }
        }

        false
    }

    /// Prepare an interrupted resize immediately before applying the new geometry.
    pub fn prepare_resize_retarget(
        &mut self,
        renderer: crate::render_helpers::renderer::AnyRenderer<'_>,
        window: &W::Id,
    ) {
        if let Some(InteractiveMoveState::Moving(move_)) = &mut self.interactive_move {
            if move_.tile.window().id() == window {
                move_.tile.prepare_resize_retarget(renderer);
                return;
            }
        }
        for ws in self.workspaces_mut() {
            if let Some(tile) = ws.tiles_mut().find(|tile| tile.window().id() == window) {
                tile.prepare_resize_retarget(renderer);
                return;
            }
        }
    }

    /// Return the affected output only if registering this target removes a
    /// visible transition; advisory destinations do not change the scene.
    pub fn set_minimize_target(
        &mut self,
        window: &W::Id,
        target: Option<clarion_ipc::DockTarget>,
        reduce_motion: bool,
    ) -> Option<Output> {
        for ws in self.workspaces_mut() {
            let output = ws.current_output().cloned();
            if let Some(tile) = ws.tiles_mut().find(|tile| tile.window().id() == window) {
                let target = target.filter(|target| {
                    target.is_valid()
                        && output.as_ref().is_some_and(|output| {
                            let size = output_size(output);
                            target.output == output.name()
                                && target.x + target.width <= size.w
                                && target.y + target.height <= size.h
                        })
                });
                return tile
                    .set_minimize_target(target, reduce_motion)
                    .then_some(output)
                    .flatten();
            }
        }
        None
    }

    pub fn minimize_window(&mut self, window: &W::Id) -> bool {
        self.with_desktop_transition(|this| {
            this.interactive_move_end(window);
            this.discard_stage_drag_origin(window);
            let active: Vec<_> = this
                .monitors()
                .map(|mon| mon.active_workspace_ref().id())
                .collect();
            let visible = !this.is_overview_open() && !this.minimize_animations_suspended;
            for ws in this.workspaces_mut() {
                if ws.has_window(window) {
                    return ws.minimize_window(window, visible && active.contains(&ws.id()));
                }
            }
            false
        })
    }

    pub fn restore_window(&mut self, window: &W::Id) -> bool {
        self.with_desktop_transition(|this| {
            let visible = !this.is_overview_open() && !this.minimize_animations_suspended;
            for ws in this.workspaces_mut() {
                if ws.has_window(window) {
                    return ws.restore_window(window, visible);
                }
            }
            false
        })
    }

    pub fn is_minimized(&self, window: &W::Id) -> bool {
        self.workspaces().any(|(_, _, ws)| ws.is_minimized(window))
    }

    /// Drop textures and complete presentation on lock/mode changes; mapped
    /// minimized state is retained and can still be restored after unlocking.
    pub fn complete_minimize_animations(&mut self) {
        for ws in self.workspaces_mut() {
            for tile in ws.tiles_mut() {
                tile.complete_genie();
            }
        }
    }

    pub fn suspend_minimize_animations(&mut self, suspended: bool) {
        self.minimize_animations_suspended = suspended;
        if suspended {
            self.complete_minimize_animations();
        }
    }

    pub fn update_window(&mut self, window: &W::Id, serial: Option<Serial>) {
        self.with_desktop_transition(|this| {
            if let Some(InteractiveMoveState::Moving(move_)) = &mut this.interactive_move {
                if move_.tile.window().id() == window {
                    // Do this before calling update_window() so it can get up-to-date info.
                    if let Some(serial) = serial {
                        move_.tile.window_mut().on_commit(serial);
                    }

                    move_.tile.update_window();
                    return;
                }
            }

            match &mut this.monitor_set {
                MonitorSet::Normal { monitors, .. } => {
                    for mon in monitors {
                        for ws in &mut mon.workspaces {
                            if ws.has_window(window) {
                                ws.update_window(window, serial);
                                return;
                            }
                        }
                    }
                }
                MonitorSet::NoOutputs { workspaces, .. } => {
                    for ws in workspaces {
                        if ws.has_window(window) {
                            ws.update_window(window, serial);
                            return;
                        }
                    }
                }
            }
        })
    }

    pub fn find_workspace_by_id(&self, id: WorkspaceId) -> Option<(usize, &Workspace<W>)> {
        match &self.monitor_set {
            MonitorSet::Normal { ref monitors, .. } => {
                for mon in monitors {
                    if let Some(index) = mon.idx_of_ws(id) {
                        let workspace = &mon.workspaces[index];
                        return Some((index, workspace));
                    }
                }
            }
            MonitorSet::NoOutputs { workspaces } => {
                if let Some((index, workspace)) =
                    workspaces.iter().enumerate().find(|(_, w)| w.id() == id)
                {
                    return Some((index, workspace));
                }
            }
        }

        None
    }

    pub fn find_workspace_by_name(&self, workspace_name: &str) -> Option<(usize, &Workspace<W>)> {
        let id = self.core.borrow().named_space(workspace_name)?;
        self.find_workspace_by_id(WorkspaceId::specific(id.0))
    }

    pub fn find_workspace_by_ref(
        &mut self,
        reference: WorkspaceReference,
    ) -> Option<&mut Workspace<W>> {
        if let WorkspaceReference::Index(index) = reference {
            self.active_monitor().and_then(|m| {
                let index = index.saturating_sub(1) as usize;
                m.workspaces.get_mut(index)
            })
        } else {
            let id = match &reference {
                WorkspaceReference::Name(name) => self.core.borrow().named_space(name)?.0,
                WorkspaceReference::Id(id) => *id,
                WorkspaceReference::Index(_) => unreachable!(),
            };
            self.workspaces_mut()
                .find(|workspace| workspace.id().get() == id)
        }
    }

    pub fn unname_workspace(&mut self, workspace_name: &str) {
        self.unname_workspace_by_ref(WorkspaceReference::Name(workspace_name.into()));
    }

    pub fn unname_workspace_by_ref(&mut self, reference: WorkspaceReference) {
        let id = self.find_workspace_by_ref(reference).map(|ws| ws.id());
        if let Some(id) = id {
            self.unname_workspace_by_id(id);
        }
    }

    pub fn unname_workspace_by_id(&mut self, id: WorkspaceId) {
        match &mut self.monitor_set {
            MonitorSet::Normal { monitors, .. } => {
                for mon in monitors {
                    if mon.unname_workspace(id) {
                        return;
                    }
                }
            }
            MonitorSet::NoOutputs { workspaces } => {
                for (idx, ws) in workspaces.iter_mut().enumerate() {
                    if ws.id() == id {
                        ws.unname();

                        // Clean up empty workspaces.
                        if !ws.has_windows() {
                            workspaces.remove(idx);
                        }

                        return;
                    }
                }
            }
        }
    }

    pub fn find_window_and_output(&self, wl_surface: &WlSurface) -> Option<(&W, Option<&Output>)> {
        if let Some(InteractiveMoveState::Moving(move_)) = &self.interactive_move {
            if move_.tile.window().is_wl_surface(wl_surface) {
                return Some((move_.tile.window(), Some(&move_.output)));
            }
        }

        match &self.monitor_set {
            MonitorSet::Normal { monitors, .. } => {
                for mon in monitors {
                    for ws in &mon.workspaces {
                        if let Some(window) = ws.find_wl_surface(wl_surface) {
                            return Some((window, Some(&mon.output)));
                        }
                    }
                }
            }
            MonitorSet::NoOutputs { workspaces } => {
                for ws in workspaces {
                    if let Some(window) = ws.find_wl_surface(wl_surface) {
                        return Some((window, None));
                    }
                }
            }
        }

        None
    }

    pub fn find_window_and_output_mut(
        &mut self,
        wl_surface: &WlSurface,
    ) -> Option<(&mut W, Option<&Output>)> {
        if let Some(InteractiveMoveState::Moving(move_)) = &mut self.interactive_move {
            if move_.tile.window().is_wl_surface(wl_surface) {
                return Some((move_.tile.window_mut(), Some(&move_.output)));
            }
        }

        match &mut self.monitor_set {
            MonitorSet::Normal { monitors, .. } => {
                for mon in monitors {
                    for ws in &mut mon.workspaces {
                        if let Some(window) = ws.find_wl_surface_mut(wl_surface) {
                            return Some((window, Some(&mon.output)));
                        }
                    }
                }
            }
            MonitorSet::NoOutputs { workspaces } => {
                for ws in workspaces {
                    if let Some(window) = ws.find_wl_surface_mut(wl_surface) {
                        return Some((window, None));
                    }
                }
            }
        }

        None
    }

    /// Computes the window-geometry-relative target rect for popup unconstraining.
    ///
    /// We will try to fit popups inside this rect.
    pub fn popup_target_rect(&self, window: &W::Id) -> Rectangle<f64, Logical> {
        if let Some(InteractiveMoveState::Moving(move_)) = &self.interactive_move {
            if move_.tile.window().id() == window {
                // Follow the scrolling layout logic and fit the popup horizontally within the
                // window geometry.
                let width = move_.tile.window_size().w;
                let height = output_size(&move_.output).h;
                let mut target = Rectangle::from_size(Size::from((width, height)));
                // FIXME: ideally this shouldn't include the tile render offset, but the code
                // duplication would be a bit annoying for this edge case.
                target.loc.y -= move_.tile_render_location(1.).y;
                target.loc.y -= move_.tile.window_loc().y;
                return target;
            }
        }

        self.workspaces()
            .find_map(|(_, _, ws)| ws.popup_target_rect(window))
            .unwrap()
    }

    pub fn update_output_size(&mut self, output: &Output) {
        self.with_desktop_transition(|this| {
            this.flush_pending_move();
            core_adapter::connect_output(&this.core, output);
            let _span = clarion_tracy_span!("Layout::update_output_size");

            let Some(mon) = this.monitor_for_output_mut(output) else {
                error!("monitor missing in update_output_size()");
                return;
            };

            mon.update_output_size();
        })
    }

    pub fn scroll_amount_to_activate(&self, window: &W::Id) -> f64 {
        if let Some(InteractiveMoveState::Moving(move_)) = &self.interactive_move {
            if move_.tile.window().id() == window {
                return 0.;
            }
        }

        for mon in self.monitors() {
            for ws in &mon.workspaces {
                if ws.has_window(window) {
                    return ws.scroll_amount_to_activate(window);
                }
            }
        }

        0.
    }

    pub fn should_trigger_focus_follows_mouse_on(&self, window: &W::Id) -> bool {
        // During an animation, it's easy to trigger focus-follows-mouse on the previous workspace,
        // especially when clicking to switch workspace on a bar of some kind. This cancels the
        // workspace switch, which is annoying and not intended.
        //
        // This function allows focus-follows-mouse to trigger only on the animation target
        // workspace.
        if let Some(InteractiveMoveState::Moving(move_)) = &self.interactive_move {
            if move_.tile.window().id() == window {
                return true;
            }
        }

        let MonitorSet::Normal { monitors, .. } = &self.monitor_set else {
            return true;
        };

        let (mon, ws_idx) = monitors
            .iter()
            .find_map(|mon| {
                mon.workspaces
                    .iter()
                    .position(|ws| ws.has_window(window))
                    .map(|ws_idx| (mon, ws_idx))
            })
            .unwrap();

        // During a gesture, focus-follows-mouse does not cause any unintended workspace switches.
        if let Some(WorkspaceSwitch::Gesture(_)) = mon.workspace_switch {
            return true;
        }

        ws_idx == mon.active_workspace_idx()
    }

    pub fn activate_window(&mut self, window: &W::Id) {
        self.with_desktop_transition(|this| {
            if this.is_window_spread_visible() {
                for mon in this.monitors_mut() {
                    mon.capture_spread_frame();
                }
            }
            // Spread can find parked windows. Restore before workspace activation
            // so an overview selection cannot start a mesh in transformed geometry.
            this.restore_window(window);
            if let Some(InteractiveMoveState::Moving(move_)) = &this.interactive_move {
                if move_.tile.window().id() == window {
                    return;
                }
            }

            let MonitorSet::Normal { monitors, .. } = &mut this.monitor_set else {
                return;
            };

            for mon in monitors.iter_mut() {
                for (workspace_idx, ws) in mon.workspaces.iter_mut().enumerate() {
                    if ws.activate_window(window) {
                        mon.select_output();

                        // If currently in the middle of a vertical swipe between the target
                        // workspace and some other, don't switch the
                        // workspace.
                        match &mon.workspace_switch {
                            Some(WorkspaceSwitch::Gesture(_))
                                if mon.workspace_gesture_value().floor()
                                    == workspace_idx as f64
                                    || mon.workspace_gesture_value().ceil()
                                        == workspace_idx as f64 => {}
                            _ => mon.switch_workspace(workspace_idx),
                        }

                        return;
                    }
                }
            }
        })
    }

    pub fn activate_window_without_raising(&mut self, window: &W::Id) {
        self.with_desktop_transition(|this| {
            this.restore_window(window);
            if let Some(InteractiveMoveState::Moving(move_)) = &this.interactive_move {
                if move_.tile.window().id() == window {
                    return;
                }
            }

            let MonitorSet::Normal { monitors, .. } = &mut this.monitor_set else {
                return;
            };

            for mon in monitors.iter_mut() {
                for (workspace_idx, ws) in mon.workspaces.iter_mut().enumerate() {
                    if ws.activate_window_without_raising(window) {
                        mon.select_output();

                        // If currently in the middle of a vertical swipe between the target
                        // workspace and some other, don't switch the
                        // workspace.
                        match &mon.workspace_switch {
                            Some(WorkspaceSwitch::Gesture(_))
                                if mon.workspace_gesture_value().floor()
                                    == workspace_idx as f64
                                    || mon.workspace_gesture_value().ceil()
                                        == workspace_idx as f64 => {}
                            _ => mon.switch_workspace(workspace_idx),
                        }

                        return;
                    }
                }
            }
        })
    }

    pub fn active_output(&self) -> Option<&Output> {
        let MonitorSet::Normal { monitors, .. } = &self.monitor_set else {
            return None;
        };

        Some(&monitors[selected_monitor_idx(monitors)].output)
    }

    pub fn active_workspace(&self) -> Option<&Workspace<W>> {
        let MonitorSet::Normal { monitors, .. } = &self.monitor_set else {
            return None;
        };

        let mon = &monitors[selected_monitor_idx(monitors)];
        Some(&mon.workspaces[mon.active_workspace_idx()])
    }

    pub fn active_window(&self) -> Option<&W> {
        self.focus_with_output().map(|(window, _)| window)
    }

    pub(crate) fn drain_core_protocol_ops(&self, ops: &mut Vec<CoreProtocolOp>) {
        self.core.drain_protocol_ops(ops);
    }

    pub(crate) fn core_focus_id(&self) -> Option<u64> {
        let core = self.core.borrow();
        let id = *core.active_window()?;
        let window = core.window(&id)?;
        let moving = core.interaction().is_some_and(|interaction| {
            interaction.id == id && interaction.phase == clarion_core::desktop::MovePhase::Moving
        });
        (!window.minimized && (window.attached || moving)).then_some(id)
    }

    pub(crate) fn core_focus_window(&self, id: u64) -> Option<&W> {
        let valid = {
            let core = self.core.borrow();
            core.window(&id).is_some_and(|window| {
                let moving = core.interaction().filter(|interaction| {
                    interaction.id == id
                        && interaction.phase == clarion_core::desktop::MovePhase::Moving
                });
                let connector = moving
                    .and_then(|interaction| interaction.output.as_deref())
                    .or(window.output.as_deref());
                !window.minimized
                    && (window.attached || moving.is_some())
                    && connector.is_some_and(|connector| {
                        self.monitors()
                            .any(|monitor| monitor.output_name() == connector)
                    })
            })
        };
        valid.then(|| self.window_for_core_id(id)).flatten()
    }

    fn window_for_core_id(&self, id: u64) -> Option<&W> {
        if let Some(InteractiveMoveState::Moving(moving)) = &self.interactive_move {
            if moving.tile.window().core_id() == id {
                return Some(moving.tile.window());
            }
        }
        let space = self.core.borrow().window(&id).map(|window| window.space)?;
        let workspace = self
            .workspaces()
            .find(|(_, _, workspace)| workspace.id().get() == space.0)?
            .2;
        if let Some(window) = workspace
            .active_window()
            .filter(|window| window.core_id() == id)
        {
            return Some(window);
        }
        workspace.windows().find(|window| window.core_id() == id)
    }

    pub fn active_workspace_mut(&mut self) -> Option<&mut Workspace<W>> {
        let MonitorSet::Normal { monitors, .. } = &mut self.monitor_set else {
            return None;
        };

        let selected_index = selected_monitor_idx(monitors);

        let mon = &mut monitors[selected_index];
        Some(mon.active_workspace())
    }

    pub fn windows_for_output(&self, output: &Output) -> impl Iterator<Item = &W> + '_ {
        let moving_window = self
            .interactive_move
            .as_ref()
            .and_then(|x| x.moving())
            .filter(|move_| move_.output == *output)
            .map(|move_| move_.tile.window())
            .into_iter();

        let mon_windows = self
            .monitor_for_output(output)
            .into_iter()
            .flat_map(|mon| mon.workspaces.iter().flat_map(|ws| ws.windows()));

        moving_window.chain(mon_windows)
    }

    pub fn windows_for_output_mut(&mut self, output: &Output) -> impl Iterator<Item = &mut W> + '_ {
        let moving_window = self
            .interactive_move
            .as_mut()
            .and_then(|x| x.moving_mut())
            .filter(|move_| move_.output == *output)
            .map(|move_| move_.tile.window_mut())
            .into_iter();

        let monitors = if let MonitorSet::Normal { monitors, .. } = &mut self.monitor_set {
            &mut monitors[..]
        } else {
            &mut [][..]
        };
        let mon_windows = monitors
            .iter_mut()
            .find(|mon| &mon.output == output)
            .into_iter()
            .flat_map(|mon| mon.workspaces.iter_mut().flat_map(|ws| ws.windows_mut()));

        moving_window.chain(mon_windows)
    }

    /// Visits every mapped window without computing its IPC geometry.
    ///
    /// Keep the same ordering and output attribution as `with_windows`, including
    /// parked windows and a tile detached by an interactive move. Protocol and
    /// capture bookkeeping need this membership even for windows not rendered.
    pub fn with_windows_and_outputs(&self, mut f: impl FnMut(&W, Option<&Output>)) {
        if let Some(InteractiveMoveState::Moving(move_)) = &self.interactive_move {
            f(move_.tile.window(), Some(&move_.output));
        }

        match &self.monitor_set {
            MonitorSet::Normal { monitors, .. } => {
                for mon in monitors {
                    for ws in &mon.workspaces {
                        ws.with_windows_in_ipc_order(&mut |window| f(window, Some(&mon.output)));
                    }
                }
            }
            MonitorSet::NoOutputs { workspaces } => {
                for ws in workspaces {
                    ws.with_windows_in_ipc_order(&mut |window| f(window, None));
                }
            }
        }
    }

    pub fn with_windows(
        &self,
        mut f: impl FnMut(&W, Option<&Output>, Option<WorkspaceId>, WindowLayout),
    ) {
        if let Some(InteractiveMoveState::Moving(move_)) = &self.interactive_move {
            // We don't fill any positions for interactively moved windows.
            let layout = move_.tile.ipc_layout_template();
            f(move_.tile.window(), Some(&move_.output), None, layout);
        }

        match &self.monitor_set {
            MonitorSet::Normal { monitors, .. } => {
                for mon in monitors {
                    for ws in &mon.workspaces {
                        for (tile, layout) in ws.tiles_with_ipc_layouts() {
                            f(tile.window(), Some(&mon.output), Some(ws.id()), layout);
                        }
                    }
                }
            }
            MonitorSet::NoOutputs { workspaces } => {
                for ws in workspaces {
                    for (tile, layout) in ws.tiles_with_ipc_layouts() {
                        f(tile.window(), None, Some(ws.id()), layout);
                    }
                }
            }
        }
    }

    pub fn with_windows_mut(&mut self, mut f: impl FnMut(&mut W, Option<&Output>)) {
        if let Some(InteractiveMoveState::Moving(move_)) = &mut self.interactive_move {
            f(move_.tile.window_mut(), Some(&move_.output));
        }

        match &mut self.monitor_set {
            MonitorSet::Normal { monitors, .. } => {
                for mon in monitors {
                    for ws in &mut mon.workspaces {
                        for win in ws.windows_mut() {
                            f(win, Some(&mon.output));
                        }
                    }
                }
            }
            MonitorSet::NoOutputs { workspaces } => {
                for ws in workspaces {
                    for win in ws.windows_mut() {
                        f(win, None);
                    }
                }
            }
        }
    }

    fn active_monitor(&mut self) -> Option<&mut Monitor<W>> {
        let MonitorSet::Normal { monitors, .. } = &mut self.monitor_set else {
            return None;
        };

        {
            let index = selected_monitor_idx(monitors);
            Some(&mut monitors[index])
        }
    }

    pub fn active_monitor_ref(&self) -> Option<&Monitor<W>> {
        let MonitorSet::Normal { monitors, .. } = &self.monitor_set else {
            return None;
        };

        Some(&monitors[selected_monitor_idx(monitors)])
    }

    pub fn active_workspace_idx(&self) -> Option<usize> {
        self.active_monitor_ref().map(|m| m.active_workspace_idx())
    }

    pub fn monitors(&self) -> impl Iterator<Item = &Monitor<W>> + '_ {
        let monitors = if let MonitorSet::Normal { monitors, .. } = &self.monitor_set {
            &monitors[..]
        } else {
            &[][..]
        };

        monitors.iter()
    }

    pub fn monitors_mut(&mut self) -> impl Iterator<Item = &mut Monitor<W>> + '_ {
        self.monitor_set.monitors_mut()
    }

    pub fn monitor_for_output(&self, output: &Output) -> Option<&Monitor<W>> {
        self.monitors().find(|mon| &mon.output == output)
    }

    pub fn monitor_for_output_mut(&mut self, output: &Output) -> Option<&mut Monitor<W>> {
        self.monitors_mut().find(|mon| &mon.output == output)
    }

    pub fn monitor_for_workspace(&self, workspace_name: &str) -> Option<&Monitor<W>> {
        let id = self.core.borrow().named_space(workspace_name)?;
        self.monitors().find(|monitor| {
            monitor
                .workspaces
                .iter()
                .any(|workspace| workspace.id().get() == id.0)
        })
    }

    pub fn outputs(&self) -> impl Iterator<Item = &Output> + '_ {
        self.monitors().map(|mon| &mon.output)
    }

    pub fn move_left(&mut self) {
        let Some(workspace) = self.active_workspace_mut() else {
            return;
        };
        workspace.move_left();
    }

    pub fn move_right(&mut self) {
        let Some(workspace) = self.active_workspace_mut() else {
            return;
        };
        workspace.move_right();
    }

    pub fn move_column_to_first(&mut self) {
        let Some(workspace) = self.active_workspace_mut() else {
            return;
        };
        workspace.move_column_to_first();
    }

    pub fn move_column_to_last(&mut self) {
        let Some(workspace) = self.active_workspace_mut() else {
            return;
        };
        workspace.move_column_to_last();
    }

    pub fn move_column_left_or_to_output(&mut self, output: &Output) -> bool {
        if let Some(workspace) = self.active_workspace_mut() {
            if workspace.move_left() {
                return false;
            }
        }

        self.move_column_to_output(output, None, true);
        true
    }

    pub fn move_column_right_or_to_output(&mut self, output: &Output) -> bool {
        if let Some(workspace) = self.active_workspace_mut() {
            if workspace.move_right() {
                return false;
            }
        }

        self.move_column_to_output(output, None, true);
        true
    }

    pub fn move_column_to_index(&mut self, index: usize) {
        let Some(workspace) = self.active_workspace_mut() else {
            return;
        };
        workspace.move_column_to_index(index);
    }

    pub fn move_down(&mut self) {
        let Some(workspace) = self.active_workspace_mut() else {
            return;
        };
        workspace.move_down();
    }

    pub fn move_up(&mut self) {
        let Some(workspace) = self.active_workspace_mut() else {
            return;
        };
        workspace.move_up();
    }

    pub fn move_down_or_to_workspace_down(&mut self) {
        let Some(monitor) = self.active_monitor() else {
            return;
        };
        monitor.move_down_or_to_workspace_down();
    }

    pub fn move_up_or_to_workspace_up(&mut self) {
        let Some(monitor) = self.active_monitor() else {
            return;
        };
        monitor.move_up_or_to_workspace_up();
    }

    pub fn consume_or_expel_window_left(&mut self, window: Option<&W::Id>) {
        if let Some(InteractiveMoveState::Moving(move_)) = &mut self.interactive_move {
            if window.is_none() || window == Some(move_.tile.window().id()) {
                return;
            }
        }

        let workspace = if let Some(window) = window {
            Some(
                self.workspaces_mut()
                    .find(|ws| ws.has_window(window))
                    .unwrap(),
            )
        } else {
            self.active_workspace_mut()
        };

        let Some(workspace) = workspace else {
            return;
        };
        workspace.consume_or_expel_window_left(window);
    }

    pub fn consume_or_expel_window_right(&mut self, window: Option<&W::Id>) {
        if let Some(InteractiveMoveState::Moving(move_)) = &mut self.interactive_move {
            if window.is_none() || window == Some(move_.tile.window().id()) {
                return;
            }
        }

        let workspace = if let Some(window) = window {
            Some(
                self.workspaces_mut()
                    .find(|ws| ws.has_window(window))
                    .unwrap(),
            )
        } else {
            self.active_workspace_mut()
        };

        let Some(workspace) = workspace else {
            return;
        };
        workspace.consume_or_expel_window_right(window);
    }

    pub fn focus_left(&mut self) {
        self.with_desktop_transition(|this| {
            let Some(workspace) = this.active_workspace_mut() else {
                return;
            };
            workspace.focus_left();
        })
    }

    pub fn focus_right(&mut self) {
        self.with_desktop_transition(|this| {
            let Some(workspace) = this.active_workspace_mut() else {
                return;
            };
            workspace.focus_right();
        })
    }

    pub fn focus_column_first(&mut self) {
        self.with_desktop_transition(|this| {
            let Some(workspace) = this.active_workspace_mut() else {
                return;
            };
            workspace.focus_column_first();
        })
    }

    pub fn focus_column_last(&mut self) {
        self.with_desktop_transition(|this| {
            let Some(workspace) = this.active_workspace_mut() else {
                return;
            };
            workspace.focus_column_last();
        })
    }

    pub fn focus_column_right_or_first(&mut self) {
        self.with_desktop_transition(|this| {
            let Some(workspace) = this.active_workspace_mut() else {
                return;
            };
            workspace.focus_column_right_or_first();
        })
    }

    pub fn focus_column_left_or_last(&mut self) {
        self.with_desktop_transition(|this| {
            let Some(workspace) = this.active_workspace_mut() else {
                return;
            };
            workspace.focus_column_left_or_last();
        })
    }

    pub fn focus_column(&mut self, index: usize) {
        self.with_desktop_transition(|this| {
            let Some(workspace) = this.active_workspace_mut() else {
                return;
            };
            workspace.focus_column(index);
        })
    }

    pub fn focus_window_up_or_output(&mut self, output: &Output) -> bool {
        self.with_desktop_transition(|this| {
            if let Some(workspace) = this.active_workspace_mut() {
                if workspace.focus_up() {
                    return false;
                }
            }

            this.focus_output(output);
            true
        })
    }

    pub fn focus_window_down_or_output(&mut self, output: &Output) -> bool {
        self.with_desktop_transition(|this| {
            if let Some(workspace) = this.active_workspace_mut() {
                if workspace.focus_down() {
                    return false;
                }
            }

            this.focus_output(output);
            true
        })
    }

    pub fn focus_column_left_or_output(&mut self, output: &Output) -> bool {
        self.with_desktop_transition(|this| {
            if let Some(workspace) = this.active_workspace_mut() {
                if workspace.focus_left() {
                    return false;
                }
            }

            this.focus_output(output);
            true
        })
    }

    pub fn focus_column_right_or_output(&mut self, output: &Output) -> bool {
        self.with_desktop_transition(|this| {
            if let Some(workspace) = this.active_workspace_mut() {
                if workspace.focus_right() {
                    return false;
                }
            }

            this.focus_output(output);
            true
        })
    }

    pub fn focus_window_in_column(&mut self, index: u8) {
        self.with_desktop_transition(|this| {
            let Some(workspace) = this.active_workspace_mut() else {
                return;
            };
            workspace.focus_window_in_column(index);
        })
    }

    pub fn focus_down(&mut self) {
        self.with_desktop_transition(|this| {
            let Some(workspace) = this.active_workspace_mut() else {
                return;
            };
            workspace.focus_down();
        })
    }

    pub fn focus_up(&mut self) {
        self.with_desktop_transition(|this| {
            let Some(workspace) = this.active_workspace_mut() else {
                return;
            };
            workspace.focus_up();
        })
    }

    pub fn focus_down_or_left(&mut self) {
        self.with_desktop_transition(|this| {
            let Some(workspace) = this.active_workspace_mut() else {
                return;
            };
            workspace.focus_down_or_left();
        })
    }

    pub fn focus_down_or_right(&mut self) {
        self.with_desktop_transition(|this| {
            let Some(workspace) = this.active_workspace_mut() else {
                return;
            };
            workspace.focus_down_or_right();
        })
    }

    pub fn focus_up_or_left(&mut self) {
        self.with_desktop_transition(|this| {
            let Some(workspace) = this.active_workspace_mut() else {
                return;
            };
            workspace.focus_up_or_left();
        })
    }

    pub fn focus_up_or_right(&mut self) {
        self.with_desktop_transition(|this| {
            let Some(workspace) = this.active_workspace_mut() else {
                return;
            };
            workspace.focus_up_or_right();
        })
    }

    pub fn focus_window_or_workspace_down(&mut self) {
        self.with_desktop_transition(|this| {
            let Some(monitor) = this.active_monitor() else {
                return;
            };
            monitor.focus_window_or_workspace_down();
        })
    }

    pub fn focus_window_or_workspace_up(&mut self) {
        self.with_desktop_transition(|this| {
            let Some(monitor) = this.active_monitor() else {
                return;
            };
            monitor.focus_window_or_workspace_up();
        })
    }

    pub fn focus_window_top(&mut self) {
        self.with_desktop_transition(|this| {
            let Some(workspace) = this.active_workspace_mut() else {
                return;
            };
            workspace.focus_window_top();
        })
    }

    pub fn focus_window_bottom(&mut self) {
        self.with_desktop_transition(|this| {
            let Some(workspace) = this.active_workspace_mut() else {
                return;
            };
            workspace.focus_window_bottom();
        })
    }

    pub fn focus_window_down_or_top(&mut self) {
        self.with_desktop_transition(|this| {
            let Some(workspace) = this.active_workspace_mut() else {
                return;
            };
            workspace.focus_window_down_or_top();
        })
    }

    pub fn focus_window_up_or_bottom(&mut self) {
        self.with_desktop_transition(|this| {
            let Some(workspace) = this.active_workspace_mut() else {
                return;
            };
            workspace.focus_window_up_or_bottom();
        })
    }

    pub fn move_to_workspace_up(&mut self, focus: bool) {
        self.with_desktop_transition(|this| {
            let Some(monitor) = this.active_monitor() else {
                return;
            };
            let activate = if focus {
                ActivateWindow::Smart
            } else {
                ActivateWindow::No
            };
            monitor.move_to_workspace_up(activate);
        })
    }

    pub fn move_to_workspace_down(&mut self, focus: bool) {
        self.with_desktop_transition(|this| {
            let Some(monitor) = this.active_monitor() else {
                return;
            };
            let activate = if focus {
                ActivateWindow::Smart
            } else {
                ActivateWindow::No
            };
            monitor.move_to_workspace_down(activate);
        })
    }

    pub fn move_to_workspace(
        &mut self,
        window: Option<&W::Id>,
        idx: usize,
        activate: ActivateWindow,
    ) {
        self.with_desktop_transition(|this| {
            if let Some(InteractiveMoveState::Moving(move_)) = &mut this.interactive_move {
                if window.is_none() || window == Some(move_.tile.window().id()) {
                    return;
                }
            }

            let monitor = if let Some(window) = window {
                match &mut this.monitor_set {
                    MonitorSet::Normal { monitors, .. } => monitors
                        .iter_mut()
                        .find(|mon| mon.has_window(window))
                        .unwrap(),
                    MonitorSet::NoOutputs { .. } => {
                        return;
                    }
                }
            } else {
                let Some(monitor) = this.active_monitor() else {
                    return;
                };
                monitor
            };
            monitor.move_to_workspace(window, idx, activate);
        })
    }

    pub fn move_column_to_workspace_up(&mut self, activate: bool) {
        self.with_desktop_transition(|this| {
            let Some(monitor) = this.active_monitor() else {
                return;
            };
            monitor.move_column_to_workspace_up(activate);
        })
    }

    pub fn move_column_to_workspace_down(&mut self, activate: bool) {
        self.with_desktop_transition(|this| {
            let Some(monitor) = this.active_monitor() else {
                return;
            };
            monitor.move_column_to_workspace_down(activate);
        })
    }

    pub fn move_column_to_workspace(&mut self, idx: usize, activate: bool) {
        self.with_desktop_transition(|this| {
            let Some(monitor) = this.active_monitor() else {
                return;
            };
            monitor.move_column_to_workspace(idx, activate);
        })
    }

    pub fn switch_workspace_up(&mut self) {
        self.with_desktop_transition(|this| {
            let Some(monitor) = this.active_monitor() else {
                return;
            };
            monitor.switch_workspace_up();
        })
    }

    pub fn switch_workspace_down(&mut self) {
        self.with_desktop_transition(|this| {
            let Some(monitor) = this.active_monitor() else {
                return;
            };
            monitor.switch_workspace_down();
        })
    }

    pub fn switch_workspace(&mut self, idx: usize) {
        self.with_desktop_transition(|this| {
            let Some(monitor) = this.active_monitor() else {
                return;
            };
            monitor.switch_workspace(idx);
        })
    }

    pub fn switch_workspace_auto_back_and_forth(&mut self, idx: usize) {
        self.with_desktop_transition(|this| {
            let Some(monitor) = this.active_monitor() else {
                return;
            };
            monitor.switch_workspace_auto_back_and_forth(idx);
        })
    }

    pub fn switch_workspace_previous(&mut self) {
        self.with_desktop_transition(|this| {
            let Some(monitor) = this.active_monitor() else {
                return;
            };
            monitor.switch_workspace_previous();
        })
    }

    pub fn consume_into_column(&mut self) {
        let Some(workspace) = self.active_workspace_mut() else {
            return;
        };
        workspace.consume_into_column();
    }

    pub fn expel_from_column(&mut self) {
        let Some(workspace) = self.active_workspace_mut() else {
            return;
        };
        workspace.expel_from_column();
    }

    pub fn swap_window_in_direction(&mut self, direction: ScrollDirection) {
        let Some(workspace) = self.active_workspace_mut() else {
            return;
        };
        workspace.swap_window_in_direction(direction);
    }

    pub fn toggle_column_tabbed_display(&mut self) {
        let Some(workspace) = self.active_workspace_mut() else {
            return;
        };
        workspace.toggle_column_tabbed_display();
    }

    pub fn toggle_column_dwindle_display(&mut self) {
        let Some(workspace) = self.active_workspace_mut() else {
            return;
        };
        workspace.toggle_column_dwindle_display();
    }

    pub fn set_column_display(&mut self, display: ColumnDisplay) {
        let Some(workspace) = self.active_workspace_mut() else {
            return;
        };
        workspace.set_column_display(display);
    }

    pub fn center_column(&mut self) {
        let Some(workspace) = self.active_workspace_mut() else {
            return;
        };
        workspace.center_column();
    }

    pub fn center_window(&mut self, id: Option<&W::Id>) {
        if let Some(InteractiveMoveState::Moving(move_)) = &mut self.interactive_move {
            if id.is_none() || id == Some(move_.tile.window().id()) {
                return;
            }
        }

        let workspace = if let Some(id) = id {
            self.workspaces_mut().find(|ws| ws.has_window(id))
        } else {
            self.active_workspace_mut()
        };

        let Some(workspace) = workspace else {
            return;
        };
        workspace.center_window(id);
    }

    pub fn center_visible_columns(&mut self) {
        let Some(workspace) = self.active_workspace_mut() else {
            return;
        };
        workspace.center_visible_columns();
    }

    pub fn focus(&self) -> Option<&W> {
        self.focus_with_output().map(|(win, _out)| win)
    }

    pub fn focus_with_output(&self) -> Option<(&W, &Output)> {
        let core = self.core.borrow();
        let id = *core.active_window()?;
        let connector = core
            .interaction()
            .filter(|interaction| {
                interaction.phase == clarion_core::desktop::MovePhase::Moving
                    && interaction.id == id
            })
            .and_then(|interaction| interaction.output.as_deref())
            .or_else(|| core.window(&id).and_then(|window| window.output.as_deref()))?;
        let output = self
            .monitors()
            .find(|monitor| monitor.output_name().as_str() == connector)?
            .output();
        self.window_for_core_id(id).map(|window| (window, output))
    }

    pub fn interactive_moved_window_under(
        &self,
        output: &Output,
        pos_within_output: Point<f64, Logical>,
    ) -> Option<(&W, HitType)> {
        if let Some(InteractiveMoveState::Moving(move_)) = &self.interactive_move {
            if move_.output == *output {
                if self.overview_progress.is_some() {
                    let zoom = self.interactive_move_zoom();
                    let tile_pos = move_.tile_render_location(zoom);
                    let pos_within_tile = (pos_within_output - tile_pos).downscale(zoom);
                    // During the overview animation, we cannot do input hits because we cannot
                    // really represent scaled windows properly.
                    let (win, hit) =
                        HitType::hit_tile(&move_.tile, Point::from((0., 0.)), pos_within_tile)?;
                    Some((win, hit.to_activate()))
                } else {
                    let tile_pos = move_.tile_render_location(1.);
                    HitType::hit_tile(&move_.tile, tile_pos, pos_within_output)
                }
            } else {
                None
            }
        } else {
            None
        }
    }

    /// Returns the window under the cursor and the hit type.
    pub fn window_under(
        &self,
        output: &Output,
        pos_within_output: Point<f64, Logical>,
    ) -> Option<(&W, HitType)> {
        let mon = self.monitor_for_output(output)?;
        mon.window_under(pos_within_output)
    }

    pub fn resize_edges_under(
        &self,
        output: &Output,
        pos_within_output: Point<f64, Logical>,
    ) -> Option<ResizeEdge> {
        let mon = self.monitor_for_output(output)?;
        mon.resize_edges_under(pos_within_output)
    }

    pub fn workspace_under(
        &self,
        extended_bounds: bool,
        output: &Output,
        pos_within_output: Point<f64, Logical>,
    ) -> Option<&Workspace<W>> {
        if self
            .interactive_moved_window_under(output, pos_within_output)
            .is_some()
        {
            return None;
        }

        let mon = self.monitor_for_output(output)?;
        if extended_bounds {
            mon.workspace_under(pos_within_output).map(|(ws, _)| ws)
        } else {
            mon.workspace_under_narrow(pos_within_output)
        }
    }

    fn overview_progress_value(&self) -> Option<f64> {
        let gesture_value = self.core.borrow().overview().gesture_value().unwrap_or(0.);
        self.overview_progress
            .as_ref()
            .map(|p| p.value(gesture_value))
    }

    pub fn overview_zoom(&self) -> f64 {
        let progress = self.overview_progress_value();
        compute_overview_zoom(&self.options, progress)
    }

    fn interactive_move_zoom(&self) -> f64 {
        match &self.interactive_move {
            Some(InteractiveMoveState::Moving(move_)) => self
                .monitors()
                .find(|mon| mon.output() == &move_.output)
                .map(|mon| {
                    let (_, geo) = mon.insert_position(move_.pointer_pos_within_output);
                    if mon.is_stage_visible() && (geo.size.w <= 0. || geo.size.h <= 0.) {
                        mon.stage_card_zoom()
                    } else {
                        let zoom = mon.workspace_render_zoom(geo);
                        if mon.is_stage_visible() {
                            zoom.max(0.01)
                        } else {
                            zoom
                        }
                    }
                }),
            Some(InteractiveMoveState::Starting { window_id, .. }) => {
                self.monitors().find_map(|mon| {
                    mon.workspaces_with_render_geo()
                        .find(|(ws, _)| ws.has_window(window_id))
                        .map(|(_, geo)| {
                            if mon.is_stage_visible() && (geo.size.w <= 0. || geo.size.h <= 0.) {
                                mon.stage_card_zoom()
                            } else {
                                let zoom = mon.workspace_render_zoom(geo);
                                if mon.is_stage_visible() {
                                    zoom.max(0.01)
                                } else {
                                    zoom
                                }
                            }
                        })
                })
            }
            None => None,
        }
        .unwrap_or_else(|| self.overview_zoom())
    }

    #[cfg(test)]
    pub(crate) fn verify_invariants(&self) {
        use std::collections::HashSet;

        use approx::assert_abs_diff_eq;

        let core = self.core.borrow();
        assert!(
            core.navigation().verify_invariants(),
            "core navigation topology must be consistent"
        );
        let output_names: Vec<_> = self
            .monitors()
            .map(|monitor| monitor.output_name().clone())
            .collect();
        assert_eq!(
            core.navigation().output_order(),
            output_names,
            "device output slots must follow core order"
        );
        for monitor in self.monitors() {
            let expected: Vec<_> = monitor
                .workspaces
                .iter()
                .map(|workspace| clarion_core::SpaceId(workspace.id().get()))
                .collect();
            assert_eq!(
                core.navigation()
                    .output(monitor.output_name())
                    .unwrap()
                    .spaces(),
                expected,
                "device workspace slots must follow core stable-ID topology"
            );
        }
        if let MonitorSet::NoOutputs { workspaces } = &self.monitor_set {
            let expected: Vec<_> = workspaces
                .iter()
                .map(|workspace| clarion_core::SpaceId(workspace.id().get()))
                .collect();
            assert_eq!(
                core.navigation().detached_spaces(),
                expected,
                "detached device slots must follow core stable-ID topology"
            );
        } else {
            assert!(
                core.navigation().detached_spaces().is_empty(),
                "completed connected desktop mutation cannot retain detached payload slots"
            );
        }
        let physical_spaces: HashSet<_> = self
            .workspaces()
            .map(|(_, _, workspace)| clarion_core::SpaceId(workspace.id().get()))
            .collect();
        let core_spaces: HashSet<_> = core.spaces().map(|space| space.id).collect();
        assert_eq!(
            core_spaces, physical_spaces,
            "retired workspace state must not leak into the desktop core"
        );
        let moving = match &self.interactive_move {
            Some(InteractiveMoveState::Starting { window_id, .. }) => self.core.id(window_id),
            Some(InteractiveMoveState::Moving(data)) => Some(data.tile.window().core_id()),
            None => None,
        };
        assert_eq!(
            core.interaction().map(|interaction| interaction.id),
            moving,
            "move identity belongs to the independent core"
        );
        for record in core.windows() {
            if record.attached {
                assert!(
                    self.workspaces()
                        .any(|(_, _, workspace)| workspace.id().get() == record.space.0
                            && workspace
                                .windows()
                                .any(|window| window.core_id() == record.id)),
                    "attached core state needs one physical payload"
                );
            } else {
                assert_eq!(
                    Some(record.id),
                    moving,
                    "only the active transfer may retain a detached mapped identity"
                );
            }
        }
        let expected_focus = match &self.interactive_move {
            Some(InteractiveMoveState::Moving(data)) if self.monitors().next().is_some() => {
                Some(data.tile.window().core_id())
            }
            _ => self
                .active_workspace()
                .and_then(Workspace::active_window)
                .map(LayoutElement::core_id),
        };
        assert_eq!(
            core.active_window().copied(),
            expected_focus,
            "global focus must agree with the active output/space or transfer"
        );
        for (_, _, workspace) in self.workspaces() {
            assert_eq!(
                core.space(clarion_core::SpaceId(workspace.id().get()))
                    .and_then(|space| space.focused),
                workspace.active_window().map(LayoutElement::core_id),
                "local focus memory must describe its physical policy payloads"
            );
        }
        drop(core);

        let zoom = self.interactive_move_zoom();

        let mut move_win_id = None;
        if let Some(state) = &self.interactive_move {
            match state {
                InteractiveMoveState::Starting { window_id } => {
                    assert!(
                        self.has_window(window_id),
                        "interactive move must be on an existing window"
                    );
                    move_win_id = Some(window_id.clone());
                }
                InteractiveMoveState::Moving(move_) => {
                    assert_eq!(self.clock, move_.tile.presentation.clock);
                    assert!(move_.tile.window().pending_sizing_mode().is_normal());

                    move_.tile.verify_invariants();

                    let scale = move_.output.current_scale().fractional_scale();
                    let options = Options::clone(&self.options)
                        .with_merged_layout(move_.output_config.as_ref())
                        .with_merged_layout(move_.workspace_config.as_ref().map(|(_, c)| c))
                        .adjusted_for_scale(scale);
                    assert_eq!(
                        &*move_.tile.presentation.options, &options,
                        "interactive moved tile options must be \
                         base options adjusted for output scale"
                    );

                    let tile_pos = move_.tile_render_location(zoom);
                    let rounded_pos = tile_pos.to_physical_precise_round(scale).to_logical(scale);

                    // Tile position must be rounded to physical pixels.
                    assert_abs_diff_eq!(tile_pos.x, rounded_pos.x, epsilon = 1e-5);
                    assert_abs_diff_eq!(tile_pos.y, rounded_pos.y, epsilon = 1e-5);

                    if let Some(alpha) = &move_.tile.presentation.alpha_animation {
                        if move_.is_floating() {
                            assert_eq!(
                                alpha.anim.to(),
                                1.,
                                "interactively moved floating tile can animate alpha only to 1"
                            );

                            assert!(
                                !alpha.hold_after_done,
                                "interactively moved floating tile \
                                 cannot have held alpha animation"
                            );
                        } else {
                            assert_ne!(
                                alpha.anim.to(),
                                1.,
                                "interactively moved scrolling tile must animate alpha to not 1"
                            );

                            assert!(
                                alpha.hold_after_done,
                                "interactively moved scrolling tile \
                                 must have held alpha animation"
                            );
                        }
                    }
                }
            }
        }

        let mut seen_workspace_id = HashSet::new();
        let mut seen_workspace_name = Vec::<String>::new();

        let monitors = match &self.monitor_set {
            MonitorSet::Normal { monitors } => monitors,
            MonitorSet::NoOutputs { workspaces } => {
                for workspace in workspaces {
                    assert!(
                        workspace.has_windows_or_name()
                            || Some(workspace.id()) == self.stage_drag_source_workspace(),
                        "with no outputs there cannot be empty unnamed workspaces"
                    );

                    assert_eq!(self.clock, workspace.clock);

                    assert_eq!(
                        workspace.base_options, self.options,
                        "workspace base options must be synchronized with layout"
                    );

                    assert!(
                        seen_workspace_id.insert(workspace.id()),
                        "workspace id must be unique"
                    );

                    if let Some(name) = workspace.name() {
                        assert!(
                            !seen_workspace_name
                                .iter()
                                .any(|n| n.eq_ignore_ascii_case(&name)),
                            "workspace name must be unique"
                        );
                        seen_workspace_name.push((*name).clone());
                    }

                    workspace.verify_invariants(move_win_id.as_ref());
                }

                return;
            }
        };

        assert!(primary_monitor_idx(monitors) < monitors.len());
        assert!(selected_monitor_idx(monitors) < monitors.len());

        let mut saw_view_offset_gesture = false;

        for (idx, monitor) in monitors.iter().enumerate() {
            assert_eq!(self.clock, monitor.clock);
            assert_eq!(
                monitor.base_options, self.options,
                "monitor base options must be synchronized with layout"
            );

            assert_eq!(self.is_overview_open(), monitor.is_overview_open());
            assert_eq!(
                self.overview_progress_value(),
                monitor.overview_progress_value()
            );

            monitor.verify_invariants();

            if idx == primary_monitor_idx(monitors) {
                for ws in &monitor.workspaces {
                    if ws.prefers_output(&monitor.output) {
                        // This is the primary monitor's own workspace.
                        continue;
                    }

                    let own_monitor_exists = monitors.iter().any(|m| ws.prefers_output(&m.output));
                    assert!(
                        !own_monitor_exists,
                        "primary monitor cannot have workspaces for which their own monitor exists"
                    );
                }
            } else {
                assert!(
                    monitor
                        .workspaces
                        .iter()
                        .any(|workspace| workspace.prefers_output(&monitor.output)),
                    "secondary monitor must not have any non-own workspaces"
                );
            }

            // FIXME: verify that primary doesn't have any workspaces for which their own monitor
            // exists.

            for workspace in &monitor.workspaces {
                assert!(
                    seen_workspace_id.insert(workspace.id()),
                    "workspace id must be unique"
                );

                if let Some(name) = workspace.name() {
                    assert!(
                        !seen_workspace_name
                            .iter()
                            .any(|n| n.eq_ignore_ascii_case(&name)),
                        "workspace name must be unique"
                    );
                    seen_workspace_name.push((*name).clone());
                }

                workspace.verify_invariants(move_win_id.as_ref());

                let has_view_offset_gesture = workspace.scrolling().view_offset().is_gesture();
                if self.dnd.is_some() || self.interactive_move.is_some() {
                    // We'd like to check that all workspaces have the gesture here, furthermore we
                    // want to check that they have the gesture only if the interactive move
                    // targets the scrolling layout. However, we cannot do that because we start
                    // and stop the gesture lazily. Otherwise the gesture code would pollute a lot
                    // of places like adding new workspaces, implicitly moving windows between
                    // floating and tiling on fullscreen, etc.
                    //
                    // assert!(
                    //     has_view_offset_gesture,
                    //     "during an interactive move in the scrolling layout, \
                    //      all workspaces should be in a view offset gesture"
                    // );
                } else if saw_view_offset_gesture {
                    assert!(
                        !has_view_offset_gesture,
                        "only one workspace can have an ongoing view offset gesture"
                    );
                }
                saw_view_offset_gesture = has_view_offset_gesture;
            }
        }
    }

    pub fn advance_animations(&mut self) {
        let _span = clarion_tracy_span!("Layout::advance_animations");

        let mut dnd_scroll = None;
        let mut is_dnd = false;
        if let Some(dnd) = &self.dnd {
            dnd_scroll = Some((dnd.output.clone(), dnd.pointer_pos_within_output, true));
            is_dnd = true;
        }

        let effect_zoom = self.interactive_move_zoom();
        if let Some(InteractiveMoveState::Moving(move_)) = &mut self.interactive_move {
            move_.tile.advance_animations();
            move_
                .tile
                .advance_effect_pose(move_.tile_render_location(effect_zoom));

            if dnd_scroll.is_none() {
                dnd_scroll = Some((
                    move_.output.clone(),
                    move_.pointer_pos_within_output,
                    !move_.is_floating(),
                ));
            }
        }

        let is_overview_open = self.is_overview_open();

        // Keep the card grid stable while arranging windows in Expo.
        if self.is_overview_open()
            && matches!(
                self.overview_mode(),
                OverviewMode::Expo | OverviewMode::Stage
            )
            && !is_dnd
        {
            dnd_scroll = None;
        }
        // Device hit testing supplies numeric candidates. Only the core owns
        // the stationary-hover target/timer and accepts an activation intent.
        if let Some((output, pos_within_output, is_scrolling)) = dnd_scroll {
            let observed = self.monitor_for_output_mut(&output).map(|mon| {
                let zoom = mon.overview_zoom();
                let mut scrolled = mon.dnd_scroll_gesture_scroll(pos_within_output, 1. / zoom);
                if is_scrolling {
                    if let Some((ws, geo)) = mon.workspace_under(pos_within_output) {
                        let idx = mon.idx_of_ws(ws.id()).unwrap();
                        let ws = &mut mon.workspaces[idx];
                        let ws_pos = Point::from((0., geo.loc.y));
                        scrolled |=
                            ws.dnd_scroll_gesture_scroll(pos_within_output - ws_pos, 1. / zoom);
                    }
                }
                let target = mon
                    .window_under(pos_within_output)
                    .map(|(win, _)| clarion_core::dnd_hold::DndTarget::Window(win.core_id()))
                    .or_else(|| {
                        mon.workspace_under_narrow(pos_within_output).map(|ws| {
                            clarion_core::dnd_hold::DndTarget::Workspace(clarion_core::SpaceId(
                                ws.id().get(),
                            ))
                        })
                    });
                (scrolled, target)
            });
            if is_dnd {
                if let Some((scrolled, target)) = observed {
                    let intent = self.core.borrow_mut().dnd_hold_action(
                        clarion_core::dnd_hold::DndHoldAction::Observe {
                            connector: output.name(),
                            target,
                            timestamp: self.clock.now_unadjusted(),
                            scrolled,
                        },
                    );
                    if let Some(intent) = intent {
                        let config = is_overview_open
                            .then_some(self.options.animations.overview_open_close.0);
                        if let Some(mon) = self.monitor_for_output_mut(&output) {
                            if let Some(index) =
                                mon.idx_of_ws(WorkspaceId::specific(intent.space.0))
                            {
                                let accepted = if let Some(id) = intent.window {
                                    let window = mon.workspaces[index]
                                        .tiles()
                                        .find(|tile| tile.window().core_id() == id)
                                        .map(|tile| tile.window().id().clone());
                                    window.is_some_and(|window| {
                                        mon.workspaces[index].activate_window(&window)
                                    })
                                } else {
                                    true
                                };
                                if accepted {
                                    mon.dnd_scroll_gesture_end();
                                    mon.activate_workspace_with_anim_config(index, config);
                                    self.focus_output(&output);
                                    if is_overview_open {
                                        self.close_overview();
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        if let Some(OverviewProgress::Animation(anim)) = &mut self.overview_progress {
            if anim.is_done() {
                self.core
                    .borrow_mut()
                    .overview_action(OverviewAction::PresentationSettled);
                if self.is_overview_open() {
                    self.overview_progress = Some(OverviewProgress::Open);
                } else {
                    self.overview_progress = None;
                }
            }
        }

        match &mut self.monitor_set {
            MonitorSet::Normal { monitors, .. } => {
                for mon in monitors {
                    mon.set_overview_progress(self.overview_progress.as_ref());
                    mon.advance_animations();
                }
            }
            MonitorSet::NoOutputs { workspaces, .. } => {
                for ws in workspaces {
                    ws.advance_animations();
                }
            }
        }
    }

    pub fn are_animations_ongoing(&self, output: Option<&Output>) -> bool {
        // Keep advancing animations if we might need to scroll the view.
        if let Some(dnd) = &self.dnd {
            if output.is_none_or(|output| *output == dnd.output) {
                return true;
            }
        }

        if let Some(InteractiveMoveState::Moving(move_)) = &self.interactive_move {
            if output.is_none_or(|output| *output == move_.output) {
                if move_.tile.are_animations_ongoing() {
                    return true;
                }

                // Keep advancing animations if we might need to scroll the view.
                if !move_.is_floating() || self.is_overview_open() {
                    return true;
                }
            }
        }

        if self
            .overview_progress
            .as_ref()
            .is_some_and(|p| p.is_animation())
        {
            return true;
        }

        for mon in self.monitors() {
            if output.is_some_and(|output| mon.output != *output) {
                continue;
            }

            if mon.are_animations_ongoing() {
                return true;
            }
        }

        false
    }

    pub fn are_transitions_ongoing(&self, output: Option<&Output>) -> bool {
        // Idle scheduling still needs edge scrolling, but only on the drag's output.
        if let Some(dnd) = &self.dnd {
            if output.is_none_or(|output| *output == dnd.output) {
                return true;
            }
        }

        if let Some(InteractiveMoveState::Moving(move_)) = &self.interactive_move {
            if output.is_none_or(|output| *output == move_.output)
                && (move_.tile.are_transitions_ongoing()
                    || !move_.is_floating()
                    || self.is_overview_open())
            {
                return true;
            }
        }

        if self
            .overview_progress
            .as_ref()
            .is_some_and(|p| p.is_animation())
        {
            return true;
        }

        for mon in self.monitors() {
            if output.is_some_and(|output| mon.output != *output) {
                continue;
            }

            if mon.are_transitions_ongoing() {
                return true;
            }
        }

        false
    }

    pub fn update_render_elements(&mut self, output: Option<&Output>) {
        let _span = clarion_tracy_span!("Layout::update_render_elements");

        self.update_render_elements_time = self.clock.now();

        let zoom = self.interactive_move_zoom();
        if let Some(InteractiveMoveState::Moving(move_)) = &mut self.interactive_move {
            if output.is_none_or(|output| move_.output == *output) {
                let pos_within_output = move_.tile_render_location(zoom);

                // We're not on any specific workspace so we can't compute a "workspace view" rect.
                // Let's instead compute a rect relative to the output.
                //
                // FIXME: we could make the colors match up better in the overview by figuring out
                // where a centered workspace would currently be, and computing the view rect
                // against that. Since most of the time the dragged window will be on a centered
                // workspace.
                let view_rect =
                    Rectangle::new(pos_within_output.upscale(-1.), output_size(&move_.output))
                        .downscale(zoom);

                move_.tile.update_render_elements(true, view_rect);
            }
        }

        self.update_insert_hint(output);

        let MonitorSet::Normal { monitors, .. } = &mut self.monitor_set else {
            if output.is_some() {
                error!("update_render_elements called with no monitors but Some output");
            }
            return;
        };

        for mon in monitors.iter_mut() {
            if output.is_none_or(|output| mon.output == *output) {
                let is_active = self.is_active
                    && mon.is_selected_output()
                    && !matches!(self.interactive_move, Some(InteractiveMoveState::Moving(_)));
                mon.set_overview_progress(self.overview_progress.as_ref());
                mon.update_render_elements(is_active);
            }
        }
    }

    /// Invalidate native pixel snapshots after device replacement or GLES fallback.
    /// This is an epoch boundary, including captures that span several outputs.
    pub(crate) fn on_native_renderer_replaced(&mut self) {
        if let Some(InteractiveMoveState::Moving(move_)) = &mut self.interactive_move {
            move_.tile.on_native_renderer_replaced();
        }
        for monitor in self.monitors() {
            monitor.clear_native_caches();
        }
        for workspace in self.workspaces_mut() {
            workspace.on_native_renderer_replaced();
        }
    }

    pub fn update_shaders(&mut self) {
        if let Some(InteractiveMoveState::Moving(move_)) = &mut self.interactive_move {
            move_.tile.update_shaders();
        }

        match &mut self.monitor_set {
            MonitorSet::Normal { monitors, .. } => {
                for mon in monitors {
                    mon.update_shaders();
                }
            }
            MonitorSet::NoOutputs { workspaces, .. } => {
                for ws in workspaces {
                    ws.update_shaders();
                }
            }
        }
    }

    fn update_insert_hint(&mut self, output: Option<&Output>) {
        let _span = clarion_tracy_span!("Layout::update_insert_hint");

        for mon in self.monitors_mut() {
            mon.insert_hint = None;
        }

        if !matches!(self.interactive_move, Some(InteractiveMoveState::Moving(_))) {
            return;
        }
        let Some(InteractiveMoveState::Moving(move_)) = self.interactive_move.take() else {
            unreachable!()
        };
        if output.is_some_and(|out| &move_.output != out) {
            self.interactive_move = Some(InteractiveMoveState::Moving(move_));
            return;
        }

        let _span = clarion_tracy_span!("Layout::update_insert_hint::update");

        if let Some(mon) = self.monitor_for_output_mut(&move_.output) {
            let (insert_ws, geo) = mon.insert_position(move_.pointer_pos_within_output);
            let zoom = mon.workspace_render_zoom(geo);
            match insert_ws {
                InsertWorkspace::Existing(ws_id) => {
                    let idx = mon.idx_of_ws(ws_id).unwrap();
                    let ws = &mut mon.workspaces[idx];
                    let pos_within_workspace =
                        (move_.pointer_pos_within_output - geo.loc).downscale(zoom);
                    let position = if move_.is_floating() {
                        InsertPosition::Floating
                    } else {
                        ws.scrolling_insert_position(pos_within_workspace)
                    };

                    let border_width = move_.tile.effective_border_width().unwrap_or(0.);
                    let corner_radius = move_
                        .tile
                        .resolved_corner_radius()
                        .expanded_by(border_width as f32);
                    mon.insert_hint = Some(InsertHint {
                        workspace: insert_ws,
                        position,
                        corner_radius,
                    });
                }
                InsertWorkspace::NewAt(_) => {
                    let position = if move_.is_floating() {
                        InsertPosition::Floating
                    } else {
                        InsertPosition::NewColumn(0)
                    };
                    mon.insert_hint = Some(InsertHint {
                        workspace: insert_ws,
                        position,
                        corner_radius: CornerRadius::default(),
                    });
                }
            }
        }

        self.interactive_move = Some(InteractiveMoveState::Moving(move_));
    }

    pub fn ensure_named_workspace(&mut self, ws_config: &WorkspaceConfig) {
        if self.find_workspace_by_name(&ws_config.name.0).is_some() {
            return;
        }

        let clock = self.clock.clone();
        let options = self.options.clone();

        match &mut self.monitor_set {
            MonitorSet::Normal { monitors } => {
                let mon_idx = ws_config
                    .open_on_output
                    .as_deref()
                    .map(|name| {
                        monitors
                            .iter_mut()
                            .position(|monitor| output_matches_name(&monitor.output, name))
                            .unwrap_or(primary_monitor_idx(monitors))
                    })
                    .unwrap_or(selected_monitor_idx(monitors));
                let mon = &mut monitors[mon_idx];

                let ws = Workspace::new_with_config(
                    mon.output.clone(),
                    Some(ws_config.clone()),
                    clock,
                    options,
                    self.scene_resources.clone(),
                );
                mon.insert_workspace(ws, 0, false);
            }
            MonitorSet::NoOutputs { workspaces } => {
                let ws = Workspace::new_with_config_no_outputs(
                    Some(ws_config.clone()),
                    clock,
                    options,
                    self.scene_resources.clone(),
                )
                .with_core(self.core.clone());
                let id = clarion_core::SpaceId(ws.id().get());
                workspaces.insert(0, ws);
                self.core.borrow_mut().reorder_detached_space(id, 0);
            }
        }
    }

    pub fn update_config(&mut self, config: &Config) {
        self.with_desktop_transition(|this| {
            this.flush_pending_move();
            // Update workspace-specific config for all named workspaces.
            for ws in this.workspaces_mut() {
                let matched_config = ws.name().and_then(|name| {
                    config
                        .workspaces
                        .iter()
                        .find(|workspace| workspace.name.0 == *name)
                        .map(|config| config.layout.clone().map(|part| part.0))
                });
                if let Some(layout_config) = matched_config {
                    ws.update_layout_config(layout_config);
                }
            }

            this.update_options(Options::from_config(config));
        })
    }

    fn update_options(&mut self, options: Options) {
        let options = Rc::new(options);

        if let Some(InteractiveMoveState::Moving(move_)) = &mut self.interactive_move {
            let view_size = output_size(&move_.output);
            let scale = move_.output.current_scale().fractional_scale();
            let options = Options::clone(&options)
                .with_merged_layout(move_.output_config.as_ref())
                .with_merged_layout(move_.workspace_config.as_ref().map(|(_, c)| c))
                .adjusted_for_scale(scale);
            move_.tile.update_config(view_size, scale, Rc::new(options));
        }

        match &mut self.monitor_set {
            MonitorSet::Normal { monitors, .. } => {
                for mon in monitors {
                    mon.update_config(options.clone());
                }
            }
            MonitorSet::NoOutputs { workspaces } => {
                for ws in workspaces {
                    ws.update_config(options.clone());
                }
            }
        }

        self.options = options;
    }

    /// Apply the session's accessibility preference to the shared animation
    /// clock. Configuration `animations.off` remains an independent override.
    pub fn set_reduced_motion(&mut self, enabled: bool) {
        if self.clock.reduced_motion() == enabled {
            return;
        }
        // Drain pure viewport controllers while instant completion still
        // applies, including when disabling the preference between frames.
        if !enabled {
            self.advance_animations();
        }
        self.clock.set_reduced_motion(enabled);
        if enabled {
            for workspace in self.workspaces_mut() {
                workspace.stop_canvas_pan();
            }
            self.advance_animations();
        }
    }

    pub fn reduced_motion(&self) -> bool {
        self.clock.reduced_motion()
    }

    /// Toggle interactive window wobble for the current compositor session.
    ///
    /// Reloading the configuration replaces this transient value with the configured one.
    pub fn toggle_window_wobble(&mut self) -> bool {
        let mut options = Options::clone(&self.options);
        options.animations.window_wobble.off = !options.animations.window_wobble.off;
        let enabled = !options.animations.window_wobble.off;
        self.update_options(options);
        enabled
    }

    /// Toggle the cube workspace transition for the current compositor session.
    pub fn toggle_workspace_cube(&mut self) -> bool {
        let mut options = Options::clone(&self.options);
        options.animations.workspace_switch.1.off = !options.animations.workspace_switch.1.off;
        let enabled = !options.animations.workspace_switch.1.off;
        self.update_options(options);
        enabled
    }

    /// Toggle Glide for both window opening and closing for the current compositor session.
    pub fn toggle_window_glide(&mut self) -> bool {
        let mut options = Options::clone(&self.options);
        let turn_off =
            !options.animations.window_open.glide.off || !options.animations.window_close.glide.off;
        options.animations.window_open.glide.off = turn_off;
        options.animations.window_close.glide.off = turn_off;
        self.update_options(options);
        !turn_off
    }

    pub fn toggle_width(&mut self, forwards: bool) {
        let Some(workspace) = self.active_workspace_mut() else {
            return;
        };
        workspace.toggle_width(forwards);
    }

    pub fn toggle_window_width(&mut self, window: Option<&W::Id>, forwards: bool) {
        if let Some(InteractiveMoveState::Moving(move_)) = &mut self.interactive_move {
            if window.is_none() || window == Some(move_.tile.window().id()) {
                return;
            }
        }

        let workspace = if let Some(window) = window {
            Some(
                self.workspaces_mut()
                    .find(|ws| ws.has_window(window))
                    .unwrap(),
            )
        } else {
            self.active_workspace_mut()
        };

        let Some(workspace) = workspace else {
            return;
        };
        workspace.toggle_window_width(window, forwards);
    }

    pub fn toggle_window_height(&mut self, window: Option<&W::Id>, forwards: bool) {
        if let Some(InteractiveMoveState::Moving(move_)) = &mut self.interactive_move {
            if window.is_none() || window == Some(move_.tile.window().id()) {
                return;
            }
        }

        let workspace = if let Some(window) = window {
            Some(
                self.workspaces_mut()
                    .find(|ws| ws.has_window(window))
                    .unwrap(),
            )
        } else {
            self.active_workspace_mut()
        };

        let Some(workspace) = workspace else {
            return;
        };
        workspace.toggle_window_height(window, forwards);
    }

    pub fn toggle_full_width(&mut self) {
        let Some(workspace) = self.active_workspace_mut() else {
            return;
        };
        workspace.toggle_full_width();
    }

    pub fn set_column_width(&mut self, change: SizeChange) {
        let Some(workspace) = self.active_workspace_mut() else {
            return;
        };
        workspace.set_column_width(change);
    }

    pub fn set_window_width(&mut self, window: Option<&W::Id>, change: SizeChange) {
        if let Some(InteractiveMoveState::Moving(move_)) = &mut self.interactive_move {
            if window.is_none() || window == Some(move_.tile.window().id()) {
                return;
            }
        }

        let workspace = if let Some(window) = window {
            Some(
                self.workspaces_mut()
                    .find(|ws| ws.has_window(window))
                    .unwrap(),
            )
        } else {
            self.active_workspace_mut()
        };

        let Some(workspace) = workspace else {
            return;
        };
        workspace.set_window_width(window, change);
    }

    pub fn set_window_height(&mut self, window: Option<&W::Id>, change: SizeChange) {
        if let Some(InteractiveMoveState::Moving(move_)) = &mut self.interactive_move {
            if window.is_none() || window == Some(move_.tile.window().id()) {
                return;
            }
        }

        let workspace = if let Some(window) = window {
            Some(
                self.workspaces_mut()
                    .find(|ws| ws.has_window(window))
                    .unwrap(),
            )
        } else {
            self.active_workspace_mut()
        };

        let Some(workspace) = workspace else {
            return;
        };
        workspace.set_window_height(window, change);
    }

    pub fn reset_window_height(&mut self, window: Option<&W::Id>) {
        if let Some(InteractiveMoveState::Moving(move_)) = &mut self.interactive_move {
            if window.is_none() || window == Some(move_.tile.window().id()) {
                return;
            }
        }

        let workspace = if let Some(window) = window {
            Some(
                self.workspaces_mut()
                    .find(|ws| ws.has_window(window))
                    .unwrap(),
            )
        } else {
            self.active_workspace_mut()
        };

        let Some(workspace) = workspace else {
            return;
        };
        workspace.reset_window_height(window);
    }

    pub fn expand_column_to_available_width(&mut self) {
        let Some(workspace) = self.active_workspace_mut() else {
            return;
        };
        workspace.expand_column_to_available_width();
    }

    pub fn toggle_window_floating(&mut self, window: Option<&W::Id>) {
        if let Some(InteractiveMoveState::Moving(move_)) = &mut self.interactive_move {
            if window.is_none() || window == Some(move_.tile.window().id()) {
                let id = move_.tile.window().core_id();
                let floating = !move_.is_floating();
                let Some(placement) = self.core.borrow_mut().set_move_floating(id, floating) else {
                    return;
                };

                // When going to floating, restore the floating window size.
                if placement.floating {
                    let floating_size = move_.tile.floating_window_size;
                    let win = move_.tile.window_mut();
                    let mut size =
                        floating_size.unwrap_or_else(|| win.expected_size().unwrap_or_default());

                    // Apply min/max size window rules. If requesting a concrete size, apply
                    // completely; if requesting (0, 0), apply only when min/max results in a fixed
                    // size.
                    let min_size = win.min_size();
                    let max_size = win.max_size();
                    size.w = ensure_min_max_size_maybe_zero(size.w, min_size.w, max_size.w);
                    size.h = ensure_min_max_size_maybe_zero(size.h, min_size.h, max_size.h);

                    win.request_size_once(size, true);

                    // Animate the tile back to opaque.
                    move_.tile.animate_alpha(
                        INTERACTIVE_MOVE_ALPHA,
                        1.,
                        self.options.animations.window_movement.0,
                    );

                    // Unlock the view on the workspaces.
                    for ws in self.workspaces_mut() {
                        ws.dnd_scroll_gesture_end();
                    }
                } else {
                    // Animate the tile back to semitransparent.
                    move_.tile.animate_alpha(
                        1.,
                        INTERACTIVE_MOVE_ALPHA,
                        self.options.animations.window_movement.0,
                    );
                    move_.tile.hold_alpha_animation_after_done();
                }

                return;
            }
        }

        let workspace = if let Some(window) = window {
            Some(
                self.workspaces_mut()
                    .find(|ws| ws.has_window(window))
                    .unwrap(),
            )
        } else {
            self.active_workspace_mut()
        };

        let Some(workspace) = workspace else {
            return;
        };
        workspace.toggle_window_floating(window);
    }

    pub fn switch_tiling_mode(&mut self, mode: Option<clarion_ipc::TilingMode>) {
        if let Some(workspace) = self.active_workspace_mut() {
            if let Some(target_mode) = mode {
                workspace.set_tiling_mode(target_mode);
            } else {
                workspace.toggle_tiling_mode();
            }
        }
    }

    pub fn toggle_dwindle_split(&mut self) {
        if let Some(workspace) = self.active_workspace_mut() {
            workspace.toggle_dwindle_split();
        }
    }

    pub fn set_window_floating(&mut self, window: Option<&W::Id>, floating: bool) {
        if let Some(InteractiveMoveState::Moving(move_)) = &mut self.interactive_move {
            if window.is_none() || window == Some(move_.tile.window().id()) {
                if move_.is_floating() != floating {
                    self.toggle_window_floating(window);
                }
                return;
            }
        }

        let workspace = if let Some(window) = window {
            self.workspaces_mut().find(|ws| ws.has_window(window))
        } else {
            self.active_workspace_mut()
        };

        let Some(workspace) = workspace else {
            return;
        };
        workspace.set_window_floating(window, floating);
    }

    pub fn focus_floating(&mut self) {
        self.with_desktop_transition(|this| {
            let Some(workspace) = this.active_workspace_mut() else {
                return;
            };
            workspace.focus_floating();
        })
    }

    pub fn focus_tiling(&mut self) {
        self.with_desktop_transition(|this| {
            let Some(workspace) = this.active_workspace_mut() else {
                return;
            };
            workspace.focus_tiling();
        })
    }

    pub fn switch_focus_floating_tiling(&mut self) {
        let Some(workspace) = self.active_workspace_mut() else {
            return;
        };
        workspace.switch_focus_floating_tiling();
    }

    pub fn move_floating_window(
        &mut self,
        id: Option<&W::Id>,
        x: PositionChange,
        y: PositionChange,
        animate: bool,
    ) {
        if let Some(InteractiveMoveState::Moving(move_)) = &mut self.interactive_move {
            if id.is_none() || id == Some(move_.tile.window().id()) {
                return;
            }
        }

        let workspace = if let Some(id) = id {
            self.workspaces_mut().find(|ws| ws.has_window(id))
        } else {
            self.active_workspace_mut()
        };

        let Some(workspace) = workspace else {
            return;
        };
        workspace.move_floating_window(id, x, y, animate);
    }

    pub fn focus_output(&mut self, output: &Output) {
        self.with_desktop_transition(|this| {
            if let MonitorSet::Normal { monitors, .. } = &mut this.monitor_set {
                for (idx, mon) in monitors.iter().enumerate() {
                    if &mon.output == output {
                        monitors[idx].select_output();
                        return;
                    }
                }
            }
        })
    }

    pub fn move_to_output(
        &mut self,
        window: Option<&W::Id>,
        output: &Output,
        target_ws_idx: Option<usize>,
        activate: ActivateWindow,
    ) {
        self.with_desktop_transition(|this| {
            if let Some(InteractiveMoveState::Moving(move_)) = &mut this.interactive_move {
                if window.is_none() || window == Some(move_.tile.window().id()) {
                    return;
                }
            }

            if let MonitorSet::Normal { monitors, .. } = &mut this.monitor_set {
                let new_idx = monitors
                    .iter()
                    .position(|mon| &mon.output == output)
                    .unwrap();

                let (mon_idx, ws_idx) = if let Some(window) = window {
                    monitors
                        .iter()
                        .enumerate()
                        .find_map(|(mon_idx, mon)| {
                            mon.workspaces
                                .iter()
                                .position(|ws| ws.has_window(window))
                                .map(|ws_idx| (mon_idx, ws_idx))
                        })
                        .unwrap()
                } else {
                    let mon_idx = selected_monitor_idx(monitors);
                    let mon = &monitors[mon_idx];
                    (mon_idx, mon.active_workspace_idx())
                };

                let workspace_idx =
                    target_ws_idx.unwrap_or(monitors[new_idx].active_workspace_idx());
                if mon_idx == new_idx && ws_idx == workspace_idx {
                    return;
                }

                let mon = &monitors[new_idx];
                if mon.workspaces.len() <= workspace_idx {
                    return;
                }

                let ws_id = mon.workspaces[workspace_idx].id();

                let mon = &mut monitors[mon_idx];
                let activate = activate.map_smart(|| {
                    window.is_none_or(|win| {
                        mon.is_selected_output()
                            && mon.active_window().map(|win| win.id()) == Some(win)
                    })
                });
                let activate = if activate {
                    ActivateWindow::Yes
                } else {
                    ActivateWindow::No
                };

                let ws = &mut mon.workspaces[ws_idx];
                let Some(window) = window.or_else(|| ws.active_window().map(|win| win.id())) else {
                    return;
                };
                let window = window.clone();

                let transaction = Transaction::new();
                let mut removed = ws.remove_tile(&window, transaction);

                removed.tile.stop_move_animations();

                let mon = &mut monitors[new_idx];
                mon.add_tile(
                    removed.tile,
                    MonitorAddWindowTarget::Workspace {
                        id: ws_id,
                        column_idx: None,
                    },
                    activate,
                    true,
                    removed.width,
                    removed.is_full_width,
                    removed.is_floating,
                    None,
                );
                if activate.map_smart(|| false) {
                    monitors[new_idx].select_output();
                }

                let mon = &mut monitors[mon_idx];
                if mon.workspace_switch.is_none() {
                    monitors[mon_idx].clean_up_workspaces();
                }
            }
        })
    }

    pub fn move_column_to_output(
        &mut self,
        output: &Output,
        target_ws_idx: Option<usize>,
        activate: bool,
    ) {
        self.with_desktop_transition(|this| {
            if let MonitorSet::Normal { monitors, .. } = &mut this.monitor_set {
                let new_idx = monitors
                    .iter()
                    .position(|mon| &mon.output == output)
                    .unwrap();

                let selected_index = selected_monitor_idx(monitors);

                let current = &mut monitors[selected_index];
                let ws = current.active_workspace();

                if ws.floating_is_active() {
                    this.move_to_output(None, output, None, ActivateWindow::Smart);
                    return;
                }

                let Some(column) = ws.remove_active_column() else {
                    return;
                };

                let workspace_idx = target_ws_idx
                    .unwrap_or(monitors[new_idx].active_workspace_idx())
                    .min(monitors[new_idx].workspaces.len() - 1);
                this.add_column_by_idx(new_idx, workspace_idx, column, activate);
            }
        })
    }

    pub fn move_workspace_to_output(&mut self, output: &Output) -> bool {
        self.with_desktop_transition(|this| {
            let MonitorSet::Normal { monitors, .. } = &this.monitor_set else {
                return false;
            };

            let idx = monitors[selected_monitor_idx(monitors)].active_workspace_idx();
            this.move_workspace_to_output_by_id(idx, None, output)
        })
    }

    // FIXME: accept workspace by id
    pub fn move_workspace_to_output_by_id(
        &mut self,
        old_idx: usize,
        old_output: Option<Output>,
        new_output: &Output,
    ) -> bool {
        self.with_desktop_transition(|this| {
            let MonitorSet::Normal { monitors, .. } = &mut this.monitor_set else {
                return false;
            };

            let current_idx = if let Some(old_output) = old_output {
                monitors
                    .iter()
                    .position(|mon| mon.output == old_output)
                    .unwrap()
            } else {
                selected_monitor_idx(monitors)
            };
            let target_idx = monitors
                .iter()
                .position(|mon| mon.output == *new_output)
                .unwrap();

            let current = &mut monitors[current_idx];

            if current.workspaces.len() <= old_idx {
                return false;
            }

            // Do not do anything if the output is already correct
            if current_idx == target_idx {
                // Just update the original output since this is an explicit movement action.
                current.workspaces[old_idx].prefer_output(&current.output);

                return false;
            }

            // Only switch active monitor if the workspace to be moved is the currently focused one
            // on the current monitor.
            let activate =
                current.is_selected_output() && old_idx == current.active_workspace_idx();

            let ws = current.remove_workspace_by_idx(old_idx);
            ws.prefer_output(new_output);

            let target = &mut monitors[target_idx];
            target.insert_workspace(ws, target.active_workspace_idx() + 1, activate);

            if activate {
                monitors[target_idx].select_output();
            }

            activate
        })
    }

    pub fn set_fullscreen(&mut self, id: &W::Id, is_fullscreen: bool) {
        self.with_desktop_transition(|this| {
            // Check if this is a request to unset the windowed fullscreen state.
            if !is_fullscreen {
                let mut handled = false;
                this.with_windows_mut(|window, _| {
                    if window.id() == id && window.is_pending_windowed_fullscreen() {
                        window.request_windowed_fullscreen(false);
                        handled = true;
                    }
                });
                if handled {
                    return;
                }
            }

            if let Some(InteractiveMoveState::Moving(move_)) = &this.interactive_move {
                if move_.tile.window().id() == id {
                    return;
                }
            }

            for ws in this.workspaces_mut() {
                if ws.has_window(id) {
                    ws.set_fullscreen(id, is_fullscreen);
                    return;
                }
            }
        })
    }

    pub fn toggle_fullscreen(&mut self, id: &W::Id) {
        self.with_desktop_transition(|this| {
            if let Some(InteractiveMoveState::Moving(move_)) = &this.interactive_move {
                if move_.tile.window().id() == id {
                    return;
                }
            }

            for ws in this.workspaces_mut() {
                if ws.has_window(id) {
                    ws.toggle_fullscreen(id);
                    return;
                }
            }
        })
    }

    pub fn toggle_windowed_fullscreen(&mut self, id: &W::Id) {
        self.with_desktop_transition(|this| {
            let Some((_, window)) = this.windows().find(|(_, win)| win.id() == id) else {
                return;
            };
            if window.pending_sizing_mode().is_fullscreen() {
                // Remove the real fullscreen.
                for ws in this.workspaces_mut() {
                    if ws.has_window(id) {
                        ws.set_fullscreen(id, false);
                        break;
                    }
                }
            }

            // This will switch is_pending_fullscreen() to false right away.
            this.with_windows_mut(|window, _| {
                if window.id() == id {
                    window.request_windowed_fullscreen(!window.is_pending_windowed_fullscreen());
                }
            });
        })
    }

    pub fn set_maximized(&mut self, id: &W::Id, maximize: bool) {
        self.with_desktop_transition(|this| {
            if let Some(InteractiveMoveState::Moving(move_)) = &this.interactive_move {
                if move_.tile.window().id() == id {
                    return;
                }
            }

            for ws in this.workspaces_mut() {
                if ws.has_window(id) {
                    ws.set_maximized(id, maximize);
                    return;
                }
            }
        })
    }

    pub fn toggle_maximized(&mut self, id: &W::Id) {
        self.with_desktop_transition(|this| {
            if let Some(InteractiveMoveState::Moving(move_)) = &this.interactive_move {
                if move_.tile.window().id() == id {
                    return;
                }
            }

            for ws in this.workspaces_mut() {
                if ws.has_window(id) {
                    ws.toggle_maximized(id);
                    return;
                }
            }
        })
    }

    pub fn workspace_switch_gesture_begin(&mut self, output: &Output, is_touchpad: bool) {
        self.with_desktop_transition(|this| {
            let monitors = match &mut this.monitor_set {
                MonitorSet::Normal { monitors, .. } => monitors,
                MonitorSet::NoOutputs { .. } => return,
            };

            for monitor in monitors {
                // Cancel the gesture on other outputs.
                if &monitor.output != output {
                    monitor.workspace_switch_gesture_cancel(None);
                    continue;
                }

                monitor.workspace_switch_gesture_begin(is_touchpad);
            }
        })
    }

    pub fn workspace_switch_gesture_update(
        &mut self,
        delta_y: f64,
        timestamp: Duration,
        is_touchpad: bool,
    ) -> Option<Option<Output>> {
        self.with_desktop_transition(|this| {
            let monitors = match &mut this.monitor_set {
                MonitorSet::Normal { monitors, .. } => monitors,
                MonitorSet::NoOutputs { .. } => return None,
            };

            for monitor in monitors {
                if let Some(refresh) =
                    monitor.workspace_switch_gesture_update(delta_y, timestamp, is_touchpad)
                {
                    if refresh {
                        return Some(Some(monitor.output.clone()));
                    } else {
                        return Some(None);
                    }
                }
            }

            None
        })
    }

    pub fn workspace_switch_gesture_cancel(&mut self, is_touchpad: Option<bool>) -> Option<Output> {
        self.with_desktop_transition(|this| {
            this.monitors_mut().find_map(|monitor| {
                monitor
                    .workspace_switch_gesture_cancel(is_touchpad)
                    .then(|| monitor.output.clone())
            })
        })
    }

    pub fn workspace_switch_gesture_end(&mut self, is_touchpad: Option<bool>) -> Option<Output> {
        self.with_desktop_transition(|this| {
            let monitors = match &mut this.monitor_set {
                MonitorSet::Normal { monitors, .. } => monitors,
                MonitorSet::NoOutputs { .. } => return None,
            };

            for monitor in monitors {
                if monitor.workspace_switch_gesture_end(is_touchpad) {
                    return Some(monitor.output.clone());
                }
            }

            None
        })
    }

    pub fn view_offset_gesture_begin(
        &mut self,
        output: &Output,
        workspace_idx: Option<usize>,
        is_touchpad: bool,
    ) {
        let monitors = match &mut self.monitor_set {
            MonitorSet::Normal { monitors, .. } => monitors,
            MonitorSet::NoOutputs { .. } => return,
        };

        for monitor in monitors {
            let active_workspace = monitor.active_workspace_idx();
            for (idx, ws) in monitor.workspaces.iter_mut().enumerate() {
                // Cancel the gesture on other workspaces.
                if &monitor.output != output || idx != workspace_idx.unwrap_or(active_workspace) {
                    ws.view_offset_gesture_cancel(None);
                    continue;
                }

                ws.view_offset_gesture_begin(is_touchpad);
            }
        }
    }

    pub fn view_offset_gesture_update(
        &mut self,
        delta_x: f64,
        timestamp: Duration,
        is_touchpad: bool,
    ) -> Option<Option<Output>> {
        let zoom = self.overview_zoom();
        let delta_x = delta_x / zoom;

        let monitors = match &mut self.monitor_set {
            MonitorSet::Normal { monitors, .. } => monitors,
            MonitorSet::NoOutputs { .. } => return None,
        };

        for monitor in monitors {
            for ws in &mut monitor.workspaces {
                if let Some(refresh) =
                    ws.view_offset_gesture_update(delta_x, timestamp, is_touchpad)
                {
                    if refresh {
                        return Some(Some(monitor.output.clone()));
                    } else {
                        return Some(None);
                    }
                }
            }
        }

        None
    }

    pub fn view_offset_gesture_cancel(&mut self, is_touchpad: Option<bool>) -> Option<Output> {
        self.monitors_mut().find_map(|monitor| {
            monitor
                .workspaces
                .iter_mut()
                .any(|ws| ws.view_offset_gesture_cancel(is_touchpad))
                .then(|| monitor.output.clone())
        })
    }

    pub fn view_offset_gesture_end(&mut self, is_touchpad: Option<bool>) -> Option<Output> {
        let monitors = match &mut self.monitor_set {
            MonitorSet::Normal { monitors, .. } => monitors,
            MonitorSet::NoOutputs { .. } => return None,
        };

        for monitor in monitors {
            for ws in &mut monitor.workspaces {
                if ws.view_offset_gesture_end(is_touchpad) {
                    return Some(monitor.output.clone());
                }
            }
        }

        None
    }

    pub fn overview_gesture_begin(&mut self) {
        self.with_desktop_transition(|this| {
            this.complete_minimize_animations();
            this.cancel_overview_navigation_gestures();
            if !this.is_overview_visible() {
                let mode = this.default_overview_mode();
                if mode == OverviewMode::Stage {
                    this.prepare_stage_entry();
                }
                this.set_overview_mode(mode);
            }
            let value = this.overview_progress_value().unwrap_or(0.);
            this.core
                .borrow_mut()
                .overview_action(OverviewAction::GestureBegin { value });
            this.overview_progress = Some(OverviewProgress::Gesture);
            this.set_monitors_overview_state();
        })
    }

    pub fn overview_gesture_update(&mut self, delta_y: f64, timestamp: Duration) -> Option<bool> {
        self.with_desktop_transition(|this| {
            this.core.borrow().overview().gesture_value()?;
            let changed = this
                .core
                .borrow_mut()
                .overview_action(OverviewAction::GestureUpdate { delta_y, timestamp })
                .changed;
            if changed {
                this.set_monitors_overview_state();
            }
            Some(changed)
        })
    }

    fn apply_overview_settlement(&mut self, settlement: OverviewSettlement) {
        self.overview_progress = Some(OverviewProgress::Animation(Animation::new(
            self.clock.clone(),
            settlement.from,
            settlement.to,
            settlement.velocity,
            self.options.animations.overview_open_close.0,
        )));
    }

    pub fn overview_gesture_end(&mut self) -> bool {
        self.with_desktop_transition(|this| {
            let outcome = this
                .core
                .borrow_mut()
                .overview_action(OverviewAction::GestureEnd {
                    timestamp: this.clock.now_unadjusted(),
                });
            let Some(settlement) = outcome.settlement else {
                return false;
            };
            this.apply_overview_settlement(settlement);
            this.set_monitors_overview_state();
            true
        })
    }

    pub fn overview_gesture_cancel(&mut self) -> bool {
        self.with_desktop_transition(|this| {
            let outcome = this
                .core
                .borrow_mut()
                .overview_action(OverviewAction::GestureCancel {
                    timestamp: this.clock.now_unadjusted(),
                });
            let Some(settlement) = outcome.settlement else {
                return false;
            };
            this.apply_overview_settlement(settlement);
            this.set_monitors_overview_state();
            true
        })
    }

    pub fn interactive_move_begin(
        &mut self,
        window_id: W::Id,
        output: &Output,
        start_pos_within_output: Point<f64, Logical>,
    ) -> bool {
        self.with_desktop_transition(|this| {
            if this.interactive_move.is_some()
                || (this.is_overview_open() && this.overview_mode() == OverviewMode::Windows)
            {
                return false;
            }

            this.checkpoint_layout();

            let Some((mon, (ws, ws_geo))) = this.monitors().find_map(|mon| {
                mon.workspaces_with_render_geo()
                    .find(|(ws, _)| ws.has_window(&window_id))
                    .map(|rv| (mon, rv))
            }) else {
                return false;
            };

            if mon.output() != output {
                return false;
            }

            let zoom = mon.workspace_render_zoom(ws_geo);

            let is_floating = ws.is_floating(&window_id);
            let (tile, tile_offset, _visible) = ws
                .tiles_with_render_positions()
                .find(|(tile, _, _)| tile.window().id() == &window_id)
                .unwrap();
            let window_offset = tile.window_loc();

            let tile_pos = ws_geo.loc + tile_offset.upscale(zoom);

            let window_size = tile.window_size().upscale(zoom);
            let window_pos = tile_pos + window_offset.upscale(zoom);
            let (width, full_width) = ws
                .scrolling()
                .move_preferences(tile.window().core_id())
                .unwrap_or_else(|| {
                    if is_floating {
                        (
                            ColumnWidth::Fixed(tile.tile_expected_or_current_size().w),
                            false,
                        )
                    } else {
                        (ColumnWidth::Proportion(1.), false)
                    }
                });
            let start = clarion_core::move_controls::MoveStart {
                pointer: [start_pos_within_output.x, start_pos_within_output.y],
                visual_window: clarion_core::Rect::new(
                    window_pos.x,
                    window_pos.y,
                    window_size.w,
                    window_size.h,
                ),
                width,
                full_width,
            };
            this.capture_stage_drag_origin(&window_id);
            let Some(id) = this.core.id(&window_id) else {
                return false;
            };
            this.core
                .borrow_mut()
                .apply(clarion_core::CoreEvent::BeginMove { id, start });
            if this.core.borrow().interaction().is_none() {
                this.discard_stage_drag_origin(&window_id);
                return false;
            }
            this.interactive_move = Some(InteractiveMoveState::Starting { window_id });

            for mon in this.monitors_mut() {
                mon.dnd_scroll_gesture_begin();
            }

            // Lock the view for scrolling interactive move.
            if !is_floating {
                for ws in this.workspaces_mut() {
                    ws.dnd_scroll_gesture_begin();
                }
            }

            true
        })
    }

    /// Coalesce only compositor-owned move work; Wayland motion is delivered immediately.
    pub fn queue_interactive_move_update(
        &mut self,
        window: &W::Id,
        delta: Point<f64, Logical>,
        output: Output,
        pointer_pos: Point<f64, Logical>,
    ) -> bool {
        let matches = self
            .interactive_move
            .as_ref()
            .is_some_and(|state| match state {
                InteractiveMoveState::Starting { window_id, .. } => window_id == window,
                InteractiveMoveState::Moving(data) => data.tile.window().id() == window,
            });
        if !matches {
            return false;
        }
        if !pointer_pos.x.is_finite()
            || !pointer_pos.y.is_finite()
            || !delta.x.is_finite()
            || !delta.y.is_finite()
        {
            return true;
        }
        if matches!(
            self.interactive_move,
            Some(InteractiveMoveState::Starting { .. })
        ) {
            // Threshold crossing changes grab ownership and must be visible to
            // the current input event. Only established drag poses are batched.
            return self.interactive_move_update(window, delta, output, pointer_pos);
        }
        let pending = self.pending_move.get_or_insert_with(|| PendingMove {
            window: window.clone(),
            motion: Default::default(),
        });
        pending
            .motion
            .push((delta.x, delta.y), (output, pointer_pos));
        true
    }

    /// Apply the newest pose once before constructing a scene or handling a discrete event.
    pub fn flush_pending_move(&mut self) {
        let Some(mut pending) = self.pending_move.take() else {
            return;
        };
        let Some(batch) = pending.motion.take() else {
            return;
        };
        let (output, pointer_pos) = batch.latest;
        if self.monitor_for_output(&output).is_none() {
            return;
        }
        self.apply_interactive_move_update(
            &pending.window,
            Point::from(batch.delta),
            output,
            pointer_pos,
        );
    }

    pub fn interactive_move_update(
        &mut self,
        window: &W::Id,
        delta: Point<f64, Logical>,
        output: Output,
        pointer_pos_within_output: Point<f64, Logical>,
    ) -> bool {
        self.with_desktop_transition(|this| {
            this.flush_pending_move();
            this.apply_interactive_move_update(window, delta, output, pointer_pos_within_output)
        })
    }

    fn apply_interactive_move_update(
        &mut self,
        window: &W::Id,
        delta: Point<f64, Logical>,
        output: Output,
        pointer_pos_within_output: Point<f64, Logical>,
    ) -> bool {
        let starting_zoom = self.interactive_move_zoom();
        let Some(state) = self.interactive_move.take() else {
            return false;
        };

        match state {
            InteractiveMoveState::Starting { window_id } => {
                if window_id != *window {
                    self.interactive_move = Some(InteractiveMoveState::Starting { window_id });
                    return false;
                }
                let zoom = starting_zoom;
                let Some(id) = self.core.id(&window_id) else {
                    return false;
                };
                let intent = self
                    .core
                    .borrow_mut()
                    .start_move_motion(id, [delta.x, delta.y], zoom);
                let Some(intent) = intent else {
                    self.interactive_move = Some(InteractiveMoveState::Starting { window_id });
                    return false;
                };
                let (tile, workspace_config) = self
                    .workspaces_mut()
                    .find(|ws| ws.has_window(&window_id))
                    .map(|ws| {
                        let config = ws.layout_config().cloned().map(|c| (ws.id(), c));
                        (
                            ws.tiles_mut()
                                .find(|tile| *tile.window().id() == window_id)
                                .unwrap(),
                            config,
                        )
                    })
                    .unwrap();
                let new_offset = Point::from((intent.offset[0], intent.offset[1]));
                let visual_delta = new_offset - tile.interactive_move_offset;
                tile.start_wobble(WobbleAnchor::Point((intent.anchor[0], intent.anchor[1])));
                tile.add_pending_wobble_displacement(visual_delta);
                tile.interactive_move_offset = new_offset;
                self.interactive_move = Some(InteractiveMoveState::Starting {
                    window_id: window_id.clone(),
                });
                if !intent.detach {
                    return true;
                }

                let output_config = self
                    .monitors()
                    .find(|mon| mon.output() == &output)
                    .and_then(|mon| mon.layout_config().cloned());

                // If the pointer is currently on the window's own output, then we can animate the
                // window movement from its current (rubberbanded and possibly moved away) position
                // to the pointer. Otherwise, we just teleport it as the layout code is not aware
                // of monitor positions.
                //
                // FIXME: when and if the layout code knows about monitor positions, this will be
                // potentially animatable.
                let mut tile_pos = None;
                if let Some((mon, (ws, ws_geo))) = self.monitors().find_map(|mon| {
                    mon.workspaces_with_render_geo()
                        .find(|(ws, _)| ws.has_window(window))
                        .map(|rv| (mon, rv))
                }) {
                    if mon.output() == &output {
                        let (_, tile_offset, _) = ws
                            .tiles_with_render_positions()
                            .find(|(tile, _, _)| tile.window().id() == window)
                            .unwrap();

                        let zoom = mon.workspace_render_zoom(ws_geo);
                        tile_pos = Some((ws_geo.loc + tile_offset.upscale(zoom), zoom));
                    }
                }

                if let Some(id) = self.core.id(window) {
                    let geometry = self
                        .core
                        .borrow()
                        .window(&id)
                        .map(|window| window.geometry)
                        .unwrap_or_default();
                    self.core
                        .borrow_mut()
                        .apply(clarion_core::CoreEvent::MovePose {
                            id,
                            output: Some(output.name()),
                            geometry,
                            pointer: [pointer_pos_within_output.x, pointer_pos_within_output.y],
                        });
                }

                // Clear it before calling remove_window() to avoid running interactive_move_end()
                // in the middle of interactive_move_update() and the confusion that causes.
                self.interactive_move = None;

                // Unset fullscreen before removing the tile. This will restore its size properly,
                // and move it to floating if needed, so we don't have to deal with that here.
                let ws = self
                    .workspaces_mut()
                    .find(|ws| ws.has_window(&window_id))
                    .unwrap();
                ws.set_fullscreen(window, false);
                ws.set_maximized(window, false);

                let RemovedTile {
                    mut tile,
                    width,
                    is_full_width,
                    is_floating,
                } = self.detach_window(window, Transaction::new()).unwrap();
                let acknowledged =
                    self.core
                        .borrow_mut()
                        .acknowledge_move_detached(id, width, is_full_width);
                assert!(
                    acknowledged,
                    "moving payload must acknowledge core controls exactly once"
                );
                debug_assert_eq!(
                    self.core
                        .borrow()
                        .interaction()
                        .unwrap()
                        .controls
                        .placement()
                        .floating,
                    is_floating
                );

                tile.stop_move_animations();
                tile.interactive_move_offset = Point::from((0., 0.));
                tile.window().output_enter(&output);
                tile.window().set_preferred_scale_transform(
                    output.current_scale(),
                    output.current_transform(),
                );

                let view_size = output_size(&output);
                let scale = output.current_scale().fractional_scale();
                let options = Options::clone(&self.options)
                    .with_merged_layout(output_config.as_ref())
                    .with_merged_layout(workspace_config.as_ref().map(|(_, c)| c))
                    .adjusted_for_scale(scale);
                tile.update_config(view_size, scale, Rc::new(options));

                if is_floating {
                    // Unlock the view in case we locked it moving a fullscreen window that is
                    // going to unfullscreen to floating.
                    for ws in self.workspaces_mut() {
                        ws.dnd_scroll_gesture_end();
                    }
                } else {
                    // Animate to semitransparent.
                    tile.animate_alpha(
                        1.,
                        INTERACTIVE_MOVE_ALPHA,
                        self.options.animations.window_movement.0,
                    );
                    tile.hold_alpha_animation_after_done();
                }

                let mut data = InteractiveMoveData {
                    tile,
                    output,
                    pointer_pos_within_output,
                    core: self.core.clone(),
                    output_config,
                    workspace_config,
                };

                if let Some((tile_pos, zoom)) = tile_pos {
                    let new_tile_pos = data.tile_render_location(zoom);
                    data.tile
                        .animate_move_from((tile_pos - new_tile_pos).downscale(zoom));
                }

                self.interactive_move = Some(InteractiveMoveState::Moving(data));
            }
            InteractiveMoveState::Moving(mut move_) => {
                if window != move_.tile.window().id() {
                    self.interactive_move = Some(InteractiveMoveState::Moving(move_));
                    return false;
                }

                let mut ws_id = None;
                if let Some(mon) = self.monitor_for_output(&output) {
                    let (insert_ws, _) = mon.insert_position(move_.pointer_pos_within_output);
                    if let InsertWorkspace::Existing(id) = insert_ws {
                        ws_id = Some(id);
                    }
                }

                // If moved over a different workspace, reset the config override.
                let mut update_config = false;
                if let Some((id, _)) = &move_.workspace_config {
                    if Some(*id) != ws_id {
                        move_.workspace_config = None;
                        update_config = true;
                    }
                }

                let previous_output = move_.output.clone();
                let previous_pointer_pos = move_.pointer_pos_within_output;
                if output != move_.output {
                    move_.tile.window().output_leave(&move_.output);
                    move_.tile.window().output_enter(&output);
                    move_.tile.window().set_preferred_scale_transform(
                        output.current_scale(),
                        output.current_transform(),
                    );
                    move_.output = output.clone();
                    self.focus_output(&output);

                    move_.output_config = self
                        .monitor_for_output(&output)
                        .and_then(|mon| mon.layout_config().cloned());

                    update_config = true;
                }

                if update_config {
                    let view_size = output_size(&output);
                    let scale = output.current_scale().fractional_scale();
                    let options = Options::clone(&self.options)
                        .with_merged_layout(move_.output_config.as_ref())
                        .with_merged_layout(move_.workspace_config.as_ref().map(|(_, c)| c))
                        .adjusted_for_scale(scale);
                    move_.tile.update_config(view_size, scale, Rc::new(options));
                }

                move_.pointer_pos_within_output = pointer_pos_within_output;
                if output == previous_output {
                    move_.tile.add_pending_wobble_displacement(
                        pointer_pos_within_output - previous_pointer_pos,
                    );
                }

                self.interactive_move = Some(InteractiveMoveState::Moving(move_));
            }
        }

        self.commit_core_move_pose();
        true
    }

    /// Cancel a still-attached drag's visual offset. A detached normal drag
    /// places at its current location, preserving the established public API;
    /// Stage's captured-origin restoration is a separate core command.
    pub fn interactive_move_cancel(&mut self, window: &W::Id) {
        self.with_desktop_transition(|this| {
            if this
                .pending_move
                .as_ref()
                .is_some_and(|p| &p.window == window)
            {
                this.pending_move = None;
            }
            if !this.finish_stage_drag(window, true) {
                this.interactive_move_end(window);
            }
        })
    }

    pub fn interactive_move_end(&mut self, window: &W::Id) {
        self.with_desktop_transition(|this| {
            if this
                .pending_move
                .as_ref()
                .is_some_and(|p| &p.window == window)
            {
                this.flush_pending_move();
            }
            if this.finish_stage_drag(window, false) {
                return;
            }
            let Some(move_) = &this.interactive_move else {
                return;
            };

            let move_ = match move_ {
                InteractiveMoveState::Starting { window_id, .. } => {
                    if window_id != window {
                        return;
                    }

                    let Some(InteractiveMoveState::Starting { window_id, .. }) =
                        this.interactive_move.take()
                    else {
                        unreachable!()
                    };

                    for mon in this.monitors_mut() {
                        mon.dnd_scroll_gesture_end();
                    }

                    for ws in this.workspaces_mut() {
                        if let Some(tile) =
                            ws.tiles_mut().find(|tile| *tile.window().id() == window_id)
                        {
                            let offset = tile.interactive_move_offset;
                            tile.interactive_move_offset = Point::from((0., 0.));
                            tile.finish_wobble();
                            tile.animate_move_from(offset);
                        }

                        // Unlock the view on the workspaces, but if the moved window was active,
                        // preserve that.
                        let moved_tile_was_active =
                            ws.active_window().is_some_and(|win| *win.id() == window_id);

                        ws.dnd_scroll_gesture_end();

                        if moved_tile_was_active {
                            ws.activate_window(&window_id);
                        }
                    }

                    return;
                }
                InteractiveMoveState::Moving(move_) => move_,
            };

            if window != move_.tile.window().id() {
                return;
            }

            let Some(InteractiveMoveState::Moving(mut move_)) = this.interactive_move.take() else {
                unreachable!()
            };

            for mon in this.monitors_mut() {
                mon.dnd_scroll_gesture_end();
            }

            let placement = move_.placement();
            // Unlock the view on the workspaces.
            if !placement.floating {
                for ws in this.workspaces_mut() {
                    ws.dnd_scroll_gesture_end();
                }

                // Also animate the tile back to opaque.
                move_.tile.animate_alpha(
                    INTERACTIVE_MOVE_ALPHA,
                    1.,
                    this.options.animations.window_movement.0,
                );
            }

            move_.tile.finish_wobble();
            // Placement is a transient core intent; only the Tile payload is moved.

            // Dragging in the overview shouldn't switch the workspace and so on.
            let allow_to_activate_workspace = !this.is_overview_open();

            match &mut this.monitor_set {
                MonitorSet::Normal { monitors, .. } => {
                    let (mon, insert_ws, position, offset, zoom) = if let Some(mon) =
                        monitors.iter_mut().find(|mon| mon.output == move_.output)
                    {
                        let (insert_ws, geo) = mon.insert_position(move_.pointer_pos_within_output);
                        let zoom = mon.workspace_render_zoom(geo);
                        let zoom = if mon.is_stage_visible() {
                            zoom.max(0.01)
                        } else {
                            zoom
                        };
                        let (position, offset) = match insert_ws {
                            InsertWorkspace::Existing(ws_id) => {
                                let ws_idx = mon.idx_of_ws(ws_id).unwrap();

                                let position = if placement.floating {
                                    InsertPosition::Floating
                                } else {
                                    let pos_within_workspace =
                                        (move_.pointer_pos_within_output - geo.loc).downscale(zoom);
                                    let ws = &mut mon.workspaces[ws_idx];
                                    ws.scrolling_insert_position(pos_within_workspace)
                                };

                                (position, Some(geo.loc))
                            }
                            InsertWorkspace::NewAt(_) => {
                                let position = if placement.floating {
                                    InsertPosition::Floating
                                } else {
                                    InsertPosition::NewColumn(0)
                                };

                                (position, None)
                            }
                        };

                        (mon, insert_ws, position, offset, zoom)
                    } else {
                        let selected_index = selected_monitor_idx(monitors);
                        let mon = &mut monitors[selected_index];
                        let zoom = mon.overview_zoom();
                        // No point in trying to use the pointer position on the wrong output.
                        let ws = &mon.workspaces[0];
                        let ws_geo = mon.workspaces_render_geo().next().unwrap();

                        let position = if placement.floating {
                            InsertPosition::Floating
                        } else {
                            ws.scrolling_insert_position(Point::from((0., 0.)))
                        };

                        let insert_ws = InsertWorkspace::Existing(ws.id());
                        (mon, insert_ws, position, Some(ws_geo.loc), zoom)
                    };

                    let win_id = move_.tile.window().id().clone();
                    let tile_render_loc = move_.tile_render_location(zoom);

                    let ws_idx = match insert_ws {
                        InsertWorkspace::Existing(ws_id) => mon.idx_of_ws(ws_id).unwrap(),
                        InsertWorkspace::NewAt(ws_idx) => {
                            if mon.options.layout.empty_workspace_above_first && ws_idx == 0 {
                                // Reuse the top empty workspace.
                                0
                            } else if mon.workspaces.len() - 1 <= ws_idx {
                                // Reuse the bottom empty workspace.
                                mon.workspaces.len() - 1
                            } else {
                                mon.add_workspace_at(ws_idx);
                                ws_idx
                            }
                        }
                    };

                    if mon.is_stage_visible() {
                        let id = mon.workspaces[ws_idx].id();
                        mon.stage_select(id);
                    }

                    match position {
                        InsertPosition::NewColumn(column_idx) => {
                            let ws_id = mon.workspaces[ws_idx].id();
                            mon.add_tile(
                                move_.tile,
                                MonitorAddWindowTarget::Workspace {
                                    id: ws_id,
                                    column_idx: Some(column_idx),
                                },
                                ActivateWindow::Yes,
                                allow_to_activate_workspace,
                                placement.width,
                                placement.full_width,
                                false,
                                None,
                            );
                        }
                        InsertPosition::InColumn(column_idx, tile_idx) => {
                            mon.add_tile_to_column(
                                ws_idx,
                                column_idx,
                                Some(tile_idx),
                                move_.tile,
                                true,
                                allow_to_activate_workspace,
                            );
                        }
                        InsertPosition::Floating => {
                            let mut tile = move_.tile;
                            tile.floating_pos = None;

                            match insert_ws {
                                InsertWorkspace::Existing(_) => {
                                    if let Some(offset) = offset {
                                        let pos = (tile_render_loc - offset).downscale(zoom);
                                        let pos = mon.workspaces[ws_idx]
                                            .floating_logical_to_size_frac(pos);
                                        tile.floating_pos = Some(pos);
                                    } else {
                                        error!(
                                            "offset unset for inserting a floating tile \
                                         to existing workspace"
                                        );
                                    }
                                }
                                InsertWorkspace::NewAt(_) => {
                                    // When putting a floating tile on a new workspace, we don't
                                    // really have a good
                                    // pre-existing position.
                                }
                            }

                            // Set the floating size so it takes into account any window resizing
                            // that took place during the move.
                            if let Some(size) = tile.window().expected_size() {
                                tile.floating_window_size = Some(size);
                            }

                            let ws_id = mon.workspaces[ws_idx].id();
                            mon.add_tile(
                                tile,
                                MonitorAddWindowTarget::Workspace {
                                    id: ws_id,
                                    column_idx: None,
                                },
                                ActivateWindow::Yes,
                                allow_to_activate_workspace,
                                placement.width,
                                placement.full_width,
                                true,
                                None,
                            );
                        }
                    }

                    // needed because empty_workspace_above_first could have modified the idx
                    let view_width = mon.view_size().w;
                    let stage = mon.is_stage_visible();
                    let (tile, tile_offset, ws_geo) = mon
                        .workspaces_with_render_geo_mut(false)
                        .find_map(|(ws, geo)| {
                            ws.tiles_with_render_positions_mut(false)
                                .find(|(tile, _)| tile.window().id() == &win_id)
                                .map(|(tile, tile_offset)| (tile, tile_offset, geo))
                        })
                        .unwrap();
                    // A new workspace can have a different animated card size from
                    // the drop target. Convert back with the geometry actually used
                    // to render it; a hidden card has no geometry yet, so retain the
                    // visible drop scale until its first animation frame.
                    let actual_zoom = ws_geo.size.w / view_width;
                    let actual_zoom = if actual_zoom > 1e-6 {
                        actual_zoom
                    } else {
                        zoom
                    };
                    let actual_zoom = if stage {
                        actual_zoom.max(0.01)
                    } else {
                        actual_zoom
                    };
                    let new_tile_render_loc = ws_geo.loc + tile_offset.upscale(actual_zoom);

                    tile.animate_move_from(
                        (tile_render_loc - new_tile_render_loc).downscale(actual_zoom),
                    );

                    // Interactive move into floating barely animates (it doesn't really move after
                    // being dropped), so setting it as moving between workspaces would just cause
                    // it to awkwardly sit unclipped for a moment before the
                    // animation runs out.
                    if !matches!(position, InsertPosition::Floating) {
                        tile.set_anim_y_between_workspaces();
                    }
                }
                MonitorSet::NoOutputs { workspaces, .. } => {
                    if workspaces.is_empty() {
                        workspaces.push(
                            Workspace::new_no_outputs(
                                this.clock.clone(),
                                this.options.clone(),
                                this.scene_resources.clone(),
                            )
                            .with_core(this.core.clone()),
                        );
                    }
                    let index = detached_workspace_idx(&this.core, workspaces);
                    let ws = &mut workspaces[index];

                    // No point in trying to use the pointer position without outputs.
                    ws.add_tile(
                        move_.tile,
                        WorkspaceAddWindowTarget::Auto,
                        ActivateWindow::Yes,
                        placement.width,
                        placement.full_width,
                        placement.floating,
                        None,
                    );
                }
            }
            this.finish_group_drag(window);
            this.discard_stage_drag_origin(window);
        })
    }

    pub fn interactive_move_is_moving_above_output(&self, output: &Output) -> bool {
        let Some(InteractiveMoveState::Moving(move_)) = &self.interactive_move else {
            return false;
        };

        move_.output == *output
    }

    pub fn dnd_update(&mut self, output: Output, pointer_pos_within_output: Point<f64, Logical>) {
        // The actual live device object must match before the engine derives
        // its connection generation; a retired same-name Output is rejected.
        if self.monitor_for_output(&output).is_none() {
            return;
        }
        let begin_gesture = self.dnd.is_none();
        self.core
            .borrow_mut()
            .dnd_hold_action(clarion_core::dnd_hold::DndHoldAction::Motion {
                connector: output.name(),
            });
        self.dnd = Some(DndData {
            output,
            pointer_pos_within_output,
        });

        if begin_gesture {
            for mon in self.monitors_mut() {
                mon.dnd_scroll_gesture_begin();
            }

            for ws in self.workspaces_mut() {
                ws.dnd_scroll_gesture_begin();
            }
        }
    }

    pub fn dnd_end(&mut self) {
        self.with_desktop_transition(|this| {
            if this.dnd.is_none() {
                return;
            }

            this.dnd = None;
            this.core
                .borrow_mut()
                .dnd_hold_action(clarion_core::dnd_hold::DndHoldAction::End);

            for mon in this.monitors_mut() {
                mon.dnd_scroll_gesture_end();
            }

            for ws in this.workspaces_mut() {
                ws.dnd_scroll_gesture_end();
            }
        })
    }

    pub fn interactive_resize_begin(&mut self, window: W::Id, edges: ResizeEdge) -> bool {
        self.with_desktop_transition(|this| {
            this.checkpoint_layout();
            match &mut this.monitor_set {
                MonitorSet::Normal { monitors, .. } => {
                    for mon in monitors {
                        for ws in &mut mon.workspaces {
                            if ws.has_window(&window) {
                                return ws.interactive_resize_begin(window, edges);
                            }
                        }
                    }
                }
                MonitorSet::NoOutputs { workspaces, .. } => {
                    for ws in workspaces {
                        if ws.has_window(&window) {
                            return ws.interactive_resize_begin(window, edges);
                        }
                    }
                }
            }

            false
        })
    }

    pub fn interactive_resize_update(
        &mut self,
        window: &W::Id,
        delta: Point<f64, Logical>,
    ) -> bool {
        self.with_desktop_transition(|this| {
            if let Some(InteractiveMoveState::Moving(move_)) = &this.interactive_move {
                if move_.tile.window().id() == window {
                    return false;
                }
            }

            match &mut this.monitor_set {
                MonitorSet::Normal { monitors, .. } => {
                    for mon in monitors {
                        for ws in &mut mon.workspaces {
                            if ws.has_window(window) {
                                return ws.interactive_resize_update(window, delta);
                            }
                        }
                    }
                }
                MonitorSet::NoOutputs { workspaces, .. } => {
                    for ws in workspaces {
                        if ws.has_window(window) {
                            return ws.interactive_resize_update(window, delta);
                        }
                    }
                }
            }

            false
        })
    }

    pub fn interactive_resize_end(&mut self, window: &W::Id) {
        self.with_desktop_transition(|this| {
            if let Some(InteractiveMoveState::Moving(move_)) = &this.interactive_move {
                if move_.tile.window().id() == window {
                    return;
                }
            }

            match &mut this.monitor_set {
                MonitorSet::Normal { monitors, .. } => {
                    for mon in monitors {
                        for ws in &mut mon.workspaces {
                            if ws.has_window(window) {
                                ws.interactive_resize_end(Some(window));
                                return;
                            }
                        }
                    }
                }
                MonitorSet::NoOutputs { workspaces, .. } => {
                    for ws in workspaces {
                        if ws.has_window(window) {
                            ws.interactive_resize_end(Some(window));
                            return;
                        }
                    }
                }
            }
        })
    }

    pub fn move_workspace_down(&mut self) {
        self.with_desktop_transition(|this| {
            let Some(monitor) = this.active_monitor() else {
                return;
            };
            monitor.move_workspace_down();
        })
    }

    pub fn move_workspace_up(&mut self) {
        self.with_desktop_transition(|this| {
            let Some(monitor) = this.active_monitor() else {
                return;
            };
            monitor.move_workspace_up();
        })
    }

    pub fn move_workspace_to_idx(
        &mut self,
        reference: Option<(Option<Output>, usize)>,
        new_idx: usize,
    ) {
        self.with_desktop_transition(|this| {
            let (monitor, old_idx) = if let Some((output, old_idx)) = reference {
                let monitor = if let Some(output) = output {
                    let Some(monitor) = this.monitor_for_output_mut(&output) else {
                        return;
                    };
                    monitor
                } else {
                    // In case a numbered workspace reference is used, assume the active monitor
                    let Some(monitor) = this.active_monitor() else {
                        return;
                    };
                    monitor
                };

                (monitor, old_idx)
            } else {
                let Some(monitor) = this.active_monitor() else {
                    return;
                };
                let index = monitor.active_workspace_idx();
                (monitor, index)
            };

            monitor.move_workspace_to_idx(old_idx, new_idx);
        })
    }

    pub fn set_workspace_name(&mut self, name: String, reference: Option<WorkspaceReference>) {
        self.with_desktop_transition(|this| {
            let ws = if let Some(reference) = reference {
                this.find_workspace_by_ref(reference)
            } else {
                this.active_workspace_mut()
            };
            let Some(ws) = ws else {
                return;
            };

            if !ws.set_name(Some(name)) {
                return;
            }

            let wsid = ws.id();

            // if `empty_workspace_above_first` is set and `ws` is the first
            // workspace on a monitor, another empty workspace needs to
            // be added before.
            // Conversely, if `ws` was the last workspace on a monitor, an
            // empty workspace needs to be added after.

            if let MonitorSet::Normal { monitors, .. } = &mut this.monitor_set {
                let selected_index = selected_monitor_idx(monitors);
                let monitor = &mut monitors[selected_index];
                if monitor.options.layout.empty_workspace_above_first
                    && monitor
                        .workspaces
                        .first()
                        .is_some_and(|first| first.id() == wsid)
                {
                    monitor.add_workspace_top();
                }
                if monitor
                    .workspaces
                    .last()
                    .is_some_and(|last| last.id() == wsid)
                {
                    monitor.add_workspace_bottom();
                }
            }
        })
    }

    pub fn unset_workspace_name(&mut self, reference: Option<WorkspaceReference>) {
        self.with_desktop_transition(|this| {
            let ws = if let Some(reference) = reference {
                this.find_workspace_by_ref(reference)
            } else {
                this.active_workspace_mut()
            };
            let Some(ws) = ws else {
                return;
            };
            let id = ws.id();

            this.unname_workspace_by_id(id);
        })
    }

    pub fn set_monitors_overview_state(&mut self) {
        let MonitorSet::Normal { monitors, .. } = &mut self.monitor_set else {
            return;
        };

        for mon in monitors {
            mon.set_overview_progress(self.overview_progress.as_ref());
        }
    }

    pub fn overview_mode(&self) -> OverviewMode {
        self.core.borrow().overview().mode()
    }

    pub fn is_overview_visible(&self) -> bool {
        self.core.borrow().overview().visible()
    }

    pub fn default_overview_mode(&self) -> OverviewMode {
        match self.options.overview.style {
            clarion_config::OverviewStyle::Stage => OverviewMode::Stage,
            clarion_config::OverviewStyle::Strip => OverviewMode::Strip,
        }
    }

    fn cancel_overview_navigation_gestures(&mut self) {
        for monitor in self.monitors_mut() {
            monitor.workspace_switch_gesture_cancel(None);
            for workspace in &mut monitor.workspaces {
                workspace.view_offset_gesture_cancel(None);
            }
        }
    }

    /// All mode entry points release obsolete gestures before accepting input
    /// for the new geometry. State captures the old composed frame first.
    fn set_overview_mode(&mut self, mode: OverviewMode) {
        self.complete_minimize_animations();
        if self.overview_mode() == mode {
            return;
        }
        self.cancel_overview_navigation_gestures();
        if self.overview_mode() == OverviewMode::Stage {
            if let Some(window) = self.interactive_move.as_ref().map(|move_| match move_ {
                InteractiveMoveState::Starting { window_id, .. } => window_id.clone(),
                InteractiveMoveState::Moving(move_) => move_.tile.window().id().clone(),
            }) {
                self.interactive_move_cancel(&window);
            }
        }
        let outcome = self
            .core
            .borrow_mut()
            .overview_action(OverviewAction::SetMode {
                mode,
                timestamp: self.clock.now_unadjusted(),
            });
        if let Some(settlement) = outcome.settlement {
            self.apply_overview_settlement(settlement);
        }
    }

    pub fn toggle_overview(&mut self) {
        self.with_desktop_transition(|this| {
            let mode = this.default_overview_mode();
            if mode == OverviewMode::Stage {
                if this.is_overview_open() && this.overview_mode() == mode {
                    this.close_overview();
                    return;
                }
                this.prepare_stage_entry();
            }
            if this.is_overview_open() && this.overview_mode() != mode {
                this.set_overview_mode(mode);
                this.set_monitors_overview_state();
                return;
            }

            this.set_overview_mode(mode);
            this.toggle_overview_inner();
        })
    }

    pub fn toggle_window_spread(&mut self) {
        self.with_desktop_transition(|this| {
            if this.is_overview_open() && this.overview_mode() == OverviewMode::Windows {
                this.close_overview();
                return;
            }
            let fresh = !this.is_window_spread_visible();
            if fresh {
                let initial = this.active_window().map(LayoutElement::core_id);
                this.core
                    .borrow_mut()
                    .spread_action(clarion_core::spread::SpreadAction::Begin { initial });
            }
            this.set_overview_mode(OverviewMode::Windows);
            if let MonitorSet::Normal { monitors, .. } = &mut this.monitor_set {
                for mon in monitors {
                    if fresh {
                        mon.reset_spread();
                    }
                }
            }
            if !this.is_overview_open() {
                this.toggle_overview_inner();
            }
            this.set_monitors_overview_state();
        })
    }

    pub fn spread_key(&mut self, key: smithay::input::keyboard::Keysym) -> bool {
        use smithay::input::keyboard::keysyms;
        if !self.is_overview_open() || self.overview_mode() != OverviewMode::Windows {
            return false;
        }
        match key.raw() {
            keysyms::KEY_Escape => {
                for mon in self.monitors_mut() {
                    mon.capture_spread_frame();
                }
                let initial = self.core.borrow().spread().initial().copied();
                let id = initial
                    .and_then(|id| self.window_for_core_id(id))
                    .map(|window| window.id().clone());
                if let Some(id) = id {
                    self.activate_window(&id);
                }
                self.close_overview();
            }
            keysyms::KEY_Return | keysyms::KEY_KP_Enter => {
                let id = self.active_monitor_ref().and_then(|m| m.spread_selected());
                if let Some(id) = id {
                    for mon in self.monitors_mut() {
                        mon.capture_spread_frame();
                    }
                    self.activate_window(&id);
                    self.close_overview();
                }
            }
            _ => {
                if let Some(mon) = self.active_monitor() {
                    mon.spread_key(key);
                }
            }
        }
        true
    }

    /// A client metadata event updates exactly one cached Spread candidate.
    /// Reading frames does not rebuild title/app search metadata.
    pub(crate) fn window_metadata_changed(&self, window: &W::Id) -> Option<Output> {
        if !self.is_window_spread_visible() {
            return None;
        }
        let id = self.core.id(window)?;
        let window = self.window_for_core_id(id)?;
        let core = self.core.borrow();
        let connector = core.window(&id)?.output.as_deref()?;
        let spread = core.spread().output(connector)?;
        if !spread.order().contains(&id) {
            return None;
        }
        let output = self
            .monitors()
            .find(|monitor| monitor.output_name() == connector)?
            .output()
            .clone();
        let search_text = (!spread.query().is_empty()).then(|| {
            let (app, title) = window.session_identity();
            format!("{app} {title}")
        });
        drop(core);
        self.core
            .borrow_mut()
            .window_metadata_changed(id, search_text);
        Some(output)
    }

    pub fn is_window_spread_visible(&self) -> bool {
        self.overview_mode() == OverviewMode::Windows
            && (self.is_overview_open() || self.overview_progress.is_some())
    }

    pub fn is_window_spread_closing(&self) -> bool {
        self.is_window_spread_visible() && !self.is_overview_open()
    }

    fn remember_expo_origin(&self, id: Option<WorkspaceId>) {
        self.core
            .borrow_mut()
            .overview_action(OverviewAction::RememberExpo(
                id.map(|id| clarion_core::SpaceId(id.get())),
            ));
    }

    fn take_expo_origin(&self) -> Option<WorkspaceId> {
        let id = self.core.borrow().overview().initial_expo();
        self.core
            .borrow_mut()
            .overview_action(OverviewAction::RememberExpo(None));
        id.map(|id| WorkspaceId::specific(id.0))
    }

    fn take_stage_origin(&self) -> Option<WorkspaceId> {
        let id = self.core.borrow().overview().initial_stage();
        self.core
            .borrow_mut()
            .overview_action(OverviewAction::RememberStage(None));
        id.map(|id| WorkspaceId::specific(id.0))
    }

    pub fn toggle_expo(&mut self) {
        self.with_desktop_transition(|this| {
            if this.is_overview_open() && this.overview_mode() != OverviewMode::Expo {
                this.remember_expo_origin(this.active_workspace().map(Workspace::id));
                this.set_overview_mode(OverviewMode::Expo);
                this.set_monitors_overview_state();
                return;
            }

            if !this.is_overview_open() {
                this.remember_expo_origin(this.active_workspace().map(Workspace::id));
            } else {
                this.remember_expo_origin(None);
            }

            this.set_overview_mode(OverviewMode::Expo);
            this.toggle_overview_inner();
        })
    }

    pub fn cancel_expo(&mut self) -> bool {
        self.with_desktop_transition(|this| {
            if !this.is_overview_open() || this.overview_mode() != OverviewMode::Expo {
                return false;
            }
            if let Some(initial) = this.take_expo_origin() {
                if let Some((idx, ws)) = this.find_workspace_by_id(initial) {
                    if let Some(output) = ws.current_output().cloned() {
                        this.focus_output(&output);
                        this.switch_workspace(idx);
                    }
                }
            }
            this.toggle_overview_inner();
            true
        })
    }

    pub fn confirm_expo(&mut self) -> bool {
        self.with_desktop_transition(|this| {
            if !this.is_overview_open() || this.overview_mode() != OverviewMode::Expo {
                return false;
            }
            this.remember_expo_origin(None);
            this.toggle_overview_inner();
            true
        })
    }

    pub fn expo_navigate(&mut self, dx: isize, dy: isize) -> bool {
        self.with_desktop_transition(|this| {
            if !this.is_overview_open() || this.overview_mode() != OverviewMode::Expo {
                return false;
            }
            let Some(mon) = this.active_monitor() else {
                return false;
            };
            mon.navigate_expo(dx, dy)
        })
    }

    pub fn ensure_scratchpad_workspace(&mut self) -> (usize, WorkspaceId) {
        self.with_desktop_transition(|this| {
            if let Some((idx, ws)) = this.find_workspace_by_name("scratchpad") {
                let ws_id = ws.id();
                let ws_output = ws.current_output().cloned();
                let active_output = this.active_output().cloned();
                if let (Some(ws_out), Some(act_out)) = (ws_output, active_output) {
                    if ws_out != act_out {
                        this.move_workspace_to_output_by_id(idx, Some(ws_out), &act_out);
                        if let Some((new_idx, ws)) = this.find_workspace_by_name("scratchpad") {
                            return (new_idx, ws.id());
                        }
                    }
                }
                return (idx, ws_id);
            }
            let config = WorkspaceConfig {
                name: clarion_config::WorkspaceName("scratchpad".to_string()),
                open_on_output: None,
                layout: None,
            };
            let clock = this.clock.clone();
            let options = this.options.clone();

            match &mut this.monitor_set {
                MonitorSet::Normal { monitors, .. } => {
                    let selected_index = selected_monitor_idx(monitors);
                    let mon = &mut monitors[selected_index];
                    let mut ws = Workspace::new_with_config(
                        mon.output.clone(),
                        Some(config),
                        clock,
                        options,
                        this.scene_resources.clone(),
                    )
                    .with_core(this.core.clone());
                    ws.set_output(Some(mon.output.clone()));
                    ws.update_config(mon.options.clone());

                    let id = clarion_core::SpaceId(ws.id().get());
                    let inserted_index;
                    if mon.workspaces.is_empty() {
                        inserted_index = 0;
                        mon.workspaces.push(ws);
                    } else {
                        let last_idx = mon.workspaces.len() - 1;
                        if !mon.workspaces[last_idx].has_windows_or_name() {
                            inserted_index = last_idx;
                            mon.workspaces.insert(last_idx, ws);
                        } else {
                            inserted_index = mon.workspaces.len();
                            mon.workspaces.push(ws);
                        }
                    }
                    this.core
                        .borrow_mut()
                        .attach_space(id, mon.output_name(), inserted_index);
                }
                MonitorSet::NoOutputs { workspaces } => {
                    let ws = Workspace::new_with_config_no_outputs(
                        Some(config),
                        clock,
                        options,
                        this.scene_resources.clone(),
                    )
                    .with_core(this.core.clone());
                    workspaces.push(ws);
                }
            }

            if let Some((idx, ws)) = this.find_workspace_by_name("scratchpad") {
                (idx, ws.id())
            } else {
                (
                    0,
                    this.active_workspace()
                        .map_or_else(|| WorkspaceId::specific(0), |w| w.id()),
                )
            }
        })
    }

    pub fn toggle_scratchpad(&mut self) {
        let is_current_scratchpad = self.active_workspace().is_some_and(|ws| ws.is_scratchpad());

        if is_current_scratchpad {
            let return_id = self.core.borrow_mut().take_scratchpad_return();
            if let Some(return_id) = return_id.map(|id| WorkspaceId::specific(id.0)) {
                if let Some((idx, ws)) = self.find_workspace_by_id(return_id) {
                    if let Some(out) = ws.current_output().cloned() {
                        self.focus_output(&out);
                    }
                    self.switch_workspace(idx);
                    return;
                }
            }
            self.switch_workspace_previous();
        } else {
            let space = self
                .active_workspace()
                .map(|workspace| clarion_core::SpaceId(workspace.id().get()));
            self.core
                .borrow_mut()
                .apply(clarion_core::CoreEvent::RememberScratchpadReturn { space });
            let (scratch_idx, _) = self.ensure_scratchpad_workspace();
            self.switch_workspace(scratch_idx);
        }
    }

    pub fn move_window_to_scratchpad(&mut self) {
        let is_current_scratchpad = self.active_workspace().is_some_and(|ws| ws.is_scratchpad());
        if is_current_scratchpad {
            let return_id = self
                .core
                .borrow()
                .scratchpad_return()
                .map(|id| WorkspaceId::specific(id.0));
            let target_info = return_id.and_then(|id| {
                self.find_workspace_by_id(id)
                    .map(|(idx, ws)| (idx, ws.current_output().cloned()))
            });
            if let Some((idx, Some(out))) = target_info {
                // Move the scratchpad's active window before changing output focus.
                self.move_to_output(None, &out, Some(idx), ActivateWindow::Smart);
                self.focus_output(&out);
                self.switch_workspace(idx);
            } else {
                let target_idx = self
                    .active_monitor()
                    .and_then(|m| m.workspaces.iter().position(|w| !w.is_scratchpad()))
                    .unwrap_or(0);

                self.move_to_workspace(None, target_idx, ActivateWindow::Smart);
                self.switch_workspace(target_idx);
            }
        } else {
            let space = self
                .active_workspace()
                .map(|workspace| clarion_core::SpaceId(workspace.id().get()));
            self.core
                .borrow_mut()
                .apply(clarion_core::CoreEvent::RememberScratchpadReturn { space });
            let (scratch_idx, _) = self.ensure_scratchpad_workspace();
            self.move_to_workspace(None, scratch_idx, ActivateWindow::No);
        }
    }

    fn toggle_overview_inner(&mut self) {
        let open = !self.is_overview_open();
        if !open {
            self.remember_expo_origin(None);
        }
        let gesture_velocity = self.core.borrow().overview().gesture_velocity();
        let gesture_value = self.core.borrow().overview().gesture_value().unwrap_or(0.);
        let (from, velocity) = self.overview_progress.take().map_or((0., 0.), |p| {
            let velocity = match &p {
                OverviewProgress::Animation(anim) => anim.velocity(),
                OverviewProgress::Gesture => gesture_velocity,
                OverviewProgress::Open => 0.,
            };
            (p.value(gesture_value), velocity)
        });
        self.core
            .borrow_mut()
            .overview_action(OverviewAction::SetOpen(open));
        self.apply_overview_settlement(OverviewSettlement {
            from,
            to: if open { 1. } else { 0. },
            velocity,
        });
        self.set_monitors_overview_state();
    }

    pub fn open_overview(&mut self) -> bool {
        self.with_desktop_transition(|this| {
            let mode = this.default_overview_mode();
            if this.is_overview_open() && this.overview_mode() == mode {
                return false;
            }

            if mode == OverviewMode::Stage {
                this.prepare_stage_entry();
            }
            this.set_overview_mode(mode);
            if !this.is_overview_open() {
                this.toggle_overview_inner();
            } else {
                this.set_monitors_overview_state();
            }
            true
        })
    }

    pub fn close_overview(&mut self) -> bool {
        self.with_desktop_transition(|this| {
            if !this.is_overview_open() {
                return false;
            }

            if this.is_window_spread_visible() {
                for mon in this.monitors_mut() {
                    mon.prepare_spread_close();
                }
            }

            if this.overview_mode() == OverviewMode::Stage {
                for mon in this.monitors_mut() {
                    mon.stage_prepare_close(mon.active_workspace_ref().id());
                }
            }

            this.toggle_overview_inner();
            true
        })
    }

    pub fn toggle_overview_to_workspace(&mut self, ws_idx: usize) {
        self.with_desktop_transition(|this| {
            if !this.is_overview_open() {
                return;
            }
            let config = this.options.animations.overview_open_close.0;
            if this.overview_mode() == OverviewMode::Stage {
                let active_output = this.active_monitor_ref().map(|mon| mon.output().clone());
                for mon in this.monitors_mut() {
                    let target = if active_output.as_ref() == Some(mon.output()) {
                        mon.workspaces.get(ws_idx).map(Workspace::id)
                    } else {
                        Some(mon.active_workspace_ref().id())
                    };
                    if let Some(target) = target {
                        mon.stage_prepare_close(target);
                    }
                }
            }
            if let Some(mon) = this.active_monitor() {
                mon.activate_workspace_with_anim_config(ws_idx, Some(config));
            }
            this.toggle_overview_inner();
        })
    }

    fn prepare_stage_entry(&mut self) {
        let fresh = self.overview_mode() != OverviewMode::Stage || !self.is_overview_visible();
        if fresh || self.core.borrow().overview().initial_stage().is_none() {
            let origin = self
                .active_workspace()
                .map(|ws| clarion_core::SpaceId(ws.id().get()));
            self.core
                .borrow_mut()
                .overview_action(OverviewAction::RememberStage(origin));
            self.set_stage_preview_output(self.active_output().cloned().as_ref());
        }
        if fresh {
            for mon in self.monitors_mut() {
                mon.reset_stage();
            }
        }
    }

    fn stage_preview_output(&self) -> Option<Output> {
        let core = self.core.borrow();
        let connector = core.overview().preview_connector()?;
        self.monitors()
            .find(|mon| mon.output_name() == connector)
            .map(|mon| mon.output().clone())
    }

    fn set_stage_preview_output(&mut self, output: Option<&Output>) {
        self.core
            .borrow_mut()
            .overview_action(OverviewAction::PreviewOutput(output.map(Output::name)));
    }

    pub fn is_stage_open(&self) -> bool {
        self.is_overview_open() && self.overview_mode() == OverviewMode::Stage
    }

    pub fn is_stage_visible(&self) -> bool {
        self.overview_mode() == OverviewMode::Stage && self.is_overview_visible()
    }

    pub fn stage_navigate(&mut self, output: Option<&Output>, delta: isize) -> bool {
        if !self.is_stage_open() || self.interactive_move.is_some() {
            return false;
        }
        let output = output
            .cloned()
            .or_else(|| self.stage_preview_output())
            .filter(|output| self.monitor_for_output(output).is_some())
            .or_else(|| self.active_output().cloned());
        let Some(output) = output else {
            return false;
        };
        let changed = self
            .monitor_for_output_mut(&output)
            .is_some_and(|mon| mon.stage_navigate(delta));
        if changed {
            self.set_stage_preview_output(Some(&output));
        }
        changed
    }

    pub fn stage_select_at(&mut self, output: &Output, pos: Point<f64, Logical>) -> bool {
        if !self.is_stage_open() {
            return false;
        }
        let Some(mon) = self.monitor_for_output_mut(output) else {
            return false;
        };
        if let Some(delta) = mon.stage_rail_navigation(pos) {
            mon.stage_navigate(delta);
            self.set_stage_preview_output(Some(output));
            return true;
        }
        if let Some(target) = mon.stage_rail_under(pos) {
            match target {
                InsertWorkspace::Existing(id) => {
                    if mon.stage_selected() == id {
                        self.set_stage_preview_output(Some(output));
                        self.confirm_stage();
                        return true;
                    }
                    mon.stage_select(id);
                }
                InsertWorkspace::NewAt(_) => {
                    // Every monitor keeps an empty trailing workspace ready for
                    // new windows. A click enters it; a drag still creates a
                    // workspace through the explicit NewAt drop target.
                    let empty = mon.workspaces.last().unwrap().id();
                    mon.stage_select(empty);
                    self.set_stage_preview_output(Some(output));
                    self.confirm_stage();
                    return true;
                }
            }
            self.set_stage_preview_output(Some(output));
            return true;
        }
        false
    }

    pub fn stage_selected_for_output(&self, output: &Output) -> Option<WorkspaceId> {
        self.is_stage_open()
            .then(|| {
                self.monitor_for_output(output)
                    .map(|mon| mon.stage_selected())
            })
            .flatten()
    }

    pub fn stage_select(&mut self, output: &Output, id: WorkspaceId) -> bool {
        let changed = self.is_stage_open()
            && self
                .monitor_for_output_mut(output)
                .is_some_and(|mon| mon.stage_select(id));
        if changed {
            self.set_stage_preview_output(Some(output));
        }
        changed
    }

    pub fn confirm_stage(&mut self) -> bool {
        self.with_desktop_transition(|this| {
            if !this.is_stage_open() || this.interactive_move.is_some() {
                return false;
            }
            let selected_monitor = this
                .stage_preview_output()
                .as_ref()
                .and_then(|output| this.monitor_for_output(output))
                .or_else(|| this.active_monitor_ref());
            let Some(id) = selected_monitor.map(|mon| mon.stage_selected()) else {
                return this.close_overview();
            };
            let Some((idx, ws)) = this.find_workspace_by_id(id) else {
                return this.close_overview();
            };
            let Some(output) = ws.current_output().cloned() else {
                return this.close_overview();
            };
            this.focus_output(&output);
            this.toggle_overview_to_workspace(idx);
            true
        })
    }

    pub fn cancel_stage(&mut self) -> bool {
        self.with_desktop_transition(|this| {
            if !this.is_stage_open() {
                return false;
            }
            if let Some(window) = this.interactive_move.as_ref().map(|move_| match move_ {
                InteractiveMoveState::Starting { window_id, .. } => window_id.clone(),
                InteractiveMoveState::Moving(move_) => move_.tile.window().id().clone(),
            }) {
                this.interactive_move_cancel(&window);
            }
            for mon in this.monitors_mut() {
                mon.stage_restore();
            }
            if let Some(initial) = this.take_stage_origin() {
                if let Some((idx, ws)) = this.find_workspace_by_id(initial) {
                    if let Some(output) = ws.current_output().cloned() {
                        this.focus_output(&output);
                        this.switch_workspace(idx);
                    }
                }
            }
            this.close_overview()
        })
    }

    pub fn stage_key(&mut self, key: smithay::input::keyboard::Keysym) -> bool {
        use smithay::input::keyboard::keysyms;
        if !self.is_stage_open() {
            return false;
        }
        match key.raw() {
            keysyms::KEY_Escape => {
                self.cancel_stage();
            }
            keysyms::KEY_Return | keysyms::KEY_KP_Enter | keysyms::KEY_space => {
                self.confirm_stage();
            }
            keysyms::KEY_Left | keysyms::KEY_Up => {
                self.stage_navigate(None, -1);
            }
            keysyms::KEY_Right | keysyms::KEY_Down => {
                self.stage_navigate(None, 1);
            }
            _ => return false,
        }
        true
    }

    pub fn start_open_animation_for_window(&mut self, window: &W::Id) {
        if let Some(InteractiveMoveState::Moving(move_)) = &self.interactive_move {
            if move_.tile.window().id() == window {
                return;
            }
        }

        for ws in self.workspaces_mut() {
            if ws.start_open_animation(window) {
                return;
            }
        }
    }

    pub fn store_unmap_snapshot_any(
        &mut self,
        renderer: &mut AnyRenderer<'_>,
        xray: Option<&mut Xray>,
        xray_has_blocked_out_layers: bool,
        window: &W::Id,
    ) {
        let _span = clarion_tracy_span!("Layout::store_unmap_snapshot");

        if self.is_window_spread_visible() {
            for mon in self.monitors_mut() {
                if let Some((_, geo, zoom)) = mon
                    .spread_tiles()
                    .into_iter()
                    .find(|(tile, _, _)| tile.window().id() == window)
                {
                    let tile = mon
                        .workspaces
                        .iter_mut()
                        .flat_map(Workspace::tiles_mut)
                        .find(|tile| tile.window().id() == window)
                        .unwrap();
                    tile.store_unmap_snapshot_any_if_empty(
                        renderer,
                        xray,
                        xray_has_blocked_out_layers,
                        XrayPos::new(geo.loc, zoom),
                    );
                    return;
                }
            }
        }

        let zoom = self.interactive_move_zoom();

        if let Some(InteractiveMoveState::Moving(move_)) = &mut self.interactive_move {
            if move_.tile.window().id() == window {
                let pos_within_output = move_.tile_render_location(zoom);

                // Computation matches update_render_elements().
                let view_rect =
                    Rectangle::new(pos_within_output.upscale(-1.), output_size(&move_.output))
                        .downscale(zoom);
                move_.tile.update_render_elements(false, view_rect);

                move_.tile.store_unmap_snapshot_any_if_empty(
                    renderer,
                    xray,
                    xray_has_blocked_out_layers,
                    XrayPos::new(pos_within_output, zoom),
                );
                return;
            }
        }

        match &mut self.monitor_set {
            MonitorSet::Normal { monitors, .. } => {
                for mon in monitors {
                    for (ws, geo) in mon.workspaces_with_render_geo_mut(false) {
                        if ws.has_window(window) {
                            let zoom = (geo.size.w / ws.view_size().w).max(f64::EPSILON);
                            ws.store_any_unmap_snapshot_if_empty(
                                renderer,
                                xray,
                                xray_has_blocked_out_layers,
                                XrayPos::new(geo.loc, zoom),
                                window,
                            );
                            return;
                        }
                    }
                }
            }
            MonitorSet::NoOutputs { workspaces, .. } => {
                for ws in workspaces {
                    if ws.has_window(window) {
                        ws.store_any_unmap_snapshot_if_empty(
                            renderer,
                            xray,
                            xray_has_blocked_out_layers,
                            XrayPos::default(),
                            window,
                        );
                        return;
                    }
                }
            }
        }
    }

    pub fn store_unmap_snapshot(
        &mut self,
        renderer: &mut GlesRenderer,
        xray: Option<&mut Xray>,
        xray_has_blocked_out_layers: bool,
        window: &W::Id,
    ) {
        self.store_unmap_snapshot_any(
            &mut AnyRenderer::Gles(renderer),
            xray,
            xray_has_blocked_out_layers,
            window,
        );
    }

    pub fn store_unmap_snapshot_native(
        &mut self,
        renderer: &mut crate::render_helpers::vulkan::native::NativeRenderer,
        xray: Option<&mut Xray>,
        xray_has_blocked_out_layers: bool,
        window: &W::Id,
    ) {
        self.store_unmap_snapshot_any(
            &mut AnyRenderer::Native(renderer),
            xray,
            xray_has_blocked_out_layers,
            window,
        );
    }

    pub fn clear_unmap_snapshot(&mut self, window: &W::Id) {
        if let Some(InteractiveMoveState::Moving(move_)) = &mut self.interactive_move {
            if move_.tile.window().id() == window {
                let _ = move_.tile.take_unmap_snapshot();
                return;
            }
        }

        match &mut self.monitor_set {
            MonitorSet::Normal { monitors, .. } => {
                for mon in monitors {
                    for ws in &mut mon.workspaces {
                        if ws.has_window(window) {
                            ws.clear_unmap_snapshot(window);
                            return;
                        }
                    }
                }
            }
            MonitorSet::NoOutputs { workspaces, .. } => {
                for ws in workspaces {
                    if ws.has_window(window) {
                        ws.clear_unmap_snapshot(window);
                        return;
                    }
                }
            }
        }
    }

    pub fn start_close_animation_for_window<'a>(
        &mut self,
        renderer: impl Into<AnyRenderer<'a>>,
        window: &W::Id,
        blocker: TransactionBlocker,
    ) {
        let _span = clarion_tracy_span!("Layout::start_close_animation_for_window");
        let mut renderer = renderer.into();

        if self.is_window_spread_visible() {
            if let Some(mon) = self.monitors_mut().find(|mon| mon.has_window(window)) {
                if let Some((idx, origin)) = mon.spread_close_origin(window) {
                    let stable_id = {
                        let ws = &mut mon.workspaces[idx];
                        let before = ws.closing_windows().count();
                        ws.start_close_animation_for_window_at(
                            &mut renderer,
                            window,
                            origin.source_pos,
                            blocker,
                        );
                        if ws.closing_windows().count() > before {
                            Some(ws.closing_windows().last().unwrap().stable_id().clone())
                        } else {
                            None
                        }
                    };
                    if let Some(stable_id) = stable_id {
                        mon.register_spread_closing(stable_id, origin);
                    }
                    return;
                }
            }
        }

        let zoom = self.interactive_move_zoom();

        let Layout {
            interactive_move,
            monitor_set,
            ..
        } = self;

        if let Some(InteractiveMoveState::Moving(move_)) = interactive_move {
            if move_.tile.window().id() == window {
                let tile_pos = move_.tile_render_location(zoom);
                let output = &move_.output;
                let pointer_pos_within_output = move_.pointer_pos_within_output;
                let Some(mon) = monitor_set.monitors_mut().find(|mon| &mon.output == output) else {
                    return;
                };
                let Some((ws, ws_geo)) = mon.workspace_under(pointer_pos_within_output) else {
                    return;
                };
                let idx = mon.idx_of_ws(ws.id()).unwrap();
                let ws = &mut mon.workspaces[idx];

                let tile_pos = tile_pos - ws_geo.loc;
                ws.start_close_animation_for_dragged_tile(
                    &mut renderer,
                    &move_.tile,
                    tile_pos,
                    blocker,
                );
                return;
            }
        }

        match monitor_set {
            MonitorSet::Normal { monitors, .. } => {
                for mon in monitors {
                    for ws in &mut mon.workspaces {
                        if ws.has_window(window) {
                            ws.start_close_animation_for_window(&mut renderer, window, blocker);
                            return;
                        }
                    }
                }
            }
            MonitorSet::NoOutputs { workspaces, .. } => {
                for ws in workspaces {
                    if ws.has_window(window) {
                        ws.start_close_animation_for_window(&mut renderer, window, blocker);
                        return;
                    }
                }
            }
        }
    }

    pub fn start_close_animation_for_window_native(
        &mut self,
        renderer: &mut crate::render_helpers::vulkan::native::NativeRenderer,
        window: &W::Id,
        blocker: TransactionBlocker,
    ) {
        self.start_close_animation_for_window(renderer, window, blocker);
    }

    pub fn refresh(&mut self, is_active: bool) {
        let _span = clarion_tracy_span!("Layout::refresh");

        self.is_active = is_active;
        let overview_is_open = self.is_overview_open();

        let mut ongoing_scrolling_dnd = self.dnd.is_some().then_some(true);

        if let Some(InteractiveMoveState::Moving(move_)) = &mut self.interactive_move {
            let floating = move_.is_floating();
            let win = move_.tile.window_mut();

            win.set_active_in_column(true);
            win.set_floating(floating);
            win.set_activated(true);

            win.set_interactive_resize(None);

            win.set_bounds(output_size(&move_.output).to_i32_round());

            win.send_pending_configure();
            win.refresh();

            ongoing_scrolling_dnd.get_or_insert(!floating);
        } else if let Some(InteractiveMoveState::Starting { window_id, .. }) =
            &self.interactive_move
        {
            ongoing_scrolling_dnd.get_or_insert_with(|| {
                let id = self.core.id(window_id).unwrap();
                let core = self.core.borrow();
                let interaction = core.interaction().unwrap();
                assert_eq!(interaction.id, id);
                !interaction.controls.placement().floating
            });
        }

        match &mut self.monitor_set {
            MonitorSet::Normal { monitors, .. } => {
                for mon in monitors.iter_mut() {
                    let is_active = self.is_active
                        && mon.is_selected_output()
                        && !matches!(self.interactive_move, Some(InteractiveMoveState::Moving(_)));

                    if ongoing_scrolling_dnd.is_some() && overview_is_open {
                        // Begin the scroll on new monitors and when opening the overview.
                        mon.dnd_scroll_gesture_begin();
                    } else if !overview_is_open {
                        mon.dnd_scroll_gesture_end();
                    }

                    let active_workspace = mon.active_workspace_idx();
                    for (ws_idx, ws) in mon.workspaces.iter_mut().enumerate() {
                        let is_focused = is_active && ws_idx == active_workspace;
                        ws.refresh(is_active, is_focused);

                        if let Some(is_scrolling) = ongoing_scrolling_dnd {
                            // Lock or unlock the view for scrolling interactive move.
                            if is_scrolling {
                                ws.dnd_scroll_gesture_begin();
                            } else {
                                ws.dnd_scroll_gesture_end();
                            }
                        } else {
                            // Cancel the view offset gesture after workspace switches, moves, etc.
                            if !overview_is_open && ws_idx != active_workspace {
                                ws.view_offset_gesture_end(None);
                            }
                        }
                    }
                }
            }
            MonitorSet::NoOutputs { workspaces, .. } => {
                for ws in workspaces {
                    ws.refresh(false, false);
                    ws.view_offset_gesture_end(None);
                }
            }
        }
    }

    pub fn workspaces(
        &self,
    ) -> impl Iterator<Item = (Option<&Monitor<W>>, usize, &Workspace<W>)> + '_ {
        let (iter_normal, iter_no_outputs) = match &self.monitor_set {
            MonitorSet::Normal { monitors, .. } => {
                let it = monitors.iter().flat_map(|mon| {
                    mon.workspaces
                        .iter()
                        .enumerate()
                        .map(move |(idx, ws)| (Some(mon), idx, ws))
                });

                (Some(it), None)
            }
            MonitorSet::NoOutputs { workspaces } => {
                let it = workspaces
                    .iter()
                    .enumerate()
                    .map(|(idx, ws)| (None, idx, ws));

                (None, Some(it))
            }
        };

        let iter_normal = iter_normal.into_iter().flatten();
        let iter_no_outputs = iter_no_outputs.into_iter().flatten();
        iter_normal.chain(iter_no_outputs)
    }

    pub fn workspaces_mut(&mut self) -> impl Iterator<Item = &mut Workspace<W>> + '_ {
        let (iter_normal, iter_no_outputs) = match &mut self.monitor_set {
            MonitorSet::Normal { monitors, .. } => {
                let it = monitors
                    .iter_mut()
                    .flat_map(|mon| mon.workspaces.iter_mut());

                (Some(it), None)
            }
            MonitorSet::NoOutputs { workspaces } => {
                let it = workspaces.iter_mut();

                (None, Some(it))
            }
        };

        let iter_normal = iter_normal.into_iter().flatten();
        let iter_no_outputs = iter_no_outputs.into_iter().flatten();
        iter_normal.chain(iter_no_outputs)
    }

    pub fn windows(&self) -> impl Iterator<Item = (Option<&Monitor<W>>, &W)> {
        let moving_window = self
            .interactive_move
            .as_ref()
            .and_then(|x| x.moving())
            .map(|move_| (self.monitor_for_output(&move_.output), move_.tile.window()))
            .into_iter();

        let rest = self
            .workspaces()
            .flat_map(|(mon, _, ws)| ws.windows().map(move |win| (mon, win)));

        moving_window.chain(rest)
    }

    pub fn has_window(&self, window: &W::Id) -> bool {
        self.windows().any(|(_, win)| win.id() == window)
    }

    /// Skip expensive close snapshots only for background tabs, which do not animate on close.
    /// Unknown windows and interactive moves preserve the previous snapshot behavior.
    pub fn should_capture_close_snapshot(&self, window: &W::Id) -> bool {
        if self
            .interactive_move
            .as_ref()
            .and_then(|move_| move_.moving())
            .is_some_and(|move_| move_.tile.window().id() == window)
        {
            return true;
        }

        self.workspaces()
            .find_map(|(_, _, ws)| ws.should_capture_close_snapshot(window))
            .unwrap_or(true)
    }

    pub fn is_overview_open(&self) -> bool {
        self.core.borrow().overview().desired_open()
    }
}

fn detached_workspace_idx<W: LayoutElement>(
    core: &core_adapter::DesktopContext<W::Id>,
    workspaces: &[Workspace<W>],
) -> usize {
    let id = core
        .borrow()
        .navigation()
        .detached_spaces()
        .first()
        .copied()
        .expect("a detached target must have a registered desktop identity");
    workspaces
        .iter()
        .position(|workspace| workspace.id().get() == id.0)
        .expect("detached desktop identity must have a device payload")
}

fn selected_monitor_idx<W: LayoutElement>(monitors: &[Monitor<W>]) -> usize {
    let core = monitors
        .first()
        .expect("normal monitor set cannot be empty")
        .core
        .borrow();
    let selected = core
        .navigation()
        .selected_output()
        .expect("connected monitor set must have a selected output");
    monitors
        .iter()
        .position(|monitor| monitor.output_name() == selected)
        .expect("selected output must have a device payload")
}

fn primary_monitor_idx<W: LayoutElement>(monitors: &[Monitor<W>]) -> usize {
    let core = monitors
        .first()
        .expect("normal monitor set cannot be empty")
        .core
        .borrow();
    let first = core
        .navigation()
        .output_order()
        .first()
        .expect("connected monitor set must have an output order");
    monitors
        .iter()
        .position(|monitor| monitor.output_name() == first)
        .expect("primary output must have a device payload")
}

impl<W: LayoutElement> Default for MonitorSet<W> {
    fn default() -> Self {
        Self::NoOutputs { workspaces: vec![] }
    }
}

fn compute_overview_zoom(options: &Options, overview_progress: Option<f64>) -> f64 {
    // Clamp to some sane values.
    let zoom = options.overview.zoom.clamp(0.0001, 0.75);

    if let Some(p) = overview_progress {
        (1. - p * (1. - zoom)).max(0.0001)
    } else {
        1.
    }
}

impl<W: LayoutElement> Layout<W> {
    /// Scene damage resolves previous/current animated bounds at the next frame.
    pub(crate) fn drain_core_scene_outputs(&self, output: &mut Vec<String>) {
        self.core.drain_scene_outputs(output);
    }
}
