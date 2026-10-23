// SPDX-License-Identifier: GPL-3.0-or-later
//! Each desktop component has its own placement and surface role.
use super::*;
use crate::preferences::FolderLayout;

pub(crate) struct Placement {
    /// The layer-shell window bounds. A modal popup covers the whole output so
    /// it can catch a click outside the visible popup and dismiss itself.
    pub bounds: Bounds<Pixels>,
    pub anchor: LayerShellAnchor,
    pub margins: LayerShellMargins,
    pub normal: bool,
    /// A pointer triangle on the popup edge that names the owning Dock tile.
    pub arrow: Option<Arrow>,
    /// For a modal popup, the visible popup rectangle inside the fullscreen
    /// window. `None` means the content fills the whole window.
    pub popup: Option<Bounds<Pixels>>,
}

/// The edge of a popup that carries the pointer triangle and its offset from
/// the popup's leading corner along that edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Arrow {
    pub edge: ArrowEdge,
    pub offset: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ArrowEdge {
    Bottom,
    Left,
    Right,
}

/// Half-width of the pointer triangle and the height it protrudes, shared by
/// the placement and the Dock menu view so the popup fits its own arrow.
pub(crate) const DOCK_ARROW: f32 = 9.;
/// One Dock context-menu button row, matching the rendered `ShellView::button`
/// height (13pt label, 5pt vertical padding and rounding).
pub(crate) const DOCK_MENU_ROW: f32 = 31.;
/// Row gap and body padding of the Dock context menu, matching the view.
pub(crate) const DOCK_MENU_GAP: f32 = 8.;
pub(crate) const DOCK_MENU_PAD: f32 = 10.;
/// The menu body's one-point border on each side.
pub(crate) const DOCK_MENU_BORDER: f32 = 2.;
pub(crate) const DOCK_MENU_WIDTH: f32 = 250.;
pub(crate) const FOLDER_LIST_WIDTH: f32 = 300.;
pub(crate) const FOLDER_GRID_WIDTH: f32 = 380.;
pub(crate) const FOLDER_MENU_WIDTH: f32 = 240.;
const FOLDER_LIST_ROW: f32 = 34.;
const FOLDER_GRID_ROW: f32 = 84.;
const FOLDER_GRID_COLUMNS: usize = 4;
const FOLDER_HEADER: f32 = 34.;
const FOLDER_FOOTER: f32 = 34.;
/// Compact metrics shared with the folder menu view. The menu packs four
/// option groups, their dividers and the actions into one short column.
pub(crate) const FOLDER_MENU_HEADER_ROW: f32 = 27.;
pub(crate) const FOLDER_MENU_LABEL_ROW: f32 = 18.;
pub(crate) const FOLDER_MENU_CHOICE_ROW: f32 = 22.;
pub(crate) const FOLDER_MENU_ACTION_ROW: f32 = 22.;
pub(crate) const FOLDER_MENU_SEPARATOR_ROW: f32 = 1.;
pub(crate) const FOLDER_MENU_INNER_GAP: f32 = 1.;
pub(crate) const FOLDER_MENU_CHOICE_GAP: f32 = 1.;
/// Folder popups never cover more than this share of the output. Their bodies
/// scroll, so clamping only shortens the visible list as entries change.
const FOLDER_POPUP_MAX_SCREEN_SHARE: f32 = 0.8;

/// Height of the Dock context menu content: one focus row per window, a
/// minimize/close row per window, and the pin row, plus the body padding and
/// the gaps between rows. The popup is sized from this so no empty band is left
/// below the last item and no row is clipped.
pub(crate) fn dock_menu_content_height(state: &ViewState, id: &str) -> f32 {
    let apps = state.dock_apps();
    let rows = match apps.iter().find(|app| app.id == id) {
        Some(app) => {
            // Each window contributes a focus row and a minimize/close row,
            // then the pin row follows. A closed pinned app shows only the pin
            // row, so zero windows is one row rather than two.
            app.windows.len().min(8) as f32 * 2. + 1.
        }
        None => {
            // A closed, unpinned application offers launch and pin.
            2.
        }
    };
    2. * DOCK_MENU_PAD + rows * DOCK_MENU_ROW + (rows - 1.) * DOCK_MENU_GAP
}

/// The whole popup window height for a Dock menu, including the body border and
/// the pointer triangle. The view lays the body out at
/// `height - DOCK_ARROW`, so this is the window extent the placement asks for.
pub(crate) fn dock_menu_window_height(state: &ViewState, id: &str) -> f32 {
    dock_menu_content_height(state, id) + DOCK_MENU_BORDER + DOCK_ARROW
}

/// Dock geometry for the current preferences and output. The overflow tile
/// folds only pinned applications, so running and recent applications keep
/// their own slots in the row.
pub(crate) fn dock_metrics(
    state: &ViewState,
    apps: &[DockApp],
    w: f32,
    h: f32,
) -> crate::dock_motion::DockMetrics {
    let mut metrics = crate::dock_motion::DockMetrics::with_folders(
        apps.len(),
        state.prefs.dock_size as f32,
        state.prefs.magnification_scale as f32,
        state.prefs.magnification && !state.prefs.reduce_motion,
        state.prefs.show_launcher,
        state.dock_folders.len() + usize::from(state.folder_drop_preview),
        state.prefs.dock_position != DockPosition::Bottom,
        w,
        h,
    );
    let (pinned, running, recent) = dock_app_counts(apps);
    metrics.restrict_overflow(pinned, running, recent);
    metrics
}

