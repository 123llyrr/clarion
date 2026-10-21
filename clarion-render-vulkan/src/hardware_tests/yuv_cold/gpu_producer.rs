//! Proper GPU allocation and actual exported metadata, independent of DRM dumb storage.
use std::os::fd::{AsRawFd, FromRawFd};
use std::sync::Arc;

use ash::vk;

use super::*;
use crate::render_helpers::vulkan::VulkanContext;

struct GpuProducer {
    ctx: Arc<VulkanContext>,
    image: vk::Image,
    memory: vk::DeviceMemory,
    staging: vk::Buffer,
    staging_memory: vk::DeviceMemory,
    pool: vk::CommandPool,
    fence: vk::Fence,
    pending: bool,
    dmabuf: Option<Dmabuf>,
    offsets: [u32; 2],
    strides: [u32; 2],
    plane_count: u32,
    allocation_size: u64,
}
impl Drop for GpuProducer {
    fn drop(&mut self) {
        let device = &self.ctx.device;
        if self.pending
            && unsafe { device.wait_for_fences(&[self.fence], true, 5_000_000_000) }.is_err()
        {
            // A failed hardware fixture must not free referenced resources or
            // make its last context perform an unbounded teardown wait.
            std::mem::forget(self.ctx.clone());
            return;
        }
        unsafe {
            if self.pool != vk::CommandPool::null() {
                device.destroy_command_pool(self.pool, None);
            }
            if self.fence != vk::Fence::null() {
                device.destroy_fence(self.fence, None);
            }
            if self.staging != vk::Buffer::null() {
                device.destroy_buffer(self.staging, None);
            }
            if self.staging_memory != vk::DeviceMemory::null() {
                device.free_memory(self.staging_memory, None);
            }
            if self.image != vk::Image::null() {
                device.destroy_image(self.image, None);
            }
            if self.memory != vk::DeviceMemory::null() {
                device.free_memory(self.memory, None);
            }
        }
    }
}
fn memory_type(ctx: &VulkanContext, bits: u32, flags: vk::MemoryPropertyFlags) -> u32 {
    let props = unsafe {
        ctx.instance
            .get_physical_device_memory_properties(ctx.physical_device)
    };
    (0..props.memory_type_count)
        .find(|index| {
            bits & (1 << index) != 0
                && props.memory_types[*index as usize]
                    .property_flags
                    .contains(flags)
        })
        .unwrap()
}
impl GpuProducer {
    fn new(node: &DrmNode) -> Self {
        // A distinct Vulkan device prevents same-device image aliasing from
        // standing in for the external client-to-compositor import being tested.
        let ctx = VulkanContext::init(Some(node)).unwrap();
        let mut result = Self {
            ctx: ctx.clone(),
            image: vk::Image::null(),
            memory: vk::DeviceMemory::null(),
            staging: vk::Buffer::null(),
            staging_memory: vk::DeviceMemory::null(),
            pool: vk::CommandPool::null(),
            fence: vk::Fence::null(),
            pending: false,
            dmabuf: None,
            offsets: [0; 2],
            strides: [0; 2],
            plane_count: 0,
            allocation_size: 0,
        };
        let device = &ctx.device;
        let format = vk::Format::G8_B8R8_2PLANE_420_UNORM;
        let mut modifier_properties = vk::DrmFormatModifierPropertiesListEXT::default();
        let mut properties = vk::FormatProperties2::default().push_next(&mut modifier_properties);
        unsafe {
            ctx.instance.get_physical_device_format_properties2(
                ctx.physical_device,
                format,
                &mut properties,
            );
        }
        let mut modifiers = vec![
            vk::DrmFormatModifierPropertiesEXT::default();
            modifier_properties.drm_format_modifier_count as usize
        ];
        let mut modifier_properties = vk::DrmFormatModifierPropertiesListEXT::default()
            .drm_format_modifier_properties(&mut modifiers);
        let mut properties = vk::FormatProperties2::default().push_next(&mut modifier_properties);
        unsafe {
            ctx.instance.get_physical_device_format_properties2(
                ctx.physical_device,
                format,
                &mut properties,
            );
        }
        result.plane_count = modifiers
            .iter()
            .find(|modifier| modifier.drm_format_modifier == 0)
            .expect("producer must advertise LINEAR NV12")
            .drm_format_modifier_plane_count;
        assert_eq!(result.plane_count, 2);
        let usage = vk::ImageUsageFlags::SAMPLED
            | vk::ImageUsageFlags::TRANSFER_SRC
            | vk::ImageUsageFlags::TRANSFER_DST;
        let mut modifier_info = vk::PhysicalDeviceImageDrmFormatModifierInfoEXT::default()
            .drm_format_modifier(0)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let mut external_info = vk::PhysicalDeviceExternalImageFormatInfo::default()
            .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let query = vk::PhysicalDeviceImageFormatInfo2::default()
            .format(format)
            .ty(vk::ImageType::TYPE_2D)
            .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .usage(usage)
            .push_next(&mut modifier_info)
            .push_next(&mut external_info);
        let mut external = vk::ExternalImageFormatProperties::default();
        let mut properties = vk::ImageFormatProperties2::default().push_next(&mut external);
        unsafe {
            ctx.instance.get_physical_device_image_format_properties2(
                ctx.physical_device,
                &query,
                &mut properties,
            )
        }
        .unwrap();
        assert!(external
            .external_memory_properties
            .external_memory_features
            .contains(vk::ExternalMemoryFeatureFlags::EXPORTABLE));
        let modifiers = [0u64];
        let mut list =
            vk::ImageDrmFormatModifierListCreateInfoEXT::default().drm_format_modifiers(&modifiers);
        let mut external = vk::ExternalMemoryImageCreateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D {
                width: 16,
                height: 16,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .push_next(&mut list)
            .push_next(&mut external);
        result.image = unsafe { device.create_image(&image_info, None) }.unwrap();
        let modifier_api = ash::ext::image_drm_format_modifier::Device::new(&ctx.instance, device);
        let mut selected = vk::ImageDrmFormatModifierPropertiesEXT::default();
        unsafe {
            modifier_api.get_image_drm_format_modifier_properties(result.image, &mut selected)
        }
        .unwrap();
        assert_eq!(selected.drm_format_modifier, 0);
        for (index, aspect) in [
            vk::ImageAspectFlags::MEMORY_PLANE_0_EXT,
            vk::ImageAspectFlags::MEMORY_PLANE_1_EXT,
        ]
        .into_iter()
        .enumerate()
        {
            let layout = unsafe {
                device.get_image_subresource_layout(
                    result.image,
                    vk::ImageSubresource::default().aspect_mask(aspect),
                )
            };
            result.offsets[index] = layout.offset.try_into().unwrap();
            result.strides[index] = layout.row_pitch.try_into().unwrap();
        }
        let requirements = unsafe { device.get_image_memory_requirements(result.image) };
        result.allocation_size = requirements.size;
        let mut export = vk::ExportMemoryAllocateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(result.image);
        let alloc = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_type(
                &ctx,
                requirements.memory_type_bits,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
            ))
            .push_next(&mut export)
            .push_next(&mut dedicated);
        result.memory = unsafe { device.allocate_memory(&alloc, None) }.unwrap();
        unsafe { device.bind_image_memory(result.image, result.memory, 0) }.unwrap();
        result.staging = unsafe {
            device.create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(384)
                    .usage(vk::BufferUsageFlags::TRANSFER_SRC | vk::BufferUsageFlags::TRANSFER_DST)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
        }
        .unwrap();
        let requirements = unsafe { device.get_buffer_memory_requirements(result.staging) };
        result.staging_memory = unsafe {
            device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(requirements.size)
                    .memory_type_index(memory_type(
                        &ctx,
                        requirements.memory_type_bits,
                        vk::MemoryPropertyFlags::HOST_VISIBLE
                            | vk::MemoryPropertyFlags::HOST_COHERENT,
                    )),
                None,
            )
        }
        .unwrap();
        unsafe { device.bind_buffer_memory(result.staging, result.staging_memory, 0) }.unwrap();
        let mut expected = vec![0u8; 384];
        for row in 0..16 {
            for x in 0..16 {
                expected[row * 16 + x] = if x < 8 { 81 } else { 41 };
            }
        }
        for row in 0..8 {
            for x in (0..16).step_by(2) {
                let (u, v) = if x < 8 { (90, 240) } else { (240, 110) };
                expected[256 + row * 16 + x] = u;
                expected[256 + row * 16 + x + 1] = v;
            }
        }
        unsafe {
            let mapped = device
                .map_memory(result.staging_memory, 0, 384, vk::MemoryMapFlags::empty())
                .unwrap();
            std::ptr::copy_nonoverlapping(expected.as_ptr(), mapped.cast::<u8>(), expected.len());
            device.unmap_memory(result.staging_memory);
        }
        result.pool = unsafe {
            device.create_command_pool(
                &vk::CommandPoolCreateInfo::default().queue_family_index(ctx.graphics_queue_family),
                None,
            )
        }
        .unwrap();
        let command = unsafe {
            device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(result.pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )
        }
        .unwrap()[0];
        result.fence =
            unsafe { device.create_fence(&vk::FenceCreateInfo::default(), None) }.unwrap();
        let color = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(1);
        let regions = [
            vk::BufferImageCopy::default()
                .buffer_offset(0)
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::PLANE_0)
                        .layer_count(1),
                )
                .image_extent(vk::Extent3D {
                    width: 16,
                    height: 16,
                    depth: 1,
                }),
            vk::BufferImageCopy::default()
                .buffer_offset(256)
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::PLANE_1)
                        .layer_count(1),
                )
                .image_extent(vk::Extent3D {
                    width: 8,
                    height: 8,
                    depth: 1,
                }),
        ];
        unsafe {
            device
                .begin_command_buffer(
                    command,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .unwrap();
            let initial = [vk::ImageMemoryBarrier::default()
                .image(result.image)
                .subresource_range(color)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)];
            device.cmd_pipeline_barrier(
                command,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &initial,
            );
            device.cmd_copy_buffer_to_image(
                command,
                result.staging,
                result.image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &regions,
            );
            let readable = [vk::ImageMemoryBarrier::default()
                .image(result.image)
                .subresource_range(color)
                .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::TRANSFER_READ)];
            let writable = [vk::BufferMemoryBarrier::default()
                .buffer(result.staging)
                .offset(0)
                .size(384)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .src_access_mask(vk::AccessFlags::TRANSFER_READ)
                .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)];
            device.cmd_pipeline_barrier(
                command,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &writable,
                &readable,
            );
            device.cmd_copy_image_to_buffer(
                command,
                result.image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                result.staging,
                &regions,
            );
            let host = [vk::BufferMemoryBarrier::default()
                .buffer(result.staging)
                .offset(0)
                .size(384)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::HOST_READ)];
            device.cmd_pipeline_barrier(
                command,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::HOST,
                vk::DependencyFlags::empty(),
                &[],
                &host,
                &[],
            );
            let release = [vk::ImageMemoryBarrier::default()
                .image(result.image)
                .subresource_range(color)
                .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .new_layout(vk::ImageLayout::GENERAL)
                .src_queue_family_index(ctx.graphics_queue_family)
                .dst_queue_family_index(vk::QUEUE_FAMILY_FOREIGN_EXT)
                .src_access_mask(vk::AccessFlags::TRANSFER_READ)];
            device.cmd_pipeline_barrier(
                command,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &release,
            );
            device.end_command_buffer(command).unwrap();
            device
                .queue_submit(
                    ctx.graphics_queue,
                    &[vk::SubmitInfo::default().command_buffers(&[command])],
                    result.fence,
                )
                .unwrap();
            result.pending = true;
            device
                .wait_for_fences(&[result.fence], true, 5_000_000_000)
                .unwrap();
            result.pending = false;
            let mapped = device
                .map_memory(result.staging_memory, 0, 384, vk::MemoryMapFlags::empty())
                .unwrap();
            assert_eq!(
                std::slice::from_raw_parts(mapped.cast::<u8>(), 384),
                expected,
                "producer GPU upload/readback must verify both full planes before export"
            );
            device.unmap_memory(result.staging_memory);
        }
        let api = ash::khr::external_memory_fd::Device::new(&ctx.instance, device);
        let raw = unsafe {
            api.get_memory_fd(
                &vk::MemoryGetFdInfoKHR::default()
                    .memory(result.memory)
                    .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT),
            )
        }
        .unwrap();
        assert!(raw >= 0);
        let fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(raw) };
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        assert_eq!(unsafe { libc::fstat(fd.as_raw_fd(), stat.as_mut_ptr()) }, 0);
        let backing_bytes = unsafe { stat.assume_init() }.st_size;
        assert!(backing_bytes >= result.allocation_size as i64);
        let fdinfo =
            std::fs::read_to_string(format!("/proc/self/fdinfo/{}", fd.as_raw_fd())).unwrap();
        let exporter = fdinfo
            .lines()
            .find_map(|line| line.strip_prefix("exp_name:").map(str::trim));
        let mut builder = Dmabuf::builder(
            (16, 16),
            Fourcc::Nv12,
            Modifier::Linear,
            DmabufFlags::empty(),
        );
        for index in 0..2 {
            builder.add_plane(
                fd.try_clone().unwrap(),
                result.offsets[index],
                result.strides[index],
            );
        }
        result.dmabuf = Some(builder.build().unwrap());
        eprintln!(
            "YUV_GPU_PRODUCER_METADATA {}",
            serde_json::json!({
                "producer_kind": "vulkan_modifier_list_gpu_upload", "producer_upload_verified": true,
                "visible_size": [16,16], "modifier": 0, "plane_offsets": result.offsets,
                "plane_strides": result.strides, "allocation_bytes": result.allocation_size,
                "plane_count": result.plane_count, "backing_bytes": backing_bytes, "exporter": exporter,
                "producer_device": format!("{:?}", device.handle()), "separate_consumer_device_required": true,
            })
        );
        result
    }
}

