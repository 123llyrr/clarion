use insta::assert_snapshot;
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer;
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    Anchor, KeyboardInteractivity,
};
use smithay::reexports::wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};
use wayland_client::protocol::{wl_pointer::ButtonState, wl_surface::WlSurface};

use super::*;
use crate::compositor::KeyboardFocus;
use crate::tests::client::{ClientId, LayerConfigureProps, LayerMargin};

#[test]
fn integrated_shell_glass_default_keeps_explicit_overrides_and_namespace_scope() {
    for override_value in [None, Some(false), Some(true)] {
        let config = override_value.map_or_else(String::new, |value| format!(
            "layer-rule {{ match namespace=\"^realand-shell(-dock)?$\"; background-effect {{ liquid-glass {value}; }}; }}"
        ));
        let mut f = Fixture::with_config(realand_config::Config::parse_mem(&config).unwrap());
        f.add_output(1, (1280, 800));
        let client = f.add_client();
        for namespace in [
            "realand-shell",
            "realand-shell-dock",
            "realand-shell-other",
            "waybar",
        ] {
            map_focus_layer(
                &mut f,
                client,
                Layer::Top,
                namespace,
                KeyboardInteractivity::None,
            );
            let realand = f.realand();
            let layer = realand
                .mapped_layer_surfaces
                .iter()
                .find(|(surface, _)| surface.namespace() == namespace)
                .unwrap()
                .1;
            let expected = if matches!(namespace, "realand-shell" | "realand-shell-dock") {
                Some(override_value.unwrap_or(true))
            } else {
                None
            };
            assert_eq!(layer.rules().background_effect.liquid_glass, expected);
        }
    }
}

fn layer_focus_fixture() -> (Fixture, ClientId, KeyboardFocus, u64) {
    let mut config = realand_config::Config::default();
    config.input.focus_follows_mouse = None;
    config.animations.off = true;
    let mut f = Fixture::with_config(config);
    f.add_output(1, (1280, 800));
    let client = f.add_client();
    let window = f.client(client).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.double_roundtrip(client);
    let window = f.client(client).window(&surface);
    window.attach_new_buffer();
    window.set_size(100, 100);
    window.ack_last_and_commit();
    f.double_roundtrip(client);
    f.realand().layout.toggle_window_floating(None);
    f.realand().layout.move_floating_window(
        None,
        realand_ipc::PositionChange::SetFixed(200.),
        realand_ipc::PositionChange::SetFixed(200.),
        false,
    );
    f.realand_complete_animations();
    f.double_roundtrip(client);
    let focus = f.realand().keyboard_focus.clone();
    assert!(matches!(focus, KeyboardFocus::Layout { surface: Some(_) }));
    let id = f.realand().layout.active_window().unwrap().id().get();
    (f, client, focus, id)
}

fn map_focus_layer(
    f: &mut Fixture,
    client: ClientId,
    layer: Layer,
    namespace: &str,
    interactivity: KeyboardInteractivity,
) -> WlSurface {
    let panel = f.client(client).create_layer(None, layer, namespace);
    let surface = panel.surface.clone();
    panel.set_configure_props(LayerConfigureProps {
        anchor: Some(Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right),
        size: Some((0, 0)),
        exclusive_zone: Some(-1),
        kb_interactivity: Some(interactivity),
        ..Default::default()
    });
    panel.commit();
    f.double_roundtrip(client);
    let panel = f.client(client).layer(&surface);
    panel.attach_new_buffer();
    panel.set_size(1280, 800);
    panel.ack_last_and_commit();
    f.double_roundtrip(client);
    surface
}

fn assert_layer_keyboard_focus(f: &mut Fixture, namespace: &str) {
    let layer = f
        .realand()
        .mapped_layer_surfaces
        .keys()
        .find(|layer| layer.namespace() == namespace)
        .unwrap()
        .clone();
    assert_eq!(
        f.realand().keyboard_focus,
        KeyboardFocus::LayerShell {
            surface: layer.wl_surface().clone(),
        }
    );
    assert_eq!(
        f.realand().seat.get_keyboard().unwrap().current_focus(),
        Some(layer.wl_surface().clone())
    );
}

fn focus_test_click(pointer: &ZwlrVirtualPointerV1, time: u32, x: u32, y: u32) {
    pointer.motion_absolute(time, x, y, 1280, 800);
    pointer.button(time + 1, 0x110, ButtonState::Pressed);
    pointer.button(time + 2, 0x110, ButtonState::Released);
    pointer.frame();
}

