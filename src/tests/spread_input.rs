use clarion_config::Config;
use clarion_ipc::PositionChange;
use smithay::desktop::Window;
use smithay::input::keyboard::Keysym;
use smithay::input::pointer::MotionEvent;
use smithay::output::Output;
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer;
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    Anchor, KeyboardInteractivity,
};
use smithay::utils::{Logical, Point, SERIAL_COUNTER};

use super::client::LayerConfigureProps;
use super::Fixture;
use crate::layout::HitType;

fn setup() -> (Fixture, Window, Window) {
    let mut config = Config::default();
    config.layout.auto_stack_new_windows = false;
    config.input.focus_follows_mouse = Some(clarion_config::input::FocusFollowsMouse {
        max_scroll_amount: None,
    });
    let mut f = Fixture::with_config(config);
    f.add_output(1, (1280, 800));
    let client = f.add_client();
    let mut windows = Vec::new();
    for x in [200., 600.] {
        let window = f.client(client).create_window();
        let surface = window.surface.clone();
        window.commit();
        f.double_roundtrip(client);
        let window = f.client(client).window(&surface);
        window.attach_new_buffer();
        window.set_size(100, 100);
        window.ack_last_and_commit();
        f.double_roundtrip(client);
        let window = f.clarion().layout.active_window().unwrap().window.clone();
        let layout = &mut f.clarion().layout;
        layout.set_window_floating(Some(&window), true);
        layout.move_floating_window(
            Some(&window),
            PositionChange::SetFixed(x),
            PositionChange::SetFixed(200.),
            false,
        );
        windows.push(window);
    }
    let first = windows.remove(0);
    let second = windows.remove(0);
    f.clarion().layout.activate_window(&first);
    f.clarion_complete_animations();
    let pointer = f.clarion().seat.get_pointer().unwrap();
    pointer.motion(
        f.clarion_state(),
        None,
        &MotionEvent {
            location: Point::from((100., 100.)),
            serial: SERIAL_COUNTER.next_serial(),
            time: 1,
        },
    );
    pointer.frame(f.clarion_state());
    (f, first, second)
}

fn preview_point(f: &mut Fixture, output: &Output, window: &Window) -> Point<f64, Logical> {
    // Find a point using the production hit test, without exposing grid geometry
    // merely for a fixture. Mock client surfaces are 100 x 100, so 16 px steps
    // also cover the transformed preview after the grid has settled.
    for y in (80..800).step_by(16) {
        for x in (16..1280).step_by(16) {
            let point = Point::from((x as f64, y as f64));
            if f.clarion()
                .layout
                .window_under(output, point)
                .is_some_and(|(mapped, _)| mapped.window == *window)
            {
                return point;
            }
        }
    }
    panic!("window has no visible preview hit");
}

#[test]
fn closing_spread_previews_do_not_activate_or_deliver_client_input() {
    let (mut f, first, second) = setup();
    let output = f.clarion_output(1);
    f.clarion().layout.toggle_window_spread();
    f.clarion_complete_animations();
    let point = preview_point(&mut f, &output, &second);
    assert!(f.clarion().contents_under(point).window.is_some());
    f.clarion().layout.spread_key(Keysym::Escape);
    assert!(f.clarion().layout.is_window_spread_closing());

    // The compositor still paints this preview, but it cannot receive a real
    // app click or become the target of a new move/resize grab on the way out.
    let (mapped, hit) = f.clarion().layout.window_under(&output, point).unwrap();
    assert_eq!(mapped.window, second);
    assert!(matches!(hit, HitType::Activate { .. }));
    assert!(f.clarion().window_under(point).is_none());
    let under = f.clarion().contents_under(point);
    assert_eq!(under.output.as_ref(), Some(&output));
    assert!(under.window.is_none());
    assert!(under.surface.is_none());

    f.clarion_complete_animations();
    assert!(!f.clarion().layout.is_window_spread_visible());
    let normal = f.clarion().contents_under(Point::from((220., 220.)));
    assert_eq!(normal.window.unwrap().0, first);
    assert!(
        normal.surface.is_some(),
        "normal client input resumes at 1:1"
    );
}

#[test]
fn closing_spread_ignores_stale_hover_then_restores_focus_follows_mouse() {
    let (mut f, first, second) = setup();
    let output = f.clarion_output(1);
    f.clarion().layout.toggle_window_spread();
    f.clarion_complete_animations();
    let point = preview_point(&mut f, &output, &second);
    // This is the pointer snapshot computed just before the closing action.
    let stale_hover = f.clarion().contents_under(point);
    assert_eq!(stale_hover.window.as_ref().unwrap().0, second);
    f.clarion().layout.spread_key(Keysym::Escape);
    f.clarion().handle_focus_follows_mouse(&stale_hover);
    assert_eq!(f.clarion().layout.active_window().unwrap().window, first);

    f.clarion_complete_animations();
    let normal = f.clarion().contents_under(Point::from((620., 220.)));
    assert_eq!(normal.window.as_ref().unwrap().0, second);
    assert!(normal.surface.is_some());
    f.clarion().handle_focus_follows_mouse(&normal);
    assert_eq!(f.clarion().layout.active_window().unwrap().window, second);
}

#[test]
fn closing_spread_keeps_top_panel_input_and_on_demand_hover_focus() {
    let (mut f, _, _) = setup();
    let client = f.add_client();
    let panel = f
        .client(client)
        .create_layer(None, Layer::Top, "spread-panel");
    let surface = panel.surface.clone();
    panel.set_configure_props(LayerConfigureProps {
        anchor: Some(Anchor::Top | Anchor::Left | Anchor::Right),
        size: Some((0, 60)),
        kb_interactivity: Some(KeyboardInteractivity::OnDemand),
        ..Default::default()
    });
    panel.commit();
    f.double_roundtrip(client);
    let panel = f.client(client).layer(&surface);
    panel.attach_new_buffer();
    panel.set_size(1280, 60);
    panel.ack_last_and_commit();
    f.double_roundtrip(client);
    f.clarion_complete_animations();

    f.clarion().layout.toggle_window_spread();
    f.clarion_complete_animations();
    f.clarion().layout.spread_key(Keysym::Escape);
    assert!(f.clarion().layout.is_window_spread_closing());
    let under = f.clarion().contents_under(Point::from((640., 20.)));
    assert!(under.surface.is_some());
    assert!(under.layer.is_some());
    assert!(under.window.is_none());
    f.clarion().handle_focus_follows_mouse(&under);
    assert_eq!(f.clarion().layer_shell_on_demand_focus, under.layer);
}
