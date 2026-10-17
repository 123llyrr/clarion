use std::time::Duration;

use clarion_ipc::{Output, Window, Workspace};
use eframe::egui::{
    self, Align, Align2, Button, Frame, Layout, Margin, RichText, Rounding, ScrollArea, Sense,
    Stroke, TextEdit, Ui, Vec2,
};

use crate::appearance_controls::{BlurOverrides, CloseEffect, CloseState};
use crate::command_palette::{Command, CommandPalette, Sources as CommandSources};
use crate::config_manager::{ConfigManager, MotionPreset, ValidationStatus};
use crate::editor::{
    highlight_kdl, SNIPPET_ANIMATED_BORDER, SNIPPET_HOT_CORNERS, SNIPPET_KEYBIND,
    SNIPPET_WINDOW_RULE,
};
use crate::icons::{self, Icon};
use crate::ipc::IpcClient;
use crate::preferences_home;
use crate::theme::colors as palette;
use crate::window_manager::WindowManager;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveTab {
    Dashboard,
    Windows,
    Layout,
    Appearance,
    InputHotCorners,
    Trackpad,
    Outputs,
    ConfigEditor,
    FullConfig,
}

pub struct ClarionSettingsApp {
    ipc: IpcClient,
    config_mgr: ConfigManager,
    active_tab: ActiveTab,
    back_stack: Vec<ActiveTab>,
    forward_stack: Vec<ActiveTab>,
    preferences_search: String,
    preferences_ime_composing: bool,
    preferences_search_selected: usize,
    show_hot_corners: bool,
    trackpad_section: usize,
    output_section: usize,
    selected_output: Option<String>,
    deferred_pointer_events: Vec<egui::Event>,
    status_message: Option<(String, bool, std::time::Instant)>, // (msg, is_error, time)
    // Cached IPC data
    cached_outputs: Vec<Output>,
    frame_timings: Option<Vec<clarion_ipc::diagnostics::OutputFrameTimings>>,
    cached_windows: Vec<Window>,
    window_manager: WindowManager,
    management: Option<clarion_ipc::management::ManagementState>,
    connection_error: Option<String>,
    cached_workspaces: Vec<Workspace>,
    clarion_version: String,
    last_ipc_poll: std::time::Instant,
    toggle_flag: Option<crate::instance::Signal>,
    focus_pending: bool,
    initial_window_geometry: bool,
    confirm_close: bool,
    confirm_reload: bool,
    allow_close: bool,
    /// The user chose “save and exit” while a display preview is still pending.
    /// The window must stay alive until the compositor acknowledges the confirmation.
    close_after_preview: bool,
    full_config: crate::full_config::FullConfig,
    command_palette: CommandPalette,
    pending_window_focus: Option<PendingWindowFocus>,
    desktop_theme: crate::theme::DesktopThemeWatcher,
}

#[derive(Clone, Copy)]
enum PreferenceNavigation {
    Open(ActiveTab),
    Back,
    Forward,
}

struct PendingWindowFocus {
    id: u64,
    reply: std::sync::mpsc::Receiver<Result<clarion_ipc::Response, String>>,
}

impl ClarionSettingsApp {
    pub fn new(
        _cc: &eframe::CreationContext<'_>,
        toggle_flag: Option<crate::instance::Signal>,
    ) -> Self {
        let ipc = IpcClient::new(_cc.egui_ctx.clone());
        let config_mgr = ConfigManager::new().unwrap_or_else(|e| {
            eprintln!("Error loading config: {e}");
            ConfigManager::empty()
        });

        let mut app = Self {
            ipc,
            config_mgr,
            active_tab: ActiveTab::Dashboard,
            back_stack: Vec::new(),
            forward_stack: Vec::new(),
            preferences_search: String::new(),
            preferences_ime_composing: false,
            preferences_search_selected: 0,
            show_hot_corners: false,
            trackpad_section: 0,
            output_section: 0,
            selected_output: None,
            deferred_pointer_events: Vec::new(),
            status_message: None,
            cached_outputs: Vec::new(),
            frame_timings: None,
            cached_windows: Vec::new(),
            window_manager: WindowManager::default(),
            management: None,
            connection_error: Some("正在连接 Clarion".into()),
            cached_workspaces: Vec::new(),
            clarion_version: "Unknown".into(),
            last_ipc_poll: std::time::Instant::now() - Duration::from_secs(10),
            toggle_flag,
            focus_pending: true,
            initial_window_geometry: true,
            confirm_close: false,
            confirm_reload: false,
            allow_close: false,
            close_after_preview: false,
            full_config: Default::default(),
            command_palette: Default::default(),
            pending_window_focus: None,
            desktop_theme: Default::default(),
        };

        app.refresh_ipc_data();
        app
    }

    fn flush_config_form(&mut self) -> anyhow::Result<()> {
        if self.config_mgr.visual_is_dirty() {
            self.config_mgr.try_apply_visual_settings_to_raw()?;
        }
        if let Some(next) = self
            .full_config
            .apply_pending(self.config_mgr.raw_content())?
        {
            self.config_mgr.set_raw_content(next);
        }
        Ok(())
    }
    fn choose_close_effect(&mut self, effect: CloseEffect) -> anyhow::Result<()> {
        self.flush_config_form()?;
        let original = self.config_mgr.raw_content().to_owned();
        let state = CloseState::read(&original)?;
        let before = self.config_mgr.effective_config()?.animations.window_close;
        let mut close_sources = Vec::new();
        for (path, content) in self.config_mgr.effective_source_contents()? {
            if let Some(content) = content {
                let source = std::str::from_utf8(&content)?;
                if crate::appearance_controls::contains_close_config(source)? {
                    close_sources.push(path);
                }
            }
        }
        let inherited = !state.local_config && !close_sources.is_empty();
        let next = crate::appearance_controls::set_close_effect_preserving_inheritance(
            &original,
            effect,
            inherited.then_some(&before),
        )?;
        // When editing an included file, a later file in the root configuration
        // can still replace this override. Verify the assembled candidate before
        // changing the editor or reporting success.
        self.config_mgr.merged_config(&next).and_then(|config| {
            let after = config.animations.window_close;
            let selected = match effect {
                CloseEffect::Disabled => after.anim.off,
                CloseEffect::Explosion => {
                    !after.anim.off
                        && after.effect == clarion_config::WindowCloseEffect::Explosion
                        && after.custom_shader.is_none()
                }
                CloseEffect::Implode => {
                    !after.anim.off
                        && after.effect == clarion_config::WindowCloseEffect::Implode
                        && after.custom_shader.is_none()
                }
                CloseEffect::Fade => {
                    !after.anim.off
                        && after.effect == clarion_config::WindowCloseEffect::Fade
                        && after.custom_shader.is_none()
                }
            };
            anyhow::ensure!(
                selected,
                "关闭设置被后续配置覆盖，原草稿已保留。请在全部设置检查这些来源的载入顺序：{}",
                close_sources
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join("、"),
            );
            if inherited {
                anyhow::ensure!(
                    after.anim.kind == before.anim.kind && after.glide == before.glide,
                    "包含文件改变了关闭时长或 Glide 参数，原草稿已保留"
                );
                if effect == CloseEffect::Disabled {
                    anyhow::ensure!(
                        after.effect == before.effect
                            && after.custom_shader == before.custom_shader,
                        "包含文件改变了原关闭效果，原草稿已保留"
                    );
                }
            }
            Ok(())
        })?;
        self.config_mgr.set_raw_content(next);
        Ok(())
    }
    fn update_blur_overrides(&mut self, draft: &BlurOverrides) -> anyhow::Result<()> {
        self.flush_config_form()?;
        let next = draft.apply(self.config_mgr.raw_content())?;
        self.config_mgr.set_raw_content(next);
        Ok(())
    }
    fn choose_overview_style(
        &mut self,
        style: Option<clarion_config::OverviewStyle>,
    ) -> anyhow::Result<()> {
        self.flush_config_form()?;
        let before = self.config_mgr.effective_config()?;
        let next =
            crate::appearance_controls::set_overview_style(self.config_mgr.raw_content(), style)?;
        let after = self.config_mgr.merged_config(&next)?;
        if let Some(style) = style {
            anyhow::ensure!(
                after.overview.style == style,
                "概览风格被后续配置覆盖，原草稿已保留。请在全部设置检查主配置与导入文件的载入顺序"
            );
        }
        anyhow::ensure!(
            after.overview.zoom == before.overview.zoom
                && after.overview.backdrop_color == before.overview.backdrop_color
                && after.overview.workspace_shadow == before.overview.workspace_shadow
                && after.animations == before.animations,
            "概览风格修改影响了其他外观或动画参数，原草稿已保留"
        );
        self.config_mgr.set_raw_content(next);
        Ok(())
    }
    fn switch_tab(&mut self, tab: ActiveTab) -> bool {
        match self.flush_config_form() {
            Ok(()) => {
                if self.active_tab != tab {
                    self.back_stack.push(self.active_tab);
                    if self.back_stack.len() > 64 {
                        self.back_stack.remove(0);
                    }
                    self.forward_stack.clear();
                }
                self.active_tab = tab;
                self.preferences_search.clear();
                true
            }
            Err(error) => {
                self.set_status(format!("修改已保留，请先修正当前表单：{error}"), true);
                false
            }
        }
    }

    fn navigate_history(&mut self, backwards: bool) -> bool {
        let target = if backwards {
            self.back_stack.last()
        } else {
            self.forward_stack.last()
        }
        .copied();
        let Some(target) = target else {
            return false;
        };
        if let Err(error) = self.flush_config_form() {
            self.set_status(format!("修改已保留，请先修正当前表单：{error}"), true);
            return false;
        }
        if backwards {
            self.back_stack.pop();
            self.forward_stack.push(self.active_tab);
        } else {
            self.forward_stack.pop();
            self.back_stack.push(self.active_tab);
        }
        self.active_tab = target;
        self.preferences_search.clear();
        true
    }

    fn command_palette_available(&self) -> bool {
        !self.config_mgr.preview_pending()
            && !self.confirm_close
            && !self.confirm_reload
            && !self.show_hot_corners
    }

    pub fn open_command_palette(&mut self) {
        if self.command_palette_available() {
            self.pending_window_focus = None;
            self.command_palette.open();
        } else {
            self.set_status("请先完成当前确认，再打开命令中心", true);
        }
    }

    fn execute_command(&mut self, command: Command) -> anyhow::Result<()> {
        anyhow::ensure!(self.command_palette_available(), "请先完成当前确认");
        if !matches!(command, Command::FocusWindow(_)) {
            self.pending_window_focus = None;
        }
        if let Command::Navigate(tab) = command {
            anyhow::ensure!(
                self.switch_tab(tab),
                "当前表单有无效输入，修改已保留，请先修正后再跳转"
            );
            return Ok(());
        }
        if command == Command::Refresh {
            self.refresh_ipc_data();
            self.set_status("已请求刷新并重新连接 Clarion", false);
            return Ok(());
        }
        anyhow::ensure!(
            self.connection_error.is_none(),
            "Clarion 未连接，请先重新连接"
        );
        if let Command::FocusWindow(id) = command {
            anyhow::ensure!(
                self.pending_window_focus.is_none(),
                "正在确认目标窗口，请稍候"
            );
            anyhow::ensure!(
                self.cached_windows.iter().any(|window| window.id == id),
                "窗口已关闭，请重新搜索"
            );
            // Validate against a fresh reply on the IPC worker before leaving overview.
            // The event cache may still contain a window that closed just before Enter.
            let reply = self.ipc.request(clarion_ipc::Request::Windows)?;
            self.pending_window_focus = Some(PendingWindowFocus { id, reply });
            self.focus_pending = false;
            self.set_status("正在确认目标窗口…", false);
            return Ok(());
        }
        use clarion_ipc::Action;
        let action = match command {
            Command::Overview => Action::ToggleOverview {},
            Command::Expo => Action::ToggleExpo {},
            Command::WindowSpread => Action::ToggleWindowSpread {},
            Command::SwitchTiling => Action::SwitchTilingMode { mode: None },
            Command::ToggleCanvas => Action::ToggleCanvas {},
            Command::ResetCanvas => Action::ResetCanvas {},
            Command::UndoLayout => {
                anyhow::ensure!(
                    self.management
                        .as_ref()
                        .is_some_and(|state| state.undo_steps > 0),
                    "没有可撤销的布局操作"
                );
                Action::UndoLayout {}
            }
            Command::RedoLayout => {
                anyhow::ensure!(
                    self.management
                        .as_ref()
                        .is_some_and(|state| state.redo_steps > 0),
                    "没有可重做的布局操作"
                );
                Action::RedoLayout {}
            }
            Command::Navigate(_) | Command::Refresh | Command::FocusWindow(_) => unreachable!(),
        };
        self.ipc.perform_action(action)?;
        self.set_status("桌面操作请求已发送", false);
        Ok(())
    }

