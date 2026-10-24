// SPDX-License-Identifier: GPL-3.0-or-later
//! Floating macOS Dock. The shadow canvas is outside the tray's layout budget.
use super::*;
use super::dock_folder::FolderEntryDrag;

/// Hover tooltip text size.
const TOOLTIP_TEXT_SIZE: f32 = 13.;
/// Inner horizontal padding of the hover tooltip.
const TOOLTIP_PAD: f32 = 10.;
/// The shortest and longest hover tooltip, in logical pixels. The label grows
/// with the application name between these bounds.
const TOOLTIP_MIN_W: f32 = 64.;
const TOOLTIP_MAX_W: f32 = 220.;
/// How long the pointer must rest on an icon before its name appears. macOS
/// waits instead of showing the label the moment the pointer arrives.
pub(super) const TOOLTIP_DELAY: std::time::Duration = std::time::Duration::from_secs(1);
/// The gap between the hovered tile and its label. Magnification lifts and
/// widens the icon, so the label clears the enlarged tile rather than touching
/// its resting edge.
const TOOLTIP_GAP: f32 = 14.;

fn utility_icon(name: &'static str) -> Option<PathBuf> {
    static ICONS: std::sync::OnceLock<std::collections::HashMap<&'static str, PathBuf>> =
        std::sync::OnceLock::new();
    ICONS
        .get_or_init(|| {
            let mut roots = Vec::new();
            if let Some(prefix) = std::env::current_exe()
                .ok()
                .and_then(|path| path.parent().and_then(|p| p.parent()).map(PathBuf::from))
            {
                roots.push(prefix.join("assets/icons"));
                roots.push(prefix.join("share/clarion-shell/icons"));
            }
            roots.extend([
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/icons"),
                PathBuf::from("/usr/share/clarion-shell/icons"),
                PathBuf::from("/usr/local/share/clarion-shell/icons"),
            ]);
            ["launchpad.png", "trash-empty.png", "trash-full.png"]
                .into_iter()
                .filter_map(|name| {
                    roots
                        .iter()
                        .map(|root| root.join(name))
                        .find(|path| path.is_file())
                        .map(|path| (name, path))
                })
                .collect()
        })
        .get(name)
        .cloned()
}

fn dock_slot(id: impl Into<SharedString>, size: f32) -> Stateful<Div> {
    let id = id.into();
    let selector = id.to_string();
    div()
        .id(id)
        .debug_selector(move || selector.clone())
        .absolute()
        .w(px(size))
        .h(px(size))
        .cursor_pointer()
        .active(|style| style.opacity(0.65))
}

/// The "+" affordance on an app tile that accepts the file being dragged. It
/// sits on the icon itself instead of drawing a frame around the tile.
fn drop_accept_badge(size: f32, p: Palette) -> AnyElement {
    let badge = size * 0.42;
    div()
        .id("dock-drag-accept")
        .debug_selector(|| "dock-drag-accept".into())
        .absolute()
        .left(px((size - badge) / 2.))
        .top(px((size - badge) / 2.))
        .size(px(badge))
        .rounded_full()
        .bg(p.accent)
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgba(0xffffffff))
        .text_size(px(badge * 0.72))
        .font_weight(FontWeight::BOLD)
        .child("+")
        .into_any_element()
}

pub(super) fn folder_preview_entries(
    entries: &[crate::dock_folder::FolderEntry],
) -> Vec<&crate::dock_folder::FolderEntry> {
    use crate::dock_folder::FolderEntryKind;
    let mut preview = entries
        .iter()
        .filter(|entry| entry.kind != FolderEntryKind::Directory)
        .take(3)
        .collect::<Vec<_>>();
    for entry in entries
        .iter()
        .filter(|entry| entry.kind == FolderEntryKind::Directory)
    {
        if preview.len() >= 3 {
            break;
        }
        preview.push(entry);
    }
    preview
}

fn folder_tile_icon(
    folder: Option<&crate::app::DockFolderView>,
    size: f32,
    open: bool,
    p: Palette,
) -> AnyElement {
    use crate::desktop_icons::{icon, rotated_icon, DesktopIconKind};
    let Some(folder) = folder else {
        return icon(DesktopIconKind::Folder, size);
    };
    let entries = folder
        .snapshot
        .as_ref()
        .map(|snapshot| snapshot.entries.as_slice())
        .unwrap_or(&[]);
    if open {
        // The open folder is replaced by a rounded collapse control at the
        // Dock anchor; folded content is indicated by the downward chevron.
        return div()
            .size(px(size))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .id("dock-folder-open")
                    .debug_selector(|| "dock-folder-open".into())
                    .size(px(size))
                    .rounded(px(size * 0.22))
                    .border_1()
                    .border_color(p.line)
                    .bg(p.card)
                    .flex()
                    .items_center()
                    .justify_center()
                    .pb(px(size * 0.1))
                    .text_color(p.fg)
                    .text_size(px(size * 0.68))
                    .child("⌄"),
            )
            .into_any_element();
    }
    if folder.display == crate::preferences::FolderDisplay::Folder || entries.is_empty() {
        return icon(DesktopIconKind::Folder, size);
    }
    let mut stack = div().relative().w(px(size)).h(px(size));
    let mut picked = folder_preview_entries(entries);
    picked.reverse();
    let artwork_size = size;
    for (layer, entry) in picked.into_iter().enumerate() {
        let offset = layer as f32 * 0.03;
        let kind = match entry.kind {
            crate::dock_folder::FolderEntryKind::Directory => DesktopIconKind::Folder,
            crate::dock_folder::FolderEntryKind::Application => DesktopIconKind::Application,
            crate::dock_folder::FolderEntryKind::Image => DesktopIconKind::Image,
            crate::dock_folder::FolderEntryKind::Document => DesktopIconKind::Document,
        };
        let angle = match layer {
            0 => -16.,
            1 => 8.,
            _ => 14.,
        };
        stack = stack.child(
            div()
                .absolute()
                .left(px(size * offset))
                .top(px(size * offset))
                .child(rotated_icon(kind, angle, artwork_size)),
        );
    }
    stack.into_any_element()
}

fn dock_separator(vertical: bool, dark: bool) -> Div {
    div()
        .absolute()
        .when(vertical, |s| {
            s.top(px(-1.)).left(px(8.)).right(px(8.)).h(px(2.))
        })
        .when(!vertical, |s| {
            s.left(px(-1.)).top(px(8.)).bottom(px(8.)).w(px(2.))
        })
        .bg(if dark {
            rgba(0xffffff30)
        } else {
            rgba(0x1a1a2433)
        })
}

/// A press on a Dock tile. Only the pinned applications may be rearranged, so
/// the running and recent sections never move.
#[derive(Clone)]
pub(super) struct DockPress {
    /// The application below the pointer.
    pub id: String,
    /// Whether the application may be rearranged.
    pub pinned: bool,
    /// The pointer position along the row when the press started.
    pub pointer: f32,
    /// The pointer position across the row, used by the overflow drawer grid.
    pub pointer_y: f32,
}

/// A pinned application dragged along the Dock row.
#[derive(Clone)]
pub(super) struct DockDrag {
    /// The dragged application.
    pub id: String,
    /// The distance between the pointer and the icon's leading edge.
    pub grab: f32,
    /// The pointer position along the row.
    pub pointer: f32,
    /// The pinned group in its new order.
    pub order: Vec<String>,
    /// The slot the dragged application would land in.
    pub insert: usize,
}

#[derive(Clone)]
pub(super) struct DockFolderPress {
    pub path: String,
    pub pointer: f32,
}

#[derive(Clone)]
pub(super) struct DockFolderDrag {
    pub path: String,
    pub grab: f32,
    pub pointer: f32,
    pub order: Vec<String>,
    pub insert: usize,
}

/// The row geometry the reorder drag works on.
#[derive(Clone, Copy, Debug)]
pub(super) struct DockGeometry {
    /// The leading edge of the first application slot.
    pub base: f32,
    /// The leading edge of the first folder slot.
    pub folder_base: f32,
    /// The distance between two slots.
    pub item: f32,
    /// The icon size.
    pub icon: f32,
    /// The cross-axis span of the tray as last painted.
    pub tray_cross_start: f32,
    pub tray_cross_end: f32,
    /// The trailing edge (right or bottom) of the last application slot,
    /// where the folder strip begins.
    pub apps_end: f32,
    /// The leading edge of the trash tile on the main axis.
    pub trash_start: f32,
}

/// Local to the Dock view: pointer packets never rebuild the top bar, catalog,
/// or other outputs. The next render consumes the most recent packet once.
pub(super) struct DockViewMotion {
    pub(super) epoch: std::time::Instant,
    pub(super) hovered: bool,
    pub(super) pointer_target: Option<f32>,
    /// The geometry of the last rendered row.
    pub(super) geometry: DockGeometry,
    /// The pending press, if the pointer is still down on a tile.
    pub(super) press: Option<DockPress>,
    /// The pinned application being dragged.
    pub(super) drag: Option<DockDrag>,
    /// The pending press on a folder tile.
    pub(super) folder_press: Option<DockFolderPress>,
    /// The pinned folder being dragged.
    pub(super) folder_drag: Option<DockFolderDrag>,
    /// Per-folder main-axis positions while a folder reorder slides the
    /// untouched tiles towards their new slots.
    pub(super) folder_reorder: std::collections::HashMap<String, crate::dock_motion::Motion>,
    /// The application name shown in the hover tooltip, if the pointer rests on
    /// a Dock tile.
    pub(super) tooltip_name: Option<String>,
    /// The tile the pointer just entered and the elapsed instant it entered,
    /// waiting out `TOOLTIP_DELAY` before the label appears.
    pub(super) tooltip_armed: Option<(String, std::time::Duration)>,
    /// The app tile that would accept the file currently being dragged.
    pub(super) drag_accept: Option<String>,
    /// A folder from outside the Dock is hovering the tray and the Dock is
    /// holding open an extra folder slot for it.
    pub(super) folder_drop_active: bool,    /// The reserved slot should show the folder placeholder. It stays true
    /// while the drag remains over the Dock, including over apps or the
    /// shifted trash, until the drag leaves or ends.
    pub(super) folder_drop_visible: bool,
    /// Fade and scale of the folder pin placeholder.
    pub(super) folder_drop: crate::dock_motion::Motion,
    /// Grow-in slides for freshly pinned folders, keyed by folder path.
    /// Finished slides retire here so the map never outlives the animation.
    pub(super) folder_appear: std::collections::HashMap<String, crate::dock_motion::Motion>,
    /// Unpinned folders riding out a fade while a reserved slot still holds
    /// their place, in pin order; the row settles instead of the icon
    /// popping away and the rest re-flowing instantly.
    pub(super) folder_exiting: Vec<(crate::app::DockFolderView, crate::dock_motion::Motion)>,
    /// The pinned folder list as the previous render saw it. The per-render
    /// pin diff compares against this to key the appear and exit slides.
    pub(super) folders_snapshot: Vec<crate::app::DockFolderView>,
    /// Whether the pin diff has completed once. The very first render only
    /// seeds the snapshot: without this gate a shell restart would replay a
    /// batch of false arrivals for folders that were already pinned.
    pub(super) folders_seeded: bool,
    /// Resting slot layout fingerprints. The idle geometry (used for the
    /// strip bounds and drawer mapping) only changes with the metrics; every
    /// magnification frame reuses the cached sample instead of re-running the
    /// layout walkers.
    pub(super) idle_fingerprint: Option<crate::dock_motion::DockMetrics>,
    pub(super) idle_slots: std::rc::Rc<Vec<crate::dock_motion::Slot>>,
    /// Wall-clock anchor for the slow hover pulse that keeps frames coming
    /// while the Dock believes the pointer rests on it, so the platform-based
    /// recovery below can fire even without further motion.
    hover_pulse: std::time::Instant,
    /// Main-axis centers and hit reach of the app tiles as last painted. File
    /// drags resolve the tile under the pointer against this row instead of
    /// mapping hover from another surface.
    pub(super) drag_targets: Vec<(String, f32, f32)>,
    /// Per-application main-axis positions while a reorder drag slides the
    /// untouched icons towards their new slots.
    pub(super) reorder: std::collections::HashMap<String, crate::dock_motion::Motion>,
    pointer: crate::dock_motion::Motion,
    strength: crate::dock_motion::Motion,
    visibility: crate::dock_motion::Motion,
    #[cfg(test)]
    pub(super) input_regions: Vec<Bounds<Pixels>>,
    #[cfg(test)]
    pub(super) glass_regions: Vec<Bounds<Pixels>>,
    #[cfg(test)]
    pub(super) animation_active: bool,
}
impl DockViewMotion {
    /// The single place hover magnification is set up: the row becomes
    /// hovered and follows `target`. Callers under an active hover (the
    /// drag paths) pass the fresh position so the magnification center
    /// tracks it; a plain session re-entering the surface gets the same snap
    /// to rest semantics from `point_at`.
    pub(super) fn enter_hover(&mut self, target: f32) -> bool {
        let mut changed = false;
        if !self.hovered {
            self.hovered = true;
            changed = true;
        }
        if self.point_at(target) {
            changed = true;
        }
        changed
    }

