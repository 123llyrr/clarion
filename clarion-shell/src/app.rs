// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{
    backend,
    desktop::{DesktopAction, DesktopView},
    preferences::{DockPosition, Preferences},
    services,
    view::{ShellView, Surface},
    Options,
};
use clarion_ipc::{Action as CoreAction, DockEdge, DockTarget, MinimizeEffect};
use clarion_launcher::catalog::{AppCatalog, AppCategory};
use gpui::{prelude::*, *};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

mod panels;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Panel {
    Apps,
    Recent,
    Notifications,
    Controls,
    Preferences,
    Screenshot,
    Tray(String),
    Dock(String),
}
#[derive(Clone)]
pub enum Action {
    Panel(Panel),
    Close,
    Launch(String),
    Focus(u64),
    Minimize(u64),
    CloseWindow(u64),
    PinApp(String),
    Core(CoreAction),
    Service(services::Command),
    Backend(backend::Command),
    Theme(String),
    Accent(String),
    DockSize(f64),
    BarHeight(f64),
    DockPosition(DockPosition),
    AutoHide(bool),
    ReduceMotion(bool),
    Screenshot(u8),
    Settings,
    DockHover(bool),
    DockEdgeHover(bool),
    DoNotDisturb(bool),
    PickWallpaper,
}
#[derive(Clone)]
pub struct DockApp {
    pub id: String,
    pub name: String,
    pub icon: Option<std::path::PathBuf>,
    pub windows: Vec<clarion_ipc::Window>,
    pub pinned: bool,
}
#[derive(Clone)]
pub struct ViewState {
    pub prefs: Preferences,
    pub desktop: backend::DesktopSnapshot,
    pub system: backend::SystemSnapshot,
    pub catalog: Arc<AppCatalog>,
    pub notifications: Vec<services::Notification>,
    pub tray: Vec<services::TrayItem>,
    pub history: Vec<services::HistoryEntry>,
    pub clipboard_available: bool,
    pub clipboard_paused: bool,
    pub tray_menu: Vec<services::MenuItem>,
    pub error: Option<String>,
    pub panel: Option<Panel>,
    pub dnd: bool,
    dock_apps: Vec<DockApp>,
}
impl ViewState {
    pub fn dark(&self) -> bool {
        self.prefs.theme != "light"
    }
    pub fn dock_apps(&self) -> Vec<DockApp> {
        self.dock_apps.clone()
    }
}
struct OutputViews {
    bar: WindowHandle<ShellView>,
    dock: Option<WindowHandle<ShellView>>,
    edge: Option<WindowHandle<ShellView>>,
    desktop: Option<WindowHandle<DesktopView>>,
    display: DisplayId,
    width: f32,
    height: f32,
}

pub struct Host {
    pub prefs: Preferences,
    pub desktop: backend::DesktopSnapshot,
    pub system: backend::SystemSnapshot,
    pub catalog: Arc<AppCatalog>,
    pub notifications: Vec<services::Notification>,
    pub tray: Vec<services::TrayItem>,
    pub history: Vec<services::HistoryEntry>,
    pub clipboard_available: bool,
    pub clipboard_paused: bool,
    pub tray_menu: Vec<services::MenuItem>,
    pub error: Option<String>,
    pub panel: Option<Panel>,
    pub preview: bool,
    pub dnd: bool,
    pub revision: u64,
    backend: backend::Backend,
    service: services::ServiceHandle,
    desktop_actions: async_channel::Sender<DesktopAction>,
    local_preferences: bool,
    environment: HashMap<String, String>,
    outputs: BTreeMap<String, OutputViews>,
    popup: Option<WindowHandle<ShellView>>,
    toast: Option<WindowHandle<ShellView>>,
    toast_id: Option<u32>,
    options: Options,
    ready: bool,
    preferences_placed: Option<u64>,
    last_target: HashMap<u64, DockTarget>,
    auto_hidden: bool,
    dock_hover: bool,
    dock_edge_hover: bool,
    idle_since: Instant,
}

