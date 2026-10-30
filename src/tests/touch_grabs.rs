use std::time::Duration;

use clarion_config::Config;
use clarion_ipc::{PositionChange, SizeChange};
use smithay::backend::input::{TabletToolCapabilities, TabletToolDescriptor, TabletToolType};
use smithay::desktop::Window;
use smithay::input::pointer::Focus;
use smithay::input::tablet::{tool, TabletDescriptor, TabletSeatTrait};
use smithay::input::touch::{DownEvent, GrabStartData, MotionEvent, TouchHandle, UpEvent};
use smithay::utils::{Logical, Point, SERIAL_COUNTER};
use wayland_client::protocol::wl_surface::WlSurface;

use super::client::ClientId;
use super::Fixture;
use crate::compositor::State;
use crate::input::move_grab::MoveGrab;
use crate::input::resize_grab::ResizeGrab;
use crate::input::touch_overview_grab::TouchOverviewGrab;
use crate::input::AnyStartData;
use crate::layout::LayoutElement as _;
use crate::utils::ResizeEdge;

const START: (f64, f64) = (220., 220.);

fn setup() -> (Fixture, ClientId, WlSurface, Window, TouchHandle<State>) {
    let mut config = Config::default();
    config.animations.off = true;
    let mut f = Fixture::with_config(config);
    f.add_output(1, (1280, 800));
    let client = f.add_client();
    let window = f.client(client).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(client);
    let window = f.client(client).window(&surface);
    window.attach_new_buffer();
    window.set_size(100, 100);
    window.ack_last_and_commit();
    f.double_roundtrip(client);
    let layout = &mut f.clarion().layout;
    layout.toggle_window_floating(None);
    layout.move_floating_window(
        None,
        PositionChange::SetFixed(200.),
        PositionChange::SetFixed(200.),
        false,
    );
    let window = layout.windows().next().unwrap().1.window.clone();
    f.clarion_complete_animations();
    f.clarion().clock.set_complete_instantly(true);
    f.double_roundtrip(client);
    f.client(client).window(&surface).ack_last_and_commit();
    f.double_roundtrip(client);
    f.client(client)
        .window(&surface)
        .recent_configures()
        .for_each(drop);
    let touch = f.clarion().seat.add_touch();
    // Both contacts precede the compositor grab, so the second contact does not
    // invoke MoveGrab's separate two-finger toggle-floating action.
    for slot in [1, 2] {
        touch.down(
            f.clarion_state(),
            None,
            &DownEvent {
                slot: Some(slot).into(),
                location: START.into(),
                serial: SERIAL_COUNTER.next_serial(),
                time: smithay::backend::input::InputTime::from_millis(10),
            },
        );
    }
    touch.frame(f.clarion_state());
    (f, client, surface, window, touch)
}

fn start_data() -> AnyStartData<State> {
    AnyStartData::Touch(GrabStartData {
        focus: None,
        slot: Some(1).into(),
        location: START.into(),
    })
}

fn motion(f: &mut Fixture, touch: &TouchHandle<State>, slot: u32, location: (f64, f64)) {
    touch.motion(
        f.clarion_state(),
        None,
        &MotionEvent {
            slot: Some(slot).into(),
            location: location.into(),
            time: smithay::backend::input::InputTime::from_millis(20),
        },
    );
}

fn up(f: &mut Fixture, touch: &TouchHandle<State>, slot: u32) {
    touch.up(
        f.clarion_state(),
        &UpEvent {
            slot: Some(slot).into(),
            serial: SERIAL_COUNTER.next_serial(),
            time: smithay::backend::input::InputTime::from_millis(21),
        },
    );
}

fn position(f: &mut Fixture) -> Point<f64, Logical> {
    f.clarion().layout.session_layout().windows[0]
        .position
        .unwrap()
        .into()
}

fn overview_grab(f: &mut Fixture, window: Window, start: AnyStartData<State>) -> TouchOverviewGrab {
    f.clarion().layout.open_overview();
    f.clarion_complete_animations();
    let (output, workspace_id) = f
        .clarion()
        .layout
        .workspaces()
        .find_map(|(mon, _, ws)| {
            ws.has_window(&window)
                .then(|| (mon.unwrap().output().clone(), ws.id()))
        })
        .unwrap();
    TouchOverviewGrab::new(
        start,
        Duration::from_millis(10),
        output,
        START.into(),
        Some(workspace_id),
        true,
        Some(window),
        f.clarion().layout.overview_mode(),
    )
}

