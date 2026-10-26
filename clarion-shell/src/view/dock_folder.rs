// SPDX-License-Identifier: GPL-3.0-or-later
//! Dock folder popup contents. Geometry lives in the placement module; this
//! module only builds the list/grid rows and their actions.

use super::*;
use crate::app::panels::{
    FOLDER_MENU_ACTION_ROW, FOLDER_MENU_CHOICE_GAP, FOLDER_MENU_CHOICE_ROW,
    FOLDER_MENU_HEADER_ROW, FOLDER_MENU_INNER_GAP, FOLDER_MENU_LABEL_ROW,
    FOLDER_MENU_SEPARATOR_ROW,
};
use crate::dock_folder::{FolderEntry, FolderEntryKind};
use crate::preferences::{FolderDisplay, FolderLayout, FolderSort};

#[derive(Clone, Debug)]
pub(super) struct FolderEntryDrag {
    pub path: PathBuf,
}

struct FolderDragPreview {
    path: PathBuf,
    background: Hsla,
    foreground: Hsla,
    shadow: Hsla,
}

/// Applies a glyph-only rotation while an element paints. The element keeps an
/// axis-aligned hitbox, but the text follows the fan tangent.
struct RotatedText {
    child: AnyElement,
    center: Point<Pixels>,
    angle: f32,
}

impl IntoElement for RotatedText {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for RotatedText {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_text_rotation(self.center, self.angle, |window| {
            self.child.paint(window, cx)
        });
    }
}

fn rotate_text(child: AnyElement, center: Point<Pixels>, angle: f32) -> AnyElement {
    RotatedText {
        child,
        center,
        angle,
    }
    .into_any_element()
}

fn fit_label(text: &str, width: f32) -> String {
    let limit = ((width / 7.).floor() as usize).max(3);
    let mut used = 0;
    let mut fitted = String::with_capacity(text.len());
    for ch in text.chars() {
        let weight = if ch.is_ascii() { 1 } else { 2 };
        if used + weight > limit {
            fitted.push('…');
            return fitted;
        }
        used += weight;
        fitted.push(ch);
    }
    fitted
}

/// Rasterize a rotated rounded plate into one-logical-pixel scanline rows.
///
/// The glass protocol only carries axis-aligned rectangles, and the compositor
/// frosts exactly their union, so a tilted plate can only be followed by its
/// own silhouette: neither the bounding box (spills past the label) nor an
/// inscribed rectangle (leaves the plate's ends bare) is the plate's shape.
/// This mirrors clavis, which drives its compositor mask the same way.
pub(super) fn rotated_plate_rows(
    glass: &mut Vec<Bounds<Pixels>>,
    center: Point<Pixels>,
    plate: Size<Pixels>,
    radians: f32,
    radius: f32,
) {
    let width = f32::from(plate.width);
    let height = f32::from(plate.height);
    if width <= 0. || height <= 0. {
        return;
    }
    let radius = radius.clamp(0., width.min(height) / 2.);
    let (sin, cos) = radians.sin_cos();
    let cx = f32::from(center.x);
    let cy = f32::from(center.y);

    // Outline of the rounded plate in surface coordinates, walked clockwise
    // from the top-left arc. Eight steps per quarter keep corners round at
    // Dock-label sizes without flooding the region with vertices.
    const ARC_STEPS: usize = 8;
    let mut points = Vec::with_capacity(4 * (ARC_STEPS + 1));
    for (corner_x, corner_y, start_degrees) in [
        (-width / 2. + radius, -height / 2. + radius, 180.),
        (width / 2. - radius, -height / 2. + radius, 270.),
        (width / 2. - radius, height / 2. - radius, 0.),
        (-width / 2. + radius, height / 2. - radius, 90.),
    ] {
        for step in 0..=ARC_STEPS {
            let (step_sin, step_cos) = (start_degrees
                + 90. * step as f32 / ARC_STEPS as f32)
                .to_radians()
                .sin_cos();
            let x = corner_x + radius * step_cos;
            let y = corner_y + radius * step_sin;
            points.push((cx + x * cos - y * sin, cy + x * sin + y * cos));
        }
    }

    let top = points.iter().map(|p| p.1).fold(f32::MAX, f32::min).floor() as i32;
    let bottom = points.iter().map(|p| p.1).fold(f32::MIN, f32::max).ceil() as i32;
    // Vertically adjacent rows spanning the same columns fold into one
    // rectangle; the plate's straight middle keeps a handful of rects.
    let mut run: Option<(i32, i32, i32)> = None; // (x, width, top)
    let mut flush = |run: &mut Option<(i32, i32, i32)>, bottom: i32| {
        if let Some((x, width, top)) = run.take() {
            glass.push(Bounds::new(
                point(px(x as f32), px(top as f32)),
                size(px(width as f32), px((bottom - top) as f32)),
            ));
        }
    };
    for y in top..bottom {
        let scan_y = y as f32 + 0.5;
        let mut crossings = Vec::with_capacity(2);
        for index in 0..points.len() {
            let a = points[index];
            let b = points[(index + 1) % points.len()];
            if (a.1 <= scan_y && b.1 > scan_y) || (b.1 <= scan_y && a.1 > scan_y) {
                crossings.push(a.0 + (scan_y - a.1) * (b.0 - a.0) / (b.1 - a.1));
            }
        }
        if crossings.len() < 2 {
            flush(&mut run, y);
            continue;
        }
        crossings.sort_by(f32::total_cmp);
        let left = crossings[0];
        let right = crossings[crossings.len() - 1];
        // Include the pixels whose centers fall inside the row's span.
        let x = (left - 0.5).ceil() as i32;
        let width = (right - 0.5).floor() as i32 + 1 - x;
        if width <= 0 {
            flush(&mut run, y);
            continue;
        }
        match run {
            Some((run_x, run_width, _)) if run_x == x && run_width == width => {}
            _ => {
                flush(&mut run, y);
                run = Some((x, width, y));
            }
        }
    }
    flush(&mut run, bottom);
}

fn append_rotated_round_rect(
    path: &mut PathBuilder,
    bounds: Bounds<Pixels>,
    radius: f32,
    radians: f32,
) {
    let center = bounds.center();
    let cx = f32::from(center.x);
    let cy = f32::from(center.y);
    let (sin, cos) = radians.sin_cos();
    let at = |x: f32, y: f32| {
        let dx = x - cx;
        let dy = y - cy;
        point(px(cx + dx * cos - dy * sin), px(cy + dx * sin + dy * cos))
    };
    let half_w = f32::from(bounds.size.width) / 2.;
    let half_h = f32::from(bounds.size.height) / 2.;
    let r = radius.min(half_w).min(half_h).max(0.);
    let left = cx - half_w;
    let right = cx + half_w;
    let top = cy - half_h;
    let bottom = cy + half_h;
    let k = 0.552_284_8;
    let corner = |path: &mut PathBuilder, a: (f32, f32), c: (f32, f32), b: (f32, f32)| {
        let c1 = (a.0 + (c.0 - a.0) * k, a.1 + (c.1 - a.1) * k);
        let c2 = (b.0 + (c.0 - b.0) * k, b.1 + (c.1 - b.1) * k);
        path.cubic_bezier_to(at(b.0, b.1), at(c1.0, c1.1), at(c2.0, c2.1));
    };
    path.move_to(at(left + r, top));
    path.line_to(at(right - r, top));
    corner(path, (right - r, top), (right, top), (right, top + r));
    path.line_to(at(right, bottom - r));
    corner(
        path,
        (right, bottom - r),
        (right, bottom),
        (right - r, bottom),
    );
    path.line_to(at(left + r, bottom));
    corner(path, (left + r, bottom), (left, bottom), (left, bottom - r));
    path.line_to(at(left, top + r));
    corner(path, (left, top + r), (left, top), (left + r, top));
}

/// A short ease-out-back curve for the fan unfolding. Values above 1 are the
/// elastic overshoot; `false` means the animation has settled.
pub(super) fn fan_open_progress(
    elapsed: std::time::Duration,
) -> (f32, bool) {
    const DURATION: f32 = 0.42;
    const BACK: f32 = 1.35;
    let t = elapsed.as_secs_f32() / DURATION;
    if t >= 1. {
        return (1., false);
    }
    let p = t - 1.;
    let back = BACK + 1.;
    (1. + back * p * p * p + BACK * p * p, true)
}

