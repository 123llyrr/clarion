//! Floating desktop policy with stable identity and device-independent state.
//!
//! Client handles, decoration sizes, protocol serials and rendered animation
//! offsets are supplied by the adapter. This policy owns target placement,
//! stacking order, focus and the numeric state of an interactive resize.

use crate::geometry::{finite, nonnegative, Axis, Rect, Size};
use crate::layout::{resolve_floating_rect, FloatingPosition, LiveLayoutPolicy};
pub use crate::resize::{Resize, ResizeIntent};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// Relative working-area position survives display size changes.
    pub relative: [f64; 2],
    pub unbounded: bool,
    pub rectangle: Rect,
    pub area: Rect,
}

impl Placement {
    pub fn new(area: Rect, size: Size, position: [f64; 2], unbounded: bool) -> Self {
        let mut placement = Self {
            relative: [0.0; 2],
            unbounded,
            rectangle: Rect::new(0.0, 0.0, size.width, size.height).sanitized(),
            area: area.sanitized(),
        };
        placement.set_position(position);
        placement
    }

    pub fn relative_position(area: Rect, position: [f64; 2]) -> [f64; 2] {
        [
            finite((position[0] - area.x) / area.width.max(1.0), 0.0),
            finite((position[1] - area.y) / area.height.max(1.0), 0.0),
        ]
    }

    pub fn logical_position(area: Rect, relative: [f64; 2]) -> [f64; 2] {
        [
            finite(relative[0] * area.width + area.x, area.x),
            finite(relative[1] * area.height + area.y, area.y),
        ]
    }

    fn recompute(&mut self) {
        let position = Self::logical_position(self.area, self.relative);
        let mode = if self.unbounded {
            FloatingPosition::Unbounded
        } else {
            FloatingPosition::Reachable
        };
        self.rectangle = resolve_floating_rect(
            Rect::new(
                position[0],
                position[1],
                self.rectangle.width,
                self.rectangle.height,
            ),
            self.area,
            mode,
        );
    }

    pub fn set_position(&mut self, position: [f64; 2]) {
        self.relative = Self::relative_position(
            self.area,
            [
                finite(position[0], self.rectangle.x),
                finite(position[1], self.rectangle.y),
            ],
        );
        self.recompute();
    }

    pub fn set_size(&mut self, size: Size) {
        let size = size.sanitized();
        if self.rectangle.size() == size {
            return;
        }
        self.rectangle.width = size.width;
        self.rectangle.height = size.height;
        self.recompute();
    }

    pub fn set_area(&mut self, area: Rect) {
        let area = area.sanitized();
        if self.area == area {
            return;
        }
        if self.unbounded {
            self.relative = Self::relative_position(area, [self.rectangle.x, self.rectangle.y]);
        }
        self.area = area;
        self.recompute();
    }

    pub fn set_unbounded(&mut self, unbounded: bool) {
        self.unbounded = unbounded;
        self.recompute();
    }

    pub fn center(&self) -> [f64; 2] {
        [
            self.rectangle.x + self.rectangle.width / 2.0,
            self.rectangle.y + self.rectangle.height / 2.0,
        ]
    }

