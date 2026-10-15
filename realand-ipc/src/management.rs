//! Versioned, application-independent layout persistence. No application commands are stored.
#![allow(missing_docs)]
use crate::TilingMode;
use serde::{Deserialize, Serialize};

/// Maximum split-tree depth accepted by layout restoration and live insertion.
pub const MAX_SESSION_TREE_DEPTH: usize = 32;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionLayout {
    pub version: u32,
    pub workspaces: Vec<SessionWorkspace>,
    pub windows: Vec<SessionWindow>,
    pub groups: Vec<SessionGroup>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionWorkspace {
    pub name: Option<String>,
    #[serde(default)]
    pub index: usize,
    pub output: Option<String>,
    pub mode: TilingMode,
    pub tree: Option<SessionTree>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionTree {
    Leaf(usize),
    Split {
        vertical: bool,
        ratio: f64,
        first: Box<SessionTree>,
        second: Box<SessionTree>,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionWindow {
    pub app_id: String,
    pub title: String,
    pub workspace: usize,
    pub column: Option<usize>,
    pub display: crate::ColumnDisplay,
    #[serde(default)]
    pub sizing: Option<SessionSizing>,
    pub floating: bool,
    pub size: (i32, i32),
    pub position: Option<(f64, f64)>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSizing {
    pub width: f64,
    pub proportional: bool,
    pub full_width: bool,
    pub height: SessionHeight,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionHeight {
    Auto(f64),
    Fixed(f64),
    Preset(usize),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionGroup {
    pub name: String,
    pub members: Vec<usize>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowGroup {
    pub name: String,
    pub windows: Vec<u64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagementState {
    pub groups: Vec<WindowGroup>,
    pub undo_steps: usize,
    pub redo_steps: usize,
    pub session: SessionLayout,
}
impl SessionLayout {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err("不支持的会话版本".into());
        }
        if self.windows.len() > 4096 || self.workspaces.len() > 256 || self.groups.len() > 256 {
            return Err("会话布局超出大小限制".into());
        }
        let mut workspace_names = std::collections::HashSet::new();
        for workspace in &self.workspaces {
            if let Some(name) = &workspace.name {
                if !workspace_names.insert(name.to_ascii_lowercase()) {
                    return Err("重复的工作区名称".into());
                }
            }
        }
        for window in &self.windows {
            if let Some(sizing) = &window.sizing {
                let height_valid = match sizing.height {
                    SessionHeight::Auto(w) => w.is_finite() && w > 0.,
                    SessionHeight::Fixed(h) => h.is_finite() && h > 0. && h <= 100000.,
                    SessionHeight::Preset(i) => i < 256,
                };
                if !sizing.width.is_finite()
                    || sizing.width < 0.
                    || sizing.width > 100000.
                    || !height_valid
                {
                    return Err("无效的列尺寸".into());
                }
            }
            if window.workspace >= self.workspaces.len()
                || window.size.0 <= 0
                || window.size.1 <= 0
                || window.size.0 > 100000
                || window.size.1 > 100000
                || window
                    .position
                    .is_some_and(|(x, y)| !x.is_finite() || !y.is_finite())
            {
                return Err("无效的窗口布局".into());
            }
        }
        fn check(
            tree: &SessionTree,
            windows: &[SessionWindow],
            workspace: usize,
            depth: usize,
            seen: &mut Vec<usize>,
        ) -> bool {
            if depth > MAX_SESSION_TREE_DEPTH {
                return false;
            }
            match tree {
                SessionTree::Leaf(i) => {
                    if windows
                        .get(*i)
                        .is_none_or(|w| w.workspace != workspace || w.floating)
                        || seen.contains(i)
                    {
                        return false;
                    }
                    seen.push(*i);
                    true
                }
                SessionTree::Split {
                    ratio,
                    first,
                    second,
                    ..
                } => {
                    ratio.is_finite()
                        && (0.1..=0.9).contains(ratio)
                        && check(first, windows, workspace, depth + 1, seen)
                        && check(second, windows, workspace, depth + 1, seen)
                }
            }
        }
        for (idx, ws) in self.workspaces.iter().enumerate() {
            if let Some(tree) = &ws.tree {
                if !check(tree, &self.windows, idx, 0, &mut Vec::new()) {
                    return Err("无效的动态平铺树".into());
                }
            }
        }
        let mut members = Vec::new();
        let mut names = Vec::new();
        for group in &self.groups {
            if group.name.trim().is_empty() || group.name.len() > 128 || names.contains(&group.name)
            {
                return Err("无效或重复的分组名称".into());
            }
            names.push(group.name.clone());
            for member in &group.members {
                if *member >= self.windows.len() || members.contains(member) {
                    return Err("重复或无效的分组成员".into());
                }
                members.push(*member);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_tree_depth_matches_live_layout_limit() {
        let window = SessionWindow {
            app_id: "app".into(),
            title: "title".into(),
            workspace: 0,
            column: None,
            display: crate::ColumnDisplay::Dwindle,
            sizing: None,
            floating: false,
            size: (300, 200),
            position: None,
        };
        let mut session = SessionLayout {
            version: 1,
            workspaces: vec![SessionWorkspace {
                name: None,
                index: 0,
                output: None,
                mode: TilingMode::Dwindle,
                tree: Some(SessionTree::Leaf(0)),
            }],
            windows: vec![window.clone()],
            groups: vec![],
        };
        for depth in 1..=MAX_SESSION_TREE_DEPTH + 1 {
            let first = session.workspaces[0].tree.take().unwrap();
            session.windows.push(window.clone());
            session.workspaces[0].tree = Some(SessionTree::Split {
                vertical: false,
                ratio: 0.5,
                first: Box::new(first),
                second: Box::new(SessionTree::Leaf(depth)),
            });
            assert_eq!(session.validate().is_ok(), depth <= MAX_SESSION_TREE_DEPTH);
        }
    }
    #[test]
    fn rejects_workspace_names_that_alias_case_insensitively() {
        let workspace = SessionWorkspace {
            name: Some("Work".into()),
            index: 0,
            output: None,
            mode: TilingMode::Scrolling,
            tree: None,
        };
        let mut other = workspace.clone();
        other.name = Some("work".into());
        let session = SessionLayout {
            version: 1,
            workspaces: vec![workspace, other],
            windows: vec![],
            groups: vec![],
        };
        assert!(session.validate().is_err());
    }

    #[test]
    fn rejects_unknown_versions_and_invalid_window_destinations() {
        let mut session = SessionLayout {
            version: 2,
            workspaces: vec![],
            windows: vec![],
            groups: vec![],
        };
        assert!(session.validate().is_err());
        session.version = 1;
        session.windows.push(SessionWindow {
            app_id: "app".into(),
            title: "title".into(),
            workspace: 9,
            column: None,
            display: crate::ColumnDisplay::Normal,
            sizing: None,
            floating: true,
            size: (300, 200),
            position: None,
        });
        assert!(session.validate().is_err());
    }
    #[test]
    fn rejects_duplicate_groups_and_out_of_range_tree_leaves() {
        let mut session = SessionLayout {
            version: 1,
            workspaces: vec![SessionWorkspace {
                name: None,
                index: 0,
                output: None,
                mode: TilingMode::Dwindle,
                tree: Some(SessionTree::Leaf(10)),
            }],
            windows: vec![],
            groups: vec![],
        };
        assert!(session.validate().is_err());
        session.workspaces[0].tree = None;
        session.groups = vec![
            SessionGroup {
                name: "same".into(),
                members: vec![],
            },
            SessionGroup {
                name: "same".into(),
                members: vec![],
            },
        ];
        assert!(session.validate().is_err());
    }
}
