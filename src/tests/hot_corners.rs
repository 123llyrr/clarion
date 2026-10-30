use clarion_config::Config;
use smithay::input::pointer::{Focus, GrabStartData, MotionEvent};
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer;
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::Anchor;
use smithay::reexports::wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};
use smithay::utils::{Point, SERIAL_COUNTER};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};

use super::client::LayerConfigureProps;
use super::Fixture;
use crate::compositor::HotCornerKind;
use crate::input::spatial_movement_grab::SpatialMovementGrab;
use crate::layout::OverviewMode;

fn setup() -> Fixture {
    let mut config = Config::default();
    config.gestures.hot_corners.top_right = true;
    config.gestures.hot_corners.top_left = true;
    let mut f = Fixture::with_config(config);
    f.add_output(1, (1280, 800));
    f.clarion_complete_animations();
    f
}

#[test]
fn overlay_surface_at_a_hot_corner_keeps_its_input() {
    let mut f = setup();
    let client = f.add_client();
    let panel = f
        .client(client)
        .create_layer(None, Layer::Overlay, "corner-overlay");
    let surface = panel.surface.clone();
    panel.set_configure_props(LayerConfigureProps {
        anchor: Some(Anchor::Top | Anchor::Right),
        size: Some((100, 60)),
        ..Default::default()
    });
    panel.commit();
    f.double_roundtrip(client);
    let panel = f.client(client).layer(&surface);
    panel.attach_new_buffer();
    panel.set_size(100, 60);
    panel.ack_last_and_commit();
    f.double_roundtrip(client);
    f.clarion_complete_animations();

    let under = f.clarion().contents_under(Point::from((1276., 4.)));
    assert!(
        under.layer.is_some(),
        "a visible overlay must take precedence over the corner"
    );
    assert!(under.surface.is_some());
    assert!(under.hot_corner.is_none());
}

pub(super) fn motion(f: &mut Fixture, pos: (f64, f64)) {
    let pos = Point::from(pos);
    let under = f.clarion().contents_under(pos);
    let pointer = f.clarion().seat.get_pointer().unwrap();
    pointer.motion(
        f.clarion_state(),
        under.surface,
        &MotionEvent {
            location: pos,
            serial: SERIAL_COUNTER.next_serial(),
            time: smithay::backend::input::InputTime::from_millis(1),
        },
    );
    pointer.frame(f.clarion_state());
    f.clarion_state()
        .process_hot_corner_motion(pos, under.output.zip(under.hot_corner));
}

#[test]
fn hot_corner_hysteresis_survives_surface_focus_in_the_exit_halo() {
    let mut f = setup();
    motion(&mut f, (1276., 4.));
    assert!(f.clarion().layout.is_overview_open());
    assert_eq!(f.clarion().layout.overview_mode(), OverviewMode::Expo);
    for x in [1271., 1274., 1269., 1276.] {
        motion(&mut f, (x, 4.));
        assert!(f.clarion().layout.is_overview_open());
    }
    // The eight-pixel trigger ends at x=1272; the latch ends four pixels further out.
    motion(&mut f, (1267., 4.));
    assert!(f.clarion().hot_corner_latch().is_none());
    motion(&mut f, (1276., 4.));
    assert!(!f.clarion().layout.is_overview_open());
}

#[test]
fn absolute_corner_jumps_track_corner_and_output_identity() {
    let mut f = setup();
    let first = f.clarion_output(1);
    assert!(f.clarion().hot_corner_motion_entered(
        (1276., 4.).into(),
        Some((first.clone(), HotCornerKind::TopRight)),
    ));
    assert!(f
        .clarion()
        .hot_corner_motion_entered((4., 4.).into(), Some((first, HotCornerKind::TopLeft)),));
    f.add_output(2, (1280, 800));
    let second = f.clarion_output(2);
    let geo = f.clarion().global_space.output_geometry(&second).unwrap();
    let pos = geo.loc.to_f64() + Point::from((4., 4.));
    assert!(f
        .clarion()
        .hot_corner_motion_entered(pos, Some((second.clone(), HotCornerKind::TopLeft))));
    f.clarion().remove_output(&second);
    assert!(f.clarion().hot_corner_latch().is_none());
}

