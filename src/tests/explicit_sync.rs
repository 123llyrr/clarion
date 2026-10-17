//! Wayland buffer-acquire synchronization tests. Readiness/error paths use real
//! socket polling without a GPU. The opt-in DRM syncobj test opens a render node
//! only; it does not need a DRM master, a running Wayland session, or a display.

use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use calloop::EventLoop;
use clarion_config::Config;
use smithay::backend::allocator::dmabuf::AsDmabuf;
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice};
use smithay::backend::allocator::{Allocator, Buffer, Fourcc, Modifier};
use smithay::backend::drm::DrmDeviceFd;
use smithay::backend::renderer::ImportDma;
use smithay::reexports::drm::control::{syncobj, Device as _};
use smithay::reexports::wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_buffer_params_v1::{self, ZwpLinuxBufferParamsV1},
    zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
};
use smithay::reexports::wayland_protocols::wp::linux_drm_syncobj::v1::client::{
    wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1,
    wp_linux_drm_syncobj_surface_v1::WpLinuxDrmSyncobjSurfaceV1,
    wp_linux_drm_syncobj_timeline_v1::WpLinuxDrmSyncobjTimelineV1,
};
use smithay::reexports::wayland_server::Display;
use smithay::utils::DeviceFd;
use wayland_client::protocol::wl_buffer::{self, WlBuffer};
use wayland_client::{Dispatch, Proxy};

use super::client::{Client, State as ClientState};
use super::server::Server;
use crate::backend::{AnyRenderer, RendererBackendType};
use crate::compositor::{NewClient, State};

mod snapshot_leases;

mod implicit {
    use std::io::Write;
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;
    use std::sync::Arc;

    use calloop::generic::Generic;
    use calloop::{Interest, Mode, PostAction};
    use smithay::backend::allocator::dmabuf::{AlreadyReady, Dmabuf, DmabufFlags};
    use smithay::backend::allocator::{Fourcc, Modifier};
    use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface as ServerSurface;
    use smithay::reexports::wayland_server::Resource;
    use smithay::wayland::compositor::{add_pre_commit_hook, with_states, SurfaceAttributes};
    use wayland_client::protocol::wl_surface::WlSurface;
    use wayland_client::Proxy;

    use crate::compositor::State;
    use crate::tests::client::ClientId;
    use crate::tests::Fixture;

    fn fixture() -> (Fixture, ClientId, WlSurface, ServerSurface) {
        let mut fixture = Fixture::new();
        let id = fixture.add_client();
        let client = fixture.client(id);
        let surface = client
            .state
            .compositor
            .as_ref()
            .unwrap()
            .create_surface(&client.qh, ());
        surface.commit();
        fixture.double_roundtrip(id);
        let server = fixture
            .clarion()
            .root_surface
            .keys()
            .find(|s| s.id().protocol_id() == surface.id().protocol_id())
            .unwrap()
            .clone();
        (fixture, id, surface, server)
    }

    fn readiness_buffer() -> (Dmabuf, Arc<OwnedFd>, UnixStream) {
        // DmabufSource only polls each plane FD. A socket pair gives the real
        // calloop/Smithay readiness semantics without creating or rendering a
        // GPU allocation. These buffers never enter any renderer/importer.
        let (reader, producer) = UnixStream::pair().unwrap();
        let reader = Arc::new(OwnedFd::from(reader));
        let mut builder = Dmabuf::builder(
            (8, 8),
            Fourcc::Argb8888,
            Modifier::Linear,
            DmabufFlags::empty(),
        );
        assert!(builder.add_plane(reader.clone(), 0, 32));
        (builder.build().unwrap(), reader, producer)
    }

    fn install_readiness_hook(server: &ServerSurface, dmabuf: Dmabuf) {
        add_pre_commit_hook::<State, _>(server, move |state, _, surface| {
            state.add_implicit_dmabuf_blocker(surface, &dmabuf);
        });
    }

    fn scale(server: &ServerSurface) -> i32 {
        with_states(server, |states| {
            states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .buffer_scale
        })
    }

