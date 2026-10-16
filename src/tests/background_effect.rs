//! Real protocol commits and production renderer readback for translucent layers.
//! These fixtures own their Wayland display and headless outputs; they never
//! connect to the user's compositor or acquire a DRM display master.

use std::sync::{Arc, Mutex};

use clarion_config::{BlockOutFrom, Config, WindowRule};
use smithay::backend::allocator::Fourcc;
use smithay::output::Scale as OutputScale;
use smithay::reexports::wayland_protocols::ext::background_effect::v1::client::{
    ext_background_effect_manager_v1::{self, Capability, ExtBackgroundEffectManagerV1},
    ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1,
};
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::Layer,
    zwlr_layer_surface_v1::{Anchor, KeyboardInteractivity},
};
use smithay::utils::Scale;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};

use super::client::{ClientId, LayerConfigureProps};
use super::Fixture;
use crate::render_helpers::{render_to_vec, RenderCtx, RenderTarget};

type Capabilities = Arc<Mutex<Option<Capability>>>;

fn assert_same_pixels(actual: &[u8], expected: &[u8], message: std::fmt::Arguments<'_>) {
    assert_eq!(
        actual.len(),
        expected.len(),
        "{message}: image size changed"
    );
    let mut mismatches = actual
        .as_chunks::<4>()
        .0
        .iter()
        .zip(expected.as_chunks::<4>().0.iter())
        .enumerate()
        .filter(|(_, (actual, expected))| actual != expected);
    if let Some((index, (actual, expected))) = mismatches.next() {
        let count = 1 + mismatches.count();
        panic!("{message}: {count} differing pixels, first at ({},{}): actual={actual:?}, expected={expected:?}", index % 320, index / 320);
    }
}

macro_rules! assert_pixels_eq {
    ($actual:expr, $expected:expr $(,)?) => {
        assert_same_pixels(&$actual, &$expected, format_args!("images must match"))
    };
    ($actual:expr, $expected:expr, $($message:tt)+) => {
        assert_same_pixels(&$actual, &$expected, format_args!($($message)+))
    };
}

impl Dispatch<ExtBackgroundEffectManagerV1, Capabilities> for super::client::State {
    fn event(
        _state: &mut Self,
        _proxy: &ExtBackgroundEffectManagerV1,
        event: ext_background_effect_manager_v1::Event,
        capabilities: &Capabilities,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            ext_background_effect_manager_v1::Event::Capabilities {
                flags: WEnum::Value(flags),
            } => *capabilities.lock().unwrap() = Some(flags),
            _ => panic!("unexpected background-effect capabilities event: {event:?}"),
        }
    }
}

wayland_client::delegate_noop!(super::client::State: ignore ExtBackgroundEffectSurfaceV1);

fn manager(f: &mut Fixture, client: ClientId) -> ExtBackgroundEffectManagerV1 {
    let capabilities = Capabilities::default();
    let c = f.client(client);
    let global = c
        .state
        .globals
        .iter()
        .find(|g| g.interface == ExtBackgroundEffectManagerV1::interface().name)
        .unwrap();
    assert_eq!(global.version, 1);
    let registry = c.display.get_registry(&c.qh, ());
    let manager = registry.bind(global.name, 1, &c.qh, capabilities.clone());
    f.double_roundtrip(client);
    assert_eq!(*capabilities.lock().unwrap(), Some(Capability::Blur));
    manager
}

