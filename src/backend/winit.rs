use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Context as _;
use clarion_config::{Config, OutputName};
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::RenderElementStates;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::{Bind, DebugFlags, ImportDma, Offscreen, Renderer};
use smithay::backend::winit::{self, WinitGraphicsBackend};
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::RegistrationToken;
use smithay::reexports::winit::dpi::LogicalSize;
use smithay::reexports::winit::platform::wayland::WindowAttributesWayland;
use smithay::reexports::winit::raw_window_handle::{
    HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle,
};
use smithay::reexports::winit::window::{Window, WindowAttributes};
use smithay::utils::{Buffer, Physical, Size, Transform};
use smithay::wayland::dmabuf::DmabufGlobal;

use super::{AnyRenderer, IpcOutputMap, OutputId, RenderResult, RendererBackendType};
use crate::platform::device_bridge::{DevicePort, DeviceSetup};
use crate::render_helpers::vulkan::device::{VulkanContext, VulkanSurfaceHandle};
use crate::render_helpers::vulkan::native::NativeRenderer;
use crate::render_helpers::vulkan::wsi::{PresentResult, VulkanWsi};
use crate::render_helpers::vulkan::{VulkanRenderer, VulkanTexture};
use crate::utils::logical_output;

pub(crate) mod native_window;
mod retry;
use native_window::NativeWindowLoop;
use retry::Retry;

pub(crate) enum WinitBackend {
    Gles(Box<WinitGraphicsBackend<GlesRenderer>>),
    Native(Box<NativeWinit>),
}

/// The selected device's framebuffer convention, independent of connector names
/// or requested renderer configuration. Vulkan renders canonical top-left rows;
/// the legacy GLES window framebuffer still requires its output Y compensation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FramebufferOrigin {
    BottomLeftGles,
    TopLeftNative,
}

/// Apply only the convention published by the successfully selected Winit device.
/// Unmarked outputs and native Vulkan WSI preserve requested output transforms.
pub(crate) fn output_transform(output: &Output, requested: Transform) -> Transform {
    match output.user_data().get::<FramebufferOrigin>() {
        Some(FramebufferOrigin::BottomLeftGles) => Transform::Flipped180,
        Some(FramebufferOrigin::TopLeftNative) | None => requested,
    }
}

pub(crate) struct NativeWinit {
    // The host window and display connection must outlive the Vulkan surface.
    pub(crate) wsi: VulkanWsi,
    pub(crate) window: Arc<dyn Window>,
    pub(crate) renderer: Box<NativeRenderer>,
    pub(crate) target: Option<VulkanTexture>,
    pub(crate) target_size: Size<i32, Physical>,
    pub(crate) target_valid: bool,
    // Preserve the composition and its source states while the host/GPU cannot
    // accept it. A retry must not advance damage history by rendering it again.
    pub(crate) pending: Option<PendingComposition>,
}

pub(crate) struct PendingComposition {
    pub(crate) states: RenderElementStates,
    pub(crate) locked: bool,
}

impl PendingComposition {
    fn take_for_lock(
        pending: &mut Option<Self>,
        locked: bool,
    ) -> (Option<RenderElementStates>, bool) {
        let Some(pending) = pending.take() else {
            return (None, false);
        };
        if pending.locked == locked {
            (Some(pending.states), false)
        } else {
            // The WSI still owns its acquired image/semaphore, but this source
            // composition can no longer be used to confirm the current lock.
            (None, true)
        }
    }
}

impl WinitBackend {
    fn framebuffer_origin(&self) -> FramebufferOrigin {
        match self {
            Self::Gles(_) => FramebufferOrigin::BottomLeftGles,
            Self::Native(_) => FramebufferOrigin::TopLeftNative,
        }
    }

    fn window_size(&self) -> Size<i32, Physical> {
        match self {
            Self::Gles(backend) => backend.window_size(),
            Self::Native(backend) => {
                if let Some(extent) = backend.wsi.extent() {
                    return (extent.width as i32, extent.height as i32).into();
                }
                let size = backend.window.surface_size();
                (size.width as i32, size.height as i32).into()
            }
        }
    }
}

