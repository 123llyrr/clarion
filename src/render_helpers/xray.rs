use std::array;
use std::cell::RefCell;
use std::rc::Rc;

use glam::{Mat3, Vec2};
use realand_config::CornerRadius;
use smithay::backend::renderer::element::{Element, Id, RenderElement};
use smithay::backend::renderer::gles::{
    GlesError, GlesFrame, GlesRenderer, GlesTexProgram, Uniform,
};
use smithay::backend::renderer::utils::{CommitCounter, OpaqueRegions};
use smithay::backend::renderer::Color32F;
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Size, Transform};

use crate::backend::tty::{TtyFrame, TtyRenderer, TtyRendererError};
use crate::render_helpers::background_effect::RenderParams;
use crate::render_helpers::effect_buffer::EffectBuffer;
use crate::render_helpers::renderer::AsGlesFrame as _;
use crate::render_helpers::shaders::{mat3_uniform, Shaders};
use crate::render_helpers::vulkan::native::{NativeError, NativeFrame, NativeRenderer};
use crate::render_helpers::{RenderCtx, RenderTarget};
use crate::utils::region::TransformedRegion;

#[derive(Debug)]
pub struct Xray {
    // The buffers are per-render-target to avoid constant rerendering when screencasting.
    pub background: [Rc<RefCell<EffectBuffer>>; RenderTarget::COUNT],
    pub backdrop: [Rc<RefCell<EffectBuffer>>; RenderTarget::COUNT],
    pub backdrop_color: Color32F,
    pub workspaces: Vec<(Rectangle<f64, Logical>, Color32F)>,
}

/// Position for drawing xray background.
#[derive(Debug, Clone, Copy)]
pub struct XrayPos {
    /// Position of geometry relative to the backdrop in zoomed coordinates.
    ///
    /// Should be upscaled by `zoom` to get position in backdrop coordinates.
    pub pos_in_backdrop: Point<f64, Logical>,

    /// Zoom factor between backdrop coordinates and geometry.
    pub zoom: f64,
}

impl XrayPos {
    pub fn new(pos_in_backdrop: Point<f64, Logical>, zoom: f64) -> Self {
        Self {
            pos_in_backdrop: pos_in_backdrop.downscale(zoom),
            zoom,
        }
    }

    pub fn offset(mut self, offset: Point<f64, Logical>) -> Self {
        self.pos_in_backdrop += offset;
        self
    }
}

impl Default for XrayPos {
    fn default() -> Self {
        Self {
            pos_in_backdrop: Point::new(0., 0.),
            zoom: 1.,
        }
    }
}

#[derive(Debug)]
pub struct XrayElement {
    buffer: Rc<RefCell<EffectBuffer>>,
    id: Id,
    geometry: Rectangle<f64, Logical>,
    src: Rectangle<f64, Buffer>,
    subregion: Option<TransformedRegion>,
    input_to_clip_geo: Mat3,
    clip_geo_size: Vec2,
    corner_radius: CornerRadius,
    scale: f32,
    blur: bool,
    noise: f32,
    saturation: f32,
    bg_color: Color32F,
    program: Option<GlesTexProgram>,
}

impl Default for Xray {
    fn default() -> Self {
        Self::new()
    }
}

