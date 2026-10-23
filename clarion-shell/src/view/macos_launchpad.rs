// SPDX-License-Identifier: GPL-3.0-or-later
//! Full-output Launchpad, sharing the existing search and activation model.
use super::*;

/// Keep the application grid clear of the real Dock and its edge trigger.
/// Auto-hide still reserves space because its edge trigger may reveal the Dock.
pub(super) fn launchpad_insets(host: &ViewState) -> (f32, f32, f32) {
    if host.prefs.dock_hidden {
        return (0., 0., 0.);
    }
    let strip = (host.prefs.dock_size as f32 + 64.).max(112.);
    match host.prefs.dock_position {
        DockPosition::Bottom => (0., 0., strip),
        DockPosition::Left => (strip, 0., 0.),
        DockPosition::Right => (0., strip, 0.),
    }
}

/// The resting tray rectangle in output coordinates, the shape the Launchpad
/// leaves undimmed. Mirrors the Dock canvas so the cutout matches the tray
/// instead of the larger layer-shell surface.
pub(super) fn dock_cutout(host: &ViewState, width: f32, height: f32) -> Option<(f32, f32, f32, f32)> {
    if host.prefs.dock_hidden {
        return None;
    }
    let vertical = host.prefs.dock_position != DockPosition::Bottom;
    let metrics = crate::dock_motion::DockMetrics::with_folders(
        host.dock_apps().len(),
        host.prefs.dock_size as f32,
        host.prefs.magnification_scale as f32,
        host.prefs.magnification && !host.prefs.reduce_motion,
        host.prefs.show_launcher,
        host.dock_folders.len(),
        vertical,
        width,
        height,
    );
    let (x, y) = crate::app::panels::dock_surface_origin(host, &metrics, width, height);
    if vertical {
        let tray_x = if host.prefs.dock_position == DockPosition::Left {
            x
        } else {
            x + metrics.surface_width - metrics.thickness
        };
        Some((
            tray_x,
            y + metrics.main_padding,
            metrics.thickness,
            metrics.length,
        ))
    } else {
        Some((
            x + metrics.main_padding,
            y + metrics.cross_padding,
            metrics.length,
            metrics.thickness,
        ))
    }
}

/// Blur the whole backdrop, leaving a hole only for the actual Dock canvas.
/// A full-width clear strip would leave a conspicuous seam in the wallpaper.
pub(super) fn backdrop_regions(host: &ViewState, width: f32, height: f32) -> Vec<Bounds<Pixels>> {
    // Punch the hole around the resting tray, not the whole surface. The
    // surface reserves a transparent band for the hover tooltip, shadow and
    // magnification; dimming around that band would show a rectangle.
    let (tx, ty, tw, th) = match dock_cutout(host, width, height) {
        Some(cutout) => cutout,
        None => {
            return vec![Bounds::new(
                point(px(0.), px(0.)),
                size(px(width), px(height)),
            )];
        }
    };
    let margin = 10.;
    let left = (tx - margin).clamp(0., width);
    let right = (tx + tw + margin).clamp(left, width);
    let top = (ty - margin).clamp(0., height);
    let bottom = (ty + th + margin).clamp(top, height);
    [
        (0., 0., width, top),
        (0., bottom, width, height - bottom),
        (0., top, left, bottom - top),
        (right, top, width - right, bottom - top),
    ]
    .into_iter()
    .filter(|(_, _, w, h)| *w > 0. && *h > 0.)
    .map(|(x, y, w, h)| Bounds::new(point(px(x), px(y)), size(px(w), px(h))))
    .collect()
}

fn grid_metrics(host: &ViewState, window: &Window) -> (f32, usize) {
    let (left, right, _) = launchpad_insets(host);
    let available = (f32::from(window.viewport_size().width) - left - right).max(1.);
    let width = (available - 64.).clamp(1., 1050.);
    let columns = ((width / 126.).floor() as usize).clamp(1, 7);
    (width, columns)
}

