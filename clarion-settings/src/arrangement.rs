//! Monitor arrangement: draggable placements, snapping, and re-flow.
//!
//! The arrangement view never writes configuration. It produces positions for
//! the display preview transaction, which remains the only writer.

use clarion_config::output::OutputName;
use clarion_config::scale::{closest_representable_scale, guess_monitor_scale};
use clarion_ipc::{Output, Transform};
use eframe::egui::{self, Align2, CursorIcon, Rect, Sense, Stroke, Ui};

use crate::output_settings::OutputSettings;
use crate::theme::colors as palette;

/// Screen-space inset around the arrangement canvas.
const CANVAS_PADDING: f32 = 16.;
/// How close an edge must be, in screen points, before it snaps.
const SNAP_DISTANCE: f32 = 8.;

#[derive(Clone, Copy, Debug, PartialEq)]
struct MonitorRect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

impl MonitorRect {
    fn right(self) -> f32 {
        self.x + self.width
    }

    fn bottom(self) -> f32 {
        self.y + self.height
    }

    fn overlaps(self, other: Self) -> bool {
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }
}

/// One output as it exists before the arrangement is edited.
#[derive(Clone, Debug)]
pub(super) struct PlacementSeed {
    pub name: OutputName,
    pub size: (i32, i32),
    pub actual: (i32, i32),
}

#[derive(Clone, Debug)]
struct Placement {
    name: OutputName,
    size: (i32, i32),
    actual: (i32, i32),
    pos: (f32, f32),
}

#[derive(Clone, Debug)]
struct Drag {
    index: usize,
    /// Where inside the monitor the pointer grabbed it, in logical pixels.
    grab: (f32, f32),
}

/// The outcome of one rendered frame.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Outcome {
    /// A drag moved the monitor and the draft now differs from the layout.
    pub changed: bool,
    /// The pointer was released after an actual drag.
    pub released: bool,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Arrangement {
    placements: Vec<Placement>,
    drag: Option<Drag>,
}

impl Arrangement {
    pub(super) fn new(seeds: Vec<PlacementSeed>) -> Self {
        Self {
            placements: seeds
                .into_iter()
                .map(|seed| Placement {
                    name: seed.name,
                    size: seed.size,
                    actual: seed.actual,
                    pos: (seed.actual.0 as f32, seed.actual.1 as f32),
                })
                .collect(),
            drag: None,
        }
    }

    /// Whether this arrangement still describes the same physical outputs.
    pub(super) fn same_outputs(&self, seeds: &[PlacementSeed]) -> bool {
        self.placements.len() == seeds.len()
            && self
                .placements
                .iter()
                .zip(seeds)
                .all(|(placement, seed)| placement.name.connector == seed.name.connector)
    }

    /// Re-read sizes and positions after an external layout change while the
    /// draft is clean. Keep the draft's own positions if it is dirty.
    pub(super) fn refresh(&mut self, seeds: &[PlacementSeed]) {
        for placement in &mut self.placements {
            let Some(seed) = seeds
                .iter()
                .find(|seed| seed.name.connector == placement.name.connector)
            else {
                continue;
            };
            placement.size = seed.size;
            placement.actual = seed.actual;
            placement.pos = (seed.actual.0 as f32, seed.actual.1 as f32);
        }
        self.drag = None;
    }

    pub(super) fn dirty(&self) -> bool {
        self.placements
            .iter()
            .any(|placement| placement.rounded() != placement.actual)
    }

