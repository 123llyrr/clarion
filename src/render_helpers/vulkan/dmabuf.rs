//! DMA-BUF Zero-Copy Import for Vulkan in Clarion.
//!
//! Imports Wayland client DMA-BUF buffers into Vulkan textures (VkImage & VkImageView)
//! without CPU copy, leveraging VK_EXT_image_drm_format_modifier and VK_KHR_external_memory_fd.

use std::cell::{Cell, RefCell};
use std::cmp::Reverse;
use std::collections::HashMap;
use std::fmt;
use std::os::fd::AsRawFd;
use std::rc::Rc;
use std::sync::Arc;

use ash::vk;
use smithay::backend::allocator::dmabuf::{Dmabuf, WeakDmabuf};
use smithay::backend::allocator::{Buffer, Fourcc, Modifier};

use super::device::VulkanContext;
use super::pipeline::{VulkanPipelineManager, YuvKey};
use super::{TextureGpu, VulkanTexture};

const MAX_CACHED_IMPORTS: usize = 64;
const MAX_CACHED_IMPORT_BYTES: u64 = 256 * 1024 * 1024;

/// Number of oldest imports to discard before retaining a new allocation.
/// Use actual memory requirements, including tiling/alignment and YUV planes.
fn import_cache_evictions(
    mut allocations: impl ExactSizeIterator<Item = u64> + Clone,
    incoming: u64,
) -> Option<usize> {
    if incoming > MAX_CACHED_IMPORT_BYTES {
        return None;
    }
    let mut count = allocations.len();
    let mut bytes: u128 = allocations.clone().map(u128::from).sum();
    let mut evictions = 0;
    while count >= MAX_CACHED_IMPORTS
        || bytes + u128::from(incoming) > u128::from(MAX_CACHED_IMPORT_BYTES)
    {
        bytes -= u128::from(allocations.next()?);
        count -= 1;
        evictions += 1;
    }
    Some(evictions)
}

/// Preserve driver objects across ordinary double/triple-buffer commits. The
/// producer key remains weak and the value retains neither a Dmabuf nor a
/// Wayland buffer owner, so release fences and wl_buffer.release are unaffected.
pub(super) struct CachedDmabufTexture {
    texture: VulkanTexture,
    last_used: u64,
}

impl CachedDmabufTexture {
    fn new(texture: &VulkanTexture, tick: u64) -> Self {
        Self {
            texture: texture.clone(),
            last_used: tick,
        }
    }
}

#[derive(Debug)]
pub enum VulkanImportError {
    UnsupportedFormat(Fourcc),
    VkError(vk::Result),
    UnsupportedExtension,
    NoMatchingMemoryType,
    InvalidLayout,
    AllocationTooSmall { available: u64, required: u64 },
    ImportFailureBudgetExceeded,
}

impl fmt::Display for VulkanImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ImportFailureBudgetExceeded => write!(f,
                "Vulkan import refused: {} uncertain driver-owned descriptors quarantined",
                super::fd_import::MAX_UNCERTAIN_IMPORTS),
            Self::AllocationTooSmall { available, required } => write!(
                f,
                "linear YUV DMA-BUF allocation has {available} bytes; Vulkan image requires {required} bytes"
            ),
            Self::InvalidLayout => {
                write!(f, "invalid DMA-BUF dimensions, modifier, or plane layout")
            }
            Self::UnsupportedFormat(fmt) => write!(f, "unsupported Fourcc format: {fmt:?}"),
            Self::VkError(err) => {
                write!(f, "Vulkan driver error during image creation/import: {err}")
            }
            Self::UnsupportedExtension => {
                write!(f, "missing external memory or DRM modifier support")
            }
            Self::NoMatchingMemoryType => {
                write!(f, "failed to find suitable memory type for DMA-BUF import")
            }
        }
    }
}

impl std::error::Error for VulkanImportError {}

impl From<vk::Result> for VulkanImportError {
    fn from(err: vk::Result) -> Self {
        Self::VkError(err)
    }
}

