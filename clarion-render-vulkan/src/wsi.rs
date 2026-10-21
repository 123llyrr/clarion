//! Native Vulkan presentation for a nested Winit window.
//!
//! Smithay's WinitGraphicsBackend always creates an EGL surface. This path
//! instead copies the completed Vulkan composition target into a KHR swapchain
//! on the same device and queue that rendered it.

use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _};
use ash::vk;
use smithay::backend::allocator::Fourcc;
use smithay::reexports::winit::window::Window;

use super::device::{VulkanContext, VulkanSurfaceHandle};
use super::VulkanTexture;

struct Swapchain {
    handle: vk::SwapchainKHR,
    images: Vec<vk::Image>,
    extent: vk::Extent2D,
    format: vk::Format,
    // A present wait semaphore is tied to the image, not the CPU frame. It is
    // safe to reuse only after that image is acquired again.
    present_semaphores: Vec<vk::Semaphore>,
    // swapchainMaintenance1 fences also prove presentation has stopped using
    // the semaphores and swapchain when resize or shutdown retires every image.
    present_fences: Vec<vk::Fence>,
    present_pending: Vec<bool>,
}

struct FrameSlot {
    command: vk::CommandBuffer,
    fence: vk::Fence,
    acquire_semaphore: vk::Semaphore,
    in_flight: bool,
    source: Option<VulkanTexture>,
}

/// A temporary host/GPU wait is not a renderer error and must preserve the target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentResult {
    Presented,
    Pending,
    Recreate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AcquiredImage {
    slot: usize,
    index: u32,
    suboptimal: bool,
    acquire_ready: bool,
}

/// The image and its signaled acquire semaphore stay owned while its old
/// presentation fence is pending. Never acquire another image in that state.
#[derive(Default)]
struct Acquisition {
    image: Option<AcquiredImage>,
}

impl Acquisition {
    fn get_or_acquire(
        &mut self,
        slot: usize,
        acquire: impl FnOnce() -> Result<(u32, bool), vk::Result>,
    ) -> Result<Option<AcquiredImage>, vk::Result> {
        if let Some(image) = self.image {
            assert_eq!(image.slot, slot);
            return Ok(Some(image));
        }
        match acquire() {
            Ok((index, suboptimal)) => {
                let image = AcquiredImage {
                    slot,
                    index,
                    suboptimal,
                    acquire_ready: false,
                };
                self.image = Some(image);
                Ok(Some(image))
            }
            Err(vk::Result::NOT_READY | vk::Result::TIMEOUT) => Ok(None),
            Err(err) => Err(err),
        }
    }

    fn mark_ready(&mut self) {
        self.image.as_mut().unwrap().acquire_ready = true;
    }

    fn submitted(&mut self) {
        self.image = None;
    }
}

struct CopySync {
    acquire: vk::Semaphore,
    copied: vk::Semaphore,
    fence: vk::Fence,
}

/// Owns the surface, swapchain, and binary semaphores. The Winit window and
/// display connection must outlive this object.
pub struct VulkanWsi {
    ctx: Arc<VulkanContext>,
    surface_api: ash::khr::surface::Instance,
    swapchain_api: ash::khr::swapchain::Device,
    surface: vk::SurfaceKHR,
    swapchain: Option<Swapchain>,
    requested_size: Option<(u32, u32)>,
    slots: Vec<FrameSlot>,
    next_slot: usize,
    acquisition: Acquisition,
    presented_frames: u64,
    generation: u64,
    presented_generation: u64,
}

impl VulkanWsi {
    fn wait_for_queue_idle(&self, context: &'static str) -> anyhow::Result<()> {
        // `vkQueueWaitIdle` has the same queue-level host synchronization
        // requirement as queue submission.  Keep it under the context lock so
        // a concurrent renderer submission cannot race this retirement path.
        let _queue_lock = self.ctx.command_lock.lock().unwrap();
        unsafe { self.ctx.device.queue_wait_idle(self.ctx.graphics_queue) }.context(context)
    }

    fn destroy_swapchain(&self, swapchain: Swapchain) {
        unsafe {
            for semaphore in swapchain.present_semaphores {
                self.ctx.device.destroy_semaphore(semaphore, None);
            }
            for fence in swapchain.present_fences {
                if fence != vk::Fence::null() {
                    self.ctx.device.destroy_fence(fence, None);
                }
            }
            self.swapchain_api.destroy_swapchain(swapchain.handle, None);
        }
    }

