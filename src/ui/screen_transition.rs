use std::time::Duration;

use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesTexture;
use smithay::utils::{Scale, Transform};

use crate::animation::{Animation, Clock};
use crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement;
use crate::render_helpers::texture::{TextureBuffer, TextureRenderElement};
use crate::render_helpers::RenderTarget;

pub const DELAY: Duration = Duration::from_millis(250);
pub const DURATION: Duration = Duration::from_millis(500);

#[derive(Debug, Clone)]
pub enum TransitionTexture {
    Gles(TextureBuffer<GlesTexture>),
    Vulkan(TextureBuffer<crate::render_helpers::vulkan::VulkanTexture>),
}

impl From<TextureBuffer<GlesTexture>> for TransitionTexture {
    fn from(value: TextureBuffer<GlesTexture>) -> Self {
        Self::Gles(value)
    }
}
impl From<TextureBuffer<crate::render_helpers::vulkan::VulkanTexture>> for TransitionTexture {
    fn from(value: TextureBuffer<crate::render_helpers::vulkan::VulkanTexture>) -> Self {
        Self::Vulkan(value)
    }
}

#[derive(Debug)]
pub struct ScreenTransition {
    /// Texture to crossfade from for each render target.
    from_texture: [TransitionTexture; 3],
    /// Monotonic time when to start the crossfade.
    start_at: Duration,
    duration: Duration,
    overview: bool,
    overview_animation: Option<Animation>,
    /// Clock to drive animations.
    clock: Clock,
}

impl ScreenTransition {
    pub fn new(from_texture: [TransitionTexture; 3], delay: Duration, clock: Clock) -> Self {
        Self {
            from_texture,
            start_at: clock.now_unadjusted() + delay,
            duration: DURATION,
            overview: false,
            overview_animation: None,
            clock,
        }
    }

    pub fn new_overview(
        from_texture: [TransitionTexture; 3],
        clock: Clock,
        config: realand_config::Animation,
    ) -> Self {
        let animation = Animation::new(clock.clone(), 1., 0., 0., config);
        Self {
            from_texture,
            start_at: clock.now(),
            duration: Duration::ZERO,
            overview: true,
            overview_animation: Some(animation),
            clock,
        }
    }

    pub fn is_overview(&self) -> bool {
        self.overview
    }

    fn now(&self) -> Duration {
        if self.overview {
            self.clock.now()
        } else {
            self.clock.now_unadjusted()
        }
    }

    pub fn is_done(&self) -> bool {
        if let Some(animation) = &self.overview_animation {
            return animation.is_done();
        }
        (self.overview && self.clock.should_complete_instantly())
            || self.start_at.saturating_add(self.duration) <= self.now()
    }

    pub fn update_render_elements(&mut self, scale: Scale<f64>, transform: Transform) {
        // These textures should remain full-screen, even if scale or transform changes.
        for buffer in &mut self.from_texture {
            match buffer {
                TransitionTexture::Gles(buffer) => {
                    buffer.set_texture_scale(scale);
                    buffer.set_texture_transform(transform);
                }
                TransitionTexture::Vulkan(buffer) => {
                    buffer.set_texture_scale(scale);
                    buffer.set_texture_transform(transform);
                }
            }
        }
    }

    pub fn render(&self, target: RenderTarget) -> PrimaryGpuTextureRenderElement {
        let alpha = self.alpha();

        let idx = match target {
            RenderTarget::Output => 0,
            RenderTarget::Screencast => 1,
            RenderTarget::ScreenCapture => 2,
        };

        match &self.from_texture[idx] {
            TransitionTexture::Gles(buffer) => {
                PrimaryGpuTextureRenderElement::from(TextureRenderElement::from_texture_buffer(
                    buffer.clone(),
                    (0., 0.),
                    alpha,
                    None,
                    None,
                    Kind::Unspecified,
                ))
            }
            TransitionTexture::Vulkan(buffer) => {
                PrimaryGpuTextureRenderElement::from(TextureRenderElement::from_texture_buffer(
                    buffer.clone(),
                    (0., 0.),
                    alpha,
                    None,
                    None,
                    Kind::Unspecified,
                ))
            }
        }
    }

    pub(crate) fn alpha(&self) -> f32 {
        if let Some(animation) = &self.overview_animation {
            return animation.value().clamp(0., 1.) as f32;
        }
        let now = self.now();
        if self.is_done() {
            0.
        } else if self.start_at <= now {
            1. - (now - self.start_at).as_secs_f32() / self.duration.as_secs_f32()
        } else {
            1.
        }
    }
}
