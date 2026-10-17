//! Opt-in real GPU regression tests. No display master or running session is required.

mod lifecycle;
mod window_snapshot;
mod yuv_cold;

#[cfg(test)]
mod resize_capture_regressions {
    use smithay::backend::renderer::element::utils::{Relocate, RelocateRenderElement};
    use smithay::backend::renderer::element::RenderElement;
    use smithay::backend::renderer::{ImportMem, RendererSuper, Texture};
    use smithay::utils::{Logical, Physical, Scale, Size};

    use super::*;
    use crate::render_helpers::resize::{capture_resize_texture, ResizeRenderElement};
    use crate::render_helpers::{render_to_vec, RenderTarget};

    type Input<T> = ((T, Rectangle<i32, Physical>), Size<f64, Logical>);

    fn read_resize<R>(renderer: &mut R, element: &ResizeRenderElement, scale: Scale<f64>) -> Vec<u8>
    where
        R: Renderer
            + Offscreen<<R as RendererSuper>::TextureId>
            + Bind<<R as RendererSuper>::TextureId>
            + ExportMem,
        R::Error: Send + Sync + 'static,
        ResizeRenderElement: RenderElement<R>,
        for<'a> R::Framebuffer<'a>: Texture,
    {
        // Include negative CSD offsets instead of cropping them at the origin.
        let shifted = RelocateRenderElement::from_element(element, (12, 12), Relocate::Relative);
        render_to_vec(
            renderer,
            (64, 56).into(),
            scale,
            Transform::Normal,
            Fourcc::Abgr8888,
            std::iter::once(&shifted),
        )
        .unwrap()
    }

    fn assert_pixels(actual: &[u8], expected: &[u8], context: &str) {
        assert_eq!(actual.len(), expected.len());
        let worst = actual
            .iter()
            .zip(expected)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        assert!(worst <= 2, "{context}: worst channel difference {worst}");
    }

    fn exercise_capture<R, F>(renderer: &mut R, mut make: F)
    where
        R: Renderer
            + Offscreen<<R as RendererSuper>::TextureId>
            + Bind<<R as RendererSuper>::TextureId>
            + ExportMem
            + ImportMem,
        R::Error: Send + Sync + 'static,
        R::TextureId: Clone,
        ResizeRenderElement: RenderElement<R>,
        for<'a> R::Framebuffer<'a>: Texture,
        F: FnMut(
            &mut R,
            Rectangle<f64, Logical>,
            Scale<f64>,
            Input<R::TextureId>,
            Input<R::TextureId>,
            f32,
        ) -> ResizeRenderElement,
    {
        let size: Size<f64, Logical> = (16., 12.).into();
        let area = Rectangle::from_size(size);
        for (scale, geo, channel) in [
            (1.0, Rectangle::from_size((16, 12).into()), 255u8),
            (
                1.25,
                Rectangle::new((-5, -5).into(), (30, 25).into()),
                128u8,
            ),
        ] {
            let scale = Scale::from(scale);
            let colors = [
                [channel, 0, 0, channel],
                [0, channel, 0, channel],
                [0, 0, channel, channel],
            ];
            let textures = colors.map(|color| {
                renderer
                    .import_memory(
                        &color.repeat((geo.size.w * geo.size.h) as usize),
                        Fourcc::Abgr8888,
                        (geo.size.w, geo.size.h).into(),
                        false,
                    )
                    .unwrap()
            });
            let mut captures = Vec::new();
            let mut before = Vec::new();
            for i in 0..RenderTarget::COUNT {
                let element = make(
                    renderer,
                    area,
                    scale,
                    ((textures[i].clone(), geo), size),
                    ((textures[(i + 1) % 3].clone(), geo), size),
                    0.4,
                );
                before.push(read_resize(renderer, &element, scale));
                captures.push(capture_resize_texture(renderer, &element, scale, size).unwrap());
            }

            // Read all three after capturing them: shared/recycled storage must
            // not let the last capture replace an earlier target's pixels.
            for (i, target) in [
                RenderTarget::Output,
                RenderTarget::Screencast,
                RenderTarget::ScreenCapture,
            ]
            .into_iter()
            .enumerate()
            {
                for requested_scale in [scale, Scale::from(2.)] {
                    let input_size = Size::from((
                        size.w * scale.x / requested_scale.x,
                        size.h * scale.y / requested_scale.y,
                    ));
                    let reference = make(
                        renderer,
                        area,
                        requested_scale,
                        ((textures[i].clone(), geo), input_size),
                        ((textures[(i + 1) % 3].clone(), geo), input_size),
                        0.4,
                    );
                    let expected = read_resize(renderer, &reference, requested_scale);
                    let resumed = make(
                        renderer,
                        area,
                        requested_scale,
                        captures[i].as_input(requested_scale),
                        ((textures[(i + 2) % 3].clone(), geo), input_size),
                        0.,
                    );
                    let actual = read_resize(renderer, &resumed, requested_scale);
                    assert_pixels(
                        &actual,
                        &expected,
                        &format!("{target:?}, scale {requested_scale:?}, alpha {channel}"),
                    );
                    if requested_scale == scale {
                        assert_pixels(
                            &actual,
                            &before[i],
                            "retarget must start at the displayed mix",
                        );
                    }
                    let progressing = make(
                        renderer,
                        area,
                        requested_scale,
                        captures[i].as_input(requested_scale),
                        ((textures[(i + 2) % 3].clone(), geo), input_size),
                        0.5,
                    );
                    let progressing = read_resize(renderer, &progressing, requested_scale);
                    let center = ((18 * 64 + 20) * 4) as usize;
                    let next = colors[(i + 2) % 3];
                    for c in 0..4 {
                        let expected = ((u16::from(actual[center + c]) + u16::from(next[c])) as f32
                            * 0.5)
                            .round() as u8;
                        assert!(
                            progressing[center + c].abs_diff(expected) <= 2,
                            "new transition must advance with premultiplied alpha"
                        );
                    }
                }
            }

            // Keep a clone of the first image while replacing each intermediate
            // with a flat texture. Older captures must remain immutable.
            let retained = captures[0].as_input(scale);
            let mut current = captures.remove(0);
            for i in 0..8 {
                let element = make(
                    renderer,
                    area,
                    scale,
                    current.as_input(scale),
                    ((textures[(i + 2) % 3].clone(), geo), size),
                    0.4,
                );
                let expected = read_resize(renderer, &element, scale);
                current = capture_resize_texture(renderer, &element, scale, size).unwrap();
                let resumed = make(
                    renderer,
                    area,
                    scale,
                    current.as_input(scale),
                    ((textures[(i + 1) % 3].clone(), geo), size),
                    0.,
                );
                assert_pixels(
                    &read_resize(renderer, &resumed, scale),
                    &expected,
                    "repeated interruption must flatten exactly once",
                );
            }
            let original = make(
                renderer,
                area,
                scale,
                retained,
                ((textures[2].clone(), geo), size),
                0.,
            );
            assert_pixels(
                &read_resize(renderer, &original, scale),
                &before[0],
                "retained capture must not be overwritten",
            );
        }
    }

    #[test]
    #[ignore = "requires real DRM render node and GLES"]
    fn gles_resize_capture_retargets_without_pixel_jump() {
        let (_vk, mut renderer, _) = setup();
        crate::render_helpers::shaders::init(&mut renderer);
        crate::render_helpers::resources::init(&mut renderer);
        exercise_capture(
            &mut renderer,
            |_, area, scale, (prev, size_prev), (next, size_next), progress| {
                ResizeRenderElement::new(
                    area,
                    scale,
                    prev,
                    size_prev,
                    next,
                    size_next,
                    progress,
                    progress,
                    Default::default(),
                    false,
                    1.,
                )
            },
        );
    }

    #[test]
    #[ignore = "requires real DRM render node and Vulkan"]
    fn native_resize_capture_retargets_without_pixel_jump() {
        let (vk, _gles, _) = setup();
        let mut renderer = native::NativeRenderer::new(vk).unwrap();
        // This fixture asserts pixels/lifetimes after bounded format preparation.
        let resize_source = crate::render_helpers::shaders::animation_source(
            crate::render_helpers::shaders::ProgramType::Resize,
            None,
        );
        renderer
            .wait_for_animation_program_for_test(
                crate::render_helpers::shaders::ProgramType::Resize,
                &resize_source,
            )
            .unwrap();
        renderer
            .prepare_output_format_for_test(Fourcc::Abgr8888)
            .unwrap();
        exercise_capture(
            &mut renderer,
            |renderer, area, scale, (prev, size_prev), (next, size_next), progress| {
                ResizeRenderElement::new_native(
                    renderer,
                    None,
                    area,
                    scale,
                    prev,
                    size_prev,
                    next,
                    size_next,
                    progress,
                    progress,
                    Default::default(),
                    false,
                    1.,
                )
                .unwrap()
            },
        );
    }

    #[test]
    #[ignore = "requires real DRM render node, Vulkan and GLES DMA-BUF producer"]
    fn frozen_resize_snapshot_survives_client_reuse_and_scale_change() {
        use clarion_config::BlockOutFrom;
        use smithay::backend::renderer::element::Kind;
        use smithay::backend::renderer::ImportDma;

        use crate::layout::LayoutElementRenderSnapshot;
        use crate::render_helpers::snapshot::RenderSnapshot;
        use crate::render_helpers::solid_color::SolidColorBuffer;
        use crate::render_helpers::texture::{TextureBuffer, TextureRenderElement};
        use crate::render_helpers::{BakedBuffer, RenderCtx};

        let (vk, mut gles, mut allocator) = setup();
        let mods = modifiers(&vk, &gles, Fourcc::Argb8888);
        let mut source = allocator
            .create_buffer(16, 12, Fourcc::Argb8888, &mods)
            .unwrap()
            .export()
            .unwrap();
        let repaint = |renderer: &mut GlesRenderer, source: &mut Dmabuf, color: [f32; 4]| {
            let mut target = renderer.bind(source).unwrap();
            let mut frame = renderer
                .render(&mut target, (16, 12).into(), Transform::Normal)
                .unwrap();
            frame
                .clear(color.into(), &[Rectangle::from_size((16, 12).into())])
                .unwrap();
            frame.finish().unwrap().wait().unwrap();
        };
        repaint(&mut gles, &mut source, [1., 0., 0., 1.]);
        let mut native = native::NativeRenderer::new(vk).unwrap();
        // This fixture asserts pixels/lifetimes after bounded format preparation.
        native
            .prepare_output_format_for_test(Fourcc::Abgr8888)
            .unwrap();

        macro_rules! freeze {
            ($renderer:expr, $method:ident) => {{
                let texture = $renderer.import_dmabuf(&source, None).unwrap();
                let snapshot: LayoutElementRenderSnapshot<_> = RenderSnapshot {
                    contents: vec![BakedBuffer {
                        buffer: TextureBuffer::from_texture(
                            $renderer,
                            texture,
                            1.,
                            Transform::Normal,
                            Vec::new(),
                        ),
                        location: (-4., -4.).into(),
                        src: None,
                        dst: None,
                    }],
                    contents_with_blocked_out_bg: None,
                    blocked_out_contents: vec![BakedBuffer {
                        buffer: SolidColorBuffer::new((16., 12.), [0., 0., 0., 1.]),
                        location: (0., 0.).into(),
                        src: None,
                        dst: None,
                    }],
                    block_out_from: Some(BlockOutFrom::ScreenCapture),
                    size: (16., 12.).into(),
                    texture: Default::default(),
                    texture_with_blocked_out_bg: Default::default(),
                    blocked_out_texture: Default::default(),
                };
                snapshot.$method($renderer, 1.25.into()).unwrap()
            }};
        }
        // Raw imported DMA-BUFs have no Wayland owner, so native finish keeps
        // its synchronous fallback even through the native snapshot API.
        let frozen_native = freeze!(&mut native, freeze_native);
        let frozen_gles = freeze!(&mut gles, freeze);
        // The producer reuses the released storage after the owned caches were
        // populated, and both snapshots move to a different output scale.
        repaint(&mut gles, &mut source, [0., 1., 0., 1.]);
        macro_rules! check {
            ($renderer:expr, $snapshot:expr) => {{
                for target in [RenderTarget::Output, RenderTarget::Screencast, RenderTarget::ScreenCapture] {
                    let (texture, geo) = $snapshot.texture(RenderCtx {
                        renderer: $renderer,
                        target,
                        xray: None,
                    }, 2.0.into()).unwrap();
                    let cached_scale = $snapshot.texture_scale(target).unwrap();
                    let expected_scale = if target == RenderTarget::Output { 1.25 } else { 2.0 };
                    assert_eq!(cached_scale, Scale::from(expected_scale));
                    let expected_location = if target == RenderTarget::Output { (-5, -5) } else { (0, 0) };
                    assert_eq!(*geo, Rectangle::new(expected_location.into(),
                        Size::<f64, Logical>::from((16., 12.)).to_physical_precise_round(cached_scale)));
                    let element = TextureRenderElement::from_texture_buffer(
                        TextureBuffer::from_texture($renderer, texture.clone(), cached_scale, Transform::Normal, Vec::new()),
                        (0., 0.), 1., None, None, Kind::Unspecified);
                    let pixels = render_to_vec($renderer, (32, 24).into(), 2.0.into(), Transform::Normal,
                        Fourcc::Abgr8888, std::iter::once(&element)).unwrap();
                    let expected = if target == RenderTarget::Output { [255, 0, 0, 255] } else { [0, 0, 0, 255] };
                    assert!(pixels.as_chunks::<4>().0.iter().all(|pixel| pixel.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 1)),
                        "frozen snapshot must retain its old pixels and target privacy at scale 2: {target:?}");
                }
            }};
        }
        check!(&mut native, frozen_native);
        check!(&mut gles, frozen_gles);
    }
}

use smithay::backend::allocator::dmabuf::AsDmabuf;
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice};
use smithay::backend::allocator::{Allocator, Buffer, Fourcc, Modifier};
use smithay::backend::egl::{EGLContext, EGLDisplay};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::{
    Bind, Blit, Color32F, ExportMem, Frame, Offscreen, Renderer, TextureFilter,
};
use smithay::utils::{Rectangle, Transform};

use super::*;

pub(crate) fn setup() -> (
    VulkanRenderer,
    GlesRenderer,
    GbmAllocator<Arc<std::fs::File>>,
) {
    let path = std::env::var("CLARION_VULKAN_TEST_NODE")
        .expect("set CLARION_VULKAN_TEST_NODE to a render node");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    let node = DrmNode::from_file(&file).unwrap();
    let mut vk = VulkanRenderer::new();
    vk.init_from_drm_node(Some(&node)).unwrap();
    // Client device discovery must describe the selected GPU, including the
    // NVIDIA row of the matrix; a valid-looking primary or other render node
    // would silently force EGL clients onto a different path.
    let advertised_device = vk.ctx.as_ref().unwrap().drm_render_device_id().unwrap();
    assert_eq!(advertised_device, Some(node.dev_id()));
    eprintln!("HARDWARE: {} ({path})", vk.info.device_name);
    let gbm = GbmDevice::new(Arc::new(file)).unwrap();
    let display = unsafe { EGLDisplay::new(gbm.clone()).unwrap() };
    let egl = EGLContext::new(&display).unwrap();
    let gles = unsafe { GlesRenderer::new(egl).unwrap() };
    (vk, gles, GbmAllocator::new(gbm, GbmBufferFlags::RENDERING))
}

fn modifiers(vk: &VulkanRenderer, gles: &GlesRenderer, code: Fourcc) -> Vec<Modifier> {
    let formats = gles
        .egl_context()
        .dmabuf_render_formats()
        .iter()
        .copied()
        .filter(|f| f.code == code && vk.supports_target_format(*f))
        .map(|f| f.modifier)
        .collect::<Vec<_>>();
    assert!(
        !formats.is_empty(),
        "no shared Vulkan/EGL modifier for {code:?}"
    );
    formats
}

fn pixels(gles: &mut GlesRenderer, dmabuf: &mut Dmabuf) -> Vec<u8> {
    let size = dmabuf.size();
    // Blit through RGBA8: Smithay does not expose direct readback for every
    // valid 10-bit DRM framebuffer format.
    let mut rgba: smithay::backend::renderer::gles::GlesTexture =
        gles.create_buffer(Fourcc::Abgr8888, size).unwrap();
    let target = gles.bind(dmabuf).unwrap();
    let mut readable = gles.bind(&mut rgba).unwrap();
    let rect = Rectangle::from_size(size.to_logical(1, Transform::Normal).to_physical(1));
    gles.blit(&target, &mut readable, rect, rect, TextureFilter::Nearest)
        .unwrap()
        .wait()
        .unwrap();
    let map = gles
        .copy_framebuffer(&readable, Rectangle::from_size(size), Fourcc::Abgr8888)
        .unwrap();
    gles.map_texture(&map).unwrap().to_vec()
}

fn intel_primary_node(ctx: &VulkanContext) -> Option<std::path::PathBuf> {
    use smithay::backend::drm::NodeType;
    let vendor = unsafe {
        ctx.instance
            .get_physical_device_properties(ctx.physical_device)
    }
    .vendor_id;
    if vendor != 0x8086 {
        return None;
    }
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
    // Allocate on the same physical GPU selected by setup(), irrespective of
    // card numbering or a /dev/dri/by-path alias supplied by the test runner.
    if device(&selected)? != device(&primary)? {
        return None;
    }
    primary.dev_path()
}

#[test]
#[ignore = "requires real DRM render node and Vulkan"]
fn scaled_capture_pool_reuses_images_and_releases_context() {
    for drop_renderer_before_finish in [false, true] {
        let weak_context = {
            let (renderer, _gles, _allocator) = setup();
            let ctx = renderer.ctx.as_ref().unwrap();
            let weak_context = Arc::downgrade(ctx);
            let make_target = |size| {
                allocate_texture(
                    ctx,
                    size,
                    size,
                    vk::Format::R8G8B8A8_UNORM,
                    vk::ImageTiling::OPTIMAL,
                    vk::ImageUsageFlags::COLOR_ATTACHMENT
                        | vk::ImageUsageFlags::SAMPLED
                        | vk::ImageUsageFlags::TRANSFER_SRC
                        | vk::ImageUsageFlags::TRANSFER_DST,
                    vk::ImageLayout::UNDEFINED,
                    true,
                )
                .unwrap()
            };
            let target = make_target(8);
            let captured = make_target(2);
            let mut pooled_image = None;
            for _ in 0..2 {
                let mut frame = renderer
                    .render_frame(
                        target.image(),
                        target.image_view(),
                        vk::Format::R8G8B8A8_UNORM,
                        true,
                        8,
                        8,
                        Some([1., 0., 0., 1.]),
                    )
                    .unwrap();
                frame
                    .capture_scaled_into(&captured, Rectangle::new((2, 2).into(), (4, 4).into()))
                    .unwrap();
                frame.finish_and_wait().unwrap();
                let pool = renderer.scratch_textures.lock().unwrap();
                assert_eq!(pool.len(), 1);
                let image = pool[0].image();
                assert_eq!(*pooled_image.get_or_insert(image), image);
            }
            if drop_renderer_before_finish {
                let mut frame = renderer
                    .render_frame(
                        target.image(),
                        target.image_view(),
                        vk::Format::R8G8B8A8_UNORM,
                        true,
                        8,
                        8,
                        Some([0., 1., 0., 1.]),
                    )
                    .unwrap();
                frame
                    .capture_scaled_into(&captured, Rectangle::new((2, 2).into(), (4, 4).into()))
                    .unwrap();
                // A frame owns its pool independently of the renderer lifetime.
                drop(renderer);
                frame.finish_and_wait().unwrap();
            }
            weak_context
        };
        assert!(
            weak_context.upgrade().is_none(),
            "pooled capture textures must not retain the Vulkan context after teardown"
        );
    }
}

// A future host timeline signal legally blocks ordinary submissions. Do not
// export a SYNC_FD from this dependency: export requires its resolving semaphore
// signal operation to have already been submitted.
struct BlockedQueue {
    ctx: Arc<VulkanContext>,
    semaphore: vk::Semaphore,
}

pub(crate) struct SubmittedGpuWork {
    ctx: Arc<VulkanContext>,
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    signal: vk::Semaphore,
    command: vk::CommandBuffer,
    fence: vk::Fence,
    submitted: bool,
}
impl Drop for SubmittedGpuWork {
    fn drop(&mut self) {
        unsafe {
            if self.submitted {
                let _ = self
                    .ctx
                    .device
                    .wait_for_fences(&[self.fence], true, u64::MAX);
            }
            let _lock = self.ctx.command_lock.lock().unwrap();
            if self.command != vk::CommandBuffer::null() {
                self.ctx
                    .device
                    .free_command_buffers(self.ctx.command_pool, &[self.command]);
            }
            self.ctx.device.destroy_fence(self.fence, None);
            self.ctx.device.destroy_semaphore(self.signal, None);
            self.ctx.device.destroy_buffer(self.buffer, None);
            self.ctx.device.free_memory(self.memory, None);
        }
    }
}

impl SubmittedGpuWork {
    pub(crate) fn new(ctx: Arc<VulkanContext>) -> Self {
        let mut work = SubmittedGpuWork {
            ctx: ctx.clone(),
            buffer: vk::Buffer::null(),
            memory: vk::DeviceMemory::null(),
            signal: vk::Semaphore::null(),
            command: vk::CommandBuffer::null(),
            fence: vk::Fence::null(),
            submitted: false,
        };
        unsafe {
            // Keep GPU busy with already-submitted work, not unresolved future
            // semaphore signals (which cannot legally be exported as SYNC_FD).
            let size = 16 * 1024 * 1024;
            work.buffer = ctx
                .device
                .create_buffer(
                    &vk::BufferCreateInfo::default()
                        .size(size)
                        .usage(vk::BufferUsageFlags::TRANSFER_DST),
                    None,
                )
                .unwrap();
            let req = ctx.device.get_buffer_memory_requirements(work.buffer);
            let props = ctx
                .instance
                .get_physical_device_memory_properties(ctx.physical_device);
            let index = (0..props.memory_type_count)
                .find(|i| {
                    req.memory_type_bits & (1 << i) != 0
                        && props.memory_types[*i as usize]
                            .property_flags
                            .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
                })
                .unwrap();
            work.memory = ctx
                .device
                .allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(req.size)
                        .memory_type_index(index),
                    None,
                )
                .unwrap();
            ctx.device
                .bind_buffer_memory(work.buffer, work.memory, 0)
                .unwrap();
            let mut export = vk::ExportSemaphoreCreateInfo::default()
                .handle_types(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
            work.signal = ctx
                .device
                .create_semaphore(
                    &vk::SemaphoreCreateInfo::default().push_next(&mut export),
                    None,
                )
                .unwrap();
            work.fence = ctx
                .device
                .create_fence(&vk::FenceCreateInfo::default(), None)
                .unwrap();
            let _lock = ctx.command_lock.lock().unwrap();
            work.command = ctx
                .device
                .allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default()
                        .command_pool(ctx.command_pool)
                        .level(vk::CommandBufferLevel::PRIMARY)
                        .command_buffer_count(1),
                )
                .unwrap()[0];
            ctx.device
                .begin_command_buffer(work.command, &vk::CommandBufferBeginInfo::default())
                .unwrap();
            for i in 0..2048 {
                ctx.device
                    .cmd_fill_buffer(work.command, work.buffer, 0, size, i);
                let barrier = vk::BufferMemoryBarrier::default()
                    .buffer(work.buffer)
                    .size(size)
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED);
                ctx.device.cmd_pipeline_barrier(
                    work.command,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[barrier],
                    &[],
                );
            }
            ctx.device.end_command_buffer(work.command).unwrap();
        }
        work
    }

    pub(crate) fn submit(&mut self) -> std::os::fd::OwnedFd {
        use std::os::fd::FromRawFd;
        unsafe {
            let _lock = self.ctx.command_lock.lock().unwrap();
            self.ctx
                .device
                .queue_submit(
                    self.ctx.graphics_queue,
                    &[vk::SubmitInfo::default()
                        .command_buffers(&[self.command])
                        .signal_semaphores(&[self.signal])],
                    self.fence,
                )
                .unwrap();
            self.submitted = true;
            let api =
                ash::khr::external_semaphore_fd::Device::new(&self.ctx.instance, &self.ctx.device);
            let raw = api
                .get_semaphore_fd(
                    &vk::SemaphoreGetFdInfoKHR::default()
                        .semaphore(self.signal)
                        .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD),
                )
                .unwrap();
            assert!(raw >= 0);
            std::os::fd::OwnedFd::from_raw_fd(raw)
        }
    }

    pub(crate) fn wait(&self) {
        unsafe {
            self.ctx
                .device
                .wait_for_fences(&[self.fence], true, 30_000_000_000)
                .unwrap();
        }
    }

    pub(crate) fn is_reached(&self) -> bool {
        unsafe { self.ctx.device.get_fence_status(self.fence).unwrap() }
    }
}

#[test]
#[ignore = "requires real DRM render node and Vulkan native fences"]
fn native_idle_gpu_completion_releases_resources_with_live_sync_point() {
    native_idle_gpu_completion(false);
}

#[test]
#[ignore = "requires real DRM render node and Vulkan native fences"]
fn native_renderer_teardown_preserves_and_releases_in_flight_resources() {
    native_idle_gpu_completion(true);
}

fn native_idle_gpu_completion(teardown_before_completion: bool) {
    use std::time::{Duration, Instant};

    use super::native::NativeRenderer;

    let (vk, _gles, _allocator) = setup();
    let ctx = vk.ctx.as_ref().unwrap().clone();
    let mut renderer = NativeRenderer::new(vk).unwrap();
    // This fixture exercises prepared rendering; wait on the bounded test-only
    // preparer before its pixel, timing or ownership assertions begin.
    renderer
        .prepare_output_format_for_test(Fourcc::Abgr8888)
        .unwrap();
    let mut event_loop = smithay::reexports::calloop::EventLoop::<()>::try_new().unwrap();
    renderer
        .install_completion_wakeup(&event_loop.handle())
        .unwrap();
    let mut target = renderer
        .create_buffer(Fourcc::Abgr8888, (8, 8).into())
        .unwrap();
    let weak_target = Arc::downgrade(&target.inner);
    let mut fb = renderer.bind(&mut target).unwrap();
    let mut frame = renderer
        .render(&mut fb, (8, 8).into(), Transform::Normal)
        .unwrap();
    frame
        .clear(Color32F::BLACK, &[Rectangle::from_size((8, 8).into())])
        .unwrap();
    let mut work = SubmittedGpuWork::new(ctx);
    let _producer_fd = work.submit();
    let sync = frame.finish().unwrap();
    assert!(
        !sync.is_reached(),
        "the GPU submission must genuinely remain pending"
    );
    drop(fb);
    drop(target);
    assert!(
        weak_target.upgrade().is_some(),
        "pending GPU target released too soon"
    );
    let before_drop = Instant::now();
    let renderer = (!teardown_before_completion).then_some(renderer);
    if teardown_before_completion {
        assert!(
            before_drop.elapsed() < Duration::from_millis(500),
            "renderer teardown blocked the main thread on pending GPU work"
        );
        assert!(weak_target.upgrade().is_some());
    }
    work.wait();
    sync.wait().unwrap();
    // Deliberately keep both the renderer and its external SyncPoint alive and
    // make no further renderer calls: an idle desktop must release GPU owners.
    let wake = Instant::now();
    event_loop
        .dispatch(Duration::from_secs(2), &mut ())
        .unwrap();
    assert!(
        wake.elapsed() < Duration::from_secs(1),
        "GPU completion did not wake the idle event loop"
    );
    assert!(weak_target.upgrade().is_none(), "idle Vulkan submission retained completed target while renderer and SyncPoint stayed alive");
    assert!(sync.is_reached());
    assert!(sync.export().is_some());
    drop(renderer);
    assert_completion_loop_idle(&mut event_loop);
}

fn assert_completion_loop_idle(event_loop: &mut smithay::reexports::calloop::EventLoop<'_, ()>) {
    use std::time::{Duration, Instant};
    // The worker's last Ping can close just after dispatch observes its final
    // completion ping. Allow that one close event, then require a quiet loop;
    // a periodic completion timer would keep waking it until this deadline.
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        let idle = Instant::now();
        event_loop
            .dispatch(Duration::from_millis(30), &mut ())
            .unwrap();
        if idle.elapsed() >= Duration::from_millis(25) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "completion source keeps waking an idle event loop"
        );
    }
}

impl BlockedQueue {
    fn new(ctx: Arc<VulkanContext>) -> Self {
        assert!(ctx.info.supports_timeline_semaphores);
        let mut timeline =
            vk::SemaphoreTypeCreateInfo::default().semaphore_type(vk::SemaphoreType::TIMELINE);
        let semaphore = unsafe {
            ctx.device
                .create_semaphore(
                    &vk::SemaphoreCreateInfo::default().push_next(&mut timeline),
                    None,
                )
                .unwrap()
        };
        {
            let _lock = ctx.command_lock.lock().unwrap();
            let mut wait = vk::TimelineSemaphoreSubmitInfo::default().wait_semaphore_values(&[1]);
            unsafe {
                ctx.device
                    .queue_submit(
                        ctx.graphics_queue,
                        &[vk::SubmitInfo::default()
                            .push_next(&mut wait)
                            .wait_semaphores(&[semaphore])
                            .wait_dst_stage_mask(&[vk::PipelineStageFlags::ALL_COMMANDS])],
                        vk::Fence::null(),
                    )
                    .unwrap();
            }
        }
        Self { ctx, semaphore }
    }
}

impl Drop for BlockedQueue {
    fn drop(&mut self) {
        let _lock = self.ctx.command_lock.lock().unwrap();
        unsafe {
            self.ctx
                .device
                .signal_semaphore(
                    &vk::SemaphoreSignalInfo::default()
                        .semaphore(self.semaphore)
                        .value(1),
                )
                .unwrap();
            self.ctx
                .device
                .queue_wait_idle(self.ctx.graphics_queue)
                .unwrap();
            self.ctx.device.destroy_semaphore(self.semaphore, None);
        }
    }
}

#[test]
#[ignore = "requires real DRM render node, Vulkan and timeline semaphores"]
fn fence_wait_does_not_block_later_queue_progress() {
    let (renderer, _gles, _allocator) = setup();
    let ctx = renderer.ctx.as_ref().unwrap().clone();
    let target = allocate_texture(
        &ctx,
        8,
        8,
        vk::Format::R8G8B8A8_UNORM,
        vk::ImageTiling::OPTIMAL,
        vk::ImageUsageFlags::COLOR_ATTACHMENT,
        vk::ImageLayout::UNDEFINED,
        true,
    )
    .unwrap();
    let mut frame = renderer
        .render_frame(
            target.image(),
            target.image_view(),
            vk::Format::R8G8B8A8_UNORM,
            true,
            8,
            8,
            Some([0.; 4]),
        )
        .unwrap();
    frame.retain_target(&target);

    // This queue dependency cannot complete until another thread acquires the
    // command lock and signals the timeline semaphore. A fence wait that holds
    // the lock will time out, even though the GPU itself is healthy.
    let blocked = BlockedQueue::new(ctx);
    let release = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(50));
        drop(blocked);
    });
    let result = frame.finish_and_wait();
    release.join().unwrap();
    assert_eq!(result, Ok(()));
}

#[test]
#[ignore = "requires real DRM render node, Vulkan and timeline semaphores"]
fn transfer_wait_does_not_block_later_queue_progress() {
    for readback in [true, false] {
        let (renderer, _gles, _allocator) = setup();
        let initial = (0..32u8)
            .flat_map(|i| [i * 3, i * 5, 255 - i, 255])
            .collect::<Vec<_>>();
        let texture = renderer
            .import_memory_texture(&initial, Fourcc::Abgr8888, 8, 4, 32)
            .unwrap();
        let whole = Rectangle::from_size((8, 4).into());
        let patch = Rectangle::new((2, 1).into(), (4, 2).into());
        let updated = [29, 83, 157, 255].repeat(32);
        let mut expected = initial.clone();
        if !readback {
            for y in 1..3 {
                let start = (y * 8 + 2) * 4;
                expected[start..start + 16].copy_from_slice(&updated[start..start + 16]);
            }
        }

        // A healthy queue waits for a host signal from another thread. Its
        // release needs command_lock, so holding that lock across the transfer
        // fence wait creates a timeout instead of allowing producer progress.
        let ctx = renderer.ctx.as_ref().unwrap().clone();
        let pooled = *ctx
            .idle_commands
            .lock()
            .unwrap()
            .last()
            .expect("initial texture upload must return its command/fence to the pool");
        let blocked = BlockedQueue::new(ctx.clone());
        let release = std::thread::spawn(move || {
            use std::time::{Duration, Instant};
            // Observe the real transfer taking its command/fence before the
            // delay. Scheduler load must not let the producer signal before
            // this operation starts and accidentally make the old lock pass.
            let deadline = Instant::now() + Duration::from_secs(5);
            let acquired = loop {
                if !ctx.idle_commands.lock().unwrap().contains(&pooled) {
                    break true;
                }
                if Instant::now() >= deadline {
                    break false;
                }
                std::thread::sleep(Duration::from_millis(1));
            };
            if acquired {
                std::thread::sleep(Duration::from_millis(50));
            }
            drop(blocked);
            assert!(
                acquired,
                "transfer did not acquire the pooled command/fence"
            );
        });
        let result = if readback {
            renderer.read_memory_texture(&texture, whole)
        } else {
            renderer
                .update_memory_texture(&texture, &updated, 32, patch)
                .map(|_| Vec::new())
        };
        release.join().unwrap();
        let pixels = result.unwrap_or_else(|error| {
            panic!("transfer readback={readback} blocked producer progress: {error:?}")
        });
        if readback {
            assert_eq!(pixels, expected);
        }
        assert_eq!(
            renderer.read_memory_texture(&texture, whole).unwrap(),
            expected,
            "transfer readback={readback} changed pixels outside the ordered update"
        );
    }
}

#[test]
#[ignore = "requires real DRM render node and Vulkan validation"]
fn frame_packs_interleaved_meshes_and_reuses_draw_bindings() {
    let (renderer, _gles, _allocator) = setup();
    let ctx = renderer.ctx.as_ref().unwrap();
    let target = allocate_texture(
        ctx,
        16,
        8,
        vk::Format::R8G8B8A8_UNORM,
        vk::ImageTiling::OPTIMAL,
        vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
        vk::ImageLayout::UNDEFINED,
        true,
    )
    .unwrap();
    let red = renderer.create_solid_texture([255, 0, 0, 255]).unwrap();
    let green = renderer.create_solid_texture([0, 255, 0, 255]).unwrap();
    let blue = renderer.create_solid_texture([0, 0, 255, 255]).unwrap();
    let rectangle = |x0: f32, x1: f32| {
        let vertex = |x, y| pipeline::MeshGpuVertex {
            position: [x, y],
            uv: [0.5, 0.5],
        };
        vec![
            vertex(x0, -1.),
            vertex(x1, -1.),
            vertex(x0, 1.),
            vertex(x1, -1.),
            vertex(x1, 1.),
            vertex(x0, 1.),
        ]
    };
    let mut frame = renderer
        .render_frame(
            target.image(),
            target.image_view(),
            vk::Format::R8G8B8A8_UNORM,
            true,
            16,
            8,
            Some([0.; 4]),
        )
        .unwrap();
    frame
        .draw_mesh(
            &red,
            pipeline::MeshPushConstants::default(),
            rectangle(-1., -0.5),
        )
        .unwrap();
    frame
        .draw_mesh(
            &red,
            pipeline::MeshPushConstants::default(),
            rectangle(-0.5, 0.),
        )
        .unwrap();
    frame
        .draw_texture(&green, 8., 0., 4., 8., None, 1., None, 0.)
        .unwrap();
    frame
        .draw_texture(&green, 12., 0., 2., 8., None, 1., None, 0.)
        .unwrap();
    frame
        .draw_mesh(
            &blue,
            pipeline::MeshPushConstants::default(),
            rectangle(0.75, 1.),
        )
        .unwrap();
    frame.finish_and_wait().unwrap();
    // `render_frame` accepts raw image handles, so the caller owns the target
    // layout state and must publish the layout reached by the completed frame.
    target.current_layout().set(vk::ImageLayout::GENERAL);
    let pixels = renderer
        .read_memory_texture(&target, Rectangle::from_size((16, 8).into()))
        .unwrap();
    for (index, pixel) in pixels.as_chunks::<4>().0.iter().enumerate() {
        let expected = match index % 16 {
            0..=7 => [255, 0, 0, 255],
            8..=13 => [0, 255, 0, 255],
            _ => [0, 0, 255, 255],
        };
        assert_eq!(*pixel, expected, "pixel ({}, {})", index % 16, index / 16);
    }
    assert_eq!(ctx.vertex_buffers.lock().unwrap().len(), 1);
}

#[test]
#[ignore = "requires real DRM render node and Vulkan"]
fn fence_timeout_keeps_submitted_frame_resources_alive() {
    {
        let (renderer, _gles, _allocator) = setup();
        let ctx = renderer.ctx.as_ref().unwrap().clone();
        let target = allocate_texture(
            &ctx,
            8,
            8,
            vk::Format::R8G8B8A8_UNORM,
            vk::ImageTiling::OPTIMAL,
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
            vk::ImageLayout::UNDEFINED,
            true,
        )
        .unwrap();
        let source = renderer.create_solid_texture([255; 4]).unwrap();
        let captured = allocate_texture(
            &ctx,
            2,
            2,
            vk::Format::R8G8B8A8_UNORM,
            vk::ImageTiling::OPTIMAL,
            vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
            vk::ImageLayout::UNDEFINED,
            true,
        )
        .unwrap();
        let weak_textures = [&target, &source, &captured].map(|t| Arc::downgrade(&t.inner));
        let weak_pipeline = Arc::downgrade(renderer.pipeline_mgr.as_ref().unwrap());
        let mut frame = renderer
            .render_frame(
                target.image(),
                target.image_view(),
                vk::Format::R8G8B8A8_UNORM,
                true,
                8,
                8,
                Some([0.; 4]),
            )
            .unwrap();
        frame.retain_target(&target);
        frame
            .draw_texture(&source, 0., 0., 8., 8., None, 1., None, 0.)
            .unwrap();
        frame
            .capture_scaled_into(&captured, Rectangle::from_size((4, 4).into()))
            .unwrap();
        drop(source);
        drop(captured);
        drop(renderer);
        let blocked = BlockedQueue::new(ctx);
        drop(target);
        assert_eq!(frame.finish_and_wait(), Err(vk::Result::TIMEOUT));
        for texture in weak_textures {
            assert!(
                texture.upgrade().is_some(),
                "submitted image was freed on timeout"
            );
        }
        assert!(weak_pipeline.upgrade().is_some());
        // Let the pending draws actually execute while validation checks that
        // none of their images, memory, descriptors or pipelines were destroyed.
        drop(blocked);
    }
}

#[test]
#[ignore = "requires real DRM render node and Vulkan"]
fn fence_timeout_keeps_transfer_texture_alive() {
    for readback in [false, true] {
        let (renderer, _gles, _allocator) = setup();
        let texture = renderer
            .import_memory_texture(&[255; 16], Fourcc::Abgr8888, 2, 2, 8)
            .unwrap();
        let weak_texture = Arc::downgrade(&texture.inner);
        let blocked = BlockedQueue::new(renderer.ctx.as_ref().unwrap().clone());
        let region = Rectangle::from_size((2, 2).into());
        let result = if readback {
            renderer.read_memory_texture(&texture, region).map(|_| ())
        } else {
            renderer.update_memory_texture(&texture, &[0; 16], 8, region)
        };
        assert_eq!(result, Err(vk::Result::TIMEOUT));
        drop(texture);
        drop(renderer);
        assert!(
            weak_texture.upgrade().is_some(),
            "submitted transfer texture was freed on timeout"
        );
        drop(blocked);
    }
}

#[test]
#[ignore = "requires real DRM render node and Vulkan"]
fn timeout_preserves_submitted_layouts_for_readback_and_retry() {
    let (renderer, _gles, _allocator) = setup();
    let ctx = renderer.ctx.as_ref().unwrap();
    let make_texture = || {
        allocate_texture(
            ctx,
            4,
            4,
            vk::Format::R8G8B8A8_UNORM,
            vk::ImageTiling::OPTIMAL,
            vk::ImageUsageFlags::COLOR_ATTACHMENT
                | vk::ImageUsageFlags::SAMPLED
                | vk::ImageUsageFlags::TRANSFER_SRC
                | vk::ImageUsageFlags::TRANSFER_DST,
            vk::ImageLayout::UNDEFINED,
            true,
        )
        .unwrap()
    };
    let target = make_texture();
    let captured = make_texture();
    let region = Rectangle::from_size((4, 4).into());
    let mut frame = renderer
        .render_frame(
            target.image(),
            target.image_view(),
            vk::Format::R8G8B8A8_UNORM,
            true,
            4,
            4,
            Some([1., 0., 0., 1.]),
        )
        .unwrap();
    frame.retain_target(&target);
    frame.capture_into(&captured, 0, 0).unwrap();
    let blocked = BlockedQueue::new(ctx.clone());
    assert_eq!(frame.finish_and_wait(), Err(vk::Result::TIMEOUT));
    drop(blocked);
    assert_eq!(target.current_layout().get(), vk::ImageLayout::GENERAL);
    assert_eq!(
        captured.current_layout().get(),
        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
    );
    for texture in [&target, &captured] {
        let pixels = renderer.read_memory_texture(texture, region).unwrap();
        assert!(pixels
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| *pixel == [255, 0, 0, 255]));
    }

    // A first upload can time out while the queue still executes its transition.
    let uploaded = make_texture();
    let blocked = BlockedQueue::new(ctx.clone());
    assert_eq!(
        renderer.update_memory_texture(&uploaded, &[255; 64], 16, region),
        Err(vk::Result::TIMEOUT)
    );
    drop(blocked);
    assert_eq!(
        uploaded.current_layout().get(),
        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
    );
    assert_eq!(
        renderer.read_memory_texture(&uploaded, region).unwrap(),
        vec![255; 64]
    );
    // Retry only one pixel: the rest of the first upload must survive.
    let mut data = [255; 64];
    data[..4].copy_from_slice(&[0, 255, 0, 255]);
    renderer
        .update_memory_texture(&uploaded, &data, 16, Rectangle::from_size((1, 1).into()))
        .unwrap();
    assert_eq!(
        renderer.read_memory_texture(&uploaded, region).unwrap(),
        data
    );
}

#[test]
#[ignore = "requires real DRM render node, EGL readback and Vulkan"]
fn native_memory_texture_to_scanout_pixels() {
    // GLES is only the independent readback oracle, never the source uploader
    // or scene renderer. Test padded source rows and both channel orders.
    let (mut renderer, mut oracle, mut allocator) = setup();
    for format in [
        Fourcc::Abgr8888,
        Fourcc::Argb8888,
        Fourcc::Xbgr8888,
        Fourcc::Xrgb8888,
    ] {
        let opaque = matches!(format, Fourcc::Xbgr8888 | Fourcc::Xrgb8888);
        let bgra = matches!(format, Fourcc::Argb8888 | Fourcc::Xrgb8888);
        let expected = [
            [64, 16, 32, if opaque { 255 } else { 128 }],
            [8, 48, 96, if opaque { 255 } else { 128 }],
        ];
        let mut source = vec![0xee; 80 * 15 + 64];
        for y in 0..16 {
            for x in 0..16 {
                let mut color = expected[y / 8];
                if opaque {
                    color[3] = 0;
                }
                if bgra {
                    color.swap(0, 2);
                }
                source[y * 80 + x * 4..y * 80 + x * 4 + 4].copy_from_slice(&color);
            }
        }
        let texture = renderer
            .import_memory_texture(&source, format, 16, 16, 80)
            .unwrap();
        drop(source);
        assert_eq!(
            smithay::backend::renderer::Texture::format(&texture),
            Some(format)
        );
        let mut target = allocator
            .create_buffer(
                16,
                16,
                Fourcc::Argb8888,
                &modifiers(&renderer, &oracle, Fourcc::Argb8888),
            )
            .unwrap()
            .export()
            .unwrap();
        renderer
            .render_to_target_dmabuf(&target, Some([0.; 4]), |frame| {
                frame.draw_texture(&texture, 0., 0., 16., 16., None, 1., None, 0.)
            })
            .unwrap();
        let actual = pixels(&mut oracle, &mut target);
        for color in expected {
            let matches = actual
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|p| p.iter().zip(color).all(|(a, b)| a.abs_diff(b) <= 2))
                .count();
            assert_eq!(
                matches, 128,
                "wrong native pixels for {format:?}: {color:?}"
            );
        }
    }
}

#[test]
#[ignore = "requires real DRM render node, EGL readback and Vulkan"]
fn native_frame_scissor_and_ordered_transparent_clear() {
    let (mut renderer, mut oracle, mut allocator) = setup();
    let red = renderer.create_solid_texture([128, 0, 0, 128]).unwrap();
    let white = renderer.create_solid_texture([255; 4]).unwrap();
    let mut target = allocator
        .create_buffer(
            16,
            16,
            Fourcc::Argb8888,
            &modifiers(&renderer, &oracle, Fourcc::Argb8888),
        )
        .unwrap()
        .export()
        .unwrap();
    renderer
        .render_to_target_dmabuf(&target, Some([0., 0., 0., 1.]), |frame| {
            frame.clear_regions([0.; 4], &[Rectangle::new((0, 0).into(), (8, 16).into())])?;
            frame.set_clip(Some(Rectangle::new((4, 4).into(), (8, 8).into())));
            frame.draw_texture(&red, 0., 0., 16., 16., None, 1., None, 0.)?;
            frame.clear_regions([0.; 4], &[Rectangle::new((6, 6).into(), (4, 4).into())])?;
            frame.set_clip(Some(Rectangle::new((20, 20).into(), (4, 4).into())));
            frame.draw_texture(&white, 0., 0., 16., 16., None, 1., None, 0.)?;
            frame.set_clip(None);
            frame.draw_texture(&white, 14., 14., 2., 2., None, 1., None, 0.)
        })
        .unwrap();
    let actual = pixels(&mut oracle, &mut target);
    for (color, count) in [
        ([0, 0, 0, 0], 112),
        ([128, 0, 0, 128], 24),
        ([128, 0, 0, 255], 24),
        ([0, 0, 0, 255], 92),
        ([255; 4], 4),
    ] {
        let matches = actual
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p.iter().zip(color).all(|(a, b)| a.abs_diff(b) <= 2))
            .count();
        assert_eq!(
            matches, count,
            "native frame clear/scissor mismatch: {color:?}"
        );
    }
}

#[test]
#[ignore = "requires real DRM render node, EGL reference and Vulkan"]
fn native_smithay_renderer_matches_gles_transforms() {
    use smithay::backend::renderer::ImportMem;

    use super::native::NativeRenderer;
    let (vk, mut reference, mut allocator) = setup();
    let source = [
        255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255, 0, 255, 255, 255, 255, 0,
        255, 255, 80, 120, 160, 255, 200, 80, 20, 255,
    ];
    let mods = modifiers(&vk, &reference, Fourcc::Argb8888);
    let mut native = NativeRenderer::new(vk).unwrap();
    native.upscale_filter(TextureFilter::Nearest).unwrap();
    reference.upscale_filter(TextureFilter::Nearest).unwrap();
    let transforms = [
        Transform::Normal,
        Transform::_90,
        Transform::_180,
        Transform::_270,
        Transform::Flipped,
        Transform::Flipped90,
        Transform::Flipped180,
        Transform::Flipped270,
    ];
    let output_size: smithay::utils::Size<i32, smithay::utils::Physical> = (24, 16).into();
    for flipped in [false, true] {
        let texture = native
            .import_memory(&source, Fourcc::Abgr8888, (4, 2).into(), flipped)
            .unwrap();
        // The current GLES y_inverted matrix negates v without translating it,
        // clamping all negative coordinates to the edge. Build the oracle with
        // explicitly reversed rows to verify the intended flip independently.
        let flip_rows = |mut bytes: [u8; 32]| {
            if flipped {
                let (top, bottom) = bytes.split_at_mut(16);
                top.swap_with_slice(bottom);
            }
            bytes
        };
        let ref_texture = reference
            .import_memory(&flip_rows(source), Fourcc::Abgr8888, (4, 2).into(), false)
            .unwrap();
        let mut updated = source;
        updated[4..8].copy_from_slice(&[30, 180, 90, 255]);
        let region = Rectangle::new((1, 0).into(), (1, 1).into());
        native.update_memory(&texture, &updated, region).unwrap();
        let ref_region = Rectangle::new((1, if flipped { 1 } else { 0 }).into(), (1, 1).into());
        reference
            .update_memory(&ref_texture, &flip_rows(updated), ref_region)
            .unwrap();
        for output in transforms {
            let physical_size = output_size;
            let drawing_size = output.transform_size(output_size);
            for source_transform in transforms {
                let mut result = allocator
                    .create_buffer(
                        physical_size.w as u32,
                        physical_size.h as u32,
                        Fourcc::Argb8888,
                        &mods,
                    )
                    .unwrap()
                    .export()
                    .unwrap();
                let mut expected = allocator
                    .create_buffer(
                        physical_size.w as u32,
                        physical_size.h as u32,
                        Fourcc::Argb8888,
                        &mods,
                    )
                    .unwrap()
                    .export()
                    .unwrap();
                let size = source_transform.transform_size((8, 4).into());
                let dst = Rectangle::new((2, 3).into(), size);
                let damage = [Rectangle::from_size(size)];
                let src = Rectangle::new((1., 0.).into(), (3., 2.).into());
                let mut target = native.bind(&mut result).unwrap();
                let mut frame = native.render(&mut target, output_size, output).unwrap();
                assert_eq!(frame.output_size(), drawing_size);
                frame
                    .clear(Color32F::BLACK, &[Rectangle::from_size(drawing_size)])
                    .unwrap();
                frame
                    .render_texture_from_to(&texture, src, dst, &damage, &[], source_transform, 1.)
                    .unwrap();
                frame.finish().unwrap().wait().unwrap();
                drop(target);
                let mut target = reference.bind(&mut expected).unwrap();
                let mut frame = reference.render(&mut target, output_size, output).unwrap();
                Frame::clear(
                    &mut frame,
                    Color32F::BLACK,
                    &[Rectangle::from_size(drawing_size)],
                )
                .unwrap();
                Frame::render_texture_from_to(
                    &mut frame,
                    &ref_texture,
                    src,
                    dst,
                    &damage,
                    &[],
                    source_transform,
                    1.,
                )
                .unwrap();
                frame.finish().unwrap().wait().unwrap();
                drop(target);
                let result = pixels(&mut reference, &mut result);
                let expected = pixels(&mut reference, &mut expected);
                if !result
                    .iter()
                    .zip(&expected)
                    .all(|(a, b)| a.abs_diff(*b) <= 2)
                {
                    let colored = |data: &[u8]| {
                        data.as_chunks::<4>()
                            .0
                            .iter()
                            .enumerate()
                            .filter(|(_, p)| p[..3] != [0, 0, 0])
                            .map(|(i, p)| {
                                (
                                    i % physical_size.w as usize,
                                    i / physical_size.w as usize,
                                    p.to_vec(),
                                )
                            })
                            .collect::<Vec<_>>()
                    };
                    eprintln!(
                        "actual: {:?}\nexpected: {:?}",
                        colored(&result),
                        colored(&expected)
                    );
                }
                assert!(
                    result
                        .iter()
                        .zip(&expected)
                        .all(|(a, b)| a.abs_diff(*b) <= 2),
                    "native/GLES mismatch: output {output:?}, source {source_transform:?}, flipped {flipped}"
                );
            }
        }
    }
}

#[test]
#[ignore = "requires real DRM render node, EGL readback and Vulkan"]
fn native_smithay_offscreen_and_overlapping_damage() {
    use super::native::NativeRenderer;
    let (vk, mut oracle, mut allocator) = setup();
    let mods = modifiers(&vk, &oracle, Fourcc::Argb8888);
    let mut sampled_only = vk.create_solid_texture([255; 4]).unwrap();
    let mut renderer = NativeRenderer::new(vk).unwrap();
    assert!(renderer.bind(&mut sampled_only).is_err());
    let mut offscreen = renderer
        .create_buffer(Fourcc::Abgr8888, (16, 16).into())
        .unwrap();
    let mut framebuffer = renderer.bind(&mut offscreen).unwrap();
    let mut frame = renderer
        .render(&mut framebuffer, (16, 16).into(), Transform::Normal)
        .unwrap();
    let all = Rectangle::from_size((16, 16).into());
    frame.clear(Color32F::TRANSPARENT, &[all]).unwrap();
    frame
        .draw_solid(all, &[all, all], [0.5, 0., 0., 0.5].into())
        .unwrap();
    frame.finish().unwrap().wait().unwrap();
    drop(framebuffer);
    let mut target = allocator
        .create_buffer(16, 16, Fourcc::Argb8888, &mods)
        .unwrap()
        .export()
        .unwrap();
    let mut framebuffer = renderer.bind(&mut target).unwrap();
    let mut frame = renderer
        .render(&mut framebuffer, (16, 16).into(), Transform::Normal)
        .unwrap();
    frame.clear(Color32F::TRANSPARENT, &[all]).unwrap();
    frame
        .render_texture_from_to(
            &offscreen,
            Rectangle::from_size((16., 16.).into()),
            all,
            &[all],
            &[],
            Transform::Normal,
            1.,
        )
        .unwrap();
    frame.finish().unwrap().wait().unwrap();
    drop(framebuffer);
    for p in pixels(&mut oracle, &mut target).as_chunks::<4>().0 {
        assert!(
            p.iter()
                .zip([128, 0, 0, 128])
                .all(|(a, b)| a.abs_diff(b) <= 2),
            "{p:?}"
        );
    }
}

#[test]
#[ignore = "requires real DRM render node, native producer fences and Vulkan"]
fn native_frame_multiple_gpu_producers_and_abort() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use smithay::backend::renderer::sync::{Fence, Interrupted, SyncPoint};

    use super::native::NativeRenderer;
    #[derive(Debug)]
    struct Producer {
        sync: SyncPoint,
        cpu_waits: Arc<AtomicUsize>,
        exports: Arc<AtomicUsize>,
    }
    impl Fence for Producer {
        fn is_signaled(&self) -> bool {
            self.sync.is_reached()
        }
        fn wait(&self) -> Result<(), Interrupted> {
            self.cpu_waits.fetch_add(1, Ordering::SeqCst);
            self.sync.wait()
        }
        fn is_exportable(&self) -> bool {
            self.sync.is_exportable()
        }
        fn export(&self) -> Option<std::os::fd::OwnedFd> {
            let fd = self.sync.export();
            if fd.is_some() {
                self.exports.fetch_add(1, Ordering::SeqCst);
            }
            fd
        }
    }
    let (vk, mut clients, mut allocator) = setup();
    assert!(vk.ctx.as_ref().unwrap().supports_sync_file_import);
    let mods = modifiers(&vk, &clients, Fourcc::Argb8888);
    let cpu_waits = Arc::new(AtomicUsize::new(0));
    let exports = Arc::new(AtomicUsize::new(0));
    let mut sources = Vec::new();
    for color in [[1., 0., 0., 1.], [0., 1., 0., 1.]] {
        let mut dmabuf = allocator
            .create_buffer(16, 16, Fourcc::Argb8888, &mods)
            .unwrap()
            .export()
            .unwrap();
        let sync = {
            let mut target = clients.bind(&mut dmabuf).unwrap();
            let mut frame = clients
                .render(&mut target, (16, 16).into(), Transform::Normal)
                .unwrap();
            Frame::clear(
                &mut frame,
                color.into(),
                &[Rectangle::from_size((16, 16).into())],
            )
            .unwrap();
            frame.finish().unwrap()
        };
        assert!(sync.is_exportable());
        let texture = vk.import_dmabuf(&dmabuf).unwrap();
        sources.push((
            dmabuf,
            texture,
            SyncPoint::from(Producer {
                sync,
                cpu_waits: cpu_waits.clone(),
                exports: exports.clone(),
            }),
        ));
    }
    let mut renderer = NativeRenderer::new(vk).unwrap();
    let mut output = allocator
        .create_buffer(32, 16, Fourcc::Argb8888, &mods)
        .unwrap()
        .export()
        .unwrap();
    // Also abort an imported-but-unsubmitted frame. Validation must see no leaked
    // semaphores, and its clear must not modify the previous displayed image.
    for submit in [true, false] {
        let mut target = renderer.bind(&mut output).unwrap();
        let mut frame = renderer
            .render(&mut target, (32, 16).into(), Transform::Normal)
            .unwrap();
        frame
            .clear(Color32F::BLACK, &[Rectangle::from_size((32, 16).into())])
            .unwrap();
        frame.wait(&SyncPoint::signaled()).unwrap();
        for (i, (_, texture, sync)) in sources.iter().enumerate() {
            frame.wait(sync).unwrap();
            frame
                .render_texture_from_to(
                    texture,
                    Rectangle::from_size((16., 16.).into()),
                    Rectangle::new(((i * 16) as i32, 0).into(), (16, 16).into()),
                    &[Rectangle::from_size((16, 16).into())],
                    &[],
                    Transform::Normal,
                    1.,
                )
                .unwrap();
        }
        if submit {
            frame.finish().unwrap().wait().unwrap();
        } else {
            drop(frame);
        }
        drop(target);
        assert_eq!(
            cpu_waits.load(Ordering::SeqCst),
            0,
            "native frame fell back to CPU producer waits"
        );
        for (i, p) in pixels(&mut clients, &mut output)
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
        {
            let expected = if i % 32 < 16 {
                [255, 0, 0, 255]
            } else {
                [0, 255, 0, 255]
            };
            assert_eq!(
                *p, expected,
                "lost producer or aborted frame changed output"
            );
        }
    }
    assert_eq!(exports.load(Ordering::SeqCst), 4);
    eprintln!("PASS: two native GPU producer waits, no CPU producer wait, clean frame abort");
}

#[test]
#[ignore = "requires real DRM render node, EGL and Vulkan"]
fn dmabuf_gles_vulkan_roundtrip() {
    let (mut vk, mut gles, mut allocator) = setup();
    for format in [
        Fourcc::Argb8888,
        Fourcc::Abgr8888,
        Fourcc::Abgr2101010,
        Fourcc::Xrgb8888,
        Fourcc::Xrgb2101010,
        Fourcc::Xbgr2101010,
        Fourcc::Argb2101010,
    ] {
        let buffer = match allocator.create_buffer(64, 64, format, &modifiers(&vk, &gles, format)) {
            Ok(buffer) => buffer,
            Err(err)
                if !matches!(
                    format,
                    Fourcc::Argb8888 | Fourcc::Abgr8888 | Fourcc::Abgr2101010
                ) =>
            {
                eprintln!("NOT AVAILABLE: optional {format:?} GBM allocation: {err}");
                continue;
            }
            Err(err) => panic!("required compositor format {format:?}: {err}"),
        };
        let mut dmabuf = buffer.export().unwrap();
        eprintln!(
            "DMA-BUF {format:?}: {:?}, {} planes",
            dmabuf.format().modifier,
            dmabuf.num_planes()
        );
        let size = dmabuf
            .size()
            .to_logical(1, Transform::Normal)
            .to_physical(1);
        // Reuse one swapchain buffer through multiple GLES producer / Vulkan consumer cycles.
        for iteration in 0..4 {
            {
                let mut target = gles.bind(&mut dmabuf).unwrap();
                let mut frame = gles.render(&mut target, size, Transform::Normal).unwrap();
                frame
                    .clear(
                        Color32F::new(0.1, 0.2, 0.3, 1.),
                        &[Rectangle::from_size(size)],
                    )
                    .unwrap();
                frame
                    .clear(
                        Color32F::new(0.8, 0.1, 0.05, 1.),
                        &[Rectangle::new((0, 0).into(), (16 + iteration, 32).into())],
                    )
                    .unwrap();
                frame
                    .clear(
                        Color32F::new(0.05, 0.4, 0.1, 0.5),
                        &[Rectangle::new((32, 32).into(), (32, 32).into())],
                    )
                    .unwrap();
                frame.finish().unwrap().wait().unwrap();
            }
            let before = pixels(&mut gles, &mut dmabuf);
            let exported = buffer.export().unwrap();
            let key = TargetKey::new(&exported).unwrap();
            let old = vk.target_cache.get(&key).map(|c| c.texture.image());
            vk.composite_dmabuf(&exported).unwrap();
            if let Some(old) = old {
                assert_eq!(
                    old,
                    vk.target_cache[&key].texture.image(),
                    "re-export must hit cache"
                );
            }
            let after = pixels(&mut gles, &mut dmabuf);
            assert_eq!(before.len(), after.len());
            for (i, (a, b)) in before.iter().zip(&after).enumerate() {
                assert!(
                    a.abs_diff(*b) <= 1,
                    "{format:?} cycle {iteration}, byte {i}: GLES {a}, Vulkan {b}"
                );
            }
        }
        eprintln!("PASS: {format:?} GLES -> Vulkan -> GLES, pixel equality <= 1/255");
    }
}

#[test]
#[ignore = "requires real DRM render node, EGL and Vulkan"]
fn texture_draw_abort_lifetime_and_descriptor_churn() {
    let (mut vk, mut gles, mut allocator) = setup();
    let buffer = allocator
        .create_buffer(
            32,
            32,
            Fourcc::Argb8888,
            &modifiers(&vk, &gles, Fourcc::Argb8888),
        )
        .unwrap();
    let mut dmabuf = buffer.export().unwrap();
    crate::render_helpers::clear_dmabuf(&mut gles, dmabuf.clone())
        .unwrap()
        .wait()
        .unwrap();
    let texture = vk.create_solid_texture([51, 102, 153, 255]).unwrap();
    // Abandon a recorded draw: layout state must stay PREINITIALIZED.
    let target = vk.import_dmabuf_as_target(&dmabuf).unwrap();
    {
        let mut frame = vk
            .render_frame(
                target.image(),
                target.image_view(),
                vk::Format::B8G8R8A8_UNORM,
                target.has_alpha,
                32,
                32,
                None,
            )
            .unwrap();
        frame
            .draw_texture(&texture, 0., 0., 32., 32., None, 1., None, 0.)
            .unwrap();
    }
    assert_eq!(
        texture.current_layout().get(),
        vk::ImageLayout::PREINITIALIZED
    );
    vk.render_to_target_dmabuf(&dmabuf, Some([0.; 4]), |f| {
        f.draw_texture(&texture, 0., 0., 32., 32., None, 1., None, 0.)
    })
    .unwrap();
    for p in pixels(&mut gles, &mut dmabuf).as_chunks::<4>().0 {
        assert_eq!(*p, [51, 102, 153, 255]);
    }
    // Draw commands own textures even if the caller drops them before submission.
    let ctx = vk.ctx.as_ref().unwrap().clone();
    vk.render_to_target_dmabuf(&dmabuf, Some([0.; 4]), |f| {
        let t = create_solid_texture_raw(&ctx, [255, 0, 0, 255])?;
        f.draw_texture(&t, 0., 0., 16., 32., None, 1., None, 0.)?;
        f.draw_border(16., 0., 16., 32., pipeline::BorderPushConstants::default())
    })
    .unwrap();
    // Reuse a client DMA-BUF after external GLES writes. Each frame must acquire
    // and release FOREIGN ownership, even when the import/descriptor was cached.
    let source_buffer = allocator
        .create_buffer(
            32,
            32,
            Fourcc::Argb8888,
            &modifiers(&vk, &gles, Fourcc::Argb8888),
        )
        .unwrap();
    let mut source = source_buffer.export().unwrap();
    for color in [[0.8, 0.2, 0.1, 1.], [0.1, 0.2, 0.8, 1.]] {
        {
            let mut target = gles.bind(&mut source).unwrap();
            let size = (32, 32).into();
            let mut frame = gles.render(&mut target, size, Transform::Normal).unwrap();
            frame
                .clear(
                    Color32F::new(color[0], color[1], color[2], color[3]),
                    &[Rectangle::from_size(size)],
                )
                .unwrap();
            frame.finish().unwrap().wait().unwrap();
        }
        vk.render_to_target_dmabuf(&dmabuf, Some([0.; 4]), |frame| {
            frame
                .draw_texture_dmabuf(&source, 0., 0., 32., 32., None, 1., None, 0.)
                .unwrap();
            Ok(())
        })
        .unwrap();
        assert_eq!(
            pixels(&mut gles, &mut source),
            pixels(&mut gles, &mut dmabuf)
        );
    }
    // Many short-lived swapchain allocations must not retain unbounded GPU memory.
    for _ in 0..17 {
        let buffer = allocator
            .create_buffer(
                32,
                32,
                Fourcc::Argb8888,
                &modifiers(&vk, &gles, Fourcc::Argb8888),
            )
            .unwrap();
        let buffer = buffer.export().unwrap();
        crate::render_helpers::clear_dmabuf(&mut gles, buffer.clone())
            .unwrap()
            .wait()
            .unwrap();
        vk.composite_dmabuf(&buffer).unwrap();
        assert!(vk.target_cache.len() <= 16);
    }
    let mgr = vk.pipeline_mgr.as_ref().unwrap().clone();
    let start = std::time::Instant::now();
    for _ in 0..1100 {
        let t = vk.create_solid_texture([255; 4]).unwrap();
        let linear = mgr.get_or_create_texture_descriptor_set(&t, true).unwrap();
        let nearest = mgr.get_or_create_texture_descriptor_set(&t, false).unwrap();
        assert_ne!(linear, nearest);
        assert_eq!(
            linear,
            mgr.get_or_create_texture_descriptor_set(&t, true).unwrap()
        );
    }
    eprintln!(
        "PASS: 2200 descriptor allocations/reclaims in {:?}",
        start.elapsed()
    );
    drop(mgr);
    drop(target);
    // A public texture may safely outlive the renderer/device/pipeline fields.
    drop(ctx);
    drop(vk);
    drop(texture);
}

#[test]
#[ignore = "requires real DRM render node, GBM, EGL and Vulkan"]
fn native_dmabuf_import_cache_retains_rotating_buffers_and_bounds_driver_objects() {
    use smithay::backend::renderer::{ImportDma as _, Texture as _};

    fn paint(gles: &mut GlesRenderer, buffer: &mut Dmabuf, color: [f32; 4]) {
        let size = buffer.size();
        let mut target = gles.bind(buffer).unwrap();
        let mut frame = gles
            .render(
                &mut target,
                size.to_logical(1, Transform::Normal).to_physical(1),
                Transform::Normal,
            )
            .unwrap();
        frame
            .clear(color.into(), &[Rectangle::from_size(frame.output_size())])
            .unwrap();
        frame.finish().unwrap().wait().unwrap();
    }

    fn draw(
        native: &mut native::NativeRenderer,
        source: &VulkanTexture,
        target: &mut VulkanTexture,
        expected: [u8; 4],
    ) {
        let size = target.size();
        {
            let mut framebuffer = native.bind(target).unwrap();
            let mut frame = native
                .render(&mut framebuffer, (size.w, size.h).into(), Transform::Normal)
                .unwrap();
            let damage = Rectangle::from_size(frame.output_size());
            frame.clear(Color32F::TRANSPARENT, &[damage]).unwrap();
            frame
                .render_texture_from_to(
                    source,
                    Rectangle::from_size(source.size()).to_f64(),
                    damage,
                    &[damage],
                    &[],
                    Transform::Normal,
                    1.,
                )
                .unwrap();
            frame.finish().unwrap().wait().unwrap();
        }
        let mapping = native
            .copy_texture(target, Rectangle::from_size(size), Fourcc::Abgr8888)
            .unwrap();
        assert!(native
            .map_texture(&mapping)
            .unwrap()
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == expected));
    }

    let (vk, mut gles, mut allocator) = setup();
    let mods = modifiers(&vk, &gles, Fourcc::Argb8888);
    let mut buffers = (0..3)
        .map(|_| {
            allocator
                .create_buffer(32, 24, Fourcc::Argb8888, &mods)
                .unwrap()
                .export()
                .unwrap()
        })
        .collect::<Vec<_>>();
    let colors = [[255u8, 0, 0, 255], [0u8, 255, 0, 255], [0u8, 0, 255, 255]];
    for (buffer, color) in buffers.iter_mut().zip(colors) {
        paint(&mut gles, buffer, color.map(|v| f32::from(v) / 255.));
    }
    let mut native = native::NativeRenderer::new(vk).unwrap();
    let weak_context = Arc::downgrade(native.vulkan().ctx.as_ref().unwrap());
    let importer = native.vulkan().importer.as_ref().unwrap().clone();
    let mut target = native
        .create_buffer(Fourcc::Abgr8888, (32, 24).into())
        .unwrap();
    let start_count = importer.image_import_count();
    let mut identities = Vec::new();
    for (buffer, expected) in buffers.iter().zip(colors) {
        let texture = native.import_dmabuf(buffer, None).unwrap();
        let requirements = unsafe {
            texture
                .inner
                .ctx
                .device
                .get_image_memory_requirements(texture.image())
        };
        assert_eq!(texture.inner.allocation_size, requirements.size);
        assert!(texture.inner.allocation_size > 0);
        identities.push((texture.image(), Arc::downgrade(&texture.inner)));
        draw(&mut native, &texture, &mut target, expected);
        drop(texture);
    }
    assert_eq!(importer.image_import_count() - start_count, 3);
    assert!(identities
        .iter()
        .all(|(_, texture)| texture.upgrade().is_some()));

    // Complete GPU sampling and drop the imported texture on every frame, as
    // Smithay does after a new buffer commit. The next rotation must reuse the
    // existing driver import, independent of allocator handle reuse behavior.
    for frame in 0..90 {
        let index = frame % 3;
        let texture = native.import_dmabuf(&buffers[index], None).unwrap();
        assert_eq!(texture.image(), identities[index].0);
        draw(&mut native, &texture, &mut target, colors[index]);
        drop(texture);
    }
    assert_eq!(importer.image_import_count() - start_count, 3);

    // Measure actual driver-import CPU work against the same three producer
    // allocations with warm cache hits. Timings are reported, never used as a
    // hardware-dependent pass/fail FPS claim.
    let samples = 120;
    let cold_count = importer.image_import_count();
    let started = std::time::Instant::now();
    for frame in 0..samples {
        drop(
            importer
                .import_dmabuf_with_usage(
                    &buffers[frame % 3],
                    vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_SRC,
                )
                .unwrap(),
        );
    }
    let cold = started.elapsed();
    assert_eq!(importer.image_import_count() - cold_count, samples as u64);
    let warm_count = importer.image_import_count();
    let started = std::time::Instant::now();
    for frame in 0..samples {
        drop(importer.import_dmabuf(&buffers[frame % 3]).unwrap());
    }
    let warm = started.elapsed();
    assert_eq!(importer.image_import_count(), warm_count);
    eprintln!(
        "TIMING: sampled DMA-BUF import 3 buffers/{samples} samples, cold={}us warm={}us, driver images {samples} -> 0",
        cold.as_micros(), warm.as_micros()
    );

    // A producer can update the same allocation; retaining its image must not
    // cache pixels or skip synchronization supplied by the caller.
    paint(&mut gles, &mut buffers[0], [0., 1., 1., 1.]);
    let texture = native.import_dmabuf(&buffers[0], None).unwrap();
    assert_eq!(texture.image(), identities[0].0);
    draw(&mut native, &texture, &mut target, [0, 255, 255, 255]);
    drop(texture);
    assert_eq!(importer.image_import_count(), warm_count);

    // Different producer identities, dimensions and formats must not alias an
    // existing entry. Keep all producers alive to exercise the real LRU limit.
    for (width, height, code) in [(19, 11, Fourcc::Argb8888), (32, 24, Fourcc::Xrgb8888)] {
        let mods = modifiers(native.vulkan(), &gles, code);
        let mut buffer = allocator
            .create_buffer(width, height, code, &mods)
            .unwrap()
            .export()
            .unwrap();
        paint(&mut gles, &mut buffer, [1., 1., 0., 1.]);
        let texture = native.import_dmabuf(&buffer, None).unwrap();
        assert_eq!(texture.size(), (width as i32, height as i32).into());
        assert_eq!(texture.format(), Some(code));
        assert!(identities
            .iter()
            .all(|(image, _)| *image != texture.image()));
        draw(&mut native, &texture, &mut target, [255, 255, 0, 255]);
        drop(texture);
        buffers.push(buffer);
    }
    while buffers.len() < 64 {
        let buffer = allocator
            .create_buffer(32, 24, Fourcc::Argb8888, &mods)
            .unwrap()
            .export()
            .unwrap();
        drop(native.import_dmabuf(&buffer, None).unwrap());
        buffers.push(buffer);
    }
    let allocations = importer.cached_allocation_sizes();
    assert_eq!(allocations.len(), 64);
    assert!(allocations.iter().sum::<u64>() <= 256 * 1024 * 1024);

    // Touch buffer 0, then exceed the count budget. Buffer 1 is now the oldest;
    // evicting it must not discard the recently used buffer 0.
    drop(native.import_dmabuf(&buffers[0], None).unwrap());
    let buffer = allocator
        .create_buffer(32, 24, Fourcc::Argb8888, &mods)
        .unwrap()
        .export()
        .unwrap();
    drop(native.import_dmabuf(&buffer, None).unwrap());
    buffers.push(buffer);
    assert!(identities[0].1.upgrade().is_some());
    assert!(identities[1].1.upgrade().is_none());
    assert_eq!(importer.cached_allocation_sizes().len(), 64);
    let before_hit = importer.image_import_count();
    drop(native.import_dmabuf(&buffers[0], None).unwrap());
    assert_eq!(importer.image_import_count(), before_hit);
    drop(native.import_dmabuf(&buffers[1], None).unwrap());
    assert_eq!(importer.image_import_count(), before_hit + 1);

    let retained = native.import_dmabuf(&buffers[0], None).unwrap();
    let producer_keys = buffers.iter().map(Dmabuf::weak).collect::<Vec<_>>();
    drop(buffers);
    assert!(producer_keys.iter().all(|key| key.is_gone()));
    importer.sweep_expired_imports();
    assert!(importer.cached_allocation_sizes().is_empty());
    draw(&mut native, &retained, &mut target, [0, 255, 255, 255]);
    drop(retained);
    drop(target);
    drop(native);
    drop(importer);
    assert!(weak_context.upgrade().is_none());
    eprintln!(
        "PASS: sampled DMA-BUF triple buffering uses 3 driver imports for 90 completed/drop/reimport frames, live pixels/format/size stay correct, 64-entry/256MiB LRU evicts oldest, weak producers expire and renderer context releases"
    );
}

#[test]
#[ignore = "requires real DRM render node, EGL and Vulkan"]
fn final_pass_warm_cache_timing() {
    let (mut vk, mut gles, mut allocator) = setup();
    let buffer = allocator
        .create_buffer(
            1920,
            1080,
            Fourcc::Xrgb8888,
            &modifiers(&vk, &gles, Fourcc::Xrgb8888),
        )
        .unwrap();
    crate::render_helpers::clear_dmabuf(&mut gles, buffer.export().unwrap())
        .unwrap()
        .wait()
        .unwrap();
    vk.composite_dmabuf(&buffer.export().unwrap()).unwrap();
    let start = std::time::Instant::now();
    for _ in 0..120 {
        vk.composite_dmabuf(&buffer.export().unwrap()).unwrap();
    }
    let warm = start.elapsed();
    assert_eq!(vk.target_cache.len(), 1);
    let start = std::time::Instant::now();
    for _ in 0..30 {
        vk.target_cache.clear();
        vk.composite_dmabuf(&buffer.export().unwrap()).unwrap();
    }
    eprintln!(
        "TIMING: same pass with imports/scratch recreated, 30 frames: {:.3} ms/frame",
        start.elapsed().as_secs_f64() * 1000. / 30.
    );
    eprintln!(
        "TIMING: 1080p GPU final pass, warm cache, 120 frames: {:.3} ms/frame (CPU wall including fence wait)",
        warm.as_secs_f64() * 1000. / 120.
    );
    let mut damage: smithay::backend::renderer::utils::DamageBag<i32, smithay::utils::Buffer> =
        smithay::backend::renderer::utils::DamageBag::new(4);
    let rect = Rectangle::new((128, 128).into(), (64, 64).into());
    damage.add([rect]);
    let ready = smithay::backend::renderer::sync::SyncPoint::signaled();
    vk.composite_dmabuf_after(&buffer.export().unwrap(), &ready, Some(&damage.snapshot()))
        .unwrap();
    let start = std::time::Instant::now();
    for _ in 0..120 {
        damage.add([rect]);
        let stats = vk
            .composite_dmabuf_after(&buffer.export().unwrap(), &ready, Some(&damage.snapshot()))
            .unwrap();
        assert_eq!(stats.damaged_pixels, 4096);
    }
    eprintln!(
        "TIMING: 1080p target with 64x64 damage, 120 frames: {:.3} ms/frame",
        start.elapsed().as_secs_f64() * 1000. / 120.
    );
}

#[test]
#[ignore = "requires real DRM render node, EGL and Vulkan"]
fn nourish_damage_and_gpu_acquire() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use smithay::backend::renderer::sync::{Fence, Interrupted, SyncPoint};
    use smithay::backend::renderer::utils::DamageBag;
    #[derive(Debug)]
    struct Producer {
        sync: SyncPoint,
        waits: Arc<AtomicUsize>,
        exportable: bool,
    }
    impl Fence for Producer {
        fn is_signaled(&self) -> bool {
            self.sync.is_reached()
        }
        fn wait(&self) -> Result<(), Interrupted> {
            self.waits.fetch_add(1, Ordering::SeqCst);
            self.sync.wait()
        }
        fn is_exportable(&self) -> bool {
            self.exportable && self.sync.is_exportable()
        }
        fn export(&self) -> Option<std::os::fd::OwnedFd> {
            self.exportable.then(|| self.sync.export()).flatten()
        }
    }
    let (mut vk, mut gles, mut allocator) = setup();
    assert!(vk.ctx.as_ref().unwrap().supports_sync_file_import);
    let buffer = allocator
        .create_buffer(
            64,
            64,
            Fourcc::Argb8888,
            &modifiers(&vk, &gles, Fourcc::Argb8888),
        )
        .unwrap();
    let mut target = buffer.export().unwrap();
    let mut damage: DamageBag<i32, smithay::utils::Buffer> = DamageBag::new(4);
    let waits = Arc::new(AtomicUsize::new(0));
    let mut reused = None;
    for iteration in 0..5 {
        let rect = if iteration == 0 {
            Rectangle::from_size((64, 64).into())
        } else {
            Rectangle::new((9, 11).into(), (13, 7).into())
        };
        let sync = {
            let mut bound = gles.bind(&mut target).unwrap();
            let mut f = gles
                .render(&mut bound, (64, 64).into(), Transform::Normal)
                .unwrap();
            f.clear(
                Color32F::new(0.1 * (iteration as f32 + 1.), 0.2, 0.3, 0.5),
                &[rect],
            )
            .unwrap();
            f.finish().unwrap()
        };
        assert!(
            sync.is_exportable(),
            "this hardware test requires an EGL native fence"
        );
        // The first iteration submits without a CPU readback/wait of the producer.
        let expected = (iteration != 0).then(|| pixels(&mut gles, &mut target));
        damage.add([Rectangle::new(
            (rect.loc.x, rect.loc.y).into(),
            (rect.size.w, rect.size.h).into(),
        )]);
        let producer = SyncPoint::from(Producer {
            sync,
            waits: waits.clone(),
            exportable: true,
        });
        let stats = vk
            .composite_dmabuf_after(&target, &producer, Some(&damage.snapshot()))
            .unwrap();
        assert!(
            stats.gpu_acquire,
            "must execute SYNC_FD GPU wait, not silently fall back"
        );
        assert_eq!(waits.load(Ordering::SeqCst), 0);
        assert_eq!(
            stats.damaged_pixels,
            if iteration == 0 { 64 * 64 } else { 13 * 7 }
        );
        let actual = pixels(&mut gles, &mut target);
        if let Some(expected) = expected {
            assert_eq!(
                actual, expected,
                "partial draw changed alpha or untouched pixels"
            );
        } else {
            for pixel in actual.as_chunks::<4>().0 {
                for (a, b) in pixel.iter().zip([26u8, 51, 76, 128]) {
                    assert!(a.abs_diff(b) <= 1);
                }
            }
        }
        let idle = vk.ctx.as_ref().unwrap().idle_commands.lock().unwrap();
        assert_eq!(idle.len(), 1);
        if let Some(handles) = reused {
            assert_eq!(handles, idle[0]);
        }
        reused = Some(idle[0]);
    }
    let unchanged = vk
        .composite_dmabuf_after(&target, &SyncPoint::signaled(), Some(&damage.snapshot()))
        .unwrap();
    assert_eq!(unchanged.damaged_pixels, 0);
    for _ in 0..5 {
        damage.add([Rectangle::from_size((1, 1).into())]);
    }
    let expired = vk
        .composite_dmabuf_after(&target, &SyncPoint::signaled(), Some(&damage.snapshot()))
        .unwrap();
    assert_eq!(
        expired.damaged_pixels,
        64 * 64,
        "expired history must redraw fully"
    );
    let fallback = SyncPoint::from(Producer {
        sync: SyncPoint::signaled(),
        waits: waits.clone(),
        exportable: false,
    });
    let fallback_stats = vk.composite_dmabuf_after(&target, &fallback, None).unwrap();
    assert!(!fallback_stats.gpu_acquire);
    assert_eq!(
        waits.load(Ordering::SeqCst),
        1,
        "a non-exportable producer must be waited on"
    );
    eprintln!(
        "PASS: Nourish damage: 4096 -> 91 pixels; GPU acquire; zero producer CPU waits; command/fence reuse; expired-history full redraw"
    );
}

#[test]
#[ignore = "requires real DRM render node, EGL and Vulkan"]
fn interrupted_producer_preserves_damage_and_allows_retry() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use smithay::backend::renderer::sync::{Fence, Interrupted, SyncPoint};
    use smithay::backend::renderer::utils::DamageBag;

    #[derive(Debug)]
    struct Producer {
        interrupted: Arc<AtomicBool>,
        waits: Arc<AtomicUsize>,
    }
    impl Fence for Producer {
        fn is_signaled(&self) -> bool {
            !self.interrupted.load(Ordering::SeqCst)
        }
        fn wait(&self) -> Result<(), Interrupted> {
            self.waits.fetch_add(1, Ordering::SeqCst);
            if self.is_signaled() {
                Ok(())
            } else {
                Err(Interrupted)
            }
        }
        fn is_exportable(&self) -> bool {
            false
        }
        fn export(&self) -> Option<std::os::fd::OwnedFd> {
            None
        }
    }
    let (mut vk, mut gles, mut allocator) = setup();
    let buffer = allocator
        .create_buffer(
            64,
            64,
            Fourcc::Argb8888,
            &modifiers(&vk, &gles, Fourcc::Argb8888),
        )
        .unwrap();
    let mut target = buffer.export().unwrap();
    let interrupted = Arc::new(AtomicBool::new(true));
    let waits = Arc::new(AtomicUsize::new(0));
    let producer = SyncPoint::from(Producer {
        interrupted: interrupted.clone(),
        waits: waits.clone(),
    });
    let mut damage: DamageBag<i32, smithay::utils::Buffer> = DamageBag::new(4);
    for (iteration, rect) in [
        Rectangle::from_size((64, 64).into()),
        Rectangle::new((9, 11).into(), (5, 7).into()),
    ]
    .into_iter()
    .enumerate()
    {
        {
            let mut bound = gles.bind(&mut target).unwrap();
            let mut frame = gles
                .render(&mut bound, (64, 64).into(), Transform::Normal)
                .unwrap();
            frame
                .clear(
                    Color32F::new(0.2, 0.1 * (iteration as f32 + 1.), 0.3, 0.5),
                    &[rect],
                )
                .unwrap();
            frame.finish().unwrap().wait().unwrap();
        }
        let expected = pixels(&mut gles, &mut target);
        damage.add([Rectangle::new(
            (rect.loc.x, rect.loc.y).into(),
            (rect.size.w, rect.size.h).into(),
        )]);
        let key = TargetKey::new(&target).unwrap();
        let last_commit = vk
            .target_cache
            .get(&key)
            .and_then(|entry| entry.last_commit);
        interrupted.store(true, Ordering::SeqCst);
        let count = waits.load(Ordering::SeqCst);
        let error = vk
            .composite_dmabuf_after(&target, &producer, Some(&damage.snapshot()))
            .unwrap_err();
        assert!(
            error.is::<Interrupted>(),
            "retain the recoverable error type: {error}"
        );
        assert_eq!(
            waits.load(Ordering::SeqCst) - count,
            3,
            "persistent interruption must not spin"
        );
        assert_eq!(vk.target_cache[&key].last_commit, last_commit);
        assert_eq!(pixels(&mut gles, &mut target), expected);
        interrupted.store(false, Ordering::SeqCst);
        let stats = vk
            .composite_dmabuf_after(&target, &producer, Some(&damage.snapshot()))
            .unwrap();
        assert_eq!(stats.damaged_pixels, if iteration == 0 { 4096 } else { 35 });
        assert_eq!(pixels(&mut gles, &mut target), expected);
    }
    // Even an unchanged image must not be presented before the producer completes.
    interrupted.store(true, Ordering::SeqCst);
    let error = vk
        .composite_dmabuf_after(&target, &producer, Some(&damage.snapshot()))
        .unwrap_err();
    assert!(error.is::<Interrupted>());
    interrupted.store(false, Ordering::SeqCst);
    assert_eq!(
        vk.composite_dmabuf_after(&target, &producer, Some(&damage.snapshot()))
            .unwrap()
            .damaged_pixels,
        0
    );
    eprintln!(
        "PASS: interrupted producer: bounded waits, preserved damage, full/partial/empty retry, unchanged pixels"
    );
}

#[test]
#[ignore = "requires real DRM render node, EGL and Vulkan"]
fn rejects_invalid_dmabuf_extents_before_image_creation() {
    use smithay::backend::allocator::dmabuf::DmabufFlags;
    let (vk, gles, mut allocator) = setup();
    let buffer = allocator
        .create_buffer(
            64,
            64,
            Fourcc::Argb8888,
            &modifiers(&vk, &gles, Fourcc::Argb8888),
        )
        .unwrap();
    let source = buffer.export().unwrap();
    for size in [
        (-1, 64),
        (64, -1),
        (0, 64),
        (64, 0),
        (i32::MAX, 64),
        (64, i32::MAX),
    ] {
        // Size::from(tuple) debug-asserts for negative dimensions. Its fields
        // remain public, and release builds do not enforce that assertion.
        let mut extent = source.size();
        extent.w = size.0;
        extent.h = size.1;
        let mut builder = Dmabuf::builder(
            extent,
            source.format().code,
            source.format().modifier,
            DmabufFlags::empty(),
        );
        for ((fd, offset), stride) in source.handles().zip(source.offsets()).zip(source.strides()) {
            assert!(builder.add_plane(fd.try_clone_to_owned().unwrap(), offset, stride));
        }
        let invalid = builder.build().unwrap();
        assert!(
            matches!(
                vk.import_dmabuf(&invalid),
                Err(VulkanImportError::InvalidLayout)
            ),
            "invalid sampled extent: {size:?}"
        );
        assert!(
            matches!(
                vk.import_dmabuf_as_target(&invalid),
                Err(VulkanImportError::InvalidLayout)
            ),
            "invalid target extent: {size:?}"
        );
    }
    // Rejection must leave the real allocation usable.
    vk.import_dmabuf_as_target(&source).unwrap();
    eprintln!(
        "PASS: invalid DMA-BUF dimensions rejected without submitting invalid Vulkan image creation"
    );
}

#[test]
#[ignore = "requires real DRM render node, GBM and Vulkan"]
fn native_dma_format_negotiation_and_import() {
    use smithay::backend::renderer::ImportDma;

    use super::native::NativeRenderer;
    let (vk, mut oracle, mut allocator) = setup();
    let mut renderer = NativeRenderer::new(vk).unwrap();
    let samples = renderer.dmabuf_formats();
    let targets = <NativeRenderer as Bind<Dmabuf>>::supported_formats(&renderer).unwrap();
    assert!(samples.iter().next().is_some());
    assert!(targets.iter().next().is_some());
    assert!(samples
        .iter()
        .chain(targets.iter())
        .all(|f| f.modifier != Modifier::Invalid));
    assert!(!targets.iter().any(|f| f.code == Fourcc::Nv12));
    let size = (16, 16).into();
    let all = Rectangle::from_size(size);
    // Allocate from Vulkan's advertised set, with no EGL negotiation. GLES is
    // only used after native rendering as an independent readback oracle.
    for code in [
        Fourcc::Argb8888,
        Fourcc::Abgr8888,
        Fourcc::Xrgb8888,
        Fourcc::Xbgr8888,
    ] {
        let mods: Vec<_> = samples
            .intersection(&targets)
            .filter(|f| f.code == code)
            .map(|f| f.modifier)
            .collect();
        assert!(!mods.is_empty(), "no native sampled target for {code:?}");
        let mut source = allocator
            .create_buffer(16, 16, code, &mods)
            .unwrap()
            .export()
            .unwrap();
        assert!(renderer.has_dmabuf_format(source.format()));
        {
            let mut target = renderer.bind(&mut source).unwrap();
            let mut frame = renderer
                .render(&mut target, size, Transform::Normal)
                .unwrap();
            frame.clear([1., 0., 0., 1.].into(), &[all]).unwrap();
            frame.finish().unwrap().wait().unwrap();
        }
        let texture = ImportDma::import_dmabuf(&mut renderer, &source, None).unwrap();
        let cached = ImportDma::import_dmabuf(&mut renderer, &source, Some(&[])).unwrap();
        assert_eq!(texture.image(), cached.image());
        let output_mods: Vec<_> = targets
            .iter()
            .filter(|f| f.code == Fourcc::Argb8888)
            .map(|f| f.modifier)
            .collect();
        let mut output = allocator
            .create_buffer(16, 16, Fourcc::Argb8888, &output_mods)
            .unwrap()
            .export()
            .unwrap();
        {
            let mut target = renderer.bind(&mut output).unwrap();
            let mut frame = renderer
                .render(&mut target, size, Transform::Normal)
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
        }
        assert!(
            pixels(&mut oracle, &mut output)
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| *p == [255, 0, 0, 255]),
            "native negotiated {code:?} pixels differ"
        );
    }
    eprintln!("PASS: native DMA-BUF negotiation, binding, cached import and pixel output");
}

#[test]
#[ignore = "requires Intel DRM render node with linear NV12, GBM CPU producer and Vulkan"]
fn nv12_dmabuf_immutable_ycbcr_sampler_pixels() {
    use std::os::fd::{AsFd, BorrowedFd};

    use drm::buffer::Buffer as _;
    use drm::control::Device as _;
    use smithay::backend::allocator::dmabuf::{Dmabuf, DmabufFlags};
    use smithay::backend::renderer::ImportDma;
    use smithay::reexports::drm;
    struct Card(std::fs::File);
    impl AsFd for Card {
        fn as_fd(&self) -> BorrowedFd<'_> {
            self.0.as_fd()
        }
    }
    impl drm::Device for Card {}
    impl drm::control::Device for Card {}
    let (vk, mut oracle, mut allocator) = setup();
    let mut renderer = native::NativeRenderer::new(vk).unwrap();
    // A different producer GPU may advertise linear NV12 but still reject
    // that cross-device PRIME fd. Use this renderer's own primary node.
    let Some(primary_node) = intel_primary_node(renderer.vulkan().ctx.as_ref().unwrap()) else {
        eprintln!("SKIP: linear NV12 producer requires the selected Intel GPU's primary DRM node");
        return;
    };
    let nv12 = smithay::backend::allocator::Format {
        code: Fourcc::Nv12,
        modifier: Modifier::Linear,
    };
    if !renderer.dmabuf_formats().contains(&nv12) {
        eprintln!("SKIP: Vulkan driver does not expose sampled linear NV12 DMA-BUF");
        return;
    }
    let vk = renderer.vulkan_mut();
    // Intel GBM refuses CPU-writable NV12 allocations. A DRM dumb buffer is
    // linear and gives this test an independent one-time NV12 producer.
    let card = Card(
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&primary_node)
            .expect("Intel primary DRM node"),
    );
    eprintln!("NV12 PRODUCER: {}", primary_node.display());
    let mut dumb = card
        .create_dumb_buffer((16, 24), Fourcc::Nv12, 8)
        .expect("linear NV12 test storage");
    let pitch = dumb.pitch();
    {
        let mut mapped = card.map_dumb_buffer(&mut dumb).unwrap();
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
    let fd = card
        .buffer_to_prime_fd(dumb.handle(), drm::CLOEXEC | drm::RDWR)
        .expect("export test DMA-BUF");
    let mut builder = Dmabuf::builder(
        (16, 16),
        Fourcc::Nv12,
        Modifier::Linear,
        DmabufFlags::empty(),
    );
    builder.add_plane(fd.try_clone().unwrap(), 0, pitch);
    builder.add_plane(fd, 16 * pitch, pitch);
    let source = builder.build().unwrap();
    card.destroy_dumb_buffer(dumb).unwrap();
    assert_eq!(source.num_planes(), 2);
    let texture = vk.import_dmabuf(&source).expect("NV12 Vulkan import");
    assert_eq!(
        smithay::backend::renderer::Texture::format(&texture),
        Some(Fourcc::Nv12)
    );

    let mut target = allocator
        .create_buffer(
            16,
            16,
            Fourcc::Argb8888,
            &modifiers(vk, &oracle, Fourcc::Argb8888),
        )
        .unwrap()
        .export()
        .unwrap();
    vk.render_to_target_dmabuf(&target, Some([0., 0., 0., 1.]), |frame| {
        frame
            .draw_texture_dmabuf(&source, 0., 0., 16., 16., None, 1., None, 0.)
            .map_err(|_| vk::Result::ERROR_FORMAT_NOT_SUPPORTED)
    })
    .unwrap();
    let bytes = pixels(&mut oracle, &mut target);
    let red = &bytes[(4 * 16 + 3) * 4..(4 * 16 + 3) * 4 + 4];
    let blue = &bytes[(4 * 16 + 12) * 4..(4 * 16 + 12) * 4 + 4];
    assert!(
        red[0] > 170 && red[1] < 100 && red[2] < 100 && red[3] == 255,
        "NV12 red sample: {red:?}"
    );
    assert!(
        blue[2] > 150 && blue[0] < 110 && blue[1] < 110 && blue[3] == 255,
        "NV12 blue sample: {blue:?}"
    );
}

#[test]
#[ignore = "requires Intel DRM node with disjoint linear NV12 Vulkan import"]
fn nv12_distinct_plane_fds_bind_and_sample() {
    use std::os::fd::{AsFd, BorrowedFd};

    use drm::buffer::Buffer as _;
    use drm::control::Device as _;
    use smithay::backend::allocator::dmabuf::{Dmabuf, DmabufFlags};
    use smithay::reexports::drm;
    struct Card(std::fs::File);
    impl AsFd for Card {
        fn as_fd(&self) -> BorrowedFd<'_> {
            self.0.as_fd()
        }
    }
    impl drm::Device for Card {}
    impl drm::control::Device for Card {}

    let (mut vk, mut oracle, mut allocator) = setup();
    let ctx = vk.ctx.as_ref().unwrap();
    // Keep both dumb-buffer planes on this Vulkan GPU so the test exercises
    // plane binding rather than unrelated cross-device PRIME support.
    let Some(primary_node) = intel_primary_node(ctx) else {
        eprintln!(
            "SKIP: distinct-plane NV12 producer requires the selected Intel GPU's primary DRM node"
        );
        return;
    };
    let mut list = vk::DrmFormatModifierPropertiesListEXT::default();
    let mut props = vk::FormatProperties2::default().push_next(&mut list);
    unsafe {
        ctx.instance.get_physical_device_format_properties2(
            ctx.physical_device,
            vk::Format::G8_B8R8_2PLANE_420_UNORM,
            &mut props,
        )
    };
    let mut modifier_props = vec![
        vk::DrmFormatModifierPropertiesEXT::default();
        list.drm_format_modifier_count as usize
    ];
    let mut list = vk::DrmFormatModifierPropertiesListEXT::default()
        .drm_format_modifier_properties(&mut modifier_props);
    let mut props = vk::FormatProperties2::default().push_next(&mut list);
    unsafe {
        ctx.instance.get_physical_device_format_properties2(
            ctx.physical_device,
            vk::Format::G8_B8R8_2PLANE_420_UNORM,
            &mut props,
        )
    };
    if !modifier_props.iter().any(|m| {
        m.drm_format_modifier == u64::from(Modifier::Linear)
            && m.drm_format_modifier_plane_count == 2
            && m.drm_format_modifier_tiling_features
                .contains(vk::FormatFeatureFlags::DISJOINT)
    }) {
        eprintln!("SKIP: Intel modifier does not expose disjoint NV12 memory planes");
        return;
    }
    let mut modifier_query = vk::PhysicalDeviceImageDrmFormatModifierInfoEXT::default()
        .drm_format_modifier(u64::from(Modifier::Linear))
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let mut external_query = vk::PhysicalDeviceExternalImageFormatInfo::default()
        .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
    let query = vk::PhysicalDeviceImageFormatInfo2::default()
        .format(vk::Format::G8_B8R8_2PLANE_420_UNORM)
        .ty(vk::ImageType::TYPE_2D)
        .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
        .flags(vk::ImageCreateFlags::DISJOINT)
        .usage(vk::ImageUsageFlags::SAMPLED)
        .push_next(&mut modifier_query)
        .push_next(&mut external_query);
    let mut external = vk::ExternalImageFormatProperties::default();
    let mut properties = vk::ImageFormatProperties2::default().push_next(&mut external);
    let compatible = unsafe {
        ctx.instance.get_physical_device_image_format_properties2(
            ctx.physical_device,
            &query,
            &mut properties,
        )
    }
    .is_ok()
        && external
            .external_memory_properties
            .external_memory_features
            .contains(vk::ExternalMemoryFeatureFlags::IMPORTABLE)
        && !external
            .external_memory_properties
            .external_memory_features
            .contains(vk::ExternalMemoryFeatureFlags::DEDICATED_ONLY);
    if !compatible {
        eprintln!("SKIP: driver cannot import two distinct NV12 DMA-BUF plane FDs");
        return;
    }
    let card = Card(
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&primary_node)
            .unwrap(),
    );
    eprintln!("NV12 PRODUCER: {}", primary_node.display());
    let mut y = card.create_dumb_buffer((16, 16), Fourcc::Nv12, 8).unwrap();
    let mut uv = card.create_dumb_buffer((16, 8), Fourcc::Nv12, 8).unwrap();
    let y_pitch = y.pitch();
    let uv_pitch = uv.pitch();
    {
        let mut map = card.map_dumb_buffer(&mut y).unwrap();
        for row in 0..16usize {
            for x in 0..16usize {
                map[row * y_pitch as usize + x] = 81;
            }
        }
    }
    {
        let mut map = card.map_dumb_buffer(&mut uv).unwrap();
        for row in 0..8usize {
            for x in (0..16usize).step_by(2) {
                map[row * uv_pitch as usize + x] = 90;
                map[row * uv_pitch as usize + x + 1] = 240;
            }
        }
    }
    let mut builder = Dmabuf::builder(
        (16, 16),
        Fourcc::Nv12,
        Modifier::Linear,
        DmabufFlags::empty(),
    );
    builder.add_plane(
        card.buffer_to_prime_fd(y.handle(), drm::CLOEXEC | drm::RDWR)
            .unwrap(),
        0,
        y_pitch,
    );
    builder.add_plane(
        card.buffer_to_prime_fd(uv.handle(), drm::CLOEXEC | drm::RDWR)
            .unwrap(),
        0,
        uv_pitch,
    );
    let source = builder.build().unwrap();
    card.destroy_dumb_buffer(y).unwrap();
    card.destroy_dumb_buffer(uv).unwrap();
    vk.import_dmabuf(&source).expect("import disjoint NV12 FDs");
    let mut target = allocator
        .create_buffer(
            16,
            16,
            Fourcc::Argb8888,
            &modifiers(&vk, &oracle, Fourcc::Argb8888),
        )
        .unwrap()
        .export()
        .unwrap();
    vk.render_to_target_dmabuf(&target, Some([0., 0., 0., 1.]), |frame| {
        frame
            .draw_texture_dmabuf(&source, 0., 0., 16., 16., None, 1., None, 0.)
            .map_err(|_| vk::Result::ERROR_FORMAT_NOT_SUPPORTED)
    })
    .unwrap();
    let bytes = pixels(&mut oracle, &mut target);
    let red = &bytes[(4 * 16 + 4) * 4..(4 * 16 + 4) * 4 + 4];
    assert!(
        red[0] > 170 && red[1] < 100 && red[2] < 100 && red[3] == 255,
        "disjoint NV12 red sample: {red:?}"
    );
}

#[test]
#[ignore = "requires real DRM render node, EGL readback and Vulkan"]
fn native_staging_updates_preserve_texture_and_undamaged_pixels() {
    let (mut renderer, mut oracle, mut allocator) = setup();
    let mut output = allocator
        .create_buffer(
            16,
            16,
            Fourcc::Argb8888,
            &modifiers(&renderer, &oracle, Fourcc::Argb8888),
        )
        .unwrap()
        .export()
        .unwrap();
    for format in [
        Fourcc::Argb8888,
        Fourcc::Abgr8888,
        Fourcc::Xrgb8888,
        Fourcc::Xbgr8888,
    ] {
        let opaque = matches!(format, Fourcc::Xrgb8888 | Fourcc::Xbgr8888);
        let bgra = matches!(format, Fourcc::Argb8888 | Fourcc::Xrgb8888);
        let mut bytes = vec![0; 80 * 15 + 64];
        let base = if bgra {
            [0, 0, 255, 255]
        } else {
            [255, 0, 0, 255]
        };
        for y in 0..16 {
            for x in 0..16 {
                bytes[y * 80 + x * 4..y * 80 + x * 4 + 4].copy_from_slice(&base);
            }
        }
        let texture = renderer
            .import_memory_texture(&bytes, format, 16, 16, 80)
            .unwrap();
        let alias = texture.clone();
        for update in [false, true] {
            if update {
                // Garbage outside damage must never overwrite existing pixels.
                bytes.fill(99);
                for y in 5..11 {
                    for x in 3..8 {
                        bytes[y * 80 + x * 4..y * 80 + x * 4 + 4].copy_from_slice(&[
                            0,
                            128,
                            0,
                            if opaque { 0 } else { 128 },
                        ]);
                    }
                }
                // Invalid later entries must reject the whole batch before any copy.
                assert!(renderer
                    .update_memory_texture_regions(
                        &texture,
                        &bytes,
                        80,
                        &[
                            Rectangle::new((0, 0).into(), (1, 1).into()),
                            Rectangle::new((15, 15).into(), (2, 1).into()),
                        ],
                    )
                    .is_err());
                assert!(renderer
                    .update_memory_texture_regions(
                        &texture,
                        &bytes,
                        80,
                        &[
                            Rectangle::new((0, 0).into(), (2, 2).into()),
                            Rectangle::new((1, 1).into(), (2, 2).into()),
                        ],
                    )
                    .is_err());
                renderer
                    .update_memory_texture_regions(&texture, &bytes, 80, &[])
                    .unwrap();
                let regions = [
                    Rectangle::new((3, 5).into(), (5, 3).into()),
                    Rectangle::new((0, 0).into(), (0, 0).into()),
                    Rectangle::new((3, 8).into(), (5, 3).into()),
                ];
                if bgra {
                    renderer
                        .update_memory_texture_regions(&texture, &bytes, 80, &regions)
                        .unwrap();
                } else {
                    // The SHM path snapshots only damaged pixels in region order.
                    // A malformed packed snapshot must not modify the image.
                    let green = [0, 128, 0, if opaque { 0 } else { 128 }];
                    let blue = [0, 0, 128, if opaque { 0 } else { 128 }];
                    let mut packed = green.repeat(15);
                    packed.extend_from_slice(&blue.repeat(15));
                    assert!(renderer
                        .update_memory_texture_packed_regions(
                            &texture,
                            &packed[..packed.len() - 1],
                            &regions,
                        )
                        .is_err());
                    renderer
                        .update_memory_texture_packed_regions(&texture, &packed, &regions)
                        .unwrap();
                }
                assert!(renderer
                    .update_memory_texture(
                        &texture,
                        &bytes,
                        80,
                        Rectangle::new((15, 15).into(), (2, 1).into())
                    )
                    .is_err());
                assert!(renderer
                    .update_memory_texture(
                        &texture,
                        &bytes[..10],
                        80,
                        Rectangle::from_size((1, 1).into())
                    )
                    .is_err());
            }
            renderer
                .render_to_target_dmabuf(&output, Some([0.; 4]), |frame| {
                    frame.draw_texture(&alias, 0., 0., 16., 16., None, 1., None, 0.)
                })
                .unwrap();
            let actual = pixels(&mut oracle, &mut output);
            let green = [0, 128, 0, if opaque { 255 } else { 128 }];
            assert_eq!(
                actual
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .filter(|p| **p == [255, 0, 0, 255])
                    .count(),
                if update { 226 } else { 256 }
            );
            assert_eq!(
                actual
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .filter(|p| **p == green)
                    .count(),
                if update {
                    if bgra {
                        30
                    } else {
                        15
                    }
                } else {
                    0
                }
            );
            if update && !bgra {
                let blue = [0, 0, 128, if opaque { 255 } else { 128 }];
                assert_eq!(&actual[(5 * 16 + 3) * 4..][..4], &green);
                assert_eq!(&actual[(8 * 16 + 3) * 4..][..4], &blue);
                assert_eq!(
                    actual
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .filter(|p| **p == blue)
                        .count(),
                    15
                );
            }
        }
    }
    eprintln!("PASS: optimal-tiling staging, post-draw partial updates, shared handles and bounds");
}

#[test]
#[ignore = "requires real DRM render node, Vulkan and timeline semaphores"]
fn deferred_shm_uploads_preserve_queue_order_and_texture_lifetime() {
    let (renderer, _oracle, _allocator) = setup();
    let ctx = renderer.ctx.as_ref().unwrap().clone();
    let whole = Rectangle::from_size((4, 4).into());
    let blocked = BlockedQueue::new(ctx.clone());
    let initial = vec![255u8; 4 * 4 * 4];
    let (texture, first) = renderer
        .import_memory_texture_deferred(&initial, Fourcc::Abgr8888, 4, 4, 16)
        .unwrap();
    assert!(!first.status().unwrap(), "first upload waited for the GPU");
    let patch = [0u8, 0, 255, 255];
    let region = Rectangle::new((1, 2).into(), (1, 1).into());
    let second = renderer
        .update_memory_texture_packed_regions_deferred(&texture, &patch, &[region])
        .unwrap()
        .unwrap();
    assert!(
        !second.status().unwrap(),
        "damaged update waited for the GPU"
    );
    drop(blocked);
    let pixels = renderer.read_memory_texture(&texture, whole).unwrap();
    for (index, pixel) in pixels.as_chunks::<4>().0.iter().enumerate() {
        assert_eq!(
            pixel,
            if index == 2 * 4 + 1 {
                &patch
            } else {
                &[255; 4]
            }
        );
    }
    drop(first);
    drop(second);
    let weak = Arc::downgrade(&texture.inner);
    drop(texture);
    assert!(weak.upgrade().is_none());

    // The owner must also keep a submitted image alive after its caller and
    // renderer have gone away, until the queue reaches the upload fence.
    let blocked = BlockedQueue::new(ctx);
    let (texture, pending) = renderer
        .import_memory_texture_deferred(&initial, Fourcc::Abgr8888, 4, 4, 16)
        .unwrap();
    let weak = Arc::downgrade(&texture.inner);
    drop(texture);
    drop(renderer);
    assert!(weak.upgrade().is_some());
    drop(blocked);
    pending.wait().unwrap();
    drop(pending);
    assert!(weak.upgrade().is_none());
}

#[test]
#[ignore = "requires real DRM render node, Vulkan and timeline semaphores"]
fn native_idle_upload_completion_releases_storage_and_stops_waking() {
    use std::time::{Duration, Instant};

    use super::native::NativeRenderer;

    for teardown in [false, true] {
        let (vk, _gles, _allocator) = setup();
        let ctx = vk.ctx.as_ref().unwrap().clone();
        let mut renderer = NativeRenderer::new(vk).unwrap();
        let mut event_loop = smithay::reexports::calloop::EventLoop::<()>::try_new().unwrap();
        renderer
            .install_completion_wakeup(&event_loop.handle())
            .unwrap();
        let blocked = BlockedQueue::new(ctx);
        let permit = renderer.reserve_upload().unwrap();
        let (texture, upload) = renderer
            .vulkan()
            .import_memory_texture_deferred(&[255; 16], Fourcc::Abgr8888, 2, 2, 8)
            .unwrap();
        assert!(!upload.status().unwrap());
        let weak = Arc::downgrade(&texture.inner);
        renderer.retain_shm_upload(upload, permit).unwrap();
        drop(texture);
        let before_drop = Instant::now();
        let renderer = (!teardown).then_some(renderer);
        assert!(before_drop.elapsed() < Duration::from_millis(500));
        assert!(
            weak.upgrade().is_some(),
            "upload was released before GPU completion"
        );
        drop(blocked);
        let wake = Instant::now();
        event_loop
            .dispatch(Duration::from_secs(2), &mut ())
            .unwrap();
        assert!(wake.elapsed() < Duration::from_secs(1));
        assert!(
            weak.upgrade().is_none(),
            "completed SHM upload stayed owned on an idle renderer"
        );
        drop(renderer);
        assert_completion_loop_idle(&mut event_loop);
    }
}

#[test]
#[ignore = "requires real DRM render node, Vulkan and timeline semaphores"]
fn deferred_shm_upload_timeout_keeps_resources_alive() {
    let (renderer, _oracle, _allocator) = setup();
    let blocked = BlockedQueue::new(renderer.ctx.as_ref().unwrap().clone());
    let (texture, pending) = renderer
        .import_memory_texture_deferred(&[255; 16], Fourcc::Abgr8888, 2, 2, 8)
        .unwrap();
    let weak = Arc::downgrade(&texture.inner);
    drop(texture);
    drop(renderer);
    drop(pending);
    assert!(
        weak.upgrade().is_some(),
        "timed-out upload released its in-flight image"
    );
    drop(blocked);
}

mod shm_protocol {
    use std::os::fd::{AsFd, FromRawFd};
    use std::os::unix::fs::FileExt;
    use std::os::unix::net::UnixStream;

    use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
    use smithay::reexports::wayland_server::{self as server, Resource};
    use smithay::wayland::buffer::BufferHandler;
    use smithay::wayland::compositor::{
        with_states, CompositorClientState, CompositorHandler, CompositorState,
    };
    use smithay::wayland::shm::{ShmHandler, ShmState};
    use wayland_client::protocol::{
        wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_subcompositor,
        wl_subsurface, wl_surface,
    };
    use wayland_client::{self as client, Proxy};

    use super::*;
    struct ServerState {
        shm: ShmState,
        compositor: CompositorState,
    }
    impl ShmHandler for ServerState {
        fn shm_state(&self) -> &ShmState {
            &self.shm
        }
    }
    impl BufferHandler for ServerState {
        fn buffer_destroyed(&mut self, _: &WlBuffer) {}
    }
    impl CompositorHandler for ServerState {
        fn compositor_state(&mut self) -> &mut CompositorState {
            &mut self.compositor
        }
        fn client_compositor_state<'a>(
            &self,
            client: &'a server::Client,
        ) -> &'a CompositorClientState {
            &client.get_data::<PeerData>().unwrap().compositor
        }
        fn commit(&mut self, surface: &server::protocol::wl_surface::WlSurface) {
            smithay::backend::renderer::utils::on_commit_buffer_handler::<Self>(surface);
            super::native::record_surface_tree_buffer_owners(surface);
        }
    }
    #[derive(Debug, Default)]
    struct PeerData {
        compositor: CompositorClientState,
    }
    impl server::backend::ClientData for PeerData {}
    smithay::delegate_dispatch2!(ServerState);
    #[derive(Default)]
    struct ClientState {
        shm: Option<u32>,
        compositor: Option<u32>,
        subcompositor: Option<u32>,
    }
    impl client::Dispatch<wl_registry::WlRegistry, ()> for ClientState {
        fn event(
            state: &mut Self,
            _: &wl_registry::WlRegistry,
            event: wl_registry::Event,
            _: &(),
            _: &client::Connection,
            _: &client::QueueHandle<Self>,
        ) {
            if let wl_registry::Event::Global {
                name, interface, ..
            } = event
            {
                if interface == "wl_shm" {
                    state.shm = Some(name);
                }
                if interface == "wl_subcompositor" {
                    state.subcompositor = Some(name);
                }
                if interface == "wl_compositor" {
                    state.compositor = Some(name);
                }
            }
        }
    }
    client::delegate_noop!(ClientState: ignore wl_shm::WlShm);
    client::delegate_noop!(ClientState: ignore wl_shm_pool::WlShmPool);
    client::delegate_noop!(ClientState: ignore wl_buffer::WlBuffer);
    client::delegate_noop!(ClientState: ignore wl_compositor::WlCompositor);
    client::delegate_noop!(ClientState: ignore wl_surface::WlSurface);
    client::delegate_noop!(ClientState: ignore wl_subcompositor::WlSubcompositor);
    client::delegate_noop!(ClientState: ignore wl_subsurface::WlSubsurface);

    #[test]
    fn synchronized_child_owner_updates_on_parent_commit() {
        let mut display = server::Display::<ServerState>::new().unwrap();
        let mut handle = display.handle();
        let mut state = ServerState {
            shm: ShmState::new::<ServerState>(&handle, []),
            compositor: CompositorState::new::<ServerState>(&handle),
        };
        let (server_socket, client_socket) = UnixStream::pair().unwrap();
        let peer = handle
            .insert_client(server_socket, Arc::new(PeerData::default()))
            .unwrap();
        let connection = client::Connection::from_socket(client_socket).unwrap();
        let mut queue = connection.new_event_queue::<ClientState>();
        let qh = queue.handle();
        let registry = connection.display().get_registry(&qh, ());
        connection.flush().unwrap();
        display.dispatch_clients(&mut state).unwrap();
        display.flush_clients().unwrap();
        let mut client_state = ClientState::default();
        queue.blocking_dispatch(&mut client_state).unwrap();

        let shm: wl_shm::WlShm = registry.bind(client_state.shm.unwrap(), 1, &qh, ());
        let compositor: wl_compositor::WlCompositor =
            registry.bind(client_state.compositor.unwrap(), 2, &qh, ());
        let subcompositor: wl_subcompositor::WlSubcompositor =
            registry.bind(client_state.subcompositor.unwrap(), 1, &qh, ());
        let parent = compositor.create_surface(&qh, ());
        let child = compositor.create_surface(&qh, ());
        let _subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
        let fd = unsafe { libc::memfd_create(c"clarion-sync-child".as_ptr(), libc::MFD_CLOEXEC) };
        assert!(fd >= 0);
        let file = unsafe { std::fs::File::from_raw_fd(fd) };
        file.set_len(128).unwrap();
        let pool = shm.create_pool(file.as_fd(), 128, &qh, ());
        let parent_buffer = pool.create_buffer(0, 4, 4, 16, wl_shm::Format::Argb8888, &qh, ());
        let child_buffer = pool.create_buffer(64, 4, 4, 16, wl_shm::Format::Argb8888, &qh, ());
        connection.flush().unwrap();
        display.dispatch_clients(&mut state).unwrap();
        let server_parent: server::protocol::wl_surface::WlSurface = peer
            .object_from_protocol_id(&handle, parent.id().protocol_id())
            .unwrap();
        let server_child: server::protocol::wl_surface::WlSurface = peer
            .object_from_protocol_id(&handle, child.id().protocol_id())
            .unwrap();
        let server_child_buffer: WlBuffer = peer
            .object_from_protocol_id(&handle, child_buffer.id().protocol_id())
            .unwrap();

        child.attach(Some(&child_buffer), 0, 0);
        child.commit();
        connection.flush().unwrap();
        display.dispatch_clients(&mut state).unwrap();
        assert!(super::native::recorded_surface_buffer_id(&server_child).is_none());

        parent.attach(Some(&parent_buffer), 0, 0);
        parent.commit();
        connection.flush().unwrap();
        display.dispatch_clients(&mut state).unwrap();
        assert_eq!(
            super::native::recorded_surface_buffer_id(&server_child),
            Some(server_child_buffer.id())
        );
        assert!(super::native::recorded_surface_buffer_id(&server_parent).is_some());
    }

    #[test]
    #[ignore = "requires real DRM render node, native Vulkan and Wayland SHM"]
    fn native_wayland_shm_rotating_buffers_and_format_change() {
        use smithay::backend::renderer::ImportAll;

        use super::super::native::NativeRenderer;
        let (vk, mut oracle, mut allocator) = setup();
        let mut output = allocator
            .create_buffer(
                8,
                4,
                Fourcc::Argb8888,
                &modifiers(&vk, &oracle, Fourcc::Argb8888),
            )
            .unwrap()
            .export()
            .unwrap();
        let mut renderer = NativeRenderer::new(vk).unwrap();
        let mut display = server::Display::<ServerState>::new().unwrap();
        let mut handle = display.handle();
        let mut state = ServerState {
            shm: ShmState::new::<ServerState>(&handle, []),
            compositor: CompositorState::new::<ServerState>(&handle),
        };
        let (server_socket, client_socket) = UnixStream::pair().unwrap();
        let peer = handle
            .insert_client(server_socket, Arc::new(PeerData::default()))
            .unwrap();
        let connection = client::Connection::from_socket(client_socket).unwrap();
        let mut queue = connection.new_event_queue::<ClientState>();
        let qh = queue.handle();
        let registry = connection.display().get_registry(&qh, ());
        connection.flush().unwrap();
        display.dispatch_clients(&mut state).unwrap();
        display.flush_clients().unwrap();
        let mut client_state = ClientState::default();
        queue.blocking_dispatch(&mut client_state).unwrap();
        let shm: wl_shm::WlShm = registry.bind(client_state.shm.unwrap(), 1, &qh, ());
        let compositor: wl_compositor::WlCompositor =
            registry.bind(client_state.compositor.unwrap(), 2, &qh, ());
        let client_surface = compositor.create_surface(&qh, ());
        let fd = unsafe { libc::memfd_create(c"clarion-shm-test".as_ptr(), libc::MFD_CLOEXEC) };
        assert!(fd >= 0);
        let file = unsafe { std::fs::File::from_raw_fd(fd) };
        file.set_len(512).unwrap();
        let pool = shm.create_pool(file.as_fd(), 512, &qh, ());
        let client_a = pool.create_buffer(32, 8, 4, 48, wl_shm::Format::Argb8888, &qh, ());
        let client_b = pool.create_buffer(256, 8, 4, 48, wl_shm::Format::Argb8888, &qh, ());
        let client_opaque = pool.create_buffer(256, 8, 4, 48, wl_shm::Format::Xrgb8888, &qh, ());
        connection.flush().unwrap();
        display.dispatch_clients(&mut state).unwrap();
        let a: WlBuffer = peer
            .object_from_protocol_id(&handle, client_a.id().protocol_id())
            .unwrap();
        let b: WlBuffer = peer
            .object_from_protocol_id(&handle, client_b.id().protocol_id())
            .unwrap();
        let opaque: WlBuffer = peer
            .object_from_protocol_id(&handle, client_opaque.id().protocol_id())
            .unwrap();
        let surface: server::protocol::wl_surface::WlSurface = peer
            .object_from_protocol_id(&handle, client_surface.id().protocol_id())
            .unwrap();
        // A server-side wl_buffer carrying a real DMA-BUF exercises ImportAll's
        // buffer-kind switch without claiming linux-dmabuf protocol negotiation.
        let dma_buffer = peer
            .create_resource::<WlBuffer, _, ServerState>(&handle, 1, output.clone())
            .unwrap();
        with_states(&surface, |surface| {
            let mut old_image = None;
            for step in 0..7 {
                let offset = if step == 0 || step == 2 { 32 } else { 256 };
                let mut bytes = vec![0x99; 176];
                for y in 0..4 {
                    for x in 0..8 {
                        let bgra = match step {
                            0 => [0, 0, 255, 255],
                            1 if x < 4 => [0, 255, 0, 255],
                            2 if x >= 4 => [255, 0, 0, 255],
                            5 => [255; 4],
                            6 => [255, 255, 255, 0],
                            _ => [99; 4],
                        };
                        bytes[y * 48 + x * 4..y * 48 + x * 4 + 4].copy_from_slice(&bgra);
                    }
                }
                file.write_all_at(&bytes, offset).unwrap();
                let buffer = match step {
                    0 | 2 => &a,
                    1 | 3 | 4 | 5 => &b,
                    _ => &opaque,
                };
                let damage = match step {
                    // Many overlapping client rectangles take the bounded
                    // upload fallback and must still preserve the right half.
                    1 => vec![Rectangle::new((0, 0).into(), (4, 4).into()); 100],
                    2 => vec![Rectangle::new((4, 0).into(), (4, 4).into())],
                    4 => vec![
                        Rectangle::new((-4, 0).into(), (2, 4).into()),
                        Rectangle::new((8, 4).into(), (5, 5).into()),
                        Rectangle::new((0, 0).into(), (0, 4).into()),
                    ],
                    _ => vec![],
                };
                if step == 5 {
                    renderer
                        .import_buffer(&dma_buffer, Some(surface), &[])
                        .expect("DMA-BUF must be recognized")
                        .unwrap();
                }
                // A new SHM texture and a later damaged update must both
                // return while the GPU queue is blocked. The subsequent draw
                // verifies that queue ordering still produces the right pixels.
                let blocked = (step <= 1)
                    .then(|| BlockedQueue::new(renderer.vulkan().ctx.as_ref().unwrap().clone()));
                let texture = renderer
                    .import_buffer(buffer, Some(surface), &damage)
                    .expect("SHM buffer must be recognized by native ImportAll")
                    .unwrap();
                drop(blocked);
                if (1..=4).contains(&step) {
                    assert_eq!(old_image, Some(texture.image()));
                }
                if step >= 5 {
                    assert_ne!(old_image, Some(texture.image()));
                }
                old_image = Some(texture.image());
                let mut target = renderer.bind(&mut output).unwrap();
                let mut frame = renderer
                    .render(&mut target, (8, 4).into(), Transform::Normal)
                    .unwrap();
                let all = Rectangle::from_size((8, 4).into());
                frame.clear(Color32F::BLACK, &[all]).unwrap();
                frame
                    .render_texture_from_to(
                        &texture,
                        Rectangle::from_size((8., 4.).into()),
                        all,
                        &[all],
                        &[],
                        Transform::Normal,
                        1.,
                    )
                    .unwrap();
                frame.finish().unwrap().wait().unwrap();
                drop(target);
                for (i, pixel) in pixels(&mut oracle, &mut output)
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .enumerate()
                {
                    let expected = match step {
                        0 => [255, 0, 0, 255],
                        1..=4 if i % 8 < 4 => [0, 255, 0, 255],
                        1 => [255, 0, 0, 255],
                        2..=4 => [0, 0, 255, 255],
                        _ => [255; 4],
                    };
                    assert_eq!(*pixel, expected, "SHM step {step}, pixel {i}");
                }
            }
        });
        // Exercise the actual committed surface tree and standard render elements.
        let subcompositor: wl_subcompositor::WlSubcompositor =
            registry.bind(client_state.subcompositor.unwrap(), 1, &qh, ());
        let child = compositor.create_surface(&qh, ());
        let subsurface = subcompositor.get_subsurface(&child, &client_surface, &qh, ());
        subsurface.set_position(2, 1);
        for (offset, color) in [(32, [0, 0, 255, 255]), (256, [0, 255, 0, 0])] {
            let mut bytes = vec![0; 176];
            for y in 0..4 {
                for x in 0..8 {
                    bytes[y * 48 + x * 4..y * 48 + x * 4 + 4].copy_from_slice(&color);
                }
            }
            file.write_all_at(&bytes, offset).unwrap();
        }
        child.attach(Some(&client_opaque), 0, 0);
        child.damage(0, 0, 8, 4);
        child.commit();
        client_surface.attach(Some(&client_a), 0, 0);
        client_surface.damage(0, 0, 8, 4);
        client_surface.commit();
        connection.flush().unwrap();
        display.dispatch_clients(&mut state).unwrap();
        use smithay::backend::renderer::element::surface::{
            render_elements_from_surface_tree, WaylandSurfaceRenderElement,
        };
        use smithay::backend::renderer::element::{Element, Kind, RenderElement};
        let elements: Vec<WaylandSurfaceRenderElement<NativeRenderer>> =
            render_elements_from_surface_tree(
                &mut renderer,
                &surface,
                (0, 0),
                1.,
                1.,
                Kind::Unspecified,
            );
        assert_eq!(
            elements.len(),
            2,
            "root and committed synchronized subsurface"
        );
        let mut target = renderer.bind(&mut output).unwrap();
        let mut frame = renderer
            .render(&mut target, (8, 4).into(), Transform::Normal)
            .unwrap();
        frame
            .clear(Color32F::BLACK, &[Rectangle::from_size((8, 4).into())])
            .unwrap();
        for element in elements.iter().rev() {
            let dst = element.geometry(1.0.into());
            element
                .draw(
                    &mut frame,
                    element.src(),
                    dst,
                    &[Rectangle::from_size(dst.size)],
                    &[],
                    None,
                )
                .unwrap();
        }
        frame.finish().unwrap().wait().unwrap();
        drop(target);
        for (i, pixel) in pixels(&mut oracle, &mut output)
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
        {
            let expected = if i % 8 >= 2 && i / 8 >= 1 {
                [0, 255, 0, 255]
            } else {
                [255, 0, 0, 255]
            };
            assert_eq!(*pixel, expected, "native surface tree pixel {i}");
        }
        // Exercise the real rounded surface wrapper on both backends, including
        // a child crossing the parent's clip rectangle.
        use crate::render_helpers::clipped_surface::ClippedSurfaceRenderElement;
        use crate::render_helpers::{render_to_vec, resources, shaders};
        resources::init(&mut oracle);
        shaders::init(&mut oracle);
        use client::protocol::wl_output::Transform as ClientTransform;
        for buffer_transform in [
            ClientTransform::Normal,
            ClientTransform::_90,
            ClientTransform::_180,
            ClientTransform::_270,
            ClientTransform::Flipped,
            ClientTransform::Flipped90,
            ClientTransform::Flipped180,
            ClientTransform::Flipped270,
        ] {
            client_surface.set_buffer_transform(buffer_transform);
            client_surface.commit();
            connection.flush().unwrap();
            display.dispatch_clients(&mut state).unwrap();
            for scale in [1., 1.5, 2.] {
                for transform in [
                    Transform::Normal,
                    Transform::_90,
                    Transform::_180,
                    Transform::_270,
                    Transform::Flipped,
                    Transform::Flipped90,
                    Transform::Flipped180,
                    Transform::Flipped270,
                ] {
                    let geometry = Rectangle::new((1., 0.5).into(), (6., 3.).into());
                    let radius = clarion_config::CornerRadius {
                        top_left: 1.,
                        top_right: 1.5,
                        bottom_right: 0.5,
                        bottom_left: 1.,
                    };
                    let native_elements: Vec<WaylandSurfaceRenderElement<NativeRenderer>> =
                        render_elements_from_surface_tree(
                            &mut renderer,
                            &surface,
                            (0, 0),
                            scale,
                            0.7,
                            Kind::Unspecified,
                        );
                    let clipped: Vec<_> = native_elements
                        .into_iter()
                        .map(|e| {
                            ClippedSurfaceRenderElement::new(e, scale.into(), geometry, (), radius)
                        })
                        .collect();
                    let reference_elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                        render_elements_from_surface_tree(
                            &mut oracle,
                            &surface,
                            (0, 0),
                            scale,
                            0.7,
                            Kind::Unspecified,
                        );
                    let program = ClippedSurfaceRenderElement::shader(&mut oracle)
                        .unwrap()
                        .clone();
                    let reference_clipped: Vec<_> = reference_elements
                        .into_iter()
                        .map(|e| {
                            ClippedSurfaceRenderElement::new(
                                e,
                                scale.into(),
                                geometry,
                                program.clone(),
                                radius,
                            )
                        })
                        .collect();
                    let a = render_to_vec(
                        &mut renderer,
                        (20, 14).into(),
                        scale.into(),
                        transform,
                        Fourcc::Abgr8888,
                        clipped.iter(),
                    )
                    .unwrap();
                    let b = render_to_vec(
                        &mut oracle,
                        (20, 14).into(),
                        scale.into(),
                        transform,
                        Fourcc::Abgr8888,
                        reference_clipped.iter(),
                    )
                    .unwrap();
                    let worst = a.iter().zip(&b).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
                    assert!(
                        worst <= 3,
                        "rounded surface mismatch input={buffer_transform:?} scale={scale} output={transform:?}: {worst}"
                    );
                    assert!(
                        a.as_chunks::<4>().0.iter().any(|p| p[3] > 0),
                        "empty clipped surface input={buffer_transform:?} scale={scale} output={transform:?}"
                    );
                }
            }
        }
        client_surface.set_buffer_transform(ClientTransform::Normal);
        client_surface.commit();
        connection.flush().unwrap();
        display.dispatch_clients(&mut state).unwrap();
        eprintln!(
            "PASS: actual native rounded root/subsurface clipping, asymmetric radii, fractional scale and output transforms"
        );
        let mut snapshots = Vec::new();
        crate::render_helpers::surface::render_snapshot_from_surface_tree(
            &mut renderer,
            &surface,
            (0., 0.).into(),
            &mut snapshots,
        );
        assert_eq!(snapshots.len(), 2);
        drop(elements);
        subsurface.destroy();
        child.destroy();
        client_surface.destroy();
        connection.flush().unwrap();
        display.dispatch_clients(&mut state).unwrap();
        use crate::render_helpers::ToRenderElement;
        let mut target = renderer.bind(&mut output).unwrap();
        let mut frame = renderer
            .render(&mut target, (8, 4).into(), Transform::Normal)
            .unwrap();
        frame
            .clear(Color32F::BLACK, &[Rectangle::from_size((8, 4).into())])
            .unwrap();
        for baked in snapshots.iter().rev() {
            let element =
                baked.to_render_element((0., 0.).into(), 1.0.into(), 1., Kind::Unspecified);
            let dst = element.geometry(1.0.into());
            <_ as RenderElement<NativeRenderer>>::draw(
                &element,
                &mut frame,
                element.src(),
                dst,
                &[Rectangle::from_size(dst.size)],
                &[],
                None,
            )
            .unwrap();
        }
        frame.finish().unwrap().wait().unwrap();
        drop(target);
        for (i, pixel) in pixels(&mut oracle, &mut output)
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
        {
            let expected = if i % 8 >= 2 && i / 8 >= 1 {
                [0, 255, 0, 255]
            } else {
                [255, 0, 0, 255]
            };
            assert_eq!(
                *pixel, expected,
                "native snapshot after surface destruction: {i}"
            );
        }
        use smithay::backend::renderer::damage::OutputDamageTracker;
        use smithay::backend::renderer::element::RenderElementStates;

        use crate::render_helpers::render_to_shm;
        use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
        let blue = SolidColorBuffer::new((8., 4.), [0., 0., 1., 1.]);
        // Asymmetric rows catch accidental readback inversion; a solid fill cannot.
        let red = SolidColorBuffer::new((8., 1.), [1., 0., 0., 1.]);
        let capture_elements = [
            SolidColorRenderElement::from_buffer(&red, (0., 0.), 1., Kind::Unspecified),
            SolidColorRenderElement::from_buffer(&blue, (0., 0.), 1., Kind::Unspecified),
        ];
        let mut capture_damage = OutputDamageTracker::new((8, 4), 1., Transform::Normal);
        file.write_all_at(&[0xa5; 512], 0).unwrap();
        assert!(
            render_to_shm(
                &mut renderer,
                &mut capture_damage,
                &a,
                &capture_elements,
                RenderElementStates::default()
            )
            .is_err(),
            "reject unsupported capture format"
        );
        let mut captured = [0; 512];
        file.read_exact_at(&mut captured, 0).unwrap();
        assert_eq!(captured, [0xa5; 512]);
        render_to_shm(
            &mut renderer,
            &mut capture_damage,
            &opaque,
            &capture_elements,
            RenderElementStates::default(),
        )
        .unwrap();
        file.read_exact_at(&mut captured, 0).unwrap();
        let mut expected_pool = [0xa5; 512];
        for y in 0..4 {
            for x in 0..8 {
                let offset = 256 + y * 48 + x * 4;
                expected_pool[offset..offset + 4].copy_from_slice(if y == 0 {
                    &[0, 0, 255, 255]
                } else {
                    &[255, 0, 0, 255]
                });
            }
        }
        assert_eq!(
            captured, expected_pool,
            "native SHM output must preserve prefix/padding/suffix"
        );
        file.write_all_at(&[0xa5; 512], 0).unwrap();
        let mut reference_damage = OutputDamageTracker::new((8, 4), 1., Transform::Normal);
        render_to_shm(
            &mut oracle,
            &mut reference_damage,
            &opaque,
            &capture_elements,
            RenderElementStates::default(),
        )
        .unwrap();
        file.read_exact_at(&mut captured, 0).unwrap();
        assert_eq!(captured, expected_pool, "GLES SHM output compatibility");
        eprintln!(
            "PASS: Wayland SHM offset/stride, rotating buffers, damage and opaque format change"
        );
    }
}

#[test]
#[ignore = "requires real DRM render node, native Vulkan and EGL readback"]
fn native_opaque_offscreen_attachment_and_sampling_views() {
    use super::native::NativeRenderer;
    let (vk, mut oracle, mut allocator) = setup();
    let mut output = allocator
        .create_buffer(
            16,
            16,
            Fourcc::Argb8888,
            &modifiers(&vk, &oracle, Fourcc::Argb8888),
        )
        .unwrap()
        .export()
        .unwrap();
    let mut renderer = NativeRenderer::new(vk).unwrap();
    let all = Rectangle::from_size((16, 16).into());
    for format in [
        Fourcc::Xrgb8888,
        Fourcc::Xbgr8888,
        Fourcc::Xrgb2101010,
        Fourcc::Xbgr2101010,
    ] {
        let mut source = renderer.create_buffer(format, (16, 16).into()).unwrap();
        assert_eq!(
            smithay::backend::renderer::Texture::format(&source),
            Some(format)
        );
        assert_ne!(source.image_view(), source.sampled_image_view());
        for red in [true, false, true] {
            let mut target = renderer.bind(&mut source).unwrap();
            let mut frame = renderer
                .render(&mut target, (16, 16).into(), Transform::Normal)
                .unwrap();
            // Deliberately write zero/partial physical alpha into opaque storage.
            frame.clear(Color32F::TRANSPARENT, &[all]).unwrap();
            if red {
                frame
                    .draw_solid(all, &[all], [0.5, 0., 0., 0.5].into())
                    .unwrap();
            }
            frame.finish().unwrap().wait().unwrap();
            drop(target);
            let mut target = renderer.bind(&mut output).unwrap();
            let mut frame = renderer
                .render(&mut target, (16, 16).into(), Transform::Normal)
                .unwrap();
            frame.clear([0., 0., 1., 1.].into(), &[all]).unwrap();
            frame
                .render_texture_from_to(
                    &source,
                    Rectangle::from_size((16., 16.).into()),
                    all,
                    &[all],
                    &[],
                    Transform::Normal,
                    1.,
                )
                .unwrap();
            frame.finish().unwrap().wait().unwrap();
            drop(target);
            let expected = [if red { 128 } else { 0 }, 0, 0, 255];
            for pixel in pixels(&mut oracle, &mut output).as_chunks::<4>().0 {
                assert!(
                    pixel.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 2),
                    "opaque {format:?}: expected {expected:?}, got {pixel:?}"
                );
            }
        }
    }
    eprintln!(
        "PASS: opaque 8/10-bit offscreen attachment identity, alpha-one sampling and repeated reuse"
    );
}

#[test]
#[ignore = "requires real DRM render node, native Vulkan and EGL pixel oracle"]
fn native_export_memory_formats_regions_and_reuse() {
    use smithay::backend::renderer::{ImportMem, Texture, TextureMapping};

    use super::native::NativeRenderer;
    let (vk, mut oracle, mut allocator) = setup();
    let mut output = allocator
        .create_buffer(
            4,
            4,
            Fourcc::Argb8888,
            &modifiers(&vk, &oracle, Fourcc::Argb8888),
        )
        .unwrap()
        .export()
        .unwrap();
    let mut renderer = NativeRenderer::new(vk).unwrap();
    let all = Rectangle::from_size((4, 4).into());
    let physical = Rectangle::from_size((4, 4).into());
    for code in [
        Fourcc::Argb8888,
        Fourcc::Abgr8888,
        Fourcc::Xrgb8888,
        Fourcc::Xbgr8888,
        Fourcc::Argb2101010,
        Fourcc::Abgr2101010,
        Fourcc::Xrgb2101010,
        Fourcc::Xbgr2101010,
    ] {
        let mut source = renderer.create_buffer(code, (4, 4).into()).unwrap();
        assert!(renderer.can_read_texture(&source).unwrap());
        assert!(
            renderer.copy_texture(&source, all, code).is_err(),
            "undefined source must be rejected"
        );
        {
            let mut fb = renderer.bind(&mut source).unwrap();
            let mut frame = renderer
                .render(&mut fb, (4, 4).into(), Transform::Normal)
                .unwrap();
            frame.clear([1., 0., 0., 1.].into(), &[physical]).unwrap();
            frame
                .draw_solid(
                    Rectangle::new((1, 1).into(), (2, 2).into()),
                    &[Rectangle::from_size((2, 2).into())],
                    [0., 0.5, 0., 0.5].into(),
                )
                .unwrap();
            frame.finish().unwrap().wait().unwrap();
        }
        let raw = renderer.copy_texture(&source, all, code).unwrap();
        assert_eq!(Texture::format(&raw), Some(code));
        assert_eq!(renderer.map_texture(&raw).unwrap().len(), 64);
        let crop = renderer
            .copy_texture(
                &source,
                Rectangle::new((1, 0).into(), (2, 3).into()),
                Fourcc::Abgr8888,
            )
            .unwrap();
        assert_eq!(crop.size(), (2, 3).into());
        assert!(!crop.flipped());
        for (i, pixel) in renderer
            .map_texture(&crop)
            .unwrap()
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
        {
            let expected = if i < 2 {
                [255, 0, 0, 255]
            } else {
                [128, 128, 0, 255]
            };
            assert!(
                pixel.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 2),
                "{code:?} crop {i}: {pixel:?}"
            );
        }
        let snapshot = renderer.map_texture(&crop).unwrap().to_vec();
        assert!(renderer
            .copy_texture(&source, Rectangle::new((-1, 0).into(), (2, 3).into()), code)
            .is_err());
        assert!(renderer
            .copy_texture(&source, Rectangle::new((3, 0).into(), (2, 3).into()), code)
            .is_err());
        assert!(renderer.copy_texture(&source, all, Fourcc::Nv12).is_err());
        assert!(renderer.copy_texture(&source, all, Fourcc::Rgb565).is_err());
        // Readback must preserve the source for sampling and allow the external
        // target to be acquired, released, and rendered again on the next pass.
        for draw_source in [true, false] {
            let mut fb = renderer.bind(&mut output).unwrap();
            let mut frame = renderer
                .render(&mut fb, (4, 4).into(), Transform::Normal)
                .unwrap();
            frame.clear([0., 0., 1., 1.].into(), &[physical]).unwrap();
            if draw_source {
                frame
                    .render_texture_from_to(
                        &source,
                        Rectangle::from_size((4., 4.).into()),
                        physical,
                        &[physical],
                        &[],
                        Transform::Normal,
                        1.,
                    )
                    .unwrap();
            }
            frame.finish().unwrap().wait().unwrap();
            let native = renderer
                .copy_framebuffer(&fb, all, Fourcc::Abgr8888)
                .unwrap();
            drop(fb);
            let expected = pixels(&mut oracle, &mut output);
            assert!(renderer
                .map_texture(&native)
                .unwrap()
                .iter()
                .zip(expected)
                .all(|(a, b)| a.abs_diff(b) <= 2));
        }
        assert_eq!(renderer.map_texture(&crop).unwrap(), snapshot);
    }
    let texture = renderer
        .import_memory(
            &[1, 2, 3, 4, 5, 6, 7, 8],
            Fourcc::Abgr8888,
            (1, 2).into(),
            true,
        )
        .unwrap();
    let mapped = renderer
        .copy_texture(
            &texture,
            Rectangle::from_size((1, 2).into()),
            Fourcc::Argb8888,
        )
        .unwrap();
    assert!(mapped.flipped());
    drop(texture);
    assert_eq!(
        renderer.map_texture(&mapped).unwrap(),
        &[3, 2, 1, 4, 7, 6, 5, 8]
    );
    eprintln!(
        "PASS: native ExportMem 8/10-bit formats, crop, immutable mappings, foreign ownership and redraw"
    );
}

#[test]
#[ignore = "requires real DRM render node and Vulkan validation"]
fn native_texture_cross_thread_descriptors_and_last_drop() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<VulkanTexture>();
    let (mut renderer, mut oracle, mut allocator) = setup();
    let mut target = allocator
        .create_buffer(
            16,
            16,
            Fourcc::Argb8888,
            &modifiers(&renderer, &oracle, Fourcc::Argb8888),
        )
        .unwrap()
        .export()
        .unwrap();
    let shared = renderer.create_solid_texture([20, 40, 60, 255]).unwrap();
    let manager = renderer.pipeline_mgr.as_ref().unwrap().clone();
    let batches: Vec<Vec<_>> = (0..8)
        .map(|_| {
            (0..64)
                .map(|_| renderer.create_solid_texture([255; 4]).unwrap())
                .collect()
        })
        .collect();
    let barrier = Arc::new(std::sync::Barrier::new(9));
    let threads: Vec<_> = batches
        .into_iter()
        .map(|textures| {
            let shared = shared.clone();
            let manager = manager.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                let linear = manager
                    .get_or_create_texture_descriptor_set(&shared, true)
                    .unwrap();
                let nearest = manager
                    .get_or_create_texture_descriptor_set(&shared, false)
                    .unwrap();
                for texture in textures {
                    manager
                        .get_or_create_texture_descriptor_set(&texture, true)
                        .unwrap();
                    manager
                        .get_or_create_texture_descriptor_set(&texture, false)
                        .unwrap();
                    drop(texture);
                    assert_eq!(
                        manager
                            .get_or_create_texture_descriptor_set(&shared, true)
                            .unwrap(),
                        linear
                    );
                }
                (linear, nearest)
            })
        })
        .collect();
    barrier.wait();
    for _ in 0..32 {
        renderer
            .render_to_target_dmabuf(&target, Some([0.; 4]), |frame| {
                frame.draw_texture(&shared, 0., 0., 16., 16., None, 1., None, 0.)
            })
            .unwrap();
    }
    let sets: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    assert!(sets.iter().all(|pair| *pair == sets[0]));
    assert!(pixels(&mut oracle, &mut target)
        .as_chunks::<4>()
        .0
        .iter()
        .all(|p| *p == [20, 40, 60, 255]));
    // Keep the final image and its pool leases alive after the renderer is gone,
    // then release the last resource owner on a different thread.
    drop(manager);
    drop(renderer);
    std::thread::spawn(move || drop(shared)).join().unwrap();
    eprintln!(
        "PASS: Send+Sync native textures, concurrent descriptor allocation/free and cross-thread final owner"
    );
}

#[test]
#[ignore = "requires real DRM render node and native Vulkan"]
fn native_lazy_snapshot_composition_and_privacy_branches() {
    use super::native::NativeRenderer;
    use crate::render_helpers::snapshot::RenderSnapshot;
    use crate::render_helpers::solid_color::SolidColorBuffer;
    use crate::render_helpers::{BakedBuffer, RenderCtx, RenderTarget};
    let (vk, _oracle, _allocator) = setup();
    let mut renderer = NativeRenderer::new(vk).unwrap();
    // This fixture exercises prepared rendering; wait on the bounded test-only
    // preparer before its pixel, timing or ownership assertions begin.
    renderer
        .prepare_output_format_for_test(Fourcc::Abgr8888)
        .unwrap();
    let baked = |color: [f32; 4], x: f64, width: f64| BakedBuffer {
        buffer: SolidColorBuffer::new((width, 2.), color),
        location: (x, 3.).into(),
        src: None,
        dst: None,
    };
    let mut snapshot: RenderSnapshot<_, _, VulkanTexture> = RenderSnapshot {
        contents: vec![baked([1.; 4], 0., 2.), baked([1., 0., 0., 1.], -2., 4.)],
        contents_with_blocked_out_bg: Some(vec![baked([0., 1., 0., 1.], -2., 4.)]),
        blocked_out_contents: vec![baked([0., 0., 1., 1.], -2., 4.)],
        block_out_from: None,
        size: (4., 2.).into(),
        texture: Default::default(),
        texture_with_blocked_out_bg: Default::default(),
        blocked_out_texture: Default::default(),
    };
    let mut ids = Vec::new();
    for target in [
        RenderTarget::Output,
        RenderTarget::Screencast,
        RenderTarget::ScreenCapture,
    ] {
        if target == RenderTarget::ScreenCapture {
            snapshot.block_out_from = Some(clarion_config::BlockOutFrom::ScreenCapture);
        }
        let (texture, geo) = snapshot
            .texture(
                RenderCtx {
                    renderer: &mut renderer,
                    target,
                    xray: None,
                },
                1.0.into(),
            )
            .unwrap();
        assert_eq!(*geo, Rectangle::new((-2, 3).into(), (4, 2).into()));
        ids.push(texture.image());
        let mapping = renderer
            .copy_texture(
                texture,
                Rectangle::from_size((4, 2).into()),
                Fourcc::Abgr8888,
            )
            .unwrap();
        for (i, pixel) in renderer
            .map_texture(&mapping)
            .unwrap()
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
        {
            let expected = match target {
                RenderTarget::Output if i % 4 < 2 => [255, 0, 0, 255],
                RenderTarget::Output => [255; 4],
                RenderTarget::Screencast => [0, 255, 0, 255],
                RenderTarget::ScreenCapture => [0, 0, 255, 255],
            };
            assert_eq!(*pixel, expected, "lazy snapshot {target:?} pixel {i}");
        }
    }
    assert!(ids[0] != ids[1] && ids[1] != ids[2] && ids[0] != ids[2]);
    // Once rendered, changing live contents does not rebuild an existing snapshot.
    snapshot.contents[1].buffer.set_color([0., 0., 0., 1.]);
    let (cached, _) = snapshot
        .texture(
            RenderCtx {
                renderer: &mut renderer,
                target: RenderTarget::Output,
                xray: None,
            },
            1.0.into(),
        )
        .unwrap();
    assert_eq!(cached.image(), ids[0]);
    let mapping = renderer
        .copy_texture(
            cached,
            Rectangle::from_size((1, 1).into()),
            Fourcc::Abgr8888,
        )
        .unwrap();
    assert_eq!(renderer.map_texture(&mapping).unwrap(), &[255, 0, 0, 255]);
    eprintln!(
        "PASS: native lazy snapshots, encompassing origin, draw order, privacy branches and cache reuse"
    );
}

#[test]
#[ignore = "requires real DRM render node, Vulkan and GLES reference"]
fn native_render_and_download_matches_gles() {
    use smithay::backend::renderer::element::Kind;

    use super::native::NativeRenderer;
    use crate::render_helpers::render_to_vec;
    use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
    let (vk, mut reference, _allocator) = setup();
    let mut native = NativeRenderer::new(vk).unwrap();
    let background = SolidColorBuffer::new((6., 6.), [0.25, 0., 0.5, 0.5]);
    let foreground = SolidColorBuffer::new((2., 3.), [0., 0.5, 0., 0.5]);
    let elements = [
        SolidColorRenderElement::from_buffer(&background, (0., 0.), 1., Kind::Unspecified),
        SolidColorRenderElement::from_buffer(&foreground, (1., 1.), 1., Kind::Unspecified),
    ];
    for transform in [
        Transform::Normal,
        Transform::_90,
        Transform::_180,
        Transform::_270,
        Transform::Flipped,
        Transform::Flipped90,
        Transform::Flipped180,
        Transform::Flipped270,
    ] {
        for format in [Fourcc::Argb8888, Fourcc::Abgr8888] {
            let actual = render_to_vec(
                &mut native,
                (6, 4).into(),
                1.0.into(),
                transform,
                format,
                elements.iter(),
            )
            .unwrap();
            let expected = render_to_vec(
                &mut reference,
                (6, 4).into(),
                1.0.into(),
                transform,
                format,
                elements.iter(),
            )
            .unwrap();
            assert_eq!(actual.len(), 6 * 4 * 4);
            assert!(
                actual.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 2),
                "native render/download mismatch: {transform:?} {format:?}"
            );
        }
    }
    eprintln!(
        "PASS: generic native render/download, non-square transforms, channel order and alpha composition"
    );
}

#[test]
#[ignore = "requires real DRM render node, Vulkan and GLES reference"]
fn native_damage_tracker_dmabuf_updates_and_idle_frames() {
    use smithay::backend::renderer::damage::OutputDamageTracker;
    use smithay::backend::renderer::element::{Kind, RenderElementStates};

    use super::native::NativeRenderer;
    use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
    use crate::render_helpers::{clear_dmabuf, render_to_dmabuf};
    let (vk, mut reference, mut allocator) = setup();
    let mods = modifiers(&vk, &reference, Fourcc::Argb8888);
    let mut native = NativeRenderer::new(vk).unwrap();
    for transform in [
        Transform::Normal,
        Transform::_90,
        Transform::_180,
        Transform::_270,
        Transform::Flipped,
        Transform::Flipped90,
        Transform::Flipped180,
        Transform::Flipped270,
    ] {
        let mut actual = allocator
            .create_buffer(6, 4, Fourcc::Argb8888, &mods)
            .unwrap()
            .export()
            .unwrap();
        let mut expected = allocator
            .create_buffer(6, 4, Fourcc::Argb8888, &mods)
            .unwrap()
            .export()
            .unwrap();
        let mut native_damage = OutputDamageTracker::new((6, 4), 1., transform);
        let mut reference_damage = OutputDamageTracker::new((6, 4), 1., transform);
        let background = SolidColorBuffer::new((6., 6.), [0., 0., 1., 1.]);
        let mut foreground = SolidColorBuffer::new((2., 2.), [1., 0., 0., 1.]);
        for step in 0..4 {
            if step == 3 {
                foreground.set_color([0., 1., 0., 1.]);
            }
            let elements = [
                SolidColorRenderElement::from_buffer(
                    &foreground,
                    (if step == 0 { 0. } else { 1. }, 1.),
                    1.,
                    Kind::Unspecified,
                ),
                SolidColorRenderElement::from_buffer(&background, (0., 0.), 1., Kind::Unspecified),
            ];
            if step == 0 {
                render_to_dmabuf(
                    &mut native,
                    &mut native_damage,
                    actual.clone(),
                    &elements,
                    RenderElementStates::default(),
                )
                .unwrap()
                .wait()
                .unwrap();
                render_to_dmabuf(
                    &mut reference,
                    &mut reference_damage,
                    expected.clone(),
                    &elements,
                    RenderElementStates::default(),
                )
                .unwrap()
                .wait()
                .unwrap();
            } else {
                let mut fb = native.bind(&mut actual).unwrap();
                let result = native_damage
                    .render_output_with_states(
                        &mut native,
                        &mut fb,
                        1,
                        &elements,
                        Color32F::TRANSPARENT,
                        RenderElementStates::default(),
                    )
                    .unwrap();
                assert_eq!(
                    result.damage.is_none(),
                    step == 2,
                    "native damage {transform:?} step {step}"
                );
                result.sync.wait().unwrap();
                drop(fb);
                let mut fb = reference.bind(&mut expected).unwrap();
                let result = reference_damage
                    .render_output_with_states(
                        &mut reference,
                        &mut fb,
                        1,
                        &elements,
                        Color32F::TRANSPARENT,
                        RenderElementStates::default(),
                    )
                    .unwrap();
                assert_eq!(result.damage.is_none(), step == 2);
                result.sync.wait().unwrap();
            }
            assert_eq!(
                pixels(&mut reference, &mut actual),
                pixels(&mut reference, &mut expected),
                "damage output differs: {transform:?} step {step}"
            );
        }
        clear_dmabuf(&mut native, actual.clone())
            .unwrap()
            .wait()
            .unwrap();
        assert!(pixels(&mut reference, &mut actual).iter().all(|p| *p == 0));
    }
    eprintln!(
        "PASS: native DMA-BUF helper, damage-tracked moves/content changes, idle frames and clear across transforms"
    );
}

#[test]
#[ignore = "requires real DRM render node and native fence export"]
fn native_frozen_snapshot_queues_copies_and_draws_without_cpu_wait() {
    use smithay::backend::renderer::ImportMem;

    use crate::layout::LayoutElementRenderSnapshot;
    use crate::render_helpers::snapshot::RenderSnapshot;
    use crate::render_helpers::solid_color::SolidColorBuffer;
    use crate::render_helpers::texture::TextureBuffer;
    use crate::render_helpers::{BakedBuffer, RenderCtx, RenderTarget};

    let (vk, _oracle, _) = setup();
    let ctx = vk.ctx.as_ref().unwrap().clone();
    assert!(ctx.supports_sync_file_export);
    let mut renderer = native::NativeRenderer::new(vk).unwrap();
    let source = renderer
        .import_memory(
            &[255, 0, 0, 255].repeat(16 * 12),
            Fourcc::Abgr8888,
            (16, 12).into(),
            false,
        )
        .unwrap();
    let background = renderer
        .import_memory(
            &[0, 0, 255, 255].repeat(16 * 12),
            Fourcc::Abgr8888,
            (16, 12).into(),
            false,
        )
        .unwrap();
    let weak_source = Arc::downgrade(&source.inner);
    let weak_background = Arc::downgrade(&background.inner);
    let baked = |texture| BakedBuffer {
        buffer: TextureBuffer::from_texture(&renderer, texture, 1., Transform::Normal, Vec::new()),
        location: (0., 0.).into(),
        src: None,
        dst: None,
    };
    let snapshot: LayoutElementRenderSnapshot<VulkanTexture> = RenderSnapshot {
        contents: vec![baked(source)],
        contents_with_blocked_out_bg: Some(vec![baked(background)]),
        blocked_out_contents: vec![BakedBuffer {
            buffer: SolidColorBuffer::new((16., 12.), [0., 0., 0., 1.]),
            location: (0., 0.).into(),
            src: None,
            dst: None,
        }],
        block_out_from: None,
        size: (16., 12.).into(),
        texture: Default::default(),
        texture_with_blocked_out_bg: Default::default(),
        blocked_out_texture: Default::default(),
    };
    let targets = [
        RenderTarget::Output,
        RenderTarget::Screencast,
        RenderTarget::ScreenCapture,
    ];
    let mut outputs = targets.map(|_| {
        renderer
            .create_buffer(Fourcc::Abgr8888, (16, 12).into())
            .unwrap()
    });
    // Use already-submitted GPU work rather than a future timeline signal,
    // so exporting the native completion SYNC_FD remains valid.
    let mut work = SubmittedGpuWork::new(ctx.clone());
    let _producer = work.submit();
    let mut snapshot = snapshot.freeze_native(&mut renderer, 1.0.into()).unwrap();
    assert!(
        unsafe { !ctx.device.get_fence_status(work.fence).unwrap() },
        "native snapshot freezing waited for the busy graphics queue"
    );
    assert!(
        weak_source.upgrade().is_some() && weak_background.upgrade().is_some(),
        "queued snapshot copies must retain their source images"
    );

    let mut syncs = Vec::new();
    let mut queued_copies = Vec::new();
    for (target, output) in targets.into_iter().zip(&mut outputs) {
        if target == RenderTarget::ScreenCapture {
            // ScreenCapture blocking also masks screencasts. Enable it only
            // for the last draw so this test samples all three cache branches.
            snapshot.block_out_from = Some(clarion_config::BlockOutFrom::ScreenCapture);
        }
        let (texture, _) = snapshot
            .texture(
                RenderCtx {
                    renderer: &mut renderer,
                    target,
                    xray: None,
                },
                1.0.into(),
            )
            .unwrap();
        queued_copies.push(Arc::downgrade(&texture.inner));
        let mut fb = renderer.bind(output).unwrap();
        let mut frame = renderer
            .render(&mut fb, (16, 12).into(), Transform::Normal)
            .unwrap();
        let dst = Rectangle::from_size((16, 12).into());
        frame
            .render_texture_from_to(
                texture,
                Rectangle::from_size((16., 12.).into()),
                dst,
                &[dst],
                &[],
                Transform::Normal,
                1.,
            )
            .unwrap();
        syncs.push(frame.finish().unwrap());
    }
    // Drop the entire snapshot while both copies and their consumers are in
    // flight. Completion owners, rather than a CPU wait, protect the images.
    drop(snapshot);
    assert!(
        unsafe { !ctx.device.get_fence_status(work.fence).unwrap() },
        "sampling the queued copies must also remain asynchronous"
    );
    assert!(queued_copies.iter().all(|image| image.upgrade().is_some()));
    work.wait();
    for sync in syncs {
        sync.wait().unwrap();
    }
    for ((output, target), expected) in
        outputs
            .iter()
            .zip(targets)
            .zip([[255, 0, 0, 255], [0, 0, 255, 255], [0, 0, 0, 255]])
    {
        let mapping = renderer
            .copy_texture(
                output,
                Rectangle::from_size((16, 12).into()),
                Fourcc::Abgr8888,
            )
            .unwrap();
        for (index, pixel) in renderer
            .map_texture(&mapping)
            .unwrap()
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
        {
            assert_eq!(*pixel, expected,
                "queued frozen snapshot {target:?} pixel {index} changed its pixels or privacy branch");
        }
    }
}

#[test]
#[ignore = "requires real DRM render node and native fence export"]
fn native_async_finish_returns_before_gpu_and_exports_sync_file() {
    use std::os::fd::{AsRawFd, OwnedFd};

    use smithay::backend::renderer::sync::{Fence, Interrupted, SyncPoint};

    use super::native::NativeRenderer;
    #[derive(Debug)]
    struct Producer(OwnedFd);
    impl Fence for Producer {
        fn is_signaled(&self) -> bool {
            let mut p = libc::pollfd {
                fd: self.0.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            unsafe { libc::poll(&mut p, 1, 0) == 1 && p.revents & libc::POLLIN != 0 }
        }
        fn wait(&self) -> Result<(), Interrupted> {
            panic!("producer must be waited on by GPU")
        }
        fn is_exportable(&self) -> bool {
            true
        }
        fn export(&self) -> Option<OwnedFd> {
            self.0.try_clone().ok()
        }
    }
    let (vk, _oracle, _allocator) = setup();
    let ctx = vk.ctx.as_ref().unwrap().clone();
    assert!(ctx.supports_sync_file_import && ctx.supports_sync_file_export);
    let mut renderer = NativeRenderer::new(vk).unwrap();
    let mut texture = renderer
        .create_buffer(Fourcc::Abgr8888, (16, 16).into())
        .unwrap();
    let mut work = SubmittedGpuWork::new(ctx.clone());
    let mut fb = renderer.bind(&mut texture).unwrap();
    let mut frame = renderer
        .render(&mut fb, (16, 16).into(), Transform::Normal)
        .unwrap();
    let fd = work.submit();
    frame.wait(&SyncPoint::from(Producer(fd))).unwrap();
    frame
        .clear(
            [1., 0., 0., 1.].into(),
            &[Rectangle::from_size((16, 16).into())],
        )
        .unwrap();
    let sync = frame.finish().unwrap();
    let pending = !sync.is_reached();
    let producer_pending = unsafe { !ctx.device.get_fence_status(work.fence).unwrap() };
    let exported = sync.export();
    let exported_again = sync.export();
    assert!(
        pending && producer_pending,
        "finish must return before the submitted GPU workload completes"
    );
    assert!(sync.is_exportable());
    assert_ne!(
        exported.as_ref().unwrap().as_raw_fd(),
        exported_again.as_ref().unwrap().as_raw_fd()
    );
    // This test deliberately submits 32 GiB of fills to keep the producer busy.
    // Its duration is hardware/load dependent and can exceed the compositor's
    // bounded wait. The nonblocking finish and pending-producer assertions above
    // already establish the contract; finish the artificial workload separately.
    unsafe {
        ctx.device
            .wait_for_fences(&[work.fence], true, 30_000_000_000)
            .expect("artificial producer workload must complete within 30 seconds");
    }
    sync.wait().unwrap();
    drop(fb);
    let mapping = renderer
        .copy_texture(
            &texture,
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
        .all(|p| *p == [255, 0, 0, 255]));
    drop(texture);
    drop(sync);
    drop(renderer);
    let mut poll = libc::pollfd {
        fd: exported_again.as_ref().unwrap().as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    assert_eq!(unsafe { libc::poll(&mut poll, 1, 0) }, 1);
    assert_ne!(poll.revents & libc::POLLIN, 0);
    eprintln!(
        "PASS: native finish precedes submitted GPU completion, reusable SYNC_FD export and deferred resources"
    );
}

#[test]
#[ignore = "requires real DRM render node, Vulkan native fences and GLES"]
fn native_external_source_waits_before_offscreen_and_kms_finish() {
    use std::sync::atomic::{AtomicBool, Ordering};

    use smithay::backend::renderer::ImportDma as _;

    use super::native::NativeRenderer;

    let (vk, mut gles, mut allocator) = setup();
    let ctx = vk.ctx.as_ref().unwrap().clone();
    assert!(ctx.supports_sync_file_export);
    let mods = modifiers(&vk, &gles, Fourcc::Argb8888);
    let mut source_dmabuf = allocator
        .create_buffer(8, 8, Fourcc::Argb8888, &mods)
        .unwrap()
        .export()
        .unwrap();
    {
        let mut fb = gles.bind(&mut source_dmabuf).unwrap();
        let mut frame = gles
            .render(&mut fb, (8, 8).into(), Transform::Normal)
            .unwrap();
        frame
            .clear(
                [0., 0., 1., 1.].into(),
                &[Rectangle::from_size((8, 8).into())],
            )
            .unwrap();
        frame.finish().unwrap().wait().unwrap();
    }
    let mut native = NativeRenderer::new(vk).unwrap();
    // This fixture asserts pixels/lifetimes after bounded format preparation.
    native
        .prepare_output_format_for_test(Fourcc::Abgr8888)
        .unwrap();
    native
        .prepare_output_format_for_test(Fourcc::Argb8888)
        .unwrap();
    let external = native.import_dmabuf(&source_dmabuf, None).unwrap();
    let mut offscreen = native
        .create_buffer(Fourcc::Abgr8888, (8, 8).into())
        .unwrap();
    let src = Rectangle::from_size((8., 8.).into());
    let dst = Rectangle::from_size((8, 8).into());

    // An ordinary offscreen render has no KMS fence consumer. It must still
    // finish sampling the client allocation before a buffer release is safe.
    let mut fb = native.bind(&mut offscreen).unwrap();
    let mut frame = native
        .render(&mut fb, (8, 8).into(), Transform::Normal)
        .unwrap();
    frame
        .render_texture_from_to(&external, src, dst, &[dst], &[], Transform::Normal, 1.)
        .unwrap();
    let blocked = BlockedQueue::new(ctx);
    let release_started = Arc::new(AtomicBool::new(false));
    let release_flag = release_started.clone();
    let release = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(50));
        release_flag.store(true, Ordering::Release);
        drop(blocked);
    });
    let sync = frame.finish().unwrap();
    assert!(release_started.load(Ordering::Acquire));
    assert!(
        !sync.contains_fence(),
        "external source must finish synchronously"
    );
    release.join().unwrap();
    drop(fb);

    // The same branch applies with an external KMS swapchain target.
    let mut kms_target = allocator
        .create_buffer(8, 8, Fourcc::Argb8888, &mods)
        .unwrap()
        .export()
        .unwrap();
    let mut fb = native.bind(&mut kms_target).unwrap();
    let mut frame = native
        .render(&mut fb, (8, 8).into(), Transform::Normal)
        .unwrap();
    frame
        .render_texture_from_to(&external, src, dst, &[dst], &[], Transform::Normal, 1.)
        .unwrap();
    assert!(!frame.finish().unwrap().contains_fence());
    drop(fb);

    // Internal textures have no client wl_buffer to release; retain the native
    // async completion fence for the KMS deadline path.
    let mut fb = native.bind(&mut kms_target).unwrap();
    let mut frame = native
        .render(&mut fb, (8, 8).into(), Transform::Normal)
        .unwrap();
    frame
        .render_texture_from_to(&offscreen, src, dst, &[dst], &[], Transform::Normal, 1.)
        .unwrap();
    assert!(frame.finish().unwrap().contains_fence());
}

#[test]
#[ignore = "requires real DRM render node, Vulkan and GLES reference"]
fn native_config_notification_matches_gles() {
    use std::cell::RefCell;
    use std::path::Path;
    use std::rc::Rc;

    use smithay::backend::renderer::element::Element;
    use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};

    use crate::animation::Clock;
    use crate::render_helpers::render_to_vec;
    use crate::ui::config_error_notification::ConfigErrorNotification;

    let (vk, mut reference, _) = setup();
    let mut native = native::NativeRenderer::new(vk).unwrap();
    let mut clock = Clock::default();
    clock.set_complete_instantly(true);
    let mut notification = ConfigErrorNotification::new(
        clock,
        Rc::new(RefCell::new(clarion_config::Config::default())),
    );
    let output = Output::new(
        "notification-test".into(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "test".into(),
            model: "test".into(),
            serial_number: "test".into(),
        },
    );
    for scale in [1., 1.5, 2.] {
        output.change_current_state(
            Some(Mode {
                size: (1920, 300).into(),
                refresh: 60_000,
            }),
            Some(Transform::Normal),
            Some(Scale::Fractional(scale)),
            None,
        );
        for created in [true, false] {
            if created {
                notification.show_created(Path::new("/tmp/clarion/config.kdl"));
            } else {
                notification.show();
            }
            notification.advance_animations();
            let actual = notification.render_with(&mut native, &output).unwrap();
            let cached = notification.render_with(&mut native, &output).unwrap();
            assert_eq!(
                actual.id(),
                cached.id(),
                "notification should reuse uploaded texture"
            );
            let expected = notification.render_with(&mut reference, &output).unwrap();
            let pixels = render_to_vec(
                &mut native,
                (1920, 300).into(),
                scale.into(),
                Transform::Normal,
                Fourcc::Abgr8888,
                [&actual].into_iter(),
            )
            .unwrap();
            let reference_pixels = render_to_vec(
                &mut reference,
                (1920, 300).into(),
                scale.into(),
                Transform::Normal,
                Fourcc::Abgr8888,
                [&expected].into_iter(),
            )
            .unwrap();
            assert!(pixels
                .iter()
                .zip(&reference_pixels)
                .all(|(a, b)| a.abs_diff(*b) <= 2));
            assert!(
                pixels.as_chunks::<4>().0.iter().any(|p| p[3] != 0),
                "notification must be visible"
            );
        }
    }
    notification.hide();
    notification.advance_animations();
    assert!(notification.render_with(&mut native, &output).is_none());
    eprintln!(
        "PASS: native config notification text, colors, fractional scale, cache reuse and hide"
    );
}

#[test]
#[ignore = "requires real DRM render node, Vulkan and GLES reference"]
fn native_hotkeys_and_exit_dialog_match_gles() {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;

    use smithay::backend::renderer::element::Element;
    use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};

    use crate::animation::Clock;
    use crate::render_helpers::render_to_vec;
    use crate::ui::exit_confirm_dialog::ExitConfirmDialog;
    use crate::ui::hotkey_overlay::HotkeyOverlay;

    let (vk, mut reference, _) = setup();
    let mut native = native::NativeRenderer::new(vk).unwrap();
    let config = Rc::new(RefCell::new(clarion_config::Config::default()));
    let mut clock = Clock::with_time(Duration::ZERO);
    let mut dialog = ExitConfirmDialog::new(clock.clone(), config.clone());
    let mut hotkeys = HotkeyOverlay::new(config, clarion_config::ModKey::Super);
    let output = Output::new(
        "ui-test".into(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "test".into(),
            model: "test".into(),
            serial_number: "test".into(),
        },
    );
    assert!(hotkeys.show());
    assert!(dialog.show());
    // Keep both renderers at exactly the same intermediate animation time.
    clock.set_unadjusted(Duration::from_millis(80));
    dialog.advance_animations();
    for scale in [1., 1.5, 2.] {
        output.change_current_state(
            Some(Mode {
                size: (1600, 1000).into(),
                refresh: 60_000,
            }),
            Some(Transform::Normal),
            Some(Scale::Fractional(scale)),
            None,
        );
        let actual = hotkeys.render_with(&mut native, &output).unwrap();
        let cached = hotkeys.render_with(&mut native, &output).unwrap();
        assert_eq!(actual.id(), cached.id());
        let expected = hotkeys.render_with(&mut reference, &output).unwrap();
        let actual_pixels = render_to_vec(
            &mut native,
            (1600, 1000).into(),
            scale.into(),
            Transform::Normal,
            Fourcc::Abgr8888,
            [&actual].into_iter(),
        )
        .unwrap();
        let expected_pixels = render_to_vec(
            &mut reference,
            (1600, 1000).into(),
            scale.into(),
            Transform::Normal,
            Fourcc::Abgr8888,
            [&expected].into_iter(),
        )
        .unwrap();
        assert!(actual_pixels
            .iter()
            .zip(&expected_pixels)
            .all(|(a, b)| a.abs_diff(*b) <= 2));
        assert!(actual_pixels.as_chunks::<4>().0.iter().any(|p| p[3] > 100));
        hotkeys.on_hotkey_config_updated(clarion_config::ModKey::Alt);
        assert_ne!(
            actual.id(),
            hotkeys.render_with(&mut native, &output).unwrap().id()
        );

        let mut actual = Vec::new();
        dialog.render_with(&mut native, &output, &mut |e| actual.push(e));
        let mut cached = Vec::new();
        dialog.render_with(&mut native, &output, &mut |e| cached.push(e));
        assert_eq!(actual.len(), 2);
        assert_eq!(
            actual[0].id(),
            cached[0].id(),
            "static dialog texture must not upload each frame"
        );
        let mut expected = Vec::new();
        // Exercise the production GLES wrapper, including the primary-GPU element.
        dialog.render(&mut reference, &output, &mut |e| expected.push(e));
        let actual_pixels = render_to_vec(
            &mut native,
            (1600, 1000).into(),
            scale.into(),
            Transform::Normal,
            Fourcc::Abgr8888,
            actual.iter(),
        )
        .unwrap();
        let expected_pixels = render_to_vec(
            &mut reference,
            (1600, 1000).into(),
            scale.into(),
            Transform::Normal,
            Fourcc::Abgr8888,
            expected.iter(),
        )
        .unwrap();
        assert!(actual_pixels
            .iter()
            .zip(&expected_pixels)
            .all(|(a, b)| a.abs_diff(*b) <= 2));
        assert!(
            actual_pixels[3] > 0,
            "animated backdrop must remain visible"
        );
        assert!(actual_pixels
            .as_chunks::<4>()
            .0
            .iter()
            .any(|p| p[3] > actual_pixels[3]));
    }
    assert!(hotkeys.hide());
    assert!(hotkeys.render_with(&mut native, &output).is_none());
    assert!(dialog.hide());
    clock.set_complete_instantly(true);
    dialog.advance_animations();
    dialog.render_with(&mut native, &output, &mut |_| {
        panic!("hidden dialog emitted elements")
    });
    eprintln!(
        "PASS: native hotkeys and animated exit dialog, fractional scales, cache reuse, invalidation, backdrop and hiding"
    );
}

#[test]
#[ignore = "requires real DRM render node, Vulkan and GLES reference"]
fn native_border_element_gradients_crop_and_transforms() {
    use clarion_config::{
        Color, CornerRadius, GradientColorSpace, GradientInterpolation, HueInterpolation,
    };
    use smithay::backend::renderer::element::{Element, RenderElement};

    use crate::render_helpers::border::BorderRenderElement;
    use crate::render_helpers::{render_to_vec, resources, shaders};
    let (vk, mut reference, _) = setup();
    let mut native = native::NativeRenderer::new(vk).unwrap();
    resources::init(&mut reference);
    shaders::init(&mut reference);
    assert!(BorderRenderElement::has_shader(&mut reference));
    for color_space in [
        GradientColorSpace::Srgb,
        GradientColorSpace::SrgbLinear,
        GradientColorSpace::Oklab,
        GradientColorSpace::Oklch,
    ] {
        for hue_interpolation in [
            HueInterpolation::Shorter,
            HueInterpolation::Longer,
            HueInterpolation::Increasing,
            HueInterpolation::Decreasing,
        ] {
            let element = BorderRenderElement::new(
                (64., 40.).into(),
                Rectangle::new((-4., -3.).into(), (72., 48.).into()),
                GradientInterpolation {
                    color_space,
                    hue_interpolation,
                },
                Color::new_unpremul(0.8, 0.1, 0.2, 0.8),
                Color::new_unpremul(0.1, 0.7, 0.9, 0.5),
                -0.7,
                Rectangle::new((5., 3.).into(), (54., 34.).into()),
                3.,
                CornerRadius {
                    top_left: 5.,
                    top_right: 9.,
                    bottom_right: 7.,
                    bottom_left: 3.,
                },
                1.5,
                0.7,
            )
            .with_location((3., 2.).into());
            for transform in [
                Transform::Normal,
                Transform::_90,
                Transform::_180,
                Transform::_270,
                Transform::Flipped,
                Transform::Flipped90,
                Transform::Flipped180,
                Transform::Flipped270,
            ] {
                let actual = render_to_vec(
                    &mut native,
                    (106, 70).into(),
                    1.5.into(),
                    transform,
                    Fourcc::Abgr8888,
                    [&element].into_iter(),
                )
                .unwrap();
                let expected = render_to_vec(
                    &mut reference,
                    (106, 70).into(),
                    1.5.into(),
                    transform,
                    Fourcc::Abgr8888,
                    [&element].into_iter(),
                )
                .unwrap();
                let worst = actual
                    .iter()
                    .zip(&expected)
                    .map(|(a, b)| a.abs_diff(*b))
                    .max()
                    .unwrap();
                assert!(
                    worst <= 3,
                    "border mismatch {color_space:?} {hue_interpolation:?} {transform:?}: {worst}"
                );
                assert!(actual.as_chunks::<4>().0.iter().any(|p| p[3] > 0));
            }
            // Direct element crop with duplicate damage exercises normalized source coordinates
            // and prevents applying translucent border pixels twice.
            let mut texture = native
                .create_buffer(Fourcc::Abgr8888, (50, 30).into())
                .unwrap();
            let mut framebuffer = native.bind(&mut texture).unwrap();
            let mut frame = native
                .render(&mut framebuffer, (50, 30).into(), Transform::Normal)
                .unwrap();
            let full = Rectangle::from_size((50, 30).into());
            frame.clear(Color32F::TRANSPARENT, &[full]).unwrap();
            let src = Rectangle::new((0.15, 0.1).into(), (0.6, 0.7).into());
            <BorderRenderElement as RenderElement<native::NativeRenderer>>::draw(
                &element,
                &mut frame,
                src,
                full,
                &[full, full],
                &[],
                None,
            )
            .unwrap();
            frame.finish().unwrap().wait().unwrap();
            drop(framebuffer);
            let map = native
                .copy_texture(
                    &texture,
                    Rectangle::from_size((50, 30).into()),
                    Fourcc::Abgr8888,
                )
                .unwrap();
            let actual = native.map_texture(&map).unwrap().to_vec();
            let mut texture = reference
                .create_buffer(Fourcc::Abgr8888, (50, 30).into())
                .unwrap();
            let mut framebuffer = reference.bind(&mut texture).unwrap();
            let mut frame = reference
                .render(&mut framebuffer, (50, 30).into(), Transform::Normal)
                .unwrap();
            frame.clear(Color32F::TRANSPARENT, &[full]).unwrap();
            <BorderRenderElement as RenderElement<GlesRenderer>>::draw(
                &element,
                &mut frame,
                src,
                full,
                &[full],
                &[],
                None,
            )
            .unwrap();
            frame.finish().unwrap().wait().unwrap();
            drop(framebuffer);
            let map = reference
                .copy_texture(
                    &texture,
                    Rectangle::from_size((50, 30).into()),
                    Fourcc::Abgr8888,
                )
                .unwrap();
            let expected = reference.map_texture(&map).unwrap();
            assert!(
                actual
                    .iter()
                    .zip(expected)
                    .all(|(a, b)| a.abs_diff(*b) <= 3),
                "cropped border mismatch"
            );
            assert_eq!(element.src().size, (1., 1.).into());
        }
    }
    eprintln!(
        "PASS: native scene border, four color spaces/hue paths, asymmetric corners, crop, duplicate damage and all output transforms"
    );
}

#[test]
#[ignore = "requires real DRM render node, Vulkan and GLES reference"]
fn native_shadow_element_blur_cutout_and_transforms() {
    use clarion_config::{Color, CornerRadius};
    use smithay::backend::renderer::element::{Element, RenderElement};

    use crate::render_helpers::shadow::ShadowRenderElement;
    use crate::render_helpers::{render_to_vec, resources, shaders};
    let (vk, mut reference, _) = setup();
    let mut native = native::NativeRenderer::new(vk).unwrap();
    resources::init(&mut reference);
    shaders::init(&mut reference);
    assert!(ShadowRenderElement::has_shader(&mut reference));
    for sigma in [0., 0.05, 1.5, 5.] {
        for cutout in [false, true] {
            let window = if cutout {
                Rectangle::new((14., 10.).into(), (36., 23.).into())
            } else {
                Rectangle::default()
            };
            let element = ShadowRenderElement::new(
                (64., 40.).into(),
                Rectangle::new((8., 6.).into(), (46., 29.).into()),
                Color::new_unpremul(0.2, 0.4, 0.8, 0.65),
                sigma,
                CornerRadius {
                    top_left: 5.,
                    top_right: 9.,
                    bottom_right: 7.,
                    bottom_left: 3.,
                },
                1.5,
                window,
                CornerRadius::from(6.),
                0.7,
            )
            .with_location((3., 2.).into())
            .with_alpha(0.45);
            for transform in [
                Transform::Normal,
                Transform::_90,
                Transform::_180,
                Transform::_270,
                Transform::Flipped,
                Transform::Flipped90,
                Transform::Flipped180,
                Transform::Flipped270,
            ] {
                let actual = render_to_vec(
                    &mut native,
                    (106, 70).into(),
                    1.5.into(),
                    transform,
                    Fourcc::Abgr8888,
                    [&element].into_iter(),
                )
                .unwrap();
                let expected = render_to_vec(
                    &mut reference,
                    (106, 70).into(),
                    1.5.into(),
                    transform,
                    Fourcc::Abgr8888,
                    [&element].into_iter(),
                )
                .unwrap();
                let worst = actual
                    .iter()
                    .zip(&expected)
                    .map(|(a, b)| a.abs_diff(*b))
                    .max()
                    .unwrap();
                if worst > 3 {
                    let i = actual
                        .iter()
                        .zip(&expected)
                        .position(|(a, b)| a.abs_diff(*b) > 3)
                        .unwrap()
                        / 4;
                    eprintln!(
                        "first mismatch ({}, {}): {:?} vs {:?}",
                        i % 106,
                        i / 106,
                        &actual[i * 4..i * 4 + 4],
                        &expected[i * 4..i * 4 + 4]
                    );
                }
                assert!(
                    worst <= 3,
                    "shadow mismatch sigma={sigma} cutout={cutout} {transform:?}: {worst}"
                );
                assert!(actual.as_chunks::<4>().0.iter().any(|p| p[3] > 0));
            }
            // Direct element crop with duplicate damage exercises normalized source coordinates
            // and prevents applying translucent shadow pixels twice.
            let mut texture = native
                .create_buffer(Fourcc::Abgr8888, (50, 30).into())
                .unwrap();
            let mut framebuffer = native.bind(&mut texture).unwrap();
            let mut frame = native
                .render(&mut framebuffer, (50, 30).into(), Transform::Normal)
                .unwrap();
            let full = Rectangle::from_size((50, 30).into());
            frame.clear(Color32F::TRANSPARENT, &[full]).unwrap();
            let src = Rectangle::new((0.15, 0.1).into(), (0.6, 0.7).into());
            <ShadowRenderElement as RenderElement<native::NativeRenderer>>::draw(
                &element,
                &mut frame,
                src,
                full,
                &[full, full],
                &[],
                None,
            )
            .unwrap();
            frame.finish().unwrap().wait().unwrap();
            drop(framebuffer);
            let map = native
                .copy_texture(
                    &texture,
                    Rectangle::from_size((50, 30).into()),
                    Fourcc::Abgr8888,
                )
                .unwrap();
            let actual = native.map_texture(&map).unwrap().to_vec();
            let mut texture = reference
                .create_buffer(Fourcc::Abgr8888, (50, 30).into())
                .unwrap();
            let mut framebuffer = reference.bind(&mut texture).unwrap();
            let mut frame = reference
                .render(&mut framebuffer, (50, 30).into(), Transform::Normal)
                .unwrap();
            frame.clear(Color32F::TRANSPARENT, &[full]).unwrap();
            <ShadowRenderElement as RenderElement<GlesRenderer>>::draw(
                &element,
                &mut frame,
                src,
                full,
                &[full],
                &[],
                None,
            )
            .unwrap();
            frame.finish().unwrap().wait().unwrap();
            drop(framebuffer);
            let map = reference
                .copy_texture(
                    &texture,
                    Rectangle::from_size((50, 30).into()),
                    Fourcc::Abgr8888,
                )
                .unwrap();
            let expected = reference.map_texture(&map).unwrap();
            assert!(
                actual
                    .iter()
                    .zip(expected)
                    .all(|(a, b)| a.abs_diff(*b) <= 3),
                "cropped shadow mismatch"
            );
            assert_eq!(element.src().size, (1., 1.).into());
        }
    }
    eprintln!(
        "PASS: native scene shadows, zero/low/blurred sigma, window cutout, alpha override, crop, duplicate damage and output transforms"
    );
}

#[test]
#[ignore = "requires real DRM render node, Vulkan and GLES reference"]
fn native_layout_decorations_match_gles() {
    use std::time::Duration;

    use clarion_config::{Color, CornerRadius, Gradient, GradientRelativeTo};

    use crate::render_helpers::{render_to_vec, resources, shaders};
    use crate::scene::focus_ring::{FocusRing, FocusRingRenderElement};
    use crate::scene::shadow::Shadow;
    let (vk, mut reference, _) = setup();
    let mut native = native::NativeRenderer::new(vk).unwrap();
    resources::init(&mut reference);
    shaders::init(&mut reference);
    let mut gradient = Gradient::from(Color::new_unpremul(0.8, 0.2, 0.1, 0.8));
    gradient.to = Color::new_unpremul(0.1, 0.6, 0.8, 0.6);
    gradient.relative_to = GradientRelativeTo::WorkspaceView;
    gradient.rotate_deg_per_sec = Some(clarion_config::FloatOrInt(45.));
    let mut focus = FocusRing::new(clarion_config::FocusRing {
        active_gradient: Some(gradient),
        urgent_gradient: Some(gradient),
        ..Default::default()
    });
    let mut shadow = Shadow::new(clarion_config::Shadow {
        on: true,
        softness: 8.,
        spread: 2.,
        ..Default::default()
    });
    for scale in [1., 1.5, 2.] {
        for (active, urgent, border) in [
            (true, false, true),
            (false, true, false),
            (false, false, true),
        ] {
            let radius = if active || urgent {
                CornerRadius::from(8.)
            } else {
                CornerRadius::default()
            };
            focus.update_render_elements(
                (64., 40.).into(),
                active,
                border,
                urgent,
                Rectangle::new((-4., -8.).into(), (120., 90.).into()),
                radius,
                scale,
                0.7,
                Some(Duration::from_millis(600)),
            );
            let mut actual = Vec::new();
            focus.render(&mut native, (25., 25.).into(), &mut |e| actual.push(e));
            assert!(!actual.is_empty());
            if active || urgent {
                assert!(
                    actual
                        .iter()
                        .all(|e| matches!(e, FocusRingRenderElement::Gradient(_))),
                    "native layout must retain gradient effects"
                );
            } else {
                assert!(actual
                    .iter()
                    .all(|e| matches!(e, FocusRingRenderElement::SolidColor(_))));
            }
            let mut expected = Vec::new();
            focus.render(&mut reference, (25., 25.).into(), &mut |e| expected.push(e));
            let a = render_to_vec(
                &mut native,
                (240, 180).into(),
                scale.into(),
                Transform::Normal,
                Fourcc::Abgr8888,
                actual.iter(),
            )
            .unwrap();
            let b = render_to_vec(
                &mut reference,
                (240, 180).into(),
                scale.into(),
                Transform::Normal,
                Fourcc::Abgr8888,
                expected.iter(),
            )
            .unwrap();
            let worst = a.iter().zip(&b).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
            if worst > 3 {
                let i = a
                    .iter()
                    .zip(&b)
                    .position(|(a, b)| a.abs_diff(*b) > 3)
                    .unwrap()
                    / 4;
                eprintln!(
                    "focus scale={scale} active={active} urgent={urgent} border={border} worst={worst} at ({},{}): {:?} vs {:?}",
                    i % 240,
                    i / 240,
                    &a[i * 4..i * 4 + 4],
                    &b[i * 4..i * 4 + 4]
                );
            }
            assert!(
                a.iter().zip(&b).all(|(a, b)| a.abs_diff(*b) <= 3),
                "layout focus mismatch"
            );
            shadow.update_render_elements((64., 40.).into(), active, radius, scale, 0.7);
            let mut actual = Vec::new();
            shadow.render(&mut native, (25., 25.).into(), &mut |e| actual.push(e));
            let mut expected = Vec::new();
            shadow.render(&mut reference, (25., 25.).into(), &mut |e| expected.push(e));
            assert!(!actual.is_empty());
            let a = render_to_vec(
                &mut native,
                (240, 180).into(),
                scale.into(),
                Transform::Normal,
                Fourcc::Abgr8888,
                actual.iter(),
            )
            .unwrap();
            let b = render_to_vec(
                &mut reference,
                (240, 180).into(),
                scale.into(),
                Transform::Normal,
                Fourcc::Abgr8888,
                expected.iter(),
            )
            .unwrap();
            assert!(
                a.iter().zip(&b).all(|(a, b)| a.abs_diff(*b) <= 3),
                "layout shadow mismatch"
            );
        }
    }
    focus.update_config(clarion_config::FocusRing {
        off: true,
        ..Default::default()
    });
    shadow.update_config(clarion_config::Shadow::default());
    focus.render(&mut native, (0., 0.).into(), &mut |_| {
        panic!("disabled focus ring rendered")
    });
    shadow.render(&mut native, (0., 0.).into(), &mut |_| {
        panic!("disabled shadow rendered")
    });
    eprintln!(
        "PASS: native layout focus/border/shadow generation, animated workspace gradients, active/urgent/inactive styles and fractional scales"
    );
}

#[test]
#[ignore = "requires real DRM render node, Vulkan and GLES reference"]
fn native_tabs_and_insert_hints_match_gles() {
    use std::time::Duration;

    use clarion_config::{Color, CornerRadius, Gradient, GradientRelativeTo, TabIndicatorPosition};

    use crate::animation::Clock;
    use crate::render_helpers::{render_to_vec, resources, shaders};
    use crate::scene::insert_hint_element::InsertHintElement;
    use crate::scene::tab_indicator::{TabIndicator, TabInfo};
    let (vk, mut reference, _) = setup();
    let mut native = native::NativeRenderer::new(vk).unwrap();
    resources::init(&mut reference);
    shaders::init(&mut reference);
    let mut gradient = Gradient::from(Color::new_unpremul(0.8, 0.1, 0.2, 0.6));
    gradient.to = Color::new_unpremul(0.1, 0.7, 0.9, 0.4);
    gradient.angle = 130;
    gradient.relative_to = GradientRelativeTo::WorkspaceView;
    let area = Rectangle::new((0., 0.).into(), (80., 60.).into());
    let view = Rectangle::new((-12., -8.).into(), (110., 90.).into());
    let mut clock = Clock::with_time(Duration::ZERO);
    for scale in [1., 1.5, 2.] {
        for position in [
            TabIndicatorPosition::Left,
            TabIndicatorPosition::Right,
            TabIndicatorPosition::Top,
            TabIndicatorPosition::Bottom,
        ] {
            for gap in [0., 2.] {
                let config = clarion_config::TabIndicator {
                    position,
                    gaps_between_tabs: gap,
                    corner_radius: 3.,
                    width: 6.,
                    hide_when_single_tab: true,
                    ..Default::default()
                };
                let mut tabs = TabIndicator::new(config);
                clock.set_unadjusted(Duration::ZERO);
                tabs.start_open_animation(
                    clock.clone(),
                    clarion_config::Config::default()
                        .animations
                        .window_movement
                        .0,
                );
                clock.set_unadjusted(Duration::from_millis(100));
                tabs.advance_animations();
                for active in [true, false] {
                    tabs.update_render_elements(
                        true,
                        area,
                        view,
                        3,
                        (0..3).map(|_| TabInfo {
                            gradient,
                            geometry: area,
                        }),
                        active,
                        scale,
                    );
                    let mut actual = Vec::new();
                    tabs.render(&mut native, (20., 20.).into(), &mut |e| actual.push(e));
                    let mut expected = Vec::new();
                    tabs.render(&mut reference, (20., 20.).into(), &mut |e| expected.push(e));
                    assert_eq!(
                        actual.len(),
                        3,
                        "native tabs must retain all gradient elements"
                    );
                    let a = render_to_vec(
                        &mut native,
                        (240, 200).into(),
                        scale.into(),
                        Transform::Normal,
                        Fourcc::Abgr8888,
                        actual.iter(),
                    )
                    .unwrap();
                    let b = render_to_vec(
                        &mut reference,
                        (240, 200).into(),
                        scale.into(),
                        Transform::Normal,
                        Fourcc::Abgr8888,
                        expected.iter(),
                    )
                    .unwrap();
                    assert!(
                        a.iter().zip(&b).all(|(a, b)| a.abs_diff(*b) <= 3),
                        "tab mismatch scale={scale} position={position:?} gap={gap} active={active}"
                    );
                    assert!(a.as_chunks::<4>().0.iter().any(|p| p[3] > 0));
                }
                tabs.update_render_elements(
                    true,
                    area,
                    view,
                    1,
                    [TabInfo {
                        gradient,
                        geometry: area,
                    }]
                    .into_iter(),
                    true,
                    scale,
                );
                tabs.render(&mut native, (20., 20.).into(), &mut |_| {
                    panic!("single hidden tab rendered")
                });
            }
        }
        let mut hint = InsertHintElement::new(clarion_config::InsertHint {
            gradient: Some(gradient),
            ..Default::default()
        });
        hint.update_render_elements((80., 60.).into(), view, CornerRadius::from(8.), scale);
        let mut actual = Vec::new();
        hint.render(&mut native, (20., 20.).into(), &mut |e| actual.push(e));
        let mut expected = Vec::new();
        hint.render(&mut reference, (20., 20.).into(), &mut |e| expected.push(e));
        assert_eq!(actual.len(), 1);
        let a = render_to_vec(
            &mut native,
            (240, 200).into(),
            scale.into(),
            Transform::Normal,
            Fourcc::Abgr8888,
            actual.iter(),
        )
        .unwrap();
        let b = render_to_vec(
            &mut reference,
            (240, 200).into(),
            scale.into(),
            Transform::Normal,
            Fourcc::Abgr8888,
            expected.iter(),
        )
        .unwrap();
        assert!(a.iter().zip(&b).all(|(a, b)| a.abs_diff(*b) <= 3));
        hint.update_config(clarion_config::InsertHint {
            off: true,
            ..Default::default()
        });
        hint.render(&mut native, (20., 20.).into(), &mut |_| {
            panic!("disabled hint rendered")
        });
    }
    eprintln!(
        "PASS: native tab indicators on four sides, fractional scales, joined/separate corners, opening animation, inactive alpha and gradient insertion hints"
    );
}

#[test]
#[ignore = "requires real DRM render node, Vulkan and GLES reference"]
fn native_dual_kawase_blur_matches_gles_and_preserves_results() {
    use smithay::backend::renderer::{ImportMem, Texture};

    use crate::render_helpers::blur::{Blur, BlurOptions};
    use crate::render_helpers::{resources, shaders};
    let (vk, mut reference, _) = setup();
    let mut native = native::NativeRenderer::new(vk).unwrap();
    let mut blur = blur::NativeBlur::default();
    resources::init(&mut reference);
    shaders::init(&mut reference);
    let mut oracle = Blur::new(&mut reference).unwrap();
    let mut retained = None;
    let mut inline_blur = blur::NativeBlur::default();
    for (w, h) in [(37, 23), (8, 6), (1, 9)] {
        let mut data = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let a = ((x * 7 + y * 11) % 128 + 128) as u8;
                data.extend_from_slice(&[
                    a / 2,
                    if x % 3 == 0 { a } else { 0 },
                    if y % 2 == 0 { a / 3 } else { 0 },
                    a,
                ]);
            }
        }
        for flipped in [false, true] {
            let source = native
                .import_memory(&data, Fourcc::Abgr8888, (w, h).into(), flipped)
                .unwrap();
            let expected_data = if flipped {
                data.chunks_exact(w as usize * 4)
                    .rev()
                    .flatten()
                    .copied()
                    .collect::<Vec<_>>()
            } else {
                data.clone()
            };
            let expected_source = reference
                .import_memory(&expected_data, Fourcc::Abgr8888, (w, h).into(), false)
                .unwrap();
            for passes in [1, 3, 31] {
                for offset in [0., 1.25] {
                    let options = BlurOptions { passes, offset };
                    let (texture, done) = blur.render(&mut native, &source, options).unwrap();
                    done.wait().unwrap();
                    let region = Rectangle::from_size((w, h).into());
                    let mapping = native
                        .copy_texture(&texture, region, Fourcc::Abgr8888)
                        .unwrap();
                    let a = native.map_texture(&mapping).unwrap().to_vec();
                    oracle
                        .prepare_textures(
                            |format, size| reference.create_buffer(format, size),
                            &expected_source,
                            options,
                        )
                        .unwrap();
                    let expected = oracle
                        .render(&mut reference, &expected_source, options)
                        .unwrap();
                    let mapping = reference
                        .copy_texture(&expected, region, Fourcc::Abgr8888)
                        .unwrap();
                    let b = reference.map_texture(&mapping).unwrap();
                    let worst = a.iter().zip(b).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
                    assert!(
                        worst <= 3,
                        "native blur {w}x{h} passes={passes} offset={offset} flipped={flipped}: {worst}"
                    );
                    assert!(a.as_chunks::<4>().0.iter().all(|p| p[3] > 0));
                    // Capture and all blur passes execute before a single finish().
                    let mut scene = native
                        .create_buffer(Fourcc::Abgr8888, (w + 4, h + 4).into())
                        .unwrap();
                    let captured = native
                        .create_buffer(Fourcc::Abgr8888, (w, h).into())
                        .unwrap();
                    let prepared = inline_blur
                        .prepare_in_frame(&mut native, &captured, options)
                        .unwrap();
                    let mut fb = native.bind(&mut scene).unwrap();
                    let mut frame = native
                        .render(&mut fb, (w + 4, h + 4).into(), Transform::Normal)
                        .unwrap();
                    let full = Rectangle::from_size(frame.output_size());
                    let dst = Rectangle::new((2, 2).into(), (w, h).into());
                    let damage = [Rectangle::from_size((w, h).into())];
                    frame.clear(Color32F::TRANSPARENT, &[full]).unwrap();
                    frame
                        .render_texture_from_to(
                            &source,
                            region.to_f64(),
                            dst,
                            &damage,
                            &[],
                            Transform::Normal,
                            1.,
                        )
                        .unwrap();
                    frame.capture_into(&captured, (2, 2).into()).unwrap();
                    frame.clear(Color32F::TRANSPARENT, &[full]).unwrap();
                    let result = prepared.render(&mut frame).unwrap();
                    frame
                        .render_texture_from_to(
                            &result,
                            region.to_f64(),
                            dst,
                            &damage,
                            &[],
                            Transform::Normal,
                            1.,
                        )
                        .unwrap();
                    frame.finish().unwrap().wait().unwrap();
                    let mapping = native
                        .copy_texture(&result, region, Fourcc::Abgr8888)
                        .unwrap();
                    let inline = native.map_texture(&mapping).unwrap();
                    let worst = inline
                        .iter()
                        .zip(b)
                        .map(|(a, b)| a.abs_diff(*b))
                        .max()
                        .unwrap();
                    assert!(
                        worst <= 3,
                        "inline blur {w}x{h} passes={passes} offset={offset} flipped={flipped}: {worst}"
                    );
                    let mapping = native
                        .copy_framebuffer(
                            &fb,
                            Rectangle::from_size((w + 4, h + 4).into()),
                            Fourcc::Abgr8888,
                        )
                        .unwrap();
                    let composed = native.map_texture(&mapping).unwrap();
                    for y in 0..h + 4 {
                        for x in 0..w + 4 {
                            let pixel = &composed[((y * (w + 4) + x) * 4) as usize..][..4];
                            if x >= 2 && x < w + 2 && y >= 2 && y < h + 2 {
                                let expected = &b[(((y - 2) * w + x - 2) * 4) as usize..][..4];
                                assert!(
                                    pixel.iter().zip(expected).all(|(a, b)| a.abs_diff(*b) <= 3),
                                    "inline composition viewport mismatch"
                                );
                            } else {
                                assert_eq!(pixel, [0; 4], "blur changed surrounding scene");
                            }
                        }
                    }

                    if retained.is_none() {
                        retained = Some((texture, a));
                    }
                }
            }
            assert!(blur
                .render(
                    &mut native,
                    &source,
                    BlurOptions {
                        passes: 1,
                        offset: f64::NAN
                    }
                )
                .is_err());
            // Once callers release the previous output and fence, reuse its allocation.
            let options = BlurOptions {
                passes: 1,
                offset: 1.25,
            };
            let (first, ready) = blur.render(&mut native, &source, options).unwrap();
            ready.wait().unwrap();
            let image = first.image();
            drop(first);
            drop(ready);
            let (second, ready) = blur.render(&mut native, &source, options).unwrap();
            ready.wait().unwrap();
            assert_eq!(
                second.image(),
                image,
                "completed unshared blur output should be reused"
            );
        }
    }
    let (texture, expected) = retained.unwrap();
    let mapping = native
        .copy_texture(
            &texture,
            Rectangle::from_size(texture.size()),
            Fourcc::Abgr8888,
        )
        .unwrap();
    assert_eq!(native.map_texture(&mapping).unwrap(), expected);
    eprintln!(
        "PASS: native dual-Kawase blur, odd/one-pixel dimensions, 1/3/31 passes, transparent input and retained-output immutability"
    );
}

#[test]
#[ignore = "requires real DRM render node and Vulkan native fences"]
fn native_blur_rotates_outputs_without_overwriting_pending_or_retained_results() {
    use smithay::backend::renderer::sync::SyncPoint;
    use smithay::backend::renderer::{ImportMem, Texture};

    use crate::render_helpers::blur::BlurOptions;

    fn render(
        native: &mut native::NativeRenderer,
        blur: &mut blur::NativeBlur,
        source: &VulkanTexture,
        inline: bool,
    ) -> (VulkanTexture, SyncPoint) {
        let options = BlurOptions {
            passes: 3,
            offset: 1.25,
        };
        if !inline {
            return blur.render(native, source, options).unwrap();
        }
        let size = source.size();
        let mut scene = native.create_buffer(Fourcc::Abgr8888, size).unwrap();
        let mut target = native.bind(&mut scene).unwrap();
        let mut frame = native
            .render(&mut target, (size.w, size.h).into(), Transform::Normal)
            .unwrap();
        let job = blur
            .prepare_in_native_frame(&frame, source, options)
            .unwrap();
        let result = job.render(&mut frame).unwrap();
        let damage = Rectangle::from_size((size.w, size.h).into());
        frame.clear(Color32F::TRANSPARENT, &[damage]).unwrap();
        frame
            .render_texture_from_to(
                &result,
                Rectangle::from_size(size).to_f64(),
                damage,
                &[damage],
                &[],
                Transform::Normal,
                1.,
            )
            .unwrap();
        (result, frame.finish().unwrap())
    }

    fn read(native: &mut native::NativeRenderer, texture: &VulkanTexture) -> Vec<u8> {
        let mapping = native
            .copy_texture(
                texture,
                Rectangle::from_size(texture.size()),
                Fourcc::Abgr8888,
            )
            .unwrap();
        native.map_texture(&mapping).unwrap().to_vec()
    }

    for inline in [false, true] {
        let (vk, _reference, _allocator) = setup();
        let ctx = vk.ctx.as_ref().unwrap().clone();
        assert!(ctx.supports_sync_file_export);
        let mut native = native::NativeRenderer::new(vk).unwrap();
        let mut blur = blur::NativeBlur::default();
        let size = (37, 23);
        let sources = [[160, 0, 0, 192], [0, 128, 0, 192], [0, 0, 96, 192]].map(|color| {
            native
                .import_memory(
                    &color.repeat((size.0 * size.1) as usize),
                    Fourcc::Abgr8888,
                    size.into(),
                    false,
                )
                .unwrap()
        });

        // Warm the pipeline, image chain and descriptors before the workload;
        // first-use driver compilation must not drain the queued GPU prefix.
        let (warm, ready) = render(&mut native, &mut blur, &sources[0], inline);
        ready.wait().unwrap();
        drop(warm);

        // This complete GPU workload precedes both blur submissions. All
        // resolving signal operations are already submitted, making the blur
        // completion SYNC_FDs legal to export while GPU work is still pending.
        // An unresolved future host/timeline signal would violate 03254 here.
        let mut work = SubmittedGpuWork::new(ctx);
        let _producer_fd = work.submit();
        let (first, first_ready) = render(&mut native, &mut blur, &sources[0], inline);
        let (second, second_ready) = render(&mut native, &mut blur, &sources[1], inline);
        assert!(!first_ready.is_reached() && !second_ready.is_reached());
        let first_image = first.image();
        assert_ne!(first_image, second.image());
        // Retire the artificial 32 GiB prefix with its own longer test bound,
        // then ordinary renderer waits only cover the short blur submissions.
        work.wait();
        first_ready.wait().unwrap();
        second_ready.wait().unwrap();
        let first_pixels = read(&mut native, &first);
        let second_pixels = read(&mut native, &second);
        for (pixels, expected) in [
            (&first_pixels, [160u8, 0, 0, 192]),
            (&second_pixels, [0u8, 128, 0, 192]),
        ] {
            assert!(pixels
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| p.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 3)));
        }

        // The latest output remains owned by the caller. A completed older
        // output must rotate back in instead of allocating a third image.
        drop(first);
        let (third, ready) = render(&mut native, &mut blur, &sources[2], inline);
        ready.wait().unwrap();
        assert_eq!(
            third.image(),
            first_image,
            "completed older output must rotate back in, inline={inline}"
        );
        assert_eq!(read(&mut native, &second), second_pixels);
        let third_pixels = read(&mut native, &third);
        assert!(third_pixels.as_chunks::<4>().0.iter().all(|p| p
            .iter()
            .zip([0u8, 0, 96, 192])
            .all(|(a, b)| a.abs_diff(b) <= 3)));
        let mut retained = vec![(second, second_pixels), (third, third_pixels)];

        // Evicting cache references must preserve all caller-owned snapshots,
        // including ones that no longer fit the eight-image retention budget.
        for _ in 0..12 {
            let (texture, ready) = render(&mut native, &mut blur, &sources[0], inline);
            ready.wait().unwrap();
            let pixels = read(&mut native, &texture);
            retained.push((texture, pixels));
            let allocations = blur.retained_output_allocations();
            assert!(allocations.len() <= 8);
            assert!(allocations.iter().sum::<u64>() <= 64 * 1024 * 1024);
        }

        let resized_source = native
            .import_memory(
                &[80, 80, 0, 128].repeat(19 * 11),
                Fourcc::Abgr8888,
                (19, 11).into(),
                false,
            )
            .unwrap();
        let (resized, ready) = render(&mut native, &mut blur, &resized_source, inline);
        ready.wait().unwrap();
        assert_eq!(resized.size(), (19, 11).into());
        assert!(blur.retained_output_allocations().is_empty());
        let resized_pixels = read(&mut native, &resized);

        // A context change must discard the previous device's pool even when
        // dimensions match, without changing any retained old-device texture.
        let (vk, _reference, _allocator) = setup();
        let mut other = native::NativeRenderer::new(vk).unwrap();
        let other_source = other
            .import_memory(
                &[0, 80, 80, 128].repeat(19 * 11),
                Fourcc::Abgr8888,
                (19, 11).into(),
                false,
            )
            .unwrap();
        let (other_result, ready) = render(&mut other, &mut blur, &other_source, inline);
        ready.wait().unwrap();
        assert!(Arc::ptr_eq(
            &other_result.inner.ctx,
            other.vulkan().ctx.as_ref().unwrap()
        ));
        assert!(blur.retained_output_allocations().is_empty());
        assert_eq!(read(&mut native, &resized), resized_pixels);
        for (texture, expected) in retained {
            assert_eq!(read(&mut native, &texture), expected);
        }
    }
    eprintln!(
        "PASS: standalone/in-frame blur pending outputs stay immutable, completed A/B/C output rotation, bounded actual allocation retention, size/context invalidation and retained snapshot pixels"
    );
}

#[test]
#[ignore = "requires real DRM render node, EGL reference and Vulkan"]
fn native_ordered_framebuffer_capture_and_resampling() {
    use smithay::backend::renderer::Texture;
    let (vk, mut reference, _) = setup();
    let mut native = native::NativeRenderer::new(vk).unwrap();
    let mut target = native
        .create_buffer(Fourcc::Abgr8888, (16, 12).into())
        .unwrap();
    let mut oracle: smithay::backend::renderer::gles::GlesTexture = reference
        .create_buffer(Fourcc::Abgr8888, (16, 12).into())
        .unwrap();
    let captured = native
        .create_buffer(Fourcc::Abgr8888, (7, 5).into())
        .unwrap();
    let final_capture = native
        .create_buffer(Fourcc::Abgr8888, (16, 12).into())
        .unwrap();
    let wrong_format = native
        .create_buffer(Fourcc::Argb8888, (7, 5).into())
        .unwrap();
    let wrong_alpha = native
        .create_buffer(Fourcc::Xbgr8888, (7, 5).into())
        .unwrap();
    for transform in [
        Transform::Normal,
        Transform::_90,
        Transform::_180,
        Transform::_270,
        Transform::Flipped,
        Transform::Flipped90,
        Transform::Flipped180,
        Transform::Flipped270,
    ] {
        // Reuse capture images across submissions, including images previously sampled.
        for _ in 0..2 {
            let mut fb = reference.bind(&mut oracle).unwrap();
            let mut frame = reference
                .render(&mut fb, (16, 12).into(), transform)
                .unwrap();
            let full = Rectangle::from_size(frame.output_size());
            frame
                .clear(Color32F::new(0.5, 0., 0., 0.5), &[full])
                .unwrap();
            frame
                .draw_solid(
                    Rectangle::new((3, 2).into(), (6, 5).into()),
                    &[Rectangle::from_size((6, 5).into())],
                    Color32F::new(0., 1., 0., 1.),
                )
                .unwrap();
            frame.finish().unwrap().wait().unwrap();
            let map = reference
                .copy_framebuffer(
                    &fb,
                    Rectangle::new((2, 3).into(), (7, 5).into()),
                    Fourcc::Abgr8888,
                )
                .unwrap();
            let expected = reference.map_texture(&map).unwrap().to_vec();
            drop(fb);
            let mut fb = native.bind(&mut target).unwrap();
            let mut frame = native.render(&mut fb, (16, 12).into(), transform).unwrap();
            let full = Rectangle::from_size(frame.output_size());
            frame
                .clear(Color32F::new(0.5, 0., 0., 0.5), &[full])
                .unwrap();
            frame
                .draw_solid(
                    Rectangle::new((3, 2).into(), (6, 5).into()),
                    &[Rectangle::from_size((6, 5).into())],
                    Color32F::new(0., 1., 0., 1.),
                )
                .unwrap();
            assert!(frame.capture_into(&captured, (-1, 0).into()).is_err());
            assert!(frame.capture_into(&captured, (10, 0).into()).is_err());
            assert!(frame.capture_into(&wrong_format, (0, 0).into()).is_err());
            assert!(frame.capture_into(&wrong_alpha, (0, 0).into()).is_err());
            frame.capture_into(&captured, (2, 3).into()).unwrap();
            assert!(frame.capture_into(&captured, (2, 3).into()).is_err());
            frame.clear(Color32F::TRANSPARENT, &[full]).unwrap();
            // Sample the newly captured image in this very command buffer.
            frame
                .render_texture_from_to(
                    &captured,
                    Rectangle::from_size(captured.size()).to_f64(),
                    Rectangle::new((1, 1).into(), (7, 5).into()),
                    &[Rectangle::from_size((7, 5).into())],
                    &[],
                    Transform::Normal,
                    1.,
                )
                .unwrap();
            frame.capture_into(&final_capture, (0, 0).into()).unwrap();
            frame.clear(Color32F::new(0., 0., 1., 1.), &[full]).unwrap();
            frame.finish().unwrap().wait().unwrap();
            let map = native
                .copy_texture(
                    &captured,
                    Rectangle::from_size(captured.size()),
                    Fourcc::Abgr8888,
                )
                .unwrap();
            let actual = native.map_texture(&map).unwrap();
            assert!(
                actual
                    .iter()
                    .zip(&expected)
                    .all(|(a, b)| a.abs_diff(*b) <= 2),
                "capture mismatch {transform:?}"
            );
            // The final capture must preserve the sampled image before the blue clear.
            let map = native
                .copy_texture(
                    &final_capture,
                    Rectangle::from_size(final_capture.size()),
                    Fourcc::Abgr8888,
                )
                .unwrap();
            let actual = native.map_texture(&map).unwrap();
            let nonzero = actual
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|p| p[3] != 0)
                .count();
            assert_eq!(
                nonzero, 35,
                "capture must observe earlier sampling {transform:?}"
            );
            // Independent reference for sampling and transform, including transparent surroundings.
            let input = {
                use smithay::backend::renderer::ImportMem;
                reference
                    .import_memory(&expected, Fourcc::Abgr8888, (7, 5).into(), false)
                    .unwrap()
            };
            let mut fb = reference.bind(&mut oracle).unwrap();
            let mut frame = reference
                .render(&mut fb, (16, 12).into(), transform)
                .unwrap();
            frame.clear(Color32F::TRANSPARENT, &[full]).unwrap();
            Frame::render_texture_from_to(
                &mut frame,
                &input,
                Rectangle::from_size(input.size()).to_f64(),
                Rectangle::new((1, 1).into(), (7, 5).into()),
                &[Rectangle::from_size((7, 5).into())],
                &[],
                Transform::Normal,
                1.,
            )
            .unwrap();
            frame.finish().unwrap().wait().unwrap();
            let map = reference
                .copy_framebuffer(&fb, Rectangle::from_size((16, 12).into()), Fourcc::Abgr8888)
                .unwrap();
            assert!(
                actual
                    .iter()
                    .zip(reference.map_texture(&map).unwrap())
                    .all(|(a, b)| a.abs_diff(*b) <= 2),
                "sampled capture mismatch {transform:?}"
            );
        }
    }
}

#[test]
#[ignore = "requires real DRM render node, EGL reference and Vulkan"]
fn native_scaled_capture_matches_gles_blit() {
    use smithay::backend::renderer::{ImportMem, Texture};
    let (vk, mut reference, mut allocator) = setup();
    let mods = modifiers(&vk, &reference, Fourcc::Argb8888);
    let mut native = native::NativeRenderer::new(vk).unwrap();
    let data = (0..17 * 13)
        .flat_map(|i| {
            let a = (128 + (i * 13) % 128) as u8;
            [a / 3, if i % 3 == 0 { a } else { 0 }, a / 2, a]
        })
        .collect::<Vec<_>>();
    let source = native
        .import_memory(&data, Fourcc::Abgr8888, (17, 13).into(), false)
        .unwrap();
    let expected_source = reference
        .import_memory(&data, Fourcc::Abgr8888, (17, 13).into(), false)
        .unwrap();
    let mut scene = native
        .create_buffer(Fourcc::Argb8888, (17, 13).into())
        .unwrap();
    let mut oracle: smithay::backend::renderer::gles::GlesTexture = reference
        .create_buffer(Fourcc::Abgr8888, (17, 13).into())
        .unwrap();
    let mut dma = allocator
        .create_buffer(17, 13, Fourcc::Argb8888, &mods)
        .unwrap()
        .export()
        .unwrap();
    for external in [false, true] {
        for size in [(11, 3), (3, 9), (1, 1), (7, 5)] {
            for format in [Fourcc::Abgr8888, Fourcc::Argb8888] {
                let captured = native.create_buffer(format, size.into()).unwrap();
                let mut expected: smithay::backend::renderer::gles::GlesTexture = reference
                    .create_buffer(Fourcc::Abgr8888, size.into())
                    .unwrap();
                let region = Rectangle::new((2, 3).into(), (7, 5).into());
                let mut fb = reference.bind(&mut oracle).unwrap();
                let mut frame = reference
                    .render(&mut fb, (17, 13).into(), Transform::Normal)
                    .unwrap();
                let full = Rectangle::from_size(frame.output_size());
                frame.clear(Color32F::TRANSPARENT, &[full]).unwrap();
                Frame::render_texture_from_to(
                    &mut frame,
                    &expected_source,
                    Rectangle::from_size(expected_source.size()).to_f64(),
                    full,
                    &[full],
                    &[],
                    Transform::Normal,
                    1.,
                )
                .unwrap();
                frame.finish().unwrap().wait().unwrap();
                // Crop to a separate image before filtering. GLES drivers differ in whether
                // linear framebuffer blits sample outside their source rectangle.
                let mut cropped: smithay::backend::renderer::gles::GlesTexture = reference
                    .create_buffer(Fourcc::Abgr8888, (7, 5).into())
                    .unwrap();
                let mut crop_fb = reference.bind(&mut cropped).unwrap();
                reference
                    .blit(
                        &fb,
                        &mut crop_fb,
                        region,
                        Rectangle::from_size((7, 5).into()),
                        TextureFilter::Nearest,
                    )
                    .unwrap()
                    .wait()
                    .unwrap();
                let mut out = reference.bind(&mut expected).unwrap();
                reference
                    .blit(
                        &crop_fb,
                        &mut out,
                        Rectangle::from_size((7, 5).into()),
                        Rectangle::from_size(size.into()),
                        TextureFilter::Linear,
                    )
                    .unwrap()
                    .wait()
                    .unwrap();
                let mapping = reference
                    .copy_framebuffer(&out, Rectangle::from_size(size.into()), Fourcc::Abgr8888)
                    .unwrap();
                let expected_pixels = reference.map_texture(&mapping).unwrap().to_vec();
                let mut fb = if external {
                    native.bind(&mut dma).unwrap()
                } else {
                    native.bind(&mut scene).unwrap()
                };
                let mut frame = native
                    .render(&mut fb, (17, 13).into(), Transform::Normal)
                    .unwrap();
                frame.clear(Color32F::TRANSPARENT, &[full]).unwrap();
                frame
                    .render_texture_from_to(
                        &source,
                        Rectangle::from_size(source.size()).to_f64(),
                        full,
                        &[full],
                        &[],
                        Transform::Normal,
                        1.,
                    )
                    .unwrap();
                assert!(frame
                    .capture_scaled_into(&captured, Rectangle::new((0, 0).into(), (0, 1).into()))
                    .is_err());
                assert!(frame
                    .capture_scaled_into(&captured, Rectangle::new((16, 0).into(), (2, 1).into()))
                    .is_err());
                frame
                    .capture_scaled_into(&captured, Rectangle::new((2, 3).into(), (7, 5).into()))
                    .unwrap();
                frame.clear(Color32F::new(0., 0., 1., 1.), &[full]).unwrap();
                frame.finish().unwrap().wait().unwrap();
                let mapping = native
                    .copy_texture(
                        &captured,
                        Rectangle::from_size(size.into()),
                        Fourcc::Abgr8888,
                    )
                    .unwrap();
                let actual = native.map_texture(&mapping).unwrap();
                let worst = actual
                    .iter()
                    .zip(&expected_pixels)
                    .map(|(a, b)| a.abs_diff(*b))
                    .max()
                    .unwrap();
                assert!(
                    worst <= 3,
                    "scaled capture external={external} size={size:?} format={format:?}: {worst}"
                );
            }
        }
    }
}

#[test]
#[ignore = "requires real DRM render node, Vulkan and GLES reference"]
fn native_framebuffer_effect_blur_noise_saturation_and_opaque_capture() {
    use clarion_config::CornerRadius;
    use smithay::backend::renderer::element::{Element, RenderElement};
    use smithay::backend::renderer::{Frame, ImportMem, Texture};
    use smithay::utils::user_data::UserDataMap;

    use crate::render_helpers::background_effect::RenderParams;
    use crate::render_helpers::blur::BlurOptions;
    use crate::render_helpers::framebuffer_effect::{
        FramebufferEffect, FramebufferEffectElement, NativeInner,
    };
    use crate::render_helpers::{resources, shaders};

    let (vk, mut reference, mut allocator) = setup();
    resources::init(&mut reference);
    shaders::init(&mut reference);

    let xrgb_mods = modifiers(&vk, &reference, Fourcc::Xrgb8888);
    let argb_mods = modifiers(&vk, &reference, Fourcc::Argb8888);
    let mut native = native::NativeRenderer::new(vk).unwrap();
    // This fixture exercises prepared rendering; wait on the bounded test-only
    // preparer before its pixel, timing or ownership assertions begin.
    native
        .prepare_output_format_for_test(Fourcc::Xrgb8888)
        .unwrap();
    native
        .prepare_output_format_for_test(Fourcc::Argb8888)
        .unwrap();

    let (w, h) = (64, 48);
    let full = Rectangle::from_size((w as i32, h as i32).into());
    let src_rect = Rectangle::from_size((w as f64, h as f64).into());
    let mut pattern_data = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            let r = ((x * 19 + y * 7) % 256) as u8;
            let g = ((x * 13 + y * 29) % 256) as u8;
            let b = ((x * 37 + y * 11) % 256) as u8;
            pattern_data.extend_from_slice(&[r, g, b, 255]);
        }
    }
    let native_bg = native
        .import_memory(
            &pattern_data,
            Fourcc::Abgr8888,
            (w as i32, h as i32).into(),
            false,
        )
        .unwrap();
    let ref_bg = reference
        .import_memory(
            &pattern_data,
            Fourcc::Abgr8888,
            (w as i32, h as i32).into(),
            false,
        )
        .unwrap();

    for (format, mods) in [
        (Fourcc::Xrgb8888, &xrgb_mods),
        (Fourcc::Argb8888, &argb_mods),
    ] {
        for blur in [true, false] {
            let effect = FramebufferEffect::new();
            let params = RenderParams {
                geometry: Rectangle::new((8., 8.).into(), (48., 32.).into()),
                subregion: None,
                clip: Some((
                    Rectangle::new((12., 10.).into(), (40., 28.).into()),
                    CornerRadius {
                        top_left: 6.,
                        top_right: 8.,
                        bottom_right: 4.,
                        bottom_left: 7.,
                    },
                )),
                scale: 1.0,
            };

            let blur_options = if blur {
                Some(BlurOptions {
                    passes: 2,
                    offset: 1.25,
                })
            } else {
                None
            };
            let noise = if blur { 0.15 } else { 0.0 };
            let saturation = 1.25;

            let element = effect.render(None, params, blur_options, noise, saturation);

            let mut target_dma = allocator
                .create_buffer(w as u32, h as u32, format, mods)
                .unwrap()
                .export()
                .unwrap();

            let native_cache = UserDataMap::new();
            let src = element.src();
            let dst = element.geometry((1., 1.).into());
            let damage = [Rectangle::from_size(dst.size)];

            {
                let mut fb = native.bind(&mut target_dma).unwrap();
                let mut frame = native
                    .render(&mut fb, (w as i32, h as i32).into(), Transform::Normal)
                    .unwrap();
                frame.clear(Color32F::BLACK, &[full]).unwrap();
                frame
                    .render_texture_from_to(
                        &native_bg,
                        src_rect,
                        full,
                        &[full],
                        &[],
                        Transform::Normal,
                        1.,
                    )
                    .unwrap();
                <FramebufferEffectElement as RenderElement<native::NativeRenderer>>::capture_framebuffer(
                    &element,
                    &mut frame,
                    src,
                    dst,
                    &native_cache,
                )
                .unwrap();
                <FramebufferEffectElement as RenderElement<native::NativeRenderer>>::draw(
                    &element,
                    &mut frame,
                    src,
                    dst,
                    &damage,
                    &[],
                    Some(&native_cache),
                )
                .unwrap();
                frame.finish().unwrap().wait().unwrap();
            }

            // Verify opaque capture alpha conversion:
            if format == Fourcc::Xrgb8888 {
                let inner = native_cache
                    .get::<std::cell::RefCell<NativeInner>>()
                    .unwrap()
                    .borrow();
                let intermediate = inner.intermediate.as_ref().unwrap();
                let mapping = native
                    .copy_texture(
                        intermediate,
                        Rectangle::from_size(intermediate.size()),
                        Fourcc::Abgr8888,
                    )
                    .unwrap();
                let captured_pixels = native.map_texture(&mapping).unwrap();
                assert!(
                    captured_pixels.as_chunks::<4>().0.iter().all(|p| p[3] == 255),
                    "captured intermediate buffer from Fourcc::Xrgb8888 must force alpha = 255 (blur={blur})"
                );
            }

            let mut ref_dma = allocator
                .create_buffer(w as u32, h as u32, format, mods)
                .unwrap()
                .export()
                .unwrap();
            let ref_cache = UserDataMap::new();

            {
                let mut fb = reference.bind(&mut ref_dma).unwrap();
                let mut frame = reference
                    .render(&mut fb, (w as i32, h as i32).into(), Transform::Normal)
                    .unwrap();
                frame.clear(Color32F::BLACK, &[full]).unwrap();
                Frame::render_texture_from_to(
                    &mut frame,
                    &ref_bg,
                    src_rect,
                    full,
                    &[full],
                    &[],
                    Transform::Normal,
                    1.,
                )
                .unwrap();
                <FramebufferEffectElement as RenderElement<GlesRenderer>>::capture_framebuffer(
                    &element, &mut frame, src, dst, &ref_cache,
                )
                .unwrap();
                <FramebufferEffectElement as RenderElement<GlesRenderer>>::draw(
                    &element,
                    &mut frame,
                    src,
                    dst,
                    &damage,
                    &[],
                    Some(&ref_cache),
                )
                .unwrap();
                frame.finish().unwrap().wait().unwrap();
            }

            let actual = pixels(&mut reference, &mut target_dma);
            let expected = pixels(&mut reference, &mut ref_dma);
            let worst = actual
                .iter()
                .zip(&expected)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap();
            assert!(
                worst <= 3,
                "framebuffer effect mismatch for format {format:?} blur={blur}: worst diff {worst}"
            );
        }
    }
}

#[test]
#[ignore = "requires real DRM render node and Vulkan"]
fn native_noise_keeps_transparent_pixels_premultiplied() {
    use smithay::backend::renderer::{
        Bind, Color32F, ExportMem, Frame, ImportMem, Offscreen, Renderer,
    };

    use super::native::{NativeRenderer, Postprocess};

    let (vk, _gles, _allocator) = setup();
    let mut renderer = NativeRenderer::new(vk).unwrap();
    let size: smithay::utils::Size<i32, smithay::utils::Buffer> = (16, 16).into();
    let physical_size: smithay::utils::Size<i32, smithay::utils::Physical> = (16, 16).into();
    let rect = Rectangle::from_size(size);
    let physical = Rectangle::from_size(physical_size);
    let source = renderer
        .import_memory(&[0; 16 * 16 * 4], Fourcc::Abgr8888, size, false)
        .unwrap();
    let mut target = renderer.create_buffer(Fourcc::Abgr8888, size).unwrap();
    {
        let mut fb = renderer.bind(&mut target).unwrap();
        let mut frame = renderer
            .render(&mut fb, physical_size, Transform::Normal)
            .unwrap();
        frame.clear(Color32F::TRANSPARENT, &[physical]).unwrap();
        frame.replace_postprocess(Postprocess {
            noise: 0.4,
            ..Default::default()
        });
        frame
            .render_texture_from_to(
                &source,
                Rectangle::from_size((16., 16.).into()),
                physical,
                &[physical],
                &[],
                Transform::Normal,
                1.,
            )
            .unwrap();
        frame.finish().unwrap().wait().unwrap();
    }
    let mapped = renderer
        .copy_texture(&target, rect, Fourcc::Abgr8888)
        .unwrap();
    assert!(
        renderer
            .map_texture(&mapped)
            .unwrap()
            .iter()
            .all(|&component| component == 0),
        "noise on a transparent snapshot must not add visible RGB"
    );
}

#[test]
#[ignore = "requires real DRM render node, Vulkan"]
fn native_scene_full_desktop_migration() {
    use smithay::backend::allocator::dmabuf::Dmabuf;
    use smithay::backend::renderer::damage::OutputDamageTracker;
    use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
    use smithay::backend::renderer::element::utils::{Relocate, RelocateRenderElement};
    use smithay::backend::renderer::element::{Kind, RenderElementStates};
    use smithay::backend::renderer::{Bind, ExportMem, ImportDma};
    use smithay::reexports::gbm::Format as Fourcc;
    use smithay::utils::{Point, Rectangle, Transform};

    use crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement;
    use crate::render_helpers::render_to_dmabuf;
    use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
    use crate::render_helpers::texture::{TextureBuffer, TextureRenderElement};
    use crate::render_helpers::vulkan::native;
    use crate::scene::OutputRenderElements;

    let (vk, reference, mut allocator) = setup();
    // Drop GLES reference to prove 100% GLES absence
    drop(reference);

    let mut native = native::NativeRenderer::new(vk).unwrap();
    // This fixture exercises prepared rendering; wait on the bounded test-only
    // preparer before its pixel, timing or ownership assertions begin.
    native
        .prepare_output_format_for_test(Fourcc::Argb8888)
        .unwrap();
    let mods: Vec<_> = native
        .dmabuf_formats()
        .iter()
        .filter(|f| f.code == Fourcc::Argb8888)
        .map(|f| f.modifier)
        .collect();
    assert!(
        !mods.is_empty(),
        "NativeRenderer must support ARGB8888 DMA-BUF"
    );

    let w = 800;
    let h = 600;
    let mut swapchain_dma: Dmabuf = allocator
        .create_buffer(w, h, Fourcc::Argb8888, &mods)
        .unwrap()
        .export()
        .unwrap();

    let mut damage_tracker = OutputDamageTracker::new((w as i32, h as i32), 1.0, Transform::Normal);

    // 1. Desktop background: solid color element (deep blue)
    let bg_buffer = SolidColorBuffer::new((w as f64, h as f64), [0.05, 0.1, 0.2, 1.0]);
    let bg_elem: OutputRenderElements<native::NativeRenderer> =
        OutputRenderElements::SolidColor(SolidColorRenderElement::from_buffer(
            &bg_buffer,
            Point::from((0., 0.)),
            1.0,
            Kind::Unspecified,
        ));

    // 2. Native Vulkan Window surface texture
    let window_w = 400;
    let window_h = 300;
    let window_pixels = vec![200u8; (window_w * window_h * 4) as usize];
    let window_texture = TextureBuffer::from_memory(
        &mut native,
        &window_pixels,
        Fourcc::Argb8888,
        (window_w, window_h),
        false,
        1.0,
        Transform::Normal,
        Vec::new(),
    )
    .unwrap();
    let window_render_elem = TextureRenderElement::from_texture_buffer(
        window_texture,
        Point::from((50., 50.)),
        1.0,
        None,
        None,
        Kind::Unspecified,
    );
    let window_elem: OutputRenderElements<native::NativeRenderer> =
        OutputRenderElements::Texture(PrimaryGpuTextureRenderElement::from(window_render_elem));

    // 3. Border element around the window
    let border_buffer = SolidColorBuffer::new(
        (window_w as f64 + 4.0, window_h as f64 + 4.0),
        [0.8, 0.4, 0.1, 1.0],
    );
    let border_elem: OutputRenderElements<native::NativeRenderer> =
        OutputRenderElements::SolidColor(SolidColorRenderElement::from_buffer(
            &border_buffer,
            Point::from((48., 48.)),
            1.0,
            Kind::Unspecified,
        ));

    // 4. Cursor element (Software / Memory buffer)
    let cursor_pixels = vec![255u8; 16 * 16 * 4];
    let cursor_mem = MemoryRenderBuffer::from_slice(
        &cursor_pixels,
        Fourcc::Argb8888,
        (16, 16),
        1,
        Transform::Normal,
        None,
    );
    let cursor_elem: OutputRenderElements<native::NativeRenderer> =
        OutputRenderElements::RelocatedMemoryBuffer(
            RelocateRenderElement::from_element(
                smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement::from_buffer(
                    &mut native,
                    Point::from((200., 200.)),
                    &cursor_mem,
                    None,
                    None,
                    None,
                    Kind::Unspecified,
                )
                .unwrap(),
                Point::from((200, 200)),
                Relocate::Relative,
            ),
        );

    // 5. Native UI dialog element
    let dialog_w = 300;
    let dialog_h = 100;
    let dialog_pixels = vec![180u8; (dialog_w * dialog_h * 4) as usize];
    let dialog_texture = TextureBuffer::from_memory(
        &mut native,
        &dialog_pixels,
        Fourcc::Argb8888,
        (dialog_w, dialog_h),
        false,
        1.0,
        Transform::Normal,
        Vec::new(),
    )
    .unwrap();
    let dialog_render_elem = TextureRenderElement::from_texture_buffer(
        dialog_texture,
        Point::from((250., 250.)),
        1.0,
        None,
        None,
        Kind::Unspecified,
    );
    let dialog_elem: OutputRenderElements<native::NativeRenderer> =
        OutputRenderElements::Texture(PrimaryGpuTextureRenderElement::from(dialog_render_elem));

    // Construct the complete desktop scene tree
    let scene: Vec<OutputRenderElements<native::NativeRenderer>> =
        vec![cursor_elem, dialog_elem, window_elem, border_elem, bg_elem];

    // Render directly to DMA-BUF via damage tracker and NativeRenderer
    let sync = render_to_dmabuf(
        &mut native,
        &mut damage_tracker,
        swapchain_dma.clone(),
        &scene,
        RenderElementStates::default(),
    )
    .expect("full desktop scene must render natively to swapchain DMA-BUF");
    sync.wait()
        .expect("Vulkan render completion sync point wait failed");

    // Read back rendered pixels purely via native Vulkan readback
    let fb = native.bind(&mut swapchain_dma).unwrap();
    let mapping = native
        .copy_framebuffer(
            &fb,
            Rectangle::from_size((w as i32, h as i32).into()),
            Fourcc::Argb8888,
        )
        .expect("native Vulkan framebuffer readback must succeed");
    let pixels = native.map_texture(&mapping).unwrap().to_vec();

    // Verify non-empty, non-black desktop content rendered
    assert_eq!(pixels.len(), (w * h * 4) as usize);
    let non_zero_count = pixels.iter().filter(|&&b| b > 0).count();
    assert!(
        non_zero_count > (w * h * 2) as usize,
        "desktop scene should contain rendered elements, found only {non_zero_count} non-zero bytes"
    );

    eprintln!(
        "Test 41: Native full desktop scene successfully rendered {non_zero_count} pixels on pure Vulkan without GLES!"
    );
}

#[test]
#[ignore = "requires real DRM render node, KMS sync export and Vulkan"]
fn native_dmabuf_sync_export_and_renderer_recreation() {
    use std::os::fd::AsRawFd;

    use smithay::backend::allocator::dmabuf::Dmabuf;
    use smithay::backend::renderer::damage::OutputDamageTracker;
    use smithay::backend::renderer::element::{Kind, RenderElementStates};
    use smithay::backend::renderer::sync::SyncPoint;
    use smithay::backend::renderer::{Bind, ExportMem, ImportDma};
    use smithay::reexports::gbm::Format as Fourcc;
    use smithay::utils::{Point, Rectangle, Transform};

    use super::native;
    use crate::render_helpers::render_to_dmabuf;
    use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
    use crate::scene::OutputRenderElements;

    let (vk, reference, mut allocator) = setup();
    drop(reference);

    let mut native = native::NativeRenderer::new(vk).unwrap();
    // This fixture checks exported fences and renderer recreation after normal
    // device preparation, rather than cold target admission. Wait on the real
    // worker for both this BGRA target and the RGBA intermediate before drawing.
    native
        .prepare_output_format_for_test(Fourcc::Argb8888)
        .unwrap();
    let mods: Vec<_> = native
        .dmabuf_formats()
        .iter()
        .filter(|f| f.code == Fourcc::Argb8888)
        .map(|f| f.modifier)
        .collect();
    assert!(!mods.is_empty());

    let w = 640;
    let h = 480;
    let swapchain_dma: Dmabuf = allocator
        .create_buffer(w, h, Fourcc::Argb8888, &mods)
        .unwrap()
        .export()
        .unwrap();

    let mut damage_tracker = OutputDamageTracker::new((w as i32, h as i32), 1.0, Transform::Normal);

    let color_buffer = SolidColorBuffer::new((w as f64, h as f64), [0.2, 0.4, 0.6, 1.0]);
    let elem: OutputRenderElements<native::NativeRenderer> =
        OutputRenderElements::SolidColor(SolidColorRenderElement::from_buffer(
            &color_buffer,
            Point::from((0., 0.)),
            1.0,
            Kind::Unspecified,
        ));
    let scene = vec![elem];

    // 1. Render and verify async sync file export for KMS IN_FENCE_FD
    let sync: SyncPoint = render_to_dmabuf(
        &mut native,
        &mut damage_tracker,
        swapchain_dma.clone(),
        &scene,
        RenderElementStates::default(),
    )
    .expect("native KMS swapchain frame render must succeed");

    assert!(
        sync.is_exportable(),
        "Vulkan KMS completion sync point must be exportable"
    );
    let fence_fd = sync
        .export()
        .expect("Vulkan KMS completion sync point must export valid fence fd");
    assert!(
        fence_fd.as_raw_fd() >= 0,
        "exported sync file fd must be valid descriptor for IN_FENCE_FD"
    );

    // Wait on sync completion
    sync.wait().expect("KMS sync point wait must succeed");

    // Recreate the renderer. This does not inject DEVICE_LOST or exercise KMS.
    drop(native);
    let mut recovered_vk = crate::render_helpers::vulkan::VulkanRenderer::new();
    let node_path = std::env::var("CLARION_VULKAN_TEST_NODE")
        .unwrap_or_else(|_| "/dev/dri/renderD128".to_string());
    let drm_node = smithay::backend::drm::DrmNode::from_path(&node_path).unwrap();
    recovered_vk
        .init_from_drm_node(Some(&drm_node))
        .expect("Vulkan device recovery must succeed on DRM node");

    let mut recovered_native = native::NativeRenderer::new(recovered_vk)
        .expect("recovered NativeRenderer must initialize");
    recovered_native
        .prepare_output_format_for_test(Fourcc::Argb8888)
        .unwrap();

    // 3. Verify that a recreated native renderer can render into a fresh DMA-BUF.
    let mut swapchain_dma2: Dmabuf = allocator
        .create_buffer(w, h, Fourcc::Argb8888, &mods)
        .unwrap()
        .export()
        .unwrap();
    let mut damage_tracker2 =
        OutputDamageTracker::new((w as i32, h as i32), 1.0, Transform::Normal);

    let sync2 = render_to_dmabuf(
        &mut recovered_native,
        &mut damage_tracker2,
        swapchain_dma2.clone(),
        &scene,
        RenderElementStates::default(),
    )
    .expect("recovered Vulkan renderer must render cleanly to KMS swapchain");
    sync2.wait().expect("recovered sync wait must succeed");

    let fb = recovered_native.bind(&mut swapchain_dma2).unwrap();
    let mapping = recovered_native
        .copy_framebuffer(
            &fb,
            Rectangle::from_size((w as i32, h as i32).into()),
            Fourcc::Argb8888,
        )
        .expect("recovered Vulkan readback must succeed");
    let pixels = recovered_native.map_texture(&mapping).unwrap().to_vec();
    let non_zero_count = pixels.iter().filter(|&&b| b > 0).count();
    assert!(non_zero_count > (w * h * 2) as usize);

    eprintln!(
        "PASS: offscreen DMA-BUF rendering, SYNC_FD export and renderer recreation (no KMS submission or device-loss injection)"
    );
}

#[test]
#[ignore = "requires real DRM render node and Vulkan"]
fn native_offscreen_multioutput_scaling_rotation_and_readback() {
    use smithay::backend::allocator::dmabuf::Dmabuf;
    use smithay::backend::renderer::damage::OutputDamageTracker;
    use smithay::backend::renderer::element::{Kind, RenderElementStates};
    use smithay::backend::renderer::{Bind, ExportMem, ImportDma};
    use smithay::reexports::gbm::Format as Fourcc;
    use smithay::utils::{Point, Rectangle, Transform};

    use crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement;
    use crate::render_helpers::render_to_dmabuf;
    use crate::render_helpers::renderer::AsGlesRenderer;
    use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
    use crate::render_helpers::texture::{TextureBuffer, TextureRenderElement};
    use crate::render_helpers::vulkan::native;
    use crate::scene::OutputRenderElements;

    let (vk, reference, mut allocator) = setup();
    // 1. Verify 100% GLES absence: completely drop GLES reference context
    drop(reference);

    let mut native = native::NativeRenderer::new(vk).unwrap();
    // Verify no GLES scene renderer exists or is accessible
    assert!(
        native.as_gles_renderer().is_none(),
        "NativeRenderer must not wrap or expose GLES"
    );

    let mods: Vec<_> = native
        .dmabuf_formats()
        .iter()
        .filter(|f| f.code == Fourcc::Argb8888)
        .map(|f| f.modifier)
        .collect();
    assert!(
        !mods.is_empty(),
        "NativeRenderer must support ARGB8888 DMA-BUF"
    );

    // 2. Multi-Monitor verification:
    // Output A: 1920x1080 with 1.25 fractional scale, Transform::Normal
    let (w_a, h_a) = (1920u32, 1080u32);
    let mut swapchain_a: Dmabuf = allocator
        .create_buffer(w_a, h_a, Fourcc::Argb8888, &mods)
        .unwrap()
        .export()
        .unwrap();
    let mut damage_tracker_a =
        OutputDamageTracker::new((w_a as i32, h_a as i32), 1.25, Transform::Normal);

    // Output B: 1200x1920 vertical rotated monitor with 1.5 fractional scale, Transform::_90
    let (w_b, h_b) = (1200u32, 1920u32);
    let swapchain_b: Dmabuf = allocator
        .create_buffer(w_b, h_b, Fourcc::Argb8888, &mods)
        .unwrap()
        .export()
        .unwrap();
    let mut damage_tracker_b =
        OutputDamageTracker::new((w_b as i32, h_b as i32), 1.5, Transform::_90);

    // Construct desktop elements
    let bg_buf_a = SolidColorBuffer::new((w_a as f64, h_a as f64), [0.1, 0.2, 0.3, 1.0]);
    let bg_elem_a: OutputRenderElements<native::NativeRenderer> =
        OutputRenderElements::SolidColor(SolidColorRenderElement::from_buffer(
            &bg_buf_a,
            Point::from((0., 0.)),
            1.0,
            Kind::Unspecified,
        ));

    let bg_buf_b = SolidColorBuffer::new((w_b as f64, h_b as f64), [0.2, 0.1, 0.3, 1.0]);
    let bg_elem_b: OutputRenderElements<native::NativeRenderer> =
        OutputRenderElements::SolidColor(SolidColorRenderElement::from_buffer(
            &bg_buf_b,
            Point::from((0., 0.)),
            1.0,
            Kind::Unspecified,
        ));

    let win_pixels = vec![220u8; 400 * 300 * 4];
    let win_tex = TextureBuffer::from_memory(
        &mut native,
        &win_pixels,
        Fourcc::Argb8888,
        (400, 300),
        false,
        1.25,
        Transform::Normal,
        Vec::new(),
    )
    .unwrap();
    let win_elem_a: OutputRenderElements<native::NativeRenderer> = OutputRenderElements::Texture(
        PrimaryGpuTextureRenderElement::from(TextureRenderElement::from_texture_buffer(
            win_tex.clone(),
            Point::from((100., 100.)),
            1.0,
            None,
            None,
            Kind::Unspecified,
        )),
    );
    let win_elem_b: OutputRenderElements<native::NativeRenderer> = OutputRenderElements::Texture(
        PrimaryGpuTextureRenderElement::from(TextureRenderElement::from_texture_buffer(
            win_tex,
            Point::from((100., 100.)),
            1.0,
            None,
            None,
            Kind::Unspecified,
        )),
    );

    // Render Output A with fractional scale
    let sync_a = render_to_dmabuf(
        &mut native,
        &mut damage_tracker_a,
        swapchain_a.clone(),
        &[win_elem_a, bg_elem_a],
        RenderElementStates::default(),
    )
    .expect("Output A (scale 1.25) must render natively to swapchain DMA-BUF");
    sync_a
        .wait()
        .expect("Output A render completion sync failed");

    // Render Output B with rotation (Transform::_90)
    let sync_b = render_to_dmabuf(
        &mut native,
        &mut damage_tracker_b,
        swapchain_b.clone(),
        &[win_elem_b, bg_elem_b],
        RenderElementStates::default(),
    )
    .expect("Output B (rotated 90 deg, scale 1.5) must render natively to swapchain DMA-BUF");
    sync_b
        .wait()
        .expect("Output B render completion sync failed");

    // Verify Output A readback pixels
    let fb_a = native.bind(&mut swapchain_a).unwrap();
    let mapping_a = native
        .copy_framebuffer(
            &fb_a,
            Rectangle::from_size((w_a as i32, h_a as i32).into()),
            Fourcc::Argb8888,
        )
        .expect("Output A readback must succeed");
    let pixels_a = native.map_texture(&mapping_a).unwrap();
    let non_zero_a = pixels_a.iter().filter(|&&p| p > 0).count();
    assert!(
        non_zero_a > (w_a * h_a * 2) as usize,
        "Output A must contain rendered desktop pixels, got {non_zero_a}"
    );

    // 3. Monitor Hotplugging verification:
    // Hot-unplug Output B, then hotplug Output C (2560x1440, scale 1.75, Transform::_180)
    drop(swapchain_b);
    drop(damage_tracker_b);

    let (w_c, h_c) = (2560u32, 1440u32);
    let swapchain_c: Dmabuf = allocator
        .create_buffer(w_c, h_c, Fourcc::Argb8888, &mods)
        .unwrap()
        .export()
        .unwrap();
    let mut damage_tracker_c =
        OutputDamageTracker::new((w_c as i32, h_c as i32), 1.75, Transform::_180);

    let bg_buf_c = SolidColorBuffer::new((w_c as f64, h_c as f64), [0.3, 0.3, 0.05, 1.0]);
    let bg_elem_c: OutputRenderElements<native::NativeRenderer> =
        OutputRenderElements::SolidColor(SolidColorRenderElement::from_buffer(
            &bg_buf_c,
            Point::from((0., 0.)),
            1.0,
            Kind::Unspecified,
        ));

    let sync_c = render_to_dmabuf(
        &mut native,
        &mut damage_tracker_c,
        swapchain_c.clone(),
        &[bg_elem_c],
        RenderElementStates::default(),
    )
    .expect("Hotplugged Output C must render natively");
    sync_c.wait().expect("Output C completion sync failed");

    // 4. Screen Recording / Screencopy verification:
    // Capture Output A into a separate recording buffer (1920x1080)
    let recording_dma: Dmabuf = allocator
        .create_buffer(w_a, h_a, Fourcc::Argb8888, &mods)
        .unwrap()
        .export()
        .unwrap();
    let mut recording_damage_tracker =
        OutputDamageTracker::new((w_a as i32, h_a as i32), 1.0, Transform::Normal);

    let rec_sync = render_to_dmabuf(
        &mut native,
        &mut recording_damage_tracker,
        recording_dma.clone(),
        &[OutputRenderElements::SolidColor(
            SolidColorRenderElement::from_buffer(
                &bg_buf_a,
                Point::from((0., 0.)),
                1.0,
                Kind::Unspecified,
            ),
        )],
        RenderElementStates::default(),
    )
    .expect("Screencopy recording stream must render natively");
    rec_sync.wait().expect("Recording sync wait must succeed");

    // 5. Short offscreen reuse check; does not exercise suspend/resume or long-term stability.
    // Render 10 continuous frames through Output C to verify scratch buffer pooling and memory
    // stability
    for frame_idx in 0..10 {
        let dynamic_color = [0.1 * (frame_idx as f32), 0.2, 0.3, 1.0];
        let frame_buf = SolidColorBuffer::new((w_c as f64, h_c as f64), dynamic_color);
        let frame_elem: OutputRenderElements<native::NativeRenderer> =
            OutputRenderElements::SolidColor(SolidColorRenderElement::from_buffer(
                &frame_buf,
                Point::from((0., 0.)),
                1.0,
                Kind::Unspecified,
            ));
        let sync = render_to_dmabuf(
            &mut native,
            &mut damage_tracker_c,
            swapchain_c.clone(),
            &[frame_elem],
            RenderElementStates::default(),
        )
        .expect("continuous desktop frame must render smoothly");
        sync.wait().expect("continuous frame sync wait failed");
    }

    eprintln!(
        "PASS: offscreen multioutput scaling, rotation, readback and ten-frame reuse (no physical hotplug, suspend/resume or desktop-session validation)"
    );
}

#[test]
#[ignore = "requires real DRM render node, EGL readback and Vulkan"]
fn native_screenshot_selection_pixels_and_pointer() {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    use smithay::backend::renderer::ImportMem;
    use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};

    use crate::animation::Clock;
    use crate::ui::screenshot_ui::{OutputScreenshot, ScreenshotUi};

    let (vk, reference, _) = setup();
    drop(reference);
    let mut renderer = native::NativeRenderer::new(vk).unwrap();
    let output = Output::new(
        "screenshot-test".into(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "test".into(),
            model: "test".into(),
            serial_number: "test".into(),
        },
    );
    let mut source = Vec::new();
    for y in 0..48u8 {
        for x in 0..64u8 {
            source.extend_from_slice(&[x, y, 80, 255]);
        }
    }
    for scale in [1., 1.5, 2.] {
        output.change_current_state(
            Some(Mode {
                size: (64, 48).into(),
                refresh: 60_000,
            }),
            Some(Transform::Normal),
            Some(Scale::Fractional(scale)),
            None,
        );
        for show_pointer in [false, true] {
            let captures = std::array::from_fn(|_| {
                let texture = renderer
                    .import_memory(&source, Fourcc::Abgr8888, (64, 48).into(), false)
                    .unwrap();
                let pointer = renderer
                    .import_memory(&[255; 4 * 4 * 4], Fourcc::Abgr8888, (4, 4).into(), false)
                    .unwrap();
                OutputScreenshot::from_native_textures(
                    &mut renderer,
                    scale.into(),
                    texture,
                    Some((pointer, Rectangle::new((20, 16).into(), (4, 4).into()))),
                )
            });
            let mut ui = ScreenshotUi::new(
                Clock::default(),
                Rc::new(RefCell::new(clarion_config::Config::default())),
            );
            assert!(ui.open_native(
                &mut renderer,
                HashMap::from([(output.clone(), captures)]),
                output.clone(),
                show_pointer,
                None,
            ));
            let (size, pixels) = ui.capture_native(&mut renderer).unwrap();
            assert_eq!(size, (32, 24).into());
            for y in 0..24usize {
                for x in 0..32usize {
                    let expected = if show_pointer && (4..8).contains(&x) && (4..8).contains(&y) {
                        [255; 4]
                    } else {
                        [(x + 16) as u8, (y + 12) as u8, 80, 255]
                    };
                    let offset = (y * 32 + x) * 4;
                    assert_eq!(
                        &pixels[offset..offset + 4],
                        &expected,
                        "selection pixel ({x}, {y}), scale={scale}, pointer={show_pointer}"
                    );
                }
            }
            assert!(ui.close());
        }
    }
}

#[test]
#[ignore = "requires real DRM render node, EGL readback and Vulkan"]
fn native_screenshot_selection_survives_output_changes() {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    use smithay::backend::renderer::ImportMem;
    use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};

    use crate::animation::Clock;
    use crate::ui::screenshot_ui::{OutputScreenshot, ScreenshotUi};

    let (vk, reference, _) = setup();
    drop(reference);
    let mut renderer = native::NativeRenderer::new(vk).unwrap();
    let make_output = |name: &str| {
        let output = Output::new(
            name.into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "test".into(),
                model: "test".into(),
                serial_number: "test".into(),
            },
        );
        output.change_current_state(
            Some(Mode {
                size: (64, 48).into(),
                refresh: 60_000,
            }),
            Some(Transform::Normal),
            None,
            None,
        );
        output
    };
    let available = make_output("available");
    let failed = make_output("capture-failed");
    let source: Vec<u8> = (0..48u8)
        .flat_map(|y| (0..64u8).flat_map(move |x| [x, y, 80, 255]))
        .collect();
    let center = Rectangle::new((16, 12).into(), (32, 24).into());
    for (old, expected) in [
        (None, center),
        (
            Some(Rectangle::new((60, 40).into(), (20, 20).into())),
            Rectangle::new((60, 40).into(), (4, 8).into()),
        ),
        (
            Some(Rectangle::new((100, 100).into(), (20, 20).into())),
            center,
        ),
    ] {
        let mut ui = ScreenshotUi::Closed {
            last_selection: old.map(|rect| (available.downgrade(), rect)),
            clock: Clock::default(),
            config: Rc::new(RefCell::new(clarion_config::Config::default())),
        };
        let captures = std::array::from_fn(|_| {
            let texture = renderer
                .import_memory(&source, Fourcc::Abgr8888, (64, 48).into(), false)
                .unwrap();
            OutputScreenshot::from_native_textures(&mut renderer, 1.0.into(), texture, None)
        });
        assert!(ui.open_native(
            &mut renderer,
            HashMap::from([(available.clone(), captures)]),
            failed.clone(),
            false,
            None
        ));
        assert_eq!(ui.selection_output(), Some(&available));
        let (size, pixels) = ui.capture_native(&mut renderer).unwrap();
        assert_eq!(size, expected.size);
        for y in 0..size.h {
            for x in 0..size.w {
                let offset = ((y * size.w + x) * 4) as usize;
                assert_eq!(
                    &pixels[offset..offset + 4],
                    &[
                        (expected.loc.x + x) as u8,
                        (expected.loc.y + y) as u8,
                        80,
                        255,
                    ]
                );
            }
        }
        assert!(ui.close());
        assert!(!ui.open_native(&mut renderer, HashMap::new(), failed.clone(), false, None));
        assert!(!ui.is_open());
    }
}

#[test]
#[ignore = "requires real DRM render node, EGL readback and Vulkan"]
fn native_offscreen_recycle_preserves_live_images_and_alpha() {
    use smithay::backend::renderer::Texture as _;
    let (vk, _reference, _) = setup();
    let mut renderer = native::NativeRenderer::new(vk).unwrap();
    let texture = renderer
        .create_buffer(Fourcc::Abgr8888, (32, 24).into())
        .unwrap();
    let retained = texture.clone();
    renderer.recycle_offscreen(texture);
    let replacement = renderer
        .create_buffer(Fourcc::Abgr8888, (32, 24).into())
        .unwrap();
    assert_ne!(
        retained.cache_key(),
        replacement.cache_key(),
        "must not overwrite a live snapshot"
    );
    drop(retained);
    renderer.recycle_offscreen(replacement);
    let opaque = renderer
        .create_buffer(Fourcc::Xbgr8888, (32, 24).into())
        .unwrap();
    assert!(
        !opaque.has_alpha,
        "same Vulkan format does not imply same alpha semantics"
    );
    assert_eq!(opaque.format(), Some(Fourcc::Xbgr8888));
    // A foreign texture must never enter this renderer's offscreen pool.
    let (other, _reference, _) = setup();
    let mut other = native::NativeRenderer::new(other).unwrap();
    let foreign = other
        .create_buffer(Fourcc::Abgr8888, (17, 13).into())
        .unwrap();
    renderer.recycle_offscreen(foreign);
    let mut local = renderer
        .create_buffer(Fourcc::Abgr8888, (17, 13).into())
        .unwrap();
    let mut target = renderer.bind(&mut local).unwrap();
    let mut frame = renderer
        .render(&mut target, (17, 13).into(), Transform::Normal)
        .unwrap();
    frame
        .clear(Color32F::BLACK, &[Rectangle::from_size((17, 13).into())])
        .unwrap();
    frame.finish().unwrap().wait().unwrap();
}

#[test]
#[ignore = "requires real DRM render node, EGL readback and Vulkan"]
fn native_pooled_snapshots_sample_without_cpu_wait() {
    use std::time::Instant;

    use smithay::backend::renderer::element::Kind;

    use crate::render_helpers::render_to_vec;
    use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
    use crate::render_helpers::texture::{TextureBuffer, TextureRenderElement};

    let (vk, _reference, _) = setup();
    let mut renderer = native::NativeRenderer::new(vk).unwrap();
    let mut previous = None;
    for i in 0..24 {
        let color = if i % 2 == 0 {
            [1., 0., 0., 1.]
        } else {
            [0., 0., 1., 1.]
        };
        let background = SolidColorBuffer::new((160., 96.), color);
        let corner = SolidColorBuffer::new((20., 10.), [0., 1., 0., 1.]);
        let elements = [
            SolidColorRenderElement::from_buffer(&background, (0., 0.), 1., Kind::Unspecified),
            SolidColorRenderElement::from_buffer(&corner, (7., 3.), 1., Kind::Unspecified),
        ];
        let front_to_back = elements.iter().rev().collect::<Vec<_>>();
        let (texture, _sync, geo, states) =
            crate::render_helpers::offscreen::render_native_offscreen(
                &mut renderer,
                1.0.into(),
                &front_to_back,
            )
            .unwrap();
        assert_eq!(geo, Rectangle::from_size((160, 96).into()));
        for element in &elements {
            use smithay::backend::renderer::element::Element;
            assert!(
                states.element_was_presented(element.id().clone()),
                "animation must preserve surface visibility"
            );
        }
        if let Some(key) = previous {
            assert_eq!(
                texture.cache_key(),
                key,
                "reuse completed snapshot allocation"
            );
        }
        previous = Some(texture.cache_key());
        renderer.recycle_offscreen(texture.clone());
        // Deliberately do not wait for the source frame. A later submission samples it
        // through the native queue's image barrier; only final readback waits for GPU.
        let buffer =
            TextureBuffer::from_texture(&renderer, texture, 1.0, Transform::Normal, Vec::new());
        let sampled = TextureRenderElement::from_texture_buffer(
            buffer,
            (0., 0.),
            1.,
            None,
            None,
            Kind::Unspecified,
        );
        let pixels = render_to_vec(
            &mut renderer,
            (160, 96).into(),
            1.0.into(),
            Transform::Normal,
            Fourcc::Abgr8888,
            std::iter::once(&sampled),
        )
        .unwrap();
        for (index, pixel) in pixels.as_chunks::<4>().0.iter().enumerate() {
            let x = index % 160;
            let y = index / 160;
            let expected = if (7..27).contains(&x) && (3..13).contains(&y) {
                [0, 255, 0, 255]
            } else if i % 2 == 0 {
                [255, 0, 0, 255]
            } else {
                [0, 0, 255, 255]
            };
            assert_eq!(*pixel, expected, "frame {i}, pixel {index}");
        }
    }
    const N: usize = 400;
    let start = Instant::now();
    for _ in 0..N {
        drop(
            renderer
                .create_buffer(Fourcc::Abgr8888, (512, 256).into())
                .unwrap(),
        );
    }
    let allocated = start.elapsed();
    let start = Instant::now();
    for _ in 0..N {
        let texture = renderer
            .create_buffer(Fourcc::Abgr8888, (512, 256).into())
            .unwrap();
        renderer.recycle_offscreen(texture);
    }
    eprintln!(
        "BENCH: {N} offscreen targets: allocate={allocated:?}, recycle={:?}",
        start.elapsed()
    );
}

#[test]
#[ignore = "requires DRM, Vulkan and glslc"]
fn native_animation_glsl_interface_and_crossfade() {
    use smithay::backend::renderer::ImportMem;

    use crate::render_helpers::render_to_vec;
    use crate::render_helpers::resize::ResizeRenderElement;
    use crate::render_helpers::shaders::{animation_source, ProgramType};
    let (vk, _gles, _) = setup();
    let mut renderer = native::NativeRenderer::new(vk).unwrap();
    for kind in [ProgramType::Resize, ProgramType::Open, ProgramType::Close] {
        renderer
            .animation_program(kind, &animation_source(kind, None))
            .unwrap();
    }
    for source in [
        crate::render_helpers::shaders::DENIAL_EXPLOSION_SHADER,
        crate::render_helpers::shaders::DENIAL_IMPLODE_SHADER,
    ] {
        renderer
            .animation_program(
                ProgramType::Close,
                &animation_source(ProgramType::Close, Some(source)),
            )
            .unwrap();
    }
    let prev = renderer
        .import_memory(
            &[128, 0, 0, 128].repeat(64),
            Fourcc::Abgr8888,
            (8, 8).into(),
            false,
        )
        .unwrap();
    let next = renderer
        .import_memory(
            &[0, 0, 255, 255].repeat(64),
            Fourcc::Abgr8888,
            (8, 8).into(),
            false,
        )
        .unwrap();
    renderer
        .prepare_output_format_for_test(Fourcc::Abgr8888)
        .unwrap();
    let element = ResizeRenderElement::new_native(
        &mut renderer,
        None,
        Rectangle::from_size((8., 8.).into()),
        1.0.into(),
        (prev, Rectangle::from_size((8, 8).into())),
        (8., 8.).into(),
        (next, Rectangle::from_size((8, 8).into())),
        (8., 8.).into(),
        0.5,
        0.5,
        Default::default(),
        false,
        1.,
    )
    .unwrap();
    let result = render_to_vec(
        &mut renderer,
        (8, 8).into(),
        1.0.into(),
        Transform::Normal,
        Fourcc::Abgr8888,
        std::iter::once(&element),
    )
    .unwrap();
    for pixel in result.as_chunks::<4>().0 {
        for (actual, expected) in pixel.iter().zip([64u8, 0, 128, 192]) {
            assert!(
                actual.abs_diff(expected) <= 1,
                "crossfade must mix premultiplied pixels: {pixel:?}"
            );
        }
    }
}

#[test]
#[ignore = "requires DRM, Vulkan and glslc"]
fn native_animation_three_sampled_textures() {
    use std::collections::HashMap;
    use std::rc::Rc;

    use smithay::backend::renderer::element::Kind;
    use smithay::backend::renderer::gles::Uniform;
    use smithay::backend::renderer::ImportMem;

    use crate::render_helpers::render_to_vec;
    use crate::render_helpers::shader_element::ShaderRenderElement;
    use crate::render_helpers::shaders::{animation_source, mat3_uniform, ProgramType};

    let (vk, _gles, _) = setup();
    let mut renderer = native::NativeRenderer::new(vk).unwrap();
    renderer
        .prepare_output_format_for_test(Fourcc::Abgr8888)
        .unwrap();
    let source = animation_source(
        ProgramType::Close,
        Some(
            "uniform sampler2D second;\nuniform sampler2D third;\n\
             vec4 close_color(vec3 p, vec3 s) {\n\
             return (texture2D(clarion_tex, p.xy) + texture2D(second, p.xy) + texture2D(third, p.xy)) / 3.0;\n}",
        ),
    );
    renderer
        .wait_for_animation_program_for_test(ProgramType::Close, &source)
        .unwrap();
    let input = [
        ("clarion_tex", [90, 0, 0, 255]),
        ("second", [0, 90, 0, 255]),
        ("third", [0, 0, 90, 255]),
    ]
    .into_iter()
    .map(|(name, pixel)| {
        let texture = renderer
            .import_memory(&pixel.repeat(64), Fourcc::Abgr8888, (8, 8).into(), false)
            .unwrap();
        (name.to_owned(), texture)
    })
    .collect::<HashMap<_, _>>();
    let uniforms: Rc<[Uniform<'static>]> = Rc::new([
        mat3_uniform("clarion_input_to_geo", glam::Mat3::IDENTITY),
        mat3_uniform("clarion_geo_to_tex", glam::Mat3::IDENTITY),
        Uniform::new("clarion_geo_size", [8., 8.]),
        Uniform::new("clarion_glide_enabled", 0f32),
    ]);
    let element = ShaderRenderElement::new_native(
        &mut renderer,
        ProgramType::Close,
        &source,
        (8., 8.).into(),
        1.,
        1.,
        uniforms,
        input,
        Kind::Unspecified,
    )
    .unwrap();
    let pixels = render_to_vec(
        &mut renderer,
        (8, 8).into(),
        1.0.into(),
        Transform::Normal,
        Fourcc::Abgr8888,
        std::iter::once(&element),
    )
    .unwrap();
    for pixel in pixels.as_chunks::<4>().0 {
        for (actual, expected) in pixel.iter().zip([30u8, 30, 30, 255]) {
            assert!(actual.abs_diff(expected) <= 2, "{pixel:?}");
        }
    }
}

#[test]
#[ignore = "requires real DRM render node and Vulkan"]
fn native_output_prewarm_builds_standard_formats_before_first_draw() {
    let (vk, _gles, _allocator) = setup();
    let manager = vk.pipeline_mgr.as_ref().unwrap().clone();
    let mut renderer = native::NativeRenderer::new(vk).unwrap();
    let rgba8 = vk::Format::R8G8B8A8_UNORM;
    let ten_bit = vk::Format::A2B10G10R10_UNORM_PACK32;
    assert!(manager.prepared_pipelines(rgba8).is_none());
    assert!(manager.prepared_pipelines(ten_bit).is_none());

    // No custom shader is loaded. Even an ordinary BGRA8 output must prepare
    // the standard pipelines used by its first RGBA8 screenshot or effect.
    renderer
        .prepare_output_format_for_test(Fourcc::Argb8888)
        .unwrap();
    assert!(manager.prepared_pipelines(rgba8).is_some());
    assert!(manager.prepared_pipelines(ten_bit).is_none());

    // Only a negotiated 10-bit output makes that format eager as well.
    renderer
        .prepare_output_format_for_test(Fourcc::Abgr2101010)
        .unwrap();
    let handles = |pipes: pipeline::FormatPipelines| {
        [
            pipes.quad_pipeline,
            pipes.border_pipeline,
            pipes.shadow_pipeline,
            pipes.blur_pipeline,
            pipes.particle_pipeline,
            pipes.alpha_one_pipeline,
            pipes.mesh_pipeline,
        ]
    };
    for (code, format) in [(Fourcc::Abgr8888, rgba8), (Fourcc::Abgr2101010, ten_bit)] {
        let prepared = handles(manager.prepared_pipelines(format).unwrap());
        assert!(prepared
            .iter()
            .all(|handle| *handle != vk::Pipeline::null()));
        renderer.prepare_output_format_for_test(code).unwrap();
        assert_eq!(
            handles(manager.prepared_pipelines(format).unwrap()),
            prepared
        );

        // Exercise a real first draw and readback: prepared pipelines must be
        // usable without a later replacement, including a 10-bit attachment.
        let size = (8, 8).into();
        let all = Rectangle::from_size(size);
        let mut texture = renderer.create_buffer(code, (8, 8).into()).unwrap();
        {
            let mut target = renderer.bind(&mut texture).unwrap();
            let mut frame = renderer
                .render(&mut target, size, Transform::Normal)
                .unwrap();
            frame.clear(Color32F::BLACK, &[all]).unwrap();
            frame
                .draw_solid(all, &[all], [1., 0., 0., 1.].into())
                .unwrap();
            frame.finish().unwrap().wait().unwrap();
        }
        let mapping = renderer
            .copy_texture(
                &texture,
                Rectangle::from_size((8, 8).into()),
                Fourcc::Abgr8888,
            )
            .unwrap();
        let pixels = renderer.map_texture(&mapping).unwrap();
        assert_eq!(pixels.len(), 8 * 8 * 4);
        assert!(
            pixels.as_chunks::<4>().0.iter().all(|pixel| {
                pixel
                    .iter()
                    .zip([255, 0, 0, 255])
                    .all(|(actual, expected)| actual.abs_diff(expected) <= 1)
            }),
            "first prepared draw must retain its pixels for {code:?}"
        );
        assert_eq!(
            handles(manager.prepared_pipelines(format).unwrap()),
            prepared
        );
    }
}

#[test]
#[ignore = "requires real DRM render node and Vulkan"]
fn native_animation_prewarm_keeps_old_and_new_close_programs() {
    use std::collections::HashMap;
    use std::rc::Rc;

    use smithay::backend::renderer::element::Kind;
    use smithay::backend::renderer::ImportMem;

    use crate::render_helpers::render_to_vec;
    use crate::render_helpers::shader_element::ShaderRenderElement;
    use crate::render_helpers::shaders::{animation_source, ProgramType, DENIAL_EXPLOSION_SHADER};
    let (vk, _gles, _allocator) = setup();
    let mut renderer = native::NativeRenderer::new(vk).unwrap();
    let default_source = animation_source(ProgramType::Close, None);
    let explosion_source = animation_source(ProgramType::Close, Some(DENIAL_EXPLOSION_SHADER));
    let default = renderer
        .animation_program(ProgramType::Close, &default_source)
        .unwrap();
    let format = vk::Format::B8G8R8A8_UNORM;
    assert!(!default.has_prepared_format(format));

    // KMS chooses this format before the first frame. Existing shaders and
    // shaders subsequently loaded by config changes must be ready for drawing.
    renderer
        .prepare_output_format_for_test(Fourcc::Argb8888)
        .unwrap();
    assert!(default.has_prepared_format(format));
    assert!(default.has_prepared_format(vk::Format::R8G8B8A8_UNORM));
    let explosion = renderer
        .wait_for_animation_program_for_test(ProgramType::Close, &explosion_source)
        .unwrap();
    assert!(explosion.has_prepared_format(format));
    assert!(explosion.has_prepared_format(vk::Format::R8G8B8A8_UNORM));
    assert!(!Arc::ptr_eq(&default, &explosion));
    assert!(Arc::ptr_eq(
        &renderer
            .animation_program(ProgramType::Close, &default_source)
            .unwrap(),
        &default,
    ));
    assert!(Arc::ptr_eq(
        &renderer
            .animation_program(ProgramType::Close, &explosion_source)
            .unwrap(),
        &explosion,
    ));
    let invalid = "this is invalid GLSL";
    assert!(Arc::ptr_eq(
        &renderer
            .animation_program(ProgramType::Close, invalid)
            .unwrap(),
        &explosion,
    ));
    assert!(renderer
        .wait_for_animation_program_for_test(ProgramType::Close, invalid)
        .is_err());
    renderer
        .animation_program(ProgramType::Close, &default_source)
        .unwrap();
    assert!(
        Arc::ptr_eq(
            &renderer
                .animation_program(ProgramType::Close, invalid)
                .unwrap(),
            &default,
        ),
        "a cached invalid source must follow the latest working shader"
    );

    // Screen captures and nested effects render animations into RGBA8 offscreen
    // buffers. Exercise that draw path after prewarming, not just the cache map.
    let texture = renderer
        .import_memory(
            &[255, 0, 0, 255].repeat(64),
            Fourcc::Abgr8888,
            (8, 8).into(),
            false,
        )
        .unwrap();
    let element = ShaderRenderElement::new_native(
        &mut renderer,
        ProgramType::Close,
        &explosion_source,
        (8., 8.).into(),
        1.,
        1.,
        Rc::new([]),
        HashMap::from([(String::from("clarion_tex"), texture)]),
        Kind::Unspecified,
    )
    .unwrap();
    let pixels = render_to_vec(
        &mut renderer,
        (8, 8).into(),
        1.0.into(),
        Transform::Normal,
        Fourcc::Abgr8888,
        std::iter::once(&element),
    )
    .unwrap();
    assert_eq!(pixels.len(), 8 * 8 * 4);
}

#[test]
#[ignore = "requires DRM, Vulkan and GLES"]
fn native_animation_shaders_match_gles() {
    use std::collections::HashMap;
    use std::rc::Rc;

    use smithay::backend::renderer::element::Kind;
    use smithay::backend::renderer::gles::Uniform;
    use smithay::backend::renderer::ImportMem;

    use crate::render_helpers::shader_element::ShaderRenderElement;
    use crate::render_helpers::{render_to_vec, shaders};
    let (vk, mut reference, _) = setup();
    let mut renderer = native::NativeRenderer::new(vk).unwrap();
    renderer
        .prepare_output_format_for_test(Fourcc::Abgr8888)
        .unwrap();
    shaders::init(&mut reference);
    crate::render_helpers::resources::init(&mut reference);
    let data = (0..40)
        .flat_map(|y| {
            (0..64).flat_map(move |x| [((x * 3) % 128) as u8, ((y * 5) % 128) as u8, 64, 192])
        })
        .collect::<Vec<_>>();
    let texture = renderer
        .import_memory(&data, Fourcc::Abgr8888, (64, 40).into(), false)
        .unwrap();
    let ref_texture = reference
        .import_memory(&data, Fourcc::Abgr8888, (64, 40).into(), false)
        .unwrap();
    for (kind, custom) in [
        (shaders::ProgramType::Open, None),
        (shaders::ProgramType::Close, None),
        (
            shaders::ProgramType::Close,
            Some(shaders::DENIAL_EXPLOSION_SHADER),
        ),
        (
            shaders::ProgramType::Close,
            Some(shaders::DENIAL_IMPLODE_SHADER),
        ),
        (
            shaders::ProgramType::Open,
            Some(
                "vec4 open_color(vec3 p, vec3 s) { return texture2D(clarion_tex, p.xy) * clarion_progress; }",
            ),
        ),
    ] {
        match kind {
            shaders::ProgramType::Open => shaders::set_custom_open_program(&mut reference, custom),
            _ => shaders::set_custom_close_program(&mut reference, custom),
        }
        renderer.wait_for_animation_program_for_test(kind,&shaders::animation_source(kind,custom)).unwrap();
        for (glide, progress) in [0., 1.].into_iter().flat_map(|glide| {
            [0.12f32, 0.30, 0.45, 1.].map(|progress| (glide, progress))
        }) {
            let uniforms: Rc<[Uniform<'static>]> = Rc::new([
                shaders::mat3_uniform("clarion_input_to_geo", glam::Mat3::IDENTITY),
                shaders::mat3_uniform("clarion_geo_to_tex", glam::Mat3::IDENTITY),
                Uniform::new("clarion_geo_size", [64., 40.]),
                Uniform::new("clarion_progress", progress),
                Uniform::new("clarion_clamped_progress", progress),
                Uniform::new("clarion_random_seed", 0.375f32),
                Uniform::new("clarion_glide_enabled", glide),
                Uniform::new("clarion_glide_tilt", 0.25f32),
                Uniform::new("clarion_glide_depth", 0.1f32),
                Uniform::new("clarion_popin_ratio", 0.8f32),
            ]);
            let uniforms: Rc<[Uniform<'static>]> = uniforms
                .iter()
                .filter(|uniform| {
                    matches!(kind, shaders::ProgramType::Open)
                        || uniform.name != "clarion_popin_ratio"
                })
                .cloned()
                .collect::<Vec<_>>()
                .into();
            let native = ShaderRenderElement::new_native(
                &mut renderer,
                kind,
                &shaders::animation_source(kind, custom),
                (64., 40.).into(),
                1.,
                0.8,
                uniforms.clone(),
                HashMap::from([("clarion_tex".into(), texture.clone())]),
                Kind::Unspecified,
            )
            .unwrap();
            let gles = ShaderRenderElement::new(
                kind,
                (64., 40.).into(),
                None,
                1.,
                0.8,
                uniforms,
                HashMap::from([("clarion_tex".into(), ref_texture.clone())]),
                Kind::Unspecified,
            );
            for transform in [Transform::Normal, Transform::_90, Transform::Flipped] {
                let size = transform.transform_size((64, 40).into());
                let actual = render_to_vec(
                    &mut renderer,
                    size,
                    1.0.into(),
                    transform,
                    Fourcc::Abgr8888,
                    std::iter::once(&native),
                )
                .unwrap();
                let expected = render_to_vec(
                    &mut reference,
                    size,
                    1.0.into(),
                    transform,
                    Fourcc::Abgr8888,
                    std::iter::once(&gles),
                )
                .unwrap();
                let max = actual
                    .iter()
                    .zip(&expected)
                    .map(|(a, b)| a.abs_diff(*b))
                    .max()
                    .unwrap();
                let bad = actual
                    .iter()
                    .zip(&expected)
                    .filter(|(a, b)| a.abs_diff(**b) > 8)
                    .count();
                assert!(
                    bad < actual.len() / 100,
                    "shader {kind:?}, progress {progress}, glide {glide}, {transform:?}: max error {max}, mismatches {bad}/{}",
                    actual.len()
                );
            }
        }
    }
    let source = shaders::animation_source(shaders::ProgramType::Open, None);
    let previous = renderer
        .animation_program(shaders::ProgramType::Open, &source)
        .unwrap();
    let invalid = renderer
        .animation_program(shaders::ProgramType::Open, "this is invalid GLSL")
        .unwrap();
    assert!(
        Arc::ptr_eq(&previous, &invalid),
        "invalid reload must preserve the previous shader"
    );
    assert!(renderer
        .wait_for_animation_program_for_test(shaders::ProgramType::Open, "this is invalid GLSL")
        .is_err());
}

#[test]
#[ignore = "requires DRM and Vulkan"]
fn native_custom_shader_compiler_completion_prepares_on_idle_owner() {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use crate::backend::headless::HeadlessRenderer;
    use crate::render_helpers::renderer::AnyRenderer;
    use crate::render_helpers::shaders::{
        animation_source, ProgramType, DENIAL_EXPLOSION_SHADER, DENIAL_IMPLODE_SHADER,
    };
    use crate::tests::Fixture;
    #[derive(Clone)]
    struct Identity {
        renderer: u64,
        kind: u8,
        generation: u64,
        source: String,
        success: bool,
    }
    fn cpu() -> Option<Duration> {
        let mut t = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: clock_gettime writes only this owned timespec.
        if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) } != 0
            || t.tv_sec < 0
            || !(0..1_000_000_000).contains(&t.tv_nsec)
        {
            return None;
        }
        Some(Duration::new(t.tv_sec as u64, t.tv_nsec as u32))
    }
    fn with_native<T>(
        f: &mut Fixture,
        mut action: impl FnMut(&mut native::NativeRenderer) -> T,
    ) -> T {
        f.clarion_state()
            .backend
            .with_renderer(|renderer| {
                let AnyRenderer::Native(native) = renderer else {
                    panic!("native owner replaced");
                };
                action(native)
            })
            .unwrap()
    }
    fn wait(f: &mut Fixture, mut condition: impl FnMut(&mut native::NativeRenderer) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !with_native(f, &mut condition) {
            assert!(
                Instant::now() < deadline,
                "idle GPU owner completion deadline"
            );
            let server = &mut f.state.server;
            server
                .event_loop
                .dispatch(Duration::from_millis(50), &mut server.state)
                .unwrap();
        }
    }
    let (vk, _gles, _allocator) = setup();
    let mut renderer = native::NativeRenderer::new(vk).unwrap();
    renderer
        .prepare_output_format_for_test(Fourcc::Argb8888)
        .unwrap();
    let kind = ProgramType::Close;
    let default_source = animation_source(kind, None);
    let default = renderer
        .configure_animation_program(kind, &default_source)
        .unwrap();
    renderer
        .prepare_output_format_for_test(Fourcc::Argb8888)
        .unwrap();
    let first = animation_source(
        kind,
        Some("vec4 close_color(vec3 p, vec3 s) { return texture2D(clarion_tex,p.xy)*0.25; }"),
    );
    let second = animation_source(
        kind,
        Some("vec4 close_color(vec3 p, vec3 s) { return texture2D(clarion_tex,p.xy)*0.75; }"),
    );
    assert!(Arc::ptr_eq(
        &renderer.configure_animation_program(kind, &first).unwrap(),
        &default
    ));
    assert!(Arc::ptr_eq(
        &renderer.configure_animation_program(kind, &second).unwrap(),
        &default
    ));
    let cpu_source = renderer.take_shader_completion_source().unwrap();
    let gpu_source = renderer.take_gpu_preparation_source().unwrap();
    let records = Rc::new(RefCell::new(Vec::new()));
    let cpu_records = records.clone();
    let owner = std::thread::current().id();
    let mut config = clarion_config::Config::default();
    config.animations.off = true;
    config.hotkey_overlay.skip_at_startup = true;
    let mut f = Fixture::with_config(config);
    f.state.server.event_loop.handle().insert_source(cpu_source,move |event,_,state| {
        if let calloop::channel::Event::Msg(completion)=event {
            assert_eq!(std::thread::current().id(),owner);
            let identity=Identity {renderer:completion.renderer,kind:completion.kind,generation:completion.generation,source:completion.source.clone(),success:completion.result.is_ok()};
            let start=Instant::now();let cpu_start=cpu();state.on_native_shader_completion(completion);
            eprintln!("SHADER_OWNER_WORK scope=cpu_result_enqueue_gpu_only wall_ms={} thread_cpu_ms={:?}",start.elapsed().as_secs_f64()*1000.,cpu().zip(cpu_start).and_then(|(end,start)|end.checked_sub(start)).map(|v|v.as_secs_f64()*1000.));
            cpu_records.borrow_mut().push(identity);
        }
    }).unwrap();
    f.state.server.event_loop.handle().insert_source(gpu_source,move |event,_,state| {
        if let calloop::channel::Event::Msg(event)=event {
            assert_eq!(std::thread::current().id(),owner);
            let start=Instant::now();let cpu_start=cpu();state.on_native_gpu_preparation_event(event);
            eprintln!("SHADER_OWNER_WORK scope=gpu_result_validation_publication_only wall_ms={} thread_cpu_ms={:?}",start.elapsed().as_secs_f64()*1000.,cpu().zip(cpu_start).and_then(|(end,start)|end.checked_sub(start)).map(|v|v.as_secs_f64()*1000.));
        }
    }).unwrap();
    let mut headless = crate::backend::Headless::new(
        f.clarion().config.clone(),
        crate::backend::RendererBackendType::Vulkan,
    );
    headless.renderer = Some(HeadlessRenderer::Native(Box::new(renderer)));
    headless.init(&mut crate::platform::device_bridge::DeviceCoordinator::new(
        f.clarion(),
    ));
    f.clarion_state().backend = crate::backend::Backend::Headless(headless);
    f.add_output(1, (64, 40));
    let output = f.clarion_output(1);
    f.clarion().output_state.get_mut(&output).unwrap().schedule =
        crate::output_schedule::OutputSchedule::new(
            Some(Duration::from_nanos(16_666_667)),
            false,
            false,
        );
    assert!(matches!(
        f.clarion().output_state[&output].schedule.redraw_state(),
        crate::output_schedule::RedrawState::Idle
    ));
    wait(&mut f, |native| {
        native
            .prepared_animation_program_for_test(kind, &first)
            .is_some()
            && native
                .prepared_animation_program_for_test(kind, &second)
                .is_some()
    });
    assert_eq!(records.borrow().len(), 2);
    assert!(records.borrow().iter().all(|record| record.success));
    assert!(matches!(
        f.clarion().output_state[&output].schedule.redraw_state(),
        crate::output_schedule::RedrawState::Queued
    ));
    let (first_program, second_program) = with_native(&mut f, |native| {
        let first_program = native
            .prepared_animation_program_for_test(kind, &first)
            .unwrap();
        let second_program = native
            .prepared_animation_program_for_test(kind, &second)
            .unwrap();
        assert!(second_program.has_prepared_format(vk::Format::B8G8R8A8_UNORM));
        assert!(second_program.has_prepared_format(vk::Format::R8G8B8A8_UNORM));
        assert!(Arc::ptr_eq(
            &native.animation_program(kind, &first).unwrap(),
            &first_program
        ));
        (first_program, second_program)
    });
    assert!(!Arc::ptr_eq(&first_program, &second_program));
    assert!(!Arc::ptr_eq(&default, &second_program));
    let invalid = "this is invalid GLSL";
    assert!(Arc::ptr_eq(
        &with_native(&mut f, |native| native
            .configure_animation_program(kind, invalid)
            .unwrap()),
        &second_program
    ));
    wait(&mut f, |_| records.borrow().len() == 3);
    assert!(!records.borrow()[2].success);
    let old = records.borrow()[0].clone();
    with_native(&mut f, |native| {
        for (renderer, generation) in [
            (old.renderer, old.generation),
            (u64::MAX, old.generation),
            (old.renderer, u64::MAX),
        ] {
            assert!(
                !native.apply_shader_completion(super::custom_shader::CompileData {
                    renderer,
                    kind: old.kind,
                    generation,
                    source: old.source.clone(),
                    result: Err("late completion".into())
                })
            );
        }
        assert!(Arc::ptr_eq(
            &native.animation_program(kind, invalid).unwrap(),
            &second_program
        ));
    });
    // Evict the actual configured fallback from the four historical entries while external
    // captured elements retain it. Its weak live-program registration must still warm formats.
    for custom in [
        Some(DENIAL_EXPLOSION_SHADER),
        Some(DENIAL_IMPLODE_SHADER),
        None,
    ] {
        let source = animation_source(kind, custom);
        with_native(&mut f, |native| {
            native.animation_program(kind, &source).unwrap();
        });
        wait(&mut f, |native| {
            native
                .prepared_animation_program_for_test(kind, &source)
                .is_some()
        });
    }
    assert!(with_native(&mut f, |native| native
        .prepared_animation_program_for_test(kind, &second)
        .is_none()));
    assert!(!second_program.has_prepared_format(vk::Format::A2B10G10R10_UNORM_PACK32));
    with_native(&mut f, |native| {
        let start = Instant::now();
        let cpu_start = cpu();
        native.prepare_output_format(Fourcc::Abgr2101010).unwrap();
        eprintln!("SHADER_OWNER_WORK scope=request_output_format_nonblocking wall_ms={} thread_cpu_ms={:?}",start.elapsed().as_secs_f64()*1000.,cpu().zip(cpu_start).and_then(|(end,start)|end.checked_sub(start)).map(|v|v.as_secs_f64()*1000.));
        // A cold display must not stop an already prepared output while the worker compiles.
        native.begin_scene();
        native
            .ensure_target_format_ready(vk::Format::B8G8R8A8_UNORM)
            .unwrap();
        assert!(!native.scene_was_deferred());
        assert!(!native.take_backpressure());
    });
    wait(&mut f, |native| {
        native
            .ensure_target_format_ready(vk::Format::A2B10G10R10_UNORM_PACK32)
            .is_ok()
    });
    assert!(second_program.has_prepared_format(vk::Format::A2B10G10R10_UNORM_PACK32));
}

#[test]
#[ignore = "requires DRM and Vulkan"]
fn native_xray_background_cache_and_target_isolation() {
    use smithay::backend::renderer::element::Kind;

    use crate::render_helpers::background_effect::RenderParams;
    use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
    use crate::render_helpers::xray::{Xray, XrayPos};
    use crate::render_helpers::{render_to_vec, RenderCtx, RenderTarget};
    let (vk, _gles, _) = setup();
    let mut renderer = native::NativeRenderer::new(vk).unwrap();
    // This fixture exercises prepared rendering; wait on the bounded test-only
    // preparer before its pixel, timing or ownership assertions begin.
    renderer
        .prepare_output_format_for_test(Fourcc::Abgr8888)
        .unwrap();
    let mut xray = Xray::new();
    xray.workspaces.push((
        Rectangle::from_size((32., 24.).into()),
        Color32F::TRANSPARENT,
    ));
    let colors = [[1., 0., 0., 1.], [0., 1., 0., 1.], [0., 0., 1., 1.]];
    for (i, color) in colors.into_iter().enumerate() {
        let mut background = xray.background[i].borrow_mut();
        background.update_size((32, 24).into(), 1.0.into());
        background.elements_native().push(
            SolidColorRenderElement::from_buffer(
                &SolidColorBuffer::new((32., 24.), color),
                (0., 0.),
                1.,
                Kind::Unspecified,
            )
            .into(),
        );
        xray.backdrop[i]
            .borrow_mut()
            .update_size((32, 24).into(), 1.0.into());
    }
    for blur in [false, true] {
        for (i, target) in [
            RenderTarget::Output,
            RenderTarget::Screencast,
            RenderTarget::ScreenCapture,
        ]
        .into_iter()
        .enumerate()
        {
            let mut elements = Vec::new();
            xray.render(
                RenderCtx {
                    renderer: &mut renderer,
                    target,
                    xray: None,
                },
                RenderParams {
                    geometry: Rectangle::from_size((32., 24.).into()),
                    subregion: None,
                    clip: None,
                    scale: 1.,
                },
                XrayPos::default(),
                blur,
                0.,
                1.,
                &mut |elem| elements.push(elem),
            );
            let result = render_to_vec(
                &mut renderer,
                (32, 24).into(),
                1.0.into(),
                Transform::Normal,
                Fourcc::Abgr8888,
                elements.iter().rev(),
            )
            .unwrap();
            for pixel in result.as_chunks::<4>().0 {
                for (actual, expected) in pixel.iter().zip(colors[i].map(|v| (v * 255.) as u8)) {
                    assert!(
                        actual.abs_diff(expected) <= 1,
                        "xray target {target:?}, blur {blur}: {pixel:?}"
                    );
                }
            }
        }
    }
    let mut background = xray.background[0].borrow_mut();
    let retained = background.native_texture(false).unwrap().clone();
    let key = retained.cache_key();
    background.elements_native().clear();
    background.elements_native().push(
        SolidColorRenderElement::from_buffer(
            &SolidColorBuffer::new((32., 24.), [0., 1., 1., 1.]),
            (0., 0.),
            1.,
            Kind::Unspecified,
        )
        .into(),
    );
    assert!(background.prepare_any(&mut renderer, false));
    assert_ne!(
        background.native_texture(false).unwrap().cache_key(),
        key,
        "retained background cannot be overwritten"
    );
    let map = renderer
        .copy_texture(
            &retained,
            Rectangle::from_size((32, 24).into()),
            Fourcc::Abgr8888,
        )
        .unwrap();
    assert!(renderer
        .map_texture(&map)
        .unwrap()
        .as_chunks::<4>()
        .0
        .iter()
        .all(|p| *p == [255, 0, 0, 255]));
}

#[test]
#[ignore = "requires DRM and Vulkan"]
fn native_effect_buffer_keeps_unchanged_shared_background() {
    use smithay::backend::renderer::element::Kind;

    use crate::render_helpers::effect_buffer::EffectBuffer;
    use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};

    let (vk, _gles, _) = setup();
    let mut renderer = native::NativeRenderer::new(vk).unwrap();
    let mut background = EffectBuffer::new();
    background.update_size((96, 64).into(), 1.0.into());
    let mut source = SolidColorBuffer::new((96., 64.), [1., 0., 0., 1.]);
    let refill = |background: &mut EffectBuffer, source: &SolidColorBuffer| {
        let elements = background.elements_native();
        elements.clear();
        elements.push(
            SolidColorRenderElement::from_buffer(source, (0., 0.), 1., Kind::Unspecified).into(),
        );
    };
    refill(&mut background, &source);
    assert!(background.prepare_any(&mut renderer, true));
    let original = background.native_texture(false).unwrap().clone();
    let blurred = background.native_texture(true).unwrap().clone();
    let commit = background.commit();
    // Rebuilding the scene is not a content change, even while a transition
    // snapshot or submitted frame keeps both images alive.
    for _ in 0..48 {
        refill(&mut background, &source);
        assert!(background.prepare_any(&mut renderer, true));
        assert_eq!(
            background.native_texture(false).unwrap().cache_key(),
            original.cache_key()
        );
        assert_eq!(
            background.native_texture(true).unwrap().cache_key(),
            blurred.cache_key()
        );
        assert_eq!(background.commit(), commit);
    }
    source.set_color([0., 1., 0., 1.]);
    refill(&mut background, &source);
    assert!(background.prepare_any(&mut renderer, true));
    assert_ne!(
        background.native_texture(false).unwrap().cache_key(),
        original.cache_key()
    );
    assert_ne!(background.commit(), commit);
    for (texture, expected) in [
        (&original, [255, 0, 0, 255]),
        (background.native_texture(false).unwrap(), [0, 255, 0, 255]),
    ] {
        let mapping = renderer
            .copy_texture(
                texture,
                Rectangle::from_size((96, 64).into()),
                Fourcc::Abgr8888,
            )
            .unwrap();
        assert!(renderer
            .map_texture(&mapping)
            .unwrap()
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == expected));
    }
    // Also exercise changed pixels in an unshared image. The preliminary
    // damage check must not consume the repaint needed by the real render.
    renderer.retire_completed().unwrap();
    source.set_color([0., 0., 1., 1.]);
    refill(&mut background, &source);
    assert!(background.prepare_any(&mut renderer, false));
    let mapping = renderer
        .copy_texture(
            background.native_texture(false).unwrap(),
            Rectangle::from_size((96, 64).into()),
            Fourcc::Abgr8888,
        )
        .unwrap();
    assert!(renderer
        .map_texture(&mapping)
        .unwrap()
        .as_chunks::<4>()
        .0
        .iter()
        .all(|p| *p == [0, 0, 255, 255]));
}

#[test]
#[ignore = "requires DRM and Vulkan"]
fn native_animation_target_growth_reuses_capacity_and_clears_shrink() {
    use std::collections::HashSet;

    use smithay::backend::renderer::element::Kind;
    use smithay::backend::renderer::Texture as _;

    use crate::render_helpers::offscreen::NativeOffscreenBuffer;
    use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};

    let (vk, _gles, _) = setup();
    let mut renderer = native::NativeRenderer::new(vk).unwrap();
    let buffer = NativeOffscreenBuffer::default();
    let mut source = SolidColorBuffer::new((32., 24.), [1., 0., 0., 1.]);
    let mut identities = HashSet::new();
    for width in (33..96).chain([40, 32]) {
        source.resize((f64::from(width), 24.));
        let element =
            SolidColorRenderElement::from_buffer(&source, (-4., -6.), 1., Kind::Unspecified);
        let (texture, sync, geo, _, _) = buffer
            .render(&mut renderer, 1.0.into(), &[element])
            .unwrap();
        sync.wait().unwrap();
        identities.insert(texture.cache_key());
        assert_eq!(geo, Rectangle::new((-4, -6).into(), (width, 24).into()));
        let mapping = renderer
            .copy_texture(
                &texture,
                Rectangle::from_size((texture.width() as i32, texture.height() as i32).into()),
                Fourcc::Abgr8888,
            )
            .unwrap();
        let pixels = renderer.map_texture(&mapping).unwrap();
        for (i, pixel) in pixels.as_chunks::<4>().0.iter().enumerate() {
            let x = i % texture.width() as usize;
            let y = i / texture.width() as usize;
            let expected = if x < width as usize && y < 24 {
                [255, 0, 0, 255]
            } else {
                [0; 4]
            };
            assert_eq!(*pixel, expected, "width {width}, pixel ({x},{y})");
        }
    }
    assert!(
        identities.len() <= 2,
        "one-pixel animation steps allocated {} targets",
        identities.len()
    );
    println!(
        "PASS: 65 animated sizes used {} target allocations; shrink cleared old pixels",
        identities.len()
    );
}

#[test]
#[ignore = "requires DRM and Vulkan"]
fn native_transition_and_closing_capture_targets() {
    use std::time::Duration;

    use smithay::backend::renderer::ImportMem;

    use crate::animation::{Animation, Clock};
    use crate::layout::tile::NativeTileSnapshot;
    use crate::render_helpers::texture::TextureBuffer;
    use crate::render_helpers::{render_to_vec, RenderCtx, RenderTarget};
    use crate::scene::closing_window::ClosingWindow;
    use crate::ui::screen_transition::{ScreenTransition, DURATION};
    use crate::utils::transaction::Transaction;
    let (vk, _gles, _) = setup();
    let mut renderer = native::NativeRenderer::new(vk).unwrap();
    // This fixture exercises prepared rendering; wait on the bounded test-only
    // preparer before its pixel, timing or ownership assertions begin.
    renderer
        .prepare_output_format_for_test(Fourcc::Abgr8888)
        .unwrap();
    let colors = [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]];
    let textures = colors.map(|color| {
        renderer
            .import_memory(&color.repeat(32), Fourcc::Abgr8888, (8, 4).into(), false)
            .unwrap()
    });
    let mut clock = Clock::with_time(Duration::ZERO);
    let mut transition = ScreenTransition::new(
        textures.clone().map(|t| {
            TextureBuffer::from_texture(&renderer, t, 1., Transform::Normal, Vec::new()).into()
        }),
        Duration::ZERO,
        clock.clone(),
    );
    clock.set_unadjusted(DURATION / 2);
    for transform in [Transform::Normal, Transform::_90, Transform::Flipped] {
        transition.update_render_elements(1.0.into(), transform);
        for (i, target) in [
            RenderTarget::Output,
            RenderTarget::Screencast,
            RenderTarget::ScreenCapture,
        ]
        .into_iter()
        .enumerate()
        {
            let element = transition.render(target);
            let result = render_to_vec(
                &mut renderer,
                transform.transform_size((8, 4).into()),
                1.0.into(),
                Transform::Normal,
                Fourcc::Abgr8888,
                std::iter::once(&element),
            )
            .unwrap();
            assert!(result.as_chunks::<4>().0.iter().all(|p| {
                p.iter()
                    .zip(colors[i])
                    .all(|(a, b)| a.abs_diff((f32::from(b) * 0.5).round() as u8) <= 1)
            }));
        }
    }
    let transaction = Transaction::new();
    let scene_resources = crate::scene::resources::SceneResources::new();
    let closing = ClosingWindow::new_native(
        &mut renderer,
        1.0.into(),
        (8., 4.).into(),
        (0., 0.).into(),
        transaction.blocker(),
        Animation::new(
            clock.clone(),
            0.,
            1.,
            0.,
            clarion_config::Animations::default().window_close.anim,
        ),
        Default::default(),
        clarion_config::WindowCloseEffect::Fade,
        None,
        Some(NativeTileSnapshot {
            targets: textures.map(|t| (t, (0., 0.).into())),
        }),
        None,
        scene_resources.allocator().closing,
    )
    .unwrap();
    for (i, target) in [
        RenderTarget::Output,
        RenderTarget::Screencast,
        RenderTarget::ScreenCapture,
    ]
    .into_iter()
    .enumerate()
    {
        let elements = closing.render(
            RenderCtx {
                renderer: &mut renderer,
                target,
                xray: None,
            },
            Rectangle::from_size((8., 4.).into()),
            1.0.into(),
        );
        let result = render_to_vec(
            &mut renderer,
            (8, 4).into(),
            1.0.into(),
            Transform::Normal,
            Fourcc::Abgr8888,
            elements.iter().rev(),
        )
        .unwrap();
        assert!(
            result.as_chunks::<4>().0.iter().all(|p| *p == colors[i]),
            "closing target {target:?} must retain its own snapshot"
        );
    }
}

#[test]
#[ignore = "requires real DRM render node, Vulkan and GLES DMA-BUF producer"]
fn native_closing_animation_owns_a_stable_dmabuf_snapshot() {
    use std::time::Duration;

    use crate::animation::{Animation, Clock};
    use crate::render_helpers::{render_to_vec, RenderCtx, RenderTarget};
    use crate::scene::closing_window::ClosingWindow;
    use crate::utils::transaction::Transaction;

    let (vk, mut gles, mut allocator) = setup();
    let mods = modifiers(&vk, &gles, Fourcc::Argb8888);
    let mut source = allocator
        .create_buffer(8, 4, Fourcc::Argb8888, &mods)
        .unwrap()
        .export()
        .unwrap();
    let size = (8, 4).into();
    let rect = Rectangle::from_size(size);
    let repaint = |gles: &mut GlesRenderer, buffer: &mut Dmabuf, color: [f32; 4]| {
        let mut fb = gles.bind(buffer).unwrap();
        let mut frame = gles.render(&mut fb, size, Transform::Normal).unwrap();
        frame.clear(color.into(), &[rect]).unwrap();
        frame.finish().unwrap().wait().unwrap();
    };
    repaint(&mut gles, &mut source, [1., 0., 0., 1.]);

    let mut renderer = native::NativeRenderer::new(vk).unwrap();
    let clock = Clock::with_time(Duration::ZERO);
    let transaction = Transaction::new();
    let scene_resources = crate::scene::resources::SceneResources::new();
    let closing = ClosingWindow::new_native(
        &mut renderer,
        1.0.into(),
        (8., 4.).into(),
        (0., 0.).into(),
        transaction.blocker(),
        Animation::new(
            clock,
            0.,
            1.,
            0.,
            clarion_config::Animations::default().window_close.anim,
        ),
        Default::default(),
        clarion_config::WindowCloseEffect::Fade,
        None,
        None,
        Some(crate::scene::window_resources::NativeCloseSource::Unowned(
            source.clone(),
        )),
        scene_resources.allocator().closing,
    )
    .unwrap();

    // The client may immediately reuse its old wl_buffer after the surface
    // closes. Repainting its allocation must not recolor the closing animation.
    repaint(&mut gles, &mut source, [0., 1., 0., 1.]);
    let elements = closing.render(
        RenderCtx {
            renderer: &mut renderer,
            target: RenderTarget::Output,
            xray: None,
        },
        Rectangle::from_size((8., 4.).into()),
        1.0.into(),
    );
    let pixels = render_to_vec(
        &mut renderer,
        (8, 4).into(),
        1.0.into(),
        Transform::Normal,
        Fourcc::Abgr8888,
        elements.iter().rev(),
    )
    .unwrap();
    assert!(
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == [255, 0, 0, 255]),
        "closing animation must retain the red frame after the source DMA-BUF turns green"
    );
}

#[test]
#[ignore = "requires real DRM render node and Vulkan"]
fn native_explosion_close_does_not_tint_pixels_outside_the_window_area() {
    use std::time::Duration;

    use smithay::backend::renderer::ImportMem;

    use crate::animation::{Animation, Clock};
    use crate::layout::tile::NativeTileSnapshot;
    use crate::render_helpers::{render_to_vec, RenderCtx, RenderTarget};
    use crate::scene::closing_window::ClosingWindow;
    use crate::utils::transaction::Transaction;

    let (vk, _gles, _) = setup();
    let mut renderer = native::NativeRenderer::new(vk).unwrap();
    let texture = renderer
        .import_memory(
            &[255, 0, 0, 255].repeat(64 * 64),
            Fourcc::Abgr8888,
            (64, 64).into(),
            false,
        )
        .unwrap();
    let mut clock = Clock::with_time(Duration::ZERO);
    let transaction = Transaction::new();
    let scene_resources = crate::scene::resources::SceneResources::new();
    let mut closing = ClosingWindow::new_native(
        &mut renderer,
        1.0.into(),
        (64., 64.).into(),
        (160., 120.).into(),
        transaction.blocker(),
        Animation::ease(
            clock.clone(),
            0.,
            1.,
            0.,
            1000,
            crate::animation::Curve::Linear,
        ),
        Default::default(),
        clarion_config::WindowCloseEffect::Explosion,
        None,
        Some(NativeTileSnapshot {
            targets: std::array::from_fn(|_| (texture.clone(), (0., 0.).into())),
        }),
        None,
        scene_resources.allocator().closing,
    )
    .unwrap();

    let elements = closing.render(
        RenderCtx {
            renderer: &mut renderer,
            target: RenderTarget::Output,
            xray: None,
        },
        Rectangle::from_size((400., 300.).into()),
        1.0.into(),
    );
    let pixels = render_to_vec(
        &mut renderer,
        (400, 300).into(),
        1.0.into(),
        Transform::Normal,
        Fourcc::Abgr8888,
        elements.iter().rev(),
    )
    .unwrap();
    let pixel = |x: usize, y: usize| &pixels[(y * 400 + x) * 4..(y * 400 + x + 1) * 4];
    for (x, y) in [(0, 0), (399, 0), (0, 299), (399, 299)] {
        assert_eq!(pixel(x, y), &[0, 0, 0, 0], "unexpected tint at ({x}, {y})");
    }
    assert_ne!(pixel(192, 152), &[0, 0, 0, 0]);

    drop(transaction);
    closing.advance_animations();
    for (milliseconds, expected_elements) in [(200, 2), (500, 1), (1000, 0)] {
        clock.set_unadjusted(Duration::from_millis(milliseconds));
        closing.advance_animations();
        let elements = closing.render(
            RenderCtx {
                renderer: &mut renderer,
                target: RenderTarget::Output,
                xray: None,
            },
            Rectangle::from_size((400., 300.).into()),
            1.0.into(),
        );
        assert_eq!(
            elements.len(),
            expected_elements,
            "close phase at {milliseconds} ms"
        );
        if milliseconds == 500 {
            assert!(
                matches!(
                    elements[0],
                    crate::scene::closing_window::ClosingWindowRenderElement::Particles(_)
                ),
                "the transparent body must not be submitted during the particle tail"
            );
        }
        let pixels = render_to_vec(
            &mut renderer,
            (400, 300).into(),
            1.0.into(),
            Transform::Normal,
            Fourcc::Abgr8888,
            elements.iter().rev(),
        )
        .unwrap();
        if milliseconds == 1000 {
            assert!(
                pixels.iter().all(|&value| value == 0),
                "no closing residue at completion"
            );
        } else {
            assert!(
                pixels.iter().any(|&value| value != 0),
                "animation must remain visible"
            );
        }
    }
}

#[test]
#[ignore = "requires DRM, Vulkan and GLES"]
fn explosion_transparent_snapshot_has_no_flash_or_ring() {
    use std::collections::HashMap;
    use std::rc::Rc;

    use smithay::backend::renderer::element::Kind;
    use smithay::backend::renderer::gles::Uniform;
    use smithay::backend::renderer::ImportMem;

    use crate::render_helpers::shader_element::ShaderRenderElement;
    use crate::render_helpers::{render_to_vec, shaders};

    let (vk, mut gles, _) = setup();
    let mut native = native::NativeRenderer::new(vk).unwrap();
    shaders::init(&mut gles);
    shaders::set_custom_close_program(&mut gles, Some(shaders::DENIAL_EXPLOSION_SHADER));
    crate::render_helpers::resources::init(&mut gles);
    // Transparent source pixels must stay transparent throughout the body pass.
    // This catches both the old center flash and the expanding shockwave, on both APIs.
    let data = vec![0; 128 * 96 * 4];
    let texture = native
        .import_memory(&data, Fourcc::Abgr8888, (128, 96).into(), false)
        .unwrap();
    let ref_texture = gles
        .import_memory(&data, Fourcc::Abgr8888, (128, 96).into(), false)
        .unwrap();
    let source = shaders::animation_source(
        shaders::ProgramType::Close,
        Some(shaders::DENIAL_EXPLOSION_SHADER),
    );
    for progress in [0., 0.08, 0.2, 0.37, 0.38, 0.5, 0.75, 1.] {
        let uniforms: Rc<[Uniform<'static>]> = Rc::new([
            shaders::mat3_uniform("clarion_input_to_geo", glam::Mat3::IDENTITY),
            shaders::mat3_uniform("clarion_geo_to_tex", glam::Mat3::IDENTITY),
            Uniform::new("clarion_geo_size", [128., 96.]),
            Uniform::new("clarion_progress", progress),
            Uniform::new("clarion_clamped_progress", progress),
            Uniform::new("clarion_glide_enabled", 0f32),
        ]);
        let element = ShaderRenderElement::new_native(
            &mut native,
            shaders::ProgramType::Close,
            &source,
            (128., 96.).into(),
            1.,
            1.,
            uniforms.clone(),
            HashMap::from([("clarion_tex".into(), texture.clone())]),
            Kind::Unspecified,
        )
        .unwrap();
        let reference = ShaderRenderElement::new(
            shaders::ProgramType::Close,
            (128., 96.).into(),
            None,
            1.,
            1.,
            uniforms,
            HashMap::from([("clarion_tex".into(), ref_texture.clone())]),
            Kind::Unspecified,
        );
        let actual = render_to_vec(
            &mut native,
            (128, 96).into(),
            1.0.into(),
            Transform::Normal,
            Fourcc::Abgr8888,
            std::iter::once(&element),
        )
        .unwrap();
        let expected = render_to_vec(
            &mut gles,
            (128, 96).into(),
            1.0.into(),
            Transform::Normal,
            Fourcc::Abgr8888,
            std::iter::once(&reference),
        )
        .unwrap();
        assert!(
            actual.iter().all(|&value| value == 0),
            "Vulkan halo at {progress}"
        );
        assert!(
            expected.iter().all(|&value| value == 0),
            "GLES halo at {progress}"
        );
    }
}

#[test]
#[ignore = "requires DRM and GLES"]
fn gles_particle_overlapping_damage_blends_once() {
    use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement};

    use crate::render_helpers::particles::{ParticleRenderElement, ParticleVertex};

    let (_vk, mut gles, _) = setup();
    crate::render_helpers::shaders::init(&mut gles);
    crate::render_helpers::resources::init(&mut gles);
    let corners = [[0., 0.], [0., 1.], [1., 1.], [1., 0.]].map(|position| ParticleVertex {
        position,
        local: [position[0] - 0.5, position[1] - 0.5],
        color: [0.5, 0., 0., 0.5],
        shape: 0.,
        inv_width: 1. / 32.,
        aspect: 1.,
    });
    let element = ParticleRenderElement::new(
        Id::new(),
        Default::default(),
        Rectangle::new((8., 8.).into(), (32., 32.).into()),
        [0, 1, 2, 0, 2, 3].map(|i| corners[i]).to_vec(),
        Kind::Unspecified,
    );
    let mut draw = |damage: &[Rectangle<i32, smithay::utils::Physical>]| {
        let mut output: smithay::backend::renderer::gles::GlesTexture = gles
            .create_buffer(Fourcc::Abgr8888, (48, 48).into())
            .unwrap();
        let mut target = gles.bind(&mut output).unwrap();
        let mut frame = gles
            .render(&mut target, (48, 48).into(), Transform::Normal)
            .unwrap();
        frame
            .clear(
                Color32F::TRANSPARENT,
                &[Rectangle::from_size((48, 48).into())],
            )
            .unwrap();
        <ParticleRenderElement as RenderElement<GlesRenderer>>::draw(
            &element,
            &mut frame,
            element.src(),
            element.geometry(1.0.into()),
            damage,
            &[],
            None,
        )
        .unwrap();
        frame.finish().unwrap().wait().unwrap();
        let map = gles
            .copy_framebuffer(
                &target,
                Rectangle::from_size((48, 48).into()),
                Fourcc::Abgr8888,
            )
            .unwrap();
        gles.map_texture(&map).unwrap().to_vec()
    };
    let expected = draw(&[Rectangle::from_size((32, 32).into())]);
    let actual = draw(&[
        Rectangle::from_size((24, 32).into()),
        Rectangle::new((8, 0).into(), (24, 32).into()),
        Rectangle::new((12, 8).into(), (8, 16).into()),
    ]);
    assert!(
        expected.iter().any(|&value| value != 0),
        "particle must be visible"
    );
    for (actual, expected) in actual.iter().zip(expected) {
        assert!(
            actual.abs_diff(expected) <= 1,
            "overlapping damage brightened the particle"
        );
    }
}

#[test]
#[ignore = "requires DRM, Vulkan and GLES"]
fn native_particle_batches_match_gles_with_perspective_and_meshes() {
    use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement};
    use smithay::backend::renderer::gles::GlesTexture;
    use smithay::backend::renderer::ImportMem;

    use crate::render_helpers::mesh::{MeshRenderElement, MeshVertex};
    use crate::render_helpers::particles::{ParticleRenderElement, ParticleVertex};

    let (vk, mut gles, _) = setup();
    let mut native = native::NativeRenderer::new(vk).unwrap();
    crate::render_helpers::shaders::init(&mut gles);
    crate::render_helpers::resources::init(&mut gles);
    let mesh_vertices = [[0., 0.], [0., 1.], [1., 1.], [0., 0.], [1., 1.], [1., 0.]]
        .map(|position| MeshVertex {
            position,
            uv: position,
        })
        .to_vec();
    let texture = native
        .import_memory(
            &[20, 30, 40, 80].repeat(64),
            Fourcc::Abgr8888,
            (8, 8).into(),
            false,
        )
        .unwrap();
    let ref_texture = gles
        .import_memory(
            &[20, 30, 40, 80].repeat(64),
            Fourcc::Abgr8888,
            (8, 8).into(),
            false,
        )
        .unwrap();
    let area = Rectangle::new((8., 6.).into(), (80., 64.).into());
    let mesh = MeshRenderElement::new_vulkan(
        Id::new(),
        Default::default(),
        texture,
        (8., 8.).into(),
        area,
        mesh_vertices.clone(),
        0.4,
        Kind::Unspecified,
    );
    let ref_mesh = MeshRenderElement::new(
        Id::new(),
        Default::default(),
        ref_texture,
        (8., 8.).into(),
        area,
        mesh_vertices,
        0.4,
        Kind::Unspecified,
    );
    let particles: Vec<_> = (0..2)
        .map(|batch| {
            let mut vertices = Vec::new();
            for index in 0..4 {
                // The fourth corner deliberately does not complete a parallelogram: the old
                // three-corner affine reconstruction changed this shape and its interpolation.
                let corners = [[0.08, 0.10], [0.16, 0.65], [0.62, 0.85], [0.90, 0.22]];
                let local = [[-0.5, -0.7], [-0.5, 0.7], [0.5, 0.7], [0.5, -0.7]];
                for i in [0, 1, 2, 0, 2, 3] {
                    vertices.push(ParticleVertex {
                        position: [
                            corners[i][0] * 0.7 + index as f32 * 0.06,
                            corners[i][1] * 0.7 + batch as f32 * 0.12,
                        ],
                        local: local[i],
                        color: [0.12, 0.08 + index as f32 * 0.05, 0.16, 0.35],
                        shape: if index == 0 { 1. } else { 0. },
                        inv_width: 0.02,
                        aspect: 1.4,
                    });
                }
            }
            ParticleRenderElement::new(
                Id::new(),
                Default::default(),
                area,
                vertices,
                Kind::Unspecified,
            )
        })
        .collect();
    for scale in [1.0, 1.25, 2.0] {
        let scale: smithay::utils::Scale<f64> = scale.into();
        let untransformed = smithay::utils::Size::<i32, smithay::utils::Physical>::from((96, 80))
            .to_f64()
            .upscale(scale)
            .to_i32_round();
        let dest = particles[0].geometry(scale);
        for transform in [
            Transform::Normal,
            Transform::_90,
            Transform::_180,
            Transform::_270,
            Transform::Flipped,
            Transform::Flipped90,
            Transform::Flipped180,
            Transform::Flipped270,
        ] {
            let size = transform.transform_size(untransformed);
            let damage = [
                Rectangle::from_size((dest.size.w * 3 / 4, dest.size.h).into()),
                Rectangle::new(
                    (dest.size.w / 4, 0).into(),
                    (dest.size.w * 3 / 4, dest.size.h).into(),
                ),
            ];
            macro_rules! render {
                ($renderer:ident, $renderer_type:ty, $texture_type:ty, $mesh:ident) => {{
                    let mut output: $texture_type = $renderer
                        .create_buffer(
                            Fourcc::Abgr8888,
                            size.to_logical(1).to_buffer(1, Transform::Normal),
                        )
                        .unwrap();
                    let mut target = $renderer.bind(&mut output).unwrap();
                    let mut frame = $renderer.render(&mut target, size, transform).unwrap();
                    frame
                        .clear(
                            Color32F::TRANSPARENT,
                            &[Rectangle::from_size(untransformed)],
                        )
                        .unwrap();
                    for particle in &particles {
                        <MeshRenderElement as RenderElement<$renderer_type>>::draw(
                            &$mesh,
                            &mut frame,
                            $mesh.src(),
                            dest,
                            &damage,
                            &[],
                            None,
                        )
                        .unwrap();
                        <ParticleRenderElement as RenderElement<$renderer_type>>::draw(
                            particle,
                            &mut frame,
                            particle.src(),
                            dest,
                            &damage,
                            &[],
                            None,
                        )
                        .unwrap();
                    }
                    <MeshRenderElement as RenderElement<$renderer_type>>::draw(
                        &$mesh,
                        &mut frame,
                        $mesh.src(),
                        dest,
                        &damage,
                        &[],
                        None,
                    )
                    .unwrap();
                    frame.finish().unwrap().wait().unwrap();
                    let map = $renderer
                        .copy_framebuffer(
                            &target,
                            Rectangle::from_size(
                                size.to_logical(1).to_buffer(1, Transform::Normal),
                            ),
                            Fourcc::Abgr8888,
                        )
                        .unwrap();
                    $renderer.map_texture(&map).unwrap().to_vec()
                }};
            }
            let actual = render!(native, native::NativeRenderer, VulkanTexture, mesh);
            let expected = render!(gles, GlesRenderer, GlesTexture, ref_mesh);
            assert!(actual.iter().any(|&value| value != 0));
            let bad = actual
                .iter()
                .zip(&expected)
                .filter(|(a, b)| a.abs_diff(**b) > 4)
                .count();
            assert!(
                bad < actual.len() / 100,
                "particle batch mismatch: {transform:?}, {scale:?}, {bad}/{}",
                actual.len()
            );
        }
    }
}

#[cfg(test)]
mod resize_capture_bounds_regressions {
    use smithay::backend::renderer::element::utils::{Relocate, RelocateRenderElement};
    use smithay::backend::renderer::element::RenderElement;
    use smithay::backend::renderer::{ImportMem, RendererSuper, Texture};
    use smithay::utils::{Logical, Physical, Scale, Size};

    use super::*;
    use crate::render_helpers::render_to_vec;
    use crate::render_helpers::resize::{capture_resize_texture, ResizeRenderElement};

    type Input<T> = ((T, Rectangle<i32, Physical>), Size<f64, Logical>);

    fn read_resize<R>(renderer: &mut R, element: &ResizeRenderElement, scale: Scale<f64>) -> Vec<u8>
    where
        R: Renderer
            + Offscreen<<R as RendererSuper>::TextureId>
            + Bind<<R as RendererSuper>::TextureId>
            + ExportMem,
        R::Error: Send + Sync + 'static,
        ResizeRenderElement: RenderElement<R>,
        for<'a> R::Framebuffer<'a>: Texture,
    {
        let shifted = RelocateRenderElement::from_element(element, (16, 16), Relocate::Relative);
        render_to_vec(
            renderer,
            (64, 56).into(),
            scale,
            Transform::Normal,
            Fourcc::Abgr8888,
            std::iter::once(&shifted),
        )
        .unwrap()
    }

    fn exercise_bounds<R, F>(renderer: &mut R, mut make: F)
    where
        R: Renderer
            + Offscreen<<R as RendererSuper>::TextureId>
            + Bind<<R as RendererSuper>::TextureId>
            + ExportMem
            + ImportMem,
        R::Error: Send + Sync + 'static,
        R::TextureId: Clone,
        ResizeRenderElement: RenderElement<R>,
        for<'a> R::Framebuffer<'a>: Texture,
        F: FnMut(
            &mut R,
            Rectangle<f64, Logical>,
            Scale<f64>,
            Input<R::TextureId>,
            Input<R::TextureId>,
            f32,
        ) -> ResizeRenderElement,
    {
        let displayed_size: Size<f64, Logical> = (16., 12.).into();
        let area = Rectangle::from_size(displayed_size);
        for factor in [1, 2] {
            let scale = Scale::from(f64::from(factor));
            // Different window sizes and different CSD margins, including
            // translucent nonzero edge pixels that CLAMP_TO_EDGE would repeat.
            let inputs = [
                ((8., 6.), (-1, -1, 10, 8)),
                ((16., 12.), (0, 0, 16, 12)),
                ((32., 24.), (-8, -8, 48, 40)),
            ]
            .into_iter()
            .enumerate()
            .map(|(index, (size, (x, y, width, height)))| {
                let width = width * factor;
                let height = height * factor;
                let mut pixels = Vec::with_capacity((width * height * 4) as usize);
                for y in 0..height {
                    for x in 0..width {
                        let edge = x == 0 || y == 0 || x == width - 1 || y == height - 1;
                        let alpha = if edge { 64u8 } else { 128u8 };
                        let mut color = [0u8; 4];
                        color[index] = alpha;
                        color[(index + 1) % 3] = if (x + y) % 2 == 0 { alpha / 3 } else { 0 };
                        color[3] = alpha;
                        pixels.extend_from_slice(&color);
                    }
                }
                let texture = renderer
                    .import_memory(&pixels, Fourcc::Abgr8888, (width, height).into(), false)
                    .unwrap();
                (
                    (
                        texture,
                        Rectangle::new((x * factor, y * factor).into(), (width, height).into()),
                    ),
                    Size::from(size),
                )
            })
            .collect::<Vec<_>>();
            let before = make(
                renderer,
                area,
                scale,
                inputs[0].clone(),
                inputs[1].clone(),
                0.4,
            );
            let expected = read_resize(renderer, &before, scale);
            let captured =
                capture_resize_texture(renderer, &before, scale, displayed_size).unwrap();
            let resumed = make(
                renderer,
                area,
                scale,
                captured.as_input(scale),
                inputs[2].clone(),
                0.,
            );
            let actual = read_resize(renderer, &resumed, scale);
            let worst = actual
                .iter()
                .zip(&expected)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap();
            assert!(
                worst <= 2,
                "retarget with larger CSD bounds changed pixels at scale {factor}: {worst}"
            );
            let outside_old_bounds = (((16 + 6 * factor) * 64 + 16 - 3 * factor) * 4) as usize;
            assert_eq!(
                &actual[outside_old_bounds..outside_old_bounds + 4],
                &[0, 0, 0, 0]
            );

            let finished = make(
                renderer,
                area,
                scale,
                captured.as_input(scale),
                inputs[2].clone(),
                1.,
            );
            let next_only = make(
                renderer,
                area,
                scale,
                inputs[2].clone(),
                inputs[2].clone(),
                1.,
            );
            let expected = read_resize(renderer, &next_only, scale);
            let actual = read_resize(renderer, &finished, scale);
            let worst = actual
                .iter()
                .zip(&expected)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap();
            assert!(
                worst <= 2,
                "completed resize must retain the new CSD bounds: {worst}"
            );
        }
    }

    #[test]
    #[ignore = "requires real DRM render node and GLES"]
    fn gles_resize_retarget_does_not_repeat_translucent_edges() {
        let (_vk, mut renderer, _) = setup();
        crate::render_helpers::shaders::init(&mut renderer);
        crate::render_helpers::resources::init(&mut renderer);
        exercise_bounds(
            &mut renderer,
            |_, area, scale, (previous, previous_size), (next, next_size), progress| {
                ResizeRenderElement::new(
                    area,
                    scale,
                    previous,
                    previous_size,
                    next,
                    next_size,
                    progress,
                    progress,
                    Default::default(),
                    false,
                    1.,
                )
            },
        );
    }

    #[test]
    #[ignore = "requires real DRM render node and Vulkan"]
    fn native_resize_retarget_does_not_repeat_translucent_edges() {
        let (vk, _gles, _) = setup();
        let mut renderer = native::NativeRenderer::new(vk).unwrap();
        exercise_bounds(
            &mut renderer,
            |renderer, area, scale, (previous, previous_size), (next, next_size), progress| {
                ResizeRenderElement::new_native(
                    renderer,
                    None,
                    area,
                    scale,
                    previous,
                    previous_size,
                    next,
                    next_size,
                    progress,
                    progress,
                    Default::default(),
                    false,
                    1.,
                )
                .unwrap()
            },
        );
    }
}

#[test]
#[ignore = "requires real DRM render node and Vulkan; no DRM display master"]
fn native_regional_framebuffer_blur_matches_full_pass_pixels() {
    use std::cell::RefCell;

    use clarion_config::CornerRadius;
    use smithay::backend::renderer::element::{Element, RenderElement};
    use smithay::backend::renderer::ImportMem;
    use smithay::utils::user_data::UserDataMap;
    use smithay::utils::{Buffer, Logical, Scale, Size};

    use crate::render_helpers::background_effect::RenderParams;
    use crate::render_helpers::blur::BlurOptions;
    use crate::render_helpers::framebuffer_effect::{
        FramebufferEffect, FramebufferEffectElement, NativeInner,
    };
    use crate::utils::region::TransformedRegion;

    let (vk, _gles, _allocator) = setup();
    let mut renderer = native::NativeRenderer::new(vk).unwrap();
    let size: Size<i32, Buffer> = (257, 193).into();
    let mut backgrounds = Vec::new();
    for phase in [0, 91] {
        let mut bytes = Vec::new();
        for y in 0..size.h {
            for x in 0..size.w {
                bytes.extend_from_slice(&[
                    ((x * 19 + y * 7 + phase) % 256) as u8,
                    ((x * 13 + y * 29 + phase) % 256) as u8,
                    ((x * 37 + y * 11 + phase) % 256) as u8,
                    255,
                ]);
            }
        }
        backgrounds.push(
            renderer
                .import_memory(&bytes, Fourcc::Abgr8888, size, false)
                .unwrap(),
        );
    }
    let mut targets: Vec<super::VulkanTexture> = (0..2)
        .map(|_| renderer.create_buffer(Fourcc::Abgr8888, size).unwrap())
        .collect();
    let transforms = [
        Transform::Normal,
        Transform::_90,
        Transform::_180,
        Transform::_270,
        Transform::Flipped,
        Transform::Flipped90,
        Transform::Flipped180,
        Transform::Flipped270,
    ];
    let regions: Vec<Option<Vec<Rectangle<i32, Logical>>>> = vec![
        Some(vec![Rectangle::new((0, 0).into(), (210, 12).into())]),
        Some(vec![
            Rectangle::new((0, 20).into(), (14, 30).into()),
            Rectangle::new((100, 12).into(), (30, 8).into()),
        ]),
        Some(vec![Rectangle::new((20, 30).into(), (130, 90).into())]),
        Some(vec![
            Rectangle::new((-40, -50).into(), (65, 75).into()),
            Rectangle::new((174, 125).into(), (50, 50).into()),
        ]),
        Some(vec![]),
        None,
        Some(vec![Rectangle::new((0, 0).into(), (210, 12).into())]),
    ];
    let mut frames = 0;
    let mut reduced_frames = 0;
    let mut regional_pixels = 0;
    let mut full_pixels = 0;
    for transform in transforms {
        for (scale, zoom, crop) in [
            (1., 1., false),
            (1.25, 0.7, true),
            (1.5, 1.3, false),
            (2., 0.8, true),
        ] {
            let mut options = vec![BlurOptions {
                passes: 3,
                offset: 2.75,
            }];
            if transform == Transform::Normal && scale == 1. {
                options.extend([
                    BlurOptions {
                        passes: 1,
                        offset: 0.,
                    },
                    BlurOptions {
                        passes: 31,
                        offset: 1.25,
                    },
                ]);
            }
            for options in options {
                let caches = [UserDataMap::new(), UserDataMap::new()];
                let effect = FramebufferEffect::new();
                for (step, region) in regions.iter().enumerate() {
                    let geometry = Rectangle::new((-13.25, -7.75).into(), (190.5, 140.25).into());
                    let subregion = region.clone().map(|rects| TransformedRegion {
                        rects: Arc::new(rects),
                        scale: Scale::from((1.1, 0.85)),
                        offset: geometry.loc,
                    });
                    let mut element = effect.render(
                        None,
                        RenderParams {
                            geometry,
                            subregion,
                            clip: Some((
                                Rectangle::new((-9.5, -3.25).into(), (180., 132.).into()),
                                CornerRadius {
                                    top_left: 11.,
                                    top_right: 17.,
                                    bottom_left: 7.,
                                    bottom_right: 9.,
                                },
                            )),
                            scale,
                        },
                        Some(options),
                        0.13,
                        1.2,
                    );
                    let mut src = element.src();
                    if crop {
                        src.loc += (3.25, 5.75).into();
                        src.size.w -= 14.5;
                        src.size.h -= 12.25;
                    }
                    let dst = Rectangle::new(
                        (-17, -9).into(),
                        src.size
                            .to_logical(1., Transform::Normal)
                            .to_physical_precise_round(scale * zoom),
                    );
                    let damage = [Rectangle::from_size(dst.size)];
                    let output = transform
                        .invert()
                        .transform_size(size.to_logical(1, Transform::Normal).to_physical(1));
                    let full = Rectangle::from_size(output);
                    let mut images = Vec::new();
                    for index in 0..2 {
                        element.full_blur_for_test = index == 1;
                        {
                            let mut fb = renderer.bind(&mut targets[index]).unwrap();
                            let mut frame = renderer
                                .render(
                                    &mut fb,
                                    size.to_logical(1, Transform::Normal).to_physical(1),
                                    transform,
                                )
                                .unwrap();
                            frame.clear(Color32F::BLACK, &[full]).unwrap();
                            frame
                                .render_texture_from_to(
                                    &backgrounds[step % 2],
                                    Rectangle::from_size(size.to_f64()),
                                    full,
                                    &[full],
                                    &[],
                                    Transform::Normal,
                                    1.,
                                )
                                .unwrap();
                            <FramebufferEffectElement as RenderElement<native::NativeRenderer>>::capture_framebuffer(&element, &mut frame, src, dst, &caches[index]).unwrap();
                            <FramebufferEffectElement as RenderElement<native::NativeRenderer>>::draw(
                            &element,
                            &mut frame,
                            src,
                            dst,
                            &damage,
                            &[],
                            Some(&caches[index]),
                        )
                        .unwrap();
                            frame.finish().unwrap().wait().unwrap();
                        }
                        let mapping = renderer
                            .copy_texture(
                                &targets[index],
                                Rectangle::from_size(size),
                                Fourcc::Abgr8888,
                            )
                            .unwrap();
                        images.push(renderer.map_texture(&mapping).unwrap().to_vec());
                    }
                    let worst = images[0]
                        .iter()
                        .zip(&images[1])
                        .map(|(a, b)| a.abs_diff(*b))
                        .max()
                        .unwrap();
                    assert_eq!(worst, 0, "regional/full blur mismatch transform={transform:?} scale={scale} zoom={zoom} crop={crop} region={step}");
                    let work = |index: usize| {
                        caches[index]
                            .get::<RefCell<NativeInner>>()
                            .unwrap()
                            .borrow()
                            .blur_pixels
                    };
                    let (regional, full) = (work(0), work(1));
                    assert!(regional <= full);
                    reduced_frames += usize::from(regional < full);
                    regional_pixels += regional;
                    full_pixels += full;
                    frames += 1;
                }
            }
        }
    }
    assert!(
        reduced_frames > 100,
        "expected real scissor work reduction, got {reduced_frames}"
    );
    eprintln!("PASS: regional framebuffer blur {frames} frames, all 8 transforms, fractional scales/crop/zoom, rounded glass/noise, moving/growing/disjoint/empty regions and cached-image reuse: byte-identical; blur fragment area {full_pixels} -> {regional_pixels} ({:.1}% less)", 100. * (1. - regional_pixels as f64 / full_pixels as f64));
}

#[test]
#[ignore = "requires DRM and Vulkan"]
fn native_transition_batch_reuses_equal_pixels_without_mutating_private_snapshots() {
    use smithay::backend::renderer::element::Kind;
    use smithay::utils::{Physical, Scale, Size};

    use crate::render_helpers::render_to_vec;
    use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
    use crate::render_helpers::transition_capture::TransitionCapture;

    let (vk, _gles, _) = setup();
    let mut renderer = native::NativeRenderer::new(vk).unwrap();
    for transform in [Transform::Normal, Transform::_90, Transform::Flipped180] {
        for scale in [1., 1.25, 2.] {
            let size: Size<i32, Physical> = (128, 96).into();
            let logical = transform
                .invert()
                .transform_size(size)
                .to_f64()
                .to_logical(scale);
            let mut batch = TransitionCapture::new(size, Scale::from(scale), transform);
            let mut front = SolidColorBuffer::new(logical, [1., 0., 0., 1.]);
            let mut back = SolidColorBuffer::new(logical, [0., 1., 0., 1.]);
            let scene = |front: &SolidColorBuffer, back: &SolidColorBuffer| {
                vec![
                    SolidColorRenderElement::from_buffer(front, (0., 0.), 1., Kind::Unspecified),
                    SolidColorRenderElement::from_buffer(back, (0., 0.), 1., Kind::Unspecified),
                ]
            };
            let mut images = Vec::new();
            let first = batch.capture(&mut renderer, &scene(&front, &back)).unwrap();
            for _ in 0..3 {
                let same = batch.capture(&mut renderer, &scene(&front, &back)).unwrap();
                assert_eq!(first.texture().cache_key(), same.texture().cache_key());
            }
            // Occluded updates cannot affect captured pixels or cause another draw.
            back.set_color([0., 0., 1., 1.]);
            let hidden_update = batch.capture(&mut renderer, &scene(&front, &back)).unwrap();
            assert_eq!(
                first.texture().cache_key(),
                hidden_update.texture().cache_key()
            );
            images.push((first, [255, 0, 0, 255]));
            // A protected target changes the same element's content and must get
            // its own storage even while earlier snapshots remain on screen.
            front.set_color([0., 0., 0., 1.]);
            let private = batch.capture(&mut renderer, &scene(&front, &back)).unwrap();
            assert_ne!(
                images[0].0.texture().cache_key(),
                private.texture().cache_key()
            );
            images.push((private, [0, 0, 0, 255]));
            for (image, expected) in &images {
                let mapping = renderer
                    .copy_texture(
                        image.texture(),
                        Rectangle::from_size(size.to_logical(1).to_buffer(1, Transform::Normal)),
                        Fourcc::Abgr8888,
                    )
                    .unwrap();
                assert!(renderer
                    .map_texture(&mapping)
                    .unwrap()
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .all(|p| *p == *expected));
            }
            // Alpha, geometry, empty scenes, fractional scales and transformed
            // outputs must match the old unculled full-paint pixel oracle.
            for phase in 0..4 {
                let elements = match phase {
                    0 => {
                        front.set_color([0.5, 0., 0., 0.5]);
                        scene(&front, &back)
                    }
                    1 => {
                        front.resize((logical.w * 0.43, logical.h * 0.71));
                        scene(&front, &back)
                    }
                    2 => Vec::new(),
                    _ => scene(&front, &back),
                };
                let expected = render_to_vec(
                    &mut renderer,
                    size,
                    scale.into(),
                    transform,
                    Fourcc::Abgr8888,
                    elements.iter().rev(),
                )
                .unwrap();
                let captured = batch.capture(&mut renderer, &elements).unwrap();
                let mapping = renderer
                    .copy_texture(
                        captured.texture(),
                        Rectangle::from_size((128, 96).into()),
                        Fourcc::Abgr8888,
                    )
                    .unwrap();
                let actual = renderer.map_texture(&mapping).unwrap();
                assert_eq!(actual.len(), expected.len());
                assert!(
                    actual
                        .iter()
                        .zip(&expected)
                        .all(|(a, b)| a.abs_diff(*b) <= 1),
                    "scale={scale}, transform={transform:?}, phase={phase}"
                );
            }
        }
    }
    eprintln!("HARDWARE: transition batch shares identical/occluded scenes; privacy, alpha, crop, empty, scale and transform pixel checks passed");
}

#[test]
#[ignore = "requires DRM and Vulkan"]
fn native_transition_batch_preserves_live_backdrop_pixels() {
    use smithay::backend::renderer::element::Kind;

    use crate::render_helpers::background_effect::RenderParams;
    use crate::render_helpers::blur::BlurOptions;
    use crate::render_helpers::framebuffer_effect::{FramebufferEffect, FramebufferEffectElement};
    use crate::render_helpers::render_to_vec;
    use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
    use crate::render_helpers::transition_capture::TransitionCapture;
    crate::clarion_render_elements! {
        CaptureTestElement => {
            Solid = SolidColorRenderElement,
            Effect = FramebufferEffectElement,
        }
    }
    let (vk, _gles, _) = setup();
    let mut renderer = native::NativeRenderer::new(vk).unwrap();
    for glass in [false, true] {
        let effect = FramebufferEffect::new();
        let mut background = SolidColorBuffer::new((128., 96.), [1., 0., 0., 1.]);
        let cover = SolidColorBuffer::new((32., 96.), [0., 0., 0., 1.]);
        let mut batch = TransitionCapture::new((128, 96).into(), 1.0.into(), Transform::Normal);
        let mut previous_key = None;
        let finish_effect = |element: FramebufferEffectElement| {
            if glass {
                element.with_liquid_glass(0)
            } else {
                element
            }
        };
        for color in [[1., 0., 0., 1.], [0., 1., 0., 1.], [0., 0., 1., 1.]] {
            background.set_color(color);
            let elements: Vec<CaptureTestElement> = vec![
                SolidColorRenderElement::from_buffer(&cover, (0., 0.), 1., Kind::Unspecified)
                    .into(),
                finish_effect(effect.render(
                    None,
                    RenderParams {
                        geometry: Rectangle::new((20., 8.).into(), (90., 80.).into()),
                        subregion: None,
                        clip: Some((
                            Rectangle::new((22., 10.).into(), (86., 76.).into()),
                            clarion_config::CornerRadius::from(9.),
                        )),
                        scale: 1.,
                    },
                    Some(BlurOptions {
                        passes: 2,
                        offset: 1.25,
                    }),
                    0.,
                    1.0,
                ))
                .into(),
                SolidColorRenderElement::from_buffer(&background, (0., 0.), 1., Kind::Unspecified)
                    .into(),
            ];
            let expected = render_to_vec(
                &mut renderer,
                (128, 96).into(),
                1.0.into(),
                Transform::Normal,
                Fourcc::Abgr8888,
                elements.iter().rev(),
            )
            .unwrap();
            let captured = batch.capture(&mut renderer, &elements).unwrap();
            assert_ne!(previous_key, Some(captured.texture().cache_key()));
            previous_key = Some(captured.texture().cache_key());
            let map = renderer
                .copy_texture(
                    captured.texture(),
                    Rectangle::from_size((128, 96).into()),
                    Fourcc::Abgr8888,
                )
                .unwrap();
            let actual = renderer.map_texture(&map).unwrap();
            assert!(actual
                .iter()
                .zip(&expected)
                .all(|(a, b)| a.abs_diff(*b) <= 1));
            let same = batch.capture(&mut renderer, &elements).unwrap();
            assert_eq!(previous_key, Some(same.texture().cache_key()));
        }
    }
    eprintln!("HARDWARE: transition captures preserve rounded blur/glass pixels, react to live backgrounds and reuse unchanged scenes");
}

#[test]
#[ignore = "requires DRM and Vulkan; reports controlled capture timings, not desktop FPS"]
fn native_transition_batch_reports_4k_capture_cost() {
    use std::time::Instant;

    use smithay::backend::renderer::element::Kind;

    use crate::render_helpers::render_to_texture;
    use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
    use crate::render_helpers::transition_capture::TransitionCapture;
    let (vk, _gles, _) = setup();
    let mut renderer = native::NativeRenderer::new(vk).unwrap();
    let buffers: Vec<_> = (0..24)
        .map(|i| SolidColorBuffer::new((1400., 1000.), [i as f32 / 24., 0.3, 0.7, 1.]))
        .collect();
    let elements: Vec<_> = buffers
        .iter()
        .enumerate()
        .map(|(i, b)| {
            SolidColorRenderElement::from_buffer(
                b,
                ((i % 6) as f64 * 230., (i / 6) as f64 * 160.),
                1.,
                Kind::Unspecified,
            )
        })
        .collect();
    let mut samples = [Vec::new(), Vec::new()];
    for round in 0..16 {
        // Reverse order each pair to reduce systematic warm-up/clock bias.
        for batched in if round % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let started = Instant::now();
            let images = if batched {
                let mut batch =
                    TransitionCapture::new((3840, 2400).into(), 1.0.into(), Transform::Normal);
                (0..3)
                    .map(|_| {
                        batch
                            .capture(&mut renderer, &elements)
                            .unwrap()
                            .texture()
                            .clone()
                    })
                    .collect::<Vec<_>>()
            } else {
                (0..3)
                    .map(|_| {
                        render_to_texture(
                            &mut renderer,
                            (3840, 2400).into(),
                            1.0.into(),
                            Transform::Normal,
                            Fourcc::Abgr8888,
                            elements.iter().rev(),
                        )
                        .unwrap()
                        .0
                    })
                    .collect::<Vec<_>>()
            };
            // One-pixel readback after both variants waits for the same queue;
            // otherwise the timing would omit asynchronously submitted GPU work.
            let map = renderer
                .copy_texture(
                    &images[2],
                    Rectangle::from_size((1, 1).into()),
                    Fourcc::Abgr8888,
                )
                .unwrap();
            assert_eq!(renderer.map_texture(&map).unwrap().len(), 4);
            if round >= 4 {
                samples[usize::from(batched)].push(started.elapsed().as_secs_f64() * 1000.);
            }
            if batched {
                assert_eq!(images[0].cache_key(), images[1].cache_key());
                assert_eq!(images[1].cache_key(), images[2].cache_key());
            }
        }
    }
    for (label, mut times) in ["previous", "batched"].into_iter().zip(samples) {
        times.sort_by(f64::total_cmp);
        eprintln!("CAPTURE_BENCH: {label} size=3840x2400 windows=24 targets=3 samples={} median_ms={:.3} max_ms={:.3}", times.len(), (times[5] + times[6]) / 2., times[11]);
    }
}

#[test]
#[ignore = "requires DRM, Vulkan and GLES"]
fn lightweight_window_open_matches_gles_and_preserves_group_opacity() {
    use std::time::Duration;

    use smithay::backend::renderer::element::{Element, Kind};

    use crate::animation::{Animation, Clock};
    use crate::layout::opening_window::OpenAnimation;
    use crate::render_helpers::render_to_vec;
    use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
    use crate::scene::opening_window::{
        render_opening_window_gles, render_opening_window_native, OpeningWindowRenderElement,
    };

    let (vk, mut gles, _) = setup();
    let mut native = native::NativeRenderer::new(vk).unwrap();
    native
        .prepare_output_format_for_test(Fourcc::Abgr8888)
        .unwrap();
    for scale in [1., 1.25, 2.] {
        let config = clarion_config::animations::WindowOpenAnim::default();
        let mut clock = Clock::with_time(Duration::ZERO);
        let make = || {
            OpenAnimation::new(
                Animation::new(clock.clone(), 0., 1., 0., config.anim),
                config.popin_ratio,
                config.glide,
                true,
                config.effect,
            )
        };
        let native_open = make();
        let gles_open = make();
        let native_buffers = crate::scene::window_resources::AnimationBuffers::default();
        let gles_buffers = crate::scene::window_resources::AnimationBuffers::default();
        let under = SolidColorBuffer::new((48., 32.), [1., 0., 0., 1.]);
        let mut over = SolidColorBuffer::new((20., 16.), [0., 1., 0., 1.]);
        for millis in [0, 16, 60, 90, 135, 180, 200] {
            clock.set_unadjusted(Duration::from_millis(millis));
            // A new client buffer during the animation must replace cached pixels.
            if millis == 135 {
                over.set_color([0., 0., 1., 1.]);
            }
            let elements = [
                SolidColorRenderElement::from_buffer(&over, (8., 6.), 1., Kind::Unspecified),
                SolidColorRenderElement::from_buffer(&under, (-4., -6.), 1., Kind::Unspecified),
            ];
            let (actual, native_data) = render_opening_window_native(
                &native_open.pose(),
                &mut native,
                &native_buffers,
                None,
                &elements,
                (40., 24.).into(),
                (16., 16.).into(),
                scale.into(),
                0.75,
            )
            .unwrap();
            let (expected, gles_data) = render_opening_window_gles(
                &gles_open.pose(),
                &mut gles,
                &gles_buffers,
                &elements,
                (40., 24.).into(),
                (16., 16.).into(),
                scale.into(),
                0.75,
            )
            .unwrap();
            assert!(matches!(actual, OpeningWindowRenderElement::Texture(_)));
            assert!(matches!(expected, OpeningWindowRenderElement::Offscreen(_)));
            assert_eq!(&native_data.id, actual.id());
            assert_eq!(&gles_data.id, expected.id());
            assert!(native_data
                .states
                .element_render_state(elements[0].id().clone())
                .is_some());
            let size = ((80. * scale) as i32, (64. * scale) as i32).into();
            let pixels = render_to_vec(
                &mut native,
                size,
                scale.into(),
                Transform::Normal,
                Fourcc::Abgr8888,
                std::iter::once(&actual),
            )
            .unwrap();
            let reference = render_to_vec(
                &mut gles,
                size,
                scale.into(),
                Transform::Normal,
                Fourcc::Abgr8888,
                std::iter::once(&expected),
            )
            .unwrap();
            let worst = pixels
                .iter()
                .zip(&reference)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap();
            assert!(
                worst <= 2,
                "scale={scale} ms={millis}: GLES mismatch {worst}"
            );
            // An opaque foreground covering another opaque surface still fades once.
            let index =
                (((26. * scale) as usize) * ((80. * scale) as usize) + (32. * scale) as usize) * 4;
            let alpha = (255. * native_open.progress() * 0.75).round() as u8;
            let expected = if millis >= 135 {
                [0, 0, alpha, alpha]
            } else {
                [0, alpha, 0, alpha]
            };
            for (a, b) in pixels[index..index + 4].iter().zip(expected) {
                assert!(
                    a.abs_diff(b) <= 2,
                    "scale={scale} ms={millis}: group fade {:?} expected {expected:?}",
                    &pixels[index..index + 4]
                );
            }
        }
    }
    println!("PASS: lightweight opening, cached source updates, negative decoration offsets, fractional scales and group opacity match GLES");
}

#[test]
#[ignore = "requires DRM, Vulkan and GLES"]
fn liquid_glass_matches_gles_preserves_masks_and_restores_state() {
    use std::sync::Arc;

    use smithay::backend::renderer::element::Kind;
    use smithay::backend::renderer::{ImportMem, RendererSuper};

    use crate::render_helpers::background_effect::RenderParams;
    use crate::render_helpers::framebuffer_effect::{FramebufferEffect, FramebufferEffectElement};
    use crate::render_helpers::solid_color::SolidColorRenderElement;
    use crate::render_helpers::texture::{TextureBuffer, TextureRenderElement};
    use crate::render_helpers::{render_to_vec, resources, shaders};
    use crate::utils::region::TransformedRegion;

    crate::clarion_render_elements! {
        GlassTestElement<R> => {
            Texture = TextureRenderElement<<R as RendererSuper>::TextureId>,
            Effect = FramebufferEffectElement,
            Solid = SolidColorRenderElement,
        }
    }
    let (vk, mut gles, _) = setup();
    resources::init(&mut gles);
    shaders::init(&mut gles);
    assert!(shaders::Shaders::get(&mut gles)
        .postprocess_and_clip
        .is_some());
    let mut native = native::NativeRenderer::new(vk).unwrap();
    // This fixture exercises prepared rendering; wait on the bounded test-only
    // preparer before its pixel, timing or ownership assertions begin.
    native
        .prepare_output_format_for_test(Fourcc::Abgr8888)
        .unwrap();
    let effect = FramebufferEffect::new();
    let pattern: Vec<u8> = (0..96 * 80)
        .flat_map(|i| {
            let x = i % 96;
            let y = i / 96;
            [
                ((x * 3) % 180 + 20) as u8,
                ((y * 3) % 180 + 20) as u8,
                ((x + y) % 180 + 20) as u8,
                255,
            ]
        })
        .collect();
    let native_texture = native
        .import_memory(&pattern, Fourcc::Abgr8888, (96, 80).into(), false)
        .unwrap();
    let gles_texture = gles
        .import_memory(&pattern, Fourcc::Abgr8888, (96, 80).into(), false)
        .unwrap();
    let native_buffer =
        TextureBuffer::from_texture(&native, native_texture, 1., Transform::Normal, vec![]);
    let gles_buffer =
        TextureBuffer::from_texture(&gles, gles_texture, 1., Transform::Normal, vec![]);
    for scale in [1., 1.25, 2.] {
        for transform in [Transform::Normal, Transform::_90, Transform::Flipped] {
            let size = ((96. * scale) as i32, (80. * scale) as i32).into();
            for location in [(-3., 8.), (12., 10.)] {
                let geometry = Rectangle::new(location.into(), (64., 52.).into());
                let make_effect = |glass| {
                    let element = effect.render(
                        None,
                        RenderParams {
                            geometry,
                            clip: Some((geometry, clarion_config::CornerRadius::from(8.))),
                            // Two independent bands: the gap must retain untouched background.
                            subregion: Some(TransformedRegion {
                                rects: Arc::new(vec![
                                    Rectangle::new((0, 0).into(), (64, 20).into()),
                                    Rectangle::new((0, 32).into(), (64, 20).into()),
                                ]),
                                scale: 1.0.into(),
                                offset: geometry.loc,
                            }),
                            scale,
                        },
                        None,
                        0.,
                        1.0,
                    );
                    if glass {
                        element.with_liquid_glass(0)
                    } else {
                        element
                    }
                };
                let mut outputs = Vec::new();
                for glass in [false, true, false] {
                    let native_elements: Vec<GlassTestElement<native::NativeRenderer>> = vec![
                        TextureRenderElement::from_texture_buffer(
                            native_buffer.clone(),
                            (0., 0.),
                            1.,
                            None,
                            None,
                            Kind::Unspecified,
                        )
                        .into(),
                        make_effect(glass).into(),
                    ];
                    let gles_elements: Vec<GlassTestElement<GlesRenderer>> = vec![
                        TextureRenderElement::from_texture_buffer(
                            gles_buffer.clone(),
                            (0., 0.),
                            1.,
                            None,
                            None,
                            Kind::Unspecified,
                        )
                        .into(),
                        make_effect(glass).into(),
                    ];
                    let pixels = render_to_vec(
                        &mut native,
                        size,
                        scale.into(),
                        transform,
                        Fourcc::Abgr8888,
                        native_elements.iter(),
                    )
                    .unwrap();
                    let reference = render_to_vec(
                        &mut gles,
                        size,
                        scale.into(),
                        transform,
                        Fourcc::Abgr8888,
                        gles_elements.iter(),
                    )
                    .unwrap();
                    let worst = pixels
                        .iter()
                        .zip(&reference)
                        .map(|(a, b)| a.abs_diff(*b))
                        .max()
                        .unwrap();
                    assert!(worst<=3,"glass={glass} scale={scale} transform={transform:?} loc={location:?}: mismatch {worst}");
                    outputs.push(pixels);
                }
                assert_eq!(
                    outputs[0], outputs[2],
                    "glass uniforms leaked into a later ordinary draw"
                );
                assert!(
                    outputs[0]
                        .iter()
                        .zip(&outputs[1])
                        .any(|(a, b)| a.abs_diff(*b) > 4),
                    "glass did not affect the scene"
                );
                if transform == Transform::Normal {
                    let x = (28. * scale) as usize;
                    let y = ((location.1 + 26.) * scale) as usize;
                    let index = (y * size.w as usize + x) * 4;
                    assert_eq!(
                        outputs[0][index..index + 4],
                        outputs[1][index..index + 4],
                        "effect crossed a mask gap"
                    );
                }
            }
        }
    }
    // Incremental live-frame capture is tested separately by the transition
    // cache and framebuffer-effect suite; no CPU screenshot loop is used here.
    println!("PASS: real liquid refraction, GLES/Vulkan parity, fractional/rotated/clipped geometry, mask gaps and non-glass state restoration");
}

#[test]
#[ignore = "requires real DRM render node and Vulkan"]
fn native_panel_glass_removal_repaints_old_bounds() {
    use smithay::backend::renderer::damage::OutputDamageTracker;
    use smithay::backend::renderer::element::Kind;

    use crate::render_helpers::background_effect::RenderParams;
    use crate::render_helpers::blur::BlurOptions;
    use crate::render_helpers::framebuffer_effect::{FramebufferEffect, FramebufferEffectElement};
    use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
    crate::clarion_render_elements! {
        PanelTestElement => {
            Solid = SolidColorRenderElement,
            Effect = FramebufferEffectElement,
        }
    }
    let (vk, mut oracle, mut allocator) = setup();
    let mods = modifiers(&vk, &oracle, Fourcc::Argb8888);
    let mut renderer = native::NativeRenderer::new(vk).unwrap();
    // This fixture exercises prepared rendering; wait on the bounded test-only
    // preparer before its pixel, timing or ownership assertions begin.
    renderer
        .prepare_output_format_for_test(Fourcc::Argb8888)
        .unwrap();
    let stripes: Vec<_> = (0..64)
        .map(|i| {
            let v = if i % 2 == 0 { 0.9 } else { 0.2 };
            SolidColorBuffer::new((2., 96.), [v, v, v, 1.])
        })
        .collect();
    // The launcher reuses part 0 when its full-screen mask shrinks to the
    // menu strip. Exercise that identity/size change at the desktop's 2x scale.
    for (launcher, scale) in [(false, 1.), (true, 1.), (true, 2.)] {
        let size = ((128. * scale) as i32, (96. * scale) as i32);
        for count in [1, 3] {
            for glass in [false, true] {
                let mut buffers: Vec<_> = (0..count)
                    .map(|_| {
                        allocator
                            .create_buffer(size.0 as u32, size.1 as u32, Fourcc::Argb8888, &mods)
                            .unwrap()
                            .export()
                            .unwrap()
                    })
                    .collect();
                let mut expected = allocator
                    .create_buffer(size.0 as u32, size.1 as u32, Fourcc::Argb8888, &mods)
                    .unwrap()
                    .export()
                    .unwrap();
                let mut tracker = OutputDamageTracker::new(size, scale, Transform::Normal);
                let mut full = OutputDamageTracker::new(size, scale, Transform::Normal);
                let mut effect = FramebufferEffect::new();
                let mut previous_panel = None;
                // Include repeated frames, closing, resize, reversal and all effects off.
                for (step, panel) in [
                    None,
                    Some(50.),
                    Some(50.),
                    None,
                    None,
                    Some(70.),
                    None,
                    Some(50.),
                    Some(60.),
                    None,
                    None,
                    None,
                ]
                .into_iter()
                .enumerate()
                {
                    if step == 0 || panel != previous_panel {
                        effect.damage();
                    }
                    previous_panel = panel;
                    let mut elements: Vec<PanelTestElement> = Vec::new();
                    if step != 10 && step != 11 {
                        let regions = if launcher {
                            vec![Rectangle::from_size(
                                (128., if panel.is_some() { 96. } else { 8. }).into(),
                            )]
                        } else {
                            std::iter::once(Rectangle::new((0., 0.).into(), (128., 8.).into()))
                                .chain(panel.map(|width| {
                                    Rectangle::new((32., 14.).into(), (width, 60.).into())
                                }))
                                .collect()
                        };
                        for (part, geometry) in regions.into_iter().enumerate() {
                            let elem = effect.render(
                                None,
                                RenderParams {
                                    geometry,
                                    subregion: None,
                                    clip: Some((
                                        geometry,
                                        clarion_config::CornerRadius::from(if part == 0 {
                                            0.
                                        } else {
                                            8.
                                        }),
                                    )),
                                    scale,
                                },
                                Some(BlurOptions {
                                    passes: 2,
                                    offset: 1.25,
                                }),
                                0.,
                                1.,
                            );
                            // Independently identified bar/panel captures, as on the Shell surface.
                            let elem = if glass {
                                elem.with_liquid_glass(part)
                            } else {
                                effect.render(
                                    Some(part),
                                    RenderParams {
                                        geometry,
                                        subregion: None,
                                        clip: None,
                                        scale,
                                    },
                                    Some(BlurOptions {
                                        passes: 2,
                                        offset: 1.25,
                                    }),
                                    0.,
                                    1.,
                                )
                            };
                            elements.push(elem.into());
                        }
                    }
                    elements.extend(stripes.iter().enumerate().map(|(i, stripe)| {
                        SolidColorRenderElement::from_buffer(
                            stripe,
                            (i as f64 * 2., 0.),
                            1.,
                            Kind::Unspecified,
                        )
                        .into()
                    }));
                    let target = &mut buffers[step % count];
                    let mut fb = renderer.bind(target).unwrap();
                    tracker
                        .render_output(
                            &mut renderer,
                            &mut fb,
                            if step < count { 0 } else { count },
                            &elements,
                            Color32F::TRANSPARENT,
                        )
                        .unwrap()
                        .sync
                        .wait()
                        .unwrap();
                    drop(fb);
                    let mut fb = renderer.bind(&mut expected).unwrap();
                    full.render_output(&mut renderer, &mut fb, 0, &elements, Color32F::TRANSPARENT)
                        .unwrap()
                        .sync
                        .wait()
                        .unwrap();
                    drop(fb);
                    let actual = pixels(&mut oracle, target);
                    if launcher && panel.is_some() {
                        let y = size.1 as usize / 2;
                        let row: Vec<_> = (16 * scale as usize..48 * scale as usize)
                            .map(|x| actual[(y * size.0 as usize + x) * 4])
                            .collect();
                        let range = row.iter().max().unwrap() - row.iter().min().unwrap();
                        assert!(
                            range < 60,
                            "fixture must actually blur the stripes, range={range}"
                        );
                    }
                    let wanted = pixels(&mut oracle, &mut expected);
                    let wrong = actual
                        .iter()
                        .zip(&wanted)
                        .filter(|(a, b)| a.abs_diff(**b) > 1)
                        .count();
                    assert_eq!(
                    wrong, 0,
                    "stale glass pixels: launcher={launcher}, scale={scale}, glass={glass}, buffers={count}, step={step}"
                );
                }
            }
        }
    }
    eprintln!("HARDWARE: closing/resizing/reversing bar and panel glass matches fresh output with one and three buffers");
}
