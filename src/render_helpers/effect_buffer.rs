use std::mem;

use anyhow::{ensure, Context as _};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::{Id, RenderElementStates};
use smithay::backend::renderer::gles::{GlesFrame, GlesRenderer, GlesTexture};
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::renderer::{
    Bind as _, Color32F, ContextId, FrameContext as _, Offscreen as _, Renderer as _, Texture,
};
use smithay::utils::{Buffer, Logical, Physical, Scale, Size, Transform};

use crate::render_helpers::blur::{Blur, BlurOptions};
use crate::scene::OutputRenderElements;

#[derive(Debug)]
pub struct EffectBuffer {
    /// Id to be used for this effect buffer's elements.
    id: Id,

    /// Size of the effect buffer.
    size: Size<i32, Buffer>,
    /// Scale of the effect buffer.
    scale: Scale<f64>,
    /// Options for blurring.
    blur_options: BlurOptions,

    /// Elements to be rendered on demand.
    elements: Elements,
    native_elements: Vec<OutputRenderElements<super::vulkan::native::NativeRenderer>>,
    native_texture: Option<super::vulkan::VulkanTexture>,
    native_blurred: Option<super::vulkan::VulkanTexture>,
    native_states: Option<RenderElementStates>,
    native_dirty: bool,
    native_damage: Option<OutputDamageTracker>,
    native_context: Option<ContextId<super::vulkan::VulkanTexture>>,
    native_blur: super::vulkan::blur::NativeBlur,
    /// Offscreen buffer where elements get rendered.
    offscreen: Option<Offscreen>,
    /// Blurring program, if available.
    blur: Option<Blur>,

    /// Commit counter that takes into account both original and blurred texture changes.
    commit_counter: CommitCounter,
}

#[derive(Debug)]
enum Elements {
    /// Contents remain unchanged.
    Unchanged(
        // Storage to avoid reallocating it every time.
        Vec<OutputRenderElements<GlesRenderer>>,
    ),
    /// New contents, need to check damage and render.
    New(Vec<OutputRenderElements<GlesRenderer>>),
}

#[derive(Debug, Clone, Copy)]
struct CachedOffscreenState {
    texture_matches: bool,
    scale_matches: bool,
    unique: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OffscreenPlan {
    Reuse,
    Redraw,
    Recreate,
    NeedsElements,
}

fn offscreen_plan(new_elements: bool, cached: Option<CachedOffscreenState>) -> OffscreenPlan {
    if !new_elements {
        return match cached {
            Some(state) if state.texture_matches && state.scale_matches => OffscreenPlan::Reuse,
            _ => OffscreenPlan::NeedsElements,
        };
    }

    match cached {
        Some(state) if state.texture_matches && state.unique => OffscreenPlan::Redraw,
        _ => OffscreenPlan::Recreate,
    }
}

#[derive(Debug)]
struct Offscreen {
    /// The texture with the offscreen contents.
    texture: GlesTexture,
    /// Id of the renderer context that the texture comes from.
    renderer_context_id: ContextId<GlesTexture>,
    /// Scale of the texture.
    scale: Scale<f64>,
    /// Damage tracker for drawing to the texture.
    damage: OutputDamageTracker,
    /// Render element states from the last render into the offscreen.
    states: RenderElementStates,
    /// Rendered blurred version of the texture.
    ///
    /// When texture needs to be reblurred, this field must be reset to `None`.
    blurred: Option<GlesTexture>,
}

impl Default for Elements {
    fn default() -> Self {
        Self::Unchanged(Vec::new())
    }
}

impl Elements {
    fn render_new<T>(
        &mut self,
        render: impl FnOnce(&[OutputRenderElements<GlesRenderer>]) -> anyhow::Result<T>,
    ) -> anyhow::Result<Option<T>> {
        let Self::New(elements) = self else {
            return Ok(None);
        };
        let result = render(elements)?;
        elements.clear();
        *self = Self::Unchanged(mem::take(elements));
        Ok(Some(result))
    }
}

impl Default for EffectBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl EffectBuffer {
    pub fn new() -> Self {
        Self {
            id: Id::new(),
            size: Size::default(),
            scale: Scale::from(1.),
            blur_options: BlurOptions::default(),
            elements: Elements::default(),
            native_elements: Vec::new(),
            native_texture: None,
            native_blurred: None,
            native_states: None,
            native_dirty: true,
            native_damage: None,
            native_context: None,
            native_blur: Default::default(),
            offscreen: None,
            blur: None,
            commit_counter: CommitCounter::default(),
        }
    }