impl ShellView {
    /// Called only after a keyboard selection change, never during rendering:
    /// mouse/trackpad scrolling must not snap back to the current selection.
    pub(super) fn keyboard_selection_into_view(&self, host: &ViewState, window: &Window) {
        let count = host.catalog.search(&self.query).len();
        if count == 0 {
            return;
        }
        let (_, columns) = grid_metrics(host, window);
        let query = self.query.to_lowercase();
        let window_group = !query.is_empty()
            && host.desktop.windows.iter().any(|window| {
                window
                    .title
                    .as_deref()
                    .unwrap_or_default()
                    .to_lowercase()
                    .contains(&query)
            });
        self.scroll
            .scroll_to_item(self.selection.min(count - 1) / columns + usize::from(window_group));
    }

    pub(super) fn macos_launchpad(
        &self,
        host: &ViewState,
        p: Palette,
        window: &Window,
        cx: &Context<Self>,
    ) -> AnyElement {
        let (left, right, bottom) = launchpad_insets(host);
        let (grid_width, columns) = grid_metrics(host, window);
        let cell_width = grid_width / columns as f32;
        let icon_size = if grid_width >= 936. { 72. } else { 64. };
        let white: Hsla = rgb(0xffffff).into();
        let found = host.catalog.search(&self.query);
        let query = self.query.to_lowercase();
        let mut results = div()
            .id("launchpad-scroll")
            .flex_1()
            .min_h(px(0.))
            .w(px(grid_width))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(22.))
            .pb(px(16.))
            .overflow_y_scroll()
            .track_scroll(&self.scroll);
        let mut has_window_matches = false;

        // Window matches do not consume app indexes: Enter and the existing
        // keyboard selection still address catalog.search(query) exactly.
        if !query.is_empty() {
            let mut windows = div()
                .w(px(grid_width.min(680.)))
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
                .take(4)
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
                results = results.child(windows);
            }
        }

        for (row_index, apps) in found.chunks(columns).enumerate() {
            let mut row = div().w(px(grid_width)).flex_shrink_0().flex();
            for (column, app) in apps.iter().enumerate() {
                let index = row_index * columns + column;
                let selected = index == self.selection;
                let id = app.id.clone();
                let context = id.clone();
                let h = self.host.clone();
                row = row.child(
                    div()
                        .id(SharedString::from(format!("launchpad-app-{id}")))
                        .w(px(cell_width))
                        .flex_shrink_0()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(9.))
                        .py(px(8.))
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .id(SharedString::from(format!("launchpad-icon-{context}")))
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
                                .w(px(cell_width - 10.))
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
                            let h = self.host.clone();
                            move |_, _, cx| {
                                cx.stop_propagation();
                                h.update(cx, |h, cx| {
                                    h.handle(Action::Panel(Panel::Dock(context.clone())), cx)
                                });
                            }
                        }),
                );
            }
            results = results.child(row);
        }

        if found.is_empty() && !has_window_matches {
            results = results.child(
                div()
                    .mt(px(36.))
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

        let h = self.host.clone();
        let content = div()
            .id("macos-launchpad")
            .absolute()
            .left(px(left))
            .right(px(right))
            .top(px(0.))
            .bottom(px(bottom))
            .flex()
            .flex_col()
            .items_center()
            .pt(px(38.))
            .gap(px(44.))
            .text_color(white)
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                h.update(cx, |h, cx| h.handle(Action::Close, cx));
            })
            .child(
                div()
                    .id("launchpad-search")
                    .w(px(280_f32.min(grid_width)))
                    .h(px(28.))
                    .flex_shrink_0()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(self.input.clone()),
            )
            .child(results);
        let width = f32::from(window.viewport_size().width);
        let height = f32::from(window.viewport_size().height);
        let mut surface = div().relative().size_full();
        for (index, region) in backdrop_regions(host, width, height)
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
