//! Vulkan Device & Context Management for Realand.
//!
//! Handles VkInstance creation, physical device selection matching DRM KMS nodes,
//! extension verification, and logical VkDevice creation.

use std::ffi::{CStr, CString};
use std::fmt;
use std::sync::Arc;

use ash::vk;
use ash::{Device, Entry, Instance};
use libc::{major, minor};
use smithay::backend::drm::DrmNode;

use super::VulkanBackendInfo;

const EXT_SURFACE: &CStr = c"VK_KHR_surface";
const EXT_WAYLAND_SURFACE: &CStr = c"VK_KHR_wayland_surface";
const EXT_XLIB_SURFACE: &CStr = c"VK_KHR_xlib_surface";
const EXT_XCB_SURFACE: &CStr = c"VK_KHR_xcb_surface";
const EXT_GET_SURFACE_CAPABILITIES2: &CStr = ash::khr::get_surface_capabilities2::NAME;
const EXT_SURFACE_MAINTENANCE1: &CStr = ash::ext::surface_maintenance1::NAME;
const EXT_SWAPCHAIN_MAINTENANCE1: &CStr = ash::ext::swapchain_maintenance1::NAME;

const EXT_EXTERNAL_MEMORY_DMA_BUF: &CStr = c"VK_EXT_external_memory_dma_buf";
const EXT_QUEUE_FAMILY_FOREIGN: &CStr = c"VK_EXT_queue_family_foreign";
const EXT_EXTERNAL_MEMORY_FD: &CStr = c"VK_KHR_external_memory_fd";
const EXT_IMAGE_DRM_FORMAT_MODIFIER: &CStr = c"VK_EXT_image_drm_format_modifier";

/// Native window handles borrowed from a live Winit window. The caller keeps
/// both the display connection and the window alive for the WSI lifetime.
#[derive(Clone, Copy, Debug)]
pub enum VulkanSurfaceHandle {
    Wayland {
        display: *mut std::ffi::c_void,
        surface: *mut std::ffi::c_void,
    },
    Xlib {
        display: *mut std::ffi::c_void,
        window: u64,
    },
    Xcb {
        connection: *mut std::ffi::c_void,
        window: u32,
    },
}

impl VulkanSurfaceHandle {
    fn extension(self) -> &'static CStr {
        match self {
            Self::Wayland { .. } => EXT_WAYLAND_SURFACE,
            Self::Xlib { .. } => EXT_XLIB_SURFACE,
            Self::Xcb { .. } => EXT_XCB_SURFACE,
        }
    }

    /// # Safety
    /// The borrowed display and window handles must remain valid.
    pub unsafe fn create_surface(
        self,
        entry: &Entry,
        instance: &Instance,
    ) -> Result<vk::SurfaceKHR, vk::Result> {
        match self {
            Self::Wayland { display, surface } => {
                let api = ash::khr::wayland_surface::Instance::new(entry, instance);
                api.create_wayland_surface(
                    &vk::WaylandSurfaceCreateInfoKHR::default()
                        .display(display.cast())
                        .surface(surface.cast()),
                    None,
                )
            }
            Self::Xlib { display, window } => {
                let api = ash::khr::xlib_surface::Instance::new(entry, instance);
                api.create_xlib_surface(
                    &vk::XlibSurfaceCreateInfoKHR::default()
                        .dpy(display.cast())
                        .window(window as _),
                    None,
                )
            }
            Self::Xcb { connection, window } => {
                let api = ash::khr::xcb_surface::Instance::new(entry, instance);
                api.create_xcb_surface(
                    &vk::XcbSurfaceCreateInfoKHR::default()
                        .connection(connection.cast())
                        .window(window),
                    None,
                )
            }
        }
    }
}

#[derive(Debug)]
pub enum VulkanInitError {
    LoadError(ash::LoadingError),
    VkError(vk::Result),
    NoMatchingPhysicalDevice,
    MissingExtension(String),
}

impl fmt::Display for VulkanInitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LoadError(err) => write!(f, "failed to load Vulkan library: {err}"),
            Self::VkError(err) => write!(f, "Vulkan driver error: {err}"),
            Self::NoMatchingPhysicalDevice => write!(
                f,
                "no suitable Vulkan physical device found matching DRM node"
            ),
            Self::MissingExtension(ext) => write!(f, "required Vulkan extension missing: {ext}"),
        }
    }
}

impl std::error::Error for VulkanInitError {}

impl From<ash::LoadingError> for VulkanInitError {
    fn from(err: ash::LoadingError) -> Self {
        Self::LoadError(err)
    }
}

impl From<vk::Result> for VulkanInitError {
    fn from(err: vk::Result) -> Self {
        Self::VkError(err)
    }
}

