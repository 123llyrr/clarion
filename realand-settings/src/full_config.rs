//! Structured, lossless editing of every KDL node, including repeated rules and outputs.
use std::ops::Range;

use anyhow::{Context, Result};
use egui::{ComboBox, ScrollArea, TextEdit, Ui};

use crate::icons::{self, Icon};
use crate::theme::colors;
use knuffel::{
    ast::{Document, Literal, SpannedNode},
    span::Span,
};

use crate::config_schema as schema;
type Node = SpannedNode<Span>;

pub struct DocumentEditor {
    source: String,
    document: Document<Span>,
}
impl DocumentEditor {
    pub fn new(source: &str) -> Result<Self> {
        let document = knuffel::parse_ast::<Span>("config.kdl", source)
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        Ok(Self {
            source: source.into(),
            document,
        })
    }
    fn node(&self, path: &[usize]) -> Option<&Node> {
        let mut nodes = self.document.nodes.as_slice();
        for (depth, index) in path.iter().enumerate() {
            let node = nodes.get(*index)?;
            if depth + 1 == path.len() {
                return Some(node);
            }
            nodes = node.children.as_deref()?;
        }
        None
    }
    /// Apply from right to left so offsets refer to the original document.
    fn patch(&self, mut edits: Vec<(Range<usize>, String)>) -> Result<String> {
        edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
        let mut result = self.source.clone();
        let mut boundary = result.len();
        for (range, text) in edits {
            anyhow::ensure!(range.end <= boundary, "编辑范围重叠，请重新选择配置项");
            boundary = range.start;
            result.replace_range(range, &text);
        }
        Self::new(&result)?;
        Ok(result)
    }
    fn append(&self, path: &[usize], text: &str) -> Result<String> {
        let (range, text) = if path.is_empty() {
            (self.source.len()..self.source.len(), format!("\n{text}\n"))
        } else {
            let node = self.node(path).context("配置项已改变")?;
            if let Some(children) = &node.children {
                let end = children.span().1 - 1;
                (end..end, format!("\n{text}\n"))
            } else {
                let end = header_end(node);
                (end..end, format!(" {{\n{text}\n}}"))
            }
        };
        self.patch(vec![(range, text)])
    }
    fn remove(&self, path: &[usize]) -> Result<String> {
        let span = self.node(path).context("配置项已改变")?.span();
        self.patch(vec![(span.0..span.1, String::new())])
    }
    fn duplicate(&self, path: &[usize]) -> Result<String> {
        let span = self.node(path).context("配置项已改变")?.span();
        self.append(&path[..path.len() - 1], &self.source[span.0..span.1])
    }
    fn context(&self, path: &[usize]) -> (String, String) {
        let Some(root) = self.document.nodes.get(path[0]) else {
            return Default::default();
        };
        let mut ty = schema::section(&root.node_name)
            .map(|s| s.ty)
            .unwrap_or("")
            .to_owned();
        let mut module = schema::spec("lib", &ty)
            .map(|s| s.module)
            .unwrap_or("lib")
            .to_owned();
        for depth in 1..path.len() {
            let Some(node) = self.node(&path[..=depth]) else {
                break;
            };
            ty = if ty == "Binds" {
                "Bind".into()
            } else if ty == "Environment" {
                "EnvironmentVariable".into()
            } else {
                schema::fields(&module, &ty)
                    .iter()
                    .find(|f| f.kind == "child" && f.name == &**node.node_name)
                    .map(|f| f.ty)
                    .unwrap_or("")
                    .to_owned()
            };
            if let Some(spec) = schema::spec(&module, &ty) {
                module = spec.module.into();
            }
        }
        (module, ty)
    }
}
fn header_end(node: &Node) -> usize {
    node.arguments
        .iter()
        .map(|v| v.literal.span().1)
        .chain(node.properties.values().map(|v| v.literal.span().1))
        .chain(std::iter::once(node.node_name.span().1))
        .max()
        .unwrap()
}
fn quote(text: &str) -> String {
    serde_json::to_string(text).expect("string serialization")
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    String,
    Number,
    Bool,
    Null,
}
impl Kind {
    fn label(self) -> &'static str {
        match self {
            Self::String => "文本",
            Self::Number => "数值",
            Self::Bool => "开关",
            Self::Null => "空值",
        }
    }
}
#[derive(Clone, PartialEq, Eq)]
struct ValueDraft {
    name: String,
    text: String,
    kind: Kind,
    original: Option<(Range<usize>, Range<usize>)>, // literal / entire property
    removed: bool,
}
impl ValueDraft {
    fn from_value(
        source: &str,
        name: String,
        literal: &knuffel::span::Spanned<Literal, Span>,
        start: usize,
    ) -> Self {
        let span = literal.span();
        let (kind, text) = match &**literal {
            Literal::String(text) => (Kind::String, text.to_string()),
            Literal::Bool(value) => (Kind::Bool, value.to_string()),
            Literal::Null => (Kind::Null, "null".into()),
            _ => (Kind::Number, source[span.0..span.1].into()),
        };
        Self {
            name,
            text,
            kind,
            original: Some((span.0..span.1, start..span.1)),
            removed: false,
        }
    }
    fn new(name: &str, value: &str) -> Self {
        let source = format!("value {value}\n");
        let doc = DocumentEditor::new(&source).expect("schema value is valid KDL");
        let mut draft = Self::from_value(
            &source,
            name.into(),
            &doc.document.nodes[0].arguments[0].literal,
            0,
        );
        draft.original = None;
        draft
    }
    fn encoded(&self) -> Result<String> {
        let value = match self.kind {
            Kind::String => quote(&self.text),
            Kind::Bool => {
                if self.text == "true" {
                    "true".into()
                } else {
                    "false".into()
                }
            }
            Kind::Null => "null".into(),
            Kind::Number => {
                let source = format!("value {}\n", self.text);
                let parsed = DocumentEditor::new(&source)?;
                let nodes = &parsed.document.nodes;
                anyhow::ensure!(
                    nodes.len() == 1
                        && nodes[0].arguments.len() == 1
                        && nodes[0].properties.is_empty()
                        && nodes[0].children.is_none(),
                    "请输入单个数值"
                );
                anyhow::ensure!(
                    matches!(
                        &*nodes[0].arguments[0].literal,
                        Literal::Int(_) | Literal::Decimal(_)
                    ),
                    "请输入有效数值"
                );
                self.text.clone()
            }
        };
        Ok(value)
    }
}
#[derive(Clone)]
struct Draft {
    name: String,
    arguments: Vec<ValueDraft>,
    properties: Vec<ValueDraft>,
    new_property: String,
    new_child: String,
    field_search: String,
}
impl Draft {
    fn new(editor: &DocumentEditor, path: &[usize]) -> Option<Self> {
        let node = editor.node(path)?;
        Some(Self {
            name: node.node_name.to_string(),
            arguments: node
                .arguments
                .iter()
                .map(|v| {
                    ValueDraft::from_value(
                        &editor.source,
                        String::new(),
                        &v.literal,
                        v.literal.span().0,
                    )
                })
                .collect(),
            properties: node
                .properties
                .iter()
                .map(|(k, v)| {
                    ValueDraft::from_value(&editor.source, k.to_string(), &v.literal, k.span().0)
                })
                .collect(),
            new_property: String::new(),
            new_child: String::new(),
            field_search: String::new(),
        })
    }
    fn apply(&self, editor: &DocumentEditor, path: &[usize]) -> Result<String> {
        let node = editor.node(path).context("配置项已改变")?;
        let mut edits = Vec::new();
        if self.name != **node.node_name {
            let span = node.node_name.span();
            edits.push((span.0..span.1, quote(&self.name)));
        }
        let mut added = String::new();
        let original = Self::new(editor, path).context("配置项已改变")?;
        for (property, values, baseline) in [
            (false, &self.arguments, &original.arguments),
            (true, &self.properties, &original.properties),
        ] {
            for (index, v) in values.iter().enumerate() {
                // Preserve untouched raw strings and other literal spelling exactly.
                if baseline.get(index) == Some(v) {
                    continue;
                }
                if v.removed {
                    if let Some((_, range)) = &v.original {
                        edits.push((range.clone(), String::new()));
                    }
                    continue;
                }
                let encoded = v.encoded()?;
                if let Some((literal, _)) = &v.original {
                    if editor.source[literal.clone()] != encoded {
                        edits.push((literal.clone(), encoded));
                    }
                } else if property {
                    added.push_str(&format!(" {}={encoded}", quote(&v.name)));
                } else {
                    added.push_str(&format!(" {encoded}"));
                }
            }
        }
        if !added.is_empty() {
            let end = header_end(node);
            edits.push((end..end, added));
        }
        editor.patch(edits)
    }
}

