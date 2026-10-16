//! GTK menu metadata is sent over a real, private Wayland connection. It must
//! stay attached to the exporting toplevel and never grant keyboard focus.

use std::sync::{Arc, Mutex};

use clarion_config::Config;
use clarion_ipc::GtkAppMenu;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface as ServerSurface;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};

use super::client::ClientId;
use super::Fixture;
use crate::handlers::gtk_shell::{surface_application_id_matches, surface_menu};

mod protocol {
    #![allow(dead_code, non_camel_case_types, unused_unsafe, unused_variables)]
    #![allow(non_upper_case_globals, non_snake_case, unused_imports)]
    #![allow(missing_docs, clippy::all)]

    use wayland_client;
    use wayland_client::protocol::*;
    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("resources/gtk-shell.xml");
    }
    use self::__interfaces::*;
    wayland_scanner::generate_client_code!("resources/gtk-shell.xml");
}

use protocol::gtk_shell1::{self, GtkShell1};
use protocol::gtk_surface1::GtkSurface1;

type Capabilities = Arc<Mutex<Option<u32>>>;

impl Dispatch<GtkShell1, Capabilities> for super::client::State {
    fn event(
        _state: &mut Self,
        _proxy: &GtkShell1,
        event: gtk_shell1::Event,
        capabilities: &Capabilities,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            gtk_shell1::Event::Capabilities {
                capabilities: value,
            } => {
                *capabilities.lock().unwrap() = Some(value);
            }
        }
    }
}

wayland_client::delegate_noop!(super::client::State: ignore GtkSurface1);

fn bind_shell(f: &mut Fixture, client: ClientId) -> GtkShell1 {
    let capabilities = Capabilities::default();
    let c = f.client(client);
    let global = c
        .state
        .globals
        .iter()
        .find(|global| global.interface == GtkShell1::interface().name)
        .unwrap();
    // Advertising global menu capabilities would hide native app menus in
    // Material mode, so the metadata bridge advertises only v1/capabilities 0.
    assert_eq!(global.version, 1);
    let registry = c.display.get_registry(&c.qh, ());
    let shell = registry.bind(global.name, 1, &c.qh, capabilities.clone());
    f.double_roundtrip(client);
    assert_eq!(*capabilities.lock().unwrap(), Some(0));
    shell
}

fn map_window(f: &mut Fixture, client: ClientId) -> (WlSurface, ServerSurface) {
    let window = f.client(client).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.double_roundtrip(client);
    let window = f.client(client).window(&surface);
    window.attach_new_buffer();
    window.set_size(100, 100);
    window.ack_last_and_commit();
    f.double_roundtrip(client);
    let server = f
        .clarion()
        .layout
        .active_window()
        .unwrap()
        .toplevel()
        .wl_surface()
        .clone();
    (surface, server)
}

