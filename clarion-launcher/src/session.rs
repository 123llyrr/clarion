// SPDX-License-Identifier: GPL-3.0-or-later
//! Palette navigation state, ported from Clavis `SpotlightSession.js`.
//!
//! The session never touches preferences or executes actions: it only decides
//! which provider a query belongs to, which override applies, and where
//! `Backspace`/`Escape` return to.
use crate::commands::ENTRIES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Search,
    Apps,
    Wallpapers,
    Clipboard,
    Files,
    Commands,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Web,
    Settings,
    Actions,
    Calculator,
    Currency,
    Time,
}

impl Mode {
    pub const ALL: [Self; 6] = [
        Self::Search,
        Self::Apps,
        Self::Wallpapers,
        Self::Clipboard,
        Self::Files,
        Self::Commands,
    ];
    /// The four buttons on the mode rail (`Ctrl+1..4`).
    pub const RAIL: [Self; 4] = [Self::Apps, Self::Wallpapers, Self::Clipboard, Self::Files];

    pub fn value(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Apps => "apps",
            Self::Wallpapers => "wallpapers",
            Self::Clipboard => "clipboard",
            Self::Files => "files",
            Self::Commands => "commands",
        }
    }

    pub fn from_value(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.value() == value)
    }
}

impl Tool {
    pub fn value(self) -> &'static str {
        match self {
            Self::Web => "web",
            Self::Settings => "settings",
            Self::Actions => "actions",
            Self::Calculator => "calculator",
            Self::Currency => "currency",
            Self::Time => "time",
        }
    }

    pub fn from_value(value: &str) -> Option<Self> {
        [
            Self::Web,
            Self::Settings,
            Self::Actions,
            Self::Calculator,
            Self::Currency,
            Self::Time,
        ]
        .into_iter()
        .find(|tool| tool.value() == value)
    }

    /// Tools whose result is plain text that can be copied.
    pub fn yields_value(self) -> bool {
        matches!(self, Self::Calculator | Self::Currency | Self::Time)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Override {
    pub key: &'static str,
    pub value: &'static str,
}

/// Where the typed text goes in the current session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    Query(String),
    Commands(String),
    Slash { name: String, arguments: String },
    Literal(String),
}

#[derive(Clone, Copy, Debug, Default)]
pub struct BackspaceState {
    pub selection: bool,
    pub preedit: bool,
    pub modal: bool,
    pub completion: bool,
    pub repeat: bool,
    pub focused: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub mode: Mode,
    pub tool: Option<Tool>,
    pub query: String,
    /// The query is literal: prefixes are not interpreted again.
    pub literal: bool,
    pub selection: String,
    pub overrides: Vec<Override>,
    pub parent: Option<Box<Session>>,
    pub serial: u64,
}

impl Default for Session {
    fn default() -> Self {
        Self::new(Mode::Search, 0)
    }
}

impl Session {
    pub fn new(mode: Mode, serial: u64) -> Self {
        Self {
            mode,
            tool: None,
            query: String::new(),
            literal: false,
            selection: String::new(),
            overrides: Vec::new(),
            parent: None,
            serial,
        }
    }

