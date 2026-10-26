// SPDX-License-Identifier: GPL-3.0-or-later
//! The palette's action catalog.
//!
//! Clavis exposes its IPC action set as a searchable page (`/actions`). Clarion
//! keeps the same shape with a curated subset: the compositor actions Sway-like
//! sessions drive every day, plus the shell's own panels and settings toggles.
//! Entries whose id matches a [`crate::commands`] action reuse that dispatch;
//! the rest map to a `clarion_ipc::Action` in the shell.
//!
//! This module stays toolkit-independent: the table, the matcher and the ids
//! are testable without GPUI or a compositor.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaletteAction {
    /// Dispatch id. Either a `commands` action id or a compositor action id.
    pub id: &'static str,
    pub title: &'static str,
    /// The section the row belongs to ("窗口", "列", "工作区", …).
    pub group: &'static str,
    pub keywords: &'static [&'static str],
}

macro_rules! action {
    ($id:literal, $title:literal, $group:literal, $keywords:expr) => {
        PaletteAction {
            id: $id,
            title: $title,
            group: $group,
            keywords: $keywords,
        }
    };
}

pub const ACTIONS: &[PaletteAction] = &[
    // Windows.
    action!("window.close", "关闭窗口", "窗口", &["close", "close-window", "关闭", "关窗"]),
    action!("window.fullscreen", "全屏切换", "窗口", &["fullscreen", "全屏"]),
    action!(
        "window.windowed-fullscreen",
        "窗口化全屏",
        "窗口",
        &["windowed-fullscreen", "fake-fullscreen", "窗口化全屏"]
    ),
    action!("window.minimize", "最小化窗口", "窗口", &["minimize", "最小化"]),
    action!("window.focus-previous", "上一个窗口", "窗口", &["previous", "上一个", "切回"]),
    action!("window.focus-up", "聚焦上方窗口", "窗口", &["focus-up", "up", "上"]),
    action!("window.focus-down", "聚焦下方窗口", "窗口", &["focus-down", "down", "下"]),
    action!("window.focus-top", "最上层窗口", "窗口", &["top", "最上层", "顶层"]),
    action!("window.focus-bottom", "最底层窗口", "窗口", &["bottom", "最底层", "底层"]),
    action!("window.move-up", "窗口上移", "窗口", &["move-up", "上移"]),
    action!("window.move-down", "窗口下移", "窗口", &["move-down", "下移"]),
    action!("window.swap-left", "与左侧窗口交换", "窗口", &["swap", "left", "交换", "左"]),
    action!("window.swap-right", "与右侧窗口交换", "窗口", &["swap", "right", "交换", "右"]),
    action!(
        "window.consume-left",
        "向左吸收窗口",
        "窗口",
        &["consume", "left", "吸收", "左"]
    ),
    action!(
        "window.consume-right",
        "向右吸收窗口",
        "窗口",
        &["consume", "right", "吸收", "右"]
    ),
    action!("window.expel", "从列中弹出窗口", "窗口", &["expel", "弹出"]),
    action!("window.center", "居中窗口", "窗口", &["center", "居中"]),
    // Columns.
    action!("column.focus-left", "聚焦左列", "列", &["column", "left", "左列"]),
    action!("column.focus-right", "聚焦右列", "列", &["column", "right", "右列"]),
    action!("column.focus-first", "聚焦首列", "列", &["column", "first", "首列"]),
    action!("column.focus-last", "聚焦末列", "列", &["column", "last", "末列"]),
    action!(
        "column.focus-left-wrap",
        "左列（循环）",
        "列",
        &["column", "left", "wrap", "循环"]
    ),
    action!(
        "column.focus-right-wrap",
        "右列（循环）",
        "列",
        &["column", "right", "wrap", "循环"]
    ),
    action!("column.move-left", "列左移", "列", &["move-column", "left", "列左移"]),
    action!("column.move-right", "列右移", "列", &["move-column", "right", "列右移"]),
    action!("column.move-first", "列移至最前", "列", &["move-column", "first", "最前"]),
    action!("column.move-last", "列移至最后", "列", &["move-column", "last", "最后"]),
    action!(
        "column.toggle-tabbed",
        "列标签页显示",
        "列",
        &["tabbed", "tabs", "标签页", "标签"]
    ),
    action!(
        "column.toggle-dwindle",
        "列瀑布显示",
        "列",
        &["dwindle", "瀑布", "平铺"]
    ),
    action!("column.center", "居中列", "列", &["center-column", "居中列"]),
    action!(
        "column.center-visible",
        "居中所有可见列",
        "列",
        &["center-visible", "可见列", "居中"]
    ),
    // Workspaces.
    action!(
        "workspace.focus-down",
        "下一个工作区",
        "工作区",
        &["workspace", "next", "下一工作区", "下"]
    ),
    action!(
        "workspace.focus-up",
        "上一个工作区",
        "工作区",
        &["workspace", "previous", "上一工作区", "上"]
    ),
    action!(
        "workspace.focus-previous",
        "上次的工作区",
        "工作区",
        &["workspace", "previous", "上次"]
    ),
    action!("workspace.move-down", "工作区下移", "工作区", &["move-workspace", "下移"]),
    action!("workspace.move-up", "工作区上移", "工作区", &["move-workspace", "上移"]),
    action!(
        "workspace.window-down",
        "窗口送到下一工作区",
        "工作区",
        &["window", "workspace", "next", "送往下"]
    ),
    action!(
        "workspace.window-up",
        "窗口送到上一工作区",
        "工作区",
        &["window", "workspace", "previous", "送往"]
    ),
    action!(
        "workspace.column-down",
        "列送到下一工作区",
        "工作区",
        &["column", "workspace", "next", "列送往下"]
    ),
    action!(
        "workspace.column-up",
        "列送到上一工作区",
        "工作区",
        &["column", "workspace", "previous", "列送往"]
    ),
    // Monitors.
    action!("monitor.focus-left", "聚焦左侧显示器", "显示器", &["monitor", "left", "显示器", "左"]),
    action!(
        "monitor.focus-right",
        "聚焦右侧显示器",
        "显示器",
        &["monitor", "right", "显示器", "右"]
    ),
    action!("monitor.focus-down", "聚焦下方显示器", "显示器", &["monitor", "down", "显示器", "下"]),
    action!("monitor.focus-up", "聚焦上方显示器", "显示器", &["monitor", "up", "显示器", "上"]),
    action!(
        "monitor.focus-previous",
        "上一个显示器",
        "显示器",
        &["monitor", "previous", "上一個"]
    ),
    action!("monitor.focus-next", "下一个显示器", "显示器", &["monitor", "next", "下一个"]),
    action!(
        "monitor.window-left",
        "窗口移到左侧显示器",
        "显示器",
        &["window", "monitor", "left", "移到左"]
    ),
    action!(
        "monitor.window-right",
        "窗口移到右侧显示器",
        "显示器",
        &["window", "monitor", "right", "移到右"]
    ),
    action!(
        "monitor.window-down",
        "窗口移到下方显示器",
        "显示器",
        &["window", "monitor", "down", "移到下"]
    ),
    action!(
        "monitor.window-up",
        "窗口移到上方显示器",
        "显示器",
        &["window", "monitor", "up", "移到上"]
    ),
    action!(
        "monitor.column-left",
        "列移到左侧显示器",
        "显示器",
        &["column", "monitor", "left", "列移到左"]
    ),
    action!(
        "monitor.column-right",
        "列移到右侧显示器",
        "显示器",
        &["column", "monitor", "right", "列移到右"]
    ),
    action!(
        "monitor.column-next",
        "列移到下一个显示器",
        "显示器",
        &["column", "monitor", "next", "列转移到"]
    ),
    // Screens and the session.
    action!("system.power-off-monitors", "关闭显示器", "系统", &["dpms", "off", "关屏", "熄屏"]),
    action!("system.power-on-monitors", "打开显示器", "系统", &["dpms", "on", "开屏", "亮屏"]),
    action!(
        "system.screen-transition",
        "屏幕过渡动画",
        "系统",
        &["transition", "过渡", "动画"]
    ),
    action!(
        "system.inhibit-shortcuts",
        "切换快捷键抑制",
        "系统",
        &["inhibit", "shortcuts", "抑制", "快捷键"]
    ),
    action!("system.lock", "锁定屏幕", "系统", &["lock", "锁定", "锁屏"]),
    action!("system.logout", "退出登录", "系统", &["logout", "退出登录", "注销"]),
    action!("system.suspend", "睡眠", "系统", &["suspend", "sleep", "睡眠", "挂起"]),
    action!("system.reboot", "重启", "系统", &["reboot", "restart", "重启"]),
    action!("system.shutdown", "关机", "系统", &["shutdown", "poweroff", "关机"]),
    action!("screenshot.screen", "截取整个屏幕", "系统", &["screenshot", "screen", "截图", "全屏"]),
    action!("screenshot.window", "截取当前窗口", "系统", &["screenshot", "window", "截图", "窗口"]),
    // The shell's own surfaces and toggles. These ids match the `commands`
    // table, so the palette dispatches them through the existing action path.
    action!("shell.screenshot", "截图…", "界面", &["screenshot", "region", "截图", "区域"]),
    action!("shell.notifications", "通知中心", "界面", &["notifications", "通知"]),
    action!("shell.recent", "最近内容", "界面", &["recent", "clipboard", "最近"]),
    action!("shell.control", "控制中心", "界面", &["control", "center", "控制中心"]),
    action!("settings.open", "打开设置", "界面", &["settings", "设置", "偏好"]),
    action!("settings.wallpaper", "选择壁纸", "界面", &["wallpaper", "壁纸"]),
    action!("theme.light", "浅色外观", "界面", &["light", "theme", "浅色", "亮色"]),
    action!("theme.dark", "深色外观", "界面", &["dark", "theme", "深色", "暗色"]),
    action!(
        "settings.reduce-motion",
        "减少动效",
        "界面",
        &["motion", "animation", "动效", "动画"]
    ),
    action!(
        "settings.dock-autohide",
        "程序坞自动隐藏",
        "界面",
        &["dock", "autohide", "程序坞", "自动隐藏"]
    ),
    action!(
        "settings.launcher.fullscreen",
        "启动台：全屏",
        "界面",
        &["launcher", "fullscreen", "启动台", "全屏"]
    ),
    action!(
        "settings.launcher.palette",
        "启动台：弹窗",
        "界面",
        &["launcher", "palette", "启动台", "弹窗"]
    ),
];

