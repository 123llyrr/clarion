//! Direct display preferences share the compositor's guarded preview protocol.
//! The configuration editor remains untouched until a settled edit is staged.

#[cfg(test)]
#[path = "display_controls_tests.rs"]
mod tests;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Context;
use clarion_config::output::OutputName;

use super::*;
use crate::output_settings::OutputSettings;

#[derive(Default)]
pub(super) struct DisplayControls {
    settings: Option<OutputSettings>,
    baseline: Option<OutputSettings>,
    target: Option<OutputName>,
    source: Option<(PathBuf, String, String)>,
    due: Option<Instant>,
    original: Option<String>,
    retry: Option<(OutputName, OutputSettings, OutputSettings)>,
    status: DisplayStatus,
    pub(super) confirm_close: bool,
}

#[derive(Default)]
enum DisplayStatus {
    #[default]
    Ready,
    Pending,
    Preview,
    Applied,
    Restored,
    Error(String),
}

impl DisplayControls {
    pub(super) fn pending(&self) -> bool {
        self.settings != self.baseline
    }

    pub(super) fn owns_preview(&self) -> bool {
        self.original.is_some()
    }

    pub(super) fn needs_save(&self) -> bool {
        self.pending() || self.retry.is_some()
    }

    pub(super) fn selection_locked(&self) -> bool {
        self.needs_save() || self.owns_preview()
    }
}

impl ClarionSettingsApp {
    fn sync_display_controls(&mut self, output: &Output) {
        if self.display.pending()
            || self.display.owns_preview()
            || self.config_mgr.is_dirty()
            || self.full_config.has_pending()
        {
            return;
        }
        let target = OutputName::from_ipc_output(output);
        let source = (
            self.config_mgr.config_path().to_owned(),
            self.config_mgr.raw_content().to_owned(),
            format!("{target:?}"),
        );
        if self.display.source.as_ref() == Some(&source) {
            return;
        }
        self.display.source = Some(source);
        self.display.target = Some(target.clone());
        match OutputSettings::read(&self.config_mgr, &target) {
            Ok(settings) => {
                self.display.baseline = Some(settings.clone());
                self.display.settings = Some(settings);
            }
            Err(error) => {
                self.display.baseline = None;
                self.display.settings = None;
                self.display.status = DisplayStatus::Error(format!("{error:#}"));
            }
        }
    }

    pub(super) fn refresh_display_controls_from_disk(&mut self) {
        if self.active_tab != ActiveTab::Outputs
            || self.display.selection_locked()
            || self.config_mgr.preview_pending()
            || self.config_mgr.is_dirty()
            || self.full_config.has_pending()
            || self.common.pending()
            || self.common.worker.is_busy()
        {
            return;
        }
        match self.config_mgr.reload_from_disk() {
            Ok(()) => {
                self.display.source = None;
                self.common.source = None;
                self.full_config = Default::default();
                if matches!(self.display.status, DisplayStatus::Error(_)) {
                    self.display.status = DisplayStatus::Ready;
                }
            }
            Err(error) => {
                self.display.status = DisplayStatus::Error(format!("无法刷新显示配置：{error:#}"));
            }
        }
    }

