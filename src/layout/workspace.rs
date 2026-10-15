use std::cell::Ref;
use std::cmp::max;
use std::rc::Rc;
use std::time::Duration;

use realand_config::utils::MergeWith as _;
use realand_config::{CenterFocusedColumn, CornerRadius, PresetSize, Workspace as WorkspaceConfig};
use realand_core::workspace_state::{
    CanvasPolicyMemory, MinimizeRestore, RestoreColumn, RestoreDestination, SpaceLayerFocus,
};
use realand_core::{CoreEvent, Layer, LayoutMode, SpaceId};
use realand_ipc::{ColumnDisplay, PositionChange, SizeChange, WindowLayout};

use super::core_adapter::{self, DesktopContext};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::desktop::{layer_map_for_output, Window};
use smithay::output::Output;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle, Serial, Size, Transform};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::SurfaceCachedState;

use super::closing_window::ClosingWindow;
use super::dwindle::{DwindleSpace, DwindleSpaceRenderElement};
use super::floating::{FloatingSpace, FloatingSpaceRenderElement};
use super::scrolling::{
    Column, ColumnWidth, ScrollDirection, ScrollingSpace, ScrollingSpaceRenderElement,
};
use super::shadow::Shadow;
use super::tile::{Tile, TileRenderElement, TileRenderSnapshot};
use super::{
    ActivateWindow, HitType, InsertPosition, LayoutElement, Options, RemovedTile, SizeFrac,
};
use crate::animation::Clock;
use crate::layout::RenderLayer;
use crate::realand_render_elements;
use crate::render_helpers::renderer::RealandRenderer;
use crate::render_helpers::shadow::ShadowRenderElement;
use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
use crate::render_helpers::xray::{Xray, XrayPos};
use crate::render_helpers::RenderCtx;
use crate::utils::id::IdCounter;
use crate::utils::transaction::{Transaction, TransactionBlocker};
use crate::utils::{
    ensure_min_max_size, ensure_min_max_size_maybe_zero, output_size, send_scale_transform,
    ResizeEdge,
};
use crate::window::ResolvedWindowRules;

#[derive(Debug)]
pub struct Workspace<W: LayoutElement> {
    /// Shared authoritative identity, mode and visibility state.
    pub(super) core: DesktopContext<W::Id>,
    core_transition_depth: usize,

    /// The scrollable-tiling layout.
    scrolling: ScrollingSpace<W>,

    /// The dynamic dwindle / bsp layout.
    pub(super) dwindle: DwindleSpace<W>,

    /// The floating layout.
    floating: FloatingSpace<W>,
    /// Mapped tiles detached from ordinary render, focus and hit-test spaces.
    minimized: Vec<ParkedTile<W>>,

    /// Current output of this workspace.
    output: Option<Output>,

    /// Latest known output scale for this workspace.
    ///
    /// This should be set from the current workspace output, or, if all outputs have been
    /// disconnected, preserved until a new output is connected.
    scale: smithay::output::Scale,

    /// Latest known output transform for this workspace.
    ///
    /// This should be set from the current workspace output, or, if all outputs have been
    /// disconnected, preserved until a new output is connected.
    transform: Transform,

    /// Latest known view size for this workspace.
    ///
    /// This should be computed from the current workspace output size, or, if all outputs have
    /// been disconnected, preserved until a new output is connected.
    view_size: Size<f64, Logical>,

    /// Latest known working area for this workspace.
    ///
    /// Not rounded to physical pixels.
    ///
    /// This is similar to view size, but takes into account things like layer shell exclusive
    /// zones.
    working_area: Rectangle<f64, Logical>,

    /// This workspace's shadow in the overview.
    shadow: Shadow,

    /// This workspace's background.
    background_buffer: SolidColorBuffer,

    /// Clock for driving animations.
    pub(super) clock: Clock,

    /// Configurable properties of the layout as received from the parent monitor.
    pub(super) base_options: Rc<Options>,

    /// Configurable properties of the layout with logical sizes adjusted for the current `scale`.
    pub(super) options: Rc<Options>,

    /// Layout config overrides for this workspace.
    layout_config: Option<realand_config::LayoutPart>,

    /// Unique ID of this workspace.
    id: WorkspaceId,
}

impl<W: LayoutElement> Drop for Workspace<W> {
    fn drop(&mut self) {
        self.core.retire_space(SpaceId(self.id.get()));
    }
}

#[derive(Debug)]
struct ParkedTile<W: LayoutElement> {
    tile: Tile<W>,
    /// Snapshot pose for the renderer's Genie payload; restore identities and
    /// placement preferences are owned by WindowRecord in the pure core.
    source: Rectangle<f64, Logical>,
}

static WORKSPACE_ID_COUNTER: IdCounter = IdCounter::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WorkspaceId(u64);

impl Default for WorkspaceId {
    fn default() -> Self {
        Self::next()
    }
}

impl WorkspaceId {
    fn next() -> WorkspaceId {
        WorkspaceId(WORKSPACE_ID_COUNTER.next())
    }

    pub fn get(self) -> u64 {
        self.0
    }

    pub fn specific(id: u64) -> Self {
        Self(id)
    }
}

realand_render_elements! {
    WorkspaceRenderElement<R> => {
        Scrolling = ScrollingSpaceRenderElement<R>,
        Floating = FloatingSpaceRenderElement<R>,
        Dwindle = DwindleSpaceRenderElement<R>,
        Minimized = TileRenderElement<R>,
    }
}

/// Resolved width or height in logical pixels.
#[derive(Debug, Clone, Copy)]
pub enum ResolvedSize {
    /// Size of the tile including borders.
    Tile(f64),
    /// Size of the window excluding borders.
    Window(f64),
}

/// Where to put a newly added window.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceAddWindowTarget<'a, W: LayoutElement> {
    /// No particular preference.
    #[default]
    Auto,
    /// As a new column at this index.
    NewColumnAt(usize),
    /// Next to this existing window.
    NextTo(&'a W::Id),
}

impl<W: LayoutElement> Workspace<W> {
    pub fn new(output: Output, clock: Clock, options: Rc<Options>) -> Self {
        Self::new_with_config(output, None, clock, options)
    }

    pub fn new_with_config(
        output: Output,
        mut config: Option<WorkspaceConfig>,
        clock: Clock,
        base_options: Rc<Options>,
    ) -> Self {
        let original_output = config
            .as_ref()
            .and_then(|c| c.open_on_output.clone())
            .unwrap_or_else(|| core_adapter::output_identity(&output).canonical_name());

        let layout_config = config.as_mut().and_then(|c| c.layout.take().map(|x| x.0));

        let scale = output.current_scale();
        let options = Rc::new(
            Options::clone(&base_options)
                .with_merged_layout(layout_config.as_ref())
                .adjusted_for_scale(scale.fractional_scale()),
        );

        let view_size = output_size(&output);
        let working_area = compute_working_area(&output);

        let scrolling = ScrollingSpace::new(
            view_size,
            working_area,
            scale.fractional_scale(),
            clock.clone(),
            options.clone(),
        );

        let floating = FloatingSpace::new(
            view_size,
            working_area,
            scale.fractional_scale(),
            clock.clone(),
            options.clone(),
        );

        let dwindle = DwindleSpace::new(
            view_size,
            working_area,
            scale.fractional_scale(),
            clock.clone(),
            options.clone(),
        );

        let shadow_config =
            compute_workspace_shadow_config(options.overview.workspace_shadow, view_size);

        let id = WorkspaceId::next();
        let core = core_adapter::create_space(SpaceId(id.get()), Some(&output));
        core.borrow_mut()
            .set_space_output_preference(SpaceId(id.get()), original_output);
        core.borrow_mut()
            .set_space_name(SpaceId(id.get()), config.map(|config| config.name.0));
        Self {
            core,
            core_transition_depth: 0,
            scrolling,
            dwindle,
            floating,
            minimized: Vec::new(),
            scale,
            transform: output.current_transform(),
            view_size,
            working_area,
            shadow: Shadow::new(shadow_config),
            background_buffer: SolidColorBuffer::new(view_size, options.layout.background_color),
            output: Some(output),
            clock,
            base_options,
            options,
            layout_config,
            id,
        }
    }

    pub fn new_with_config_no_outputs(
        mut config: Option<WorkspaceConfig>,
        clock: Clock,
        base_options: Rc<Options>,
    ) -> Self {
        let original_output = config
            .as_ref()
            .and_then(|c| c.open_on_output.clone())
            .unwrap_or_default();

        let layout_config = config.as_mut().and_then(|c| c.layout.take().map(|x| x.0));

        let scale = smithay::output::Scale::Integer(1);
        let options = Rc::new(
            Options::clone(&base_options)
                .with_merged_layout(layout_config.as_ref())
                .adjusted_for_scale(scale.fractional_scale()),
        );

        let view_size = Size::from((1280., 720.));
        let working_area = Rectangle::from_size(Size::from((1280., 720.)));

        let scrolling = ScrollingSpace::new(
            view_size,
            working_area,
            scale.fractional_scale(),
            clock.clone(),
            options.clone(),
        );

        let floating = FloatingSpace::new(
            view_size,
            working_area,
            scale.fractional_scale(),
            clock.clone(),
            options.clone(),
        );

        let dwindle = DwindleSpace::new(
            view_size,
            working_area,
            scale.fractional_scale(),
            clock.clone(),
            options.clone(),
        );

        let shadow_config =
            compute_workspace_shadow_config(options.overview.workspace_shadow, view_size);

        let id = WorkspaceId::next();
        let core = core_adapter::create_space(SpaceId(id.get()), None);
        core.borrow_mut()
            .set_space_output_preference(SpaceId(id.get()), original_output);
        core.borrow_mut()
            .set_space_name(SpaceId(id.get()), config.map(|config| config.name.0));
        Self {
            core,
            core_transition_depth: 0,
            scrolling,
            dwindle,
            floating,
            minimized: Vec::new(),
            output: None,
            scale,
            transform: Transform::Normal,
            view_size,
            working_area,
            shadow: Shadow::new(shadow_config),
            background_buffer: SolidColorBuffer::new(view_size, options.layout.background_color),
            clock,
            base_options,
            options,
            layout_config,
            id,
        }
    }

    pub fn new_no_outputs(clock: Clock, options: Rc<Options>) -> Self {
        Self::new_with_config_no_outputs(None, clock, options)
    }

    /// Attach once to the session core. This transfers constructor state; it
    /// is never called during frame refresh or used to rebuild a state mirror.
    pub(super) fn set_core(&mut self, context: DesktopContext<W::Id>) {
        if Rc::ptr_eq(&self.core, &context) {
            return;
        }
        let space = SpaceId(self.id.get());
        let mode = self
            .core
            .borrow()
            .space(space)
            .map(|record| record.mode)
            .unwrap_or_default();
        let history = self
            .core
            .borrow()
            .space(space)
            .map(|record| record.history.clone())
            .unwrap_or_default();
        let interaction = self
            .core
            .borrow()
            .space(space)
            .map(|record| record.interaction.clone())
            .unwrap_or_default();
        let output_preference = self
            .core
            .borrow()
            .space(space)
            .map(|record| record.output_preference.clone())
            .unwrap_or_default();
        let name = self
            .core
            .borrow()
            .space(space)
            .and_then(|record| record.name.clone());
        let records: Vec<_> = self
            .core
            .borrow()
            .windows()
            .filter(|record| record.space == space && record.attached)
            .cloned()
            .collect();
        self.core = context;
        for window in self.windows() {
            self.core.register(window.id().clone(), window.core_id());
        }
        if let Some(output) = &self.output {
            core_adapter::connect_output(&self.core, output);
        }
        self.core
            .borrow_mut()
            .apply(CoreEvent::CreateSpace { id: space, mode });
        self.core
            .borrow_mut()
            .set_space_output_preference(space, output_preference);
        self.core.borrow_mut().set_space_name(space, name);
        if let Some(snapshot) = history.scrolling {
            self.core.borrow_mut().capture_scrolling(space, snapshot);
        }
        if let Some(snapshot) = history.dwindle {
            self.core.borrow_mut().capture_dwindle(space, snapshot);
        }
        for record in records {
            let exists = self.core.borrow().window(&record.id).is_some();
            if exists {
                self.core.borrow_mut().move_window(
                    &record.id,
                    space,
                    self.output.as_ref().map(Output::name),
                );
                self.core
                    .borrow_mut()
                    .set_minimized(&record.id, record.minimized);
            } else {
                self.core
                    .borrow_mut()
                    .apply(CoreEvent::MapWindow { window: record });
            }
        }
        self.core
            .borrow_mut()
            .apply(CoreEvent::AdoptSpaceInteraction { space, interaction });
        self.record_core_focus(false);
    }

    pub(super) fn with_core(mut self, context: DesktopContext<W::Id>) -> Self {
        self.set_core(context);
        self
    }

    fn register_core_tile(&self, tile: &Tile<W>, floating: bool) {
        let id = tile.window().core_id();
        self.core.register(tile.window().id().clone(), id);
        let output = self.output.as_ref().map(Output::name);
        let geometry = core_adapter::rect(Rectangle::from_size(tile.tile_size()));
        let mut core = self.core.borrow_mut();
        if core.window(&id).is_some() {
            core.move_window(&id, SpaceId(self.id.get()), output.clone());
            core.set_minimized(&id, false);
        } else {
            core.add_window(id, SpaceId(self.id.get()), output.clone(), geometry);
        }
        // A workspace can be intentionally detached while another output lives.
        core.set_output(&id, output, false);
        core.apply(CoreEvent::SetLayer {
            id,
            layer: if floating {
                Layer::Above
            } else {
                Layer::Normal
            },
        });
    }

