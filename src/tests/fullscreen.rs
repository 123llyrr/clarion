use client::ClientId;
use insta::assert_snapshot;
use smithay::utils::Point;
use wayland_client::protocol::wl_surface::WlSurface;

use super::*;
use crate::layout::LayoutElement as _;

// Sets up a fixture with two outputs and 100×100 window.
fn set_up() -> (Fixture, ClientId, WlSurface) {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    f.add_output(2, (1280, 720));

    let id = f.add_client();
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);

    let window = f.client(id).window(&surface);
    window.attach_new_buffer();
    window.set_size(100, 100);
    window.ack_last_and_commit();
    f.double_roundtrip(id);

    (f, id, surface)
}

#[test]
fn windowed_fullscreen() {
    let (mut f, id, surface) = set_up();

    let _ = f.client(id).window(&surface).recent_configures();

    let realand = f.realand();
    let mapped = realand.layout.windows().next().unwrap().1;
    let window_id = mapped.window.clone();

    // Enable windowed fullscreen.
    realand.layout.toggle_windowed_fullscreen(&window_id);
    f.double_roundtrip(id);

    // Should request fullscreen state with the tiled size.
    let window = f.client(id).window(&surface);
    assert_snapshot!(
        window.format_recent_configures(),
        @"size: 936 × 1048, bounds: 1888 × 1048, states: [Activated, Fullscreen]"
    );

    let mapped = f.realand().layout.windows().next().unwrap().1;
    // Not committed yet.
    assert!(!mapped.is_windowed_fullscreen());

    // Commit in response.
    let window = f.client(id).window(&surface);
    window.ack_last_and_commit();
    f.roundtrip(id);

    let mapped = f.realand().layout.windows().next().unwrap().1;
    // Now it is committed.
    assert!(mapped.is_windowed_fullscreen());

    // Disable windowed fullscreen.
    f.realand().layout.toggle_windowed_fullscreen(&window_id);
    f.double_roundtrip(id);

    // Should request without fullscreen state with the tiled size.
    let window = f.client(id).window(&surface);
    assert_snapshot!(
        window.format_recent_configures(),
        @"size: 936 × 1048, bounds: 1888 × 1048, states: [Activated]"
    );

    let mapped = f.realand().layout.windows().next().unwrap().1;
    // Not committed yet.
    assert!(mapped.is_windowed_fullscreen());

    // Commit in response.
    let window = f.client(id).window(&surface);
    window.ack_last_and_commit();
    f.roundtrip(id);

    let mapped = f.realand().layout.windows().next().unwrap().1;
    // Now it is committed.
    assert!(!mapped.is_windowed_fullscreen());
}

#[test]
fn windowed_fullscreen_chain() {
    let (mut f, id, surface) = set_up();

    let _ = f.client(id).window(&surface).recent_configures();

    let mapped = f.realand().layout.windows().next().unwrap().1;
    let window_id = mapped.window.clone();

    f.realand().layout.toggle_windowed_fullscreen(&window_id);
    f.roundtrip(id);
    f.realand().layout.toggle_windowed_fullscreen(&window_id);
    f.roundtrip(id);
    f.realand().layout.toggle_windowed_fullscreen(&window_id);
    f.roundtrip(id);
    f.realand().layout.toggle_windowed_fullscreen(&window_id);
    f.double_roundtrip(id);

    // Should be four configures matching the four requests.
    let window = f.client(id).window(&surface);
    assert_snapshot!(
        window.format_recent_configures(),
        @r"
    size: 936 × 1048, bounds: 1888 × 1048, states: [Activated, Fullscreen]
    size: 936 × 1048, bounds: 1888 × 1048, states: [Activated]
    size: 936 × 1048, bounds: 1888 × 1048, states: [Activated, Fullscreen]
    size: 936 × 1048, bounds: 1888 × 1048, states: [Activated]
    "
    );

    let window = f.client(id).window(&surface);
    let serials = Vec::from_iter(
        window.configures_received[window.configures_received.len() - 4..]
            .iter()
            .map(|(s, _c)| *s),
    );

    let get_state = |f: &mut Fixture| {
        let mapped = f.realand().layout.windows().next().unwrap().1;
        format!(
            "fs {}, wfs {}",
            mapped.sizing_mode().is_fullscreen(),
            mapped.is_windowed_fullscreen()
        )
    };

    let mut states = vec![get_state(&mut f)];
    for serial in serials {
        let window = f.client(id).window(&surface);
        window.xdg_surface.ack_configure(serial);
        window.commit();
        f.roundtrip(id);
        states.push(get_state(&mut f));
    }

    // We expect fs to always be false (because each Fullscreen state request corresponded to a
    // windowed fullscreen), and wfs to toggle on and off.
    assert_snapshot!(
        states.join("\n"),
        @r"
    fs false, wfs false
    fs false, wfs true
    fs false, wfs false
    fs false, wfs true
    fs false, wfs false
    "
    );
}