#[allow(dead_code)]
pub fn fourcc_is_opaque(fourcc: Fourcc) -> bool {
    matches!(
        fourcc,
        Fourcc::Xrgb8888
            | Fourcc::Xbgr8888
            | Fourcc::Rgbx8888
            | Fourcc::Bgrx8888
            | Fourcc::Xrgb2101010
            | Fourcc::Xbgr2101010
    )
}

pub fn fourcc_to_vk_format(fourcc: Fourcc) -> Option<(vk::Format, bool)> {
    match fourcc {
        Fourcc::Argb8888 => Some((vk::Format::B8G8R8A8_UNORM, true)),
        Fourcc::Xrgb8888 => Some((vk::Format::B8G8R8A8_UNORM, false)),
        Fourcc::Abgr8888 => Some((vk::Format::R8G8B8A8_UNORM, true)),
        Fourcc::Xbgr8888 => Some((vk::Format::R8G8B8A8_UNORM, false)),
        Fourcc::Abgr2101010 => Some((vk::Format::A2B10G10R10_UNORM_PACK32, true)),
        Fourcc::Xbgr2101010 => Some((vk::Format::A2B10G10R10_UNORM_PACK32, false)),
        Fourcc::Argb2101010 => Some((vk::Format::A2R10G10B10_UNORM_PACK32, true)),
        Fourcc::Xrgb2101010 => Some((vk::Format::A2R10G10B10_UNORM_PACK32, false)),
        Fourcc::Rgb565 => Some((vk::Format::R5G6B5_UNORM_PACK16, false)),
        Fourcc::Bgr565 => Some((vk::Format::B5G6R5_UNORM_PACK16, false)),
        Fourcc::Nv12 => Some((vk::Format::G8_B8R8_2PLANE_420_UNORM, false)),
        Fourcc::Yuv420 => Some((vk::Format::G8_B8_R8_3PLANE_420_UNORM, false)),
        Fourcc::P010 => Some((vk::Format::G10X6_B10X6R10X6_2PLANE_420_UNORM_3PACK16, false)),
        _ => None,
    }
}

/// Vulkan DMA-BUF Importer.
#[derive(Clone)]
pub struct DmabufImporter {
    ctx: Arc<VulkanContext>,
    pipeline_mgr: Arc<VulkanPipelineManager>,
    memory_props: vk::PhysicalDeviceMemoryProperties,
    cache: Rc<RefCell<HashMap<WeakDmabuf, CachedDmabufTexture>>>,
    cache_lookups: Rc<Cell<u64>>,
    #[cfg(test)]
    image_imports: Rc<Cell<u64>>,
}

impl DmabufImporter {
    pub(super) fn new(
        ctx: Arc<VulkanContext>,
        cache: Rc<RefCell<HashMap<WeakDmabuf, CachedDmabufTexture>>>,
        pipeline_mgr: Arc<VulkanPipelineManager>,
    ) -> Self {
        let memory_props = unsafe {
            ctx.instance
                .get_physical_device_memory_properties(ctx.physical_device)
        };
        Self {
            ctx,
            pipeline_mgr,
            memory_props,
            cache,
            cache_lookups: Rc::new(Cell::new(0)),
            #[cfg(test)]
            image_imports: Rc::new(Cell::new(0)),
        }
    }

