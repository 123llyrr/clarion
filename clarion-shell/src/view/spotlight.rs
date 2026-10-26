// SPDX-License-Identifier: GPL-3.0-or-later
//! The compact launcher palette.
//!
//! The interaction model is the reference implementation's Spotlight
//! (`Modules/Launcher/LauncherWindow.qml`): one search field, scored results,
//! modes, slashed commands and a parent stack that `Backspace` unwinds. The
//! look stays Clarion's: the same translucent surface, accents and row rhythm
//! the menus use.
//!
//! Scope of the current slice: search, apps (grid/list, ordering), the command
//! list and the clipboard. Wallpapers, files and the tool pages arrive with
//! their backends; until then their commands are filtered out instead of
//! offering dead entries.
use super::*;
use clarion_launcher::catalog::{AppCategory, DesktopApp};
use clarion_launcher::commands::{self, Command, CommandKind, ResolveError};
use clarion_launcher::search::{ranked_apps, AppOrder};
use clarion_launcher::session::{Mode, Route, SavedStyles};
use clarion_launcher::usage::unix_millis;

/// One-list-row height and the gutter under it. Application and command rows
/// share the rhythm, including the pixel-based keyboard scroll maths.
const LIST_ROW_H: f32 = 54.;
const LIST_GAP: f32 = 4.;
const LIST_PITCH: f32 = LIST_ROW_H + LIST_GAP;
/// Five tiles per grid row. Every row renders five equal slots across the
/// sheet's full inner width — full and partial rows alike stay symmetric —
/// and each tile sits centred in its slot. `GRID_GAP` is the vertical gutter.
const GRID_COLUMNS: usize = 5;
const GRID_TILE_W: f32 = 128.;
const GRID_TILE_H: f32 = 128.;
/// The plate behind a grid icon: 10px of air around the 72px icon, so the
/// selection frame reads as a card rather than a tight outline.
const GRID_PLATE: f32 = 92.;
/// The clipboard details layout: a list column beside the preview. Both are
/// fixed so the preview never rides the list's scroll and the keyboard's pixel
/// stepping has a known viewport.
const CLIPBOARD_LIST_W: f32 = 292.;
const CLIPBOARD_LIST_H: f32 = 486.;

/// The content-type tags above the clipboard body. `Image` covers screenshots
/// too, the way the Recent panel groups them; `Text` is only text, so a file
/// list (which also answers `text/plain`) does not drown the text tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ClipboardFilter {
    Text,
    Image,
    Files,
}

impl ClipboardFilter {
    /// Tag order on screen, and the order `Alt+T`, `Alt+I`, `Alt+F` name.
    const ALL: [ClipboardFilter; 3] = [Self::Text, Self::Image, Self::Files];

    fn title(self) -> &'static str {
        match self {
            Self::Text => "文本",
            Self::Image => "图片",
            Self::Files => "文件",
        }
    }

    /// The debug selector's suffix and the shortcut's letter, lower case.
    fn key(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Image => "image",
            Self::Files => "files",
        }
    }

    fn accepts(self, kind: services::HistoryKind) -> bool {
        match self {
            Self::Text => matches!(kind, services::HistoryKind::Text),
            Self::Image => matches!(
                kind,
                services::HistoryKind::Image | services::HistoryKind::Screenshot
            ),
            Self::Files => matches!(kind, services::HistoryKind::Files),
        }
    }
}
const GRID_GAP: f32 = 5.;
/// Row pitch, including the gutter under each row.
const GRID_PITCH: f32 = GRID_TILE_H + GRID_GAP;
/// A categorized section's header block and its divider, both fixed so the
/// keyboard scroll maths can walk the plan in pixels.
const SECTION_HEADER_H: f32 = 16.;
const SECTION_DIVIDER_H: f32 = 12.;

/// One categorized slice of the resting grid: its header title, the category
/// a "更多" button narrows the tag bar to (recent launches carry none), and
/// the applications in ranked order.
struct PaletteSection<'a> {
    title: &'static str,
    category: Option<AppCategory>,
    /// The applications this section paints. The resting grid shows one row,
    /// so this is capped at `GRID_COLUMNS`.
    apps: Vec<&'a DesktopApp>,
    /// The full category count, used by the header and its "更多" action.
    total: usize,
}

/// What the palette is showing for the current session and draft.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PaletteBody {
    Apps,
    Clipboard,
    Commands,
    Slash,
}

/// Whether a command has a working implementation behind it. The table keeps
/// the whole reference set so the parser stays honest; the palette only offers
/// entries the shell can actually run.
fn implemented(command: &Command) -> bool {
    match command.kind {
        CommandKind::Mode => {
            matches!(command.value, Some("search" | "apps" | "commands" | "clipboard"))
        }
        CommandKind::Tool => false,
        CommandKind::Action => action_ready(command.id),
        CommandKind::Override => matches!(
            command.key,
            Some("appsLayout" | "appsOrder" | "clipboardLayout")
        ),
    }
}

/// Every action entry the shell can actually dispatch. The table keeps the
/// reference set; this is the executable subset, including the session power
/// actions and the settings toggles the palette offers.
fn action_ready(id: &str) -> bool {
    matches!(
        id,
        "theme.light"
            | "theme.dark"
            | "settings.open"
            | "settings.launcher.fullscreen"
            | "settings.launcher.palette"
            | "settings.wallpaper"
            | "settings.reduce-motion"
            | "settings.dock-autohide"
            | "shell.screenshot"
            | "shell.notifications"
            | "shell.recent"
            | "shell.control"
            | "system.lock"
            | "system.logout"
            | "system.suspend"
            | "system.reboot"
            | "system.shutdown"
    )
}

fn kind_label(kind: CommandKind) -> &'static str {
    match kind {
        CommandKind::Mode => "模式",
        CommandKind::Tool => "工具",
        CommandKind::Action => "动作",
        CommandKind::Override => "显示",
    }
}

/// The one-line label for a clipboard entry: its first non-empty text line, or
/// the payload kind for the image-only kinds.
fn clipboard_title(entry: &services::HistoryEntry) -> String {
    match entry.kind {
        services::HistoryKind::Text => {
            let line = entry
                .text
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .unwrap_or_default();
            if line.is_empty() {
                "空白文本".to_owned()
            } else {
                line.chars().take(160).collect()
            }
        }
        services::HistoryKind::Image => "图片".to_owned(),
        services::HistoryKind::Screenshot => "截图".to_owned(),
        services::HistoryKind::Files => {
            let mut lines = entry.text.lines().filter(|line| !line.is_empty());
            let first = lines.next().unwrap_or_default();
            let count = 1 + lines.count();
            let name = Path::new(first)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| first.to_owned());
            if count > 1 {
                format!("{name} 等 {count} 项")
            } else {
                name
            }
        }
    }
}

/// The row subtitle: pin state, kind, a size hint and the age.
fn clipboard_meta(entry: &services::HistoryEntry) -> String {
    let kind = match entry.kind {
        services::HistoryKind::Text => "文字",
        services::HistoryKind::Image => "图片",
        services::HistoryKind::Screenshot => "截图",
        services::HistoryKind::Files => "文件",
    };
    let detail = match entry.kind {
        services::HistoryKind::Text => format!("{} 字", entry.text.chars().count()),
        services::HistoryKind::Files => {
            format!(
                "{} 个文件",
                entry.text.lines().filter(|line| !line.is_empty()).count()
            )
        }
        _ => entry
            .width
            .zip(entry.height)
            .map(|(width, height)| format!("{width}×{height}"))
            .unwrap_or_default(),
    };
    let pin = if entry.pinned { "已置顶 · " } else { "" };
    let detail = if detail.is_empty() {
        String::new()
    } else {
        format!(" · {detail}")
    };
    format!(
        "{pin}{kind}{detail} · {} · {}",
        clipboard_size(entry.bytes),
        clipboard_age(entry.created_at)
    )
}

