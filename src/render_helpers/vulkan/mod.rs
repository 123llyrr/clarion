//! Vulkan Rendering Backend architecture for Clarion.
//!
//! Inspired by Gamescope (Valve), wlroots (`wlr_vk_renderer`), and Smithay.
//! This module provides the core types and traits required to integrate Vulkan rendering
//! into Clarion's compositing pipeline without breaking existing GLES pipelines.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;
use std::sync::Arc;

use ash::vk;
use ash::vk::Handle as _;
use smithay::backend::allocator::dmabuf::{Dmabuf, WeakDmabuf};
use smithay::backend::drm::DrmNode;

pub mod blur;
mod color;
pub(crate) mod completion;
pub mod custom_shader;
mod damage;
pub mod device;
pub mod dmabuf;
mod fd_import;
pub mod frame;
pub mod hdr;
pub mod lifecycle;
mod memory;
pub mod native;
pub mod pipeline;
mod shm;
pub(crate) mod timing;
pub mod wsi;

pub use device::{VulkanContext, VulkanInitError};
pub use dmabuf::{DmabufImporter, VulkanImportError};
pub use frame::VulkanFrame;
pub use pipeline::{FormatPipelines, QuadPushConstants, VulkanPipelineManager};

/// Bounded GPU wait so a hung device cannot freeze the compositor event loop.
pub const FENCE_WAIT_TIMEOUT_NS: u64 = 2_000_000_000;

#[cfg(test)]
thread_local! {
    static FENCE_WAIT_CALLS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn owner_fence_wait_calls_for_test() -> u64 {
    FENCE_WAIT_CALLS.with(std::cell::Cell::get)
}

pub fn wait_for_fences(device: &ash::Device, fences: &[vk::Fence]) -> Result<(), vk::Result> {
    // Cover asynchronous frame/upload backpressure and resource destruction as
    // well as explicitly synchronous submissions, without timing either twice.
    let _latency = crate::utils::latency::Span::new(crate::utils::latency::Operation::VulkanFence);
    #[cfg(test)]
    FENCE_WAIT_CALLS.with(|calls| calls.set(calls.get() + 1));
    unsafe { device.wait_for_fences(fences, true, FENCE_WAIT_TIMEOUT_NS) }
}

/// Backend indicator for Vulkan rendering capabilities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VulkanBackendInfo {
    pub api_version: (u32, u32, u32),
    pub device_name: String,
    pub supports_dynamic_rendering: bool,
    pub supports_drm_modifiers: bool,
    pub supports_dmabuf_import: bool,
    pub supports_timeline_semaphores: bool,
}

impl Default for VulkanBackendInfo {
    fn default() -> Self {
        Self {
            api_version: (0, 0, 0),
            device_name: "Not initialized".into(),
            supports_dynamic_rendering: false,
            supports_drm_modifiers: false,
            supports_dmabuf_import: false,
            supports_timeline_semaphores: false,
        }
    }
}

/// GPU objects for one imported or allocated image. Dropped when the last
/// `VulkanTexture` clone is gone — nourish-style, so a cache can share imports
/// across frames without per-frame create/destroy.
struct TextureGpu {
    ctx: Arc<VulkanContext>,
    image: vk::Image,
    image_view: vk::ImageView,
    sampled_view: vk::ImageView,
    memory: vk::DeviceMemory,
    allocation_size: vk::DeviceSize,
    extra_memories: Vec<vk::DeviceMemory>,
    yuv_pipeline: Option<Arc<pipeline::YuvPipeline>>,
    current_layout: ImageLayoutState,
    descriptors: std::sync::Mutex<Vec<pipeline::TextureDescriptor>>,
    external: bool,
    usage: vk::ImageUsageFlags,
}

impl Drop for TextureGpu {
    fn drop(&mut self) {
        self.descriptors.get_mut().unwrap().clear();
        unsafe {
            self.ctx.device.destroy_image_view(self.sampled_view, None);
            self.ctx.device.destroy_image_view(self.image_view, None);
            self.ctx.device.destroy_image(self.image, None);
            self.ctx.device.free_memory(self.memory, None);
            for memory in self.extra_memories.drain(..) {
                self.ctx.device.free_memory(memory, None);
            }
        }
    }
}

unsafe impl Send for TextureGpu {}
unsafe impl Sync for TextureGpu {}

/// Shared layout metadata. GPU submission and layout transitions remain ordered
/// by the context command lock; atomic storage allows texture handles to travel
/// between threads without exposing a non-thread-safe Cell.
#[derive(Debug)]
pub struct ImageLayoutState(std::sync::atomic::AtomicI32);
impl ImageLayoutState {
    fn new(layout: vk::ImageLayout) -> Self {
        Self(std::sync::atomic::AtomicI32::new(layout.as_raw()))
    }
    pub fn get(&self) -> vk::ImageLayout {
        vk::ImageLayout::from_raw(self.0.load(std::sync::atomic::Ordering::Acquire))
    }
    pub fn set(&self, layout: vk::ImageLayout) {
        self.0
            .store(layout.as_raw(), std::sync::atomic::Ordering::Release);
    }
}

/// Representation of a Vulkan texture / image handle in Clarion.
#[derive(Clone)]
pub struct VulkanTexture {
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub has_alpha: bool,
    y_inverted: bool,
    inner: Arc<TextureGpu>,
    // An attachment lease belongs to this imported handle, not the shared VkImage
    // or DMA-BUF cache. Captured clones preserve the exact old buffer generation.
    client_buffer: Option<smithay::backend::renderer::utils::Buffer>,
    pub(crate) color: crate::color::SurfaceColor,
}