    pub(super) fn ui_direct_output(&mut self, ui: &mut Ui, output: &Output) {
        if self.config_mgr.config_path() != self.config_mgr.root_path() {
            ui.label("显示器控件使用主配置，导入文件的未保存草稿会保留。");
            if ui.button("返回主配置").clicked() {
                let path = self.config_mgr.root_path().to_owned();
                if let Err(error) = self.open_config_file(&path) {
                    self.set_status(error.to_string(), true);
                } else {
                    self.display = DisplayControls::default();
                }
            }
            return;
        }
        self.sync_display_controls(output);
        let manual = (self.config_mgr.is_dirty() && !self.display.owns_preview())
            || self.full_config.has_pending();
        if manual {
            ui.label("高级配置还有未保存修改，请先处理草稿，再调整显示器。");
            ui.horizontal_wrapped(|ui| {
                if ui.button("保存现有草稿").clicked() {
                    self.save_changes();
                }
                if ui.button("查看高级配置").clicked() {
                    self.switch_tab(ActiveTab::FullConfig);
                }
            });
            ui.add_space(12.);
        }
        if self.common.pending() || self.common.retry.is_some() {
            ui.label("请先完成尚未应用的常用设置，再调整显示器。");
            if ui.button("查看常用设置").clicked() {
                self.switch_tab(ActiveTab::Appearance);
            }
            ui.add_space(12.);
        }
        let enabled = !manual
            && self.connection_error.is_none()
            && !self.config_mgr.preview_pending()
            && !self.common.pending()
            && self.common.retry.is_none()
            && !self.common.worker.is_busy();
        let active = self
            .cached_outputs
            .iter()
            .filter(|o| o.logical.is_some())
            .count();
        let Some(settings) = &mut self.display.settings else {
            ui.label("无法读取此显示器的配置，请在高级配置检查错误。");
            return;
        };
        let before = settings.clone();
        ui.add_enabled_ui(enabled, |ui| {
            crate::output_preferences::show(ui, output, settings, active);
        });
        if *settings != before {
            self.display.retry = None;
            if self.display.pending() {
                self.display.due = Some(Instant::now() + Duration::from_millis(350));
                self.display.status = DisplayStatus::Pending;
            } else {
                self.display.due = None;
                self.display.status = DisplayStatus::Ready;
            }
        }
    }