fn map_pattern(f: &mut Fixture, client: ClientId) {
    let window = f.client(client).create_window();
    let parent = window.surface.clone();
    window.commit();
    f.double_roundtrip(client);
    let c = f.client(client);
    // Synchronized opaque subsurfaces form a checkerboard in an ordinary app,
    // deliberately distinct from the solid wallpaper used by xray caches.
    for row in 0..8 {
        for column in 0..10 {
            let child = c
                .state
                .compositor
                .as_ref()
                .unwrap()
                .create_surface(&c.qh, ());
            let subsurface =
                c.state
                    .subcompositor
                    .as_ref()
                    .unwrap()
                    .get_subsurface(&child, &parent, &c.qh, ());
            subsurface.set_position(column * 8, row * 8);
            let viewport = c
                .state
                .viewporter
                .as_ref()
                .unwrap()
                .get_viewport(&child, &c.qh, ());
            viewport.set_destination(8, 8);
            let (red, green) = if (row + column) % 2 == 0 {
                (u32::MAX / 10 * 9, u32::MAX / 10)
            } else {
                (u32::MAX / 10, u32::MAX / 10 * 9)
            };
            let buffer = c.state.spbm.as_ref().unwrap().create_u32_rgba_buffer(
                red,
                green,
                0,
                u32::MAX,
                &c.qh,
                (),
            );
            child.attach(Some(&buffer), 0, 0);
            child.commit();
        }
    }
    let window = c.window(&parent);
    window.attach_new_buffer();
    window.set_size(80, 64);
    window.ack_last_and_commit();
    f.double_roundtrip(client);
    f.clarion().layout.toggle_window_floating(None);
    f.clarion().layout.move_floating_window(
        None,
        clarion_ipc::PositionChange::SetFixed(16.),
        clarion_ipc::PositionChange::SetFixed(16.),
        false,
    );
    f.clarion_complete_animations();
    f.double_roundtrip(client);
}

fn map_glass(f: &mut Fixture, client: ClientId) -> WlSurface {
    let output = f.client(client).output("headless-1");
    let panel = f
        .client(client)
        .create_layer(Some(&output), Layer::Top, "protocol-glass");
    let surface = panel.surface.clone();
    panel.set_configure_props(LayerConfigureProps {
        size: Some((120, 100)),
        anchor: Some(Anchor::Top | Anchor::Left),
        exclusive_zone: Some(0),
        kb_interactivity: Some(KeyboardInteractivity::None),
        ..Default::default()
    });
    panel.commit();
    f.double_roundtrip(client);
    let panel = f.client(client).layer(&surface);
    panel.attach_new_buffer();
    panel.set_size(120, 100);
    panel.ack_last_and_commit();
    f.double_roundtrip(client);
    surface
}

fn set_region(
    f: &mut Fixture,
    client: ClientId,
    effect: &ExtBackgroundEffectSurfaceV1,
    rects: &[(i32, i32, i32, i32)],
) {
    let c = f.client(client);
    let region = c
        .state
        .compositor
        .as_ref()
        .unwrap()
        .create_region(&c.qh, ());
    for &(x, y, width, height) in rects {
        region.add(x, y, width, height);
    }
    effect.set_blur_region(Some(&region));
    // The protocol must copy the region, rather than retain this object.
    region.destroy();
    f.double_roundtrip(client);
}

fn pixels(f: &mut Fixture, output_index: u8, target: RenderTarget) -> Vec<u8> {
    let output = f.clarion_output(output_index);
    let state = f.clarion_state();
    let clarion = &mut state.clarion;
    clarion.update_render_elements(None);
    macro_rules! draw {
        ($renderer:expr) => {{
            let renderer = $renderer;
            let elements = clarion.render_to_vec(
                RenderCtx {
                    renderer,
                    target,
                    xray: None,
                },
                &output,
                false,
            );
            render_to_vec(
                renderer,
                output.current_mode().unwrap().size,
                Scale::from(output.current_scale().fractional_scale()),
                output.current_transform(),
                Fourcc::Abgr8888,
                elements.iter().rev(),
            )
            .unwrap()
        }};
    }
    state
        .backend
        .with_renderer(|renderer| match renderer {
            crate::render_helpers::renderer::AnyRenderer::Gles(renderer) => draw!(renderer),
            crate::render_helpers::renderer::AnyRenderer::Native(renderer) => draw!(renderer),
        })
        .unwrap()
}

fn save_image(native: bool, scale: f64, phase: &str, pixels: &[u8]) {
    let Some(dir) = std::env::var_os("CLARION_BLUR_PNG_DIR") else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    let path = std::path::Path::new(&dir).join(format!(
        "{}-{scale}-{phase}.png",
        if native { "vulkan" } else { "gles" },
    ));
    let mut encoder = png::Encoder::new(std::fs::File::create(&path).unwrap(), 320, 240);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(pixels)
        .unwrap();
    eprintln!("BLUR_CAPTURE: {}", path.display());
}