#[test]
fn unfullscreen_before_fullscreen_ack_doesnt_prevent_view_offset_save_restore() {
    let (mut f, id, _surface) = set_up();

    let window2 = f.client(id).create_window();
    let surface2 = window2.surface.clone();
    window2.commit();
    f.roundtrip(id);

    let window2 = f.client(id).window(&surface2);
    window2.attach_new_buffer();
    window2.set_size(200, 200);
    window2.ack_last_and_commit();
    f.double_roundtrip(id);

    let _ = f.client(id).window(&surface2).recent_configures();

    let realand = f.realand();
    let mapped2 = realand.layout.windows().last().unwrap().1;
    let window2_id = mapped2.window.clone();

    // The view position is at the first window.
    assert_snapshot!(realand.layout.active_workspace().unwrap().scrolling().view_pos(), @"-16");

    // Fullscreen window2 and send the configure so we can clear pending.
    realand.layout.set_fullscreen(&window2_id, true);
    f.double_roundtrip(id);

    // Before acking, unfullscreen the column, clearing the pending fullscreen flag.
    f.realand().layout.set_fullscreen(&window2_id, false);

    // Now, window2 receives the fullscreen configure and resizes in response.
    let window2 = f.client(id).window(&surface2);
    assert_snapshot!(
        window2.format_recent_configures(),
        @"size: 1920 × 1080, bounds: 1888 × 1048, states: [Activated, Fullscreen]"
    );
    let (_, configure) = window2.configures_received.last().unwrap();
    window2.set_size(configure.size.0 as u16, configure.size.1 as u16);
    window2.ack_last_and_commit();
    f.double_roundtrip(id);
    f.realand_complete_animations();

    // The view position is now at the fullscreen-sized window2.
    assert_snapshot!(f.realand().layout.active_workspace().unwrap().scrolling().view_pos(), @"116");

    // Now, window2 receives the unfullscreen configure and resizes in response.
    let window2 = f.client(id).window(&surface2);
    assert_snapshot!(
        window2.format_recent_configures(),
        @"size: 936 × 1048, bounds: 1888 × 1048, states: [Activated]"
    );
    window2.set_size(200, 200);
    window2.ack_last_and_commit();
    f.roundtrip(id);
    f.realand_complete_animations();

    // The view position should restore to the first window.
    assert_snapshot!(f.realand().layout.active_workspace().unwrap().scrolling().view_pos(), @"-16");
}

#[test]
fn interactive_move_unfullscreen_to_scrolling_restores_size() {
    let (mut f, id, surface) = set_up();

    let _ = f.client(id).window(&surface).recent_configures();

    let realand = f.realand();
    let mapped = realand.layout.windows().next().unwrap().1;
    let window = mapped.window.clone();
    realand.layout.set_fullscreen(&window, true);
    f.double_roundtrip(id);

    // This should request a fullscreen size.
    assert_snapshot!(
        f.client(id).window(&surface).format_recent_configures(),
        @"size: 1920 × 1080, bounds: 1888 × 1048, states: [Activated, Fullscreen]"
    );

    // Start an interactive move which causes an unfullscreen.
    let output = f.realand_output(1);
    let realand = f.realand();
    let mapped = realand.layout.windows().next().unwrap().1;
    let window = mapped.window.clone();
    realand
        .layout
        .interactive_move_begin(window.clone(), &output, Point::default());
    realand.layout.interactive_move_update(
        &window,
        Point::from((1000., 0.)),
        output,
        Point::default(),
    );
    f.double_roundtrip(id);

    // This should request the tiled size.
    assert_snapshot!(
        f.client(id).window(&surface).format_recent_configures(),
        @"size: 936 × 1048, bounds: 1920 × 1080, states: [Activated]"
    );
}