/// Host-visible staging buffer with allocated device memory and byte capacity.
pub struct StagingBuffer {
    pub buffer: vk::Buffer,
    pub memory: vk::DeviceMemory,
    pub size: usize,
}

// Keep enough upload space for several 4K SHM frames without pinning arbitrary
// large host-visible allocations after a one-off image or mesh has gone away.
const MAX_BUFFER_POOL_ENTRIES: usize = 8;
const MAX_STAGING_POOL_BYTES: usize = 128 * 1024 * 1024;
const MAX_VERTEX_POOL_BYTES: usize = 8 * 1024 * 1024;

fn best_fit_buffer(pool: &[StagingBuffer], size: usize) -> Option<usize> {
    pool.iter()
        .enumerate()
        .filter(|(_, buffer)| buffer.size >= size)
        .min_by_key(|(_, buffer)| buffer.size)
        .map(|(index, _)| index)
}

fn make_room_for_buffer(
    pool: &mut Vec<StagingBuffer>,
    size: usize,
    max_bytes: usize,
) -> Vec<StagingBuffer> {
    let mut evicted = Vec::new();
    let mut total = pool.iter().map(|buffer| buffer.size as u128).sum::<u128>();
    while pool.len() >= MAX_BUFFER_POOL_ENTRIES || total + size as u128 > max_bytes as u128 {
        // Preserve small, frequently reused allocations ahead of a stale
        // oversized allocation when the budget is full.
        let index = pool
            .iter()
            .enumerate()
            .max_by_key(|(_, buffer)| buffer.size)
            .map(|(index, _)| index)
            .expect("an over-budget pool cannot be empty");
        let removed = pool.swap_remove(index);
        total -= removed.size as u128;
        evicted.push(removed);
    }
    evicted
}

/// Vulkan rendering context holding instance, physical device, and logical device.
pub struct VulkanContext {
    pub entry: Entry,
    pub instance: Instance,
    pub physical_device: vk::PhysicalDevice,
    pub device: Device,
    pub graphics_queue: vk::Queue,
    pub graphics_queue_family: u32,
    pub command_pool: vk::CommandPool,
    pub supports_sync_file_import: bool,
    pub supports_sync_file_export: bool,
    /// Whether the device can import Linux DMA-BUFs as explicit DRM-modifier
    /// images.  Vulkan itself remains useful without this capability (SHM and
    /// internally allocated images use ordinary optimal-tiling images), so it
    /// is intentionally a capability bit rather than an initialization
    /// requirement.
    pub supports_dmabuf_import: bool,
    /// Whether `VK_EXT_image_drm_format_modifier` is available.  This is kept
    /// separate from `supports_dmabuf_import` for diagnostics and format
    /// negotiation: importing a DMA-BUF also needs external-memory and
    /// foreign-queue support.
    pub supports_drm_modifiers: bool,
    /// Whether the WSI present-fence extension and feature are enabled.
    /// `VulkanWsi` uses a queue-idle retirement path when this is false.
    pub supports_swapchain_maintenance: bool,
    pub supports_ycbcr_conversion: bool,
    pub(super) command_lock: std::sync::Mutex<()>,
    pub(super) idle_commands: std::sync::Mutex<Vec<(vk::CommandBuffer, vk::Fence)>>,
    pub(super) staging_buffers: std::sync::Mutex<Vec<StagingBuffer>>,
    pub(super) vertex_buffers: std::sync::Mutex<Vec<StagingBuffer>>,
    pub info: VulkanBackendInfo,
    pub physical_device_properties: vk::PhysicalDeviceProperties,
}

unsafe impl Send for VulkanContext {}
unsafe impl Sync for VulkanContext {}

/// Serializes process-wide Vulkan instance creation.
///
/// `vkCreateInstance` makes the Vulkan loader scan and `dlopen` every installed ICD.
/// Some proprietary ICD libraries crash when the loader scans them concurrently from
/// multiple threads (observed with a broken `libGLX_nvidia.so.0` ICD: parallel
/// initialization segfaults before any error can be reported, while serialized
/// initialization fails gracefully). The unit-test suite and device-recovery paths
/// may both initialize Vulkan more than once, so guard the whole probe.
static VULKAN_INIT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

impl VulkanContext {
    /// Initialize Vulkan context, optionally matching a DRM node (e.g. /dev/dri/cardX or renderD128).
    pub fn init(drm_node: Option<&DrmNode>) -> Result<Arc<Self>, VulkanInitError> {
        Self::init_inner(drm_node, None)
    }

    /// Select a GPU and graphics queue that can present to this live host
    /// window, and enable the swapchain extension on the same Vulkan device.
    pub fn init_for_surface(surface: VulkanSurfaceHandle) -> Result<Arc<Self>, VulkanInitError> {
        Self::init_inner(None, Some(surface))
    }