/// The retract curve. The fan leaves with a short outward anticipation, then
/// settles onto the stack with zero velocity so the last frames fade instead of
/// snapping back. The second value is the paint opacity, which follows the
/// collapse and reaches zero as the folder icon is revealed; `false` means the
/// window is ready to close.
pub(super) fn fan_close_progress(
    elapsed: std::time::Duration,
) -> (f32, f32, bool) {
    const DURATION: f32 = 0.36;
    const ANTICIPATION: f32 = 4.;
    let t = elapsed.as_secs_f32() / DURATION;
    if t >= 1. {
        return (0., 0., false);
    }
    // (1 - t)^2 keeps the end velocity at zero; the linear term is the gentle
    // outward anticipation before the fan folds back into the tile.
    let left = 1. - t;
    let open = left * left * (1. + ANTICIPATION * t);
    (open, open.clamp(0., 1.), true)
}

fn fan_label_background(radians: f32, p: Palette) -> AnyElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            // The fan is a transparent overlay, so labels need more body than
            // the frosted menu surface behind it: the compositor frost is only
            // a backdrop, and a whisper-thin wash left the plates reading as
            // bare text. Compositing the panel over itself lifts the alpha
            // without changing its hue.
            let mut fill = PathBuilder::fill();
            append_rotated_round_rect(&mut fill, bounds, FAN_LABEL_RADIUS, radians);
            if let Ok(path) = fill.build() {
                window.paint_path(path, p.panel.blend(p.panel));
            }
            let mut stroke = PathBuilder::stroke(px(1.));
            append_rotated_round_rect(&mut stroke, bounds, FAN_LABEL_RADIUS, radians);
            if let Ok(path) = stroke.build() {
                window.paint_path(path, p.line);
            }
        },
    )
    .absolute()
    .inset_0()
    .into_any_element()
}

impl Render for FolderDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let name = self
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.to_string_lossy().into_owned());
        div()
            .debug_selector(|| "folder-drag-preview".into())
            .flex()
            .items_center()
            .gap_1()
            .px(px(8.))
            .py(px(6.))
            .rounded(px(6.))
            .opacity(0.3)
            .bg(self.background)
            .text_color(self.foreground)
            .shadow(vec![BoxShadow {
                color: self.shadow,
                offset: point(px(0.), px(4.)),
                blur_radius: px(12.),
                spread_radius: px(0.),
            }])
            .child(crate::file_icons::icon(
                Some(&self.path),
                self.path.is_dir(),
                &[],
                28.,
                0.,
            ))
            .child(
                div()
                    .max_w(px(180.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(name),
            )
    }
}

fn menu_separator(id: &'static str, p: Palette) -> AnyElement {
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .h(px(FOLDER_MENU_SEPARATOR_ROW))
        .mx(px(6.))
        .bg(p.line)
        .into_any_element()
}

fn section_label(label: &'static str, p: Palette) -> AnyElement {
    div()
        .h(px(FOLDER_MENU_LABEL_ROW))
        .flex()
        .items_center()
        .px(px(6.))
        .text_size(px(11.))
        .text_color(p.fg)
        .child(label)
        .into_any_element()
}

/// Theme-matched artwork for one folder entry, with its tags riding along.
fn entry_icon(
    path: Option<&std::path::Path>,
    kind: FolderEntryKind,
    tags: &[String],
    size: f32,
    angle: f32,
) -> AnyElement {
    crate::file_icons::icon(path, kind == FolderEntryKind::Directory, tags, size, angle)
}

const FAN_LABEL_W: f32 = 112.;
const FAN_LABEL_H: f32 = 24.;
const FAN_LABEL_RADIUS: f32 = 8.;
const FAN_LABEL_GAP: f32 = 12.;

#[derive(Clone, Debug)]
pub(super) struct FanItem {
    pub index: usize,
    pub icon: Bounds<Pixels>,
    /// The label rectangle before its own rotation. Its center is already the
    /// rotated center, so the icon and label form one rigid group.
    pub label: Bounds<Pixels>,
    /// Clockwise degrees for the icon, following the fan tangent.
    pub angle: f32,
    /// The label's own angle. Both bottom and side labels turn with the icon
    /// so the pair stays one rigid card.
    pub label_angle: f32,
}

impl FanItem {
    #[cfg(test)]
    pub fn icon_extent(&self) -> Bounds<Pixels> {
        rotated_bounds(self.icon, self.angle)
    }

    #[cfg(test)]
    pub fn label_extent(&self) -> Bounds<Pixels> {
        rotated_bounds(self.label, self.label_angle)
    }
}

fn rotated_bounds(bounds: Bounds<Pixels>, degrees: f32) -> Bounds<Pixels> {
    let center = bounds.center();
    let (sin, cos) = degrees.to_radians().sin_cos();
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    let half_w = (w * cos.abs() + h * sin.abs()) / 2.;
    let half_h = (w * sin.abs() + h * cos.abs()) / 2.;
    Bounds::new(
        point(
            px(f32::from(center.x) - half_w),
            px(f32::from(center.y) - half_h),
        ),
        size(px(half_w * 2.), px(half_h * 2.)),
    )
}

/// One uniform row advance. Every card travels the same distance along the
/// fan; the outer end is no longer compressed to fake convergence.
fn fan_step(position: DockPosition, icon: f32) -> f32 {
    match position {
        DockPosition::Bottom => (icon * 1.12).max(58.),
        // Side cards carry a compact label strip under the icon (its width
        // hugs the icon), so the pitch only needs the strip's rotated extent
        // plus a small margin — far tighter than the old full-plate labels.
        DockPosition::Left | DockPosition::Right => (icon * 1.1 + 28.).max(88.),
    }
}

/// The side label is a strip as wide as its icon plus a little breathing
/// room; fitting the text inside keeps the rotated group one rigid card.
fn side_label_width(icon: f32) -> f32 {
    icon + 24.
}

/// Vertical rise added per card step; used quadratically so the row starts
/// horizontal and bends upward towards the outer end. Kept small so the side
/// fan reads as a shallow arc rather than a tall quarter-circle.
fn side_rise_step(icon: f32) -> f32 {
    icon * 0.34 + 6.
}

/// The side fan sits this much closer to its dock tile now that the label is
/// a compact strip instead of a full-width plate.
fn side_base_offset(icon: f32) -> f32 {
    icon / 2. + 10. + side_label_width(icon) / 2.
}
/// Positive means the fan bends right; negative means it bends left.
fn fan_direction(
    position: DockPosition,
    anchor: Point<Pixels>,
    viewport: Size<Pixels>,
) -> f32 {
    match position {
        DockPosition::Bottom => {
            if f32::from(anchor.x) < f32::from(viewport.width) / 2. {
                -1.
            } else {
                1.
            }
        }
        DockPosition::Left => 1.,
        DockPosition::Right => -1.,
    }
}