    /// Commit resolved policy targets once at a user/client transition boundary.
    /// Animated offsets are scene transforms, never saved desktop geometry.
    pub(super) fn commit_core_geometry(&self) {
        let mut core = self.core.borrow_mut();
        let mut commit = |tile: &Tile<W>, geometry: Rectangle<f64, Logical>, floating: bool| {
            let id = tile.window().core_id();
            let layer = if floating {
                Layer::Above
            } else {
                Layer::Normal
            };
            if !core.window(&id).is_some_and(|record| {
                record.attached && record.space == SpaceId(self.id.get()) && !record.minimized
            }) {
                return;
            }
            if core.window(&id).is_some_and(|record| record.layer != layer) {
                core.apply(CoreEvent::SetLayer { id, layer });
            }
            let minimum = tile.window().min_size();
            let maximum = tile.window().max_size();
            let minimum =
                realand_core::Size::new(f64::from(minimum.w), f64::from(minimum.h)).sanitized();
            let maximum =
                realand_core::Size::new(f64::from(maximum.w), f64::from(maximum.h)).sanitized();
            if core
                .window(&id)
                .is_some_and(|record| record.min_size != minimum || record.max_size != maximum)
            {
                core.apply(CoreEvent::SetConstraints {
                    id,
                    minimum,
                    maximum,
                });
            }
            core.commit_geometry(&id, core_adapter::rect(geometry));
        };
        for (tile, offset) in self.floating.tiles_with_offsets() {
            commit(
                tile,
                Rectangle::new(offset, tile.tile_expected_or_current_size()),
                true,
            );
        }
        self.scrolling
            .with_layout_geometry(&mut |tile, rect| commit(tile, rect, false));
        self.dwindle
            .with_layout_geometry(&mut |tile, rect| commit(tile, rect, false));
    }

    /// Kinetic camera ticks only acknowledge already-resolved floating targets.
    /// They do not recalculate constraints, tiling, focus, or client configure.
    fn commit_canvas_geometry(&self) {
        let mut core = self.core.borrow_mut();
        for (tile, offset) in self.floating.tiles_with_offsets() {
            let id = tile.window().core_id();
            if core.window(&id).is_some_and(|window| {
                window.attached
                    && !window.minimized
                    && window.space == SpaceId(self.id.get())
                    && window.layer == Layer::Above
            }) {
                core.commit_geometry(
                    &id,
                    core_adapter::rect(Rectangle::new(
                        offset,
                        tile.tile_expected_or_current_size(),
                    )),
                );
            }
        }
    }

    fn with_layout_transition<T>(&mut self, operation: impl FnOnce(&mut Self) -> T) -> T {
        self.core_transition_depth += 1;
        let result = operation(self);
        self.core_transition_depth -= 1;
        if self.core_transition_depth == 0 {
            self.commit_core_geometry();
            self.record_core_focus(false);
        }
        result
    }

    pub(super) fn record_core_focus(&self, activate: bool) {
        let id = self.active_window().map(LayoutElement::core_id);
        self.core
            .borrow_mut()
            .set_space_focus(SpaceId(self.id.get()), id, activate);
    }

    pub fn tiling_mode(&self) -> realand_ipc::TilingMode {
        match self
            .core
            .borrow()
            .space(SpaceId(self.id.get()))
            .map(|space| space.mode)
            .unwrap_or(LayoutMode::Scrolling)
        {
            LayoutMode::Dwindle => realand_ipc::TilingMode::Dwindle,
            LayoutMode::Floating | LayoutMode::Scrolling => realand_ipc::TilingMode::Scrolling,
        }
    }

    fn tiling_is_empty(&self) -> bool {
        match self.tiling_mode() {
            realand_ipc::TilingMode::Dwindle => self.dwindle.is_empty(),
            realand_ipc::TilingMode::Scrolling => self.scrolling.is_empty(),
        }
    }

    pub fn toggle_tiling_mode(&mut self) {
        match self.tiling_mode() {
            realand_ipc::TilingMode::Scrolling => {
                self.set_tiling_mode(realand_ipc::TilingMode::Dwindle)
            }
            realand_ipc::TilingMode::Dwindle => {
                self.set_tiling_mode(realand_ipc::TilingMode::Scrolling)
            }
        }
    }

    pub fn set_tiling_mode(&mut self, mode: realand_ipc::TilingMode) {
        self.with_layout_transition(|this| {
            if this.canvas_enabled() {
                this.toggle_canvas();
            }
            if this.tiling_mode() == mode {
                return;
            }
            let space = SpaceId(this.id.get());
            match this.tiling_mode() {
                realand_ipc::TilingMode::Scrolling => this
                    .core
                    .borrow_mut()
                    .capture_scrolling(space, this.scrolling.policy_snapshot()),
                realand_ipc::TilingMode::Dwindle => this
                    .core
                    .borrow_mut()
                    .capture_dwindle(space, this.dwindle.policy_snapshot()),
            }
            this.core.borrow_mut().set_layout_mode(
                SpaceId(this.id.get()),
                match mode {
                    realand_ipc::TilingMode::Scrolling => LayoutMode::Scrolling,
                    realand_ipc::TilingMode::Dwindle => LayoutMode::Dwindle,
                },
            );

            // Migrate windows between the two tiling spaces so that every window always lives in
            // the space of the active tiling mode, and can be focused, rendered and closed no
            // matter which mode it was originally opened in.
            match mode {
                realand_ipc::TilingMode::Dwindle => {
                    let active_id = this.scrolling.active_window().map(|win| win.id().clone());

                    let closing = this.scrolling.take_closing_windows();
                    this.dwindle.extend_closing_windows(closing);

                    this.dwindle
                        .add_tiles(this.scrolling.drain_tiles(), ActivateWindow::No);
                    let saved = this
                        .core
                        .borrow()
                        .space(space)
                        .and_then(|record| record.history.dwindle.clone());
                    if let Some(saved) = saved {
                        this.dwindle.restore_policy(&saved);
                    }

                    if let Some(id) = active_id {
                        this.dwindle.set_active_window(id);
                    }
                }
                realand_ipc::TilingMode::Scrolling => {
                    let active_id = this.dwindle.active_window_id().cloned();

                    let closing = this.dwindle.take_closing_windows();
                    this.scrolling.extend_closing_windows(closing);

                    let saved = this
                        .core
                        .borrow()
                        .space(space)
                        .and_then(|record| record.history.scrolling.clone());
                    let tiles = this.dwindle.drain_tiles();
                    if let Some(saved) = saved {
                        let active = active_id.as_ref().and_then(|id| this.core.id(id));
                        this.scrolling.restore_policy(&saved, tiles, active);
                    } else {
                        let default_width = this
                            .options
                            .layout
                            .default_column_width
                            .map(super::scrolling::column_width_from_preset)
                            .unwrap_or(ColumnWidth::Proportion(0.5));
                        for tile in tiles {
                            this.scrolling
                                .add_tile(None, tile, false, default_width, false, None);
                        }
                    }

                    if let Some(id) = active_id {
                        this.scrolling.activate_window(&id);
                    }
                }
            }
        })
    }

    pub fn toggle_dwindle_split(&mut self) {
        if self.tiling_mode() == realand_ipc::TilingMode::Dwindle {
            self.dwindle.toggle_split_direction();
        }
    }

    pub fn id(&self) -> WorkspaceId {
        self.id
    }

