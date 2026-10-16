use std::fmt::Write as _;
use std::time::Duration;

use clarion_config::animations::{Curve, EasingParams, Kind};
use clarion_config::Config;
use clarion_ipc::SizeChange;
use insta::assert_snapshot;
use smithay::utils::{Point, Size};
use wayland_client::protocol::wl_surface::WlSurface;

use super::client::ClientId;
use super::*;
use crate::compositor::Clarion;

fn format_tiles(clarion: &Clarion) -> String {
    let mut buf = String::new();
    let ws = clarion.layout.active_workspace().unwrap();
    let mut tiles: Vec<_> = ws.tiles_with_render_positions().collect();

    // We sort by id since that gives us a consistent order (from first opened to last), but we
    // don't print the id since it's nondeterministic (the id is a global counter across all
    // running tests in the same binary).
    tiles.sort_by_key(|(tile, _, _)| tile.window().id().get());
    for (tile, pos, _visible) in tiles {
        let Size { w, h, .. } = tile.animated_tile_size();
        let Point { x, y, .. } = pos;
        writeln!(&mut buf, "{w:>3.0} × {h:>3.0} at x:{x:>3.0} y:{y:>3.0}").unwrap();
    }
    buf
}

fn create_window(f: &mut Fixture, id: ClientId, w: u16, h: u16) -> WlSurface {
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);

    let window = f.client(id).window(&surface);
    window.attach_new_buffer();
    window.set_size(w, h);
    window.ack_last_and_commit();
    f.roundtrip(id);

    surface
}

fn set_time(clarion: &mut Clarion, time: Duration) {
    // This is a bit involved because we're dealing with an AdjustableClock that maintains its own
    // internal current_time.

    // First, reset current_time to zero by matching unadjusted time to it (at rate 0.0), then
    // setting unadjusted time to zero at rate 1.0 (causing current_time to also go to zero).
    let now = clarion.clock.now();
    clarion.clock.set_unadjusted(now);
    let _ = clarion.clock.now();
    clarion.clock.set_unadjusted(Duration::ZERO);
    clarion.clock.set_rate(1.0);
    let _ = clarion.clock.now();

    // Now, set the desired time at rate 1.0.
    clarion.clock.set_unadjusted(time);
    let _ = clarion.clock.now();

    // Freeze the clock so that clear() inside the clarion loop callback followed by some get()
    // doesn't replace it with the monotonic time.
    clarion.clock.set_rate(0.0);
}

// Sets up a fixture with linear animations, a renderer, and an output.
fn set_up() -> Fixture {
    const LINEAR: Kind = Kind::Easing(EasingParams {
        duration_ms: 1000,
        curve: Curve::Linear,
    });

    let mut config = Config::default();
    config.layout.gaps = 0.0;
    config.layout.auto_stack_new_windows = false;
    config.animations.window_resize.anim.kind = LINEAR;
    config.animations.window_movement.0.kind = LINEAR;

    let mut f = Fixture::with_config(config);
    f.init_gles_renderer();
    f.add_output(1, (1920, 1080));

    f
}

fn set_up_two_in_column() -> (Fixture, ClientId, WlSurface, WlSurface) {
    let mut f = set_up();

    let id = f.add_client();

    let surface1 = create_window(&mut f, id, 100, 100);
    let surface2 = create_window(&mut f, id, 200, 200);
    f.double_roundtrip(id);

    let _ = f.client(id).window(&surface1).recent_configures();
    let _ = f.client(id).window(&surface2).recent_configures();

    // Consume into one column.
    f.clarion().layout.focus_left();
    f.clarion().layout.consume_into_column();
    f.double_roundtrip(id);

    // Commit for the column consume.
    let window = f.client(id).window(&surface1);
    window.ack_last_and_commit();

    let window = f.client(id).window(&surface2);
    window.ack_last_and_commit();

    f.double_roundtrip(id);

    set_time(f.clarion(), Duration::ZERO);
    f.clarion_complete_animations();

    (f, id, surface1, surface2)
}

