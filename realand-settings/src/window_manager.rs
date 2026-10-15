use std::collections::{HashMap, HashSet};

use eframe::egui::{self, Button, RichText, ScrollArea, TextEdit, Ui};
use realand_ipc::{Action, Window, Workspace, WorkspaceReferenceArg};

#[derive(Default)]
pub struct WindowManager {
    query: String,
    workspace_filter: Option<u64>,
    destinations: HashMap<u64, u64>,
    selected: HashSet<u64>,
    group_name: String,
    group_target: Option<u64>,
}

fn workspace_label(workspace: &Workspace) -> String {
    format!(
        "{} · {}{}",
        workspace.output.as_deref().unwrap_or("未连接显示器"),
        workspace.idx,
        workspace
            .name
            .as_ref()
            .map(|name| format!(" · {name}"))
            .unwrap_or_default(),
    )
}

fn matches_search(window: &Window, query: &str) -> bool {
    let title = window.title.as_deref().unwrap_or("").to_lowercase();
    let app = window.app_id.as_deref().unwrap_or("").to_lowercase();
    query.split_whitespace().all(|word| {
        let word = word.to_lowercase();
        title.contains(&word) || app.contains(&word)
    })
}

fn move_action(window_id: u64, workspace_id: u64) -> Action {
    Action::MoveWindowToWorkspace {
        window_id: Some(window_id),
        reference: WorkspaceReferenceArg::Id(workspace_id),
        focus: false,
    }
}

