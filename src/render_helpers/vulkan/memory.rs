//! Native CPU-memory texture import. No EGL context or GLES upload is involved.
use super::{
    allocate_texture, device::VulkanContext, frame::color_range, VulkanRenderer, VulkanTexture,
};
use ash::vk;
use smithay::backend::allocator::Fourcc;
use smithay::utils::{Buffer, Rectangle};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};

// Includes cancelled or timed-out jobs whose GPU allocations are still live.
// The screenshot UI applies a smaller admission quota before submitting work.
const MAX_READBACK_JOBS: usize = 8;
const MAX_READBACK_BYTES: usize = 256 * 1024 * 1024;
static READBACK_JOBS: AtomicUsize = AtomicUsize::new(0);
static READBACK_BYTES: AtomicUsize = AtomicUsize::new(0);

struct ReadbackPermit {
    bytes: usize,
    _lifecycle: super::lifecycle::TaskLease,
}

impl ReadbackPermit {
    fn acquire(bytes: usize) -> Result<Self, vk::Result> {
        if bytes > MAX_READBACK_BYTES {
            return Err(vk::Result::ERROR_OUT_OF_HOST_MEMORY);
        }
        let lifecycle = super::lifecycle::TaskLease::acquire()
            .map_err(|_| vk::Result::ERROR_INITIALIZATION_FAILED)?;
        // Admit a readback only after its fixed retirement queue exists. A
        // cancelled job can then retire even after process admission closes.
        if retirement_sender().is_none() {
            return Err(vk::Result::ERROR_INITIALIZATION_FAILED);
        }
        READBACK_JOBS
            .fetch_update(Ordering::AcqRel, Ordering::Relaxed, |jobs| {
                (jobs < MAX_READBACK_JOBS).then_some(jobs + 1)
            })
            .map_err(|_| vk::Result::ERROR_OUT_OF_HOST_MEMORY)?;
        let mut permit = Self {
            bytes: 0,
            _lifecycle: lifecycle,
        };
        permit.grow_to(bytes)?;
        Ok(permit)
    }
    fn grow_to(&mut self, bytes: usize) -> Result<(), vk::Result> {
        let additional = bytes.saturating_sub(self.bytes);
        if additional == 0 {
            return Ok(());
        }
        READBACK_BYTES
            .fetch_update(Ordering::AcqRel, Ordering::Relaxed, |current| {
                current
                    .checked_add(additional)
                    .filter(|total| *total <= MAX_READBACK_BYTES)
            })
            .map_err(|_| vk::Result::ERROR_OUT_OF_HOST_MEMORY)?;
        self.bytes += additional;
        Ok(())
    }
}

impl Drop for ReadbackPermit {
    fn drop(&mut self) {
        READBACK_BYTES.fetch_sub(self.bytes, Ordering::AcqRel);
        READBACK_JOBS.fetch_sub(1, Ordering::AcqRel);
    }
}

struct RetiredReadback {
    copy: Upload,
    _permit: ReadbackPermit,
}

enum RetirementMessage {
    Work(RetiredReadback),
    Wake(super::lifecycle::Wake),
}
fn retirement_sender() -> Option<&'static Arc<mpsc::Sender<RetirementMessage>>> {
    static SENDER: super::lifecycle::ServiceCell<Arc<mpsc::Sender<RetirementMessage>>> =
        super::lifecycle::ServiceCell::new();
    SENDER
        .get_or_try_init(|| {
            let (sender, receiver) = mpsc::channel::<RetirementMessage>();
            let sender = Arc::new(sender);
            let wake = super::lifecycle::unbounded_queue_waker(&sender, RetirementMessage::Wake);
            super::lifecycle::spawn_worker("clarion-readback-retire", wake, move |stop| {
                fn retire(mut retired: RetiredReadback) {
                    let result = unsafe {
                        retired.copy.ctx.device.wait_for_fences(&[retired.copy.fence], true, u64::MAX)
                    };
                    if let Err(error) = result {
                        warn!(?error, "Vulkan readback retirement failed; retaining bounded in-flight GPU resources");
                        super::lifecycle::record_quarantine();
                        std::mem::forget(retired);
                    } else {
                        retired.copy.submitted = false;
                    }
                }
                while let Ok(message) = receiver.recv() {
                    match message {
                        RetirementMessage::Work(retired) => retire(retired),
                        RetirementMessage::Wake(wake) => {
                            drop(wake);
                            if stop.should_exit() {
                                for message in receiver.try_iter() {
                                    if let RetirementMessage::Work(retired) = message {
                                        retire(retired);
                                    }
                                }
                                break;
                            }
                        }
                    }
                }
            })?;
            Ok::<_, std::io::Error>(sender)
        })
        .ok()
}

