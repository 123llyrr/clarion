// SPDX-License-Identifier: GPL-3.0-or-later
//! Full-output Launchpad, sharing the existing search and activation model.
//!
//! The grid spends the screen on columns rather than rows: one page is
//! `columns × rows` tiles, the pages run sideways (edge arrows and a dot row
//! say where you are) and the search line is a wide capsule with its own
//! magnifier, the shape the reference launcher uses on a full output.
use super::*;

/// The fullscreen launcher owns the whole output. The Dock surface hides
/// while this panel is open, so the backdrop is one unbroken scrim and there
/// is no cutout to align with the tray or its animation band.
pub(super) fn backdrop_regions(width: f32, height: f32) -> Vec<Bounds<Pixels>> {
    vec![Bounds::new(
        point(px(0.), px(0.)),
        size(px(width), px(height)),
    )]
}

/// Few rows, many columns: one page of the fullscreen launcher.
const LAUNCHPAD_MIN_COLUMNS: usize = 4;
const LAUNCHPAD_MAX_COLUMNS: usize = 14;
const LAUNCHPAD_MIN_ROWS: usize = 1;
const LAUNCHPAD_MAX_ROWS: usize = 6;
/// The output is split by proportion, not by fixed pixel chrome: the search
/// capsule owns the top quarter and the Dock keeps the bottom fifth free.
const LAUNCHPAD_SEARCH_SHARE: f32 = 0.25;
const LAUNCHPAD_DOCK_SHARE: f32 = 0.20;
/// Tile height and the steady gutter between rows. The grid keeps this pitch
/// instead of stretching spare height into the gaps.
const LAUNCHPAD_ROW_H: f32 = 154.;
const LAUNCHPAD_ROW_GAP: f32 = 20.;
const LAUNCHPAD_ROW_PITCH: f32 = LAUNCHPAD_ROW_H + LAUNCHPAD_ROW_GAP;
/// The search capsule and the page dots that close the middle band.
const LAUNCHPAD_CAPSULE_H: f32 = 44.;
const LAUNCHPAD_DOTS_H: f32 = 10.;
const LAUNCHPAD_DOTS_BOTTOM: f32 = 14.;
const LAUNCHPAD_DOTS_GAP: f32 = 36.;
/// One fixed column pitch keeps a full-size icon plate and its breathing room
/// whether the page is full or only has a few tiles.
const LAUNCHPAD_COLUMN_PITCH: f32 = 130.;
/// The arrows sit in a band twice as wide as their old margin; the button is
/// centred in it so both the edge and the grid keep room.
const LAUNCHPAD_ARROW_BAND: f32 = 200.;
const LAUNCHPAD_ARROW_SIZE: f32 = 46.;

struct LaunchpadMetrics {
    /// The width the grid's fixed columns occupy.
    width: f32,
    columns: usize,
    rows: usize,
    /// One fixed column pitch, never stretched to use spare width.
    cell_width: f32,
    /// The reserved paging band on each side of the grid.
    side: f32,
    /// The top and natural height of the grid inside the middle band.
    grid_top: f32,
    grid_height: f32,
    /// The top of the page dot row, right below the grid.
    dots_top: f32,
    /// The top of the reserved Dock band. Geometry tests read it; the
    /// renderer stops the content above it by construction.
    #[allow(dead_code)]
    dock_top: f32,
}

impl LaunchpadMetrics {
    fn per_page(&self) -> usize {
        self.columns * self.rows
    }

    fn arrow_inset(&self) -> f32 {
        ((self.side - LAUNCHPAD_ARROW_SIZE) / 2.).max(0.)
    }
}

/// The grid shape for the room the output leaves once the three proportional
/// bands and the fixed chrome inside the middle band are taken off. Columns
/// keep a fixed pitch and start at the left edge of the paging band.
fn launchpad_metrics(width: f32, height: f32) -> LaunchpadMetrics {
    let grid_top = height * LAUNCHPAD_SEARCH_SHARE;
    let dock_top = height * (1. - LAUNCHPAD_DOCK_SHARE);
    let band_bottom = (dock_top - LAUNCHPAD_DOTS_BOTTOM - LAUNCHPAD_DOTS_H).max(grid_top);
    let available = (band_bottom - grid_top - LAUNCHPAD_DOTS_GAP).max(0.);
    let min_grid = LAUNCHPAD_MIN_COLUMNS as f32 * LAUNCHPAD_COLUMN_PITCH;
    let side = LAUNCHPAD_ARROW_BAND
        .min(((width - min_grid) / 2.).max(LAUNCHPAD_ARROW_SIZE / 2.));
    let available_width = (width - 2. * side).max(LAUNCHPAD_COLUMN_PITCH);
    let columns = ((available_width / LAUNCHPAD_COLUMN_PITCH).floor() as usize)
        .clamp(LAUNCHPAD_MIN_COLUMNS, LAUNCHPAD_MAX_COLUMNS);
    let rows = ((available + LAUNCHPAD_ROW_GAP) / LAUNCHPAD_ROW_PITCH).floor() as usize;
    let rows = rows.clamp(LAUNCHPAD_MIN_ROWS, LAUNCHPAD_MAX_ROWS);
    let grid_height =
        rows as f32 * LAUNCHPAD_ROW_H + (rows.saturating_sub(1)) as f32 * LAUNCHPAD_ROW_GAP;
    let dots_top = grid_top + grid_height + LAUNCHPAD_DOTS_GAP;
    LaunchpadMetrics {
        width: columns as f32 * LAUNCHPAD_COLUMN_PITCH,
        columns,
        rows,
        cell_width: LAUNCHPAD_COLUMN_PITCH,
        side,
        grid_top,
        grid_height,
        dots_top,
        dock_top,
    }
}

