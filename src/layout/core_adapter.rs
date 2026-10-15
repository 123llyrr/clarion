//! Device handles stay in the layout adapter; identity and desktop decisions
//! belong to `realand-core` and are shared by every output and workspace.

use std::cell::{Ref, RefCell, RefMut};
use std::collections::HashMap;
use std::hash::Hash;
use std::ops::Deref;
use std::rc::Rc;

use realand_core::{CoreEffect, CoreEvent, DesktopCore, LayoutMode, Rect, SpaceId};
use smithay::output::Output;
use smithay::utils::{Logical, Rectangle};

/// A request to reconcile final core focus with the platform's lock, layer,
/// popup and capture overrides. It never directly commands a seat target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CoreProtocolOp {
    ReconcileFocus {
        previous: Option<u64>,
        current: Option<u64>,
    },
}

#[derive(Default)]
struct CoreResponse {
    space_name_changed: bool,
    overview: Option<realand_core::overview::OverviewOutcome>,
    spread_changed: bool,
    stage_drag_changed: bool,
    stage_restore: Option<realand_core::stage_drag::StageRestoreIntent<u64>>,
    workspace_switch: Option<realand_core::workspace_switch::WorkspaceSwitchOutcome>,
    minimize: Option<realand_core::workspace_state::MinimizeIntent<u64>>,
    canvas: Option<realand_core::workspace_state::CanvasTransition<u64>>,
    management: Option<realand_core::management::ManagementOutcome<u64>>,
    scratchpad_return: Option<SpaceId>,
    dnd_activate: Option<realand_core::dnd_hold::DndActivateIntent<u64>>,
    move_start: Option<realand_core::move_controls::MoveStartIntent>,
    move_detached: bool,
    move_placement: Option<realand_core::move_controls::MovePlacement>,
}

pub(super) struct CoreAdapter<I> {
    core: RefCell<DesktopCore<u64>>,
    /// Platform identity lives exclusively in this device-facing adapter.
    ids: RefCell<HashMap<I, u64>>,
    retired: RefCell<Vec<SpaceId>>,
    effects: RefCell<Vec<CoreEffect<u64>>>,
    scene_outputs: RefCell<Vec<String>>,
    protocol_ops: RefCell<Vec<CoreProtocolOp>>,
}

