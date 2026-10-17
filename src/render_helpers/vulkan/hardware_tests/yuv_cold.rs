//! Measure native NV12 import and the raw DMA-BUF compatibility render path.
//! An unregistered external texture has no Wayland buffer synchronization owner;
//! NativeFrame::finish may therefore wait for the GPU here. This is renderer
//! import-cache cold, with target pipelines prepared outside measured rendering;
//! the test does not clear the driver's persistent shader cache. A separate gate
//! case checks cold target admission without submitting the incomplete scene.
mod gpu_producer;

use std::os::fd::{AsFd, BorrowedFd};
use std::time::{Duration, Instant};

use smithay::backend::allocator::dmabuf::{Dmabuf, DmabufFlags, DmabufSyncFlags};
use smithay::backend::drm::NodeType;
use smithay::backend::renderer::ImportDma;
use smithay::reexports::drm;

use super::*;

struct Card(std::fs::File);
impl AsFd for Card {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}
impl drm::Device for Card {}
impl drm::control::Device for Card {}

fn primary_node() -> Option<std::path::PathBuf> {
    let selected = DrmNode::from_path(std::env::var_os("CLARION_VULKAN_TEST_NODE")?).ok()?;
    let primary = selected.node_with_type(NodeType::Primary)?.ok()?;
    let device = |node: &DrmNode| {
        std::fs::canonicalize(format!(
            "/sys/dev/char/{}:{}/device",
            node.major(),
            node.minor()
        ))
        .ok()
    };
    (device(&selected)? == device(&primary)?)
        .then(|| primary.dev_path())
        .flatten()
}

fn thread_cpu() -> Option<Duration> {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: clock_gettime writes only the owned timespec.
    if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) } != 0
        || time.tv_sec < 0
        || !(0..1_000_000_000).contains(&time.tv_nsec)
    {
        return None;
    }
    Some(Duration::new(time.tv_sec as u64, time.tv_nsec as u32))
}

fn measure<R>(phase: &str, iteration: usize, operation: &str, action: impl FnOnce() -> R) -> R {
    let wall_start = Instant::now();
    let cpu_start = thread_cpu();
    let result = action();
    let wall = wall_start.elapsed();
    let cpu = thread_cpu()
        .zip(cpu_start)
        .and_then(|(end, start)| end.checked_sub(start));
    eprintln!(
        "YUV_OWNER_WORK {}",
        serde_json::json!({
            "phase": phase, "iteration": iteration, "operation": operation,
            "wall_ms": wall.as_secs_f64() * 1000.,
            "thread_cpu_ms": cpu.map(|value| value.as_secs_f64() * 1000.),
            "scope": "calling_thread_only",
            "import_cache_cold": iteration == 0 && operation == "import_dmabuf",
            "yuv_target_pipeline_prepared": operation != "import_dmabuf",
            "finish_may_wait_for_unregistered_external_texture": operation == "bind_draw_finish_raw_dmabuf_compatibility",
            "driver_cache_cleared": false
        })
    );
    result
}

fn unavailable(reason: impl std::fmt::Display) {
    eprintln!(
        "YUV_COLD_RESULT {}",
        serde_json::json!({
            "status": "UNAVAILABLE", "reason": reason.to_string(),
            "node": std::env::var("CLARION_VULKAN_TEST_NODE").ok()
        })
    );
}

#[test]
#[ignore = "requires selected DRM GPU with sampled linear NV12 and a same-device CPU producer"]
fn native_nv12_cold_import_and_first_submit_owner_work() {
    check_native_nv12(24, false, false);
}

#[test]
#[ignore = "requires selected DRM GPU with sampled linear NV12 and a padded same-device producer"]
fn native_nv12_padded_backing_import_and_compatibility_pixels() {
    // A separately named case: NVIDIA requests 128 KiB for this 16x16 view.
    // Enlarging storage does not alter any visible row, pitch or UV offset and
    // must not turn the original 4 KiB incompatibility into a pixel PASS.
    check_native_nv12(8192, false, false);
}