fn grab_overview(f: &mut Fixture, window: Window, touch: &TouchHandle<State>) {
    let grab = overview_grab(f, window, start_data());
    touch.set_grab(f.clarion_state(), grab, SERIAL_COUNTER.next_serial());
}

#[test]
fn overview_tablet_release_and_proximity_out_have_distinct_semantics() {
    for (target, proximity_out, stays_open, hold) in [
        ((221., 221.), false, false, false), // Normal tap.
        ((280., 220.), false, true, false),  // Pending horizontal swipe.
        ((220., 280.), false, true, false),  // Pending vertical swipe.
        ((221., 221.), true, true, false),   // Lost proximity is not a tap.
        (START, false, true, true),          // Stationary hold is not a tap.
        (START, true, true, true),           // Held pen leaving cancels its drag.
    ] {
        let (mut f, _, _, window, _) = setup();
        let tablet_seat = f.clarion().seat.tablet_seat();
        let tablet = tablet_seat.add_tablet(&TabletDescriptor {
            name: "Overview regression tablet".into(),
            usb_id: None,
            syspath: None,
        });
        let pen = tablet_seat.add_tool(&TabletToolDescriptor {
            tool_type: TabletToolType::Pen,
            hardware_serial: 1,
            hardware_id_wacom: 0,
            capabilities: TabletToolCapabilities::empty(),
        });
        pen.proximity_in(
            f.clarion_state(),
            None,
            tablet,
            &tool::ProximityInEvent {
                location: START.into(),
                axis: None,
                serial: SERIAL_COUNTER.next_serial(),
                time: smithay::backend::input::InputTime::from_millis(9),
            },
        );
        pen.down(
            f.clarion_state(),
            &tool::DownEvent {
                serial: SERIAL_COUNTER.next_serial(),
                time: smithay::backend::input::InputTime::from_millis(10),
            },
        );
        let grab = overview_grab(
            &mut f,
            window,
            AnyStartData::TabletTool(tool::GrabStartData {
                focus: None,
                trigger: tool::GrabTrigger::Tip,
                location: START.into(),
            }),
        );
        pen.set_grab(
            f.clarion_state(),
            grab,
            smithay::backend::input::InputTime::from_millis(10),
            SERIAL_COUNTER.next_serial(),
            Focus::Clear,
        );
        assert!(pen.is_grabbed());
        pen.frame(f.clarion_state(), smithay::backend::input::InputTime::from_millis(10));
        if hold {
            dispatch_for(&mut f, Duration::from_millis(550));
            let output = f.clarion_output(1);
            assert!(f
                .clarion()
                .layout
                .interactive_move_is_moving_above_output(&output));
        }
        pen.motion(
            f.clarion_state(),
            None,
            &tool::MotionEvent {
                location: target.into(),
                serial: SERIAL_COUNTER.next_serial(),
                time: smithay::backend::input::InputTime::from_millis(20),
            },
        );
        if proximity_out {
            pen.proximity_out(
                f.clarion_state(),
                &tool::ProximityOutEvent {
                    serial: SERIAL_COUNTER.next_serial(),
                    time: smithay::backend::input::InputTime::from_millis(21),
                },
            );
        } else {
            pen.up(
                f.clarion_state(),
                &tool::UpEvent {
                    serial: SERIAL_COUNTER.next_serial(),
                    time: smithay::backend::input::InputTime::from_millis(21),
                },
            );
        }
        assert!(!pen.is_grabbed());
        assert_eq!(f.clarion().layout.is_overview_open(), stays_open);
        pen.frame(f.clarion_state(), smithay::backend::input::InputTime::from_millis(21));
        assert_eq!(f.clarion().layout.is_overview_open(), stays_open);
    }
}

fn dispatch_for(f: &mut Fixture, duration: Duration) {
    let deadline = std::time::Instant::now() + duration;
    while std::time::Instant::now() < deadline {
        f.state
            .server
            .event_loop
            .dispatch(Duration::from_millis(20), &mut f.state.server.state)
            .unwrap();
    }
}