    /// Retire a native-device epoch even for inactive capture targets. Waiting
    /// until prepare_native() would leave hidden effect and blur-pool images
    /// holding the old device indefinitely. Submitted GPU work retains its own
    /// image and client-buffer owners independently of these scene caches.
    pub(crate) fn invalidate_native_resources(&mut self) {
        self.native_elements.clear();
        self.native_texture = None;
        self.native_blurred = None;
        self.native_states = None;
        self.native_damage = None;
        self.native_context = None;
        self.native_blur = Default::default();
        self.native_dirty = true;
        self.commit_counter.increment();
    }

    pub fn id(&self) -> &Id {
        &self.id
    }

    pub fn commit(&self) -> CommitCounter {
        self.commit_counter
    }

    pub fn logical_size(&self) -> Size<f64, Logical> {
        self.size.to_f64().to_logical(self.scale, Transform::Normal)
    }

    pub fn scale(&self) -> Scale<f64> {
        self.scale
    }

    pub fn render_element_states(&self) -> Option<&RenderElementStates> {
        self.native_states
            .as_ref()
            .or_else(|| self.offscreen.as_ref().map(|o| &o.states))
    }

    pub fn update_size(&mut self, size: Size<i32, Physical>, scale: Scale<f64>) {
        if self.size != size.to_logical(1).to_buffer(1, Transform::Normal) || self.scale != scale {
            self.native_dirty = true;
            self.native_damage = None;
        }
        self.size = size.to_logical(1).to_buffer(1, Transform::Normal);
        self.scale = scale;
    }

    pub fn update_blur_options(&mut self, options: BlurOptions) {
        if self.blur_options != options {
            self.native_blurred = None;
        }
        if self.blur_options == options {
            return;
        }

        self.blur_options = options;

        if let Some(offscreen) = &mut self.offscreen {
            if offscreen.blurred.is_some() {
                offscreen.blurred = None;
                self.commit_counter.increment();
            }
        }
    }

    pub fn elements(&mut self) -> &mut Vec<OutputRenderElements<GlesRenderer>> {
        // Assume we're going to insert new elements, switch to New.
        match mem::take(&mut self.elements) {
            Elements::Unchanged(elements) | Elements::New(elements) => {
                self.elements = Elements::New(elements);
            }
        }
        let Elements::New(elements) = &mut self.elements else {
            unreachable!();
        };
        elements
    }

    pub fn elements_native(
        &mut self,
    ) -> &mut Vec<OutputRenderElements<super::vulkan::native::NativeRenderer>> {
        self.native_dirty = true;
        &mut self.native_elements
    }

    pub fn native_texture(&self, blur: bool) -> Option<&super::vulkan::VulkanTexture> {
        if blur {
            self.native_blurred.as_ref()
        } else {
            self.native_texture.as_ref()
        }
    }

    pub fn prepare_any<R: super::renderer::ClarionRenderer>(
        &mut self,
        renderer: &mut R,
        blur: bool,
    ) -> bool {
        if let Some(renderer) = renderer.as_gles_renderer() {
            return self.prepare(renderer, blur);
        }
        let Some(renderer) = renderer.as_native_renderer() else {
            return false;
        };
        match self.prepare_native(renderer, blur) {
            Ok(()) => true,
            Err(err) => {
                warn!("error preparing native background: {err:?}");
                false
            }
        }
    }

