//! Test-only construction: no GPUI platform, Wayland, D-Bus or command socket.
use super::*;

pub(crate) fn test_host(
    cx: &mut App,
    catalog: AppCatalog,
    environment: HashMap<String, String>,
) -> Entity<Host> {
    let (backend, _events) = backend::Backend::start(backend::Options {
        environment: environment.clone(),
        preview: true,
        command_server: false,
    })
    .unwrap();
    let (events, _receiver) = async_channel::unbounded();
    let service = services::start(events, true);
    let (desktop_actions, _desktop_events) = async_channel::unbounded();
    cx.new(|_| Host {
        prefs: Preferences::default(),
        desktop: Default::default(),
        system: Default::default(),
        catalog: Arc::new(catalog),
        notifications: Vec::new(),
        tray: Vec::new(),
        history: Vec::new(),
        clipboard_available: false,
        clipboard_paused: false,
        tray_menu: Vec::new(),
        expanded_menus: Vec::new(),
        collapsing_menus: Vec::new(),
        error: None,
        error_generation: 0,
        panel: None,
        anchored: None,
        panel_exit: false,
        preview: true,
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
        dispatch_output: None,
        exiting: None,
        exit_generation: 0,
        toast: None,
        toast_id: None,
        options: Options {
            preview: true,
            demo: false,
            monitor: 0,
            output_name: None,
            quit_after: None,
            check: false,
            no_wallpaper: true,
        },
        ready: false,
        preferences_placed: None,
        last_target: HashMap::new(),
        targets_pending: false,
        auto_hidden: false,
        launches: HashMap::new(),
        dock_folder_cache: HashMap::new(),
        dock_folder_errors: HashMap::new(),
        folder_navigation: None,
        folder_scan_generation: HashMap::new(),
        dock_hover: false,
        dock_edge_hover: false,
        idle_since: Instant::now(),
    })
}

pub(crate) fn open_panel(
    host: &Entity<Host>,
    panel: Panel,
    cx: &mut App,
) -> WindowHandle<ShellView> {
    host.update(cx, |host, cx| host.open(panel, cx));
    host.read(cx).popup.expect("test panel was not created")
}

pub(crate) fn open_folder(
    host: &Entity<Host>,
    root: std::path::PathBuf,
    cx: &mut App,
) -> WindowHandle<ShellView> {
    host.update(cx, |host, cx| host.apply(Action::OpenFolder(root), cx));
    host.read(cx).popup.expect("test folder panel was not created")
}

pub(crate) fn popup(host: &Entity<Host>, cx: &App) -> WindowHandle<ShellView> {
    host.read(cx).popup.expect("test panel was not created")
}

pub(crate) fn snapshot(host: &Entity<Host>, cx: &App) -> Arc<ViewState> {
    Arc::new(host.read(cx).snapshot())
}

pub(crate) fn output_name(host: &Entity<Host>, cx: &App) -> String {
    host.read(cx)
        .outputs
        .keys()
        .next()
        .cloned()
        .expect("test output was not created")
}

/// Register an extra monitor that shares the test platform's only display, so
/// a dispatched action can name an output other than the one that holds focus.
pub(crate) fn register_output(host: &Entity<Host>, name: &str, cx: &mut App) {
    host.update(cx, |host, cx| {
        let Some(view) = host.outputs.values().next() else {
            return;
        };
        let view = OutputViews {
            bar: view.bar,
            dock: view.dock,
            edge: view.edge,
            desktop: view.desktop,
            display: view.display,
            width: view.width,
            height: view.height,
        };
        host.outputs.insert(name.to_owned(), view);
        host.changed(cx);
    });
}

/// Apply an action tagged with the surface output that dispatched it, the way
/// a bar or Dock click does.
pub(crate) fn dispatch_on_output(
    host: &Entity<Host>,
    output: &str,
    action: Action,
    cx: &mut App,
) -> WindowHandle<ShellView> {
    host.update(cx, |host, cx| {
        host.apply(Action::on_output(output, action), cx)
    });
    host.read(cx).popup.expect("test panel was not created")
}

pub(crate) fn open_dock(host: &Entity<Host>, cx: &mut App) -> WindowHandle<ShellView> {
    host.update(cx, |host, cx| host.reconcile_outputs(cx));
    host.read(cx)
        .outputs
        .values()
        .find_map(|output| output.dock)
        .expect("test Dock was not created")
}

pub(crate) fn capture_backend(
    host: &Entity<Host>,
    cx: &mut App,
) -> std::sync::mpsc::Receiver<backend::Command> {
    let (backend, received) = backend::Backend::test_channel(32);
    host.update(cx, |host, _| host.backend = backend);
    received
}

pub(crate) fn age_launch(host: &Entity<Host>, id: &str, elapsed: Duration, cx: &mut App) {
    host.update(cx, |host, cx| {
        *host
            .launches
            .get_mut(id)
            .expect("test application was not launched") = Instant::now() - elapsed;
        host.changed(cx);
    });
}

pub(crate) fn set_dock_revealed(host: &Entity<Host>, revealed: bool, cx: &mut App) {
    host.update(cx, |host, cx| {
        host.auto_hidden = !revealed;
        host.reconcile_docks(cx);
        host.changed(cx);
    });
}