#[test]
fn overview_tablet_release_after_completed_mode_switch_does_not_commit_old_tap() {
    let (mut f, _, _, window, _) = setup();
    let tablet_seat = f.clarion().seat.tablet_seat();
    let tablet = tablet_seat.add_tablet(&TabletDescriptor {
        name: "Overview mode switch tablet".into(),
        usb_id: None,
        syspath: None,
    });
    let pen = tablet_seat.add_tool(&TabletToolDescriptor {
        tool_type: TabletToolType::Pen,
        hardware_serial: 2,
        hardware_id_wacom: 0,
        capabilities: TabletToolCapabilities::empty(),
    });
    pen.proximity_in(
        f.clarion_state(),
        None,
        tablet,
        &tool::ProximityInEvent {
            location: START.into(),
            axis: None,
            serial: SERIAL_COUNTER.next_serial(),
            time: smithay::backend::input::InputTime::from_millis(9),
        },
    );
    pen.down(
        f.clarion_state(),
        &tool::DownEvent {
            serial: SERIAL_COUNTER.next_serial(),
            time: smithay::backend::input::InputTime::from_millis(10),
        },
    );
    let grab = overview_grab(
        &mut f,
        window,
        AnyStartData::TabletTool(tool::GrabStartData {
            focus: None,
            trigger: tool::GrabTrigger::Tip,
            location: START.into(),
        }),
    );
    let workspace = f.clarion().layout.active_workspace().unwrap().id();
    pen.set_grab(
        f.clarion_state(),
        grab,
        smithay::backend::input::InputTime::from_millis(10),
        SERIAL_COUNTER.next_serial(),
        Focus::Clear,
    );
    assert!(pen.is_grabbed());
    // Use the direct layout switch to exercise the grab's own guard rather
    // than State's proactive cancellation. Release follows the completed fade.
    f.clarion().layout.toggle_expo();
    f.clarion_complete_animations();
    assert!(pen.is_grabbed());
    pen.up(
        f.clarion_state(),
        &tool::UpEvent {
            serial: SERIAL_COUNTER.next_serial(),
            time: smithay::backend::input::InputTime::from_millis(21),
        },
    );
    assert!(!pen.is_grabbed());
    assert!(f.clarion().layout.is_overview_open());
    assert_eq!(
        f.clarion().layout.overview_mode(),
        crate::layout::OverviewMode::Expo
    );
    assert_eq!(
        f.clarion().layout.active_workspace().unwrap().id(),
        workspace
    );
    pen.frame(f.clarion_state(), smithay::backend::input::InputTime::from_millis(21));
    assert!(f.clarion().layout.is_overview_open());
}

#[test]
fn overview_release_recognizes_pending_swipe_instead_of_tapping() {
    for target in [(280., 220.), (220., 280.)] {
        for frame_before_release in [false, true] {
            let (mut f, _, _, window, touch) = setup();
            grab_overview(&mut f, window, &touch);
            motion(&mut f, &touch, 1, target);
            // Another contact ending must neither consume nor end this gesture.
            motion(&mut f, &touch, 2, (500., 500.));
            up(&mut f, &touch, 2);
            assert!(touch.is_grabbed());
            assert!(f.clarion().layout.is_overview_open());
            if frame_before_release {
                touch.frame(f.clarion_state());
            }
            up(&mut f, &touch, 1);
            assert!(!touch.is_grabbed());
            assert!(f.clarion().layout.is_overview_open());
            touch.frame(f.clarion_state());
            assert!(f.clarion().layout.is_overview_open());
        }
    }
}

#[test]
fn overview_cancel_or_replaced_grab_does_not_activate_a_tap() {
    for cancel in [false, true] {
        let (mut f, _, _, window, touch) = setup();
        grab_overview(&mut f, window, &touch);
        motion(&mut f, &touch, 1, (221., 221.));
        if cancel {
            touch.cancel(f.clarion_state());
        } else {
            touch.unset_grab(f.clarion_state());
        }
        assert!(!touch.is_grabbed());
        assert!(f.clarion().layout.is_overview_open());
        touch.frame(f.clarion_state());
        assert!(f.clarion().layout.is_overview_open());
    }
}

#[test]
fn overview_normal_tap_still_activates_window() {
    let (mut f, _, _, window, touch) = setup();
    grab_overview(&mut f, window, &touch);
    motion(&mut f, &touch, 1, (221., 221.));
    up(&mut f, &touch, 1);
    assert!(!touch.is_grabbed());
    assert!(!f.clarion().layout.is_overview_open());
}