/// A payload size for the row subtitle and the preview stats.
fn clipboard_size(bytes: usize) -> String {
    match bytes {
        0..=1023 => format!("{bytes} B"),
        1024..=1_048_575 => format!("{:.1} KB", bytes as f64 / 1024.),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.),
    }
}

/// How old a capture is, in the coarsest unit that still reads naturally.
fn clipboard_age(created_at: u64) -> String {
    let now = unix_millis().max(created_at);
    let seconds = (now - created_at) / 1000;
    match seconds {
        0..=59 => "刚刚".to_owned(),
        60..=3599 => format!("{} 分钟前", seconds / 60),
        3600..=86_399 => format!("{} 小时前", seconds / 3600),
        _ => format!("{} 天前", seconds / 86_400),
    }
}

/// Theme icons named for individual commands, so a row can use the installed
/// icon set directly instead of one of the bundled glyphs. A name the theme
/// does not carry falls through to [`command_icon`]'s bundled asset.
pub(crate) fn command_symbol(id: &str) -> Option<&'static str> {
    Some(match id {
        "mode.search" => "file-search-symbolic",
        "mode.apps" => "software-store-symbolic",
        "mode.commands" => "layers-button-symbolic",
        "system.lock" => "lock-symbolic",
        "system.logout" => "exit-symbolic",
        "system.suspend" => "system-suspend-symbolic",
        "system.reboot" => "system-reboot-symbolic",
        "system.shutdown" => "system-shutdown-symbolic",
        "shell.screenshot" => "screenshooter-symbolic",
        "shell.notifications" => "notification-symbolic",
        "shell.control" => "tweaks-app-symbolic",
        "settings.wallpaper" => "image-round-symbolic",
        "settings.reduce-motion" => "blur-symbolic",
        "settings.launcher.fullscreen" => "move-symbolic",
        "settings.launcher.palette" => "window-new-symbolic",
        "mode.clipboard" | "shell.recent" => "category-search-symbolic",
        _ => return None,
    })
}

/// The leading glyph for a command. Entries with a close visual match get
/// their own icon; the rest fall back to the command's kind so every row has
/// the same visual anchor as an application row.
fn command_icon(command: &Command) -> &'static str {
    match command.id {
        "mode.search" => "launchpad.svg",
        "mode.apps" => "grid.svg",
        "mode.wallpapers" => "wallpaper.svg",
        "mode.clipboard" => "clipboard.svg",
        "mode.files" => "generic-folder.svg",
        "mode.commands" => "terminal.svg",
        "theme.light" => "sun.svg",
        "theme.dark" => "moon.svg",
        "settings.open" | "tool.settings" => "settings.svg",
        "settings.launcher.fullscreen" | "settings.launcher.palette" => "launchpad.svg",
        "settings.wallpaper" => "wallpaper.svg",
        "settings.reduce-motion" => "sliders.svg",
        "settings.dock-autohide" => "display.svg",
        "shell.screenshot" => "capture-screen.svg",
        "shell.notifications" => "bell.svg",
        "shell.recent" => "clipboard.svg",
        "shell.control" => "control-center.svg",
        "tool.web" => "safari.svg",
        "tool.calculator" | "tool.currency" => "calculator.svg",
        "tool.time" => "calendar.svg",
        "tool.actions" => "scan.svg",
        "apps.grid" => "grid.svg",
        "apps.list" | "apps.smart" | "apps.most-used" | "apps.recent" | "apps.name" => {
            "sliders.svg"
        }
        _ => match command.kind {
            CommandKind::Mode => "launchpad.svg",
            CommandKind::Tool => "scan.svg",
            CommandKind::Action => "settings.svg",
            CommandKind::Override => "sliders.svg",
        },
    }
}

/// Applications ranked for one palette query.
///
/// The fullscreen Launchpad still uses the catalog's substring search; the
/// palette uses the score ladder so an exact name beats a keyword hit, with
/// usage history only breaking textual ties.
pub(super) fn palette_apps<'a>(
    host: &'a ViewState,
    query: &str,
    order: AppOrder,
) -> Vec<&'a DesktopApp> {
    ranked_apps(
        host.catalog.apps(),
        query,
        order,
        host.launch_history.records(),
        unix_millis(),
    )
}

impl ShellView {
    fn palette_body(&self) -> PaletteBody {
        // A slash draft always opens the completion, whatever mode it was
        // typed in, so `/detail` and friends stay reachable from the clipboard.
        match self.palette.route(&self.query) {
            Route::Slash { .. } => return PaletteBody::Slash,
            Route::Commands(_) => return PaletteBody::Commands,
            _ => {}
        }
        match self.palette.mode {
            Mode::Commands => PaletteBody::Commands,
            Mode::Clipboard => PaletteBody::Clipboard,
            _ => PaletteBody::Apps,
        }
    }

    /// The header mark names the mode the sheet is *in*, not the row under the
    /// keyboard: only the modes the user keeps searching in (applications,
    /// commands, clipboard, and the file search to come) replace the mark.
    /// While the draft merely composes a command, the generic command mark
    /// stands in; the selection never moves the header.
    pub(super) fn palette_glyph_path(&self) -> PathBuf {
        match self.palette.mode {
            Mode::Commands => self.themed_glyph_path("layers-button-symbolic", "terminal.svg"),
            Mode::Clipboard => {
                self.themed_glyph_path("category-search-symbolic", "clipboard.svg")
            }
            Mode::Files => self.themed_glyph_path("folder-symbolic", "generic-folder.svg"),
            Mode::Wallpapers => {
                self.themed_glyph_path("image-round-symbolic", "wallpaper.svg")
            }
            Mode::Search | Mode::Apps => {
                if matches!(
                    self.palette.route(&self.query),
                    Route::Slash { .. } | Route::Commands(_)
                ) {
                    self.themed_glyph_path(SPOTLIGHT_COMMAND_GLYPH, "terminal.svg")
                } else {
                    self.themed_glyph_path("software-store-symbolic", "launchpad.svg")
                }
            }
        }
    }

    fn palette_saved_styles(&self, host: &ViewState) -> SavedStyles {
        SavedStyles {
            apps_layout: match host.prefs.spotlight_app_layout {
                LauncherLayout::Grid => "grid",
                LauncherLayout::List => "list",
            },
            apps_order: host.prefs.spotlight_app_order.value(),
            clipboard_layout: "default",
        }
    }

    pub(super) fn palette_layout(&self, host: &ViewState) -> LauncherLayout {
        match self
            .palette
            .effective("appsLayout", self.palette_saved_styles(host).apps_layout)
        {
            "list" => LauncherLayout::List,
            _ => LauncherLayout::Grid,
        }
    }

    fn palette_order(&self, host: &ViewState) -> AppOrder {
        AppOrder::from_value(
            self.palette
                .effective("appsOrder", self.palette_saved_styles(host).apps_order),
        )
    }

    /// The palette opens as a grid (Launchpad feel) and filters in place while
    /// typing in apps mode. In the resting search mode a typed query switches
    /// to the denser result list.
    fn palette_grid(&self, host: &ViewState) -> bool {
        if self.palette_body() != PaletteBody::Apps {
            return false;
        }
        match self.palette.mode {
            Mode::Apps => self.palette_layout(host) == LauncherLayout::Grid,
            _ => self.query.is_empty() && self.palette_layout(host) == LauncherLayout::Grid,
        }
    }

