// SPDX-License-Identifier: GPL-3.0-or-later
//! Floating macOS Dock. The shadow canvas is outside the tray's layout budget.
use super::*;

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

/// Local to the Dock view: pointer packets never rebuild the top bar, catalog,
/// or other outputs. The next render consumes the most recent packet once.
pub(super) struct DockViewMotion {
    pub(super) epoch: std::time::Instant,
    pub(super) hovered: bool,
    pub(super) pointer_target: Option<f32>,
    pointer: crate::dock_motion::Motion,
    strength: crate::dock_motion::Motion,
    visibility: crate::dock_motion::Motion,
    #[cfg(test)]
    pub(super) input_regions: Vec<Bounds<Pixels>>,
    #[cfg(test)]
    pub(super) animation_active: bool,
}
impl DockViewMotion {
    pub(super) fn new(revealed: bool) -> Self {
        Self {
            #[cfg(test)]
            input_regions: Vec::new(),
            #[cfg(test)]
            animation_active: false,
            epoch: std::time::Instant::now(),
            hovered: false,
            pointer_target: None,
            pointer: crate::dock_motion::Motion::with_frequency(0., 40.),
            strength: crate::dock_motion::Motion::new(0.),
            visibility: crate::dock_motion::Motion::new(if revealed { 1. } else { 0. }),
        }
    }
}

