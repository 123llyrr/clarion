//! Headless backend for tests.
//!
//! The Vulkan mode uses a real offscreen target and advertises DMA-BUF formats.
//! The GLES test mode keeps its lightweight callback-only rendering behavior.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use anyhow::Context as _;
use clarion_config::Config;
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::allocator::Fourcc;
use smithay::backend::egl::native::EGLSurfacelessDisplay;
use smithay::backend::egl::{EGLContext, EGLDisplay};
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::RenderElementStates;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::{Bind, ImportDma, Offscreen};
use smithay::output::Output;
use smithay::utils::{Buffer, Physical, Size};
use smithay::wayland::dmabuf::DmabufGlobal;

use super::{AnyRenderer, IpcOutputMap, RenderResult, RendererBackendType};
#[cfg(test)]
use crate::output_schedule::RedrawState;
use crate::platform::device_bridge::DevicePort;
use crate::render_helpers::vulkan::{native::NativeRenderer, VulkanRenderer, VulkanTexture};
use crate::render_helpers::{resources, shaders};

#[cfg(test)]
use crate::frame_pipeline::HEADLESS_FRAME_INTERVAL;

pub(crate) enum HeadlessRenderer {
    Gles(Box<GlesRenderer>),
    Native(Box<NativeRenderer>),
}

pub(crate) struct NativeTarget {
    pub(crate) buffer: VulkanTexture,
    pub(crate) damage_tracker: OutputDamageTracker,
    pub(crate) size: Size<i32, Physical>,
    pub(crate) valid: bool,
}

pub struct Headless {
    pub(crate) config: Rc<super::DeviceSettings>,
    pub(crate) requested_renderer: RendererBackendType,
    pub(crate) renderer: Option<HeadlessRenderer>,
    pub(crate) native_targets: HashMap<String, NativeTarget>,
    pub(crate) dmabuf_global: Option<DmabufGlobal>,
    pub(crate) ipc_outputs: Arc<Mutex<IpcOutputMap>>,
}

impl Headless {
    pub fn new(config: Rc<RefCell<Config>>, requested_renderer: RendererBackendType) -> Self {
        let config = Rc::new(super::DeviceSettings::from(&*config.borrow()));
        Self {
            config,
            requested_renderer,
            renderer: None,
            native_targets: HashMap::new(),
            dmabuf_global: None,
            ipc_outputs: Default::default(),
        }
    }

    pub(crate) fn init(&mut self, port: &mut impl DevicePort) {
        port.initialize_headless(self);
    }
    #[cfg(test)]
    pub(crate) fn add_output(&mut self, port: &mut impl DevicePort, number: u8, size: (u16, u16)) {
        port.add_headless_output(self, number, size);
    }

    pub fn add_renderer(&mut self) -> anyhow::Result<()> {
        if self.renderer.is_some() {
            error!("add_renderer: renderer must not already exist");
            return Ok(());
        }

        self.renderer = Some(match self.requested_renderer {
            RendererBackendType::Gles => {
                let mut renderer = unsafe {
                    let display = EGLDisplay::new(EGLSurfacelessDisplay)
                        .context("error creating EGL display")?;
                    let context =
                        EGLContext::new(&display).context("error creating EGL context")?;
                    GlesRenderer::new(context).context("error creating GLES renderer")?
                };
                resources::init(&mut renderer);
                shaders::init(&mut renderer);
                HeadlessRenderer::Gles(Box::new(renderer))
            }
            RendererBackendType::Vulkan => {
                let mut renderer = VulkanRenderer::new();
                renderer
                    .init_from_drm_node(None)
                    .context("error initializing headless Vulkan device")?;
                let mut renderer = NativeRenderer::new(renderer)
                    .context("error creating headless native Vulkan renderer")?;
                shaders::init_native(&mut renderer, &self.config.animations);
                renderer
                    .prepare_output_format(Fourcc::Argb8888)
                    .context("error preparing headless Vulkan output format")?;
                info!("headless native Vulkan renderer initialized");
                HeadlessRenderer::Native(Box::new(renderer))
            }
        });
        Ok(())
    }

    pub fn seat_name(&self) -> String {
        "headless".to_owned()
    }

