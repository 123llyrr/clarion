// SPDX-License-Identifier: GPL-3.0-or-later
//! The menu dropped from the bar's leading Clarion mark.
//!
//! It is a static list: rows, rules and icons live here, while
//! `app::panels::system_menu_height` mirrors the same rhythm. Every action is
//! an existing `Action`, so the menu adds presentation rather than behaviour.
//! Icons come from the installed icon theme, like the Spotlight command rows;
//! the only bundled fallbacks are assets the project already shipped.
use super::*;

impl ShellView {
    pub(super) fn system_menu(&self, host: &ViewState, p: Palette) -> AnyElement {
        let mut menu = div().flex().flex_col().gap(px(6.));

        // The About row uses the same row recipe and weight as the rest of the
        // menu: the shell's CJK face has no semibold counterpart, and asking
        // for one made this row fall back to a visibly different font.
        menu = menu.child(self.row_button(
            "system-menu-about",
            "关于本机".into(),
            Action::Panel(Panel::About),
            p,
            self.named_icon("laptop-symbolic", 16., p),
            None,
        ));
        menu = menu
            .child(self.system_menu_row(
                p,
                "settings",
                "系统设置…",
                ("preferences-system-symbolic", Some("settings.svg")),
                Action::Panel(Panel::Preferences),
            ))
            .child(self.system_menu_row(
                p,
                "recent",
                "最近使用的项目",
                ("document-open-recent-symbolic", Some("menu-folder-open.svg")),
                Action::Panel(Panel::Recent),
            ))
            .child(self.separator(p))
            .child(self.system_menu_row(
                p,
                "sleep",
                "睡眠",
                ("system-suspend-symbolic", Some("moon.svg")),
                Action::System(SystemAction::Suspend),
            ))
            .child(self.system_menu_row(
                p,
                "reboot",
                "重新启动…",
                ("system-reboot-symbolic", None),
                Action::System(SystemAction::Reboot),
            ))
            .child(self.system_menu_row(
                p,
                "shutdown",
                "关机…",
                ("system-shutdown-symbolic", None),
                Action::System(SystemAction::Shutdown),
            ))
            .child(self.separator(p))
            .child(self.system_menu_row(
                p,
                "lock",
                "锁定屏幕",
                ("system-lock-screen-symbolic", None),
                Action::System(SystemAction::Lock),
            ))
            .child(self.system_menu_row(
                p,
                "logout",
                format!("退出登录 “{}”…", host.user_name),
                ("system-log-out-symbolic", None),
                Action::System(SystemAction::Logout),
            ));
        menu.into_any_element()
    }

    fn separator(&self, p: Palette) -> AnyElement {
        div().h(px(1.)).my_2().bg(p.line).into_any_element()
    }

    fn system_menu_row(
        &self,
        p: Palette,
        id: &'static str,
        label: impl Into<String>,
        icon: (&'static str, Option<&'static str>),
        action: Action,
    ) -> AnyElement {
        let leading = self.menu_symbol(icon.0, icon.1, 16., p);
        self.row_button(
            format!("system-menu-{id}"),
            label.into(),
            action,
            p,
            leading,
            None,
        )
    }

    /// The theme's symbolic mark. Rows whose glyph predates the system menu
    /// keep their shipped fallback; the session rows are theme-only, so no new
    /// artwork is added to the repository.
    fn menu_symbol(
        &self,
        symbol: &str,
        asset: Option<&'static str>,
        size: f32,
        p: Palette,
    ) -> Option<AnyElement> {
        if let Some(icon) = self.named_icon(symbol, size, p) {
            return Some(icon);
        }
        let asset = asset?;
        let path = crate::icons::asset_svg(asset);
        path.is_file().then(|| {
            svg()
                .path(path.to_string_lossy().into_owned())
                .size(px(size))
                .text_color(p.fg)
                .into_any_element()
        })
    }
}
