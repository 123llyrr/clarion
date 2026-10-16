//! Capture submission stays on the compositor thread. Vulkan transfers may
//! finish on a worker; GLES PBOs are polled by their owning EGL event loop.

use anyhow::{ensure, Context};
use calloop::timer::{TimeoutAction, Timer};
use calloop::LoopHandle;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::{ExportMem, Texture};
use smithay::output::Output;
use smithay::utils::{Buffer, Rectangle};
use std::time::Duration;

use crate::compositor::State;
use crate::render_helpers::renderer::ClarionRenderer;
use crate::render_helpers::vulkan::lifecycle;
use crate::render_helpers::vulkan::native::{NativeRenderer, PendingNativeReadback};

use super::capture::CaptureStamp;
use super::gles_readback::GlesReadbackPoll;
use super::gles_readback::PendingGlesReadback;

#[derive(Debug)]
pub(crate) enum PendingPixels {
    Native(PendingNativeReadback),
    Gles(PendingGlesReadback),
    Ready(Vec<u8>),
}

/// Completion is an event-loop capability. Workers own only the device transfer
/// and a bounded result channel, never desktop state or a protocol callback.
struct ReadbackCompletion<F> {
    scene: CaptureStamp,
    output: Option<Output>,
    callback: F,
}

impl<F: FnOnce(&mut State, Result<Vec<u8>, String>)> ReadbackCompletion<F> {
    fn is_current(&self, state: &State) -> bool {
        self.scene.is_current()
            && self
                .output
                .as_ref()
                .is_none_or(|output| state.clarion.output_state.contains_key(output))
    }

    fn deliver(self, state: &mut State, result: Result<Vec<u8>, String>) {
        let result = validate_completion(self.is_current(state), result);
        (self.callback)(state, result);
    }
}

fn validate_completion<T>(current: bool, result: Result<T, String>) -> Result<T, String> {
    if current {
        result
    } else {
        Err("capture scene or output was invalidated".into())
    }
}

/// Register the one completion route for native, deferred GLES and ready pixels.
/// GPU waits remain on a tracked worker; EGL polling stays with its owner.
pub(crate) fn enqueue(
    event_loop: &LoopHandle<'static, State>,
    scene: CaptureStamp,
    output: Option<Output>,
    job: PendingPixels,
    on_done: impl FnOnce(&mut State, Result<Vec<u8>, String>) + 'static,
) -> anyhow::Result<()> {
    let completion = ReadbackCompletion {
        scene,
        output,
        callback: on_done,
    };
    match job {
        PendingPixels::Native(job) => {
            let (tx, rx) = calloop::channel::sync_channel(1);
            let mut completion = Some(completion);
            event_loop
                .insert_source(rx, move |event, _, state| {
                    let result = match event {
                        calloop::channel::Event::Msg(result) => result,
                        calloop::channel::Event::Closed => {
                            Err("Vulkan capture worker closed without completion".into())
                        }
                    };
                    if let Some(completion) = completion.take() {
                        completion.deliver(state, result);
                    }
                })
                .map_err(|error| {
                    anyhow::anyhow!(
                        "error registering Vulkan readback completion: {}",
                        error.error
                    )
                })?;
            lifecycle::spawn_task("capture-readback", move || {
                let result = job.finish().map_err(|error| error.to_string());
                let _ = tx.send(result);
            })
            .context("error starting Vulkan readback")?;
        }
        PendingPixels::Gles(mut job) => {
            let output = completion
                .output
                .clone()
                .context("GLES readback needs its original output")?;
            let mut completion = Some(completion);
            event_loop
                .insert_source(Timer::from_duration(Duration::ZERO), move |_, _, state| {
                    if !completion
                        .as_ref()
                        .is_some_and(|completion| completion.is_current(state))
                    {
                        if let Some(completion) = completion.take() {
                            completion.deliver(state, Err("GLES capture was invalidated".into()));
                        }
                        return TimeoutAction::Drop;
                    }
                    let result = state
                        .backend
                        .with_renderer_for_output(Some(&output), |renderer| match renderer {
                            crate::render_helpers::renderer::AnyRenderer::Gles(gles) => {
                                job.poll(gles)
                            }
                            _ => Err(anyhow::anyhow!("GLES capture renderer was replaced")),
                        })
                        .unwrap_or_else(|| Err(anyhow::anyhow!("GLES capture output disappeared")));
                    match result {
                        Ok(GlesReadbackPoll::Pending) => {
                            TimeoutAction::ToDuration(Duration::from_millis(2))
                        }
                        result => {
                            if let Some(completion) = completion.take() {
                                let result = match result {
                                    Ok(GlesReadbackPoll::Ready(pixels)) => Ok(pixels),
                                    Err(error) => Err(error.to_string()),
                                    Ok(GlesReadbackPoll::Pending) => unreachable!(),
                                };
                                completion.deliver(state, result);
                            }
                            TimeoutAction::Drop
                        }
                    }
                })
                .map_err(|error| {
                    anyhow::anyhow!("error registering GLES readback: {}", error.error)
                })?;
        }
        PendingPixels::Ready(pixels) => {
            event_loop.insert_idle(move |state| completion.deliver(state, Ok(pixels)));
        }
    }
    Ok(())
}