    #[test]
    fn failed_readiness_registration_cancels_commit_and_later_commit_recovers() {
        let (mut fixture, id, surface, server) = fixture();
        let (dmabuf, reader, mut producer) = readiness_buffer();
        assert!(dmabuf.generate_blocker(Interest::READ).is_ok());
        // Register the exact same FD, not a dup. The production insert_source
        // below must hit a real epoll EEXIST failure for the unsignaled source.
        let occupied = fixture
            .clarion()
            .event_loop
            .insert_source(
                Generic::new(reader, Interest::READ, Mode::Level),
                |_, _, _| Ok(PostAction::Continue),
            )
            .unwrap();
        install_readiness_hook(&server, dmabuf);
        surface.set_buffer_scale(2);
        surface.commit();
        fixture.double_roundtrip(id);
        assert_eq!(
            scale(&server),
            1,
            "unwatched producer must not let a commit reach compositor state"
        );

        fixture.clarion().event_loop.remove(occupied);
        producer.write_all(&[1]).unwrap();
        surface.set_buffer_scale(3);
        surface.commit();
        fixture.double_roundtrip(id);
        assert_eq!(
            scale(&server),
            3,
            "cancelled acquire poisoned later commits"
        );
        assert!(fixture.client(id).connection.protocol_error().is_none());
    }

    #[test]
    fn already_ready_implicit_buffer_applies_without_waiting() {
        let (mut fixture, id, surface, server) = fixture();
        let (dmabuf, _reader, mut producer) = readiness_buffer();
        producer.write_all(&[1]).unwrap();
        assert!(matches!(
            dmabuf.generate_blocker(Interest::READ),
            Err(AlreadyReady)
        ));
        install_readiness_hook(&server, dmabuf);
        surface.set_buffer_scale(2);
        surface.commit();
        fixture.double_roundtrip(id);
        assert_eq!(scale(&server), 2);
    }

    #[test]
    fn implicit_buffer_waits_for_producer_then_applies_without_another_commit() {
        let (mut fixture, id, surface, server) = fixture();
        let (dmabuf, _reader, mut producer) = readiness_buffer();
        install_readiness_hook(&server, dmabuf);
        surface.set_buffer_scale(2);
        surface.commit();
        fixture.double_roundtrip(id);
        assert_eq!(scale(&server), 1);
        producer.write_all(&[1]).unwrap();
        fixture.double_roundtrip(id);
        assert_eq!(scale(&server), 2);
        assert!(fixture.client(id).connection.protocol_error().is_none());
    }
}

// Keep the ordinary test client generic. Only this test binds these protocols.
wayland_client::delegate_noop!(ClientState: ignore ZwpLinuxDmabufV1);
wayland_client::delegate_noop!(ClientState: ignore ZwpLinuxBufferParamsV1);
wayland_client::delegate_noop!(ClientState: ignore WpLinuxDrmSyncobjManagerV1);
wayland_client::delegate_noop!(ClientState: ignore WpLinuxDrmSyncobjSurfaceV1);
wayland_client::delegate_noop!(ClientState: ignore WpLinuxDrmSyncobjTimelineV1);

#[derive(Clone, Default)]
struct BufferReleases(Arc<AtomicUsize>);

impl BufferReleases {
    fn count(&self) -> usize {
        self.0.load(Ordering::Relaxed)
    }
}

impl Dispatch<WlBuffer, BufferReleases> for ClientState {
    fn event(
        _state: &mut Self,
        _proxy: &WlBuffer,
        event: wl_buffer::Event,
        releases: &BufferReleases,
        _connection: &wayland_client::Connection,
        _qh: &wayland_client::QueueHandle<Self>,
    ) {
        assert!(matches!(event, wl_buffer::Event::Release));
        releases.0.fetch_add(1, Ordering::Relaxed);
    }
}

struct RenderNode {
    device: DrmDeviceFd,
    allocator: GbmAllocator<Arc<std::fs::File>>,
}

impl RenderNode {
    fn open() -> Self {
        let path = std::env::var("CLARION_VULKAN_TEST_NODE")
            .unwrap_or_else(|_| "/dev/dri/renderD128".to_owned());
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap_or_else(|err| panic!("cannot open DRM render node {path}: {err}"));
        let device = DrmDeviceFd::new(DeviceFd::from(OwnedFd::from(file.try_clone().unwrap())));
        let gbm = GbmDevice::new(Arc::new(file)).expect("GBM render node");
        let allocator = GbmAllocator::new(gbm, GbmBufferFlags::RENDERING);
        Self { device, allocator }
    }
}