/// Center and clockwise degrees for each visible card. The bottom fan is a
/// uniform vertical fan; the side fans spread horizontally from the clicked
/// tile and curve upward, so the cards start upright and lean into the arc.
fn fan_slots(
    anchor: Point<Pixels>,
    position: DockPosition,
    visible: usize,
    icon: f32,
    viewport: Size<Pixels>,
) -> Vec<(f32, f32, f32)> {
    let visible = visible.max(1);
    let direction = fan_direction(position, anchor, viewport);
    let span = (visible - 1) as f32;
    let t = |index: usize| {
        if visible <= 1 {
            0.
        } else {
            index as f32 / span
        }
    };
    match position {
        DockPosition::Bottom => {
            let step = fan_step(position, icon);
            // A true circular arc: the total bend adapts to the count, every
            // card rides one circle, and its rotation is the arc tangent. Near
            // an output edge the arc straightens by bisection instead of
            // moving its foot away from the folder or dropping cards.
            let distance = visible as f32 * step;
            let lateral = if direction > 0. {
                (f32::from(viewport.width) - 8. - FAN_LABEL_W / 2. - f32::from(anchor.x)).max(0.)
            } else {
                (f32::from(anchor.x) - 8. - FAN_LABEL_W / 2.).max(0.)
            };
            let wanted = (6. + visible as f32).min(16.).to_radians();
            let mut low = 0.;
            let mut high = wanted;
            for _ in 0..18 {
                let mid = (low + high) / 2.;
                let outset = distance / mid * (1. - mid.cos())
                    + (icon / 2. + FAN_LABEL_GAP + FAN_LABEL_W) * mid.cos()
                    + FAN_LABEL_H / 2. * mid.sin();
                if outset <= lateral {
                    low = mid;
                } else {
                    high = mid;
                }
            }
            let total = low.max(1e-4);
            let radius = distance / total;
            (0..visible)
                .map(|index| {
                    let travelled = (index + 1) as f32 * step;
                    let theta = travelled / radius;
                    (
                        f32::from(anchor.x) + direction * radius * (1. - theta.cos()),
                        f32::from(anchor.y) - radius * theta.sin() - icon / 2.,
                        direction * theta.to_degrees(),
                    )
                })
                .collect()
        }
        DockPosition::Left | DockPosition::Right => {
            let step = fan_step(position, icon);
            let rise_step = side_rise_step(icon);
            let rise = span * rise_step;
            let base_x =
                f32::from(anchor.x) + direction * side_base_offset(icon);
            // The end tilt is the curve's own tangent at the outer end, so
            // the cards lean exactly into the parabola they ride.
            let end_tilt =
                f32::atan2(2. * rise_step, step).to_degrees();
            (0..visible)
                .map(|index| {
                    let t = t(index);
                    // Flat start: the horizontal step is constant while the
                    // upward rise grows quadratically.
                    (
                        base_x + direction * step * index as f32,
                        f32::from(anchor.y) - rise * t * t,
                        direction * end_tilt * t,
                    )
                })
                .collect()
        }
    }
}

pub(super) fn fan_capacity(
    anchor: Point<Pixels>,
    position: DockPosition,
    icon: f32,
    viewport: Size<Pixels>,
) -> usize {
    match position {
        DockPosition::Bottom => {
            let step = fan_step(position, icon);
            // Reserve one extra row for the terminal reveal button / stack.
            let available = (f32::from(anchor.y) - icon - 6. - step).max(1.);
            ((available / step).floor() as usize).clamp(1, 10)
        }
        DockPosition::Left | DockPosition::Right => {
            let step = fan_step(position, icon);
            let rise = side_rise_step(icon);
            let direction = fan_direction(position, anchor, viewport);
            let base = f32::from(anchor.x) + direction * side_base_offset(icon);
            let label_half = side_label_width(icon) / 2.;
            let columns = if direction > 0. {
                (f32::from(viewport.width) - 8. - label_half - base).max(0.) / step
            } else {
                (base - 8. - label_half).max(0.) / step
            };
            // Leave room for the horizontal terminal button / stack, which sits
            // four rise steps above the last card.
            let vertical = ((f32::from(anchor.y) - icon / 2. - 8. - 4. * rise)
                .max(0.)
                / rise)
                .floor();
            ((columns.floor().min(vertical) as usize) + 1).clamp(1, 10)
        }
    }
}

/// Estimated painted width of a label at its 12pt size: the same weights the
/// fitter uses, so a content-sized plate fits its text without truncation.
fn label_text_width(text: &str) -> f32 {
    text.chars().map(|c| if c.is_ascii() { 7. } else { 13. }).sum()
}

#[allow(clippy::too_many_arguments)]
pub(super) fn fan_geometry(
    anchor: Point<Pixels>,
    position: DockPosition,
    count: usize,
    visible: usize,
    offset: usize,
    icon: f32,
    viewport: Size<Pixels>,
    labels: &[&str],
) -> (Vec<FanItem>, Bounds<Pixels>) {
    let visible = visible.min(count.saturating_sub(offset)).min(10);
    if visible == 0 {
        return (Vec::new(), Bounds::new(anchor, size(px(icon), px(icon))));
    }
    let direction = fan_direction(position, anchor, viewport);
    let label_side = match position {
        DockPosition::Bottom => -direction,
        DockPosition::Left => 1.,
        DockPosition::Right => -1.,
    };
    let mut items = Vec::with_capacity(visible);
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;
    for (step, (center_x, center_y, angle)) in
        fan_slots(anchor, position, visible, icon, viewport)
            .into_iter()
            .enumerate()
    {
        // The bottom fan's label plate grows to fit its name (clavis-style),
        // so long file names are never truncated; the side fans keep their
        // compact icon-hugging strips.
        let label_width = match position {
            DockPosition::Bottom => labels
                .get(step)
                .map(|name| (label_text_width(name) + 24.).clamp(64., 240.))
                .unwrap_or(FAN_LABEL_W),
            DockPosition::Left | DockPosition::Right => side_label_width(icon),
        };
        let label_angle = angle;
        let (local_x, local_y) = match position {
            DockPosition::Bottom => (
                label_side * (icon / 2. + FAN_LABEL_GAP + label_width / 2.),
                0.,
            ),
            // The side label hangs off the card's local bottom edge, directly
            // below the upright icon, and turns with the icon as one card.
            DockPosition::Left | DockPosition::Right => {
                (0., icon / 2. + 4. + FAN_LABEL_H / 2.)
            }
        };
        let (sin, cos) = angle.to_radians().sin_cos();
        let label_center = point(
            px(center_x + local_x * cos - local_y * sin),
            px(center_y + local_x * sin + local_y * cos),
        );
        let mut icon_bounds = Bounds::new(
            point(px(center_x - icon / 2.), px(center_y - icon / 2.)),
            size(px(icon), px(icon)),
        );
        let mut label_bounds = Bounds::new(
            point(
                px(f32::from(label_center.x) - label_width / 2.),
                px(f32::from(label_center.y) - FAN_LABEL_H / 2.),
            ),
            size(px(label_width), px(FAN_LABEL_H)),
        );
        // Move the icon and label together if the rotated group would cross an
        // output edge. Clamping them separately would break the rigid pair.
        let icon_extent = rotated_bounds(icon_bounds, angle);
        let label_extent = rotated_bounds(label_bounds, label_angle);
        let group_left = f32::from(icon_extent.left()).min(f32::from(label_extent.left()));
        let group_top = f32::from(icon_extent.top()).min(f32::from(label_extent.top()));
        let group_right = f32::from(icon_extent.right()).max(f32::from(label_extent.right()));
        let group_bottom = f32::from(icon_extent.bottom()).max(f32::from(label_extent.bottom()));
        let mut shift_x = 0.;
        let mut shift_y = 0.;
        if group_left < 6. {
            shift_x = 6. - group_left;
        } else if group_right > f32::from(viewport.width) - 6. {
            shift_x = f32::from(viewport.width) - 6. - group_right;
        }
        if group_top < 6. {
            shift_y = 6. - group_top;
        } else if group_bottom > f32::from(viewport.height) - 6. {
            shift_y = f32::from(viewport.height) - 6. - group_bottom;
        }
        icon_bounds.origin.x += px(shift_x);
        icon_bounds.origin.y += px(shift_y);
        label_bounds.origin.x += px(shift_x);
        label_bounds.origin.y += px(shift_y);
        let icon_extent = rotated_bounds(icon_bounds, angle);
        let label_extent = rotated_bounds(label_bounds, label_angle);
        min_x = min_x
            .min(f32::from(icon_extent.left()))
            .min(f32::from(label_extent.left()));
        min_y = min_y
            .min(f32::from(icon_extent.top()))
            .min(f32::from(label_extent.top()));
        max_x = max_x
            .max(f32::from(icon_extent.right()))
            .max(f32::from(label_extent.right()));
        max_y = max_y
            .max(f32::from(icon_extent.bottom()))
            .max(f32::from(label_extent.bottom()));
        items.push(FanItem {
            index: offset + step,
            icon: icon_bounds,
            label: label_bounds,
            angle,
            label_angle,
        });
    }
    (
        items,
        Bounds::from_corners(point(px(min_x), px(min_y)), point(px(max_x), px(max_y))),
    )
}

