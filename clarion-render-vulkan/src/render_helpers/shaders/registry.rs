//! Renderer-independent registry and assembly plan for the scene animation
//! shader programs.
//!
//! The GLSL bodies next to this file remain the rendered implementation and are
//! not reproduced, translated or reworded here; they stay Denial-derived and are
//! a source-audit residual. What *is* independent of their text is the calling
//! convention around them:
//!
//! * which animation programs exist and their stable cache identities,
//! * the uniform names/types and sampler bindings each program is compiled against,
//! * the order in which the prelude, body, epilogue and trailing fragments are concatenated,
//! * which body wins when a user shader is configured and which built-in body is the default.
//!
//! Both renderer paths consume this description: the GLES path builds its
//! [`smithay::backend::renderer::gles::UniformName`] list and source from it,
//! and the native path asks for the same assembled source. Previously each
//! `compile_*_program` function, `animation_source` and the native cache key
//! repeated those facts, so a change in one place could silently diverge from
//! the others.
//!
//! This module deliberately depends only on `core`/`alloc`, so its plan and its
//! tests can be compiled and executed without a renderer, a windowing system or
//! Cargo. See `tests/` in the task evidence directory.

/// One of the three animation programs compiled from a prelude, a swappable
/// body and an epilogue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AnimationKind {
    Resize,
    Open,
    Close,
}

impl AnimationKind {
    /// Every animation program in declaration order.
    pub const ALL: [Self; 3] = [Self::Resize, Self::Open, Self::Close];

    /// The stable identity used by the native program cache.
    ///
    /// These values intentionally mirror `ProgramType as u8`; `shaders/mod.rs`
    /// asserts the equality at compile time so a reordering cannot silently
    /// remap a cached program.
    pub const fn key(self) -> u8 {
        match self {
            Self::Resize => 2,
            Self::Close => 3,
            Self::Open => 4,
        }
    }

    /// Inverse of [`Self::key`].
    pub const fn from_key(key: u8) -> Option<Self> {
        match key {
            2 => Some(Self::Resize),
            3 => Some(Self::Close),
            4 => Some(Self::Open),
            _ => None,
        }
    }

    /// A short name for diagnostics and test output.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Resize => "resize",
            Self::Open => "open",
            Self::Close => "close",
        }
    }
}

/// The GLSL type of one declared uniform, restricted to the animation ABI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UniformKind {
    Float,
    Vec2,
    Vec4,
    Mat3,
}

impl UniformKind {
    /// The declaration spelling used by the existing shader fragments.
    pub const fn glsl_type(self) -> &'static str {
        match self {
            Self::Float => "float",
            Self::Vec2 => "vec2",
            Self::Vec4 => "vec4",
            Self::Mat3 => "mat3",
        }
    }
}

/// One named, typed uniform supplied by the program-specific uniform block.
///
/// The element-supplied `clarion_v_coords`, `clarion_size`, `clarion_alpha`,
/// `clarion_scale` and the debug-only `clarion_tint` are intentionally absent:
/// `ShaderRenderElement` owns them for every program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UniformSpec {
    pub name: &'static str,
    pub kind: UniformKind,
}

impl UniformSpec {
    const fn new(name: &'static str, kind: UniformKind) -> Self {
        Self { name, kind }
    }
}

/// A fragment slot that [`assemble`] needs the caller to resolve to text.
///
/// The registry never stores GLSL. It stores which slot exists and where it
/// belongs, and the owning module supplies the `include_str!` text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SourcePart {
    ResizePrelude,
    ResizeDefault,
    ResizeEpilogue,
    RoundingAlpha,
    OpenPrelude,
    OpenDefault,
    OpenEpilogue,
    ClosePrelude,
    CloseDefault,
    CloseEpilogue,
}

impl SourcePart {
    /// The file name this slot is hosted in, relative to this directory.
    pub const fn file(self) -> &'static str {
        match self {
            Self::ResizePrelude => "resize_prelude.frag",
            Self::ResizeDefault => "resize.frag",
            Self::ResizeEpilogue => "resize_epilogue.frag",
            Self::RoundingAlpha => "rounding_alpha.frag",
            Self::OpenPrelude => "open_prelude.frag",
            Self::OpenDefault => "open_default.frag",
            Self::OpenEpilogue => "open_epilogue.frag",
            Self::ClosePrelude => "close_prelude.frag",
            Self::CloseDefault => "close_default.frag",
            Self::CloseEpilogue => "close_epilogue.frag",
        }
    }
}