/// Screen-space center of the Dock slot at `slot` in the resting row. The Dock
/// layer surface is centered across the free axis inside the output's
/// non-exclusive zone, and the launcher owns slot 0, so the slot of a dock
/// application is its index plus the launcher offset. The idle tray is centered
/// inside the surface band reserved for magnification.
///
/// Top-left origin of the Dock layer surface in output coordinates.
///
/// A bottom Dock is anchored to the output's bottom edge with an 8pt margin.
/// A left or right Dock leaves its vertical axis unanchored; layer-shell centers
/// it in the output's non-exclusive zone, which starts below the top bar. Using
/// the full-output centre here would put every popup arrow half a bar too high.
pub(crate) fn dock_surface_origin(
    state: &ViewState,
    metrics: &crate::dock_motion::DockMetrics,
    w: f32,
    h: f32,
) -> (f32, f32) {
    match state.prefs.dock_position {
        DockPosition::Bottom => {
            ((w - metrics.surface_width) / 2., h - 8. - metrics.surface_height)
        }
        DockPosition::Left | DockPosition::Right => {
            let bar = state.prefs.bar_height as f32;
            let y = bar + (h - bar - metrics.surface_height) / 2.;
            if state.prefs.dock_position == DockPosition::Left {
                (8., y)
            } else {
                (w - 8. - metrics.surface_width, y)
            }
        }
    }
}

/// Returns `(x, y)` in output coordinates.
pub(crate) fn dock_tile_center(
    state: &ViewState,
    metrics: &crate::dock_motion::DockMetrics,
    slot: usize,
    w: f32,
    h: f32,
) -> (f32, f32) {
    // Surface-local idle icon center along the main axis. Sample the same
    // separator-aware layout the Dock paints instead of assuming a uniform row.
    let main = metrics
        .samples(None, 0.)
        .get(slot)
        .map(|sample| sample.origin_main + sample.size / 2.)
        .unwrap_or_else(|| {
            metrics.main_padding
                + metrics.end_gap
                + slot as f32 * metrics.item
                + metrics.icon_size / 2.
        });
    let cross = metrics.thickness / 2.;
    let (surface_x, surface_y) = dock_surface_origin(state, metrics, w, h);
    match state.prefs.dock_position {
        DockPosition::Bottom => (surface_x + main, surface_y + metrics.cross_padding + cross),
        DockPosition::Left => (surface_x + cross, surface_y + main),
        DockPosition::Right => (
            surface_x + metrics.surface_width - cross,
            surface_y + main,
        ),
    }
}

fn folder_entry_count(state: &ViewState, root: &std::path::Path) -> usize {
    let path = state
        .folder_navigation
        .as_ref()
        .filter(|navigation| navigation.root == root)
        .map(|navigation| navigation.current.as_path())
        .unwrap_or(root);
    state
        .folder_snapshots
        .get(path)
        .map(|snapshot| snapshot.entries.len())
        .or_else(|| {
            state
                .dock_folders
                .iter()
                .find(|folder| folder.path == root)
                .and_then(|folder| folder.snapshot.as_ref())
                .map(|snapshot| snapshot.entries.len())
        })
        .unwrap_or(0)
}

fn folder_popup_size(
    state: &ViewState,
    root: &std::path::Path,
    layout: FolderLayout,
    w: f32,
    h: f32,
) -> (f32, f32) {
    let count = folder_entry_count(state, root);
    let (width, rows, row_height) = match layout {
        FolderLayout::Grid => (
            FOLDER_GRID_WIDTH,
            if count == 0 {
                2
            } else {
                count.div_ceil(FOLDER_GRID_COLUMNS)
            },
            FOLDER_GRID_ROW,
        ),
        _ => (
            FOLDER_LIST_WIDTH,
            if count == 0 { 3 } else { count },
            FOLDER_LIST_ROW,
        ),
    };
    let width = width.min(w - 24.);
    let content = rows as f32 * row_height + (rows.saturating_sub(1)) as f32 * 4.;
    let height = 2. * DOCK_MENU_PAD
        + FOLDER_HEADER
        + 8.
        + content
        + 8.
        + FOLDER_FOOTER
        + DOCK_MENU_BORDER
        + DOCK_ARROW;
    let available = (h - (state.prefs.bar_height as f32 + 8.) - 12.).max(100.);
    (
        width,
        height
            .min(h * FOLDER_POPUP_MAX_SCREEN_SHARE)
            .min(available),
    )
}