    fn prepare_native(
        &mut self,
        renderer: &mut super::vulkan::native::NativeRenderer,
        blur: bool,
    ) -> anyhow::Result<()> {
        let _latency =
            crate::utils::latency::Span::new(crate::utils::latency::Operation::NativeBackground);
        let context = renderer.context_id();
        if self.native_context.as_ref() != Some(&context)
            || self.native_texture.as_ref().is_some_and(|texture| {
                texture.color.description != renderer.scene_color_description()
                    || texture.format() != Some(renderer.scene_buffer_format(Fourcc::Abgr8888))
            })
        {
            self.native_texture = None;
            self.native_blurred = None;
            self.native_damage = None;
            self.native_context = Some(context);
            self.native_dirty = true;
        }
        // Scene reconstruction only means we must *check* damage. A snapshot
        // may retain the source and blurred textures even when their pixels
        // are unchanged; testing ownership first would redraw and reblur every
        // frame for the entire lifetime of that snapshot.
        let pending_states = if self.native_dirty && self.native_texture.is_some() {
            if let Some(damage) = &mut self.native_damage {
                let (changed, states) = damage.damage_output(1, &self.native_elements)?;
                if changed.is_none() {
                    self.native_states = Some(states);
                    self.native_dirty = false;
                    None
                } else {
                    Some(states)
                }
            } else {
                None
            }
        } else {
            None
        };
        if self.native_dirty || self.native_texture.is_none() {
            // Keep advanced history only after the target has been updated.
            // Any allocation, bind or submission failure must retry in full.
            let mut damage = self.native_damage.take();
            // Do not overwrite images retained by a snapshot or an in-flight submission.
            // Only retire completed work when that could make the existing image unique again.
            // Creating a replacement buffer and starting an output frame retire on their own.
            if self.native_texture.as_ref().is_some_and(|texture| {
                texture.size() == self.size && !texture.is_unique_reference()
            }) {
                renderer.retire_completed()?;
            }
            let recreate = self.native_texture.as_ref().is_none_or(|texture| {
                texture.size() != self.size || !texture.is_unique_reference()
            });
            if recreate {
                let texture = renderer.create_buffer(Fourcc::Abgr8888, self.size)?;
                if let Some(old) = self.native_texture.replace(texture) {
                    renderer.recycle_offscreen(old);
                }
                damage = None;
            }
            let full = damage.is_none();
            let size = self.size.to_logical(1, Transform::Normal).to_physical(1);
            let tracker = damage.get_or_insert_with(|| {
                OutputDamageTracker::new(size, self.scale, Transform::Normal)
            });
            let mut target = renderer.bind(self.native_texture.as_mut().unwrap())?;
            let result = if let Some(states) = pending_states {
                // damage_output already advanced the history once. Age two
                // carries that damage into the actual render; a new image
                // still needs a full redraw. Preserve effect capture states.
                tracker.render_output_with_states(
                    renderer,
                    &mut target,
                    if full { 0 } else { 2 },
                    &self.native_elements,
                    Color32F::TRANSPARENT,
                    states,
                )
            } else {
                tracker.render_output(
                    renderer,
                    &mut target,
                    0,
                    &self.native_elements,
                    Color32F::TRANSPARENT,
                )
            };
            let result = result?;
            if full || result.damage.is_some() {
                self.native_blurred = None;
                self.commit_counter.increment();
            }
            self.native_states = Some(result.states);
            self.native_damage = damage;
            self.native_dirty = false;
        }
        if blur && self.native_blurred.is_none() {
            let (texture, _sync) = self.native_blur.render(
                renderer,
                self.native_texture.as_ref().unwrap(),
                self.blur_options,
            )?;
            self.native_blurred = Some(texture);
            self.commit_counter.increment();
        }
        Ok(())
    }

    pub fn prepare(&mut self, renderer: &mut GlesRenderer, blur: bool) -> bool {
        if let Err(err) = self.prepare_offscreen(renderer) {
            warn!("error preparing offscreen: {err:?}");
            return false;
        };

        if blur {
            if let Err(err) = self.prepare_blur(renderer) {
                warn!("error preparing blur: {err:?}");
                return false;
            }
        }

        true
    }