/// Ordered source composition for one animation program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceLayout {
    pub prelude: SourcePart,
    pub default_body: SourcePart,
    pub epilogue: SourcePart,
    /// Fragments appended after the epilogue in order.
    pub trailing: &'static [SourcePart],
}

/// Complete CPU-side description of one animation program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProgramSpec {
    pub kind: AnimationKind,
    /// Uniforms set from the typed program block.
    pub uniforms: &'static [UniformSpec],
    /// Sampler names bound by name, in binding order.
    pub textures: &'static [&'static str],
    pub layout: SourceLayout,
}

const RESIZE_UNIFORMS: &[UniformSpec] = &[
    UniformSpec::new("clarion_input_to_curr_geo", UniformKind::Mat3),
    UniformSpec::new("clarion_curr_geo_to_prev_geo", UniformKind::Mat3),
    UniformSpec::new("clarion_curr_geo_to_next_geo", UniformKind::Mat3),
    UniformSpec::new("clarion_curr_geo_size", UniformKind::Vec2),
    UniformSpec::new("clarion_geo_to_tex_prev", UniformKind::Mat3),
    UniformSpec::new("clarion_geo_to_tex_next", UniformKind::Mat3),
    UniformSpec::new("clarion_progress", UniformKind::Float),
    UniformSpec::new("clarion_clamped_progress", UniformKind::Float),
    UniformSpec::new("clarion_corner_radius", UniformKind::Vec4),
    UniformSpec::new("clarion_clip_to_geometry", UniformKind::Float),
];
const RESIZE_TEXTURES: &[&str] = &["clarion_tex_prev", "clarion_tex_next"];

const OPEN_UNIFORMS: &[UniformSpec] = &[
    UniformSpec::new("clarion_input_to_geo", UniformKind::Mat3),
    UniformSpec::new("clarion_geo_size", UniformKind::Vec2),
    UniformSpec::new("clarion_geo_to_tex", UniformKind::Mat3),
    UniformSpec::new("clarion_progress", UniformKind::Float),
    UniformSpec::new("clarion_clamped_progress", UniformKind::Float),
    UniformSpec::new("clarion_random_seed", UniformKind::Float),
    UniformSpec::new("clarion_glide_enabled", UniformKind::Float),
    UniformSpec::new("clarion_glide_tilt", UniformKind::Float),
    UniformSpec::new("clarion_glide_depth", UniformKind::Float),
    UniformSpec::new("clarion_popin_ratio", UniformKind::Float),
];
const OPEN_TEXTURES: &[&str] = &["clarion_tex"];

const CLOSE_UNIFORMS: &[UniformSpec] = &[
    UniformSpec::new("clarion_input_to_geo", UniformKind::Mat3),
    UniformSpec::new("clarion_geo_size", UniformKind::Vec2),
    UniformSpec::new("clarion_geo_to_tex", UniformKind::Mat3),
    UniformSpec::new("clarion_progress", UniformKind::Float),
    UniformSpec::new("clarion_clamped_progress", UniformKind::Float),
    UniformSpec::new("clarion_random_seed", UniformKind::Float),
    UniformSpec::new("clarion_glide_enabled", UniformKind::Float),
    UniformSpec::new("clarion_glide_tilt", UniformKind::Float),
    UniformSpec::new("clarion_glide_depth", UniformKind::Float),
];
const CLOSE_TEXTURES: &[&str] = &["clarion_tex"];

/// Resize keeps the rounding helper after its epilogue.
pub const RESIZE: ProgramSpec = ProgramSpec {
    kind: AnimationKind::Resize,
    uniforms: RESIZE_UNIFORMS,
    textures: RESIZE_TEXTURES,
    layout: SourceLayout {
        prelude: SourcePart::ResizePrelude,
        default_body: SourcePart::ResizeDefault,
        epilogue: SourcePart::ResizeEpilogue,
        trailing: &[SourcePart::RoundingAlpha],
    },
};

/// Open and close share the same prelude/epilogue shape without trailing
/// fragments.
pub const OPEN: ProgramSpec = ProgramSpec {
    kind: AnimationKind::Open,
    uniforms: OPEN_UNIFORMS,
    textures: OPEN_TEXTURES,
    layout: SourceLayout {
        prelude: SourcePart::OpenPrelude,
        default_body: SourcePart::OpenDefault,
        epilogue: SourcePart::OpenEpilogue,
        trailing: &[],
    },
};