    /// Import a Wayland client DMA-BUF into a VulkanTexture with default sampled usage.
    pub fn import_dmabuf(&self, dmabuf: &Dmabuf) -> Result<VulkanTexture, VulkanImportError> {
        let tick = self.cache_lookups.get().wrapping_add(1);
        self.cache_lookups.set(tick);
        // Hits are cheap and the table is bounded. Expired producer keys are
        // swept periodically on hits, and before every miss/new allocation.
        if tick.is_multiple_of(128) {
            self.prune_expired_imports();
        }
        let weak = dmabuf.weak();
        if let Some(tex) = self.cache.borrow_mut().get_mut(&weak).map(|cached| {
            cached.last_used = tick;
            cached.texture.clone()
        }) {
            return Ok(tex);
        }
        self.prune_expired_imports();
        let yuv = is_yuv_format(dmabuf.format().code);
        let tex = self.import_dmabuf_with_usage(
            dmabuf,
            if yuv {
                vk::ImageUsageFlags::SAMPLED
            } else {
                vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_SRC
            },
        )?;
        let mut discarded = Vec::new();
        {
            let mut cache = self.cache.borrow_mut();
            let mut oldest = cache
                .iter()
                .map(|(key, cached)| {
                    (
                        key.clone(),
                        tick.wrapping_sub(cached.last_used),
                        cached.texture.inner.allocation_size,
                    )
                })
                .collect::<Vec<_>>();
            oldest.sort_unstable_by_key(|(_, age, _)| Reverse(*age));
            if let Some(evictions) = import_cache_evictions(
                oldest.iter().map(|(_, _, bytes)| *bytes),
                tex.inner.allocation_size,
            ) {
                for (key, _, _) in oldest.into_iter().take(evictions) {
                    discarded.extend(cache.remove(&key));
                }
                discarded.extend(cache.insert(weak, CachedDmabufTexture::new(&tex, tick)));
            }
        }
        // Driver destruction must not hold the import-table borrow. In-flight
        // frames keep their own texture reference regardless of cache eviction.
        drop(discarded);
        Ok(tex)
    }

    fn prune_expired_imports(&self) {
        let dead = self
            .cache
            .borrow()
            .keys()
            .filter(|key| key.is_gone())
            .cloned()
            .collect::<Vec<_>>();
        let removed = {
            let mut cache = self.cache.borrow_mut();
            dead.iter()
                .filter_map(|key| cache.remove(key))
                .collect::<Vec<_>>()
        };
        drop(removed);
    }

    #[cfg(test)]
    pub(super) fn image_import_count(&self) -> u64 {
        self.image_imports.get()
    }

    #[cfg(test)]
    pub(super) fn cached_allocation_sizes(&self) -> Vec<u64> {
        self.cache
            .borrow()
            .values()
            .map(|cached| cached.texture.inner.allocation_size)
            .collect()
    }

    #[cfg(test)]
    pub(super) fn sweep_expired_imports(&self) {
        self.prune_expired_imports();
    }