#[test]
fn realand_desktop_waits_for_click_focus_and_preserves_explicit_app_activation() {
    let (mut f, client, app_focus, app_id) = layer_focus_fixture();
    let desktop = map_focus_layer(
        &mut f,
        client,
        Layer::Bottom,
        "realand-desktop",
        KeyboardInteractivity::OnDemand,
    );
    assert_eq!(f.realand().keyboard_focus, app_focus);
    assert!(f.realand().layer_shell_on_demand_focus.is_none());

    // Go through the real Wayland virtual-pointer protocol and the compositor's
    // input handler, rather than dispatching directly through Smithay's seat.
    f.init_gles_renderer();
    let c = f.client(client);
    let global = c
        .state
        .globals
        .iter()
        .find(|global| global.interface == "zwlr_virtual_pointer_manager_v1")
        .unwrap();
    let registry = c.display.get_registry(&c.qh, ());
    let manager: ZwlrVirtualPointerManagerV1 = registry.bind(global.name, 1, &c.qh, ());
    let pointer = manager.create_virtual_pointer(None, &c.qh, ());
    let under = f.realand().contents_under((100., 100.).into());
    assert_eq!(under.layer.unwrap().namespace(), "realand-desktop");
    pointer.motion_absolute(1, 100, 100, 1280, 800);
    pointer.frame();
    f.double_roundtrip(client);
    assert_eq!(f.realand().keyboard_focus, app_focus);
    focus_test_click(&pointer, 10, 100, 100);
    f.double_roundtrip(client);
    assert_layer_keyboard_focus(&mut f, "realand-desktop");
    assert!(f.realand().layer_shell_on_demand_focus.is_some());

    f.realand_state()
        .do_action(realand_config::Action::FocusWindow(u64::MAX), false);
    f.double_roundtrip(client);
    assert_layer_keyboard_focus(&mut f, "realand-desktop");
    assert!(f.realand().layer_shell_on_demand_focus.is_some());
    f.realand_state()
        .do_action(realand_config::Action::FocusWindow(app_id), false);
    f.double_roundtrip(client);
    assert_eq!(f.realand().keyboard_focus, app_focus);
    assert!(f.realand().layer_shell_on_demand_focus.is_none());

    focus_test_click(&pointer, 20, 100, 100);
    f.double_roundtrip(client);
    assert_layer_keyboard_focus(&mut f, "realand-desktop");
    focus_test_click(&pointer, 30, 220, 220);
    f.double_roundtrip(client);
    assert_eq!(f.realand().keyboard_focus, app_focus);
    assert!(f.realand().layer_shell_on_demand_focus.is_none());

    // A desktop dialog still requests the normal exclusive overlay focus.
    let panel = f.client(client).layer(&desktop);
    panel.set_configure_props(LayerConfigureProps {
        layer: Some(Layer::Overlay),
        kb_interactivity: Some(KeyboardInteractivity::Exclusive),
        ..Default::default()
    });
    panel.commit();
    f.double_roundtrip(client);
    assert_layer_keyboard_focus(&mut f, "realand-desktop");
    let panel = f.client(client).layer(&desktop);
    panel.set_configure_props(LayerConfigureProps {
        layer: Some(Layer::Bottom),
        kb_interactivity: Some(KeyboardInteractivity::OnDemand),
        ..Default::default()
    });
    panel.commit();
    f.double_roundtrip(client);
    assert_eq!(f.realand().keyboard_focus, app_focus);

    // Restarting/remapping the persistent desktop must not steal focus either.
    let panel = f.client(client).layer(&desktop);
    panel.attach_null();
    panel.commit();
    f.double_roundtrip(client);
    let panel = f.client(client).layer(&desktop);
    // Null-buffer unmapping resets the role's pending state. GTK sends these
    // properties again before remapping, so the protocol fixture must too.
    panel.set_configure_props(LayerConfigureProps {
        anchor: Some(Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right),
        size: Some((0, 0)),
        exclusive_zone: Some(-1),
        kb_interactivity: Some(KeyboardInteractivity::OnDemand),
        layer: Some(Layer::Bottom),
        ..Default::default()
    });
    panel.commit();
    f.double_roundtrip(client);
    let panel = f.client(client).layer(&desktop);
    panel.attach_new_buffer();
    panel.set_size(1280, 800);
    panel.ack_last_and_commit();
    f.double_roundtrip(client);
    assert_eq!(f.realand().keyboard_focus, app_focus);
    assert!(f.realand().layer_shell_on_demand_focus.is_none());
    assert_eq!(
        f.realand().seat.get_keyboard().unwrap().current_focus(),
        app_focus.surface().cloned()
    );
}