fn folder_menu_window_height() -> f32 {
    let actions = 4.;
    let labels = 4.;
    let choices = 12.;
    let separators = 5.;
    let groups = 4.;
    // Include the filename header, the dividers and every gap between rows, so
    // the popup ends exactly under the last action. Choice rows have their own
    // gap because each group is a separate flex column.
    let blocks = 1. + separators + labels + groups + actions;
    2. * DOCK_MENU_PAD
        + FOLDER_MENU_HEADER_ROW
        + separators * FOLDER_MENU_SEPARATOR_ROW
        + labels * FOLDER_MENU_LABEL_ROW
        + choices * FOLDER_MENU_CHOICE_ROW
        + actions * FOLDER_MENU_ACTION_ROW
        + (blocks - 1.) * FOLDER_MENU_INNER_GAP
        + (choices - groups) * FOLDER_MENU_CHOICE_GAP
        + DOCK_MENU_BORDER
        + DOCK_ARROW
}

pub(crate) fn folder_tile_center(
    state: &ViewState,
    root: &std::path::Path,
    w: f32,
    h: f32,
) -> Option<(f32, f32)> {
    let apps = state.dock_apps();
    let metrics = dock_metrics(state, &apps, w, h);
    let index = state
        .dock_folders
        .iter()
        .position(|folder| folder.path == root)?;
    let slot = usize::from(state.prefs.show_launcher)
        + metrics.visible_apps
        + usize::from(metrics.hidden_apps > 0)
        + index;
    Some(dock_tile_center(state, &metrics, slot, w, h))
}

fn fan_fits(state: &ViewState, root: &std::path::Path, count: usize, w: f32, h: f32) -> bool {
    if count == 0 || count > 8 {
        return false;
    }
    let Some((x, y)) = folder_tile_center(state, root, w, h) else {
        return false;
    };
    let icon = state.prefs.dock_size as f32;
    let spacing = icon * 0.94;
    let margin = 16.;
    let available = match state.prefs.dock_position {
        DockPosition::Bottom => y - margin,
        DockPosition::Left | DockPosition::Right => h - 32.,
    };
    let capacity = ((available / spacing).floor() as usize).clamp(1, 10);
    let label_room = 112.;
    count <= capacity
        && match state.prefs.dock_position {
            DockPosition::Bottom => x >= label_room || w - x >= label_room,
            DockPosition::Left => w - x >= 220.,
            DockPosition::Right => x >= 220.,
        }
}

pub(crate) fn resolved_folder_layout(
    state: &ViewState,
    root: &std::path::Path,
    w: f32,
    h: f32,
) -> FolderLayout {
    let count = folder_entry_count(state, root);
    let configured = state
        .dock_folders
        .iter()
        .find(|folder| folder.path == root)
        .map(|folder| folder.layout)
        .unwrap_or(FolderLayout::Auto);
    match configured {
        FolderLayout::Auto => match state.prefs.dock_position {
            DockPosition::Bottom
                if count > 0 && count <= 8 && fan_fits(state, root, count, w, h) =>
            {
                FolderLayout::Fan
            }
            DockPosition::Bottom => FolderLayout::Grid,
            _ if count > 0 && count <= 12 => FolderLayout::List,
            _ => FolderLayout::Grid,
        },
        layout => layout,
    }
}

fn dock_popup_geometry(
    state: &ViewState,
    metrics: &crate::dock_motion::DockMetrics,
    tile_x: f32,
    tile_y: f32,
    width: f32,
    height: f32,
    w: f32,
    h: f32,
    top: f32,
) -> (f32, f32, f32, f32, Arrow) {
    let edge = metrics.thickness + 18.;
    match state.prefs.dock_position {
        DockPosition::Bottom => {
            let x = (tile_x - width / 2.).clamp(8., (w - width - 8.).max(8.));
            let offset = (tile_x - x).clamp(DOCK_ARROW, (width - DOCK_ARROW).max(DOCK_ARROW));
            (
                width,
                height,
                x,
                h - edge - (height - DOCK_ARROW),
                Arrow {
                    edge: ArrowEdge::Bottom,
                    offset,
                },
            )
        }
        DockPosition::Left | DockPosition::Right => {
            let width = width + DOCK_ARROW;
            let height = height - DOCK_ARROW;
            let y = (tile_y - height / 2.).clamp(top, (h - height - 8.).max(top));
            let offset = (tile_y - y).clamp(DOCK_ARROW, (height - DOCK_ARROW).max(DOCK_ARROW));
            if state.prefs.dock_position == DockPosition::Left {
                (
                    width,
                    height,
                    edge - DOCK_ARROW,
                    y,
                    Arrow {
                        edge: ArrowEdge::Left,
                        offset,
                    },
                )
            } else {
                (
                    width,
                    height,
                    w - edge - width + DOCK_ARROW,
                    y,
                    Arrow {
                        edge: ArrowEdge::Right,
                        offset,
                    },
                )
            }
        }
    }
}

