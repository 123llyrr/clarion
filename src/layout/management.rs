use super::*;
use clarion_core::management::{
    Arrangement, ArrangementSizing, ArrangementSpace, ArrangementWindow, Group as CoreGroup,
    ManagementAction,
};
use clarion_core::policy_memory::{ColumnDisplay as CoreDisplay, Height, Width};
use clarion_core::{LayoutMode, SpaceId};
use clarion_ipc::management::*;
use std::collections::HashMap;

fn core_display(display: clarion_ipc::ColumnDisplay) -> CoreDisplay {
    match display {
        clarion_ipc::ColumnDisplay::Normal => CoreDisplay::Normal,
        clarion_ipc::ColumnDisplay::Tabbed => CoreDisplay::Tabbed,
        clarion_ipc::ColumnDisplay::Dwindle => CoreDisplay::Dwindle,
    }
}
fn ipc_display(display: CoreDisplay) -> clarion_ipc::ColumnDisplay {
    match display {
        CoreDisplay::Normal => clarion_ipc::ColumnDisplay::Normal,
        CoreDisplay::Tabbed => clarion_ipc::ColumnDisplay::Tabbed,
        CoreDisplay::Dwindle => clarion_ipc::ColumnDisplay::Dwindle,
    }
}
fn core_sizing(sizing: SessionSizing) -> ArrangementSizing {
    ArrangementSizing {
        width: if sizing.proportional {
            Width::Proportion(sizing.width)
        } else {
            Width::Fixed(sizing.width)
        },
        full_width: sizing.full_width,
        height: match sizing.height {
            SessionHeight::Auto(value) => Height::Auto(value),
            SessionHeight::Fixed(value) => Height::Fixed(value),
            SessionHeight::Preset(index) => Height::Preset(index),
        },
    }
}
fn ipc_sizing(sizing: ArrangementSizing) -> SessionSizing {
    let (width, proportional) = match sizing.width {
        Width::Proportion(width) => (width, true),
        Width::Fixed(width) => (width, false),
    };
    SessionSizing {
        width,
        proportional,
        full_width: sizing.full_width,
        height: match sizing.height {
            Height::Auto(value) => SessionHeight::Auto(value),
            Height::Fixed(value) => SessionHeight::Fixed(value),
            Height::Preset(index) => SessionHeight::Preset(index),
        },
    }
}
/// Serialization is a permanent platform boundary. Stable IDs become file
/// slots only here; missing live clients never reorder the captured slots.
fn encode_arrangement(arrangement: &Arrangement<u64>) -> SessionLayout {
    let windows: HashMap<_, _> = arrangement
        .windows
        .iter()
        .enumerate()
        .map(|(index, window)| (window.id, index))
        .collect();
    let spaces: HashMap<_, _> = arrangement
        .spaces
        .iter()
        .enumerate()
        .map(|(index, space)| (space.id, index))
        .collect();
    fn tree(
        node: &clarion_core::layout::BspNode<u64>,
        windows: &HashMap<u64, usize>,
    ) -> Option<SessionTree> {
        match node {
            clarion_core::layout::BspNode::Leaf(id) => {
                windows.get(id).copied().map(SessionTree::Leaf)
            }
            clarion_core::layout::BspNode::Split {
                axis,
                ratio,
                first,
                second,
            } => match (tree(first, windows), tree(second, windows)) {
                (Some(first), Some(second)) => Some(SessionTree::Split {
                    vertical: *axis == clarion_core::Axis::Vertical,
                    ratio: *ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (Some(node), None) | (None, Some(node)) => Some(node),
                (None, None) => None,
            },
        }
    }
    SessionLayout {
        version: 1,
        workspaces: arrangement
            .spaces
            .iter()
            .map(|space| SessionWorkspace {
                name: space.name.clone(),
                index: space.index,
                output: space.output.clone(),
                mode: match space.mode {
                    LayoutMode::Dwindle => clarion_ipc::TilingMode::Dwindle,
                    _ => clarion_ipc::TilingMode::Scrolling,
                },
                tree: space.tree.as_ref().and_then(|node| tree(node, &windows)),
            })
            .collect(),
        windows: arrangement
            .windows
            .iter()
            .map(|window| SessionWindow {
                app_id: window.app_id.clone(),
                title: window.title.clone(),
                workspace: spaces[&window.space],
                column: window.column,
                display: ipc_display(window.display),
                sizing: window.sizing.map(ipc_sizing),
                floating: window.floating,
                size: (window.size[0], window.size[1]),
                position: window.position.map(|position| (position[0], position[1])),
            })
            .collect(),
        groups: arrangement
            .groups
            .iter()
            .map(|group| SessionGroup {
                name: group.name.clone(),
                members: group
                    .members
                    .iter()
                    .filter_map(|id| windows.get(id).copied())
                    .collect(),
            })
            .filter(|group| !group.members.is_empty())
            .collect(),
    }
}

impl<W: LayoutElement> Layout<W> {
    fn arrangement(&self) -> Arrangement<u64> {
        let mut spaces = Vec::new();
        let mut windows = Vec::new();
        for (monitor, index, workspace) in self
            .workspaces()
            .filter(|(_, _, workspace)| workspace.has_windows_or_name())
        {
            let space = SpaceId(workspace.id().get());
            for (tile, geometry) in workspace.tiles_with_ipc_layouts() {
                let (app_id, title) = tile.window().session_identity();
                let column = geometry
                    .pos_in_scrolling_layout
                    .map(|(column, _)| column - 1);
                let display = column
                    .and_then(|index| workspace.scrolling().columns().nth(index))
                    .map(|column| column.display_mode())
                    .unwrap_or(clarion_ipc::ColumnDisplay::Normal);
                let sizing = geometry.pos_in_scrolling_layout.and_then(|(column, row)| {
                    workspace
                        .scrolling()
                        .columns()
                        .nth(column - 1)
                        .map(|column| core_sizing(column.session_sizing(row - 1)))
                });
                let size = tile.window_expected_or_current_size().to_i32_round();
                windows.push(ArrangementWindow {
                    id: tile.window().core_id(),
                    app_id,
                    title,
                    space,
                    column,
                    display: core_display(display),
                    sizing,
                    floating: workspace.is_floating(tile.window().id()),
                    size: [size.w, size.h],
                    position: geometry.tile_pos_in_workspace_view.map(|(x, y)| [x, y]),
                });
            }
            spaces.push(ArrangementSpace {
                id: space,
                name: workspace.name().map(|name| (*name).clone()),
                index,
                output: monitor.map(|monitor| monitor.output().name()),
                mode: match workspace.tiling_mode() {
                    clarion_ipc::TilingMode::Dwindle => LayoutMode::Dwindle,
                    clarion_ipc::TilingMode::Scrolling => LayoutMode::Scrolling,
                },
                tree: workspace.dwindle.policy_snapshot().topology,
            });
        }
        let core = self.core.borrow();
        Arrangement {
            spaces,
            windows,
            groups: core.management().groups().to_vec(),
            focus: core.active_window().copied(),
        }
    }
    pub fn session_layout(&self) -> SessionLayout {
        encode_arrangement(&self.arrangement())
    }
    pub fn management_status(&self) -> (Vec<WindowGroup>, usize, usize) {
        let core = self.core.borrow();
        let groups = core
            .management()
            .groups()
            .iter()
            .map(|group| WindowGroup {
                name: group.name.clone(),
                // Production core IDs are the public stable MappedId values.
                windows: group.members.clone(),
            })
            .collect();
        let (undo, redo) = core.management().history_steps();
        (groups, undo, redo)
    }
    pub fn checkpoint_layout(&mut self) {
        if self.interactive_move.is_some() {
            return;
        }
        let current = self.arrangement();
        self.core
            .borrow_mut()
            .management_action(ManagementAction::Checkpoint(current));
    }
    pub fn undo_layout(&mut self, redo: bool) -> bool {
        if self.interactive_move.is_some() {
            return false;
        }
        let current = self.arrangement();
        let result = self
            .core
            .borrow_mut()
            .management_action(ManagementAction::Undo { redo, current });
        let Some(snapshot) = result.restore else {
            return false;
        };
        self.apply_arrangement(snapshot);
        true
    }
    pub fn set_window_group(&mut self, name: String, ids: Vec<W::Id>) {
        let members = ids.iter().filter_map(|id| self.core.id(id)).collect();
        let current = self.arrangement();
        self.core
            .borrow_mut()
            .management_action(ManagementAction::SetGroup {
                name,
                members,
                current,
            });
    }
    pub fn remove_window_group(&mut self, name: &str) {
        let current = self.arrangement();
        self.core
            .borrow_mut()
            .management_action(ManagementAction::RemoveGroup {
                name: name.to_owned(),
                current,
            });
    }
    pub fn move_window_group(&mut self, name: &str, target: WorkspaceId) {
        let current = self.arrangement();
        let result = self
            .core
            .borrow_mut()
            .management_action(ManagementAction::MoveGroup {
                name: name.to_owned(),
                target: SpaceId(target.get()),
                current,
            });
        if let Some(intent) = result.move_group {
            self.move_group_members(&intent.members, intent.target);
        }
    }
    fn move_group_members(&mut self, ids: &[u64], target: SpaceId) {
        let target = WorkspaceId::specific(target.0);
        for core_id in ids {
            let Some(id) = self
                .window_for_core_id(*core_id)
                .map(|window| window.id().clone())
            else {
                continue;
            };
            let Some((index, workspace)) = self.find_workspace_by_id(target) else {
                break;
            };
            if workspace.has_window(&id) {
                continue;
            }
            if let Some(output) = workspace.current_output().cloned() {
                self.move_to_output(Some(&id), &output, Some(index), ActivateWindow::No);
            } else {
                let Some(removed) = self.detach_window(&id, Transaction::new()) else {
                    continue;
                };
                let Some(workspace) = self
                    .workspaces_mut()
                    .find(|workspace| workspace.id() == target)
                else {
                    break;
                };
                workspace.add_tile(
                    removed.tile,
                    WorkspaceAddWindowTarget::Auto,
                    ActivateWindow::No,
                    removed.width,
                    removed.is_full_width,
                    removed.is_floating,
                    None,
                );
            }
        }
    }
    pub(super) fn finish_group_drag(&mut self, id: &W::Id) {
        let Some(window) = self.core.id(id) else {
            return;
        };
        let target = self
            .core
            .borrow()
            .window(&window)
            .map(|window| window.space);
        let Some(target) = target else {
            return;
        };
        let result = self
            .core
            .borrow_mut()
            .management_action(ManagementAction::FollowGroup { window, target });
        if let Some(intent) = result.move_group {
            self.move_group_members(&intent.members, intent.target);
        }
    }
    /// Match identities in pure policy, then apply stable slots through live handles.
    pub fn restore_session_layout(&mut self, session: SessionLayout) -> Result<usize, String> {
        session.validate()?;
        if self.interactive_move.is_some() {
            return Err("请先结束窗口拖动".into());
        }
        let mut handles = HashMap::new();
        let available: Vec<_> = self
            .windows()
            .map(|(_, window)| {
                let id = window.core_id();
                handles.entry(id).or_insert_with(|| window.id().clone());
                (id, window.session_identity())
            })
            .collect();
        let saved: Vec<_> = session
            .windows
            .iter()
            .map(|window| (window.app_id.as_str(), window.title.as_str()))
            .collect();
        let matched: Vec<_> =
            clarion_core::session_match::match_session_windows(&available, &saved)
                .into_iter()
                .map(|id| id.and_then(|id| handles.get(&id).cloned()))
                .collect();
        let count = matched.iter().filter(|id| id.is_some()).count();
        self.checkpoint_layout();
        self.apply_session(&session, &matched, &[], None);
        Ok(count)
    }
    fn apply_arrangement(&mut self, snapshot: Arrangement<u64>) {
        let session = encode_arrangement(&snapshot);
        let handles: Vec<_> = snapshot
            .windows
            .iter()
            .map(|saved| {
                self.window_for_core_id(saved.id)
                    .map(|window| window.id().clone())
            })
            .collect();
        let spaces: Vec<_> = snapshot
            .spaces
            .iter()
            .map(|space| WorkspaceId::specific(space.id.0))
            .collect();
        let focus = snapshot.focus.and_then(|id| {
            self.window_for_core_id(id)
                .map(|window| window.id().clone())
        });
        self.apply_session(&session, &handles, &spaces, focus);
    }
    fn apply_session(
        &mut self,
        session: &SessionLayout,
        handles: &[Option<W::Id>],
        original_ws: &[WorkspaceId],
        focus: Option<W::Id>,
    ) {
        let mut targets = Vec::new();
        for (idx, saved) in session.workspaces.iter().enumerate() {
            let restore_name = saved.name.clone().unwrap_or_else(|| {
                let base = format!("恢复-{}", idx + 1);
                let mut name = base.clone();
                let mut suffix = 2;
                // Reserve explicit names even before their workspaces have been created.
                while session.workspaces.iter().any(|ws| {
                    ws.name
                        .as_ref()
                        .is_some_and(|saved| saved.eq_ignore_ascii_case(&name))
                }) {
                    name = format!("{base}-{suffix}");
                    suffix += 1;
                }
                name
            });
            let existing = original_ws
                .get(idx)
                .and_then(|id| self.find_workspace_by_id(*id).map(|(_, ws)| ws.id()))
                .or_else(|| {
                    self.find_workspace_by_name(&restore_name)
                        .map(|(_, ws)| ws.id())
                })
                .filter(|id| !targets.contains(id));
            let id = existing.unwrap_or_else(|| {
                let base = restore_name;
                let mut name = base.clone();
                let mut suffix = 2;
                while self.find_workspace_by_name(&name).is_some() {
                    name = format!("{base}-{suffix}");
                    suffix += 1;
                }
                self.ensure_named_workspace(&WorkspaceConfig {
                    name: clarion_config::WorkspaceName(name.clone()),
                    open_on_output: saved.output.clone(),
                    layout: None,
                });
                self.find_workspace_by_name(&name)
                    .map(|(_, ws)| ws.id())
                    .unwrap_or_else(|| {
                        self.active_workspace()
                            .map(|ws| ws.id())
                            .unwrap_or_default()
                    })
            });
            if let Some(name) = &saved.output {
                let target = self.outputs().find(|out| &out.name() == name).cloned();
                if let Some(output) = target {
                    if let Some((index, ws)) = self.find_workspace_by_id(id) {
                        let old = ws.current_output().cloned();
                        if old.as_ref() != Some(&output) {
                            self.move_workspace_to_output_by_id(index, old, &output);
                        }
                    }
                }
            }
            targets.push(id);
        }
        if !original_ws.is_empty() {
            for id in &targets {
                if let Some(ws) = self.workspaces_mut().find(|ws| ws.id() == *id) {
                    ws.set_name(None);
                }
            }
            for (i, id) in targets.iter().enumerate() {
                let name = session.workspaces[i]
                    .name
                    .clone()
                    .filter(|name| self.find_workspace_by_name(name).is_none());
                if let Some(ws) = self.workspaces_mut().find(|ws| ws.id() == *id) {
                    ws.set_name(name);
                }
            }
        }
        // Detach only captured, still-live windows; new windows are retained untouched.
        let mut removed = Vec::new();
        for id in handles {
            let tile = id.as_ref().and_then(|id| {
                self.workspaces_mut()
                    .find(|ws| ws.has_window(id))
                    .map(|ws| ws.remove_tile(id, Transaction::new()))
            });
            removed.push(tile);
        }
        for (idx, saved) in session.workspaces.iter().enumerate() {
            let Some(ws) = self.workspaces_mut().find(|ws| ws.id() == targets[idx]) else {
                continue;
            };
            ws.set_tiling_mode(saved.mode);
            let mut columns: Vec<(usize, usize)> = Vec::new();
            for (i, window) in session
                .windows
                .iter()
                .enumerate()
                .filter(|(_, w)| w.workspace == idx)
            {
                let Some(tile) = removed.get_mut(i).and_then(Option::take) else {
                    continue;
                };
                let id = tile.tile.window().id().clone();
                if !window.floating && saved.mode == clarion_ipc::TilingMode::Scrolling {
                    let saved_col = window.column.unwrap_or(i);
                    if let Some((_, col)) =
                        columns.iter().find(|(original, _)| *original == saved_col)
                    {
                        ws.add_tile_to_column(*col, None, tile.tile, false);
                    } else {
                        let col = ws.scrolling().columns().count();
                        ws.add_tile(
                            tile.tile,
                            WorkspaceAddWindowTarget::NewColumnAt(col),
                            ActivateWindow::No,
                            tile.width,
                            tile.is_full_width,
                            false,
                            None,
                        );
                        columns.push((saved_col, col));
                    }
                    ws.activate_window(&id);
                    ws.set_column_display(window.display);
                } else {
                    ws.add_tile(
                        tile.tile,
                        WorkspaceAddWindowTarget::Auto,
                        ActivateWindow::No,
                        tile.width,
                        tile.is_full_width,
                        window.floating,
                        None,
                    );
                }
                ws.set_window_width(Some(&id), clarion_ipc::SizeChange::SetFixed(window.size.0));
                ws.set_window_height(Some(&id), clarion_ipc::SizeChange::SetFixed(window.size.1));
                if window.floating {
                    if let Some((x, y)) = window.position {
                        // IPC stores output-local positions; SetFixed is relative to the
                        // working area, which may start below or beside a layer-shell panel.
                        let origin = ws.working_area().loc;
                        ws.move_floating_window(
                            Some(&id),
                            clarion_ipc::PositionChange::SetFixed(x - origin.x),
                            clarion_ipc::PositionChange::SetFixed(y - origin.y),
                            false,
                        );
                    }
                }
            }
            let sizing = session
                .windows
                .iter()
                .enumerate()
                .filter(|(_, w)| w.workspace == idx)
                .filter_map(|(i, w)| Some((handles.get(i)?.as_ref()?.clone(), w.sizing.clone()?)))
                .collect::<Vec<_>>();
            ws.scrolling_mut().restore_session_sizing(&sizing);
            if let Some(tree) = &saved.tree {
                ws.dwindle.restore_session_tree(tree, handles);
            }
            ws.commit_core_geometry();
            ws.record_core_focus(false);
        }
        // Direct tile insertion must maintain the monitor's reserved empty edge workspaces.
        for mon in self.monitors_mut() {
            if mon
                .workspaces
                .last()
                .is_some_and(|ws| ws.has_windows_or_name())
            {
                mon.add_workspace_bottom();
            }
            if mon.options.layout.empty_workspace_above_first
                && mon
                    .workspaces
                    .first()
                    .is_some_and(|ws| ws.has_windows_or_name())
            {
                mon.add_workspace_top();
            }
            if mon.workspace_switch.is_none() {
                mon.clean_up_workspaces();
            }
        }
        // A captured unnamed workspace may have lost all of its windows while the
        // outputs were disconnected. It has no reserved empty slot in NoOutputs.
        if let MonitorSet::NoOutputs { workspaces } = &mut self.monitor_set {
            workspaces.retain(|ws| ws.has_windows_or_name());
        }
        for (i, target) in targets.iter().enumerate() {
            if let Some((index, ws)) = self.find_workspace_by_id(*target) {
                let output = ws.current_output().cloned();
                let saved_index = session.workspaces[i].index;
                if let MonitorSet::NoOutputs { workspaces } = &mut self.monitor_set {
                    // Recreated workspaces are inserted at the front. Preserve their
                    // saved order before outputs return, just as on a live monitor.
                    let new_index = saved_index.min(workspaces.len() - 1);
                    let workspace = workspaces.remove(index);
                    let id = clarion_core::SpaceId(workspace.id().get());
                    workspaces.insert(new_index, workspace);
                    self.core.borrow_mut().reorder_detached_space(id, new_index);
                } else {
                    self.move_workspace_to_idx(Some((output, index)), saved_index);
                }
            }
        }
        let groups = session
            .groups
            .iter()
            .map(|group| CoreGroup {
                name: group.name.clone(),
                members: group
                    .members
                    .iter()
                    .filter_map(|index| {
                        let id = handles.get(*index)?.as_ref()?;
                        self.core.id(id)
                    })
                    .collect(),
            })
            .collect();
        self.core
            .borrow_mut()
            .management_action(ManagementAction::ReplaceGroups(groups));
        if let Some(id) = focus {
            if self.windows().any(|(_, w)| w.id() == &id) {
                self.activate_window(&id);
            }
        }
        self.commit_core_focus_scope();
    }
}
