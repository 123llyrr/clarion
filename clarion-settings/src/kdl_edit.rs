//! Lossless edits using the same KDL parser as the compositor. Untouched source stays intact.
use anyhow::{Context, Result};
use knuffel::{
    ast::{Document, Literal, SpannedNode},
    span::Span,
};
use std::ops::Range;

type Node = SpannedNode<Span>;

pub struct Editor {
    source: String,
    document: Document<Span>,
}

impl Editor {
    pub fn new(source: &str) -> Result<Self> {
        let document = knuffel::parse_ast::<Span>("config.kdl", source)
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        Ok(Self {
            source: source.into(),
            document,
        })
    }
    fn node(&self, path: &[&str]) -> Option<&Node> {
        let mut nodes = self.document.nodes.as_slice();
        for (i, name) in path.iter().enumerate() {
            let node = nodes.iter().find(|node| &**node.node_name == *name)?;
            if i + 1 == path.len() {
                return Some(node);
            }
            nodes = node.children.as_deref()?;
        }
        None
    }
    pub fn exists(&self, path: &[&str]) -> bool {
        self.node(path).is_some()
    }
    pub fn body(&self, path: &[&str]) -> Option<Range<usize>> {
        let span = self.node(path)?.children.as_ref()?.span();
        Some(span.0 + 1..span.1 - 1)
    }
    pub fn argument(&self, path: &[&str]) -> Option<String> {
        let value = &self.node(path)?.arguments.first()?.literal;
        match &**value {
            Literal::String(s) => return Some(s.to_string()),
            Literal::Int(number) => {
                return i64::try_from(number)
                    .map(|n| n.to_string())
                    .or_else(|_| u64::try_from(number).map(|n| n.to_string()))
                    .ok();
            }
            _ => {}
        }
        let span = value.span();
        Some(self.source[span.0..span.1].replace('_', ""))
    }
    fn replace(&mut self, range: Range<usize>, replacement: &str) -> Result<()> {
        let mut next = self.source.clone();
        next.replace_range(range, replacement);
        *self = Self::new(&next)?;
        Ok(())
    }
    pub fn ensure(&mut self, path: &[&str]) -> Result<()> {
        for i in 0..path.len() {
            let prefix = &path[..=i];
            if self.body(prefix).is_some() {
                continue;
            }
            if let Some(node) = self.node(prefix) {
                let span = node.span();
                let text = self.source[span.0..span.1].trim_end().trim_end_matches(';');
                let replacement = format!("{text} {{\n}}\n");
                self.replace(span.0..span.1, &replacement)?;
            } else {
                self.upsert(&path[..i], path[i], &format!("{} {{\n}}", path[i]))?;
            }
        }
        Ok(())
    }
    pub fn upsert(&mut self, parent: &[&str], name: &str, replacement: &str) -> Result<()> {
        self.ensure(parent)?;
        let mut path = parent.to_vec();
        path.push(name);
        if let Some(node) = self.node(&path) {
            let span = node.span();
            return self.replace(span.0..span.1, &format!("{}\n", replacement.trim()));
        }
        let index = if parent.is_empty() {
            self.source.len()
        } else {
            self.body(parent).context("KDL block has no children")?.end
        };
        self.replace(index..index, &format!("\n{replacement}\n"))
    }
    pub fn remove(&mut self, path: &[&str]) -> Result<()> {
        if let Some(node) = self.node(path) {
            let span = node.span();
            self.replace(span.0..span.1, "")?;
        }
        Ok(())
    }
    pub fn insert(&mut self, parent: &[&str], snippet: &str) -> Result<()> {
        self.ensure(parent)?;
        let index = self.body(parent).context("KDL block not found")?.end;
        self.replace(index..index, &format!("\n{snippet}\n"))
    }
    pub fn finish(self) -> String {
        self.source
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edits_exact_child_preserving_comments_strings_and_siblings() {
        let original = "/* layout { gaps 99; } */\nlayout { gaps 16; border { width 2; }; }\nspawn-at-startup r#\"{ gaps 1; }\"#\n";
        let mut edit = Editor::new(original).unwrap();
        edit.upsert(&["layout"], "gaps", "gaps 24").unwrap();
        let updated = edit.finish();
        assert!(updated.contains("/* layout { gaps 99; } */"));
        assert!(updated.contains("border { width 2; }"));
        assert!(updated.contains("r#\"{ gaps 1; }\"#"));
        clarion_config::Config::parse_mem(&updated).unwrap();
    }
    #[test]
    fn ignores_slashdash_and_nested_rules() {
        let mut edit =
            Editor::new("/- layout { gaps 88; }\nwindow-rule { border { width 12; }; }\n").unwrap();
        edit.upsert(&["layout", "border"], "width", "width 3")
            .unwrap();
        assert_eq!(
            edit.argument(&["layout", "border", "width"]).as_deref(),
            Some("3")
        );
        assert_eq!(
            edit.argument(&["window-rule", "border", "width"])
                .as_deref(),
            Some("12")
        );
    }
}