fn fan_extension_delta(
    position: DockPosition,
    icon: f32,
    angle: f32,
    beyond: f32,
) -> (f32, f32) {
    match position {
        DockPosition::Bottom => {
            // Continue the same circle: the step rides the tangent at the
            // card's own angle, so the terminal group stays on the arc.
            let step = fan_step(position, icon);
            let radians = angle.to_radians();
            (
                step * beyond * radians.sin(),
                -step * beyond * radians.cos(),
            )
        }
        DockPosition::Left => (
            fan_step(position, icon) * beyond,
            -2. * side_rise_step(icon) * beyond,
        ),
        DockPosition::Right => (
            -fan_step(position, icon) * beyond,
            -2. * side_rise_step(icon) * beyond,
        ),
    }
}

/// Center and shared angle of a virtual item on the terminal tangent. The
/// reveal button and its label use this same pair, so the terminal group can
/// never drift apart.
pub(super) fn fan_extension(
    anchor: Point<Pixels>,
    position: DockPosition,
    visible: usize,
    icon: f32,
    viewport: Size<Pixels>,
    beyond: f32,
) -> (Point<Pixels>, f32) {
    let Some((x, y, angle)) = fan_slots(anchor, position, visible, icon, viewport).pop()
    else {
        return (anchor, 0.);
    };
    let beyond = beyond.max(0.);
    let (dx, dy) = fan_extension_delta(position, icon, angle, beyond);
    (point(px(x + dx), px(y + dy)), angle)
}

/// Terminal tangent measured from the card that was actually painted. Edge
/// avoidance may have moved the last card, so the terminal group follows it
/// instead of the unclamped layout slot.
pub(super) fn fan_extension_after(
    last: &FanItem,
    position: DockPosition,
    icon: f32,
    beyond: f32,
) -> (Point<Pixels>, f32) {
    let beyond = beyond.max(0.);
    let center = last.icon.center();
    let (dx, dy) = fan_extension_delta(position, icon, last.angle, beyond);
    (
        point(
            px(f32::from(center.x) + dx),
            px(f32::from(center.y) + dy),
        ),
        last.angle,
    )
}

/// Center of the terminal tangent, for callers that do not need its angle.
#[cfg(test)]
pub(super) fn fan_extension_center(
    anchor: Point<Pixels>,
    position: DockPosition,
    visible: usize,
    icon: f32,
    viewport: Size<Pixels>,
    beyond: f32,
) -> Point<Pixels> {
    fan_extension(anchor, position, visible, icon, viewport, beyond).0
}

impl ShellView {
    /// Retracts the fan along its opening curve and lets the host remove the
    /// window once the icons have stacked back onto the folder tile.
    pub(super) fn dismiss_folder_fan(&mut self, cx: &mut Context<Self>) {
        if self.folder_drag_layer {
            self.close_folder_now(cx);
            return;
        }
        if !self.folder_fan_active {
            self.invoke(Action::Close, cx);
            return;
        }
        if self.folder_fan_closing.is_some() || self.folder_fan_closed {
            return;
        }
        if cfg!(test) || self.state.prefs.reduce_motion {
            self.folder_fan_closed = true;
            self.invoke(Action::Close, cx);
            return;
        }
        self.folder_fan_closing = Some(std::time::Instant::now());
        cx.notify();
    }

    /// Keeps the folder surface alive for the duration of a file drag, but
    /// makes it invisible and empty. The surface still owns the implicit
    /// pointer grab so it can paint the drag preview anywhere on the output;
    /// each pointer event is forwarded to the Dock window below.
    pub(super) fn begin_folder_drag(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.folder_drag_layer {
            return;
        }
        self.folder_drag_layer = true;
        self.folder_fan_closed = false;
        self.folder_fan_closing = None;
        self.host.update(cx, |host, _| host.folder_drag_layer = true);
        window.set_background_appearance(WindowBackgroundAppearance::Transparent);
        window.refresh();
        cx.notify();
    }

    /// Translates a fullscreen folder-overlay point into Dock surface coordinates.
    fn dock_drag_position(&self, position: Point<Pixels>, window: &Window) -> Option<Point<Pixels>> {
        let host = &self.state;
        let apps = host.dock_apps();
        let (width, height) = host
            .desktop
            .outputs
            .get(&self.output)
            .and_then(|o| o.logical)
            .map(|o| (o.width as f32, o.height as f32))
            .unwrap_or_else(|| {
                let viewport = window.viewport_size();
                (f32::from(viewport.width), f32::from(viewport.height))
            });
        let metrics = crate::app::panels::dock_metrics(host, &apps, width, height);
        let (x, y) = crate::app::panels::dock_surface_origin(host, &metrics, width, height);
        Some(point(position.x - px(x), position.y - px(y)))
    }

