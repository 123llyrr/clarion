// SPDX-License-Identifier: GPL-3.0-or-later
//! Native desktop files and wallpaper. Filesystem scans, image decoding and file
//! operations run on a bounded worker; rendering uses only immutable snapshots.
//! The former per-output desktop layout files are intentionally never rewritten.
use std::collections::HashMap;
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{Cursor, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context as _, Result};
use async_channel::Sender;
use gpui::{prelude::*, *};
use image::ImageReader;

use crate::input::{QueryChanged, TextInput};

const MAX_ENTRIES: usize = 500;
const MAX_VISITED: usize = 4096;
const MAX_IMAGE_BYTES: u64 = 64 * 1024 * 1024;
const CONFIG_LIMIT: u64 = 65536;

actions!(
    clarion_desktop,
    [
        OpenSelection,
        RenameSelection,
        TrashSelection,
        Cancel,
        NextSelection,
        PreviousSelection
    ]
);

#[derive(Clone, Debug)]
pub enum DesktopAction {
    Open(PathBuf),
    Settings,
    /// None asks the host to choose an image; Some supplies a desktop image.
    Wallpaper(Option<PathBuf>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EntryKind {
    Directory,
    File,
    Image,
    Application,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DesktopEntry {
    path: PathBuf,
    name: String,
    kind: EntryKind,
    modified: Option<SystemTime>,
}

struct DesktopSnapshot {
    directory: Option<PathBuf>,
    entries: Vec<DesktopEntry>,
    wallpaper: Option<Arc<Image>>,
    error: Option<String>,
}

enum Work {
    Refresh,
    Rename { path: PathBuf, name: String },
    Trash(PathBuf),
    Wallpaper(PathBuf),
}
enum Update {
    Snapshot(DesktopSnapshot),
    Error(String),
}

pub struct DesktopView {
    output: String,
    actions: Sender<DesktopAction>,
    preview: bool,
    entries: Vec<DesktopEntry>,
    directory: Option<PathBuf>,
    wallpaper: Option<Arc<Image>>,
    selected: Option<PathBuf>,
    menu: Option<Point<Pixels>>,
    rename: Option<PathBuf>,
    rename_name: String,
    input: Entity<TextInput>,
    focus: FocusHandle,
    error: Option<String>,
    busy: bool,
    work: mpsc::SyncSender<Work>,
    alive: Arc<AtomicBool>,
    scroll: ScrollHandle,
    reserved_edges: [f32; 4],
}

impl DesktopView {
    pub fn bind_keys(cx: &mut App) {
        cx.bind_keys([
            KeyBinding::new("enter", OpenSelection, Some("ClarionDesktop")),
            KeyBinding::new("f2", RenameSelection, Some("ClarionDesktop")),
            KeyBinding::new("delete", TrashSelection, Some("ClarionDesktop")),
            KeyBinding::new("escape", Cancel, Some("ClarionDesktop")),
            KeyBinding::new("right", NextSelection, Some("ClarionDesktop")),
            KeyBinding::new("down", NextSelection, Some("ClarionDesktop")),
            KeyBinding::new("left", PreviousSelection, Some("ClarionDesktop")),
            KeyBinding::new("up", PreviousSelection, Some("ClarionDesktop")),
        ]);
    }

    pub fn new(
        output: String,
        environment: HashMap<String, String>,
        actions: Sender<DesktopAction>,
        preview: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| TextInput::new(cx).with_placeholder("文件名称"));
        cx.subscribe(&input, |this, _, QueryChanged(name), cx| {
            this.rename_name = name.clone();
            cx.notify();
        })
        .detach();
        let (work, incoming) = mpsc::sync_channel(16);
        let (updates, received) = async_channel::bounded(4);
        let alive = Arc::new(AtomicBool::new(true));
        let worker_alive = alive.clone();
        let startup_error = std::thread::Builder::new()
            .name("clarion-desktop-files".into())
            .spawn(move || worker(environment, preview, incoming, updates, worker_alive))
            .err()
            .map(|error| format!("无法启动桌面文件服务：{error}"));
        cx.spawn(async move |this, cx| {
            while let Ok(update) = received.recv().await {
                if this
                    .update(cx, |this, cx| {
                        this.busy = false;
                        match update {
                            Update::Snapshot(snapshot) => {
                                this.entries = snapshot.entries;
                                this.directory = snapshot.directory;
                                this.wallpaper = snapshot.wallpaper;
                                this.error = snapshot.error;
                                if this.selected.as_ref().is_some_and(|path| {
                                    !this.entries.iter().any(|entry| &entry.path == path)
                                }) {
                                    this.selected = None;
                                }
                            }
                            Update::Error(error) => this.error = Some(error),
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Self {
            output,
            actions,
            preview,
            entries: Vec::new(),
            directory: None,
            wallpaper: None,
            selected: None,
            menu: None,
            rename: None,
            rename_name: String::new(),
            input,
            focus: cx.focus_handle(),
            error: startup_error,
            busy: false,
            work,
            alive,
            scroll: ScrollHandle::new(),
            reserved_edges: [28., 78., 0., 0.],
        }
    }

    /// Space occupied by the top bar and the chosen Dock edge, in logical
    /// pixels. The grid adds its own 16px breathing room inside these edges.
    pub fn set_reserved_edges(
        &mut self,
        top: f32,
        bottom: f32,
        left: f32,
        right: f32,
        cx: &mut Context<Self>,
    ) {
        let edges = [top, bottom, left, right].map(|value| {
            if value.is_finite() {
                value.clamp(0., 16384.)
            } else {
                0.
            }
        });
        if self.reserved_edges != edges {
            self.reserved_edges = edges;
            cx.notify();
        }
    }

    /// The host calls this after selecting a file. Validation, image decoding and
    /// atomic preference saving happen on the desktop worker.
    pub fn set_wallpaper(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.enqueue(Work::Wallpaper(path), cx);
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.enqueue(Work::Refresh, cx);
    }

    fn enqueue(&mut self, work: Work, cx: &mut Context<Self>) {
        if self.preview && !matches!(work, Work::Refresh) {
            self.error = Some("预览模式不会修改文件".into());
            cx.notify();
            return;
        }
        match self.work.try_send(work) {
            Ok(()) => {
                self.busy = true;
                self.error = None;
            }
            Err(_) => self.error = Some("文件操作正在处理中，请稍后重试".into()),
        }
        cx.notify();
    }

    fn send(&mut self, action: DesktopAction, cx: &mut Context<Self>) {
        if self.preview {
            self.error = Some("预览模式不会打开或修改用户文件".into());
        } else if self.actions.try_send(action).is_err() {
            self.error = Some("桌面操作暂时不可用".into());
        }
        self.menu = None;
        cx.notify();
    }

    fn begin_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.preview || self.busy {
            return;
        }
        let Some(entry) = self
            .entries
            .iter()
            .find(|entry| Some(&entry.path) == self.selected.as_ref())
            .cloned()
        else {
            return;
        };
        self.menu = None;
        self.rename = Some(entry.path);
        self.rename_name = entry.name.clone();
        self.input
            .update(cx, |input, cx| input.set_text(&entry.name, true, cx));
        window.focus(&self.input.focus_handle(cx));
        cx.notify();
    }

    fn activate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.rename.clone() {
            if self
                .input
                .update(cx, |input, cx| input.marked_text_range(window, cx))
                .is_some()
            {
                return;
            }
            if let Err(error) = validate_name(&self.rename_name) {
                self.error = Some(error.to_string());
                cx.notify();
                return;
            }
            self.rename = None;
            window.focus(&self.focus);
            self.enqueue(
                Work::Rename {
                    path,
                    name: self.rename_name.clone(),
                },
                cx,
            );
        } else if let Some(path) = self.selected.clone() {
            self.send(DesktopAction::Open(path), cx);
        }
    }

    fn trash(&mut self, cx: &mut Context<Self>) {
        if self.rename.is_none() && !self.busy {
            if let Some(path) = self.selected.clone() {
                self.menu = None;
                self.enqueue(Work::Trash(path), cx);
            }
        }
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.rename = None;
        self.error = None;
        window.focus(&self.focus);
        cx.notify();
    }

    fn step(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.rename.is_some() || self.entries.is_empty() {
            return;
        }
        let current = self
            .entries
            .iter()
            .position(|entry| Some(&entry.path) == self.selected.as_ref());
        let index = match current {
            Some(index) if forward => (index + 1).min(self.entries.len() - 1),
            Some(index) => index.saturating_sub(1),
            None => 0,
        };
        self.selected = Some(self.entries[index].path.clone());
        self.menu = None;
        cx.notify();
    }

    fn context_button(
        &self,
        id: &'static str,
        label: &'static str,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &Context<Self>,
    ) -> AnyElement {
        div()
            .id(id)
            .px_3()
            .py_2()
            .rounded(px(6.))
            .text_sm()
            .cursor_pointer()
            .hover(|style| style.bg(rgba(0xffffff12)))
            .child(label)
            .on_click(cx.listener(move |this, _, window, cx| action(this, window, cx)))
            .into_any_element()
    }

    fn file_icon(kind: EntryKind) -> AnyElement {
        match kind {
            EntryKind::Directory => div()
                .w(px(46.))
                .h(px(38.))
                .relative()
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .w(px(21.))
                        .h(px(11.))
                        .rounded_t(px(4.))
                        .bg(rgb(0x3b93d8)),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(7.))
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .rounded(px(5.))
                        .bg(rgb(0x64b9ed))
                        .border_1()
                        .border_color(rgba(0xd6efff66)),
                )
                .into_any_element(),
            kind => div()
                .w(px(34.))
                .h(px(43.))
                .rounded(px(5.))
                .bg(rgb(0xe6edf5))
                .border_1()
                .border_color(rgba(0xffffff66))
                .flex()
                .items_center()
                .justify_center()
                .text_xl()
                .text_color(rgb(0x4d6c89))
                .child(match kind {
                    EntryKind::Image => "▧",
                    EntryKind::Application => "◇",
                    _ => "≡",
                })
                .into_any_element(),
        }
    }
}

impl Drop for DesktopView {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Relaxed);
    }
}
impl Focusable for DesktopView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for DesktopView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let width = f32::from(viewport.width);
        let height = f32::from(viewport.height);
        let [top, bottom, left, right] = self.reserved_edges;
        let menu_position = self.menu.map(|position| {
            point(
                px(f32::from(position.x).clamp(left + 8., (width - right - 238.).max(left + 8.))),
                px(f32::from(position.y).clamp(top + 8., (height - bottom - 310.).max(top + 8.))),
            )
        });
        let mut root = div()
            .id("clarion-desktop")
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(rgb(0x202634))
            .text_color(rgb(0xf1f5fa))
            .font_family("Noto Sans")
            .key_context("ClarionDesktop")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &OpenSelection, window, cx| this.activate(window, cx)))
            .on_action(
                cx.listener(|this, _: &RenameSelection, window, cx| this.begin_rename(window, cx)),
            )
            .on_action(cx.listener(|this, _: &TrashSelection, _, cx| this.trash(cx)))
            .on_action(cx.listener(|this, _: &Cancel, window, cx| this.cancel(window, cx)))
            .on_action(cx.listener(|this, _: &NextSelection, _, cx| this.step(true, cx)))
            .on_action(cx.listener(|this, _: &PreviousSelection, _, cx| this.step(false, cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.selected = None;
                    this.menu = None;
                    this.rename = None;
                    window.focus(&this.focus);
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.selected = None;
                    this.rename = None;
                    this.menu = Some(event.position);
                    window.focus(&this.focus);
                    cx.notify();
                }),
            );
        if let Some(image) = &self.wallpaper {
            root = root.child(
                img(image.clone())
                    .absolute()
                    .size_full()
                    .object_fit(ObjectFit::Cover),
            );
        } else {
            root = root.child(
                div()
                    .absolute()
                    .top(px(-140.))
                    .right(px(-100.))
                    .size(px(width.min(900.)))
                    .rounded_full()
                    .bg(rgba(0x5d668328)),
            );
        }
        let mut grid = div()
            .id("desktop-grid")
            .absolute()
            .inset_0()
            .pt(px(top + 16.))
            .pb(px(bottom + 16.))
            .pl(px(left + 16.))
            .pr(px(right + 16.))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .flex()
            .flex_wrap()
            .content_start()
            .gap_2();
        for (index, entry) in self.entries.iter().enumerate() {
            let path = entry.path.clone();
            let context_path = path.clone();
            let selected = self.selected.as_ref() == Some(&path);
            let editing = self.rename.as_ref() == Some(&path);
            let mut icon = div()
                .id(("desktop-file", index))
                .w(px(94.))
                .min_h(px(90.))
                .flex_none()
                .p_2()
                .rounded(px(8.))
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .border_1()
                .border_color(if selected {
                    rgba(0xb8dcff99)
                } else {
                    rgba(0xffffff00)
                })
                .bg(if selected {
                    rgba(0x0a84ff66)
                } else {
                    rgba(0xffffff00)
                })
                .cursor_pointer()
                .hover(|style| style.bg(rgba(0xffffff18)))
                .child(Self::file_icon(entry.kind))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        window.focus(&this.focus);
                        this.menu = None;
                        if this.rename.as_ref() != Some(&path) {
                            this.rename = None;
                        }
                        this.selected = Some(path.clone());
                        if event.click_count >= 2 && this.rename.is_none() {
                            this.send(DesktopAction::Open(path.clone()), cx);
                        }
                        cx.notify();
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        window.focus(&this.focus);
                        this.rename = None;
                        this.selected = Some(context_path.clone());
                        this.menu = Some(event.position);
                        cx.notify();
                    }),
                );
            if editing {
                icon = icon.child(
                    div()
                        .w_full()
                        .px_1()
                        .py_1()
                        .rounded(px(4.))
                        .bg(rgb(0x202b37))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(self.input.clone()),
                );
            } else {
                icon = icon.child(
                    div()
                        .w_full()
                        .max_h(px(34.))
                        .overflow_hidden()
                        .text_xs()
                        .text_center()
                        .child(entry.name.clone()),
                );
            }
            grid = grid.child(icon);
        }
        root = root.child(grid);
        if self.preview {
            root = root.child(
                div()
                    .absolute()
                    .bottom(px(12.))
                    .left(px(20.))
                    .text_xs()
                    .text_color(rgb(0xafbacb))
                    .child(format!(
                        "Clarion 桌面预览 · {} · 不读取或修改用户目录",
                        self.output
                    )),
            );
        }
        if let Some(error) = &self.error {
            root = root.child(
                div()
                    .absolute()
                    .left(px(16.))
                    .bottom(px(12.))
                    .right(px(16.))
                    .p_3()
                    .rounded(px(8.))
                    .bg(rgba(0x382923ee))
                    .text_sm()
                    .child(error.clone()),
            );
        }
        if let Some(position) = menu_position {
            let mut menu = div()
                .id("desktop-context-menu")
                .absolute()
                .left(position.x)
                .top(position.y)
                .w(px(230.))
                .p_1()
                .rounded(px(10.))
                .bg(rgba(0x202b38f5))
                .border_1()
                .border_color(rgba(0xffffff24))
                .shadow_lg()
                .flex()
                .flex_col()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation());
            if self.selected.is_some() {
                menu = menu
                    .child(self.context_button(
                        "desktop-open",
                        "打开",
                        |this, _, cx| {
                            if let Some(path) = this.selected.clone() {
                                this.send(DesktopAction::Open(path), cx)
                            }
                        },
                        cx,
                    ))
                    .child(self.context_button(
                        "desktop-rename",
                        "重命名…",
                        |this, window, cx| this.begin_rename(window, cx),
                        cx,
                    ))
                    .child(self.context_button(
                        "desktop-trash",
                        "移到废纸篓",
                        |this, _, cx| this.trash(cx),
                        cx,
                    ));
                if self
                    .entries
                    .iter()
                    .find(|entry| Some(&entry.path) == self.selected.as_ref())
                    .is_some_and(|entry| entry.kind == EntryKind::Image)
                {
                    menu = menu.child(self.context_button(
                        "desktop-wallpaper-file",
                        "设为桌面壁纸",
                        |this, _, cx| {
                            if let Some(path) = this.selected.clone() {
                                this.send(DesktopAction::Wallpaper(Some(path)), cx)
                            }
                        },
                        cx,
                    ));
                }
                menu = menu.child(div().h(px(1.)).my_1().bg(rgba(0xffffff18)));
            }
            root = root.child(
                menu.child(self.context_button(
                    "desktop-refresh",
                    "刷新桌面",
                    |this, _, cx| {
                        this.menu = None;
                        this.refresh(cx)
                    },
                    cx,
                ))
                .child(self.context_button(
                    "desktop-open-directory",
                    "打开桌面文件夹",
                    |this, _, cx| {
                        if let Some(path) = this.directory.clone() {
                            this.send(DesktopAction::Open(path), cx)
                        }
                    },
                    cx,
                ))
                .child(self.context_button(
                    "desktop-wallpaper",
                    "更换壁纸…",
                    |this, _, cx| this.send(DesktopAction::Wallpaper(None), cx),
                    cx,
                ))
                .child(self.context_button(
                    "desktop-settings",
                    "桌面设置…",
                    |this, _, cx| this.send(DesktopAction::Settings, cx),
                    cx,
                )),
            );
        }
        root
    }
}

