//! Output previews live in the compositor, so a stalled or dead settings client
//! cannot leave an unconfirmed configuration active or persist it to disk.
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{ensure, Context, Result};
use calloop::timer::{TimeoutAction, Timer};
use calloop::RegistrationToken;
use realand_config::{Config, Outputs};

use crate::compositor::State;

pub(crate) struct OutputPreview {
    id: u64,
    original: Outputs,
    deadline: Instant,
    timer: RegistrationToken,
}

/// A bounded compensation record while the settings client saves the confirmed
/// draft. Any later output operation or configuration reload invalidates it.
pub(crate) struct ConfirmedOutputPreview {
    id: u64,
    original: Outputs,
    applied: Outputs,
}

pub(crate) fn parse_outputs(root: &str, edited: &str, content: String) -> Result<Outputs> {
    let root = Path::new(root);
    let edited = Path::new(edited);
    ensure!(
        root.is_absolute() && edited.is_absolute(),
        "配置路径必须是绝对路径"
    );
    let canonical = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_owned());
    let parsed = if canonical(root) == canonical(edited) {
        Config::parse(root, &content)
    } else {
        let source = realand_config::read_config_file(root).context("无法读取主配置")?;
        let parsed =
            Config::parse_with_overrides(root, &source, [(canonical(edited), content)].into());
        ensure!(
            parsed
                .includes
                .iter()
                .any(|p| canonical(p) == canonical(edited)),
            "编辑文件不在主配置的 include 中"
        );
        parsed
    };
    Ok(parsed
        .config
        .map_err(|e| anyhow::anyhow!(e.to_string()))?
        .outputs)
}

impl State {
    pub(crate) fn begin_output_preview(
        &mut self,
        outputs: Outputs,
        timeout: Duration,
    ) -> Result<u64> {
        ensure!(!self.realand.is_locked(), "会话锁定时不能预览显示设置");
        ensure!(
            self.realand.output_preview.is_none(),
            "已有显示设置等待确认"
        );
        // Tokens must not collide when a client reconnects to a restarted compositor.
        use std::io::Read;
        let mut bytes = [0; 8];
        std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
        let id = u64::from_ne_bytes(bytes);
        let deadline = Instant::now() + timeout;
        // Arm recovery before changing any hardware state.
        let timer = self
            .realand
            .event_loop
            .insert_source(Timer::from_deadline(deadline), move |_, _, state| {
                if state
                    .realand
                    .output_preview
                    .as_ref()
                    .is_some_and(|p| p.id == id)
                {
                    let preview = state.realand.output_preview.take().unwrap();
                    state.realand.last_cancelled_output_preview = Some(preview.id);
                    state.realand.config.borrow_mut().outputs = preview.original;
                    state.reload_output_config();
                    tracing::info!(
                        id,
                        "output preview expired; restored previous configuration"
                    );
                }
                TimeoutAction::Drop
            })
            .map_err(|e| anyhow::anyhow!("无法启动显示设置恢复计时器：{e}"))?;
        let original = std::mem::replace(&mut self.realand.config.borrow_mut().outputs, outputs);
        self.realand.output_preview = Some(OutputPreview {
            id,
            original,
            deadline,
            timer,
        });
        self.realand.confirmed_output_preview = None;
        self.realand.last_cancelled_output_preview = None;
        self.reload_output_config();
        Ok(id)
    }

    pub(crate) fn finish_output_preview(&mut self, id: u64, keep: bool) -> Result<()> {
        if !keep && self.realand.output_preview.is_none() {
            if let Some(confirmed) = self.realand.confirmed_output_preview.as_ref() {
                ensure!(confirmed.id == id, "显示预览编号不匹配");
                ensure!(
                    confirmed.applied == self.realand.config.borrow().outputs,
                    "显示配置已发生变化，不能恢复旧预览"
                );
                let confirmed = self.realand.confirmed_output_preview.take().unwrap();
                self.realand.config.borrow_mut().outputs = confirmed.original;
                self.realand.last_cancelled_output_preview = Some(id);
                self.reload_output_config();
                return Ok(());
            }
        }
        if !keep
            && self.realand.output_preview.is_none()
            && self.realand.last_cancelled_output_preview == Some(id)
        {
            return Ok(());
        }
        let preview = self
            .realand
            .output_preview
            .as_ref()
            .context("显示预览已结束，请重新预览")?;
        ensure!(preview.id == id, "显示预览编号不匹配");
        if Instant::now() >= preview.deadline {
            self.cancel_output_preview();
            ensure!(!keep, "显示预览已超时，已恢复原设置");
            return Ok(());
        }
        if keep {
            ensure!(!self.realand.is_locked(), "会话锁定时不能确认显示设置");
            let preview = self.realand.output_preview.take().unwrap();
            self.realand.event_loop.remove(preview.timer);
            self.realand.confirmed_output_preview = Some(ConfirmedOutputPreview {
                id: preview.id,
                original: preview.original,
                applied: self.realand.config.borrow().outputs.clone(),
            });
        } else {
            self.cancel_output_preview();
        }
        Ok(())
    }

    pub(crate) fn cancel_output_preview(&mut self) {
        self.realand.confirmed_output_preview = None;
        if let Some(preview) = self.realand.output_preview.take() {
            self.realand.last_cancelled_output_preview = Some(preview.id);
            self.realand.event_loop.remove(preview.timer);
            self.realand.config.borrow_mut().outputs = preview.original;
            self.reload_output_config();
        }
    }
}