    fn prepare_offscreen(&mut self, renderer: &mut GlesRenderer) -> anyhow::Result<()> {
        let _span = clarion_tracy_span!("EffectBuffer::prepare_offscreen");

        let cached = self
            .offscreen
            .as_mut()
            .map(|offscreen| CachedOffscreenState {
                texture_matches: offscreen.texture.size() == self.size
                    && offscreen.renderer_context_id == renderer.context_id(),
                scale_matches: offscreen.scale == self.scale,
                unique: offscreen.texture.is_unique_reference(),
            });
        let plan = offscreen_plan(matches!(self.elements, Elements::New(_)), cached);
        match plan {
            OffscreenPlan::Reuse => return Ok(()),
            OffscreenPlan::NeedsElements => {
                // Rendered elements are released after use. The caller must provide them again
                // after a size, scale or renderer change before the cached image is usable.
                self.offscreen = None;
                anyhow::bail!("effect buffer needs new elements to recreate its offscreen image");
            }
            OffscreenPlan::Redraw | OffscreenPlan::Recreate => {}
        }

        let reason = if plan == OffscreenPlan::Recreate {
            let reason = match self.offscreen.as_ref() {
                None => "first render".to_owned(),
                Some(offscreen) if offscreen.texture.size() != self.size => format!(
                    "size changed from {} × {} to {} × {}",
                    offscreen.texture.size().w,
                    offscreen.texture.size().h,
                    self.size.w,
                    self.size.h
                ),
                Some(offscreen) if offscreen.renderer_context_id != renderer.context_id() => {
                    "renderer id changed".to_owned()
                }
                Some(_) => "not unique".to_owned(),
            };
            self.offscreen = None;
            reason
        } else {
            String::new()
        };

        let offscreen = if let Some(offscreen) = &mut self.offscreen {
            offscreen
        } else {
            trace!("creating new offscreen texture: {reason}");
            let span = clarion_tracy_span!("creating effect offscreen texture");
            span.emit_text(&reason);

            let texture: GlesTexture = renderer
                .create_buffer(Fourcc::Abgr8888, self.size)
                .context("error creating texture")?;

            let buffer_size = self.size.to_logical(1, Transform::Normal).to_physical(1);
            let damage = OutputDamageTracker::new(buffer_size, self.scale, Transform::Normal);

            self.offscreen.insert(Offscreen {
                texture,
                renderer_context_id: renderer.context_id(),
                scale: self.scale,
                damage,
                states: RenderElementStates::default(),
                blurred: None,
            })
        };

        // Recreate the damage tracker if the scale changes. We already recreate it for buffer size
        // changes, and transform is always Normal.
        if offscreen.scale != self.scale {
            offscreen.scale = self.scale;

            trace!("recreating damage tracker due to scale change");
            let buffer_size = self.size.to_logical(1, Transform::Normal).to_physical(1);
            offscreen.damage = OutputDamageTracker::new(buffer_size, self.scale, Transform::Normal);

            self.commit_counter.increment();
            offscreen.blurred = None;
        }

        // Keep the pending elements until rendering succeeds, so a transient GPU error can retry.
        let res = self.elements.render_new(|elements| {
            let mut target = renderer
                .bind(&mut offscreen.texture)
                .context("error binding texture")?;
            offscreen
                .damage
                .render_output(renderer, &mut target, 1, elements, Color32F::TRANSPARENT)
                .context("error rendering")
        });
        let res = match res {
            Ok(Some(res)) => res,
            Ok(None) => unreachable!("offscreen redraw requires new elements"),
            Err(err) => {
                // The target may be partially modified; start the retry with a fresh tracker.
                self.offscreen = None;
                return Err(err);
            }
        };

        offscreen.states = res.states;

        if res.damage.is_some() {
            self.commit_counter.increment();

            // Original texture changed; reset the blurred texture.
            offscreen.blurred = None;
        }

        Ok(())
    }

    fn prepare_blur(&mut self, renderer: &mut GlesRenderer) -> anyhow::Result<()> {
        let offscreen = self.offscreen.as_mut().context("missing offscreen")?;
        if offscreen.blurred.is_some() {
            // Already rendered.
            return Ok(());
        }

        if let Some(blur) = &self.blur {
            if blur.context_id() != renderer.context_id() {
                debug!("recreating blur: renderer changed");
                self.blur = None;
            }
        }

        let blur = if let Some(blur) = &mut self.blur {
            blur
        } else {
            let Some(blur) = Blur::new(renderer) else {
                // Missing blur shader.
                return Ok(());
            };
            self.blur.insert(blur)
        };

        ensure!(
            offscreen.renderer_context_id == renderer.context_id(),
            "wrong renderer context id"
        );

        blur.prepare_textures(
            |fourcc, size| renderer.create_buffer(fourcc, size),
            &offscreen.texture,
            self.blur_options,
        )
        .context("error preparing blur textures")?;

        Ok(())
    }