/// Owned transfer resources; no renderer, Wayland client or mapped pointer is
/// sent to a worker. Vulkan's existing context/texture owners provide Send.
pub(super) struct PendingReadback {
    copy: Option<Upload>,
    permit: Option<ReadbackPermit>,
    size: usize,
}

impl std::fmt::Debug for PendingReadback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingReadback")
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

impl PendingReadback {
    pub(super) fn finish(mut self) -> Result<Vec<u8>, vk::Result> {
        let mut pixels = Vec::new();
        pixels
            .try_reserve_exact(self.size)
            .map_err(|_| vk::Result::ERROR_OUT_OF_HOST_MEMORY)?;
        pixels.resize(self.size, 0);
        self.copy_into(&mut pixels)?;
        Ok(pixels)
    }
    fn finish_into(mut self, dst: &mut [u8]) -> Result<(), vk::Result> {
        self.copy_into(dst)
    }

    fn copy_into(&mut self, dst: &mut [u8]) -> Result<(), vk::Result> {
        if dst.len() < self.size {
            return Err(vk::Result::ERROR_INITIALIZATION_FAILED);
        }
        let copy = self.copy.as_mut().unwrap();
        super::wait_for_fences(&copy.ctx.device, &[copy.fence])?;
        copy.submitted = false;
        // Staging allocations are HOST_VISIBLE | HOST_COHERENT. The transfer
        // submission includes a HOST_READ barrier; its fence orders CPU access.
        unsafe {
            let pointer = copy.ctx.device.map_memory(
                copy.memory(),
                0,
                vk::WHOLE_SIZE,
                vk::MemoryMapFlags::empty(),
            )? as *const u8;
            std::ptr::copy_nonoverlapping(pointer, dst.as_mut_ptr(), self.size);
            copy.ctx.device.unmap_memory(copy.memory());
        }
        Ok(())
    }
    #[cfg(test)]
    pub(super) fn is_ready(&self) -> Result<bool, vk::Result> {
        unsafe {
            self.copy
                .as_ref()
                .unwrap()
                .ctx
                .device
                .get_fence_status(self.copy.as_ref().unwrap().fence)
        }
    }
}

impl Drop for PendingReadback {
    fn drop(&mut self) {
        let Some(mut copy) = self.copy.take() else {
            return;
        };
        let permit = self.permit.take().unwrap();
        if !copy.submitted || unsafe { copy.ctx.device.get_fence_status(copy.fence) } == Ok(true) {
            copy.submitted = false;
            // No fence wait: completed resources return to command/staging pools.
            drop(copy);
            drop(permit);
        } else {
            let retired = RetiredReadback {
                copy,
                _permit: permit,
            };
            if let Some(sender) = retirement_sender() {
                if let Err(error) = sender.send(RetirementMessage::Work(retired)) {
                    warn!("Vulkan readback retirement worker unavailable; retaining bounded GPU resources");
                    super::lifecycle::record_quarantine();
                    std::mem::forget(error.0);
                }
            } else {
                warn!("Vulkan readback retirement worker unavailable; retaining bounded GPU resources");
                super::lifecycle::record_quarantine();
                std::mem::forget(retired);
            }
        }
    }
}