fn grid_metrics(window: &Window) -> LaunchpadMetrics {
    let viewport = window.viewport_size();
    launchpad_metrics(f32::from(viewport.width), f32::from(viewport.height))
}

/// The current page, the page count and the page size for this output size.
/// The tests also use it to know how a wide catalog pages; the renderer reads
/// the same numbers instead of recomputing them from its own copy.
pub(super) fn launchpad_page_count(
    window: &Window,
    count: usize,
    selection: usize,
) -> (usize, usize, usize) {
    let per_page = grid_metrics(window).per_page();
    let (page, pages) = launchpad_page(selection, count, per_page);
    (page, pages, per_page)
}

/// The page the selection sits on, clamped into the real page count.
fn launchpad_page(selection: usize, count: usize, per_page: usize) -> (usize, usize) {
    let per_page = per_page.max(1);
    let pages = count.div_ceil(per_page).max(1);
    ((selection / per_page).min(pages - 1), pages)
}

fn launchpad_icon(path: &Path, size: f32, color: Hsla) -> AnyElement {
    if path.is_file() {
        svg()
            .path(path.to_string_lossy().into_owned())
            .size(px(size))
            .text_color(color)
            .into_any_element()
    } else {
        div()
            .size(px(size))
            .text_center()
            .child("◇")
            .into_any_element()
    }
}

impl ShellView {
    /// One paging step. The page is derived from the selection, so stepping
    /// also lands the keyboard on the first tile of the target page.
    pub(super) fn launchpad_page(
        &mut self,
        forward: bool,
        host: &ViewState,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let metrics = grid_metrics(window);
        let count = host.catalog.search(&self.query).len();
        let (page, pages) = launchpad_page(self.selection, count, metrics.per_page());
        let next = if forward {
            (page + 1).min(pages - 1)
        } else {
            page.saturating_sub(1)
        };
        self.launchpad_go_to_page(next, host, window, cx);
    }

    /// Jump to a page by index (the dot row).
    pub(super) fn launchpad_go_to_page(
        &mut self,
        page: usize,
        host: &ViewState,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let metrics = grid_metrics(window);
        let count = host.catalog.search(&self.query).len();
        if count == 0 {
            return;
        }
        let per_page = metrics.per_page();
        let (_, pages) = launchpad_page(self.selection, count, per_page);
        let page = page.min(pages - 1);
        let target = (page * per_page).min(count - 1);
        if target != self.selection {
            self.selection = target;
            cx.notify();
        }
    }

    fn launchpad_arrow(
        &self,
        forward: bool,
        enabled: bool,
        host: &ViewState,
        cx: &Context<Self>,
    ) -> AnyElement {
        let white: Hsla = rgb(0xffffff).into();
        let path = if forward {
            self.themed_glyph_path("go-next-symbolic", "chevron-right.svg")
        } else {
            self.themed_glyph_path("go-previous-symbolic", "chevron-left.svg")
        };
        let selector = if forward {
            "launchpad-next"
        } else {
            "launchpad-prev"
        };
        let view = cx.entity().downgrade();
        let state = host.clone();
        div()
            .id(SharedString::from(selector))
            .debug_selector(move || selector.to_owned())
            .size(px(LAUNCHPAD_ARROW_SIZE))
            .rounded(px(LAUNCHPAD_ARROW_SIZE / 2.))
            .bg(rgba(0xffffff1f))
            .flex()
            .items_center()
            .justify_center()
            .when(!enabled, |arrow| arrow.opacity(0.35))
            .when(enabled, |arrow| {
                arrow
                    .cursor_pointer()
                    .hover(|style| style.bg(rgba(0xffffff33)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        let state = state.clone();
                        let win = &*window;
                        let _ = view.update(cx, |view, cx| {
                            view.launchpad_page(forward, &state, win, cx)
                        });
                    })
            })
            .child(launchpad_icon(&path, 20., white))
            .into_any_element()
    }

