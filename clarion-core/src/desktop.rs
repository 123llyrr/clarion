//! Authoritative desktop identity and transitions.
//!
//! Mutation is event-driven. Platform adapters consume state intents, route
//! focus and invalidate scenes. Retained layout policies calculate geometry;
//! adapters configure clients and acknowledge those targets here. No renderer
//! or protocol handle is retained in this model.

use std::collections::HashMap;
use std::hash::Hash;

use crate::bsp::BspSnapshot;
use crate::geometry::{Rect, Size};
use crate::layout::LayoutMode;
use crate::move_controls::{MoveControls, MovePlacement, MoveStart, MoveStartIntent};
use crate::policy_memory::Width;
use crate::policy_memory::{LayoutHistory, ScrollingSnapshot};
use crate::workspace_state::{
    CanvasTransition, MinimizeIntent, MinimizeRestore, RestoreColumn, SpaceInteraction,
    SpaceLayerFocus,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct SpaceId(pub u64);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Layer {
    Background,
    Below,
    #[default]
    Normal,
    Above,
    Overlay,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SavedGeometry([Option<Rect>; 3]);

impl SavedGeometry {
    pub fn get(&self, mode: LayoutMode) -> Option<Rect> {
        self.0[mode.index()]
    }
    pub fn set(&mut self, mode: LayoutMode, rect: Rect) {
        self.0[mode.index()] = Some(rect.sanitized());
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WindowRecord<I> {
    pub id: I,
    pub space: SpaceId,
    pub output: Option<String>,
    /// False while a platform interaction holds a mapped tile between spaces.
    pub attached: bool,
    /// The intended connector survives disconnection and fallback placement.
    pub preferred_output: Option<String>,
    pub layer: Layer,
    pub minimized: bool,
    pub geometry: Rect,
    pub min_size: Size,
    pub max_size: Size,
    pub saved_geometry: SavedGeometry,
    /// Numeric placement memory belongs to core while the platform parks the
    /// client's Tile, buffers and visual effect snapshot.
    pub restoration: Option<MinimizeRestore<I>>,
}

impl<I> WindowRecord<I> {
    pub fn new(id: I, space: SpaceId, geometry: Rect) -> Self {
        Self {
            id,
            space,
            output: None,
            attached: true,
            preferred_output: None,
            layer: Layer::Normal,
            minimized: false,
            geometry: geometry.sanitized(),
            min_size: Size::default(),
            max_size: Size::default(),
            saved_geometry: SavedGeometry::default(),
            restoration: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpaceRecord<I> {
    pub id: SpaceId,
    pub name: Option<String>,
    pub mode: LayoutMode,
    pub focused: Option<I>,
    pub history: LayoutHistory<I>,
    pub interaction: SpaceInteraction<I>,
    /// Preferred return output; this remains alive while all outputs are absent.
    pub output_preference: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutputRecord {
    pub connector: String,
    pub bounds: Rect,
    /// Connection identity, distinct from mutable bounds or a reused name.
    pub generation: u64,
    pub identity: crate::output_identity::OutputIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MovePhase {
    Starting,
    Moving,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MoveInteraction<I> {
    pub id: I,
    pub controls: MoveControls,
    pub phase: MovePhase,
    pub origin_space: SpaceId,
    pub origin_output: Option<String>,
    pub origin_geometry: Rect,
    pub output: Option<String>,
    pub geometry: Rect,
    pub pointer: [f64; 2],
}

#[derive(Debug, Clone, PartialEq)]
pub enum CoreEvent<I> {
    Overview(crate::overview::OverviewAction),
    Spread(crate::spread::SpreadAction<I>),
    /// Metadata from the current client identity. Search membership is cached
    /// by Spread; title painting still needs damage when case folding agrees.
    WindowMetadataChanged {
        id: I,
        search_text: Option<String>,
    },
    OutputIdentityUpdated {
        connector: String,
        identity: crate::output_identity::OutputIdentity,
    },
    SetSpaceOutputPreference {
        space: SpaceId,
        selector: String,
    },
    SetSpaceName {
        space: SpaceId,
        name: Option<String>,
    },
    PreferUnnamedSpaceOutput {
        space: SpaceId,
        connector: String,
    },
    PreferSpaceOutput {
        space: SpaceId,
        connector: String,
    },
    NormalizeSpaceOutputPreference {
        space: SpaceId,
        connector: String,
    },
    WorkspaceSwitch(crate::workspace_switch::WorkspaceSwitchAction),
    StageDrag(crate::stage_drag::StageDragAction<I>),
    DndHold(crate::dnd_hold::DndHoldAction<I>),
    Management(crate::management::ManagementAction<I>),
    RememberScratchpadReturn {
        space: Option<SpaceId>,
    },
    TakeScratchpadReturn,
    SetSpaceLayerFocus {
        space: SpaceId,
        focus: SpaceLayerFocus,
    },
    RepairSpaceLayer {
        space: SpaceId,
    },
    AdoptSpaceInteraction {
        space: SpaceId,
        interaction: SpaceInteraction<I>,
    },
    CaptureCanvasPolicy {
        space: SpaceId,
        policy: crate::workspace_state::CanvasPolicyMemory<I>,
    },
    SetCanvasMode {
        space: SpaceId,
        enabled: bool,
    },
    SetCanvasOrigin {
        id: I,
        tiled: bool,
    },
    /// A real command. The returned intent is executed once by the platform;
    /// SetMinimized remains an acknowledgement used during payload transfer.
    MinimizeWindow {
        id: I,
        restoration: MinimizeRestore<I>,
    },
    RestoreWindow {
        id: I,
        columns: Vec<RestoreColumn<I>>,
    },
    OutputConnected {
        connector: String,
        bounds: Rect,
    },
    /// Register device metadata when workspace placement is supplied explicitly
    /// by an adapter, rather than invoking automatic connector restoration.
    OutputRegistered {
        connector: String,
        bounds: Rect,
    },
    OutputDisconnected {
        connector: String,
    },
    CreateSpace {
        id: SpaceId,
        mode: LayoutMode,
    },
    RemoveSpace {
        id: SpaceId,
        fallback: SpaceId,
    },
    /// Retire an empty platform workspace without migrating any windows.
    ForgetSpace {
        id: SpaceId,
    },
    ActivateSpace {
        id: SpaceId,
    },
    AttachSpace {
        id: SpaceId,
        connector: String,
        index: usize,
    },
    DetachSpace {
        id: SpaceId,
    },
    ReorderSpace {
        id: SpaceId,
        index: usize,
    },
    ReorderDetachedSpace {
        id: SpaceId,
        index: usize,
    },
    SelectOutput {
        connector: String,
    },
    SelectOutputSpace {
        connector: String,
        id: SpaceId,
        remember_previous: bool,
    },
    SetPreviousSpace {
        connector: String,
        id: Option<SpaceId>,
    },
    RememberOutputSpace {
        connector: String,
    },
    /// Suspend global focus without discarding each space's remembered target.
    DeactivateSpace,
    MapWindow {
        window: WindowRecord<I>,
    },
    UnmapWindow {
        id: I,
    },
    DetachWindow {
        id: I,
    },
    SetOutput {
        id: I,
        output: Option<String>,
        remember: bool,
    },
    SetSpaceFocus {
        space: SpaceId,
        id: Option<I>,
        activate: bool,
    },
    MoveWindow {
        id: I,
        space: SpaceId,
        output: Option<String>,
    },
    FocusWindow {
        id: Option<I>,
    },
    SetMinimized {
        id: I,
        minimized: bool,
    },
    SetLayer {
        id: I,
        layer: Layer,
    },
    /// Acknowledge a target already configured by the platform policy adapter.
    CommitGeometry {
        id: I,
        geometry: Rect,
    },
    SetConstraints {
        id: I,
        minimum: Size,
        maximum: Size,
    },
    SetLayoutMode {
        space: SpaceId,
        mode: LayoutMode,
    },
    CaptureScrolling {
        space: SpaceId,
        snapshot: ScrollingSnapshot<I>,
    },
    CaptureDwindle {
        space: SpaceId,
        snapshot: BspSnapshot<I>,
    },
    BeginMove {
        id: I,
        start: MoveStart,
    },
    StartMoveMotion {
        id: I,
        delta: [f64; 2],
        zoom: f64,
    },
    /// Acknowledge placement returned by the retained policy's removal.
    MoveDetached {
        id: I,
        width: Width,
        full_width: bool,
    },
    SetMoveFloating {
        id: I,
        floating: bool,
    },
    MovePose {
        id: I,
        output: Option<String>,
        geometry: Rect,
        pointer: [f64; 2],
    },
    /// The platform has placed or disposed of the moving payload. Normal
    /// grab cancellation keeps its existing drop behavior; Stage restoration
    /// is a separate, explicit StageDrag command.
    MovePlaced {
        id: I,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum CoreEffect<I> {
    SpaceNameChanged {
        space: SpaceId,
    },
    ManagementChanged {
        outcome: crate::management::ManagementOutcome<I>,
    },
    ScratchpadReturnTaken(Option<SpaceId>),
    DndHoldChanged,
    DndActivateIntent(crate::dnd_hold::DndActivateIntent<I>),
    MoveStartIntent(MoveStartIntent),
    MoveDetachedAccepted,
    MovePlacementIntent(MovePlacement),
    SpaceLayerFocusChanged {
        space: SpaceId,
        focus: SpaceLayerFocus,
    },
    CanvasChanged {
        space: SpaceId,
        transition: CanvasTransition<I>,
    },
    MinimizeIntent(MinimizeIntent<I>),
    /// Synchronous numeric presentation outcome, consumed by the platform
    /// animation adapter rather than retained as a second interaction owner.
    OverviewChanged {
        outcome: crate::overview::OverviewOutcome,
    },
    /// Synchronous stable-ID search/selection state changed.
    SpreadChanged,
    StageDragChanged,
    StageRestoreIntent(crate::stage_drag::StageRestoreIntent<I>),
    WorkspaceSwitchChanged {
        outcome: crate::workspace_switch::WorkspaceSwitchOutcome,
    },
    WindowMapped {
        id: I,
    },
    WindowRemoved {
        id: I,
    },
    /// Adapters damage both regions, including effects around the old bounds.
    SceneChanged {
        id: I,
        output: Option<String>,
        old: Option<Rect>,
        new: Option<Rect>,
    },
    FocusChanged {
        previous: Option<I>,
        current: Option<I>,
    },
    WindowMoved {
        id: I,
        previous_space: SpaceId,
        space: SpaceId,
    },
    WindowOutputChanged {
        id: I,
        previous: Option<String>,
        current: Option<String>,
    },
    MinimizedChanged {
        id: I,
        minimized: bool,
    },
    LayerChanged {
        id: I,
        layer: Layer,
    },
    LayoutModeChanged {
        space: SpaceId,
        mode: LayoutMode,
    },
    SpaceActivated {
        previous: Option<SpaceId>,
        current: SpaceId,
    },
    OutputChanged {
        connector: String,
        connected: bool,
    },
    /// Space topology/display selection can change an empty output's scene.
    OutputSceneChanged {
        connector: String,
    },
}

/// The core keeps deterministic insertion order independently of hash keys.
/// IDs can be protocol-neutral integers, UUIDs or a platform adapter's stable ID.
#[derive(Debug, Clone)]
pub struct DesktopCore<I: Clone + Eq + Hash> {
    windows: HashMap<I, WindowRecord<I>>,
    window_order: Vec<I>,
    focus_order: Vec<I>,
    spaces: HashMap<SpaceId, SpaceRecord<I>>,
    space_order: Vec<SpaceId>,
    outputs: HashMap<String, OutputRecord>,
    next_output_generation: u64,
    navigation: crate::navigation::OutputSpacePolicy,
    active_space: Option<SpaceId>,
    /// Explicitly suspended keyboard scope is distinct from an initial empty session.
    focus_suspended: bool,
    active_window: Option<I>,
    interaction: Option<MoveInteraction<I>>,
    overview: crate::overview::OverviewState,
    spread: crate::spread::SpreadState<I>,
    workspace_switch: crate::workspace_switch::WorkspaceSwitchState,
    stage_drag: crate::stage_drag::StageDragState<I>,
    dnd_hold: crate::dnd_hold::DndHoldState<I>,
    management: crate::management::ManagementState<I>,
    scratchpad_return: Option<SpaceId>,
}

impl<I: Clone + Eq + Hash> Default for DesktopCore<I> {
    fn default() -> Self {
        Self {
            windows: HashMap::new(),
            window_order: Vec::new(),
            focus_order: Vec::new(),
            spaces: HashMap::new(),
            space_order: Vec::new(),
            outputs: HashMap::new(),
            next_output_generation: 1,
            navigation: crate::navigation::OutputSpacePolicy::new(),
            active_space: None,
            focus_suspended: false,
            active_window: None,
            interaction: None,
            overview: crate::overview::OverviewState::default(),
            spread: crate::spread::SpreadState::default(),
            workspace_switch: crate::workspace_switch::WorkspaceSwitchState::default(),
            stage_drag: crate::stage_drag::StageDragState::default(),
            dnd_hold: crate::dnd_hold::DndHoldState::default(),
            management: crate::management::ManagementState::default(),
            scratchpad_return: None,
        }
    }
}

impl<I: Clone + Eq + Hash> DesktopCore<I> {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn window(&self, id: &I) -> Option<&WindowRecord<I>> {
        self.windows.get(id)
    }
    pub fn windows(&self) -> impl Iterator<Item = &WindowRecord<I>> {
        self.window_order
            .iter()
            .filter_map(|id| self.windows.get(id))
    }
    pub fn space(&self, id: SpaceId) -> Option<&SpaceRecord<I>> {
        self.spaces.get(&id)
    }
    pub fn named_space(&self, name: &str) -> Option<SpaceId> {
        self.spaces()
            .find(|space| {
                space
                    .name
                    .as_ref()
                    .is_some_and(|candidate| candidate.eq_ignore_ascii_case(name))
            })
            .map(|space| space.id)
    }
    pub fn space_is_scratchpad(&self, id: SpaceId) -> bool {
        self.space(id)
            .and_then(|space| space.name.as_deref())
            .is_some_and(|name| {
                name.eq_ignore_ascii_case("scratchpad") || name.eq_ignore_ascii_case("special")
            })
    }
    pub fn spaces(&self) -> impl Iterator<Item = &SpaceRecord<I>> {
        self.space_order.iter().filter_map(|id| self.spaces.get(id))
    }
    pub fn output(&self, connector: &str) -> Option<&OutputRecord> {
        self.outputs.get(connector)
    }
    pub fn outputs(&self) -> impl Iterator<Item = &OutputRecord> {
        self.navigation
            .output_order()
            .iter()
            .filter_map(|id| self.outputs.get(id))
    }
    pub fn space_prefers_output(&self, space: SpaceId, connector: &str) -> bool {
        self.spaces
            .get(&space)
            .zip(self.outputs.get(connector))
            .is_some_and(|(space, output)| output.identity.matches(&space.output_preference))
    }
    /// Return stable IDs in their current source order at one hotplug mutation.
    /// Physical payload movement acknowledges these decisions afterward.
    pub fn returning_spaces(&self, source: &str, target: &str) -> Vec<SpaceId> {
        self.navigation
            .output(source)
            .into_iter()
            .flat_map(|output| output.spaces().iter().copied())
            .filter(|space| self.space_prefers_output(*space, target))
            .collect()
    }
    pub fn navigation(&self) -> &crate::navigation::OutputSpacePolicy {
        &self.navigation
    }
    pub fn take_remembered_space(&mut self, connector: &str) -> Option<SpaceId> {
        self.navigation.take_remembered_space(connector)
    }
    pub fn active_window(&self) -> Option<&I> {
        self.active_window.as_ref()
    }
    pub fn management(&self) -> &crate::management::ManagementState<I> {
        &self.management
    }
    pub fn scratchpad_return(&self) -> Option<SpaceId> {
        self.scratchpad_return
    }
    pub fn dnd_hold(&self) -> &crate::dnd_hold::DndHoldState<I> {
        &self.dnd_hold
    }
    pub fn interaction(&self) -> Option<&MoveInteraction<I>> {
        self.interaction.as_ref()
    }
    pub fn active_space(&self) -> Option<SpaceId> {
        self.active_space
    }
    pub fn overview(&self) -> &crate::overview::OverviewState {
        &self.overview
    }
    pub fn spread(&self) -> &crate::spread::SpreadState<I> {
        &self.spread
    }
    pub fn workspace_switch(&self) -> &crate::workspace_switch::WorkspaceSwitchState {
        &self.workspace_switch
    }
    pub fn stage_drag(&self) -> &crate::stage_drag::StageDragState<I> {
        &self.stage_drag
    }
    pub fn stage_drag_source(&self) -> Option<SpaceId> {
        self.stage_drag
            .source_space()
            .filter(|id| self.spaces.contains_key(id))
    }
    pub fn stage_drag_destination(&self) -> Option<SpaceId> {
        self.stage_drag.destination(
            &self.navigation,
            |id| self.spaces.contains_key(&id),
            |connector| self.outputs.get(connector).map(|output| output.generation),
        )
    }

    pub fn add_window(
        &mut self,
        id: I,
        space: SpaceId,
        output: Option<String>,
        geometry: Rect,
    ) -> Vec<CoreEffect<I>> {
        let mut window = WindowRecord::new(id, space, geometry);
        window.output = output.clone();
        window.preferred_output = output;
        self.apply(CoreEvent::MapWindow { window })
    }
    pub fn remove_window(&mut self, id: &I) -> Vec<CoreEffect<I>> {
        self.apply(CoreEvent::UnmapWindow { id: id.clone() })
    }
    pub fn focus_window(&mut self, id: Option<I>) -> Vec<CoreEffect<I>> {
        self.apply(CoreEvent::FocusWindow { id })
    }
    pub fn detach_window(&mut self, id: &I) -> Vec<CoreEffect<I>> {
        self.apply(CoreEvent::DetachWindow { id: id.clone() })
    }
    pub fn set_output(
        &mut self,
        id: &I,
        output: Option<String>,
        remember: bool,
    ) -> Vec<CoreEffect<I>> {
        self.apply(CoreEvent::SetOutput {
            id: id.clone(),
            output,
            remember,
        })
    }
    pub fn set_space_focus(
        &mut self,
        space: SpaceId,
        id: Option<I>,
        activate: bool,
    ) -> Vec<CoreEffect<I>> {
        self.apply(CoreEvent::SetSpaceFocus {
            space,
            id,
            activate,
        })
    }
    pub fn set_minimized(&mut self, id: &I, minimized: bool) -> Vec<CoreEffect<I>> {
        self.apply(CoreEvent::SetMinimized {
            id: id.clone(),
            minimized,
        })
    }
    pub fn move_window(
        &mut self,
        id: &I,
        space: SpaceId,
        output: Option<String>,
    ) -> Vec<CoreEffect<I>> {
        self.apply(CoreEvent::MoveWindow {
            id: id.clone(),
            space,
            output,
        })
    }
    pub fn set_layout_mode(&mut self, space: SpaceId, mode: LayoutMode) -> Vec<CoreEffect<I>> {
        self.apply(CoreEvent::SetLayoutMode { space, mode })
    }
    pub fn capture_scrolling(&mut self, space: SpaceId, snapshot: ScrollingSnapshot<I>) {
        self.apply(CoreEvent::CaptureScrolling { space, snapshot });
    }
    pub fn capture_dwindle(&mut self, space: SpaceId, snapshot: BspSnapshot<I>) {
        self.apply(CoreEvent::CaptureDwindle { space, snapshot });
    }

    fn ensure_space(&mut self, id: SpaceId) {
        if let std::collections::hash_map::Entry::Vacant(entry) = self.spaces.entry(id) {
            entry.insert(SpaceRecord {
                id,
                name: None,
                mode: LayoutMode::default(),
                focused: None,
                history: LayoutHistory::default(),
                interaction: SpaceInteraction::default(),
                output_preference: String::new(),
            });
            self.space_order.push(id);
            self.navigation.register_space(id);
            if self.active_space.is_none() && !self.focus_suspended {
                self.active_space = Some(id);
            }
        }
    }

    fn resolve_output(&self, preferred: Option<&str>) -> Option<String> {
        preferred
            .filter(|connector| self.outputs.contains_key(*connector))
            .map(str::to_owned)
            .or_else(|| self.navigation.output_order().first().cloned())
    }

    fn layer_members(&self, space: SpaceId) -> (bool, bool) {
        let mut tiled = false;
        let mut floating = false;
        for window in self
            .windows()
            .filter(|window| window.attached && window.space == space && !window.minimized)
        {
            if window.layer == Layer::Above {
                floating = true;
            } else {
                tiled = true;
            }
        }
        (tiled, floating)
    }

    fn space_scene_changed(&self, space: SpaceId, effects: &mut Vec<CoreEffect<I>>) {
        if let Some(connector) = self.navigation.owner(space) {
            effects.push(CoreEffect::OutputSceneChanged {
                connector: connector.to_owned(),
            });
        }
    }

    fn repair_space_layer(&mut self, space: SpaceId, effects: &mut Vec<CoreEffect<I>>) {
        let (tiled, floating) = self.layer_members(space);
        let Some(record) = self.spaces.get_mut(&space) else {
            return;
        };
        if record.interaction.repair_layer(tiled, floating) {
            effects.push(CoreEffect::SpaceLayerFocusChanged {
                space,
                focus: record.interaction.layer_focus(),
            });
            self.space_scene_changed(space, effects);
        }
    }

    fn prune_stage_associations(&mut self, effects: &mut Vec<CoreEffect<I>>) {
        let navigation = &self.navigation;
        let outputs = &self.outputs;
        let spaces = &self.spaces;
        if self.overview.retain_stage_spaces(|connector, space| {
            outputs.contains_key(connector)
                && spaces.contains_key(&space)
                && navigation.owner(space) == Some(connector)
        }) {
            effects.push(CoreEffect::OverviewChanged {
                outcome: crate::overview::OverviewOutcome {
                    changed: true,
                    settlement: None,
                },
            });
            for connector in self.navigation.output_order() {
                effects.push(CoreEffect::OutputSceneChanged {
                    connector: connector.clone(),
                });
            }
        }
    }

    fn stage_drag_changed(&self, effects: &mut Vec<CoreEffect<I>>) {
        effects.push(CoreEffect::StageDragChanged);
        for connector in self.navigation.output_order() {
            effects.push(CoreEffect::OutputSceneChanged {
                connector: connector.clone(),
            });
        }
    }

    fn reconcile_workspace_switch_order(&mut self, effects: &mut Vec<CoreEffect<I>>) {
        // Gesture positions follow stable center IDs at actual topology edits.
        // The presentation animation is only a relative offset, so it needs
        // no second index mirror or renderer-driven coordinate repair.
        for connector in self.navigation.output_order() {
            let order = self.navigation.output(connector).unwrap().spaces();
            if self.workspace_switch.reconcile_order(connector, order) {
                effects.push(CoreEffect::WorkspaceSwitchChanged {
                    outcome: crate::workspace_switch::WorkspaceSwitchOutcome {
                        changed: true,
                        ..Default::default()
                    },
                });
                effects.push(CoreEffect::OutputSceneChanged {
                    connector: connector.clone(),
                });
            }
        }
    }

    fn spread_candidate_allowed(&self, connector: &str, id: &I) -> bool {
        self.outputs.contains_key(connector)
            && self.windows.get(id).is_some_and(|window| {
                window.attached
                    && window.output.as_deref() == Some(connector)
                    && self
                        .navigation
                        .owner(window.space)
                        .is_none_or(|owner| owner == connector)
            })
    }

    fn prune_spread_associations(&mut self, effects: &mut Vec<CoreEffect<I>>) {
        let windows = &self.windows;
        let outputs = &self.outputs;
        let navigation = &self.navigation;
        if self.spread.retain_candidates(|connector, id| {
            outputs.contains_key(connector)
                && windows.get(id).is_some_and(|window| {
                    window.attached
                        && window.output.as_deref() == Some(connector)
                        && navigation
                            .owner(window.space)
                            .is_none_or(|owner| owner == connector)
                })
        }) {
            effects.push(CoreEffect::SpreadChanged);
            for connector in self.navigation.output_order() {
                effects.push(CoreEffect::OutputSceneChanged {
                    connector: connector.clone(),
                });
            }
        }
    }

    fn choose_focus(&self, space: SpaceId) -> Option<I> {
        let eligible = |id: &&I| {
            self.windows
                .get(*id)
                .is_some_and(|window| window.attached && window.space == space && !window.minimized)
        };
        self.focus_order
            .iter()
            .rev()
            .find(eligible)
            .cloned()
            .or_else(|| self.window_order.iter().rev().find(eligible).cloned())
    }

    fn change_focus(&mut self, next: Option<I>, effects: &mut Vec<CoreEffect<I>>) {
        if let Some(id) = &next {
            if let Some(window) = self
                .windows
                .get(id)
                .filter(|window| window.attached && !window.minimized)
            {
                if let Some(space) = self.spaces.get_mut(&window.space) {
                    space.focused = Some(id.clone());
                }
            }
        }
        if self.active_window == next {
            return;
        }
        let previous = self.active_window.take();
        if let Some(id) = &next {
            self.focus_order.retain(|old| old != id);
            self.focus_order.push(id.clone());
        }
        self.active_window = next.clone();
        effects.push(CoreEffect::FocusChanged {
            previous,
            current: next,
        });
    }

    fn select_display_scope(&mut self, id: SpaceId, effects: &mut Vec<CoreEffect<I>>) {
        let Some(connector) = self.navigation.owner(id).map(str::to_owned) else {
            return;
        };
        let old_output = self.navigation.selected_output().map(str::to_owned);
        if self.navigation.select_space(&connector, id, true) {
            effects.push(CoreEffect::OutputSceneChanged {
                connector: connector.clone(),
            });
        }
        if self.navigation.select_output(&connector) {
            if let Some(previous) = old_output {
                effects.push(CoreEffect::OutputSceneChanged {
                    connector: previous,
                });
            }
            effects.push(CoreEffect::OutputSceneChanged { connector });
        }
    }

    fn repair_focus(&mut self, effects: &mut Vec<CoreEffect<I>>) {
        // A moving payload keeps keyboard focus, but cannot be remembered as
        // an attached member of its former workspace.
        for space in self.spaces.values_mut() {
            if space.focused.as_ref().is_some_and(|id| {
                !self.windows.get(id).is_some_and(|window| {
                    window.attached && window.space == space.id && !window.minimized
                })
            }) {
                space.focused = None;
            }
        }
        if let Some(interaction) = self.interaction.as_ref().filter(|interaction| {
            interaction.phase == MovePhase::Moving
                && interaction.output.is_some()
                && self.windows.contains_key(&interaction.id)
        }) {
            let id = interaction.id.clone();
            self.change_focus(Some(id), effects);
            return;
        }
        let active_valid = self
            .active_window
            .as_ref()
            .and_then(|id| self.windows.get(id))
            .is_some_and(|window| {
                window.attached && !window.minimized && Some(window.space) == self.active_space
            });
        if !active_valid {
            let next = self.active_space.and_then(|space| self.choose_focus(space));
            self.change_focus(next, effects);
        }
    }

    fn update_geometry(&mut self, id: &I, geometry: Rect, effects: &mut Vec<CoreEffect<I>>) {
        let Some(window) = self.windows.get_mut(id) else {
            return;
        };
        let geometry = geometry.sanitized();
        let mode = if window.layer == Layer::Above {
            LayoutMode::Floating
        } else {
            self.spaces
                .get(&window.space)
                .map(|space| space.mode)
                .unwrap_or_default()
        };
        window.saved_geometry.set(mode, geometry);
        if window.geometry == geometry {
            return;
        }
        let old = std::mem::replace(&mut window.geometry, geometry);
        if !window.minimized {
            effects.push(CoreEffect::SceneChanged {
                id: id.clone(),
                output: window.output.clone(),
                old: Some(old),
                new: Some(geometry),
            });
        }
    }

    fn interaction_scene_changed(
        id: &I,
        previous_output: Option<String>,
        previous: Option<Rect>,
        current_output: &Option<String>,
        current: Option<Rect>,
        effects: &mut Vec<CoreEffect<I>>,
    ) {
        if previous_output == *current_output {
            effects.push(CoreEffect::SceneChanged {
                id: id.clone(),
                output: previous_output,
                old: previous,
                new: current,
            });
        } else {
            if previous.is_some() {
                effects.push(CoreEffect::SceneChanged {
                    id: id.clone(),
                    output: previous_output,
                    old: previous,
                    new: None,
                });
            }
            if current.is_some() {
                effects.push(CoreEffect::SceneChanged {
                    id: id.clone(),
                    output: current_output.clone(),
                    old: None,
                    new: current,
                });
            }
        }
    }

    fn output_connection_generation(&mut self, connector: &str) -> u64 {
        if let Some(output) = self.outputs.get(connector) {
            return output.generation;
        }
        let generation = self.next_output_generation;
        // Refuse exhaustion rather than reuse an identity retained by a drag.
        self.next_output_generation = generation
            .checked_add(1)
            .expect("output connection generation exhausted");
        generation
    }

    fn dnd_target_space(
        &self,
        connector: &str,
        target: &crate::dnd_hold::DndTarget<I>,
    ) -> Option<SpaceId> {
        use crate::dnd_hold::DndTarget;
        match target {
            DndTarget::Window(id) => self
                .windows
                .get(id)
                .filter(|window| {
                    window.attached
                        && !window.minimized
                        && window.output.as_deref() == Some(connector)
                        && self.navigation.owner(window.space) == Some(connector)
                })
                .map(|window| window.space),
            DndTarget::Workspace(space) => (self.spaces.contains_key(space)
                && self.navigation.owner(*space) == Some(connector))
            .then_some(*space),
        }
    }
    fn invalidate_dnd_window(&mut self, id: &I, effects: &mut Vec<CoreEffect<I>>) {
        if self.dnd_hold.invalidate_window(id) {
            effects.push(CoreEffect::DndHoldChanged);
        }
    }
    fn invalidate_dnd_space(&mut self, id: SpaceId, effects: &mut Vec<CoreEffect<I>>) {
        if self.dnd_hold.invalidate_space(id) {
            effects.push(CoreEffect::DndHoldChanged);
        }
    }
    fn prune_dnd_target(&mut self, effects: &mut Vec<CoreEffect<I>>) {
        let valid = self.dnd_hold.connection().is_some_and(|connection| {
            self.outputs
                .get(&connection.connector)
                .is_some_and(|output| output.generation == connection.generation)
                && self.dnd_hold.target().is_none_or(|target| {
                    self.dnd_target_space(&connection.connector, target)
                        .is_some()
                })
        });
        if self.dnd_hold.retain_target(|_| valid) {
            effects.push(CoreEffect::DndHoldChanged);
        }
    }
    fn update_output(&mut self, id: &I, output: Option<String>, effects: &mut Vec<CoreEffect<I>>) {
        let Some(window) = self.windows.get_mut(id) else {
            return;
        };
        if window.output == output {
            return;
        }
        let old = std::mem::replace(&mut window.output, output.clone());
        if self.dnd_hold.invalidate_window(id) {
            effects.push(CoreEffect::DndHoldChanged);
        }
        if !window.minimized {
            effects.push(CoreEffect::SceneChanged {
                id: id.clone(),
                output: old.clone(),
                old: Some(window.geometry),
                new: None,
            });
            effects.push(CoreEffect::SceneChanged {
                id: id.clone(),
                output: output.clone(),
                old: None,
                new: Some(window.geometry),
            });
        }
        effects.push(CoreEffect::WindowOutputChanged {
            id: id.clone(),
            previous: old,
            current: output,
        });
        self.prune_spread_associations(effects);
    }

    pub fn apply(&mut self, event: CoreEvent<I>) -> Vec<CoreEffect<I>> {
        let mut effects = Vec::new();
        self.apply_into(event, &mut effects);
        effects
    }

    /// Append effects into caller-retained storage, preserving queued operation order.
    pub fn apply_into(&mut self, event: CoreEvent<I>, effects: &mut Vec<CoreEffect<I>>) {
        match event {
            CoreEvent::SetSpaceName { space, name } => {
                // Naming an already named identity with any occupied alias is
                // the same no-op as the public CLI. Case-only collisions never
                // override another desktop or mutate a query's identity.
                if name
                    .as_ref()
                    .is_some_and(|name| self.named_space(name).is_some())
                {
                    return;
                }
                let Some(record) = self.spaces.get_mut(&space) else {
                    return;
                };
                if record.name == name {
                    return;
                }
                record.name = name;
                effects.push(CoreEffect::SpaceNameChanged { space });
                if let Some(connector) = self.navigation.owner(space) {
                    effects.push(CoreEffect::OutputSceneChanged {
                        connector: connector.into(),
                    });
                }
            }
            CoreEvent::PreferUnnamedSpaceOutput { space, connector } => {
                if self.space(space).is_some_and(|space| space.name.is_none()) {
                    self.apply_into(CoreEvent::PreferSpaceOutput { space, connector }, effects);
                }
            }
            CoreEvent::OutputIdentityUpdated {
                connector,
                identity,
            } => {
                if let Some(output) = self.outputs.get_mut(&connector) {
                    output.identity = identity;
                }
            }
            CoreEvent::SetSpaceOutputPreference { space, selector } => {
                if let Some(space) = self.spaces.get_mut(&space) {
                    space.output_preference = selector;
                }
            }
            CoreEvent::PreferSpaceOutput { space, connector } => {
                if let Some((space, output)) = self
                    .spaces
                    .get_mut(&space)
                    .zip(self.outputs.get(&connector))
                {
                    space.output_preference = output.identity.canonical_name();
                }
            }
            CoreEvent::NormalizeSpaceOutputPreference { space, connector } => {
                if let Some((space, output)) = self
                    .spaces
                    .get_mut(&space)
                    .zip(self.outputs.get(&connector))
                {
                    if output.identity.matches(&space.output_preference) {
                        space.output_preference = output.identity.canonical_name();
                    }
                }
            }
            CoreEvent::RememberScratchpadReturn { space } => {
                if space.is_none_or(|id| self.spaces.contains_key(&id)) {
                    self.scratchpad_return = space;
                }
            }
            CoreEvent::TakeScratchpadReturn => {
                effects.push(CoreEffect::ScratchpadReturnTaken(
                    self.scratchpad_return.take(),
                ));
            }
            CoreEvent::Management(action) => {
                use crate::management::ManagementAction;
                let target = match &action {
                    ManagementAction::MoveGroup { target, .. }
                    | ManagementAction::FollowGroup { target, .. } => Some(*target),
                    _ => None,
                };
                if target.is_some_and(|target| !self.spaces.contains_key(&target)) {
                    return;
                }
                let allow_history = self.interaction.is_none();
                let current = match &action {
                    ManagementAction::Checkpoint(current)
                    | ManagementAction::Undo { current, .. }
                    | ManagementAction::SetGroup { current, .. }
                    | ManagementAction::RemoveGroup { current, .. }
                    | ManagementAction::MoveGroup { current, .. } => Some(current),
                    _ => None,
                };
                if allow_history
                    && current.is_some_and(|current| {
                        current
                            .spaces
                            .iter()
                            .any(|space| !self.spaces.contains_key(&space.id))
                            || current.windows.iter().any(|saved| {
                                !self
                                    .windows
                                    .get(&saved.id)
                                    .is_some_and(|window| window.space == saved.space)
                            })
                    })
                {
                    return;
                }
                let windows = &self.windows;
                let outcome =
                    self.management
                        .apply(action, |id| windows.contains_key(id), allow_history);
                if outcome.changed || outcome.restore.is_some() || outcome.move_group.is_some() {
                    effects.push(CoreEffect::ManagementChanged { outcome });
                }
            }
            CoreEvent::DndHold(mut action) => {
                use crate::dnd_hold::{DndActivateIntent, DndHoldAction, DndTarget};
                let connector = match &action {
                    DndHoldAction::Motion { connector }
                    | DndHoldAction::Observe { connector, .. } => Some(connector.as_str()),
                    DndHoldAction::End => None,
                };
                let generation = connector
                    .and_then(|connector| self.outputs.get(connector))
                    .map_or(0, |output| output.generation);
                if let DndHoldAction::Observe {
                    connector, target, ..
                } = &mut action
                {
                    if target
                        .as_ref()
                        .is_some_and(|target| self.dnd_target_space(connector, target).is_none())
                    {
                        *target = None;
                    }
                }
                let outcome = self.dnd_hold.apply(action, generation);
                if outcome.changed {
                    effects.push(CoreEffect::DndHoldChanged);
                }
                if let Some(target) = outcome.activate {
                    let connector = self.dnd_hold.connection().unwrap().connector.clone();
                    if let Some(space) = self.dnd_target_space(&connector, &target) {
                        let window = match target {
                            DndTarget::Window(id) => Some(id),
                            DndTarget::Workspace(_) => None,
                        };
                        effects.push(CoreEffect::DndActivateIntent(DndActivateIntent {
                            connector,
                            space,
                            window,
                        }));
                    }
                }
            }
            CoreEvent::SetSpaceLayerFocus { space, focus } => {
                if !self.spaces.contains_key(&space) {
                    return;
                }
                let (tiled, floating) = self.layer_members(space);
                if self
                    .spaces
                    .get_mut(&space)
                    .unwrap()
                    .interaction
                    .select_layer(focus, tiled, floating)
                {
                    effects.push(CoreEffect::SpaceLayerFocusChanged {
                        space,
                        focus: self.spaces[&space].interaction.layer_focus(),
                    });
                    self.space_scene_changed(space, effects);
                }
            }
            CoreEvent::RepairSpaceLayer { space } => self.repair_space_layer(space, effects),
            CoreEvent::AdoptSpaceInteraction { space, interaction } => {
                if !self.spaces.contains_key(&space) {
                    return;
                }
                self.spaces.get_mut(&space).unwrap().interaction = interaction;
                self.repair_space_layer(space, effects);
                self.space_scene_changed(space, effects);
            }
            CoreEvent::CaptureCanvasPolicy { space, policy } => {
                if let Some(record) = self.spaces.get_mut(&space) {
                    record.interaction.capture_canvas_policy(policy);
                }
            }
            CoreEvent::SetCanvasMode { space, enabled } => {
                if !self.spaces.contains_key(&space) {
                    return;
                }
                let transition = self
                    .spaces
                    .get_mut(&space)
                    .unwrap()
                    .interaction
                    .set_canvas(enabled);
                if transition.changed {
                    effects.push(CoreEffect::CanvasChanged { space, transition });
                    self.space_scene_changed(space, effects);
                }
            }
            CoreEvent::SetCanvasOrigin { id, tiled } => {
                if let Some(window) = self
                    .windows
                    .get(&id)
                    .filter(|window| !tiled || (window.attached && !window.minimized))
                {
                    if let Some(space) = self.spaces.get_mut(&window.space) {
                        space.interaction.set_canvas_origin(id, tiled);
                    }
                }
            }
            CoreEvent::MinimizeWindow {
                id,
                mut restoration,
            } => {
                let Some(window) = self
                    .windows
                    .get(&id)
                    .filter(|window| window.attached && !window.minimized)
                else {
                    return;
                };
                let space = window.space;
                let was_floating = window.layer == Layer::Above;
                let canvas_origin = self.spaces[&space]
                    .interaction
                    .canvas_origins()
                    .contains(&id);
                if canvas_origin {
                    match self.spaces[&space].interaction.canvas_policy() {
                        Some(crate::workspace_state::CanvasPolicyMemory::Scrolling(snapshot)) => {
                            restoration = MinimizeRestore::capture(
                                &id,
                                was_floating,
                                window.geometry.width,
                                &snapshot.columns,
                                None,
                            );
                        }
                        Some(crate::workspace_state::CanvasPolicyMemory::Dwindle(snapshot)) => {
                            restoration = MinimizeRestore::capture(
                                &id,
                                was_floating,
                                window.geometry.width,
                                &[],
                                snapshot.topology.as_ref(),
                            );
                        }
                        None => {}
                    }
                }
                restoration.was_floating = was_floating;
                restoration.canvas_origin = self
                    .spaces
                    .get_mut(&space)
                    .unwrap()
                    .interaction
                    .take_canvas_origin(&id);
                self.windows.get_mut(&id).unwrap().restoration = Some(restoration);
                self.apply_into(
                    CoreEvent::SetMinimized {
                        id: id.clone(),
                        minimized: true,
                    },
                    effects,
                );
                self.repair_space_layer(space, effects);
                effects.push(CoreEffect::MinimizeIntent(MinimizeIntent {
                    id,
                    minimized: true,
                    restore: None,
                }));
            }
            CoreEvent::RestoreWindow { id, columns } => {
                let Some(window) = self
                    .windows
                    .get(&id)
                    .filter(|window| window.attached && window.minimized)
                else {
                    return;
                };
                let space = window.space;
                let Some(restoration) = window.restoration.as_ref() else {
                    return;
                };
                let record = &self.spaces[&space];
                let restore = restoration.resolve(
                    record.mode,
                    record.interaction.canvas_enabled(),
                    &columns,
                    |other| {
                        other != &id
                            && self.windows.get(other).is_some_and(|window| {
                                window.attached
                                    && window.space == space
                                    && !window.minimized
                                    && window.layer != Layer::Above
                            })
                    },
                );
                self.apply_into(
                    CoreEvent::SetMinimized {
                        id: id.clone(),
                        minimized: false,
                    },
                    effects,
                );
                self.spaces
                    .get_mut(&space)
                    .unwrap()
                    .interaction
                    .set_canvas_origin(id.clone(), restore.canvas_origin);
                self.windows.get_mut(&id).unwrap().restoration = None;
                self.repair_space_layer(space, effects);
                effects.push(CoreEffect::MinimizeIntent(MinimizeIntent {
                    id,
                    minimized: false,
                    restore: Some(restore),
                }));
            }
            CoreEvent::StageDrag(action) => {
                use crate::stage_drag::{StageCaptureContext, StageDragAction, StageOutput};
                let changed = match action {
                    StageDragAction::Capture(mut capture) => {
                        let Some(window) = self
                            .windows
                            .get(&capture.window)
                            .filter(|window| window.attached && !window.minimized)
                        else {
                            return;
                        };
                        let space = window.space;
                        let context = StageCaptureContext {
                            space,
                            output: window.output.as_ref().and_then(|connector| {
                                self.outputs.get(connector).map(|output| StageOutput {
                                    connector: connector.clone(),
                                    generation: output.generation,
                                })
                            }),
                            floating: window.layer == Layer::Above,
                            canvas_origin: self.spaces[&space]
                                .interaction
                                .canvas_origins()
                                .contains(&capture.window),
                        };
                        if context.canvas_origin {
                            match self.spaces[&space].interaction.canvas_policy() {
                                Some(crate::workspace_state::CanvasPolicyMemory::Scrolling(
                                    snapshot,
                                )) => {
                                    capture.columns = snapshot.columns.clone();
                                    capture.bsp = None;
                                }
                                Some(crate::workspace_state::CanvasPolicyMemory::Dwindle(
                                    snapshot,
                                )) => {
                                    capture.columns.clear();
                                    capture.bsp = Some(snapshot.clone());
                                }
                                None => {}
                            }
                        }
                        for column in &mut capture.columns {
                            column.windows.retain(|id| {
                                self.windows.get(id).is_some_and(|window| {
                                    window.attached && window.space == space && !window.minimized
                                })
                            });
                        }
                        self.stage_drag.capture(capture, context)
                    }
                    StageDragAction::Resolve {
                        window,
                        destination,
                        mut columns,
                    } => {
                        if !self.windows.contains_key(&window)
                            || self.stage_drag_destination() != Some(destination)
                        {
                            return;
                        }
                        let record = &self.spaces[&destination];
                        let windows = &self.windows;
                        let live = |id: &I| {
                            id == &window
                                || windows.get(id).is_some_and(|other| {
                                    other.attached && other.space == destination && !other.minimized
                                })
                        };
                        for column in &mut columns {
                            column.windows.retain(&live);
                        }
                        self.stage_drag.resolve(
                            &window,
                            destination,
                            record.mode,
                            record.interaction.canvas_enabled(),
                            &columns,
                            live,
                        )
                    }
                    StageDragAction::Take { window } => {
                        if !self.windows.contains_key(&window) {
                            return;
                        }
                        let Some(destination) = self.stage_drag.resolved_space() else {
                            return;
                        };
                        if !self.spaces.contains_key(&destination) {
                            if self.stage_drag.discard(&window) {
                                self.stage_drag_changed(effects);
                            }
                            return;
                        }
                        let Some(intent) = self.stage_drag.take(&window) else {
                            return;
                        };
                        effects.push(CoreEffect::StageRestoreIntent(intent));
                        true
                    }
                    StageDragAction::Finish { window } => {
                        let Some(destination) = self.stage_drag.restoring_space() else {
                            return;
                        };
                        if !self.windows.get(&window).is_some_and(|record| {
                            record.attached && record.space == destination && !record.minimized
                        }) {
                            return;
                        }
                        let Some(completion) = self.stage_drag.finish(&window) else {
                            return;
                        };
                        self.apply_into(
                            CoreEvent::SetCanvasOrigin {
                                id: completion.window,
                                tiled: completion.canvas_origin,
                            },
                            effects,
                        );
                        true
                    }
                    StageDragAction::Discard { window } => self.stage_drag.discard(&window),
                };
                if changed {
                    self.stage_drag_changed(effects);
                }
            }
            CoreEvent::WorkspaceSwitch(action) => {
                use crate::workspace_switch::WorkspaceSwitchAction;
                let connector = action.connector().to_owned();
                let allowed = match &action {
                    WorkspaceSwitchAction::OutputRemoved(_) => true,
                    WorkspaceSwitchAction::Begin { center, .. } => {
                        self.outputs.contains_key(&connector)
                            && self.navigation.owner(*center) == Some(connector.as_str())
                    }
                    WorkspaceSwitchAction::Activate { target, .. } => {
                        self.outputs.contains_key(&connector)
                            && self.navigation.owner(*target) == Some(connector.as_str())
                    }
                    _ => self.outputs.contains_key(&connector),
                };
                if !allowed {
                    return;
                }
                let order = self
                    .navigation
                    .output(&connector)
                    .map(|output| output.spaces())
                    .unwrap_or(&[]);
                let outcome = self.workspace_switch.apply(action, order);
                if let Some(settlement) = outcome.settlement {
                    self.apply_into(
                        CoreEvent::SelectOutputSpace {
                            connector: connector.clone(),
                            id: settlement.target,
                            remember_previous: settlement.remember_previous,
                        },
                        effects,
                    );
                }
                if outcome.changed {
                    effects.push(CoreEffect::WorkspaceSwitchChanged { outcome });
                    if self.outputs.contains_key(&connector) {
                        effects.push(CoreEffect::OutputSceneChanged { connector });
                    }
                }
            }
            CoreEvent::WindowMetadataChanged { id, search_text } => {
                if self.overview.mode() != crate::overview::OverviewMode::Windows
                    || !self.overview.visible()
                {
                    return;
                }
                let Some(connector) = self
                    .windows
                    .get(&id)
                    .and_then(|window| window.output.clone())
                    .filter(|connector| self.spread_candidate_allowed(connector, &id))
                    .filter(|connector| {
                        self.spread
                            .output(connector)
                            .is_some_and(|output| output.order().contains(&id))
                    })
                else {
                    return;
                };
                if self
                    .spread
                    .apply(crate::spread::SpreadAction::MetadataChanged {
                        connector: connector.clone(),
                        id,
                        search_text,
                    })
                {
                    effects.push(CoreEffect::SpreadChanged);
                }
                // Search caches lowercase text, while the renderer displays
                // the original title. Even an equal search value can repaint.
                effects.push(CoreEffect::OutputSceneChanged { connector });
            }
            CoreEvent::Spread(mut action) => {
                use crate::spread::SpreadAction;
                let connector = match &mut action {
                    SpreadAction::Candidates { connector, windows } => {
                        if !self.outputs.contains_key(connector) {
                            return;
                        }
                        windows.retain(|candidate| {
                            self.spread_candidate_allowed(connector, &candidate.id)
                        });
                        Some(connector.clone())
                    }
                    SpreadAction::MetadataChanged { connector, id, .. } => {
                        if !self.spread_candidate_allowed(connector, id) {
                            return;
                        }
                        Some(connector.clone())
                    }
                    SpreadAction::Reset {
                        connector,
                        preferred,
                    } => {
                        if !self.outputs.contains_key(connector) {
                            return;
                        }
                        if preferred
                            .as_ref()
                            .is_some_and(|id| !self.spread_candidate_allowed(connector, id))
                        {
                            *preferred = None;
                        }
                        Some(connector.clone())
                    }
                    SpreadAction::Key { connector, .. } => {
                        if !self.outputs.contains_key(connector) {
                            return;
                        }
                        Some(connector.clone())
                    }
                    SpreadAction::Begin { initial } => {
                        if initial
                            .as_ref()
                            .is_some_and(|id| !self.windows.contains_key(id))
                        {
                            *initial = None;
                        }
                        None
                    }
                    SpreadAction::WindowRemoved(_)
                    | SpreadAction::OutputRemoved(_)
                    | SpreadAction::Close => None,
                };
                if self.spread.apply(action) {
                    effects.push(CoreEffect::SpreadChanged);
                    if let Some(connector) = connector {
                        effects.push(CoreEffect::OutputSceneChanged { connector });
                    } else {
                        for connector in self.navigation.output_order() {
                            effects.push(CoreEffect::OutputSceneChanged {
                                connector: connector.clone(),
                            });
                        }
                    }
                }
            }
            CoreEvent::Overview(action) => {
                use crate::overview::OverviewAction;
                let allowed = match &action {
                    OverviewAction::StageReset { connector, space }
                    | OverviewAction::StageSelect { connector, space } => {
                        self.outputs.contains_key(connector)
                            && self.spaces.contains_key(space)
                            && self.navigation.owner(*space) == Some(connector.as_str())
                    }
                    OverviewAction::StageCloseAnchor { connector, space } => {
                        self.outputs.contains_key(connector)
                            && space.is_none_or(|space| {
                                self.spaces.contains_key(&space)
                                    && self.navigation.owner(space) == Some(connector.as_str())
                            })
                    }
                    OverviewAction::RememberExpo(space) | OverviewAction::RememberStage(space) => {
                        space.is_none_or(|space| self.spaces.contains_key(&space))
                    }
                    OverviewAction::PreviewOutput(connector) => connector
                        .as_ref()
                        .is_none_or(|connector| self.outputs.contains_key(connector)),
                    _ => true,
                };
                if !allowed {
                    return;
                }
                let presentation_settled = matches!(&action, OverviewAction::PresentationSettled);
                let outcome = self.overview.apply(action);
                if presentation_settled && !self.overview.desired_open() {
                    self.apply_into(
                        CoreEvent::Spread(crate::spread::SpreadAction::Close),
                        effects,
                    );
                }
                if outcome.changed {
                    effects.push(CoreEffect::OverviewChanged { outcome });
                    for connector in self.navigation.output_order() {
                        effects.push(CoreEffect::OutputSceneChanged {
                            connector: connector.clone(),
                        });
                    }
                }
            }
            CoreEvent::CreateSpace { id, mode } => {
                if !self.spaces.contains_key(&id) {
                    self.ensure_space(id);
                    self.spaces.get_mut(&id).unwrap().mode = mode;
                }
            }
            CoreEvent::RemoveSpace { id, fallback } => {
                if id == fallback || !self.spaces.contains_key(&id) {
                    return;
                }
                self.ensure_space(fallback);
                for window_id in &self.window_order {
                    let window = self.windows.get_mut(window_id).unwrap();
                    if window.space != id {
                        continue;
                    }
                    window.space = fallback;
                    if self.dnd_hold.invalidate_window(window_id) {
                        effects.push(CoreEffect::DndHoldChanged);
                    }
                    effects.push(CoreEffect::WindowMoved {
                        id: window.id.clone(),
                        previous_space: id,
                        space: fallback,
                    });
                }
                self.spaces.remove(&id);
                if self.scratchpad_return == Some(id) {
                    self.scratchpad_return = None;
                }
                self.invalidate_dnd_space(id, effects);
                self.space_order.retain(|old| *old != id);
                self.navigation.forget_space(id);
                self.reconcile_workspace_switch_order(effects);
                self.apply_into(
                    CoreEvent::Overview(crate::overview::OverviewAction::SpaceRemoved(id)),
                    effects,
                );
                if self.active_space == Some(id) {
                    self.select_display_scope(fallback, effects);
                    self.active_space = Some(fallback);
                    effects.push(CoreEffect::SpaceActivated {
                        previous: Some(id),
                        current: fallback,
                    });
                }
                self.prune_spread_associations(effects);
                self.repair_focus(effects);
            }
            CoreEvent::AttachSpace {
                id,
                connector,
                index,
            } => {
                if self.navigation.output(&connector).is_none() {
                    return;
                }
                self.ensure_space(id);
                let previous = self.navigation.owner(id).map(str::to_owned);
                if self.navigation.attach_space(&connector, id, index) {
                    self.invalidate_dnd_space(id, effects);
                    self.prune_dnd_target(effects);
                    if let Some(previous) = previous.filter(|old| old != &connector) {
                        effects.push(CoreEffect::OutputSceneChanged {
                            connector: previous,
                        });
                    }
                    effects.push(CoreEffect::OutputSceneChanged { connector });
                    self.reconcile_workspace_switch_order(effects);
                    self.prune_stage_associations(effects);
                    self.prune_spread_associations(effects);
                }
            }
            CoreEvent::DetachSpace { id } => {
                let previous = self.navigation.owner(id).map(str::to_owned);
                if self.navigation.detach_space(id) {
                    self.invalidate_dnd_space(id, effects);
                    self.prune_dnd_target(effects);
                    let windows = &self.windows;
                    if self.spread.retain_candidates(|_, candidate| {
                        windows
                            .get(candidate)
                            .is_some_and(|window| window.space != id)
                    }) {
                        effects.push(CoreEffect::SpreadChanged);
                    }
                    if let Some(connector) = previous {
                        effects.push(CoreEffect::OutputSceneChanged { connector });
                    }
                    self.reconcile_workspace_switch_order(effects);
                    self.prune_stage_associations(effects);
                    self.prune_spread_associations(effects);
                }
            }
            CoreEvent::ReorderSpace { id, index } => {
                if self.navigation.reorder_space(id, index) {
                    effects.push(CoreEffect::OutputSceneChanged {
                        connector: self.navigation.owner(id).unwrap().to_owned(),
                    });
                    self.reconcile_workspace_switch_order(effects);
                }
            }
            CoreEvent::ReorderDetachedSpace { id, index } => {
                self.navigation.reorder_detached_space(id, index);
            }
            CoreEvent::SelectOutput { connector } => {
                let previous = self.navigation.selected_output().map(str::to_owned);
                if self.navigation.select_output(&connector) {
                    if let Some(connector) = previous {
                        effects.push(CoreEffect::OutputSceneChanged { connector });
                    }
                    effects.push(CoreEffect::OutputSceneChanged { connector });
                }
            }
            CoreEvent::SelectOutputSpace {
                connector,
                id,
                remember_previous,
            } => {
                if self
                    .navigation
                    .select_space(&connector, id, remember_previous)
                {
                    effects.push(CoreEffect::OutputSceneChanged { connector });
                }
            }
            CoreEvent::SetPreviousSpace { connector, id } => {
                self.navigation.set_previous_space(&connector, id)
            }
            CoreEvent::RememberOutputSpace { connector } => {
                self.navigation.remember_active_space(&connector)
            }
            CoreEvent::ActivateSpace { id } => {
                self.focus_suspended = false;
                self.ensure_space(id);
                self.select_display_scope(id, effects);
                if self.active_space != Some(id) {
                    let previous = self.active_space.replace(id);
                    effects.push(CoreEffect::SpaceActivated {
                        previous,
                        current: id,
                    });
                }
                let next = self.spaces[&id].focused.clone().filter(|window| {
                    self.windows.get(window).is_some_and(|window| {
                        window.attached && !window.minimized && window.space == id
                    })
                });
                if self
                    .interaction
                    .as_ref()
                    .is_some_and(|interaction| interaction.phase == MovePhase::Moving)
                {
                    self.repair_focus(effects);
                } else {
                    self.change_focus(next, effects);
                }
            }
            CoreEvent::DeactivateSpace => {
                self.focus_suspended = true;
                self.active_space = None;
                self.change_focus(None, effects);
            }
            CoreEvent::ForgetSpace { id } => {
                if self
                    .windows()
                    .any(|window| window.attached && window.space == id)
                {
                    return;
                }
                self.spaces.remove(&id);
                if self.scratchpad_return == Some(id) {
                    self.scratchpad_return = None;
                }
                self.invalidate_dnd_space(id, effects);
                self.prune_dnd_target(effects);
                self.space_order.retain(|space| *space != id);
                let output = self.navigation.owner(id).map(str::to_owned);
                self.navigation.forget_space(id);
                self.reconcile_workspace_switch_order(effects);
                self.apply_into(
                    CoreEvent::Overview(crate::overview::OverviewAction::SpaceRemoved(id)),
                    effects,
                );
                if let Some(connector) = output {
                    effects.push(CoreEffect::OutputSceneChanged { connector });
                }
                if self.active_space == Some(id) {
                    self.active_space = self
                        .navigation
                        .selected_space()
                        .or_else(|| self.navigation.detached_spaces().first().copied())
                        .or_else(|| self.space_order.first().copied());
                    if let Some(current) = self.active_space {
                        effects.push(CoreEffect::SpaceActivated {
                            previous: Some(id),
                            current,
                        });
                    }
                    self.repair_focus(effects);
                }
            }
            CoreEvent::OutputConnected { connector, bounds } => {
                let bounds = bounds.sanitized();
                if self
                    .outputs
                    .get(&connector)
                    .is_some_and(|output| output.bounds == bounds)
                {
                    return;
                }
                let generation = self.output_connection_generation(&connector);
                let identity = self
                    .outputs
                    .get(&connector)
                    .map(|output| output.identity.clone())
                    .unwrap_or_else(|| {
                        crate::output_identity::OutputIdentity::connector_only(connector.clone())
                    });
                self.navigation.connect_output(connector.clone());
                self.outputs.insert(
                    connector.clone(),
                    OutputRecord {
                        connector: connector.clone(),
                        bounds,
                        generation,
                        identity,
                    },
                );
                effects.push(CoreEffect::OutputChanged {
                    connector: connector.clone(),
                    connected: true,
                });
                let ids: Vec<_> = self
                    .windows()
                    .filter(|window| {
                        window.attached
                            && (window.output.is_none()
                                || window.preferred_output.as_deref() == Some(connector.as_str()))
                    })
                    .map(|window| window.id.clone())
                    .collect();
                for id in ids {
                    self.update_output(&id, Some(connector.clone()), effects);
                }
            }
            CoreEvent::OutputRegistered { connector, bounds } => {
                let bounds = bounds.sanitized();
                if self
                    .outputs
                    .get(&connector)
                    .is_some_and(|output| output.bounds == bounds)
                {
                    return;
                }
                let generation = self.output_connection_generation(&connector);
                let identity = self
                    .outputs
                    .get(&connector)
                    .map(|output| output.identity.clone())
                    .unwrap_or_else(|| {
                        crate::output_identity::OutputIdentity::connector_only(connector.clone())
                    });
                self.navigation.connect_output(connector.clone());
                self.outputs.insert(
                    connector.clone(),
                    OutputRecord {
                        connector: connector.clone(),
                        bounds,
                        generation,
                        identity,
                    },
                );
                effects.push(CoreEffect::OutputChanged {
                    connector,
                    connected: true,
                });
            }
            CoreEvent::OutputDisconnected { connector } => {
                if self.outputs.remove(&connector).is_none() {
                    return;
                }
                self.navigation.disconnect_output(&connector);
                if self.dnd_hold.output_removed(&connector) {
                    effects.push(CoreEffect::DndHoldChanged);
                }
                self.apply_into(
                    CoreEvent::WorkspaceSwitch(
                        crate::workspace_switch::WorkspaceSwitchAction::OutputRemoved(
                            connector.clone(),
                        ),
                    ),
                    effects,
                );
                self.apply_into(
                    CoreEvent::Spread(crate::spread::SpreadAction::OutputRemoved(
                        connector.clone(),
                    )),
                    effects,
                );
                self.apply_into(
                    CoreEvent::Overview(crate::overview::OverviewAction::OutputRemoved(
                        connector.clone(),
                    )),
                    effects,
                );
                let fallback = self.navigation.output_order().first().cloned();
                if let Some(interaction) = self
                    .interaction
                    .as_mut()
                    .filter(|interaction| interaction.output.as_deref() == Some(connector.as_str()))
                {
                    interaction.output = fallback.clone();
                }
                let ids: Vec<_> = self
                    .windows()
                    .filter(|window| window.output.as_deref() == Some(connector.as_str()))
                    .map(|window| window.id.clone())
                    .collect();
                for id in ids {
                    self.update_output(&id, fallback.clone(), effects);
                }
                effects.push(CoreEffect::OutputChanged {
                    connector,
                    connected: false,
                });
            }
            CoreEvent::MapWindow { mut window } => {
                if self.windows.contains_key(&window.id) {
                    return;
                }
                self.ensure_space(window.space);
                window.geometry = window.geometry.sanitized();
                window.min_size = window.min_size.sanitized();
                window.max_size = window.max_size.sanitized();
                if window.preferred_output.is_none() {
                    window.preferred_output = window.output.clone();
                }
                window.output = self.resolve_output(window.preferred_output.as_deref());
                let mode = if window.layer == Layer::Above {
                    LayoutMode::Floating
                } else {
                    self.spaces[&window.space].mode
                };
                window.saved_geometry.set(mode, window.geometry);
                let id = window.id.clone();
                effects.push(CoreEffect::WindowMapped { id: id.clone() });
                if !window.minimized {
                    effects.push(CoreEffect::SceneChanged {
                        id: id.clone(),
                        output: window.output.clone(),
                        old: None,
                        new: Some(window.geometry),
                    });
                }
                let space = window.space;
                self.window_order.push(id.clone());
                self.windows.insert(id, window);
                self.repair_space_layer(space, effects);
            }
            CoreEvent::UnmapWindow { id } => {
                let Some(window) = self.windows.remove(&id) else {
                    return;
                };
                let space = window.space;
                self.invalidate_dnd_window(&id, effects);
                if self.management.remove_window(&id) {
                    effects.push(CoreEffect::ManagementChanged {
                        outcome: crate::management::ManagementOutcome {
                            changed: true,
                            ..Default::default()
                        },
                    });
                }
                if self.stage_drag.discard(&id) {
                    self.stage_drag_changed(effects);
                }
                self.apply_into(
                    CoreEvent::Spread(crate::spread::SpreadAction::WindowRemoved(id.clone())),
                    effects,
                );
                for record in self.spaces.values_mut() {
                    record.interaction.take_canvas_origin(&id);
                }
                if self
                    .interaction
                    .as_ref()
                    .is_some_and(|interaction| interaction.id == id)
                {
                    let interaction = self.interaction.take().unwrap();
                    Self::interaction_scene_changed(
                        &id,
                        interaction.output,
                        Some(interaction.geometry),
                        &None,
                        None,
                        effects,
                    );
                }
                self.window_order.retain(|old| old != &id);
                self.focus_order.retain(|old| old != &id);
                if !window.minimized {
                    effects.push(CoreEffect::SceneChanged {
                        id: id.clone(),
                        output: window.output,
                        old: Some(window.geometry),
                        new: None,
                    });
                }
                effects.push(CoreEffect::WindowRemoved { id });
                self.repair_space_layer(space, effects);
                self.repair_focus(effects);
            }
            CoreEvent::MoveWindow { id, space, output } => {
                if !self.windows.contains_key(&id) {
                    return;
                }
                self.ensure_space(space);
                let resolved = self.resolve_output(output.as_deref());
                let window = self.windows.get_mut(&id).unwrap();
                let previous_space = std::mem::replace(&mut window.space, space);
                window.attached = true;
                window.preferred_output = output;
                if previous_space != space {
                    if self.dnd_hold.invalidate_window(&id) {
                        effects.push(CoreEffect::DndHoldChanged);
                    }
                    effects.push(CoreEffect::WindowMoved {
                        id: id.clone(),
                        previous_space,
                        space,
                    });
                    if !window.minimized {
                        effects.push(CoreEffect::SceneChanged {
                            id: id.clone(),
                            output: window.output.clone(),
                            old: Some(window.geometry),
                            new: Some(window.geometry),
                        });
                    }
                }
                self.update_output(&id, resolved, effects);
                if previous_space != space {
                    if let Some(record) = self.spaces.get_mut(&previous_space) {
                        record.interaction.take_canvas_origin(&id);
                    }
                    self.repair_space_layer(previous_space, effects);
                }
                self.repair_space_layer(space, effects);
                self.prune_spread_associations(effects);
                self.repair_focus(effects);
            }
            CoreEvent::DetachWindow { id } => {
                let Some(window) = self.windows.get_mut(&id) else {
                    return;
                };
                if !window.attached {
                    return;
                }
                window.attached = false;
                let space = window.space;
                self.update_output(&id, None, effects);
                self.repair_space_layer(space, effects);
                self.repair_focus(effects);
            }
            CoreEvent::SetOutput {
                id,
                output,
                remember,
            } => {
                if let Some(window) = self.windows.get_mut(&id) {
                    if remember {
                        window.preferred_output = output.clone();
                    }
                    self.update_output(&id, output, effects);
                }
            }
            CoreEvent::SetSpaceFocus {
                space,
                id,
                activate,
            } => {
                self.ensure_space(space);
                if id.as_ref().is_some_and(|id| {
                    !self.windows.get(id).is_some_and(|window| {
                        window.attached && window.space == space && !window.minimized
                    })
                }) {
                    return;
                }
                if let Some(window) = id.as_ref().and_then(|id| self.windows.get(id)) {
                    let focus = if window.layer == Layer::Above {
                        SpaceLayerFocus::Floating
                    } else if self.spaces[&space].interaction.layer_focus()
                        == SpaceLayerFocus::TilingWithFloatingRaised
                    {
                        SpaceLayerFocus::TilingWithFloatingRaised
                    } else {
                        SpaceLayerFocus::Tiling
                    };
                    self.apply_into(CoreEvent::SetSpaceLayerFocus { space, focus }, effects);
                }
                self.spaces.get_mut(&space).unwrap().focused = id.clone();
                if activate || self.active_space == Some(space) {
                    if activate {
                        self.focus_suspended = false;
                        self.select_display_scope(space, effects);
                    }
                    if self.active_space != Some(space) {
                        let previous = self.active_space.replace(space);
                        effects.push(CoreEffect::SpaceActivated {
                            previous,
                            current: space,
                        });
                    }
                    if self
                        .interaction
                        .as_ref()
                        .is_some_and(|interaction| interaction.phase == MovePhase::Moving)
                    {
                        self.repair_focus(effects);
                    } else {
                        self.change_focus(id, effects);
                    }
                }
            }
            CoreEvent::FocusWindow { id } => {
                if let Some(id) = &id {
                    let Some(window) = self
                        .windows
                        .get(id)
                        .filter(|window| window.attached && !window.minimized)
                    else {
                        return;
                    };
                    let space = window.space;
                    let focus = if window.layer == Layer::Above {
                        SpaceLayerFocus::Floating
                    } else {
                        SpaceLayerFocus::Tiling
                    };
                    self.apply_into(CoreEvent::SetSpaceLayerFocus { space, focus }, effects);
                    self.focus_suspended = false;
                    self.select_display_scope(space, effects);
                    if self.active_space != Some(space) {
                        let previous = self.active_space.replace(space);
                        effects.push(CoreEffect::SpaceActivated {
                            previous,
                            current: space,
                        });
                    }
                }
                self.change_focus(id, effects);
            }
            CoreEvent::SetMinimized { id, minimized } => {
                let Some(window) = self.windows.get_mut(&id) else {
                    return;
                };
                if window.minimized == minimized {
                    return;
                }
                window.minimized = minimized;
                if self.dnd_hold.invalidate_window(&id) {
                    effects.push(CoreEffect::DndHoldChanged);
                }
                if !minimized {
                    window.restoration = None;
                }
                let space = window.space;
                effects.push(CoreEffect::MinimizedChanged {
                    id: id.clone(),
                    minimized,
                });
                effects.push(CoreEffect::SceneChanged {
                    id,
                    output: window.output.clone(),
                    old: minimized.then_some(window.geometry),
                    new: (!minimized).then_some(window.geometry),
                });
                self.repair_space_layer(space, effects);
                self.repair_focus(effects);
            }
            CoreEvent::SetLayer { id, layer } => {
                let Some(window) = self.windows.get_mut(&id) else {
                    return;
                };
                if window.layer == layer {
                    return;
                }
                let space_mode = self
                    .spaces
                    .get(&window.space)
                    .map(|space| space.mode)
                    .unwrap_or_default();
                let previous_mode = if window.layer == Layer::Above {
                    LayoutMode::Floating
                } else {
                    space_mode
                };
                window.saved_geometry.set(previous_mode, window.geometry);
                window.layer = layer;
                if self.dnd_hold.invalidate_window(&id) {
                    effects.push(CoreEffect::DndHoldChanged);
                }
                let space = window.space;
                effects.push(CoreEffect::LayerChanged {
                    id: id.clone(),
                    layer,
                });
                if !window.minimized {
                    effects.push(CoreEffect::SceneChanged {
                        id,
                        output: window.output.clone(),
                        old: Some(window.geometry),
                        new: Some(window.geometry),
                    });
                }
                self.repair_space_layer(space, effects);
            }
            CoreEvent::CommitGeometry { id, geometry } => {
                self.update_geometry(&id, geometry, effects)
            }
            CoreEvent::SetConstraints {
                id,
                minimum,
                maximum,
            } => {
                if let Some(window) = self.windows.get_mut(&id) {
                    window.min_size = minimum.sanitized();
                    window.max_size = maximum.sanitized();
                }
            }
            CoreEvent::SetLayoutMode { space, mode } => {
                self.ensure_space(space);
                let previous_mode = self.spaces[&space].mode;
                if previous_mode == mode {
                    return;
                }
                self.spaces.get_mut(&space).unwrap().mode = mode;
                effects.push(CoreEffect::LayoutModeChanged { space, mode });
                // A mode change selects the retained policy and its history.
                // Its resolved targets arrive through CommitGeometry after
                // platform size/transaction conversion. Reusing an old saved
                // rectangle here would bypass that policy and falsely dirty
                // a scene before any target had actually been configured.
            }
            CoreEvent::CaptureScrolling { space, snapshot } => {
                self.ensure_space(space);
                self.spaces.get_mut(&space).unwrap().history.scrolling = Some(snapshot);
            }
            CoreEvent::CaptureDwindle { space, snapshot } => {
                self.ensure_space(space);
                self.spaces.get_mut(&space).unwrap().history.dwindle = Some(snapshot);
            }
            CoreEvent::BeginMove { id, start } => {
                if self.interaction.is_some() {
                    return;
                }
                let Some(window) = self
                    .windows
                    .get(&id)
                    .filter(|window| window.attached && !window.minimized)
                else {
                    return;
                };
                let Some(controls) = MoveControls::new(start, window.layer == Layer::Above) else {
                    return;
                };
                Self::interaction_scene_changed(
                    &id,
                    window.output.clone(),
                    Some(window.geometry),
                    &window.output,
                    Some(window.geometry),
                    effects,
                );
                self.interaction = Some(MoveInteraction {
                    id,
                    controls,
                    phase: MovePhase::Starting,
                    origin_space: window.space,
                    origin_output: window.output.clone(),
                    origin_geometry: window.geometry,
                    output: window.output.clone(),
                    geometry: window.geometry,
                    pointer: start.pointer,
                });
            }
            CoreEvent::StartMoveMotion { id, delta, zoom } => {
                let Some(window) = self
                    .windows
                    .get(&id)
                    .filter(|window| window.attached && !window.minimized)
                else {
                    return;
                };
                let stage_open = self.overview.desired_open()
                    && self.overview.mode() == crate::overview::OverviewMode::Stage;
                let Some(interaction) = self.interaction.as_mut().filter(|interaction| {
                    interaction.id == id && interaction.phase == MovePhase::Starting
                }) else {
                    return;
                };
                if let Some(intent) = interaction.controls.starting_motion(
                    delta,
                    zoom,
                    stage_open,
                    window.layer == Layer::Above,
                ) {
                    if intent.detach {
                        interaction.phase = MovePhase::Moving;
                    }
                    effects.push(CoreEffect::MoveStartIntent(intent));
                }
            }
            CoreEvent::MoveDetached {
                id,
                width,
                full_width,
            } => {
                // Fullscreen restoration may change the registered layer
                // between the threshold decision and actual policy removal.
                let Some(window) = self.windows.get(&id).filter(|window| !window.minimized) else {
                    return;
                };
                let Some(interaction) = self.interaction.as_mut().filter(|interaction| {
                    interaction.id == id && interaction.phase == MovePhase::Moving
                }) else {
                    return;
                };
                if interaction.controls.acknowledge_detached(
                    width,
                    full_width,
                    window.layer == Layer::Above,
                ) {
                    effects.push(CoreEffect::MoveDetachedAccepted);
                }
            }
            CoreEvent::SetMoveFloating { id, floating } => {
                let Some(interaction) = self.interaction.as_mut().filter(|interaction| {
                    interaction.id == id && interaction.phase == MovePhase::Moving
                }) else {
                    return;
                };
                effects.push(CoreEffect::MovePlacementIntent(
                    interaction.controls.set_floating(floating),
                ));
            }
            CoreEvent::MovePose {
                id,
                output,
                geometry,
                pointer,
            } => {
                if !pointer.iter().all(|value| value.is_finite()) {
                    return;
                }
                let Some(interaction) = self.interaction.as_mut().filter(|interaction| {
                    interaction.id == id && interaction.phase == MovePhase::Moving
                }) else {
                    return;
                };
                let geometry = geometry.sanitized();
                let changed = interaction.phase != MovePhase::Moving
                    || interaction.output != output
                    || interaction.geometry != geometry;
                interaction.phase = MovePhase::Moving;
                let previous_output = std::mem::replace(&mut interaction.output, output);
                let previous = std::mem::replace(&mut interaction.geometry, geometry);
                interaction.pointer = pointer;
                if changed {
                    Self::interaction_scene_changed(
                        &id,
                        previous_output,
                        Some(previous),
                        &interaction.output,
                        Some(geometry),
                        effects,
                    );
                }
                self.change_focus(Some(id), effects);
            }
            CoreEvent::MovePlaced { id } => {
                if !self
                    .interaction
                    .as_ref()
                    .is_some_and(|interaction| interaction.id == id)
                {
                    return;
                }
                if self.stage_drag.end_move(&id) {
                    self.stage_drag_changed(effects);
                }
                let interaction = self.interaction.take().unwrap();
                if let Some(window) = self
                    .windows
                    .get(&id)
                    .filter(|window| window.attached && !window.minimized)
                {
                    Self::interaction_scene_changed(
                        &id,
                        interaction.output,
                        Some(interaction.geometry),
                        &window.output,
                        Some(window.geometry),
                        effects,
                    );
                    let space = window.space;
                    self.focus_suspended = false;
                    self.select_display_scope(space, effects);
                    if self.active_space != Some(space) {
                        let previous = self.active_space.replace(space);
                        effects.push(CoreEffect::SpaceActivated {
                            previous,
                            current: space,
                        });
                    }
                    self.change_focus(Some(id), effects);
                } else {
                    Self::interaction_scene_changed(
                        &id,
                        interaction.output,
                        Some(interaction.geometry),
                        &None,
                        None,
                        effects,
                    );
                    self.repair_focus(effects);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    struct WindowId(String);
    fn id(value: &str) -> WindowId {
        WindowId(value.to_owned())
    }
    fn geometry() -> Rect {
        Rect::new(10.0, 20.0, 300.0, 200.0)
    }
    fn core() -> DesktopCore<WindowId> {
        let mut core = DesktopCore::new();
        core.apply(CoreEvent::OutputConnected {
            connector: "DP-1".to_owned(),
            bounds: Rect::new(0.0, 0.0, 1920.0, 1080.0),
        });
        core.add_window(id("a"), SpaceId(1), Some("DP-1".to_owned()), geometry());
        core.add_window(id("b"), SpaceId(1), Some("DP-1".to_owned()), geometry());
        core
    }

    #[test]
    fn moving_pose_invalidates_both_connectors_and_end_restores_the_attached_scene() {
        let mut core = core();
        core.apply(CoreEvent::OutputRegistered {
            connector: "HDMI-1".into(),
            bounds: Rect::new(0., 0., 800., 600.),
        });
        assert!(core
            .apply(CoreEvent::BeginMove {
                id: id("a"),
                start: MoveStart::default()
            })
            .iter()
            .any(|effect| matches!(effect, CoreEffect::SceneChanged { .. })));
        core.apply(CoreEvent::StartMoveMotion {
            id: id("a"),
            delta: [256., 0.],
            zoom: 1.,
        });
        let first = Rect::new(40., 50., 300., 200.);
        let second = Rect::new(80., 90., 300., 200.);
        core.apply(CoreEvent::MovePose {
            id: id("a"),
            output: Some("DP-1".into()),
            geometry: first,
            pointer: [40., 50.],
        });
        let unchanged = core.apply(CoreEvent::MovePose {
            id: id("a"),
            output: Some("DP-1".into()),
            geometry: first,
            pointer: [41., 51.],
        });
        assert!(unchanged.is_empty());
        let cross = core.apply(CoreEvent::MovePose {
            id: id("a"),
            output: Some("HDMI-1".into()),
            geometry: second,
            pointer: [80., 90.],
        });
        assert!(cross.contains(&CoreEffect::SceneChanged {
            id: id("a"),
            output: Some("DP-1".into()),
            old: Some(first),
            new: None
        }));
        assert!(cross.contains(&CoreEffect::SceneChanged {
            id: id("a"),
            output: Some("HDMI-1".into()),
            old: None,
            new: Some(second)
        }));
        let end = core.apply(CoreEvent::MovePlaced { id: id("a") });
        assert!(end.contains(&CoreEffect::SceneChanged {
            id: id("a"),
            output: Some("HDMI-1".into()),
            old: Some(second),
            new: None
        }));
        assert!(end.contains(&CoreEffect::SceneChanged {
            id: id("a"),
            output: Some("DP-1".into()),
            old: None,
            new: Some(geometry())
        }));
        assert!(core.interaction().is_none());
    }

    #[test]
    fn destroying_a_detached_drag_invalidates_its_final_rendered_connector() {
        let mut core = core();
        core.apply(CoreEvent::BeginMove {
            id: id("a"),
            start: MoveStart::default(),
        });
        core.apply(CoreEvent::StartMoveMotion {
            id: id("a"),
            delta: [256., 0.],
            zoom: 1.,
        });
        core.detach_window(&id("a"));
        let pose = Rect::new(40., 50., 300., 200.);
        core.apply(CoreEvent::MovePose {
            id: id("a"),
            output: Some("DP-1".into()),
            geometry: pose,
            pointer: [40., 50.],
        });
        let effects = core.remove_window(&id("a"));
        assert!(effects.contains(&CoreEffect::SceneChanged {
            id: id("a"),
            output: Some("DP-1".into()),
            old: Some(pose),
            new: None
        }));
        assert!(core.interaction().is_none());
    }

    #[test]
    fn committed_geometry_damages_old_and_new_regions_and_updates_mode_history() {
        let mut core = core();
        let target = Rect::new(80., 90., 300., 200.);
        let mut effects = Vec::with_capacity(4);
        core.apply_into(
            CoreEvent::CommitGeometry {
                id: id("a"),
                geometry: target,
            },
            &mut effects,
        );
        assert_eq!(
            effects,
            vec![CoreEffect::SceneChanged {
                id: id("a"),
                output: Some("DP-1".into()),
                old: Some(geometry()),
                new: Some(target),
            }]
        );
        let window = core.window(&id("a")).unwrap();
        assert_eq!(window.geometry, target);
        assert_eq!(
            window.saved_geometry.get(LayoutMode::Scrolling),
            Some(target)
        );
        effects.clear();
        core.apply_into(
            CoreEvent::CommitGeometry {
                id: id("a"),
                geometry: target,
            },
            &mut effects,
        );
        assert!(effects.is_empty());
    }

    #[test]
    fn retained_effect_buffer_preserves_focus_notification_and_scene_order() {
        let mut core = core();
        core.focus_window(Some(id("a")));
        let target = Rect::new(40., 20., 300., 200.);
        let mut effects = core.focus_window(Some(id("b")));
        assert_eq!(
            effects,
            vec![CoreEffect::FocusChanged {
                previous: Some(id("a")),
                current: Some(id("b")),
            }]
        );
        core.apply_into(
            CoreEvent::CommitGeometry {
                id: id("a"),
                geometry: target,
            },
            &mut effects,
        );
        core.apply_into(
            CoreEvent::CommitGeometry {
                id: id("a"),
                geometry: geometry(),
            },
            &mut effects,
        );
        assert_eq!(effects.len(), 3);
        assert!(
            matches!(&effects[0], CoreEffect::FocusChanged { current: Some(current), .. } if current == &id("b"))
        );
        assert!(
            matches!(&effects[1], CoreEffect::SceneChanged { old: Some(old), new: Some(new), .. } if *old == geometry() && *new == target)
        );
        assert!(
            matches!(&effects[2], CoreEffect::SceneChanged { old: Some(old), new: Some(new), .. } if *old == target && *new == geometry())
        );
    }

    #[test]
    fn mode_round_trip_preserves_identity_and_history_until_solver_acknowledgement() {
        let mut core = core();
        core.focus_window(Some(id("a")));
        let scrolling = geometry();
        let floating = Rect::new(500., 400., 700., 600.);
        let dwindle = Rect::new(0., 0., 960., 1080.);
        let effects = core.set_layout_mode(SpaceId(1), LayoutMode::Floating);
        assert_eq!(
            effects,
            vec![CoreEffect::LayoutModeChanged {
                space: SpaceId(1),
                mode: LayoutMode::Floating
            }]
        );
        assert_eq!(core.window(&id("a")).unwrap().geometry, scrolling);
        core.apply(CoreEvent::CommitGeometry {
            id: id("a"),
            geometry: floating,
        });
        core.set_layout_mode(SpaceId(1), LayoutMode::Dwindle);
        assert_eq!(core.window(&id("a")).unwrap().geometry, floating);
        core.apply(CoreEvent::CommitGeometry {
            id: id("a"),
            geometry: dwindle,
        });
        core.set_layout_mode(SpaceId(1), LayoutMode::Scrolling);
        let window = core.window(&id("a")).unwrap();
        assert_eq!(window.geometry, dwindle);
        assert_eq!(
            window.saved_geometry.get(LayoutMode::Scrolling),
            Some(scrolling)
        );
        assert_eq!(
            window.saved_geometry.get(LayoutMode::Floating),
            Some(floating)
        );
        assert_eq!(
            window.saved_geometry.get(LayoutMode::Dwindle),
            Some(dwindle)
        );
        core.apply(CoreEvent::CommitGeometry {
            id: id("a"),
            geometry: scrolling,
        });
        core.set_layout_mode(SpaceId(1), LayoutMode::Floating);
        assert_eq!(core.window(&id("a")).unwrap().geometry, scrolling);
        core.apply(CoreEvent::CommitGeometry {
            id: id("a"),
            geometry: floating,
        });
        assert_eq!(core.window(&id("a")).unwrap().geometry, floating);
        assert_eq!(core.window(&id("a")).unwrap().space, SpaceId(1));
        assert_eq!(core.active_window(), Some(&id("a")));
    }

    #[test]
    fn unacknowledged_mode_reversal_does_not_invent_geometry_history_or_scene_damage() {
        let mut core = core();
        core.set_layout_mode(SpaceId(1), LayoutMode::Dwindle);
        let effects = core.set_layout_mode(SpaceId(1), LayoutMode::Scrolling);
        assert_eq!(
            effects,
            vec![CoreEffect::LayoutModeChanged {
                space: SpaceId(1),
                mode: LayoutMode::Scrolling
            }]
        );
        let window = core.window(&id("a")).unwrap();
        assert_eq!(window.geometry, geometry());
        assert_eq!(
            window.saved_geometry.get(LayoutMode::Scrolling),
            Some(geometry())
        );
        assert_eq!(window.saved_geometry.get(LayoutMode::Dwindle), None);
        assert_eq!(window.saved_geometry.get(LayoutMode::Floating), None);
    }

    #[test]
    fn disconnect_to_no_output_then_reconnect_keeps_destination_and_geometry() {
        let mut core = core();
        core.apply(CoreEvent::OutputDisconnected {
            connector: "DP-1".to_owned(),
        });
        let window = core.window(&id("a")).unwrap();
        assert_eq!(window.output, None);
        assert_eq!(window.preferred_output.as_deref(), Some("DP-1"));
        assert_eq!(window.geometry, geometry());
        core.apply(CoreEvent::OutputConnected {
            connector: "HDMI-1".to_owned(),
            bounds: Rect::new(0.0, 0.0, 800.0, 600.0),
        });
        assert_eq!(
            core.window(&id("a")).unwrap().output.as_deref(),
            Some("HDMI-1")
        );
        core.apply(CoreEvent::OutputConnected {
            connector: "DP-1".to_owned(),
            bounds: Rect::new(0.0, 0.0, 1920.0, 1080.0),
        });
        assert_eq!(
            core.window(&id("a")).unwrap().output.as_deref(),
            Some("DP-1")
        );
    }

    #[test]
    fn minimize_destroy_and_move_repair_focus_without_stale_ids() {
        let mut core = core();
        core.focus_window(Some(id("a")));
        core.focus_window(Some(id("b")));
        core.set_minimized(&id("b"), true);
        assert_eq!(core.active_window(), Some(&id("a")));
        assert!(!core
            .space(SpaceId(1))
            .unwrap()
            .focused
            .as_ref()
            .is_some_and(|focused| focused == &id("b")));
        core.move_window(&id("a"), SpaceId(2), Some("DP-1".to_owned()));
        assert_eq!(core.active_window(), None);
        core.focus_window(Some(id("a")));
        assert_eq!(core.active_space(), Some(SpaceId(2)));
        core.remove_window(&id("a"));
        assert_eq!(core.active_window(), None);
        assert_eq!(core.space(SpaceId(2)).unwrap().focused, None);
    }

    #[test]
    fn unchanged_acknowledgements_generate_no_scene_or_state_effect() {
        let mut core = core();
        assert!(core
            .apply(CoreEvent::CommitGeometry {
                id: id("a"),
                geometry: geometry()
            })
            .is_empty());
        assert!(core.set_minimized(&id("a"), false).is_empty());
        assert!(core
            .set_layout_mode(SpaceId(1), LayoutMode::Scrolling)
            .is_empty());
        assert!(core.focus_window(Some(id("missing"))).is_empty());
        assert_eq!(
            core.windows()
                .map(|window| window.id.0.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );
    }

    #[test]
    fn minimization_effect_damages_the_correct_visibility_edge() {
        let mut core = core();
        let effects = core.set_minimized(&id("a"), true);
        assert!(effects.iter().any(|effect| matches!(
            effect,
            CoreEffect::SceneChanged {
                old: Some(_),
                new: None,
                ..
            }
        )));
        let effects = core.set_minimized(&id("a"), false);
        assert!(effects.iter().any(|effect| matches!(
            effect,
            CoreEffect::SceneChanged {
                old: None,
                new: Some(_),
                ..
            }
        )));
    }

    #[test]
    fn retained_floating_policy_applies_constraints_before_geometry_acknowledgement() {
        use crate::floating::{FloatingPolicy, Placement};
        use crate::layout::LiveLayoutPolicy;
        let mut core = DesktopCore::<u64>::new();
        let bounds = Rect::new(0., 0., 800., 600.);
        core.apply(CoreEvent::OutputRegistered {
            connector: "DP-1".into(),
            bounds,
        });
        for window in [1, 2] {
            core.add_window(window, SpaceId(1), Some("DP-1".into()), geometry());
            core.apply(CoreEvent::SetLayer {
                id: window,
                layer: Layer::Above,
            });
        }
        core.set_minimized(&2, true);
        core.apply(CoreEvent::SetConstraints {
            id: 1,
            minimum: Size::new(400., 200.),
            maximum: Size::default(),
        });
        let mut floating = FloatingPolicy::new();
        for id in [1, 2, 999] {
            let Some(window) = core.window(&id).filter(|window| {
                window.attached && !window.minimized && window.space == SpaceId(1)
            }) else {
                continue;
            };
            let size = crate::geometry::constrain_size(
                window.geometry.size(),
                window.min_size,
                window.max_size,
            );
            floating.insert(
                id,
                Placement::new(bounds, size, [window.geometry.x, window.geometry.y], false),
                0,
                true,
                &[],
            );
        }
        let mut effects = Vec::new();
        floating.arrange(bounds, |id, geometry| {
            core.apply_into(
                CoreEvent::CommitGeometry { id: *id, geometry },
                &mut effects,
            )
        });
        assert_eq!(
            core.window(&1).unwrap().geometry,
            Rect::new(10., 20., 400., 200.)
        );
        assert_eq!(core.window(&2).unwrap().geometry, geometry());
        assert_eq!(floating.len(), 1);
        assert_eq!(effects.len(), 1);
        assert!(matches!(
            &effects[0],
            CoreEffect::SceneChanged { id: 1, .. }
        ));
    }

    #[test]
    fn removing_workspace_moves_identity_and_activates_fallback() {
        let mut core = core();
        core.focus_window(Some(id("a")));
        core.apply(CoreEvent::RemoveSpace {
            id: SpaceId(1),
            fallback: SpaceId(9),
        });
        assert_eq!(core.active_space(), Some(SpaceId(9)));
        assert_eq!(core.active_window(), Some(&id("a")));
        assert!(core.space(SpaceId(1)).is_none());
        assert!(core.windows().all(|window| window.space == SpaceId(9)));
    }

    #[test]
    fn detached_identity_is_not_reassigned_by_connector_events() {
        let mut core = core();
        core.focus_window(Some(id("a")));
        core.detach_window(&id("a"));
        core.apply(CoreEvent::OutputDisconnected {
            connector: "DP-1".to_owned(),
        });
        core.apply(CoreEvent::OutputConnected {
            connector: "DP-1".to_owned(),
            bounds: Rect::new(0.0, 0.0, 1600.0, 900.0),
        });
        let window = core.window(&id("a")).unwrap();
        assert!(!window.attached);
        assert!(window.output.is_none());
        assert_eq!(window.preferred_output.as_deref(), Some("DP-1"));
        core.move_window(&id("a"), SpaceId(2), Some("DP-1".to_owned()));
        assert!(core.window(&id("a")).unwrap().attached);
    }

    #[test]
    fn retained_bsp_arrangement_never_overwrites_floating_saved_targets() {
        use crate::bsp::{BspArrangement, BspTree};
        use crate::layout::LiveLayoutPolicy;
        let mut core = core();
        core.apply(CoreEvent::SetLayer {
            id: id("b"),
            layer: Layer::Above,
        });
        let floating = Rect::new(300., 100., 450., 250.);
        core.apply(CoreEvent::CommitGeometry {
            id: id("b"),
            geometry: floating,
        });
        core.set_layout_mode(SpaceId(1), LayoutMode::Dwindle);
        let bounds = Rect::new(0., 0., 800., 600.);
        let mut tree = BspTree::new(bounds, 0., 64);
        for window in core
            .windows()
            .filter(|window| window.attached && !window.minimized && window.layer != Layer::Above)
        {
            tree.insert(window.id.clone(), None, None);
        }
        tree.arrange(BspArrangement { bounds, gap: 0. }, |id, geometry| {
            core.apply(CoreEvent::CommitGeometry {
                id: id.clone(),
                geometry,
            });
        });
        assert_eq!(core.window(&id("a")).unwrap().geometry, bounds);
        assert_eq!(core.window(&id("b")).unwrap().geometry, floating);
        assert_eq!(
            core.window(&id("b"))
                .unwrap()
                .saved_geometry
                .get(LayoutMode::Floating),
            Some(floating)
        );
    }

    #[test]
    fn inactive_scope_keeps_local_memory_without_reactivating_on_local_changes() {
        let mut core = core();
        core.focus_window(Some(id("a")));
        core.apply(CoreEvent::DeactivateSpace);
        core.set_space_focus(SpaceId(1), Some(id("b")), false);
        core.apply(CoreEvent::CommitGeometry {
            id: id("b"),
            geometry: Rect::new(200.0, 10.0, 300.0, 250.0),
        });
        core.remove_window(&id("a"));
        assert_eq!(core.active_space(), None);
        assert_eq!(core.active_window(), None);
        assert_eq!(core.space(SpaceId(1)).unwrap().focused, Some(id("b")));
        core.apply(CoreEvent::ActivateSpace { id: SpaceId(1) });
        assert_eq!(core.active_window(), Some(&id("b")));
    }

    #[test]
    fn activating_space_honors_an_explicitly_empty_focus_selection() {
        let mut core = core();
        core.focus_window(Some(id("a")));
        core.set_space_focus(SpaceId(1), None, false);
        core.apply(CoreEvent::ActivateSpace { id: SpaceId(1) });
        assert_eq!(core.active_window(), None);
        assert_eq!(core.space(SpaceId(1)).unwrap().focused, None);
    }

    #[test]
    fn moving_focus_does_not_pollute_its_detached_origin_space() {
        let mut core = core();
        core.focus_window(Some(id("a")));
        core.apply(CoreEvent::BeginMove {
            id: id("a"),
            start: MoveStart::default(),
        });
        core.apply(CoreEvent::StartMoveMotion {
            id: id("a"),
            delta: [256., 0.],
            zoom: 1.,
        });
        let pose = || CoreEvent::MovePose {
            id: id("a"),
            output: Some("DP-1".to_owned()),
            geometry: geometry(),
            pointer: [100.0, 100.0],
        };
        core.apply(pose());
        core.detach_window(&id("a"));
        core.set_space_focus(SpaceId(1), Some(id("b")), false);
        core.apply(pose());
        core.apply(CoreEvent::ActivateSpace { id: SpaceId(1) });
        assert_eq!(core.active_window(), Some(&id("a")));
        assert_eq!(core.space(SpaceId(1)).unwrap().focused, Some(id("b")));
        core.apply(CoreEvent::OutputDisconnected {
            connector: "DP-1".to_owned(),
        });
        core.apply(CoreEvent::DeactivateSpace);
        core.remove_window(&id("b"));
        assert_eq!(core.active_window(), None);
        assert_eq!(core.space(SpaceId(1)).unwrap().focused, None);
    }
}