    /// Import a DMA-BUF with custom usage flags (e.g. COLOR_ATTACHMENT for scanout targets).
    pub fn import_dmabuf_with_usage(
        &self,
        dmabuf: &Dmabuf,
        usage: vk::ImageUsageFlags,
    ) -> Result<VulkanTexture, VulkanImportError> {
        // Measure actual driver imports, not the cheap shared-texture lookup.
        let _latency =
            crate::utils::latency::Span::new(crate::utils::latency::Operation::VulkanImport);
        // A Vulkan context can intentionally run without Linux DMA-BUF
        // interop (for example nested WSI on a driver that only exposes core
        // Vulkan).  Do not issue DRM-modifier/external-memory pNext queries
        // in that mode; callers can continue with SHM and internally-owned
        // optimal-tiling images instead.
        if !self.ctx.supports_dmabuf_import {
            return Err(VulkanImportError::UnsupportedExtension);
        }
        let (format, has_alpha) = fourcc_to_vk_format(dmabuf.format().code)
            .ok_or(VulkanImportError::UnsupportedFormat(dmabuf.format().code))?;
        let yuv = is_yuv_format(dmabuf.format().code);
        if yuv && (!self.ctx.supports_ycbcr_conversion || usage != vk::ImageUsageFlags::SAMPLED) {
            return Err(VulkanImportError::UnsupportedExtension);
        }

        let width = dmabuf.width();
        let height = dmabuf.height();
        let num_planes = dmabuf.num_planes();
        let modifier = u64::from(dmabuf.format().modifier);

        let offsets: Vec<u32> = dmabuf.offsets().collect();
        let strides: Vec<u32> = dmabuf.strides().collect();
        let handles: Vec<_> = dmabuf.handles().collect();

        // Buffer::width/height cast signed dimensions to u32. Reject negative
        // sizes before they become enormous Vulkan extents.
        if dmabuf.size().w <= 0
            || dmabuf.size().h <= 0
            || handles.is_empty()
            || num_planes != handles.len()
            || num_planes != offsets.len()
            || num_planes != strides.len()
            || strides.contains(&0)
            || modifier == u64::from(smithay::backend::allocator::Modifier::Invalid)
        {
            return Err(VulkanImportError::InvalidLayout);
        }
        // A distinct FD for each memory plane requires a disjoint VkImage and
        // per-plane vkBindImageMemory2. Duplicate FDs for one allocation use the
        // simpler non-disjoint binding path.
        let fd_identity = |fd: i32| -> Result<(u64, u64), VulkanImportError> {
            let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
            if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } != 0 {
                return Err(VulkanImportError::InvalidLayout);
            }
            let stat = unsafe { stat.assume_init() };
            Ok((stat.st_dev, stat.st_ino))
        };
        let first = fd_identity(handles[0].as_raw_fd())?;
        let identities = handles[1..]
            .iter()
            .map(|fd| fd_identity(fd.as_raw_fd()))
            .collect::<Result<Vec<_>, _>>()?;
        let distinct_fds = identities.iter().any(|identity| *identity != first);
        if distinct_fds && (!yuv || num_planes != yuv_plane_count(format)) {
            return Err(VulkanImportError::InvalidLayout);
        }
        if yuv && modifier == u64::from(smithay::backend::allocator::Modifier::Linear) {
            let mut all_identities = Vec::with_capacity(num_planes);
            all_identities.push(first);
            all_identities.extend(identities.iter().copied());
            if !valid_linear_yuv_layout(format, width, height, &offsets, &strides, &all_identities)
            {
                return Err(VulkanImportError::InvalidLayout);
            }
        }
        let mut external_props = vk::ExternalImageFormatProperties::default();
        let mut ycbcr_props = vk::SamplerYcbcrConversionImageFormatProperties::default();
        let mut format_props = vk::ImageFormatProperties2::default()
            .push_next(&mut external_props)
            .push_next(&mut ycbcr_props);
        let mut modifier_query = vk::PhysicalDeviceImageDrmFormatModifierInfoEXT::default()
            .drm_format_modifier(modifier)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let mut external_query = vk::PhysicalDeviceExternalImageFormatInfo::default()
            .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let query = vk::PhysicalDeviceImageFormatInfo2::default()
            .format(format)
            .ty(vk::ImageType::TYPE_2D)
            .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .flags(if distinct_fds {
                vk::ImageCreateFlags::DISJOINT
            } else {
                vk::ImageCreateFlags::empty()
            })
            .usage(usage)
            .push_next(&mut modifier_query)
            .push_next(&mut external_query);
        unsafe {
            self.ctx
                .instance
                .get_physical_device_image_format_properties2(
                    self.ctx.physical_device,
                    &query,
                    &mut format_props,
                )?;
        }
        if width > format_props.image_format_properties.max_extent.width
            || height > format_props.image_format_properties.max_extent.height
        {
            return Err(VulkanImportError::InvalidLayout);
        }
        if !external_props
            .external_memory_properties
            .external_memory_features
            .contains(vk::ExternalMemoryFeatureFlags::IMPORTABLE)
        {
            return Err(VulkanImportError::UnsupportedExtension);
        }
        if distinct_fds
            && external_props
                .external_memory_properties
                .external_memory_features
                .contains(vk::ExternalMemoryFeatureFlags::DEDICATED_ONLY)
        {
            // Dedicated image allocations cannot bind a disjoint plane.
            return Err(VulkanImportError::UnsupportedExtension);
        }
        if yuv
            && !(1..=super::pipeline::MAX_YUV_DESCRIPTORS_PER_SET)
                .contains(&ycbcr_props.combined_image_sampler_descriptor_count)
        {
            return Err(VulkanImportError::UnsupportedExtension);
        }
        let mut modifier_list = vk::DrmFormatModifierPropertiesListEXT::default();
        let mut props = vk::FormatProperties2::default().push_next(&mut modifier_list);
        unsafe {
            self.ctx.instance.get_physical_device_format_properties2(
                self.ctx.physical_device,
                format,
                &mut props,
            );
        }
        let mut modifiers = vec![
            vk::DrmFormatModifierPropertiesEXT::default();
            modifier_list.drm_format_modifier_count as usize
        ];
        modifier_list = modifier_list.drm_format_modifier_properties(&mut modifiers);
        let mut props = vk::FormatProperties2::default().push_next(&mut modifier_list);
        unsafe {
            self.ctx.instance.get_physical_device_format_properties2(
                self.ctx.physical_device,
                format,
                &mut props,
            );
        }
        let modifier_props = modifiers
            .iter()
            .find(|m| {
                m.drm_format_modifier == modifier
                    && m.drm_format_modifier_plane_count as usize == num_planes
            })
            .ok_or(VulkanImportError::InvalidLayout)?;
        let features = modifier_props.drm_format_modifier_tiling_features;
        if distinct_fds && !features.contains(vk::FormatFeatureFlags::DISJOINT) {
            return Err(VulkanImportError::UnsupportedExtension);
        }
        let yuv_pipeline = if yuv {
            if !features.contains(vk::FormatFeatureFlags::SAMPLED_IMAGE) {
                return Err(VulkanImportError::UnsupportedExtension);
            }
            let chroma = if features.contains(vk::FormatFeatureFlags::MIDPOINT_CHROMA_SAMPLES) {
                vk::ChromaLocation::MIDPOINT
            } else if features.contains(vk::FormatFeatureFlags::COSITED_CHROMA_SAMPLES) {
                vk::ChromaLocation::COSITED_EVEN
            } else {
                return Err(VulkanImportError::UnsupportedExtension);
            };
            let filter = if features.contains(
                vk::FormatFeatureFlags::SAMPLED_IMAGE_FILTER_LINEAR
                    | vk::FormatFeatureFlags::SAMPLED_IMAGE_YCBCR_CONVERSION_LINEAR_FILTER,
            ) {
                vk::Filter::LINEAR
            } else {
                vk::Filter::NEAREST
            };
            // linux-dmabuf itself carries no matrix/range metadata. Follow the
            // conventional SDR video policy until color-management metadata is
            // threaded through the surface importer.
            let model = if height <= 576 {
                vk::SamplerYcbcrModelConversion::YCBCR_601
            } else {
                vk::SamplerYcbcrModelConversion::YCBCR_709
            };
            Some(self.pipeline_mgr.yuv_pipeline(YuvKey {
                format,
                model,
                chroma,
                filter,
            })?)
        } else {
            None
        };