    /// Routes the folder surface's pointer move to the Dock window at the same
    /// screen position. The Dock then runs its own hover, magnification and
    /// badge logic instead of receiving a synthesized target from the overlay.
    pub(super) fn forward_drag_move(
        &self,
        position: Point<Pixels>,
        modifiers: Modifiers,
        window: &Window,
        cx: &mut App,
    ) {
        let Some(position) = self.dock_drag_position(position, window) else {
            return;
        };
        let dock = self.host.read(cx).dock_window(&self.output);
        let Some(dock) = dock else {
            return;
        };
        let _ = dock.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position,
                pressed_button: Some(MouseButton::Left),
                modifiers,
            }),
            cx,
        );
    }

    fn forward_drag_up(
        &self,
        position: Point<Pixels>,
        modifiers: Modifiers,
        window: &Window,
        cx: &mut App,
    ) {
        let Some(position) = self.dock_drag_position(position, window) else {
            return;
        };
        let dock = self.host.read(cx).dock_window(&self.output);
        let Some(dock) = dock else {
            return;
        };
        let _ = dock.dispatch_event(
            PlatformInput::MouseUp(MouseUpEvent {
                button: MouseButton::Left,
                position,
                modifiers,
                click_count: 1,
            }),
            cx,
        );
    }

    /// Ends the invisible drag layer: release the Dock drop first, then remove
    /// the overlay so the preview disappears with the drag.
    pub(super) fn finish_folder_drag_layer(
        &mut self,
        position: Point<Pixels>,
        modifiers: Modifiers,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if !self.folder_drag_layer {
            return;
        }
        self.forward_drag_up(position, modifiers, window, cx);
        self.folder_drag_layer = false;
        self.invoke(Action::Close, cx);
    }

    /// Dismisses the fan without the retract animation. Used when a drag or the
    /// window lifetime ends before the regular close path can animate.
    pub(super) fn close_folder_now(&mut self, cx: &mut Context<Self>) {
        if self.folder_fan_closed && !self.folder_drag_layer {
            return;
        }
        self.folder_fan_closed = true;
        self.folder_fan_closing = None;
        self.folder_drag_layer = false;
        self.host.update(cx, |host, _| host.folder_drag_layer = false);
        self.invoke(Action::Close, cx);
    }

    pub(super) fn dock_folder_fan(
        &mut self,
        host: &ViewState,
        root: &std::path::Path,
        width: f32,
        height: f32,
        p: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(navigation) = host.folder_navigation.as_ref() else {
            return div().into_any_element();
        };
        let current = navigation.current.clone();
        if self.folder_fan_root.as_deref() != Some(root) {
            self.folder_fan_root = Some(root.to_path_buf());
            self.folder_fan_offset = 0;
            self.folder_fan_started = None;
            self.folder_fan_closing = None;
            self.folder_fan_closed = false;
        }
        let (fan_open, fan_opacity, fan_animating) = if self.folder_fan_closed {
            (0., 0., false)
        } else if let Some(started) = self.folder_fan_closing {
            if cfg!(test) || host.prefs.reduce_motion {
                (0., 0., false)
            } else {
                fan_close_progress(started.elapsed())
            }
        } else if cfg!(test) || host.prefs.reduce_motion {
            (1., 1., false)
        } else {
            let now = std::time::Instant::now();
            let started = *self.folder_fan_started.get_or_insert(now);
            let (open, active) = fan_open_progress(now.duration_since(started));
            (open, open.clamp(0., 1.), active)
        };
        if fan_animating {
            window.request_animation_frame();
        }
        if self.folder_fan_closing.is_some() && !fan_animating && !self.folder_fan_closed {
            self.folder_fan_closed = true;
            self.invoke(Action::Close, cx);
        }
        let closing = self.folder_fan_closing.is_some() || self.folder_fan_closed;
        let Some((anchor_x, anchor_y)) =
            crate::app::panels::folder_tile_center(host, root, width, height)
        else {
            return div().into_any_element();
        };
        self.folder_fan_active = true;
        let anchor = point(px(anchor_x), px(anchor_y));
        let viewport = size(px(width), px(height));
        let icon_size = host.prefs.dock_size as f32;
        let entries = host
            .folder_snapshots
            .get(&current)
            .map(|snapshot| snapshot.entries.as_slice())
            .unwrap_or(&[]);
        let capacity = fan_capacity(anchor, host.prefs.dock_position, icon_size, viewport);
        self.folder_fan_offset = self
            .folder_fan_offset
            .min(entries.len().saturating_sub(capacity));
        let visible = capacity.min(entries.len().saturating_sub(self.folder_fan_offset));
        let names: Vec<&str> = entries
            .iter()
            .skip(self.folder_fan_offset)
            .take(visible)
            .map(|entry| entry.name.as_str())
            .collect();
        let (items, _fan_bounds) = fan_geometry(
            anchor,
            host.prefs.dock_position,
            entries.len(),
            visible,
            self.folder_fan_offset,
            icon_size,
            viewport,
            &names,
        );
        let terminal = |beyond: f32| {
            let (center, angle) = items
                .last()
                .map(|last| {
                    fan_extension_after(
                        last,
                        host.prefs.dock_position,
                        icon_size,
                        beyond,
                    )
                })
                .unwrap_or_else(|| {
                    fan_extension(
                        anchor,
                        host.prefs.dock_position,
                        visible,
                        icon_size,
                        viewport,
                        beyond,
                    )
                });
            (
                point(
                    px(anchor_x
                        + (f32::from(center.x) - anchor_x) * fan_open),
                    px(anchor_y
                        + (f32::from(center.y) - anchor_y) * fan_open),
                ),
                angle * fan_open,
            )
        };
        let view = cx.entity();
        let close = div()
            .absolute()
            .inset_0()
            .bg(rgba(0x00000001))
            .on_mouse_down(MouseButton::Left, {
                let view = view.clone();
                move |_, _, cx| view.update(cx, |this, cx| this.dismiss_folder_fan(cx))
            })
            .on_mouse_down(MouseButton::Right, {
                let view = view.clone();
                move |_, _, cx| view.update(cx, |this, cx| this.dismiss_folder_fan(cx))
            });
        let mut overlay = div().relative().size_full().child(close);
        // Frost regions follow the name labels only: the compositor blurs
        // behind each label plate (and the terminal label), while icons and
        // the air between cards stay sharp.
        let mut glass: Vec<Bounds<Pixels>> = Vec::new();
        // The overflow folds into a living stack past the last card: up to
        // four upright layers ride the arc's own tangent line behind the
        // reveal control, each dimmed and shrunk by its depth until it melts
        // into the backdrop, and the wheel pulls them out onto the arc one by
        // one. The pile never leans; it only extends the curve it grows from.
        // (`beyond` below is measured in card steps, so the pixel distances
        // divide by the step.)
        let labels_left = items.first().is_some_and(|item| {
            f32::from(item.label.center().x) < f32::from(item.icon.center().x)
        });
        const STACK_LAYERS: usize = 4;
        let step = fan_step(host.prefs.dock_position, icon_size);
        let stack_head = 8.;
        let stack_pacing = 8.;
        let stack_start = self.folder_fan_offset + visible;
        let stack_count = entries.len().saturating_sub(stack_start).min(STACK_LAYERS);
        let has_more = stack_count > 0;
        let side_fan = matches!(
            host.prefs.dock_position,
            DockPosition::Left | DockPosition::Right
        );
        for depth in 1..=stack_count {
            let Some(entry) = entries.get(stack_start + depth - 1) else {
                break;
            };
            let beyond = (stack_head + (depth - 1) as f32 * stack_pacing) / step;
            let (center, _) = terminal(beyond);
            let scale = 0.94_f32.powi(depth as i32);
            // A steady fade to nothing: the deepest layer is a whisper on the
            // tangent line rather than a second stack of icons.
            let opacity = (1. - depth as f32 / (STACK_LAYERS as f32 + 1.)).powf(1.3);
            let glyph = icon_size * (0.7 + 0.3 * fan_open) * scale;
            let x = anchor_x + (f32::from(center.x) - anchor_x) * fan_open;
            let y = anchor_y + (f32::from(center.y) - anchor_y) * fan_open;
            let layer = div()
                .id(SharedString::from(format!("folder-fan-stack-{depth}")))
                .debug_selector(|| format!("folder-fan-stack-{depth}"))
                .absolute()
                .left(px(x - glyph / 2.))
                .top(px(y - glyph / 2.))
                .size(px(glyph))
                .opacity(opacity)
                .child(entry_icon(
                    Some(&entry.path),
                    entry.kind,
                    &entry.tags,
                    glyph,
                    0.,
                ));
            overlay = overlay.child(layer);
        }
        for item in &items {
            let Some(entry) = entries.get(item.index) else {
                continue;
            };
            let id = format!("folder-fan-{}", item.index);
            let path = entry.path.clone();
            let name = entry.name.clone();
            let kind = entry.kind;
            let reveal = if kind == FolderEntryKind::Directory {
                path.clone()
            } else {
                path.parent().unwrap_or(&path).to_path_buf()
            };
            let icon_host = self.host.clone();
            let label_host = self.host.clone();
            let label_click_host = self.host.clone();
            let icon_click_view = view.clone();
            let icon_reveal_view = view.clone();
            let label_click_view = view.clone();
            let label_path = path.clone();
            let final_icon_center = item.icon.center();
            // clavis-style presentation: the icons grow in from 70% while the
            // labels only reveal once the cards have mostly arrived.
            let icon_scale = 0.7 + 0.3 * fan_open;
            let label_reveal = ((fan_open - 0.4) / 0.6).clamp(0., 1.);
            let icon_center = point(
                px(anchor_x + (f32::from(final_icon_center.x) - anchor_x) * fan_open),
                px(anchor_y + (f32::from(final_icon_center.y) - anchor_y) * fan_open),
            );
            let icon_left = f32::from(icon_center.x) - icon_size / 2.;
            let icon_top = f32::from(icon_center.y) - icon_size / 2.;
            let icon_angle = item.angle * fan_open;
            let final_label_center = item.label.center();
            let label_center = point(
                px(anchor_x
                    + (f32::from(final_label_center.x) - anchor_x) * fan_open),
                px(anchor_y
                    + (f32::from(final_label_center.y) - anchor_y) * fan_open),
            );
            let label_left = f32::from(label_center.x) - f32::from(item.label.size.width) / 2.;
            let label_top = f32::from(label_center.y) - f32::from(item.label.size.height) / 2.;
            let label_angle = (item.label_angle * fan_open).to_radians();
            let label_text = fit_label(
                &name,
                (f32::from(item.label.size.width) - 24.).max(1.),
            );
            let mut icon = div()
                .id(SharedString::from(id.clone()))
                .debug_selector({
                    let id = id.clone();
                    move || id.clone()
                })
                .absolute()
                .left(px(icon_left))
                .top(px(icon_top))
                .size(px(icon_size))
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(entry_icon(
                            Some(&path),
                            kind,
                            &entry.tags,
                            icon_size * icon_scale,
                            icon_angle,
                        )),
                );
            let drag_view = view.clone();
            if !closing {
                icon = icon.on_drag(
                    FolderEntryDrag {
                        path: path.clone(),
                    },
                    move |drag, _offset, window, cx| {
                        // Keep the surface as an invisible drag layer so the
                        // preview follows the pointer; the Dock gets the same
                        // position through forwarded events.
                        drag_view.update(cx, |this, cx| this.begin_folder_drag(window, cx));
                        cx.new(|_| FolderDragPreview {
                            path: drag.path.clone(),
                            background: p.panel.blend(p.panel),
                            foreground: p.fg,
                            shadow: p.shadow,
                        })
                    },
                );
            }
            let icon = icon
                .on_click(move |_, _, cx| {
                    if closing {
                        return;
                    }
                    let path = path.clone();
                    icon_host.update(cx, |host, cx| {
                        if kind == FolderEntryKind::Directory {
                            host.handle(Action::NavigateFolder(path.clone()), cx);
                        } else {
                            host.handle(
                                Action::Backend(backend::Command::OpenPath(path.clone())),
                                cx,
                            );
                        }
                    });
                    if kind != FolderEntryKind::Directory {
                        icon_click_view.update(cx, |this, cx| this.dismiss_folder_fan(cx));
                    }
                })
                .on_mouse_down(MouseButton::Right, move |_, _, cx| {
                    cx.stop_propagation();
                    if closing {
                        return;
                    }
                    label_host.update(cx, |host, cx| {
                        host.handle(
                            Action::Backend(backend::Command::OpenPath(reveal.clone())),
                            cx,
                        )
                    });
                    icon_reveal_view.update(cx, |this, cx| this.dismiss_folder_fan(cx));
                });
            let label = div()
                .id(SharedString::from(format!("folder-fan-label-{}", item.index)))
                .absolute()
                .left(px(label_left))
                .top(px(label_top))
                .w(item.label.size.width)
                .h(item.label.size.height)
                .flex()
                .items_center()
                .opacity(label_reveal)
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, _, cx| {
                    if closing {
                        return;
                    }
                    let path = label_path.clone();
                    label_click_host.update(cx, |host, cx| {
                        if kind == FolderEntryKind::Directory {
                            host.handle(Action::NavigateFolder(path.clone()), cx);
                        } else {
                            host.handle(
                                Action::Backend(backend::Command::OpenPath(path.clone())),
                                cx,
                            );
                        }
                    });
                    if kind != FolderEntryKind::Directory {
                        label_click_view.update(cx, |this, cx| this.dismiss_folder_fan(cx));
                    }
                })
                .child(fan_label_background(label_angle, p))
                .child(rotate_text(
                    div()
                        .px(px(12.))
                        .text_color(p.fg)
                        .text_size(px(12.))
                        .whitespace_nowrap()
                        .child(label_text)
                        .into_any_element(),
                    label_center,
                    label_angle,
                ));
            overlay = overlay.child(icon).child(label);
            // The frost follows the name plate only, and only once the plate
            // is nearly revealed: the mask cannot fade with the ghost labels.
            if label_reveal > 0.9 {
                rotated_plate_rows(
                    &mut glass,
                    label_center,
                    item.label.size,
                    label_angle,
                    FAN_LABEL_RADIUS,
                );
            }
        }
        // The back button is anchored to the folder tile, not to the fan
        // geometry: navigating into deeper folders changes the fan contents
        // and therefore the fan bounds, so deriving the button from them made
        // it drift around. It rides on the fan-facing side of the tile but is
        // raised above the Dock row so it never covers the Dock icons.
        let fan_bend = fan_direction(host.prefs.dock_position, anchor, viewport);
        let (back_x, back_y) = match host.prefs.dock_position {
            DockPosition::Bottom => {
                (
                    if fan_bend < 0. {
                        anchor_x - icon_size / 2. - 70.
                    } else {
                        anchor_x + icon_size / 2. + 8.
                    },
                    anchor_y - icon_size / 2. - 68.,
                )
            }
            DockPosition::Left => (
                anchor_x + icon_size / 2. + 8.,
                anchor_y - icon_size / 2. - 68.,
            ),
            DockPosition::Right => (
                anchor_x - icon_size / 2. - 70.,
                anchor_y - icon_size / 2. - 68.,
            ),
        };
        let back_left = anchor_x + (back_x - anchor_x) * fan_open;
        let back_top = anchor_y + (back_y - anchor_y) * fan_open;
        if !navigation.history.is_empty() {
            let back = self.button("folder-fan-back", "返回", Action::FolderBack, p);
            overlay = overlay.child(
                div()
                    .absolute()
                    .left(px(back_left.clamp(8., (width - 90.).max(8.))))
                    .top(px(back_top.clamp(8., (height - 40.).max(8.))))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                    .child(back),
            );
        }
        let reveal_size = icon_size * 0.72;
        let (reveal_center, reveal_angle) = terminal(if has_more {
            // Clear the whole pile: its outermost edge plus the button's own
            // radius and a breathing gap.
            (stack_head
                + (stack_count - 1) as f32 * stack_pacing
                + icon_size / 2.
                + reveal_size / 2.
                + 12.)
                / step
        } else {
            1.0
        });
        let label_w = FAN_LABEL_W + 36.;
        let reveal_side = match host.prefs.dock_position {
            DockPosition::Bottom => {
                if labels_left {
                    -1.
                } else {
                    1.
                }
            }
            DockPosition::Left => 1.,
            DockPosition::Right => -1.,
        };
        let label_offset = reveal_size / 2. + 8. + label_w / 2.;
        let reveal_label_deg = reveal_angle;
        let (sin, cos) = reveal_label_deg.to_radians().sin_cos();
        let mut button_center = (f32::from(reveal_center.x), f32::from(reveal_center.y));
        let button_half = reveal_size / 2.;
        let (local_x, local_y) = if side_fan {
            (0., button_half + 4. + FAN_LABEL_H / 2.)
        } else {
            (reveal_side * label_offset, 0.)
        };
        let mut label_center = (
            button_center.0 + local_x * cos - local_y * sin,
            button_center.1 + local_x * sin + local_y * cos,
        );
        // The reveal button and its filename are one rigid group. Shift both
        // together so the terminal pair never separates at an output edge.
        let half_w = (label_w * cos.abs() + FAN_LABEL_H * sin.abs()) / 2.;
        let half_h = (label_w * sin.abs() + FAN_LABEL_H * cos.abs()) / 2.;
        let group_left = (button_center.0 - button_half).min(label_center.0 - half_w);
        let group_top = (button_center.1 - button_half).min(label_center.1 - half_h);
        let group_right = (button_center.0 + button_half).max(label_center.0 + half_w);
        let group_bottom = (button_center.1 + button_half).max(label_center.1 + half_h);
        let shift_x = if group_left < 8. {
            8. - group_left
        } else if group_right > width - 8. {
            width - 8. - group_right
        } else {
            0.
        };
        let shift_y = if group_top < 8. {
            8. - group_top
        } else if group_bottom > height - 8. {
            height - 8. - group_bottom
        } else {
            0.
        };
        button_center.0 += shift_x;
        button_center.1 += shift_y;
        label_center.0 += shift_x;
        label_center.1 += shift_y;
        let reveal_left = button_center.0 - button_half;
        let reveal_top = button_center.1 - button_half;
        // One glass region: the terminal name plate's own silhouette.
        rotated_plate_rows(
            &mut glass,
            point(px(label_center.0), px(label_center.1)),
            size(px(label_w), px(FAN_LABEL_H)),
            reveal_label_deg.to_radians(),
            FAN_LABEL_RADIUS,
        );
        let reveal_host = self.host.clone();
        let label_host = self.host.clone();
        let reveal_view = view.clone();
        let reveal_label_view = view.clone();
        let label_current = current.clone();
        // clavis attaches its terminal action the same way: the round mark is
        // a card's icon slot, not a separate floating pill, so the button and
        // its name plate read as one terminal card. The glyph is the
        // open-in-new mark the shell ships, not a text arrow, so the terminal
        // stays in the same visual family as the file icons beside it.
        let reveal_glyph = (reveal_size * 0.5).max(14.);
        overlay = overlay.child(
            div()
                .id("folder-fan-reveal")
                .debug_selector(|| "folder-fan-reveal".into())
                .absolute()
                .left(px(reveal_left))
                .top(px(reveal_top))
                .size(px(reveal_size))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(p.panel.blend(p.panel))
                .border_1()
                .border_color(p.line)
                .shadow(vec![BoxShadow {
                    color: p.shadow,
                    offset: point(px(0.), px(3.)),
                    blur_radius: px(10.),
                    spread_radius: px(0.),
                }])
                .cursor_pointer()
                .hover(move |style| style.bg(p.panel.blend(p.panel).blend(p.line)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .child(self.symbol("external-link", reveal_glyph, p))
                .on_click(move |_, _, cx| {
                    if closing {
                        return;
                    }
                    reveal_host.update(cx, |host, cx| {
                        host.handle(
                            Action::Backend(backend::Command::OpenPath(current.clone())),
                            cx,
                        )
                    });
                    reveal_view.update(cx, |this, cx| this.dismiss_folder_fan(cx));
                }),
        );
        let label_left = label_center.0 - label_w / 2.;
        let label_top = label_center.1 - FAN_LABEL_H / 2.;
        let reveal_label_center = point(px(label_center.0), px(label_center.1));
        let reveal_label_angle = reveal_label_deg.to_radians();
        overlay = overlay.child(
            div()
                .id("folder-fan-reveal-label")
                .debug_selector(|| "folder-fan-reveal-label".into())
                .absolute()
                .left(px(label_left))
                .top(px(label_top))
                .w(px(label_w))
                .h(px(FAN_LABEL_H))
                .flex()
                .items_center()
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .child(fan_label_background(reveal_label_angle, p))
                .child(rotate_text(
                    div()
                        .px(px(12.))
                        .text_color(p.fg)
                        .text_size(px(12.))
                        .whitespace_nowrap()
                        .child("在文件管理器中显示")
                        .into_any_element(),
                    reveal_label_center,
                    reveal_label_angle,
                ))
                .on_click(move |_, _, cx| {
                    if closing {
                        return;
                    }
                    label_host.update(cx, |host, cx| {
                        host.handle(
                            Action::Backend(backend::Command::OpenPath(label_current.clone())),
                            cx,
                        )
                    });
                    reveal_label_view.update(cx, |this, cx| this.dismiss_folder_fan(cx));
                }),
        );
        // The fan's own glass: without this the surface keeps the empty frost
        // region the modal path advertised (it assumes a popup rectangle) and
        // the labels sit on plain transparency instead of frosted glass.
        //
        // Frost cannot fade with the overlay, so it must be revoked before the
        // retract leaves bare blurred silhouettes behind: the terminal plate
        // outlives the per-card labels, and reporting its region at zero
        // opacity flashed a frosted plate on the wallpaper for one frame.
        if p.frosted() && fan_opacity > 0.15 {
            window.set_glass_region(Some(&glass));
        } else {
            window.set_glass_region(Some(&[]));
        }
        let max_offset = entries.len().saturating_sub(capacity);
        overlay
            .opacity(fan_opacity.clamp(0., 1.))
            .on_scroll_wheel(cx.listener(move |this, event: &gpui::ScrollWheelEvent, _, cx| {
                let delta = f32::from(event.delta.pixel_delta(px(20.)).y);
                if delta < 0. {
                    this.folder_fan_offset = (this.folder_fan_offset + 1).min(max_offset);
                } else {
                    this.folder_fan_offset = this.folder_fan_offset.saturating_sub(1);
                }
                cx.notify();
            }))
            .into_any_element()
    }

    pub(super) fn dock_folder_content(
        &self,
        host: &ViewState,
        width: f32,
        height: f32,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(navigation) = host.folder_navigation.as_ref() else {
            return div().into_any_element();
        };
        let current = navigation.current.clone();
        let snapshot = host.folder_snapshots.get(&current);
        let entries = snapshot
            .map(|snapshot| snapshot.entries.as_slice())
            .unwrap_or(&[]);
        let error = host.folder_errors.get(&current);
        let truncated = snapshot.is_some_and(|snapshot| snapshot.truncated);
        let layout = host
            .dock_folders
            .iter()
            .find(|folder| folder.path == navigation.root)
            .map(|folder| folder.layout)
            .unwrap_or(FolderLayout::Auto);
        let layout = if layout == FolderLayout::Auto {
            crate::app::panels::resolved_folder_layout(host, &navigation.root, width, height)
        } else {
            layout
        };
        let body = if let Some(error) = error {
            self.empty("无法读取文件夹", error, p)
        } else if entries.is_empty() {
            self.empty("文件夹为空", "此文件夹中没有可显示的项目", p)
        } else if layout == FolderLayout::Grid {
            let mut grid = div().flex().flex_wrap().gap_2();
            for (index, entry) in entries.iter().enumerate() {
                grid = grid.child(self.folder_grid_cell(entry, index, p, cx));
            }
            grid.into_any_element()
        } else {
            let mut rows = div().flex().flex_col().gap_1();
            for (index, entry) in entries.iter().enumerate() {
                rows = rows.child(self.folder_list_row(entry, index, p, cx));
            }
            rows.into_any_element()
        };
        let mut footer = div().flex().items_center().justify_between().h(px(34.));
        if truncated {
            footer = footer.child(self.button(
                "folder-show-more",
                "显示剩余项目",
                Action::RevealFolder(current.clone()),
                p,
            ));
        } else {
            footer = footer.child(div());
        }
        footer = footer.child(self.button(
            "folder-reveal-footer",
            "在文件管理器中显示",
            Action::RevealFolder(current.clone()),
            p,
        ));
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_2()
            .child(self.folder_header(host, p))
            .child(
                div()
                    .id("folder-scroll")
                    .debug_selector(|| "folder-scroll".into())
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(body),
            )
            .child(footer)
            .into_any_element()
    }

    fn folder_header(&self, host: &ViewState, p: Palette) -> AnyElement {
        let Some(navigation) = host.folder_navigation.as_ref() else {
            return div().into_any_element();
        };
        let title = navigation
            .current
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| navigation.current.to_string_lossy().into_owned());
        let mut header = div().flex().items_center().gap_2().h(px(34.));
        if !navigation.history.is_empty() {
            header = header.child(self.button("folder-back", "返回", Action::FolderBack, p));
        }
        header = header
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(title),
            )
            .child(self.button(
                "folder-reveal",
                "显示",
                Action::RevealFolder(navigation.current.clone()),
                p,
            ));
        header.into_any_element()
    }

    fn folder_list_row(
        &self,
        entry: &FolderEntry,
        index: usize,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = format!("folder-entry-{index}");
        let selector = id.clone();
        let h = self.host.clone();
        let click_host = h.clone();
        let path = entry.path.clone();
        let name = entry.name.clone();
        let kind = entry.kind;
        let reveal = if kind == FolderEntryKind::Directory {
            path.clone()
        } else {
            path.parent().unwrap_or(&path).to_path_buf()
        };
        let view = cx.entity();
        div()
            .id(SharedString::from(id))
            .debug_selector(move || selector.clone())
            .w_full()
            .flex()
            .items_center()
            .gap_2()
            .h(px(34.))
            .px(px(6.))
            .rounded(px(5.))
            .cursor_pointer()
            .hover(|style| style.bg(p.line))
            .child(entry_icon(Some(&entry.path), kind, &entry.tags, 24., 0.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(name),
            )
            .on_drag(
                FolderEntryDrag {
                    path: path.clone(),
                },
                move |drag, _offset, window, cx| {
                    // The popup becomes an invisible fullscreen drag layer so
                    // the preview follows the pointer everywhere; its pointer
                    // events are forwarded to the Dock.
                    view.update(cx, |this, cx| this.begin_folder_drag(window, cx));
                    cx.new(|_| FolderDragPreview {
                        path: drag.path.clone(),
                        background: p.panel.blend(p.panel),
                        foreground: p.fg,
                        shadow: p.shadow,
                    })
                },
            )
            .on_click(move |_, _, cx| {
                let h = click_host.clone();
                let path = path.clone();
                h.update(cx, |host, cx| {
                    if kind == FolderEntryKind::Directory {
                        host.handle(Action::NavigateFolder(path.clone()), cx);
                    } else {
                        host.handle(
                            Action::Backend(backend::Command::OpenPath(path.clone())),
                            cx,
                        );
                        host.handle(Action::Close, cx);
                    }
                });
            })
            .on_mouse_down(MouseButton::Right, move |_, _, cx| {
                h.update(cx, |host, cx| {
                    host.handle(Action::RevealFolder(reveal.clone()), cx)
                });
            })
            .into_any_element()
    }

    fn folder_grid_cell(
        &self,
        entry: &FolderEntry,
        index: usize,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = format!("folder-entry-{index}");
        let selector = id.clone();
        let h = self.host.clone();
        let click_host = h.clone();
        let path = entry.path.clone();
        let name = entry.name.clone();
        let kind = entry.kind;
        let reveal = if kind == FolderEntryKind::Directory {
            path.clone()
        } else {
            path.parent().unwrap_or(&path).to_path_buf()
        };
        let view = cx.entity();
        div()
            .id(SharedString::from(id))
            .debug_selector(move || selector.clone())
            .w(px(84.))
            .h(px(84.))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_1()
            .rounded(px(8.))
            .cursor_pointer()
            .hover(|style| style.bg(p.line))
            .child(entry_icon(Some(&entry.path), kind, &entry.tags, 40., 0.))
            .child(
                div()
                    .w_full()
                    .text_xs()
                    .text_center()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(name),
            )
            .on_drag(
                FolderEntryDrag {
                    path: path.clone(),
                },
                move |drag, _offset, window, cx| {
                    // The grid popup becomes the same invisible fullscreen drag
                    // layer as the fan and list layouts.
                    view.update(cx, |this, cx| this.begin_folder_drag(window, cx));
                    cx.new(|_| FolderDragPreview {
                        path: drag.path.clone(),
                        background: p.panel.blend(p.panel),
                        foreground: p.fg,
                        shadow: p.shadow,
                    })
                },
            )
            .on_click(move |_, _, cx| {
                let h = click_host.clone();
                let path = path.clone();
                h.update(cx, |host, cx| {
                    if kind == FolderEntryKind::Directory {
                        host.handle(Action::NavigateFolder(path.clone()), cx);
                    } else {
                        host.handle(
                            Action::Backend(backend::Command::OpenPath(path.clone())),
                            cx,
                        );
                        host.handle(Action::Close, cx);
                    }
                });
            })
            .on_mouse_down(MouseButton::Right, move |_, _, cx| {
                h.update(cx, |host, cx| {
                    host.handle(Action::RevealFolder(reveal.clone()), cx)
                });
            })
            .into_any_element()
    }

    pub(super) fn dock_folder_menu_content(
        &self,
        host: &ViewState,
        path: &std::path::Path,
        p: Palette,
    ) -> AnyElement {
        let folder = host.dock_folders.iter().find(|folder| folder.path == path);
        let name = folder
            .map(|folder| folder.name.clone())
            .unwrap_or_else(|| "文件夹".into());
        let display = folder.map(|folder| folder.display).unwrap_or_default();
        let layout = folder.map(|folder| folder.layout).unwrap_or_default();
        let sort = folder.map(|folder| folder.sort).unwrap_or_default();
        let descending = folder.is_some_and(|folder| folder.descending);
        let path = path.to_path_buf();
        // A tighter column keeps every action visible while the group dividers
        // still separate the four option groups.
        let mut rows = div().flex().flex_col().gap(px(FOLDER_MENU_INNER_GAP));
        rows = rows
            .child(
                div()
                    .h(px(FOLDER_MENU_HEADER_ROW))
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(name),
            )
            .child(menu_separator("folder-menu-separator-0", p))
            .child(section_label("显示", p))
            .child(self.choice_list(
                vec![
                    (
                        "folder-display-stack",
                        "图标叠加",
                        Action::SetFolderDisplay(path.clone(), FolderDisplay::Stack),
                        display == FolderDisplay::Stack,
                    ),
                    (
                        "folder-display-folder",
                        "文件夹",
                        Action::SetFolderDisplay(path.clone(), FolderDisplay::Folder),
                        display == FolderDisplay::Folder,
                    ),
                ],
                p,
            ))
            .child(menu_separator("folder-menu-separator-1", p))
            .child(section_label("布局", p))
            .child(self.choice_list(
                vec![
                    (
                        "folder-layout-list",
                        "列表",
                        Action::SetFolderLayout(path.clone(), FolderLayout::List),
                        layout == FolderLayout::List,
                    ),
                    (
                        "folder-layout-grid",
                        "网格",
                        Action::SetFolderLayout(path.clone(), FolderLayout::Grid),
                        layout == FolderLayout::Grid,
                    ),
                    (
                        "folder-layout-fan",
                        "扇形",
                        Action::SetFolderLayout(path.clone(), FolderLayout::Fan),
                        layout == FolderLayout::Fan,
                    ),
                    (
                        "folder-layout-auto",
                        "自动",
                        Action::SetFolderLayout(path.clone(), FolderLayout::Auto),
                        layout == FolderLayout::Auto,
                    ),
                ],
                p,
            ))
            .child(menu_separator("folder-menu-separator-2", p))
            .child(section_label("排序", p))
            .child(self.choice_list(
                vec![
                    (
                        "folder-sort-name",
                        "名称",
                        Action::SetFolderSort(path.clone(), FolderSort::Name),
                        sort == FolderSort::Name,
                    ),
                    (
                        "folder-sort-modified",
                        "修改日期",
                        Action::SetFolderSort(path.clone(), FolderSort::Modified),
                        sort == FolderSort::Modified,
                    ),
                    (
                        "folder-sort-kind",
                        "类型",
                        Action::SetFolderSort(path.clone(), FolderSort::Kind),
                        sort == FolderSort::Kind,
                    ),
                    (
                        "folder-sort-size",
                        "大小",
                        Action::SetFolderSort(path.clone(), FolderSort::Size),
                        sort == FolderSort::Size,
                    ),
                ],
                p,
            ))
            .child(menu_separator("folder-menu-separator-3", p))
            .child(section_label("方向", p))
            .child(self.choice_list(
                vec![
                    (
                        "folder-ascending",
                        "升序 ↑",
                        Action::SetFolderDescending(path.clone(), false),
                        !descending,
                    ),
                    (
                        "folder-descending",
                        "降序 ↓",
                        Action::SetFolderDescending(path.clone(), true),
                        descending,
                    ),
                ],
                p,
            ))
            .child(menu_separator("folder-menu-separator-4", p))
            .child(self.folder_action_button(
                "folder-open",
                "打开文件夹",
                Action::OpenFolder(path.clone()),
                p,
            ))
            .child(self.folder_action_button(
                "folder-reveal-menu",
                "在文件管理器中显示",
                Action::RevealFolder(path.clone()),
                p,
            ))
            .child(self.folder_action_button(
                "folder-refresh",
                "刷新",
                Action::RefreshFolder(path.clone()),
                p,
            ))
            .child(self.folder_action_button(
                "folder-remove",
                "从 Dock 移除",
                Action::UnpinFolder(path),
                p,
            ));
        div()
            .id("folder-menu-scroll")
            .debug_selector(|| "folder-menu-scroll".into())
            .h_full()
            .overflow_y_scroll()
            .child(rows)
            .into_any_element()
    }

    fn choice_list(
        &self,
        choices: Vec<(&'static str, &'static str, Action, bool)>,
        p: Palette,
    ) -> AnyElement {
        let mut column = div().flex().flex_col().gap(px(FOLDER_MENU_CHOICE_GAP));
        for (id, label, action, selected) in choices {
            column = column.child(self.choice_button(id, label, action, selected, p));
        }
        column.into_any_element()
    }

    fn choice_button(
        &self,
        id: &'static str,
        label: &'static str,
        action: Action,
        selected: bool,
        p: Palette,
    ) -> AnyElement {
        self.folder_menu_row(
            id,
            label,
            action,
            selected.then_some("✓"),
            FOLDER_MENU_CHOICE_ROW,
            p,
        )
    }

    fn folder_action_button(
        &self,
        id: &'static str,
        label: &'static str,
        action: Action,
        p: Palette,
    ) -> AnyElement {
        self.folder_menu_row(id, label, action, None, FOLDER_MENU_ACTION_ROW, p)
    }

    fn folder_menu_row(
        &self,
        id: &'static str,
        label: &'static str,
        action: Action,
        check: Option<&'static str>,
        height: f32,
        p: Palette,
    ) -> AnyElement {
        let host = self.host.clone();
        // The row surface (like the popup background) may be translucent, but
        // label and check colours stay fully opaque so lowering menu opacity
        // never fades the text.
        div()
            .id(id)
            .debug_selector(move || id.to_owned())
            .h(px(height))
            .flex()
            .items_center()
            .gap(px(4.))
            .px(px(6.))
            .rounded(px(5.))
            .cursor_pointer()
            .hover(move |style| style.bg(p.line))
            .child(
                div()
                    .w(px(16.))
                    .text_size(px(12.))
                    .text_color(p.fg)
                    .child(check.unwrap_or("")),
            )
            .child(
                div()
                    .min_w_0()
                    .max_w_full()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(12.))
                    .text_color(p.fg)
                    .child(label),
            )
            .on_click(move |_, _, cx| {
                host.update(cx, |host, cx| host.handle(action.clone(), cx))
            })
            .into_any_element()
    }
}