pub struct Winit {
    pub(crate) config: Rc<super::DeviceSettings>,
    pub(crate) output: Output,
    pub(crate) backend: WinitBackend,
    pub(crate) damage_tracker: OutputDamageTracker,
    pub(crate) dmabuf_global: Option<DmabufGlobal>,
    pub(crate) ipc_outputs: Arc<Mutex<IpcOutputMap>>,
    retry: Retry,
    pub(crate) retry_timer: Option<RegistrationToken>,
    pub(crate) last_attempt_deferred: bool,
}

impl Winit {
    pub(crate) fn new(
        config: Rc<RefCell<Config>>,
        setup: &mut impl DeviceSetup,
        requested_renderer: RendererBackendType,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let _span = clarion_tracy_span!("Winit::new");
        let config = Rc::new(super::DeviceSettings::from(&*config.borrow()));

        let builder = WindowAttributes::default()
            .with_surface_size(LogicalSize::new(1280.0, 800.0))
            // .with_resizable(false)
            .with_title("clarion")
            .with_platform_attributes(Box::new(
                WindowAttributesWayland::default().with_name("clarion", ""),
            ));
        let (backend, gles_events, native_events) = if requested_renderer.is_vulkan() {
            let (window, native_events) = NativeWindowLoop::create(builder)?;
            let handle = native_surface_handle(window.as_ref())?;
            let ctx = VulkanContext::init_for_surface(handle)?;
            let mut renderer = VulkanRenderer::new();
            renderer.init_with_context(ctx.clone())?;
            let mut renderer = Box::new(NativeRenderer::new(renderer)?);
            setup.install_completion_wakeup(&mut renderer)?;
            let size = window.surface_size();
            let wsi = VulkanWsi::new(ctx, handle, size.width, size.height)?;
            if let Some(format) = wsi.format() {
                renderer.prepare_output_format(format)?;
            }
            let native = NativeWinit {
                wsi,
                window,
                renderer,
                target: None,
                target_size: (0, 0).into(),
                target_valid: false,
                pending: None,
            };
            info!("nested native Vulkan WSI renderer initialized");
            (
                WinitBackend::Native(Box::new(native)),
                None,
                Some(native_events),
            )
        } else {
            let (backend, events) = winit::init_from_attributes(builder)?;
            (WinitBackend::Gles(Box::new(backend)), Some(events), None)
        };

        let output = Output::new(
            "winit".to_string(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "Smithay".into(),
                model: "Winit".into(),
                serial_number: "Unknown".into(),
            },
        );

        // Publish the actual chosen backend before device setup initializes the
        // output. Reload and initial configuration consume the same typed marker.
        output
            .user_data()
            .insert_if_missing(|| backend.framebuffer_origin());

        let mode = Mode {
            size: backend.window_size(),
            refresh: 60_000,
        };
        output.change_current_state(Some(mode), None, None, None);
        output.set_preferred(mode);

        output.user_data().insert_if_missing(|| OutputName {
            connector: "winit".to_string(),
            make: Some("Smithay".to_string()),
            model: Some("Winit".to_string()),
            serial: None,
        });

        let physical_properties = output.physical_properties();
        let ipc_outputs = Arc::new(Mutex::new(HashMap::from([(
            OutputId::next(),
            clarion_ipc::Output {
                name: output.name(),
                make: physical_properties.make,
                model: physical_properties.model,
                serial: None,
                physical_size: None,
                modes: vec![clarion_ipc::Mode {
                    width: backend.window_size().w.clamp(0, u16::MAX as i32) as u16,
                    height: backend.window_size().h.clamp(0, u16::MAX as i32) as u16,
                    refresh_rate: 60_000,
                    is_preferred: true,
                }],
                current_mode: Some(0),
                is_custom_mode: true,
                vrr_supported: false,
                vrr_enabled: false,
                logical: Some(logical_output(&output)),
                max_bpc: None,
                hdr_active: false,
            },
        )])));

        let damage_tracker = OutputDamageTracker::from_output(&output);

        if let Some(events) = gles_events {
            setup.register_nested_gles(events)?;
        }
        if let Some(events) = native_events {
            setup.register_nested_native(events)?;
        }

        Ok(Self {
            config,
            output,
            backend,
            damage_tracker,
            dmabuf_global: None,
            ipc_outputs,
            retry: Retry::default(),
            retry_timer: None,
            last_attempt_deferred: false,
        })
    }

