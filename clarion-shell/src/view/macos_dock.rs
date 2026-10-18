// SPDX-License-Identifier: GPL-3.0-or-later
//! Floating macOS Dock. The shadow canvas is outside the tray's layout budget.
use super::*;

const SHADOW_INSET: f32 = 12.;
const TRAY_RADIUS: f32 = 20.;

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

fn dock_slot(id: impl Into<SharedString>, item: f32, dark: bool) -> Stateful<Div> {
    div()
        .id(id.into())
        .relative()
        .flex_shrink_0()
        .w(px(item))
        .h(px(item))
        .rounded(px(12.))
        .cursor_pointer()
        .hover(move |style| {
            style.bg(if dark {
                rgba(0xffffff0e)
            } else {
                rgba(0xffffff45)
            })
        })
}

fn dock_separator(vertical: bool, dark: bool) -> Div {
    div()
        .absolute()
        .when(vertical, |s| {
            s.top(px(-0.5)).left(px(8.)).right(px(8.)).h(px(1.))
        })
        .when(!vertical, |s| {
            s.left(px(-0.5)).top(px(8.)).bottom(px(8.)).w(px(1.))
        })
        .bg(if dark {
            rgba(0xffffff30)
        } else {
            rgba(0x1a1a2433)
        })
}

impl ShellView {
    pub(super) fn macos_dock(
        &self,
        host: &ViewState,
        p: Palette,
        window: &mut Window,
    ) -> AnyElement {
        let vertical = host.prefs.dock_position != DockPosition::Bottom;
        let dark = host.dark();
        let icon_size = host.prefs.dock_size as f32;
        let item = icon_size + 12.;
        let available = f32::from(if vertical {
            window.viewport_size().height
        } else {
            window.viewport_size().width
        }) - SHADOW_INSET * 2.;
        let visible =
            crate::app::dock_visible_count(available, icon_size, host.prefs.show_launcher);
        let mut content = div()
            .absolute()
            .left(px(10.))
            .top(px(10.))
            .right(px(10.))
            .bottom(px(10.))
            .flex()
            .when(vertical, |style| style.flex_col());

        if host.prefs.show_launcher {
            let h = self.host.clone();
            content = content.child(
                dock_slot("dock-launchpad", item, dark)
                    .child(div().absolute().left(px(6.)).top(px(3.5)).child(self.icon(
                        utility_icon("launchpad.png"),
                        icon_size,
                        p,
                    )))
                    .on_click(move |_, _, cx| {
                        h.update(cx, |h, cx| h.handle(Action::Panel(Panel::Apps), cx))
                    }),
            );
        }

        let mut recent_started = false;
        for app in host.dock_apps().into_iter().take(visible) {
            let h = self.host.clone();
            let activate = app.clone();
            let id = app.id.clone();
            let context = id.clone();
            let running = !app.windows.is_empty();
            // Running apps remain beside pinned apps. Only the genuine recent
            // tail forms a separate section; the line consumes no extra slot.
            let recent = host.prefs.show_recent_applications && !app.pinned && !running;
            let separate_recent = recent && !recent_started;
            recent_started |= recent;
            // Dock 2092.50.2's DOCKIndicatorLayer uses 3 px below tile size 33,
            // otherwise 4 px, with a radius of exactly half the diameter.
            let dot = if icon_size < 33. { 3. } else { 4. };
            content = content.child(
                dock_slot(SharedString::from(format!("dock-{id}")), item, dark)
                    .when(separate_recent, |slot| {
                        slot.child(dock_separator(vertical, dark))
                    })
                    .child(
                        div()
                            .absolute()
                            .left(px(6.))
                            .top(px(3.5))
                            .child(self.icon(app.icon, icon_size, p)),
                    )
                    .when(running && host.prefs.show_running_indicators, |slot| {
                        slot.child(
                            div()
                                .absolute()
                                .when(!vertical, |s| {
                                    s.left(px((item - dot) / 2.)).top(px(icon_size + 8.))
                                })
                                .when(vertical, |s| {
                                    s.top(px((item - dot) / 2.)).left(px(
                                        if host.prefs.dock_position == DockPosition::Left {
                                            0.
                                        } else {
                                            item - dot
                                        },
                                    ))
                                })
                                .size(px(dot))
                                .rounded_full()
                                .bg(if dark {
                                    rgba(0xf4f4f4e6)
                                } else {
                                    rgba(0x252529bf)
                                }),
                        )
                    })
                    .on_click(move |_, _, cx| h.update(cx, |h, cx| h.activate_app(&activate, cx)))
                    .on_mouse_down(MouseButton::Right, {
                        let h = self.host.clone();
                        move |_, _, cx| {
                            h.update(cx, |h, cx| {
                                h.handle(Action::Panel(Panel::Dock(context.clone())), cx)
                            })
                        }
                    }),
            );
        }

        let h = self.host.clone();
        content = content.child(
            dock_slot("dock-trash", item, dark)
                .child(dock_separator(vertical, dark))
                .child(div().absolute().left(px(6.)).top(px(3.5)).child(self.icon(
                    utility_icon(if host.system.trash_full {
                        "trash-full.png"
                    } else {
                        "trash-empty.png"
                    }),
                    icon_size,
                    p,
                )))
                .on_click(move |_, _, cx| {
                    h.update(cx, |h, cx| {
                        h.handle(
                            Action::Core(clarion_ipc::Action::Spawn {
                                command: vec!["gio".into(), "open".into(), "trash:///".into()],
                            }),
                            cx,
                        )
                    })
                }),
        );

        let tray = div()
            .absolute()
            .when(!vertical, |s| {
                s.left(px(SHADOW_INSET))
                    .right(px(SHADOW_INSET))
                    .top(px(SHADOW_INSET))
                    .bottom(px(0.))
            })
            .when(vertical, |s| {
                s.top(px(SHADOW_INSET))
                    .bottom(px(SHADOW_INSET))
                    .when(host.prefs.dock_position == DockPosition::Left, |s| {
                        s.left(px(0.)).right(px(SHADOW_INSET))
                    })
                    .when(host.prefs.dock_position == DockPosition::Right, |s| {
                        s.left(px(SHADOW_INSET)).right(px(0.))
                    })
            })
            .rounded(px(TRAY_RADIUS))
            .bg(if dark {
                rgba(0x29292fa3)
            } else {
                rgba(0xf2f2f2a8)
            })
            .shadow(vec![
                BoxShadow {
                    color: rgba(0x00000024).into(),
                    offset: point(px(0.), px(2.)),
                    blur_radius: px(8.),
                    spread_radius: px(0.),
                },
                BoxShadow {
                    color: rgba(0x00000014).into(),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(2.),
                    spread_radius: px(0.),
                },
            ])
            // The border is a separate layer so it never changes icon geometry.
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded(px(TRAY_RADIUS))
                    .border_1()
                    .border_color(if dark {
                        rgba(0xffffff3b)
                    } else {
                        rgba(0xffffffb3)
                    })
                    .bg(linear_gradient(
                        180.,
                        linear_color_stop(
                            if dark {
                                rgba(0xffffff0c)
                            } else {
                                rgba(0xffffff2b)
                            },
                            0.,
                        ),
                        linear_color_stop(rgba(0xffffff00), 1.),
                    )),
            )
            .child(content);

        let h = self.host.clone();
        div()
            .id("dock-surface")
            .relative()
            .size_full()
            .on_hover(move |hovered, _, cx| {
                let h = h.clone();
                let hovered = *hovered;
                cx.defer(move |cx| h.update(cx, |h, cx| h.handle(Action::DockHover(hovered), cx)));
            })
            .child(tray)
            .into_any_element()
    }
}