#[test]
fn other_on_demand_layer_clients_keep_automatic_mapping_focus() {
    for layer in [Layer::Top, Layer::Bottom] {
        let (mut f, client, _, _) = layer_focus_fixture();
        map_focus_layer(
            &mut f,
            client,
            layer,
            "lxqt-runner",
            KeyboardInteractivity::OnDemand,
        );
        assert_layer_keyboard_focus(&mut f, "lxqt-runner");
        assert!(f.realand().layer_shell_on_demand_focus.is_some());
    }
}

#[test]
fn desktop_interaction_restores_only_its_existing_same_client_desktop_focus() {
    for (base_namespace, popup_namespace, same_client, focus_base, mode, retain) in [
        (
            "realand-desktop",
            "realand-desktop-interaction",
            true,
            true,
            KeyboardInteractivity::Exclusive,
            true,
        ),
        (
            "realand-desktop",
            "realand-desktop-interaction",
            false,
            true,
            KeyboardInteractivity::Exclusive,
            false,
        ),
        (
            "realand-desktop",
            "other-overlay",
            true,
            true,
            KeyboardInteractivity::Exclusive,
            false,
        ),
        (
            "other-desktop",
            "realand-desktop-interaction",
            true,
            true,
            KeyboardInteractivity::Exclusive,
            false,
        ),
        (
            "realand-desktop",
            "realand-desktop-interaction",
            true,
            false,
            KeyboardInteractivity::Exclusive,
            false,
        ),
        (
            "realand-desktop",
            "realand-desktop-interaction",
            true,
            true,
            KeyboardInteractivity::None,
            false,
        ),
    ] {
        let (mut f, client, app_focus, _) = layer_focus_fixture();
        map_focus_layer(
            &mut f,
            client,
            Layer::Bottom,
            base_namespace,
            KeyboardInteractivity::OnDemand,
        );
        f.init_gles_renderer();
        let c = f.client(client);
        let global = c
            .state
            .globals
            .iter()
            .find(|global| global.interface == "zwlr_virtual_pointer_manager_v1")
            .unwrap();
        let registry = c.display.get_registry(&c.qh, ());
        let manager: ZwlrVirtualPointerManagerV1 = registry.bind(global.name, 1, &c.qh, ());
        let pointer = manager.create_virtual_pointer(None, &c.qh, ());
        if focus_base {
            focus_test_click(&pointer, 10, 100, 100);
            f.double_roundtrip(client);
            assert_layer_keyboard_focus(&mut f, base_namespace);
        } else {
            assert!(f.realand().layer_shell_on_demand_focus.is_none());
        }
        let popup_client = if same_client { client } else { f.add_client() };
        let popup = map_focus_layer(&mut f, popup_client, Layer::Overlay, popup_namespace, mode);
        if mode == KeyboardInteractivity::Exclusive {
            // Remembering the desktop must never bypass the exclusive dialog.
            assert_layer_keyboard_focus(&mut f, popup_namespace);
        }
        focus_test_click(&pointer, 20, 100, 100);
        f.double_roundtrip(client);
        assert_eq!(f.realand().layer_shell_on_demand_focus.is_some(), retain,
            "base={base_namespace} popup={popup_namespace} same_client={same_client} focus_base={focus_base} mode={mode:?}");
        if mode == KeyboardInteractivity::Exclusive {
            assert_layer_keyboard_focus(&mut f, popup_namespace);
        }

        // Match the persistent transparent native popup: closing removes input
        // and keyboard interactivity without destroying or unmapping its view.
        let c = f.client(popup_client);
        let empty = c
            .state
            .compositor
            .as_ref()
            .unwrap()
            .create_region(&c.qh, ());
        popup.set_input_region(Some(&empty));
        empty.destroy();
        let panel = c.layer(&popup);
        panel.set_configure_props(LayerConfigureProps {
            kb_interactivity: Some(KeyboardInteractivity::None),
            ..Default::default()
        });
        panel.commit();
        f.double_roundtrip(popup_client);
        if retain {
            assert_layer_keyboard_focus(&mut f, base_namespace);
        } else {
            assert_eq!(f.realand().keyboard_focus, app_focus);
            assert_eq!(
                f.realand().seat.get_keyboard().unwrap().current_focus(),
                app_focus.surface().cloned()
            );
        }
        // A subsequent application click still ends the remembered desktop.
        focus_test_click(&pointer, 30, 220, 220);
        f.double_roundtrip(client);
        assert!(f.realand().layer_shell_on_demand_focus.is_none());
        assert_eq!(f.realand().keyboard_focus, app_focus);
    }
}