impl<I: Eq + Hash> CoreAdapter<I> {
    fn apply_retired(&self, core: &mut DesktopCore<u64>) {
        for id in self.retired.take() {
            self.apply_to(core, CoreEvent::ForgetSpace { id });
        }
    }
    fn flush_retired(&self) {
        if self.retired.borrow().is_empty() {
            return;
        }
        if let Ok(mut core) = self.core.try_borrow_mut() {
            self.apply_retired(&mut core);
        }
    }
    pub(super) fn borrow(&self) -> Ref<'_, DesktopCore<u64>> {
        self.flush_retired();
        self.core.borrow()
    }
    pub(super) fn borrow_mut(&self) -> CoreMutation<'_, I> {
        let mut core = self.core.borrow_mut();
        self.apply_retired(&mut core);
        CoreMutation {
            adapter: self,
            core,
        }
    }
    fn apply_to(&self, core: &mut DesktopCore<u64>, event: CoreEvent<u64>) -> CoreResponse {
        let mut effects = self.effects.borrow_mut();
        debug_assert!(effects.is_empty());
        core.apply_into(event, &mut effects);
        let mut outputs = self.scene_outputs.borrow_mut();
        let mut response = CoreResponse::default();
        for effect in effects.drain(..) {
            let output = match effect {
                CoreEffect::SpaceNameChanged { .. } => {
                    response.space_name_changed = true;
                    None
                }
                CoreEffect::ManagementChanged { outcome } => {
                    response.management = Some(outcome);
                    None
                }
                CoreEffect::ScratchpadReturnTaken(space) => {
                    response.scratchpad_return = space;
                    None
                }
                CoreEffect::DndHoldChanged => None,
                CoreEffect::DndActivateIntent(intent) => {
                    response.dnd_activate = Some(intent);
                    None
                }
                CoreEffect::MoveStartIntent(intent) => {
                    response.move_start = Some(intent);
                    None
                }
                CoreEffect::MoveDetachedAccepted => {
                    response.move_detached = true;
                    None
                }
                CoreEffect::MovePlacementIntent(intent) => {
                    response.move_placement = Some(intent);
                    None
                }
                CoreEffect::StageDragChanged => {
                    response.stage_drag_changed = true;
                    None
                }
                CoreEffect::StageRestoreIntent(intent) => {
                    response.stage_restore = Some(intent);
                    None
                }
                CoreEffect::WorkspaceSwitchChanged { outcome } => {
                    response.workspace_switch = Some(outcome);
                    None
                }
                CoreEffect::SpreadChanged => {
                    response.spread_changed = true;
                    None
                }
                CoreEffect::OverviewChanged { outcome } => {
                    response.overview = Some(outcome);
                    None
                }
                CoreEffect::MinimizeIntent(intent) => {
                    response.minimize = Some(intent);
                    None
                }
                CoreEffect::CanvasChanged { transition, .. } => {
                    response.canvas = Some(transition);
                    None
                }
                CoreEffect::FocusChanged { previous, current } => {
                    let mut ops = self.protocol_ops.borrow_mut();
                    if let Some(CoreProtocolOp::ReconcileFocus {
                        current: pending, ..
                    }) = ops.last_mut()
                    {
                        *pending = current;
                    } else {
                        ops.push(CoreProtocolOp::ReconcileFocus { previous, current });
                    }
                    None
                }
                CoreEffect::SceneChanged {
                    output: Some(output),
                    ..
                }
                | CoreEffect::OutputSceneChanged { connector: output } => Some(output),
                // These are observations of core state/acknowledged payload
                // transitions. They are not client protocol commands. Real
                // minimize/canvas/overview intents are extracted above; focus
                // is reconciled against final state at the seat boundary.
                CoreEffect::SceneChanged { output: None, .. }
                | CoreEffect::SpaceLayerFocusChanged { .. }
                | CoreEffect::WindowMapped { .. }
                | CoreEffect::WindowRemoved { .. }
                | CoreEffect::WindowMoved { .. }
                | CoreEffect::WindowOutputChanged { .. }
                | CoreEffect::MinimizedChanged { .. }
                | CoreEffect::LayerChanged { .. }
                | CoreEffect::LayoutModeChanged { .. }
                | CoreEffect::SpaceActivated { .. }
                | CoreEffect::OutputChanged { .. } => None,
            };
            if let Some(output) = output {
                if !outputs.contains(&output) {
                    outputs.push(output);
                }
            }
            // CommitGeometry acknowledges targets already converted by the
            // permanent Tile/client transaction adapter; mode changes cannot
            // request a second configure with an old saved rectangle.
        }
        response
    }
    pub(super) fn drain_scene_outputs(&self, output: &mut Vec<String>) {
        output.extend(self.scene_outputs.borrow_mut().drain(..));
    }
    pub(super) fn drain_protocol_ops(&self, ops: &mut Vec<CoreProtocolOp>) {
        // Retirement can repair focus and enqueue its final target. Do this
        // before taking the notification batch, even if no query preceded it.
        self.flush_retired();
        ops.extend(self.protocol_ops.borrow_mut().drain(..));
    }
    pub(super) fn retire_space(&self, id: SpaceId) {
        if !self.retired.borrow().contains(&id) {
            self.retired.borrow_mut().push(id);
        }
        self.flush_retired();
    }
    pub(super) fn id(&self, platform: &I) -> Option<u64> {
        self.ids.borrow().get(platform).copied()
    }
    pub(super) fn register(&self, platform: I, id: u64) {
        self.ids.borrow_mut().insert(platform, id);
    }
    pub(super) fn unregister(&self, platform: &I) {
        self.ids.borrow_mut().remove(platform);
    }
}