#[derive(Default)]
pub struct FullConfig {
    source: Option<String>,
    editor: Option<DocumentEditor>,
    parse_error: Option<String>,
    section: String,
    search: String,
    selected: Vec<usize>,
    draft: Option<Draft>,
    error: Option<String>,
    undo: Vec<String>,
    redo: Vec<String>,
    remove_confirmation: Option<Vec<usize>>,
}
enum EditAction {
    Section(String),
    Select(Vec<usize>),
    Apply,
    Append(Vec<usize>, String),
    Duplicate(Vec<usize>),
    Remove(Vec<usize>),
    Undo,
    Redo,
    DiscardForm,
}

fn remember(history: &mut Vec<String>, source: &str) {
    const LIMIT: usize = 4 * 1024 * 1024;
    if source.len() > LIMIT {
        history.clear();
        return;
    }
    history.push(source.into());
    while history.len() > 32 || history.iter().map(String::len).sum::<usize>() > LIMIT {
        history.remove(0);
    }
}
impl FullConfig {
    fn load(&mut self, source: &str) {
        self.remove_confirmation = None;
        self.source = Some(source.into());
        match DocumentEditor::new(source) {
            Ok(editor) => {
                if editor.node(&self.selected).is_none() {
                    self.selected.clear();
                }
                self.draft = Draft::new(&editor, &self.selected);
                self.editor = Some(editor);
                self.parse_error = None;
            }
            Err(error) => {
                self.editor = None;
                self.draft = None;
                self.parse_error = Some(error.to_string());
            }
        }
    }
    fn sync(&mut self, source: &str) {
        if self.source.as_deref() != Some(source) {
            self.undo.clear();
            self.redo.clear();
            self.load(source);
        }
    }
    fn accept(&mut self, next: &str) {
        if self.source.as_deref() == Some(next) {
            return;
        }
        if let Some(source) = &self.source {
            remember(&mut self.undo, source);
        }
        self.redo.clear();
        self.load(next);
    }
    #[cfg(test)]
    pub(crate) fn with_pending_argument_for_test(
        source: &str,
        path: Vec<usize>,
        value: &str,
    ) -> Self {
        let mut page = Self::default();
        page.load(source);
        page.perform(EditAction::Select(path)).unwrap();
        page.draft.as_mut().unwrap().arguments[0].text = value.into();
        page
    }
    pub fn has_pending(&self) -> bool {
        let (Some(editor), Some(draft)) = (&self.editor, &self.draft) else {
            return false;
        };
        let Some(original) = Draft::new(editor, &self.selected) else {
            return false;
        };
        draft.name != original.name
            || draft.arguments != original.arguments
            || draft.properties != original.properties
    }
    pub fn apply_pending(&mut self, source: &str) -> Result<Option<String>> {
        anyhow::ensure!(
            !self.has_pending() || self.source.as_deref() == Some(source),
            "配置草稿已在其他页面改变，请先处理当前表单"
        );
        self.sync(source);
        if self.has_pending() {
            let next = self
                .draft
                .as_ref()
                .unwrap()
                .apply(self.editor.as_ref().unwrap(), &self.selected)?;
            self.accept(&next);
            return Ok(Some(next));
        }
        Ok(None)
    }
    fn perform(&mut self, action: EditAction) -> Result<Option<String>> {
        let before = self.source.clone().unwrap_or_default();
        if matches!(action, EditAction::DiscardForm) {
            self.draft = self
                .editor
                .as_ref()
                .and_then(|e| Draft::new(e, &self.selected));
            self.error = None;
            return Ok(None);
        }
        if matches!(action, EditAction::Undo) {
            self.draft = self
                .editor
                .as_ref()
                .and_then(|e| Draft::new(e, &self.selected));
            self.error = None;
            if let Some(previous) = self.undo.pop() {
                remember(&mut self.redo, &before);
                self.load(&previous);
                return Ok(self.source.clone().filter(|s| s != &before));
            }
            return Ok(None);
        }
        // Navigation and structural edits all use the current form, never the stale AST.
        let pending = if self.has_pending() {
            self.draft
                .as_ref()
                .unwrap()
                .apply(self.editor.as_ref().unwrap(), &self.selected)?
        } else {
            before.clone()
        };
        let editor = DocumentEditor::new(&pending)?;
        let mut selection = self.selected.clone();
        let next = match action {
            EditAction::Section(section) => {
                self.section = section;
                selection.clear();
                self.search.clear();
                pending
            }
            EditAction::Select(path) => {
                anyhow::ensure!(editor.node(&path).is_some(), "配置项不存在");
                selection = path;
                pending
            }
            EditAction::Apply => pending,
            EditAction::Append(parent, text) => {
                let count = if parent.is_empty() {
                    editor.document.nodes.len()
                } else {
                    editor
                        .node(&parent)
                        .context("父项不存在")?
                        .children()
                        .count()
                };
                selection = parent.clone();
                selection.push(count);
                editor.append(&parent, &text)?
            }
            EditAction::Duplicate(path) => {
                let parent = &path[..path.len() - 1];
                let count = if parent.is_empty() {
                    editor.document.nodes.len()
                } else {
                    editor
                        .node(parent)
                        .context("父项不存在")?
                        .children()
                        .count()
                };
                selection = parent.to_vec();
                selection.push(count);
                editor.duplicate(&path)?
            }
            EditAction::Remove(path) => {
                selection = path[..path.len() - 1].to_vec();
                editor.remove(&path)?
            }
            EditAction::Undo => unreachable!(),
            EditAction::Redo => {
                anyhow::ensure!(!self.has_pending(), "请先处理当前表单，再重做");
                if let Some(next) = self.redo.pop() {
                    remember(&mut self.undo, &before);
                    self.load(&next);
                }
                self.error = None;
                return Ok(self.source.clone().filter(|s| s != &before));
            }
            EditAction::DiscardForm => unreachable!(),
        };
        self.accept(&next);
        self.selected = selection;
        self.draft = self
            .editor
            .as_ref()
            .and_then(|e| Draft::new(e, &self.selected));
        self.error = None;
        Ok((next != before).then_some(next))
    }
    pub fn select_output(&mut self, source: &str, name: &str) -> Result<Option<String>> {
        self.sync(source);
        let before = source.to_owned();
        // Flush before changing output or creating a configuration block.
        self.apply_pending(source)?;
        let editor = self.editor.as_ref().context("请先修复配置语法")?;
        let existing = editor.document.nodes.iter().position(|n| {
            &**n.node_name == "output"
                && n.arguments
                    .first()
                    .is_some_and(|v| matches!(&*v.literal,Literal::String(s) if &**s==name))
        });
        let action = match existing {
            Some(index) => EditAction::Select(vec![index]),
            None => EditAction::Append(vec![], format!("output {} {{\n}}", quote(name))),
        };
        self.perform(action)?;
        self.section = "output".into();
        self.search.clear();
        Ok(self.source.clone().filter(|s| s != &before))
    }
    pub fn ui(&mut self, ui: &mut Ui, source: &str) -> Option<String> {
        self.sync(source);
        if self.section.is_empty() {
            self.section = "input".into();
        }
        let mut action = None;
        let pending = self.has_pending();
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled_ui(!self.undo.is_empty() || pending, |ui| {
                    icons::button(ui, Icon::Undo, "撤销")
                })
                .inner
                .clicked()
            {
                action = Some(EditAction::Undo);
            }
            if ui
                .add_enabled_ui(!self.redo.is_empty() && !pending, |ui| {
                    icons::button(ui, Icon::Refresh, "重做")
                })
                .inner
                .clicked()
            {
                action = Some(EditAction::Redo);
            }
            if pending {
                ui.colored_label(colors::WARNING, "本项有修改");
                // Keep recovery visible even when the invalid field is in a closed advanced group.
                if ui.small_button("放弃本项修改").clicked() {
                    action = Some(EditAction::DiscardForm);
                }
            }
        });
        ui.add_space(8.);
        ComboBox::from_id_salt("config-section")
            .width(ui.available_width().min(320.))
            .selected_text(
                schema::section(&self.section)
                    .map(|s| s.title)
                    .unwrap_or("其他配置"),
            )
            .show_ui(ui, |ui| {
                for group in ["桌面与外观", "设备与操作", "应用与工作区", "高级设置"]
                {
                    ui.add_space(6.);
                    ui.colored_label(colors::MUTED, group);
                    for section in schema::SECTIONS
                        .iter()
                        .filter(|s| section_group(s.name) == group)
                    {
                        ui.horizontal(|ui| {
                            icons::icon(ui, section_icon(section.name), 16., colors::MUTED);
                            if ui
                                .selectable_label(self.section == section.name, section.title)
                                .on_hover_text(section.name)
                                .clicked()
                            {
                                action = Some(EditAction::Section(section.name.into()));
                            }
                        });
                    }
                }
            });
        ui.add_space(6.);
        ui.horizontal(|ui| {
            icons::icon(ui, Icon::Search, 16., colors::MUTED);
            let width =
                (ui.available_width() - if self.search.is_empty() { 0. } else { 64. }).max(40.);
            ui.add_sized(
                [width, 30.],
                TextEdit::singleline(&mut self.search)
                    .hint_text("搜索所有已配置项目：中文、参数名或内容"),
            );
            if !self.search.is_empty() && ui.small_button("清空").clicked() {
                self.search.clear();
            }
        });
        ui.small("切换项目自动保留修改，点击底部“保存并应用”后生效。");
        if let Some(error) = &self.parse_error {
            ui.colored_label(
                colors::ERROR,
                format!("配置暂时无法读取，请在“配置文件”页面修复：{error}"),
            );
            return None;
        }
        if let Some(error) = &self.error {
            ui.colored_label(colors::ERROR, error);
        }
        let editor = self.editor.as_ref()?;
        if self.selected.is_empty() {
            if let Some(index) = editor.document.nodes.iter().position(|node| {
                schema::section(&node.node_name).is_some_and(|section| section.name == self.section)
            }) {
                self.selected = vec![index];
                self.draft = Draft::new(editor, &self.selected);
            }
        }
        if let Some(section) = schema::section(&self.section) {
            let exists = editor
                .document
                .nodes
                .iter()
                .any(|n| schema::section(&n.node_name).is_some_and(|s| s.name == section.name));
            if !exists {
                ui.add_space(8.);
                ui.colored_label(
                    colors::MUTED,
                    "本文件尚未设置此分类，沿用默认值或导入文件中的设置。",
                );
            }
            if section.repeated || !exists {
                if ui.button(format!("＋ 设置{}", section.title)).clicked() {
                    action = Some(EditAction::Append(vec![], section.sample.into()));
                }
            }
        }
        ui.add_space(10.);
        ui.separator();
        let width = ui.available_width();
        let height = ui.available_height().max(240.);
        if width >= 820. {
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(260., height),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        ui.strong(if self.search.is_empty() {
                            "已配置的项目"
                        } else {
                            "搜索结果"
                        });
                        config_tree(
                            ui,
                            editor,
                            &self.section,
                            &self.selected,
                            &self.search,
                            height - 28.,
                            &mut action,
                        );
                    },
                );
                ui.separator();
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), height),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        inspector(
                            ui,
                            editor,
                            &self.selected,
                            &mut self.draft,
                            &mut self.remove_confirmation,
                            height,
                            &mut action,
                            &mut self.error,
                        );
                    },
                );
            });
        } else {
            egui::CollapsingHeader::new(if self.search.is_empty() {
                "浏览已配置项目"
            } else {
                "搜索结果"
            })
            .id_salt("compact-config-tree")
            .open((!self.search.is_empty()).then_some(true))
            .show(ui, |ui| {
                config_tree(
                    ui,
                    editor,
                    &self.section,
                    &self.selected,
                    &self.search,
                    200.,
                    &mut action,
                );
            });
            ui.add_space(6.);
            let height = ui.available_height().max(240.);
            inspector(
                ui,
                editor,
                &self.selected,
                &mut self.draft,
                &mut self.remove_confirmation,
                height,
                &mut action,
                &mut self.error,
            );
        }
        if let Some(action) = action {
            match self.perform(action) {
                Ok(next) => return next,
                Err(error) => self.error = Some(format!("当前修改已保留，请修正后继续：{error}")),
            }
        }
        None
    }
}