#[test]
fn desktop_focus_restore_capability_query_does_not_change_keyboard_focus() {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    let (mut f, _, app_focus, _) = layer_focus_fixture();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let name = format!("focus-review-{nonce:x}");
    let server = crate::ipc::server::IpcServer::start(
        &f.state.server.event_loop.handle(),
        Some(name.as_ref()),
    )
    .unwrap();
    let socket = server.socket_path.clone().unwrap();
    f.realand().ipc_server = Some(server);
    // The normal State constructor seeds this snapshot when it creates the
    // original socket-less test server. Seed the replacement in the same way.
    f.realand_state().ipc_keyboard_layouts_changed();
    let mut client = UnixStream::connect(socket).unwrap();
    client.write_all(b"\"Capabilities\"\n").unwrap();
    client.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut reply = Vec::new();
    while !reply.contains(&b'\n') {
        assert!(Instant::now() < deadline, "capabilities reply timed out");
        f.dispatch();
        let mut bytes = [0; 1024];
        match client.read(&mut bytes) {
            Ok(0) => panic!("capabilities socket closed before its reply"),
            Ok(length) => reply.extend_from_slice(&bytes[..length]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::yield_now()
            }
            Err(error) => panic!("could not read capabilities reply: {error}"),
        }
    }
    let response: realand_ipc::Reply = serde_json::from_slice(&reply).unwrap();
    let realand_ipc::Response::Capabilities(names) = response.unwrap() else {
        panic!("unexpected capabilities response");
    };
    assert!(names
        .iter()
        .any(|name| name == "desktop-menu-focus-restore"));
    assert_eq!(f.realand().keyboard_focus, app_focus);
    assert_eq!(
        f.realand().seat.get_keyboard().unwrap().current_focus(),
        app_focus.surface().cloned()
    );
    assert!(f.realand().layer_shell_on_demand_focus.is_none());
}

#[test]
#[allow(clippy::mutable_key_type)]
fn layer_subsurfaces_created_before_role_inherit_scale() {
    for scale in [1., 1.25, 1.5, 2.] {
        let mut f = Fixture::new();
        f.add_output(1, (1280, 800));
        let output = f.realand_output(1);
        output.change_current_state(
            None,
            None,
            Some(smithay::output::Scale::Fractional(scale)),
            None,
        );
        f.realand().output_resized(&output);
        let id = f.add_client();
        let client = f.client(id);
        let qh = client.qh.clone();
        let compositor = client.state.compositor.as_ref().unwrap();
        let subcompositor = client.state.subcompositor.as_ref().unwrap();
        let surface = compositor.create_surface(&qh, ());
        let early_child = compositor.create_surface(&qh, ());
        let _early = subcompositor.get_subsurface(&early_child, &surface, &qh, ());
        let grandchild = compositor.create_surface(&qh, ());
        let _grandchild = subcompositor.get_subsurface(&grandchild, &early_child, &qh, ());
        let panel = client.state.create_layer_on_surface(
            surface.clone(),
            None,
            Layer::Top,
            "early-subsurface".to_owned(),
        );
        panel.set_configure_props(LayerConfigureProps {
            size: Some((100, 40)),
            ..Default::default()
        });
        panel.commit();
        f.double_roundtrip(id);
        let expected = output.current_scale().integer_scale();
        let scales = &f.client(id).state.surface_scales;
        assert_eq!(scales.get(&surface).copied().unwrap_or(1), expected);
        assert_eq!(scales.get(&early_child).copied().unwrap_or(1), expected);
        assert_eq!(scales.get(&grandchild).copied().unwrap_or(1), expected);

        // Children created after the role is assigned use the normal subsurface hook.
        let client = f.client(id);
        let late_child = client
            .state
            .compositor
            .as_ref()
            .unwrap()
            .create_surface(&qh, ());
        let _late = client.state.subcompositor.as_ref().unwrap().get_subsurface(
            &late_child,
            &surface,
            &qh,
            (),
        );
        f.double_roundtrip(id);
        assert_eq!(
            f.client(id)
                .state
                .surface_scales
                .get(&late_child)
                .copied()
                .unwrap_or(1),
            expected
        );
    }
}