pub(crate) fn placement(state: &ViewState, panel: &Panel, w: f32, h: f32) -> Placement {
    let bar = state.prefs.bar_height as f32;
    let top = bar + 8.;
    let available = (h - top - 12.).max(100.);
    let mut anchor = LayerShellAnchor::default();
    let mut margins = LayerShellMargins::default();
    let (width, height, x, y) = match panel {
        Panel::Apps => {
            anchor = LayerShellAnchor::ALL;
            (w, h, 0., 0.)
        }
        Panel::Preferences => {
            let width = 640f32.min(w - 32.).max(280.);
            let height = 520f32.min(h - 48.).max(240.);
            (width, height, (w - width) / 2., (h - height) / 2.)
        }
        Panel::Screenshot => {
            let width = 390f32.min(w - 24.);
            let bottom = if state.prefs.dock_position == DockPosition::Bottom
                && !state.prefs.dock_hidden
                && !state.prefs.dock_auto_hide
            {
                state.prefs.dock_size as f32 + 56.
            } else {
                24.
            };
            anchor.bottom = true;
            margins.bottom = bottom.round() as i32;
            (width, 64., (w - width) / 2., h - bottom - 64.)
        }
        Panel::Dock(id) => {
            let apps = state.dock_apps();
            // Size the popup to its actual content so the last row sits flush
            // with the bottom instead of leaving an empty band or clipping a row.
            let width = DOCK_MENU_WIDTH.min(w - 24.);
            let height = dock_menu_window_height(state, id).min(available);
            let metrics = dock_metrics(state, &apps, w, h);
            let app_index = dock_display_apps(&apps, &metrics)
                .iter()
                .position(|app| &app.id == id)
                .unwrap_or(0);
            // The tile this menu belongs to, in screen coordinates. The Dock
            // surface is centered across the free axis and the launcher takes
            // slot 0, so the app at `app_index` sits at `app_index + launcher`.
            let (tile_x, tile_y) = dock_tile_center(
                state,
                &metrics,
                app_index + usize::from(state.prefs.show_launcher),
                w,
                h,
            );
            let edge = metrics.thickness + 18.;
            let (width, height, x, y, arrow) = match state.prefs.dock_position {
                DockPosition::Bottom => {
                    // The triangle points down at the tile from the popup bottom.
                    let x = (tile_x - width / 2.).clamp(8., (w - width - 8.).max(8.));
                    let offset = (tile_x - x).clamp(DOCK_ARROW, (width - DOCK_ARROW).max(DOCK_ARROW));
                    (
                        width,
                        height,
                        x,
                        h - edge - (height - DOCK_ARROW),
                        Some(Arrow {
                            edge: ArrowEdge::Bottom,
                            offset,
                        }),
                    )
                }
                DockPosition::Left | DockPosition::Right => {
                    // The triangle points sideways at the tile from the inner
                    // edge, so the width grows and the body keeps the content
                    // height.
                    let width = width + DOCK_ARROW;
                    let height = height - DOCK_ARROW;
                    let y = (tile_y - height / 2.)
                        .clamp(top, (h - height - 8.).max(top));
                    let offset = (tile_y - y).clamp(DOCK_ARROW, (height - DOCK_ARROW).max(DOCK_ARROW));
                    let (x, arrow) = if state.prefs.dock_position == DockPosition::Left {
                        (
                            edge - DOCK_ARROW,
                            Some(Arrow {
                                edge: ArrowEdge::Left,
                                offset,
                            }),
                        )
                    } else {
                        (
                            w - edge - width + DOCK_ARROW,
                            Some(Arrow {
                                edge: ArrowEdge::Right,
                                offset,
                            }),
                        )
                    };
                    (width, height, x, y, arrow)
                }
            };
            // The popup is drawn inside a fullscreen modal window so a click
            // anywhere outside it dismisses the menu.
            return Placement {
                bounds: Bounds::new(point(px(0.), px(0.)), size(px(w), px(h))),
                anchor: LayerShellAnchor::ALL,
                margins: LayerShellMargins::default(),
                normal: false,
                arrow,
                popup: Some(Bounds::new(point(px(x), px(y)), size(px(width), px(height)))),
            };
        }
        Panel::DockFolder { root } => {
            let layout = resolved_folder_layout(state, root, w, h);
            if layout == FolderLayout::Fan {
                return Placement {
                    bounds: Bounds::new(point(px(0.), px(0.)), size(px(w), px(h))),
                    anchor: LayerShellAnchor::ALL,
                    margins: LayerShellMargins::default(),
                    normal: false,
                    arrow: None,
                    popup: None,
                };
            }
            let apps = state.dock_apps();
            let metrics = dock_metrics(state, &apps, w, h);
            let (width, height) = folder_popup_size(state, root, layout, w, h);
            let (tile_x, tile_y) = folder_tile_center(state, root, w, h)
                .unwrap_or((w / 2., h / 2.));
            let (width, height, x, y, arrow) = dock_popup_geometry(
                state, &metrics, tile_x, tile_y, width, height, w, h, top,
            );
            return Placement {
                bounds: Bounds::new(point(px(0.), px(0.)), size(px(w), px(h))),
                anchor: LayerShellAnchor::ALL,
                margins: LayerShellMargins::default(),
                normal: false,
                arrow: Some(arrow),
                popup: Some(Bounds::new(point(px(x), px(y)), size(px(width), px(height)))),
            };
        }
        Panel::DockFolderMenu { path } => {
            let apps = state.dock_apps();
            let metrics = dock_metrics(state, &apps, w, h);
            let (tile_x, tile_y) = folder_tile_center(state, path, w, h)
                .unwrap_or((w / 2., h / 2.));
            let menu_height = folder_menu_window_height()
                .min(h * FOLDER_POPUP_MAX_SCREEN_SHARE)
                .min(available);
            let (width, height, x, y, arrow) = dock_popup_geometry(
                state,
                &metrics,
                tile_x,
                tile_y,
                FOLDER_MENU_WIDTH.min(w - 24.),
                menu_height,
                w,
                h,
                top,
            );
            return Placement {
                bounds: Bounds::new(point(px(0.), px(0.)), size(px(w), px(h))),
                anchor: LayerShellAnchor::ALL,
                margins: LayerShellMargins::default(),
                normal: false,
                arrow: Some(arrow),
                popup: Some(Bounds::new(point(px(x), px(y)), size(px(width), px(height)))),
            };
        }
        Panel::DockOverflow { ids, icon_size } => {
            let apps = state.dock_apps();
            let metrics = dock_metrics(state, &apps, w, h);
            // The drawer is a three-column grid that starts at three rows, so the
            // panel only grows when the overflow needs another row. The body adds
            // its own 10pt padding. Size from the fitted icon the Dock is drawing
            // so the tiles line up with the row instead of clipping.
            let (container_w, container_h) =
                crate::view::macos_dock::drawer_size(f32::from(*icon_size), ids.len());
            let width = (container_w + 20.).min(w - 24.);
            let height = (container_h + 20.).min(available);
            // The overflow tile sits right after the last visible application.
            let (tile_x, tile_y) = dock_tile_center(
                state,
                &metrics,
                metrics.visible_apps + usize::from(state.prefs.show_launcher),
                w,
                h,
            );
            let edge = metrics.thickness + 18.;
            let (width, height, x, y, arrow) = match state.prefs.dock_position {
                DockPosition::Bottom => {
                    let height = height + DOCK_ARROW;
                    let x = (tile_x - width / 2.).clamp(8., (w - width - 8.).max(8.));
                    let offset =
                        (tile_x - x).clamp(DOCK_ARROW, (width - DOCK_ARROW).max(DOCK_ARROW));
                    (
                        width,
                        height,
                        x,
                        h - edge - (height - DOCK_ARROW),
                        Some(Arrow {
                            edge: ArrowEdge::Bottom,
                            offset,
                        }),
                    )
                }
                DockPosition::Left | DockPosition::Right => {
                    let width = width + DOCK_ARROW;
                    let y = (tile_y - height / 2.)
                        .clamp(top, (h - height - 8.).max(top));
                    let offset =
                        (tile_y - y).clamp(DOCK_ARROW, (height - DOCK_ARROW).max(DOCK_ARROW));
                    let (x, arrow) = if state.prefs.dock_position == DockPosition::Left {
                        (
                            edge - DOCK_ARROW,
                            Some(Arrow {
                                edge: ArrowEdge::Left,
                                offset,
                            }),
                        )
                    } else {
                        (
                            w - edge - width + DOCK_ARROW,
                            Some(Arrow {
                                edge: ArrowEdge::Right,
                                offset,
                            }),
                        )
                    };
                    (width, height, x, y, arrow)
                }
            };
            return Placement {
                bounds: Bounds::new(point(px(0.), px(0.)), size(px(w), px(h))),
                anchor: LayerShellAnchor::ALL,
                margins: LayerShellMargins::default(),
                normal: false,
                arrow,
                popup: Some(Bounds::new(point(px(x), px(y)), size(px(width), px(height)))),
            };
        }
        _ => {
            let (width, height) = match panel {
                Panel::Controls => (320f32, 424f32.min(available)),
                Panel::Notifications => (368., available.min(1800.)),
                Panel::Recent => (400., 600f32.min(available)),
                Panel::Tray(_) => (
                    260.,
                    (menu_height(&state.tray_menu) + 20.).clamp(48., available),
                ),
                _ => unreachable!(),
            };
            let width = width.min(w - 24.);
            anchor.top = true;
            anchor.right = true;
            margins.top = top.round() as i32;
            margins.right = 12;
            (width, height, w - width - 12., top)
        }
    };
    Placement {
        bounds: Bounds::new(point(px(x), px(y)), size(px(width), px(height))),
        anchor,
        margins,
        normal: *panel == Panel::Preferences,
        arrow: None,
        popup: None,
    }
}