    fn poll_window_focus(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.pending_window_focus.take() else {
            return;
        };
        let result = match pending.reply.try_recv() {
            Ok(result) => result.map_err(anyhow::Error::msg),
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                self.pending_window_focus = Some(pending);
                ctx.request_repaint_after(Duration::from_millis(50));
                return;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err(anyhow::anyhow!("窗口确认连接中断，请重新连接"))
            }
        };
        let result = result.and_then(|response| {
            anyhow::ensure!(self.connection_error.is_none(), "Clarion 已断开连接");
            anyhow::ensure!(
                self.command_palette_available()
                    && !ctx.input(|input| input.viewport().close_requested()),
                "当前需要确认，窗口跳转已取消"
            );
            let clarion_ipc::Response::Windows(windows) = response else {
                anyhow::bail!("窗口确认收到无效响应，请重新连接");
            };
            if !windows.iter().any(|window| window.id == pending.id) {
                self.cached_windows.retain(|window| window.id != pending.id);
                anyhow::bail!("目标窗口已关闭，请重新搜索");
            }
            anyhow::ensure!(
                self.cached_windows
                    .iter()
                    .any(|window| window.id == pending.id),
                "目标窗口已关闭，请重新搜索"
            );
            self.ipc.focus_window(pending.id)
        });
        match result {
            Ok(()) => {
                self.command_palette.close(ctx);
                self.set_status("已请求聚焦目标窗口", false);
            }
            Err(error) => {
                if self.command_palette.is_open() {
                    self.command_palette
                        .show_error(format!("窗口跳转失败：{error}"));
                }
                self.set_status(format!("窗口跳转失败：{error}"), true);
            }
        }
    }

    fn show_command_palette(&mut self, ctx: &egui::Context) {
        let was_open = self.command_palette.is_open();
        let command = self.command_palette.show(
            ctx,
            &CommandSources {
                connected: self.connection_error.is_none(),
                windows: &self.cached_windows,
                workspaces: &self.cached_workspaces,
                management: self.management.as_ref(),
                pending_window_focus: self.pending_window_focus.as_ref().map(|pending| pending.id),
            },
        );
        if was_open && !self.command_palette.is_open() {
            self.pending_window_focus = None;
        }
        if let Some(command) = command {
            match self.execute_command(command) {
                Ok(()) if matches!(command, Command::FocusWindow(_)) => {
                    self.command_palette.focus_search(ctx);
                }
                Ok(()) => self.command_palette.close(ctx),
                Err(error) => {
                    self.command_palette.show_error(error.to_string());
                    self.set_status(format!("命令未执行：{error}"), true);
                }
            }
        }
    }
    fn open_config_file(&mut self, path: &std::path::Path) -> anyhow::Result<()> {
        self.flush_config_form()?;
        self.config_mgr.open_file(path)?;
        self.full_config = Default::default();
        Ok(())
    }
    fn reload_config_from_disk(&mut self) -> anyhow::Result<()> {
        // Keep the structured form and its unsaved fields if reading fails.
        self.config_mgr.reload_from_disk()?;
        self.full_config = Default::default();
        Ok(())
    }
    fn close_guard(&mut self, ctx: &egui::Context) {
        if self.config_mgr.preview_pending() {
            if ctx.input(|i| i.viewport().close_requested()) {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                if let Err(error) = self.config_mgr.finish_preview(&self.ipc, false) {
                    self.set_status(error.to_string(), true);
                }
            }
            self.confirm_close = false;
            return;
        }
        if !self.allow_close
            && ctx.input(|i| i.viewport().close_requested())
            && (self.config_mgr.is_dirty() || self.full_config.has_pending())
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm_close = true;
        }
        if self.confirm_close {
            egui::Window::new("尚有未保存的配置")
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.label("保存后退出，或放弃本次修改。继续编辑会保留所有输入。");
                    ui.horizontal(|ui| {
                        if ui.button("保存并退出").clicked() {
                            match self
                                .flush_config_form()
                                .and_then(|_| self.config_mgr.save_and_reload(&self.ipc))
                            {
                                Ok(()) => {
                                    self.confirm_close = false;
                                    if !self.config_mgr.preview_pending() {
                                        self.allow_close = true;
                                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                                    } else {
                                        // A display change is applied as a guarded preview. Do
                                        // not close before Clarion acknowledges the confirmation,
                                        // otherwise the user cannot keep the new mode and the
                                        // configuration file would remain unsaved.
                                        self.close_after_preview = true;
                                    }
                                }
                                Err(error) => {
                                    self.set_status(format!("保存失败，修改已保留：{error}"), true)
                                }
                            }
                        }
                        if ui.button("放弃修改并退出").clicked() {
                            self.allow_close = true;
                            self.confirm_close = false;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        if ui.button("继续编辑").clicked() {
                            self.confirm_close = false;
                        }
                    });
                });
        }
        if self.confirm_reload {
            egui::Window::new("放弃未保存的修改？")
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.label("从磁盘重新载入会丢弃当前草稿。");
                    ui.horizontal(|ui| {
                        if ui.button("放弃修改并载入").clicked() {
                            self.confirm_reload = false;
                            match self.reload_config_from_disk() {
                                Ok(()) => self.set_status("已重新从磁盘加载配置", false),
                                Err(e) => self.set_status(format!("载入失败: {e}"), true),
                            }
                        }
                        if ui.button("取消").clicked() {
                            self.confirm_reload = false;
                        }
                    });
                });
        }
    }

    fn set_status(&mut self, msg: impl Into<String>, is_error: bool) {
        let msg = if !is_error && self.config_mgr.preview_pending() {
            "显示设置等待确认，配置文件尚未保存".into()
        } else {
            msg.into()
        };
        self.status_message = Some((msg, is_error, std::time::Instant::now()));
    }

    fn display_confirmation(&mut self, ctx: &egui::Context) {
        if !self.config_mgr.preview_pending() {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(100));
        egui::Window::new("保留这些显示设置？")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label("确认前不会写入配置文件。配置中心退出或卡住时，Clarion 仍会自动恢复。");
                if let Some(seconds) = self.config_mgr.preview_seconds() {
                    ui.label(format!("剩余 {seconds} 秒，未确认将恢复原设置。"));
                    ui.horizontal(|ui| {
                        let keep = ui.button("保留并保存").clicked();
                        let cancel = ui.button("恢复原设置").clicked()
                            || ctx.input(|i| i.key_pressed(egui::Key::Escape));
                        if keep || cancel {
                            if let Err(error) =
                                self.config_mgr.finish_preview(&self.ipc, keep && !cancel)
                            {
                                self.set_status(error.to_string(), true);
                            }
                        }
                    });
                } else {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("正在等待 Clarion 响应…");
                    });
                }
            });
    }

    fn prepare_initial_window(&mut self, window: &Window) -> anyhow::Result<()> {
        if !self.initial_window_geometry {
            return Ok(());
        }
        self.initial_window_geometry = false;
        // A preference window has a compact natural size. Only change this
        // newly-created window, once; later activations keep the user's layout.
        // An explicit floating window rule already owns its initial geometry.
        if window.is_floating {
            return Ok(());
        }
        let logical = self
            .cached_workspaces
            .iter()
            .find(|workspace| Some(workspace.id) == window.workspace_id)
            .and_then(|workspace| workspace.output.as_ref())
            .and_then(|name| {
                self.cached_outputs
                    .iter()
                    .find(|output| &output.name == name)
            })
            .and_then(|output| output.logical);
        let width = logical.map_or(700, |output| (output.width as i32 - 48).clamp(480, 700));
        let height = logical.map_or(580, |output| (output.height as i32 - 100).clamp(360, 580));
        for action in [
            clarion_ipc::Action::ToggleWindowFloating {
                id: Some(window.id),
            },
            clarion_ipc::Action::SetWindowWidth {
                id: Some(window.id),
                change: clarion_ipc::SizeChange::SetFixed(width),
            },
            clarion_ipc::Action::SetWindowHeight {
                id: Some(window.id),
                change: clarion_ipc::SizeChange::SetFixed(height),
            },
            clarion_ipc::Action::CenterWindow {
                id: Some(window.id),
            },
        ] {
            self.ipc.perform_action(action)?;
        }
        Ok(())
    }

    fn refresh_ipc_data(&mut self) {
        self.last_ipc_poll = std::time::Instant::now();
        self.ipc.refresh();
        let Some(snapshot) = self.ipc.take_snapshot() else {
            return;
        };
        match snapshot {
            Ok(crate::ipc::Snapshot {
                version,
                mut outputs,
                mut workspaces,
                mut windows,
                management,
                frame_timings,
            }) => {
                outputs.sort_by(|a, b| a.name.cmp(&b.name));
                workspaces.sort_by(|a, b| (&a.output, a.idx).cmp(&(&b.output, b.idx)));
                windows.sort_by_key(|window| window.id);
                self.management = management;
                self.frame_timings = frame_timings;
                self.clarion_version = version;
                self.cached_outputs = outputs;
                self.cached_workspaces = workspaces;
                self.cached_windows = windows;
                self.connection_error = None;
            }
            Err(error) => {
                self.connection_error = Some(error.to_string());
                self.management = None;
                self.frame_timings = None;
                self.cached_windows.clear();
                self.cached_workspaces.clear();
                self.cached_outputs.clear();
                self.clarion_version = "Unknown".into();
            }
        }
        self.last_ipc_poll = std::time::Instant::now();
    }

    fn poll_display_preview(&mut self, ctx: &egui::Context) {
        if let Some(result) = self.config_mgr.poll_preview(&self.ipc) {
            match result {
                Ok(message) => {
                    let confirmed = message.contains("已确认并保存");
                    self.set_status(message, false);
                    // The first successful reply only starts the countdown.
                    // Preserve the exit intent until confirmation or cancellation finishes.
                    if self.close_after_preview && !self.config_mgr.preview_pending() {
                        self.close_after_preview = false;
                        if confirmed {
                            self.allow_close = true;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    }
                }
                Err(error) => {
                    self.close_after_preview = false;
                    self.set_status(error.to_string(), true);
                }
            }
        }
    }
}

impl ClarionSettingsApp {
    fn update_ui(&mut self, ctx: &egui::Context) {
        // A desktop light/dark switch applies to the settings window in place,
        // without waiting for the next launch.
        self.desktop_theme.poll(ctx);
        let previous_ui = (
            self.active_tab,
            self.show_hot_corners,
            self.command_palette.is_open(),
            self.confirm_close,
            self.confirm_reload,
        );
        self.poll_display_preview(ctx);
        if let Some(snapshot) = self.ipc.take_snapshot() {
            match snapshot {
                Ok(mut snapshot) => {
                    snapshot.outputs.sort_by(|a, b| a.name.cmp(&b.name));
                    snapshot
                        .workspaces
                        .sort_by(|a, b| (&a.output, a.idx).cmp(&(&b.output, b.idx)));
                    snapshot.windows.sort_by_key(|w| w.id);
                    self.management = snapshot.management;
                    self.frame_timings = snapshot.frame_timings;
                    self.cached_outputs = snapshot.outputs;
                    self.cached_workspaces = snapshot.workspaces;
                    self.cached_windows = snapshot.windows;
                    self.clarion_version = snapshot.version;
                    self.connection_error = None;
                }
                Err(error) => {
                    self.connection_error = Some(error);
                    self.management = None;
                    self.frame_timings = None;
                    self.cached_windows.clear();
                    self.cached_workspaces.clear();
                    self.cached_outputs.clear();
                    self.clarion_version = "Unknown".into();
                }
            }
        }
        for message in self.ipc.take_notices() {
            self.set_status(message, false);
        }
        for error in self.ipc.take_errors() {
            self.set_status(format!("操作失败: {error}"), true);
        }
        self.config_mgr.validate_if_due();
        if matches!(
            self.config_mgr.validation_status(),
            ValidationStatus::NotChecked
        ) {
            ctx.request_repaint_after(Duration::from_millis(300));
        }

        // Handle single instance toggle/focus signal
        if let Some(flag) = &self.toggle_flag {
            let command = flag.swap(0, std::sync::atomic::Ordering::SeqCst);
            if command != 0 {
                let is_focused = ctx.input(|i| i.viewport().focused.unwrap_or(false));
                if command == crate::instance::COMMAND_PALETTE {
                    self.open_command_palette();
                    self.focus_pending = true;
                } else if command == crate::instance::TOGGLE && is_focused {
                    self.pending_window_focus = None;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                } else {
                    self.pending_window_focus = None;
                    self.focus_pending = true;
                }
            }
        }

        let palette_was_open = self.command_palette.is_open();
        self.command_palette.handle_shortcut(
            ctx,
            self.command_palette_available()
                && !ctx.input(|input| input.viewport().close_requested()),
        );
        if !palette_was_open && self.command_palette.is_open() {
            // A newer settings activation supersedes an older asynchronous jump.
            self.pending_window_focus = None;
        }

        // winit's Wayland focus_window() is a no-op. Wait for our own
        // mapped window to appear, then leave overview and focus its exact ID.
        if self.focus_pending {
            if let Some(window) = self
                .cached_windows
                .iter()
                .find(|window| window.pid == Some(std::process::id() as i32))
                .cloned()
            {
                if let Err(error) = self.prepare_initial_window(&window) {
                    self.set_status(format!("无法设置偏好窗口初始布局：{error}"), true);
                }
                match self.ipc.focus_window(window.id) {
                    Ok(()) => self.focus_pending = false,
                    Err(error) => self.set_status(format!("无法聚焦设置窗口: {error}"), true),
                }
            }
        }

        // Auto-refresh IPC data every 3 seconds
        if self.last_ipc_poll.elapsed() > Duration::from_secs(3) {
            self.refresh_ipc_data();
        }

        // Check the desktop preference about once a second; IPC data still
        // refreshes on its own 3-second schedule.
        ctx.request_repaint_after(Duration::from_secs(1));

        // Consume palette keys before the editor; disable background widgets for
        // this whole frame even if Enter or Escape closes the foreground panel.
        let palette_was_open = self.command_palette.is_open();
        self.show_command_palette(ctx);
        // Dismissal and a newer command take priority over a ready focus reply.
        self.poll_window_focus(ctx);

        let blocked = self.config_mgr.preview_pending()
            || palette_was_open
            || self.command_palette.is_open()
            || self.confirm_close
            || self.confirm_reload
            || self.show_hot_corners
            || ctx.input(|input| input.viewport().close_requested());
        let mut save_requested = !blocked
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, egui::Key::S));
        let mut check_requested = false;

        // Apply navigation after the current page has processed TextEdit/IME
        // events, for the same draft-preservation reason as saving below.
        let mut navigation = if !blocked {
            ctx.input_mut(|input| {
                if input.consume_key(egui::Modifiers::ALT, egui::Key::ArrowLeft) {
                    Some(PreferenceNavigation::Back)
                } else if input.consume_key(egui::Modifiers::ALT, egui::Key::ArrowRight) {
                    Some(PreferenceNavigation::Forward)
                } else {
                    None
                }
            })
        } else {
            None
        };
        self.preferences_titlebar(ctx);
        if let Some(request) = self.preferences_toolbar(ctx, blocked) {
            navigation = Some(request);
        }

        // A fixed action bar keeps saving reachable even on long settings pages.
        egui::TopBottomPanel::bottom("config_actions")
            .frame(
                Frame::none()
                    .fill(palette::sidebar())
                    .inner_margin(Margin::symmetric(14., 8.)),
            )
            .show(ctx, |ui| {
                if blocked {
                    ui.disable();
                }
                let dirty = self.config_mgr.is_dirty() || self.full_config.has_pending();
                let invalid = matches!(
                    self.config_mgr.validation_status(),
                    ValidationStatus::Error(_)
                );
                ui.horizontal(|ui| {
                    icons::icon(
                        ui,
                        if invalid {
                            Icon::Warning
                        } else if dirty {
                            Icon::Settings
                        } else {
                            Icon::Check
                        },
                        18.,
                        if invalid {
                            palette::error()
                        } else if dirty {
                            palette::warning()
                        } else {
                            palette::muted()
                        },
                    );
                    let summary_width = (ui.available_width() - 255.).max(70.);
                    ui.vertical(|ui| {
                        ui.set_width(summary_width);
                        ui.label(
                            RichText::new(if invalid {
                                "配置需修正，暂时无法保存"
                            } else if dirty {
                                "有未保存的修改"
                            } else {
                                "当前没有未保存修改"
                            })
                            .size(13.)
                            .color(if invalid {
                                palette::error()
                            } else if dirty {
                                palette::warning()
                            } else {
                                palette::muted()
                            }),
                        );
                        ui.add(
                            egui::Label::new(
                                RichText::new(
                                    self.config_mgr
                                        .config_path()
                                        .file_name()
                                        .unwrap_or_default()
                                        .to_string_lossy(),
                                )
                                .size(11.)
                                .color(palette::muted()),
                            )
                            .truncate(),
                        )
                        .on_hover_text(self.config_mgr.config_path().display().to_string());
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add_enabled_ui(dirty, |ui| {
                                icons::primary_button(ui, Icon::Save, "保存并应用")
                            })
                            .inner
                            .clicked()
                        {
                            save_requested = true;
                        }
                        if icons::button(ui, Icon::Check, "检查修改").clicked() {
                            check_requested = true;
                        }
                    });
                });
            });

        egui::CentralPanel::default()
            .frame(
                Frame::none()
                    .fill(palette::background())
                    .inner_margin(Margin::same(20.)),
            )
            .show(ctx, |ui| {
                if blocked {
                    ui.disable();
                }
                if let Some((ref message, is_error, time)) = self.status_message {
                    if time.elapsed() < Duration::from_secs(6) {
                        Frame::none()
                            .fill(palette::surface())
                            .stroke(Stroke::new(
                                1.0_f32,
                                if is_error {
                                    palette::error()
                                } else {
                                    palette::success()
                                },
                            ))
                            .rounding(Rounding::same(8.))
                            .inner_margin(Margin::same(12.))
                            .show(ui, |ui| {
                                ui.horizontal_wrapped(|ui| {
                                    icons::icon(
                                        ui,
                                        if is_error { Icon::Warning } else { Icon::Check },
                                        18.,
                                        if is_error {
                                            palette::error()
                                        } else {
                                            palette::success()
                                        },
                                    );
                                    ui.add(egui::Label::new(message).wrap());
                                });
                            });
                        ui.add_space(12.);
                    }
                }
                match self.active_tab {
                    ActiveTab::Dashboard => self.ui_dashboard(ui),
                    ActiveTab::Windows => self.ui_windows(ui),
                    ActiveTab::Layout => self.ui_layout(ui),
                    ActiveTab::Appearance => self.ui_appearance(ui),
                    ActiveTab::InputHotCorners => self.ui_input_hot_corners(ui),
                    ActiveTab::Trackpad => self.ui_trackpad(ui),
                    ActiveTab::Outputs => self.ui_outputs(ui),
                    ActiveTab::FullConfig => self.ui_full_config(ui),
                    ActiveTab::ConfigEditor => self.ui_config_editor(ui),
                }
            });
        if let Some(request) = navigation.filter(|_| {
            !blocked && self.command_palette_available() && !self.command_palette.is_open()
        }) {
            let navigated = match request {
                PreferenceNavigation::Open(tab) => self.switch_tab(tab),
                PreferenceNavigation::Back => self.navigate_history(true),
                PreferenceNavigation::Forward => self.navigate_history(false),
            };
            if navigated {
                ctx.memory_mut(|memory| memory.close_popup());
            }
        }
        // TextEdit and IME commits are processed while rendering the page. Save
        // only after that, so typing and Ctrl+S in one frame save the latest text.
        if check_requested && self.command_palette_available() && !self.command_palette.is_open() {
            match self.flush_config_form() {
                Ok(()) => {
                    self.config_mgr.validate();
                    match self.config_mgr.validation_status() {
                        ValidationStatus::Valid => {
                            self.set_status("检查通过，可以保存并应用", false)
                        }
                        ValidationStatus::Error(error) => {
                            self.set_status(format!("请修正配置：{error}"), true)
                        }
                        ValidationStatus::NotChecked => {}
                    }
                }
                Err(error) => self.set_status(error.to_string(), true),
            }
        }
        if save_requested {
            self.save_changes();
        }
        let title = preferences_home::title(self.active_tab);
        if ctx.input(|input| input.viewport().title.as_deref() != Some(title)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.into()));
        }
        self.close_guard(ctx);
        self.display_confirmation(ctx);
        self.hot_corners_dialog(ctx, blocked);
        Self::resize_handle(ctx);
        if previous_ui
            != (
                self.active_tab,
                self.show_hot_corners,
                self.command_palette.is_open(),
                self.confirm_close,
                self.confirm_reload,
            )
        {
            // Toolbar navigation is applied after rendering the current editor.
            // Paint the new pane/sheet on the next frame, not the 3 s IPC timer.
            ctx.request_repaint();
        }
    }
}

