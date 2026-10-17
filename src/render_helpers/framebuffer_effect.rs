use std::cell::RefCell;

use clarion_config::CornerRadius;
use glam::{Mat3, Vec2};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::{Element, Id, RenderElement};
use smithay::backend::renderer::gles::{
    ffi, GlesError, GlesFrame, GlesRenderer, GlesTexture, Uniform,
};
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::renderer::{ContextId, Frame as _, FrameContext, Offscreen, Texture as _};
use smithay::gpu_span_location;
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Size, Transform};

use crate::backend::tty::{TtyFrame, TtyRenderer, TtyRendererError};
use crate::render_helpers::background_effect::RenderParams;
use crate::render_helpers::blur::{Blur, BlurOptions};
use crate::render_helpers::renderer::AsGlesFrame as _;
use crate::render_helpers::shaders::{mat3_uniform, Shaders};
use crate::render_helpers::vulkan::blur::NativeBlur;
use crate::render_helpers::vulkan::native::{
    NativeError, NativeFrame, NativeRenderer, Postprocess,
};
use crate::render_helpers::vulkan::pipeline::TextureClip;
use crate::render_helpers::vulkan::VulkanTexture;
use crate::utils::region::TransformedRegion;

#[derive(Debug)]
pub struct FramebufferEffect {
    id: Id,
    commit: CommitCounter,
}

#[derive(Debug)]
pub struct FramebufferEffectElement {
    id: Id,
    commit: CommitCounter,
    geometry: Rectangle<f64, Logical>,
    clip_geo: Rectangle<f64, Logical>,
    corner_radius: CornerRadius,
    subregion: Option<TransformedRegion>,
    scale: f32,
    blur_options: Option<BlurOptions>,
    noise: f32,
    saturation: f32,
    liquid_glass: bool,
    #[cfg(test)]
    pub(crate) full_blur_for_test: bool,
}

#[derive(Debug)]
struct Inner {
    framebuffer: Option<GlesTexture>,
    blur: Option<Blur>,
    intermediate: Option<GlesTexture>,
    /// Reusable storage for subregion-filtered damage rects.
    subregion_damage: Vec<Rectangle<i32, Physical>>,
}

impl Default for FramebufferEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl FramebufferEffect {
    pub fn new() -> Self {
        Self {
            id: Id::new(),
            commit: CommitCounter::default(),
        }
    }

    pub fn damage(&mut self) {
        self.commit.increment();
    }

    pub fn render(
        &self,
        ns: Option<usize>,
        params: RenderParams,
        blur_options: Option<BlurOptions>,
        noise: f32,
        saturation: f32,
    ) -> FramebufferEffectElement {
        let (clip_geo, corner_radius) = params
            .clip
            .unwrap_or((params.geometry, CornerRadius::default()));

        let mut id = self.id.clone();
        if let Some(ns) = ns {
            id = id.namespaced(ns);
        }

        FramebufferEffectElement {
            id,
            commit: self.commit,
            geometry: params.geometry,
            clip_geo,
            corner_radius,
            subregion: params.subregion,
            scale: params.scale as f32,
            blur_options,
            noise,
            saturation,
            liquid_glass: false,
            #[cfg(test)]
            full_blur_for_test: false,
        }
    }
}

impl FramebufferEffectElement {
    pub(crate) fn with_liquid_glass(mut self, part: usize) -> Self {
        self.liquid_glass = true;
        self.id = self.id.namespaced(part);
        self
    }