    /// The mode the panel renders: a tool replaces its base mode.
    pub fn view_mode(&self) -> &'static str {
        self.tool.map_or(self.mode.value(), Tool::value)
    }

    pub fn input(mut self, text: &str, literal: bool, selection: &str) -> Self {
        self.query = text.to_owned();
        self.literal = literal;
        self.selection = selection.to_owned();
        self
    }

    pub fn switch_mode(self, mode: Mode, preserve_query: bool) -> Self {
        let mut next = Self::new(mode, self.serial.wrapping_add(1));
        let reusable = !self.tool.is_some()
            && self.mode != Mode::Commands
            && (self.literal || !self.query.starts_with('/') && !self.query.starts_with('>'));
        if preserve_query && reusable {
            next.query = self.query;
            next.literal = self.literal;
        }
        next
    }

    pub fn enter_commands(self, query: &str) -> Self {
        let parent = self.clone();
        let mut next = self.switch_mode(Mode::Commands, false);
        next.parent = Some(Box::new(parent));
        next.query = query.to_owned();
        next.literal = false;
        next
    }

    pub fn enter_tool(self, tool: Tool, query: &str, consume_command: bool) -> Self {
        let mut parent = match &self.tool {
            Some(_) => self.parent.as_deref().cloned().unwrap_or_else(|| self.clone()),
            None => self.clone(),
        };
        if consume_command && self.tool.is_none() {
            let selection = parent.selection.clone();
            parent = parent.input("", false, &selection);
        }
        let mut next = parent.clone();
        next.tool = Some(tool);
        next.query = query.to_owned();
        next.literal = false;
        next.selection.clear();
        next.parent = Some(Box::new(parent));
        next.serial = self.serial.wrapping_add(1);
        next
    }

    /// Resolve the saved value against the current override stack.
    pub fn effective(&self, key: &str, saved: &'static str) -> &'static str {
        self.overrides
            .iter()
            .rev()
            .find(|entry| entry.key == key)
            .map_or(saved, |entry| entry.value)
    }

    pub fn set_override(self, key: &'static str, value: &'static str, saved: &'static str) -> Self {
        let scope = if key == "clipboardLayout" {
            Mode::Clipboard
        } else {
            Mode::Apps
        };
        let allowed = ENTRIES.iter().any(|entry| {
            entry.kind == crate::commands::CommandKind::Override
                && entry.key == Some(key)
                && entry.override_value == Some(value)
        });
        if self.tool.is_some() || self.mode != scope || !allowed {
            return self;
        }
        let current = self.overrides.iter().find(|entry| entry.key == key).copied();
        // Even a repeated override consumes the slash draft.
        let selection = self.selection.clone();
        let mut next = self.input("", false, &selection);
        if current.is_some_and(|entry| entry.value == value) {
            return next;
        }
        next.overrides.retain(|entry| entry.key != key);
        if value != saved {
            next.overrides.push(Override { key, value });
        }
        next
    }

    pub fn pop(self) -> Self {
        if let Some(parent) = self.parent {
            let mut next = *parent;
            next.serial = self.serial.wrapping_add(1);
            return next;
        }
        if !self.overrides.is_empty() {
            let mut next = self;
            next.overrides.pop();
            next.serial = next.serial.wrapping_add(1);
            return next;
        }
        self
    }

    pub fn can_backspace(&self, state: BackspaceState) -> bool {
        self.query.is_empty()
            && !state.selection
            && !state.preedit
            && !state.modal
            && !state.completion
            && !state.repeat
            && state.focused
            && (self.parent.is_some() || !self.overrides.is_empty())
    }

    pub fn route(&self, text: &str) -> Route {
        if self.tool.is_some() || self.literal {
            return Route::Query(text.to_owned());
        }
        if let Some(rest) = text.strip_prefix('\\').and_then(|rest| {
            (rest.starts_with('/') || rest.starts_with('>')).then_some(rest)
        }) {
            return Route::Literal(rest.to_owned());
        }
        if self.mode != Mode::Commands {
            if let Some(rest) = text.strip_prefix('>') {
                return Route::Commands(rest.to_owned());
            }
        }
        if let Some(rest) = text.strip_prefix('/') {
            let mut parts = rest.splitn(2, char::is_whitespace);
            let name = parts.next().unwrap_or_default().to_lowercase();
            let arguments = parts.next().unwrap_or_default().trim_start().to_owned();
            return Route::Slash { name, arguments };
        }
        Route::Query(text.to_owned())
    }

    /// Apply one already-resolved command. Modes replace the session (a slash
    /// mode command does not carry the draft query along, matching the
    /// reference implementation), tools keep a parent for `Backspace`, and
    /// overrides only stack inside their own mode.
    pub fn apply_entry(
        self,
        command: &'static crate::commands::Command,
        arguments: &str,
        saved: SavedStyles,
    ) -> Self {
        use crate::commands::CommandKind;
        match command.kind {
            CommandKind::Mode => {
                let mode =
                    Mode::from_value(command.value.unwrap_or_default()).unwrap_or(Mode::Search);
                self.switch_mode(mode, false)
            }
            CommandKind::Tool => match Tool::from_value(command.value.unwrap_or_default()) {
                Some(tool) => self.enter_tool(tool, arguments, true),
                None => self,
            },
            CommandKind::Override => self.set_override(
                command.key.unwrap_or_default(),
                command.override_value.unwrap_or_default(),
                saved.for_key(command.key.unwrap_or_default()),
            ),
            CommandKind::Action => {
                let selection = self.selection.clone();
                self.input("", false, &selection)
            }
        }
    }
}