        // Build plane layouts for explicit DRM modifiers
        let mut plane_layouts = Vec::with_capacity(num_planes);
        for i in 0..num_planes {
            let offset = offsets[i] as vk::DeviceSize;
            let row_pitch = strides[i] as vk::DeviceSize;
            plane_layouts.push(vk::SubresourceLayout {
                offset,
                size: 0,
                row_pitch,
                array_pitch: 0,
                depth_pitch: 0,
            });
        }

        let mut modifier_info = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default()
            .drm_format_modifier(modifier)
            .plane_layouts(&plane_layouts);

        let mut external_memory_info = vk::ExternalMemoryImageCreateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);

        let mut image_create_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .flags(if distinct_fds {
                vk::ImageCreateFlags::DISJOINT
            } else {
                vk::ImageCreateFlags::empty()
            })
            .extent(vk::Extent3D {
                width,
                height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);

        image_create_info = image_create_info.push_next(&mut external_memory_info);
        image_create_info = image_create_info.push_next(&mut modifier_info);

        let mut gpu = TextureGpu::empty(self.ctx.clone());
        gpu.external = true;
        gpu.current_layout.set(vk::ImageLayout::GENERAL);
        gpu.usage = usage;
        let device = &self.ctx.device;
        gpu.image = unsafe { device.create_image(&image_create_info, None) }?;
        #[cfg(test)]
        self.image_imports
            .set(self.image_imports.get().saturating_add(1));
        let fd_api = ash::khr::external_memory_fd::Device::new(&self.ctx.instance, device);
        if distinct_fds {
            let aspects = [
                vk::ImageAspectFlags::MEMORY_PLANE_0_EXT,
                vk::ImageAspectFlags::MEMORY_PLANE_1_EXT,
                vk::ImageAspectFlags::MEMORY_PLANE_2_EXT,
            ];
            let mut memories = Vec::with_capacity(num_planes);
            for (fd_handle, aspect) in handles.iter().zip(aspects) {
                let mut plane =
                    vk::ImagePlaneMemoryRequirementsInfo::default().plane_aspect(aspect);
                let info = vk::ImageMemoryRequirementsInfo2::default()
                    .image(gpu.image)
                    .push_next(&mut plane);
                let mut requirements = vk::MemoryRequirements2::default();
                unsafe { device.get_image_memory_requirements2(&info, &mut requirements) };
                gpu.allocation_size = gpu
                    .allocation_size
                    .checked_add(requirements.memory_requirements.size)
                    .ok_or(VulkanImportError::InvalidLayout)?;
                let memory = self.import_fd_memory(
                    &fd_api,
                    fd_handle,
                    requirements.memory_requirements,
                    None,
                    yuv && modifier == u64::from(Modifier::Linear),
                )?;
                if gpu.memory == vk::DeviceMemory::null() {
                    gpu.memory = memory;
                } else {
                    gpu.extra_memories.push(memory);
                }
                memories.push(memory);
            }
            let mut planes = aspects[..num_planes]
                .iter()
                .copied()
                .map(|aspect| vk::BindImagePlaneMemoryInfo::default().plane_aspect(aspect))
                .collect::<Vec<_>>();
            let binds = planes
                .iter_mut()
                .zip(memories.iter())
                .map(|(plane, memory)| {
                    vk::BindImageMemoryInfo::default()
                        .image(gpu.image)
                        .memory(*memory)
                        .memory_offset(0)
                        .push_next(plane)
                })
                .collect::<Vec<_>>();
            unsafe { device.bind_image_memory2(&binds)? };
        } else {
            let requirements = unsafe { device.get_image_memory_requirements(gpu.image) };
            gpu.allocation_size = requirements.size;
            gpu.memory = self.import_fd_memory(
                &fd_api,
                &handles[0],
                requirements,
                Some(gpu.image),
                yuv && modifier == u64::from(Modifier::Linear),
            )?;
            unsafe { device.bind_image_memory(gpu.image, gpu.memory, 0)? };
        }

        if let Some(yuv_pipeline) = yuv_pipeline {
            let mut conversion =
                vk::SamplerYcbcrConversionInfo::default().conversion(yuv_pipeline.conversion);
            let view = vk::ImageViewCreateInfo::default()
                .image(gpu.image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(format)
                .subresource_range(super::frame::color_range())
                .push_next(&mut conversion);
            gpu.image_view = unsafe { device.create_image_view(&view, None) }?;
            gpu.yuv_pipeline = Some(yuv_pipeline);
        } else {
            gpu.create_views(format, has_alpha, usage)?;
        }
        Ok(VulkanTexture {
            width,
            height,
            format: format.as_raw() as u32,
            has_alpha,
            // GLES imports keep the same flag; GL clients submit Y_INVERTED
            // DMA-BUFs that must be drawn flipped to appear upright.
            y_inverted: dmabuf.y_inverted(),
            inner: Arc::new(gpu),
            client_buffer: None,
        })
    }

    fn find_memory_type(
        &self,
        type_filter: u32,
        properties: vk::MemoryPropertyFlags,
    ) -> Option<u32> {
        (0..self.memory_props.memory_type_count).find(|&i| {
            (type_filter & (1 << i)) != 0
                && (self.memory_props.memory_types[i as usize].property_flags & properties)
                    == properties
        })
    }

    fn import_fd_memory(
        &self,
        fd_api: &ash::khr::external_memory_fd::Device,
        handle: &std::os::fd::BorrowedFd<'_>,
        requirements: vk::MemoryRequirements,
        dedicated_image: Option<vk::Image>,
        validate_linear_yuv_size: bool,
    ) -> Result<vk::DeviceMemory, VulkanImportError> {
        if validate_linear_yuv_size {
            // A valid EGL allocation can still be smaller than this Vulkan
            // image's actual requirements. Reject before duplicating/importing
            // its FD; this is an interop constraint, not GPU memory exhaustion.
            let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
            let available = (unsafe { libc::fstat(handle.as_raw_fd(), stat.as_mut_ptr()) } == 0)
                .then(|| unsafe { stat.assume_init() }.st_size)
                .and_then(|size| u64::try_from(size).ok())
                .filter(|size| *size != 0);
            validate_linear_yuv_allocation_size(available, requirements.size)?;
        }
        let mut fd_props = vk::MemoryFdPropertiesKHR::default();
        unsafe {
            fd_api.get_memory_fd_properties(
                vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT,
                handle.as_raw_fd(),
                &mut fd_props,
            )?
        };
        let memory_type_index = self
            .find_memory_type(
                requirements.memory_type_bits & fd_props.memory_type_bits,
                vk::MemoryPropertyFlags::empty(),
            )
            .ok_or(VulkanImportError::NoMatchingMemoryType)?;
        // A prior NVIDIA probe consumed a dedicated-import FD on failure,
        // while its non-dedicated failure retained it. Do not close an uncertain
        // raw number: it may already refer to another thread's unrelated file.
        // Reserve before duplication; ambiguous failures permanently occupy one
        // of 64 processwide slots, bounding this compatibility quarantine.
        super::fd_import::import(
            || {
                handle
                    .try_clone_to_owned()
                    .map_err(|_| VulkanImportError::InvalidLayout)
            },
            |raw| {
                let mut import = vk::ImportMemoryFdInfoKHR::default()
                    .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT)
                    .fd(raw);
                let mut dedicated = vk::MemoryDedicatedAllocateInfo::default()
                    .image(dedicated_image.unwrap_or(vk::Image::null()));
                let mut alloc = vk::MemoryAllocateInfo::default()
                    .allocation_size(requirements.size)
                    .memory_type_index(memory_type_index)
                    .push_next(&mut import);
                if dedicated_image.is_some() {
                    alloc = alloc.push_next(&mut dedicated);
                }
                unsafe { self.ctx.device.allocate_memory(&alloc, None) }
                    .map_err(VulkanImportError::from)
            },
        )
        .map_err(|error| match error {
            super::fd_import::ImportError::Exhausted => {
                VulkanImportError::ImportFailureBudgetExceeded
            }
            super::fd_import::ImportError::Operation(error) => error,
        })
    }
}