    /// Follow the pointer to a new magnification target. Both element hover
    /// moves and the drag paths drive this, so undefined transitions must not
    /// re-snap from a resting pointer.
    pub(super) fn point_at(&mut self, target: f32) -> bool {
        if self.pointer_target != Some(target) {
            if self.pointer_target.is_none() {
                self.pointer.snap(target);
            }
            self.pointer_target = Some(target);
            return true;
        }
        false
    }

    /// The single place hover magnification is torn down. Every exit path —
    /// the element exit event, the drag-end cleanup, the platform watchdog —
    /// funnels through this so the invariants ("nothing magnifies while the
    /// pointer is not over the row", "no strip offer survives a leave") hold
    /// no matter which listener ran last. Returns whether anything changed.
    ///
    /// Ordering fallout from Wayland data-device grabs means `hovered` can
    /// already be false while a plain move re-established a magnification
    /// target; callers must therefore invoke this on `Leave` unconditionally
    /// instead of comparing against the current hover state.
    pub(super) fn clear_hover(&mut self) -> bool {
        let mut changed = false;
        if self.hovered {
            self.hovered = false;
            changed = true;
        }
        if self.pointer_target.take().is_some() {
            changed = true;
        }
        if self.tooltip_name.take().is_some() {
            changed = true;
        }
        if self.tooltip_armed.take().is_some() {
            changed = true;
        }
        if self.drag_accept.take().is_some() {
            changed = true;
        }
        if std::mem::take(&mut self.folder_drop_active) {
            changed = true;
        }
        if std::mem::take(&mut self.folder_drop_visible) {
            changed = true;
        }
        changed
    }

    pub(super) fn new(revealed: bool) -> Self {
        Self {
            #[cfg(test)]
            input_regions: Vec::new(),
            #[cfg(test)]
            glass_regions: Vec::new(),
            #[cfg(test)]
            animation_active: false,
            epoch: std::time::Instant::now(),
            hovered: false,
            pointer_target: None,
            geometry: DockGeometry {
                base: 0.,
                folder_base: 0.,
                apps_end: 0.,
                item: 60.,
                icon: 48.,
                tray_cross_start: 0.,
                tray_cross_end: 0.,
                trash_start: 0.,
            },
            press: None,
            drag: None,
            folder_press: None,
            folder_drag: None,
            folder_reorder: std::collections::HashMap::new(),
            tooltip_name: None,
            tooltip_armed: None,
            drag_accept: None,
            folder_drop_active: false,
            folder_drop_visible: false,
            folder_drop: crate::dock_motion::Motion::new(0.),
            folder_appear: std::collections::HashMap::new(),
            folder_exiting: Vec::new(),
            folders_snapshot: Vec::new(),
            folders_seeded: false,
            idle_fingerprint: None,
            idle_slots: std::rc::Rc::new(Vec::new()),
            hover_pulse: std::time::Instant::now(),
            drag_targets: Vec::new(),
            reorder: std::collections::HashMap::new(),
            pointer: crate::dock_motion::Motion::with_frequency(0., 40.),
            strength: crate::dock_motion::Motion::new(0.),
            visibility: crate::dock_motion::Motion::new(if revealed { 1. } else { 0. }),
        }
    }
    /// Record a press on a tile. Only the pinned applications may be dragged.
    pub(super) fn press(&mut self, id: String, pinned: bool, pointer: f32) {
        self.press = Some(DockPress {
            id,
            pinned,
            pointer,
            pointer_y: 0.,
        });
    }
    /// Turn a press into a reorder drag once the pointer leaves the tile.
    pub(super) fn begin_drag(&mut self, pointer: f32, pinned: &[DockApp]) -> bool {
        let Some(press) = self.press.clone() else {
            return false;
        };
        if !press.pinned {
            return false;
        }
        if (pointer - press.pointer).abs() < crate::dock_motion::DRAG_THRESHOLD {
            return false;
        }
        let Some(index) = pinned.iter().position(|app| app.id == press.id) else {
            return false;
        };
        let geometry = self.geometry;
        self.drag = Some(DockDrag {
            id: press.id.clone(),
            grab: press.pointer - (geometry.base + index as f32 * geometry.item),
            pointer,
            order: pinned.iter().map(|app| app.id.clone()).collect(),
            insert: index,
        });
        self.press = None;
        // The magnification would fight the dragged icon.
        self.strength.snap(0.);
        // A lifted icon no longer shows the resting tooltip.
        self.tooltip_name = None;
        self.tooltip_armed = None;
        true
    }
    /// Enter or leave a Dock tile. Entering arms the delayed label; leaving the
    /// tile clears it, so the name only appears after the pointer settles.
    pub(super) fn hover_tooltip(&mut self, name: Option<&str>, now: std::time::Duration) {
        match name {
            Some(name) => {
                if self.tooltip_name.as_deref() == Some(name) {
                    // Already shown for this tile; keep it.
                    return;
                }
                match &self.tooltip_armed {
                    Some((armed, _)) if armed == name => {}
                    _ => self.tooltip_armed = Some((name.to_owned(), now)),
                }
            }
            None => {
                self.tooltip_name = None;
                self.tooltip_armed = None;
            }
        }
    }
    /// Promote an armed label once its delay elapses. Returns true while the
    /// view must keep requesting frames so the label still has a chance to show.
    pub(super) fn tick_tooltip(&mut self, now: std::time::Duration) -> bool {
        if self.tooltip_name.is_some() {
            return false;
        }
        let Some((name, armed)) = self.tooltip_armed.clone() else {
            return false;
        };
        if now.saturating_sub(armed) >= TOOLTIP_DELAY {
            self.tooltip_name = Some(name);
            self.tooltip_armed = None;
            false
        } else {
            true
        }
    }
    /// Move the dragged application towards the slot under the pointer.
    pub(super) fn track_drag(&mut self, pointer: f32) {
        let Some(mut drag) = self.drag.take() else {
            return;
        };
        let geometry = self.geometry;
        let remaining = drag
            .order
            .iter()
            .enumerate()
            .filter(|(_, id)| **id != drag.id)
            .map(|(index, id)| (index, id.clone()))
            .collect::<Vec<_>>();
        let mut insert = 0usize;
        for (index, _) in &remaining {
            // The dragged application leaves a hole at its own slot.
            let slot = if *index < drag.insert { *index } else { *index + 1 };
            let center = geometry.base + slot as f32 * geometry.item + geometry.icon / 2.;
            if center < pointer {
                insert = *index + 1;
            } else {
                break;
            }
        }
        insert = insert.min(remaining.len());
        let mut order: Vec<String> = remaining.into_iter().map(|(_, id)| id).collect();
        order.insert(insert, drag.id.clone());
        drag.insert = insert;
        drag.order = order;
        drag.pointer = pointer;
        self.drag = Some(drag);
    }
    /// Finish the drag and report the new order of the pinned group.
    pub(super) fn end_drag(&mut self) -> Option<Vec<String>> {
        self.press = None;
        self.drag.take().map(|drag| drag.order)
    }
    /// Abandon the drag without changing the order.
    pub(super) fn abort_drag(&mut self) {
        self.press = None;
        self.drag = None;
        self.tooltip_name = None;
    }

    /// Record a press on a folder tile.
    pub(super) fn press_folder(&mut self, path: String, pointer: f32) {
        self.folder_press = Some(DockFolderPress { path, pointer });
    }

    /// Turn a folder press into a reorder drag once the pointer leaves the tile.
    pub(super) fn begin_folder_drag(
        &mut self,
        pointer: f32,
        folders: &[crate::app::DockFolderView],
    ) -> bool {
        let Some(press) = self.folder_press.clone() else {
            return false;
        };
        if (pointer - press.pointer).abs() < crate::dock_motion::DRAG_THRESHOLD {
            return false;
        }
        let Some(index) = folders
            .iter()
            .position(|folder| folder.path.to_string_lossy() == press.path)
        else {
            return false;
        };
        let geometry = self.geometry;
        self.folder_drag = Some(DockFolderDrag {
            path: press.path,
            grab: press.pointer - (geometry.folder_base + index as f32 * geometry.item),
            pointer,
            order: folders
                .iter()
                .map(|folder| folder.path.to_string_lossy().into_owned())
                .collect(),
            insert: index,
        });
        self.folder_press = None;
        self.strength.snap(0.);
        true
    }

    /// Move the dragged folder towards the folder slot under the pointer.
    pub(super) fn track_folder_drag(&mut self, pointer: f32) {
        let Some(mut drag) = self.folder_drag.take() else {
            return;
        };
        let geometry = self.geometry;
        let leading = pointer - drag.grab;
        let raw = ((leading - geometry.folder_base) / geometry.item).round();
        let insert = raw.clamp(0., (drag.order.len().saturating_sub(1)) as f32) as usize;
        let mut others = drag
            .order
            .iter()
            .filter(|path| **path != drag.path)
            .cloned()
            .collect::<Vec<_>>();
        let insert = insert.min(others.len());
        others.insert(insert, drag.path.clone());
        drag.insert = insert;
        drag.order = others;
        drag.pointer = pointer;
        self.folder_drag = Some(drag);
    }

    /// Finish the folder drag and report the new folder order.
    pub(super) fn end_folder_drag(&mut self) -> Option<Vec<String>> {
        self.folder_press = None;
        self.folder_drag.take().map(|drag| drag.order)
    }

    /// Abandon a folder drag without changing the order.
    pub(super) fn abort_folder_drag(&mut self) {
        self.folder_press = None;
        self.folder_drag = None;
    }
}