    pub fn render(&mut self, frame: &mut GlesFrame, blur: bool) -> anyhow::Result<GlesTexture> {
        let offscreen = self.offscreen.as_mut().context("offscreen is missing")?;

        if !blur {
            return Ok(offscreen.texture.clone());
        }

        let texture = if let Some(texture) = &offscreen.blurred {
            texture.clone()
        } else {
            let blur = self.blur.as_mut().context("blur is missing")?;
            let mut guard = frame.renderer();
            let renderer = guard.as_mut();
            let blurred = blur
                .render(renderer, &offscreen.texture, self.blur_options)
                .context("error rendering blur")?;
            offscreen.blurred.insert(blurred).clone()
        };

        Ok(texture)
    }
}

#[cfg(test)]
mod tests {
    use smithay::backend::renderer::element::{Element as _, Id, Kind};
    use smithay::backend::renderer::utils::CommitCounter;
    use smithay::backend::renderer::Color32F;
    use smithay::utils::Rectangle;

    use super::{offscreen_plan, CachedOffscreenState, Elements, OffscreenPlan};
    use crate::render_helpers::solid_color::SolidColorRenderElement;

    #[test]
    fn shared_offscreen_is_reused_until_its_contents_change() {
        let shared = CachedOffscreenState {
            texture_matches: true,
            scale_matches: true,
            unique: false,
        };
        assert_eq!(offscreen_plan(false, Some(shared)), OffscreenPlan::Reuse);
        assert_eq!(offscreen_plan(true, Some(shared)), OffscreenPlan::Recreate);

        let unique = Some(CachedOffscreenState {
            unique: true,
            ..shared
        });
        assert_eq!(offscreen_plan(true, unique), OffscreenPlan::Redraw);
        assert_eq!(offscreen_plan(true, None), OffscreenPlan::Recreate);
    }

    #[test]
    fn stale_offscreen_cannot_be_used_without_new_elements() {
        let old_scale = Some(CachedOffscreenState {
            texture_matches: true,
            scale_matches: false,
            unique: true,
        });
        assert_eq!(
            offscreen_plan(false, old_scale),
            OffscreenPlan::NeedsElements
        );
        assert_eq!(offscreen_plan(true, old_scale), OffscreenPlan::Redraw);

        let old_texture = Some(CachedOffscreenState {
            texture_matches: false,
            scale_matches: true,
            unique: true,
        });
        assert_eq!(
            offscreen_plan(false, old_texture),
            OffscreenPlan::NeedsElements
        );
        assert_eq!(offscreen_plan(true, old_texture), OffscreenPlan::Recreate);
    }

    #[test]
    fn pending_elements_survive_a_failed_render_and_can_retry() {
        let color = SolidColorRenderElement::new(
            Id::new(),
            Rectangle::from_size((1., 1.).into()),
            CommitCounter::default(),
            Color32F::TRANSPARENT,
            Kind::Unspecified,
        );
        let mut elements = Elements::New(vec![color.into()]);
        let failed = elements.render_new(|pending| -> anyhow::Result<()> {
            assert_eq!(pending.len(), 1);
            anyhow::bail!("transient render failure")
        });
        assert!(failed.is_err());
        assert!(matches!(elements, Elements::New(_)));

        assert_eq!(
            elements.render_new(|pending| Ok(pending.len())).unwrap(),
            Some(1)
        );
        assert!(matches!(elements, Elements::Unchanged(_)));
    }