    pub(crate) fn init(&mut self, port: &mut impl DevicePort) {
        port.initialize_winit(self);
    }

    pub fn seat_name(&self) -> String {
        "winit".to_owned()
    }

    pub fn with_primary_renderer<T>(
        &mut self,
        f: impl FnOnce(&mut GlesRenderer) -> T,
    ) -> Option<T> {
        match &mut self.backend {
            WinitBackend::Gles(backend) => Some(f(backend.renderer())),
            WinitBackend::Native(_) => None,
        }
    }

    pub fn with_renderer<T>(&mut self, f: impl FnOnce(AnyRenderer<'_>) -> T) -> Option<T> {
        Some(match &mut self.backend {
            WinitBackend::Gles(backend) => f(AnyRenderer::Gles(backend.renderer())),
            WinitBackend::Native(native) => f(AnyRenderer::Native(&mut native.renderer)),
        })
    }

    pub fn renderer_backend(&self) -> RendererBackendType {
        match self.backend {
            WinitBackend::Gles(_) => RendererBackendType::Gles,
            WinitBackend::Native(_) => RendererBackendType::Vulkan,
        }
    }

    pub(crate) fn render(
        &mut self,
        port: &mut impl crate::frame_pipeline::FramePort,
        output: &Output,
    ) -> RenderResult {
        let _span = clarion_tracy_span!("Winit::render");
        self.last_attempt_deferred = false;

        if matches!(self.backend, WinitBackend::Native(_)) && self.retry.waiting(Instant::now()) {
            self.last_attempt_deferred = self.retry.pending();
            // Input/client commits remain coalesced behind the existing timer.
            // They cannot turn a pending host image into an immediate GPU poll.
            return RenderResult::Skipped;
        }

        if let WinitBackend::Native(native) = &self.backend {
            if let Some(extent) = native.wsi.extent() {
                let size: Size<i32, Physical> = (extent.width as i32, extent.height as i32).into();
                if output.current_mode().is_some_and(|mode| mode.size != size) {
                    self.update_output_size(port, size);
                }
            }
        }

        let rendered = match &mut self.backend {
            WinitBackend::Gles(backend) => {
                render_gles(backend, &mut self.damage_tracker, &self.config, port)
            }
            WinitBackend::Native(native) => {
                render_native(native, &mut self.damage_tracker, port, output)
            }
        };
        let (states, rv) = match rendered {
            Ok(value) => value,
            Err(err) => {
                let native = matches!(self.backend, WinitBackend::Native(_));
                if !native || self.retry.should_warn(Instant::now()) {
                    warn!("winit renderer failed on {}: {err:#}", output.name());
                }
                // Keep a pending composition/acquired image together. Other
                // failures invalidate damage history for the next full repaint.
                let preserve = match &mut self.backend {
                    WinitBackend::Native(native) => {
                        native.target_valid = false;
                        native.pending.is_some()
                    }
                    WinitBackend::Gles(_) => false,
                };
                if !preserve {
                    self.damage_tracker = OutputDamageTracker::from_output(output);
                }
                if native {
                    self.defer_native_attempt(port, true);
                }
                return RenderResult::Skipped;
            }
        };
        if rv.is_unpresented() {
            if let WinitBackend::Native(native) = &mut self.backend {
                if native.pending.is_none() {
                    self.damage_tracker = OutputDamageTracker::from_output(output);
                    native.target_valid = false;
                }
                self.last_attempt_deferred = true;
                self.defer_native_attempt(port, false);
                return rv;
            }
        } else if matches!(self.backend, WinitBackend::Native(_)) {
            self.retry.recovered();
            if let Some(token) = self.retry_timer.take() {
                port.cancel_timer(token);
            }
        }
        port.update_scanout(&states);
        if rv == RenderResult::Submitted {
            port.present_immediately(&states);
        }
        let animations_remain = port.finish_nested_redraw();

        // Host submission paces damaged frames. Empty frames resume their animations
        // at the estimated refresh deadline in finish_frame instead of spinning here.
        if rv == RenderResult::Submitted && animations_remain {
            match &self.backend {
                WinitBackend::Gles(backend) => backend.window().request_redraw(),
                WinitBackend::Native(native) => native.window.request_redraw(),
            }
        }

        rv
    }

    pub(crate) fn redraw_is_deferred(&self) -> bool {
        self.last_attempt_deferred
    }

    pub(crate) fn retry_timer_fired(&mut self) {
        self.retry_timer = None;
    }

    pub(crate) fn defer_native_attempt(
        &mut self,
        port: &mut impl crate::frame_pipeline::FrameServices,
        failed: bool,
    ) {
        if self.retry_timer.is_some() {
            return;
        }
        let deadline = self
            .retry
            .defer(Instant::now(), port.request().refresh_interval, failed);
        // The platform coordinator owns timer callbacks and output queueing.
        self.retry_timer = port.arm_nested_retry(deadline);
    }

    pub fn toggle_debug_tint(&mut self) {
        match &mut self.backend {
            WinitBackend::Gles(backend) => {
                let renderer = backend.renderer();
                renderer.set_debug_flags(renderer.debug_flags() ^ DebugFlags::TINT);
            }
            WinitBackend::Native(native) => {
                let renderer = native.renderer.as_mut();
                renderer.set_debug_flags(renderer.debug_flags() ^ DebugFlags::TINT);
            }
        }
    }

    pub fn import_dmabuf(&mut self, dmabuf: &Dmabuf) -> bool {
        let result = match &mut self.backend {
            WinitBackend::Gles(backend) => backend
                .renderer()
                .import_dmabuf(dmabuf, None)
                .map(|_| ())
                .map_err(|e| e.to_string()),
            WinitBackend::Native(native) => native
                .renderer
                .import_dmabuf(dmabuf, None)
                .map(|_| ())
                .map_err(|e| e.to_string()),
        };
        match result {
            Ok(()) => true,
            Err(err) => {
                debug!("error importing dmabuf: {err:?}");
                false
            }
        }
    }

    pub fn ipc_outputs(&self) -> Arc<Mutex<IpcOutputMap>> {
        self.ipc_outputs.clone()
    }

    pub(crate) fn update_output_size(
        &mut self,
        port: &mut impl crate::frame_pipeline::FrameServices,
        size: Size<i32, Physical>,
    ) {
        if self
            .output
            .current_mode()
            .is_some_and(|mode| mode.size == size)
        {
            return;
        }
        self.output.change_current_state(
            Some(Mode {
                size,
                refresh: 60_000,
            }),
            None,
            None,
            None,
        );
        {
            let mut ipc_outputs = self.ipc_outputs.lock().unwrap();
            let output = ipc_outputs.values_mut().next().unwrap();
            refresh_ipc_output_size(&self.output, output);
        }
        port.output_resized();
    }
}

pub(crate) fn refresh_ipc_output_size(output: &Output, snapshot: &mut clarion_ipc::Output) {
    let size = output.current_mode().unwrap().size;
    let mode = &mut snapshot.modes[0];
    mode.width = size.w.clamp(0, u16::MAX as i32) as u16;
    mode.height = size.h.clamp(0, u16::MAX as i32) as u16;
    if let Some(logical) = snapshot.logical.as_mut() {
        *logical = logical_output(output);
    }
}

pub(crate) use crate::frame_pipeline::finish_nested_frame as finish_frame;

pub(crate) fn native_surface_handle(window: &dyn Window) -> anyhow::Result<VulkanSurfaceHandle> {
    let display = window.display_handle()?.as_raw();
    let window = window.window_handle()?.as_raw();
    match (display, window) {
        (RawDisplayHandle::Wayland(display), RawWindowHandle::Wayland(window)) => {
            Ok(VulkanSurfaceHandle::Wayland {
                display: display.display.as_ptr(),
                surface: window.surface.as_ptr(),
            })
        }
        (RawDisplayHandle::Xlib(display), RawWindowHandle::Xlib(window)) => {
            Ok(VulkanSurfaceHandle::Xlib {
                display: display
                    .display
                    .context("Xlib display pointer unavailable")?
                    .as_ptr(),
                window: window.window,
            })
        }
        (RawDisplayHandle::Xcb(display), RawWindowHandle::Xcb(window)) => {
            Ok(VulkanSurfaceHandle::Xcb {
                connection: display
                    .connection
                    .context("XCB connection pointer unavailable")?
                    .as_ptr(),
                window: window.window.get(),
            })
        }
        _ => anyhow::bail!("host Winit window system has no supported Vulkan surface"),
    }
}

pub(crate) fn render_gles(
    backend: &mut WinitGraphicsBackend<GlesRenderer>,
    damage_tracker: &mut OutputDamageTracker,
    config: &super::DeviceSettings,
    port: &mut impl crate::frame_pipeline::FramePort,
) -> anyhow::Result<(RenderElementStates, RenderResult)> {
    let scene = port.prepare_scene(backend.renderer());
    let elements = scene.elements();
    let res = {
        let (renderer, mut framebuffer) = backend.bind()?;
        damage_tracker.render_output(renderer, &mut framebuffer, 0, elements, [0.; 4])?
    };
    let rv = if let Some(damage) = res.damage {
        if config.debug.wait_for_frame_completion_before_queueing {
            if let Err(err) = res.sync.wait() {
                warn!("error waiting for GLES frame completion: {err:?}");
            }
        }
        backend.submit(Some(damage))?;
        RenderResult::Submitted
    } else {
        RenderResult::NoDamage
    };
    Ok((res.states, rv))
}

pub(crate) fn render_native(
    native: &mut NativeWinit,
    damage_tracker: &mut OutputDamageTracker,
    port: &mut impl crate::frame_pipeline::FramePort,
    output: &Output,
) -> anyhow::Result<(RenderElementStates, RenderResult)> {
    let locked = port.request().locked;
    let (pending, lock_changed) = PendingComposition::take_for_lock(&mut native.pending, locked);
    if let Some(states) = pending {
        return present_native(native, states, true, locked);
    }
    if lock_changed {
        native.target_valid = false;
        *damage_tracker = OutputDamageTracker::from_output(output);
    }
    let size = native.window.surface_size();
    if size.width == 0 || size.height == 0 {
        // A hidden host window still has not submitted locked pixels. Keep
        // the lock request pending until a nonzero target can be presented.
        return Ok((
            RenderElementStates::default(),
            if locked {
                RenderResult::Skipped
            } else {
                RenderResult::NoDamage
            },
        ));
    }
    if native.wsi.requested_size() != Some((size.width, size.height))
        && !native.wsi.has_acquired_image()
    {
        if !native.wsi.resize(size.width, size.height)? {
            return Ok((RenderElementStates::default(), RenderResult::Skipped));
        }
        native.target = None;
        native.target_valid = false;
    }
    let extent = native
        .wsi
        .extent()
        .context("host Vulkan swapchain has no extent")?;
    let wanted: Size<i32, Physical> = (extent.width as i32, extent.height as i32).into();
    let format = native
        .wsi
        .format()
        .context("host Vulkan surface has no supported format")?;
    if native.target.is_none()
        || native.target_size != wanted
        || native
            .target
            .as_ref()
            .and_then(smithay::backend::renderer::Texture::format)
            != Some(format)
    {
        native.renderer.prepare_output_format(format)?;
        native.target = Some(
            native
                .renderer
                .create_buffer(format, Size::<i32, Buffer>::from((wanted.w, wanted.h)))?,
        );
        native.target_size = wanted;
        native.target_valid = false;
    }
    let renderer = native.renderer.as_mut();
    renderer.begin_scene();
    let scene = port.prepare_scene(renderer);
    if renderer.scene_was_deferred() {
        return Ok((RenderElementStates::default(), RenderResult::GpuDeferred));
    }
    let elements = scene.elements();
    let mut framebuffer = renderer.bind(native.target.as_mut().unwrap())?;
    renderer.set_completion_export_required(true);
    if let (Some(tag), Some(extent)) = (
        port.request().gpu_timing_tag,
        Some((wanted.w as u32, wanted.h as u32)),
    ) {
        renderer.arm_output_timing(tag, extent);
    }
    let res = damage_tracker.render_output(
        renderer,
        &mut framebuffer,
        usize::from(native.target_valid),
        elements,
        [0.; 4],
    );
    renderer.clear_output_timing();
    renderer.set_completion_export_required(false);
    let res = match res {
        Err(error)
            if crate::render_helpers::vulkan::native::NativeError::is_backpressure(&error) =>
        {
            return Ok((RenderElementStates::default(), RenderResult::GpuDeferred));
        }
        result => result?,
    };
    if res.damage.is_some() {
        present_native(native, res.states, false, locked)
    } else {
        Ok((res.states, RenderResult::NoDamage))
    }
}

pub(crate) fn present_native(
    native: &mut NativeWinit,
    states: RenderElementStates,
    resumed: bool,
    locked: bool,
) -> anyhow::Result<(RenderElementStates, RenderResult)> {
    match native
        .wsi
        .present_from(native.target.as_ref().unwrap(), native.window.as_ref())
    {
        Ok(PresentResult::Presented) => {
            native.target_valid = true;
            if resumed {
                // Commits coalesced while presenting this older composition
                // must be rendered next, even after its animation has finished.
                native.window.request_redraw();
            }
            Ok((states, RenderResult::Submitted))
        }
        Ok(PresentResult::Pending) => {
            native.pending = Some(PendingComposition { states, locked });
            Ok((RenderElementStates::default(), RenderResult::Skipped))
        }
        Ok(PresentResult::Recreate) => {
            native.target_valid = false;
            Ok((RenderElementStates::default(), RenderResult::Skipped))
        }
        Err(err) => {
            native.pending = Some(PendingComposition { states, locked });
            Err(err)
        }
    }
}

#[cfg(test)]
mod tests {
    use smithay::output::Scale;
    use smithay::utils::Transform;