    fn init_inner(
        drm_node: Option<&DrmNode>,
        surface_handle: Option<VulkanSurfaceHandle>,
    ) -> Result<Arc<Self>, VulkanInitError> {
        // Poisoning would only mean another initializer panicked while holding the
        // lock; the loader state is still consistent enough to retry.
        let _init_guard = VULKAN_INIT_LOCK
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let entry = unsafe { Entry::load() }?;

        let app_name = CString::new("Realand Compositor").unwrap();
        let engine_name = CString::new("Realand Vulkan Engine").unwrap();

        let app_info = vk::ApplicationInfo::default()
            .application_name(&app_name)
            .application_version(vk::make_api_version(0, 0, 1, 0))
            .engine_name(&engine_name)
            .engine_version(vk::make_api_version(0, 0, 1, 0))
            .api_version(vk::API_VERSION_1_3);

        let (required_instance_extensions, optional_instance_extensions): (
            Vec<&'static CStr>,
            Vec<&'static CStr>,
        ) = if let Some(surface) = surface_handle {
            // The capabilities2 and surface-maintenance extensions are useful on
            // some drivers but are not used by the WSI path.  Treating them as
            // mandatory made Vulkan fail to initialize on otherwise valid
            // Wayland/X11 implementations that expose only the core surface
            // extension and their platform extension.
            (
                vec![EXT_SURFACE, surface.extension()],
                vec![EXT_GET_SURFACE_CAPABILITIES2, EXT_SURFACE_MAINTENANCE1],
            )
        } else {
            // DRM and headless paths never create a VkSurface.  Keep their
            // instance requirements limited to Vulkan core so a render-only
            // driver without window-system extensions can still run them.
            (Vec::new(), Vec::new())
        };

        // Probe available instance extensions to avoid requesting unsupported ones
        let available_instance_exts =
            unsafe { entry.enumerate_instance_extension_properties(None) }?;
        for ext in &required_instance_extensions {
            if !available_instance_exts
                .iter()
                .any(|avail| unsafe { CStr::from_ptr(avail.extension_name.as_ptr()) == *ext })
            {
                return Err(VulkanInitError::MissingExtension(
                    ext.to_string_lossy().into_owned(),
                ));
            }
        }
        let filtered_instance_names: Vec<&'static CStr> = required_instance_extensions
            .into_iter()
            .chain(optional_instance_extensions)
            .filter(|&ext| {
                available_instance_exts.iter().any(|avail| {
                    let avail_cstr = unsafe { CStr::from_ptr(avail.extension_name.as_ptr()) };
                    avail_cstr == ext
                })
            })
            .collect();
        let filtered_instance_exts: Vec<*const i8> = filtered_instance_names
            .iter()
            .map(|ext| ext.as_ptr())
            .collect();
        // VK_EXT_swapchain_maintenance1 has an instance-extension dependency
        // on VK_EXT_surface_maintenance1.  Do not enable its device feature
        // unless the dependency was actually enabled above.
        let instance_surface_maintenance1 =
            filtered_instance_names.contains(&EXT_SURFACE_MAINTENANCE1);

        let create_info = vk::InstanceCreateInfo::default()
            .application_info(&app_info)
            .enabled_extension_names(&filtered_instance_exts);

        let instance = unsafe { entry.create_instance(&create_info, None) }?;

        // Release the instance on every partial initialization failure.
        let mut instance_guard = InstanceDropGuard(Some(instance));
        let instance = instance_guard.0.as_ref().expect("instance just wrapped");

        // Select physical device
        let physical_devices = unsafe { instance.enumerate_physical_devices() }?;
        let (physical_device, graphics_queue_family) = if let Some(handle) = surface_handle {
            // Destroy even on selection failure; the window's own surface is
            // recreated after device initialization for the swapchain.
            let surface = unsafe { handle.create_surface(&entry, instance)? };
            let api = ash::khr::surface::Instance::new(&entry, instance);
            let selected = Self::select_present_device(instance, &physical_devices, &api, surface);
            unsafe { api.destroy_surface(surface, None) };
            selected?
        } else {
            let physical_device =
                Self::select_physical_device(instance, &physical_devices, drm_node)?;
            let queue_props =
                unsafe { instance.get_physical_device_queue_family_properties(physical_device) };
            let family = queue_props
                .iter()
                .position(|q| q.queue_flags.contains(vk::QueueFlags::GRAPHICS))
                .map(|idx| idx as u32)
                .ok_or(VulkanInitError::NoMatchingPhysicalDevice)?;
            (physical_device, family)
        };

        let props = unsafe { instance.get_physical_device_properties(physical_device) };
        let device_name = unsafe { CStr::from_ptr(props.device_name.as_ptr()) }
            .to_str()
            .unwrap_or("Unknown Vulkan Device");

        debug!("selected Vulkan physical device: {device_name}");

        if props.api_version < vk::API_VERSION_1_3 {
            return Err(VulkanInitError::MissingExtension("Vulkan 1.3".into()));
        }
        // The quad pipeline layout is the largest push-constant user (236 bytes).
        if props.limits.max_push_constants_size
            < std::mem::size_of::<super::pipeline::QuadPushConstants>() as u32
        {
            return Err(VulkanInitError::MissingExtension(
                "236-byte push constants".into(),
            ));
        }
        // DMA-BUF import is an optional part of the renderer.  A Vulkan WSI
        // session can still render SHM and internally allocated images on a
        // driver that lacks one of the Linux interop extensions, so do not
        // reject an otherwise valid Vulkan 1.3 device here.  The importer
        // checks `supports_dmabuf_import` before ever constructing extension
        // image pNext chains and the TTY backend falls back to GLES when KMS
        // scanout cannot negotiate a compatible modifier.
        let dmabuf_extensions = [
            EXT_EXTERNAL_MEMORY_FD,
            EXT_EXTERNAL_MEMORY_DMA_BUF,
            EXT_IMAGE_DRM_FORMAT_MODIFIER,
            EXT_QUEUE_FAMILY_FOREIGN,
        ];
        let available_device_exts =
            unsafe { instance.enumerate_device_extension_properties(physical_device)? };
        let has_device_extension = |wanted: &CStr| {
            available_device_exts
                .iter()
                .any(|avail| unsafe { CStr::from_ptr(avail.extension_name.as_ptr()) == wanted })
        };
        let mut enabled_device_extensions = Vec::new();
        for ext in dmabuf_extensions {
            if has_device_extension(ext) {
                enabled_device_extensions.push(ext.as_ptr());
            }
        }
        let supports_drm_modifiers = has_device_extension(EXT_IMAGE_DRM_FORMAT_MODIFIER);
        let supports_dmabuf_import = dmabuf_extensions.iter().copied().all(has_device_extension);
        if surface_handle.is_some() {
            // Swapchain is the only mandatory device extension for a nested
            // WSI surface.  Swapchain maintenance is optional; the WSI path
            // uses present fences when available and a queue-idle fallback
            // otherwise, just like the surface capability helpers above.
            if !has_device_extension(ash::khr::swapchain::NAME) {
                return Err(VulkanInitError::MissingExtension(
                    ash::khr::swapchain::NAME.to_string_lossy().into_owned(),
                ));
            }
            enabled_device_extensions.push(ash::khr::swapchain::NAME.as_ptr());
            let has_swapchain_maintenance =
                instance_surface_maintenance1 && has_device_extension(EXT_SWAPCHAIN_MAINTENANCE1);
            if has_swapchain_maintenance {
                enabled_device_extensions.push(EXT_SWAPCHAIN_MAINTENANCE1.as_ptr());
            }
            // Keep this value until feature probing below.  A driver may
            // advertise the extension but not the feature.
            if !has_swapchain_maintenance {
                debug!("Vulkan swapchain maintenance extension unavailable; using core WSI path");
            }
        }
        // Nourish's sync.import: wait on producer sync_file payloads on the GPU.
        let mut supports_sync_file_import = false;
        let mut supports_sync_file_export = false;
        if available_device_exts.iter().any(|ext| unsafe {
            CStr::from_ptr(ext.extension_name.as_ptr()) == ash::khr::external_semaphore_fd::NAME
        }) {
            let mut properties = vk::ExternalSemaphoreProperties::default();
            let query = vk::PhysicalDeviceExternalSemaphoreInfo::default()
                .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
            unsafe {
                instance.get_physical_device_external_semaphore_properties(
                    physical_device,
                    &query,
                    &mut properties,
                );
            }
            supports_sync_file_import = properties
                .external_semaphore_features
                .contains(vk::ExternalSemaphoreFeatureFlags::IMPORTABLE);
            supports_sync_file_export = properties
                .external_semaphore_features
                .contains(vk::ExternalSemaphoreFeatureFlags::EXPORTABLE);
            if supports_sync_file_import || supports_sync_file_export {
                enabled_device_extensions.push(ash::khr::external_semaphore_fd::NAME.as_ptr());
            }
        }
        let mut dynamic = vk::PhysicalDeviceDynamicRenderingFeatures::default();
        let mut timeline = vk::PhysicalDeviceTimelineSemaphoreFeatures::default();
        let mut ycbcr = vk::PhysicalDeviceSamplerYcbcrConversionFeatures::default();
        let mut swapchain_maintenance =
            vk::PhysicalDeviceSwapchainMaintenance1FeaturesEXT::default();
        let mut supported = vk::PhysicalDeviceFeatures2::default()
            .push_next(&mut dynamic)
            .push_next(&mut timeline)
            .push_next(&mut ycbcr);
        let swapchain_maintenance_extension = if surface_handle.is_some() {
            instance_surface_maintenance1 && has_device_extension(EXT_SWAPCHAIN_MAINTENANCE1)
        } else {
            false
        };
        if swapchain_maintenance_extension {
            supported = supported.push_next(&mut swapchain_maintenance);
        }
        unsafe {
            instance.get_physical_device_features2(physical_device, &mut supported);
        }
        if dynamic.dynamic_rendering == vk::FALSE {
            return Err(VulkanInitError::MissingExtension(
                "dynamicRendering feature".into(),
            ));
        }
        let supports_dynamic_rendering = true;
        let supports_timeline_semaphores = timeline.timeline_semaphore != vk::FALSE;
        let supports_ycbcr_conversion = ycbcr.sampler_ycbcr_conversion != vk::FALSE;
        let supports_swapchain_maintenance = swapchain_maintenance_extension
            && swapchain_maintenance.swapchain_maintenance1 != vk::FALSE;

        let queue_priority = [1.0f32];
        let queue_create_info = vk::DeviceQueueCreateInfo::default()
            .queue_family_index(graphics_queue_family)
            .queue_priorities(&queue_priority);

        let queue_create_infos = [queue_create_info];

        // Enable Vulkan 1.3 features if available (dynamic rendering, timeline semaphores)
        let mut dynamic_rendering_feature = vk::PhysicalDeviceDynamicRenderingFeatures::default()
            .dynamic_rendering(supports_dynamic_rendering);
        let mut timeline_semaphore_feature = vk::PhysicalDeviceTimelineSemaphoreFeatures::default()
            .timeline_semaphore(supports_timeline_semaphores);
        let mut ycbcr_feature = vk::PhysicalDeviceSamplerYcbcrConversionFeatures::default()
            .sampler_ycbcr_conversion(supports_ycbcr_conversion);
        let mut swapchain_maintenance_feature =
            vk::PhysicalDeviceSwapchainMaintenance1FeaturesEXT::default()
                .swapchain_maintenance1(supports_swapchain_maintenance);

        let mut features2 = vk::PhysicalDeviceFeatures2::default()
            .push_next(&mut dynamic_rendering_feature)
            .push_next(&mut timeline_semaphore_feature)
            .push_next(&mut ycbcr_feature);
        if supports_swapchain_maintenance {
            features2 = features2.push_next(&mut swapchain_maintenance_feature);
        }

        let device_create_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_create_infos)
            .enabled_extension_names(&enabled_device_extensions)
            .push_next(&mut features2);