fn read_regular(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)?;
    let metadata = file.metadata()?;
    anyhow::ensure!(
        metadata.is_file() && metadata.len() <= limit,
        "文件太大或不是普通文件"
    );
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() as u64 <= limit, "文件过大");
    Ok(bytes)
}

fn desktop_directory(environment: &HashMap<String, String>) -> Result<PathBuf> {
    let home = environment
        .get("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .context("缺少用户主目录")?;
    if let Some(path) = environment
        .get("XDG_DESKTOP_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        return Ok(path);
    }
    let config = environment
        .get("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home.join(".config"));
    if let Ok(bytes) = read_regular(&config.join("user-dirs.dirs"), CONFIG_LIMIT) {
        if let Ok(text) = String::from_utf8(bytes) {
            for line in text.lines().map(str::trim) {
                let Some(value) = line.strip_prefix("XDG_DESKTOP_DIR=") else {
                    continue;
                };
                let value = value.trim();
                if let Some(value) = value
                    .strip_prefix('"')
                    .and_then(|value| value.strip_suffix('"'))
                {
                    let resolved = if value == "$HOME" {
                        home.clone()
                    } else if let Some(suffix) = value.strip_prefix("$HOME/") {
                        home.join(suffix)
                    } else {
                        PathBuf::from(value)
                    };
                    if resolved.is_absolute()
                        && !value.contains(['`', '\n', '\0'])
                        && !value.contains("$(")
                    {
                        return Ok(resolved);
                    }
                }
            }
        }
    }
    Ok(home.join("Desktop"))
}

fn scan(directory: &Path) -> Result<Vec<DesktopEntry>> {
    let mut entries = Vec::new();
    let directory = match fs::read_dir(directory) {
        Ok(directory) => directory,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(entries),
        Err(error) => return Err(error.into()),
    };
    for entry in directory.take(MAX_VISITED).flatten() {
        if entries.len() >= MAX_ENTRIES {
            break;
        }
        let path = entry.path();
        let Some(raw_name) = path.file_name() else {
            continue;
        };
        if raw_name.as_bytes().starts_with(b".") {
            continue;
        }
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        let extension = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let kind = if metadata.is_dir() {
            EntryKind::Directory
        } else if matches!(extension.as_str(), "png" | "jpg" | "jpeg") {
            EntryKind::Image
        } else if extension == "desktop" {
            EntryKind::Application
        } else {
            EntryKind::File
        };
        entries.push(DesktopEntry {
            path: path.clone(),
            name: raw_name.to_string_lossy().into_owned(),
            kind,
            modified: metadata.modified().ok(),
        });
    }
    entries.sort_by_key(|entry| {
        (
            entry.kind != EntryKind::Directory,
            entry.name.to_lowercase(),
            entry.path.clone(),
        )
    });
    Ok(entries)
}

fn validate_name(name: &str) -> Result<()> {
    anyhow::ensure!(
        !name.trim().is_empty()
            && !matches!(name, "." | "..")
            && !name.contains(['/', '\0'])
            && name.len() <= 255,
        "文件名不能为空，不能包含 /，且不能超过255字节"
    );
    Ok(())
}

fn validate_source(directory: &Path, path: &Path) -> Result<()> {
    anyhow::ensure!(
        path.parent() == Some(directory) && path.file_name().is_some(),
        "只能修改桌面文件夹中的项目"
    );
    fs::symlink_metadata(path).context("项目已被移动或删除")?;
    Ok(())
}

fn rename_entry(directory: &Path, path: &Path, name: &str) -> Result<()> {
    validate_source(directory, path)?;
    validate_name(name)?;
    let destination = directory.join(name);
    if destination == path {
        return Ok(());
    }
    let from = CString::new(path.as_os_str().as_bytes())?;
    let to = CString::new(destination.as_os_str().as_bytes())?;
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error()).context("重命名失败，现有同名文件不会被覆盖");
    };
    Ok(())
}