    pub fn with_primary_renderer<T>(
        &mut self,
        f: impl FnOnce(&mut GlesRenderer) -> T,
    ) -> Option<T> {
        match self.renderer.as_mut()? {
            HeadlessRenderer::Gles(renderer) => Some(f(renderer)),
            HeadlessRenderer::Native(_) => None,
        }
    }

    pub fn with_renderer<T>(&mut self, f: impl FnOnce(AnyRenderer<'_>) -> T) -> Option<T> {
        match self.renderer.as_mut()? {
            HeadlessRenderer::Gles(renderer) => Some(f(AnyRenderer::Gles(renderer))),
            HeadlessRenderer::Native(renderer) => Some(f(AnyRenderer::Native(renderer))),
        }
    }

    pub fn renderer_backend(&self) -> RendererBackendType {
        match self.renderer.as_ref() {
            Some(HeadlessRenderer::Native(_)) => RendererBackendType::Vulkan,
            Some(HeadlessRenderer::Gles(_)) | None => RendererBackendType::Gles,
        }
    }

    pub(crate) fn render(
        &mut self,
        port: &mut impl crate::frame_pipeline::FramePort,
        output: &Output,
    ) -> RenderResult {
        let (states, rv) = if matches!(self.renderer, Some(HeadlessRenderer::Native(_))) {
            match self.render_native(port, output) {
                Ok(result) => result,
                Err(err) => {
                    warn!(
                        "headless Vulkan rendering failed on {}: {err:#}",
                        output.name()
                    );
                    self.native_targets.remove(&output.name());
                    return RenderResult::Skipped;
                }
            }
        } else {
            // Preserve the lightweight GLES test backend's existing callback behavior.
            (RenderElementStates::default(), RenderResult::Submitted)
        };

        if rv.is_unpresented() {
            return rv;
        }
        port.present_immediately(&states);
        port.finish_headless_redraw();

        rv
    }

    fn render_native(
        &mut self,
        port: &mut impl crate::frame_pipeline::FramePort,
        output: &Output,
    ) -> anyhow::Result<(RenderElementStates, RenderResult)> {
        let active_outputs = port.active_output_names();
        self.native_targets
            .retain(|name, _| active_outputs.contains(name));
        let Some(HeadlessRenderer::Native(renderer)) = self.renderer.as_mut() else {
            unreachable!();
        };
        let renderer = renderer.as_mut();
        // Test fixtures can add the renderer after init; also retry a failed
        // setup before accepting work that could release idle client buffers.
        port.install_completion_wakeup(renderer)?;
        let mode = output
            .current_mode()
            .context("headless output has no mode")?;
        let key = output.name();
        if self
            .native_targets
            .get(&key)
            .is_none_or(|target| target.size != mode.size)
        {
            let buffer = renderer
                .create_buffer(
                    Fourcc::Argb8888,
                    Size::<i32, Buffer>::from((mode.size.w, mode.size.h)),
                )
                .context("error allocating headless Vulkan target")?;
            self.native_targets.insert(
                key.clone(),
                NativeTarget {
                    buffer,
                    damage_tracker: OutputDamageTracker::from_output(output),
                    size: mode.size,
                    valid: false,
                },
            );
        }

        renderer.begin_scene();
        let scene = port.prepare_scene(renderer);
        if renderer.scene_was_deferred() {
            return Ok((RenderElementStates::default(), RenderResult::GpuDeferred));
        }
        let elements = scene.elements();

        let target = self.native_targets.get_mut(&key).unwrap();
        let mut framebuffer = renderer
            .bind(&mut target.buffer)
            .context("error binding headless Vulkan target")?;
        renderer.set_completion_export_required(true);
        if let (Some(tag), Some(extent)) = (
            port.request().gpu_timing_tag,
            Some((mode.size.w as u32, mode.size.h as u32)),
        ) {
            renderer.arm_output_timing(tag, extent);
        }
        let result = target
            .damage_tracker
            .render_output(
                renderer,
                &mut framebuffer,
                usize::from(target.valid),
                elements,
                [0.; 4],
            )
            .context("error drawing headless Vulkan frame");
        renderer.clear_output_timing();
        renderer.set_completion_export_required(false);
        let result = match result {
            Err(error)
                if crate::render_helpers::vulkan::native::NativeError::is_backpressure(
                    error.as_ref(),
                ) =>
            {
                return Ok((RenderElementStates::default(), RenderResult::GpuDeferred));
            }
            result => result?,
        };
        crate::render_helpers::sync::wait_for_completion(&result.sync)
            .context("headless Vulkan frame completion interrupted")?;
        let rv = if result.damage.is_some() {
            RenderResult::Submitted
        } else {
            RenderResult::NoDamage
        };
        target.valid = true;
        port.update_scanout(&result.states);
        Ok((result.states, rv))
    }

    pub fn import_dmabuf(&mut self, dmabuf: &Dmabuf) -> bool {
        let imported = match self.renderer.as_mut() {
            Some(HeadlessRenderer::Gles(renderer)) => renderer
                .import_dmabuf(dmabuf, None)
                .map(|_| ())
                .map_err(|err| format!("{err:?}")),
            Some(HeadlessRenderer::Native(renderer)) => renderer
                .import_dmabuf(dmabuf, None)
                .map(|_| ())
                .map_err(|err| format!("{err:?}")),
            None => return false,
        };
        if let Err(err) = imported {
            debug!("error importing headless dmabuf: {err:?}");
            false
        } else {
            true
        }
    }

    pub fn ipc_outputs(&self) -> Arc<Mutex<IpcOutputMap>> {
        self.ipc_outputs.clone()
    }
}

impl Default for Headless {
    fn default() -> Self {
        Self::new(
            Rc::new(RefCell::new(Config::default())),
            RendererBackendType::Gles,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Backend;
    use crate::compositor::State;
    use smithay::reexports::calloop::EventLoop;
    use smithay::reexports::wayland_server::Display;
    use std::time::Duration;

    #[test]
    fn device_settings_are_isolated_until_an_explicit_snapshot_publication() {
        let shared = Rc::new(RefCell::new(Config::default()));
        let mut backend =
            Backend::Headless(Headless::new(shared.clone(), RendererBackendType::Gles));
        shared.borrow_mut().debug.disable_10bit_output = true;
        shared.borrow_mut().animations.window_open.custom_shader = Some("new shader".into());
        assert!(!backend.headless().config.debug.disable_10bit_output);
        assert!(backend
            .headless()
            .config
            .animations
            .window_open
            .custom_shader
            .is_none());

        backend.update_config_snapshot(&shared.borrow());
        assert!(backend.headless().config.debug.disable_10bit_output);
        assert_eq!(
            backend
                .headless()
                .config
                .animations
                .window_open
                .custom_shader
                .as_deref(),
            Some("new shader")
        );
        shared.borrow_mut().debug.disable_10bit_output = false;
        assert!(backend.headless().config.debug.disable_10bit_output);
    }

    #[test]
    fn config_reload_and_temporary_output_edits_publish_device_settings() {
        let event_loop = EventLoop::<State>::try_new().unwrap();
        let mut state = State::new(
            Config::default(),
            event_loop.handle(),
            event_loop.get_signal(),
            Display::new().unwrap(),
            true,
            false,
            false,
            RendererBackendType::Gles,
        )
        .unwrap();
        let mut config = Config::default();
        config.debug.disable_10bit_output = true;
        config.animations.window_open.custom_shader = Some("latest shader".into());
        config.outputs.0.push(clarion_config::Output {
            name: "headless-1".into(),
            off: true,
            ..Default::default()
        });
        state.reload_config(Ok(config));
        assert!(state.backend.headless().config.debug.disable_10bit_output);
        assert_eq!(
            state
                .backend
                .headless()
                .config
                .animations
                .window_open
                .custom_shader
                .as_deref(),
            Some("latest shader")
        );
        assert!(state.backend.headless().config.outputs.0[0].off);

        state.apply_transient_output_config("headless-1", clarion_ipc::OutputAction::On);
        assert!(!state.backend.headless().config.outputs.0[0].off);
        // Device snapshots do not retain ambient access to later desktop writes.
        state.clarion.config.borrow_mut().outputs.0[0].off = true;
        assert!(!state.backend.headless().config.outputs.0[0].off);
    }

    #[test]
    #[ignore = "requires a Vulkan-capable GPU"]
    fn native_vulkan_renders_headless_output() {
        let mut event_loop = EventLoop::<State>::try_new().unwrap();
        let mut state = State::new(
            Config::default(),
            event_loop.handle(),
            event_loop.get_signal(),
            Display::new().unwrap(),
            true,
            false,
            false,
            RendererBackendType::Vulkan,
        )
        .unwrap();

        // State installed the real completion sources. Dispatch those sources
        // rather than synchronously compiling pipelines in the test's renderer.
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            let ready = state
                .backend
                .with_renderer(|renderer| {
                    let crate::render_helpers::renderer::AnyRenderer::Native(renderer) = renderer
                    else {
                        unreachable!()
                    };
                    match renderer.ensure_target_format_ready(ash::vk::Format::B8G8R8A8_UNORM) {
                        Ok(()) => true,
                        Err(error) if error.is_busy() => false,
                        Err(error) => panic!("native fixture preparation failed: {error}"),
                    }
                })
                .unwrap();
            if ready {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "actual GPU owner source did not prepare the fixture"
            );
            event_loop
                .dispatch(Duration::from_millis(5), &mut state)
                .unwrap();
        }

        let State { backend, clarion } = &mut state;
        let Backend::Headless(headless) = backend else {
            unreachable!();
        };
        assert_eq!(headless.renderer_backend(), RendererBackendType::Vulkan);
        assert_eq!(
            headless.with_renderer(|renderer| renderer.backend_type()),
            Some(RendererBackendType::Vulkan)
        );
        let Some(HeadlessRenderer::Native(renderer)) = headless.renderer.as_ref() else {
            unreachable!("test requires the native Vulkan renderer");
        };
        eprintln!(
            "HARDWARE: {} (headless loader/ICD selection)",
            renderer.vulkan().info.device_name
        );
        assert!(headless.dmabuf_global.is_some());
        headless.add_output(
            &mut crate::platform::device_bridge::DeviceCoordinator::new(clarion),
            1,
            (64, 48),
        );
        let output = clarion.global_space.outputs().next().unwrap().clone();
        clarion.queue_redraw(&output);

        assert!(matches!(
            headless.render(
                &mut crate::frame_pipeline::FrameCoordinator::new(clarion, &output),
                &output
            ),
            RenderResult::Submitted
        ));
        assert!(headless.native_targets[&output.name()].valid);
        clarion.queue_redraw(&output);
        assert!(matches!(
            headless.render(
                &mut crate::frame_pipeline::FrameCoordinator::new(clarion, &output),
                &output
            ),
            RenderResult::NoDamage
        ));

        clarion.remove_output(&output);
        headless.add_output(
            &mut crate::platform::device_bridge::DeviceCoordinator::new(clarion),
            2,
            (32, 32),
        );
        assert!(headless.native_targets.is_empty());
    }

    #[test]
    fn unfinished_animation_schedules_next_headless_redraw_without_spinning() {
        let mut event_loop = EventLoop::<State>::try_new().unwrap();
        let handle = event_loop.handle();
        let display = Display::new().unwrap();
        let mut state = State::new(
            Config::default(),
            handle,
            event_loop.get_signal(),
            display,
            true,
            false,
            false,
            RendererBackendType::Gles,
        )
        .unwrap();

        let output = {
            let State { backend, clarion } = &mut state;
            backend.headless().add_output(
                &mut crate::platform::device_bridge::DeviceCoordinator::new(clarion),
                1,
                (64, 48),
            );
            let output = clarion.global_space.outputs().next().unwrap().clone();
            clarion
                .output_state
                .get_mut(&output)
                .unwrap()
                .schedule
                .set_animations_remain(true);
            clarion.queue_redraw(&output);

            assert!(matches!(
                backend.headless().render(
                    &mut crate::frame_pipeline::FrameCoordinator::new(clarion, &output),
                    &output
                ),
                RenderResult::Submitted
            ));
            assert!(matches!(
                clarion.output_state[&output].schedule.redraw_state(),
                RedrawState::WaitingForEstimatedVBlank(_)
            ));
            output
        };

        event_loop
            .dispatch(HEADLESS_FRAME_INTERVAL, &mut state)
            .unwrap();
        assert!(matches!(
            state.clarion.output_state[&output].schedule.redraw_state(),
            RedrawState::Queued
        ));
    }
}
