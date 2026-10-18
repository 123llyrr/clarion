//! Owner-side publication, timeout and retry policy for asynchronous Vulkan recovery.
//! Driver preparation is confined to the bounded backend worker; this adapter alone
//! registers wakeups and invalidates renderer-dependent scene resources.

use std::time::Duration;

use smithay::backend::renderer::ImportDma;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::wayland::dmabuf::DmabufFeedbackBuilder;

use crate::backend::tty::{is_primary_gpu_node, surface_dmabuf_feedback};
use crate::compositor::State;

fn fallback_policy(tty: &crate::backend::tty::Tty) -> &'static str {
    if tty.color_management_enabled {
        "preserving the last color-correct scanout; session restart may be required"
    } else {
        "using GLES fallback"
    }
}

/// Event-side publication of a prepared device. This callback never creates a
/// Vulkan instance/device, compiles a pipeline, queries formats or waits on the GPU.
pub(super) fn on_completion(state: &mut State) {
    let crate::backend::Backend::Tty(tty) = &mut state.backend else {
        return;
    };
    let Some(generation) = tty.vulkan_recovery_generation else {
        return;
    };
    if !tty
        .devices
        .keys()
        .copied()
        .any(|node| is_primary_gpu_node(node, tty.primary_node, tty.primary_render_node))
    {
        tty.vulkan_recovery.invalidate();
        tty.vulkan_recovery_generation = None;
        if let Some(timer) = tty.vulkan_recovery_watchdog.take() {
            state.clarion.event_loop.remove(timer);
        }
        return;
    }
    let Some(completion) = tty.vulkan_recovery.take_completed() else {
        if tty.vulkan_recovery_watchdog.is_none() {
            match state.clarion.event_loop.insert_source(
                Timer::from_duration(Duration::from_secs(15)),
                move |_, _, state| {
                    let crate::backend::Backend::Tty(tty) = &mut state.backend else {
                        return TimeoutAction::Drop;
                    };
                    if tty.vulkan_recovery_generation == Some(generation) {
                        // Do not join a hung driver or free its in-flight objects.
                        // Late success is stale and is destroyed by the same worker.
                        tty.vulkan_recovery_watchdog = None;
                        tty.vulkan_recovery_generation = None;
                        tty.vulkan_recovery.invalidate();
                        warn!("Vulkan device recovery timed out; {}", fallback_policy(tty));
                        state.clarion.on_native_renderer_replaced();
                    }
                    TimeoutAction::Drop
                },
            ) {
                Ok(token) => tty.vulkan_recovery_watchdog = Some(token),
                Err(error) => {
                    warn!(
                        "cannot attach Vulkan recovery watchdog: {error}; {}",
                        fallback_policy(tty)
                    );
                    tty.vulkan_recovery.invalidate();
                    tty.vulkan_recovery_generation = None;
                    state.clarion.on_native_renderer_replaced();
                }
            }
        }
        return;
    };
    debug_assert_eq!(completion.generation, generation);
    if let Some(timer) = tty.vulkan_recovery_watchdog.take() {
        state.clarion.event_loop.remove(timer);
    }
    match completion.result {
        Ok(prepared) => {
            let anchor = prepared.device_anchor();
            let mut native =
                crate::render_helpers::vulkan::native::NativeRenderer::from_prepared(prepared);
            if let Err(error) =
                super::attach_native_renderer_wakeup(&mut native, &state.clarion.event_loop)
            {
                // request() reserves this emergency slot before detaching the old
                // renderer. Keep final context teardown off the owner on failure too.
                assert!(
                    tty.vulkan_recovery.retire(anchor).is_ok(),
                    "reserved Vulkan retirement slot unavailable"
                );
                warn!(
                    "cannot attach recovered Vulkan renderer: {error:#}; {}",
                    fallback_policy(tty)
                );
                tty.vulkan_recovery_generation = None;
                state.clarion.on_native_renderer_replaced();
                return;
            }
            crate::render_helpers::shaders::init_native(&mut native, &tty.config.animations);
            let primary_formats = native.dmabuf_formats();
            for (node, device) in &mut tty.devices {
                for surface in device
                    .surfaces
                    .values_mut()
                    .filter(|surface| surface.native_swapchain)
                {
                    if let Err(error) = native.prepare_output_format(surface.compositor.format()) {
                        warn!("cannot enqueue recovered Vulkan output pipelines: {error}");
                    }
                    surface.frame_epoch = surface.frame_epoch.wrapping_add(1);
                    surface.compositor.reset_buffer_ages();
                    match surface_dmabuf_feedback(
                        &surface.compositor,
                        primary_formats.clone(),
                        tty.primary_render_node,
                        device.render_node,
                        tty.primary_node,
                        *node,
                    ) {
                        Ok(feedback) => surface.dmabuf_feedback = Some(feedback),
                        Err(error) => {
                            warn!("cannot refresh recovered Vulkan output feedback: {error:#}")
                        }
                    }
                }
            }
            if let Some(global) = &tty.dmabuf_global {
                match DmabufFeedbackBuilder::new(tty.primary_render_node.dev_id(), primary_formats)
                    .build()
                {
                    Ok(feedback) => state
                        .clarion
                        .dmabuf_state
                        .set_default_feedback(global, &feedback),
                    Err(error) => {
                        warn!("cannot refresh recovered Vulkan default feedback: {error:#}")
                    }
                }
            }
            tty.native_renderer = Some(native);
            tty.vulkan_recovery_generation = None;
            tty.vulkan_recovery_attempt = 0;
            tty.vulkan_recovery_retry_after = Duration::ZERO;
            state.clarion.on_native_renderer_replaced();
            info!("Vulkan device recovered asynchronously; owner caches published for generation {generation}");
        }
        Err(error) => {
            warn!(
                "Vulkan recovery attempt {} failed: {error}",
                tty.vulkan_recovery_attempt
            );
            if tty.vulkan_recovery_attempt >= 3 {
                warn!("Vulkan recovery exhausted; {}", fallback_policy(tty));
                tty.vulkan_recovery_generation = None;
                state.clarion.on_native_renderer_replaced();
                return;
            }
            let delay = Duration::from_millis(250 * u64::from(tty.vulkan_recovery_attempt));
            match state.clarion.event_loop.insert_source(
                Timer::from_duration(delay),
                move |_, _, state| {
                    let crate::backend::Backend::Tty(tty) = &mut state.backend else {
                        return TimeoutAction::Drop;
                    };
                    if tty.vulkan_recovery_generation != Some(generation) {
                        return TimeoutAction::Drop;
                    }
                    match tty.vulkan_recovery.request(tty.primary_render_node, None) {
                        Ok(next) => {
                            tty.vulkan_recovery_generation = Some(next);
                            tty.vulkan_recovery_attempt += 1;
                        }
                        Err(_) => {
                            tty.vulkan_recovery_generation = None;
                            warn!(
                                "cannot queue Vulkan recovery retry; {}",
                                fallback_policy(tty)
                            );
                            state.clarion.on_native_renderer_replaced();
                        }
                    }
                    TimeoutAction::Drop
                },
            ) {
                Ok(_) => (),
                Err(error) => {
                    warn!(
                        "cannot attach Vulkan recovery retry: {error}; {}",
                        fallback_policy(tty)
                    );
                    tty.vulkan_recovery_generation = None;
                    state.clarion.on_native_renderer_replaced();
                }
            }
        }
    }
}