fn source_layout(width: u32, height: u32, stride: usize, len: usize) -> Result<usize, vk::Result> {
    let invalid = vk::Result::ERROR_INITIALIZATION_FAILED;
    if width == 0 || height == 0 || width > i32::MAX as u32 || height > i32::MAX as u32 {
        return Err(invalid);
    }
    let row = (width as usize).checked_mul(4).ok_or(invalid)?;
    let required = stride
        .checked_mul(height as usize - 1)
        .and_then(|n| n.checked_add(row))
        .ok_or(invalid)?;
    if stride < row || len < required {
        return Err(invalid);
    }
    Ok(row)
}

impl VulkanRenderer {
    /// Import premultiplied RGBA/BGRA pixels, respecting source and GPU row pitch.
    /// The texture owns a copy; the caller may immediately reuse its source buffer.
    /// A staging buffer copies pixels into an optimal-tiling device-local image.
    pub fn import_memory_texture(
        &self,
        data: &[u8],
        format: Fourcc,
        width: u32,
        height: u32,
        stride: usize,
    ) -> Result<VulkanTexture, vk::Result> {
        let texture = self.create_memory_texture(data, format, width, height, stride)?;
        self.update_memory_texture(
            &texture,
            data,
            stride,
            Rectangle::from_size((width as i32, height as i32).into()),
        )?;
        Ok(texture)
    }

    /// CPU-memory imports can be consumed by later submissions on this renderer's
    /// graphics queue without waiting for the upload on the CPU.
    pub(super) fn import_memory_texture_deferred(
        &self,
        data: &[u8],
        format: Fourcc,
        width: u32,
        height: u32,
        stride: usize,
    ) -> Result<(VulkanTexture, PendingUpload), vk::Result> {
        let texture = self.create_memory_texture(data, format, width, height, stride)?;
        let region = Rectangle::from_size((width as i32, height as i32).into());
        let upload = self
            .update_memory_texture_regions_impl(&texture, data, Some(stride), &[region], false)?
            .ok_or(vk::Result::ERROR_INITIALIZATION_FAILED)?;
        Ok((texture, upload))
    }

    fn create_memory_texture(
        &self,
        data: &[u8],
        format: Fourcc,
        width: u32,
        height: u32,
        stride: usize,
    ) -> Result<VulkanTexture, vk::Result> {
        source_layout(width, height, stride, data.len())?;
        let (format, alpha) = match format {
            Fourcc::Abgr8888 => (vk::Format::R8G8B8A8_UNORM, true),
            Fourcc::Xbgr8888 => (vk::Format::R8G8B8A8_UNORM, false),
            Fourcc::Argb8888 => (vk::Format::B8G8R8A8_UNORM, true),
            Fourcc::Xrgb8888 => (vk::Format::B8G8R8A8_UNORM, false),
            _ => return Err(vk::Result::ERROR_FORMAT_NOT_SUPPORTED),
        };
        let ctx = self
            .ctx
            .as_ref()
            .ok_or(vk::Result::ERROR_INITIALIZATION_FAILED)?;
        let props = unsafe {
            ctx.instance
                .get_physical_device_format_properties(ctx.physical_device, format)
        };
        if !props.optimal_tiling_features.contains(
            vk::FormatFeatureFlags::SAMPLED_IMAGE
                | vk::FormatFeatureFlags::SAMPLED_IMAGE_FILTER_LINEAR
                | vk::FormatFeatureFlags::TRANSFER_DST
                | vk::FormatFeatureFlags::TRANSFER_SRC,
        ) {
            return Err(vk::Result::ERROR_FORMAT_NOT_SUPPORTED);
        }
        let limits = unsafe {
            ctx.instance.get_physical_device_image_format_properties(
                ctx.physical_device,
                format,
                vk::ImageType::TYPE_2D,
                vk::ImageTiling::OPTIMAL,
                vk::ImageUsageFlags::SAMPLED
                    | vk::ImageUsageFlags::TRANSFER_DST
                    | vk::ImageUsageFlags::TRANSFER_SRC,
                vk::ImageCreateFlags::empty(),
            )?
        };
        if width > limits.max_extent.width || height > limits.max_extent.height {
            return Err(vk::Result::ERROR_FORMAT_NOT_SUPPORTED);
        }
        let texture = allocate_texture(
            ctx,
            width,
            height,
            format,
            vk::ImageTiling::OPTIMAL,
            vk::ImageUsageFlags::SAMPLED
                | vk::ImageUsageFlags::TRANSFER_DST
                | vk::ImageUsageFlags::TRANSFER_SRC,
            vk::ImageLayout::UNDEFINED,
            alpha,
        )?;
        Ok(texture)
    }
    /// Copy raw packed RGB image pixels into owned, tightly packed CPU memory.
    /// The caller must complete external producer synchronization before reading.
    pub fn read_memory_texture(
        &self,
        texture: &VulkanTexture,
        region: Rectangle<i32, Buffer>,
    ) -> Result<Vec<u8>, vk::Result> {
        self.enqueue_memory_readback(texture, region)?.finish()
    }

