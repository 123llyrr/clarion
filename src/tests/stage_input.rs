use std::time::Duration;

use realand_config::{Config, Workspace, WorkspaceName};
use smithay::desktop::Window;
use smithay::input::touch::{DownEvent, GrabStartData, MotionEvent, TouchHandle, UpEvent};
use smithay::output::{Output, Scale};
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer;
use smithay::utils::{Logical, Point, SERIAL_COUNTER};

use super::stage_render::StageTestBar;
use super::Fixture;
use crate::compositor::State;
use crate::input::touch_overview_grab::TouchOverviewGrab;
use crate::input::AnyStartData;
use crate::layout::workspace::WorkspaceId;

fn setup(scale: f64) -> (Fixture, Output, Window) {
    let config = Config {
        workspaces: ["first", "second", "third"]
            .into_iter()
            .map(|name| Workspace {
                name: WorkspaceName(name.into()),
                open_on_output: None,
                layout: None,
            })
            .collect(),
        ..Default::default()
    };
    let mut f = Fixture::with_config(config);
    f.add_output(1, (1280, 800));
    let output = f.realand_output(1);
    output.change_current_state(None, None, Some(Scale::Fractional(scale)), None);
    f.realand().output_resized(&output);
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
    let window = f.realand().layout.active_window().unwrap().window.clone();
    f.realand_complete_animations();
    f.realand().layout.open_overview();
    f.realand_complete_animations();
    (f, output, window)
}

fn hit_point(
    f: &mut Fixture,
    output: &Output,
    workspace: WorkspaceId,
    window: Option<&Window>,
) -> Point<f64, Logical> {
    let size = f
        .realand()
        .layout
        .monitor_for_output(output)
        .unwrap()
        .view_size();
    for y in (48..(size.h as usize).saturating_sub(100)).step_by(8) {
        for x in (16..(size.w as usize).saturating_sub(16)).step_by(8) {
            let point = Point::from((x as f64, y as f64));
            let layout = &f.realand().layout;
            if !layout
                .workspace_under(false, output, point)
                .is_some_and(|ws| ws.id() == workspace)
            {
                continue;
            }
            let hit = layout
                .window_under(output, point)
                .map(|(mapped, _)| &mapped.window);
            if hit == window {
                return point;
            }
        }
    }
    panic!("no matching visible Stage hit for workspace {workspace:?}");
}

fn grab(
    f: &mut Fixture,
    output: &Output,
    point: Point<f64, Logical>,
    window: Option<Window>,
) -> TouchHandle<State> {
    let workspace = f
        .realand()
        .layout
        .workspace_under(false, output, point)
        .map(|ws| ws.id());
    let touch = f.realand().seat.add_touch();
    touch.down(
        f.realand_state(),
        None,
        &DownEvent {
            slot: Some(1).into(),
            location: point,
            serial: SERIAL_COUNTER.next_serial(),
            time: 10,
        },
    );
    let grab = TouchOverviewGrab::new(
        AnyStartData::Touch(GrabStartData {
            focus: None,
            slot: Some(1).into(),
            location: point,
        }),
        Duration::from_millis(10),
        output.clone(),
        point,
        workspace,
        true,
        window,
        f.realand().layout.overview_mode(),
    );
    touch.set_grab(f.realand_state(), grab, SERIAL_COUNTER.next_serial());
    touch
}

fn release(f: &mut Fixture, touch: &TouchHandle<State>) {
    touch.up(
        f.realand_state(),
        &UpEvent {
            slot: Some(1).into(),
            serial: SERIAL_COUNTER.next_serial(),
            time: 21,
        },
    );
    assert!(!touch.is_grabbed());
}

#[test]
fn stage_touch_swipe_previews_without_committing_and_cancel_restores_selection() {
    for scale in [1., 1.25, 1.5] {
        for cancel in [false, true] {
            let (mut f, output, _) = setup(scale);
            let initial = f.realand().layout.active_workspace().unwrap().id();
            let point = hit_point(&mut f, &output, initial, None);
            let touch = grab(&mut f, &output, point, None);
            touch.motion(
                f.realand_state(),
                None,
                &MotionEvent {
                    slot: Some(1).into(),
                    location: point - Point::from((96., 0.)),
                    time: 20,
                },
            );
            touch.frame(f.realand_state());
            assert_ne!(
                f.realand().layout.stage_selected_for_output(&output),
                Some(initial)
            );
            assert_eq!(f.realand().layout.active_workspace().unwrap().id(), initial);
            if cancel {
                touch.cancel(f.realand_state());
                assert_eq!(
                    f.realand().layout.stage_selected_for_output(&output),
                    Some(initial)
                );
            } else {
                release(&mut f, &touch);
                assert_ne!(
                    f.realand().layout.stage_selected_for_output(&output),
                    Some(initial)
                );
            }
            assert!(f.realand().layout.is_stage_open());
            assert_eq!(f.realand().layout.active_workspace().unwrap().id(), initial);
        }
    }
}

