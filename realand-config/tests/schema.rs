use realand_config::schema::{self, FieldSpec, TypeSpec};

// Captured from the settings-owned generator before this API was moved. Keeping
// this as data tests every field/variant and its order without retaining a second
// implementation of the generator in the settings crate.
mod before {
    use super::{FieldSpec, TypeSpec};
    include!("fixtures/schema-v1.rs");
}

#[test]
fn public_catalog_preserves_previous_editor_contract() {
    assert_eq!(schema::SCHEMA_VERSION, 1);
    // Approved additive extensions: minimize, window-open styles, and glass.
    // Compare every pre-existing type, field, variant, and ordering exactly.
    let minimized = schema::spec("binds", "Action::MinimizeWindow").unwrap();
    assert_eq!(
        minimized,
        &TypeSpec {
            module: "binds",
            name: "Action::MinimizeWindow",
            fields: &[],
            variants: &[],
        }
    );
    let actual: Vec<_> = schema::TYPES
        .iter()
        .filter(|spec| {
            spec.name != "Action::MinimizeWindow"
                && !(spec.module == "animations"
                    && matches!(
                        spec.name,
                        "WindowOpenEffect" | "WindowOpenEffect::Fade" | "WindowOpenEffect::Reveal"
                    ))
        })
        .collect();
    assert_eq!(schema::TYPES.len(), before::TYPES.len() + 4);
    assert_eq!(actual.len(), before::TYPES.len());
    for (actual, previous) in actual.into_iter().zip(before::TYPES) {
        if actual.module == "binds" && actual.name == "Action" {
            let added: Vec<_> = actual
                .fields
                .iter()
                .filter(|field| field.name == "minimize-window")
                .copied()
                .collect();
            assert_eq!(
                added,
                [FieldSpec {
                    name: "minimize-window",
                    ty: "Action::MinimizeWindow",
                    kind: "child",
                    scalar: false,
                    repeated: true,
                    optional: true,
                }]
            );
            assert_eq!(
                (actual.module, actual.name),
                (previous.module, previous.name)
            );
            let fields: Vec<_> = actual
                .fields
                .iter()
                .filter(|field| field.name != "minimize-window")
                .collect();
            assert_eq!(fields, previous.fields.iter().collect::<Vec<_>>());
            let variants: Vec<_> = actual
                .variants
                .iter()
                .filter(|name| **name != "minimize-window")
                .collect();
            assert_eq!(variants, previous.variants.iter().collect::<Vec<_>>());
            assert_eq!(
                actual
                    .variants
                    .iter()
                    .filter(|name| **name == "minimize-window")
                    .count(),
                1
            );
        } else if actual.module == "appearance" && actual.name == "BackgroundEffectRule" {
            let added: Vec<_> = actual
                .fields
                .iter()
                .filter(|field| field.name == "liquid-glass")
                .copied()
                .collect();
            assert_eq!(
                added,
                [FieldSpec {
                    name: "liquid-glass",
                    ty: "bool",
                    kind: "child",
                    scalar: true,
                    repeated: false,
                    optional: true,
                }]
            );
            let existing: Vec<_> = actual
                .fields
                .iter()
                .filter(|field| field.name != "liquid-glass")
                .collect();
            assert_eq!(existing, previous.fields.iter().collect::<Vec<_>>());
            assert_eq!(actual.variants, previous.variants);
        } else {
            assert_eq!(actual, previous);
        }
    }
}

#[test]
fn added_effects_remain_editable_and_parse_with_existing_rules() {
    let styles = schema::spec("animations", "WindowOpenEffect").unwrap();
    assert_eq!(styles.variants, ["fade", "reveal"]);
    assert_eq!(styles.fields.len(), 2);
    for (name, ty) in [
        ("fade", "WindowOpenEffect::Fade"),
        ("reveal", "WindowOpenEffect::Reveal"),
    ] {
        assert!(styles.fields.contains(&FieldSpec {
            name,
            ty,
            kind: "child",
            scalar: false,
            repeated: true,
            optional: true,
        }));
        assert_eq!(
            schema::spec("animations", ty).unwrap(),
            &TypeSpec {
                module: "animations",
                name: ty,
                fields: &[],
                variants: &[],
            }
        );
        realand_config::Config::parse_mem(&format!(
            "animations {{ window-open {{ effect \"{name}\"; }}; }}"
        ))
        .unwrap();
    }
    for value in [true, false] {
        let config = realand_config::Config::parse_mem(&format!(
            "layer-rule {{ match namespace=\"realand-shell\"; background-effect {{ liquid-glass {value}; }}; }}"
        )).unwrap();
        assert_eq!(
            config.layer_rules[0].background_effect.liquid_glass,
            Some(value)
        );
    }
}