    /// Texture pixels the final draw can sample. Capture size, viewport and UVs
    /// stay unchanged; include bilinear neighbours before limiting blur passes.
    fn sampled_blur_region(
        &self,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        clamped_dst: Rectangle<i32, Physical>,
        transform: Transform,
        texture_size: Size<i32, Buffer>,
        filtered: &mut Vec<Rectangle<i32, Physical>>,
    ) -> Rectangle<i32, Buffer> {
        let full = Rectangle::from_size(texture_size);
        let Some(subregion) = &self.subregion else {
            return full;
        };
        filtered.clear();
        let mut crop = src.to_logical(1., Transform::Normal, &src.size);
        crop.loc += self.geometry.loc;
        subregion.filter_damage(crop, dst, &[Rectangle::from_size(dst.size)], filtered);
        let clamp_offset = clamped_dst.loc - dst.loc;
        let bounds = Rectangle::new(clamp_offset, clamped_dst.size);
        let captured_size = transform.transform_size(clamped_dst.size);
        let ratio_x = f64::from(texture_size.w) / f64::from(captured_size.w);
        let ratio_y = f64::from(texture_size.h) / f64::from(captured_size.h);
        filtered
            .iter()
            .filter_map(|r| r.intersection(bounds))
            .map(|mut r| {
                r.loc -= clamp_offset;
                let r = transform.transform_rect_in(r, &clamped_dst.size);
                // Two texels include bilinear neighbours plus rounding tolerance;
                // the final draw uses the original full capture's normalized UVs.
                let a = (
                    (f64::from(r.loc.x) * ratio_x).floor() as i32 - 2,
                    (f64::from(r.loc.y) * ratio_y).floor() as i32 - 2,
                );
                let b = (
                    (f64::from(r.loc.x + r.size.w) * ratio_x).ceil() as i32 + 2,
                    (f64::from(r.loc.y + r.size.h) * ratio_y).ceil() as i32 + 2,
                );
                Rectangle::from_extremities(a, b)
            })
            .filter_map(|r| r.intersection(full))
            .reduce(|a, b| a.merge(b))
            .unwrap_or_default()
    }

    fn compute_uniforms(
        &self,
        crop: Rectangle<f64, Logical>,
        transform: Transform,
    ) -> [Uniform<'static>; 8] {
        let offset = crop.loc - (self.clip_geo.loc - self.geometry.loc);
        let offset = Vec2::new(offset.x as f32, offset.y as f32);
        let crop_size = Vec2::new(crop.size.w as f32, crop.size.h as f32);
        let clip_size = Vec2::new(self.clip_geo.size.w as f32, self.clip_geo.size.h as f32);

        // Our v_coords are [0, 1] inside crop. We want them to be [0, 1] inside clip_geo.
        let input_to_clip_geo =
            Mat3::from_scale(crop_size / clip_size) * Mat3::from_translation(offset / crop_size);

        // Revert the effect of the texture transform.
        let transform_mat = Mat3::from_translation(Vec2::new(0.5, 0.5))
            * transform.matrix()
            * Mat3::from_translation(Vec2::new(-0.5, -0.5));
        let input_to_clip_geo = input_to_clip_geo * transform_mat;

        let clip_geo_size = (self.clip_geo.size.w as f32, self.clip_geo.size.h as f32);

        [
            Uniform::new("clarion_scale", self.scale),
            Uniform::new("geo_size", clip_geo_size),
            Uniform::new("corner_radius", <[f32; 4]>::from(self.corner_radius)),
            mat3_uniform("input_to_geo", input_to_clip_geo),
            Uniform::new("noise", self.noise),
            Uniform::new("saturation", self.saturation),
            Uniform::new("bg_color", [0f32, 0., 0., 0.]),
            Uniform::new(
                "liquid_glass",
                if self.liquid_glass {
                    [4f32, 0.22]
                } else {
                    [0f32; 2]
                },
            ),
        ]
    }

    fn compute_clip(&self, crop: Rectangle<f64, Logical>, transform: Transform) -> TextureClip {
        let offset = crop.loc - (self.clip_geo.loc - self.geometry.loc);
        let offset = Vec2::new(offset.x as f32, offset.y as f32);
        let crop_size = Vec2::new(crop.size.w as f32, crop.size.h as f32);
        let clip_size = Vec2::new(self.clip_geo.size.w as f32, self.clip_geo.size.h as f32);

        // Our v_coords are [0, 1] inside crop. We want them to be [0, 1] inside clip_geo.
        let input_to_clip_geo =
            Mat3::from_scale(crop_size / clip_size) * Mat3::from_translation(offset / crop_size);

        // Revert the effect of the texture transform.
        let transform_mat = Mat3::from_translation(Vec2::new(0.5, 0.5))
            * transform.matrix()
            * Mat3::from_translation(Vec2::new(-0.5, -0.5));
        let input_to_clip_geo = input_to_clip_geo * transform_mat;

        let clip_geo_size = (self.clip_geo.size.w as f32, self.clip_geo.size.h as f32);
        let radius = <[f32; 4]>::from(self.corner_radius);
        let cols = input_to_clip_geo.to_cols_array_2d();

        TextureClip {
            matrix: [
                [cols[0][0], cols[0][1], cols[0][2], 0.0],
                // These std430 padding lanes are not part of the mat3. Keep
                // the existing Vulkan push-constant size/device requirements.
                [
                    cols[1][0],
                    cols[1][1],
                    cols[1][2],
                    if self.liquid_glass { 4.0 } else { 0.0 },
                ],
                [
                    cols[2][0],
                    cols[2][1],
                    cols[2][2],
                    if self.liquid_glass { 0.22 } else { 0.0 },
                ],
            ],
            radius,
            size: [clip_geo_size.0, clip_geo_size.1],
            scale: self.scale,
            enabled: 1.0,
        }
    }