struct Timeline {
    device: DrmDeviceFd,
    handle: syncobj::Handle,
}

impl Timeline {
    fn new(device: &DrmDeviceFd) -> Self {
        Self {
            device: device.clone(),
            handle: device
                .create_syncobj(false)
                .expect("create real DRM syncobj"),
        }
    }

    fn export(&self) -> OwnedFd {
        self.device
            .syncobj_to_fd(self.handle, false)
            .expect("export timeline fd")
    }

    fn signal(&self, point: u64) {
        self.device
            .syncobj_timeline_signal(&[self.handle], &[point])
            .expect("signal acquire point");
    }

    fn value(&self) -> u64 {
        let mut points = [0];
        self.device
            .syncobj_timeline_query(&[self.handle], &mut points, false)
            .expect("query real DRM timeline");
        points[0]
    }
}

impl Drop for Timeline {
    fn drop(&mut self) {
        self.device.destroy_syncobj(self.handle).unwrap();
    }
}

struct HardwareFixture {
    server: Server,
    client: Client,
    node: RenderNode,
}

impl HardwareFixture {
    fn new() -> Self {
        let event_loop = EventLoop::try_new().unwrap();
        let state = State::new(
            Config::default(),
            event_loop.handle(),
            event_loop.get_signal(),
            Display::new().unwrap(),
            true,
            false,
            false,
            RendererBackendType::Vulkan,
        )
        .expect("initialize real headless Vulkan compositor");
        let mut server = Server { event_loop, state };
        server
            .state
            .backend
            .with_renderer(|renderer| match renderer {
                AnyRenderer::Native(renderer) => eprintln!(
                    "HARDWARE: {} (Wayland sync fixture headless renderer)",
                    renderer.vulkan().info.device_name
                ),
                AnyRenderer::Gles(_) => unreachable!("fixture requires native Vulkan"),
            })
            .expect("Wayland sync fixture renderer is initialized");
        let node = RenderNode::open();
        server.state.clarion.enable_drm_syncobj(node.device.clone());
        assert!(
            server.state.clarion.drm_syncobj_state.is_some(),
            "render node does not support syncobj eventfd"
        );
        {
            let State { backend, clarion } = &mut server.state;
            backend.headless().add_output(
                &mut crate::platform::device_bridge::DeviceCoordinator::new(clarion),
                1,
                (64, 64),
            );
        }

        let (server_socket, client_socket) = UnixStream::pair().unwrap();
        server.state.clarion.insert_client(NewClient {
            client: server_socket,
            restricted: false,
            credentials_unknown: false,
        });
        let client = Client::new(client_socket);
        let mut fixture = Self {
            server,
            client,
            node,
        };
        fixture.roundtrip();
        fixture
    }

    fn dispatch(&mut self) {
        self.server
            .event_loop
            .dispatch(Duration::from_millis(2), &mut self.server.state)
            .unwrap();
        self.server.state.refresh_and_flush_clients();
        self.client.dispatch();
    }

    fn roundtrip_without_render(&mut self) {
        let done = self.client.send_sync();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done.done.load(Ordering::Relaxed) {
            assert!(Instant::now() < deadline, "Wayland roundtrip timed out");
            self.server
                .event_loop
                .dispatch(Duration::from_millis(2), &mut self.server.state)
                .unwrap();
            self.server
                .state
                .clarion
                .display_handle
                .flush_clients()
                .unwrap();
            self.client.dispatch();
        }
    }

