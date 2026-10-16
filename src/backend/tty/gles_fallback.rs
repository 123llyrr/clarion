//! GLES compatibility resources are activated only when a caller actually needs
//! a GLES renderer. Registering a native Vulkan GPU must not load its EGL driver.

use anyhow::{ensure, Context as _};
use smithay::backend::allocator::gbm::GbmDevice;
use smithay::backend::allocator::Fourcc;
use smithay::backend::drm::{DrmDeviceFd, DrmNode};
use smithay::backend::egl::context::ContextPriority;
use smithay::backend::egl::{EGLDevice, EGLDisplay};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::multigpu::gbm::GbmGlesBackend;
use smithay::backend::renderer::multigpu::GpuManager;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;

use super::renderer_init::DeferredNodes;
use super::TtyRenderer;

pub(crate) struct GlesFallback {
    manager: GpuManager<GbmGlesBackend<GlesRenderer, DrmDeviceFd>>,
    nodes: DeferredNodes<DrmNode, GbmDevice<DrmDeviceFd>>,
}

fn hardware_egl_device(gbm: GbmDevice<DrmDeviceFd>) -> anyhow::Result<(EGLDisplay, EGLDevice)> {
    let display = unsafe { EGLDisplay::new(gbm)? };
    let device = EGLDevice::device_for_display(&display)?;
    // Preserve the previous llvmpipe/softpipe rejection: their cross-device imports
    // can crash and they must not be advertised as hardware render/lease devices.
    ensure!(!device.is_software(), "software EGL renderers are skipped");
    Ok((display, device))
}

impl GlesFallback {
    pub(crate) fn new() -> anyhow::Result<Self> {
        // This empty manager creates no EGLDisplay or GL context. In Smithay,
        // add_node eagerly creates EGLDisplay; single_renderer enumerates contexts.
        let api = GbmGlesBackend::with_context_priority(ContextPriority::High);
        let manager = GpuManager::new(api).context("error creating the GPU manager")?;
        Ok(Self {
            manager,
            nodes: DeferredNodes::default(),
        })
    }

    pub(crate) fn register_deferred(&mut self, node: DrmNode, gbm: GbmDevice<DrmDeviceFd>) {
        self.nodes.register(node, gbm, false);
    }

    /// Explicit GLES and unsuccessful Vulkan startup retain EGL's original device
    /// discovery, including shared render nodes behind display-only DRM devices.
    pub(crate) fn probe_and_register(
        &mut self,
        node: DrmNode,
        gbm: GbmDevice<DrmDeviceFd>,
    ) -> anyhow::Result<DrmNode> {
        let (_display, device) = hardware_egl_device(gbm.clone())?;
        let render_node = device.try_get_render_node().ok().flatten().unwrap_or(node);
        self.manager
            .as_mut()
            .add_node(render_node, gbm.clone())
            .context("error adding render node to GPU manager")?;
        self.nodes.register(render_node, gbm, true);
        Ok(render_node)
    }

    pub(crate) fn ensure_registered(&mut self, node: &DrmNode) -> anyhow::Result<()> {
        let manager = &mut self.manager;
        let available = self.nodes.initialize(node, |gbm| {
            // Retain the validated display while add_node obtains its reference.
            let (_display, _device) = hardware_egl_device(gbm.clone())?;
            manager
                .as_mut()
                .add_node(*node, gbm.clone())
                .context("error activating GLES fallback node")?;
            Ok::<_, anyhow::Error>(())
        })?;
        ensure!(available, "no GBM device registered for GLES node {node}");
        Ok(())
    }

