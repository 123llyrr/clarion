use ash::vk;
use std::mem;

impl super::Surface {
    pub fn info(&self) -> crate::SurfaceInfo {
        crate::SurfaceInfo {
            format: self.swapchain.format,
            alpha: self.swapchain.alpha,
        }
    }

    /// Whether a swapchain is installed. When it is not (the driver refused to
    /// create one, or the surface was lost), the renderer skips the frame
    /// instead of aborting.
    pub fn has_swapchain(&self) -> bool {
        !self.lost && self.swapchain.raw != vk::SwapchainKHR::null() && !self.frames.is_empty()
    }

    unsafe fn deinit_swapchain(&mut self, raw_device: &ash::Device) {
        let _ = raw_device.device_wait_idle();
        self.device
            .destroy_swapchain(mem::take(&mut self.swapchain.raw), None);
        for frame in self.frames.drain(..) {
            raw_device.destroy_image_view(frame.view, None);
            raw_device.destroy_semaphore(frame.acquire_semaphore, None);
            raw_device.destroy_semaphore(frame.present_semaphore, None);
        }
    }

    pub fn acquire_frame(&mut self) -> super::Frame {
        let acquire_semaphore = self.next_semaphore;
        match unsafe {
            self.device.acquire_next_image(
                self.swapchain.raw,
                !0,
                acquire_semaphore,
                vk::Fence::null(),
            )
        } {
            Ok((index, _suboptimal)) => {
                self.next_semaphore = mem::replace(
                    &mut self.frames[index as usize].acquire_semaphore,
                    acquire_semaphore,
                );
                super::Frame {
                    internal: self.frames[index as usize],
                    swapchain: self.swapchain,
                    image_index: Some(index),
                }
            }
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                log::warn!("Acquire failed because the surface is out of date");
                super::Frame {
                    internal: self.frames[0],
                    swapchain: self.swapchain,
                    image_index: None,
                }
            }
            Err(other) => panic!("Aquire image error {}", other),
        }
    }
}

impl super::Context {
    pub fn create_surface<
        I: raw_window_handle::HasWindowHandle + raw_window_handle::HasDisplayHandle,
    >(
        &self,
        window: &I,
    ) -> Result<super::Surface, crate::NotSupportedError> {
        let khr_swapchain = self
            .device
            .swapchain
            .clone()
            .ok_or(crate::NotSupportedError::NoSupportedDeviceFound)?;

        let raw = unsafe {
            ash_window::create_surface(
                &self.entry,
                &self.instance.core,
                window.display_handle().unwrap().as_raw(),
                window.window_handle().unwrap().as_raw(),
                None,
            )
            .map_err(super::PlatformError::Init)?
        };

        let khr_surface = self
            .instance
            .surface
            .as_ref()
            .ok_or(crate::NotSupportedError::PlatformNotSupported)?;
        if unsafe {
            khr_surface.get_physical_device_surface_support(
                self.physical_device,
                self.queue_family_index,
                raw,
            ) != Ok(true)
        } {
            log::warn!("Rejected for not presenting to the window surface");
            return Err(crate::NotSupportedError::PlatformNotSupported);
        }

        let mut surface_info = vk::PhysicalDeviceSurfaceInfo2KHR {
            surface: raw,
            ..Default::default()
        };
        let mut fullscreen_exclusive_win32 = vk::SurfaceFullScreenExclusiveWin32InfoEXT::default();
        surface_info = surface_info.push_next(&mut fullscreen_exclusive_win32);
        let mut fullscreen_exclusive_ext = vk::SurfaceCapabilitiesFullScreenExclusiveEXT::default();
        let mut capabilities2_khr =
            vk::SurfaceCapabilities2KHR::default().push_next(&mut fullscreen_exclusive_ext);
        let _ = unsafe {
            self.instance
                .get_surface_capabilities2
                .as_ref()
                .unwrap()
                .get_physical_device_surface_capabilities2(
                    self.physical_device,
                    &surface_info,
                    &mut capabilities2_khr,
                )
        };
        log::debug!("{:?}", capabilities2_khr.surface_capabilities);

        let semaphore_create_info = vk::SemaphoreCreateInfo::default();
        let next_semaphore = unsafe {
            self.device
                .core
                .create_semaphore(&semaphore_create_info, None)
                .unwrap()
        };

        Ok(super::Surface {
            device: khr_swapchain,
            raw,
            frames: Vec::new(),
            next_semaphore,
            swapchain: super::Swapchain {
                raw: vk::SwapchainKHR::null(),
                format: crate::TextureFormat::Rgba8Unorm,
                alpha: crate::AlphaMode::Ignored,
                target_size: [0; 2],
            },
            full_screen_exclusive: fullscreen_exclusive_ext.full_screen_exclusive_supported != 0,
            pending_config: None,
            pending_retry_at: None,
            pending_backoff: None,
            lost: false,
        })
    }

