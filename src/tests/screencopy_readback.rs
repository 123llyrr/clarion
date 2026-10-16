//! A real private Wayland client verifies protocol completion and SHM ownership.
//! Expected pixels come from an opaque known desktop color, not the readback
//! implementation. The pool also has a prefix, row padding and a tail sentinel.

use std::fs::{File, OpenOptions};
use std::os::fd::AsFd;
use std::os::unix::fs::FileExt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clarion_config::{Color, Config};
use smithay::backend::allocator::Fourcc;
use smithay::reexports::wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};

use super::client::{ClientId, State as ClientState};
use super::Fixture;

const OFFSET: usize = 7;
const PADDING: usize = 12;
const SENTINEL: u8 = 199;
const RGBA: [u8; 4] = [17, 73, 157, 255];
type FrameEvents = Arc<Mutex<CaptureEvents>>;

#[derive(Default)]
struct CaptureEvents {
    format: Option<(wl_shm::Format, u32, u32, u32)>,
    buffer_done: bool,
    ready: usize,
    failed: usize,
    damage: Vec<(u32, u32, u32, u32)>,
    y_invert: bool,
    pool: Option<(Arc<File>, usize)>,
    // Read the pool inside the Ready event dispatch, so correctness cannot
    // depend on a write performed sometime after the protocol notification.
    pixels_at_ready: Vec<Vec<u8>>,
}

impl Dispatch<ZwlrScreencopyFrameV1, FrameEvents> for ClientState {
    fn event(
        _state: &mut Self,
        _proxy: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        data: &FrameEvents,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let mut events = data.lock().unwrap();
        match event {
            zwlr_screencopy_frame_v1::Event::Buffer {
                format,
                width,
                height,
                stride,
            } => {
                let WEnum::Value(format) = format else {
                    panic!("unknown SHM capture format")
                };
                events.format = Some((format, width, height, stride));
            }
            zwlr_screencopy_frame_v1::Event::BufferDone => events.buffer_done = true,
            zwlr_screencopy_frame_v1::Event::Damage {
                x,
                y,
                width,
                height,
            } => {
                events.damage.push((x, y, width, height));
            }
            zwlr_screencopy_frame_v1::Event::Flags { flags } => {
                if let WEnum::Value(flags) = flags {
                    events.y_invert = flags.contains(zwlr_screencopy_frame_v1::Flags::YInvert);
                }
            }
            zwlr_screencopy_frame_v1::Event::Ready { .. } => {
                events.ready += 1;
                let (file, size) = events.pool.as_ref().expect("capture pool not installed");
                let mut bytes = vec![0; *size];
                file.read_exact_at(&mut bytes, 0).unwrap();
                events.pixels_at_ready.push(bytes);
            }
            zwlr_screencopy_frame_v1::Event::Failed => events.failed += 1,
            zwlr_screencopy_frame_v1::Event::LinuxDmabuf { .. } => (),
            _ => unreachable!(),
        }
    }
}

wayland_client::delegate_noop!(ClientState: ignore WlShm);
wayland_client::delegate_noop!(ClientState: ignore WlShmPool);
wayland_client::delegate_noop!(ClientState: ignore ZwlrScreencopyManagerV1);

struct Capture {
    frame: ZwlrScreencopyFrameV1,
    events: FrameEvents,
    buffer: WlBuffer,
    file: Arc<File>,
    width: usize,
    height: usize,
    stride: usize,
    len: usize,
}

impl Capture {
    fn bytes(&self) -> Vec<u8> {
        let mut bytes = vec![0; self.len];
        self.file.read_exact_at(&mut bytes, 0).unwrap();
        bytes
    }

    fn expected(&self) -> Vec<u8> {
        let mut expected = vec![SENTINEL; self.len];
        for row in 0..self.height {
            for x in 0..self.width {
                let start = OFFSET + row * self.stride + x * 4;
                expected[start..start + 4].copy_from_slice(&[RGBA[2], RGBA[1], RGBA[0], 255]);
            }
        }
        expected
    }

    fn assert_ready(&self) {
        let events = self.events.lock().unwrap();
        assert_eq!(events.ready, 1, "capture must complete exactly once");
        assert_eq!(events.failed, 0);
        assert!(
            !events.y_invert,
            "composed SHM capture rows must remain Y-down"
        );
        assert_eq!(events.pixels_at_ready, [self.expected()]);
        assert_eq!(self.bytes(), self.expected());
    }
}