#[test]
fn shell_repaints_and_exclusive_zone_changes_keep_workspace_geometry_current() {
    for scale in [1., 1.25, 1.5, 2.] {
        let mut f = Fixture::new();
        f.add_output(1, (1280, 800));
        let output = f.realand_output(1);
        output.change_current_state(
            None,
            None,
            Some(smithay::output::Scale::Fractional(scale)),
            None,
        );
        f.realand().output_resized(&output);
        let client = f.add_client();
        let panel = f
            .client(client)
            .create_layer(None, Layer::Top, "live-panel");
        let surface = panel.surface.clone();
        panel.set_configure_props(LayerConfigureProps {
            anchor: Some(Anchor::Top | Anchor::Left | Anchor::Right),
            size: Some((0, 40)),
            exclusive_zone: Some(40),
            ..Default::default()
        });
        panel.commit();
        f.double_roundtrip(client);
        let panel = f.client(client).layer(&surface);
        panel.attach_new_buffer();
        let (w, h) = panel.configures_received.last().unwrap().1.size;
        panel.set_size(w as u16, h as u16);
        panel.ack_last_and_commit();
        f.double_roundtrip(client);
        for zone in [40, 40, 80, 80, 0, 0, 40] {
            let panel = f.client(client).layer(&surface);
            panel.set_configure_props(LayerConfigureProps {
                exclusive_zone: Some(zone),
                ..Default::default()
            });
            panel.attach_new_buffer();
            panel.commit();
            f.double_roundtrip(client);
            let expected = smithay::desktop::layer_map_for_output(&output)
                .non_exclusive_zone()
                .to_f64();
            assert_eq!(expected.loc.y, f64::from(zone));
            assert!(f
                .realand()
                .layout
                .workspaces()
                .all(|(_, _, workspace)| workspace.working_area() == expected));
        }
        f.client(client).layer(&surface).layer_surface.destroy();
        f.double_roundtrip(client);
        assert_eq!(
            f.realand()
                .layout
                .active_workspace()
                .unwrap()
                .working_area()
                .loc
                .y,
            0.
        );
    }
}

#[test]
#[ignore = "CPU-only layer commit microbenchmark; excludes protocol roundtrip, GPU and presentation"]
fn shell_repaint_commit_cost_for_128_windows() {
    use std::time::Instant;
    for windows in [0, 128] {
        let mut f = Fixture::new();
        f.add_output(1, (1280, 800));
        let client = f.add_client();
        for _ in 0..windows {
            let window = f.client(client).create_window();
            let surface = window.surface.clone();
            window.commit();
            f.double_roundtrip(client);
            let window = f.client(client).window(&surface);
            window.attach_new_buffer();
            window.set_size(100, 100);
            window.ack_last_and_commit();
            f.double_roundtrip(client);
        }
        let panel = f
            .client(client)
            .create_layer(None, Layer::Background, "video-wallpaper");
        let surface = panel.surface.clone();
        panel.set_configure_props(LayerConfigureProps {
            anchor: Some(Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right),
            size: Some((0, 0)),
            ..Default::default()
        });
        panel.commit();
        f.double_roundtrip(client);
        let panel = f.client(client).layer(&surface);
        panel.attach_new_buffer();
        panel.set_size(1280, 800);
        panel.ack_last_and_commit();
        f.double_roundtrip(client);
        f.realand_complete_animations();
        let surface = f
            .realand()
            .mapped_layer_surfaces
            .keys()
            .find(|layer| layer.namespace() == "video-wallpaper")
            .unwrap()
            .wl_surface()
            .clone();
        for _ in 0..100 {
            assert!(f.realand_state().layer_shell_handle_commit(&surface));
        }
        let mut samples = Vec::with_capacity(1000);
        for _ in 0..1000 {
            let start = Instant::now();
            assert!(f.realand_state().layer_shell_handle_commit(&surface));
            samples.push(start.elapsed().as_secs_f64() * 1_000_000.);
        }
        samples.sort_by(f64::total_cmp);
        eprintln!(
            "shell-repaint windows={windows} median_us={:.3} p95_us={:.3}",
            samples[500], samples[950]
        );
    }
}

