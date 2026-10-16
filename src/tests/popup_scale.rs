use clarion_config::Config;
use smithay::reexports::wayland_protocols::xdg::shell::client::xdg_popup::XdgPopup;
use smithay::reexports::wayland_protocols::xdg::shell::client::xdg_positioner::XdgPositioner;

use super::client::State as ClientState;
use super::Fixture;
use crate::layout::ActivateWindow;

wayland_client::delegate_noop!(ClientState: ignore XdgPopup);
wayland_client::delegate_noop!(ClientState: ignore XdgPositioner);

#[test]
#[allow(clippy::mutable_key_type)]
fn popup_subsurfaces_inherit_output_scale_before_and_after_popup_creation() {
    let config = Config::parse_mem("output \"headless-1\" { scale 2; }").unwrap();
    let mut f = Fixture::with_config(config);
    f.add_output(1, (1920, 1080));

    let id = f.add_client();
    let window = f.client(id).create_window();
    let parent_surface = window.surface.clone();
    let parent_xdg_surface = window.xdg_surface.clone();
    window.commit();
    f.roundtrip(id);
    let window = f.client(id).window(&parent_surface);
    window.attach_new_buffer();
    window.ack_last_and_commit();
    f.double_roundtrip(id);

    let client = f.client(id);
    let qh = client.qh.clone();
    let compositor = client.state.compositor.as_ref().unwrap();
    let subcompositor = client.state.subcompositor.as_ref().unwrap();
    let xdg_wm_base = client.state.xdg_wm_base.as_ref().unwrap();

    let popup_surface = compositor.create_surface(&qh, ());
    let early_child = compositor.create_surface(&qh, ());
    let _early_subsurface = subcompositor.get_subsurface(&early_child, &popup_surface, &qh, ());

    let popup_xdg_surface = xdg_wm_base.get_xdg_surface(&popup_surface, &qh, ());
    let positioner = xdg_wm_base.create_positioner(&qh, ());
    positioner.set_size(20, 20);
    positioner.set_anchor_rect(0, 0, 1, 1);
    let _popup = popup_xdg_surface.get_popup(Some(&parent_xdg_surface), &positioner, &qh, ());

    let late_child = compositor.create_surface(&qh, ());
    let _late_subsurface = subcompositor.get_subsurface(&late_child, &popup_surface, &qh, ());
    f.roundtrip(id);

    let scales = &f.client(id).state.surface_scales;
    assert_eq!(scales.get(&popup_surface), Some(&2));
    assert_eq!(scales.get(&early_child), Some(&2));
    assert_eq!(scales.get(&late_child), Some(&2));

    // A popup remains attached to its parent window as it crosses displays. Both its root and
    // existing child surfaces must receive the destination output's new scale.
    f.add_output(2, (1920, 1080));
    let output = f.clarion_output(2);
    let window = f
        .clarion()
        .layout
        .windows()
        .next()
        .unwrap()
        .1
        .window
        .clone();
    f.clarion()
        .layout
        .move_to_output(Some(&window), &output, None, ActivateWindow::Yes);
    f.double_roundtrip(id);

    let scales = &f.client(id).state.surface_scales;
    assert_eq!(scales.get(&popup_surface), Some(&1));
    assert_eq!(scales.get(&early_child), Some(&1));
    assert_eq!(scales.get(&late_child), Some(&1));
}
