// SPDX-License-Identifier: GPL-3.0-or-later
//! Clarion's single visual system. No legacy themes or alternate launcher.
use crate::{
    app::{Action, Host, Panel, ViewState},
    backend,
    input::{QueryChanged, TextInput},
    preferences::DockPosition,
    services,
};
use gpui::{prelude::*, *};
use std::{path::PathBuf, sync::Arc};

mod macos_controls;
mod macos_dock;
mod macos_launchpad;
mod macos_notifications;
mod macos_preferences;

actions!(shell, [Dismiss, Activate, Previous, Next]);
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", Dismiss, Some("ClarionPanel")),
        KeyBinding::new("enter", Activate, Some("ClarionPanel")),
        KeyBinding::new("up", Previous, Some("ClarionPanel")),
        KeyBinding::new("down", Next, Some("ClarionPanel")),
    ]);
}
#[derive(Clone, Debug)]
pub enum Surface {
    Bar,
    Dock,
    Panel,
    Toast(u32),
    DockEdge,
}
pub struct ShellView {
    pub host: Entity<Host>,
    pub state: Arc<ViewState>,
    kind: Surface,
    dock_motion: macos_dock::DockViewMotion,
    output: String,
    input: Entity<TextInput>,
    query: String,
    filter: u8,
    selection: usize,
    scroll: ScrollHandle,
    focus: FocusHandle,
    activated: bool,
    image_cache: std::collections::HashMap<String, Arc<Image>>,
    tray_image_hashes: std::collections::HashMap<String, [u8; 32]>,
    cached_snapshot: std::sync::Weak<ViewState>,
}
#[derive(Clone, Copy)]
struct Palette {
    panel: Hsla,
    card: Hsla,
    fg: Hsla,
    muted: Hsla,
    line: Hsla,
    accent: Hsla,
}
impl Palette {
    fn new(host: &ViewState) -> Self {
        let dark = host.dark();
        let accent =
            u32::from_str_radix(host.prefs.accent.trim_start_matches('#'), 16).unwrap_or(0x0a84ff);
        if dark {
            Self {
                panel: rgba(0x24242bb8).into(),
                card: rgba(0xffffff0d).into(),
                fg: rgb(0xf3f3f5).into(),
                muted: rgb(0xaaaab0).into(),
                line: rgba(0xffffff26).into(),
                accent: rgb(accent).into(),
            }
        } else {
            Self {
                panel: rgba(0xf4f4f7bd).into(),
                card: rgba(0x3449660d).into(),
                fg: rgb(0x242426).into(),
                muted: rgb(0x7a7a80).into(),
                line: rgba(0x263f5e20).into(),
                accent: rgb(accent).into(),
            }
        }
    }
}
#[derive(Default)]
struct SliderGesture {
    dragging: bool,
    preview: Option<(f64, std::time::Instant)>,
    last_sent: Option<std::time::Instant>,
    last_sent_value: Option<f64>,
}

fn slider_value(bounds: Bounds<Pixels>, x: Pixels) -> f64 {
    let width = f32::from((bounds.size.width - px(16.)).max(px(1.)));
    f64::from((f32::from(x - bounds.left() - px(8.)) / width).clamp(0., 1.))
}

fn update_slider(
    gesture: &Entity<SliderGesture>,
    bounds: Bounds<Pixels>,
    x: Pixels,
    force: bool,
    window: &mut Window,
    cx: &mut App,
) -> Option<f64> {
    let value = slider_value(bounds, x);
    let send = gesture.update(cx, |state, cx| {
        let now = std::time::Instant::now();
        state.preview = Some((value, now));
        let send = state.last_sent_value != Some(value)
            && (force
                || state
                    .last_sent
                    .is_none_or(|sent| sent.elapsed() >= std::time::Duration::from_millis(80)));
        if send {
            state.last_sent = Some(now);
            state.last_sent_value = Some(value);
        }
        cx.notify();
        send
    });
    window.refresh();
    send.then_some(value)
}