    pub(super) fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Applies a new logical size to one output and re-flows the whole draft.
    ///
    /// Used when a scale, rotation or mode change would otherwise leave
    /// monitors overlapping or with gaps. Positions are written for every
    /// output so the compositor cannot place an unconfigured one elsewhere.
    pub(super) fn reflow_for(
        &mut self,
        resize: Option<(&OutputName, (i32, i32))>,
    ) -> Vec<(OutputName, (i32, i32))> {
        if let Some((target, size)) = resize {
            let items: Vec<ReflowItem> = self
                .placements
                .iter()
                .map(|placement| ReflowItem {
                    name: placement.name.clone(),
                    old: placement.rounded(),
                    old_size: placement.size,
                    size: if placement.name.connector == target.connector {
                        size
                    } else {
                        placement.size
                    },
                })
                .collect();
            let laid_out = reflow(&items);
            for placement in &mut self.placements {
                if placement.name.connector == target.connector {
                    placement.size = size;
                }
                if let Some((_, position)) = laid_out
                    .iter()
                    .find(|(name, _)| name.connector == placement.name.connector)
                {
                    placement.pos = (position.0 as f32, position.1 as f32);
                }
            }
        }
        self.positions()
    }

    /// Every position that must be written to keep this arrangement exact.
    pub(super) fn positions(&self) -> Vec<(OutputName, (i32, i32))> {
        self.placements
            .iter()
            .map(|placement| (placement.name.clone(), placement.rounded()))
            .collect()
    }

    /// Draws the arrangement and applies pointer interaction.
    ///
    /// `editable` allows dragging, `selectable` allows plain clicks to select
    /// a monitor. They differ while an arrangement change is waiting for its
    /// preview: dragging must keep working, selecting another monitor must not
    /// replace the pending draft.
    pub(super) fn show(
        &mut self,
        ui: &mut Ui,
        selected: &mut Option<String>,
        editable: bool,
        selectable: bool,
    ) -> Outcome {
        let (canvas, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 180.), Sense::hover());
        ui.painter().rect(
            canvas,
            4.,
            palette::surface_hover(),
            Stroke::new(1.0_f32, palette::border()),
        );
        if self.placements.is_empty() {
            return Outcome::default();
        }

        let bounds = self.bounds(None);
        let (origin, scale) = fit(canvas, bounds);
        let pointer = ui.ctx().pointer_interact_pos();
        let mut changed = false;
        let mut released = false;

        let ids: Vec<usize> = (0..self.placements.len()).collect();
        for index in ids {
            let rect = monitor_rect(
                origin,
                scale,
                bounds,
                self.placements[index].pos,
                self.placements[index].size,
            );
            let sense = match (editable, selectable) {
                (true, true) => Sense::click_and_drag(),
                (true, false) => Sense::drag(),
                (false, true) => Sense::click(),
                (false, false) => Sense::hover(),
            };
            let response = ui.interact(
                rect,
                ui.id()
                    .with(("arranged-output", &self.placements[index].name.connector)),
                sense,
            );
            let name = self.placements[index].name.connector.clone();
            if selectable && response.clicked() {
                *selected = Some(name.clone());
            }
            if editable && response.drag_started() {
                if let Some(pointer) = pointer {
                    let world = screen_to_world(pointer, origin, scale, bounds);
                    let position = self.placements[index].pos;
                    self.drag = Some(Drag {
                        index,
                        grab: (world.0 - position.0, world.1 - position.1),
                    });
                    ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
                }
            }
            let drag_index = self.drag.as_ref().map(|drag| drag.index);
            if drag_index == Some(index) {
                if let Some(pointer) = pointer {
                    if response.dragged() {
                        changed |= self.drag_to(canvas, pointer);
                        ui.ctx().request_repaint();
                    }
                }
            }
            if response.drag_stopped() && drag_index == Some(index) {
                self.drag = None;
                released = true;
            }
            if response.hovered() && editable {
                ui.ctx()
                    .set_cursor_icon(if self.drag.is_some() {
                        CursorIcon::Grabbing
                    } else {
                        CursorIcon::Grab
                    });
            }
        }

