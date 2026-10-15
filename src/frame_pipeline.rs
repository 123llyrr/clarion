//! Platform frame coordinator and the scene/device boundary.
//!
//! Device renderers receive immutable output metadata and completed render elements.
//! The preparation port exists because client-buffer import needs the selected device
//! renderer. It exposes no desktop, input, layout, or configuration state.

use std::fmt::Display;
use std::time::{Duration, Instant};

use smithay::backend::renderer::element::RenderElementStates;
use smithay::desktop::utils::OutputPresentationFeedback;
use smithay::output::Output;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::RegistrationToken;
use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback;
use smithay::wayland::dmabuf::{DmabufFeedback, DmabufGlobal};
use smithay::wayland::presentation::Refresh;

use crate::backend::tty::SurfaceDmabufFeedback;
use crate::backend::{RenderResult, Tty};
use crate::compositor::{OutputRenderElements, Realand};
use crate::frame_clock::FrameClock;
use crate::render_helpers::debug::draw_damage;
use crate::render_helpers::renderer::RealandRenderer;
use crate::render_helpers::{RenderCtx, RenderTarget};
use crate::utils::{get_monotonic_time, next_presentation_seq};

pub(crate) const HEADLESS_FRAME_INTERVAL: Duration = Duration::from_micros(16_667);

/// Frozen policy relevant to one device submission, captured before scene preparation.
#[derive(Debug, Clone)]
pub(crate) struct FrameRequest {
    pub locked: bool,
    pub variable_refresh: bool,
    pub refresh_interval: Duration,
    pub gpu_timing_tag: Option<crate::render_helpers::vulkan::timing::GpuTimingTag>,
}

/// A renderer-specific scene. Device code can read elements, never edit desktop state.
pub(crate) struct SceneSnapshot<R: RealandRenderer> {
    elements: Vec<OutputRenderElements<R>>,
}

impl<R: RealandRenderer> SceneSnapshot<R> {
    pub fn elements(&self) -> &[OutputRenderElements<R>] {
        &self.elements
    }
}

/// Scene preparation is the sole renderer-dependent desktop operation.
pub(crate) trait ScenePreparation {
    fn prepare_scene<R: RealandRenderer>(&mut self, renderer: &mut R) -> SceneSnapshot<R>;
}

/// Restricted platform operations needed around a device submission.
///
/// No accessor returns Realand or its layout/input state. Protocol feedback must be
/// collected before KMS queues it with the frame; schedule mutations stay here.
pub(crate) trait FrameServices {
    fn request(&self) -> &FrameRequest;
    fn install_completion_wakeup(
        &mut self,
        renderer: &mut crate::render_helpers::vulkan::native::NativeRenderer,
    ) -> anyhow::Result<()>;
    fn recover_vulkan_renderer(
        &mut self,
        node: smithay::backend::drm::DrmNode,
        config: &crate::backend::DeviceSettings,
    ) -> Option<crate::render_helpers::vulkan::native::NativeRenderer>;
    fn active_output_names(&self) -> Vec<String>;
    fn update_scanout(&mut self, states: &RenderElementStates);
    fn send_dmabuf_feedback(
        &mut self,
        feedback: &SurfaceDmabufFeedback,
        states: &RenderElementStates,
    );
    fn take_presentation_feedback(
        &mut self,
        states: &RenderElementStates,
    ) -> OutputPresentationFeedback;
    fn present_immediately(&mut self, states: &RenderElementStates);
    fn set_default_dmabuf_feedback(&mut self, global: &DmabufGlobal, feedback: &DmabufFeedback);
    fn kms_submitted(&mut self);
    fn retry_after_vblank(&mut self, target: Duration);
    fn kms_submission_failed(&mut self, target: Duration, error: &dyn Display);
    fn estimated_vblank(&mut self, target: Duration);
    fn finish_nested_redraw(&mut self) -> bool;
    fn finish_headless_redraw(&mut self);
    fn output_resized(&mut self);
    fn cancel_timer(&mut self, token: RegistrationToken);
    fn arm_nested_retry(&mut self, deadline: Instant) -> Option<RegistrationToken>;
}

