//! Workspace interaction and restore choices contain only stable identities and
//! numeric preferences. Client transactions and effect snapshots are payloads
//! resolved by the platform after these decisions have been made.

use crate::layout::LayoutMode;
use crate::policy_memory::{ColumnDisplay, ScrollingSnapshot, Width};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SpaceLayerFocus {
    #[default]
    Tiling,
    /// Keep tiled keyboard focus while allowing floating content above a
    /// fullscreen tile. This is independent of selecting the floating lane.
    TilingWithFloatingRaised,
    Floating,
}

impl SpaceLayerFocus {
    pub fn is_floating(self) -> bool {
        self == Self::Floating
    }
    pub fn floating_above_fullscreen(self) -> bool {
        self != Self::Tiling
    }
}

/// Captured once before canvas moves tiled payloads into the floating policy.
#[derive(Debug, Clone, PartialEq)]
pub enum CanvasPolicyMemory<I> {
    Scrolling(ScrollingSnapshot<I>),
    Dwindle(crate::bsp::BspSnapshot<I>),
}
impl<I: Clone + Eq + std::hash::Hash> CanvasPolicyMemory<I> {
    /// Keep expanded modes changed while canvas was open; the caller supplies
    /// current pure policy metadata, never client handles or GPU state.
    pub fn preserve_runtime(self, current: &Self) -> Self {
        match (self, current) {
            (Self::Scrolling(mut saved), Self::Scrolling(current)) => {
                use std::collections::{HashMap, HashSet};

                use crate::policy_memory::{ColumnSnapshot, Height};
                let current_columns: HashMap<_, _> = current
                    .columns
                    .iter()
                    .map(|column| (column.id, column))
                    .collect();
                let modes: HashMap<_, _> = current
                    .columns
                    .iter()
                    .flat_map(|column| column.windows.iter().map(move |id| (id, column)))
                    .collect();
                let original_ids: HashSet<_> =
                    saved.columns.iter().map(|column| column.id).collect();
                let reserved: HashSet<_> = original_ids
                    .iter()
                    .copied()
                    .chain(current_columns.keys().copied())
                    .collect();
                for id in &reserved {
                    assert!(
                        crate::scrolling::reserve_column_id(*id),
                        "invalid canvas column identity"
                    );
                }
                let mut used = HashSet::new();
                let mut columns = Vec::with_capacity(saved.columns.len());
                for mut column in saved.columns {
                    let mode = |id: &I| {
                        modes.get(id).map_or((false, false), |column| {
                            (column.fullscreen, !column.fullscreen && column.maximized)
                        })
                    };
                    let expanded = column.windows.iter().any(|id| {
                        let value = mode(id);
                        value.0 || value.1
                    });
                    let first_mode = column.windows.first().map_or((false, false), &mode);
                    let mixed = column.windows.iter().any(|id| mode(id) != first_mode);
                    // A Normal/Dwindle multirow column cannot expand. A Tabbed
                    // column can expand only as one whole mode, so mixed live
                    // modes must also remain separate after preference restore.
                    let partition = expanded
                        && column.windows.len() > 1
                        && (column.display != ColumnDisplay::Tabbed || mixed);
                    if !partition {
                        column.fullscreen = first_mode.0;
                        column.maximized = first_mode.1;
                        used.insert(column.id);
                        columns.push(column);
                        continue;
                    }
                    let normal_remainder = column.windows.iter().any(|id| {
                        let value = mode(id);
                        !value.0 && !value.1
                    });
                    let mut buckets: Vec<(Option<u64>, ColumnSnapshot<I>)> = Vec::new();
                    let mut indices = HashMap::new();
                    for (row, id) in column.windows.iter().enumerate() {
                        let live = modes
                            .get(id)
                            .copied()
                            .filter(|column| column.fullscreen || column.maximized);
                        let key = live.map(|column| column.id);
                        let index = *indices.entry(key).or_insert_with(|| {
                            let candidate = key.unwrap_or(column.id);
                            // Reusing another saved column's identity would
                            // redirect parked restore hints to the wrong row.
                            let mut identity = candidate;
                            if used.contains(&identity)
                                || (identity != column.id && original_ids.contains(&identity))
                                || (key.is_some() && normal_remainder && identity == column.id)
                            {
                                loop {
                                    identity = crate::scrolling::allocate_column_id();
                                    if !reserved.contains(&identity) && !used.contains(&identity) {
                                        break;
                                    }
                                }
                            }
                            used.insert(identity);
                            buckets.push((
                                key,
                                ColumnSnapshot {
                                    id: identity,
                                    windows: Vec::new(),
                                    heights: Vec::new(),
                                    width: column.width,
                                    preset_width: column.preset_width,
                                    full_width: column.full_width,
                                    fullscreen: live.is_some_and(|column| column.fullscreen),
                                    maximized: live.is_some_and(|column| {
                                        !column.fullscreen && column.maximized
                                    }),
                                    display: column.display,
                                    active: None,
                                },
                            ));
                            buckets.len() - 1
                        });
                        buckets[index].1.windows.push(id.clone());
                        buckets[index]
                            .1
                            .heights
                            .push(column.heights.get(row).copied().unwrap_or(Height::Auto(1.)));
                    }
                    for (key, mut bucket) in buckets {
                        let current = key.and_then(|id| current_columns.get(&id).copied());
                        if bucket.windows.len() > 1 {
                            if let Some(current) = current {
                                bucket.display = current.display;
                            }
                        }
                        bucket.active = column
                            .active
                            .as_ref()
                            .filter(|id| bucket.windows.contains(id))
                            .cloned()
                            .or_else(|| {
                                current
                                    .and_then(|column| column.active.as_ref())
                                    .filter(|id| bucket.windows.contains(id))
                                    .cloned()
                            })
                            .or_else(|| bucket.windows.first().cloned());
                        columns.push(bucket);
                    }
                }
                saved.columns = columns;
                saved.active_column = saved
                    .active
                    .as_ref()
                    .and_then(|id| {
                        saved
                            .columns
                            .iter()
                            .find(|column| column.windows.contains(id))
                    })
                    .map(|column| column.id);
                Self::Scrolling(saved)
            }
            (Self::Dwindle(mut saved), Self::Dwindle(current)) => {
                saved.expanded = current.expanded.clone();
                saved.active = current.active.clone();
                Self::Dwindle(saved)
            }
            (saved, _) => saved,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpaceInteraction<I> {
    layer: SpaceLayerFocus,
    canvas_enabled: bool,
    canvas_origins: Vec<I>,
    canvas_policy: Option<CanvasPolicyMemory<I>>,
}

impl<I> Default for SpaceInteraction<I> {
    fn default() -> Self {
        Self {
            layer: SpaceLayerFocus::Tiling,
            canvas_enabled: false,
            canvas_origins: Vec::new(),
            canvas_policy: None,
        }
    }
}

impl<I: Clone + Eq> SpaceInteraction<I> {
    pub fn layer_focus(&self) -> SpaceLayerFocus {
        self.layer
    }
    pub fn canvas_enabled(&self) -> bool {
        self.canvas_enabled
    }
    pub fn canvas_origins(&self) -> &[I] {
        &self.canvas_origins
    }
    pub fn canvas_policy(&self) -> Option<&CanvasPolicyMemory<I>> {
        self.canvas_policy.as_ref()
    }
    pub fn capture_canvas_policy(&mut self, policy: CanvasPolicyMemory<I>) -> bool {
        if self.canvas_enabled {
            return false;
        }
        self.canvas_policy = Some(policy);
        true
    }

    pub fn select_layer(
        &mut self,
        requested: SpaceLayerFocus,
        tiled: bool,
        floating: bool,
    ) -> bool {
        let selected = if !floating {
            SpaceLayerFocus::Tiling
        } else if !tiled {
            SpaceLayerFocus::Floating
        } else {
            requested
        };
        if self.layer == selected {
            return false;
        }
        self.layer = selected;
        true
    }
    pub fn repair_layer(&mut self, tiled: bool, floating: bool) -> bool {
        self.select_layer(self.layer, tiled, floating)
    }
    pub fn set_canvas(&mut self, enabled: bool) -> CanvasTransition<I> {
        if self.canvas_enabled == enabled {
            return CanvasTransition {
                changed: false,
                enabled,
                restore_tiled: Vec::new(),
                restore_policy: None,
            };
        }
        self.canvas_enabled = enabled;
        let restore_tiled = if enabled {
            Vec::new()
        } else {
            std::mem::take(&mut self.canvas_origins)
        };
        CanvasTransition {
            changed: true,
            enabled,
            restore_tiled,
            restore_policy: if enabled {
                None
            } else {
                self.canvas_policy.take()
            },
        }
    }
    pub fn set_canvas_origin(&mut self, id: I, tiled: bool) {
        if tiled && self.canvas_enabled {
            if !self.canvas_origins.contains(&id) {
                self.canvas_origins.push(id);
            }
        } else {
            self.canvas_origins.retain(|old| old != &id);
        }
    }
    pub fn take_canvas_origin(&mut self, id: &I) -> bool {
        let Some(index) = self.canvas_origins.iter().position(|old| old == id) else {
            return false;
        };
        self.canvas_origins.remove(index);
        true
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CanvasTransition<I> {
    pub changed: bool,
    pub enabled: bool,
    /// Pure stable-ID operations to restore former tiling members when leaving
    /// canvas mode. The platform resolves these IDs without choosing members.
    pub restore_tiled: Vec<I>,
    /// Moved once on exit. Minimized/Stage captures already own their choices.
    pub restore_policy: Option<CanvasPolicyMemory<I>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScrollingRestore<I> {
    pub column_id: u64,
    pub column_index: usize,
    pub row_index: usize,
    pub sibling: Option<I>,
}

/// Numeric current policy topology, supplied only for a restore command. This
/// is not a per-frame reconstruction of desktop or protocol state.
#[derive(Debug, Clone, PartialEq)]
pub struct RestoreColumn<I> {
    pub id: u64,
    pub windows: Vec<I>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MinimizeRestore<I> {
    pub was_floating: bool,
    pub width: Width,
    pub full_width: bool,
    pub display: Option<ColumnDisplay>,
    pub neighbor: Option<I>,
    pub scrolling: Option<ScrollingRestore<I>>,
    /// Filled by core from its canvas membership when minimizing, rather than
    /// supplied by a platform payload that could retain an independent truth.
    pub canvas_origin: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RestoreDestination<I> {
    Floating,
    Scrolling {
        column: Option<u64>,
        column_index: usize,
        row_index: usize,
    },
    Tiled {
        neighbor: Option<I>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct RestorePlacement<I> {
    pub destination: RestoreDestination<I>,
    pub width: Width,
    pub full_width: bool,
    pub display: Option<ColumnDisplay>,
    pub canvas_origin: bool,
}

impl<I> RestorePlacement<I> {
    pub fn is_floating(&self) -> bool {
        matches!(self.destination, RestoreDestination::Floating)
    }
}

impl<I: Clone> MinimizeRestore<I> {
    /// Capture placement preferences from the live pure-policy topology. The
    /// platform never selects a sibling by scanning protocol window handles.
    pub fn capture(
        id: &I,
        was_floating: bool,
        floating_width: f64,
        columns: &[crate::policy_memory::ColumnSnapshot<I>],
        topology: Option<&crate::layout::BspNode<I>>,
    ) -> Self
    where
        I: Eq,
    {
        let column = columns.iter().enumerate().find_map(|(index, column)| {
            column
                .windows
                .iter()
                .position(|window| window == id)
                .map(|row| (index, row, column))
        });
        let mut stack = topology.into_iter().collect::<Vec<_>>();
        let mut neighbor = None;
        while let Some(node) = stack.pop() {
            match node {
                crate::layout::BspNode::Leaf(window) if window != id => {
                    neighbor = Some(window.clone());
                    break;
                }
                crate::layout::BspNode::Leaf(_) => {}
                crate::layout::BspNode::Split { first, second, .. } => {
                    stack.push(second);
                    stack.push(first);
                }
            }
        }
        let (width, full_width, scrolling) = if let Some((index, row, column)) = column {
            (
                column.width,
                column.full_width,
                Some(ScrollingRestore {
                    column_id: column.id,
                    column_index: index,
                    row_index: row,
                    sibling: column.windows.iter().find(|window| *window != id).cloned(),
                }),
            )
        } else {
            (
                if was_floating {
                    Width::Fixed(crate::geometry::finite(floating_width, 1.).max(1.))
                } else {
                    Width::Proportion(1.)
                },
                false,
                None,
            )
        };
        Self {
            was_floating,
            width,
            full_width,
            display: column.map(|(_, _, column)| column.display),
            scrolling,
            neighbor,
            canvas_origin: false,
        }
    }
    pub fn resolve(
        &self,
        mode: LayoutMode,
        canvas: bool,
        columns: &[RestoreColumn<I>],
        mut eligible: impl FnMut(&I) -> bool,
    ) -> RestorePlacement<I>
    where
        I: Eq,
    {
        // Canvas temporarily transports former tiled members through the
        // floating policy. Preserve genuine floating identity, but return a
        // parked former tiled member to the current tiling policy after exit.
        let destination = if self.was_floating && (canvas || !self.canvas_origin) {
            RestoreDestination::Floating
        } else if mode == LayoutMode::Scrolling && !canvas {
            if let Some(origin) = &self.scrolling {
                let sibling = origin.sibling.as_ref().filter(|id| eligible(id));
                let column = sibling
                    .and_then(|id| columns.iter().find(|column| column.windows.contains(id)))
                    .or_else(|| columns.iter().find(|column| column.id == origin.column_id))
                    .map(|column| column.id);
                RestoreDestination::Scrolling {
                    column,
                    column_index: origin.column_index,
                    row_index: origin.row_index,
                }
            } else {
                RestoreDestination::Tiled { neighbor: None }
            }
        } else {
            RestoreDestination::Tiled {
                neighbor: if mode == LayoutMode::Dwindle {
                    self.neighbor.clone().filter(&mut eligible)
                } else {
                    None
                },
            }
        };
        let display = if mode == LayoutMode::Scrolling
            && matches!(
                destination,
                RestoreDestination::Scrolling { column: None, .. }
                    | RestoreDestination::Tiled { .. }
            ) {
            self.display
        } else {
            None
        };
        RestorePlacement {
            destination,
            width: self.width,
            full_width: self.full_width,
            display,
            canvas_origin: self.canvas_origin && canvas,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MinimizeIntent<I> {
    pub id: I,
    pub minimized: bool,
    pub restore: Option<RestorePlacement<I>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn raised_floating_content_never_selects_the_floating_keyboard_lane() {
        let mut state = SpaceInteraction::<u64>::default();
        assert!(state.select_layer(SpaceLayerFocus::TilingWithFloatingRaised, true, true));
        assert!(!state.layer_focus().is_floating());
        assert!(state.layer_focus().floating_above_fullscreen());
        assert!(!state.repair_layer(true, true));
        assert!(state.repair_layer(false, true));
        assert!(state.layer_focus().is_floating());
        assert!(state.repair_layer(false, false));
        assert_eq!(state.layer_focus(), SpaceLayerFocus::Tiling);
    }
    #[test]
    fn canvas_exit_returns_original_stable_members_once_in_order() {
        let mut state = SpaceInteraction::default();
        state.set_canvas_origin(9, true);
        assert!(state.canvas_origins().is_empty());
        state.set_canvas(true);
        for id in [9, 2, 9, 7] {
            state.set_canvas_origin(id, true);
        }
        assert!(state.take_canvas_origin(&2));
        assert!(!state.take_canvas_origin(&2));
        assert_eq!(state.set_canvas(false).restore_tiled, [9, 7]);
        assert!(!state.set_canvas(false).changed);
        assert!(state.canvas_origins().is_empty());
    }
    #[test]
    fn restore_resolution_rejects_closed_siblings_and_honors_current_mode() {
        let restore = MinimizeRestore {
            was_floating: false,
            width: Width::Proportion(0.4),
            full_width: true,
            display: None,
            neighbor: Some(8),
            scrolling: Some(ScrollingRestore {
                column_id: 31,
                column_index: 3,
                row_index: 2,
                sibling: Some(5),
            }),
            canvas_origin: true,
        };
        let result = restore.resolve(LayoutMode::Scrolling, false, &[], |_| false);
        assert_eq!(
            result.destination,
            RestoreDestination::Scrolling {
                column: None,
                column_index: 3,
                row_index: 2
            }
        );
        assert!(!result.canvas_origin);
        assert!(result.full_width);
        assert_eq!(
            restore
                .resolve(LayoutMode::Dwindle, false, &[], |id| *id == 8)
                .destination,
            RestoreDestination::Tiled { neighbor: Some(8) }
        );
        assert_eq!(
            restore
                .resolve(LayoutMode::Floating, true, &[], |_| true)
                .destination,
            RestoreDestination::Tiled { neighbor: None }
        );
    }
    #[test]
    fn restore_prefers_live_sibling_then_stable_column_instead_of_shifted_slot() {
        let restore = MinimizeRestore {
            was_floating: false,
            width: Width::Fixed(500.),
            full_width: false,
            display: None,
            neighbor: None,
            scrolling: Some(ScrollingRestore {
                column_id: 31,
                column_index: 3,
                row_index: 2,
                sibling: Some(5),
            }),
            canvas_origin: false,
        };
        let columns = [
            RestoreColumn {
                id: 7,
                windows: vec![5],
            },
            RestoreColumn {
                id: 31,
                windows: vec![9],
            },
        ];
        let resolve = |live| {
            restore.resolve(LayoutMode::Scrolling, false, &columns, move |id| {
                *id == live
            })
        };
        assert!(matches!(
            resolve(5).destination,
            RestoreDestination::Scrolling {
                column: Some(7),
                ..
            }
        ));
        assert!(matches!(
            resolve(9).destination,
            RestoreDestination::Scrolling {
                column: Some(31),
                ..
            }
        ));
    }
}