        // Paint after interaction so a dragged monitor follows this frame's
        // pointer instead of lagging one frame behind.
        let bounds = self.bounds(None);
        let (origin, scale) = fit(canvas, bounds);
        let drag_index = self.drag.as_ref().map(|drag| drag.index);
        for (index, placement) in self.placements.iter().enumerate() {
            let rect = monitor_rect(origin, scale, bounds, placement.pos, placement.size);
            let selected = selected.as_ref() == Some(&placement.name.connector);
            let highlighted = drag_index == Some(index) || selected;
            ui.painter().rect(
                rect.shrink(1.),
                2.,
                if highlighted {
                    palette::accent()
                } else {
                    palette::muted()
                },
                Stroke::new(1.0_f32, palette::border()),
            );
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                &placement.name.connector,
                egui::FontId::proportional(12.),
                palette::on_accent(),
            );
        }

        Outcome { changed, released }
    }

    fn bounds(&self, override_pos: Option<(usize, (f32, f32))>) -> MonitorRect {
        let mut bounds: Option<MonitorRect> = None;
        for (index, placement) in self.placements.iter().enumerate() {
            let pos = override_pos
                .filter(|(override_index, _)| *override_index == index)
                .map_or(placement.pos, |(_, pos)| pos);
            let rect = MonitorRect {
                x: pos.0,
                y: pos.1,
                width: placement.size.0 as f32,
                height: placement.size.1 as f32,
            };
            bounds = Some(match bounds {
                None => rect,
                Some(current) => {
                    let min_x = current.x.min(rect.x);
                    let min_y = current.y.min(rect.y);
                    let max_x = current.right().max(rect.right());
                    let max_y = current.bottom().max(rect.bottom());
                    MonitorRect {
                        x: min_x,
                        y: min_y,
                        width: max_x - min_x,
                        height: max_y - min_y,
                    }
                }
            });
        }
        bounds.unwrap_or(MonitorRect {
            x: 0.,
            y: 0.,
            width: 1.,
            height: 1.,
        })
    }

    fn drag_to(&mut self, canvas: Rect, pointer: egui::Pos2) -> bool {
        let Some(drag) = &self.drag else {
            return false;
        };
        let index = drag.index;
        let grab = drag.grab;
        let mut candidate = self.placements[index].pos;
        for _ in 0..4 {
            let bounds = self.bounds(Some((index, candidate)));
            let (origin, scale) = fit(canvas, bounds);
            let world = screen_to_world(pointer, origin, scale, bounds);
            let target = (world.0 - grab.0, world.1 - grab.1);
            let others = self.other_rects(index, None);
            let snapped = snap(
                target,
                self.placements[index].size,
                &others,
                SNAP_DISTANCE / scale,
            );
            candidate = settle(snapped, self.placements[index].size, &others)
                .unwrap_or(self.placements[index].pos);
        }
        let changed = candidate != self.placements[index].pos;
        self.placements[index].pos = candidate;
        changed
    }

    fn other_rects(
        &self,
        skip: usize,
        override_pos: Option<(usize, (f32, f32))>,
    ) -> Vec<MonitorRect> {
        self.placements
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != skip)
            .map(|(index, placement)| {
                let pos = override_pos
                    .filter(|(override_index, _)| *override_index == index)
                    .map_or(placement.pos, |(_, pos)| pos);
                MonitorRect {
                    x: pos.0,
                    y: pos.1,
                    width: placement.size.0 as f32,
                    height: placement.size.1 as f32,
                }
            })
            .collect()
    }
}

impl Placement {
    fn rounded(&self) -> (i32, i32) {
        (self.pos.0.round() as i32, self.pos.1.round() as i32)
    }
}

fn fit(canvas: Rect, bounds: MonitorRect) -> (egui::Pos2, f32) {
    let available = canvas.shrink(CANVAS_PADDING);
    let width = available.width().max(1.);
    let height = available.height().max(1.);
    let scale = (width / bounds.width.max(1.)).min(height / bounds.height.max(1.));
    let scale = scale.max(0.001);
    let origin = canvas.center() - egui::vec2(bounds.width, bounds.height) * scale / 2.;
    (origin, scale)
}