    pub(crate) fn single_renderer(&mut self, node: &DrmNode) -> anyhow::Result<TtyRenderer<'_>> {
        self.ensure_registered(node)?;
        self.manager.single_renderer(node).map_err(Into::into)
    }

    pub(crate) fn renderer(
        &mut self,
        render: &DrmNode,
        target: &DrmNode,
        format: Fourcc,
    ) -> anyhow::Result<TtyRenderer<'_>> {
        self.ensure_registered(render)?;
        self.ensure_registered(target)?;
        self.manager
            .renderer(render, target, format)
            .map_err(Into::into)
    }

    pub(crate) fn early_import(
        &mut self,
        node: DrmNode,
        surface: &WlSurface,
    ) -> anyhow::Result<()> {
        self.ensure_registered(&node)?;
        self.manager.early_import(node, surface).map_err(Into::into)
    }

    pub(crate) fn is_initialized(&self, node: &DrmNode) -> bool {
        self.nodes.is_initialized(node)
    }

    pub(crate) fn remove_node(&mut self, node: &DrmNode) {
        self.nodes.remove(node);
        self.manager.as_mut().remove_node(node);
        // Enumerate only nodes already registered with EGL; pending Vulkan-only
        // devices are absent from the underlying manager and remain uninitialized.
        let _ = self.manager.devices();
    }
}

#[cfg(test)]
mod tests {
    use std::os::fd::OwnedFd;
    use std::path::Path;

    use smithay::backend::drm::NodeType;
    use smithay::utils::DeviceFd;

    use super::*;

    fn render_device() -> (DrmNode, GbmDevice<DrmDeviceFd>) {
        let path = std::env::var("CLARION_VULKAN_TEST_NODE")
            .expect("set CLARION_VULKAN_TEST_NODE to a DRM render node");
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let node = DrmNode::from_file(&file).unwrap();
        // A render node cannot become DRM master or change the running session.
        assert_eq!(node.ty(), NodeType::Render);
        let owned: OwnedFd = file.into();
        let fd = DrmDeviceFd::new(DeviceFd::from(owned));
        let gbm = GbmDevice::new(fd).unwrap();
        eprintln!("GBM hardware test on {path}");
        (node, gbm)
    }

    #[test]
    #[ignore = "requires CLARION_VULKAN_TEST_NODE and an unavailable EGL vendor list in an isolated process"]
    fn gbm_registration_does_not_require_egl_and_failed_activation_remains_retryable() {
        let vendor_list = std::env::var("__EGL_VENDOR_LIBRARY_FILENAMES")
            .expect("run this test in isolation with an unavailable EGL vendor list");
        assert!(!vendor_list.is_empty());
        assert!(vendor_list.split(':').all(|path| !Path::new(path).exists()));

        let (node, gbm) = render_device();

        let mut fallback = GlesFallback::new().unwrap();
        fallback.register_deferred(node, gbm.clone());
        assert!(!fallback.is_initialized(&node));
        for _ in 0..2 {
            assert!(fallback.single_renderer(&node).is_err());
            assert!(!fallback.is_initialized(&node));
            assert_eq!(
                fallback
                    .nodes
                    .initialize(&node, |_| Err::<(), _>("still pending")),
                Err("still pending")
            );
        }
        fallback.remove_node(&node);
        fallback.register_deferred(node, gbm);
        assert!(!fallback.is_initialized(&node));
        fallback.remove_node(&node);
    }

    #[test]
    #[ignore = "requires CLARION_VULKAN_TEST_NODE and hardware EGL support"]
    fn deferred_gles_activates_once_and_reopens_after_device_removal() {
        let (node, gbm) = render_device();
        let mut fallback = GlesFallback::new().unwrap();
        fallback.register_deferred(node, gbm.clone());
        assert!(!fallback.is_initialized(&node));
        let context = fallback
            .single_renderer(&node)
            .unwrap()
            .as_ref()
            .egl_context()
            .get_context_handle();
        assert!(fallback.is_initialized(&node));

        fallback.register_deferred(node, gbm.clone());
        assert_eq!(
            fallback
                .single_renderer(&node)
                .unwrap()
                .as_ref()
                .egl_context()
                .get_context_handle(),
            context,
            "a duplicate shared render node must preserve its existing GLES context"
        );
        fallback.remove_node(&node);
        assert!(!fallback.is_initialized(&node));
        fallback.register_deferred(node, gbm.clone());
        assert!(fallback.single_renderer(&node).is_ok());
        fallback.remove_node(&node);

        // Explicit GLES uses the same manager but retains EGL's node discovery.
        let discovered = fallback.probe_and_register(node, gbm).unwrap();
        assert!(fallback.single_renderer(&discovered).is_ok());
        fallback.remove_node(&discovered);
    }
}
