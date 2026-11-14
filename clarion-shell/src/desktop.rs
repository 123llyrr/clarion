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
use gpui::prelude::*;
use gpui::*;
use image::ImageReader;
use unicode_segmentation::UnicodeSegmentation;

use crate::desktop_menu::{DesktopMenuView, MenuColors};
use crate::input::{QueryChanged, TextInput};
use crate::preferences::SurfaceAppearance;

const MAX_ENTRIES: usize = 500;
const MAX_VISITED: usize = 4096;
const MAX_IMAGE_BYTES: u64 = 64 * 1024 * 1024;
const CONFIG_LIMIT: u64 = 65536;
const MAX_OPEN_SELECTION: usize = 16;
const DESKTOP_MARGIN: f32 = 16.;
const DESKTOP_CELL_WIDTH: f32 = 112.;
const DESKTOP_CELL_HEIGHT: f32 = 112.;
const DESKTOP_ICON_SIZE: f32 = 64.;
const DESKTOP_LABEL_WIDTH: f32 = 104.;
/// The inline rename/new-folder field. One compact capsule for both so a
/// tile never grows a control larger than its own grid cell.
const DESKTOP_EDIT_FIELD_HEIGHT: f32 = 28.;
const DESKTOP_EDIT_FIELD_RADIUS: f32 = 7.;

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
        SelectUp,
        SelectDown,
        SelectLeft,
        SelectRight,
        ExtendUp,
        ExtendDown,
        ExtendLeft,
        ExtendRight
    ]
);

#[derive(Clone, Debug)]
pub enum DesktopAction {
    Open(PathBuf),
    /// Open one file through the desktop's application chooser.
    OpenWith(PathBuf),
    /// Show one file in the shell's quick previewer (GNOME Sushi, the same
    /// window Nautilus opens on space).
    Preview(PathBuf),
    Settings,
    /// None asks the host to choose an image; Some supplies a desktop image.
    Wallpaper(Option<PathBuf>),
    /// Show one path in the file manager (its folder for a file).
    Reveal(PathBuf),
    /// Move one path (from any folder) to the trash.
    Trash(PathBuf),
    /// Show the finished inspector card as a modal on the desktop's output.
    Info {
        output: String,
        path: Option<PathBuf>,
        title: String,
        rows: Vec<(String, String)>,
    },
    /// Ask the host to surface a hint through the session's notification
    /// service. The desktop itself never paints a banner over the files.
    Notify { body: String },
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
    /// Finder-style tags (`user.xdg.tags`), drawn as dots on the artwork.
    tags: Vec<String>,
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
    /// The file the card belongs to; `None` for a multi-selection.
    path: Option<PathBuf>,
}