#[test]
fn gtk_menu_metadata_is_owned_bounded_updateable_and_never_grants_focus() {
    let mut config = Config::default();
    config.animations.off = true;
    config.input.focus_follows_mouse = None;
    let mut f = Fixture::with_config(config);
    f.add_output(1, (1280, 800));
    let first = f.add_client();
    let (surface, server_surface) = map_window(&mut f, first);
    let second = f.add_client();
    let (_, second_surface) = map_window(&mut f, second);
    let focused = f.clarion().keyboard_focus.clone();
    let shell = bind_shell(&mut f, first);
    let c = f.client(first);
    let gtk_surface = shell.get_gtk_surface(&surface, &c.qh, ());
    gtk_surface.set_dbus_properties(
        Some("org.example.First".into()),
        Some("/org/example/First/appmenu".into()),
        Some("/org/example/First/menubar".into()),
        Some("/org/example/First/window/1".into()),
        Some("/org/example/First".into()),
        Some(":1.42".into()),
    );
    f.double_roundtrip(first);
    let expected = GtkAppMenu {
        application_id: Some("org.example.First".into()),
        app_menu_path: Some("/org/example/First/appmenu".into()),
        menubar_path: Some("/org/example/First/menubar".into()),
        window_object_path: Some("/org/example/First/window/1".into()),
        application_object_path: Some("/org/example/First".into()),
        unique_bus_name: Some(":1.42".into()),
    };
    assert_eq!(surface_menu(&server_surface), Some(expected));
    assert_eq!(
        surface_menu(&second_surface),
        None,
        "metadata must not follow active-window focus onto another client's surface"
    );
    assert_eq!(f.clarion().keyboard_focus, focused);

    gtk_surface.present(0);
    shell.set_startup_id(Some("arbitrary-client-string".into()));
    shell.system_bell(Some(&gtk_surface));
    f.double_roundtrip(first);
    assert_eq!(
        f.clarion().keyboard_focus,
        focused,
        "GTK metadata timestamps/startup IDs must not authorize focus"
    );

    gtk_surface.set_dbus_properties(
        Some(String::new()),
        None,
        None,
        None,
        Some("/org/example/Updated".into()),
        Some(":1.43".into()),
    );
    f.double_roundtrip(first);
    let updated = surface_menu(&server_surface).unwrap();
    assert_eq!(updated.application_id, None);
    assert_eq!(updated.app_menu_path, None);
    assert_eq!(updated.menubar_path, None);
    assert_eq!(
        updated.application_object_path.as_deref(),
        Some("/org/example/Updated")
    );
    assert_eq!(updated.unique_bus_name.as_deref(), Some(":1.43"));
    assert_eq!(f.clarion().keyboard_focus, focused);

    let window = f.client(first).window(&surface);
    window.xdg_toplevel.destroy();
    window.xdg_surface.destroy();
    window.viewport.destroy();
    window.surface.destroy();
    f.double_roundtrip(first);
    assert_eq!(
        surface_menu(&server_surface),
        None,
        "a destroyed wl_surface must not retain a usable export"
    );
    gtk_surface.set_dbus_properties(None, None, None, None, None, Some(":1.44".into()));
    f.double_roundtrip(first);
    assert_eq!(surface_menu(&server_surface), None);
    assert_eq!(surface_menu(&second_surface), None);
    assert_eq!(f.clarion().keyboard_focus, focused);
}