    use super::*;

    #[test]
    fn pending_unlocked_composition_is_discarded_before_lock_confirmation() {
        let mut pending = Some(PendingComposition {
            states: RenderElementStates::default(),
            locked: false,
        });
        let (states, changed) = PendingComposition::take_for_lock(&mut pending, true);
        assert!(states.is_none() && changed && pending.is_none());
        // Only the newly composed locked target can resume and be submitted.
        pending = Some(PendingComposition {
            states: RenderElementStates::default(),
            locked: true,
        });
        let (states, changed) = PendingComposition::take_for_lock(&mut pending, true);
        assert!(states.is_some() && !changed && pending.is_none());
        pending = Some(PendingComposition {
            states: RenderElementStates::default(),
            locked: true,
        });
        let (states, changed) = PendingComposition::take_for_lock(&mut pending, false);
        assert!(states.is_none() && changed);
    }

    fn output_and_snapshot() -> (Output, clarion_ipc::Output) {
        let output = Output::new(
            "resize-test".into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "test".into(),
                model: "test".into(),
                serial_number: String::new(),
            },
        );
        output.change_current_state(
            Some(Mode {
                size: (1280, 800).into(),
                refresh: 60_000,
            }),
            Some(Transform::Normal),
            Some(Scale::Fractional(1.25)),
            Some((10, 20).into()),
        );
        let snapshot = clarion_ipc::Output {
            name: output.name(),
            make: "test".into(),
            model: "test".into(),
            serial: None,
            physical_size: None,
            modes: vec![clarion_ipc::Mode {
                width: 1280,
                height: 800,
                refresh_rate: 60_000,
                is_preferred: true,
            }],
            current_mode: Some(0),
            is_custom_mode: true,
            vrr_supported: false,
            vrr_enabled: false,
            logical: Some(logical_output(&output)),
            max_bpc: None,
            hdr_active: false,
        };
        (output, snapshot)
    }