impl ShellView {
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
        let metrics = DockMetrics::new(
            host.dock_apps().len(),
            host.prefs.dock_size as f32,
            host.prefs.magnification_scale as f32,
            host.prefs.magnification && !host.prefs.reduce_motion,
            host.prefs.show_launcher,
            vertical,
            width,
            height,
        );
        let now = self.dock_motion.epoch.elapsed();
        let reveal = if host.dock_revealed { 1. } else { 0. };
        let strength = if self.dock_motion.hovered
            && self.dock_motion.pointer_target.is_some()
            && host.prefs.magnification
            && host.dock_revealed
            && !host.prefs.reduce_motion
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
        let mut active =
            visibility.active || strength.active || (strength.value > 0.001 && pointer.active);
        let mut regions = rounded_region(tray_w, tray_h, TRAY_RADIUS);
        for region in &mut regions {
            region.origin.x += px(tray_x);
            region.origin.y += px(tray_y);
        }
        let mut content = div().absolute().inset_0();
        let apps = host.dock_apps();
        let mut recent_started = false;
        for (index, slot) in slots.iter().enumerate() {
            let launcher = host.prefs.show_launcher && index == 0;
            let app_index = index.saturating_sub(usize::from(host.prefs.show_launcher));
            let app = (!launcher && app_index < metrics.visible_apps).then(|| &apps[app_index]);
            let trash = !launcher && app.is_none();
            let id = if launcher {
                "dock-launchpad".to_owned()
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
            let (x, y) = if !vertical {
                (
                    slot.origin_main,
                    metrics.cross_padding + 13.5 - slot.lift - hop + slide,
                )
            } else if left {
                (16. + hop - slide, slot.origin_main)
            } else {
                (
                    metrics.cross_padding + 16. - slot.lift - hop + slide,
                    slot.origin_main,
                )
            };
            debug_assert!(slot.size <= metrics.icon_size * metrics.scale + 0.001);
            let mut tile = dock_slot(SharedString::from(id), slot.size)
                .left(px(x))
                .top(px(y));
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
            tile = tile.child(self.icon(icon, slot.size, p));
            if launcher {
                let h = self.host.clone();
                tile = tile.on_click(move |_, _, cx| {
                    h.update(cx, |h, cx| h.handle(Action::Panel(Panel::Apps), cx))
                });
            } else if let Some(app) = app {
                let h = self.host.clone();
                let activate = app.clone();
                let context = app.id.clone();
                tile = tile
                    .on_click(move |_, _, cx| h.update(cx, |h, cx| h.activate_app(&activate, cx)))
                    .on_mouse_down(MouseButton::Right, {
                        let h = self.host.clone();
                        move |_, _, cx| {
                            h.update(cx, |h, cx| {
                                h.handle(Action::Panel(Panel::Dock(context.clone())), cx)
                            })
                        }
                    });
            } else {
                let h = self.host.clone();
                tile = tile.on_click(move |_, _, cx| {
                    h.update(cx, |h, cx| {
                        h.handle(
                            Action::Core(clarion_ipc::Action::Spawn {
                                command: vec!["gio".into(), "open".into(), "trash:///".into()],
                            }),
                            cx,
                        )
                    })
                });
            }
            // Raised/bouncing pixels remain clickable. The empty animation
            // budget stays click-through; glass and input use these same masks.
            regions.push(Bounds::new(
                point(px(x), px(y)),
                size(px(slot.size), px(slot.size)),
            ));
            let recent = app.is_some_and(|a| {
                host.prefs.show_recent_applications && !a.pinned && a.windows.is_empty()
            });
            let separate = trash || (recent && !recent_started);
            recent_started |= recent;
            if separate {
                let position = slot.origin_main - if vertical { 3.5 } else { 6. };
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
                let dot = if metrics.icon_size < 33. { 3. } else { 4. };
                let main = slot.origin_main + slot.size / 2. - dot / 2.;
                let cross = if !vertical {
                    metrics.cross_padding + metrics.icon_size + 18. + slide
                } else if left {
                    10. - slide
                } else {
                    metrics.cross_padding + metrics.item + 10. - dot + slide
                };
                content = content.child(
                    div()
                        .absolute()
                        .left(px(if vertical { cross } else { main }))
                        .top(px(if vertical { main } else { cross }))
                        .size(px(dot))
                        .rounded_full()
                        .bg(if dark {
                            rgba(0xf4f4f4e6)
                        } else {
                            rgba(0x252529bf)
                        }),
                );
            }
            content = content.child(tile);
        }
        // Clip the protocol masks to the negotiated surface. Quantizing keeps
        // subpixel animation from issuing redundant Wayland region commits.
        let viewport = window.viewport_size();
        let regions: Vec<_> = regions
            .into_iter()
            .filter_map(|r| {
                let x = f32::from(r.left()).floor().max(0.);
                let y = f32::from(r.top()).floor().max(0.);
                let right = f32::from(r.right()).ceil().min(f32::from(viewport.width));
                let bottom = f32::from(r.bottom()).ceil().min(f32::from(viewport.height));
                (right > x && bottom > y)
                    .then(|| Bounds::new(point(px(x), px(y)), size(px(right - x), px(bottom - y))))
            })
            .collect();
        #[cfg(test)]
        {
            self.dock_motion.input_regions = regions.clone();
            self.dock_motion.animation_active = active;
        }
        window.set_input_region(Some(&regions));
        if active {
            window.request_animation_frame();
        }
        let tray = div()
            .absolute()
            .left(px(tray_x))
            .top(px(tray_y))
            .w(px(tray_w))
            .h(px(tray_h))
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
            );
        div()
            .id("dock-surface")
            .debug_selector(|| "dock-surface".into())
            .relative()
            .size_full()
            .child(tray)
            .child(content)
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                let target = f32::from(if vertical {
                    event.position.y
                } else {
                    event.position.x
                });
                if this.dock_motion.pointer_target != Some(target) {
                    if this.dock_motion.pointer_target.is_none() {
                        this.dock_motion.pointer.snap(target);
                    }
                    this.dock_motion.pointer_target = Some(target);
                    cx.notify();
                }
            }))
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
                }
                let h = this.host.clone();
                let hovered = *hovered;
                cx.defer(move |cx| h.update(cx, |h, cx| h.handle(Action::DockHover(hovered), cx)));
                cx.notify();
            }))
            .into_any_element()
    }
}