fn menu_height(items: &[services::MenuItem]) -> f32 {
    items
        .iter()
        .filter(|item| item.visible)
        .map(|item| {
            if item.separator {
                17.
            } else {
                40. + menu_height(&item.children)
            }
        })
        .sum::<f32>()
        .max(28.)
}

#[cfg(test)]
mod tests {
    use ::core::prelude::v1::test;

    use super::*;
    use crate::app::testing;

    #[gpui::test]
    fn desktop_components_have_distinct_positions_and_roles(cx: &mut TestAppContext) {
        let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
        let state = host.read_with(cx, |host, _| host.snapshot());
        let apps = placement(&state, &Panel::Apps, 1440., 900.);
        assert_eq!(
            apps.bounds,
            Bounds::new(point(px(0.), px(0.)), size(px(1440.), px(900.)))
        );
        assert_eq!(apps.anchor, LayerShellAnchor::ALL);
        assert!(!apps.normal);
        let controls = placement(&state, &Panel::Controls, 1440., 900.);
        assert_eq!(controls.bounds.origin.x, px(1108.));
        assert_eq!(controls.margins.top, state.prefs.bar_height as i32 + 8);
        assert!(controls.anchor.top && controls.anchor.right);
        let capture = placement(&state, &Panel::Screenshot, 1440., 900.);
        assert!(capture.anchor.bottom);
        assert_eq!(capture.bounds.size.height, px(64.));
        let prefs = placement(&state, &Panel::Preferences, 1440., 900.);
        assert!(prefs.normal);
        assert_eq!(prefs.bounds.size, size(px(640.), px(520.)));
        for output_height in [2160., 4320.] {
            let notifications = placement(&state, &Panel::Notifications, 3840., output_height);
            assert!(notifications.bounds.size.height <= px(1800.));
        }
        for panel in [
            Panel::Controls,
            Panel::Notifications,
            Panel::Recent,
            Panel::Screenshot,
            Panel::Preferences,
        ] {
            let geometry = placement(&state, &panel, 800., 600.);
            assert!(geometry.bounds.left() >= px(0.) && geometry.bounds.top() >= px(0.));
            assert!(geometry.bounds.right() <= px(800.) && geometry.bounds.bottom() <= px(600.));
        }
    }