    fn compute_postprocess(&self) -> Postprocess {
        Postprocess {
            noise: self.noise,
            saturation: self.saturation,
            bg_color: [0.0, 0.0, 0.0, 0.0],
        }
    }
}

impl Element for FramebufferEffectElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        // We don't use src for drawing but we can use it to figure out how we were cropped.
        let size = self.geometry.size.to_buffer(1., Transform::Normal);
        Rectangle::from_size(size)
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.geometry.to_physical_precise_round(scale)
    }

    fn is_framebuffer_effect(&self) -> bool {
        true
    }
}

impl RenderElement<GlesRenderer> for FramebufferEffectElement {
    fn capture_framebuffer(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        cache: &UserDataMap,
    ) -> Result<(), GlesError> {
        let _span = clarion_tracy_span!("FramebufferEffectElement::capture_framebuffer");
        let location = gpu_span_location!("FramebufferEffectElement::capture_framebuffer");
        frame.with_gpu_span(location, |frame| {
            let output_rect = Rectangle::from_size(frame.output_size());
            let transform = frame.transformation();

            let mut guard = frame.renderer();

            let inner = cache
                .get_or_insert::<RefCell<Inner>, _>(|| RefCell::new(Inner::new(guard.as_mut())));
            let mut inner = inner.borrow_mut();
            let inner = &mut *inner;

            let clamped_dst = match dst.intersection(output_rect) {
                Some(clamped) => clamped,
                None => {
                    inner.intermediate = None;
                    return Ok(());
                }
            };
            let clamp_scale = clamped_dst.size.to_f64() / dst.size.to_f64();

            let dst = transform.transform_rect_in(clamped_dst, &output_rect.size);

            // Compute size from our geometry and scale.
            //
            // The "correct" size is always dst.size since that's the pixel region we're actually
            // blitting. However, using dst.size causes two undesirable things when zooming out for
            // the overview:
            // 1. dst.size shrinks every frame, causing a texture realloaction for every fb effect
            //    element every frame.
            // 2. The underlying blur visually expands. This is technically correct, since the
            //    underlying contents shrink, but it's not what you visually expect: you expect the
            //    blur to also shrink as the windows zoom out, to give the zooming out effect.
            //
            // Using size computed from geometry and scale solves both of those problems (even
            // though there's a bit of a cost in that zoomed-out elements still blur the entire
            // unzoomed texture size, and even though the blur ends up slightly wrong as there's two
            // layers of texture resampling, up and back down).
            //
            // Here we use src.size rather than geometry directly because src takes into account
            // cropping.
            let size = src
                .size
                .to_logical(1., Transform::Normal)
                .upscale(clamp_scale)
                .to_physical_precise_round(self.scale);
            let size = transform.transform_size(size);

            let size = size.to_logical(1).to_buffer(1, Transform::Normal);

            // Recreate framebuffer if needed.
            if inner
                .framebuffer
                .as_ref()
                .is_some_and(|fb| fb.size() != size)
            {
                inner.framebuffer = None;
            }
            let framebuffer = if let Some(fb) = &inner.framebuffer {
                fb
            } else {
                trace!("creating framebuffer texture sized {} × {}", size.w, size.h);
                let renderer = guard.as_mut();
                let texture = renderer.create_buffer(Fourcc::Abgr8888, size)?;
                inner.framebuffer.insert(texture)
            };

            // Prepare blur textures.
            let mut blur = Option::zip(inner.blur.as_mut(), self.blur_options);
            if let Some((b, options)) = &mut blur {
                let renderer = guard.as_mut();
                if let Err(err) = b.prepare_textures(
                    |fourcc, size| renderer.create_buffer(fourcc, size),
                    framebuffer,
                    *options,
                ) {
                    warn!("error preparing blur textures: {err:?}");
                    blur = None;
                }
            }

            // We can't use renderer.with_context() as that will reset the GlesFrame binding that we
            // want to blit from.
            drop(guard);

            // Blit the framebuffer contents.
            frame.with_context(|gl| unsafe {
                while gl.GetError() != ffi::NO_ERROR {}

                let mut current_fbo = 0i32;
                gl.GetIntegerv(ffi::DRAW_FRAMEBUFFER_BINDING, &mut current_fbo as *mut _);

                // BlitFramebuffer is affected by the scissor test, we don't want that.
                gl.Disable(ffi::SCISSOR_TEST);

                let mut fbo = 0;
                gl.GenFramebuffers(1, &mut fbo as *mut _);
                gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, fbo);

                gl.FramebufferTexture2D(
                    ffi::DRAW_FRAMEBUFFER,
                    ffi::COLOR_ATTACHMENT0,
                    ffi::TEXTURE_2D,
                    framebuffer.tex_id(),
                    0,
                );

                gl.BlitFramebuffer(
                    dst.loc.x,
                    dst.loc.y,
                    dst.loc.x + dst.size.w,
                    dst.loc.y + dst.size.h,
                    0,
                    0,
                    size.w,
                    size.h,
                    ffi::COLOR_BUFFER_BIT,
                    ffi::LINEAR,
                );

                // Restore state set by GlesFrame that we just modified.
                gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, current_fbo as u32);
                gl.Enable(ffi::SCISSOR_TEST);

                gl.DeleteFramebuffers(1, &mut fbo as *mut _);

                if gl.GetError() != ffi::NO_ERROR {
                    Err(GlesError::BlitError)
                } else {
                    Ok(())
                }
            })??;

            // If blur is off, use the unblurred texture.
            if self.blur_options.is_none() {
                inner.intermediate = Some(framebuffer.clone());
                return Ok(());
            }

            if let Some((blur, options)) = blur {
                let mut guard = frame.renderer();
                let renderer = guard.as_mut();
                match blur.render(renderer, framebuffer, options) {
                    Ok(blurred) => inner.intermediate = Some(blurred),
                    Err(err) => {
                        warn!("error rendering blur: {err:?}");
                    }
                }
            }

            Ok(())
        })
    }

    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        let Some(cache) = cache else {
            return Ok(());
        };
        let Some(inner) = cache.get::<RefCell<Inner>>() else {
            return Ok(());
        };
        let mut inner = inner.borrow_mut();
        let inner = &mut *inner;

        let Some(texture) = &inner.intermediate else {
            return Ok(());
        };

        // Clamp the same way as in capture_framebuffer().
        let output_rect = Rectangle::from_size(frame.output_size());
        let clamped_dst = match dst.intersection(output_rect) {
            Some(clamped) => clamped,
            None => return Ok(()),
        };
        let clamp_offset = clamped_dst.loc - dst.loc;

        // Filter damage by subregion, reusing the stored Vec to avoid allocation.
        let filtered = &mut inner.subregion_damage;
        filtered.clear();

        if let Some(subregion) = &self.subregion {
            // Convert to subregion coordinates.
            let mut crop = src.to_logical(1., Transform::Normal, &src.size);
            crop.loc += self.geometry.loc;
            subregion.filter_damage(crop, dst, damage, filtered);
        } else {
            filtered.extend(damage.iter());
        };

        // Adjust for clamped dst.
        if clamped_dst != dst {
            let r = Rectangle::new(clamp_offset, clamped_dst.size);
            filtered.retain_mut(|d| {
                if let Some(mut crop) = d.intersection(r) {
                    crop.loc -= clamp_offset;
                    *d = crop;
                    true
                } else {
                    false
                }
            });
        }

        if filtered.is_empty() {
            return Ok(());
        }
        let damage = &filtered[..];

        // Adjust src proportionally to the dst clamping.
        let src_loc = src.loc.to_logical(1., Transform::Normal, &src.size);
        let dst_to_src = src.size / dst.size.to_f64();
        let crop = Rectangle::new(
            src_loc + clamp_offset.to_f64().upscale(dst_to_src).to_logical(1.),
            clamped_dst.size.to_f64().upscale(dst_to_src).to_logical(1.),
        );

        let program = Shaders::get_from_frame(frame).postprocess_and_clip.clone();
        let uniforms = program
            .is_some()
            .then(|| self.compute_uniforms(crop, frame.transformation()));
        let uniforms = uniforms.as_ref().map_or(&[][..], |x| &x[..]);

        frame.render_texture_from_to(
            texture,
            Rectangle::from_size(texture.size().to_f64()),
            clamped_dst,
            damage,
            &[],
            // The intermediate texture has the same transform as the frame.
            frame.transformation().invert(),
            1.,
            program.as_ref(),
            uniforms,
        )
    }
}

