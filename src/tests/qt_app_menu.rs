//! Real KDE appmenu wire requests update actual mapped-window IPC state.

use clarion_config::{Action, Config};
use clarion_ipc::AppMenu;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface as ServerSurface;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::Proxy as _;
use wayland_protocols_plasma::appmenu::client::org_kde_kwin_appmenu::OrgKdeKwinAppmenu;
use wayland_protocols_plasma::appmenu::client::org_kde_kwin_appmenu_manager::OrgKdeKwinAppmenuManager;

use super::client::ClientId;
use super::Fixture;
use crate::handlers::qt_app_menu::surface_menu;

wayland_client::delegate_noop!(super::client::State: ignore OrgKdeKwinAppmenuManager);
wayland_client::delegate_noop!(super::client::State: ignore OrgKdeKwinAppmenu);

fn fixture() -> Fixture {
    let mut config = Config::default();
    config.animations.off = true;
    config.input.focus_follows_mouse = None;
    config.hotkey_overlay.skip_at_startup = true;
    let mut f = Fixture::with_config(config);
    f.add_output(1, (1280, 800));
    f
}

fn bind(f: &mut Fixture, client: ClientId, version: u32) -> OrgKdeKwinAppmenuManager {
    let c = f.client(client);
    let global = c
        .state
        .globals
        .iter()
        .find(|g| g.interface == OrgKdeKwinAppmenuManager::interface().name)
        .unwrap();
    assert_eq!(global.version, 2);
    let registry = c.display.get_registry(&c.qh, ());
    registry.bind(global.name, version, &c.qh, ())
}

fn map(f: &mut Fixture, client: ClientId, surface: &WlSurface) -> (ServerSurface, u64) {
    let window = f.client(client).window(surface);
    window.commit();
    f.double_roundtrip(client);
    let window = f.client(client).window(surface);
    window.attach_new_buffer();
    window.set_size(160, 100);
    window.ack_last_and_commit();
    f.double_roundtrip(client);
    let mapped = f.clarion().layout.active_window().unwrap();
    (mapped.toplevel().wl_surface().clone(), mapped.id().get())
}

fn ipc_menu(f: &mut Fixture, id: u64) -> Option<AppMenu> {
    let windows = f.clarion().ipc_server.as_ref().unwrap().test_windows();
    windows
        .into_iter()
        .find(|w| w.id == id)
        .unwrap()
        .app_menu
        .map(|menu| *menu)
}

#[test]
fn canonical_menu_is_owned_by_its_real_surface_and_updates_ipc_without_focus() {
    let mut f = fixture();
    let first = f.add_client();
    let first_surface = f.client(first).create_window().surface.clone();
    let (first_server, first_id) = map(&mut f, first, &first_surface);
    let second = f.add_client();
    let second_surface = f.client(second).create_window().surface.clone();
    let (second_server, second_id) = map(&mut f, second, &second_surface);
    let focused = f.clarion().keyboard_focus.clone();
    let manager = bind(&mut f, first, 2);
    let c = f.client(first);
    let menu = manager.create(&first_surface, &c.qh, ());
    menu.set_address(":1.42".into(), "/MenuBar/1".into());
    f.double_roundtrip(first);
    let expected = AppMenu {
        bus_name: ":1.42".into(),
        menu_path: "/MenuBar/1".into(),
    };
    assert_eq!(surface_menu(&first_server), Some(expected.clone()));
    assert_eq!(ipc_menu(&mut f, first_id), Some(expected));
    assert_eq!(surface_menu(&second_server), None);
    assert_eq!(ipc_menu(&mut f, second_id), None);
    assert_eq!(f.clarion().keyboard_focus, focused);
    let windows = f.clarion().ipc_server.as_ref().unwrap().test_windows();
    let window = windows.iter().find(|w| w.id == first_id).unwrap();
    let json = serde_json::to_value(window).unwrap();
    assert_eq!(json["app_menu"]["bus_name"], ":1.42");
    assert_eq!(json["app_menu"]["menu_path"], "/MenuBar/1");
    assert!(
        json.get("gtk_app_menu").is_none(),
        "canonical export must not masquerade as GTK"
    );
    manager.release();
    menu.set_address("org.example.App".into(), "/MenuBar/2".into());
    f.double_roundtrip(first);
    assert_eq!(ipc_menu(&mut f, first_id).unwrap().menu_path, "/MenuBar/2");
    assert_eq!(f.clarion().keyboard_focus, focused);
    for (bus, path) in [
        (String::new(), "/MenuBar".into()),
        (":1.42".into(), "/Menu-Bar".into()),
        ("org.é".into(), "/MenuBar".into()),
        // Oversized bus names remain below Wayland's message-size limit, so
        // this exercises our bound rather than the transport disconnect path.
        (format!("org.{}", "x".repeat(252)), "/MenuBar".into()),
    ] {
        menu.set_address(bus, path);
        f.double_roundtrip(first);
        assert_eq!(ipc_menu(&mut f, first_id), None);
    }
    menu.set_address(":1.43".into(), "/MenuBar/3".into());
    f.double_roundtrip(first);
    menu.release();
    f.double_roundtrip(first);
    assert_eq!(surface_menu(&first_server), None);
    assert_eq!(ipc_menu(&mut f, first_id), None);
    assert_eq!(f.clarion().keyboard_focus, focused);
}

#[test]
fn unmapped_export_recreated_objects_and_surface_destruction_keep_correct_identity() {
    for version in [1, 2] {
        let mut f = fixture();
        let client = f.add_client();
        let surface = f.client(client).create_window().surface.clone();
        let manager = bind(&mut f, client, version);
        let c = f.client(client);
        let old = manager.create(&surface, &c.qh, ());
        old.set_address(":1.10".into(), "/MenuBar/10".into());
        f.double_roundtrip(client);
        assert!(f
            .clarion()
            .ipc_server
            .as_ref()
            .unwrap()
            .test_windows()
            .is_empty());
        let (server, id) = map(&mut f, client, &surface);
        assert_eq!(ipc_menu(&mut f, id).unwrap().menu_path, "/MenuBar/10");
        let c = f.client(client);
        let new = manager.create(&surface, &c.qh, ());
        new.set_address(":1.11".into(), "/MenuBar/11".into());
        old.set_address(":1.12".into(), "/WrongOldObject".into());
        old.release();
        f.double_roundtrip(client);
        assert_eq!(surface_menu(&server).unwrap().menu_path, "/MenuBar/11");
        assert_eq!(ipc_menu(&mut f, id).unwrap().menu_path, "/MenuBar/11");
        f.clarion_state().do_action(Action::FocusWindow(id), false);
        let window = f.client(client).window(&surface);
        window.xdg_toplevel.destroy();
        window.xdg_surface.destroy();
        window.viewport.destroy();
        window.surface.destroy();
        f.double_roundtrip(client);
        assert_eq!(surface_menu(&server), None);
        assert!(f
            .clarion()
            .ipc_server
            .as_ref()
            .unwrap()
            .test_windows()
            .is_empty());
        new.set_address(":1.13".into(), "/AlreadyDeadSurface".into());
        f.double_roundtrip(client);
        assert_eq!(surface_menu(&server), None);
        new.release();
        if version == 2 {
            manager.release();
        }
        f.double_roundtrip(client);
        f.clarion().layout.verify_invariants();
    }
}
