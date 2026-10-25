// SPDX-License-Identifier: GPL-3.0-or-later
//! The palette's slash-command whitelist.
//!
//! Mirrors Clavis `Common/functions/SpotlightCommands.js`: one table drives the
//! command list, slash resolution and the override (layout/order) commands.
//! Titles are the ones the Clarion palette renders; English words stay in
//! `keywords` so `/calc`, `calculator` and `计算` all find the same entry.
use crate::session::{Mode, Tool};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandKind {
    Mode,
    Tool,
    Action,
    Override,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Command {
    pub id: &'static str,
    pub slash: &'static str,
    pub aliases: &'static [&'static str],
    pub title: &'static str,
    pub keywords: &'static [&'static str],
    pub kind: CommandKind,
    /// Mode/tool value for `Mode`/`Tool` entries.
    pub value: Option<&'static str>,
    /// Override scope (`apps` or `clipboard`).
    pub scope: Option<&'static str>,
    pub key: Option<&'static str>,
    pub override_value: Option<&'static str>,
}

pub const ENTRIES: &[Command] = &[
    Command {
        id: "mode.search",
        slash: "default",
        aliases: &["everything"],
        title: "默认搜索",
        keywords: &["default", "search", "everything", "搜索"],
        kind: CommandKind::Mode,
        value: Some("search"),
        scope: None,
        key: None,
        override_value: None,
    },
    Command {
        id: "mode.apps",
        slash: "apps",
        aliases: &["app", "applications"],
        title: "应用",
        keywords: &["apps", "applications", "应用"],
        kind: CommandKind::Mode,
        value: Some("apps"),
        scope: None,
        key: None,
        override_value: None,
    },
    Command {
        id: "mode.wallpapers",
        slash: "wallpaper",
        aliases: &["wallpapers"],
        title: "壁纸",
        keywords: &["wallpaper", "wallpapers", "background", "壁纸"],
        kind: CommandKind::Mode,
        value: Some("wallpapers"),
        scope: None,
        key: None,
        override_value: None,
    },
    Command {
        id: "mode.clipboard",
        slash: "clipboard",
        aliases: &["clip", "history"],
        title: "剪贴板",
        keywords: &["clipboard", "clip", "history", "剪贴板"],
        kind: CommandKind::Mode,
        value: Some("clipboard"),
        scope: None,
        key: None,
        override_value: None,
    },
    Command {
        id: "mode.files",
        slash: "files",
        aliases: &["file"],
        title: "文件",
        keywords: &["files", "file", "documents", "文件"],
        kind: CommandKind::Mode,
        value: Some("files"),
        scope: None,
        key: None,
        override_value: None,
    },
    Command {
        id: "mode.commands",
        slash: "commands",
        aliases: &["command"],
        title: "命令",
        keywords: &["commands", "command", "slash", "命令"],
        kind: CommandKind::Mode,
        value: Some("commands"),
        scope: None,
        key: None,
        override_value: None,
    },
    Command {
        id: "tool.web",
        slash: "search",
        aliases: &["web", "www"],
        title: "网页搜索",
        keywords: &["web", "search", "internet", "engine", "网页"],
        kind: CommandKind::Tool,
        value: Some("web"),
        scope: None,
        key: None,
        override_value: None,
    },
    Command {
        id: "tool.calculator",
        slash: "calc",
        aliases: &["calculator", "math"],
        title: "计算器",
        keywords: &["calculator", "calculate", "math", "计算"],
        kind: CommandKind::Tool,
        value: Some("calculator"),
        scope: None,
        key: None,
        override_value: None,
    },
    Command {
        id: "tool.currency",
        slash: "fx",
        aliases: &["currency", "exchange"],
        title: "汇率",
        keywords: &["currency", "exchange", "rate", "汇率", "货币"],
        kind: CommandKind::Tool,
        value: Some("currency"),
        scope: None,
        key: None,
        override_value: None,
    },
    Command {
        id: "tool.time",
        slash: "time",
        aliases: &["tz", "timezone"],
        title: "时区",
        keywords: &["time", "timezone", "clock", "时区", "时间"],
        kind: CommandKind::Tool,
        value: Some("time"),
        scope: None,
        key: None,
        override_value: None,
    },
    Command {
        id: "theme.light",
        slash: "light",
        aliases: &[],
        title: "浅色主题",
        keywords: &["light", "theme", "浅色", "主题"],
        kind: CommandKind::Action,
        value: None,
        scope: None,
        key: None,
        override_value: None,
    },
    Command {
        id: "theme.dark",
        slash: "dark",
        aliases: &[],
        title: "深色主题",
        keywords: &["dark", "theme", "深色", "主题"],
        kind: CommandKind::Action,
        value: None,
        scope: None,
        key: None,
        override_value: None,
    },
    Command {
        id: "settings.open",
        slash: "settings",
        aliases: &["preferences"],
        title: "打开设置",
        keywords: &["settings", "preferences", "设置", "偏好"],
        kind: CommandKind::Action,
        value: None,
        scope: None,
        key: None,
        override_value: None,
    },
    Command {
        id: "tool.settings",
        slash: "find-settings",
        aliases: &["settings-search"],
        title: "搜索设置",
        keywords: &["settings", "find", "search", "设置"],
        kind: CommandKind::Tool,
        value: Some("settings"),
        scope: None,
        key: None,
        override_value: None,
    },
    Command {
        id: "tool.actions",
        slash: "actions",
        aliases: &["commands-actions"],
        title: "搜索动作",
        keywords: &["actions", "ipc", "commands", "动作"],
        kind: CommandKind::Tool,
        value: Some("actions"),
        scope: None,
        key: None,
        override_value: None,
    },
    Command {
        id: "location.open",
        slash: "map",
        aliases: &["location"],
        title: "位置",
        keywords: &["map", "location", "where", "位置", "地图"],
        kind: CommandKind::Action,
        value: None,
        scope: None,
        key: None,
        override_value: None,
    },
    Command {
        id: "apps.list",
        slash: "list",
        aliases: &[],
        title: "列表",
        keywords: &["list", "layout", "列表", "布局"],
        kind: CommandKind::Override,
        value: None,
        scope: Some("apps"),
        key: Some("appsLayout"),
        override_value: Some("list"),
    },
    Command {
        id: "apps.grid",
        slash: "grid",
        aliases: &[],
        title: "网格",
        keywords: &["grid", "layout", "网格", "布局"],
        kind: CommandKind::Override,
        value: None,
        scope: Some("apps"),
        key: Some("appsLayout"),
        override_value: Some("grid"),
    },
    Command {
        id: "apps.smart",
        slash: "smart",
        aliases: &[],
        title: "智能排序",
        keywords: &["smart", "order", "智能", "排序"],
        kind: CommandKind::Override,
        value: None,
        scope: Some("apps"),
        key: Some("appsOrder"),
        override_value: Some("smart"),
    },
    Command {
        id: "apps.most-used",
        slash: "most-used",
        aliases: &["mostused"],
        title: "最常用",
        keywords: &["most", "used", "frequent", "常用"],
        kind: CommandKind::Override,
        value: None,
        scope: Some("apps"),
        key: Some("appsOrder"),
        override_value: Some("most-used"),
    },
    Command {
        id: "apps.recent",
        slash: "recent",
        aliases: &["recently-used"],
        title: "最近使用",
        keywords: &["recent", "recently", "最近"],
        kind: CommandKind::Override,
        value: None,
        scope: Some("apps"),
        key: Some("appsOrder"),
        override_value: Some("recently-used"),
    },
    Command {
        id: "apps.name",
        slash: "name",
        aliases: &["sort-name"],
        title: "按名称",
        keywords: &["name", "alphabetical", "名称", "排序"],
        kind: CommandKind::Override,
        value: None,
        scope: Some("apps"),
        key: Some("appsOrder"),
        override_value: Some("name"),
    },
    Command {
        id: "clipboard.compact",
        slash: "compact",
        aliases: &["default-layout"],
        title: "紧凑",
        keywords: &["compact", "list", "紧凑", "列表"],
        kind: CommandKind::Override,
        value: None,
        scope: Some("clipboard"),
        key: Some("clipboardLayout"),
        override_value: Some("default"),
    },
    Command {
        id: "clipboard.detail",
        slash: "detail",
        aliases: &["details"],
        title: "详情",
        keywords: &["detail", "details", "preview", "详情", "预览"],
        kind: CommandKind::Override,
        value: None,
        scope: Some("clipboard"),
        key: Some("clipboardLayout"),
        override_value: Some("details"),
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolveError {
    Unknown,
    /// The entry exists but belongs to another mode (overrides).
    Scope,
    Arguments,
}

pub fn exact(name: &str) -> Option<&'static Command> {
    let name = name.to_lowercase();
    ENTRIES.iter().find(|entry| {
        entry.slash == name || entry.aliases.iter().any(|alias| *alias == name)
    })
}

pub fn by_id(id: &str) -> Option<&'static Command> {
    ENTRIES.iter().find(|entry| entry.id == id)
}

