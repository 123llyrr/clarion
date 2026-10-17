//! Flatten an interrupted resize into scene-owned pixels.
//!
//! Layout supplies one sampled numeric pose. Capture cannot advance animation
//! time, change layout state, or access a window/desktop owner.

use clarion_config::CornerRadius;
use smithay::utils::{Logical, Rectangle, Scale, Size};

use super::window_resources::{ResizeSnapshot, WindowResourceHandle};
use crate::render_helpers::renderer::AnyRenderer;
use crate::render_helpers::resize::{capture_resize_texture, ResizeRenderElement};
use crate::render_helpers::RenderTarget;

/// Renderer-free inputs sampled after acquire and transaction blockers clear.
/// `size` is the logical trajectory value before physical-pixel rounding.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ResizeRetargetPose<'a> {
    pub(crate) size: Size<f64, Logical>,
    pub(crate) progress: f32,
    pub(crate) clamped: f32,
    pub(crate) scale: f64,
    pub(crate) custom_shader: Option<&'a str>,
}

/// Capture the current previous/next blend, keeping decorations and opacity
/// outside the owned content image. All targets retain independent privacy.
///
/// `capture_resize_texture` preserves the renderer's copy-completion contract:
/// native queues retain sampled resources until completion, and this adapter
/// never substitutes a raw client buffer for the compositor-owned inputs.
pub(crate) fn capture_resize_retarget(
    renderer: AnyRenderer<'_>,
    resources: &WindowResourceHandle,
    next: &ResizeSnapshot,
    pose: ResizeRetargetPose<'_>,
) -> anyhow::Result<ResizeSnapshot> {
    let resources = resources.lease();
    let images = resources.resize.borrow();
    let images = images.as_ref().expect("resize trajectory has scene images");
    let scale = Scale::from(pose.scale);
    let size = pose.size.to_physical_precise_round(scale).to_logical(scale);
    let area = Rectangle::from_size(size);
    let targets = [
        RenderTarget::Output,
        RenderTarget::Screencast,
        RenderTarget::ScreenCapture,
    ];
    match renderer {
        AnyRenderer::Gles(renderer) => {
            anyhow::ensure!(
                ResizeRenderElement::has_shader(renderer),
                "resize shader unavailable"
            );
            let mut captured = Vec::with_capacity(RenderTarget::COUNT);
            for target in targets {
                let (previous, previous_size) = images
                    .snapshot
                    .gles_texture(renderer, target, scale)
                    .ok_or_else(|| anyhow::anyhow!("previous GLES resize snapshot unavailable"))?;
                let (next_texture, next_size) = next
                    .gles_texture(renderer, target, scale)
                    .ok_or_else(|| anyhow::anyhow!("committed GLES resize snapshot unavailable"))?;
                let element = ResizeRenderElement::new(
                    area,
                    scale,
                    previous,
                    previous_size,
                    next_texture,
                    next_size,
                    pose.progress,
                    pose.clamped,
                    CornerRadius::default(),
                    false,
                    1.,
                );
                let texture = capture_resize_texture(renderer, &element, scale, size)?;
                captured.push((
                    texture,
                    images.snapshot.blocked_out(target) && next.blocked_out(target),
                ));
            }
            Ok(ResizeSnapshot::CompositedGles(captured.try_into().unwrap()))
        }
        AnyRenderer::Native(renderer) => {
            let mut captured = Vec::with_capacity(RenderTarget::COUNT);
            for target in targets {
                let (previous, previous_size) = images
                    .snapshot
                    .native_texture(renderer, target, scale)
                    .ok_or_else(|| {
                        anyhow::anyhow!("previous Vulkan resize snapshot unavailable")
                    })?;
                let (next_texture, next_size) = next
                    .native_texture(renderer, target, scale)
                    .ok_or_else(|| {
                        anyhow::anyhow!("committed Vulkan resize snapshot unavailable")
                    })?;
                let element = ResizeRenderElement::new_native(
                    renderer,
                    pose.custom_shader,
                    area,
                    scale,
                    previous,
                    previous_size,
                    next_texture,
                    next_size,
                    pose.progress,
                    pose.clamped,
                    CornerRadius::default(),
                    false,
                    1.,
                )?;
                let texture = capture_resize_texture(renderer, &element, scale, size)?;
                captured.push((
                    texture,
                    images.snapshot.blocked_out(target) && next.blocked_out(target),
                ));
            }
            Ok(ResizeSnapshot::CompositedNative(
                captured.try_into().unwrap(),
            ))
        }
    }
}
