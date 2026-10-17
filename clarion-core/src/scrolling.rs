//! Persistent scrolling-layout policy, independent of physical tile payloads.
//!
//! Adapters apply returned index changes to their device handles and submit
//! configure requests. Identity, membership, focus and preferences live here.

use std::hash::Hash;

use crate::column::{HeightConstraint, HeightScratch};
use crate::geometry::Rect;
use crate::layout::LiveLayoutPolicy;
pub use crate::policy_memory::{ColumnDisplay, Width as ColumnWidth};

static COLUMN_ID_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn allocate_from(counter: &std::sync::atomic::AtomicU64) -> Option<u64> {
    use std::sync::atomic::Ordering;
    counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .ok()
}
fn reserve_in(counter: &std::sync::atomic::AtomicU64, id: u64) -> bool {
    use std::sync::atomic::Ordering;
    let Some(next) = id.checked_add(1) else {
        return false;
    };
    counter.fetch_max(next, Ordering::Relaxed);
    true
}
/// One allocator shared by live columns and restored pure column topology.
/// Overflow fails before an existing numeric identity can be reused.
pub fn allocate_column_id() -> u64 {
    allocate_from(&COLUMN_ID_COUNTER).expect("column identity exhausted")
}
/// Reserve adopted identities before subsequent live allocations.
pub fn reserve_column_id(id: u64) -> bool {
    reserve_in(&COLUMN_ID_COUNTER, id)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WindowHeight {
    Auto { weight: f64 },
    Fixed(f64),
    Preset(usize),
}

#[derive(Debug, Clone, Copy)]
pub enum ColumnArrangement {
    /// Fullscreen/maximized output bounds are resolved by the platform adapter.
    Expanded(Rect),
    Normal {
        width: f64,
        working_height: f64,
        gap: f64,
        tabbed_limit: f64,
    },
}

impl WindowHeight {
    pub const fn auto_1() -> Self {
        Self::Auto { weight: 1. }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row<I> {
    id: I,
    pub height: WindowHeight,
}

impl<I> Row<I> {
    pub fn id(&self) -> &I {
        &self.id
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Removal {
    pub old_active: usize,
    pub new_active: usize,
    pub removed_active: bool,
}

/// Numeric user intent, after the platform resolves IPC integer quantities.
#[derive(Debug, Clone, Copy)]
pub enum DimensionChange {
    SetFixed(f64),
    SetProportion(f64),
    AdjustFixed(f64),
    AdjustProportion(f64),
}

#[derive(Debug, Clone, Copy)]
pub enum PresetExtent {
    Tile(f64),
    Window(f64),
}

#[derive(Debug, Clone, Copy)]
pub struct WidthContext {
    pub working_width: f64,
    pub gap: f64,
    pub extra_width: f64,
    pub decoration: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct HeightContext {
    pub working_height: f64,
    pub gap: f64,
    pub extra_height: f64,
    pub decoration: f64,
    pub current_window_height: f64,
    pub other_minimum_tile_height: f64,
    pub minimum_window_height: f64,
    pub maximum_window_height: f64,
}

pub fn resolve_column_width(width: ColumnWidth, working_width: f64, gap: f64, extra: f64) -> f64 {
    match width {
        ColumnWidth::Proportion(proportion) => (working_width - gap) * proportion - gap - extra,
        ColumnWidth::Fixed(width) => width,
    }
}

/// Column ordering and focus survive physical payload moves and mode snapshots.
#[derive(Debug, Default)]
pub struct ColumnOrder {
    ids: Vec<u64>,
    active: usize,
    pub activate_previous_on_removal: Option<f64>,
    pub view_offset_to_restore: Option<f64>,
    pub resize: Option<crate::resize::Resize>,
}

impl ColumnOrder {
    pub fn ids(&self) -> &[u64] {
        &self.ids
    }
    pub fn active_index(&self) -> usize {
        self.active
    }
    pub fn active_id(&self) -> Option<u64> {
        self.ids.get(self.active).copied()
    }
    pub fn insertion_index(&self, requested: Option<usize>) -> usize {
        requested
            .unwrap_or_else(|| {
                if self.ids.is_empty() {
                    0
                } else {
                    self.active + 1
                }
            })
            .min(self.ids.len())
    }
    pub fn adjacent(&self, forward: bool) -> Option<usize> {
        if self.ids.is_empty() {
            return None;
        }
        if forward {
            (self.active + 1 < self.ids.len()).then_some(self.active + 1)
        } else {
            self.active.checked_sub(1)
        }
    }
    pub fn one_based_index(&self, index: usize) -> Option<usize> {
        (!self.ids.is_empty()).then(|| index.saturating_sub(1).min(self.ids.len() - 1))
    }
    pub fn activate(&mut self, index: usize) -> bool {
        self.activate_from(index, self.active)
    }
    /// Complete a physical view transition after the pure order already moved.
    pub fn activate_from(&mut self, index: usize, previous: usize) -> bool {
        if self.ids.is_empty() {
            self.active = 0;
            return false;
        }
        let index = index.min(self.ids.len() - 1);
        if previous == index {
            self.active = index;
            return false;
        }
        self.active = index;
        self.activate_previous_on_removal = None;
        self.view_offset_to_restore = None;
        true
    }
    pub fn insert(&mut self, index: usize, id: u64) -> bool {
        if index > self.ids.len() || self.ids.contains(&id) {
            return false;
        }
        if !self.ids.is_empty() && index <= self.active {
            self.active += 1;
        }
        self.ids.insert(index, id);
        true
    }
    pub fn remove(&mut self, index: usize) -> Removal {
        let old_active = self.active;
        let removed_active = index == old_active;
        let previous_removed = index + 1 == old_active;
        self.ids.remove(index);
        if removed_active {
            self.view_offset_to_restore = None;
        }
        self.active = if self.ids.is_empty() {
            0
        } else if index < old_active {
            old_active - 1
        } else if removed_active && self.activate_previous_on_removal.is_some() && index > 0 {
            index - 1
        } else {
            old_active.min(self.ids.len() - 1)
        };
        if previous_removed || index < old_active {
            self.activate_previous_on_removal = None;
        }
        Removal {
            old_active,
            new_active: self.active,
            removed_active,
        }
    }
    pub fn move_active_to(&mut self, index: usize) -> Option<(usize, usize)> {
        if self.ids.is_empty() {
            return None;
        }
        let index = index.min(self.ids.len() - 1);
        let previous = self.active;
        if previous == index {
            return None;
        }
        let id = self.ids.remove(previous);
        self.ids.insert(index, id);
        self.active = index;
        self.activate_previous_on_removal = None;
        self.view_offset_to_restore = None;
        Some((previous, index))
    }
    pub fn clear(&mut self) {
        self.ids.clear();
        self.active = 0;
        self.activate_previous_on_removal = None;
        self.view_offset_to_restore = None;
        self.resize = None;
    }
}

impl ColumnPolicy<u64> {
    /// Convert a gesture delta into sizes, with the scrolling top-row rule.
    pub fn resize_intent(
        &self,
        resize: crate::resize::Resize,
        delta: [f64; 2],
        centered: bool,
    ) -> Option<crate::resize::ResizeIntent> {
        let index = self.position(&resize.id)?;
        if !delta.iter().all(|delta| delta.is_finite()) {
            return None;
        }
        let width = resize
            .width_delta(delta[0], centered)
            .map(|delta| (resize.original_size.width + delta).round() as i32);
        let height = if resize.edges & 1 != 0 && index == 0 {
            None
        } else {
            resize
                .height_delta(delta[1])
                .map(|delta| (resize.original_size.height + delta).round() as i32)
        };
        Some(crate::resize::ResizeIntent { width, height })
    }
}

#[derive(Debug)]
pub struct ColumnPolicy<I> {
    pub id: u64,
    rows: Vec<Row<I>>,
    active: usize,
    pub width: ColumnWidth,
    pub preset_width_idx: Option<usize>,
    pub is_full_width: bool,
    pub is_pending_fullscreen: bool,
    pub is_pending_maximized: bool,
    pub display_mode: ColumnDisplay,
    revision: u64,
    pub height_constraints: Vec<HeightConstraint>,
    pub resolved_heights: Vec<f64>,
    pub height_scratch: HeightScratch,
    spiral_geometry: Vec<Rect>,
}

impl<I: Clone + Eq + Hash> ColumnPolicy<I> {
    pub fn new(
        id: u64,
        width: ColumnWidth,
        full_width: bool,
        display: ColumnDisplay,
        preset: Option<usize>,
    ) -> Self {
        assert!(reserve_column_id(id), "column identity exhausted");
        Self {
            id,
            rows: Vec::new(),
            active: 0,
            width,
            preset_width_idx: preset,
            is_full_width: full_width,
            is_pending_fullscreen: false,
            is_pending_maximized: false,
            display_mode: display,
            revision: 0,
            height_constraints: Vec::new(),
            resolved_heights: Vec::new(),
            height_scratch: HeightScratch::default(),
            spiral_geometry: Vec::new(),
        }
    }
    pub fn adopt_id(&mut self, id: u64) {
        assert!(reserve_column_id(id), "column identity exhausted");
        self.id = id;
    }
    pub fn rows(&self) -> &[Row<I>] {
        &self.rows
    }
    pub fn rows_mut(&mut self) -> &mut [Row<I>] {
        self.mark_dirty();
        &mut self.rows
    }
    pub fn active_index(&self) -> usize {
        self.active
    }
    pub fn active_id(&self) -> Option<&I> {
        self.rows.get(self.active).map(Row::id)
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn mark_dirty(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
    pub fn position(&self, id: &I) -> Option<usize> {
        self.rows.iter().position(|row| row.id == *id)
    }
    pub fn activate(&mut self, index: usize) -> bool {
        if self.rows.is_empty() {
            self.active = 0;
            return false;
        }
        let index = index.min(self.rows.len() - 1);
        if self.active == index {
            return false;
        }
        self.active = index;
        if self.display_mode == ColumnDisplay::Tabbed {
            self.mark_dirty();
        }
        true
    }
    pub fn one_based_index(&self, index: usize) -> Option<usize> {
        (!self.rows.is_empty()).then(|| index.saturating_sub(1).min(self.rows.len() - 1))
    }
    pub fn adjacent(&self, forward: bool) -> Option<usize> {
        if self.rows.is_empty() {
            return None;
        }
        if forward {
            (self.active + 1 < self.rows.len()).then_some(self.active + 1)
        } else {
            self.active.checked_sub(1)
        }
    }
    pub fn insert(&mut self, index: usize, id: I) -> bool {
        if index > self.rows.len() || self.position(&id).is_some() {
            return false;
        }
        if !self.rows.is_empty() && index <= self.active {
            self.active += 1;
        }
        self.rows.insert(
            index,
            Row {
                id,
                height: WindowHeight::auto_1(),
            },
        );
        if self.display_mode != ColumnDisplay::Tabbed {
            self.is_pending_fullscreen = false;
            self.is_pending_maximized = false;
        }
        self.mark_dirty();
        true
    }
    pub fn remove(&mut self, index: usize) -> (Row<I>, Removal) {
        let old_active = self.active;
        let row = self.rows.remove(index);
        self.active = if self.rows.is_empty() {
            0
        } else if index < old_active {
            old_active - 1
        } else {
            old_active.min(self.rows.len() - 1)
        };
        self.normalize_single_weight();
        self.mark_dirty();
        (
            row,
            Removal {
                old_active,
                new_active: self.active,
                removed_active: index == old_active,
            },
        )
    }
    pub fn swap(&mut self, first: usize, second: usize, select_second: bool) {
        self.rows.swap(first, second);
        if select_second {
            self.active = second;
        }
        self.mark_dirty();
    }
    pub fn normalize_single_weight(&mut self) {
        if let [Row {
            height: WindowHeight::Auto { weight },
            ..
        }] = self.rows.as_mut_slice()
        {
            *weight = 1.;
        }
    }
    pub fn reset_height(&mut self, index: usize) {
        if self.display_mode == ColumnDisplay::Tabbed {
            for row in &mut self.rows {
                row.height = WindowHeight::auto_1();
            }
        } else if let Some(row) = self.rows.get_mut(index) {
            row.height = WindowHeight::auto_1();
        }
        self.mark_dirty();
    }
    pub fn toggle_full_width(&mut self) {
        if self.is_pending_maximized {
            self.is_pending_maximized = false;
            self.is_full_width = false;
        } else {
            self.is_full_width = !self.is_full_width;
        }
        self.mark_dirty();
    }
    pub fn next_width_preset(
        &self,
        forward: bool,
        presets: &[PresetExtent],
        current_window: f64,
        current_tile: f64,
    ) -> Option<usize> {
        let count = presets.len();
        if count == 0 {
            return None;
        }
        let previous = if self.is_full_width || self.is_pending_maximized {
            None
        } else {
            self.preset_width_idx.filter(|index| *index < count)
        };
        if let Some(index) = previous {
            return Some((index + if forward { 1 } else { count - 1 }) % count);
        }
        let matching = presets.iter().enumerate().filter_map(|(index, preset)| {
            let (current, resolved) = match *preset {
                PresetExtent::Tile(value) => (current_tile, value),
                PresetExtent::Window(value) => (current_window, value),
            };
            (if forward {
                current + 1. < resolved
            } else {
                resolved + 1. < current
            })
            .then_some(index)
        });
        Some(if forward {
            matching.into_iter().next().unwrap_or(0)
        } else {
            matching.into_iter().next_back().unwrap_or(count - 1)
        })
    }
    pub fn select_width_preset(&mut self, index: usize) {
        self.preset_width_idx = Some(index);
        self.mark_dirty();
    }
    pub fn next_height_preset(
        &self,
        index: usize,
        forward: bool,
        presets: &[f64],
        current_tile: f64,
    ) -> Option<usize> {
        let count = presets.len();
        if count == 0 {
            return None;
        }
        if let WindowHeight::Preset(index) = self.rows[index].height {
            if !self.is_pending_maximized && index < count {
                return Some((index + if forward { 1 } else { count - 1 }) % count);
            }
        }
        let matching = presets.iter().enumerate().filter_map(|(index, value)| {
            (if forward {
                current_tile + 1. < *value
            } else {
                *value + 1. < current_tile
            })
            .then_some(index)
        });
        Some(if forward {
            matching.into_iter().next().unwrap_or(0)
        } else {
            matching.into_iter().next_back().unwrap_or(count - 1)
        })
    }
    pub fn select_height_preset(&mut self, index: usize, preset: usize) {
        self.rows[index].height = WindowHeight::Preset(preset);
        self.is_pending_maximized = false;
        self.mark_dirty();
    }
    pub fn set_width(&mut self, change: DimensionChange, context: WidthContext) {
        let current = if self.is_full_width || self.is_pending_maximized {
            ColumnWidth::Proportion(1.)
        } else {
            self.width
        };
        let current_px = resolve_column_width(
            current,
            context.working_width,
            context.gap,
            context.extra_width,
        );
        const MAX_PX: f64 = 100000.;
        const MAX_F: f64 = 10000.;
        self.width = match (current, change) {
            (_, DimensionChange::SetFixed(width)) => {
                ColumnWidth::Fixed((width + context.decoration).clamp(1., MAX_PX))
            }
            (_, DimensionChange::SetProportion(proportion)) => {
                ColumnWidth::Proportion((proportion / 100.).clamp(0., MAX_F))
            }
            (_, DimensionChange::AdjustFixed(delta)) => {
                ColumnWidth::Fixed((current_px + delta).clamp(1., MAX_PX))
            }
            (ColumnWidth::Proportion(current), DimensionChange::AdjustProportion(delta)) => {
                ColumnWidth::Proportion((current + delta / 100.).clamp(0., MAX_F))
            }
            (ColumnWidth::Fixed(_), DimensionChange::AdjustProportion(delta)) => {
                let full = context.working_width - context.gap;
                let current = if full == 0. {
                    1.
                } else {
                    (current_px + context.gap + context.extra_width) / full
                };
                ColumnWidth::Proportion((current + delta / 100.).clamp(0., MAX_F))
            }
        };
        self.preset_width_idx = None;
        self.is_full_width = false;
        self.is_pending_maximized = false;
        self.mark_dirty();
    }
    pub fn set_height(&mut self, index: usize, change: DimensionChange, context: HeightContext) {
        let current_window = match self.rows[index].height {
            WindowHeight::Fixed(height) => height,
            WindowHeight::Auto { .. } | WindowHeight::Preset(_) => context.current_window_height,
        };
        let current_tile = current_window + context.decoration;
        let full = context.working_height - context.gap;
        let current_prop = if full == 0. {
            1.
        } else {
            (current_tile + context.gap) / full
        };
        let mut height = match change {
            DimensionChange::SetFixed(height) => height,
            DimensionChange::SetProportion(proportion) => {
                full * (proportion / 100.) - context.gap - context.extra_height - context.decoration
            }
            DimensionChange::AdjustFixed(delta) => current_window + delta,
            DimensionChange::AdjustProportion(delta) => {
                full * (current_prop + delta / 100.)
                    - context.gap
                    - context.extra_height
                    - context.decoration
            }
        };
        let taken = if self.display_mode == ColumnDisplay::Tabbed {
            0.
        } else {
            context.other_minimum_tile_height
        };
        let left =
            context.working_height - context.extra_height - context.gap - taken - context.gap;
        let left = f64::max(1., left - context.decoration);
        height = f64::min(left, height);
        if context.maximum_window_height > 0. {
            height = f64::min(height, context.maximum_window_height);
        }
        if context.minimum_window_height > 0. {
            height = f64::max(height, context.minimum_window_height);
        }
        self.rows[index].height = WindowHeight::Fixed(height.clamp(1., 100000.));
        self.is_pending_maximized = false;
        self.mark_dirty();
    }
    pub fn set_fullscreen(&mut self, fullscreen: bool) -> bool {
        if self.is_pending_fullscreen == fullscreen {
            return false;
        }
        if fullscreen {
            assert!(self.rows.len() == 1 || self.display_mode == ColumnDisplay::Tabbed);
        }
        self.is_pending_fullscreen = fullscreen;
        self.mark_dirty();
        true
    }
    pub fn set_maximized(&mut self, maximized: bool) -> bool {
        if self.is_pending_maximized == maximized {
            return false;
        }
        if maximized {
            assert!(self.rows.len() == 1 || self.display_mode == ColumnDisplay::Tabbed);
        }
        self.is_pending_maximized = maximized;
        self.mark_dirty();
        true
    }
    pub fn set_display(&mut self, display: ColumnDisplay) -> bool {
        if self.display_mode == display {
            return false;
        }
        self.display_mode = display;
        self.mark_dirty();
        true
    }
    /// Preserve relative apparent heights using the established median normalization.
    pub fn automatic_heights(&mut self, heights: &[f64]) {
        assert_eq!(heights.len(), self.rows.len());
        if heights.is_empty() {
            return;
        }
        let mut sorted = heights.to_vec();
        sorted.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
        let median = sorted[sorted.len() / 2];
        for (row, height) in self.rows.iter_mut().zip(heights) {
            row.height = WindowHeight::Auto {
                weight: height / median,
            };
        }
        self.mark_dirty();
    }
    pub fn arrange_heights(
        &mut self,
        working_height: f64,
        gaps: f64,
        tabbed: bool,
        max_height: f64,
    ) {
        crate::column::resolve_heights_into(
            working_height,
            gaps,
            tabbed,
            max_height,
            &self.height_constraints,
            &mut self.resolved_heights,
            &mut self.height_scratch,
        );
    }
    /// Client constraints arrive already converted from window to tile units.
    pub fn arranged_width(
        &self,
        working_width: f64,
        gap: f64,
        extra_width: f64,
        constraints: impl Iterator<Item = (f64, f64)>,
    ) -> f64 {
        let (mut minimum, mut maximum): (f64, f64) = (0., f64::from(i32::MAX));
        let mut bounded = false;
        for (min, max) in constraints {
            minimum = minimum.max(min);
            if max != 0. {
                maximum = if bounded { maximum.min(max) } else { max };
                bounded = true;
            }
        }
        let maximum = maximum.max(minimum);
        let width = if self.is_full_width {
            ColumnWidth::Proportion(1.)
        } else {
            self.width
        };
        resolve_column_width(width, working_width, gap, extra_width)
            .min(maximum)
            .max(minimum)
    }
    /// Reserve all other automatic rows before capping one fixed/preset row.
    pub fn nonautomatic_height_cap(
        &self,
        max_height: f64,
        gap: f64,
        minimum_heights: impl Iterator<Item = f64>,
    ) -> Option<(usize, f64)> {
        if self.rows.len() <= 1 || self.display_mode == ColumnDisplay::Tabbed {
            return None;
        }
        let index = self
            .rows
            .iter()
            .position(|row| !matches!(row.height, WindowHeight::Auto { .. }))?;
        let occupied = minimum_heights
            .enumerate()
            .filter(|(other, _)| *other != index)
            .map(|(_, height)| height + gap)
            .sum::<f64>();
        Some((index, max_height - occupied))
    }
    /// Locate a directional neighbor from unanimated logical tile geometry.
    pub fn directional_neighbor(
        &self,
        active_rect: Rect,
        geometry: impl Iterator<Item = Rect>,
        vertical: bool,
        positive: bool,
    ) -> Option<usize> {
        let (cx, cy) = (
            active_rect.x + active_rect.width / 2.,
            active_rect.y + active_rect.height / 2.,
        );
        let mut best = None;
        let mut distance = f64::MAX;
        for (index, rect) in geometry.enumerate() {
            if index == self.active {
                continue;
            }
            let (x, y) = (rect.x + rect.width / 2., rect.y + rect.height / 2.);
            let (primary, secondary) = if vertical {
                (if positive { y - cy } else { cy - y }, x - cx)
            } else {
                (if positive { x - cx } else { cx - x }, y - cy)
            };
            if primary < 10. {
                continue;
            }
            let candidate = primary * primary + secondary * secondary;
            if candidate < distance {
                distance = candidate;
                best = Some(index);
            }
        }
        best
    }
    pub fn horizontal_extreme(
        &self,
        geometry: impl Iterator<Item = Rect>,
        positive: bool,
    ) -> Option<usize> {
        let mut best = None;
        let mut bound = if positive {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
        for (index, rect) in geometry.enumerate().take(self.rows.len()) {
            let center = rect.x + rect.width / 2.;
            if (positive && center > bound) || (!positive && center < bound) {
                bound = center;
                best = Some(index);
            }
        }
        best
    }
}

impl<I: Clone + Eq + Hash> LiveLayoutPolicy<I> for ColumnPolicy<I> {
    type Request<'a>
        = ColumnArrangement
    where
        Self: 'a;

    fn arrange(&mut self, request: ColumnArrangement, mut emit: impl FnMut(&I, Rect)) {
        let ColumnArrangement::Normal {
            width,
            working_height,
            gap,
            tabbed_limit,
        } = request
        else {
            if let ColumnArrangement::Expanded(bounds) = request {
                for row in &self.rows {
                    emit(&row.id, bounds.sanitized());
                }
            }
            return;
        };
        if self.display_mode == ColumnDisplay::Dwindle {
            crate::geometry::spiral_rects_into(
                Rect::new(0., 0., width, (working_height - gap * 2.).max(1.)),
                self.rows.len(),
                gap,
                &mut self.spiral_geometry,
            );
            for (row, rect) in self.rows.iter().zip(&self.spiral_geometry) {
                emit(&row.id, *rect);
            }
            return;
        }
        let tabbed = self.display_mode == ColumnDisplay::Tabbed;
        self.arrange_heights(working_height, gap, tabbed, tabbed_limit);
        let mut y = 0.;
        for (row, height) in self.rows.iter().zip(&self.resolved_heights) {
            emit(&row.id, Rect::new(0., y, width, *height));
            if !tabbed {
                y += height + gap;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn insertion_and_removal_preserve_identity_and_repair_active_rows() {
        let mut column = ColumnPolicy::new(
            7,
            ColumnWidth::Proportion(0.5),
            false,
            ColumnDisplay::Normal,
            None,
        );
        assert!(column.insert(0, 1));
        assert!(column.insert(1, 2));
        column.activate(1);
        assert!(column.insert(0, 3));
        assert_eq!(column.active_id(), Some(&2));
        assert!(!column.insert(0, 2));
        let (_, removed) = column.remove(2);
        assert!(removed.removed_active);
        assert_eq!(column.active_id(), Some(&1));
        column.remove(0);
        column.rows_mut()[0].height = WindowHeight::Auto { weight: 4. };
        column.normalize_single_weight();
        assert_eq!(column.rows()[0].height, WindowHeight::auto_1());
    }
    #[test]
    fn column_removal_and_move_keep_stable_focus_and_previous_preference() {
        let mut order = ColumnOrder::default();
        order.insert(0, 10);
        order.insert(1, 20);
        order.insert(2, 30);
        order.activate(2);
        assert_eq!(order.move_active_to(0), Some((2, 0)));
        assert_eq!(order.ids(), &[30, 10, 20]);
        assert_eq!(order.active_id(), Some(30));
        order.activate(2);
        order.activate_previous_on_removal = Some(17.);
        let result = order.remove(2);
        assert_eq!(result.new_active, 1);
        assert_eq!(order.active_id(), Some(10));
        order.remove(0);
        assert_eq!(order.active_id(), Some(10));
        order.remove(0);
        assert_eq!(order.active_id(), None);
    }
    #[test]
    fn row_preferences_follow_swaps_and_tabbed_height_reset() {
        let mut column = ColumnPolicy::new(
            7,
            ColumnWidth::Fixed(400.),
            false,
            ColumnDisplay::Tabbed,
            None,
        );
        for id in [1, 2, 3] {
            column.insert(column.rows().len(), id);
        }
        column.rows_mut()[0].height = WindowHeight::Fixed(200.);
        column.swap(0, 2, true);
        assert_eq!(column.active_id(), Some(&1));
        assert_eq!(column.rows()[2].height, WindowHeight::Fixed(200.));
        column.automatic_heights(&[100., 200., 400.]);
        assert_eq!(column.rows()[0].height, WindowHeight::Auto { weight: 0.5 });
        assert_eq!(column.rows()[2].height, WindowHeight::Auto { weight: 2. });
        column.reset_height(0);
        assert!(column
            .rows()
            .iter()
            .all(|row| row.height == WindowHeight::auto_1()));
    }
    #[test]
    fn width_changes_preserve_proportions_and_reset_expanded_policy() {
        let mut column = ColumnPolicy::new(
            7,
            ColumnWidth::Proportion(0.5),
            false,
            ColumnDisplay::Normal,
            Some(0),
        );
        column.insert(0, 1);
        let context = WidthContext {
            working_width: 1000.,
            gap: 16.,
            extra_width: 20.,
            decoration: 8.,
        };
        assert_eq!(resolve_column_width(column.width, 1000., 16., 20.), 456.);
        column.set_width(DimensionChange::SetFixed(400.), context);
        assert_eq!(column.width, ColumnWidth::Fixed(408.));
        assert_eq!(column.preset_width_idx, None);
        column.set_width(DimensionChange::AdjustProportion(10.), context);
        assert_eq!(
            column.width,
            ColumnWidth::Proportion((408. + 16. + 20.) / 984. + 0.1)
        );
        column.set_maximized(true);
        column.toggle_full_width();
        assert!(!column.is_pending_maximized && !column.is_full_width);
        column.toggle_full_width();
        column.set_width(DimensionChange::AdjustFixed(-100.), context);
        assert_eq!(column.width, ColumnWidth::Fixed(848.));
        assert!(!column.is_full_width);
    }
    #[test]
    fn height_changes_account_for_decorations_and_other_client_minima() {
        let mut column = ColumnPolicy::new(
            7,
            ColumnWidth::Fixed(400.),
            false,
            ColumnDisplay::Normal,
            None,
        );
        column.insert(0, 1);
        column.insert(1, 2);
        let context = HeightContext {
            working_height: 1000.,
            gap: 16.,
            extra_height: 20.,
            decoration: 8.,
            current_window_height: 300.,
            other_minimum_tile_height: 216.,
            minimum_window_height: 100.,
            maximum_window_height: 800.,
        };
        column.set_height(0, DimensionChange::SetProportion(50.), context);
        assert_eq!(column.rows()[0].height, WindowHeight::Fixed(448.));
        column.set_height(0, DimensionChange::SetFixed(10000.), context);
        assert_eq!(column.rows()[0].height, WindowHeight::Fixed(724.));
        column.set_display(ColumnDisplay::Tabbed);
        column.set_height(0, DimensionChange::SetFixed(10000.), context);
        assert_eq!(column.rows()[0].height, WindowHeight::Fixed(800.));
        column.set_height(0, DimensionChange::SetFixed(1.), context);
        assert_eq!(column.rows()[0].height, WindowHeight::Fixed(100.));
    }
    #[test]
    fn directional_selection_uses_stable_row_order_and_first_equal_distance() {
        let mut column = ColumnPolicy::new(
            7,
            ColumnWidth::Fixed(400.),
            false,
            ColumnDisplay::Dwindle,
            None,
        );
        for id in [1, 2, 3, 4] {
            column.insert(column.rows().len(), id);
        }
        let rectangles = [
            Rect::new(0., 0., 100., 100.),
            Rect::new(110., 0., 100., 100.),
            Rect::new(0., 110., 100., 100.),
            Rect::new(110., 110., 100., 100.),
        ];
        assert_eq!(
            column.directional_neighbor(rectangles[0], rectangles.into_iter(), false, true),
            Some(1)
        );
        assert_eq!(
            column.directional_neighbor(rectangles[0], rectangles.into_iter(), true, true),
            Some(2)
        );
        assert_eq!(
            column.horizontal_extreme(rectangles.into_iter(), true),
            Some(1)
        );
        assert_eq!(
            column.horizontal_extreme(rectangles.into_iter(), false),
            Some(0)
        );
    }
    #[test]
    fn interactive_resize_preserves_centered_width_and_suppresses_top_row_motion() {
        let mut column = ColumnPolicy::new(
            7,
            ColumnWidth::Fixed(400.),
            false,
            ColumnDisplay::Normal,
            None,
        );
        column.insert(0, 1);
        column.insert(1, 2);
        let resize = crate::resize::Resize::new(1, crate::Size::new(400., 300.), 5);
        assert_eq!(
            column.resize_intent(resize, [30., -20.], true),
            Some(crate::resize::ResizeIntent {
                width: Some(340),
                height: None
            })
        );
        let resize = crate::resize::Resize::new(2, crate::Size::new(400., 300.), 5);
        assert_eq!(
            column.resize_intent(resize, [30., -20.], false),
            Some(crate::resize::ResizeIntent {
                width: Some(370),
                height: Some(320)
            })
        );
        assert_eq!(column.resize_intent(resize, [f64::NAN, 0.], false), None);
    }

    #[test]
    fn independent_width_constraints_and_nonautomatic_height_cap_keep_client_minima() {
        let mut column = ColumnPolicy::new(
            7,
            ColumnWidth::Fixed(800.),
            false,
            ColumnDisplay::Normal,
            None,
        );
        column.insert(0, 1);
        column.insert(1, 2);
        column.insert(2, 3);
        assert_eq!(
            column.arranged_width(1000., 16., 0., [(400., 600.), (700., 650.)].into_iter()),
            700.
        );
        column.rows_mut()[1].height = WindowHeight::Fixed(400.);
        assert_eq!(
            column.nonautomatic_height_cap(1000., 16., [100., 200., 300.].into_iter()),
            Some((1, 568.))
        );
        column.set_display(ColumnDisplay::Tabbed);
        assert_eq!(
            column.nonautomatic_height_cap(1000., 16., [100., 200., 300.].into_iter()),
            None
        );
    }

    #[test]
    fn preset_cycles_use_the_correct_window_or_tile_units_and_expanded_reset() {
        let mut column = ColumnPolicy::new(
            7,
            ColumnWidth::Fixed(400.),
            false,
            ColumnDisplay::Normal,
            None,
        );
        column.insert(0, 1);
        let widths = [
            PresetExtent::Window(400.),
            PresetExtent::Tile(500.),
            PresetExtent::Window(600.),
        ];
        assert_eq!(column.next_width_preset(true, &widths, 410., 418.), Some(1));
        assert_eq!(
            column.next_width_preset(false, &widths, 410., 418.),
            Some(0)
        );
        column.select_width_preset(1);
        assert_eq!(column.next_width_preset(true, &widths, 410., 418.), Some(2));
        column.set_maximized(true);
        assert_eq!(column.next_width_preset(true, &widths, 410., 418.), Some(1));
        assert_eq!(column.next_width_preset(true, &[], 410., 418.), None);
        column.set_maximized(false);
        column.select_height_preset(0, 1);
        assert_eq!(
            column.next_height_preset(0, false, &[300., 400., 500.], 550.),
            Some(0)
        );
        column.set_maximized(true);
        assert_eq!(
            column.next_height_preset(0, false, &[300., 400., 500.], 550.),
            Some(2)
        );
        assert_eq!(column.next_height_preset(0, false, &[], 550.), None);
    }
}

#[cfg(test)]
mod identity_tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    #[test]
    fn local_reservations_skip_foreign_high_ids_and_reject_exhaustion_without_wrapping() {
        let counter = AtomicU64::new(1);
        assert_eq!(allocate_from(&counter), Some(1));
        assert!(reserve_in(&counter, 10000));
        assert_eq!(allocate_from(&counter), Some(10001));
        assert!(reserve_in(&counter, 9));
        assert_eq!(allocate_from(&counter), Some(10002));
        assert!(!reserve_in(&counter, u64::MAX));
        assert_eq!(counter.load(Ordering::Relaxed), 10003);
        assert!(reserve_in(&counter, u64::MAX - 1));
        assert_eq!(allocate_from(&counter), None);
        assert_eq!(counter.load(Ordering::Relaxed), u64::MAX);
    }
    #[test]
    fn adopted_policy_ids_are_reserved_before_any_future_live_column() {
        let high = allocate_column_id().checked_add(1000000).unwrap();
        let mut policy = ColumnPolicy::<u64>::new(
            high,
            ColumnWidth::Fixed(400.),
            false,
            ColumnDisplay::Normal,
            None,
        );
        assert!(allocate_column_id() > high);
        policy.adopt_id(high + 1000000);
        assert!(allocate_column_id() > policy.id);
        let second = ColumnPolicy::<u64>::new(
            allocate_column_id(),
            ColumnWidth::Fixed(500.),
            false,
            ColumnDisplay::Normal,
            None,
        );
        assert_ne!(policy.id, second.id);
    }
}