fn monitor_rect(
    origin: egui::Pos2,
    scale: f32,
    bounds: MonitorRect,
    pos: (f32, f32),
    size: (i32, i32),
) -> Rect {
    let offset = egui::vec2(pos.0 - bounds.x, pos.1 - bounds.y) * scale;
    Rect::from_min_size(
        origin + offset,
        egui::vec2(size.0 as f32, size.1 as f32) * scale,
    )
}

fn screen_to_world(
    pointer: egui::Pos2,
    origin: egui::Pos2,
    scale: f32,
    bounds: MonitorRect,
) -> (f32, f32) {
    let local = (pointer - origin) / scale;
    (local.x + bounds.x, local.y + bounds.y)
}

fn snap(
    pos: (f32, f32),
    size: (i32, i32),
    others: &[MonitorRect],
    threshold: f32,
) -> (f32, f32) {
    let width = size.0 as f32;
    let height = size.1 as f32;
    let mut x_targets = vec![0.];
    let mut y_targets = vec![0.];
    for other in others {
        x_targets.extend([other.x, other.right(), other.x - width, other.right() - width]);
        y_targets.extend([other.y, other.bottom(), other.y - height, other.bottom() - height]);
    }
    (
        snap_axis(pos.0, &x_targets, threshold),
        snap_axis(pos.1, &y_targets, threshold),
    )
}

fn snap_axis(value: f32, targets: &[f32], threshold: f32) -> f32 {
    let mut best = threshold;
    let mut snapped = value;
    for &target in targets {
        let distance = (target - value).abs();
        if distance < best {
            best = distance;
            snapped = target;
        }
    }
    snapped
}

/// Pushes a rectangle out of its neighbours, always taking the smallest move.
///
/// Returns `None` if no stable non-overlapping position was found, in which
/// case the caller keeps the last valid position.
fn settle(pos: (f32, f32), size: (i32, i32), others: &[MonitorRect]) -> Option<(f32, f32)> {
    let width = size.0 as f32;
    let height = size.1 as f32;
    let mut position = pos;
    for _ in 0..12 {
        let rect = MonitorRect {
            x: position.0,
            y: position.1,
            width,
            height,
        };
        let Some(other) = others.iter().find(|other| rect.overlaps(**other)) else {
            return Some(position);
        };
        let options = [
            (other.x - width, position.1, (position.0 - (other.x - width)).abs()),
            (other.right(), position.1, (position.0 - other.right()).abs()),
            (position.0, other.y - height, (position.1 - (other.y - height)).abs()),
            (position.0, other.bottom(), (position.1 - other.bottom()).abs()),
        ];
        let (x, y, _) = options
            .into_iter()
            .min_by(|a, b| a.2.total_cmp(&b.2))?;
        position = (x, y);
    }
    None
}

/// A previous placement plus the size an output will have after a geometry
/// change.
#[derive(Clone, Debug)]
pub(super) struct ReflowItem {
    pub name: OutputName,
    pub old: (i32, i32),
    pub old_size: (i32, i32),
    pub size: (i32, i32),
}