    fn dock_window(id: u64, app_id: &str) -> clarion_ipc::Window {
        clarion_ipc::Window {
            id,
            title: Some(format!("window {id}")),
            app_id: Some(app_id.into()),
            pid: None,
            gtk_app_menu: None,
            app_menu: None,
            workspace_id: Some(1),
            is_focused: false,
            is_minimized: false,
            is_floating: false,
            is_urgent: false,
            focus_timestamp: None,
            layout: clarion_ipc::WindowLayout {
                pos_in_scrolling_layout: None,
                tile_size: (600., 400.),
                window_size: (600, 400),
                tile_pos_in_workspace_view: None,
                window_offset_in_tile: (0., 0.),
            },
        }
    }

    #[gpui::test]
    fn the_dock_menu_is_as_tall_as_its_rows_and_points_at_the_tile(cx: &mut TestAppContext) {
        let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
        let id = "beta";
        cx.update(|cx| {
            host.update(cx, |host, _| {
                host.prefs.pinned_app_ids = Some(vec![id.into()]);
                // Three windows: three focus rows, three action rows and a pin row.
                host.desktop.windows = (1..=3).map(|n| dock_window(n, id)).collect();
            });
        });
        let state = host.read_with(cx, |host, _| host.snapshot());
        let content = dock_menu_content_height(&state, id);
        let expect_rows = 3. * 2. + 1.;
        assert_eq!(
            content,
            2. * DOCK_MENU_PAD + expect_rows * DOCK_MENU_ROW + (expect_rows - 1.) * DOCK_MENU_GAP
        );
        // The window is the content plus the body's two-point border and the
        // triangle, so no row is clipped and no band is left empty.
        let window_height = dock_menu_window_height(&state, id);
        assert!((window_height - (content + DOCK_MENU_BORDER + DOCK_ARROW)).abs() < 0.01);
        for position in [
            DockPosition::Bottom,
            DockPosition::Left,
            DockPosition::Right,
        ] {
            let state = {
                let mut state = state.clone();
                state.prefs.dock_position = position;
                state
            };
            let placement = placement(&state, &Panel::Dock(id.into()), 1440., 900.);
            let arrow = placement.arrow.expect("a Dock menu points at its tile");
            // The window is a fullscreen modal so it can catch a click outside
            // the menu; the visible popup is reported separately.
            assert_eq!(
                f32::from(placement.bounds.size.width),
                1440.,
                "the modal window covers the output"
            );
            assert_eq!(f32::from(placement.bounds.size.height), 900.);
            let popup = placement.popup.expect("the visible popup is reported");
            let body = content + DOCK_MENU_BORDER;
            let menu_w = DOCK_MENU_WIDTH.min(1440. - 24.);
            // The popup grows by exactly the arrow on the dock-facing edge, so
            // the body fills the rest without an empty band. A horizontal Dock
            // puts the arrow on the bottom edge below the body; a vertical Dock
            // puts the arrow on the inner side beside the body.
            match position {
                DockPosition::Bottom => {
                    assert!((f32::from(popup.size.width) - menu_w).abs() < 0.01);
                    assert!(
                        (f32::from(popup.size.height) - (body + DOCK_ARROW)).abs() < 0.01,
                        "height={}",
                        f32::from(popup.size.height)
                    );
                }
                _ => {
                    assert!(
                        (f32::from(popup.size.width) - (menu_w + DOCK_ARROW)).abs() < 0.01,
                        "width={}",
                        f32::from(popup.size.width)
                    );
                    assert!((f32::from(popup.size.height) - body).abs() < 0.01);
                }
            }
            // The triangle sits on the edge that faces the Dock.
            let want = match position {
                DockPosition::Bottom => ArrowEdge::Bottom,
                DockPosition::Left => ArrowEdge::Left,
                DockPosition::Right => ArrowEdge::Right,
            };
            assert_eq!(arrow.edge, want);
            // The triangle stays within the popup so it is never clipped away.
            let span = match position {
                DockPosition::Bottom => f32::from(popup.size.width),
                _ => f32::from(popup.size.height),
            };
            assert!(arrow.offset >= DOCK_ARROW && arrow.offset <= span - DOCK_ARROW);
        }
    }