pub fn by_id(id: &str) -> Option<&'static PaletteAction> {
    ACTIONS.iter().find(|action| action.id == id)
}

/// Ranked matches for the actions page. An empty query keeps the table order;
/// otherwise the title leads, then keywords, then the group name.
pub fn search(query: &str) -> Vec<&'static PaletteAction> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return ACTIONS.iter().collect();
    }
    let mut scored: Vec<(i64, usize, &'static PaletteAction)> = ACTIONS
        .iter()
        .enumerate()
        .filter_map(|(index, action)| {
            score(action, &query).map(|score| (score, index, action))
        })
        .collect();
    scored.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    scored.into_iter().map(|(_, _, action)| action).collect()
}

fn score(action: &PaletteAction, query: &str) -> Option<i64> {
    let title = action.title.to_lowercase();
    if title == query {
        return Some(1000);
    }
    if title.starts_with(query) {
        return Some(900);
    }
    if let Some(position) = title.find(query) {
        return Some(800 - position as i64);
    }
    if action.keywords.contains(&query) {
        return Some(700);
    }
    if let Some(position) = action
        .keywords
        .iter()
        .filter_map(|keyword| keyword.find(query))
        .min()
    {
        return Some(600 - position as i64);
    }
    if action.group.to_lowercase().contains(query) {
        return Some(400);
    }
    if is_subsequence(query, &title) {
        return Some(300);
    }
    None
}

fn is_subsequence(query: &str, haystack: &str) -> bool {
    let mut chars = haystack.chars();
    query.chars().all(|needle| chars.any(|hay| hay == needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_is_searchable_by_title_keyword_and_group() {
        assert_eq!(search("").len(), ACTIONS.len());
        assert_eq!(search("居中窗口")[0].id, "window.center");
        assert_eq!(search("tabbed")[0].id, "column.toggle-tabbed");
        assert_eq!(search("工作区")[0].group, "工作区");
        assert!(search("no-such-action").is_empty());
        assert_eq!(
            search("关闭窗口")[0].id,
            "window.close",
            "the exact title leads"
        );
    }

    #[test]
    fn the_catalog_has_one_entry_per_id() {
        let mut ids: Vec<_> = ACTIONS.iter().map(|action| action.id).collect();
        ids.sort_unstable();
        let total = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), total, "a dispatch id appears twice");
        assert!(ACTIONS.len() >= 60, "the curated set stays useful");
        assert!(by_id("window.close").is_some());
        assert!(by_id("nope").is_none());
    }
}