impl<'render> RenderElement<TtyRenderer<'render>> for FramebufferEffectElement {
    fn capture_framebuffer(
        &self,
        frame: &mut TtyFrame<'_, '_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        cache: &UserDataMap,
    ) -> Result<(), TtyRendererError<'render>> {
        let gles_frame = frame.as_gles_frame();
        RenderElement::<GlesRenderer>::capture_framebuffer(&self, gles_frame, src, dst, cache)?;
        Ok(())
    }

    fn draw(
        &self,
        frame: &mut TtyFrame<'_, '_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), TtyRendererError<'render>> {
        let gles_frame = frame.as_gles_frame();
        RenderElement::<GlesRenderer>::draw(
            &self,
            gles_frame,
            src,
            dst,
            damage,
            opaque_regions,
            cache,
        )?;
        Ok(())
    }
}

impl Inner {
    fn new(renderer: &mut GlesRenderer) -> Self {
        Inner {
            framebuffer: None,
            blur: Blur::new(renderer),
            intermediate: None,
            subregion_damage: Vec::new(),
        }
    }
}

#[derive(Default)]
pub(crate) struct NativeInner {
    renderer_context: Option<ContextId<VulkanTexture>>,
    pub(crate) framebuffer: Option<VulkanTexture>,
    pub(crate) blur: Option<NativeBlur>,
    pub(crate) intermediate: Option<VulkanTexture>,
    pub(crate) subregion_damage: Vec<Rectangle<i32, Physical>>,
    #[cfg(test)]
    pub(crate) blur_pixels: usize,
}