    #[test]
    fn resize_snapshot_keeps_physical_mode_and_rounded_logical_geometry() {
        let (output, mut snapshot) = output_and_snapshot();
        output.change_current_state(
            Some(Mode {
                size: (1896, 2150).into(),
                refresh: 60_000,
            }),
            None,
            None,
            None,
        );
        refresh_ipc_output_size(&output, &mut snapshot);
        assert_eq!(
            (snapshot.modes[0].width, snapshot.modes[0].height),
            (1896, 2150)
        );
        assert_eq!(
            snapshot.logical,
            Some(clarion_ipc::LogicalOutput {
                x: 10,
                y: 20,
                width: 1517,
                height: 1720,
                scale: 1.25,
                transform: clarion_ipc::Transform::Normal,
            })
        );

        output.change_current_state(
            Some(Mode {
                size: (1918, 1078).into(),
                refresh: 60_000,
            }),
            Some(Transform::_90),
            Some(Scale::Fractional(1.5)),
            Some((-240, 36).into()),
        );
        refresh_ipc_output_size(&output, &mut snapshot);
        assert_eq!(
            (snapshot.modes[0].width, snapshot.modes[0].height),
            (1918, 1078)
        );
        assert_eq!(
            snapshot.logical,
            Some(clarion_ipc::LogicalOutput {
                x: -240,
                y: 36,
                width: 719,
                height: 1279,
                scale: 1.5,
                transform: clarion_ipc::Transform::_90,
            })
        );
    }