impl Xray {
    pub fn new() -> Self {
        Self {
            background: array::from_fn(|_| Rc::new(RefCell::new(EffectBuffer::new()))),
            backdrop: array::from_fn(|_| Rc::new(RefCell::new(EffectBuffer::new()))),
            backdrop_color: Color32F::TRANSPARENT,
            workspaces: Vec::new(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render<R: crate::render_helpers::renderer::RealandRenderer>(
        &self,
        ctx: RenderCtx<R>,
        params: RenderParams,
        xray_pos: XrayPos,
        blur: bool,
        noise: f32,
        saturation: f32,
        push: &mut dyn FnMut(XrayElement),
    ) {
        let program = ctx
            .renderer
            .as_gles_renderer()
            .and_then(|renderer| Shaders::get(renderer).postprocess_and_clip.clone());

        let zoom = xray_pos.zoom;
        let pos_in_backdrop = xray_pos.pos_in_backdrop.upscale(zoom);

        let (clip_geo, corner_radius) = params
            .clip
            .unwrap_or((params.geometry, CornerRadius::default()));

        let clip_offset = clip_geo.loc - params.geometry.loc;
        let clip_pos_in_backdrop = pos_in_backdrop + clip_offset.upscale(zoom);

        let geo_in_backdrop = Rectangle::new(pos_in_backdrop, params.geometry.size.upscale(zoom));

        let mut backdrop = self.backdrop[ctx.target as usize].borrow_mut();
        let backdrop_geo = Rectangle::from_size(backdrop.logical_size());
        let intersection_with_backdrop = backdrop_geo.intersection(geo_in_backdrop);

        let mut skip_backdrop = intersection_with_backdrop.is_none();

        let mut background = self.background[ctx.target as usize].borrow_mut();
        let prev = background.commit();
        if background.prepare_any(ctx.renderer, blur) {
            if background.commit() != prev {
                trace!("background damaged");
            }

            let clip_geo_size = Vec2::new(clip_geo.size.w as f32, clip_geo.size.h as f32);
            let buf_size = background.logical_size();

            for (ws_geo, bg_color) in &self.workspaces {
                // If the background color is opaque, check if the workspace fully covers the
                // element. In this case, we will skip the backdrop element since it's fully
                // covered.
                //
                // FIXME: also implement some way to check if the background elements are fully
                // covered in opaque regions, and not just the realand background color is opaque
                let crop = if bg_color.is_opaque() && ws_geo.contains_rect(geo_in_backdrop) {
                    skip_backdrop = true;
                    // No need to intersect, we know it's fully covered.
                    Some(geo_in_backdrop)
                } else {
                    ws_geo.intersection(geo_in_backdrop)
                };

                let Some(crop) = crop else {
                    continue;
                };

                // If crop contains the intersection with backdrop, then the workspace fully
                // covers the backdrop, so we can skip the backdrop.
                //
                // This can happen when the overview is closed (so workspaces align left/right with
                // the backdrop) and the window is peeking out off screen to the side. In this
                // case, this off-screen part is on top of nothing, neither workspace nor backdrop,
                // but since the window doesn't fully cover the workspace, the check above doesn't
                // skip the backdrop.
                if bg_color.is_opaque()
                    && intersection_with_backdrop
                        .is_some_and(|backdrop| crop.contains_rect(backdrop))
                {
                    skip_backdrop = true;
                }

                // This can be different from zoom for surfaces that do not scale with
                // workspaces, e.g. layer-shell top and overlay layer.
                let ws_zoom = ws_geo.size / buf_size;

                let src = Rectangle::new(crop.loc - ws_geo.loc, crop.size).downscale(ws_zoom);
                let src = src.to_buffer(background.scale(), Transform::Normal, &buf_size);

                let buf_size = Vec2::new(buf_size.w as f32, buf_size.h as f32);
                let pos_against_buf = (clip_pos_in_backdrop - ws_geo.loc).downscale(ws_zoom);
                let pos_against_buf = Vec2::new(pos_against_buf.x as f32, pos_against_buf.y as f32);
                let ws_zoom_vec = Vec2::new(ws_zoom.x as f32, ws_zoom.y as f32);
                let input_to_clip_geo = Mat3::from_scale(ws_zoom_vec / zoom as f32)
                    * Mat3::from_scale(buf_size / clip_geo_size)
                    * Mat3::from_translation(-pos_against_buf / buf_size);

                let mut geometry =
                    Rectangle::new(crop.loc - geo_in_backdrop.loc, crop.size).downscale(zoom);
                geometry.loc += params.geometry.loc;

                let elem = XrayElement {
                    buffer: self.background[ctx.target as usize].clone(),
                    id: background.id().clone(),
                    geometry,
                    src,
                    subregion: params.subregion.clone(),
                    input_to_clip_geo,
                    clip_geo_size,
                    corner_radius,
                    scale: params.scale as f32,
                    blur,
                    noise,
                    saturation,
                    bg_color: *bg_color,
                    program: program.clone(),
                };
                push(elem);
            }
        }

        // If the backdrop is fully covered by opaque background, we can skip it.
        if skip_backdrop {
            return;
        }

        let prev = backdrop.commit();
        if backdrop.prepare_any(ctx.renderer, blur) {
            if backdrop.commit() != prev {
                trace!("backdrop damaged");
            }

            let buf_size = backdrop.logical_size();
            let src = geo_in_backdrop.to_buffer(backdrop.scale(), Transform::Normal, &buf_size);

            let mut clip_geo_in_backdrop = Rectangle::new(clip_offset, clip_geo.size).upscale(zoom);
            clip_geo_in_backdrop.loc += geo_in_backdrop.loc;

            let clip_pos_in_backdrop = Vec2::new(
                clip_geo_in_backdrop.loc.x as f32,
                clip_geo_in_backdrop.loc.y as f32,
            );
            let clip_geo_size = Vec2::new(
                clip_geo_in_backdrop.size.w as f32,
                clip_geo_in_backdrop.size.h as f32,
            );

            let buf_size = Vec2::new(buf_size.w as f32, buf_size.h as f32);
            let input_to_clip_geo = Mat3::from_scale(buf_size / clip_geo_size)
                * Mat3::from_translation(-clip_pos_in_backdrop / buf_size);

            let elem = XrayElement {
                buffer: self.backdrop[ctx.target as usize].clone(),
                id: backdrop.id().clone(),
                geometry: params.geometry,
                src,
                subregion: params.subregion.clone(),
                input_to_clip_geo,
                clip_geo_size,
                corner_radius: corner_radius.scaled_by(zoom as f32),
                scale: params.scale as f32,
                blur,
                noise,
                saturation,
                bg_color: self.backdrop_color,
                program: program.clone(),
            };
            push(elem);
        }
    }
}

impl XrayElement {
    fn compute_uniforms(&self) -> [Uniform<'static>; 8] {
        [
            Uniform::new("realand_scale", self.scale),
            Uniform::new("geo_size", <[f32; 2]>::from(self.clip_geo_size)),
            Uniform::new("corner_radius", <[f32; 4]>::from(self.corner_radius)),
            mat3_uniform("input_to_geo", self.input_to_clip_geo),
            Uniform::new("noise", self.noise),
            Uniform::new("saturation", self.saturation),
            Uniform::new("bg_color", self.bg_color.components()),
            Uniform::new("liquid_glass", [0f32; 2]),
        ]
    }
}

impl Element for XrayElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.buffer.borrow().commit()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.src
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.geometry.to_physical_precise_round(scale)
    }

    fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        // FIXME: if bg_color alpha is 1 then compute opaque regions here taking corners into
        // account
        OpaqueRegions::default()
    }
}