impl eframe::App for ClarionSettingsApp {
    fn raw_input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        // egui blurs a TextEdit on a pointer press before processing its IME
        // commit. Let that commit reach the focused editor first; replay the
        // same pointer sequence on the next frame, in its original order.
        if !self.deferred_pointer_events.is_empty() {
            input
                .events
                .splice(0..0, std::mem::take(&mut self.deferred_pointer_events));
        }
        let committing = input
            .events
            .iter()
            .any(|event| matches!(event, egui::Event::Ime(egui::ImeEvent::Commit(_))));
        let pointer_pressed = input
            .events
            .iter()
            .any(|event| matches!(event, egui::Event::PointerButton { pressed: true, .. }));
        if committing && pointer_pressed && ctx.memory(|memory| memory.focused().is_some()) {
            input.events.retain(|event| {
                if matches!(
                    event,
                    egui::Event::PointerMoved(_)
                        | egui::Event::PointerButton { .. }
                        | egui::Event::PointerGone
                ) {
                    self.deferred_pointer_events.push(event.clone());
                    false
                } else {
                    true
                }
            });
            ctx.request_repaint();
        }
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.update_ui(ctx);
    }
}

impl ClarionSettingsApp {
    fn save_changes(&mut self) {
        if !self.command_palette_available() || self.command_palette.is_open() {
            return;
        }
        let had_changes = self.config_mgr.is_dirty() || self.full_config.has_pending();
        match self
            .flush_config_form()
            .and_then(|_| self.config_mgr.save_and_reload(&self.ipc))
        {
            Ok(()) if self.config_mgr.preview_pending() => {
                self.set_status("正在预览显示设置，请确认保留后完成保存", false)
            }
            Ok(()) => self.set_status("配置已保存，已请求桌面应用修改", false),
            Err(error) => {
                let message = if had_changes
                    && !self.config_mgr.is_dirty()
                    && !self.full_config.has_pending()
                {
                    format!("配置文件已保存，应用修改未完成：{error}")
                } else {
                    format!("修改已保留，保存失败：{error}")
                };
                self.set_status(message, true);
            }
        }
    }