#[test]
fn blocked_grabs_and_modal_ui_do_not_trigger_hot_corners() {
    let mut f = setup();
    let output = f.clarion_output(1);
    let workspace = f.clarion().layout.active_workspace().unwrap().id();
    let pointer = f.clarion().seat.get_pointer().unwrap();
    let grab = SpatialMovementGrab::new(
        GrabStartData {
            focus: None,
            button: 0x110,
            location: (1275., 4.).into(),
        },
        output,
        workspace,
        false,
        false,
    );
    pointer.set_grab(
        f.clarion_state(),
        grab,
        SERIAL_COUNTER.next_serial(),
        Focus::Clear,
    );
    motion(&mut f, (1276., 4.));
    assert!(!f.clarion().layout.is_overview_open());
    assert!(f.clarion().hot_corner_latch().is_none());
    pointer.unset_grab(f.clarion_state(), SERIAL_COUNTER.next_serial(), smithay::backend::input::InputTime::from_millis(1));
    assert!(f.clarion().exit_confirm_dialog.show());
    motion(&mut f, (1276., 4.));
    assert!(!f.clarion().layout.is_overview_open());
    assert!(f.clarion().hot_corner_latch().is_none());
}

#[test]
fn an_already_moving_compositor_window_keeps_hot_corner_input_priority() {
    use smithay::backend::input::ButtonState;
    use smithay::input::pointer::ButtonEvent;

    use crate::input::move_grab::MoveGrab;
    use crate::input::AnyStartData;

    let mut f = setup();
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
    f.clarion_complete_animations();
    let window = f.clarion().layout.active_window().unwrap().window.clone();
    let pointer = f.clarion().seat.get_pointer().unwrap();
    pointer.button(
        f.clarion_state(),
        &ButtonEvent {
            button: 0x110,
            state: ButtonState::Pressed,
            serial: SERIAL_COUNTER.next_serial(),
            time: smithay::backend::input::InputTime::from_millis(10),
        },
    );
    let grab = MoveGrab::new(
        f.clarion_state(),
        AnyStartData::Pointer(GrabStartData {
            focus: None,
            button: 0x110,
            location: (40., 40.).into(),
        }),
        window,
        false,
        None,
    )
    .unwrap();
    pointer.set_grab(
        f.clarion_state(),
        grab,
        SERIAL_COUNTER.next_serial(),
        Focus::Clear,
    );
    motion(&mut f, (64., 64.));
    assert!(pointer
        .with_grab(|_, grab| grab.as_any().downcast_ref::<MoveGrab>().unwrap().is_move())
        .unwrap());
    motion(&mut f, (1276., 4.));
    assert!(!f.clarion().layout.is_overview_open());
    assert!(f.clarion().hot_corner_latch().is_none());
    assert!(pointer.is_grabbed());
    pointer.unset_grab(f.clarion_state(), SERIAL_COUNTER.next_serial(), smithay::backend::input::InputTime::from_millis(30));
}

