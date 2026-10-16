use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clarion_config::{Config, ModKey};
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;

use crate::platform::device_bridge::DevicePort;
use crate::utils::id::IdCounter;

pub mod tty;
pub use tty::Tty;

pub mod winit;
pub use winit::Winit;

pub mod headless;
pub use headless::Headless;

pub use crate::render_helpers::renderer::{
    AnyFrame, AnyRenderer, AsAnyFrame, AsAnyRenderer, RendererBackendType,
};

#[allow(clippy::large_enum_variant)]
pub enum Backend {
    Tty(Tty),
    Winit(Winit),
    Headless(Headless),
}

#[derive(PartialEq, Eq)]
pub enum RenderResult {
    /// The frame was submitted to the backend for presentation.
    Submitted,
    /// Rendering succeeded, but there was no damage.
    NoDamage,
    /// The frame was not rendered and submitted, due to an error or otherwise.
    Skipped,
    /// Bounded GPU work is pending. Keep active presentation continuity so
    /// overload is measured as a real gap rather than mistaken for idle time.
    GpuDeferred,
}

impl RenderResult {
    pub(crate) fn is_unpresented(&self) -> bool {
        matches!(self, Self::Skipped | Self::GpuDeferred)
    }
}

pub type IpcOutputMap = HashMap<OutputId, clarion_ipc::Output>;

static OUTPUT_ID_COUNTER: IdCounter = IdCounter::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OutputId(u64);