impl std::fmt::Debug for VulkanTexture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VulkanTexture")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("format", &self.format)
            .field("has_alpha", &self.has_alpha)
            .finish()
    }
}

impl VulkanTexture {
    pub(crate) fn is_unique_reference(&self) -> bool {
        Arc::strong_count(&self.inner) == 1
    }

    pub fn image(&self) -> vk::Image {
        self.inner.image
    }

    pub fn cache_key(&self) -> u64 {
        self.inner.image.as_raw()
    }

    pub fn image_view(&self) -> vk::ImageView {
        self.inner.image_view
    }

    pub fn sampled_image_view(&self) -> vk::ImageView {
        if self.inner.sampled_view == vk::ImageView::null() {
            self.inner.image_view
        } else {
            self.inner.sampled_view
        }
    }

    pub fn current_layout(&self) -> &ImageLayoutState {
        &self.inner.current_layout
    }
}

impl smithay::backend::renderer::Texture for VulkanTexture {
    fn width(&self) -> u32 {
        self.width
    }
    fn height(&self) -> u32 {
        self.height
    }
    fn format(&self) -> Option<smithay::backend::allocator::Fourcc> {
        use smithay::backend::allocator::Fourcc;
        Some(
            match (vk::Format::from_raw(self.format as i32), self.has_alpha) {
                (vk::Format::R16G16B16A16_SFLOAT, true) => Fourcc::Abgr16161616f,
                (vk::Format::R16G16B16A16_SFLOAT, false) => Fourcc::Xbgr16161616f,
                (vk::Format::R8G8B8A8_UNORM, true) => Fourcc::Abgr8888,
                (vk::Format::R8G8B8A8_UNORM, false) => Fourcc::Xbgr8888,
                (vk::Format::B8G8R8A8_UNORM, true) => Fourcc::Argb8888,
                (vk::Format::B8G8R8A8_UNORM, false) => Fourcc::Xrgb8888,
                (vk::Format::A2B10G10R10_UNORM_PACK32, true) => Fourcc::Abgr2101010,
                (vk::Format::A2B10G10R10_UNORM_PACK32, false) => Fourcc::Xbgr2101010,
                (vk::Format::A2R10G10B10_UNORM_PACK32, true) => Fourcc::Argb2101010,
                (vk::Format::A2R10G10B10_UNORM_PACK32, false) => Fourcc::Xrgb2101010,
                (vk::Format::R5G6B5_UNORM_PACK16, false) => Fourcc::Rgb565,
                (vk::Format::B5G6R5_UNORM_PACK16, false) => Fourcc::Bgr565,
                (vk::Format::G8_B8R8_2PLANE_420_UNORM, false) => Fourcc::Nv12,
                (vk::Format::G8_B8_R8_3PLANE_420_UNORM, false) => Fourcc::Yuv420,
                (vk::Format::G10X6_B10X6R10X6_2PLANE_420_UNORM_3PACK16, false) => Fourcc::P010,
                _ => return None,
            },
        )
    }
}