    /// Synchronous compatibility path for callers with preallocated memory.
    pub fn read_memory_texture_into(
        &self,
        texture: &VulkanTexture,
        region: Rectangle<i32, Buffer>,
        dst: &mut [u8],
    ) -> Result<(), vk::Result> {
        let size = self.readback_size(texture, region)?;
        if dst.len() < size {
            return Err(vk::Result::ERROR_INITIALIZATION_FAILED);
        }
        self.enqueue_memory_readback(texture, region)?
            .finish_into(dst)
    }

    fn readback_size(
        &self,
        texture: &VulkanTexture,
        region: Rectangle<i32, Buffer>,
    ) -> Result<usize, vk::Result> {
        let invalid = vk::Result::ERROR_INITIALIZATION_FAILED;
        let ctx = self.ctx.as_ref().ok_or(invalid)?;
        if !Arc::ptr_eq(ctx, &texture.inner.ctx)
            || !texture
                .inner
                .usage
                .contains(vk::ImageUsageFlags::TRANSFER_SRC)
            || region.loc.x < 0
            || region.loc.y < 0
            || region.size.w <= 0
            || region.size.h <= 0
            || i64::from(region.loc.x) + i64::from(region.size.w) > i64::from(texture.width)
            || i64::from(region.loc.y) + i64::from(region.size.h) > i64::from(texture.height)
            || texture.current_layout().get() == vk::ImageLayout::UNDEFINED
        {
            return Err(invalid);
        }
        if !matches!(
            vk::Format::from_raw(texture.format as i32),
            vk::Format::R8G8B8A8_UNORM
                | vk::Format::B8G8R8A8_UNORM
                | vk::Format::A2B10G10R10_UNORM_PACK32
                | vk::Format::A2R10G10B10_UNORM_PACK32
        ) {
            return Err(vk::Result::ERROR_FORMAT_NOT_SUPPORTED);
        }
        let size = (region.size.w as usize)
            .checked_mul(region.size.h as usize)
            .and_then(|n| n.checked_mul(4))
            .ok_or(invalid)?;
        Ok(size)
    }