    #[gpui::test]
    fn the_folder_menu_never_covers_more_than_most_of_the_screen(cx: &mut TestAppContext) {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
        cx.update(|cx| {
            host.update(cx, |host, cx| {
                host.handle(crate::app::Action::PinFolder(root.clone()), cx)
            });
        });
        cx.run_until_parked();
        let state = host.read_with(cx, |host, _| host.snapshot());
        assert!(state.dock_folders.iter().any(|folder| folder.path == root));
        for height in [600., 720., 900., 1080., 1440.] {
            for position in [
                DockPosition::Bottom,
                DockPosition::Left,
                DockPosition::Right,
            ] {
                let mut state = state.clone();
                state.prefs.dock_position = position;
                let placement = placement(
                    &state,
                    &Panel::DockFolderMenu { path: root.clone() },
                    1440.,
                    height,
                );
                let popup = placement.popup.expect("the folder menu reports its visible rect");
                let popup_height = f32::from(popup.size.height);
                assert!(
                    popup_height <= height * FOLDER_POPUP_MAX_SCREEN_SHARE + 0.01,
                    "{position:?} at {height}: menu height {popup_height} exceeds the screen share"
                );
                assert!(popup_height > 0.);
            }
        }
    }

    #[gpui::test]
    fn the_folder_popup_never_covers_more_than_most_of_the_screen(
        cx: &mut TestAppContext,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let child = root.join("crowded");
        std::fs::create_dir(&child).unwrap();
        for index in 0..60 {
            std::fs::write(child.join(format!("item-{index}.txt")), b"x").unwrap();
        }
        let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
        cx.update(|cx| {
            host.update(cx, |host, cx| {
                host.handle(Action::PinFolder(root.clone()), cx);
                host.handle(Action::OpenFolder(root.clone()), cx);
            });
        });
        cx.run_until_parked();
        cx.update(|cx| {
            host.update(cx, |host, cx| {
                host.handle(Action::NavigateFolder(child.clone()), cx)
            });
        });
        cx.run_until_parked();
        let state = host.read_with(cx, |host, _| host.snapshot());
        assert!(state
            .folder_navigation
            .as_ref()
            .is_some_and(|navigation| navigation.current == child));
        for height in [600., 720., 900., 1080., 1440.] {
            for position in [
                DockPosition::Bottom,
                DockPosition::Left,
                DockPosition::Right,
            ] {
                let mut state = state.clone();
                state.prefs.dock_position = position;
                for layout in [FolderLayout::List, FolderLayout::Grid] {
                    if let Some(folder) = state
                        .dock_folders
                        .iter_mut()
                        .find(|folder| folder.path == root)
                    {
                        folder.layout = layout;
                    }
                    let placement = placement(
                        &state,
                        &Panel::DockFolder { root: root.clone() },
                        1440.,
                        height,
                    );
                    let popup = placement
                        .popup
                        .expect("the folder popup reports its visible rect");
                    let popup_height = f32::from(popup.size.height);
                    assert!(
                        popup_height <= height * FOLDER_POPUP_MAX_SCREEN_SHARE + 0.01,
                        "{position:?} {layout:?} at {height}: popup height {popup_height} exceeds the screen share"
                    );
                }
            }
        }
    }

    #[gpui::test]
    fn a_closed_pinned_app_offers_a_compact_menu(cx: &mut TestAppContext) {
        let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
        let id = "beta";
        cx.update(|cx| {
            host.update(cx, |host, _| {
                host.prefs.pinned_app_ids = Some(vec![id.into()]);
                host.desktop.windows = Vec::new();
            });
        });
        let state = host.read_with(cx, |host, _| host.snapshot());
        // A closed pinned app shows only the unpin row, so there is no empty
        // band below a single item.
        assert_eq!(
            dock_menu_content_height(&state, id),
            2. * DOCK_MENU_PAD + DOCK_MENU_ROW
        );
        let placement = placement(&state, &Panel::Dock(id.into()), 1440., 900.);
        assert!(placement.arrow.is_some());
    }

    #[gpui::test]
    fn folder_tile_centers_follow_the_dock_surface_origin(cx: &mut TestAppContext) {
        let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
        let state = host.read_with(cx, |host, _| host.snapshot());
        for position in [
            DockPosition::Bottom,
            DockPosition::Left,
            DockPosition::Right,
        ] {
            let mut state = state.clone();
            state.prefs.dock_position = position;
            let metrics = crate::dock_motion::DockMetrics::with_folders(
                2,
                48.,
                1.45,
                true,
                true,
                2,
                position != DockPosition::Bottom,
                1440.,
                900.,
            );
            for slot in [4, 5] {
                let (x, y) = dock_tile_center(&state, &metrics, slot, 1440., 900.);
                assert!(x.is_finite() && y.is_finite());
                assert!(x > 0. && x < 1440. && y > 0. && y < 900.);
            }
        }
    }