fn config_tree(
    ui: &mut Ui,
    editor: &DocumentEditor,
    section: &str,
    selected: &[usize],
    search: &str,
    height: f32,
    action: &mut Option<EditAction>,
) {
    ScrollArea::vertical()
        .id_salt("config-tree")
        .max_height(height.max(80.))
        .min_scrolled_height(height.max(80.))
        .show(ui, |ui| {
            let mut selection = None;
            let mut found = false;
            for (index, node) in editor.document.nodes.iter().enumerate() {
                if search.is_empty()
                    && !schema::section(&node.node_name).is_some_and(|s| s.name == section)
                {
                    continue;
                }
                if !search.is_empty() && !matches_search(node, search, &editor.source) {
                    continue;
                }
                found = true;
                tree(
                    ui,
                    node,
                    &mut vec![index],
                    selected,
                    search,
                    &editor.source,
                    &mut selection,
                );
            }
            if !found {
                ui.colored_label(
                    colors::MUTED,
                    if search.is_empty() {
                        "此分类还没有写入配置"
                    } else {
                        "没有找到匹配的配置"
                    },
                );
            }
            if let Some(path) = selection {
                *action = Some(EditAction::Select(path));
            }
        });
}

#[allow(clippy::too_many_arguments)]
fn inspector(
    ui: &mut Ui,
    editor: &DocumentEditor,
    selected: &[usize],
    draft: &mut Option<Draft>,
    remove_confirmation: &mut Option<Vec<usize>>,
    height: f32,
    action: &mut Option<EditAction>,
    error: &mut Option<String>,
) {
    ScrollArea::vertical()
        .id_salt("config-inspector")
        .max_height(height)
        .min_scrolled_height(height)
        .show(ui, |ui| {
            let Some(draft) = draft else {
                ui.strong("选择要调整的项目");
                ui.colored_label(
                    colors::MUTED,
                    "从已配置项目进入编辑，或添加需要覆盖默认值的设置。导入文件保持独立。",
                );
                return;
            };
            let (module, ty) = editor.context(selected);
            let fields = schema::fields(&module, &ty);
            ui.push_id(selected, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for depth in 1..selected.len() {
                        if let Some(node) = editor.node(&selected[..depth]) {
                            if ui.small_button(friendly_label(&node.node_name)).clicked() {
                                *action = Some(EditAction::Select(selected[..depth].to_vec()));
                            }
                            icons::icon(ui, Icon::ChevronRight, 12., colors::MUTED);
                        }
                    }
                });
                let title = if selected.len() == 1 {
                    schema::section(&draft.name)
                        .map(|s| s.title)
                        .unwrap_or_else(|| friendly_label(&draft.name))
                } else {
                    friendly_label(&draft.name)
                };
                ui.heading(title);
                ui.colored_label(colors::MUTED, "已设置 · 当前配置文件")
                    .on_hover_text(format!("原配置名：{}", draft.name));
                ui.add_space(10.);
                if ty == "Bind" || ty == "EnvironmentVariable" {
                    ui.label(if ty == "Bind" {
                        "快捷键"
                    } else {
                        "变量名称"
                    });
                    let width = ui.available_width().min(500.);
                    ui.add_sized([width, 30.], TextEdit::singleline(&mut draft.name));
                }
                let argument_fields: Vec<_> =
                    fields.iter().filter(|f| f.kind == "argument").collect();
                for (i, value) in draft.arguments.iter_mut().enumerate() {
                    let field = argument_fields.get(i).or_else(|| argument_fields.last());
                    let value_ty = field.map(|f| f.ty).unwrap_or(&ty);
                    let label = field
                        .map(|f| friendly_label(f.name))
                        .filter(|name| *name != "0" && *name != "1")
                        .map(str::to_owned)
                        .unwrap_or_else(|| {
                            if i == 0 {
                                "设置值".into()
                            } else {
                                format!("设置值 {}", i + 1)
                            }
                        });
                    value_ui(ui, ("arg", i), value, &label, &module, value_ty);
                }
                for (i, value) in draft.properties.iter_mut().enumerate() {
                    let value_ty = fields
                        .iter()
                        .find(|f| f.kind == "property" && f.name == value.name)
                        .map(|f| f.ty)
                        .unwrap_or("");
                    let label = friendly_label(&value.name).to_owned();
                    value_ui(ui, ("prop", i), value, &label, &module, value_ty);
                }
                if draft.arguments.is_empty() && draft.properties.iter().all(|v| v.removed) {
                    ui.colored_label(colors::MUTED, "此设置已写入当前文件。");
                }
                if fields.iter().any(|f| {
                    f.kind == "property"
                        && !draft
                            .properties
                            .iter()
                            .any(|v| v.name == f.name && !v.removed)
                }) {
                    ComboBox::from_id_salt("new-property")
                        .selected_text("＋ 添加设置字段")
                        .show_ui(ui, |ui| {
                            for field in fields.iter().filter(|f| f.kind == "property") {
                                if draft
                                    .properties
                                    .iter()
                                    .any(|v| v.name == field.name && !v.removed)
                                {
                                    continue;
                                }
                                if ui
                                    .button(friendly_label(field.name))
                                    .on_hover_text(format!("{} · 尚未在本文件设置", field.name))
                                    .clicked()
                                {
                                    draft.properties.push(ValueDraft::new(
                                        field.name,
                                        &schema::value_default(field.name, &module, field.ty),
                                    ));
                                }
                            }
                        });
                }
                let changed = Draft::new(editor, selected).is_some_and(|original| {
                    draft.name != original.name
                        || draft.arguments != original.arguments
                        || draft.properties != original.properties
                });
                if changed {
                    ui.add_space(8.);
                    ui.horizontal_wrapped(|ui| {
                        if icons::button(ui, Icon::Check, "保留本项修改").clicked() {
                            *action = Some(EditAction::Apply);
                        }
                        if ui.button("放弃本项修改").clicked() {
                            *action = Some(EditAction::DiscardForm);
                        }
                    });
                }
                if fields.iter().any(|f| f.kind == "child") || ty == "Binds" || ty == "Environment"
                {
                    ui.add_space(12.);
                    ui.separator();
                    ui.strong("更多设置");
                    ui.small("未添加的选项继续沿用默认值或导入文件中的设置。");
                    ui.add_sized(
                        [ui.available_width(), 30.],
                        TextEdit::singleline(&mut draft.field_search).hint_text("查找可设置的选项"),
                    );
                    let node = editor.node(selected);
                    let mut shown = 0;
                    for field in fields.iter().filter(|f| f.kind == "child") {
                        let title = friendly_label(field.name);
                        if !draft.field_search.is_empty()
                            && !format!("{title} {}", field.name)
                                .to_lowercase()
                                .contains(&draft.field_search.to_lowercase())
                        {
                            continue;
                        }
                        let existing = node.and_then(|node| {
                            node.children().position(|n| &**n.node_name == field.name)
                        });
                        if let Some(index) = existing.filter(|_| !field.repeated) {
                            // Existing nested values are discoverable alongside unset suggestions.
                            ui.horizontal_wrapped(|ui| {
                                if ui.button(title).on_hover_text(field.name).clicked() {
                                    let mut path = selected.to_vec();
                                    path.push(index);
                                    *action = Some(EditAction::Select(path));
                                }
                                ui.colored_label(colors::MUTED, "已设置");
                            });
                        } else {
                            ui.horizontal_wrapped(|ui| {
                                if ui
                                    .button(format!("＋ {title}"))
                                    .on_hover_text(field.name)
                                    .clicked()
                                {
                                    *action = Some(EditAction::Append(
                                        selected.to_vec(),
                                        schema::sample(&module, *field),
                                    ));
                                }
                                ui.colored_label(
                                    colors::MUTED,
                                    if field.repeated {
                                        "可添加多项"
                                    } else {
                                        "尚未设置"
                                    },
                                );
                            });
                        }
                        shown += 1;
                    }
                    if shown == 0 && !draft.field_search.is_empty() {
                        ui.colored_label(colors::MUTED, "没有匹配的选项");
                    }
                    if ty == "Binds" && ui.button("＋ 添加快捷键").clicked() {
                        *action = Some(EditAction::Append(
                            selected.to_vec(),
                            "Mod+F12 { spawn \"kitty\"; }".into(),
                        ));
                    }
                    if ty == "Environment" && ui.button("＋ 添加环境变量").clicked() {
                        *action = Some(EditAction::Append(
                            selected.to_vec(),
                            "MY_VARIABLE \"value\"".into(),
                        ));
                    }
                }
                ui.add_space(12.);
                egui::CollapsingHeader::new("高级编辑").show(ui, |ui| {
                    ui.label("原始配置名");
                    ui.add_sized(
                        [ui.available_width(), 30.],
                        TextEdit::singleline(&mut draft.name),
                    );
                    if ui.button("＋ 添加位置参数").clicked() {
                        draft.arguments.push(ValueDraft::new("", "\"\""));
                    }
                    ui.label("自定义字段名称");
                    ui.add_sized(
                        [ui.available_width(), 30.],
                        TextEdit::singleline(&mut draft.new_property),
                    );
                    if ui
                        .add_enabled(
                            !draft.new_property.trim().is_empty(),
                            egui::Button::new("添加自定义字段"),
                        )
                        .clicked()
                    {
                        if draft
                            .properties
                            .iter()
                            .any(|v| v.name == draft.new_property.trim() && !v.removed)
                        {
                            *error = Some("字段名称已存在".into());
                        } else {
                            draft
                                .properties
                                .push(ValueDraft::new(draft.new_property.trim(), "\"\""));
                            draft.new_property.clear();
                        }
                    }
                    ui.label("自定义子项名称");
                    ui.add_sized(
                        [ui.available_width(), 30.],
                        TextEdit::singleline(&mut draft.new_child),
                    );
                    if ui
                        .add_enabled(
                            !draft.new_child.trim().is_empty(),
                            egui::Button::new("添加自定义子项"),
                        )
                        .clicked()
                    {
                        *action = Some(EditAction::Append(
                            selected.to_vec(),
                            quote(draft.new_child.trim()),
                        ));
                    }
                });
                egui::CollapsingHeader::new("配置项操作").show(ui, |ui| {
                    let can_duplicate = selected.len() > 1
                        || schema::section(&draft.name).is_some_and(|s| s.repeated);
                    if ui
                        .add_enabled(can_duplicate, egui::Button::new("复制整个配置项"))
                        .clicked()
                    {
                        *action = Some(EditAction::Duplicate(selected.to_vec()));
                    }
                    if remove_confirmation.as_deref() == Some(selected) {
                        ui.colored_label(
                            colors::WARNING,
                            "移除此配置及其所有子项？导入文件不会改变。",
                        );
                        ui.horizontal_wrapped(|ui| {
                            if ui.button("确认移除").clicked() {
                                *action = Some(EditAction::Remove(selected.to_vec()));
                            }
                            if ui.button("取消").clicked() {
                                *remove_confirmation = None;
                            }
                        });
                    } else if ui
                        .button(egui::RichText::new("移除此配置项…").color(colors::ERROR))
                        .clicked()
                    {
                        *remove_confirmation = Some(selected.to_vec());
                    }
                });
            });
        });
}

