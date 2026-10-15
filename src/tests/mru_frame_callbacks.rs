use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use smithay::backend::renderer::element::{
    Id, RenderElementPresentationState, RenderElementState, RenderElementStates,
};
use smithay::desktop::utils::{send_frames_surface_tree, surface_primary_scanout_output};
use smithay::output::Output;
use smithay::reexports::wayland_server::{
    protocol::wl_surface::WlSurface as ServerSurface, Resource as _,
};
use smithay::wayland::compositor::with_states;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::Proxy as _;

use super::client::{ClientId, SyncData};
use super::Fixture;
use crate::layout::LayoutElement as _;
use crate::render_helpers::offscreen::OffscreenData;
use crate::render_helpers::RenderTarget;
use crate::ui::mru::{MruCloseRequest, WindowMru};
use crate::utils::get_monotonic_time;

fn setup(home_output: u8) -> (Fixture, ClientId, WlSurface, ServerSurface) {
    let mut f = Fixture::new();
    f.add_output(1, (1280, 720));
    f.add_output(2, (1280, 720));
    f.realand_focus_output(home_output);
    let client_id = f.add_client();
    let window = f.client(client_id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.double_roundtrip(client_id);
    let window = f.client(client_id).window(&surface);
    window.attach_new_buffer();
    window.set_size(100, 100);
    window.ack_last_and_commit();
    f.double_roundtrip(client_id);
    f.realand_complete_animations();

    let server = f
        .realand()
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

    // Prime the invisible-surface throttle. A subsequent callback must be caused
    // by presentation, rather than the first unconditional fallback callback.
    let primed = request_frame(&mut f, client_id, &surface);
    let home = f.realand_output(home_output);
    f.realand()
        .update_primary_scanout_output(&home, &rendered(Id::from_wayland_resource(&server)));
    f.realand().send_frame_callbacks(&home);
    f.double_roundtrip(client_id);
    assert!(primed.done.load(Ordering::Relaxed));
    f.realand()
        .update_primary_scanout_output(&home, &RenderElementStates::default());
    // Keep this a visibility test even if a busy parallel test run pauses the
    // thread longer than the compositor's 995 ms invisible-surface throttle.
    suppress_fallback(&server, &home);
    (f, client_id, surface, server)
}

fn suppress_fallback(surface: &ServerSurface, output: &Output) {
    // No callback is pending here. Seed Smithay's throttle through its public API.
    send_frames_surface_tree(
        surface,
        output,
        get_monotonic_time() + Duration::from_secs(3600),
        None,
        |_, _| Some(output.clone()),
    );
}

fn request_frame(f: &mut Fixture, client: ClientId, surface: &WlSurface) -> Arc<SyncData> {
    let data = Arc::new(SyncData::default());
    surface.frame(&f.client(client).qh, data.clone());
    surface.commit();
    f.double_roundtrip(client);
    data
}

fn rendered(id: Id) -> RenderElementStates {
    RenderElementStates {
        states: [(
            id,
            RenderElementState {
                visible_area: 100,
                presentation_state: RenderElementPresentationState::Rendering { reason: None },
                needs_capture: false,
            },
        )]
        .into_iter()
        .collect(),
    }
}

fn open_mru(f: &mut Fixture, output: &Output) {
    let realand = f.realand();
    realand.config.borrow_mut().recent_windows.open_delay_ms = 0;
    let mru = WindowMru::new(realand);
    realand
        .window_mru_ui
        .open(realand.clock.clone(), mru, output.clone());
}

#[test]
fn mru_local_animations_only_schedule_the_owner_but_backdrop_fades_reach_all_outputs() {
    use realand_config::MruScope;

    for idle in [false, true] {
        let (mut f, _, _, _) = setup(2);
        // The only window belongs to the other monitor, so changing scope hides and reveals it.
        f.realand_focus_output(1);
        let output = f.realand_output(1);
        let other = f.realand_output(2);
        if idle {
            f.realand().last_user_activity = std::time::Instant::now() - Duration::from_secs(31);
        }
        assert!(!f.realand().animations_need_redraw(&output));
        assert!(!f.realand().animations_need_redraw(&other));

        {
            let realand = f.realand();
            realand.config.borrow_mut().recent_windows.open_delay_ms = 100;
            let mru = WindowMru::new(realand);
            realand
                .window_mru_ui
                .open(realand.clock.clone(), mru, output.clone());
        }
        assert!(f.realand().animations_need_redraw(&output));
        assert!(f.realand().animations_need_redraw(&other));

        let now = f.realand().clock.now_unadjusted();
        f.realand()
            .clock
            .set_unadjusted(now + Duration::from_secs(1));
        f.realand_complete_animations();
        f.realand().window_mru_ui.set_scope(MruScope::Output);
        f.realand().window_mru_ui.set_scope(MruScope::All);
        assert!(f.realand().animations_need_redraw(&output));
        assert!(!f.realand().animations_need_redraw(&other));

        f.realand_complete_animations();
        f.realand().window_mru_ui.close(MruCloseRequest::Cancel);
        // The full-screen backdrops on all monitors must finish fading as well.
        assert!(!f.realand().window_mru_ui.is_open());
        assert!(f.realand().animations_need_redraw(&output));
        assert!(f.realand().animations_need_redraw(&other));

        open_mru(&mut f, &output);
        assert!(f.realand().animations_need_redraw(&output));
        assert!(f.realand().animations_need_redraw(&other));
        f.realand_complete_animations();
        f.realand().window_mru_ui.close(MruCloseRequest::Cancel);

        // Opening on another monitor discards the old fade and transfers the local animation.
        open_mru(&mut f, &other);
        f.realand().window_mru_ui.set_scope(MruScope::Output);
        f.realand().window_mru_ui.set_scope(MruScope::All);
        assert!(!f.realand().animations_need_redraw(&output));
        assert!(f.realand().animations_need_redraw(&other));
        f.realand_complete_animations();
        f.realand().window_mru_ui.close(MruCloseRequest::Cancel);
        assert!(f.realand().animations_need_redraw(&output));
        assert!(f.realand().animations_need_redraw(&other));
        f.realand_complete_animations();
        assert!(!f.realand().animations_need_redraw(&output));
        assert!(!f.realand().animations_need_redraw(&other));
    }
}

fn record_fade(f: &mut Fixture, output: &Output, surface: &ServerSurface) -> Id {
    let id = Id::new();
    f.realand().window_mru_ui.record_offscreen_render(
        output,
        RenderTarget::Output,
        Some(OffscreenData {
            id: id.clone(),
            states: rendered(Id::from_wayland_resource(surface)),
        }),
    );
    id
}

fn primary(surface: &ServerSurface) -> Option<Output> {
    with_states(surface, |states| {
        surface_primary_scanout_output(surface, states)
    })
}

#[test]
fn removing_mru_output_discards_open_and_fading_overlay_resources() {
    use crate::compositor::RedrawState;
    use realand_config::MruScope;

    for (closing, remove_other_first) in
        [(false, false), (false, true), (true, false), (true, true)]
    {
        let (mut f, _, _, surface) = setup(1);
        let output = f.realand_output(1);
        let other = f.realand_output(2);
        open_mru(&mut f, &output);
        f.realand().window_mru_ui.set_scope(MruScope::Workspace);
        if closing {
            f.realand().window_mru_ui.close(MruCloseRequest::Cancel);
        }
        record_fade(&mut f, &output, &surface);
        if remove_other_first {
            f.realand().remove_output(&other);
        } else {
            f.realand()
                .output_state
                .get_mut(&other)
                .unwrap()
                .schedule
                .test_set_redraw(RedrawState::Idle);
        }
        assert!(f.realand().window_mru_ui.offscreen_data(&output).is_some());
        assert_eq!(f.realand().window_mru_ui.is_open(), !closing);

        f.realand().remove_output(&output);
        let ui = &f.realand().window_mru_ui;
        assert!(!ui.is_open());
        assert!(!ui.are_animations_ongoing());
        assert!(ui.offscreen_data(&output).is_none());
        assert!(ui.output().is_none());
        assert_eq!(ui.scope(), MruScope::Workspace);
        if !remove_other_first {
            assert!(
                matches!(
                    f.realand().output_state[&other].schedule.redraw_state(),
                    RedrawState::Queued
                ),
                "the remaining output must clear its backdrop"
            );
        }
    }
}

#[test]
fn faded_mru_preview_receives_real_frame_callback_on_same_or_other_output() {
    for home in [1, 2] {
        let (mut f, client, surface, server) = setup(home);
        let output = f.realand_output(1);
        // The window is not in the final frame directly; only its preview is.
        f.realand().layout.switch_workspace_down();
        f.realand_complete_animations();
        // Drain activation/configure changes, whose one-shot callback allowance
        // intentionally bypasses visibility and is unrelated to this regression.
        f.double_roundtrip(client);
        suppress_fallback(&server, &output);
        let callback = request_frame(&mut f, client, &surface);
        assert!(!callback.done.load(Ordering::Relaxed));
        open_mru(&mut f, &output);
        f.realand().window_mru_ui.close(MruCloseRequest::Cancel);
        let fade = record_fade(&mut f, &output, &server);
        f.realand()
            .update_primary_scanout_output(&output, &rendered(fade));
        assert_eq!(primary(&server), Some(output.clone()));
        f.realand().send_frame_callbacks(&output);
        f.double_roundtrip(client);
        assert!(callback.done.load(Ordering::Relaxed), "home output {home}");
    }
}

#[test]
fn occluded_mru_offscreen_does_not_make_its_sources_visible() {
    let (mut f, client, surface, server) = setup(1);
    let output = f.realand_output(1);
    let callback = request_frame(&mut f, client, &surface);
    open_mru(&mut f, &output);
    record_fade(&mut f, &output, &server);
    f.realand()
        .update_primary_scanout_output(&output, &RenderElementStates::default());
    f.realand().send_frame_callbacks(&output);
    f.double_roundtrip(client);
    assert_eq!(primary(&server), None);
    assert!(!callback.done.load(Ordering::Relaxed));
}

#[test]
fn fade_visibility_is_isolated_from_captures_and_other_outputs() {
    let (mut f, _, _, server) = setup(2);
    let output = f.realand_output(1);
    let other = f.realand_output(2);
    open_mru(&mut f, &output);
    let fade = record_fade(&mut f, &output, &server);
    for target in [RenderTarget::Screencast, RenderTarget::ScreenCapture] {
        f.realand()
            .window_mru_ui
            .record_offscreen_render(&output, target, None);
    }
    f.realand()
        .window_mru_ui
        .record_offscreen_render(&other, RenderTarget::Output, None);
    f.realand()
        .update_primary_scanout_output(&output, &rendered(fade.clone()));
    assert_eq!(primary(&server), Some(output.clone()));
    f.realand()
        .update_primary_scanout_output(&other, &rendered(fade.clone()));
    assert_eq!(primary(&server), Some(output.clone()));

    // A fresh display render must clear the old fade mapping, even if rendering
    // later fails and an old outer element id remains in the supplied states.
    f.realand()
        .window_mru_ui
        .record_offscreen_render(&output, RenderTarget::Output, None);
    f.realand()
        .update_primary_scanout_output(&output, &rendered(fade));
    assert_eq!(primary(&server), None);
}

#[test]
fn direct_preview_wins_over_unrelated_window_offscreen_and_keeps_home_pacing() {
    let (mut f, _, _, server) = setup(2);
    let preview_output = f.realand_output(1);
    let home_output = f.realand_output(2);
    let surface_id = Id::from_wayland_resource(&server);
    let tile_id = Id::new();
    let mapped = f.realand().layout.windows().next().unwrap().1;
    mapped.set_offscreen_data(Some(OffscreenData {
        id: tile_id.clone(),
        states: rendered(surface_id.clone()),
    }));
    f.realand()
        .update_primary_scanout_output(&preview_output, &rendered(surface_id.clone()));
    assert_eq!(primary(&server), Some(preview_output.clone()));
    f.realand()
        .update_primary_scanout_output(&home_output, &rendered(tile_id));
    assert_eq!(primary(&server), Some(home_output.clone()));
    f.realand()
        .update_primary_scanout_output(&preview_output, &rendered(surface_id));
    assert_eq!(primary(&server), Some(home_output));
}
