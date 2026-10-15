//! Apply pure Stage restoration intents to the physical Tile and scene pose.
use super::*;
use realand_core::stage_drag::{
    StageCapture, StageDragAction, StageRestoreIntent, StageRestorePosition,
};

impl<W: LayoutElement> Layout<W> {
    pub(super) fn stage_drag_source_workspace(&self) -> Option<WorkspaceId> {
        self.core
            .borrow()
            .stage_drag_source()
            .map(|id| WorkspaceId::specific(id.0))
    }

    pub(super) fn capture_stage_drag_origin(&mut self, window: &W::Id) {
        if !self.is_stage_open() {
            return;
        }
        let capture = self.workspaces().find_map(|(_, _, workspace)| {
            let tile = workspace
                .tiles()
                .find(|tile| tile.window().id() == window)?;
            let floating = workspace.is_floating(window);
            let floating_position = if floating {
                workspace
                    .floating()
                    .tiles_with_offsets()
                    .find(|(tile, _)| tile.window().id() == window)
                    .map(|(_, position)| workspace.floating_logical_to_size_frac(position))
            } else {
                tile.floating_pos
            };
            Some(StageCapture {
                window: tile.window().core_id(),
                columns: workspace.scrolling().policy_snapshot().columns,
                bsp: workspace
                    .dwindle
                    .has_window(window)
                    .then(|| workspace.dwindle.policy_snapshot()),
                floating_position: floating_position.map(|position| [position.x, position.y]),
                floating_size: if floating {
                    tile.window().expected_size().or(tile.floating_window_size)
                } else {
                    tile.floating_window_size
                }
                .map(|size| [size.w, size.h]),
                floating_width: tile.tile_expected_or_current_size().w,
                restore_to_floating: tile.restore_to_floating,
                fullscreen: tile.window().pending_sizing_mode().is_fullscreen(),
                maximized: tile.window().pending_sizing_mode().is_maximized(),
            })
        });
        if let Some(capture) = capture {
            self.core
                .borrow_mut()
                .stage_drag_action(StageDragAction::Capture(capture));
        }
    }

    pub(super) fn discard_stage_drag_origin(&mut self, window: &W::Id) {
        let Some(id) = self.core.id(window) else {
            return;
        };
        if !self
            .core
            .borrow_mut()
            .stage_drag_action(StageDragAction::Discard { window: id })
        {
            return;
        }
        match &mut self.monitor_set {
            MonitorSet::Normal { monitors, .. } => {
                for monitor in monitors {
                    if monitor.workspace_switch.is_none() {
                        monitor.clean_up_workspaces();
                    }
                }
            }
            MonitorSet::NoOutputs { workspaces } => {
                workspaces.retain(Workspace::has_windows_or_name)
            }
        }
    }

    fn restore_stage_metadata(
        workspace: &mut Workspace<W>,
        window: &W::Id,
        intent: &StageRestoreIntent<u64>,
    ) {
        if let Some(snapshot) = &intent.bsp {
            workspace.dwindle.restore_stage_topology(snapshot);
        }
        workspace.activate_window(window);
        if let Some(display) = intent.display {
            workspace.set_column_display(match display {
                realand_core::policy_memory::ColumnDisplay::Normal => ColumnDisplay::Normal,
                realand_core::policy_memory::ColumnDisplay::Tabbed => ColumnDisplay::Tabbed,
                realand_core::policy_memory::ColumnDisplay::Dwindle => ColumnDisplay::Dwindle,
            });
        }
        if intent.maximized {
            workspace.set_maximized(window, true);
        }
        if intent.fullscreen {
            workspace.set_fullscreen(window, true);
        }
        if let Some(tile) = workspace
            .tiles_mut()
            .find(|tile| tile.window().core_id() == intent.window)
        {
            tile.restore_to_floating = intent.restore_to_floating;
            tile.floating_pos = intent.floating_position.map(|[x, y]| (x, y).into());
            tile.floating_window_size = intent
                .floating_size
                .map(|[width, height]| (width, height).into());
        }
    }

