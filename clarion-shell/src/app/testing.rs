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
        error: None,
        panel: None,
        preview: true,
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