    fn discard_retired_swapchain(&mut self) {
        // Passing oldSwapchain to vkCreateSwapchainKHR retires it even when
        // creation fails. Never reuse that handle on the next resize attempt.
        if let Some(old) = self.swapchain.take() {
            self.destroy_swapchain(old);
        }
        self.requested_size = None;
    }

    pub fn new(
        ctx: Arc<VulkanContext>,
        handle: VulkanSurfaceHandle,
        width: u32,
        height: u32,
    ) -> anyhow::Result<Self> {
        let surface_api = ash::khr::surface::Instance::new(&ctx.entry, &ctx.instance);
        let swapchain_api = ash::khr::swapchain::Device::new(&ctx.instance, &ctx.device);
        // SAFETY: the caller retains the Winit window and connection.
        let surface = unsafe { handle.create_surface(&ctx.entry, &ctx.instance) }
            .context("creating Vulkan host surface")?;
        let mut this = Self {
            ctx,
            surface_api,
            swapchain_api,
            surface,
            swapchain: None,
            requested_size: None,
            slots: Vec::new(),
            next_slot: 0,
            acquisition: Acquisition::default(),
            presented_frames: 0,
            generation: 0,
            presented_generation: 0,
        };
        for _ in 0..2 {
            this.slots.push(this.new_slot()?);
        }
        if width > 0 && height > 0 {
            this.resize(width, height)?;
        }
        Ok(this)
    }

