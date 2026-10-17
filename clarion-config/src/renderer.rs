use knuffel::errors::DecodeError;
use miette::miette;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RendererBackendConfig {
    Gles,
    #[default]
    Vulkan,
}

impl RendererBackendConfig {
    pub fn is_vulkan(self) -> bool {
        matches!(self, Self::Vulkan)
    }

    pub fn is_gles(self) -> bool {
        matches!(self, Self::Gles)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gles => "gles",
            Self::Vulkan => "vulkan",
        }
    }
}

impl std::fmt::Display for RendererBackendConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for RendererBackendConfig {
    type Err = miette::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "gles" | "opengl" | "gl" => Ok(Self::Gles),
            "vulkan" | "vk" => Ok(Self::Vulkan),
            _ => Err(miette!(
                r#"invalid renderer backend `{s}`, expected "vulkan" or "gles""#
            )),
        }
    }
}

impl<S: knuffel::traits::ErrorSpan> knuffel::DecodeScalar<S> for RendererBackendConfig {
    fn type_check(
        type_name: &Option<knuffel::span::Spanned<knuffel::ast::TypeName, S>>,
        ctx: &mut knuffel::decode::Context<S>,
    ) {
        if let Some(type_name) = type_name {
            ctx.emit_error(DecodeError::unexpected(
                type_name,
                "type name",
                "no type name expected for this node",
            ));
        }
    }

    fn raw_decode(
        value: &knuffel::span::Spanned<knuffel::ast::Literal, S>,
        ctx: &mut knuffel::decode::Context<S>,
    ) -> Result<Self, DecodeError<S>> {
        match &**value {
            knuffel::ast::Literal::String(s) => s
                .parse()
                .map_err(|e: miette::Error| DecodeError::conversion(value, e.to_string())),
            _ => {
                ctx.emit_error(DecodeError::scalar_kind(
                    knuffel::decode::Kind::String,
                    value,
                ));
                Ok(Self::default())
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RendererPart {
    pub backend: Option<RendererBackendConfig>,
}

impl<S: knuffel::traits::ErrorSpan> knuffel::Decode<S> for RendererPart {
    fn decode_node(
        node: &knuffel::ast::SpannedNode<S>,
        ctx: &mut knuffel::decode::Context<S>,
    ) -> Result<Self, knuffel::errors::DecodeError<S>> {
        // 1. Try decoding from the first argument (e.g. `renderer "vulkan"` or `renderer-backend
        //    "gles"`)
        if let Some(arg) = node.arguments.first() {
            let backend = knuffel::traits::DecodeScalar::decode(arg, ctx)?;
            return Ok(Self {
                backend: Some(backend),
            });
        }

        // 2. Try decoding from child node (e.g. `renderer { backend "vulkan" }`)
        for child in node.children() {
            let name = &**child.node_name;
            if matches!(name, "backend" | "renderer" | "type") {
                if let Some(arg) = child.arguments.first() {
                    let backend = knuffel::traits::DecodeScalar::decode(arg, ctx)?;
                    return Ok(Self {
                        backend: Some(backend),
                    });
                }
            }
        }

        Err(knuffel::errors::DecodeError::missing(
            node,
            "expected renderer backend (e.g. `renderer \"vulkan\"` or `renderer { backend \"vulkan\"; }`)",
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::Config;

    #[test]
    fn test_parse_renderer_top_level() {
        let config = Config::parse(Path::new("test"), r#"renderer "vulkan""#)
            .config
            .unwrap();
        assert_eq!(config.renderer, Some(RendererBackendConfig::Vulkan));
        assert_eq!(
            config.renderer_backend(),
            Some(RendererBackendConfig::Vulkan)
        );

        let config = Config::parse(Path::new("test"), r#"renderer "gles""#)
            .config
            .unwrap();
        assert_eq!(config.renderer, Some(RendererBackendConfig::Gles));
        assert_eq!(config.renderer_backend(), Some(RendererBackendConfig::Gles));
    }

    #[test]
    fn test_parse_renderer_backend_alias() {
        let config = Config::parse(Path::new("test"), r#"renderer-backend "vulkan""#)
            .config
            .unwrap();
        assert_eq!(config.renderer, Some(RendererBackendConfig::Vulkan));
        assert_eq!(
            config.renderer_backend(),
            Some(RendererBackendConfig::Vulkan)
        );

        let config = Config::parse(Path::new("test"), r#"renderer "vk""#)
            .config
            .unwrap();
        assert_eq!(config.renderer, Some(RendererBackendConfig::Vulkan));

        let config = Config::parse(Path::new("test"), r#"renderer "opengl""#)
            .config
            .unwrap();
        assert_eq!(config.renderer, Some(RendererBackendConfig::Gles));
    }

    #[test]
    fn test_parse_renderer_block() {
        let config = Config::parse(Path::new("test"), r#"renderer { backend "vulkan"; }"#)
            .config
            .unwrap();
        assert_eq!(config.renderer, Some(RendererBackendConfig::Vulkan));
        assert_eq!(
            config.renderer_backend(),
            Some(RendererBackendConfig::Vulkan)
        );
    }

    #[test]
    fn test_parse_debug_renderer() {
        let config = Config::parse(Path::new("test"), r#"debug { renderer "vulkan"; }"#)
            .config
            .unwrap();
        assert_eq!(config.debug.renderer, Some(RendererBackendConfig::Vulkan));
        assert_eq!(
            config.renderer_backend(),
            Some(RendererBackendConfig::Vulkan)
        );

        let config = Config::parse(Path::new("test"), r#"debug { renderer-backend "gles"; }"#)
            .config
            .unwrap();
        assert_eq!(config.debug.renderer, Some(RendererBackendConfig::Gles));
        assert_eq!(config.renderer_backend(), Some(RendererBackendConfig::Gles));
    }

    #[test]
    fn test_override_precedence() {
        // Top-level renderer takes precedence over debug.renderer
        let config = Config::parse(
            Path::new("test"),
            r#"
            renderer "vulkan"
            debug { renderer "gles"; }
            "#,
        )
        .config
        .unwrap();
        assert_eq!(config.renderer, Some(RendererBackendConfig::Vulkan));
        assert_eq!(config.debug.renderer, Some(RendererBackendConfig::Gles));
        assert_eq!(
            config.renderer_backend(),
            Some(RendererBackendConfig::Vulkan)
        );
    }
}