#[test]
fn simple_top_anchor() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let layer = f.client(id).create_layer(None, Layer::Top, "");
    let surface = layer.surface.clone();
    layer.set_configure_props(LayerConfigureProps {
        anchor: Some(Anchor::Left | Anchor::Right | Anchor::Top),
        size: Some((0, 50)),
        ..Default::default()
    });
    layer.commit();
    f.roundtrip(id);

    let layer = f.client(id).layer(&surface);
    layer.attach_new_buffer();
    layer.set_size(100, 100);
    layer.ack_last_and_commit();
    f.double_roundtrip(id);

    let layer = f.client(id).layer(&surface);
    assert_snapshot!(layer.format_recent_configures(), @"size: 1920 × 50");
}

#[test]
fn margin_overflow() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let layer = f.client(id).create_layer(None, Layer::Top, "");
    let surface = layer.surface.clone();
    layer.set_configure_props(LayerConfigureProps {
        anchor: Some(Anchor::Left | Anchor::Right | Anchor::Top | Anchor::Bottom),
        margin: Some(LayerMargin {
            top: i32::MAX,
            right: i32::MAX,
            bottom: i32::MAX,
            left: i32::MAX,
        }),
        exclusive_zone: Some(i32::MAX),
        ..Default::default()
    });
    layer.commit();
    f.roundtrip(id);

    let layer = f.client(id).layer(&surface);
    layer.attach_new_buffer();
    layer.set_size(100, 100);
    layer.ack_last_and_commit();
    f.double_roundtrip(id);

    let layer = f.client(id).layer(&surface);
    assert_snapshot!(layer.format_recent_configures(), @"size: 0 × 0");

    // Add a second one for good measure.
    let layer = f.client(id).create_layer(None, Layer::Top, "");
    let surface = layer.surface.clone();
    layer.set_configure_props(LayerConfigureProps {
        anchor: Some(Anchor::Left | Anchor::Right | Anchor::Top | Anchor::Bottom),
        margin: Some(LayerMargin {
            top: i32::MAX,
            right: i32::MAX,
            bottom: i32::MAX,
            left: i32::MAX,
        }),
        exclusive_zone: Some(i32::MAX),
        ..Default::default()
    });
    layer.commit();
    f.roundtrip(id);

    let layer = f.client(id).layer(&surface);
    layer.attach_new_buffer();
    layer.set_size(100, 100);
    layer.ack_last_and_commit();
    f.double_roundtrip(id);

    let layer = f.client(id).layer(&surface);
    assert_snapshot!(layer.format_recent_configures(), @"size: 0 × 0");
}

#[test]
fn unmap_through_null_buffer() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let layer = f.client(id).create_layer(None, Layer::Top, "");
    let surface = layer.surface.clone();
    layer.set_configure_props(LayerConfigureProps {
        anchor: Some(Anchor::Left | Anchor::Right | Anchor::Top),
        size: Some((0, 50)),
        ..Default::default()
    });
    layer.commit();
    f.double_roundtrip(id);

    let layer = f.client(id).layer(&surface);
    assert_snapshot!(layer.format_recent_configures(), @"size: 1920 × 50");

    layer.attach_new_buffer();
    layer.set_size(100, 100);
    layer.ack_last_and_commit();
    f.double_roundtrip(id);

    let layer = f.client(id).layer(&surface);
    // No new configure since nothing changed.
    assert_snapshot!(layer.format_recent_configures(), @"");

    // Unmap by attaching a null buffer. This moves the surface back to pre-initial-commit stage.
    layer.attach_null();
    layer.commit();
    f.double_roundtrip(id);

    let layer = f.client(id).layer(&surface);
    // Configures must be empty because we haven't done an initial commit yet.
    assert_snapshot!(layer.format_recent_configures(), @"");

    // Do the initial commit again.
    layer.set_configure_props(LayerConfigureProps {
        anchor: Some(Anchor::Left | Anchor::Right | Anchor::Top),
        size: Some((0, 100)),
        ..Default::default()
    });
    layer.commit();
    f.double_roundtrip(id);

    let layer = f.client(id).layer(&surface);
    // This is the new initial configure.
    assert_snapshot!(layer.format_recent_configures(), @"size: 1920 × 100");

    layer.attach_new_buffer();
    layer.set_size(100, 100);
    layer.ack_last_and_commit();
    f.double_roundtrip(id);

    let layer = f.client(id).layer(&surface);
    assert_snapshot!(layer.format_recent_configures(), @"");
}

