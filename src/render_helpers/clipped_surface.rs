use glam::{Mat3, Vec2};
use realand_config::CornerRadius;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{
    GlesError, GlesFrame, GlesRenderer, GlesTexProgram, Uniform,
};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::backend::renderer::{buffer_y_inverted, ImportAll, Renderer};
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Size, Transform};

use super::damage::ExtraDamage;
use super::renderer::{AsGlesFrame as _, AsGlesRenderer, RealandRenderer};
use super::shaders::{mat3_uniform, Shaders};
use crate::backend::tty::{TtyFrame, TtyRenderer, TtyRendererError};

/// Keeps the native surface type independent of GLES shader handles.
pub trait ClipRenderer: Renderer + ImportAll {
    type ClipProgram: Clone + std::fmt::Debug;

    fn clip_program(&mut self) -> Option<Self::ClipProgram>;
}
impl ClipRenderer for GlesRenderer {
    type ClipProgram = GlesTexProgram;

    fn clip_program(&mut self) -> Option<Self::ClipProgram> {
        Shaders::get(self).clipped_surface.clone()
    }
}
impl<'a> ClipRenderer for TtyRenderer<'a> {
    type ClipProgram = GlesTexProgram;

    fn clip_program(&mut self) -> Option<Self::ClipProgram> {
        self.as_gles_renderer()
            .and_then(|gles| Shaders::get(gles).clipped_surface.clone())
    }
}
impl ClipRenderer for super::vulkan::native::NativeRenderer {
    type ClipProgram = ();

    fn clip_program(&mut self) -> Option<Self::ClipProgram> {
        Some(())
    }
}

#[derive(Debug)]
pub struct ClippedSurfaceRenderElement<R: ClipRenderer> {
    inner: WaylandSurfaceRenderElement<R>,
    program: R::ClipProgram,
    corner_radius: CornerRadius,
    geometry: Rectangle<f64, Logical>,
    scale: f32,
    rounding_power: f32,
}

#[derive(Debug, Default, Clone)]
pub struct RoundedCornerDamage {
    damage: ExtraDamage,
    corner_radius: CornerRadius,
}

impl<R: ClipRenderer> ClippedSurfaceRenderElement<R> {
    pub fn new(
        elem: WaylandSurfaceRenderElement<R>,
        scale: Scale<f64>,
        geometry: Rectangle<f64, Logical>,
        program: R::ClipProgram,
        corner_radius: CornerRadius,
    ) -> Self {
        Self {
            inner: elem,
            program,
            corner_radius,
            geometry,
            scale: scale.x as f32,
            rounding_power: 2.,
        }
    }

    pub fn with_rounding_power(mut self, power: f32) -> Self {
        self.rounding_power = power.max(2.);
        self
    }

    fn clip_matrix(&self) -> Mat3 {
        let scale = Scale::from(f64::from(self.scale));
        let elem_geo = self.inner.geometry(scale);

        let elem_geo_loc = Vec2::new(elem_geo.loc.x as f32, elem_geo.loc.y as f32);
        let elem_geo_size = Vec2::new(elem_geo.size.w as f32, elem_geo.size.h as f32);

        let geo = self.geometry.to_physical_precise_round(scale);
        let geo_loc = Vec2::new(geo.loc.x, geo.loc.y);
        let geo_size = Vec2::new(geo.size.w, geo.size.h);

        let buf_size = self.inner.buffer_size();
        let buf_size = Vec2::new(buf_size.w as f32, buf_size.h as f32);

        let view = self.inner.view();
        let src_loc = Vec2::new(view.src.loc.x as f32, view.src.loc.y as f32);
        let src_size = Vec2::new(view.src.size.w as f32, view.src.size.h as f32);

        let transform = self.inner.transform();
        // HACK: ??? for some reason flipped ones are fine.
        let transform = match transform {
            Transform::_90 => Transform::_270,
            Transform::_270 => Transform::_90,
            x => x,
        };
        let transform_matrix = Mat3::from_translation(Vec2::new(0.5, 0.5))
            * transform.matrix()
            * Mat3::from_translation(-Vec2::new(0.5, 0.5));

        let y_invert = if buffer_y_inverted(self.inner.buffer()).unwrap_or(false) {
            Mat3::from_scale(Vec2::new(1., -1.))
        } else {
            Mat3::IDENTITY
        };

        transform_matrix * Mat3::from_scale(elem_geo_size / geo_size)
            * Mat3::from_translation((elem_geo_loc - geo_loc) / elem_geo_size)
            // Apply viewporter src.
            * Mat3::from_scale(buf_size / src_size)
            * Mat3::from_translation(-src_loc / buf_size)
            * y_invert
    }