/// Overrides are only offered inside their own mode and never inside a tool.
pub fn available(command: &Command, mode: Mode, tool: Option<Tool>) -> bool {
    match command.scope {
        None => true,
        Some(scope) => tool.is_none() && mode.value() == scope,
    }
}

/// Entries shown in the standalone command list (`/commands` and `>`).
pub fn listable() -> impl Iterator<Item = &'static Command> {
    ENTRIES
        .iter()
        .filter(|entry| entry.kind != CommandKind::Override)
}

/// Free-text filter used by the command list and slash completion.
pub fn search(query: &str, mode: Mode, tool: Option<Tool>) -> Vec<&'static Command> {
    let words: Vec<String> = query
        .trim()
        .to_lowercase()
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    let mut found: Vec<&'static Command> = listable()
        .filter(|entry| available(entry, mode, tool))
        .filter(|entry| {
            words.iter().all(|word| {
                entry.slash.contains(word.as_str())
                    || entry.title.to_lowercase().contains(word.as_str())
                    || entry
                        .aliases
                        .iter()
                        .any(|alias| alias.contains(word.as_str()))
                    || entry
                        .keywords
                        .iter()
                        .any(|keyword| keyword.contains(word.as_str()))
            })
        })
        .collect();
    found.sort_by(|left, right| {
        command_rank(left, &words)
            .cmp(&command_rank(right, &words))
            .then_with(|| left.title.cmp(right.title))
            .then_with(|| left.id.cmp(right.id))
    });
    found
}