#[test]
fn interactive_move_unmaximize_to_scrolling_restores_size() {
    let (mut f, id, surface) = set_up();

    let _ = f.client(id).window(&surface).recent_configures();

    let realand = f.realand();
    let mapped = realand.layout.windows().next().unwrap().1;
    let window = mapped.window.clone();
    realand.layout.set_maximized(&window, true);
    f.double_roundtrip(id);

    // This should request a maximized size.
    assert_snapshot!(
        f.client(id).window(&surface).format_recent_configures(),
        @"size: 1920 × 1080, bounds: 1888 × 1048, states: [Activated, Maximized]"
    );

    // Start an interactive move which causes an unmaximize.
    let output = f.realand_output(1);
    let realand = f.realand();
    let mapped = realand.layout.windows().next().unwrap().1;
    let window = mapped.window.clone();
    realand
        .layout
        .interactive_move_begin(window.clone(), &output, Point::default());
    realand.layout.interactive_move_update(
        &window,
        Point::from((1000., 0.)),
        output,
        Point::default(),
    );
    f.double_roundtrip(id);

    // This should request the tiled size.
    assert_snapshot!(
        f.client(id).window(&surface).format_recent_configures(),
        @"size: 936 × 1048, bounds: 1920 × 1080, states: [Activated]"
    );
}

#[test]
fn is_fullscreen_reflects_fullscreen_state() {
    let (mut f, id, surface) = set_up();
    let mapped = f.realand().layout.windows().next().unwrap().1;
    let window_id = mapped.window.clone();

    // Normal window: is_fullscreen is false
    assert!(!mapped.is_fullscreen());

    // Windowed fullscreen: is_fullscreen remains false (only windowed fullscreen)
    f.realand().layout.toggle_windowed_fullscreen(&window_id);
    f.double_roundtrip(id);
    let window = f.client(id).window(&surface);
    window.ack_last_and_commit();
    f.roundtrip(id);
    let mapped = f.realand().layout.windows().next().unwrap().1;
    assert!(mapped.is_windowed_fullscreen());
    assert!(!mapped.is_fullscreen());

    // Real fullscreen: is_fullscreen is true
    f.realand().layout.set_fullscreen(&window_id, true);
    f.double_roundtrip(id);
    let window = f.client(id).window(&surface);
    window.ack_last_and_commit();
    f.roundtrip(id);
    let mapped = f.realand().layout.windows().next().unwrap().1;
    assert!(mapped.is_fullscreen());
    assert!(!mapped.is_windowed_fullscreen());

    // Unfullscreen: is_fullscreen returns to false
    f.realand().layout.set_fullscreen(&window_id, false);
    f.double_roundtrip(id);
    let window = f.client(id).window(&surface);
    window.ack_last_and_commit();
    f.roundtrip(id);
    let mapped = f.realand().layout.windows().next().unwrap().1;
    assert!(!mapped.is_fullscreen());
}

