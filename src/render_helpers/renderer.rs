use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::renderer::gles::{GlesFrame, GlesRenderer};
use smithay::backend::renderer::{Bind, ExportMem, ImportAll, ImportMem, Renderer, Texture};

use super::clipped_surface::ClipRenderer;
use super::vulkan::native::NativeRenderer;
use crate::backend::tty::{TtyFrame, TtyRenderer};

/// Trait with our main renderer requirements to save on the typing.
pub trait RealandRenderer:
    ImportAll
    + ImportMem
    + ExportMem
    + Bind<Dmabuf>
    + Renderer<TextureId = Self::RealandTextureId, Error = Self::RealandError>
    + AsGlesRenderer
    + AsNativeRenderer
    + ClipRenderer
{
    // Associated types to work around the instability of associated type bounds.
    type RealandTextureId: Texture + Clone + Send + 'static;
    type RealandError: std::error::Error + Send + Sync + 'static;
}

impl<R> RealandRenderer for R
where
    R: ImportAll
        + ImportMem
        + ExportMem
        + Bind<Dmabuf>
        + AsGlesRenderer
        + AsNativeRenderer
        + ClipRenderer,
    R::TextureId: Texture + Clone + Send + 'static,
    R::Error: std::error::Error + Send + Sync + 'static,
{
    type RealandTextureId = R::TextureId;
    type RealandError = R::Error;
}

/// Backend capabilities needed by layout decorations, independent of GLES access.
pub trait DecorationRenderer: Renderer {
    fn supports_border(&mut self) -> bool;
    fn supports_shadow(&mut self) -> bool;
}

impl<R: RealandRenderer> DecorationRenderer for R {
    fn supports_border(&mut self) -> bool {
        if let Some(gles) = self.as_gles_renderer() {
            super::shaders::Shaders::get(gles)
                .program(super::shaders::ProgramType::Border)
                .is_some()
        } else {
            true
        }
    }
    fn supports_shadow(&mut self) -> bool {
        if let Some(gles) = self.as_gles_renderer() {
            super::shaders::Shaders::get(gles)
                .program(super::shaders::ProgramType::Shadow)
                .is_some()
        } else {
            true
        }
    }
}

/// Trait for getting the underlying `GlesRenderer`, if available.
pub trait AsGlesRenderer {
    fn as_gles_renderer(&mut self) -> Option<&mut GlesRenderer> {
        None
    }
}

impl AsGlesRenderer for GlesRenderer {
    fn as_gles_renderer(&mut self) -> Option<&mut GlesRenderer> {
        Some(self)
    }
}

impl AsGlesRenderer for TtyRenderer<'_> {
    fn as_gles_renderer(&mut self) -> Option<&mut GlesRenderer> {
        Some(self.as_mut())
    }
}

impl AsGlesRenderer for super::vulkan::native::NativeRenderer {
    fn as_gles_renderer(&mut self) -> Option<&mut GlesRenderer> {
        None
    }
}

/// Trait for getting the underlying `NativeRenderer`, if available.
pub trait AsNativeRenderer {
    fn as_native_renderer(&mut self) -> Option<&mut NativeRenderer> {
        None
    }
}

impl AsNativeRenderer for GlesRenderer {}
impl AsNativeRenderer for TtyRenderer<'_> {}
impl AsNativeRenderer for NativeRenderer {
    fn as_native_renderer(&mut self) -> Option<&mut NativeRenderer> {
        Some(self)
    }
}

/// Trait for getting the underlying `GlesFrame`.
pub trait AsGlesFrame<'frame, 'buffer>
where
    Self: 'frame,
{
    fn as_gles_frame(&mut self) -> &mut GlesFrame<'frame, 'buffer>;
}

impl<'frame, 'buffer> AsGlesFrame<'frame, 'buffer> for GlesFrame<'frame, 'buffer> {
    fn as_gles_frame(&mut self) -> &mut GlesFrame<'frame, 'buffer> {
        self
    }
}

impl<'frame, 'buffer> AsGlesFrame<'frame, 'buffer> for TtyFrame<'_, 'frame, 'buffer> {
    fn as_gles_frame(&mut self) -> &mut GlesFrame<'frame, 'buffer> {
        self.as_mut()
    }
}

