//! Exercise the production action/selection/readback/PNG pipeline in an owned
//! headless fixture. No capture or input is sent to the running desktop.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use clarion_config::{Action, Config};
use smithay::utils::Transform;

use super::*;
use crate::tests::Fixture;

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "clarion-capture-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
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

fn fixture(native: bool) -> Fixture {
    let config = Config {
        renderer: Some(if native {
            clarion_config::RendererBackendConfig::Vulkan
        } else {
            clarion_config::RendererBackendConfig::Gles
        }),
        animations: clarion_config::Animations {
            off: true,
            ..Default::default()
        },
        hotkey_overlay: clarion_config::HotkeyOverlay {
            skip_at_startup: true,
            ..Default::default()
        },
        layout: clarion_config::Layout {
            background_color: clarion_config::appearance::Color::from_rgba8_unpremul(
                34, 68, 102, 255,
            ),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut f = Fixture::with_config(config);
    if native {
        use crate::backend::headless::HeadlessRenderer;
        use crate::render_helpers::vulkan::{native::NativeRenderer, VulkanRenderer};
        let path =
            std::env::var_os("CLARION_VULKAN_TEST_NODE").expect("set CLARION_VULKAN_TEST_NODE");
        let node = smithay::backend::drm::DrmNode::from_path(&path).unwrap();
        let mut vk = VulkanRenderer::new();
        vk.init_from_drm_node(Some(&node)).unwrap();
        let mut renderer = NativeRenderer::new(vk).unwrap();
        crate::render_helpers::shaders::init_native(
            &mut renderer,
            &f.clarion().config.borrow().animations,
        );
        renderer
            .prepare_output_format_for_test(Fourcc::Xrgb8888)
            .unwrap();
        let mut headless = crate::backend::Headless::new(
            f.clarion().config.clone(),
            crate::backend::RendererBackendType::Vulkan,
        );
        headless.renderer = Some(HeadlessRenderer::Native(Box::new(renderer)));
        headless.init(&mut crate::platform::device_bridge::DeviceCoordinator::new(
            f.clarion(),
        ));
        f.clarion_state().backend = crate::backend::Backend::Headless(headless);
    } else {
        f.init_gles_renderer();
    }
    f.add_output(1, (320, 240));
    f.clarion_state()
        .backend
        .with_renderer(|renderer| match renderer {
            crate::render_helpers::renderer::AnyRenderer::Native(renderer) if native => {
                eprintln!("HARDWARE CAPTURE: {}", renderer.vulkan().info.device_name)
            }
            crate::render_helpers::renderer::AnyRenderer::Gles(_) if !native => (),
            _ => panic!("capture fixture used wrong renderer"),
        })
        .unwrap();
    f
}

fn wait_png(f: &mut Fixture, path: &Path) -> (u32, u32, Vec<u8>) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        f.state.server.dispatch();
        f.dispatch();
        if let Ok(file) = std::fs::File::open(path) {
            if let Ok(mut decoder) = png::Decoder::new(std::io::BufReader::new(file)).read_info() {
                let mut bytes = vec![0; decoder.output_buffer_size().unwrap()];
                if let Ok(info) = decoder.next_frame(&mut bytes) {
                    bytes.truncate(info.buffer_size());
                    return (info.width, info.height, bytes);
                }
            }
        }
        assert!(
            Instant::now() < deadline,
            "capture did not complete: {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn output_selection_window_pipeline(native: bool) {
    let files = Files::new();
    let mut f = fixture(native);
    for (index, transform, scale) in [
        (0, Transform::Normal, 1.),
        (1, Transform::_90, 1.25),
        (2, Transform::Flipped, 2.),
    ] {
        let output = f.clarion_output(1);
        output.change_current_state(
            None,
            Some(transform),
            Some(smithay::output::Scale::Fractional(scale)),
            None,
        );
        f.clarion().output_resized(&output);
        let path = files.path(&format!("output-{index}.png"));
        f.clarion_state().do_action(
            Action::ScreenshotScreen(true, false, Some(path.to_string_lossy().into_owned())),
            false,
        );
        let (w, h, bytes) = wait_png(&mut f, &path);
        let size = transform.transform_size(output.current_mode().unwrap().size);
        assert_eq!((w, h), (size.w as u32, size.h as u32));
        assert!(
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| *pixel == [34, 68, 102, 255]),
            "screen capture channel order, alpha or output coordinates changed"
        );

        let path = files.path(&format!("selection-{index}.png"));
        f.clarion_state().do_action(
            Action::Screenshot(false, Some(path.to_string_lossy().into_owned())),
            false,
        );
        let ScreenshotUi::Open {
            selection: (_, a, b),
            ..
        } = &f.clarion().screenshot_ui
        else {
            panic!("selection failed to open");
        };
        let expected = (
            (a.x - b.x).unsigned_abs() + 1,
            (a.y - b.y).unsigned_abs() + 1,
        );
        f.clarion_state().confirm_screenshot(true);
        assert!(
            !f.clarion().screenshot_ui.is_open(),
            "accepted transfer must close selection immediately"
        );
        let (w, h, bytes) = wait_png(&mut f, &path);
        assert_eq!((w, h), expected);
        assert!(
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| *pixel == [34, 68, 102, 255]),
            "selection crop or pointer-free composition changed"
        );
    }

    let client = f.add_client();
    let window = f.client(client).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.double_roundtrip(client);
    let window = f.client(client).window(&surface);
    let buffer =
        window
            .spbm
            .create_u32_rgba_buffer(u32::MAX, u32::MAX / 2, 0, u32::MAX, &window.qh, ());
    window.surface.attach(Some(&buffer), 0, 0);
    window.set_size(70, 50);
    window.ack_last_and_commit();
    f.double_roundtrip(client);
    let path = files.path("window.png");
    f.clarion_state().do_action(
        Action::ScreenshotWindow(true, false, Some(path.to_string_lossy().into_owned())),
        false,
    );
    let (w, h, bytes) = wait_png(&mut f, &path);
    assert!(w > 0 && h > 0 && bytes.len() == w as usize * h as usize * 4);
    assert!(
        bytes.as_chunks::<4>().0.iter().any(|pixel| pixel[3] != 0),
        "window screenshot is empty"
    );
}

