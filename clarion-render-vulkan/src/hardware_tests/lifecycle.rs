//! Default-libtest process exit must not race Vulkan worker or native TLS Drop.
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{mpsc, Arc, OnceLock};
use std::time::{Duration, Instant};

use super::super::device::VulkanContext;
use super::super::lifecycle;
use super::super::pipeline::VulkanPipelineManager;

/// Keep deliberately quarantined GPU submissions out of the parent test
/// process. Only these exact timeout fixtures expect the safe exit 124;
/// their original resource/pixel assertions still run in the selected GPU child.
pub(crate) fn run_quarantine_fixture_in_child() -> bool {
    const CASES: [&str; 5] = [
        "render_helpers::vulkan::frame::tests::fence_timeout_keeps_completion_owners_alive",
        "render_helpers::vulkan::hardware_tests::deferred_shm_upload_timeout_keeps_resources_alive",
        "render_helpers::vulkan::hardware_tests::fence_timeout_keeps_submitted_frame_resources_alive",
        "render_helpers::vulkan::hardware_tests::fence_timeout_keeps_transfer_texture_alive",
        "render_helpers::vulkan::hardware_tests::timeout_preserves_submitted_layouts_for_readback_and_retry",
    ];
    let thread = std::thread::current();
    let name = thread.name().expect("named libtest fixture");
    assert!(
        CASES.contains(&name),
        "only exact quarantine cases may expect exit 124"
    );
    if let Ok(child) = std::env::var("CLARION_QUARANTINE_TEST_CHILD") {
        assert_eq!(
            child, name,
            "quarantine child must run exactly its requested test"
        );
        return false;
    }
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let directory =
        std::env::temp_dir().join(format!("clarion-quarantine-{}-{id}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            name,
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("CLARION_QUARANTINE_TEST_CHILD", name)
        .stdout(std::fs::File::create(directory.join("stdout.log")).unwrap())
        .stderr(std::fs::File::create(directory.join("stderr.log")).unwrap())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(45);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("quarantine child exceeded bound: {}", directory.display());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let stdout = std::fs::read_to_string(directory.join("stdout.log")).unwrap();
    let stderr = std::fs::read_to_string(directory.join("stderr.log")).unwrap();
    for line in stdout.lines().chain(stderr.lines()) {
        // Keep device and validation diagnostics visible to the outer checker,
        // while prefixed child receipts cannot masquerade as the parent result.
        eprintln!("QUARANTINE_CHILD {name}: {line}");
    }
    assert_eq!(
        status.code(),
        Some(124),
        "only a successful quarantine child may exit 124"
    );
    assert!(
        stdout
            .lines()
            .any(|line| line.starts_with("test result: ok. 1 passed; 0 failed; 0 ignored;")),
        "quarantine child must complete its real GPU assertions"
    );
    assert!(stdout.contains("HARDWARE:") || stderr.contains("HARDWARE:"));
    assert!(stderr.contains("skipping unsafe loader teardown (exit 124)"));
    let diagnostics = format!("{stdout}\n{stderr}").to_ascii_lowercase();
    for bad in [
        "validation error",
        "validation warning",
        "vuid-",
        "sync-hazard",
        "panicked at",
    ] {
        assert!(
            !diagnostics.contains(bad),
            "quarantine child diagnostic: {bad}"
        );
    }
    eprintln!(
        "QUARANTINE_EVIDENCE test={name} path={}",
        directory.display()
    );
    true
}

#[test]
#[ignore = "requires real DRM render node, Vulkan timestamps and native Wayland server"]
fn native_event_loop_teardown_releases_device_before_loop_drop() {
    use std::cell::Cell;
    use std::os::unix::net::UnixStream;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use futures_util::io::AsyncReadExt;
    use smithay::backend::allocator::Fourcc;
    use smithay::backend::renderer::Offscreen;
    use smithay::reexports::calloop::generic::Generic;
    use smithay::reexports::calloop::{EventLoop, Interest, Mode, PostAction};
    use smithay::reexports::wayland_server::backend::ClientData;
    use smithay::reexports::wayland_server::Display;

    use crate::platform::event_sources::ScopedSource;
    use crate::render_helpers::vulkan::native::NativeRenderer;
    use crate::render_helpers::vulkan::{VulkanRenderer, VulkanTexture};

    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    struct ClientCache {
        texture: Option<VulkanTexture>,
        dropped: Arc<AtomicBool>,
    }
    impl ClientData for ClientCache {}
    impl Drop for ClientCache {
        fn drop(&mut self) {
            drop(self.texture.take());
            self.dropped.store(true, Ordering::Release);
        }
    }

    let node = smithay::backend::drm::DrmNode::from_path(
        std::env::var_os("CLARION_VULKAN_TEST_NODE").expect("set CLARION_VULKAN_TEST_NODE"),
    )
    .unwrap();
    let context = VulkanContext::init(Some(&node)).unwrap();
    // Ensure the query-pool owner responsible for the original leak is active.
    let queues = unsafe {
        context
            .instance
            .get_physical_device_queue_family_properties(context.physical_device)
    };
    assert!(queues[context.graphics_queue_family as usize].timestamp_valid_bits > 0);
    assert!(context.physical_device_properties.limits.timestamp_period > 0.);
    let mut renderer = VulkanRenderer::new();
    renderer.init_with_context(context.clone()).unwrap();
    let mut native = NativeRenderer::new(renderer).unwrap();
    let texture = native
        .create_buffer(Fourcc::Abgr8888, (2, 2).into())
        .unwrap();
    let (mut fixture, surface_destroyed, surface_cache_dropped) =
        populated_smithay_surface_cache(&mut native);
    let mut event_loop = EventLoop::<()>::try_new().unwrap();
    let handle = event_loop.handle();
    native.install_completion_wakeup(&handle).unwrap();

    // A real connected Wayland client retains a real Vulkan allocation. Its
    // Display source must be explicitly retired even if another source cycles.
    let display = Display::<()>::new().unwrap();
    let (wayland_server, wayland_client) = UnixStream::pair().unwrap();
    let client_dropped = Arc::new(AtomicBool::new(false));
    let client = display
        .handle()
        .insert_client(
            wayland_server,
            Arc::new(ClientCache {
                texture: Some(texture),
                dropped: client_dropped.clone(),
            }),
        )
        .unwrap();
    drop(client);
    let display_source = ScopedSource::insert(
        &handle,
        Generic::new(display, Interest::READ, Mode::Level),
        |_, display, state| {
            // SAFETY: dispatch does not move or drop the registered Display.
            unsafe { display.get_mut().dispatch_clients(state).unwrap() };
            Ok(PostAction::Continue)
        },
    )
    .unwrap();

    let (executor, scheduler) = calloop::futures::executor().unwrap();
    let executor_source = ScopedSource::insert(&handle, executor, |_, _, _| ()).unwrap();
    let (ipc_server, ipc_client) = UnixStream::pair().unwrap();
    let mut ipc = handle.adapt_io(ipc_server).unwrap();
    let pending = Rc::new(Cell::new(false));
    let polled = pending.clone();
    let future_dropped = Arc::new(AtomicBool::new(false));
    let drop_marker = Dropped(future_dropped.clone());
    let client_loop = handle.clone();
    scheduler
        .schedule(async move {
            let _drop_marker = drop_marker;
            polled.set(true);
            let _ = ipc.read(&mut [0]).await;
            drop(client_loop);
        })
        .unwrap();
    event_loop.dispatch(Duration::ZERO, &mut ()).unwrap();
    assert!(pending.get(), "client must reach pending Async read");

    // No event dispatch follows teardown. In particular, a ping cannot be
    // relied upon to remove sources after the event loop has stopped.
    drop(native);
    assert!(!client_dropped.load(Ordering::Acquire));
    assert!(!future_dropped.load(Ordering::Acquire));
    assert!(!surface_destroyed.load(Ordering::Acquire));
    assert!(!surface_cache_dropped.load(Ordering::Acquire));
    // Dropping Display alone cannot run Smithay's protocol destructors. The
    // production shutdown entry must break wl_surface's self-reference first.
    fixture.clarion_state().prepare_shutdown();
    assert!(
        surface_destroyed.load(Ordering::Acquire),
        "production shutdown skipped the real wl_surface destruction callback"
    );
    drop(fixture);
    assert!(surface_cache_dropped.load(Ordering::Acquire));
    drop(executor_source);
    assert!(future_dropped.load(Ordering::Acquire));
    drop(display_source);
    assert!(client_dropped.load(Ordering::Acquire));
    drop(scheduler);
    drop(handle);
    // SHM upload retirement is asynchronous. Its confirmed destruction must
    // finish too; an unbroken surface cache remains held beyond this bound.
    let retired = Instant::now() + Duration::from_secs(3);
    while Arc::strong_count(&context) != 1 && Instant::now() < retired {
        std::thread::yield_now();
    }
    assert_eq!(
        Arc::strong_count(&context),
        1,
        "stopped event loop or callback still retains the Vulkan device"
    );
    // The sole owner is dropped synchronously while EventLoop is still alive;
    // returning from this Drop proves vkDestroyDevice and field teardown ended.
    drop(context);
    eprintln!("NATIVE_LOOP_TEARDOWN device_drop_completed=true future_cancelled=true client_cache_dropped=true smithay_surface_destroyed=true surface_cache_dropped=true while_loop_alive=true");
    drop(event_loop);
    drop(wayland_client);
    drop(ipc_client);
}

/// Populate Smithay's actual RendererSurfaceState texture map, not just opaque
/// test ClientData. PrivateSurfaceData keeps its own WlSurface in children until
/// the protocol destruction callback runs; that self-reference retained native
/// textures during real TTY shutdown even after Display and State were dropped.
fn populated_smithay_surface_cache(
    native: &mut super::super::native::NativeRenderer,
) -> (
    crate::tests::Fixture,
    Arc<std::sync::atomic::AtomicBool>,
    Arc<std::sync::atomic::AtomicBool>,
) {
    use std::os::fd::AsFd;
    use std::os::unix::fs::FileExt;
    use std::sync::atomic::{AtomicBool, Ordering};

    use smithay::backend::renderer::utils::{import_surface, with_renderer_surface_state};
    use smithay::backend::renderer::Renderer;
    use smithay::wayland::compositor::{add_destruction_hook, with_states};
    use wayland_client::protocol::wl_shm::{self, WlShm};
    use wayland_client::Proxy;

    struct CacheDropped(Arc<AtomicBool>);
    impl Drop for CacheDropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    let mut fixture = crate::tests::Fixture::new();
    let id = fixture.add_client();
    let file = std::fs::File::from(
        smithay::reexports::rustix::fs::memfd_create(
            "clarion-native-surface-teardown",
            smithay::reexports::rustix::fs::MemfdFlags::CLOEXEC,
        )
        .unwrap(),
    );
    file.set_len(16).unwrap();
    file.write_all_at(&[71, 37, 19, 255].repeat(4), 0).unwrap();
    let client = fixture.client(id);
    let registry = client.display.get_registry(&client.qh, ());
    let global = client
        .state
        .globals
        .iter()
        .find(|global| global.interface == WlShm::interface().name)
        .unwrap();
    let shm: WlShm = registry.bind(global.name, 1, &client.qh, ());
    let pool = shm.create_pool(file.as_fd(), 16, &client.qh, ());
    let buffer = pool.create_buffer(0, 2, 2, 8, wl_shm::Format::Argb8888, &client.qh, ());
    let surface = client
        .state
        .compositor
        .as_ref()
        .unwrap()
        .create_surface(&client.qh, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    fixture.roundtrip(id);
    let server_surface = fixture
        .clarion()
        .root_surface
        .keys()
        .next()
        .expect("real wl_surface commit must reach the server")
        .clone();
    let surface_destroyed = Arc::new(AtomicBool::new(false));
    let observed = surface_destroyed.clone();
    add_destruction_hook::<crate::compositor::State, _>(&server_surface, move |_, _| {
        observed.store(true, Ordering::Release)
    });
    let cache_dropped = Arc::new(AtomicBool::new(false));
    with_states(&server_surface, |states| {
        states
            .data_map
            .insert_if_missing_threadsafe(|| CacheDropped(cache_dropped.clone()));
        import_surface(native, states).unwrap();
    });
    assert!(with_renderer_surface_state(&server_surface, |state| {
        state
            .texture::<super::super::VulkanTexture>(native.context_id())
            .is_some()
    })
    .unwrap());
    drop(server_surface);
    // Client proxies remain live on the wire: the test deliberately sends no
    // surface.destroy and performs no post-shutdown client/event-loop dispatch.
    drop((surface, buffer, pool, shm, registry));
    (fixture, surface_destroyed, cache_dropped)
}

const CHILD: &str =
    "render_helpers::vulkan::hardware_tests::lifecycle::gpu_lifecycle_child_fixture";
static EXIT_MARKERS: OnceLock<PathBuf> = OnceLock::new();
fn teardown_marker(name: &str) {
    if let Some(directory) = EXIT_MARKERS.get() {
        let value = format!(
            "gpu_drop={};tls_drop={}\n",
            directory.join("gpu-dropped").exists(),
            directory.join("tls-dropped").exists()
        );
        let _ = std::fs::write(directory.join(name), value);
    }
}
extern "C" fn older_teardown() {
    teardown_marker("older-atexit");
}
extern "C" fn later_teardown() {
    // Deliberately registered after actual driver/pipeline initialization. A
    // correct process boundary drains before libc starts even its newest hook.
    teardown_marker("later-atexit");
}
struct GpuPayload {
    pipeline: Option<VulkanPipelineManager>,
    context: Option<Arc<VulkanContext>>,
    marker: PathBuf,
}
impl Drop for GpuPayload {
    fn drop(&mut self) {
        drop(self.pipeline.take());
        drop(self.context.take());
        std::fs::write(
            &self.marker,
            "vkDestroyDevice and context fields finished\n",
        )
        .unwrap();
    }
}
struct TlsMarker(PathBuf);
impl Drop for TlsMarker {
    fn drop(&mut self) {
        std::fs::write(&self.0, "native TLS destructor finished\n").unwrap();
    }
}
thread_local! {
    static TLS_MARKER: std::cell::RefCell<Option<TlsMarker>> = const { std::cell::RefCell::new(None) };
}

#[test]
#[ignore = "private subprocess fixture; use the four lifecycle wrappers"]
fn gpu_lifecycle_child_fixture() {
    let case =
        std::env::var("CLARION_LIFECYCLE_CHILD_CASE").expect("only launched by lifecycle wrapper");
    let directory = PathBuf::from(std::env::var_os("CLARION_LIFECYCLE_MARKERS").unwrap());
    EXIT_MARKERS.set(directory.clone()).unwrap();
    assert_eq!(unsafe { libc::atexit(older_teardown) }, 0);
    let node = smithay::backend::drm::DrmNode::from_path(
        std::env::var_os("CLARION_VULKAN_TEST_NODE").expect("set CLARION_VULKAN_TEST_NODE"),
    )
    .unwrap();
    let hung = case == "hung";
    if hung {
        lifecycle::set_exit_timeout_for_test(Duration::from_millis(250));
    }
    let (wake, gate) = mpsc::channel();
    let (ready, started) = mpsc::channel();
    let worker_directory = directory.clone();
    lifecycle::spawn_worker(
        "gpu-exit-regression-preparation",
        move || {
            if !hung {
                let _ = wake.send(());
            }
        },
        move |_| {
            let context = VulkanContext::init(Some(&node)).unwrap();
            // The second initialization exercises a layer already loaded by an
            // earlier context without replacing the original process hook.
            let later_context = VulkanContext::init(Some(&node)).unwrap();
            drop(later_context);
            std::fs::write(
                worker_directory.join("device.json"),
                serde_json::to_vec_pretty(&serde_json::json!({
                    "device_name": context.info.device_name,
                    "node": std::env::var("CLARION_VULKAN_TEST_NODE").unwrap(),
                }))
                .unwrap(),
            )
            .unwrap();
            let pipeline =
                VulkanPipelineManager::new(context.clone(), ash::vk::Format::B8G8R8A8_UNORM)
                    .unwrap();
            let payload = GpuPayload {
                pipeline: Some(pipeline),
                context: Some(context),
                marker: worker_directory.join("gpu-dropped"),
            };
            TLS_MARKER.with(|slot| {
                *slot.borrow_mut() = Some(TlsMarker(worker_directory.join("tls-dropped")))
            });
            std::fs::write(
                worker_directory.join("worker-pending"),
                "real pipeline and context held behind shutdown gate\n",
            )
            .unwrap();
            ready.send(()).unwrap();
            gate.recv().unwrap();
            drop(payload);
        },
    )
    .unwrap();
    // Receipt is sent only after real driver initialization and pipeline build.
    started.recv_timeout(Duration::from_secs(20)).unwrap();
    assert_eq!(unsafe { libc::atexit(later_teardown) }, 0);
    match case.as_str() {
        "normal" => (),
        "explicit" => std::process::exit(7),
        "panic" => panic!("intentional default-libtest failure with pending GPU preparation"),
        "hung" => std::process::exit(9),
        _ => panic!("unknown lifecycle fixture case"),
    }
}

fn run_case(case: &str, expected: i32) {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let root = std::env::var_os("CLARION_LIFECYCLE_EVIDENCE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let directory = root.join(format!(
        "clarion-gpu-lifecycle-{}-{}-{case}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let stdout = std::fs::File::create(directory.join("stdout.log")).unwrap();
    let stderr = std::fs::File::create(directory.join("stderr.log")).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            CHILD,
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("CLARION_LIFECYCLE_CHILD_CASE", case)
        .env("CLARION_LIFECYCLE_MARKERS", &directory)
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "lifecycle child {case} exceeded parent bound; evidence={}",
                directory.display()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let stdout = std::fs::read_to_string(directory.join("stdout.log")).unwrap();
    let stderr = std::fs::read_to_string(directory.join("stderr.log")).unwrap();
    let bad_validation = [
        "validation error",
        "validation warning",
        "vuid-",
        "sync-hazard",
        "failed to find layer",
    ];
    let diagnostics = format!("{stdout}\n{stderr}").to_ascii_lowercase();
    let validation_errors = bad_validation
        .iter()
        .filter(|pattern| diagnostics.contains(**pattern))
        .count();
    let unexpected_panics = stdout
        .lines()
        .chain(stderr.lines())
        .filter(|line| line.contains("panicked at"))
        .count();
    let device: serde_json::Value = std::fs::read(directory.join("device.json"))
        .ok()
        .and_then(|data| serde_json::from_slice(&data).ok())
        .unwrap_or(serde_json::Value::Null);
    let checksum = |name: &str| {
        let output = Command::new("sha256sum")
            .arg(directory.join(name))
            .output()
            .unwrap();
        assert!(output.status.success(), "hash child evidence");
        String::from_utf8(output.stdout)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .to_owned()
    };
    let mut metadata = serde_json::json!({
        "case": case,
        "expected_exit_code": expected,
        "actual_exit_code": status.code(),
        "actual_signal": status.signal(),
        "validation_errors": validation_errors,
        "node": std::env::var("CLARION_VULKAN_TEST_NODE").unwrap(),
        "device_name": device.get("device_name"),
        "gpu_dropped": directory.join("gpu-dropped").exists(),
        "tls_dropped": directory.join("tls-dropped").exists(),
        "older_atexit": directory.join("older-atexit").exists(),
        "later_atexit": directory.join("later-atexit").exists(),
        "stdout_sha256": checksum("stdout.log"),
        "stderr_sha256": checksum("stderr.log"),
        "passed": false,
    });
    std::fs::write(
        directory.join("metadata.json"),
        serde_json::to_vec_pretty(&metadata).unwrap(),
    )
    .unwrap();
    assert_eq!(
        validation_errors,
        0,
        "child validation diagnostics; evidence={}",
        directory.display()
    );
    assert_eq!(
        unexpected_panics,
        usize::from(case == "panic"),
        "unexpected child panic; evidence={}",
        directory.display()
    );
    if case == "panic" {
        assert!(
            stderr.contains("intentional default-libtest failure with pending GPU preparation"),
            "wrong child panic; evidence={}",
            directory.display()
        );
    }
    // Child libtest output stays in files: never reprint nested test result/name
    // lines into the GPU matrix runner's top-level exact-filter receipt.
    assert_eq!(
        status.code(),
        Some(expected),
        "lifecycle child {case}: {status}; evidence={}",
        directory.display()
    );
    assert!(
        directory.join("worker-pending").exists(),
        "fixture did not reach real GPU work: {}",
        directory.display()
    );
    if case == "hung" {
        assert!(!directory.join("gpu-dropped").exists());
        assert!(!directory.join("tls-dropped").exists());
        assert!(
            !directory.join("later-atexit").exists(),
            "deadline entered the newest atexit teardown"
        );
        assert!(
            !directory.join("older-atexit").exists(),
            "deadline returned into older teardown"
        );
        assert!(std::fs::read_to_string(directory.join("stderr.log"))
            .unwrap()
            .contains("skipping unsafe loader teardown (exit 124)"));
    } else {
        assert_eq!(
            std::fs::read_to_string(directory.join("later-atexit")).unwrap(),
            "gpu_drop=true;tls_drop=true\n",
            "GPU and TLS Drop must precede even the newest libc atexit callback"
        );
        assert_eq!(
            std::fs::read_to_string(directory.join("older-atexit")).unwrap(),
            "gpu_drop=true;tls_drop=true\n",
            "join must acknowledge GPU and TLS destruction before older atexit"
        );
        assert!(directory.join("gpu-dropped").exists());
    }
    metadata["passed"] = true.into();
    std::fs::write(
        directory.join("metadata.json"),
        serde_json::to_vec_pretty(&metadata).unwrap(),
    )
    .unwrap();
    println!(
        "LIFECYCLE_EVIDENCE case={case} path={}",
        directory.display()
    );
}

#[test]
#[ignore = "requires real DRM render node and Vulkan"]
fn gpu_workers_retire_before_normal_process_exit() {
    run_case("normal", 0);
}
#[test]
#[ignore = "requires real DRM render node and Vulkan"]
fn gpu_workers_retire_before_explicit_nonzero_exit() {
    run_case("explicit", 7);
}
#[test]
#[ignore = "requires real DRM render node and Vulkan"]
fn gpu_workers_retire_before_panicking_libtest_exit() {
    run_case("panic", 101);
}
#[test]
#[ignore = "requires real DRM render node and Vulkan"]
fn gpu_workers_hung_exit_uses_fast_termination_without_layer_teardown() {
    run_case("hung", 124);
}