fn command_rank(command: &Command, words: &[String]) -> u8 {
    let first = words.first().map(String::as_str).unwrap_or_default();
    if first.is_empty() {
        return 3;
    }
    if command.slash == first {
        0
    } else if command.slash.starts_with(first) {
        1
    } else if command.title.starts_with(first) {
        2
    } else {
        3
    }
}

pub fn resolve(
    name: &str,
    arguments: &str,
    mode: Mode,
    tool: Option<Tool>,
) -> Result<&'static Command, ResolveError> {
    let Some(command) = exact(name) else {
        return Err(ResolveError::Unknown);
    };
    if !available(command, mode, tool) {
        return Err(ResolveError::Scope);
    }
    // Only tools take an argument; every other command is self-contained.
    if !arguments.trim().is_empty() && command.kind != CommandKind::Tool {
        return Err(ResolveError::Arguments);
    }
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_slash_name_and_alias_is_unique() {
        let mut names: Vec<&str> = Vec::new();
        for entry in ENTRIES {
            names.push(entry.slash);
            names.extend(entry.aliases.iter().copied());
        }
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "duplicate slash name or alias");
    }

    #[test]
    fn the_table_matches_the_reference_count() {
        assert_eq!(ENTRIES.len(), 24);
        assert_eq!(ENTRIES.iter().filter(|entry| entry.kind == CommandKind::Mode).count(), 6);
        assert_eq!(ENTRIES.iter().filter(|entry| entry.kind == CommandKind::Tool).count(), 6);
        assert_eq!(ENTRIES.iter().filter(|entry| entry.kind == CommandKind::Action).count(), 4);
        assert_eq!(ENTRIES.iter().filter(|entry| entry.kind == CommandKind::Override).count(), 8);
    }

    #[test]
    fn exact_resolution_accepts_aliases_case_insensitively() {
        assert!(exact("nope").is_none());
        assert_eq!(exact("CLIPBOARD").unwrap().id, "mode.clipboard");
        assert_eq!(exact("tz").unwrap().id, "tool.time");
        assert_eq!(exact("wallpapers").unwrap().id, "mode.wallpapers");
    }

    #[test]
    fn overrides_only_resolve_inside_their_mode() {
        assert_eq!(
            resolve("grid", "", Mode::Search, None),
            Err(ResolveError::Scope)
        );
        assert_eq!(resolve("grid", "", Mode::Apps, None).unwrap().id, "apps.grid");
        assert_eq!(
            resolve("grid", "", Mode::Apps, Some(Tool::Web)),
            Err(ResolveError::Scope)
        );
        assert_eq!(resolve("compact", "", Mode::Clipboard, None).unwrap().id, "clipboard.compact");
        assert_eq!(resolve("grid", "", Mode::Clipboard, None), Err(ResolveError::Scope));
    }

    #[test]
    fn only_tools_accept_arguments() {
        assert_eq!(
            resolve("calc", "1+1", Mode::Search, None).unwrap().id,
            "tool.calculator"
        );
        assert_eq!(
            resolve("apps", "now", Mode::Search, None),
            Err(ResolveError::Arguments)
        );
        assert_eq!(
            resolve("nope", "", Mode::Search, None),
            Err(ResolveError::Unknown)
        );
    }

    #[test]
    fn the_command_list_skips_overrides_and_ranks_prefixes_first() {
        let ids: Vec<&str> = listable().map(|entry| entry.id).collect();
        assert!(!ids.iter().any(|id| id.starts_with("apps.") || id.starts_with("clipboard.")));
        assert_eq!(ids.len(), 16);
        let found = search("calc", Mode::Search, None);
        assert_eq!(found.first().unwrap().id, "tool.calculator");
        let found = search("设置", Mode::Search, None);
        assert!(found.iter().any(|entry| entry.id == "settings.open"));
    }
}
