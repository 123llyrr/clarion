//! End-to-end, isolated Wayland color-management acceptance. This uses the real
//! State::new native bootstrap, protocol SHM negotiation, committed surface
//! metadata, the compositor's ordinary output scene and its PNG capture action.
//! It never assigns a description directly to an imported Vulkan texture.

use std::fs::{File, OpenOptions};
use std::io::BufReader;
use std::os::fd::AsFd;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clarion_config::{Action, Config};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::{ExportMem, Texture};
use smithay::reexports::wayland_protocols::wp::color_management::v1::client::wp_color_manager_v1::RenderIntent;
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer;
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    Anchor, KeyboardInteractivity,
};
use smithay::utils::Rectangle;
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};

use super::client::{ClientId, LayerConfigureProps, State as ClientState};
use super::Fixture;
use crate::backend::headless::HeadlessRenderer;
use crate::color::{ColorDescription, RenderingIntent, SurfaceColor};

const SIZE: (u16, u16) = (64, 48);
const RGB: [f32; 3] = [0.75, 0.5, 0.25];
static NEXT_FILES: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Default)]
struct ShmFormats(Arc<Mutex<Vec<wl_shm::Format>>>);

impl Dispatch<WlShm, ShmFormats> for ClientState {
    fn event(
        _: &mut Self,
        _: &WlShm,
        event: wl_shm::Event,
        formats: &ShmFormats,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_shm::Event::Format {
            format: WEnum::Value(format),
        } = event
        {
            formats.0.lock().unwrap().push(format);
        }
    }
}

struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "clarion-wayland-hdr-{}-{}",
            std::process::id(),
            NEXT_FILES.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fixture() -> Fixture {
    let mut config = Config::default();
    config.animations.off = true;
    native_fixture(config, SIZE)
}

fn native_fixture(mut config: Config, size: (u16, u16)) -> Fixture {
    let _node = std::env::var_os("CLARION_VULKAN_TEST_NODE")
        .expect("assign a real DRM render node with CLARION_VULKAN_TEST_NODE");
    config.hotkey_overlay.skip_at_startup = true;
    let mut fixture =
        Fixture::with_config_and_renderer(config, crate::backend::RendererBackendType::Vulkan);
    assert!(fixture.clarion_state().backend.supports_color_management(),
        "the production bootstrap must use native FP16 color management; GLES fallback is not acceptance");
    fixture
        .clarion_state()
        .backend
        .with_renderer(|renderer| match renderer {
            crate::render_helpers::renderer::AnyRenderer::Native(renderer) => {
                eprintln!(
                    "WAYLAND HDR HARDWARE: {}",
                    renderer.vulkan().info.device_name
                );
                assert!(renderer.supports_color_management());
            }
            _ => panic!("the end-to-end fixture unexpectedly fell back to GLES"),
        })
        .unwrap();
    fixture.add_output(1, size);
    fixture
}

fn expected(color: SurfaceColor) -> [u8; 4] {
    expected_rgb(RGB, color)
}

fn expected_rgb(rgb: [f32; 3], color: SurfaceColor) -> [u8; 4] {
    let rgb = crate::color::convert_color(rgb, color, ColorDescription::srgb());
    [
        (rgb[0].clamp(0., 1.) * 255.).round() as u8,
        (rgb[1].clamp(0., 1.) * 255.).round() as u8,
        (rgb[2].clamp(0., 1.) * 255.).round() as u8,
        255,
    ]
}

fn matching(actual: [u8; 4], expected: [u8; 4]) -> bool {
    actual
        .into_iter()
        .zip(expected)
        .all(|(actual, expected)| actual.abs_diff(expected) <= 3)
}

fn target_pixel(fixture: &mut Fixture) -> Option<[u8; 4]> {
    let headless = fixture.clarion_state().backend.headless();
    let target = headless.native_targets.get("headless-1")?;
    if !target.valid {
        return None;
    }
    let texture = target.buffer.clone();
    let Some(HeadlessRenderer::Native(renderer)) = headless.renderer.as_mut() else {
        panic!("native fixture renderer changed");
    };
    let mapping = renderer
        .copy_texture(
            &texture,
            Rectangle::new(
                (i32::from(SIZE.0 / 2), i32::from(SIZE.1 / 2)).into(),
                (1, 1).into(),
            ),
            Fourcc::Abgr8888,
        )
        .unwrap();
    Some(renderer.map_texture(&mapping).unwrap().try_into().unwrap())
}