    fn compute_uniforms(&self) -> Vec<Uniform<'static>> {
        let input_to_geo = self.clip_matrix();
        let geo_size = (self.geometry.size.w as f32, self.geometry.size.h as f32);

        vec![
            Uniform::new("realand_scale", self.scale),
            Uniform::new("geo_size", geo_size),
            Uniform::new("corner_radius", <[f32; 4]>::from(self.corner_radius)),
            mat3_uniform("input_to_geo", input_to_geo),
            Uniform::new("rounding_power", self.rounding_power),
        ]
    }

    pub fn clip_program(renderer: &mut R) -> Option<R::ClipProgram> {
        renderer.clip_program()
    }

    pub fn shader(renderer: &mut R) -> Option<&GlesTexProgram>
    where
        R: RealandRenderer,
    {
        renderer
            .as_gles_renderer()
            .and_then(|gles| Shaders::get(gles).clipped_surface.as_ref())
    }

    pub fn will_clip(
        elem: &WaylandSurfaceRenderElement<R>,
        scale: Scale<f64>,
        geometry: Rectangle<f64, Logical>,
        corner_radius: CornerRadius,
    ) -> bool {
        let elem_geo = elem.geometry(scale);
        let geo = geometry.to_physical_precise_round(scale);

        if corner_radius == CornerRadius::default() {
            !geo.contains_rect(elem_geo)
        } else {
            let corners = Self::rounded_corners(geometry, corner_radius);
            let corners = corners
                .into_iter()
                .map(|rect| rect.to_physical_precise_up(scale));
            let geo = Rectangle::subtract_rects_many([geo], corners);
            !Rectangle::subtract_rects_many([elem_geo], geo).is_empty()
        }
    }

    fn rounded_corners(
        geo: Rectangle<f64, Logical>,
        corner_radius: CornerRadius,
    ) -> [Rectangle<f64, Logical>; 4] {
        let top_left = corner_radius.top_left as f64;
        let top_right = corner_radius.top_right as f64;
        let bottom_right = corner_radius.bottom_right as f64;
        let bottom_left = corner_radius.bottom_left as f64;

        [
            Rectangle::new(geo.loc, Size::from((top_left, top_left))),
            Rectangle::new(
                Point::from((geo.loc.x + geo.size.w - top_right, geo.loc.y)),
                Size::from((top_right, top_right)),
            ),
            Rectangle::new(
                Point::from((
                    geo.loc.x + geo.size.w - bottom_right,
                    geo.loc.y + geo.size.h - bottom_right,
                )),
                Size::from((bottom_right, bottom_right)),
            ),
            Rectangle::new(
                Point::from((geo.loc.x, geo.loc.y + geo.size.h - bottom_left)),
                Size::from((bottom_left, bottom_left)),
            ),
        ]
    }
}

impl<R: ClipRenderer> Element for ClippedSurfaceRenderElement<R> {
    fn id(&self) -> &Id {
        self.inner.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.inner.geometry(scale)
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.inner.src()
    }