/// Indicator of which graphics rendering backend is configured/active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RendererBackendType {
    #[default]
    Gles,
    Vulkan,
}

impl RendererBackendType {
    /// Resolve the renderer backend following precedence:
    /// 1. Explicit CLI argument (`--renderer <vulkan|gles>`)
    /// 2. Config file (`renderer "vulkan"` or `debug { renderer "vulkan" }`)
    /// 3. Environment variable `REALAND_RENDERER`
    /// 4. Default (`Gles`)
    pub fn resolve(cli_renderer: Option<Self>, config: &realand_config::Config) -> Self {
        if let Some(cli) = cli_renderer {
            return cli;
        }

        if let Some(cfg) = config.renderer_backend() {
            return Self::from_config(cfg);
        }

        Self::from_env()
    }

    pub fn from_config(cfg: realand_config::RendererBackendConfig) -> Self {
        match cfg {
            realand_config::RendererBackendConfig::Gles => Self::Gles,
            realand_config::RendererBackendConfig::Vulkan => Self::Vulkan,
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "vulkan" | "vk" => Ok(Self::Vulkan),
            "gles" | "opengl" | "gl" => Ok(Self::Gles),
            _ => Err(format!(
                r#"invalid renderer backend `{s}`, expected "vulkan" or "gles""#
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gles => "gles",
            Self::Vulkan => "vulkan",
        }
    }

    /// Detect configured renderer backend from environment variable (`REALAND_RENDERER`).
    /// Defaults to `Gles` if unset or invalid.
    pub fn from_env() -> Self {
        match std::env::var("REALAND_RENDERER") {
            Ok(val) => match Self::parse(&val) {
                Ok(backend) => backend,
                Err(err) => {
                    warn!("{err}; using gles");
                    Self::Gles
                }
            },
            Err(_) => Self::Gles,
        }
    }

    #[inline]
    pub fn is_vulkan(self) -> bool {
        matches!(self, Self::Vulkan)
    }

    #[inline]
    pub fn is_gles(self) -> bool {
        matches!(self, Self::Gles)
    }
}

impl std::fmt::Display for RendererBackendType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Dynamic wrapper representing any supported rendering backend instance.
pub enum AnyRenderer<'a> {
    Gles(&'a mut GlesRenderer),
    Native(&'a mut crate::render_helpers::vulkan::native::NativeRenderer),
}

impl<'a> AnyRenderer<'a> {
    #[inline]
    pub fn backend_type(&self) -> RendererBackendType {
        match self {
            AnyRenderer::Gles(_) => RendererBackendType::Gles,
            AnyRenderer::Native(_) => RendererBackendType::Vulkan,
        }
    }

    #[inline]
    pub fn as_gles(&mut self) -> Option<&mut GlesRenderer> {
        match self {
            AnyRenderer::Gles(r) => Some(r),
            AnyRenderer::Native(_) => None,
        }
    }

    #[inline]
    pub fn as_native(
        &mut self,
    ) -> Option<&mut crate::render_helpers::vulkan::native::NativeRenderer> {
        match self {
            AnyRenderer::Gles(_) => None,
            AnyRenderer::Native(r) => Some(r),
        }
    }

    #[inline]
    pub fn as_vulkan(&mut self) -> Option<&mut crate::render_helpers::vulkan::VulkanRenderer> {
        match self {
            AnyRenderer::Gles(_) => None,
            AnyRenderer::Native(r) => Some(r.vulkan_mut()),
        }
    }
}

/// Trait for getting an `AnyRenderer` view.
pub trait AsAnyRenderer {
    fn as_any_renderer(&mut self) -> AnyRenderer<'_>;
}

impl AsAnyRenderer for GlesRenderer {
    fn as_any_renderer(&mut self) -> AnyRenderer<'_> {
        AnyRenderer::Gles(self)
    }
}

impl AsAnyRenderer for TtyRenderer<'_> {
    fn as_any_renderer(&mut self) -> AnyRenderer<'_> {
        AnyRenderer::Gles(self.as_mut())
    }
}