impl WindowManager {
    pub fn management_ui(
        &mut self,
        ui: &mut Ui,
        state: Option<&realand_ipc::management::ManagementState>,
        workspaces: &[Workspace],
    ) -> Option<Action> {
        let Some(state) = state else {
            ui.label("分组和撤销需要更新合成器后重新登录。");
            return None;
        };
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    state.undo_steps > 0,
                    Button::new(format!("撤销布局 ({})", state.undo_steps)),
                )
                .clicked()
            {
                action = Some(Action::UndoLayout {});
            }
            if ui
                .add_enabled(
                    state.redo_steps > 0,
                    Button::new(format!("重做 ({})", state.redo_steps)),
                )
                .clicked()
            {
                action = Some(Action::RedoLayout {});
            }
        });
        ui.horizontal(|ui| {
            ui.add(
                TextEdit::singleline(&mut self.group_name).hint_text("分组名称，例如 开发 / 沟通"),
            );
            if ui
                .add_enabled(
                    !self.selected.is_empty()
                        && !self.group_name.trim().is_empty()
                        && self.group_name.len() <= 128,
                    Button::new(format!("保存分组 · {} 个窗口", self.selected.len())),
                )
                .clicked()
            {
                let mut windows: Vec<_> = self.selected.iter().copied().collect();
                windows.sort_unstable();
                action = Some(Action::SetWindowGroup {
                    name: self.group_name.trim().into(),
                    windows,
                });
                self.selected.clear();
            }
        });
        if !state.groups.is_empty() {
            ui.label(
                "拖动分组名称到下方工作区按钮；在 Expo 中拖动任一成员，也会将整组带到目标工作区。",
            );
        }
        if self
            .group_target
            .is_none_or(|id| !workspaces.iter().any(|ws| ws.id == id))
        {
            self.group_target = workspaces.first().map(|ws| ws.id);
        }
        for group in &state.groups {
            ui.horizontal_wrapped(|ui| {
                ui.dnd_drag_source(
                    egui::Id::new(("group", &group.name)),
                    group.name.clone(),
                    |ui| {
                        ui.label(
                            RichText::new(format!(
                                "{} · {} 个窗口",
                                group.name,
                                group.windows.len()
                            ))
                            .strong(),
                        );
                    },
                );
                if ui.button("编辑成员").clicked() {
                    self.group_name = group.name.clone();
                    self.selected = group.windows.iter().copied().collect();
                }
                egui::ComboBox::from_id_salt(("group_target", &group.name))
                    .selected_text(
                        self.group_target
                            .and_then(|id| workspaces.iter().find(|ws| ws.id == id))
                            .map(workspace_label)
                            .unwrap_or_default(),
                    )
                    .show_ui(ui, |ui| {
                        for ws in workspaces {
                            ui.selectable_value(
                                &mut self.group_target,
                                Some(ws.id),
                                workspace_label(ws),
                            );
                        }
                    });
                if ui.button("整组移动").clicked() {
                    if let Some(workspace_id) = self.group_target {
                        action = Some(Action::MoveWindowGroup {
                            name: group.name.clone(),
                            workspace_id,
                        });
                    }
                }
                if ui.button("解散").clicked() {
                    action = Some(Action::RemoveWindowGroup {
                        name: group.name.clone(),
                    });
                }
            });
        }
        action
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        windows: &[Window],
        workspaces: &[Workspace],
    ) -> Option<Action> {
        let mut action = None;
        self.selected
            .retain(|id| windows.iter().any(|w| w.id == *id));
        self.destinations.retain(|id, target| {
            windows.iter().any(|window| window.id == *id)
                && workspaces.iter().any(|workspace| workspace.id == *target)
        });
        if self
            .workspace_filter
            .is_some_and(|id| !workspaces.iter().any(|ws| ws.id == id))
        {
            self.workspace_filter = None;
        }

        ui.heading("窗口与工作区");
        ui.label("搜索窗口并跳转，或将指定窗口移到其他工作区。移动后保留当前视图。");
        ui.add_space(12.0);
        ui.add(
            TextEdit::singleline(&mut self.query)
                .hint_text("搜索标题或应用名，支持多个关键词")
                .desired_width(f32::INFINITY),
        );
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt("workspace_filter")
                .selected_text(
                    self.workspace_filter
                        .and_then(|id| workspaces.iter().find(|ws| ws.id == id))
                        .map(workspace_label)
                        .unwrap_or_else(|| "全部工作区".into()),
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.workspace_filter, None, "全部工作区");
                    for workspace in workspaces {
                        ui.selectable_value(
                            &mut self.workspace_filter,
                            Some(workspace.id),
                            workspace_label(workspace),
                        );
                    }
                });
            if ui.button("清除筛选").clicked() {
                self.query.clear();
                self.workspace_filter = None;
            }
        });
        ui.add_space(10.0);
        ui.label(RichText::new("切换工作区").strong());
        ui.horizontal_wrapped(|ui| {
            for workspace in workspaces.iter().filter(|ws| ws.output.is_some()) {
                let count = windows
                    .iter()
                    .filter(|w| w.workspace_id == Some(workspace.id))
                    .count();
                let label = format!(
                    "{} · {count} 个窗口{}",
                    workspace_label(workspace),
                    if workspace.is_focused {
                        " · 当前"
                    } else {
                        ""
                    }
                );
                let response = ui.button(label);
                if let Some(name) = response.dnd_release_payload::<String>() {
                    action = Some(Action::MoveWindowGroup {
                        name: (*name).clone(),
                        workspace_id: workspace.id,
                    });
                }
                if response.clicked() {
                    action = Some(Action::FocusWorkspace {
                        reference: WorkspaceReferenceArg::Id(workspace.id),
                    });
                }
            }
        });
        ui.separator();
        let visible: Vec<_> = windows
            .iter()
            .filter(|window| {
                self.workspace_filter
                    .is_none_or(|id| window.workspace_id == Some(id))
                    && matches_search(window, &self.query)
            })
            .collect();
        ui.label(format!("显示 {} / {} 个窗口", visible.len(), windows.len()));
        if visible.is_empty() {
            ui.label(if windows.is_empty() {
                "当前没有窗口。"
            } else {
                "没有匹配的窗口，试试其他关键词或工作区。"
            });
        }
        ScrollArea::vertical().show(ui, |ui| {
            for window in visible {
                ui.push_id(window.id, |ui| {
                    ui.group(|ui| {
                        ui.set_width(ui.available_width());
                        let mut selected = self.selected.contains(&window.id);
                        if ui.checkbox(&mut selected, "选择加入分组").changed() {
                            if selected {
                                self.selected.insert(window.id);
                            } else {
                                self.selected.remove(&window.id);
                            }
                        }
                        ui.label(
                            RichText::new(
                                window
                                    .title
                                    .as_deref()
                                    .filter(|s| !s.is_empty())
                                    .unwrap_or("无标题窗口"),
                            )
                            .strong(),
                        );
                        let location = window
                            .workspace_id
                            .and_then(|id| workspaces.iter().find(|ws| ws.id == id))
                            .map(workspace_label)
                            .unwrap_or_else(|| "未分配工作区".into());
                        ui.label(format!(
                            "{} · {}",
                            window.app_id.as_deref().unwrap_or("未知应用"),
                            location
                        ));
                        ui.horizontal_wrapped(|ui| {
                            if window.is_focused {
                                ui.label("当前焦点");
                            }
                            if window.is_floating {
                                ui.label("浮动窗口");
                            }
                            if window.is_urgent {
                                ui.label("需要关注");
                            }
                            if ui.button("跳转窗口").clicked() {
                                action = Some(Action::FocusWindow { id: window.id });
                            }
                            let target = self.destinations.entry(window.id).or_insert_with(|| {
                                window
                                    .workspace_id
                                    .or_else(|| workspaces.first().map(|ws| ws.id))
                                    .unwrap_or(0)
                            });
                            egui::ComboBox::from_id_salt("destination")
                                .selected_text(
                                    workspaces
                                        .iter()
                                        .find(|ws| ws.id == *target)
                                        .map(workspace_label)
                                        .unwrap_or_else(|| "选择目标工作区".into()),
                                )
                                .show_ui(ui, |ui| {
                                    for ws in workspaces.iter().filter(|ws| ws.output.is_some()) {
                                        ui.selectable_value(target, ws.id, workspace_label(ws));
                                    }
                                });
                            let can_move = window.workspace_id != Some(*target)
                                && workspaces
                                    .iter()
                                    .any(|ws| ws.id == *target && ws.output.is_some());
                            if ui
                                .add_enabled(can_move, Button::new("移动到此工作区"))
                                .clicked()
                            {
                                action = Some(move_action(window.id, *target));
                            }
                        });
                    });
                    ui.add_space(8.0);
                });
            }
        });
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window() -> Window {
        serde_json::from_value(serde_json::json!({
            "id": 42, "title": "项目笔记 — README", "app_id": "Org.Editor",
            "pid": null, "workspace_id": 7, "is_focused": false,
            "is_floating": false, "is_urgent": false, "focus_timestamp": null,
            "layout": {"pos_in_scrolling_layout": null, "tile_size": [800, 600],
                "window_size": [800, 600], "tile_pos_in_workspace_view": null,
                "window_offset_in_tile": [0, 0]}
        }))
        .unwrap()
    }

    #[test]
    fn search_combines_title_and_application_keywords() {
        let window = window();
        assert!(matches_search(&window, "  EDITOR 项目 readme "));
        assert!(!matches_search(&window, "editor missing"));
        assert!(matches_search(&window, ""));
    }

    #[test]
    fn moves_explicit_window_to_stable_workspace_without_following() {
        let Action::MoveWindowToWorkspace {
            window_id,
            reference,
            focus,
        } = move_action(42, 900)
        else {
            unreachable!()
        };
        assert_eq!(window_id, Some(42));
        assert_eq!(reference, WorkspaceReferenceArg::Id(900));
        assert!(!focus);
    }

    #[test]
    fn closed_windows_and_removed_workspaces_clear_selection() {
        let mut manager = WindowManager::default();
        manager.destinations.insert(42, 7);
        manager.workspace_filter = Some(7);
        let context = egui::Context::default();
        let _ = context.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                assert!(manager.show(ui, &[], &[]).is_none());
            });
        });
        assert!(manager.destinations.is_empty());
        assert_eq!(manager.workspace_filter, None);
    }
    #[test]
    fn populated_page_renders_without_triggering_actions() {
        let mut manager = WindowManager::default();
        let workspace = Workspace {
            id: 7,
            idx: 1,
            name: Some("开发".into()),
            output: Some("DP-1".into()),
            is_urgent: false,
            is_active: true,
            is_focused: true,
            active_window_id: Some(42),
        };
        let context = egui::Context::default();
        let _ = context.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                assert!(manager
                    .show(ui, &[window()], std::slice::from_ref(&workspace))
                    .is_none());
            });
        });
        assert_eq!(manager.destinations.get(&42), Some(&7));
    }
}