        let device = unsafe { instance.create_device(physical_device, &device_create_info, None) }?;
        let graphics_queue = unsafe { device.get_device_queue(graphics_queue_family, 0) };

        // Create reusable command pool for frame command buffer recording
        let pool_info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(graphics_queue_family)
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        let command_pool = match unsafe { device.create_command_pool(&pool_info, None) } {
            Ok(pool) => pool,
            Err(err) => {
                unsafe {
                    device.destroy_device(None);
                }
                return Err(err.into());
            }
        };

        let info = VulkanBackendInfo {
            api_version: (
                vk::api_version_major(props.api_version),
                vk::api_version_minor(props.api_version),
                vk::api_version_patch(props.api_version),
            ),
            device_name: device_name.to_owned(),
            supports_dynamic_rendering,
            supports_drm_modifiers,
            supports_dmabuf_import,
            supports_timeline_semaphores,
        };

        Ok(Arc::new(Self {
            entry,
            instance: instance_guard
                .0
                .take()
                .expect("instance present on success path"),
            physical_device,
            device,
            graphics_queue,
            graphics_queue_family,
            command_pool,
            supports_sync_file_import,
            supports_sync_file_export,
            supports_dmabuf_import,
            supports_drm_modifiers,
            supports_swapchain_maintenance,
            supports_ycbcr_conversion,
            command_lock: std::sync::Mutex::new(()),
            idle_commands: std::sync::Mutex::new(Vec::new()),
            staging_buffers: std::sync::Mutex::new(Vec::new()),
            vertex_buffers: std::sync::Mutex::new(Vec::new()),
            info,
            physical_device_properties: props,
        }))
    }

    fn select_present_device(
        instance: &Instance,
        devices: &[vk::PhysicalDevice],
        surface_api: &ash::khr::surface::Instance,
        surface: vk::SurfaceKHR,
    ) -> Result<(vk::PhysicalDevice, u32), VulkanInitError> {
        let mut ranked = devices.to_vec();
        ranked.sort_by_key(|&device| {
            match unsafe { instance.get_physical_device_properties(device).device_type } {
                vk::PhysicalDeviceType::DISCRETE_GPU => 0,
                vk::PhysicalDeviceType::INTEGRATED_GPU => 1,
                _ => 2,
            }
        });
        for device in ranked {
            let properties = unsafe { instance.get_physical_device_properties(device) };
            if properties.api_version < vk::API_VERSION_1_3
                || properties.limits.max_push_constants_size
                    < std::mem::size_of::<super::pipeline::QuadPushConstants>() as u32
            {
                continue;
            }
            let extensions = unsafe { instance.enumerate_device_extension_properties(device)? };
            let has_extension = |wanted: &CStr| {
                extensions
                    .iter()
                    .any(|ext| unsafe { CStr::from_ptr(ext.extension_name.as_ptr()) == wanted })
            };
            // The nested WSI path only needs core surface support and the
            // swapchain extension.  Linux DMA-BUF interop is optional here:
            // SHM uploads and optimal-tiling render targets work without it,
            // while the renderer advertises an empty DMA-BUF format set when
            // the optional extensions are unavailable.
            if !has_extension(ash::khr::swapchain::NAME) {
                continue;
            }
            let mut dynamic = vk::PhysicalDeviceDynamicRenderingFeatures::default();
            let mut features = vk::PhysicalDeviceFeatures2::default().push_next(&mut dynamic);
            unsafe { instance.get_physical_device_features2(device, &mut features) };
            if dynamic.dynamic_rendering == vk::FALSE {
                continue;
            }
            let queues = unsafe { instance.get_physical_device_queue_family_properties(device) };
            for (index, queue) in queues.iter().enumerate() {
                if queue.queue_flags.contains(vk::QueueFlags::GRAPHICS)
                    && unsafe {
                        surface_api.get_physical_device_surface_support(
                            device,
                            index as u32,
                            surface,
                        )?
                    }
                {
                    return Ok((device, index as u32));
                }
            }
        }
        Err(VulkanInitError::NoMatchingPhysicalDevice)
    }

    /// Select physical device matching DRM node via dev_t / major / minor comparison
    fn select_physical_device(
        instance: &Instance,
        devices: &[vk::PhysicalDevice],
        drm_node: Option<&DrmNode>,
    ) -> Result<vk::PhysicalDevice, VulkanInitError> {
        if devices.is_empty() {
            return Err(VulkanInitError::NoMatchingPhysicalDevice);
        }

        if let Some(node) = drm_node {
            if let Some(dev_path) = node.dev_path() {
                if let Ok(stat) = std::fs::metadata(&dev_path) {
                    use std::os::unix::fs::MetadataExt;
                    let rdev = stat.rdev();
                    let dev_major = major(rdev);
                    let dev_minor = minor(rdev);

                    // Attempt to match using VK_EXT_physical_device_drm if available
                    for &pdev in devices {
                        let mut drm_props = vk::PhysicalDeviceDrmPropertiesEXT::default();
                        let mut props2 =
                            vk::PhysicalDeviceProperties2::default().push_next(&mut drm_props);
                        unsafe { instance.get_physical_device_properties2(pdev, &mut props2) };

                        let matches_primary = drm_props.has_primary != vk::FALSE
                            && drm_props.primary_major == dev_major as i64
                            && drm_props.primary_minor == dev_minor as i64;
                        let matches_render = drm_props.has_render != vk::FALSE
                            && drm_props.render_major == dev_major as i64
                            && drm_props.render_minor == dev_minor as i64;

                        if matches_primary || matches_render {
                            debug!(
                                "matched DRM node {:?} to Vulkan device via DRM properties",
                                dev_path
                            );
                            return Ok(pdev);
                        }
                    }
                }
            }
        }

        if drm_node.is_some() {
            return Err(VulkanInitError::NoMatchingPhysicalDevice);
        }
        // Without a requested DRM node, prefer DISCRETE_GPU over INTEGRATED_GPU over others
        let mut sorted_devices = devices.to_vec();
        sorted_devices.sort_by_key(|&d| {
            let props = unsafe { instance.get_physical_device_properties(d) };
            match props.device_type {
                vk::PhysicalDeviceType::DISCRETE_GPU => 0,
                vk::PhysicalDeviceType::INTEGRATED_GPU => 1,
                _ => 2,
            }
        });

        Ok(sorted_devices[0])
    }

    /// Acquire a host-visible, host-coherent staging buffer of at least `size` bytes.
    /// Reuses a pooled buffer if available, avoiding vkAllocateMemory stalls.
    pub fn acquire_staging_buffer(&self, size: usize) -> Result<StagingBuffer, vk::Result> {
        {
            let mut pool = self.staging_buffers.lock().unwrap();
            if let Some(pos) = best_fit_buffer(&pool, size) {
                return Ok(pool.swap_remove(pos));
            }
        }
        let alloc_size = size.next_power_of_two().max(1024 * 1024);
        let device = &self.device;
        unsafe {
            let buffer = device.create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(alloc_size as u64)
                    .usage(vk::BufferUsageFlags::TRANSFER_SRC | vk::BufferUsageFlags::TRANSFER_DST)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )?;
            let req = device.get_buffer_memory_requirements(buffer);
            let props = self
                .instance
                .get_physical_device_memory_properties(self.physical_device);
            let index = match (0..props.memory_type_count).find(|i| {
                req.memory_type_bits & (1 << i) != 0
                    && props.memory_types[*i as usize].property_flags.contains(
                        vk::MemoryPropertyFlags::HOST_VISIBLE
                            | vk::MemoryPropertyFlags::HOST_COHERENT,
                    )
            }) {
                Some(index) => index,
                None => {
                    device.destroy_buffer(buffer, None);
                    return Err(vk::Result::ERROR_OUT_OF_HOST_MEMORY);
                }
            };
            let memory = match device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(req.size)
                    .memory_type_index(index),
                None,
            ) {
                Ok(memory) => memory,
                Err(err) => {
                    device.destroy_buffer(buffer, None);
                    return Err(err);
                }
            };
            if let Err(err) = device.bind_buffer_memory(buffer, memory, 0) {
                device.destroy_buffer(buffer, None);
                device.free_memory(memory, None);
                return Err(err);
            }
            Ok(StagingBuffer {
                buffer,
                memory,
                size: alloc_size,
            })
        }
    }

    /// Return a staging buffer to the pool for reuse.
    pub fn release_staging_buffer(&self, staging: StagingBuffer) {
        self.release_pooled_buffer(&self.staging_buffers, staging, MAX_STAGING_POOL_BYTES);
    }

    /// Host-visible vertex buffer for textured triangle meshes.
    pub fn acquire_vertex_buffer(&self, size: usize) -> Result<StagingBuffer, vk::Result> {
        {
            let mut pool = self.vertex_buffers.lock().unwrap();
            if let Some(pos) = best_fit_buffer(&pool, size) {
                return Ok(pool.swap_remove(pos));
            }
        }
        let alloc_size = size.next_power_of_two().max(64 * 1024);
        let device = &self.device;
        unsafe {
            let buffer = device.create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(alloc_size as u64)
                    .usage(
                        vk::BufferUsageFlags::VERTEX_BUFFER | vk::BufferUsageFlags::UNIFORM_BUFFER,
                    )
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )?;
            let req = device.get_buffer_memory_requirements(buffer);
            let props = self
                .instance
                .get_physical_device_memory_properties(self.physical_device);
            let index = match (0..props.memory_type_count).find(|i| {
                req.memory_type_bits & (1 << i) != 0
                    && props.memory_types[*i as usize].property_flags.contains(
                        vk::MemoryPropertyFlags::HOST_VISIBLE
                            | vk::MemoryPropertyFlags::HOST_COHERENT,
                    )
            }) {
                Some(index) => index,
                None => {
                    device.destroy_buffer(buffer, None);
                    return Err(vk::Result::ERROR_OUT_OF_HOST_MEMORY);
                }
            };
            let memory = match device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(req.size)
                    .memory_type_index(index),
                None,
            ) {
                Ok(memory) => memory,
                Err(err) => {
                    device.destroy_buffer(buffer, None);
                    return Err(err);
                }
            };
            if let Err(err) = device.bind_buffer_memory(buffer, memory, 0) {
                device.destroy_buffer(buffer, None);
                device.free_memory(memory, None);
                return Err(err);
            }
            Ok(StagingBuffer {
                buffer,
                memory,
                size: alloc_size,
            })
        }
    }

    pub fn write_host_buffer(&self, buf: &StagingBuffer, data: &[u8]) -> Result<(), vk::Result> {
        if data.len() > buf.size {
            return Err(vk::Result::ERROR_OUT_OF_DEVICE_MEMORY);
        }
        unsafe {
            let ptr = self.device.map_memory(
                buf.memory,
                0,
                data.len() as u64,
                vk::MemoryMapFlags::empty(),
            )?;
            std::ptr::copy_nonoverlapping(data.as_ptr(), ptr.cast::<u8>(), data.len());
            self.device.unmap_memory(buf.memory);
        }
        Ok(())
    }

    pub fn release_vertex_buffer(&self, staging: StagingBuffer) {
        self.release_pooled_buffer(&self.vertex_buffers, staging, MAX_VERTEX_POOL_BYTES);
    }

    fn release_pooled_buffer(
        &self,
        pool: &std::sync::Mutex<Vec<StagingBuffer>>,
        staging: StagingBuffer,
        max_bytes: usize,
    ) {
        let mut evicted = Vec::new();
        if staging.size <= max_bytes {
            let mut pool = pool.lock().unwrap();
            evicted = make_room_for_buffer(&mut pool, staging.size, max_bytes);
            pool.push(staging);
        } else {
            evicted.push(staging);
        }
        // Vulkan destruction is outside the pool lock so concurrent render
        // threads can still acquire already-idle buffers.
        for buffer in evicted {
            unsafe {
                self.device.destroy_buffer(buffer.buffer, None);
                self.device.free_memory(buffer.memory, None);
            }
        }
    }

    /// Acquire an idle primary command buffer and fence, or allocate a new pair.
    pub fn acquire_command_fence(&self) -> Result<(vk::CommandBuffer, vk::Fence), vk::Result> {
        let pooled = self.idle_commands.lock().unwrap().pop();
        if let Some((cmd, fence)) = pooled {
            unsafe {
                let reset = self
                    .device
                    .reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty())
                    .and_then(|_| self.device.reset_fences(&[fence]));
                if let Err(err) = reset {
                    self.device.destroy_fence(fence, None);
                    self.device.free_command_buffers(self.command_pool, &[cmd]);
                    return Err(err);
                }
            }
            Ok((cmd, fence))
        } else {
            unsafe {
                let cmd = self.device.allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default()
                        .command_pool(self.command_pool)
                        .level(vk::CommandBufferLevel::PRIMARY)
                        .command_buffer_count(1),
                )?[0];
                let fence = match self
                    .device
                    .create_fence(&vk::FenceCreateInfo::default(), None)
                {
                    Ok(fence) => fence,
                    Err(err) => {
                        self.device.free_command_buffers(self.command_pool, &[cmd]);
                        return Err(err);
                    }
                };
                Ok((cmd, fence))
            }
        }
    }

    /// Return an idle command buffer and fence to the pool for reuse.
    pub fn release_command_fence(&self, cmd: vk::CommandBuffer, fence: vk::Fence) {
        let mut pool = self.idle_commands.lock().unwrap();
        if pool.len() < 16 {
            pool.push((cmd, fence));
        } else {
            unsafe {
                self.device.destroy_fence(fence, None);
                self.device.free_command_buffers(self.command_pool, &[cmd]);
            }
        }
    }
}