impl AsAnyRenderer for crate::render_helpers::vulkan::native::NativeRenderer {
    fn as_any_renderer(&mut self) -> AnyRenderer<'_> {
        AnyRenderer::Native(self)
    }
}

/// Dynamic wrapper representing an in-flight frame across backends.
pub enum AnyFrame<'a, 'frame, 'buffer> {
    Gles(&'a mut GlesFrame<'frame, 'buffer>),
    Vulkan(&'a mut crate::render_helpers::vulkan::VulkanFrame<'frame>),
}

impl<'a, 'frame, 'buffer> AnyFrame<'a, 'frame, 'buffer> {
    #[inline]
    pub fn as_gles(&mut self) -> Option<&mut GlesFrame<'frame, 'buffer>> {
        match self {
            AnyFrame::Gles(f) => Some(f),
            AnyFrame::Vulkan(_) => None,
        }
    }

    #[inline]
    pub fn as_vulkan(&mut self) -> Option<&mut crate::render_helpers::vulkan::VulkanFrame<'frame>> {
        match self {
            AnyFrame::Gles(_) => None,
            AnyFrame::Vulkan(f) => Some(f),
        }
    }
}

/// Trait for getting an `AnyFrame` view.
pub trait AsAnyFrame<'frame, 'buffer>
where
    Self: 'frame,
{
    fn as_any_frame<'a>(&'a mut self) -> AnyFrame<'a, 'frame, 'buffer>;
}

impl<'frame, 'buffer> AsAnyFrame<'frame, 'buffer> for GlesFrame<'frame, 'buffer> {
    fn as_any_frame<'a>(&'a mut self) -> AnyFrame<'a, 'frame, 'buffer> {
        AnyFrame::Gles(self)
    }
}

impl<'frame, 'buffer> AsAnyFrame<'frame, 'buffer> for TtyFrame<'_, 'frame, 'buffer> {
    fn as_any_frame<'a>(&'a mut self) -> AnyFrame<'a, 'frame, 'buffer> {
        AnyFrame::Gles(self.as_mut())
    }
}

impl<'frame, 'buffer> AsAnyFrame<'frame, 'buffer>
    for crate::render_helpers::vulkan::VulkanFrame<'frame>
{
    fn as_any_frame<'a>(&'a mut self) -> AnyFrame<'a, 'frame, 'buffer> {
        AnyFrame::Vulkan(self)
    }
}

/// Dispatch a renderer-generic operation without silently omitting the native backend.
#[macro_export]
macro_rules! with_any_renderer {
    ($backend:expr, |$renderer:ident| $body:block) => {
        $backend.with_renderer(|renderer| match renderer {
            $crate::render_helpers::renderer::AnyRenderer::Gles($renderer) => $body,
            $crate::render_helpers::renderer::AnyRenderer::Native($renderer) => $body,
        })
    };
}

/// Dispatch an output-specific operation while preserving the renderer selected for that output.
#[macro_export]
macro_rules! with_any_renderer_for_output {
    ($backend:expr, $output:expr, |$renderer:ident| $body:block) => {
        $backend.with_renderer_for_output(Some($output), |renderer| match renderer {
            $crate::render_helpers::renderer::AnyRenderer::Gles($renderer) => $body,
            $crate::render_helpers::renderer::AnyRenderer::Native($renderer) => $body,
        })
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use realand_config::Config;
    use std::path::Path;

    #[test]
    fn parse_rejects_unknown_backends() {
        assert!(RendererBackendType::parse("potato").is_err());
        assert_eq!(
            RendererBackendType::parse("vk").unwrap(),
            RendererBackendType::Vulkan
        );
        assert_eq!(
            RendererBackendType::parse("opengl").unwrap(),
            RendererBackendType::Gles
        );
    }

    #[test]
    fn cli_renderer_wins_over_config() {
        let config = Config::parse(Path::new("test"), r#"renderer "gles""#)
            .config
            .unwrap();
        assert_eq!(
            RendererBackendType::resolve(Some(RendererBackendType::Vulkan), &config),
            RendererBackendType::Vulkan
        );
        assert_eq!(
            RendererBackendType::resolve(None, &config),
            RendererBackendType::Gles
        );
    }
}
