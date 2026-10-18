use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use std::iter::zip;
use std::num::NonZeroU64;
use std::os::fd::AsFd;
use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::{io, mem};

use anyhow::{anyhow, bail, ensure, Context};
use bytemuck::cast_slice_mut;
use clarion_config::output::Modeline;
use clarion_config::{Config, OutputName};
use clarion_ipc::{HSyncPolarity, VSyncPolarity};
use drm_ffi::drm_mode_modeinfo;
use smithay::backend::allocator::dmabuf::{AsDmabuf, Dmabuf};
use smithay::backend::allocator::format::FormatSet;
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBuffer, GbmBufferFlags, GbmDevice};
use smithay::backend::allocator::{Format, Fourcc};
use smithay::backend::drm::compositor::{
    DrmCompositor, FrameFlags, PrimaryPlaneElement, PrimarySwapchainElement,
};
use smithay::backend::drm::exporter::gbm::GbmFramebufferExporter;
use smithay::backend::drm::gbm::GbmFramebuffer;
use smithay::backend::drm::{DrmDevice, DrmDeviceFd, DrmNode, NodeType};
use smithay::backend::libinput::{LibinputInputBackend, LibinputSessionInterface};
use smithay::backend::renderer::element::RenderElementStates;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::multigpu::gbm::GbmGlesBackend;
use smithay::backend::renderer::multigpu::{MultiFrame, MultiRenderer};
use smithay::backend::renderer::{DebugFlags, ImportDma, RendererSuper};
use smithay::backend::session::libseat::LibSeatSession;
use smithay::backend::session::Session;
use smithay::backend::udev::{self, UdevBackend};
use smithay::desktop::utils::OutputPresentationFeedback;
use smithay::output::{Mode, Output};
use smithay::reexports::calloop::RegistrationToken;
use smithay::reexports::drm::control::atomic::AtomicModeReq;
use smithay::reexports::drm::control::dumbbuffer::DumbBuffer;
use smithay::reexports::drm::control::{
    self, connector, crtc, plane, property, AtomicCommitFlags, Device, Mode as DrmMode, ModeFlags,
    ModeTypeFlags, PlaneType, ResourceHandle,
};
use smithay::reexports::gbm::Modifier;
use smithay::reexports::input::Libinput;
use smithay::reexports::wayland_protocols;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::Transform;
use smithay::wayland::dmabuf::{DmabufFeedback, DmabufFeedbackBuilder, DmabufGlobal};
use smithay::wayland::drm_lease::{
    DrmLease, DrmLeaseBuilder, DrmLeaseRequest, DrmLeaseState, LeaseRejected,
};
use smithay_drm_extras::drm_scanner::DrmScanner;
use wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_dmabuf_feedback_v1::TrancheFlags;

use super::{IpcOutputMap, RenderResult};
use crate::backend::OutputId;
#[cfg(test)]
pub(crate) use crate::output_schedule::take_redraw_state_on_vblank;
use crate::platform::device_bridge::{DevicePort, DeviceSetup, UdevInventory};
use crate::render_helpers::renderer::AsGlesRenderer;
use crate::render_helpers::sync::DeferredFrame;
use crate::render_helpers::{resources, shaders};
use crate::utils::{get_monotonic_time, is_laptop_panel};

pub(crate) mod gles_fallback;
pub(crate) mod hdr;
pub(crate) mod recovery;
pub(crate) mod renderer_init;

pub(crate) type VulkanRecoveryWorker = recovery::RecoveryWorker<
    DrmNode,
    crate::render_helpers::vulkan::native::PreparedNativeDevice,
    Arc<crate::render_helpers::vulkan::VulkanContext>,
>;
use gles_fallback::GlesFallback;
use renderer_init::resolve_gles_render_node;

// When copying from rendering Nvidia dGPU to target iGPU,
// it only understands X/Abgr and not X/Argb.
pub(crate) const SUPPORTED_COLOR_FORMATS_10BIT: [Fourcc; 4] = [
    Fourcc::Abgr2101010,
    Fourcc::Argb2101010,
    Fourcc::Argb8888,
    Fourcc::Abgr8888,
];

// Smithay should fall back to Xrgb/Xbgr automatically if needed.
pub(crate) const SUPPORTED_COLOR_FORMATS: [Fourcc; 2] = [Fourcc::Argb8888, Fourcc::Abgr8888];

pub struct Tty {
    pub(crate) config: Rc<super::DeviceSettings>,
    pub(crate) session: LibSeatSession,
    pub(crate) udev_inventory: UdevInventory,
    pub(crate) libinput: Libinput,
    pub(crate) gpu_manager: GlesFallback,
    // DRM node corresponding to the primary GPU. May or may not be the same as
    // primary_render_node.
    pub(crate) primary_node: DrmNode,
    // DRM render node corresponding to the primary GPU.
    pub(crate) primary_render_node: DrmNode,
    // Ignored DRM nodes.
    pub(crate) ignored_nodes: HashSet<DrmNode>,
    // Devices indexed by DRM node (not necessarily the render node).
    pub(crate) devices: HashMap<DrmNode, OutputDevice>,
    // The dma-buf global corresponds to the output device (the primary GPU). It is only `Some()`
    // if we have a device corresponding to the primary GPU.
    pub(crate) dmabuf_global: Option<DmabufGlobal>,
    // The output config had changed, but the session is paused, so we need to update it on resume.
    pub(crate) update_output_config_on_resume: bool,
    // Whether the debug tinting is enabled.
    pub(crate) debug_tint: bool,
    pub(crate) ipc_outputs: Arc<Mutex<IpcOutputMap>>,
    /// Active native Vulkan hardware renderer for display compositing.
    pub(crate) native_renderer: Option<crate::render_helpers::vulkan::native::NativeRenderer>,
    pub(crate) requested_renderer: crate::backend::RendererBackendType,
    pub(crate) color_management_enabled: bool,
    pub(crate) color_management_rejections: HashMap<String, String>,
    pub(crate) vulkan_recovery: VulkanRecoveryWorker,
    pub(crate) vulkan_recovery_generation: Option<u64>,
    pub(crate) vulkan_recovery_attempt: u8,
    pub(crate) vulkan_recovery_retry_after: Duration,
    pub(crate) vulkan_recovery_watchdog: Option<RegistrationToken>,
    /// Tracy names require static storage; keep one set per connector across output rebuilds.
    pub(crate) tracy_output_names: HashMap<String, TracyOutputNames>,
}

pub type TtyRenderer<'render> = MultiRenderer<
    'render,
    'render,
    GbmGlesBackend<GlesRenderer, DrmDeviceFd>,
    GbmGlesBackend<GlesRenderer, DrmDeviceFd>,
>;

pub type TtyFrame<'render, 'frame, 'buffer> = MultiFrame<
    'render,
    'render,
    'frame,
    'buffer,
    GbmGlesBackend<GlesRenderer, DrmDeviceFd>,
    GbmGlesBackend<GlesRenderer, DrmDeviceFd>,
>;

pub type TtyRendererError<'render> = <TtyRenderer<'render> as RendererSuper>::Error;

pub(crate) fn output_uses_native_renderer(
    native_available: bool,
    primary_allocator_available: bool,
) -> bool {
    native_available && primary_allocator_available
}

#[inline]
pub(crate) fn surface_uses_native_renderer(native_available: bool, native_swapchain: bool) -> bool {
    native_available && native_swapchain
}

pub(crate) fn is_primary_gpu_node(
    node: DrmNode,
    primary_node: DrmNode,
    primary_render_node: DrmNode,
) -> bool {
    node == primary_node
        || node
            .node_with_type(NodeType::Render)
            .is_some_and(|result| result.is_ok_and(|render| render == primary_render_node))
}

pub(crate) fn gles_scanout_formats(formats: &FormatSet, display_only: bool) -> FormatSet {
    formats
        .iter()
        .copied()
        .filter(|format| {
            if display_only {
                return format.modifier == Modifier::Linear;
            }

            !matches!(
                format.modifier,
                Modifier::I915_y_tiled_ccs
                    | Modifier::Unrecognized(0x100000000000005)
                    | Modifier::I915_y_tiled_gen12_rc_ccs
                    | Modifier::I915_y_tiled_gen12_mc_ccs
                    | Modifier::Unrecognized(0x100000000000008)
                    | Modifier::Unrecognized(0x10000000000000a)
                    | Modifier::Unrecognized(0x10000000000000b)
                    | Modifier::Unrecognized(0x10000000000000c)
            )
        })
        .collect()
}

pub(crate) fn vulkan_scanout_formats(
    formats: &FormatSet,
    cross_gpu: bool,
    mut supports_target: impl FnMut(Format) -> bool,
) -> FormatSet {
    formats
        .iter()
        .copied()
        .filter(|format| {
            if cross_gpu && format.modifier != Modifier::Linear {
                return false;
            }
            !matches!(
                format.modifier,
                Modifier::I915_y_tiled_ccs
                    | Modifier::Unrecognized(0x100000000000005)
                    | Modifier::I915_y_tiled_gen12_rc_ccs
                    | Modifier::I915_y_tiled_gen12_mc_ccs
                    | Modifier::Unrecognized(0x100000000000008)
                    | Modifier::Unrecognized(0x10000000000000a)
                    | Modifier::Unrecognized(0x10000000000000b)
                    | Modifier::Unrecognized(0x10000000000000c)
            ) && supports_target(*format)
        })
        .collect()
}

pub(crate) fn probe_vulkan_swapchain_target(
    native: &crate::render_helpers::vulkan::native::NativeRenderer,
    allocator: &GbmAllocator<DrmDeviceFd>,
    width: u32,
    height: u32,
    format: Fourcc,
    modifiers: &[Modifier],
) -> anyhow::Result<()> {
    let flags = GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT;
    let mut allocator = allocator.clone();
    let buffer = allocator
        .create_buffer_with_flags(width, height, format, modifiers, flags)
        .context("cannot allocate a Vulkan swapchain probe buffer")?;
    let dmabuf = buffer
        .export()
        .context("cannot export a Vulkan swapchain probe buffer")?;
    let _texture = native
        .vulkan()
        .import_dmabuf_as_target(&dmabuf)
        .context("Vulkan cannot import the scanout buffer as a render target")?;
    Ok(())
}

pub(crate) fn gles_swapchain_candidates(
    formats: &FormatSet,
    display_only: bool,
    disable_10bit_output: bool,
) -> Vec<(Fourcc, Vec<Modifier>)> {
    let formats = gles_scanout_formats(formats, display_only);
    let colors = if disable_10bit_output {
        &SUPPORTED_COLOR_FORMATS[..]
    } else {
        &SUPPORTED_COLOR_FORMATS_10BIT[..]
    };

    colors
        .iter()
        .copied()
        .filter_map(|code| {
            let modifiers = formats
                .iter()
                .filter(|format| format.code == code)
                .map(|format| format.modifier)
                .collect::<Vec<_>>();
            (!modifiers.is_empty()).then_some((code, modifiers))
        })
        .collect()
}

// Smithay's transparent DRM/damage wrappers skip their contained renderer
// error in Error::source(). Inspect the typed rendering branch before walking
// other error chains, so cold FP16 preparation cannot reject an HDR transition.
fn native_drm_frame_is_backpressure<A, B, F>(
    error: &smithay::backend::drm::compositor::RenderFrameError<
        A,
        B,
        F,
        crate::render_helpers::vulkan::native::NativeError,
    >,
) -> bool
where
    A: std::error::Error + Send + Sync + 'static,
    B: std::error::Error + Send + Sync + 'static,
    F: std::error::Error + Send + Sync + 'static,
{
    use smithay::backend::drm::compositor::RenderFrameError;
    use smithay::backend::renderer::damage::Error as DamageError;

    use crate::render_helpers::vulkan::native::NativeError;

    matches!(error, RenderFrameError::RenderFrame(DamageError::Rendering(error)) if error.is_busy())
        || NativeError::is_backpressure(error)
}

fn cursor_color_is_sdr(description: crate::color::ColorDescription, format: u32) -> bool {
    // Floating-point pixels can exceed SDR or premultiplied-alpha bounds
    // even with the default description. Keep them in FP16 composition so
    // hardware cursor conversion cannot clip them before background blending.
    description == crate::color::ColorDescription::srgb()
        && format != ash::vk::Format::R16G16B16A16_SFLOAT.as_raw() as u32
}

/// Client cursor surfaces may themselves carry PQ/scRGB declarations. Inspect
/// the frozen texture metadata before allowing any raw cursor-buffer scanout.
fn native_cursor_elements_are_sdr(
    elements: &[crate::scene::OutputRenderElements<
        crate::render_helpers::vulkan::native::NativeRenderer,
    >],
) -> bool {
    use smithay::backend::renderer::element::surface::WaylandSurfaceTexture;
    use smithay::backend::renderer::element::{Element, Kind};

    use crate::scene::{OutputRenderElements, PointerRenderElements};

    elements.iter().filter(|element| element.kind() == Kind::Cursor).all(|element| {
        match element {
            OutputRenderElements::Pointer(PointerRenderElements::NamedPointer(_)) => true,
            OutputRenderElements::Pointer(PointerRenderElements::Wayland(element)) => {
                matches!(element.texture(), WaylandSurfaceTexture::Texture(texture) if cursor_color_is_sdr(texture.color.description, texture.format))
            }
            _ => false,
        }
    })
}

pub(crate) type GbmDrmCompositor = DrmCompositor<
    GbmAllocator<DrmDeviceFd>,
    GbmFramebufferExporter<DrmDeviceFd>,
    (OutputPresentationFeedback, Duration),
    DrmDeviceFd,
>;