/// Finite mutation guard. Shared query access cannot bypass effect capture.
pub(super) struct CoreMutation<'a, I> {
    adapter: &'a CoreAdapter<I>,
    core: RefMut<'a, DesktopCore<u64>>,
}
impl<I> Deref for CoreMutation<'_, I> {
    type Target = DesktopCore<u64>;
    fn deref(&self) -> &Self::Target {
        &self.core
    }
}
impl<I: Eq + Hash> CoreMutation<'_, I> {
    pub(super) fn apply(&mut self, event: CoreEvent<u64>) {
        self.adapter.apply_to(&mut self.core, event);
    }
    pub(super) fn management_action(
        &mut self,
        action: realand_core::management::ManagementAction<u64>,
    ) -> realand_core::management::ManagementOutcome<u64> {
        self.adapter
            .apply_to(&mut self.core, CoreEvent::Management(action))
            .management
            .unwrap_or_default()
    }
    pub(super) fn take_scratchpad_return(&mut self) -> Option<SpaceId> {
        self.adapter
            .apply_to(&mut self.core, CoreEvent::TakeScratchpadReturn)
            .scratchpad_return
    }
    pub(super) fn dnd_hold_action(
        &mut self,
        action: realand_core::dnd_hold::DndHoldAction<u64>,
    ) -> Option<realand_core::dnd_hold::DndActivateIntent<u64>> {
        self.adapter
            .apply_to(&mut self.core, CoreEvent::DndHold(action))
            .dnd_activate
    }
    pub(super) fn start_move_motion(
        &mut self,
        id: u64,
        delta: [f64; 2],
        zoom: f64,
    ) -> Option<realand_core::move_controls::MoveStartIntent> {
        self.adapter
            .apply_to(
                &mut self.core,
                CoreEvent::StartMoveMotion { id, delta, zoom },
            )
            .move_start
    }
    pub(super) fn acknowledge_move_detached(
        &mut self,
        id: u64,
        width: realand_core::policy_memory::Width,
        full_width: bool,
    ) -> bool {
        self.adapter
            .apply_to(
                &mut self.core,
                CoreEvent::MoveDetached {
                    id,
                    width,
                    full_width,
                },
            )
            .move_detached
    }
    pub(super) fn set_move_floating(
        &mut self,
        id: u64,
        floating: bool,
    ) -> Option<realand_core::move_controls::MovePlacement> {
        self.adapter
            .apply_to(&mut self.core, CoreEvent::SetMoveFloating { id, floating })
            .move_placement
    }
    pub(super) fn stage_drag_action(
        &mut self,
        action: realand_core::stage_drag::StageDragAction<u64>,
    ) -> bool {
        self.adapter
            .apply_to(&mut self.core, CoreEvent::StageDrag(action))
            .stage_drag_changed
    }
    pub(super) fn take_stage_drag_restore(
        &mut self,
        window: u64,
    ) -> Option<realand_core::stage_drag::StageRestoreIntent<u64>> {
        self.adapter
            .apply_to(
                &mut self.core,
                CoreEvent::StageDrag(realand_core::stage_drag::StageDragAction::Take { window }),
            )
            .stage_restore
    }
    pub(super) fn workspace_switch_action(
        &mut self,
        action: realand_core::workspace_switch::WorkspaceSwitchAction,
    ) -> realand_core::workspace_switch::WorkspaceSwitchOutcome {
        self.adapter
            .apply_to(&mut self.core, CoreEvent::WorkspaceSwitch(action))
            .workspace_switch
            .unwrap_or_default()
    }
    pub(super) fn spread_action(
        &mut self,
        action: realand_core::spread::SpreadAction<u64>,
    ) -> bool {
        self.adapter
            .apply_to(&mut self.core, CoreEvent::Spread(action))
            .spread_changed
    }
    pub(super) fn window_metadata_changed(&mut self, id: u64, search_text: Option<String>) {
        self.apply(CoreEvent::WindowMetadataChanged { id, search_text });
    }
    pub(super) fn set_space_output_preference(&mut self, space: SpaceId, selector: String) {
        self.apply(CoreEvent::SetSpaceOutputPreference { space, selector });
    }
    pub(super) fn set_space_name(&mut self, space: SpaceId, name: Option<String>) -> bool {
        self.adapter
            .apply_to(&mut self.core, CoreEvent::SetSpaceName { space, name })
            .space_name_changed
    }
    pub(super) fn prefer_unnamed_space_output(&mut self, space: SpaceId, output: &Output) {
        self.apply(CoreEvent::PreferUnnamedSpaceOutput {
            space,
            connector: output.name(),
        });
    }
    pub(super) fn prefer_space_output(&mut self, space: SpaceId, output: &Output) {
        self.apply(CoreEvent::PreferSpaceOutput {
            space,
            connector: output.name(),
        });
    }
    pub(super) fn normalize_space_output_preference(&mut self, space: SpaceId, output: &Output) {
        self.apply(CoreEvent::NormalizeSpaceOutputPreference {
            space,
            connector: output.name(),
        });
    }
    pub(super) fn overview_action(
        &mut self,
        action: realand_core::overview::OverviewAction,
    ) -> realand_core::overview::OverviewOutcome {
        self.adapter
            .apply_to(&mut self.core, CoreEvent::Overview(action))
            .overview
            .unwrap_or_default()
    }
    pub(super) fn set_space_layer_focus(
        &mut self,
        space: SpaceId,
        focus: realand_core::workspace_state::SpaceLayerFocus,
    ) {
        self.apply(CoreEvent::SetSpaceLayerFocus { space, focus });
    }
    pub(super) fn repair_space_layer(&mut self, space: SpaceId) {
        self.apply(CoreEvent::RepairSpaceLayer { space });
    }
    pub(super) fn capture_canvas_policy(
        &mut self,
        space: SpaceId,
        policy: realand_core::workspace_state::CanvasPolicyMemory<u64>,
    ) {
        self.apply(CoreEvent::CaptureCanvasPolicy { space, policy });
    }
    pub(super) fn set_canvas_mode(
        &mut self,
        space: SpaceId,
        enabled: bool,
    ) -> realand_core::workspace_state::CanvasTransition<u64> {
        self.adapter
            .apply_to(&mut self.core, CoreEvent::SetCanvasMode { space, enabled })
            .canvas
            .unwrap_or(realand_core::workspace_state::CanvasTransition {
                changed: false,
                enabled,
                restore_tiled: Vec::new(),
                restore_policy: None,
            })
    }
    pub(super) fn set_canvas_origin(&mut self, id: u64, tiled: bool) {
        self.apply(CoreEvent::SetCanvasOrigin { id, tiled });
    }
    pub(super) fn minimize_window(
        &mut self,
        id: u64,
        restoration: realand_core::workspace_state::MinimizeRestore<u64>,
    ) -> Option<realand_core::workspace_state::MinimizeIntent<u64>> {
        self.adapter
            .apply_to(
                &mut self.core,
                CoreEvent::MinimizeWindow { id, restoration },
            )
            .minimize
    }
    pub(super) fn restore_window(
        &mut self,
        id: u64,
        columns: Vec<realand_core::workspace_state::RestoreColumn<u64>>,
    ) -> Option<realand_core::workspace_state::MinimizeIntent<u64>> {
        self.adapter
            .apply_to(&mut self.core, CoreEvent::RestoreWindow { id, columns })
            .minimize
    }
    pub(super) fn add_window(
        &mut self,
        id: u64,
        space: SpaceId,
        output: Option<String>,
        geometry: Rect,
    ) {
        let mut window = realand_core::WindowRecord::new(id, space, geometry);
        window.output = output.clone();
        window.preferred_output = output;
        self.apply(CoreEvent::MapWindow { window });
    }
    pub(super) fn remove_window(&mut self, id: &u64) {
        self.apply(CoreEvent::UnmapWindow { id: *id });
    }
    pub(super) fn detach_window(&mut self, id: &u64) {
        self.apply(CoreEvent::DetachWindow { id: *id });
    }
    pub(super) fn set_output(&mut self, id: &u64, output: Option<String>, remember: bool) {
        self.apply(CoreEvent::SetOutput {
            id: *id,
            output,
            remember,
        });
    }
    pub(super) fn set_space_focus(&mut self, space: SpaceId, id: Option<u64>, activate: bool) {
        self.apply(CoreEvent::SetSpaceFocus {
            space,
            id,
            activate,
        });
    }
    pub(super) fn attach_space(&mut self, id: SpaceId, connector: &str, index: usize) {
        self.apply(CoreEvent::AttachSpace {
            id,
            connector: connector.to_owned(),
            index,
        });
    }
    pub(super) fn detach_space(&mut self, id: SpaceId) {
        self.apply(CoreEvent::DetachSpace { id });
    }
    pub(super) fn reorder_space(&mut self, id: SpaceId, index: usize) {
        self.apply(CoreEvent::ReorderSpace { id, index });
    }
    pub(super) fn reorder_detached_space(&mut self, id: SpaceId, index: usize) {
        self.apply(CoreEvent::ReorderDetachedSpace { id, index });
    }
    pub(super) fn select_output(&mut self, connector: &str) {
        self.apply(CoreEvent::SelectOutput {
            connector: connector.to_owned(),
        });
    }
    pub(super) fn select_output_space(
        &mut self,
        connector: &str,
        id: SpaceId,
        remember_previous: bool,
    ) {
        self.apply(CoreEvent::SelectOutputSpace {
            connector: connector.to_owned(),
            id,
            remember_previous,
        });
    }
    pub(super) fn set_previous_space(&mut self, connector: &str, id: Option<SpaceId>) {
        self.apply(CoreEvent::SetPreviousSpace {
            connector: connector.to_owned(),
            id,
        });
    }
    pub(super) fn take_remembered_space(&mut self, connector: &str) -> Option<SpaceId> {
        self.core.take_remembered_space(connector)
    }
    pub(super) fn set_minimized(&mut self, id: &u64, minimized: bool) {
        self.apply(CoreEvent::SetMinimized { id: *id, minimized });
    }
    pub(super) fn move_window(&mut self, id: &u64, space: SpaceId, output: Option<String>) {
        self.apply(CoreEvent::MoveWindow {
            id: *id,
            space,
            output,
        });
    }
    pub(super) fn set_layout_mode(&mut self, space: SpaceId, mode: LayoutMode) {
        self.apply(CoreEvent::SetLayoutMode { space, mode });
    }
    pub(super) fn commit_geometry(&mut self, id: &u64, geometry: Rect) {
        self.apply(CoreEvent::CommitGeometry { id: *id, geometry });
    }
    pub(super) fn capture_scrolling(
        &mut self,
        space: SpaceId,
        snapshot: realand_core::policy_memory::ScrollingSnapshot<u64>,
    ) {
        self.apply(CoreEvent::CaptureScrolling { space, snapshot });
    }
    pub(super) fn capture_dwindle(
        &mut self,
        space: SpaceId,
        snapshot: realand_core::bsp::BspSnapshot<u64>,
    ) {
        self.apply(CoreEvent::CaptureDwindle { space, snapshot });
    }
}