    pub fn destroy_surface(&self, surface: &mut super::Surface) {
        // A lost surface may fault inside the driver while its swapchain views
        // are destroyed; leave that state to process exit.
        if !surface.lost {
            unsafe {
                surface.deinit_swapchain(&self.device.core);
            }
        }
        unsafe {
            self.device
                .core
                .destroy_semaphore(surface.next_semaphore, None)
        };
        if let Some(ref surface_instance) = self.instance.surface {
            unsafe { surface_instance.destroy_surface(surface.raw, None) };
        }
    }

    /// Install a new configuration on the surface. A failure (including the
    /// driver returning a success code without a swapchain) is kept as a
    /// pending configuration and retried from the render loop rather than
    /// aborting the process. A surface the driver reports as lost is never
    /// touched again: its swapchain cannot be recreated, and NVIDIA's driver
    /// faults while destroying views that belong to it.
    pub fn reconfigure_surface(&self, surface: &mut super::Surface, config: crate::SurfaceConfig) {
        if surface.lost {
            return;
        }
        match self.install_swapchain(surface, config) {
            Ok(()) => {
                surface.pending_config = None;
                surface.pending_retry_at = None;
                surface.pending_backoff = None;
            }
            Err(vk::Result::ERROR_SURFACE_LOST_KHR) => {
                surface.lost = true;
                surface.pending_config = None;
                surface.pending_retry_at = None;
                surface.pending_backoff = None;
                eprintln!(
                    "blade: surface lost at {}x{}; stopping rendering for this window",
                    config.size.width, config.size.height
                );
            }
            Err(result) => {
                eprintln!(
                    "blade: swapchain creation failed at {}x{} ({result:?}); keeping the window alive and retrying",
                    config.size.width, config.size.height
                );
                surface.pending_config = Some(config);
                surface.pending_retry_at = Some(std::time::Instant::now());
                surface.pending_backoff = None;
            }
        }
    }

    /// Retry a configuration that could not be installed. The renderer calls
    /// this once per frame; repeated failures back off up to four seconds.
    pub fn recover_surface(&self, surface: &mut super::Surface) {
        if surface.lost {
            return;
        }
        let Some(config) = surface.pending_config else {
            return;
        };
        let now = std::time::Instant::now();
        if surface.pending_retry_at.is_some_and(|at| now < at) {
            return;
        }
        match self.install_swapchain(surface, config) {
            Ok(()) => {
                surface.pending_config = None;
                surface.pending_retry_at = None;
                surface.pending_backoff = None;
                eprintln!(
                    "blade: swapchain at {}x{} created after retry",
                    config.size.width, config.size.height
                );
            }
            Err(vk::Result::ERROR_SURFACE_LOST_KHR) => {
                surface.lost = true;
                surface.pending_config = None;
                surface.pending_retry_at = None;
                surface.pending_backoff = None;
                eprintln!(
                    "blade: surface lost at {}x{} while retrying; stopping rendering for this window",
                    config.size.width, config.size.height
                );
            }
            Err(result) => {
                let backoff = surface
                    .pending_backoff
                    .unwrap_or(std::time::Duration::from_millis(100));
                let backoff = (backoff * 2).min(std::time::Duration::from_secs(4));
                surface.pending_backoff = Some(backoff);
                surface.pending_retry_at = Some(now + backoff);
                log::warn!(
                    "Swapchain retry at {}x{} still failed: {result:?}",
                    config.size.width,
                    config.size.height
                );
            }
        }
    }