#[test]
fn macos_desktop_shortcut_identity_tracks_style_surface_focus_and_modal_owners() {
    use crate::compositor::KeyboardFocus;
    use crate::tests::client::LayerConfigureProps;
    use crate::ui::mru::{MruCloseRequest, WindowMru};
    use smithay::reexports::wayland_protocols::ext::session_lock::v1::client::ext_session_lock_manager_v1::ExtSessionLockManagerV1;
    use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::{
        zwlr_layer_shell_v1::Layer,
        zwlr_layer_surface_v1::{Anchor, KeyboardInteractivity},
    };
    use smithay::utils::SERIAL_COUNTER;

    fn focus(f: &mut Fixture, logical: KeyboardFocus, actual: Option<ServerSurface>) {
        let state = f.clarion_state();
        state.clarion.keyboard_focus = logical;
        state.clarion.seat.get_keyboard().unwrap().set_focus(
            state,
            actual,
            SERIAL_COUNTER.next_serial(),
        );
    }

    let mut config = Config::default();
    config.animations.off = true;
    config.hotkey_overlay.skip_at_startup = true;
    let mut f = Fixture::with_config(config);
    f.add_output(1, (1280, 800));
    let client = f.add_client();
    let (_, application) = map_window(&mut f, client);
    let shell = bind_shell(&mut f, client);
    let macos = "io.github.clarion.Desktop.MacOS";
    for namespace in ["clarion-desktop", "clarion-desktop-interaction"] {
        let panel = f
            .client(client)
            .create_layer(None, Layer::Bottom, namespace);
        let surface = panel.surface.clone();
        panel.set_configure_props(LayerConfigureProps {
            anchor: Some(Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right),
            size: Some((0, 0)),
            exclusive_zone: Some(-1),
            kb_interactivity: Some(KeyboardInteractivity::OnDemand),
            ..Default::default()
        });
        panel.commit();
        f.double_roundtrip(client);
        let panel = f.client(client).layer(&surface);
        panel.attach_new_buffer();
        panel.set_size(1280, 800);
        panel.ack_last_and_commit();
        f.double_roundtrip(client);
        let server = f
            .clarion()
            .mapped_layer_surfaces
            .keys()
            .find(|layer| layer.namespace() == namespace)
            .unwrap()
            .wl_surface()
            .clone();
        let logical = KeyboardFocus::LayerShell {
            surface: server.clone(),
        };
        focus(&mut f, logical.clone(), Some(server.clone()));
        assert!(
            !f.clarion_state().desktop_owns_macos_shortcuts(),
            "no GTK marker must fail closed"
        );
        let c = f.client(client);
        let metadata = shell.get_gtk_surface(&surface, &c.qh, ());
        for application_id in [macos, "io.github.clarion.Desktop.Material", macos] {
            metadata.set_dbus_properties(Some(application_id.into()), None, None, None, None, None);
            f.double_roundtrip(client);
            focus(&mut f, logical.clone(), Some(server.clone()));
            assert!(surface_application_id_matches(&server, application_id));
            assert_eq!(
                f.clarion_state().desktop_owns_macos_shortcuts(),
                namespace == "clarion-desktop" && application_id == macos
            );
        }
        if namespace != "clarion-desktop" {
            continue;
        }

        // A root's metadata cannot grant shortcuts to a popup (different actual
        // seat surface) or to an application through stale logical focus.
        focus(&mut f, logical.clone(), Some(application.clone()));
        assert!(!f.clarion_state().desktop_owns_macos_shortcuts());
        focus(
            &mut f,
            KeyboardFocus::Layout {
                surface: Some(server.clone()),
            },
            Some(server.clone()),
        );
        assert!(!f.clarion_state().desktop_owns_macos_shortcuts());
        for owner in [
            KeyboardFocus::ScreenshotUi,
            KeyboardFocus::Mru,
            KeyboardFocus::Overview,
            KeyboardFocus::LockScreen {
                surface: Some(server.clone()),
            },
        ] {
            focus(&mut f, owner, Some(server.clone()));
            assert!(!f.clarion_state().desktop_owns_macos_shortcuts());
        }
        focus(&mut f, logical.clone(), Some(server.clone()));
        f.clarion().hotkey_overlay.show();
        assert!(!f.clarion_state().desktop_owns_macos_shortcuts());
        f.clarion().hotkey_overlay.hide();
        f.clarion().exit_confirm_dialog.show();
        assert!(!f.clarion_state().desktop_owns_macos_shortcuts());
        f.clarion().exit_confirm_dialog.hide();
        f.clarion_complete_animations();
        focus(&mut f, logical.clone(), Some(server.clone()));
        assert!(!f.clarion().exit_confirm_dialog.is_open());
        let output = f.clarion_output(1);
        let clarion = f.clarion();
        let mru = WindowMru::new(clarion);
        clarion
            .window_mru_ui
            .open(clarion.clock.clone(), mru, output);
        assert!(!f.clarion_state().desktop_owns_macos_shortcuts());
        f.clarion().window_mru_ui.close(MruCloseRequest::Cancel);
        assert!(f.clarion_state().desktop_owns_macos_shortcuts());
    }

    // Even the pre-surface lock stage disables the desktop override, without
    // changing the normal lock/bind processing or requiring a rendered frame.
    let c = f.client(client);
    let global = c
        .state
        .globals
        .iter()
        .find(|global| global.interface == "ext_session_lock_manager_v1")
        .unwrap();
    let registry = c.display.get_registry(&c.qh, ());
    let manager: ExtSessionLockManagerV1 = registry.bind(global.name, 1, &c.qh, ());
    let _lock = manager.lock(&c.qh, ());
    f.double_roundtrip(client);
    assert!(matches!(
        f.clarion().lock_state,
        crate::compositor::LockState::WaitingForSurfaces { .. }
    ));
    let server = f
        .clarion()
        .mapped_layer_surfaces
        .keys()
        .find(|layer| layer.namespace() == "clarion-desktop")
        .unwrap()
        .wl_surface()
        .clone();
    focus(
        &mut f,
        KeyboardFocus::LayerShell {
            surface: server.clone(),
        },
        Some(server),
    );
    assert!(!f.clarion_state().desktop_owns_macos_shortcuts());
}