impl<I> std::fmt::Debug for CoreAdapter<I> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoreAdapter").finish_non_exhaustive()
    }
}

pub(super) type DesktopContext<I> = Rc<CoreAdapter<I>>;

pub(super) fn new_context<I: Clone + Eq + Hash>() -> DesktopContext<I> {
    Rc::new(CoreAdapter {
        core: RefCell::new(DesktopCore::new()),
        ids: RefCell::new(HashMap::new()),
        retired: RefCell::new(Vec::new()),
        effects: RefCell::new(Vec::with_capacity(4)),
        scene_outputs: RefCell::new(Vec::new()),
        protocol_ops: RefCell::new(Vec::with_capacity(1)),
    })
}

pub(super) fn rect(rectangle: Rectangle<f64, Logical>) -> Rect {
    Rect::new(
        rectangle.loc.x,
        rectangle.loc.y,
        rectangle.size.w,
        rectangle.size.h,
    )
}

pub(super) fn create_space<I: Clone + Eq + Hash>(
    id: SpaceId,
    output: Option<&Output>,
) -> DesktopContext<I> {
    let context = new_context();
    context.borrow_mut().apply(CoreEvent::CreateSpace {
        id,
        mode: LayoutMode::Scrolling,
    });
    if let Some(output) = output {
        connect_output(&context, output);
    }
    context
}

