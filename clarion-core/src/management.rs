//! Stable-identity grouping and bounded arrangement history.
//!
//! No protocol handles or serialization types enter this owner. The platform
//! materializes a returned arrangement through its permanent client adapter.
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::Hash;

use crate::layout::BspNode;
use crate::policy_memory::{ColumnDisplay, Height, Width};
use crate::{LayoutMode, SpaceId};

const HISTORY_LIMIT: usize = 32;
const WINDOW_LIMIT: usize = 4096;
const SPACE_LIMIT: usize = 256;
const GROUP_LIMIT: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group<I> {
    pub name: String,
    pub members: Vec<I>,
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArrangementSizing {
    pub width: Width,
    pub full_width: bool,
    pub height: Height,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ArrangementWindow<I> {
    pub id: I,
    pub app_id: String,
    pub title: String,
    pub space: SpaceId,
    pub column: Option<usize>,
    pub display: ColumnDisplay,
    pub sizing: Option<ArrangementSizing>,
    pub floating: bool,
    pub size: [i32; 2],
    pub position: Option<[f64; 2]>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ArrangementSpace<I> {
    pub id: SpaceId,
    pub name: Option<String>,
    pub index: usize,
    pub output: Option<String>,
    pub mode: LayoutMode,
    pub tree: Option<BspNode<I>>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Arrangement<I> {
    pub spaces: Vec<ArrangementSpace<I>>,
    pub windows: Vec<ArrangementWindow<I>>,
    pub groups: Vec<Group<I>>,
    pub focus: Option<I>,
}
impl<I: Eq + Hash> Arrangement<I> {
    /// Validate identity/size bounds before changing history. Closed IDs in a
    /// previously accepted snapshot remain stable slots until restoration.
    pub fn valid(&self) -> bool {
        if self.windows.len() > WINDOW_LIMIT || self.spaces.len() > SPACE_LIMIT {
            return false;
        }
        let spaces: HashSet<_> = self.spaces.iter().map(|space| space.id).collect();
        let windows: HashMap<_, _> = self
            .windows
            .iter()
            .map(|window| (&window.id, window))
            .collect();
        fn valid_tree<'a, I: Eq + Hash>(
            tree: &'a BspNode<I>,
            space: SpaceId,
            windows: &HashMap<&I, &ArrangementWindow<I>>,
            seen: &mut HashSet<&'a I>,
            depth: usize,
        ) -> bool {
            if depth > 32 {
                return false;
            }
            match tree {
                BspNode::Leaf(id) => {
                    windows
                        .get(id)
                        .is_some_and(|window| window.space == space && !window.floating)
                        && seen.insert(id)
                }
                BspNode::Split {
                    ratio,
                    first,
                    second,
                    ..
                } => {
                    ratio.is_finite()
                        && (0.1..=0.9).contains(ratio)
                        && valid_tree(first, space, windows, seen, depth + 1)
                        && valid_tree(second, space, windows, seen, depth + 1)
                }
            }
        }
        spaces.len() == self.spaces.len()
            && windows.len() == self.windows.len()
            && self.windows.iter().all(|window| {
                let sizing_valid = window.sizing.is_none_or(|sizing| {
                    let width = match sizing.width {
                        Width::Fixed(value) | Width::Proportion(value) => value,
                    };
                    width.is_finite()
                        && width >= 0.
                        && match sizing.height {
                            Height::Auto(value) | Height::Fixed(value) => {
                                value.is_finite() && value > 0.
                            }
                            Height::Preset(_) => true,
                        }
                });
                spaces.contains(&window.space)
                    && sizing_valid
                    && window.size.iter().all(|size| *size > 0)
                    && window
                        .position
                        .is_none_or(|position| position.iter().all(|value| value.is_finite()))
            })
            && self
                .focus
                .as_ref()
                .is_none_or(|id| windows.contains_key(id))
            && self.spaces.iter().all(|space| {
                space
                    .tree
                    .as_ref()
                    .is_none_or(|tree| valid_tree(tree, space.id, &windows, &mut HashSet::new(), 0))
            })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ManagementAction<I> {
    Checkpoint(Arrangement<I>),
    Undo {
        redo: bool,
        current: Arrangement<I>,
    },
    SetGroup {
        name: String,
        members: Vec<I>,
        current: Arrangement<I>,
    },
    RemoveGroup {
        name: String,
        current: Arrangement<I>,
    },
    MoveGroup {
        name: String,
        target: SpaceId,
        current: Arrangement<I>,
    },
    FollowGroup {
        window: I,
        target: SpaceId,
    },
    /// Session restoration supplies already matched stable IDs.
    ReplaceGroups(Vec<Group<I>>),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupMoveIntent<I> {
    pub target: SpaceId,
    pub members: Vec<I>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ManagementOutcome<I> {
    pub changed: bool,
    pub restore: Option<Arrangement<I>>,
    pub move_group: Option<GroupMoveIntent<I>>,
}
impl<I> Default for ManagementOutcome<I> {
    fn default() -> Self {
        Self {
            changed: false,
            restore: None,
            move_group: None,
        }
    }
}
#[derive(Debug, Clone)]
pub struct ManagementState<I> {
    groups: Vec<Group<I>>,
    undo: VecDeque<Arrangement<I>>,
    redo: VecDeque<Arrangement<I>>,
}
impl<I> Default for ManagementState<I> {
    fn default() -> Self {
        Self {
            groups: Vec::new(),
            undo: VecDeque::new(),
            redo: VecDeque::new(),
        }
    }
}
impl<I: Clone + Eq + Hash> ManagementState<I> {
    pub fn groups(&self) -> &[Group<I>] {
        &self.groups
    }
    pub fn history_steps(&self) -> (usize, usize) {
        (self.undo.len(), self.redo.len())
    }
    pub fn group(&self, name: &str) -> Option<&Group<I>> {
        self.groups.iter().find(|group| group.name == name)
    }
    pub fn remove_window(&mut self, id: &I) -> bool {
        let old_count = self
            .groups
            .iter()
            .map(|group| group.members.len())
            .sum::<usize>();
        for group in &mut self.groups {
            group.members.retain(|member| member != id);
        }
        self.groups.retain(|group| !group.members.is_empty());
        old_count
            != self
                .groups
                .iter()
                .map(|group| group.members.len())
                .sum::<usize>()
    }
    fn checkpoint(&mut self, mut current: Arrangement<I>) {
        current.groups = self.groups.clone();
        self.undo.push_back(current);
        if self.undo.len() > HISTORY_LIMIT {
            self.undo.pop_front();
        }
        self.redo.clear();
    }
    fn accepted_groups(groups: Vec<Group<I>>, mut live: impl FnMut(&I) -> bool) -> Vec<Group<I>> {
        let mut seen_members = HashSet::new();
        let mut seen_names = HashSet::new();
        groups
            .into_iter()
            .filter_map(|mut group| {
                if group.name.trim().is_empty()
                    || group.name.len() > 128
                    || !seen_names.insert(group.name.clone())
                {
                    return None;
                }
                group
                    .members
                    .retain(|id| live(id) && seen_members.insert(id.clone()));
                (!group.members.is_empty()).then_some(group)
            })
            .take(GROUP_LIMIT)
            .collect()
    }
    pub fn apply(
        &mut self,
        action: ManagementAction<I>,
        mut live: impl FnMut(&I) -> bool,
        allow_history: bool,
    ) -> ManagementOutcome<I> {
        let mut outcome = ManagementOutcome::default();
        match action {
            ManagementAction::Checkpoint(current) => {
                if allow_history && current.valid() {
                    self.checkpoint(current);
                    outcome.changed = true;
                }
            }
            ManagementAction::Undo { redo, mut current } => {
                if !allow_history || !current.valid() {
                    return outcome;
                }
                let selected = if redo {
                    self.redo.pop_back()
                } else {
                    self.undo.pop_back()
                };
                let Some(snapshot) = selected else {
                    return outcome;
                };
                current.groups = self.groups.clone();
                let destination = if redo { &mut self.undo } else { &mut self.redo };
                destination.push_back(current);
                if destination.len() > HISTORY_LIMIT {
                    destination.pop_front();
                }
                self.groups = Self::accepted_groups(snapshot.groups.clone(), &mut live);
                outcome.changed = true;
                outcome.restore = Some(snapshot);
            }
            ManagementAction::SetGroup {
                name,
                members,
                current,
            } => {
                let name = name.trim().to_owned();
                let mut accepted = Self::accepted_groups(vec![Group { name, members }], &mut live);
                let Some(group) = accepted.pop() else {
                    return outcome;
                };
                if (allow_history && !current.valid())
                    || (self.groups.len() >= GROUP_LIMIT && self.group(&group.name).is_none())
                {
                    return outcome;
                }
                if allow_history {
                    self.checkpoint(current);
                }
                let members: HashSet<_> = group.members.iter().cloned().collect();
                self.groups.retain(|old| old.name != group.name);
                for old in &mut self.groups {
                    old.members.retain(|id| !members.contains(id));
                }
                self.groups.retain(|old| !old.members.is_empty());
                self.groups.push(group);
                outcome.changed = true;
            }
            ManagementAction::RemoveGroup { name, current } => {
                if self.group(&name).is_none() || (allow_history && !current.valid()) {
                    return outcome;
                }
                if allow_history {
                    self.checkpoint(current);
                }
                self.groups.retain(|group| group.name != name);
                outcome.changed = true;
            }
            ManagementAction::MoveGroup {
                name,
                target,
                current,
            } => {
                let Some(group) = self.group(&name) else {
                    return outcome;
                };
                let members: Vec<_> = group
                    .members
                    .iter()
                    .filter(|id| live(id))
                    .cloned()
                    .collect();
                if members.is_empty() || (allow_history && !current.valid()) {
                    return outcome;
                }
                if allow_history {
                    self.checkpoint(current);
                }
                outcome.changed = allow_history;
                outcome.move_group = Some(GroupMoveIntent { target, members });
            }
            ManagementAction::FollowGroup { window, target } => {
                if let Some(group) = self
                    .groups
                    .iter()
                    .find(|group| group.members.contains(&window))
                {
                    let members: Vec<_> = group
                        .members
                        .iter()
                        .filter(|id| live(id))
                        .cloned()
                        .collect();
                    if !members.is_empty() {
                        outcome.move_group = Some(GroupMoveIntent { target, members });
                    }
                }
            }
            ManagementAction::ReplaceGroups(groups) => {
                let groups = Self::accepted_groups(groups, &mut live);
                outcome.changed = self.groups != groups;
                self.groups = groups;
            }
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot(id: u64) -> Arrangement<u64> {
        Arrangement {
            spaces: vec![ArrangementSpace {
                id: SpaceId(7),
                name: None,
                index: 0,
                output: None,
                mode: LayoutMode::Scrolling,
                tree: None,
            }],
            windows: vec![ArrangementWindow {
                id,
                app_id: "app".into(),
                title: "title".into(),
                space: SpaceId(7),
                column: Some(0),
                display: ColumnDisplay::Normal,
                sizing: None,
                floating: false,
                size: [300, 200],
                position: None,
            }],
            groups: Vec::new(),
            focus: Some(id),
        }
    }
    #[test]
    fn history_is_bounded_branches_clear_redo_and_closed_ids_do_not_shift_slots() {
        let mut state = ManagementState::default();
        for id in 0..40 {
            state.apply(ManagementAction::Checkpoint(snapshot(id)), |_| true, true);
        }
        assert_eq!(state.history_steps(), (32, 0));
        let result = state.apply(
            ManagementAction::Undo {
                redo: false,
                current: snapshot(100),
            },
            |_| false,
            true,
        );
        assert_eq!(result.restore.unwrap().windows[0].id, 39);
        assert_eq!(state.history_steps(), (31, 1));
        state.apply(ManagementAction::Checkpoint(snapshot(200)), |_| true, true);
        assert_eq!(state.history_steps(), (32, 0));
        assert!(state
            .apply(
                ManagementAction::Undo {
                    redo: true,
                    current: snapshot(200)
                },
                |_| true,
                true
            )
            .restore
            .is_none());
    }
    #[test]
    fn groups_have_unique_live_members_and_one_owner_across_reassignment_and_unmap() {
        let mut state = ManagementState::default();
        state.apply(
            ManagementAction::SetGroup {
                name: " first ".into(),
                members: vec![1, 1, 2, 9],
                current: snapshot(1),
            },
            |id| *id != 9,
            true,
        );
        state.apply(
            ManagementAction::SetGroup {
                name: "second".into(),
                members: vec![2, 3],
                current: snapshot(1),
            },
            |_| true,
            true,
        );
        assert_eq!(state.groups()[0].members, [1]);
        assert_eq!(state.groups()[1].members, [2, 3]);
        assert!(state.remove_window(&2));
        assert_eq!(state.groups()[1].members, [3]);
        let restore = state
            .apply(
                ManagementAction::Undo {
                    redo: false,
                    current: snapshot(1),
                },
                |id| *id != 2,
                true,
            )
            .restore
            .unwrap();
        assert_eq!(restore.groups[0].members, [1, 2]);
        assert_eq!(state.groups()[0].members, [1]);
    }
    #[test]
    fn rejected_actions_are_atomic_and_active_interaction_only_suppresses_history() {
        let mut state = ManagementState::default();
        let mut invalid = snapshot(1);
        invalid.windows.push(invalid.windows[0].clone());
        assert!(
            !state
                .apply(ManagementAction::Checkpoint(invalid), |_| true, true)
                .changed
        );
        state.apply(
            ManagementAction::SetGroup {
                name: "valid".into(),
                members: vec![1],
                current: snapshot(1),
            },
            |_| true,
            false,
        );
        assert_eq!(state.history_steps(), (0, 0));
        assert_eq!(state.group("valid").unwrap().members, [1]);
        assert!(state
            .apply(
                ManagementAction::Undo {
                    redo: false,
                    current: snapshot(1)
                },
                |_| true,
                false
            )
            .restore
            .is_none());
        assert!(
            !state
                .apply(
                    ManagementAction::RemoveGroup {
                        name: "missing".into(),
                        current: snapshot(1)
                    },
                    |_| true,
                    true
                )
                .changed
        );
        assert_eq!(state.history_steps(), (0, 0));
    }
    #[test]
    fn move_and_drag_follow_return_only_live_stable_ids_without_mutating_membership() {
        let mut state = ManagementState::default();
        state.apply(
            ManagementAction::ReplaceGroups(vec![Group {
                name: "pair".into(),
                members: vec![1, 2, 3],
            }]),
            |_| true,
            true,
        );
        let result = state.apply(
            ManagementAction::MoveGroup {
                name: "pair".into(),
                target: SpaceId(9),
                current: snapshot(1),
            },
            |id| *id != 2,
            true,
        );
        assert_eq!(result.move_group.unwrap().members, [1, 3]);
        assert_eq!(state.groups()[0].members, [1, 2, 3]);
        assert_eq!(state.history_steps(), (1, 0));
        assert_eq!(
            state
                .apply(
                    ManagementAction::FollowGroup {
                        window: 1,
                        target: SpaceId(10)
                    },
                    |_| true,
                    true
                )
                .move_group
                .unwrap()
                .target,
            SpaceId(10)
        );
        assert_eq!(state.history_steps(), (1, 0));
    }
}