    #[test]
    #[ignore = "requires a real DRM render node"]
    fn native_effect_color_cache_rebuilds_for_output_color_and_precision_changes() {
        use crate::color::{ColorDescription, HdrOutputSettings};
        use crate::render_helpers::solid_color::SolidColorBuffer;
        use crate::render_helpers::vulkan::hardware_tests::setup;
        use crate::render_helpers::vulkan::native::NativeRenderer;
        use smithay::backend::allocator::Fourcc;
        use smithay::backend::renderer::Texture;
        use smithay::utils::Scale;
        let (vk, reference, allocator) = setup();
        drop(reference);
        drop(allocator);
        let mut renderer = NativeRenderer::new(vk).unwrap();
        renderer
            .prepare_output_format_for_test(Fourcc::Abgr8888)
            .unwrap();
        let mut buffer = super::EffectBuffer::new();
        buffer.update_size((32, 24).into(), Scale::from(1.));
        let source = SolidColorBuffer::new((32., 24.), [0.75, 0.5, 0.25, 1.]);
        buffer.elements_native().push(
            SolidColorRenderElement::from_buffer(&source, (0., 0.), 1., Kind::Unspecified).into(),
        );
        let outputs = [
            None,
            Some(ColorDescription::srgb()),
            Some(HdrOutputSettings::default().description()),
            Some(
                HdrOutputSettings {
                    peak_luminance: 500.,
                    ..Default::default()
                }
                .description(),
            ),
            None,
        ];
        let mut previous: Option<crate::render_helpers::vulkan::VulkanTexture> = None;
        for output in outputs {
            let _scope = renderer.output_color_scope(output);
            buffer.prepare_native(&mut renderer, false).unwrap();
            let texture = buffer.native_texture(false).unwrap().clone();
            assert_eq!(
                texture.color.description,
                renderer.scene_color_description()
            );
            assert_eq!(
                texture.format(),
                Some(renderer.scene_buffer_format(Fourcc::Abgr8888))
            );
            if let Some(previous) = previous.take() {
                assert_ne!(
                    texture.image(),
                    previous.image(),
                    "retained prior scene pixels must stay immutable"
                );
            }
            let commit = buffer.commit();
            buffer.prepare_native(&mut renderer, false).unwrap();
            assert_eq!(buffer.commit(), commit, "unchanged scene must not redraw");
            previous = Some(texture);
        }
    }