fn validate_linear_yuv_allocation_size(
    available: Option<u64>,
    required: u64,
) -> Result<(), VulkanImportError> {
    if let Some(available) = available.filter(|available| *available != 0 && *available < required)
    {
        return Err(VulkanImportError::AllocationTooSmall {
            available,
            required,
        });
    }
    Ok(())
}

pub(super) fn is_yuv_format(fourcc: Fourcc) -> bool {
    matches!(fourcc, Fourcc::Nv12 | Fourcc::Yuv420 | Fourcc::P010)
}

pub(super) fn yuv_plane_count(format: vk::Format) -> usize {
    if format == vk::Format::G8_B8_R8_3PLANE_420_UNORM {
        3
    } else {
        2
    }
}

fn valid_linear_yuv_layout(
    format: vk::Format,
    width: u32,
    height: u32,
    offsets: &[u32],
    strides: &[u32],
    identities: &[(u64, u64)],
) -> bool {
    let half_width = u64::from(width).div_ceil(2);
    let half_height = u64::from(height).div_ceil(2);
    let (min_pitches, rows): (Vec<u64>, Vec<u64>) = match format {
        vk::Format::G8_B8R8_2PLANE_420_UNORM => (
            vec![u64::from(width), half_width * 2],
            vec![u64::from(height), half_height],
        ),
        vk::Format::G8_B8_R8_3PLANE_420_UNORM => (
            vec![u64::from(width), half_width, half_width],
            vec![u64::from(height), half_height, half_height],
        ),
        vk::Format::G10X6_B10X6R10X6_2PLANE_420_UNORM_3PACK16 => (
            vec![u64::from(width) * 2, half_width * 4],
            vec![u64::from(height), half_height],
        ),
        _ => return false,
    };
    if offsets.len() != min_pitches.len()
        || strides.len() != min_pitches.len()
        || identities.len() != min_pitches.len()
    {
        return false;
    }
    let mut ranges = Vec::with_capacity(min_pitches.len());
    for i in 0..min_pitches.len() {
        if u64::from(strides[i]) < min_pitches[i] {
            return false;
        }
        let Some(end) = u64::from(strides[i])
            .checked_mul(rows[i])
            .and_then(|span| u64::from(offsets[i]).checked_add(span))
        else {
            return false;
        };
        ranges.push((u64::from(offsets[i]), end));
    }
    for i in 0..ranges.len() {
        for j in i + 1..ranges.len() {
            if identities[i] == identities[j]
                && ranges[i].0 < ranges[j].1
                && ranges[j].0 < ranges[i].1
            {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod cache_tests {
    use super::*;

    #[test]
    fn linear_yuv_payload_size_obeys_requirements_and_delegates_unknown_sizes() {
        assert!(matches!(
            validate_linear_yuv_allocation_size(Some(4096), 131072),
            Err(VulkanImportError::AllocationTooSmall {
                available: 4096,
                required: 131072
            })
        ));
        for available in [None, Some(0), Some(131072), Some(131073)] {
            assert!(validate_linear_yuv_allocation_size(available, 131072).is_ok());
        }
    }

    #[test]
    fn import_cache_bounds_actual_allocations_count_and_oversized_bypass() {
        let mib = 1024 * 1024;
        // Logical pixels would fit, but driver padding/multiple memory planes
        // make two existing 96 MiB allocations plus a third exceed 256 MiB.
        assert_eq!(
            import_cache_evictions([96 * mib, 96 * mib].into_iter(), 96 * mib),
            Some(1)
        );
        assert_eq!(
            import_cache_evictions([4096; MAX_CACHED_IMPORTS].into_iter(), 4096),
            Some(1)
        );
        assert_eq!(
            import_cache_evictions([1].into_iter(), MAX_CACHED_IMPORT_BYTES),
            Some(1)
        );
        assert_eq!(
            import_cache_evictions([1].into_iter(), MAX_CACHED_IMPORT_BYTES + 1),
            None
        );
        assert_eq!(
            import_cache_evictions([u64::MAX, u64::MAX].into_iter(), 4096),
            Some(2)
        );

        let mut allocations = Vec::new();
        for incoming in [1, 96 * mib, 96 * mib, 96 * mib, 257 * mib]
            .into_iter()
            .chain(std::iter::repeat_n(4096, 80))
        {
            let Some(evictions) = import_cache_evictions(allocations.iter().copied(), incoming)
            else {
                assert!(incoming > MAX_CACHED_IMPORT_BYTES);
                continue;
            };
            if evictions != 0 {
                let fewer = &allocations[evictions - 1..];
                assert!(
                    fewer.len() + 1 > MAX_CACHED_IMPORTS
                        || fewer.iter().sum::<u64>() + incoming > MAX_CACHED_IMPORT_BYTES
                );
            }
            drop(allocations.drain(..evictions));
            allocations.push(incoming);
            assert!(allocations.len() <= MAX_CACHED_IMPORTS);
            assert!(allocations.iter().sum::<u64>() <= MAX_CACHED_IMPORT_BYTES);
        }
        assert_eq!(allocations, [4096; MAX_CACHED_IMPORTS]);
    }
}
