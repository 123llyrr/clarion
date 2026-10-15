use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{GlesError, GlesFrame, GlesRenderer, GlesTexture};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Buffer, Physical, Rectangle, Scale, Transform};

use super::renderer::AsGlesFrame;
use super::texture::TextureRenderElement;
use super::vulkan::native::{NativeError, NativeFrame, NativeRenderer};
use super::vulkan::VulkanTexture;
use crate::backend::tty::{TtyFrame, TtyRenderer, TtyRendererError};

/// Wrapper for a texture from the primary GPU for rendering with the primary GPU.
#[derive(Debug, Clone)]
pub enum PrimaryGpuTextureRenderElement {
    Gles(TextureRenderElement<GlesTexture>),
    Vulkan(TextureRenderElement<VulkanTexture>),
}

impl From<TextureRenderElement<GlesTexture>> for PrimaryGpuTextureRenderElement {
    fn from(elem: TextureRenderElement<GlesTexture>) -> Self {
        Self::Gles(elem)
    }
}

impl From<TextureRenderElement<VulkanTexture>> for PrimaryGpuTextureRenderElement {
    fn from(elem: TextureRenderElement<VulkanTexture>) -> Self {
        Self::Vulkan(elem)
    }
}

impl PrimaryGpuTextureRenderElement {
    pub fn texture_scale(&self) -> Scale<f64> {
        match self {
            Self::Gles(elem) => elem.buffer().texture_scale(),
            Self::Vulkan(elem) => elem.buffer().texture_scale(),
        }
    }
}

impl Element for PrimaryGpuTextureRenderElement {
    fn id(&self) -> &Id {
        match self {
            Self::Gles(elem) => elem.id(),
            Self::Vulkan(elem) => elem.id(),
        }
    }

    fn current_commit(&self) -> CommitCounter {
        match self {
            Self::Gles(elem) => elem.current_commit(),
            Self::Vulkan(elem) => elem.current_commit(),
        }
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        match self {
            Self::Gles(elem) => elem.geometry(scale),
            Self::Vulkan(elem) => elem.geometry(scale),
        }
    }

    fn transform(&self) -> Transform {
        match self {
            Self::Gles(elem) => elem.transform(),
            Self::Vulkan(elem) => elem.transform(),
        }
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        match self {
            Self::Gles(elem) => elem.src(),
            Self::Vulkan(elem) => elem.src(),
        }
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        match self {
            Self::Gles(elem) => elem.damage_since(scale, commit),
            Self::Vulkan(elem) => elem.damage_since(scale, commit),
        }
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        match self {
            Self::Gles(elem) => elem.opaque_regions(scale),
            Self::Vulkan(elem) => elem.opaque_regions(scale),
        }
    }

    fn alpha(&self) -> f32 {
        match self {
            Self::Gles(elem) => elem.alpha(),
            Self::Vulkan(elem) => elem.alpha(),
        }
    }

    fn kind(&self) -> Kind {
        match self {
            Self::Gles(elem) => elem.kind(),
            Self::Vulkan(elem) => elem.kind(),
        }
    }
}

impl RenderElement<GlesRenderer> for PrimaryGpuTextureRenderElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        match self {
            Self::Gles(elem) => {
                let gles_frame = frame.as_gles_frame();
                RenderElement::<GlesRenderer>::draw(
                    elem,
                    gles_frame,
                    src,
                    dst,
                    damage,
                    opaque_regions,
                    cache,
                )
            }
            Self::Vulkan(_) => Ok(()),
        }
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

impl<'render> RenderElement<TtyRenderer<'render>> for PrimaryGpuTextureRenderElement {
    fn draw(
        &self,
        frame: &mut TtyFrame<'_, '_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), TtyRendererError<'render>> {
        match self {
            Self::Gles(elem) => {
                let gles_frame = frame.as_gles_frame();
                RenderElement::<GlesRenderer>::draw(
                    elem,
                    gles_frame,
                    src,
                    dst,
                    damage,
                    opaque_regions,
                    cache,
                )?;
                Ok(())
            }
            Self::Vulkan(_) => Ok(()),
        }
    }

    fn underlying_storage(
        &self,
        _renderer: &mut TtyRenderer<'render>,
    ) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

impl RenderElement<NativeRenderer> for PrimaryGpuTextureRenderElement {
    fn draw(
        &self,
        frame: &mut NativeFrame<'_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), NativeError> {
        match self {
            Self::Vulkan(elem) => RenderElement::<NativeRenderer>::draw(
                elem,
                frame,
                src,
                dst,
                damage,
                opaque_regions,
                cache,
            ),
            Self::Gles(_) => Ok(()),
        }
    }

    fn underlying_storage(&self, _renderer: &mut NativeRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}