/// The preference values an override compares itself against.
#[derive(Clone, Copy, Debug)]
pub struct SavedStyles {
    pub apps_layout: &'static str,
    pub apps_order: &'static str,
    pub clipboard_layout: &'static str,
}

impl SavedStyles {
    fn for_key(self, key: &str) -> &'static str {
        match key {
            "appsLayout" => self.apps_layout,
            "appsOrder" => self.apps_order,
            _ => self.clipboard_layout,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_route_to_the_right_provider() {
        let session = Session::default();
        assert_eq!(session.route("firefox"), Route::Query("firefox".into()));
        assert_eq!(
            session.route("/calc 1+1"),
            Route::Slash {
                name: "calc".into(),
                arguments: "1+1".into()
            }
        );
        assert_eq!(
            session.route("/") ,
            Route::Slash {
                name: String::new(),
                arguments: String::new()
            }
        );
        assert_eq!(session.route(">apps"), Route::Commands("apps".into()));
        assert_eq!(session.route("\\/calc"), Route::Literal("/calc".into()));
        assert_eq!(session.route("\\>quote"), Route::Literal(">quote".into()));
    }

    #[test]
    fn tools_and_literal_queries_never_reparse_prefixes() {
        let tool = Session::new(Mode::Search, 0).enter_tool(Tool::Calculator, "", false);
        assert_eq!(tool.route("/calc"), Route::Query("/calc".into()));
        assert_eq!(tool.route(">x"), Route::Query(">x".into()));
        let literal = Session::new(Mode::Search, 0).input("/calc", true, "");
        assert_eq!(literal.route("/calc"), Route::Query("/calc".into()));
    }

    #[test]
    fn commands_mode_treats_a_leading_angle_as_text() {
        let commands = Session::new(Mode::Commands, 0);
        assert_eq!(commands.route(">x"), Route::Query(">x".into()));
        assert_eq!(commands.route("/apps"), Route::Slash { name: "apps".into(), arguments: String::new() });
    }

    #[test]
    fn switching_modes_preserves_safe_queries_only() {
        let typed = Session::new(Mode::Apps, 0).input("fire", false, "");
        assert_eq!(typed.switch_mode(Mode::Files, true).query, "fire");
        let slash = Session::new(Mode::Apps, 0).input("/grid", false, "");
        assert_eq!(slash.switch_mode(Mode::Files, true).query, "");
        let from_tool = Session::new(Mode::Apps, 0)
            .enter_tool(Tool::Calculator, "1+1", false)
            .switch_mode(Mode::Files, true);
        assert_eq!(from_tool.query, "");
        assert_eq!(from_tool.mode, Mode::Files);
        assert_eq!(from_tool.overrides, Vec::new());
    }

    #[test]
    fn entering_a_tool_keeps_one_parent_and_returns_to_it() {
        let base = Session::new(Mode::Search, 3).input("draft", false, "sel");
        let calc = base.clone().enter_tool(Tool::Calculator, "1+1", true);
        // The command text is consumed from the parent, but the mode survives.
        assert_eq!(calc.mode, Mode::Search);
        assert_eq!(calc.parent.as_ref().unwrap().query, "");
        let currency = calc.clone().enter_tool(Tool::Currency, "10 usd", true);
        assert_eq!(currency.parent.as_ref().unwrap().mode, Mode::Search);
        assert_eq!(currency.parent.as_ref().unwrap().tool, None);
        assert_eq!(currency.parent.as_ref().unwrap().query, "");
        let popped = currency.pop();
        assert_eq!(popped.mode, Mode::Search);
        assert_eq!(popped.tool, None);
        assert!(popped.parent.is_none());
    }

    #[test]
    fn commands_remember_their_parent() {
        let session = Session::new(Mode::Apps, 0).input("calc", false, "");
        let commands = session.clone().enter_commands("");
        assert_eq!(commands.mode, Mode::Commands);
        assert_eq!(commands.parent.as_ref().unwrap().mode, Mode::Apps);
        let popped = commands.pop();
        assert_eq!(popped.mode, Mode::Apps);
        assert_eq!(popped.query, "calc");
    }

    #[test]
    fn overrides_only_apply_in_their_mode_and_consume_the_draft() {
        let draft = Session::new(Mode::Apps, 0).input("/grid", false, "");
        let grid = draft.set_override("appsLayout", "grid", "list");
        assert!(grid.query.is_empty());
        assert_eq!(grid.effective("appsLayout", "list"), "grid");
        // Repeating the override clears the draft but does not stack.
        let again = grid.clone().input("/grid", false, "").set_override("appsLayout", "grid", "list");
        assert_eq!(again.overrides.len(), 1);
        // The saved value removes the override.
        let saved = grid.set_override("appsLayout", "list", "list");
        assert!(saved.overrides.is_empty());
        // Wrong mode / tool scope leaves the session untouched.
        let clipboard = Session::new(Mode::Clipboard, 0).input("/grid", false, "");
        assert_eq!(clipboard.clone().set_override("appsLayout", "grid", "list"), clipboard);
    }

    #[test]
    fn pop_unwinds_overrides_only_when_no_parent_exists() {
        let stacked = Session::new(Mode::Apps, 0)
            .input("/grid", false, "")
            .set_override("appsLayout", "grid", "list")
            .input("/list", false, "")
            .set_override("appsLayout", "list", "list");
        let once = stacked.pop();
        assert!(once.overrides.is_empty());
        let twice = once.pop();
        assert!(twice.overrides.is_empty());
        assert_eq!(twice.mode, Mode::Apps);
    }

    #[test]
    fn backspace_only_unwinds_on_an_empty_focused_query() {
        let nested = Session::new(Mode::Search, 0).enter_tool(Tool::Calculator, "", false);
        assert!(nested.can_backspace(BackspaceState {
            focused: true,
            ..Default::default()
        }));
        assert!(!nested.can_backspace(BackspaceState {
            focused: true,
            preedit: true,
            ..Default::default()
        }));
        let plain = Session::new(Mode::Search, 0);
        assert!(!plain.can_backspace(BackspaceState {
            focused: true,
            ..Default::default()
        }));
        let with_override = Session::new(Mode::Apps, 0)
            .input("/grid", false, "")
            .set_override("appsLayout", "grid", "list");
        assert!(with_override.can_backspace(BackspaceState {
            focused: true,
            ..Default::default()
        }));
    }

    #[test]
    fn applying_a_slash_command_updates_the_session() {
        fn saved() -> SavedStyles {
            SavedStyles {
                apps_layout: "list",
                apps_order: "name",
                clipboard_layout: "default",
            }
        }
        fn apply(
            session: Session,
            name: &str,
            arguments: &str,
        ) -> (
            Session,
            Result<&'static crate::commands::Command, crate::commands::ResolveError>,
        ) {
            let resolved = crate::commands::resolve(name, arguments, session.mode, session.tool);
            match resolved {
                Ok(entry) => (session.apply_entry(entry, arguments, saved()), Ok(entry)),
                Err(error) => (session, Err(error)),
            }
        }
        let (next, entry) = apply(Session::new(Mode::Search, 0), "calc", "2*3");
        assert_eq!(entry.unwrap().id, "tool.calculator");
        assert_eq!(next.tool, Some(Tool::Calculator));
        assert_eq!(next.query, "2*3");
        let (next, entry) = apply(Session::new(Mode::Search, 0), "apps", "");
        assert_eq!(entry.unwrap().id, "mode.apps");
        assert_eq!(next.mode, Mode::Apps);
        // A slash mode command does not carry the draft along.
        assert_eq!(next.query, "");
        let (next, entry) = apply(Session::new(Mode::Apps, 0), "grid", "");
        assert_eq!(entry.unwrap().id, "apps.grid");
        assert_eq!(next.effective("appsLayout", "list"), "grid");
        // Choosing the saved value removes the override instead of stacking it.
        let (back, _) = apply(next, "list", "");
        assert_eq!(back.effective("appsLayout", "list"), "list");
        assert!(back.overrides.is_empty());
        let (next, entry) = apply(Session::new(Mode::Search, 0), "grid", "");
        assert!(entry.is_err());
        assert_eq!(next.mode, Mode::Search);
    }

    #[test]
    fn view_mode_reports_the_tool_when_one_is_open() {
        let session = Session::new(Mode::Apps, 0).enter_tool(Tool::Time, "", false);
        assert_eq!(session.view_mode(), "time");
        assert_eq!(Session::new(Mode::Clipboard, 0).view_mode(), "clipboard");
    }
}
