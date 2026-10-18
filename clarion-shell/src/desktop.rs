// SPDX-License-Identifier: GPL-3.0-or-later
//! Native desktop files and wallpaper. Filesystem scans, image decoding and file
//! operations run on a bounded worker; rendering uses only immutable snapshots.
//! The former per-output desktop layout files are intentionally never rewritten.
use std::collections::{BTreeSet, HashMap};
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
const MAX_PREVIEW_TEXT: u64 = 256 * 1024;
const MAX_OPEN_SELECTION: usize = 16;

actions!(
    clarion_desktop,
    [
        OpenSelection,
        RenameSelection,
        TrashSelection,
        SelectAllEntries,
        GetInfo,
        QuickLook,
        NewFolder,
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

struct Inspector {
    title: String,
    rows: Vec<(String, String)>,
    image: Option<Arc<Image>>,
    text: Option<String>,
}

enum Work {
    Inspect {
        generation: u64,
        paths: Vec<PathBuf>,
        quick_look: bool,
    },
    CreateFolder(String),
    Refresh,
    Rename {
        path: PathBuf,
        name: String,
    },
    Trash(Vec<PathBuf>),
    Wallpaper(PathBuf),
}
enum Update {
    Snapshot(DesktopSnapshot),
    Error(String),
    Created(PathBuf),
    Inspector {
        generation: u64,
        result: std::result::Result<Inspector, String>,
    },
}

pub struct DesktopView {
    output: String,
    actions: Sender<DesktopAction>,
    preview: bool,
    entries: Vec<DesktopEntry>,
    directory: Option<PathBuf>,
    wallpaper: Option<Arc<Image>>,
    selected: Option<PathBuf>,
    selected_many: BTreeSet<PathBuf>,
    new_folder: bool,
    inspector: Option<Inspector>,
    inspection_generation: u64,
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
            KeyBinding::new("super-o", OpenSelection, Some("ClarionDesktop")),
            KeyBinding::new("super-down", OpenSelection, Some("ClarionDesktop")),
            KeyBinding::new("super-a", SelectAllEntries, Some("ClarionDesktop")),
            KeyBinding::new("super-i", GetInfo, Some("ClarionDesktop")),
            KeyBinding::new("super-y", QuickLook, Some("ClarionDesktop")),
            KeyBinding::new("super-backspace", TrashSelection, Some("ClarionDesktop")),
            KeyBinding::new("super-shift-n", NewFolder, Some("ClarionDesktop")),
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
                                this.selected_many.retain(|path| {
                                    this.entries.iter().any(|entry| &entry.path == path)
                                });
                                if this.selected.as_ref().is_some_and(|path| {
                                    !this.entries.iter().any(|entry| &entry.path == path)
                                }) {
                                    this.selected = None;
                                }
                            }
                            Update::Error(error) => this.error = Some(error),
                            Update::Created(path) => {
                                this.selected = Some(path.clone());
                                this.selected_many = [path].into();
                            }
                            Update::Inspector { generation, result } => {
                                if generation == this.inspection_generation {
                                    match result {
                                        Ok(inspector) => this.inspector = Some(inspector),
                                        Err(error) => this.error = Some(error),
                                    }
                                }
                            }
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
            selected_many: BTreeSet::new(),
            new_folder: false,
            inspector: None,
            inspection_generation: 0,
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
        if self.busy || self.rename.is_some() || self.new_folder || self.inspector.is_some() {
            return;
        }
        if self.preview {
            self.error = Some("预览模式不会修改文件".into());
            cx.notify();
            return;
        }
        if self.selected_many.len() != 1 {
            self.error = Some("请选择一个项目进行重命名".into());
            cx.notify();
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
        if self.inspector.is_some() {
            return;
        }
        if self.new_folder {
            if self.input.read(cx).has_preedit() {
                return;
            }
            if let Err(error) = validate_name(&self.rename_name) {
                self.error = Some(error.to_string());
                cx.notify();
                return;
            }
            self.new_folder = false;
            window.focus(&self.focus);
            self.enqueue(Work::CreateFolder(self.rename_name.clone()), cx);
        } else if let Some(path) = self.rename.clone() {
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
        } else {
            let paths = self.selection();
            if paths.len() > MAX_OPEN_SELECTION {
                self.error = Some(format!(
                    "一次最多打开 {MAX_OPEN_SELECTION} 个项目，请缩小选择范围"
                ));
                cx.notify();
                return;
            }
            for path in paths {
                self.send(DesktopAction::Open(path), cx);
            }
        }
    }

    fn selection(&self) -> Vec<PathBuf> {
        self.entries
            .iter()
            .filter(|entry| self.selected_many.contains(&entry.path))
            .map(|entry| entry.path.clone())
            .collect()
    }

    fn select_all(&mut self, cx: &mut Context<Self>) {
        if self.rename.is_some() || self.new_folder || self.inspector.is_some() {
            return;
        }
        self.selected_many = self
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect();
        self.selected = self.entries.first().map(|entry| entry.path.clone());
        self.menu = None;
        cx.notify();
    }

    fn inspect(&mut self, quick_look: bool, cx: &mut Context<Self>) {
        if self.rename.is_some() || self.new_folder {
            return;
        }
        if quick_look && self.inspector.is_some() {
            self.inspector = None;
            self.inspection_generation = self.inspection_generation.wrapping_add(1);
            cx.notify();
            return;
        }
        let paths = self.selection();
        if paths.is_empty() {
            self.error = Some("请先选择桌面项目".into());
            cx.notify();
            return;
        }
        self.inspection_generation = self.inspection_generation.wrapping_add(1);
        self.menu = None;
        self.enqueue(
            Work::Inspect {
                generation: self.inspection_generation,
                paths,
                quick_look,
            },
            cx,
        );
    }

    fn begin_new_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.rename.is_some() || self.new_folder || self.inspector.is_some() {
            return;
        }
        if self.preview || self.directory.is_none() {
            self.error = Some("预览模式不会创建文件夹".into());
            cx.notify();
            return;
        }
        self.rename_name = available_folder_name(&self.entries);
        self.new_folder = true;
        self.menu = None;
        self.selected = None;
        self.selected_many.clear();
        self.input
            .update(cx, |input, cx| input.set_text(&self.rename_name, true, cx));
        window.focus(&self.input.focus_handle(cx));
        cx.notify();
    }

    fn trash(&mut self, cx: &mut Context<Self>) {
        if self.rename.is_none() && !self.new_folder && self.inspector.is_none() && !self.busy {
            let paths = self.selection();
            if !paths.is_empty() {
                self.menu = None;
                self.enqueue(Work::Trash(paths), cx);
            }
        }
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.rename = None;
        self.new_folder = false;
        self.inspector = None;
        self.inspection_generation = self.inspection_generation.wrapping_add(1);
        self.error = None;
        window.focus(&self.focus);
        cx.notify();
    }

    fn step(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.rename.is_some()
            || self.new_folder
            || self.inspector.is_some()
            || self.entries.is_empty()
        {
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
        self.selected_many = [self.entries[index].path.clone()].into();
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
                px(f32::from(position.y).clamp(
                    top + 8.,
                    (height - bottom - if self.selected.is_some() { 430. } else { 270. })
                        .max(top + 8.),
                )),
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
            .on_action(cx.listener(|this, _: &SelectAllEntries, _, cx| this.select_all(cx)))
            .on_action(cx.listener(|this, _: &GetInfo, _, cx| this.inspect(false, cx)))
            .on_action(cx.listener(|this, _: &QuickLook, _, cx| this.inspect(true, cx)))
            .on_action(
                cx.listener(|this, _: &NewFolder, window, cx| this.begin_new_folder(window, cx)),
            )
            .on_action(cx.listener(|this, _: &Cancel, window, cx| this.cancel(window, cx)))
            .on_action(cx.listener(|this, _: &NextSelection, _, cx| this.step(true, cx)))
            .on_action(cx.listener(|this, _: &PreviousSelection, _, cx| this.step(false, cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.selected = None;
                    this.selected_many.clear();
                    this.menu = None;
                    this.rename = None;
                    this.new_folder = false;
                    window.focus(&this.focus);
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.selected = None;
                    this.selected_many.clear();
                    this.rename = None;
                    this.new_folder = false;
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
        if self.new_folder {
            grid = grid.child(
                div()
                    .w(px(140.))
                    .min_h(px(90.))
                    .p_2()
                    .rounded(px(8.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .bg(rgba(0x0a84ff66))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(Self::file_icon(EntryKind::Directory))
                    .child(
                        div()
                            .w_full()
                            .px_1()
                            .py_1()
                            .rounded(px(4.))
                            .bg(rgb(0x202b37))
                            .child(self.input.clone()),
                    ),
            );
        }
        for (index, entry) in self.entries.iter().enumerate() {
            let path = entry.path.clone();
            let context_path = path.clone();
            let selected = self.selected_many.contains(&path);
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
                        this.new_folder = false;
                        if event.modifiers.platform || event.modifiers.control {
                            if !this.selected_many.remove(&path) {
                                this.selected_many.insert(path.clone());
                            }
                            this.selected = this.selected_many.last().cloned();
                        } else {
                            this.selected = Some(path.clone());
                            this.selected_many = [path.clone()].into();
                        }
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
                        this.new_folder = false;
                        this.selected = Some(context_path.clone());
                        if !this.selected_many.contains(&context_path) {
                            this.selected_many = [context_path.clone()].into();
                        }
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
                .max_h(px((height - top - bottom - 16.).max(80.)))
                .overflow_y_scroll()
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
                        |this, window, cx| this.activate(window, cx),
                        cx,
                    ))
                    .child(self.context_button(
                        "desktop-info",
                        "显示简介",
                        |this, _, cx| this.inspect(false, cx),
                        cx,
                    ))
                    .child(self.context_button(
                        "desktop-quick-look",
                        "快速查看",
                        |this, _, cx| this.inspect(true, cx),
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
                    "desktop-new-folder",
                    "新建文件夹…",
                    |this, window, cx| this.begin_new_folder(window, cx),
                    cx,
                ))
                .child(self.context_button(
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
        if let Some(inspector) = &self.inspector {
            let panel_width = (width - 64.).clamp(220., 760.);
            let panel_height = (height - top - bottom - 32.).clamp(180., 640.);
            let mut content = div()
                .id("desktop-inspector-content")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap_3()
                .p_5();
            if let Some(image) = &inspector.image {
                content = content.child(
                    img(image.clone())
                        .w_full()
                        .h(px((panel_height - 140.).max(80.)))
                        .object_fit(ObjectFit::Contain),
                );
            }
            if let Some(text) = &inspector.text {
                content =
                    content.child(div().text_sm().font_family("monospace").child(text.clone()));
            }
            for (label, value) in &inspector.rows {
                content = content.child(
                    div()
                        .flex()
                        .gap_4()
                        .text_sm()
                        .child(
                            div()
                                .w(px(82.))
                                .flex_none()
                                .text_color(rgb(0xaab6c7))
                                .child(label.clone()),
                        )
                        .child(div().flex_1().min_w_0().child(value.clone())),
                );
            }
            root = root.child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(rgba(0x00000032))
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.inspector = None;
                            this.inspection_generation = this.inspection_generation.wrapping_add(1);
                            cx.notify();
                            cx.stop_propagation();
                        }),
                    )
                    .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .id("desktop-inspector")
                            .w(px(panel_width))
                            .max_h(px(panel_height))
                            .rounded(px(16.))
                            .overflow_hidden()
                            .bg(rgba(0x202b38f5))
                            .border_1()
                            .border_color(rgba(0xffffff32))
                            .shadow_lg()
                            .flex()
                            .flex_col()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .px_4()
                                    .py_3()
                                    .border_b_1()
                                    .border_color(rgba(0xffffff18))
                                    .child(
                                        div()
                                            .id("desktop-inspector-close")
                                            .size(px(12.))
                                            .rounded_full()
                                            .bg(rgb(0xff5f57))
                                            .cursor_pointer()
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.inspector = None;
                                                this.inspection_generation =
                                                    this.inspection_generation.wrapping_add(1);
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_sm()
                                            .overflow_hidden()
                                            .child(inspector.title.clone()),
                                    ),
                            )
                            .child(content),
                    ),
            );
        }
        root
    }
}

fn available_folder_name(entries: &[DesktopEntry]) -> String {
    for index in 1..=MAX_ENTRIES + 1 {
        let name = if index == 1 {
            "未命名文件夹".into()
        } else {
            format!("未命名文件夹 {index}")
        };
        if !entries.iter().any(|entry| entry.name == name) {
            return name;
        }
    }
    unreachable!("the bounded entry list always leaves a candidate")
}

fn create_folder(directory: &Path, name: &str) -> Result<PathBuf> {
    validate_name(name)?;
    let path = directory.join(name);
    fs::create_dir(&path).context("创建文件夹失败，现有同名项目不会被覆盖")?;
    Ok(path)
}

fn trash_entries(
    directory: &Path,
    paths: &[PathBuf],
    environment: &HashMap<String, String>,
) -> Result<()> {
    anyhow::ensure!(
        !paths.is_empty() && paths.len() <= MAX_ENTRIES,
        "选择的项目数量无效"
    );
    // Validate the complete batch before handing literal paths to GIO. A single
    // bounded process avoids one service startup and timeout per selected file.
    for path in paths {
        validate_source(directory, path)?;
    }
    let mut child = Command::new("gio")
        .arg("trash")
        .arg("--")
        .args(paths)
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
                anyhow::ensure!(
                    status.success(),
                    "部分项目可能已移到废纸篓，请刷新桌面检查；未使用直接删除"
                );
                return Ok(());
            }
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                anyhow::bail!("废纸篓操作超时，请刷新检查项目是否已移走")
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

fn display_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} 字节")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB（{bytes} 字节）", bytes as f64 / 1024.)
    } else {
        format!("{:.1} MB（{bytes} 字节）", bytes as f64 / (1024. * 1024.))
    }
}