    pub(super) fn ui_display_save_status(&mut self, ui: &mut Ui) {
        let message = match &self.display.status {
            DisplayStatus::Ready => "更改后自动预览，确认保留才保存",
            DisplayStatus::Pending => "等待预览…",
            DisplayStatus::Preview => "正在预览显示设置，确认前不会写入文件",
            DisplayStatus::Applied => "显示设置已确认并保存",
            DisplayStatus::Restored => "已恢复原显示设置，配置文件未修改",
            DisplayStatus::Error(_) => "显示设置尚未完成应用",
        };
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(message).size(12.5).color(
                if matches!(self.display.status, DisplayStatus::Error(_)) {
                    palette::error()
                } else {
                    palette::muted()
                },
            ));
            if let DisplayStatus::Error(error) = &self.display.status {
                ui.menu_button("查看原因", |ui| {
                    ui.set_max_width(360.);
                    ui.add(egui::Label::new(error).wrap());
                });
            }
            if self.display.retry.is_some() {
                if ui.button("重试预览").clicked() {
                    self.start_display_controls_preview();
                }
                if ui.button("放弃这次修改").clicked() {
                    self.display.retry = None;
                    self.display.status = DisplayStatus::Ready;
                }
            }
        });
    }

    pub(super) fn start_display_controls_preview(&mut self) {
        if self.config_mgr.preview_pending() || self.display.owns_preview() {
            return;
        }
        let request = if self.display.pending() {
            self.display
                .target
                .clone()
                .zip(self.display.baseline.clone())
                .zip(self.display.settings.clone())
                .map(|((target, before), after)| (target, before, after))
        } else {
            self.display.retry.clone()
        };
        let Some((target, before, after)) = request else {
            return;
        };
        self.display.due = None;
        self.display.retry = Some((target.clone(), before.clone(), after.clone()));
        let original = self.config_mgr.raw_content().to_owned();
        let clean_before = !self.config_mgr.is_dirty() && !self.full_config.has_pending();
        let result = (|| -> anyhow::Result<()> {
            anyhow::ensure!(
                !self.config_mgr.is_dirty() && !self.full_config.has_pending(),
                "高级草稿尚未处理，未启动显示预览"
            );
            anyhow::ensure!(
                !self.common.pending()
                    && self.common.retry.is_none()
                    && !self.common.worker.is_busy(),
                "请先处理尚未应用的常用设置"
            );
            anyhow::ensure!(
                self.connection_error.is_none(),
                "Clarion 尚未连接，未修改配置文件"
            );
            let output = self
                .cached_outputs
                .iter()
                .find(|o| o.name == target.connector)
                .context("目标显示器已断开连接，请重新选择")?;
            anyhow::ensure!(
                format!("{:?}", OutputName::from_ipc_output(output)) == format!("{target:?}"),
                "目标显示器已改变，请重新选择"
            );
            let active = self
                .cached_outputs
                .iter()
                .filter(|o| o.logical.is_some())
                .count();
            anyhow::ensure!(
                after.enabled || output.logical.is_none() || active > 1,
                "请至少保留一个活动显示器"
            );
            anyhow::ensure!(
                before.hdr || !after.hdr || output.hdr_supported,
                "此显示器当前不支持 HDR"
            );
            anyhow::ensure!(
                before.vrr != crate::output_settings::VrrMode::Off
                    || after.vrr == crate::output_settings::VrrMode::Off
                    || output.vrr_supported,
                "此显示器当前不支持可变刷新率"
            );
            OutputSettings::apply(&mut self.config_mgr, &target, &before, &after)?;
            self.config_mgr.begin_output_preview(&self.ipc)
        })();
        match result {
            Ok(()) => {
                self.display.original = Some(original);
                self.display.status = DisplayStatus::Preview;
            }
            Err(error) => {
                // Only a patch created above belongs to this page. Existing manual
                // drafts were rejected before touching the editor.
                if self.config_mgr.raw_content() != original && !self.full_config.has_pending() {
                    self.config_mgr.set_raw_content(original);
                }
                if clean_before {
                    let _ = self.config_mgr.reload_from_disk();
                    self.common.source = None;
                }
                self.display.settings = None;
                self.display.baseline = None;
                self.display.source = None;
                self.display.status = DisplayStatus::Error(format!("{error:#}"));
                self.close_after_preview = false;
            }
        }
    }

    pub(super) fn submit_display_controls_if_ready(&mut self, ctx: &egui::Context) {
        if self.display.pending()
            && self
                .display
                .due
                .is_some_and(|due| Instant::now() >= due || self.active_tab != ActiveTab::Outputs)
            && !ctx.input(|input| input.pointer.any_down())
            && !ctx.output(|output| output.ime.is_some())
            && !self.common.worker.is_busy()
        {
            self.start_display_controls_preview();
        }
        if self.display.pending() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }

    /// Called only after the compositor's asynchronous preview has finished.
    pub(super) fn finish_display_controls_preview(
        &mut self,
        confirmed: bool,
        error: Option<String>,
    ) {
        if let Some(original) = self.display.original.take() {
            if !confirmed && self.config_mgr.is_dirty() {
                self.config_mgr.set_raw_content(original);
            }
            let reload = self.config_mgr.reload_from_disk();
            let already_saved = reload.is_ok()
                && self
                    .display
                    .retry
                    .as_ref()
                    .is_some_and(|(target, _, after)| {
                        OutputSettings::read(&self.config_mgr, target)
                            .is_ok_and(|actual| &actual == after)
                    });
            self.display.settings = None;
            self.display.baseline = None;
            self.display.source = None;
            self.display.due = None;
            self.common.source = None;
            self.full_config = Default::default();
            self.display.status = match (error, reload) {
                (Some(error), _) => DisplayStatus::Error(error),
                (_, Err(error)) => DisplayStatus::Error(format!("无法重新读取显示配置：{error:#}")),
                (None, _) if confirmed => DisplayStatus::Applied,
                _ => DisplayStatus::Restored,
            };
            if already_saved || !matches!(self.display.status, DisplayStatus::Error(_)) {
                self.display.retry = None;
            }
        }
    }

    pub(super) fn display_controls_close_guard(&mut self, ctx: &egui::Context) -> bool {
        if !self.allow_close && ctx.input(|i| i.viewport().close_requested()) {
            if self.display.pending() && self.display.due.is_some() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.close_after_preview = true;
                self.start_display_controls_preview();
                return true;
            }
            if self.display.retry.is_some()
                && !self.display.owns_preview()
                && !self.config_mgr.is_dirty()
                && !self.full_config.has_pending()
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.display.confirm_close = true;
            }
        }
        if !self.display.confirm_close {
            return false;
        }
        egui::Window::new("显示器修改尚未应用")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label("重试预览并确认后退出，或保留当前配置关闭窗口。");
                ui.horizontal_wrapped(|ui| {
                    if ui.button("重试并退出").clicked() {
                        self.display.confirm_close = false;
                        self.close_after_preview = true;
                        self.start_display_controls_preview();
                    }
                    if ui.button("关闭").clicked() {
                        self.display.confirm_close = false;
                        self.display.retry = None;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    if ui.button("继续编辑").clicked() {
                        self.display.confirm_close = false;
                    }
                });
            });
        true
    }
}