#[test]
fn stationary_overview_touch_starts_drag_without_another_motion_event() {
    let (mut f, _, _, window, touch) = setup();
    grab_overview(&mut f, window, &touch);
    let output = f.clarion_output(1);
    // A touch device need not produce motion events for a stationary finger.
    touch.frame(f.clarion_state());
    let deadline = std::time::Instant::now() + Duration::from_millis(650);
    while std::time::Instant::now() < deadline
        && !f
            .clarion()
            .layout
            .interactive_move_is_moving_above_output(&output)
    {
        f.state
            .server
            .event_loop
            .dispatch(Duration::from_millis(20), &mut f.state.server.state)
            .unwrap();
    }
    assert!(f
        .clarion()
        .layout
        .interactive_move_is_moving_above_output(&output));
    touch.cancel(f.clarion_state());
    assert!(f.clarion().layout.is_overview_open());
    assert!(!f
        .clarion()
        .layout
        .interactive_move_is_moving_above_output(&output));
}

#[test]
fn pending_overview_long_press_cannot_activate_after_cancel_or_context_change() {
    for transition in [
        "cancel", "replace", "swipe", "mode", "modal", "output", "lock", "destroy",
    ] {
        let (mut f, client, surface, window, touch) = setup();
        let before = position(&mut f);
        grab_overview(&mut f, window.clone(), &touch);
        let output = f.clarion_output(1);
        touch.frame(f.clarion_state());
        match transition {
            "cancel" => touch.cancel(f.clarion_state()),
            "replace" => {
                let replacement =
                    MoveGrab::new(f.clarion_state(), start_data(), window, false, None).unwrap();
                touch.set_grab(f.clarion_state(), replacement, SERIAL_COUNTER.next_serial());
            }
            "swipe" => {
                motion(&mut f, &touch, 1, (120., 220.));
                touch.frame(f.clarion_state());
            }
            "mode" => f.clarion().layout.toggle_expo(),
            "modal" => {
                assert!(f.clarion().exit_confirm_dialog.show());
            }
            "output" => f.clarion().remove_output(&output),
            "lock" => f.clarion_state().cancel_grabs_for_lock(),
            "destroy" => {
                let window = f.client(client).window(&surface);
                window.xdg_toplevel.destroy();
                window.xdg_surface.destroy();
                window.surface.destroy();
                f.double_roundtrip(client);
            }
            _ => unreachable!(),
        }
        let selected = f.clarion().layout.stage_selected_for_output(&output);
        dispatch_for(&mut f, Duration::from_millis(550));
        assert!(f.clarion().layout.is_overview_open(), "{transition}");
        assert!(!f
            .clarion()
            .layout
            .interactive_move_is_moving_above_output(&output));
        if !matches!(transition, "output" | "destroy") {
            assert_eq!(position(&mut f), before, "{transition}");
            assert_eq!(
                f.clarion().layout.stage_selected_for_output(&output),
                selected
            );
        }
        if transition == "replace" {
            assert!(touch
                .with_grab(|_, grab| grab.as_any().is::<MoveGrab>())
                .unwrap());
        } else if transition != "swipe" {
            assert!(!touch.is_grabbed(), "{transition}");
        }
        touch.cancel(f.clarion_state());
    }
}

#[test]
fn overview_move_release_keeps_the_last_position() {
    let run = |frame_before_release| {
        let (mut f, _, _, window, touch) = setup();
        grab_overview(&mut f, window, &touch);
        // A held contact followed by movement starts a window drag.
        touch.motion(
            f.clarion_state(),
            None,
            &MotionEvent {
                slot: Some(1).into(),
                location: (260., 260.).into(),
                time: smithay::backend::input::InputTime::from_millis(520),
            },
        );
        touch.frame(f.clarion_state());
        touch.motion(
            f.clarion_state(),
            None,
            &MotionEvent {
                slot: Some(1).into(),
                location: (320., 300.).into(),
                time: smithay::backend::input::InputTime::from_millis(530),
            },
        );
        if frame_before_release {
            touch.frame(f.clarion_state());
        }
        touch.up(
            f.clarion_state(),
            &UpEvent {
                slot: Some(1).into(),
                serial: SERIAL_COUNTER.next_serial(),
                time: smithay::backend::input::InputTime::from_millis(531),
            },
        );
        assert!(!touch.is_grabbed());
        f.clarion_complete_animations();
        position(&mut f)
    };
    assert_eq!(run(false), run(true));
}

