//! Capture a batch of privacy targets without repainting identical scenes.
//!
//! Equivalence comes from the same damage/occlusion contract used for output
//! rendering, not from current window rules: closing windows and interrupted
//! transitions can still contain older, target-specific snapshots.

use anyhow::Context as _;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::RenderElement;
use smithay::backend::renderer::{Bind, Color32F, Offscreen, Renderer, Texture};
use smithay::utils::{Physical, Scale, Size, Transform};

use super::create_texture;
use super::texture::TextureBuffer;

pub(crate) struct TransitionCapture<T: Texture> {
    size: Size<i32, Physical>,
    scale: Scale<f64>,
    transform: Transform,
    damage: Option<OutputDamageTracker>,
    previous: Option<TextureBuffer<T>>,
}

impl<T: Texture + Clone> TransitionCapture<T> {
    pub(crate) fn new(size: Size<i32, Physical>, scale: Scale<f64>, transform: Transform) -> Self {
        Self {
            size,
            scale,
            transform,
            damage: None,
            previous: None,
        }
    }

    /// Elements are front to back, as for a normal output frame. Previously
    /// returned images are immutable, even when the next target differs.
    pub(crate) fn capture<R, E>(
        &mut self,
        renderer: &mut R,
        elements: &[E],
    ) -> anyhow::Result<TextureBuffer<T>>
    where
        R: Renderer<TextureId = T> + Offscreen<T> + Bind<T>,
        R::Error: Send + Sync + 'static,
        E: RenderElement<R>,
    {
        // Taking both before fallible work makes an error invalidate the batch
        // cache. A retry must never reuse pixels from an unsuccessful capture.
        let previous = self.previous.take();
        let mut damage = self
            .damage
            .take()
            .unwrap_or_else(|| OutputDamageTracker::new(self.size, self.scale, self.transform));
        let (changed, states) = damage.damage_output(1, elements)?;
        if changed.is_none() {
            if let Some(previous) = previous {
                self.previous = Some(previous.clone());
                self.damage = Some(damage);
                return Ok(previous);
            }
        }

        let mut texture = create_texture(renderer, self.size, Fourcc::Abgr8888)
            .context("error creating transition texture")?;
        {
            let mut framebuffer = renderer
                .bind(&mut texture)
                .context("error binding transition texture")?;
            // A changed target gets fresh storage: age zero redraws all visible
            // pixels, while the tracker clips covered content and preserves
            // framebuffer-effect dependencies. Age one here would leave holes.
            damage.render_output_with_states(
                renderer,
                &mut framebuffer,
                0,
                elements,
                Color32F::TRANSPARENT,
                states,
            )?;
        }
        let buffer = TextureBuffer::from_texture(
            renderer,
            texture,
            self.scale,
            self.transform,
            Vec::new(), // Windows below the transition still need frame callbacks.
        );
        self.previous = Some(buffer.clone());
        self.damage = Some(damage);
        Ok(buffer)
    }
}