pub struct OutputDevice {
    pub(crate) token: RegistrationToken,
    // The hardware registration survives a display-only compatibility fallback.
    // Keep it separate so removal releases the deferred GBM resource as well.
    pub(crate) registered_render_node: Option<DrmNode>,
    // Can be None for display-only devices such as DisplayLink.
    pub(crate) render_node: Option<DrmNode>,
    pub(crate) gles_classification_pending: bool,
    pub(crate) drm_scanner: DrmScanner,
    pub(crate) surfaces: HashMap<crtc::Handle, Surface>,
    pub(crate) known_crtcs: HashMap<crtc::Handle, CrtcInfo>,
    // SAFETY: drop after all the objects used with them are dropped.
    // See https://github.com/Smithay/smithay/issues/1102.
    pub(crate) drm: DrmDevice,
    pub(crate) gbm: GbmDevice<DrmDeviceFd>,
    // For display-only devices this will be the allocator from the primary device.
    pub(crate) allocator: GbmAllocator<DrmDeviceFd>,

    pub drm_lease_state: Option<DrmLeaseState>,
    pub(crate) non_desktop_connectors: HashSet<(connector::Handle, crtc::Handle)>,
    pub(crate) active_leases: Vec<DrmLease>,
}

/// Retain the old display-only fallback when a secondary GPU cannot initialize
/// EGL. Resolve it only when native Vulkan actually needs GLES, and commit the
/// new allocator/node only after the primary compatibility renderer is usable.
pub(crate) fn prepare_gles_device(
    manager: &mut GlesFallback,
    render_node: &mut Option<DrmNode>,
    pending: &mut bool,
    allocator: &mut GbmAllocator<DrmDeviceFd>,
    primary: DrmNode,
    primary_allocator: Option<&GbmAllocator<DrmDeviceFd>>,
) -> anyhow::Result<()> {
    if !*pending {
        return Ok(());
    }
    let resolved = resolve_gles_render_node(*render_node, primary, manager, |manager, node| {
        manager.ensure_registered(&node).inspect_err(|err| {
            if node != primary {
                debug!("secondary EGL unavailable; trying primary display-only fallback: {err:#}");
            }
        })
    })?;
    let fallback_allocator = if resolved.is_none() {
        Some(
            primary_allocator
                .context("no primary allocator for GLES fallback")?
                .clone(),
        )
    } else {
        None
    };
    // Context creation may still fail after EGLDisplay initialization. Retain
    // the original device state so a later retry can recover either renderer.
    let _renderer = manager.single_renderer(&resolved.unwrap_or(primary))?;
    if let Some(fallback_allocator) = fallback_allocator {
        *allocator = fallback_allocator;
    }
    *render_node = resolved;
    *pending = false;
    Ok(())
}

// A connected, but not necessarily enabled, crtc.
#[derive(Debug, Clone)]
pub struct CrtcInfo {
    pub(crate) id: OutputId,
    pub(crate) name: OutputName,
}

impl OutputDevice {
    pub fn lease_request(
        &self,
        request: DrmLeaseRequest,
    ) -> Result<DrmLeaseBuilder, LeaseRejected> {
        let mut builder = DrmLeaseBuilder::new(&self.drm);
        for connector in request.connectors {
            let (_, crtc) = self
                .non_desktop_connectors
                .iter()
                .find(|(conn, _)| connector == *conn)
                .ok_or_else(|| {
                    warn!("Attempted to lease connector that is not non-desktop");
                    LeaseRejected::default()
                })?;
            builder.add_connector(connector);
            builder.add_crtc(*crtc);
            let planes = self.drm.planes(crtc).map_err(LeaseRejected::with_cause)?;
            let (primary_plane, primary_plane_claim) = planes
                .primary
                .iter()
                .find_map(|plane| {
                    self.drm
                        .claim_plane(plane.handle, *crtc)
                        .map(|claim| (plane, claim))
                })
                .ok_or_else(LeaseRejected::default)?;
            builder.add_plane(primary_plane.handle, primary_plane_claim);
        }
        Ok(builder)
    }

    pub fn new_lease(&mut self, lease: DrmLease) {
        self.active_leases.push(lease);
    }

    pub fn remove_lease(&mut self, lease_id: u32) {
        self.active_leases.retain(|l| l.id() != lease_id);
    }

    pub fn known_crtc_name(
        &self,
        crtc: &crtc::Handle,
        conn: &connector::Info,
        disable_monitor_names: bool,
    ) -> OutputName {
        if disable_monitor_names {
            let conn_name = format_connector_name(conn);
            return OutputName {
                connector: conn_name,
                make: None,
                model: None,
                serial: None,
            };
        }

        let Some(info) = self.known_crtcs.get(crtc) else {
            let conn_name = format_connector_name(conn);
            error!("crtc for connector {conn_name} missing from known");
            return OutputName {
                connector: conn_name,
                make: None,
                model: None,
                serial: None,
            };
        };
        info.name.clone()
    }