fn value_ui(
    ui: &mut Ui,
    id: (&str, usize),
    value: &mut ValueDraft,
    label: &str,
    module: &str,
    ty: &str,
) {
    ui.push_id(id, |ui| {
        let parameter = if value.name.is_empty() {
            format!("位置参数 {}", id.1 + 1)
        } else {
            value.name.clone()
        };
        ui.strong(label)
            .on_hover_text(format!("原参数：{parameter}\n值类型：{ty}"));
        if value.removed {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(colors::MUTED, "此值将从当前文件移除");
                if ui.small_button("恢复").clicked() {
                    value.removed = false;
                }
            });
            return;
        }
        let choices: Vec<&str> = if ty == "RendererPart" {
            vec!["vulkan", "gles"]
        } else {
            schema::spec(module, ty)
                .map(|s| s.variants.to_vec())
                .unwrap_or_default()
        };
        ui.horizontal(|ui| {
            let width = (ui.available_width() - 60.).max(40.);
            match value.kind {
                Kind::Bool => {
                    let mut checked = value.text == "true";
                    if ui.checkbox(&mut checked, "是").changed() {
                        value.text = checked.to_string();
                    }
                }
                Kind::Null => {
                    ui.colored_label(colors::MUTED, "显式空值（null）");
                }
                Kind::String if !choices.is_empty() => {
                    ComboBox::from_id_salt("choices")
                        .width(width.min(320.))
                        .selected_text(friendly_label(&value.text))
                        .show_ui(ui, |ui| {
                            for choice in &choices {
                                ui.selectable_value(
                                    &mut value.text,
                                    (*choice).into(),
                                    friendly_label(choice),
                                )
                                .on_hover_text(*choice);
                            }
                        });
                }
                _ => {
                    ui.add_sized(
                        [width.min(500.), 30.],
                        TextEdit::singleline(&mut value.text),
                    );
                }
            }
            ui.menu_button("更多", |ui| {
                if !choices.is_empty() && value.kind == Kind::String {
                    ui.label("自定义值");
                    ui.add(TextEdit::singleline(&mut value.text).desired_width(180.));
                    ui.separator();
                }
                ui.menu_button("值类型", |ui| {
                    for kind in [Kind::String, Kind::Number, Kind::Bool, Kind::Null] {
                        ui.selectable_value(&mut value.kind, kind, kind.label());
                    }
                });
                if ui
                    .button("移除此值")
                    .on_hover_text("仅移除此参数或属性，其他配置保持不变")
                    .clicked()
                {
                    value.removed = true;
                    ui.close_menu();
                }
            });
        });
        ui.add_space(8.);
    });
}