pub(crate) trait FramePort: ScenePreparation + FrameServices {}
impl<T: ScenePreparation + FrameServices> FramePort for T {}

/// The adapter lives above the device backends. Its desktop reference is private and
/// cannot be reached through the frame-port bounds used by device rendering code.
pub(crate) struct FrameCoordinator<'a> {
    realand: &'a mut Realand,
    output: Output,
    request: FrameRequest,
}

impl<'a> FrameCoordinator<'a> {
    pub fn new(realand: &'a mut Realand, output: &Output) -> Self {
        let clock = realand.output_state[output].schedule.clock();
        let request = FrameRequest {
            locked: realand.is_locked(),
            variable_refresh: clock.vrr(),
            refresh_interval: clock.refresh_interval().unwrap_or(HEADLESS_FRAME_INTERVAL),
            gpu_timing_tag: realand.output_state[output].schedule.gpu_timing_tag(),
        };
        Self {
            realand,
            output: output.clone(),
            request,
        }
    }
}

impl ScenePreparation for FrameCoordinator<'_> {
    fn prepare_scene<R: RealandRenderer>(&mut self, renderer: &mut R) -> SceneSnapshot<R> {
        let _latency =
            crate::utils::latency::Span::new(crate::utils::latency::Operation::SceneBuild);
        let ctx = RenderCtx {
            renderer,
            target: RenderTarget::Output,
            xray: None,
        };
        let mut elements = self.realand.render_to_vec(ctx, &self.output, true);
        if self.realand.debug_draw_damage {
            let output_state = self.realand.output_state.get_mut(&self.output).unwrap();
            draw_damage(&mut output_state.debug_damage_tracker, &mut elements);
        }
        SceneSnapshot { elements }
    }
}

