// SPDX-License-Identifier: GPL-3.0-or-later
//! Clock-independent Dock geometry and finite animation samples.
//!
//! Raised-cosine magnification, the critical response, and the launch bounce
//! below are Clarion tuning choices. The available Big Sur evidence does not
//! establish these as Apple's magnification, bounce, or autohide constants.
//! Callers sample at their frame clock and request another frame only while a
//! sample is active; no timer, platform object, or renderer lives in this module.

use std::f32::consts::PI;
use std::time::Duration;

const SHADOW: f32 = 12.;
/// Vertical room reserved for the hover tooltip on the side away from the
/// screen edge. The label is 26 points tall and clears the magnified icon by
/// the tooltip gap, so this band fits both plus a little margin.
const TOOLTIP_BAND: f32 = 40.;
/// Cross-axis room reserved on a vertical Dock for a hover tooltip shown beside
/// the tray. It fits the widest label plus its gap and margin.
const TOOLTIP_REACH: f32 = 236.;
const MAGNIFICATION_SUPPORT: f32 = 2.5;
const RESPONSE_FREQUENCY: f64 = 26.;
const POSITION_EPSILON: f64 = 0.001;

/// The idle tray keeps at most this share of the output's main axis. The hover
/// magnification may briefly exceed it; the resting layout never does.
pub const LENGTH_SHARE: f32 = 0.9;
/// The smallest icon the Dock shrinks to before it folds applications away.
pub const MIN_ICON_SIZE: f32 = 32.;
/// The largest icon the Dock draws.
pub const MAX_ICON_SIZE: f32 = 64.;
/// The discrete icon steps, matching the sizes an icon theme ships. A crowded
/// Dock steps down the ladder one rung at a time instead of shrinking smoothly,
/// so the icons keep their native assets: 64 -> 48 -> 32.
pub const ICON_STEPS: [f32; 3] = [64., 48., 32.];
/// Movement along the row that turns a press into a reorder drag.
pub const DRAG_THRESHOLD: f32 = 4.;

/// Diameter of the running indicator dot. It stays small so a shrunk icon is
/// not dominated by the dot; the two steps keep the dot legible but subordinate.
pub fn indicator_diameter(icon: f32) -> f32 {
    if icon < 40. { 4. } else { 5. }
}

/// Separation between two adjacent icons. It follows the icon size so a shrunk
/// Dock does not turn into a row of islands.
pub fn row_gap(icon: f32) -> f32 {
    (icon * 0.25).clamp(6., 16.)
}

/// Clearance between the icon and the tray edge across the row. The running
/// indicator lives in this band on the screen-facing side, so the band also
/// sets the minimum clearance.
pub fn cross_clearance(icon: f32) -> f32 {
    (icon / 3.)
        .max(indicator_diameter(icon) + 4.)
        .min(20.)
}

/// Clearance between the first or last icon and the tray end along the row. It
/// is deliberately wider than the cross clearance so the row has breathing
/// room at both ends.
pub fn end_gap(icon: f32) -> f32 {
    (cross_clearance(icon) + 6.).max(14.)
}

/// Outer corner radius of the tray. A fixed radius would swallow the end gap
/// once the icons shrink, so it follows the icon size. The result is larger
/// than a hairline rounding so the tray reads as a rounded capsule.
pub fn corner_radius(icon: f32) -> f32 {
    (icon * 0.5).clamp(18., 34.)
}

/// Extra clearance inserted on each side of a separator line. It is additional
/// to the normal row gap and keeps the two adjacent icons visually grouped.
pub fn separator_clearance(icon: f32) -> f32 {
    (row_gap(icon) * 1.25).clamp(12., 18.)
}

/// Idle tray length for `count` slots: the icons, the gaps between them, and
/// the clearance at both ends.
pub fn tray_length(count: usize, icon: f32) -> f32 {
    if count == 0 {
        return 0.;
    }
    let gap = row_gap(icon);
    count as f32 * icon + (count as f32 - 1.) * gap + 2. * end_gap(icon)
}

/// Snap an arbitrary requested icon size down to the nearest ladder step. A
/// preference between two steps (for example 56) resolves to the step at or
/// below it (48), so the Dock only ever draws native icon assets.
pub fn snap_icon_step(size: f32) -> f32 {
    let size = finite(size, 48.);
    let mut chosen = ICON_STEPS[ICON_STEPS.len() - 1];
    for step in ICON_STEPS {
        if step <= size + 0.001 {
            chosen = step;
            break;
        }
    }
    chosen
}

/// Largest discrete icon step whose tray still fits `count` slots inside
/// `limit`. The Dock walks the `ICON_STEPS` ladder rather than shrinking
/// smoothly, so a growing row drops from 64 to 48 to 32 and only then folds
/// applications into the overflow tile.
fn fit_icon_size(preferred: f32, count: usize, separators: usize, limit: f32) -> f32 {
    let ceiling = snap_icon_step(preferred);
    for step in ICON_STEPS {
        if step <= ceiling + 0.001
            && tray_length(count, step)
                + separators as f32 * separator_clearance(step)
                <= limit
        {
            return step;
        }
    }
    // Nothing on the ladder fits; the caller folds the remainder into overflow.
    MIN_ICON_SIZE.min(ceiling)
}

/// Split the non-pinned slots between running and recent applications.
/// Running applications win when even they do not fit; a recent application is
/// never allowed to hide one that is still open.
fn fit_non_pinned(running: usize, recent: usize, room: usize) -> (usize, usize) {
    let running = running.min(room);
    let recent = recent.min(room - running);
    (running, recent)
}