/// Re-flows a previous arrangement and returns a compact, usable layout.
///
/// Rows are recovered from the old vertical overlap, each row keeps its
/// left-to-right order and rows keep their top-to-bottom order. New sizes can
/// grow or swap axes freely; the layout never overlaps and has no gaps.
pub(super) fn reflow(items: &[ReflowItem]) -> Vec<(OutputName, (i32, i32))> {
    if items.is_empty() {
        return Vec::new();
    }
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by_key(|&index| (items[index].old.1, items[index].old.0));

    struct Row {
        top: i32,
        bottom: i32,
        items: Vec<usize>,
    }
    let mut rows: Vec<Row> = Vec::new();
    for index in order {
        let item = &items[index];
        let top = item.old.1;
        let bottom = item.old.1 + item.old_size.1;
        let mut best: Option<(usize, i32)> = None;
        for (row_index, row) in rows.iter().enumerate() {
            let overlap = bottom.min(row.bottom) - top.max(row.top);
            if overlap > 0 && best.is_none_or(|(_, best)| overlap > best) {
                best = Some((row_index, overlap));
            }
        }
        match best {
            Some((row_index, _)) => {
                let row = &mut rows[row_index];
                row.top = row.top.min(top);
                row.bottom = row.bottom.max(bottom);
                row.items.push(index);
            }
            None => rows.push(Row {
                top,
                bottom,
                items: vec![index],
            }),
        }
    }
    rows.sort_by_key(|row| row.top);

    let mut result = Vec::with_capacity(items.len());
    let mut y = 0;
    for row in rows {
        let mut ordered = row.items;
        ordered.sort_by_key(|&index| items[index].old.0);
        let mut x = 0;
        let mut height = 0;
        for index in ordered {
            result.push((items[index].name.clone(), (x, y)));
            x += items[index].size.0;
            height = height.max(items[index].size.1);
        }
        y += height;
    }
    result
}

/// The logical size an output will have once `after` replaces `before`.
///
/// Returns `None` when the output is disabled, not currently mapped, or its
/// geometry does not change.
pub(super) fn changed_size(
    output: &Output,
    before: &OutputSettings,
    after: &OutputSettings,
) -> Option<(i32, i32)> {
    if !after.enabled || !before.enabled {
        return None;
    }
    if before.scale == after.scale
        && before.transform == after.transform
        && before.mode == after.mode
        && before.custom_modeline == after.custom_modeline
    {
        return None;
    }
    let logical = output.logical.as_ref()?;
    let mode_changed = after.mode != before.mode || before.custom_modeline != after.custom_modeline;
    let mode = match after.mode {
        Some(mode) => mode,
        None if mode_changed => preferred_mode(output)?,
        None => current_mode(output)?,
    };
    let scale = match after.scale {
        Some(scale) => scale,
        None if after.scale != before.scale || mode_changed => {
            let physical = output
                .physical_size
                .map_or((0, 0), |(width, height)| {
                    (width as i32, height as i32)
                });
            guess_monitor_scale(physical, (i32::from(mode.width), i32::from(mode.height)))
        }
        None => logical.scale,
    };
    let scale = closest_representable_scale(scale.clamp(0.1, 10.));
    let mut width = f64::from(mode.width) / scale;
    let mut height = f64::from(mode.height) / scale;
    let actual_swaps = swaps(logical.transform);
    let new_swaps = actual_swaps ^ swaps(before.transform) ^ swaps(after.transform);
    if new_swaps {
        std::mem::swap(&mut width, &mut height);
    }
    Some((width.round() as i32, height.round() as i32))
}

fn current_mode(output: &Output) -> Option<clarion_ipc::ConfiguredMode> {
    let mode = output.modes.get(output.current_mode?)?;
    Some(configured_mode(mode))
}

fn preferred_mode(output: &Output) -> Option<clarion_ipc::ConfiguredMode> {
    output
        .modes
        .iter()
        .find(|mode| mode.is_preferred)
        .or_else(|| output.current_mode.and_then(|index| output.modes.get(index)))
        .map(configured_mode)
}

fn configured_mode(mode: &clarion_ipc::Mode) -> clarion_ipc::ConfiguredMode {
    clarion_ipc::ConfiguredMode {
        width: mode.width,
        height: mode.height,
        refresh: None,
    }
}