#[test]
fn fullscreen_window_becomes_scanout_candidate_despite_geometry_corner_radius() {
    let (mut f, id, surface) = set_up();
    f.init_gles_renderer();
    let mapped = f.realand().layout.windows().next().unwrap().1;
    let window_id = mapped.window.clone();

    // Configure window rule with geometry corner radius
    f.realand()
        .config
        .borrow_mut()
        .window_rules
        .push(realand_config::WindowRule {
            geometry_corner_radius: Some(realand_config::CornerRadius::from(12.0)),
            ..Default::default()
        });
    f.realand().recompute_window_rules();

    let check_kind = |f: &mut Fixture| -> smithay::backend::renderer::element::Kind {
        let mut kind = None;
        let state = f.realand_state();
        let mapped = state.realand.layout.windows().next().unwrap().1;
        state.backend.with_renderer(|renderer| {
            if let crate::render_helpers::renderer::AnyRenderer::Gles(renderer) = renderer {
                let ctx = crate::render_helpers::RenderCtx {
                    renderer,
                    target: crate::render_helpers::RenderTarget::Output,
                    xray: None,
                };
                mapped.render_normal(
                    ctx,
                    smithay::utils::Point::from((0., 0.)),
                    smithay::utils::Scale::from(1.),
                    1.,
                    &mut |elem| {
                        if let crate::layout::LayoutElementRenderElement::Wayland(w) = elem {
                            use smithay::backend::renderer::element::Element;
                            kind = Some(w.kind());
                        }
                    },
                );
            }
        });
        kind.expect("expected wayland render element")
    };

    use smithay::backend::renderer::element::Kind;

    // While normal: geometry_corner_radius forces Kind::Unspecified
    assert_eq!(check_kind(&mut f), Kind::Unspecified);

    // When fullscreen: covers entire screen, so it promotes to Kind::ScanoutCandidate
    f.realand().layout.set_fullscreen(&window_id, true);
    f.double_roundtrip(id);
    let window = f.client(id).window(&surface);
    window.ack_last_and_commit();
    f.roundtrip(id);
    assert_eq!(check_kind(&mut f), Kind::ScanoutCandidate);

    // When unfullscreen: reverts to Kind::Unspecified
    f.realand().layout.set_fullscreen(&window_id, false);
    f.double_roundtrip(id);
    let window = f.client(id).window(&surface);
    window.ack_last_and_commit();
    f.roundtrip(id);
    assert_eq!(check_kind(&mut f), Kind::Unspecified);
}

#[test]
fn fullscreen_window_becomes_scanout_candidate_despite_clip_to_geometry() {
    let (mut f, id, surface) = set_up();
    f.init_gles_renderer();
    let mapped = f.realand().layout.windows().next().unwrap().1;
    let window_id = mapped.window.clone();

    // Configure window rule with clip_to_geometry
    f.realand()
        .config
        .borrow_mut()
        .window_rules
        .push(realand_config::WindowRule {
            clip_to_geometry: Some(true),
            ..Default::default()
        });
    f.realand().recompute_window_rules();

    let check_kind = |f: &mut Fixture| -> smithay::backend::renderer::element::Kind {
        let mut kind = None;
        let state = f.realand_state();
        let mapped = state.realand.layout.windows().next().unwrap().1;
        state.backend.with_renderer(|renderer| {
            if let crate::render_helpers::renderer::AnyRenderer::Gles(renderer) = renderer {
                let ctx = crate::render_helpers::RenderCtx {
                    renderer,
                    target: crate::render_helpers::RenderTarget::Output,
                    xray: None,
                };
                mapped.render_normal(
                    ctx,
                    smithay::utils::Point::from((0., 0.)),
                    smithay::utils::Scale::from(1.),
                    1.,
                    &mut |elem| {
                        if let crate::layout::LayoutElementRenderElement::Wayland(w) = elem {
                            use smithay::backend::renderer::element::Element;
                            kind = Some(w.kind());
                        }
                    },
                );
            }
        });
        kind.expect("expected wayland render element")
    };

    use smithay::backend::renderer::element::Kind;

    // While normal: clip_to_geometry forces Kind::Unspecified
    assert_eq!(check_kind(&mut f), Kind::Unspecified);

    // When fullscreen: covers entire screen, so it promotes to Kind::ScanoutCandidate
    f.realand().layout.set_fullscreen(&window_id, true);
    f.double_roundtrip(id);
    let window = f.client(id).window(&surface);
    window.ack_last_and_commit();
    f.roundtrip(id);
    assert_eq!(check_kind(&mut f), Kind::ScanoutCandidate);

    // When unfullscreen: reverts to Kind::Unspecified
    f.realand().layout.set_fullscreen(&window_id, false);
    f.double_roundtrip(id);
    let window = f.client(id).window(&surface);
    window.ack_last_and_commit();
    f.roundtrip(id);
    assert_eq!(check_kind(&mut f), Kind::Unspecified);
}