#[test]
fn stage_window_and_empty_card_taps_enter_the_hit_workspace() {
    for scale in [1., 1.25, 1.5] {
        for tap_window in [false, true] {
            let (mut f, output, window) = setup(scale);
            if !tap_window {
                assert!(f.realand().layout.stage_navigate(Some(&output), 1));
                f.realand_complete_animations();
            }
            let selected = f
                .realand()
                .layout
                .stage_selected_for_output(&output)
                .unwrap();
            let point = hit_point(&mut f, &output, selected, tap_window.then_some(&window));
            let touch = grab(&mut f, &output, point, tap_window.then_some(window.clone()));
            release(&mut f, &touch);
            assert!(!f.realand().layout.is_overview_open());
            f.realand_complete_animations();
            assert_eq!(
                f.realand().layout.active_workspace().unwrap().id(),
                selected
            );
            if tap_window {
                assert_eq!(f.realand().layout.active_window().unwrap().window, window);
            }
        }
    }
}

#[test]
fn stage_pending_tap_is_cancelled_when_output_disappears_or_modal_ui_opens() {
    for remove in [false, true] {
        let (mut f, output, window) = setup(1.25);
        let initial = f.realand().layout.active_workspace().unwrap().id();
        let point = hit_point(&mut f, &output, initial, Some(&window));
        let touch = grab(&mut f, &output, point, Some(window));
        if remove {
            f.realand().remove_output(&output);
        } else {
            assert!(f.realand().exit_confirm_dialog.show());
        }
        release(&mut f, &touch);
        assert!(
            f.realand().layout.is_overview_open(),
            "rejected touch must not activate a tap"
        );
    }
}

#[test]
fn stage_long_press_starts_a_window_drag_and_touch_cancel_restores_its_workspace() {
    for scale in [1., 1.25, 1.5] {
        let (mut f, output, window) = setup(scale);
        let initial = f.realand().layout.active_workspace().unwrap().id();
        let point = hit_point(&mut f, &output, initial, Some(&window));
        let touch = grab(&mut f, &output, point, Some(window.clone()));
        touch.motion(
            f.realand_state(),
            None,
            &MotionEvent {
                slot: Some(1).into(),
                location: point + Point::from((100., 20.)),
                time: 520,
            },
        );
        touch.frame(f.realand_state());
        assert!(f
            .realand()
            .layout
            .interactive_move_is_moving_above_output(&output));
        touch.cancel(f.realand_state());
        assert!(!touch.is_grabbed());
        assert!(!f
            .realand()
            .layout
            .interactive_move_is_moving_above_output(&output));
        f.realand_complete_animations();
        let layout = &f.realand().layout;
        assert!(layout.is_stage_open());
        assert_eq!(
            layout
                .workspaces()
                .find(|(_, _, ws)| ws.has_window(&window))
                .unwrap()
                .2
                .id(),
            initial,
        );
    }
}

#[test]
fn stage_rail_hits_follow_the_work_area_when_native_bar_changes_edge_and_height() {
    for scale in [1., 1.25, 1.5, 2.] {
        let (mut f, output, _) = setup(scale);
        let initial = f.realand().layout.active_workspace().unwrap().id();
        let second = f
            .realand()
            .layout
            .monitor_for_output(&output)
            .unwrap()
            .workspaces_with_render_geo()
            .nth(1)
            .unwrap()
            .0
            .id();
        let logical_height = f
            .realand()
            .layout
            .monitor_for_output(&output)
            .unwrap()
            .view_size()
            .h;
        let mut bar = StageTestBar::new(&mut f);
        for (bottom, height, layer) in [
            (false, 40, Layer::Top),
            (true, 40, Layer::Top),
            (true, 128, Layer::Top),
            (true, logical_height as u32, Layer::Overlay),
            (false, logical_height as u32, Layer::Overlay),
            (false, 128, Layer::Top),
            (false, 40, Layer::Top),
            (true, 40, Layer::Top),
        ] {
            bar.configure(&mut f, bottom, height, layer);
            let monitor = f.realand().layout.monitor_for_output(&output).unwrap();
            let area = monitor.working_area();
            assert_eq!(area.loc.y, if bottom { 0. } else { 40. });
            assert_eq!(area.size.h, logical_height.round() - 40.);
            let count = monitor.workspaces_with_render_geo().count().min(7);
            let available = area.size.w - 48.;
            let fit = (available / (88. + 148. * count as f64 + 144.)).min(1.);
            let total = (88. + 148. * count as f64 + 144.) * fit;
            let second_x = area.loc.x + (area.size.w - total) / 2. + (44. + 148. + 74.) * fit;
            let rail_top = area.loc.y + area.size.h - 76. + 8.;
            for offset in [3., 26., 49.] {
                f.realand().layout.stage_select(&output, initial);
                f.realand_complete_animations();
                let point = Point::from((second_x, rail_top + offset));
                assert!(
                    f.realand().contents_under(point).layer.is_none(),
                    "the whole workspace rail must accept compositor input above the bar"
                );
                assert!(
                    f.realand().layout.stage_select_at(&output, point),
                    "visible rail hit must match work area: bottom={bottom}, height={height}, scale={scale}, point={point:?}"
                );
                assert_eq!(
                    f.realand().layout.stage_selected_for_output(&output),
                    Some(second)
                );
                assert_eq!(f.realand().layout.active_workspace().unwrap().id(), initial);
            }
        }
    }
}