#[test]
fn touch_and_tablet_compositor_grabs_block_pointer_hot_corners() {
    use std::time::Duration;

    use smithay::backend::input::{TabletToolCapabilities, TabletToolDescriptor, TabletToolType};
    use smithay::input::tablet::{tool, TabletDescriptor, TabletSeatTrait};
    use smithay::input::touch::DownEvent;

    use crate::input::touch_overview_grab::TouchOverviewGrab;
    use crate::input::AnyStartData;

    let mut f = setup();
    let output = f.clarion_output(1);
    let workspace = f.clarion().layout.active_workspace().unwrap().id();
    let position = (40., 40.).into();
    let touch = f.clarion().seat.add_touch();
    touch.down(
        f.clarion_state(),
        None,
        &DownEvent {
            slot: Some(1).into(),
            location: position,
            serial: SERIAL_COUNTER.next_serial(),
            time: smithay::backend::input::InputTime::from_millis(10),
        },
    );
    let grab = TouchOverviewGrab::new(
        AnyStartData::Touch(smithay::input::touch::GrabStartData {
            focus: None,
            slot: Some(1).into(),
            location: position,
        }),
        Duration::from_millis(10),
        output.clone(),
        position,
        Some(workspace),
        false,
        None,
        OverviewMode::Stage,
    );
    touch.set_grab(f.clarion_state(), grab, SERIAL_COUNTER.next_serial());
    motion(&mut f, (1276., 4.));
    assert!(!f.clarion().layout.is_overview_open());
    assert!(f.clarion().hot_corner_latch().is_none());
    assert!(touch.is_grabbed());
    touch.cancel(f.clarion_state());

    let tablet_seat = f.clarion().seat.tablet_seat();
    let tablet = tablet_seat.add_tablet(&TabletDescriptor {
        name: "Hot corner tablet".into(),
        usb_id: None,
        syspath: None,
    });
    let pen = tablet_seat.add_tool(&TabletToolDescriptor {
        tool_type: TabletToolType::Pen,
        hardware_serial: 19,
        hardware_id_wacom: 0,
        capabilities: TabletToolCapabilities::empty(),
    });
    pen.proximity_in(
        f.clarion_state(),
        None,
        tablet,
        &tool::ProximityInEvent {
            location: position,
            axis: None,
            serial: SERIAL_COUNTER.next_serial(),
            time: smithay::backend::input::InputTime::from_millis(10),
        },
    );
    pen.down(
        f.clarion_state(),
        &tool::DownEvent {
            serial: SERIAL_COUNTER.next_serial(),
            time: smithay::backend::input::InputTime::from_millis(10),
        },
    );
    let grab = TouchOverviewGrab::new(
        AnyStartData::TabletTool(tool::GrabStartData {
            focus: None,
            trigger: tool::GrabTrigger::Tip,
            location: position,
        }),
        Duration::from_millis(10),
        output,
        position,
        Some(workspace),
        false,
        None,
        OverviewMode::Stage,
    );
    pen.set_grab(
        f.clarion_state(),
        grab,
        smithay::backend::input::InputTime::from_millis(10),
        SERIAL_COUNTER.next_serial(),
        Focus::Clear,
    );
    motion(&mut f, (1276., 4.));
    assert!(!f.clarion().layout.is_overview_open());
    assert!(f.clarion().hot_corner_latch().is_none());
    assert!(pen.is_grabbed());
    pen.unset_grab(f.clarion_state(), SERIAL_COUNTER.next_serial(), smithay::backend::input::InputTime::from_millis(30));
}