    fn transform(&self) -> Transform {
        self.inner.transform()
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        // FIXME: radius changes need to cause damage.
        let damage = self.inner.damage_since(scale, commit);

        // Intersect with geometry, since we're clipping by it.
        let mut geo = self.geometry.to_physical_precise_round(scale);
        geo.loc -= self.geometry(scale).loc;
        damage
            .into_iter()
            .filter_map(|rect| rect.intersection(geo))
            .collect()
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        let regions = self.inner.opaque_regions(scale);

        // Intersect with geometry, since we're clipping by it.
        let mut geo = self.geometry.to_physical_precise_round(scale);
        geo.loc -= self.geometry(scale).loc;
        let regions = regions
            .into_iter()
            .filter_map(|rect| rect.intersection(geo));

        // Subtract the rounded corners.
        if self.corner_radius == CornerRadius::default() {
            regions.collect()
        } else {
            let corners = Self::rounded_corners(self.geometry, self.corner_radius);

            let elem_loc = self.geometry(scale).loc;
            let corners = corners.into_iter().map(|rect| {
                let mut rect = rect.to_physical_precise_up(scale);
                rect.loc -= elem_loc;
                rect
            });

            OpaqueRegions::from_slice(&Rectangle::subtract_rects_many(regions, corners))
        }
    }

    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }

    fn kind(&self) -> Kind {
        // Clipped/rounded pixels no longer match the client buffer; never scan it out.
        Kind::Unspecified
    }
}

impl RenderElement<GlesRenderer> for ClippedSurfaceRenderElement<GlesRenderer> {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        frame.override_default_tex_program(self.program.clone(), self.compute_uniforms());
        let result = RenderElement::<GlesRenderer>::draw(
            &self.inner,
            frame,
            src,
            dst,
            damage,
            opaque_regions,
            cache,
        );
        frame.clear_tex_program_override();
        result
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        // If scanout for things other than Wayland buffers is implemented, this will need to take
        // the target GPU into account.
        None
    }
}

impl<'render> RenderElement<TtyRenderer<'render>>
    for ClippedSurfaceRenderElement<TtyRenderer<'render>>
{
    fn draw(
        &self,
        frame: &mut TtyFrame<'render, '_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), TtyRendererError<'render>> {
        frame
            .as_gles_frame()
            .override_default_tex_program(self.program.clone(), self.compute_uniforms());
        let result =
            RenderElement::draw(&self.inner, frame, src, dst, damage, opaque_regions, cache);
        frame.as_gles_frame().clear_tex_program_override();
        result
    }

    fn underlying_storage(
        &self,
        _renderer: &mut TtyRenderer<'render>,
    ) -> Option<UnderlyingStorage<'_>> {
        // If scanout for things other than Wayland buffers is implemented, this will need to take
        // the target GPU into account.
        None
    }
}

impl RenderElement<super::vulkan::native::NativeRenderer>
    for ClippedSurfaceRenderElement<super::vulkan::native::NativeRenderer>
{
    fn draw(
        &self,
        frame: &mut super::vulkan::native::NativeFrame<'_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), super::vulkan::native::NativeError> {
        if self.geometry.size.is_empty() {
            return Ok(());
        }
        let mut matrix = self
            .clip_matrix()
            .to_cols_array_2d()
            .map(|[x, y, z]| [x, y, z, 0.]);
        matrix[0][3] = self.rounding_power;
        let clip = super::vulkan::pipeline::TextureClip {
            matrix,
            radius: self.corner_radius.into(),
            size: [self.geometry.size.w as f32, self.geometry.size.h as f32],
            scale: self.scale,
            enabled: 1.,
        };
        let previous = frame.replace_texture_clip(clip);
        let result = RenderElement::<super::vulkan::native::NativeRenderer>::draw(
            &self.inner,
            frame,
            src,
            dst,
            damage,
            opaque_regions,
            cache,
        );
        frame.replace_texture_clip(previous);
        result
    }

    fn underlying_storage(
        &self,
        _renderer: &mut super::vulkan::native::NativeRenderer,
    ) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

impl RoundedCornerDamage {
    pub fn set_corner_radius(&mut self, corner_radius: CornerRadius) {
        if self.corner_radius == corner_radius {
            return;
        }

        // FIXME: make the damage granular.
        self.corner_radius = corner_radius;
        self.damage.damage_all();
    }

    pub fn render(&self, geometry: Rectangle<f64, Logical>) -> ExtraDamage {
        self.damage.render(geometry)
    }
}