    fn new_slot(&self) -> anyhow::Result<FrameSlot> {
        let _command_lock = self.ctx.command_lock.lock().unwrap();
        let device = &self.ctx.device;
        let command = unsafe {
            device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(self.ctx.command_pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )
        }
        .context("allocating Vulkan WSI command buffer")?[0];
        let fence = match unsafe {
            device.create_fence(
                &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
                None,
            )
        } {
            Ok(fence) => fence,
            Err(err) => {
                unsafe { device.free_command_buffers(self.ctx.command_pool, &[command]) };
                return Err(err).context("creating Vulkan WSI fence");
            }
        };
        let acquire_semaphore =
            match unsafe { device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None) } {
                Ok(semaphore) => semaphore,
                Err(err) => {
                    unsafe {
                        device.destroy_fence(fence, None);
                        device.free_command_buffers(self.ctx.command_pool, &[command]);
                    }
                    return Err(err).context("creating Vulkan WSI acquire semaphore");
                }
            };
        Ok(FrameSlot {
            command,
            fence,
            acquire_semaphore,
            in_flight: false,
            source: None,
        })
    }

    pub fn format(&self) -> Option<Fourcc> {
        match self.swapchain.as_ref()?.format {
            vk::Format::B8G8R8A8_UNORM => Some(Fourcc::Argb8888),
            vk::Format::R8G8B8A8_UNORM => Some(Fourcc::Abgr8888),
            _ => None,
        }
    }

    pub fn extent(&self) -> Option<vk::Extent2D> {
        self.swapchain.as_ref().map(|swapchain| swapchain.extent)
    }

    pub fn has_acquired_image(&self) -> bool {
        self.acquisition.image.is_some()
    }

    pub fn requested_size(&self) -> Option<(u32, u32)> {
        self.requested_size
    }

    pub fn resize(&mut self, width: u32, height: u32) -> anyhow::Result<bool> {
        if width == 0 || height == 0 || self.acquisition.image.is_some() {
            return Ok(false);
        }
        // Do not wait on the compositor's input loop while an old composition
        // or host presentation still owns the swapchain. Keep every owner until
        // the paced retry observes completion.
        for slot in self.slots.iter().filter(|slot| slot.in_flight) {
            if !unsafe { self.ctx.device.get_fence_status(slot.fence) }
                .context("checking old Vulkan swapchain copy")?
            {
                return Ok(false);
            }
        }
        if self.ctx.supports_swapchain_maintenance {
            if let Some(old) = &self.swapchain {
                for (&fence, &pending) in old.present_fences.iter().zip(&old.present_pending) {
                    if pending
                        && !unsafe { self.ctx.device.get_fence_status(fence) }
                            .context("checking old Vulkan swapchain presentation")?
                    {
                        return Ok(false);
                    }
                }
            }
        }
        let capabilities = unsafe {
            self.surface_api
                .get_physical_device_surface_capabilities(self.ctx.physical_device, self.surface)
        }
        .context("querying host surface capabilities")?;
        if !capabilities
            .supported_usage_flags
            .contains(vk::ImageUsageFlags::TRANSFER_DST)
        {
            bail!("host Vulkan swapchain does not support transfer destination images");
        }
        let extent = if capabilities.current_extent.width != u32::MAX {
            capabilities.current_extent
        } else {
            vk::Extent2D {
                width: width.clamp(
                    capabilities.min_image_extent.width,
                    capabilities.max_image_extent.width,
                ),
                height: height.clamp(
                    capabilities.min_image_extent.height,
                    capabilities.max_image_extent.height,
                ),
            }
        };
        if extent.width == 0 || extent.height == 0 {
            return Ok(false);
        }
        let formats = unsafe {
            self.surface_api
                .get_physical_device_surface_formats(self.ctx.physical_device, self.surface)
        }
        .context("querying host surface formats")?;
        let format = formats
            .iter()
            .find(|format| {
                format.format == vk::Format::B8G8R8A8_UNORM
                    && format.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
            })
            .or_else(|| {
                formats.iter().find(|format| {
                    format.format == vk::Format::R8G8B8A8_UNORM
                        && format.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
                })
            })
            .copied()
            .or_else(|| {
                formats
                    .iter()
                    .find(|format| format.format == vk::Format::UNDEFINED)
                    .map(|format| vk::SurfaceFormatKHR {
                        format: vk::Format::B8G8R8A8_UNORM,
                        color_space: format.color_space,
                    })
            })
            .ok_or_else(|| anyhow!("host Vulkan surface has no RGBA8 UNORM swapchain format"))?;
        let alpha = [
            vk::CompositeAlphaFlagsKHR::OPAQUE,
            vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED,
            vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED,
            vk::CompositeAlphaFlagsKHR::INHERIT,
        ]
        .into_iter()
        .find(|flag| capabilities.supported_composite_alpha.contains(*flag))
        .ok_or_else(|| anyhow!("host Vulkan surface has no supported composite alpha mode"))?;
        let image_count = if capabilities.max_image_count == 0 {
            capabilities.min_image_count.saturating_add(1)
        } else {
            capabilities
                .min_image_count
                .saturating_add(1)
                .min(capabilities.max_image_count)
        };
        let old = self
            .swapchain
            .as_ref()
            .map_or(vk::SwapchainKHR::null(), |old| old.handle);
        // Drivers without swapchainMaintenance1 have no presentation fence.
        // Keep the legacy queue-idle compatibility fallback for resize and
        // shutdown; it completes queue submissions but cannot formally prove
        // the presentation engine has released its semaphore/swapchain users.
        // Maintenance1 present fences provide that guarantee. Steady-state
        // presentation below never waits for queue idle.
        if !self.ctx.supports_swapchain_maintenance && self.swapchain.is_some() {
            self.wait_for_queue_idle("waiting for old Vulkan swapchain queue")?;
        }
        for slot in &mut self.slots {
            slot.in_flight = false;
            slot.source = None;
        }
        let info = vk::SwapchainCreateInfoKHR::default()
            .surface(self.surface)
            .min_image_count(image_count)
            .image_format(format.format)
            .image_color_space(format.color_space)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(vk::ImageUsageFlags::TRANSFER_DST)
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .pre_transform(capabilities.current_transform)
            .composite_alpha(alpha)
            .present_mode(vk::PresentModeKHR::FIFO)
            .clipped(true)
            .old_swapchain(old);
        let handle = match unsafe { self.swapchain_api.create_swapchain(&info, None) } {
            Ok(handle) => handle,
            Err(err) => {
                self.discard_retired_swapchain();
                return Err(err).context("creating host Vulkan swapchain");
            }
        };
        let images = match unsafe { self.swapchain_api.get_swapchain_images(handle) } {
            Ok(images) => images,
            Err(err) => {
                unsafe { self.swapchain_api.destroy_swapchain(handle, None) };
                self.discard_retired_swapchain();
                return Err(err).context("querying Vulkan swapchain images");
            }
        };
        let mut present_semaphores = Vec::with_capacity(images.len());
        let mut present_fences = Vec::with_capacity(images.len());
        for _ in &images {
            let semaphore = match unsafe {
                self.ctx
                    .device
                    .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
            } {
                Ok(semaphore) => semaphore,
                Err(err) => {
                    unsafe {
                        for semaphore in present_semaphores {
                            self.ctx.device.destroy_semaphore(semaphore, None);
                        }
                        for fence in present_fences {
                            self.ctx.device.destroy_fence(fence, None);
                        }
                        self.swapchain_api.destroy_swapchain(handle, None);
                    }
                    self.discard_retired_swapchain();
                    return Err(err).context("creating per-image Vulkan present semaphore");
                }
            };
            let fence = if self.ctx.supports_swapchain_maintenance {
                match unsafe {
                    self.ctx
                        .device
                        .create_fence(&vk::FenceCreateInfo::default(), None)
                } {
                    Ok(fence) => fence,
                    Err(err) => {
                        unsafe {
                            self.ctx.device.destroy_semaphore(semaphore, None);
                            for semaphore in present_semaphores {
                                self.ctx.device.destroy_semaphore(semaphore, None);
                            }
                            for fence in present_fences {
                                if fence != vk::Fence::null() {
                                    self.ctx.device.destroy_fence(fence, None);
                                }
                            }
                            self.swapchain_api.destroy_swapchain(handle, None);
                        }
                        self.discard_retired_swapchain();
                        return Err(err).context("creating per-image Vulkan present fence");
                    }
                }
            } else {
                vk::Fence::null()
            };
            present_semaphores.push(semaphore);
            present_fences.push(fence);
        }
        let present_pending = vec![false; images.len()];
        if let Some(old) = self.swapchain.replace(Swapchain {
            handle,
            images,
            extent,
            format: format.format,
            present_semaphores,
            present_fences,
            present_pending,
        }) {
            self.destroy_swapchain(old);
        }
        self.requested_size = Some((width, height));
        self.generation = self.generation.wrapping_add(1);
        debug!(
            "nested Vulkan swapchain ready: {}x{}, {:?}, {} images",
            extent.width,
            extent.height,
            format.format,
            self.swapchain.as_ref().unwrap().images.len(),
        );
        Ok(true)
    }

    /// Copy a submitted target and present it through Vulkan WSI. The source
    /// renderer uses the same graphics queue, so its submission precedes this
    /// copy without a CPU fence wait. The frame slot retains the target until
    /// its copy fence signals.
    pub fn present_from(
        &mut self,
        source: &VulkanTexture,
        window: &dyn Window,
    ) -> anyhow::Result<PresentResult> {
        if self.requested_size.is_none() && self.acquisition.image.is_none() {
            return Ok(PresentResult::Recreate);
        }
        let swapchain = self
            .swapchain
            .as_ref()
            .context("Vulkan swapchain is not ready")?;
        if !Arc::ptr_eq(&self.ctx, &source.inner.ctx)
            || source.width != swapchain.extent.width
            || source.height != swapchain.extent.height
            || vk::Format::from_raw(source.format as i32) != swapchain.format
        {
            bail!("Vulkan WSI target format, device or dimensions differ from swapchain");
        }
        let slot_index = self.next_slot;
        if self.slots[slot_index].in_flight {
            if !unsafe {
                self.ctx
                    .device
                    .get_fence_status(self.slots[slot_index].fence)
            }
            .context("checking previous Vulkan WSI frame")?
            {
                return Ok(PresentResult::Pending);
            }
            self.slots[slot_index].in_flight = false;
            self.slots[slot_index].source = None;
        }
        let command = self.slots[slot_index].command;
        let fence = self.slots[slot_index].fence;
        let acquire_semaphore = self.slots[slot_index].acquire_semaphore;
        let acquired = self.acquisition.get_or_acquire(slot_index, || unsafe {
            self.ctx.device.reset_fences(&[fence])?;
            self.swapchain_api
                .acquire_next_image(swapchain.handle, 0, acquire_semaphore, fence)
        });
        let acquired = match acquired {
            Ok(Some(image)) => image,
            Ok(None) => return Ok(PresentResult::Pending),
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                self.requested_size = None;
                return Ok(PresentResult::Recreate);
            }
            Err(err) => return Err(err).context("acquiring Vulkan swapchain image"),
        };
        if !acquired.acquire_ready {
            if !unsafe { self.ctx.device.get_fence_status(fence) }
                .context("checking Vulkan WSI image acquisition")?
            {
                return Ok(PresentResult::Pending);
            }
            self.acquisition.mark_ready();
        }
        let index = acquired.index;
        let suboptimal = acquired.suboptimal;
        let image = swapchain.images[index as usize];
        let present_semaphore = swapchain.present_semaphores[index as usize];
        let present_fence = swapchain.present_fences[index as usize];
        let previous_present_pending = swapchain.present_pending[index as usize];
        let extent = swapchain.extent;
        let swapchain_handle = swapchain.handle;
        if previous_present_pending {
            if !unsafe { self.ctx.device.get_fence_status(present_fence) }
                .context("checking previous Vulkan image presentation")?
            {
                return Ok(PresentResult::Pending);
            }
            self.swapchain.as_mut().unwrap().present_pending[index as usize] = false;
            unsafe { self.ctx.device.reset_fences(&[present_fence]) }
                .context("resetting Vulkan image present fence")?;
        }

        // The pool and queue are shared with the renderer. Its completed frame
        // has left the source in GENERAL layout; restore it after this copy.
        let _queue_lock = self.ctx.command_lock.lock().unwrap();
        unsafe {
            self.ctx
                .device
                .reset_command_buffer(command, vk::CommandBufferResetFlags::empty())?;
            self.ctx.device.reset_fences(&[fence])?;
        }
        if let Err(err) = self.submit_copy(
            command,
            source.image(),
            image,
            extent,
            CopySync {
                acquire: acquire_semaphore,
                copied: present_semaphore,
                fence,
            },
        ) {
            // A rejected copy did not consume the signaled acquire semaphore.
            // Keep the acquired image for the paced retry instead of waiting for
            // queue-idle and abandoning a live host image.
            return Err(err).context("submitting Vulkan WSI copy");
        }
        self.acquisition.submitted();
        self.slots[slot_index].in_flight = true;
        self.slots[slot_index].source = Some(source.clone());
        window.pre_present_notify();
        let present = vk::PresentInfoKHR::default()
            .wait_semaphores(std::slice::from_ref(&present_semaphore))
            .swapchains(std::slice::from_ref(&swapchain_handle))
            .image_indices(std::slice::from_ref(&index));
        let presented = if self.ctx.supports_swapchain_maintenance {
            let mut present_fence_info = vk::SwapchainPresentFenceInfoEXT::default()
                .fences(std::slice::from_ref(&present_fence));
            unsafe {
                self.swapchain_api.queue_present(
                    self.ctx.graphics_queue,
                    &present.push_next(&mut present_fence_info),
                )
            }
        } else {
            unsafe {
                self.swapchain_api
                    .queue_present(self.ctx.graphics_queue, &present)
            }
        };
        drop(_queue_lock);
        if self.ctx.supports_swapchain_maintenance && present_operation_is_queued(&presented) {
            self.swapchain.as_mut().unwrap().present_pending[index as usize] = true;
        }
        self.next_slot = (slot_index + 1) % self.slots.len();
        if presented.is_ok() {
            self.presented_frames += 1;
            if self.presented_frames == 1 {
                info!("nested Vulkan WSI presented first frame");
            }
            if self.presented_generation != self.generation {
                self.presented_generation = self.generation;
                debug!(
                    "nested Vulkan WSI presented swapchain generation {} at {}x{}",
                    self.generation, extent.width, extent.height,
                );
            }
        }
        match presented {
            Ok(present_suboptimal) if suboptimal || present_suboptimal => {
                // Recreate on a later attempt once all old work is ready.
                self.requested_size = None;
            }
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                self.requested_size = None;
                return Ok(PresentResult::Recreate);
            }
            Err(err) => {
                if !present_operation_is_queued(&presented) {
                    // The copy may still be signaling its semaphore. Retire it
                    // only after resize's nonblocking copy readiness check; the
                    // rejected present never owned that semaphore or its fence.
                    self.requested_size = None;
                }
                return Err(err).context("presenting Vulkan swapchain image");
            }
            _ => (),
        }
        Ok(PresentResult::Presented)
    }

    fn submit_copy(
        &self,
        command: vk::CommandBuffer,
        source: vk::Image,
        destination: vk::Image,
        extent: vk::Extent2D,
        sync: CopySync,
    ) -> anyhow::Result<()> {
        let device = &self.ctx.device;
        unsafe {
            device.begin_command_buffer(command, &vk::CommandBufferBeginInfo::default())?;
            let src_to_copy = vk::ImageMemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::MEMORY_WRITE)
                .dst_access_mask(vk::AccessFlags::TRANSFER_READ)
                .old_layout(vk::ImageLayout::GENERAL)
                .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(source)
                .subresource_range(color_range());
            let dst_to_copy = vk::ImageMemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::empty())
                .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(destination)
                .subresource_range(color_range());
            device.cmd_pipeline_barrier(
                command,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[src_to_copy, dst_to_copy],
            );
            device.cmd_copy_image(
                command,
                source,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                destination,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[vk::ImageCopy::default()
                    .src_subresource(color_layers())
                    .dst_subresource(color_layers())
                    .extent(vk::Extent3D {
                        width: extent.width,
                        height: extent.height,
                        depth: 1,
                    })],
            );
            let src_from_copy = vk::ImageMemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::TRANSFER_READ)
                .dst_access_mask(vk::AccessFlags::MEMORY_READ)
                .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .new_layout(vk::ImageLayout::GENERAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(source)
                .subresource_range(color_range());
            let dst_to_present = vk::ImageMemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::empty())
                .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(destination)
                .subresource_range(color_range());
            device.cmd_pipeline_barrier(
                command,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[src_from_copy, dst_to_present],
            );
            device.end_command_buffer(command)?;
            let buffers = [command];
            let waits = [sync.acquire];
            let stages = [vk::PipelineStageFlags::TRANSFER];
            let signals = [sync.copied];
            let submit = vk::SubmitInfo::default()
                .wait_semaphores(&waits)
                .wait_dst_stage_mask(&stages)
                .command_buffers(&buffers)
                .signal_semaphores(&signals);
            device.queue_submit(self.ctx.graphics_queue, &[submit], sync.fence)?;
        }
        Ok(())
    }
}