pub const CLOSE: ProgramSpec = ProgramSpec {
    kind: AnimationKind::Close,
    uniforms: CLOSE_UNIFORMS,
    textures: CLOSE_TEXTURES,
    layout: SourceLayout {
        prelude: SourcePart::ClosePrelude,
        default_body: SourcePart::CloseDefault,
        epilogue: SourcePart::CloseEpilogue,
        trailing: &[],
    },
};

/// The registry proper, in declaration order.
pub static ALL: [ProgramSpec; 3] = [RESIZE, OPEN, CLOSE];

/// Every registered program.
pub fn all() -> &'static [ProgramSpec] {
    &ALL
}

/// Look up the registered spec for a kind.
pub const fn spec(kind: AnimationKind) -> &'static ProgramSpec {
    match kind {
        AnimationKind::Resize => &RESIZE,
        AnimationKind::Open => &OPEN,
        AnimationKind::Close => &CLOSE,
    }
}

/// Look up a registered spec by its native cache key.
pub fn spec_for_key(key: u8) -> Option<&'static ProgramSpec> {
    AnimationKind::from_key(key).map(spec)
}

/// Concatenate one program's source in the registered order.
///
/// `custom_body` is a configured user shader or a built-in effect body; when it
/// is absent the registered default body is used. `part_text` resolves the
/// static fragments and is supplied by the owning module, which keeps this
/// module free of GLSL text and of `include_str!`.
pub fn assemble(
    layout: SourceLayout,
    custom_body: Option<&str>,
    part_text: impl Fn(SourcePart) -> &'static str,
) -> String {
    let body = match custom_body {
        Some(body) => body,
        None => part_text(layout.default_body),
    };

    let mut source = String::new();
    source.push_str(part_text(layout.prelude));
    source.push_str(body);
    source.push_str(part_text(layout.epilogue));
    for part in layout.trailing {
        source.push_str(part_text(*part));
    }
    source
}