    /// Record and submit the image transfer on this renderer's ordered queue.
    /// Completion, memory mapping and CPU copying belong to the returned Send
    /// job. External producers must already have been synchronized by the caller.
    pub(super) fn enqueue_memory_readback(
        &self,
        texture: &VulkanTexture,
        region: Rectangle<i32, Buffer>,
    ) -> Result<PendingReadback, vk::Result> {
        let size = self.readback_size(texture, region)?;
        let mut permit = ReadbackPermit::acquire(size)?;
        let _ = retirement_sender().ok_or(vk::Result::ERROR_INITIALIZATION_FAILED)?;
        let ctx = self.ctx.as_ref().unwrap();
        let mut copy = Upload::acquire(ctx, size, texture)?;
        permit.grow_to(copy.staging.as_ref().unwrap().size)?;
        let device = &ctx.device;
        {
            let _lock = ctx.command_lock.lock().unwrap();
            unsafe {
                device.begin_command_buffer(
                    copy.command,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )?;
                let external = texture.inner.external;
                let old = texture.current_layout().get();
                let barrier = vk::ImageMemoryBarrier::default()
                    .image(texture.image())
                    .subresource_range(color_range())
                    .old_layout(old)
                    .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                    .src_access_mask(if external {
                        vk::AccessFlags::empty()
                    } else {
                        vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE
                    })
                    .dst_access_mask(vk::AccessFlags::TRANSFER_READ)
                    .src_queue_family_index(if external {
                        vk::QUEUE_FAMILY_FOREIGN_EXT
                    } else {
                        vk::QUEUE_FAMILY_IGNORED
                    })
                    .dst_queue_family_index(if external {
                        ctx.graphics_queue_family
                    } else {
                        vk::QUEUE_FAMILY_IGNORED
                    });
                device.cmd_pipeline_barrier(
                    copy.command,
                    vk::PipelineStageFlags::ALL_COMMANDS,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[barrier],
                );
                let copy_region = vk::BufferImageCopy::default()
                    .image_subresource(
                        vk::ImageSubresourceLayers::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .layer_count(1),
                    )
                    .image_offset(vk::Offset3D {
                        x: region.loc.x,
                        y: region.loc.y,
                        z: 0,
                    })
                    .image_extent(vk::Extent3D {
                        width: region.size.w as u32,
                        height: region.size.h as u32,
                        depth: 1,
                    });
                device.cmd_copy_image_to_buffer(
                    copy.command,
                    texture.image(),
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    copy.buffer(),
                    &[copy_region],
                );
                let restored = barrier
                    .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                    .new_layout(old)
                    .src_access_mask(vk::AccessFlags::TRANSFER_READ)
                    .dst_access_mask(if external {
                        vk::AccessFlags::empty()
                    } else {
                        vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE
                    })
                    .src_queue_family_index(if external {
                        ctx.graphics_queue_family
                    } else {
                        vk::QUEUE_FAMILY_IGNORED
                    })
                    .dst_queue_family_index(if external {
                        vk::QUEUE_FAMILY_FOREIGN_EXT
                    } else {
                        vk::QUEUE_FAMILY_IGNORED
                    });
                let host = vk::BufferMemoryBarrier::default()
                    .buffer(copy.buffer())
                    .size(vk::WHOLE_SIZE)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .dst_access_mask(vk::AccessFlags::HOST_READ);
                device.cmd_pipeline_barrier(
                    copy.command,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::ALL_COMMANDS | vk::PipelineStageFlags::HOST,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[host],
                    &[restored],
                );
                device.end_command_buffer(copy.command)?;
                device.queue_submit(
                    ctx.graphics_queue,
                    &[vk::SubmitInfo::default().command_buffers(&[copy.command])],
                    copy.fence,
                )?;
                copy.submitted = true;
            }
        }
        Ok(PendingReadback {
            copy: Some(copy),
            permit: Some(permit),
            size,
        })
    }
    /// Update a region using full-image source coordinates. Shared texture handles
    /// retain their identity and immediately observe the completed GPU copy.
    pub fn update_memory_texture(
        &self,
        texture: &VulkanTexture,
        data: &[u8],
        stride: usize,
        region: Rectangle<i32, Buffer>,
    ) -> Result<(), vk::Result> {
        self.update_memory_texture_regions(texture, data, stride, &[region])
    }

    /// Snapshot a full-source-coordinate update and submit its copy without a
    /// CPU fence wait. Later draws and updates on this graphics queue observe it
    /// in submission order; the returned owner retains the staging allocation.
    pub(super) fn update_memory_texture_deferred(
        &self,
        texture: &VulkanTexture,
        data: &[u8],
        stride: usize,
        region: Rectangle<i32, Buffer>,
    ) -> Result<Option<PendingUpload>, vk::Result> {
        self.update_memory_texture_regions_impl(texture, data, Some(stride), &[region], false)
    }

    /// Upload disjoint regions in one submission and wait once for the whole batch.
    /// Validate the complete batch before modifying the texture.
    pub(super) fn update_memory_texture_regions(
        &self,
        texture: &VulkanTexture,
        data: &[u8],
        stride: usize,
        regions: &[Rectangle<i32, Buffer>],
    ) -> Result<(), vk::Result> {
        self.update_memory_texture_regions_impl(texture, data, Some(stride), regions, true)?;
        Ok(())
    }