fn color_range() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .level_count(1)
        .layer_count(1)
}

fn color_layers() -> vk::ImageSubresourceLayers {
    vk::ImageSubresourceLayers::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .layer_count(1)
}

/// Return whether `vkQueuePresentKHR` enqueued the queue operations that own
/// the wait semaphore and present fence.  The four rejection results below
/// are explicitly specified to enqueue those operations; allocation errors
/// leave every referenced object untouched, so their fence must not be waited.
fn present_operation_is_queued(result: &Result<bool, vk::Result>) -> bool {
    matches!(
        result,
        Ok(_)
            | Err(vk::Result::ERROR_OUT_OF_DATE_KHR)
            | Err(vk::Result::ERROR_SURFACE_LOST_KHR)
            | Err(vk::Result::ERROR_FULL_SCREEN_EXCLUSIVE_MODE_LOST_EXT)
    )
}

impl Drop for VulkanWsi {
    fn drop(&mut self) {
        // A present fence, unlike a submit fence or device_wait_idle, proves
        // that the presentation engine has released its wait semaphore. Keep
        // the driver objects alive on timeout/device loss instead of freeing
        // objects that the presentation engine might still reference.
        let pending_presents = self
            .swapchain
            .as_ref()
            .map(|swapchain| {
                swapchain
                    .present_fences
                    .iter()
                    .zip(&swapchain.present_pending)
                    .filter_map(|(&fence, &pending)| pending.then_some(fence))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if !self.ctx.supports_swapchain_maintenance {
            // Legacy compatibility policy: complete queue work before cleanup.
            // Core Vulkan cannot formally prove presentation resource release
            // with QueueWaitIdle alone; unlike the maintenance1 path above,
            // this is the common best-effort shutdown convention, not a fence
            // guarantee. See docs/vulkan-validation.md for the limitation.
            if let Err(err) =
                self.wait_for_queue_idle("waiting for Vulkan WSI queue during shutdown")
            {
                warn!(
                    "error waiting for Vulkan WSI queue during shutdown: {err}; retaining WSI resources"
                );
                super::lifecycle::record_quarantine();
                std::mem::forget(self.ctx.clone());
                super::lifecycle::record_quarantine();
                std::mem::forget(std::mem::take(&mut self.slots));
                return;
            }
        }
        let pending_copies = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| {
                let acquiring = self
                    .acquisition
                    .image
                    .is_some_and(|image| image.slot == index && !image.acquire_ready);
                (slot.in_flight || acquiring).then_some(slot.fence)
            })
            .collect::<Vec<_>>();
        for (kind, fences) in [
            ("presentation", pending_presents),
            ("WSI copy", pending_copies),
        ] {
            if !fences.is_empty() {
                if let Err(err) = super::wait_for_fences(&self.ctx.device, &fences) {
                    warn!(
                        "error waiting for Vulkan {kind} during shutdown: {err}; retaining WSI resources"
                    );
                    super::lifecycle::record_quarantine();
                    std::mem::forget(self.ctx.clone());
                    super::lifecycle::record_quarantine();
                    std::mem::forget(std::mem::take(&mut self.slots));
                    return;
                }
            }
        }
        // The WSI slots share VulkanContext's command pool with rendering and
        // transfers. Freeing their command buffers needs the same external
        // synchronization used for allocation and recording.
        let _command_lock = self.ctx.command_lock.lock().unwrap();
        unsafe {
            if let Some(swapchain) = self.swapchain.take() {
                self.destroy_swapchain(swapchain);
            }
            for slot in self.slots.drain(..) {
                self.ctx
                    .device
                    .destroy_semaphore(slot.acquire_semaphore, None);
                self.ctx.device.destroy_fence(slot.fence, None);
                self.ctx
                    .device
                    .free_command_buffers(self.ctx.command_pool, &[slot.command]);
            }
            self.surface_api.destroy_surface(self.surface, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_acquisition_keeps_the_same_image_and_semaphore_slot_until_submission() {
        let mut acquisition = Acquisition::default();
        // Temporary acquire failures neither own an image nor authorize binary
        // semaphore reuse after a successful acquisition.
        for error in [vk::Result::NOT_READY, vk::Result::TIMEOUT] {
            assert_eq!(acquisition.get_or_acquire(1, || Err(error)), Ok(None));
            assert!(acquisition.image.is_none());
        }
        let image = acquisition
            .get_or_acquire(1, || Ok((2, true)))
            .unwrap()
            .unwrap();
        // A slow acquire signal/present fence must never call acquire again or
        // reset the already signaled semaphore/fence on that slot.
        for _ in 0..1000 {
            assert_eq!(
                acquisition.get_or_acquire(1, || panic!("acquired twice")),
                Ok(Some(image))
            );
        }
        acquisition.mark_ready();
        let ready = AcquiredImage {
            acquire_ready: true,
            ..image
        };
        assert_eq!(
            acquisition.get_or_acquire(1, || panic!("reacquired after copy rejection")),
            Ok(Some(ready))
        );
        acquisition.submitted();
        assert!(acquisition.image.is_none());
        assert_eq!(
            acquisition
                .get_or_acquire(0, || Ok((0, false)))
                .unwrap()
                .unwrap()
                .slot,
            0
        );
    }

    #[test]
    fn acquisition_failure_does_not_install_an_image_or_lose_a_future_retry() {
        let mut acquisition = Acquisition::default();
        for error in [
            vk::Result::ERROR_OUT_OF_DATE_KHR,
            vk::Result::ERROR_DEVICE_LOST,
        ] {
            assert_eq!(acquisition.get_or_acquire(0, || Err(error)), Err(error));
            assert!(acquisition.image.is_none());
        }
        assert!(acquisition
            .get_or_acquire(0, || Ok((1, false)))
            .unwrap()
            .is_some());
    }

    #[test]
    fn rejected_present_does_not_wait_for_a_fence_that_was_never_enqueued() {
        for result in [
            Ok(false),
            Ok(true),
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR),
            Err(vk::Result::ERROR_SURFACE_LOST_KHR),
            Err(vk::Result::ERROR_FULL_SCREEN_EXCLUSIVE_MODE_LOST_EXT),
        ] {
            assert!(present_operation_is_queued(&result));
        }
        for error in [
            vk::Result::ERROR_OUT_OF_HOST_MEMORY,
            vk::Result::ERROR_OUT_OF_DEVICE_MEMORY,
        ] {
            assert!(!present_operation_is_queued(&Err(error)));
        }
    }
}