    fn until(&mut self, description: &str, ready: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready(self) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {description}"
            );
            self.dispatch();
        }
    }

    fn roundtrip(&mut self) {
        let done = self.client.send_sync();
        self.until("Wayland roundtrip", |_| done.done.load(Ordering::Relaxed));
    }

    fn bind<P: wayland_client::Proxy + 'static>(&mut self) -> P
    where
        ClientState: wayland_client::Dispatch<P, ()>,
    {
        let global = self
            .client
            .state
            .globals
            .iter()
            .find(|global| global.interface == P::interface().name)
            .unwrap_or_else(|| panic!("missing Wayland global {}", P::interface().name));
        let registry = self.client.display.get_registry(&self.client.qh, ());
        registry.bind(
            global.name,
            global.version.min(P::interface().version),
            &self.client.qh,
            (),
        )
    }

    fn make_dmabuf(&mut self, dmabuf: &ZwpLinuxDmabufV1) -> WlBuffer {
        let modifiers: Vec<Modifier> = self
            .server
            .state
            .backend
            .with_renderer(|renderer| match renderer {
                AnyRenderer::Native(renderer) => renderer
                    .dmabuf_formats()
                    .iter()
                    .filter(|format| format.code == Fourcc::Argb8888)
                    .map(|format| format.modifier)
                    .collect(),
                AnyRenderer::Gles(_) => unreachable!(),
            })
            .unwrap();
        assert!(!modifiers.is_empty(), "no Vulkan ARGB8888 DMA-BUF format");
        let backing = self
            .node
            .allocator
            .create_buffer(64, 64, Fourcc::Argb8888, &modifiers)
            .expect("allocate client GBM buffer")
            .export()
            .expect("export client DMA-BUF");
        let params = dmabuf.create_params(&self.client.qh, ());
        let modifier = u64::from(backing.format().modifier);
        for (i, ((fd, offset), stride)) in backing
            .handles()
            .zip(backing.offsets())
            .zip(backing.strides())
            .enumerate()
        {
            params.add(
                fd.as_fd(),
                i as u32,
                offset,
                stride,
                (modifier >> 32) as u32,
                modifier as u32,
            );
        }
        let buffer = params.create_immed(
            64,
            64,
            Fourcc::Argb8888 as u32,
            zwp_linux_buffer_params_v1::Flags::empty(),
            &self.client.qh,
            BufferReleases::default(),
        );
        params.destroy();
        buffer
    }
}

#[test]
#[ignore = "requires Vulkan and a DRM render node with timeline syncobj/eventfd support"]
fn wayland_client_acquire_and_release_follow_gpu_use() {
    let mut fixture = HardwareFixture::new();
    let syncobj: WpLinuxDrmSyncobjManagerV1 = fixture.bind();
    let dmabuf: ZwpLinuxDmabufV1 = fixture.bind();
    fixture.roundtrip();

    let acquire = Timeline::new(&fixture.node.device);
    let release = Timeline::new(&fixture.node.device);
    let acquire_fd = acquire.export();
    let release_fd = release.export();
    let acquire_proxy = syncobj.import_timeline(acquire_fd.as_fd(), &fixture.client.qh, ());
    let release_proxy = syncobj.import_timeline(release_fd.as_fd(), &fixture.client.qh, ());
    let buffer = fixture.make_dmabuf(&dmabuf);

    let surface = fixture.client.create_window().surface.clone();
    let sync_surface = syncobj.get_surface(&surface, &fixture.client.qh, ());
    surface.commit();
    fixture.roundtrip();
    fixture.roundtrip();
    fixture.client.window(&surface).ack_last();

    sync_surface.set_acquire_point(&acquire_proxy, 0, 1);
    sync_surface.set_release_point(&release_proxy, 0, 2);
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 64, 64);
    surface.commit();
    fixture.roundtrip();
    assert_eq!(
        fixture.server.state.clarion.layout.windows().count(),
        0,
        "unsignaled client acquire point allowed the Wayland commit to map"
    );
    assert_eq!(release.value(), 0, "release signaled before acquire");

    acquire.signal(1);
    fixture.until("acquire blocker and mapped Vulkan frame", |fixture| {
        fixture.server.state.clarion.layout.windows().count() == 1
    });
    assert_eq!(release.value(), 0, "client still owns an attached buffer");

    // Submit fresh client-buffer damage and render it before the normal event
    // loop redraw. The native headless path waits for its GPU fence to complete.
    surface.damage_buffer(0, 0, 64, 64);
    surface.commit();
    fixture.client.connection.flush().unwrap();
    fixture
        .server
        .event_loop
        .dispatch(Duration::from_millis(2), &mut fixture.server.state)
        .unwrap();
    let output = fixture
        .server
        .state
        .clarion
        .global_space
        .outputs()
        .next()
        .unwrap()
        .clone();
    {
        let State { backend, clarion } = &mut fixture.server.state;
        clarion.queue_redraw(&output);
        let mut coordinator = crate::frame_pipeline::FrameCoordinator::new(clarion, &output);
        assert!(
            matches!(
                backend.headless().render(&mut coordinator, &output),
                crate::backend::RenderResult::Submitted
            ),
            "native Vulkan did not submit the client-buffer damage"
        );
    }
    assert_eq!(
        release.value(),
        0,
        "release signaled while buffer is attached"
    );

    surface.attach(None, 0, 0);
    surface.commit();
    fixture.until("GPU-safe release point", |_| release.value() >= 2);
    assert_eq!(release.value(), 2);
    assert!(fixture.client.connection.protocol_error().is_none());
}