impl NativeInner {
    /// Damage trackers retain effect caches across renderer replacement. Size and element ID
    /// cannot establish image ownership, so every capture and draw validates the device epoch.
    /// Submitted frames hold independent Arc leases to any images released by this cache.
    fn select_context(&mut self, context: ContextId<VulkanTexture>) -> bool {
        if self.renderer_context.as_ref() == Some(&context) {
            return true;
        }
        *self = Self {
            renderer_context: Some(context),
            ..Self::default()
        };
        false
    }
}

impl RenderElement<NativeRenderer> for FramebufferEffectElement {
    fn capture_framebuffer(
        &self,
        frame: &mut NativeFrame<'_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        cache: &UserDataMap,
    ) -> Result<(), NativeError> {
        let _span = clarion_tracy_span!("FramebufferEffectElement::capture_framebuffer [Vulkan]");
        let output_rect = Rectangle::from_size(frame.output_size());
        let transform = frame.transformation();

        let inner =
            cache.get_or_insert::<RefCell<NativeInner>, _>(|| RefCell::new(NativeInner::default()));
        let mut inner = inner.borrow_mut();
        let inner = &mut *inner;
        inner.select_context(frame.context_id());

        let clamped_dst = match dst.intersection(output_rect) {
            Some(clamped) => clamped,
            None => {
                inner.intermediate = None;
                return Ok(());
            }
        };
        let clamp_scale = clamped_dst.size.to_f64() / dst.size.to_f64();
        let capture_dst = transform.transform_rect_in(clamped_dst, &output_rect.size);

        let size = src
            .size
            .to_logical(1., Transform::Normal)
            .upscale(clamp_scale)
            .to_physical_precise_round(self.scale);
        let size = transform.transform_size(size);
        let size = size.to_logical(1).to_buffer(1, Transform::Normal);

        let blur_region = self.sampled_blur_region(
            src,
            dst,
            clamped_dst,
            transform,
            size,
            &mut inner.subregion_damage,
        );
        #[cfg(test)]
        let blur_region = if self.full_blur_for_test {
            Rectangle::from_size(size)
        } else {
            blur_region
        };
        #[cfg(test)]
        {
            inner.blur_pixels = 0;
        }
        if blur_region.is_empty() {
            // macOS BackdropFreeze: when the backdrop region has no dirty damage and an existing
            // cached blurred texture matches the target size, freeze and reuse it without
            // issuing redundant GPU framebuffer capture and dual-Kawase passes.
            if inner.intermediate.is_some() {
                return Ok(());
            }
            return Ok(());
        }

        if inner
            .framebuffer
            .as_ref()
            .is_some_and(|fb| fb.size() != size)
        {
            inner.framebuffer = None;
            inner.intermediate = None;
        }

        let framebuffer = if let Some(fb) = &inner.framebuffer {
            fb.clone()
        } else {
            trace!(
                "creating Vulkan framebuffer texture sized {} × {}",
                size.w,
                size.h
            );
            let texture = frame.create_buffer(Fourcc::Abgr8888, size)?;
            inner.framebuffer = Some(texture.clone());
            texture
        };

        let dst_buf = Rectangle::new(
            Point::from((capture_dst.loc.x, capture_dst.loc.y)),
            Size::from((capture_dst.size.w, capture_dst.size.h)),
        );
        frame.capture_scaled_into(&framebuffer, dst_buf)?;

        if self.blur_options.is_none() {
            inner.intermediate = Some(framebuffer);
            return Ok(());
        }

        if let Some(options) = self.blur_options {
            // Release the previous intermediate reference before preparing the blur job so that
            // NativeBlur's output texture has strong_count == 1, avoiding reallocation in
            // create_buffer.
            inner.intermediate = None;
            let mut blur = inner.blur.take().unwrap_or_default();
            let job = blur.prepare_in_native_frame(frame, &framebuffer, options)?;
            #[cfg(test)]
            {
                inner.blur_pixels =
                    crate::render_helpers::vulkan::blur::pass_regions(size, options, blur_region)
                        .iter()
                        .map(|r| r.size.w as usize * r.size.h as usize)
                        .sum();
            }
            #[cfg(test)]
            let blurred = if self.full_blur_for_test {
                job.render(frame)?
            } else {
                job.render_region(frame, blur_region)?
            };
            #[cfg(not(test))]
            let blurred = job.render_region(frame, blur_region)?;
            inner.blur = Some(blur);
            inner.intermediate = Some(blurred);
        }

        Ok(())
    }