    /// Synchronous packed upload for callers that need completed pixels before
    /// returning, including direct texture update tests.
    #[cfg(test)]
    pub(super) fn update_memory_texture_packed_regions(
        &self,
        texture: &VulkanTexture,
        data: &[u8],
        regions: &[Rectangle<i32, Buffer>],
    ) -> Result<(), vk::Result> {
        self.update_memory_texture_regions_impl(texture, data, None, regions, true)?;
        Ok(())
    }

    /// Upload a snapshot containing only the damaged rectangles, tightly packed
    /// in region order. This avoids a full-image CPU copy for small SHM damage.
    pub(super) fn update_memory_texture_packed_regions_deferred(
        &self,
        texture: &VulkanTexture,
        data: &[u8],
        regions: &[Rectangle<i32, Buffer>],
    ) -> Result<Option<PendingUpload>, vk::Result> {
        self.update_memory_texture_regions_impl(texture, data, None, regions, false)
    }

    fn update_memory_texture_regions_impl(
        &self,
        texture: &VulkanTexture,
        data: &[u8],
        source_stride: Option<usize>,
        regions: &[Rectangle<i32, Buffer>],
        wait: bool,
    ) -> Result<Option<PendingUpload>, vk::Result> {
        let invalid = vk::Result::ERROR_INITIALIZATION_FAILED;
        let ctx = self.ctx.as_ref().ok_or(invalid)?;
        if !Arc::ptr_eq(ctx, &texture.inner.ctx)
            || texture.inner.external
            || !texture
                .inner
                .usage
                .contains(vk::ImageUsageFlags::TRANSFER_DST)
            || !matches!(
                vk::Format::from_raw(texture.format as i32),
                vk::Format::R8G8B8A8_UNORM | vk::Format::B8G8R8A8_UNORM
            )
        {
            return Err(invalid);
        }
        if let Some(stride) = source_stride {
            source_layout(texture.width, texture.height, stride, data.len())?;
        }
        let mut size = 0usize;
        let mut copies = Vec::new();
        let mut nonempty = Vec::new();
        for region in regions {
            if region.loc.x < 0
                || region.loc.y < 0
                || region.size.w < 0
                || region.size.h < 0
                || i64::from(region.loc.x) + i64::from(region.size.w) > i64::from(texture.width)
                || i64::from(region.loc.y) + i64::from(region.size.h) > i64::from(texture.height)
            {
                return Err(invalid);
            }
            if region.size.is_empty() {
                continue;
            }
            // Vulkan copy regions must not overlap in destination memory.
            if nonempty
                .iter()
                .any(|other: &Rectangle<i32, Buffer>| other.overlaps(*region))
            {
                return Err(invalid);
            }
            copies.push(
                vk::BufferImageCopy::default()
                    .buffer_offset(size as u64)
                    .image_subresource(
                        vk::ImageSubresourceLayers::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .layer_count(1),
                    )
                    .image_offset(vk::Offset3D {
                        x: region.loc.x,
                        y: region.loc.y,
                        z: 0,
                    })
                    .image_extent(vk::Extent3D {
                        width: region.size.w as u32,
                        height: region.size.h as u32,
                        depth: 1,
                    }),
            );
            let bytes = (region.size.w as usize)
                .checked_mul(4)
                .and_then(|row| row.checked_mul(region.size.h as usize))
                .ok_or(invalid)?;
            size = size.checked_add(bytes).ok_or(invalid)?;
            nonempty.push(*region);
        }
        if source_stride.is_none() && data.len() != size {
            return Err(invalid);
        }
        if nonempty.is_empty() {
            return Ok(None);
        }
        let mut upload = Upload::acquire(ctx, size, texture)?;
        let device = &ctx.device;
        unsafe {
            let ptr = device.map_memory(
                upload.memory(),
                0,
                vk::WHOLE_SIZE,
                vk::MemoryMapFlags::empty(),
            )? as *mut u8;
            for (region, copy) in nonempty.iter().zip(&copies) {
                let row = region.size.w as usize * 4;
                for y in 0..region.size.h as usize {
                    let offset = if let Some(stride) = source_stride {
                        (region.loc.y as usize + y) * stride + region.loc.x as usize * 4
                    } else {
                        copy.buffer_offset as usize + y * row
                    };
                    std::ptr::copy_nonoverlapping(
                        data.as_ptr().add(offset),
                        ptr.add(copy.buffer_offset as usize + y * row),
                        row,
                    );
                }
            }
            device.unmap_memory(upload.memory());
        }
        {
            let _lock = ctx.command_lock.lock().unwrap();
            unsafe {
                device.begin_command_buffer(
                    upload.command,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )?;
                let old = texture.current_layout().get();
                let before = vk::ImageMemoryBarrier::default()
                    .image(texture.image())
                    .subresource_range(color_range())
                    .old_layout(old)
                    .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .src_access_mask(if old == vk::ImageLayout::UNDEFINED {
                        vk::AccessFlags::empty()
                    } else {
                        vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE
                    })
                    .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE);
                device.cmd_pipeline_barrier(
                    upload.command,
                    vk::PipelineStageFlags::ALL_COMMANDS,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[before],
                );
                device.cmd_copy_buffer_to_image(
                    upload.command,
                    upload.buffer(),
                    texture.image(),
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &copies,
                );
                let after = vk::ImageMemoryBarrier::default()
                    .image(texture.image())
                    .subresource_range(color_range())
                    .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .dst_access_mask(vk::AccessFlags::SHADER_READ);
                device.cmd_pipeline_barrier(
                    upload.command,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::FRAGMENT_SHADER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[after],
                );
                device.end_command_buffer(upload.command)?;
                device.queue_submit(
                    ctx.graphics_queue,
                    &[vk::SubmitInfo::default().command_buffers(&[upload.command])],
                    upload.fence,
                )?;
                upload.submitted = true;
                // A timeout does not roll back a successful GPU submission.
                texture
                    .current_layout()
                    .set(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
            }
        }
        if wait {
            super::wait_for_fences(device, &[upload.fence])?;
            upload.submitted = false;
        } else {
            return Ok(Some(upload.into_pending()));
        }
        Ok(None)
    }
}

