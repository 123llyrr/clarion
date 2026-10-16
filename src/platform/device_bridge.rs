//! Event-side device boundary. Device code receives only source-registration and
//! finite lifecycle operations; the desktop reference remains private here.

use anyhow::Context as _;
use smithay::backend::libinput::LibinputInputBackend;
use smithay::backend::session::libseat::LibSeatSessionNotifier;
use smithay::backend::udev::UdevBackend;
use smithay::backend::winit::WinitEventLoop;
use smithay::output::Output;
use smithay::reexports::calloop::{Dispatcher, LoopHandle};

use crate::backend::winit::native_window::NativeWindowLoop;
use crate::backend::{Headless, Tty, Winit};
use crate::compositor::{Clarion, State};
use crate::render_helpers::vulkan::native::NativeRenderer;

mod headless;
mod tty;
mod vulkan_recovery;
mod winit;

/// Opaque registration ownership. No accessor exposes the callback state or event loop.
pub(crate) struct UdevInventory {
    dispatcher: Dispatcher<'static, UdevBackend, State>,
}

pub(crate) trait DeviceSetup {
    fn register_udev(&mut self, backend: UdevBackend) -> UdevInventory;
    fn register_libinput(&mut self, backend: LibinputInputBackend);
    fn register_session(&mut self, notifier: LibSeatSessionNotifier);
    fn register_vulkan_recovery(
        &mut self,
        source: smithay::reexports::calloop::ping::PingSource,
    ) -> anyhow::Result<()>;
    fn register_nested_gles(
        &mut self,
        events: WinitEventLoop,
    ) -> Result<(), Box<dyn std::error::Error>>;
    fn register_nested_native(
        &mut self,
        events: NativeWindowLoop,
    ) -> Result<(), Box<dyn std::error::Error>>;
    fn install_completion_wakeup(&mut self, renderer: &mut NativeRenderer) -> anyhow::Result<()>;
}

/// Owns event-loop integration before the desktop instance has been constructed.
pub(crate) struct DeviceBootstrap {
    event_loop: LoopHandle<'static, State>,
}

impl DeviceBootstrap {
    pub(crate) fn new(event_loop: LoopHandle<'static, State>) -> Self {
        Self { event_loop }
    }
}

impl DeviceSetup for DeviceBootstrap {
    fn register_udev(&mut self, backend: UdevBackend) -> UdevInventory {
        let dispatcher = Dispatcher::new(backend, |event, _, state: &mut State| {
            tty::on_udev(state, event)
        });
        self.event_loop
            .register_dispatcher(dispatcher.clone())
            .unwrap();
        UdevInventory { dispatcher }
    }
    fn register_libinput(&mut self, backend: LibinputInputBackend) {
        self.event_loop
            .insert_source(backend, |mut event, _, state| {
                let _latency =
                    crate::utils::latency::Span::new(crate::utils::latency::Operation::Input);
                state.process_libinput_event(&mut event);
                state.process_input_event(event);
            })
            .unwrap();
    }
    fn register_session(&mut self, notifier: LibSeatSessionNotifier) {
        self.event_loop
            .insert_source(notifier, |event, _, state| tty::on_session(state, event))
            .unwrap();
    }
    fn register_vulkan_recovery(
        &mut self,
        source: smithay::reexports::calloop::ping::PingSource,
    ) -> anyhow::Result<()> {
        self.event_loop
            .insert_source(source, |(), &mut (), state| {
                vulkan_recovery::on_completion(state);
            })
            .map_err(|error| anyhow::anyhow!("cannot attach Vulkan recovery: {}", error.error))?;
        Ok(())
    }
    fn register_nested_gles(
        &mut self,
        events: WinitEventLoop,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.event_loop
            .insert_source(events, |event, _, state| winit::on_gles_event(state, event))?;
        Ok(())
    }
    fn register_nested_native(
        &mut self,
        events: NativeWindowLoop,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.event_loop.insert_source(events, |event, _, state| {
            winit::on_native_event(state, event)
        })?;
        Ok(())
    }
    fn install_completion_wakeup(&mut self, renderer: &mut NativeRenderer) -> anyhow::Result<()> {
        install_native_shader_wakeup(renderer, &self.event_loop)?;
        Ok(renderer.install_completion_wakeup_with_callbacks(
            &self.event_loop,
            |sample, state| state.clarion.record_gpu_timing(sample),
            State::on_native_gpu_completion,
        )?)
    }
}