#[test]
#[ignore = "requires selected GPU supporting exported GPU-produced LINEAR NV12"]
fn native_gpu_produced_nv12_cold_import_and_compatibility_pixels() {
    check_gpu_native_nv12(false);
}
#[test]
#[ignore = "requires selected GPU supporting exported GPU-produced LINEAR NV12"]
fn native_gpu_produced_nv12_cold_preparation_defers_without_owner_wait_or_overwrite() {
    check_gpu_native_nv12(true);
}

fn check_gpu_native_nv12(gate_preparation: bool) {
    let (vk, mut oracle, mut allocator) = setup();
    let mut renderer = native::NativeRenderer::new(vk).unwrap();
    let node = DrmNode::from_path(std::env::var_os("CLARION_VULKAN_TEST_NODE").unwrap()).unwrap();
    let producer = GpuProducer::new(&node);
    assert_ne!(
        producer.ctx.device.handle(),
        renderer.vulkan().ctx.as_ref().unwrap().device.handle(),
        "external import must cross independent Vulkan devices"
    );
    let source = producer.dmabuf.as_ref().unwrap().clone();
    assert!(renderer.dmabuf_formats().contains(&source.format()));
    let primary = "independent Vulkan GPU producer on selected DRM device";
    let pitch = producer.strides[0];
    let backing_bytes = producer.allocation_size;
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
        let before_waits = crate::render_helpers::vulkan::owner_fence_wait_calls_for_test();
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
            crate::render_helpers::vulkan::owner_fence_wait_calls_for_test(),
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
        {
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
        }
        let bytes = pixels(&mut oracle, &mut target);
        assert_nv12_pixels(&bytes, 0);
        eprintln!(
            "YUV_GATED_PREPARATION_RESULT {}",
            serde_json::json!({
                "status": "PASS", "producer_kind": "vulkan_modifier_list_gpu_upload",
                "producer_upload_verified": true, "independent_vulkan_devices": true,
                "plane_offsets": producer.offsets, "plane_strides": producer.strides, "node": std::env::var("CLARION_VULKAN_TEST_NODE").ok(),
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
            "status": "PASS", "producer_kind": "vulkan_modifier_list_gpu_upload",
                "producer_upload_verified": true, "independent_vulkan_devices": true,
                "plane_offsets": producer.offsets, "plane_strides": producer.strides, "pixel_checks": 9, "cold_iterations": 1, "warm_iterations": 8,
            "node": std::env::var("CLARION_VULKAN_TEST_NODE").ok(),
            "producer": primary, "renderer_import_cache_cold": true, "driver_cache_cleared": false,
        "producer_visible_size": [16, 16], "pitch": pitch,
        "uv_offset": producer.offsets[1], "backing_bytes": backing_bytes,
        "normal_wayland_submit_verified": false,
        "import_cache_cold_on_iteration_zero": true,
        "yuv_target_pipeline_prepared_before_measured_draw": true,
        "finish_may_wait_for_unregistered_external_texture": true,
        "scope": "native import and raw DMA-BUF compatibility render, including any finish GPU wait; subsequent sync wait and GLES readback reported separately"
        })
    );
}