    #[test]
    #[ignore = "requires real DRM render node and native Vulkan contexts"]
    fn native_epoch_invalidation_releases_inactive_capture_context_and_rebuilds() {
        use std::sync::Arc;
        use std::time::{Duration, Instant};

        use smithay::backend::allocator::Fourcc;
        use smithay::backend::renderer::{Bind, ExportMem, Texture};

        use crate::render_helpers::blur::BlurOptions;
        use crate::render_helpers::solid_color::SolidColorBuffer;
        use crate::render_helpers::vulkan::hardware_tests::setup;
        use crate::render_helpers::vulkan::native::NativeRenderer;
        use crate::render_helpers::xray::Xray;

        // This test checks epoch ownership and rebuilt pixels, while native
        // preparation can legitimately defer to a later owner frame. Wait on
        // real progress before the original public-API/cache assertions.
        fn prepare_ready(buffer: &mut super::EffectBuffer, renderer: &mut NativeRenderer) {
            use crate::render_helpers::vulkan::native::NativeError;
            let ids: Vec<_> = buffer
                .native_elements
                .iter()
                .map(|element| element.id().clone())
                .collect();
            let deadline = Instant::now() + Duration::from_secs(15);
            let mut retries = 0;
            loop {
                renderer.begin_scene();
                match buffer.prepare_native(renderer, true) {
                    Ok(()) => {
                        if retries > 0 {
                            eprintln!("EFFECT_BUFFER_PREPARATION retries={retries}");
                        }
                        return;
                    }
                    Err(error) if NativeError::is_backpressure(error.as_ref()) => {
                        assert!(
                            renderer.scene_was_deferred(),
                            "Busy must wake a retry frame"
                        );
                        assert_eq!(
                            buffer
                                .native_elements
                                .iter()
                                .map(|element| element.id().clone())
                                .collect::<Vec<_>>(),
                            ids,
                            "backpressure must preserve pending scene elements"
                        );
                        assert!(
                            Instant::now() < deadline,
                            "native effect retry timed out: {error:#}"
                        );
                        if retries == 0 {
                            eprintln!("EFFECT_BUFFER_PREPARATION deferred={error:#}");
                        }
                        retries += 1;
                        renderer.poll_preparation_for_test().unwrap();
                        renderer.retire_completed().unwrap();
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("native effect preparation failed: {error:#}"),
                }
            }
        }

        let (vk, reference, allocator) = setup();
        drop(reference);
        drop(allocator);
        let old_context = Arc::downgrade(vk.ctx.as_ref().unwrap());
        let mut previous = NativeRenderer::new(vk).unwrap();
        previous
            .prepare_output_format_for_test(Fourcc::Abgr8888)
            .unwrap();
        let xray = Xray::new();
        let mut identities = Vec::new();
        for buffer in xray.background.iter().chain(&xray.backdrop) {
            let mut buffer = buffer.borrow_mut();
            buffer.update_size((48, 32).into(), 1.0.into());
            buffer.update_blur_options(BlurOptions {
                passes: 2,
                offset: 1.0,
            });
            let mut source = SolidColorBuffer::new((48., 32.), [1., 0., 0., 1.]);
            buffer.elements_native().push(
                SolidColorRenderElement::from_buffer(&source, (0., 0.), 1., Kind::Unspecified)
                    .into(),
            );
            prepare_ready(&mut buffer, &mut previous);
            assert!(buffer.prepare_any(&mut previous, true));
            // Force rotation into NativeBlur's retained output pool. Clearing
            // only EffectBuffer.native_texture/native_blurred must not pass.
            let retained = buffer.native_texture(true).unwrap().clone();
            source.set_color([0., 0., 1., 1.]);
            buffer.elements_native().clear();
            buffer.elements_native().push(
                SolidColorRenderElement::from_buffer(&source, (0., 0.), 1., Kind::Unspecified)
                    .into(),
            );
            prepare_ready(&mut buffer, &mut previous);
            assert!(buffer.prepare_any(&mut previous, true));
            assert!(!buffer.native_blur.retained_output_allocations().is_empty());
            drop(retained);
            identities.push((buffer.id().clone(), buffer.commit()));
        }
        // This explicit wait is confined to the GPU test. It distinguishes
        // scene-cache ownership from legitimately unfinished GPU submissions.
        unsafe {
            previous
                .vulkan()
                .ctx
                .as_ref()
                .unwrap()
                .device
                .device_wait_idle()
                .unwrap();
        }
        previous.retire_completed().unwrap();
        drop(previous);
        assert!(
            old_context.upgrade().is_some(),
            "inactive effect caches must pin the fixture before invalidation"
        );

        xray.invalidate_native_resources();
        for (buffer, (identity, commit)) in
            xray.background.iter().chain(&xray.backdrop).zip(identities)
        {
            let buffer = buffer.borrow();
            assert_eq!(buffer.id(), &identity);
            assert_ne!(
                buffer.commit(),
                commit,
                "replacement must damage the old effect contents"
            );
            assert!(buffer.native_texture(false).is_none());
            assert!(buffer.native_texture(true).is_none());
            assert!(buffer.native_states.is_none());
            assert!(buffer.native_damage.is_none());
            assert!(buffer.native_context.is_none());
            assert!(buffer.native_elements.is_empty());
            assert!(buffer.native_blur.retained_output_allocations().is_empty());
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while old_context.upgrade().is_some() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            old_context.upgrade().is_none(),
            "display and unused capture caches must release the retired Vulkan context"
        );

        let (vk, reference, allocator) = setup();
        drop(reference);
        drop(allocator);
        let mut replacement = NativeRenderer::new(vk).unwrap();
        replacement
            .prepare_output_format_for_test(Fourcc::Abgr8888)
            .unwrap();
        for buffer in xray.background.iter().chain(&xray.backdrop) {
            let mut buffer = buffer.borrow_mut();
            let source = SolidColorBuffer::new((48., 32.), [0., 1., 0., 1.]);
            buffer.elements_native().push(
                SolidColorRenderElement::from_buffer(&source, (0., 0.), 1., Kind::Unspecified)
                    .into(),
            );
            prepare_ready(&mut buffer, &mut replacement);
            assert!(buffer.prepare_any(&mut replacement, true));
            let mut texture = buffer.native_texture(true).unwrap().clone();
            let size = texture.size();
            let framebuffer = replacement.bind(&mut texture).unwrap();
            let mapping = replacement
                .copy_framebuffer(&framebuffer, Rectangle::from_size(size), Fourcc::Abgr8888)
                .unwrap();
            let pixels = replacement.map_texture(&mapping).unwrap();
            assert!(
                pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|pixel| { pixel[0] < 10 && pixel[1] > 240 && pixel[2] < 10 }),
                "all display and capture buffers must rebuild fresh green pixels"
            );
        }
    }
}