impl RenderElement<GlesRenderer> for XrayElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
        _cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        let mut buffer = self.buffer.borrow_mut();
        let texture = match buffer.render(frame, self.blur) {
            Ok(x) => x,
            Err(err) => {
                warn!("error rendering effect buffer: {err:?}");
                return Ok(());
            }
        };

        // FIXME: avoid reallocating a fresh Vec here somehow.
        let mut filtered_damage = Vec::new();
        let damage = if let Some(subregion) = &self.subregion {
            let src_to_geo = self.geometry.size / self.src.size;

            // Compute crop in geometry coordinates.
            let mut crop = src;
            crop.loc -= self.src.loc;
            crop = crop.upscale(src_to_geo);
            let mut crop = crop.to_logical(1., Transform::Normal, &Size::default());

            // Then convert to subregion coordinates.
            crop.loc += self.geometry.loc;

            subregion.filter_damage(crop, dst, damage, &mut filtered_damage);

            if filtered_damage.is_empty() {
                return Ok(());
            }
            &filtered_damage[..]
        } else {
            damage
        };

        let uniforms = self.program.is_some().then(|| self.compute_uniforms());
        let uniforms = uniforms.as_ref().map_or(&[][..], |x| &x[..]);

        frame.render_texture_from_to(
            &texture,
            src,
            dst,
            damage,
            // FIXME: opaque regions need to be filtered like damage.
            &[],
            Transform::Normal,
            1.,
            self.program.as_ref(),
            uniforms,
        )
    }
}

impl<'render> RenderElement<TtyRenderer<'render>> for XrayElement {
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

impl RenderElement<NativeRenderer> for XrayElement {
    fn draw(
        &self,
        frame: &mut NativeFrame<'_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
        _cache: Option<&UserDataMap>,
    ) -> Result<(), NativeError> {
        use smithay::backend::renderer::Frame as _;
        let buffer = self.buffer.borrow();
        let Some(texture) = buffer.native_texture(self.blur) else {
            return Ok(());
        };
        let mut filtered_damage = Vec::new();
        let damage = if let Some(subregion) = &self.subregion {
            let src_to_geo = self.geometry.size / self.src.size;
            let mut crop = src;
            crop.loc -= self.src.loc;
            crop = crop.upscale(src_to_geo);
            let mut crop = crop.to_logical(1., Transform::Normal, &Size::default());
            crop.loc += self.geometry.loc;
            subregion.filter_damage(crop, dst, damage, &mut filtered_damage);
            &filtered_damage[..]
        } else {
            damage
        };
        let clip = super::vulkan::pipeline::TextureClip {
            matrix: self
                .input_to_clip_geo
                .to_cols_array_2d()
                .map(|[x, y, z]| [x, y, z, 0.]),
            radius: self.corner_radius.into(),
            size: self.clip_geo_size.to_array(),
            scale: self.scale,
            enabled: 1.,
        };
        let old_clip = frame.replace_texture_clip(clip);
        let old_post = frame.replace_postprocess(super::vulkan::native::Postprocess {
            noise: self.noise,
            saturation: self.saturation,
            bg_color: self.bg_color.components(),
        });
        let result =
            frame.render_texture_from_to(texture, src, dst, damage, &[], Transform::Normal, 1.);
        frame.replace_texture_clip(old_clip);
        frame.replace_postprocess(old_post);
        result
    }

    fn underlying_storage(
        &self,
        _renderer: &mut NativeRenderer,
    ) -> Option<smithay::backend::renderer::element::UnderlyingStorage<'_>> {
        None
    }
}