#[test]
fn move_release_applies_pending_motion_only_for_the_starting_contact() {
    let run = |frame_before_release| {
        let (mut f, _client, _surface, window, touch) = setup();
        let initial = position(&mut f);
        let grab = MoveGrab::new(f.clarion_state(), start_data(), window, false, None).unwrap();
        touch.set_grab(f.clarion_state(), grab, SERIAL_COUNTER.next_serial());

        motion(&mut f, &touch, 1, (260., 280.));
        motion(&mut f, &touch, 2, (500., 500.));
        up(&mut f, &touch, 2);
        assert!(touch.is_grabbed());
        assert!(!touch
            .with_grab(|_, grab| grab.as_any().downcast_ref::<MoveGrab>().unwrap().is_move())
            .unwrap());
        assert_eq!(position(&mut f), initial);

        if frame_before_release {
            touch.frame(f.clarion_state());
        }
        up(&mut f, &touch, 1);
        assert!(!touch.is_grabbed());
        f.clarion_complete_animations();
        let final_position = position(&mut f);
        assert_ne!(final_position, initial);
        touch.frame(f.clarion_state());
        assert_eq!(position(&mut f), final_position);
        final_position
    };

    // A motion immediately followed by up must have the same effect as motion
    // completed by a frame. Keep normal layout placement and animation semantics.
    assert_eq!(run(false), run(true));
}

#[test]
fn move_grab_overview_release_commits_but_cancel_and_replacement_do_not_tap() {
    for action in [0, 1, 2] {
        let (mut f, _, _, window, touch) = setup();
        f.clarion().layout.open_overview();
        f.clarion_complete_animations();
        let grab = MoveGrab::new(f.clarion_state(), start_data(), window, false, None).unwrap();
        touch.set_grab(f.clarion_state(), grab, SERIAL_COUNTER.next_serial());
        motion(&mut f, &touch, 1, (221., 221.));
        match action {
            0 => up(&mut f, &touch, 1),
            1 => touch.cancel(f.clarion_state()),
            _ => touch.unset_grab(f.clarion_state()),
        }
        assert!(!touch.is_grabbed());
        assert_eq!(f.clarion().layout.is_overview_open(), action != 0);
        touch.frame(f.clarion_state());
        assert_eq!(f.clarion().layout.is_overview_open(), action != 0);
    }
}

#[test]
fn stage_move_grab_cancel_or_replacement_restores_position_over_valid_card() {
    for cancel in [false, true] {
        let (mut f, _, _, window, touch) = setup();
        let before = position(&mut f);
        f.clarion().layout.open_overview();
        f.clarion_complete_animations();
        let grab = MoveGrab::new(f.clarion_state(), start_data(), window, false, None).unwrap();
        touch.set_grab(f.clarion_state(), grab, SERIAL_COUNTER.next_serial());
        motion(&mut f, &touch, 1, (640., 380.));
        touch.frame(f.clarion_state());
        let output = f.clarion().layout.outputs().next().unwrap().clone();
        assert!(f
            .clarion()
            .layout
            .interactive_move_is_moving_above_output(&output));
        if cancel {
            touch.cancel(f.clarion_state());
        } else {
            touch.unset_grab(f.clarion_state());
        }
        f.clarion_complete_animations();
        assert!(!touch.is_grabbed());
        assert!(!f
            .clarion()
            .layout
            .interactive_move_is_moving_above_output(&output));
        assert!(f.clarion().layout.is_stage_open());
        assert_eq!(position(&mut f), before);
    }
}

#[test]
fn stage_move_grab_release_without_final_motion_respects_modal_priority() {
    for modal in ["exit", "mru", "screenshot"] {
        for moving in [false, true] {
            let (mut f, _, _, window, touch) = setup();
            if modal == "screenshot" {
                f.init_gles_renderer();
            }
            let before = position(&mut f);
            f.clarion().layout.open_overview();
            f.clarion_complete_animations();
            let grab = MoveGrab::new(f.clarion_state(), start_data(), window, false, None).unwrap();
            touch.set_grab(f.clarion_state(), grab, SERIAL_COUNTER.next_serial());
            if moving {
                motion(&mut f, &touch, 1, (640., 380.));
                touch.frame(f.clarion_state());
            }
            match modal {
                "exit" => assert!(f.clarion().exit_confirm_dialog.show()),
                "mru" => {
                    let wmru = crate::ui::mru::WindowMru::new(f.clarion());
                    let clock = f.clarion().clock.clone();
                    let output = f.clarion_output(1);
                    f.clarion().window_mru_ui.open(clock, wmru, output);
                    assert!(f.clarion().window_mru_ui.is_open());
                }
                _ => {
                    f.clarion_state().open_screenshot_ui(false, None);
                    assert!(f.clarion().screenshot_ui.is_open());
                }
            }
            // No final motion/frame occurs after the modal grabs priority.
            up(&mut f, &touch, 1);
            assert!(!touch.is_grabbed());
            assert!(
                f.clarion().layout.is_stage_open(),
                "modal={modal}, moving={moving}"
            );
            f.clarion_complete_animations();
            assert_eq!(position(&mut f), before, "modal={modal}, moving={moving}");
        }
    }
}

