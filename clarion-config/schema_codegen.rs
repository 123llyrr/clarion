//! Build-time metadata extraction owned by the configuration crate.
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::{fs, io};

use quote::ToTokens;
use syn::visit::Visit;
use syn::{Fields, GenericArgument, Item, PathArguments, Type};

fn kebab(s: &str) -> String {
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if c == '_' {
            out.push('-');
        } else {
            if i > 0 && c.is_uppercase() {
                out.push('-');
            }
            out.extend(c.to_lowercase());
        }
    }
    out
}
fn ty_name(ty: &Type) -> String {
    if let Type::Path(p) = ty {
        let s = p.path.segments.last().unwrap();
        if ["Option", "Vec", "Box"].iter().any(|n| s.ident == *n) {
            if let PathArguments::AngleBracketed(a) = &s.arguments {
                if let Some(GenericArgument::Type(t)) = a.args.first() {
                    return ty_name(t);
                }
            }
        }
        return s.ident.to_string();
    }
    "String".into()
}
fn field_specs(input: &Fields) -> Vec<String> {
    let mut fields = Vec::new();
    for (i, f) in input.iter().enumerate() {
        let attrs = f
            .attrs
            .iter()
            .filter(|a| a.path().is_ident("knuffel"))
            .map(|a| a.meta.to_token_stream().to_string())
            .collect::<Vec<_>>()
            .join(" ");
        if attrs.is_empty() {
            continue;
        }
        let kind = if attrs.contains("children") || attrs.contains("child") {
            "child"
        } else if attrs.contains("property") {
            "property"
        } else if attrs.contains("argument") {
            "argument"
        } else {
            continue;
        };
        let mut name = f
            .ident
            .as_ref()
            .map(|i| kebab(&i.to_string()))
            .unwrap_or_else(|| i.to_string());
        if attrs.contains("name =") {
            if let Some(n) = attrs.split('"').nth(1) {
                name = n.into();
            }
        }
        let optional =
            f.ty.to_token_stream().to_string().starts_with("Option") || attrs.contains("default");
        let scalar = kind != "child" || (attrs.contains("unwrap") && attrs.contains("argument"));
        let repeated = attrs.contains("children") && !attrs.contains("child , unwrap");
        fields.push(format!("FieldSpec {{ name: {name:?}, ty: {:?}, kind: {kind:?}, scalar: {scalar}, repeated: {repeated}, optional: {optional} }}", ty_name(&f.ty)));
    }

    fields
}

pub struct Generated {
    pub code: String,
    pub sources: Vec<PathBuf>,
}

struct Module {
    file: PathBuf,
    group: String,
    items: Vec<Item>,
}

fn test_only(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("cfg")
            && attr
                .parse_args::<syn::Path>()
                .is_ok_and(|path| path.is_ident("test"))
    })
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

// Follow module declarations rather than scanning a directory. This also avoids
// including unreferenced backup files, fixture code and cfg(test) modules.
fn collect_file(
    file: &Path,
    group: &str,
    crate_root: bool,
    modules: &mut Vec<Module>,
    sources: &mut HashSet<PathBuf>,
) -> io::Result<()> {
    let canonical = file.canonicalize()?;
    if !sources.insert(canonical) {
        return Err(invalid(format!(
            "configuration module is included more than once: {}",
            file.display()
        )));
    }
    let syntax = syn::parse_file(&fs::read_to_string(file)?)
        .map_err(|error| invalid(format!("{}: {error}", file.display())))?;
    let parent = file.parent().unwrap();
    let directory = if crate_root || file.file_name().is_some_and(|name| name == "mod.rs") {
        parent.to_owned()
    } else {
        parent.join(file.file_stem().unwrap())
    };
    collect_items(
        file,
        &directory,
        parent,
        group,
        syntax.items,
        modules,
        sources,
    )
}

fn collect_items(
    file: &Path,
    directory: &Path,
    path_attribute_base: &Path,
    group: &str,
    items: Vec<Item>,
    modules: &mut Vec<Module>,
    sources: &mut HashSet<PathBuf>,
) -> io::Result<()> {
    let mut declarations = Vec::new();
    for item in items {
        let Item::Mod(module) = item else {
            declarations.push(item);
            continue;
        };
        if test_only(&module.attrs) {
            continue;
        }
        let name = module.ident.to_string();
        // Keep the editor-facing group stable when an implementation moves into
        // a private child module, e.g. input.rs -> input/keyboard.rs.
        let child_group = if group == "lib" { &name } else { group };
        let explicit_path = module.attrs.iter().find_map(|attr| {
            if !attr.path().is_ident("path") {
                return None;
            }
            let syn::Meta::NameValue(value) = &attr.meta else {
                return None;
            };
            let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(path),
                ..
            }) = &value.value
            else {
                return None;
            };
            Some(path.value())
        });
        if let Some((_, children)) = module.content {
            let child_directory = explicit_path
                .map(|path| path_attribute_base.join(path))
                .unwrap_or_else(|| directory.join(&name));
            collect_items(
                file,
                &child_directory,
                &child_directory,
                child_group,
                children,
                modules,
                sources,
            )?;
        } else {
            let child = if let Some(path) = explicit_path {
                path_attribute_base.join(path)
            } else {
                let flat = directory.join(format!("{name}.rs"));
                let nested = directory.join(&name).join("mod.rs");
                match (flat.exists(), nested.exists()) {
                    (true, false) => flat,
                    (false, true) => nested,
                    _ => {
                        return Err(invalid(format!(
                            "expected exactly one module source for {name} in {}",
                            directory.display()
                        )));
                    }
                }
            };
            collect_file(&child, child_group, false, modules, sources)?;
        }
    }
    modules.push(Module {
        file: file.to_owned(),
        group: group.into(),
        items: declarations,
    });
    Ok(())
}