    fn draw(
        &self,
        frame: &mut NativeFrame<'_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), NativeError> {
        let Some(cache) = cache else {
            return Ok(());
        };
        let Some(inner) = cache.get::<RefCell<NativeInner>>() else {
            return Ok(());
        };
        let mut inner = inner.borrow_mut();
        let inner = &mut *inner;
        if !inner.select_context(frame.context_id()) {
            // A direct draw can arrive before capture when a persistent damage tracker has
            // no new capture for this element. Never sample an old device's cached pixels.
            return Ok(());
        }

        let Some(texture) = &inner.intermediate else {
            return Ok(());
        };

        let output_rect = Rectangle::from_size(frame.output_size());
        let clamped_dst = match dst.intersection(output_rect) {
            Some(clamped) => clamped,
            None => return Ok(()),
        };
        let clamp_offset = clamped_dst.loc - dst.loc;

        let filtered = &mut inner.subregion_damage;
        filtered.clear();

        if let Some(subregion) = &self.subregion {
            let mut crop = src.to_logical(1., Transform::Normal, &src.size);
            crop.loc += self.geometry.loc;
            subregion.filter_damage(crop, dst, damage, filtered);
        } else {
            filtered.extend(damage.iter());
        };

        if clamped_dst != dst {
            let r = Rectangle::new(clamp_offset, clamped_dst.size);
            filtered.retain_mut(|d| {
                if let Some(mut crop) = d.intersection(r) {
                    crop.loc -= clamp_offset;
                    *d = crop;
                    true
                } else {
                    false
                }
            });
        }

        if filtered.is_empty() {
            return Ok(());
        }
        let damage = &filtered[..];

        let src_loc = src.loc.to_logical(1., Transform::Normal, &src.size);
        let dst_to_src = src.size / dst.size.to_f64();
        let crop = Rectangle::new(
            src_loc + clamp_offset.to_f64().upscale(dst_to_src).to_logical(1.),
            clamped_dst.size.to_f64().upscale(dst_to_src).to_logical(1.),
        );

        let clip = self.compute_clip(crop, frame.transformation());
        let postprocess = self.compute_postprocess();

        let old_clip = frame.replace_texture_clip(clip);
        let old_postprocess = frame.replace_postprocess(postprocess);

        let res = frame.render_texture_from_to(
            texture,
            Rectangle::from_size(texture.size().to_f64()),
            clamped_dst,
            damage,
            &[],
            frame.transformation().invert(),
            1.,
        );

        frame.replace_texture_clip(old_clip);
        frame.replace_postprocess(old_postprocess);

        res
    }

