//! Searchable, keyboard-driven entry points to existing settings and desktop actions.
use clarion_ipc::{management::ManagementState, Window, Workspace};
use egui::{self, Align2, Button, Key, Modifiers, ScrollArea, TextEdit, Vec2};

use crate::app::ActiveTab;
use crate::theme::colors as palette;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Navigate(ActiveTab),
    Refresh,
    Overview,
    Expo,
    WindowSpread,
    SwitchTiling,
    ToggleCanvas,
    ResetCanvas,
    UndoLayout,
    RedoLayout,
    FocusWindow(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Category {
    Page,
    Window,
    Desktop,
    Connection,
}

impl Command {
    fn category(self) -> Category {
        match self {
            Self::Navigate(_) => Category::Page,
            Self::FocusWindow(_) => Category::Window,
            Self::Refresh => Category::Connection,
            _ => Category::Desktop,
        }
    }
}

impl Category {
    fn label(self) -> &'static str {
        match self {
            Self::Page => "设置页",
            Self::Window => "窗口",
            Self::Desktop => "桌面操作",
            Self::Connection => "连接",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub command: Command,
    pub title: String,
    pub description: String,
    pub disabled: Option<&'static str>,
    keywords: String,
}

pub struct Sources<'a> {
    pub connected: bool,
    pub windows: &'a [Window],
    pub workspaces: &'a [Workspace],
    pub management: Option<&'a ManagementState>,
    pub pending_window_focus: Option<u64>,
}

fn entry(command: Command, title: &str, description: &str, keywords: &str) -> Entry {
    Entry {
        command,
        title: title.into(),
        description: description.into(),
        disabled: None,
        keywords: keywords.into(),
    }
}

fn entries(sources: &Sources<'_>) -> Vec<Entry> {
    use ActiveTab::*;
    let mut entries = vec![
        entry(
            Command::Navigate(Dashboard),
            "系统偏好设置",
            "设置页 · 全部显示与分类总览",
            "dashboard status overview preferences 概览 开始 首页 状态 全部显示",
        ),
        entry(
            Command::Navigate(Windows),
            "调度中心",
            "设置页 · 搜索、移动、分组与触发角",
            "mission control windows workspace hot corners 分组 窗口 工作区 管理 热区 触发角",
        ),
        entry(
            Command::Navigate(Layout),
            "窗口布局",
            "设置页 · 平铺、间隙、无限画布",
            "layout tiling gaps scrollable dwindle 布局",
        ),
        entry(
            Command::Navigate(Appearance),
            "通用",
            "设置页 · 边框、动画、背景模糊",
            "appearance animation blur border 外观",
        ),
        entry(
            Command::Navigate(InputHotCorners),
            "键盘",
            "设置页 · 数字键盘与更多键盘设置",
            "keyboard input numlock 输入 键盘 数字键盘",
        ),
        entry(
            Command::Navigate(Trackpad),
            "触控板",
            "设置页 · 轻触点击、自然滚动与手势",
            "trackpad touchpad tap natural scroll gestures 触控板 轻触 自然滚动 手势",
        ),
        entry(
            Command::Navigate(FullConfig),
            "全部设置",
            "设置页 · 参数、规则、快捷键",
            "config binds rules 设置 配置 快捷键",
        ),
        entry(
            Command::Navigate(Outputs),
            "显示器",
            "设置页 · 分辨率、缩放、位置",
            "outputs monitor display scale 显示器",
        ),
        entry(
            Command::Navigate(ConfigEditor),
            "配置文件",
            "设置页 · 直接编辑 KDL 草稿",
            "editor kdl file 配置 编辑 直写",
        ),
        entry(
            Command::Refresh,
            "刷新 / 重新连接",
            "操作 · 重新获取桌面状态",
            "refresh reconnect 连接 刷新",
        ),
    ];
    for (command, title, description, keywords) in [
        (
            Command::Overview,
            "切换工作区概览",
            "桌面 · 展示或收起 Overview",
            "overview 工作区 概览",
        ),
        (
            Command::Expo,
            "切换工作区网格",
            "桌面 · 展示或收起 Expo",
            "expo 工作区 网格",
        ),
        (
            Command::WindowSpread,
            "全窗口展开 / 搜索",
            "桌面 · 展示或收起所有窗口",
            "spread windows 窗口 搜索",
        ),
        (
            Command::SwitchTiling,
            "切换当前工作区平铺模式",
            "桌面 · 滚动列平铺与 Dwindle 互换",
            "tiling layout scrollable dwindle 平铺",
        ),
        (
            Command::ToggleCanvas,
            "切换无限画布",
            "桌面 · 开启或退出画布",
            "canvas 无限 画布",
        ),
        (
            Command::ResetCanvas,
            "画布回到原点",
            "桌面 · 复位当前画布位置",
            "canvas reset 画布 原点",
        ),
        (
            Command::UndoLayout,
            "撤销布局",
            "桌面 · 撤销最近的布局操作",
            "undo layout 撤销 布局",
        ),
        (
            Command::RedoLayout,
            "重做布局",
            "桌面 · 重做已撤销的布局操作",
            "redo layout 重做 布局",
        ),
    ] {
        let mut item = entry(command, title, description, keywords);
        item.disabled = if !sources.connected {
            Some("需要连接 Clarion")
        } else {
            match command {
                Command::UndoLayout | Command::RedoLayout if sources.management.is_none() => {
                    Some("当前合成器不支持布局历史")
                }
                Command::UndoLayout
                    if sources
                        .management
                        .is_some_and(|state| state.undo_steps == 0) =>
                {
                    Some("没有可撤销的布局操作")
                }
                Command::RedoLayout
                    if sources
                        .management
                        .is_some_and(|state| state.redo_steps == 0) =>
                {
                    Some("没有可重做的布局操作")
                }
                _ => None,
            }
        };
        entries.push(item);
    }
    if sources.connected {
        let mut windows: Vec<_> = sources.windows.iter().collect();
        windows.sort_by_key(|window| {
            (
                std::cmp::Reverse(window.focus_timestamp.map(|time| (time.secs, time.nanos))),
                window.id,
            )
        });
        for window in windows {
            let app = window
                .app_id
                .as_deref()
                .map(str::trim)
                .filter(|app| !app.is_empty())
                .unwrap_or("未知应用");
            let title = window
                .title
                .as_deref()
                .map(str::trim)
                .filter(|title| !title.is_empty())
                .unwrap_or(app);
            let workspace = sources
                .workspaces
                .iter()
                .find(|ws| Some(ws.id) == window.workspace_id);
            let location = workspace
                .map(|ws| {
                    format!(
                        "{} · 工作区 {}{}",
                        ws.output.as_deref().unwrap_or("未连接显示器"),
                        ws.idx,
                        ws.name
                            .as_ref()
                            .map(|name| format!(" · {name}"))
                            .unwrap_or_default()
                    )
                })
                .unwrap_or_else(|| "未分配工作区".into());
            let workspace_alias = workspace
                .map(|ws| format!("工作区{} workspace{}", ws.idx, ws.idx))
                .unwrap_or_default();
            let mut item = entry(
                Command::FocusWindow(window.id),
                title,
                &format!(
                    "窗口 · {app} · {location}{}",
                    if window.is_focused { " · 当前" } else { "" }
                ),
                &format!("focus window 窗口 聚焦 {app} {location} {workspace_alias}"),
            );
            if sources.pending_window_focus.is_some() {
                item.disabled = Some("正在确认目标窗口，Esc 可取消");
            }
            entries.push(item);
        }
    }
    entries
}

/// All query words must match; title matches rank before metadata matches.
fn match_score(item: &Entry, query: &str) -> Option<usize> {
    let title = item.title.to_lowercase();
    let metadata = format!("{} {}", item.description, item.keywords).to_lowercase();
    query.split_whitespace().try_fold(0, |score, word| {
        let word = word.to_lowercase();
        let weight = if title == word {
            0
        } else if title.starts_with(&word) {
            1
        } else if title.contains(&word) {
            2
        } else if metadata.contains(&word) {
            3
        } else {
            return None;
        };
        Some(score + weight)
    })
}

fn results(sources: &Sources<'_>, query: &str) -> Vec<Entry> {
    let mut matches: Vec<_> = entries(sources)
        .into_iter()
        .enumerate()
        .filter_map(|(index, item)| match_score(&item, query).map(|score| (score, index, item)))
        .collect();
    matches.sort_by_key(|(score, index, _)| (*score, *index));
    matches.into_iter().map(|(_, _, item)| item).collect()
}

#[derive(Default)]
pub struct CommandPalette {
    open: bool,
    query: String,
    selected: Option<Command>,
    selected_position: Option<usize>,
    requires_selection: bool,
    ime_composing: bool,
    focus_search: bool,
    scroll_selected: bool,
    error: Option<String>,
}

fn search_id() -> egui::Id {
    egui::Id::new("command_palette_search")
}

impl CommandPalette {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self) {
        self.open = true;
        self.query.clear();
        self.selected = None;
        self.selected_position = None;
        self.requires_selection = false;
        self.ime_composing = false;
        self.focus_search = true;
        self.scroll_selected = true;
        self.error = None;
    }

    pub fn close(&mut self, ctx: &egui::Context) {
        self.open = false;
        self.ime_composing = false;
        ctx.memory_mut(|memory| memory.surrender_focus(search_id()));
    }

    pub fn show_error(&mut self, error: impl Into<String>) {
        self.error = Some(error.into());
        self.focus_search = true;
    }

    pub fn focus_search(&mut self, ctx: &egui::Context) {
        ctx.memory_mut(|memory| memory.request_focus(search_id()));
        self.focus_search = false;
    }

    /// Run before settings widgets so a focused editor cannot consume the shortcut.
    pub fn handle_shortcut(&mut self, ctx: &egui::Context, available: bool) {
        if !available {
            if self.open {
                self.close(ctx);
            }
            return;
        }
        let pressed = ctx.input_mut(|input| {
            let mut pressed = false;
            input.events.retain(|event| {
                if let egui::Event::Key {
                    key: Key::K,
                    pressed: true,
                    repeat,
                    modifiers,
                    ..
                } = event
                {
                    if modifiers.ctrl && !modifiers.shift && !modifiers.alt && !modifiers.mac_cmd {
                        pressed |= !repeat;
                        return false;
                    }
                }
                true
            });
            pressed
        });
        if pressed {
            if self.open {
                self.close(ctx);
            } else {
                self.open();
            }
        }
    }

    fn synchronize_selection(&mut self, entries: &[Entry]) {
        let mut position = entries
            .iter()
            .position(|item| Some(item.command) == self.selected);
        if position.is_none() {
            if matches!(self.selected, Some(Command::FocusWindow(_))) {
                // A live snapshot must not silently turn a window jump into a
                // different command. A new query or arrow press explicitly chooses.
                self.selected = None;
                self.requires_selection = true;
                if self.error.is_none() {
                    self.show_error("所选窗口已不在结果中，请用 ↑↓ 重新选择");
                }
            } else if !self.requires_selection {
                self.selected = entries.first().map(|item| item.command);
                position = (!entries.is_empty()).then_some(0);
            }
        }
        if position != self.selected_position {
            self.scroll_selected = true;
        }
        self.selected_position = position;
    }

    fn move_selection(&mut self, entries: &[Entry], down: bool) {
        if entries.is_empty() {
            self.selected = None;
            return;
        }
        let current = entries
            .iter()
            .position(|item| Some(item.command) == self.selected);
        let next = match (current, down) {
            (Some(current), true) => (current + 1) % entries.len(),
            (Some(current), false) => (current + entries.len() - 1) % entries.len(),
            (None, true) => 0,
            (None, false) => entries.len() - 1,
        };
        self.selected = Some(entries[next].command);
        self.selected_position = Some(next);
        self.requires_selection = false;
        self.scroll_selected = true;
        self.error = None;
    }

    /// Call before rendering the disabled background, including keyboard consumption.
    pub fn show(&mut self, ctx: &egui::Context, sources: &Sources<'_>) -> Option<Command> {
        if !self.open {
            return None;
        }
        let ime_keys = ctx.input(|input| {
            let mut block_keys = self.ime_composing;
            for event in &input.events {
                if let egui::Event::Ime(event) = event {
                    match event {
                        // Enabling IME does not mean a candidate is being composed.
                        egui::ImeEvent::Enabled => {}
                        egui::ImeEvent::Preedit(text) => {
                            block_keys |= self.ime_composing || !text.is_empty();
                            self.ime_composing = !text.is_empty();
                        }
                        egui::ImeEvent::Commit(_) => {
                            block_keys = true;
                            self.ime_composing = false;
                        }
                        egui::ImeEvent::Disabled => {
                            block_keys |= self.ime_composing;
                            self.ime_composing = false;
                        }
                    }
                }
            }
            block_keys
        });
        let (escape, up, down, enter) = ctx.input_mut(|input| {
            let keys = (
                input.consume_key(Modifiers::NONE, Key::Escape),
                input.consume_key(Modifiers::NONE, Key::ArrowUp),
                input.consume_key(Modifiers::NONE, Key::ArrowDown),
                input.consume_key(Modifiers::NONE, Key::Enter),
            );
            // Keep candidate confirmation/navigation out of the command path.
            // Consuming Enter also keeps egui's single-line editor focused.
            if ime_keys {
                (false, false, false, false)
            } else {
                keys
            }
        });
        if ime_keys {
            // egui clears keyboard focus on Escape before widgets consume it.
            // Candidate dismissal must leave the search editor ready for input.
            ctx.memory_mut(|memory| memory.request_focus(search_id()));
        }
        if escape {
            self.close(ctx);
            return None;
        }
        let screen = ctx.screen_rect();
        let backdrop = egui::Area::new(egui::Id::new("command_palette_backdrop"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let (rect, response) = ui.allocate_exact_size(screen.size(), egui::Sense::click());
                ui.painter().rect_filled(rect, 0.0, palette::SCRIM);
                if response.clicked() {
                    self.close(ctx);
                }
            });
        if !self.open {
            return None;
        }
        let mut open = true;
        let mut activated = None;
        let window = egui::Window::new("命令中心")
            .id(egui::Id::new("command_palette"))
            .order(egui::Order::Foreground)
            .open(&mut open)
            .anchor(Align2::CENTER_TOP, Vec2::new(0.0, 70.0))
            .default_width((screen.width() - 80.0).min(620.0))
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(if sources.connected {
                        "● 已连接 · 配置页、桌面操作与窗口"
                    } else {
                        "○ 离线 · 配置页与重新连接仍可使用"
                    });
                    if sources.pending_window_focus.is_some() {
                        ui.spinner();
                        ui.small("正在确认窗口…");
                    }
                });
                if self.focus_search {
                    ctx.memory_mut(|memory| memory.request_focus(search_id()));
                }
                let response = ui.add(
                    TextEdit::singleline(&mut self.query)
                        .id(search_id())
                        .hint_text("搜索设置、操作、窗口标题、应用或工作区…")
                        .desired_width(f32::INFINITY),
                );
                if self.focus_search {
                    // The first sizing pass may render disabled widgets and
                    // surrender the early request; restore focus after allocation.
                    response.request_focus();
                    self.focus_search = false;
                }
                let entries = results(sources, &self.query);
                if response.changed() {
                    // Refining a query keeps the user's selection when it matches.
                    if !entries
                        .iter()
                        .any(|item| Some(item.command) == self.selected)
                    {
                        self.selected = None;
                    }
                    self.requires_selection = false;
                    self.error = None;
                    self.scroll_selected = true;
                }
                self.synchronize_selection(&entries);
                if down {
                    self.move_selection(&entries, true);
                }
                if up {
                    self.move_selection(&entries, false);
                }
                ui.add_space(6.0);
                ScrollArea::vertical()
                    .id_salt("command_palette_results")
                    .max_height((screen.height() - 270.0).clamp(100.0, 360.0))
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        if entries.is_empty() {
                            ui.label("没有匹配结果，试试配置页名称或应用名。");
                        }
                        for item in &entries {
                            let selected = self.selected == Some(item.command);
                            let mut text = egui::text::LayoutJob::default();
                            let secondary = ui.visuals().weak_text_color();
                            text.append(
                                &format!("{}  ", item.command.category().label()),
                                0.0,
                                egui::TextFormat {
                                    font_id: egui::FontId::proportional(11.0),
                                    color: secondary,
                                    ..Default::default()
                                },
                            );
                            text.append(
                                &format!("{}\n", item.title),
                                0.0,
                                egui::TextFormat {
                                    font_id: egui::FontId::proportional(15.0),
                                    color: if item.disabled.is_some() {
                                        secondary
                                    } else {
                                        ui.visuals().text_color()
                                    },
                                    ..Default::default()
                                },
                            );
                            let description = item
                                .description
                                .split_once(" · ")
                                .map_or(item.description.as_str(), |(_, description)| description);
                            text.append(
                                description,
                                0.0,
                                egui::TextFormat {
                                    font_id: egui::FontId::proportional(12.0),
                                    color: secondary,
                                    ..Default::default()
                                },
                            );
                            if let Some(reason) = item.disabled {
                                text.append(
                                    &format!("\n{reason}"),
                                    0.0,
                                    egui::TextFormat {
                                        font_id: egui::FontId::proportional(12.0),
                                        color: secondary,
                                        ..Default::default()
                                    },
                                );
                            }
                            let response = ui
                                .add_sized(
                                    [ui.available_width(), 49.0],
                                    Button::new(text).selected(selected).wrap(),
                                )
                                .on_hover_text(format!("{}\n{}", item.title, item.description));
                            if selected && self.scroll_selected {
                                response.scroll_to_me(Some(egui::Align::Center));
                            }
                            if response.clicked() {
                                self.selected = Some(item.command);
                                if let Some(reason) = item.disabled {
                                    self.show_error(reason);
                                } else {
                                    activated = Some(item.command);
                                }
                            }
                        }
                    });
                self.scroll_selected = false;
                if enter {
                    if let Some(item) = entries
                        .iter()
                        .find(|item| Some(item.command) == self.selected)
                    {
                        if let Some(reason) = item.disabled {
                            self.show_error(reason);
                        } else {
                            activated = Some(item.command);
                        }
                    }
                }
                if let Some(error) = &self.error {
                    ui.colored_label(palette::ERROR, error);
                }
                ui.separator();
                ui.small(format!(
                    "{} 个结果 · ↑↓ 选择 · Enter 执行 · Esc / Ctrl+K 关闭",
                    entries.len()
                ));
                if sources.pending_window_focus.is_some() {
                    ui.small("Esc 取消窗口跳转；也可以搜索并选择其他命令。");
                }
            });
        if let Some(window) = window {
            // Clicking or dragging an Area promotes it. Keep the panel above
            // its fullscreen backdrop even when an outside press is not a click.
            ctx.set_sublayer(backdrop.response.layer_id, window.response.layer_id);
        }
        if !open {
            self.close(ctx);
        }
        activated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sources<'a>(windows: &'a [Window]) -> Sources<'a> {
        Sources {
            connected: true,
            windows,
            workspaces: &[],
            management: None,
            pending_window_focus: None,
        }
    }

    fn window(id: u64, title: &str, app: &str, secs: u64) -> Window {
        serde_json::from_value(serde_json::json!({
            "id": id, "title": title, "app_id": app, "pid": null,
            "workspace_id": null, "is_focused": false, "is_floating": false,
            "is_urgent": false, "focus_timestamp": {"secs": secs, "nanos": 0},
            "layout": {"pos_in_scrolling_layout": null, "tile_size": [800, 600],
                "window_size": [800, 600], "tile_pos_in_workspace_view": null,
                "window_offset_in_tile": [0, 0]}
        }))
        .unwrap()
    }

    fn key(key: Key, modifiers: Modifiers, repeat: bool) -> egui::RawInput {
        egui::RawInput {
            events: vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat,
                modifiers,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn search_combines_words_and_ranks_titles_before_keywords() {
        let windows = [window(42, "Editor", "org.Note", 1)];
        let matches = results(&sources(&windows), "eDiToR");
        assert_eq!(matches[0].command, Command::FocusWindow(42));
        assert!(matches
            .iter()
            .any(|item| item.command == Command::Navigate(ActiveTab::ConfigEditor)));
        assert_eq!(results(&sources(&windows), "EDITOR org.note").len(), 1);
        assert!(results(&sources(&windows), "editor missing").is_empty());
        assert!(results(&sources(&windows), "  ").len() > 8);
    }

    #[test]
    fn preference_names_and_trackpad_search_match_the_actual_panes() {
        let sources = sources(&[]);
        let catalogue = entries(&sources);
        for (tab, title) in [
            (ActiveTab::Dashboard, "系统偏好设置"),
            (ActiveTab::Appearance, "通用"),
            (ActiveTab::Windows, "调度中心"),
            (ActiveTab::InputHotCorners, "键盘"),
            (ActiveTab::Trackpad, "触控板"),
        ] {
            let pane = catalogue
                .iter()
                .find(|entry| entry.command == Command::Navigate(tab))
                .unwrap();
            assert_eq!(pane.title, title);
            assert!(pane.disabled.is_none());
        }
        for query in ["触控板", "TRACKPAD", "touchpad", "自然滚动"] {
            let result = results(&sources, query);
            assert_eq!(result.len(), 1, "{query}");
            assert_eq!(result[0].command, Command::Navigate(ActiveTab::Trackpad));
        }
        assert_eq!(
            results(&sources, "触发角")[0].command,
            Command::Navigate(ActiveTab::Windows)
        );
    }

    #[test]
    fn windows_use_recency_and_stable_ids_for_ties() {
        let windows = [
            window(3, "Three", "app", 5),
            window(2, "Two", "app", 10),
            window(1, "One", "app", 10),
        ];
        let ids: Vec<_> = results(&sources(&windows), "app")
            .into_iter()
            .filter_map(|item| {
                if let Command::FocusWindow(id) = item.command {
                    Some(id)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(ids, [1, 2, 3]);
    }

    #[test]
    fn offline_actions_are_disabled_and_cached_windows_are_not_offered() {
        let windows = [window(42, "Editor", "app", 1)];
        let mut sources = sources(&windows);
        sources.connected = false;
        let items = results(&sources, "");
        assert_eq!(
            items.iter().filter(|item| item.disabled.is_none()).count(),
            10
        );
        assert!(!items
            .iter()
            .any(|item| matches!(item.command, Command::FocusWindow(_))));
        assert!(items
            .iter()
            .filter(|item| matches!(item.command, Command::UndoLayout | Command::RedoLayout))
            .all(|item| item.disabled.is_some()));
    }

    #[test]
    fn selection_follows_identity_when_results_change_and_clears_when_empty() {
        let mut palette = CommandPalette::default();
        palette.open();
        let windows = [window(42, "Editor", "app", 1)];
        let items = results(&sources(&windows), "");
        palette.selected = Some(Command::FocusWindow(42));
        palette.synchronize_selection(&items);
        assert_eq!(palette.selected, Some(Command::FocusWindow(42)));
        palette.synchronize_selection(&results(&sources(&[]), ""));
        assert_eq!(palette.selected, None);
        assert!(palette.requires_selection);
        palette.synchronize_selection(&[]);
        assert_eq!(palette.selected, None);
    }

    #[test]
    fn shortcut_is_consumed_once_ignores_repeat_and_respects_confirmation() {
        let ctx = egui::Context::default();
        let mut palette = CommandPalette::default();
        let _ = ctx.run(key(Key::K, Modifiers::CTRL, false), |ctx| {
            palette.handle_shortcut(ctx, true);
            assert!(!ctx.input(|input| input.key_pressed(Key::K)));
        });
        assert!(palette.is_open());
        let _ = ctx.run(key(Key::K, Modifiers::CTRL, true), |ctx| {
            palette.handle_shortcut(ctx, true)
        });
        assert!(palette.is_open());
        let _ = ctx.run(key(Key::K, Modifiers::CTRL, false), |ctx| {
            palette.handle_shortcut(ctx, false)
        });
        assert!(!palette.is_open());
        let _ = ctx.run(
            key(Key::K, Modifiers::CTRL | Modifiers::SHIFT, false),
            |ctx| palette.handle_shortcut(ctx, true),
        );
        assert!(!palette.is_open());
    }

    #[test]
    fn headless_keyboard_search_navigation_enter_and_escape_work() {
        let ctx = egui::Context::default();
        let mut palette = CommandPalette::default();
        palette.open();
        let mut command = None;
        let _ = ctx.run(Default::default(), |ctx| {
            command = palette.show(ctx, &sources(&[]));
        });
        assert_eq!(command, None);
        let _ = ctx.run(key(Key::ArrowDown, Modifiers::NONE, false), |ctx| {
            command = palette.show(ctx, &sources(&[]));
        });
        assert_eq!(
            palette.selected,
            Some(Command::Navigate(ActiveTab::Windows))
        );
        let _ = ctx.run(key(Key::ArrowUp, Modifiers::NONE, false), |ctx| {
            command = palette.show(ctx, &sources(&[]));
        });
        assert_eq!(
            palette.selected,
            Some(Command::Navigate(ActiveTab::Dashboard))
        );
        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Text("blur".into())],
                ..Default::default()
            },
            |ctx| {
                command = palette.show(ctx, &sources(&[]));
            },
        );
        assert_eq!(
            palette.selected,
            Some(Command::Navigate(ActiveTab::Appearance))
        );
        let _ = ctx.run(key(Key::Enter, Modifiers::NONE, false), |ctx| {
            command = palette.show(ctx, &sources(&[]));
            assert!(!ctx.input(|input| input.key_pressed(Key::Enter)));
        });
        assert_eq!(command, Some(Command::Navigate(ActiveTab::Appearance)));
        let _ = ctx.run(key(Key::Escape, Modifiers::NONE, false), |ctx| {
            command = palette.show(ctx, &sources(&[]));
        });
        assert_eq!(command, None);
        assert!(!palette.is_open());
    }

    #[test]
    fn headless_empty_or_disabled_result_never_activates() {
        let ctx = egui::Context::default();
        let mut palette = CommandPalette::default();
        palette.open();
        for query in ["missing-command", "撤销布局"] {
            palette.query = query.into();
            let _ = ctx.run(key(Key::Enter, Modifiers::NONE, false), |ctx| {
                assert!(palette.show(ctx, &sources(&[])).is_none());
            });
        }
        assert!(palette.error.is_some());
    }

    #[test]
    fn headless_query_edit_keeps_the_matching_keyboard_selection() {
        let ctx = egui::Context::default();
        let windows = [
            window(42, "Editor Alpha", "app", 10),
            window(43, "Editor Beta", "app", 1),
        ];
        let mut palette = CommandPalette::default();
        palette.open();
        palette.query = "edit".into();
        for _ in 0..2 {
            let _ = ctx.run(Default::default(), |ctx| {
                palette.show(ctx, &sources(&windows));
            });
        }
        palette.selected = Some(Command::FocusWindow(43));
        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Text("or".into())],
                ..Default::default()
            },
            |ctx| {
                assert!(palette.show(ctx, &sources(&windows)).is_none());
            },
        );
        assert_eq!(palette.query, "editor");
        assert_eq!(palette.selected, Some(Command::FocusWindow(43)));
    }

    #[test]
    fn headless_closed_selection_requires_explicit_reselection_before_enter() {
        let ctx = egui::Context::default();
        let mut palette = CommandPalette::default();
        palette.open();
        let windows = [window(42, "Editor", "app", 1)];
        let _ = ctx.run(Default::default(), |ctx| {
            palette.show(ctx, &sources(&windows));
        });
        palette.selected = Some(Command::FocusWindow(42));
        let _ = ctx.run(key(Key::Enter, Modifiers::NONE, false), |ctx| {
            assert!(
                palette.show(ctx, &sources(&[])).is_none(),
                "a removed window must not activate a settings page"
            );
        });
        assert_eq!(palette.selected, None);
        assert!(palette.error.is_some());
        let _ = ctx.run(key(Key::Enter, Modifiers::NONE, false), |ctx| {
            assert!(palette.show(ctx, &sources(&[])).is_none());
        });
        let _ = ctx.run(key(Key::ArrowDown, Modifiers::NONE, false), |ctx| {
            palette.show(ctx, &sources(&[]));
        });
        assert_eq!(
            palette.selected,
            Some(Command::Navigate(ActiveTab::Dashboard))
        );
    }

    #[test]
    fn headless_ime_candidates_do_not_execute_or_dismiss_the_palette() {
        let ctx = egui::Context::default();
        let mut palette = CommandPalette::default();
        palette.open();
        for _ in 0..2 {
            let _ = ctx.run(Default::default(), |ctx| {
                palette.show(ctx, &sources(&[]));
            });
        }
        let mut input = key(Key::Enter, Modifiers::NONE, false);
        input.events.splice(
            0..0,
            [
                egui::Event::Ime(egui::ImeEvent::Enabled),
                egui::Event::Ime(egui::ImeEvent::Preedit("外观".into())),
            ],
        );
        let _ = ctx.run(input, |ctx| {
            assert!(palette.show(ctx, &sources(&[])).is_none());
        });
        let _ = ctx.run(key(Key::Escape, Modifiers::NONE, false), |ctx| {
            assert!(palette.show(ctx, &sources(&[])).is_none());
        });
        assert!(palette.is_open());
        let mut input = key(Key::Enter, Modifiers::NONE, false);
        input
            .events
            .insert(0, egui::Event::Ime(egui::ImeEvent::Commit("外观".into())));
        let _ = ctx.run(input, |ctx| {
            assert!(palette.show(ctx, &sources(&[])).is_none());
        });
        assert_eq!(palette.query, "外观");
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(search_id()));
        let _ = ctx.run(key(Key::Enter, Modifiers::NONE, false), |ctx| {
            assert_eq!(
                palette.show(ctx, &sources(&[])),
                Some(Command::Navigate(ActiveTab::Appearance))
            );
        });
    }

    #[test]
    fn enabling_ime_without_a_candidate_keeps_normal_palette_keys_available() {
        let ctx = egui::Context::default();
        let mut palette = CommandPalette::default();
        palette.open();
        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Ime(egui::ImeEvent::Enabled)],
                ..Default::default()
            },
            |ctx| {
                palette.show(ctx, &sources(&[]));
            },
        );
        let _ = ctx.run(key(Key::ArrowDown, Modifiers::NONE, false), |ctx| {
            palette.show(ctx, &sources(&[]));
        });
        assert_eq!(
            palette.selected,
            Some(Command::Navigate(ActiveTab::Windows))
        );
        let _ = ctx.run(key(Key::Enter, Modifiers::NONE, false), |ctx| {
            assert_eq!(
                palette.show(ctx, &sources(&[])),
                Some(Command::Navigate(ActiveTab::Windows))
            );
        });
    }

    #[test]
    fn chinese_app_workspace_queries_and_result_categories_remain_visible() {
        let ctx = egui::Context::default();
        let mut win = window(42, "研究笔记", "org.Example.Editor", 1);
        win.workspace_id = Some(900);
        let windows = [win];
        let workspaces = [Workspace {
            id: 900,
            idx: 2,
            name: Some("写作".into()),
            output: Some("HDMI-A-1".into()),
            is_urgent: false,
            is_active: true,
            is_focused: true,
            active_window_id: Some(42),
        }];
        let mut source = sources(&windows);
        source.workspaces = &workspaces;
        for query in [
            "研究",
            "org.example.editor 写作",
            "工作区2",
            "workspace2 HDMI-A-1",
        ] {
            assert_eq!(
                results(&source, query).first().unwrap().command,
                Command::FocusWindow(42)
            );
        }
        let mut palette = CommandPalette::default();
        palette.open();
        palette.query = "研究".into();
        let _ = ctx.run(Default::default(), |ctx| {
            palette.show(ctx, &source);
        });
        let output = ctx.run(Default::default(), |ctx| {
            palette.show(ctx, &source);
        });
        let visible: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| {
                if let egui::Shape::Text(text) = &shape.shape {
                    Some(text.galley.text())
                } else {
                    None
                }
            })
            .collect();
        assert!(visible
            .iter()
            .any(|text| text
                .contains("窗口  研究笔记\norg.Example.Editor · HDMI-A-1 · 工作区 2 · 写作")));
        source.pending_window_focus = Some(42);
        let item = results(&source, "研究").remove(0);
        assert!(item.disabled.is_some());
        assert!(item.description.contains("org.Example.Editor"));
        source.connected = false;
        assert!(results(&source, "研究").is_empty());
        assert_eq!(results(&source, "重新连接")[0].command, Command::Refresh);
        palette.query.clear();
        palette.show_error("Clarion 已断开连接，请重新连接。未保存的配置仍保留。");
        for _ in 0..2 {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(760.0, 520.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    palette.show(ctx, &source);
                },
            );
        }
        let panel = ctx
            .memory(|memory| memory.area_rect(egui::Id::new("command_palette")))
            .unwrap();
        assert!(
            panel.min.x >= 0.0 && panel.max.x <= 760.0,
            "panel width must fit the minimum settings window: {panel:?}"
        );
        assert!(
            panel.min.y >= 0.0 && panel.max.y <= 520.0,
            "errors and offline rows must fit the minimum settings window: {panel:?}"
        );
    }

    #[test]
    fn selection_follows_async_recency_reordering_and_scrolls_to_its_new_position() {
        let mut palette = CommandPalette::default();
        palette.open();
        palette.query = "Editor".into();
        let windows = [
            window(42, "Editor Alpha", "app", 10),
            window(43, "Editor Beta", "app", 1),
        ];
        palette.selected = Some(Command::FocusWindow(43));
        palette.synchronize_selection(&results(&sources(&windows), &palette.query));
        palette.scroll_selected = false;
        let windows = [
            window(42, "Editor Alpha", "app", 10),
            window(43, "Editor Beta", "app", 20),
        ];
        palette.synchronize_selection(&results(&sources(&windows), &palette.query));
        assert_eq!(palette.selected, Some(Command::FocusWindow(43)));
        assert_eq!(palette.selected_position, Some(0));
        assert!(palette.scroll_selected);
    }

    #[test]
    fn headless_mouse_can_activate_result_and_dismiss_backdrop() {
        let ctx = egui::Context::default();
        let mut palette = CommandPalette::default();
        palette.open();
        let input = |events| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(980.0, 680.0),
            )),
            events,
            ..Default::default()
        };
        let _ = ctx.run(input(vec![]), |ctx| {
            palette.show(ctx, &sources(&[]));
        });
        let output = ctx.run(input(vec![]), |ctx| {
            palette.show(ctx, &sources(&[]));
        });
        let target = output
            .shapes
            .iter()
            .find_map(|shape| {
                if let egui::Shape::Text(text) = &shape.shape {
                    text.galley
                        .text()
                        .contains("调度中心\n")
                        .then_some(text.pos + text.galley.size() * 0.5)
                } else {
                    None
                }
            })
            .expect("visible window settings result");
        let pointer = |pos, pressed| {
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                },
            ]
        };
        // Pressing a fullscreen Area normally moves it above its sibling Window.
        // A drag is not a click, so dismissal alone cannot protect their ordering.
        let outside = egui::pos2(10.0, 10.0);
        let dragged = egui::pos2(40.0, 35.0);
        let _ = ctx.run(input(pointer(outside, true)), |ctx| {
            palette.show(ctx, &sources(&[]));
        });
        let _ = ctx.run(input(vec![egui::Event::PointerMoved(dragged)]), |ctx| {
            palette.show(ctx, &sources(&[]));
        });
        let _ = ctx.run(input(pointer(dragged, false)), |ctx| {
            palette.show(ctx, &sources(&[]));
        });
        assert!(palette.is_open());
        assert_eq!(
            ctx.layer_id_at(target),
            Some(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("command_palette")
            ))
        );
        let _ = ctx.run(input(pointer(target, true)), |ctx| {
            assert!(palette.show(ctx, &sources(&[])).is_none());
        });
        let _ = ctx.run(input(pointer(target, false)), |ctx| {
            assert_eq!(
                palette.show(ctx, &sources(&[])),
                Some(Command::Navigate(ActiveTab::Windows))
            );
        });
        let _ = ctx.run(input(pointer(outside, true)), |ctx| {
            palette.show(ctx, &sources(&[]));
        });
        let _ = ctx.run(input(pointer(outside, false)), |ctx| {
            palette.show(ctx, &sources(&[]));
        });
        assert!(!palette.is_open());
    }
}