    #[test]
    fn resize_snapshot_preserves_unmapped_output() {
        let (output, mut snapshot) = output_and_snapshot();
        snapshot.logical = None;
        output.change_current_state(
            Some(Mode {
                size: (1920, 1080).into(),
                refresh: 60_000,
            }),
            None,
            None,
            None,
        );
        refresh_ipc_output_size(&output, &mut snapshot);
        assert_eq!(
            (snapshot.modes[0].width, snapshot.modes[0].height),
            (1920, 1080)
        );
        assert!(snapshot.logical.is_none());
    }

    fn all_transforms() -> [Transform; 8] {
        [
            Transform::Normal,
            Transform::_90,
            Transform::_180,
            Transform::_270,
            Transform::Flipped,
            Transform::Flipped90,
            Transform::Flipped180,
            Transform::Flipped270,
        ]
    }

    #[test]
    fn native_framebuffer_origin_preserves_upright_corners_and_user_transforms() {
        let (output, _) = output_and_snapshot();
        output
            .user_data()
            .insert_if_missing(|| FramebufferOrigin::TopLeftNative);
        let size = Size::<i32, Physical>::from((80, 60));
        let corners = [(0, 0), (80, 0), (0, 60), (80, 60)];
        let upright: Vec<_> = corners
            .into_iter()
            .map(|point| {
                output_transform(&output, Transform::Normal)
                    .transform_point_in(smithay::utils::Point::from(point), &size)
            })
            .collect();
        assert_eq!(
            upright,
            corners
                .into_iter()
                .map(smithay::utils::Point::from)
                .collect::<Vec<_>>()
        );
        for requested in all_transforms() {
            assert_eq!(output_transform(&output, requested), requested);
        }
    }

    #[test]
    fn gles_framebuffer_origin_retains_legacy_window_compensation() {
        // The connector is arbitrary: convention belongs to the selected device.
        let (output, _) = output_and_snapshot();
        output
            .user_data()
            .insert_if_missing(|| FramebufferOrigin::BottomLeftGles);
        for requested in all_transforms() {
            assert_eq!(output_transform(&output, requested), Transform::Flipped180);
        }
    }

    #[test]
    fn unmarked_winit_name_never_implies_a_framebuffer_origin() {
        let output = Output::new(
            "winit".into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "test".into(),
                model: "test".into(),
                serial_number: String::new(),
            },
        );
        for requested in all_transforms() {
            assert_eq!(output_transform(&output, requested), requested);
        }
    }
}
