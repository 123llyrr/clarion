use clarion_ipc::SizeChange;
use smithay::reexports::wayland_server::{
    protocol::wl_surface::WlSurface as ServerSurface, Resource as _,
};
use smithay::utils::{Logical, Rectangle, Serial, Size};
use smithay::wayland::compositor::{add_blocker, Barrier, CompositorHandler as _};
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::Proxy as _;

use super::client::ClientId;
use super::Fixture;

type SnapshotSource = (Serial, Rectangle<i32, Logical>, Option<Size<i32, Logical>>);

fn setup(explicit_geometry: bool) -> (Fixture, ClientId, WlSurface, ServerSurface) {
    let mut f = Fixture::new();
    f.add_output(1, (1280, 720));
    let id = f.add_client();
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.double_roundtrip(id);
    let window = f.client(id).window(&surface);
    window.attach_new_buffer();
    window.set_size(200, 180);
    if explicit_geometry {
        window.xdg_surface.set_window_geometry(11, 13, 160, 130);
    }
    window.ack_last_and_commit();
    f.double_roundtrip(id);
    // Settle any activation configure sent by mapping the window.
    f.client(id).window(&surface).ack_last_and_commit();
    f.double_roundtrip(id);
    f.clarion_complete_animations();
    let server = f
        .clarion()
        .layout
        .windows()
        .find(|(_, mapped)| {
            mapped.toplevel().wl_surface().id().protocol_id() == surface.id().protocol_id()
        })
        .unwrap()
        .1
        .toplevel()
        .wl_surface()
        .clone();
    f.clarion()
        .layout
        .find_window_and_output_mut(&server)
        .unwrap()
        .0
        .animation_snapshot_sources
        .clear();
    (f, id, surface, server)
}

fn sources(f: &mut Fixture, server: &ServerSurface) -> Vec<SnapshotSource> {
    f.clarion()
        .layout
        .find_window_and_output(server)
        .unwrap()
        .0
        .animation_snapshot_sources
        .clone()
}

fn geometry(f: &mut Fixture, server: &ServerSurface) -> Rectangle<i32, Logical> {
    f.clarion()
        .layout
        .find_window_and_output(server)
        .unwrap()
        .0
        .window
        .geometry()
}

fn request_resize(f: &mut Fixture, id: ClientId, surface: &WlSurface, width: i32) -> Serial {
    f.clarion()
        .layout
        .set_column_width(SizeChange::SetFixed(width));
    f.double_roundtrip(id);
    f.client(id)
        .window(surface)
        .configures_received
        .last()
        .unwrap()
        .0
        .into()
}

fn notify_blocker_cleared(f: &mut Fixture, server: &ServerSurface) {
    let client = server.client().unwrap();
    let state = f.clarion_state();
    let dh = state.clarion.display_handle.clone();
    state
        .client_compositor_state(&client)
        .blocker_cleared(state, &dh);
}

#[test]
fn blocked_resize_commits_capture_each_applied_predecessor() {
    // Exercise both separate wakeups and a single wakeup that drains two commits.
    for release_together in [false, true] {
        let (mut f, id, surface, server) = setup(true);
        let initial_geometry = Rectangle::new((11, 13).into(), (160, 130).into());
        let first_geometry = Rectangle::new((17, 19).into(), (260, 230).into());
        let final_geometry = Rectangle::new((23, 29).into(), (360, 330).into());

        let first_serial = request_resize(&mut f, id, &surface, 300);
        let first_barrier = Barrier::new(false);
        add_blocker(&server, first_barrier.clone());
        let window = f.client(id).window(&surface);
        window.attach_new_buffer();
        window.set_size(300, 280);
        window.xdg_surface.set_window_geometry(17, 19, 260, 230);
        window.ack_last_and_commit();
        f.double_roundtrip(id);

        // A state change may legitimately send another animated configure while
        // a size-only configure is still in flight (which normally is throttled).
        f.client(id).window(&surface).set_fullscreen(None);
        f.double_roundtrip(id);
        let second_serial = f
            .client(id)
            .window(&surface)
            .configures_received
            .last()
            .unwrap()
            .0
            .into();
        assert_ne!(first_serial, second_serial);
        let second_barrier = Barrier::new(false);
        add_blocker(&server, second_barrier.clone());
        let window = f.client(id).window(&surface);
        window.attach_new_buffer();
        window.set_size(400, 380);
        window.xdg_surface.set_window_geometry(23, 29, 360, 330);
        window.ack_last_and_commit();
        f.double_roundtrip(id);

        assert!(
            sources(&mut f, &server).is_empty(),
            "blocked commits must not consume animation serials"
        );
        assert_eq!(geometry(&mut f, &server), initial_geometry);

        first_barrier.signal();
        if release_together {
            second_barrier.signal();
        }
        notify_blocker_cleared(&mut f, &server);
        if !release_together {
            assert_eq!(
                sources(&mut f, &server),
                vec![(first_serial, initial_geometry, Some((200, 180).into()))],
            );
            assert_eq!(geometry(&mut f, &server), first_geometry);
            second_barrier.signal();
            notify_blocker_cleared(&mut f, &server);
        }
        let expected = vec![
            (first_serial, initial_geometry, Some((200, 180).into())),
            (second_serial, first_geometry, Some((300, 280).into())),
        ];
        assert_eq!(sources(&mut f, &server), expected);
        assert_eq!(geometry(&mut f, &server), final_geometry);

        // Ordinary content commits with the same configure serial must not restart it.
        f.client(id).window(&surface).commit();
        f.double_roundtrip(id);
        assert_eq!(sources(&mut f, &server), expected);
    }
}

#[test]
fn desynchronized_child_updates_geometry_for_next_resize_snapshot() {
    let (mut f, id, surface, server) = setup(false);
    let client = f.client(id);
    let child = client
        .state
        .compositor
        .as_ref()
        .unwrap()
        .create_surface(&client.qh, ());
    let subsurface = client.state.subcompositor.as_ref().unwrap().get_subsurface(
        &child,
        &surface,
        &client.qh,
        (),
    );
    let viewport = client
        .state
        .viewporter
        .as_ref()
        .unwrap()
        .get_viewport(&child, &client.qh, ());
    subsurface.set_position(250, 60);
    subsurface.set_desync();
    // Apply the child position while it is still unmapped, then change the
    // root's bounding box using only a desynchronized child commit.
    surface.commit();
    f.double_roundtrip(id);
    let client = f.client(id);
    let buffer =
        client
            .state
            .spbm
            .as_ref()
            .unwrap()
            .create_u32_rgba_buffer(0, 0, 0, 0, &client.qh, ());
    child.attach(Some(&buffer), 0, 0);
    viewport.set_destination(80, 90);
    child.commit();
    f.double_roundtrip(id);
    let previous_geometry = Rectangle::from_size((330, 180).into());
    assert_eq!(geometry(&mut f, &server), previous_geometry);

    let serial = request_resize(&mut f, id, &surface, 500);
    let window = f.client(id).window(&surface);
    window.set_size(500, 380);
    window.ack_last_and_commit();
    f.double_roundtrip(id);
    assert_eq!(
        sources(&mut f, &server),
        vec![(serial, previous_geometry, Some((200, 180).into()))],
    );
}
