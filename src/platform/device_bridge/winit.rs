use anyhow::Context as _;
use smithay::backend::egl::EGLDevice;
use smithay::backend::renderer::{ImportDma, ImportEgl};
use smithay::backend::winit::WinitEvent;
use smithay::output::Output;
use smithay::utils::{Physical, Size};
use smithay::wayland::dmabuf::DmabufFeedbackBuilder;

use crate::compositor::{Realand, State};
use crate::render_helpers::{resources, shaders};

use crate::backend::winit::native_window::{NativeDevice, NativeWindowEvent};
use crate::backend::winit::*;

impl crate::input::backend_ext::RealandInputDevice for NativeDevice {
    fn output(&self, _: &State) -> Option<Output> {
        None
    }
}

impl Winit {
    fn platform_init(&mut self, realand: &mut Realand) {
        match &mut self.backend {
            WinitBackend::Gles(backend) => {
                let renderer = backend.renderer();
                if let Err(err) = renderer.bind_wl_display(&realand.display_handle) {
                    trace!("error binding legacy EGL to wl_display: {err}");
                } else {
                    debug!("bound legacy EGL to wl_display");
                }
                resources::init(renderer);
                shaders::init(renderer);
                let config = self.config.as_ref();
                if let Some(src) = config.animations.window_resize.custom_shader.as_deref() {
                    shaders::set_custom_resize_program(renderer, Some(src));
                }
                let close_src = shaders::resolve_close_shader(
                    config.animations.window_close.custom_shader.as_deref(),
                    config.animations.window_close.effect,
                );
                if let Some(src) = close_src {
                    shaders::set_custom_close_program(renderer, Some(src));
                }
                if let Some(src) = config.animations.window_open.custom_shader.as_deref() {
                    shaders::set_custom_open_program(renderer, Some(src));
                }
            }
            WinitBackend::Native(native) => {
                shaders::init_native(&mut native.renderer, &self.config.animations);
            }
        }

        realand.update_shaders();

        self.platform_create_dmabuf_global(realand);

        realand.add_output(self.output.clone(), None, false, false);
    }

    fn platform_create_dmabuf_global(&mut self, realand: &mut Realand) {
        let WinitBackend::Gles(backend) = &mut self.backend else {
            let WinitBackend::Native(native) = &mut self.backend else {
                unreachable!()
            };
            self.dmabuf_global =
                Some(realand.dmabuf_state.create_global::<State>(
                    &realand.display_handle,
                    native.renderer.dmabuf_formats(),
                ));
            return;
        };
        let renderer = backend.renderer();

        let default_feedback = || {
            let display = renderer.egl_context().display();
            let device =
                EGLDevice::device_for_display(display).context("error getting EGL device")?;
            let node = device
                .try_get_render_node()
                .context("error getting EGL device render node")?
                .context("failed to query EGL device render node")?;

            let primary_formats = renderer.dmabuf_formats();
            DmabufFeedbackBuilder::new(node.dev_id(), primary_formats)
                .build()
                .context("error building dmabuf feedback")
        };

        // Fallback to dmabuf v3 if we failed to build feedback.
        let dmabuf_global = match default_feedback() {
            Ok(feedback) => realand
                .dmabuf_state
                .create_global_with_default_feedback::<State>(&realand.display_handle, &feedback),
            Err(err) => {
                debug!("failed building default dmabuf feedback, falling back to v3: {err:?}");
                let primary_formats = renderer.dmabuf_formats();
                realand
                    .dmabuf_state
                    .create_global::<State>(&realand.display_handle, primary_formats)
            }
        };
        assert!(self.dmabuf_global.replace(dmabuf_global).is_none());
    }
}

fn handle_resize(state: &mut State, size: Size<i32, Physical>) {
    if size.w <= 0 || size.h <= 0 {
        return;
    }
    let winit = state.backend.winit();
    let mut size = size;
    if let WinitBackend::Native(native) = &mut winit.backend {
        if native.pending.is_some() {
            // The old target still owns a composition/acquire operation. Let
            // presentation consume or reject it before a resize replaces it.
            let output = winit.output.clone();
            let mut coordinator =
                crate::frame_pipeline::FrameCoordinator::new(&mut state.realand, &output);
            winit.defer_native_attempt(&mut coordinator, false);
            return;
        }
        if native.wsi.requested_size() != Some((size.w as u32, size.h as u32)) {
            match native.wsi.resize(size.w as u32, size.h as u32) {
                Ok(true) => (),
                Ok(false) => {
                    let output = winit.output.clone();
                    let mut coordinator =
                        crate::frame_pipeline::FrameCoordinator::new(&mut state.realand, &output);
                    winit.defer_native_attempt(&mut coordinator, false);
                    return;
                }
                Err(err) => {
                    warn!("error resizing nested Vulkan swapchain: {err:#}");
                    let output = winit.output.clone();
                    let mut coordinator =
                        crate::frame_pipeline::FrameCoordinator::new(&mut state.realand, &output);
                    winit.defer_native_attempt(&mut coordinator, true);
                    return;
                }
            }
            native.target = None;
            native.target_valid = false;
        }
        if let Some(extent) = native.wsi.extent() {
            size = (extent.width as i32, extent.height as i32).into();
        }
    }
    let output = winit.output.clone();
    let mut coordinator = crate::frame_pipeline::FrameCoordinator::new(&mut state.realand, &output);
    winit.update_output_size(&mut coordinator, size);
}

pub(super) fn initialize(device: &mut Winit, realand: &mut Realand) {
    device.platform_init(realand);
}
pub(super) fn on_gles_event(state: &mut State, event: WinitEvent) {
    match event {
        WinitEvent::Resized { size, .. } => handle_resize(state, size),
        WinitEvent::Input(event) => state.process_input_event(event),
        WinitEvent::Focus(_) => (),
        WinitEvent::Redraw => state.realand.queue_redraw(&state.backend.winit().output),
        WinitEvent::CloseRequested => state.realand.stop_signal.stop(),
    }
}
pub(super) fn on_native_event(state: &mut State, event: NativeWindowEvent) {
    match event {
        NativeWindowEvent::Resized(size) => handle_resize(state, size),
        NativeWindowEvent::Input(event) => state.process_input_event(event),
        NativeWindowEvent::Focus => (),
        NativeWindowEvent::Redraw => state.realand.queue_redraw(&state.backend.winit().output),
        NativeWindowEvent::CloseRequested => state.realand.stop_signal.stop(),
    }
}