    #[gpui::test]
    fn folder_popups_stay_inside_the_output_and_point_at_the_folder(cx: &mut TestAppContext) {
        let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
        let state = host.read_with(cx, |host, _| host.snapshot());
        for position in [
            DockPosition::Bottom,
            DockPosition::Left,
            DockPosition::Right,
        ] {
            let mut state = state.clone();
            state.prefs.dock_position = position;
            for panel in [
                Panel::DockFolder {
                    root: PathBuf::from("/tmp/Projects"),
                },
                Panel::DockFolderMenu {
                    path: PathBuf::from("/tmp/Projects"),
                },
            ] {
                let placement = placement(&state, &panel, 1440., 900.);
                assert_eq!(f32::from(placement.bounds.size.width), 1440.);
                assert_eq!(f32::from(placement.bounds.size.height), 900.);
                let popup = placement.popup.expect("folder popup reports its rect");
                assert!(popup.origin.x >= px(0.) && popup.origin.y >= px(0.));
                assert!(popup.right() <= px(1440.) && popup.bottom() <= px(900.));
                assert!(placement.arrow.is_some());
            }
        }
    }

    #[gpui::test]
    fn a_vertical_dock_popup_points_at_the_icon_below_the_bar(cx: &mut TestAppContext) {
        let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
        let id = "beta";
        cx.update(|cx| {
            host.update(cx, |host, _| {
                host.prefs.pinned_app_ids = Some(vec![id.into()]);
                // A taller bar makes the compositor's usable-area offset
                // unmistakable: the Dock surface is centered below the bar.
                host.prefs.bar_height = 32.;
            });
        });
        let state = host.read_with(cx, |host, _| host.snapshot());
        let (w, h) = (1440., 900.);
        let overflow = Panel::DockOverflow {
            ids: vec![id.into()],
            icon_size: 48,
        };
        for position in [DockPosition::Left, DockPosition::Right] {
            let state = {
                let mut state = state.clone();
                state.prefs.dock_position = position;
                state
            };
            let apps = state.dock_apps();
            let metrics = dock_metrics(&state, &apps, w, h);
            let app_index = dock_display_apps(&apps, &metrics)
                .iter()
                .position(|app| app.id == id)
                .unwrap();
            // The layer-shell surface is not centered in the whole output: the
            // non-exclusive zone starts below the top bar, so the surface and
            // its icons sit half a bar lower. This is the geometry the arrow
            // must point at.
            let bar = state.prefs.bar_height as f32;
            let surface_y = bar + (h - bar - metrics.surface_height) / 2.;
            let idle = metrics.samples(None, 0.);
            let icon_y = |slot: usize| {
                surface_y
                    + idle
                        .get(slot)
                        .map(|sample| sample.origin_main + sample.size / 2.)
                        .unwrap_or(metrics.icon_size / 2.)
            };
            let launcher = usize::from(state.prefs.show_launcher);
            for (panel, slot) in [
                (Panel::Dock(id.into()), app_index + launcher),
                (overflow.clone(), metrics.visible_apps + launcher),
            ] {
                let placement = placement(&state, &panel, w, h);
                let popup = placement.popup.expect("the popup reports its visible rect");
                let arrow = placement.arrow.expect("the popup points at its tile");
                let tip_y = f32::from(popup.origin.y) + arrow.offset;
                let expected = icon_y(slot);
                assert!(
                    (tip_y - expected).abs() < 0.01,
                    "{position:?}: the arrow tip {tip_y} points at {expected}",
                );
            }
        }
    }

    #[gpui::test]
    fn the_overflow_drawer_points_at_the_overflow_tile(cx: &mut TestAppContext) {
        let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
        cx.update(|cx| {
            host.update(cx, |host, _| {
                host.prefs.pinned_app_ids = Some(vec!["a".into(), "b".into(), "c".into()]);
            });
        });
        let state = host.read_with(cx, |host, _| host.snapshot());
        let ids = vec!["a".into(), "b".into(), "c".into()];
        for position in [
            DockPosition::Bottom,
            DockPosition::Left,
            DockPosition::Right,
        ] {
            let state = {
                let mut state = state.clone();
                state.prefs.dock_position = position;
                state
            };
            let placement = placement(
                &state,
                &Panel::DockOverflow {
                    ids: ids.clone(),
                    icon_size: 48,
                },
                1440.,
                900.,
            );
            let arrow = placement.arrow.expect("the drawer points at its tile");
            assert_eq!(f32::from(placement.bounds.size.width), 1440.);
            assert_eq!(f32::from(placement.bounds.size.height), 900.);
            let popup = placement.popup.expect("the drawer reports its visible rect");
            let want = match position {
                DockPosition::Bottom => ArrowEdge::Bottom,
                DockPosition::Left => ArrowEdge::Left,
                DockPosition::Right => ArrowEdge::Right,
            };
            assert_eq!(arrow.edge, want);
            let span = match position {
                DockPosition::Bottom => f32::from(popup.size.width),
                _ => f32::from(popup.size.height),
            };
            assert!(arrow.offset >= DOCK_ARROW && arrow.offset <= span - DOCK_ARROW);
        }
    }
}
