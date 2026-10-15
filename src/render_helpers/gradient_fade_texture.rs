use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{
    GlesError, GlesFrame, GlesRenderer, GlesTexProgram, GlesTexture, Uniform,
};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Buffer, Physical, Rectangle, Scale, Transform};

use super::texture::TextureRenderElement;
use super::vulkan::native::{NativeError, NativeFrame, NativeRenderer};
use super::vulkan::VulkanTexture;
use crate::backend::tty::{TtyFrame, TtyRenderer, TtyRendererError};
use crate::render_helpers::renderer::AsGlesFrame as _;
use crate::render_helpers::shaders::Shaders;

#[derive(Debug, Clone)]
pub struct GradientFadeTextureRenderElement {
    inner: TextureRenderElement<GlesTexture>,
    program: GradientFadeShader,
    cutoff: (f32, f32),
}

#[derive(Debug, Clone)]
pub struct GradientFadeShader(GlesTexProgram);

/// Compute the gradient fade cutoff in texture UV space for a possibly right-clipped texture.
///
/// Returns `(1., 1.)` (disabled) when the texture is displayed in full.
fn fade_cutoff<T: smithay::backend::renderer::Texture>(
    texture: &TextureRenderElement<T>,
) -> (f32, f32) {
    let logical_w = texture.buffer().logical_size().w;
    let logical_src_w = texture.logical_src().size.w;
    if logical_src_w < logical_w {
        // Texture is clipped, add a fade.
        let cutoff = 1. - f64::min(18. / logical_src_w, 1.);
        let full = logical_src_w / logical_w;
        ((cutoff * full) as f32, full as f32)
    } else {
        // Texture is displayed full-size, no cutoff necessary.
        (1., 1.)
    }
}

impl GradientFadeTextureRenderElement {
    pub fn new(texture: TextureRenderElement<GlesTexture>, program: GradientFadeShader) -> Self {
        let cutoff = fade_cutoff(&texture);
        Self {
            inner: texture,
            program,
            cutoff,
        }
    }

    pub fn shader(renderer: &mut GlesRenderer) -> Option<GradientFadeShader> {
        let program = Shaders::get(renderer).gradient_fade.clone();
        program.map(GradientFadeShader)
    }
}

impl Element for GradientFadeTextureRenderElement {
    fn id(&self) -> &Id {
        self.inner.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.inner.geometry(scale)
    }

    fn transform(&self) -> Transform {
        self.inner.transform()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.inner.src()
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        self.inner.damage_since(scale, commit)
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        self.inner.opaque_regions(scale)
    }

    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }

    fn kind(&self) -> Kind {
        self.inner.kind()
    }
}

impl RenderElement<GlesRenderer> for GradientFadeTextureRenderElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        let uniforms = vec![Uniform::new("cutoff", self.cutoff)];
        frame.override_default_tex_program(self.program.0.clone(), uniforms);
        RenderElement::<GlesRenderer>::draw(
            &self.inner,
            frame,
            src,
            dst,
            damage,
            opaque_regions,
            cache,
        )?;
        frame.clear_tex_program_override();
        Ok(())
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        // If scanout for things other than Wayland buffers is implemented, this will need to take
        // the target GPU into account.
        None
    }
}

impl<'render> RenderElement<TtyRenderer<'render>> for GradientFadeTextureRenderElement {
    fn draw(
        &self,
        frame: &mut TtyFrame<'render, '_, '_>,
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

    fn underlying_storage(
        &self,
        _renderer: &mut TtyRenderer<'render>,
    ) -> Option<UnderlyingStorage<'_>> {
        // If scanout for things other than Wayland buffers is implemented, this will need to take
        // the target GPU into account.
        None
    }
}

impl
    smithay::backend::renderer::element::RenderElement<
        crate::render_helpers::vulkan::native::NativeRenderer,
    > for GradientFadeTextureRenderElement
{
    fn draw(
        &self,
        _frame: &mut crate::render_helpers::vulkan::native::NativeFrame<'_>,
        _src: Rectangle<f64, Buffer>,
        _dst: Rectangle<i32, Physical>,
        _damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
        _cache: Option<&UserDataMap>,
    ) -> Result<(), crate::render_helpers::vulkan::native::NativeError> {
        Ok(())
    }

    fn underlying_storage(
        &self,
        _renderer: &mut crate::render_helpers::vulkan::native::NativeRenderer,
    ) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

/// Vulkan counterpart of [`GradientFadeTextureRenderElement`].
///
/// The fade is evaluated by the quad fragment shader from the cutoff push constants, so no
/// separate program is needed. Only ever constructed for the Vulkan renderer; the GLES and TTY
/// impls are no-ops to satisfy the render-element enums.
#[derive(Debug, Clone)]
pub struct VulkanGradientFadeTextureRenderElement {
    inner: TextureRenderElement<VulkanTexture>,
    cutoff: (f32, f32),
}

impl VulkanGradientFadeTextureRenderElement {
    pub fn new(texture: TextureRenderElement<VulkanTexture>) -> Self {
        let cutoff = fade_cutoff(&texture);
        Self {
            inner: texture,
            cutoff,
        }
    }
}

impl Element for VulkanGradientFadeTextureRenderElement {
    fn id(&self) -> &Id {
        self.inner.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.inner.geometry(scale)
    }

    fn transform(&self) -> Transform {
        self.inner.transform()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.inner.src()
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        self.inner.damage_since(scale, commit)
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        self.inner.opaque_regions(scale)
    }

    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }

    fn kind(&self) -> Kind {
        self.inner.kind()
    }
}

impl RenderElement<NativeRenderer> for VulkanGradientFadeTextureRenderElement {
    fn draw(
        &self,
        frame: &mut NativeFrame<'_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
        _cache: Option<&UserDataMap>,
    ) -> Result<(), NativeError> {
        if frame.context_id() != *self.inner.buffer().renderer_context_id() {
            warn!("trying to render texture from different renderer");
            return Ok(());
        }

        frame.draw_gradient_fade(
            self.inner.buffer().texture(),
            src,
            dst,
            damage,
            self.inner.buffer().texture_transform(),
            self.inner.alpha(),
            [self.cutoff.0, self.cutoff.1],
        )
    }

    fn underlying_storage(&self, _renderer: &mut NativeRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

impl RenderElement<GlesRenderer> for VulkanGradientFadeTextureRenderElement {
    fn draw(
        &self,
        _frame: &mut GlesFrame<'_, '_>,
        _src: Rectangle<f64, Buffer>,
        _dst: Rectangle<i32, Physical>,
        _damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
        _cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        Ok(())
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

impl<'render> RenderElement<TtyRenderer<'render>> for VulkanGradientFadeTextureRenderElement {
    fn draw(
        &self,
        _frame: &mut TtyFrame<'render, '_, '_>,
        _src: Rectangle<f64, Buffer>,
        _dst: Rectangle<i32, Physical>,
        _damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
        _cache: Option<&UserDataMap>,
    ) -> Result<(), TtyRendererError<'render>> {
        Ok(())
    }

    fn underlying_storage(
        &self,
        _renderer: &mut TtyRenderer<'render>,
    ) -> Option<UnderlyingStorage<'_>> {
        None
    }
}