fn matches_search(node: &Node, search: &str, source: &str) -> bool {
    let span = node.span();
    source[span.0..span.1]
        .to_lowercase()
        .contains(&search.to_lowercase())
        || friendly_label(&node.node_name)
            .to_lowercase()
            .contains(&search.to_lowercase())
        || node.properties.keys().any(|name| {
            friendly_label(name)
                .to_lowercase()
                .contains(&search.to_lowercase())
        })
        || schema::section(&node.node_name).is_some_and(|s| s.title.contains(search))
        || node
            .children()
            .any(|child| matches_search(child, search, source))
}
fn tree(
    ui: &mut Ui,
    node: &Node,
    path: &mut Vec<usize>,
    selected: &[usize],
    search: &str,
    source: &str,
    selection: &mut Option<Vec<usize>>,
) {
    if !search.is_empty() && !matches_search(node, search, source) {
        return;
    }
    let title = (path.len() == 1)
        .then(|| schema::section(&node.node_name))
        .flatten()
        .map(|s| s.title)
        .unwrap_or_else(|| friendly_label(&node.node_name));
    let summary = node_summary(node, source);
    let title = if summary.is_empty() {
        title.into()
    } else {
        format!("{title} · {summary}")
    };
    let end = header_end(node);
    let header = &source[node.node_name.span().0..end];
    let root_icon = (path.len() == 1).then(|| section_icon(&node.node_name));
    ui.push_id(path.clone(), |ui| {
        if node.children().next().is_none() {
            ui.horizontal(|ui| {
                if tree_header(ui, &title, header, root_icon, path == selected) {
                    *selection = Some(path.clone());
                }
            });
            return;
        }
        let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(),
            ui.make_persistent_id("tree-node"),
            path.len() == 1 || selected.starts_with(path),
        );
        if !search.is_empty() || (selected.starts_with(path) && selected.len() > path.len()) {
            state.set_open(true);
        }
        state
            .show_header(ui, |ui| {
                if tree_header(ui, &title, header, root_icon, path == selected) {
                    *selection = Some(path.clone());
                }
            })
            .body(|ui| {
                for (index, child) in node.children().enumerate() {
                    path.push(index);
                    tree(ui, child, path, selected, search, source, selection);
                    path.pop();
                }
            });
    });
}

fn tree_header(ui: &mut Ui, title: &str, raw: &str, icon: Option<Icon>, selected: bool) -> bool {
    if let Some(icon) = icon {
        icons::icon(ui, icon, 16., colors::MUTED);
    }
    ui.add(
        egui::Button::new(title)
            .selected(selected)
            .frame(false)
            .wrap(),
    )
    .on_hover_text(raw)
    .clicked()
}