/// Own partially initialized upload/download resources with pooled buffer/fences.
/// Command-pool allocation and cleanup take their own short command_lock;
/// callers hold it only while recording/submitting, never while dropping this
/// guard or waiting for its submitted fence.
struct Upload {
    ctx: Arc<VulkanContext>,
    texture: Option<VulkanTexture>,
    staging: Option<super::device::StagingBuffer>,
    command: vk::CommandBuffer,
    fence: vk::Fence,
    submitted: bool,
}
impl Upload {
    fn acquire(
        ctx: &Arc<VulkanContext>,
        size: usize,
        texture: &VulkanTexture,
    ) -> Result<Self, vk::Result> {
        let staging = ctx.acquire_staging_buffer(size)?;
        let command_fence = {
            let _lock = ctx.command_lock.lock().unwrap();
            ctx.acquire_command_fence()
        };
        let (command, fence) = match command_fence {
            Ok(command_fence) => command_fence,
            Err(err) => {
                ctx.release_staging_buffer(staging);
                return Err(err);
            }
        };
        Ok(Self {
            ctx: ctx.clone(),
            texture: Some(texture.clone()),
            staging: Some(staging),
            command,
            fence,
            submitted: false,
        })
    }

    fn buffer(&self) -> vk::Buffer {
        self.staging.as_ref().unwrap().buffer
    }

    fn memory(&self) -> vk::DeviceMemory {
        self.staging.as_ref().unwrap().memory
    }