/// The overflow drawer shows the applications that did not fit the row in a
/// three-column grid. The container keeps at least `DRAWER_MIN_ROWS` rows so the
/// popup has a stable base shape; a larger overflow adds rows rather than
/// widening the grid.
pub(crate) const DRAWER_COLUMNS: usize = 3;
/// The container keeps at least this many rows. More applications add rows, so
/// the drawer is a fixed three-column grid that grows downward.
pub(crate) const DRAWER_MIN_ROWS: usize = 3;
/// Space between two drawer cells, and the padding between the grid and the
/// container edge. The gap keeps the icons from crowding; the padding keeps the
/// grid off the rounded border so the corner radius reads cleanly.
pub(crate) const DRAWER_GAP: f32 = 10.;
pub(crate) const DRAWER_PAD: f32 = 16.;
/// The container's outer corner radius. It follows the icon so a small Dock and
/// a large Dock round together.
pub(crate) fn drawer_radius(icon: f32) -> f32 {
    (icon * 0.5).clamp(16., 26.)
}
/// The number of grid rows for `count` applications: at least three, more when
/// the overflow does not fit the base shape.
pub(crate) fn drawer_rows(count: usize) -> usize {
    count.div_ceil(DRAWER_COLUMNS).max(DRAWER_MIN_ROWS)
}
/// The drawer container extent for the given icon size and application count,
/// as (width, height). The width is always three columns; the height grows a
/// row at a time.
pub(crate) fn drawer_size(icon: f32, count: usize) -> (f32, f32) {
    let width =
        DRAWER_COLUMNS as f32 * icon + (DRAWER_COLUMNS as f32 - 1.) * DRAWER_GAP + 2. * DRAWER_PAD;
    let rows = drawer_rows(count) as f32;
    let height = rows * icon + (rows - 1.) * DRAWER_GAP + 2. * DRAWER_PAD;
    (width, height)
}

/// Grid geometry for the drawer. `cell` is the distance between two cell
/// origins; `x`/`y` are the container-local origin of the grid.
#[derive(Clone, Copy, Debug)]
pub(super) struct DrawerGeometry {
    pub x: f32,
    pub y: f32,
    pub cell: f32,
    /// The current number of grid rows, at least `DRAWER_MIN_ROWS`.
    pub rows: usize,
}

impl DrawerGeometry {
    /// The container-local origin of the cell at `index` in the drawer order.
    pub(super) fn cell_origin(&self, index: usize) -> (f32, f32) {
        let column = (index % DRAWER_COLUMNS) as f32;
        let row = (index / DRAWER_COLUMNS) as f32;
        (self.x + column * self.cell, self.y + row * self.cell)
    }
    /// The cell whose center is nearest the pointer, or `None` outside the grid.
    pub(super) fn nearest(&self, x: f32, y: f32) -> Option<usize> {
        let column = ((x - self.x) / self.cell).round();
        let row = ((y - self.y) / self.cell).round();
        if column < 0. || row < 0. || column >= DRAWER_COLUMNS as f32 || row >= self.rows as f32 {
            return None;
        }
        Some((row as usize) * DRAWER_COLUMNS + column as usize)
    }
}

/// A reorder drag inside the overflow drawer. It reorders the same pinned group
/// the Dock row does, but on a 3x3 grid rather than a single row.
#[derive(Clone)]
pub(super) struct DrawerDrag {
    pub id: String,
    /// The drawer applications in their live order.
    pub order: Vec<String>,
    /// The pointer position in container-local coordinates.
    pub pointer: gpui::Point<Pixels>,
    /// The offset from the pointer to the dragged cell's leading corner.
    pub grab: (f32, f32),
    /// The grid index the dragged application would land in.
    pub insert: usize,
}

/// Local to the overflow drawer: presses, the reorder drag, and the per-cell
/// animation that slides the untouched icons towards their new cells.
pub(super) struct DockDrawerMotion {
    pub(super) press: Option<DockPress>,
    pub(super) drag: Option<DrawerDrag>,
    /// The geometry of the last rendered grid.
    pub(super) geometry: DrawerGeometry,
    /// The clock the per-cell motions sample against.
    pub(super) epoch: std::time::Instant,
    /// Per-application container-local cell origins while a reorder slides the
    /// untouched icons. Two scalars per card: X and Y.
    pub(super) reorder_x: std::collections::HashMap<String, crate::dock_motion::Motion>,
    pub(super) reorder_y: std::collections::HashMap<String, crate::dock_motion::Motion>,
    /// Whether the pointer hovered a tile on the last rendered frame.
    pub(super) hovered: bool,
}

impl DockDrawerMotion {
    pub(super) fn new() -> Self {
        Self {
            press: None,
            drag: None,
            geometry: DrawerGeometry {
                x: 0.,
                y: 0.,
                cell: 64.,
                rows: DRAWER_MIN_ROWS,
            },
            epoch: std::time::Instant::now(),
            reorder_x: std::collections::HashMap::new(),
            reorder_y: std::collections::HashMap::new(),
            hovered: false,
        }
    }
    /// Record a press on a drawer tile. Only pinned applications reorder, the
    /// same rule the Dock row follows.
    pub(super) fn press(&mut self, id: String, pinned: bool, x: f32, y: f32) {
        self.press = Some(DockPress {
            id,
            pinned,
            pointer: x,
            pointer_y: y,
        });
    }
    /// Turn a press into a reorder drag once the pointer leaves the tile.
    pub(super) fn begin_drag(&mut self, x: f32, y: f32, order: &[String]) -> bool {
        let Some(press) = self.press.clone() else {
            return false;
        };
        if !press.pinned {
            return false;
        }
        let dx = x - press.pointer;
        let dy = y - press.pointer_y;
        if dx.abs() < crate::dock_motion::DRAG_THRESHOLD
            && dy.abs() < crate::dock_motion::DRAG_THRESHOLD
        {
            return false;
        }
        let Some(index) = order.iter().position(|id| *id == press.id) else {
            return false;
        };
        let geometry = self.geometry;
        let (origin_x, origin_y) = geometry.cell_origin(index);
        self.drag = Some(DrawerDrag {
            id: press.id.clone(),
            order: order.to_vec(),
            pointer: gpui::point(px(x), px(y)),
            grab: (press.pointer - origin_x, press.pointer_y - origin_y),
            insert: index,
        });
        self.press = None;
        true
    }
    /// Move the dragged application towards the cell under the pointer.
    pub(super) fn track_drag(&mut self, x: f32, y: f32) {
        let Some(mut drag) = self.drag.take() else {
            return;
        };
        drag.pointer = gpui::point(px(x), px(y));
        // The target cell follows the pointer's leading corner, so the icon
        // stays under the finger rather than snapping its center.
        let corner_x = x - drag.grab.0;
        let corner_y = y - drag.grab.1;
        if let Some(target) = self.geometry.nearest(corner_x, corner_y) {
            // `others` is the drawer order without the dragged icon; the insert
            // index is into that shortened list.
            let others = drag.order.iter().filter(|id| **id != drag.id).count();
            let insert = target.min(others);
            let mut order: Vec<String> = drag
                .order
                .iter()
                .filter(|id| **id != drag.id)
                .cloned()
                .collect();
            order.insert(insert, drag.id.clone());
            drag.insert = insert;
            drag.order = order;
        }
        self.drag = Some(drag);
    }
    /// Finish the drag and report the new drawer order.
    pub(super) fn end_drag(&mut self) -> Option<Vec<String>> {
        self.press = None;
        self.drag.take().map(|drag| drag.order)
    }
    /// Abandon the drag without changing the order.
    pub(super) fn abort_drag(&mut self) {
        self.press = None;
        self.drag = None;
    }
}

/// Per-frame Dock geometry and motion state, passed from the layout step to
/// the paint step. Splitting the former 1400-line render into these phases
/// keeps the motion math (side effects on `DockViewMotion`) and the element
/// tree construction readable in isolation.
pub(super) struct DockFrame {
    pub(crate) metrics: crate::dock_motion::DockMetrics,
    pub(crate) slots: std::rc::Rc<Vec<crate::dock_motion::Slot>>,
    pub(crate) pointer: crate::dock_motion::MotionSample,
    pub(crate) folder_drop: crate::dock_motion::MotionSample,
    pub(crate) folder_drop_lift: f32,
    pub(crate) tray_x: f32,
    pub(crate) tray_y: f32,
    pub(crate) tray_w: f32,
    pub(crate) tray_h: f32,
    pub(crate) vertical: bool,
    pub(crate) left: bool,
    pub(crate) dark: bool,
    pub(crate) slide: f32,
    pub(crate) launcher_offset: usize,
    pub(crate) now: std::time::Duration,
    pub(crate) active: bool,
    /// The resting tray outline before quantization: the magnification grid
    /// adds icon bands on top, and the frosted shape must stay the tray alone.
    pub(crate) tray_regions: Vec<Bounds<Pixels>>,
}

impl ShellView {
    /// Main-axis span of the folder pin strip: a third of the gap after the
    /// application group's trailing edge, up to the trash. Single source of
    /// truth — drag detection and the tray's `can_drop` both read this.
    fn folder_strip_main_range(&self) -> (f32, f32) {
        let geometry = &self.dock_motion.geometry;
        let start = geometry.apps_end
            + ((geometry.folder_base - geometry.apps_end) / 3.).max(0.);
        (start, geometry.trash_start)
    }

    /// The Dock app under `position` that would accept `paths`, if any.
    fn drag_accept_target(
        &self,
        position: Point<Pixels>,
        paths: &[PathBuf],
        window: &Window,
    ) -> Option<String> {
        let viewport = window.viewport_size();
        if position.x < px(0.)
            || position.y < px(0.)
            || position.x > viewport.width
            || position.y > viewport.height
        {
            return None;
        }
        let vertical = self.state.prefs.dock_position != DockPosition::Bottom;
        let main = f32::from(if vertical {
            position.y
        } else {
            position.x
        });
        let id = self
            .dock_motion
            .drag_targets
            .iter()
            .find(|(_, center, reach)| (main - *center).abs() <= *reach)
            .map(|(id, _, _)| id.clone())?;
        let app = self.state.catalog.get(&id)?;
        crate::mime::app_accepts_paths(app, paths).then_some(id)
    }