impl ShellView {
    pub fn new(
        host: Entity<Host>,
        kind: Surface,
        output: String,
        state: Arc<ViewState>,
        cx: &mut Context<Self>,
    ) -> Self {
        let launchpad = state.panel == Some(Panel::Apps);
        let dark_input = launchpad || state.dark();
        let input = cx.new(|cx| {
            TextInput::new(cx)
                .with_placeholder(if launchpad {
                    "搜索"
                } else {
                    "搜索最近内容"
                })
                .with_compact(dark_input)
        });
        cx.subscribe(&input, |this, _, QueryChanged(query), cx| {
            this.query = query.clone();
            this.selection = 0;
            this.scroll.set_offset(point(px(0.), px(0.)));
            cx.notify();
        })
        .detach();
        let dock_motion = macos_dock::DockViewMotion::new(state.dock_revealed);
        Self {
            host,
            state,
            dock_motion,
            kind,
            output,
            input,
            query: String::new(),
            filter: 0,
            selection: 0,
            scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            activated: false,
            image_cache: Default::default(),
            tray_image_hashes: Default::default(),
            cached_snapshot: Default::default(),
        }
    }
    pub fn observe_activation(&self, window: &mut Window, cx: &mut Context<Self>) {
        cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.activated = true;
            } else if this.activated
                && matches!(this.kind, Surface::Panel)
                && this.state.panel != Some(Panel::Preferences)
            {
                this.activated = false;
                let host = this.host.clone();
                let panel = this.state.panel.clone();
                cx.defer(move |cx| {
                    host.update(cx, |host, cx| {
                        if host.panel == panel {
                            host.handle(Action::Close, cx);
                        }
                    });
                });
            }
        })
        .detach();
    }
    pub fn focus(&self, window: &mut Window, cx: &App) {
        if matches!(self.state.panel, Some(Panel::Apps | Panel::Recent)) {
            window.focus(&self.input.focus_handle(cx));
        } else {
            window.focus(&self.focus);
        }
    }
    fn cache_images(&mut self) {
        use sha2::{Digest, Sha256};
        let snapshot = Arc::downgrade(&self.state);
        if std::sync::Weak::ptr_eq(&self.cached_snapshot, &snapshot) {
            return;
        }
        self.cached_snapshot = snapshot;
        let mut keep = std::collections::HashSet::new();
        for entry in &self.state.history {
            if let Some(bytes) = &entry.thumbnail_png {
                let key = format!("history:{}", entry.id);
                keep.insert(key.clone());
                self.image_cache.entry(key).or_insert_with(|| {
                    Arc::new(Image::from_bytes(ImageFormat::Png, bytes.clone()))
                });
            }
        }
        for item in &self.state.tray {
            if let Some(bytes) = &item.icon_png {
                let key = format!("tray:{}", item.id);
                keep.insert(key.clone());
                let hash: [u8; 32] = Sha256::digest(bytes).into();
                if self.tray_image_hashes.get(&key) != Some(&hash) {
                    self.image_cache.insert(
                        key.clone(),
                        Arc::new(Image::from_bytes(ImageFormat::Png, bytes.clone())),
                    );
                    self.tray_image_hashes.insert(key, hash);
                }
            }
        }
        self.image_cache.retain(|key, _| keep.contains(key));
        self.tray_image_hashes.retain(|key, _| keep.contains(key));
    }
    fn invoke(&self, action: Action, cx: &mut App) {
        let host = self.host.clone();
        cx.defer(move |cx| host.update(cx, |host, cx| host.handle(action, cx)));
    }
    fn button(
        &self,
        id: impl Into<SharedString>,
        text: impl Into<SharedString>,
        action: Action,
        p: Palette,
    ) -> AnyElement {
        let host = self.host.clone();
        div()
            .id(id.into())
            .px(px(8.))
            .py(px(5.))
            .rounded(px(5.))
            .text_color(p.fg)
            .text_size(px(13.))
            .cursor_pointer()
            .hover(move |s| s.bg(p.line))
            .child(text.into())
            .on_click(move |_, _, cx| host.update(cx, |host, cx| host.handle(action.clone(), cx)))
            .into_any_element()
    }
    fn icon(&self, path: Option<PathBuf>, size: f32, p: Palette) -> AnyElement {
        if let Some(path) = path {
            img(path)
                .size(px(size))
                .object_fit(ObjectFit::Contain)
                .into_any_element()
        } else {
            div()
                .size(px(size))
                .rounded(px(12.))
                .bg(p.card)
                .flex()
                .items_center()
                .justify_center()
                .text_color(p.accent)
                .child("◇")
                .into_any_element()
        }
    }
    fn symbol(&self, name: &str, size: f32, p: Palette) -> AnyElement {
        let path = icon_path(name);
        if path.is_file() {
            svg()
                .path(path.to_string_lossy().into_owned())
                .size(px(size))
                .text_color(p.fg)
                .into_any_element()
        } else {
            div()
                .size(px(size))
                .text_center()
                .child("◇")
                .into_any_element()
        }
    }
    fn glyph_button(&self, id: &str, name: &str, action: Action, p: Palette) -> AnyElement {
        let host = self.host.clone();
        div()
            .id(SharedString::from(id.to_owned()))
            .size(px(30.))
            .rounded(px(8.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(move |s| s.bg(p.line))
            .child(self.symbol(name, 17., p))
            .on_click(move |_, _, cx| host.update(cx, |h, cx| h.handle(action.clone(), cx)))
            .into_any_element()
    }
    fn bar(&self, host: &ViewState, p: Palette, width: f32, cx: &App) -> AnyElement {
        // Menu-bar content sits on the compositor's live backdrop. In particular,
        // do not cover it with the old opaque application-panel background.
        let focused = host.desktop.windows.iter().find(|w| w.is_focused);
        let appname = focused
            .and_then(|w| w.app_id.as_deref())
            .map(|id| {
                if id == "io.github.clarion.DesktopPreferences" {
                    return "桌面与程序坞".into();
                }
                host.catalog
                    .apps()
                    .iter()
                    .find(|app| app.id.trim_end_matches(".desktop").eq_ignore_ascii_case(id))
                    .map(|app| app.name.clone())
                    .unwrap_or_else(|| id.to_owned())
            })
            .unwrap_or_else(|| "访达".into());
        let mark = image_path(if host.dark() {
            "clarion-mark-white.png"
        } else {
            "clarion-mark-black.png"
        });
        let h = self.host.clone();
        let mut row = div()
            .id("macos-menu-bar")
            .size_full()
            .flex()
            .items_center()
            .px(px(10.))
            .gap(px(0.))
            .bg(if host.dark() {
                rgba(0x14141a45)
            } else {
                rgba(0xffffff38)
            })
            .text_color(p.fg)
            .text_size(px(13.))
            .line_height(px(18.))
            .child(
                div()
                    .id("clarion-menu-mark")
                    .w(px(32.))
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(move |s| s.bg(p.line))
                    .child(
                        img(mark)
                            .w(px(24.5))
                            .h(px(26.5))
                            .flex_shrink_0()
                            .object_fit(ObjectFit::Contain),
                    )
                    .on_click(move |_, _, cx| {
                        h.update(cx, |h, cx| h.handle(Action::Panel(Panel::Preferences), cx))
                    }),
            )
            .child(
                div()
                    .max_w(px(width * 0.28))
                    .px(px(9.))
                    .h_full()
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .text_ellipsis()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(menu_text(appname, FontWeight::SEMIBOLD, p.fg, cx)),
            )
            .child(div().flex_1());
        // This is deliberately an activity area, never application-exported
        // File/Edit/View menus or a Mission Control/workspace switcher.
        if width > 680. {
            for item in host
                .tray
                .iter()
                .filter(|item| item.status != "Passive")
                .take(6)
            {
                let id = item.id.clone();
                let action = if item.item_is_menu {
                    Action::Panel(Panel::Tray(id.clone()))
                } else {
                    Action::Service(services::Command::TrayActivate {
                        id: id.clone(),
                        x: 0,
                        y: 0,
                    })
                };
                let h = self.host.clone();
                let menu = id.clone();
                let icon = if let Some(image) = self.image_cache.get(&format!("tray:{}", item.id)) {
                    img(image.clone())
                        .size(px(16.))
                        .object_fit(ObjectFit::Contain)
                        .into_any_element()
                } else {
                    self.symbol("circle", 14., p)
                };
                row = row.child(
                    div()
                        .id(SharedString::from(format!("tray-{id}")))
                        .w(px(28.))
                        .h_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.))
                        .cursor_pointer()
                        .hover(move |s| s.bg(p.line))
                        .child(icon)
                        .on_click(move |_, _, cx| {
                            h.update(cx, |h, cx| h.handle(action.clone(), cx))
                        })
                        .on_mouse_down(MouseButton::Right, {
                            let h = self.host.clone();
                            move |_, _, cx| {
                                h.update(cx, |h, cx| {
                                    h.handle(Action::Panel(Panel::Tray(menu.clone())), cx)
                                })
                            }
                        }),
                );
            }
        }
        row = row.child(self.status_button("recent", "clipboard", Action::Panel(Panel::Recent), p));
        if width > 520. {
            row = row.child(self.status_button(
                "capture",
                "scan",
                Action::Panel(Panel::Screenshot),
                p,
            ));
        }
        if host.dnd {
            row =
                row.child(self.status_button("dnd-active", "moon", Action::DoNotDisturb(false), p));
        }
        if width > 800. {
            if let Some(battery) = host.system.battery {
                let fill = div()
                    .w(px((19. * battery.clamp(0., 1.)) as f32))
                    .h(px(7.))
                    .rounded(px(1.))
                    .bg(if host.system.charging {
                        rgb(0x30d158).into()
                    } else {
                        p.fg
                    });
                let mut indicator = div()
                    .id("menu-battery")
                    .h_full()
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(5.));
                if host.prefs.show_battery_percentage {
                    indicator = indicator.child(
                        div()
                            .text_size(px(12.))
                            .child(format!("{}%", (battery * 100.).round())),
                    );
                }
                row = row.child(
                    indicator.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(1.5))
                            .child(
                                div()
                                    .w(px(23.))
                                    .h(px(11.))
                                    .p(px(1.))
                                    .border_1()
                                    .border_color(p.fg.opacity(0.65))
                                    .rounded(px(2.5))
                                    .child(fill),
                            )
                            .child(
                                div()
                                    .w(px(1.5))
                                    .h(px(4.))
                                    .rounded(px(0.75))
                                    .bg(p.fg.opacity(0.65)),
                            ),
                    ),
                );
            }
        }
        row = row.child(self.status_button(
            "controls",
            "control-center",
            Action::Panel(Panel::Controls),
            p,
        ));
        let clock = if host.prefs.clock_show_date && width > 880. {
            format!("{}  {}", host.system.date, host.system.clock)
        } else {
            host.system.clock.clone()
        };
        let h = self.host.clone();
        row.child(
            div()
                .id("menu-clock")
                .h_full()
                .px(px(8.))
                .flex()
                .items_center()
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .rounded(px(4.))
                .hover(move |s| s.bg(p.line))
                .child(menu_text(clock, FontWeight::NORMAL, p.fg, cx))
                .on_click(move |_, _, cx| {
                    h.update(cx, |h, cx| {
                        h.handle(Action::Panel(Panel::Notifications), cx)
                    })
                }),
        )
        .into_any_element()
    }
    fn status_button(&self, id: &str, name: &str, action: Action, p: Palette) -> AnyElement {
        let h = self.host.clone();
        div()
            .id(SharedString::from(id.to_owned()))
            .w(px(28.))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .cursor_pointer()
            .hover(move |s| s.bg(p.line))
            .child(self.symbol(name, 15., p))
            .on_click(move |_, _, cx| h.update(cx, |h, cx| h.handle(action.clone(), cx)))
            .into_any_element()
    }
    fn empty(&self, title: &str, detail: &str, p: Palette) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .py_12()
            .gap_3()
            .child(self.symbol("circle", 30., p))
            .child(title.to_owned())
            .child(div().text_sm().text_color(p.muted).child(detail.to_owned()))
            .into_any_element()
    }
    fn recent(&self, host: &ViewState, p: Palette, cx: &Context<Self>) -> AnyElement {
        let mut rows = div().flex().flex_col().gap_3();
        let mut filters = div().flex().gap_2();
        for (i, label) in ["全部", "文本", "图片", "截图"].iter().enumerate() {
            filters = filters.child(
                div()
                    .id(("recent-filter", i))
                    .px_3()
                    .py_2()
                    .rounded(px(8.))
                    .text_xs()
                    .cursor_pointer()
                    .bg(if self.filter == i as u8 {
                        p.line
                    } else {
                        p.card
                    })
                    .child(*label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.filter = i as u8;
                        cx.notify();
                    })),
            );
        }
        rows = rows.child(filters).child(
            div()
                .flex()
                .gap_2()
                .child(self.button(
                    "history-pause",
                    if host.clipboard_paused {
                        "恢复记录"
                    } else {
                        "暂停记录"
                    },
                    Action::Service(services::Command::Pause(!host.clipboard_paused)),
                    p,
                ))
                .child(self.button(
                    "history-clear",
                    "清空未置顶",
                    Action::Service(services::Command::Clear),
                    p,
                )),
        );
        if !host.clipboard_available {
            rows = rows.child(
                div()
                    .text_xs()
                    .text_color(p.muted)
                    .child("剪贴板服务尚未连接，已有内容会保留。"),
            );
        }
        let query = self.query.to_lowercase();
        let mut count = 0;
        for entry in &host.history {
            if !query.is_empty() && !entry.text.to_lowercase().contains(&query) {
                continue;
            }
            if self.filter == 1 && !matches!(entry.kind, services::HistoryKind::Text) {
                continue;
            }
            if self.filter == 2 && matches!(entry.kind, services::HistoryKind::Text) {
                continue;
            }
            if self.filter == 3 && !matches!(entry.kind, services::HistoryKind::Screenshot) {
                continue;
            }
            count += 1;
            let label = match entry.kind {
                services::HistoryKind::Text => "文字",
                services::HistoryKind::Image => "图片",
                services::HistoryKind::Screenshot => "截图",
            };
            let mut card = div()
                .p_4()
                .rounded(px(12.))
                .bg(p.card)
                .flex()
                .flex_col()
                .gap_3()
                .child(div().text_xs().text_color(p.muted).child(format!(
                    "{}{}",
                    if entry.pinned { "已置顶 · " } else { "" },
                    label
                )));
            if let Some(image) = self.image_cache.get(&format!("history:{}", entry.id)) {
                card = card.child(
                    img(image.clone())
                        .w_full()
                        .max_h(px(150.))
                        .object_fit(ObjectFit::Contain),
                );
            } else {
                card = card.child(
                    div()
                        .text_sm()
                        .max_h(px(120.))
                        .overflow_hidden()
                        .child(entry.text.chars().take(1000).collect::<String>()),
                );
            }
            let mut actions = div().flex().flex_wrap().gap_2();
            if host.clipboard_available {
                actions = actions.child(self.button(
                    format!("copy-{}", entry.id),
                    "复制",
                    Action::Service(services::Command::Copy(entry.id.clone())),
                    p,
                ));
            }
            actions = actions
                .child(self.button(
                    format!("pin-{}", entry.id),
                    if entry.pinned {
                        "取消置顶"
                    } else {
                        "置顶"
                    },
                    Action::Service(services::Command::Pin(entry.id.clone())),
                    p,
                ))
                .child(self.button(
                    format!("delete-{}", entry.id),
                    "移除",
                    Action::Service(services::Command::Delete(entry.id.clone())),
                    p,
                ));
            if let Some(path) = &entry.file_path {
                actions = actions.child(self.button(
                    format!("open-{}", entry.id),
                    "打开原图",
                    Action::Backend(backend::Command::OpenPath(path.clone())),
                    p,
                ));
            }
            rows = rows.child(card.child(actions));
        }
        if count == 0 {
            rows = rows.child(self.empty(
                "这里还没有内容",
                "复制文字、图片或完成截图后，会在这里出现。",
                p,
            ));
        }
        rows.child(
            div()
                .text_xs()
                .text_color(p.muted)
                .child("历史仅留在本次会话。清空或移除记录不会删除截图原件。"),
        )
        .into_any_element()
    }
    fn tray_menu(&self, host: &ViewState, p: Palette) -> AnyElement {
        let Some(Panel::Tray(id)) = &host.panel else {
            return div().into_any_element();
        };
        let mut menu = div().flex().flex_col().gap_2();
        if host.tray_menu.is_empty() {
            menu = menu.child("正在读取应用菜单…");
        }
        for item in &host.tray_menu {
            menu = menu.child(self.menu_entry(id, item, p, 0));
        }
        menu.into_any_element()
    }
    fn menu_entry(
        &self,
        id: &str,
        item: &services::MenuItem,
        p: Palette,
        depth: usize,
    ) -> AnyElement {
        if !item.visible || depth > 5 {
            return div().into_any_element();
        }
        if item.separator {
            return div().h(px(1.)).my_2().bg(p.line).into_any_element();
        }
        let mut row = div().flex().flex_col().gap_1().pl(px(depth as f32 * 10.));
        let label = format!(
            "{}{}",
            if item.toggle == Some(true) {
                "✓ "
            } else {
                ""
            },
            item.label.replace('_', "")
        );
        if item.enabled {
            row = row.child(self.button(
                format!("menu-{id}-{}", item.id),
                label,
                Action::Service(services::Command::MenuClick {
                    id: id.into(),
                    item: item.id,
                }),
                p,
            ));
        } else {
            row = row.child(div().text_sm().text_color(p.muted).p_2().child(label));
        }
        for child in &item.children {
            row = row.child(self.menu_entry(id, child, p, depth + 1));
        }
        row.into_any_element()
    }
    fn dock_menu(&self, host: &ViewState, id: &str, p: Palette) -> AnyElement {
        let apps = host.dock_apps();
        let app = apps.iter().find(|a| a.id == id);
        let mut row = div().flex().flex_col().gap_2();
        if let Some(app) = app {
            for w in &app.windows {
                row = row
                    .child(self.button(
                        format!("focus-{}", w.id),
                        w.title.clone().unwrap_or_else(|| app.name.clone()),
                        Action::Focus(w.id),
                        p,
                    ))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(self.button(
                                format!("minimize-{}", w.id),
                                if w.is_minimized {
                                    "还原"
                                } else {
                                    "最小化"
                                },
                                if w.is_minimized {
                                    Action::Focus(w.id)
                                } else {
                                    Action::Minimize(w.id)
                                },
                                p,
                            ))
                            .child(self.button(
                                format!("close-{}", w.id),
                                "关闭窗口",
                                Action::CloseWindow(w.id),
                                p,
                            )),
                    );
            }
            row = row.child(self.button(
                "pin-app",
                if app.pinned {
                    "从 Dock 取消固定"
                } else {
                    "固定到 Dock"
                },
                Action::PinApp(id.into()),
                p,
            ));
        } else if host.catalog.get(id).is_some() {
            row = row
                .child(self.button("launch-from-menu", "打开应用", Action::Launch(id.into()), p))
                .child(self.button("pin-from-menu", "固定到 Dock", Action::PinApp(id.into()), p));
        }
        row.into_any_element()
    }
    fn panel(
        &self,
        host: &ViewState,
        p: Palette,
        window: &Window,
        cx: &Context<Self>,
    ) -> AnyElement {
        let panel = host.panel.clone().unwrap_or(Panel::Apps);
        match &panel {
            Panel::Apps => return self.macos_launchpad(host, p, window, cx),
            Panel::Preferences => return self.macos_preferences(host, p),
            Panel::Notifications => return self.macos_notifications(host, p, cx),
            _ => {}
        }
        let mut body = div()
            .size_full()
            .rounded(px(16.))
            .bg(p.panel)
            .border_1()
            .border_color(p.line)
            .text_color(p.fg)
            .text_size(px(13.));
        if panel == Panel::Controls {
            return body
                .p(px(12.))
                .child(self.macos_controls(host, p))
                .into_any_element();
        }
        if panel == Panel::Screenshot {
            let mut tools = div().flex().items_center().gap(px(5.));
            for (id, label, symbol, mode) in [
                ("capture-screen", "整个屏幕", "capture-screen", 1),
                ("capture-window", "当前窗口", "capture-window", 2),
                ("capture-region", "所选部分", "capture-region", 0),
            ] {
                let h = self.host.clone();
                tools = tools.child(
                    div()
                        .id(id)
                        .w(px(96.))
                        .h(px(44.))
                        .rounded(px(7.))
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(2.))
                        .cursor_pointer()
                        .hover(move |s| s.bg(p.line))
                        .child(self.symbol(symbol, 24., p))
                        .child(
                            div()
                                .text_size(px(10.))
                                .line_height(px(12.))
                                .child(menu_text(label.into(), FontWeight::NORMAL, p.fg, cx)),
                        )
                        .on_click(move |_, _, cx| {
                            h.update(cx, |h, cx| h.handle(Action::Screenshot(mode), cx))
                        }),
                );
            }
            return body
                .p(px(10.))
                .flex()
                .items_center()
                .gap(px(8.))
                .child(self.glyph_button("capture-close", "x", Action::Close, p))
                .child(div().w(px(1.)).h(px(28.)).bg(p.line))
                .child(tools)
                .into_any_element();
        }
        let content = match &panel {
            Panel::Recent => self.recent(host, p, cx),
            Panel::Tray(_) => self.tray_menu(host, p),
            Panel::Dock(id) => self.dock_menu(host, id, p),
            _ => unreachable!(),
        };
        body = body.p(px(10.)).flex().flex_col().gap(px(8.));
        if panel == Panel::Recent {
            body = body
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .pl(px(4.))
                        .child(menu_text("最近内容".into(), FontWeight::SEMIBOLD, p.fg, cx))
                        .child(self.glyph_button("recent-close", "x", Action::Close, p)),
                )
                .child(
                    div()
                        .h(px(28.))
                        .rounded(px(6.))
                        .bg(p.card)
                        .child(self.input.clone()),
                );
        }
        if let Some(error) = &host.error {
            body = body.child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(0xe5a87f))
                    .child(error.clone()),
            );
        }
        body.child(
            div()
                .id("panel-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .child(content),
        )
        .into_any_element()
    }
}
impl Render for ShellView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.cache_images();
        let snapshot = self.state.clone();
        let host = snapshot.as_ref();
        let p = Palette::new(host);
        let width = f32::from(window.viewport_size().width);
        let height = f32::from(window.viewport_size().height);
        let content = match self.kind {
            Surface::Bar => self.bar(host, p, width, cx),
            Surface::Dock => self.macos_dock(host, p, window, cx),
            Surface::Panel => self.panel(host, p, window, cx),
            Surface::Toast(id) => {
                if let Some(item) = host.notifications.iter().find(|n| n.id == id) {
                    div()
                        .size_full()
                        .p_4()
                        .rounded(px(14.))
                        .bg(p.panel)
                        .border_1()
                        .border_color(p.line)
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(div().text_xs().text_color(p.muted).child(item.app.clone()))
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(item.title.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .overflow_hidden()
                                .child(item.body.chars().take(180).collect::<String>()),
                        )
                        .into_any_element()
                } else {
                    div().into_any_element()
                }
            }
            Surface::DockEdge => {
                let host = self.host.clone();
                div()
                    .id("dock-edge")
                    .size_full()
                    .bg(rgba(0x00000001))
                    .on_hover(move |hovered, _, cx| {
                        let host = host.clone();
                        let hovered = *hovered;
                        cx.defer(move |cx| {
                            host.update(cx, |host, cx| {
                                host.handle(Action::DockEdgeHover(hovered), cx)
                            })
                        });
                    })
                    .into_any_element()
            }
        };
        if matches!(self.kind, Surface::Panel | Surface::Toast(_)) {
            let (x, y, w, h, radius) =
                if matches!(self.kind, Surface::Panel) && host.panel == Some(Panel::Apps) {
                    (0., 0., width, height, 0.)
                } else {
                    (0., 0., width, height, 16.)
                };
            let mut regions =
                if matches!(self.kind, Surface::Panel) && host.panel == Some(Panel::Apps) {
                    macos_launchpad::backdrop_regions(host, width, height)
                } else if matches!(self.kind, Surface::Panel)
                    && host.panel == Some(Panel::Notifications)
                {
                    macos_notifications::card_regions(
                        width,
                        height,
                        host.notifications.len(),
                        f32::from(self.scroll.offset().y),
                    )
                } else {
                    rounded_region(w, h, radius)
                };
            for region in &mut regions {
                region.origin.x += px(x);
                region.origin.y += px(y);
            }
            window.set_input_region(Some(&regions));
        }
        div()
            .id("clarion-surface")
            .size_full()
            .font_family(shell_font(cx))
            .text_color(p.fg)
            .key_context("ClarionPanel")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &Dismiss, _, cx| {
                if !this.input.read(cx).has_preedit() {
                    this.invoke(Action::Close, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &Activate, _, cx| {
                if this.input.read(cx).has_preedit() || this.state.panel != Some(Panel::Apps) {
                    return;
                }
                let id = {
                    let h = &this.state;
                    h.catalog
                        .search(&this.query)
                        .get(this.selection)
                        .map(|a| a.id.clone())
                };
                if let Some(id) = id {
                    this.invoke(Action::Launch(id), cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Previous, window, cx| {
                if !this.input.read(cx).has_preedit() {
                    this.selection = this.selection.saturating_sub(1);
                    if this.state.panel == Some(Panel::Apps) {
                        this.keyboard_selection_into_view(this.state.as_ref(), window);
                    }
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &Next, window, cx| {
                if !this.input.read(cx).has_preedit() {
                    let count = this.state.catalog.search(&this.query).len();
                    this.selection = (this.selection + 1).min(count.saturating_sub(1));
                    if this.state.panel == Some(Panel::Apps) {
                        this.keyboard_selection_into_view(this.state.as_ref(), window);
                    }
                    cx.notify();
                }
            }))
            .child(content)
    }
}
fn menu_text(text: String, weight: FontWeight, color: Hsla, cx: &App) -> StyledText {
    static CJK: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let cjk = CJK.get_or_init(|| {
        let names = cx.text_system().all_font_names();
        let family = if names.iter().any(|name| name == "PingFang SC") {
            "PingFang SC".to_owned()
        } else {
            "Noto Sans CJK SC".to_owned()
        };
        eprintln!("Clarion UI CJK font: {family}");
        family
    });
    let latin = shell_font(cx);
    let mut runs: Vec<TextRun> = Vec::new();
    // Linux GPUI does not honor Font::fallbacks yet. Explicit UTF-8 runs keep
    // Chinese menu glyphs in PingFang rather than a random fontconfig fallback.
    for character in text.chars() {
        let family = if matches!(character as u32,
            0x2e80..=0xa4cf | 0xac00..=0xd7af | 0xf900..=0xfaff |
            0xff00..=0xffef | 0x20000..=0x3134f)
        {
            SharedString::from(cjk.clone())
        } else {
            latin.clone()
        };
        if let Some(last) = runs.last_mut().filter(|run| run.font.family == family) {
            last.len += character.len_utf8();
        } else {
            let mut face = font(family);
            face.weight = weight;
            runs.push(TextRun {
                len: character.len_utf8(),
                font: face,
                color,
                background_color: None,
                underline: None,
                strikethrough: None,
            });
        }
    }
    StyledText::new(text).with_runs(runs)
}

fn shell_font(cx: &App) -> SharedString {
    static FAMILY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    FAMILY
        .get_or_init(|| {
            let available = cx.text_system().all_font_names();
            let family = [
                "Clarion Big Sur Text",
                "SF Pro Text",
                ".SF NS Text",
                "System Font",
                ".SF NS",
                "SF Pro",
                "Inter",
                "Noto Sans",
            ]
            .into_iter()
            .find(|wanted| available.iter().any(|name| name == wanted))
            .unwrap_or("Noto Sans CJK SC")
            .to_owned();
            eprintln!("Clarion UI Latin font: {family}");
            family
        })
        .clone()
        .into()
}
pub fn rounded_region(width: f32, height: f32, radius: f32) -> Vec<Bounds<Pixels>> {
    if width <= 0. || height <= 0. {
        return vec![];
    }
    let r = radius.min(width / 2.).min(height / 2.).max(0.);
    let mut regions = vec![Bounds::new(
        point(px(0.), px(r)),
        size(px(width), px((height - 2. * r).max(0.))),
    )];
    for y in 0..r.ceil() as u32 {
        let d = r - y as f32 - 0.5;
        let inset = (r - (r * r - d * d).max(0.).sqrt()).ceil();
        let w = (width - 2. * inset).max(0.);
        for top in [y as f32, height - y as f32 - 1.] {
            regions.push(Bounds::new(point(px(inset), px(top)), size(px(w), px(1.))));
        }
    }
    regions
}
fn icon_path(name: &str) -> PathBuf {
    image_path(&format!("{name}.svg"))
}
fn image_path(filename: &str) -> PathBuf {
    let prefix = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().and_then(|p| p.parent()).map(|p| p.to_path_buf()));
    let executable = prefix.as_ref().map(|p| p.join("assets/icons"));
    let system = prefix.map(|p| p.join("share/clarion-shell/icons"));
    for root in executable.into_iter().chain(system).chain([
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/icons"),
        PathBuf::from("/usr/share/clarion-shell/icons"),
        PathBuf::from("/usr/local/share/clarion-shell/icons"),
    ]) {
        let path = root.join(filename);
        if path.is_file() {
            return path;
        }
    }
    PathBuf::from(filename)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn rounded_hits_exclude_transparent_corners_and_keep_center() {
        let regions = rounded_region(400., 70., 16.);
        assert!(regions.len() <= 33);
        let contains = |x: f32, y: f32| {
            regions.iter().any(|r| {
                x >= f32::from(r.origin.x)
                    && y >= f32::from(r.origin.y)
                    && x < f32::from(r.origin.x) + f32::from(r.size.width)
                    && y < f32::from(r.origin.y) + f32::from(r.size.height)
            })
        };
        assert!(!contains(0., 0.));
        assert!(contains(200., 0.));
        assert!(contains(200., 35.));
        assert!(!contains(-1., 35.));
    }
    #[::core::prelude::v1::test]
    fn tiny_regions_never_have_negative_sizes() {
        for (w, h) in [(1., 1.), (20., 4.), (0., 30.)] {
            assert!(rounded_region(w, h, 16.)
                .iter()
                .all(|r| f32::from(r.size.width) >= 0. && f32::from(r.size.height) >= 0.));
        }
    }
}

#[cfg(test)]
#[path = "ui_tests.rs"]
mod ui_tests;

#[cfg(test)]
mod dock_tests;