    /// Return true only when this hook completed the move itself.
    pub(super) fn finish_stage_drag(&mut self, window: &W::Id, cancel: bool) -> bool {
        let Some(core_id) = self.core.id(window) else {
            return false;
        };
        if self.core.borrow().stage_drag().window() != Some(&core_id) {
            return false;
        }
        let Some(state) = &self.interactive_move else {
            self.discard_stage_drag_origin(window);
            return false;
        };
        match state {
            InteractiveMoveState::Starting { window_id, .. } => {
                if window_id != window {
                    return false;
                }
                self.discard_stage_drag_origin(window);
                if cancel {
                    self.interactive_move_end(window);
                }
                return cancel;
            }
            InteractiveMoveState::Moving(move_) => {
                if move_.tile.window().id() != window {
                    return false;
                }
                if !cancel
                    && self.monitor_for_output(&move_.output).is_some_and(|mon| {
                        mon.stage_drop_target(move_.pointer_pos_within_output)
                            .is_some()
                    })
                {
                    // Keep the source pinned until the normal drop has inserted
                    // the tile, including a drop onto its own now-empty card.
                    return false;
                }
            }
        }
        let needs_destination = self.core.borrow().stage_drag_destination().is_none();
        if needs_destination {
            if let MonitorSet::NoOutputs { workspaces } = &mut self.monitor_set {
                if workspaces.is_empty() {
                    workspaces.push(
                        Workspace::new_no_outputs(self.clock.clone(), self.options.clone())
                            .with_core(self.core.clone()),
                    );
                }
            }
        }
        let Some(destination) = self.core.borrow().stage_drag_destination() else {
            return false;
        };
        let columns = self
            .workspaces()
            .find(|(_, _, workspace)| workspace.id().get() == destination.0)
            .map(|(_, _, workspace)| {
                workspace
                    .scrolling()
                    .columns()
                    .map(|column| realand_core::workspace_state::RestoreColumn {
                        id: column.id().get(),
                        windows: column
                            .tiles()
                            .map(|(tile, _)| tile.window().core_id())
                            .collect(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        if !self
            .core
            .borrow_mut()
            .stage_drag_action(StageDragAction::Resolve {
                window: core_id,
                destination,
                columns,
            })
        {
            return false;
        }
        let origin = self
            .core
            .borrow_mut()
            .take_stage_drag_restore(core_id)
            .expect("resolved Stage restoration is consumed once");
        let zoom = self.interactive_move_zoom();
        let Some(InteractiveMoveState::Moving(mut move_)) = self.interactive_move.take() else {
            unreachable!();
        };
        let render_location = move_.tile_render_location(zoom);
        let render_output = move_.output.clone();
        for mon in self.monitors_mut() {
            mon.dnd_scroll_gesture_end();
        }
        for ws in self.workspaces_mut() {
            ws.dnd_scroll_gesture_end();
        }
        move_.tile.finish_wobble();
        move_.tile.stop_move_animations();
        move_.tile.interactive_move_offset = Point::default();
        move_.tile.animate_alpha(
            INTERACTIVE_MOVE_ALPHA,
            1.,
            self.options.animations.window_movement.0,
        );
        move_.tile.floating_pos = origin.floating_position.map(|[x, y]| (x, y).into());
        move_.tile.floating_window_size = origin
            .floating_size
            .map(|[width, height]| (width, height).into());
        move_.tile.window().output_leave(&move_.output);
        match &mut self.monitor_set {
            MonitorSet::Normal { monitors, .. } => {
                let mon_index = monitors
                    .iter()
                    .position(|monitor| {
                        monitor
                            .idx_of_ws(WorkspaceId::specific(origin.space.0))
                            .is_some()
                    })
                    .expect("core restore destination must have a physical workspace");
                let mon = &mut monitors[mon_index];
                let ws_index = mon
                    .idx_of_ws(WorkspaceId::specific(origin.space.0))
                    .unwrap();
                let ws_id = mon.workspaces[ws_index].id();
                let position = origin.position;
                if let StageRestorePosition::InColumn(col, tile_idx) = position {
                    mon.add_tile_to_column(ws_index, col, Some(tile_idx), move_.tile, true, false);
                } else {
                    mon.add_tile(
                        move_.tile,
                        MonitorAddWindowTarget::Workspace {
                            id: ws_id,
                            column_idx: match position {
                                StageRestorePosition::NewColumn(idx) => Some(idx),
                                _ => None,
                            },
                        },
                        ActivateWindow::Yes,
                        false,
                        origin.width,
                        origin.full_width,
                        origin.floating,
                        None,
                    );
                }
                let ws_index = mon.idx_of_ws(ws_id).unwrap();
                Self::restore_stage_metadata(&mut mon.workspaces[ws_index], window, &origin);
                if mon.output() == &render_output {
                    let geo = mon.workspaces_render_geo().nth(ws_index).unwrap();
                    // Hotplug can move the source beyond the visible card range.
                    // Its zero-sized placeholder cannot provide an animation
                    // scale; the next visible frame will render the restored tile.
                    if geo.size.w > 0. && geo.size.h > 0. {
                        let zoom = mon.workspace_render_zoom(geo).max(0.01);
                        if let Some((tile, pos)) = mon.workspaces[ws_index]
                            .tiles_with_render_positions_mut(false)
                            .find(|(tile, _)| tile.window().id() == window)
                        {
                            tile.animate_move_from(
                                (render_location - geo.loc).downscale(zoom) - pos,
                            );
                        }
                    }
                }
            }
            MonitorSet::NoOutputs { workspaces } => {
                if workspaces.is_empty() {
                    workspaces.push(
                        Workspace::new_no_outputs(self.clock.clone(), self.options.clone())
                            .with_core(self.core.clone()),
                    );
                }
                let idx = workspaces
                    .iter()
                    .position(|workspace| workspace.id().get() == origin.space.0)
                    .expect("core detached restore destination must have a physical workspace");
                let ws = &mut workspaces[idx];
                let position = origin.position;
                if let StageRestorePosition::InColumn(col, tile_idx) = position {
                    ws.add_tile_to_column(col, Some(tile_idx), move_.tile, true);
                } else {
                    ws.add_tile(
                        move_.tile,
                        match position {
                            StageRestorePosition::NewColumn(idx) => {
                                WorkspaceAddWindowTarget::NewColumnAt(idx)
                            }
                            _ => WorkspaceAddWindowTarget::Auto,
                        },
                        ActivateWindow::Yes,
                        origin.width,
                        origin.full_width,
                        origin.floating,
                        None,
                    );
                }
                Self::restore_stage_metadata(ws, window, &origin);
            }
        }
        self.core
            .borrow_mut()
            .stage_drag_action(StageDragAction::Finish { window: core_id });
        for mon in self.monitors_mut() {
            if mon.workspace_switch.is_none() {
                mon.clean_up_workspaces();
            }
        }
        if let MonitorSet::NoOutputs { workspaces } = &mut self.monitor_set {
            workspaces.retain(Workspace::has_windows_or_name);
        }
        true
    }
}