/// Pure compiler events are integrated above the backend. The receiving State owns renderer
/// selection and desktop invalidation; no callback capability is passed into device code.
pub(crate) fn install_native_shader_wakeup(
    renderer: &mut NativeRenderer,
    event_loop: &LoopHandle<'static, State>,
) -> anyhow::Result<()> {
    if let Some(source) = renderer.take_gpu_preparation_source() {
        event_loop
            .insert_source(source, |event, _, state| {
                if let calloop::channel::Event::Msg(event) = event {
                    state.on_native_gpu_preparation_event(event);
                }
            })
            .map_err(|error| {
                anyhow::anyhow!("cannot attach native GPU preparation: {}", error.error)
            })?;
    }
    if let Some(source) = renderer.take_shader_completion_source() {
        event_loop
            .insert_source(source, |event, _, state| {
                if let calloop::channel::Event::Msg(completion) = event {
                    state.on_native_shader_completion(completion);
                }
            })
            .map_err(|error| {
                anyhow::anyhow!("cannot attach native shader completion: {}", error.error)
            })?;
    }
    Ok(())
}

pub(crate) trait DevicePort {
    fn initialize_tty(&mut self, device: &mut Tty);
    fn initialize_winit(&mut self, device: &mut Winit);
    fn initialize_headless(&mut self, device: &mut Headless);
    #[cfg(test)]
    fn add_headless_output(&mut self, device: &mut Headless, number: u8, size: (u16, u16));
    fn set_tty_vrr(&mut self, device: &mut Tty, output: &Output, enabled: bool);
    fn update_tty_ignored_nodes(&mut self, device: &mut Tty);
    fn configure_tty_outputs(&mut self, device: &mut Tty);
    fn estimated_vblank(&mut self, output: Output);
}

/// Lifecycle coordinator above the device backend. The reference cannot be recovered
/// through DevicePort, DeviceSetup, Deref, or a callback registration capability.
pub(crate) struct DeviceCoordinator<'a> {
    clarion: &'a mut Clarion,
}

impl<'a> DeviceCoordinator<'a> {
    pub(crate) fn new(clarion: &'a mut Clarion) -> Self {
        Self { clarion }
    }
}

impl DevicePort for DeviceCoordinator<'_> {
    fn initialize_tty(&mut self, device: &mut Tty) {
        tty::initialize(device, self.clarion);
    }
    fn initialize_winit(&mut self, device: &mut Winit) {
        winit::initialize(device, self.clarion);
    }
    fn initialize_headless(&mut self, device: &mut Headless) {
        headless::initialize(device, self.clarion);
    }
    #[cfg(test)]
    fn add_headless_output(&mut self, device: &mut Headless, number: u8, size: (u16, u16)) {
        headless::add_output(device, self.clarion, number, size);
    }
    fn set_tty_vrr(&mut self, device: &mut Tty, output: &Output, enabled: bool) {
        tty::set_vrr(device, self.clarion, output, enabled);
    }
    fn update_tty_ignored_nodes(&mut self, device: &mut Tty) {
        tty::update_ignored_nodes(device, self.clarion);
    }
    fn configure_tty_outputs(&mut self, device: &mut Tty) {
        tty::configure_outputs(device, self.clarion);
    }
    fn estimated_vblank(&mut self, output: Output) {
        tty::estimated_vblank(self.clarion, output);
    }
}

/// GPU creation is device-only; completion registration belongs to the platform.
pub(crate) fn native_renderer_with_wakeup(
    node: smithay::backend::drm::DrmNode,
    config: &crate::backend::DeviceSettings,
    event_loop: &LoopHandle<'static, State>,
) -> Option<NativeRenderer> {
    let mut renderer = Tty::create_vulkan_renderer(node, config)?;
    if let Err(error) = attach_native_renderer_wakeup(&mut renderer, event_loop) {
        warn!("{error:#}; using GLES fallback");
        return None;
    }
    Some(renderer)
}

fn attach_native_renderer_wakeup(
    renderer: &mut NativeRenderer,
    event_loop: &LoopHandle<'static, State>,
) -> anyhow::Result<()> {
    install_native_shader_wakeup(renderer, event_loop)?;
    renderer
        .install_completion_wakeup_with_callbacks(
            event_loop,
            |sample, state| state.clarion.record_gpu_timing(sample),
            State::on_native_gpu_completion,
        )
        .context("error attaching Vulkan completion wakeup")?;
    Ok(())
}