#[derive(Default)]
struct RootNodes {
    names: Vec<String>,
    dispatches: usize,
}

impl<'ast> Visit<'ast> for RootNodes {
    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        let Type::Path(ty) = &*node.self_ty else {
            return;
        };
        if !ty.path.is_ident("ConfigPart")
            || !node
                .trait_
                .as_ref()
                .is_some_and(|(_, path, _)| path.segments.last().unwrap().ident == "DecodeChildren")
        {
            return;
        }
        syn::visit::visit_item_impl(self, node);
    }

    fn visit_expr_match(&mut self, node: &'ast syn::ExprMatch) {
        if matches!(&*node.expr, syn::Expr::Path(path) if path.path.is_ident("name")) {
            self.dispatches += 1;
            for arm in &node.arms {
                self.collect_pattern(&arm.pat);
            }
        }
        syn::visit::visit_expr_match(self, node);
    }
}

impl RootNodes {
    fn collect_pattern(&mut self, pattern: &syn::Pat) {
        match pattern {
            syn::Pat::Lit(syn::ExprLit {
                lit: syn::Lit::Str(name),
                ..
            }) => self.names.push(name.value()),
            syn::Pat::Or(pattern) => {
                for case in &pattern.cases {
                    self.collect_pattern(case);
                }
            }
            _ => {}
        }
    }
}

pub fn generate(crate_root: &Path) -> io::Result<Generated> {
    let mut modules = Vec::new();
    let mut sources = HashSet::new();
    collect_file(crate_root, "lib", true, &mut modules, &mut sources)?;
    modules.sort_by(|a, b| a.file.cmp(&b.file).then(a.group.cmp(&b.group)));
    let mut output = String::from(
        "/// Structural descriptions generated from this crate's decoding declarations.\n\
         /// Use `fields()` to include handwritten-decoder metadata.\n\
         pub static TYPES: &[TypeSpec] = &[\n",
    );
    let mut roots = RootNodes::default();
    for source in modules {
        let module = &source.group;
        for item in source.items {
            if let Item::Impl(node) = &item {
                roots.visit_item_impl(node);
            }
            append_item(&mut output, module, item);
        }
    }
    output.push_str("];\n");
    if roots.dispatches != 1 || roots.names.is_empty() {
        return Err(invalid(
            "expected one ConfigPart::decode_children root-node dispatch; update the schema generator for parser changes",
        ));
    }
    output.push_str(&format!(
        "/// Accepted top-level KDL node names, including compatibility aliases.\n\
         pub static ROOT_NODES: &[&str] = &{:?};\n",
        roots.names
    ));
    let mut sources: Vec<_> = sources.into_iter().collect();
    sources.sort();
    Ok(Generated {
        code: output,
        sources,
    })
}

fn append_item(output: &mut String, module: &str, item: Item) {
    match item {
        Item::Struct(s) => {
            let fields = field_specs(&s.fields);
            if !fields.is_empty() {
                output.push_str(&format!("TypeSpec {{ module: {module:?}, name: {:?}, fields: &[{}], variants: &[] }},\n", s.ident.to_string(), fields.join(",")));
            }
        }
        Item::Enum(e) => {
            let mut variants = Vec::new();
            let mut children = Vec::new();
            for v in &e.variants {
                if v.attrs.iter().any(|a| {
                    (a.path().is_ident("knuffel")
                        && a.meta.to_token_stream().to_string().contains("skip"))
                        || (a.path().is_ident("doc")
                            && a.parse_args::<syn::Path>()
                                .is_ok_and(|path| path.is_ident("hidden")))
                }) {
                    continue;
                }
                let name = kebab(&v.ident.to_string());
                let attrs = v
                    .attrs
                    .iter()
                    .map(|a| a.meta.to_token_stream().to_string())
                    .collect::<Vec<_>>()
                    .join(" ");
                let name = if attrs.contains("rename") {
                    attrs.split('"').nth(1).unwrap_or(&name).to_owned()
                } else {
                    name
                };
                if matches!(v.fields, Fields::Unit) {
                    variants.push(format!("{name:?}"));
                }
                let ty = format!("{}::{}", e.ident, v.ident);
                let fields = field_specs(&v.fields);
                output.push_str(&format!("TypeSpec {{ module: {module:?}, name: {ty:?}, fields: &[{}], variants: &[] }},\n",fields.join(",")));
                children.push(format!("FieldSpec {{ name: {name:?}, ty: {ty:?}, kind: \"child\", scalar: false, repeated: true, optional: true }}"));
            }
            let scalar = e.attrs.iter().any(|a| {
                a.meta
                    .to_token_stream()
                    .to_string()
                    .contains("DecodeScalar")
            });
            output.push_str(&format!(
                "TypeSpec {{ module: {module:?}, name: {:?}, fields: &[{}], variants: &[{}] }},\n",
                e.ident.to_string(),
                if scalar {
                    String::new()
                } else {
                    children.join(",")
                },
                variants.join(",")
            ));
        }
        _ => (),
    }
}
