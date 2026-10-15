#[path = "../schema_codegen.rs"]
mod generator;

use std::fs;

const DISPATCH: &str = r#"
struct ConfigPart;
impl DecodeChildren for ConfigPart {
    fn decode_children() {
        match name {
            "input" => (),
            "renderer" | "renderer-backend" => (),
            name => (),
        }
    }
}
"#;

const FIELDS: &str = r#"
#[derive(knuffel::Decode)]
pub struct KeyboardPart {
    #[knuffel(child, unwrap(argument), default)]
    pub repeat_rate: Option<u32>,
    #[knuffel(children(name = "layout"))]
    pub layouts: Vec<Layout>,
}
#[derive(knuffel::DecodeScalar)]
pub enum Choice {
    First,
    #[knuffel(rename = "custom-choice")]
    Second,
    #[knuffel(skip)]
    Hidden,
}
"#;

#[test]
fn source_layout_changes_preserve_public_logical_groups() {
    let layouts: &[(&str, &[(&str, &str)])] = &[
        ("mod input;", &[("input.rs", FIELDS)]),
        ("mod input;", &[("input/mod.rs", FIELDS)]),
        (
            "mod input;",
            &[("input.rs", "mod keyboard;"), ("input/keyboard.rs", FIELDS)],
        ),
        (
            "mod input;",
            &[
                ("input/mod.rs", "mod keyboard;"),
                ("input/keyboard/mod.rs", FIELDS),
            ],
        ),
        (
            "mod input { mod keyboard; }",
            &[("input/keyboard.rs", FIELDS)],
        ),
        (
            "#[path = \"implementation/keyboard.rs\"] mod input;",
            &[("implementation/keyboard.rs", FIELDS)],
        ),
        (
            "mod input;",
            &[
                (
                    "input.rs",
                    "#[path = \"implementation/keyboard.rs\"] mod keyboard;",
                ),
                ("implementation/keyboard.rs", FIELDS),
            ],
        ),
    ];
    let mut expected = None;
    for (root, files) in layouts {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("lib.rs"),
            format!("{root}\n{DISPATCH}"),
        )
        .unwrap();
        for (path, contents) in *files {
            let file = directory.path().join(path);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, contents).unwrap();
        }
        let generated = generator::generate(&directory.path().join("lib.rs")).unwrap();
        assert_eq!(generated.sources.len(), files.len() + 1);
        let expected = expected.get_or_insert_with(|| generated.code.clone());
        assert_eq!(
            generated.code, *expected,
            "module layout: {root}, {files:?}"
        );
    }
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("lib.rs"),
        format!("mod input {{ mod keyboard {{ {FIELDS} }} }}\n{DISPATCH}"),
    )
    .unwrap();
    let generated = generator::generate(&directory.path().join("lib.rs")).unwrap();
    assert_eq!(generated.code, expected.unwrap());
}

#[test]
fn test_and_orphan_sources_are_not_schema_inputs() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("lib.rs"),
        format!(
            "#[cfg(test)] mod unavailable;\n#[cfg(test)] mod tests {{ enum Bogus {{ A }} }}\n{DISPATCH}"
        ),
    )
    .unwrap();
    fs::write(directory.path().join("orphan.rs"), "not valid Rust").unwrap();
    let generated = generator::generate(&directory.path().join("lib.rs")).unwrap();
    assert_eq!(generated.sources.len(), 1);
    assert!(!generated.code.contains("Bogus"));
    assert!(generated
        .code
        .contains("&[\"input\", \"renderer\", \"renderer-backend\"]"));
}

#[test]
fn missing_modules_and_changed_dispatch_fail_loudly() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("lib.rs");
    fs::write(&root, format!("mod missing; {DISPATCH}")).unwrap();
    assert!(generator::generate(&root).is_err());
    fs::write(&root, "struct ConfigPart;").unwrap();
    assert!(generator::generate(&root)
        .err()
        .unwrap()
        .to_string()
        .contains("root-node dispatch"));
}