    fn underlying_storage(
        &self,
        _renderer: &mut NativeRenderer,
    ) -> Option<smithay::backend::renderer::element::UnderlyingStorage<'_>> {
        None
    }
}

#[cfg(test)]
mod recovery_tests {
    use std::cell::RefCell;
    use std::sync::Arc;

    use smithay::backend::allocator::Fourcc;
    use smithay::backend::renderer::damage::OutputDamageTracker;
    use smithay::backend::renderer::element::{Element, Kind, RenderElement};
    use smithay::backend::renderer::{
        Bind, Color32F, ExportMem, Frame, Offscreen, Renderer, Texture,
    };
    use smithay::utils::user_data::UserDataMap;
    use smithay::utils::{Physical, Rectangle, Scale, Size, Transform};

    use super::{FramebufferEffect, FramebufferEffectElement, NativeInner};
    use crate::render_helpers::background_effect::RenderParams;
    use crate::render_helpers::blur::BlurOptions;
    use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
    use crate::render_helpers::vulkan::hardware_tests::setup;
    use crate::render_helpers::vulkan::native::NativeRenderer;
    use crate::render_helpers::vulkan::VulkanTexture;

    crate::clarion_render_elements! {
        RecoveryElement => {
            Effect = FramebufferEffectElement,
            Solid = SolidColorRenderElement,
        }
    }

    fn element(effect: &FramebufferEffect) -> FramebufferEffectElement {
        effect.render(
            None,
            RenderParams {
                geometry: Rectangle::from_size((64., 48.).into()),
                subregion: None,
                clip: None,
                scale: 1.,
            },
            Some(BlurOptions {
                passes: 2,
                offset: 1.25,
            }),
            0.,
            1.,
        )
    }

    fn pixels(renderer: &mut NativeRenderer, target: &VulkanTexture) -> Vec<u8> {
        let mapping = renderer
            .copy_texture(
                target,
                Rectangle::from_size(target.size()),
                Fourcc::Abgr8888,
            )
            .unwrap();
        renderer.map_texture(&mapping).unwrap().to_vec()
    }

    fn render_tracker(
        renderer: &mut NativeRenderer,
        tracker: &mut OutputDamageTracker,
        effect: &FramebufferEffect,
        color: [f32; 4],
    ) -> Vec<u8> {
        let mut target: VulkanTexture = renderer
            .create_buffer(Fourcc::Abgr8888, (64, 48).into())
            .unwrap();
        let background = SolidColorBuffer::new((64., 48.), color);
        let elements = [
            RecoveryElement::Effect(element(effect)),
            RecoveryElement::Solid(SolidColorRenderElement::from_buffer(
                &background,
                (0., 0.),
                1.,
                Kind::Unspecified,
            )),
        ];
        {
            let mut framebuffer = renderer.bind(&mut target).unwrap();
            let result = tracker
                .render_output(renderer, &mut framebuffer, 0, &elements, Color32F::BLACK)
                .unwrap();
            assert!(
                result.damage.is_some(),
                "recovery must actually redraw the target"
            );
            result.sync.wait().unwrap();
        }
        pixels(renderer, &target)
    }

    fn render_direct(
        renderer: &mut NativeRenderer,
        effect: &FramebufferEffectElement,
        cache: &UserDataMap,
        capture: bool,
        color: [f32; 4],
    ) -> Vec<u8> {
        let mut target: VulkanTexture = renderer
            .create_buffer(Fourcc::Abgr8888, (64, 48).into())
            .unwrap();
        let full = Rectangle::from_size((64, 48).into());
        {
            let mut framebuffer = renderer.bind(&mut target).unwrap();
            let mut frame = renderer
                .render(&mut framebuffer, (64, 48).into(), Transform::Normal)
                .unwrap();
            frame.clear(color.into(), &[full]).unwrap();
            if capture {
                <FramebufferEffectElement as RenderElement<NativeRenderer>>::capture_framebuffer(
                    effect,
                    &mut frame,
                    effect.src(),
                    full,
                    cache,
                )
                .unwrap();
            }
            <FramebufferEffectElement as RenderElement<NativeRenderer>>::draw(
                effect,
                &mut frame,
                effect.src(),
                full,
                &[full],
                &[],
                Some(cache),
            )
            .unwrap();
            frame.finish().unwrap().wait().unwrap();
        }
        pixels(renderer, &target)
    }