/// Close-specific body precedence: an explicit user shader wins over the
/// configured effect body, and `None` means the default close body.
pub fn close_body<'a>(custom: Option<&'a str>, effect: Option<&'a str>) -> Option<&'a str> {
    custom.or(effect)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn part_text(part: SourcePart) -> &'static str {
        match part {
            SourcePart::ResizePrelude => include_str!("resize_prelude.frag"),
            SourcePart::ResizeDefault => include_str!("resize.frag"),
            SourcePart::ResizeEpilogue => include_str!("resize_epilogue.frag"),
            SourcePart::RoundingAlpha => include_str!("rounding_alpha.frag"),
            SourcePart::OpenPrelude => include_str!("open_prelude.frag"),
            SourcePart::OpenDefault => include_str!("open_default.frag"),
            SourcePart::OpenEpilogue => include_str!("open_epilogue.frag"),
            SourcePart::ClosePrelude => include_str!("close_prelude.frag"),
            SourcePart::CloseDefault => include_str!("close_default.frag"),
            SourcePart::CloseEpilogue => include_str!("close_epilogue.frag"),
        }
    }

    /// Parse bare `uniform <type> <name>;` declarations. Blocks, arrays and
    /// preprocessor branches are not part of the animation ABI.
    fn declared_uniforms(source: &str) -> Vec<(&str, &str)> {
        source
            .lines()
            .filter_map(|line| {
                let declaration = line.trim().strip_prefix("uniform ")?;
                let declaration = declaration.strip_suffix(';')?;
                let mut words = declaration.split_whitespace();
                let ty = words.next()?;
                let name = words.next()?;
                words.next().is_none().then_some((name, ty))
            })
            .collect()
    }

    #[test]
    fn every_kind_is_registered_exactly_once() {
        let kinds = all().iter().map(|spec| spec.kind).collect::<Vec<_>>();
        assert_eq!(kinds, AnimationKind::ALL);
        assert_eq!(all().len(), 3);
    }

    #[test]
    fn keys_are_stable_unique_and_roundtrip() {
        let mut keys = HashSet::new();
        for kind in AnimationKind::ALL {
            let key = kind.key();
            assert!(keys.insert(key), "duplicate cache key for {}", kind.label());
            assert_eq!(AnimationKind::from_key(key), Some(kind));
            assert_eq!(spec_for_key(key).map(|spec| spec.kind), Some(kind));
        }
        assert_eq!(AnimationKind::Resize.key(), 2);
        assert_eq!(AnimationKind::Close.key(), 3);
        assert_eq!(AnimationKind::Open.key(), 4);
        assert_eq!(AnimationKind::from_key(0), None);
        assert_eq!(AnimationKind::from_key(1), None);
        assert_eq!(AnimationKind::from_key(5), None);
        assert_eq!(AnimationKind::from_key(u8::MAX), None);
    }

    #[test]
    fn abi_names_are_unique_within_each_program() {
        for spec in all() {
            let mut names = HashSet::new();
            for uniform in spec.uniforms {
                assert!(
                    names.insert(uniform.name),
                    "duplicate uniform {} in {}",
                    uniform.name,
                    spec.kind.label()
                );
            }
            for texture in spec.textures {
                assert!(
                    names.insert(*texture),
                    "sampler {texture} collides with a uniform in {}",
                    spec.kind.label()
                );
            }
        }
    }

    #[test]
    fn abi_order_and_types_match_the_existing_compile_contract() {
        let resize = spec(AnimationKind::Resize)
            .uniforms
            .iter()
            .map(|uniform| (uniform.name, uniform.kind))
            .collect::<Vec<_>>();
        assert_eq!(
            resize,
            vec![
                ("clarion_input_to_curr_geo", UniformKind::Mat3),
                ("clarion_curr_geo_to_prev_geo", UniformKind::Mat3),
                ("clarion_curr_geo_to_next_geo", UniformKind::Mat3),
                ("clarion_curr_geo_size", UniformKind::Vec2),
                ("clarion_geo_to_tex_prev", UniformKind::Mat3),
                ("clarion_geo_to_tex_next", UniformKind::Mat3),
                ("clarion_progress", UniformKind::Float),
                ("clarion_clamped_progress", UniformKind::Float),
                ("clarion_corner_radius", UniformKind::Vec4),
                ("clarion_clip_to_geometry", UniformKind::Float),
            ]
        );

        let open = spec(AnimationKind::Open)
            .uniforms
            .iter()
            .map(|uniform| (uniform.name, uniform.kind))
            .collect::<Vec<_>>();
        assert_eq!(
            open,
            vec![
                ("clarion_input_to_geo", UniformKind::Mat3),
                ("clarion_geo_size", UniformKind::Vec2),
                ("clarion_geo_to_tex", UniformKind::Mat3),
                ("clarion_progress", UniformKind::Float),
                ("clarion_clamped_progress", UniformKind::Float),
                ("clarion_random_seed", UniformKind::Float),
                ("clarion_glide_enabled", UniformKind::Float),
                ("clarion_glide_tilt", UniformKind::Float),
                ("clarion_glide_depth", UniformKind::Float),
                ("clarion_popin_ratio", UniformKind::Float),
            ]
        );

        let close = spec(AnimationKind::Close)
            .uniforms
            .iter()
            .map(|uniform| (uniform.name, uniform.kind))
            .collect::<Vec<_>>();
        assert_eq!(
            close,
            vec![
                ("clarion_input_to_geo", UniformKind::Mat3),
                ("clarion_geo_size", UniformKind::Vec2),
                ("clarion_geo_to_tex", UniformKind::Mat3),
                ("clarion_progress", UniformKind::Float),
                ("clarion_clamped_progress", UniformKind::Float),
                ("clarion_random_seed", UniformKind::Float),
                ("clarion_glide_enabled", UniformKind::Float),
                ("clarion_glide_tilt", UniformKind::Float),
                ("clarion_glide_depth", UniformKind::Float),
            ]
        );
    }

    #[test]
    fn textures_match_the_existing_sampler_bindings() {
        assert_eq!(
            spec(AnimationKind::Resize).textures,
            &["clarion_tex_prev", "clarion_tex_next"]
        );
        assert_eq!(spec(AnimationKind::Open).textures, &["clarion_tex"]);
        assert_eq!(spec(AnimationKind::Close).textures, &["clarion_tex"]);
    }

    #[test]
    fn every_registered_uniform_and_sampler_is_declared_by_its_fragments() {
        for spec in all() {
            let mut source = String::new();
            source.push_str(part_text(spec.layout.prelude));
            source.push_str(part_text(spec.layout.default_body));
            source.push_str(part_text(spec.layout.epilogue));
            for part in spec.layout.trailing {
                source.push_str(part_text(*part));
            }
            let declared = declared_uniforms(&source);

            for uniform in spec.uniforms {
                assert!(
                    declared.contains(&(uniform.name, uniform.kind.glsl_type())),
                    "{} declares no `uniform {} {}`",
                    spec.kind.label(),
                    uniform.kind.glsl_type(),
                    uniform.name
                );
            }
            for texture in spec.textures {
                assert!(
                    declared.contains(&(*texture, "sampler2D")),
                    "{} declares no `uniform sampler2D {texture}`",
                    spec.kind.label()
                );
            }
        }
    }

    #[test]
    fn layouts_place_the_rounding_helper_only_after_resize() {
        assert_eq!(
            spec(AnimationKind::Resize).layout.trailing,
            &[SourcePart::RoundingAlpha]
        );
        assert!(spec(AnimationKind::Open).layout.trailing.is_empty());
        assert!(spec(AnimationKind::Close).layout.trailing.is_empty());
        assert_eq!(
            spec(AnimationKind::Resize).layout.default_body,
            SourcePart::ResizeDefault
        );
        assert_eq!(
            spec(AnimationKind::Open).layout.default_body,
            SourcePart::OpenDefault
        );
        assert_eq!(
            spec(AnimationKind::Close).layout.default_body,
            SourcePart::CloseDefault
        );
    }

    #[test]
    fn assemble_uses_the_registered_order_and_default_body() {
        for kind in AnimationKind::ALL {
            let layout = spec(kind).layout;
            let assembled = assemble(layout, None, part_text);
            assert!(assembled.starts_with(part_text(layout.prelude)));
            assert!(assembled.contains(part_text(layout.default_body)));
            assert!(assembled.contains(part_text(layout.epilogue)));
            let expected_len = part_text(layout.prelude).len()
                + part_text(layout.default_body).len()
                + part_text(layout.epilogue).len()
                + layout
                    .trailing
                    .iter()
                    .map(|part| part_text(*part).len())
                    .sum::<usize>();
            assert_eq!(assembled.len(), expected_len);
            for part in layout.trailing {
                assert!(assembled.ends_with(part_text(*part)));
            }
        }
    }

    #[test]
    fn assemble_prefers_a_custom_body_over_the_registered_default() {
        let layout = spec(AnimationKind::Open).layout;
        let assembled = assemble(layout, Some("CUSTOM_BODY"), part_text);
        let expected = format!(
            "{}{}{}",
            part_text(layout.prelude),
            "CUSTOM_BODY",
            part_text(layout.epilogue)
        );
        assert_eq!(assembled, expected);
        assert!(!assembled.contains(part_text(layout.default_body)));
    }

    #[test]
    fn close_body_precedence_is_custom_then_effect_then_default() {
        assert_eq!(close_body(Some("custom"), Some("effect")), Some("custom"));
        assert_eq!(close_body(None, Some("effect")), Some("effect"));
        assert_eq!(close_body(Some("custom"), None), Some("custom"));
        assert_eq!(close_body(None, None), None);
        // The default body is selected by `assemble`, not by this policy.
        let layout = spec(AnimationKind::Close).layout;
        assert!(assemble(layout, close_body(None, None), part_text)
            .contains(part_text(SourcePart::CloseDefault)));
    }

    #[test]
    fn uniform_kind_spellings_are_the_fragment_spellings() {
        assert_eq!(UniformKind::Float.glsl_type(), "float");
        assert_eq!(UniformKind::Vec2.glsl_type(), "vec2");
        assert_eq!(UniformKind::Vec4.glsl_type(), "vec4");
        assert_eq!(UniformKind::Mat3.glsl_type(), "mat3");
    }

    #[test]
    fn source_parts_name_existing_fragment_files() {
        let parts = [
            SourcePart::ResizePrelude,
            SourcePart::ResizeDefault,
            SourcePart::ResizeEpilogue,
            SourcePart::RoundingAlpha,
            SourcePart::OpenPrelude,
            SourcePart::OpenDefault,
            SourcePart::OpenEpilogue,
            SourcePart::ClosePrelude,
            SourcePart::CloseDefault,
            SourcePart::CloseEpilogue,
        ];
        let files = parts.iter().map(|part| part.file()).collect::<HashSet<_>>();
        assert_eq!(files.len(), parts.len());
        for spec in all() {
            assert!(files.contains(spec.layout.prelude.file()));
            assert!(files.contains(spec.layout.default_body.file()));
            assert!(files.contains(spec.layout.epilogue.file()));
        }
    }
}