fn wait_target(fixture: &mut Fixture, expected: [u8; 4]) -> [u8; 4] {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut last = None;
    loop {
        fixture.state.server.dispatch();
        fixture.dispatch();
        if let Some(pixel) = target_pixel(fixture) {
            if matching(pixel, expected) {
                return pixel;
            }
            last = Some(pixel);
        }
        assert!(
            Instant::now() < deadline,
            "the composed headless target did not update: actual {last:?}, expected {expected:?}"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn capture_pixel(fixture: &mut Fixture, path: &Path) -> [u8; 4] {
    fixture.clarion_state().do_action(
        Action::ScreenshotScreen(true, false, Some(path.to_string_lossy().into_owned())),
        false,
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        fixture.state.server.dispatch();
        fixture.dispatch();
        if let Ok(file) = File::open(path) {
            if let Ok(mut reader) = png::Decoder::new(BufReader::new(file)).read_info() {
                let mut bytes = vec![0; reader.output_buffer_size().unwrap()];
                if let Ok(info) = reader.next_frame(&mut bytes) {
                    assert_eq!(
                        (info.width, info.height),
                        (u32::from(SIZE.0), u32::from(SIZE.1))
                    );
                    assert_eq!(info.color_type, png::ColorType::Rgba);
                    let offset = ((usize::from(SIZE.1 / 2) * usize::from(SIZE.0))
                        + usize::from(SIZE.0 / 2))
                        * 4;
                    return bytes[offset..offset + 4].try_into().unwrap();
                }
            }
        }
        assert!(
            Instant::now() < deadline,
            "the production screenshot action did not complete"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn verify(fixture: &mut Fixture, files: &Files, name: &str, color: SurfaceColor) -> [u8; 4] {
    let reference = expected(color);
    let displayed = wait_target(fixture, reference);
    let captured = capture_pixel(fixture, &files.path(name));
    assert!(
        matching(captured, reference),
        "{name}: screenshot {captured:?}, CPU reference {reference:?}, actual output {displayed:?}"
    );
    assert!(
        matching(captured, displayed),
        "{name}: screenshot {captured:?} differs from the displayed SDR scene {displayed:?}"
    );
    displayed
}

#[test]
#[ignore = "requires an assigned real DRM render node and native Vulkan; never acquires display master"]
fn native_fp16_wayland_metadata_reaches_scene_and_sdr_capture() {
    let files = Files::new();
    let mut fixture = fixture();
    let id: ClientId = fixture.add_client();
    fixture.double_roundtrip(id);
    let client = fixture.client(id);
    let manager = client
        .state
        .color_manager
        .as_ref()
        .expect("the production native bootstrap must advertise wp_color_manager_v1")
        .clone();
    let registry = client.display.get_registry(&client.qh, ());
    let global = client
        .state
        .globals
        .iter()
        .find(|global| global.interface == WlShm::interface().name)
        .unwrap();
    let formats = ShmFormats::default();
    let shm: WlShm = registry.bind(global.name, 1, &client.qh, formats.clone());
    fixture.double_roundtrip(id);
    assert!(
        formats
            .0
            .lock()
            .unwrap()
            .contains(&wl_shm::Format::Abgr16161616f),
        "wl_shm FP16 advertisement must come from the actual native renderer's capabilities"
    );

    let file = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(files.path("client-shm"))
        .unwrap();
    std::fs::remove_file(files.path("client-shm")).unwrap();
    // ABGR16161616F on these Linux GPU targets stores binary16 R,G,B,A words.
    // All sample values are exactly representable, independent of f32→f16 helpers.
    let pixel = [0x3a00_u16, 0x3800, 0x3400, 0x3c00]
        .map(u16::to_ne_bytes)
        .concat();
    let bytes = pixel.repeat(usize::from(SIZE.0) * usize::from(SIZE.1));
    file.set_len(bytes.len() as u64).unwrap();
    file.write_all_at(&bytes, 0).unwrap();
    let client = fixture.client(id);
    let pool = shm.create_pool(file.as_fd(), bytes.len() as i32, &client.qh, ());
    let buffer = pool.create_buffer(
        0,
        i32::from(SIZE.0),
        i32::from(SIZE.1),
        i32::from(SIZE.0) * 8,
        wl_shm::Format::Abgr16161616f,
        &client.qh,
        (),
    );
    let output = client.state.outputs.keys().next().unwrap().clone();
    let layer = client.create_layer(Some(&output), Layer::Overlay, "clarion-hdr-color-fixture");
    let surface = layer.surface.clone();
    layer.set_configure_props(LayerConfigureProps {
        size: Some((u32::from(SIZE.0), u32::from(SIZE.1))),
        anchor: Some(Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right),
        exclusive_zone: Some(-1),
        kb_interactivity: Some(KeyboardInteractivity::None),
        ..Default::default()
    });
    layer.commit();
    fixture.double_roundtrip(id);
    let color = manager.get_surface(&surface, &fixture.client(id).qh, ());
    let layer = fixture.client(id).layer(&surface);
    layer.ack_last();
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, i32::from(SIZE.0), i32::from(SIZE.1));
    surface.commit();
    fixture.double_roundtrip(id);
    let legacy = verify(
        &mut fixture,
        &files,
        "untagged.png",
        SurfaceColor::default(),
    );

    let scrgb = manager.create_windows_scrgb(&fixture.client(id).qh, ());
    let pq = manager.create_windows_bt2100(&fixture.client(id).qh, ());
    fixture.double_roundtrip(id);
    color.set_image_description(&scrgb, RenderIntent::Perceptual);
    // Change only committed metadata: no attach, damage or changed SHM bytes.
    surface.commit();
    fixture.double_roundtrip(id);
    let scrgb_pixel = verify(
        &mut fixture,
        &files,
        "scrgb.png",
        SurfaceColor {
            description: ColorDescription::windows_scrgb(),
            intent: RenderingIntent::Perceptual,
        },
    );
    assert_ne!(
        scrgb_pixel, legacy,
        "scRGB metadata must change the actual pixels"
    );

    color.set_image_description(&pq, RenderIntent::Perceptual);
    surface.commit();
    fixture.double_roundtrip(id);
    let pq_pixel = verify(
        &mut fixture,
        &files,
        "pq.png",
        SurfaceColor {
            description: ColorDescription::windows_bt2100(),
            intent: RenderingIntent::Perceptual,
        },
    );
    assert_ne!(
        pq_pixel, scrgb_pixel,
        "PQ metadata must replace the cached scRGB import"
    );

    color.unset_image_description();
    surface.commit();
    fixture.double_roundtrip(id);
    let restored = verify(
        &mut fixture,
        &files,
        "restored-sdr.png",
        SurfaceColor::default(),
    );
    assert_eq!(
        restored, legacy,
        "unset must restore the original buffer's SDR interpretation"
    );
    // Keep the client's pool and FD alive until every GPU/capture operation ends.
    drop(pool);
    drop(file);
}

fn wait_scene_colors(fixture: &mut Fixture, colors: &[[u8; 4]]) {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut counts = vec![0; colors.len()];
    loop {
        fixture.state.server.dispatch();
        fixture.dispatch();
        let headless = fixture.clarion_state().backend.headless();
        if let Some(target) = headless
            .native_targets
            .get("headless-1")
            .filter(|t| t.valid)
        {
            let texture = target.buffer.clone();
            let Some(HeadlessRenderer::Native(renderer)) = headless.renderer.as_mut() else {
                panic!("native fixture renderer changed");
            };
            let mapping = renderer
                .copy_texture(
                    &texture,
                    Rectangle::from_size(texture.size()),
                    Fourcc::Abgr8888,
                )
                .unwrap();
            let pixels = renderer.map_texture(&mapping).unwrap();
            counts.fill(0);
            for pixel in pixels.as_chunks::<4>().0 {
                for (index, color) in colors.iter().enumerate() {
                    counts[index] += usize::from(matching(*pixel, *color));
                }
            }
            if counts.iter().all(|count| *count > 100) {
                return;
            }
        }
        assert!(
            Instant::now() < deadline,
            "the actual output never contained the expected committed colors {colors:?}: {counts:?} pixels"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
#[ignore = "requires an assigned real DRM render node and native Vulkan; never acquires display master"]
fn native_resize_snapshot_keeps_previous_parent_and_synchronized_child_colors() {
    use clarion_config::animations::{Curve, EasingParams, Kind};
    use clarion_ipc::SizeChange;
    use smithay::backend::renderer::utils::with_renderer_surface_state;
    use smithay::reexports::wayland_server::Resource;

    use crate::protocols::color_management::surface_color_description_for_surface;
    use crate::render_helpers::RenderTarget;

    let files = Files::new();
    let mut config = Config::default();
    config.layout.gaps = 0.;
    config.animations.window_open.anim.off = true;
    config.animations.window_resize.anim.kind = Kind::Easing(EasingParams {
        duration_ms: 1000,
        curve: Curve::Linear,
    });
    let mut fixture = native_fixture(config, (640, 480));
    let id = fixture.add_client();
    fixture.double_roundtrip(id);
    let client = fixture.client(id);
    let manager = client.state.color_manager.as_ref().unwrap().clone();
    let scrgb = manager.create_windows_scrgb(&client.qh, ());
    let pq = manager.create_windows_bt2100(&client.qh, ());
    let registry = client.display.get_registry(&client.qh, ());
    let global = client
        .state
        .globals
        .iter()
        .find(|global| global.interface == WlShm::interface().name)
        .unwrap();
    let shm: WlShm = registry.bind(global.name, 1, &client.qh, ShmFormats::default());
    let window = client.create_window();
    let surface = window.surface.clone();
    window.commit();
    fixture.double_roundtrip(id);

    // Four actual attachments: every parent/child replacement has distinct
    // storage and buffer identity, as well as a different color description.
    let old = [0x3a00_u16, 0x3800, 0x3400, 0x3c00]
        .map(u16::to_ne_bytes)
        .concat()
        .repeat(usize::from(SIZE.0) * usize::from(SIZE.1));
    let new = [0x3400_u16, 0x3a00, 0x3800, 0x3c00]
        .map(u16::to_ne_bytes)
        .concat()
        .repeat(usize::from(SIZE.0) * usize::from(SIZE.1));
    let bytes = [&old[..], &old[..], &new[..], &new[..]].concat();
    let file = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(files.path("resize-shm"))
        .unwrap();
    std::fs::remove_file(files.path("resize-shm")).unwrap();
    file.set_len(bytes.len() as u64).unwrap();
    file.write_all_at(&bytes, 0).unwrap();
    let client = fixture.client(id);
    let pool = shm.create_pool(file.as_fd(), bytes.len() as i32, &client.qh, ());
    let buffers: Vec<_> = (0..4)
        .map(|index| {
            pool.create_buffer(
                index * old.len() as i32,
                i32::from(SIZE.0),
                i32::from(SIZE.1),
                i32::from(SIZE.0) * 8,
                wl_shm::Format::Abgr16161616f,
                &client.qh,
                (),
            )
        })
        .collect();
    let root_color = manager.get_surface(&surface, &client.qh, ());
    root_color.set_image_description(&scrgb, RenderIntent::Perceptual);
    let child = client
        .state
        .compositor
        .as_ref()
        .unwrap()
        .create_surface(&client.qh, ());
    let subsurface = client.state.subcompositor.as_ref().unwrap().get_subsurface(
        &child,
        &surface,
        &client.qh,
        (),
    );
    let viewport = client
        .state
        .viewporter
        .as_ref()
        .unwrap()
        .get_viewport(&child, &client.qh, ());
    let child_color = manager.get_surface(&child, &client.qh, ());
    child_color.set_image_description(&pq, RenderIntent::Perceptual);
    subsurface.set_position(100, 0);
    viewport.set_destination(100, 180);
    child.attach(Some(&buffers[1]), 0, 0);
    child.damage_buffer(0, 0, i32::from(SIZE.0), i32::from(SIZE.1));
    child.commit();
    let window = client.window(&surface);
    window.set_size(200, 180);
    surface.attach(Some(&buffers[0]), 0, 0);
    surface.damage_buffer(0, 0, i32::from(SIZE.0), i32::from(SIZE.1));
    window.ack_last_and_commit();
    fixture.double_roundtrip(id);
    fixture.client(id).window(&surface).ack_last_and_commit();
    fixture.double_roundtrip(id);
    fixture.clarion_complete_animations();
    let root_server = fixture
        .clarion()
        .layout
        .windows()
        .next()
        .unwrap()
        .1
        .toplevel()
        .wl_surface()
        .clone();
    let child_server = fixture
        .clarion()
        .root_surface
        .keys()
        .find(|s| s.id().protocol_id() == child.id().protocol_id())
        .unwrap()
        .clone();
    let linear = SurfaceColor {
        description: ColorDescription::windows_scrgb(),
        intent: RenderingIntent::Perceptual,
    };
    let absolute = SurfaceColor {
        description: ColorDescription::windows_bt2100(),
        intent: RenderingIntent::Perceptual,
    };
    wait_scene_colors(&mut fixture, &[expected(linear), expected(absolute)]);
    // Keep the old resize image alive while dispatching real Wayland commits.
    fixture.clarion().clock.set_rate(0.);
    fixture
        .clarion()
        .layout
        .find_window_and_output_mut(&root_server)
        .unwrap()
        .0
        .animation_snapshot_sources
        .clear();
    fixture
        .clarion()
        .layout
        .set_column_width(SizeChange::SetFixed(300));
    fixture.double_roundtrip(id);

    // The child's new buffer and metadata are cached until the parent applies.
    child_color.set_image_description(&scrgb, RenderIntent::Perceptual);
    child.attach(Some(&buffers[3]), 0, 0);
    child.damage_buffer(0, 0, i32::from(SIZE.0), i32::from(SIZE.1));
    viewport.set_destination(150, 280);
    subsurface.set_position(150, 0);
    child.commit();
    fixture.double_roundtrip(id);
    assert_eq!(
        surface_color_description_for_surface(&child_server),
        absolute
    );
    assert_eq!(
        with_renderer_surface_state(&child_server, |state| {
            state.buffer().unwrap().id().protocol_id()
        }),
        Some(buffers[1].id().protocol_id())
    );

    root_color.set_image_description(&pq, RenderIntent::Perceptual);
    surface.attach(Some(&buffers[2]), 0, 0);
    surface.damage_buffer(0, 0, i32::from(SIZE.0), i32::from(SIZE.1));
    let window = fixture.client(id).window(&surface);
    window.set_size(300, 280);
    window.ack_last_and_commit();
    fixture.double_roundtrip(id);
    assert_eq!(
        surface_color_description_for_surface(&root_server),
        absolute
    );
    assert_eq!(surface_color_description_for_surface(&child_server), linear);
    for (server, buffer) in [(&root_server, &buffers[2]), (&child_server, &buffers[3])] {
        assert_eq!(
            with_renderer_surface_state(server, |state| {
                state.buffer().unwrap().id().protocol_id()
            }),
            Some(buffer.id().protocol_id())
        );
    }
    let resources = {
        let clarion = fixture.clarion();
        let mapped = clarion
            .layout
            .find_window_and_output(&root_server)
            .unwrap()
            .0;
        assert_eq!(mapped.animation_snapshot_sources.len(), 1);
        assert_eq!(
            mapped.animation_snapshot_sources[0].1.size,
            (200, 180).into()
        );
        let workspace = clarion.layout.active_workspace().unwrap();
        let tile = workspace.tiles_with_render_positions().next().unwrap().0;
        tile.scene().resources
    };
    let headless = fixture.clarion_state().backend.headless();
    let Some(HeadlessRenderer::Native(renderer)) = headless.renderer.as_mut() else {
        panic!("native fixture renderer changed");
    };
    let resize = resources.resize.borrow();
    let snapshot = resize
        .as_ref()
        .expect("a real animated resize must retain its previous GPU image");
    let ((texture, bounds), size) = snapshot
        .snapshot
        .native_texture(renderer, RenderTarget::Output, 1.0.into())
        .unwrap();
    assert_eq!(
        texture.format(),
        Some(Fourcc::Abgr16161616f),
        "the production resize capture must retain the native scene's FP16 format"
    );
    assert_eq!(size, (200., 180.).into());
    assert_eq!(bounds.size, (200, 180).into());
    for (x, color) in [(50, linear), (150, absolute)] {
        let mapping = renderer
            .copy_texture(
                &texture,
                Rectangle::new((x, 90).into(), (1, 1).into()),
                Fourcc::Abgr8888,
            )
            .unwrap();
        let pixel: [u8; 4] = renderer.map_texture(&mapping).unwrap().try_into().unwrap();
        assert!(
            matching(pixel, expected(color)),
            "old resize snapshot x={x} has {pixel:?}, expected previous buffer+metadata {:?}",
            expected(color)
        );
    }
    drop(resize);
    // Retiring the animation must expose both new buffers with their new colors.
    fixture.clarion_complete_animations();
    let new_rgb = [0.25, 0.75, 0.5];
    wait_scene_colors(
        &mut fixture,
        &[
            expected_rgb(new_rgb, absolute),
            expected_rgb(new_rgb, linear),
        ],
    );
    drop(pool);
    drop(file);
}