    fn into_pending(mut self) -> PendingUpload {
        debug_assert!(self.submitted);
        self.submitted = false;
        PendingUpload {
            ctx: self.ctx.clone(),
            texture: self.texture.take(),
            staging: self.staging.take(),
            command: std::mem::replace(&mut self.command, vk::CommandBuffer::null()),
            fence: std::mem::replace(&mut self.fence, vk::Fence::null()),
        }
    }
}
impl Drop for Upload {
    fn drop(&mut self) {
        let device = &self.ctx.device;
        if self.submitted {
            if let Err(err) = super::wait_for_fences(device, &[self.fence]) {
                warn!(?err, "Vulkan transfer fence wait failed; retaining in-flight GPU resources until process exit");
                // The texture owns the context as well as the image. Leaking
                // only raw staging handles would still free both on unwinding.
                super::lifecycle::record_quarantine();
                std::mem::forget(self.texture.take());
                self.command = vk::CommandBuffer::null();
                self.fence = vk::Fence::null();
                self.staging = None;
                return;
            }
        }
        if self.command != vk::CommandBuffer::null() && self.fence != vk::Fence::null() {
            let _lock = self.ctx.command_lock.lock().unwrap();
            self.ctx.release_command_fence(self.command, self.fence);
        }
        if let Some(staging) = self.staging.take() {
            self.ctx.release_staging_buffer(staging);
        }
    }
}

/// Keeps an SHM upload's source image, staging memory and command alive until
/// the queue reaches its fence. Later scene draws use the same ordered queue.
pub(super) struct PendingUpload {
    ctx: Arc<VulkanContext>,
    texture: Option<VulkanTexture>,
    staging: Option<super::device::StagingBuffer>,
    command: vk::CommandBuffer,
    fence: vk::Fence,
}

impl std::fmt::Debug for PendingUpload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingUpload")
            .field("fence", &self.fence)
            .finish()
    }
}

impl PendingUpload {
    pub(super) fn wait_in_worker(&self) -> Result<(), vk::Result> {
        unsafe {
            self.ctx
                .device
                .wait_for_fences(&[self.fence], true, u64::MAX)
        }
    }

    pub(super) fn status(&self) -> Result<bool, vk::Result> {
        unsafe { self.ctx.device.get_fence_status(self.fence) }
    }

    pub(super) fn wait(&self) -> Result<(), vk::Result> {
        super::wait_for_fences(&self.ctx.device, &[self.fence])
    }
}

impl Drop for PendingUpload {
    fn drop(&mut self) {
        if let Err(err) = self.wait() {
            warn!(?err, "Vulkan SHM upload fence wait failed; retaining in-flight GPU resources until process exit");
            super::lifecycle::record_quarantine();
            std::mem::forget(self.texture.take());
            super::lifecycle::record_quarantine();
            std::mem::forget(self.ctx.clone());
            return;
        }
        let _lock = self.ctx.command_lock.lock().unwrap();
        self.ctx.release_command_fence(self.command, self.fence);
        if let Some(staging) = self.staging.take() {
            self.ctx.release_staging_buffer(staging);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn admission_bounds_jobs_and_staging_capacity_and_recovers_after_release() {
        let mut permits = Vec::new();
        for _ in 0..MAX_READBACK_JOBS {
            permits.push(ReadbackPermit::acquire(4).unwrap());
        }
        assert!(ReadbackPermit::acquire(4).is_err());
        drop(permits);
        let mut permit = ReadbackPermit::acquire(MAX_READBACK_BYTES).unwrap();
        assert!(ReadbackPermit::acquire(1).is_err());
        assert!(permit.grow_to(MAX_READBACK_BYTES + 1).is_err());
        drop(permit);
        assert!(ReadbackPermit::acquire(MAX_READBACK_BYTES + 1).is_err());
        assert!(ReadbackPermit::acquire(4).is_ok());
    }
    #[test]
    fn validates_padded_rows_without_requiring_last_row_padding() {
        assert_eq!(source_layout(3, 2, 16, 28).unwrap(), 12);
        assert!(source_layout(3, 2, 16, 27).is_err());
        assert!(source_layout(3, 2, 11, 100).is_err());
        assert!(source_layout(0, 2, 16, 32).is_err());
        assert!(source_layout(3, 0, 16, 32).is_err());
        assert!(source_layout(1, 3, usize::MAX, usize::MAX).is_err());
    }
}
