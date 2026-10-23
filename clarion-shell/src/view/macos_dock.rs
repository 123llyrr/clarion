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
    /// The main-axis span of the tray as last painted.
    pub tray_start: f32,
    pub tray_end: f32,
    /// The cross-axis span of the tray as last painted.
    pub tray_cross_start: f32,
    pub tray_cross_end: f32,
    /// The main-axis span of the trash tile.
    pub trash_start: f32,
    pub trash_end: f32,
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
    pub(super) folder_drop_active: bool,
    /// The reserved slot should show the folder placeholder rather than stay
    /// empty because the pointer is over the trash or an application.
    pub(super) folder_drop_visible: bool,
    /// Fade and scale of the folder pin placeholder.
    pub(super) folder_drop: crate::dock_motion::Motion,
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
                item: 60.,
                icon: 48.,
                tray_start: 0.,
                tray_end: 0.,
                tray_cross_start: 0.,
                tray_cross_end: 0.,
                trash_start: 0.,
                trash_end: 0.,
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

impl ShellView {
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

    pub(super) fn macos_dock(
        &mut self,
        host: &ViewState,
        p: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use crate::dock_motion::{launch_bounce, DockMetrics};
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
        let mut metrics = DockMetrics::with_folders(
            apps.len(),
            host.prefs.dock_size as f32,
            host.prefs.magnification_scale as f32,
            host.prefs.magnification && !host.prefs.reduce_motion,
            host.prefs.show_launcher,
            host.dock_folders.len() + usize::from(host.folder_drop_preview),
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
            self.dock_motion.drag_accept = None;
            self.dock_motion.folder_drop_active = false;
            self.dock_motion.folder_drop_visible = false;
            self.dock_drag_seen = false;
        }
        // The host owns the extra slot because it also owns the layer-surface
        // size. Release it after the drag ends or after the pointer leaves the
        // tray, even if no final drag-move packet arrives.
        if host.folder_drop_preview && !self.dock_motion.folder_drop_active {
            let h = self.host.clone();
            h.update(cx, |h, cx| h.handle(Action::FolderDropPreview(false), cx));
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
                self.dock_motion.hovered = true;
                if self.dock_motion.pointer_target != Some(target) {
                    if self.dock_motion.pointer_target.is_none() {
                        self.dock_motion.pointer.snap(target);
                    }
                    self.dock_motion.pointer_target = Some(target);
                }
            } else {
                self.dock_motion.hovered = false;
                self.dock_motion.pointer_target = None;
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
        let mut active = visibility.active
            || strength.active
            || folder_drop.active
            || (strength.value > 0.001 && pointer.active);
        let mut regions = rounded_region(tray_w, tray_h, metrics.radius);
        for region in &mut regions {
            region.origin.x += px(tray_x);
            region.origin.y += px(tray_y);
        }
        // The frosted shape is the resting tray alone. Lifted, magnified icons
        // are still clickable but must not smear the backdrop beside the tray.
        let glass: Vec<Bounds<Pixels>> = regions.clone();
        let mut content = div().absolute().inset_0();
        let launcher_offset = usize::from(host.prefs.show_launcher);
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
        let idle_slots = metrics.samples(None, 0.);
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
            tray_start: if vertical { tray_y } else { tray_x },
            tray_end: if vertical {
                tray_y + tray_h
            } else {
                tray_x + tray_w
            },
            tray_cross_start: if vertical { tray_x } else { tray_y },
            tray_cross_end: if vertical {
                tray_x + tray_w
            } else {
                tray_y + tray_h
            },
            trash_start: slots.last().map(|slot| slot.origin_main).unwrap_or(0.),
            trash_end: slots
                .last()
                .map(|slot| slot.origin_main + slot.size)
                .unwrap_or(0.),
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
            for (index, app) in apps.iter().enumerate() {
                if !app.pinned || index >= pinned_count {
                    continue;
                }
                if dragged.as_deref() == Some(app.id.as_str()) {
                    continue;
                }
                let target_main = base + index as f32 * item;
                let motion = self
                    .dock_motion
                    .reorder
                    .entry(app.id.clone())
                    .or_insert_with(|| crate::dock_motion::Motion::with_frequency(target_main, 40.));
                if motion.target() != target_main {
                    motion.retarget(target_main, now);
                }
                reorder_active |= motion.sample(now).active;
            }
        } else {
            for motion in self.dock_motion.reorder.values_mut() {
                motion.snap(motion.target());
            }
            self.dock_motion.reorder.clear();
        }
        active |= reorder_active;
        let mut folder_reorder_active = false;
        if let Some(drag) = self.dock_motion.folder_drag.clone() {
            let base = self.dock_motion.geometry.folder_base;
            let item = self.dock_motion.geometry.item;
            for (index, path) in drag.order.iter().enumerate() {
                if path == &drag.path {
                    continue;
                }
                let target_main = base + index as f32 * item;
                let motion = self
                    .dock_motion
                    .folder_reorder
                    .entry(path.clone())
                    .or_insert_with(|| {
                        crate::dock_motion::Motion::with_frequency(target_main, 40.)
                    });
                if motion.target() != target_main {
                    motion.retarget(target_main, now);
                }
                folder_reorder_active |= motion.sample(now).active;
            }
        } else {
            for motion in self.dock_motion.folder_reorder.values_mut() {
                motion.snap(motion.target());
            }
            self.dock_motion.folder_reorder.clear();
        }
        active |= folder_reorder_active;
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
                let progress = folder_drop.value.clamp(0., 1.);
                let visible = self.dock_motion.folder_drop_visible;
                if visible || progress > 0.001 {
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
                                .opacity(progress)
                                .child(folder_tile_icon(None, icon, false, p)),
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
            active |= bounce.is_some_and(|b| b.active);
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
                        slot.size,
                        folder_open,
                        p,
                    ));
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
                                    // Consume unsupported drops so they can never
                                    // become a folder pin.
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
        for region in icon_regions {
            regions.push(region);
        }
        // Clip the protocol masks to the negotiated surface. Quantizing keeps
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
            quantize(glass)
        } else {
            Vec::new()
        };
        let regions = quantize(regions);
        // The hover label waits out `TOOLTIP_DELAY` before it appears. While it
        // is armed keep asking for frames so the delay elapses even if the
        // pointer stops moving.
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
        let tray = div()
            .absolute()
            .left(px(tray_x))
            .top(px(tray_y))
            .w(px(tray_w))
            .h(px(tray_h))
            .rounded(px(metrics.radius))
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
            let center = pointer.value.clamp(
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
                if this.dock_motion.pointer_target != Some(target) {
                    if this.dock_motion.pointer_target.is_none() {
                        this.dock_motion.pointer.snap(target);
                    }
                    this.dock_motion.pointer_target = Some(target);
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
                    let over_tray = inside
                        && main >= this.dock_motion.geometry.tray_start
                        && main <= this.dock_motion.geometry.tray_end
                        && cross >= this.dock_motion.geometry.tray_cross_start
                        && cross <= this.dock_motion.geometry.tray_cross_end;
                    let over_app = inside
                        && this.dock_motion.drag_targets.iter().any(|(_, center, reach)| {
                            (main - *center).abs() <= *reach
                        });
                    let over_trash = main >= this.dock_motion.geometry.trash_start
                        && main <= this.dock_motion.geometry.trash_end;
                    let paths = event.drag(cx).paths();
                    let holds_folder = over_tray && paths.len() == 1 && paths[0].is_dir();
                    let over_control = over_app || over_trash;
                    // Once the slot is open it stays open while the folder is
                    // still over the tray, even when the pointer crosses the
                    // trash or an app. That keeps the shifted trash from
                    // oscillating out from under the drop target.
                    let reserve = holds_folder
                        && (this.dock_motion.folder_drop_active
                            || (!over_control && this.dock_motion.drag_accept.is_none()));
                    // Once the slot is reserved, keep its placeholder visible
                    // while the pointer crosses the trash or an app. Hiding it
                    // here made the slot look like it had disappeared even
                    // though the host was still holding it open.
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
                if this.dock_motion.hovered == *hovered {
                    return;
                }
                this.dock_motion.hovered = *hovered;
                if *hovered {
                    let pos = window.mouse_position();
                    let target = f32::from(if vertical { pos.y } else { pos.x });
                    this.dock_motion.pointer.snap(target);
                    this.dock_motion.pointer_target = Some(target);
                } else {
                    this.dock_motion.pointer_target = None;
                    this.dock_drag_seen = false;
                    this.dock_motion.drag_accept = None;
                    this.dock_motion.folder_drop_active = false;
                    this.dock_motion.folder_drop_visible = false;
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