impl FrameServices for FrameCoordinator<'_> {
    fn cancel_timer(&mut self, token: RegistrationToken) {
        self.realand.event_loop.remove(token);
    }
    fn arm_nested_retry(&mut self, deadline: Instant) -> Option<RegistrationToken> {
        let timer_output = self.output.clone();
        match self.realand.event_loop.insert_source(
            Timer::from_deadline(deadline),
            move |_, _, state| {
                if !state.realand.output_state.contains_key(&timer_output) {
                    return TimeoutAction::Drop;
                }
                if let crate::backend::Backend::Winit(winit) = &mut state.backend {
                    winit.retry_timer_fired();
                    state.realand.queue_redraw(&timer_output);
                }
                TimeoutAction::Drop
            },
        ) {
            Ok(token) => Some(token),
            Err(err) => {
                warn!("error scheduling nested Vulkan presentation retry: {err}");
                None
            }
        }
    }
    fn request(&self) -> &FrameRequest {
        &self.request
    }
    fn install_completion_wakeup(
        &mut self,
        renderer: &mut crate::render_helpers::vulkan::native::NativeRenderer,
    ) -> anyhow::Result<()> {
        crate::platform::device_bridge::install_native_shader_wakeup(
            renderer,
            &self.realand.event_loop,
        )?;
        renderer.install_completion_wakeup_with_callbacks(
            &self.realand.event_loop,
            |sample, state| state.realand.record_gpu_timing(sample),
            crate::compositor::State::on_native_gpu_completion,
        )?;
        Ok(())
    }
    fn recover_vulkan_renderer(
        &mut self,
        node: smithay::backend::drm::DrmNode,
        config: &crate::backend::DeviceSettings,
    ) -> Option<crate::render_helpers::vulkan::native::NativeRenderer> {
        crate::platform::device_bridge::native_renderer_with_wakeup(
            node,
            config,
            &self.realand.event_loop,
        )
    }
    fn active_output_names(&self) -> Vec<String> {
        self.realand
            .global_space
            .outputs()
            .map(Output::name)
            .collect()
    }
    fn update_scanout(&mut self, states: &RenderElementStates) {
        self.realand
            .update_primary_scanout_output(&self.output, states);
    }
    fn send_dmabuf_feedback(
        &mut self,
        feedback: &SurfaceDmabufFeedback,
        states: &RenderElementStates,
    ) {
        self.realand
            .send_dmabuf_feedbacks(&self.output, feedback, states);
    }
    fn take_presentation_feedback(
        &mut self,
        states: &RenderElementStates,
    ) -> OutputPresentationFeedback {
        self.realand
            .take_presentation_feedbacks(&self.output, states)
    }
    fn present_immediately(&mut self, states: &RenderElementStates) {
        let mut feedback = self.take_presentation_feedback(states);
        feedback.presented::<_, smithay::utils::Monotonic>(
            get_monotonic_time(),
            Refresh::Unknown,
            next_presentation_seq(),
            wp_presentation_feedback::Kind::empty(),
        );
    }
    fn set_default_dmabuf_feedback(&mut self, global: &DmabufGlobal, feedback: &DmabufFeedback) {
        self.realand
            .dmabuf_state
            .set_default_feedback(global, feedback);
    }
    fn kms_submitted(&mut self) {
        if let Some(token) = self
            .realand
            .output_state
            .get_mut(&self.output)
            .unwrap()
            .schedule
            .kms_submitted()
        {
            self.realand.event_loop.remove(token);
        }
    }
    fn retry_after_vblank(&mut self, target: Duration) {
        queue_retry_after_estimated_vblank(self.realand, self.output.clone(), target);
    }
    fn kms_submission_failed(&mut self, target: Duration, error: &dyn Display) {
        queue_kms_submission_retry(self.realand, self.output.clone(), target, error);
    }
    fn estimated_vblank(&mut self, target: Duration) {
        queue_estimated_vblank_timer(self.realand, self.output.clone(), target);
    }
    fn finish_nested_redraw(&mut self) -> bool {
        let schedule = &mut self
            .realand
            .output_state
            .get_mut(&self.output)
            .unwrap()
            .schedule;
        schedule.finish_nested_redraw();
        schedule.animations_remain()
    }
    fn output_resized(&mut self) {
        self.realand.ipc_outputs_changed = true;
        self.realand.output_resized(&self.output);
    }
    fn finish_headless_redraw(&mut self) {
        let output_state = self.realand.output_state.get_mut(&self.output).unwrap();
        if let Some(token) = output_state.schedule.finish_headless_redraw() {
            self.realand.event_loop.remove(token);
        }
        if output_state.schedule.animations_remain() {
            let timer_output = self.output.clone();
            match self.realand.event_loop.insert_source(
                Timer::from_duration(HEADLESS_FRAME_INTERVAL),
                move |_, _, state| {
                    if let Some(output_state) = state.realand.output_state.get_mut(&timer_output) {
                        output_state.schedule.headless_timer_fired();
                    }
                    TimeoutAction::Drop
                },
            ) {
                Ok(token) => output_state.schedule.arm_headless_timer(token),
                Err(err) => warn!("error scheduling headless animation: {err}"),
            }
        }
    }
}

/// Retry a failed frame even if its animation has already reached its endpoint.
/// Skipped redraw cleanup clears immediately queued damage, so the retry must be
/// retained independently until the estimated refresh wakes the output.
pub(crate) fn queue_retry_after_estimated_vblank(
    realand: &mut Realand,
    output: Output,
    target_presentation_time: Duration,
) {
    realand
        .output_state
        .get_mut(&output)
        .unwrap()
        .schedule
        .retry_after_estimated_vblank();
    queue_estimated_vblank_timer(realand, output, target_presentation_time);
}

/// Used by both native Vulkan and GLES after a rejected KMS submission.
pub(crate) fn queue_kms_submission_retry(
    realand: &mut Realand,
    output: Output,
    target: Duration,
    error: &(impl std::fmt::Display + ?Sized),
) {
    let now = get_monotonic_time();
    let (deadline, should_warn) = realand
        .output_state
        .get_mut(&output)
        .unwrap()
        .schedule
        .kms_submission_failed(now, target);
    if should_warn {
        warn!(
            output = output.name(),
            retry_delay_ms = deadline.saturating_sub(now).as_secs_f64() * 1000.,
            "error queueing frame to KMS: {error}; retrying with bounded backoff"
        );
    }
    queue_retry_after_estimated_vblank(realand, output, deadline);
}