pub(super) fn connect_output<I: Clone + Eq + Hash>(context: &DesktopContext<I>, output: &Output) {
    let size = crate::utils::output_size(output);
    let mut core = context.borrow_mut();
    core.apply(CoreEvent::OutputRegistered {
        connector: output.name(),
        bounds: Rect::new(0.0, 0.0, size.w, size.h),
    });
    core.apply(CoreEvent::OutputIdentityUpdated {
        connector: output.name(),
        identity: output_identity(output),
    });
}

pub(super) fn output_identity(output: &Output) -> realand_core::output_identity::OutputIdentity {
    let facts = output
        .user_data()
        .get::<realand_config::OutputName>()
        .unwrap();
    realand_core::output_identity::OutputIdentity {
        connector: facts.connector.clone(),
        make: facts.make.clone(),
        model: facts.model.clone(),
        serial: facts.serial.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protocol_focus_drain_flushes_deferred_retirement_before_choosing_target() {
        let context = new_context::<u64>();
        let geometry = Rect::new(0., 0., 100., 80.);
        {
            let mut core = context.borrow_mut();
            core.add_window(1, SpaceId(1), None, geometry);
            core.add_window(2, SpaceId(2), None, geometry);
            core.apply(CoreEvent::FocusWindow { id: Some(1) });
            core.detach_window(&1);
            core.set_space_focus(SpaceId(2), Some(2), false);
        }
        let mut ops = Vec::new();
        context.drain_protocol_ops(&mut ops);
        ops.clear();
        let reader = context.borrow();
        context.retire_space(SpaceId(1));
        assert_eq!(reader.active_space(), Some(SpaceId(1)));
        drop(reader);
        // No core query before this drain: it must run pending retirement.
        context.drain_protocol_ops(&mut ops);
        assert_eq!(
            ops,
            [CoreProtocolOp::ReconcileFocus {
                previous: None,
                current: Some(2)
            }]
        );
        assert_eq!(context.borrow().active_window(), Some(&2));
        assert!(context.borrow().space(SpaceId(1)).is_none());
        ops.clear();
        context.drain_protocol_ops(&mut ops);
        assert!(ops.is_empty());
    }
    #[test]
    fn unconsumed_focus_notifications_are_bounded_and_reconcile_final_core_once() {
        let context = new_context::<u64>();
        {
            let mut core = context.borrow_mut();
            for id in [1, 2] {
                core.add_window(id, SpaceId(1), None, Rect::new(0., 0., 10., 10.));
            }
            core.apply(CoreEvent::FocusWindow { id: Some(1) });
        }
        let mut ops = Vec::new();
        context.drain_protocol_ops(&mut ops);
        ops.clear();
        for _ in 0..2048 {
            let mut core = context.borrow_mut();
            core.apply(CoreEvent::FocusWindow { id: Some(2) });
            core.apply(CoreEvent::FocusWindow { id: Some(1) });
        }
        context
            .borrow_mut()
            .apply(CoreEvent::FocusWindow { id: Some(2) });
        assert_eq!(context.protocol_ops.borrow().len(), 1);
        context.drain_protocol_ops(&mut ops);
        assert_eq!(
            ops,
            [CoreProtocolOp::ReconcileFocus {
                previous: Some(1),
                current: Some(2)
            }]
        );
        ops.clear();
        context.drain_protocol_ops(&mut ops);
        assert!(ops.is_empty());
    }
    #[test]
    fn interaction_cross_output_and_final_cleanup_are_drained_once() {
        let context = new_context::<u64>();
        let rect = Rect::new(10., 20., 100., 80.);
        {
            let mut core = context.borrow_mut();
            for connector in ["DP-1", "HDMI-1"] {
                core.apply(CoreEvent::OutputRegistered {
                    connector: connector.into(),
                    bounds: Rect::new(0., 0., 1920., 1080.),
                });
            }
            core.add_window(1, SpaceId(1), Some("DP-1".into()), rect);
        }
        let mut outputs = Vec::new();
        context.drain_scene_outputs(&mut outputs);
        outputs.clear();
        context.borrow_mut().apply(CoreEvent::BeginMove {
            id: 1,
            start: realand_core::move_controls::MoveStart::default(),
        });
        context.drain_scene_outputs(&mut outputs);
        assert_eq!(outputs, ["DP-1"]);
        outputs.clear();
        context.borrow_mut().start_move_motion(1, [256., 0.], 1.);
        let pose = CoreEvent::MovePose {
            id: 1,
            output: Some("HDMI-1".into()),
            geometry: rect,
            pointer: [10., 20.],
        };
        context.borrow_mut().apply(pose.clone());
        context.drain_scene_outputs(&mut outputs);
        assert_eq!(outputs, ["DP-1", "HDMI-1"]);
        outputs.clear();
        context.borrow_mut().apply(pose);
        context.drain_scene_outputs(&mut outputs);
        assert!(outputs.is_empty());
        context.borrow_mut().apply(CoreEvent::MovePlaced { id: 1 });
        context.drain_scene_outputs(&mut outputs);
        assert_eq!(outputs, ["HDMI-1", "DP-1"]);
        outputs.clear();
        context.drain_scene_outputs(&mut outputs);
        assert!(outputs.is_empty());
    }

    #[test]
    fn changed_targets_drain_each_connector_once_and_unchanged_targets_do_not_queue() {
        let context = new_context::<u64>();
        let initial = Rect::new(10., 20., 100., 80.);
        {
            let mut core = context.borrow_mut();
            core.apply(CoreEvent::OutputRegistered {
                connector: "DP-1".into(),
                bounds: Rect::new(0., 0., 1920., 1080.),
            });
            core.add_window(1, SpaceId(1), Some("DP-1".into()), initial);
        }
        let mut outputs = Vec::new();
        context.drain_scene_outputs(&mut outputs);
        assert_eq!(outputs, ["DP-1"]);
        outputs.clear();
        {
            let mut core = context.borrow_mut();
            core.commit_geometry(&1, Rect::new(11., 20., 100., 80.));
            core.commit_geometry(&1, Rect::new(12., 20., 100., 80.));
        }
        context.drain_scene_outputs(&mut outputs);
        assert_eq!(outputs, ["DP-1"]);
        outputs.clear();
        context
            .borrow_mut()
            .commit_geometry(&1, Rect::new(12., 20., 100., 80.));
        context.drain_scene_outputs(&mut outputs);
        assert!(outputs.is_empty());
        context.drain_scene_outputs(&mut outputs);
        assert!(outputs.is_empty());
    }
}