    fn assert_color(pixels: &[u8], expected: [u8; 4]) {
        assert!(
            pixels.as_chunks::<4>().0.iter().all(|pixel| {
                pixel
                    .iter()
                    .zip(expected)
                    .all(|(actual, expected)| actual.abs_diff(expected) <= 1)
            }),
            "cached effect must contain this renderer's new pixels"
        );
    }

    #[test]
    #[ignore = "requires a real DRM render node and two native Vulkan contexts"]
    fn native_framebuffer_effect_recovery_reuses_tracker_without_foreign_context() {
        let (vk, reference, allocator) = setup();
        drop(reference);
        drop(allocator);
        let mut previous = NativeRenderer::new(vk).unwrap();
        previous
            .prepare_output_format_for_test(Fourcc::Abgr8888)
            .unwrap();
        let old_context = previous.context_id();
        let old_device = Arc::downgrade(&previous.device_anchor());
        let effect = FramebufferEffect::new();
        let mut tracker = OutputDamageTracker::new(
            Size::<i32, Physical>::from((64, 48)),
            Scale::from(1.),
            Transform::Normal,
        );
        assert_color(
            &render_tracker(&mut previous, &mut tracker, &effect, [1., 0., 0., 1.]),
            [255, 0, 0, 255],
        );
        let direct_cache = UserDataMap::new();
        let direct_element = element(&effect);
        assert_color(
            &render_direct(
                &mut previous,
                &direct_element,
                &direct_cache,
                true,
                [1., 0., 0., 1.],
            ),
            [255, 0, 0, 255],
        );
        drop(previous);
        assert!(
            old_device.upgrade().is_some(),
            "the regression must retain real old-context effect caches"
        );

        let (vk, reference, allocator) = setup();
        drop(reference);
        drop(allocator);
        let mut replacement = NativeRenderer::new(vk).unwrap();
        replacement
            .prepare_output_format_for_test(Fourcc::Abgr8888)
            .unwrap();
        assert_ne!(old_context, replacement.context_id());
        // The draw entry must be safe even when capture has not run for this epoch.
        assert_color(
            &render_direct(
                &mut replacement,
                &direct_element,
                &direct_cache,
                false,
                [0., 1., 0., 1.],
            ),
            [0, 255, 0, 255],
        );
        {
            let inner = direct_cache.get::<RefCell<NativeInner>>().unwrap().borrow();
            assert!(
                inner.framebuffer.is_none() && inner.intermediate.is_none() && inner.blur.is_none()
            );
        }
        // Same effect ID, same size, same damage tracker: size-only cache validation fails here.
        assert_color(
            &render_tracker(&mut replacement, &mut tracker, &effect, [0., 1., 0., 1.]),
            [0, 255, 0, 255],
        );
        // A healthy completion worker may still own its final Arc after the signaled fence.
        // Await actual retirement without using that scheduling detail as a correctness failure.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while old_device.upgrade().is_some() && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(
            old_device.upgrade().is_none(),
            "idle old device must not remain pinned by tracker caches"
        );

        assert_color(
            &render_direct(
                &mut replacement,
                &direct_element,
                &direct_cache,
                true,
                [0., 1., 0., 1.],
            ),
            [0, 255, 0, 255],
        );
        let image = direct_cache
            .get::<RefCell<NativeInner>>()
            .unwrap()
            .borrow()
            .framebuffer
            .as_ref()
            .unwrap()
            .image();
        assert_color(
            &render_direct(
                &mut replacement,
                &direct_element,
                &direct_cache,
                true,
                [0., 0., 1., 1.],
            ),
            [0, 0, 255, 255],
        );
        assert_eq!(
            image,
            direct_cache
                .get::<RefCell<NativeInner>>()
                .unwrap()
                .borrow()
                .framebuffer
                .as_ref()
                .unwrap()
                .image(),
            "unchanged device must reuse its cache instead of reallocating every frame"
        );
    }
}