fn privacy_epoch_cancels_transfer(native: bool) {
    let files = Files::new();
    let mut f = fixture(native);
    let path = files.path("cancelled.png");
    f.clarion_state().do_action(
        Action::ScreenshotScreen(true, false, Some(path.to_string_lossy().into_owned())),
        false,
    );
    // A lock transition invalidates snapshots even if it ends again before the
    // worker returns. The queued callback must release its permit without save.
    f.clarion().capture_scene_epoch.invalidate();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        f.state.server.dispatch();
        f.dispatch();
        let slots = &f.clarion().screenshot_tasks;
        let first = slots.acquire().unwrap();
        if let Ok(second) = slots.acquire() {
            drop(second);
            break;
        }
        drop(first);
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(!path.exists(), "invalidated desktop snapshot was published");
}

fn color_pipeline(native: bool) {
    let mut f = fixture(native);
    let (tx, rx) = async_channel::bounded(1);
    crate::input::pick_color_grab::PickColorGrab::pick_color_at_point(
        (16., 16.).into(),
        f.clarion_state(),
        tx,
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let color = loop {
        f.state.server.dispatch();
        if let Ok(color) = rx.try_recv() {
            break color.unwrap();
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    };
    assert_eq!(color.rgb, [34. / 255., 68. / 255., 102. / 255.]);
    assert!(
        rx.try_recv().is_err(),
        "color reply must be delivered exactly once"
    );
}

#[cfg(feature = "dbus")]
fn portal_pipeline(native: bool) {
    let files = Files::new();
    let mut f = fixture(native);
    let path = files.path("new-screenshot-directory/portal.png");
    assert!(!path.parent().unwrap().exists());
    f.clarion().config.borrow_mut().screenshot_path.0 = Some(path.to_string_lossy().into_owned());
    let (tx, rx) = async_channel::bounded(1);
    f.clarion_state().handle_take_screenshot(&tx, false);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        f.state.server.dispatch();
        if let Ok(ClarionToScreenshot::ScreenshotResult(result)) = rx.try_recv() {
            assert_eq!(result, Some(path.clone()));
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    let (w, h, pixels) = wait_png(&mut f, &path);
    assert_eq!((w, h), (320, 240));
    assert!(pixels
        .as_chunks::<4>()
        .0
        .iter()
        .all(|pixel| *pixel == [34, 68, 102, 255]));
    assert!(rx.try_recv().is_err());
    let cancelled = files.path("cancelled-portal.png");
    f.clarion().config.borrow_mut().screenshot_path.0 =
        Some(cancelled.to_string_lossy().into_owned());
    f.clarion_state().handle_take_screenshot(&tx, false);
    f.clarion().capture_scene_epoch.invalidate();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        f.state.server.dispatch();
        if let Ok(ClarionToScreenshot::ScreenshotResult(result)) = rx.try_recv() {
            assert_eq!(result, None);
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(!cancelled.exists());
    assert!(rx.try_recv().is_err());
}

#[test]
fn gles_color_picker_delivers_completed_pixel_once() {
    color_pipeline(false);
}
#[test]
#[ignore = "requires a real DRM render node and Vulkan"]
fn native_color_picker_delivers_completed_pixel_once() {
    color_pipeline(true);
}

#[cfg(feature = "dbus")]
#[test]
fn gles_portal_creates_default_directory_and_cancels_old_scene() {
    portal_pipeline(false);
}
#[cfg(feature = "dbus")]
#[test]
#[ignore = "requires a real DRM render node and Vulkan"]
fn native_portal_creates_default_directory_and_cancels_old_scene() {
    portal_pipeline(true);
}

#[test]
fn gles_output_selection_window_png_pipeline() {
    output_selection_window_pipeline(false);
}
#[test]
fn gles_old_scene_transfer_is_cancelled_and_releases_capacity() {
    privacy_epoch_cancels_transfer(false);
}

#[test]
#[ignore = "requires a real DRM render node and Vulkan; never acquires display master"]
fn native_output_selection_window_png_pipeline() {
    output_selection_window_pipeline(true);
}
#[test]
#[ignore = "requires a real DRM render node and Vulkan; never acquires display master"]
fn native_old_scene_transfer_is_cancelled_and_releases_capacity() {
    privacy_epoch_cancels_transfer(true);
}