fn test_layer_protocol(native: bool) {
    for scale in [1., 1.25, 2.] {
        let mut config = Config::default();
        config.animations.off = true;
        config.hotkey_overlay.skip_at_startup = true;
        config.layout.focus_ring.off = true;
        config.layout.border.off = true;
        config.blur.noise = 0.;
        config.blur.saturation = 1.;
        config.blur.passes = 2;
        config.blur.offset = 2.;
        config.window_rules.push(WindowRule {
            block_out_from: Some(BlockOutFrom::ScreenCapture),
            ..Default::default()
        });
        let mut f = Fixture::with_config(config);
        if native {
            let mut headless = crate::backend::Headless::new(
                f.clarion().config.clone(),
                crate::backend::RendererBackendType::Vulkan,
            );
            headless.add_renderer().unwrap();
            headless.init(&mut crate::platform::device_bridge::DeviceCoordinator::new(
                f.clarion(),
            ));
            f.clarion_state().backend = crate::backend::Backend::Headless(headless);
            f.clarion_state()
                .backend
                .with_renderer(|renderer| match renderer {
                    crate::render_helpers::renderer::AnyRenderer::Native(renderer) => eprintln!(
                        "HARDWARE: background-effect Vulkan {}",
                        renderer.vulkan().info.device_name,
                    ),
                    crate::render_helpers::renderer::AnyRenderer::Gles(_) => {
                        panic!("Vulkan test must not fall back to GLES")
                    }
                })
                .unwrap();
        } else {
            f.init_gles_renderer();
            f.clarion_state()
                .backend
                .with_renderer(|renderer| match renderer {
                    crate::render_helpers::renderer::AnyRenderer::Gles(renderer) => {
                        let name = renderer
                            .with_context(|gl| unsafe {
                                let name = gl.GetString(0x1f01); // GL_RENDERER, queried without changing GL state.
                                assert!(!name.is_null());
                                std::ffi::CStr::from_ptr(name.cast())
                                    .to_string_lossy()
                                    .into_owned()
                            })
                            .unwrap();
                        eprintln!(
                            "HARDWARE: background-effect GLES {name} software={}",
                            renderer.is_software()
                        );
                    }
                    crate::render_helpers::renderer::AnyRenderer::Native(_) => unreachable!(),
                })
                .unwrap();
        }
        f.add_output(1, (320, 240));
        f.add_output(2, (320, 240));
        f.clarion_focus_output(1);
        let output = f.clarion_output(1);
        output.change_current_state(None, None, Some(OutputScale::Fractional(scale)), None);
        f.clarion().output_resized(&output);
        let client = f.add_client();
        map_pattern(&mut f, client);
        let surface = map_glass(&mut f, client);
        let manager = manager(&mut f, client);
        let c = f.client(client);
        let effect = manager.get_background_effect(&surface, &c.qh, ());
        let baseline = pixels(&mut f, 1, RenderTarget::Output);
        let second_baseline = pixels(&mut f, 2, RenderTarget::Output);
        let capture_baseline = pixels(&mut f, 1, RenderTarget::ScreenCapture);
        save_image(native, scale, "before", &baseline);
        let sample = |image: &[u8], x: f64, y: f64| {
            let offset = ((y * scale).floor() as usize * 320 + (x * scale).floor() as usize) * 4;
            <[u8; 4]>::try_from(&image[offset..offset + 4]).unwrap()
        };
        let color = sample(&baseline, 28., 28.);
        assert!(
            color[0].abs_diff(color[1]) > 150 && color[2] < 5,
            "ordinary app checkerboard missing: {color:?}, scale={scale}, native={native}"
        );

        set_region(
            &mut f,
            client,
            &effect,
            &[(32, 32, 16, 16), (64, 32, 16, 16)],
        );
        assert_pixels_eq!(
            pixels(&mut f, 1, RenderTarget::Output),
            baseline,
            "region must await wl_surface.commit"
        );
        surface.commit();
        f.double_roundtrip(client);
        let blurred = pixels(&mut f, 1, RenderTarget::Output);
        save_image(native, scale, "blur", &blurred);
        let mut changed = 0;
        for y in 0..240 {
            for x in 0..320 {
                let logical_x = (f64::from(x) + 0.5) / scale;
                let logical_y = (f64::from(y) + 0.5) / scale;
                let inside = (32. ..48.).contains(&logical_y)
                    && ((32. ..48.).contains(&logical_x) || (64. ..80.).contains(&logical_x));
                let offset = (y as usize * 320 + x as usize) * 4;
                if inside {
                    changed +=
                        usize::from(baseline[offset..offset + 4] != blurred[offset..offset + 4]);
                    // Xray would show the gray wallpaper instead of these app colors.
                    assert!(
                        blurred[offset + 2] < 5,
                        "blur must preserve the actual application background"
                    );
                } else {
                    assert_eq!(
                        &blurred[offset..offset + 4],
                        &baseline[offset..offset + 4],
                        "blur escaped its disjoint region at ({x},{y}), scale={scale}"
                    );
                }
            }
        }
        assert!(changed > 100, "the committed region must visibly blur checkerboard, changed={changed}, scale={scale}, native={native}");
        assert_pixels_eq!(
            pixels(&mut f, 2, RenderTarget::Output),
            second_baseline,
            "one output's effect must not contaminate another output"
        );
        for target in [RenderTarget::ScreenCapture, RenderTarget::Screencast] {
            let capture = pixels(&mut f, 1, target);
            save_image(
                native,
                scale,
                if target == RenderTarget::ScreenCapture {
                    "protected-capture"
                } else {
                    "protected-cast"
                },
                &capture,
            );
            for (x, y) in [(36., 36.), (44., 44.), (68., 36.), (76., 44.)] {
                assert_eq!(sample(&capture, x, y), sample(&capture_baseline, x, y), "blur must sample the protected scene, not leak the output's application pixels");
            }
        }
        let removed = f.clarion_output(2);
        f.clarion().remove_output(&removed);
        f.double_roundtrip(client);
        assert_pixels_eq!(
            pixels(&mut f, 1, RenderTarget::Output),
            blurred,
            "removing another output must not invalidate the remaining layer's blur"
        );

        effect.set_blur_region(None);
        f.double_roundtrip(client);
        assert_pixels_eq!(
            pixels(&mut f, 1, RenderTarget::Output),
            blurred,
            "NULL must await commit"
        );
        surface.commit();
        f.double_roundtrip(client);
        assert_pixels_eq!(
            pixels(&mut f, 1, RenderTarget::Output),
            baseline,
            "NULL must clear blur on commit"
        );
        set_region(&mut f, client, &effect, &[]);
        surface.commit();
        f.double_roundtrip(client);
        assert_pixels_eq!(
            pixels(&mut f, 1, RenderTarget::Output),
            baseline,
            "an explicitly empty region must disable blur"
        );
        set_region(
            &mut f,
            client,
            &effect,
            &[(32, 32, 16, 16), (64, 32, 16, 16)],
        );
        surface.commit();
        f.double_roundtrip(client);
        assert_pixels_eq!(pixels(&mut f, 1, RenderTarget::Output), blurred);
        effect.destroy();
        f.double_roundtrip(client);
        assert_pixels_eq!(
            pixels(&mut f, 1, RenderTarget::Output),
            blurred,
            "effect destruction must await commit"
        );
        surface.commit();
        f.double_roundtrip(client);
        assert_pixels_eq!(
            pixels(&mut f, 1, RenderTarget::Output),
            baseline,
            "destroyed effect must clear its cached region"
        );

        // Reusing a surface after releasing the effect must not retain protocol
        // ownership or stale renderer data. Unmapping/remapping must also work.
        let c = f.client(client);
        let recreated = manager.get_background_effect(&surface, &c.qh, ());
        set_region(
            &mut f,
            client,
            &recreated,
            &[(32, 32, 16, 16), (64, 32, 16, 16)],
        );
        surface.commit();
        f.double_roundtrip(client);
        assert_pixels_eq!(pixels(&mut f, 1, RenderTarget::Output), blurred);
        surface.attach(None, 0, 0);
        surface.commit();
        f.double_roundtrip(client);
        assert_pixels_eq!(
            pixels(&mut f, 1, RenderTarget::Output),
            baseline,
            "unmapped transparent layer must remove its blur"
        );
        let panel = f.client(client).layer(&surface);
        panel.set_configure_props(LayerConfigureProps {
            size: Some((120, 100)),
            anchor: Some(Anchor::Top | Anchor::Left),
            exclusive_zone: Some(0),
            kb_interactivity: Some(KeyboardInteractivity::None),
            layer: Some(Layer::Top),
            ..Default::default()
        });
        panel.commit();
        f.double_roundtrip(client);
        let panel = f.client(client).layer(&surface);
        panel.attach_new_buffer();
        panel.set_size(120, 100);
        panel.ack_last_and_commit();
        f.double_roundtrip(client);
        assert_pixels_eq!(
            pixels(&mut f, 1, RenderTarget::Output),
            blurred,
            "remapped layer must recover its committed protocol blur"
        );
        recreated.destroy();
        manager.destroy();
        f.double_roundtrip(client);
        eprintln!("BLUR_VERIFIED: backend={} scale={scale} changed_pixels={changed} copy=ok double_buffer=ok targets=ok outputs=ok cleanup=ok", if native { "vulkan" } else { "gles" });
    }
}