#[test]
fn multiple_commits_before_mapping() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let layer = f.client(id).create_layer(None, Layer::Top, "");
    let surface = layer.surface.clone();
    layer.set_configure_props(LayerConfigureProps {
        anchor: Some(Anchor::Left | Anchor::Right | Anchor::Top),
        size: Some((0, 50)),
        ..Default::default()
    });
    layer.commit();
    f.double_roundtrip(id);

    let layer = f.client(id).layer(&surface);
    assert_snapshot!(layer.format_recent_configures(), @"size: 1920 × 50");

    // Change something that won't cause a configure.
    layer.set_configure_props(LayerConfigureProps {
        anchor: Some(Anchor::Left | Anchor::Right | Anchor::Top),
        size: Some((0, 50)),
        kb_interactivity: Some(KeyboardInteractivity::OnDemand),
        ..Default::default()
    });
    layer.ack_last_and_commit();
    f.double_roundtrip(id);

    let layer = f.client(id).layer(&surface);
    // No new configure since the size hasn't changed.
    assert_snapshot!(layer.format_recent_configures(), @"");

    // Change something that will cause a configure.
    layer.set_configure_props(LayerConfigureProps {
        anchor: Some(Anchor::Left | Anchor::Right | Anchor::Top),
        size: Some((0, 100)),
        ..Default::default()
    });
    layer.commit();
    f.double_roundtrip(id);

    let layer = f.client(id).layer(&surface);
    // Configure with new size.
    assert_snapshot!(layer.format_recent_configures(), @"size: 1920 × 100");

    // Map.
    layer.attach_new_buffer();
    layer.set_size(100, 100);
    layer.ack_last_and_commit();
    f.double_roundtrip(id);

    let layer = f.client(id).layer(&surface);
    // No new configure since nothing changed.
    assert_snapshot!(layer.format_recent_configures(), @"");

    // Unmap by attaching a null buffer. This moves the surface back to pre-initial-commit stage.
    layer.attach_null();
    layer.commit();
    f.double_roundtrip(id);

    let layer = f.client(id).layer(&surface);
    // Configures must be empty because we haven't done an initial commit yet.
    assert_snapshot!(layer.format_recent_configures(), @"");

    // Same configure props as before, but since we unmapped, we should get a new initial
    // configure (that will happen to match the previous configure we had got while mapped).
    let surface = layer.surface.clone();
    layer.set_configure_props(LayerConfigureProps {
        anchor: Some(Anchor::Left | Anchor::Right | Anchor::Top),
        size: Some((0, 100)),
        ..Default::default()
    });
    layer.commit();
    f.double_roundtrip(id);

    let layer = f.client(id).layer(&surface);
    assert_snapshot!(layer.format_recent_configures(), @"size: 1920 × 100");

    // Change something that won't cause a configure.
    layer.set_configure_props(LayerConfigureProps {
        anchor: Some(Anchor::Left | Anchor::Right | Anchor::Top),
        size: Some((0, 100)),
        kb_interactivity: Some(KeyboardInteractivity::OnDemand),
        ..Default::default()
    });
    layer.ack_last_and_commit();
    f.double_roundtrip(id);

    let layer = f.client(id).layer(&surface);
    // No new configure since the size hasn't changed.
    assert_snapshot!(layer.format_recent_configures(), @"");

    // Change something that will cause a configure.
    layer.set_configure_props(LayerConfigureProps {
        anchor: Some(Anchor::Left | Anchor::Right | Anchor::Top),
        size: Some((0, 50)),
        ..Default::default()
    });
    layer.commit();
    f.double_roundtrip(id);

    let layer = f.client(id).layer(&surface);
    // Configure with new size.
    assert_snapshot!(layer.format_recent_configures(), @"size: 1920 × 50");
}