pub fn run(options: Options) -> anyhow::Result<()> {
    let preview = options.preview || options.demo;
    let environment: HashMap<String, String> = std::env::vars().collect();
    let prefs = Preferences::load(&environment, preview)?;
    let (backend, events) = backend::Backend::start(backend::Options {
        environment: environment.clone(),
        preview,
        command_server: !preview,
    })?;
    let (service_tx, service_rx) = async_channel::bounded(128);
    let service = services::start(service_tx, preview);
    let (desktop_actions, desktop_events) = async_channel::bounded(32);
    let auto_hidden = prefs.dock_auto_hide;
    Application::new()
        .with_assets(ShellAssets)
        .run(move |cx: &mut App| {
            crate::input::TextInput::bind_keys(cx);
            DesktopView::bind_keys(cx);
            crate::view::bind_keys(cx);
            let host = cx.new(|_| Host {
                prefs,
                desktop: Default::default(),
                system: Default::default(),
                catalog: Arc::new(AppCatalog::default()),
                notifications: vec![],
                tray: vec![],
                history: vec![],
                clipboard_available: false,
                clipboard_paused: false,
                tray_menu: vec![],
                error: None,
                panel: None,
                preview,
                dnd: false,
                revision: 0,
                backend,
                service,
                desktop_actions,
                local_preferences: false,
                environment,
                outputs: BTreeMap::new(),
                popup: None,
                toast: None,
                toast_id: None,
                options,
                ready: false,
                preferences_placed: None,
                last_target: HashMap::new(),
                auto_hidden,
                dock_hover: false,
                dock_edge_hover: false,
                idle_since: Instant::now(),
            });
            let retained = host.clone();
            cx.spawn(async move |cx| {
                while let Ok(event) = events.recv().await {
                    if retained
                        .update(cx, |host, cx| host.backend_event(event, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();
            let retained = host.clone();
            cx.spawn(async move |cx| {
                while let Ok(event) = service_rx.recv().await {
                    if retained
                        .update(cx, |host, cx| host.service_event(event, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();
            let retained = host.clone();
            cx.spawn(async move |cx| loop {
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
                if retained.update(cx, |host, cx| host.tick(cx)).is_err() {
                    break;
                }
            })
            .detach();
            let retained = host.clone();
            cx.spawn(async move |cx| {
                while let Ok(action) = desktop_events.recv().await {
                    if retained
                        .update(cx, |host, cx| host.desktop_action(action, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();
            // Event-loop lifetime is owned by this entity, including when output
            // hotplug temporarily leaves the process without mapped surfaces.
            cx.set_global(ShellLifetime(host));
        });
    Ok(())
}
struct ShellLifetime(#[allow(dead_code)] Entity<Host>);
impl Global for ShellLifetime {}

impl Host {
    fn snapshot(&self) -> ViewState {
        ViewState {
            prefs: self.prefs.clone(),
            desktop: self.desktop.clone(),
            system: self.system.clone(),
            catalog: self.catalog.clone(),
            notifications: self.notifications.clone(),
            tray: self.tray.clone(),
            history: self.history.clone(),
            clipboard_available: self.clipboard_available,
            clipboard_paused: self.clipboard_paused,
            tray_menu: self.tray_menu.clone(),
            error: self.error.clone(),
            panel: self.panel.clone(),
            dnd: self.dnd,
            dock_apps: self.dock_apps(),
        }
    }
    fn desktop_action(&mut self, action: DesktopAction, cx: &mut Context<Self>) {
        match action {
            DesktopAction::Open(path) => {
                let _ = self.backend.send(backend::Command::OpenPath(path));
            }
            DesktopAction::Settings => self.handle(Action::Settings, cx),
            DesktopAction::Wallpaper(Some(path)) => {
                for output in self.outputs.values() {
                    if let Some(desktop) = output.desktop {
                        let path = path.clone();
                        let _ = desktop.update(cx, |view, _, cx| view.set_wallpaper(path, cx));
                    }
                }
            }
            DesktopAction::Wallpaper(None) => {
                // Release the panel's exclusive keyboard role before a portal
                // opens an ordinary file chooser window.
                self.close(cx);
                if self.preview {
                    return;
                }
                let prompt = cx.prompt_for_paths(PathPromptOptions {
                    files: true,
                    directories: false,
                    multiple: false,
                    prompt: Some("选择桌面壁纸".into()),
                });
                cx.spawn(async move |host, cx| {
                    if let Ok(Ok(Some(paths))) = prompt.await {
                        if let Some(path) = paths.into_iter().next() {
                            let _ = host.update(cx, |host, cx| {
                                host.desktop_action(DesktopAction::Wallpaper(Some(path)), cx)
                            });
                        }
                    }
                })
                .detach();
            }
        }
    }
    fn backend_event(&mut self, event: backend::Event, cx: &mut Context<Self>) {
        match event {
            backend::Event::Desktop(snapshot) => {
                if snapshot.generation != self.desktop.generation
                    && matches!(self.panel, Some(Panel::Dock(_)))
                {
                    self.close(cx);
                }
                if !snapshot.connected {
                    self.last_target.clear();
                }
                self.desktop = snapshot;
                self.reconcile_outputs(cx);
                self.place_preferences();
                self.targets();
            }
            backend::Event::System(mut snapshot) => {
                if !self.prefs.clock_show_seconds {
                    snapshot.clock.truncate(5);
                }
                if snapshot == self.system {
                    return;
                }
                self.system = snapshot;
            }
            backend::Event::Preferences(prefs) => {
                if !self.local_preferences && self.prefs != prefs {
                    self.prefs = prefs;
                    self.rebuild(cx);
                    self.targets();
                }
            }
            backend::Event::Catalog(catalog) => {
                self.catalog = Arc::new(catalog);
                self.targets();
            }
            backend::Event::Command(command) => match command.as_str() {
                "launcher" | "spotlight" => self.handle(Action::Panel(Panel::Apps), cx),
                "close" => self.handle(Action::Close, cx),
                "preferences" => self.handle(Action::Panel(Panel::Preferences), cx),
                "screenshot" => self.handle(Action::Panel(Panel::Screenshot), cx),
                "reload" => {
                    let _ = self.backend.send(backend::Command::RefreshCatalog);
                }
                "control-center" | "quick-settings" => {
                    self.handle(Action::Panel(Panel::Controls), cx)
                }
                "notifications" => self.handle(Action::Panel(Panel::Notifications), cx),
                "clipboard" | "recent" | "recent-content" => {
                    self.handle(Action::Panel(Panel::Recent), cx)
                }
                "overview" | "spaces" | "mission-control" => {
                    self.core(CoreAction::ToggleOverview {});
                }
                _ => {}
            },
            backend::Event::Screenshot { path, is_preview } => {
                if !is_preview {
                    if let Some(path) = path {
                        self.service
                            .send(services::Command::RecordScreenshot(path.into()));
                    }
                }
            }
            backend::Event::Error(error) => self.error = Some(error),
        }
        self.changed(cx);
    }
    fn service_event(&mut self, event: services::Event, cx: &mut Context<Self>) {
        match event {
            services::Event::Notifications(items) => {
                let new = items
                    .iter()
                    .find(|item| {
                        item.unread && !self.notifications.iter().any(|old| old.id == item.id)
                    })
                    .cloned();
                self.notifications = items;
                if !self.dnd {
                    if let Some(item) = new {
                        self.show_toast(item.id, cx);
                    }
                }
            }
            services::Event::Tray(items) => self.tray = items,
            services::Event::History {
                entries,
                paused,
                available,
            } => {
                self.history = entries;
                self.clipboard_paused = paused;
                self.clipboard_available = available;
            }
            services::Event::TrayMenu { id, items } => {
                if self.panel == Some(Panel::Tray(id)) {
                    self.tray_menu = items;
                }
            }
            services::Event::Error(error) => self.error = Some(error),
        }
        self.changed(cx);
    }
    pub fn changed(&mut self, cx: &mut Context<Self>) {
        self.revision = self.revision.wrapping_add(1);
        cx.notify();
        let state = Arc::new(self.snapshot());
        for view in self.outputs.values() {
            let count = self.dock_apps().len().min(18) as f32
                + 1.
                + u8::from(self.prefs.show_launcher) as f32;
            let vertical = self.prefs.dock_position != DockPosition::Bottom;
            let length = (count * (self.prefs.dock_size as f32 + 12.) + 20.).min(if vertical {
                view.height - 90.
            } else {
                view.width - 32.
            });
            let thickness = self.prefs.dock_size as f32 + 32.;
            let dimensions = if vertical {
                size(px(thickness + 12.), px(length + 24.))
            } else {
                size(px(length + 24.), px(thickness + 12.))
            };
            let _ = view.bar.update(cx, |v, window, cx| {
                v.state = state.clone();
                let desired = size(px(view.width), px(self.prefs.bar_height as f32));
                if window.viewport_size() != desired {
                    window.resize(desired);
                }
                window.set_layer_exclusive_zone(self.prefs.bar_height.round() as i32);
                cx.notify()
            });
            if let Some(desktop) = view.desktop {
                let reserved = if self.prefs.dock_auto_hide || self.prefs.dock_hidden {
                    0.
                } else {
                    thickness + 8.
                };
                let _ = desktop.update(cx, |desktop, _, cx| {
                    desktop.set_reserved_edges(
                        self.prefs.bar_height as f32,
                        if self.prefs.dock_position == DockPosition::Bottom {
                            reserved
                        } else {
                            0.
                        },
                        if self.prefs.dock_position == DockPosition::Left {
                            reserved
                        } else {
                            0.
                        },
                        if self.prefs.dock_position == DockPosition::Right {
                            reserved
                        } else {
                            0.
                        },
                        cx,
                    );
                });
            }
            if let Some(dock) = view.dock {
                let _ = dock.update(cx, |v, window, cx| {
                    v.state = state.clone();
                    if window.viewport_size() != dimensions {
                        window.resize(dimensions);
                    }
                    window.set_layer_exclusive_zone(if self.prefs.dock_auto_hide {
                        0
                    } else {
                        thickness.round() as i32 + 8
                    });
                    cx.notify()
                });
            }
        }
        if let Some(popup) = self.popup {
            let dynamic_size = self
                .panel
                .as_ref()
                .filter(|panel| matches!(panel, Panel::Tray(_) | Panel::Dock(_)))
                .map(|panel| {
                    let output = self.active_output();
                    let (w, h) = self
                        .outputs
                        .get(&output)
                        .map(|v| (v.width, v.height))
                        .unwrap_or((1200., 800.));
                    panels::placement(&state, panel, w, h).bounds.size
                });
            let _ = popup.update(cx, |v, window, cx| {
                v.state = state.clone();
                if let Some(desired) = dynamic_size {
                    if window.viewport_size() != desired {
                        window.resize(desired);
                    }
                }
                cx.notify()
            });
        }
        if let Some(toast) = self.toast {
            let _ = toast.update(cx, |v, _, cx| {
                v.state = state.clone();
                cx.notify()
            });
        }
    }
    pub fn core(&self, action: CoreAction) {
        let _ = self.backend.send(backend::Command::Action(action));
    }
    /// Platform mouse dispatch still owns its Window while a callback runs.
    /// Defer all surface mutations until that lease is released; this also
    /// covers generic button closures which do not borrow a View entity.
    pub fn handle(&mut self, action: Action, cx: &mut Context<Self>) {
        let host = cx.entity().downgrade();
        cx.defer(move |cx| {
            let _ = host.update(cx, |host, cx| host.apply(action, cx));
        });
    }
    fn apply(&mut self, action: Action, cx: &mut Context<Self>) {
        self.idle_since = Instant::now();
        match action {
            Action::DockHover(hovered) => {
                self.dock_hover = hovered;
                if hovered && self.auto_hidden {
                    self.auto_hidden = false;
                    self.reconcile_docks(cx);
                }
            }
            Action::DockEdgeHover(hovered) => {
                self.dock_edge_hover = hovered;
                if hovered && self.auto_hidden {
                    self.auto_hidden = false;
                    self.reconcile_docks(cx);
                }
            }
            Action::DoNotDisturb(value) => {
                self.dnd = value;
                self.service.send(services::Command::Dnd(value));
            }
            Action::PickWallpaper => self.desktop_action(DesktopAction::Wallpaper(None), cx),
            Action::Close => self.close(cx),
            Action::Panel(panel) => {
                if self.panel.as_ref() == Some(&panel) {
                    self.close(cx);
                } else {
                    self.open(panel, cx);
                }
            }
            Action::Launch(id) => {
                if let Some(app) = self.catalog.get(&id) {
                    let _ = self
                        .backend
                        .send(backend::Command::LaunchDesktop(app.desktop_file.clone()));
                    self.prefs.recent_app_ids.retain(|other| other != &id);
                    self.prefs.recent_app_ids.insert(0, id);
                    self.prefs.recent_app_ids.truncate(24);
                    self.save(cx);
                    self.close(cx);
                }
            }
            Action::Focus(id) => {
                self.core(CoreAction::FocusWindow { id });
                self.close(cx);
            }
            Action::Minimize(id) => {
                self.core(CoreAction::MinimizeWindow { id: Some(id) });
                self.close(cx);
            }
            Action::CloseWindow(id) => {
                self.core(CoreAction::CloseWindow { id: Some(id) });
                self.close(cx);
            }
            Action::PinApp(id) => {
                let mut pins = self.pinned();
                if pins.contains(&id) {
                    pins.retain(|item| item != &id);
                } else {
                    pins.push(id);
                }
                self.prefs.pinned_app_ids = Some(pins);
                self.save(cx);
            }
            Action::Core(action) => self.core(action),
            Action::Service(command) => {
                self.service.send(command);
            }
            Action::Backend(command) => {
                let _ = self.backend.send(command);
            }
            Action::Theme(theme) => {
                self.prefs.theme = theme;
                self.save(cx);
            }
            Action::Accent(accent) => {
                self.prefs.accent = accent;
                self.save(cx);
            }
            Action::DockSize(value) => {
                self.prefs.dock_size = value.clamp(32., 64.).round();
                self.save(cx);
            }
            Action::BarHeight(value) => {
                self.prefs.bar_height = value.clamp(22., 48.);
                self.save(cx);
            }
            Action::DockPosition(value) => {
                self.prefs.dock_position = value;
                self.save(cx);
                self.rebuild(cx);
            }
            Action::AutoHide(value) => {
                self.prefs.dock_auto_hide = value;
                self.auto_hidden = false;
                self.save(cx);
                self.rebuild(cx);
            }
            Action::ReduceMotion(value) => {
                self.prefs.reduce_motion = value;
                self.core(CoreAction::SetReducedMotion {
                    enabled: value,
                    platform: false,
                    initialize_only: false,
                });
                self.save(cx);
            }
            Action::Settings => {
                let _ = self.backend.send(backend::Command::OpenSettings);
                self.close(cx);
            }
            Action::Screenshot(mode) => {
                self.close(cx);
                let host = cx.entity().downgrade();
                cx.spawn(async move |_, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(120))
                        .await;
                    let _ = host.update(cx, |host, _| {
                        host.core(match mode {
                            1 => CoreAction::ScreenshotScreen {
                                write_to_disk: true,
                                show_pointer: false,
                                path: None,
                            },
                            2 => CoreAction::ScreenshotWindow {
                                id: None,
                                write_to_disk: true,
                                show_pointer: false,
                                path: None,
                                is_preview: false,
                            },
                            _ => CoreAction::Screenshot {
                                show_pointer: true,
                                path: None,
                            },
                        })
                    });
                })
                .detach();
            }
        }
        self.targets();
        self.changed(cx);
    }
    fn save(&mut self, _cx: &mut Context<Self>) {
        self.local_preferences = true;
        let _ = self
            .backend
            .send(backend::Command::SavePreferences(self.prefs.clone()));
    }
    fn place_preferences(&mut self) {
        if self.preview || self.panel != Some(Panel::Preferences) {
            return;
        }
        let Some(window) = self.desktop.windows.iter().find(|window| {
            window.app_id.as_deref() == Some("io.github.clarion.DesktopPreferences")
                && window.pid == Some(std::process::id() as i32)
        }) else {
            return;
        };
        if self.preferences_placed == Some(window.id) {
            return;
        }
        let id = window.id;
        let output = self.active_output();
        let (w, h) = self
            .outputs
            .get(&output)
            .map(|view| (view.width, view.height))
            .unwrap_or((1200., 800.));
        let geometry = panels::placement(&self.snapshot(), &Panel::Preferences, w, h).bounds;
        // xdg_toplevel's initial bounds are only a request on Wayland. Place
        // this one owned window explicitly, without changing any client rule.
        if self
            .backend
            .send(backend::Command::PlacePreferences {
                id,
                width: f32::from(geometry.size.width).round() as i32,
                height: f32::from(geometry.size.height).round() as i32,
                x: f64::from(f32::from(geometry.origin.x))
                    - if self.prefs.dock_position == DockPosition::Left
                        && !self.prefs.dock_hidden
                        && !self.prefs.dock_auto_hide
                    {
                        self.prefs.dock_size + 40.
                    } else {
                        0.
                    },
                y: f64::from(f32::from(geometry.origin.y)) - self.prefs.bar_height,
            })
            .is_ok()
        {
            self.preferences_placed = Some(id);
        }
    }
    fn close(&mut self, cx: &mut Context<Self>) {
        self.preferences_placed = None;
        self.panel = None;
        self.tray_menu.clear();
        if let Some(window) = self.popup.take() {
            let _ = window.update(cx, |_, window, _| window.remove_window());
        }
    }
    fn open(&mut self, panel: Panel, cx: &mut Context<Self>) {
        self.close(cx);
        if let Panel::Tray(id) = &panel {
            self.service.send(services::Command::GetMenu(id.clone()));
        }
        if panel == Panel::Apps {
            let _ = self.backend.send(backend::Command::RefreshCatalog);
        }
        if panel == Panel::Notifications {
            self.service.send(services::Command::MarkRead);
        }
        self.panel = Some(panel);
        let output = self.active_output();
        let view = self.outputs.get(&output);
        let display = view.map(|v| v.display);
        let (w, h) = view.map(|v| (v.width, v.height)).unwrap_or((1200., 800.));
        let kind = Surface::Panel;
        let placement = panels::placement(&self.snapshot(), self.panel.as_ref().unwrap(), w, h);
        let bounds = if placement.normal {
            Bounds::centered(display, placement.bounds.size, cx)
        } else {
            placement.bounds
        };
        let mut options = window_options(
            "clarion-panel",
            display,
            bounds,
            self.preview,
            placement.anchor,
            -1,
            LayerShellKeyboardInteractivity::Exclusive,
            placement.margins,
            LayerShellLayer::Overlay,
        );
        if placement.normal {
            options.kind = WindowKind::Normal;
            options.is_movable = true;
            options.is_minimizable = true;
            options.app_id = Some("io.github.clarion.DesktopPreferences".into());
        }
        let host = cx.entity();
        let state = Arc::new(self.snapshot());
        match cx.open_window(options, move |window, cx| {
            if placement.normal {
                window.set_window_title("桌面与程序坞");
                let close_host = host.downgrade();
                let window_id = window.window_handle().window_id();
                window.on_window_should_close(cx, move |_, cx| {
                    let close_host = close_host.clone();
                    // Leave the platform's close callback and Window lease
                    // before Host removes this ordinary window. A queued close
                    // from an old window must never dismiss a replacement.
                    cx.spawn(async move |cx| {
                        let _ = close_host.update(cx, |host, cx| {
                            if host
                                .popup
                                .is_some_and(|popup| popup.window_id() == window_id)
                            {
                                host.close(cx);
                                host.changed(cx);
                            }
                        });
                    })
                    .detach();
                    false
                });
            }
            let view = cx.new(|cx| ShellView::new(host, kind, output, state, cx));
            view.update(cx, |view, cx| view.observe_activation(window, cx));
            view.read(cx).focus(window, cx);
            view
        }) {
            Ok(window) => self.popup = Some(window),
            Err(error) => {
                self.error = Some(format!("无法打开面板：{error}"));
                self.panel = None;
            }
        }
    }
    fn show_toast(&mut self, id: u32, cx: &mut Context<Self>) {
        if self.toast_id == Some(id) {
            return;
        }
        self.toast_id = Some(id);
        if let Some(old) = self.toast.take() {
            let _ = old.update(cx, |_, window, _| window.remove_window());
        }
        let output = self.active_output();
        let display = self.outputs.get(&output).map(|v| v.display);
        let host = cx.entity();
        let bounds = Bounds::centered(display, size(px(360.), px(124.)), cx);
        let state = Arc::new(self.snapshot());
        self.toast = cx
            .open_window(
                window_options(
                    "clarion-notification",
                    display,
                    bounds,
                    self.preview,
                    LayerShellAnchor {
                        top: true,
                        right: true,
                        ..Default::default()
                    },
                    0,
                    LayerShellKeyboardInteractivity::None,
                    LayerShellMargins {
                        top: self.prefs.bar_height as i32 + 14,
                        right: 14,
                        ..Default::default()
                    },
                    LayerShellLayer::Overlay,
                ),
                move |_, cx| {
                    cx.new(|cx| ShellView::new(host, Surface::Toast(id), output, state, cx))
                },
            )
            .ok();
        let host = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            cx.background_executor().timer(Duration::from_secs(6)).await;
            let _ = host.update(cx, |host, cx| {
                if host.toast_id == Some(id) {
                    host.toast_id = None;
                    if let Some(old) = host.toast.take() {
                        let _ = old.update(cx, |_, window, _| window.remove_window());
                    }
                }
            });
        })
        .detach();
    }
    pub fn active_output(&self) -> String {
        self.desktop
            .workspaces
            .iter()
            .find(|w| w.is_focused)
            .and_then(|w| w.output.clone())
            .or_else(|| self.outputs.keys().next().cloned())
            .unwrap_or_default()
    }
    pub fn pinned(&self) -> Vec<String> {
        self.prefs.pinned_app_ids.clone().unwrap_or_else(|| {
            let mut result = vec![];
            for terms in [
                ["file", "nautilus", "dolphin"],
                ["terminal", "kitty", "alacritty"],
                ["browser", "firefox", "chrome"],
            ] {
                if let Some(app) = self.catalog.apps().iter().find(|app| {
                    terms
                        .iter()
                        .any(|term| app.id.to_lowercase().contains(term))
                }) {
                    if !result.contains(&app.id) {
                        result.push(app.id.clone());
                    }
                }
            }
            result
        })
    }
    pub fn dock_apps(&self) -> Vec<DockApp> {
        let mut ids = self.pinned();
        for window in &self.desktop.windows {
            let id = self
                .catalog
                .apps()
                .iter()
                .find(|a| {
                    a.id.trim_end_matches(".desktop")
                        .eq_ignore_ascii_case(window.app_id.as_deref().unwrap_or_default())
                })
                .map(|a| a.id.clone())
                .unwrap_or_else(|| format!("window:{}", window.id));
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        append_recent_dock_apps(
            &mut ids,
            &self.prefs.recent_app_ids,
            &self.catalog,
            self.prefs.show_recent_applications,
        );
        let pins = self.pinned();
        ids.into_iter()
            .map(|id| {
                let app = self.catalog.get(&id);
                let windows = self
                    .desktop
                    .windows
                    .iter()
                    .filter(|w| {
                        if let Some(raw) = id.strip_prefix("window:") {
                            raw.parse::<u64>().ok() == Some(w.id)
                        } else {
                            w.app_id.as_deref().is_some_and(|wid| {
                                wid.eq_ignore_ascii_case(id.trim_end_matches(".desktop"))
                            })
                        }
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                DockApp {
                    id: id.clone(),
                    name: app.map(|a| a.name.clone()).unwrap_or_else(|| {
                        if windows.first().and_then(|window| window.app_id.as_deref())
                            == Some("io.github.clarion.DesktopPreferences")
                        {
                            return "桌面与程序坞".into();
                        }
                        windows
                            .first()
                            .and_then(|w| w.app_id.clone())
                            .unwrap_or_else(|| "应用".into())
                    }),
                    icon: if windows.first().and_then(|window| window.app_id.as_deref())
                        == Some("io.github.clarion.DesktopPreferences")
                    {
                        themed_dock_icons().get("generic-application.svg").cloned()
                    } else if self.prefs.use_original_icons {
                        app.and_then(|app| app.icon_path.clone())
                    } else {
                        let name = app
                            .map(|app| app.id.as_str())
                            .or_else(|| windows.first().and_then(|window| window.app_id.as_deref()))
                            .unwrap_or(id.as_str());
                        themed_dock_icons()
                            .get(dock_icon_name(name, app.map(|app| app.category)))
                            .cloned()
                    },
                    windows,
                    pinned: pins.contains(&id),
                }
            })
            .collect()
    }
    pub fn activate_app(&mut self, app: &DockApp, cx: &mut Context<Self>) {
        if let Some(window) = app
            .windows
            .iter()
            .find(|w| !w.is_focused)
            .or_else(|| app.windows.first())
        {
            self.handle(Action::Focus(window.id), cx)
        } else {
            self.handle(Action::Launch(app.id.clone()), cx)
        }
    }
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        self.dock_hover = false;
        self.dock_edge_hover = false;
        for (_, v) in std::mem::take(&mut self.outputs) {
            let _ = v.bar.update(cx, |_, w, _| w.remove_window());
            if let Some(d) = v.dock {
                let _ = d.update(cx, |_, w, _| w.remove_window());
            }
            if let Some(d) = v.desktop {
                let _ = d.update(cx, |_, w, _| w.remove_window());
            }
            if let Some(edge) = v.edge {
                let _ = edge.update(cx, |_, w, _| w.remove_window());
            }
        }
        self.reconcile_outputs(cx);
    }
    fn reconcile_outputs(&mut self, cx: &mut Context<Self>) {
        let displays = cx.displays();
        let requested = self.options.output_name.clone();
        let mut live = vec![];
        for display in displays {
            let name = self
                .desktop
                .outputs
                .keys()
                .find(|name| {
                    display.uuid().ok()
                        == Some(uuid::Uuid::new_v5(
                            &uuid::Uuid::NAMESPACE_DNS,
                            name.as_bytes(),
                        ))
                })
                .cloned()
                .unwrap_or_else(|| format!("output-{:?}", display.id()));
            if requested.as_deref().is_some_and(|r| r != name) {
                continue;
            }
            live.push(name.clone());
            let (width, height) = output_dimensions(
                self.desktop
                    .outputs
                    .get(&name)
                    .and_then(|output| output.logical),
                display.bounds().size,
            );
            let changed = self.outputs.get(&name).is_some_and(|view| {
                view.width != width || view.height != height || view.display != display.id()
            });
            if changed {
                self.remove_output(&name, cx);
            }
            if self.outputs.contains_key(&name) {
                continue;
            }
            let did = Some(display.id());
            let host = cx.entity();
            let output = name.clone();
            let bh = self.prefs.bar_height as f32;
            let state = Arc::new(self.snapshot());
            let bar = cx.open_window(
                window_options(
                    "clarion-shell",
                    did,
                    Bounds::new(point(px(0.), px(0.)), size(px(width), px(bh))),
                    self.preview,
                    LayerShellAnchor {
                        top: true,
                        left: true,
                        right: true,
                        ..Default::default()
                    },
                    bh as i32,
                    LayerShellKeyboardInteractivity::None,
                    LayerShellMargins::default(),
                    LayerShellLayer::Top,
                ),
                move |_, cx| cx.new(|cx| ShellView::new(host, Surface::Bar, output, state, cx)),
            );
            let Ok(bar) = bar else { continue };
            let dock = if self.prefs.dock_hidden || self.auto_hidden {
                None
            } else {
                self.create_dock(&name, display.id(), width, height, cx)
            };
            let reserved_top = bh;
            let reserved_bottom = if self.prefs.dock_position == DockPosition::Bottom
                && !self.prefs.dock_auto_hide
                && !self.prefs.dock_hidden
            {
                self.prefs.dock_size as f32 + 40.
            } else {
                0.
            };
            let reserved_left = if self.prefs.dock_position == DockPosition::Left
                && !self.prefs.dock_auto_hide
                && !self.prefs.dock_hidden
            {
                self.prefs.dock_size as f32 + 40.
            } else {
                0.
            };
            let reserved_right = if self.prefs.dock_position == DockPosition::Right
                && !self.prefs.dock_auto_hide
                && !self.prefs.dock_hidden
            {
                self.prefs.dock_size as f32 + 40.
            } else {
                0.
            };
            let desktop = if self.preview || self.options.no_wallpaper {
                None
            } else {
                let environment = self.environment.clone();
                let actions = self.desktop_actions.clone();
                let output = name.clone();
                cx.open_window(
                    window_options(
                        "clarion-desktop",
                        did,
                        Bounds::new(point(px(0.), px(0.)), size(px(width), px(height))),
                        false,
                        LayerShellAnchor::ALL,
                        -1,
                        LayerShellKeyboardInteractivity::OnDemand,
                        LayerShellMargins::default(),
                        LayerShellLayer::Background,
                    ),
                    move |_, cx| {
                        cx.new(|cx| {
                            let mut view =
                                DesktopView::new(output, environment, actions, false, cx);
                            view.set_reserved_edges(
                                reserved_top,
                                reserved_bottom,
                                reserved_left,
                                reserved_right,
                                cx,
                            );
                            view
                        })
                    },
                )
                .ok()
            };
            let edge = if self.prefs.dock_auto_hide && !self.prefs.dock_hidden {
                let host = cx.entity();
                let state = Arc::new(self.snapshot());
                let output = name.clone();
                let bottom = self.prefs.dock_position == DockPosition::Bottom;
                let edge_bounds = Bounds::centered(
                    did,
                    if bottom {
                        size(px(width), px(2.))
                    } else {
                        size(px(2.), px(height))
                    },
                    cx,
                );
                cx.open_window(
                    window_options(
                        "clarion-dock-edge",
                        did,
                        edge_bounds,
                        self.preview,
                        LayerShellAnchor {
                            bottom,
                            top: !bottom,
                            left: self.prefs.dock_position == DockPosition::Left,
                            right: self.prefs.dock_position == DockPosition::Right,
                        },
                        0,
                        LayerShellKeyboardInteractivity::None,
                        LayerShellMargins::default(),
                        LayerShellLayer::Top,
                    ),
                    move |_, cx| {
                        cx.new(|cx| ShellView::new(host, Surface::DockEdge, output, state, cx))
                    },
                )
                .ok()
            } else {
                None
            };
            self.outputs.insert(
                name,
                OutputViews {
                    bar,
                    dock,
                    edge,
                    desktop,
                    display: display.id(),
                    width,
                    height,
                },
            );
        }
        for name in self
            .outputs
            .keys()
            .filter(|name| !live.contains(name))
            .cloned()
            .collect::<Vec<_>>()
        {
            self.remove_output(&name, cx);
        }
    }
    fn remove_output(&mut self, name: &str, cx: &mut Context<Self>) {
        if let Some(v) = self.outputs.remove(name) {
            let _ = v.bar.update(cx, |_, window, _| window.remove_window());
            if let Some(dock) = v.dock {
                let _ = dock.update(cx, |_, window, _| window.remove_window());
            }
            if let Some(edge) = v.edge {
                let _ = edge.update(cx, |_, window, _| window.remove_window());
            }
            if let Some(desktop) = v.desktop {
                let _ = desktop.update(cx, |_, window, _| window.remove_window());
            }
        }
    }
    fn create_dock(
        &self,
        name: &str,
        display: DisplayId,
        width: f32,
        height: f32,
        cx: &mut Context<Self>,
    ) -> Option<WindowHandle<ShellView>> {
        let n =
            self.dock_apps().len().min(18) as f32 + 1. + u8::from(self.prefs.show_launcher) as f32;
        let item = self.prefs.dock_size as f32 + 12.;
        let vertical = self.prefs.dock_position != DockPosition::Bottom;
        let length = (n * item + 20.).min(if vertical { height - 90. } else { width - 32. });
        let thickness = self.prefs.dock_size as f32 + 32.;
        let mut anchor = LayerShellAnchor::default();
        match self.prefs.dock_position {
            DockPosition::Bottom => anchor.bottom = true,
            DockPosition::Left => anchor.left = true,
            DockPosition::Right => anchor.right = true,
        };
        let size = if vertical {
            size(px(thickness + 12.), px(length + 24.))
        } else {
            size(px(length + 24.), px(thickness + 12.))
        };
        let bounds = Bounds::centered(Some(display), size, cx);
        let host = cx.entity();
        let output = name.to_owned();
        let state = Arc::new(self.snapshot());
        cx.open_window(
            window_options(
                "clarion-dock",
                Some(display),
                bounds,
                self.preview,
                anchor,
                if self.prefs.dock_auto_hide {
                    0
                } else {
                    thickness as i32 + 8
                },
                LayerShellKeyboardInteractivity::None,
                LayerShellMargins {
                    bottom: if self.prefs.dock_position == DockPosition::Bottom {
                        8
                    } else {
                        0
                    },
                    left: if self.prefs.dock_position == DockPosition::Left {
                        8
                    } else {
                        0
                    },
                    right: if self.prefs.dock_position == DockPosition::Right {
                        8
                    } else {
                        0
                    },
                    top: 0,
                },
                LayerShellLayer::Top,
            ),
            move |_, cx| cx.new(|cx| ShellView::new(host, Surface::Dock, output, state, cx)),
        )
        .ok()
    }
    fn tick(&mut self, cx: &mut Context<Self>) {
        self.reconcile_outputs(cx);
        if self.prefs.dock_auto_hide
            && !self.auto_hidden
            && !self.dock_hover
            && !self.dock_edge_hover
            && self.panel.is_none()
            && self.idle_since.elapsed() > Duration::from_millis(850)
        {
            self.auto_hidden = true;
            self.reconcile_docks(cx);
            self.targets();
        }
        if !self.ready && !self.outputs.is_empty() {
            self.ready = true;
            let preview = self.preview;
            let host = cx.entity().downgrade();
            let window = self.outputs.values().next().unwrap().bar;
            let _ = window.update(cx, move |_, w, _| {
                w.on_next_frame(move |_, cx| {
                    let _ = host.update(cx, |host, _| {
                        if !preview {
                            notify_ready();
                            host.core(CoreAction::SetReducedMotion {
                                enabled: host.prefs.reduce_motion,
                                platform: false,
                                initialize_only: true,
                            });
                        }
                    });
                });
            });
        }
        if self
            .options
            .quit_after
            .is_some_and(|seconds| self.idle_since.elapsed() > Duration::from_secs(seconds))
        {
            cx.quit();
        }
    }
    fn reconcile_docks(&mut self, cx: &mut Context<Self>) {
        let keys = self.outputs.keys().cloned().collect::<Vec<_>>();
        for name in keys {
            let (display, width, height, current) = {
                let v = &self.outputs[&name];
                (v.display, v.width, v.height, v.dock)
            };
            if self.auto_hidden || self.prefs.dock_hidden {
                if let Some(window) = current {
                    let _ = window.update(cx, |_, w, _| w.remove_window());
                    self.outputs.get_mut(&name).unwrap().dock = None;
                }
            } else if current.is_none() {
                let dock = self.create_dock(&name, display, width, height, cx);
                self.outputs.get_mut(&name).unwrap().dock = dock;
            }
        }
    }
    fn targets(&mut self) {
        let apps = self.dock_apps();
        let mut next = HashMap::new();
        if self.prefs.minimize_into_app && !self.prefs.dock_hidden && !self.auto_hidden {
            for (name, output) in &self.outputs {
                let vertical = self.prefs.dock_position != DockPosition::Bottom;
                let item = self.prefs.dock_size + 12.;
                let thickness = self.prefs.dock_size + 32.;
                let length =
                    ((apps.len().min(18) as f64 + 1. + u8::from(self.prefs.show_launcher) as f64)
                        * item
                        + 20.)
                        .min(if vertical {
                            output.height as f64 - 90.
                        } else {
                            output.width as f64 - 32.
                        });
                let visible = dock_visible_count(
                    length as f32,
                    self.prefs.dock_size as f32,
                    self.prefs.show_launcher,
                );
                for (index, app) in apps.iter().take(visible).enumerate() {
                    let offset = 10.
                        + (index as f64 + u8::from(self.prefs.show_launcher) as f64) * item
                        + 6.;
                    let (x, y) = match self.prefs.dock_position {
                        DockPosition::Bottom => (
                            (output.width as f64 - length) / 2. + offset,
                            output.height as f64 - 8. - thickness + 13.5,
                        ),
                        DockPosition::Left => {
                            (24., (output.height as f64 - length) / 2. + offset - 2.5)
                        }
                        DockPosition::Right => (
                            output.width as f64 - 8. - thickness + 16.,
                            (output.height as f64 - length) / 2. + offset - 2.5,
                        ),
                    };
                    if offset + self.prefs.dock_size > length {
                        continue;
                    }
                    for window in &app.windows {
                        let workspace = self
                            .desktop
                            .workspaces
                            .iter()
                            .find(|w| Some(w.id) == window.workspace_id);
                        if workspace.and_then(|w| w.output.as_deref()) != Some(name.as_str()) {
                            continue;
                        }
                        next.insert(
                            window.id,
                            DockTarget {
                                output: name.clone(),
                                x,
                                y,
                                width: self.prefs.dock_size,
                                height: self.prefs.dock_size,
                                edge: match self.prefs.dock_position {
                                    DockPosition::Bottom => DockEdge::Bottom,
                                    DockPosition::Left => DockEdge::Left,
                                    DockPosition::Right => DockEdge::Right,
                                },
                                effect: if self.prefs.minimize_effect
                                    == crate::preferences::MinimizeEffect::Scale
                                {
                                    MinimizeEffect::Scale
                                } else {
                                    MinimizeEffect::Genie
                                },
                            },
                        );
                    }
                }
            }
        }
        for (id, target) in &next {
            if self.last_target.get(id) != Some(target) {
                self.core(CoreAction::SetMinimizeTarget {
                    id: *id,
                    target: Some(target.clone()),
                    reduce_motion: self.prefs.reduce_motion,
                });
            }
        }
        for id in self.last_target.keys() {
            if !next.contains_key(id) {
                self.core(CoreAction::SetMinimizeTarget {
                    id: *id,
                    target: None,
                    reduce_motion: self.prefs.reduce_motion,
                });
            }
        }
        self.last_target = next;
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        for id in self.last_target.keys() {
            self.core(CoreAction::SetMinimizeTarget {
                id: *id,
                target: None,
                reduce_motion: self.prefs.reduce_motion,
            });
        }
        self.service.send(services::Command::Shutdown);
    }
}
fn notify_ready() {
    if let Ok(path) = std::env::var("NOTIFY_SOCKET") {
        let _ = std::thread::Builder::new()
            .name("clarion-ready".into())
            .spawn(move || {
                if let Ok(socket) = std::os::unix::net::UnixDatagram::unbound() {
                    if let Some(name) = path.strip_prefix('@') {
                        use std::os::linux::net::SocketAddrExt;
                        let address = std::os::unix::net::SocketAddr::from_abstract_name(name);
                        if let Ok(address) = address {
                            let _ = socket.send_to_addr(b"READY=1", &address);
                        }
                    } else {
                        let _ = socket.send_to(b"READY=1", path);
                    }
                }
            });
    }
}
// The compositor's logical geometry is authoritative after a mode/scale change;
// a toolkit display cache may retain the first advertised dimensions.
fn output_dimensions(
    logical: Option<clarion_ipc::LogicalOutput>,
    fallback: Size<Pixels>,
) -> (f32, f32) {
    logical
        .filter(|output| output.width > 0 && output.height > 0)
        .map(|output| (output.width as f32, output.height as f32))
        .unwrap_or((f32::from(fallback.width), f32::from(fallback.height)))
}

// Mirror the layer-shell role declaration at each call site.
#[allow(clippy::too_many_arguments)]
fn window_options(
    namespace: &'static str,
    display: Option<DisplayId>,
    bounds: Bounds<Pixels>,
    preview: bool,
    anchor: LayerShellAnchor,
    zone: i32,
    keyboard: LayerShellKeyboardInteractivity,
    margins: LayerShellMargins,
    layer: LayerShellLayer,
) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        kind: if preview {
            WindowKind::Normal
        } else {
            WindowKind::LayerShell(LayerShellOptions {
                namespace,
                output: display,
                layer,
                anchor,
                exclusive_zone: zone,
                keyboard_interactivity: keyboard,
                margins,
                keep_alive: true,
            })
        },
        display_id: display,
        window_background: if preview
            || matches!(namespace, "clarion-desktop" | "clarion-dock-edge")
        {
            WindowBackgroundAppearance::Transparent
        } else {
            WindowBackgroundAppearance::Blurred
        },
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        app_id: Some("io.github.clarion.Shell".into()),
        ..Default::default()
    }
}

struct ShellAssets;
impl AssetSource for ShellAssets {
    fn load(&self, path: &str) -> anyhow::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        let name = std::path::Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let bytes: &'static [u8] = match name {
            "grid.svg" => include_bytes!("../assets/icons/grid.svg"),
            "x.svg" => include_bytes!("../assets/icons/x.svg"),
            "clipboard.svg" => include_bytes!("../assets/icons/clipboard.svg"),
            "scan.svg" => include_bytes!("../assets/icons/scan.svg"),
            "sliders.svg" => include_bytes!("../assets/icons/sliders.svg"),
            "bell.svg" => include_bytes!("../assets/icons/bell.svg"),
            "circle.svg" => include_bytes!("../assets/icons/circle.svg"),
            "control-center.svg" => include_bytes!("../assets/icons/control-center.svg"),
            "moon.svg" => include_bytes!("../assets/icons/moon.svg"),
            "capture-screen.svg" => include_bytes!("../assets/icons/capture-screen.svg"),
            "capture-window.svg" => include_bytes!("../assets/icons/capture-window.svg"),
            "capture-region.svg" => include_bytes!("../assets/icons/capture-region.svg"),
            "sun.svg" => include_bytes!("../assets/icons/sun.svg"),
            "speaker.svg" => include_bytes!("../assets/icons/speaker.svg"),
            "speaker-muted.svg" => include_bytes!("../assets/icons/speaker-muted.svg"),
            "display.svg" => include_bytes!("../assets/icons/display.svg"),
            "settings.svg" => include_bytes!("../assets/icons/settings.svg"),
            "wallpaper.svg" => include_bytes!("../assets/icons/wallpaper.svg"),
            _ => return Ok(None),
        };
        Ok(Some(std::borrow::Cow::Borrowed(bytes)))
    }
    fn list(&self, _path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(vec![])
    }
}

#[cfg(test)]
pub(crate) mod testing;

/// Share the exact icon budget between rendering and minimize destinations.
pub fn dock_visible_count(length: f32, icon_size: f32, show_launcher: bool) -> usize {
    let fixed = 1 + usize::from(show_launcher);
    (((length - 20.) / (icon_size + 12.)).floor().max(0.) as usize).saturating_sub(fixed)
}

fn append_recent_dock_apps(
    ids: &mut Vec<String>,
    recent: &[String],
    catalog: &AppCatalog,
    enabled: bool,
) {
    if !enabled {
        return;
    }
    let mut added = 0;
    for id in recent {
        if catalog.get(id).is_some() && !ids.contains(id) {
            ids.push(id.clone());
            added += 1;
            if added == 3 {
                break;
            }
        }
    }
}

fn dock_icon_name(id: &str, category: Option<AppCategory>) -> &'static str {
    let id = id.to_ascii_lowercase();
    let has = |terms: &[&str]| terms.iter().any(|term| id.contains(term));
    if has(&[
        "firefox", "chromium", "chrome", "brave", "vivaldi", "safari", "epiphany", "browser",
        "falkon",
    ]) {
        "safari.svg"
    } else if has(&[
        "terminal",
        "kitty",
        "alacritty",
        "konsole",
        "wezterm",
        "ghostty",
        "tilix",
    ]) || id.split(['.', '-', '_']).any(|part| part == "foot")
    {
        "terminal.svg"
    } else if has(&[
        "nautilus",
        "dolphin",
        "thunar",
        "nemo",
        "pcmanfm",
        "filemanager",
        "finder",
    ]) {
        "home.svg"
    } else if has(&["calendar", "korganizer"]) {
        "calendar.svg"
    } else if has(&["calculator", "kcalc", "galculator"]) {
        "calculator.svg"
    } else if has(&["screenshot", "spectacle", "flameshot"]) {
        "screenshot.png"
    } else if has(&["font-manager", "fontviewer", "fontbook"]) {
        "font-book.svg"
    } else if has(&["monitor", "btop", "htop", "sysprof"]) {
        "activity-monitor.png"
    } else if has(&[
        "evince",
        "okular",
        "zathura",
        "gwenview",
        "loupe",
        "imageviewer",
        "pdfviewer",
    ]) {
        "preview.png"
    } else if has(&[
        "textedit", "gedit", "kwrite", "kate", "vscode", "neovim", "emacs", "geany", "sublime",
    ]) || id.split(['.', '-', '_']).any(|part| part == "code")
    {
        "textedit.svg"
    } else {
        match category {
            Some(AppCategory::ProductivityFinance) => "textedit.svg",
            Some(AppCategory::Creativity) => "preview.png",
            Some(AppCategory::InformationReading) => "safari.svg",
            _ => "generic-application.svg",
        }
    }
}

fn dock_icon_roots(executable: Option<&std::path::Path>) -> Vec<std::path::PathBuf> {
    let mut roots = Vec::new();
    if let Some(prefix) = executable
        .and_then(|path| path.parent())
        .and_then(|path| path.parent())
    {
        roots.push(prefix.join("assets/icons"));
        roots.push(prefix.join("share/clarion-shell/icons"));
    }
    roots.extend([
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/icons"),
        std::path::PathBuf::from("/usr/share/clarion-shell/icons"),
        std::path::PathBuf::from("/usr/local/share/clarion-shell/icons"),
    ]);
    roots
}

fn resolve_dock_icon(name: &str, roots: &[std::path::PathBuf]) -> Option<std::path::PathBuf> {
    roots
        .iter()
        .map(|root| root.join(name))
        .find(|path| path.is_file())
}

fn themed_dock_icons() -> &'static HashMap<&'static str, std::path::PathBuf> {
    // Installed releases are immutable. Resolve the small known asset set once;
    // hovering the Dock and receiving desktop snapshots never rescan files.
    static ICONS: std::sync::OnceLock<HashMap<&'static str, std::path::PathBuf>> =
        std::sync::OnceLock::new();
    ICONS.get_or_init(|| {
        let executable = std::env::current_exe().ok();
        let roots = dock_icon_roots(executable.as_deref());
        [
            "generic-application.svg",
            "safari.svg",
            "terminal.svg",
            "home.svg",
            "textedit.svg",
            "calculator.svg",
            "calendar.svg",
            "font-book.svg",
            "preview.png",
            "activity-monitor.png",
            "screenshot.png",
        ]
        .into_iter()
        .filter_map(|name| resolve_dock_icon(name, &roots).map(|path| (name, path)))
        .collect()
    })
}

#[cfg(test)]
mod dock_preferences_tests;
