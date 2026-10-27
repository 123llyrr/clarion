// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{
    backend,
    desktop::{DesktopAction, DesktopView},
    preferences::{
        DockFolderPreference, DockPosition, FolderDisplay, FolderLayout, FolderSort,
        LauncherPresentation, Preferences,
    },
    services,
    view::{ShellView, Surface},
    Options,
};
use clarion_ipc::{Action as CoreAction, DockEdge, DockTarget, MinimizeEffect};
use clarion_launcher::catalog::{AppCatalog, AppCategory, DesktopApp};
use clarion_launcher::usage::{unix_millis, LaunchHistory};
use gpui::{prelude::*, *};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

mod dock_folders;
pub(crate) mod panels;
#[cfg(test)]
mod dock_folder_tests;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Panel {
    Apps,
    /// The compact launcher palette: scored search, modes and tools over the
    /// same catalog the fullscreen Launchpad shows.
    Spotlight,
    Recent,
    Notifications,
    Controls,
    Preferences,
    Screenshot,
    Tray(String),
    Dock(String),
    /// A pinned folder's contents. The root identifies the Dock tile the popup
    /// points at; navigation state lives in `ViewState::folder_navigation`.
    DockFolder { root: PathBuf },
    /// Configuration for one pinned folder.
    DockFolderMenu { path: PathBuf },
    /// The applications folded into the Dock's overflow tile, plus the fitted
    /// icon size (a discrete ladder step) the Dock row is drawing so the drawer
    /// tiles match it.
    DockOverflow { ids: Vec<String>, icon_size: u16 },
}
#[derive(Clone)]
pub enum Action {
    Panel(Panel),
    /// A bar-anchored opener: the panel plus the pointer x inside the bar's
    /// own coordinate space, so its popup drops from the clicked icon instead
    /// of the resting screen edge.
    AnchoredPanel(Panel, f32),
    /// A surface-dispatched action that names the monitor it came from. A
    /// panel it opens belongs to that monitor even when keyboard focus rests
    /// on another one (a bar or Dock on a secondary output is still clickable
    /// without focusing it).
    OnOutput(String, Box<Action>),
    Close,
    Launch(String),
    Focus(u64),
    Minimize(u64),
    CloseWindow(u64),
    PinApp(String),
    /// The new order of the pinned Dock applications.
    DockOrder(Vec<String>),
    PinFolder(PathBuf),
    UnpinFolder(PathBuf),
    FolderOrder(Vec<String>),
    OpenFolder(PathBuf),
    NavigateFolder(PathBuf),
    FolderBack,
    RevealFolder(PathBuf),
    RefreshFolder(PathBuf),
    SetFolderDisplay(PathBuf, FolderDisplay),
    SetFolderLayout(PathBuf, FolderLayout),
    SetFolderSort(PathBuf, FolderSort),
    SetFolderDescending(PathBuf, bool),
    /// Hold open (or release) one extra folder slot for an external drop.
    FolderDropPreview(bool),
    OpenPathsWithApp { app_id: String, paths: Vec<PathBuf> },
    /// Open a web URL through the desktop handler (the `/web` page).
    OpenUrl(String),
    Core(CoreAction),
    Service(services::Command),
    Backend(backend::Command),
    /// Toggle one tray submenu's expansion. The rows live in the shell, so a
    /// parent row expands locally instead of asking the application.
    MenuExpand(i32),
    Theme(String),
    Accent(String),
    DockSize(f64),
    DockOpacity(f64),
    SurfaceOpacity(f64),
    FrostStrength(f64),
    BarHeight(f64),
    DockPosition(DockPosition),
    AutoHide(bool),
    Magnification(bool),
    ShowRecentApplications(bool),
    LauncherPresentation(LauncherPresentation),
    /// Open the launcher in whichever presentation the preferences select.
    Launcher,
    /// Open one launcher tile's application menu as a surface over the
    /// launcher. The launcher stays open underneath.
    OpenMenu { id: String, anchor: (i32, i32) },
    /// Fold the launcher's application menu away and hand the keyboard back to
    /// the launcher it belongs to.
    CloseMenu,
    MagnificationScale(f64),
    AnimateOpeningApplications(bool),
    ReduceMotion(bool),
    Screenshot(u8),
    Settings,
    /// Run one palette command row, addressed by its stable id. The session
    /// lives in the popup view, so the click routes back through the host.
    PaletteCommand(String),
    /// Run one row of the `/actions` catalog by its stable id.
    RunCatalog(String),
    DockHover(bool),
    DockEdgeHover(bool),
    DoNotDisturb(bool),
    PickWallpaper,
    /// Lock, suspend or power the session off. The worker owns the bus
    /// round-trip so a slow logind/systemd never blocks the UI.
    System(SystemAction),
}