/// Number of slots that fit inside `limit` at the given icon size, leaving room
/// for the maximum number of separator clearances.
fn slot_capacity(icon: f32, separators: usize, limit: f32) -> usize {
    let gap = row_gap(icon);
    let span = icon + gap;
    if span <= 0. {
        return 0;
    }
    let room =
        limit - 2. * end_gap(icon) + gap - separators as f32 * separator_clearance(icon);
    (room / span).floor().max(0.) as usize
}

fn separator_slots(
    launcher: usize,
    visible_pinned: usize,
    visible_running: usize,
    visible_recent: usize,
    visible_apps: usize,
    hidden_apps: usize,
    folder_count: usize,
    slot_count: usize,
) -> Vec<usize> {
    let mut slots = Vec::new();
    if visible_recent > 0 {
        slots.push(launcher + visible_pinned + visible_running);
    }
    if folder_count > 0 {
        slots.push(launcher + visible_apps + usize::from(hidden_apps > 0));
    } else if slot_count > 0 {
        slots.push(slot_count - 1);
    }
    slots.sort_unstable();
    slots.dedup();
    slots
}

/// A stable surface budget; hover and bounce do not resize the Wayland surface.
#[derive(Clone, Debug, PartialEq)]
pub struct DockMetrics {
    /// Idle tray extent along the row/column.
    pub length: f32,
    /// Idle tray extent perpendicular to the row/column.
    pub thickness: f32,
    pub surface_width: f32,
    pub surface_height: f32,
    /// Surface-local leading edge of the idle tray.
    pub main_padding: f32,
    /// Empty space on the side facing away from the screen edge.
    pub cross_padding: f32,
    /// Clearance between the icon and the tray edge across the row.
    pub inset: f32,
    /// Clearance between the first or last icon and the tray end.
    pub end_gap: f32,
    /// Outer corner radius of the tray.
    pub radius: f32,
    pub item: f32,
    pub icon_size: f32,
    pub scale: f32,
    pub growth: f32,
    pub bounce_amplitude: f32,
    pub visible_apps: usize,
    /// Extra clearance inserted on both sides of each separator line.
    pub separator_extra: f32,
    /// Slot indices preceded by a separator line.
    pub separator_before: Vec<usize>,
    /// Number of leading utility slots (the launcher when shown).
    pub launcher: usize,
    /// Pinned folders kept between the overflow tile and the trash.
    pub folder_count: usize,
    /// Pinned applications kept in the leading group.
    pub visible_pinned: usize,
    /// Running applications kept in the middle group.
    pub visible_running: usize,
    /// Recent applications kept in the trailing group.
    pub visible_recent: usize,
    /// Pinned applications folded into the overflow tile.
    pub hidden_apps: usize,
    pub slot_count: usize,
    /// Slots that fit at the chosen icon size, utilities included.
    pub capacity: usize,
    /// The overflow tile plus the trash, and the launcher when shown.
    pub utilities: usize,
    /// The surface extent along the main axis, stored so a re-split can recenter
    /// the tray without knowing the Dock orientation.
    pub surface_main: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slot {
    /// Surface-local leading edge of the icon (not the enclosing hit cell).
    pub origin_main: f32,
    pub size: f32,
    /// Growth away from the screen edge; keep the icon's edge baseline fixed.
    pub lift: f32,
    /// Enclosing cell extent, including the unchanged 12-point separation.
    pub extent: f32,
}

/// What a Dock slot paints. Slots run from the screen-facing edge towards the
/// trash: launcher, visible apps, overflow, folders, trash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockSlotKind {
    Launcher,
    App(usize),
    Overflow,
    Folder(usize),
    Trash,
}

pub fn dock_slot_kind(metrics: &DockMetrics, launcher: usize, slot: usize) -> DockSlotKind {
    if launcher == 1 && slot == 0 {
        return DockSlotKind::Launcher;
    }
    let app_start = launcher;
    let app_end = app_start + metrics.visible_apps;
    if slot < app_end {
        return DockSlotKind::App(slot - app_start);
    }
    let mut next = app_end;
    if metrics.hidden_apps > 0 {
        if slot == next {
            return DockSlotKind::Overflow;
        }
        next += 1;
    }
    let folder_end = next + metrics.folder_count;
    if slot < folder_end {
        return DockSlotKind::Folder(slot - next);
    }
    DockSlotKind::Trash
}

fn finite(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        fallback
    }
}