impl OutputId {
    pub(crate) fn next() -> OutputId {
        OutputId(OUTPUT_ID_COUNTER.next())
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

/// Immutable device settings. This omits input, desktop policy and Shell state.
/// Snapshots are replaced explicitly at lifecycle/config transitions.
#[derive(Debug, Clone)]
pub(crate) struct DeviceSettings {
    pub(crate) outputs: clarion_config::Outputs,
    pub(crate) animations: clarion_config::Animations,
    pub(crate) debug: DeviceDebug,
}

#[derive(Debug, Clone)]
pub(crate) struct DeviceDebug {
    pub(crate) wait_for_frame_completion_before_queueing: bool,
    pub(crate) enable_overlay_planes: bool,
    pub(crate) disable_cursor_plane: bool,
    pub(crate) disable_direct_scanout: bool,
    pub(crate) restrict_primary_scanout_to_matching_format: bool,
    pub(crate) force_disable_connectors_on_resume: bool,
    pub(crate) render_drm_device: Option<std::path::PathBuf>,
    pub(crate) ignored_drm_devices: Vec<std::path::PathBuf>,
    pub(crate) keep_laptop_panel_on_when_lid_is_closed: bool,
    pub(crate) disable_monitor_names: bool,
    pub(crate) skip_cursor_only_updates_during_vrr: bool,
    pub(crate) disable_10bit_output: bool,
}

impl From<&Config> for DeviceSettings {
    fn from(config: &Config) -> Self {
        let debug = &config.debug;
        Self {
            outputs: config.outputs.clone(),
            animations: config.animations.clone(),
            debug: DeviceDebug {
                wait_for_frame_completion_before_queueing: debug
                    .wait_for_frame_completion_before_queueing,
                enable_overlay_planes: debug.enable_overlay_planes,
                disable_cursor_plane: debug.disable_cursor_plane,
                disable_direct_scanout: debug.disable_direct_scanout,
                restrict_primary_scanout_to_matching_format: debug
                    .restrict_primary_scanout_to_matching_format,
                force_disable_connectors_on_resume: debug.force_disable_connectors_on_resume,
                render_drm_device: debug.render_drm_device.clone(),
                ignored_drm_devices: debug.ignored_drm_devices.clone(),
                keep_laptop_panel_on_when_lid_is_closed: debug
                    .keep_laptop_panel_on_when_lid_is_closed,
                disable_monitor_names: debug.disable_monitor_names,
                skip_cursor_only_updates_during_vrr: debug.skip_cursor_only_updates_during_vrr,
                disable_10bit_output: debug.disable_10bit_output,
            },
        }
    }
}

impl Backend {
    /// Publish a settings generation at a config/lifecycle transition. Device
    /// code retains an immutable snapshot and cannot mutate desktop settings.
    pub(crate) fn update_config_snapshot(&mut self, config: &Config) {
        let snapshot = std::rc::Rc::new(DeviceSettings::from(config));
        match self {
            Self::Tty(device) => device.config = snapshot,
            Self::Winit(device) => device.config = snapshot,
            Self::Headless(device) => device.config = snapshot,
        }
    }

    pub(crate) fn init(&mut self, port: &mut impl DevicePort) {
        let _span = clarion_tracy_span!("Backend::init");
        match self {
            Backend::Tty(tty) => tty.init(port),
            Backend::Winit(winit) => winit.init(port),
            Backend::Headless(headless) => headless.init(port),
        }
    }

    pub fn seat_name(&self) -> String {
        match self {
            Backend::Tty(tty) => tty.seat_name(),
            Backend::Winit(winit) => winit.seat_name(),
            Backend::Headless(headless) => headless.seat_name(),
        }
    }

    pub fn with_primary_renderer<T>(
        &mut self,
        f: impl FnOnce(&mut GlesRenderer) -> T,
    ) -> Option<T> {
        match self {
            Backend::Tty(tty) => tty.with_primary_renderer(f),
            Backend::Winit(winit) => winit.with_primary_renderer(f),
            Backend::Headless(headless) => headless.with_primary_renderer(f),
        }
    }

    /// Access the primary renderer as a polymorphic `AnyRenderer`.
    pub fn with_renderer<T>(&mut self, f: impl FnOnce(AnyRenderer<'_>) -> T) -> Option<T> {
        match self {
            Backend::Tty(tty) => tty.with_renderer(f),
            Backend::Winit(winit) => winit.with_renderer(f),
            Backend::Headless(headless) => headless.with_renderer(f),
        }
    }

    /// Access the renderer that can render to a particular output.
    ///
    /// TTY backends may have a native renderer available globally while an individual KMS
    /// surface has migrated to a GLES-compatible swapchain. Callers that render output-specific
    /// content (snapshots, captures, or grabs) must use this accessor so that backend-owned
    /// textures and render elements stay compatible with the target surface. Backends with one
    /// renderer ignore the output argument. Passing `None` preserves the global renderer choice.
    pub fn with_renderer_for_output<T>(
        &mut self,
        output: Option<&Output>,
        f: impl FnOnce(AnyRenderer<'_>) -> T,
    ) -> Option<T> {
        match self {
            Backend::Tty(tty) => tty.with_renderer_for_output(output, f),
            Backend::Winit(winit) => winit.with_renderer(f),
            Backend::Headless(headless) => headless.with_renderer(f),
        }
    }

    /// Returns the initialized backend, including any runtime GLES fallback.
    pub fn renderer_backend(&self) -> RendererBackendType {
        match self {
            Backend::Tty(tty) => tty.renderer_backend(),
            Backend::Winit(winit) => winit.renderer_backend(),
            Backend::Headless(headless) => headless.renderer_backend(),
        }
    }

    pub(crate) fn render(
        &mut self,
        port: &mut impl crate::frame_pipeline::FramePort,
        output: &Output,
        target_presentation_time: Duration,
    ) -> RenderResult {
        match self {
            Backend::Tty(tty) => tty.render(port, output, target_presentation_time),
            Backend::Winit(winit) => winit.render(port, output),
            Backend::Headless(headless) => headless.render(port, output),
        }
    }

    /// Temporary nested presentation backpressure preserves an outstanding
    /// session-lock request. A genuine renderer failure keeps its failure path.
    pub(crate) fn redraw_is_deferred(&self) -> bool {
        matches!(self, Backend::Winit(winit) if winit.redraw_is_deferred())
    }

    pub fn mod_key(&self, config: &Config) -> ModKey {
        match self {
            Backend::Winit(_) => config.input.mod_key_nested.unwrap_or({
                if let Some(ModKey::Alt) = config.input.mod_key {
                    ModKey::Super
                } else {
                    ModKey::Alt
                }
            }),
            Backend::Tty(_) | Backend::Headless(_) => config.input.mod_key.unwrap_or(ModKey::Super),
        }
    }

    pub fn change_vt(&mut self, vt: i32) {
        match self {
            Backend::Tty(tty) => tty.change_vt(vt),
            Backend::Winit(_) => (),
            Backend::Headless(_) => (),
        }
    }

    pub fn suspend(&mut self) {
        match self {
            Backend::Tty(tty) => tty.suspend(),
            Backend::Winit(_) => (),
            Backend::Headless(_) => (),
        }
    }

    pub fn toggle_debug_tint(&mut self) {
        match self {
            Backend::Tty(tty) => tty.toggle_debug_tint(),
            Backend::Winit(winit) => winit.toggle_debug_tint(),
            Backend::Headless(_) => (),
        }
    }

    pub fn import_dmabuf(&mut self, dmabuf: &Dmabuf) -> bool {
        match self {
            Backend::Tty(tty) => tty.import_dmabuf(dmabuf),
            Backend::Winit(winit) => winit.import_dmabuf(dmabuf),
            Backend::Headless(headless) => headless.import_dmabuf(dmabuf),
        }
    }

    pub fn early_import(&mut self, surface: &WlSurface) {
        match self {
            Backend::Tty(tty) => tty.early_import(surface),
            Backend::Winit(_) => (),
            Backend::Headless(_) => (),
        }
    }

    pub fn ipc_outputs(&self) -> Arc<Mutex<IpcOutputMap>> {
        match self {
            Backend::Tty(tty) => tty.ipc_outputs(),
            Backend::Winit(winit) => winit.ipc_outputs(),
            Backend::Headless(headless) => headless.ipc_outputs(),
        }
    }

    #[cfg(feature = "xdp-gnome-screencast")]
    pub fn gbm_device(
        &self,
    ) -> Option<smithay::backend::allocator::gbm::GbmDevice<smithay::backend::drm::DrmDeviceFd>>
    {
        match self {
            Backend::Tty(tty) => tty.primary_gbm_device(),
            Backend::Winit(_) => None,
            Backend::Headless(_) => None,
        }
    }

    pub fn set_monitors_active(&mut self, active: bool) {
        match self {
            Backend::Tty(tty) => tty.set_monitors_active(active),
            Backend::Winit(_) => (),
            Backend::Headless(_) => (),
        }
    }

    pub(crate) fn set_output_on_demand_vrr(
        &mut self,
        port: &mut impl DevicePort,
        output: &Output,
        enable_vrr: bool,
    ) {
        match self {
            Backend::Tty(tty) => tty.set_output_on_demand_vrr(port, output, enable_vrr),
            Backend::Winit(_) => (),
            Backend::Headless(_) => (),
        }
    }

    pub(crate) fn update_ignored_nodes_config(&mut self, port: &mut impl DevicePort) {
        match self {
            Backend::Tty(tty) => tty.update_ignored_nodes_config(port),
            Backend::Winit(_) => (),
            Backend::Headless(_) => (),
        }
    }

    pub(crate) fn on_output_config_changed(&mut self, port: &mut impl DevicePort) {
        match self {
            Backend::Tty(tty) => tty.on_output_config_changed(port),
            Backend::Winit(_) => (),
            Backend::Headless(_) => (),
        }
    }

    pub fn tty_checked(&mut self) -> Option<&mut Tty> {
        if let Self::Tty(v) = self {
            Some(v)
        } else {
            None
        }
    }

    pub fn tty(&mut self) -> &mut Tty {
        if let Self::Tty(v) = self {
            v
        } else {
            panic!("backend is not Tty");
        }
    }

    pub fn winit(&mut self) -> &mut Winit {
        if let Self::Winit(v) = self {
            v
        } else {
            panic!("backend is not Winit")
        }
    }

    pub fn headless(&mut self) -> &mut Headless {
        if let Self::Headless(v) = self {
            v
        } else {
            panic!("backend is not Headless")
        }
    }
}
