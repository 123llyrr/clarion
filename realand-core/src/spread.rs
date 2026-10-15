//! Stable-identity window overview interaction. The adapter supplies logical
//! tile order and optional owned metadata at mutation edges; rendering only
//! reads the resulting search membership and selection.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpreadCandidate<I> {
    pub id: I,
    /// None preserves previously supplied metadata. Empty-query mutation edges
    /// need no title/app allocation; a search key supplies fresh text first.
    pub search_text: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpreadKey {
    Left,
    Right,
    Up,
    Down,
    Next,
    Backspace,
    Character(char),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpreadAction<I> {
    Begin {
        initial: Option<I>,
    },
    Candidates {
        connector: String,
        windows: Vec<SpreadCandidate<I>>,
    },
    /// Update one existing candidate at a client metadata mutation edge. This
    /// cannot register a window or output, or change the logical tile order.
    MetadataChanged {
        connector: String,
        id: I,
        search_text: Option<String>,
    },
    Reset {
        connector: String,
        preferred: Option<I>,
    },
    Key {
        connector: String,
        key: SpreadKey,
        columns: usize,
    },
    WindowRemoved(I),
    OutputRemoved(String),
    Close,
}

#[derive(Debug, Clone, Default)]
struct CandidateText {
    text: String,
}

#[derive(Debug, Clone)]
pub struct SpreadOutput<I> {
    query: String,
    words: Vec<String>,
    order: Vec<I>,
    metadata: HashMap<I, CandidateText>,
    matching: Vec<I>,
    membership: HashSet<I>,
    selected: Option<I>,
}

impl<I: Clone + Eq + Hash> Default for SpreadOutput<I> {
    fn default() -> Self {
        Self {
            query: String::new(),
            words: vec![],
            order: vec![],
            metadata: HashMap::new(),
            matching: vec![],
            membership: HashSet::new(),
            selected: None,
        }
    }
}

impl<I: Clone + Eq + Hash> SpreadOutput<I> {
    pub fn query(&self) -> &str {
        &self.query
    }
    pub fn order(&self) -> &[I] {
        &self.order
    }
    pub fn matching(&self) -> &[I] {
        &self.matching
    }
    pub fn selected(&self) -> Option<&I> {
        self.selected.as_ref()
    }
    pub fn matches(&self, id: &I) -> bool {
        // Unfiltered frames avoid a linear result lookup as well as metadata.
        self.words.is_empty() || self.membership.contains(id)
    }
    fn rebuild(&mut self, reset: bool) {
        let previous_index = self
            .selected
            .as_ref()
            .and_then(|id| self.matching.iter().position(|v| v == id))
            .unwrap_or(0);
        self.matching.clear();
        self.matching.extend(
            self.order
                .iter()
                .filter(|id| {
                    self.words.is_empty()
                        || self.metadata.get(*id).is_some_and(|value| {
                            self.words.iter().all(|word| value.text.contains(word))
                        })
                })
                .cloned(),
        );
        self.membership.clear();
        self.membership.extend(self.matching.iter().cloned());
        if reset
            || !self
                .selected
                .as_ref()
                .is_some_and(|id| self.matching.contains(id))
        {
            let index = if reset {
                0
            } else {
                previous_index.min(self.matching.len().saturating_sub(1))
            };
            self.selected = self.matching.get(index).cloned();
        }
    }
    fn candidates(&mut self, candidates: Vec<SpreadCandidate<I>>) -> bool {
        let mut changed = self.order.len() != candidates.len();
        if !changed {
            changed = self
                .order
                .iter()
                .zip(&candidates)
                .any(|(id, item)| id != &item.id);
        }
        for item in &candidates {
            if let Some(text) = &item.search_text {
                let text = text.to_lowercase();
                let stored = self.metadata.entry(item.id.clone()).or_default();
                if stored.text != text {
                    stored.text = text;
                    changed = true;
                }
            }
        }
        if !changed {
            return false;
        }
        self.order.clear();
        for item in candidates {
            if !self.order.contains(&item.id) {
                self.order.push(item.id);
            }
        }
        self.metadata.retain(|id, _| self.order.contains(id));
        self.rebuild(false);
        true
    }
    fn key(&mut self, key: SpreadKey, columns: usize) -> bool {
        let columns = columns.max(1);
        let old_query = self.query.clone();
        let old_selection = self.selected.clone();
        let count = self.matching.len();
        let index = self
            .selected
            .as_ref()
            .and_then(|id| self.matching.iter().position(|v| v == id))
            .unwrap_or(0);
        let target = match key {
            SpreadKey::Left if !index.is_multiple_of(columns) => index - 1,
            SpreadKey::Right if index % columns + 1 < columns && index + 1 < count => index + 1,
            SpreadKey::Up if index >= columns => index - columns,
            SpreadKey::Down if index + columns < count => index + columns,
            SpreadKey::Next => (index + 1).min(count.saturating_sub(1)),
            SpreadKey::Backspace => {
                self.query.pop();
                index
            }
            SpreadKey::Character(ch) if !ch.is_control() && self.query.len() < 512 => {
                self.query.push(ch);
                index
            }
            _ => index,
        };
        if self.query != old_query {
            self.words = self
                .query
                .split_whitespace()
                .map(str::to_lowercase)
                .collect();
            self.rebuild(true);
        } else {
            self.selected = self.matching.get(target).cloned();
        }
        self.query != old_query || self.selected != old_selection
    }
    fn metadata_changed(&mut self, id: I, search_text: Option<String>) -> bool {
        if !self.order.contains(&id) {
            return false;
        }
        let Some(text) = search_text else {
            return false;
        };
        let text = text.to_lowercase();
        let stored = self.metadata.entry(id).or_default();
        if stored.text == text {
            return false;
        }
        stored.text = text;
        self.rebuild(false);
        true
    }
}

#[derive(Debug, Clone)]
pub struct SpreadState<I> {
    initial: Option<I>,
    outputs: HashMap<String, SpreadOutput<I>>,
}
impl<I: Clone + Eq + Hash> Default for SpreadState<I> {
    fn default() -> Self {
        Self {
            initial: None,
            outputs: HashMap::new(),
        }
    }
}
impl<I: Clone + Eq + Hash> SpreadState<I> {
    pub fn initial(&self) -> Option<&I> {
        self.initial.as_ref()
    }
    pub fn output(&self, connector: &str) -> Option<&SpreadOutput<I>> {
        self.outputs.get(connector)
    }
    pub fn retain_candidates(&mut self, mut contains: impl FnMut(&str, &I) -> bool) -> bool {
        let mut changed = false;
        for (connector, output) in &mut self.outputs {
            let len = output.order.len();
            output.order.retain(|id| contains(connector, id));
            if len != output.order.len() {
                output.metadata.retain(|id, _| output.order.contains(id));
                output.rebuild(false);
                changed = true;
            }
        }
        changed
    }
    pub fn apply(&mut self, action: SpreadAction<I>) -> bool {
        match action {
            SpreadAction::Begin { initial } => {
                let changed = self.initial != initial || !self.outputs.is_empty();
                self.initial = initial;
                self.outputs.clear();
                changed
            }
            SpreadAction::Candidates { connector, windows } => self
                .outputs
                .entry(connector)
                .or_default()
                .candidates(windows),
            SpreadAction::MetadataChanged {
                connector,
                id,
                search_text,
            } => self
                .outputs
                .get_mut(&connector)
                .is_some_and(|output| output.metadata_changed(id, search_text)),
            SpreadAction::Reset {
                connector,
                preferred,
            } => {
                let output = self.outputs.entry(connector).or_default();
                let changed = !output.query.is_empty() || output.selected != preferred;
                output.query.clear();
                output.words.clear();
                output.selected = preferred;
                output.rebuild(false);
                changed
            }
            SpreadAction::Key {
                connector,
                key,
                columns,
            } => self
                .outputs
                .get_mut(&connector)
                .is_some_and(|output| output.key(key, columns)),
            SpreadAction::WindowRemoved(id) => {
                let mut changed = false;
                if self.initial.as_ref() == Some(&id) {
                    self.initial = None;
                    changed = true;
                }
                changed | self.retain_candidates(|_, candidate| candidate != &id)
            }
            SpreadAction::OutputRemoved(connector) => self.outputs.remove(&connector).is_some(),
            SpreadAction::Close => {
                let changed = self.initial.take().is_some() || !self.outputs.is_empty();
                self.outputs.clear();
                changed
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn candidates(values: &[(u64, &str)]) -> SpreadAction<u64> {
        SpreadAction::Candidates {
            connector: "DP-1".into(),
            windows: values
                .iter()
                .map(|(id, text)| SpreadCandidate {
                    id: *id,
                    search_text: Some((*text).into()),
                })
                .collect(),
        }
    }
    fn key(state: &mut SpreadState<u64>, value: SpreadKey, columns: usize) {
        state.apply(SpreadAction::Key {
            connector: "DP-1".into(),
            key: value,
            columns,
        });
    }
    #[test]
    fn search_is_case_insensitive_and_all_words_must_match() {
        let mut state = SpreadState::default();
        state.apply(candidates(&[
            (1, "Firefox Rust Manual"),
            (2, "Firefox Home"),
            (3, "Terminal Rust Manual"),
        ]));
        for ch in "FIREFOX rust".chars() {
            key(&mut state, SpreadKey::Character(ch), 2);
        }
        assert_eq!(state.output("DP-1").unwrap().matching(), &[1]);
        key(&mut state, SpreadKey::Backspace, 2);
        assert_eq!(state.output("DP-1").unwrap().query(), "FIREFOX rus");
    }
    #[test]
    fn stable_selection_survives_nonselected_removal_and_reorder() {
        let mut state = SpreadState::default();
        state.apply(candidates(&[(1, ""), (2, ""), (3, "")]));
        state.apply(SpreadAction::Reset {
            connector: "DP-1".into(),
            preferred: Some(2),
        });
        state.apply(candidates(&[(3, ""), (2, "")]));
        assert_eq!(state.output("DP-1").unwrap().selected(), Some(&2));
        state.apply(SpreadAction::WindowRemoved(2));
        assert_eq!(state.output("DP-1").unwrap().selected(), Some(&3));
    }
    #[test]
    fn directional_navigation_respects_grid_edges_and_empty_results() {
        let mut state = SpreadState::default();
        state.apply(candidates(&[(1, ""), (2, ""), (3, ""), (4, ""), (5, "")]));
        key(&mut state, SpreadKey::Right, 3);
        key(&mut state, SpreadKey::Down, 3);
        assert_eq!(state.output("DP-1").unwrap().selected(), Some(&5));
        key(&mut state, SpreadKey::Right, 3);
        assert_eq!(state.output("DP-1").unwrap().selected(), Some(&5));
        key(&mut state, SpreadKey::Character('x'), 3);
        assert_eq!(state.output("DP-1").unwrap().selected(), None);
    }
    #[test]
    fn transfer_prunes_output_selection_but_preserves_global_cancel_origin() {
        let mut state = SpreadState::default();
        state.apply(SpreadAction::Begin { initial: Some(2) });
        state.apply(candidates(&[(1, ""), (2, "")]));
        state.apply(SpreadAction::Reset {
            connector: "DP-1".into(),
            preferred: Some(2),
        });
        state.retain_candidates(|connector, id| connector != "DP-1" || *id != 2);
        assert_eq!(state.output("DP-1").unwrap().selected(), Some(&1));
        assert_eq!(state.initial(), Some(&2));
        state.apply(SpreadAction::WindowRemoved(2));
        assert_eq!(state.initial(), None);
    }
    #[test]
    fn output_disconnect_and_close_release_metadata_and_reopening_resets_query() {
        let mut state = SpreadState::default();
        state.apply(candidates(&[(1, "Editor")]));
        key(&mut state, SpreadKey::Character('e'), 1);
        state.apply(SpreadAction::Reset {
            connector: "DP-1".into(),
            preferred: Some(1),
        });
        assert_eq!(state.output("DP-1").unwrap().query(), "");
        state.apply(SpreadAction::OutputRemoved("DP-1".into()));
        assert!(state.output("DP-1").is_none());
        state.apply(SpreadAction::Close);
        assert!(state.outputs.is_empty());
    }
    #[test]
    fn metadata_acknowledgement_updates_filtered_membership_and_none_retains_text() {
        let mut state = SpreadState::default();
        state.apply(candidates(&[(1, "Editor"), (2, "Terminal")]));
        key(&mut state, SpreadKey::Character('e'), 2);
        state.apply(SpreadAction::Candidates {
            connector: "DP-1".into(),
            windows: vec![
                SpreadCandidate {
                    id: 2,
                    search_text: None,
                },
                SpreadCandidate {
                    id: 1,
                    search_text: None,
                },
            ],
        });
        assert_eq!(state.output("DP-1").unwrap().matching(), &[2, 1]);
        state.apply(candidates(&[(2, "TTY"), (1, "Editor")]));
        assert_eq!(state.output("DP-1").unwrap().matching(), &[1]);
    }

    #[test]
    fn individual_metadata_refresh_preserves_order_selection_and_replacement_ordinal() {
        let mut state = SpreadState::default();
        state.apply(candidates(&[
            (1, "Editor A"),
            (2, "Editor B"),
            (3, "Editor C"),
        ]));
        for ch in "editor".chars() {
            key(&mut state, SpreadKey::Character(ch), 3);
        }
        key(&mut state, SpreadKey::Right, 3);
        let metadata = |state: &mut SpreadState<u64>, id, text: &str| {
            state.apply(SpreadAction::MetadataChanged {
                connector: "DP-1".into(),
                id,
                search_text: Some(text.into()),
            })
        };
        assert!(metadata(&mut state, 1, "Terminal A"));
        assert_eq!(state.output("DP-1").unwrap().matching(), &[2, 3]);
        assert_eq!(state.output("DP-1").unwrap().selected(), Some(&2));
        assert!(metadata(&mut state, 2, "Terminal B"));
        assert_eq!(state.output("DP-1").unwrap().matching(), &[3]);
        assert_eq!(state.output("DP-1").unwrap().selected(), Some(&3));
        assert!(metadata(&mut state, 1, "Editor A"));
        assert_eq!(state.output("DP-1").unwrap().order(), &[1, 2, 3]);
        assert_eq!(state.output("DP-1").unwrap().matching(), &[1, 3]);
        assert_eq!(state.output("DP-1").unwrap().selected(), Some(&3));
        assert!(!metadata(&mut state, 1, "EDITOR A"));
    }

    #[test]
    fn metadata_refresh_never_creates_output_or_candidate_and_none_retains_cache() {
        let mut state = SpreadState::default();
        state.apply(candidates(&[(1, "Editor")]));
        key(&mut state, SpreadKey::Character('e'), 1);
        for (connector, id, search_text) in [
            ("UNKNOWN", 1, Some("Terminal".into())),
            ("DP-1", 99, Some("Editor".into())),
            ("DP-1", 1, None),
        ] {
            assert!(!state.apply(SpreadAction::MetadataChanged {
                connector: connector.into(),
                id,
                search_text,
            }));
        }
        assert!(state.output("UNKNOWN").is_none());
        assert_eq!(state.output("DP-1").unwrap().order(), &[1]);
        assert_eq!(state.output("DP-1").unwrap().matching(), &[1]);
        assert_eq!(state.output("DP-1").unwrap().metadata.len(), 1);
    }
}
