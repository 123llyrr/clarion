use crate::kdl_edit::Editor;
use anyhow::{Context, Result};
use realand_config::{read_config_file, Config};
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const VALIDATION_DEBOUNCE: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, PartialEq)]
pub enum ValidationStatus {
    NotChecked,
    Valid,
    Error(String),
}

#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub struct VisualSettings {
    // None inherits the value from included configuration files (or compositor defaults).
    pub animations_enabled: Option<bool>,
    pub animation_slowdown: Option<f32>,
    pub motion_preset: Option<MotionPreset>,
    pub default_tiling_dwindle: bool,
    pub gaps: f32,
    pub center_focused_column: String, // "never", "always", "on-overflow"
    pub always_center_single_column: bool,
    pub border_width: f32,
    pub border_active_color: [f32; 4],
    pub border_inactive_color: [f32; 4],
    pub focus_ring_width: f32,
    pub focus_ring_active_color: [f32; 4],
    pub focus_ring_inactive_color: [f32; 4],
    pub animated_border_enabled: bool,
    pub gradient_from_color: [f32; 4],
    pub gradient_to_color: [f32; 4],
    pub gradient_rotation_speed: f32,
    // Classic 3D effects
    pub window_wobble_enabled: bool,
    pub wobble_mesh: u8,
    pub wobble_stiffness: f32,
    pub wobble_damping: f32,
    pub wobble_mass: f32,
    pub wobble_intensity: f32,
    pub wobble_position_epsilon: f32,
    pub wobble_velocity_epsilon: f32,
    pub workspace_cube_enabled: bool,
    pub cube_perspective: f32,
    pub cube_background_color: [f32; 4],
    pub window_glide_enabled: bool,
    pub glide_tilt_deg: f32,
    pub glide_depth: f32,
    // Hot corners
    pub hot_corner_top_left: bool,
    pub hot_corner_top_right: bool,
    pub hot_corner_bottom_left: bool,
    pub hot_corner_bottom_right: bool,
    // Input
    pub touchpad_tap: bool,
    pub touchpad_natural_scroll: bool,
    pub touchpad_accel_speed: f32,
    pub mouse_natural_scroll: bool,
    pub mouse_accel_speed: f32,
    pub keyboard_numlock: bool,
    pub keyboard_repeat_delay: u16,
    pub keyboard_repeat_rate: u8,
}