fn setup(native: bool) -> (Fixture, ClientId, ZwlrScreencopyManagerV1, WlShm) {
    let mut config = Config::default();
    config.animations.off = true;
    config.layout.background_color = Color::from_rgba8_unpremul(RGBA[0], RGBA[1], RGBA[2], RGBA[3]);
    let mut f = Fixture::with_config(config);
    if native {
        use crate::backend::headless::HeadlessRenderer;
        use crate::render_helpers::vulkan::{native::NativeRenderer, VulkanRenderer};
        let path =
            std::env::var_os("CLARION_VULKAN_TEST_NODE").expect("set CLARION_VULKAN_TEST_NODE");
        let node = smithay::backend::drm::DrmNode::from_path(&path).unwrap();
        let mut renderer = VulkanRenderer::new();
        renderer.init_from_drm_node(Some(&node)).unwrap();
        eprintln!(
            "SCREENCOPY HARDWARE: {} ({path:?})",
            renderer.info.device_name
        );
        let mut renderer = NativeRenderer::new(renderer).unwrap();
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
    f.add_output(1, (32, 24));
    let id = f.add_client();
    f.double_roundtrip(id);
    let c = f.client(id);
    let registry = c.display.get_registry(&c.qh, ());
    let manager_global = c
        .state
        .globals
        .iter()
        .find(|g| g.interface == ZwlrScreencopyManagerV1::interface().name)
        .unwrap();
    let manager = registry.bind(
        manager_global.name,
        manager_global.version.min(3),
        &c.qh,
        (),
    );
    let shm_global = c
        .state
        .globals
        .iter()
        .find(|g| g.interface == WlShm::interface().name)
        .unwrap();
    let shm = registry.bind(shm_global.name, 1, &c.qh, ());
    f.double_roundtrip(id);
    (f, id, manager, shm)
}

fn capture(
    f: &mut Fixture,
    id: ClientId,
    manager: &ZwlrScreencopyManagerV1,
    shm: &WlShm,
) -> Capture {
    let events = FrameEvents::default();
    let c = f.client(id);
    let output = c.state.outputs.keys().next().unwrap().clone();
    let frame = manager.capture_output_region(0, &output, 3, 4, 8, 6, &c.qh, events.clone());
    f.double_roundtrip(id);
    let (format, width, height, tight_stride) = events.lock().unwrap().format.unwrap();
    assert_eq!(format, wl_shm::Format::Xrgb8888);
    assert_eq!((width, height, tight_stride), (8, 6, 32));
    assert!(events.lock().unwrap().buffer_done);
    let width = width as usize;
    let height = height as usize;
    let stride = tight_stride as usize + PADDING;
    let len = OFFSET + stride * height + 11;
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let path = std::env::temp_dir().join(format!(
        "clarion-screencopy-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let file = Arc::new(
        OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap(),
    );
    std::fs::remove_file(path).unwrap();
    file.set_len(len as u64).unwrap();
    file.write_all_at(&vec![SENTINEL; len], 0).unwrap();
    events.lock().unwrap().pool = Some((file.clone(), len));
    let c = f.client(id);
    let pool = shm.create_pool(file.as_fd(), len as i32, &c.qh, ());
    let buffer = pool.create_buffer(
        OFFSET as i32,
        width as i32,
        height as i32,
        stride as i32,
        format,
        &c.qh,
        (),
    );
    pool.destroy();
    f.double_roundtrip(id);
    let capture = Capture {
        frame,
        events,
        buffer,
        file,
        width,
        height,
        stride,
        len,
    };
    assert_eq!(
        capture.bytes(),
        vec![SENTINEL; len],
        "advertising a frame must not modify its pool"
    );
    capture
}

fn pump_until(f: &mut Fixture, id: ClientId, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(
            Instant::now() < deadline,
            "screencopy did not finish within five seconds"
        );
        f.state.server.dispatch();
        f.client(id).dispatch();
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn drive_damage(f: &mut Fixture) {
    let output = f.clarion_output(1);
    let state = f.clarion_state();
    state.clarion.update_render_elements(Some(&output));
    // The headless GLES fixture intentionally renders frame callbacks only.
    // Drive the same capture redraw entry used by real device backends.
    state
        .backend
        .with_renderer(|renderer| match renderer {
            crate::render_helpers::renderer::AnyRenderer::Gles(gles) => state
                .clarion
                .render_for_screencopy_with_damage(gles, &output),
            crate::render_helpers::renderer::AnyRenderer::Native(native) => state
                .clarion
                .render_for_screencopy_with_damage(native, &output),
        })
        .unwrap();
}

fn success_paths(native: bool) {
    for with_damage in [false, true] {
        let (mut f, id, manager, shm) = setup(native);
        let capture = capture(&mut f, id, &manager, &shm);
        if with_damage {
            capture.frame.copy_with_damage(&capture.buffer);
        } else {
            capture.frame.copy(&capture.buffer);
        }
        f.client(id).connection.flush().unwrap();
        if with_damage {
            f.double_roundtrip(id);
            assert_eq!(capture.events.lock().unwrap().ready, 0);
            assert_eq!(capture.bytes(), vec![SENTINEL; capture.len]);
            drive_damage(&mut f);
        }
        pump_until(&mut f, id, || {
            let events = capture.events.lock().unwrap();
            events.ready + events.failed > 0
        });
        capture.assert_ready();
        if with_damage {
            assert!(!capture.events.lock().unwrap().damage.is_empty());
        }
    }
}

fn lifecycle_paths(native: bool) {
    // Cancellation is processed in the same client request batch as Copy,
    // before any async completion callback can access its protocol objects.
    let (mut f, id, manager, shm) = setup(native);
    let cancelled = capture(&mut f, id, &manager, &shm);
    cancelled.frame.copy(&cancelled.buffer);
    cancelled.frame.destroy();
    f.client(id).connection.flush().unwrap();
    for _ in 0..30 {
        f.state.server.dispatch();
        f.client(id).dispatch();
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(cancelled.events.lock().unwrap().ready, 0);
    assert_eq!(
        cancelled.bytes(),
        vec![SENTINEL; cancelled.len],
        "cancelled frame must not later modify reusable SHM"
    );

    // Removing an output with a live readback must fail rather than publish a
    // stale successful frame. One server dispatch submits work, the next polls.
    let removed = capture(&mut f, id, &manager, &shm);
    removed.frame.copy(&removed.buffer);
    f.client(id).connection.flush().unwrap();
    f.state.server.dispatch();
    let output = f.clarion_output(1);
    f.clarion().remove_output(&output);
    pump_until(&mut f, id, || removed.events.lock().unwrap().failed > 0);
    assert_eq!(removed.events.lock().unwrap().ready, 0);
    assert_eq!(removed.bytes(), vec![SENTINEL; removed.len]);

    // Three copies in one request batch must not bypass the two-live-job cap.
    let (mut f, id, manager, shm) = setup(native);
    let captures: Vec<_> = (0..3)
        .map(|_| capture(&mut f, id, &manager, &shm))
        .collect();
    for capture in &captures {
        capture.frame.copy(&capture.buffer);
    }
    f.client(id).connection.flush().unwrap();
    pump_until(&mut f, id, || {
        captures.iter().all(|c| {
            let e = c.events.lock().unwrap();
            e.ready + e.failed > 0
        })
    });
    let ready: usize = captures
        .iter()
        .map(|c| c.events.lock().unwrap().ready)
        .sum();
    let failed: usize = captures
        .iter()
        .map(|c| c.events.lock().unwrap().failed)
        .sum();
    assert_eq!((ready, failed), (2, 1));
    for capture in &captures {
        if capture.events.lock().unwrap().ready > 0 {
            capture.assert_ready();
        } else {
            assert_eq!(capture.bytes(), vec![SENTINEL; capture.len]);
        }
    }
}

#[test]
fn gles_real_client_cropped_shm_copy_and_copy_with_damage_complete_after_pixels() {
    success_paths(false);
}

#[test]
fn gles_real_client_cancel_output_removal_and_job_limit_never_publish_stale_ready() {
    lifecycle_paths(false);
}

#[test]
#[ignore = "requires CLARION_VULKAN_TEST_NODE selecting a physical Vulkan render node"]
fn native_real_client_shm_screencopy_preserves_pixels_lifecycle_and_bounds() {
    success_paths(true);
    lifecycle_paths(true);
}