/// GBM exports new Dmabuf wrappers for the same allocation on each frame.
#[derive(Clone, PartialEq, Eq, Hash)]
struct TargetKey {
    planes: Vec<(u64, u64, u32, u32)>,
    width: u32,
    height: u32,
    format: u32,
    modifier: u64,
}
impl TargetKey {
    fn new(buffer: &Dmabuf) -> Result<Self, VulkanImportError> {
        use std::os::fd::AsRawFd;

        use smithay::backend::allocator::Buffer;
        let planes = buffer
            .handles()
            .zip(buffer.offsets())
            .zip(buffer.strides())
            .map(|((fd, offset), stride)| {
                let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
                if unsafe { libc::fstat(fd.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
                    return Err(VulkanImportError::InvalidLayout);
                }
                let stat = unsafe { stat.assume_init() };
                Ok((stat.st_dev, stat.st_ino, offset, stride))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            planes,
            width: buffer.width(),
            height: buffer.height(),
            format: buffer.format().code as u32,
            modifier: buffer.format().modifier.into(),
        })
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CompositionStats {
    pub damaged_pixels: u64,
    pub gpu_acquire: bool,
}

struct CachedTarget {
    texture: VulkanTexture,
    scratch: Option<VulkanTexture>,
    dmabuf_owners: Vec<WeakDmabuf>,
    last_commit: Option<smithay::backend::renderer::utils::CommitCounter>,
}

/// Driver-owned initialization only. This Send bundle contains no renderer caches,
/// Wayland objects, or Rc values; recovery prepares it on its one bounded worker.
pub(crate) struct PreparedVulkanDevice {
    pub ctx: Arc<VulkanContext>,
    pub pipeline_mgr: Arc<VulkanPipelineManager>,
}

impl PreparedVulkanDevice {
    fn from_context(ctx: Arc<VulkanContext>) -> Result<Self, VulkanInitError> {
        let pipeline_mgr = Arc::new(VulkanPipelineManager::new(
            ctx.clone(),
            vk::Format::B8G8R8A8_UNORM,
        )?);
        Ok(Self { ctx, pipeline_mgr })
    }
}

/// Core Vulkan renderer instance for Clarion.
pub struct VulkanRenderer {
    pub info: VulkanBackendInfo,
    pub ctx: Option<Arc<VulkanContext>>,
    pub importer: Option<DmabufImporter>,
    pub pipeline_mgr: Option<Arc<VulkanPipelineManager>>,
    texture_cache: Rc<RefCell<HashMap<WeakDmabuf, dmabuf::CachedDmabufTexture>>>,
    target_cache: HashMap<TargetKey, CachedTarget>,
    // Textures retain their context; the context must never own this pool.
    scratch_textures: Arc<std::sync::Mutex<frame::ScratchPool>>,
}

impl VulkanRenderer {
    pub fn new() -> Self {
        Self {
            info: VulkanBackendInfo::default(),
            ctx: None,
            importer: None,
            pipeline_mgr: None,
            texture_cache: Rc::new(RefCell::new(HashMap::new())),
            target_cache: HashMap::new(),
            scratch_textures: Arc::default(),
        }
    }

    pub fn init_from_drm_node(
        &mut self,
        drm_node: Option<&DrmNode>,
    ) -> Result<(), VulkanInitError> {
        let ctx = VulkanContext::init(drm_node)?;
        self.init_with_context(ctx)
    }

    /// Initialize the renderer on a preselected Vulkan device, for example a
    /// WSI device whose graphics queue can present to the host window.
    pub fn init_with_context(&mut self, ctx: Arc<VulkanContext>) -> Result<(), VulkanInitError> {
        self.init_prepared_device(PreparedVulkanDevice::from_context(ctx)?);
        Ok(())
    }

    /// Owner-only cache construction after all driver initialization has finished.
    pub(crate) fn init_prepared_device(&mut self, device: PreparedVulkanDevice) {
        let PreparedVulkanDevice { ctx, pipeline_mgr } = device;
        self.target_cache.clear();
        self.scratch_textures = Arc::default();
        self.texture_cache = Rc::new(RefCell::new(HashMap::new()));
        self.info = ctx.info.clone();
        self.importer = Some(DmabufImporter::new(
            ctx.clone(),
            self.texture_cache.clone(),
            pipeline_mgr.clone(),
        ));
        self.pipeline_mgr = Some(pipeline_mgr);
        self.ctx = Some(ctx);
    }

    pub(crate) fn prepared_device(&self) -> PreparedVulkanDevice {
        PreparedVulkanDevice {
            ctx: self.ctx.as_ref().expect("initialized renderer").clone(),
            pipeline_mgr: self
                .pipeline_mgr
                .as_ref()
                .expect("initialized renderer")
                .clone(),
        }
    }

    pub fn import_dmabuf(&self, dmabuf: &Dmabuf) -> Result<VulkanTexture, VulkanImportError> {
        if let Some(importer) = &self.importer {
            importer.import_dmabuf(dmabuf)
        } else {
            Err(VulkanImportError::UnsupportedExtension)
        }
    }

    /// Negotiate the same explicit format/modifier with EGL, GBM, KMS and Vulkan.
    pub fn supports_target_format(&self, format: smithay::backend::allocator::Format) -> bool {
        self.supports_dmabuf_usage(
            format,
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
        )
    }

    fn supports_dmabuf_usage(
        &self,
        format: smithay::backend::allocator::Format,
        usage: vk::ImageUsageFlags,
    ) -> bool {
        let Some(ctx) = &self.ctx else {
            return false;
        };
        // Explicit DMA-BUF images use all four Linux interop extensions and
        // foreign queue-family ownership transfers.  Keep the Vulkan renderer
        // usable for SHM/optimal-tiling paths when a driver lacks one of them;
        // those paths simply advertise no DMA-BUF formats and the compositor
        // selects its GLES/KMS fallback where necessary.
        if !ctx.supports_dmabuf_import {
            return false;
        }
        let Some((vk_format, _)) = dmabuf::fourcc_to_vk_format(format.code) else {
            return false;
        };
        let yuv = dmabuf::is_yuv_format(format.code);
        if yuv && (!ctx.supports_ycbcr_conversion || usage != vk::ImageUsageFlags::SAMPLED) {
            return false;
        }
        if format.modifier == smithay::backend::allocator::Modifier::Invalid {
            return false;
        }
        let mut modifier = vk::PhysicalDeviceImageDrmFormatModifierInfoEXT::default()
            .drm_format_modifier(format.modifier.into())
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let mut external = vk::PhysicalDeviceExternalImageFormatInfo::default()
            .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let info = vk::PhysicalDeviceImageFormatInfo2::default()
            .format(vk_format)
            .ty(vk::ImageType::TYPE_2D)
            .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .usage(usage)
            .push_next(&mut modifier)
            .push_next(&mut external);
        let mut external_props = vk::ExternalImageFormatProperties::default();
        let mut ycbcr_props = vk::SamplerYcbcrConversionImageFormatProperties::default();
        let mut props = vk::ImageFormatProperties2::default()
            .push_next(&mut external_props)
            .push_next(&mut ycbcr_props);
        let result = unsafe {
            ctx.instance.get_physical_device_image_format_properties2(
                ctx.physical_device,
                &info,
                &mut props,
            )
        };
        result.is_ok()
            && (!yuv
                || (1..=pipeline::MAX_YUV_DESCRIPTORS_PER_SET)
                    .contains(&ycbcr_props.combined_image_sampler_descriptor_count))
            && external_props
                .external_memory_properties
                .external_memory_features
                .contains(vk::ExternalMemoryFeatureFlags::IMPORTABLE)
    }

    /// Import a DMA-BUF specifically configured as a color attachment (render target).
    pub fn import_dmabuf_as_target(
        &self,
        dmabuf: &Dmabuf,
    ) -> Result<VulkanTexture, VulkanImportError> {
        if let Some(importer) = &self.importer {
            importer.import_dmabuf_with_usage(
                dmabuf,
                vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
            )
        } else {
            Err(VulkanImportError::UnsupportedExtension)
        }
    }

    fn prune_caches(&mut self) {
        self.texture_cache
            .borrow_mut()
            .retain(|k, _| k.upgrade().is_some());
    }

    fn cache_target(&mut self, target: &Dmabuf) -> Result<TargetKey, VulkanImportError> {
        self.prune_caches();
        let key = TargetKey::new(target)?;
        // Swapchain Slot::export retains the original Dmabuf until that slot
        // dies. Imported VkImages must follow that lifetime: an LRU eviction
        // would discard the exact FP16 scene described by a live buffer age.
        self.target_cache.retain(|old, entry| {
            entry.dmabuf_owners.retain(|owner| !owner.is_gone());
            old == &key || !entry.dmabuf_owners.is_empty()
        });
        if let Some(entry) = self.target_cache.get_mut(&key) {
            // A producer may re-export one live allocation into a fresh wrapper.
            // Our imported Vulkan memory keeps that allocation and inode alive;
            // refresh its weak wrapper without losing the scene/damage history.
            let owner = target.weak();
            if !entry.dmabuf_owners.contains(&owner) {
                entry.dmabuf_owners.push(owner);
            }
        }
        if !self.target_cache.contains_key(&key) {
            let texture = self.import_dmabuf_as_target(target)?;
            const MAX_TARGET_CACHE_BYTES: u64 = 1024 * 1024 * 1024;
            let retained: u64 = self
                .target_cache
                .values()
                .map(|entry| {
                    entry.texture.inner.allocation_size
                        + entry
                            .scratch
                            .as_ref()
                            .map_or(0, |scratch| scratch.inner.allocation_size)
                })
                .sum();
            if retained.saturating_add(texture.inner.allocation_size) > MAX_TARGET_CACHE_BYTES {
                return Err(VulkanImportError::VkError(
                    vk::Result::ERROR_OUT_OF_DEVICE_MEMORY,
                ));
            }
            self.target_cache.insert(
                key.clone(),
                CachedTarget {
                    texture,
                    scratch: None,
                    dmabuf_owners: vec![target.weak()],
                    last_commit: None,
                },
            );
        }
        Ok(key)
    }

    /// Render to externally owned raw image handles. The caller must keep the
    /// image and view alive until the GPU completes, including after a fence
    /// timeout: an error does not cancel an already submitted command buffer.
    #[allow(clippy::too_many_arguments)]
    pub fn render_frame<'frame>(
        &self,
        target_image: vk::Image,
        target_image_view: vk::ImageView,
        target_format: vk::Format,
        target_has_alpha: bool,
        width: u32,
        height: u32,
        clear_color: Option<[f32; 4]>,
    ) -> Result<VulkanFrame<'frame>, vk::Result> {
        let ctx = self
            .ctx
            .as_ref()
            .ok_or(vk::Result::ERROR_INITIALIZATION_FAILED)?;
        let pipeline_mgr = self
            .pipeline_mgr
            .as_ref()
            .ok_or(vk::Result::ERROR_INITIALIZATION_FAILED)?;
        VulkanFrame::begin(
            ctx.clone(),
            pipeline_mgr.clone(),
            self.scratch_textures.clone(),
            self.importer.clone().unwrap_or_else(|| {
                DmabufImporter::new(
                    ctx.clone(),
                    self.texture_cache.clone(),
                    pipeline_mgr.clone(),
                )
            }),
            target_image,
            target_image_view,
            target_format,
            target_has_alpha,
            width,
            height,
            clear_color,
            if clear_color.is_some() {
                vk::ImageLayout::UNDEFINED
            } else {
                vk::ImageLayout::GENERAL
            },
            false,
        )
    }

    /// Execute a Vulkan rendering pass directly into a target DMA-BUF (such as the DRM scanout
    /// framebuffer).
    pub fn render_to_target_dmabuf(
        &mut self,
        target_dmabuf: &Dmabuf,
        clear_color: Option<[f32; 4]>,
        render_fn: impl FnOnce(&mut VulkanFrame<'_>) -> Result<(), vk::Result>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let ctx = self
            .ctx
            .as_ref()
            .ok_or(VulkanImportError::UnsupportedExtension)?
            .clone();
        let pipeline_mgr = self
            .pipeline_mgr
            .as_ref()
            .ok_or(VulkanImportError::UnsupportedExtension)?
            .clone();
        let importer = self
            .importer
            .as_ref()
            .ok_or(VulkanImportError::UnsupportedExtension)?
            .clone();
        let key = self.cache_target(target_dmabuf)?;
        let target_texture = self.target_cache[&key].texture.clone();
        let target_format = vk::Format::from_raw(target_texture.format as i32);
        let mut frame = VulkanFrame::begin(
            ctx,
            pipeline_mgr,
            self.scratch_textures.clone(),
            importer,
            target_texture.image(),
            target_texture.image_view(),
            target_format,
            target_texture.has_alpha,
            target_texture.width,
            target_texture.height,
            clear_color,
            vk::ImageLayout::GENERAL,
            true,
        )?;
        frame.retain_target(&target_texture);
        render_fn(&mut frame)?;
        frame.finish_and_wait()?;
        Ok(())
    }

    /// Compatibility entry point for producers whose work has already completed.
    pub fn composite_dmabuf(
        &mut self,
        target: &Dmabuf,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.composite_dmabuf_after(
            target,
            &smithay::backend::renderer::sync::SyncPoint::signaled(),
            None,
        )
        .map(|_| ())
    }

    /// Nourish-style damage-aware composition and GPU producer-fence acquisition.
    /// Damage must describe the untransformed framebuffer. Pass None for full damage.
    pub fn composite_dmabuf_after(
        &mut self,
        target: &Dmabuf,
        producer: &smithay::backend::renderer::sync::SyncPoint,
        damage: Option<
            &smithay::backend::renderer::utils::DamageSnapshot<i32, smithay::utils::Buffer>,
        >,
    ) -> Result<CompositionStats, Box<dyn std::error::Error + Send + Sync>> {
        let key = self.cache_target(target)?;
        let cached = self.target_cache.get_mut(&key).unwrap();
        let texture = &cached.texture;
        let full = vk::Rect2D {
            offset: vk::Offset2D::default(),
            extent: vk::Extent2D {
                width: texture.width,
                height: texture.height,
            },
        };
        let dirty = damage.and_then(|d| d.damage_since(cached.last_commit));
        let region = if cached.scratch.is_none() {
            Some(full)
        } else if let Some(dirty) = dirty {
            damage::bounding_rect(&dirty, texture.width, texture.height)
        } else {
            Some(full)
        };
        let Some(region) = region else {
            // No Vulkan submission can acquire the dependency for KMS. Preserve
            // the commit and retry instead of waiting on the desktop thread.
            crate::render_helpers::sync::require_ready(producer)?;
            cached.last_commit = damage.map(|d| d.current_commit());
            return Ok(CompositionStats::default());
        };
        let format = vk::Format::from_raw(texture.format as i32);
        let ctx = self
            .ctx
            .as_ref()
            .ok_or(VulkanImportError::UnsupportedExtension)?
            .clone();
        let pipeline_mgr = self
            .pipeline_mgr
            .as_ref()
            .ok_or(VulkanImportError::UnsupportedExtension)?
            .clone();
        let importer = self
            .importer
            .as_ref()
            .ok_or(VulkanImportError::UnsupportedExtension)?
            .clone();
        if cached.scratch.is_none() {
            cached.scratch = Some(allocate_texture(
                &ctx,
                texture.width,
                texture.height,
                format,
                vk::ImageTiling::OPTIMAL,
                vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
                vk::ImageLayout::UNDEFINED,
                texture.has_alpha,
            )?);
        }
        let mut frame = VulkanFrame::begin(
            ctx,
            pipeline_mgr,
            self.scratch_textures.clone(),
            importer,
            texture.image(),
            texture.image_view(),
            format,
            texture.has_alpha,
            texture.width,
            texture.height,
            None,
            vk::ImageLayout::GENERAL,
            true,
        )?;
        frame.retain_target(texture);
        let gpu_acquire = frame.wait_for_producer(producer)?;
        frame.composite_scene(
            cached.scratch.as_ref().unwrap().clone(),
            if region == full { None } else { Some(region) },
        )?;
        frame.finish_and_wait()?;
        // Commit only after success, so retries never skip a failed frame's damage.
        cached.last_commit = damage.map(|d| d.current_commit());
        Ok(CompositionStats {
            damaged_pixels: u64::from(region.extent.width) * u64::from(region.extent.height),
            gpu_acquire,
        })
    }

    /// Create a 1x1 solid color texture on host-visible memory (for pure color quads, overlays, and
    /// fallback textures).
    pub fn create_solid_texture(&self, color: [u8; 4]) -> Result<VulkanTexture, vk::Result> {
        let ctx = self
            .ctx
            .as_ref()
            .ok_or(vk::Result::ERROR_INITIALIZATION_FAILED)?;
        create_solid_texture_raw(ctx, color)
    }
}

/// Own partially initialized image objects so every error path releases them.
impl TextureGpu {
    fn create_views(
        &mut self,
        format: vk::Format,
        has_alpha: bool,
        usage: vk::ImageUsageFlags,
    ) -> Result<(), vk::Result> {
        let attachment = usage.contains(vk::ImageUsageFlags::COLOR_ATTACHMENT);
        let mut components = vk::ComponentMapping::default();
        if !has_alpha && !attachment {
            components.a = vk::ComponentSwizzle::ONE;
        }
        let base = vk::ImageViewCreateInfo::default()
            .image(self.image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(format)
            .components(components)
            .subresource_range(frame::color_range());
        self.image_view = unsafe { self.ctx.device.create_image_view(&base, None) }?;
        if !has_alpha && attachment && usage.contains(vk::ImageUsageFlags::SAMPLED) {
            // Color attachments require identity component mapping. Sampling the
            // same opaque storage needs alpha-one regardless of stored alpha bits.
            components.a = vk::ComponentSwizzle::ONE;
            let mut sampled_usage =
                vk::ImageViewUsageCreateInfo::default().usage(vk::ImageUsageFlags::SAMPLED);
            let sampled = base.components(components).push_next(&mut sampled_usage);
            self.sampled_view = unsafe { self.ctx.device.create_image_view(&sampled, None) }?;
        }
        Ok(())
    }
    fn empty(ctx: Arc<VulkanContext>) -> Self {
        Self {
            ctx,
            image: vk::Image::null(),
            image_view: vk::ImageView::null(),
            sampled_view: vk::ImageView::null(),
            memory: vk::DeviceMemory::null(),
            allocation_size: 0,
            extra_memories: Vec::new(),
            yuv_pipeline: None,
            current_layout: ImageLayoutState::new(vk::ImageLayout::UNDEFINED),
            descriptors: std::sync::Mutex::new(Vec::new()),
            external: false,
            usage: vk::ImageUsageFlags::empty(),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn allocate_texture(
    ctx: &Arc<VulkanContext>,
    width: u32,
    height: u32,
    format: vk::Format,
    tiling: vk::ImageTiling,
    usage: vk::ImageUsageFlags,
    layout: vk::ImageLayout,
    has_alpha: bool,
) -> Result<VulkanTexture, vk::Result> {
    let mut gpu = TextureGpu::empty(ctx.clone());
    let device = &ctx.device;
    let info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(format)
        .extent(vk::Extent3D {
            width,
            height,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(tiling)
        .usage(usage)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(layout);
    unsafe {
        gpu.image = device.create_image(&info, None)?;
        let req = device.get_image_memory_requirements(gpu.image);
        gpu.allocation_size = req.size;
        let props = ctx
            .instance
            .get_physical_device_memory_properties(ctx.physical_device);
        let flags = if tiling == vk::ImageTiling::LINEAR {
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT
        } else {
            vk::MemoryPropertyFlags::DEVICE_LOCAL
        };
        let index = (0..props.memory_type_count)
            .find(|i| {
                req.memory_type_bits & (1 << i) != 0
                    && props.memory_types[*i as usize]
                        .property_flags
                        .contains(flags)
            })
            .ok_or(vk::Result::ERROR_OUT_OF_DEVICE_MEMORY)?;
        gpu.memory = device.allocate_memory(
            &vk::MemoryAllocateInfo::default()
                .allocation_size(req.size)
                .memory_type_index(index),
            None,
        )?;
        device.bind_image_memory(gpu.image, gpu.memory, 0)?;
        gpu.create_views(format, has_alpha, usage)?;
    }
    gpu.current_layout.set(layout);
    gpu.usage = usage;
    Ok(VulkanTexture {
        width,
        height,
        format: format.as_raw() as u32,
        has_alpha,
        y_inverted: false,
        inner: Arc::new(gpu),
        client_buffer: None,
        color: Default::default(),
    })
}

pub fn create_solid_texture_raw(
    ctx: &Arc<VulkanContext>,
    color: [u8; 4],
) -> Result<VulkanTexture, vk::Result> {
    let texture = allocate_texture(
        ctx,
        1,
        1,
        vk::Format::R8G8B8A8_UNORM,
        vk::ImageTiling::LINEAR,
        vk::ImageUsageFlags::SAMPLED,
        vk::ImageLayout::PREINITIALIZED,
        color[3] < 255,
    )?;
    unsafe {
        let subresource = vk::ImageSubresource::default().aspect_mask(vk::ImageAspectFlags::COLOR);
        let layout = ctx
            .device
            .get_image_subresource_layout(texture.image(), subresource);
        let ptr = ctx.device.map_memory(
            texture.inner.memory,
            0,
            vk::WHOLE_SIZE,
            vk::MemoryMapFlags::empty(),
        )? as *mut u8;
        std::ptr::copy_nonoverlapping(color.as_ptr(), ptr.add(layout.offset as usize), 4);
        ctx.device.unmap_memory(texture.inner.memory);
    }
    Ok(texture)
}

impl Default for VulkanRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for VulkanRenderer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VulkanRenderer")
            .field("info", &self.info)
            .field("has_context", &self.ctx.is_some())
            .field("has_pipeline_mgr", &self.pipeline_mgr.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use smithay::backend::allocator::Fourcc;

    use super::*;

    fn gpu_test_renderer() -> Option<VulkanRenderer> {
        let path = std::env::var_os("CLARION_VULKAN_TEST_NODE");
        let required = std::env::var_os("CLARION_REQUIRE_VULKAN").is_some();
        assert!(
            !required || path.is_some(),
            "CLARION_REQUIRE_VULKAN requires CLARION_VULKAN_TEST_NODE to select a physical GPU"
        );
        let node = path.as_ref().map(|path| {
            DrmNode::from_path(path)
                .unwrap_or_else(|err| panic!("invalid CLARION_VULKAN_TEST_NODE {path:?}: {err}"))
        });
        let mut renderer = VulkanRenderer::new();
        match renderer.init_from_drm_node(node.as_ref()) {
            Ok(()) => {
                let selected = path
                    .as_ref()
                    .map(|path| std::path::Path::new(path).display().to_string())
                    .unwrap_or_else(|| "loader default device".into());
                eprintln!("HARDWARE: {} ({selected})", renderer.info.device_name);
                Some(renderer)
            }
            Err(err) => {
                assert!(
                    !required && path.is_none(),
                    "Vulkan initialization is required for selected node {path:?}: {err}"
                );
                eprintln!("Vulkan unavailable in optional hardware probe: {err}");
                None
            }
        }
    }

    #[test]
    fn test_vulkan_renderer_scaffolding() {
        let renderer = VulkanRenderer::new();
        assert!(renderer.ctx.is_none());
        assert!(!renderer.info.supports_dynamic_rendering);
        assert!(!renderer.info.supports_drm_modifiers);
        assert!(!renderer.info.supports_dmabuf_import);
    }

    #[test]
    fn test_fourcc_format_mapping() {
        let (fmt_argb, has_alpha) = dmabuf::fourcc_to_vk_format(Fourcc::Argb8888).unwrap();
        assert_eq!(fmt_argb, vk::Format::B8G8R8A8_UNORM);
        assert!(has_alpha);

        let (fmt_xrgb, has_alpha_x) = dmabuf::fourcc_to_vk_format(Fourcc::Xrgb8888).unwrap();
        assert_eq!(fmt_xrgb, vk::Format::B8G8R8A8_UNORM);
        assert!(!has_alpha_x);

        let (fmt_abgr, has_alpha_abgr) = dmabuf::fourcc_to_vk_format(Fourcc::Abgr8888).unwrap();
        assert_eq!(fmt_abgr, vk::Format::R8G8B8A8_UNORM);
        assert!(has_alpha_abgr);

        let (fmt_abgr10, has_alpha_abgr10) =
            dmabuf::fourcc_to_vk_format(Fourcc::Abgr2101010).unwrap();
        assert_eq!(fmt_abgr10, vk::Format::A2B10G10R10_UNORM_PACK32);
        assert!(has_alpha_abgr10);

        let (fmt_xbgr10, has_alpha_xbgr10) =
            dmabuf::fourcc_to_vk_format(Fourcc::Xbgr2101010).unwrap();
        assert_eq!(fmt_xbgr10, vk::Format::A2B10G10R10_UNORM_PACK32);
        assert!(!has_alpha_xbgr10);

        let (fmt_argb10, has_alpha_argb10) =
            dmabuf::fourcc_to_vk_format(Fourcc::Argb2101010).unwrap();
        assert_eq!(fmt_argb10, vk::Format::A2R10G10B10_UNORM_PACK32);
        assert!(has_alpha_argb10);

        let (fmt_xrgb10, has_alpha_xrgb10) =
            dmabuf::fourcc_to_vk_format(Fourcc::Xrgb2101010).unwrap();
        assert_eq!(fmt_xrgb10, vk::Format::A2R10G10B10_UNORM_PACK32);
        assert!(!has_alpha_xrgb10);

        let (fmt_rgb565, has_alpha_rgb565) = dmabuf::fourcc_to_vk_format(Fourcc::Rgb565).unwrap();
        assert_eq!(fmt_rgb565, vk::Format::R5G6B5_UNORM_PACK16);
        assert!(!has_alpha_rgb565);

        let (fmt_bgr565, has_alpha_bgr565) = dmabuf::fourcc_to_vk_format(Fourcc::Bgr565).unwrap();
        assert_eq!(fmt_bgr565, vk::Format::B5G6R5_UNORM_PACK16);
        assert!(!has_alpha_bgr565);

        for (fourcc, format) in [
            (Fourcc::Nv12, vk::Format::G8_B8R8_2PLANE_420_UNORM),
            (Fourcc::Yuv420, vk::Format::G8_B8_R8_3PLANE_420_UNORM),
            (
                Fourcc::P010,
                vk::Format::G10X6_B10X6R10X6_2PLANE_420_UNORM_3PACK16,
            ),
        ] {
            assert_eq!(dmabuf::fourcc_to_vk_format(fourcc), Some((format, false)));
        }
    }

    #[test]
    fn test_push_constants_size_and_alignment() {
        assert_eq!(std::mem::size_of::<pipeline::QuadPushConstants>(), 236);
        assert_eq!(std::mem::offset_of!(pipeline::QuadPushConstants, clip), 112);
        assert_eq!(
            std::mem::offset_of!(pipeline::QuadPushConstants, noise),
            192
        );
        assert_eq!(
            std::mem::offset_of!(pipeline::QuadPushConstants, saturation),
            196
        );
        assert_eq!(
            std::mem::offset_of!(pipeline::QuadPushConstants, target_size),
            200
        );
        assert_eq!(
            std::mem::offset_of!(pipeline::QuadPushConstants, bg_color),
            208
        );
        assert_eq!(
            std::mem::offset_of!(pipeline::QuadPushConstants, fade_cutoff),
            224
        );
        let pc = pipeline::QuadPushConstants::default();
        assert_eq!(pc.alpha, 1.0);
        assert_eq!(pc.color_tint, [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(pc.noise, 0.0);
        assert_eq!(pc.saturation, 1.0);
        assert_eq!(pc.target_size, [0.0, 0.0]);
        assert_eq!(pc.bg_color, [0.0, 0.0, 0.0, 0.0]);

        assert_eq!(std::mem::size_of::<pipeline::BorderPushConstants>(), 192);
        assert_eq!(
            std::mem::offset_of!(pipeline::BorderPushConstants, geo_bounds),
            176
        );
        let border_pc = pipeline::BorderPushConstants::default();
        assert_eq!(border_pc.border_width, 4.0);
        assert_eq!(border_pc.colorspace, 3.0); // OKLCh

        assert_eq!(std::mem::size_of::<pipeline::ParticlePushConstants>(), 64);
        assert_eq!(std::mem::size_of::<pipeline::ShadowPushConstants>(), 176);
        assert_eq!(
            std::mem::offset_of!(pipeline::ShadowPushConstants, params),
            160
        );
    }

    #[test]
    fn test_vulkan_driver_probe() {
        let Some(renderer) = gpu_test_renderer() else {
            return;
        };
        assert!(renderer.ctx.is_some());
        assert!(renderer.pipeline_mgr.is_some());
    }

    #[test]
    fn test_vulkan_frame_rendering_lifecycle() {
        let Some(renderer) = gpu_test_renderer() else {
            return;
        };
        let ctx = renderer.ctx.clone().unwrap();

        // Create a 256x256 test render target
        let img_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk::Format::B8G8R8A8_UNORM)
            .extent(vk::Extent3D {
                width: 256,
                height: 256,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);

        let image = unsafe { ctx.device.create_image(&img_info, None).unwrap() };
        let mem_reqs = unsafe { ctx.device.get_image_memory_requirements(image) };
        let mem_props = unsafe {
            ctx.instance
                .get_physical_device_memory_properties(ctx.physical_device)
        };
        let mut mem_type_index = None;
        for i in 0..mem_props.memory_type_count {
            if (mem_reqs.memory_type_bits & (1 << i)) != 0
                && mem_props.memory_types[i as usize]
                    .property_flags
                    .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
            {
                mem_type_index = Some(i);
                break;
            }
        }
        let mem_type_index = mem_type_index.unwrap_or(0);
        let alloc_info = vk::MemoryAllocateInfo::default()
            .allocation_size(mem_reqs.size)
            .memory_type_index(mem_type_index);
        let memory = unsafe { ctx.device.allocate_memory(&alloc_info, None).unwrap() };
        unsafe { ctx.device.bind_image_memory(image, memory, 0).unwrap() };

        let view_info = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(vk::Format::B8G8R8A8_UNORM)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });
        let image_view = unsafe { ctx.device.create_image_view(&view_info, None).unwrap() };

        // Begin frame
        let mut frame = renderer
            .render_frame(
                image,
                image_view,
                vk::Format::B8G8R8A8_UNORM,
                true,
                256,
                256,
                Some([0.1, 0.2, 0.3, 1.0]),
            )
            .unwrap();

        // Draw border
        assert_eq!(
            std::mem::offset_of!(pipeline::BorderPushConstants, geo_bounds),
            176
        );
        let border_pc = pipeline::BorderPushConstants::default();
        frame
            .draw_border(10.0, 10.0, 236.0, 236.0, border_pc)
            .unwrap();

        // Finish and wait for GPU execution
        frame.finish_and_wait().unwrap();

        // Clean up test image
        unsafe {
            ctx.device.destroy_image_view(image_view, None);
            ctx.device.destroy_image(image, None);
            ctx.device.free_memory(memory, None);
        }
    }

    #[test]
    fn test_vulkan_frame_rendering_10bit_lifecycle() {
        let Some(renderer) = gpu_test_renderer() else {
            return;
        };
        let ctx = renderer.ctx.clone().unwrap();

        // Create a 256x256 test 10-bit render target (AB30 equivalent)
        let img_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk::Format::A2B10G10R10_UNORM_PACK32)
            .extent(vk::Extent3D {
                width: 256,
                height: 256,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);

        let image = unsafe { ctx.device.create_image(&img_info, None).unwrap() };
        let mem_reqs = unsafe { ctx.device.get_image_memory_requirements(image) };
        let mem_props = unsafe {
            ctx.instance
                .get_physical_device_memory_properties(ctx.physical_device)
        };
        let mut mem_type_index = None;
        for i in 0..mem_props.memory_type_count {
            if (mem_reqs.memory_type_bits & (1 << i)) != 0
                && mem_props.memory_types[i as usize]
                    .property_flags
                    .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
            {
                mem_type_index = Some(i);
                break;
            }
        }
        let mem_type_index = mem_type_index.unwrap_or(0);
        let alloc_info = vk::MemoryAllocateInfo::default()
            .allocation_size(mem_reqs.size)
            .memory_type_index(mem_type_index);
        let memory = unsafe { ctx.device.allocate_memory(&alloc_info, None).unwrap() };
        unsafe { ctx.device.bind_image_memory(image, memory, 0).unwrap() };

        let view_info = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(vk::Format::A2B10G10R10_UNORM_PACK32)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });
        let image_view = unsafe { ctx.device.create_image_view(&view_info, None).unwrap() };

        // Begin frame with 10-bit format
        let mut frame = renderer
            .render_frame(
                image,
                image_view,
                vk::Format::A2B10G10R10_UNORM_PACK32,
                true,
                256,
                256,
                Some([0.1, 0.2, 0.3, 1.0]),
            )
            .unwrap();

        // Draw border
        assert_eq!(
            std::mem::offset_of!(pipeline::BorderPushConstants, geo_bounds),
            176
        );
        let border_pc = pipeline::BorderPushConstants::default();
        frame
            .draw_border(10.0, 10.0, 236.0, 236.0, border_pc)
            .unwrap();

        // Finish and wait for GPU execution
        frame.finish_and_wait().unwrap();

        // Clean up test image
        unsafe {
            ctx.device.destroy_image_view(image_view, None);
            ctx.device.destroy_image(image, None);
            ctx.device.free_memory(memory, None);
        }
    }
}

#[cfg(test)]
pub(crate) mod hardware_tests;