    fn preferences_titlebar(&self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("preferences_titlebar")
            .exact_height(30.)
            .show_separator_line(false)
            .frame(Frame::none().fill(palette::sidebar()))
            .show(ctx, |ui| {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(ui.available_width(), 30.), Sense::hover());
                let focused = ctx.input(|input| input.viewport().focused.unwrap_or(true));
                let controls = egui::Rect::from_min_max(
                    rect.left_top(),
                    egui::pos2(rect.left() + 80., rect.bottom()),
                );
                let show_symbols = ui.rect_contains_pointer(controls);
                for (index, label, color) in [
                    (0, "关闭", egui::Color32::from_rgb(255, 95, 87)),
                    (1, "最小化", egui::Color32::from_rgb(254, 188, 46)),
                    (2, "缩放", egui::Color32::from_rgb(40, 200, 64)),
                ] {
                    let center =
                        egui::pos2(rect.left() + 20. + index as f32 * 20., rect.center().y);
                    let response = ui.interact(
                        egui::Rect::from_center_size(center, egui::vec2(20., 24.)),
                        ui.id().with(("traffic-light", index)),
                        Sense::click(),
                    );
                    let fill = if focused || show_symbols {
                        color
                    } else if palette::is_dark() {
                        egui::Color32::from_gray(90)
                    } else {
                        egui::Color32::from_gray(202)
                    };
                    ui.painter().circle(
                        center,
                        6.,
                        fill,
                        Stroke::new(0.5_f32, egui::Color32::from_black_alpha(35)),
                    );
                    if show_symbols || response.has_focus() {
                        let stroke = Stroke::new(1.0_f32, egui::Color32::from_black_alpha(160));
                        match index {
                            0 => {
                                ui.painter().line_segment(
                                    [center + egui::vec2(-2., -2.), center + egui::vec2(2., 2.)],
                                    stroke,
                                );
                                ui.painter().line_segment(
                                    [center + egui::vec2(2., -2.), center + egui::vec2(-2., 2.)],
                                    stroke,
                                );
                            }
                            1 => {
                                ui.painter().line_segment(
                                    [center + egui::vec2(-3., 0.), center + egui::vec2(3., 0.)],
                                    stroke,
                                );
                            }
                            _ => {
                                ui.painter().line_segment(
                                    [center + egui::vec2(-2., 2.), center + egui::vec2(2., -2.)],
                                    stroke,
                                );
                            }
                        }
                    }
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
                    });
                    if response.on_hover_text(label).clicked() {
                        ctx.send_viewport_cmd(match index {
                            0 => egui::ViewportCommand::Close,
                            1 => egui::ViewportCommand::Minimized(true),
                            _ => egui::ViewportCommand::Maximized(
                                !ctx.input(|input| input.viewport().maximized.unwrap_or(false)),
                            ),
                        });
                    }
                }
                let title = ui.interact(
                    egui::Rect::from_min_max(
                        egui::pos2(controls.right(), rect.top()),
                        rect.right_bottom(),
                    ),
                    ui.id().with("title-drag"),
                    Sense::click_and_drag(),
                );
                if title.drag_started() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                if title.double_clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(
                        !ctx.input(|input| input.viewport().maximized.unwrap_or(false)),
                    ));
                }
                ui.painter().text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    preferences_home::title(self.active_tab),
                    egui::FontId::proportional(13.),
                    if focused {
                        palette::text()
                    } else {
                        palette::muted()
                    },
                );
            });
    }

    fn preferences_toolbar(
        &mut self,
        ctx: &egui::Context,
        blocked: bool,
    ) -> Option<PreferenceNavigation> {
        let mut request = None;
        egui::TopBottomPanel::top("preferences_toolbar")
            .exact_height(42.)
            .frame(
                Frame::none()
                    .fill(palette::sidebar())
                    .inner_margin(Margin::symmetric(14., 8.)),
            )
            .show(ctx, |ui| {
                if blocked {
                    ui.disable();
                }
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 1.;
                    if Self::toolbar_button(ui, "后退 · Alt+←", 0, !self.back_stack.is_empty())
                        .clicked()
                    {
                        request = Some(PreferenceNavigation::Back);
                    }
                    if Self::toolbar_button(ui, "前进 · Alt+→", 1, !self.forward_stack.is_empty())
                        .clicked()
                    {
                        request = Some(PreferenceNavigation::Forward);
                    }
                    ui.add_space(10.);
                    let home = Self::toolbar_button(ui, "全部显示", 2, true);
                    if home.clicked() {
                        request = Some(PreferenceNavigation::Open(ActiveTab::Dashboard));
                    }
                    home.context_menu(|ui| {
                        if blocked {
                            ui.disable();
                        }
                        for pane in preferences_home::PANES {
                            if ui
                                .selectable_label(self.active_tab == pane.tab, pane.title)
                                .clicked()
                            {
                                request = Some(PreferenceNavigation::Open(pane.tab));
                                ui.close_menu();
                            }
                        }
                    });
                    ui.add_space(7.);
                    if ui
                        .small_button("命令中心")
                        .on_hover_text("搜索窗口和桌面操作 · Ctrl+K")
                        .clicked()
                    {
                        self.open_command_palette();
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let search_id = egui::Id::new("preferences-search");
                        let search_was_focused = ctx.memory(|memory| memory.has_focus(search_id));
                        let ime_keys = if search_was_focused && !blocked {
                            ctx.input(|input| {
                                let mut block_keys = self.preferences_ime_composing;
                                for event in &input.events {
                                    if let egui::Event::Ime(event) = event {
                                        match event {
                                            egui::ImeEvent::Enabled => {}
                                            egui::ImeEvent::Preedit(text) => {
                                                block_keys |= self.preferences_ime_composing
                                                    || !text.is_empty();
                                                self.preferences_ime_composing = !text.is_empty();
                                            }
                                            egui::ImeEvent::Commit(_) => {
                                                block_keys = true;
                                                self.preferences_ime_composing = false;
                                            }
                                            egui::ImeEvent::Disabled => {
                                                block_keys |= self.preferences_ime_composing;
                                                self.preferences_ime_composing = false;
                                            }
                                        }
                                    }
                                }
                                block_keys
                            })
                        } else {
                            self.preferences_ime_composing = false;
                            false
                        };
                        let (up, down, enter, escape) = if search_was_focused && !blocked {
                            ctx.input_mut(|input| {
                                let keys = (
                                    input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                                    input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                                    input.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                                    input.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                                );
                                if ime_keys {
                                    (false, false, false, false)
                                } else {
                                    keys
                                }
                            })
                        } else {
                            (false, false, false, false)
                        };
                        let search = ui.add_sized(
                            [ui.available_width().clamp(100., 190.), 22.],
                            TextEdit::singleline(&mut self.preferences_search)
                                .id(search_id)
                                .hint_text("搜索设置")
                                .margin(egui::vec2(7., 3.)),
                        );
                        if !blocked
                            && ctx.input_mut(|input| {
                                input.consume_key(egui::Modifiers::CTRL, egui::Key::F)
                            })
                        {
                            search.request_focus();
                        }
                        let popup_id = ui.id().with("preference-search-results");
                        let has_query = !self.preferences_search.trim().is_empty();
                        if !has_query || blocked {
                            if ctx.memory(|memory| memory.is_popup_open(popup_id)) {
                                ctx.memory_mut(|memory| memory.close_popup());
                            }
                        } else if search.changed() || search.gained_focus() {
                            ctx.memory_mut(|memory| memory.open_popup(popup_id));
                        }
                        let matches = preferences_home::matching_panes(&self.preferences_search);
                        if search.changed() {
                            self.preferences_search_selected = 0;
                        }
                        if down && !matches.is_empty() {
                            self.preferences_search_selected =
                                (self.preferences_search_selected + 1).min(matches.len() - 1);
                        }
                        if up {
                            self.preferences_search_selected =
                                self.preferences_search_selected.saturating_sub(1);
                        }
                        self.preferences_search_selected = self
                            .preferences_search_selected
                            .min(matches.len().saturating_sub(1));
                        if !blocked && has_query && enter {
                            if let Some(pane) = matches.get(self.preferences_search_selected) {
                                request = Some(PreferenceNavigation::Open(pane.tab));
                            }
                        }
                        if escape {
                            self.preferences_search.clear();
                            ctx.memory_mut(|memory| memory.close_popup());
                            search.request_focus();
                        } else if ime_keys {
                            search.request_focus();
                        }
                        if has_query {
                            egui::popup_below_widget(
                                ui,
                                popup_id,
                                &search,
                                egui::PopupCloseBehavior::CloseOnClickOutside,
                                |ui| {
                                    if matches.is_empty() {
                                        ui.weak("没有匹配的设置");
                                    }
                                    if blocked {
                                        ui.disable();
                                    }
                                    for (index, pane) in matches.into_iter().enumerate() {
                                        if ui
                                            .selectable_label(
                                                self.preferences_search_selected == index,
                                                pane.title,
                                            )
                                            .clicked()
                                        {
                                            request = Some(PreferenceNavigation::Open(pane.tab));
                                        }
                                    }
                                },
                            );
                        }
                    });
                });
            });
        request.filter(|_| !blocked)
    }

    fn toolbar_button(ui: &mut Ui, label: &str, kind: usize, enabled: bool) -> egui::Response {
        let response = ui.add_enabled(enabled, Button::new("").min_size(egui::vec2(28., 22.)));
        let color = if enabled && ui.is_enabled() {
            ui.style().interact(&response).fg_stroke.color
        } else {
            egui::Color32::from_gray(170)
        };
        let center = response.rect.center();
        if kind < 2 {
            let direction = if kind == 0 { -1. } else { 1. };
            ui.painter().add(egui::Shape::line(
                vec![
                    center + egui::vec2(-2. * direction, -4.),
                    center + egui::vec2(2. * direction, 0.),
                    center + egui::vec2(-2. * direction, 4.),
                ],
                Stroke::new(1.3_f32, color),
            ));
        } else {
            for row in 0..3 {
                for column in 0..3 {
                    ui.painter().rect_filled(
                        egui::Rect::from_center_size(
                            center + egui::vec2((column as f32 - 1.) * 4., (row as f32 - 1.) * 4.),
                            egui::vec2(2.5, 2.5),
                        ),
                        0.,
                        color,
                    );
                }
            }
        }
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled() && enabled, label)
        });
        response.on_hover_text(label)
    }

    fn page_heading(ui: &mut Ui, icon: Icon, title: &str, detail: &str) {
        ui.horizontal(|ui| {
            match icon {
                Icon::Appearance => icons::preference_icon(ui, icons::PreferenceIcon::General, 32.),
                Icon::Display => icons::preference_icon(ui, icons::PreferenceIcon::Displays, 32.),
                Icon::Input => icons::preference_icon(ui, icons::PreferenceIcon::Keyboard, 32.),
                _ => icons::icon(ui, icon, 20., palette::muted()),
            };
            ui.label(
                RichText::new(title)
                    .size(20.)
                    .strong()
                    .color(palette::text()),
            );
        });
        ui.add_space(5.);
        ui.add(egui::Label::new(RichText::new(detail).color(palette::muted()).size(13.)).wrap());
        ui.add_space(16.);
    }

    fn ui_frame_timings(&self, ui: &mut Ui) {
        ui.heading("显示性能");
        ui.label("最近最多 240 次重绘，每 3 秒刷新。耗时为 CPU 墙钟时间，包含等待。");
        let Some(outputs) = &self.frame_timings else {
            ui.weak("帧统计暂不可用；旧版合成器需要更新并重新登录后才能提供此数据。");
            return;
        };
        if outputs.is_empty() {
            ui.weak("暂无连接的显示器。");
        }
        for output in outputs {
            ui.add_space(8.0);
            Frame::none()
                .fill(palette::surface())
                .rounding(Rounding::same(8.0))
                .inner_margin(Margin::same(14.0))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(&output.output);
                        ui.label(if output.variable_refresh_rate { "VRR 已开启" } else { "VRR 未开启" });
                        if let Some(interval) = output.refresh_interval_ms {
                            ui.label(format!("刷新间隔 {interval:.2} ms"));
                        }
                    });
                    if let Some(budget) = output.kms_latch_budget_ms {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(format!("KMS latch 预算 {budget:.2} ms"));
                            ui.label(format!("延后重绘 {} 次", output.kms_latch_deferrals));
                            if let Some(samples) = &output.kms_latch_delay_ms {
                                ui.weak(format!(
                                    "预定等待 P50 {:.2} ms · P95 {:.2} ms",
                                    samples.p50, samples.p95
                                ));
                            }
                        });
                        if let Some(samples) = &output.kms_latch_wakeup_lateness_ms {
                            ui.weak(format!(
                                "实际唤醒晚于期限：P50 {:.2} ms · P95 {:.2} ms（{} 个样本）",
                                samples.p50, samples.p95, samples.samples
                            ));
                        }
                    }
                    if let Some(samples) = &output.render_time_ms {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(format!("P50  {:.2} ms", samples.p50));
                            ui.label(format!("P95  {:.2} ms", samples.p95));
                            ui.label(format!("峰值  {:.2} ms", samples.max));
                            ui.weak(format!("{} 个样本", samples.samples));
                        });
                        if let Some(interval) = output.refresh_interval_ms.filter(|v| *v > 0.) {
                            let fraction = samples.p95 / interval;
                            let color = if fraction >= 1. {
                                palette::error()
                            } else {
                                palette::accent()
                            };
                            ui.add(egui::ProgressBar::new(fraction.clamp(0., 1.) as f32)
                                .fill(color)
                                .text(format!("P95 占刷新间隔 {:.0}%", fraction * 100.)))
                                .on_hover_text("超过刷新间隔说明准备与提交可能来不及完成；未超过也不保证 GPU 能按时呈现。");
                        }
                    } else {
                        ui.weak("等待首次重绘。");
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!("已提交 {}", output.submitted));
                        ui.label(format!("无需更新 {}", output.no_damage));
                        ui.label(format!("已跳过 {}", output.skipped));
                    });
                    if output.presentation_error_ms.is_some() {
                        ui.label(format!("固定刷新逾期呈现：{} / {}",
                            output.late_presentations, output.fixed_refresh_presentations))
                            .on_hover_text("比预测时间晚超过半个刷新间隔的硬件呈现次数。计数从显示器连接开始；不等于应用丢帧或输入延迟。VRR 不计入此比率。");
                    } else {
                        ui.weak("暂无硬件呈现时间；嵌套会话不提供此数据。");
                    }
                });
        }
    }

    fn ui_dashboard(&mut self, ui: &mut Ui) {
        ScrollArea::vertical()
            .id_salt("preferences_home_scroll")
            .show(ui, |ui| {
                if let Some(tab) = preferences_home::show(ui, &self.preferences_search) {
                    self.switch_tab(tab);
                }
                ui.add_space(20.);
                ui.separator();
                ui.add_space(8.);
                ui.horizontal_wrapped(|ui| {
                    ui.weak(if self.connection_error.is_none() {
                        "Clarion 桌面已连接"
                    } else {
                        "离线编辑"
                    })
                    .on_hover_text(
                        self.connection_error
                            .as_deref()
                            .unwrap_or("配置修改需要保存并应用"),
                    );
                    if self.connection_error.is_none() {
                        ui.weak(format!(
                            "{} 个显示器 · {} 个工作区 · {} 个窗口",
                            self.cached_outputs.len(),
                            self.cached_workspaces.len(),
                            self.cached_windows.len()
                        ));
                    }
                    if ui.small_button("刷新连接").clicked() {
                        self.refresh_ipc_data();
                    }
                });
                egui::CollapsingHeader::new("显示性能").show(ui, |ui| self.ui_frame_timings(ui));
            });
    }

    fn ui_layout(&mut self, ui: &mut Ui) {
        ScrollArea::vertical().show(ui, |ui| {
            Self::page_heading(
                ui,
                Icon::Layout,
                "窗口布局",
                "调整窗口间距和居中习惯，让桌面排列更舒服。",
            );

            ui.add_enabled_ui(self.connection_error.is_none(), |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("切换无限画布").clicked() {
                        if let Err(err) = self
                            .ipc
                            .perform_action(clarion_ipc::Action::ToggleCanvas {})
                        {
                            self.set_status(format!("切换无限画布失败: {err}"), true);
                        }
                    }
                    if ui.button("画布回到原点").clicked() {
                        if let Err(err) =
                            self.ipc.perform_action(clarion_ipc::Action::ResetCanvas {})
                        {
                            self.set_status(format!("画布复位失败: {err}"), true);
                        }
                    }
                });
            });
            ui.label("无限画布：按住 Mod 键，用中键拖动或滚轮、双指滚动平移。")
                .on_hover_text("Mod 是配置中指定的桌面快捷键修饰键；默认通常为 Super（徽标键）。");
            ui.add_space(16.0);

            // Gaps
            ui.label(
                RichText::new("窗口间距 (Gaps)")
                    .color(palette::text())
                    .strong(),
            );
            ui.horizontal(|ui| {
                let s = &mut self.config_mgr.visual_settings_mut().gaps;
                ui.add(
                    egui::Slider::new(s, 0.0..=64.0)
                        .clamping(egui::SliderClamping::Edits)
                        .text("px"),
                );
            });
            ui.label(
                RichText::new("控制平铺窗口与屏幕边缘及窗口之间的留白间距。")
                    .size(11.5)
                    .color(palette::muted()),
            );
            ui.add_space(14.0);

            // Center focused column
            ui.label(
                RichText::new("居中聚焦列 (Center Focused Column)")
                    .color(palette::text())
                    .strong(),
            );
            let current = &mut self.config_mgr.visual_settings_mut().center_focused_column;
            ui.horizontal(|ui| {
                ui.selectable_value(current, "never".to_string(), "从不 (Never)");
                ui.selectable_value(current, "always".to_string(), "始终居中 (Always)");
                ui.selectable_value(current, "on-overflow".to_string(), "溢出时 (On Overflow)");
            });
            ui.add_space(14.0);

            // Always center single column
            let center_single = &mut self
                .config_mgr
                .visual_settings_mut()
                .always_center_single_column;
            ui.checkbox(center_single, "单个窗口列始终在屏幕居中显示");
            ui.add_space(20.0);
        });
    }

    fn ui_appearance(&mut self, ui: &mut Ui) {
        ScrollArea::vertical().id_salt("general_preferences").show(ui, |ui| {
            Self::page_heading(ui, Icon::Appearance, "通用", "窗口边框、动画与桌面视觉设置。");
            ui.group(|ui| {
                ui.strong("动效与响应");
                if let Some(animations) = self.config_mgr.effective_animations() {
                    ui.weak(format!("已解析配置（含继承）：动画{} · 时长 {} 倍；页面修改保存后应用。",
                        if animations.off { "关闭" } else { "开启" }, animations.slowdown));
                }
                ui.label(
                    "选择一种动效习惯。减少动态会关闭过渡和窗口浮动；修改后可在底部保存。",
                );
                let settings = self.config_mgr.visual_settings_mut();
                ui.horizontal_wrapped(|ui| {
                    for (title, preset, hint) in [
                        (
                            "流畅",
                            MotionPreset::Fluid,
                            "标准速度，开启弹性窗口、立方体和 Glide",
                        ),
                        (
                            "轻量",
                            MotionPreset::Lightweight,
                            "缩短过渡，关闭三项 3D 动效；实际性能取决于应用和显卡",
                        ),
                        (
                            "减少动态",
                            MotionPreset::Reduced,
                            "关闭动画，保留细节参数，方便重新启用",
                        ),
                    ] {
                        if ui.button(title).on_hover_text(hint).clicked() {
                            settings.apply_motion_preset(preset);
                        }
                    }
                });
                egui::ComboBox::from_label("动画总开关")
                    .selected_text(match settings.animations_enabled {
                        Some(true) => "开启",
                        Some(false) => "关闭",
                        None => "继承配置",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut settings.animations_enabled, None, "继承配置");
                        ui.selectable_value(&mut settings.animations_enabled, Some(true), "开启");
                        ui.selectable_value(&mut settings.animations_enabled, Some(false), "关闭");
                    });
                let mut custom_speed = settings.animation_slowdown.is_some();
                if ui.checkbox(&mut custom_speed, "自定义动画时长").changed() {
                    settings.animation_slowdown = custom_speed.then_some(1.);
                }
                if let Some(slowdown) = &mut settings.animation_slowdown {
                    ui.add(egui::Slider::new(slowdown, 0.25..=2.0).clamping(egui::SliderClamping::Edits).text("时长倍数"));
                    ui.weak("小于 1 更快，大于 1 更慢；关闭自定义后继承配置中的速度。");
                }
            });
            ui.add_space(16.0);

            // Border & Focus Ring Width
            ui.label(RichText::new("边框宽度设置").color(palette::text()).strong());
            ui.horizontal(|ui| {
                let s = &mut self.config_mgr.visual_settings_mut().border_width;
                ui.add(egui::Slider::new(s, 0.0..=12.0).clamping(egui::SliderClamping::Edits).text("边框宽度 (px)"));
            });
            ui.horizontal(|ui| {
                let s = &mut self.config_mgr.visual_settings_mut().focus_ring_width;
                ui.add(egui::Slider::new(s, 0.0..=12.0).clamping(egui::SliderClamping::Edits).text("聚焦光环宽度 (px)"));
            });
            ui.add_space(16.0);

            ui.separator();
            egui::CollapsingHeader::new("工作区概览").show(ui, |ui| self.ui_overview_style(ui));
            egui::CollapsingHeader::new("窗口关闭效果").show(ui, |ui| self.ui_close_effect(ui));
            egui::CollapsingHeader::new("窗口与工作区动效").show(ui, |ui| {
            Frame::none()
                .fill(palette::surface())
                .stroke(Stroke::new(1.0_f32, palette::border()))
                .rounding(Rounding::same(6.0))
                .inner_margin(Margin::same(12.0))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new("窗口与工作区动效")
                            .color(palette::accent())
                            .strong(),
                    );
                    ui.label("运行时快捷开关不改配置；这里保存后会通过 IPC 热重载。");
                    ui.add_space(10.0);

                    let vs = self.config_mgr.visual_settings_mut();
                    ui.checkbox(
                        &mut vs.window_wobble_enabled,
                        "弹性窗口（拖动、缩放与回弹）",
                    );
                    ui.add_enabled_ui(vs.window_wobble_enabled, |ui| {
                        egui::CollapsingHeader::new("弹性窗口的高级参数").show(ui, |ui| {
                        ui.add(egui::Slider::new(&mut vs.wobble_mesh, 2..=32).clamping(egui::SliderClamping::Edits).text("网格密度"));
                        ui.add(
                            egui::Slider::new(&mut vs.wobble_stiffness, 1.0..=1000.0).clamping(egui::SliderClamping::Edits)
                                .text("弹簧刚度"),
                        );
                        ui.add(egui::Slider::new(&mut vs.wobble_damping, 0.0..=100.0).clamping(egui::SliderClamping::Edits).text("阻尼"));
                        ui.add(egui::Slider::new(&mut vs.wobble_mass, 0.1..=10.0).clamping(egui::SliderClamping::Edits).text("质量"));
                        ui.add(
                            egui::Slider::new(&mut vs.wobble_intensity, 0.0..=1.0).clamping(egui::SliderClamping::Edits).text("形变强度"),
                        );
                        ui.add(
                            egui::Slider::new(&mut vs.wobble_position_epsilon, 0.01..=5.0).clamping(egui::SliderClamping::Edits)
                                .text("位置停止阈值"),
                        );
                        ui.add(
                            egui::Slider::new(&mut vs.wobble_velocity_epsilon, 0.1..=20.0).clamping(egui::SliderClamping::Edits)
                                .text("速度停止阈值"),
                        );
                        });
                    });

                    ui.separator();
                    ui.checkbox(
                        &mut vs.workspace_cube_enabled,
                        "立方体工作区（键盘与触控板）",
                    );
                    ui.add_enabled_ui(vs.workspace_cube_enabled, |ui| {
                        egui::CollapsingHeader::new("立方体的视觉参数").show(ui, |ui| {
                        ui.add(
                            egui::Slider::new(&mut vs.cube_perspective, 0.05..=1.0).clamping(egui::SliderClamping::Edits)
                                .text("透视强度"),
                        );
                        ui.horizontal(|ui| {
                            ui.label("立方体背景色");
                            // egui converts RGBA through HSVA even without an edit.
                            // Commit that round-trip only after a user change.
                            let mut color = vs.cube_background_color;
                            if ui.color_edit_button_rgba_unmultiplied(&mut color).changed() {
                                vs.cube_background_color = color;
                            }
                        });
                        });
                    });

                    ui.separator();
                    ui.checkbox(&mut vs.window_glide_enabled, "3D Glide（以开窗为基准，改动同步对应关窗参数）");
                    ui.add_enabled_ui(vs.window_glide_enabled, |ui| {
                        egui::CollapsingHeader::new("Glide 的视觉参数").show(ui, |ui| {
                        ui.add(
                            egui::Slider::new(&mut vs.glide_tilt_deg, 0.0..=45.0).clamping(egui::SliderClamping::Edits).text("倾斜角度"),
                        );
                        ui.add(egui::Slider::new(&mut vs.glide_depth, 0.0..=1.0).clamping(egui::SliderClamping::Edits).text("纵深"));
                        });
                    });

                    ui.add_space(8.0);
                    if ui.button("恢复三项动效默认值").clicked() {
                        vs.window_wobble_enabled = true;
                        vs.wobble_mesh = 12;
                        vs.wobble_stiffness = 200.0;
                        vs.wobble_damping = 12.0;
                        vs.wobble_mass = 1.0;
                        vs.wobble_intensity = 0.2;
                        vs.wobble_position_epsilon = 0.25;
                        vs.wobble_velocity_epsilon = 2.0;
                        vs.workspace_cube_enabled = true;
                        vs.cube_perspective = 0.65;
                        vs.cube_background_color = [16. / 255., 16. / 255., 24. / 255., 1.0];
                        vs.window_glide_enabled = true;
                        vs.glide_tilt_deg = 12.0;
                        vs.glide_depth = 0.08;
                    }
                });
            if let Some(animations) = self.config_mgr.effective_animations() {
                if animations.window_open.glide != animations.window_close.glide {
                    let open = animations.window_open.glide;
                    let close = animations.window_close.glide;
                    ui.weak(format!("已解析 Glide：开窗{} / 倾斜 {}° / 纵深 {}；关窗{} / 倾斜 {}° / 纵深 {}。只同步您修改的参数，其余独立值保留。",
                        if open.off { "关闭" } else { "开启" }, open.tilt_deg, open.depth,
                        if close.off { "关闭" } else { "开启" }, close.tilt_deg, close.depth));
                }
            }

            ui.add_space(16.0);

            });
            egui::CollapsingHeader::new("背景模糊").show(ui, |ui| self.ui_background_blur(ui));
            egui::CollapsingHeader::new("动态渐变边框").show(ui, |ui| {
            // Clarion special feature: Animated gradient borders
            Frame::none()
                .fill(palette::surface())
                .rounding(Rounding::same(6.0))
                .inner_margin(Margin::same(12.0))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new("动态渐变边框")
                            .color(palette::accent())
                            .strong(),
                    );
                    ui.label(
                        "Clarion 支持给活动窗口应用平滑旋转的着色器渐变边框 (rotate-deg-per-sec)。",
                    );
                    ui.add_space(8.0);
                    if ui.button("插入动态渐变边框示例到配置文件").clicked() {
                        self.switch_tab(ActiveTab::ConfigEditor);
                        if self.active_tab == ActiveTab::ConfigEditor {
                            let current = self.config_mgr.raw_content().to_string();
                            let updated = ConfigManager::upsert_child_block(
                                &current,
                                &["layout"],
                                "border",
                                SNIPPET_ANIMATED_BORDER,
                            );
                            self.config_mgr.set_raw_content(updated);
                            self.set_status("已将动态渐变边框代码插入到 layout 块中！", false);
                        }
                    }
                });

            });
        });
    }

    fn ui_overview_style(&mut self, ui: &mut Ui) {
        ui.group(|ui| {
            ui.strong("工作区概览风格");
            ui.label("保存后，左上角热区与 Mod+O 使用此风格；Expo 和全窗口搜索独立保留。");
            let local = match crate::appearance_controls::read_overview_style(
                self.config_mgr.raw_content(),
            ) {
                Ok(style) => style,
                Err(error) => {
                    ui.colored_label(palette::warning(), error.to_string());
                    return;
                }
            };
            match self.config_mgr.effective_config() {
                Ok(config) => {
                    ui.weak(format!(
                        "草稿有效风格：{}{}",
                        crate::appearance_controls::overview_style_label(config.overview.style),
                        if local.is_none() {
                            "（继承配置或默认值）"
                        } else {
                            ""
                        },
                    ));
                }
                Err(error) => {
                    ui.colored_label(palette::warning(), format!("请先修正配置：{error}"));
                }
            }
            let mut selected = local;
            egui::ComboBox::from_label("概览风格")
                .selected_text(
                    selected.map_or("继承配置", crate::appearance_controls::overview_style_label),
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut selected, None, "继承配置");
                    for style in [
                        clarion_config::OverviewStyle::Stage,
                        clarion_config::OverviewStyle::Strip,
                    ] {
                        ui.selectable_value(
                            &mut selected,
                            Some(style),
                            crate::appearance_controls::overview_style_label(style),
                        );
                    }
                });
            if selected != local {
                match self.choose_overview_style(selected) {
                    Ok(()) => self.set_status("概览风格已更新到草稿，请在底部保存并应用", false),
                    Err(error) => self.set_status(format!("概览风格修改失败：{error}"), true),
                }
            }
            ui.weak("空间舞台：中央卡片、两侧分层预览和底部工作区条，可拖动窗口整理桌面。");
            ui.weak("经典纵向概览：工作区沿纵向排列。选择风格会保留缩放、阴影和动画参数。");
        });
    }

    fn ui_close_effect(&mut self, ui: &mut Ui) {
        ui.group(|ui| {
            ui.strong("窗口关闭效果");
            ui.label("编辑当前文件的关闭效果，保存并应用后生效。");
            let state = match CloseState::read(self.config_mgr.raw_content()) {
                Ok(state) => state,
                Err(error) => {
                    ui.colored_label(palette::warning(), error.to_string());
                    return;
                }
            };
            if state.custom_shader {
                ui.colored_label(palette::warning(), "当前文件保留了自定义关闭着色器。");
            } else if state.selected.is_none() {
                ui.weak("本文件尚未指定关闭效果；包含文件和桌面会话的默认值可能提供效果。");
            }
            ui.weak("选择内置效果会覆盖自定义关闭着色器（含继承）；禁用会保留它。");
            ui.horizontal_wrapped(|ui| {
                for effect in CloseEffect::ALL {
                    if ui
                        .selectable_label(state.selected == Some(effect), effect.label())
                        .clicked()
                    {
                        match self.choose_close_effect(effect) {
                            Ok(()) => self.set_status("关闭效果已更新到草稿，请保存并应用", false),
                            Err(error) => {
                                self.set_status(format!("关闭效果修改失败：{error}"), true)
                            }
                        }
                    }
                }
            });
            ui.weak("保留现有关闭时长、曲线与 Glide 参数。动画总开关关闭时，关闭效果不会播放。");
        });
    }

    fn ui_background_blur(&mut self, ui: &mut Ui) {
        ui.group(|ui| {
            ui.strong("背景模糊");
            ui.label("应用于请求背景模糊的窗口与面板；窗口透明度可在窗口规则中调整。");
            let before = match BlurOverrides::read(self.config_mgr.raw_content()) {
                Ok(overrides) => overrides,
                Err(error) => {
                    ui.colored_label(palette::warning(), error.to_string());
                    return;
                }
            };
            let mut draft = before.clone();
            egui::ComboBox::from_label("模糊开关")
                .selected_text(match draft.enabled {
                    None => "继承配置",
                    Some(true) => "开启",
                    Some(false) => "关闭",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut draft.enabled, None, "继承配置");
                    ui.selectable_value(&mut draft.enabled, Some(true), "开启");
                    ui.selectable_value(&mut draft.enabled, Some(false), "关闭");
                });
            ui.weak("勾选参数后设置本文件的覆盖值；取消勾选会沿用包含文件或 Clarion 默认值。");
            ui.add_enabled_ui(draft.enabled != Some(false), |ui| {
                let defaults = clarion_config::Blur::default();
                let mut custom_passes = draft.passes.is_some();
                if ui.checkbox(&mut custom_passes, "自定义模糊级数").changed() {
                    draft.passes = custom_passes.then_some(defaults.passes);
                }
                if let Some(passes) = &mut draft.passes {
                    ui.add(
                        egui::Slider::new(passes, 1..=31)
                            .clamping(egui::SliderClamping::Edits)
                            .text("级数"),
                    );
                }
                optional_blur_slider(
                    ui,
                    "模糊强度",
                    &mut draft.offset,
                    defaults.offset,
                    0.0..=100.0,
                );
                optional_blur_slider(ui, "噪点", &mut draft.noise, defaults.noise, 0.0..=0.2);
                optional_blur_slider(
                    ui,
                    "饱和度",
                    &mut draft.saturation,
                    defaults.saturation,
                    0.0..=3.0,
                );
            });
            ui.weak("更高的级数增加采样范围与 GPU 工作量。超出滑块范围的已有值会原样保留。");
            if ui.button("恢复本文件的模糊继承").clicked() {
                draft = BlurOverrides::default();
            }
            if draft != before {
                match self.update_blur_overrides(&draft) {
                    Ok(()) => self.set_status("模糊参数已更新到草稿，请保存并应用", false),
                    Err(error) => self.set_status(format!("模糊参数修改失败：{error}"), true),
                }
            }
        });
    }

    fn ui_input_hot_corners(&mut self, ui: &mut Ui) {
        ScrollArea::vertical()
            .id_salt("keyboard_preferences")
            .show(ui, |ui| {
                Self::page_heading(
                    ui,
                    Icon::Input,
                    "键盘",
                    "设置按键重复与键盘输入。修改后保存并应用。",
                );
                ui.add_space(12.);
                let settings = self.config_mgr.visual_settings_mut();
                ui.columns(2, |columns| {
                    for column in columns.iter_mut() {
                        column.spacing_mut().slider_width = column.available_width();
                    }
                    columns[0].label("按键重复");
                    columns[0]
                        .add(
                            egui::Slider::new(&mut settings.keyboard_repeat_rate, 0..=60)
                                .clamping(egui::SliderClamping::Edits)
                                .show_value(false),
                        )
                        .on_hover_text(format!(
                            "每秒 {} 次；0 为关闭按键重复",
                            settings.keyboard_repeat_rate
                        ));
                    columns[0].horizontal(|ui| {
                        ui.weak("关／慢");
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.weak("快");
                        });
                    });
                    columns[1].label("重复前延迟");
                    columns[1]
                        .add(
                            egui::Slider::new(
                                &mut settings.keyboard_repeat_delay,
                                std::ops::RangeInclusive::new(1200, 100),
                            )
                            .clamping(egui::SliderClamping::Edits)
                            .show_value(false),
                        )
                        .on_hover_text(format!("{} 毫秒", settings.keyboard_repeat_delay));
                    columns[1].horizontal(|ui| {
                        ui.weak("长");
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.weak("短");
                        });
                    });
                });
                ui.add_space(28.);
                ui.separator();
                ui.add_space(14.);
                ui.checkbox(&mut settings.keyboard_numlock, "启动时自动启用数字键盘");
                ui.add_space(18.);
                ui.horizontal_wrapped(|ui| {
                    ui.weak("键盘布局与快捷键");
                    if ui.button("更多键盘设置…").clicked() {
                        self.switch_tab(ActiveTab::FullConfig);
                    }
                });
            });
    }

    fn ui_trackpad(&mut self, ui: &mut Ui) {
        Self::segmented_tabs(
            ui,
            &mut self.trackpad_section,
            &["光标与点按", "滚动缩放", "更多手势"],
        );
        ui.add_space(28.);
        ScrollArea::vertical()
            .id_salt("trackpad_preferences")
            .show(ui, |ui| match self.trackpad_section {
                0 => {
                    ui.checkbox(
                        &mut self.config_mgr.visual_settings_mut().touchpad_tap,
                        "轻点来点按",
                    );
                    ui.weak("用单指轻点触控板即可点按。");
                    ui.add_space(28.);
                    ui.label("跟踪速度");
                    ui.add(
                        egui::Slider::new(
                            &mut self.config_mgr.visual_settings_mut().touchpad_accel_speed,
                            -1.0..=1.0,
                        )
                        .clamping(egui::SliderClamping::Edits)
                        .show_value(false),
                    );
                    ui.horizontal(|ui| {
                        ui.weak("慢");
                        ui.add_space(130.);
                        ui.weak("快");
                    });
                }
                1 => {
                    ui.checkbox(
                        &mut self
                            .config_mgr
                            .visual_settings_mut()
                            .touchpad_natural_scroll,
                        "滚动方向：自然",
                    );
                    ui.weak("内容随手指移动的方向滚动。");
                }
                _ => {
                    ui.label("工作区与概览手势");
                    ui.weak("手势绑定、加速度与滚动速度可在更多设置中调整。");
                    ui.add_space(12.);
                    if ui.button("更多触控板设置…").clicked() {
                        self.switch_tab(ActiveTab::FullConfig);
                    }
                }
            });
    }

    fn segmented_tabs(ui: &mut Ui, selected: &mut usize, labels: &[&str]) {
        ui.horizontal(|ui| {
            let width = labels
                .iter()
                .map(|label| {
                    ui.fonts(|fonts| {
                        fonts.layout_no_wrap(
                            (*label).into(),
                            egui::FontId::proportional(13.),
                            palette::text(),
                        )
                    })
                    .size()
                    .x + 24.
                })
                .sum::<f32>();
            ui.add_space(((ui.available_width() - width) / 2.).max(0.));
            ui.spacing_mut().item_spacing.x = 0.;
            for (index, label) in labels.iter().enumerate() {
                let active = *selected == index;
                let rounding = Rounding {
                    nw: if index == 0 { 4. } else { 0. },
                    sw: if index == 0 { 4. } else { 0. },
                    ne: if index + 1 == labels.len() { 4. } else { 0. },
                    se: if index + 1 == labels.len() { 4. } else { 0. },
                };
                let response = ui.add(
                    Button::new(RichText::new(*label).color(if active {
                        palette::on_accent()
                    } else {
                        palette::text()
                    }))
                    .fill(if active {
                        palette::accent()
                    } else {
                        palette::surface()
                    })
                    .stroke(Stroke::new(
                        1.0_f32,
                        if active {
                            palette::accent()
                        } else {
                            palette::border()
                        },
                    ))
                    .rounding(rounding)
                    .min_size(egui::vec2(0., 22.)),
                );
                if response.clicked() {
                    *selected = index;
                }
            }
        });
    }

    fn resize_handle(ctx: &egui::Context) {
        if ctx.input(|input| input.viewport().maximized.unwrap_or(false)) {
            return;
        }
        let size = egui::vec2(12., 12.);
        egui::Area::new(egui::Id::new("preferences-resize"))
            .order(egui::Order::Foreground)
            .fixed_pos(ctx.screen_rect().right_bottom() - size)
            .show(ctx, |ui| {
                ui.spacing_mut().window_margin = Margin::same(0.);
                let (_, response) = ui.allocate_exact_size(size, Sense::drag());
                if response.drag_started() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(
                        egui::ResizeDirection::SouthEast,
                    ));
                }
                response.on_hover_cursor(egui::CursorIcon::ResizeSouthEast);
            });
    }

    fn hot_corners_dialog(&mut self, ctx: &egui::Context, _blocked: bool) {
        if !self.show_hot_corners {
            return;
        }
        let unavailable = self.config_mgr.preview_pending()
            || self.confirm_close
            || self.confirm_reload
            || self.command_palette.is_open();
        if unavailable {
            return;
        }
        let popup_was_open = ctx.memory(|memory| memory.any_popup_open());
        let width = (ctx.screen_rect().width() - 72.).clamp(280., 605.);
        let mut done = false;
        egui::Window::new("触发角")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .default_width(width)
            .max_width(width)
            .show(ctx, |ui| {
                ui.set_width(width);
                if unavailable {
                    ui.disable();
                }
                ui.label("将指针移到屏幕角落时执行以下操作：");
                let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 132.), Sense::hover());
                let control_width = (width * 0.32).min(170.);
                let center_width = (width - control_width * 2. - 24.).clamp(60., 115.);
                let monitor =
                    egui::Rect::from_center_size(rect.center(), egui::vec2(center_width, 73.));
                ui.painter().rect(
                    monitor,
                    4.,
                    palette::surface_hover(),
                    Stroke::new(1.0_f32, palette::border()),
                );
                ui.painter().text(
                    monitor.center(),
                    Align2::CENTER_CENTER,
                    "屏幕",
                    egui::FontId::proportional(13.),
                    palette::muted(),
                );
                let settings = self.config_mgr.visual_settings_mut();
                for (label, value, action, right, bottom) in [
                    (
                        "左上角",
                        &mut settings.hot_corner_top_left,
                        "工作区概览",
                        false,
                        false,
                    ),
                    (
                        "右上角",
                        &mut settings.hot_corner_top_right,
                        "Expo 全景工作区",
                        true,
                        false,
                    ),
                    (
                        "左下角",
                        &mut settings.hot_corner_bottom_left,
                        "系统偏好设置",
                        false,
                        true,
                    ),
                ] {
                    let position = egui::pos2(
                        if right {
                            rect.right() - control_width
                        } else {
                            rect.left()
                        },
                        rect.top() + if bottom { 82. } else { 26. },
                    );
                    ui.allocate_new_ui(
                        egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                            position,
                            egui::vec2(control_width, 26.),
                        )),
                        |ui| {
                            egui::ComboBox::from_id_salt(("hot-corner", label))
                                .width(control_width)
                                .selected_text(if *value { action } else { "—" })
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(value, false, "—");
                                    ui.selectable_value(value, true, action);
                                })
                                .response
                                .on_hover_text(label);
                        },
                    );
                }
                ui.allocate_new_ui(
                    egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                        egui::pos2(rect.right() - control_width, rect.top() + 82.),
                        egui::vec2(control_width, 26.),
                    )),
                    |ui| {
                        ui.add_enabled(
                            false,
                            Button::new("—").min_size(egui::vec2(control_width, 22.)),
                        )
                        .on_hover_text("右下角暂未绑定操作");
                    },
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    done = ui
                        .add(
                            Button::new(RichText::new("好").color(palette::on_accent()))
                                .fill(palette::accent())
                                .min_size(egui::vec2(80., 22.)),
                        )
                        .clicked();
                });
            });
        let keyboard_done = !unavailable
            && !popup_was_open
            && ctx.input_mut(|input| {
                input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)
                    || input.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
            });
        if done || keyboard_done {
            self.show_hot_corners = false;
        }
    }

    fn ui_windows(&mut self, ui: &mut Ui) {
        let content_size = egui::vec2(ui.available_width(), (ui.available_height() - 38.).max(80.));
        ui.allocate_ui_with_layout(content_size, Layout::top_down(Align::LEFT), |ui| {
            ScrollArea::vertical().id_salt("mission_control_page")
                .auto_shrink([false, false]).max_height(content_size.y)
                .show(ui, |ui| {
                ui.set_max_height(content_size.y);
            if ui.button("刷新 / 重新连接").clicked() {
                self.refresh_ipc_data();
            }
            if let Some(error) = &self.connection_error {
                ui.heading("窗口与工作区");
                ui.label("未连接到 Clarion，正在每 3 秒自动重连。");
                ui.label(error);
                return;
            }
            ui.horizontal_wrapped(|ui| {
                if ui.button("全窗口展开 / 搜索").clicked() {
                    if let Err(e) = self
                        .ipc
                        .perform_action(clarion_ipc::Action::ToggleWindowSpread {})
                    {
                        self.set_status(e.to_string(), true);
                    }
                }
                let supported = self.management.is_some();
                if ui
                    .add_enabled(supported, Button::new("保存会话布局"))
                    .clicked()
                {
                    if let Err(e) = self.ipc.save_session() {
                        self.set_status(e.to_string(), true);
                    }
                }
                if ui
                    .add_enabled(supported, Button::new("恢复会话布局"))
                    .clicked()
                {
                    if let Err(e) = self.ipc.restore_session() {
                        self.set_status(e.to_string(), true);
                    }
                }
            });
            ui.label("恢复工作区、窗口尺寸、平铺关系与分组；请先打开应用，再点击恢复。支持撤销。");
            if let Some(action) = self.window_manager.management_ui(
                ui,
                self.management.as_ref(),
                &self.cached_workspaces,
            ) {
                if let Err(e) = self.ipc.perform_action(action) {
                    self.set_status(e.to_string(), true);
                }
            }
            if let Some(action) =
                self.window_manager
                    .show(ui, &self.cached_windows, &self.cached_workspaces)
            {
                match self.ipc.perform_action(action) {
                    Ok(()) => self.set_status("操作请求已发送", false),
                    Err(error) => self.set_status(format!("操作失败: {error}"), true),
                }
                self.refresh_ipc_data();
            }
            });
        });
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            if ui.button("触发角…").clicked() {
                self.show_hot_corners = true;
            }
            if ui.button("概览设置…").clicked() {
                self.switch_tab(ActiveTab::Appearance);
            }
        });
    }

    fn ui_outputs(&mut self, ui: &mut Ui) {
        ScrollArea::vertical().show(ui, |ui| {
            if self.cached_outputs.len() > 1 {
                Self::segmented_tabs(ui, &mut self.output_section, &["显示器", "排列"]);
                ui.add_space(20.);
            } else {
                self.output_section = 0;
            }
            if !self
                .cached_outputs
                .iter()
                .any(|output| Some(&output.name) == self.selected_output.as_ref())
            {
                self.selected_output = self
                    .cached_outputs
                    .first()
                    .map(|output| output.name.clone());
            }
            if self.cached_outputs.len() > 1 {
                ui.horizontal_wrapped(|ui| {
                    ui.label("显示器：");
                    for output in &self.cached_outputs {
                        ui.selectable_value(
                            &mut self.selected_output,
                            Some(output.name.clone()),
                            &output.name,
                        );
                    }
                });
                ui.add_space(12.);
            }
            if self.output_section == 1 {
                self.ui_output_arrangement(ui);
                ui.add_space(18.);
            }

            if self.cached_outputs.is_empty() {
                ui.label("暂未检测到活动的输出设备或未连接 Clarion IPC。");
            } else {
                for i in 0..self.cached_outputs.len() {
                    if Some(&self.cached_outputs[i].name) != self.selected_output.as_ref() {
                        continue;
                    }
                    let name = self.cached_outputs[i].name.clone();
                    let make = self.cached_outputs[i].make.clone();
                    let model = self.cached_outputs[i].model.clone();
                    let logical = self.cached_outputs[i].logical;
                    let vrr_supported = self.cached_outputs[i].vrr_supported;
                    let vrr_enabled = self.cached_outputs[i].vrr_enabled;
                    let hdr_active = self.cached_outputs[i].hdr_active;
                    let current_mode = self.cached_outputs[i]
                        .current_mode
                        .and_then(|idx| self.cached_outputs[i].modes.get(idx).cloned());
                    Frame::none()
                        .fill(palette::background())
                        .rounding(Rounding::same(4.0))
                        .inner_margin(Margin::same(12.0))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal_wrapped(|ui| {
                                ui.label(
                                    RichText::new(&name)
                                        .size(16.0)
                                        .color(palette::accent())
                                        .strong(),
                                );
                                ui.label(format!("(制造商: {}, 型号: {})", make, model));
                            });
                            ui.add_space(6.0);
                            if let Some(logical) = &logical {
                                ui.label(format!(
                                    "逻辑尺寸: {}x{}，缩放比例: {:.2}x，位置: ({}, {})",
                                    logical.width,
                                    logical.height,
                                    logical.scale,
                                    logical.x,
                                    logical.y
                                ));
                            }
                            if let Some(mode) = &current_mode {
                                ui.label(format!(
                                    "当前物理分辨率: {}x{} @ {:.2} Hz",
                                    mode.width,
                                    mode.height,
                                    mode.refresh_rate as f32 / 1000.0
                                ));
                            }
                            ui.label(format!(
                                "可变刷新率（VRR）：{}",
                                if vrr_enabled {
                                    "已启用"
                                } else if vrr_supported {
                                    "支持，当前未启用"
                                } else {
                                    "此显示器不支持"
                                }
                            ));
                            ui.label(format!(
                                "HDR：{}",
                                if hdr_active {
                                    "已启用（HDR10 / BT.2020）"
                                } else {
                                    "未启用"
                                }
                            ));
                            if icons::button(ui, Icon::Settings, "配置此显示器").clicked() {
                                match self
                                    .full_config
                                    .select_output(self.config_mgr.raw_content(), &name)
                                {
                                    Ok(next) => {
                                        if let Some(next) = next {
                                            self.config_mgr.set_raw_content(next);
                                        }
                                        self.switch_tab(ActiveTab::FullConfig);
                                    }
                                    Err(error) => {
                                        self.status_message = Some((
                                            error.to_string(),
                                            true,
                                            std::time::Instant::now(),
                                        ));
                                    }
                                }
                            }
                        });
                    ui.add_space(10.0);
                }
            }

            ui.add_space(14.0);
            if icons::button(ui, Icon::Refresh, "刷新显示器状态").clicked() {
                self.refresh_ipc_data();
                if let Some(error) = &self.connection_error {
                    self.set_status(format!("暂时无法连接桌面：{error}"), true);
                } else {
                    self.set_status("已刷新显示器信息", false);
                }
            }
        });
    }

    fn ui_output_arrangement(&mut self, ui: &mut Ui) {
        ui.label("下图显示已连接屏幕的实际位置。选择屏幕后可配置它的分辨率与位置。");
        ui.add_space(10.);
        let logical: Vec<_> = self
            .cached_outputs
            .iter()
            .filter_map(|output| {
                output
                    .logical
                    .as_ref()
                    .map(|logical| (output.name.clone(), *logical))
            })
            .collect();
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 180.), Sense::hover());
        ui.painter().rect(
            rect,
            4.,
            palette::surface_hover(),
            Stroke::new(1.0_f32, palette::border()),
        );
        if logical.is_empty() {
            return;
        }
        let bounds = logical
            .iter()
            .fold(egui::Rect::NOTHING, |bounds, (_, output)| {
                bounds.union(egui::Rect::from_min_size(
                    egui::pos2(output.x as f32, output.y as f32),
                    egui::vec2(output.width as f32, output.height as f32),
                ))
            });
        if bounds.width() <= 0. || bounds.height() <= 0. {
            return;
        }
        let scale =
            ((rect.width() - 32.) / bounds.width()).min((rect.height() - 32.) / bounds.height());
        for (name, output) in logical {
            let top_left = rect.center() - bounds.size() * scale / 2.
                + (egui::vec2(output.x as f32, output.y as f32) - bounds.min.to_vec2()) * scale;
            let monitor = egui::Rect::from_min_size(
                top_left,
                egui::vec2(output.width as f32, output.height as f32) * scale,
            );
            let selected = self.selected_output.as_ref() == Some(&name);
            let response = ui.interact(
                monitor,
                ui.id().with(("arranged-output", &name)),
                Sense::click(),
            );
            ui.painter().rect(
                monitor.shrink(1.),
                2.,
                if selected {
                    palette::accent()
                } else {
                    palette::muted()
                },
                Stroke::new(1.0_f32, palette::border()),
            );
            ui.painter().text(
                monitor.center(),
                Align2::CENTER_CENTER,
                &name,
                egui::FontId::proportional(12.),
                palette::on_accent(),
            );
            if response.clicked() {
                self.selected_output = Some(name);
            }
        }
    }

    fn ui_full_config(&mut self, ui: &mut Ui) {
        let height = ui.available_height();
        ScrollArea::vertical()
            .id_salt("full_config_page")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_max_height(height);
                Self::page_heading(
                    ui,
                    Icon::Settings,
                    "全部设置",
                    "按分类查找参数。未在当前文件设置的选项继续使用默认值或导入文件的值。",
                );
                egui::CollapsingHeader::new("当前配置文件与导入文件").show(ui, |ui| {
                    ui.add(
                        egui::Label::new(self.config_mgr.config_path().display().to_string())
                            .wrap(),
                    );
                    ui.horizontal_wrapped(|ui| {
                        if self.config_mgr.config_path() != self.config_mgr.root_path()
                            && icons::button(ui, Icon::Undo, "返回主配置").clicked()
                        {
                            let path = self.config_mgr.root_path().to_owned();
                            if let Err(error) = self.open_config_file(&path) {
                                self.set_status(error.to_string(), true);
                            }
                        }
                        egui::ComboBox::from_id_salt("include-file")
                            .selected_text("编辑导入文件…")
                            .show_ui(ui, |ui| {
                                for path in self.config_mgr.included_paths() {
                                    if ui.button(path.display().to_string()).clicked() {
                                        if let Err(error) = self.open_config_file(&path) {
                                            self.set_status(error.to_string(), true);
                                        }
                                    }
                                }
                            });
                    });
                    ui.weak("导入文件独立保存。渲染后端、自启动和环境变量在重新登录后生效。");
                });
                match self.config_mgr.validation_status() {
                    ValidationStatus::Valid => {
                        ui.colored_label(palette::success(), "配置校验通过");
                    }
                    ValidationStatus::Error(error) => {
                        egui::CollapsingHeader::new("配置校验失败（无法保存）").show(ui, |ui| {
                            ui.colored_label(palette::error(), error);
                        });
                    }
                    ValidationStatus::NotChecked => {
                        ui.label("等待校验…");
                    }
                }
                if let Some(next) = self.full_config.ui(ui, self.config_mgr.raw_content()) {
                    self.config_mgr.set_raw_content(next);
                }
            });
    }

    fn ui_config_editor(&mut self, ui: &mut Ui) {
        ui.vertical(|ui| {
            Self::page_heading(
                ui,
                Icon::Code,
                "配置文件",
                "直接编辑 KDL 源码。日常设置可从“全部显示”进入可视化面板。",
            );
            ui.horizontal_wrapped(|ui| {
                if icons::button(ui, Icon::Refresh, "从磁盘重新载入").clicked() {
                    if self.config_mgr.is_dirty() || self.full_config.has_pending() {
                        self.confirm_reload = true;
                    } else {
                        match self.reload_config_from_disk() {
                            Ok(()) => self.set_status("已从磁盘载入配置", false),
                            Err(error) => self.set_status(error.to_string(), true),
                        }
                    }
                }
                if icons::button(ui, Icon::Undo, "还原最近备份到草稿").clicked() {
                    match self.config_mgr.restore_backup() {
                        Ok(()) => self.set_status("备份已载入草稿，确认后再保存并应用", false),
                        Err(error) => self.set_status(error.to_string(), true),
                    }
                }
            });
            ui.add_space(6.0);

            // Validation Banner
            match self.config_mgr.validation_status() {
                ValidationStatus::Valid => {
                    Frame::none()
                        .fill(palette::success_soft())
                        .rounding(Rounding::same(6.0))
                        .inner_margin(Margin::symmetric(10.0, 6.0))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new("配置检查通过")
                                        .color(palette::success())
                                        .size(12.5),
                                );
                            });
                        });
                }
                ValidationStatus::Error(err) => {
                    Frame::none()
                        .fill(palette::error_soft())
                        .stroke(Stroke::new(1.0_f32, palette::error()))
                        .rounding(Rounding::same(6.0))
                        .inner_margin(Margin::symmetric(10.0, 6.0))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new("配置检查未通过：")
                                        .color(palette::error())
                                        .strong()
                                        .size(12.5),
                                );
                                ui.label(
                                    RichText::new(err)
                                        .color(palette::text())
                                        .size(12.0)
                                        .monospace(),
                                );
                            });
                        });
                }
                ValidationStatus::NotChecked => {
                    ui.label(
                        RichText::new("正在等待输入完成后校验…")
                            .color(palette::muted())
                            .size(12.5),
                    );
                }
            }

            ui.add_space(6.0);

            // Snippet Insert Bar
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new("插入配置示例：")
                        .size(12.0)
                        .color(palette::muted()),
                );
                if ui.button("+ 热区定义").clicked() {
                    let current = self.config_mgr.raw_content().to_string();
                    self.config_mgr
                        .set_raw_content(ConfigManager::upsert_child_block(
                            &current,
                            &["gestures"],
                            "hot-corners",
                            SNIPPET_HOT_CORNERS,
                        ));
                }
                if ui.button("+ 快捷键绑定").clicked() {
                    let current = self.config_mgr.raw_content().to_string();
                    self.config_mgr
                        .set_raw_content(ConfigManager::insert_snippet_in_block(
                            &current,
                            "binds",
                            SNIPPET_KEYBIND,
                        ));
                }
                if ui.button("+ 窗口规则").clicked() {
                    let mut text = self.config_mgr.raw_content().to_string();
                    text.push('\n');
                    text.push_str(SNIPPET_WINDOW_RULE);
                    self.config_mgr.set_raw_content(text);
                }
                if ui.button("+ 动态旋转渐变边框").clicked() {
                    let current = self.config_mgr.raw_content().to_string();
                    self.config_mgr
                        .set_raw_content(ConfigManager::upsert_child_block(
                            &current,
                            &["layout"],
                            "border",
                            SNIPPET_ANIMATED_BORDER,
                        ));
                }
            });

            ui.add_space(8.0);

            // Text editor with KDL syntax highlighter
            let mut text = self.config_mgr.raw_content().to_string();
            let mut layouter = |ui: &egui::Ui, text: &str, wrap_width: f32| {
                let mut job = highlight_kdl(text);
                job.wrap.max_width = wrap_width;
                ui.fonts(|f| f.layout_job(job))
            };

            let text_edit = TextEdit::multiline(&mut text)
                .id(egui::Id::new("config-source-editor"))
                .code_editor()
                .font(egui::FontId::monospace(13.0))
                .desired_width(f32::INFINITY)
                .desired_rows(32)
                .layouter(&mut layouter);

            let resp = ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| ui.add(text_edit));

            if resp.inner.changed() {
                self.config_mgr.set_raw_content(text);
            }
        });
    }
}

