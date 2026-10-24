// SPDX-License-Identifier: GPL-3.0-or-later
//! Clarion's single visual system. No legacy themes or alternate launcher.
use crate::{
    app::{Action, DockApp, Host, Panel, ViewState},
    backend,
    input::{QueryChanged, TextInput},
    preferences::{DockPosition, MAX_SURFACE_OPACITY, MIN_SURFACE_OPACITY},
    services,
};
use gpui::{prelude::*, *};
use std::{path::PathBuf, sync::Arc};

mod dock_folder;
mod macos_controls;
pub(crate) mod macos_dock;
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
    drawer: macos_dock::DockDrawerMotion,
    output: String,
    input: Entity<TextInput>,
    query: String,
    filter: u8,
    selection: usize,
    scroll: ScrollHandle,
    folder_fan_offset: usize,
    folder_fan_root: Option<PathBuf>,
    folder_fan_started: Option<std::time::Instant>,
    /// Set when the fan retracts; `None` while it is open or opening.
    folder_fan_closing: Option<std::time::Instant>,
    /// Keeps the fan collapsed until the window is actually removed.
    folder_fan_closed: bool,
    /// Whether the last `panel()` render used the fan layout.
    folder_fan_active: bool,
    /// Keeps the folder surface mapped but empty while a file drag is in
    /// flight. The layer still owns the pointer grab, which is what lets the
    /// drag preview follow the cursor across the whole output.
    folder_drag_layer: bool,
    /// Whether this Dock window itself received the current drag events.
    pub(crate) dock_drag_seen: bool,
    #[cfg(test)]
    notification_regions: std::rc::Rc<std::cell::RefCell<Vec<Bounds<Pixels>>>>,
    focus: FocusHandle,
    activated: bool,
    image_cache: std::collections::HashMap<String, Arc<Image>>,
    tray_image_hashes: std::collections::HashMap<String, [u8; 32]>,
    cached_snapshot: std::sync::Weak<ViewState>,
}
#[derive(Clone, Copy)]
struct Palette {
    /// Menus, popups and notification cards share one calm slate surface.
    panel: Hsla,
    /// The Dock tray is a touch deeper so icons stay the brightest element.
    dock: Hsla,
    card: Hsla,
    fg: Hsla,
    muted: Hsla,
    line: Hsla,
    dock_line: Hsla,
    accent: Hsla,
    /// A barely-there accent wash layered over panel outlines.
    accent_wash: Hsla,
    /// The compositor blur needs a small in-app lift to read as frost rather
    /// than plain transparency, especially over busy wallpaper.
    frost_wash: Hsla,
    scrim: Hsla,
    shadow: Hsla,
    frost: f32,
}
impl Palette {
    fn new(host: &ViewState) -> Self {
        let accent: Hsla = rgb(
            u32::from_str_radix(host.prefs.accent.trim_start_matches('#'), 16).unwrap_or(0x0a84ff),
        )
        .into();
        Self::styled(
            host.dark(),
            accent,
            host.prefs.dock_opacity as f32,
            host.prefs.surface_opacity as f32,
            host.prefs.frost_strength as f32,
        )
    }
    /// Build the palette from the resolved appearance values. Only the two
    /// surface colours receive the opacity sliders; foreground colours stay
    /// fully opaque so text never fades with the menu background.
    fn styled(
        dark: bool,
        accent: Hsla,
        dock_alpha: f32,
        surface_alpha: f32,
        frost: f32,
    ) -> Self {
        let dock_alpha = dock_alpha.clamp(0.3, 1.);
        let surface_alpha = surface_alpha.clamp(0.3, 1.);
        let frost = frost.clamp(0., 1.);
        let tint = |hex: u32, alpha: f32| Hsla::from(rgb(hex)).opacity(alpha);
        if dark {
            // Indigo-leaning neutrals let a coloured wallpaper glow through
            // without turning text grey. The accent wash supplies the only
            // saturated colour, so any chosen accent stays coherent.
            let panel: Hsla = rgb(0x222431).into();
            let dock: Hsla = rgb(0x191b26).into();
            let accent_wash = accent.opacity(0.035 + 0.045 * frost);
            let frost_wash = tint(0xffffff, 0.012 + 0.04 * frost);
            Self {
                panel: panel
                    .opacity(surface_alpha)
                    .blend(accent_wash)
                    .blend(frost_wash),
                dock: dock
                    .opacity(dock_alpha)
                    .blend(accent_wash.opacity(0.6))
                    .blend(frost_wash),
                card: tint(0xffffff, 0.055 + 0.03 * frost),
                fg: rgb(0xf4f5fa).into(),
                muted: rgb(0xb0b4c4).into(),
                line: tint(0xd9e3ff, 0.12 + 0.08 * frost),
                dock_line: tint(0xe7edff, 0.16 + 0.10 * frost),
                accent,
                accent_wash,
                frost_wash,
                scrim: tint(0x10121b, 0.16 + 0.16 * frost),
                shadow: tint(0x000000, 0.18 + 0.14 * frost),
                frost,
            }
        } else {
            let panel: Hsla = rgb(0xf8f8fd).into();
            let dock: Hsla = rgb(0xf3f4fa).into();
            let accent_wash = accent.opacity(0.025 + 0.035 * frost);
            let frost_wash = tint(0xffffff, 0.03 + 0.06 * frost);
            Self {
                panel: panel
                    .opacity(surface_alpha)
                    .blend(accent_wash)
                    .blend(frost_wash),
                dock: dock
                    .opacity(dock_alpha)
                    .blend(accent_wash.opacity(0.5))
                    .blend(frost_wash),
                card: tint(0x344966, 0.04 + 0.025 * frost),
                fg: rgb(0x23242a).into(),
                muted: rgb(0x6d707b).into(),
                line: tint(0x355578, 0.10 + 0.08 * frost),
                dock_line: tint(0xffffff, 0.22 + 0.10 * frost),
                accent,
                accent_wash,
                frost_wash,
                scrim: tint(0x111523, 0.14 + 0.14 * frost),
                shadow: tint(0x1c2740, 0.12 + 0.12 * frost),
                frost,
            }
        }
    }
    /// Zero frost turns the compositor effect off rather than asking it for a
    /// zero-opacity blur that some compositors still apply to the whole output.
    fn frosted(&self) -> bool {
        self.frost > 0.01
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
        let drawer = macos_dock::DockDrawerMotion::new();
        Self {
            host,
            state,
            dock_motion,
            drawer,
            kind,
            output,
            input,
            query: String::new(),
            filter: 0,
            selection: 0,
            scroll: ScrollHandle::new(),
            folder_fan_offset: 0,
            folder_fan_root: None,
            folder_fan_started: None,
            folder_fan_closing: None,
            folder_fan_closed: false,
            folder_fan_active: false,
            folder_drag_layer: false,
            dock_drag_seen: false,
            #[cfg(test)]
            notification_regions: Default::default(),
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
                let view = cx.entity();
                let panel = this.state.panel.clone();
                cx.defer(move |cx| {
                    if host.read(cx).panel == panel {
                        view.update(cx, |this, cx| {
                            if this.state.panel == panel {
                                this.dismiss_folder_fan(cx);
                            }
                        });
                    }
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
        let id = id.into();
        let selector = id.to_string();
        div()
            .id(id)
            .debug_selector(move || selector.clone())
            .min_w_0()
            .max_w_full()
            .px(px(8.))
            .py(px(5.))
            .rounded(px(5.))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
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
            .unwrap_or_else(|| "Home".into());
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
    /// The applications folded into the Dock's overflow tile, as a fixed 3x3
    /// grid. A click activates the application; a right click opens its Dock
    /// menu; a pinned application can be dragged to reorder the group.
    fn dock_overflow(
        &mut self,
        host: &ViewState,
        ids: &[String],
        icon_size: f32,
        p: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use crate::view::macos_dock::{
            drawer_radius, drawer_rows, drawer_size, DrawerGeometry, DRAWER_COLUMNS, DRAWER_GAP,
            DRAWER_PAD,
        };
        let apps = host.dock_apps();
        // The fitted size the Dock row is drawing, so the drawer tiles match it.
        let icon = icon_size;
        let cell = icon + DRAWER_GAP;
        let radius = drawer_radius(icon);
        // The live arrangement: the dragged order when a reorder is in flight,
        // otherwise the overflow order the Dock handed us.
        let order: Vec<String> = self
            .drawer
            .drag
            .as_ref()
            .map(|drag| drag.order.clone())
            .unwrap_or_else(|| ids.to_vec());
        // The container is always three columns; the rows grow with the overflow.
        let rows = drawer_rows(order.len());
        let (container_w, container_h) = drawer_size(icon, order.len());
        // The grid is centered horizontally and starts from the top padding.
        let grid_x = (container_w - (DRAWER_COLUMNS as f32 * cell - DRAWER_GAP)) / 2.;
        let grid_y = DRAWER_PAD;
        let geometry = DrawerGeometry {
            x: grid_x,
            y: grid_y,
            cell,
            rows,
        };
        self.drawer.geometry = geometry;
        // While a reorder is in flight each untouched cell slides towards its new
        // grid position instead of jumping. Without a drag the motions collapse
        // back onto the resting cells, so the maps drain.
        let now = self.drawer.epoch.elapsed();
        let mut drawer_active = false;
        if self.drawer.drag.is_some() {
            let dragged = self.drawer.drag.as_ref().map(|drag| drag.id.clone());
            let positions: Vec<(String, f32, f32)> = order
                .iter()
                .enumerate()
                .filter(|(_, id)| dragged.as_deref() != Some(id.as_str()))
                .map(|(index, id)| {
                    let (tx, ty) = geometry.cell_origin(index);
                    (id.clone(), tx, ty)
                })
                .collect();
            drawer_active |= crate::dock_motion::track_reorder(
                &mut self.drawer.reorder_x,
                30.,
                positions.iter().map(|(id, tx, _)| (id.clone(), *tx)),
                now,
            );
            drawer_active |= crate::dock_motion::track_reorder(
                &mut self.drawer.reorder_y,
                30.,
                positions.iter().map(|(id, _, ty)| (id.clone(), *ty)),
                now,
            );
        } else {
            crate::dock_motion::drain_reorder(&mut self.drawer.reorder_x);
            crate::dock_motion::drain_reorder(&mut self.drawer.reorder_y);
        }
        let mut surface = div()
            .id("dock-drawer")
            .debug_selector(|| "dock-drawer".into())
            .relative()
            .w(px(container_w))
            .h(px(container_h))
            .rounded(px(radius))
            .on_mouse_move(cx.listener({
                let order = order.clone();
                move |this, event: &MouseMoveEvent, _, cx| {
                    let x = f32::from(event.position.x);
                    let y = f32::from(event.position.y);
                    if this.drawer.drag.is_some() {
                        this.drawer.track_drag(x, y);
                        cx.notify();
                    } else if this
                        .drawer
                        .press
                        .as_ref()
                        .is_some_and(|press| press.pinned)
                        && {
                            let press = this.drawer.press.as_ref().unwrap();
                            (x - press.pointer).abs() >= crate::dock_motion::DRAG_THRESHOLD
                                || (y - press.pointer_y).abs() >= crate::dock_motion::DRAG_THRESHOLD
                        }
                        && this.drawer.begin_drag(x, y, &order)
                    {
                        cx.notify();
                    }
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    let Some(new_order) = this.drawer.end_drag() else {
                        return;
                    };
                    let h = this.host.clone();
                    cx.defer(move |cx| {
                        h.update(cx, |h, cx| {
                            let order = h.reorder_pinned_subset(&new_order);
                            h.handle(Action::DockOrder(order), cx)
                        })
                    });
                    cx.notify();
                }),
            )
            .on_hover(cx.listener(|this, hovered, _, _| {
                this.drawer.hovered = *hovered;
                if !*hovered {
                    this.drawer.abort_drag();
                }
            }));
        for (index, id) in order.iter().enumerate() {
            let dragging = self
                .drawer
                .drag
                .as_ref()
                .is_some_and(|drag| &drag.id == id);
            if dragging {
                // The dragged tile floats above the grid; the grid keeps its
                // shape and the vacated cell stays empty.
                continue;
            }
            let app = apps.iter().find(|app| &app.id == id).cloned();
            let icon_path = app.as_ref().and_then(|app| app.icon.clone());
            // A sliding cell uses its animated origin; a resting cell uses the
            // grid origin directly.
            let (ox, oy) = match (self.drawer.reorder_x.get(id), self.drawer.reorder_y.get(id)) {
                (Some(mx), Some(my)) if self.drawer.drag.is_some() => {
                    (mx.sample(now).value, my.sample(now).value)
                }
                _ => geometry.cell_origin(index),
            };
            let activate = app.clone();
            let context = id.to_owned();
            let pressed_id = id.to_owned();
            let released_id = id.to_owned();
            let pinned = app.as_ref().is_some_and(|app| app.pinned);
            surface = surface.child(
                div()
                    .id(SharedString::from(format!("overflow-{id}")))
                    .debug_selector({
                        let context = id.to_owned();
                        move || format!("overflow-{context}")
                    })
                    .absolute()
                    .left(px(ox))
                    .top(px(oy))
                    .w(px(icon))
                    .h(px(icon))
                    .rounded(px(icon * 0.24))
                    .cursor_pointer()
                    .active(|style| style.opacity(0.65))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, _| {
                            this.drawer.press(
                                pressed_id.clone(),
                                pinned,
                                f32::from(event.position.x),
                                f32::from(event.position.y),
                            );
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            // A reorder never launches the application.
                            if this.drawer.drag.is_some() {
                                return;
                            }
                            let held = this
                                .drawer
                                .press
                                .as_ref()
                                .is_some_and(|press| press.id == released_id);
                            if !held {
                                return;
                            }
                            if let Some(app) = &activate {
                                let h = this.host.clone();
                                let app = app.clone();
                                h.update(cx, |h, cx| {
                                    h.activate_app(&app, cx);
                                    // Activating from the drawer dismisses it, as
                                    // a Dock menu row does.
                                    h.handle(Action::Close, cx);
                                });
                            }
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
                    .child(self.icon(icon_path, icon, p)),
            );
        }
        // The dragged application follows the pointer.
        if let Some(drag) = self.drawer.drag.clone() {
            if let Some(app) = apps.iter().find(|app| app.id == drag.id).cloned() {
                let x = f32::from(drag.pointer.x) - drag.grab.0;
                let y = f32::from(drag.pointer.y) - drag.grab.1;
                surface = surface.child(
                    div()
                        .id(SharedString::from(format!("drawer-drag-{}", app.id)))
                        .debug_selector({
                            let id = app.id.clone();
                            move || format!("drawer-drag-{id}")
                        })
                        .absolute()
                        .left(px(x))
                        .top(px(y))
                        .w(px(icon))
                        .h(px(icon))
                        .rounded(px(icon * 0.24))
                        .shadow(vec![BoxShadow {
                            color: rgba(0x00000052).into(),
                            offset: point(px(0.), px(6.)),
                            blur_radius: px(14.),
                            spread_radius: px(0.),
                        }])
                        .child(self.icon(app.icon.clone(), icon, p)),
                );
            }
        }
        if drawer_active {
            window.request_animation_frame();
        }
        surface.into_any_element()
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
        &mut self,
        host: &ViewState,
        p: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.folder_fan_active = false;
        let panel = host.panel.clone().unwrap_or(Panel::Apps);
        if self.folder_drag_layer && matches!(&panel, Panel::DockFolder { .. }) {
            return div()
                .size_full()
                .debug_selector(|| "folder-drag-layer".into())
                .into_any_element();
        }
        match &panel {
            Panel::Apps => return self.macos_launchpad(host, p, window, cx),
            Panel::Preferences => return self.macos_preferences(host, p),
            Panel::Notifications => return self.macos_notifications(host, p, window, cx),
            _ => {}
        }
        let viewport = window.viewport_size();
        let (width, height) = (f32::from(viewport.width), f32::from(viewport.height));
        // Menus read tighter than dashboards, so they use a smaller radius.
        let menu = matches!(
            &panel,
            Panel::Dock(_)
                | Panel::Tray(_)
                | Panel::DockFolder { .. }
                | Panel::DockFolderMenu { .. }
        );
        // A Dock menu and its overflow drawer are modal: the window covers the
        // whole output so a click outside the visible popup can dismiss it, and
        // the placement reports where the visible popup sits inside that window.
        let modal = matches!(
            &panel,
            Panel::Dock(_)
                | Panel::DockOverflow { .. }
                | Panel::DockFolder { .. }
                | Panel::DockFolderMenu { .. }
        );
        let placement = if modal {
            Some(crate::app::panels::placement(host, &panel, width, height))
        } else {
            None
        };
        let arrow = placement.as_ref().and_then(|placement| placement.arrow);
        if let Panel::DockFolder { root } = &panel {
            if placement
                .as_ref()
                .is_some_and(|placement| placement.popup.is_none())
            {
                // The fan layout is a transparent full-output overlay. It is
                // rendered separately from the rectangular list/grid popup.
                return self.dock_folder_fan(host, root, width, height, p, window, cx);
            }
        }
        let mut body = div()
            .size_full()
            .rounded(px(if menu { 12. } else { 16. }))
            .text_color(p.fg)
            .text_size(px(13.));
        if arrow.is_none() {
            // A modal popup draws its own outline so the body and the pointer
            // share one fill and one border.
            body = body.bg(p.panel).border_1().border_color(p.line);
        }
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
            Panel::DockFolder { .. } => {
                self.dock_folder_content(host, width, height, p, cx)
            }
            Panel::DockFolderMenu { path } => {
                self.dock_folder_menu_content(host, path, p)
            }
            Panel::DockOverflow { ids, icon_size } => {
                self.dock_overflow(host, ids, f32::from(*icon_size), p, window, cx)
            }
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
        if let Some(arrow) = arrow {
            use crate::app::panels::{ArrowEdge, DOCK_ARROW};
            let popup = placement
                .as_ref()
                .and_then(|placement| placement.popup)
                .expect("a modal popup carries its visible rectangle");
            let pw = f32::from(popup.size.width);
            let ph = f32::from(popup.size.height);
            // The content is laid out inside the visible popup, leaving the
            // strip the pointer occupies empty.
            let mut inner = body.child(content);
            inner = match arrow.edge {
                ArrowEdge::Bottom => inner
                    .w(px(pw))
                    .h(px((ph - DOCK_ARROW).max(0.)))
                    .absolute()
                    .left(px(0.))
                    .top(px(0.)),
                ArrowEdge::Left => inner
                    .w(px((pw - DOCK_ARROW).max(0.)))
                    .h(px(ph))
                    .absolute()
                    .left(px(DOCK_ARROW))
                    .top(px(0.)),
                ArrowEdge::Right => inner
                    .w(px((pw - DOCK_ARROW).max(0.)))
                    .h(px(ph))
                    .absolute()
                    .left(px(0.))
                    .top(px(0.)),
            };
            // A fullscreen invisible catcher dismisses the menu on a click
            // outside the visible popup. It is an earlier sibling, so the popup
            // wins the hit test and clicks inside the menu never reach it.
            let h = self.host.clone();
            let backdrop = div()
                .id("dock-menu-backdrop")
                .absolute()
                .left(px(0.))
                .top(px(0.))
                .w(px(width))
                .h(px(height))
                .bg(rgba(0x00000001))
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    h.update(cx, |h, cx| h.handle(Action::Close, cx))
                });
            let h = self.host.clone();
            let backdrop = backdrop.on_mouse_down(MouseButton::Right, move |_, _, cx| {
                h.update(cx, |h, cx| h.handle(Action::Close, cx))
            });
            let overlay = div().relative().size_full().child(backdrop);
            return overlay
                .child(
                    div()
                        .absolute()
                        .left(popup.origin.x)
                        .top(popup.origin.y)
                        .w(px(pw))
                        .h(px(ph))
                        .relative()
                        // The catcher covers the whole output, so a click inside
                        // the visible popup must not also reach it.
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                        .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_mouse_up(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                        .child(popup_shape(arrow, pw, ph, p))
                        .child(inner),
                )
                .into_any_element();
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
                    self.macos_toast(item, host, p, window, cx)
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
        // Notifications set their regions after layout, when wrapped card
        // heights and the clamped scroll offset are available.
        let modal = matches!(
            host.panel,
            Some(
                Panel::Dock(_)
                    | Panel::DockOverflow { .. }
                    | Panel::DockFolder { .. }
                    | Panel::DockFolderMenu { .. }
            )
        );
        if matches!(self.kind, Surface::Panel)
            && host.panel != Some(Panel::Notifications)
            && modal
        {
            // A modal Dock popup covers the whole output so it can catch a click
            // anywhere outside the visible popup. Frost only the popup itself,
            // never the transparent catcher, so the rest of the screen stays
            // untouched.
            let full = vec![Bounds::new(
                point(px(0.), px(0.)),
                size(px(width), px(height)),
            )];
            // Set the frost region first so the background effect never falls
            // back to blurring the whole fullscreen window for a frame.
            if !p.frosted() {
                window.set_glass_region(Some(&[]));
            } else if let Some(panel) = host.panel.as_ref() {
                let placement =
                    crate::app::panels::placement(host, panel, width, height);
                if let Some(popup) = placement.popup {
                    if self.folder_drag_layer {
                        // The popup is now an invisible drag layer; never leave
                        // its old frost rectangle over the desktop.
                        window.set_glass_region(Some(&[]));
                    } else {
                        // Frost the popup's actual outline. A plain rectangle would
                        // blur the transparent corners and the strip beside the
                        // pointer wedge, leaving a rectangular halo around it.
                        window.set_glass_region(Some(&popup_region(placement.arrow, popup)));
                    }
                } else {
                    // `None` falls back to the full input region in the
                    // compositor, which would blur the whole output. The fan
                    // must explicitly advertise an empty frost region.
                    window.set_glass_region(Some(&[]));
                }
            }
            window.set_input_region(Some(&full));
        } else if matches!(self.kind, Surface::Panel)
            && host.panel != Some(Panel::Notifications)
        {
            let is_apps = host.panel == Some(Panel::Apps);
            let (x, y, w, h, radius) = if is_apps {
                (0., 0., width, height, 0.)
            } else if matches!(host.panel, Some(Panel::Tray(_))) {
                (0., 0., width, height, 12.)
            } else {
                (0., 0., width, height, 16.)
            };
            let mut regions = if is_apps {
                macos_launchpad::backdrop_regions(host, width, height)
            } else {
                rounded_region(w, h, radius)
            };
            for region in &mut regions {
                region.origin.x += px(x);
                region.origin.y += px(y);
            }
            // For non-modal panels the input shape already follows the visible
            // container. Mirror it explicitly so the frost preference can also
            // switch the compositor effect off without falling back to a
            // full-window blur.
            if p.frosted() {
                window.set_glass_region(Some(&regions));
            } else {
                window.set_glass_region(Some(&[]));
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
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                if this.folder_drag_layer {
                    this.forward_drag_move(event.position, event.modifiers, window, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, window, cx| {
                    this.finish_folder_drag_layer(event.position, event.modifiers, window, cx);
                }),
            )
            .on_action(cx.listener(|this, _: &Dismiss, window, cx| {
                if this.input.read(cx).has_preedit() {
                    return;
                }
                if this.folder_drag_layer {
                    cx.stop_active_drag(window);
                    this.close_folder_now(cx);
                } else if this.folder_fan_active {
                    this.dismiss_folder_fan(cx)
                } else {
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

/// Width of a single-line label in the Shell UI font, for sizing containers
/// that must grow with their text (the Dock hover tooltip).
pub(crate) fn measure_label(text: &str, size: f32, window: &Window) -> f32 {
    let mut label_font = font(shell_font_for(&window.text_system().all_font_names()));
    label_font.weight = FontWeight::MEDIUM;
    let run = TextRun {
        len: text.len(),
        font: label_font,
        color: rgb(0xffffff).into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    f32::from(
        window
            .text_system()
            .shape_line(text.to_owned().into(), px(size), &[run], None)
            .width,
    )
}

fn shell_font(cx: &App) -> SharedString {
    shell_font_for(&cx.text_system().all_font_names())
}

fn shell_font_for(available: &[String]) -> SharedString {
    static FAMILY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    FAMILY
        .get_or_init(|| {
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
/// Draw the whole popup — the rounded body plus the pointer wedge — as one
/// filled outline with one border. Painting a single shape keeps the arrow and
/// the panel exactly the same colour and removes the seam at the join, which
/// separate elements cannot achieve for an irregular outline.
fn popup_shape(
    arrow: crate::app::panels::Arrow,
    width: f32,
    height: f32,
    p: Palette,
) -> AnyElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let origin = bounds.origin;
            let mut fill = PathBuilder::fill();
            append_popup_path(&mut fill, arrow, origin, width, height);
            if let Ok(path) = fill.build() {
                window.paint_path(path, p.panel);
            }
            // A one-point stroke on the same inset outline sits fully inside the
            // window, so the popup has a crisp border all the way round.
            let mut stroke = PathBuilder::stroke(px(1.));
            append_popup_path(&mut stroke, arrow, origin, width, height);
            if let Ok(path) = stroke.build() {
                window.paint_path(path, p.line);
            }
        },
    )
    .size_full()
    .absolute()
    .into_any_element()
}

/// Corner radius and inset of a Dock popup outline, shared by the painted path
/// and the frosted region so the two shapes cannot drift apart.
const POPUP_RADIUS: f32 = 12.;
const POPUP_INSET: f32 = 0.5;

/// Geometry shared by the painted outline and the frosted region: the body
/// rectangle, excluding the strip the pointer wedge lives in, as
/// `(x, y, width, height)` in popup-local coordinates.
fn popup_body(
    edge: crate::app::panels::ArrowEdge,
    width: f32,
    height: f32,
) -> (f32, f32, f32, f32) {
    use crate::app::panels::{ArrowEdge, DOCK_ARROW};
    match edge {
        ArrowEdge::Bottom => (
            POPUP_INSET,
            POPUP_INSET,
            width - 2. * POPUP_INSET,
            height - DOCK_ARROW - POPUP_INSET,
        ),
        ArrowEdge::Left => (
            DOCK_ARROW,
            POPUP_INSET,
            width - DOCK_ARROW - POPUP_INSET,
            height - 2. * POPUP_INSET,
        ),
        ArrowEdge::Right => (
            POPUP_INSET,
            POPUP_INSET,
            width - DOCK_ARROW - POPUP_INSET,
            height - 2. * POPUP_INSET,
        ),
    }
}

/// Keep the pointer clear of the rounded corners so the wedge base never
/// crosses a corner arc and folds the outline over itself.
fn clamp_pointer(value: f32, min: f32, max: f32) -> f32 {
    if min >= max {
        (min + max) * 0.5
    } else {
        value.clamp(min, max)
    }
}

fn translate_regions(regions: &mut [Bounds<Pixels>], x: f32, y: f32) {
    for region in regions {
        region.origin.x += px(x);
        region.origin.y += px(y);
    }
}

fn push_region(regions: &mut Vec<Bounds<Pixels>>, x: f32, y: f32, width: f32, height: f32) {
    if width > 0. && height > 0. {
        regions.push(Bounds::new(
            point(px(x), px(y)),
            size(px(width), px(height)),
        ));
    }
}

/// Append the closed outline of a popup with a pointer wedge on `arrow.edge`.
/// The outline is inset half a point so a one-point border stays on-screen.
fn append_popup_path(
    path: &mut PathBuilder,
    arrow: crate::app::panels::Arrow,
    origin: Point<Pixels>,
    width: f32,
    height: f32,
) {
    use crate::app::panels::{ArrowEdge, DOCK_ARROW};
    let k = 0.552_284_8;
    let at = |x: f32, y: f32| origin + point(px(x), px(y));
    // Body rectangle, excluding the strip the wedge lives in.
    let (bx, by, bw, bh) = popup_body(arrow.edge, width, height);
    let left = bx;
    let right = bx + bw;
    let top = by;
    let bottom = by + bh;
    let r = POPUP_RADIUS.min(bw / 2.).min(bh / 2.);
    let base = DOCK_ARROW;
    let corner = |path: &mut PathBuilder, a: (f32, f32), c: (f32, f32), b: (f32, f32)| {
        let c1 = (a.0 + (c.0 - a.0) * k, a.1 + (c.1 - a.1) * k);
        let c2 = (b.0 + (c.0 - b.0) * k, b.1 + (c.1 - b.1) * k);
        path.cubic_bezier_to(at(b.0, b.1), at(c1.0, c1.1), at(c2.0, c2.1));
    };
    match arrow.edge {
        ArrowEdge::Bottom => {
            let tip = clamp_pointer(
                arrow.offset,
                left + r + base,
                right - r - base,
            );
            path.move_to(at(left + r, top));
            path.line_to(at(right - r, top));
            corner(path, (right - r, top), (right, top), (right, top + r));
            // Round the bottom corners as well: without this the body ends in
            // a straight edge above the wedge and looks clipped.
            path.line_to(at(right, bottom - r));
            corner(
                path,
                (right, bottom - r),
                (right, bottom),
                (right - r, bottom),
            );
            path.line_to(at(tip + base, bottom));
            path.line_to(at(tip, height - POPUP_INSET));
            path.line_to(at(tip - base, bottom));
            path.line_to(at(left + r, bottom));
            corner(
                path,
                (left + r, bottom),
                (left, bottom),
                (left, bottom - r),
            );
            path.line_to(at(left, top + r));
            corner(path, (left, top + r), (left, top), (left + r, top));
            path.close();
        }
        ArrowEdge::Left => {
            let tip = clamp_pointer(
                arrow.offset,
                top + r + base,
                bottom - r - base,
            );
            path.move_to(at(left + r, top));
            path.line_to(at(right - r, top));
            corner(path, (right - r, top), (right, top), (right, top + r));
            path.line_to(at(right, bottom - r));
            corner(path, (right, bottom - r), (right, bottom), (right - r, bottom));
            path.line_to(at(left + r, bottom));
            corner(path, (left + r, bottom), (left, bottom), (left, bottom - r));
            path.line_to(at(left, tip + base));
            path.line_to(at(POPUP_INSET, tip));
            path.line_to(at(left, tip - base));
            path.line_to(at(left, top + r));
            corner(path, (left, top + r), (left, top), (left + r, top));
            path.close();
        }
        ArrowEdge::Right => {
            let tip = clamp_pointer(
                arrow.offset,
                top + r + base,
                bottom - r - base,
            );
            path.move_to(at(left + r, top));
            path.line_to(at(right - r, top));
            corner(path, (right - r, top), (right, top), (right, top + r));
            path.line_to(at(right, tip - base));
            path.line_to(at(width - POPUP_INSET, tip));
            path.line_to(at(right, tip + base));
            path.line_to(at(right, bottom - r));
            corner(path, (right, bottom - r), (right, bottom), (right - r, bottom));
            path.line_to(at(left + r, bottom));
            corner(path, (left + r, bottom), (left, bottom), (left, bottom - r));
            path.line_to(at(left, top + r));
            corner(path, (left, top + r), (left, top), (left + r, top));
            path.close();
        }
    }
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

/// Rectangles covering a Dock popup's visible outline: its rounded body plus
/// the pointer wedge. Wayland background-effect regions are rectangle lists,
/// so the wedge is approximated with one-point bands, the same way
/// [`rounded_region`] approximates the corner arcs.
pub fn popup_region(
    arrow: Option<crate::app::panels::Arrow>,
    popup: Bounds<Pixels>,
) -> Vec<Bounds<Pixels>> {
    use crate::app::panels::{ArrowEdge, DOCK_ARROW};
    let x = f32::from(popup.origin.x);
    let y = f32::from(popup.origin.y);
    let width = f32::from(popup.size.width);
    let height = f32::from(popup.size.height);
    if width <= 0. || height <= 0. {
        return Vec::new();
    }
    let Some(arrow) = arrow else {
        let mut regions = rounded_region(width, height, POPUP_RADIUS);
        translate_regions(&mut regions, x, y);
        return regions;
    };
    let (bx, by, bw, bh) = popup_body(arrow.edge, width, height);
    let radius = POPUP_RADIUS.min(bw / 2.).min(bh / 2.);
    let mut regions = rounded_region(bw.max(0.), bh.max(0.), radius);
    translate_regions(&mut regions, x + bx, y + by);
    let base = DOCK_ARROW;
    match arrow.edge {
        ArrowEdge::Bottom => {
            let body_bottom = by + bh;
            let tip_y = height - POPUP_INSET;
            let tip_x = clamp_pointer(
                arrow.offset,
                bx + radius + base,
                bx + bw - radius - base,
            );
            // One-point rows narrowing from the base to the tip.
            let rows = (tip_y - body_bottom).ceil().max(0.) as u32;
            for row in 0..rows {
                let top = body_bottom + row as f32;
                let progress =
                    ((top + 0.5 - body_bottom) / (tip_y - body_bottom)).clamp(0., 1.);
                let half = base * (1. - progress);
                let left = (tip_x - half).max(bx);
                let right = (tip_x + half).min(bx + bw);
                push_region(&mut regions, x + left, y + top, right - left, 1.);
            }
        }
        ArrowEdge::Left => {
            let tip_x = POPUP_INSET;
            let tip_y = clamp_pointer(
                arrow.offset,
                by + radius + base,
                by + bh - radius - base,
            );
            let columns = (bx - tip_x).ceil().max(0.) as u32;
            for column in 0..columns {
                let left = tip_x + column as f32;
                let progress = ((left + 0.5 - tip_x) / (bx - tip_x)).clamp(0., 1.);
                let half = base * progress;
                let top = (tip_y - half).max(by);
                let bottom = (tip_y + half).min(by + bh);
                push_region(&mut regions, x + left, y + top, 1., bottom - top);
            }
        }
        ArrowEdge::Right => {
            let body_right = bx + bw;
            let tip_x = width - POPUP_INSET;
            let tip_y = clamp_pointer(
                arrow.offset,
                by + radius + base,
                by + bh - radius - base,
            );
            let columns = (tip_x - body_right).ceil().max(0.) as u32;
            for column in 0..columns {
                let left = body_right + column as f32;
                let progress =
                    ((left + 0.5 - body_right) / (tip_x - body_right)).clamp(0., 1.);
                let half = base * (1. - progress);
                let top = (tip_y - half).max(by);
                let bottom = (tip_y + half).min(by + bh);
                push_region(&mut regions, x + left, y + top, 1., bottom - top);
            }
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

    #[::core::prelude::v1::test]
    fn surface_opacity_only_fades_backgrounds_not_foreground_text() {
        for dark in [true, false] {
            for opacity in [0.3, 0.68, 1.0] {
                let palette = Palette::styled(
                    dark,
                    rgb(0x0a84ff).into(),
                    opacity,
                    opacity,
                    0.82,
                );
                assert!((palette.panel.a - opacity).abs() < 0.0001);
                assert!((palette.dock.a - opacity).abs() < 0.0001);
                assert_eq!(palette.fg.a, 1.0);
                assert_eq!(palette.muted.a, 1.0);
            }
        }
    }

    #[::core::prelude::v1::test]
    fn the_popup_frost_region_follows_the_rounded_outline_and_wedge() {
        use crate::app::panels::{Arrow, ArrowEdge};
        let popup = Bounds::new(point(px(50.), px(100.)), size(px(250.), px(200.)));
        let contains = |regions: &[Bounds<Pixels>], x: f32, y: f32| {
            regions.iter().any(|region| {
                x >= f32::from(region.origin.x)
                    && y >= f32::from(region.origin.y)
                    && x < f32::from(region.right())
                    && y < f32::from(region.bottom())
            })
        };
        for edge in [ArrowEdge::Bottom, ArrowEdge::Left, ArrowEdge::Right] {
            let regions = popup_region(Some(Arrow { edge, offset: 100. }), popup);
            assert!(!regions.is_empty());
            assert!(regions.len() <= 256);
            for region in &regions {
                assert!(region.origin.x >= popup.origin.x && region.origin.y >= popup.origin.y);
                assert!(region.right() <= popup.right() && region.bottom() <= popup.bottom());
            }
            // The body still frosts its center, but not the transparent space
            // outside its rounded corners.
            assert!(contains(&regions, 175., 200.));
            assert!(!contains(&regions, 50.2, 100.2));
            assert!(!contains(&regions, 299.8, 100.2));
        }
        let regions = popup_region(Some(Arrow {
            edge: ArrowEdge::Bottom,
            offset: 125.,
        }), popup);
        // The wedge is frosted all the way to its tip, while the empty strip
        // on either side of it stays clear.
        assert!(contains(&regions, 175., 298.5));
        assert!(!contains(&regions, 190., 298.5));
        // The two bottom corners are rounded, not a square clipped edge.
        assert!(!contains(&regions, 51., 290.2));
        assert!(!contains(&regions, 298., 290.2));

        let regions = popup_region(None, popup);
        assert!(contains(&regions, 175., 200.));
        assert!(!contains(&regions, 50.2, 100.2));
        assert!(!contains(&regions, 50.5, 290.2));
    }

    #[::core::prelude::v1::test]
    fn the_popup_outline_stays_inside_its_window() {
        use crate::app::panels::{Arrow, ArrowEdge};
        let (width, height) = (250., 200.);
        for (edge, offset) in [
            (ArrowEdge::Bottom, 125.),
            (ArrowEdge::Left, 100.),
            (ArrowEdge::Right, 100.),
        ] {
            let arrow = Arrow { edge, offset };
            // The single-shape outline must build for every edge; a failure here
            // would leave the popup unpainted.
            for stroke in [false, true] {
                let mut path = if stroke {
                    PathBuilder::stroke(px(1.))
                } else {
                    PathBuilder::fill()
                };
                append_popup_path(&mut path, arrow, point(px(0.), px(0.)), width, height);
                assert!(path.build().is_ok(), "the popup outline builds for {edge:?}");
            }
        }
    }
}

#[cfg(test)]
#[path = "ui_tests.rs"]
mod ui_tests;

#[cfg(test)]
mod dock_folder_tests;
#[cfg(test)]
mod dock_tests;