fn swaps(transform: Transform) -> bool {
    matches!(
        transform,
        Transform::_90 | Transform::_270 | Transform::Flipped90 | Transform::Flipped270
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(connector: &str) -> OutputName {
        OutputName {
            connector: connector.into(),
            make: None,
            model: None,
            serial: None,
        }
    }

    fn rect(x: f32, y: f32, width: f32, height: f32) -> MonitorRect {
        MonitorRect {
            x,
            y,
            width,
            height,
        }
    }

    fn placed(positions: &[(OutputName, (i32, i32))]) -> Vec<(String, (i32, i32))> {
        positions
            .iter()
            .map(|(name, position)| (name.connector.clone(), *position))
            .collect()
    }

    fn expected(items: &[(&str, (i32, i32))]) -> Vec<(String, (i32, i32))> {
        items
            .iter()
            .map(|(name, position)| ((*name).to_owned(), *position))
            .collect()
    }

    fn union(items: &[MonitorRect]) -> MonitorRect {
        let mut bounds = items[0];
        for item in &items[1..] {
            let min_x = bounds.x.min(item.x);
            let min_y = bounds.y.min(item.y);
            let max_x = bounds.right().max(item.right());
            let max_y = bounds.bottom().max(item.bottom());
            bounds = MonitorRect {
                x: min_x,
                y: min_y,
                width: max_x - min_x,
                height: max_y - min_y,
            };
        }
        bounds
    }

    #[test]
    fn snapping_prefers_the_nearest_edge() {
        let others = [rect(0., 0., 1920., 1080.)];
        let snapped = snap((1908., 4.), (1920, 1080), &others, 20.);
        assert_eq!(snapped, (1920., 0.));
    }

    #[test]
    fn settling_pushes_overlaps_to_the_closest_side() {
        let others = [rect(0., 0., 1920., 1080.)];
        let settled = settle((100., 40.), (1280, 720), &others).unwrap();
        assert!(!rect(settled.0, settled.1, 1280., 720.).overlaps(others[0]));
    }

    #[test]
    fn settling_never_returns_an_overlap() {
        let others = [rect(0., 0., 1920., 1080.), rect(1920., 0., 2560., 1440.)];
        let settled = settle((1800., 900.), (1920, 1080), &others).unwrap();
        assert!(!rect(settled.0, settled.1, 1920., 1080.).overlaps(others[0]));
        assert!(!rect(settled.0, settled.1, 1920., 1080.).overlaps(others[1]));
    }

    #[test]
    fn reflow_keeps_side_by_side_order_when_a_monitor_grows() {
        let items = vec![
            ReflowItem {
                name: name("A"),
                old: (0, 0),
                old_size: (1920, 1080),
                size: (1280, 720),
            },
            ReflowItem {
                name: name("B"),
                old: (1920, 0),
                old_size: (1920, 1080),
                size: (1920, 1080),
            },
        ];
        assert_eq!(
            placed(&reflow(&items)),
            expected(&[("A", (0, 0)), ("B", (1280, 0))])
        );
    }

    #[test]
    fn reflow_keeps_stacked_order_when_a_monitor_gets_taller() {
        let items = vec![
            ReflowItem {
                name: name("A"),
                old: (0, 0),
                old_size: (1920, 1080),
                size: (1920, 1080),
            },
            ReflowItem {
                name: name("B"),
                old: (0, 1080),
                old_size: (1920, 1080),
                size: (1920, 1920),
            },
        ];
        assert_eq!(
            placed(&reflow(&items)),
            expected(&[("A", (0, 0)), ("B", (0, 1080))])
        );
    }

    #[test]
    fn reflow_preserves_a_two_by_two_grid() {
        let items = vec![
            ReflowItem {
                name: name("A"),
                old: (0, 0),
                old_size: (1920, 1080),
                size: (1000, 800),
            },
            ReflowItem {
                name: name("B"),
                old: (1920, 0),
                old_size: (1920, 1080),
                size: (1000, 800),
            },
            ReflowItem {
                name: name("C"),
                old: (0, 1080),
                old_size: (1920, 1080),
                size: (900, 700),
            },
            ReflowItem {
                name: name("D"),
                old: (1920, 1080),
                old_size: (1920, 1080),
                size: (1100, 900),
            },
        ];
        let positions = reflow(&items);
        assert_eq!(
            placed(&positions),
            expected(&[
                ("A", (0, 0)),
                ("B", (1000, 0)),
                ("C", (0, 800)),
                ("D", (900, 800)),
            ])
        );
    }

    #[test]
    fn arrangement_reports_dirty_only_after_a_real_move() {
        let seeds = vec![PlacementSeed {
            name: name("A"),
            size: (1920, 1080),
            actual: (0, 0),
        }];
        let arrangement = Arrangement::new(seeds);
        assert!(!arrangement.dirty());
        assert_eq!(
            placed(&arrangement.positions()),
            expected(&[("A", (0, 0))])
        );
    }

    #[test]
    fn reflow_for_updates_sizes_and_keeps_the_arrangement_usable() {
        let mut arrangement = Arrangement::new(vec![
            PlacementSeed {
                name: name("A"),
                size: (1920, 1080),
                actual: (0, 0),
            },
            PlacementSeed {
                name: name("B"),
                size: (1920, 1080),
                actual: (1920, 0),
            },
        ]);
        let positions = arrangement.reflow_for(Some((&name("A"), (1280, 720))));
        assert_eq!(
            placed(&positions),
            expected(&[("A", (0, 0)), ("B", (1280, 0))])
        );
        assert!(arrangement.dirty());
        // The next preview recomputes the same compact layout from the draft.
        assert_eq!(
            placed(&arrangement.reflow_for(None)),
            expected(&[("A", (0, 0)), ("B", (1280, 0))])
        );
    }

    #[test]
    fn fitted_canvas_contains_every_monitor() {
        let bounds = union(&[
            rect(0., 0., 3840., 1080.),
            rect(3840., 0., 1080., 1920.),
        ]);
        let canvas = Rect::from_min_size(egui::pos2(0., 0.), egui::vec2(668., 180.));
        let (origin, scale) = fit(canvas, bounds);
        let area = canvas.shrink(CANVAS_PADDING);
        assert!(origin.x >= area.left() - 1.);
        assert!(origin.y >= area.top() - 1.);
        assert!(origin.x + bounds.width * scale <= area.right() + 1.);
        assert!(origin.y + bounds.height * scale <= area.bottom() + 1.);
    }

    fn display() -> Output {
        serde_json::from_value(serde_json::json!({
            "name": "DP-1",
            "make": "Fixture",
            "model": "Panel",
            "serial": null,
            "physical_size": [600, 340],
            "modes": [{
                "width": 1920,
                "height": 1080,
                "refresh_rate": 60000,
                "is_preferred": true
            }],
            "current_mode": 0,
            "is_custom_mode": false,
            "vrr_supported": false,
            "vrr_enabled": false,
            "hdr_active": false,
            "logical": {
                "x": 0,
                "y": 0,
                "width": 1920,
                "height": 1080,
                "scale": 1.0,
                "transform": "Normal"
            }
        }))
        .unwrap()
    }

    #[test]
    fn geometry_change_computes_the_new_logical_size() {
        let output = display();
        let before = OutputSettings::default();
        let mut after = before.clone();
        after.scale = Some(2.);
        assert_eq!(changed_size(&output, &before, &after), Some((960, 540)));

        let mut after = before.clone();
        after.transform = Transform::_90;
        assert_eq!(changed_size(&output, &before, &after), Some((1080, 1920)));

        // Explicit scale 1 -> automatic. This panel guesses 1x, so the size
        // returns to the mode's logical size.
        let before = OutputSettings {
            scale: Some(1.),
            ..OutputSettings::default()
        };
        let mut after = before.clone();
        after.scale = None;
        assert_eq!(changed_size(&output, &before, &after), Some((1920, 1080)));
    }

    #[test]
    fn unchanged_geometry_never_reflows() {
        let output = display();
        let before = OutputSettings::default();
        assert_eq!(changed_size(&output, &before, &before), None);
        let mut after = before.clone();
        after.hdr = !before.hdr;
        assert_eq!(changed_size(&output, &before, &after), None);
    }
}