/// The session power actions the launcher's action catalog offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemAction {
    Lock,
    Logout,
    Suspend,
    Reboot,
    Shutdown,
}
impl Action {
    /// Tag a panel opener with the monitor whose surface dispatched it, so a
    /// click on a secondary bar or Dock still drops its popup on that monitor
    /// instead of following keyboard focus.
    pub fn on_output(output: impl Into<String>, action: Action) -> Self {
        Action::OnOutput(output.into(), Box::new(action))
    }
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
pub struct DockFolderView {
    pub path: PathBuf,
    pub name: String,
    pub display: FolderDisplay,
    pub layout: FolderLayout,
    pub sort: FolderSort,
    pub descending: bool,
    pub snapshot: Option<Arc<crate::dock_folder::FolderSnapshot>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderNavigation {
    pub root: PathBuf,
    pub current: PathBuf,
    pub history: Vec<PathBuf>,
}
#[derive(Clone)]
pub struct ViewState {
    pub prefs: Preferences,
    pub desktop: backend::DesktopSnapshot,
    pub system: backend::SystemSnapshot,
    pub catalog: Arc<AppCatalog>,
    /// Launch counts used by the palette's usage-aware ordering.
    pub launch_history: Arc<LaunchHistory>,
    pub notifications: Vec<services::Notification>,
    pub tray: Vec<services::TrayItem>,
    pub history: Vec<services::HistoryEntry>,
    pub clipboard_available: bool,
    pub clipboard_paused: bool,
    pub tray_menu: Vec<services::MenuItem>,
    /// The tray submenu ids the user expanded in the current popup. Submenus
    /// open collapsed; the state dies with the popup.
    pub expanded_menus: Vec<i32>,
    /// Submenu ids currently fading out; they stay in `expanded_menus` until
    /// their card animation has run so the popup keeps its size while fading.
    pub collapsing_menus: Vec<i32>,
    pub error: Option<String>,
    pub panel: Option<Panel>,
    /// The output whose bar or Dock opened the current panel. The fullscreen
    /// launcher hides only this output's Dock; other monitors keep theirs.
    pub panel_output: Option<String>,
    pub dnd: bool,
    pub dock_revealed: bool,
    pub launches: HashMap<String, Instant>,
    pub dock_folders: Vec<DockFolderView>,
    /// A folder is being dragged over the tray, so the Dock reserves one more
    /// folder slot before the trash instead of covering an existing tile.
    pub folder_drop_preview: bool,
    pub folder_navigation: Option<FolderNavigation>,
    pub folder_snapshots: HashMap<PathBuf, Arc<crate::dock_folder::FolderSnapshot>>,
    pub folder_errors: HashMap<PathBuf, String>,
    /// Bar-space x of the last anchored bar click (a tray icon or the control
    /// center button); `None` drops its popup at the resting right edge.
    pub anchored: Option<f32>,
    /// The popup surface is being kept alive for its closing fade.
    pub panel_exit: bool,
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
    pub launch_history: Arc<LaunchHistory>,
    pub notifications: Vec<services::Notification>,
    pub tray: Vec<services::TrayItem>,
    pub history: Vec<services::HistoryEntry>,
    pub clipboard_available: bool,
    pub clipboard_paused: bool,
    pub tray_menu: Vec<services::MenuItem>,
    /// The tray submenu ids the user expanded in the current popup. Submenus
    /// open collapsed; the state dies with the popup.
    pub expanded_menus: Vec<i32>,
    /// Submenu ids currently fading out; they stay in `expanded_menus` until
    /// their card animation has run so the popup keeps its size while fading.
    pub collapsing_menus: Vec<i32>,
    pub error: Option<String>,
    /// Lifetime generation of the error banner; each failure restarts its own
    /// release timer, and an older timer never erases a newer banner.
    pub error_generation: u64,
    pub panel: Option<Panel>,
    /// The output that owns the open panel, so per-output chrome such as the
    /// Dock can hide only where the launcher actually covers the screen.
    pub panel_output: Option<String>,
    /// Bar-space x of the last anchored bar click; mirrored into snapshots so
    /// `placement` can drop its popup from the clicked icon.
    pub(crate) anchored: Option<f32>,
    /// A popup is riding its closing fade: `panel` stays visible to the view
    /// while the animation runs, then the release clears it.
    pub(crate) panel_exit: bool,
    pub preview: bool,
    pub dnd: bool,
    /// Whether a folder surface is currently kept alive as an invisible drag layer.
    pub folder_drag_layer: bool,
    /// Whether an external folder drag is holding open an extra Dock slot.
    pub folder_drop_preview: bool,
    pub revision: u64,
    backend: backend::Backend,
    service: services::ServiceHandle,
    desktop_actions: async_channel::Sender<DesktopAction>,
    local_preferences: bool,
    environment: HashMap<String, String>,
    outputs: BTreeMap<String, OutputViews>,
    popup: Option<WindowHandle<ShellView>>,
    /// The application menu surface floating over the launcher. It is kept
    /// apart from `popup` so the launcher (and its draft) stays open underneath.
    pub(crate) menu: Option<WindowHandle<ShellView>>,
    /// A menu surface is being created right now. Mapping it takes the keyboard,
    /// and the launcher's deactivation must not read that as a click elsewhere
    /// before `menu` itself lands.
    pub(crate) menu_opening: bool,
    /// The monitor whose surface dispatched the action currently being
    /// applied. A bar or Dock click can land on a monitor that does not hold
    /// keyboard focus; its popup still belongs to that monitor, not to the
    /// focused one.
    dispatch_output: Option<String>,
    /// The platform window kept alive while a closing popup rides its fade
    /// out; superseded fades are cut short.
    exiting: Option<WindowHandle<ShellView>>,
    exit_generation: u64,
    toast: Option<WindowHandle<ShellView>>,
    toast_id: Option<u32>,
    options: Options,
    ready: bool,
    preferences_placed: Option<u64>,
    // Suppression is part of the registration, even when geometry is unchanged.
    last_target: HashMap<u64, (DockTarget, bool)>,
    targets_pending: bool,
    auto_hidden: bool,
    launches: HashMap<String, Instant>,
    dock_folder_cache: HashMap<PathBuf, Arc<crate::dock_folder::FolderSnapshot>>,
    dock_folder_errors: HashMap<PathBuf, String>,
    folder_navigation: Option<FolderNavigation>,
    folder_scan_generation: HashMap<PathBuf, u64>,
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
            let launch_history = Arc::new(LaunchHistory::load(&environment));
            let host = cx.new(|_| Host {
                prefs,
                desktop: Default::default(),
                system: Default::default(),
                catalog: Arc::new(AppCatalog::default()),
                launch_history,
                notifications: vec![],
                tray: vec![],
                history: vec![],
                clipboard_available: false,
                clipboard_paused: false,
                tray_menu: vec![],
                expanded_menus: vec![],
                collapsing_menus: vec![],
                error: None,
                error_generation: 0,
                panel: None,
                panel_output: None,
                anchored: None,
                panel_exit: false,
                preview,
                dnd: false,
                folder_drag_layer: false,
                folder_drop_preview: false,
                revision: 0,
                backend,
                service,
                desktop_actions,
                local_preferences: false,
                environment,
                outputs: BTreeMap::new(),
                popup: None,
                menu: None,
                menu_opening: false,
                dispatch_output: None,
                exiting: None,
                exit_generation: 0,
                toast: None,
                toast_id: None,
                options,
                ready: false,
                preferences_placed: None,
                last_target: HashMap::new(),
                targets_pending: false,
                auto_hidden,
                launches: HashMap::new(),
                dock_folder_cache: HashMap::new(),
                dock_folder_errors: HashMap::new(),
                folder_navigation: None,
                folder_scan_generation: HashMap::new(),
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

/// Record the labels of the expanded submenu parents whose id is listed.
fn collect_expanded_labels(
    items: &[services::MenuItem],
    expanded: &[i32],
    out: &mut Vec<String>,
) {
    for item in items {
        if item.visible
            && !item.children.is_empty()
            && expanded.contains(&item.id)
            && !out.contains(&item.label)
        {
            out.push(item.label.clone());
        }
        collect_expanded_labels(&item.children, expanded, out);
    }
}

/// Whether a refreshed tree still carries a parent with this id.
fn tree_has_parent(items: &[services::MenuItem], id: i32) -> bool {
    items.iter().any(|item| {
        (item.visible && !item.children.is_empty() && item.id == id)
            || tree_has_parent(&item.children, id)
    })
}

/// Expand refreshed parents that inherited an expanded label.
fn collect_label_parents(items: &[services::MenuItem], labels: &[String], out: &mut Vec<i32>) {
    for item in items {
        if item.visible
            && !item.children.is_empty()
            && labels.contains(&item.label)
            && !out.contains(&item.id)
        {
            out.push(item.id);
        }
        collect_label_parents(&item.children, labels, out);
    }
}

impl Host {
    fn snapshot(&self) -> ViewState {
        ViewState {
            prefs: self.prefs.clone(),
            desktop: self.desktop.clone(),
            system: self.system.clone(),
            catalog: self.catalog.clone(),
            launch_history: self.launch_history.clone(),
            notifications: self.notifications.clone(),
            tray: self.tray.clone(),
            history: self.history.clone(),
            clipboard_available: self.clipboard_available,
            clipboard_paused: self.clipboard_paused,
            tray_menu: self.tray_menu.clone(),
            expanded_menus: self.expanded_menus.clone(),
            collapsing_menus: self.collapsing_menus.clone(),
            error: self.error.clone(),
            panel: self.panel.clone(),
            panel_output: self.panel_output.clone(),
            dnd: self.dnd,
            dock_revealed: !self.auto_hidden,
            launches: self.launches.clone(),
            dock_folders: self.dock_folder_views(),
            folder_drop_preview: self.folder_drop_preview,
            folder_navigation: self.folder_navigation.clone(),
            folder_snapshots: self.dock_folder_cache.clone(),
            folder_errors: self.dock_folder_errors.clone(),
            anchored: self.anchored,
            panel_exit: self.panel_exit,
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
                // Keep an automatically hidden Dock visible through a real
                // minimize/restore transition. Its stable target must survive
                // hiding; clearing it would truncate an in-flight core warp.
                if self.prefs.dock_auto_hide
                    && snapshot.connected
                    && snapshot.generation == self.desktop.generation
                    && snapshot.windows.iter().any(|next| {
                        self.desktop
                            .windows
                            .iter()
                            .any(|old| old.id == next.id && old.is_minimized != next.is_minimized)
                    })
                {
                    self.auto_hidden = false;
                    self.idle_since = Instant::now();
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
                    if !prefs.dock_auto_hide {
                        self.auto_hidden = false;
                    }
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
                "launcher" => self.handle(Action::Launcher, cx),
                "spotlight" => self.handle(Action::Panel(Panel::Spotlight), cx),
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
            backend::Event::Error(error) => self.show_error(error, cx),
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
                if self
                    .toast_id
                    .is_some_and(|id| !self.notifications.iter().any(|item| item.id == id))
                {
                    self.toast_id = None;
                    if let Some(toast) = self.toast.take() {
                        let _ = toast.update(cx, |_, window, _| window.remove_window());
                    }
                }
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
                    self.tray_menu = self.remap_expanded(items);
                }
            }
            services::Event::Error(error) => self.show_error(error, cx),
        }
        self.changed(cx);
    }
    pub fn changed(&mut self, cx: &mut Context<Self>) {
        self.revision = self.revision.wrapping_add(1);
        cx.notify();
        let state = Arc::new(self.snapshot());
        for view in self.outputs.values() {
            let metrics = self.dock_metrics(view.width, view.height);
            let thickness = metrics.thickness;
            let dimensions = size(px(metrics.surface_width), px(metrics.surface_height));
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
                    desktop.set_appearance(self.prefs.surface_appearance(), cx);
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
                .filter(|panel| {
                    matches!(
                        panel,
                        Panel::Tray(_)
                            | Panel::Dock(_)
                            | Panel::DockFolder { .. }
                            | Panel::DockFolderMenu { .. }
                    )
                })
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
        if let Some(menu) = self.menu {
            // The card lists live windows, so it follows a refreshed snapshot
            // just like the launcher under it.
            let _ = menu.update(cx, |v, _, cx| {
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
            Action::OnOutput(output, action) => {
                // Bar and Dock surfaces live on every output. Carry the one
                // that dispatched the action while it applies, so its popup is
                // created there instead of on the keyboard-focused output.
                let previous = self.dispatch_output.replace(output);
                self.apply(*action, cx);
                self.dispatch_output = previous;
            }
            Action::Launcher => {
                self.anchored = None;
                let panel = match self.prefs.launcher_presentation {
                    LauncherPresentation::Fullscreen => Panel::Apps,
                    LauncherPresentation::Palette => Panel::Spotlight,
                };
                if self.panel.as_ref() == Some(&panel) {
                    self.close(cx);
                } else {
                    self.open(panel, cx);
                }
            }
            Action::Panel(panel) => {
                // A non-anchored opener drops its popup at the resting edge.
                self.anchored = None;
                if self.panel.as_ref() == Some(&panel) {
                    self.close(cx);
                } else {
                    self.open(panel, cx);
                }
            }
            Action::OpenMenu { id, anchor } => self.open_menu(id, anchor, cx),
            Action::CloseMenu => self.close_menu(cx),
            Action::AnchoredPanel(panel, x) => {
                self.anchored = Some(x);
                if self.panel.as_ref() == Some(&panel) {
                    self.close(cx);
                } else {
                    self.open(panel, cx);
                }
            }
            Action::Launch(id) => {
                if let Some(app) = self.catalog.get(&id) {
                    self.auto_hidden = false;
                    // Count the launch for the palette's smart ordering; the
                    // worker owns the atomic file write so the UI never blocks.
                    // The launch itself goes first: it is the command every
                    // existing observer reads.
                    Arc::make_mut(&mut self.launch_history).record_launch(&id, unix_millis());
                    let _ = self
                        .backend
                        .send(backend::Command::LaunchDesktop(app.desktop_file.clone()));
                    let _ = self.backend.send(backend::Command::RecordLaunch(id.clone()));
                    if self.prefs.animate_opening_applications && !self.prefs.reduce_motion {
                        let now = Instant::now();
                        let started = self.launches.entry(id.clone()).or_insert(now);
                        if now.duration_since(*started) >= Duration::from_secs(1) {
                            *started = now;
                        }
                    }
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
                let mut pins = self.pinned_raw();
                if pins.contains(&id) {
                    pins.retain(|item| item != &id);
                } else {
                    pins.push(id.clone());
                }
                // An unpinned application leaves the saved arrangement.
                if let Some(order) = &mut self.prefs.dock_order {
                    order.retain(|item| item != &id);
                }
                self.prefs.pinned_app_ids = Some(pins);
                self.save(cx);
                // The menu has served its purpose; dismiss it like the other
                // one-shot items.
                self.close(cx);
            }
            Action::DockOrder(order) => {
                let pins = self.pinned_raw();
                let mut seen = HashSet::new();
                let order: Vec<String> = order
                    .into_iter()
                    .filter(|id| pins.contains(id) && seen.insert(id.clone()))
                    .collect();
                self.prefs.dock_order = Some(order);
                self.save(cx);
            }
            Action::PinFolder(path) => self.pin_folder(path, cx),
            Action::UnpinFolder(path) => self.unpin_folder(&path, cx),
            Action::FolderOrder(order) => self.reorder_folders(order, cx),
            Action::OpenFolder(path) => {
                self.folder_navigation = Some(FolderNavigation {
                    root: path.clone(),
                    current: path.clone(),
                    history: Vec::new(),
                });
                self.request_folder_scan(path.clone(), cx);
                self.open(Panel::DockFolder { root: path }, cx);
            }
            Action::NavigateFolder(path) => self.navigate_folder(path, cx),
            Action::FolderBack => self.folder_back(cx),
            Action::RevealFolder(path) => {
                let _ = self.backend.send(backend::Command::OpenPath(path));
                self.close(cx);
            }
            Action::OpenUrl(url) => {
                let _ = self.backend.send(backend::Command::OpenUrl(url));
            }
            Action::RefreshFolder(path) => self.request_folder_scan(path, cx),
            Action::SetFolderDisplay(path, display) => {
                self.configure_folder(&path, |folder| folder.display = display, cx)
            }
            Action::SetFolderLayout(path, layout) => {
                self.configure_folder(&path, |folder| folder.layout = layout, cx)
            }
            Action::SetFolderSort(path, sort) => {
                self.configure_folder(&path, |folder| folder.sort = sort, cx)
            }
            Action::SetFolderDescending(path, descending) => {
                self.configure_folder(&path, |folder| folder.descending = descending, cx)
            }
            Action::FolderDropPreview(active) => {
                let active = active
                    && self.prefs.dock_folders.len() < crate::preferences::MAX_DOCK_FOLDERS;
                if self.folder_drop_preview != active {
                    self.folder_drop_preview = active;
                    self.changed(cx);
                }
            }
            Action::OpenPathsWithApp { app_id, paths } => {
                let Some(app) = self.catalog.get(&app_id) else {
                    return;
                };
                if !crate::mime::app_accepts_paths(app, &paths) {
                    return;
                }
                self.auto_hidden = false;
                let _ = self.backend.send(backend::Command::LaunchDesktopWithPaths(
                    app.desktop_file.clone(),
                    paths,
                ));
                self.prefs.recent_app_ids.retain(|other| other != &app_id);
                self.prefs.recent_app_ids.insert(0, app_id);
                self.prefs.recent_app_ids.truncate(24);
                self.save(cx);
                self.close(cx);
            }
            Action::Core(action) => self.core(action),
            Action::Service(command) => {
                self.service.send(command);
            }
            Action::MenuExpand(id) => {
                // A collapsed submenu expands and an expanded one folds back;
                // nothing reaches the application until a leaf is clicked. A
                // collapse rides its card fade out: the id leaves the expanded
                // set only when the release timer runs, so the popup keeps its
                // size while the card animates away.
                if self.expanded_menus.contains(&id) {
                    if !self.collapsing_menus.contains(&id) {
                        self.collapsing_menus.push(id);
                        let target = id;
                        cx.spawn(async move |host, cx| {
                            cx.background_executor()
                                .timer(Self::SUBMENU_FADE)
                                .await;
                            let _ = host.update(cx, |host, cx| {
                                host.finish_submenu_collapse(target, cx)
                            });
                        })
                        .detach();
                    }
                } else {
                    self.collapsing_menus.retain(|other| *other != id);
                    self.expanded_menus.push(id);
                    self.changed(cx);
                }
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
                // The Dock draws native icon assets only, so the preference
                // snaps to the 64 -> 48 -> 32 ladder instead of any size.
                self.prefs.dock_size =
                    crate::dock_motion::snap_icon_step(value.clamp(32., 64.) as f32) as f64;
                self.save(cx);
            }
            Action::DockOpacity(value) => {
                if value.is_finite() {
                    self.prefs.dock_opacity = value.clamp(
                        crate::preferences::MIN_SURFACE_OPACITY,
                        crate::preferences::MAX_SURFACE_OPACITY,
                    );
                    self.save(cx);
                }
            }
            Action::SurfaceOpacity(value) => {
                if value.is_finite() {
                    self.prefs.surface_opacity = value.clamp(
                        crate::preferences::MIN_SURFACE_OPACITY,
                        crate::preferences::MAX_SURFACE_OPACITY,
                    );
                    self.save(cx);
                }
            }
            Action::FrostStrength(value) => {
                if value.is_finite() {
                    self.prefs.frost_strength = value.clamp(0., 1.);
                    self.save(cx);
                }
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
            Action::Magnification(value) => {
                self.prefs.magnification = value;
                self.save(cx);
            }
            Action::ShowRecentApplications(value) => {
                self.prefs.show_recent_applications = value;
                self.save(cx);
                self.changed(cx);
            }
            Action::LauncherPresentation(value) => {
                if self.prefs.launcher_presentation != value {
                    self.prefs.launcher_presentation = value;
                    self.save(cx);
                    self.changed(cx);
                }
            }
            Action::MagnificationScale(value) => {
                if value.is_finite() {
                    self.prefs.magnification_scale = value.clamp(1., 2.);
                    self.save(cx);
                }
            }
            Action::AnimateOpeningApplications(value) => {
                self.prefs.animate_opening_applications = value;
                self.save(cx);
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
            Action::PaletteCommand(id) => {
                if let Some(popup) = self.popup {
                    let _ = popup.update(cx, |view, _, cx| {
                        view.palette_command_by_id(&id, cx)
                    });
                }
            }
            Action::RunCatalog(id) => {
                if let Some(popup) = self.popup {
                    let _ = popup.update(cx, |view, _, cx| {
                        view.palette_action_by_id(&id, cx)
                    });
                }
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
            Action::System(action) => {
                // The worker owns the bus round-trip; the palette just hands
                // the request over and folds away.
                let _ = self.backend.send(backend::Command::System(action));
                self.close(cx);
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
                        f64::from(self.dock_metrics(w, h).thickness) + 8.
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
    /// A closing fade keeps the surface alive this long; the fade itself is
    /// an analytic Motion the view samples at its own frame clock. The motion
    /// settles in about 150ms at the shared response frequency, so the release
    /// timer adds slack rather than a second animation driver.
    const POPUP_EXIT: Duration = Duration::from_millis(170);
    fn close(&mut self, cx: &mut Context<Self>) {
        self.preferences_placed = None;
        // The launcher's menu is not part of the panel's exit ride; it folds
        // away at once and its launcher keeps focus for the fade.
        self.dismiss_menu(cx);
        // A fading bar popup repaints and rides out before its surface is
        // released; anything else dismisses as before, right away.
        let animate = self
            .panel
            .as_ref()
            .is_some_and(|panel| panels::popup_exit_panel(panel) && !self.prefs.reduce_motion);
        if !animate {
            self.discard(cx);
            return;
        }
        self.cancel_popup_exit(cx);
        let Some(window) = self.popup.take() else {
            self.panel = None;
            self.panel_output = None;
            self.folder_drag_layer = false;
            self.tray_menu.clear();
            self.expanded_menus.clear();
            self.collapsing_menus.clear();
            return;
        };
        self.exit_generation = self.exit_generation.wrapping_add(1);
        self.panel_exit = true;
        // `panel` stays visible so the fading view keeps painting a real popup;
        // the release clears it once the surface is gone.
        let state = Arc::new(self.snapshot());
        let generation = self.exit_generation;
        let _ = window.update(cx, |view, _, cx| {
            view.state = state;
            cx.notify()
        });
        self.exiting = Some(window);
        cx.notify();
        cx.spawn(async move |host, cx| {
            cx.background_executor()
                .timer(Self::POPUP_EXIT)
                .await;
            let _ = host.update(cx, |host, cx| host.finish_popup_exit(generation, cx));
        })
        .detach();
    }
    /// Remove the current popup immediately. A superseding open never lingers:
    /// a fade that a newer window replaces is cut short instead of racing it.
    fn discard(&mut self, cx: &mut Context<Self>) {
        self.preferences_placed = None;
        self.cancel_popup_exit(cx);
        // The menu is a surface of its own; it never outlives its launcher.
        self.dismiss_menu(cx);
        self.panel = None;
        self.panel_output = None;
        self.folder_drag_layer = false;
        self.tray_menu.clear();
        self.expanded_menus.clear();
        self.collapsing_menus.clear();
        if let Some(window) = self.popup.take() {
            let _ = window.update(cx, |_, window, _| window.remove_window());
        }
    }
    /// A failed desktop action shows its hint for this long; a later failure
    /// restarts the timer so the hint stays a transient toast, not a permanent
    /// resident of every popup.
    const ERROR_BANNER: Duration = Duration::from_millis(4000);
    /// A collapsing submenu card fades for this long before the popup
    /// reclaims its height, matching the popup exit ride's rhythm.
    const SUBMENU_FADE: Duration = Duration::from_millis(170);
    /// The collapse fade ran out: fold the submenu away. A re-expansion
    /// during the fade already removed the id, so this becomes a no-op.
    fn finish_submenu_collapse(&mut self, id: i32, cx: &mut Context<Self>) {
        if !self.collapsing_menus.contains(&id) {
            return;
        }
        self.collapsing_menus.retain(|other| *other != id);
        self.expanded_menus.retain(|other| *other != id);
        self.changed(cx);
    }
    fn show_error(&mut self, error: String, cx: &mut Context<Self>) {
        self.error = Some(error);
        self.error_generation = self.error_generation.wrapping_add(1);
        let generation = self.error_generation;
        cx.notify();
        cx.spawn(async move |host, cx| {
            cx.background_executor()
                .timer(Self::ERROR_BANNER)
                .await;
            let _ = host.update(cx, |host, cx| host.release_error(generation, cx));
        })
        .detach();
    }
    /// The hint's lifetime ran out. The generation check keeps an older timer
    /// from erasing a newer banner that replaced it while it waited.
    fn release_error(&mut self, generation: u64, cx: &mut Context<Self>) {
        if self.error_generation != generation {
            return;
        }
        self.error = None;
        self.changed(cx);
    }
    /// Weld off an in-flight exit so at most one fade waits for its release.
    /// Carry the user's open submenus onto a refreshed menu. An owner that
    /// republishes its tree (a bluetooth or wifi scan repopulating its device
    /// list) may renumber ids, so a fold first follows its id and then, when
    /// the id is gone, the parent that still carries the same label.
    fn remap_expanded(&mut self, items: Vec<services::MenuItem>) -> Vec<services::MenuItem> {
        if self.expanded_menus.is_empty() {
            return items;
        }
        let mut labels: Vec<String> = Vec::new();
        collect_expanded_labels(&self.tray_menu, &self.expanded_menus, &mut labels);
        let keep: Vec<i32> = self
            .expanded_menus
            .iter()
            .copied()
            .filter(|id| tree_has_parent(&items, *id))
            .collect();
        let mut next = keep;
        if !labels.is_empty() {
            collect_label_parents(&items, &labels, &mut next);
        }
        next.sort_unstable();
        next.dedup();
        self.expanded_menus = next;
        items
    }
    /// Weld off an in-flight exit so at most one fade waits for its release.
    fn cancel_popup_exit(&mut self, cx: &mut Context<Self>) {
        self.exit_generation = self.exit_generation.wrapping_add(1);
        self.panel_exit = false;
        if let Some(window) = self.exiting.take() {
            let _ = window.update(cx, |_, window, _| window.remove_window());
        }
    }
    /// The fade ran out: release the surface and clear the panel state that was
    /// kept alive only so the view could keep painting during the fade.
    fn finish_popup_exit(&mut self, generation: u64, cx: &mut Context<Self>) {
        if self.exit_generation != generation {
            return;
        }
        self.panel_exit = false;
        self.panel = None;
        self.panel_output = None;
        self.folder_drag_layer = false;
        self.tray_menu.clear();
        self.expanded_menus.clear();
        self.collapsing_menus.clear();
        self.anchored = None;
        if let Some(window) = self.exiting.take() {
            let _ = window.update(cx, |_, window, _| window.remove_window());
        }
        self.changed(cx);
    }
    fn open(&mut self, panel: Panel, cx: &mut Context<Self>) {
        self.discard(cx);
        if let Panel::Tray(id) = &panel {
            self.service.send(services::Command::GetMenu(id.clone()));
        }
        if panel == Panel::Apps || panel == Panel::Spotlight {
            let _ = self.backend.send(backend::Command::RefreshCatalog);
        }
        if panel == Panel::Notifications {
            self.service.send(services::Command::MarkRead);
            // The persistent column replaces its transient banner; otherwise
            // the banner covers the first notification for its remaining timeout.
            self.toast_id = None;
            if let Some(toast) = self.toast.take() {
                let _ = toast.update(cx, |_, window, _| window.remove_window());
            }
        }
        self.panel = Some(panel);
        // A popup opened from a bar or Dock belongs to that surface's monitor;
        // keyboard focus may still sit on a different output.
        let output = self
            .dispatch_output
            .clone()
            .filter(|output| self.outputs.contains_key(output))
            .unwrap_or_else(|| self.active_output());
        self.panel_output = Some(output.clone());
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
            // Grab keyboard focus at the platform level so a later click
            // elsewhere reports a deactivation and dismisses the popup. Without
            // this a Dock menu opened by a right-click can keep its grab and
            // never observe the focus change.
            if !placement.normal {
                view.update(cx, |_, _| window.activate_window());
            }
            view
        }) {
            Ok(window) => self.popup = Some(window),
            Err(error) => {
                self.show_error(format!("无法打开面板：{error}"), cx);
                self.panel = None;
                self.panel_output = None;
            }
        }
    }
    /// Open one launcher tile's application menu as a surface of its own over
    /// the launcher. The launcher keeps its window, draft and selection; only
    /// the keyboard moves to the menu while it is up. A card painted into the
    /// launcher's window could never be frosted — the compositor blurs what lies
    /// behind a surface, never its own content.
    fn open_menu(&mut self, id: String, anchor: (i32, i32), cx: &mut Context<Self>) {
        self.dismiss_menu(cx);
        if !matches!(self.panel, Some(Panel::Apps | Panel::Spotlight)) {
            return;
        }
        // The menu belongs to the launcher's monitor, not to whichever output
        // happens to hold keyboard focus.
        let output = self
            .dispatch_output
            .clone()
            .filter(|output| self.outputs.contains_key(output))
            .unwrap_or_else(|| self.active_output());
        let view = self.outputs.get(&output);
        let display = view.map(|v| v.display);
        let (w, h) = view.map(|v| (v.width, v.height)).unwrap_or((1200., 800.));
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(w), px(h)));
        let options = window_options(
            "clarion-panel",
            display,
            bounds,
            self.preview,
            LayerShellAnchor::ALL,
            -1,
            LayerShellKeyboardInteractivity::Exclusive,
            LayerShellMargins::default(),
            LayerShellLayer::Overlay,
        );
        let host = cx.entity();
        let state = Arc::new(self.snapshot());
        self.menu_opening = true;
        let opened = cx.open_window(options, move |window, cx| {
            let view = cx.new(|cx| {
                ShellView::new_launcher_menu(host, output, state, id, anchor, cx)
            });
            view.update(cx, |view, cx| view.observe_activation(window, cx));
            view.read(cx).focus(window, cx);
            view
        });
        self.menu_opening = false;
        match opened {
            Ok(window) => {
                self.menu = Some(window);
                // Take the keyboard only once the host knows a menu is up: the
                // launcher's deactivation must not dismiss it as "a click
                // elsewhere".
                let _ = window.update(cx, |_, window, _| window.activate_window());
            }
            Err(error) => self.show_error(format!("无法打开应用菜单：{error}"), cx),
        }
    }
    /// Fold the launcher's application menu away and hand the keyboard back to
    /// the launcher, so its field keeps typing.
    fn close_menu(&mut self, cx: &mut Context<Self>) {
        if self.dismiss_menu(cx) {
            if let Some(launcher) = self.popup.as_ref() {
                let _ = launcher.update(cx, |_, window, _| window.activate_window());
            }
        }
    }
    /// Release the menu surface without touching the launcher's focus.
    fn dismiss_menu(&mut self, cx: &mut Context<Self>) -> bool {
        match self.menu.take() {
            Some(window) => {
                let _ = window.update(cx, |_, window, _| window.remove_window());
                true
            }
            None => false,
        }
    }
    fn show_toast(&mut self, id: u32, cx: &mut Context<Self>) {
        if self.panel == Some(Panel::Notifications) || self.toast_id == Some(id) {
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
    /// Dock window for an output, used to forward drag events received by a
    /// fullscreen folder overlay.
    pub(crate) fn dock_window(&self, output: &str) -> Option<WindowHandle<ShellView>> {
        self.outputs.get(output).and_then(|view| view.dock)
    }
    /// The pinned applications in the order the user arranged them. Only the
    /// pinned group carries an order; the running and recent sections keep the
    /// order the desktop reports so the separator never moves.
    pub fn pinned(&self) -> Vec<String> {
        let mut result = self.pinned_raw();
        if let Some(order) = &self.prefs.dock_order {
            let mut ordered: Vec<String> = Vec::with_capacity(result.len());
            for id in order {
                if result.contains(id) && !ordered.contains(id) {
                    ordered.push(id.clone());
                }
            }
            for id in result {
                if !ordered.contains(&id) {
                    ordered.push(id);
                }
            }
            result = ordered;
        }
        result
    }
    /// The pinned applications without the saved arrangement.
    pub fn pinned_raw(&self) -> Vec<String> {
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
                .find(|a| window_matches_app(window, a))
                .map(|a| a.id.clone())
                // An unmatched window keeps a stable identity across its own
                // windows: grouping by application id rather than by window id
                // keeps one tile per application even when the compositor
                // reports an app id the catalog cannot resolve.
                .unwrap_or_else(|| synthetic_dock_id(window));
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
                        if id.starts_with("window:") {
                            synthetic_dock_id(w) == id
                        } else {
                            window_matches_app_id(w, &id)
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
        // Clicking a Dock icon: launch when the application is closed, focus it
        // when a window is open but unfocused, and minimize when it is already
        // the focused application.
        if let Some(focused) = app.windows.iter().find(|window| window.is_focused) {
            self.handle(Action::Minimize(focused.id), cx);
        } else if let Some(window) = app.windows.iter().max_by_key(|window| {
            (
                !window.is_minimized,
                window.focus_timestamp.map(|time| (time.secs, time.nanos)),
                std::cmp::Reverse(window.id),
            )
        }) {
            self.handle(Action::Focus(window.id), cx);
        } else {
            self.handle(Action::Launch(app.id.clone()), cx);
        }
    }
    /// Fold a drawer reorder back into the full pinned arrangement. The overflow
    /// drawer shows only a subset of the pinned group, so a new drawer order is
    /// written into the positions those applications already occupy while every
    /// other pinned application keeps its place. Non-pinned drawer applications
    /// never reorder. Returns the new full order, or the unchanged one.
    pub fn reorder_pinned_subset(&self, drawer_order: &[String]) -> Vec<String> {
        let mut order = self.pinned();
        let positions: Vec<usize> = order
            .iter()
            .enumerate()
            .filter(|(_, id)| drawer_order.contains(id))
            .map(|(index, _)| index)
            .collect();
        let mut subset: Vec<String> = drawer_order
            .iter()
            .filter(|id| order.contains(id))
            .cloned()
            .collect();
        subset.truncate(positions.len());
        for (slot, id) in positions.into_iter().zip(subset) {
            order[slot] = id;
        }
        order
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
            let dock = if self.prefs.dock_hidden {
                None
            } else {
                self.create_dock(&name, display.id(), width, height, cx)
            };
            let reserved_top = bh;
            let reserved_dock = self.dock_metrics(width, height).thickness + 8.;
            let reserved_bottom = if self.prefs.dock_position == DockPosition::Bottom
                && !self.prefs.dock_auto_hide
                && !self.prefs.dock_hidden
            {
                reserved_dock
            } else {
                0.
            };
            let reserved_left = if self.prefs.dock_position == DockPosition::Left
                && !self.prefs.dock_auto_hide
                && !self.prefs.dock_hidden
            {
                reserved_dock
            } else {
                0.
            };
            let reserved_right = if self.prefs.dock_position == DockPosition::Right
                && !self.prefs.dock_auto_hide
                && !self.prefs.dock_hidden
            {
                reserved_dock
            } else {
                0.
            };
            let desktop = if self.preview || self.options.no_wallpaper {
                None
            } else {
                let environment = self.environment.clone();
                let actions = self.desktop_actions.clone();
                let output = name.clone();
                let appearance = self.prefs.surface_appearance();
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
                            let mut view = DesktopView::new(
                                output,
                                environment,
                                actions,
                                false,
                                appearance,
                                cx,
                            );
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
    fn dock_metrics(&self, width: f32, height: f32) -> crate::dock_motion::DockMetrics {
        let apps = self.dock_apps();
        let mut metrics = crate::dock_motion::DockMetrics::with_folders(
            apps.len(),
            self.prefs.dock_size as f32,
            self.prefs.magnification_scale as f32,
            self.prefs.magnification && !self.prefs.reduce_motion,
            self.prefs.show_launcher,
            self.prefs.dock_folders.len() + usize::from(self.folder_drop_preview),
            self.prefs.dock_position != DockPosition::Bottom,
            width,
            height,
        );
        let (pinned, running, recent) = dock_app_counts(&apps);
        metrics.restrict_overflow(pinned, running, recent);
        metrics
    }
    fn create_dock(
        &self,
        name: &str,
        display: DisplayId,
        width: f32,
        height: f32,
        cx: &mut Context<Self>,
    ) -> Option<WindowHandle<ShellView>> {
        let metrics = self.dock_metrics(width, height);
        let thickness = metrics.thickness;
        let mut anchor = LayerShellAnchor::default();
        match self.prefs.dock_position {
            DockPosition::Bottom => anchor.bottom = true,
            DockPosition::Left => anchor.left = true,
            DockPosition::Right => anchor.right = true,
        };
        let size = size(px(metrics.surface_width), px(metrics.surface_height));
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
        if self.targets_pending {
            self.targets();
        }
        if self.prefs.dock_auto_hide
            && !self.auto_hidden
            && !self.dock_hover
            && !self.dock_edge_hover
            && self.panel.is_none()
            && self.idle_since.elapsed() > Duration::from_millis(850)
        {
            self.auto_hidden = true;
            self.changed(cx);
        }
        self.launches
            .retain(|_, started| started.elapsed() < Duration::from_secs(1));
        if !self.ready && !self.outputs.is_empty() {
            self.ready = true;
            for folder in self.prefs.dock_folders.clone() {
                self.request_folder_scan(PathBuf::from(folder.path), cx);
            }
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
            if self.prefs.dock_hidden {
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
        self.targets_pending = false;
        let state = self.snapshot();
        let apps = state.dock_apps();
        let launcher = usize::from(self.prefs.show_launcher);
        let mut next = HashMap::new();
        if self.prefs.minimize_into_app && !self.prefs.dock_hidden {
            for (name, output) in &self.outputs {
                let metrics = self.dock_metrics(output.width, output.height);
                let (surface_x, surface_y) = panels::dock_surface_origin(
                    &state,
                    &metrics,
                    output.width,
                    output.height,
                );
                let slots = metrics.samples(None, 0.);
                let icon = f64::from(metrics.icon_size);
                let inset = f64::from(metrics.inset);
                let visible = dock_display_apps(&apps, &metrics);
                for (index, app) in visible.iter().enumerate() {
                    let main = slots
                        .get(index + launcher)
                        .map(|slot| f64::from(slot.origin_main))
                        .unwrap_or_else(|| {
                            f64::from(metrics.main_padding + metrics.end_gap)
                                + (index + launcher) as f64 * f64::from(metrics.item)
                        });
                    let (x, y) = match self.prefs.dock_position {
                        DockPosition::Bottom => (
                            surface_x as f64 + main,
                            surface_y as f64 + f64::from(metrics.cross_padding) + inset,
                        ),
                        DockPosition::Left => (
                            surface_x as f64 + inset,
                            surface_y as f64 + main,
                        ),
                        DockPosition::Right => (
                            surface_x as f64
                                + f64::from(metrics.surface_width - metrics.thickness)
                                + inset,
                            surface_y as f64 + main,
                        ),
                    };
                    if main + icon
                        > f64::from(metrics.main_padding + metrics.length) + 0.001
                    {
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
                                width: f64::from(metrics.icon_size),
                                height: f64::from(metrics.icon_size),
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
            let registration = (target.clone(), self.prefs.reduce_motion);
            if self.last_target.get(id) == Some(&registration) {
                continue;
            }
            if self
                .backend
                .send(backend::Command::Action(CoreAction::SetMinimizeTarget {
                    id: *id,
                    target: Some(target.clone()),
                    reduce_motion: self.prefs.reduce_motion,
                }))
                .is_ok()
            {
                self.last_target.insert(*id, registration);
            } else {
                self.targets_pending = true;
            }
        }
        let obsolete = self
            .last_target
            .keys()
            .filter(|id| !next.contains_key(id))
            .copied()
            .collect::<Vec<_>>();
        for id in obsolete {
            if self
                .backend
                .send(backend::Command::Action(CoreAction::SetMinimizeTarget {
                    id,
                    target: None,
                    reduce_motion: self.prefs.reduce_motion,
                }))
                .is_ok()
            {
                self.last_target.remove(&id);
            } else {
                self.targets_pending = true;
            }
        }
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
            "menu-ring.svg" => include_bytes!("../assets/icons/menu-ring.svg"),
            "menu-check.svg" => include_bytes!("../assets/icons/menu-check.svg"),
            "chevron-right.svg" => include_bytes!("../assets/icons/chevron-right.svg"),
            "chevron-left.svg" => include_bytes!("../assets/icons/chevron-left.svg"),
            "search.svg" => include_bytes!("../assets/icons/search.svg"),
            "chevron-down.svg" => include_bytes!("../assets/icons/chevron-down.svg"),
            // Absolute paths are resolved system theme icons; serve the file.
            _ => {
                let file = std::path::Path::new(path);
                if file.is_absolute() {
                    match std::fs::read(file) {
                        Ok(bytes) => return Ok(Some(std::borrow::Cow::Owned(bytes))),
                        Err(_) => return Ok(None),
                    }
                }
                return Ok(None);
            }
        };
        Ok(Some(std::borrow::Cow::Borrowed(bytes)))
    }
    fn list(&self, _path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(vec![])
    }
}

#[cfg(test)]
pub(crate) mod testing;

/// Split the Dock composition into its three groups. Recent applications are
/// the unpinned, non-running entries appended from launch history.
pub(crate) fn dock_app_counts(apps: &[DockApp]) -> (usize, usize, usize) {
    let pinned = apps.iter().filter(|app| app.pinned).count();
    let running = apps
        .iter()
        .filter(|app| !app.pinned && !app.windows.is_empty())
        .count();
    let recent = apps
        .iter()
        .filter(|app| !app.pinned && app.windows.is_empty())
        .count();
    (pinned, running, recent)
}

/// The tiles the Dock actually paints, in row order. Pinned applications use
/// the visible prefix, then running applications, then recent applications.
/// The folded pinned tail is excluded so a recent or running tile is never
/// displaced by an application waiting in the overflow drawer.
pub(crate) fn dock_display_apps<'a>(
    apps: &'a [DockApp],
    metrics: &crate::dock_motion::DockMetrics,
) -> Vec<&'a DockApp> {
    apps.iter()
        .filter(|app| app.pinned)
        .take(metrics.visible_pinned)
        .chain(
            apps.iter()
                .filter(|app| !app.pinned && !app.windows.is_empty())
                .take(metrics.visible_running),
        )
        .chain(
            apps.iter()
                .filter(|app| !app.pinned && app.windows.is_empty())
                .take(metrics.visible_recent),
        )
        .collect()
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

/// Stable identity for a running window the catalog could not resolve to a
/// desktop entry. Grouping by `app_id` (rather than the window id) keeps one
/// Dock tile per application, so several windows of an unknown application do
/// not appear as duplicates.
fn synthetic_dock_id(window: &clarion_ipc::Window) -> String {
    match window.app_id.as_deref().filter(|id| !id.is_empty()) {
        Some(app_id) => format!("window:{}", app_id.to_ascii_lowercase()),
        None => format!("window:{}", window.id),
    }
}

/// Candidate identifiers a compositor may report for one application. Wayland
/// app ids differ between machines and come in several shapes for the same
/// desktop file: `org.gnome.Nautilus`, `nautilus`, or `org.gnome.Nautilus`
/// for `org.gnome.Nautilus.desktop`. Comparing only the desktop file stem
/// leaves unmatched windows behind, which previously showed as a row of
/// duplicate generic icons.
fn window_identifiers(window: &clarion_ipc::Window) -> Vec<String> {
    let mut identifiers = Vec::new();
    let Some(raw) = window.app_id.as_deref().filter(|id| !id.is_empty()) else {
        return identifiers;
    };
    let raw = raw.to_ascii_lowercase();
    identifiers.push(raw.clone());
    // The trailing dotted component: `org.gnome.Nautilus` -> `nautilus`.
    if let Some(last) = raw.rsplit('.').next() {
        if last != raw && !last.is_empty() {
            identifiers.push(last.to_owned());
        }
    }
    identifiers
}

fn app_identifiers(id: &str) -> Vec<String> {
    let stem = id.trim_end_matches(".desktop").to_ascii_lowercase();
    let mut identifiers = vec![stem.clone()];
    // Nested directories arrive as `subdir-name.desktop`.
    if let Some(last) = stem.rsplit('-').next() {
        if last != stem && !last.is_empty() {
            identifiers.push(last.to_owned());
        }
    }
    if let Some(last) = stem.rsplit('.').next() {
        if last != stem && !last.is_empty() {
            identifiers.push(last.to_owned());
        }
    }
    identifiers
}

fn window_matches_app(window: &clarion_ipc::Window, app: &DesktopApp) -> bool {
    window_matches_app_id(window, &app.id)
}

fn window_matches_app_id(window: &clarion_ipc::Window, id: &str) -> bool {
    let window_ids = window_identifiers(window);
    let app_ids = app_identifiers(id);
    window_ids
        .iter()
        .any(|window_id| app_ids.iter().any(|app_id| app_id == window_id))
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

#[cfg(test)]
mod notification_lifecycle_tests {
    use ::core::prelude::v1::test;

    use super::*;

    fn notification(id: u32) -> services::Notification {
        services::Notification {
            id,
            app: "Notification fixture".into(),
            title: "通知标题".into(),
            body: "通知正文".into(),
            actions: Vec::new(),
            created_at: u64::from(id),
            unread: true,
        }
    }

    #[gpui::test]
    fn dismissing_a_notification_removes_its_banner_and_input_surface(cx: &mut TestAppContext) {
        let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
        host.update(cx, |host, cx| {
            host.service_event(services::Event::Notifications(vec![notification(1)]), cx)
        });
        let toast = host.read_with(cx, |host, _| host.toast.expect("banner created"));
        host.update(cx, |host, cx| {
            host.service_event(services::Event::Notifications(Vec::new()), cx)
        });
        host.read_with(cx, |host, _| {
            assert!(host.toast.is_none());
            assert!(host.toast_id.is_none());
        });
        cx.update(|cx| assert!(toast.update(cx, |_, _, _| ()).is_err()));
    }

    #[gpui::test]
    fn the_notification_center_replaces_banners_and_stays_unobscured(cx: &mut TestAppContext) {
        let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
        host.update(cx, |host, cx| {
            host.service_event(services::Event::Notifications(vec![notification(1)]), cx);
            assert!(host.toast.is_some());
            host.open(Panel::Notifications, cx);
            assert!(host.toast.is_none());
            assert!(host.toast_id.is_none());
            host.service_event(
                services::Event::Notifications(vec![notification(1), notification(2)]),
                cx,
            );
            assert!(
                host.toast.is_none(),
                "new notifications belong in the open center"
            );
            assert_eq!(host.notifications.len(), 2);
            assert_eq!(host.panel, Some(Panel::Notifications));
        });
    }
}

#[cfg(test)]
mod error_banner_tests {
    use ::core::prelude::v1::test;

    use super::*;

    #[gpui::test]
    fn a_service_error_banner_is_a_transient_toast(cx: &mut TestAppContext) {
        let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
        host.update(cx, |host, cx| {
            host.service_event(services::Event::Error("菜单项目不可用".into()), cx);
        });
        host.read_with(cx, |host, _| assert!(host.error.is_some()));
        cx.executor().advance_clock(Host::ERROR_BANNER);
        cx.run_until_parked();
        host.read_with(cx, |host, _| assert!(host.error.is_none()));
    }

    #[gpui::test]
    fn a_replaced_error_banner_outlives_the_older_release_timer(cx: &mut TestAppContext) {
        let host = cx.update(|cx| testing::test_host(cx, AppCatalog::default(), HashMap::new()));
        let tick = Duration::from_millis(100);
        host.update(cx, |host, cx| host.show_error("第一次错误".into(), cx));
        cx.executor().advance_clock(Host::ERROR_BANNER - tick);
        host.update(cx, |host, cx| host.show_error("第二次错误".into(), cx));
        cx.executor().advance_clock(tick);
        cx.run_until_parked();
        host.read_with(cx, |host, _| {
            assert_eq!(host.error.as_deref(), Some("第二次错误"));
        });
        cx.executor().advance_clock(Host::ERROR_BANNER);
        cx.run_until_parked();
        host.read_with(cx, |host, _| assert!(host.error.is_none()));
    }
}