#[test]
fn handwritten_decoder_fields_preserve_previous_editor_contract() {
    let mut actual = String::new();
    for name in [
        "Binds",
        "Environment",
        "Bind",
        "RendererPart",
        "WorkspaceSwitchAnim",
        "HorizontalViewMovementAnim",
        "WindowMovementAnim",
        "WindowOpenAnim",
        "WindowCloseAnim",
        "WindowResizeAnim",
        "ConfigNotificationOpenCloseAnim",
        "ExitConfirmationOpenCloseAnim",
        "ScreenshotUiOpenAnim",
        "OverviewOpenCloseAnim",
        "RecentWindowsCloseAnim",
        "DefaultPresetSize",
        "PresetSize",
        "SpringParams",
        "Include",
        "WorkspaceLayoutPart",
    ] {
        let fields: Vec<_> = schema::fields("", name)
            .into_iter()
            .filter(|field| name != "Bind" || field.name != "minimize-window")
            .collect();
        actual.push_str(&format!("{name}: {fields:?}\n"));
    }
    assert_eq!(actual, include_str!("fixtures/custom-fields-v1.txt"));
}

#[test]
fn minimize_action_is_editable_but_runtime_target_registration_is_not_configuration() {
    let config =
        realand_config::Config::parse_mem("binds { Mod+M { minimize-window; }; }").unwrap();
    assert!(matches!(
        config.binds.0[0].action,
        realand_config::Action::MinimizeWindow
    ));
    assert!(schema::fields("", "Bind")
        .iter()
        .any(|field| field.name == "minimize-window"));
    assert!(schema::spec("binds", "Action::SetMinimizeTarget").is_none());
    assert!(schema::spec("binds", "Action::MinimizeWindowById").is_none());
    assert!(
        realand_config::Config::parse_mem("binds { Mod+M { set-minimize-target; }; }").is_err()
    );
}

#[test]
fn logical_groups_disambiguate_rules_and_resolve_fragment_types() {
    let window = schema::fields("window_rule", "Match");
    let layer = schema::fields("layer_rule", "Match");
    assert!(window.iter().any(|field| field.name == "app-id"));
    assert!(!window.iter().any(|field| field.name == "namespace"));
    assert!(layer.iter().any(|field| field.name == "namespace"));
    assert_eq!(schema::spec("input", "Input").unwrap().name, "InputPart");
    let workspace = schema::fields("workspace", "WorkspaceLayoutPart");
    assert!(workspace.iter().any(|field| field.name == "gaps"));
    assert!(!workspace
        .iter()
        .any(|field| matches!(field.name, "empty-workspace-above-first" | "insert-hint")));
}

#[test]
fn custom_grammar_still_uses_real_parser_for_validation() {
    let samples = [
        "renderer \"vulkan\"",
        "renderer-backend \"gl\"",
        "renderer { backend \"vk\"; }",
        "environment { CUSTOM_VARIABLE \"value\"; }",
        "binds { Mod+T repeat=false allow-when-locked=false { spawn \"true\"; }; }",
        "animations { workspace-switch { spring damping-ratio=1.0 stiffness=1000 epsilon=0.0001; cube {}; }; window-open { glide {}; }; window-close { effect \"fade\"; }; }",
        "layout { default-column-width { proportion 0.5; }; preset-column-widths { fixed 800; proportion 0.5; }; }",
    ];
    for sample in samples {
        realand_config::Config::parse_mem(sample)
            .unwrap_or_else(|error| panic!("{sample}: {error:?}"));
    }
    // Dynamic names and parser aliases deliberately aren't presented as a fixed
    // field inventory. The catalog never replaces the real decoder.
    assert!(schema::fields("binds", "Binds").is_empty());
    assert!(schema::fields("lib", "Environment").is_empty());
    assert_eq!(
        schema::spec("renderer", "RendererBackendConfig")
            .unwrap()
            .variants,
        ["gles", "vulkan"]
    );
    assert!(realand_config::Config::parse_mem("renderer \"unknown\"").is_err());
}