#[test]
#[ignore = "requires Vulkan and a DRM render node with timeline syncobj/eventfd support"]
fn idle_gpu_completion_releases_replaced_and_detached_wayland_buffers() {
    for export_required in [true, false] {
        use calloop::timer::{TimeoutAction, Timer};
        use smithay::backend::drm::DrmNode;
        use smithay::backend::renderer::element::surface::{
            render_elements_from_surface_tree, WaylandSurfaceRenderElement,
        };
        use smithay::backend::renderer::element::{Element, Kind, RenderElement};
        use smithay::backend::renderer::{Bind, Color32F, Frame, Offscreen, Renderer};
        use smithay::reexports::wayland_server::Resource;
        use smithay::utils::{Rectangle, Transform};

        use crate::render_helpers::vulkan::hardware_tests::SubmittedGpuWork;
        use crate::render_helpers::vulkan::native::NativeRenderer;
        use crate::render_helpers::vulkan::VulkanRenderer;

        let mut fixture = HardwareFixture::new();
        let syncobj: WpLinuxDrmSyncobjManagerV1 = fixture.bind();
        let dmabuf: ZwpLinuxDmabufV1 = fixture.bind();
        fixture.roundtrip();
        let acquire = Timeline::new(&fixture.node.device);
        acquire.signal(1);
        let acquire_fd = acquire.export();
        let acquire_proxy = syncobj.import_timeline(acquire_fd.as_fd(), &fixture.client.qh, ());
        let releases = [
            Timeline::new(&fixture.node.device),
            Timeline::new(&fixture.node.device),
        ];
        let release_proxies = releases.each_ref().map(|release| {
            syncobj.import_timeline(release.export().as_fd(), &fixture.client.qh, ())
        });
        let buffers = [fixture.make_dmabuf(&dmabuf), fixture.make_dmabuf(&dmabuf)];
        let replacement = fixture.make_dmabuf(&dmabuf);
        let surfaces = buffers.each_ref().map(|buffer| {
            let surface = fixture
                .client
                .state
                .compositor
                .as_ref()
                .unwrap()
                .create_surface(&fixture.client.qh, ());
            let sync_surface = syncobj.get_surface(&surface, &fixture.client.qh, ());
            surface.attach(Some(buffer), 0, 0);
            (surface, sync_surface)
        });
        for ((surface, sync_surface), release) in surfaces.iter().zip(&release_proxies) {
            sync_surface.set_acquire_point(&acquire_proxy, 0, 1);
            sync_surface.set_release_point(release, 0, 2);
            surface.damage_buffer(0, 0, 64, 64);
            surface.commit();
        }
        fixture.roundtrip_without_render();
        fixture.roundtrip_without_render();
        let server_surfaces = surfaces.each_ref().map(|(surface, _)| {
            fixture
                .server
                .state
                .clarion
                .root_surface
                .keys()
                .find(|server| server.id().protocol_id() == surface.id().protocol_id())
                .expect("committed real Wayland surface")
                .clone()
        });

        // Keep this renderer's completion loop separate from compositor fallback
        // timers. Its only possible wakeups are GPU completion or our failure timer;
        // the ordinary headless backend's synchronous render path is never called.
        let mut completion_loop = EventLoop::<bool>::try_new().unwrap();
        let mut vulkan = VulkanRenderer::new();
        vulkan
            .init_from_drm_node(Some(&DrmNode::from_file(&fixture.node.device).unwrap()))
            .unwrap();
        eprintln!(
            "HARDWARE: {} (idle completion renderer selected DRM node)",
            vulkan.info.device_name
        );
        let mut renderer = NativeRenderer::new(vulkan).unwrap();
        renderer.set_completion_export_required(export_required);
        renderer
            .prepare_output_format_for_test(Fourcc::Abgr8888)
            .unwrap();
        eprintln!("IDLE CLIENT BUFFER COMPLETION: export_required={export_required}");
        renderer
            .install_completion_wakeup(&completion_loop.handle())
            .unwrap();
        let mut target = renderer
            .create_buffer(Fourcc::Abgr8888, (128, 64).into())
            .unwrap();
        let elements: Vec<WaylandSurfaceRenderElement<NativeRenderer>> = server_surfaces
            .iter()
            .enumerate()
            .flat_map(|(index, surface)| {
                render_elements_from_surface_tree(
                    &mut renderer,
                    surface,
                    (index as i32 * 64, 0),
                    1.,
                    1.,
                    Kind::Unspecified,
                )
            })
            .collect();
        assert_eq!(elements.len(), 2, "both client DMA-BUFs must be sampled");
        let mut framebuffer = renderer.bind(&mut target).unwrap();
        let mut frame = renderer
            .render(&mut framebuffer, (128, 64).into(), Transform::Normal)
            .unwrap();
        frame
            .clear(Color32F::BLACK, &[Rectangle::from_size((128, 64).into())])
            .unwrap();
        for element in &elements {
            let geometry = element.geometry(1.0.into());
            element
                .draw(
                    &mut frame,
                    element.src(),
                    geometry,
                    &[Rectangle::from_size(geometry.size)],
                    &[],
                    None,
                )
                .unwrap();
        }
        // Reuse the existing legal, already-submitted GPU workload. A future host
        // timeline dependency cannot be legally used to export the frame SYNC_FD.
        let mut work = SubmittedGpuWork::new(frame.context());
        let _producer_fd = work.submit();
        let sync = frame.finish().unwrap();
        assert!(
            !sync.is_reached(),
            "the last frame must actually be pending"
        );
        drop(framebuffer);
        drop(elements);

        surfaces[0].0.attach(Some(&replacement), 0, 0);
        surfaces[0].1.set_acquire_point(&acquire_proxy, 0, 1);
        surfaces[0].1.set_release_point(&release_proxies[0], 0, 4);
        surfaces[0].0.commit();
        surfaces[1].0.attach(None, 0, 0);
        surfaces[1].0.commit();
        fixture.roundtrip_without_render();
        fixture.roundtrip_without_render();
        assert!(
            !sync.is_reached(),
            "test workload ended before buffer replacement"
        );
        for (release, buffer) in releases.iter().zip(&buffers) {
            assert_eq!(
                release.value(),
                0,
                "released a buffer still sampled by the GPU"
            );
            assert_eq!(buffer.data::<BufferReleases>().unwrap().count(), 0);
        }

        // No renderer call, GPU fence wait, compositor timer, or polling flush may
        // retire this last frame. The worker must release owners and wake this loop.
        completion_loop
            .handle()
            .insert_source(
                Timer::from_duration(Duration::from_secs(10)),
                |_, _, timed_out| {
                    *timed_out = true;
                    TimeoutAction::Drop
                },
            )
            .unwrap();
        let mut timed_out = false;
        completion_loop.dispatch(None, &mut timed_out).unwrap();
        assert!(
            !timed_out,
            "GPU completion did not wake the idle event loop"
        );
        assert!(sync.is_reached());
        fixture
            .server
            .state
            .clarion
            .display_handle
            .flush_clients()
            .unwrap();
        fixture.client.dispatch();
        for (release, buffer) in releases.iter().zip(&buffers) {
            assert_eq!(
                release.value(),
                2,
                "idle completion must signal the old buffer's release point"
            );
            assert_eq!(
                buffer.data::<BufferReleases>().unwrap().count(),
                1,
                "completion wakeup must deliver wl_buffer.release without another render"
            );
        }
        assert_eq!(
            replacement.data::<BufferReleases>().unwrap().count(),
            0,
            "the replacement is still attached and must not be released"
        );
        assert!(fixture.client.connection.protocol_error().is_none());
        // Keep renderer, target, and exported SyncPoint alive through every assertion.
        drop((sync, target, renderer));
    }
}