fn node_summary(node: &Node, source: &str) -> String {
    let values = node
        .arguments
        .iter()
        .map(|value| &value.literal)
        .chain(node.properties.values().map(|value| &value.literal))
        .take(2)
        .map(|literal| match &**literal {
            Literal::String(text) => text.to_string(),
            _ => {
                let span = literal.span();
                source[span.0..span.1].to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" · ");
    let summary = if values.is_empty() {
        node.children()
            .find(|n| &**n.node_name == "match")
            .map(|n| node_summary(n, source))
            .unwrap_or_default()
    } else {
        values
    };
    if summary.chars().count() > 32 {
        format!("{}…", summary.chars().take(32).collect::<String>())
    } else {
        summary
    }
}

fn section_group(name: &str) -> &'static str {
    match name {
        "layout" | "animations" | "blur" | "overview" | "cursor" | "prefer-no-csd" => "桌面与外观",
        "input" | "output" | "gestures" | "binds" | "clipboard" | "screenshot-path"
        | "recent-windows" | "hotkey-overlay" => "设备与操作",
        "window-rule" | "layer-rule" | "workspace" | "spawn-at-startup" | "spawn-sh-at-startup" => {
            "应用与工作区"
        }
        _ => "高级设置",
    }
}

fn section_icon(name: &str) -> Icon {
    match name {
        "input" | "binds" | "gestures" | "hotkey-overlay" => Icon::Input,
        "output" => Icon::Display,
        "layout" | "workspace" => Icon::Layout,
        "animations" | "blur" | "overview" | "cursor" | "prefer-no-csd" => Icon::Appearance,
        "window-rule" | "layer-rule" | "recent-windows" => Icon::Windows,
        "include" | "debug" | "environment" | "renderer" | "renderer-backend" => Icon::Code,
        _ => Icon::Settings,
    }
}

fn friendly_label(name: &str) -> &str {
    match name {
        "name" => "名称",
        "layout" => "布局",
        "title" => "标题",
        "value" => "设置值",
        "style" => "概览风格",
        "stage" => "空间舞台",
        "strip" => "经典纵向概览",
        "width" => "宽度",
        "height" => "高度",
        "size" => "尺寸",
        "x" => "水平位置",
        "y" => "垂直位置",
        "top" => "顶部",
        "bottom" => "底部",
        "left" => "左侧",
        "right" => "右侧",
        "padding" => "内边距",
        "struts" => "屏幕边缘留白",
        "corner-radius" => "圆角半径",
        "geometry-corner-radius" => "窗口内容圆角",
        "app-id" => "应用标识",
        "namespace" => "图层名称",
        "is-focused" => "窗口获得焦点",
        "is-active" => "窗口已激活",
        "is-floating" => "窗口处于浮动状态",
        "is-urgent" => "窗口请求注意",
        "opacity" => "不透明度",
        "color" => "颜色",
        "background-color" => "背景颜色",
        "backdrop-color" => "概览背景颜色",
        "active-color" => "激活颜色",
        "inactive-color" => "未激活颜色",
        "urgent-color" => "提醒颜色",
        "active-gradient" => "激活渐变",
        "inactive-gradient" => "未激活渐变",
        "gradient" => "渐变",
        "angle" => "角度",
        "from" => "起始值",
        "to" => "结束值",
        "relative-to" => "参考位置",
        "duration-ms" => "动画时长（毫秒）",
        "delay-ms" => "延迟（毫秒）",
        "curve" => "动画曲线",
        "spring" => "弹簧运动",
        "damping-ratio" => "阻尼比",
        "stiffness" => "弹簧刚度",
        "epsilon" => "收尾精度",
        "effect" => "关闭效果",
        "tilt-deg" => "倾斜角度",
        "perspective" => "透视强度",
        "depth" => "深度",
        "intensity" => "强度",
        "noise" => "噪点",
        "softness" => "柔和程度",
        "spread" => "扩散范围",
        "offset" => "偏移",
        "zoom" => "缩放",
        "window-resize" => "窗口调整大小",
        "window-movement" => "窗口移动",
        "overview-open-close" => "概览打开与关闭",
        "custom-shader" => "自定义着色器",
        "workspace-shadow" => "工作区阴影",
        "model" => "键盘型号",
        "variant" => "布局变体",
        "options" => "布局选项",
        "track-layout" => "布局记忆范围",
        "mod-key" => "主修饰键",
        "numlock" => "数字键盘锁定",
        "tap" => "轻触点击",
        "natural-scroll" => "自然滚动",
        "accel-speed" => "指针加速",
        "accel-profile" => "加速方式",
        "left-handed" => "左手模式",
        "dwt" => "输入时关闭触摸板",
        "dwtp" => "指点杆使用时关闭触摸板",
        "middle-emulation" => "模拟中键",
        "scroll-factor" => "滚动倍率",
        "click-method" => "点击方式",
        "scroll-method" => "滚动方式",
        "tap-button-map" => "轻触按键映射",
        "backend" => "渲染后端",
        "vulkan" => "Vulkan",
        "gles" => "OpenGL ES",
        "hot-corners" => "屏幕热角",
        "open-delay-ms" => "打开延迟（毫秒）",
        "trigger-width" => "触发宽度",
        "trigger-height" => "触发高度",
        "max-scale" => "最大缩放",
        "max-speed" => "最大速度",
        "default-column-width" => "默认列宽",
        "default-window-height" => "默认窗口高度",
        "default-floating-position" => "默认浮动位置",
        "default-column-display" => "默认列显示方式",
        "preset-column-widths" => "列宽预设",
        "preset-window-heights" => "高度预设",
        "preset-window-widths" => "宽度预设",
        "focus-follows-mouse" => "焦点跟随鼠标",
        "warp-mouse-to-focus" => "指针跟随焦点",
        "center-focused-column" => "聚焦列居中",
        "empty-workspace-above-first" => "顶部保留空工作区",
        "workspace-auto-back-and-forth" => "工作区自动返回",
        "dim-inactive" => "弱化未激活窗口",
        "dim-factor" => "未激活窗口亮度",
        "auto-stack-new-windows" => "自动堆叠新窗口",
        "tab-indicator" => "标签指示器",
        "hide-when-single-tab" => "单标签时隐藏",
        "clip-to-geometry" => "裁剪到窗口边界",
        "xcursor-theme" => "光标主题",
        "xcursor-size" => "光标大小",
        "map-to-output" => "映射到显示器",
        "calibration-matrix" => "校准矩阵",
        "position-epsilon" => "位置收尾精度",
        "velocity-epsilon" => "速度收尾精度",
        "min-width" => "最小宽度",
        "max-width" => "最大宽度",
        "min-height" => "最小高度",
        "max-height" => "最大高度",
        "open-floating" => "以浮动窗口打开",
        "open-focused" => "打开时获得焦点",
        "open-fullscreen" => "以全屏打开",
        "open-maximized" => "以最大化打开",
        "open-on-output" => "在指定显示器打开",
        "open-on-workspace" => "在指定工作区打开",
        "block-out-from" => "隐藏于截图或录屏",
        "repeat" => "允许重复触发",
        "cooldown-ms" => "触发间隔（毫秒）",
        "allow-when-locked" => "锁屏时允许",
        "allow-inhibiting" => "允许应用屏蔽",
        "hotkey-overlay-title" => "快捷键提示名称",
        "linear" => "线性",
        "ease-out-cubic" => "三次缓出",
        "ease-out-quad" => "二次缓出",
        "ease-out-expo" => "指数缓出",
        "cubic-bezier" => "贝塞尔曲线",
        "explosion" => "散开",
        "implode" => "收拢",
        "fade" => "淡出",
        "fixed" => "固定尺寸",
        "proportion" => "比例尺寸",
        "always" => "始终",
        "never" => "从不",
        "on-overflow" => "空间不足时",
        "adaptive" => "自适应",
        "flat" => "固定速度",
        "horizontal" => "水平",
        "vertical" => "垂直",
        _ => schema::section(name)
            .map(|s| s.title)
            .unwrap_or_else(|| schema::label(name)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    fn valid(source: &str) {
        if let Err(error) =
            realand_config::Config::parse(Path::new("/tmp/realand-settings-test.kdl"), source)
                .config
        {
            panic!("{source}\n{error:?}");
        }
    }
    fn editing(source: &str, path: &[usize]) -> FullConfig {
        let mut page = FullConfig::default();
        page.sync(source);
        page.selected = path.to_vec();
        page.draft = Draft::new(page.editor.as_ref().unwrap(), path);
        page
    }
    #[test]
    fn changing_one_property_keeps_other_literal_spelling_exactly() {
        let source = "window-rule { match app-id=r#\"example\"# /* keep */ is-focused=false; }\n";
        let editor = DocumentEditor::new(source).unwrap();
        let mut draft = Draft::new(&editor, &[0, 0]).unwrap();
        draft
            .properties
            .iter_mut()
            .find(|v| v.name == "is-focused")
            .unwrap()
            .text = "true".into();
        assert_eq!(
            draft.apply(&editor, &[0, 0]).unwrap(),
            source.replace("is-focused=false", "is-focused=true")
        );
    }
    #[test]
    fn navigation_keeps_fields_and_undo_redo_restores_source() {
        let source = "layout { gaps /* preserved */ 8; }\ninput {}\n";
        let mut page = editing(source, &[0, 0]);
        page.draft.as_mut().unwrap().arguments[0].text = "12".into();
        let next = page
            .perform(EditAction::Section("input".into()))
            .unwrap()
            .unwrap();
        assert_eq!(next, source.replace("*/ 8", "*/ 12"));
        assert_eq!(page.section, "input");
        assert!(!page.has_pending());
        assert_eq!(page.perform(EditAction::Undo).unwrap().unwrap(), source);
        assert_eq!(page.perform(EditAction::Redo).unwrap().unwrap(), next);
    }
    #[test]
    fn invalid_form_blocks_navigation_and_structural_edits_without_losing_input() {
        for action in [
            EditAction::Select(vec![1]),
            EditAction::Section("input".into()),
            EditAction::Append(vec![0], "border {}".into()),
            EditAction::Duplicate(vec![0, 0]),
            EditAction::Remove(vec![0, 0]),
        ] {
            let source = "layout { gaps 8; }\ninput {}\n";
            let mut page = editing(source, &[0, 0]);
            page.draft.as_mut().unwrap().arguments[0].text = "-".into();
            assert!(page.perform(action).is_err());
            assert_eq!(page.selected, vec![0, 0]);
            assert_eq!(page.source.as_deref(), Some(source));
            assert_eq!(page.draft.as_ref().unwrap().arguments[0].text, "-");
            assert!(page.has_pending());
            page.perform(EditAction::DiscardForm).unwrap();
            assert!(!page.has_pending());
        }
    }
    #[test]
    fn undo_discards_invalid_form_without_requiring_a_fix() {
        let source = "layout { gaps 8; }\ninput {}\n";
        let mut page = editing(source, &[0, 0]);
        page.draft.as_mut().unwrap().arguments[0].text = "-".into();
        assert!(page.has_pending());
        assert!(page.perform(EditAction::Undo).unwrap().is_none());
        assert!(!page.has_pending());
        assert_eq!(page.source.as_deref(), Some(source));
    }
    #[test]
    fn copying_and_adding_use_current_form_instead_of_stale_document() {
        let source = "output \"DP-1\" { scale 1.0; }\n";
        let mut page = editing(source, &[0, 0]);
        page.draft.as_mut().unwrap().arguments[0].text = "2.0".into();
        let next = page
            .perform(EditAction::Duplicate(vec![0]))
            .unwrap()
            .unwrap();
        assert_eq!(next.matches("scale 2.0").count(), 2);
        assert!(!next.contains("scale 1.0"));
        assert_eq!(page.perform(EditAction::Undo).unwrap().unwrap(), source);
        let mut page = editing(source, &[0]);
        page.draft.as_mut().unwrap().arguments[0].text = "HDMI-A-1".into();
        let next = page
            .perform(EditAction::Append(vec![0], "position x=0 y=0".into()))
            .unwrap()
            .unwrap();
        assert!(next.contains("HDMI-A-1"));
        assert_eq!(page.selected, vec![0, 1]);
        valid(&next);
    }
    #[test]
    fn output_shortcut_and_sibling_navigation_flush_pending_form() {
        let source = "layout { gaps 8; }\noutput \"DP-1\" {}\n";
        let mut page = editing(source, &[0, 0]);
        page.draft.as_mut().unwrap().arguments[0].text = "16".into();
        let next = page.select_output(source, "DP-1").unwrap().unwrap();
        assert!(next.contains("gaps 16"));
        assert_eq!(page.selected, vec![1]);
        let mut page = editing(source, &[0, 0]);
        page.draft.as_mut().unwrap().arguments[0].text = "24".into();
        assert!(page
            .perform(EditAction::Select(vec![1]))
            .unwrap()
            .unwrap()
            .contains("gaps 24"));
    }
    #[test]
    fn external_file_change_clears_history_and_unedited_raw_strings_stay_lossless() {
        let source = "output r#\"DP-1\"# {}\n";
        let mut page = editing(source, &[0]);
        assert!(!page.has_pending());
        assert!(page
            .perform(EditAction::Section("input".into()))
            .unwrap()
            .is_none());
        page.perform(EditAction::Append(vec![], "input {}".into()))
            .unwrap();
        assert!(!page.undo.is_empty());
        page.sync("layout {}\n");
        assert!(page.undo.is_empty());
        assert!(page.redo.is_empty());
    }
    #[test]
    fn history_limits_memory_and_editing_after_undo_discards_redo_branch() {
        let mut history = Vec::new();
        for _ in 0..40 {
            remember(&mut history, &"a".repeat(200_000));
        }
        assert!(history.len() <= 32);
        assert!(history.iter().map(String::len).sum::<usize>() <= 4 * 1024 * 1024);
        let mut page = editing("layout {}\n", &[0]);
        page.perform(EditAction::Append(vec![0], "gaps 8".into()))
            .unwrap();
        page.perform(EditAction::Undo).unwrap();
        assert!(!page.redo.is_empty());
        page.perform(EditAction::Append(vec![0], "gaps 12".into()))
            .unwrap();
        assert!(page.redo.is_empty());
    }
    #[test]
    fn suggested_root_children_parse() {
        let mut failures = Vec::new();
        for section in schema::SECTIONS {
            let Some(spec) = schema::spec("lib", section.ty) else {
                continue;
            };
            let editor = DocumentEditor::new(&format!("{}\n", section.sample)).unwrap();
            for field in schema::fields(spec.module, section.ty)
                .into_iter()
                .filter(|f| f.kind == "child")
            {
                if editor
                    .node(&[0])
                    .unwrap()
                    .children()
                    .any(|n| &**n.node_name == field.name)
                {
                    continue;
                }
                let sample = schema::sample(spec.module, field);
                let source = editor.append(&[0], &sample).unwrap();
                if let Err(error) = realand_config::Config::parse(
                    Path::new("/tmp/realand-settings-test.kdl"),
                    &source,
                )
                .config
                {
                    failures.push(format!(
                        "{} / {}: {} => {error:?}",
                        section.name, field.name, sample
                    ));
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
    #[test]
    fn overview_style_inspector_choices_and_template_use_typed_schema() {
        let field = schema::fields("misc", "OverviewPart")
            .into_iter()
            .find(|field| field.name == "style")
            .unwrap();
        assert!(field.scalar);
        let choices = schema::spec("misc", field.ty).unwrap().variants;
        assert_eq!(choices, &["stage", "strip"]);
        for value in choices {
            let source = format!("overview {{ style \"{value}\"; }}");
            realand_config::Config::parse_mem(&source).unwrap();
            assert_ne!(friendly_label(value), *value);
        }
        let template = format!("overview {{ {}; }}", schema::sample("misc", field));
        assert_eq!(
            realand_config::Config::parse_mem(&template)
                .unwrap()
                .overview
                .style,
            realand_config::OverviewStyle::Stage,
        );
    }
    #[test]
    fn spring_epsilon_inspector_default_parses() {
        let source = format!(
            "animations {{\n    window-open {{\n        spring damping-ratio=1.0 stiffness=1000 epsilon={}\n    }}\n}}\n",
            schema::value_default("epsilon", "animations", "f64")
        );
        realand_config::Config::parse(Path::new("/tmp/realand-settings-test.kdl"), &source)
            .config
            .unwrap();
    }
    #[test]
    fn nested_input_layout_animation_and_rule_suggestions_parse() {
        let fixtures=["input { keyboard {}; touchpad {}; mouse {}; tablet {}; }\n",
            "layout { border {}; shadow {}; tab-indicator {}; struts {}; }\n",
            "animations { workspace-switch {}; window-open {}; window-close {}; window-resize {}; window-wobble {}; }\n",
            "window-rule { border {}; shadow {}; default-floating-position x=1 y=1; }\n"];
        let mut failures = Vec::new();
        for source in fixtures {
            let editor = DocumentEditor::new(source).unwrap();
            for (index, _) in editor.document.nodes[0].children().enumerate() {
                let path = [0, index];
                let (module, ty) = editor.context(&path);
                for field in schema::fields(&module, &ty)
                    .into_iter()
                    .filter(|f| f.kind == "child")
                {
                    let sample = schema::sample(&module, field);
                    let next = editor.append(&path, &sample).unwrap();
                    if let Err(error) = realand_config::Config::parse(
                        Path::new("/tmp/realand-settings-test.kdl"),
                        &next,
                    )
                    .config
                    {
                        failures.push(format!("{ty} / {}: {sample} => {error:?}", field.name));
                    }
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
    #[test]
    fn catalog_covers_compositor_dispatch_including_aliases() {
        for name in realand_config::schema::ROOT_NODES {
            assert!(schema::section(name).is_some(), "missing section {name}");
        }
        assert_eq!(
            realand_config::schema::ROOT_NODES.len(),
            schema::SECTIONS.len() + 1 // renderer-backend alias
        );
    }
    #[test]
    fn chinese_search_finds_descendants_and_save_flushes_form() {
        let source = "input { touchpad { tap; }; }\nlayout { gaps 8; }\n";
        let editor = DocumentEditor::new(source).unwrap();
        assert!(matches_search(&editor.document.nodes[0], "触摸板", source));
        let mut page = FullConfig::default();
        page.sync(source);
        page.selected = vec![1, 0];
        page.draft = Draft::new(page.editor.as_ref().unwrap(), &page.selected);
        page.draft.as_mut().unwrap().arguments[0].text = "12".into();
        let next = page.apply_pending(source).unwrap().unwrap();
        assert!(next.contains("gaps 12"));
        valid(&next);
    }
    #[test]
    fn every_section_template_is_accepted_by_compositor() {
        for section in schema::SECTIONS {
            valid(&format!("{}\n", section.sample));
        }
    }
    #[test]
    fn repeated_outputs_preserve_comments_and_siblings() {
        let source="// Keep header\noutput \"eDP-1\" { scale 1.25; }\noutput \"DP-1\" {\n scale /* keep */ 1.5 // comment\n}\ninclude \"extra.kdl\" optional=true\n";
        let editor = DocumentEditor::new(source).unwrap();
        let mut draft = Draft::new(&editor, &[1, 0]).unwrap();
        draft.arguments[0].text = "2.0".into();
        let changed = draft.apply(&editor, &[1, 0]).unwrap();
        assert_eq!(changed, source.replace("*/ 1.5", "*/ 2.0"));
        valid(&changed);
    }
    #[test]
    fn repeated_rules_properties_and_literal_types_roundtrip() {
        let source="window-rule { match app-id=\"first\"; }\nwindow-rule { match app-id=\"second\" is-focused=false; }\n";
        let editor = DocumentEditor::new(source).unwrap();
        let mut draft = Draft::new(&editor, &[1, 0]).unwrap();
        draft
            .properties
            .iter_mut()
            .find(|v| v.name == "app-id")
            .unwrap()
            .text = "quote\"\\test".into();
        draft
            .properties
            .iter_mut()
            .find(|v| v.name == "is-focused")
            .unwrap()
            .text = "true".into();
        let changed = draft.apply(&editor, &[1, 0]).unwrap();
        assert!(changed.starts_with("window-rule { match app-id=\"first\"; }"));
        valid(&changed);
        for (input, kind) in [
            ("null", Kind::Null),
            ("true", Kind::Bool),
            ("-1.25", Kind::Number),
            ("\"hello\\nworld\"", Kind::String),
        ] {
            let value = ValueDraft::new("", input);
            assert_eq!(value.kind, kind);
            assert_eq!(value.encoded().unwrap(), input);
        }
    }
    #[test]
    fn append_remove_duplicate_and_invalid_values() {
        let editor = DocumentEditor::new("output \"DP-1\" // retained\n").unwrap();
        let appended = editor.append(&[0], "scale 1.5").unwrap();
        valid(&appended);
        assert!(appended.contains("// retained"));
        let editor = DocumentEditor::new(&appended).unwrap();
        let mut draft = Draft::new(&editor, &[0, 0]).unwrap();
        draft.arguments[0].text = "1; output \"evil\"".into();
        assert!(draft.apply(&editor, &[0, 0]).is_err());
        let duplicate = editor.duplicate(&[0]).unwrap();
        assert_eq!(
            DocumentEditor::new(&duplicate)
                .unwrap()
                .document
                .nodes
                .len(),
            2
        );
        let removed = editor.remove(&[0, 0]).unwrap();
        assert!(!removed.contains("scale"));
        valid(&removed);
    }
    #[test]
    fn output_navigation_edits_existing_or_creates_exact_name() {
        let mut ui = FullConfig::default();
        let source = "output \"eDP-1\" {}\noutput \"DP-2\" {}\n";
        assert!(ui.select_output(source, "DP-2").unwrap().is_none());
        assert_eq!(ui.selected, vec![1]);
        let next = ui.select_output(source, "HDMI-A-1").unwrap().unwrap();
        valid(&next);
        assert_eq!(ui.selected, vec![2]);
    }
    #[test]
    fn context_resolves_rules_bind_actions_and_nested_layouts() {
        let source="window-rule { match app-id=\"app\"; }\nlayer-rule { match namespace=\"panel\"; }\nbinds { Mod+T { spawn \"kitty\"; }; }\noutput \"DP-1\" { layout { border {}; }; }\n";
        let editor = DocumentEditor::new(source).unwrap();
        assert_eq!(
            editor.context(&[0, 0]),
            ("window_rule".into(), "Match".into())
        );
        assert_eq!(
            editor.context(&[1, 0]),
            ("layer_rule".into(), "Match".into())
        );
        assert_eq!(editor.context(&[2, 0, 0]).1, "Action::Spawn");
        assert_eq!(editor.context(&[3, 0, 0]).1, "BorderRule");
        assert!(schema::fields("binds", "Bind")
            .iter()
            .any(|f| f.name == "toggle-canvas"));
        assert!(schema::fields("input", "InputPart")
            .iter()
            .any(|f| f.name == "touchpad"));
    }
    #[test]
    fn every_section_and_existing_node_renders_headlessly() {
        let source = include_str!("../../resources/default-config.kdl");
        let editor = DocumentEditor::new(source).unwrap();
        let mut paths = vec![];
        fn collect(nodes: &[Node], parent: &[usize], paths: &mut Vec<Vec<usize>>) {
            for (i, n) in nodes.iter().enumerate() {
                let mut path = parent.to_vec();
                path.push(i);
                paths.push(path.clone());
                if let Some(children) = &n.children {
                    collect(children, &path, paths);
                }
            }
        }
        collect(&editor.document.nodes, &[], &mut paths);
        let context = egui::Context::default();
        let mut page = FullConfig::default();
        page.sync(source);
        for path in paths {
            page.selected = path;
            page.draft = Draft::new(page.editor.as_ref().unwrap(), &page.selected);
            let _ = context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1100.0, 800.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        assert!(page.ui(ui, source).is_none());
                    });
                },
            );
        }
    }

    #[test]
    fn narrow_forms_and_search_stay_within_the_available_width() {
        let source = "output \"DP-1\" { scale 1.5; position x=0 y=1080; }\nanimations { window-close { duration-ms 240; effect \"fade\"; }; }\n";
        let context = egui::Context::default();
        for width in [320., 500., 800., 1100.] {
            for (path, search) in [(vec![0, 1], ""), (vec![1, 0], "动画时长")] {
                let mut page = editing(source, &path);
                page.search = search.into();
                let _ = context.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 800.),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            let right = ui.max_rect().right();
                            assert!(page.ui(ui, source).is_none());
                            assert!(
                                ui.min_rect().right() <= right + 1.,
                                "width={width}, search={search}, content={:?}, right={right}",
                                ui.min_rect()
                            );
                        });
                    },
                );
                assert_eq!(page.source.as_deref(), Some(source));
            }
        }
        let editor = DocumentEditor::new(source).unwrap();
        assert!(matches_search(
            &editor.document.nodes[1],
            "动画时长",
            source
        ));
        assert!(matches_search(
            &editor.document.nodes[0],
            "水平位置",
            source
        ));
    }
}