#[test]
fn resize_release_applies_pending_motion_only_for_the_starting_contact() {
    let (mut f, client, surface, window, touch) = setup();
    assert!(f
        .clarion()
        .layout
        .interactive_resize_begin(window.clone(), ResizeEdge::BOTTOM_RIGHT));
    touch.set_grab(
        f.clarion_state(),
        ResizeGrab::new(start_data(), window),
        SERIAL_COUNTER.next_serial(),
    );

    let before = f
        .clarion()
        .layout
        .windows()
        .next()
        .unwrap()
        .1
        .requested_size();

    motion(&mut f, &touch, 1, (260., 280.));
    motion(&mut f, &touch, 2, (500., 500.));
    up(&mut f, &touch, 2);
    assert!(touch.is_grabbed());
    assert_eq!(
        f.clarion()
            .layout
            .windows()
            .next()
            .unwrap()
            .1
            .requested_size(),
        before
    );

    up(&mut f, &touch, 1);
    assert!(!touch.is_grabbed());
    f.double_roundtrip(client);
    assert_eq!(
        f.client(client)
            .window(&surface)
            .recent_configures()
            .last()
            .unwrap()
            .size,
        (140, 160)
    );
    touch.frame(f.clarion_state());
    f.double_roundtrip(client);
    assert_eq!(
        f.client(client)
            .window(&surface)
            .recent_configures()
            .count(),
        0
    );
}

#[test]
fn resize_release_does_not_reapply_an_already_consumed_motion() {
    let (mut f, client, surface, window, touch) = setup();
    assert!(f
        .clarion()
        .layout
        .interactive_resize_begin(window.clone(), ResizeEdge::BOTTOM_RIGHT));
    touch.set_grab(
        f.clarion_state(),
        ResizeGrab::new(start_data(), window),
        SERIAL_COUNTER.next_serial(),
    );
    motion(&mut f, &touch, 1, (260., 280.));
    touch.frame(f.clarion_state());
    f.double_roundtrip(client);
    assert_eq!(
        f.client(client)
            .window(&surface)
            .recent_configures()
            .last()
            .unwrap()
            .size,
        (140, 160)
    );

    let window = f.client(client).window(&surface);
    window.set_size(140, 160);
    window.ack_last_and_commit();
    f.double_roundtrip(client);
    f.client(client)
        .window(&surface)
        .recent_configures()
        .for_each(drop);

    // A separate size action can arrive after the last motion frame. Releasing
    // the contact must not replay that older location over the new request.
    f.clarion()
        .layout
        .set_column_width(SizeChange::SetFixed(180));
    f.double_roundtrip(client);
    assert_eq!(
        f.client(client)
            .window(&surface)
            .recent_configures()
            .last()
            .unwrap()
            .size,
        (180, 160)
    );

    // Nested Winit emits a frame after every touch event. Frames from another
    // contact, or with no motion at all, must not replay the primary contact.
    motion(&mut f, &touch, 2, (500., 500.));
    touch.frame(f.clarion_state());
    up(&mut f, &touch, 2);
    touch.frame(f.clarion_state());
    touch.frame(f.clarion_state());
    assert!(touch.is_grabbed());
    assert_eq!(
        f.clarion()
            .layout
            .windows()
            .next()
            .unwrap()
            .1
            .requested_size(),
        Some((180, 160).into())
    );
    f.double_roundtrip(client);
    assert_eq!(
        f.client(client)
            .window(&surface)
            .recent_configures()
            .count(),
        0
    );

    up(&mut f, &touch, 1);
    touch.frame(f.clarion_state());
    f.double_roundtrip(client);
    assert!(f
        .client(client)
        .window(&surface)
        .recent_configures()
        .all(|c| c.size == (180, 160)));
}