    /// Builds the per-frame geometry and advances all Dock motion.
    /// Lifecycle invariants live here; see the UPSTREAM(gpui) note below.
    fn dock_frame(
        &mut self,
        host: &ViewState,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> DockFrame {
        use crate::dock_motion::DockMetrics;
        let vertical = host.prefs.dock_position != DockPosition::Bottom;
        let left = host.prefs.dock_position == DockPosition::Left;
        let dark = host.dark();
        let logical = host
            .desktop
            .outputs
            .get(&self.output)
            .and_then(|o| o.logical);
        let (width, height) = logical
            .map(|o| (o.width as f32, o.height as f32))
            .unwrap_or_else(|| {
                let bounds = window
                    .display(cx)
                    .map(|d| d.bounds().size)
                    .unwrap_or(size(px(1200.), px(800.)));
                (f32::from(bounds.width), f32::from(bounds.height))
            });
        let apps = host.dock_apps();
        // Folder pin diff — every render compares the pinned list against
        // what the previous render saw. A fresh pin keys a grow-in slide; a
        // removal moves its folder into an exit ride so the slot lingers
        // while the tile fades. Slot counts below must include the exit
        // riders or the strip would re-flow one frame before the fade ends.
        {
            let motion = &mut self.dock_motion;
            let now = motion.epoch.elapsed();
            let paths: Vec<String> = host
                .dock_folders
                .iter()
                .map(|folder| folder.path.to_string_lossy().into_owned())
                .collect();
            let snapshot = std::mem::take(&mut motion.folders_snapshot);
            if motion.folders_seeded {
                // Removals seed their exit slide from wherever the arrival
                // is right now, so a fast pin→unpin goes straight into the
                // fade instead of restarting from full size.
                for folder in snapshot.iter() {
                    let path = folder.path.to_string_lossy().into_owned();
                    if paths.contains(&path) {
                        continue;
                    }
                    let mut exit = motion
                        .folder_appear
                        .remove(&path)
                        .unwrap_or_else(|| crate::dock_motion::Motion::new(1.));
                    if host.prefs.reduce_motion {
                        exit.snap(0.);
                    } else {
                        exit.retarget(0., now);
                    }
                    motion.folder_exiting.push((folder.clone(), exit));
                }
                // A path pinned again while its exit still owns a slot
                // returns to a real tile at once; the fade must not fight
                // the arrival on the shared slot row.
                motion.folder_exiting.retain(|(folder, _)| {
                    let path = folder.path.to_string_lossy().into_owned();
                    !paths.contains(&path)
                });
                // Retire slides that have settled so a later pin of the same
                // path re-plays the animation from zero.
                motion.folder_appear.retain(|_, slide| {
                    let sample = slide.sample(now);
                    sample.active || sample.value < 0.999
                });
                motion.folder_exiting.retain(|(_, slide)| {
                    let sample = slide.sample(now);
                    sample.active || sample.value > 0.001
                });
                // Arrivals grow and fade in from the placeholder shape.
                for folder in host.dock_folders.iter() {
                    if snapshot.iter().any(|old| old.path == folder.path) {
                        continue;
                    }
                    let entry = motion
                        .folder_appear
                        .entry(folder.path.to_string_lossy().into_owned())
                        .or_insert_with(|| crate::dock_motion::Motion::new(0.));
                    if host.prefs.reduce_motion {
                        entry.snap(1.);
                    } else {
                        entry.retarget(1., now);
                    }
                }
            }
            motion.folders_seeded = true;
            motion.folders_snapshot = host.dock_folders.clone();
        }
        let mut metrics = DockMetrics::with_folders(
            apps.len(),
            host.prefs.dock_size as f32,
            host.prefs.magnification_scale as f32,
            host.prefs.magnification && !host.prefs.reduce_motion,
            host.prefs.show_launcher,
            host.dock_folders.len()
                + usize::from(host.folder_drop_preview)
                + self.dock_motion.folder_exiting.len(),
            vertical,
            width,
            height,
        );
        // The overflow tile folds only pinned applications, so the drawer never
        // hides a recent or running app behind the pinned group.
        let (pinned, running, recent) = crate::app::dock_app_counts(&apps);
        metrics.restrict_overflow(pinned, running, recent);
        let now = self.dock_motion.epoch.elapsed();
        if !cx.has_active_drag() {
            if self.dock_drag_seen {
                // An external drag ends in FileDropEvent::Exited, which carries
                // no position and therefore never dispatches the final "outside"
                // mouse move. Clear the hover explicitly or the last icon stays
                // magnified after the pointer has left the Dock.
                self.dock_motion.clear_hover();
            }
            self.dock_drag_seen = false;
            // The reservation is host-wide, so only release it when no drag is
            // active at all. An inactive Dock on another output must never
            // release the slot that the Dock under the pointer is using.
            if host.folder_drop_preview {
                let h = self.host.clone();
                h.update(cx, |h, cx| h.handle(Action::FolderDropPreview(false), cx));
            }
        }
        let folder_drop_target = if self.dock_motion.folder_drop_active
            && self.dock_motion.folder_drop_visible
        {
            1.
        } else {
            0.
        };
        if host.prefs.reduce_motion {
            self.dock_motion.folder_drop.snap(folder_drop_target);
        } else {
            self.dock_motion.folder_drop.retarget(folder_drop_target, now);
        }
        let folder_drop = self.dock_motion.folder_drop.sample(now);
        // UPSTREAM(gpui): this watchdog papers over two upstream gaps that we
        // cannot fix from inside this view. The principled fixes live in
        // third_party/gpui and would let both this block and the 250ms
        // heartbeat below be deleted:
        //
        // 1. Active drag teardown does not re-run the hit-test stack: when
        //    `cx.active_drag` is cleared at the end of
        //    `Window::dispatch_mouse_event`, the row that hovered before the
        //    drag never receives a final MouseExitEvent, and plain moves can
        //    re-establish a magnification target against a `hovered` that the
        //    drop cleanup had already reset (the exact sequence behind the
        //    "pinned folder keeps a magnified icon" bug).
        // 2. Hovered changes are invisible to observers: the platform clients
        //    call `window.set_hovered` on `wl_pointer` enter/leave, but
        //    nothing surfaces that transition, so the view can only poll
        //    `window.is_window_hovered()` on a render it gets for free —
        //    hence the heartbeat keeps frames coming while the row believes
        //    the pointer rests on it.
        // Upstreaming either one unlocks a lifecycle in which this
        // poll-and-recover shim disappears entirely; `clear_hover` and the
        // reducer already keep every other edge strict.
        //
        // On Wayland the pointer leave can never reach the element hover
        // listeners in one specific case: the pin resizes and repositions the
        // Dock surface right under the cursor, and the surface reconfigure
        // swallows the leave-or-enter pair that would tell the row about it.
        // The platform window knows whether it still owns the cursor, so any
        // render where the Dock still believes the pointer is resting while
        // the window has lost it reconciles the hover state. A slow pulse
        // keeps frames coming while the row believes it is hovered so this
        // recovery runs even if no motion arrives afterwards.
        if cfg!(test) {
            // Tests control hover strictly through simulated events.
        } else {
            // A live data-device drag takes the pointer focus away from this
            // surface (`wl_pointer.leave` while the drag grab runs), which
            // would look exactly like the lost-pointer state. The drag path
            // drives the hover state itself below, so reconciliation must not
            // fight it while a drag is active.
            if self.dock_motion.hovered
                && !cx.has_active_drag()
                && !window.is_window_hovered()
            {
                self.dock_motion.clear_hover();
                self.dock_drag_seen = false;
                let h = self.host.clone();
                cx.defer(move |cx| h.update(cx, |h, cx| h.handle(Action::DockHover(false), cx)));
                cx.notify();
            }
            let wall = std::time::Instant::now();
            if self.dock_motion.hovered
                && wall.duration_since(self.dock_motion.hover_pulse)
                    >= std::time::Duration::from_millis(250)
            {
                self.dock_motion.hover_pulse = wall;
                window.request_animation_frame();
            }
        }
        if cx.has_active_drag() && self.dock_drag_seen {
            // The Dock window saw the drag itself, or a folder surface
            // forwarded the pointer events from its invisible drag layer.
            let viewport = window.viewport_size();
            let mouse = window.mouse_position();
            let inside = mouse.x >= px(0.)
                && mouse.y >= px(0.)
                && mouse.x <= viewport.width
                && mouse.y <= viewport.height;
            let target = f32::from(if vertical { mouse.y } else { mouse.x });
            if inside {
                // A drag over the row behaves like a hover: magnify under the
                // file. Re-establishing `hovered` here covers surfaces whose
                // pointer events arrived before the drag was recognized.
                self.dock_motion.enter_hover(target);
            } else {
                self.dock_motion.clear_hover();
            }
        }
        let reveal = if host.dock_revealed { 1. } else { 0. };
        // An external file drag delivers pointer moves without a normal hover
        // enter event. The pointer target is the authoritative signal that an
        // icon is being pointed at, whether by the mouse or by a dragged file.
        let strength = if self.dock_motion.pointer_target.is_some()
            && host.prefs.magnification
            && host.dock_revealed
            && !host.prefs.reduce_motion
            && self.dock_motion.drag.is_none()
        {
            1.
        } else {
            0.
        };
        if host.prefs.reduce_motion {
            self.dock_motion.strength.snap(0.);
            self.dock_motion.visibility.snap(reveal);
        } else {
            self.dock_motion.strength.retarget(strength, now);
            self.dock_motion.visibility.retarget(reveal, now);
        }
        if let Some(pointer) = self.dock_motion.pointer_target {
            if self.dock_motion.pointer.target() != pointer {
                self.dock_motion.pointer.retarget(pointer, now);
            }
        }
        let strength = self.dock_motion.strength.sample(now);
        let visibility = self.dock_motion.visibility.sample(now);
        let pointer = self.dock_motion.pointer.sample(now);
        let slots = metrics.samples(Some(pointer.value), strength.value);
        let total_growth: f32 = slots.iter().map(|s| s.lift).sum();
        let tray_main = metrics.main_padding - total_growth / 2.;
        let tray_length = metrics.length + total_growth;
        let slide = (1. - visibility.value.clamp(0., 1.))
            * (metrics.thickness + metrics.cross_padding + 8.);
        let (tray_x, tray_y, tray_w, tray_h) = if !vertical {
            (
                tray_main,
                metrics.cross_padding + slide,
                tray_length,
                metrics.thickness,
            )
        } else if left {
            (-slide, tray_main, metrics.thickness, tray_length)
        } else {
            (
                metrics.cross_padding + slide,
                tray_main,
                metrics.thickness,
                tray_length,
            )
        };
        let active = visibility.active
            || strength.active
            || folder_drop.active
            || (strength.value > 0.001 && pointer.active);        let mut regions = rounded_region(tray_w, tray_h, metrics.radius);
        for region in &mut regions {
            region.origin.x += px(tray_x);
            region.origin.y += px(tray_y);
        }
        // The frosted shape is the resting tray alone. Lifted, magnified icons
        // are still clickable but must not smear the backdrop beside the tray.
        let folder_drop_lift = metrics.growth + metrics.bounce_amplitude;
        DockFrame {
            metrics,
            slots: std::rc::Rc::new(slots),
            pointer,
            folder_drop,
            folder_drop_lift,
            tray_x,
            tray_y,
            tray_w,
            tray_h,
            vertical,
            left,
            dark,
            slide,
            launcher_offset: usize::from(host.prefs.show_launcher),
            now,
            active,
            tray_regions: regions,
        }
    }

    /// Paints the tray tiles: the in-flight arrangement, the per-tile
    /// elements and the reorder slides. Pure paint over `DockFrame` — the
    /// only state writes are the derived arrangement maps; activity
    /// discovered while painting (launch bounce) is returned so the caller
    /// can schedule the next frame.
    fn dock_slots(
        &mut self,
        host: &ViewState,
        frame: &DockFrame,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> (AnyElement, Vec<Bounds<Pixels>>, bool) {
        use crate::dock_motion::launch_bounce;
        let apps = host.dock_apps();
        let metrics = frame.metrics.clone();
        let slots = frame.slots.clone();
        let folder_drop = frame.folder_drop;
        let tray_x = frame.tray_x;
        let tray_y = frame.tray_y;
        let tray_w = frame.tray_w;
        let tray_h = frame.tray_h;
        let vertical = frame.vertical;
        let left = frame.left;
        let dark = frame.dark;
        let slide = frame.slide;
        let launcher_offset = frame.launcher_offset;        let now = frame.now;
        let mut extra_active = false;
        let mut content = div().absolute().inset_0();
        // The pinned applications that did not fit the row wait in the overflow
        // tile. Running and recent apps are never folded, so the list skips the
        // visible pinned prefix and stops before the non-pinned tail.
        let hidden: Vec<String> = apps
            .iter()
            .filter(|app| app.pinned)
            .skip(metrics.visible_pinned)
            .take(metrics.hidden_apps)
            .map(|app| app.id.clone())
            .collect();
        // The in-flight arrangement replaces the pinned group only.
        let mut apps = apps;
        if let Some(drag) = &self.dock_motion.drag {
            let mut pinned: Vec<DockApp> = Vec::new();
            for id in &drag.order {
                if let Some(app) = apps.iter().find(|app| &app.id == id) {
                    pinned.push(app.clone());
                }
            }
            for app in apps.iter().filter(|app| app.pinned) {
                if !pinned.iter().any(|pinned| pinned.id == app.id) {
                    pinned.push(app.clone());
                }
            }
            let rest: Vec<DockApp> = apps.iter().filter(|app| !app.pinned).cloned().collect();
            apps = pinned;
            apps.extend(rest);
        }
        // The painted row is the visible pinned prefix, then running
        // applications, then recent applications. The folded pinned tail stays
        // in the overflow drawer instead of occupying any of those slots.
        let visible = crate::app::dock_display_apps(&apps, &metrics);
        self.dock_motion.drag_targets.clear();
        for (index, app) in visible.iter().enumerate() {
            if let Some(slot) = slots.get(index + launcher_offset) {
                self.dock_motion.drag_targets.push((
                    app.id.clone(),
                    slot.origin_main + slot.size / 2.,
                    slot.size / 2. + 4.,
                ));
            }
        }
        let folder_start =
            launcher_offset + metrics.visible_apps + usize::from(metrics.hidden_apps > 0);
        // The resting layout only moves when the metrics move, so fingerprint
        // the metrics and reuse the sample: the magnification frame path then
        // never re-runs the idle layout walkers.
        if self.dock_motion.idle_fingerprint.as_ref() != Some(&metrics) {
            self.dock_motion.idle_slots = std::rc::Rc::new(metrics.samples(None, 0.));
            self.dock_motion.idle_fingerprint = Some(metrics.clone());
        }
        let idle_slots = self.dock_motion.idle_slots.clone();
        self.dock_motion.geometry = DockGeometry {
            // Window coordinates: the tray is inset by the surface padding.
            base: idle_slots
                .get(launcher_offset)
                .map(|slot| slot.origin_main)
                .unwrap_or(metrics.main_padding + metrics.end_gap),
            folder_base: idle_slots
                .get(folder_start)
                .map(|slot| slot.origin_main)
                .unwrap_or(metrics.main_padding + metrics.end_gap),
            item: metrics.item,
            icon: metrics.icon_size,
            apps_end: folder_start
                .checked_sub(1)
                .and_then(|index| (index >= launcher_offset).then_some(index))
                .and_then(|index| idle_slots.get(index))
                .map(|slot| slot.origin_main + slot.size)
                .unwrap_or(metrics.main_padding),
            tray_cross_start: if vertical { tray_x } else { tray_y },
            tray_cross_end: if vertical {
                tray_x + tray_w
            } else {
                tray_y + tray_h
            },
            trash_start: slots.last().map(|slot| slot.origin_main).unwrap_or(0.),
        };
        // While a reorder is in flight the untouched pinned icons slide towards
        // their new slots instead of jumping. Each icon owns a critically damped
        // scalar keyed by application id; the target is the main-axis position of
        // the slot that icon would occupy in the live arrangement. Without a drag
        // the targets collapse back onto the resting slots, so the map drains.
        let mut reorder_active = false;
        if self.dock_motion.drag.is_some() {
            let base = self.dock_motion.geometry.base;
            let item = self.dock_motion.geometry.item;
            let dragged = self.dock_motion.drag.as_ref().map(|drag| drag.id.clone());
            let pinned_count = apps.iter().filter(|app| app.pinned).count();
            reorder_active |= crate::dock_motion::track_reorder(
                &mut self.dock_motion.reorder,
                40.,
                apps.iter()
                    .enumerate()
                    .filter(|(index, app)| {
                        app.pinned
                            && *index < pinned_count
                            && dragged.as_deref() != Some(app.id.as_str())
                    })
                    .map(|(index, app)| (app.id.clone(), base + index as f32 * item)),
                now,
            );
        } else {
            crate::dock_motion::drain_reorder(&mut self.dock_motion.reorder);
        }
        extra_active |= reorder_active;
        let mut folder_reorder_active = false;
        if let Some(drag) = self.dock_motion.folder_drag.clone() {
            let base = self.dock_motion.geometry.folder_base;
            let item = self.dock_motion.geometry.item;
            folder_reorder_active |= crate::dock_motion::track_reorder(
                &mut self.dock_motion.folder_reorder,
                40.,
                drag.order
                    .iter()
                    .enumerate()
                    .filter(|(_, path)| path != &&drag.path)
                    .map(|(index, path)| (path.clone(), base + index as f32 * item)),
                now,
            );
        } else {
            crate::dock_motion::drain_reorder(&mut self.dock_motion.folder_reorder);
        }
        extra_active |= folder_reorder_active;
        let mut recent_started = false;
        let mut folder_started = false;
        let mut icon_regions: Vec<Bounds<Pixels>> = Vec::new();
        for (index, slot) in slots.iter().enumerate() {
            let kind = crate::dock_motion::dock_slot_kind(&metrics, launcher_offset, index);
            let launcher = kind == crate::dock_motion::DockSlotKind::Launcher;
            let trash = kind == crate::dock_motion::DockSlotKind::Trash;
            let overflow_tile = kind == crate::dock_motion::DockSlotKind::Overflow;
            let folder_index = match kind {
                crate::dock_motion::DockSlotKind::Folder(index) => Some(index),
                _ => None,
            };
            let folder = folder_index.and_then(|index| host.dock_folders.get(index));
            // A freshly pinned folder grows and fades in on its real slot; a
            // settled slide rests at one, so every other tile ignores it.
            let folder_t = folder
                .and_then(|folder| {
                    self.dock_motion
                        .folder_appear
                        .get(&folder.path.to_string_lossy().into_owned())
                })
                .map(|slide| slide.sample(now).value)
                .unwrap_or(1.);
            extra_active |= folder
                .and_then(|folder| {
                    self.dock_motion
                        .folder_appear
                        .get(&folder.path.to_string_lossy().into_owned())
                })
                .map(|slide| slide.sample(now).active)
                .unwrap_or(false);
            let app_slot = match kind {
                crate::dock_motion::DockSlotKind::App(index) => index,
                _ => usize::MAX,
            };
            // The dragged application leaves a hole at its own slot; every
            // other slot maps one to one onto the reordered list.
            let hole = self
                .dock_motion
                .drag
                .as_ref()
                .is_some_and(|drag| app_slot == drag.insert);
            let app = if launcher || trash || overflow_tile || folder.is_some() || hole {
                None
            } else {
                visible.get(app_slot).copied()
            };
            if hole {
                // The dragged application floats over this slot; the slot itself
                // holds a soft, shadowed placeholder that marks the landing
                // position. It keeps the layout stable while the order changes.
                let size = slot.size.max(metrics.icon_size);
                let (ph_x, ph_y) = if !vertical {
                    (
                        slot.origin_main,
                        metrics.cross_padding + metrics.inset - slot.lift + slide,
                    )
                } else if left {
                    (metrics.inset - slide, slot.origin_main)
                } else {
                    (
                        metrics.cross_padding + metrics.inset - slot.lift + slide,
                        slot.origin_main,
                    )
                };
                let placeholder = div()
                    .absolute()
                    .left(px(ph_x))
                    .top(px(ph_y))
                    .w(px(size))
                    .h(px(size))
                    .rounded(px(size * 0.28))
                    .bg(if dark {
                        rgba(0xffffff1a)
                    } else {
                        rgba(0x1a1a2412)
                    })
                    .border_1()
                    .border_color(if dark {
                        rgba(0xffffff26)
                    } else {
                        rgba(0x1a1a241f)
                    });
                content = content.child(placeholder);
                continue;
            }
            // An external folder owns the extra slot reserved by the host, not
            // the trash slot. The trash keeps its artwork and slides outward.
            if folder_index.is_some() && folder.is_none() {
                // A recently unpinned folder rides this slot out before the
                // row settles: the tile stays in place and fades away under
                // the same shape math the pin placeholder uses.
                let exit_ride = folder_index
                    .and_then(|index| {
                        index.checked_sub(
                            host.dock_folders.len() + usize::from(host.folder_drop_preview),
                        )
                    })
                    .and_then(|offset| self.dock_motion.folder_exiting.get(offset).cloned());
                if let Some((folder, exit_slide)) = &exit_ride {
                    let sample = exit_slide.sample(now);
                    extra_active |= sample.active;
                    let t = sample.value.clamp(0., 1.);
                    if t > 0.001 {
                        let size = slot.size.max(metrics.icon_size);
                        let icon = size * (0.72 + 0.28 * t);
                        let (x, y) = if !vertical {
                            (
                                slot.origin_main,
                                metrics.cross_padding + metrics.inset - slot.lift + slide,
                            )
                        } else if left {
                            (metrics.inset - slide, slot.origin_main)
                        } else {
                            (
                                metrics.cross_padding + metrics.inset - slot.lift + slide,
                                slot.origin_main,
                            )
                        };
                        content = content.child(
                            div()
                                .debug_selector(|| "dock-folder-exit".into())
                                .absolute()
                                .left(px(x))
                                .top(px(y))
                                .size(px(size))
                                .opacity(t)
                                .child(folder_tile_icon(Some(folder), icon, false, p)),
                        );
                    }
                }
                let progress = folder_drop.value.clamp(0., 1.);
                let visible = self.dock_motion.folder_drop_visible;
                if exit_ride.is_none() && (visible || progress > 0.001) {
                    let size = slot.size.max(metrics.icon_size);
                    let icon = size * (0.72 + 0.28 * progress);
                    let (ph_x, ph_y) = if !vertical {
                        (
                            slot.origin_main,
                            metrics.cross_padding + metrics.inset - slot.lift + slide,
                        )
                    } else if left {
                        (metrics.inset - slide, slot.origin_main)
                    } else {
                        (
                            metrics.cross_padding + metrics.inset - slot.lift + slide,
                            slot.origin_main,
                        )
                    };
                    let opacity = if visible {
                        0.30 + 0.70 * progress
                    } else {
                        progress
                    };
                    let drop_host = self.host.clone();
                    // The + matches the badge an app shows when it accepts a
                    // dragged file: the reserved slot is another pin, styled
                    // like the communication point it is.
                    let plus = icon * 0.46;
                    let placeholder = div()
                        .id("dock-folder-drop-placeholder")
                        .debug_selector(|| "dock-folder-drop-placeholder".into())
                        .absolute()
                        .left(px(ph_x))
                        .top(px(ph_y))
                        .size(px(size))
                        .rounded(px(size * 0.26))
                        .bg(p.accent.opacity(0.10))
                        .border_1()
                        .border_color(p.accent.opacity(0.42))
                        .opacity(opacity)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .size(px(icon))
                                .relative()
                                .opacity(progress)
                                .child(folder_tile_icon(None, icon, false, p))
                                .child(
                                    div()
                                        .id("dock-folder-drop-plus")
                                        .debug_selector(|| "dock-folder-drop-plus".into())
                                        .absolute()
                                        .left(px(icon / 2. - plus / 2.))
                                        .top(px(icon / 2. - plus / 2.))
                                        .w(px(plus))
                                        .h(px(plus))
                                        .child(drop_accept_badge(plus, p)),
                                ),
                        )
                        .can_drop(|value, _, _| {
                            value
                                .downcast_ref::<gpui::ExternalPaths>()
                                .is_some_and(|paths| {
                                    paths.paths().len() == 1 && paths.paths()[0].is_dir()
                                })
                        })
                        .on_drop::<gpui::ExternalPaths>(move |paths, _, cx| {
                            let Some(path) = paths.paths().first().cloned() else {
                                return;
                            };
                            drop_host
                                .update(cx, |host, cx| host.handle(Action::PinFolder(path), cx));
                        });
                    content = content.child(placeholder);
                }
                // The preview is the first folder when no folder is pinned yet,
                // so it owns the group separator that the resting row would draw.
                if folder_index == Some(0) {
                    let position = slot.origin_main
                        - (crate::dock_motion::row_gap(metrics.icon_size)
                            + metrics.separator_extra)
                            / 2.;
                    content = content.child(
                        div()
                            .absolute()
                            .left(px(if vertical { tray_x } else { position }))
                            .top(px(if vertical { position } else { tray_y }))
                            .w(px(if vertical { metrics.thickness } else { 1. }))
                            .h(px(if vertical { 1. } else { metrics.thickness }))
                            .child(dock_separator(vertical, dark)),
                    );
                }
                folder_started = true;
                continue;
            }
            let id = if launcher {
                "dock-launchpad".to_owned()
            } else if overflow_tile {
                "dock-overflow".to_owned()
            } else if let Some(index) = folder_index {
                format!("dock-folder-{index}")
            } else if let Some(app) = app {
                format!("dock-{}", app.id)
            } else {
                "dock-trash".to_owned()
            };
            let bounce = app
                .and_then(|app| host.launches.get(&app.id))
                .filter(|_| host.prefs.animate_opening_applications && !host.prefs.reduce_motion)
                .map(|started| launch_bounce(started.elapsed(), metrics.icon_size));
            let hop = bounce
                .map(|b| b.offset)
                .unwrap_or(0.)
                .min(metrics.bounce_amplitude);
            extra_active |= bounce.is_some_and(|b| b.active);
            // A pinned icon sliding towards its new slot during a reorder
            // replaces its resting main-axis position with the animated sample.
            let slide_main = app
                .filter(|app| app.pinned)
                .and_then(|app| self.dock_motion.reorder.get(&app.id))
                .map(|motion| motion.sample(now).value)
                .or_else(|| {
                    folder.and_then(|folder| {
                        self.dock_motion
                            .folder_reorder
                            .get(&folder.path.to_string_lossy().into_owned())
                            .map(|motion| motion.sample(now).value)
                    })
                });
            let main = slide_main.unwrap_or(slot.origin_main);
            let (x, y) = if !vertical {
                (
                    main,
                    metrics.cross_padding + metrics.inset - slot.lift - hop + slide,
                )
            } else if left {
                (metrics.inset + hop - slide, main)
            } else {
                (
                    metrics.cross_padding + metrics.inset - slot.lift - hop + slide,
                    main,
                )
            };
            debug_assert!(slot.size <= metrics.icon_size * metrics.scale + 0.001);
            let folder_dragged = folder.is_some_and(|folder| {
                self.dock_motion
                    .folder_drag
                    .as_ref()
                    .is_some_and(|drag| drag.path == folder.path.to_string_lossy())
            });
            let mut tile = dock_slot(SharedString::from(id), slot.size)
                .left(px(x))
                .top(px(y));
            if overflow_tile {
                tile = tile.child(
                    div()
                        .id("dock-overflow-ellipsis")
                        .debug_selector(|| "dock-overflow-ellipsis".into())
                        .size(px(slot.size))
                        .rounded(px(slot.size * 0.22))
                        .border_1()
                        .border_color(p.line)
                        .bg(p.card)
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(p.fg)
                        .text_size(px(slot.size * 0.48))
                        .child("…"),
                );
                if metrics.icon_size >= 32. {
                    let badge = slot.size * 0.38;
                    let badge_font = (slot.size * 0.22).max(10.);
                    let badge_inset = slot.size * 0.10;
                    // A fixed-size wrapper keeps the badge centre anchored while
                    // the glyph inside may grow for a multi-digit count.
                    tile = tile.child(
                        div()
                            .absolute()
                            .top(px(-badge_inset))
                            .right(px(-badge_inset))
                            .w(px(badge))
                            .h(px(badge))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .id("dock-overflow-badge")
                                    .debug_selector(|| "dock-overflow-badge".into())
                                    .min_w(px(badge))
                                    .h(px(badge))
                                    .px(px(badge * 0.21))
                                    .rounded_full()
                                    .border_1()
                                    .border_color(if dark {
                                        rgba(0xffffff59)
                                    } else {
                                        rgba(0x00000026)
                                    })
                                    .bg(if dark {
                                        rgba(0x3a3a3ff2)
                                    } else {
                                        rgba(0xffffffff)
                                    })
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_color(if dark {
                                        rgba(0xffffffff)
                                    } else {
                                        rgba(0x252529ff)
                                    })
                                    .text_size(px(badge_font))
                                    .child(format!("{}", hidden.len())),
                            ),
                    );
                }
                let h = self.host.clone();
                let ids = hidden.clone();
                let icon_size = slot.size.round() as u16;
                tile = tile
                    .on_click(move |_, _, cx| {
                        h.update(cx, |h, cx| {
                            h.handle(
                                Action::Panel(Panel::DockOverflow {
                                    ids: ids.clone(),
                                    icon_size,
                                }),
                                cx,
                            )
                        })
                    })
                    .can_drop(|_, _, _| true)
                    .on_drop::<gpui::ExternalPaths>(|_, _, cx| cx.stop_propagation());
            } else if let Some(folder) = folder {
                if !folder_dragged {
                    let folder_open = matches!(
                        host.panel.as_ref(),
                        Some(Panel::DockFolder { root }) if root == &folder.path
                    );
                    tile = tile.child(folder_tile_icon(
                        Some(folder),
                        slot.size * (0.72 + 0.28 * folder_t),
                        folder_open,
                        p,
                    ));
                    if folder_t < 1. {
                        tile = tile.opacity(folder_t);
                    }
                    // A lifted folder tile is often the visible drop target.
                    // Accept a lone external folder and pin it to the folder
                    // group, matching the slot preview instead of letting the
                    // release fall through without an action.
                    let folder_tile_drop_host = self.host.clone();
                    tile = tile
                        .can_drop(|_, _, _| true)
                        .on_drop::<gpui::ExternalPaths>(move |paths, _, cx| {
                            let paths = paths.paths();
                            if paths.len() == 1 && paths[0].is_dir() {
                                let folder = paths[0].clone();
                                folder_tile_drop_host.update(cx, |host, cx| {
                                    host.handle(Action::PinFolder(folder), cx)
                                });
                            }
                        });
                    let name = folder.name.clone();
                    let pressed = folder.path.to_string_lossy().into_owned();
                    let released = pressed.clone();
                    let path = folder.path.clone();
                    let context = folder.path.clone();
                    let context_host = self.host.clone();
                    tile = tile
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, event: &MouseDownEvent, _, _| {
                                let pointer = f32::from(if vertical {
                                    event.position.y
                                } else {
                                    event.position.x
                                });
                                this.dock_motion.press_folder(pressed.clone(), pointer);
                            }),
                        )
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                if this.dock_motion.folder_drag.is_some() {
                                    return;
                                }
                                let held = this
                                    .dock_motion
                                    .folder_press
                                    .as_ref()
                                    .is_some_and(|press| press.path == released);
                                if !held {
                                    return;
                                }
                                let h = this.host.clone();
                                let path = path.clone();
                                h.update(cx, |h, cx| {
                                    h.handle(Action::OpenFolder(path), cx)
                                });
                            }),
                        )
                        .on_mouse_down(MouseButton::Right, move |_, _, cx| {
                            context_host.update(cx, |host, cx| {
                                host.handle(
                                    Action::Panel(Panel::DockFolderMenu {
                                        path: context.clone(),
                                    }),
                                    cx,
                                )
                            })
                        })
                        .on_hover(cx.listener(move |this, hovered, _, cx| {
                            this.dock_motion.hover_tooltip(
                                if *hovered { Some(name.as_str()) } else { None },
                                this.dock_motion.epoch.elapsed(),
                            );
                            cx.notify();
                        }));
                }
            } else {
                let accepting = app.is_some_and(|app| {
                    self.dock_motion.drag_accept.as_deref() == Some(app.id.as_str())
                });
                let icon = if launcher {
                    utility_icon("launchpad.png")
                } else if let Some(app) = app {
                    app.icon.clone()
                } else {
                    utility_icon(if host.system.trash_full {
                        "trash-full.png"
                    } else {
                        "trash-empty.png"
                    })
                };
                // A file the app can open lifts its icon slightly and tints the
                // artwork so the drop target stands out without hiding it.
                let icon_size = if accepting { slot.size * 1.12 } else { slot.size };
                let icon_inset = (slot.size - icon_size) / 2.;
                let icon_element = self.icon(icon, icon_size, p);
                tile = if accepting {
                    tile.child(
                        div()
                            .id("dock-drag-accept-icon")
                            .debug_selector(|| "dock-drag-accept-icon".into())
                            .absolute()
                            .left(px(icon_inset))
                            .top(px(icon_inset))
                            .size(px(icon_size))
                            .child(icon_element),
                    )
                } else {
                    tile.child(icon_element)
                };
                if accepting {
                    tile = tile
                        .child(
                            div()
                                .id("dock-drag-accept-wash")
                                .debug_selector(|| "dock-drag-accept-wash".into())
                                .absolute()
                                .left(px(icon_inset))
                                .top(px(icon_inset))
                                .size(px(icon_size))
                                .rounded(px(icon_size * 0.24))
                                .bg(p.accent.opacity(0.18))
                                .border_1()
                                .border_color(p.accent.opacity(0.55)),
                        )
                        .child(drop_accept_badge(slot.size, p));
                }
                if launcher {
                    let h = self.host.clone();
                    tile = tile
                        .on_click(move |_, _, cx| {
                            h.update(cx, |h, cx| h.handle(Action::Panel(Panel::Apps), cx))
                        })
                        .can_drop(|_, _, _| true)
                        .on_drop::<gpui::ExternalPaths>(|_, _, cx| cx.stop_propagation());
                } else if let Some(app) = app {
                    let activate = app.clone();
                    let context = app.id.clone();
                    let pressed = app.id.clone();
                    let released = app.id.clone();
                    let pinned = app.pinned;
                    tile = tile
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, event: &MouseDownEvent, _, _| {
                                let pointer = f32::from(if vertical {
                                    event.position.y
                                } else {
                                    event.position.x
                                });
                                this.dock_motion.press(pressed.clone(), pinned, pointer);
                            }),
                        )
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                // A reorder never launches the application.
                                if this.dock_motion.drag.is_some() {
                                    return;
                                }
                                let held = this
                                    .dock_motion
                                    .press
                                    .as_ref()
                                    .is_some_and(|press| press.id == released);
                                if !held {
                                    return;
                                }
                                let h = this.host.clone();
                                let app = activate.clone();
                                h.update(cx, |h, cx| h.activate_app(&app, cx));
                            }),
                        )
                        .on_mouse_down(MouseButton::Right, {
                            let h = self.host.clone();
                            move |_, _, cx| {
                                h.update(cx, |h, cx| {
                                    h.handle(Action::Panel(Panel::Dock(context.clone())), cx)
                                })
                            }
                        })
                        .on_hover(cx.listener({
                            let name = app.name.clone();
                            move |this, hovered, _, cx| {
                                let now = this.dock_motion.epoch.elapsed();
                                this.dock_motion.hover_tooltip(
                                    if *hovered { Some(name.as_str()) } else { None },
                                    now,
                                );
                                cx.notify();
                            }
                        }));
                    if let Some(desktop) = host.catalog.get(&app.id).cloned() {
                        let drop_app = desktop.clone();
                        let folder_drop_app = desktop;
                        let app_id = app.id.clone();
                        let drop_host = self.host.clone();
                        let folder_app_id = app.id.clone();
                        let folder_drop_host = self.host.clone();
                        tile = tile
                            .can_drop(|_, _, _| true)
                            .on_drop::<gpui::ExternalPaths>(move |paths, _, cx| {
                                let paths = paths.paths().to_vec();
                                if !crate::mime::app_accepts_paths(&drop_app, &paths) {
                                    // Unsupported drops stay consumed so a
                                    // stray file never opens or changes the
                                    // Dock; pinning is decided in the folder
                                    // strip only.
                                    return;
                                }
                                drop_host.update(cx, |host, cx| {
                                    host.handle(
                                        Action::OpenPathsWithApp {
                                            app_id: app_id.clone(),
                                            paths,
                                        },
                                        cx,
                                    )
                                });
                            })
                            .on_drop::<FolderEntryDrag>(move |drag, _, cx| {
                                let paths = vec![drag.path.clone()];
                                if !crate::mime::app_accepts_paths(&folder_drop_app, &paths) {
                                    return;
                                }
                                folder_drop_host.update(cx, |host, cx| {
                                    host.handle(
                                        Action::OpenPathsWithApp {
                                            app_id: folder_app_id.clone(),
                                            paths,
                                        },
                                        cx,
                                    )
                                });
                            });
                    } else {
                        tile = tile
                            .can_drop(|_, _, _| true)
                            .on_drop::<gpui::ExternalPaths>(|_, _, cx| cx.stop_propagation());
                    }
                } else {
                    let h = self.host.clone();
                    tile = tile
                        .on_click(move |_, _, cx| {
                            h.update(cx, |h, cx| {
                                h.handle(
                                    Action::Core(clarion_ipc::Action::Spawn {
                                        command: vec!["gio".into(), "open".into(), "trash:///".into()],
                                    }),
                                    cx,
                                )
                            })
                        })
                        .can_drop(|_, _, _| true)
                        .on_drop::<gpui::ExternalPaths>(|_, _, cx| cx.stop_propagation());
                }
            }
            // Raised/bouncing pixels remain clickable. The empty animation
            // budget stays click-through; glass and input use these same masks.
            if !folder_dragged {
                icon_regions.push(Bounds::new(
                    point(px(x), px(y)),
                    size(px(slot.size), px(slot.size)),
                ));
            }
            let recent = app.is_some_and(|a| {
                host.prefs.show_recent_applications && !a.pinned && a.windows.is_empty()
            });
            let folder_slot = folder.is_some();
            let first_folder = folder_slot && !folder_started;
            folder_started |= folder_slot;
            let separate =
                first_folder || (trash && !folder_started) || (recent && !recent_started);
            recent_started |= recent;
            if separate {
                // The separator sits in the middle of the gap it divides.
                let position = slot.origin_main
                    - (crate::dock_motion::row_gap(metrics.icon_size)
                        + metrics.separator_extra)
                        / 2.;
                content = content.child(
                    div()
                        .absolute()
                        .left(px(if vertical { tray_x } else { position }))
                        .top(px(if vertical { position } else { tray_y }))
                        .w(px(if vertical { metrics.thickness } else { 1. }))
                        .h(px(if vertical { 1. } else { metrics.thickness }))
                        .child(dock_separator(vertical, dark)),
                );
            }
            if app.is_some_and(|a| !a.windows.is_empty()) && host.prefs.show_running_indicators {
                // The indicator lives inside the band between the icon and the
                // screen facing edge of the tray.
                let dot = crate::dock_motion::indicator_diameter(metrics.icon_size);
                let (dot_x, dot_y) = if !vertical {
                    (
                        x + slot.size / 2. - dot / 2.,
                        y + slot.size + (metrics.inset - dot) / 2.,
                    )
                } else if left {
                    (
                        x - (metrics.inset + dot) / 2.,
                        slot.origin_main + slot.size / 2. - dot / 2.,
                    )
                } else {
                    (
                        x + slot.size + (metrics.inset - dot) / 2.,
                        slot.origin_main + slot.size / 2. - dot / 2.,
                    )
                };
                content = content.child(
                    div()
                        .absolute()
                        .left(px(dot_x))
                        .top(px(dot_y))
                        .size(px(dot))
                        .rounded_full()
                        .bg(if dark {
                            rgba(0xf4f4f4e6)
                        } else {
                            rgba(0x252529bf)
                        }),
                );
            }
            if !folder_dragged {
                content = content.child(tile);
            }
        }
        // The dragged application follows the pointer.
        if let Some(drag) = self.dock_motion.drag.clone() {
            if let Some(app) = apps.iter().find(|app| app.id == drag.id).cloned() {
                let icon = metrics.icon_size;
                let leading = (drag.pointer - drag.grab)
                    .clamp(metrics.end_gap, (metrics.length - metrics.end_gap - icon).max(0.));
                let (x, y) = if !vertical {
                    (
                        leading,
                        metrics.cross_padding + metrics.inset + slide,
                    )
                } else if left {
                    (metrics.inset - slide, leading)
                } else {
                    (
                        metrics.cross_padding + metrics.inset + slide,
                        leading,
                    )
                };
                let mut tile = dock_slot(SharedString::from(format!("dock-drag-{}", app.id)), icon)
                    .left(px(x))
                    .top(px(y))
                    .shadow(vec![BoxShadow {
                        color: rgba(0x00000052).into(),
                        offset: point(px(0.), px(6.)),
                        blur_radius: px(14.),
                        spread_radius: px(0.),
                    }]);
                tile = tile.child(self.icon(app.icon.clone(), icon, p));
                icon_regions.push(Bounds::new(
                    point(px(x), px(y)),
                    size(px(icon), px(icon)),
                ));
                content = content.child(tile);
            }
        }
        if let Some(drag) = self.dock_motion.folder_drag.clone() {
            if let Some(folder) = host
                .dock_folders
                .iter()
                .find(|folder| folder.path.to_string_lossy() == drag.path.as_str())
            {
                let icon = metrics.icon_size;
                let group_start = self.dock_motion.geometry.folder_base;
                let group_end = group_start + metrics.folder_count as f32 * metrics.item;
                let leading = (drag.pointer - drag.grab)
                    .clamp(group_start, (group_end - icon).max(group_start));
                let (x, y) = if !vertical {
                    (
                        leading,
                        metrics.cross_padding + metrics.inset + slide,
                    )
                } else if left {
                    (metrics.inset - slide, leading)
                } else {
                    (
                        metrics.cross_padding + metrics.inset + slide,
                        leading,
                    )
                };
                let mut tile =
                    dock_slot(SharedString::from("dock-folder-drag"), icon)
                        .left(px(x))
                        .top(px(y))
                        .shadow(vec![BoxShadow {
                            color: rgba(0x00000052).into(),
                            offset: point(px(0.), px(6.)),
                            blur_radius: px(14.),
                            spread_radius: px(0.),
                        }]);
                tile = tile.child(folder_tile_icon(Some(folder), icon, false, p));
                icon_regions.push(Bounds::new(
                    point(px(x), px(y)),
                    size(px(icon), px(icon)),
                ));
                content = content.child(tile);
            }
        }
        (content.into_any_element(), icon_regions, extra_active)
    }

    pub(super) fn macos_dock(
        &mut self,
        host: &ViewState,
        p: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut frame = self.dock_frame(host, window, cx);
        let (content, icon_regions, extra_active) = self.dock_slots(host, &frame, p, cx);
        frame.active |= extra_active;
        let mut regions = frame.tray_regions.clone();
        regions.extend(icon_regions);
        let vertical = frame.vertical;
        let left = frame.left;
        let tray_x = frame.tray_x;
        let tray_y = frame.tray_y;
        let tray_w = frame.tray_w;
        let tray_h = frame.tray_h;
        let metrics = &frame.metrics;
        let folder_drop_lift = frame.folder_drop_lift;        // Clip the protocol masks to the negotiated surface. Quantizing keeps
        // subpixel animation from issuing redundant Wayland region commits.
        let viewport = window.viewport_size();
        let quantize = |regions: Vec<Bounds<Pixels>>| -> Vec<Bounds<Pixels>> {
            regions
                .into_iter()
                .filter_map(|r| {
                    let x = f32::from(r.left()).floor().max(0.);
                    let y = f32::from(r.top()).floor().max(0.);
                    let right = f32::from(r.right()).ceil().min(f32::from(viewport.width));
                    let bottom = f32::from(r.bottom()).ceil().min(f32::from(viewport.height));
                    (right > x && bottom > y)
                        .then(|| Bounds::new(point(px(x), px(y)), size(px(right - x), px(bottom - y))))
                })
                .collect()
        };
        let glass = if p.frosted() {
            quantize(frame.tray_regions.clone())
        } else {
            Vec::new()
        };
        let regions = quantize(regions);
        // The hover label waits out `TOOLTIP_DELAY` before it appears. While it
        // is armed keep asking for frames so the delay elapses even if the
        // pointer stops moving.
        let now = frame.now;
        let mut active = frame.active;
        active |= self.dock_motion.tick_tooltip(now);
        #[cfg(test)]
        {
            self.dock_motion.input_regions = regions.clone();
            self.dock_motion.glass_regions = glass.clone();
            self.dock_motion.animation_active = active;
        }
        window.set_glass_region(Some(&glass));
        window.set_input_region(Some(&regions));
        if active {
            window.request_animation_frame();
        }
        let drop_host = self.host.clone();
        // The tray only accepts a folder pin inside the strip between the last
        // application tile and the trash; over the applications the drop is
        // decided by their own tiles and stays open-or-consumed.
        let (strip_start, strip_end) = self.folder_strip_main_range();
        let tray = div()
            .absolute()
            .left(px(tray_x))
            .top(px(tray_y))
            .w(px(tray_w))
            .h(px(tray_h))
            .rounded(px(metrics.radius))
            .can_drop(move |value, window, _| {
                value
                    .downcast_ref::<gpui::ExternalPaths>()
                    .is_some_and(|paths| {
                        let mouse = window.mouse_position();
                        let main = f32::from(if vertical { mouse.y } else { mouse.x });
                        main >= strip_start
                            && main < strip_end
                            && paths.paths().len() == 1
                            && paths.paths()[0].is_dir()
                    })
            })
            .on_drop::<gpui::ExternalPaths>(move |paths, _, cx| {
                let Some(path) = paths.paths().first().cloned() else {
                    return;
                };
                drop_host.update(cx, |host, cx| host.handle(Action::PinFolder(path), cx));
            })
            // The folder placeholder marks the landing slot. Do not outline
            // the whole tray while a folder is merely passing over it.
            .bg(p.dock)
            .shadow(vec![
                BoxShadow {
                    color: p.shadow,
                    offset: point(px(0.), px(2.)),
                    blur_radius: px(8.),
                    spread_radius: px(0.),
                },
                BoxShadow {
                    color: p.shadow.opacity(0.6),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(2.),
                    spread_radius: px(0.),
                },
            ])
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded(px(metrics.radius))
                    .border_1()
                    .border_color(p.dock_line)
                    .bg(linear_gradient(
                        180.,
                        linear_color_stop(p.frost_wash, 0.),
                        linear_color_stop(rgba(0xffffff00), 1.),
                    ))
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .rounded(px(metrics.radius))
                            .bg(linear_gradient(
                                180.,
                                linear_color_stop(p.accent_wash, 0.),
                                linear_color_stop(rgba(0xffffff00), 0.35),
                            )),
                    ),
            );
        let tooltip = self.dock_motion.tooltip_name.clone().map(|name| {
            // Size the label to the application name so the container grows with
            // it, and place it next to the hovered tile on the side away from the
            // screen edge.
            let size = metrics.icon_size;
            let viewport = window.viewport_size();
            let viewport_w = f32::from(viewport.width);
            let viewport_h = f32::from(viewport.height);
            // A resting minimum keeps short names from looking cramped; a ceiling
            // keeps a long name from spanning the output.
            let text = crate::view::measure_label(&name, TOOLTIP_TEXT_SIZE, window);
            let width = (text + TOOLTIP_PAD * 2.).clamp(TOOLTIP_MIN_W, TOOLTIP_MAX_W);
            let height = 26.;
            let center = frame.pointer.value.clamp(
                metrics.main_padding + size / 2.,
                (metrics.main_padding + metrics.length - size / 2.)
                    .max(metrics.main_padding + size / 2.),
            );
            let (tooltip_x, tooltip_y) = if !vertical {
                (
                    (center - width / 2.).clamp(2., (viewport_w - width - 2.).max(2.)),
                    (tray_y - height - TOOLTIP_GAP).max(2.),
                )
            } else if left {
                (
                    (tray_x + tray_w + TOOLTIP_GAP).min((viewport_w - width - 2.).max(2.)),
                    (center - height / 2.).clamp(2., (viewport_h - height - 2.).max(2.)),
                )
            } else {
                (
                    (tray_x - width - TOOLTIP_GAP).max(2.),
                    (center - height / 2.).clamp(2., (viewport_h - height - 2.).max(2.)),
                )
            };
            div()
                .id("dock-tooltip")
                .debug_selector(|| "dock-tooltip".into())
                .absolute()
                .left(px(tooltip_x))
                .top(px(tooltip_y))
                .w(px(width))
                .h(px(height))
                .flex()
                .items_center()
                .justify_center()
                .px(px(TOOLTIP_PAD))
                .overflow_hidden()
                .rounded(px(8.))
                .border_1()
                .border_color(p.line)
                .bg(p.panel)
                .text_color(p.fg)
                .text_size(px(TOOLTIP_TEXT_SIZE))
                .whitespace_nowrap()
                .text_ellipsis()
                .shadow(vec![BoxShadow {
                    color: p.shadow,
                    offset: point(px(0.), px(4.)),
                    blur_radius: px(12.),
                    spread_radius: px(0.),
                }])
                .child(name)
        });
        div()
            .id("dock-surface")
            .debug_selector(|| "dock-surface".into())
            .relative()
            .size_full()
            .child(tray)
            .child(content)
            .children(tooltip)
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                let target = f32::from(if vertical {
                    event.position.y
                } else {
                    event.position.x
                });
                if cx.has_active_drag() {
                    this.dock_drag_seen = true;
                }
                if this.dock_motion.folder_drag.is_some() {
                    this.dock_motion.track_folder_drag(target);
                    cx.notify();
                } else if this.dock_motion.drag.is_some() {
                    this.dock_motion.track_drag(target);
                    cx.notify();
                } else if this
                    .dock_motion
                    .folder_press
                    .as_ref()
                    .is_some_and(|press| {
                        (target - press.pointer).abs() >= crate::dock_motion::DRAG_THRESHOLD
                    })
                {
                    let folders = this.state.dock_folders.clone();
                    if this.dock_motion.begin_folder_drag(target, &folders) {
                        this.dock_motion.track_folder_drag(target);
                        cx.notify();
                    }
                } else if this
                    .dock_motion
                    .press
                    .as_ref()
                    .is_some_and(|press| press.pinned)
                    && (target - this.dock_motion.press.as_ref().unwrap().pointer).abs()
                        >= crate::dock_motion::DRAG_THRESHOLD
                {
                    let pinned: Vec<DockApp> = this
                        .host
                        .read(cx)
                        .dock_apps()
                        .into_iter()
                        .filter(|app| app.pinned)
                        .collect();
                    if this.dock_motion.begin_drag(target, &pinned) {
                        cx.notify();
                    }
                }
                if this.dock_motion.point_at(target) {
                    cx.notify();
                }
            }))
            // External file drags arrive as an active drag rather than a plain
            // hover. Keep the same pointer target and magnification state alive
            // so the app under the file animates exactly like a mouse hover.
            .on_drag_move::<gpui::ExternalPaths>(cx.listener(
                move |this, event: &gpui::DragMoveEvent<gpui::ExternalPaths>, window, cx| {
                    let position = event.event.position;
                    let viewport = window.viewport_size();
                    let inside = position.x >= px(0.)
                        && position.y >= px(0.)
                        && position.x <= viewport.width
                        && position.y <= viewport.height;
                    let was_hovered = this.dock_motion.hovered;
                    this.dock_drag_seen = true;
                    this.dock_motion.hovered = inside;
                    let mut changed = was_hovered != inside;
                    if inside {
                        let target = f32::from(if vertical { position.y } else { position.x });
                        if this.dock_motion.pointer_target != Some(target) {
                            if this.dock_motion.pointer_target.is_none() {
                                this.dock_motion.pointer.snap(target);
                            }
                            this.dock_motion.pointer_target = Some(target);
                            changed = true;
                        }
                    } else if this.dock_motion.pointer_target.take().is_some() {
                        changed = true;
                    }
                    let accept = if inside {
                        this.drag_accept_target(position, event.drag(cx).paths(), window)
                    } else {
                        None
                    };
                    if this.dock_motion.drag_accept != accept {
                        this.dock_motion.drag_accept = accept;
                        changed = true;
                    }
                    let main = f32::from(if vertical { position.y } else { position.x });
                    let cross = f32::from(if vertical { position.x } else { position.y });
                    let cross_start = if left {
                        this.dock_motion.geometry.tray_cross_start
                    } else {
                        this.dock_motion.geometry.tray_cross_start - folder_drop_lift
                    };
                    let cross_end = if left {
                        this.dock_motion.geometry.tray_cross_end + folder_drop_lift
                    } else {
                        this.dock_motion.geometry.tray_cross_end
                    };
                    // The fold strip starts a third of the way between the
                    // application group's trailing edge and the first folder
                    // slot (or the trash, when nothing is pinned), so drops
                    // heading for the applications keep using their tiles.
                    // Detection lives inside this strip: over the applications
                    // a drag is only checked for openable targets, and the
                    // trash never pins.
                    let (strip_start, strip_end) = this.folder_strip_main_range();
                    let over_folder_strip = inside
                        && main >= strip_start
                        && main < strip_end
                        && cross >= cross_start
                        && cross <= cross_end;
                    let paths = event.drag(cx).paths();
                    let reserve = over_folder_strip
                        && paths.len() == 1
                        && paths[0].is_dir()
                        && this.dock_motion.drag_accept.is_none();
                    // The slot is a live offer, not a latch: leaving the strip
                    // — towards the applications or the trash — ends the
                    // reservation, so detection only exists inside the folder
                    // area.
                    let visible = reserve;
                    if this.dock_motion.folder_drop_active != reserve {
                        this.dock_motion.folder_drop_active = reserve;
                        let h = this.host.clone();
                        h.update(cx, |host, cx| {
                            host.handle(Action::FolderDropPreview(reserve), cx)
                        });
                        changed = true;
                    }
                    if this.dock_motion.folder_drop_visible != visible {
                        this.dock_motion.folder_drop_visible = visible;
                        changed = true;
                    }
                    if changed {
                        cx.notify();
                    }
                },
            ))
            .on_drag_move::<FolderEntryDrag>(cx.listener(
                move |this, event: &gpui::DragMoveEvent<FolderEntryDrag>, window, cx| {
                    let position = event.event.position;
                    let viewport = window.viewport_size();
                    let inside = position.x >= px(0.)
                        && position.y >= px(0.)
                        && position.x <= viewport.width
                        && position.y <= viewport.height;
                    let path = event.drag(cx).path.clone();
                    this.dock_drag_seen = true;
                    let mut changed = if inside {
                        let target = f32::from(if vertical { position.y } else { position.x });
                        this.dock_motion.enter_hover(target)
                    } else {
                        this.dock_motion.clear_hover()
                    };
                    let accept = if inside {
                        this.drag_accept_target(position, std::slice::from_ref(&path), window)
                    } else {
                        None
                    };
                    if this.dock_motion.drag_accept != accept {
                        this.dock_motion.drag_accept = accept;
                        changed = true;
                    }
                    if changed {
                        cx.notify();
                    }
                },
            ))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    if this.dock_motion.drag_accept.take().is_some() {
                        cx.notify();
                    }
                    if let Some(order) = this.dock_motion.end_folder_drag() {
                        let h = this.host.clone();
                        cx.defer(move |cx| {
                            h.update(cx, |h, cx| h.handle(Action::FolderOrder(order), cx))
                        });
                        cx.notify();
                        return;
                    }
                    let Some(order) = this.dock_motion.end_drag() else {
                        return;
                    };
                    let h = this.host.clone();
                    cx.defer(move |cx| h.update(cx, |h, cx| h.handle(Action::DockOrder(order), cx)));
                    cx.notify();
                }),
            )
            .on_hover(cx.listener(move |this, hovered, window, cx| {
                if *hovered {
                    let pos = window.mouse_position();
                    let target = f32::from(if vertical { pos.y } else { pos.x });
                    if this.dock_motion.enter_hover(target) {
                        this.dock_drag_seen = false;
                    }
                } else {
                    // The magnification target is not only driven by this
                    // listener: plain moves inside the surface re-establish it
                    // without touching `hovered` (see the mouse-move handler),
                    // and the drag-end cleanup may already have reset `hovered`
                    // before this exit arrives. `clear_hover` must run on every
                    // leave unconditionally, or a row that just absorbed a
                    // dropped folder keeps the icon magnified while the pointer
                    // is outside.
                    this.dock_motion.clear_hover();
                    this.dock_drag_seen = false;
                    // A pointer that leaves the surface abandons the reorder.
                    this.dock_motion.abort_drag();
                    this.dock_motion.abort_folder_drag();
                }
                let h = this.host.clone();
                let hovered = *hovered;
                cx.defer(move |cx| h.update(cx, |h, cx| h.handle(Action::DockHover(hovered), cx)));
                cx.notify();
            }))
            .into_any_element()
    }
}