    pub(crate) fn cleanup_mismatching_resources(
        &self,
        should_be_off: &dyn Fn(crtc::Handle, &connector::Info) -> bool,
    ) -> anyhow::Result<()> {
        let _span = clarion_tracy_span!("OutputDevice::cleanup_disconnected_resources");

        let res_handles = self
            .drm
            .resource_handles()
            .context("error getting plane handles")?;
        let plane_handles = self
            .drm
            .plane_handles()
            .context("error getting plane handles")?;

        let mut req = AtomicModeReq::new();

        // We want to disable all CRTCs that do not correspond to a connector we're using.
        let mut cleanup = HashSet::<crtc::Handle>::new();
        cleanup.extend(res_handles.crtcs());

        for (conn, info) in self.drm_scanner.connectors() {
            // We only keep the connector if it has a CRTC and the output isn't off in clarion.
            if let Some(crtc) = self.drm_scanner.crtc_for_connector(conn) {
                // Verify that the connector's current CRTC matches the CRTC we expect. If not,
                // clear the CRTC and the connector so that all connectors can get the expected
                // CRTCs afterwards. (We do this because we do not handle CRTC rotations across TTY
                // switches.)
                let mut has_different_crtc = false;
                if let Some(enc) = info.current_encoder() {
                    match self.drm.get_encoder(enc) {
                        Ok(enc) => {
                            if let Some(current_crtc) = enc.crtc() {
                                if current_crtc != crtc {
                                    has_different_crtc = true;
                                }
                            }
                        }
                        Err(err) => {
                            debug!("couldn't get encoder: {err:?}");
                            // Err on the safe side.
                            has_different_crtc = true;
                        }
                    }
                }

                if !has_different_crtc && !should_be_off(crtc, info) {
                    // Keep the corresponding CRTC.
                    cleanup.remove(&crtc);
                    continue;
                }
            }

            // Clear the connector.
            let Some((crtc_id, _, _)) = find_drm_property(&self.drm, *conn, "CRTC_ID") else {
                debug!("couldn't find connector CRTC_ID property");
                continue;
            };

            req.add_property(*conn, crtc_id, property::Value::CRTC(None));
        }

        // Legacy fallback.
        if !self.drm.is_atomic() {
            for crtc in res_handles.crtcs() {
                #[allow(deprecated)]
                let _ = self.drm.set_cursor(*crtc, Option::<&DumbBuffer>::None);
            }
            for crtc in cleanup {
                let _ = self.drm.set_crtc(crtc, None, (0, 0), &[], None);
            }
            return Ok(());
        }

        // Disable non-primary planes, and planes belonging to disabled CRTCs.
        let is_primary = |plane: plane::Handle| {
            if let Some((_, info, value)) = find_drm_property(&self.drm, plane, "type") {
                match info.value_type().convert_value(value) {
                    property::Value::Enum(Some(val)) => val.value() == PlaneType::Primary as u64,
                    _ => false,
                }
            } else {
                debug!("couldn't find plane type property");
                false
            }
        };

        for plane in plane_handles {
            let info = match self.drm.get_plane(plane) {
                Ok(x) => x,
                Err(err) => {
                    debug!("error getting plane: {err:?}");
                    continue;
                }
            };

            let Some(crtc) = info.crtc() else {
                continue;
            };

            if !cleanup.contains(&crtc) && is_primary(plane) {
                continue;
            }

            let Some((crtc_id, _, _)) = find_drm_property(&self.drm, plane, "CRTC_ID") else {
                debug!("couldn't find plane CRTC_ID property");
                continue;
            };

            let Some((fb_id, _, _)) = find_drm_property(&self.drm, plane, "FB_ID") else {
                debug!("couldn't find plane FB_ID property");
                continue;
            };

            req.add_property(plane, crtc_id, property::Value::CRTC(None));
            req.add_property(plane, fb_id, property::Value::Framebuffer(None));
        }

        // Disable the CRTCs.
        for crtc in cleanup {
            let Some((mode_id, _, _)) = find_drm_property(&self.drm, crtc, "MODE_ID") else {
                debug!("couldn't find CRTC MODE_ID property");
                continue;
            };

            let Some((active, _, _)) = find_drm_property(&self.drm, crtc, "ACTIVE") else {
                debug!("couldn't find CRTC ACTIVE property");
                continue;
            };

            req.add_property(crtc, mode_id, property::Value::Unknown(0));
            req.add_property(crtc, active, property::Value::Boolean(false));
        }

        self.drm
            .atomic_commit(AtomicCommitFlags::ALLOW_MODESET, req)
            .context("error doing atomic commit")?;

        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct TtyOutputState {
    pub(crate) node: DrmNode,
    pub(crate) crtc: crtc::Handle,
}

pub(crate) struct Surface {
    pub(crate) name: OutputName,
    pub(crate) compositor: GbmDrmCompositor,
    pub(crate) frame_epoch: u64,
    /// A prepared frame awaiting a non-exportable fence. The primary element
    /// owns its swapchain slot; no later render may recycle it before readiness.
    pub(crate) pending_kms: Option<DeferredFrame<PendingKmsFrame>>,
    /// Failed GPU frames keep their slots outside the replacement swapchain.
    /// Never recycle storage whose producer has not confirmed completion.
    pub(crate) quarantined_kms: Vec<DeferredFrame<PendingKmsFrame>>,
    /// Set until the KMS swapchain has been converted to a GLES-compatible format.
    pub(crate) native_swapchain: bool,
    pub(crate) hdr: hdr::HdrOutputState,
    pub(crate) last_gles_migration_warning: Option<Duration>,
    pub(crate) next_gles_migration_attempt: Duration,
    pub(crate) connector: connector::Handle,
    pub(crate) dmabuf_feedback: Option<SurfaceDmabufFeedback>,
    pub(crate) gamma_props: Option<GammaProps>,
    /// Gamma change to apply upon session resume.
    pub(crate) pending_gamma_change: Option<Option<Vec<u16>>>,
    /// Tracy frame that goes from vblank to vblank.
    pub(crate) vblank_frame: Option<tracy_client::Frame>,
    /// Frame name for the VBlank frame.
    pub(crate) vblank_frame_name: tracy_client::FrameName,
    /// Plot name for the time since presentation plot.
    pub(crate) time_since_presentation_plot_name: tracy_client::PlotName,
    /// Plot name for the presentation misprediction plot.
    pub(crate) presentation_misprediction_plot_name: tracy_client::PlotName,
    pub(crate) sequence_delta_plot_name: tracy_client::PlotName,
}

#[derive(Clone, Copy, PartialEq)]
struct KmsFrameIdentity {
    epoch: u64,
    locked: bool,
    mode: Option<Mode>,
    scale: f64,
    transform: Transform,
    native_swapchain: bool,
    format: Fourcc,
}

impl KmsFrameIdentity {
    fn new(
        output: &Output,
        epoch: u64,
        locked: bool,
        native_swapchain: bool,
        format: Fourcc,
    ) -> Self {
        Self {
            epoch,
            locked,
            mode: output.current_mode(),
            scale: output.current_scale().fractional_scale(),
            transform: output.current_transform(),
            native_swapchain,
            format,
        }
    }
}

pub(crate) struct PendingKmsFrame {
    _primary: PrimarySwapchainElement<GbmBuffer, GbmFramebuffer>,
    states: RenderElementStates,
    is_empty: bool,
    identity: KmsFrameIdentity,
    feedback: Option<OutputPresentationFeedback>,
}

/// Publish feedback only for a frame that can actually be queued. Smithay owns
/// its prepared next_frame while a PendingKmsFrame retains the swapchain slot.
fn submit_kms_frame(
    surface: &mut Surface,
    port: &mut impl crate::frame_pipeline::FramePort,
    states: RenderElementStates,
    is_empty: bool,
    target: Duration,
    feedback: Option<OutputPresentationFeedback>,
) -> RenderResult {
    port.update_scanout(&states);
    if let Some(feedback) = surface.dmabuf_feedback.as_ref() {
        port.send_dmabuf_feedback(feedback, &states);
    }
    if is_empty {
        drop(surface.vblank_frame.take());
        port.estimated_vblank(target);
        return RenderResult::NoDamage;
    }
    let feedback = feedback.unwrap_or_else(|| port.take_presentation_feedback(&states));
    let result = {
        let _latency =
            crate::utils::latency::Span::new(crate::utils::latency::Operation::KmsSubmit);
        surface.compositor.queue_frame((feedback, target))
    };
    match result {
        Ok(()) => {
            surface
                .hdr
                .refresh_active(surface.compositor.surface(), surface.connector);
            port.kms_submitted();
            RenderResult::Submitted
        }
        Err(error) => {
            surface.hdr.reject_transition(&error.to_string());
            surface.compositor.reset_buffer_ages();
            drop(surface.vblank_frame.take());
            port.kms_submission_failed(target, &error);
            RenderResult::Skipped
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct TracyOutputNames {
    pub(crate) vblank_frame: tracy_client::FrameName,
    pub(crate) time_since_presentation: tracy_client::PlotName,
    pub(crate) presentation_misprediction: tracy_client::PlotName,
    pub(crate) sequence_delta: tracy_client::PlotName,
}

pub struct SurfaceDmabufFeedback {
    pub render: DmabufFeedback,
    pub scanout: DmabufFeedback,
}

pub(crate) struct GammaProps {
    pub(crate) crtc: crtc::Handle,
    pub(crate) gamma_lut: property::Handle,
    pub(crate) gamma_lut_size: property::Handle,
    pub(crate) previous_blob: Option<NonZeroU64>,
}

pub(crate) struct ConnectorProperties<'a> {
    pub(crate) device: &'a DrmDevice,
    pub(crate) connector: connector::Handle,
    pub(crate) properties: Vec<(property::Info, property::RawValue)>,
    pub(crate) has_change: bool,
    pub(crate) requests: AtomicModeReq,
}

impl Tty {
    /// Whether every connected output can decode managed client colors.
    pub(crate) fn supports_color_management(&self) -> bool {
        self.native_renderer
            .as_ref()
            .is_some_and(|renderer| renderer.supports_color_management())
            && self
                .devices
                .values()
                .flat_map(|device| device.surfaces.values())
                .all(|surface| surface.native_swapchain)
    }

    pub(crate) fn enable_color_management(&mut self) {
        debug_assert!(self.supports_color_management());
        self.color_management_enabled = true;
    }

    pub(crate) fn output_color_description(
        &self,
        output: &Output,
    ) -> crate::color::ColorDescription {
        let Some(tty) = output.user_data().get::<TtyOutputState>() else {
            return Default::default();
        };
        self.devices
            .get(&tty.node)
            .and_then(|device| device.surfaces.get(&tty.crtc))
            .and_then(|surface| surface.hdr.active)
            .map(|settings| settings.description())
            .unwrap_or_default()
    }

    /// Reports the primary scene renderer; individual KMS outputs can fall back to GLES.
    pub fn renderer_backend(&self) -> crate::render_helpers::renderer::RendererBackendType {
        if self.native_renderer.is_some() {
            crate::render_helpers::renderer::RendererBackendType::Vulkan
        } else {
            crate::render_helpers::renderer::RendererBackendType::Gles
        }
    }

    pub fn with_renderer<T>(
        &mut self,
        f: impl FnOnce(crate::render_helpers::renderer::AnyRenderer<'_>) -> T,
    ) -> Option<T> {
        if let Some(native) = self.native_renderer.as_mut() {
            Some(f(crate::render_helpers::renderer::AnyRenderer::Native(
                native,
            )))
        } else {
            self.with_gles_renderer(f)
        }
    }

    /// Access the renderer compatible with a particular KMS output.
    ///
    /// Vulkan is initialized once for the primary GPU, but each output owns its own KMS
    /// swapchain. A failed Vulkan target import can migrate just one of those swapchains to GLES;
    /// in that case using the global native renderer would produce textures and snapshots that
    /// the GLES output cannot consume.
    pub fn with_renderer_for_output<T>(
        &mut self,
        output: Option<&Output>,
        f: impl FnOnce(crate::render_helpers::renderer::AnyRenderer<'_>) -> T,
    ) -> Option<T> {
        let use_gles = output.is_some_and(|output| !self.output_uses_native_renderer(output));
        if use_gles {
            self.with_gles_renderer(f)
        } else {
            self.with_renderer(f)
        }
    }

    /// Whether the given output still has a Vulkan-compatible KMS swapchain.
    ///
    /// Outputs which have gone away may still be referenced by a queued cast or screenshot. In
    /// that case retain the global renderer choice; there is no live surface to target.
    pub(crate) fn output_uses_native_renderer(&self, output: &Output) -> bool {
        let Some(state) = output.user_data().get::<TtyOutputState>() else {
            return self.native_renderer.is_some();
        };
        let native_swapchain = self
            .devices
            .get(&state.node)
            .and_then(|device| device.surfaces.get(&state.crtc))
            .is_none_or(|surface| surface.native_swapchain);
        surface_uses_native_renderer(self.native_renderer.is_some(), native_swapchain)
    }

    pub(crate) fn with_gles_renderer<T>(
        &mut self,
        f: impl FnOnce(crate::render_helpers::renderer::AnyRenderer<'_>) -> T,
    ) -> Option<T> {
        let mut renderer = self
            .gpu_manager
            .single_renderer(&self.primary_render_node)
            .ok()?;
        let gles = renderer.as_gles_renderer()?;
        Self::init_gles_renderer(&self.config, gles);
        Some(f(crate::render_helpers::renderer::AnyRenderer::Gles(gles)))
    }

    pub(crate) fn create_vulkan_renderer(
        render_node: DrmNode,
        config: &super::DeviceSettings,
    ) -> Option<crate::render_helpers::vulkan::native::NativeRenderer> {
        let mut vk_renderer = crate::render_helpers::vulkan::VulkanRenderer::new();
        match vk_renderer.init_from_drm_node(Some(&render_node)) {
            Ok(()) => {
                info!(
                    "Vulkan direct KMS renderer initialized on {:?}: {} (Dynamic Rendering: {}, DRM Modifiers: {}, Timeline Semaphores: {})",
                    render_node,
                    vk_renderer.info.device_name,
                    vk_renderer.info.supports_dynamic_rendering,
                    vk_renderer.info.supports_drm_modifiers,
                    vk_renderer.info.supports_timeline_semaphores,
                );
                match crate::render_helpers::vulkan::native::NativeRenderer::new(vk_renderer) {
                    Ok(mut native) => {
                        shaders::init_native(&mut native, &config.animations);
                        info!("Native Vulkan desktop renderer ready for direct KMS submission!");
                        Some(native)
                    }
                    Err(err) => {
                        warn!("Failed to create NativeRenderer from VulkanRenderer: {err:?}, running in GLES fallback mode");
                        None
                    }
                }
            }
            Err(err) => {
                warn!("Failed to initialize Vulkan backend on DRM node {render_node:?}: {err:?}, running in GLES fallback mode");
                None
            }
        }
    }

    pub(crate) fn init(&mut self, port: &mut impl DevicePort) {
        port.initialize_tty(self);
    }
    pub(crate) fn set_output_on_demand_vrr(
        &mut self,
        port: &mut impl DevicePort,
        output: &Output,
        enabled: bool,
    ) {
        port.set_tty_vrr(self, output, enabled);
    }
    pub(crate) fn update_ignored_nodes_config(&mut self, port: &mut impl DevicePort) {
        port.update_tty_ignored_nodes(self);
    }
    pub(crate) fn on_output_config_changed(&mut self, port: &mut impl DevicePort) {
        port.configure_tty_outputs(self);
    }
    pub(crate) fn on_estimated_vblank_timer(port: &mut impl DevicePort, output: Output) {
        port.estimated_vblank(output);
    }

    /// Turns every connected panel on or off through each connector's DPMS
    /// property. Blanking is the power-saving half of the idle policy; the
    /// session itself stays composited so input can wake it immediately.
    pub(crate) fn set_outputs_blanked(&mut self, blanked: bool) {
        let _span = clarion_tracy_span!("Tty::set_outputs_blanked");

        for device in self.devices.values() {
            for surface in device.surfaces.values() {
                let Ok(mut props) = ConnectorProperties::try_new(&device.drm, surface.connector)
                else {
                    debug!("failed to read connector properties for DPMS");
                    continue;
                };
                if let Err(err) = props.set_dpms(!blanked) {
                    debug!("failed to set DPMS for {:?}: {err:?}", surface.name);
                    continue;
                }
                if let Err(err) = props.commit() {
                    warn!(
                        "failed to commit the DPMS change for {:?}: {err:?}",
                        surface.name
                    );
                }
            }
        }
    }

    pub(crate) fn new(
        config: Rc<RefCell<Config>>,
        setup: &mut impl DeviceSetup,
        requested_renderer: crate::backend::RendererBackendType,
    ) -> anyhow::Result<Self> {
        let _span = clarion_tracy_span!("Tty::new");
        let config = Rc::new(super::DeviceSettings::from(&*config.borrow()));

        let (session, notifier) = LibSeatSession::new().context(
            "Error creating a session. This might mean that you're trying to run clarion on a TTY \
             that is already busy, for example if you're running this inside tmux that had been \
             originally started on a different TTY",
        )?;
        let seat_name = session.seat();

        let udev_backend =
            UdevBackend::new(session.seat()).context("error creating a udev backend")?;
        let udev_inventory = setup.register_udev(udev_backend);

        let mut libinput = Libinput::new_with_udev(LibinputSessionInterface::from(session.clone()));
        unsafe { init_libinput_plugin_system(&libinput) };
        {
            let _span = clarion_tracy_span!("Libinput::udev_assign_seat");
            libinput.udev_assign_seat(&seat_name)
        }
        .map_err(|()| anyhow!("error assigning the seat to libinput"))?;

        // If the session is not active at startup (e.g. clarion was launched from a different TTY),
        // suspend libinput now so that when ActivateSession fires, libinput.resume() performs a
        // full re-enumeration of input devices instead of being a no-op.
        if !session.is_active() {
            debug!("session is not active, starting libinput in paused state");
            libinput.suspend();
        }

        let input_backend = LibinputInputBackend::new(libinput.clone());
        setup.register_libinput(input_backend);
        setup.register_session(notifier);

        let gpu_manager = GlesFallback::new()?;

        let (primary_node, primary_render_node) =
            primary_node_from_config(&config).ok_or(()).or_else(|()| {
                let primary_gpu_path = udev::primary_gpu(&seat_name)
                    .context("error getting the primary GPU")?
                    .context("couldn't find a GPU")?;
                let primary_node = DrmNode::from_path(primary_gpu_path)
                    .context("error opening the primary GPU DRM node")?;
                let primary_render_node = primary_node
                    .node_with_type(NodeType::Render)
                    .and_then(Result::ok)
                    .unwrap_or_else(|| {
                        warn!(
                            "error getting the render node for the primary GPU; proceeding anyway"
                        );
                        primary_node
                    });

                Ok::<_, anyhow::Error>((primary_node, primary_render_node))
            })?;

        let mut node_path = String::new();
        if let Some(path) = primary_render_node.dev_path() {
            write!(node_path, "{path:?}").unwrap();
        } else {
            write!(node_path, "{primary_render_node}").unwrap();
        }
        info!("using as the render node: {node_path}");

        let (recovery_ping, recovery_source) = smithay::reexports::calloop::ping::make_ping()
            .context("error creating Vulkan recovery wakeup")?;
        let vulkan_recovery = VulkanRecoveryWorker::new(
            recovery_ping,
            crate::render_helpers::vulkan::native::PreparedNativeDevice::prepare,
            |context| Arc::strong_count(context) == 1,
        )
        .context("error starting Vulkan recovery worker")?;
        setup.register_vulkan_recovery(recovery_source)?;

        Ok(Self {
            config,
            session,
            udev_inventory,
            libinput,
            gpu_manager,
            primary_node,
            primary_render_node,
            ignored_nodes: HashSet::new(),
            devices: HashMap::new(),
            dmabuf_global: None,
            update_output_config_on_resume: false,
            debug_tint: false,
            ipc_outputs: Arc::new(Mutex::new(HashMap::new())),
            native_renderer: None,
            requested_renderer,
            color_management_enabled: false,
            color_management_rejections: HashMap::new(),
            vulkan_recovery,
            vulkan_recovery_generation: None,
            vulkan_recovery_attempt: 0,
            vulkan_recovery_retry_after: Duration::ZERO,
            vulkan_recovery_watchdog: None,
            tracy_output_names: HashMap::new(),
        })
    }

    pub(crate) fn primary_allocator(&self) -> Option<GbmAllocator<DrmDeviceFd>> {
        self.devices
            .get(&self.primary_node)
            .or_else(|| {
                self.devices
                    .iter()
                    .find(|(node, _)| {
                        is_primary_gpu_node(**node, self.primary_node, self.primary_render_node)
                    })
                    .map(|(_, device)| device)
            })
            .map(|device| device.allocator.clone())
    }

    pub fn seat_name(&self) -> String {
        self.session.seat()
    }

    /// Compile the GLES shader programs (and custom animation shaders from the
    /// config) on the first use of an EGL context.
    pub(crate) fn init_gles_renderer(
        config: &super::DeviceSettings,
        gles_renderer: &mut GlesRenderer,
    ) {
        if gles_renderer
            .egl_context()
            .user_data()
            .get::<shaders::Shaders>()
            .is_some()
        {
            return;
        }

        resources::init(gles_renderer);
        shaders::init(gles_renderer);

        if let Some(src) = config.animations.window_resize.custom_shader.as_deref() {
            shaders::set_custom_resize_program(gles_renderer, Some(src));
        }
        let close_src = shaders::resolve_close_shader(
            config.animations.window_close.custom_shader.as_deref(),
            config.animations.window_close.effect,
        );
        if let Some(src) = close_src {
            shaders::set_custom_close_program(gles_renderer, Some(src));
        }
        if let Some(src) = config.animations.window_open.custom_shader.as_deref() {
            shaders::set_custom_open_program(gles_renderer, Some(src));
        }
    }

    pub fn with_primary_renderer<T>(
        &mut self,
        f: impl FnOnce(&mut GlesRenderer) -> T,
    ) -> Option<T> {
        if self.native_renderer.is_some() {
            return None;
        }
        let mut renderer = self
            .gpu_manager
            .single_renderer(&self.primary_render_node)
            .ok()?;
        let gles_renderer = renderer.as_gles_renderer()?;
        Self::init_gles_renderer(&self.config, gles_renderer);
        Some(f(gles_renderer))
    }

    pub(crate) fn render(
        &mut self,
        port: &mut impl crate::frame_pipeline::FramePort,
        output: &Output,
        target_presentation_time: Duration,
    ) -> RenderResult {
        let span = clarion_tracy_span!("Tty::render");
        let _latency = crate::utils::latency::Span::new(crate::utils::latency::Operation::Render);

        let rv = RenderResult::Skipped;

        let tty_state: &TtyOutputState = output.user_data().get().unwrap();
        // Only a lost native renderer leaves an existing native swapchain needing
        // its first EGL classification. Normal native frames do not clone an allocator.
        let primary_allocator = if self.native_renderer.is_none()
            && self
                .devices
                .get(&tty_state.node)
                .is_some_and(|device| device.gles_classification_pending)
        {
            self.primary_allocator()
        } else {
            None
        };
        let Some(device) = self.devices.get_mut(&tty_state.node) else {
            error!("missing output device");
            return rv;
        };

        let Some(surface) = device.surfaces.get_mut(&tty_state.crtc) else {
            error!("missing surface");
            return rv;
        };

        span.emit_text(&surface.name.connector);

        if !device.drm.is_active() {
            // This branch hits any time we try to render while the user had switched to a
            // different VT, so don't print anything here.
            return rv;
        }

        if surface.pending_kms.is_some() {
            let pending = match DeferredFrame::take_ready(&mut surface.pending_kms) {
                Ok(Some(pending)) => pending,
                Ok(None) => {
                    // Keep the exact prepared frame and slot. A new render_frame
                    // would drop Smithay's next_frame and allow premature reuse.
                    port.retry_after_vblank(target_presentation_time);
                    return RenderResult::GpuDeferred;
                }
                Err(error) => {
                    if let Some(frame) = surface.pending_kms.as_mut() {
                        frame.update_retained(|frame| drop(frame.feedback.take()));
                    }
                    // Bound quarantine per output. An unrecoverable series of
                    // failures may stop this output, never grow without limit
                    // or free a slot that unfinished GPU work could still use.
                    if surface.quarantined_kms.len() >= 4 {
                        port.retry_after_vblank(get_monotonic_time() + Duration::from_millis(500));
                        return RenderResult::GpuDeferred;
                    }
                    warn!(
                        ?error,
                        "prepared KMS frame failed; isolating its slot before Vulkan recovery"
                    );
                    surface
                        .quarantined_kms
                        .push(surface.pending_kms.take().unwrap());
                    surface.compositor.reset_buffers();
                    surface.compositor.reset_buffer_ages();
                    if let Some(native) = self.native_renderer.as_ref() {
                        let anchor = native.device_anchor();
                        match self
                            .vulkan_recovery
                            .request(self.primary_render_node, Some(anchor))
                        {
                            Ok(generation) => {
                                self.vulkan_recovery_generation = Some(generation);
                                self.vulkan_recovery_attempt = 1;
                                self.native_renderer = None;
                            }
                            Err(_) => {
                                self.vulkan_recovery_retry_after =
                                    get_monotonic_time() + Duration::from_millis(500);
                            }
                        }
                    }
                    port.retry_after_vblank(target_presentation_time);
                    return RenderResult::GpuDeferred;
                }
            };
            // Query the output only when a retained frame actually becomes
            // ready. Normal frames never need an identity or its output locks.
            let frame_identity = KmsFrameIdentity::new(
                output,
                surface.frame_epoch,
                port.request().locked,
                surface.native_swapchain,
                surface.compositor.format(),
            );
            if pending.identity == frame_identity {
                let result = submit_kms_frame(
                    surface,
                    port,
                    pending.states,
                    pending.is_empty,
                    target_presentation_time,
                    pending.feedback,
                );
                // New commits/animation endpoints coalesced while A waited.
                // Presenting A must not consume their request for the next frame.
                if matches!(result, RenderResult::Submitted | RenderResult::NoDamage) {
                    port.queue_followup_redraw();
                }
                return result;
            }
            // Never present an old unlocked composition after a lock request,
            // or an image prepared for a different output generation. Readiness
            // was required before releasing the retained slot and rendering anew.
            surface.compositor.reset_buffer_ages();
        }

        if surface.native_swapchain && self.vulkan_recovery_generation.is_some() {
            // The failed renderer is detached, but KMS keeps its last committed buffer.
            // Do not migrate swapchains or consume client leases during initialization.
            port.retry_after_vblank(target_presentation_time);
            return RenderResult::GpuDeferred;
        }
        if surface.native_swapchain && !self.vulkan_recovery_retry_after.is_zero() {
            let now = get_monotonic_time();
            if now >= self.vulkan_recovery_retry_after {
                if let Some(native) = self.native_renderer.as_ref() {
                    // Capacity refusal retained this failed renderer. Retry only
                    // mailbox admission, never another draw on the lost device.
                    match self
                        .vulkan_recovery
                        .request(self.primary_render_node, Some(native.device_anchor()))
                    {
                        Ok(generation) => {
                            self.vulkan_recovery_generation = Some(generation);
                            self.vulkan_recovery_attempt = 1;
                            self.vulkan_recovery_retry_after = Duration::ZERO;
                            self.native_renderer = None;
                        }
                        Err(_) => {
                            self.vulkan_recovery_retry_after = now + Duration::from_millis(500);
                        }
                    }
                } else {
                    self.vulkan_recovery_retry_after = Duration::ZERO;
                }
            }
            port.retry_after_vblank(
                self.vulkan_recovery_retry_after
                    .max(target_presentation_time),
            );
            return RenderResult::GpuDeferred;
        }

        if self.color_management_enabled
            && surface.native_swapchain
            && self.native_renderer.is_none()
        {
            // Clients may retain PQ/scRGB surfaces for the lifetime of the
            // advertised global. Preserve the last correct image while the
            // bounded native device recovery runs; GLES cannot decode them.
            let status = "native Vulkan renderer unavailable; preserving last color-correct frame during recovery";
            if surface.hdr.status.as_deref() != Some(status) {
                warn!("output {}: {status}", surface.name.connector);
                surface.hdr.status = Some(status.into());
                surface.hdr.ipc_dirty = true;
                if let Some(output) = self
                    .ipc_outputs
                    .lock()
                    .unwrap()
                    .values_mut()
                    .find(|output| output.name == surface.name.connector)
                {
                    output.hdr_status = Some(status.into());
                }
            }
            port.retry_after_vblank(
                target_presentation_time.max(get_monotonic_time() + Duration::from_millis(100)),
            );
            return RenderResult::GpuDeferred;
        }

        if surface
            .hdr
            .status
            .as_ref()
            .is_some_and(|status| status.starts_with("native Vulkan renderer unavailable"))
        {
            surface.hdr.resume();
        }
        let output_config = self
            .config
            .as_ref()
            .outputs
            .find(&surface.name)
            .cloned()
            .unwrap_or_default();
        if surface.hdr.configure(
            &output_config,
            surface.native_swapchain
                && self
                    .native_renderer
                    .as_ref()
                    .is_some_and(|renderer| renderer.supports_color_management()),
            surface.compositor.format(),
            !surface.compositor.surface().is_legacy(),
        ) {
            surface.frame_epoch = surface.frame_epoch.wrapping_add(1);
            surface.compositor.reset_buffer_ages();
        }
        if surface.hdr.needs_stage {
            if get_monotonic_time() < surface.hdr.retry_after {
                port.retry_after_vblank(surface.hdr.retry_after);
                return RenderResult::GpuDeferred;
            }
            if surface.compositor.has_pending_frames() {
                port.retry_after_vblank(target_presentation_time);
                return RenderResult::GpuDeferred;
            }
            if let Err(error) =
                surface
                    .hdr
                    .stage(&device.drm, surface.compositor.surface(), surface.connector)
            {
                surface.hdr.reject_transition(&error.to_string());
                // Do not display an image until signaling for that image can be staged.
                port.retry_after_vblank(target_presentation_time);
                return RenderResult::Skipped;
            }
            surface.compositor.reset_buffer_ages();
        }

        let mut flags = {
            let debug = &self.config.debug;

            let primary_scanout_flag = if debug.restrict_primary_scanout_to_matching_format {
                FrameFlags::ALLOW_PRIMARY_PLANE_SCANOUT
            } else {
                FrameFlags::ALLOW_PRIMARY_PLANE_SCANOUT_ANY
            };
            let mut flags = primary_scanout_flag | FrameFlags::ALLOW_CURSOR_PLANE_SCANOUT;

            if debug.enable_overlay_planes {
                flags.insert(FrameFlags::ALLOW_OVERLAY_PLANE_SCANOUT);
            }
            if debug.disable_direct_scanout {
                flags.remove(primary_scanout_flag);
                flags.remove(FrameFlags::ALLOW_OVERLAY_PLANE_SCANOUT);
            }
            if debug.disable_cursor_plane {
                flags.remove(FrameFlags::ALLOW_CURSOR_PLANE_SCANOUT);
            }
            if debug.skip_cursor_only_updates_during_vrr && port.request().variable_refresh {
                flags.insert(FrameFlags::SKIP_CURSOR_ONLY_UPDATES);
            }

            // Client surfaces and overlays must pass through the color pipeline.
            // A confirmed SDR signal can also carry ordinary sRGB cursor pixels.
            // HDR transitions keep the cursor composited until SDR is confirmed.
            if self.color_management_enabled
                || surface.hdr.target.is_some()
                || surface.hdr.active.is_some()
            {
                flags.remove(
                    FrameFlags::ALLOW_PRIMARY_PLANE_SCANOUT
                        | FrameFlags::ALLOW_PRIMARY_PLANE_SCANOUT_ANY
                        | FrameFlags::ALLOW_OVERLAY_PLANE_SCANOUT,
                );
                if !surface.hdr.hardware_cursor_allowed() {
                    flags.remove(FrameFlags::ALLOW_CURSOR_PLANE_SCANOUT);
                }
            }
            flags
        };

        let use_native_renderer = self.native_renderer.is_some() && surface.native_swapchain;

        if use_native_renderer {
            // === Direct Native Vulkan KMS Display Path ===
            let native = self.native_renderer.as_mut().unwrap();

            let _allocation_scope = native.async_offscreen_scope();

            let _color_scope = native.output_color_scope(Some(surface.hdr.description()));
            native.begin_scene();
            let scene = port.prepare_scene(native);
            if native.scene_was_deferred() {
                port.retry_after_vblank(target_presentation_time);
                return RenderResult::GpuDeferred;
            }
            let elements = scene.elements();
            if !native_cursor_elements_are_sdr(elements) {
                flags.remove(FrameFlags::ALLOW_CURSOR_PLANE_SCANOUT);
            }
            let drm_compositor = &mut surface.compositor;
            native.set_completion_export_required(true);
            if let (Some(tag), Some(extent)) = (
                port.request().gpu_timing_tag,
                output
                    .current_mode()
                    .map(|mode| (mode.size.w as u32, mode.size.h as u32)),
            ) {
                native.arm_output_timing(tag, extent);
            }
            let render_result = {
                let _brightness_scope = native.output_brightness_scope(surface.hdr.brightness);
                let _latency = crate::utils::latency::Span::new(
                    crate::utils::latency::Operation::VulkanRender,
                );
                drm_compositor.render_frame::<_, _>(native, elements, [0.; 4], flags)
            };
            // The flag belongs only to this KMS output frame. Capture and offscreen
            // rendering must remain in the SDR scene space.
            native.clear_output_timing();
            native.set_completion_export_required(false);
            match render_result {
                Ok(res) => {
                    // Native completion retains sampled client-buffer owners
                    // until the GPU finishes. Exportable output fences pass
                    // directly to KMS; fallback/debug paths defer submission.
                    let needs_sync = res.needs_sync()
                        || self
                            .config
                            .as_ref()
                            .debug
                            .wait_for_frame_completion_before_queueing;
                    if needs_sync {
                        if let PrimaryPlaneElement::Swapchain(element) = res.primary_element {
                            if crate::render_helpers::sync::require_ready(&element.sync).is_err() {
                                port.update_scanout(&res.states);
                                let feedback = (!res.is_empty)
                                    .then(|| port.take_presentation_feedback(&res.states));
                                surface.pending_kms = Some(DeferredFrame::new(
                                    element.sync.clone(),
                                    PendingKmsFrame {
                                        _primary: element,
                                        states: res.states,
                                        is_empty: res.is_empty,
                                        identity: KmsFrameIdentity::new(
                                            output,
                                            surface.frame_epoch,
                                            port.request().locked,
                                            surface.native_swapchain,
                                            surface.compositor.format(),
                                        ),
                                        feedback,
                                    },
                                ));
                                port.retry_after_vblank(target_presentation_time);
                                return RenderResult::GpuDeferred;
                            }
                        }
                    }
                    return submit_kms_frame(
                        surface,
                        port,
                        res.states,
                        res.is_empty,
                        target_presentation_time,
                        None,
                    );
                }
                Err(err) => {
                    if native_drm_frame_is_backpressure(&err) {
                        drm_compositor.reset_buffer_ages();
                        drop(surface.vblank_frame.take());
                        port.retry_after_vblank(target_presentation_time);
                        return RenderResult::GpuDeferred;
                    }
                    warn!(
                        "Native Vulkan render_frame failed on {}: {err:?}",
                        surface.name.connector
                    );
                    let err_str = format!("{err:?}");
                    surface.hdr.reject_transition(&err_str);
                    let is_device_lost = err_str.contains("DEVICE_LOST");
                    if is_device_lost {
                        warn!("Vulkan device lost; queuing bounded asynchronous device recovery");
                        let anchor = native.device_anchor();
                        match self
                            .vulkan_recovery
                            .request(self.primary_render_node, Some(anchor))
                        {
                            Ok(generation) => {
                                self.vulkan_recovery_generation = Some(generation);
                                self.vulkan_recovery_attempt = 1;
                                // The worker owns an anchor before owner caches are detached.
                                // Pending GPU work keeps its existing client-buffer leases.
                                self.native_renderer = None;
                            }
                            Err(_) => {
                                warn!("Vulkan recovery retirement capacity is busy; retaining failed device until capacity returns");
                                self.vulkan_recovery_retry_after =
                                    get_monotonic_time() + Duration::from_millis(500);
                            }
                        }
                    }
                    drm_compositor.reset_buffer_ages();
                    drop(surface.vblank_frame.take());
                    port.retry_after_vblank(target_presentation_time);
                    return RenderResult::Skipped;
                }
            }
        } else {
            // === GLES Fallback / Secondary GPU Path ===
            if surface.native_swapchain {
                let now = get_monotonic_time();
                if now < surface.next_gles_migration_attempt {
                    port.retry_after_vblank(surface.next_gles_migration_attempt);
                    return RenderResult::Skipped;
                }

                // DEVICE_LOST may have made Vulkan recovery impossible. Keep the
                // existing output and KMS surface, but replace its Vulkan-specific
                // swapchain before asking GLES to render into it.
                let migration_renderer = prepare_gles_device(
                    &mut self.gpu_manager,
                    &mut device.render_node,
                    &mut device.gles_classification_pending,
                    &mut device.allocator,
                    self.primary_render_node,
                    primary_allocator.as_ref(),
                )
                .and_then(|()| self.gpu_manager.single_renderer(&self.primary_render_node));
                let migration = match migration_renderer {
                    Ok(renderer) => {
                        let gles_render_formats =
                            renderer.as_ref().egl_context().dmabuf_render_formats();
                        let primary_formats = renderer.dmabuf_formats();
                        migrate_native_swapchain_to_gles(
                            surface,
                            device.allocator.clone(),
                            gles_render_formats,
                            primary_formats.clone(),
                            self.config.as_ref().debug.disable_10bit_output,
                            SwapchainMigrationNodes {
                                primary_render: self.primary_render_node,
                                surface_render: device.render_node,
                                primary_scanout: self.primary_node,
                                surface_scanout: tty_state.node,
                            },
                        )
                        .map(|()| primary_formats)
                    }
                    Err(err) => Err(anyhow!("error creating GLES renderer for recovery: {err}")),
                };

                match migration {
                    Ok(primary_formats) => {
                        if let Some(global) = self.dmabuf_global.as_ref() {
                            match DmabufFeedbackBuilder::new(
                                self.primary_render_node.dev_id(),
                                primary_formats,
                            )
                            .build()
                            {
                                Ok(feedback) => port.set_default_dmabuf_feedback(global, &feedback),
                                Err(err) => {
                                    warn!("error updating GLES default dmabuf feedback: {err}")
                                }
                            }
                        }
                        info!(
                            "migrated KMS swapchain for {} to GLES format {:?}",
                            surface.name.connector,
                            surface.compositor.format()
                        );
                    }
                    Err(err) => {
                        let now = get_monotonic_time();
                        surface.next_gles_migration_attempt = now + Duration::from_millis(500);
                        if surface
                            .last_gles_migration_warning
                            .is_none_or(|last| now.saturating_sub(last) >= Duration::from_secs(5))
                        {
                            warn!("error migrating KMS swapchain to GLES: {err}");
                            surface.last_gles_migration_warning = Some(now);
                        }
                        drop(surface.vblank_frame.take());
                        // Persistent device failure waits for bounded recovery backoff.
                        port.retry_after_vblank(surface.next_gles_migration_attempt);
                        return RenderResult::Skipped;
                    }
                }
            }

            let mut renderer = match self.gpu_manager.renderer(
                &self.primary_render_node,
                &device.render_node.unwrap_or(self.primary_render_node),
                surface.compositor.format(),
            ) {
                Ok(renderer) => renderer,
                Err(err) => {
                    warn!("error creating renderer for primary GPU: {err:?}");
                    port.retry_after_vblank(target_presentation_time);
                    return RenderResult::Skipped;
                }
            };

            // A Vulkan session can reach this path for a secondary GPU or after a
            // failed Vulkan KMS setup/recovery. In that case the GLES context was
            // never initialized during device_added().
            if let Some(gles) = renderer.as_gles_renderer() {
                Self::init_gles_renderer(&self.config, gles);
            }

            let scene = port.prepare_scene(&mut renderer);
            let elements = scene.elements();
            let drm_compositor = &mut surface.compositor;
            let render_result = {
                let _latency =
                    crate::utils::latency::Span::new(crate::utils::latency::Operation::GlesRender);
                drm_compositor.render_frame::<_, _>(&mut renderer, elements, [0.; 4], flags)
            };
            match render_result {
                Ok(res) => {
                    let needs_sync = res.needs_sync()
                        || self
                            .config
                            .as_ref()
                            .debug
                            .wait_for_frame_completion_before_queueing;
                    if needs_sync {
                        if let PrimaryPlaneElement::Swapchain(element) = res.primary_element {
                            if crate::render_helpers::sync::require_ready(&element.sync).is_err() {
                                port.update_scanout(&res.states);
                                let feedback = (!res.is_empty)
                                    .then(|| port.take_presentation_feedback(&res.states));
                                surface.pending_kms = Some(DeferredFrame::new(
                                    element.sync.clone(),
                                    PendingKmsFrame {
                                        _primary: element,
                                        states: res.states,
                                        is_empty: res.is_empty,
                                        identity: KmsFrameIdentity::new(
                                            output,
                                            surface.frame_epoch,
                                            port.request().locked,
                                            surface.native_swapchain,
                                            surface.compositor.format(),
                                        ),
                                        feedback,
                                    },
                                ));
                                port.retry_after_vblank(target_presentation_time);
                                return RenderResult::GpuDeferred;
                            }
                        }
                    }
                    return submit_kms_frame(
                        surface,
                        port,
                        res.states,
                        res.is_empty,
                        target_presentation_time,
                        None,
                    );
                }
                Err(err) => {
                    warn!("error rendering frame: {err}");
                    drm_compositor.reset_buffer_ages();
                    port.retry_after_vblank(target_presentation_time);
                }
            }
        }

        // We're not expecting a vblank right after this.
        drop(surface.vblank_frame.take());

        // Queue a timer to fire at the predicted vblank time.
        port.estimated_vblank(target_presentation_time);

        rv
    }

    pub fn change_vt(&mut self, vt: i32) {
        if let Err(err) = self.session.change_vt(vt) {
            warn!("error changing VT: {err}");
        }
    }

    pub fn suspend(&self) {
        #[cfg(feature = "dbus")]
        if let Err(err) = suspend() {
            warn!("error suspending: {err:?}");
        }
    }

    pub fn toggle_debug_tint(&mut self) {
        self.debug_tint = !self.debug_tint;

        for device in self.devices.values_mut() {
            for surface in device.surfaces.values_mut() {
                let compositor = &mut surface.compositor;

                let mut flags = compositor.debug_flags();
                flags.set(DebugFlags::TINT, self.debug_tint);
                compositor.set_debug_flags(flags);
            }
        }
    }

    pub fn import_dmabuf(&mut self, dmabuf: &Dmabuf) -> bool {
        if let Some(native) = self.native_renderer.as_mut() {
            match native.import_dmabuf(dmabuf, None) {
                Ok(_texture) => {
                    if dmabuf.node().is_none() {
                        dmabuf.set_node(Some(self.primary_render_node));
                    }
                    return true;
                }
                Err(err) => {
                    debug!("error importing dmabuf in Vulkan: {err:?}");
                    return false;
                }
            }
        }

        let mut renderer = match self.gpu_manager.single_renderer(&self.primary_render_node) {
            Ok(renderer) => renderer,
            Err(err) => {
                debug!("error creating renderer for primary GPU: {err:?}");
                return false;
            }
        };

        match renderer.import_dmabuf(dmabuf, None) {
            Ok(_texture) => {
                if dmabuf.node().is_none() {
                    dmabuf.set_node(Some(self.primary_render_node));
                }
                true
            }
            Err(err) => {
                debug!("error importing dmabuf: {err:?}");
                false
            }
        }
    }

    pub fn early_import(&mut self, surface: &WlSurface) {
        if self.native_renderer.is_some() {
            // Native Vulkan mode imports DMA-BUFs on commit/render directly via import_dmabuf
            // without GLES
            return;
        }
        if let Err(err) = self.gpu_manager.early_import(
            // We always render on the primary GPU.
            self.primary_render_node,
            surface,
        ) {
            warn!("error doing early import: {err:?}");
        }
    }

    pub fn get_gamma_size(&self, output: &Output) -> anyhow::Result<u32> {
        let tty_state = output.user_data().get::<TtyOutputState>().unwrap();
        let crtc = tty_state.crtc;

        let device = self
            .devices
            .get(&tty_state.node)
            .context("missing device")?;

        let surface = device.surfaces.get(&crtc).context("missing surface")?;
        if let Some(gamma_props) = &surface.gamma_props {
            gamma_props.gamma_size(&device.drm)
        } else {
            let info = device
                .drm
                .get_crtc(crtc)
                .context("error getting crtc info")?;
            Ok(info.gamma_length())
        }
    }

    pub fn set_gamma(&mut self, output: &Output, ramp: Option<Vec<u16>>) -> anyhow::Result<()> {
        let tty_state = output.user_data().get::<TtyOutputState>().unwrap();
        let crtc = tty_state.crtc;

        let device = self
            .devices
            .get_mut(&tty_state.node)
            .context("missing device")?;
        let surface = device.surfaces.get_mut(&crtc).context("missing surface")?;

        if ramp.is_some() && !surface.hdr.gamma_allowed() {
            bail!("gamma ramps/night-light are unavailable while HDR is active or pending; PQ requires an identity CRTC gamma LUT");
        }

        // Cannot change properties while the device is inactive.
        if !self.session.is_active() {
            surface.pending_gamma_change = Some(ramp);
            return Ok(());
        }

        let ramp = ramp.as_deref();
        if let Some(gamma_props) = &mut surface.gamma_props {
            gamma_props.set_gamma(&device.drm, ramp)
        } else {
            set_gamma_for_crtc(&device.drm, crtc, ramp)
        }
    }

    pub fn ipc_outputs(&self) -> Arc<Mutex<IpcOutputMap>> {
        self.ipc_outputs.clone()
    }

    #[cfg(feature = "xdp-gnome-screencast")]
    pub fn primary_gbm_device(&self) -> Option<GbmDevice<DrmDeviceFd>> {
        // Try to find a device corresponding to the primary render node.
        let device = self
            .devices
            .values()
            .find(|d| d.render_node == Some(self.primary_render_node));
        // Otherwise, try to get the device corresponding to the primary node.
        let device = device.or_else(|| self.devices.get(&self.primary_node));

        Some(device?.gbm.clone())
    }

    pub fn set_monitors_active(&mut self, active: bool) {
        // We only disable the CRTC here, this will also reset the
        // surface state so that the next call to `render_frame` will
        // always produce a new frame and `queue_frame` will change
        // the CRTC to active. This makes sure we always enable a CRTC
        // within an atomic operation.
        if active {
            return;
        }

        for device in self.devices.values_mut() {
            for surface in device.surfaces.values_mut() {
                surface.frame_epoch = surface.frame_epoch.wrapping_add(1);
                match surface.compositor.clear() {
                    Ok(()) => {
                        surface.hdr.resume();
                        if let Some(output) = self
                            .ipc_outputs
                            .lock()
                            .unwrap()
                            .values_mut()
                            .find(|output| output.name == surface.name.connector)
                        {
                            output.hdr_active = false;
                            output.hdr_status = surface.hdr.status.clone();
                        }
                    }
                    Err(err) => warn!("error clearing drm surface: {err:?}"),
                }
            }
        }
    }

    pub(crate) fn compute_ignored_nodes(&self) -> HashSet<DrmNode> {
        let mut ignored_nodes = ignored_nodes_from_config(self.config.as_ref());
        if ignored_nodes.remove(&self.primary_node)
            || ignored_nodes.remove(&self.primary_render_node)
        {
            warn!("ignoring the primary node or render node is not allowed");
        }
        ignored_nodes
    }

    pub(crate) fn should_disable_laptop_panels(&self, is_lid_closed: bool) -> bool {
        if !is_lid_closed {
            return false;
        }

        let config = self.config.as_ref();
        if !config.debug.keep_laptop_panel_on_when_lid_is_closed {
            // Check if any external monitor is connected.
            for device in self.devices.values() {
                for (connector, _crtc) in device.drm_scanner.crtcs() {
                    if !is_laptop_panel(&format_connector_name(connector)) {
                        return true;
                    }
                }
            }
        }

        false
    }

    pub fn get_device_from_node(&mut self, node: DrmNode) -> Option<&mut OutputDevice> {
        self.devices.get_mut(&node)
    }

    pub fn disconnected_connector_name_by_name_match(&self, target: &str) -> Option<OutputName> {
        let disable_monitor_names = self.config.as_ref().debug.disable_monitor_names;
        for device in self.devices.values() {
            for (connector, crtc) in device.drm_scanner.crtcs() {
                // Check if connected.
                if connector.state() != connector::State::Connected {
                    continue;
                }

                // Check if already enabled.
                if device.surfaces.contains_key(&crtc)
                    || device
                        .non_desktop_connectors
                        .contains(&(connector.handle(), crtc))
                {
                    continue;
                }

                let output_name = device.known_crtc_name(&crtc, connector, disable_monitor_names);
                if output_name.matches(target) {
                    return Some(output_name);
                }
            }
        }

        None
    }
}

impl GammaProps {
    pub(crate) fn new(device: &DrmDevice, crtc: crtc::Handle) -> anyhow::Result<Self> {
        let mut gamma_lut = None;
        let mut gamma_lut_size = None;

        let props = device
            .get_properties(crtc)
            .context("error getting properties")?;
        for (prop, _) in props {
            let Ok(info) = device.get_property(prop) else {
                continue;
            };

            let Ok(name) = info.name().to_str() else {
                continue;
            };

            match name {
                "GAMMA_LUT" => {
                    ensure!(
                        matches!(info.value_type(), property::ValueType::Blob),
                        "wrong GAMMA_LUT value type"
                    );
                    gamma_lut = Some(prop);
                }
                "GAMMA_LUT_SIZE" => {
                    ensure!(
                        matches!(info.value_type(), property::ValueType::UnsignedRange(_, _)),
                        "wrong GAMMA_LUT_SIZE value type"
                    );
                    gamma_lut_size = Some(prop);
                }
                _ => (),
            }
        }

        let gamma_lut = gamma_lut.context("missing GAMMA_LUT property")?;
        let gamma_lut_size = gamma_lut_size.context("missing GAMMA_LUT_SIZE property")?;

        Ok(Self {
            crtc,
            gamma_lut,
            gamma_lut_size,
            previous_blob: None,
        })
    }

    pub(crate) fn gamma_size(&self, device: &DrmDevice) -> anyhow::Result<u32> {
        let value = get_drm_property(device, self.crtc, self.gamma_lut_size)
            .context("missing GAMMA_LUT_SIZE property")?;
        Ok(value as u32)
    }

    pub(crate) fn set_gamma(
        &mut self,
        device: &DrmDevice,
        gamma: Option<&[u16]>,
    ) -> anyhow::Result<()> {
        let _span = clarion_tracy_span!("GammaProps::set_gamma");

        let blob = if let Some(gamma) = gamma {
            let gamma_size = self
                .gamma_size(device)
                .context("error getting gamma size")? as usize;

            ensure!(gamma.len() == gamma_size * 3, "wrong gamma length");

            #[allow(non_camel_case_types)]
            #[repr(C)]
            #[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
            pub struct drm_color_lut {
                pub red: u16,
                pub green: u16,
                pub blue: u16,
                pub reserved: u16,
            }

            let (red, rest) = gamma.split_at(gamma_size);
            let (blue, green) = rest.split_at(gamma_size);
            let mut data = zip(zip(red, blue), green)
                .map(|((&red, &green), &blue)| drm_color_lut {
                    red,
                    green,
                    blue,
                    reserved: 0,
                })
                .collect::<Vec<_>>();
            let data = cast_slice_mut(&mut data);

            let blob = drm_ffi::mode::create_property_blob(device.as_fd(), data)
                .context("error creating property blob")?;
            NonZeroU64::new(u64::from(blob.blob_id))
        } else {
            None
        };

        {
            let _span = clarion_tracy_span!("set_property");

            let blob = blob.map(NonZeroU64::get).unwrap_or(0);
            device
                .set_property(
                    self.crtc,
                    self.gamma_lut,
                    property::Value::Blob(blob).into(),
                )
                .context("error setting GAMMA_LUT")
                .inspect_err(|_| {
                    if blob != 0 {
                        // Destroy the blob we just allocated.
                        if let Err(err) = device.destroy_property_blob(blob) {
                            warn!("error destroying GAMMA_LUT property blob: {err:?}");
                        }
                    }
                })?;
        }

        if let Some(blob) = mem::replace(&mut self.previous_blob, blob) {
            if let Err(err) = device.destroy_property_blob(blob.get()) {
                warn!("error destroying previous GAMMA_LUT blob: {err:?}");
            }
        }

        Ok(())
    }

    pub(crate) fn restore_gamma(&self, device: &DrmDevice) -> anyhow::Result<()> {
        let _span = clarion_tracy_span!("GammaProps::restore_gamma");

        let blob = self.previous_blob.map(NonZeroU64::get).unwrap_or(0);
        device
            .set_property(
                self.crtc,
                self.gamma_lut,
                property::Value::Blob(blob).into(),
            )
            .context("error setting GAMMA_LUT")?;

        Ok(())
    }
}

pub(crate) fn primary_node_from_render_node(path: &Path) -> Option<(DrmNode, DrmNode)> {
    match DrmNode::from_path(path) {
        Ok(node) => {
            if node.ty() == NodeType::Render {
                match node.node_with_type(NodeType::Primary) {
                    Some(Ok(primary_node)) => {
                        return Some((primary_node, node));
                    }
                    Some(Err(err)) => {
                        warn!("error opening primary node for render node {path:?}: {err:?}");
                    }
                    None => {
                        warn!("error opening primary node for render node {path:?}");
                    }
                }
            } else {
                warn!("DRM node {path:?} is not a render node");

                // Gracefully handle misconfiguration on regular desktop systems.
                if let Some(Ok(render_node)) = node.node_with_type(NodeType::Render) {
                    return Some((node, render_node));
                }

                warn!("could not get render node for DRM node {path:?}; proceeding anyway");
                return Some((node, node));
            }
        }
        Err(err) => {
            warn!("error opening {path:?} as DRM node: {err:?}");
        }
    }

    None
}

pub(crate) fn primary_node_from_config(
    config: &super::DeviceSettings,
) -> Option<(DrmNode, DrmNode)> {
    let path = config.debug.render_drm_device.as_ref()?;
    debug!("attempting to use render node from config: {path:?}");

    primary_node_from_render_node(path)
}

pub(crate) fn ignored_nodes_from_config(config: &super::DeviceSettings) -> HashSet<DrmNode> {
    let mut disabled_nodes = HashSet::new();

    for path in &config.debug.ignored_drm_devices {
        if let Some((primary_node, render_node)) = primary_node_from_render_node(path) {
            disabled_nodes.insert(primary_node);
            disabled_nodes.insert(render_node);
        }
    }

    disabled_nodes
}

pub(crate) fn surface_dmabuf_feedback(
    compositor: &GbmDrmCompositor,
    primary_formats: FormatSet,
    primary_render_node: DrmNode,
    surface_render_node: Option<DrmNode>,
    primary_scanout_node: DrmNode,
    surface_scanout_node: DrmNode,
) -> Result<SurfaceDmabufFeedback, io::Error> {
    let surface = compositor.surface();
    let planes = surface.planes();

    let primary_plane_formats = surface.plane_info().formats.clone();
    let primary_or_overlay_plane_formats = primary_plane_formats
        .iter()
        .chain(planes.overlay.iter().flat_map(|p| p.formats.iter()))
        .copied()
        .collect::<FormatSet>();

    // We limit the scan-out trache to formats we can also render from so that there is always a
    // fallback render path available in case the supplied buffer can not be scanned out directly.
    let mut primary_scanout_formats = primary_plane_formats
        .intersection(&primary_formats)
        .copied()
        .collect::<Vec<_>>();
    let mut primary_or_overlay_scanout_formats = primary_or_overlay_plane_formats
        .intersection(&primary_formats)
        .copied()
        .collect::<Vec<_>>();

    // HACK: AMD iGPU + dGPU systems share some modifiers between the two, and yet cross-device
    // buffers produce a glitched scanout if the modifier is not Linear...
    //
    // Also limit scan-out formats to Linear if we have a device without a render node (i.e.
    // we're rendering on a different device).
    if surface_render_node != Some(primary_render_node)
        || surface_scanout_node != primary_scanout_node
    {
        primary_scanout_formats.retain(|f| f.modifier == Modifier::Linear);
        primary_or_overlay_scanout_formats.retain(|f| f.modifier == Modifier::Linear);
    }

    let builder = DmabufFeedbackBuilder::new(primary_render_node.dev_id(), primary_formats);

    trace!(
        "primary scanout formats: {}, overlay adds: {}",
        primary_scanout_formats.len(),
        primary_or_overlay_scanout_formats.len() - primary_scanout_formats.len(),
    );

    // Prefer the primary-plane-only formats, then primary-or-overlay-plane formats. This will
    // increase the chance of scanning out a client even with our disabled-by-default overlay
    // planes.
    let scanout = builder
        .clone()
        .add_preference_tranche(
            surface_scanout_node.dev_id(),
            TrancheFlags::Scanout,
            primary_scanout_formats,
            4..=6,
        )
        .add_preference_tranche(
            surface_scanout_node.dev_id(),
            TrancheFlags::Scanout,
            primary_or_overlay_scanout_formats,
            4..=6,
        )
        .build()?;

    // If this is the primary node surface, send scanout formats in both tranches to avoid
    // duplication.
    let render = if surface_render_node == Some(primary_render_node) {
        scanout.clone()
    } else {
        builder.build()?
    };

    Ok(SurfaceDmabufFeedback { render, scanout })
}

pub(crate) struct SwapchainMigrationNodes {
    pub(crate) primary_render: DrmNode,
    pub(crate) surface_render: Option<DrmNode>,
    pub(crate) primary_scanout: DrmNode,
    pub(crate) surface_scanout: DrmNode,
}

pub(crate) fn migrate_native_swapchain_to_gles(
    surface: &mut Surface,
    allocator: GbmAllocator<DrmDeviceFd>,
    gles_render_formats: &FormatSet,
    primary_formats: FormatSet,
    disable_10bit_output: bool,
    nodes: SwapchainMigrationNodes,
) -> anyhow::Result<()> {
    let mut last_error = None;
    for (code, modifiers) in gles_swapchain_candidates(
        gles_render_formats,
        nodes.surface_render.is_none(),
        disable_10bit_output,
    ) {
        match surface
            .compositor
            .set_format(allocator.clone(), code, modifiers)
        {
            Ok(()) => {
                // The new swapchain has fresh buffers; force a complete frame on it.
                surface.compositor.reset_buffer_ages();
                surface.frame_epoch = surface.frame_epoch.wrapping_add(1);
                if let Err(err) = surface.compositor.reset_state() {
                    warn!("error resetting KMS state after GLES swapchain migration: {err}");
                }
                surface.dmabuf_feedback = surface_dmabuf_feedback(
                    &surface.compositor,
                    primary_formats,
                    nodes.primary_render,
                    nodes.surface_render,
                    nodes.primary_scanout,
                    nodes.surface_scanout,
                )
                .map_err(|err| warn!("error updating GLES dmabuf feedback: {err}"))
                .ok();
                surface.native_swapchain = false;
                return Ok(());
            }
            Err(err) => last_error = Some(err.to_string()),
        }
    }

    bail!(
        "no GLES-renderable KMS swapchain format for {}: {}",
        surface.name.connector,
        last_error.unwrap_or_else(|| "no matching EGL formats".into()),
    )
}

pub(crate) fn find_drm_property(
    drm: &DrmDevice,
    resource: impl ResourceHandle,
    name: &str,
) -> Option<(property::Handle, property::Info, property::RawValue)> {
    let props = match drm.get_properties(resource) {
        Ok(props) => props,
        Err(err) => {
            warn!("error getting properties: {err:?}");
            return None;
        }
    };

    props.into_iter().find_map(|(handle, value)| {
        let info = drm.get_property(handle).ok()?;
        let n = info.name().to_str().ok()?;

        (n == name).then_some((handle, info, value))
    })
}

pub(crate) fn get_drm_property(
    drm: &DrmDevice,
    resource: impl ResourceHandle,
    prop: property::Handle,
) -> Option<property::RawValue> {
    let props = match drm.get_properties(resource) {
        Ok(props) => props,
        Err(err) => {
            warn!("error getting properties: {err:?}");
            return None;
        }
    };

    props
        .into_iter()
        .find_map(|(handle, value)| (handle == prop).then_some(value))
}

pub(crate) fn refresh_interval(mode: DrmMode) -> Duration {
    let clock = mode.clock() as u64;
    let htotal = mode.hsync().2 as u64;
    let vtotal = mode.vsync().2 as u64;

    if clock == 0 || htotal == 0 || vtotal == 0 {
        warn!(
            ?mode,
            "DRM mode has invalid zero timing; using a 60 Hz scheduling fallback"
        );
        return Duration::from_micros(16_667);
    }

    let mut numerator = htotal.saturating_mul(vtotal).saturating_mul(1_000_000);
    let mut denominator = clock;

    if mode.flags().contains(ModeFlags::INTERLACE) {
        denominator = denominator.saturating_mul(2);
    }

    if mode.flags().contains(ModeFlags::DBLSCAN) {
        numerator = numerator.saturating_mul(2);
    }

    if mode.vscan() > 1 {
        numerator = numerator.saturating_mul(mode.vscan() as u64);
    }

    let refresh_interval = numerator.saturating_add(denominator / 2) / denominator;
    Duration::from_nanos(refresh_interval)
}

#[cfg(feature = "dbus")]
pub(crate) fn suspend() -> anyhow::Result<()> {
    let conn = zbus::blocking::Connection::system().context("error connecting to system bus")?;

    conn.call_method(
        Some("org.freedesktop.login1"),
        "/org/freedesktop/login1",
        Some("org.freedesktop.login1.Manager"),
        "Suspend",
        &(true),
    )
    .context("error suspending")?;

    Ok(())
}

#[cfg(test)]
use crate::frame_pipeline::estimated_vblank_delay;
#[cfg(test)]
pub(crate) use crate::frame_pipeline::queue_kms_submission_retry;
pub(crate) use crate::frame_pipeline::{
    queue_retry_after_estimated_vblank, reset_kms_submission_backoff,
};

pub fn calculate_drm_mode_from_modeline(modeline: &Modeline) -> anyhow::Result<DrmMode> {
    ensure!(
        modeline.clock.is_finite() && modeline.clock > 0.0,
        "clock {} must be finite and > 0",
        modeline.clock
    );
    ensure!(
        modeline.hdisplay < modeline.hsync_start,
        "hdisplay {} must be < hsync_start {}",
        modeline.hdisplay,
        modeline.hsync_start
    );
    ensure!(
        modeline.hsync_start < modeline.hsync_end,
        "hsync_start {} must be < hsync_end {}",
        modeline.hsync_start,
        modeline.hsync_end
    );
    ensure!(
        modeline.hsync_end < modeline.htotal,
        "hsync_end {} must be < htotal {}",
        modeline.hsync_end,
        modeline.htotal
    );
    ensure!(
        modeline.vdisplay < modeline.vsync_start,
        "vdisplay {} must be < vsync_start {}",
        modeline.vdisplay,
        modeline.vsync_start
    );
    ensure!(
        modeline.vsync_start < modeline.vsync_end,
        "vsync_start {} must be < vsync_end {}",
        modeline.vsync_start,
        modeline.vsync_end
    );
    ensure!(
        modeline.vsync_end < modeline.vtotal,
        "vsync_end {} must be < vtotal {}",
        modeline.vsync_end,
        modeline.vtotal
    );

    let pixel_clock_kilo_hertz = modeline.clock * 1000.0;
    let rounded_pixel_clock = pixel_clock_kilo_hertz.round();
    ensure!(
        (1.0..=u32::MAX as f64).contains(&rounded_pixel_clock),
        "clock {} MHz is outside the DRM pixel-clock range",
        modeline.clock
    );
    // Calculated as documented in the CVT 1.2 standard:
    // https://app.box.com/s/vcocw3z73ta09txiskj7cnk6289j356b/file/93518784646
    let vrefresh_hertz = (pixel_clock_kilo_hertz * 1000.0)
        / (modeline.htotal as u64 * modeline.vtotal as u64) as f64;
    ensure!(
        vrefresh_hertz.is_finite(),
        "calculated refresh rate is not finite"
    );
    let vrefresh_rounded = vrefresh_hertz.round() as u32;

    let flags = match modeline.hsync_polarity {
        HSyncPolarity::PHSync => ModeFlags::PHSYNC,
        HSyncPolarity::NHSync => ModeFlags::NHSYNC,
    } | match modeline.vsync_polarity {
        VSyncPolarity::PVSync => ModeFlags::PVSYNC,
        VSyncPolarity::NVSync => ModeFlags::NVSYNC,
    };

    let mode_name = format!(
        "{}x{}@{:.2}",
        modeline.hdisplay, modeline.vdisplay, vrefresh_hertz
    );
    let name = modeinfo_name_slice_from_string(&mode_name);

    // https://www.kernel.org/doc/html/v6.17/gpu/drm-uapi.html#c.drm_mode_modeinfo
    Ok(DrmMode::from(drm_mode_modeinfo {
        clock: rounded_pixel_clock as u32,
        hdisplay: modeline.hdisplay,
        hsync_start: modeline.hsync_start,
        hsync_end: modeline.hsync_end,
        htotal: modeline.htotal,
        vdisplay: modeline.vdisplay,
        vsync_start: modeline.vsync_start,
        vsync_end: modeline.vsync_end,
        vtotal: modeline.vtotal,
        vrefresh: vrefresh_rounded,
        flags: flags.bits(),
        name,
        // Defaults
        type_: drm_ffi::DRM_MODE_TYPE_USERDEF,
        hskew: 0,
        vscan: 0,
    }))
}

pub fn calculate_mode_cvt(width: u16, height: u16, refresh: f64) -> anyhow::Result<DrmMode> {
    // Cross-checked with sway's implementation:
    // https://gitlab.freedesktop.org/wlroots/wlroots/-/blob/22528542970687720556035790212df8d9bb30bb/backend/drm/util.c#L251
    ensure!(width > 0 && height > 0, "custom mode has zero dimensions");
    ensure!(
        refresh.is_finite() && refresh > 0.,
        "custom mode has invalid refresh rate"
    );

    let options = libdisplay_info::cvt::Options {
        red_blank_ver: libdisplay_info::cvt::ReducedBlankingVersion::None,
        h_pixels: width as i32,
        v_lines: height as i32,
        ip_freq_rqd: refresh,

        // Defaults
        video_opt: false,
        vblank: 0f64,
        additional_hblank: 0,
        early_vsync_rqd: false,
        int_rqd: false,
        margins_rqd: false,
    };
    let cvt_timing = libdisplay_info::cvt::Timing::compute(options);

    let timing_component = |value: f64| -> anyhow::Result<u16> {
        ensure!(
            value.is_finite() && (0. ..=f64::from(u16::MAX)).contains(&value),
            "custom mode CVT timing is outside the DRM range"
        );
        Ok(value as u16)
    };
    let horizontal_overflow = || anyhow!("custom mode horizontal timing exceeds DRM limit");
    let vertical_overflow = || anyhow!("custom mode vertical timing exceeds DRM limit");

    let hsync_start = width
        .checked_add(timing_component(cvt_timing.h_front_porch)?)
        .ok_or_else(horizontal_overflow)?;
    let hsync_end = hsync_start
        .checked_add(timing_component(cvt_timing.h_sync)?)
        .ok_or_else(horizontal_overflow)?;
    let htotal = hsync_end
        .checked_add(timing_component(cvt_timing.h_back_porch)?)
        .ok_or_else(horizontal_overflow)?;

    let vsync_start = timing_component(cvt_timing.v_lines_rnd)?
        .checked_add(timing_component(cvt_timing.v_front_porch)?)
        .ok_or_else(vertical_overflow)?;
    let vsync_end = vsync_start
        .checked_add(timing_component(cvt_timing.v_sync)?)
        .ok_or_else(vertical_overflow)?;
    let vtotal = vsync_end
        .checked_add(timing_component(cvt_timing.v_back_porch)?)
        .ok_or_else(vertical_overflow)?;

    let clock = (cvt_timing.act_pixel_freq * 1000.).round();
    ensure!(
        clock.is_finite() && (1. ..=f64::from(u32::MAX)).contains(&clock),
        "custom mode pixel clock is outside the DRM range"
    );
    let vrefresh = cvt_timing.act_frame_rate.round();
    ensure!(
        vrefresh.is_finite() && (1. ..=f64::from(u32::MAX)).contains(&vrefresh),
        "custom mode refresh rate is outside the DRM range"
    );

    let flags = drm_ffi::DRM_MODE_FLAG_NHSYNC | drm_ffi::DRM_MODE_FLAG_PVSYNC;

    let mode_name = format!("{width}x{height}@{:.2}", cvt_timing.act_frame_rate);
    let name = modeinfo_name_slice_from_string(&mode_name);

    let drm_ffi_mode = drm_ffi::drm_sys::drm_mode_modeinfo {
        clock: clock as u32,

        hdisplay: width,
        hsync_start,
        hsync_end,
        htotal,

        vdisplay: height,
        vsync_start,
        vsync_end,
        vtotal,

        vrefresh: vrefresh as u32,

        flags,
        type_: drm_ffi::DRM_MODE_TYPE_USERDEF,
        name,

        // Defaults
        hskew: 0,
        vscan: 0,
    };

    Ok(DrmMode::from(drm_ffi_mode))
}

// Returns a c-string of maximally 31 Rust string chars + null terminator. Excess characters are
// dropped.
pub(crate) fn modeinfo_name_slice_from_string(mode_name: &str) -> [core::ffi::c_char; 32] {
    let mut name: [core::ffi::c_char; 32] = [0; 32];

    for (a, b) in zip(&mut name[..31], mode_name.as_bytes()) {
        // Can be u8 on aarch64 and i8 on x86_64.
        *a = *b as _;
    }

    name
}

pub(crate) fn pick_mode(
    connector: &connector::Info,
    target: Option<clarion_config::output::Mode>,
) -> Option<(control::Mode, bool)> {
    let mut mode = None;
    let mut fallback = false;

    if let Some(target) = target {
        let target_mode = target.mode;

        if target.custom {
            if let Some(refresh) = target_mode.refresh {
                match calculate_mode_cvt(target_mode.width, target_mode.height, refresh) {
                    Ok(custom_mode) => return Some((custom_mode, false)),
                    Err(err) => {
                        warn!("invalid custom CVT mode; falling back to advertised modes: {err:?}")
                    }
                }
            } else {
                warn!("custom mode refresh unspecified; trying advertised modes");
            }
        }

        let refresh = target_mode.refresh.map(|r| (r * 1000.).round() as i32);
        for m in connector.modes() {
            if m.size() != (target.mode.width, target.mode.height) {
                continue;
            }

            // Interlaced modes don't appear to work.
            if m.flags().contains(ModeFlags::INTERLACE) {
                continue;
            }

            if let Some(refresh) = refresh {
                // If refresh is set, only pick modes with matching refresh.
                let wl_mode = Mode::from(*m);
                if wl_mode.refresh == refresh {
                    mode = Some(m);
                }
            } else if let Some(curr) = mode {
                // If refresh isn't set, pick the mode with the highest refresh.
                if curr.vrefresh() < m.vrefresh() {
                    mode = Some(m);
                }
            } else {
                mode = Some(m);
            }
        }

        if mode.is_none() {
            fallback = true;
        }
    }

    if mode.is_none() {
        // Pick a preferred mode.
        for m in connector.modes() {
            if !m.mode_type().contains(ModeTypeFlags::PREFERRED) {
                continue;
            }

            if let Some(curr) = mode {
                if curr.vrefresh() < m.vrefresh() {
                    mode = Some(m);
                }
            } else {
                mode = Some(m);
            }
        }
    }

    if mode.is_none() {
        // Last attempt.
        mode = connector.modes().first();
    }

    mode.map(|m| (*m, fallback))
}

pub(crate) fn get_edid_info(
    device: &DrmDevice,
    connector: connector::Handle,
) -> anyhow::Result<libdisplay_info::info::Info> {
    let (_, info, value) =
        find_drm_property(device, connector, "EDID").context("no EDID property")?;
    let blob = info
        .value_type()
        .convert_value(value)
        .as_blob()
        .context("EDID was not blob type")?;
    let data = device
        .get_property_blob(blob)
        .context("error getting EDID blob value")?;
    libdisplay_info::info::Info::parse_edid(&data).context("error parsing EDID")
}

impl<'a> ConnectorProperties<'a> {
    pub(crate) fn try_new(
        device: &'a DrmDevice,
        connector: connector::Handle,
    ) -> anyhow::Result<Self> {
        let prop_vals = device
            .get_properties(connector)
            .context("error getting properties")?;

        let mut properties = Vec::new();

        for (prop, value) in prop_vals {
            let info = device
                .get_property(prop)
                .context("error getting property")?;

            properties.push((info, value));
        }

        Ok(Self {
            device,
            connector,
            properties,
            has_change: false,
            requests: AtomicModeReq::new(),
        })
    }

    pub(crate) fn find(
        &self,
        name: &std::ffi::CStr,
    ) -> anyhow::Result<&(property::Info, property::RawValue)> {
        for prop in &self.properties {
            if prop.0.name() == name {
                return Ok(prop);
            }
        }

        Err(anyhow!("couldn't find property: {name:?}"))
    }

    pub(crate) fn get_panel_orientation(&self) -> anyhow::Result<Transform> {
        let (info, value) = self.find(c"panel orientation")?;
        match info.value_type().convert_value(*value) {
            property::Value::Enum(Some(val)) => match val.value() {
                // "Normal"
                0 => Ok(Transform::Normal),
                // "Upside Down"
                1 => Ok(Transform::_180),
                // "Left Side Up"
                2 => Ok(Transform::_90),
                // "Right Side Up"
                3 => Ok(Transform::_270),
                _ => bail!("panel orientation has invalid value: {:?}", val),
            },
            _ => bail!("panel orientation has wrong value type"),
        }
    }

    /// Sets the connector's DPMS state. The enum values are driver-defined, so
    /// the wanted state is matched by name instead of by index.
    pub(crate) fn set_dpms(&mut self, on: bool) -> anyhow::Result<()> {
        let (info, value) = self.find(c"DPMS")?;
        let value_type = info.value_type();
        let property::ValueType::Enum(values) = &value_type else {
            bail!("wrong DPMS property type")
        };
        let property::Value::Enum(current) = value_type.convert_value(*value) else {
            bail!("wrong DPMS property type")
        };
        let want = if on { c"On" } else { c"Off" };
        let (raws, enums) = values.values();
        let index = enums
            .iter()
            .position(|entry| entry.name() == want)
            .context("DPMS property has no On/Off value")?;
        if current.map(|entry| entry.value()) != Some(raws[index]) {
            self.requests.add_raw_property(
                self.connector.into(),
                info.handle(),
                property::Value::Enum(Some(&enums[index])).into(),
            );
            self.has_change = true;
        }
        Ok(())
    }

    pub(crate) fn commit(&mut self) -> anyhow::Result<()> {
        if self.has_change {
            self.device.atomic_commit(
                AtomicCommitFlags::ALLOW_MODESET,
                std::mem::take(&mut self.requests),
            )?;
        }

        Ok(())
    }
}

pub(crate) fn is_vrr_capable(device: &DrmDevice, connector: connector::Handle) -> Option<bool> {
    let (_, info, value) = find_drm_property(device, connector, "vrr_capable")?;
    info.value_type().convert_value(value).as_boolean()
}

pub fn set_gamma_for_crtc(
    device: &DrmDevice,
    crtc: crtc::Handle,
    ramp: Option<&[u16]>,
) -> anyhow::Result<()> {
    let _span = clarion_tracy_span!("set_gamma_for_crtc");

    let info = device.get_crtc(crtc).context("error getting crtc info")?;
    let gamma_length = info.gamma_length() as usize;

    ensure!(gamma_length != 0, "setting gamma is not supported");

    let mut temp;
    let ramp = if let Some(ramp) = ramp {
        ensure!(ramp.len() == gamma_length * 3, "wrong gamma length");
        ramp
    } else {
        let _span = clarion_tracy_span!("generate linear gamma");

        // The legacy API provides no way to reset the gamma, so set a linear one manually.
        ensure!(
            gamma_length > 1,
            "legacy gamma table cannot represent an identity ramp"
        );
        temp = vec![0u16; gamma_length * 3];

        let (red, rest) = temp.split_at_mut(gamma_length);
        let (green, blue) = rest.split_at_mut(gamma_length);
        let denom = gamma_length as u64 - 1;
        for (i, ((r, g), b)) in zip(zip(red, green), blue).enumerate() {
            let value = (0xFFFFu64 * i as u64 / denom) as u16;
            *r = value;
            *g = value;
            *b = value;
        }

        &temp
    };

    let (red, ramp) = ramp.split_at(gamma_length);
    let (green, blue) = ramp.split_at(gamma_length);

    device
        .set_gamma(crtc, red, green, blue)
        .context("error setting gamma")?;

    Ok(())
}

pub(crate) fn format_connector_name(connector: &connector::Info) -> String {
    format!(
        "{}-{}",
        connector.interface().as_str(),
        connector.interface_id(),
    )
}

pub(crate) fn make_output_name(
    device: &DrmDevice,
    connector: connector::Handle,
    connector_name: String,
) -> OutputName {
    let info = get_edid_info(device, connector)
        .map_err(|err| warn!("error getting EDID info for {connector_name}: {err:?}"))
        .ok();
    OutputName {
        connector: connector_name,
        make: info.as_ref().and_then(|info| info.make()),
        model: info.as_ref().and_then(|info| info.model()),
        serial: info.as_ref().and_then(|info| info.serial()),
    }
}

/// Initializes the libinput plugin system.
///
/// # Safety
///
/// This function must be called before libinput iterates through the devices, i.e. before
/// libinput_udev_assign_seat() or the first call to libinput_path_add_device().
unsafe fn init_libinput_plugin_system(libinput: &Libinput) {
    #[cfg(have_libinput_plugin_system)]
    unsafe {
        use std::ffi::{c_char, c_int, CString};
        use std::os::unix::ffi::OsStringExt;

        use directories::BaseDirs;
        use input::ffi::libinput;
        use input::AsRaw as _;

        extern "C" {
            fn libinput_plugin_system_append_path(libinput: *const libinput, path: *const c_char);
            fn libinput_plugin_system_append_default_paths(libinput: *const libinput);
            fn libinput_plugin_system_load_plugins(
                libinput: *const libinput,
                flags: c_int,
            ) -> c_int;
        }
        const LIBINPUT_PLUGIN_SYSTEM_FLAG_NONE: c_int = 0;
        let libinput = libinput.as_raw();

        // Also load plugins from $XDG_CONFIG_HOME/libinput/plugins.
        if let Some(dirs) = BaseDirs::new() {
            let mut plugins_dir = dirs.config_dir().to_path_buf();
            plugins_dir.push("libinput");
            plugins_dir.push("plugins");
            if let Ok(plugins_dir) = CString::new(plugins_dir.into_os_string().into_vec()) {
                libinput_plugin_system_append_path(libinput, plugins_dir.as_ptr());
            }
        }

        libinput_plugin_system_append_default_paths(libinput);
        libinput_plugin_system_load_plugins(libinput, LIBINPUT_PLUGIN_SYSTEM_FLAG_NONE);
    }
    #[cfg(not(have_libinput_plugin_system))]
    let _ = libinput;
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use clarion_config::output::Modeline;
    use clarion_ipc::{HSyncPolarity, VSyncPolarity};
    use insta::assert_debug_snapshot;
    use smithay::backend::allocator::format::FormatSet;
    use smithay::backend::allocator::{Format, Fourcc, Modifier};
    use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
    use smithay::utils::Transform;

    use crate::backend::tty::{
        calculate_drm_mode_from_modeline, calculate_mode_cvt, estimated_vblank_delay,
        gles_swapchain_candidates, output_uses_native_renderer, surface_uses_native_renderer,
        vulkan_scanout_formats, KmsFrameIdentity,
    };
    use crate::frame_clock::FrameClock;

    #[test]
    fn hardware_cursor_rejects_hdr_scrgb_wide_gamut_and_extended_sdr_sources() {
        use ash::vk::Format;

        use crate::color::{ColorDescription, Primaries};

        let packed = Format::B8G8R8A8_UNORM.as_raw() as u32;
        assert!(super::cursor_color_is_sdr(ColorDescription::srgb(), packed));
        assert!(super::cursor_color_is_sdr(
            ColorDescription::srgb(),
            Format::R8G8B8A8_UNORM.as_raw() as u32
        ));
        assert!(!super::cursor_color_is_sdr(
            ColorDescription::bt2020_pq(),
            packed
        ));
        assert!(!super::cursor_color_is_sdr(
            ColorDescription::windows_scrgb(),
            packed
        ));
        assert!(!super::cursor_color_is_sdr(
            ColorDescription {
                primaries: Primaries::DisplayP3,
                ..ColorDescription::srgb()
            },
            packed
        ));
        assert!(!super::cursor_color_is_sdr(
            ColorDescription {
                max_luminance: 1000.,
                ..ColorDescription::srgb()
            },
            packed
        ));
        assert!(
            !super::cursor_color_is_sdr(
                ColorDescription::srgb(),
                Format::R16G16B16A16_SFLOAT.as_raw() as u32
            ),
            "default sRGB metadata does not bound floating-point cursor pixels"
        );
    }

    #[test]
    fn drm_native_busy_survives_transparent_wrappers_without_rejecting_hard_errors() {
        use smithay::backend::drm::compositor::{FrameError, RenderFrameError};
        use smithay::backend::renderer::damage::Error as DamageError;

        use crate::render_helpers::vulkan::native::{NativeBusy, NativeError};

        type TestError =
            RenderFrameError<std::io::Error, std::io::Error, std::io::Error, NativeError>;
        for kind in [
            NativeBusy::Frame,
            NativeBusy::Upload,
            NativeBusy::Completion,
            NativeBusy::OutputFormat,
            NativeBusy::YuvPipelines,
            NativeBusy::ProducerFence,
            NativeBusy::OffscreenAllocation,
        ] {
            let error = TestError::RenderFrame(DamageError::Rendering(NativeError::Busy(kind)));
            assert!(std::error::Error::source(&error).is_none());
            assert!(super::native_drm_frame_is_backpressure(&error));
        }
        let hard = TestError::RenderFrame(DamageError::Rendering(NativeError::Other(
            "actual GPU preparation failure".into(),
        )));
        assert!(!super::native_drm_frame_is_backpressure(&hard));
        let prepare = TestError::PrepareFrame(FrameError::NoSupportedPlaneFormat);
        assert!(!super::native_drm_frame_is_backpressure(&prepare));
    }

    fn kms_identity_output() -> Output {
        let output = Output::new(
            "kms-identity".into(),
            PhysicalProperties {
                size: (300, 200).into(),
                subpixel: Subpixel::Unknown,
                make: String::new(),
                model: String::new(),
                serial_number: String::new(),
            },
        );
        output.change_current_state(
            Some(Mode {
                size: (1920, 1080).into(),
                refresh: 60_000,
            }),
            None,
            None,
            None,
        );
        output
    }

    #[test]
    fn retained_kms_identity_rejects_lock_epoch_renderer_and_format_changes() {
        let output = kms_identity_output();
        let captured = KmsFrameIdentity::new(&output, 7, false, true, Fourcc::Argb8888);
        assert!(captured == KmsFrameIdentity::new(&output, 7, false, true, Fourcc::Argb8888));
        assert!(captured != KmsFrameIdentity::new(&output, 7, true, true, Fourcc::Argb8888));
        assert!(captured != KmsFrameIdentity::new(&output, 8, false, true, Fourcc::Argb8888));
        assert!(captured != KmsFrameIdentity::new(&output, 7, false, false, Fourcc::Argb8888));
        assert!(captured != KmsFrameIdentity::new(&output, 7, false, true, Fourcc::Abgr8888));
    }

    #[test]
    fn retained_kms_identity_reads_current_mode_scale_and_transform_after_readiness() {
        let output = kms_identity_output();
        let capture = || KmsFrameIdentity::new(&output, 7, false, true, Fourcc::Argb8888);
        let before_mode = capture();
        output.change_current_state(
            Some(Mode {
                size: (2560, 1440).into(),
                refresh: 60_000,
            }),
            None,
            None,
            None,
        );
        let before_scale = capture();
        assert!(before_mode != before_scale);
        output.change_current_state(None, None, Some(Scale::Fractional(1.5)), None);
        let before_transform = capture();
        assert!(before_scale != before_transform);
        output.change_current_state(None, Some(Transform::_90), None, None);
        assert!(before_transform != capture());
    }

    #[test]
    fn estimated_vblank_timer_stays_on_fixed_refresh_grid_after_a_late_frame() {
        let last = Duration::from_secs(1);
        let interval = Duration::from_millis(10);
        let mut clock = FrameClock::new(Some(interval), false);
        clock.presented(last);
        let target = last + interval;

        assert_eq!(
            estimated_vblank_delay(target, last + Duration::from_millis(12), &clock),
            Duration::from_millis(8)
        );
        assert_eq!(
            estimated_vblank_delay(target, last + Duration::from_millis(37), &clock),
            Duration::from_millis(3)
        );
        assert_eq!(estimated_vblank_delay(target, target, &clock), interval);
    }

    #[test]
    fn estimated_vblank_timer_preserves_future_retries_and_unknown_timing() {
        let now = Duration::from_secs(1);
        let fixed = FrameClock::new(Some(Duration::from_millis(10)), false);
        assert_eq!(
            estimated_vblank_delay(now + Duration::from_millis(500), now, &fixed),
            Duration::from_millis(500)
        );
        assert_eq!(
            estimated_vblank_delay(now, now, &fixed),
            Duration::from_millis(10)
        );
        assert_eq!(
            estimated_vblank_delay(now, now, &FrameClock::new(None, false)),
            Duration::from_micros(16_667)
        );

        let mut variable = FrameClock::new(Some(Duration::from_millis(10)), true);
        variable.presented(now - Duration::from_millis(30));
        assert_eq!(
            estimated_vblank_delay(now, now, &variable),
            Duration::from_millis(10)
        );
    }

    #[test]
    fn secondary_gpu_native_output_requires_primary_allocator() {
        assert!(output_uses_native_renderer(true, true));
        assert!(!output_uses_native_renderer(true, false));
        assert!(!output_uses_native_renderer(false, true));
    }

    #[test]
    fn output_renderer_follows_per_surface_swapchain() {
        assert!(surface_uses_native_renderer(true, true));
        assert!(!surface_uses_native_renderer(true, false));
        assert!(!surface_uses_native_renderer(false, true));
    }

    #[test]
    fn secondary_gpu_vulkan_swapchain_uses_importable_linear_targets() {
        let linear = Format {
            code: Fourcc::Argb8888,
            modifier: Modifier::Linear,
        };
        let unsupported = Format {
            code: Fourcc::Abgr8888,
            modifier: Modifier::Linear,
        };
        let implicit = Format {
            code: Fourcc::Argb8888,
            modifier: Modifier::Invalid,
        };
        let compressed = Format {
            code: Fourcc::Argb8888,
            modifier: Modifier::I915_y_tiled_ccs,
        };
        let formats = [linear, unsupported, implicit, compressed]
            .into_iter()
            .collect::<FormatSet>();
        let supports_target = |format: Format| format.code == Fourcc::Argb8888;

        let cross_gpu = vulkan_scanout_formats(&formats, true, supports_target);
        assert_eq!(cross_gpu.iter().count(), 1);
        assert!(cross_gpu.contains(&linear));

        let same_gpu = vulkan_scanout_formats(&formats, false, supports_target);
        assert_eq!(same_gpu.iter().count(), 2);
        assert!(same_gpu.contains(&linear));
        assert!(same_gpu.contains(&implicit));
    }

    #[test]
    fn gles_recovery_prefers_renderable_scanout_formats() {
        let formats = [
            Format {
                code: Fourcc::Abgr2101010,
                modifier: Modifier::I915_y_tiled_ccs,
            },
            Format {
                code: Fourcc::Argb8888,
                modifier: Modifier::Linear,
            },
            Format {
                code: Fourcc::Argb8888,
                modifier: Modifier::Invalid,
            },
        ]
        .into_iter()
        .collect::<FormatSet>();

        assert_eq!(
            gles_swapchain_candidates(&formats, false, false),
            vec![(Fourcc::Argb8888, vec![Modifier::Linear, Modifier::Invalid])]
        );
        assert_eq!(
            gles_swapchain_candidates(&formats, true, false),
            vec![(Fourcc::Argb8888, vec![Modifier::Linear])]
        );
    }

    #[test]
    fn test_calculate_drmmode_from_modeline() {
        let modeline1 = Modeline {
            clock: 173.0,
            hdisplay: 1920,
            vdisplay: 1080,
            hsync_start: 2048,
            hsync_end: 2248,
            htotal: 2576,
            vsync_start: 1083,
            vsync_end: 1088,
            vtotal: 1120,
            hsync_polarity: HSyncPolarity::NHSync,
            vsync_polarity: VSyncPolarity::PVSync,
        };
        assert_debug_snapshot!(calculate_drm_mode_from_modeline(&modeline1).unwrap(), @r#"
        Mode {
            name: "1920x1080@59.96",
            clock: 173000,
            size: (
                1920,
                1080,
            ),
            hsync: (
                2048,
                2248,
                2576,
            ),
            vsync: (
                1083,
                1088,
                1120,
            ),
            hskew: 0,
            vscan: 0,
            vrefresh: 60,
            mode_type: ModeTypeFlags(
                USERDEF,
            ),
        }
        "#);
        let modeline2 = Modeline {
            clock: 452.5,
            hdisplay: 1920,
            vdisplay: 1080,
            hsync_start: 2088,
            hsync_end: 2296,
            htotal: 2672,
            vsync_start: 1083,
            vsync_end: 1088,
            vtotal: 1177,
            hsync_polarity: HSyncPolarity::NHSync,
            vsync_polarity: VSyncPolarity::PVSync,
        };
        assert_debug_snapshot!(calculate_drm_mode_from_modeline(&modeline2).unwrap(), @r#"
        Mode {
            name: "1920x1080@143.88",
            clock: 452500,
            size: (
                1920,
                1080,
            ),
            hsync: (
                2088,
                2296,
                2672,
            ),
            vsync: (
                1083,
                1088,
                1177,
            ),
            hskew: 0,
            vscan: 0,
            vrefresh: 144,
            mode_type: ModeTypeFlags(
                USERDEF,
            ),
        }
        "#);

        for clock in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let mut invalid = modeline1;
            invalid.clock = clock;
            assert!(calculate_drm_mode_from_modeline(&invalid).is_err());
        }
    }

    #[test]
    fn test_calc_cvt() {
        // Crosschecked with other calculators like the cvt commandline utility.
        assert_debug_snapshot!(calculate_mode_cvt(1920, 1080, 60.0).unwrap(), @r#"
        Mode {
            name: "1920x1080@59.96",
            clock: 173000,
            size: (
                1920,
                1080,
            ),
            hsync: (
                2048,
                2248,
                2576,
            ),
            vsync: (
                1083,
                1088,
                1120,
            ),
            hskew: 0,
            vscan: 0,
            vrefresh: 60,
            mode_type: ModeTypeFlags(
                USERDEF,
            ),
        }
        "#);
        assert_debug_snapshot!(calculate_mode_cvt(1920, 1080, 144.0).unwrap(), @r#"
        Mode {
            name: "1920x1080@143.88",
            clock: 452500,
            size: (
                1920,
                1080,
            ),
            hsync: (
                2088,
                2296,
                2672,
            ),
            vsync: (
                1083,
                1088,
                1177,
            ),
            hskew: 0,
            vscan: 0,
            vrefresh: 144,
            mode_type: ModeTypeFlags(
                USERDEF,
            ),
        }
        "#);

        for (width, height, refresh) in [
            (0, 1080, 60.),
            (1920, 0, 60.),
            (u16::MAX, u16::MAX, 60.),
            (1920, 1080, 1_000_000.),
            (1920, 1080, f64::NAN),
            (1920, 1080, f64::INFINITY),
        ] {
            assert!(calculate_mode_cvt(width, height, refresh).is_err());
        }
    }
}