pub(crate) trait DeferredCaptureRenderer: ClarionRenderer {
    fn enqueue_pixels(
        &mut self,
        texture: &Self::TextureId,
        region: Rectangle<i32, Buffer>,
        format: Fourcc,
    ) -> anyhow::Result<PendingPixels>;
}

impl DeferredCaptureRenderer for NativeRenderer {
    fn enqueue_pixels(
        &mut self,
        texture: &Self::TextureId,
        region: Rectangle<i32, Buffer>,
        format: Fourcc,
    ) -> anyhow::Result<PendingPixels> {
        Ok(PendingPixels::Native(
            self.enqueue_readback(texture, region, format)
                .context("submitting Vulkan capture readback")?,
        ))
    }
}

impl DeferredCaptureRenderer for GlesRenderer {
    fn enqueue_pixels(
        &mut self,
        texture: &Self::TextureId,
        region: Rectangle<i32, Buffer>,
        format: Fourcc,
    ) -> anyhow::Result<PendingPixels> {
        if PendingGlesReadback::supports(self) {
            return Ok(PendingPixels::Gles(PendingGlesReadback::begin(
                self, texture, region, format,
            )?));
        }
        validate_fallback_capture(region, texture.size(), format)?;
        warn!("GLES capture fences unavailable; using synchronous readback compatibility path");
        let mapping = self
            .copy_texture(texture, region, format)
            .context("copying GLES capture without fence support")?;
        Ok(PendingPixels::Ready(crate::render_helpers::mapping_to_vec(
            self, &mapping,
        )?))
    }
}

fn validate_fallback_capture(
    region: Rectangle<i32, Buffer>,
    texture_size: smithay::utils::Size<i32, Buffer>,
    format: Fourcc,
) -> anyhow::Result<()> {
    // The legacy GLES map length is an i32 packed-four-byte product. Apply the
    // same bounds as the deferred path before its synchronous fallback.
    ensure!(
        matches!(
            format,
            Fourcc::Abgr8888 | Fourcc::Xbgr8888 | Fourcc::Argb8888 | Fourcc::Xrgb8888
        ),
        "GLES capture requires a packed 32-bit format"
    );
    ensure!(
        region.loc.x >= 0
            && region.loc.y >= 0
            && region.size.w > 0
            && region.size.h > 0
            && region
                .loc
                .x
                .checked_add(region.size.w)
                .is_some_and(|x| x <= texture_size.w)
            && region
                .loc
                .y
                .checked_add(region.size.h)
                .is_some_and(|y| y <= texture_size.h)
            && region
                .size
                .w
                .checked_mul(region.size.h)
                .and_then(|n| n.checked_mul(4))
                .is_some(),
        "invalid GLES capture region or byte length"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalidated_readback_never_delivers_pixels_or_an_obsolete_device_error() {
        let mut epoch = super::super::capture::CaptureEpoch::default();
        let old = epoch.stamp();
        assert_eq!(
            validate_completion(old.is_current(), Ok(vec![1, 2, 3])).unwrap(),
            vec![1, 2, 3]
        );
        epoch.invalidate();
        assert!(validate_completion(old.is_current(), Ok(vec![1, 2, 3])).is_err());
        let result =
            validate_completion::<Vec<u8>>(false, Err("old device error".into())).unwrap_err();
        assert_eq!(result, "capture scene or output was invalidated");
        assert_eq!(
            validate_completion::<Vec<u8>>(true, Err("current device error".into())).unwrap_err(),
            "current device error"
        );
        assert!(validate_completion::<Vec<u8>>(false, Ok(Vec::new())).is_err());
    }

    #[test]
    fn synchronous_fallback_rejects_unsupported_formats_and_overflow_before_gpu_access() {
        let size = (20, 10).into();
        assert!(validate_fallback_capture(
            Rectangle::new((2, 3).into(), (5, 6).into()),
            size,
            Fourcc::Xrgb8888
        )
        .is_ok());
        for region in [
            Rectangle::new((-1, 0).into(), (2, 2).into()),
            Rectangle::new((19, 9).into(), (2, 2).into()),
            Rectangle::from_size((0, 1).into()),
            Rectangle::new((i32::MAX, 0).into(), (1, 1).into()),
            Rectangle::from_size((32_768, 32_768).into()),
        ] {
            assert!(validate_fallback_capture(region, size, Fourcc::Xrgb8888).is_err());
        }
        assert!(
            validate_fallback_capture(Rectangle::from_size(size), size, Fourcc::Rgb565).is_err()
        );
    }
}