#[test]
fn layer_float_respects_reduced_motion_and_idle_throttling() {
    let mut config = realand_config::Config::default();
    config.layer_rules.push(realand_config::LayerRule {
        baba_is_float: Some(true),
        ..Default::default()
    });
    let mut f = Fixture::with_config(config);
    let mut config = realand_config::Config::default();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let layer = f.client(id).create_layer(None, Layer::Top, "motion-test");
    let surface = layer.surface.clone();
    layer.set_configure_props(LayerConfigureProps {
        size: Some((100, 50)),
        ..Default::default()
    });
    layer.commit();
    f.roundtrip(id);
    let layer = f.client(id).layer(&surface);
    layer.attach_new_buffer();
    layer.set_size(100, 50);
    layer.ack_last_and_commit();
    f.double_roundtrip(id);
    f.realand_complete_animations();
    let output = f.realand_output(1);
    let realand = f.realand();
    assert_eq!(realand.mapped_layer_surfaces.len(), 1);
    assert!(realand.animations_need_redraw(&output));
    realand.last_user_activity = std::time::Instant::now() - std::time::Duration::from_secs(31);
    assert!(!realand.animations_need_redraw(&output));
    realand.last_user_activity = std::time::Instant::now();
    assert!(realand.animations_need_redraw(&output));

    let mapped = realand.mapped_layer_surfaces.values_mut().next().unwrap();
    assert!(mapped.are_animations_ongoing());
    config.animations.off = true;
    mapped.update_config(&config);
    assert!(!mapped.are_animations_ongoing());
    assert_eq!(mapped.bob_offset(), (0., 0.).into());
    config.animations.off = false;
    mapped.update_config(&config);
    assert!(mapped.are_animations_ongoing());
    realand.clock.set_complete_instantly(true);
    assert!(!mapped.are_animations_ongoing());
    assert_eq!(mapped.bob_offset(), (0., 0.).into());
    realand.clock.set_complete_instantly(false);
    assert!(mapped.are_animations_ongoing());
}

fn core_focus_app(f: &mut Fixture, app_id: u64) -> smithay::desktop::Window {
    f.realand()
        .layout
        .windows()
        .find(|(_, window)| window.id().get() == app_id)
        .unwrap()
        .1
        .window
        .clone()
}

#[test]
fn core_focus_requests_are_consumed_at_the_immediate_protocol_boundary() {
    let (mut f, _, app_focus, app_id) = layer_focus_fixture();
    let window = core_focus_app(&mut f, app_id);
    assert!(f.realand().layout.minimize_window(&window));
    // No event-loop roundtrip between the core command and this discrete edge.
    f.realand_state().update_keyboard_focus();
    assert_eq!(
        f.realand().keyboard_focus,
        KeyboardFocus::Layout { surface: None }
    );
    assert!(f
        .realand()
        .seat
        .get_keyboard()
        .unwrap()
        .current_focus()
        .is_none());
    let mut remaining = Vec::new();
    f.realand().layout.drain_core_protocol_ops(&mut remaining);
    assert!(
        remaining.is_empty(),
        "seat reconciliation must consume core requests"
    );

    assert!(f.realand().layout.restore_window(&window));
    f.realand().layout.activate_window(&window);
    f.realand_state().update_keyboard_focus();
    assert_eq!(f.realand().keyboard_focus, app_focus);
    f.realand().layout.drain_core_protocol_ops(&mut remaining);
    assert!(remaining.is_empty());
}

#[test]
fn core_focus_requests_never_override_an_exclusive_shell_surface() {
    let (mut f, client, _, app_id) = layer_focus_fixture();
    let window = core_focus_app(&mut f, app_id);
    map_focus_layer(
        &mut f,
        client,
        Layer::Overlay,
        "core-focus-exclusive",
        KeyboardInteractivity::Exclusive,
    );
    assert_layer_keyboard_focus(&mut f, "core-focus-exclusive");
    assert!(f.realand().layout.minimize_window(&window));
    f.realand_state().update_keyboard_focus();
    assert_layer_keyboard_focus(&mut f, "core-focus-exclusive");
    assert!(f.realand().layout.restore_window(&window));
    f.realand().layout.activate_window(&window);
    f.realand_state().update_keyboard_focus();
    assert_layer_keyboard_focus(&mut f, "core-focus-exclusive");
    let mut remaining = Vec::new();
    f.realand().layout.drain_core_protocol_ops(&mut remaining);
    assert!(remaining.is_empty());
}

#[test]
fn remembered_core_focus_has_no_seat_target_without_an_output() {
    let (mut f, _, app_focus, app_id) = layer_focus_fixture();
    let window = core_focus_app(&mut f, app_id);
    let output = f.realand_output(1);
    f.realand().remove_output(&output);
    f.realand_state().update_keyboard_focus();
    assert_eq!(
        f.realand().keyboard_focus,
        KeyboardFocus::Layout { surface: None }
    );
    assert!(f
        .realand()
        .seat
        .get_keyboard()
        .unwrap()
        .current_focus()
        .is_none());
    // Restoring a connector cannot revive a stale cached wl_surface target.
    f.add_output(2, (1024, 768));
    f.realand().layout.activate_window(&window);
    f.realand_state().update_keyboard_focus();
    assert_eq!(f.realand().keyboard_focus, app_focus);
}