impl Drop for VulkanContext {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();
            for staging in self.staging_buffers.get_mut().unwrap().drain(..) {
                self.device.destroy_buffer(staging.buffer, None);
                self.device.free_memory(staging.memory, None);
            }
            for staging in self.vertex_buffers.get_mut().unwrap().drain(..) {
                self.device.destroy_buffer(staging.buffer, None);
                self.device.free_memory(staging.memory, None);
            }
            for (_, fence) in self.idle_commands.get_mut().unwrap().drain(..) {
                self.device.destroy_fence(fence, None);
            }
            self.device.destroy_command_pool(self.command_pool, None);
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}

/// Release the instance on initialization failure.
struct InstanceDropGuard(Option<Instance>);

impl Drop for InstanceDropGuard {
    fn drop(&mut self) {
        if let Some(instance) = self.0.take() {
            unsafe { instance.destroy_instance(None) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(size: usize) -> StagingBuffer {
        StagingBuffer {
            buffer: vk::Buffer::null(),
            memory: vk::DeviceMemory::null(),
            size,
        }
    }

    #[test]
    fn pooled_buffers_choose_best_fit_and_bound_retained_memory() {
        let mut pool = vec![buffer(128), buffer(16), buffer(64)];
        assert_eq!(best_fit_buffer(&pool, 12), Some(1));
        assert_eq!(best_fit_buffer(&pool, 32), Some(2));
        assert_eq!(best_fit_buffer(&pool, 256), None);

        let evicted = make_room_for_buffer(&mut pool, 32, 128);
        assert_eq!(
            evicted.iter().map(|b| b.size).collect::<Vec<_>>(),
            vec![128]
        );
        pool.push(buffer(32));
        assert_eq!(pool.iter().map(|b| b.size).sum::<usize>(), 112);

        let mut pool = (0..MAX_BUFFER_POOL_ENTRIES).map(|_| buffer(1)).collect();
        assert_eq!(make_room_for_buffer(&mut pool, 1, 128).len(), 1);
        assert_eq!(pool.len(), MAX_BUFFER_POOL_ENTRIES - 1);
    }
}