fn optional_blur_slider(
    ui: &mut Ui,
    label: &str,
    value: &mut Option<f64>,
    default: f64,
    range: std::ops::RangeInclusive<f64>,
) {
    let mut custom = value.is_some();
    if ui.checkbox(&mut custom, format!("自定义{label}")).changed() {
        *value = custom.then_some(default);
    }
    if let Some(value) = value {
        ui.add(
            egui::Slider::new(value, range)
                .clamping(egui::SliderClamping::Edits)
                .text(label),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn app() -> ClarionSettingsApp {
        ClarionSettingsApp {
            ipc: IpcClient::disconnected_for_test(),
            config_mgr: ConfigManager::empty(),
            active_tab: ActiveTab::FullConfig,
            back_stack: Vec::new(),
            forward_stack: Vec::new(),
            preferences_search: String::new(),
            preferences_ime_composing: false,
            preferences_search_selected: 0,
            show_hot_corners: false,
            trackpad_section: 0,
            output_section: 0,
            selected_output: None,
            deferred_pointer_events: Vec::new(),
            status_message: None,
            cached_outputs: vec![],
            frame_timings: None,
            cached_windows: vec![],
            window_manager: Default::default(),
            management: None,
            connection_error: None,
            cached_workspaces: vec![],
            clarion_version: String::new(),
            last_ipc_poll: std::time::Instant::now(),
            toggle_flag: None,
            focus_pending: false,
            initial_window_geometry: false,
            confirm_close: false,
            confirm_reload: false,
            allow_close: false,
            close_after_preview: false,
            full_config: Default::default(),
            command_palette: Default::default(),
            pending_window_focus: None,
            desktop_theme: Default::default(),
        }
    }
    fn close(app: &mut ClarionSettingsApp) -> egui::FullOutput {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .events
            .push(egui::ViewportEvent::Close);
        ctx.run(input, |ctx| app.close_guard(ctx))
    }
    fn cancelled(output: &egui::FullOutput) -> bool {
        output.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .iter()
            .any(|command| matches!(command, egui::ViewportCommand::CancelClose))
    }

    fn command_key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    fn command_frame(app: &mut ClarionSettingsApp, ctx: &egui::Context, events: Vec<egui::Event>) {
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(980.0, 680.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| app.update_ui(ctx),
        );
    }

    fn command_window(id: u64) -> Window {
        serde_json::from_value(serde_json::json!({
            "id": id, "title": "Draft notes", "app_id": "org.Editor", "pid": null,
            "workspace_id": null, "is_focused": false, "is_floating": false,
            "is_urgent": false, "focus_timestamp": null,
            "layout": {"pos_in_scrolling_layout": null, "tile_size": [800, 600],
                "window_size": [800, 600], "tile_pos_in_workspace_view": null,
                "window_offset_in_tile": [0, 0]}
        }))
        .unwrap()
    }

    #[test]
    fn global_save_shortcut_flushes_form_and_preserves_comments() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.kdl");
        let source = "// keep this comment\nlayout { gaps 8; }\n";
        std::fs::write(&path, source).unwrap();
        let mut app = app();
        app.config_mgr = ConfigManager::from_path(&path).unwrap();
        app.ipc = IpcClient::scripted_for_test(|_| Ok(clarion_ipc::Response::Handled));
        app.full_config = crate::full_config::FullConfig::with_pending_argument_for_test(
            source,
            vec![0, 0],
            "24",
        );
        let ctx = egui::Context::default();
        command_frame(
            &mut app,
            &ctx,
            vec![command_key(egui::Key::S, egui::Modifiers::CTRL)],
        );
        let saved = std::fs::read_to_string(&path).unwrap();
        assert_eq!(saved, "// keep this comment\nlayout { gaps 24; }\n");
        assert_eq!(
            std::fs::read_to_string(path.with_extension("kdl.bak")).unwrap(),
            source
        );
        assert!(!app.config_mgr.is_dirty());
        assert!(!app.full_config.has_pending());
    }

    #[test]
    fn global_save_includes_text_and_ime_commits_from_the_same_frame() {
        for ime in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.kdl");
            let source = "layout { gaps 8; }\n";
            std::fs::write(&path, source).unwrap();
            let mut app = app();
            app.config_mgr = ConfigManager::from_path(&path).unwrap();
            app.ipc = IpcClient::scripted_for_test(|_| Ok(clarion_ipc::Response::Handled));
            app.active_tab = ActiveTab::ConfigEditor;
            let ctx = egui::Context::default();
            command_frame(&mut app, &ctx, vec![]);
            let id = egui::Id::new("config-source-editor");
            ctx.memory_mut(|memory| memory.request_focus(id));
            let mut state = egui::text_edit::TextEditState::load(&ctx, id).unwrap();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(
                    egui::text::CCursor::new(source.find('8').unwrap() + 1),
                )));
            state.store(&ctx, id);
            let mut events = if ime {
                vec![
                    egui::Event::Ime(egui::ImeEvent::Enabled),
                    egui::Event::Ime(egui::ImeEvent::Commit("0".into())),
                ]
            } else {
                vec![egui::Event::Text("0".into())]
            };
            events.push(command_key(egui::Key::S, egui::Modifiers::CTRL));
            command_frame(&mut app, &ctx, events);
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                "layout { gaps 80; }\n",
                "IME: {ime}"
            );
            assert_eq!(app.config_mgr.raw_content(), "layout { gaps 80; }\n");
            assert!(!app.config_mgr.is_dirty());
        }
    }

    #[test]
    fn global_save_shortcut_preserves_invalid_form_and_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.kdl");
        let source = "layout { gaps 8; }\n";
        std::fs::write(&path, source).unwrap();
        let mut app = app();
        app.config_mgr = ConfigManager::from_path(&path).unwrap();
        app.full_config = crate::full_config::FullConfig::with_pending_argument_for_test(
            source,
            vec![0, 0],
            "invalid number",
        );
        command_frame(
            &mut app,
            &egui::Context::default(),
            vec![command_key(egui::Key::S, egui::Modifiers::CTRL)],
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), source);
        assert_eq!(app.config_mgr.raw_content(), source);
        assert!(app.full_config.has_pending());
        assert!(app.status_message.as_ref().unwrap().1);
        assert!(!path.with_extension("kdl.bak").exists());
    }

    #[test]
    fn global_save_shortcut_waits_for_confirmation_palette_and_display_preview() {
        for state in 0..4 {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.kdl");
            let source = "output \"test\" { scale 1; }\nlayout { gaps 8; }\n";
            std::fs::write(&path, source).unwrap();
            let mut app = app();
            app.config_mgr = ConfigManager::from_path(&path).unwrap();
            app.ipc = IpcClient::scripted_for_test(|request| {
                Ok(match request {
                    clarion_ipc::Request::PreviewOutputConfig { .. } => {
                        clarion_ipc::Response::OutputPreview {
                            id: 42,
                            timeout_ms: 15_000,
                        }
                    }
                    _ => clarion_ipc::Response::Handled,
                })
            });
            app.config_mgr
                .set_raw_content(source.replace("gaps 8", "gaps 24"));
            match state {
                0 => app.confirm_close = true,
                1 => app.confirm_reload = true,
                2 => app.open_command_palette(),
                _ => {
                    app.config_mgr.set_raw_content(
                        app.config_mgr.raw_content().replace("scale 1", "scale 2"),
                    );
                    app.config_mgr.save_and_reload(&app.ipc).unwrap();
                }
            }
            let draft = app.config_mgr.raw_content().to_owned();
            command_frame(
                &mut app,
                &egui::Context::default(),
                vec![command_key(egui::Key::S, egui::Modifiers::CTRL)],
            );
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                source,
                "state {state}"
            );
            assert_eq!(app.config_mgr.raw_content(), draft, "state {state}");
            assert!(app.config_mgr.is_dirty());
            assert!(!path.with_extension("kdl.bak").exists());
        }
    }

    #[test]
    fn browsing_layout_keeps_existing_large_gap_values() {
        let source = "// custom spacing\nlayout { gaps 120.5; }\n";
        let mut app = app();
        app.config_mgr.set_raw_content(source.into());
        app.active_tab = ActiveTab::Layout;
        let ctx = egui::Context::default();
        for _ in 0..3 {
            command_frame(&mut app, &ctx, vec![]);
        }
        assert_eq!(app.config_mgr.visual_settings().gaps, 120.5);
        assert!(!app.config_mgr.visual_is_dirty());
        assert_eq!(app.config_mgr.raw_content(), source);
    }

    #[test]
    fn command_palette_headless_navigation_flushes_and_preserves_visual_draft() {
        let mut app = app();
        app.config_mgr.set_raw_content(
            "// keep this note\nlayout { gaps 13; border { width 2; }; }\n".into(),
        );
        app.config_mgr.visual_settings_mut().border_width = 5.25;
        let ctx = egui::Context::default();
        command_frame(
            &mut app,
            &ctx,
            vec![command_key(egui::Key::K, egui::Modifiers::CTRL)],
        );
        assert!(app.command_palette.is_open());
        command_frame(&mut app, &ctx, vec![egui::Event::Text("appearance".into())]);
        command_frame(
            &mut app,
            &ctx,
            vec![command_key(egui::Key::Enter, egui::Modifiers::NONE)],
        );
        assert_eq!(app.active_tab, ActiveTab::Appearance);
        assert!(!app.command_palette.is_open());
        assert!(app.config_mgr.raw_content().contains("// keep this note"));
        let config = clarion_config::Config::parse_mem(app.config_mgr.raw_content()).unwrap();
        assert_eq!(config.layout.border.width, 5.25);
        assert_eq!(config.layout.gaps, 13.0);
        assert!(app.config_mgr.is_dirty());
    }

    #[test]
    fn command_palette_invalid_form_blocks_navigation_and_keeps_pending_input() {
        let source = "layout { gaps 8; }\n";
        let mut app = app();
        app.config_mgr.set_raw_content(source.into());
        app.full_config = crate::full_config::FullConfig::with_pending_argument_for_test(
            source,
            vec![0, 0],
            "bad number",
        );
        app.open_command_palette();
        assert!(app
            .execute_command(Command::Navigate(ActiveTab::Appearance))
            .is_err());
        assert_eq!(app.active_tab, ActiveTab::FullConfig);
        assert_eq!(app.config_mgr.raw_content(), source);
        assert!(app.full_config.has_pending());
        assert!(app.command_palette.is_open());
    }

    #[test]
    fn command_palette_offline_and_confirmation_block_live_actions() {
        let mut app = app();
        app.connection_error = Some("offline".into());
        assert!(app
            .execute_command(Command::Overview)
            .unwrap_err()
            .to_string()
            .contains("未连接"));
        app.execute_command(Command::Navigate(ActiveTab::Windows))
            .unwrap();
        app.execute_command(Command::Refresh).unwrap();
        app.confirm_reload = true;
        app.open_command_palette();
        assert!(!app.command_palette.is_open());
        assert!(app
            .execute_command(Command::Navigate(ActiveTab::Dashboard))
            .is_err());
        assert_eq!(app.active_tab, ActiveTab::Windows);
    }

    #[test]
    fn command_palette_focus_requests_fresh_windows_without_flushing_draft() {
        let (seen, requests) = std::sync::mpsc::channel();
        let mut app = app();
        app.ipc = IpcClient::scripted_for_test(move |request| {
            seen.send(request).unwrap();
            Ok(clarion_ipc::Response::Windows(vec![command_window(42)]))
        });
        app.cached_windows = vec![command_window(42)];
        app.config_mgr
            .set_raw_content("layout { border { width 2; }; }\n".into());
        app.config_mgr.visual_settings_mut().border_width = 7.0;
        let raw = app.config_mgr.raw_content().to_owned();
        app.focus_pending = true;
        app.execute_command(Command::FocusWindow(42)).unwrap();
        assert!(matches!(
            requests.recv_timeout(Duration::from_secs(1)).unwrap(),
            clarion_ipc::Request::Windows
        ));
        let ctx = egui::Context::default();
        for _ in 0..100 {
            app.poll_window_focus(&ctx);
            if app.pending_window_focus.is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(app.pending_window_focus.is_none());
        assert!(app
            .status_message
            .as_ref()
            .unwrap()
            .0
            .contains("已请求聚焦"));
        assert_eq!(app.config_mgr.raw_content(), raw);
        assert!(app.config_mgr.visual_is_dirty());
        assert!(!app.focus_pending);
    }

    #[test]
    fn command_palette_keeps_search_open_during_verification_and_after_failure() {
        for succeeds in [false, true] {
            let (confirm, gate) = std::sync::mpsc::channel();
            let mut app = app();
            app.ipc = IpcClient::scripted_for_test(move |request| {
                if matches!(request, clarion_ipc::Request::Windows) {
                    let _ = gate.recv_timeout(Duration::from_secs(1));
                    Ok(clarion_ipc::Response::Windows(if succeeds {
                        vec![command_window(42)]
                    } else {
                        vec![]
                    }))
                } else {
                    Ok(clarion_ipc::Response::Handled)
                }
            });
            app.cached_windows = vec![command_window(42)];
            app.open_command_palette();
            let ctx = egui::Context::default();
            command_frame(&mut app, &ctx, vec![]);
            command_frame(
                &mut app,
                &ctx,
                vec![egui::Event::Text("Draft notes".into())],
            );
            command_frame(
                &mut app,
                &ctx,
                vec![command_key(egui::Key::Enter, egui::Modifiers::NONE)],
            );
            assert!(app.pending_window_focus.is_some());
            assert!(
                app.command_palette.is_open(),
                "the search should stay visible during async verification"
            );
            confirm.send(()).unwrap();
            for _ in 0..100 {
                command_frame(&mut app, &ctx, vec![]);
                if app.pending_window_focus.is_none() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(app.pending_window_focus.is_none());
            assert_eq!(app.command_palette.is_open(), !succeeds);
            if succeeds {
                assert!(app
                    .status_message
                    .as_ref()
                    .unwrap()
                    .0
                    .contains("已请求聚焦"));
            } else {
                assert!(app.status_message.as_ref().unwrap().0.contains("已关闭"));
                assert!(app.cached_windows.is_empty());
                assert_eq!(
                    ctx.memory(|memory| memory.focused()),
                    Some(egui::Id::new("command_palette_search"))
                );
            }
        }
    }

    #[test]
    fn command_palette_escape_cancels_a_ready_window_verification() {
        let mut app = app();
        let (seen, requests) = std::sync::mpsc::channel();
        app.ipc = IpcClient::scripted_for_test(move |request| {
            seen.send(request).unwrap();
            Ok(clarion_ipc::Response::Handled)
        });
        app.cached_windows = vec![command_window(42)];
        app.open_command_palette();
        let (sender, reply) = std::sync::mpsc::channel();
        sender
            .send(Ok(clarion_ipc::Response::Windows(vec![command_window(42)])))
            .unwrap();
        app.pending_window_focus = Some(PendingWindowFocus { id: 42, reply });
        let ctx = egui::Context::default();
        command_frame(
            &mut app,
            &ctx,
            vec![command_key(egui::Key::Escape, egui::Modifiers::NONE)],
        );
        assert!(app.pending_window_focus.is_none());
        assert!(!app.command_palette.is_open());
        assert!(!app
            .status_message
            .as_ref()
            .is_some_and(|(message, _, _)| message.contains("已请求聚焦")));
        assert!(
            requests.recv_timeout(Duration::from_millis(20)).is_err(),
            "dismissal must not enqueue a focus action"
        );
    }

    #[test]
    fn command_palette_focus_rejects_closed_window_wrong_reply_disconnect_and_confirmation() {
        for failure in 0..5 {
            let mut app = app();
            app.cached_windows = vec![command_window(42)];
            let (sender, reply) = std::sync::mpsc::channel();
            let response = match failure {
                0 => Ok(clarion_ipc::Response::Windows(vec![command_window(99)])),
                1 => Ok(clarion_ipc::Response::Handled),
                2 => Err("server disconnected".into()),
                _ => Ok(clarion_ipc::Response::Windows(vec![command_window(42)])),
            };
            sender.send(response).unwrap();
            if failure == 3 {
                app.connection_error = Some("disconnected".into());
            }
            if failure == 4 {
                app.confirm_close = true;
            }
            app.pending_window_focus = Some(PendingWindowFocus { id: 42, reply });
            app.poll_window_focus(&egui::Context::default());
            assert!(app.pending_window_focus.is_none());
            let (message, is_error, _) = app.status_message.unwrap();
            assert!(is_error);
            let reason = [
                "已关闭",
                "无效响应",
                "server disconnected",
                "断开连接",
                "窗口跳转已取消",
            ][failure];
            assert!(
                message.contains(reason),
                "must reject before sending focus: {message}"
            );
        }
    }

    #[test]
    fn command_palette_live_action_uses_existing_ipc_without_saving_or_reloading() {
        let (seen, requests) = std::sync::mpsc::channel();
        let mut app = app();
        app.ipc = IpcClient::scripted_for_test(move |request| {
            seen.send(request).unwrap();
            Ok(clarion_ipc::Response::Handled)
        });
        app.config_mgr
            .set_raw_content("// unsaved\nlayout { gaps 11; }\n".into());
        let raw = app.config_mgr.raw_content().to_owned();
        app.execute_command(Command::WindowSpread).unwrap();
        assert!(matches!(
            requests.recv_timeout(Duration::from_secs(1)).unwrap(),
            clarion_ipc::Request::Action(clarion_ipc::Action::ToggleWindowSpread {})
        ));
        assert!(requests.try_recv().is_err());
        assert_eq!(app.config_mgr.raw_content(), raw);
        assert!(app.config_mgr.is_dirty());
        assert!(app.execute_command(Command::UndoLayout).is_err());
    }

    #[test]
    fn command_palette_single_instance_signal_opens_and_consumes_flag() {
        let mut app = app();
        let signal = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(
            crate::instance::COMMAND_PALETTE,
        ));
        app.toggle_flag = Some(signal.clone());
        command_frame(&mut app, &egui::Context::default(), vec![]);
        assert!(app.command_palette.is_open());
        assert!(app.focus_pending);
        assert_eq!(signal.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn command_palette_focus_cancels_when_close_request_and_verified_reply_arrive_together() {
        for via_toggle in [false, true] {
            let mut app = app();
            app.cached_windows = vec![command_window(42)];
            app.config_mgr
                .set_raw_content("// unsaved\nlayout { gaps 11; }\n".into());
            let (sender, reply) = std::sync::mpsc::channel();
            sender
                .send(Ok(clarion_ipc::Response::Windows(vec![command_window(42)])))
                .unwrap();
            app.pending_window_focus = Some(PendingWindowFocus { id: 42, reply });
            let mut input = egui::RawInput::default();
            let viewport = input.viewports.get_mut(&egui::ViewportId::ROOT).unwrap();
            if via_toggle {
                viewport.focused = Some(true);
                app.toggle_flag = Some(std::sync::Arc::new(std::sync::atomic::AtomicU8::new(
                    crate::instance::TOGGLE,
                )));
            } else {
                viewport.events.push(egui::ViewportEvent::Close);
            }
            let _ = egui::Context::default().run(input, |ctx| app.update_ui(ctx));
            assert!(app.pending_window_focus.is_none());
            if !via_toggle {
                assert!(app.confirm_close);
                assert!(app
                    .status_message
                    .as_ref()
                    .unwrap()
                    .0
                    .contains("窗口跳转已取消"));
            }
            assert!(!app
                .status_message
                .as_ref()
                .is_some_and(|(message, _, _)| message.contains("已请求聚焦")));
        }
    }

    #[test]
    fn command_palette_new_settings_activation_supersedes_pending_window_focus() {
        for activation in 0..4 {
            let mut app = app();
            app.cached_windows = vec![command_window(42)];
            let (sender, reply) = std::sync::mpsc::channel();
            app.pending_window_focus = Some(PendingWindowFocus { id: 42, reply });
            let events = if activation == 2 {
                vec![command_key(egui::Key::K, egui::Modifiers::CTRL)]
            } else {
                vec![]
            };
            match activation {
                0 | 1 => {
                    app.toggle_flag = Some(std::sync::Arc::new(std::sync::atomic::AtomicU8::new(
                        if activation == 0 {
                            crate::instance::COMMAND_PALETTE
                        } else {
                            crate::instance::FOCUS
                        },
                    )))
                }
                3 => app.open_command_palette(),
                _ => {}
            }
            command_frame(&mut app, &egui::Context::default(), events);
            assert!(app.pending_window_focus.is_none());
            assert!(sender
                .send(Ok(clarion_ipc::Response::Windows(vec![command_window(42)])))
                .is_err());
            if activation != 1 {
                assert!(app.command_palette.is_open());
            }
        }
    }

    #[test]
    fn appearance_view_preserves_custom_settings_outside_slider_ranges() {
        let source = "layout { border { width 20; }; focus-ring { width 21; }; }\nanimations { slowdown 3.25; window-wobble { stiffness 0.5; damping 250; mass 50; intensity 2; position-epsilon 10; velocity-epsilon 50; }; workspace-switch { cube { perspective 0.01; }; }; }\n";
        clarion_config::Config::parse_mem(source).unwrap();
        let mut app = app();
        app.config_mgr.set_raw_content(source.into());
        let before = app.config_mgr.visual_settings().clone();
        let _ = egui::Context::default().run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.ui_appearance(ui));
        });
        assert_eq!(app.config_mgr.visual_settings(), &before);
        app.flush_config_form().unwrap();
        assert_eq!(app.config_mgr.raw_content(), source);
    }

    #[test]
    fn close_effect_choice_preserves_pending_appearance_fields() {
        let mut app = app();
        app.config_mgr
            .set_raw_content("layout { border { width 2; }; }\n".into());
        app.config_mgr.visual_settings_mut().border_width = 5.25;

        app.choose_close_effect(CloseEffect::Fade).unwrap();

        let config = clarion_config::Config::parse_mem(app.config_mgr.raw_content()).unwrap();
        assert_eq!(config.layout.border.width, 5.25);
        assert_eq!(
            config.animations.window_close.effect,
            clarion_config::WindowCloseEffect::Fade
        );
        assert!(app.config_mgr.is_dirty());
    }

    #[test]
    fn overview_style_choice_preserves_pending_fields_and_invalid_form() {
        let source = "layout { border { width 2; }; gaps 8; }\n";
        let mut app = app();
        app.config_mgr.set_raw_content(source.into());
        app.config_mgr.visual_settings_mut().border_width = 5.25;
        app.choose_overview_style(Some(clarion_config::OverviewStyle::Strip))
            .unwrap();
        let config = app.config_mgr.effective_config().unwrap();
        assert_eq!(config.layout.border.width, 5.25);
        assert_eq!(config.overview.style, clarion_config::OverviewStyle::Strip);
        let draft = app.config_mgr.raw_content().to_owned();
        app.full_config = crate::full_config::FullConfig::with_pending_argument_for_test(
            &draft,
            vec![0, 1],
            "invalid",
        );
        assert!(app
            .choose_overview_style(Some(clarion_config::OverviewStyle::Stage))
            .is_err());
        assert_eq!(app.config_mgr.raw_content(), draft);
        assert!(app.full_config.has_pending());
    }

    #[test]
    fn viewing_overview_style_keeps_inheritance_and_pending_form() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.kdl");
        std::fs::write(
            dir.path().join("base.kdl"),
            "overview { style \"strip\"; }\n",
        )
        .unwrap();
        let source = "include \"base.kdl\"\nlayout { gaps 8; }\n";
        std::fs::write(&path, source).unwrap();
        let mut app = app();
        app.config_mgr = ConfigManager::from_path(&path).unwrap();
        app.full_config = crate::full_config::FullConfig::with_pending_argument_for_test(
            source,
            vec![1, 0],
            "invalid",
        );
        let validation = app.config_mgr.validation_status().clone();
        for width in [320., 500., 1000.] {
            let context = egui::Context::default();
            let _ = context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 500.),
                    )),
                    ..Default::default()
                },
                |context| {
                    egui::CentralPanel::default().show(context, |ui| app.ui_overview_style(ui));
                },
            );
            assert_eq!(app.config_mgr.raw_content(), source);
            assert_eq!(app.config_mgr.validation_status(), &validation);
            assert!(!app.config_mgr.is_dirty());
            assert!(app.full_config.has_pending());
        }
        assert_eq!(
            app.config_mgr.effective_config().unwrap().overview.style,
            clarion_config::OverviewStyle::Strip,
        );
    }

    #[test]
    fn overview_style_local_override_preserves_inherited_appearance_and_can_be_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.kdl");
        let base = "overview { style \"strip\"; zoom 0.37; workspace-shadow { off; }; }\nanimations { off; overview-open-close { duration-ms 713; }; window-close { custom-shader \"retained\"; }; }\n";
        std::fs::write(dir.path().join("base.kdl"), base).unwrap();
        std::fs::write(&path, "// keep include\ninclude \"base.kdl\"\n").unwrap();
        let mut app = app();
        app.config_mgr = ConfigManager::from_path(&path).unwrap();
        let before = app.config_mgr.effective_config().unwrap();
        app.choose_overview_style(Some(clarion_config::OverviewStyle::Stage))
            .unwrap();
        let after = app.config_mgr.effective_config().unwrap();
        assert_eq!(after.overview.style, clarion_config::OverviewStyle::Stage);
        assert_eq!(after.overview.zoom, before.overview.zoom);
        assert_eq!(
            after.overview.workspace_shadow,
            before.overview.workspace_shadow
        );
        assert_eq!(after.animations, before.animations);
        assert!(app
            .config_mgr
            .raw_content()
            .contains("// keep include\ninclude \"base.kdl\""));
        app.choose_overview_style(None).unwrap();
        assert_eq!(
            app.config_mgr.effective_config().unwrap().overview,
            before.overview
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("base.kdl")).unwrap(),
            base
        );
    }

    #[test]
    fn overview_style_choice_detects_later_include_and_root_overrides() {
        for edit_include in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.kdl");
            let included = dir.path().join("base.kdl");
            let local = "// local style\noverview { style \"stage\"; }\n";
            let source = if edit_include {
                std::fs::write(&included, local).unwrap();
                "include \"base.kdl\"\noverview { style \"stage\"; }\n".to_owned()
            } else {
                std::fs::write(&included, "overview { style \"stage\"; }\n").unwrap();
                format!("{local}include \"base.kdl\"\n")
            };
            std::fs::write(&path, &source).unwrap();
            let mut app = app();
            app.config_mgr = ConfigManager::from_path(&path).unwrap();
            if edit_include {
                app.config_mgr.open_file(&included).unwrap();
            }
            let draft = app.config_mgr.raw_content().to_owned();
            let validation = app.config_mgr.validation_status().clone();
            let error = app
                .choose_overview_style(Some(clarion_config::OverviewStyle::Strip))
                .unwrap_err();
            assert!(error.to_string().contains("后续配置覆盖"));
            assert_eq!(app.config_mgr.raw_content(), draft);
            assert_eq!(app.config_mgr.validation_status(), &validation);
            assert!(!app.config_mgr.is_dirty());
        }
    }

    #[test]
    fn close_effect_preserves_inherited_easing_and_glide_after_saving_and_reediting() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.kdl");
        let inherited = "animations { window-close { duration-ms 731; curve \"cubic-bezier\" 0.17 0.31 0.42 0.93; custom-shader \"inherited shader\"; glide { off; tilt-deg 17.123456789; depth 0.23456789; }; }; }\n";
        std::fs::write(dir.path().join("base.kdl"), inherited).unwrap();
        let source =
            "include \"base.kdl\"\nanimations { /* unrelated */ window-wobble { mass 0.321; }; }\n";
        std::fs::write(&path, source).unwrap();
        let mut app = app();
        app.ipc = IpcClient::scripted_for_test(|_| Ok(clarion_ipc::Response::Handled));
        app.config_mgr = ConfigManager::from_path(&path).unwrap();
        let before = app
            .config_mgr
            .effective_config()
            .unwrap()
            .animations
            .window_close;

        app.choose_close_effect(CloseEffect::Fade).unwrap();
        app.config_mgr.save_and_reload(&app.ipc).unwrap();
        app.config_mgr = ConfigManager::from_path(&path).unwrap();
        app.choose_close_effect(CloseEffect::Implode).unwrap();

        let after = app
            .config_mgr
            .effective_config()
            .unwrap()
            .animations
            .window_close;
        assert_eq!(after.anim.kind, before.anim.kind);
        assert_eq!(after.glide, before.glide);
        assert_eq!(after.effect, clarion_config::WindowCloseEffect::Implode);
        assert!(after.custom_shader.is_none());
        assert!(app
            .config_mgr
            .raw_content()
            .contains("/* unrelated */ window-wobble { mass 0.321; }"));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("base.kdl")).unwrap(),
            inherited
        );
    }

    #[test]
    fn close_effect_without_inherited_close_uses_its_own_default_duration() {
        for has_include in [false, true] {
            for (choice, duration) in [
                (CloseEffect::Fade, 220),
                (CloseEffect::Implode, 280),
                (CloseEffect::Explosion, 400),
            ] {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("config.kdl");
                std::fs::write(
                    dir.path().join("base.kdl"),
                    "layout { gaps 13; }\nanimations { window-wobble { mass 0.123; }; }\n",
                )
                .unwrap();
                let source = if has_include {
                    "include \"base.kdl\"\n"
                } else {
                    "layout { gaps 13; }\n"
                };
                std::fs::write(&path, source).unwrap();
                let mut app = app();
                app.config_mgr = ConfigManager::from_path(&path).unwrap();

                app.choose_close_effect(choice).unwrap();

                let config = app.config_mgr.effective_config().unwrap();
                let clarion_config::animations::Kind::Easing(params) =
                    config.animations.window_close.anim.kind
                else {
                    panic!("built-in default must use easing");
                };
                assert_eq!(params.duration_ms, duration);
                let editor = crate::kdl_edit::Editor::new(app.config_mgr.raw_content()).unwrap();
                assert!(!editor.exists(&["animations", "window-close", "duration-ms"]));
                assert!(!editor.exists(&["animations", "window-close", "curve"]));
                assert!(!editor.exists(&["animations", "window-close", "glide"]));
            }
        }
    }

    #[test]
    fn disabling_inherited_close_preserves_spring_shader_and_glide() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.kdl");
        let inherited = "animations { window-close { spring damping-ratio=0.71 stiffness=1234 epsilon=0.00234567; effect \"implode\"; custom-shader r#\"shader {\\\"nested\\\"}\"#; glide { tilt-deg 23.123456789; depth 0.7654321; }; }; }\n";
        std::fs::write(dir.path().join("base.kdl"), inherited).unwrap();
        std::fs::write(&path, "include \"base.kdl\"\n").unwrap();
        let mut app = app();
        app.config_mgr = ConfigManager::from_path(&path).unwrap();
        let before = app
            .config_mgr
            .effective_config()
            .unwrap()
            .animations
            .window_close;

        app.choose_close_effect(CloseEffect::Disabled).unwrap();

        let after = app
            .config_mgr
            .effective_config()
            .unwrap()
            .animations
            .window_close;
        assert!(after.anim.off);
        assert_eq!(after.anim.kind, before.anim.kind);
        assert_eq!(after.glide, before.glide);
        assert_eq!(after.custom_shader, before.custom_shader);
        assert_eq!(after.effect, before.effect);
    }

    #[test]
    fn later_include_cannot_silently_override_a_close_effect_choice() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.kdl");
        std::fs::write(
            dir.path().join("later.kdl"),
            "animations { window-close { effect \"implode\"; }; }\n",
        )
        .unwrap();
        let source =
            "animations { window-close { effect \"explosion\"; }; }\ninclude \"later.kdl\"\n";
        std::fs::write(&path, source).unwrap();
        let mut app = app();
        app.config_mgr = ConfigManager::from_path(&path).unwrap();

        let validation = app.config_mgr.validation_status().clone();
        for choice in [CloseEffect::Fade, CloseEffect::Disabled] {
            let error = app.choose_close_effect(choice).unwrap_err().to_string();
            assert!(error.contains("later.kdl"));
            assert_eq!(app.config_mgr.raw_content(), source);
            assert!(!app.config_mgr.is_dirty());
            assert_eq!(app.config_mgr.validation_status(), &validation);
        }
    }

    #[test]
    fn later_include_without_close_settings_does_not_block_the_control() {
        for local_close in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.kdl");
            std::fs::write(
                dir.path().join("later.kdl"),
                "layout { gaps 19; }\nanimations { window-wobble { mass 0.234; }; }\n",
            )
            .unwrap();
            let source = if local_close {
                "animations { window-close { duration-ms 551; curve \"ease-out-quad\"; glide { off; depth 0.345; }; }; }\ninclude \"later.kdl\"\n"
            } else {
                "animations { window-wobble { mass 0.123; }; }\ninclude \"later.kdl\"\n"
            };
            std::fs::write(&path, source).unwrap();
            let mut app = app();
            app.config_mgr = ConfigManager::from_path(&path).unwrap();

            app.choose_close_effect(CloseEffect::Fade).unwrap();

            let close = app
                .config_mgr
                .effective_config()
                .unwrap()
                .animations
                .window_close;
            assert_eq!(close.effect, clarion_config::WindowCloseEffect::Fade);
            let clarion_config::animations::Kind::Easing(params) = close.anim.kind else {
                panic!("easing expected");
            };
            assert_eq!(params.duration_ms, if local_close { 551 } else { 220 });
        }
    }

    #[test]
    fn editing_included_close_cannot_report_success_if_root_overrides_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("config.kdl");
        let edited = dir.path().join("edited.kdl");
        let source = "animations { window-close { effect \"explosion\"; }; }\n";
        std::fs::write(&edited, source).unwrap();
        std::fs::write(
            &root,
            "include \"edited.kdl\"\nanimations { window-close { effect \"implode\"; }; }\n",
        )
        .unwrap();
        let mut app = app();
        app.config_mgr = ConfigManager::from_path(&root).unwrap();
        app.config_mgr.open_file(&edited).unwrap();

        assert!(app.choose_close_effect(CloseEffect::Fade).is_err());
        assert_eq!(app.config_mgr.raw_content(), source);
        assert!(!app.config_mgr.is_dirty());
    }

    #[test]
    fn close_effect_choice_keeps_invalid_structured_draft_for_correction() {
        let source = "layout { gaps 8; }\n";
        let mut app = app();
        app.config_mgr.set_raw_content(source.into());
        app.full_config = crate::full_config::FullConfig::with_pending_argument_for_test(
            source,
            vec![0, 0],
            "invalid",
        );

        assert!(app.choose_close_effect(CloseEffect::Fade).is_err());
        assert_eq!(app.config_mgr.raw_content(), source);
        assert!(app.full_config.has_pending());
    }

    #[test]
    fn viewing_blur_controls_does_not_clamp_or_rewrite_custom_values() {
        let source =
            "blur { passes 255; offset 4.123456789; noise 9.123456789; saturation 9.123456789; }\n";
        let mut app = app();
        app.config_mgr.set_raw_content(source.into());
        let context = egui::Context::default();
        let _ = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| app.ui_background_blur(ui));
        });
        assert_eq!(app.config_mgr.raw_content(), source);
    }

    #[test]
    fn failed_reload_preserves_pending_structured_form() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.kdl");
        let source = "layout { gaps 8; }\n";
        std::fs::write(&path, source).unwrap();
        let mut app = app();
        app.config_mgr.open_file(&path).unwrap();
        app.full_config = crate::full_config::FullConfig::with_pending_argument_for_test(
            source,
            vec![0, 0],
            "12",
        );
        std::fs::remove_file(path).unwrap();

        assert!(app.reload_config_from_disk().is_err());
        assert_eq!(app.config_mgr.raw_content(), source);
        assert!(app.full_config.has_pending());
        assert_eq!(
            app.full_config.apply_pending(source).unwrap().unwrap(),
            "layout { gaps 12; }\n",
        );
    }

    #[test]
    fn successful_reload_discards_pending_structured_form() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.kdl");
        let source = "layout { gaps 8; }\n";
        std::fs::write(&path, source).unwrap();
        let mut app = app();
        app.config_mgr.open_file(&path).unwrap();
        app.full_config = crate::full_config::FullConfig::with_pending_argument_for_test(
            source,
            vec![0, 0],
            "12",
        );
        let updated = "layout { gaps 24; }\n";
        std::fs::write(path, updated).unwrap();

        app.reload_config_from_disk().unwrap();
        assert_eq!(app.config_mgr.raw_content(), updated);
        assert!(!app.full_config.has_pending());
        assert!(app.full_config.apply_pending(updated).unwrap().is_none());
    }

    #[test]
    fn close_request_keeps_dirty_config_and_shows_choice() {
        let mut app = app();
        app.config_mgr
            .set_raw_content("layout { gaps 20; }\n".into());
        assert!(cancelled(&close(&mut app)));
        assert!(app.confirm_close);
        assert_eq!(app.config_mgr.raw_content(), "layout { gaps 20; }\n");
        assert!(app.config_mgr.is_dirty());
    }
    #[test]
    fn clean_close_and_explicit_discard_are_not_intercepted() {
        let mut app = app();
        assert!(!cancelled(&close(&mut app)));
        assert!(!app.confirm_close);
        app.config_mgr.set_raw_content("layout {}\n".into());
        app.allow_close = true;
        assert!(!cancelled(&close(&mut app)));
    }

    #[test]
    fn save_and_exit_waits_for_preview_confirmation_and_honors_cancellation() {
        for keep in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.kdl");
            std::fs::write(&path, "output \"test\" { scale 1; }\n").unwrap();
            let mut app = app();
            app.config_mgr = ConfigManager::from_path(&path).unwrap();
            app.ipc = IpcClient::scripted_for_test(|request| {
                Ok(match request {
                    clarion_ipc::Request::PreviewOutputConfig { .. } => {
                        clarion_ipc::Response::OutputPreview {
                            id: 42,
                            timeout_ms: 15_000,
                        }
                    }
                    _ => clarion_ipc::Response::Handled,
                })
            });
            app.config_mgr
                .set_raw_content("output \"test\" { scale 2; }\n".into());
            app.config_mgr.save_and_reload(&app.ipc).unwrap();
            app.close_after_preview = true;
            let ctx = egui::Context::default();
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while app.config_mgr.preview_seconds().is_none() {
                let _ = ctx.run(egui::RawInput::default(), |ctx| {
                    app.poll_display_preview(ctx)
                });
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(
                app.close_after_preview,
                "starting preview must preserve save-and-exit intent"
            );
            assert!(!app.allow_close);
            app.config_mgr.finish_preview(&app.ipc, keep).unwrap();
            let output = loop {
                let output = ctx.run(egui::RawInput::default(), |ctx| {
                    app.poll_display_preview(ctx)
                });
                if !app.config_mgr.preview_pending() {
                    break output;
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(1));
            };
            assert!(!app.close_after_preview);
            assert_eq!(app.allow_close, keep);
            let closed = output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::Close));
            assert_eq!(closed, keep);
            assert_eq!(app.config_mgr.is_dirty(), !keep);
        }
    }
}

#[cfg(test)]
#[path = "preferences_ui_tests.rs"]
mod preferences_ui_tests;
