mod spread;
mod stage;
use std::cell::RefCell;
use std::cmp::min;
use std::collections::HashMap;
use std::f64::consts::FRAC_PI_2;
use std::iter::zip;
use std::rc::Rc;
use std::time::Duration;

use super::core_adapter::{self, DesktopContext};
use realand_config::{CornerRadius, LayoutPart};
use realand_core::SpaceId;
use smithay::backend::renderer::element::utils::{
    CropRenderElement, Relocate, RelocateRenderElement, RescaleRenderElement,
};
use smithay::backend::renderer::element::{Element as _, Id, Kind};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::renderer::ErasedContextId;
use smithay::output::Output;
use smithay::utils::{Logical, Point, Rectangle, Size};

use super::cube::face_mesh;
use super::insert_hint_element::{InsertHintElement, InsertHintRenderElement};
use super::scrolling::{Column, ColumnWidth};
use super::tile::Tile;
use super::workspace::{
    compute_working_area, Workspace, WorkspaceAddWindowTarget, WorkspaceId, WorkspaceRenderElement,
};
use super::{
    compute_overview_zoom, ActivateWindow, HitType, LayoutElement, Options, OverviewAction,
    OverviewMode,
};
use crate::animation::{Animation, Clock};
use crate::layout::RenderLayer;
use crate::realand_render_elements;
use crate::render_helpers::mesh::MeshRenderElement;
use crate::render_helpers::offscreen::{NativeOffscreenBuffer, OffscreenBuffer, OffscreenData};
use crate::render_helpers::renderer::RealandRenderer;
use crate::render_helpers::shadow::ShadowRenderElement;
use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
use crate::render_helpers::vulkan::native::NativeRenderer;
use crate::render_helpers::xray::XrayPos;
use crate::render_helpers::RenderCtx;
use crate::utils::transaction::Transaction;
use crate::utils::{
    output_size, round_logical_in_physical, round_logical_in_physical_max1, ResizeEdge,
};

type ExpoGeometryCache = Option<([u64; 7], Vec<WorkspaceId>, Rc<Vec<Rectangle<f64, Logical>>>)>;
type StageRailText = (String, u64, ErasedContextId, crate::ui::mru::MruTexture);

#[derive(Debug)]
struct ExpoTransition {
    start_progress: f64,
    target_progress: f64,
    /// A gesture can move toward either endpoint from its captured origin.
    is_gesture: bool,
    full_size_workspace: WorkspaceId,
    start_rects: Option<HashMap<WorkspaceId, Rectangle<f64, Logical>>>,
}

fn interpolate_expo_rect(
    from: Rectangle<f64, Logical>,
    to: Rectangle<f64, Logical>,
    fraction: f64,
    view_size: Size<f64, Logical>,
) -> Rectangle<f64, Logical> {
    if fraction == 0. {
        from
    } else if fraction == 1. {
        to
    } else {
        // Reversing a spring preserves velocity. Until it turns around, its
        // normalized fraction is negative: clamping here freezes every card at
        // the captured pose. Permit that continuation within the desktop bounds.
        // A closing anchor can change just after opening, when the normalized
        // progress interval is tiny: unbounded extrapolation would invert the
        // new anchor and magnify the old desktop many times beyond the output.
        let mut fraction = fraction;
        if !(0. ..=1.).contains(&fraction) {
            for (start, end, min, max) in [
                (
                    from.size.w,
                    to.size.w,
                    0.,
                    view_size.w.max(from.size.w).max(to.size.w),
                ),
                (
                    from.size.h,
                    to.size.h,
                    0.,
                    view_size.h.max(from.size.h).max(to.size.h),
                ),
                (
                    from.loc.x,
                    to.loc.x,
                    from.loc.x.min(to.loc.x) - view_size.w,
                    from.loc.x.max(to.loc.x) + view_size.w,
                ),
                (
                    from.loc.y,
                    to.loc.y,
                    from.loc.y.min(to.loc.y) - view_size.h,
                    from.loc.y.max(to.loc.y) + view_size.h,
                ),
            ] {
                let delta = end - start;
                if delta > 0. {
                    fraction = fraction.clamp((min - start) / delta, (max - start) / delta);
                } else if delta < 0. {
                    fraction = fraction.clamp((max - start) / delta, (min - start) / delta);
                }
            }
        }
        Rectangle::new(
            from.loc + (to.loc - from.loc).upscale(fraction),
            Size::from((
                (from.size.w + (to.size.w - from.size.w) * fraction).max(0.),
                (from.size.h + (to.size.h - from.size.h) * fraction).max(0.),
            )),
        )
    }
}

#[derive(Debug)]
pub struct Monitor<W: LayoutElement> {
    pub(super) core: DesktopContext<W::Id>,
    /// Output for this monitor.
    pub(super) output: Output,
    /// Cached name of the output.
    output_name: String,
    /// Latest known scale for this output.
    scale: smithay::output::Scale,
    /// Latest known size for this output.
    view_size: Size<f64, Logical>,
    /// Latest known working area for this output.
    ///
    /// Not rounded to physical pixels.
    // FIXME: since this is used for things like DnD scrolling edges in the overview, ideally this
    // should only consider overlay and top layer-shell surfaces. However, Smithay doesn't easily
    // let you do this at the moment.
    working_area: Rectangle<f64, Logical>,
    // Must always contain at least one.
    pub(super) workspaces: Vec<Workspace<W>>,
    /// In-progress switch between workspaces.
    pub(super) workspace_switch: Option<WorkspaceSwitch>,
    /// Indication where an interactively-moved window is about to be placed.
    pub(super) insert_hint: Option<InsertHint>,
    /// Insert hint element for rendering.
    insert_hint_element: InsertHintElement,
    /// Location to render the insert hint element.
    insert_hint_render_loc: Option<InsertHintRenderLoc>,
    /// Progress of the overview zoom animation, 1 is fully in overview.
    overview_progress: Option<OverviewProgress>,
    /// Clock for driving animations.
    pub(super) clock: Clock,
    /// Configurable properties of the layout as received from the parent layout.
    pub(super) base_options: Rc<Options>,
    /// Configurable properties of the layout.
    pub(super) options: Rc<Options>,
    /// Layout config overrides for this monitor.
    layout_config: Option<realand_config::LayoutPart>,

    /// At most two neighboring workspace textures are cached for cube transitions.
    cube_buffers: [OffscreenBuffer; 2],
    /// Vulkan counterparts keeping damage history, so steady cube frames skip
    /// re-rendering workspace contents entirely.
    cube_native_buffers: [NativeOffscreenBuffer; 2],
    cube_elements: [RefCell<Vec<CubeFaceRenderElement<GlesRenderer>>>; 2],
    cube_ids: [Id; 2],
    cube_commits: RefCell<[CommitCounter; 2]>,
    cube_face_states: RefCell<[Option<CubeFaceState>; 2]>,
    cube_visible_workspaces: RefCell<[Option<WorkspaceId>; 2]>,
    cube_background: RefCell<SolidColorBuffer>,
    spread_header: RefCell<Option<(String, u64, ErasedContextId, crate::ui::mru::MruTexture)>>,
    spread_transition: Option<spread::SpreadTransition<W::Id>>,
    spread_closing: HashMap<Id, spread::SpreadClosing>,