    fn install_swapchain(
        &self,
        surface: &mut super::Surface,
        config: crate::SurfaceConfig,
    ) -> Result<(), vk::Result> {
        let khr_surface = self.instance.surface.as_ref().unwrap();

        let capabilities = unsafe {
            khr_surface
                .get_physical_device_surface_capabilities(self.physical_device, surface.raw)
                .unwrap()
        };
        if config.size.width < capabilities.min_image_extent.width
            || config.size.width > capabilities.max_image_extent.width
            || config.size.height < capabilities.min_image_extent.height
            || config.size.height > capabilities.max_image_extent.height
        {
            log::warn!(
                "Requested size {}x{} is outside of surface capabilities",
                config.size.width,
                config.size.height
            );
        }

        let (alpha, composite_alpha) = if config.transparent {
            if capabilities
                .supported_composite_alpha
                .contains(vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED)
            {
                (
                    crate::AlphaMode::PostMultiplied,
                    vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED,
                )
            } else if capabilities
                .supported_composite_alpha
                .contains(vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED)
            {
                (
                    crate::AlphaMode::PreMultiplied,
                    vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED,
                )
            } else {
                log::error!(
                    "No composite alpha flag for transparency: {:?}",
                    capabilities.supported_composite_alpha
                );
                (
                    crate::AlphaMode::Ignored,
                    vk::CompositeAlphaFlagsKHR::OPAQUE,
                )
            }
        } else {
            (
                crate::AlphaMode::Ignored,
                vk::CompositeAlphaFlagsKHR::OPAQUE,
            )
        };

        let (requested_frame_count, mode_preferences) = match config.display_sync {
            crate::DisplaySync::Block => (3, [vk::PresentModeKHR::FIFO].as_slice()),
            crate::DisplaySync::Recent => (
                3,
                [
                    vk::PresentModeKHR::MAILBOX,
                    vk::PresentModeKHR::FIFO_RELAXED,
                    vk::PresentModeKHR::IMMEDIATE,
                ]
                .as_slice(),
            ),
            crate::DisplaySync::Tear => (2, [vk::PresentModeKHR::IMMEDIATE].as_slice()),
        };
        let effective_frame_count = requested_frame_count.max(capabilities.min_image_count);

        let present_modes = unsafe {
            khr_surface
                .get_physical_device_surface_present_modes(self.physical_device, surface.raw)
                .unwrap()
        };
        let present_mode = *mode_preferences
            .iter()
            .find(|mode| present_modes.contains(mode))
            .unwrap();
        log::info!("Using surface present mode {:?}", present_mode);

        let queue_families = [self.queue_family_index];

        let mut supported_formats = Vec::new();
        let (format, surface_format) = if surface.swapchain.target_size[0] > 0 {
            let format = surface.swapchain.format;
            log::info!("Retaining current format: {:?}", format);
            let vk_color_space = match (format, config.color_space) {
                (crate::TextureFormat::Bgra8Unorm, crate::ColorSpace::Srgb) => {
                    vk::ColorSpaceKHR::SRGB_NONLINEAR
                }
                (crate::TextureFormat::Bgra8Unorm, crate::ColorSpace::Linear) => {
                    vk::ColorSpaceKHR::EXTENDED_SRGB_LINEAR_EXT
                }
                (crate::TextureFormat::Bgra8UnormSrgb, crate::ColorSpace::Linear) => {
                    vk::ColorSpaceKHR::default()
                }
                _ => panic!(
                    "Unexpected format {:?} under color space {:?}",
                    format, config.color_space
                ),
            };
            (
                format,
                vk::SurfaceFormatKHR {
                    format: super::map_texture_format(format),
                    color_space: vk_color_space,
                },
            )
        } else {
            supported_formats = unsafe {
                khr_surface
                    .get_physical_device_surface_formats(self.physical_device, surface.raw)
                    .unwrap()
            };
            match config.color_space {
                crate::ColorSpace::Linear => {
                    let surface_format = vk::SurfaceFormatKHR {
                        format: vk::Format::B8G8R8A8_UNORM,
                        color_space: vk::ColorSpaceKHR::EXTENDED_SRGB_LINEAR_EXT,
                    };
                    if supported_formats.contains(&surface_format) {
                        log::info!("Using linear SRGB color space");
                        (crate::TextureFormat::Bgra8Unorm, surface_format)
                    } else {
                        (
                            crate::TextureFormat::Bgra8UnormSrgb,
                            vk::SurfaceFormatKHR {
                                format: vk::Format::B8G8R8A8_SRGB,
                                color_space: vk::ColorSpaceKHR::default(),
                            },
                        )
                    }
                }
                crate::ColorSpace::Srgb => (
                    crate::TextureFormat::Bgra8Unorm,
                    vk::SurfaceFormatKHR {
                        format: vk::Format::B8G8R8A8_UNORM,
                        color_space: vk::ColorSpaceKHR::SRGB_NONLINEAR,
                    },
                ),
            }
        };
        if !supported_formats.is_empty() && !supported_formats.contains(&surface_format) {
            log::error!("Surface formats are incompatible: {:?}", supported_formats);
        }

        let vk_usage = super::resource::map_texture_usage(config.usage, crate::TexelAspects::COLOR);
        if !capabilities.supported_usage_flags.contains(vk_usage) {
            log::error!(
                "Surface usages are incompatible: {:?}",
                capabilities.supported_usage_flags
            );
        }

        let mut full_screen_exclusive_info = vk::SurfaceFullScreenExclusiveInfoEXT {
            full_screen_exclusive: if config.allow_exclusive_full_screen {
                vk::FullScreenExclusiveEXT::ALLOWED
            } else {
                vk::FullScreenExclusiveEXT::DISALLOWED
            },
            ..Default::default()
        };

        let target_size = [config.size.width as u16, config.size.height as u16];

        let mut create_info = vk::SwapchainCreateInfoKHR {
            surface: surface.raw,
            min_image_count: effective_frame_count,
            image_format: surface_format.format,
            image_color_space: surface_format.color_space,
            image_extent: vk::Extent2D {
                width: config.size.width,
                height: config.size.height,
            },
            image_array_layers: 1,
            image_usage: vk_usage,
            pre_transform: vk::SurfaceTransformFlagsKHR::IDENTITY,
            composite_alpha,
            present_mode,
            old_swapchain: surface.swapchain.raw,
            ..Default::default()
        }
        .queue_family_indices(&queue_families);

        if surface.full_screen_exclusive {
            assert!(self.device.full_screen_exclusive.is_some());
            create_info = create_info.push_next(&mut full_screen_exclusive_info);
            log::info!(
                "Configuring exclusive full screen: {}",
                config.allow_exclusive_full_screen
            );
        }
        // `vkCreateSwapchainKHR` may succeed with a suboptimal swapchain, and
        // some drivers (notably NVIDIA's Wayland WSI) do exactly that when the
        // requested extent does not match the surface's current buffer. The
        // swapchain is valid and presentation still succeeds, possibly scaled;
        // GPUI reconfigures it on the next drawable-size change. Unwrapping
        // here aborted the whole shell on that transient condition, so call
        // the entry point directly to keep the handle that `ash` discards when
        // it treats this success code as an error.
        //
        // The same driver also intermittently returns a success code *without*
        // writing a handle, most visibly while a fullscreen surface is being
        // created. There is nothing to install in that case: report it and let
        // the caller retry from the render loop.
        let mut raw_swapchain = vk::SwapchainKHR::null();
        let create_result = unsafe {
            (surface.device.fp().create_swapchain_khr)(
                surface.device.device(),
                &create_info,
                core::ptr::null(),
                &mut raw_swapchain,
            )
        };
        match create_result {
            vk::Result::SUCCESS => {}
            vk::Result::SUBOPTIMAL_KHR if raw_swapchain != vk::SwapchainKHR::null() => {
                eprintln!(
                    "blade: swapchain created suboptimal at {}x{}; presenting with scaling until the next resize",
                    config.size.width, config.size.height
                );
            }
            result => {
                if surface.swapchain.raw == vk::SwapchainKHR::null() {
                    // Keep `info()` honest for the renderer built from this
                    // configuration: pipelines are created from the format and
                    // alpha the retry will use.
                    surface.swapchain = super::Swapchain {
                        raw: vk::SwapchainKHR::null(),
                        format,
                        alpha,
                        target_size,
                    };
                }
                return Err(result);
            }
        }
        if raw_swapchain == vk::SwapchainKHR::null() {
            return Err(create_result);
        }

        unsafe {
            surface.deinit_swapchain(&self.device.core);
        }

        let images = unsafe { surface.device.get_swapchain_images(raw_swapchain).unwrap() };
        let subresource_range = vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        };
        for image in images {
            let view_create_info = vk::ImageViewCreateInfo {
                image,
                view_type: vk::ImageViewType::TYPE_2D,
                format: surface_format.format,
                subresource_range,
                ..Default::default()
            };
            let view = unsafe {
                self.device
                    .core
                    .create_image_view(&view_create_info, None)
                    .unwrap()
            };
            let semaphore_create_info = vk::SemaphoreCreateInfo::default();
            let acquire_semaphore = unsafe {
                self.device
                    .core
                    .create_semaphore(&semaphore_create_info, None)
                    .unwrap()
            };
            let present_semaphore = unsafe {
                self.device
                    .core
                    .create_semaphore(&semaphore_create_info, None)
                    .unwrap()
            };
            surface.frames.push(super::InternalFrame {
                acquire_semaphore,
                present_semaphore,
                image,
                view,
            });
        }
        surface.swapchain = super::Swapchain {
            raw: raw_swapchain,
            format,
            alpha,
            target_size,
        };
        Ok(())
    }
}