impl DockMetrics {
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        app_count: usize,
        icon_size: f32,
        scale: f32,
        magnification_enabled: bool,
        show_launcher: bool,
        vertical: bool,
        output_width: f32,
        output_height: f32,
    ) -> Self {
        Self::with_folders(
            app_count,
            icon_size,
            scale,
            magnification_enabled,
            show_launcher,
            0,
            vertical,
            output_width,
            output_height,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_folders(
        app_count: usize,
        icon_size: f32,
        scale: f32,
        magnification_enabled: bool,
        show_launcher: bool,
        folder_count: usize,
        vertical: bool,
        output_width: f32,
        output_height: f32,
    ) -> Self {
        let width = finite(output_width, 1200.).max(1.);
        let height = finite(output_height, 800.).max(1.);
        let (output_main, output_cross) = if vertical {
            (height, width)
        } else {
            (width, height)
        };
        // Eight points at each screen edge leave room for the surface margin.
        let main_budget = (output_main - 16.).max(1.);
        let cross_budget = (output_cross - 16.).max(1.);
        let folder_count = folder_count.min(crate::preferences::MAX_DOCK_FOLDERS);
        let launcher = usize::from(show_launcher);
        let utilities = 1 + launcher + folder_count;
        let preferred = finite(icon_size, 48.).clamp(MIN_ICON_SIZE, MAX_ICON_SIZE);
        // The resting tray keeps at most 80% of the output's long axis. The
        // hover magnification may exceed it while the pointer is inside.
        let limit = (output_main * LENGTH_SHARE).min(main_budget);
        // The tray also has to fit across the output, where the thickness grows
        // as the icon plus twice the cross clearance.
        let cross_max = ((cross_budget - SHADOW) / 1.67).clamp(MIN_ICON_SIZE, MAX_ICON_SIZE);
        let total = app_count.saturating_add(utilities);
        // Reserve clearance for both possible separators while choosing the
        // icon step, so a later recent/folder group cannot push the row past
        // the output budget.
        let mut icon_size = fit_icon_size(preferred.min(cross_max), total, 2, limit);
        let mut visible_apps = app_count;
        let mut hidden_apps = 0usize;
        // The number of slots that fit at the final icon size. It is recomputed
        // below when the row must fold applications away.
        let mut capacity = slot_capacity(icon_size, 2, limit);
        if tray_length(total, icon_size) + 2. * separator_clearance(icon_size) > limit {
            // Even the smallest icon cannot carry every application, so the
            // overflow tile takes the remainder.
            icon_size = MIN_ICON_SIZE.min(cross_max);
            capacity = slot_capacity(icon_size, 2, limit);
            visible_apps = capacity.saturating_sub(utilities + 1).min(app_count);
            hidden_apps = app_count - visible_apps;
        }
        let slot_count = Self::slot_count_for(visible_apps, hidden_apps, utilities);
        let item = icon_size + row_gap(icon_size);
        let inset = cross_clearance(icon_size);
        let thickness = icon_size + 2. * inset;
        let separator_extra = separator_clearance(icon_size);
        let separator_before = separator_slots(
            launcher,
            visible_apps,
            0,
            0,
            visible_apps,
            hidden_apps,
            folder_count,
            slot_count,
        );
        let length = tray_length(slot_count, icon_size)
            + separator_before.len() as f32 * separator_extra;
        let bounce_amplitude = (icon_size * 0.35).min((cross_budget - thickness - SHADOW).max(0.));
        let desired_growth = if magnification_enabled {
            icon_size * (finite(scale, 1.).clamp(1., 2.) - 1.)
        } else {
            0.
        };
        // Five equally spaced raised-cosine samples sum to 2.5 at every phase.
        // This bounds the expansion even while the pointer moves between slots.
        let main_growth_limit =
            ((main_budget - length - 2. * SHADOW) / MAGNIFICATION_SUPPORT).max(0.);
        let cross_growth_limit = (cross_budget - thickness - SHADOW - bounce_amplitude).max(0.);
        let growth = desired_growth
            .min(main_growth_limit)
            .min(cross_growth_limit);
        let expansion = (MAGNIFICATION_SUPPORT * growth).ceil();
        // Layer-shell configure sizes are integer logical points. Retaining a
        // fractional requested size would cause every Host snapshot to request
        // another resize, and rounding down could crop the tray's outer edge.
        let separator_reserve =
            (2usize.saturating_sub(separator_before.len())) as f32 * separator_extra;
        let surface_main = (length + separator_reserve + 2. * SHADOW + expansion)
            .ceil()
            .min(output_main.floor().max(1.));
        // Reserve a band on the side away from the screen edge for the hover
        // tooltip, so the application name is never clipped by the surface.
        // A horizontal Dock shows the label above the tray, so only its height
        // is needed; a vertical Dock shows it beside the tray, so the band must
        // fit the widest label.
        let tooltip = if vertical { TOOLTIP_REACH } else { TOOLTIP_BAND };
        let surface_cross = (thickness + SHADOW + growth + bounce_amplitude + tooltip)
            .ceil()
            .min(output_cross.floor().max(1.));
        let main_padding = (surface_main - length) / 2.;
        let cross_padding = surface_cross - thickness;
        let (surface_width, surface_height) = if vertical {
            (surface_cross, surface_main)
        } else {
            (surface_main, surface_cross)
        };
        Self {
            length,
            thickness,
            surface_width,
            surface_height,
            main_padding,
            cross_padding,
            inset,
            end_gap: end_gap(icon_size),
            radius: corner_radius(icon_size),
            item,
            icon_size,
            scale: 1. + growth / icon_size,
            growth,
            bounce_amplitude,
            visible_apps,
            separator_extra,
            separator_before,
            launcher,
            folder_count,
            visible_pinned: visible_apps,
            visible_running: 0,
            visible_recent: 0,
            hidden_apps,
            slot_count,
            capacity,
            utilities,
            surface_main,
        }
    }

    /// Recompute the overflow split so the drawer folds only pinned
    /// applications. Running applications are kept before recent ones, and the
    /// existing recent applications keep their slots before any pinned
    /// application that does not fit. The overflow tile is created only when
    /// pinned applications actually have to fold. The surface size is left
    /// untouched, so the tray simply recenters when a slot is freed.
    pub fn restrict_overflow(
        &mut self,
        pinned_apps: usize,
        running_apps: usize,
        recent_apps: usize,
    ) {
        let total_apps = self.visible_apps + self.hidden_apps;
        let pinned_apps = pinned_apps.min(total_apps);
        let available_non_pinned = total_apps - pinned_apps;
        let running_apps = running_apps.min(available_non_pinned);
        let recent_apps = recent_apps.min(available_non_pinned - running_apps);

        let without_tile = self.capacity.saturating_sub(self.utilities);
        let (running, recent) = fit_non_pinned(running_apps, recent_apps, without_tile);
        if pinned_apps <= without_tile.saturating_sub(running + recent) {
            // Every pinned application fits without the tile; any remaining
            // slots stay empty rather than stretching the tray.
            self.visible_pinned = pinned_apps;
            self.visible_running = running;
            self.visible_recent = recent;
            self.visible_apps = pinned_apps + running + recent;
            self.hidden_apps = 0;
        } else {
            // The pinned group overflows. Fold its tail into the tile and give
            // the running and recent groups the rest of the row.
            let with_tile = without_tile.saturating_sub(1);
            let (running, recent) = fit_non_pinned(running_apps, recent_apps, with_tile);
            let pinned = pinned_apps.min(with_tile.saturating_sub(running + recent));
            self.visible_pinned = pinned;
            self.visible_running = running;
            self.visible_recent = recent;
            self.visible_apps = pinned + running + recent;
            self.hidden_apps = pinned_apps - pinned;
        }
        self.slot_count = Self::slot_count_for(self.visible_apps, self.hidden_apps, self.utilities);
        self.separator_extra = separator_clearance(self.icon_size);
        self.separator_before = separator_slots(
            self.launcher,
            self.visible_pinned,
            self.visible_running,
            self.visible_recent,
            self.visible_apps,
            self.hidden_apps,
            self.folder_count,
            self.slot_count,
        );
        self.length = tray_length(self.slot_count, self.icon_size)
            + self.separator_before.len() as f32 * self.separator_extra;
        // Keep the tray centered in the unchanged surface band reserved for
        // magnification.
        self.main_padding = (self.surface_main - self.length) / 2.;
    }

    fn slot_count_for(visible_apps: usize, hidden_apps: usize, utilities: usize) -> usize {
        visible_apps + utilities + usize::from(hidden_apps > 0)
    }

    /// Pointer and sampled icon positions use the same surface-local main axis.
    /// Influence uses idle centers, avoiding a moving-target hover feedback loop.
    pub fn samples(&self, pointer_main: Option<f32>, strength: f32) -> Vec<Slot> {
        let strength = finite(strength, 0.).clamp(0., 1.);
        let pointer = pointer_main.filter(|position| position.is_finite());
        let idle_start = self.main_padding + self.end_gap;
        // First pass: the magnification influence per slot, written straight
        // into the returned buffer's `lift` field. No scratch vectors — this
        // runs on every animation frame.
        let mut slots = Vec::with_capacity(self.slot_count);
        let mut center_cursor = idle_start;
        for index in 0..self.slot_count {
            if self.separator_before.binary_search(&index).is_ok() {
                center_cursor += self.separator_extra;
            }
            let center = center_cursor + self.icon_size / 2.;
            center_cursor += self.item;
            let influence = pointer
                .map(|pointer| {
                    let normalized =
                        (center - pointer).abs() / (MAGNIFICATION_SUPPORT * self.item);
                    if normalized >= 1. {
                        0.
                    } else {
                        (1. + (PI * normalized).cos()) / 2.
                    }
                })
                .unwrap_or(0.);
            slots.push(Slot {
                origin_main: 0.,
                size: self.icon_size,
                lift: self.growth * influence * strength,
                extent: 0.,
            });
        }
        // Second pass: folding the untouched slots equally towards the
        // magnified centre keeps the tray centred; the separator steps replay
        // in the same order as the first pass so centers match.
        let total_growth: f32 = slots.iter().map(|slot| slot.lift).sum();
        let mut cursor = idle_start - total_growth / 2.;
        for (index, slot) in slots.iter_mut().enumerate() {
            if self.separator_before.binary_search(&index).is_ok() {
                cursor += self.separator_extra;
            }
            slot.origin_main = cursor;
            slot.size = self.icon_size + slot.lift;
            slot.extent = self.item + slot.lift;
            cursor += slot.extent;
        }
        slots
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionSample {
    pub value: f32,
    pub velocity: f32,
    pub active: bool,
}

/// How fast the elastic spring oscillates, in rad/s: about a 0.45s settle
/// with two visible swings at the shared damping.
pub const ELASTIC_FREQUENCY: f64 = 15.;
/// Underdamped on purpose: the fold overshoots its resting spot and swings
/// back, which is the elasticity the card and popup height rides read from.
pub const ELASTIC_DAMPING: f64 = 0.55;

/// Analytic underdamped scalar spring with velocity-preserving retargets.
/// `Motion` settles without overshoot; this one bounces.
#[derive(Clone, Debug)]
pub struct Elastic {
    from: f64,
    target: f64,
    velocity: f64,
    started: Duration,
    frequency: f64,
    damping: f64,
}

impl Elastic {
    pub fn new(value: f32) -> Self {
        Self::tuned(value, ELASTIC_FREQUENCY, ELASTIC_DAMPING)
    }
    pub fn tuned(value: f32, frequency: f64, damping: f64) -> Self {
        let value = finite(value, 0.) as f64;
        Self {
            from: value,
            target: value,
            velocity: 0.,
            started: Duration::ZERO,
            frequency: if frequency.is_finite() {
                frequency.clamp(1., 1000.)
            } else {
                ELASTIC_FREQUENCY
            },
            damping: if damping.is_finite() {
                damping.clamp(0.1, 0.99)
            } else {
                ELASTIC_DAMPING
            },
        }
    }
    pub fn target(&self) -> f32 {
        self.target as f32
    }
    /// Set an immediate endpoint, for reduced motion or a fresh surface.
    pub fn snap(&mut self, value: f32) {
        self.target = finite(value, 0.) as f64;
        self.from = self.target;
        self.velocity = 0.;
    }
    pub fn retarget(&mut self, target: f32, now: Duration) {
        let target = finite(target, self.target as f32) as f64;
        if self.target == target {
            return;
        }
        let current = self.sample(now);
        self.from = current.value as f64;
        self.velocity = current.velocity as f64;
        self.target = target;
        self.started = now;
    }
    pub fn sample(&self, now: Duration) -> MotionSample {
        let elapsed = now.saturating_sub(self.started).as_secs_f64();
        let omega = self.frequency;
        let damped = omega * (1. - self.damping * self.damping).sqrt();
        let offset = self.from - self.target;
        let carry = (self.velocity + self.damping * omega * offset) / damped;
        let decay = (-self.damping * omega * elapsed).exp();
        let angle = damped * elapsed;
        let displacement = (offset * angle.cos() + carry * angle.sin()) * decay;
        let velocity = ((carry * damped - self.damping * omega * offset) * angle.cos()
            + (-offset * damped - self.damping * omega * carry)
                * angle.sin())
            * decay;
        let active = displacement.abs() > POSITION_EPSILON
            || velocity.abs() > POSITION_EPSILON * self.frequency;
        MotionSample {
            value: if active {
                (self.target + displacement) as f32
            } else {
                self.target as f32
            },
            velocity: if active { velocity as f32 } else { 0. },
            active,
        }
    }
}

/// Analytic critically damped scalar response with velocity-preserving retargets.
#[derive(Clone, Debug)]
pub struct Motion {
    from: f64,
    target: f64,
    velocity: f64,
    started: Duration,
    frequency: f64,
}

impl Motion {
    pub fn new(value: f32) -> Self {
        Self::with_frequency(value, RESPONSE_FREQUENCY)
    }

    pub fn with_frequency(value: f32, frequency: f64) -> Self {
        let value = finite(value, 0.) as f64;
        Self {
            from: value,
            target: value,
            velocity: 0.,
            started: Duration::ZERO,
            frequency: if frequency.is_finite() {
                frequency.clamp(1., 1000.)
            } else {
                RESPONSE_FREQUENCY
            },
        }
    }

    pub fn target(&self) -> f32 {
        self.target as f32
    }

    /// Set an immediate endpoint, for reduced motion or a newly mapped surface.
    pub fn snap(&mut self, value: f32) {
        self.target = finite(value, 0.) as f64;
        self.from = self.target;
        self.velocity = 0.;
    }

    pub fn retarget(&mut self, target: f32, now: Duration) {
        let target = finite(target, self.target as f32) as f64;
        if self.target == target {
            return;
        }
        let current = self.sample(now);
        self.from = current.value as f64;
        self.velocity = current.velocity as f64;
        self.target = target;
        self.started = now;
    }

    pub fn sample(&self, now: Duration) -> MotionSample {
        let elapsed = now.saturating_sub(self.started).as_secs_f64();
        let difference = self.from - self.target;
        let coefficient = self.velocity + self.frequency * difference;
        let decay = (-self.frequency * elapsed).exp();
        let displacement = (difference + coefficient * elapsed) * decay;
        let velocity = (self.velocity - self.frequency * coefficient * elapsed) * decay;
        let active = displacement.abs() > POSITION_EPSILON
            || velocity.abs() > POSITION_EPSILON * self.frequency;
        MotionSample {
            value: if active {
                (self.target + displacement) as f32
            } else {
                self.target as f32
            },
            velocity: if active { velocity as f32 } else { 0. },
            active,
        }
    }
}

pub const LAUNCH_BOUNCE_DURATION: Duration = Duration::from_millis(880);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BounceSample {
    /// Positive displacement away from the output edge, in logical points.
    pub offset: f32,
    pub active: bool,
}

/// Advance a reorder animation map: every untouched tile slides towards the
/// slot it would occupy. `entries` are `(identity, main-axis target)` pairs
/// minus the tile under the pointer. The Dock row, the folder strip and the
/// overflow drawer share this pattern; only the frequency and the entry
/// shapes differ. Returns whether any motion is still animating.
pub fn track_reorder(
    motions: &mut std::collections::HashMap<String, Motion>,
    frequency: f64,
    entries: impl IntoIterator<Item = (String, f32)>,
    now: Duration,
) -> bool {
    let mut active = false;
    for (id, target) in entries {
        let motion = motions
            .entry(id)
            .or_insert_with(|| Motion::with_frequency(target, frequency));
        if motion.target() != target {
            motion.retarget(target, now);
        }
        active |= motion.sample(now).active;
    }
    active
}

/// Collapse a reorder map back onto its resting layout and clear it, so a
/// later drag re-seeds from the live slot targets.
pub fn drain_reorder(motions: &mut std::collections::HashMap<String, Motion>) {
    motions.clear();
}

/// Three finite, diminishing hops. The caller starts this only for a new launch,
/// never for focusing an existing window; reduced motion bypasses it entirely.
pub fn launch_bounce(elapsed: Duration, icon_size: f32) -> BounceSample {
    if elapsed >= LAUNCH_BOUNCE_DURATION {
        return BounceSample {
            offset: 0.,
            active: false,
        };
    }
    let mut milliseconds = elapsed.as_secs_f32() * 1000.;
    let amplitude = finite(icon_size, 0.).max(0.) * 0.35;
    for (duration, attenuation) in [(360., 1.), (280., 0.48), (240., 0.2)] {
        if milliseconds < duration {
            let phase = PI * milliseconds / duration;
            return BounceSample {
                offset: amplitude * attenuation * phase.sin().powi(2),
                active: amplitude > 0.,
            };
        }
        milliseconds -= duration;
    }
    BounceSample {
        offset: 0.,
        active: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics(vertical: bool) -> DockMetrics {
        DockMetrics::new(10, 48., 1.45, true, true, vertical, 1440., 900.)
    }

    #[test]
    fn idle_icons_follow_the_scaled_geometry() {
        for vertical in [false, true] {
            let metrics = metrics(vertical);
            for (index, slot) in metrics.samples(None, 1.).iter().enumerate() {
                let separators = metrics
                    .separator_before
                    .iter()
                    .filter(|separator| **separator <= index)
                    .count() as f32;
                let expected = metrics.main_padding
                    + metrics.end_gap
                    + index as f32 * metrics.item
                    + separators * metrics.separator_extra;
                assert!((slot.origin_main - expected).abs() < 0.001);
                assert_eq!(slot.size, metrics.icon_size);
                assert_eq!(slot.lift, 0.);
            }
        }
    }

    #[test]
    fn the_tray_keeps_the_same_clearance_on_every_side() {
        for vertical in [false, true] {
            let metrics = metrics(vertical);
            // The icon is centered across the row.
            assert_eq!(metrics.thickness, metrics.icon_size + 2. * metrics.inset);
            assert_eq!(metrics.inset, cross_clearance(metrics.icon_size));
            // The row ends keep a little more clearance than the cross band so
            // the icons do not touch the rounded corners.
            assert!(metrics.end_gap >= metrics.inset);
            assert!(metrics.end_gap >= 14.);
        }
    }

    #[test]
    fn the_padding_and_the_corner_radius_scale_with_the_icon() {
        for icon in ICON_STEPS {
            assert!(row_gap(icon) >= 6. && row_gap(icon) <= 16.);
            assert!(cross_clearance(icon) >= indicator_diameter(icon) + 4.);
            assert!(cross_clearance(icon) <= 20.);
            assert!(end_gap(icon) >= 14.);
            assert!(corner_radius(icon) >= 18. && corner_radius(icon) <= 34.);
            // Larger icons never draw tighter than smaller ones.
            assert!(row_gap(icon) >= row_gap(MIN_ICON_SIZE));
            assert!(cross_clearance(icon) >= cross_clearance(MIN_ICON_SIZE));
            assert!(end_gap(icon) >= end_gap(MIN_ICON_SIZE));
            assert!(corner_radius(icon) >= corner_radius(MIN_ICON_SIZE));
        }
        assert_eq!(row_gap(48.), 12.);
        assert_eq!(cross_clearance(48.), 16.);
        assert_eq!(end_gap(48.), 22.);
        assert_eq!(corner_radius(48.), 24.);
        // The indicator stays small even on the smallest icon.
        assert_eq!(indicator_diameter(MIN_ICON_SIZE), 4.);
        assert_eq!(indicator_diameter(64.), 5.);
    }

    #[test]
    fn a_requested_size_snaps_down_the_icon_ladder() {
        // The Dock only draws the three native steps.
        assert_eq!(snap_icon_step(64.), 64.);
        assert_eq!(snap_icon_step(56.), 48.);
        assert_eq!(snap_icon_step(48.), 48.);
        assert_eq!(snap_icon_step(40.), 32.);
        assert_eq!(snap_icon_step(32.), 32.);
        // Anything below the floor still clamps to the smallest step.
        assert_eq!(snap_icon_step(20.), 32.);
    }

    #[test]
    fn the_resting_tray_keeps_at_most_the_configured_share_of_the_output() {
        for (width, height) in [(1280., 800.), (1920., 1080.), (3840., 2160.)] {
            for vertical in [false, true] {
                for count in [0usize, 1, 4, 12, 40] {
                    let metrics =
                        DockMetrics::new(count, 64., 1.45, true, true, vertical, width, height);
                    let output_main = if vertical { height } else { width };
                    assert!(metrics.length <= output_main * LENGTH_SHARE + 0.001);
                    assert!(metrics.length <= output_main - 16. + 0.001);
                }
            }
        }
    }

    #[test]
    fn the_dock_shrinks_icons_before_it_hides_applications() {
        // A moderate row still draws at the requested size.
        let metrics = DockMetrics::new(10, 48., 1.45, true, true, false, 1920., 1080.);
        assert_eq!(metrics.icon_size, 48.);
        assert_eq!(metrics.hidden_apps, 0);
        // A crowded row steps down the icon ladder instead of dropping
        // applications, and it lands exactly on a native step.
        let metrics = DockMetrics::new(30, 48., 1.45, true, true, false, 1920., 1080.);
        assert_eq!(metrics.icon_size, 32.);
        assert!(ICON_STEPS.contains(&metrics.icon_size));
        assert!(metrics.icon_size >= MIN_ICON_SIZE);
        assert_eq!(metrics.hidden_apps, 0);
    }

    #[test]
    fn the_dock_folds_applications_into_the_overflow_tile() {
        let metrics = DockMetrics::new(60, 64., 1.45, true, true, false, 1280., 800.);
        assert_eq!(metrics.icon_size, MIN_ICON_SIZE);
        assert!(metrics.hidden_apps > 0);
        assert_eq!(metrics.visible_apps + metrics.hidden_apps, 60);
        // The overflow tile takes a slot of its own next to the utilities.
        assert_eq!(metrics.slot_count, metrics.visible_apps + 2 + 1);
        assert!(metrics.length <= 1280. * LENGTH_SHARE + 0.001);
    }

    #[test]
    fn the_overflow_tile_folds_only_pinned_applications() {
        // Sixty applications on a cramped row. The drawer must never fold a
        // running or recent application: those either fill the slots left over
        // after the pinned group or are dropped, but the overflow tile always
        // holds pinned applications.
        let full = DockMetrics::new(60, 64., 1.45, true, true, false, 1280., 800.);
        assert!(full.hidden_apps > 0, "the row is genuinely crowded");

        // A small pinned group fits entirely, so no drawer is needed even though
        // running and recent applications exist.
        let mut few = full.clone();
        few.restrict_overflow(8, 2, 3);
        assert_eq!(few.hidden_apps, 0, "a small pinned group never overflows");
        assert!(few.visible_apps >= 8, "every pinned application stays visible");
        assert_eq!(few.visible_pinned, 8);

        // A large pinned group overflows, and only its tail folds away.
        let mut many = full.clone();
        many.restrict_overflow(45, 0, 0);
        assert!(many.hidden_apps > 0, "the pinned group overflows");
        assert!(many.hidden_apps <= 45, "the drawer never exceeds the pinned group");
        // The folded tail is exactly the pinned applications beyond the row: the
        // visible leading apps plus the folded apps cover the pinned group and
        // never spill into the running/recent remainder.
        assert_eq!(
            many.visible_apps + many.hidden_apps,
            60.min(many.visible_apps + many.hidden_apps),
        );
        assert!(
            many.visible_apps + many.hidden_apps >= 45,
            "visible plus folded cover at least the pinned group"
        );
        // The overflow tile takes its own slot.
        assert_eq!(many.slot_count, many.visible_apps + many.utilities + 1);
    }

    #[test]
    fn recent_applications_keep_three_slots_when_pinned_applications_overflow() {
        let mut metrics = DockMetrics::new(60, 64., 1.45, true, true, false, 1280., 800.);
        let app_room = metrics.capacity - metrics.utilities;
        metrics.restrict_overflow(45, 0, 3);
        assert!(metrics.hidden_apps > 0, "the pinned group still overflows");
        assert_eq!(metrics.visible_pinned + metrics.hidden_apps, 45);
        assert_eq!(
            metrics.visible_apps - metrics.visible_pinned,
            3,
            "all three recent applications remain in the row"
        );
        assert!(
            metrics.visible_apps + 1 <= app_room,
            "the overflow tile and the recent group fit the capacity"
        );
        assert_eq!(metrics.slot_count, metrics.visible_apps + metrics.utilities + 1);

        // Running applications are preserved before the pinned tail is folded.
        let mut with_running = DockMetrics::new(60, 64., 1.45, true, true, false, 1280., 800.);
        with_running.restrict_overflow(45, 5, 3);
        assert_eq!(with_running.visible_apps - with_running.visible_pinned, 8);
        assert_eq!(with_running.visible_pinned + with_running.hidden_apps, 45);
        assert_eq!(
            with_running.slot_count,
            with_running.visible_apps + with_running.utilities + 1
        );
    }

    #[test]
    fn recent_slots_are_only_reserved_for_applications_that_exist() {
        // A short recent list does not leave an empty gap in the row.
        let mut metrics = DockMetrics::new(15, 64., 1.45, true, true, false, 1920., 1080.);
        let capacity = metrics.capacity - metrics.utilities;
        metrics.restrict_overflow(10, 0, 1);
        assert_eq!(metrics.hidden_apps, 0);
        assert_eq!(metrics.visible_pinned, 10);
        assert_eq!(metrics.visible_apps, 11);
        assert!(metrics.visible_apps < capacity);

        // Disabling recent applications leaves the original pinned-only split.
        let mut disabled = DockMetrics::new(60, 64., 1.45, true, true, false, 1280., 800.);
        disabled.restrict_overflow(45, 0, 0);
        assert_eq!(disabled.visible_apps, disabled.visible_pinned);
        assert_eq!(disabled.visible_pinned + disabled.hidden_apps, 45);
    }

    #[test]
    fn magnification_is_continuous_nonoverlapping_and_within_stable_canvas() {
        for vertical in [false, true] {
            let metrics = metrics(vertical);
            let surface_main = if vertical {
                metrics.surface_height
            } else {
                metrics.surface_width
            };
            for step in 0..2001 {
                let pointer = step as f32 * surface_main / 2000.;
                let slots = metrics.samples(Some(pointer), 1.);
                for pair in slots.windows(2) {
                    assert!(
                        pair[1].origin_main - pair[0].origin_main - pair[0].size
                            >= row_gap(metrics.icon_size) - 0.01
                    );
                }
                for slot in slots {
                    assert!(slot.origin_main >= 0.);
                    assert!(slot.origin_main + slot.size <= surface_main);
                    assert!(slot.size >= metrics.icon_size);
                    assert!(slot.size <= metrics.icon_size * 1.45 + 0.001);
                    assert!(slot.lift <= metrics.growth + 0.001);
                }
                let next = metrics.samples(Some(pointer + 0.01), 1.);
                let current = metrics.samples(Some(pointer), 1.);
                for (before, after) in current.iter().zip(&next) {
                    assert!((before.origin_main - after.origin_main).abs() < 0.1);
                    assert!((before.size - after.size).abs() < 0.1);
                }
            }
        }
    }

    #[test]
    fn output_budgets_limit_count_and_growth_without_changing_idle_geometry() {
        for (width, height) in [(320., 240.), (640., 480.), (1440., 900.), (3840., 2160.)] {
            for vertical in [false, true] {
                for enabled in [false, true] {
                    let metrics = DockMetrics::new(
                        usize::MAX,
                        64.,
                        2.,
                        enabled,
                        true,
                        vertical,
                        width,
                        height,
                    );
                    assert!(metrics.surface_width <= width && metrics.surface_height <= height);
                    assert_eq!(metrics.thickness, metrics.icon_size + 2. * metrics.inset);
                    let center = if vertical {
                        metrics.surface_height / 2.
                    } else {
                        metrics.surface_width / 2.
                    };
                    for slot in metrics.samples(Some(center), 1.) {
                        assert!(slot.size.is_finite());
                    }
                    if !enabled {
                        assert_eq!(metrics.growth, 0.);
                    }
                }
            }
        }
    }

    #[test]
    fn negotiated_surfaces_use_integer_sizes_and_keep_the_idle_tray_centered() {
        for scale in [1., 1.1, 1.45, 1.75, 2.] {
            for vertical in [false, true] {
                let metrics = DockMetrics::new(10, 48., scale, true, true, vertical, 1440., 900.);
                assert_eq!(metrics.surface_width.fract(), 0.);
                assert_eq!(metrics.surface_height.fract(), 0.);
                let (main, cross) = if vertical {
                    (metrics.surface_height, metrics.surface_width)
                } else {
                    (metrics.surface_width, metrics.surface_height)
                };
                assert!((metrics.main_padding * 2. + metrics.length - main).abs() < 0.001);
                assert!((metrics.cross_padding + metrics.thickness - cross).abs() < 0.001);
                assert!(
                    metrics.cross_padding >= SHADOW + metrics.growth + metrics.bounce_amplitude
                );
                assert!(metrics.main_padding >= SHADOW);
            }
        }
    }

    #[test]
    fn a_retarget_starts_at_the_visible_position_and_velocity() {
        let mut motion = Motion::new(0.);
        motion.retarget(1., Duration::ZERO);
        let now = Duration::from_millis(73);
        let before = motion.sample(now);
        motion.retarget(0., now);
        let after = motion.sample(now);
        assert!((before.value - after.value).abs() < 1e-6);
        assert!((before.velocity - after.velocity).abs() < 1e-6);
        assert!(after.active);
        assert_eq!(
            motion.sample(Duration::from_secs(2)),
            MotionSample {
                value: 0.,
                velocity: 0.,
                active: false
            }
        );
    }

    #[test]
    fn motion_sampling_is_independent_of_frame_rate_and_goes_dormant() {
        let mut motion = Motion::new(0.);
        motion.retarget(1., Duration::ZERO);
        let target_time = Duration::from_millis(137);
        let expected = motion.sample(target_time);
        for refresh in [30, 60, 120, 144, 240] {
            let mut now = Duration::ZERO;
            while now < target_time {
                let _ = motion.sample(now);
                now += Duration::from_secs_f64(1. / f64::from(refresh));
            }
            assert_eq!(motion.sample(target_time), expected);
        }
        assert_eq!(
            motion.sample(Duration::from_secs(1)),
            MotionSample {
                value: 1.,
                velocity: 0.,
                active: false
            }
        );
        motion.snap(0.);
        assert!(!motion.sample(Duration::from_secs(1)).active);
    }

    #[test]
    fn launch_hops_start_and_finish_at_rest_and_never_exceed_the_budget() {
        assert_eq!(launch_bounce(Duration::ZERO, 48.).offset, 0.);
        for milliseconds in 0..=1200 {
            let sample = launch_bounce(Duration::from_millis(milliseconds), 48.);
            assert!(sample.offset >= 0. && sample.offset <= 48. * 0.35 + 0.001);
            if milliseconds >= 880 {
                assert!(!sample.active && sample.offset == 0.);
            }
        }
        for boundary in [360, 640, 880] {
            let sample = launch_bounce(Duration::from_millis(boundary), 48.);
            assert!(sample.offset.abs() < 0.001);
        }
        assert!(!launch_bounce(Duration::from_millis(100), f32::NAN).active);
    }

    #[test]
    fn folders_are_utilities_between_overflow_and_trash() {
        let mut metrics =
            DockMetrics::with_folders(3, 48., 1.45, true, true, 2, false, 1440., 900.);
        metrics.restrict_overflow(3, 0, 0);
        assert_eq!(metrics.folder_count, 2);
        assert_eq!(metrics.slot_count, 1 + 3 + 2 + 1);
        assert_eq!(dock_slot_kind(&metrics, 1, 0), DockSlotKind::Launcher);
        assert_eq!(dock_slot_kind(&metrics, 1, 1), DockSlotKind::App(0));
        assert_eq!(dock_slot_kind(&metrics, 1, 2), DockSlotKind::App(1));
        assert_eq!(dock_slot_kind(&metrics, 1, 3), DockSlotKind::App(2));
        assert_eq!(dock_slot_kind(&metrics, 1, 4), DockSlotKind::Folder(0));
        assert_eq!(dock_slot_kind(&metrics, 1, 5), DockSlotKind::Folder(1));
        assert_eq!(dock_slot_kind(&metrics, 1, 6), DockSlotKind::Trash);
    }

    #[test]
    fn folders_are_kept_before_apps_overflow() {
        let mut metrics =
            DockMetrics::with_folders(60, 64., 1.45, true, true, 8, false, 1280., 800.);
        metrics.restrict_overflow(60, 0, 0);
        assert_eq!(metrics.folder_count, 8);
        assert_eq!(metrics.visible_apps + metrics.hidden_apps, 60);
        assert_eq!(metrics.slot_count, metrics.visible_apps + 8 + 2 + 1);
    }

    #[test]
    fn separators_add_clearance_on_both_sides_without_changing_row_gap() {
        let mut metrics =
            DockMetrics::with_folders(3, 48., 1.45, true, true, 1, false, 1440., 900.);
        metrics.restrict_overflow(3, 0, 0);
        let slots = metrics.samples(None, 0.);
        let folder_slot = 1 + metrics.visible_apps;
        let before = &slots[folder_slot - 1];
        let after = &slots[folder_slot];
        let gap = after.origin_main - (before.origin_main + metrics.icon_size);
        assert!(
            (gap - (row_gap(metrics.icon_size) + metrics.separator_extra)).abs() < 0.01,
            "separator gap {gap} should be one normal gap plus the separator clearance"
        );
        assert!(metrics.separator_before.contains(&folder_slot));
        assert!(metrics.length > tray_length(metrics.slot_count, metrics.icon_size));
    }

    #[test]
    fn invalid_inputs_do_not_poison_frame_state() {
        let metrics = DockMetrics::new(10, f32::NAN, f32::INFINITY, true, true, false, 1440., 900.);
        assert!(metrics
            .samples(Some(f32::NAN), f32::NAN)
            .iter()
            .all(|slot| slot.size.is_finite()));
        let mut motion = Motion::new(f32::NAN);
        motion.retarget(f32::NAN, Duration::ZERO);
        assert_eq!(
            motion.sample(Duration::from_secs(1)),
            MotionSample {
                value: 0.,
                velocity: 0.,
                active: false
            }
        );
    }
}