#[test]
#[ignore = "requires NVIDIA DRM GPU whose Vulkan NV12 view exceeds the valid 4 KiB CPU producer"]
fn native_nv12_undersized_backing_reports_exact_driver_requirement() {
    check_native_nv12(24, true, false);
}

#[test]
#[ignore = "requires selected DRM GPU with sampled linear NV12 and a padded same-device producer"]
fn native_nv12_cold_target_preparation_defers_without_owner_wait_or_overwrite() {
    check_native_nv12(8192, false, true);
}

fn check_native_nv12(storage_height: u32, expect_undersized: bool, gate_preparation: bool) {
    use drm::buffer::Buffer as _;
    use drm::control::Device as _;
    let (vk, mut oracle, mut allocator) = setup();
    let mut renderer = native::NativeRenderer::new(vk).unwrap();
    if expect_undersized {
        let ctx = renderer.vulkan().ctx.as_ref().unwrap();
        let vendor = unsafe {
            ctx.instance
                .get_physical_device_properties(ctx.physical_device)
        }
        .vendor_id;
        if vendor != 0x10de {
            unavailable(
                "this negative fixture targets the verified NVIDIA 128 KiB image requirement",
            );
            return;
        }
    }
    let nv12 = smithay::backend::allocator::Format {
        code: Fourcc::Nv12,
        modifier: Modifier::Linear,
    };
    if !renderer.dmabuf_formats().contains(&nv12) {
        unavailable("selected Vulkan driver does not advertise sampled linear NV12");
        return;
    }
    let Some(primary) = primary_node() else {
        unavailable("no same-device primary DRM node for the selected render node");
        return;
    };
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&primary)
    {
        Ok(file) => file,
        Err(error) => {
            unavailable(format!("open {}: {error}", primary.display()));
            return;
        }
    };
    let card = Card(file);
    // The independent producer lives on the selected device, avoiding a
    // cross-GPU PRIME failure being confused with a cold pipeline regression.
    let mut dumb = match card.create_dumb_buffer((16, storage_height), Fourcc::Nv12, 8) {
        Ok(buffer) => buffer,
        Err(error) => {
            unavailable(format!("same-device linear CPU producer: {error}"));
            return;
        }
    };
    let pitch = dumb.pitch();
    let fd = card
        .buffer_to_prime_fd(dumb.handle(), drm::CLOEXEC | drm::RDWR)
        .expect("export NV12 producer DMA-BUF");
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    use std::os::fd::AsRawFd as _;
    assert_eq!(unsafe { libc::fstat(fd.as_raw_fd(), stat.as_mut_ptr()) }, 0);
    let backing_bytes = unsafe { stat.assume_init() }.st_size;
    eprintln!(
        "YUV_PRODUCER_METADATA {}",
        serde_json::json!({
            "visible_size": [16, 16], "dumb_storage_size": [16, storage_height],
            "pitch": pitch, "uv_offset": 16 * pitch, "backing_bytes": backing_bytes,
            "padded_case": storage_height != 24
        })
    );
    let mut builder = Dmabuf::builder(
        (16, 16),
        Fourcc::Nv12,
        Modifier::Linear,
        DmabufFlags::empty(),
    );
    builder.add_plane(fd.try_clone().unwrap(), 0, pitch);
    builder.add_plane(fd, 16 * pitch, pitch);
    let source = builder.build().unwrap();
    // CPU access is bracketed on the allocation's PRIME FD so noncoherent
    // devices see the producer's bytes before Vulkan samples either plane.
    // Both planes share this one allocation, so one sync pair covers both.
    source
        .sync_plane(0, DmabufSyncFlags::START | DmabufSyncFlags::WRITE)
        .expect("begin CPU write to NV12 producer");
    {
        let mut mapped = card
            .map_dumb_buffer(&mut dumb)
            .expect("map NV12 CPU producer");
        for row in 0..16usize {
            for x in 0..16usize {
                mapped[row * pitch as usize + x] = if x < 8 { 81 } else { 41 };
            }
        }
        for row in 0..8usize {
            for x in (0..16usize).step_by(2) {
                let (u, v) = if x < 8 { (90, 240) } else { (240, 110) };
                mapped[(16 + row) * pitch as usize + x] = u;
                mapped[(16 + row) * pitch as usize + x + 1] = v;
            }
        }
    }
    source
        .sync_plane(0, DmabufSyncFlags::END | DmabufSyncFlags::WRITE)
        .expect("publish CPU write to NV12 producer");
    card.destroy_dumb_buffer(dumb).unwrap();
    assert_eq!(source.num_planes(), 2);

    if expect_undersized {
        let result = renderer.vulkan().import_dmabuf(&source);
        let Err(super::super::dmabuf::VulkanImportError::AllocationTooSmall {
            available,
            required,
        }) = result
        else {
            panic!("undersized NVIDIA backing must report its precise allocation constraint");
        };
        assert_eq!(backing_bytes, 4096);
        assert_eq!(available, backing_bytes as u64);
        assert!(required > available);
        renderer.begin_scene();
        let error = match ImportDma::import_dmabuf(&mut renderer, &source, None) {
            Err(error) => error,
            Ok(_) => panic!("native DMA importer must preserve the allocation rejection"),
        };
        assert!(!error.is_busy());
        assert!(!native::NativeError::is_backpressure(&error));
        let native::NativeError::Other(message) = error else {
            panic!("ordinary allocation rejection cannot be GPU admission backpressure");
        };
        assert!(message.contains(&available.to_string()));
        assert!(message.contains(&required.to_string()));
        assert!(!message.to_ascii_uppercase().contains("DEVICE_LOST"));
        assert!(!renderer.scene_was_deferred());

        // Rejection must leave an ordinary scene usable on this same renderer.
        renderer
            .prepare_output_format_for_test(Fourcc::Argb8888)
            .unwrap();
        let mut rgb = renderer
            .create_buffer(Fourcc::Argb8888, (16, 16).into())
            .unwrap();
        let mut bound = renderer.bind(&mut rgb).unwrap();
        let size = (16, 16).into();
        let all = Rectangle::from_size(size);
        let mut frame = renderer
            .render(&mut bound, size, Transform::Normal)
            .unwrap();
        frame.clear([1., 0., 0., 1.].into(), &[all]).unwrap();
        frame.finish().unwrap().wait().unwrap();
        let mapping = renderer
            .copy_texture(
                &rgb,
                Rectangle::from_size((16, 16).into()),
                Fourcc::Abgr8888,
            )
            .unwrap();
        assert!(renderer
            .map_texture(&mapping)
            .unwrap()
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| *pixel == [255, 0, 0, 255]));
        eprintln!(
            "YUV_BACKING_REJECTION_RESULT {}",
            serde_json::json!({
                "status": "PASS", "available": available, "required": required,
                "visible_size": [16, 16], "dumb_storage_size": [16, storage_height],
                "pitch": pitch, "uv_offset": 16 * pitch,
                "pixel_verified": false,
                "ordinary_rgb_scene_after_rejection_verified": true,
                "scope": "exact controlled backing-size rejection; original native pixel interoperability remains unsupported for this allocation"
            })
        );
        return;
    }

    // Finish all ordinary RGB preparation before measuring the cold YUV path.
    // The synchronous test wait is outside the measured production calls.
    renderer
        .prepare_output_format_for_test(Fourcc::Argb8888)
        .unwrap();
    let mut target = allocator
        .create_buffer(
            16,
            16,
            Fourcc::Argb8888,
            &modifiers(renderer.vulkan(), &oracle, Fourcc::Argb8888),
        )
        .unwrap()
        .export()
        .unwrap();
    let size = (16, 16).into();
    let all = Rectangle::from_size(size);
    {
        let mut bound = renderer.bind(&mut target).unwrap();
        let mut frame = renderer
            .render(&mut bound, size, Transform::Normal)
            .unwrap();
        frame.clear(Color32F::BLACK, &[all]).unwrap();
        frame.finish().unwrap().wait().unwrap();
    }
    if gate_preparation {
        let previous = pixels(&mut oracle, &mut target);
        assert!(previous
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| *pixel == [0, 0, 0, 255]));
        // This is the same fixed bounded worker used by real YUV target jobs.
        // The helper confirms the gate has been entered, so the imported cold
        // YUV variant cannot complete while held. It releases during unwinding.
        let gate = renderer.gate_gpu_preparation_for_test().unwrap();
        let texture = ImportDma::import_dmabuf(&mut renderer, &source, None).unwrap();
        renderer.begin_scene();
        let before_waits = super::super::owner_fence_wait_calls_for_test();
        let started = Instant::now();
        let result = {
            let mut bound = renderer.bind(&mut target).unwrap();
            let mut frame = renderer
                .render(&mut bound, size, Transform::Normal)
                .unwrap();
            frame.clear([0., 1., 0., 1.].into(), &[all]).unwrap();
            frame
                .render_texture_from_to(
                    &texture,
                    Rectangle::from_size((16., 16.).into()),
                    all,
                    &[all],
                    &[],
                    Transform::Normal,
                    1.,
                )
                .unwrap();
            frame.finish()
        };
        let admission_wall = started.elapsed();
        assert!(matches!(
            result,
            Err(native::NativeError::Busy(native::NativeBusy::YuvPipelines))
        ));
        assert!(
            admission_wall < Duration::from_secs(1),
            "cold YUV admission waited for its gated worker: {admission_wall:?}"
        );
        assert_eq!(
            super::super::owner_fence_wait_calls_for_test(),
            before_waits,
            "an unprepared YUV scene cannot wait for a GPU fence"
        );
        assert!(renderer.scene_was_deferred());
        assert_eq!(
            pixels(&mut oracle, &mut target),
            previous,
            "unsubmitted YUV scene overwrote the previous target pixels"
        );

        // Deferral belongs to the sampled YUV scene. An already prepared RGB
        // scene on this renderer must remain usable while the worker is gated.
        renderer.begin_scene();
        assert!(!renderer.scene_was_deferred());
        assert!(!renderer.take_backpressure());
        let mut rgb = renderer
            .create_buffer(Fourcc::Argb8888, (16, 16).into())
            .unwrap();
        {
            let mut bound = renderer.bind(&mut rgb).unwrap();
            let mut frame = renderer
                .render(&mut bound, size, Transform::Normal)
                .unwrap();
            frame.clear([0., 1., 0., 1.].into(), &[all]).unwrap();
            frame.finish().unwrap().wait().unwrap();
        }
        assert!(!renderer.scene_was_deferred());
        let mapping = renderer
            .copy_texture(
                &rgb,
                Rectangle::from_size((16, 16).into()),
                Fourcc::Abgr8888,
            )
            .unwrap();
        assert!(renderer
            .map_texture(&mapping)
            .unwrap()
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| *pixel == [0, 255, 0, 255]));
        drop(gate);
        // This waits only in fixture setup, applying actual worker completion
        // events on this owner. Production admission above remains nonblocking.
        renderer
            .prepare_yuv_texture_for_test(&texture, Fourcc::Argb8888)
            .unwrap();
        renderer.begin_scene();
        assert!(!renderer.scene_was_deferred());
        let mut bound = renderer.bind(&mut target).unwrap();
        let mut frame = renderer
            .render(&mut bound, size, Transform::Normal)
            .unwrap();
        frame.clear(Color32F::BLACK, &[all]).unwrap();
        frame
            .render_texture_from_to(
                &texture,
                Rectangle::from_size((16., 16.).into()),
                all,
                &[all],
                &[],
                Transform::Normal,
                1.,
            )
            .unwrap();
        frame.finish().unwrap().wait().unwrap();
        let bytes = pixels(&mut oracle, &mut target);
        assert_nv12_pixels(&bytes, 0);
        eprintln!(
            "YUV_GATED_PREPARATION_RESULT {}",
            serde_json::json!({
                "status": "PASS", "node": std::env::var("CLARION_VULKAN_TEST_NODE").ok(),
                "cold_admission_wall_ms": admission_wall.as_secs_f64() * 1000.,
                "owner_fence_waits_during_cold_admission": 0,
                "previous_target_pixels_preserved": true,
                "ordinary_rgb_scene_while_preparation_gated_verified": true,
                "actual_owner_completion_applied": true, "pixel_checks": 1,
                "normal_wayland_submit_verified": false,
                "finish_may_wait_for_unregistered_external_texture": true,
                "scope": "cold YUV target admission only is nonblocking; prepared raw DMA-BUF compatibility render may synchronously finish"
            })
        );
        return;
    }
    let owner = std::thread::current().id();
    for iteration in 0..9 {
        assert_eq!(std::thread::current().id(), owner);
        let phase = if iteration == 0 { "cold" } else { "warm" };
        let texture = measure(phase, iteration, "import_dmabuf", || {
            ImportDma::import_dmabuf(&mut renderer, &source, None).expect("advertised NV12 import")
        });
        assert_eq!(
            smithay::backend::renderer::Texture::format(&texture),
            Some(Fourcc::Nv12)
        );
        // Import remains cold on iteration zero. Target pipeline creation is
        // performed by the real device worker and waited outside measured
        // compatibility drawing, so that scope cannot include a cold pipeline.
        renderer
            .prepare_yuv_texture_for_test(&texture, Fourcc::Argb8888)
            .unwrap();
        let sync = measure(
            phase,
            iteration,
            "bind_draw_finish_raw_dmabuf_compatibility",
            || {
                let mut bound = renderer.bind(&mut target).unwrap();
                let mut frame = renderer
                    .render(&mut bound, size, Transform::Normal)
                    .unwrap();
                frame.clear(Color32F::BLACK, &[all]).unwrap();
                frame
                    .render_texture_from_to(
                        &texture,
                        Rectangle::from_size((16., 16.).into()),
                        all,
                        &[all],
                        &[],
                        Transform::Normal,
                        1.,
                    )
                    .unwrap();
                frame.finish().unwrap()
            },
        );
        measure(phase, iteration, "returned_sync_diagnostic_wait", || {
            sync.wait().unwrap()
        });
        let bytes = pixels(&mut oracle, &mut target);
        assert_nv12_pixels(&bytes, iteration);
    }
    eprintln!(
        "YUV_COLD_RESULT {}",
        serde_json::json!({
            "status": "PASS", "pixel_checks": 9, "cold_iterations": 1, "warm_iterations": 8,
            "node": std::env::var("CLARION_VULKAN_TEST_NODE").ok(),
            "producer": primary, "renderer_import_cache_cold": true, "driver_cache_cleared": false,
        "dumb_storage_size": [16, storage_height], "pitch": pitch,
        "uv_offset": 16 * pitch, "backing_bytes": backing_bytes,
        "normal_wayland_submit_verified": false,
        "import_cache_cold_on_iteration_zero": true,
        "yuv_target_pipeline_prepared_before_measured_draw": true,
        "finish_may_wait_for_unregistered_external_texture": true,
        "scope": "native import and raw DMA-BUF compatibility render, including any finish GPU wait; subsequent sync wait and GLES readback reported separately"
        })
    );
}

fn assert_nv12_pixels(bytes: &[u8], iteration: usize) {
    let red = &bytes[(4 * 16 + 3) * 4..(4 * 16 + 3) * 4 + 4];
    let blue = &bytes[(4 * 16 + 12) * 4..(4 * 16 + 12) * 4 + 4];
    assert!(
        red[0] > 170 && red[1] < 100 && red[2] < 100 && red[3] == 255,
        "NV12 red sample iteration {iteration}: {red:?}"
    );
    assert!(
        blue[2] > 150 && blue[0] < 110 && blue[1] < 110 && blue[3] == 255,
        "NV12 blue sample iteration {iteration}: {blue:?}"
    );
}