fn inspect_paths(directory: &Path, paths: &[PathBuf], quick_look: bool) -> Result<Inspector> {
    anyhow::ensure!(
        !paths.is_empty() && paths.len() <= MAX_ENTRIES,
        "请选择桌面项目"
    );
    for path in paths {
        validate_source(directory, path)?;
    }
    if quick_look {
        anyhow::ensure!(
            paths.len() == 1,
            "快速查看一次只预览一个项目，请缩小选择范围"
        );
    }
    let mut inspector = Inspector {
        title: "显示简介".into(),
        rows: Vec::new(),
        image: None,
        text: None,
    };
    if paths.len() > 1 {
        let mut bytes = 0u64;
        let mut files = 0;
        let mut directories = 0;
        let mut links = 0;
        for path in paths {
            let metadata = fs::symlink_metadata(path)?;
            if metadata.is_symlink() {
                links += 1;
            } else if metadata.is_dir() {
                directories += 1;
            } else if metadata.is_file() {
                files += 1;
                bytes = bytes.saturating_add(metadata.len());
            }
        }
        inspector.title = format!("{} 个项目的简介", paths.len());
        inspector.rows = vec![
            (
                "包含".into(),
                format!("{files} 个文件、{directories} 个文件夹、{links} 个符号链接"),
            ),
            ("文件大小".into(), display_size(bytes)),
            (
                "统计范围".into(),
                "仅统计选中文件，不递归读取文件夹内容".into(),
            ),
            ("位置".into(), directory.display().to_string()),
        ];
        return Ok(inspector);
    }
    let path = &paths[0];
    let metadata = fs::symlink_metadata(path)?;
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let kind = if metadata.is_symlink() {
        "符号链接"
    } else if metadata.is_dir() {
        "文件夹"
    } else if metadata.is_file() {
        "文件"
    } else {
        "特殊文件"
    };
    inspector.title = if quick_look {
        name.clone()
    } else {
        format!("{name} 简介")
    };
    inspector.rows = vec![
        ("名称".into(), name),
        ("种类".into(), kind.into()),
        ("位置".into(), path.display().to_string()),
    ];
    if metadata.is_file() {
        inspector
            .rows
            .push(("大小".into(), display_size(metadata.len())));
    }
    if let Ok(modified) = metadata.modified() {
        let modified: chrono::DateTime<chrono::Local> = modified.into();
        inspector.rows.push((
            "修改时间".into(),
            modified.format("%Y年%m月%d日 %H:%M:%S").to_string(),
        ));
    }
    inspector.rows.push((
        "权限".into(),
        format!(
            "{:03o}{}",
            metadata.permissions().mode() & 0o777,
            if metadata.permissions().readonly() {
                "（只读）"
            } else {
                ""
            }
        ),
    ));
    if metadata.is_symlink() {
        inspector.rows.push((
            "链接目标".into(),
            fs::read_link(path)?.display().to_string(),
        ));
    }
    if quick_look {
        anyhow::ensure!(
            metadata.is_file(),
            "快速查看支持普通图片和 UTF-8 文本；该项目是{kind}，可使用显示简介查看属性"
        );
        let extension = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if matches!(extension.as_str(), "png" | "jpg" | "jpeg") {
            inspector.image = Some(
                decode_image(path, 16 * 1024 * 1024, 1600, 1000, libc::O_NOFOLLOW)
                    .context("无法快速查看此图片")?,
            );
        } else {
            let bytes = read_regular_with_flags(path, MAX_PREVIEW_TEXT, libc::O_NOFOLLOW)
                .context("文本快速查看上限为256 KB；较大文件请使用默认应用打开")?;
            let text = String::from_utf8(bytes)
                .context("该格式不支持快速查看，目前支持 PNG、JPEG 和 UTF-8 文本")?;
            anyhow::ensure!(
                !text
                    .chars()
                    .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t')),
                "该文件包含二进制内容，不能作为文本快速查看"
            );
            inspector.text = Some(text);
        }
    }
    Ok(inspector)
}

