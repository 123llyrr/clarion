// SPDX-License-Identifier: GPL-3.0-or-later
//! Each desktop component has its own placement and surface role.
use super::*;

pub(super) struct Placement {
    pub bounds: Bounds<Pixels>,
    pub anchor: LayerShellAnchor,
    pub margins: LayerShellMargins,
    pub normal: bool,
}

pub(super) fn placement(state: &ViewState, panel: &Panel, w: f32, h: f32) -> Placement {
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
            let app_index = apps.iter().position(|app| &app.id == id).unwrap_or(0);
            let app = apps.iter().find(|app| &app.id == id);
            let height = app
                .map(|app| 68. + app.windows.len().min(8) as f32 * 96.)
                .unwrap_or(100.)
                .min(available);
            let width = 250f32.min(w - 24.);
            let icon = state.prefs.dock_size as f32;
            let metrics = crate::dock_motion::DockMetrics::new(
                apps.len(),
                icon,
                state.prefs.magnification_scale as f32,
                state.prefs.magnification && !state.prefs.reduce_motion,
                state.prefs.show_launcher,
                state.prefs.dock_position != DockPosition::Bottom,
                w,
                h,
            );
            let step = metrics.item;
            let length = metrics.length;
            let position = 10.
                + (app_index + usize::from(state.prefs.show_launcher)) as f32 * step
                + step / 2.;
            match state.prefs.dock_position {
                DockPosition::Bottom => {
                    let x = ((w - length) / 2. + position - width / 2.)
                        .clamp(8., (w - width - 8.).max(8.));
                    let bottom = icon + 50.;
                    anchor.bottom = true;
                    anchor.left = true;
                    margins.left = x.round() as i32;
                    margins.bottom = bottom.round() as i32;
                    (width, height, x, h - bottom - height)
                }
                DockPosition::Left | DockPosition::Right => {
                    let y = ((h - length) / 2. + position - height / 2.)
                        .clamp(top, (h - height - 8.).max(top));
                    let edge = icon + 50.;
                    anchor.top = true;
                    margins.top = y.round() as i32;
                    let x = if state.prefs.dock_position == DockPosition::Left {
                        anchor.left = true;
                        margins.left = edge.round() as i32;
                        edge
                    } else {
                        anchor.right = true;
                        margins.right = edge.round() as i32;
                        w - edge - width
                    };
                    (width, height, x, y)
                }
            }
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
    use super::*;
    use crate::app::testing;
    use ::core::prelude::v1::test;

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
}