#[test]
fn egl_height_resize_animates_next_y() {
    let (mut f, id, surface1, surface2) = set_up_two_in_column();

    // Issue a resize.
    f.clarion()
        .layout
        .set_window_height(None, SizeChange::AdjustFixed(-50));
    f.double_roundtrip(id);

    // The top window shrinks in response, the bottom remains as is.
    let window = f.client(id).window(&surface1);
    window.set_size(100, 50);
    window.ack_last_and_commit();
    let window = f.client(id).window(&surface2);
    window.ack_last_and_commit();

    // This starts the resize animation for the top window and the Y move for the bottom.
    f.roundtrip(id);

    // No time had passed yet, so we're at the initial state.
    assert_snapshot!(format_tiles(f.clarion()), @r"
    100 × 100 at x:  0 y:  0
    200 × 200 at x:  0 y:100
    ");

    // Advance the time halfway.
    set_time(f.clarion(), Duration::from_millis(500));
    f.clarion().advance_animations();

    // Top window is half-resized at 75 px tall, bottom window is at y=75 matching it.
    assert_snapshot!(format_tiles(f.clarion()), @r"
    100 ×  75 at x:  0 y:  0
    200 × 200 at x:  0 y: 75
    ");

    // Advance the time to completion.
    set_time(f.clarion(), Duration::from_millis(1000));
    f.clarion().advance_animations();

    // Final state at 50 px.
    assert_snapshot!(format_tiles(f.clarion()), @r"
    100 ×  50 at x:  0 y:  0
    200 × 200 at x:  0 y: 50
    ");
}

#[test]
fn egl_single_pixel_resize_snapshot_keeps_premultiplied_color_and_old_viewport() {
    use std::sync::{Arc, Mutex};

    use smithay::backend::allocator::Fourcc;
    use smithay::backend::renderer::{ExportMem as _, Texture as _};
    use smithay::utils::Rectangle;
    use smithay::wayland::compositor::{add_post_commit_hook, with_states};
    use smithay::wayland::viewporter::ViewportCachedState;

    use crate::compositor::State;
    use crate::layout::LayoutElement as _;
    use crate::render_helpers::{RenderCtx, RenderTarget};

    let mut f = set_up();
    let id = f.add_client();
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.double_roundtrip(id);
    let window = f.client(id).window(&surface);
    // Protocol colors already have premultiplied alpha. Multiplying by alpha
    // during import again would turn these channels into roughly 32/16/8.
    let unit = u32::MAX / 255;
    let buffer = window.spbm.create_u32_rgba_buffer(
        64 * unit,
        32 * unit,
        16 * unit,
        128 * unit,
        &window.qh,
        (),
    );
    window.surface.attach(Some(&buffer), 0, 0);
    window.viewport.set_source(0.25, 0.125, 0.5, 0.75);
    window.set_size(80, 60);
    window.xdg_surface.set_window_geometry(5, 7, 70, 45);
    window.ack_last_and_commit();
    f.double_roundtrip(id);
    let server = f
        .clarion()
        .layout
        .windows()
        .next()
        .unwrap()
        .1
        .toplevel()
        .wl_surface()
        .clone();

    let captured = Arc::new(Mutex::new(None));
    let capture = captured.clone();
    // This is the same boundary used by the compositor: new protocol state is
    // current, but on_commit_buffer_handler has not replaced the renderer state.
    add_post_commit_hook::<State, _>(&server, move |state, _, surface| {
        with_states(surface, |states| {
            assert_eq!(
                states
                    .cached_state
                    .get::<ViewportCachedState>()
                    .current()
                    .dst,
                Some((140, 100).into()),
            );
        });
        let (mapped, _) = state
            .clarion
            .layout
            .find_window_and_output_mut(surface)
            .unwrap();
        state.backend.with_primary_renderer(|renderer| {
            mapped.store_animation_snapshot(renderer, 1.0.into());
            let snapshot = mapped
                .take_animation_snapshot()
                .expect("a mapped single-pixel surface must produce an owned snapshot");
            let (texture, bounds) = snapshot
                .texture(
                    RenderCtx {
                        renderer,
                        target: RenderTarget::Output,
                        xray: None,
                    },
                    1.0.into(),
                )
                .unwrap();
            let mapping = renderer
                .copy_texture(
                    texture,
                    Rectangle::from_size(texture.size()),
                    Fourcc::Abgr8888,
                )
                .unwrap();
            let pixels = renderer.map_texture(&mapping).unwrap().to_vec();
            *capture.lock().unwrap() = Some((snapshot.size, *bounds, pixels));
        });
    });

    let window = f.client(id).window(&surface);
    let replacement = window
        .spbm
        .create_u32_rgba_buffer(0, u32::MAX, 0, u32::MAX, &window.qh, ());
    window.surface.attach(Some(&replacement), 0, 0);
    window.viewport.set_source(0., 0., 1., 1.);
    window.set_size(140, 100);
    window.xdg_surface.set_window_geometry(11, 13, 110, 75);
    window.commit();
    f.double_roundtrip(id);

    let (size, bounds, pixels) = captured.lock().unwrap().take().unwrap();
    assert_eq!(size, (70., 45.).into());
    assert_eq!(bounds, Rectangle::new((-5, -7).into(), (80, 60).into()));
    assert_eq!(pixels.len(), 80 * 60 * 4);
    for pixel in pixels.as_chunks::<4>().0 {
        for (actual, expected) in pixel.iter().copied().zip([64_u8, 32, 16, 128]) {
            assert!(
                actual.abs_diff(expected) <= 1,
                "wrong premultiplied pixel: {pixel:?}"
            );
        }
    }
}

#[test]
fn egl_clientside_height_change_doesnt_animate() {
    let (mut f, id, surface1, _surface2) = set_up_two_in_column();

    // The initial state.
    assert_snapshot!(format_tiles(f.clarion()), @r"
    100 × 100 at x:  0 y:  0
    200 × 200 at x:  0 y:100
    ");

    // The top window shrinks by itself, without a clarion-issued resize.
    let window = f.client(id).window(&surface1);
    window.set_size(100, 50);
    window.commit();

    // This does not start any animations.
    f.roundtrip(id);

    // No time had passed yet, but we are at the final state right away.
    assert_snapshot!(format_tiles(f.clarion()), @r"
    100 ×  50 at x:  0 y:  0
    200 × 200 at x:  0 y: 50
    ");
}

#[test]
fn egl_window_spread_renders_offscreen_clients_and_search_empty_state() {
    use crate::render_helpers::{RenderCtx, RenderTarget};
    use smithay::backend::allocator::Fourcc;
    use smithay::utils::Transform;
    let mut f = set_up();
    let client = f.add_client();
    for idx in 0..9 {
        let surface = create_window(&mut f, client, 700, 600);
        let window = f.client(client).window(&surface);
        window.set_title(&format!("Spread window {idx}"));
        let color = [u32::MAX / 4, u32::MAX / 2, u32::MAX];
        let buffer = window.spbm.create_u32_rgba_buffer(
            color[idx % 3],
            color[(idx + 1) % 3],
            color[(idx + 2) % 3],
            u32::MAX,
            &window.qh,
            (),
        );
        window.surface.attach(Some(&buffer), 0, 0);
        window.commit();
        f.roundtrip(client);
    }
    f.clarion_complete_animations();
    f.clarion().layout.toggle_window_spread();
    f.clarion_complete_animations();
    let render = |f: &mut Fixture| {
        let state = f.clarion_state();
        let clarion = &mut state.clarion;
        clarion.update_render_elements(None);
        let output = clarion.layout.outputs().next().unwrap().clone();
        state
            .backend
            .headless()
            .with_primary_renderer(|renderer| {
                let mut elements = Vec::new();
                clarion
                    .layout
                    .monitor_for_output(&output)
                    .unwrap()
                    .render_workspaces(
                        RenderCtx {
                            renderer,
                            target: RenderTarget::Output,
                            xray: None,
                        },
                        true,
                        &mut |element| elements.push(element),
                    );
                crate::render_helpers::render_to_vec(
                    renderer,
                    (1920, 1080).into(),
                    1.0.into(),
                    Transform::Normal,
                    Fourcc::Abgr8888,
                    elements.iter().rev(),
                )
                .unwrap()
            })
            .unwrap()
    };
    let pixels = render(&mut f);
    let colored = |pixels: &[u8]| {
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[0].abs_diff(p[1]) > 40 || p[1].abs_diff(p[2]) > 40)
            .count()
    };
    assert!(
        colored(&pixels) > 500_000,
        "all nine client thumbnails must contribute visible color"
    );
    if let Some(path) = std::env::var_os("CLARION_SPREAD_TEST_PNG") {
        let file = std::fs::File::create(path).unwrap();
        let mut encoder = png::Encoder::new(file, 1920, 1080);
        encoder.set_color(png::ColorType::Rgba);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&pixels)
            .unwrap();
    }
    for c in "no-such-window".chars() {
        f.clarion()
            .layout
            .spread_key(smithay::input::keyboard::Keysym::from_char(c));
    }
    assert_eq!(
        colored(&render(&mut f)),
        0,
        "unmatched windows must disappear from the rendered grid"
    );
}

#[test]
fn egl_canvas_pan_culls_and_restores_window_pixels() {
    use crate::render_helpers::{RenderCtx, RenderTarget};
    use smithay::backend::allocator::Fourcc;
    use smithay::utils::Transform;
    let mut f = set_up();
    let client = f.add_client();
    let surface = create_window(&mut f, client, 400, 300);
    let window = f.client(client).window(&surface);
    let buffer = window
        .spbm
        .create_u32_rgba_buffer(u32::MAX, 0, 0, u32::MAX, &window.qh, ());
    window.surface.attach(Some(&buffer), 0, 0);
    window.commit();
    f.roundtrip(client);
    f.clarion_complete_animations();
    f.clarion()
        .layout
        .active_workspace_mut()
        .unwrap()
        .toggle_canvas();
    f.double_roundtrip(client);
    f.clarion_complete_animations();
    let render = |f: &mut Fixture| {
        let state = f.clarion_state();
        let clarion = &mut state.clarion;
        clarion.update_render_elements(None);
        let output = clarion.layout.outputs().next().unwrap().clone();
        state
            .backend
            .headless()
            .with_primary_renderer(|renderer| {
                let mut elements = Vec::new();
                clarion
                    .layout
                    .monitor_for_output(&output)
                    .unwrap()
                    .render_workspaces(
                        RenderCtx {
                            renderer,
                            target: RenderTarget::Output,
                            xray: None,
                        },
                        true,
                        &mut |element| elements.push(element),
                    );
                crate::render_helpers::render_to_vec(
                    renderer,
                    (1920, 1080).into(),
                    1.0.into(),
                    Transform::Normal,
                    Fourcc::Abgr8888,
                    elements.iter().rev(),
                )
                .unwrap()
            })
            .unwrap()
    };

    let red = |pixels: &[u8]| {
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[0] > 200 && p[1] < 30 && p[2] < 30)
            .count()
    };
    let before = red(&render(&mut f));
    assert!(before > 1000);
    f.clarion()
        .layout
        .active_workspace_mut()
        .unwrap()
        .pan_canvas((10000., 10000.).into(), false);
    assert_eq!(red(&render(&mut f)), 0);
    f.clarion()
        .layout
        .active_workspace_mut()
        .unwrap()
        .reset_canvas();
    assert_eq!(red(&render(&mut f)), before);
}