fn read_regular(path: &Path, limit: u64) -> Result<Vec<u8>> {
    read_regular_with_flags(path, limit, 0)
}

fn read_regular_with_flags(path: &Path, limit: u64, flags: i32) -> Result<Vec<u8>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC | flags)
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
        "只能操作桌面文件夹中的项目"
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

#[cfg(test)]
fn trash_entry(directory: &Path, path: &Path, environment: &HashMap<String, String>) -> Result<()> {
    trash_entries(directory, &[path.to_path_buf()], environment)
}

fn image_data(path: &Path) -> Result<Arc<Image>> {
    decode_image(path, MAX_IMAGE_BYTES, 3840, 2160, 0)
}

fn decode_image(
    path: &Path,
    byte_limit: u64,
    width: u32,
    height: u32,
    flags: i32,
) -> Result<Arc<Image>> {
    anyhow::ensure!(path.is_absolute(), "图像必须使用绝对路径");
    let bytes = read_regular_with_flags(path, byte_limit, flags)?;
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
    let image = image.thumbnail(width, height);
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
                        Work::Trash(paths) => trash_entries(&directory, &paths, &environment),
                        Work::CreateFolder(name) => create_folder(&directory, &name).map(|path| {
                            let _ = updates.send_blocking(Update::Created(path));
                        }),
                        Work::Inspect {
                            generation,
                            paths,
                            quick_look,
                        } => {
                            let result = inspect_paths(&directory, &paths, quick_look)
                                .map_err(|error| format!("{error:#}"));
                            let _ = updates.send_blocking(Update::Inspector { generation, result });
                            continue;
                        }
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
    use ::core::prelude::v1::test;
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
    fn new_folder_is_literal_and_never_replaces_an_existing_entry() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("Desktop");
        fs::create_dir(&directory).unwrap();
        let first = create_folder(&directory, "未命名文件夹").unwrap();
        fs::write(first.join("keep.txt"), b"keep").unwrap();
        assert!(create_folder(&directory, "未命名文件夹").is_err());
        assert_eq!(
            available_folder_name(&scan(&directory).unwrap()),
            "未命名文件夹 2"
        );
        let literal = create_folder(&directory, "$(touch nothing) with spaces").unwrap();
        assert!(literal.is_dir());
        for name in ["", ".", "..", "../outside", "nested/folder"] {
            assert!(create_folder(&directory, name).is_err());
        }
        assert_eq!(fs::read(first.join("keep.txt")).unwrap(), b"keep");
        assert!(!root.path().join("outside").exists());
    }

    #[::core::prelude::v1::test]
    fn info_is_read_only_and_does_not_follow_links_or_scan_directory_contents() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("Desktop");
        fs::create_dir(&directory).unwrap();
        let file = directory.join("hello.txt");
        fs::write(&file, "你好，Clarion").unwrap();
        let folder = create_folder(&directory, "folder").unwrap();
        fs::write(folder.join("not-counted"), b"nested bytes").unwrap();
        let link = directory.join("external link");
        std::os::unix::fs::symlink(root.path().join("missing target"), &link).unwrap();
        let info = inspect_paths(&directory, std::slice::from_ref(&link), false).unwrap();
        assert!(info
            .rows
            .iter()
            .any(|(label, value)| label == "种类" && value == "符号链接"));
        assert!(info.rows.iter().any(|(label, _)| label == "链接目标"));
        let info = inspect_paths(&directory, &[file.clone(), folder, link.clone()], false).unwrap();
        assert!(info
            .rows
            .iter()
            .any(|(_, value)| value == "1 个文件、1 个文件夹、1 个符号链接"));
        assert!(info
            .rows
            .iter()
            .any(|(label, value)| label == "文件大小" && value.starts_with("16 字节")));
        assert!(inspect_paths(&directory, &[link], true).is_err());
        assert!(inspect_paths(&directory, &[root.path().join("outside")], false).is_err());
        assert_eq!(fs::read_to_string(file).unwrap(), "你好，Clarion");
    }

    #[::core::prelude::v1::test]
    fn quick_look_previews_bounded_text_and_png_and_rejects_binary_special_and_large_files() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("Desktop");
        fs::create_dir(&directory).unwrap();
        let text = directory.join("text.txt");
        fs::write(&text, "Hello\n你好\tClarion").unwrap();
        let preview = inspect_paths(&directory, std::slice::from_ref(&text), true).unwrap();
        assert_eq!(preview.text.as_deref(), Some("Hello\n你好\tClarion"));
        assert!(preview.image.is_none());
        let image = directory.join("sample.png");
        image::RgbaImage::from_pixel(4, 4, image::Rgba([20, 80, 100, 255]))
            .save(&image)
            .unwrap();
        assert!(inspect_paths(&directory, &[image], true)
            .unwrap()
            .image
            .is_some());
        let binary = directory.join("binary.pdf");
        fs::write(&binary, [0, 1, 2, 3]).unwrap();
        assert!(inspect_paths(&directory, &[binary], true).is_err());
        let large = directory.join("large.txt");
        File::create(&large)
            .unwrap()
            .set_len(MAX_PREVIEW_TEXT + 1)
            .unwrap();
        assert!(inspect_paths(&directory, &[large], true).is_err());
        let fifo = directory.join("fifo.txt");
        let raw = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(raw.as_ptr(), 0o600) }, 0);
        assert!(inspect_paths(&directory, &[fifo], true).is_err());
        assert!(inspect_paths(&directory, &[text.clone(), text], true).is_err());
    }

    #[gpui::test]
    fn forwarded_desktop_shortcuts_dispatch_real_actions_and_inline_folder_creation(
        cx: &mut TestAppContext,
    ) {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("Desktop");
        fs::create_dir(&directory).unwrap();
        for name in ["first.txt", "second.txt"] {
            fs::write(directory.join(name), name).unwrap();
        }
        let entries = scan(&directory).unwrap();
        let (actions, received_actions) = async_channel::unbounded();
        let (work, received_work) = mpsc::sync_channel(16);
        cx.update(|cx| {
            TextInput::bind_keys(cx);
            DesktopView::bind_keys(cx);
        });
        let (view, visual) = cx.add_window_view(|window, cx| {
            let input = cx.new(TextInput::new);
            cx.subscribe(
                &input,
                |this: &mut DesktopView, _, QueryChanged(name), cx| {
                    this.rename_name = name.clone();
                    cx.notify();
                },
            )
            .detach();
            let focus = cx.focus_handle();
            window.focus(&focus);
            window.activate_window();
            DesktopView {
                output: "isolated".into(),
                actions,
                preview: false,
                entries,
                directory: Some(directory.clone()),
                wallpaper: None,
                selected: None,
                selected_many: BTreeSet::new(),
                new_folder: false,
                inspector: None,
                inspection_generation: 0,
                menu: None,
                rename: None,
                rename_name: String::new(),
                input,
                focus,
                error: None,
                busy: false,
                work,
                alive: Arc::new(AtomicBool::new(true)),
                scroll: ScrollHandle::new(),
                reserved_edges: [0.; 4],
            }
        });
        visual.simulate_keystrokes("super-a");
        assert_eq!(
            view.read_with(visual, |view, _| view.selected_many.len()),
            2
        );
        for shortcut in ["super-o", "super-down"] {
            visual.simulate_keystrokes(shortcut);
            for name in ["first.txt", "second.txt"] {
                assert!(
                    matches!(received_actions.try_recv(), Ok(DesktopAction::Open(path)) if path == directory.join(name))
                );
            }
        }
        visual.simulate_keystrokes("super-i");
        assert!(
            matches!(received_work.try_recv(), Ok(Work::Inspect { quick_look: false, paths, .. }) if paths.len() == 2)
        );
        view.update(visual, |view, _| view.busy = false);
        visual.simulate_keystrokes("super-y");
        assert!(
            matches!(received_work.try_recv(), Ok(Work::Inspect { quick_look: true, paths, .. }) if paths.len() == 2)
        );
        assert!(
            received_actions.try_recv().is_err(),
            "Quick Look must not silently launch an app"
        );
        view.update(visual, |view, _| view.busy = false);
        visual.simulate_keystrokes("super-backspace");
        assert!(matches!(received_work.try_recv(), Ok(Work::Trash(paths)) if paths.len() == 2));
        view.update(visual, |view, _| view.busy = false);
        visual.simulate_keystrokes("super-shift-n");
        assert!(view.read_with(visual, |view, _| view.new_folder));
        visual.update(|window, cx| {
            let input = view.read(cx).input.clone();
            input.update(cx, |input, cx| {
                input.replace_and_mark_text_in_range(None, "资料", None, window, cx)
            });
        });
        visual.simulate_keystrokes("enter");
        assert!(view.read_with(visual, |view, _| view.new_folder));
        assert!(
            received_work.try_recv().is_err(),
            "IME confirmation must not create a folder"
        );
        visual.update(|window, cx| {
            let input = view.read(cx).input.clone();
            input.update(cx, |input, cx| input.unmark_text(window, cx));
        });
        visual.simulate_keystrokes("ctrl-a");
        visual.simulate_input("新建工作资料");
        visual.simulate_keystrokes("enter");
        assert!(
            matches!(received_work.try_recv(), Ok(Work::CreateFolder(name)) if name == "新建工作资料")
        );
        assert!(
            !directory.join("新建工作资料").exists(),
            "headless test queues commands without a filesystem worker"
        );
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
        let second = directory.join("another literal --filename");
        fs::write(&second, b"second").unwrap();
        trash_entries(&directory, &[path.clone(), second.clone()], &environment).unwrap();
        let batch: Vec<String> = serde_json::from_slice(&fs::read(&capture).unwrap()).unwrap();
        assert_eq!(
            batch,
            vec![
                "trash".to_string(),
                "--".into(),
                path.to_string_lossy().into_owned(),
                second.to_string_lossy().into_owned()
            ]
        );
        let before = fs::read(&capture).unwrap();
        assert!(trash_entries(
            &directory,
            &[path.clone(), root.path().join("outside")],
            &environment
        )
        .is_err());
        assert_eq!(
            fs::read(&capture).unwrap(),
            before,
            "the complete batch must validate before GIO starts"
        );
        assert_eq!(fs::read(second).unwrap(), b"second");
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
        work.send(Work::Trash(vec![path.clone()])).unwrap();
        work.send(Work::CreateFolder("must-not-exist".into()))
            .unwrap();
        work.send(Work::Inspect {
            generation: 1,
            paths: vec![path.clone()],
            quick_look: true,
        })
        .unwrap();
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
        assert!(!root.path().join("must-not-exist").exists());
        alive.store(false, Ordering::Relaxed);
        drop(work);
        thread.join().unwrap();
    }
}