fn trash_entry(directory: &Path, path: &Path, environment: &HashMap<String, String>) -> Result<()> {
    validate_source(directory, path)?;
    let mut child = Command::new("gio")
        .arg("trash")
        .arg("--")
        .arg(path)
        .env_clear()
        .envs(environment)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("无法启动废纸篓服务")?;
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        match child.try_wait()? {
            Some(status) => {
                anyhow::ensure!(status.success(), "无法移到废纸篓，原文件未直接删除");
                return Ok(());
            }
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                anyhow::bail!("废纸篓操作超时，请检查项目是否已移走")
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

fn image_data(path: &Path) -> Result<Arc<Image>> {
    anyhow::ensure!(path.is_absolute(), "壁纸必须使用绝对路径");
    let bytes = read_regular(path, MAX_IMAGE_BYTES)?;
    let mut reader = ImageReader::new(Cursor::new(&bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().context("无法读取壁纸图像")?;
    anyhow::ensure!(
        u64::from(image.width()) * u64::from(image.height()) <= 32 * 1024 * 1024,
        "壁纸像素尺寸过大"
    );
    let image = image.thumbnail(3840, 2160);
    let mut output = Cursor::new(Vec::new());
    image.write_to(&mut output, image::ImageFormat::Png)?;
    Ok(Arc::new(Image::from_bytes(
        ImageFormat::Png,
        output.into_inner(),
    )))
}

type WallpaperKey = (PathBuf, u64, Option<SystemTime>);
fn wallpaper(
    environment: &HashMap<String, String>,
    cache: &mut Option<(WallpaperKey, Arc<Image>)>,
) -> Result<(Option<WallpaperKey>, Option<Arc<Image>>)> {
    let file = crate::preferences::config_directory(environment)?.join("wallpaper.json");
    let bytes = match read_regular(&file, CONFIG_LIMIT) {
        Ok(bytes) => bytes,
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            return Ok((None, None))
        }
        Err(error) => return Err(error),
    };
    let json: serde_json::Value = serde_json::from_slice(&bytes)?;
    anyhow::ensure!(json["version"] == 1, "壁纸配置版本无效");
    let path = PathBuf::from(json["path"].as_str().context("壁纸配置缺少路径")?);
    let metadata = fs::metadata(&path)?;
    let key = (path.clone(), metadata.len(), metadata.modified().ok());
    if let Some((old, image)) = cache {
        if old == &key {
            return Ok((Some(key), Some(image.clone())));
        }
    }
    let image = image_data(&path)?;
    *cache = Some((key.clone(), image.clone()));
    Ok((Some(key), Some(image)))
}

fn save_wallpaper(environment: &HashMap<String, String>, path: &Path) -> Result<()> {
    // Validate and decode before writing anything. The actual snapshot reuses
    // the cached result after saving; unknown wallpaper fields survive.
    image_data(path)?;
    let directory = crate::preferences::config_directory(environment)?;
    let file = directory.join("wallpaper.json");
    let mut json = match read_regular(&file, CONFIG_LIMIT) {
        Ok(bytes) => serde_json::from_slice::<serde_json::Value>(&bytes)?,
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            serde_json::json!({})
        }
        Err(error) => return Err(error),
    };
    let object = json.as_object_mut().context("壁纸配置格式无效")?;
    object.insert("version".into(), 1.into());
    object.insert("path".into(), path.to_string_lossy().to_string().into());
    object.insert("mode".into(), "fill".into());
    fs::create_dir_all(&directory)?;
    let mut temp = tempfile::NamedTempFile::new_in(&directory)?;
    temp.as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))?;
    serde_json::to_writer(&mut temp, &json)?;
    temp.write_all(b"\n")?;
    temp.as_file().sync_all()?;
    temp.persist(file)?;
    File::open(directory)?.sync_all()?;
    Ok(())
}