impl Default for VisualSettings {
    fn default() -> Self {
        let keyboard = realand_config::input::Keyboard::default();
        Self {
            animations_enabled: None,
            animation_slowdown: None,
            motion_preset: None,
            default_tiling_dwindle: false,
            gaps: 16.0,
            center_focused_column: "never".into(),
            always_center_single_column: false,
            border_width: realand_config::Border::default().width as f32,
            border_active_color: [0.98, 0.45, 0.08, 1.0], // #f97316
            border_inactive_color: [0.3, 0.3, 0.35, 0.8],
            focus_ring_width: realand_config::FocusRing::default().width as f32,
            focus_ring_active_color: [0.24, 0.48, 0.99, 1.0], // #3d7bfd
            focus_ring_inactive_color: [0.2, 0.2, 0.25, 0.5],
            animated_border_enabled: false,
            gradient_from_color: [0.98, 0.45, 0.08, 1.0],
            gradient_to_color: [0.24, 0.48, 0.99, 1.0],
            gradient_rotation_speed: 45.0,
            window_wobble_enabled: true,
            wobble_mesh: 12,
            wobble_stiffness: 200.0,
            wobble_damping: 12.0,
            wobble_mass: 1.0,
            wobble_intensity: 0.2,
            wobble_position_epsilon: 0.25,
            wobble_velocity_epsilon: 2.0,
            workspace_cube_enabled: true,
            cube_perspective: 0.65,
            cube_background_color: [16. / 255., 16. / 255., 24. / 255., 1.],
            window_glide_enabled: true,
            glide_tilt_deg: 12.0,
            glide_depth: 0.08,
            hot_corner_top_left: false,
            hot_corner_top_right: false,
            hot_corner_bottom_left: false,
            hot_corner_bottom_right: false,
            touchpad_tap: true,
            touchpad_natural_scroll: true,
            touchpad_accel_speed: 0.0,
            mouse_natural_scroll: false,
            mouse_accel_speed: 0.0,
            keyboard_numlock: true,
            keyboard_repeat_delay: keyboard.repeat_delay,
            keyboard_repeat_rate: keyboard.repeat_rate,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MotionPreset {
    Fluid,
    Lightweight,
    Reduced,
}

impl VisualSettings {
    pub fn apply_motion_preset(&mut self, preset: MotionPreset) {
        self.motion_preset = Some(preset);
        self.animations_enabled = Some(!matches!(preset, MotionPreset::Reduced));
        // Reduced motion preserves the user's effects and timing for later re-enabling.
        if matches!(preset, MotionPreset::Reduced) {
            return;
        }
        let fluid = matches!(preset, MotionPreset::Fluid);
        self.animation_slowdown = Some(if fluid { 1. } else { 0.7 });
        self.window_wobble_enabled = fluid;
        self.workspace_cube_enabled = fluid;
        self.window_glide_enabled = fluid;
    }
}

pub struct ConfigManager {
    config_path: PathBuf,
    root_path: PathBuf,
    disk_content: Option<String>,
    raw_content: String,
    validation_status: ValidationStatus,
    visual_settings: VisualSettings,
    visual_saved: VisualSettings,
    effective_animations: Option<realand_config::animations::Animations>,
    is_dirty: bool,
    validation_due: Option<Instant>,
    preview: Option<crate::output_preview::Preview>,
}

impl ConfigManager {
    pub fn new() -> Result<Self> {
        Self::from_path(&Self::find_config_path())
    }

    pub(crate) fn from_path(path: &Path) -> Result<Self> {
        let config_path = std::path::absolute(path)?;
        let disk_content = match read_config_file(&config_path) {
            Ok(content) => Some(content),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Failed to read config from {:?}", config_path));
            }
        };
        let raw_content = disk_content.clone().unwrap_or_default();

        let visual_settings = Self::parse_visual_settings(&raw_content);

        let mut mgr = Self {
            root_path: config_path.clone(),
            disk_content,
            config_path,
            raw_content,
            validation_status: ValidationStatus::NotChecked,
            visual_settings: visual_settings.clone(),
            visual_saved: visual_settings,
            effective_animations: None,
            is_dirty: false,
            validation_due: None,
            preview: None,
        };

        mgr.validate();
        Ok(mgr)
    }

    pub fn empty() -> Self {
        let config_path = Self::find_config_path();
        let config_path = std::path::absolute(&config_path).unwrap_or(config_path);
        Self {
            root_path: config_path.clone(),
            config_path,
            disk_content: None,
            raw_content: String::new(),
            validation_status: ValidationStatus::Error("failed to load config".into()),
            visual_settings: VisualSettings::default(),
            visual_saved: VisualSettings::default(),
            effective_animations: None,
            is_dirty: false,
            validation_due: None,
            preview: None,
        }
    }

    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    pub fn raw_content(&self) -> &str {
        &self.raw_content
    }

    pub fn set_raw_content(&mut self, content: String) {
        if self.raw_content != content {
            self.raw_content = content;
            self.is_dirty = self.disk_content.as_deref().unwrap_or_default() != self.raw_content;
            self.visual_settings = Self::parse_visual_settings(&self.raw_content);
            if let Ok(config) = self.effective_config() {
                Self::resolve_effective_settings(&config, &mut self.visual_settings);
                self.effective_animations = Some(config.animations);
            } else {
                self.effective_animations = None;
            }
            self.visual_saved = self.visual_settings.clone();
            self.validation_status = ValidationStatus::NotChecked;
            self.validation_due = Some(Instant::now() + VALIDATION_DEBOUNCE);
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.is_dirty || self.visual_settings != self.visual_saved
    }

    pub fn visual_is_dirty(&self) -> bool {
        self.visual_settings != self.visual_saved
    }

    pub fn validation_status(&self) -> &ValidationStatus {
        &self.validation_status
    }

    pub fn visual_settings(&self) -> &VisualSettings {
        &self.visual_settings
    }

    pub fn visual_settings_mut(&mut self) -> &mut VisualSettings {
        &mut self.visual_settings
    }

    pub fn effective_animations(&self) -> Option<&realand_config::animations::Animations> {
        self.effective_animations.as_ref()
    }

    fn find_config_path() -> PathBuf {
        if let Ok(path) = std::env::var("REALAND_CONFIG") {
            let p = PathBuf::from(path);
            return p;
        }

        if let Some(config_dir) = directories::BaseDirs::new().map(|b| b.config_dir().to_path_buf())
        {
            let p = config_dir.join("realand").join("config.kdl");
            return p;
        }

        PathBuf::from(".config/realand/config.kdl")
    }

    pub fn validate(&mut self) {
        self.validation_due = None;
        let result = if self.config_path == self.root_path {
            Config::parse(&self.config_path, &self.raw_content)
                .config
                .map_err(|e| e.to_string())
        } else {
            read_config_file(&self.root_path)
                .map_err(|e| e.to_string())
                .and_then(|root| {
                    let path = self
                        .config_path
                        .canonicalize()
                        .unwrap_or_else(|_| self.config_path.clone());
                    Config::parse_with_overrides(
                        &self.root_path,
                        &root,
                        [(path, self.raw_content.clone())].into(),
                    )
                    .config
                    .map_err(|e| e.to_string())
                })
        };
        self.validation_status = match result {
            Ok(config) => {
                if !self.visual_is_dirty() {
                    Self::resolve_effective_settings(&config, &mut self.visual_settings);
                    self.visual_saved = self.visual_settings.clone();
                }
                self.effective_animations = Some(config.animations);
                ValidationStatus::Valid
            }
            Err(err) => {
                self.effective_animations = None;
                ValidationStatus::Error(err.to_string())
            }
        };
    }

    /// Validate an edited config once the user has paused typing.
    pub fn validate_if_due(&mut self) {
        if self.validation_due.is_some_and(|due| Instant::now() >= due) {
            self.validate();
        }
    }

    pub fn save_and_reload(&mut self, ipc: &crate::ipc::IpcClient) -> Result<()> {
        anyhow::ensure!(self.preview.is_none(), "请先确认或恢复显示设置");
        self.validate();
        if let ValidationStatus::Error(err) = &self.validation_status {
            anyhow::bail!("配置校验失败，未写入文件: {err}");
        }

        self.check_disk_unchanged()?;

        if self.outputs_changed()? {
            let dependencies = self.preview_dependencies()?;
            let reply = ipc.request(realand_ipc::Request::PreviewOutputConfig {
                root_path: std::path::absolute(&self.root_path)?
                    .to_str()
                    .context("配置路径需要 UTF-8")?
                    .into(),
                edited_path: std::path::absolute(&self.config_path)?
                    .to_str()
                    .context("配置路径需要 UTF-8")?
                    .into(),
                content: self.raw_content.clone(),
            })?;
            self.preview = Some(crate::output_preview::Preview {
                content: self.raw_content.clone(),
                dependencies,
                phase: crate::output_preview::Phase::Starting {
                    reply,
                    sent: Instant::now(),
                },
            });
            return Ok(());
        }
        self.write_and_reload(ipc)
    }

    pub(crate) fn merged_config(&self, content: &str) -> Result<Config> {
        let result = if self.config_path == self.root_path {
            Config::parse(&self.root_path, content)
        } else {
            let root = read_config_file(&self.root_path)?;
            let edited = self
                .config_path
                .canonicalize()
                .unwrap_or(std::path::absolute(&self.config_path)?);
            Config::parse_with_overrides(&self.root_path, &root, [(edited, content.into())].into())
        };
        result.config.map_err(|e| anyhow::anyhow!(e.to_string()))
    }

    /// Resolve the current draft and its includes, for controls that must
    /// preserve a complete animation block when creating a local override.
    pub fn effective_config(&self) -> Result<Config> {
        self.merged_config(&self.raw_content)
    }

    /// Sources loaded alongside this draft, excluding the edited file itself.
    /// Missing optional includes are retained as None.
    pub fn effective_source_contents(&self) -> Result<Vec<(PathBuf, Option<Vec<u8>>)>> {
        self.preview_dependencies()
    }

    fn outputs_changed(&self) -> Result<bool> {
        let proposed = self.merged_config(&self.raw_content)?.outputs;
        Ok(self
            .merged_config(self.disk_content.as_deref().unwrap_or_default())
            .map_or(true, |old| old.outputs != proposed))
    }

    fn preview_dependencies(&self) -> Result<Vec<(PathBuf, Option<Vec<u8>>)>> {
        let root = if self.root_path == self.config_path {
            self.raw_content.clone()
        } else {
            read_config_file(&self.root_path)?
        };
        let edited = self
            .config_path
            .canonicalize()
            .unwrap_or(std::path::absolute(&self.config_path)?);
        let mut paths = Config::parse_with_overrides(
            &self.root_path,
            &root,
            [(edited, self.raw_content.clone())].into(),
        )
        .includes;
        if self.root_path != self.config_path {
            paths.push(self.root_path.clone());
        }
        paths.sort();
        paths.dedup();
        paths
            .into_iter()
            .filter(|p| p != &self.config_path)
            .map(|path| Self::read_dependency(&path).map(|bytes| (path, bytes)))
            .collect()
    }

    fn read_dependency(path: &Path) -> Result<Option<Vec<u8>>> {
        match read_config_file(path) {
            Ok(content) => Ok(Some(content.into_bytes())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub fn preview_pending(&self) -> bool {
        self.preview.is_some()
    }

    pub fn preview_seconds(&self) -> Option<u64> {
        match &self.preview.as_ref()?.phase {
            crate::output_preview::Phase::Ready { deadline, .. } => {
                Some(deadline.saturating_duration_since(Instant::now()).as_secs() + 1)
            }
            _ => None,
        }
    }

    fn check_preview_unchanged(&self) -> Result<()> {
        self.check_disk_unchanged()?;
        if let Some(preview) = &self.preview {
            anyhow::ensure!(
                preview.content == self.raw_content,
                "草稿在预览期间发生变化，请恢复后重新预览"
            );
            for (path, before) in &preview.dependencies {
                anyhow::ensure!(
                    &Self::read_dependency(path)? == before,
                    "关联配置已被其他程序修改，未保存：{}",
                    path.display()
                );
            }
        }
        Ok(())
    }

    pub fn finish_preview(&mut self, ipc: &crate::ipc::IpcClient, keep: bool) -> Result<()> {
        use crate::output_preview::Phase;
        let Some(Phase::Ready { id, deadline }) = self.preview.as_ref().map(|p| &p.phase) else {
            anyhow::bail!("正在等待 Realand 响应");
        };
        let id = *id;
        if keep {
            anyhow::ensure!(Instant::now() < *deadline, "确认时间已过，请等待恢复");
            if let Err(error) = self.check_preview_unchanged() {
                self.finish_preview(ipc, false)?;
                return Err(error);
            }
        }
        let request = if keep {
            realand_ipc::Request::ConfirmOutputPreview { id }
        } else {
            realand_ipc::Request::CancelOutputPreview { id }
        };
        let reply = match ipc.request(request) {
            Ok(reply) => reply,
            Err(error) => {
                if !keep {
                    self.preview = None;
                }
                return Err(error.context("无法发送显示操作；未确认的预览仍会由 Realand 超时恢复"));
            }
        };
        self.preview.as_mut().unwrap().phase = Phase::Finishing { reply, keep, id };
        Ok(())
    }

    pub fn poll_preview(&mut self, ipc: &crate::ipc::IpcClient) -> Option<Result<String>> {
        use crate::output_preview::Phase;
        use realand_ipc::Response;
        let preview = self.preview.as_mut()?;
        let (result, keep, id, save_error) = match &preview.phase {
            Phase::Ready { deadline, .. } => {
                if Instant::now() < *deadline {
                    return None;
                }
                // Explicit cancellation also handles queue delays before the preview began.
                return self.finish_preview(ipc, false).err().map(Err);
            }
            Phase::Starting { reply, .. } => (reply.try_recv(), None, None, None),
            Phase::Finishing { reply, keep, id } => {
                (reply.try_recv(), Some(*keep), Some(*id), None)
            }
            Phase::RollingBack { reply, error } => {
                (reply.try_recv(), Some(false), None, Some(error.clone()))
            }
        };
        let result = match result {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return None,
            Err(error) => Err(error.to_string()),
        };
        if let Some(error) = save_error {
            self.preview = None;
            return Some(Err(match result {
                Ok(Response::Handled) => {
                    anyhow::anyhow!("{error}。已恢复原显示设置，草稿已保留，文件未写入。")
                }
                other => anyhow::anyhow!(
                    "{error}。恢复请求未获确认：{other:?}。草稿已保留；请核对当前显示状态。"
                ),
            }));
        }
        if let (None, Ok(Response::OutputPreview { id, timeout_ms })) = (keep, &result) {
            let Phase::Starting { sent, .. } = preview.phase else {
                unreachable!()
            };
            preview.phase = Phase::Ready {
                id: *id,
                deadline: sent + Duration::from_millis((*timeout_ms).min(15_000)),
            };
            return Some(Ok(
                "显示设置正在预览，请在倒计时结束前确认；配置文件尚未修改".into(),
            ));
        }
        let result = match (keep, result) {
            (Some(true), Ok(Response::Handled)) => {
                let saved = self.check_preview_unchanged()
                    .and_then(|_| self.write_and_reload(ipc));
                if let Err(error) = saved {
                    // A failed reload notification after a successful write must
                    // keep the persisted display settings. Only compensate when
                    // the draft never reached disk.
                    if self.disk_content.as_deref() != Some(self.raw_content.as_str()) {
                        return self.rollback_after_save_error(ipc, id.unwrap(), error.to_string());
                    }
                    Err(error)
                } else {
                    Ok("显示设置已确认并保存".into())
                }
            },
            (Some(false), Ok(Response::Handled)) => Ok("已恢复原显示设置，草稿已保留".into()),
            (None, Err(error)) if error.contains("unknown variant") => Err(anyhow::anyhow!(
                "当前桌面会话尚不支持显示预览，请重新登录以使用已安装的新版本。草稿已保留，文件未修改。")),
            (Some(true), Err(error)) => {
                // The acknowledgement may have been lost after the compositor
                // kept the preview. Cancel handles both pending and confirmed
                // tokens on new sessions, without ever writing an uncertain draft.
                return self.rollback_after_save_error(
                    ipc, id.unwrap(), format!("未收到显示确认：{error}"),
                );
            },
            (Some(true), Ok(response)) => {
                return self.rollback_after_save_error(
                    ipc, id.unwrap(), format!("Realand 返回了不支持的显示确认响应：{response:?}"),
                );
            },
            (_, Err(error)) => Err(anyhow::anyhow!(
                "显示操作未完成，文件未写入、草稿已保留：{error}。已开始的未确认预览会由 Realand 在 15 秒内恢复。")),
            _ => Err(anyhow::anyhow!("Realand 返回了不支持的显示预览响应，未写入文件")),
        };
        self.preview = None;
        Some(result)
    }

    fn rollback_after_save_error(
        &mut self,
        ipc: &crate::ipc::IpcClient,
        id: u64,
        error: String,
    ) -> Option<Result<String>> {
        match ipc.request(realand_ipc::Request::CancelOutputPreview { id }) {
            Ok(reply) => {
                self.preview.as_mut().unwrap().phase =
                    crate::output_preview::Phase::RollingBack { reply, error };
                None
            }
            Err(rollback) => {
                self.preview = None;
                Some(Err(anyhow::anyhow!(
                    "{error}。无法发送恢复请求：{rollback}。草稿已保留；请核对当前显示状态。"
                )))
            }
        }
    }

    fn write_and_reload(&mut self, ipc: &crate::ipc::IpcClient) -> Result<()> {
        // Validate IPC path encoding before making irreversible disk changes.
        let reload_action = crate::ipc::config_reload_action(&self.root_path)?;
        self.check_disk_unchanged()?;

        // Ensure parent directory exists
        if let Some(parent) = self.config_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Backup existing config
        if self.config_path.exists() {
            let backup_path = self.config_path.with_extension("kdl.bak");
            crate::storage::atomic_write_backup(
                &backup_path,
                read_config_file(&self.config_path)?.as_bytes(),
                fs::metadata(&self.config_path)?.permissions(),
            )
            .context("无法写入配置备份")?;
        }

        let write = crate::storage::atomic_write(&self.config_path, self.raw_content.as_bytes());
        if let Err(error) = &write {
            if error
                .downcast_ref::<crate::storage::CommittedWriteError>()
                .is_none()
            {
                return write;
            }
        }
        // The rename already committed, even if directory fsync failed. Retain
        // the matching runtime configuration and report the durability error.
        self.is_dirty = false;
        self.disk_content = Some(self.raw_content.clone());
        let reload = ipc
            .perform_action(reload_action)
            .context("配置文件已保存，但无法通知 Realand 重新载入");
        write?;
        reload?;

        Ok(())
    }

    pub fn reload_from_disk(&mut self) -> Result<()> {
        anyhow::ensure!(self.preview.is_none(), "请先确认或恢复显示设置");
        let content = read_config_file(&self.config_path)
            .with_context(|| format!("Failed to reload config from {:?}", self.config_path))?;
        self.disk_content = Some(content.clone());
        self.raw_content = content;
        self.visual_settings = Self::parse_visual_settings(&self.raw_content);
        self.visual_saved = self.visual_settings.clone();
        self.is_dirty = false;
        self.validate();
        Ok(())
    }

    pub fn root_path(&self) -> &Path {
        &self.root_path
    }

    pub fn included_paths(&self) -> Vec<PathBuf> {
        let root = if self.config_path == self.root_path {
            self.raw_content.clone()
        } else {
            read_config_file(&self.root_path).unwrap_or_default()
        };
        let mut paths = Config::parse(&self.root_path, &root).includes;
        paths.sort();
        paths.dedup();
        paths
    }

    pub fn open_file(&mut self, path: &Path) -> Result<()> {
        anyhow::ensure!(self.preview.is_none(), "请先确认或恢复显示设置");
        anyhow::ensure!(
            !self.is_dirty(),
            "当前文件有未保存修改，请先保存，或在直写页面重新载入以放弃修改"
        );
        let path = std::path::absolute(path)?;
        let content = match read_config_file(&path) {
            Ok(content) => Some(content),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        self.config_path = path;
        self.disk_content = content.clone();
        self.raw_content = content.unwrap_or_default();
        self.visual_settings = Self::parse_visual_settings(&self.raw_content);
        self.visual_saved = self.visual_settings.clone();
        self.is_dirty = false;
        self.validate();
        Ok(())
    }

    fn check_disk_unchanged(&self) -> Result<()> {
        let current = match read_config_file(&self.config_path) {
            Ok(content) => Some(content),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        anyhow::ensure!(
            current == self.disk_content,
            "文件已被其他程序修改，未覆盖磁盘内容。请先复制草稿，再重新载入并合并修改。"
        );
        Ok(())
    }

    pub fn restore_backup(&mut self) -> Result<()> {
        anyhow::ensure!(self.preview.is_none(), "请先确认或恢复显示设置");
        anyhow::ensure!(
            !self.is_dirty(),
            "当前配置有未保存修改，请先保存，或重新从磁盘载入以放弃修改后再还原备份"
        );
        let backup_path = self.config_path.with_extension("kdl.bak");
        if backup_path.exists() {
            self.raw_content = read_config_file(&backup_path)?;
            self.visual_settings = Self::parse_visual_settings(&self.raw_content);
            self.visual_saved = self.visual_settings.clone();
            self.is_dirty = true;
            self.validate();
        } else {
            anyhow::bail!("No backup file found at {:?}", backup_path);
        }
        Ok(())
    }

    // Apply changes from visual settings into raw KDL content
    #[cfg(test)]
    fn apply_visual_settings_to_raw(&mut self) {
        self.try_apply_visual_settings_to_raw()
            .expect("visual fixture must produce a valid candidate");
    }

    pub fn try_apply_visual_settings_to_raw(&mut self) -> Result<()> {
        let sources = self.effective_source_contents()?;
        let before_config = self.effective_config()?;
        let before = before_config.animations;
        let settings = &self.visual_settings;
        let baseline = &self.visual_saved;
        let touchpad_changed = baseline.touchpad_tap != settings.touchpad_tap
            || baseline.touchpad_natural_scroll != settings.touchpad_natural_scroll
            || baseline.touchpad_accel_speed != settings.touchpad_accel_speed;
        let (wobble_changed, cube_changed, glide_changed) = classic_changes(baseline, settings);
        let mut seeded = self.raw_content.clone();
        if wobble_changed || cube_changed || glide_changed {
            let mut edit = Editor::new(&seeded)?;
            for (changed, parent) in [
                (wobble_changed, "window-wobble"),
                (cube_changed, "workspace-switch"),
                (glide_changed, "window-open"),
                (glide_changed, "window-close"),
            ] {
                if !changed || edit.exists(&["animations", parent]) {
                    continue;
                }
                // Animation parents replace earlier parents as a whole. Before
                // creating a partial local override, copy the winning source.
                if let Some(block) = inherited_animation_parent(parent, &before, &sources)? {
                    edit.upsert(&["animations"], parent, &block)?;
                }
            }
            seeded = edit.finish();
        }
        if touchpad_changed {
            let mut edit = Editor::new(&seeded)?;
            if !edit.exists(&["input", "touchpad"]) {
                // Unlike KeyboardPart, a touchpad block replaces the whole
                // inherited group. Keep all effective source fields verbatim.
                if let Some(block) =
                    inherited_touchpad_parent(&before_config.input.touchpad, &sources)?
                {
                    edit.upsert(&["input"], "touchpad", &block)?;
                }
            }
            seeded = edit.finish();
        }
        let content = Self::visual_candidate(&seeded, settings, baseline);
        let content = update_keyboard_repeat(&content, baseline, settings)?;
        let content = if baseline.touchpad_accel_speed != settings.touchpad_accel_speed {
            anyhow::ensure!(
                (-1.0..=1.0).contains(&settings.touchpad_accel_speed),
                "触控板跟踪速度必须在 -1 到 1 之间，草稿已保留"
            );
            update_input_number(
                &content,
                "touchpad",
                "accel-speed",
                &settings.touchpad_accel_speed.to_string(),
            )?
        } else {
            content
        };
        let after_config = self.merged_config(&content)?;
        if touchpad_changed {
            let mut desired = before_config.input.touchpad.clone();
            if baseline.touchpad_tap != settings.touchpad_tap {
                desired.tap = settings.touchpad_tap;
            }
            if baseline.touchpad_natural_scroll != settings.touchpad_natural_scroll {
                desired.natural_scroll = settings.touchpad_natural_scroll;
            }
            if baseline.touchpad_accel_speed != settings.touchpad_accel_speed {
                desired.accel_speed.0 = config_number(settings.touchpad_accel_speed);
            }
            anyhow::ensure!(
                after_config.input.touchpad == desired,
                "触控板设置被后续配置覆盖或来源已改变，草稿已保留；请在全部配置检查载入顺序与来源"
            );
        }
        // Keyboard parts merge individual fields. Check both repeat settings so
        // editing one cannot silently reset the other or lose to a later include.
        let delay_changed = baseline.keyboard_repeat_delay != settings.keyboard_repeat_delay;
        let rate_changed = baseline.keyboard_repeat_rate != settings.keyboard_repeat_rate;
        if delay_changed || rate_changed {
            let desired_delay = if delay_changed {
                settings.keyboard_repeat_delay
            } else {
                before_config.input.keyboard.repeat_delay
            };
            let desired_rate = if rate_changed {
                settings.keyboard_repeat_rate
            } else {
                before_config.input.keyboard.repeat_rate
            };
            anyhow::ensure!(
                after_config.input.keyboard.repeat_delay == desired_delay
                    && after_config.input.keyboard.repeat_rate == desired_rate,
                "键盘重复设置被后续配置覆盖，草稿已保留；请在全部配置检查载入顺序与来源"
            );
        }
        let after = after_config.animations;
        let desired = desired_classic_animations(&before, baseline, settings);
        anyhow::ensure!(
            !wobble_changed || after.window_wobble == desired.window_wobble,
            "弹性窗口设置被后续配置覆盖，草稿已保留；请在全部配置检查载入顺序与来源"
        );
        anyhow::ensure!(
            !cube_changed || after.workspace_switch == desired.workspace_switch,
            "立方体设置被后续配置覆盖，草稿已保留；请在全部配置检查载入顺序与来源"
        );
        anyhow::ensure!(
            !glide_changed
                || (after.window_open == desired.window_open
                    && after.window_close == desired.window_close),
            "Glide 设置被后续配置覆盖，草稿已保留；请在全部配置检查载入顺序与来源"
        );
        if baseline.animations_enabled != settings.animations_enabled {
            if let Some(enabled) = settings.animations_enabled {
                anyhow::ensure!(after.off != enabled, "动画开关被后续配置覆盖，草稿已保留");
            }
        }
        if baseline.animation_slowdown != settings.animation_slowdown {
            if let Some(slowdown) = settings.animation_slowdown {
                anyhow::ensure!(
                    after.slowdown == config_number(slowdown),
                    "动画时长被后续配置覆盖，草稿已保留"
                );
            }
        }
        anyhow::ensure!(
            self.effective_source_contents()? == sources,
            "包含文件在应用设置时改变，原草稿已保留，请重新载入后合并修改"
        );
        self.set_raw_content(content);
        self.visual_settings.motion_preset = None;
        Ok(())
    }

    fn visual_candidate(
        source: &str,
        settings: &VisualSettings,
        baseline: &VisualSettings,
    ) -> String {
        // A visual page edits one field at a time. Materializing every UI default
        // here would overwrite inherited settings and round untouched f64 values.
        let previous = Self::parse_visual_settings(source);
        let content = Self::edit(source, |edit| {
            for (path, name, before, after) in [
                (&["layout"][..], "gaps", previous.gaps, settings.gaps),
                (
                    &["layout", "border"][..],
                    "width",
                    previous.border_width,
                    settings.border_width,
                ),
                (
                    &["layout", "focus-ring"][..],
                    "width",
                    previous.focus_ring_width,
                    settings.focus_ring_width,
                ),
            ] {
                if before != after {
                    edit.upsert(path, name, &format!("{name} {after}"))?;
                }
            }
            if previous.center_focused_column != settings.center_focused_column {
                edit.upsert(
                    &["layout"],
                    "center-focused-column",
                    &format!(
                        "center-focused-column {}",
                        serde_json::to_string(&settings.center_focused_column)?,
                    ),
                )?;
            }
            for (parent, name, before, after) in [
                (
                    &["layout"][..],
                    "always-center-single-column",
                    previous.always_center_single_column,
                    settings.always_center_single_column,
                ),
                (
                    &["input", "touchpad"][..],
                    "tap",
                    baseline.touchpad_tap,
                    settings.touchpad_tap,
                ),
                (
                    &["input", "touchpad"][..],
                    "natural-scroll",
                    baseline.touchpad_natural_scroll,
                    settings.touchpad_natural_scroll,
                ),
                (
                    &["input", "keyboard"][..],
                    "numlock",
                    previous.keyboard_numlock,
                    settings.keyboard_numlock,
                ),
            ] {
                if before != after {
                    if after {
                        edit.upsert(parent, name, name)?;
                    } else {
                        let mut path = parent.to_vec();
                        path.push(name);
                        edit.remove(&path)?;
                    }
                }
            }
            Ok(())
        });
        let content = Self::update_hot_corners(&content, settings);
        Self::update_classic_effects_from(&content, settings, baseline)
    }

    fn update_classic_effects(content: &str, settings: &VisualSettings) -> String {
        Self::update_classic_effects_from(content, settings, &Self::parse_visual_settings(content))
    }

    fn update_classic_effects_from(
        content: &str,
        settings: &VisualSettings,
        baseline: &VisualSettings,
    ) -> String {
        // Update only changed fields: replacing whole blocks loses comments, precision,
        // supported values the UI cannot decode, and independent open/close settings.
        let previous = baseline;
        Self::edit(content, |edit| {
            if previous.animations_enabled != settings.animations_enabled {
                edit.remove(&["animations", "off"])?;
                edit.remove(&["animations", "on"])?;
                if let Some(enabled) = settings.animations_enabled {
                    let name = if enabled { "on" } else { "off" };
                    edit.upsert(&["animations"], name, name)?;
                }
            }
            if previous.animation_slowdown != settings.animation_slowdown {
                if let Some(slowdown) = settings.animation_slowdown {
                    edit.upsert(&["animations"], "slowdown", &format!("slowdown {slowdown}"))?;
                } else {
                    edit.remove(&["animations", "slowdown"])?;
                }
            }
            fn value<T: PartialEq + std::fmt::Display>(
                edit: &mut Editor,
                path: &[&str],
                name: &str,
                before: T,
                after: T,
            ) -> Result<()> {
                if before != after {
                    edit.upsert(path, name, &format!("{name} {after}"))?;
                }
                Ok(())
            }
            let force_effects = matches!(
                settings.motion_preset,
                Some(MotionPreset::Fluid | MotionPreset::Lightweight)
            );
            let enabled =
                |edit: &mut Editor, path: &[&str], before: bool, after: bool| -> Result<()> {
                    if force_effects || before != after {
                        let mut opposite = path.to_vec();
                        opposite.push(if after { "off" } else { "on" });
                        edit.remove(&opposite)?;
                        let name = if after { "on" } else { "off" };
                        edit.upsert(path, name, name)?;
                    }
                    Ok(())
                };

            let cube = &["animations", "workspace-switch", "cube"];
            enabled(
                edit,
                cube,
                previous.workspace_cube_enabled,
                settings.workspace_cube_enabled,
            )?;
            value(
                edit,
                cube,
                "perspective",
                previous.cube_perspective,
                settings.cube_perspective,
            )?;
            if previous.cube_background_color != settings.cube_background_color {
                let color = settings
                    .cube_background_color
                    .map(|component| (component.clamp(0., 1.) * 255.).round() as u8);
                edit.upsert(
                    cube,
                    "background-color",
                    &format!(
                        "background-color \"#{:02x}{:02x}{:02x}{:02x}\"",
                        color[0], color[1], color[2], color[3],
                    ),
                )?;
            }

            for parent in ["window-open", "window-close"] {
                let path = &["animations", parent, "glide"];
                enabled(
                    edit,
                    path,
                    previous.window_glide_enabled,
                    settings.window_glide_enabled,
                )?;
                value(
                    edit,
                    path,
                    "tilt-deg",
                    previous.glide_tilt_deg,
                    settings.glide_tilt_deg,
                )?;
                value(
                    edit,
                    path,
                    "depth",
                    previous.glide_depth,
                    settings.glide_depth,
                )?;
            }

            let wobble = &["animations", "window-wobble"];
            enabled(
                edit,
                wobble,
                previous.window_wobble_enabled,
                settings.window_wobble_enabled,
            )?;
            value(
                edit,
                wobble,
                "mesh",
                previous.wobble_mesh,
                settings.wobble_mesh,
            )?;
            value(
                edit,
                wobble,
                "stiffness",
                previous.wobble_stiffness,
                settings.wobble_stiffness,
            )?;
            value(
                edit,
                wobble,
                "damping",
                previous.wobble_damping,
                settings.wobble_damping,
            )?;
            value(
                edit,
                wobble,
                "mass",
                previous.wobble_mass,
                settings.wobble_mass,
            )?;
            value(
                edit,
                wobble,
                "intensity",
                previous.wobble_intensity,
                settings.wobble_intensity,
            )?;
            value(
                edit,
                wobble,
                "position-epsilon",
                previous.wobble_position_epsilon,
                settings.wobble_position_epsilon,
            )?;
            value(
                edit,
                wobble,
                "velocity-epsilon",
                previous.wobble_velocity_epsilon,
                settings.wobble_velocity_epsilon,
            )?;
            Ok(())
        })
    }

    pub fn upsert_child_block(
        content: &str,
        parent_path: &[&str],
        child: &str,
        replacement: &str,
    ) -> String {
        Self::edit(content, |edit| edit.upsert(parent_path, child, replacement))
    }

    fn update_hot_corners(content: &str, settings: &VisualSettings) -> String {
        let previous = Self::parse_visual_settings(content);
        let before = [
            previous.hot_corner_top_left,
            previous.hot_corner_top_right,
            previous.hot_corner_bottom_left,
            previous.hot_corner_bottom_right,
        ];
        let after = [
            settings.hot_corner_top_left,
            settings.hot_corner_top_right,
            settings.hot_corner_bottom_left,
            settings.hot_corner_bottom_right,
        ];
        if before == after {
            return content.to_owned();
        }
        Self::edit(content, |edit| {
            let parent = &["gestures", "hot-corners"];
            for (name, enabled) in ["top-left", "top-right", "bottom-left", "bottom-right"]
                .into_iter()
                .zip(after)
            {
                let path = &["gestures", "hot-corners", name];
                if enabled {
                    if !edit.exists(path) {
                        edit.upsert(parent, name, name)?;
                    }
                } else {
                    edit.remove(path)?;
                }
            }
            if after.into_iter().any(|enabled| enabled) {
                edit.remove(&["gestures", "hot-corners", "off"])?;
            } else {
                edit.upsert(parent, "off", "off")?;
            }
            Ok(())
        })
    }

    fn edit(content: &str, operation: impl FnOnce(&mut Editor) -> Result<()>) -> String {
        let Ok(mut edit) = Editor::new(content) else {
            return content.to_owned();
        };
        if operation(&mut edit).is_err() {
            return content.to_owned();
        }
        edit.finish()
    }
    fn path(block: &str) -> Vec<&str> {
        match block {
            "border" | "focus-ring" => vec!["layout", block],
            "touchpad" | "keyboard" | "mouse" => vec!["input", block],
            _ => vec![block],
        }
    }
    fn ensure_block_path(content: &str, path: &[&str]) -> String {
        Self::edit(content, |edit| edit.ensure(path))
    }
    fn replace_or_insert_in_block(
        content: &str,
        block: &str,
        keyword: &str,
        replacement: &str,
    ) -> String {
        Self::edit(content, |edit| {
            edit.upsert(&Self::path(block), keyword, replacement)
        })
    }
    fn set_flag_in_block(
        content: &str,
        block: &str,
        keyword: &str,
        enabled: bool,
        replacement: &str,
    ) -> String {
        if enabled {
            return Self::replace_or_insert_in_block(content, block, keyword, replacement);
        }
        let mut path = Self::path(block);
        path.push(keyword);
        Self::edit(content, |edit| edit.remove(&path))
    }
    fn has_node_in_block(content: &str, block: &str, keyword: &str) -> bool {
        Self::has_node_in_path(content, &Self::path(block), keyword)
    }
    fn flag_in_block(content: &str, block: &str, keyword: &str) -> bool {
        if !Self::has_node_in_block(content, block, keyword) {
            return false;
        }
        match Self::argument_in_path(content, &Self::path(block), keyword)
            .as_deref()
            .map(str::trim)
        {
            None | Some("") => true,
            Some(value) => !matches!(
                value.to_ascii_lowercase().as_str(),
                "false" | "off" | "no" | "0"
            ),
        }
    }
    fn number_in_block(content: &str, block: &str, keyword: &str) -> Option<f32> {
        Self::number_in_path(content, &Self::path(block), keyword)
    }
    fn has_node_in_path(content: &str, path: &[&str], keyword: &str) -> bool {
        let mut path = path.to_vec();
        path.push(keyword);
        Editor::new(content).is_ok_and(|edit| edit.exists(&path))
    }
    fn argument_in_path(content: &str, path: &[&str], keyword: &str) -> Option<String> {
        let mut path = path.to_vec();
        path.push(keyword);
        Editor::new(content).ok()?.argument(&path)
    }
    fn number_in_path(content: &str, path: &[&str], keyword: &str) -> Option<f32> {
        Self::argument_in_path(content, path, keyword)?.parse().ok()
    }
    fn color_in_path(content: &str, path: &[&str], keyword: &str) -> Option<[f32; 4]> {
        let value = Self::argument_in_path(content, path, keyword)?;
        value
            .parse::<realand_config::Color>()
            .ok()
            .map(|color| color.to_array_unpremul())
    }

    fn block_body_range(content: &str, block: &str) -> Option<Range<usize>> {
        Self::block_body_range_path(content, &Self::path(block))
    }
    fn block_body_range_path(content: &str, path: &[&str]) -> Option<Range<usize>> {
        Editor::new(content).ok()?.body(path)
    }
    pub fn insert_snippet_in_block(content: &str, block: &str, snippet: &str) -> String {
        Self::edit(content, |edit| edit.insert(&Self::path(block), snippet))
    }

    fn resolve_effective_settings(config: &Config, settings: &mut VisualSettings) {
        settings.keyboard_repeat_delay = config.input.keyboard.repeat_delay;
        settings.keyboard_repeat_rate = config.input.keyboard.repeat_rate;
        settings.touchpad_tap = config.input.touchpad.tap;
        settings.touchpad_natural_scroll = config.input.touchpad.natural_scroll;
        settings.touchpad_accel_speed = config.input.touchpad.accel_speed.0 as f32;
        let animations = &config.animations;
        let wobble = animations.window_wobble;
        settings.window_wobble_enabled = !wobble.off;
        settings.wobble_mesh = wobble.mesh;
        settings.wobble_stiffness = wobble.stiffness as f32;
        settings.wobble_damping = wobble.damping as f32;
        settings.wobble_mass = wobble.mass as f32;
        settings.wobble_intensity = wobble.intensity as f32;
        settings.wobble_position_epsilon = wobble.position_epsilon as f32;
        settings.wobble_velocity_epsilon = wobble.velocity_epsilon as f32;
        let cube = animations.workspace_switch.1;
        settings.workspace_cube_enabled = !cube.off;
        settings.cube_perspective = cube.perspective as f32;
        settings.cube_background_color = cube.background_color.to_array_unpremul();
        // The shared controls use the opening Glide as their baseline. Changing
        // one field affects that field in both animations, preserving the rest.
        let glide = animations.window_open.glide;
        settings.window_glide_enabled = !glide.off;
        settings.glide_tilt_deg = glide.tilt_deg as f32;
        settings.glide_depth = glide.depth as f32;
    }

    fn parse_visual_settings(content: &str) -> VisualSettings {
        let mut s = VisualSettings::default();

        // Each field reads the same document. Parsing once keeps typing costs
        // proportional to the config size instead of parsing it for every control.
        let editor = Editor::new(content).ok();
        let has_body = |path: &[&str]| editor.as_ref().and_then(|edit| edit.body(path)).is_some();
        let has_node = |parent: &[&str], name: &str| {
            let mut path = parent.to_vec();
            path.push(name);
            editor.as_ref().is_some_and(|edit| edit.exists(&path))
        };
        let argument = |parent: &[&str], name: &str| {
            let mut path = parent.to_vec();
            path.push(name);
            editor.as_ref().and_then(|edit| edit.argument(&path))
        };
        let number =
            |parent: &[&str], name: &str| -> Option<f32> { argument(parent, name)?.parse().ok() };
        let flag = |parent: &[&str], name: &str| {
            if !has_node(parent, name) {
                return false;
            }
            match argument(parent, name).as_deref().map(str::trim) {
                None | Some("") => true,
                Some(value) => !matches!(
                    value.to_ascii_lowercase().as_str(),
                    "false" | "off" | "no" | "0"
                ),
            }
        };

        // Gaps
        s.animations_enabled = if has_node(&["animations"], "on") {
            Some(true)
        } else if has_node(&["animations"], "off") {
            Some(false)
        } else {
            None
        };
        s.animation_slowdown = number(&["animations"], "slowdown");

        if let Some(value) = number(&["layout"], "gaps") {
            s.gaps = value;
        }

        // Center focused column
        if let Some(value) = argument(&["layout"], "center-focused-column") {
            s.center_focused_column = value;
        }

        // Always center single column
        s.always_center_single_column = flag(&["layout"], "always-center-single-column");

        // Border width (inside border { ... })
        if let Some(value) = number(&["layout", "border"], "width") {
            s.border_width = value;
        }

        // Focus ring width (inside focus-ring { ... })
        if let Some(value) = number(&["layout", "focus-ring"], "width") {
            s.focus_ring_width = value;
        }

        // Hot corners
        let path = &["gestures", "hot-corners"];
        if has_body(path) {
            let enabled = !has_node(path, "off");
            s.hot_corner_top_left = enabled && has_node(path, "top-left");
            s.hot_corner_top_right = enabled && has_node(path, "top-right");
            s.hot_corner_bottom_left = enabled && has_node(path, "bottom-left");
            s.hot_corner_bottom_right = enabled && has_node(path, "bottom-right");
        }

        // Touchpad
        s.touchpad_tap = flag(&["input", "touchpad"], "tap");
        s.touchpad_natural_scroll = flag(&["input", "touchpad"], "natural-scroll");
        if let Some(value) = number(&["input", "touchpad"], "accel-speed") {
            s.touchpad_accel_speed = value;
        }

        // Keyboard numlock
        s.keyboard_numlock = flag(&["input", "keyboard"], "numlock");
        if let Some(value) =
            argument(&["input", "keyboard"], "repeat-delay").and_then(|value| value.parse().ok())
        {
            s.keyboard_repeat_delay = value;
        }
        if let Some(value) =
            argument(&["input", "keyboard"], "repeat-rate").and_then(|value| value.parse().ok())
        {
            s.keyboard_repeat_rate = value;
        }

        let cube_path = &["animations", "workspace-switch", "cube"];
        if has_body(cube_path) {
            s.workspace_cube_enabled = !has_node(cube_path, "off") || has_node(cube_path, "on");
            if let Some(value) = number(cube_path, "perspective") {
                s.cube_perspective = value;
            }
            if let Some(value) = argument(cube_path, "background-color")
                .and_then(|value| value.parse::<realand_config::Color>().ok())
                .map(|color| color.to_array_unpremul())
            {
                s.cube_background_color = value;
            }
        }

        let wobble_path = &["animations", "window-wobble"];
        if has_body(wobble_path) {
            s.window_wobble_enabled = !has_node(wobble_path, "off") || has_node(wobble_path, "on");
            if let Some(value) = number(wobble_path, "mesh") {
                s.wobble_mesh = value.round().clamp(2., 32.) as u8;
            }
            if let Some(value) = number(wobble_path, "stiffness") {
                s.wobble_stiffness = value;
            }
            if let Some(value) = number(wobble_path, "damping") {
                s.wobble_damping = value;
            }
            if let Some(value) = number(wobble_path, "mass") {
                s.wobble_mass = value;
            }
            if let Some(value) = number(wobble_path, "intensity") {
                s.wobble_intensity = value;
            }
            if let Some(value) = number(wobble_path, "position-epsilon") {
                s.wobble_position_epsilon = value;
            }
            if let Some(value) = number(wobble_path, "velocity-epsilon") {
                s.wobble_velocity_epsilon = value;
            }
        }

        let open_glide_path = &["animations", "window-open", "glide"];
        let close_glide_path = &["animations", "window-close", "glide"];
        let glide_path = if has_body(open_glide_path) {
            Some(open_glide_path.as_slice())
        } else if has_body(close_glide_path) {
            Some(close_glide_path.as_slice())
        } else {
            None
        };
        if let Some(path) = glide_path {
            s.window_glide_enabled = !has_node(path, "off") || has_node(path, "on");
            if let Some(value) = number(path, "tilt-deg") {
                s.glide_tilt_deg = value;
            }
            if let Some(value) = number(path, "depth") {
                s.glide_depth = value;
            }
        }

        s
    }
}

fn classic_changes(before: &VisualSettings, after: &VisualSettings) -> (bool, bool, bool) {
    let force = matches!(
        after.motion_preset,
        Some(MotionPreset::Fluid | MotionPreset::Lightweight)
    );
    (
        force
            || (
                before.window_wobble_enabled,
                before.wobble_mesh,
                before.wobble_stiffness,
                before.wobble_damping,
                before.wobble_mass,
                before.wobble_intensity,
                before.wobble_position_epsilon,
                before.wobble_velocity_epsilon,
            ) != (
                after.window_wobble_enabled,
                after.wobble_mesh,
                after.wobble_stiffness,
                after.wobble_damping,
                after.wobble_mass,
                after.wobble_intensity,
                after.wobble_position_epsilon,
                after.wobble_velocity_epsilon,
            ),
        force
            || (
                before.workspace_cube_enabled,
                before.cube_perspective,
                before.cube_background_color,
            ) != (
                after.workspace_cube_enabled,
                after.cube_perspective,
                after.cube_background_color,
            ),
        force
            || (
                before.window_glide_enabled,
                before.glide_tilt_deg,
                before.glide_depth,
            ) != (
                after.window_glide_enabled,
                after.glide_tilt_deg,
                after.glide_depth,
            ),
    )
}

fn update_keyboard_repeat(
    source: &str,
    before: &VisualSettings,
    after: &VisualSettings,
) -> Result<String> {
    let mut content = source.to_owned();
    for (name, old, new) in [
        (
            "repeat-delay",
            before.keyboard_repeat_delay,
            after.keyboard_repeat_delay,
        ),
        (
            "repeat-rate",
            u16::from(before.keyboard_repeat_rate),
            u16::from(after.keyboard_repeat_rate),
        ),
    ] {
        if old != new {
            content = update_input_number(&content, "keyboard", name, &new.to_string())?;
        }
    }
    Ok(content)
}

fn update_input_number(source: &str, device: &str, name: &str, value: &str) -> Result<String> {
    use knuffel::{ast::SpannedNode, span::Span};

    // Config validation rejects duplicate local input/device nodes before
    // this edit runs. Includes merge separately; never rewrite their contents.
    fn node_at_path<'a>(
        nodes: &'a [SpannedNode<Span>],
        path: &[&str],
    ) -> Option<&'a SpannedNode<Span>> {
        nodes.iter().find_map(|node| {
            if &**node.node_name != path[0] {
                return None;
            }
            if path.len() == 1 {
                Some(node)
            } else {
                node_at_path(node.children.as_deref()?, &path[1..])
            }
        })
    }

    let mut content = source.to_owned();
    let document = knuffel::parse_ast::<Span>("config.kdl", &content)
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    let path = ["input", device, name];
    if let Some(node) = node_at_path(&document.nodes, &path) {
        let argument = node.arguments.first().context("输入设置缺少数值")?;
        let span = argument.literal.span();
        // Replace only the literal, keeping indentation, comments and KDL
        // separators intact, including a comment following the number.
        content.replace_range(span.0..span.1, value);
    } else {
        let mut child = format!("{name} {value}");
        if let Some(parent) = node_at_path(&document.nodes, &["input", device]).or_else(|| {
            child = format!("{device} {{\n{child}\n}}");
            node_at_path(&document.nodes, &["input"])
        }) {
            if let Some(children) = &parent.children {
                content.insert_str(children.span().1 - 1, &format!("\n{child}\n"));
            } else {
                // A bare device/input node is valid. Add children before its
                // existing separator and leave all surrounding trivia.
                content.insert_str(parent.node_name.span().1, &format!(" {{\n{child}\n}}"));
            }
        } else {
            content.push_str(&format!("\ninput {{\n{child}\n}}\n"));
        }
    }
    Ok(content)
}

fn inherited_touchpad_parent(
    before: &realand_config::input::Touchpad,
    sources: &[(PathBuf, Option<Vec<u8>>)],
) -> Result<Option<String>> {
    let mut found_parent = false;
    for (_, content) in sources {
        let Some(content) = content else { continue };
        let source = std::str::from_utf8(content)?;
        let document = knuffel::parse_ast::<knuffel::span::Span>("config.kdl", source)
            .map_err(|error| anyhow::anyhow!("{error:?}"))?;
        for node in document
            .nodes
            .iter()
            .filter(|node| &**node.node_name == "input")
            .flat_map(|node| node.children())
            .filter(|node| &**node.node_name == "touchpad")
        {
            found_parent = true;
            let span = node.span();
            let block = &source[span.0..span.1];
            let isolated = Config::parse_mem(&format!("input {{\n{block}\n}}"))?;
            if isolated.input.touchpad == *before {
                return Ok(Some(block.to_owned()));
            }
        }
    }
    anyhow::ensure!(
        !found_parent && *before == realand_config::input::Touchpad::default(),
        "触控板的包含来源已改变，原草稿已保留；请重新载入后合并修改"
    );
    Ok(None)
}

fn config_number(value: f32) -> f64 {
    // KDL receives the decimal displayed by the f32 control, not its widened
    // binary approximation. Untouched values never pass through this conversion.
    value
        .to_string()
        .parse()
        .expect("f32 decimal is a valid f64")
}

fn same_animation_parent(
    parent: &str,
    left: &realand_config::animations::Animations,
    right: &realand_config::animations::Animations,
) -> bool {
    match parent {
        "window-wobble" => left.window_wobble == right.window_wobble,
        "workspace-switch" => left.workspace_switch == right.workspace_switch,
        "window-open" => left.window_open == right.window_open,
        "window-close" => left.window_close == right.window_close,
        _ => false,
    }
}

fn inherited_animation_parent(
    parent: &str,
    before: &realand_config::animations::Animations,
    sources: &[(PathBuf, Option<Vec<u8>>)],
) -> Result<Option<String>> {
    let mut found_parent = false;
    for (_, content) in sources {
        let Some(content) = content else {
            continue;
        };
        let content = std::str::from_utf8(content)?;
        for block in crate::appearance_controls::animation_blocks(content, parent)? {
            found_parent = true;
            let isolated = Config::parse_mem(&format!("animations {{\n{block}\n}}"))?.animations;
            if same_animation_parent(parent, &isolated, before) {
                return Ok(Some(block.to_owned()));
            }
        }
    }
    anyhow::ensure!(
        !found_parent,
        "{parent} 的包含来源已改变，原草稿已保留；请重新载入后合并修改"
    );
    Ok(None)
}

fn desired_classic_animations(
    original: &realand_config::animations::Animations,
    before: &VisualSettings,
    after: &VisualSettings,
) -> realand_config::animations::Animations {
    let mut desired = original.clone();
    let force = matches!(
        after.motion_preset,
        Some(MotionPreset::Fluid | MotionPreset::Lightweight)
    );
    let wobble = &mut desired.window_wobble;
    if force || before.window_wobble_enabled != after.window_wobble_enabled {
        wobble.off = !after.window_wobble_enabled;
    }
    if before.wobble_mesh != after.wobble_mesh {
        wobble.mesh = after.wobble_mesh;
    }
    for (value, previous, next) in [
        (
            &mut wobble.stiffness,
            before.wobble_stiffness,
            after.wobble_stiffness,
        ),
        (
            &mut wobble.damping,
            before.wobble_damping,
            after.wobble_damping,
        ),
        (&mut wobble.mass, before.wobble_mass, after.wobble_mass),
        (
            &mut wobble.intensity,
            before.wobble_intensity,
            after.wobble_intensity,
        ),
        (
            &mut wobble.position_epsilon,
            before.wobble_position_epsilon,
            after.wobble_position_epsilon,
        ),
        (
            &mut wobble.velocity_epsilon,
            before.wobble_velocity_epsilon,
            after.wobble_velocity_epsilon,
        ),
    ] {
        if previous != next {
            *value = config_number(next);
        }
    }
    let cube = &mut desired.workspace_switch.1;
    if force || before.workspace_cube_enabled != after.workspace_cube_enabled {
        cube.off = !after.workspace_cube_enabled;
    }
    if before.cube_perspective != after.cube_perspective {
        cube.perspective = config_number(after.cube_perspective);
    }
    if before.cube_background_color != after.cube_background_color {
        let [r, g, b, a] = after
            .cube_background_color
            .map(|value| (value.clamp(0., 1.) * 255.).round() as u8);
        cube.background_color = realand_config::Color::from_rgba8_unpremul(r, g, b, a);
    }
    for glide in [
        &mut desired.window_open.glide,
        &mut desired.window_close.glide,
    ] {
        if force || before.window_glide_enabled != after.window_glide_enabled {
            glide.off = !after.window_glide_enabled;
        }
        if before.glide_tilt_deg != after.glide_tilt_deg {
            glide.tilt_deg = config_number(after.glide_tilt_deg);
        }
        if before.glide_depth != after.glide_depth {
            glide.depth = config_number(after.glide_depth);
        }
    }
    desired
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manager(content: &str) -> ConfigManager {
        ConfigManager {
            config_path: std::env::temp_dir().join("realand-settings-test-config.kdl"),
            root_path: std::env::temp_dir().join("realand-settings-test-config.kdl"),
            disk_content: None,
            raw_content: content.to_owned(),
            validation_status: ValidationStatus::NotChecked,
            visual_settings: ConfigManager::parse_visual_settings(content),
            visual_saved: ConfigManager::parse_visual_settings(content),
            effective_animations: None,
            is_dirty: false,
            validation_due: None,
            preview: None,
        }
    }

    fn preview_manager() -> (tempfile::TempDir, ConfigManager) {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr = manager("output \"test\" { scale 1; }\n");
        mgr.config_path = dir.path().join("config.kdl");
        mgr.root_path = mgr.config_path.clone();
        fs::write(&mgr.config_path, &mgr.raw_content).unwrap();
        mgr.disk_content = Some(mgr.raw_content.clone());
        mgr.set_raw_content("output \"test\" { scale 2; }\n".into());
        (dir, mgr)
    }

    fn inherited_effect_manager(source: &str) -> (tempfile::TempDir, ConfigManager) {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("base.kdl"), source).unwrap();
        let root = dir.path().join("config.kdl");
        fs::write(&root, "include \"base.kdl\"\n// preserve root note\n").unwrap();
        let mgr = ConfigManager::from_path(&root).unwrap();
        (dir, mgr)
    }

    const PRECISE_TOUCHPAD: &str = r#"input {
    touchpad { // keep touchpad settings
        off
        tap
        natural-scroll
        dwt
        dwtp
        drag false
        drag-lock
        click-method "clickfinger"
        accel-speed /* tuned tracking */ 0.123456789123 // preserve note
        accel-profile "flat"
        scroll-method "two-finger"
        scroll-button 274
        scroll-button-lock
        tap-button-map "left-middle-right"
        left-handed
        disabled-on-external-mouse
        middle-emulation
        scroll-factor 1.123456789123 horizontal=1.987654321987 vertical=0.456789123456
    }
}
"#;

    #[test]
    fn touchpad_tracking_browsing_local_and_inherited_values_preserves_source_precision() {
        for inherited in [false, true] {
            let (dir, mut mgr) = inherited_effect_manager(PRECISE_TOUCHPAD);
            if !inherited {
                fs::write(&mgr.config_path, PRECISE_TOUCHPAD).unwrap();
                mgr = ConfigManager::from_path(&mgr.config_path).unwrap();
            }
            let raw = mgr.raw_content().to_owned();
            let before = mgr.effective_config().unwrap().input.touchpad;
            assert_eq!(
                mgr.visual_settings().touchpad_accel_speed,
                before.accel_speed.0 as f32
            );
            assert!(mgr.visual_settings().touchpad_tap);
            assert!(mgr.visual_settings().touchpad_natural_scroll);
            mgr.validate();
            mgr.try_apply_visual_settings_to_raw().unwrap();
            assert_eq!(mgr.raw_content(), raw);
            assert_eq!(mgr.effective_config().unwrap().input.touchpad, before);
            assert!(!mgr.is_dirty());
            assert_eq!(fs::read_to_string(&mgr.config_path).unwrap(), raw);
            assert_eq!(
                fs::read_to_string(dir.path().join("base.kdl")).unwrap(),
                PRECISE_TOUCHPAD
            );
        }
    }

    #[test]
    fn touchpad_tracking_edit_replaces_only_local_speed_literal() {
        let mut mgr = manager(PRECISE_TOUCHPAD);
        mgr.validate();
        let mut expected = mgr.effective_config().unwrap().input.touchpad;
        expected.accel_speed.0 = -0.35;
        mgr.visual_settings_mut().touchpad_accel_speed = -0.35;
        mgr.try_apply_visual_settings_to_raw().unwrap();
        assert_eq!(
            mgr.raw_content(),
            PRECISE_TOUCHPAD.replace("0.123456789123", "-0.35")
        );
        assert_eq!(mgr.effective_config().unwrap().input.touchpad, expected);
    }

    #[test]
    fn touchpad_tracking_inherited_edit_copies_winning_complete_group_without_modifying_sources() {
        let (dir, mut mgr) = inherited_effect_manager(PRECISE_TOUCHPAD);
        // Source snapshots are sorted by path, not include precedence. An
        // earlier, different group must not seed the partial local override.
        let earlier = "input { touchpad { accel-speed -0.9; }; }\n";
        fs::write(dir.path().join("aaa.kdl"), earlier).unwrap();
        let raw = "include \"aaa.kdl\"\ninclude \"base.kdl\"\n// root note\ninput { keyboard { repeat-rate 37; }; }\n";
        fs::write(&mgr.config_path, raw).unwrap();
        mgr = ConfigManager::from_path(&mgr.config_path).unwrap();
        let mut expected = mgr.effective_config().unwrap().input.touchpad;
        expected.accel_speed.0 = 0.4;
        mgr.visual_settings_mut().touchpad_accel_speed = 0.4;
        mgr.try_apply_visual_settings_to_raw().unwrap();
        let after = mgr.effective_config().unwrap();
        assert_eq!(after.input.touchpad, expected);
        assert_eq!(after.input.keyboard.repeat_rate, 37);
        assert!(mgr
            .raw_content()
            .starts_with("include \"aaa.kdl\"\ninclude \"base.kdl\"\n// root note\n"));
        assert!(mgr
            .raw_content()
            .contains("accel-speed /* tuned tracking */ 0.4 // preserve note"));
        assert!(mgr.raw_content().contains(
            "scroll-factor 1.123456789123 horizontal=1.987654321987 vertical=0.456789123456"
        ));
        assert_eq!(fs::read_to_string(&mgr.config_path).unwrap(), raw);
        assert_eq!(
            fs::read_to_string(dir.path().join("base.kdl")).unwrap(),
            PRECISE_TOUCHPAD
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("aaa.kdl")).unwrap(),
            earlier
        );
    }

    #[test]
    fn touchpad_tracking_unchanged_speed_keeps_f64_precision_when_editing_inherited_flags() {
        for tap in [false, true] {
            let (_dir, mut mgr) = inherited_effect_manager(PRECISE_TOUCHPAD);
            let mut expected = mgr.effective_config().unwrap().input.touchpad;
            if tap {
                mgr.visual_settings_mut().touchpad_tap = false;
                expected.tap = false;
            } else {
                mgr.visual_settings_mut().touchpad_natural_scroll = false;
                expected.natural_scroll = false;
            }
            mgr.try_apply_visual_settings_to_raw().unwrap();
            assert_eq!(mgr.effective_config().unwrap().input.touchpad, expected);
            assert!(mgr
                .raw_content()
                .contains("accel-speed /* tuned tracking */ 0.123456789123 // preserve note"));
        }
    }

    #[test]
    fn touchpad_tracking_later_include_conflict_preserves_raw_and_visual_drafts() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("config.kdl");
        let raw = "input { touchpad { tap; accel-speed 0.1; }; }\ninclude \"later.kdl\"\n";
        fs::write(&root, raw).unwrap();
        fs::write(dir.path().join("later.kdl"), PRECISE_TOUCHPAD).unwrap();
        let mut mgr = ConfigManager::from_path(&root).unwrap();
        let draft = format!("{raw}// unsaved raw note\n");
        mgr.set_raw_content(draft.clone());
        mgr.visual_settings_mut().touchpad_accel_speed = 0.5;
        let visual = mgr.visual_settings().clone();
        // A source also changes externally while the visual draft is pending.
        let external = PRECISE_TOUCHPAD.replace("0.123456789123", "-0.6");
        fs::write(dir.path().join("later.kdl"), &external).unwrap();
        mgr.validate();
        assert_eq!(mgr.visual_settings(), &visual);
        assert!(mgr
            .try_apply_visual_settings_to_raw()
            .unwrap_err()
            .to_string()
            .contains("覆盖"));
        assert_eq!(mgr.raw_content(), draft);
        assert_eq!(mgr.visual_settings(), &visual);
        assert!(mgr.visual_is_dirty());
        assert_eq!(fs::read_to_string(&root).unwrap(), raw);
        assert_eq!(
            fs::read_to_string(dir.path().join("later.kdl")).unwrap(),
            external
        );
    }

    #[test]
    fn touchpad_tracking_validation_refreshes_effective_values_without_discarding_visual_draft() {
        let (dir, mut mgr) = inherited_effect_manager(PRECISE_TOUCHPAD);
        let updated = "input { touchpad { natural-scroll; accel-speed -0.5; }; }\n";
        fs::write(dir.path().join("base.kdl"), updated).unwrap();
        mgr.validate();
        assert_eq!(mgr.visual_settings().touchpad_accel_speed, -0.5);
        assert!(!mgr.visual_settings().touchpad_tap);
        assert!(mgr.visual_settings().touchpad_natural_scroll);
        assert!(!mgr.visual_is_dirty());
        mgr.visual_settings_mut().touchpad_accel_speed = 0.75;
        let visual = mgr.visual_settings().clone();
        fs::write(dir.path().join("base.kdl"), PRECISE_TOUCHPAD).unwrap();
        mgr.validate();
        assert_eq!(mgr.visual_settings(), &visual);
        mgr.try_apply_visual_settings_to_raw().unwrap();
        let mut expected = Config::parse_mem(PRECISE_TOUCHPAD).unwrap().input.touchpad;
        expected.accel_speed.0 = 0.75;
        assert_eq!(mgr.effective_config().unwrap().input.touchpad, expected);
        assert!(mgr.visual_settings().touchpad_tap);
    }

    #[test]
    fn touchpad_tracking_invalid_duplicates_or_nonfinite_speed_preserve_drafts() {
        for raw in [
            "input { touchpad {}; }\ninput { touchpad {}; }\n",
            "input { touchpad {}; touchpad {}; }\n",
            "input { touchpad { accel-speed 0.1; accel-speed 0.2; }; }\n",
        ] {
            let mut mgr = manager(raw);
            mgr.visual_settings_mut().touchpad_accel_speed = -0.1;
            let visual = mgr.visual_settings().clone();
            assert!(mgr.try_apply_visual_settings_to_raw().is_err());
            assert_eq!(mgr.raw_content(), raw);
            assert_eq!(mgr.visual_settings(), &visual);
        }
        for speed in [f32::NAN, f32::INFINITY, -1.1, 1.1] {
            let mut mgr = manager(PRECISE_TOUCHPAD);
            mgr.visual_settings_mut().touchpad_accel_speed = speed;
            assert!(mgr.try_apply_visual_settings_to_raw().is_err());
            assert_eq!(mgr.raw_content(), PRECISE_TOUCHPAD);
            assert_eq!(
                mgr.visual_settings().touchpad_accel_speed.to_bits(),
                speed.to_bits()
            );
        }
    }

    #[test]
    fn touchpad_tracking_mismatched_source_snapshot_cannot_reset_inherited_properties() {
        let before = Config::parse_mem(PRECISE_TOUCHPAD).unwrap().input.touchpad;
        let sources = [(
            PathBuf::from("base.kdl"),
            Some(b"input { touchpad { tap; }; }\n".to_vec()),
        )];
        assert!(inherited_touchpad_parent(&before, &sources).is_err());
        assert!(inherited_touchpad_parent(&before, &[]).is_err());
        assert_eq!(
            inherited_touchpad_parent(&realand_config::input::Touchpad::default(), &[]).unwrap(),
            None
        );
    }

    #[test]
    fn keyboard_repeat_browsing_defaults_local_and_inherited_values_never_writes() {
        let defaults = realand_config::input::Keyboard::default();
        assert_eq!(
            VisualSettings::default().keyboard_repeat_delay,
            defaults.repeat_delay
        );
        assert_eq!(
            VisualSettings::default().keyboard_repeat_rate,
            defaults.repeat_rate
        );
        for (raw, delay, rate) in [
            ("// default input\n", 600, 25),
            (
                "input { keyboard { repeat-delay 900; repeat-rate 45; }; }\n",
                900,
                45,
            ),
            ("include \"base.kdl\"\n// root note\n", 4500, 0),
            (
                "include \"base.kdl\"\ninput { keyboard { repeat-rate 255; }; }\n",
                4500,
                255,
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let base = "input { keyboard { repeat-delay 4500; repeat-rate 0; }; }\n";
            fs::write(dir.path().join("base.kdl"), base).unwrap();
            let root = dir.path().join("config.kdl");
            fs::write(&root, raw).unwrap();
            let mut mgr = ConfigManager::from_path(&root).unwrap();
            assert_eq!(mgr.visual_settings().keyboard_repeat_delay, delay);
            assert_eq!(mgr.visual_settings().keyboard_repeat_rate, rate);
            mgr.validate();
            mgr.try_apply_visual_settings_to_raw().unwrap();
            assert_eq!(mgr.raw_content(), raw);
            assert_eq!(fs::read_to_string(&root).unwrap(), raw);
            assert_eq!(
                fs::read_to_string(dir.path().join("base.kdl")).unwrap(),
                base
            );
            assert!(!root.with_extension("kdl.bak").exists());
            assert!(!mgr.is_dirty());
        }
    }

    #[test]
    fn keyboard_repeat_edits_only_changed_field_preserving_inherited_keyboard() {
        let base = "input { keyboard { xkb { layout \"us,de\"; }; numlock; repeat-delay 4500; repeat-rate 0; }; }\n";
        for change_delay in [false, true] {
            let (dir, mut mgr) = inherited_effect_manager(base);
            if change_delay {
                mgr.visual_settings_mut().keyboard_repeat_delay = 65535;
            } else {
                mgr.visual_settings_mut().keyboard_repeat_rate = 40;
            }
            mgr.try_apply_visual_settings_to_raw().unwrap();
            let config = mgr.effective_config().unwrap();
            assert_eq!(
                config.input.keyboard.repeat_delay,
                if change_delay { 65535 } else { 4500 }
            );
            assert_eq!(
                config.input.keyboard.repeat_rate,
                if change_delay { 0 } else { 40 }
            );
            assert!(config.input.keyboard.numlock);
            assert_eq!(config.input.keyboard.xkb.layout, "us,de");
            assert!(!mgr.raw_content().contains(if change_delay {
                "repeat-rate"
            } else {
                "repeat-delay"
            }));
            assert!(mgr
                .raw_content()
                .starts_with("include \"base.kdl\"\n// preserve root note\n"));
            assert_eq!(
                fs::read_to_string(dir.path().join("base.kdl")).unwrap(),
                base
            );
            assert_eq!(
                fs::read_to_string(&mgr.config_path).unwrap(),
                "include \"base.kdl\"\n// preserve root note\n"
            );
        }
    }

    #[test]
    fn keyboard_repeat_edits_only_the_literal_preserving_comments_and_slashdash_examples() {
        let raw = "/- input { keyboard { repeat-rate 99; }; }\ninput { keyboard { repeat-delay 800; repeat-rate /* explanation */ 0; // no repeat\nxkb { layout \"us\"; }; }; }\n";
        let mut mgr = manager(raw);
        mgr.validate();
        assert_eq!(mgr.visual_settings().keyboard_repeat_rate, 0);
        mgr.visual_settings_mut().keyboard_repeat_rate = 33;
        mgr.try_apply_visual_settings_to_raw().unwrap();
        assert_eq!(
            mgr.raw_content(),
            raw.replace("/* explanation */ 0", "/* explanation */ 33")
        );
        assert_eq!(
            mgr.effective_config().unwrap().input.keyboard.repeat_delay,
            800
        );
        // A new field goes into the active block, leaving disabled examples.
        let raw = "/- input { keyboard { repeat-rate 99; }; }\ninput { keyboard { numlock; xkb { layout \"us\"; }; }; }\n";
        let mut mgr = manager(raw);
        mgr.validate();
        mgr.visual_settings_mut().keyboard_repeat_rate = 0;
        mgr.try_apply_visual_settings_to_raw().unwrap();
        assert!(mgr.raw_content().starts_with("/- input { keyboard { repeat-rate 99; }; }\ninput { keyboard { numlock; xkb { layout \"us\"; }; }".trim_end_matches('}')));
        assert_eq!(
            mgr.effective_config().unwrap().input.keyboard.repeat_rate,
            0
        );
    }

    #[test]
    fn keyboard_repeat_invalid_duplicate_sections_are_rejected_without_changing_drafts() {
        for raw in [
            "input { keyboard { repeat-rate 10; }; }\ninput { keyboard { repeat-rate 20; }; }\n",
            "input { keyboard { repeat-rate 10; }; keyboard { repeat-rate 20; }; }\n",
            "input { keyboard { repeat-rate 10; repeat-rate 20; }; }\n",
        ] {
            let mut mgr = manager(raw);
            mgr.visual_settings_mut().keyboard_repeat_rate = 0;
            let visual = mgr.visual_settings().clone();
            assert!(mgr.try_apply_visual_settings_to_raw().is_err());
            assert_eq!(mgr.raw_content(), raw);
            assert_eq!(mgr.visual_settings(), &visual);
            assert!(mgr.visual_is_dirty());
        }
    }

    #[test]
    fn keyboard_repeat_later_include_rejects_edits_and_preserves_both_drafts() {
        for change_delay in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let later = "input { keyboard { repeat-rate 50; repeat-delay 5000; }; }\n";
            fs::write(dir.path().join("later.kdl"), later).unwrap();
            let root = dir.path().join("config.kdl");
            let disk = "input { keyboard { repeat-rate 30; repeat-delay 700; }; }\ninclude \"later.kdl\"\n";
            fs::write(&root, disk).unwrap();
            let mut mgr = ConfigManager::from_path(&root).unwrap();
            let raw = format!("{disk}// pending raw draft\n");
            mgr.set_raw_content(raw.clone());
            assert_eq!(mgr.visual_settings().keyboard_repeat_delay, 5000);
            assert_eq!(mgr.visual_settings().keyboard_repeat_rate, 50);
            if change_delay {
                mgr.visual_settings_mut().keyboard_repeat_delay = 800;
            } else {
                mgr.visual_settings_mut().keyboard_repeat_rate = 0;
            }
            let visual = mgr.visual_settings().clone();
            mgr.validate();
            assert_eq!(mgr.visual_settings(), &visual);
            assert!(mgr
                .try_apply_visual_settings_to_raw()
                .unwrap_err()
                .to_string()
                .contains("覆盖"));
            assert_eq!(mgr.raw_content(), raw);
            assert_eq!(mgr.visual_settings(), &visual);
            assert!(mgr.visual_is_dirty());
            assert_eq!(fs::read_to_string(&root).unwrap(), disk);
            assert_eq!(
                fs::read_to_string(dir.path().join("later.kdl")).unwrap(),
                later
            );
        }
    }

    #[test]
    fn keyboard_repeat_validation_refreshes_effective_values_only_without_visual_draft() {
        let (dir, mut mgr) = inherited_effect_manager("input { keyboard { repeat-rate 20; }; }\n");
        fs::write(
            dir.path().join("base.kdl"),
            "input { keyboard { repeat-rate 0; repeat-delay 65535; }; }\n",
        )
        .unwrap();
        mgr.validate();
        assert_eq!(mgr.visual_settings().keyboard_repeat_delay, 65535);
        assert_eq!(mgr.visual_settings().keyboard_repeat_rate, 0);
        assert!(!mgr.visual_is_dirty());
        mgr.visual_settings_mut().keyboard_repeat_rate = 255;
        let visual = mgr.visual_settings().clone();
        fs::write(
            dir.path().join("base.kdl"),
            "input { keyboard { repeat-rate 10; repeat-delay 60000; }; }\n",
        )
        .unwrap();
        mgr.validate();
        assert_eq!(mgr.visual_settings(), &visual);
        mgr.try_apply_visual_settings_to_raw().unwrap();
        assert_eq!(mgr.visual_settings().keyboard_repeat_delay, 60000);
        assert_eq!(mgr.visual_settings().keyboard_repeat_rate, 255);
        assert!(!mgr.raw_content().contains("repeat-delay"));
    }

    #[test]
    fn keyboard_repeat_handles_empty_parent_blocks_without_losing_trivia() {
        for raw in [
            "input; // keep input\n",
            "input { keyboard; // keep keyboard\n}\n",
            "input { keyboard {}; }\n",
        ] {
            let mut mgr = manager(raw);
            mgr.validate();
            mgr.visual_settings_mut().keyboard_repeat_delay = 65535;
            mgr.visual_settings_mut().keyboard_repeat_rate = 0;
            mgr.try_apply_visual_settings_to_raw().unwrap();
            let keyboard = mgr.effective_config().unwrap().input.keyboard;
            assert_eq!(keyboard.repeat_delay, 65535);
            assert_eq!(keyboard.repeat_rate, 0);
            for comment in ["// keep input", "// keep keyboard"] {
                assert_eq!(mgr.raw_content().contains(comment), raw.contains(comment));
            }
        }
    }

    #[test]
    fn appearance_inherited_wobble_controls_show_effective_values() {
        let (_dir, mgr) = inherited_effect_manager(
            "animations { window-wobble { off; stiffness 333; damping 250; mass 50; }; }\n",
        );
        let effective = mgr.effective_config().unwrap().animations.window_wobble;
        assert_eq!(mgr.visual_settings.window_wobble_enabled, !effective.off);
        assert_eq!(
            mgr.visual_settings.wobble_stiffness as f64,
            effective.stiffness
        );
        assert_eq!(mgr.visual_settings.wobble_damping as f64, effective.damping);
        assert_eq!(mgr.visual_settings.wobble_mass as f64, effective.mass);
        assert!(!mgr.visual_is_dirty());
    }

    #[test]
    fn appearance_inherited_wobble_edit_preserves_disabled_state_and_unedited_parameters() {
        let (_dir, mut mgr) = inherited_effect_manager("animations { window-wobble { off; stiffness 333.123456789; damping 250; mass 50.123456789; intensity 2.123456789; position-epsilon 10.123456789; velocity-epsilon 50.123456789; }; }\n");
        let before = mgr.effective_config().unwrap().animations.window_wobble;
        mgr.visual_settings_mut().wobble_damping = 20.;
        mgr.apply_visual_settings_to_raw();
        let after = mgr.effective_config().unwrap().animations.window_wobble;
        assert_eq!(after.off, before.off);
        assert_eq!(after.stiffness, before.stiffness);
        assert_eq!(after.mass, before.mass);
        assert_eq!(after.intensity, before.intensity);
        assert_eq!(after.position_epsilon, before.position_epsilon);
        assert_eq!(after.velocity_epsilon, before.velocity_epsilon);
        assert_eq!(after.damping, 20.);
        assert!(mgr.raw_content().contains("// preserve root note"));
    }

    #[test]
    fn appearance_inherited_cube_and_glide_edits_preserve_animation_timing_shader_and_popin() {
        let source = "animations { workspace-switch { duration-ms 875; curve \"cubic-bezier\" 0.17 0.31 0.42 0.93; cube { off; perspective 0.01; background-color \"red\"; }; }; window-open { duration-ms 731; curve \"linear\"; custom-shader \"open shader\"; popin-ratio 0.07123456789; glide { tilt-deg 17.123456789; depth 0.23456789; }; }; window-close { spring damping-ratio=0.8 stiffness=731 epsilon=0.00001; custom-shader \"close shader\"; effect \"fade\"; glide { off; tilt-deg 31.123456789; depth 0.456789123; }; }; }\n";
        let (_dir, mut mgr) = inherited_effect_manager(source);
        let before = mgr.effective_config().unwrap().animations;
        mgr.visual_settings_mut().cube_perspective = 0.4;
        mgr.visual_settings_mut().glide_depth = 0.25;
        mgr.apply_visual_settings_to_raw();
        let after = mgr.effective_config().unwrap().animations;
        assert_eq!(after.workspace_switch.0, before.workspace_switch.0);
        assert_eq!(after.workspace_switch.1.off, before.workspace_switch.1.off);
        assert_eq!(
            after.workspace_switch.1.background_color,
            before.workspace_switch.1.background_color
        );
        assert_eq!(after.workspace_switch.1.perspective, 0.4);
        assert_eq!(after.window_open.anim, before.window_open.anim);
        assert_eq!(
            after.window_open.custom_shader,
            before.window_open.custom_shader
        );
        assert_eq!(
            after.window_open.popin_ratio,
            before.window_open.popin_ratio
        );
        assert_eq!(
            after.window_open.glide.tilt_deg,
            before.window_open.glide.tilt_deg
        );
        assert_eq!(after.window_close.anim, before.window_close.anim);
        assert_eq!(
            after.window_close.custom_shader,
            before.window_close.custom_shader
        );
        assert_eq!(after.window_close.effect, before.window_close.effect);
        assert_eq!(
            after.window_close.glide.tilt_deg,
            before.window_close.glide.tilt_deg
        );
        assert_eq!(after.window_close.glide.off, before.window_close.glide.off);
        assert_eq!(after.window_open.glide.depth, 0.25);
        assert_eq!(after.window_close.glide.depth, 0.25);
    }

    #[test]
    fn appearance_later_include_rejects_visual_changes_and_preserves_both_drafts() {
        for parent in [
            "window-wobble",
            "workspace-switch",
            "window-open",
            "animations",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let source = match parent {
                "window-wobble" => "animations { window-wobble { off; stiffness 333; damping 250; mass 50; }; }\n",
                "workspace-switch" => "animations { workspace-switch { duration-ms 875; cube { off; perspective 0.1; }; }; }\n",
                "window-open" => "animations { window-open { duration-ms 731; custom-shader \"custom\"; glide { depth 0.1; }; }; }\n",
                _ => "animations { slowdown 3; }\n",
            };
            fs::write(dir.path().join("later.kdl"), source).unwrap();
            let root = dir.path().join("config.kdl");
            let raw = "// root note\ninclude \"later.kdl\"\n";
            fs::write(&root, raw).unwrap();
            let mut mgr = ConfigManager::from_path(&root).unwrap();
            // Put an earlier local animation before the include so the include
            // truly wins. Newly appended parents would intentionally override it.
            let raw = format!("{source}{raw}");
            mgr.set_raw_content(raw.clone());
            match parent {
                "window-wobble" => mgr.visual_settings_mut().wobble_damping = 20.,
                "workspace-switch" => mgr.visual_settings_mut().cube_perspective = 0.4,
                "window-open" => mgr.visual_settings_mut().glide_depth = 0.25,
                _ => mgr.visual_settings_mut().animation_slowdown = Some(1.),
            }
            let visual = mgr.visual_settings.clone();
            assert!(mgr
                .try_apply_visual_settings_to_raw()
                .unwrap_err()
                .to_string()
                .contains("覆盖"));
            assert_eq!(mgr.raw_content(), raw);
            assert_eq!(mgr.visual_settings(), &visual);
            assert!(mgr.visual_is_dirty());
            assert_eq!(
                fs::read_to_string(&root).unwrap(),
                "// root note\ninclude \"later.kdl\"\n"
            );
        }
    }

    #[test]
    fn appearance_unmodified_default_and_inherited_values_never_materialize_parent_blocks() {
        for source in ["", "include \"base.kdl\"\n"] {
            let dir = tempfile::tempdir().unwrap();
            fs::write(
                dir.path().join("base.kdl"),
                "animations { window-wobble { off; mass 50; }; }\n",
            )
            .unwrap();
            let root = dir.path().join("config.kdl");
            fs::write(&root, source).unwrap();
            let mut mgr = ConfigManager::from_path(&root).unwrap();
            mgr.try_apply_visual_settings_to_raw().unwrap();
            assert_eq!(mgr.raw_content(), source);
            assert!(!mgr.visual_is_dirty());
            assert!(!mgr.is_dirty());
        }
    }

    #[test]
    fn appearance_validation_refreshes_cached_effects_without_discarding_visual_preset() {
        let (dir, mut mgr) =
            inherited_effect_manager("animations { window-wobble { mass 50; }; }\n");
        fs::write(
            dir.path().join("base.kdl"),
            "animations { window-wobble { off; mass 75; }; }\n",
        )
        .unwrap();
        mgr.validate();
        assert_eq!(mgr.visual_settings.wobble_mass, 75.);
        assert!(!mgr.visual_settings.window_wobble_enabled);
        assert!(!mgr.visual_is_dirty());
        mgr.visual_settings_mut()
            .apply_motion_preset(MotionPreset::Fluid);
        let visual = mgr.visual_settings.clone();
        mgr.validate();
        assert_eq!(mgr.visual_settings(), &visual);
        assert!(mgr.visual_is_dirty());
    }

    #[test]
    fn appearance_mismatched_source_snapshot_rejects_partial_animation_instead_of_resetting_it() {
        let before = Config::parse_mem("").unwrap().animations;
        let source =
            "animations { window-open { duration-ms 731; custom-shader \"new source\"; }; }\n";
        let sources = [(PathBuf::from("base.kdl"), Some(source.as_bytes().to_vec()))];
        assert!(inherited_animation_parent("window-open", &before, &sources).is_err());
    }

    #[test]
    fn appearance_inherited_block_copy_preserves_comments_and_non_byte_color() {
        let source = "animations { workspace-switch { /* tuned timing */ duration-ms 875; cube { /* precise color */ background-color \"rgba(12.345% 23.456% 34.567% / 0.456789)\"; perspective 0.65; }; }; }\n";
        let (_dir, mut mgr) = inherited_effect_manager(source);
        let before = mgr.effective_config().unwrap().animations.workspace_switch;
        mgr.visual_settings_mut().cube_perspective = 0.4;
        mgr.try_apply_visual_settings_to_raw().unwrap();
        let after = mgr.effective_config().unwrap().animations.workspace_switch;
        assert_eq!(after.0, before.0);
        assert_eq!(after.1.background_color, before.1.background_color);
        assert!(mgr.raw_content().contains("/* tuned timing */"));
        assert!(mgr.raw_content().contains("/* precise color */"));
        assert!(mgr
            .raw_content()
            .contains("rgba(12.345% 23.456% 34.567% / 0.456789)"));
    }

    fn preview_ipc() -> crate::ipc::IpcClient {
        crate::ipc::IpcClient::scripted_for_test(|request| {
            Ok(match request {
                realand_ipc::Request::PreviewOutputConfig { .. } => {
                    realand_ipc::Response::OutputPreview {
                        id: 42,
                        timeout_ms: 15_000,
                    }
                }
                _ => realand_ipc::Response::Handled,
            })
        })
    }

    fn tracking_preview_ipc(
        confirm: std::result::Result<(), String>,
        cancel: std::result::Result<(), String>,
    ) -> (
        crate::ipc::IpcClient,
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
    ) {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        let cancellations = Arc::new(AtomicUsize::new(0));
        let calls = cancellations.clone();
        let ipc = crate::ipc::IpcClient::scripted_for_test(move |request| {
            use realand_ipc::{Request, Response};
            match request {
                Request::PreviewOutputConfig { .. } => Ok(Response::OutputPreview {
                    id: 42,
                    timeout_ms: 15_000,
                }),
                Request::ConfirmOutputPreview { id } => {
                    assert_eq!(id, 42);
                    confirm.clone().map(|_| Response::Handled)
                }
                Request::CancelOutputPreview { id } => {
                    assert_eq!(id, 42);
                    calls.fetch_add(1, Ordering::Relaxed);
                    cancel.clone().map(|_| Response::Handled)
                }
                _ => Ok(Response::Handled),
            }
        });
        (ipc, cancellations)
    }

    fn poll(mgr: &mut ConfigManager, ipc: &crate::ipc::IpcClient) -> Result<String> {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(result) = mgr.poll_preview(ipc) {
                return result;
            }
            assert!(Instant::now() < deadline, "preview worker did not respond");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn motion_presets_preserve_custom_curves_and_unrelated_settings() {
        let original = "layout { gaps 8.123456789; }\nanimations { /* keep curve */ window-open { duration-ms 431; curve \"linear\"; }; window-wobble { mass 0.12345; }; }\n";
        for preset in [
            MotionPreset::Fluid,
            MotionPreset::Lightweight,
            MotionPreset::Reduced,
        ] {
            let mut mgr = manager(original);
            mgr.visual_settings = ConfigManager::parse_visual_settings(original);
            mgr.visual_settings.apply_motion_preset(preset);
            mgr.apply_visual_settings_to_raw();
            let config = Config::parse_mem(mgr.raw_content()).unwrap();
            assert_eq!(config.layout.gaps, 8.123456789);
            assert!(mgr.raw_content().contains("/* keep curve */"));
            assert!(mgr.raw_content().contains("duration-ms 431"));
            assert_eq!(config.animations.window_wobble.mass, 0.12345);
            assert_eq!(
                config.animations.off,
                matches!(preset, MotionPreset::Reduced)
            );
            if !matches!(preset, MotionPreset::Reduced) {
                assert_eq!(
                    config.animations.window_wobble.off,
                    matches!(preset, MotionPreset::Lightweight)
                );
                assert_eq!(
                    config.animations.workspace_switch.1.off,
                    matches!(preset, MotionPreset::Lightweight)
                );
            }
            let saved = mgr.raw_content().to_owned();
            mgr.apply_visual_settings_to_raw();
            assert_eq!(mgr.raw_content(), saved);
        }
    }

    #[test]
    fn motion_controls_can_restore_inheritance_without_losing_effects() {
        let original = "animations { off; slowdown 1.123456789; window-wobble { mass 0.125; }; }";
        let mut mgr = manager(original);
        mgr.visual_settings = ConfigManager::parse_visual_settings(original);
        mgr.apply_visual_settings_to_raw();
        assert_eq!(mgr.raw_content(), original);
        mgr.visual_settings.animations_enabled = Some(true);
        mgr.apply_visual_settings_to_raw();
        assert!(!Config::parse_mem(mgr.raw_content()).unwrap().animations.off);
        assert!(mgr.raw_content().contains("slowdown 1.123456789"));
        mgr.visual_settings.animations_enabled = None;
        mgr.visual_settings.animation_slowdown = None;
        mgr.apply_visual_settings_to_raw();
        let editor = Editor::new(mgr.raw_content()).unwrap();
        assert!(!editor.exists(&["animations", "off"]));
        assert!(!editor.exists(&["animations", "on"]));
        assert!(!editor.exists(&["animations", "slowdown"]));
        assert_eq!(
            Config::parse_mem(mgr.raw_content())
                .unwrap()
                .animations
                .window_wobble
                .mass,
            0.125
        );
    }

    #[test]
    fn fluid_preset_explicitly_overrides_disabled_inherited_effects() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("config.kdl");
        fs::write(dir.path().join("base.kdl"), "animations { off; window-wobble { off; }; workspace-switch { cube { off; }; }; window-open { glide { off; }; }; window-close { glide { off; }; }; }").unwrap();
        fs::write(&root, "include \"base.kdl\"\n").unwrap();
        let mut mgr = ConfigManager::from_path(&root).unwrap();
        mgr.visual_settings.apply_motion_preset(MotionPreset::Fluid);
        mgr.apply_visual_settings_to_raw();
        let animations = Config::parse(&root, mgr.raw_content())
            .config
            .unwrap()
            .animations;
        assert!(!animations.off);
        assert!(!animations.window_wobble.off);
        assert!(!animations.workspace_switch.1.off);
        assert!(!animations.window_open.glide.off);
        assert!(!animations.window_close.glide.off);
    }

    #[test]
    fn display_preview_only_writes_after_confirmation() {
        let (_dir, mut mgr) = preview_manager();
        let ipc = preview_ipc();
        let before = fs::read(&mgr.config_path).unwrap();
        mgr.save_and_reload(&ipc).unwrap();
        poll(&mut mgr, &ipc).unwrap();
        assert!(mgr.preview_pending() && mgr.is_dirty());
        assert_eq!(fs::read(&mgr.config_path).unwrap(), before);
        assert!(!mgr.config_path.with_extension("kdl.bak").exists());
        assert!(mgr.save_and_reload(&ipc).is_err());
        assert!(mgr.reload_from_disk().is_err());
        mgr.finish_preview(&ipc, true).unwrap();
        poll(&mut mgr, &ipc).unwrap();
        assert!(!mgr.preview_pending() && !mgr.is_dirty());
        assert_eq!(
            fs::read_to_string(&mgr.config_path).unwrap(),
            mgr.raw_content
        );
        assert_eq!(
            fs::read(mgr.config_path.with_extension("kdl.bak")).unwrap(),
            before
        );
    }

    #[test]
    fn confirmed_display_backup_failure_restores_runtime_and_keeps_draft() {
        let (_dir, mut mgr) = preview_manager();
        let (ipc, cancellations) = tracking_preview_ipc(Ok(()), Ok(()));
        let before = fs::read(&mgr.config_path).unwrap();
        let draft = mgr.raw_content.clone();
        mgr.save_and_reload(&ipc).unwrap();
        poll(&mut mgr, &ipc).unwrap();
        fs::create_dir(mgr.config_path.with_extension("kdl.bak")).unwrap();
        mgr.finish_preview(&ipc, true).unwrap();
        let error = poll(&mut mgr, &ipc).unwrap_err().to_string();
        assert!(error.contains("已恢复原显示设置"), "{error}");
        assert_eq!(cancellations.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert_eq!(fs::read(&mgr.config_path).unwrap(), before);
        assert_eq!(mgr.raw_content, draft);
        assert!(mgr.is_dirty() && !mgr.preview_pending());
    }

    #[test]
    fn external_edit_after_confirm_request_triggers_compensation() {
        let (_dir, mut mgr) = preview_manager();
        let (ipc, cancellations) = tracking_preview_ipc(Ok(()), Ok(()));
        mgr.save_and_reload(&ipc).unwrap();
        poll(&mut mgr, &ipc).unwrap();
        mgr.finish_preview(&ipc, true).unwrap();
        fs::write(&mgr.config_path, "// external edit after confirmation\n").unwrap();
        assert!(poll(&mut mgr, &ipc)
            .unwrap_err()
            .to_string()
            .contains("已恢复"));
        assert_eq!(cancellations.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert_eq!(
            fs::read_to_string(&mgr.config_path).unwrap(),
            "// external edit after confirmation\n"
        );
        assert!(mgr.is_dirty() && !mgr.preview_pending());
    }

    #[test]
    fn lost_confirm_ack_is_cancelled_before_any_write() {
        let (_dir, mut mgr) = preview_manager();
        let (ipc, cancellations) = tracking_preview_ipc(Err("timeout".into()), Ok(()));
        let before = fs::read(&mgr.config_path).unwrap();
        mgr.save_and_reload(&ipc).unwrap();
        poll(&mut mgr, &ipc).unwrap();
        mgr.finish_preview(&ipc, true).unwrap();
        assert!(poll(&mut mgr, &ipc)
            .unwrap_err()
            .to_string()
            .contains("已恢复"));
        assert_eq!(cancellations.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert_eq!(fs::read(&mgr.config_path).unwrap(), before);
        assert!(!mgr.config_path.with_extension("kdl.bak").exists());
        assert!(mgr.is_dirty() && !mgr.preview_pending());
    }

    #[test]
    fn failed_compensation_reports_uncertain_runtime_without_writing() {
        let (_dir, mut mgr) = preview_manager();
        let (ipc, cancellations) =
            tracking_preview_ipc(Err("timeout".into()), Err("stale token".into()));
        let before = fs::read(&mgr.config_path).unwrap();
        mgr.save_and_reload(&ipc).unwrap();
        poll(&mut mgr, &ipc).unwrap();
        mgr.finish_preview(&ipc, true).unwrap();
        let error = poll(&mut mgr, &ipc).unwrap_err().to_string();
        assert!(error.contains("恢复请求未获确认"), "{error}");
        assert!(!error.contains("已恢复原显示设置"));
        assert_eq!(cancellations.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert_eq!(fs::read(&mgr.config_path).unwrap(), before);
        assert!(mgr.is_dirty() && !mgr.preview_pending());
    }

    #[test]
    fn unexpected_confirmation_response_is_compensated_without_writing() {
        let (_dir, mut mgr) = preview_manager();
        let before = fs::read(&mgr.config_path).unwrap();
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let calls = cancelled.clone();
        let ipc = crate::ipc::IpcClient::scripted_for_test(move |request| {
            use realand_ipc::{Request, Response};
            match request {
                Request::PreviewOutputConfig { .. } => Ok(Response::OutputPreview {
                    id: 42,
                    timeout_ms: 15_000,
                }),
                Request::ConfirmOutputPreview { .. } => Ok(Response::Version("unexpected".into())),
                Request::CancelOutputPreview { id } => {
                    assert_eq!(id, 42);
                    calls.store(true, std::sync::atomic::Ordering::Relaxed);
                    Ok(Response::Handled)
                }
                _ => Ok(Response::Handled),
            }
        });
        mgr.save_and_reload(&ipc).unwrap();
        poll(&mut mgr, &ipc).unwrap();
        mgr.finish_preview(&ipc, true).unwrap();
        let error = poll(&mut mgr, &ipc).unwrap_err().to_string();
        assert!(
            error.contains("不支持的显示确认响应") && error.contains("已恢复"),
            "{error}"
        );
        assert!(cancelled.load(std::sync::atomic::Ordering::Relaxed));
        assert_eq!(fs::read(&mgr.config_path).unwrap(), before);
        assert!(mgr.is_dirty() && !mgr.preview_pending());
    }

    #[test]
    fn reload_notification_failure_after_save_keeps_confirmed_display() {
        let (_dir, mut mgr) = preview_manager();
        let (ipc, cancellations) = tracking_preview_ipc(Ok(()), Ok(()));
        mgr.save_and_reload(&ipc).unwrap();
        poll(&mut mgr, &ipc).unwrap();
        mgr.finish_preview(&ipc, true).unwrap();
        let error = poll(&mut mgr, &crate::ipc::IpcClient::disconnected_for_test())
            .unwrap_err()
            .to_string();
        assert!(error.contains("已保存，但无法通知"), "{error}");
        assert_eq!(cancellations.load(std::sync::atomic::Ordering::Relaxed), 0);
        assert_eq!(
            fs::read_to_string(&mgr.config_path).unwrap(),
            mgr.raw_content
        );
        assert!(!mgr.is_dirty() && !mgr.preview_pending());
    }

    #[test]
    fn cancelling_display_preview_preserves_file_and_draft() {
        let (_dir, mut mgr) = preview_manager();
        let ipc = preview_ipc();
        let draft = mgr.raw_content.clone();
        let before = fs::read(&mgr.config_path).unwrap();
        mgr.save_and_reload(&ipc).unwrap();
        poll(&mut mgr, &ipc).unwrap();
        mgr.finish_preview(&ipc, false).unwrap();
        poll(&mut mgr, &ipc).unwrap();
        assert!(!mgr.preview_pending() && mgr.is_dirty());
        assert_eq!(fs::read(&mgr.config_path).unwrap(), before);
        assert_eq!(mgr.raw_content, draft);
    }

    #[test]
    fn unsupported_compositor_cannot_persist_display_draft() {
        let (_dir, mut mgr) = preview_manager();
        let ipc = crate::ipc::IpcClient::scripted_for_test(|_| Err("unknown variant".into()));
        let before = fs::read(&mgr.config_path).unwrap();
        mgr.save_and_reload(&ipc).unwrap();
        assert!(poll(&mut mgr, &ipc).is_err());
        assert!(!mgr.preview_pending() && mgr.is_dirty());
        assert_eq!(fs::read(&mgr.config_path).unwrap(), before);
    }

    #[test]
    fn display_confirmation_never_overwrites_external_edit() {
        let (_dir, mut mgr) = preview_manager();
        let ipc = preview_ipc();
        mgr.save_and_reload(&ipc).unwrap();
        poll(&mut mgr, &ipc).unwrap();
        fs::write(&mgr.config_path, "// external edit\n").unwrap();
        assert!(mgr.finish_preview(&ipc, true).is_err());
        poll(&mut mgr, &ipc).unwrap();
        assert_eq!(
            fs::read_to_string(&mgr.config_path).unwrap(),
            "// external edit\n"
        );
        assert!(mgr.is_dirty());
    }

    #[test]
    fn display_confirmation_checks_new_nested_include() {
        let (dir, mut mgr) = preview_manager();
        let root = dir.path().join("root.kdl");
        let nested = dir.path().join("nested.kdl");
        fs::write(&root, "include \"config.kdl\"\n").unwrap();
        fs::write(&nested, "// original\n").unwrap();
        mgr.root_path = root;
        mgr.set_raw_content("output \"test\" { scale 2; }\ninclude \"nested.kdl\"\n".into());
        let ipc = preview_ipc();
        mgr.save_and_reload(&ipc).unwrap();
        poll(&mut mgr, &ipc).unwrap();
        fs::write(nested, "// external edit\n").unwrap();
        assert!(mgr.finish_preview(&ipc, true).is_err());
        poll(&mut mgr, &ipc).unwrap();
        assert_eq!(
            fs::read_to_string(&mgr.config_path).unwrap(),
            mgr.disk_content.clone().unwrap()
        );
    }

    #[test]
    fn display_preview_accepts_missing_optional_includes() {
        let (dir, mut mgr) = preview_manager();
        let missing = dir.path().join("optional.kdl");
        mgr.set_raw_content(
            "output \"test\" { scale 2; }\ninclude \"optional.kdl\" optional=true\n".into(),
        );
        let ipc = preview_ipc();
        mgr.save_and_reload(&ipc).unwrap();
        poll(&mut mgr, &ipc).unwrap();
        assert!(mgr
            .preview
            .as_ref()
            .unwrap()
            .dependencies
            .contains(&(missing, None)));
        mgr.finish_preview(&ipc, true).unwrap();
        poll(&mut mgr, &ipc).unwrap();
        assert_eq!(
            fs::read_to_string(mgr.config_path()).unwrap(),
            mgr.raw_content()
        );
        assert!(!mgr.is_dirty());
    }

    #[test]
    fn display_confirmation_checks_optional_include_creation_and_deletion() {
        for initially_present in [false, true] {
            let (dir, mut mgr) = preview_manager();
            let optional = dir.path().join("optional.kdl");
            if initially_present {
                fs::write(&optional, "// original\n").unwrap();
            }
            mgr.set_raw_content(
                "output \"test\" { scale 2; }\ninclude \"optional.kdl\" optional=true\n".into(),
            );
            let ipc = preview_ipc();
            mgr.save_and_reload(&ipc).unwrap();
            poll(&mut mgr, &ipc).unwrap();
            if initially_present {
                fs::remove_file(&optional).unwrap();
            } else {
                fs::write(&optional, "output \"test\" { scale 3; }\n").unwrap();
            }
            assert!(mgr.finish_preview(&ipc, true).is_err());
            poll(&mut mgr, &ipc).unwrap();
            assert_eq!(
                fs::read_to_string(mgr.config_path()).unwrap(),
                mgr.disk_content.clone().unwrap()
            );
            assert!(mgr.is_dirty());
        }
    }

    #[test]
    fn expired_display_preview_is_cancelled_without_writing() {
        let (_dir, mut mgr) = preview_manager();
        let ipc = preview_ipc();
        mgr.save_and_reload(&ipc).unwrap();
        poll(&mut mgr, &ipc).unwrap();
        mgr.preview.as_mut().unwrap().phase = crate::output_preview::Phase::Ready {
            id: 42,
            deadline: Instant::now(),
        };
        assert!(mgr.finish_preview(&ipc, true).is_err());
        poll(&mut mgr, &ipc).unwrap();
        assert_eq!(
            fs::read_to_string(&mgr.config_path).unwrap(),
            mgr.disk_content.clone().unwrap()
        );
        assert!(mgr.is_dirty());
    }

    #[test]
    fn disconnected_worker_cannot_leave_expired_preview_blocking_editor() {
        let (_dir, mut mgr) = preview_manager();
        let ipc = preview_ipc();
        mgr.save_and_reload(&ipc).unwrap();
        poll(&mut mgr, &ipc).unwrap();
        mgr.preview.as_mut().unwrap().phase = crate::output_preview::Phase::Ready {
            id: 42,
            deadline: Instant::now(),
        };
        assert!(mgr
            .poll_preview(&crate::ipc::IpcClient::disconnected_for_test())
            .unwrap()
            .is_err());
        assert!(!mgr.preview_pending() && mgr.is_dirty());
    }

    #[test]
    fn lost_confirmation_response_does_not_write_configuration() {
        let (_dir, mut mgr) = preview_manager();
        let ipc = crate::ipc::IpcClient::scripted_for_test(|request| match request {
            realand_ipc::Request::PreviewOutputConfig { .. } => {
                Ok(realand_ipc::Response::OutputPreview {
                    id: 42,
                    timeout_ms: 15_000,
                })
            }
            _ => Err("timeout".into()),
        });
        mgr.save_and_reload(&ipc).unwrap();
        poll(&mut mgr, &ipc).unwrap();
        mgr.finish_preview(&ipc, true).unwrap();
        assert!(poll(&mut mgr, &ipc).is_err());
        assert_eq!(
            fs::read_to_string(&mgr.config_path).unwrap(),
            mgr.disk_content.clone().unwrap()
        );
        assert!(mgr.is_dirty());
    }

    #[test]
    fn unrelated_and_comment_edits_do_not_require_display_preview() {
        let (_dir, mut mgr) = preview_manager();
        mgr.set_raw_content(
            "// comment\noutput \"test\" { scale 1; }\nlayout { gaps 8; }\n".into(),
        );
        assert!(!mgr.outputs_changed().unwrap());
        mgr.save_and_reload(&preview_ipc()).unwrap();
        assert!(!mgr.preview_pending());
        assert!(!mgr.is_dirty());
    }

    #[test]
    fn returning_to_saved_content_clears_dirty_state() {
        let saved = "layout { gaps 8; }\n";
        let mut mgr = manager(saved);
        mgr.disk_content = Some(saved.into());
        mgr.set_raw_content("layout { gaps 16; }\n".into());
        assert!(mgr.is_dirty());
        mgr.set_raw_content(saved.into());
        assert!(!mgr.is_dirty());
    }

    #[test]
    fn restoring_backup_never_discards_unsaved_drafts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.kdl");
        let saved = "layout { gaps 8; }\n";
        let backup = "layout { gaps 4; }\n";
        fs::write(&path, saved).unwrap();
        fs::write(path.with_extension("kdl.bak"), backup).unwrap();
        let mut mgr = manager("");
        mgr.root_path = path.clone();
        mgr.open_file(&path).unwrap();

        let draft = "layout { gaps 16; }\n";
        mgr.set_raw_content(draft.into());
        assert!(mgr.restore_backup().is_err());
        assert_eq!(mgr.raw_content(), draft);
        assert_eq!(fs::read_to_string(&path).unwrap(), saved);

        mgr.set_raw_content(saved.into());
        mgr.visual_settings_mut().gaps = 24.;
        assert!(mgr.restore_backup().is_err());
        assert_eq!(mgr.visual_settings().gaps, 24.);

        mgr.reload_from_disk().unwrap();
        mgr.restore_backup().unwrap();
        assert_eq!(mgr.raw_content(), backup);
        assert!(mgr.is_dirty());
    }
    #[test]
    fn included_drafts_validate_against_root_without_touching_disk() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("config.kdl");
        let child = dir.path().join("child.kdl");
        let root_text = "include \"child.kdl\"\nrenderer \"vulkan\"\n";
        fs::write(&root, root_text).unwrap();
        fs::write(&child, "layout { gaps 8; }\n").unwrap();
        let mut mgr = manager("");
        mgr.root_path = root.clone();
        mgr.open_file(&root).unwrap();
        assert_eq!(mgr.included_paths(), vec![child.clone()]);
        mgr.open_file(&child).unwrap();
        mgr.set_raw_content("layout { gaps 12; }\n".into());
        mgr.validate();
        assert_eq!(mgr.validation_status(), &ValidationStatus::Valid);
        assert!(
            mgr.open_file(&root).is_err(),
            "must not discard unsaved edits"
        );
        mgr.set_raw_content("layout { gaps \"invalid\"; }\n".into());
        mgr.validate();
        assert!(matches!(
            mgr.validation_status(),
            ValidationStatus::Error(_)
        ));
        assert_eq!(fs::read_to_string(&child).unwrap(), "layout { gaps 8; }\n");
        assert_eq!(fs::read_to_string(&root).unwrap(), root_text);
    }

    #[test]
    fn relative_missing_include_draft_is_validated_with_absolute_override() {
        let cwd = std::env::current_dir().unwrap();
        let dir = tempfile::tempdir_in(&cwd).unwrap();
        let root = dir.path().join("config.kdl");
        let child = dir.path().join("optional.kdl");
        fs::write(&root, "include \"optional.kdl\" optional=true\n").unwrap();
        let mut mgr = manager("");
        mgr.root_path = root.clone();
        mgr.open_file(&root).unwrap();
        mgr.open_file(child.strip_prefix(&cwd).unwrap()).unwrap();
        assert!(mgr.config_path().is_absolute());
        mgr.set_raw_content("layout { gaps \"invalid\"; }\n".into());
        mgr.validate();
        assert!(matches!(
            mgr.validation_status(),
            ValidationStatus::Error(_)
        ));
        assert!(!child.exists());
    }

    #[test]
    fn config_save_does_not_follow_backup_symlink_or_expose_private_content() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.kdl");
        let backup = path.with_extension("kdl.bak");
        let unrelated = dir.path().join("unrelated");
        let saved = "layout { gaps 8; }\n";
        fs::write(&path, saved).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&unrelated, "keep me").unwrap();
        symlink(&unrelated, &backup).unwrap();
        let mut mgr = manager("");
        mgr.root_path = path.clone();
        mgr.open_file(&path).unwrap();
        mgr.set_raw_content("layout { gaps 12; }\n".into());
        mgr.save_and_reload(&preview_ipc()).unwrap();
        assert_eq!(fs::read_to_string(unrelated).unwrap(), "keep me");
        assert!(!backup.is_symlink());
        assert_eq!(fs::read_to_string(&backup).unwrap(), saved);
        assert_eq!(
            fs::metadata(backup).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(!mgr.is_dirty());
    }

    #[test]
    fn non_utf8_reload_path_is_rejected_before_configuration_is_saved() {
        use std::os::unix::ffi::OsStringExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join(std::ffi::OsString::from_vec(b"config-\xff.kdl".to_vec()));
        let saved = "layout { gaps 8; }\n";
        fs::write(&path, saved).unwrap();
        let mut mgr = ConfigManager::from_path(&path).unwrap();
        mgr.set_raw_content("layout { gaps 12; }\n".into());
        assert!(mgr.save_and_reload(&preview_ipc()).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), saved);
        assert!(!path.with_extension("kdl.bak").exists());
        assert!(mgr.is_dirty());
    }

    #[test]
    fn special_config_files_fail_without_blocking_or_discarding_drafts() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.kdl");
        fs::write(&path, "layout { gaps 8; }\n").unwrap();
        let mut mgr = ConfigManager::from_path(&path).unwrap();
        mgr.set_raw_content("layout { gaps 12; }\n".into());
        fs::remove_file(&path).unwrap();
        let fifo = CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        let start = Instant::now();
        assert!(ConfigManager::from_path(&path).is_err());
        assert!(mgr.reload_from_disk().is_err());
        assert!(mgr.check_disk_unchanged().is_err());
        assert!(mgr.open_file(&path).is_err());
        assert!(start.elapsed() < Duration::from_secs(1));
        assert_eq!(mgr.raw_content(), "layout { gaps 12; }\n");
        assert!(mgr.is_dirty());
    }

    #[test]
    fn external_edits_and_deleted_files_cannot_be_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.kdl");
        fs::write(&path, "layout {}\n").unwrap();
        let mut mgr = manager("");
        mgr.root_path = path.clone();
        mgr.open_file(&path).unwrap();
        mgr.set_raw_content("layout { gaps 20; }\n".into());
        assert!(mgr.check_disk_unchanged().is_ok());
        fs::write(&path, "layout { gaps 30; }\n").unwrap();
        assert!(mgr.check_disk_unchanged().is_err());
        mgr.reload_from_disk().unwrap();
        assert!(mgr.check_disk_unchanged().is_ok());
        fs::remove_file(&path).unwrap();
        assert!(mgr.check_disk_unchanged().is_err());
    }