    fn palette_apps<'a>(&self, host: &'a ViewState) -> Vec<&'a DesktopApp> {
        palette_apps(host, &self.query, self.palette_order(host))
    }

    /// The applications the palette currently offers: the ranked list, minus
    /// the category tag's filter when one is selected.
    fn palette_visible_apps<'a>(&self, host: &'a ViewState) -> Vec<&'a DesktopApp> {
        self.palette_apps(host)
            .into_iter()
            .filter(|app| self.palette_category.is_none_or(|c| app.category == c))
            .collect()
    }

    /// The categorized plan for the resting grid: recent launches lead ("最近
    /// 使用"), the remaining applications group by their XDG category in the
    /// canonical order — the same grouping the reference sheet shows. Every
    /// section renders one left-aligned row; the section carries its category
    /// so its "更多" button can filter the tag bar down to it. `None` while a
    /// query or a tag filters the grid — filtered results stay one flat block
    /// with a uniform row pitch.
    fn palette_sections<'a>(
        &self,
        host: &'a ViewState,
        ranked: &[&'a DesktopApp],
    ) -> Option<Vec<PaletteSection<'a>>> {
        if !self.query.is_empty() || self.palette_category.is_some() {
            return None;
        }
        let mut recents = Vec::new();
        let mut rest = Vec::new();
        for app in ranked {
            // One row of suggestions at most; the history already ranks them.
            if host.launch_history.get(&app.id).is_some() && recents.len() < GRID_COLUMNS {
                recents.push(*app);
            } else {
                rest.push(*app);
            }
        }
        let mut sections = Vec::new();
        if !recents.is_empty() {
            sections.push(PaletteSection {
                title: "最近使用",
                category: None,
                total: recents.len(),
                apps: recents,
            });
        }
        for category in AppCategory::ALL {
            let mut apps: Vec<&'a DesktopApp> = rest
                .iter()
                .copied()
                .filter(|app| app.category == category)
                .collect();
            if !apps.is_empty() {
                let total = apps.len();
                apps.truncate(GRID_COLUMNS);
                sections.push(PaletteSection {
                    title: category.title(),
                    category: Some(category),
                    apps,
                    total,
                });
            }
        }
        Some(sections)
    }

    /// Command rows for the command list and slash completions, already
    /// narrowed to the entries this shell implements.
    fn palette_commands(&self) -> Vec<&'static Command> {
        let filter = match self.palette_body() {
            PaletteBody::Commands => match self.palette.route(&self.query) {
                Route::Commands(text) => text,
                _ => self.query.clone(),
            },
            PaletteBody::Slash => match self.palette.route(&self.query) {
                Route::Slash { name, .. } => name,
                _ => String::new(),
            },
            PaletteBody::Apps | PaletteBody::Clipboard => return Vec::new(),
        };
        commands::search(&filter, self.palette.mode, self.palette.tool)
            .into_iter()
            .filter(|entry| implemented(entry))
            .collect()
    }

    pub(super) fn palette_row_count(&self, host: &ViewState) -> usize {
        match self.palette_body() {
            PaletteBody::Apps => {
                let visible = self.palette_visible_apps(host);
                if self.palette_grid(host) {
                    if let Some(sections) = self.palette_sections(host, &visible) {
                        return sections.iter().map(|section| section.apps.len()).sum();
                    }
                }
                visible.len()
            }
            PaletteBody::Commands | PaletteBody::Slash => self.palette_commands().len(),
            PaletteBody::Clipboard => self.palette_clipboard(host).len(),
        }
    }

    /// Rows per scroll row and the pitch between them, for keyboard scrolling.
    fn palette_metrics(&self, host: &ViewState) -> (usize, f32) {
        match self.palette_body() {
            PaletteBody::Apps if self.palette_grid(host) => (GRID_COLUMNS, GRID_PITCH),
            PaletteBody::Apps
            | PaletteBody::Clipboard
            | PaletteBody::Commands
            | PaletteBody::Slash => (1, LIST_PITCH),
        }
    }

    /// Keep the keyboard selection inside the visible slice. The rows sit in
    /// one flex column (or a stack of grid rows), so a pixel offset derived
    /// from the uniform pitch is more reliable than a child index. The
    /// categorized resting grid interleaves headers and dividers, so its plan
    /// is walked for the exact offset instead.
    pub(super) fn palette_selection_into_view(&self, host: &ViewState) {
        let viewport = f32::from(self.scroll.bounds().size.height);
        if viewport <= 0. {
            return;
        }
        if self.palette_body() == PaletteBody::Clipboard && self.palette_clipboard_details() {
            // The details list scrolls on its own, in a fixed viewport beside
            // the preview, so it cannot share the sheet's scroll handle.
            let pitch = LIST_PITCH;
            let visible = ((CLIPBOARD_LIST_H / pitch).floor() as usize).max(1);
            let scrolled = (-f32::from(self.clipboard_scroll.offset().y)).max(0.);
            let top = (scrolled / pitch).floor() as usize;
            let row = self.selection;
            let first = if row < top {
                row
            } else if row >= top + visible {
                row + 1 - visible
            } else {
                return;
            };
            self.clipboard_scroll
                .set_offset(point(px(0.), px(-(first as f32 * pitch))));
            return;
        }
        if self.palette_grid(host) && self.query.is_empty() && self.palette_category.is_none() {
            let ranked = self.palette_visible_apps(host);
            if let Some(sections) = self.palette_sections(host, &ranked) {
                // Sections chunk their own rows, so find the selected tile by
                // walking the same plan the grid paints.
                let mut y = 0.;
                let mut first = 0usize;
                let mut target: Option<f32> = None;
                for (si, section) in sections.iter().enumerate() {
                    y += SECTION_HEADER_H + GRID_GAP;
                    let end = first + section.apps.len();
                    if self.selection >= first && self.selection < end {
                        let local = (self.selection - first) / GRID_COLUMNS;
                        target = Some(y + local as f32 * GRID_PITCH);
                        break;
                    }
                    first = end;
                    let rows = section.apps.len().div_ceil(GRID_COLUMNS);
                    y += rows as f32 * GRID_PITCH;
                    if si + 1 != sections.len() {
                        y += SECTION_DIVIDER_H + GRID_GAP;
                    }
                }
                let Some(target) = target else {
                    return;
                };
                let scrolled = (-f32::from(self.scroll.offset().y)).max(0.);
                let offset = if target < scrolled {
                    target
                } else if target + GRID_TILE_H > scrolled + viewport {
                    target + GRID_TILE_H - viewport
                } else {
                    return;
                };
                self.scroll
                    .set_offset(point(px(0.), px(-offset.max(0.))));
                return;
            }
        }
        let (columns, pitch) = self.palette_metrics(host);
        let visible = ((viewport / pitch).floor() as usize).max(1);
        let row = self.selection / columns;
        let scrolled = (-f32::from(self.scroll.offset().y)).max(0.);
        let top = (scrolled / pitch).floor() as usize;
        let first = if row < top {
            row
        } else if row >= top + visible {
            row + 1 - visible
        } else {
            return;
        };
        self.scroll
            .set_offset(point(px(0.), px(-(first as f32 * pitch))));
    }

    /// Replace the field's text so it matches a session that consumed its
    /// draft. Deferred: the field emits `QueryChanged`, whose subscriber
    /// updates this same view.
    pub(super) fn palette_set_query(&self, text: String, cx: &mut Context<Self>) {
        let input = self.input.clone();
        cx.defer(move |cx| {
            input.update(cx, |input, cx| input.set_text(&text, false, cx));
        });
    }

    fn palette_reject(&mut self, message: &str, cx: &mut Context<Self>) {
        self.palette_error = Some(message.to_owned());
        cx.notify();
    }

    pub(super) fn palette_switch_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        let next = self.palette.clone().switch_mode(mode, true);
        let text = next.query.clone();
        self.palette = next;
        self.selection = 0;
        self.palette_error = None;
        self.palette_category = None;
        // The type tags belong to the clipboard body, so switching modes
        // starts it unfiltered.
        self.palette_clipboard_filter = None;
        if text != self.query {
            self.palette_set_query(text, cx);
        }
        cx.notify();
    }

    /// Unwind one palette level: the command editor, a tool page or the last
    /// layout/order override.
    pub(super) fn palette_pop(&mut self, host: &ViewState, cx: &mut Context<Self>) {
        let next = self.palette.clone().pop();
        if next == self.palette {
            return;
        }
        let text = next.query.clone();
        self.palette = next;
        self.selection = 0;
        self.palette_error = None;
        self.palette_category = None;
        if self.palette.mode != Mode::Clipboard {
            self.palette_clipboard_filter = None;
        }
        if text != self.query {
            self.palette_set_query(text, cx);
        }
        self.palette_selection_into_view(host);
        cx.notify();
    }

    /// The application at a flat selection index. The categorized resting
    /// grid reorders applications into sections, so the index follows the
    /// section order when it is on screen; every other body uses the visible
    /// order directly.
    fn palette_app_at<'a>(&self, host: &'a ViewState, index: usize) -> Option<&'a DesktopApp> {
        let visible = self.palette_visible_apps(host);
        if self.palette_grid(host) {
            if let Some(sections) = self.palette_sections(host, &visible) {
                let mut index = index;
                for section in sections {
                    if index < section.apps.len() {
                        return Some(section.apps[index]);
                    }
                    index -= section.apps.len();
                }
                return None;
            }
        }
        visible.get(index).copied()
    }

    /// A tag click: select the category (or clear it on a second click) and
    /// restart the keyboard selection from the top.
    pub(super) fn palette_toggle_category(
        &mut self,
        category: AppCategory,
        cx: &mut Context<Self>,
    ) {
        self.palette_category = if self.palette_category == Some(category) {
            None
        } else {
            Some(category)
        };
        self.selection = 0;
        self.scroll.set_offset(point(px(0.), px(0.)));
        cx.notify();
    }

    /// The keyboard walked the slash completion list, so Enter is allowed to
    /// run the highlighted row even when the field holds a bare `/`.
    pub(super) fn palette_note_slash_navigation(&mut self) {
        if self.palette_body() == PaletteBody::Slash {
            self.palette_slash_picked = true;
        }
    }

    /// Enter (or a click): open the selected application, copy the selected
    /// clipboard entry, run the selected command, or resolve the typed slash
    /// command. `keep_open` is the `Shift+Enter` variant: the clipboard copy
    /// lands but the sheet stays up for the next pick.
    pub(super) fn palette_activate(
        &mut self,
        host: &ViewState,
        keep_open: bool,
        cx: &mut Context<Self>,
    ) {
        match self.palette_body() {
            PaletteBody::Apps => {
                if let Some(app) = self.palette_app_at(host, self.selection) {
                    let id = app.id.clone();
                    self.invoke(Action::Launch(id), cx);
                }
            }
            PaletteBody::Clipboard => {
                let found = self.palette_clipboard(host);
                let Some(entry) = found.get(self.selection) else {
                    return;
                };
                let id = entry.id.clone();
                self.invoke(Action::Service(services::Command::Copy(id)), cx);
                if !keep_open {
                    self.invoke(Action::Close, cx);
                }
            }
            PaletteBody::Commands => {
                if let Some(entry) = self.palette_commands().get(self.selection).copied() {
                    self.palette_run_checked(entry, "", host, cx);
                }
            }
            PaletteBody::Slash => {
                let Route::Slash { name, arguments } = self.palette.route(&self.query) else {
                    return;
                };
                match commands::resolve(&name, &arguments, self.palette.mode, self.palette.tool) {
                    Ok(entry) => self.palette_run_checked(entry, &arguments, host, cx),
                    Err(ResolveError::Scope) => {
                        self.palette_reject("该命令只在对应模式下可用", cx)
                    }
                    Err(ResolveError::Arguments) => self.palette_reject("该命令不接受参数", cx),
                    Err(ResolveError::Unknown) => {
                        // A partial name runs the highlighted completion. A
                        // bare `/` only does so once the keyboard has moved in
                        // the list; otherwise it explains instead of running
                        // the first row.
                        let picked = (!name.is_empty() || self.palette_slash_picked)
                            .then(|| self.palette_commands().get(self.selection).copied())
                            .flatten();
                        let Some(entry) = picked else {
                            self.palette_reject("未知命令；输入 /commands 浏览全部命令", cx);
                            return;
                        };
                        self.palette_run_checked(entry, &arguments, host, cx);
                    }
                }
            }
        }
    }

    /// Rows are clicked from inside the surface: route the command id back
    /// through the host so the same deferred path runs as a key press.
    pub(crate) fn palette_command_by_id(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(entry) = commands::by_id(id) else {
            return;
        };
        if !implemented(entry) {
            self.palette_reject("该命令尚未接入", cx);
            return;
        }
        let host = self.state.clone();
        self.palette_run_checked(entry, "", &host, cx);
    }

    /// Validate a command against the current session, then apply it.
    fn palette_run_checked(
        &mut self,
        entry: &'static Command,
        arguments: &str,
        host: &ViewState,
        cx: &mut Context<Self>,
    ) {
        if !implemented(entry) {
            self.palette_reject("该命令尚未接入", cx);
            return;
        }
        match commands::resolve(entry.slash, arguments, self.palette.mode, self.palette.tool) {
            Ok(_) => self.palette_apply(entry, arguments, host, cx),
            Err(ResolveError::Scope) => self.palette_reject("该命令只在对应模式下可用", cx),
            Err(ResolveError::Arguments) => self.palette_reject("该命令不接受参数", cx),
            Err(ResolveError::Unknown) => self.palette_reject("未知命令", cx),
        }
    }

    fn palette_apply(
        &mut self,
        entry: &'static Command,
        arguments: &str,
        host: &ViewState,
        cx: &mut Context<Self>,
    ) {
        let saved = self.palette_saved_styles(host);
        let next = self.palette.clone().apply_entry(entry, arguments, saved);
        let text = next.query.clone();
        self.palette = next;
        self.selection = 0;
        self.palette_error = None;
        self.palette_category = None;
        if self.palette.mode != Mode::Clipboard {
            self.palette_clipboard_filter = None;
        }
        self.palette_slash_picked = false;
        if text != self.query {
            self.palette_set_query(text, cx);
        }
        if entry.kind == CommandKind::Action {
            // The session consumed the draft; the actual effect is the shell
            // action the entry names.
            let action = match entry.id {
                "theme.light" => Some(Action::Theme("light".into())),
                "theme.dark" => Some(Action::Theme("dark".into())),
                "settings.open" => Some(Action::Settings),
                "settings.launcher.fullscreen" => Some(Action::LauncherPresentation(
                    LauncherPresentation::Fullscreen,
                )),
                "settings.launcher.palette" => Some(Action::LauncherPresentation(
                    LauncherPresentation::Palette,
                )),
                "settings.wallpaper" => Some(Action::PickWallpaper),
                "settings.reduce-motion" => {
                    Some(Action::ReduceMotion(!host.prefs.reduce_motion))
                }
                "settings.dock-autohide" => Some(Action::AutoHide(!host.prefs.dock_auto_hide)),
                "shell.screenshot" => Some(Action::Panel(Panel::Screenshot)),
                "shell.notifications" => Some(Action::Panel(Panel::Notifications)),
                "shell.recent" => Some(Action::Panel(Panel::Recent)),
                "shell.control" => Some(Action::Panel(Panel::Controls)),
                "system.lock" => Some(Action::System(SystemAction::Lock)),
                "system.logout" => Some(Action::System(SystemAction::Logout)),
                "system.suspend" => Some(Action::System(SystemAction::Suspend)),
                "system.reboot" => Some(Action::System(SystemAction::Reboot)),
                "system.shutdown" => Some(Action::System(SystemAction::Shutdown)),
                _ => None,
            };
            if let Some(action) = action {
                self.invoke(action, cx);
            }
        }
        self.palette_selection_into_view(host);
        cx.notify();
    }

    /// The tag row under the search field. Application bodies show every
    /// category the catalog has; clicking a tag filters the applications to
    /// it (a second click clears). Command bodies keep a single mode chip.
    pub(super) fn spotlight_pills(
        &self,
        host: &ViewState,
        p: Palette,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        match self.palette_body() {
            PaletteBody::Apps => self.spotlight_category_tags(host, p, cx),
            PaletteBody::Clipboard => {
                // The tags filter by content type (`Alt+T`/`Alt+I`/`Alt+F`);
                // the layout chip on the right flips the two views (`Alt+D`).
                // Both are shortcuts, so the hints sit on the chips themselves.
                let details = self.palette_clipboard_details();
                let active = self.palette_clipboard_filter;
                let view = cx.entity().downgrade();
                let host = host.clone();
                let mut tags = div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .gap(px(6.))
                    .min_w_0()
                    .overflow_hidden();
                for filter in ClipboardFilter::ALL {
                    let selected = active == Some(filter);
                    let count = self.palette_clipboard_count(&host, filter);
                    let selector = format!("spotlight-clip-tag-{}", filter.key());
                    let view = view.clone();
                    tags = tags.child(
                        div()
                            .id(SharedString::from(format!(
                                "spotlight-clip-tag-{}",
                                filter.key()
                            )))
                            .debug_selector(move || selector.clone())
                            .h(px(24.))
                            .px(px(10.))
                            .rounded(px(12.))
                            .flex()
                            .items_center()
                            .gap(px(5.))
                            .flex_shrink_0()
                            .text_size(px(12.))
                            .bg(if selected {
                                p.accent.opacity(0.20)
                            } else {
                                p.fg.opacity(0.06)
                            })
                            .hover(move |tag| {
                                tag.bg(if selected {
                                    p.accent.opacity(0.28)
                                } else {
                                    p.fg.opacity(0.11)
                                })
                            })
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(move |_, _, cx| {
                                let _ = view.update(cx, |view, cx| {
                                    view.palette_toggle_clipboard_filter(filter, cx)
                                });
                            })
                            .child(menu_text(
                                filter.title().to_owned(),
                                FontWeight::BOLD,
                                if selected { p.accent } else { p.muted },
                                cx,
                            ))
                            .child(menu_text(
                                count.to_string(),
                                FontWeight::NORMAL,
                                p.muted,
                                cx,
                            ))
                            .child(
                                div().text_size(px(10.)).child(menu_text(
                                    format!(
                                        "Alt+{}",
                                        filter
                                            .key()
                                            .chars()
                                            .next()
                                            .unwrap_or(' ')
                                            .to_uppercase()
                                    ),
                                    FontWeight::NORMAL,
                                    p.muted,
                                    cx,
                                )),
                            ),
                    );
                }
                Some(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(tags)
                        .child(
                            div()
                                .id("spotlight-clipboard-layout")
                                .debug_selector(|| "spotlight-clipboard-layout".to_owned())
                                .h(px(24.))
                                .px(px(11.))
                                .rounded(px(12.))
                                .flex()
                                .flex_shrink_0()
                                .items_center()
                                .gap(px(5.))
                                .text_size(px(12.))
                                .bg(p.fg.opacity(0.06))
                                .cursor_pointer()
                                .hover(move |chip| chip.bg(p.line))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(move |_, _, cx| {
                                    let _ = view.update(cx, |view, cx| {
                                        view.palette_toggle_clipboard_layout(&host, cx)
                                    });
                                })
                                .child(menu_text(
                                    if details { "紧凑" } else { "详情" }.to_owned(),
                                    FontWeight::SEMIBOLD,
                                    p.muted,
                                    cx,
                                ))
                                .child(
                                    div().text_size(px(10.)).child(menu_text(
                                        "Alt+D".to_owned(),
                                        FontWeight::NORMAL,
                                        p.muted,
                                        cx,
                                    )),
                                ),
                        )
                        .into_any_element(),
                )
            }
            PaletteBody::Commands => Some(self.spotlight_chip("命令", true, p, cx)),
            PaletteBody::Slash => Some(self.spotlight_chip("命令补全", true, p, cx)),
        }
    }

    /// One quiet status chip.
    fn spotlight_chip(
        &self,
        label: &str,
        accent: bool,
        p: Palette,
        cx: &Context<Self>,
    ) -> AnyElement {
        div()
            .h(px(24.))
            .px(px(11.))
            .rounded(px(12.))
            .flex()
            .items_center()
            .flex_shrink_0()
            .text_size(px(12.))
            .bg(if accent {
                p.accent.opacity(0.16)
            } else {
                p.fg.opacity(0.06)
            })
            .child(menu_text(
                label.to_owned(),
                FontWeight::SEMIBOLD,
                if accent { p.accent } else { p.muted },
                cx,
            ))
            .into_any_element()
    }

    /// Every category the catalog actually has, in canonical order, as
    /// selectable tags. The row hugs the left edge and never scrolls.
    fn spotlight_category_tags(
        &self,
        host: &ViewState,
        p: Palette,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let present: Vec<AppCategory> = AppCategory::ALL
            .iter()
            .copied()
            .filter(|category| {
                host.catalog
                    .apps()
                    .iter()
                    .any(|app| app.category == *category)
            })
            .collect();
        if present.is_empty() {
            return None;
        }
        let view = cx.entity().downgrade();
        let mut tags = div().flex().items_center().gap(px(6.)).text_size(px(14.));
        for category in present {
            let selected = self.palette_category == Some(category);
            let view = view.clone();
            let selector = format!("spotlight-tag-{}", category.key());
            tags = tags.child(
                div()
                    .id(SharedString::from(format!("spotlight-tag-{}", category.key())))
                    .debug_selector(move || selector.clone())
                    .h(px(24.))
                    .px(px(10.))
                    .rounded(px(12.))
                    .flex()
                    .items_center()
                    .flex_shrink_0()
                    .bg(if selected {
                        p.accent.opacity(0.20)
                    } else {
                        p.fg.opacity(0.06)
                    })
                    .hover(move |tag| {
                        tag.bg(if selected {
                            p.accent.opacity(0.28)
                        } else {
                            p.fg.opacity(0.11)
                        })
                    })
                    .cursor_pointer()
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        let _ =
                            view.update(cx, |view, cx| view.palette_toggle_category(category, cx));
                    })
                    .child(menu_text(
                        category.title().to_owned(),
                        FontWeight::BOLD,
                        if selected { p.accent } else { p.muted },
                        cx,
                    )),
            );
        }
        Some(tags.into_any_element())
    }

    /// Result rows for the palette. Every row is a direct child of the
    /// scrolling container so the pixel-based scroll helper stays exact.
    pub(super) fn spotlight_results(
        &self,
        host: &ViewState,
        p: Palette,
        cx: &Context<Self>,
    ) -> AnyElement {
        match self.palette_body() {
            PaletteBody::Apps => {
                if self.palette_grid(host) {
                    self.spotlight_grid(host, p, cx)
                } else {
                    self.spotlight_apps(host, p, cx)
                }
            }
            PaletteBody::Clipboard => self.spotlight_clipboard(host, p, cx),
            PaletteBody::Commands | PaletteBody::Slash => self.spotlight_commands(p, cx),
        }
    }

    fn spotlight_apps(&self, host: &ViewState, p: Palette, cx: &Context<Self>) -> AnyElement {
        let found = self.palette_visible_apps(host);
        if found.is_empty() {
            return palette_empty(
                if self.query.is_empty() {
                    "没有可用的应用"
                } else {
                    "没有匹配的应用"
                },
                "试试应用名称或关键词，输入 / 可以执行命令",
                p,
                cx,
            );
        }
        let mut list = div().flex().flex_col().gap(px(LIST_GAP)).pb(px(2.));
        for (index, app) in found.iter().enumerate() {
            let selected = index == self.selection;
            let id = app.id.clone();
            let row_selector = format!("spotlight-app-{id}");
            let name = app.name.clone();
            let generic = app.generic_name.clone();
            let icon = app.icon_path.clone();
            let context = id.clone();
            let h = self.host.clone();
            list = list.child(
                div()
                    .id(SharedString::from(row_selector.clone()))
                    .debug_selector(move || row_selector.clone())
                    .h(px(LIST_ROW_H))
                    .flex_shrink_0()
                    .px(px(12.))
                    .rounded(px(14.))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .when(selected, |row| row.bg(p.accent.opacity(0.20)))
                    .hover(move |row| row.bg(p.line))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(self.icon(icon, 36., p))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(1.))
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .text_color(p.fg)
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(menu_text(name, FontWeight::SEMIBOLD, p.fg, cx)),
                            )
                            .when(!generic.is_empty(), |text| {
                                text.child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(p.muted)
                                        .whitespace_nowrap()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .child(menu_text(
                                            generic,
                                            FontWeight::MEDIUM,
                                            p.muted,
                                            cx,
                                        )),
                                )
                            }),
                    )
                    .when(selected, |row| {
                        row.child(
                            div()
                                .text_size(px(10.))
                                .text_color(p.muted)
                                .child(menu_text("↵".into(), FontWeight::NORMAL, p.muted, cx)),
                        )
                    })
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        h.update(cx, |h, cx| h.handle(Action::Launch(id.clone()), cx));
                    })
                    .on_mouse_down(MouseButton::Right, {
                        // The menu is a surface of its own over the launcher: the
                        // compositor can frost the card, and no pointer event
                        // reaches the rows underneath.
                        let output = self.output.clone();
                        let view = cx.entity().downgrade();
                        let id = context.clone();
                        move |event, _, cx| {
                            cx.stop_propagation();
                            let anchor = (
                                f32::from(event.position.x).round() as i32,
                                f32::from(event.position.y).round() as i32,
                            );
                            let action = Action::on_output(
                                output.clone(),
                                Action::OpenMenu {
                                    id: id.clone(),
                                    anchor,
                                },
                            );
                            let _ = view.update(cx, |view, cx| view.invoke(action, cx));
                        }
                    }),
            );
        }
        list.into_any_element()
    }

    fn spotlight_grid(&self, host: &ViewState, p: Palette, cx: &Context<Self>) -> AnyElement {
        // The visible list already carries the category tag's filter; the
        // sections only reorder it when no tag is selected.
        let found = self.palette_visible_apps(host);
        if found.is_empty() {
            return palette_empty("没有匹配的应用", "试试应用名称或关键词", p, cx);
        }
        let mut grid = div().flex().flex_col().gap(px(GRID_GAP)).pb(px(2.));
        match self.palette_sections(host, &found) {
            Some(sections) => {
                // The categorized resting grid: one left-aligned row per
                // section; the header carries the count and — when the
                // category overflows the row — a "更多" button that narrows
                // the tag bar to this category.
                let last = sections.len().saturating_sub(1);
                let mut index = 0;
                for (si, section) in sections.iter().enumerate() {
                    grid = grid
                        .child(self.spotlight_category_row(section, p, cx))
                        .child(self.spotlight_tile_row(&section.apps, index, p, cx));
                    index += section.apps.len();
                    if si != last {
                        grid = grid.child(self.spotlight_divider(p));
                    }
                }
            }
            None => {
                // A query filters the grid into one flat block.
                for (row_index, chunk) in found.chunks(GRID_COLUMNS).enumerate() {
                    grid = grid.child(self.spotlight_tile_row(
                        chunk,
                        row_index * GRID_COLUMNS,
                        p,
                        cx,
                    ));
                }
            }
        }
        grid.into_any_element()
    }

    /// One row of grid tiles; `first` is the selection index of the row's
    /// first application.
    fn spotlight_tile_row(
        &self,
        apps: &[&DesktopApp],
        first: usize,
        p: Palette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let mut row = div().flex_shrink_0().flex();
        for column in 0..GRID_COLUMNS {
            let Some(app) = apps.get(column) else {
                // Empty slots hold the right-hand space of partial rows so
                // the painted tiles keep their column width.
                row = row.child(div().flex_1().min_w_0());
                continue;
            };
            let index = first + column;
            let selected = index == self.selection;
            let id = app.id.clone();
            let context = id.clone();
            let selector = format!("spotlight-tile-{id}");
            let name = app.name.clone();
            let icon = app.icon_path.clone();
            let h = self.host.clone();
            row = row.child(
                div().flex_1().min_w_0().flex().justify_center().child(
                    div()
                        .id(SharedString::from(format!("spotlight-tile-{id}")))
                        .debug_selector(move || selector.clone())
                        .w(px(GRID_TILE_W))
                        .h(px(GRID_TILE_H))
                        .flex_shrink_0()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(8.))
                        .rounded(px(12.))
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            // Only the chosen tile carries a plate, like the
                            // fullscreen launcher: the label below never takes
                            // the tint.
                            div()
                                .id(SharedString::from(format!(
                                    "spotlight-tile-plate-{id}"
                                )))
                                .size(px(GRID_PLATE))
                                .flex_shrink_0()
                                .rounded(px(20.))
                                .bg(if selected {
                                    p.accent.opacity(0.30)
                                } else {
                                    p.fg.opacity(0.06)
                                })
                                .hover(move |plate| {
                                    plate.bg(if selected {
                                        p.accent.opacity(0.40)
                                    } else {
                                        p.fg.opacity(0.11)
                                    })
                                })
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(self.icon(icon, 72., p)),
                        )
                        .child(
                            div()
                                .w(px(GRID_TILE_W - 12.))
                                .text_size(px(12.))
                                .line_height(px(15.))
                                .text_color(p.fg)
                                .text_center()
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(menu_text(name, FontWeight::MEDIUM, p.fg, cx)),
                        )
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            h.update(cx, |h, cx| h.handle(Action::Launch(id.clone()), cx));
                        })
                        .on_mouse_down(MouseButton::Right, {
                            // The menu is a surface of its own over the
                            // launcher: the compositor can frost the card, and
                            // no pointer event reaches the tiles underneath.
                            let output = self.output.clone();
                            let view = cx.entity().downgrade();
                            let id = context.clone();
                            move |event, _, cx| {
                                cx.stop_propagation();
                                let anchor = (
                                    f32::from(event.position.x).round() as i32,
                                    f32::from(event.position.y).round() as i32,
                                );
                                let action = Action::on_output(
                                    output.clone(),
                                    Action::OpenMenu {
                                        id: id.clone(),
                                        anchor,
                                    },
                                );
                                let _ = view.update(cx, |view, cx| view.invoke(action, cx));
                            }
                        }),
                ),
            );
        }
        row.into_any_element()
    }

    /// A section header: bold category title on the left; on the right the
    /// application count and — when the category overflows its single row —
    /// a "更多" button that selects just this category in the tag bar.
    fn spotlight_category_row(
        &self,
        section: &PaletteSection,
        p: Palette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let overflow = section.category.is_some() && section.total > GRID_COLUMNS;
        let mut trailing = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(p.muted)
                    .whitespace_nowrap()
                    .child(menu_text(
                        format!("{} 项", section.total),
                        FontWeight::MEDIUM,
                        p.muted,
                        cx,
                    )),
            );
        if overflow {
            let view = cx.entity().downgrade();
            let category = section.category.expect("overflow implies a category");
            let selector = format!("spotlight-more-{}", category.key());
            trailing = trailing.child(
                div()
                    .id(SharedString::from(selector.clone()))
                    .debug_selector(move || selector.clone())
                    .px(px(4.))
                    .rounded(px(4.))
                    .text_size(px(13.))
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .hover(move |more| more.bg(p.line))
                    .child(menu_text("更多".into(), FontWeight::SEMIBOLD, p.accent, cx))
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        let _ =
                            view.update(cx, |view, cx| view.palette_toggle_category(category, cx));
                    }),
            );
        }
        div()
            .h(px(SECTION_HEADER_H))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_size(px(15.))
                    .child(menu_text(section.title.to_owned(), FontWeight::BOLD, p.fg, cx)),
            )
            .child(trailing)
            .into_any_element()
    }

    /// The divider between categorized sections: the same two-point line the
    /// header uses, carried by an eight-point block so the scroll maths stay
    /// uniform.
    fn spotlight_divider(&self, p: Palette) -> AnyElement {
        div()
            .h(px(SECTION_DIVIDER_H))
            .flex_shrink_0()
            .py(px(4.))
            .child(
                div()
                    .h(px(1.))
                    .debug_selector(|| "spotlight-rule-section".to_owned())
                    .bg(p.line),
            )
            .into_any_element()
    }

    /// The clipboard entries the palette offers. The service already sorts
    /// pinned first, then newest; a typed query narrows to text the same way
    /// the reference sheet does (images carry no text to match).
    fn palette_clipboard<'a>(&self, host: &'a ViewState) -> Vec<&'a services::HistoryEntry> {
        let query = self.query.trim().to_lowercase();
        host.history
            .iter()
            .filter(|entry| query.is_empty() || entry.text.to_lowercase().contains(&query))
            .filter(|entry| {
                self.palette_clipboard_filter
                    .is_none_or(|filter| filter.accepts(entry.kind))
            })
            .collect()
    }

    /// How many entries a type tag would show. Counts follow the query but not
    /// the active tag, so the other tags keep their numbers while one is on.
    fn palette_clipboard_count(&self, host: &ViewState, filter: ClipboardFilter) -> usize {
        let query = self.query.trim().to_lowercase();
        host.history
            .iter()
            .filter(|entry| query.is_empty() || entry.text.to_lowercase().contains(&query))
            .filter(|entry| filter.accepts(entry.kind))
            .count()
    }

    /// Whether the clipboard body shows the list-plus-preview layout
    /// (`/detail`) instead of the compact rows (`/compact`).
    fn palette_clipboard_details(&self) -> bool {
        self.palette.effective("clipboardLayout", "default") == "details"
    }

    /// The clipboard type tags: clicking one filters the list to that content
    /// type, a second click clears it. `Alt+T`/`Alt+I`/`Alt+F` do the same.
    pub(super) fn palette_toggle_clipboard_filter(
        &mut self,
        filter: ClipboardFilter,
        cx: &mut Context<Self>,
    ) {
        self.palette_clipboard_filter = if self.palette_clipboard_filter == Some(filter) {
            None
        } else {
            Some(filter)
        };
        self.selection = 0;
        self.scroll.set_offset(point(px(0.), px(0.)));
        self.clipboard_scroll.set_offset(point(px(0.), px(0.)));
        cx.notify();
    }

    /// The clipboard-only shortcut handler (`Alt+T`/`Alt+I`/`Alt+F`). Other
    /// modes keep these letters for their own business.
    pub(super) fn palette_clipboard_shortcut(
        &mut self,
        filter: ClipboardFilter,
        cx: &mut Context<Self>,
    ) {
        if self.state.panel != Some(Panel::Spotlight) || self.palette.mode != Mode::Clipboard {
            return;
        }
        self.palette_toggle_clipboard_filter(filter, cx);
    }

    /// The chip beside the clipboard count flips between the compact list and
    /// the details columns — the same switch `/detail` and `/compact` make,
    /// without having to know the command names. `Alt+D` does the same.
    pub(super) fn palette_toggle_clipboard_layout(
        &mut self,
        host: &ViewState,
        cx: &mut Context<Self>,
    ) {
        let id = if self.palette_clipboard_details() {
            "clipboard.compact"
        } else {
            "clipboard.detail"
        };
        let Some(entry) = commands::by_id(id) else {
            return;
        };
        self.palette_apply(entry, "", host, cx);
    }

    /// The plate glyph for one clipboard kind, through the theme's own icons.
    fn clipboard_glyph(&self, kind: services::HistoryKind) -> PathBuf {
        match kind {
            services::HistoryKind::Text => {
                self.themed_glyph_path("edit-paste-symbolic", "clipboard.svg")
            }
            services::HistoryKind::Image => {
                self.themed_glyph_path("image-round-symbolic", "wallpaper.svg")
            }
            services::HistoryKind::Screenshot => {
                self.themed_glyph_path("screenshooter-symbolic", "capture-screen.svg")
            }
            services::HistoryKind::Files => {
                self.themed_glyph_path("folder-symbolic", "generic-folder.svg")
            }
        }
    }

    /// One clipboard row. `select_only` keeps the row a plain selection target
    /// (the details list); the compact list copies and closes on click, the
    /// same thing Enter does.
    fn clipboard_row(
        &self,
        entry: &services::HistoryEntry,
        index: usize,
        select_only: bool,
        p: Palette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let selected = index == self.selection;
        let id = entry.id.clone();
        let title = clipboard_title(entry);
        let meta = clipboard_meta(entry);
        let glyph = self.clipboard_glyph(entry.kind);
        let thumbnail = self
            .image_cache
            .get(&format!("history:{}", entry.id))
            .cloned();
        let row_selector = format!("spotlight-clip-{id}");
        let hint_selector = format!("spotlight-clip-enter-{id}");
        let host = self.host.clone();
        let view = cx.entity().downgrade();
        let copy = entry.id.clone();
        div()
            .id(SharedString::from(row_selector.clone()))
            .debug_selector(move || row_selector.clone())
            .h(px(LIST_ROW_H))
            .flex_shrink_0()
            .px(px(12.))
            .rounded(px(14.))
            .flex()
            .items_center()
            .gap(px(12.))
            .when(selected, |row| row.bg(p.accent.opacity(0.20)))
            .hover(move |row| row.bg(p.line))
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .size(px(36.))
                    .flex_shrink_0()
                    .rounded(px(10.))
                    .bg(p.card)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(match thumbnail {
                        Some(image) => img(image)
                            .size(px(30.))
                            .object_fit(ObjectFit::Contain)
                            .into_any_element(),
                        None => self.glyph(&glyph, 20., p),
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(
                        div()
                            .text_size(px(14.))
                            .text_color(p.fg)
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(menu_text(title, FontWeight::SEMIBOLD, p.fg, cx)),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(p.muted)
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(menu_text(meta, FontWeight::NORMAL, p.muted, cx)),
                    ),
            )
            .when(selected, |row| {
                row.child(
                    div()
                        .debug_selector(move || hint_selector.clone())
                        .text_size(px(10.))
                        .text_color(p.muted)
                        .child(menu_text("↵".into(), FontWeight::NORMAL, p.muted, cx)),
                )
            })
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                if select_only {
                    let _ = view.update(cx, |view, cx| {
                        view.selection = index;
                        cx.notify();
                    });
                    return;
                }
                host.update(cx, |host, cx| {
                    host.handle(Action::Service(services::Command::Copy(copy.clone())), cx);
                    host.handle(Action::Close, cx);
                });
            })
            .into_any_element()
    }

    /// The clipboard body: compact rows, or the details layout with the list on
    /// the left and one entry's preview on the right.
    fn spotlight_clipboard(&self, host: &ViewState, p: Palette, cx: &Context<Self>) -> AnyElement {
        let found = self.palette_clipboard(host);
        if found.is_empty() {
            let hint = if self.palette_clipboard_filter.is_some() && self.query.is_empty() {
                "再按一次同一个标签（或它的快捷键）就显示全部类型"
            } else if host.clipboard_paused {
                "记录已暂停；在“最近内容”里可以恢复记录"
            } else if host.clipboard_available {
                "复制文字、图片或完成截图后会出现在这里"
            } else {
                "剪贴板服务尚未连接，已有内容会保留"
            };
            let title = if !self.query.is_empty() {
                "没有匹配的剪贴记录".to_owned()
            } else if let Some(filter) = self.palette_clipboard_filter {
                format!("还没有{}记录", filter.title())
            } else {
                "剪贴板里还没有内容".to_owned()
            };
            return palette_empty(title, hint, p, cx);
        }
        if !self.palette_clipboard_details() {
            let mut list = div().flex().flex_col().gap(px(LIST_GAP)).pb(px(2.));
            for (index, entry) in found.iter().enumerate() {
                list = list.child(self.clipboard_row(entry, index, false, p, cx));
            }
            return list.into_any_element();
        }
        let mut list = div()
            .id("spotlight-clipboard-list")
            .debug_selector(|| "spotlight-clipboard-list".to_owned())
            .w(px(CLIPBOARD_LIST_W))
            .h(px(CLIPBOARD_LIST_H))
            .flex_shrink_0()
            .overflow_y_scroll()
            .track_scroll(&self.clipboard_scroll)
            .flex()
            .flex_col()
            .gap(px(LIST_GAP))
            .pr(px(8.));
        for (index, entry) in found.iter().enumerate() {
            list = list.child(self.clipboard_row(entry, index, true, p, cx));
        }
        let preview = found
            .get(self.selection)
            .map(|entry| self.clipboard_preview(entry, p, cx))
            .unwrap_or_else(|| div().into_any_element());
        div()
            .flex()
            .gap(px(16.))
            .child(list)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h(px(CLIPBOARD_LIST_H))
                    .child(preview),
            )
            .into_any_element()
    }

    /// The details layout's right column: the selected entry's payload, its
    /// metadata line, and the same copy/pin/remove actions the recent panel
    /// offers.
    fn clipboard_preview(
        &self,
        entry: &services::HistoryEntry,
        p: Palette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let kind = match entry.kind {
            services::HistoryKind::Text => "文字",
            services::HistoryKind::Image => "图片",
            services::HistoryKind::Screenshot => "截图",
            services::HistoryKind::Files => "文件",
        };
        let title = clipboard_title(entry);
        let id = entry.id.clone();
        let pinned = entry.pinned;
        let mut card = div()
            .size_full()
            .rounded(px(16.))
            .bg(p.fg.opacity(0.04))
            .p(px(14.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(p.accent)
                            .child(menu_text(kind.to_owned(), FontWeight::BOLD, p.accent, cx)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(12.))
                            .text_color(p.muted)
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(menu_text(title, FontWeight::NORMAL, p.muted, cx)),
                    )
                    .when(pinned, |row| {
                        row.child(div().text_size(px(10.)).text_color(p.accent).child(
                            menu_text("已置顶".into(), FontWeight::BOLD, p.accent, cx),
                        ))
                    }),
            );
        match entry.kind {
            services::HistoryKind::Text | services::HistoryKind::Files => {
                card = card.child(
                    div()
                        .id("spotlight-clipboard-text")
                        .debug_selector(|| "spotlight-clipboard-text".to_owned())
                        .flex_1()
                        .min_h_0()
                        .overflow_hidden()
                        .text_size(px(13.))
                        .line_height(px(19.))
                        .text_color(p.fg)
                        .child(entry.text.chars().take(1600).collect::<String>()),
                );
            }
            services::HistoryKind::Image | services::HistoryKind::Screenshot => {
                let image = self
                    .image_cache
                    .get(&format!("history:{}", entry.id))
                    .cloned();
                card = card.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .overflow_hidden()
                        .child(match image {
                            Some(image) => img(image)
                                .max_w(px(330.))
                                .max_h(px(280.))
                                .object_fit(ObjectFit::Contain)
                                .into_any_element(),
                            None => div()
                                .text_size(px(12.))
                                .text_color(p.muted)
                                .child("缩略图不可用")
                                .into_any_element(),
                        }),
                );
                if let Some(path) = &entry.file_path {
                    let path = path.to_string_lossy().into_owned();
                    card = card.child(
                        div()
                            .text_size(px(11.))
                            .text_color(p.muted)
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(menu_text(path, FontWeight::NORMAL, p.muted, cx)),
                    );
                }
            }
        }
        let stats = format!(
            "{} · {} · {}",
            entry.mime,
            clipboard_size(entry.bytes),
            clipboard_age(entry.created_at)
        );
        card.child(
            div()
                .text_size(px(11.))
                .text_color(p.muted)
                .child(menu_text(stats, FontWeight::NORMAL, p.muted, cx)),
        )
        .child(
            div()
                .flex()
                .gap(px(6.))
                .child(self.button(
                    format!("clip-copy-{id}"),
                    "复制",
                    Action::Service(services::Command::Copy(id.clone())),
                    p,
                ))
                .child(self.button(
                    format!("clip-pin-{id}"),
                    if pinned { "取消置顶" } else { "置顶" },
                    Action::Service(services::Command::Pin(id.clone())),
                    p,
                ))
                .child(self.button(
                    format!("clip-delete-{id}"),
                    "移除",
                    Action::Service(services::Command::Delete(id.clone())),
                    p,
                )),
        )
        .into_any_element()
    }

    /// Delete on an empty field drops the highlighted clipboard entry and
    /// keeps the keyboard on its neighbour.
    pub(super) fn palette_delete_selected(&mut self, host: &ViewState, cx: &mut Context<Self>) {
        if self.palette_body() != PaletteBody::Clipboard {
            return;
        }
        let found = self.palette_clipboard(host);
        let Some(entry) = found.get(self.selection) else {
            return;
        };
        let id = entry.id.clone();
        self.selection = self.selection.min(found.len().saturating_sub(2));
        self.invoke(Action::Service(services::Command::Delete(id)), cx);
        cx.notify();
    }

    /// The leading path for a command row: the theme's own icon when the user
    /// named one, else the bundled glyph through its theme alias, else the
    /// bundled file itself.
    fn command_glyph_path(&self, command: &Command) -> PathBuf {
        let mut cache = self.theme_icons.borrow_mut();
        if let Some(symbol) = command_symbol(command.id) {
            if let Some(path) = theme_icon(symbol, &mut cache) {
                return path;
            }
        }
        let asset = command_icon(command);
        symbolic_name(asset)
            .and_then(|alias| theme_icon(alias, &mut cache))
            .unwrap_or_else(|| icon_path(asset))
    }

    fn spotlight_commands(&self, p: Palette, cx: &Context<Self>) -> AnyElement {
        let found = self.palette_commands();
        if found.is_empty() {
            return palette_empty(
                "没有匹配的命令",
                "输入 /commands 查看全部命令",
                p,
                cx,
            );
        }
        let mut list = div().flex().flex_col().gap(px(LIST_GAP)).pb(px(2.));
        for (index, entry) in found.iter().enumerate() {
            let selected = index == self.selection;
            let id = entry.id;
            let slash = entry.slash;
            let title = entry.title;
            let kind = kind_label(entry.kind);
            let icon = self.command_glyph_path(entry);
            let row_selector = format!("spotlight-command-{id}");
            let icon_selector = format!("spotlight-command-icon-{id}");
            let subtitle_selector = format!("spotlight-command-subtitle-{id}");
            let h = self.host.clone();
            list = list.child(
                div()
                    .id(SharedString::from(row_selector.clone()))
                    .debug_selector(move || row_selector.clone())
                    .h(px(LIST_ROW_H))
                    .flex_shrink_0()
                    .px(px(12.))
                    .rounded(px(14.))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .when(selected, |row| row.bg(p.accent.opacity(0.20)))
                    .hover(move |row| row.bg(p.line))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .debug_selector(move || icon_selector.clone())
                            .size(px(36.))
                            .rounded(px(10.))
                            .bg(p.card)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(self.glyph(&icon, 22., p)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(1.))
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .text_color(p.fg)
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(menu_text(
                                        title.to_owned(),
                                        FontWeight::SEMIBOLD,
                                        p.fg,
                                        cx,
                                    )),
                            )
                            .child(
                                div()
                                    .debug_selector(move || subtitle_selector.clone())
                                    .text_size(px(11.))
                                    .text_color(p.muted)
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(menu_text(
                                        format!("/{slash}"),
                                        FontWeight::MEDIUM,
                                        p.muted,
                                        cx,
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(p.muted)
                            .whitespace_nowrap()
                            .child(menu_text(kind.into(), FontWeight::MEDIUM, p.muted, cx)),
                    )
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        h.update(cx, |h, cx| {
                            h.handle(Action::PaletteCommand(id.to_owned()), cx)
                        });
                    }),
            );
        }
        list.into_any_element()
    }
}

fn palette_empty(
    title: impl Into<String>,
    hint: &'static str,
    p: Palette,
    cx: &Context<ShellView>,
) -> AnyElement {
    div()
        .py(px(32.))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(5.))
        .child(menu_text(title.into(), FontWeight::SEMIBOLD, p.fg, cx))
        .child(
            div()
                .text_size(px(12.))
                .text_color(p.muted)
                .child(menu_text(hint.into(), FontWeight::MEDIUM, p.muted, cx)),
        )
        .into_any_element()
}