    fn launchpad_dots(
        &self,
        page: usize,
        pages: usize,
        host: &ViewState,
        cx: &Context<Self>,
    ) -> AnyElement {
        let white: Hsla = rgb(0xffffff).into();
        let mut dots = div()
            .h(px(LAUNCHPAD_DOTS_H))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(8.));
        let view = cx.entity().downgrade();
        let state = host.clone();
        for index in 0..pages {
            let active = index == page;
            let selector = format!("launchpad-page-{index}");
            let view = view.clone();
            let state = state.clone();
            dots = dots.child(
                div()
                    .id(SharedString::from(selector.clone()))
                    .debug_selector(move || selector.clone())
                    .size(px(8.))
                    .rounded(px(4.))
                    .bg(if active {
                        white.opacity(0.95)
                    } else {
                        white.opacity(0.32)
                    })
                    .cursor_pointer()
                    .hover(|dot| dot.bg(white.opacity(0.6)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        let win = &*window;
                        let _ = view.update(cx, |view, cx| {
                            view.launchpad_go_to_page(index, &state, win, cx)
                        });
                    }),
            );
        }
        dots.into_any_element()
    }

    pub(super) fn macos_launchpad(
        &self,
        host: &ViewState,
        p: Palette,
        window: &Window,
        cx: &Context<Self>,
    ) -> AnyElement {
        let metrics = grid_metrics(window);
        let white: Hsla = rgb(0xffffff).into();
        let found = host.catalog.search(&self.query);
        let (page, pages, per_page) =
            launchpad_page_count(window, found.len(), self.selection);
        let start = page * per_page;
        let end = (start + per_page).min(found.len());
        let slice = &found[start.min(found.len())..end];
        let query = self.query.to_lowercase();
        let icon_size = if f32::from(window.viewport_size().width) >= 900. {
            86.
        } else {
            77.
        };

        let mut results = div()
            .w_full()
            .h_full()
            .flex()
            .flex_col()
            .items_start();
        let mut has_window_matches = false;

        // Window matches do not consume app indexes: Enter and the existing
        // keyboard selection still address catalog.search(query) exactly.
        if !query.is_empty() {
            let mut windows = div()
                .w(px(metrics.width.min(680.)))
                .flex_shrink_0()
                .flex()
                .flex_col()
                .gap(px(6.));
            for window in host
                .desktop
                .windows
                .iter()
                .filter(|window| {
                    window
                        .title
                        .as_deref()
                        .unwrap_or_default()
                        .to_lowercase()
                        .contains(&query)
                })
                .take(2)
            {
                has_window_matches = true;
                let h = self.host.clone();
                let id = window.id;
                windows = windows.child(
                    div()
                        .id(SharedString::from(format!("launchpad-window-{id}")))
                        .px(px(14.))
                        .py(px(10.))
                        .rounded(px(10.))
                        .bg(rgba(0xffffff18))
                        .text_size(px(13.))
                        .text_color(white)
                        .overflow_hidden()
                        .text_ellipsis()
                        .cursor_pointer()
                        .hover(|s| s.bg(rgba(0xffffff29)))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(menu_text(
                            format!(
                                "切换窗口 · {}",
                                window.title.as_deref().unwrap_or("未命名窗口")
                            ),
                            FontWeight::NORMAL,
                            white,
                            cx,
                        ))
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            h.update(cx, |h, cx| h.handle(Action::Focus(id), cx));
                        }),
                );
            }
            if has_window_matches {
                results = results.child(windows.mb(px(18.)));
            }
        }

        // One page of rows kept at their natural height. The spare middle
        // band stays empty below; stretching it would turn a short page into
        // a few icons with huge, uneven holes between them.
        let mut rows = div()
            .w(px(metrics.width))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(LAUNCHPAD_ROW_GAP));
        for (row_index, apps) in slice.chunks(metrics.columns).enumerate() {
            let mut row = div().w(px(metrics.width)).flex_shrink_0().flex();
            for (column, app) in apps.iter().enumerate() {
                let index = start + row_index * metrics.columns + column;
                let selected = index == self.selection;
                let id = app.id.clone();
                let context = id.clone();
                let h = self.host.clone();
                let selector = format!("launchpad-app-{id}");
                let icon_selector = format!("launchpad-icon-{context}");
                row = row.child(
                    div()
                        .id(SharedString::from(selector.clone()))
                        .debug_selector(move || selector.clone())
                        .w(px(metrics.cell_width))
                        .h(px(LAUNCHPAD_ROW_H))
                        .flex_shrink_0()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(9.))
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .id(SharedString::from(format!("launchpad-icon-{context}")))
                                .debug_selector(move || icon_selector.clone())
                                .size(px(icon_size + 12.))
                                .flex_shrink_0()
                                .rounded(px(18.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(if selected {
                                    rgba(0xffffff1c)
                                } else {
                                    rgba(0xffffff00)
                                })
                                .hover(|s| s.bg(rgba(0xffffff26)))
                                .child(self.icon(app.icon_path.clone(), icon_size, p)),
                        )
                        .child(
                            div()
                                .w(px(metrics.cell_width - 10.))
                                .text_size(px(13.))
                                .line_height(px(18.))
                                .text_color(white)
                                .text_center()
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(menu_text(app.name.clone(), FontWeight::NORMAL, white, cx)),
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
                                let _ =
                                    view.update(cx, |view, cx| view.invoke(action, cx));
                            }
                        }),
                );
            }
            rows = rows.child(row);
        }
        if !slice.is_empty() {
            results = results.child(rows);
        }

        if found.is_empty() && !has_window_matches {
            results = results.child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(15.))
                    .text_color(white.opacity(0.8))
                    .child(menu_text(
                        "没有找到应用".into(),
                        FontWeight::NORMAL,
                        white.opacity(0.8),
                        cx,
                    )),
            );
        }

        // The page area owns the arrows and fills the middle band. The grid
        // itself starts at the left edge of the paging band and keeps its
        // natural height; the arrows sit centred in the wider side bands.
        let mut page_area = div()
            .absolute()
            .left(px(0.))
            .right(px(0.))
            .top(px(metrics.grid_top))
            .h(px(metrics.grid_height));
        let grid_area = div()
            .absolute()
            .left(px(metrics.side))
            .top(px(0.))
            .w(px(metrics.width))
            .h_full()
            .flex()
            .flex_col()
            .items_start()
            .child(results);
        page_area = page_area.child(grid_area);
        if pages > 1 {
            let arrow = |forward: bool, enabled: bool, cx: &Context<Self>| {
                let mut layer = div()
                    .absolute()
                    .top(px(0.))
                    .bottom(px(0.))
                    .flex()
                    .items_center();
                layer = if forward {
                    layer.right(px(metrics.arrow_inset()))
                } else {
                    layer.left(px(metrics.arrow_inset()))
                };
                layer.child(self.launchpad_arrow(forward, enabled, host, cx))
            };
            page_area = page_area
                .child(arrow(false, page > 0, cx))
                .child(arrow(true, page + 1 < pages, cx));
        }

        let search_icon = self.themed_glyph_path("system-search-symbolic", "search.svg");
        let search = div()
            .id("launchpad-search")
            .debug_selector(|| "launchpad-search".to_owned())
            .w(px(metrics.width.min(620.)))
            .h(px(LAUNCHPAD_CAPSULE_H))
            .flex_shrink_0()
            .rounded(px(LAUNCHPAD_CAPSULE_H / 2.))
            .bg(rgba(0xffffff1f))
            .flex()
            .items_center()
            .gap(px(10.))
            .pl(px(18.))
            .pr(px(16.))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(launchpad_icon(&search_icon, 18., white.opacity(0.75)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .child(self.input.clone()),
            );

        let dots = if pages > 1 {
            self.launchpad_dots(page, pages, host, cx)
        } else {
            div().h(px(LAUNCHPAD_DOTS_H)).flex_shrink_0().into_any_element()
        };

        let search_zone = div()
            .absolute()
            .left(px(0.))
            .right(px(0.))
            .top(px(0.))
            .h(px(metrics.grid_top))
            .flex()
            .items_center()
            .justify_center()
            .child(search);

        let dots_layer = div()
            .absolute()
            .left(px(0.))
            .right(px(0.))
            .top(px(metrics.dots_top))
            .h(px(LAUNCHPAD_DOTS_H))
            .flex()
            .items_center()
            .justify_center()
            .child(dots);

        let h = self.host.clone();
        let content = div()
            .id("macos-launchpad")
            .absolute()
            .inset_0()
            .text_color(white)
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                h.update(cx, |h, cx| h.handle(Action::Close, cx));
            })
            .child(search_zone)
            .child(page_area)
            .child(dots_layer);
        let width = f32::from(window.viewport_size().width);
        let height = f32::from(window.viewport_size().height);
        let mut surface = div().relative().size_full();
        for (index, region) in backdrop_regions(width, height)
            .into_iter()
            .enumerate()
        {
            let h = self.host.clone();
            surface = surface.child(
                div()
                    .id(SharedString::from(format!("launchpad-backdrop-{index}")))
                    .absolute()
                    .left(region.origin.x)
                    .top(region.origin.y)
                    .w(region.size.width)
                    .h(region.size.height)
                    .bg(p.scrim)
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        h.update(cx, |h, cx| h.handle(Action::Close, cx))
                    }),
            );
        }
        surface.child(content).into_any_element()
    }
}

#[cfg(test)]
#[path = "launchpad_tests.rs"]
mod tests;