    /// Borrow the single core-owned string. Render/lookup paths do not clone it.
    pub fn name(&self) -> Option<Ref<'_, String>> {
        Ref::filter_map(self.core.borrow(), |core| {
            core.space(SpaceId(self.id.get()))?.name.as_ref()
        })
        .ok()
    }

    pub fn is_scratchpad(&self) -> bool {
        self.core
            .borrow()
            .space_is_scratchpad(SpaceId(self.id.get()))
    }

    pub fn unname(&mut self) {
        self.set_name(None);
    }

    pub(super) fn set_name(&self, name: Option<String>) -> bool {
        self.core
            .borrow_mut()
            .set_space_name(SpaceId(self.id.get()), name)
    }

    pub fn has_windows_or_name(&self) -> bool {
        self.has_windows() || self.name().is_some()
    }

    pub fn has_closing_windows(&self) -> bool {
        self.scrolling.has_closing_windows()
            || self.dwindle.has_closing_windows()
            || self.floating.has_closing_windows()
    }

    pub fn closing_windows(&self) -> impl Iterator<Item = &ClosingWindow> {
        self.scrolling
            .closing_windows
            .iter()
            .chain(self.dwindle.closing_windows.iter())
            .chain(self.floating.closing_windows.iter())
    }

    pub fn scale(&self) -> smithay::output::Scale {
        self.scale
    }

    pub fn advance_animations(&mut self) {
        self.scrolling.advance_animations();
        self.dwindle.advance_animations();
        if self.floating.advance_animations() {
            self.commit_canvas_geometry();
        }
        // Actual policy poses drive mutable effect simulation once at event time.
        // Rendering for any output or capture only samples the resulting state.
        if self.tiles().any(Tile::has_effect_pose) {
            for (tile, location, _) in self.tiles_with_render_positions() {
                tile.advance_effect_pose(location);
            }
        }
        for parked in &mut self.minimized {
            parked.tile.advance_animations();
        }
    }

    pub fn are_animations_ongoing(&self) -> bool {
        self.scrolling.are_animations_ongoing()
            || self.dwindle.are_animations_ongoing()
            || self.floating.are_animations_ongoing()
            || self.minimized.iter().any(|p| p.tile.genie_ongoing())
    }

    pub fn are_transitions_ongoing(&self) -> bool {
        self.scrolling.are_transitions_ongoing()
            || self.dwindle.are_transitions_ongoing()
            || self.floating.are_transitions_ongoing()
            || self.minimized.iter().any(|p| p.tile.genie_ongoing())
    }

    pub fn update_render_elements(&mut self, is_active: bool, layer: RenderLayer) {
        self.scrolling
            .update_render_elements(is_active && !self.active_layer().is_floating(), layer);

        let view_rect = Rectangle::from_size(self.view_size);
        self.dwindle.update_render_elements(
            is_active && !self.active_layer().is_floating(),
            view_rect,
            layer,
        );

        self.floating.update_render_elements(
            is_active && self.active_layer().is_floating(),
            view_rect,
            layer,
        );

        if layer.is_normal() {
            for parked in &mut self.minimized {
                let view_rect = Rectangle::new(
                    (-parked.source.loc.x, -parked.source.loc.y).into(),
                    self.view_size,
                );
                parked.tile.update_render_elements(false, view_rect);
            }
            self.update_shadow_render_elements();
        }
    }

    pub(super) fn update_shadow_render_elements(&mut self) {
        self.shadow.update_render_elements(
            self.view_size,
            true,
            CornerRadius::default(),
            self.scale.fractional_scale(),
            1.,
        );
    }

    pub fn update_config(&mut self, base_options: Rc<Options>) {
        self.with_layout_transition(|this| {
            let scale = this.scale.fractional_scale();
            let options = Rc::new(
                Options::clone(&base_options)
                    .with_merged_layout(this.layout_config.as_ref())
                    .adjusted_for_scale(scale),
            );

            this.scrolling.update_config(
                this.view_size,
                this.working_area,
                this.scale.fractional_scale(),
                options.clone(),
            );

            this.floating.update_config(
                this.view_size,
                this.working_area,
                this.scale.fractional_scale(),
                options.clone(),
            );

            this.dwindle.update_config(
                this.view_size,
                this.working_area,
                this.scale.fractional_scale(),
                options.clone(),
            );

            let shadow_config =
                compute_workspace_shadow_config(options.overview.workspace_shadow, this.view_size);
            this.shadow.update_config(shadow_config);

            this.background_buffer
                .set_color(options.layout.background_color);

            for parked in &mut this.minimized {
                parked
                    .tile
                    .update_config(this.view_size, scale, options.clone());
            }
            this.base_options = base_options;
            this.options = options;
        })
    }

    pub fn update_layout_config(&mut self, layout_config: Option<realand_config::LayoutPart>) {
        if self.layout_config == layout_config {
            return;
        }

        self.layout_config = layout_config;
        self.update_config(self.base_options.clone());
    }

    pub fn update_shaders(&mut self) {
        self.scrolling.update_shaders();
        for tile in self.dwindle.tiles_mut() {
            tile.update_shaders();
        }
        self.floating.update_shaders();
        for parked in &mut self.minimized {
            parked.tile.update_shaders();
        }
        self.shadow.update_shaders();
    }

    pub fn windows(&self) -> impl Iterator<Item = &W> + '_ {
        self.tiles().map(Tile::window)
    }

    /// Membership in the IPC traversal order, without building window geometry.
    pub(super) fn with_windows_in_ipc_order(&self, f: &mut impl FnMut(&W)) {
        for tile in self.floating.tiles().chain(self.scrolling.tiles()) {
            f(tile.window());
        }
        self.dwindle.with_tiles(&mut |tile| f(tile.window()));
        for parked in &self.minimized {
            f(parked.tile.window());
        }
    }

    pub fn windows_mut(&mut self) -> impl Iterator<Item = &mut W> + '_ {
        self.tiles_mut().map(Tile::window_mut)
    }

    pub fn tiles(&self) -> impl Iterator<Item = &Tile<W>> + '_ {
        let scrolling = self.scrolling.tiles();
        let dwindle = self.dwindle.tiles();
        let floating = self.floating.tiles();
        scrolling
            .chain(dwindle)
            .chain(floating)
            .chain(self.minimized.iter().map(|p| &p.tile))
    }

    pub fn tiles_mut(&mut self) -> impl Iterator<Item = &mut Tile<W>> + '_ {
        let scrolling = self.scrolling.tiles_mut();
        let dwindle = self.dwindle.tiles_mut();
        let floating = self.floating.tiles_mut();
        scrolling
            .chain(dwindle)
            .chain(floating)
            .chain(self.minimized.iter_mut().map(|p| &mut p.tile))
    }

    pub fn is_floating(&self, id: &W::Id) -> bool {
        let Some(id) = self.core.id(id) else {
            return false;
        };
        self.core.borrow().window(&id).is_some_and(|record| {
            record.attached
                && record.space == SpaceId(self.id.get())
                && record.layer == Layer::Above
        })
    }

    pub fn current_output(&self) -> Option<&Output> {
        self.output.as_ref()
    }

    pub fn active_window(&self) -> Option<&W> {
        if self.active_layer().is_floating() {
            self.floating.active_window()
        } else if self.tiling_mode() == realand_ipc::TilingMode::Dwindle {
            self.dwindle.active_window()
        } else {
            self.scrolling.active_window()
        }
    }

    pub fn active_window_mut(&mut self) -> Option<&mut W> {
        if self.active_layer().is_floating() {
            self.floating.active_window_mut()
        } else if self.tiling_mode() == realand_ipc::TilingMode::Dwindle {
            self.dwindle.active_window_mut()
        } else {
            self.scrolling.active_window_mut()
        }
    }

    pub fn is_active_pending_fullscreen(&self) -> bool {
        if self.active_layer().is_floating() {
            false
        } else if self.tiling_mode() == realand_ipc::TilingMode::Dwindle {
            self.dwindle.is_active_pending_fullscreen()
        } else {
            self.scrolling.is_active_pending_fullscreen()
        }
    }

    pub fn set_output(&mut self, output: Option<Output>) {
        self.with_layout_transition(|this| {
            if this.output == output {
                return;
            }

            for tile in this.tiles_mut() {
                tile.invalidate_minimize_target();
            }
            if let Some(output) = this.output.take() {
                for win in this.windows() {
                    win.output_leave(&output);
                }
            }

            this.output = output;
            if let Some(output) = &this.output {
                core_adapter::connect_output(&this.core, output);
            }
            if this.output.is_none() {
                this.core.borrow_mut().detach_space(SpaceId(this.id.get()));
            }
            let connector = this.output.as_ref().map(Output::name);
            let ids: Vec<_> = this.windows().map(LayoutElement::core_id).collect();
            for id in ids {
                this.core
                    .borrow_mut()
                    .set_output(&id, connector.clone(), false);
            }

            if let Some(output) = &this.output {
                this.core
                    .borrow_mut()
                    .normalize_space_output_preference(SpaceId(this.id.get()), output);

                this.update_output_size();

                for win in this.windows() {
                    this.enter_output_for_window(win);
                }
            }
        })
    }

    fn enter_output_for_window(&self, window: &W) {
        if let Some(output) = &self.output {
            window.set_preferred_scale_transform(self.scale, self.transform);
            window.output_enter(output);
        }
    }

    pub fn update_output_size(&mut self) {
        // Workspaces are detached briefly while outputs are being removed or
        // monitors are rebuilt.  During that transition there is no output
        // geometry to derive; callers will refresh the size when the
        // workspace is attached again.
        let Some(output) = self.output.as_ref() else {
            return;
        };
        let scale = output.current_scale();
        let transform = output.current_transform();
        let view_size = output_size(output);
        let working_area = compute_working_area(output);
        self.set_view_size(scale, transform, view_size, working_area);
    }

    fn set_view_size(
        &mut self,
        scale: smithay::output::Scale,
        transform: Transform,
        size: Size<f64, Logical>,
        working_area: Rectangle<f64, Logical>,
    ) {
        self.with_layout_transition(|this| {
            let scale_transform_changed = this.transform != transform
                || this.scale.integer_scale() != scale.integer_scale()
                || this.scale.fractional_scale() != scale.fractional_scale();
            if !scale_transform_changed
                && this.view_size == size
                && this.working_area == working_area
            {
                return;
            }

            let logical_extent_changed = this.view_size != size;
            for tile in this.tiles_mut() {
                if logical_extent_changed {
                    tile.invalidate_minimize_target();
                } else if scale_transform_changed {
                    // Output-local logical icon geometry remains valid. Complete
                    // effects before discarding textures at the previous scale.
                    tile.complete_genie();
                }
            }
            let fractional_scale_changed =
                this.scale.fractional_scale() != scale.fractional_scale();

            this.scale = scale;
            this.transform = transform;
            this.view_size = size;
            this.working_area = working_area;

            if fractional_scale_changed {
                // Options need to be recomputed for the new scale.
                this.update_config(this.base_options.clone());
            } else {
                // Pass our existing options as is.
                this.scrolling.update_config(
                    size,
                    working_area,
                    scale.fractional_scale(),
                    this.options.clone(),
                );
                this.floating.update_config(
                    size,
                    working_area,
                    scale.fractional_scale(),
                    this.options.clone(),
                );

                this.dwindle.update_config(
                    size,
                    working_area,
                    scale.fractional_scale(),
                    this.options.clone(),
                );

                let shadow_config =
                    compute_workspace_shadow_config(this.options.overview.workspace_shadow, size);
                this.shadow.update_config(shadow_config);
            }

            this.background_buffer.resize(size);

            if scale_transform_changed {
                for window in this.windows() {
                    window.set_preferred_scale_transform(this.scale, this.transform);
                }
            }
        })
    }

    pub fn view_size(&self) -> Size<f64, Logical> {
        self.view_size
    }

    pub fn make_tile(&self, window: W) -> Tile<W> {
        Tile::new(
            window,
            self.view_size,
            self.scale.fractional_scale(),
            self.clock.clone(),
            self.options.clone(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_tile(
        &mut self,
        mut tile: Tile<W>,
        target: WorkspaceAddWindowTarget<W>,
        activate: ActivateWindow,
        width: ColumnWidth,
        is_full_width: bool,
        is_floating: bool,
        anim: Option<realand_config::Animation>,
    ) {
        self.with_layout_transition(|this| {
            if let WorkspaceAddWindowTarget::NextTo(id) = target {
                this.restore_window(id, false);
            }
            let mut is_floating = is_floating;
            let canvas_origin = this.canvas_enabled()
                && tile.window().pending_sizing_mode().is_normal()
                && !is_floating;
            let target = if this.canvas_enabled() && tile.window().pending_sizing_mode().is_normal()
            {
                is_floating = true;
                WorkspaceAddWindowTarget::Auto
            } else {
                target
            };
            this.register_core_tile(&tile, is_floating);
            if canvas_origin {
                this.core
                    .borrow_mut()
                    .set_canvas_origin(tile.window().core_id(), true);
            }
            this.enter_output_for_window(tile.window());
            tile.restore_to_floating = is_floating;

            match target {
                WorkspaceAddWindowTarget::Auto => {
                    // Don't steal focus from an active fullscreen window.
                    let activate = activate.map_smart(|| !this.is_active_pending_fullscreen());

                    // If the tile is pending maximized or fullscreen, open it in the scrolling layout
                    // where it can do that.
                    if is_floating && tile.window().pending_sizing_mode().is_normal() {
                        this.floating.add_tile(tile, activate);

                        if activate || this.tiling_is_empty() {
                            this.set_active_layer(SpaceLayerFocus::Floating);
                        }
                    } else if this.tiling_mode() == realand_ipc::TilingMode::Dwindle {
                        let act = if activate {
                            ActivateWindow::Yes
                        } else {
                            ActivateWindow::No
                        };
                        this.dwindle.add_tile(tile, act);

                        if activate {
                            this.set_active_layer(SpaceLayerFocus::Tiling);
                        }
                    } else {
                        // Auto-stack: new windows enter the active column as a Hyprland-style
                        // dwindle 2D puzzle until it fills up, then overflow to a new column
                        // to the right (which repeats the same rule).
                        let auto_stack = this.options.layout.auto_stack_new_windows;
                        let capacity = this.options.layout.max_windows_per_column as usize;
                        let can_stack = auto_stack
                            && !this.scrolling.is_empty()
                            && this.scrolling.active_column_tile_count() < capacity
                            && this.scrolling.active_column_is_normal();

                        if can_stack {
                            this.scrolling.add_tile_to_active_column(tile, activate);
                        } else {
                            this.scrolling.add_tile(
                                None,
                                tile,
                                activate,
                                width,
                                is_full_width,
                                anim,
                            );
                        }

                        if activate {
                            this.set_active_layer(SpaceLayerFocus::Tiling);
                        }
                    }
                }
                WorkspaceAddWindowTarget::NewColumnAt(col_idx) => {
                    let activate = activate.map_smart(|| false);
                    this.scrolling.add_tile(
                        Some(col_idx),
                        tile,
                        activate,
                        width,
                        is_full_width,
                        anim,
                    );

                    if activate {
                        this.set_active_layer(SpaceLayerFocus::Tiling);
                    }
                }
                WorkspaceAddWindowTarget::NextTo(next_to) => {
                    let activate = activate
                        .map_smart(|| this.active_window().is_some_and(|win| win.id() == next_to));

                    let floating_has_window = this.floating.has_window(next_to);

                    if is_floating && tile.window().pending_sizing_mode().is_normal() {
                        if floating_has_window {
                            this.floating.add_tile_above(next_to, tile, activate);
                        } else {
                            // FIXME: use static pos
                            let (next_to_size, render_pos) = this
                                .tiles_with_render_positions()
                                .find(|(tile, _, _)| tile.window().id() == next_to)
                                .map(|(tile, pos, _)| (tile.tile_size().to_point(), pos))
                                .unwrap_or((
                                    this.working_area.size.to_point(),
                                    this.working_area.loc,
                                ));

                            // Position the new tile in the center above the next_to tile. Think a
                            // dialog opening on top of a window.
                            let tile_size = tile.tile_size();
                            let pos =
                                render_pos + (next_to_size - tile_size.to_point()).downscale(2.);
                            let pos = this.floating.clamp_within_working_area(pos, tile_size);
                            let pos = this.floating.logical_to_size_frac(pos);
                            tile.floating_pos = Some(pos);

                            this.floating.add_tile(tile, activate);
                        }

                        if activate || this.tiling_is_empty() {
                            this.set_active_layer(SpaceLayerFocus::Floating);
                        }
                    } else if this.tiling_mode() == realand_ipc::TilingMode::Dwindle {
                        let act = if activate {
                            ActivateWindow::Yes
                        } else {
                            ActivateWindow::No
                        };
                        this.dwindle.add_tile_next_to(next_to, tile, act);

                        if activate {
                            this.set_active_layer(SpaceLayerFocus::Tiling);
                        }
                    } else if floating_has_window {
                        this.scrolling
                            .add_tile(None, tile, activate, width, is_full_width, anim);

                        if activate {
                            this.set_active_layer(SpaceLayerFocus::Tiling);
                        }
                    } else {
                        this.scrolling.add_tile_right_of(
                            next_to,
                            tile,
                            activate,
                            width,
                            is_full_width,
                            anim,
                        );

                        if activate {
                            this.set_active_layer(SpaceLayerFocus::Tiling);
                        }
                    }
                }
            }
        })
    }

    pub fn add_tile_to_column(
        &mut self,
        col_idx: usize,
        tile_idx: Option<usize>,
        tile: Tile<W>,
        activate: bool,
    ) {
        self.with_layout_transition(|this| {
            this.register_core_tile(&tile, false);
            this.enter_output_for_window(tile.window());
            this.scrolling
                .add_tile_to_column(col_idx, tile_idx, tile, activate);

            if activate {
                this.set_active_layer(SpaceLayerFocus::Tiling);
            }
        })
    }

    pub fn add_column(
        &mut self,
        column: Column<W>,
        activate: bool,
        anim: Option<realand_config::Animation>,
    ) {
        self.with_layout_transition(|this| {
            for (tile, _) in column.tiles() {
                this.register_core_tile(tile, false);
                this.enter_output_for_window(tile.window());
            }

            this.scrolling.add_column(None, column, activate, anim);

            if activate {
                this.set_active_layer(SpaceLayerFocus::Tiling);
            }
        })
    }

    fn update_focus_floating_tiling_after_removing(&mut self, _removed_from_floating: bool) {
        self.core
            .borrow_mut()
            .repair_space_layer(SpaceId(self.id.get()));
    }

    pub fn remove_tile(&mut self, id: &W::Id, transaction: Transaction) -> RemovedTile<W> {
        self.with_layout_transition(|this| {
            if let Some(core_id) = this.core.id(id) {
                this.core.borrow_mut().detach_window(&core_id);
            }
            if let Some(idx) = this
                .minimized
                .iter()
                .position(|p| p.tile.window().id() == id)
            {
                let parked = this.minimized.remove(idx);
                let memory = this
                    .core
                    .borrow()
                    .window(&parked.tile.window().core_id())
                    .and_then(|record| record.restoration.clone())
                    .expect("parked payload needs core restoration memory");
                let mut removed = RemovedTile {
                    tile: parked.tile,
                    width: memory.width,
                    is_full_width: memory.full_width,
                    is_floating: memory.was_floating,
                };
                removed.tile.complete_genie();
                removed.tile.window_mut().set_minimized(false);
                if let Some(output) = &this.output {
                    removed.tile.window().output_leave(output);
                }
                return removed;
            }
            if let Some(core_id) = this.core.id(id) {
                this.core.borrow_mut().set_canvas_origin(core_id, false);
            }
            let mut from_floating = false;
            let removed = if this.floating.has_window(id) {
                from_floating = true;
                this.floating.remove_tile(id)
            } else if this.dwindle.has_window(id) {
                this.dwindle.remove_window(id).unwrap()
            } else {
                this.scrolling.remove_tile(id, transaction)
            };

            if let Some(output) = &this.output {
                removed.tile.window().output_leave(output);
            }

            this.update_focus_floating_tiling_after_removing(from_floating);

            removed
        })
    }

    pub fn remove_active_column(&mut self) -> Option<Column<W>> {
        self.with_layout_transition(|this| {
            let from_floating = this.active_layer().is_floating();
            if from_floating {
                return None;
            }

            let column = this.scrolling.remove_active_column()?;
            for (tile, _) in column.tiles() {
                this.core
                    .borrow_mut()
                    .detach_window(&tile.window().core_id());
            }

            if let Some(output) = &this.output {
                for (tile, _) in column.tiles() {
                    tile.window().output_leave(output);
                }
            }

            this.update_focus_floating_tiling_after_removing(from_floating);

            Some(column)
        })
    }

    pub fn resolve_default_width(
        &self,
        default_width: Option<Option<PresetSize>>,
        is_floating: bool,
    ) -> Option<PresetSize> {
        match default_width {
            Some(Some(width)) => Some(width),
            Some(None) => None,
            None if is_floating => None,
            None => self.options.layout.default_column_width,
        }
    }

    pub fn resolve_default_height(
        &self,
        default_height: Option<Option<PresetSize>>,
        is_floating: bool,
    ) -> Option<PresetSize> {
        match default_height {
            Some(Some(height)) => Some(height),
            Some(None) => None,
            None if is_floating => None,
            // We don't have a global default at the moment.
            None => None,
        }
    }

    pub fn new_window_size(
        &self,
        width: Option<PresetSize>,
        height: Option<PresetSize>,
        is_floating: bool,
        rules: &ResolvedWindowRules,
        (min_size, max_size): (Size<i32, Logical>, Size<i32, Logical>),
    ) -> Size<i32, Logical> {
        let mut size = if is_floating {
            self.floating.new_window_size(width, height, rules)
        } else {
            self.scrolling.new_window_size(width, height, rules)
        };

        // If the window has a fixed size, or we're picking some fixed size, apply min and max
        // size. This is to ensure that a fixed-size window rule works on open, while still
        // allowing the window freedom to pick its default size otherwise.
        let (min_size, max_size) = rules.apply_min_max_size(min_size, max_size);
        size.w = ensure_min_max_size_maybe_zero(size.w, min_size.w, max_size.w);
        // For scrolling (where height is > 0) only ensure fixed height, since at runtime scrolling
        // will only honor fixed height currently.
        if min_size.h == max_size.h {
            size.h = ensure_min_max_size(size.h, min_size.h, max_size.h);
        } else if size.h > 0 {
            // Also always honor min height, scrolling always does.
            size.h = max(size.h, min_size.h);
        }

        size
    }

    pub fn configure_new_window(
        &self,
        window: &Window,
        width: Option<PresetSize>,
        height: Option<PresetSize>,
        is_floating: bool,
        rules: &ResolvedWindowRules,
    ) {
        window.with_surfaces(|surface, data| {
            send_scale_transform(surface, data, self.scale, self.transform);
        });

        let toplevel = window.toplevel().expect("no x11 support");
        let (min_size, max_size) = with_states(toplevel.wl_surface(), |state| {
            let mut guard = state.cached_state.get::<SurfaceCachedState>();
            let current = guard.current();
            (current.min_size, current.max_size)
        });
        toplevel.with_pending_state(|state| {
            if state.states.contains(xdg_toplevel::State::Fullscreen) {
                state.size = Some(self.view_size.to_i32_round());
            } else if state.states.contains(xdg_toplevel::State::Maximized) {
                state.size = Some(self.working_area.size.to_i32_round());
            } else {
                let size =
                    self.new_window_size(width, height, is_floating, rules, (min_size, max_size));
                state.size = Some(size);
            }

            if is_floating {
                state.bounds = Some(self.floating.new_window_toplevel_bounds(rules));
            } else {
                state.bounds = Some(self.scrolling.new_window_toplevel_bounds(rules));
            }
        });
    }

    pub(super) fn resolve_scrolling_width(
        &self,
        window: &W,
        width: Option<PresetSize>,
    ) -> ColumnWidth {
        let width = width.unwrap_or_else(|| PresetSize::Fixed(window.size().w));
        match width {
            PresetSize::Fixed(fixed) => {
                let mut fixed = f64::from(fixed);

                // Add border width since ColumnWidth includes borders.
                let rules = window.rules();
                let border = self.options.layout.border.merged_with(&rules.border);
                if !border.off {
                    fixed += border.width * 2.;
                }

                ColumnWidth::Fixed(fixed)
            }
            PresetSize::Proportion(prop) => ColumnWidth::Proportion(prop),
        }
    }

    pub fn focus_left(&mut self) -> bool {
        self.with_layout_transition(|this| {
            if this.active_layer().is_floating() {
                this.floating.focus_left()
            } else if this.tiling_mode() == realand_ipc::TilingMode::Dwindle {
                this.dwindle.focus_left()
            } else {
                this.scrolling.focus_left()
            }
        })
    }

    pub fn focus_right(&mut self) -> bool {
        self.with_layout_transition(|this| {
            if this.active_layer().is_floating() {
                this.floating.focus_right()
            } else if this.tiling_mode() == realand_ipc::TilingMode::Dwindle {
                this.dwindle.focus_right()
            } else {
                this.scrolling.focus_right()
            }
        })
    }

    pub fn focus_column_first(&mut self) {
        self.with_layout_transition(|this| {
            if this.active_layer().is_floating() {
                this.floating.focus_leftmost();
            } else if this.tiling_mode() == realand_ipc::TilingMode::Dwindle {
                this.dwindle.focus_leftmost();
            } else {
                this.scrolling.focus_column_first();
            }
        })
    }

    pub fn focus_column_last(&mut self) {
        self.with_layout_transition(|this| {
            if this.active_layer().is_floating() {
                this.floating.focus_rightmost();
            } else if this.tiling_mode() == realand_ipc::TilingMode::Dwindle {
                this.dwindle.focus_rightmost();
            } else {
                this.scrolling.focus_column_last();
            }
        })
    }

    pub fn focus_column_right_or_first(&mut self) {
        self.with_layout_transition(|this| {
            if !this.focus_right() {
                this.focus_column_first();
            }
        })
    }

    pub fn focus_column_left_or_last(&mut self) {
        self.with_layout_transition(|this| {
            if !this.focus_left() {
                this.focus_column_last();
            }
        })
    }

    pub fn focus_column(&mut self, index: usize) {
        self.with_layout_transition(|this| {
            if this.active_layer().is_floating() {
                this.focus_tiling();
            }
            this.scrolling.focus_column(index);
        })
    }

    pub fn focus_window_in_column(&mut self, index: u8) {
        self.with_layout_transition(|this| {
            if this.active_layer().is_floating() {
                return;
            }
            this.scrolling.focus_window_in_column(index);
        })
    }

    pub fn focus_down(&mut self) -> bool {
        self.with_layout_transition(|this| {
            if this.active_layer().is_floating() {
                this.floating.focus_down()
            } else if this.tiling_mode() == realand_ipc::TilingMode::Dwindle {
                this.dwindle.focus_down()
            } else {
                this.scrolling.focus_down()
            }
        })
    }

    pub fn focus_up(&mut self) -> bool {
        self.with_layout_transition(|this| {
            if this.active_layer().is_floating() {
                this.floating.focus_up()
            } else if this.tiling_mode() == realand_ipc::TilingMode::Dwindle {
                this.dwindle.focus_up()
            } else {
                this.scrolling.focus_up()
            }
        })
    }

    pub fn focus_down_or_left(&mut self) {
        self.with_layout_transition(|this| {
            if this.active_layer().is_floating() {
                this.floating.focus_down();
            } else if this.tiling_mode() == realand_ipc::TilingMode::Dwindle {
                if !this.dwindle.focus_down() {
                    this.dwindle.focus_left();
                }
            } else {
                this.scrolling.focus_down_or_left();
            }
        })
    }

    pub fn focus_down_or_right(&mut self) {
        self.with_layout_transition(|this| {
            if this.active_layer().is_floating() {
                this.floating.focus_down();
            } else if this.tiling_mode() == realand_ipc::TilingMode::Dwindle {
                if !this.dwindle.focus_down() {
                    this.dwindle.focus_right();
                }
            } else {
                this.scrolling.focus_down_or_right();
            }
        })
    }

    pub fn focus_up_or_left(&mut self) {
        self.with_layout_transition(|this| {
            if this.active_layer().is_floating() {
                this.floating.focus_up();
            } else if this.tiling_mode() == realand_ipc::TilingMode::Dwindle {
                if !this.dwindle.focus_up() {
                    this.dwindle.focus_left();
                }
            } else {
                this.scrolling.focus_up_or_left();
            }
        })
    }

    pub fn focus_up_or_right(&mut self) {
        self.with_layout_transition(|this| {
            if this.active_layer().is_floating() {
                this.floating.focus_up();
            } else if this.tiling_mode() == realand_ipc::TilingMode::Dwindle {
                if !this.dwindle.focus_up() {
                    this.dwindle.focus_right();
                }
            } else {
                this.scrolling.focus_up_or_right();
            }
        })
    }

    pub fn focus_window_top(&mut self) {
        self.with_layout_transition(|this| {
            if this.active_layer().is_floating() {
                this.floating.focus_topmost();
            } else if this.tiling_mode() == realand_ipc::TilingMode::Dwindle {
                this.dwindle.focus_top();
            } else {
                this.scrolling.focus_top();
            }
        })
    }

    pub fn focus_window_bottom(&mut self) {
        self.with_layout_transition(|this| {
            if this.active_layer().is_floating() {
                this.floating.focus_bottommost();
            } else if this.tiling_mode() == realand_ipc::TilingMode::Dwindle {
                this.dwindle.focus_bottom();
            } else {
                this.scrolling.focus_bottom();
            }
        })
    }

    pub fn focus_window_down_or_top(&mut self) {
        self.with_layout_transition(|this| {
            if !this.focus_down() {
                this.focus_window_top();
            }
        })
    }

    pub fn focus_window_up_or_bottom(&mut self) {
        self.with_layout_transition(|this| {
            if !this.focus_up() {
                this.focus_window_bottom();
            }
        })
    }

    pub fn move_left(&mut self) -> bool {
        self.with_layout_transition(|this| {
            if this.active_layer().is_floating() {
                this.floating.move_left();
                true
            } else if this.tiling_mode() == realand_ipc::TilingMode::Dwindle {
                this.dwindle.move_left()
            } else {
                this.scrolling.move_left()
            }
        })
    }

    pub fn move_right(&mut self) -> bool {
        self.with_layout_transition(|this| {
            if this.active_layer().is_floating() {
                this.floating.move_right();
                true
            } else if this.tiling_mode() == realand_ipc::TilingMode::Dwindle {
                this.dwindle.move_right()
            } else {
                this.scrolling.move_right()
            }
        })
    }

    pub fn move_column_to_first(&mut self) {
        self.with_layout_transition(|this| {
            if this.active_layer().is_floating() {
                return;
            }
            this.scrolling.move_column_to_first();
        })
    }

    pub fn move_column_to_last(&mut self) {
        self.with_layout_transition(|this| {
            if this.active_layer().is_floating() {
                return;
            }
            this.scrolling.move_column_to_last();
        })
    }

    pub fn move_column_to_index(&mut self, index: usize) {
        self.with_layout_transition(|this| {
            if this.active_layer().is_floating() {
                return;
            }
            this.scrolling.move_column_to_index(index);
        })
    }

    pub fn move_down(&mut self) -> bool {
        self.with_layout_transition(|this| {
            if this.active_layer().is_floating() {
                this.floating.move_down();
                true
            } else if this.tiling_mode() == realand_ipc::TilingMode::Dwindle {
                this.dwindle.move_down()
            } else {
                this.scrolling.move_down()
            }
        })
    }

    pub fn move_up(&mut self) -> bool {
        self.with_layout_transition(|this| {
            if this.active_layer().is_floating() {
                this.floating.move_up();
                true
            } else if this.tiling_mode() == realand_ipc::TilingMode::Dwindle {
                this.dwindle.move_up()
            } else {
                this.scrolling.move_up()
            }
        })
    }

    pub fn consume_or_expel_window_left(&mut self, window: Option<&W::Id>) {
        self.with_layout_transition(|this| {
            if window.map_or(this.active_layer().is_floating(), |id| {
                this.floating.has_window(id)
            }) {
                return;
            }
            this.scrolling.consume_or_expel_window_left(window);
        })
    }

    pub fn consume_or_expel_window_right(&mut self, window: Option<&W::Id>) {
        self.with_layout_transition(|this| {
            if window.map_or(this.active_layer().is_floating(), |id| {
                this.floating.has_window(id)
            }) {
                return;
            }
            this.scrolling.consume_or_expel_window_right(window);
        })
    }

    pub fn consume_into_column(&mut self) {
        self.with_layout_transition(|this| {
            if this.active_layer().is_floating() {
                return;
            }
            this.scrolling.consume_into_column();
        })
    }

    pub fn expel_from_column(&mut self) {
        self.with_layout_transition(|this| {
            if this.active_layer().is_floating() {
                return;
            }
            this.scrolling.expel_from_column();
        })
    }

    pub fn swap_window_in_direction(&mut self, direction: ScrollDirection) {
        self.with_layout_transition(|this| {
            if this.active_layer().is_floating() {
                return;
            }
            if this.tiling_mode() == realand_ipc::TilingMode::Dwindle {
                match direction {
                    ScrollDirection::Left => {
                        this.dwindle.move_left();
                    }
                    ScrollDirection::Right => {
                        this.dwindle.move_right();
                    }
                }
                return;
            }
            this.scrolling.swap_window_in_direction(direction);
        })
    }

    pub fn toggle_column_tabbed_display(&mut self) {
        self.with_layout_transition(|this| {
            if this.active_layer().is_floating() {
                return;
            }
            this.scrolling.toggle_column_tabbed_display();
        })
    }

    pub fn toggle_column_dwindle_display(&mut self) {
        self.with_layout_transition(|this| {
            if this.active_layer().is_floating() {
                return;
            }
            this.scrolling.toggle_column_dwindle_display();
        })
    }

    pub fn set_column_display(&mut self, display: ColumnDisplay) {
        self.with_layout_transition(|this| {
            if this.active_layer().is_floating() {
                return;
            }
            this.scrolling.set_column_display(display);
        })
    }

    pub fn center_column(&mut self) {
        self.with_layout_transition(|this| {
            if this.active_layer().is_floating() {
                this.floating.center_window(None);
            } else {
                this.scrolling.center_column();
            }
        })
    }

    pub fn center_window(&mut self, id: Option<&W::Id>) {
        self.with_layout_transition(|this| {
            if id.map_or(this.active_layer().is_floating(), |id| {
                this.floating.has_window(id)
            }) {
                this.floating.center_window(id);
            } else if id.map_or(
                this.tiling_mode() == realand_ipc::TilingMode::Scrolling,
                |id| !this.dwindle.has_window(id),
            ) {
                this.scrolling.center_window(id);
            }
        })
    }

    pub fn center_visible_columns(&mut self) {
        self.with_layout_transition(|this| {
            if this.active_layer().is_floating() {
                return;
            }
            this.scrolling.center_visible_columns();
        })
    }

    pub fn toggle_width(&mut self, forwards: bool) {
        self.with_layout_transition(|this| {
            if this.active_layer().is_floating() {
                this.floating.toggle_window_width(None, forwards);
            } else if this.tiling_mode() == realand_ipc::TilingMode::Scrolling {
                this.scrolling.toggle_width(forwards);
            }
        })
    }

    pub fn toggle_full_width(&mut self) {
        self.with_layout_transition(|this| {
            if this.active_layer().is_floating()
                || this.tiling_mode() != realand_ipc::TilingMode::Scrolling
            {
                // Leave this unimplemented for now. For good UX, this probably needs moving the tile
                // to be against the left edge of the working area while it is full-width.
                return;
            }
            this.scrolling.toggle_full_width();
        })
    }

    pub fn set_column_width(&mut self, change: SizeChange) {
        self.with_layout_transition(|this| {
            if this.active_layer().is_floating() {
                this.floating.set_window_width(None, change, true);
            } else if this.tiling_mode() == realand_ipc::TilingMode::Dwindle {
                this.dwindle.set_window_width(None, change);
            } else {
                this.scrolling.set_window_width(None, change);
            }
        })
    }

    pub fn set_window_width(&mut self, window: Option<&W::Id>, change: SizeChange) {
        self.with_layout_transition(|this| {
            if window.map_or(this.active_layer().is_floating(), |id| {
                this.floating.has_window(id)
            }) {
                this.floating.set_window_width(window, change, true);
            } else if window.map_or(
                this.tiling_mode() == realand_ipc::TilingMode::Dwindle,
                |id| this.dwindle.has_window(id),
            ) {
                this.dwindle.set_window_width(window, change);
            } else {
                this.scrolling.set_window_width(window, change);
            }
        })
    }

    pub fn set_window_height(&mut self, window: Option<&W::Id>, change: SizeChange) {
        self.with_layout_transition(|this| {
            if window.map_or(this.active_layer().is_floating(), |id| {
                this.floating.has_window(id)
            }) {
                this.floating.set_window_height(window, change, true);
            } else if window.map_or(
                this.tiling_mode() == realand_ipc::TilingMode::Dwindle,
                |id| this.dwindle.has_window(id),
            ) {
                this.dwindle.set_window_height(window, change);
            } else {
                this.scrolling.set_window_height(window, change);
            }
        })
    }

    pub fn reset_window_height(&mut self, window: Option<&W::Id>) {
        self.with_layout_transition(|this| {
            let is_floating = window.map_or(this.active_layer().is_floating(), |id| {
                this.floating.has_window(id)
            });
            if !is_floating
                && window.map_or(
                    this.tiling_mode() == realand_ipc::TilingMode::Scrolling,
                    |id| !this.dwindle.has_window(id),
                )
            {
                this.scrolling.reset_window_height(window);
            }
        })
    }

    pub fn toggle_window_width(&mut self, window: Option<&W::Id>, forwards: bool) {
        self.with_layout_transition(|this| {
            if window.map_or(this.active_layer().is_floating(), |id| {
                this.floating.has_window(id)
            }) {
                this.floating.toggle_window_width(window, forwards);
            } else if window.map_or(
                this.tiling_mode() == realand_ipc::TilingMode::Scrolling,
                |id| !this.dwindle.has_window(id),
            ) {
                this.scrolling.toggle_window_width(window, forwards);
            }
        })
    }

    pub fn toggle_window_height(&mut self, window: Option<&W::Id>, forwards: bool) {
        self.with_layout_transition(|this| {
            if window.map_or(this.active_layer().is_floating(), |id| {
                this.floating.has_window(id)
            }) {
                this.floating.toggle_window_height(window, forwards);
            } else if window.map_or(
                this.tiling_mode() == realand_ipc::TilingMode::Scrolling,
                |id| !this.dwindle.has_window(id),
            ) {
                this.scrolling.toggle_window_height(window, forwards);
            }
        })
    }

    pub fn expand_column_to_available_width(&mut self) {
        self.with_layout_transition(|this| {
            if this.active_layer().is_floating()
                || this.tiling_mode() != realand_ipc::TilingMode::Scrolling
            {
                return;
            }
            this.scrolling.expand_column_to_available_width();
        })
    }

    pub fn set_fullscreen(&mut self, window: &W::Id, is_fullscreen: bool) {
        self.with_layout_transition(|this| {
            let mut restore_to_floating = false;
            if this.floating.has_window(window) {
                if is_fullscreen {
                    restore_to_floating = true;
                    this.toggle_window_floating(Some(window));
                } else {
                    // Floating windows are never fullscreen, so this is an unfullscreen request for an
                    // already unfullscreen window.
                    return;
                }
            }
            if this.dwindle.has_window(window) {
                if !is_fullscreen {
                    if let Some(tile) = this
                        .dwindle
                        .tiles()
                        .find(|tile| tile.window().id() == window)
                    {
                        if tile.restore_to_floating {
                            this.toggle_window_floating(Some(window));
                            return;
                        }
                    }
                }

                let was_normal = this
                    .dwindle
                    .tiles()
                    .find(|tile| tile.window().id() == window)
                    .is_some_and(|tile| tile.window().pending_sizing_mode().is_normal());

                this.dwindle.set_fullscreen(window, is_fullscreen);

                if was_normal {
                    if let Some(tile) = this
                        .dwindle
                        .tiles_mut()
                        .find(|tile| tile.window().id() == window)
                    {
                        tile.restore_to_floating = restore_to_floating;
                    }
                }
                return;
            } else if !is_fullscreen {
                // The window is in the scrolling layout and we're requesting an unfullscreen. If it is
                // indeed fullscreen (i.e. this isn't a duplicate unfullscreen request), then we may
                // need to unfullscreen into floating.
                let col = this.scrolling.columns().find(|col| col.contains(window));

                let Some(col) = col else {
                    return;
                };

                // When going from fullscreen to maximized, don't consider restore_to_floating yet.
                if col.is_pending_fullscreen() && !col.is_pending_maximized() {
                    let (tile, _) = col
                        .tiles()
                        .find(|(tile, _)| tile.window().id() == window)
                        .unwrap();
                    if tile.restore_to_floating {
                        // Unfullscreen and float in one call so it has a chance to notice and request a
                        // (0, 0) size, rather than the scrolling column size.
                        this.toggle_window_floating(Some(window));
                        return;
                    }
                }
            }

            let Some(tile) = this
                .scrolling
                .tiles()
                .find(|tile| tile.window().id() == window)
            else {
                return;
            };
            let was_normal = tile.window().pending_sizing_mode().is_normal();

            this.scrolling.set_fullscreen(window, is_fullscreen);

            // When going from normal to fullscreen, remember if we should unfullscreen to floating.
            if let Some(tile) = this
                .scrolling
                .tiles_mut()
                .find(|tile| tile.window().id() == window)
            {
                if was_normal && !tile.window().pending_sizing_mode().is_normal() {
                    tile.restore_to_floating = restore_to_floating;
                }
            }
        })
    }

    pub fn toggle_fullscreen(&mut self, window: &W::Id) {
        let Some(tile) = self.tiles().find(|tile| tile.window().id() == window) else {
            return;
        };
        let current = tile.window().pending_sizing_mode().is_fullscreen();
        self.set_fullscreen(window, !current);
    }

    pub fn set_maximized(&mut self, window: &W::Id, maximize: bool) {
        self.with_layout_transition(|this| {
            let mut restore_to_floating = false;
            if this.floating.has_window(window) {
                if maximize {
                    restore_to_floating = true;
                    this.toggle_window_floating(Some(window));
                } else {
                    // Floating windows are never maximized, so this is an unmaximize request for an
                    // already unmaximized window.
                    return;
                }
            }
            if this.dwindle.has_window(window) {
                if !maximize {
                    if let Some(tile) = this
                        .dwindle
                        .tiles()
                        .find(|tile| tile.window().id() == window)
                    {
                        if tile.window().pending_sizing_mode().is_maximized()
                            && tile.restore_to_floating
                        {
                            this.toggle_window_floating(Some(window));
                            return;
                        }
                    }
                }

                let was_normal = this
                    .dwindle
                    .tiles()
                    .find(|tile| tile.window().id() == window)
                    .is_some_and(|tile| tile.window().pending_sizing_mode().is_normal());

                this.dwindle.set_maximized(window, maximize);

                if was_normal {
                    if let Some(tile) = this
                        .dwindle
                        .tiles_mut()
                        .find(|tile| tile.window().id() == window)
                    {
                        tile.restore_to_floating = restore_to_floating;
                    }
                }
                return;
            } else if !maximize {
                // The window is in the scrolling layout and we're requesting to unmaximize. If it is
                // indeed maximized (i.e. this isn't a duplicate unmaximize request), then we may
                // need to unmaximize into floating.
                let tile = this
                    .scrolling
                    .tiles()
                    .find(|tile| tile.window().id() == window);
                let Some(tile) = tile else {
                    return;
                };
                // The tile cannot unmaximize into fullscreen (pending_sizing_mode() will be fullscreen
                // in that case and not maximized), so this check works.
                if tile.window().pending_sizing_mode().is_maximized() && tile.restore_to_floating {
                    // Unmaximize and float in one call so it has a chance to notice and request a
                    // (0, 0) size, rather than the scrolling column size.
                    this.toggle_window_floating(Some(window));
                    return;
                }
            }

            let Some(tile) = this
                .scrolling
                .tiles()
                .find(|tile| tile.window().id() == window)
            else {
                return;
            };
            let was_normal = tile.window().pending_sizing_mode().is_normal();

            this.scrolling.set_maximized(window, maximize);

            // When going from normal to maximized, remember if we should unmaximize to floating.
            if let Some(tile) = this
                .scrolling
                .tiles_mut()
                .find(|tile| tile.window().id() == window)
            {
                if was_normal && !tile.window().pending_sizing_mode().is_normal() {
                    tile.restore_to_floating = restore_to_floating;
                }
            }
        })
    }

    pub fn toggle_maximized(&mut self, window: &W::Id) {
        let mut current = false;

        // We have to check the column property in case the window is in the scrolling layout and
        // both maximized and fullscreen. In this case, only the column knows whether it's
        // maximized.
        //
        // In the floating layout, windows cannot be maximized.
        if let Some(col) = self.scrolling.columns().find(|col| col.contains(window)) {
            current = col.is_pending_maximized();
        } else if let Some(tile) = self
            .dwindle
            .tiles()
            .find(|tile| tile.window().id() == window)
        {
            current = tile.window().pending_sizing_mode().is_maximized();
        }

        self.set_maximized(window, !current);
    }

    pub fn canvas_enabled(&self) -> bool {
        self.core
            .borrow()
            .space(SpaceId(self.id.get()))
            .is_some_and(|space| space.interaction.canvas_enabled())
    }

    pub fn toggle_canvas(&mut self) {
        self.with_layout_transition(|this| {
            if this.canvas_enabled() {
                let transition = this
                    .core
                    .borrow_mut()
                    .set_canvas_mode(SpaceId(this.id.get()), false);
                this.floating.set_canvas_enabled(false);
                let active = this.active_window().map(LayoutElement::core_id);
                for core_id in transition.restore_tiled {
                    let id = this
                        .windows()
                        .find(|window| window.core_id() == core_id)
                        .map(|window| window.id().clone());
                    if let Some(id) = id {
                        this.set_window_floating(Some(&id), false);
                    }
                }
                if let Some(policy) = transition.restore_policy {
                    match (policy, this.tiling_mode()) {
                        (
                            CanvasPolicyMemory::Scrolling(saved),
                            realand_ipc::TilingMode::Scrolling,
                        ) => {
                            let current =
                                CanvasPolicyMemory::Scrolling(this.scrolling.policy_snapshot());
                            let CanvasPolicyMemory::Scrolling(saved) =
                                CanvasPolicyMemory::Scrolling(saved).preserve_runtime(&current)
                            else {
                                unreachable!()
                            };
                            let tiles = this.scrolling.drain_tiles();
                            this.scrolling.restore_policy(&saved, tiles, active);
                        }
                        (CanvasPolicyMemory::Dwindle(saved), realand_ipc::TilingMode::Dwindle) => {
                            this.dwindle.restore_stage_topology(&saved);
                            if let Some(id) = active.and_then(|id| {
                                this.dwindle
                                    .tiles()
                                    .find(|tile| tile.window().core_id() == id)
                                    .map(|tile| tile.window().id().clone())
                            }) {
                                this.dwindle.set_active_window(id);
                            }
                        }
                        _ => {}
                    }
                }
            } else {
                // Fullscreen/maximized windows retain their normal compositor behavior.
                if this.is_active_pending_fullscreen() {
                    return;
                }
                let mut tiled = Vec::new();
                for (tile, pos, _) in this.scrolling.tiles_with_render_positions() {
                    if tile.window().pending_sizing_mode().is_normal() {
                        tiled.push((tile.window().id().clone(), pos));
                    }
                }
                tiled.extend(this.dwindle.tiled_positions());
                let policy = match this.tiling_mode() {
                    realand_ipc::TilingMode::Scrolling => {
                        CanvasPolicyMemory::Scrolling(this.scrolling.policy_snapshot())
                    }
                    realand_ipc::TilingMode::Dwindle => {
                        CanvasPolicyMemory::Dwindle(this.dwindle.policy_snapshot())
                    }
                };
                this.core
                    .borrow_mut()
                    .capture_canvas_policy(SpaceId(this.id.get()), policy);
                this.core
                    .borrow_mut()
                    .set_canvas_mode(SpaceId(this.id.get()), true);
                this.floating.set_canvas_enabled(true);
                for (id, pos) in &tiled {
                    this.set_window_floating(Some(id), true);
                    this.floating.move_window(
                        Some(id),
                        PositionChange::SetFixed(pos.x - this.working_area.loc.x),
                        PositionChange::SetFixed(pos.y - this.working_area.loc.y),
                        false,
                    );
                    if let Some(core_id) = this.core.id(id) {
                        this.core.borrow_mut().set_canvas_origin(core_id, true);
                    }
                }
            }
        })
    }

    pub fn pan_canvas(&mut self, delta: Point<f64, Logical>, kinetic: bool) {
        self.with_layout_transition(|this| {
            if !this.is_active_pending_fullscreen() {
                this.floating.pan_canvas(delta, kinetic);
            }
        });
    }

    pub fn stop_canvas_pan(&mut self) {
        self.floating.stop_canvas_pan();
    }

    pub fn reset_canvas(&mut self) {
        self.with_layout_transition(|this| this.floating.reset_canvas());
    }

    pub fn toggle_window_floating(&mut self, id: Option<&W::Id>) {
        self.with_layout_transition(|this| {
            (|| {
                let active_id = this.active_window().map(|win| win.id().clone());
                let target_is_active = id.is_none_or(|id| Some(id) == active_id.as_ref());
                let Some(id) = id.cloned().or(active_id) else {
                    return;
                };

                let Some((_, render_pos, _)) = this
                    .tiles_with_render_positions()
                    .find(|(tile, _, _)| *tile.window().id() == id)
                else {
                    return;
                };

                if this.floating.has_window(&id) {
                    let removed = this.floating.remove_tile(&id);
                    // Update the authoritative destination before selecting
                    // its lane; validation must not see the old floating row.
                    this.register_core_tile(&removed.tile, false);
                    if this.tiling_mode() == realand_ipc::TilingMode::Dwindle {
                        this.dwindle.add_tile(
                            removed.tile,
                            if target_is_active {
                                ActivateWindow::Yes
                            } else {
                                ActivateWindow::No
                            },
                        );
                    } else {
                        this.scrolling.add_tile(
                            None,
                            removed.tile,
                            target_is_active,
                            removed.width,
                            removed.is_full_width,
                            None,
                        );
                    }
                    if target_is_active {
                        this.set_active_layer(SpaceLayerFocus::Tiling);
                    }
                } else {
                    let mut removed = if this.dwindle.has_window(&id) {
                        this.dwindle.remove_window(&id).unwrap()
                    } else {
                        this.scrolling.remove_tile(&id, Transaction::new())
                    };
                    removed.tile.stop_move_animations();

                    // Come up with a default floating position close to the tile position.
                    let stored_or_default = this.floating.stored_or_default_tile_pos(&removed.tile);
                    if stored_or_default.is_none() {
                        let offset = if this.options.layout.center_focused_column
                            == CenterFocusedColumn::Always
                        {
                            Point::from((0., 0.))
                        } else {
                            Point::from((50., 50.))
                        };
                        let pos = render_pos + offset;
                        let size = removed.tile.tile_size();
                        let pos = this.floating.clamp_within_working_area(pos, size);
                        let pos = this.floating.logical_to_size_frac(pos);
                        removed.tile.floating_pos = Some(pos);
                    }

                    this.register_core_tile(&removed.tile, true);
                    this.floating.add_tile(removed.tile, target_is_active);
                    if target_is_active {
                        this.set_active_layer(SpaceLayerFocus::Floating);
                    }
                }

                if let Some((tile, new_render_pos)) = this
                    .tiles_with_render_positions_mut(false)
                    .find(|(tile, _)| *tile.window().id() == id)
                {
                    tile.animate_move_from(render_pos - new_render_pos);
                }
            })();
        })
    }

    pub fn set_window_floating(&mut self, id: Option<&W::Id>, floating: bool) {
        if id.map_or(self.active_layer().is_floating(), |id| {
            self.floating.has_window(id)
        }) == floating
        {
            return;
        }

        self.toggle_window_floating(id);
    }

    pub fn focus_floating(&mut self) {
        self.with_layout_transition(|this| {
            if !this.active_layer().is_floating() {
                this.switch_focus_floating_tiling();
            }
        })
    }

    pub fn focus_tiling(&mut self) {
        self.with_layout_transition(|this| {
            if this.active_layer().is_floating() {
                this.switch_focus_floating_tiling();
            }
        })
    }

    pub fn switch_focus_floating_tiling(&mut self) {
        self.with_layout_transition(|this| {
            if this.floating.is_empty() || this.tiling_is_empty() {
                return;
            }

            let focus = if this.active_layer().is_floating() {
                SpaceLayerFocus::Tiling
            } else {
                SpaceLayerFocus::Floating
            };
            this.set_active_layer(focus);
        })
    }

    pub fn move_floating_window(
        &mut self,
        id: Option<&W::Id>,
        x: PositionChange,
        y: PositionChange,
        animate: bool,
    ) {
        self.with_layout_transition(|this| {
            if id.map_or(this.active_layer().is_floating(), |id| {
                this.floating.has_window(id)
            }) {
                this.floating.move_window(id, x, y, animate);
            } else {
                // If the target tile isn't floating, set its stored floating position.
                let tile = if let Some(id) = id {
                    if let Some(tile) = this
                        .dwindle
                        .tiles_mut()
                        .find(|tile| tile.window().id() == id)
                    {
                        tile
                    } else if let Some(tile) = this
                        .scrolling
                        .tiles_mut()
                        .find(|tile| tile.window().id() == id)
                    {
                        tile
                    } else {
                        return;
                    }
                } else if this.tiling_mode() == realand_ipc::TilingMode::Dwindle {
                    let Some(tile) = this.dwindle.active_tile_mut() else {
                        return;
                    };
                    tile
                } else if let Some(tile) = this.scrolling.active_tile_mut() {
                    tile
                } else {
                    return;
                };

                let pos = this.floating.stored_or_default_tile_pos(tile);

                // If there's no stored floating position, we can only set both components at once, not
                // adjust.
                let pos = pos.or_else(|| {
                    (matches!(
                        x,
                        PositionChange::SetFixed(_) | PositionChange::SetProportion(_)
                    ) && matches!(
                        y,
                        PositionChange::SetFixed(_) | PositionChange::SetProportion(_)
                    ))
                    .then_some(Point::default())
                });

                let Some(mut pos) = pos else {
                    return;
                };

                let working_area = this.floating.working_area();
                let available_width = working_area.size.w;
                let available_height = working_area.size.h;
                let working_area_loc = working_area.loc;

                const MAX_F: f64 = 10000.;

                match x {
                    PositionChange::SetFixed(x) => pos.x = x + working_area_loc.x,
                    PositionChange::SetProportion(prop) => {
                        let prop = (prop / 100.).clamp(0., MAX_F);
                        pos.x = available_width * prop + working_area_loc.x;
                    }
                    PositionChange::AdjustFixed(x) => pos.x += x,
                    PositionChange::AdjustProportion(prop) => {
                        let current_prop = (pos.x - working_area_loc.x) / available_width.max(1.);
                        let prop = (current_prop + prop / 100.).clamp(0., MAX_F);
                        pos.x = available_width * prop + working_area_loc.x;
                    }
                }
                match y {
                    PositionChange::SetFixed(y) => pos.y = y + working_area_loc.y,
                    PositionChange::SetProportion(prop) => {
                        let prop = (prop / 100.).clamp(0., MAX_F);
                        pos.y = available_height * prop + working_area_loc.y;
                    }
                    PositionChange::AdjustFixed(y) => pos.y += y,
                    PositionChange::AdjustProportion(prop) => {
                        let current_prop = (pos.y - working_area_loc.y) / available_height.max(1.);
                        let prop = (current_prop + prop / 100.).clamp(0., MAX_F);
                        pos.y = available_height * prop + working_area_loc.y;
                    }
                }

                let pos = this.floating.logical_to_size_frac(pos);
                tile.floating_pos = Some(pos);
            }
        })
    }

    pub fn has_windows(&self) -> bool {
        self.windows().next().is_some()
    }

    pub fn is_minimized(&self, window: &W::Id) -> bool {
        let Some(core_id) = self.core.id(window) else {
            return false;
        };
        self.core.borrow().window(&core_id).is_some_and(|record| {
            record.attached && record.space == SpaceId(self.id.get()) && record.minimized
        })
    }

    pub fn minimize_window(&mut self, window: &W::Id, visible: bool) -> bool {
        self.with_layout_transition(|this| {
            let Some(core_id) = this.core.id(window) else {
                return false;
            };
            if this.is_minimized(window) {
                return false;
            }
            let Some((source, actually_visible)) =
                this.tiles_with_render_positions()
                    .find_map(|(tile, pos, visible)| {
                        (tile.window().core_id() == core_id)
                            .then(|| (Rectangle::new(pos, tile.animated_tile_size()), visible))
                    })
            else {
                return false;
            };
            let record = this
                .core
                .borrow()
                .window(&core_id)
                .cloned()
                .expect("mapped payload needs a core identity");
            let scrolling = this.scrolling.policy_snapshot();
            let dwindle = this.dwindle.policy_snapshot();
            let restoration = MinimizeRestore::capture(
                &core_id,
                record.layer == Layer::Above,
                record.geometry.width,
                &scrolling.columns,
                dwindle.topology.as_ref(),
            );
            // Core accepts the command and chooses state before any client or
            // payload mutation. Its synchronous intent is the sole execution.
            let Some(intent) = this.core.borrow_mut().minimize_window(core_id, restoration) else {
                return false;
            };
            this.interactive_resize_end(Some(window));
            let mut removed = if record.layer == Layer::Above {
                this.floating.remove_tile(window)
            } else if this.tiling_mode() == realand_ipc::TilingMode::Dwindle {
                this.dwindle.remove_window(window).unwrap()
            } else {
                this.scrolling.remove_tile(window, Transaction::new())
            };
            this.update_focus_floating_tiling_after_removing(removed.is_floating);
            removed.tile.window_mut().set_minimized(intent.minimized);
            removed.tile.start_genie(
                source,
                this.output.as_ref(),
                intent.minimized,
                visible && actually_visible,
            );
            this.minimized.push(ParkedTile {
                tile: removed.tile,
                source,
            });
            true
        })
    }

    pub fn restore_window(&mut self, window: &W::Id, visible: bool) -> bool {
        self.with_layout_transition(|this| {
            let Some(core_id) = this.core.id(window) else {
                return false;
            };
            let Some(idx) = this
                .minimized
                .iter()
                .position(|p| p.tile.window().core_id() == core_id)
            else {
                return false;
            };
            let columns = this
                .scrolling
                .policy_snapshot()
                .columns
                .into_iter()
                .map(|column| RestoreColumn {
                    id: column.id,
                    windows: column.windows,
                })
                .collect();
            let Some(intent) = this.core.borrow_mut().restore_window(core_id, columns) else {
                return false;
            };
            let restore = intent
                .restore
                .expect("restore command must return a placement");
            let ParkedTile { mut tile, source } = this.minimized.remove(idx);
            tile.window_mut().set_minimized(intent.minimized);
            tile.start_genie(source, this.output.as_ref(), intent.minimized, visible);
            match restore.destination {
                RestoreDestination::Scrolling {
                    column,
                    column_index,
                    row_index,
                } => {
                    this.register_core_tile(&tile, false);
                    let target = column.and_then(|id| {
                        this.scrolling
                            .columns()
                            .enumerate()
                            .find(|(_, column)| column.id().get() == id)
                            .map(|(index, column)| (index, column.tiles().count()))
                    });
                    if let Some((column_idx, count)) = target {
                        this.scrolling.add_tile_to_column(
                            column_idx,
                            Some(row_index.min(count)),
                            tile,
                            false,
                        );
                    } else {
                        let column_idx = column_index.min(this.scrolling.columns().count());
                        this.scrolling.add_tile(
                            Some(column_idx),
                            tile,
                            false,
                            restore.width,
                            restore.full_width,
                            None,
                        );
                    }
                }
                RestoreDestination::Floating => this.add_tile(
                    tile,
                    WorkspaceAddWindowTarget::Auto,
                    ActivateWindow::No,
                    restore.width,
                    restore.full_width,
                    true,
                    None,
                ),
                RestoreDestination::Tiled { neighbor } => {
                    let neighbor = neighbor.and_then(|id| {
                        this.windows()
                            .find(|window| window.core_id() == id)
                            .map(|window| window.id().clone())
                    });
                    let target = neighbor
                        .as_ref()
                        .map(WorkspaceAddWindowTarget::NextTo)
                        .unwrap_or_default();
                    this.add_tile(
                        tile,
                        target,
                        ActivateWindow::No,
                        restore.width,
                        restore.full_width,
                        false,
                        None,
                    );
                }
            }
            if let Some(display) = restore.display {
                let display = match display {
                    realand_core::policy_memory::ColumnDisplay::Normal => ColumnDisplay::Normal,
                    realand_core::policy_memory::ColumnDisplay::Tabbed => ColumnDisplay::Tabbed,
                    realand_core::policy_memory::ColumnDisplay::Dwindle => ColumnDisplay::Dwindle,
                };
                this.scrolling.set_window_column_display(core_id, display);
            }
            true
        })
    }

    pub fn has_window(&self, window: &W::Id) -> bool {
        let Some(core_id) = self.core.id(window) else {
            return false;
        };
        self.core
            .borrow()
            .window(&core_id)
            .is_some_and(|record| record.attached && record.space == SpaceId(self.id.get()))
    }

    /// Whether a closing-window snapshot can be visible. Hidden tabs and dwindle panes do not
    /// animate when closing; capturing them would waste GPU work and show a hidden window.
    pub fn should_capture_close_snapshot(&self, window: &W::Id) -> Option<bool> {
        if self.is_minimized(window) {
            return Some(false);
        }
        if self.floating.has_window(window) {
            return Some(true);
        }
        if let Some(visible) = self.dwindle.should_capture_close_snapshot(window) {
            return Some(visible);
        }

        self.scrolling.should_capture_close_snapshot(window)
    }

    pub fn find_wl_surface(&self, wl_surface: &WlSurface) -> Option<&W> {
        self.windows().find(|win| win.is_wl_surface(wl_surface))
    }

    pub fn find_wl_surface_mut(&mut self, wl_surface: &WlSurface) -> Option<&mut W> {
        self.windows_mut().find(|win| win.is_wl_surface(wl_surface))
    }

    pub fn tiles_with_render_positions(
        &self,
    ) -> impl Iterator<Item = (&Tile<W>, Point<f64, Logical>, bool)> {
        let scrolling = self.scrolling.tiles_with_render_positions();

        let dwindle = self.dwindle.tiles_with_render_positions();

        let floating = self.floating.tiles_with_render_positions();
        let visible = self.is_floating_visible();
        let floating = floating.map(move |(tile, pos)| (tile, pos, visible));

        floating.chain(scrolling).chain(dwindle)
    }

    pub fn tiles_with_render_positions_mut(
        &mut self,
        round: bool,
    ) -> impl Iterator<Item = (&mut Tile<W>, Point<f64, Logical>)> {
        let scrolling = self.scrolling.tiles_with_render_positions_mut(round);
        let dwindle = self
            .dwindle
            .tiles_with_render_positions_mut(round)
            .map(|(tile, pos, _)| (tile, pos));
        let floating = self.floating.tiles_with_render_positions_mut(round);
        floating.chain(scrolling).chain(dwindle)
    }

    pub fn tiles_with_ipc_layouts(&self) -> impl Iterator<Item = (&Tile<W>, WindowLayout)> {
        let scrolling = self.scrolling.tiles_with_ipc_layouts();
        let dwindle = self.dwindle.tiles_with_ipc_layouts();
        let floating = self.floating.tiles_with_ipc_layouts();
        floating.chain(scrolling).chain(dwindle).chain(
            self.minimized
                .iter()
                .map(|p| (&p.tile, p.tile.ipc_layout_template())),
        )
    }

    pub fn active_window_visual_rectangle(&self) -> Option<Rectangle<f64, Logical>> {
        if self.active_layer().is_floating() {
            self.floating.active_window_visual_rectangle()
        } else if self.tiling_mode() == realand_ipc::TilingMode::Dwindle {
            self.dwindle.active_window_visual_rectangle()
        } else {
            self.scrolling.active_window_visual_rectangle()
        }
    }

    pub fn popup_target_rect(&self, window: &W::Id) -> Option<Rectangle<f64, Logical>> {
        if let Some(parked) = self
            .minimized
            .iter()
            .find(|p| p.tile.window().id() == window)
        {
            return Some(Rectangle::new(
                parked.source.loc.upscale(-1.) - parked.tile.window_loc(),
                self.view_size,
            ));
        }
        if self.floating.has_window(window) {
            self.floating.popup_target_rect(window)
        } else if self.dwindle.has_window(window) {
            self.dwindle.popup_target_rect(window)
        } else {
            self.scrolling.popup_target_rect(window)
        }
    }

    pub fn render_scrolling<R: RealandRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        xray_pos: XrayPos,
        focus_ring: bool,
        layer: RenderLayer,
        push: &mut dyn FnMut(WorkspaceRenderElement<R>),
    ) {
        if layer.is_normal() {
            for parked in &self.minimized {
                parked.tile.render_genie(
                    ctx.r(),
                    xray_pos.offset(parked.source.loc),
                    &mut |elem| push(elem.into()),
                );
            }
        }
        let tiling_focus_ring = focus_ring && !self.floating_is_active();
        if self.tiling_mode() == realand_ipc::TilingMode::Dwindle {
            let view_rect = Rectangle::from_size(self.view_size);
            self.dwindle.render(
                ctx,
                xray_pos,
                view_rect,
                tiling_focus_ring,
                layer,
                &mut |elem| push(elem.into()),
            );
        } else {
            self.scrolling
                .render(ctx, xray_pos, tiling_focus_ring, layer, &mut |elem| {
                    push(elem.into())
                });
        }
    }

    pub fn render_floating<R: RealandRenderer>(
        &self,
        ctx: RenderCtx<R>,
        xray_pos: XrayPos,
        focus_ring: bool,
        layer: RenderLayer,
        push: &mut dyn FnMut(WorkspaceRenderElement<R>),
    ) {
        if !self.is_floating_visible() && layer.is_normal() {
            return;
        }

        let view_rect = Rectangle::from_size(self.view_size);
        let floating_focus_ring = focus_ring && self.floating_is_active();
        self.floating.render(
            ctx,
            xray_pos,
            view_rect,
            floating_focus_ring,
            layer,
            &mut |elem| push(elem.into()),
        );
    }

    pub fn render_shadow<R: RealandRenderer>(
        &self,
        renderer: &mut R,
        push: &mut dyn FnMut(ShadowRenderElement),
    ) {
        self.shadow.render(renderer, Point::from((0., 0.)), push);
    }

    pub fn render_background(&self) -> SolidColorRenderElement {
        SolidColorRenderElement::from_buffer(
            &self.background_buffer,
            Point::new(0., 0.),
            1.,
            Kind::Unspecified,
        )
    }

    pub fn render_above_top_layer(&self) -> bool {
        if self.tiling_mode() == realand_ipc::TilingMode::Dwindle {
            self.dwindle.is_active_pending_fullscreen()
        } else {
            self.scrolling.render_above_top_layer()
        }
    }

    pub fn is_floating_visible(&self) -> bool {
        // If the focus is on a fullscreen scrolling window, hide the floating windows.
        matches!(
            self.active_layer(),
            SpaceLayerFocus::Floating | SpaceLayerFocus::TilingWithFloatingRaised
        ) || !self.render_above_top_layer()
    }

    pub fn store_unmap_snapshot_if_empty(
        &mut self,
        renderer: &mut GlesRenderer,
        xray: Option<&mut Xray>,
        xray_has_blocked_out_layers: bool,
        xray_pos: XrayPos,
        window: &W::Id,
    ) {
        let view_size = self.view_size();
        for (tile, tile_pos) in self.tiles_with_render_positions_mut(false) {
            if tile.window().id() == window {
                let view_pos = Point::from((-tile_pos.x, -tile_pos.y));
                let view_rect = Rectangle::new(view_pos, view_size);
                tile.update_render_elements(false, view_rect);
                let xray_pos = xray_pos.offset(tile_pos);
                tile.store_unmap_snapshot_if_empty(
                    renderer,
                    xray,
                    xray_has_blocked_out_layers,
                    xray_pos,
                );
                return;
            }
        }
    }

    pub fn store_unmap_snapshot_native_if_empty(
        &mut self,
        renderer: &mut crate::render_helpers::vulkan::native::NativeRenderer,
        xray: Option<&mut Xray>,
        xray_has_blocked_out_layers: bool,
        xray_pos: XrayPos,
        window: &W::Id,
    ) {
        let view_size = self.view_size();
        for (tile, tile_pos) in self.tiles_with_render_positions_mut(false) {
            if tile.window().id() == window {
                let view_pos = Point::from((-tile_pos.x, -tile_pos.y));
                let view_rect = Rectangle::new(view_pos, view_size);
                tile.update_render_elements(false, view_rect);
                let xray_pos = xray_pos.offset(tile_pos);
                tile.store_unmap_snapshot_native_if_empty(
                    renderer,
                    xray,
                    xray_has_blocked_out_layers,
                    xray_pos,
                );
                return;
            }
        }
    }

    pub fn clear_unmap_snapshot(&mut self, window: &W::Id) {
        for tile in self.tiles_mut() {
            if tile.window().id() == window {
                let _ = tile.take_unmap_snapshot();
                return;
            }
        }
    }

    pub fn start_close_animation_for_window(
        &mut self,
        renderer: &mut GlesRenderer,
        window: &W::Id,
        blocker: TransactionBlocker,
    ) {
        if self.is_minimized(window) {
            return;
        }
        if self.floating.has_window(window) {
            self.floating
                .start_close_animation_for_window(renderer, window, blocker);
        } else if self.dwindle.has_window(window) {
            self.dwindle
                .start_close_animation_for_window(renderer, window, blocker);
        } else {
            self.scrolling
                .start_close_animation_for_window(renderer, window, blocker);
        }
    }

    pub fn start_close_animation_for_window_native(
        &mut self,
        renderer: &mut crate::render_helpers::vulkan::native::NativeRenderer,
        window: &W::Id,
        blocker: TransactionBlocker,
    ) {
        if self.is_minimized(window) {
            return;
        }
        if self.floating.has_window(window) {
            self.floating
                .start_close_animation_for_window_native(renderer, window, blocker);
        } else if self.dwindle.has_window(window) {
            self.dwindle
                .start_close_animation_for_window_native(renderer, window, blocker);
        } else {
            self.scrolling
                .start_close_animation_for_window_native(renderer, window, blocker);
        }
    }

    pub fn start_close_animation_for_tile(
        &mut self,
        renderer: &mut GlesRenderer,
        snapshot: TileRenderSnapshot,
        tile_size: Size<f64, Logical>,
        tile_pos: Point<f64, Logical>,
        blocker: TransactionBlocker,
        dmabuf: Option<smithay::backend::allocator::dmabuf::Dmabuf>,
    ) {
        self.floating.start_close_animation_for_tile(
            renderer, snapshot, tile_size, tile_pos, blocker, dmabuf,
        );
    }

    pub fn start_close_animation_for_tile_native(
        &mut self,
        renderer: &mut crate::render_helpers::vulkan::native::NativeRenderer,
        tile_size: Size<f64, Logical>,
        tile_pos: Point<f64, Logical>,
        blocker: TransactionBlocker,
        snapshot: Option<super::tile::NativeTileSnapshot>,
        dmabuf: Option<smithay::backend::allocator::dmabuf::Dmabuf>,
    ) {
        self.floating.start_close_animation_for_tile_native(
            renderer, tile_size, tile_pos, blocker, snapshot, dmabuf,
        );
    }

    pub fn start_open_animation(&mut self, id: &W::Id) -> bool {
        self.scrolling.start_open_animation(id)
            || self.dwindle.start_open_animation(id)
            || self.floating.start_open_animation(id)
    }

    pub fn window_under(&self, pos: Point<f64, Logical>) -> Option<(&W, HitType)> {
        // This logic is consistent with tiles_with_render_positions().
        if self.is_floating_visible() {
            if let Some(rv) = self
                .floating
                .tiles_with_render_positions()
                .find_map(|(tile, tile_pos)| HitType::hit_tile(tile, tile_pos, pos))
            {
                return Some(rv);
            }
        }

        if self.tiling_mode() == realand_ipc::TilingMode::Dwindle {
            self.dwindle.window_under(pos)
        } else {
            self.scrolling.window_under(pos)
        }
    }

    pub fn resize_edges_under(&self, pos: Point<f64, Logical>) -> Option<ResizeEdge> {
        self.tiles_with_render_positions()
            .find_map(|(tile, tile_pos, visible)| {
                // This logic should be consistent with window_under() in when it returns Some vs.
                // None.
                if !visible {
                    return None;
                }

                let pos_within_tile = pos - tile_pos;

                if tile.hit(pos_within_tile).is_some() {
                    let size = tile.tile_size().to_f64();

                    let mut edges = ResizeEdge::empty();
                    if pos_within_tile.x < size.w / 3. {
                        edges |= ResizeEdge::LEFT;
                    } else if 2. * size.w / 3. < pos_within_tile.x {
                        edges |= ResizeEdge::RIGHT;
                    }
                    if pos_within_tile.y < size.h / 3. {
                        edges |= ResizeEdge::TOP;
                    } else if 2. * size.h / 3. < pos_within_tile.y {
                        edges |= ResizeEdge::BOTTOM;
                    }
                    return Some(edges);
                }

                None
            })
    }

    pub fn descendants_added(&mut self, id: &W::Id) -> bool {
        self.floating.descendants_added(id)
    }

    pub fn update_window(&mut self, window: &W::Id, serial: Option<Serial>) {
        self.with_layout_transition(|this| {
            if let Some(parked) = this
                .minimized
                .iter_mut()
                .find(|p| p.tile.window().id() == window)
            {
                if let Some(serial) = serial {
                    parked.tile.window_mut().on_commit(serial);
                }
                parked.tile.update_window();
                return;
            }
            if !this.floating.update_window(window, serial)
                && !this.dwindle.update_window(window, serial)
            {
                this.scrolling.update_window(window, serial);
            }
        })
    }

    pub fn refresh(&mut self, is_active: bool, is_focused: bool) {
        if is_focused {
            self.core.borrow_mut().apply(CoreEvent::ActivateSpace {
                id: SpaceId(self.id.get()),
            });
        }
        for parked in &mut self.minimized {
            let window = parked.tile.window_mut();
            window.set_activated(false);
            window.set_active_in_column(false);
            window.cancel_interactive_resize();
            if matches!(
                window.configure_intent(),
                super::ConfigureIntent::CanSend | super::ConfigureIntent::ShouldSend
            ) {
                window.send_pending_configure();
            }
            window.refresh();
        }
        self.scrolling
            .refresh(is_active && !self.active_layer().is_floating(), is_focused);
        self.dwindle
            .refresh(is_active && !self.active_layer().is_floating(), is_focused);
        self.floating
            .refresh(is_active && self.active_layer().is_floating(), is_focused);
    }

    pub fn scroll_amount_to_activate(&self, window: &W::Id) -> f64 {
        if self.is_minimized(window)
            || self.floating.has_window(window)
            || self.dwindle.has_window(window)
        {
            return 0.;
        }

        self.scrolling.scroll_amount_to_activate(window)
    }

    pub fn is_urgent(&self) -> bool {
        self.windows().any(|win| win.is_urgent())
    }

    pub fn activate_window(&mut self, window: &W::Id) -> bool {
        self.with_layout_transition(|this| {
            this.restore_window(window, true);
            if this.floating.activate_window(window) {
                this.set_active_layer(SpaceLayerFocus::Floating);
                true
            } else if this.dwindle.has_window(window) {
                this.dwindle.set_active_window(window.clone());
                this.set_active_layer(SpaceLayerFocus::Tiling);
                true
            } else if this.scrolling.activate_window(window) {
                this.set_active_layer(SpaceLayerFocus::Tiling);
                true
            } else {
                false
            }
        })
    }

    pub fn activate_window_without_raising(&mut self, window: &W::Id) -> bool {
        self.with_layout_transition(|this| {
            this.restore_window(window, true);
            if this.floating.activate_window_without_raising(window) {
                this.set_active_layer(SpaceLayerFocus::Floating);
                true
            } else if this.dwindle.has_window(window) {
                this.dwindle.set_active_window(window.clone());
                this.set_active_layer(SpaceLayerFocus::Tiling);
                true
            } else if this.scrolling.activate_window(window) {
                let focus = match this.active_layer() {
                    SpaceLayerFocus::Tiling => SpaceLayerFocus::Tiling,
                    SpaceLayerFocus::TilingWithFloatingRaised | SpaceLayerFocus::Floating => {
                        SpaceLayerFocus::TilingWithFloatingRaised
                    }
                };
                this.set_active_layer(focus);
                true
            } else {
                false
            }
        })
    }

    pub(super) fn scrolling_insert_position(&self, pos: Point<f64, Logical>) -> InsertPosition {
        self.scrolling.insert_position(pos)
    }

    pub(super) fn insert_hint_area(
        &self,
        position: InsertPosition,
    ) -> Option<Rectangle<f64, Logical>> {
        self.scrolling.insert_hint_area(position)
    }

    pub fn view_offset_gesture_begin(&mut self, is_touchpad: bool) {
        self.scrolling.view_offset_gesture_begin(is_touchpad);
    }

    pub fn view_offset_gesture_update(
        &mut self,
        delta_x: f64,
        timestamp: Duration,
        is_touchpad: bool,
    ) -> Option<bool> {
        self.scrolling
            .view_offset_gesture_update(delta_x, timestamp, is_touchpad)
    }

    pub fn view_offset_gesture_end(&mut self, is_touchpad: Option<bool>) -> bool {
        self.scrolling.view_offset_gesture_end(is_touchpad)
    }

    pub fn view_offset_gesture_cancel(&mut self, is_touchpad: Option<bool>) -> bool {
        self.scrolling.view_offset_gesture_cancel(is_touchpad)
    }

    pub fn dnd_scroll_gesture_begin(&mut self) {
        self.scrolling.dnd_scroll_gesture_begin();
    }

    pub fn dnd_scroll_gesture_scroll(&mut self, pos: Point<f64, Logical>, speed: f64) -> bool {
        let config = &self.options.gestures.dnd_edge_view_scroll;
        let trigger_width = config.trigger_width;

        // This working area intentionally does not include extra struts from Options.
        let x = pos.x - self.working_area.loc.x;
        let width = self.working_area.size.w;

        let x = x.clamp(0., width);
        let trigger_width = trigger_width.clamp(0., width / 2.);

        let delta = if x < trigger_width {
            -(trigger_width - x)
        } else if width - x < trigger_width {
            trigger_width - (width - x)
        } else {
            0.
        };

        let delta = if trigger_width < 0.01 {
            // Sanity check for trigger-width 0 or small window sizes.
            0.
        } else {
            // Normalize to [0, 1].
            delta / trigger_width
        };
        let delta = delta * speed;

        self.scrolling.dnd_scroll_gesture_scroll(delta)
    }

    pub fn dnd_scroll_gesture_end(&mut self) {
        self.scrolling.dnd_scroll_gesture_end();
    }

    pub fn interactive_resize_begin(&mut self, window: W::Id, edges: ResizeEdge) -> bool {
        if self.floating.has_window(&window) {
            self.floating.interactive_resize_begin(window, edges)
        } else if self.dwindle.has_window(&window) {
            self.dwindle.interactive_resize_begin(window, edges)
        } else {
            self.scrolling.interactive_resize_begin(window, edges)
        }
    }

    pub fn interactive_resize_update(
        &mut self,
        window: &W::Id,
        delta: Point<f64, Logical>,
    ) -> bool {
        self.with_layout_transition(|this| {
            if this.floating.has_window(window) {
                this.floating.interactive_resize_update(window, delta)
            } else if this.dwindle.has_window(window) {
                this.dwindle.interactive_resize_update(window, delta)
            } else {
                this.scrolling.interactive_resize_update(window, delta)
            }
        })
    }

    pub fn interactive_resize_end(&mut self, window: Option<&W::Id>) {
        self.with_layout_transition(|this| {
            if let Some(window) = window {
                if this.floating.has_window(window) {
                    this.floating.interactive_resize_end(Some(window));
                } else if this.dwindle.has_window(window) {
                    this.dwindle.interactive_resize_end(Some(window));
                } else {
                    this.scrolling.interactive_resize_end(Some(window));
                }
            } else {
                this.floating.interactive_resize_end(None);
                this.dwindle.interactive_resize_end(None);
                this.scrolling.interactive_resize_end(None);
            }
        })
    }

    fn active_layer(&self) -> SpaceLayerFocus {
        self.core
            .borrow()
            .space(SpaceId(self.id.get()))
            .map(|space| space.interaction.layer_focus())
            .unwrap_or_default()
    }

    fn set_active_layer(&self, focus: SpaceLayerFocus) {
        self.core
            .borrow_mut()
            .set_space_layer_focus(SpaceId(self.id.get()), focus);
    }

    pub fn floating_is_active(&self) -> bool {
        self.active_layer().is_floating()
    }

    pub fn floating_logical_to_size_frac(
        &self,
        logical_pos: Point<f64, Logical>,
    ) -> Point<f64, SizeFrac> {
        self.floating.logical_to_size_frac(logical_pos)
    }

    pub fn working_area(&self) -> Rectangle<f64, Logical> {
        self.working_area
    }

    pub fn layout_config(&self) -> Option<&realand_config::LayoutPart> {
        self.layout_config.as_ref()
    }

    pub fn scrolling(&self) -> &ScrollingSpace<W> {
        &self.scrolling
    }

    pub fn scrolling_mut(&mut self) -> &mut ScrollingSpace<W> {
        &mut self.scrolling
    }

    pub fn floating(&self) -> &FloatingSpace<W> {
        &self.floating
    }

    #[cfg(test)]
    pub(super) fn prefers_output(&self, output: &Output) -> bool {
        self.core
            .borrow()
            .space_prefers_output(SpaceId(self.id.get()), &output.name())
    }

    pub(super) fn prefer_output(&self, output: &Output) {
        self.core
            .borrow_mut()
            .prefer_space_output(SpaceId(self.id.get()), output);
    }

    pub(super) fn prefer_output_after_client(&self, output: &Output) {
        self.core
            .borrow_mut()
            .prefer_unnamed_space_output(SpaceId(self.id.get()), output);
    }

    #[cfg(test)]
    pub fn verify_invariants(&self, move_win_id: Option<&W::Id>) {
        use approx::assert_abs_diff_eq;

        let core = self.core.borrow();
        let space = SpaceId(self.id.get());
        assert!(
            core.space(space).is_some(),
            "physical workspace needs a core identity"
        );
        let parked: std::collections::HashSet<_> = self
            .minimized
            .iter()
            .map(|entry| entry.tile.window().core_id())
            .collect();
        let physical: std::collections::HashSet<_> =
            self.windows().map(LayoutElement::core_id).collect();
        for tile in self.tiles() {
            let window = tile.window();
            let record = core
                .window(&window.core_id())
                .expect("physical tile needs a stable core identity");
            assert!(
                record.attached,
                "physical workspace tile cannot be a detached interaction"
            );
            assert_eq!(record.space, space);
            assert_eq!(record.output, self.current_output().map(Output::name));
            assert_eq!(record.minimized, parked.contains(&window.core_id()));
            assert_eq!(
                record.minimized,
                record.restoration.is_some(),
                "parked restore decisions belong to core"
            );
            assert_eq!(self.core.id(window.id()), Some(window.core_id()));
        }
        for record in core
            .windows()
            .filter(|record| record.attached && record.space == space)
        {
            assert!(
                physical.contains(&record.id),
                "attached core identity needs a physical client adapter"
            );
        }
        drop(core);
        assert_eq!(
            self.canvas_enabled(),
            self.floating.canvas_enabled(),
            "canvas intent must be applied to the platform kinetic payload"
        );

        let scale = self.scale.fractional_scale();
        assert!(scale > 0.);
        assert!(scale.is_finite());

        let options = Options::clone(&self.base_options)
            .with_merged_layout(self.layout_config.as_ref())
            .adjusted_for_scale(scale);
        assert_eq!(
            &*self.options, &options,
            "options must be base options adjusted for scale"
        );

        assert!(self.view_size.w > 0.);
        assert!(self.view_size.h > 0.);

        assert_eq!(self.background_buffer.size(), self.view_size);
        assert_eq!(
            self.background_buffer.color().components(),
            options.layout.background_color.to_array_unpremul(),
        );

        assert_eq!(self.view_size, self.scrolling.view_size());
        assert_eq!(self.working_area, self.scrolling.parent_area());
        assert_eq!(&self.clock, self.scrolling.clock());
        assert!(Rc::ptr_eq(&self.options, self.scrolling.options()));
        self.scrolling.verify_invariants();

        assert_eq!(self.view_size, self.floating.view_size());
        assert_eq!(self.working_area, self.floating.working_area());
        assert_eq!(&self.clock, self.floating.clock());
        assert!(Rc::ptr_eq(&self.options, self.floating.options()));
        self.floating.verify_invariants();

        if self.floating.is_empty() {
            assert!(
                !self.active_layer().is_floating(),
                "when floating is empty it must never be active"
            );
        } else if self.tiling_is_empty() {
            assert!(
                self.active_layer().is_floating(),
                "when tiling is empty but floating isn't, floating should be active"
            );
        }

        for (tile, tile_pos, visible) in self.tiles_with_render_positions() {
            if Some(tile.window().id()) != move_win_id {
                assert_eq!(tile.interactive_move_offset, Point::from((0., 0.)));
            }

            let rounded_pos = tile_pos.to_physical_precise_round(scale).to_logical(scale);

            // Tile positions must be rounded to physical pixels.
            assert_abs_diff_eq!(tile_pos.x, rounded_pos.x, epsilon = 1e-5);
            assert_abs_diff_eq!(tile_pos.y, rounded_pos.y, epsilon = 1e-5);

            if let Some(alpha) = &tile.alpha_animation {
                let anim = &alpha.anim;
                if visible {
                    assert_eq!(anim.to(), 1., "visible tiles can animate alpha only to 1");
                }

                assert!(
                    !alpha.hold_after_done,
                    "tiles in the layout cannot have held alpha animation"
                );
            }
        }
    }
}

pub(super) fn compute_working_area(output: &Output) -> Rectangle<f64, Logical> {
    layer_map_for_output(output).non_exclusive_zone().to_f64()
}

fn compute_workspace_shadow_config(
    config: realand_config::WorkspaceShadow,
    view_size: Size<f64, Logical>,
) -> realand_config::Shadow {
    // Gaps between workspaces are a multiple of the view height, so shadow settings should also be
    // normalized to the view height to prevent them from overlapping on lower resolutions.
    let norm = view_size.h / 1080.;

    let mut config = realand_config::Shadow::from(config);
    config.softness *= norm;
    config.spread *= norm;
    config.offset.x.0 *= norm;
    config.offset.y.0 *= norm;

    config
}