#[test]
fn translucent_layer_protocol_blurs_actual_background_on_gles() {
    test_layer_protocol(false);
}

#[test]
#[ignore = "requires an independent Vulkan GPU device; never acquires DRM display master"]
fn translucent_layer_protocol_blurs_actual_background_on_vulkan() {
    test_layer_protocol(true);
}

#[test]
fn older_blocked_commit_cannot_consume_newer_blur_region_change() {
    use crate::handlers::background_effect::get_cached_blur_region;
    use smithay::reexports::wayland_server::Resource as _;
    use smithay::utils::Rectangle;
    use smithay::wayland::compositor::{add_blocker, with_states, Barrier, CompositorHandler as _};

    for close in ["bar", "unset", "empty", "destroy"] {
        for commit_close_before_release in [false, true] {
            let mut f = Fixture::new();
            f.add_output(1, (320, 240));
            let client = f.add_client();
            f.double_roundtrip(client);
            let surface = map_glass(&mut f, client);
            let manager = manager(&mut f, client);
            let c = f.client(client);
            let effect = manager.get_background_effect(&surface, &c.qh, ());
            let server = f
                .clarion()
                .mapped_layer_surfaces
                .keys()
                .find(|layer| layer.wl_surface().id().protocol_id() == surface.id().protocol_id())
                .unwrap()
                .wl_surface()
                .clone();
            let cached = || with_states(&server, get_cached_blur_region);
            let full = Some(Arc::new(vec![Rectangle::new(
                (0, 0).into(),
                (120, 100).into(),
            )]));
            let expected = match close {
                "bar" => Some(Arc::new(vec![Rectangle::new(
                    (0, 0).into(),
                    (120, 8).into(),
                )])),
                "empty" => Some(Arc::new(vec![])),
                _ => None,
            };
            set_region(&mut f, client, &effect, &[(0, 0, 120, 100)]);
            surface.commit();
            f.double_roundtrip(client);
            assert_eq!(cached(), full);

            // An ordinary animation buffer waits for its acquire fence. A later
            // close changes the mask while that older commit is still blocked.
            let old_frame = Barrier::new(false);
            add_blocker(&server, old_frame.clone());
            surface.commit();
            f.double_roundtrip(client);
            match close {
                "bar" => set_region(&mut f, client, &effect, &[(0, 0, 120, 8)]),
                "empty" => set_region(&mut f, client, &effect, &[]),
                "unset" => effect.set_blur_region(None),
                "destroy" => effect.destroy(),
                _ => unreachable!(),
            }
            f.double_roundtrip(client);
            let closing_frame = Barrier::new(false);
            if commit_close_before_release {
                add_blocker(&server, closing_frame.clone());
                surface.commit();
                f.double_roundtrip(client);
            }
            let release = |f: &mut Fixture, barrier: &Barrier| {
                barrier.signal();
                let state = f.clarion_state();
                let dh = state.clarion.display_handle.clone();
                state
                    .client_compositor_state(&server.client().unwrap())
                    .blocker_cleared(state, &dh);
            };
            release(&mut f, &old_frame);
            assert_eq!(cached(), full, "an older frame must retain its own mask");
            if commit_close_before_release {
                release(&mut f, &closing_frame);
            } else {
                surface.commit();
                f.double_roundtrip(client);
            }
            assert_eq!(
                cached(),
                expected,
                "{close}: closing must invalidate the full-screen blur cache"
            );
            // No input or additional set_blur_region request should be necessary.
            surface.commit();
            f.double_roundtrip(client);
            assert_eq!(cached(), expected);
        }
    }
}
