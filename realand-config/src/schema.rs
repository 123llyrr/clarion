//! Public configuration metadata for editors and other configuration tools.
//!
//! This describes the supported editor fields, not the complete KDL grammar or
//! validation rules. Use [`crate::Config::parse`] to validate a document. In
//! particular, `Binds` and `Environment` have dynamic child names, scalar decoders
//! may accept aliases, and handwritten decoders cannot be inferred from derives.
//! [`fields`] supplies the curated descriptions for those decoders where a fixed
//! field list exists. UI labels, example values and presentation belong to callers.
//!
//! Module names are logical top-level configuration groups, not source paths:
//! moving a type into a private submodule does not change its group. `Part` types
//! describe an editable configuration fragment; resolved runtime types may differ.

/// Version of the metadata contract, independent of the compositor IPC version.
///
/// Increment for incompatible interpretation or API changes. New configuration
/// types, fields and enum choices may be added without changing this value.
pub const SCHEMA_VERSION: u32 = 1;

/// One argument, property or child exposed by a configuration type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FieldSpec {
    /// KDL name, or the tuple argument's zero-based index.
    pub name: &'static str,
    /// Referenced configuration type or scalar type.
    pub ty: &'static str,
    /// One of `argument`, `property`, or `child`.
    pub kind: &'static str,
    /// Whether the field accepts a scalar directly.
    pub scalar: bool,
    /// Whether the same child may occur more than once.
    pub repeated: bool,
    /// Whether the decoder can accept omission of this field.
    pub optional: bool,
}

/// Generated structural description of a configuration type.
///
/// Prefer [`fields`] for editing: handwritten decoders need additional metadata
/// and may differ from their underlying Rust representation. `variants` contains
/// canonical scalar choices rather than every alias accepted by a custom parser.
#[derive(Debug, PartialEq, Eq)]
pub struct TypeSpec {
    /// Logical configuration group, used to disambiguate names such as `Match`.
    pub module: &'static str,
    /// Type name, or `Enum::Variant` for an enum variant's fields.
    pub name: &'static str,
    /// Fields extracted from the configuration's own decoding declarations.
    pub fields: &'static [FieldSpec],
    /// Canonical unit variant names.
    pub variants: &'static [&'static str],
}

include!(concat!(env!("OUT_DIR"), "/config_schema.rs"));

/// Find the editable description, preferring a fragment (`Part`) type.
pub fn spec(module: &str, name: &str) -> Option<&'static TypeSpec> {
    let name = if name == "WorkspaceLayoutPart" {
        "LayoutPart"
    } else {
        name
    };
    let part = format!("{name}Part");
    TYPES
        .iter()
        .find(|t| t.module == module && t.name == part)
        .or_else(|| TYPES.iter().find(|t| t.module == module && t.name == name))
        .or_else(|| TYPES.iter().find(|t| t.name == part))
        .or_else(|| TYPES.iter().find(|t| t.name == name))
}
fn child(name: &'static str, ty: &'static str, scalar: bool) -> FieldSpec {
    FieldSpec {
        name,
        ty,
        kind: "child",
        scalar,
        repeated: false,
        optional: true,
    }
}
/// Return editor fields, including curated handwritten-decoder descriptions.
///
/// An empty list does not imply that the type accepts no content: dynamically
/// named entries (notably `Binds` and `Environment`) remain caller-managed.
pub fn fields(module: &str, name: &str) -> Vec<FieldSpec> {
    if name == "Binds" || name == "Environment" {
        return vec![];
    }
    if name == "Bind" {
        let mut result = fields("binds", "Action");
        for (n, ty) in [
            ("repeat", "bool"),
            ("cooldown-ms", "u32"),
            ("allow-when-locked", "bool"),
            ("allow-inhibiting", "bool"),
            ("hotkey-overlay-title", "String"),
        ] {
            result.push(FieldSpec {
                kind: "property",
                ..child(n, ty, true)
            });
        }
        return result;
    }
    if name == "RendererPart" {
        return vec![
            FieldSpec {
                kind: "argument",
                ..child("backend", "RendererBackendConfig", true)
            },
            child("backend", "RendererBackendConfig", true),
        ];
    }
    if name.ends_with("Anim") {
        let mut v = vec![
            child("off", "Flag", false),
            child("duration-ms", "u32", true),
            child("curve", "Curve", true),
            child("spring", "SpringParams", false),
        ];
        if name == "WorkspaceSwitchAnim" {
            v.push(child("cube", "WorkspaceCubePart", false));
        }
        if matches!(name, "WindowOpenAnim" | "WindowCloseAnim") {
            v.push(child("glide", "GlidePart", false));
        }
        if matches!(
            name,
            "WindowOpenAnim" | "WindowCloseAnim" | "WindowResizeAnim"
        ) {
            v.push(child("custom-shader", "String", true));
        }
        if name == "WindowCloseAnim" {
            v.push(child("effect", "WindowCloseEffect", true));
        }
        return v;
    }
    if matches!(name, "DefaultPresetSize" | "PresetSize") {
        return vec![
            FieldSpec {
                repeated: name == "PresetSize",
                ..child("proportion", "f64", true)
            },
            FieldSpec {
                repeated: name == "PresetSize",
                ..child("fixed", "i32", true)
            },
        ];
    }
    if name == "SpringParams" {
        return ["damping-ratio", "stiffness", "epsilon"]
            .into_iter()
            .map(|n| FieldSpec {
                kind: "property",
                ..child(n, if n == "stiffness" { "u32" } else { "f64" }, true)
            })
            .collect();
    }
    if name == "Include" {
        return vec![FieldSpec {
            kind: "property",
            ..child("optional", "bool", true)
        }];
    }
    let mut result = spec(module, name)
        .map(|s| s.fields.to_vec())
        .unwrap_or_default();
    if name == "WorkspaceLayoutPart" {
        result.retain(|f| !matches!(f.name, "empty-workspace-above-first" | "insert-hint"));
    }
    result
}
