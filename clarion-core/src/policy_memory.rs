//! Policy history uses stable IDs and logical preferences, never device handles
//! or animation snapshots. It is captured only when switching layout policy.

use std::collections::HashSet;
use std::hash::Hash;

use crate::bsp::BspSnapshot;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Width {
    Fixed(f64),
    Proportion(f64),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Height {
    Auto(f64),
    Fixed(f64),
    Preset(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnDisplay {
    Normal,
    Tabbed,
    Dwindle,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColumnSnapshot<I> {
    pub id: u64,
    pub windows: Vec<I>,
    pub heights: Vec<Height>,
    pub width: Width,
    pub preset_width: Option<usize>,
    pub full_width: bool,
    pub fullscreen: bool,
    pub maximized: bool,
    pub display: ColumnDisplay,
    pub active: Option<I>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScrollingSnapshot<I> {
    pub columns: Vec<ColumnSnapshot<I>>,
    pub active: Option<I>,
    pub active_column: Option<u64>,
    /// Position relative to the active column, so output size changes reflow
    /// proportional widths without restoring a stale absolute screen offset.
    pub view_offset: f64,
    pub view_offset_to_restore: Option<f64>,
}

impl<I: Clone + Eq + Hash> ScrollingSnapshot<I> {
    /// Keep surviving IDs once, retaining their original column/row metadata.
    /// Unknown current IDs are intentionally left for deterministic insertion
    /// by the adapter after restored columns have been placed.
    pub fn retain(&self, live: &HashSet<I>) -> Self {
        let mut seen = HashSet::new();
        let mut result = self.clone();
        result.columns.retain_mut(|column| {
            let mut windows = Vec::new();
            let mut heights = Vec::new();
            for (index, id) in column.windows.iter().enumerate() {
                if live.contains(id) && seen.insert(id.clone()) {
                    windows.push(id.clone());
                    heights.push(
                        column
                            .heights
                            .get(index)
                            .copied()
                            .unwrap_or(Height::Auto(1.0)),
                    );
                }
            }
            column.active = column
                .active
                .take()
                .filter(|id| windows.contains(id))
                .or_else(|| windows.first().cloned());
            column.windows = windows;
            column.heights = heights;
            !column.windows.is_empty()
        });
        result.active = result.active.take().filter(|id| seen.contains(id));
        result.active_column = result
            .active_column
            .filter(|id| result.columns.iter().any(|column| column.id == *id));
        result
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayoutHistory<I> {
    pub scrolling: Option<ScrollingSnapshot<I>>,
    pub dwindle: Option<BspSnapshot<I>>,
}

impl<I> Default for LayoutHistory<I> {
    fn default() -> Self {
        Self {
            scrolling: None,
            dwindle: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pruning_keeps_row_preferences_and_avoids_duplicate_membership() {
        let snapshot = ScrollingSnapshot {
            columns: vec![ColumnSnapshot {
                id: 7,
                windows: vec![1, 2, 3, 2],
                heights: vec![
                    Height::Fixed(222.0),
                    Height::Auto(2.0),
                    Height::Preset(0),
                    Height::Auto(3.0),
                ],
                width: Width::Proportion(0.7),
                preset_width: Some(1),
                full_width: true,
                fullscreen: false,
                maximized: true,
                display: ColumnDisplay::Tabbed,
                active: Some(3),
            }],
            active: Some(3),
            active_column: Some(7),
            view_offset: -42.0,
            view_offset_to_restore: Some(17.0),
        };
        let result = snapshot.retain(&HashSet::from([1, 2, 4]));
        assert_eq!(result.columns[0].windows, vec![1, 2]);
        assert_eq!(
            result.columns[0].heights,
            vec![Height::Fixed(222.0), Height::Auto(2.0)]
        );
        assert_eq!(result.columns[0].active, Some(1));
        assert_eq!(result.active, None);
        assert_eq!(result.view_offset, -42.0);
        assert_eq!(result.columns[0].width, Width::Proportion(0.7));
        assert_eq!(result.columns[0].display, ColumnDisplay::Tabbed);
    }
}