impl Dispatch<ZwlrVirtualPointerManagerV1, ()> for super::client::State {
    fn event(
        _: &mut Self,
        _: &ZwlrVirtualPointerManagerV1,
        _: <ZwlrVirtualPointerManagerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrVirtualPointerV1, ()> for super::client::State {
    fn event(
        _: &mut Self,
        _: &ZwlrVirtualPointerV1,
        _: <ZwlrVirtualPointerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

#[test]
fn real_relative_and_absolute_pointer_events_share_the_latch_and_handoff() {
    let mut f = setup();
    f.init_gles_renderer();
    let client = f.add_client();
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
    pointer.motion_absolute(1, 1276, 4, 1280, 800);
    f.double_roundtrip(client);
    assert!(f.clarion().layout.is_overview_open());
    assert_eq!(f.clarion().layout.overview_mode(), OverviewMode::Expo);
    pointer.motion(2, -5., 0.);
    f.double_roundtrip(client);
    pointer.motion_absolute(3, 1276, 4, 1280, 800);
    f.double_roundtrip(client);
    assert!(
        f.clarion().layout.is_overview_open(),
        "entering through another motion backend must retain the halo latch"
    );
    // Start the close from a fully visible Expo. Protocol roundtrips clear the
    // cached clock and may include software rendering under parallel test load;
    // keep adjusted animation time fixed while testing the input-backend handoff.
    // Otherwise Expo can legitimately finish closing before the next corner event.
    f.clarion_complete_animations();
    f.clarion().clock.set_rate(0.);
    pointer.motion_absolute(4, 1267, 4, 1280, 800);
    f.double_roundtrip(client);
    pointer.motion(5, 9., 0.);
    f.double_roundtrip(client);
    assert!(!f.clarion().layout.is_overview_open());
    assert!(
        f.clarion().layout.is_overview_visible(),
        "the close must still be visible when the other input backend enters Stage"
    );
    pointer.motion(6, -1272., 0.);
    f.double_roundtrip(client);
    assert_eq!(f.clarion().layout.overview_mode(), OverviewMode::Stage);
    assert!(f.clarion().overview_mode_transition_active());
}

#[test]
fn animated_layer_commits_do_not_rearm_either_top_hot_corner_at_two_times_scale() {
    for (corner, mode, x) in [
        (HotCornerKind::TopLeft, OverviewMode::Stage, 4),
        (HotCornerKind::TopRight, OverviewMode::Expo, 1916),
    ] {
        for layer in [Layer::Background, Layer::Bottom, Layer::Top] {
            let mut config = Config::default();
            config.gestures.hot_corners.top_left = true;
            config.gestures.hot_corners.top_right = true;
            config.hotkey_overlay.skip_at_startup = true;
            let mut f = Fixture::with_config(config);
            f.add_output(1, (3840, 2400));
            let output = f.clarion_output(1);
            output.change_current_state(None, None, Some(smithay::output::Scale::Integer(2)), None);
            f.clarion().output_resized(&output);
            let client = f.add_client();
            let panel = f.client(client).create_layer(None, layer, "animated-shell");
            let surface = panel.surface.clone();
            panel.set_configure_props(LayerConfigureProps {
                anchor: Some(Anchor::Top | Anchor::Left),
                size: Some((1920, 40)),
                ..Default::default()
            });
            panel.commit();
            f.double_roundtrip(client);
            let panel = f.client(client).layer(&surface);
            panel.attach_new_buffer();
            panel.set_size(1920, 40);
            panel.ack_last_and_commit();
            f.double_roundtrip(client);
            f.clarion_complete_animations();

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
            pointer.motion_absolute(1, x, 4, 1920, 1200);
            pointer.frame();
            f.double_roundtrip(client);
            assert!(f.clarion().layout.is_overview_open());
            assert_eq!(f.clarion().layout.overview_mode(), mode);

            // An animated wallpaper, visualizer or panel submits a new buffer
            // between tiny absolute/relative motions while the mouse stays put.
            // Exercise opening, settled and closing overview states.
            for phase in 0..3 {
                if phase == 1 {
                    f.clarion_complete_animations();
                } else if phase == 2 {
                    pointer.motion_absolute(20, 960, 600, 1920, 1200);
                    pointer.frame();
                    f.double_roundtrip(client);
                    pointer.motion_absolute(21, x, 4, 1920, 1200);
                    pointer.frame();
                    f.double_roundtrip(client);
                    assert!(!f.clarion().layout.is_overview_open());
                }
                for frame in 0..3 {
                    let panel = f.client(client).layer(&surface);
                    panel.attach_new_buffer();
                    panel.commit();
                    f.double_roundtrip(client);
                    assert_eq!(
                        f.clarion().hot_corner_latch(),
                        Some((output.clone(), corner)),
                        "a shell redraw must retain the corner latch: {layer:?}, {mode:?}, phase={phase}"
                    );
                    if frame % 2 == 0 {
                        pointer.motion_absolute(30 + frame, x, 4, 1920, 1200);
                    } else {
                        pointer.motion(30 + frame, 0., 1.);
                    }
                    pointer.frame();
                    f.double_roundtrip(client);
                    assert_eq!(
                        f.clarion().layout.is_overview_open(),
                        phase != 2,
                        "one corner visit must never toggle twice: {layer:?}, {mode:?}, phase={phase}"
                    );
                    assert_eq!(f.clarion().layout.overview_mode(), mode);
                }
            }
        }
    }
}

#[test]
fn layer_lifecycle_keeps_hot_corner_mode_handoffs_and_latches() {
    for (first_x, second_x, mode, corner) in [
        (4, 1276, OverviewMode::Expo, HotCornerKind::TopRight),
        (1276, 4, OverviewMode::Stage, HotCornerKind::TopLeft),
    ] {
        let mut f = setup();
        f.init_gles_renderer();
        let output = f.clarion_output(1);
        let client = f.add_client();
        let panel = f
            .client(client)
            .create_layer(None, Layer::Bottom, "live-visualizer");
        let surface = panel.surface.clone();
        panel.set_configure_props(LayerConfigureProps {
            anchor: Some(Anchor::Bottom | Anchor::Left),
            size: Some((100, 40)),
            ..Default::default()
        });
        panel.commit();
        f.double_roundtrip(client);
        let panel = f.client(client).layer(&surface);
        panel.attach_new_buffer();
        panel.set_size(100, 40);
        panel.ack_last_and_commit();
        f.double_roundtrip(client);
        f.clarion_complete_animations();
        motion(&mut f, (f64::from(first_x), 4.));
        f.clarion_complete_animations();
        // Client roundtrips refresh the wall clock. Hold the animation clock so
        // these assertions compare shell lifecycle changes at one frame.
        f.clarion().clock.set_rate(0.);
        motion(&mut f, (f64::from(second_x), 4.));
        assert_eq!(f.clarion().layout.overview_mode(), mode);
        assert!(f.clarion().overview_mode_transition_active());
        let original_alpha = f.clarion().output_state[&output]
            .screen_transition
            .as_ref()
            .unwrap()
            .alpha();

        // All of these change the shell, but none changes the output coordinate
        // system that the corner latch and captured handoff frame refer to.
        for step in 0..4 {
            let panel = f.client(client).layer(&surface);
            match step {
                0 => {
                    panel.attach_new_buffer();
                    panel.commit();
                }
                1 => {
                    panel.set_configure_props(LayerConfigureProps {
                        size: Some((120, 40)),
                        exclusive_zone: Some(40),
                        ..Default::default()
                    });
                    panel.set_size(120, 40);
                    panel.commit();
                }
                2 => {
                    panel.attach_null();
                    panel.commit();
                }
                _ => panel.layer_surface.destroy(),
            }
            f.double_roundtrip(client);
            let transition = f.clarion().output_state[&output]
                .screen_transition
                .as_ref()
                .expect("shell updates must not cut off the mode handoff animation");
            assert_eq!(transition.alpha(), original_alpha);
            assert_eq!(
                f.clarion().hot_corner_latch(),
                Some((output.clone(), corner))
            );
            motion(&mut f, (f64::from(second_x), 4.));
            assert!(f.clarion().layout.is_overview_open());
            assert_eq!(f.clarion().layout.overview_mode(), mode);
        }
    }
}

#[test]
fn output_coordinate_changes_clear_hot_corner_latches() {
    for change in 0..3 {
        let mut f = setup();
        let output = f.clarion_output(1);
        motion(&mut f, (4., 4.));
        assert!(f.clarion().hot_corner_latch().is_some());
        match change {
            0 => {
                let mut mode = output.current_mode().unwrap();
                mode.size = (1920, 1200).into();
                output.change_current_state(Some(mode), None, None, None);
            }
            1 => output.change_current_state(
                None,
                None,
                Some(smithay::output::Scale::Fractional(1.25)),
                None,
            ),
            _ => {
                output.change_current_state(None, Some(smithay::utils::Transform::_90), None, None)
            }
        }
        f.clarion().output_resized(&output);
        assert!(
            f.clarion().hot_corner_latch().is_none(),
            "an actual output coordinate change must clear stale corner coordinates"
        );
    }
}