    pub fn is_consistent(&self) -> bool {
        let mut recomputed = *self;
        recomputed.recompute();
        recomputed == *self
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PositionChange {
    SetFixed(f64),
    SetProportion(f64),
    AdjustFixed(f64),
    AdjustProportion(f64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Top,
    Bottom,
    Left,
    Right,
}

pub fn centered_position(area: Rect, size: Size) -> [f64; 2] {
    let area = area.sanitized();
    let size = size.sanitized();
    [
        area.x + ((area.width - size.width) / 2.0).max(0.0),
        area.y + ((area.height - size.height) / 2.0).max(0.0),
    ]
}

pub fn anchored_position(area: Rect, size: Size, anchor: Anchor, offset: [f64; 2]) -> [f64; 2] {
    let area = area.sanitized();
    let size = size.sanitized();
    let mut position = [finite(offset[0], 0.0), finite(offset[1], 0.0)];
    if matches!(
        anchor,
        Anchor::TopRight | Anchor::BottomRight | Anchor::Right
    ) {
        position[0] = area.width - size.width - position[0];
    }
    if matches!(
        anchor,
        Anchor::BottomLeft | Anchor::BottomRight | Anchor::Bottom
    ) {
        position[1] = area.height - size.height - position[1];
    }
    if matches!(anchor, Anchor::Top | Anchor::Bottom) {
        position[0] += area.width / 2.0 - size.width / 2.0;
    }
    if matches!(anchor, Anchor::Left | Anchor::Right) {
        position[1] += area.height / 2.0 - size.height / 2.0;
    }
    [
        finite(position[0] + area.x, area.x),
        finite(position[1] + area.y, area.y),
    ]
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SizeChange {
    SetFixed(i32),
    SetProportion(f64),
    AdjustFixed(i32),
    AdjustProportion(f64),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Preset {
    Fixed(i32),
    Proportion(f64),
}

/// Decoration facts and client hints are adapter inputs, not policy state.
#[derive(Debug, Clone, Copy)]
pub struct SizingInput {
    pub available: Size,
    pub current_client: [i32; 2],
    pub expected_client: [i32; 2],
    pub current_tile: Size,
    pub decoration: Size,
    pub minimum: [i32; 2],
    pub maximum: [i32; 2],
}

fn axis_index(axis: Axis) -> usize {
    match axis {
        Axis::Horizontal => 0,
        Axis::Vertical => 1,
    }
}
fn extent(size: Size, axis: Axis) -> f64 {
    match axis {
        Axis::Horizontal => size.width,
        Axis::Vertical => size.height,
    }
}
fn hinted(mut size: i32, minimum: i32, maximum: i32) -> i32 {
    if maximum > 0 {
        size = size.min(maximum);
    }
    if minimum > 0 {
        size = size.max(minimum);
    }
    size
}

/// Restore a saved floating client size while preserving zero-size negotiation.
pub fn restored_client_size(
    saved: Option<[i32; 2]>,
    expanded: bool,
    expected: Option<[i32; 2]>,
    minimum: [i32; 2],
    maximum: [i32; 2],
) -> [i32; 2] {
    let size = if expanded {
        saved.unwrap_or_default()
    } else {
        saved.or(expected).unwrap_or_default()
    };
    std::array::from_fn(|axis| {
        if size[axis] != 0 {
            hinted(size[axis], minimum[axis], maximum[axis])
        } else if minimum[axis] > 0 && minimum[axis] == maximum[axis] {
            minimum[axis]
        } else {
            0
        }
    })
}

pub fn initial_client_size(
    width: Option<Preset>,
    height: Option<Preset>,
    available: Size,
    decoration: Size,
) -> [i32; 2] {
    let resolve = |preset, axis| match preset {
        None => 0,
        Some(Preset::Fixed(value)) => value.max(1),
        Some(Preset::Proportion(value)) => (extent(available, axis) * finite(value, 0.0)
            - extent(decoration, axis))
        .floor()
        .max(1.0) as i32,
    };
    [
        resolve(width, Axis::Horizontal),
        resolve(height, Axis::Vertical),
    ]
}

pub fn client_bounds(available: Size, decoration: Size) -> [i32; 2] {
    [
        (available.width - decoration.width).max(1.0).floor() as i32,
        (available.height - decoration.height).max(1.0).floor() as i32,
    ]
}

pub fn resolve_size_change(axis: Axis, change: SizeChange, input: SizingInput) -> [i32; 2] {
    let index = axis_index(axis);
    let available = nonnegative(extent(input.available, axis));
    let decoration = finite(extent(input.decoration, axis), 0.0);
    let requested = match change {
        SizeChange::SetFixed(value) => f64::from(value),
        SizeChange::AdjustFixed(value) => {
            f64::from(input.current_client[index].saturating_add(value))
        }
        SizeChange::SetProportion(value) => {
            available * (finite(value, 0.0) / 100.0).clamp(0.0, 10000.0) - decoration
        }
        SizeChange::AdjustProportion(value) => {
            let proportion =
                extent(input.current_tile, axis) / available.max(1.0) + finite(value, 0.0) / 100.0;
            available * proportion.clamp(0.0, 10000.0) - decoration
        }
    };
    let changed = finite(requested, 1.0).round().clamp(1.0, 100000.0) as i32;
    let mut size = input.expected_client;
    size[index] = changed;
    std::array::from_fn(|axis| hinted(size[axis], input.minimum[axis], input.maximum[axis]))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Member {
    pub id: u64,
    pub placement: Placement,
    parents: Vec<u64>,
    /// Live preset selection belongs to the policy. A removed member exports
    /// these values into the adapter's mode-transfer metadata.
    pub presets: [Option<usize>; 2],
    pub requested_client: Option<[i32; 2]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrderChange {
    pub from: usize,
    pub to: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacementChange {
    pub id: u64,
    pub previous: Rect,
    pub current: Rect,
}

/// Topmost member comes first. Payload arrays follow returned insertion and
/// reorder indices; focus is identified by ID rather than a shifting index.
#[derive(Debug, Clone, Default)]
pub struct FloatingPolicy {
    members: Vec<Member>,
    active: Option<u64>,
    resize: Option<Resize>,
}

impl FloatingPolicy {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn len(&self) -> usize {
        self.members.len()
    }
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }
    pub fn active(&self) -> Option<u64> {
        self.active
    }
    pub fn resize(&self) -> Option<&Resize> {
        self.resize.as_ref()
    }
    pub fn members(&self) -> impl DoubleEndedIterator<Item = &Member> + ExactSizeIterator {
        self.members.iter()
    }
    pub fn placements(&self) -> impl Iterator<Item = &Placement> {
        self.members.iter().map(|member| &member.placement)
    }
    pub fn placement_mut(&mut self, index: usize) -> &mut Placement {
        &mut self.members[index].placement
    }
    pub fn placement(&self, index: usize) -> &Placement {
        &self.members[index].placement
    }
    pub fn index(&self, id: u64) -> Option<usize> {
        self.members.iter().position(|member| member.id == id)
    }
    pub fn contains(&self, id: u64) -> bool {
        self.index(id).is_some()
    }

    pub fn insert(
        &mut self,
        id: u64,
        placement: Placement,
        requested_index: usize,
        activate: bool,
        ancestors: &[u64],
    ) -> Option<usize> {
        if self.contains(id) {
            return None;
        }
        let requested_index = requested_index.min(self.members.len());
        let index = self
            .members
            .iter()
            .take(requested_index)
            .position(|member| ancestors.contains(&member.id))
            .unwrap_or(requested_index);
        self.members.insert(
            index,
            Member {
                id,
                placement,
                parents: ancestors.to_vec(),
                presets: [None; 2],
                requested_client: None,
            },
        );
        if activate || self.active.is_none() {
            self.active = Some(id);
        }
        Some(index)
    }

    pub fn remove(&mut self, id: u64) -> Option<(usize, Member)> {
        let index = self.index(id)?;
        let member = self.members.remove(index);
        for remaining in &mut self.members {
            remaining.parents.retain(|parent| *parent != id);
        }
        if self.active == Some(id) {
            self.active = self.members.first().map(|member| member.id);
        }
        if self.resize.as_ref().is_some_and(|resize| resize.id == id) {
            self.resize = None;
        }
        Some((index, member))
    }

    pub fn raise(&mut self, id: u64, index: usize) -> Option<OrderChange> {
        let from = self.index(id)?;
        let to = index.min(from);
        if from != to {
            let member = self.members.remove(from);
            self.members.insert(to, member);
        }
        Some(OrderChange { from, to })
    }

    /// Protocol parent relationships enter as numeric identities only. They
    /// change at map/parent mutation boundaries, never during frame traversal.
    pub fn set_parents(&mut self, id: u64, parents: Vec<u64>) -> bool {
        let Some(index) = self.index(id) else {
            return false;
        };
        self.members[index].parents = parents.into_iter().filter(|parent| *parent != id).collect();
        true
    }

    /// Raise a transient family without inverting the family's stacking order.
    pub fn raise_descendants(&mut self, id: u64) -> Vec<OrderChange> {
        let Some(index) = self.index(id) else {
            return Vec::new();
        };
        let mut descendants = Vec::new();
        for member in self.members.iter().skip(index + 1).rev() {
            if member.parents.contains(&id)
                || descendants
                    .iter()
                    .any(|child| member.parents.contains(child))
            {
                descendants.push(member.id);
            }
        }
        let mut changes = Vec::with_capacity(descendants.len());
        for (offset, descendant) in descendants.into_iter().rev().enumerate() {
            changes.push(self.raise(descendant, index + offset).unwrap());
        }
        changes
    }

    pub fn focus(&mut self, id: u64) -> bool {
        if !self.contains(id) {
            return false;
        }
        self.active = Some(id);
        true
    }

    pub fn restore_presets(&mut self, id: u64, presets: [Option<usize>; 2]) -> bool {
        let Some(index) = self.index(id) else {
            return false;
        };
        self.members[index].presets = presets;
        true
    }

    pub fn clear_presets(&mut self, axis: Axis) {
        let axis = axis_index(axis);
        for member in &mut self.members {
            member.presets[axis] = None;
        }
    }

    pub fn preset_indices(&self, index: usize) -> [Option<usize>; 2] {
        self.members[index].presets
    }

    pub fn change_size(
        &mut self,
        id: u64,
        axis: Axis,
        change: SizeChange,
        input: SizingInput,
    ) -> Option<[i32; 2]> {
        let index = self.index(id)?;
        let size = resolve_size_change(axis, change, input);
        self.members[index].presets[axis_index(axis)] = None;
        self.members[index].requested_client = Some(size);
        Some(size)
    }

    /// Choose presets in client units for fixed sizes and decorated tile units
    /// for proportions, using one logical pixel of comparison allowance.
    pub fn cycle_preset(
        &mut self,
        id: u64,
        axis: Axis,
        forwards: bool,
        mut presets: impl DoubleEndedIterator<Item = Preset> + ExactSizeIterator,
        input: SizingInput,
    ) -> Option<[i32; 2]> {
        let index = self.index(id)?;
        let count = presets.len();
        if count == 0 {
            return None;
        }
        let axis_index = axis_index(axis);
        let old = self.members[index].presets[axis_index].filter(|index| *index < count);
        let mut selected = None;
        if let Some(old) = old {
            let next = if forwards {
                (old + 1) % count
            } else {
                (old + count - 1) % count
            };
            selected = Some((next, presets.nth(next).unwrap()));
        } else if forwards {
            for (index, preset) in presets.enumerate() {
                let (current, desired) = match preset {
                    Preset::Fixed(value) => (
                        f64::from(input.current_client[axis_index]),
                        f64::from(value),
                    ),
                    Preset::Proportion(value) => (
                        extent(input.current_tile, axis),
                        extent(input.available, axis) * value,
                    ),
                };
                if selected.is_none() {
                    selected = Some((index, preset));
                }
                if current + 1.0 < desired {
                    selected = Some((index, preset));
                    break;
                }
            }
        } else {
            for (index, preset) in presets.enumerate().rev() {
                let (current, desired) = match preset {
                    Preset::Fixed(value) => (
                        f64::from(input.current_client[axis_index]),
                        f64::from(value),
                    ),
                    Preset::Proportion(value) => (
                        extent(input.current_tile, axis),
                        extent(input.available, axis) * value,
                    ),
                };
                if selected.is_none() {
                    selected = Some((index, preset));
                }
                if desired + 1.0 < current {
                    selected = Some((index, preset));
                    break;
                }
            }
        }
        let (preset_index, preset) = selected?;
        let change = match preset {
            Preset::Fixed(value) => SizeChange::SetFixed(value),
            Preset::Proportion(value) => SizeChange::SetProportion(value * 100.0),
        };
        let size = self.change_size(id, axis, change, input)?;
        self.members[index].presets[axis_index] = Some(preset_index);
        Some(size)
    }

    pub fn focus_directional(&mut self, direction: Direction) -> Option<u64> {
        let active = self.active?;
        let center = self.placement(self.index(active)?).center();
        let selected = self
            .members
            .iter()
            .filter(|member| member.id != active)
            .filter_map(|member| {
                let other = member.placement.center();
                let distance = match direction {
                    Direction::Left => center[0] - other[0],
                    Direction::Right => other[0] - center[0],
                    Direction::Up => center[1] - other[1],
                    Direction::Down => other[1] - center[1],
                };
                (distance > 0.0 && distance.is_finite()).then_some((member.id, distance))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|candidate| candidate.0)?;
        self.active = Some(selected);
        Some(selected)
    }

    pub fn focus_extreme(&mut self, axis: Axis, maximum: bool) -> Option<u64> {
        let position = |member: &Member| match axis {
            Axis::Horizontal => member.placement.rectangle.x,
            Axis::Vertical => member.placement.rectangle.y,
        };
        let selected = if maximum {
            self.members
                .iter()
                .max_by(|a, b| position(a).total_cmp(&position(b)))
        } else {
            self.members
                .iter()
                .min_by(|a, b| position(a).total_cmp(&position(b)))
        }?
        .id;
        self.active = Some(selected);
        Some(selected)
    }

    pub fn move_to(&mut self, id: u64, position: [f64; 2]) -> Option<PlacementChange> {
        let index = self.index(id)?;
        let member = &mut self.members[index];
        let previous = member.placement.rectangle;
        member.placement.set_position(position);
        Some(PlacementChange {
            id,
            previous,
            current: member.placement.rectangle,
        })
    }

    pub fn move_by(&mut self, id: u64, delta: [f64; 2]) -> Option<PlacementChange> {
        if !delta.iter().all(|value| value.is_finite()) {
            return None;
        }
        let rectangle = self.placement(self.index(id)?).rectangle;
        self.move_to(id, [rectangle.x + delta[0], rectangle.y + delta[1]])
    }

    pub fn center_window(&mut self, id: u64) -> Option<PlacementChange> {
        let placement = self.placement(self.index(id)?);
        let position = centered_position(placement.area, placement.rectangle.size());
        self.move_to(id, position)
    }

    pub fn position_above(&self, id: u64, size: Size) -> Option<[f64; 2]> {
        let placement = self.placement(self.index(id)?);
        let rect = crate::geometry::clamp_rect(
            Rect::new(
                placement.rectangle.x + (placement.rectangle.width - size.width) / 2.0,
                placement.rectangle.y + (placement.rectangle.height - size.height) / 2.0,
                size.width,
                size.height,
            ),
            placement.area,
        );
        Some([rect.x, rect.y])
    }

    pub fn reveal_delta(&self, id: u64) -> Option<[f64; 2]> {
        let placement = self.placement(self.index(id)?);
        if !placement.unbounded || placement.rectangle.intersection(placement.area).is_some() {
            return None;
        }
        let center = placement.center();
        Some([
            center[0] - (placement.area.x + placement.area.width / 2.0),
            center[1] - (placement.area.y + placement.area.height / 2.0),
        ])
    }

    /// Preserve the opposite edge when a committed client size carries resize
    /// metadata from an earlier configure, even after the grab has ended.
    pub fn commit_tile_size(
        &mut self,
        id: u64,
        size: Size,
        committed_edges: Option<u32>,
    ) -> Option<PlacementChange> {
        let index = self.index(id)?;
        let placement = &mut self.members[index].placement;
        let previous = placement.rectangle;
        placement.set_size(size);
        if let Some(edges) = committed_edges {
            // set_size may clamp an offscreen position for the new size. The
            // resize anchor belongs to the old rectangle, before that clamp.
            let x = if edges & 4 != 0 {
                previous.right() - placement.rectangle.width
            } else {
                placement.rectangle.x
            };
            let y = if edges & 1 != 0 {
                previous.bottom() - placement.rectangle.height
            } else {
                placement.rectangle.y
            };
            placement.set_position([x, y]);
        }
        Some(PlacementChange {
            id,
            previous,
            current: placement.rectangle,
        })
    }

    pub fn move_with_changes(
        &mut self,
        id: u64,
        x: PositionChange,
        y: PositionChange,
    ) -> Option<PlacementChange> {
        let placement = self.placement(self.index(id)?);
        let change = |position: f64, origin: f64, extent: f64, change: PositionChange| match change
        {
            PositionChange::SetFixed(value) => finite(value, position - origin) + origin,
            PositionChange::SetProportion(value) => {
                (finite(value, 0.0) / 100.0).clamp(0.0, 10000.0) * extent + origin
            }
            PositionChange::AdjustFixed(value) => position + finite(value, 0.0),
            PositionChange::AdjustProportion(value) => {
                ((position - origin) / extent.max(1.0) + finite(value, 0.0) / 100.0)
                    .clamp(0.0, 10000.0)
                    * extent
                    + origin
            }
        };
        let position = [
            change(
                placement.rectangle.x,
                placement.area.x,
                placement.area.width,
                x,
            ),
            change(
                placement.rectangle.y,
                placement.area.y,
                placement.area.height,
                y,
            ),
        ];
        self.move_to(id, position)
    }

    pub fn set_area(&mut self, area: Rect) {
        self.arrange(area, |_, _| {});
    }
    pub fn set_unbounded(&mut self, unbounded: bool) {
        for member in &mut self.members {
            member.placement.set_unbounded(unbounded);
        }
    }
    pub fn pan(&mut self, delta: [f64; 2]) {
        if !delta.iter().all(|value| value.is_finite()) {
            return;
        }
        for member in &mut self.members {
            let rectangle = member.placement.rectangle;
            member
                .placement
                .set_position([rectangle.x - delta[0], rectangle.y - delta[1]]);
        }
    }

    pub fn begin_resize(&mut self, id: u64, original_size: Size, edges: u32) -> bool {
        if self.resize.is_some() || !self.contains(id) {
            return false;
        }
        self.resize = Some(Resize::new(id, original_size, edges));
        true
    }

    pub fn resize_intent(&self, id: u64, delta: [f64; 2]) -> Option<ResizeIntent> {
        let resize = self.resize.as_ref().filter(|resize| resize.id == id)?;
        resize.intent(delta)
    }

    pub fn end_resize(&mut self, id: Option<u64>) -> Option<u64> {
        let resize = self.resize.as_ref()?;
        if id.is_some_and(|id| id != resize.id) {
            return None;
        }
        self.resize.take().map(|resize| resize.id)
    }

    pub fn verify_invariants(&self) -> bool {
        let ids: std::collections::HashSet<_> =
            self.members.iter().map(|member| member.id).collect();
        ids.len() == self.members.len()
            && self.active.is_some() == !self.members.is_empty()
            && self.active.is_none_or(|id| ids.contains(&id))
            && self
                .resize
                .as_ref()
                .is_none_or(|resize| ids.contains(&resize.id))
            && self.members.iter().all(|member| {
                let mut recomputed = member.placement;
                recomputed.recompute();
                recomputed == member.placement
                    && [
                        recomputed.rectangle.x,
                        recomputed.rectangle.y,
                        recomputed.rectangle.width,
                        recomputed.rectangle.height,
                    ]
                    .iter()
                    .all(|value| value.is_finite())
                    && recomputed.rectangle.width == nonnegative(recomputed.rectangle.width)
            })
    }
}

impl LiveLayoutPolicy<u64> for FloatingPolicy {
    type Request<'a> = Rect;

    fn arrange(&mut self, area: Rect, mut emit: impl FnMut(&u64, Rect)) {
        for member in &mut self.members {
            member.placement.set_area(area);
            emit(&member.id, member.placement.rectangle);
        }
    }
}

impl std::ops::Index<usize> for FloatingPolicy {
    type Output = Placement;
    fn index(&self, index: usize) -> &Self::Output {
        self.placement(index)
    }
}

impl std::ops::IndexMut<usize> for FloatingPolicy {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        self.placement_mut(index)
    }
}

impl<'a> IntoIterator for &'a FloatingPolicy {
    type Item = &'a Placement;
    type IntoIter = std::iter::Map<std::slice::Iter<'a, Member>, fn(&'a Member) -> &'a Placement>;
    fn into_iter(self) -> Self::IntoIter {
        self.members.iter().map(|member| &member.placement)
    }
}

impl<'a> IntoIterator for &'a mut FloatingPolicy {
    type Item = &'a mut Placement;
    type IntoIter =
        std::iter::Map<std::slice::IterMut<'a, Member>, fn(&'a mut Member) -> &'a mut Placement>;
    fn into_iter(self) -> Self::IntoIter {
        self.members.iter_mut().map(|member| &mut member.placement)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn placement(x: f64, y: f64) -> Placement {
        Placement::new(
            Rect::new(10.0, 20.0, 1000.0, 800.0),
            Size::new(200.0, 100.0),
            [x, y],
            false,
        )
    }

    fn sizing() -> SizingInput {
        SizingInput {
            available: Size::new(1000.0, 800.0),
            current_client: [400, 300],
            expected_client: [400, 300],
            current_tile: Size::new(404.0, 304.0),
            decoration: Size::new(4.0, 4.0),
            minimum: [0, 0],
            maximum: [0, 0],
        }
    }

    #[test]
    fn proportional_size_is_resolved_in_tile_units_and_hints_constrain_both_axes() {
        assert_eq!(
            resolve_size_change(Axis::Horizontal, SizeChange::SetProportion(50.0), sizing()),
            [496, 300]
        );
        let input = SizingInput {
            minimum: [600, 0],
            maximum: [500, 250],
            ..sizing()
        };
        assert_eq!(
            resolve_size_change(Axis::Horizontal, SizeChange::SetProportion(50.0), input),
            [600, 250]
        );
    }

    #[test]
    fn size_commands_preserve_other_axis_negotiation_and_saturate_integer_adjustments() {
        let input = SizingInput {
            current_client: [i32::MAX, 300],
            expected_client: [400, 0],
            ..sizing()
        };
        assert_eq!(
            resolve_size_change(Axis::Horizontal, SizeChange::AdjustFixed(i32::MAX), input),
            [100000, 0]
        );
        assert_eq!(
            resolve_size_change(Axis::Horizontal, SizeChange::SetFixed(-100), input),
            [1, 0]
        );
    }

    #[test]
    fn expanded_restore_keeps_unknown_client_size_until_negotiated() {
        assert_eq!(
            restored_client_size(None, true, Some([1200, 900]), [100, 200], [0, 0]),
            [0, 0]
        );
        assert_eq!(
            restored_client_size(None, true, Some([1200, 900]), [300, 200], [300, 0]),
            [300, 0]
        );
        assert_eq!(
            restored_client_size(Some([640, 480]), true, None, [0, 0], [500, 0]),
            [500, 480]
        );
        assert_eq!(
            restored_client_size(None, false, Some([640, 480]), [0, 0], [0, 0]),
            [640, 480]
        );
    }

    #[test]
    fn initial_size_and_client_bounds_apply_decoration_once_and_keep_unspecified_axes_zero() {
        assert_eq!(
            initial_client_size(
                Some(Preset::Proportion(0.5)),
                None,
                Size::new(1001.0, 800.0),
                Size::new(4.0, 4.0)
            ),
            [496, 0]
        );
        assert_eq!(
            initial_client_size(
                Some(Preset::Fixed(500)),
                Some(Preset::Fixed(-100)),
                Size::default(),
                Size::new(4.0, 4.0)
            ),
            [500, 1]
        );
        assert_eq!(
            client_bounds(Size::new(10.0, 20.0), Size::new(100.0, 100.0)),
            [1, 1]
        );
    }

    #[test]
    fn preset_cycle_retains_selection_and_uses_client_or_tile_units_as_requested() {
        let mut policy = FloatingPolicy::new();
        policy.insert(1, placement(10.0, 20.0), 0, true, &[]);
        let presets = [
            Preset::Fixed(400),
            Preset::Proportion(0.5),
            Preset::Fixed(800),
        ];
        assert_eq!(
            policy.cycle_preset(1, Axis::Horizontal, true, presets.into_iter(), sizing()),
            Some([496, 300])
        );
        assert_eq!(policy.preset_indices(0), [Some(1), None]);
        assert_eq!(
            policy.cycle_preset(1, Axis::Horizontal, true, presets.into_iter(), sizing()),
            Some([800, 300])
        );
        assert_eq!(
            policy.cycle_preset(1, Axis::Horizontal, false, presets.into_iter(), sizing()),
            Some([496, 300])
        );
        policy.restore_presets(1, [Some(1), Some(2)]);
        assert_eq!(
            policy.change_size(1, Axis::Horizontal, SizeChange::SetFixed(450), sizing()),
            Some([450, 300])
        );
        assert_eq!(policy.preset_indices(0), [None, Some(2)]);
        assert_eq!(
            policy.members().next().unwrap().requested_client,
            Some([450, 300])
        );
        policy.clear_presets(Axis::Vertical);
        assert_eq!(policy.preset_indices(0), [None, None]);
    }

    #[test]
    fn invalid_or_empty_presets_cannot_panic_or_corrupt_retained_selection() {
        let mut policy = FloatingPolicy::new();
        policy.insert(1, placement(10.0, 20.0), 0, true, &[]);
        policy.restore_presets(1, [Some(usize::MAX), None]);
        let presets = [Preset::Fixed(400), Preset::Fixed(800)];
        assert_eq!(
            policy.cycle_preset(1, Axis::Horizontal, true, presets.into_iter(), sizing()),
            Some([800, 300])
        );
        assert_eq!(policy.preset_indices(0), [Some(1), None]);
        assert!(policy
            .cycle_preset(1, Axis::Horizontal, true, [].into_iter(), sizing())
            .is_none());
        assert_eq!(policy.preset_indices(0), [Some(1), None]);
    }

    #[test]
    fn exhausted_working_area_and_invalid_proportions_emit_finite_positive_changed_sizes() {
        let input = SizingInput {
            available: Size::default(),
            ..sizing()
        };
        assert_eq!(
            resolve_size_change(Axis::Horizontal, SizeChange::AdjustProportion(50.0), input),
            [1, 300]
        );
        assert_eq!(
            resolve_size_change(Axis::Vertical, SizeChange::SetProportion(f64::NAN), input),
            [400, 1]
        );
    }

    #[test]
    fn center_and_default_anchors_use_output_local_working_area() {
        let area = Rect::new(10.0, 20.0, 1000.0, 800.0);
        let size = Size::new(200.0, 100.0);
        assert_eq!(centered_position(area, size), [410.0, 370.0]);
        assert_eq!(
            centered_position(area, Size::new(2000.0, 1000.0)),
            [10.0, 20.0]
        );
        assert_eq!(
            anchored_position(area, size, Anchor::TopRight, [15.0, 30.0]),
            [795.0, 50.0]
        );
        assert_eq!(
            anchored_position(area, size, Anchor::Bottom, [15.0, 30.0]),
            [425.0, 690.0]
        );
        let mut policy = FloatingPolicy::new();
        policy.insert(1, placement(200.0, 300.0), 0, true, &[]);
        assert_eq!(
            policy.position_above(1, Size::new(100.0, 50.0)),
            Some([250.0, 325.0])
        );
        assert_eq!(
            policy.center_window(1).unwrap().current,
            Rect::new(410.0, 370.0, 200.0, 100.0)
        );
    }

    #[test]
    fn revealing_unbounded_windows_returns_pan_intent_without_rebuilding_positions() {
        let mut policy = FloatingPolicy::new();
        policy.insert(1, placement(200.0, 100.0), 0, true, &[]);
        assert!(policy.reveal_delta(1).is_none());
        policy.set_unbounded(true);
        policy.move_to(1, [2000.0, 100.0]);
        let delta = policy.reveal_delta(1).unwrap();
        assert_eq!(delta, [1590.0, -270.0]);
        policy.pan(delta);
        assert_eq!(
            policy.placement(0).rectangle,
            Rect::new(410.0, 370.0, 200.0, 100.0)
        );
        assert!(policy.reveal_delta(1).is_none());
    }

    #[test]
    fn late_resize_commit_preserves_the_opposite_edge_after_interaction_has_ended() {
        let mut policy = FloatingPolicy::new();
        policy.insert(1, placement(500.0, 200.0), 0, true, &[]);
        assert!(policy.resize().is_none());
        let change = policy
            .commit_tile_size(1, Size::new(300.0, 200.0), Some(5))
            .unwrap();
        assert_eq!(change.current, Rect::new(400.0, 100.0, 300.0, 200.0));
        assert_eq!(change.current.right(), change.previous.right());
        assert_eq!(change.current.bottom(), change.previous.bottom());
        assert!(policy.verify_invariants());
    }

    #[test]
    fn shrinking_from_offscreen_edges_keeps_the_original_opposite_edges() {
        let area = Rect::new(0., 0., 1000., 800.);
        for (position, size, edges, expected) in [
            ([-200., 100.], Size::new(100., 300.), 4, [100., 100.]),
            ([100., -150.], Size::new(400., 100.), 1, [100., 50.]),
            ([-200., -150.], Size::new(100., 100.), 5, [100., 50.]),
        ] {
            let mut policy = FloatingPolicy::new();
            policy.insert(
                1,
                Placement::new(area, Size::new(400., 300.), position, false),
                0,
                true,
                &[],
            );
            // Resize metadata can outlive the grab; its old opposite edge is
            // still the anchor even when the new size changes reachability.
            assert!(policy.resize().is_none());
            let change = policy.commit_tile_size(1, size, Some(edges)).unwrap();
            assert_eq!(
                change.current,
                Rect::new(expected[0], expected[1], size.width, size.height),
                "edges = {edges}",
            );
            assert_eq!(change.current.right(), change.previous.right());
            assert_eq!(change.current.bottom(), change.previous.bottom());
            assert!(policy.verify_invariants());
        }
    }

    #[test]
    fn identities_and_focus_survive_reordering_and_active_removal() {
        let mut policy = FloatingPolicy::new();
        policy.insert(10, placement(50.0, 50.0), 0, true, &[]);
        policy.insert(20, placement(300.0, 50.0), 0, false, &[]);
        assert_eq!(policy.active(), Some(10));
        assert_eq!(policy.raise(10, 0), Some(OrderChange { from: 1, to: 0 }));
        assert_eq!(
            policy.members().map(|member| member.id).collect::<Vec<_>>(),
            [10, 20]
        );
        assert!(policy
            .insert(10, placement(0.0, 0.0), 0, true, &[])
            .is_none());
        policy.remove(10);
        assert_eq!(policy.active(), Some(20));
        policy.remove(20);
        assert_eq!(policy.active(), None);
        assert!(policy.verify_invariants());
    }

    #[test]
    fn insertion_never_places_a_child_under_a_known_ancestor() {
        let mut policy = FloatingPolicy::new();
        for id in [1, 2, 3] {
            policy.insert(id, placement(0.0, 0.0), 0, false, &[]);
        }
        assert_eq!(
            policy.insert(4, placement(0.0, 0.0), 3, false, &[2]),
            Some(1)
        );
        assert_eq!(
            policy.members().map(|member| member.id).collect::<Vec<_>>(),
            [3, 4, 2, 1]
        );
    }

    #[test]
    fn mapping_a_parent_late_preserves_the_transient_family_order() {
        let mut policy = FloatingPolicy::new();
        policy.insert(1, placement(0.0, 0.0), 0, true, &[]);
        policy.insert(2, placement(0.0, 0.0), 0, false, &[]);
        policy.insert(3, placement(0.0, 0.0), 0, false, &[]);
        policy.set_parents(2, vec![1]);
        policy.set_parents(3, vec![2]);
        policy.raise(1, 0);
        let changes = policy.raise_descendants(1);
        assert_eq!(changes.len(), 2);
        assert_eq!(
            policy.members().map(|member| member.id).collect::<Vec<_>>(),
            [3, 2, 1]
        );
        assert_eq!(policy.active(), Some(1));
        assert!(policy.verify_invariants());
    }

    #[test]
    fn relative_placement_and_unbounded_canvas_have_distinct_area_resize_behavior() {
        let mut relative = placement(210.0, 180.0);
        let mut canvas = relative;
        canvas.set_unbounded(true);
        let area = Rect::new(30.0, 40.0, 2000.0, 1600.0);
        relative.set_area(area);
        canvas.set_area(area);
        assert_eq!([relative.rectangle.x, relative.rectangle.y], [430.0, 360.0]);
        assert_eq!([canvas.rectangle.x, canvas.rectangle.y], [210.0, 180.0]);
    }

    #[test]
    fn reachable_placement_preserves_relative_intent_and_keeps_oversized_title_reachable() {
        let mut placement = placement(5000.0, -200.0);
        assert_eq!(placement.rectangle.x, 960.0);
        assert_eq!(placement.rectangle.y, -55.0);
        placement.set_size(Size::new(3000.0, 1600.0));
        assert_eq!(placement.rectangle.x, 935.0);
        assert!((placement.rectangle.y + 200.0).abs() < 1e-8);
        placement.set_unbounded(true);
        assert_eq!(placement.rectangle.x, 5000.0);
        assert!((placement.rectangle.y + 200.0).abs() < 1e-8);
    }

    #[test]
    fn directional_selection_uses_centers_and_stacking_breaks_equal_distances() {
        let mut policy = FloatingPolicy::new();
        policy.insert(1, placement(300.0, 200.0), 0, true, &[]);
        policy.insert(2, placement(100.0, 500.0), 0, false, &[]);
        policy.insert(3, placement(100.0, 0.0), 0, false, &[]);
        assert_eq!(policy.focus_directional(Direction::Left), Some(3));
        assert_eq!(policy.focus_extreme(Axis::Vertical, true), Some(2));
        assert!(policy.verify_invariants());
    }

    #[test]
    fn move_commands_are_pure_and_pan_updates_all_target_rectangles() {
        let mut policy = FloatingPolicy::new();
        policy.insert(1, placement(100.0, 100.0), 0, true, &[]);
        let change = policy
            .move_with_changes(
                1,
                PositionChange::SetProportion(50.0),
                PositionChange::AdjustFixed(30.0),
            )
            .unwrap();
        assert_eq!([change.previous.x, change.previous.y], [100.0, 100.0]);
        assert_eq!([change.current.x, change.current.y], [510.0, 130.0]);
        policy.set_unbounded(true);
        policy.pan([1000.0, 1000.0]);
        assert_eq!(
            policy.placement(0).rectangle,
            Rect::new(-490.0, -870.0, 200.0, 100.0)
        );
        assert!(policy.verify_invariants());
    }

    #[test]
    fn interactive_resize_tracks_identity_edges_and_original_size_without_payload_handles() {
        let mut policy = FloatingPolicy::new();
        policy.insert(1, placement(100.0, 100.0), 0, true, &[]);
        assert!(policy.begin_resize(1, Size::new(400.0, 300.0), 5));
        assert!(!policy.begin_resize(1, Size::default(), 8));
        assert_eq!(
            policy.resize_intent(1, [30.0, -20.0]),
            Some(ResizeIntent {
                width: Some(370),
                height: Some(320)
            })
        );
        assert!(policy.resize_intent(2, [30.0, -20.0]).is_none());
        assert_eq!(policy.end_resize(Some(2)), None);
        assert_eq!(policy.end_resize(Some(1)), Some(1));
        assert!(policy.begin_resize(1, Size::new(400.0, 300.0), 8));
        policy.remove(1);
        assert!(policy.resize().is_none());
        assert!(policy.verify_invariants());
    }

    #[test]
    fn invalid_input_stays_finite_and_unknown_identity_never_mutates_the_policy() {
        let mut policy = FloatingPolicy::new();
        let placement = Placement::new(
            Rect::new(f64::NAN, 0.0, 0.0, f64::INFINITY),
            Size::new(f64::INFINITY, -1.0),
            [f64::NAN, f64::INFINITY],
            true,
        );
        policy.insert(1, placement, usize::MAX, true, &[]);
        assert!(policy.move_to(2, [0.0; 2]).is_none());
        policy.pan([f64::NAN, 1.0]);
        assert!(policy.verify_invariants());
    }
}
