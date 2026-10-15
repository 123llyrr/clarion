//! Real Wayland metadata requests must refresh an already-entered Spread query.

use std::collections::HashSet;

use realand_config::Config;
use smithay::desktop::Window;
use smithay::input::keyboard::Keysym;
use smithay::output::Output;
use smithay::utils::Point;
use smithay::wayland::seat::WaylandFocus;
use wayland_client::protocol::wl_surface::WlSurface;

use super::client::ClientId;
use super::Fixture;
use crate::layout::LayoutElement;
use crate::output_schedule::RedrawState;

fn setup() -> (Fixture, ClientId, Vec<WlSurface>, Vec<Window>) {
    let mut config = Config::default();
    config.layout.auto_stack_new_windows = false;
    let mut fixture = Fixture::with_config(config);
    fixture.add_output(1, (1280, 800));
    fixture.add_output(2, (1280, 800));
    fixture.realand_focus_output(1);
    let client = fixture.add_client();
    let mut surfaces = Vec::new();
    let mut windows = Vec::new();
    for (app, title) in [("org.one", "Needle one"), ("org.two", "Other")] {
        let window = fixture.client(client).create_window();
        let surface = window.surface.clone();
        window.xdg_toplevel.set_app_id(app.into());
        window.set_title(title);
        window.commit();
        fixture.double_roundtrip(client);
        let window = fixture.client(client).window(&surface);
        window.attach_new_buffer();
        window.set_size(100, 100);
        window.ack_last_and_commit();
        fixture.double_roundtrip(client);
        windows.push(
            fixture
                .realand()
                .layout
                .active_window()
                .unwrap()
                .window
                .clone(),
        );
        surfaces.push(surface);
    }
    fixture.realand().layout.activate_window(&windows[0]);
    fixture.realand_complete_animations();
    fixture.realand().layout.toggle_window_spread();
    fixture.realand_complete_animations();
    (fixture, client, surfaces, windows)
}

fn previews(fixture: &mut Fixture, output: &Output) -> HashSet<Window> {
    // Assert production grid hit testing rather than exposing a search cache
    // solely for tests. The mapped client buffers are 100 x 100 pixels.
    let mut windows = HashSet::new();
    for y in (80..800).step_by(16) {
        for x in (16..1280).step_by(16) {
            if let Some((window, _)) = fixture
                .realand()
                .layout
                .window_under(output, Point::from((x as f64, y as f64)))
            {
                windows.insert(window.window.clone());
            }
        }
    }
    windows
}

fn selected(fixture: &mut Fixture, output: &Output) -> Option<Window> {
    fixture
        .realand()
        .layout
        .monitor_for_output(output)
        .unwrap()
        .spread_selected()
}

#[test]
fn title_and_app_id_changes_refresh_existing_spread_query_without_rule_changes_or_search_keys() {
    let (mut fixture, client, surfaces, windows) = setup();
    let output = fixture.realand_output(1);
    let rules: Vec<_> = windows
        .iter()
        .map(|window| {
            fixture
                .realand()
                .layout
                .find_window_and_output(window.wl_surface().unwrap().as_ref())
                .unwrap()
                .0
                .rules()
                .clone()
        })
        .collect();
    for ch in "needle".chars() {
        assert!(fixture.realand().layout.spread_key(Keysym::from_char(ch)));
    }
    assert_eq!(
        previews(&mut fixture, &output),
        HashSet::from([windows[0].clone()])
    );
    assert_eq!(selected(&mut fixture, &output), Some(windows[0].clone()));

    // No more keys are sent to the compositor. Only live xdg_toplevel metadata
    // requests change search membership and preserve a surviving selection.
    fixture
        .client(client)
        .window(&surfaces[1])
        .set_title("Needle two");
    fixture.double_roundtrip(client);
    assert_eq!(
        previews(&mut fixture, &output),
        windows.iter().cloned().collect()
    );
    assert_eq!(selected(&mut fixture, &output), Some(windows[0].clone()));

    fixture
        .client(client)
        .window(&surfaces[0])
        .set_title("Other");
    fixture.double_roundtrip(client);
    assert_eq!(
        previews(&mut fixture, &output),
        HashSet::from([windows[1].clone()])
    );
    assert_eq!(selected(&mut fixture, &output), Some(windows[1].clone()));

    fixture
        .client(client)
        .window(&surfaces[0])
        .xdg_toplevel
        .set_app_id("needle.app".into());
    fixture.double_roundtrip(client);
    assert_eq!(
        previews(&mut fixture, &output),
        windows.iter().cloned().collect()
    );
    assert_eq!(selected(&mut fixture, &output), Some(windows[1].clone()));
    fixture
        .client(client)
        .window(&surfaces[0])
        .xdg_toplevel
        .set_app_id("org.one".into());
    fixture.double_roundtrip(client);
    assert_eq!(
        previews(&mut fixture, &output),
        HashSet::from([windows[1].clone()])
    );

    for (window, expected) in windows.iter().zip(&rules) {
        assert_eq!(
            fixture
                .realand()
                .layout
                .find_window_and_output(window.wl_surface().unwrap().as_ref())
                .unwrap()
                .0
                .rules(),
            expected
        );
    }
}

#[test]
fn metadata_with_unchanged_search_membership_still_requests_title_repaint() {
    let (mut fixture, client, surfaces, windows) = setup();
    let output = fixture.realand_output(1);
    // The empty query takes the allocation-free metadata path.
    for state in fixture.realand().output_state.values_mut() {
        state.schedule.test_set_redraw(RedrawState::Idle);
    }
    let before = fixture
        .realand()
        .frame_timings()
        .into_iter()
        .find(|timing| timing.output == output.name())
        .unwrap()
        .redraws;
    fixture
        .client(client)
        .window(&surfaces[0])
        .set_title("NEEDLE ONE");
    fixture.double_roundtrip(client);
    let after = fixture
        .realand()
        .frame_timings()
        .into_iter()
        .find(|timing| timing.output == output.name())
        .unwrap()
        .redraws;
    assert!(
        after > before,
        "a visible original-case title change needs repaint"
    );
    assert_eq!(
        previews(&mut fixture, &output),
        windows.iter().cloned().collect()
    );
    assert_eq!(selected(&mut fixture, &output), Some(windows[0].clone()));
}