enum Work {
    Inspect {
        generation: u64,
        paths: Vec<PathBuf>,
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

#[derive(Clone, Copy, Debug)]
enum SelectionDirection {
    Up,
    Down,
    Left,
    Right,
}

/// All desktop geometry is in output-local logical pixels. Painting, marquee
/// hit testing and arrow navigation share this column-major layout.
#[derive(Clone, Copy, Debug)]
struct DesktopGrid {
    origin: Point<Pixels>,
    width: f32,
    height: f32,
    rows: usize,
    cell_width: f32,
}

impl DesktopGrid {
    fn new(viewport: Size<Pixels>, edges: [f32; 4], count: usize) -> Self {
        let [top, bottom, left, right] = edges;
        let width = (f32::from(viewport.width) - left - right - 2. * DESKTOP_MARGIN).max(1.);
        let height = (f32::from(viewport.height) - top - bottom - 2. * DESKTOP_MARGIN).max(1.);
        let visible_rows = (height / DESKTOP_CELL_HEIGHT).floor().max(1.) as usize;
        let columns = (width / DESKTOP_CELL_WIDTH).floor().max(1.) as usize;
        // Keep every entry reachable if a small output has more files than can
        // fit. Ordinary desktops do not scroll or spread across the top row.
        let rows = visible_rows.max(count.div_ceil(columns));
        Self {
            origin: point(px(left + DESKTOP_MARGIN), px(top + DESKTOP_MARGIN)),
            width,
            height,
            rows,
            cell_width: DESKTOP_CELL_WIDTH.min(width),
        }
    }

    fn cell(&self, index: usize) -> Bounds<Pixels> {
        let column = index / self.rows;
        let row = index % self.rows;
        Bounds::new(
            point(
                px((self.width - (column + 1) as f32 * self.cell_width).max(0.)),
                px(row as f32 * DESKTOP_CELL_HEIGHT),
            ),
            size(px(self.cell_width), px(DESKTOP_CELL_HEIGHT)),
        )
    }

    fn content_height(&self) -> f32 {
        self.height.max(self.rows as f32 * DESKTOP_CELL_HEIGHT)
    }

    fn tile(&self, index: usize) -> Bounds<Pixels> {
        let cell = self.cell(index);
        Bounds::new(
            cell.origin + point(px(4.), px(2.)),
            size((cell.size.width - px(8.)).max(px(1.)), px(106.)),
        )
    }

    fn local_position(&self, position: Point<Pixels>, offset: Point<Pixels>) -> Point<Pixels> {
        position - self.origin - offset
    }

    fn contains(&self, position: Point<Pixels>) -> bool {
        Bounds::new(self.origin, size(px(self.width), px(self.height))).contains(&position)
    }

    fn neighbor(&self, current: usize, count: usize, direction: SelectionDirection) -> usize {
        let row = current % self.rows;
        let column = current / self.rows;
        match direction {
            SelectionDirection::Up if row > 0 => current - 1,
            SelectionDirection::Down if row + 1 < self.rows && current + 1 < count => current + 1,
            SelectionDirection::Right if column > 0 => current - self.rows,
            SelectionDirection::Left if (column + 1) * self.rows < count => {
                (current + self.rows).min(count - 1)
            }
            _ => current,
        }
    }
}

struct MarqueeSelection {
    start: Point<Pixels>,
    end: Point<Pixels>,
    base: BTreeSet<PathBuf>,
}

impl MarqueeSelection {
    fn bounds(&self) -> Bounds<Pixels> {
        Bounds::from_corners(
            point(self.start.x.min(self.end.x), self.start.y.min(self.end.y)),
            point(self.start.x.max(self.end.x), self.start.y.max(self.end.y)),
        )
    }

    fn visible(&self) -> bool {
        (self.start.x - self.end.x).abs() + (self.start.y - self.end.y).abs() >= px(3.)
    }
}

/// Two explicitly measured lines, preserving a long filename's identifying
/// suffix. Clip-only labels made similarly prefixed downloads indistinguishable.
fn desktop_label(name: &str, width: f32, measure: impl Fn(&str) -> f32) -> Vec<String> {
    let name: String = name
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect();
    if measure(&name) <= width {
        return vec![name];
    }
    let graphemes: Vec<&str> = name.graphemes(true).collect();
    let fit_prefix = |parts: &[&str], available: f32| {
        let mut low = 0;
        let mut high = parts.len();
        while low < high {
            let middle = (low + high).div_ceil(2);
            if measure(&parts[..middle].concat()) <= available {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        low
    };
    let first_count = fit_prefix(&graphemes, width).max(1);
    let first = graphemes[..first_count].concat();
    let remaining = &graphemes[first_count..];
    if measure(&remaining.concat()) <= width {
        return vec![first, remaining.concat()];
    }
    let mut suffix_start = remaining.len();
    while suffix_start > 0 && measure(&remaining[suffix_start - 1..].concat()) <= width * 0.72 {
        suffix_start -= 1;
    }
    let suffix = remaining[suffix_start..].concat();
    let prefix_count = fit_prefix(
        &remaining[..suffix_start],
        (width - measure(&format!("…{suffix}"))).max(0.),
    );
    vec![
        first,
        format!("{}…{suffix}", remaining[..prefix_count].concat()),
    ]
}

fn label_width_fn(text: &str, window: &Window) -> f32 {
    let mut label_font = font("Noto Sans");
    label_font.weight = FontWeight::MEDIUM;
    let run = TextRun {
        len: text.len(),
        font: label_font,
        color: rgb(0xffffff).into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    f32::from(
        window
            .text_system()
            .shape_line(text.to_owned().into(), px(12.), &[run], None)
            .width,
    )
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
    selection_anchor: Option<PathBuf>,
    marquee: Option<MarqueeSelection>,
    labels: HashMap<(String, u32), Vec<String>>,
    new_folder: bool,
    inspector: Option<Inspector>,
    inspection_generation: u64,
    menu: Option<Point<Pixels>>,
    /// The path a menu opened for a Dock folder entry acts on. `None` means
    /// the menu belongs to the desktop grid and uses the grid selection.
    menu_target: Option<PathBuf>,
    /// The overlay surface painting the context menu. It is a window of its
    /// own so the compositor can frost the desktop behind it.
    menu_window: Option<WindowHandle<DesktopMenuView>>,
    /// This window's handle, captured when the menu first opens so closing it
    /// can hand activation back to the desktop.
    own_window: Option<WindowHandle<DesktopView>>,
    rename: Option<PathBuf>,
    rename_name: String,
    input: Entity<TextInput>,
    focus: FocusHandle,
    /// The last hint already handed to the notification service. A repeated
    /// failure (a rescan that keeps hitting the same unreadable directory) is
    /// posted once until something succeeds in between.
    last_notice: Option<String>,
    busy: bool,
    work: mpsc::SyncSender<Work>,
    alive: Arc<AtomicBool>,
    scroll: ScrollHandle,
    reserved_edges: [f32; 4],
    appearance: SurfaceAppearance,
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
            KeyBinding::new("right", SelectRight, Some("ClarionDesktop")),
            KeyBinding::new("down", SelectDown, Some("ClarionDesktop")),
            KeyBinding::new("left", SelectLeft, Some("ClarionDesktop")),
            KeyBinding::new("up", SelectUp, Some("ClarionDesktop")),
            KeyBinding::new("shift-right", ExtendRight, Some("ClarionDesktop")),
            KeyBinding::new("shift-down", ExtendDown, Some("ClarionDesktop")),
            KeyBinding::new("shift-left", ExtendLeft, Some("ClarionDesktop")),
            KeyBinding::new("shift-up", ExtendUp, Some("ClarionDesktop")),
        ]);
    }

    pub fn new(
        output: String,
        environment: HashMap<String, String>,
        actions: Sender<DesktopAction>,
        preview: bool,
        appearance: SurfaceAppearance,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            TextInput::new(cx)
                .with_placeholder("文件名称")
                .with_capsule(appearance.dark)
        });
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
        if let Some(error) = startup_error {
            let _ = actions.try_send(DesktopAction::Notify { body: error });
        }
        cx.spawn(async move |this, cx| {
            while let Ok(update) = received.recv().await {
                if this
                    .update(cx, |this, cx| {
                        this.busy = false;
                        match update {
                            Update::Snapshot(snapshot) => {
                                this.entries = snapshot.entries;
                                this.labels.retain(|(name, _), _| {
                                    this.entries.iter().any(|entry| &entry.name == name)
                                });
                                this.marquee = None;
                                this.directory = snapshot.directory;
                                this.wallpaper = snapshot.wallpaper;
                                match snapshot.error {
                                    Some(error) => this.send_notice(error, cx),
                                    None => this.clear_notice(),
                                }
                                this.selected_many.retain(|path| {
                                    this.entries.iter().any(|entry| &entry.path == path)
                                });
                                if this.selected.as_ref().is_some_and(|path| {
                                    !this.entries.iter().any(|entry| &entry.path == path)
                                }) {
                                    this.selected = None;
                                }
                                if this.selection_anchor.as_ref().is_some_and(|path| {
                                    !this.entries.iter().any(|entry| &entry.path == path)
                                }) {
                                    this.selection_anchor = None;
                                }
                            }
                            Update::Error(error) => this.send_notice(error, cx),
                            Update::Created(path) => {
                                this.clear_notice();
                                this.selected = Some(path.clone());
                                this.selection_anchor = Some(path.clone());
                                this.selected_many = [path].into();
                            }
                            Update::Inspector { generation, result } => {
                                if generation == this.inspection_generation {
                                    match result {
                                        // The card is a modal of its own so it can
                                        // rise above application windows instead
                                        // of living on the desktop layer.
                                        Ok(inspector) => {
                                            let _ = this.actions.try_send(DesktopAction::Info {
                                                output: this.output.clone(),
                                                path: inspector.path,
                                                title: inspector.title,
                                                rows: inspector.rows,
                                            });
                                        }
                                        Err(error) => this.send_notice(error, cx),
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
            selection_anchor: None,
            marquee: None,
            labels: HashMap::new(),
            new_folder: false,
            inspector: None,
            inspection_generation: 0,
            menu: None,
            menu_target: None,
            menu_window: None,
            own_window: None,
            rename: None,
            rename_name: String::new(),
            input,
            focus: cx.focus_handle(),
            last_notice: None,
            busy: false,
            work,
            alive,
            scroll: ScrollHandle::new(),
            reserved_edges: [28., 78., 0., 0.],
            appearance,
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

    /// Mirrors the shell's opacity, frost and theme controls into the desktop
    /// context menu and inspector.
    pub fn set_appearance(&mut self, appearance: SurfaceAppearance, cx: &mut Context<Self>) {
        if self.appearance != appearance {
            self.appearance = appearance;
            self.input
                .update(cx, |input, cx| input.set_dark(appearance.dark, cx));
            if let Some(menu) = self.menu_window {
                let _ = menu.update(cx, |menu, _, cx| menu.set_appearance(appearance, cx));
            }
            cx.notify();
        }
    }

    /// Open the context menu as its own overlay surface. `position` is the
    /// click point in the desktop window's logical coordinates; the overlay
    /// covers the same output, so the card lands where the user clicked while
    /// the compositor can still frost the desktop below it.
    /// Open the desktop's context menu for one file that is not on the
    /// desktop: a Dock folder entry. The file rows act on that path, and the
    /// menu keeps its own frosted overlay.
    pub(crate) fn open_entry_menu(
        &mut self,
        path: PathBuf,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.menu_target = Some(path);
        self.open_menu(position, window, cx);
    }

    fn open_menu(&mut self, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        // Take the target before closing: `close_menu` clears it.
        let target = self.menu_target.take();
        self.close_menu(cx);
        let Some(desktop) = window.window_handle().downcast::<DesktopView>() else {
            return;
        };
        self.own_window = Some(desktop);
        let viewport = window.viewport_size();
        let width = f32::from(viewport.width);
        let height = f32::from(viewport.height);
        let display = window.display(cx).map(|display| display.id());
        let has_selection = target.is_some() || self.selected.is_some();
        let has_image = match &target {
            Some(path) => {
                crate::file_icons::kind(path, path.is_dir()) == crate::file_icons::FileKind::Image
            }
            None => self
                .entries
                .iter()
                .find(|entry| Some(&entry.path) == self.selected.as_ref())
                .is_some_and(|entry| entry.kind == EntryKind::Image),
        };
        let selected = target.clone().or_else(|| self.selected.clone());
        let reserved = self.reserved_edges;
        let appearance = self.appearance;
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(width), px(height)));
        let options = crate::app::window_options(
            "clarion-desktop-menu",
            display,
            bounds,
            false,
            LayerShellAnchor::ALL,
            -1,
            LayerShellKeyboardInteractivity::Exclusive,
            LayerShellMargins::default(),
            LayerShellLayer::Overlay,
        );
        match cx.open_window(options, move |window, cx| {
            let menu = cx.new(|cx| {
                DesktopMenuView::new(
                    desktop,
                    position,
                    width,
                    height,
                    reserved,
                    appearance,
                    has_selection,
                    has_image,
                    selected,
                    target,
                    cx,
                )
            });
            let focus = menu.read(cx).focus_handle();
            window.focus(&focus);
            window.activate_window();
            menu
        }) {
            Ok(menu) => {
                self.menu = Some(position);
                self.menu_window = Some(menu);
            }
            Err(error) => self.send_notice(format!("无法打开桌面菜单：{error}"), cx),
        }
        cx.notify();
    }

    /// Release the menu surface and clear its anchor. The removal is deferred so
    /// a row click can close the very surface that is currently dispatching.
    pub(crate) fn close_menu(&mut self, cx: &mut Context<Self>) {
        self.menu = None;
        self.menu_target = None;
        let own = self.own_window;
        if let Some(menu) = self.menu_window.take() {
            cx.defer(move |cx| {
                let _ = menu.update(cx, |_, window, _| window.remove_window());
                if let Some(own) = own {
                    let _ = own.update(cx, |_, window, _| window.activate_window());
                }
            });
        }
    }

    pub(crate) fn open_desktop_folder(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.directory.clone() {
            self.send(DesktopAction::Open(path), cx)
        }
    }

    /// The host calls this after selecting a file. Validation, image decoding and
    /// atomic preference saving happen on the desktop worker.
    pub fn set_wallpaper(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.enqueue(Work::Wallpaper(path), cx);
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.close_menu(cx);
        self.enqueue(Work::Refresh, cx);
    }

    /// Hints never paint a banner over the user's files: they go through the
    /// session's notification service like any other desktop application. A
    /// message that repeats because the same operation keeps failing is only
    /// posted once until something succeeds in between.
    fn send_notice(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        let message = message.into();
        if self.last_notice.as_deref() == Some(message.as_str()) {
            return;
        }
        self.last_notice = Some(message.clone());
        let _ = self.actions.try_send(DesktopAction::Notify { body: message });
        cx.notify();
    }

    /// The condition that produced the last hint is gone: a later failure is
    /// worth posting again.
    fn clear_notice(&mut self) {
        self.last_notice = None;
    }

    fn enqueue(&mut self, work: Work, cx: &mut Context<Self>) {
        if self.preview && !matches!(work, Work::Refresh) {
            self.send_notice("预览模式不会修改文件", cx);
            return;
        }
        match self.work.try_send(work) {
            Ok(()) => {
                self.busy = true;
                self.clear_notice();
            }
            Err(_) => self.send_notice("文件操作正在处理中，请稍后重试", cx),
        }
        cx.notify();
    }

    pub(crate) fn send(&mut self, action: DesktopAction, cx: &mut Context<Self>) {
        if self.preview {
            self.send_notice("预览模式不会打开或修改用户文件", cx);
        } else if self.actions.try_send(action).is_err() {
            self.send_notice("桌面操作暂时不可用", cx);
        }
        self.close_menu(cx);
        cx.notify();
    }

    pub(crate) fn begin_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.rename.is_some() || self.new_folder || self.inspector.is_some() {
            return;
        }
        if self.preview {
            self.send_notice("预览模式不会修改文件", cx);
            return;
        }
        if self.selected_many.len() != 1 {
            self.send_notice("请选择一个项目进行重命名", cx);
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
        self.close_menu(cx);
        self.rename = Some(entry.path);
        self.rename_name = entry.name.clone();
        self.input
            .update(cx, |input, cx| input.set_text(&entry.name, true, cx));
        window.activate_window();
        window.focus(&self.input.focus_handle(cx));
        cx.notify();
    }

    pub(crate) fn activate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.inspector.is_some() {
            return;
        }
        if self.new_folder {
            if self.input.read(cx).has_preedit() {
                return;
            }
            if let Err(error) = validate_name(&self.rename_name) {
                self.send_notice(error.to_string(), cx);
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
                self.send_notice(error.to_string(), cx);
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
                self.send_notice(
                    format!("一次最多打开 {MAX_OPEN_SELECTION} 个项目，请缩小选择范围"),
                    cx,
                );
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

    pub(crate) fn select_all(&mut self, cx: &mut Context<Self>) {
        if self.rename.is_some() || self.new_folder || self.inspector.is_some() {
            return;
        }
        self.selected_many = self
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect();
        self.selected = self.entries.first().map(|entry| entry.path.clone());
        self.selection_anchor = self.selected.clone();
        self.close_menu(cx);
        cx.notify();
    }

    /// Inspect one path that is not on the desktop: the Dock folder menu knows
    /// its target, so it never has to touch the desktop grid selection.
    pub(crate) fn inspect_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.rename.is_some() || self.new_folder {
            return;
        }
        self.inspection_generation = self.inspection_generation.wrapping_add(1);
        self.close_menu(cx);
        self.enqueue(
            Work::Inspect {
                generation: self.inspection_generation,
                paths: vec![path],
            },
            cx,
        );
    }

    pub(crate) fn inspect(&mut self, quick_look: bool, cx: &mut Context<Self>) {
        if self.rename.is_some() || self.new_folder {
            return;
        }
        let paths = self.selection();
        if quick_look {
            // Quick Look belongs to GNOME Sushi, the previewer Nautilus opens
            // on space. The shell never paints its own preview sheet, so a
            // large file lands in a proper preview window (and an unsupported
            // one is reported by the system notification service).
            if paths.len() != 1 {
                self.send_notice("快速查看一次只预览一个项目", cx);
                return;
            }
            self.send(DesktopAction::Preview(paths[0].clone()), cx);
            return;
        }
        if paths.is_empty() {
            self.send_notice("请先选择桌面项目", cx);
            return;
        }
        self.inspection_generation = self.inspection_generation.wrapping_add(1);
        self.close_menu(cx);
        self.enqueue(
            Work::Inspect {
                generation: self.inspection_generation,
                paths,
            },
            cx,
        );
    }

    pub(crate) fn begin_new_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.rename.is_some() || self.new_folder || self.inspector.is_some() {
            return;
        }
        if self.preview || self.directory.is_none() {
            self.send_notice("预览模式不会创建文件夹", cx);
            return;
        }
        self.rename_name = available_folder_name(&self.entries);
        self.new_folder = true;
        self.close_menu(cx);
        self.selected = None;
        self.selection_anchor = None;
        self.selected_many.clear();
        self.input
            .update(cx, |input, cx| input.set_text(&self.rename_name, true, cx));
        window.activate_window();
        window.focus(&self.input.focus_handle(cx));
        cx.notify();
    }

    pub(crate) fn trash(&mut self, cx: &mut Context<Self>) {
        if self.rename.is_none() && !self.new_folder && self.inspector.is_none() && !self.busy {
            let paths = self.selection();
            if !paths.is_empty() {
                self.close_menu(cx);
                self.enqueue(Work::Trash(paths), cx);
            }
        }
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_menu(cx);
        self.marquee = None;
        self.rename = None;
        self.new_folder = false;
        self.inspector = None;
        self.inspection_generation = self.inspection_generation.wrapping_add(1);
        window.focus(&self.focus);
        cx.notify();
    }

    fn grid(&self, window: &Window) -> DesktopGrid {
        DesktopGrid::new(
            window.viewport_size(),
            self.reserved_edges,
            self.entries.len() + usize::from(self.new_folder),
        )
    }

    fn select_range(&mut self, index: usize) {
        let anchor = self
            .entries
            .iter()
            .position(|entry| Some(&entry.path) == self.selection_anchor.as_ref())
            .unwrap_or(index);
        self.selected_many = self.entries[anchor.min(index)..=anchor.max(index)]
            .iter()
            .map(|entry| entry.path.clone())
            .collect();
        self.selected = Some(self.entries[index].path.clone());
        if self.selection_anchor.is_none() {
            self.selection_anchor = self.selected.clone();
        }
    }

    fn step(
        &mut self,
        direction: SelectionDirection,
        extend: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
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
        let grid = self.grid(window);
        let index = current
            .map(|current| grid.neighbor(current, self.entries.len(), direction))
            .unwrap_or(0);
        if extend {
            self.select_range(index);
        } else {
            self.selected = Some(self.entries[index].path.clone());
            self.selection_anchor = self.selected.clone();
            self.selected_many = [self.entries[index].path.clone()].into();
        }
        let bounds = grid.cell(index);
        let mut offset = self.scroll.offset();
        if bounds.top() + offset.y < px(0.) {
            offset.y = -bounds.top();
        } else if bounds.bottom() + offset.y > px(grid.height) {
            offset.y = px(grid.height) - bounds.bottom();
        }
        self.scroll.set_offset(offset);
        self.close_menu(cx);
        cx.notify();
    }

    fn update_marquee(&mut self, position: Point<Pixels>, window: &Window, cx: &mut Context<Self>) {
        let grid = self.grid(window);
        let offset = self.scroll.offset();
        let Some(marquee) = self.marquee.as_mut() else {
            return;
        };
        marquee.end = grid.local_position(position, offset);
        marquee.end.x = marquee.end.x.clamp(px(0.), px(grid.width));
        marquee.end.y = marquee.end.y.clamp(px(0.), px(grid.content_height()));
        let bounds = marquee.bounds();
        self.selected_many = marquee.base.clone();
        if marquee.visible() {
            for (index, entry) in self.entries.iter().enumerate() {
                if bounds.intersects(&grid.tile(index)) {
                    self.selected_many.insert(entry.path.clone());
                }
            }
        }
        self.selected = self
            .entries
            .iter()
            .rev()
            .find(|entry| self.selected_many.contains(&entry.path))
            .map(|entry| entry.path.clone());
        self.selection_anchor = self.selected.clone();
        cx.notify();
    }

    /// The desktop window has no compositor frost region of its own; the
    /// inline editor therefore borrows the context menu's tint and rules so the
    /// two controls read as one material.
    fn edit_field(&self, focused: bool) -> AnyElement {
        let colors = MenuColors::new(self.appearance);
        let line = if focused {
            colors.text().opacity(0.30)
        } else {
            colors.line()
        };
        div()
            .debug_selector(|| "desktop-edit-field".into())
            .w_full()
            .h(px(DESKTOP_EDIT_FIELD_HEIGHT))
            .flex()
            .items_center()
            .rounded(px(DESKTOP_EDIT_FIELD_RADIUS))
            .bg(colors.surface())
            .border_1()
            .border_color(line)
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(self.input.clone())
            .into_any_element()
    }

    /// Type-matched artwork from the current icon theme, with the path's tags
    /// riding on it. `None` is the bare folder the "new folder" tile shows.
    fn file_icon(path: Option<&Path>, is_dir: bool, tags: &[String]) -> AnyElement {
        crate::file_icons::icon(path, is_dir, tags, DESKTOP_ICON_SIZE, 0.)
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
        let [top, bottom, _, _] = self.reserved_edges;
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
            .on_action(cx.listener(|this, _: &SelectUp, window, cx| {
                this.step(SelectionDirection::Up, false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectDown, window, cx| {
                this.step(SelectionDirection::Down, false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectLeft, window, cx| {
                this.step(SelectionDirection::Left, false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectRight, window, cx| {
                this.step(SelectionDirection::Right, false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ExtendUp, window, cx| {
                this.step(SelectionDirection::Up, true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ExtendDown, window, cx| {
                this.step(SelectionDirection::Down, true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ExtendLeft, window, cx| {
                this.step(SelectionDirection::Left, true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ExtendRight, window, cx| {
                this.step(SelectionDirection::Right, true, window, cx)
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    let grid = this.grid(window);
                    let additive = event.modifiers.platform
                        || event.modifiers.control
                        || event.modifiers.shift;
                    let base = if additive {
                        this.selected_many.clone()
                    } else {
                        BTreeSet::new()
                    };
                    let start = grid.local_position(event.position, this.scroll.offset());
                    this.selected = None;
                    this.selection_anchor = None;
                    this.selected_many = base.clone();
                    this.close_menu(cx);
                    this.rename = None;
                    this.new_folder = false;
                    this.marquee = grid.contains(event.position).then_some(MarqueeSelection {
                        start,
                        end: start,
                        base,
                    });
                    window.focus(&this.focus);
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                if event.pressed_button == Some(MouseButton::Left) {
                    this.update_marquee(event.position, window, cx);
                } else if this.marquee.take().is_some() {
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, window, cx| {
                    this.update_marquee(event.position, window, cx);
                    if this.marquee.take().is_some() {
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    if this.marquee.take().is_some() {
                        cx.notify();
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.selected = None;
                    this.selection_anchor = None;
                    this.selected_many.clear();
                    this.marquee = None;
                    this.rename = None;
                    this.new_folder = false;
                    window.focus(&this.focus);
                    this.open_menu(event.position, window, cx);
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
        let layout = self.grid(window);
        if self.labels.len() > MAX_ENTRIES * 2 {
            self.labels.clear();
        }
        let slot_offset = usize::from(self.new_folder);
        let mut content = div().relative().w_full().h(px(layout.content_height()));
        if let Some(marquee) = self.marquee.as_ref().filter(|marquee| marquee.visible()) {
            let bounds = marquee.bounds();
            content = content.child(
                div()
                    .absolute()
                    .left(bounds.left())
                    .top(bounds.top())
                    .w(bounds.size.width)
                    .h(bounds.size.height)
                    .border_1()
                    .border_color(rgba(0x92bfffcc))
                    .bg(rgba(0x3689e933)),
            );
        }
        if self.new_folder {
            let bounds = layout.tile(0);
            content = content.child(
                div()
                    .absolute()
                    .left(bounds.left())
                    .top(bounds.top())
                    .w(bounds.size.width)
                    .h(bounds.size.height)
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(3.))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(Self::file_icon(None, true, &[]))
                    .child(self.edit_field(self.input.read(cx).focused(window))),
            );
        }
        for (index, entry) in self.entries.iter().enumerate() {
            let path = entry.path.clone();
            let context_path = path.clone();
            let selected = self.selected_many.contains(&path);
            let editing = self.rename.as_ref() == Some(&path);
            let bounds = layout.tile(index + slot_offset);
            let label_width = (f32::from(bounds.size.width).min(DESKTOP_LABEL_WIDTH) - 6.).max(1.);
            let label = self
                .labels
                .entry((entry.name.clone(), label_width.to_bits()))
                .or_insert_with(|| {
                    desktop_label(&entry.name, label_width, |text| {
                        label_width_fn(text, window)
                    })
                })
                .clone();
            let mut icon = div()
                .id(("desktop-file", index))
                .absolute()
                .left(bounds.left())
                .top(bounds.top())
                .w(bounds.size.width)
                .h(bounds.size.height)
                .flex()
                .flex_col()
                .items_center()
                .gap(px(3.))
                .child(
                    div()
                        .size(px(DESKTOP_ICON_SIZE))
                        .rounded(px(7.))
                        .bg(if selected {
                            rgba(0x6387ad66)
                        } else {
                            rgba(0xffffff00)
                        })
                        .child(Self::file_icon(
                            Some(&entry.path),
                            entry.kind == EntryKind::Directory,
                            &entry.tags,
                        )),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        window.focus(&this.focus);
                        this.close_menu(cx);
                        this.marquee = None;
                        if this.rename.as_ref() != Some(&path) {
                            this.rename = None;
                        }
                        this.new_folder = false;
                        if event.modifiers.platform || event.modifiers.control {
                            if !this.selected_many.remove(&path) {
                                this.selected_many.insert(path.clone());
                                this.selected = Some(path.clone());
                            } else if this.selected.as_ref() == Some(&path) {
                                this.selected = this
                                    .entries
                                    .iter()
                                    .rev()
                                    .find(|entry| this.selected_many.contains(&entry.path))
                                    .map(|entry| entry.path.clone());
                            }
                            this.selection_anchor = this.selected.clone();
                        } else if event.modifiers.shift {
                            this.select_range(index);
                        } else {
                            this.selected = Some(path.clone());
                            this.selection_anchor = Some(path.clone());
                            this.selected_many = [path.clone()].into();
                        }
                        if event.click_count >= 2
                            && this.rename.is_none()
                            && !event.modifiers.shift
                            && !event.modifiers.platform
                            && !event.modifiers.control
                        {
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
                        this.marquee = None;
                        this.selected = Some(context_path.clone());
                        this.selection_anchor = Some(context_path.clone());
                        if !this.selected_many.contains(&context_path) {
                            this.selected_many = [context_path.clone()].into();
                        }
                        this.open_menu(event.position, window, cx);
                    }),
                );
            if editing {
                icon = icon.child(self.edit_field(self.input.read(cx).focused(window)));
            } else {
                let mut caption = div()
                    .max_w_full()
                    .px(px(3.))
                    .rounded(px(3.))
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_center()
                    .line_height(px(16.))
                    .when(selected, |label| label.bg(rgb(0x176fd2)));
                for line in label {
                    caption = caption.child(
                        div()
                            .relative()
                            .h(px(16.))
                            .whitespace_nowrap()
                            .child(
                                div()
                                    .absolute()
                                    .top(px(1.))
                                    .left(px(0.))
                                    .right(px(0.))
                                    .text_color(rgba(0x000000bb))
                                    .child(line.clone()),
                            )
                            .child(div().relative().text_color(rgb(0xffffff)).child(line)),
                    );
                }
                icon = icon.child(caption);
            }
            content = content.child(icon);
        }
        root = root.child(
            div()
                .id("desktop-grid")
                .absolute()
                .left(layout.origin.x)
                .top(layout.origin.y)
                .w(px(layout.width))
                .h(px(layout.height))
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .child(content),
        );
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
        let colors = MenuColors::new(self.appearance);
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
                            .bg(colors.surface())
                            .text_color(colors.text())
                            .border_1()
                            .border_color(colors.line())
                            .shadow(vec![BoxShadow {
                                color: colors.shadow(),
                                offset: point(px(0.), px(8.)),
                                blur_radius: px(24.),
                                spread_radius: px(0.),
                            }])
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
                                    .border_color(colors.line())
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
    _directory: &Path,
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
        anyhow::ensure!(
            path.is_absolute() && !path.as_os_str().as_bytes().contains(&0),
            "无效的项目路径"
        );
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

fn inspect_paths(directory: &Path, paths: &[PathBuf]) -> Result<Inspector> {
    anyhow::ensure!(
        !paths.is_empty() && paths.len() <= MAX_ENTRIES,
        "请选择桌面项目"
    );
    for path in paths {
        anyhow::ensure!(
            path.is_absolute() && !path.as_os_str().as_bytes().contains(&0),
            "无效的项目路径"
        );
    }
    let mut inspector = Inspector {
        title: "显示简介".into(),
        rows: Vec::new(),
        path: (paths.len() == 1).then(|| paths[0].clone()),
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
        inspector.title = format!("{} 个项目", paths.len());
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
    inspector.title = name;
    inspector.rows = vec![
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
            tags: crate::file_icons::tags(&path),
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
                    tags: Vec::new(),
                    modified: None,
                },
                DesktopEntry {
                    path: PathBuf::from("/clarion-preview/Welcome.txt"),
                    name: "欢迎使用 Clarion.txt".into(),
                    kind: EntryKind::File,
                    tags: Vec::new(),
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
                        } => {
                            let result = inspect_paths(&directory, &paths)
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
    use ::core::prelude::v1::test;

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

    #[test]
    fn desktop_grid_fills_down_from_right_and_respects_each_outputs_logical_edges() {
        // A 3840x2400 output at scale 2 is presented to GPUI as 1920x1200.
        // No second multiplication by scale belongs in desktop geometry.
        for (viewport, edges) in [
            (size(px(1920.), px(1200.)), [28., 78., 0., 0.]),
            (size(px(1280.), px(720.)), [28., 0., 84., 0.]),
            (size(px(900.), px(600.)), [28., 0., 0., 84.]),
        ] {
            let grid = DesktopGrid::new(viewport, edges, 40);
            let first = grid.cell(0);
            assert_eq!(first.top(), px(0.));
            assert_eq!(first.right(), px(grid.width));
            assert_eq!(grid.cell(1).left(), first.left());
            assert_eq!(grid.cell(1).top(), px(DESKTOP_CELL_HEIGHT));
            assert!(grid.cell(grid.rows).left() < first.left());
            assert_eq!(grid.cell(grid.rows).top(), px(0.));
            assert_eq!(grid.origin, point(px(edges[2] + 16.), px(edges[0] + 16.)));
            assert_eq!(
                grid.origin.x + px(grid.width),
                viewport.width - px(edges[3] + 16.)
            );
            assert_eq!(
                grid.origin.y + px(grid.height),
                viewport.height - px(edges[1] + 16.)
            );
        }
    }

    #[test]
    fn arrows_follow_geometric_neighbors_without_wrapping_between_columns() {
        let grid = DesktopGrid::new(size(px(700.), px(480.)), [0.; 4], 14);
        assert_eq!(grid.rows, 4);
        assert_eq!(grid.neighbor(0, 14, SelectionDirection::Down), 1);
        assert_eq!(grid.neighbor(3, 14, SelectionDirection::Down), 3);
        assert_eq!(grid.neighbor(0, 14, SelectionDirection::Up), 0);
        assert_eq!(grid.neighbor(1, 14, SelectionDirection::Left), 5);
        assert_eq!(grid.neighbor(5, 14, SelectionDirection::Right), 1);
        assert_eq!(grid.neighbor(11, 14, SelectionDirection::Left), 13);
        assert_eq!(grid.neighbor(13, 14, SelectionDirection::Left), 13);
        assert_eq!(grid.neighbor(13, 14, SelectionDirection::Right), 9);
    }

    #[test]
    fn full_desktop_keeps_all_entries_reachable_and_marquee_tracks_scroll_and_output_origin() {
        let grid = DesktopGrid::new(size(px(480.), px(360.)), [28., 78., 0., 0.], MAX_ENTRIES);
        for index in 0..MAX_ENTRIES {
            let bounds = grid.tile(index);
            assert!(bounds.left() >= px(0.));
            assert!(bounds.right() <= px(grid.width));
            assert!(bounds.bottom() <= px(grid.content_height()));
        }
        let bounds = grid.tile(130);
        let offset = point(px(0.), -bounds.top());
        let screen_point = grid.origin + bounds.origin + offset + point(px(4.), px(4.));
        assert!(grid.contains(screen_point));
        let local = grid.local_position(screen_point, offset);
        assert!(bounds.contains(&local));
        let marquee = MarqueeSelection {
            start: bounds.origin,
            end: bounds.bottom_right(),
            base: BTreeSet::new(),
        };
        assert!(marquee.visible());
        assert!(marquee.bounds().intersects(&bounds));
        assert!(!marquee.bounds().intersects(&grid.tile(129)));
    }

    #[test]
    fn two_line_names_keep_unicode_graphemes_and_distinguishing_filename_suffixes() {
        let measure = |text: &str| text.graphemes(true).count() as f32;
        assert_eq!(desktop_label("Home", 12., measure), ["Home"]);
        assert_eq!(
            desktop_label("small-file.txt", 8., measure),
            ["small-fi", "le.txt"]
        );
        let name = "春日漫剧-源代码-2026-10-18-作品18.mp4";
        let first = desktop_label(name, 12., measure);
        let second = desktop_label(&name.replace("作品18", "作品19"), 12., measure);
        assert_eq!(first.len(), 2);
        assert!(first.iter().all(|line| measure(line) <= 12.));
        assert!(first[1].contains('…'));
        assert!(first[1].ends_with("作品18.mp4"));
        assert_ne!(first, second);
        let complex = "👩🏽‍💻".repeat(40) + ".txt";
        let lines = desktop_label(&complex, 12., measure);
        assert!(lines.iter().all(|line| measure(line) <= 12.));
        assert!(lines[1].ends_with(".txt"));
        assert_eq!(
            desktop_label("file\nname\t.txt", 40., measure),
            ["file name .txt"]
        );
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
        let info = inspect_paths(&directory, std::slice::from_ref(&link)).unwrap();
        assert!(info
            .rows
            .iter()
            .any(|(label, value)| label == "种类" && value == "符号链接"));
        assert!(info.rows.iter().any(|(label, _)| label == "链接目标"));
        let info = inspect_paths(&directory, &[file.clone(), folder, link.clone()]).unwrap();
        assert!(info
            .rows
            .iter()
            .any(|(_, value)| value == "1 个文件、1 个文件夹、1 个符号链接"));
        assert!(info
            .rows
            .iter()
            .any(|(label, value)| label == "文件大小" && value.starts_with("16 字节")));
        assert!(inspect_paths(&directory, &[root.path().join("outside")]).is_err());
        assert_eq!(fs::read_to_string(file).unwrap(), "你好，Clarion");
    }

    fn desktop_fixture<'a>(
        cx: &'a mut TestAppContext,
        directory: &Path,
    ) -> (
        Entity<DesktopView>,
        &'a mut VisualTestContext,
        async_channel::Receiver<DesktopAction>,
        mpsc::Receiver<Work>,
    ) {
        let entries = scan(directory).unwrap();
        let (actions, received_actions) = async_channel::unbounded();
        let (work, received_work) = mpsc::sync_channel(16);
        cx.update(|cx| {
            TextInput::bind_keys(cx);
            DesktopView::bind_keys(cx);
        });
        let (view, visual) = cx.add_window_view(|window, cx| {
            let input = cx.new(|cx| {
                TextInput::new(cx)
                    .with_placeholder("文件名称")
                    .with_capsule(true)
            });
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
                directory: Some(directory.to_path_buf()),
                wallpaper: None,
                selected: None,
                selected_many: BTreeSet::new(),
                selection_anchor: None,
                marquee: None,
                labels: HashMap::new(),
                new_folder: false,
                inspector: None,
                inspection_generation: 0,
                menu: None,
                menu_target: None,
                menu_window: None,
                own_window: None,
                rename: None,
                rename_name: String::new(),
                input,
                focus,
                last_notice: None,
                busy: false,
                work,
                alive: Arc::new(AtomicBool::new(true)),
                scroll: ScrollHandle::new(),
                reserved_edges: [0.; 4],
                appearance: Default::default(),
            }
        });
        (view, visual, received_actions, received_work)
    }

    #[gpui::test]
    fn rename_and_context_menu_use_the_compact_shell_style(cx: &mut TestAppContext) {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("Desktop");
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("first.txt"), b"keep").unwrap();
        let (view, visual, _actions, _work) = desktop_fixture(cx, &directory);
        let tile = visual.update(|window, cx| {
            let grid = view.read(cx).grid(window);
            Bounds::new(grid.origin + grid.tile(0).origin, grid.tile(0).size)
        });

        visual.simulate_click(tile.center(), Modifiers::default());
        visual.simulate_keystrokes("f2");
        visual.run_until_parked();
        let rename = visual
            .debug_bounds("desktop-edit-field")
            .expect("the rename field is painted");
        assert!(
            f32::from(rename.size.height) <= 30.,
            "the rename field must stay compact inside a desktop tile: {:?}",
            rename
        );

        visual.simulate_keystrokes("escape");
        visual.simulate_keystrokes("super-shift-n");
        visual.run_until_parked();
        let new_folder = visual
            .debug_bounds("desktop-edit-field")
            .expect("the new-folder field is painted");
        assert_eq!(
            rename.size, new_folder.size,
            "rename and new folder share one field style"
        );
        visual.simulate_keystrokes("escape");

        let windows_before = visual.cx.windows();
        visual.simulate_mouse_down(tile.center(), MouseButton::Right, Modifiers::default());
        visual.run_until_parked();
        // A card painted into the desktop surface can never be frosted: the
        // compositor blurs what lies behind a surface, never its own icons.
        // The menu therefore has to be a surface of its own, like the
        // launcher's application menu.
        assert_eq!(
            visual.cx.windows().len(),
            windows_before.len() + 1,
            "the context menu must open its own overlay surface"
        );
        let handle = view
            .read_with(visual, |view, _| view.menu_window)
            .expect("the desktop records its menu surface");
        let menu_entity = handle
            .root(&mut visual.cx)
            .expect("the menu surface has a root view");
        let mut menu = VisualTestContext::from_window(*handle, &visual.cx);
        menu.run_until_parked();
        let card = menu
            .debug_bounds("desktop-context-menu")
            .expect("the context menu is painted in its own surface");
        assert!(
            f32::from(card.size.width) <= 220.,
            "the context menu stays compact: {card:?}"
        );
        let row = menu
            .debug_bounds("desktop-open")
            .expect("the open row is painted");
        assert!(
            f32::from(row.size.height) <= 28.,
            "menu rows stay compact: {row:?}"
        );
        for id in [
            "desktop-menu-separator-0",
            "desktop-menu-separator-1",
            "desktop-menu-separator-2",
            "desktop-menu-separator-3",
        ] {
            assert!(
                menu.debug_bounds(id).is_some(),
                "the context menu must group its actions with {id}"
            );
        }
        for id in [
            "desktop-menu-icon-desktop-open",
            "desktop-menu-icon-desktop-open-with",
            "desktop-menu-icon-desktop-info",
            "desktop-menu-icon-desktop-quick-look",
            "desktop-menu-icon-desktop-rename",
            "desktop-menu-icon-desktop-trash",
            "desktop-menu-icon-desktop-new-folder",
            "desktop-menu-icon-desktop-refresh",
            "desktop-menu-icon-desktop-open-directory",
            "desktop-menu-icon-desktop-wallpaper",
            "desktop-menu-icon-desktop-settings",
        ] {
            assert!(
                menu.debug_bounds(id).is_some(),
                "the context menu row must carry the {id} prefix"
            );
        }
        assert_eq!(
            menu_entity.read_with(&menu, |menu, _| menu.glass),
            Some(card),
            "the frosted mask must be the exact painted card outline"
        );
        view.update(visual, |view, cx| {
            view.set_appearance(
                SurfaceAppearance {
                    frost: 0.,
                    ..view.appearance
                },
                cx,
            );
        });
        menu.run_until_parked();
        assert!(
            menu_entity.read_with(&menu, |menu, _| menu.glass.is_none()),
            "zero frost must drop the glass region instead of blurring the desktop"
        );

        // The inline editor lives in the desktop window, so a menu action has
        // to hand focus back there and take its own surface down.
        let new_folder = menu
            .debug_bounds("desktop-new-folder")
            .expect("the new-folder row is painted");
        menu.simulate_click(new_folder.center(), Modifiers::default());
        visual.run_until_parked();
        assert!(
            view.read_with(visual, |view, _| view.new_folder),
            "the menu action starts the inline new-folder field"
        );
        assert!(
            view.read_with(visual, |view, _| view.menu_window.is_none()),
            "the menu surface closes once its action has run"
        );
        assert!(
            visual.debug_bounds("desktop-edit-field").is_some(),
            "the field is painted back in the desktop window"
        );
        let input_focus = view.read_with(visual, |view, cx| view.input.focus_handle(cx));
        assert_eq!(
            visual.update(|window, cx| window.focused(cx)),
            Some(input_focus),
            "the inline field must own keyboard focus after the menu closes"
        );
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();

        visual.simulate_mouse_down(tile.center(), MouseButton::Right, Modifiers::default());
        visual.run_until_parked();
        let escape = view
            .read_with(visual, |view, _| view.menu_window)
            .expect("a fresh menu surface");
        let mut escape = VisualTestContext::from_window(*escape, &visual.cx);
        escape.simulate_keystrokes("escape");
        escape.run_until_parked();
        visual.run_until_parked();
        assert_eq!(
            visual.cx.windows().len(),
            windows_before.len(),
            "escape must dismiss the menu surface"
        );
    }

    #[gpui::test]
    fn desktop_hints_go_through_the_notification_service(cx: &mut TestAppContext) {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("Desktop");
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("first.txt"), b"keep").unwrap();
        let (_view, visual, actions, _work) = desktop_fixture(cx, &directory);

        // Nothing is selected, so rename is refused. The explanation belongs to
        // the session's notification service, not a banner over the files.
        visual.simulate_keystrokes("f2");
        visual.run_until_parked();
        assert!(
            matches!(
                actions.try_recv(),
                Ok(DesktopAction::Notify { body }) if body == "请选择一个项目进行重命名"
            ),
            "a refused rename must be reported as a system notification"
        );

        // Hitting the same refusal again is not worth a second toast.
        visual.simulate_keystrokes("f2");
        visual.run_until_parked();
        assert!(
            actions.try_recv().is_err(),
            "an identical hint must not be posted twice in a row"
        );

        // A different hint still gets through: dedupe is only about immediate
        // repeats, not about silencing the desktop for the rest of the session.
        visual.simulate_keystrokes("super-i");
        visual.run_until_parked();
        assert!(
            matches!(
                actions.try_recv(),
                Ok(DesktopAction::Notify { body }) if body == "请先选择桌面项目"
            ),
            "a new hint is posted even right after another one"
        );
        visual.simulate_keystrokes("f2");
        visual.run_until_parked();
        assert!(
            matches!(
                actions.try_recv(),
                Ok(DesktopAction::Notify { body }) if body == "请选择一个项目进行重命名"
            ),
            "the earlier hint may be repeated once something else was posted"
        );
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
                selection_anchor: None,
                marquee: None,
                labels: HashMap::new(),
                new_folder: false,
                inspector: None,
                inspection_generation: 0,
                menu: None,
                menu_target: None,
                menu_window: None,
                own_window: None,
                rename: None,
                rename_name: String::new(),
                input,
                focus,
                last_notice: None,
                busy: false,
                work,
                alive: Arc::new(AtomicBool::new(true)),
                scroll: ScrollHandle::new(),
                reserved_edges: [0.; 4],
                appearance: Default::default(),
            }
        });
        let (first_bounds, second_bounds) = visual.update(|window, cx| {
            let grid = view.read(cx).grid(window);
            (
                Bounds::new(grid.origin + grid.tile(0).origin, grid.tile(0).size),
                Bounds::new(grid.origin + grid.tile(1).origin, grid.tile(1).size),
            )
        });
        visual.simulate_click(first_bounds.center(), Modifiers::default());
        assert_eq!(
            view.read_with(visual, |view, _| view.selected.clone()),
            Some(directory.join("first.txt"))
        );
        visual.simulate_keystrokes("down");
        assert_eq!(
            view.read_with(visual, |view, _| view.selected.clone()),
            Some(directory.join("second.txt"))
        );
        visual.simulate_keystrokes("up shift-down");
        assert_eq!(
            view.read_with(visual, |view, _| view.selected_many.len()),
            2
        );
        visual.simulate_keystrokes("shift-up");
        assert_eq!(
            view.read_with(visual, |view, _| view.selected_many.len()),
            1
        );
        visual.simulate_mouse_down(
            second_bounds.center(),
            MouseButton::Right,
            Modifiers::default(),
        );
        assert_eq!(
            view.read_with(visual, |view, _| view.selected.clone()),
            Some(directory.join("second.txt"))
        );
        assert!(view.read_with(visual, |view, _| view.menu.is_some()));
        visual.simulate_keystrokes("escape");
        let start = first_bounds.origin - point(px(2.), px(2.));
        let end = second_bounds.bottom_right() + point(px(1.), px(1.));
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        assert_eq!(
            view.read_with(visual, |view, _| view.selected_many.len()),
            2
        );
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        assert!(view.read_with(visual, |view, _| view.marquee.is_none()));
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
            matches!(received_work.try_recv(), Ok(Work::Inspect { paths, .. }) if paths.len() == 2)
        );
        view.update(visual, |view, _| view.busy = false);
        visual.simulate_keystrokes("super-y");
        assert!(
            matches!(
                received_actions.try_recv(),
                Ok(DesktopAction::Notify { body }) if body == "快速查看一次只预览一个项目"
            ),
            "Quick Look previews one file at a time and says so through a notification"
        );
        assert!(
            received_actions.try_recv().is_err(),
            "Quick Look must not silently launch an app for a multi-selection"
        );
        visual.simulate_click(first_bounds.center(), Modifiers::default());
        visual.simulate_keystrokes("super-y");
        assert!(
            matches!(received_actions.try_recv(), Ok(DesktopAction::Preview(path)) if path == directory.join("first.txt")),
            "Quick Look hands the file to the system previewer"
        );
        assert!(
            received_work.try_recv().is_err(),
            "Quick Look never reads the file in the desktop worker"
        );
        visual.simulate_keystrokes("super-a");
        assert_eq!(
            view.read_with(visual, |view, _| view.selected_many.len()),
            2
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
        // Paths outside the desktop directory are allowed now that the same
        // menu serves Dock folder entries; only a malformed path is refused.
        assert!(trash_entries(
            &directory,
            &[path.clone(), PathBuf::from("relative")],
            &environment
        )
        .is_err());
        assert_eq!(
            fs::read(&capture).unwrap(),
            before,
            "the complete batch must validate before GIO starts"
        );
        assert_eq!(fs::read(second).unwrap(), b"second");
        assert!(trash_entry(&directory, &PathBuf::from("relative"), &environment).is_err());
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
