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
        last_target: HashMap::new(),
        auto_hidden: false,
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