    #[test]
    fn unchanged_visual_save_preserves_source_and_compositor_defaults() {
        let original = "// All defaults are inherited from the compositor.\n";
        let mut mgr = manager(original);
        mgr.visual_settings = ConfigManager::parse_visual_settings(original);
        mgr.apply_visual_settings_to_raw();
        assert_eq!(mgr.raw_content(), original);
        let defaults = Config::parse_mem(original).unwrap();
        assert_eq!(
            mgr.visual_settings.border_width as f64,
            defaults.layout.border.width
        );
        assert_eq!(
            mgr.visual_settings.focus_ring_width as f64,
            defaults.layout.focus_ring.width
        );
    }

    #[test]
    fn changing_gaps_preserves_included_settings() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("config.kdl");
        fs::write(
            dir.path().join("shared.kdl"),
            "layout { border { width 6; }; focus-ring { width 7; }; }\ninput { touchpad { tap; natural-scroll; }; keyboard { numlock; }; }\ngestures { hot-corners { top-left; }; }\n",
        )
        .unwrap();
        let original = "include \"shared.kdl\"\n";
        fs::write(&root, original).unwrap();
        let mut mgr = ConfigManager::from_path(&root).unwrap();
        mgr.visual_settings.gaps = 24.;
        mgr.apply_visual_settings_to_raw();
        let parsed = Config::parse(&root, mgr.raw_content()).config.unwrap();
        assert_eq!(parsed.layout.gaps, 24.);
        assert_eq!(parsed.layout.border.width, 6.);
        assert_eq!(parsed.layout.focus_ring.width, 7.);
        assert!(parsed.input.touchpad.tap);
        assert!(parsed.input.touchpad.natural_scroll);
        assert!(parsed.input.keyboard.numlock);
        assert!(parsed.gestures.hot_corners.top_left);
        assert!(!mgr.raw_content().contains("input"));
        assert!(!mgr.raw_content().contains("hot-corners"));
    }

    #[test]
    fn changing_input_preserves_untouched_precision_and_hot_corner_comments() {
        let original = "layout { gaps 8.123456789; border { width 2.123456789; }; focus-ring { width 3.123456789; }; }\ngestures { hot-corners { /* keep this explanation */ top-left; }; }\n";
        let mut mgr = manager(original);
        mgr.visual_settings = ConfigManager::parse_visual_settings(original);
        mgr.visual_settings.touchpad_tap = true;
        mgr.apply_visual_settings_to_raw();
        assert!(mgr.raw_content().starts_with(original));
        assert!(
            Config::parse_mem(mgr.raw_content())
                .unwrap()
                .input
                .touchpad
                .tap
        );
    }

    #[test]
    fn hot_corner_edit_preserves_comments_and_does_not_enable_disabled_siblings() {
        let original = "gestures { hot-corners { off; /* keep explanation */ top-left; }; }\n";
        let mut settings = ConfigManager::parse_visual_settings(original);
        settings.hot_corner_top_right = true;
        let updated = ConfigManager::update_hot_corners(original, &settings);
        let parsed = Config::parse_mem(&updated).unwrap().gestures.hot_corners;
        assert!(!parsed.off);
        assert!(!parsed.top_left);
        assert!(parsed.top_right);
        assert!(updated.contains("/* keep explanation */"));
    }

    #[test]
    fn visual_updates_preserve_fractional_dimensions() {
        let mut mgr =
            manager("layout { gaps 8.75; border { width 1.5; }; focus-ring { width 2.25; }; }\n");
        mgr.visual_settings = ConfigManager::parse_visual_settings(mgr.raw_content());
        mgr.apply_visual_settings_to_raw();
        let parsed = Config::parse_mem(mgr.raw_content()).unwrap();
        assert_eq!(parsed.layout.gaps, 8.75);
        assert_eq!(parsed.layout.border.width, 1.5);
        assert_eq!(parsed.layout.focus_ring.width, 2.25);
    }

    #[test]
    fn reloading_missing_file_reports_error_and_preserves_edits() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr = manager("layout { gaps 8; }");
        mgr.config_path = dir.path().join("missing.kdl");
        mgr.set_raw_content("layout { gaps 12; }".to_owned());
        assert!(mgr.reload_from_disk().is_err());
        assert_eq!(mgr.raw_content(), "layout { gaps 12; }");
        assert!(mgr.is_dirty());
    }

    #[test]
    fn incomplete_visual_source_keeps_default_values_and_disabled_flags() {
        let settings = ConfigManager::parse_visual_settings("layout { gaps");
        assert_eq!(
            settings,
            VisualSettings {
                touchpad_tap: false,
                touchpad_natural_scroll: false,
                keyboard_numlock: false,
                ..VisualSettings::default()
            },
        );
    }

    #[test]
    fn malformed_unicode_color_does_not_panic_while_editing() {
        let mut mgr = manager("");
        mgr.set_raw_content(
            "animations { workspace-switch { cube { background-color \"#a中aa\"; }; }; }".into(),
        );
        mgr.validate();
        assert!(matches!(
            mgr.validation_status(),
            ValidationStatus::Error(_)
        ));
    }

    #[test]
    fn absent_hot_corners_match_compositor_defaults() {
        let config = Config::parse_mem("").unwrap();
        let visual = ConfigManager::parse_visual_settings("");
        let corners = config.gestures.hot_corners;
        assert_eq!(visual.hot_corner_top_left, corners.top_left);
        assert_eq!(visual.hot_corner_top_right, corners.top_right);
        assert_eq!(visual.hot_corner_bottom_left, corners.bottom_left);
        assert_eq!(visual.hot_corner_bottom_right, corners.bottom_right);
    }

    #[test]
    fn changing_gaps_preserves_independent_glide_settings() {
        let original = "animations { window-open { glide { tilt-deg 15; depth 0.12345; }; }; window-close { glide { off; tilt-deg 30; depth 0.5; }; }; }\n";
        let before = Config::parse_mem(original).unwrap();
        let mut mgr = manager(original);
        mgr.visual_settings = ConfigManager::parse_visual_settings(original);
        mgr.visual_settings.gaps = 20.;
        mgr.apply_visual_settings_to_raw();
        let after = Config::parse_mem(mgr.raw_content()).unwrap();
        assert_eq!(
            after.animations.window_open.glide,
            before.animations.window_open.glide
        );
        assert_eq!(
            after.animations.window_close.glide,
            before.animations.window_close.glide
        );
    }

    #[test]
    fn changing_gaps_preserves_cube_and_wobble_source() {
        let original = r#"animations {
    workspace-switch { cube { perspective 0.654321; background-color "red"; }; }
    window-wobble { mass 0.12345; damping 12.345678; } // tuned
}
"#;
        let mut mgr = manager(original);
        mgr.visual_settings = ConfigManager::parse_visual_settings(original);
        mgr.visual_settings.gaps = 22.;
        mgr.apply_visual_settings_to_raw();
        assert!(mgr.raw_content().contains(original));
        Config::parse_mem(mgr.raw_content()).unwrap();
    }

    #[test]
    fn changing_cube_perspective_preserves_color_and_comments() {
        let original = "animations { workspace-switch { cube { perspective 0.65; /* color */ background-color \"red\"; }; }; }";
        let mut settings = ConfigManager::parse_visual_settings(original);
        settings.cube_perspective = 0.4;
        let updated = ConfigManager::update_classic_effects(original, &settings);
        let parsed = Config::parse_mem(&updated).unwrap();
        let before = Config::parse_mem(original).unwrap();
        assert_eq!(parsed.animations.workspace_switch.1.perspective, 0.4);
        assert_eq!(
            parsed.animations.workspace_switch.1.background_color,
            before.animations.workspace_switch.1.background_color
        );
        assert!(updated.contains("/* color */"));
    }

    #[test]
    fn wobble_mass_preserves_precision() {
        let mut settings = ConfigManager::parse_visual_settings("");
        settings.wobble_mass = 0.12345;
        let updated = ConfigManager::update_classic_effects("", &settings);
        let parsed = Config::parse_mem(&updated).unwrap();
        assert_eq!(parsed.animations.window_wobble.mass, 0.12345);
    }

    #[test]
    fn enabling_cube_removes_off_without_resetting_parameters() {
        let original = r#"animations { workspace-switch { cube { off; perspective 0.4321; background-color "red"; }; }; }"#;
        let mut settings = ConfigManager::parse_visual_settings(original);
        settings.workspace_cube_enabled = true;
        let updated = ConfigManager::update_classic_effects(original, &settings);
        let parsed = Config::parse_mem(&updated).unwrap();
        assert!(!parsed.animations.workspace_switch.1.off);
        assert_eq!(parsed.animations.workspace_switch.1.perspective, 0.4321);
        assert!(updated.contains("background-color \"red\""));
        settings.workspace_cube_enabled = false;
        let disabled = ConfigManager::update_classic_effects(&updated, &settings);
        assert!(
            Config::parse_mem(&disabled)
                .unwrap()
                .animations
                .workspace_switch
                .1
                .off
        );
    }

    #[test]
    fn visual_settings_read_all_supported_integer_bases() {
        let content = "layout { gaps 0x18; border { width 0b11; }; focus-ring { width 0o5; }; }";
        let config = Config::parse_mem(content).unwrap();
        let visual = ConfigManager::parse_visual_settings(content);
        assert_eq!(visual.gaps as f64, config.layout.gaps);
        assert_eq!(visual.border_width as f64, config.layout.border.width);
        assert_eq!(
            visual.focus_ring_width as f64,
            config.layout.focus_ring.width
        );
    }

    #[test]
    fn cube_color_matches_the_compositor_for_css_colors() {
        for color in ["red", "#f008", "rgba(255, 0, 0, 0.5)"] {
            let content = format!("animations {{ workspace-switch {{ cube {{ background-color \"{color}\"; }}; }}; }}");
            let config = Config::parse_mem(&content).unwrap();
            let visual = ConfigManager::parse_visual_settings(&content);
            assert_eq!(
                visual.cube_background_color,
                config
                    .animations
                    .workspace_switch
                    .1
                    .background_color
                    .to_array_unpremul()
            );
        }
    }

    #[test]
    fn animation_on_override_matches_compositor() {
        let content = "animations { workspace-switch { cube { off; on; }; }; window-wobble { off; on; }; window-open { glide { off; on; }; }; }";
        let config = Config::parse_mem(content).unwrap();
        let visual = ConfigManager::parse_visual_settings(content);
        assert_eq!(
            visual.workspace_cube_enabled,
            !config.animations.workspace_switch.1.off
        );
        assert_eq!(
            visual.window_wobble_enabled,
            !config.animations.window_wobble.off
        );
        assert_eq!(
            visual.window_glide_enabled,
            !config.animations.window_open.glide.off
        );
    }

    #[test]
    fn editing_debounces_validation() {
        let mut manager = manager("");
        manager.set_raw_content("layout { gaps 8; }".to_owned());

        assert!(manager.is_dirty());
        assert_eq!(manager.validation_status(), &ValidationStatus::NotChecked);
        assert!(manager.validation_due.is_some());
    }

    #[test]
    fn validates_in_process() {
        let mut manager = manager("layout { gaps 8; }");
        manager.validate();
        assert_eq!(manager.validation_status(), &ValidationStatus::Valid);

        manager.set_raw_content("layout {".to_owned());
        manager.validate();
        assert!(matches!(
            manager.validation_status(),
            ValidationStatus::Error(_)
        ));
    }

    #[test]
    fn visual_updates_are_scoped_to_their_blocks() {
        let mut manager = manager(
            r#"input {
    keyboard { numlock; }
    touchpad {
        tap
        natural-scroll
    }
    mouse {
        // natural-scroll
    }
}
layout {
    gaps 16
    focus-ring {
        width 3
    }
    border {
        width 2
    }
}
"#,
        );
        manager.visual_settings.border_width = 7.0;
        manager.visual_settings.focus_ring_width = 5.0;
        manager.visual_settings.touchpad_natural_scroll = false;
        manager.visual_settings.keyboard_numlock = false;
        manager.apply_visual_settings_to_raw();

        let focus_ring = ConfigManager::block_body_range(manager.raw_content(), "focus-ring")
            .map(|range| &manager.raw_content()[range])
            .unwrap();
        assert!(focus_ring.contains("width 5"));
        let border = ConfigManager::block_body_range(manager.raw_content(), "border")
            .map(|range| &manager.raw_content()[range])
            .unwrap();
        assert!(border.contains("width 7"));
        assert!(!ConfigManager::has_node_in_block(
            manager.raw_content(),
            "touchpad",
            "natural-scroll"
        ));
        assert!(!ConfigManager::has_node_in_block(
            manager.raw_content(),
            "keyboard",
            "numlock"
        ));
    }

    #[test]
    fn explicit_false_flags_are_not_treated_as_enabled() {
        let settings = ConfigManager::parse_visual_settings(
            "input { keyboard { numlock false } touchpad { tap false; natural-scroll off } } layout { always-center-single-column false }",
        );
        assert!(!settings.keyboard_numlock);
        assert!(!settings.touchpad_tap);
        assert!(!settings.touchpad_natural_scroll);
        assert!(!settings.always_center_single_column);
    }

    #[test]
    fn commented_input_options_are_not_treated_as_enabled() {
        let settings = ConfigManager::parse_visual_settings(
            "input { touchpad { // natural-scroll\n // tap\n } keyboard { // numlock\n } }",
        );

        assert!(!settings.touchpad_tap);
        assert!(!settings.touchpad_natural_scroll);
        assert!(!settings.keyboard_numlock);
    }

    #[test]
    fn classic_effects_round_trip_without_losing_close_effect() {
        let settings = VisualSettings {
            window_wobble_enabled: false,
            wobble_mesh: 16,
            workspace_cube_enabled: false,
            cube_perspective: 0.4,
            window_glide_enabled: true,
            glide_tilt_deg: 18.,
            glide_depth: 0.25,
            ..VisualSettings::default()
        };

        let original = r##"
animations {
    window-close {
        duration-ms 420
        effect "implode"
    }
}
"##;
        let updated = ConfigManager::update_classic_effects(original, &settings);
        let parsed = Config::parse_mem(&updated).unwrap();

        assert_eq!(
            parsed.animations.window_close.effect,
            realand_config::WindowCloseEffect::Implode
        );
        assert!(parsed.animations.window_wobble.off);
        assert_eq!(parsed.animations.window_wobble.mesh, 16);
        assert!(parsed.animations.workspace_switch.1.off);
        assert_eq!(parsed.animations.workspace_switch.1.perspective, 0.4);
        assert_eq!(parsed.animations.window_open.glide.tilt_deg, 18.);
        assert_eq!(parsed.animations.window_close.glide.depth, 0.25);

        let round_trip = ConfigManager::parse_visual_settings(&updated);
        assert!(!round_trip.window_wobble_enabled);
        assert_eq!(round_trip.wobble_mesh, 16);
        assert!(!round_trip.workspace_cube_enabled);
        assert!(round_trip.window_glide_enabled);
    }

    #[test]
    fn hot_corners_comment_and_round_trip() {
        let commented = r#"
gestures {
    hot-corners {
        // top-left
        top-right
        bottom-left
    }
}
"#;
        let parsed = ConfigManager::parse_visual_settings(commented);
        assert!(!parsed.hot_corner_top_left);
        assert!(parsed.hot_corner_top_right);
        assert!(parsed.hot_corner_bottom_left);
        assert!(!parsed.hot_corner_bottom_right);

        let mut settings = parsed;
        settings.hot_corner_top_left = true;
        let updated = ConfigManager::update_hot_corners(commented, &settings);
        let reparsed = ConfigManager::parse_visual_settings(&updated);
        assert!(reparsed.hot_corner_top_left);
        assert!(reparsed.hot_corner_top_right);
        assert!(reparsed.hot_corner_bottom_left);
        assert!(!reparsed.hot_corner_bottom_right);

        // When all are disabled, it should emit off
        let disabled_settings = VisualSettings {
            hot_corner_top_left: false,
            hot_corner_top_right: false,
            hot_corner_bottom_left: false,
            hot_corner_bottom_right: false,
            ..VisualSettings::default()
        };
        let off_updated = ConfigManager::update_hot_corners(commented, &disabled_settings);
        let off_parsed = ConfigManager::parse_visual_settings(&off_updated);
        assert!(!off_parsed.hot_corner_top_left);
        assert!(!off_parsed.hot_corner_top_right);
        assert!(!off_parsed.hot_corner_bottom_left);
        assert!(!off_parsed.hot_corner_bottom_right);
    }
    #[test]
    fn visual_updates_support_missing_and_inline_blocks() {
        for original in [
            "",
            "layout { gaps 16; }\n",
            "layout { gaps 16; center-focused-column \"never\"; }\n",
        ] {
            let mut mgr = manager(original);
            mgr.visual_settings.gaps = 24.;
            mgr.apply_visual_settings_to_raw();
            let parsed = Config::parse_mem(mgr.raw_content()).unwrap();
            assert_eq!(parsed.layout.gaps, 24.);
        }
    }

    #[test]
    fn hot_corners_use_existing_gestures_block() {
        let mut mgr = manager("gestures {\n dnd-edge-view-scroll { trigger-width 42; }\n}\n");
        mgr.apply_visual_settings_to_raw();
        mgr.apply_visual_settings_to_raw();
        let parsed = Config::parse_mem(mgr.raw_content()).unwrap();
        assert_eq!(parsed.gestures.dnd_edge_view_scroll.trigger_width, 42.);
        assert_eq!(mgr.raw_content().matches("gestures {").count(), 1);
    }

    #[test]
    fn cube_background_preserves_alpha() {
        let content = "animations {\n workspace-switch {\n cube {\n background-color \"#12345680\"\n }\n }\n }\n";
        let settings = ConfigManager::parse_visual_settings(content);
        assert!((settings.cube_background_color[3] - 128. / 255.).abs() < 0.001);
    }

    #[test]
    fn disabling_tap_preserves_button_mapping_and_adjacent_flags() {
        for (body, preserved) in [
            (
                "        tap-button-map \"left-middle-right\"\n",
                "tap-button-map \"left-middle-right\"",
            ),
            ("        tap; natural-scroll;\n", "natural-scroll;"),
        ] {
            let content = format!("input {{\n touchpad {{\n{body} }}\n}}\n");
            let updated =
                ConfigManager::set_flag_in_block(&content, "touchpad", "tap", false, "        tap");
            Config::parse_mem(&updated).unwrap();
            assert!(updated.contains(preserved));
            assert!(!ConfigManager::has_node_in_block(
                &updated, "touchpad", "tap"
            ));
        }
    }

    #[test]
    fn snippets_merge_into_existing_blocks_instead_of_duplicating() {
        let content = "layout {\n    gaps 8\n    border {\n        width 1\n    }\n}\ngestures {\n    dnd-edge-view-scroll {\n        trigger-width 42\n    }\n}\nbinds {\n    Mod+Q { close-window; }\n}\n";
        let with_border = ConfigManager::upsert_child_block(
            content,
            &["layout"],
            "border",
            "border {\n        width 3\n    }",
        );
        Config::parse_mem(&with_border).unwrap();
        assert_eq!(with_border.matches("border {").count(), 1);
        assert!(with_border.contains("width 3"));

        let with_corners = ConfigManager::upsert_child_block(
            content,
            &["gestures"],
            "hot-corners",
            "hot-corners {\n        top-left\n    }",
        );
        Config::parse_mem(&with_corners).unwrap();
        assert_eq!(with_corners.matches("gestures {").count(), 1);
        assert!(with_corners.contains("trigger-width 42"));
        assert!(with_corners.contains("top-left"));

        let with_binds = ConfigManager::insert_snippet_in_block(
            content,
            "binds",
            "Mod+Shift+C { spawn \"realand-settings\"; }",
        );
        Config::parse_mem(&with_binds).unwrap();
        assert_eq!(with_binds.matches("binds {").count(), 1);
        assert!(with_binds.contains("close-window"));
        assert!(with_binds.contains("realand-settings"));
    }
}