    expo_geometry: RefCell<ExpoGeometryCache>,
    expo_borders: RefCell<Vec<[SolidColorBuffer; 4]>>,
    /// Card origins and destination for the current Expo direction, independent of selection.
    expo_transition: Option<ExpoTransition>,
    stage: stage::StageState,
    stage_geometry: RefCell<stage::StageGeometryCache>,
    stage_rail_text: RefCell<Vec<Option<StageRailText>>>,
    stage_rail_buffers: RefCell<Vec<SolidColorBuffer>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CubeFaceState {
    workspace_idx: usize,
    source_commit: CommitCounter,
    source_texture: u64,
    angle_bits: u64,
    perspective_bits: u64,
    width_bits: u64,
    height_bits: u64,
    scale_bits: u64,
    focus_ring: bool,
}

#[derive(Debug)]
pub enum WorkspaceSwitch {
    Animation(Animation),
    Gesture(WorkspaceSwitchGesture),
}

#[derive(Debug)]
pub struct WorkspaceSwitchGesture {
    /// A renderer-side correction after an activation during a DnD gesture.
    /// Position, stable center identity, tracker and debounce belong to core.
    animation: Option<Animation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InsertPosition {
    NewColumn(usize),
    InColumn(usize, usize),
    Floating,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InsertWorkspace {
    Existing(WorkspaceId),
    NewAt(usize),
}

#[derive(Debug)]
pub(super) struct InsertHint {
    pub workspace: InsertWorkspace,
    pub position: InsertPosition,
    pub corner_radius: CornerRadius,
}

#[derive(Debug, Clone, Copy)]
struct InsertHintRenderLoc {
    workspace: InsertWorkspace,
    location: Point<f64, Logical>,
}

#[derive(Debug)]
pub(super) enum OverviewProgress {
    Animation(Animation),
    Value(f64),
}

/// Where to put a newly added window.
#[derive(Debug, Default, PartialEq, Eq)]
pub enum MonitorAddWindowTarget<'a, W: LayoutElement> {
    /// No particular preference.
    #[default]
    Auto,
    /// On this workspace.
    Workspace {
        /// Id of the target workspace.
        id: WorkspaceId,
        /// Override where the window will open as a new column.
        column_idx: Option<usize>,
    },
    /// Next to this existing window.
    NextTo(&'a W::Id),
}

impl<'a, W: LayoutElement> Copy for MonitorAddWindowTarget<'a, W> {}

impl<'a, W: LayoutElement> Clone for MonitorAddWindowTarget<'a, W> {
    fn clone(&self) -> Self {
        *self
    }
}

realand_render_elements! {
    MonitorInnerRenderElement<R> => {
        Workspace = CropRenderElement<WorkspaceRenderElement<R>>,
        InsertHint = CropRenderElement<InsertHintRenderElement>,
        UncroppedInsertHint = InsertHintRenderElement,
        Shadow = ShadowRenderElement,
        SolidColor = SolidColorRenderElement,
        Mesh = MeshRenderElement,
        SpreadTile = super::tile::TileRenderElement<R>,
        SpreadClosing = super::closing_window::ClosingWindowRenderElement,
        SpreadText = crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement,
        StageText = CropRenderElement<crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement>,
        StageLayer = CropRenderElement<crate::layer::mapped::LayerSurfaceRenderElement<R>>,
    }
}

realand_render_elements! {
    CubeFaceRenderElement<R> => {
        Workspace = CropRenderElement<WorkspaceRenderElement<R>>,
        SolidColor = SolidColorRenderElement,
    }
}

pub type MonitorRenderElement<R> =
    RelocateRenderElement<RescaleRenderElement<MonitorInnerRenderElement<R>>>;

impl WorkspaceSwitch {
    fn is_animation_ongoing(&self) -> bool {
        match self {
            WorkspaceSwitch::Animation(_) => true,
            WorkspaceSwitch::Gesture(gesture) => gesture.animation.is_some(),
        }
    }
}

impl WorkspaceSwitchGesture {
    fn animate_from(&mut self, from: f64, clock: Clock, config: realand_config::Animation) {
        let current = self.animation.as_ref().map_or(0., Animation::value);
        let velocity = self.animation.as_ref().map_or(0., Animation::velocity);
        self.animation = Some(Animation::new(clock, from + current, 0., velocity, config));
    }
}

impl InsertWorkspace {
    fn existing_id(self) -> Option<WorkspaceId> {
        match self {
            InsertWorkspace::Existing(id) => Some(id),
            InsertWorkspace::NewAt(_) => None,
        }
    }
}

impl OverviewProgress {
    pub fn value(&self) -> f64 {
        match self {
            OverviewProgress::Animation(anim) => anim.value(),
            OverviewProgress::Value(v) => *v,
        }
    }

    pub fn clamped_value(&self) -> f64 {
        match self {
            OverviewProgress::Animation(anim) => anim.clamped_value(),
            OverviewProgress::Value(v) => *v,
        }
    }
}

impl<W: LayoutElement> Monitor<W> {
    pub fn new(
        output: Output,
        workspaces: Vec<Workspace<W>>,
        ws_id_to_activate: Option<WorkspaceId>,
        clock: Clock,
        base_options: Rc<Options>,
        layout_config: Option<LayoutPart>,
    ) -> Self {
        Self::new_with_stage_drag_source(
            output,
            workspaces,
            ws_id_to_activate,
            clock,
            base_options,
            layout_config,
            None,
        )
    }

    pub(in crate::layout) fn new_with_stage_drag_source(
        output: Output,
        mut workspaces: Vec<Workspace<W>>,
        ws_id_to_activate: Option<WorkspaceId>,
        clock: Clock,
        base_options: Rc<Options>,
        layout_config: Option<LayoutPart>,
        stage_drag_source: Option<WorkspaceId>,
    ) -> Self {
        let options =
            Rc::new(Options::clone(&base_options).with_merged_layout(layout_config.as_ref()));

        let scale = output.current_scale();
        let view_size = output_size(&output);
        let working_area = compute_working_area(&output);

        let core = workspaces
            .first()
            .map(|workspace| workspace.core.clone())
            .unwrap_or_else(core_adapter::new_context);
        core_adapter::connect_output(&core, &output);
        for workspace in &mut workspaces {
            workspace.set_core(core.clone());
        }

        // Prepare the workspaces: set output, empty first, empty last.
        let mut active_workspace_idx = 0;

        for (idx, ws) in workspaces.iter_mut().enumerate() {
            assert!(ws.has_windows_or_name() || Some(ws.id()) == stage_drag_source);

            ws.set_output(Some(output.clone()));
            ws.update_config(options.clone());

            if ws_id_to_activate.is_some_and(|id| ws.id() == id) {
                active_workspace_idx = idx;
            }
        }

        if options.layout.empty_workspace_above_first && !workspaces.is_empty() {
            let ws = Workspace::new(output.clone(), clock.clone(), options.clone())
                .with_core(core.clone());
            workspaces.insert(0, ws);
            active_workspace_idx += 1;
        }

        let ws =
            Workspace::new(output.clone(), clock.clone(), options.clone()).with_core(core.clone());
        workspaces.push(ws);

        let cube_background = SolidColorBuffer::new(
            view_size,
            options.animations.workspace_switch.1.background_color,
        );

        {
            let mut desktop = core.borrow_mut();
            for (index, workspace) in workspaces.iter().enumerate() {
                desktop.attach_space(SpaceId(workspace.id().get()), &output.name(), index);
            }
            desktop.select_output_space(
                &output.name(),
                SpaceId(workspaces[active_workspace_idx].id().get()),
                false,
            );
        }

        Self {
            core,
            output_name: output.name(),
            output,
            scale,
            view_size,
            working_area,
            workspaces,
            insert_hint: None,
            insert_hint_element: InsertHintElement::new(options.layout.insert_hint),
            insert_hint_render_loc: None,
            overview_progress: None,
            workspace_switch: None,
            clock,
            base_options,
            options,
            layout_config,
            cube_buffers: [OffscreenBuffer::default(), OffscreenBuffer::default()],
            cube_native_buffers: [
                NativeOffscreenBuffer::default(),
                NativeOffscreenBuffer::default(),
            ],
            cube_elements: std::array::from_fn(|_| RefCell::new(Vec::new())),
            cube_ids: [Id::new(), Id::new()],
            cube_commits: RefCell::new([CommitCounter::default(), CommitCounter::default()]),
            cube_face_states: RefCell::new([None, None]),
            cube_visible_workspaces: RefCell::new([None, None]),
            cube_background: RefCell::new(cube_background),
            spread_header: RefCell::new(None),
            spread_transition: None,
            spread_closing: HashMap::new(),
            expo_geometry: RefCell::new(None),
            expo_borders: RefCell::new(Vec::new()),
            expo_transition: None,
            stage: stage::StageState::default(),
            stage_geometry: RefCell::new(None),
            stage_rail_text: RefCell::new(Vec::new()),
            stage_rail_buffers: RefCell::new(Vec::new()),
        }
    }

    pub fn into_workspaces(mut self) -> Vec<Workspace<W>> {
        let source = self.stage_drag_source();
        self.workspaces
            .retain(|ws| ws.has_windows_or_name() || Some(ws.id()) == source);

        for ws in &mut self.workspaces {
            ws.set_output(None);
        }

        self.workspaces
    }

    pub(super) fn select_output(&self) {
        self.core.borrow_mut().select_output(&self.output_name);
    }

    pub(super) fn is_selected_output(&self) -> bool {
        self.core.borrow().navigation().selected_output() == Some(self.output_name.as_str())
    }

    pub fn output(&self) -> &Output {
        &self.output
    }

    pub fn output_name(&self) -> &String {
        &self.output_name
    }

    /// Translate the independent core's stable display selection to a device payload slot.
    pub fn active_workspace_idx(&self) -> usize {
        let id = self
            .core
            .borrow()
            .navigation()
            .active_space(&self.output_name)
            .expect("connected monitor must have a displayed workspace");
        self.workspaces
            .iter()
            .position(|workspace| workspace.id().get() == id.0)
            .expect("displayed workspace must have a monitor payload")
    }

    pub fn active_workspace_ref(&self) -> &Workspace<W> {
        &self.workspaces[self.active_workspace_idx()]
    }

    pub fn find_named_workspace(&self, workspace_name: &str) -> Option<&Workspace<W>> {
        let id = self.core.borrow().named_space(workspace_name)?;
        self.workspaces
            .iter()
            .find(|workspace| workspace.id().get() == id.0)
    }

    pub fn find_named_workspace_index(&self, workspace_name: &str) -> Option<usize> {
        let id = self.core.borrow().named_space(workspace_name)?;
        self.workspaces
            .iter()
            .position(|workspace| workspace.id().get() == id.0)
    }

    pub fn active_workspace(&mut self) -> &mut Workspace<W> {
        let index = self.active_workspace_idx();
        &mut self.workspaces[index]
    }

    pub fn idx_of_ws(&self, id: WorkspaceId) -> Option<usize> {
        self.workspaces.iter().position(|ws| ws.id() == id)
    }

    pub fn has_ws(&self, id: WorkspaceId) -> bool {
        self.idx_of_ws(id).is_some()
    }

    pub(super) fn set_core(&mut self, context: DesktopContext<W::Id>) {
        if Rc::ptr_eq(&self.core, &context) {
            return;
        }
        let active = self.active_workspace_ref().id();
        let previous = self
            .core
            .borrow()
            .navigation()
            .previous_space(&self.output_name);
        self.core = context;
        core_adapter::connect_output(&self.core, &self.output);
        for workspace in &mut self.workspaces {
            workspace.set_core(self.core.clone());
        }
        let mut desktop = self.core.borrow_mut();
        for (index, workspace) in self.workspaces.iter().enumerate() {
            desktop.attach_space(SpaceId(workspace.id().get()), &self.output_name, index);
        }
        desktop.select_output_space(&self.output_name, SpaceId(active.get()), false);
        desktop.set_previous_space(&self.output_name, previous);
    }

    pub fn windows(&self) -> impl Iterator<Item = &W> {
        self.workspaces.iter().flat_map(|ws| ws.windows())
    }

    pub fn closing_windows(
        &self,
    ) -> impl Iterator<Item = &crate::layout::closing_window::ClosingWindow> {
        self.workspaces.iter().flat_map(|ws| ws.closing_windows())
    }

    pub fn has_window(&self, window: &W::Id) -> bool {
        self.windows().any(|win| win.id() == window)
    }

    pub fn add_workspace_at(&mut self, idx: usize) {
        let ws = Workspace::new(
            self.output.clone(),
            self.clock.clone(),
            self.options.clone(),
        )
        .with_core(self.core.clone());

        let id = SpaceId(ws.id().get());
        self.workspaces.insert(idx, ws);
        self.core
            .borrow_mut()
            .attach_space(id, &self.output_name, idx);

        if let Some(WorkspaceSwitch::Animation(animation)) = &mut self.workspace_switch {
            if idx as f64 <= animation.to() {
                animation.offset(1.);
            }
        }
    }

    pub fn add_workspace_top(&mut self) {
        self.add_workspace_at(0);
    }

    pub fn add_workspace_bottom(&mut self) {
        self.add_workspace_at(self.workspaces.len());
    }

    pub fn activate_workspace(&mut self, idx: usize) {
        self.activate_workspace_with_anim_config(idx, None);
    }

    pub fn activate_workspace_with_anim_config(
        &mut self,
        idx: usize,
        config: Option<realand_config::Animation>,
    ) {
        let current_idx = self.workspace_render_idx();
        let velocity = self.workspace_render_velocity();

        let prev_active_idx = self.active_workspace_idx();
        self.core.borrow_mut().select_output_space(
            &self.output_name,
            SpaceId(self.workspaces[idx].id().get()),
            true,
        );
        let workspace = &self.workspaces[idx];
        let focused = workspace.active_window().map(LayoutElement::core_id);
        self.core
            .borrow_mut()
            .set_space_focus(SpaceId(workspace.id().get()), focused, false);

        let config = config.unwrap_or(self.options.animations.workspace_switch.0);

        if self.workspace_gesture_is_dnd() {
            let outcome = self.core.borrow_mut().workspace_switch_action(
                realand_core::workspace_switch::WorkspaceSwitchAction::Activate {
                    connector: self.output_name.clone(),
                    target: SpaceId(self.workspaces[idx].id().get()),
                },
            );
            if let (Some(offset), Some(WorkspaceSwitch::Gesture(gesture))) =
                (outcome.animation_offset, &mut self.workspace_switch)
            {
                gesture.animate_from(offset, self.clock.clone(), config);
            }
        } else {
            if prev_active_idx == idx {
                return;
            }
            self.clear_workspace_switch();
            self.workspace_switch = Some(WorkspaceSwitch::Animation(Animation::new(
                self.clock.clone(),
                current_idx,
                idx as f64,
                velocity,
                config,
            )));
        }
    }

    pub(super) fn resolve_add_window_target<'a>(
        &mut self,
        target: MonitorAddWindowTarget<'a, W>,
    ) -> (usize, WorkspaceAddWindowTarget<'a, W>) {
        match target {
            MonitorAddWindowTarget::Auto => {
                (self.active_workspace_idx(), WorkspaceAddWindowTarget::Auto)
            }
            MonitorAddWindowTarget::Workspace { id, column_idx } => {
                let idx = self.idx_of_ws(id).unwrap();
                let target = if let Some(column_idx) = column_idx {
                    WorkspaceAddWindowTarget::NewColumnAt(column_idx)
                } else {
                    WorkspaceAddWindowTarget::Auto
                };
                (idx, target)
            }
            MonitorAddWindowTarget::NextTo(win_id) => {
                let idx = self
                    .workspaces
                    .iter_mut()
                    .position(|ws| ws.has_window(win_id))
                    .unwrap();
                (idx, WorkspaceAddWindowTarget::NextTo(win_id))
            }
        }
    }

    pub fn add_window(
        &mut self,
        window: W,
        target: MonitorAddWindowTarget<W>,
        activate: ActivateWindow,
        width: ColumnWidth,
        is_full_width: bool,
        is_floating: bool,
    ) {
        // Currently, everything a workspace sets on a Tile is the same across all workspaces of a
        // monitor. So we can use any workspace, not necessarily the exact target workspace.
        let tile = self.workspaces[0].make_tile(window);

        self.add_tile(
            tile,
            target,
            activate,
            true,
            width,
            is_full_width,
            is_floating,
            None,
        );
    }

    pub fn add_column(
        &mut self,
        mut workspace_idx: usize,
        column: Column<W>,
        activate: bool,
        anim: Option<realand_config::Animation>,
    ) {
        let workspace = &mut self.workspaces[workspace_idx];

        workspace.add_column(column, activate, anim);

        // After adding a new window, workspace becomes this output's own.
        workspace.prefer_output_after_client(&self.output);

        if workspace_idx == self.workspaces.len() - 1 {
            self.add_workspace_bottom();
        }
        if self.options.layout.empty_workspace_above_first && workspace_idx == 0 {
            self.add_workspace_top();
            workspace_idx += 1;
        }

        if activate {
            self.activate_workspace(workspace_idx);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_tile(
        &mut self,
        tile: Tile<W>,
        target: MonitorAddWindowTarget<W>,
        activate: ActivateWindow,
        // FIXME: Refactor ActivateWindow enum to make this better.
        allow_to_activate_workspace: bool,
        width: ColumnWidth,
        is_full_width: bool,
        is_floating: bool,
        anim: Option<realand_config::Animation>,
    ) {
        let (mut workspace_idx, target) = self.resolve_add_window_target(target);

        let workspace = &mut self.workspaces[workspace_idx];

        workspace.add_tile(
            tile,
            target,
            activate,
            width,
            is_full_width,
            is_floating,
            anim,
        );

        // After adding a new window, workspace becomes this output's own.
        workspace.prefer_output_after_client(&self.output);

        if workspace_idx == self.workspaces.len() - 1 {
            // Insert a new empty workspace.
            self.add_workspace_bottom();
        }

        if self.options.layout.empty_workspace_above_first && workspace_idx == 0 {
            self.add_workspace_top();
            workspace_idx += 1;
        }

        if allow_to_activate_workspace && activate.map_smart(|| false) {
            self.activate_workspace(workspace_idx);
        }
    }

    pub fn add_tile_to_column(
        &mut self,
        workspace_idx: usize,
        column_idx: usize,
        tile_idx: Option<usize>,
        tile: Tile<W>,
        activate: bool,
        // FIXME: Refactor ActivateWindow enum to make this better.
        allow_to_activate_workspace: bool,
    ) {
        let workspace = &mut self.workspaces[workspace_idx];

        workspace.add_tile_to_column(column_idx, tile_idx, tile, activate);

        // After adding a new window, workspace becomes this output's own.
        workspace.prefer_output_after_client(&self.output);

        // Since we're adding window to an existing column, the workspace isn't empty, and
        // therefore cannot be the last one, so we never need to insert a new empty workspace.

        if allow_to_activate_workspace && activate {
            self.activate_workspace(workspace_idx);
        }
    }

    pub fn clean_up_workspaces(&mut self) {
        assert!(self.workspace_switch.is_none());

        let range_start = if self.options.layout.empty_workspace_above_first {
            1
        } else {
            0
        };
        for idx in (range_start..self.workspaces.len() - 1).rev() {
            if self.active_workspace_idx() == idx
                || Some(self.workspaces[idx].id()) == self.stage_drag_source()
            {
                continue;
            }

            if !self.workspaces[idx].has_windows_or_name()
                && !self.workspaces[idx].has_closing_windows()
            {
                self.workspaces.remove(idx);
            }
        }

        // Special case handling when empty_workspace_above_first is set and all workspaces
        // are empty.
        if self.options.layout.empty_workspace_above_first
            && self.workspaces.len() == 2
            && self
                .stage_drag_source()
                .is_none_or(|id| self.idx_of_ws(id).is_none())
            && !self.workspaces[0].has_closing_windows()
            && !self.workspaces[1].has_closing_windows()
        {
            assert!(!self.workspaces[0].has_windows_or_name());
            assert!(!self.workspaces[1].has_windows_or_name());
            self.workspaces.remove(1);
        }
    }

    pub fn unname_workspace(&mut self, id: WorkspaceId) -> bool {
        let Some(idx) = self.idx_of_ws(id) else {
            return false;
        };
        let ws = &mut self.workspaces[idx];

        ws.unname();

        if self.workspace_switch.is_none() {
            self.clean_up_workspaces();
        }

        true
    }

    pub fn remove_workspace_by_idx(&mut self, mut idx: usize) -> Workspace<W> {
        if idx == self.workspaces.len() - 1 {
            self.add_workspace_bottom();
        }
        if self.options.layout.empty_workspace_above_first && idx == 0 {
            self.add_workspace_top();
            idx += 1;
        }

        let mut ws = self.workspaces.remove(idx);
        ws.set_output(None);

        self.clear_workspace_switch();
        self.clean_up_workspaces();

        ws
    }

    pub fn insert_workspace(&mut self, mut ws: Workspace<W>, mut idx: usize, activate: bool) {
        ws.set_core(self.core.clone());
        ws.set_output(Some(self.output.clone()));
        ws.update_config(self.options.clone());

        // Don't insert past the last empty workspace.
        if idx == self.workspaces.len() {
            idx -= 1;
        }
        if idx == 0 && self.options.layout.empty_workspace_above_first {
            // Insert a new empty workspace on top to prepare for insertion of new workspace.
            self.add_workspace_top();
            idx += 1;
        }

        let id = SpaceId(ws.id().get());
        self.workspaces.insert(idx, ws);
        self.core
            .borrow_mut()
            .attach_space(id, &self.output_name, idx);

        if activate {
            self.clear_workspace_switch();
            self.activate_workspace(idx);
        }

        self.clear_workspace_switch();
        self.clean_up_workspaces();
    }

    pub fn append_workspaces(&mut self, mut workspaces: Vec<Workspace<W>>) {
        if workspaces.is_empty() {
            return;
        }

        for ws in &mut workspaces {
            ws.set_core(self.core.clone());
            ws.set_output(Some(self.output.clone()));
            ws.update_config(self.options.clone());
        }

        // Push the workspaces from the removed monitor in the end, right before the
        // last, empty, workspace.
        let empty = self.workspaces.remove(self.workspaces.len() - 1);
        for workspace in workspaces {
            let index = self.workspaces.len();
            let id = SpaceId(workspace.id().get());
            self.workspaces.push(workspace);
            self.core
                .borrow_mut()
                .attach_space(id, &self.output_name, index);
        }
        self.workspaces.push(empty);

        // If empty_workspace_above_first is set and the first workspace is now no longer empty,
        // add a new empty workspace on top.
        if self.options.layout.empty_workspace_above_first
            && self.workspaces[0].has_windows_or_name()
        {
            self.add_workspace_top();
        }

        // FIXME: if we're adding workspaces to currently invisible positions
        // (outside the workspace switch), we don't need to cancel it.
        self.clear_workspace_switch();
        self.clean_up_workspaces();
    }

    pub fn move_down_or_to_workspace_down(&mut self) {
        if !self.active_workspace().move_down() {
            self.move_to_workspace_down(ActivateWindow::Smart);
        }
    }

    pub fn move_up_or_to_workspace_up(&mut self) {
        if !self.active_workspace().move_up() {
            self.move_to_workspace_up(ActivateWindow::Smart);
        }
    }

    pub fn focus_window_or_workspace_down(&mut self) {
        if !self.active_workspace().focus_down() {
            self.switch_workspace_down();
        }
    }

    pub fn focus_window_or_workspace_up(&mut self) {
        if !self.active_workspace().focus_up() {
            self.switch_workspace_up();
        }
    }

    pub fn move_to_workspace_up(&mut self, activate: ActivateWindow) {
        let new_idx = self.active_workspace_idx().saturating_sub(1);
        self.move_to_workspace(None, new_idx, activate);
    }

    pub fn move_to_workspace_down(&mut self, activate: ActivateWindow) {
        let new_idx = min(self.active_workspace_idx() + 1, self.workspaces.len() - 1);
        self.move_to_workspace(None, new_idx, activate);
    }

    pub fn move_to_workspace(
        &mut self,
        window: Option<&W::Id>,
        idx: usize,
        activate: ActivateWindow,
    ) {
        let source_workspace_idx = if let Some(window) = window {
            self.workspaces
                .iter()
                .position(|ws| ws.has_window(window))
                .unwrap()
        } else {
            self.active_workspace_idx()
        };
        let source_id = self.workspaces[source_workspace_idx].id();

        let new_idx = min(idx, self.workspaces.len() - 1);
        if new_idx == source_workspace_idx {
            return;
        }
        let new_id = self.workspaces[new_idx].id();

        let activate = activate.map_smart(|| {
            window.is_none_or(|win| self.active_window().map(|win| win.id()) == Some(win))
        });

        let workspace = &mut self.workspaces[source_workspace_idx];
        let Some(window) = window.or_else(|| workspace.active_window().map(|win| win.id())) else {
            return;
        };
        let window = window.clone();

        let mut old_render_pos = workspace
            .tiles_with_render_positions()
            .find_map(|(tile, offset, _visible)| (tile.window().id() == &window).then_some(offset))
            .unwrap();

        let transaction = Transaction::new();
        let removed = workspace.remove_tile(&window, transaction);

        // If the view is following the tile, match the animation.
        let config = if activate {
            self.options.animations.workspace_switch.0
        } else {
            self.options.animations.window_movement.0
        };

        self.add_tile(
            removed.tile,
            MonitorAddWindowTarget::Workspace {
                id: new_id,
                column_idx: None,
            },
            if activate {
                ActivateWindow::Yes
            } else {
                ActivateWindow::No
            },
            true,
            removed.width,
            removed.is_full_width,
            removed.is_floating,
            Some(config),
        );

        if self.workspace_switch.is_none() {
            self.clean_up_workspaces();
        }

        let new_idx = self.idx_of_ws(new_id).unwrap();

        // Animate vertical movement between workspaces.
        //
        // Recompute the source idx in case some workspace was removed during clean-up. If the
        // source workspace itself was removed, don't bother animating this since the removal is
        // instant anyway.
        if let Some(source_workspace_idx) = self.idx_of_ws(source_id) {
            old_render_pos.y +=
                self.workspace_size_with_gap(1.).h * (source_workspace_idx as f64 - new_idx as f64);
        }

        let (tile, new_render_pos) = self.workspaces[new_idx]
            .tiles_with_render_positions_mut(false)
            .find(|(tile, _)| tile.window().id() == &window)
            .unwrap();
        tile.animate_move_from_with_config(old_render_pos - new_render_pos, config);
        tile.set_anim_y_between_workspaces();
    }

    pub fn move_column_to_workspace_up(&mut self, activate: bool) {
        let new_idx = self.active_workspace_idx().saturating_sub(1);
        self.move_column_to_workspace(new_idx, activate);
    }

    pub fn move_column_to_workspace_down(&mut self, activate: bool) {
        let new_idx = min(self.active_workspace_idx() + 1, self.workspaces.len() - 1);
        self.move_column_to_workspace(new_idx, activate);
    }

    pub fn move_column_to_workspace(&mut self, idx: usize, activate: bool) {
        let source_workspace_idx = self.active_workspace_idx();

        let new_idx = min(idx, self.workspaces.len() - 1);
        if new_idx == source_workspace_idx {
            return;
        }

        let workspace = &mut self.workspaces[source_workspace_idx];
        if workspace.floating_is_active() {
            let activate = if activate {
                ActivateWindow::Smart
            } else {
                ActivateWindow::No
            };
            self.move_to_workspace(None, idx, activate);
            return;
        }

        let Some(id) = workspace.scrolling().active_column().map(Column::id) else {
            return;
        };
        let mut old_render_pos = workspace
            .scrolling()
            .columns_with_render_positions()
            .find_map(|(col, pos)| (col.id() == id).then_some(pos))
            .unwrap();

        let column = workspace.remove_active_column().unwrap();

        // Animate vertical movement between workspaces.
        old_render_pos.y +=
            self.workspace_size_with_gap(1.).h * (source_workspace_idx as f64 - new_idx as f64);

        // If the view is following the column, match the animation.
        let config = if activate {
            self.options.animations.workspace_switch.0
        } else {
            self.options.animations.window_movement.0
        };

        let new_id = self.workspaces[new_idx].id();
        self.add_column(new_idx, column, activate, Some(config));

        let new_idx = self.idx_of_ws(new_id).unwrap();
        let (column, new_render_pos) = self.workspaces[new_idx]
            .scrolling_mut()
            .columns_with_render_positions_mut()
            .find(|(col, _pos)| col.id() == id)
            .unwrap();
        column.animate_move_from_with_config(old_render_pos - new_render_pos, config);
        column.set_anim_y_between_workspaces();
    }

    pub fn switch_workspace_up(&mut self) {
        let mut new_idx = match &self.workspace_switch {
            // During a DnD scroll, select the prev apparent workspace.
            Some(WorkspaceSwitch::Gesture(_)) if self.workspace_gesture_is_dnd() => {
                let current = self.workspace_gesture_value();
                let new = current.ceil() - 1.;
                new.clamp(0., (self.workspaces.len() - 1) as f64) as usize
            }
            _ => self.active_workspace_idx().saturating_sub(1),
        };

        if !self.active_workspace().is_scratchpad() && self.workspaces[new_idx].is_scratchpad() {
            if new_idx > 0 {
                new_idx -= 1;
            } else {
                return;
            }
        }

        self.activate_workspace(new_idx);
    }

    pub fn switch_workspace_down(&mut self) {
        let mut new_idx = match &self.workspace_switch {
            // During a DnD scroll, select the next apparent workspace.
            Some(WorkspaceSwitch::Gesture(_)) if self.workspace_gesture_is_dnd() => {
                let current = self.workspace_gesture_value();
                let new = current.floor() + 1.;
                new.clamp(0., (self.workspaces.len() - 1) as f64) as usize
            }
            _ => min(self.active_workspace_idx() + 1, self.workspaces.len() - 1),
        };

        if !self.active_workspace().is_scratchpad() && self.workspaces[new_idx].is_scratchpad() {
            if new_idx + 1 < self.workspaces.len() {
                new_idx += 1;
            } else {
                return;
            }
        }

        self.activate_workspace(new_idx);
    }

    pub fn navigate_expo(&mut self, dx: isize, dy: isize) -> bool {
        let count = self.workspaces.len();
        if count <= 1 {
            return false;
        }

        let cols = match count {
            1 => 1,
            2 => 2,
            3..=4 => 2,
            5..=6 => 3,
            7..=9 => 3,
            _ => (count as f64).sqrt().ceil() as usize,
        };
        let rows = count.div_ceil(cols);

        let cur_idx = self.active_workspace_idx().min(count - 1);
        let cur_row = cur_idx / cols;
        let cur_col = cur_idx % cols;

        let new_row = (cur_row as isize + dy).clamp(0, (rows - 1) as isize) as usize;
        let new_col = (cur_col as isize + dx).clamp(0, (cols - 1) as isize) as usize;

        let mut new_idx = new_row * cols + new_col;
        if new_idx >= count {
            new_idx = count - 1;
        }

        if new_idx != cur_idx {
            self.activate_workspace(new_idx);
            true
        } else {
            false
        }
    }

    fn previous_workspace_idx(&self) -> Option<usize> {
        let id = self
            .core
            .borrow()
            .navigation()
            .previous_space(&self.output_name)?;
        self.idx_of_ws(WorkspaceId::specific(id.0))
    }

    pub fn switch_workspace(&mut self, idx: usize) {
        self.activate_workspace(min(idx, self.workspaces.len() - 1));
    }

    pub fn switch_workspace_auto_back_and_forth(&mut self, idx: usize) {
        let idx = min(idx, self.workspaces.len() - 1);

        if idx == self.active_workspace_idx() {
            if let Some(prev_idx) = self.previous_workspace_idx() {
                self.switch_workspace(prev_idx);
            }
        } else {
            self.switch_workspace(idx);
        }
    }

    pub fn switch_workspace_previous(&mut self) {
        if let Some(idx) = self.previous_workspace_idx() {
            self.switch_workspace(idx);
        }
    }

    pub fn active_window(&self) -> Option<&W> {
        self.active_workspace_ref().active_window()
    }

    pub fn advance_animations(&mut self) {
        self.advance_stage_animations();
        match &mut self.workspace_switch {
            Some(WorkspaceSwitch::Animation(anim)) => {
                if anim.is_done() {
                    self.clear_workspace_switch();
                    self.clean_up_workspaces();
                }
            }
            Some(WorkspaceSwitch::Gesture(gesture)) => {
                self.core.borrow_mut().workspace_switch_action(
                    realand_core::workspace_switch::WorkspaceSwitchAction::DndIdle {
                        connector: self.output_name.clone(),
                        timestamp: self.clock.now_unadjusted(),
                    },
                );
                if let Some(anim) = &mut gesture.animation {
                    if anim.is_done() {
                        gesture.animation = None;
                    }
                }
            }
            None => (),
        }

        for ws in &mut self.workspaces {
            ws.advance_animations();
        }
        let closing_ids: Vec<_> = self
            .workspaces
            .iter()
            .flat_map(Workspace::closing_windows)
            .map(|closing| closing.stable_id().clone())
            .collect();
        self.spread_closing.retain(|id, _| closing_ids.contains(id));
    }

    pub(super) fn are_animations_ongoing(&self) -> bool {
        self.stage_animations_ongoing()
            || self
                .workspace_switch
                .as_ref()
                .is_some_and(|s| s.is_animation_ongoing())
            || if self.is_spread_visible() {
                self.spread_render_tiles()
                    .any(|tile| tile.are_animations_ongoing())
                    || self.workspaces.iter().any(Workspace::has_closing_windows)
            } else {
                self.workspaces_with_render_geo()
                    .any(|(ws, _)| ws.are_animations_ongoing())
            }
    }

    pub fn are_transitions_ongoing(&self) -> bool {
        self.stage_animations_ongoing()
            || self
                .workspace_switch
                .as_ref()
                .is_some_and(|s| s.is_animation_ongoing())
            || if self.is_spread_visible() {
                self.spread_render_tiles()
                    .any(|tile| tile.are_transitions_ongoing())
                    || self.workspaces.iter().any(Workspace::has_closing_windows)
            } else {
                self.workspaces
                    .iter()
                    .any(|ws| ws.are_transitions_ongoing())
            }
    }

    pub fn update_render_elements(&mut self, is_active: bool) {
        if self.is_spread_visible() {
            for (ws, _) in self.workspaces_with_render_geo_mut(true) {
                ws.update_shadow_render_elements();
            }
            self.update_spread_render_elements();
            return;
        }
        let mut insert_hint_ws_geo = None;
        let insert_hint_ws_id = self
            .insert_hint
            .as_ref()
            .and_then(|hint| hint.workspace.existing_id());

        for ws in &mut self.workspaces {
            ws.update_render_elements(is_active, RenderLayer::MovingBetweenWorkspaces);
        }

        for (ws, geo) in self.workspaces_with_render_geo_mut(true) {
            ws.update_render_elements(is_active, RenderLayer::Normal);

            if Some(ws.id()) == insert_hint_ws_id {
                insert_hint_ws_geo = Some(geo);
            }
        }

        self.insert_hint_render_loc = None;
        if let Some(hint) = &self.insert_hint {
            match hint.workspace {
                InsertWorkspace::Existing(ws_id) => {
                    if let Some(idx) = self.idx_of_ws(ws_id) {
                        let ws = &self.workspaces[idx];
                        if let Some(mut area) = ws.insert_hint_area(hint.position) {
                            let scale = ws.scale().fractional_scale();
                            let view_size = ws.view_size();

                            // Make sure the hint is at least partially visible.
                            if matches!(hint.position, InsertPosition::NewColumn(_)) {
                                let geo = insert_hint_ws_geo.unwrap();
                                let zoom = self.workspace_render_zoom(geo);
                                let geo = geo.downscale(zoom);

                                area.loc.x = area.loc.x.max(-geo.loc.x - area.size.w / 2.);
                                area.loc.x =
                                    area.loc.x.min(geo.loc.x + geo.size.w - area.size.w / 2.);
                            }

                            // Round to physical pixels.
                            area = area.to_physical_precise_round(scale).to_logical(scale);

                            let view_rect = Rectangle::new(area.loc.upscale(-1.), view_size);
                            self.insert_hint_element.update_render_elements(
                                area.size,
                                view_rect,
                                hint.corner_radius,
                                scale,
                            );
                            self.insert_hint_render_loc = Some(InsertHintRenderLoc {
                                workspace: hint.workspace,
                                location: area.loc,
                            });
                        }
                    } else {
                        error!("insert hint workspace missing from monitor");
                    }
                }
                InsertWorkspace::NewAt(ws_idx) => {
                    // Stage has an explicit rail target for new workspaces. Its highlight
                    // is drawn by the rail, rather than the strip's between-workspace hint.
                    if self.is_stage_visible() {
                        return;
                    }
                    let scale = self.scale.fractional_scale();
                    let zoom = self.overview_zoom();
                    let gap = self.workspace_gap(zoom);

                    let hint_gap = round_logical_in_physical(scale, gap * 0.1);
                    let hint_height = gap - hint_gap * 2.;

                    let next_ws_geo = self.workspaces_render_geo().nth(ws_idx).unwrap();
                    let hint_width = round_logical_in_physical(scale, next_ws_geo.size.w * 0.75);
                    let hint_x =
                        round_logical_in_physical(scale, (next_ws_geo.size.w - hint_width) / 2.);

                    let hint_loc_diff = Point::from((-hint_x, hint_height + hint_gap));
                    let hint_loc = next_ws_geo.loc - hint_loc_diff;
                    let hint_size = Size::from((hint_width, hint_height));

                    // Sometimes the hint ends up 1 px wider than necessary and/or 1 px
                    // narrower than necessary. The values here seem correct. Might have to do with
                    // how zooming out currently doesn't round to output scale properly.

                    // Compute view rect as if we're above the next workspace (rather than below
                    // the previous one).
                    let view_rect = Rectangle::new(hint_loc_diff, next_ws_geo.size);

                    self.insert_hint_element.update_render_elements(
                        hint_size,
                        view_rect,
                        CornerRadius::default(),
                        scale,
                    );
                    self.insert_hint_render_loc = Some(InsertHintRenderLoc {
                        workspace: hint.workspace,
                        location: hint_loc,
                    });
                }
            }
        }
    }

    pub fn update_config(&mut self, base_options: Rc<Options>) {
        let options =
            Rc::new(Options::clone(&base_options).with_merged_layout(self.layout_config.as_ref()));

        if self.options.layout.empty_workspace_above_first
            != options.layout.empty_workspace_above_first
            && self.workspaces.len() > 1
        {
            if options.layout.empty_workspace_above_first {
                self.add_workspace_top();
            } else if self.workspace_switch.is_none() && self.active_workspace_idx() != 0 {
                self.workspaces.remove(0);
            }
        }

        for ws in &mut self.workspaces {
            ws.update_config(options.clone());
        }

        self.insert_hint_element
            .update_config(options.layout.insert_hint);
        self.cube_background.borrow_mut().update(
            self.view_size,
            options.animations.workspace_switch.1.background_color,
        );

        self.base_options = base_options;
        self.options = options;
    }

    pub fn update_layout_config(
        &mut self,
        layout_config: Option<realand_config::LayoutPart>,
    ) -> bool {
        if self.layout_config == layout_config {
            return false;
        }

        self.layout_config = layout_config;
        self.update_config(self.base_options.clone());

        true
    }

    pub fn update_shaders(&mut self) {
        for ws in &mut self.workspaces {
            ws.update_shaders();
        }

        self.insert_hint_element.update_shaders();
    }

    pub fn update_output_size(&mut self) {
        self.scale = self.output.current_scale();
        self.view_size = output_size(&self.output);
        self.working_area = compute_working_area(&self.output);
        self.cube_background.borrow_mut().update(
            self.view_size,
            self.options.animations.workspace_switch.1.background_color,
        );

        for ws in &mut self.workspaces {
            ws.update_output_size();
        }
    }

    /// Swap payload slots and apply the same stable-ID topology mutation in the core.
    fn swap_workspaces(&mut self, first: usize, second: usize) {
        let first_id = SpaceId(self.workspaces[first].id().get());
        let second_id = SpaceId(self.workspaces[second].id().get());
        self.workspaces.swap(first, second);
        let mut desktop = self.core.borrow_mut();
        desktop.reorder_space(first_id, second);
        desktop.reorder_space(second_id, first);
    }

    pub fn move_workspace_down(&mut self) {
        let old_idx = self.active_workspace_idx();
        let new_idx = min(old_idx + 1, self.workspaces.len() - 1);
        if new_idx == old_idx {
            return;
        }
        self.swap_workspaces(old_idx, new_idx);
        if new_idx == self.workspaces.len() - 1 {
            self.add_workspace_bottom();
        }
        if self.options.layout.empty_workspace_above_first && old_idx == 0 {
            self.add_workspace_top();
        }
        self.clear_workspace_switch();
        self.clean_up_workspaces();
    }

    pub fn move_workspace_up(&mut self) {
        let old_idx = self.active_workspace_idx();
        let new_idx = old_idx.saturating_sub(1);
        if new_idx == old_idx {
            return;
        }
        self.swap_workspaces(old_idx, new_idx);
        if old_idx == self.workspaces.len() - 1 {
            self.add_workspace_bottom();
        }
        if self.options.layout.empty_workspace_above_first && new_idx == 0 {
            self.add_workspace_top();
        }
        self.clear_workspace_switch();
        self.clean_up_workspaces();
    }

    pub fn move_workspace_to_idx(&mut self, old_idx: usize, new_idx: usize) {
        if self.workspaces.len() <= old_idx {
            return;
        }

        let new_idx = new_idx.clamp(0, self.workspaces.len() - 1);
        if old_idx == new_idx {
            return;
        }

        let ws = self.workspaces.remove(old_idx);
        let id = SpaceId(ws.id().get());
        self.workspaces.insert(new_idx, ws);
        self.core.borrow_mut().reorder_space(id, new_idx);

        if new_idx > old_idx {
            if new_idx == self.workspaces.len() - 1 {
                // Insert a new empty workspace.
                self.add_workspace_bottom();
            }

            if self.options.layout.empty_workspace_above_first && old_idx == 0 {
                self.add_workspace_top();
            }
        } else {
            if old_idx == self.workspaces.len() - 1 {
                // Insert a new empty workspace.
                self.add_workspace_bottom();
            }

            if self.options.layout.empty_workspace_above_first && new_idx == 0 {
                self.add_workspace_top();
            }
        }

        self.clear_workspace_switch();

        self.clean_up_workspaces();
    }

    /// Returns the geometry of the active window relative to and clamped to the output.
    ///
    /// During animations, assumes the final view position.
    pub fn active_window_visual_rectangle(&self) -> Option<Rectangle<f64, Logical>> {
        if self.is_overview_open() {
            return None;
        }

        self.active_workspace_ref().active_window_visual_rectangle()
    }

    fn workspace_size(&self, zoom: f64) -> Size<f64, Logical> {
        let ws_size = self.view_size.upscale(zoom);
        let scale = self.scale.fractional_scale();
        ws_size.to_physical_precise_ceil(scale).to_logical(scale)
    }

    fn workspace_gap(&self, zoom: f64) -> f64 {
        let scale = self.scale.fractional_scale();
        let gap = self.view_size.h * 0.1 * zoom;
        round_logical_in_physical_max1(scale, gap)
    }

    fn workspace_size_with_gap(&self, zoom: f64) -> Size<f64, Logical> {
        let gap = self.workspace_gap(zoom);
        self.workspace_size(zoom) + Size::from((0., gap))
    }

    pub(super) fn overview_mode(&self) -> OverviewMode {
        self.core.borrow().overview().mode()
    }

    pub(super) fn is_overview_open(&self) -> bool {
        self.core.borrow().overview().desired_open()
    }

    pub fn overview_zoom(&self) -> f64 {
        let progress = self.overview_progress.as_ref().map(|p| p.value());
        compute_overview_zoom(&self.options, progress)
    }

    /// Animation progress for the output-sized overview wallpaper shade.
    pub fn overview_backdrop_progress(&self) -> f64 {
        self.overview_progress
            .as_ref()
            .map_or(0., |progress| progress.clamped_value().clamp(0., 1.))
    }

    pub(super) fn set_overview_progress(&mut self, progress: Option<&super::OverviewProgress>) {
        self.set_spread_progress(progress);
        self.set_stage_progress(progress);
        if self.overview_mode() == OverviewMode::Expo && progress.is_some() {
            let target_progress = if self.is_overview_open() { 1. } else { 0. };
            let is_gesture = matches!(progress, Some(super::OverviewProgress::Gesture));
            let full_size_workspace = self.active_workspace_ref().id();
            let changed_direction = self.expo_transition.as_ref().is_none_or(|transition| {
                transition.target_progress != target_progress
                    || transition.is_gesture != is_gesture
                    || (!is_gesture
                        && target_progress == 0.
                        && transition.full_size_workspace != full_size_workspace)
            });
            if changed_direction {
                let (start_progress, start_rects) = if self.expo_transition.is_some() {
                    // The parent has already changed overview_open. Geometry still uses
                    // the previous transition until its visible origins are captured here.
                    let start_progress = self.overview_backdrop_progress();
                    let rects = self.workspaces_render_geo();
                    let origins = self
                        .workspaces
                        .iter()
                        .map(Workspace::id)
                        .zip(rects)
                        .collect();
                    (start_progress, Some(origins))
                } else {
                    (1. - target_progress, None)
                };
                self.expo_transition = Some(ExpoTransition {
                    start_progress,
                    target_progress,
                    is_gesture,
                    full_size_workspace,
                    start_rects,
                });
                *self.expo_geometry.borrow_mut() = None;
            }
            if matches!(progress, Some(super::OverviewProgress::Open)) {
                // A settled grid no longer needs captured reversal origins.
                if let Some(transition) = &mut self.expo_transition {
                    transition.start_progress = 0.;
                    transition.start_rects = None;
                }
            }
        } else {
            self.expo_transition = None;
            *self.expo_geometry.borrow_mut() = None;
        }
        let prev_render_idx = self.workspace_render_idx();
        let prev_velocity = self.workspace_render_velocity();
        let gesture_value = self.core.borrow().overview().gesture_value().unwrap_or(0.);
        self.overview_progress = progress.map(|value| match value {
            super::OverviewProgress::Animation(anim) => OverviewProgress::Animation(anim.clone()),
            super::OverviewProgress::Gesture => OverviewProgress::Value(gesture_value),
            super::OverviewProgress::Open => OverviewProgress::Value(1.),
        });
        let new_render_idx = self.workspace_render_idx();

        // If the view jumped (can happen when going from corrected to uncorrected render_idx, for
        // example when toggling the overview in the middle of an overview animation), then restart
        // the workspace switch to avoid jumps.
        if prev_render_idx != new_render_idx {
            if let Some(WorkspaceSwitch::Animation(anim)) = &mut self.workspace_switch {
                *anim = anim.restarted(prev_render_idx, anim.to(), prev_velocity);
            }
        }
    }

    #[cfg(test)]
    pub(super) fn overview_progress_value(&self) -> Option<f64> {
        self.overview_progress.as_ref().map(|p| p.value())
    }

    fn synchronized_workspace_switch(&self) -> Option<(&Animation, &Animation)> {
        let (Some(WorkspaceSwitch::Animation(switch)), Some(OverviewProgress::Animation(overview))) =
            (&self.workspace_switch, &self.overview_progress)
        else {
            return None;
        };
        (switch.start_time() == overview.start_time()
            && (switch.duration().as_secs_f64() - overview.duration().as_secs_f64()).abs() <= 0.001)
            .then_some((switch, overview))
    }

    pub fn workspace_render_idx(&self) -> f64 {
        if let Some((switch, overview)) = self.synchronized_workspace_switch() {
            // Keep the destination workspace moving monotonically while zooming:
            // its apparent distance is (to - value) * the initial workspace height.
            // Use continuous zoom here; rounding intermediate heights introduces
            // one-pixel steps into the animation's coordinate system.
            let from_zoom = compute_overview_zoom(&self.options, Some(overview.from()));
            let zoom = self.overview_zoom();
            return switch.to() + (switch.value() - switch.to()) * from_zoom / zoom;
        }

        self.workspace_switch_idx()
    }

    fn workspace_render_velocity(&self) -> f64 {
        if let Some((switch, overview)) = self.synchronized_workspace_switch() {
            let from_zoom = compute_overview_zoom(&self.options, Some(overview.from()));
            let zoom = self.overview_zoom();
            let target_zoom = self.options.overview.zoom.clamp(0.0001, 0.75);
            let zoom_velocity = if zoom > 0.0001 {
                -(1. - target_zoom) * overview.velocity()
            } else {
                0.
            };
            // Differentiate the corrected position, including the changing zoom.
            // Passing the raw switch velocity would cause a speed discontinuity
            // when a new action removes this synchronized correction.
            return from_zoom / zoom
                * (switch.velocity() - (switch.value() - switch.to()) * zoom_velocity / zoom);
        }
        match &self.workspace_switch {
            Some(WorkspaceSwitch::Animation(anim)) => anim.velocity(),
            Some(WorkspaceSwitch::Gesture(gesture)) => {
                let zoom = self.overview_zoom();
                let height = self.workspace_size_with_gap(1.).h;
                self.core
                    .borrow()
                    .workspace_switch()
                    .gesture(&self.output_name)
                    .map_or(0., |owner| {
                        owner.velocity(zoom, height, self.workspaces.len())
                    })
                    + gesture.animation.as_ref().map_or(0., Animation::velocity)
            }
            None => 0.,
        }
    }

    pub fn workspaces_render_geo(&self) -> Box<dyn Iterator<Item = Rectangle<f64, Logical>>> {
        if self.is_stage_visible() {
            let rects = self.stage_workspace_geometries();
            return Box::new((0..rects.len()).map(move |idx| rects[idx]));
        }
        let scale = self.scale.fractional_scale();

        if self.overview_mode() == OverviewMode::Expo
            && (self.is_overview_open() || self.overview_progress.is_some())
        {
            let p = self.overview_backdrop_progress();
            let transition = self.expo_transition.as_ref();
            let full_size_workspace = transition.map_or_else(
                || self.active_workspace_ref().id(),
                |t| t.full_size_workspace,
            );
            let anchor_idx = self.idx_of_ws(full_size_workspace);
            let start_progress = transition.map_or(0., |t| t.start_progress);
            let target_progress = transition.map_or(1., |t| {
                if t.is_gesture {
                    // overview_open stays true throughout a gesture. Its geometry
                    // must still follow closing movement below the captured point.
                    if p < start_progress {
                        0.
                    } else {
                        1.
                    }
                } else {
                    t.target_progress
                }
            });
            let fraction = if start_progress == target_progress {
                1.
            } else {
                (p - start_progress) / (target_progress - start_progress)
            };
            let key = [
                self.view_size.w.to_bits(),
                self.view_size.h.to_bits(),
                scale.to_bits(),
                p.to_bits(),
                self.workspaces.len() as u64,
                anchor_idx.map_or(u64::MAX, |idx| idx as u64),
                target_progress.to_bits(),
            ];
            if let Some((cached_key, workspace_ids, rects)) = &*self.expo_geometry.borrow() {
                if *cached_key == key
                    && workspace_ids
                        .iter()
                        .copied()
                        .eq(self.workspaces.iter().map(Workspace::id))
                {
                    let rects = rects.clone();
                    return Box::new((0..rects.len()).map(move |idx| rects[idx]));
                }
            }
            let count = self.workspaces.len().max(1);

            let cols = match count {
                1 => 1,
                2 => 2,
                3..=4 => 2,
                5..=6 => 3,
                7..=9 => 3,
                _ => (count as f64).sqrt().ceil() as usize,
            };
            let rows = count.div_ceil(cols);

            let view_w = self.view_size.w;
            let view_h = self.view_size.h;
            let aspect = (view_w / view_h.max(1.0)).max(0.1);

            let gap = 28.0;
            let w_avail = (view_w - gap * (cols + 1) as f64).max(10.0);
            let h_avail = (view_h - gap * (rows + 1) as f64).max(10.0);

            let cell_w_by_w = w_avail / cols as f64;
            let cell_w_by_h = (h_avail / rows as f64) * aspect;
            let cell_w = f64::min(cell_w_by_w, cell_w_by_h).max(10.0);
            let cell_h = cell_w / aspect;

            let grid_w = cols as f64 * cell_w + (cols - 1) as f64 * gap;
            let grid_h = rows as f64 * cell_h + (rows - 1) as f64 * gap;
            let origin_x = (view_w - grid_w) / 2.0;
            let origin_y = (view_h - grid_h) / 2.0;

            let mut rects = Vec::with_capacity(count + 1);
            for idx in 0..=self.workspaces.len() {
                let col = idx % cols;
                let row = idx / cols;

                let target_x = origin_x + col as f64 * (cell_w + gap);
                let target_y = origin_y + row as f64 * (cell_h + gap);
                let target_loc = Point::<f64, Logical>::from((target_x, target_y));
                let target_size = Size::<f64, Logical>::from((cell_w, cell_h));

                let full_size = Rectangle::from_size(self.view_size);
                let grid = Rectangle::new(target_loc, target_size);
                let collapsed = Rectangle::new(
                    target_loc + target_size.to_point().downscale(2.),
                    Size::from((0., 0.)),
                );
                let workspace_id = self.workspaces.get(idx).map(Workspace::id);
                let origin = if Some(idx) == anchor_idx {
                    full_size
                } else {
                    collapsed
                };
                let start = transition
                    .and_then(|t| t.start_rects.as_ref())
                    .and_then(|origins| origins.get(&workspace_id?))
                    .copied()
                    .unwrap_or_else(|| {
                        interpolate_expo_rect(origin, grid, start_progress, self.view_size)
                    });
                let target = if target_progress == 1. { grid } else { origin };
                let rect = interpolate_expo_rect(start, target, fraction, self.view_size);

                let loc = rect.loc.to_physical_precise_round(scale).to_logical(scale);
                rects.push(Rectangle::new(loc, rect.size));
            }

            let rects = Rc::new(rects);
            *self.expo_geometry.borrow_mut() = Some((
                key,
                self.workspaces.iter().map(Workspace::id).collect(),
                rects.clone(),
            ));
            return Box::new((0..rects.len()).map(move |idx| rects[idx]));
        }

        let zoom = self.overview_zoom();

        let ws_size = self.workspace_size(zoom);
        let gap = self.workspace_gap(zoom);
        let ws_height_with_gap = ws_size.h + gap;

        let static_offset = (self.view_size.to_point() - ws_size.to_point()).downscale(2.);
        let static_offset = static_offset
            .to_physical_precise_round(scale)
            .to_logical(scale);

        let first_ws_y = -self.workspace_render_idx() * ws_height_with_gap;
        let first_ws_y = round_logical_in_physical(scale, first_ws_y);

        // Return position for one-past-last workspace too.
        Box::new((0..=self.workspaces.len()).map(move |idx| {
            let y = first_ws_y + idx as f64 * ws_height_with_gap;
            let loc = Point::from((0., y)) + static_offset;

            // Even though all components that go into loc are rounded to physical pixels, the
            // floating point addition may lose precision. This can result for example in the
            // current workspace having y = 0.0000000000002 and thus missing pointer hits at the
            // monitor edge with y = 0. So, post-round the location too.
            let loc = loc.to_physical_precise_round(scale).to_logical(scale);

            Rectangle::new(loc, ws_size)
        }))
    }

    pub fn workspaces_with_render_geo_cull(
        &self,
        cull: bool,
    ) -> impl Iterator<Item = (&Workspace<W>, Rectangle<f64, Logical>)> {
        let output_geo = Rectangle::from_size(self.view_size);

        let geo: Vec<_> = self.workspaces_render_geo().collect();
        let indices = if self.is_stage_visible() {
            self.stage_front_order()
        } else {
            (0..self.workspaces.len()).collect()
        };
        indices
            .into_iter()
            .map(move |idx| (&self.workspaces[idx], geo[idx]))
            // Cull out workspaces outside the output.
            .filter(move |(_ws, geo)| {
                geo.size.w > 0.
                    && geo.size.h > 0.
                    && (!cull || geo.intersection(output_geo).is_some())
            })
    }

    pub fn workspaces_with_render_geo(
        &self,
    ) -> impl Iterator<Item = (&Workspace<W>, Rectangle<f64, Logical>)> {
        self.workspaces_with_render_geo_cull(true)
    }

    pub fn workspaces_with_render_geo_idx(
        &self,
    ) -> impl Iterator<Item = ((usize, &Workspace<W>), Rectangle<f64, Logical>)> {
        let output_geo = Rectangle::from_size(self.view_size);

        let geo = self.workspaces_render_geo();
        zip(self.workspaces.iter().enumerate(), geo)
            // Cull out workspaces outside the output.
            .filter(move |(_ws, geo)| geo.intersection(output_geo).is_some())
    }

    pub fn workspaces_with_render_geo_mut(
        &mut self,
        cull: bool,
    ) -> impl Iterator<Item = (&mut Workspace<W>, Rectangle<f64, Logical>)> {
        let output_geo = Rectangle::from_size(self.view_size);

        let geo = self.workspaces_render_geo();
        zip(self.workspaces.iter_mut(), geo)
            // Cull out workspaces outside the output.
            .filter(move |(_ws, geo)| !cull || geo.intersection(output_geo).is_some())
    }

    /// Scale shared by thumbnail rendering, hit testing, and drag-and-drop.
    pub(crate) fn workspace_render_zoom(&self, geo: Rectangle<f64, Logical>) -> f64 {
        if self.is_stage_visible() {
            return (geo.size.w / self.view_size.w.max(f64::EPSILON)).max(f64::EPSILON);
        }
        if self.overview_mode() == OverviewMode::Expo
            && (self.is_overview_open() || self.overview_progress.is_some())
        {
            (geo.size.w / self.view_size.w.max(1.)).clamp(0.01, 1.)
        } else {
            self.overview_zoom()
        }
    }

    pub fn workspace_under(
        &self,
        pos_within_output: Point<f64, Logical>,
    ) -> Option<(&Workspace<W>, Rectangle<f64, Logical>)> {
        let is_expo = self.is_stage_visible()
            || self.overview_mode() == OverviewMode::Expo
                && (self.is_overview_open() || self.overview_progress.is_some());

        let (ws, geo) = self.workspaces_with_render_geo().find_map(|(ws, geo)| {
            if is_expo {
                // In 2D Expo grid, match exact cell bounds instead of extending width across entire screen
                geo.contains(pos_within_output).then_some((ws, geo))
            } else {
                // Original 1D strip overview: extend width to entire output.
                let loc = Point::from((0., geo.loc.y));
                let size = Size::from((self.view_size.w, geo.size.h));
                let bounds = Rectangle::new(loc, size);

                bounds.contains(pos_within_output).then_some((ws, geo))
            }
        })?;
        Some((ws, geo))
    }

    pub fn workspace_under_narrow(
        &self,
        pos_within_output: Point<f64, Logical>,
    ) -> Option<&Workspace<W>> {
        self.workspaces_with_render_geo()
            .find_map(|(ws, geo)| geo.contains(pos_within_output).then_some(ws))
    }

    pub fn window_under(&self, pos_within_output: Point<f64, Logical>) -> Option<(&W, HitType)> {
        if self.is_spread_visible() {
            return self
                .spread_draw_tiles()
                .into_iter()
                .find_map(|(tile, rect, _)| {
                    rect.contains(pos_within_output).then_some((
                        tile.window(),
                        HitType::Activate {
                            is_tab_indicator: false,
                        },
                    ))
                });
        }
        let (ws, geo) = self.workspace_under(pos_within_output)?;

        if self.overview_progress.is_some() {
            let zoom = self.workspace_render_zoom(geo);
            let pos_within_workspace = (pos_within_output - geo.loc).downscale(zoom);
            let (win, hit) = ws.window_under(pos_within_workspace)?;
            // During the overview animation, we cannot do input hits because we cannot really
            // represent scaled windows properly.
            Some((win, hit.to_activate()))
        } else {
            let (win, hit) = ws.window_under(pos_within_output - geo.loc)?;
            Some((win, hit.offset_win_pos(geo.loc)))
        }
    }

    pub fn resize_edges_under(&self, pos_within_output: Point<f64, Logical>) -> Option<ResizeEdge> {
        if self.overview_progress.is_some() {
            return None;
        }

        let (ws, geo) = self.workspace_under(pos_within_output)?;
        ws.resize_edges_under(pos_within_output - geo.loc)
    }

    pub(super) fn insert_position(
        &self,
        pos_within_output: Point<f64, Logical>,
    ) -> (InsertWorkspace, Rectangle<f64, Logical>) {
        if self.is_stage_visible() {
            return self
                .stage_drop_target(pos_within_output)
                .unwrap_or_else(|| {
                    let selected = self.stage_selected();
                    let geo = self.stage_workspace_geometries()[self.idx_of_ws(selected).unwrap()];
                    (InsertWorkspace::Existing(selected), geo)
                });
        }
        // In Expo, every card is a drop target. Gaps select the nearest card,
        // rather than inserting workspaces using the vertical strip's Y-only rules.
        if self.overview_mode() == OverviewMode::Expo && self.is_overview_open() {
            let (ws, geo) = self
                .workspaces_with_render_geo()
                .min_by(|(_, a), (_, b)| {
                    let distance = |geo: Rectangle<f64, Logical>| {
                        let x = pos_within_output.x.clamp(geo.loc.x, geo.loc.x + geo.size.w);
                        let y = pos_within_output.y.clamp(geo.loc.y, geo.loc.y + geo.size.h);
                        (pos_within_output.x - x).powi(2) + (pos_within_output.y - y).powi(2)
                    };
                    distance(*a).total_cmp(&distance(*b))
                })
                .unwrap();
            return (InsertWorkspace::Existing(ws.id()), geo);
        }

        let mut iter = self.workspaces_with_render_geo_idx();

        let dummy = Rectangle::default();

        // Monitors always have at least one workspace.
        let ((idx, ws), geo) = iter.next().unwrap();

        // Check if above first.
        if pos_within_output.y < geo.loc.y {
            return (InsertWorkspace::NewAt(idx), dummy);
        }

        let contains = move |geo: Rectangle<f64, Logical>| {
            geo.loc.y <= pos_within_output.y && pos_within_output.y < geo.loc.y + geo.size.h
        };

        // Check first.
        if contains(geo) {
            return (InsertWorkspace::Existing(ws.id()), geo);
        }

        let mut last_geo = geo;
        let mut last_idx = idx;
        for ((idx, ws), geo) in iter {
            // Check gap above.
            let gap_loc = Point::from((last_geo.loc.x, last_geo.loc.y + last_geo.size.h));
            let gap_size = Size::from((geo.size.w, geo.loc.y - gap_loc.y));
            let gap_geo = Rectangle::new(gap_loc, gap_size);
            if contains(gap_geo) {
                return (InsertWorkspace::NewAt(idx), dummy);
            }

            // Check workspace itself.
            if contains(geo) {
                return (InsertWorkspace::Existing(ws.id()), geo);
            }

            last_geo = geo;
            last_idx = idx;
        }

        // Anything below.
        (InsertWorkspace::NewAt(last_idx + 1), dummy)
    }

    pub fn render_above_top_layer(&self) -> bool {
        // Render above the top layer only if the view is stationary.
        if self.workspace_switch.is_some() || self.overview_progress.is_some() {
            return false;
        }

        let ws = &self.workspaces[self.active_workspace_idx()];
        ws.render_above_top_layer()
    }

    pub fn render_insert_hint_between_workspaces<R: RealandRenderer>(
        &self,
        renderer: &mut R,
        push: &mut dyn FnMut(MonitorRenderElement<R>),
    ) {
        if self.options.layout.insert_hint.off {
            return;
        }
        let Some(render_loc) = self.insert_hint_render_loc else {
            return;
        };
        let InsertWorkspace::NewAt(_) = render_loc.workspace else {
            return;
        };

        self.insert_hint_element
            .render(renderer, render_loc.location, &mut |elem| {
                let elem = MonitorInnerRenderElement::UncroppedInsertHint(elem);
                let elem = RescaleRenderElement::from_element(elem, Point::default(), 1.);
                let elem =
                    RelocateRenderElement::from_element(elem, Point::default(), Relocate::Relative);
                push(elem);
            });
    }

    fn render_cube_face(
        &self,
        mut ctx: RenderCtx<GlesRenderer>,
        workspace_idx: usize,
        slot: usize,
        angle: f64,
        focus_ring: bool,
    ) -> anyhow::Result<Option<MeshRenderElement>> {
        let Some((area, vertices)) = face_mesh(
            self.view_size,
            angle,
            self.options.animations.workspace_switch.1.perspective,
        ) else {
            return Ok(None);
        };

        let scale = self.scale.fractional_scale();
        let crop_bounds = Rectangle::from_size(self.view_size).to_physical_precise_round(scale);
        let xray_pos = XrayPos::new(Point::default(), 1.);
        let workspace = &self.workspaces[workspace_idx];
        let mut elements = self.cube_elements[slot].borrow_mut();
        elements.clear();

        for pass in 0..4 {
            let mut push_workspace = |element| {
                if let Some(element) = CropRenderElement::from_element(element, scale, crop_bounds)
                {
                    elements.push(CubeFaceRenderElement::Workspace(element));
                }
            };
            match pass {
                0 => workspace.render_floating(
                    ctx.r(),
                    xray_pos,
                    focus_ring,
                    RenderLayer::MovingBetweenWorkspaces,
                    &mut push_workspace,
                ),
                1 => workspace.render_floating(
                    ctx.r(),
                    xray_pos,
                    focus_ring,
                    RenderLayer::Normal,
                    &mut push_workspace,
                ),
                2 => workspace.render_scrolling(
                    ctx.r(),
                    xray_pos,
                    focus_ring,
                    RenderLayer::MovingBetweenWorkspaces,
                    &mut push_workspace,
                ),
                _ => workspace.render_scrolling(
                    ctx.r(),
                    xray_pos,
                    focus_ring,
                    RenderLayer::Normal,
                    &mut push_workspace,
                ),
            }
        }

        elements.push(CubeFaceRenderElement::SolidColor(
            workspace.render_background(),
        ));

        let result = self.cube_buffers[slot].render(
            ctx.renderer,
            self.scale.fractional_scale().into(),
            &elements,
        );
        elements.clear();
        drop(elements);
        let (texture, _sync, mut data) = result?;

        let state = CubeFaceState {
            workspace_idx,
            source_commit: texture.current_commit(),
            source_texture: u64::from(texture.texture().tex_id()),
            angle_bits: angle.to_bits(),
            perspective_bits: self
                .options
                .animations
                .workspace_switch
                .1
                .perspective
                .to_bits(),
            width_bits: self.view_size.w.to_bits(),
            height_bits: self.view_size.h.to_bits(),
            scale_bits: scale.to_bits(),
            focus_ring,
        };
        let changed = {
            let mut states = self.cube_face_states.borrow_mut();
            let changed = states[slot] != Some(state);
            states[slot] = Some(state);
            changed
        };
        let commit = {
            let mut commits = self.cube_commits.borrow_mut();
            if changed {
                commits[slot].increment();
            }
            commits[slot]
        };
        let element = MeshRenderElement::new(
            self.cube_ids[slot].clone(),
            commit,
            texture.texture().clone(),
            texture.src().size,
            area,
            vertices,
            1.,
            Kind::Unspecified,
        );

        data.id = element.id().clone();
        for window in workspace.windows() {
            window.set_offscreen_data(Some(data.clone()));
        }

        Ok(Some(element))
    }

    fn render_cube_workspaces(
        &self,
        mut ctx: RenderCtx<GlesRenderer>,
        focus_ring: bool,
    ) -> anyhow::Result<[Option<MeshRenderElement>; 2]> {
        let current = self
            .workspace_switch_idx()
            .clamp(0., (self.workspaces.len() - 1) as f64);
        let low = current.floor() as usize;
        let fraction = current - low as f64;

        let low_face = (low, -fraction * FRAC_PI_2);
        let high_face = (low + 1 < self.workspaces.len() && fraction > f64::EPSILON)
            .then_some((low + 1, (1. - fraction) * FRAC_PI_2));
        let faces = match high_face {
            Some(high_face) if high_face.1.abs() < low_face.1.abs() => {
                [Some(high_face), Some(low_face)]
            }
            Some(high_face) => [Some(low_face), Some(high_face)],
            None => [Some(low_face), None],
        };

        self.update_cube_visible_workspaces(
            faces.map(|face| face.map(|(workspace_idx, _)| self.workspaces[workspace_idx].id())),
        );

        let mut meshes = [None, None];
        for (slot, face) in faces.into_iter().enumerate() {
            let Some((workspace_idx, angle)) = face else {
                continue;
            };
            meshes[slot] = self.render_cube_face(
                ctx.r(),
                workspace_idx,
                slot,
                angle,
                focus_ring && workspace_idx == self.active_workspace_idx(),
            )?;
        }
        Ok(meshes)
    }

    fn render_cube_face_native(
        &self,
        mut ctx: RenderCtx<NativeRenderer>,
        workspace_idx: usize,
        slot: usize,
        angle: f64,
        focus_ring: bool,
    ) -> anyhow::Result<Option<MeshRenderElement>> {
        let Some((area, vertices)) = face_mesh(
            self.view_size,
            angle,
            self.options.animations.workspace_switch.1.perspective,
        ) else {
            return Ok(None);
        };

        let scale = self.scale.fractional_scale();
        let crop_bounds = Rectangle::from_size(self.view_size).to_physical_precise_round(scale);
        let xray_pos = XrayPos::new(Point::default(), 1.);
        let workspace = &self.workspaces[workspace_idx];
        let mut elements = Vec::new();

        for pass in 0..4 {
            let mut push_workspace = |element| {
                if let Some(element) = CropRenderElement::from_element(element, scale, crop_bounds)
                {
                    elements.push(CubeFaceRenderElement::Workspace(element));
                }
            };
            match pass {
                0 => workspace.render_floating(
                    ctx.r(),
                    xray_pos,
                    focus_ring,
                    RenderLayer::MovingBetweenWorkspaces,
                    &mut push_workspace,
                ),
                1 => workspace.render_floating(
                    ctx.r(),
                    xray_pos,
                    focus_ring,
                    RenderLayer::Normal,
                    &mut push_workspace,
                ),
                2 => workspace.render_scrolling(
                    ctx.r(),
                    xray_pos,
                    focus_ring,
                    RenderLayer::MovingBetweenWorkspaces,
                    &mut push_workspace,
                ),
                _ => workspace.render_scrolling(
                    ctx.r(),
                    xray_pos,
                    focus_ring,
                    RenderLayer::Normal,
                    &mut push_workspace,
                ),
            }
        }

        elements.push(CubeFaceRenderElement::SolidColor(
            workspace.render_background(),
        ));

        let (texture, _sync, geo, states, source_commit) =
            self.cube_native_buffers[slot].render(ctx.renderer, scale.into(), &elements)?;
        // Native submissions share a queue; layout barriers order each
        // incremental re-render against the previous frame's sampling. No CPU
        // fence wait here, and steady frames redraw nothing at all.

        let state = CubeFaceState {
            workspace_idx,
            source_commit,
            source_texture: texture.cache_key(),
            angle_bits: angle.to_bits(),
            perspective_bits: self
                .options
                .animations
                .workspace_switch
                .1
                .perspective
                .to_bits(),
            width_bits: self.view_size.w.to_bits(),
            height_bits: self.view_size.h.to_bits(),
            scale_bits: scale.to_bits(),
            focus_ring,
        };
        let changed = {
            let mut states = self.cube_face_states.borrow_mut();
            let changed = states[slot] != Some(state);
            states[slot] = Some(state);
            changed
        };
        let commit = {
            let mut commits = self.cube_commits.borrow_mut();
            if changed {
                commits[slot].increment();
            }
            commits[slot]
        };
        let used = smithay::utils::Size::from((f64::from(geo.size.w), f64::from(geo.size.h)));
        let element = MeshRenderElement::new_vulkan(
            self.cube_ids[slot].clone(),
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
        for window in workspace.windows() {
            window.set_offscreen_data(Some(data.clone()));
        }

        Ok(Some(element))
    }

    fn render_cube_workspaces_native(
        &self,
        mut ctx: RenderCtx<NativeRenderer>,
        focus_ring: bool,
    ) -> anyhow::Result<[Option<MeshRenderElement>; 2]> {
        let current = self
            .workspace_switch_idx()
            .clamp(0., (self.workspaces.len() - 1) as f64);
        let low = current.floor() as usize;
        let fraction = current - low as f64;

        let low_face = (low, -fraction * FRAC_PI_2);
        let high_face = (low + 1 < self.workspaces.len() && fraction > f64::EPSILON)
            .then_some((low + 1, (1. - fraction) * FRAC_PI_2));
        let faces = match high_face {
            Some(high_face) if high_face.1.abs() < low_face.1.abs() => {
                [Some(high_face), Some(low_face)]
            }
            Some(high_face) => [Some(low_face), Some(high_face)],
            None => [Some(low_face), None],
        };

        self.update_cube_visible_workspaces(
            faces.map(|face| face.map(|(workspace_idx, _)| self.workspaces[workspace_idx].id())),
        );

        let mut meshes = [None, None];
        for (slot, face) in faces.into_iter().enumerate() {
            let Some((workspace_idx, angle)) = face else {
                continue;
            };
            meshes[slot] = self.render_cube_face_native(
                ctx.r(),
                workspace_idx,
                slot,
                angle,
                focus_ring && workspace_idx == self.active_workspace_idx(),
            )?;
        }
        Ok(meshes)
    }

    fn update_cube_visible_workspaces(&self, visible: [Option<WorkspaceId>; 2]) {
        let previous = std::mem::replace(&mut *self.cube_visible_workspaces.borrow_mut(), visible);
        for workspace_id in previous.into_iter().flatten() {
            if visible.contains(&Some(workspace_id)) {
                continue;
            }
            if let Some(workspace) = self
                .workspaces
                .iter()
                .find(|workspace| workspace.id() == workspace_id)
            {
                for window in workspace.windows() {
                    window.set_offscreen_data(None);
                }
            }
        }

        if visible.iter().all(Option::is_none) && previous.iter().any(Option::is_some) {
            for buffer in &self.cube_buffers {
                buffer.clear();
            }
            for buffer in &self.cube_native_buffers {
                buffer.clear();
            }
            *self.cube_face_states.borrow_mut() = [None, None];
        }
    }

    pub fn render_workspaces<R: RealandRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        focus_ring: bool,
        push: &mut dyn FnMut(MonitorRenderElement<R>),
    ) {
        let _span = realand_tracy_span!("Monitor::render_workspaces");
        if self.is_stage_visible() {
            self.update_cube_visible_workspaces([None, None]);
            self.render_stage(ctx, focus_ring, push);
            return;
        }
        if self.is_spread_visible() {
            self.render_spread(ctx, push);
            return;
        }

        let cube_enabled = self.workspace_switch.is_some()
            && self.overview_progress.is_none()
            && !self.is_overview_open()
            && !self.options.animations.off
            && !self.options.animations.workspace_switch.1.off;
        if cube_enabled {
            let result = {
                if let Some(gles_ctx) = ctx.as_gles() {
                    if MeshRenderElement::is_available(gles_ctx.renderer) {
                        Some(self.render_cube_workspaces(gles_ctx, focus_ring))
                    } else {
                        None
                    }
                } else {
                    ctx.as_native().map(|native_ctx| {
                        self.render_cube_workspaces_native(native_ctx, focus_ring)
                    })
                }
            };
            match result {
                None => {}
                Some(Ok(meshes)) => {
                    for mesh in meshes.into_iter().flatten() {
                        let inner = MonitorInnerRenderElement::<R>::Mesh(mesh);
                        let element =
                            RescaleRenderElement::from_element(inner, Point::default(), 1.);
                        push(RelocateRenderElement::from_element(
                            element,
                            Point::default(),
                            Relocate::Relative,
                        ));
                    }

                    let background = self.cube_background.borrow();
                    let inner = MonitorInnerRenderElement::<R>::SolidColor(
                        SolidColorRenderElement::from_buffer(
                            &background,
                            Point::default(),
                            1.,
                            Kind::Unspecified,
                        ),
                    );
                    let element = RescaleRenderElement::from_element(inner, Point::default(), 1.);
                    push(RelocateRenderElement::from_element(
                        element,
                        Point::default(),
                        Relocate::Relative,
                    ));
                    return;
                }
                Some(Err(err)) => {
                    warn!("error rendering workspace cube; falling back to 2D: {err:?}")
                }
            }
        }

        self.update_cube_visible_workspaces([None, None]);

        let scale = self.scale.fractional_scale();
        // Ceil the height in physical pixels.
        let height = (self.view_size.h * scale).ceil() as i32;

        let zoom = self.overview_zoom();

        let insert_hint_render_loc = self
            .insert_hint_render_loc
            .filter(|_| !self.options.layout.insert_hint.off);

        let is_expo = self.overview_mode() == OverviewMode::Expo
            && (self.is_overview_open() || self.overview_progress.is_some());
        let view_w = self.view_size.w.max(1.0);

        let scale_relocate = move |geo: Rectangle<f64, Logical>, elem| {
            let actual_zoom = if is_expo {
                (geo.size.w / view_w).clamp(0.01, 1.0)
            } else {
                zoom
            };
            let elem = RescaleRenderElement::from_element(elem, Point::from((0, 0)), actual_zoom);
            RelocateRenderElement::from_element(
                elem,
                // The offset we get from workspaces_with_render_geo() is already
                // rounded to physical pixels, but it's in the logical coordinate
                // space, so we need to convert it to physical.
                geo.loc.to_physical_precise_round(scale),
                Relocate::Relative,
            )
        };

        // Draw in passes for correct Z ordering during window movement between workspaces:
        // - floating windows moving between workspaces
        // - normal floating windows
        // - scrolling windows moving between workspaces
        // - normal scrolling windows
        for pass in 0..4 {
            // Don't cull when drawing windows moving between workspaces so that windows moving to
            // workspaces off-screen will still render.
            let cull = matches!(pass, 1 | 3);

            // Crop the elements to prevent them overflowing, currently visible during a workspace
            // switch.
            //
            // HACK: crop to infinite bounds at least horizontally where we
            // know there's no workspace joining or monitor bounds, otherwise
            // it will cut pixel shaders and mess up the coordinate space.
            // There's also a damage tracking bug which causes glitched
            // rendering for maximized GTK windows.
            //
            // FIXME: use proper bounds after fixing the Crop element.
            //
            // Also, check cull here to avoid cropping windows moving between workspaces.
            //
            // FIXME: for cull=true, it might be better visually to crop to a workspace-high region
            // anchored to the window/column as it moves between workspaces, to prevent overflowing
            // windows from appearing and disappearing.
            let crop_bounds = if is_expo {
                Rectangle::from_size(self.view_size.to_physical_precise_round(scale))
            } else if cull && (self.workspace_switch.is_some() || self.overview_progress.is_some())
            {
                Rectangle::new(
                    Point::from((-i32::MAX / 2, 0)),
                    Size::from((i32::MAX, height)),
                )
            } else {
                Rectangle::new(
                    Point::from((-i32::MAX / 2, -i32::MAX / 2)),
                    Size::from((i32::MAX, i32::MAX)),
                )
            };

            for (ws, geo) in self.workspaces_with_render_geo_cull(cull) {
                // Macro instead of closure because ws and insert hint have different elem types.
                macro_rules! push {
                    () => {{
                        &mut |elem| {
                            let elem = CropRenderElement::from_element(elem, scale, crop_bounds);
                            if let Some(elem) = elem {
                                let elem = MonitorInnerRenderElement::from(elem);
                                push(scale_relocate(geo, elem));
                            }
                        }
                    }};
                }

                let actual_zoom = if is_expo {
                    (geo.size.w / view_w).clamp(0.01, 1.0)
                } else {
                    zoom
                };
                let xray_pos = XrayPos::new(geo.loc, actual_zoom);

                match pass {
                    0 => {
                        ws.render_floating(
                            ctx.r(),
                            xray_pos,
                            focus_ring,
                            RenderLayer::MovingBetweenWorkspaces,
                            push!(),
                        );
                    }
                    1 => {
                        ws.render_floating(
                            ctx.r(),
                            xray_pos,
                            focus_ring,
                            RenderLayer::Normal,
                            push!(),
                        );

                        if let Some(loc) = insert_hint_render_loc {
                            if loc.workspace == InsertWorkspace::Existing(ws.id()) {
                                self.insert_hint_element.render(
                                    ctx.renderer,
                                    loc.location,
                                    push!(),
                                );
                            }
                        }
                    }
                    2 => {
                        ws.render_scrolling(
                            ctx.r(),
                            xray_pos,
                            focus_ring,
                            RenderLayer::MovingBetweenWorkspaces,
                            push!(),
                        );
                    }
                    _ => {
                        ws.render_scrolling(
                            ctx.r(),
                            xray_pos,
                            focus_ring,
                            RenderLayer::Normal,
                            push!(),
                        );
                    }
                }
            }
        }

        if is_expo {
            let p = self.overview_progress.as_ref().map_or(1.0, |p| p.value());
            let border_alpha = (p.clamp(0.0, 1.0) as f32).max(0.0);
            let selected_idx = self
                .insert_hint
                .as_ref()
                .and_then(|hint| match hint.workspace {
                    InsertWorkspace::Existing(id) => self.idx_of_ws(id),
                    InsertWorkspace::NewAt(_) => None,
                })
                .unwrap_or(self.active_workspace_idx());
            if border_alpha > 0.05 {
                for (idx, active_geo) in self
                    .workspaces_render_geo()
                    .take(self.workspaces.len())
                    .enumerate()
                {
                    let selected = idx == selected_idx;
                    let bw = if selected { 4.0 } else { 1.0 };
                    let color_arr = if selected {
                        self.options
                            .layout
                            .focus_ring
                            .active_color
                            .to_array_premul()
                    } else {
                        [0.25, 0.25, 0.30, 0.6]
                    };
                    let loc = active_geo.loc;
                    let sz = active_geo.size;

                    if sz.w > bw * 2.0 && sz.h > bw * 2.0 {
                        let mut cache = self.expo_borders.borrow_mut();
                        cache.resize_with(self.workspaces.len(), || {
                            std::array::from_fn(|_| {
                                SolidColorBuffer::new(Size::from((0., 0.)), [0.; 4])
                            })
                        });
                        let [top_buf, btm_buf, left_buf, right_buf] = &mut cache[idx];
                        top_buf.update(Size::from((sz.w, bw)), color_arr);
                        btm_buf.update(Size::from((sz.w, bw)), color_arr);
                        left_buf.update(Size::from((bw, sz.h - bw * 2.0)), color_arr);
                        right_buf.update(Size::from((bw, sz.h - bw * 2.0)), color_arr);

                        let top_elem = SolidColorRenderElement::from_buffer(
                            top_buf,
                            loc,
                            border_alpha,
                            Kind::Unspecified,
                        );
                        let btm_elem = SolidColorRenderElement::from_buffer(
                            btm_buf,
                            Point::from((loc.x, loc.y + sz.h - bw)),
                            border_alpha,
                            Kind::Unspecified,
                        );
                        let left_elem = SolidColorRenderElement::from_buffer(
                            left_buf,
                            Point::from((loc.x, loc.y + bw)),
                            border_alpha,
                            Kind::Unspecified,
                        );
                        let right_elem = SolidColorRenderElement::from_buffer(
                            right_buf,
                            Point::from((loc.x + sz.w - bw, loc.y + bw)),
                            border_alpha,
                            Kind::Unspecified,
                        );

                        for elem in [top_elem, btm_elem, left_elem, right_elem] {
                            let inner = MonitorInnerRenderElement::SolidColor(elem);
                            let rescaled =
                                RescaleRenderElement::from_element(inner, Point::default(), 1.0);
                            push(RelocateRenderElement::from_element(
                                rescaled,
                                Point::default(),
                                Relocate::Relative,
                            ));
                        }
                    }
                }
            }
        }
    }

    pub fn render_workspace_shadows<R: RealandRenderer>(
        &self,
        renderer: &mut R,
        push: &mut dyn FnMut(MonitorRenderElement<R>),
    ) {
        if self.is_stage_visible() {
            return;
        }
        let Some(progress) = self.overview_progress.as_ref().map(|p| p.clamped_value()) else {
            return;
        };
        let alpha = progress.clamp(0., 1.) as f32;

        let _span = realand_tracy_span!("Monitor::render_workspace_shadows");

        let scale = self.scale.fractional_scale();
        let zoom = self.overview_zoom();

        for (ws, geo) in self.workspaces_with_render_geo() {
            let actual_zoom = if self.overview_mode() == OverviewMode::Expo
                && (self.is_overview_open() || self.overview_progress.is_some())
            {
                (geo.size.w / self.view_size.w.max(1.0)).clamp(0.01, 1.0)
            } else {
                zoom
            };
            ws.render_shadow(renderer, &mut |elem| {
                let elem = elem.with_alpha(alpha);
                let elem = MonitorInnerRenderElement::Shadow(elem);
                let elem =
                    RescaleRenderElement::from_element(elem, Point::from((0, 0)), actual_zoom);
                let elem = RelocateRenderElement::from_element(
                    elem,
                    geo.loc.to_physical_precise_round(scale),
                    Relocate::Relative,
                );
                push(elem);
            });
        }
    }

    pub(super) fn workspace_gesture_value(&self) -> f64 {
        self.core
            .borrow()
            .workspace_switch()
            .gesture(&self.output_name)
            .map_or(self.active_workspace_idx() as f64, |gesture| {
                gesture.current()
            })
    }

    fn workspace_gesture_is_dnd(&self) -> bool {
        self.core
            .borrow()
            .workspace_switch()
            .gesture(&self.output_name)
            .is_some_and(|gesture| gesture.is_dnd())
    }

    pub(super) fn workspace_switch_idx(&self) -> f64 {
        match &self.workspace_switch {
            Some(WorkspaceSwitch::Animation(animation)) => animation.value(),
            Some(WorkspaceSwitch::Gesture(gesture)) => {
                self.workspace_gesture_value()
                    + gesture.animation.as_ref().map_or(0., Animation::value)
            }
            None => self.active_workspace_idx() as f64,
        }
    }

    pub(super) fn clear_workspace_switch(&mut self) {
        self.core.borrow_mut().workspace_switch_action(
            realand_core::workspace_switch::WorkspaceSwitchAction::Clear {
                connector: self.output_name.clone(),
            },
        );
        self.workspace_switch = None;
    }

    pub fn workspace_switch_gesture_begin(&mut self, is_touchpad: bool) {
        let center = SpaceId(self.active_workspace_ref().id().get());
        let current = self.workspace_render_idx();
        let is_clamped = !self.is_overview_open();
        self.core.borrow_mut().workspace_switch_action(
            realand_core::workspace_switch::WorkspaceSwitchAction::Begin {
                connector: self.output_name.clone(),
                center,
                current,
                is_touchpad,
                is_clamped,
                dnd_timestamp: None,
            },
        );
        self.workspace_switch = Some(WorkspaceSwitch::Gesture(WorkspaceSwitchGesture {
            animation: None,
        }));
    }

    pub fn dnd_scroll_gesture_begin(&mut self) {
        if self.workspace_gesture_is_dnd() || !self.is_overview_open() {
            return;
        }
        let center = SpaceId(self.active_workspace_ref().id().get());
        let current = self.workspace_render_idx();
        self.core.borrow_mut().workspace_switch_action(
            realand_core::workspace_switch::WorkspaceSwitchAction::Begin {
                connector: self.output_name.clone(),
                center,
                current,
                is_touchpad: false,
                is_clamped: false,
                dnd_timestamp: Some(self.clock.now_unadjusted()),
            },
        );
        self.workspace_switch = Some(WorkspaceSwitch::Gesture(WorkspaceSwitchGesture {
            animation: None,
        }));
    }

    pub fn workspace_switch_gesture_update(
        &mut self,
        delta_y: f64,
        timestamp: Duration,
        is_touchpad: bool,
    ) -> Option<bool> {
        {
            let core = self.core.borrow();
            let gesture = core.workspace_switch().gesture(&self.output_name)?;
            if gesture.is_touchpad() != is_touchpad || gesture.is_dnd() {
                return None;
            }
        }
        let zoom = self.overview_zoom();
        let viewport_height = self.workspace_size_with_gap(1.).h;
        Some(
            self.core
                .borrow_mut()
                .workspace_switch_action(
                    realand_core::workspace_switch::WorkspaceSwitchAction::Update {
                        connector: self.output_name.clone(),
                        delta_y,
                        timestamp,
                        is_touchpad,
                        zoom,
                        viewport_height,
                    },
                )
                .changed,
        )
    }

    pub fn dnd_scroll_gesture_scroll(&mut self, pos: Point<f64, Logical>, speed: f64) -> bool {
        use realand_core::workspace_switch::{DndGeometry, WorkspaceSwitchAction};
        if !self.workspace_gesture_is_dnd() {
            return false;
        }
        let config = &self.options.gestures.dnd_edge_workspace_switch;
        let geometry = DndGeometry {
            position: [pos.x, pos.y],
            viewport_width: self.view_size.w,
            working_top: self.working_area.loc.y,
            working_height: self.working_area.size.h,
            zoom: self.overview_zoom(),
            trigger_height: config.trigger_height,
            speed,
            max_speed: config.max_speed,
            delay: Duration::from_millis(u64::from(config.delay_ms)),
        };
        self.core
            .borrow_mut()
            .workspace_switch_action(WorkspaceSwitchAction::DndScroll {
                connector: self.output_name.clone(),
                timestamp: self.clock.now_unadjusted(),
                geometry,
            })
            .changed
    }

    fn finish_workspace_gesture(&mut self, is_touchpad: Option<bool>, cancelled: bool) -> bool {
        use realand_core::workspace_switch::WorkspaceSwitchAction;
        let Some(WorkspaceSwitch::Gesture(gesture)) = &self.workspace_switch else {
            return false;
        };
        let extra_position = gesture.animation.as_ref().map_or(0., Animation::value);
        let extra_velocity = gesture.animation.as_ref().map_or(0., Animation::velocity);
        let connector = self.output_name.clone();
        let timestamp = self.clock.now_unadjusted();
        let zoom = self.overview_zoom();
        let viewport_height = self.workspace_size_with_gap(1.).h;
        let action = if cancelled {
            WorkspaceSwitchAction::Cancel {
                connector,
                is_touchpad,
                timestamp,
                zoom,
                viewport_height,
                extra_position,
                extra_velocity,
            }
        } else {
            WorkspaceSwitchAction::End {
                connector,
                is_touchpad,
                timestamp,
                zoom,
                viewport_height,
                extra_position,
                extra_velocity,
            }
        };
        let Some(settlement) = self
            .core
            .borrow_mut()
            .workspace_switch_action(action)
            .settlement
        else {
            return false;
        };
        self.workspace_switch = Some(WorkspaceSwitch::Animation(Animation::new(
            self.clock.clone(),
            settlement.from,
            settlement.to,
            settlement.velocity,
            self.options.animations.workspace_switch.0,
        )));
        true
    }

    pub fn workspace_switch_gesture_cancel(&mut self, is_touchpad: Option<bool>) -> bool {
        self.finish_workspace_gesture(is_touchpad, true)
    }

    pub fn workspace_switch_gesture_end(&mut self, is_touchpad: Option<bool>) -> bool {
        self.finish_workspace_gesture(is_touchpad, false)
    }

    pub fn dnd_scroll_gesture_end(&mut self) {
        if self.workspace_gesture_is_dnd() {
            self.workspace_switch_gesture_end(None);
        }
    }

    pub fn scale(&self) -> smithay::output::Scale {
        self.scale
    }

    pub fn view_size(&self) -> Size<f64, Logical> {
        self.view_size
    }

    pub fn working_area(&self) -> Rectangle<f64, Logical> {
        self.working_area
    }

    pub fn layout_config(&self) -> Option<&realand_config::LayoutPart> {
        self.layout_config.as_ref()
    }

    #[cfg(test)]
    pub(super) fn verify_invariants(&self) {
        use approx::assert_abs_diff_eq;

        let options =
            Options::clone(&self.base_options).with_merged_layout(self.layout_config.as_ref());
        assert_eq!(&*self.options, &options);

        assert!(
            !self.workspaces.is_empty(),
            "monitor must have at least one workspace"
        );
        assert!(self.active_workspace_idx() < self.workspaces.len());

        if let Some(WorkspaceSwitch::Animation(anim)) = &self.workspace_switch {
            let before_idx = anim.from() as usize;
            let after_idx = anim.to() as usize;

            assert!(before_idx < self.workspaces.len());
            assert!(after_idx < self.workspaces.len());
        }

        assert!(
            !self.workspaces.last().unwrap().has_windows(),
            "monitor must have an empty workspace in the end"
        );
        if self.options.layout.empty_workspace_above_first {
            assert!(
                !self.workspaces.first().unwrap().has_windows(),
                "first workspace must be empty when empty_workspace_above_first is set"
            )
        }

        assert!(
            self.workspaces.last().unwrap().name().is_none(),
            "monitor must have an unnamed workspace in the end"
        );
        if self.options.layout.empty_workspace_above_first {
            assert!(
                self.workspaces.first().unwrap().name().is_none(),
                "first workspace must be unnamed when empty_workspace_above_first is set"
            )
        }

        if self.options.layout.empty_workspace_above_first {
            assert!(
                self.workspaces.len() != 2
                    || self
                        .stage_drag_source()
                        .is_some_and(|id| self.idx_of_ws(id).is_some()),
                "if empty_workspace_above_first is set there must be just 1 or 3+ workspaces"
            )
        }

        // If there's no workspace switch in progress, there can't be any non-last non-active
        // empty workspaces. If empty_workspace_above_first is set then the first workspace
        // will be empty too.
        let pre_skip = if self.options.layout.empty_workspace_above_first {
            1
        } else {
            0
        };
        if self.workspace_switch.is_none() {
            for (idx, ws) in self
                .workspaces
                .iter()
                .enumerate()
                .skip(pre_skip)
                .rev()
                // skip last
                .skip(1)
            {
                if idx != self.active_workspace_idx() && Some(ws.id()) != self.stage_drag_source() {
                    assert!(
                        ws.has_windows_or_name(),
                        "non-active workspace can't be empty and unnamed except the last one"
                    );
                }
            }
        }

        for workspace in &self.workspaces {
            assert_eq!(self.clock, workspace.clock);

            assert_eq!(
                self.scale().integer_scale(),
                workspace.scale().integer_scale()
            );
            assert_eq!(
                self.scale().fractional_scale(),
                workspace.scale().fractional_scale()
            );
            assert_eq!(self.view_size, workspace.view_size());
            assert_eq!(self.working_area, workspace.working_area());

            assert_eq!(
                workspace.base_options, self.options,
                "workspace options must be synchronized with monitor"
            );
        }

        let scale = self.scale().fractional_scale();
        let iter = self.workspaces_with_render_geo();
        for (_ws, ws_geo) in iter {
            let pos = ws_geo.loc;
            let rounded_pos = pos.to_physical_precise_round(scale).to_logical(scale);

            // Workspace positions must be rounded to physical pixels.
            assert_abs_diff_eq!(pos.x, rounded_pos.x, epsilon = 1e-5);
            assert_abs_diff_eq!(pos.y, rounded_pos.y, epsilon = 1e-5);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_expo_grid_dimensions() {
        // 4 workspaces -> 2 cols, 2 rows
        let count: usize = 4;
        let cols = match count {
            1 => 1,
            2 => 2,
            3..=4 => 2,
            5..=6 => 3,
            7..=9 => 3,
            _ => (count as f64).sqrt().ceil() as usize,
        };
        let rows = count.div_ceil(cols);
        assert_eq!(cols, 2);
        assert_eq!(rows, 2);

        // 6 workspaces -> 3 cols, 2 rows
        let count: usize = 6;
        let cols = match count {
            1 => 1,
            2 => 2,
            3..=4 => 2,
            5..=6 => 3,
            7..=9 => 3,
            _ => (count as f64).sqrt().ceil() as usize,
        };
        let rows = count.div_ceil(cols);
        assert_eq!(cols, 3);
        assert_eq!(rows, 2);
    }

    #[test]
    fn test_expo_2d_grid_hit_testing_non_overlapping() {
        use smithay::utils::{Logical, Point, Rectangle, Size};

        // Two cells in a row: cell 0 at (10, 10), cell 1 at (500, 10)
        let cell0: Rectangle<f64, Logical> =
            Rectangle::new(Point::from((10.0, 10.0)), Size::from((400.0, 300.0)));
        let cell1: Rectangle<f64, Logical> =
            Rectangle::new(Point::from((500.0, 10.0)), Size::from((400.0, 300.0)));

        let pos_right = Point::<f64, Logical>::from((600.0, 100.0));
        assert!(!cell0.contains(pos_right));
        assert!(cell1.contains(pos_right));
    }
}