/// Activation should present promptly even if the previous DRM master rejected frames.
pub(crate) fn reset_kms_submission_backoff(realand: &mut Realand) {
    for state in realand.output_state.values_mut() {
        state.schedule.reset_kms_retry();
    }
}

fn queue_estimated_vblank_timer(
    realand: &mut Realand,
    output: Output,
    target_presentation_time: Duration,
) {
    let output_state = realand.output_state.get_mut(&output).unwrap();
    if output_state.schedule.reuse_estimated_timer() {
        return;
    }

    let duration = estimated_vblank_delay(
        target_presentation_time,
        get_monotonic_time(),
        output_state.schedule.clock(),
    );

    trace!("queueing estimated vblank timer to fire in {duration:?}");

    let timer = Timer::from_duration(duration);
    let token = realand
        .event_loop
        .insert_source(timer, move |_, _, data| {
            Tty::on_estimated_vblank_timer(
                &mut crate::platform::device_bridge::DeviceCoordinator::new(&mut data.realand),
                output.clone(),
            );
            TimeoutAction::Drop
        })
        .unwrap();
    output_state.schedule.arm_estimated_timer(token);
}

pub(crate) fn estimated_vblank_delay(
    target: Duration,
    now: Duration,
    clock: &FrameClock,
) -> Duration {
    if target > now {
        return target - now;
    }

    // A no-damage frame or a failed submission can finish after its predicted vblank.
    // Waiting a whole interval from *now* drifts the next frame callback off the physical
    // refresh grid, adding avoidable latency on fixed-refresh outputs.
    if clock.has_presentation_history() && !clock.vrr() {
        let next = clock.next_presentation_time_at(now);
        if next > now {
            return next - now;
        }
    }

    // With no reliable presentation grid (or after an idle VRR interval), avoid an immediate
    // timer firing while render() is still finishing its frame callbacks.
    clock
        .refresh_interval()
        .unwrap_or(Duration::from_micros(16_667))
}

/// Complete nested frame callback pacing after the render result is known.
pub(crate) fn finish_nested_frame(
    realand: &mut Realand,
    output: &Output,
    result: &RenderResult,
    render_started: Instant,
) {
    let Some(output_state) = realand.output_state.get_mut(output) else {
        return;
    };
    match result {
        RenderResult::Submitted => {
            if let Some(token) = output_state.schedule.nested_submitted() {
                realand.event_loop.remove(token);
            }
        }
        RenderResult::NoDamage => {
            // Empty commits do not perform a host swap/present. Preserve the first
            // deadline across later empty commits so they neither spin nor starve.
            if output_state.schedule.nested_frame_timer().is_some() {
                return;
            }
            let interval = output_state
                .schedule
                .clock()
                .refresh_interval()
                .unwrap_or(Duration::from_micros(16_667));
            // Match nested FIFO pacing, including time already spent rendering.
            let deadline = render_started
                .checked_add(interval)
                .unwrap_or(render_started);
            let timer_output = output.clone();
            let timer = Timer::from_deadline(deadline);
            match realand.event_loop.insert_source(timer, move |_, _, state| {
                let realand = &mut state.realand;
                let Some(output_state) = realand.output_state.get_mut(&timer_output) else {
                    return TimeoutAction::Drop;
                };
                if !output_state.schedule.nested_frame_timer_fired() {
                    return TimeoutAction::Drop;
                }
                realand.send_frame_callbacks(&timer_output);
                if realand.animations_need_redraw(&timer_output) {
                    realand.queue_redraw(&timer_output);
                }
                TimeoutAction::Drop
            }) {
                Ok(token) => output_state.schedule.arm_nested_frame_timer(token),
                Err(err) => warn!("error scheduling nested frame callbacks: {err}"),
            }
            return;
        }
        RenderResult::Skipped | RenderResult::GpuDeferred => return,
    }
    realand.send_frame_callbacks(output);
}
