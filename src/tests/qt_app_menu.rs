//! Application menus remain inside clients. No global menu protocol or IPC
//! export may silently encourage a toolkit to hide its native menu bar.
use super::Fixture;

#[test]
fn global_menu_protocol_is_not_advertised_and_window_ipc_has_no_exports() {
    let mut f = Fixture::new();
    f.add_output(1, (1280, 800));
    let client = f.add_client();
    assert!(f.client(client).state.globals.iter().all(|global| {
        global.interface != "org_kde_kwin_appmenu_manager"
            && global.interface != "org_kde_kwin_appmenu"
    }));
    let window = f.client(client).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.double_roundtrip(client);
    let window = f.client(client).window(&surface);
    window.attach_new_buffer();
    window.set_size(160, 100);
    window.ack_last_and_commit();
    f.double_roundtrip(client);
    let windows = f.clarion().ipc_server.as_ref().unwrap().test_windows();
    assert_eq!(windows.len(), 1);
    let json = serde_json::to_value(&windows[0]).unwrap();
    assert!(json.get("app_menu").is_none());
    assert!(json.get("gtk_app_menu").is_none());
}