fn worker(
    environment: HashMap<String, String>,
    preview: bool,
    incoming: mpsc::Receiver<Work>,
    updates: Sender<Update>,
    alive: Arc<AtomicBool>,
) {
    if preview {
        let snapshot = DesktopSnapshot {
            directory: None,
            entries: vec![
                DesktopEntry {
                    path: PathBuf::from("/clarion-preview/Documents"),
                    name: "文档".into(),
                    kind: EntryKind::Directory,
                    modified: None,
                },
                DesktopEntry {
                    path: PathBuf::from("/clarion-preview/Welcome.txt"),
                    name: "欢迎使用 Clarion.txt".into(),
                    kind: EntryKind::File,
                    modified: None,
                },
            ],
            wallpaper: None,
            error: None,
        };
        let _ = updates.try_send(Update::Snapshot(snapshot));
        while alive.load(Ordering::Relaxed) {
            if incoming.recv_timeout(Duration::from_millis(100)).is_err()
                && matches!(incoming.try_recv(), Err(mpsc::TryRecvError::Disconnected))
            {
                break;
            }
        }
        return;
    }
    let directory = match desktop_directory(&environment) {
        Ok(directory) => directory,
        Err(error) => {
            let _ = updates.try_send(Update::Error(error.to_string()));
            return;
        }
    };
    let mut cache = None;
    let mut previous: Option<(Vec<DesktopEntry>, Option<WallpaperKey>, Option<String>)> = None;
    let mut forced = true;
    while alive.load(Ordering::Relaxed) {
        let mut operation_error = None;
        if !forced {
            match incoming.recv_timeout(Duration::from_secs(3)) {
                Ok(work) => {
                    let result = match work {
                        Work::Refresh => Ok(()),
                        Work::Rename { path, name } => rename_entry(&directory, &path, &name),
                        Work::Trash(path) => trash_entry(&directory, &path, &environment),
                        Work::Wallpaper(path) => save_wallpaper(&environment, &path),
                    };
                    if let Err(error) = result {
                        operation_error = Some(error.to_string())
                    }
                    forced = true;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
        let (entries, scan_error) = match scan(&directory) {
            Ok(entries) => (entries, None),
            Err(error) => (vec![], Some(format!("无法读取桌面文件夹：{error}"))),
        };
        let (key, image, wallpaper_error) = match wallpaper(&environment, &mut cache) {
            Ok((key, image)) => (key, image, None),
            Err(error) => {
                let (key, image) = cache
                    .as_ref()
                    .map(|(key, image)| (Some(key.clone()), Some(image.clone())))
                    .unwrap_or_default();
                (key, image, Some(format!("壁纸暂时不可用：{error}")))
            }
        };
        let error = operation_error.or(scan_error).or(wallpaper_error);
        let signature = (entries.clone(), key.clone(), error.clone());
        if forced || previous.as_ref() != Some(&signature) {
            let snapshot = DesktopSnapshot {
                directory: Some(directory.clone()),
                entries,
                wallpaper: image,
                error,
            };
            match updates.try_send(Update::Snapshot(snapshot)) {
                Ok(()) => previous = Some(signature),
                Err(async_channel::TrySendError::Closed(_)) => break,
                Err(_) => {}
            }
        }
        forced = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn env(root: &Path) -> HashMap<String, String> {
        [
            ("HOME".into(), root.to_string_lossy().into_owned()),
            (
                "XDG_CONFIG_HOME".into(),
                root.join("config").to_string_lossy().into_owned(),
            ),
        ]
        .into()
    }
    #[::core::prelude::v1::test]
    fn localized_xdg_path_is_parsed_as_data_without_executing_shell() {
        let root = tempfile::tempdir().unwrap();
        let environment = env(root.path());
        fs::create_dir(root.path().join("config")).unwrap();
        fs::write(
            root.path().join("config/user-dirs.dirs"),
            "XDG_DESKTOP_DIR=\"$HOME/桌面\"\n",
        )
        .unwrap();
        assert_eq!(
            desktop_directory(&environment).unwrap(),
            root.path().join("桌面")
        );
        fs::write(
            root.path().join("config/user-dirs.dirs"),
            "XDG_DESKTOP_DIR=\"$(touch /tmp/not-run)\"\n",
        )
        .unwrap();
        assert_eq!(
            desktop_directory(&environment).unwrap(),
            root.path().join("Desktop")
        );
    }
    #[::core::prelude::v1::test]
    fn bounded_scan_and_rename_never_overwrite_or_modify_layout_files() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("Desktop");
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("a.txt"), b"a").unwrap();
        fs::write(directory.join("b.txt"), b"b").unwrap();
        fs::write(
            root.path().join("desktop-layout.json"),
            b"untouched positions",
        )
        .unwrap();
        assert!(rename_entry(&directory, &directory.join("a.txt"), "b.txt").is_err());
        assert_eq!(fs::read(directory.join("b.txt")).unwrap(), b"b");
        for name in ["", "..", "../escape", "a/b"] {
            assert!(rename_entry(&directory, &directory.join("a.txt"), name).is_err());
        }
        rename_entry(&directory, &directory.join("a.txt"), "中文文件.txt").unwrap();
        assert!(directory.join("中文文件.txt").exists());
        assert_eq!(
            fs::read(root.path().join("desktop-layout.json")).unwrap(),
            b"untouched positions"
        );
        for i in 0..550 {
            fs::write(directory.join(format!("fixture-{i}")), b"").unwrap();
        }
        assert_eq!(scan(&directory).unwrap().len(), MAX_ENTRIES);
    }
    #[::core::prelude::v1::test]
    fn trash_passes_a_literal_path_with_option_terminator_and_never_deletes_directly() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("Desktop");
        let bin = root.path().join("bin");
        fs::create_dir(&directory).unwrap();
        fs::create_dir(&bin).unwrap();
        let path = directory.join("name $(echo should-not-run) with spaces.txt");
        fs::write(&path, b"original").unwrap();
        let capture = root.path().join("arguments.json");
        let gio = bin.join("gio");
        fs::write(&gio, b"#!/usr/bin/python3\nimport json, os, sys\nwith open(os.environ['CLARION_TEST_CAPTURE'], 'w') as output: json.dump(sys.argv[1:], output)\n").unwrap();
        fs::set_permissions(&gio, fs::Permissions::from_mode(0o700)).unwrap();
        let environment = [
            ("PATH".into(), bin.to_string_lossy().into_owned()),
            (
                "CLARION_TEST_CAPTURE".into(),
                capture.to_string_lossy().into_owned(),
            ),
        ]
        .into();
        trash_entry(&directory, &path, &environment).unwrap();
        let args: Vec<String> = serde_json::from_slice(&fs::read(&capture).unwrap()).unwrap();
        assert_eq!(
            args,
            vec![
                "trash".into(),
                "--".into(),
                path.to_string_lossy().into_owned()
            ]
        );
        assert_eq!(fs::read(&path).unwrap(), b"original");
        assert!(trash_entry(&directory, &root.path().join("outside"), &environment).is_err());
    }

    #[::core::prelude::v1::test]
    fn wallpaper_schema_roundtrips_preserving_unknown_data_and_rejects_invalid_images() {
        let root = tempfile::tempdir().unwrap();
        let environment = env(root.path());
        let directory = crate::preferences::config_directory(&environment).unwrap();
        fs::create_dir_all(&directory).unwrap();
        let path = root.path().join("image.png");
        image::RgbaImage::from_pixel(4, 4, image::Rgba([20, 80, 100, 255]))
            .save(&path)
            .unwrap();
        fs::write(
            directory.join("wallpaper.json"),
            br#"{"version":1,"path":"/old","mode":"fill","unknown":{"a":1}}"#,
        )
        .unwrap();
        save_wallpaper(&environment, &path).unwrap();
        let saved = fs::read(directory.join("wallpaper.json")).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&saved).unwrap();
        assert_eq!(json["unknown"]["a"], 1);
        let mut cache = None;
        let (key, data) = wallpaper(&environment, &mut cache).unwrap();
        assert_eq!(key.unwrap().0, path);
        assert!(data.is_some());
        let bad = root.path().join("bad.png");
        fs::write(&bad, b"not an image").unwrap();
        assert!(save_wallpaper(&environment, &bad).is_err());
        assert_eq!(fs::read(directory.join("wallpaper.json")).unwrap(), saved);
    }
    #[::core::prelude::v1::test]
    fn preview_worker_never_reads_or_modifies_user_directory() {
        let root = tempfile::tempdir().unwrap();
        let environment = env(root.path());
        let path = root.path().join("secret");
        fs::write(&path, b"unchanged").unwrap();
        let (work, incoming) = mpsc::sync_channel(4);
        let (updates, received) = async_channel::bounded(4);
        let alive = Arc::new(AtomicBool::new(true));
        let worker_alive = alive.clone();
        let thread =
            std::thread::spawn(move || worker(environment, true, incoming, updates, worker_alive));
        work.send(Work::Trash(path.clone())).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        let snapshot = loop {
            if let Ok(Update::Snapshot(snapshot)) = received.try_recv() {
                break snapshot;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5))
        };
        assert!(snapshot.directory.is_none());
        assert!(snapshot
            .entries
            .iter()
            .all(|entry| entry.path.starts_with("/clarion-preview")));
        assert_eq!(fs::read(path).unwrap(), b"unchanged");
        assert!(!root.path().join("config").exists());
        alive.store(false, Ordering::Relaxed);
        drop(work);
        thread.join().unwrap();
    }
}
