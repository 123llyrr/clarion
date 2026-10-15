//! Authoritative scroll viewport intent and numeric gesture/trajectory state.
//! The platform provides timestamps, configuration conversion and column geometry;
//! projected rendering samples this owner without advancing it.
use crate::{gestures::swipe_tracker::SwipeTracker, trajectory::Trajectory, Rect};
use std::time::Duration;

const TOUCHPAD_WORKING_AREA_MOVEMENT: f64 = 1200.;
#[derive(Debug, Clone, Copy)]
pub struct ViewportTime {
    pub animation: Duration,
    pub event: Duration,
    pub rate: f64,
    pub complete: bool,
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MotionIntent {
    pub from: f64,
    pub to: f64,
    pub velocity: f64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GestureKind {
    Pointer,
    Touchpad,
    Dnd,
}
#[derive(Debug, Clone)]
struct Gesture {
    current: f64,
    animation: Option<Trajectory>,
    tracker: SwipeTracker,
    delta: f64,
    stationary: f64,
    kind: GestureKind,
    normalization: f64,
    dnd_last_event: Option<Duration>,
    dnd_nonzero_start: Option<Duration>,
}
#[derive(Debug, Clone)]
enum State {
    Static(f64),
    Animation(Trajectory),
    Gesture(Gesture),
}
#[derive(Debug, Clone)]
pub struct Viewport {
    state: State,
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Release {
    pub current: f64,
    pub target: f64,
    pub velocity: f64,
}

impl Viewport {
    pub fn new(offset: f64) -> Self {
        Self {
            state: State::Static(offset),
        }
    }
    pub fn current_at(&self, time: ViewportTime) -> f64 {
        match &self.state {
            State::Static(value) => *value,
            State::Animation(motion) => motion.value_at(time.animation, time.complete),
            State::Gesture(gesture) => gesture.current + extra(gesture, time).0,
        }
    }
    pub fn target(&self) -> f64 {
        match &self.state {
            State::Static(value) => *value,
            State::Animation(motion) => motion.to(),
            State::Gesture(gesture) => gesture.current,
        }
    }
    pub fn stationary(&self) -> f64 {
        match &self.state {
            State::Gesture(gesture) => gesture.stationary,
            _ => self.target(),
        }
    }
    pub fn is_static(&self) -> bool {
        matches!(self.state, State::Static(_))
    }
    pub fn is_gesture(&self) -> bool {
        matches!(self.state, State::Gesture(_))
    }
    pub fn is_dnd(&self) -> bool {
        self.gesture_kind() == Some(GestureKind::Dnd)
    }
    pub fn gesture_kind(&self) -> Option<GestureKind> {
        match &self.state {
            State::Gesture(gesture) => Some(gesture.kind),
            _ => None,
        }
    }
    pub fn is_animating(&self) -> bool {
        match &self.state {
            State::Static(_) => false,
            State::Animation(_) => true,
            State::Gesture(gesture) => gesture.animation.is_some(),
        }
    }
    pub fn set_static(&mut self, value: f64) {
        if value.is_finite() {
            self.state = State::Static(value);
        }
    }
    pub fn translate(&mut self, delta: f64) {
        if !delta.is_finite() {
            return;
        }
        match &mut self.state {
            State::Static(value) => *value += delta,
            State::Animation(motion) => motion.offset(delta),
            State::Gesture(gesture) => {
                gesture.current += delta;
                gesture.delta += delta;
                gesture.stationary += delta;
            }
        }
    }
    /// Event-time lifecycle advancement only. Rendering calls current_at instead.
    pub fn advance(&mut self, time: ViewportTime) {
        match &mut self.state {
            State::Animation(motion) if motion.is_done_at(time.animation, time.complete) => {
                self.state = State::Static(motion.to());
            }
            State::Gesture(gesture) => {
                if let Some(last) = &mut gesture.dnd_last_event {
                    if *last != time.event {
                        *last = time.event;
                        gesture.dnd_nonzero_start = None;
                    }
                }
                if gesture
                    .animation
                    .as_ref()
                    .is_some_and(|motion| motion.is_done_at(time.animation, time.complete))
                {
                    gesture.animation = None;
                }
            }
            _ => {}
        }
    }
    /// Configuration is permanently adapted to a pure trajectory synchronously.
    /// The accepted endpoint and its trajectory are never stored by the platform.
    pub fn settle(&mut self, intent: MotionIntent, make: impl FnOnce(MotionIntent) -> Trajectory) {
        if finite(intent) {
            self.state = State::Animation(make(intent));
        }
    }
    pub fn animate_to(
        &mut self,
        target: f64,
        pixel: f64,
        time: ViewportTime,
        make: impl FnOnce(MotionIntent) -> Trajectory,
    ) {
        if !target.is_finite() || !pixel.is_finite() || pixel <= 0. {
            return;
        }
        let difference = target - self.target();
        if difference.abs() < pixel {
            self.translate(difference);
            return;
        }
        if let State::Gesture(gesture) = &mut self.state {
            if gesture.kind == GestureKind::Dnd {
                gesture.stationary = target;
                let displacement = target - gesture.current;
                gesture.delta += displacement;
                gesture.current = target;
                let (position, velocity) = extra(gesture, time);
                gesture.animation = Some(make(MotionIntent {
                    from: -displacement + position,
                    to: 0.,
                    velocity,
                }));
                return;
            }
        }
        // Retargeting a live gesture must inherit its actual velocity as well as
        // its pose. Tracker idle decay is evaluated at the owner event time.
        let velocity = self.velocity_at(time, true);
        self.settle(
            MotionIntent {
                from: self.current_at(time),
                to: target,
                velocity,
            },
            make,
        );
    }
    pub fn begin(
        &mut self,
        kind: GestureKind,
        working_width: f64,
        time: ViewportTime,
        make: impl FnOnce(MotionIntent) -> Trajectory,
    ) -> bool {
        if kind == GestureKind::Dnd && self.is_dnd() {
            return false;
        }
        let current = self.current_at(time);
        let stationary = self.stationary();
        let velocity = self.velocity_at(time, true);
        let animation = (velocity != 0.).then(|| {
            make(MotionIntent {
                from: 0.,
                to: 0.,
                velocity,
            })
        });
        self.state = State::Gesture(Gesture {
            current,
            animation,
            tracker: SwipeTracker::new(),
            delta: current,
            stationary,
            kind,
            normalization: normalization(kind, working_width),
            dnd_last_event: (kind == GestureKind::Dnd).then_some(time.event),
            dnd_nonzero_start: None,
        });
        true
    }
    pub fn update(
        &mut self,
        delta: f64,
        timestamp: Duration,
        touchpad: bool,
        working_width: f64,
    ) -> Option<bool> {
        let State::Gesture(gesture) = &mut self.state else {
            return None;
        };
        if gesture.kind == GestureKind::Dnd
            || (gesture.kind == GestureKind::Touchpad) != touchpad
            || !delta.is_finite()
        {
            return None;
        }
        gesture.normalization = normalization(gesture.kind, working_width);
        gesture.tracker.push(delta, timestamp);
        gesture.current = gesture.tracker.pos() * gesture.normalization + gesture.delta;
        Some(true)
    }
    pub fn dnd_scroll(
        &mut self,
        delta: f64,
        now: Duration,
        delay: Duration,
        speed: f64,
        bounds: [f64; 2],
    ) -> bool {
        let State::Gesture(gesture) = &mut self.state else {
            return false;
        };
        let Some(last) = gesture.dnd_last_event else {
            return false;
        };
        if !delta.is_finite() || !speed.is_finite() || !bounds.iter().all(|x| x.is_finite()) {
            return false;
        }
        gesture.dnd_last_event = Some(now);
        if delta == 0. {
            gesture.dnd_nonzero_start = None;
            return false;
        }
        let start = *gesture.dnd_nonzero_start.get_or_insert(now);
        if now.saturating_sub(start) < delay {
            return true;
        }
        let movement = delta * now.saturating_sub(last).as_secs_f64() * speed;
        if !movement.is_finite() {
            return false;
        }
        gesture.tracker.push(movement, now);
        let position = gesture.tracker.pos() + gesture.delta;
        let clamped = position.clamp(bounds[0].min(bounds[1]), bounds[0].max(bounds[1]));
        gesture.delta += clamped - position;
        gesture.current = clamped;
        true
    }
    pub fn release(
        &mut self,
        touchpad: Option<bool>,
        working_width: f64,
        time: ViewportTime,
    ) -> Option<Release> {
        let State::Gesture(gesture) = &mut self.state else {
            return None;
        };
        if touchpad.is_some_and(|value| value != (gesture.kind == GestureKind::Touchpad)) {
            return None;
        }
        gesture.normalization = normalization(gesture.kind, working_width);
        gesture.tracker.push(0., time.event);
        let (position, velocity) = extra(gesture, time);
        Some(Release {
            current: gesture.current + position,
            target: gesture.tracker.projected_end_pos() * gesture.normalization + gesture.delta,
            velocity: gesture.tracker.velocity() * gesture.normalization + velocity,
        })
    }
    pub fn cancel(
        &mut self,
        touchpad: Option<bool>,
        working_width: f64,
        time: ViewportTime,
        make: impl FnOnce(MotionIntent) -> Trajectory,
    ) -> bool {
        let Some(release) = self.release(touchpad, working_width, time) else {
            return false;
        };
        let State::Gesture(gesture) = &self.state else {
            unreachable!()
        };
        let target = gesture.delta;
        self.settle(
            MotionIntent {
                from: release.current,
                to: target,
                velocity: release.velocity,
            },
            make,
        );
        true
    }
    /// A DnD grab that never scrolled preserves the active relative trajectory.
    pub fn finish_unscrolled_dnd(&mut self) -> bool {
        let State::Gesture(gesture) = &mut self.state else {
            return false;
        };
        if gesture.kind != GestureKind::Dnd || gesture.tracker.pos() != 0. {
            return false;
        }
        self.state = if let Some(mut motion) = gesture.animation.take() {
            motion.offset(gesture.current);
            State::Animation(motion)
        } else {
            State::Static(gesture.delta)
        };
        true
    }
    /// Explicit resize ownership freezes the viewport, rather than retargeting it.
    pub fn freeze(&mut self, time: ViewportTime) {
        self.state = State::Static(self.current_at(time));
    }
    pub fn freeze_gesture(&mut self, time: ViewportTime) {
        if self.is_gesture() {
            self.freeze(time);
        }
    }
    fn velocity_at(&mut self, time: ViewportTime, decay: bool) -> f64 {
        match &mut self.state {
            State::Static(_) => 0.,
            State::Animation(motion) => {
                motion.velocity_at(time.animation, time.complete) * time.rate
            }
            State::Gesture(gesture) => {
                if decay {
                    gesture.tracker.push(0., time.event);
                }
                gesture.tracker.velocity() * gesture.normalization + extra(gesture, time).1
            }
        }
    }
}
fn normalization(kind: GestureKind, width: f64) -> f64 {
    if kind == GestureKind::Touchpad && width.is_finite() {
        width / TOUCHPAD_WORKING_AREA_MOVEMENT
    } else {
        1.
    }
}
fn extra(gesture: &Gesture, time: ViewportTime) -> (f64, f64) {
    gesture.animation.as_ref().map_or((0., 0.), |motion| {
        (
            motion.value_at(time.animation, time.complete),
            motion.velocity_at(time.animation, time.complete) * time.rate,
        )
    })
}
fn finite(intent: MotionIntent) -> bool {
    [intent.from, intent.to, intent.velocity]
        .iter()
        .all(|x| x.is_finite())
}

/// Numeric bounds from live column widths, with no per-frame geometry allocation.
pub fn dnd_bounds(
    widths: impl Iterator<Item = f64>,
    active: usize,
    gaps: f64,
    area: Rect,
) -> [f64; 2] {
    let mut x = 0.;
    let mut active_x = 0.;
    let mut right = None;
    for (index, width) in widths.enumerate() {
        if index == active {
            active_x = x;
        }
        right = Some(x + width);
        x += width + gaps;
    }
    right.map_or([0., 0.], |right| {
        [-area.width - active_x, right - area.x - active_x]
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnSizing {
    Normal,
    Fullscreen,
    Maximized,
}
impl ColumnSizing {
    fn is_fullscreen(self) -> bool {
        self == Self::Fullscreen
    }
    fn is_maximized(self) -> bool {
        self == Self::Maximized
    }
}
#[derive(Debug, Clone, Copy)]
pub struct ColumnGeometry {
    pub width: f64,
    pub sizing: ColumnSizing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusCentering {
    Always,
    OnOverflow,
    Never,
}

#[derive(Debug, Clone, Copy)]
pub struct FocusContext {
    pub working: Rect,
    pub parent: Rect,
    pub gaps: f64,
    pub centering: FocusCentering,
    pub always_center_single: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct PositionedColumn {
    pub x: f64,
    pub width: f64,
    pub sizing: ColumnSizing,
}
pub fn focus_columns_centered(context: FocusContext, count: usize) -> bool {
    context.centering == FocusCentering::Always || (context.always_center_single && count <= 1)
}

impl FocusContext {
    fn valid(self) -> bool {
        [
            self.working.x,
            self.working.width,
            self.parent.x,
            self.parent.width,
            self.gaps,
        ]
        .iter()
        .all(|value| value.is_finite())
            && self.working.width >= 0.
            && self.parent.width >= 0.
            && self.gaps >= 0.
    }
}
impl PositionedColumn {
    fn valid(self) -> bool {
        self.x.is_finite() && self.width.is_finite() && self.width >= 0.
    }
}

/// Keep a visible column where it is, otherwise choose the nearer padded edge.
/// Returned offsets are relative to that column's own position.
pub fn fit_column_offset(
    current: f64,
    column: PositionedColumn,
    context: FocusContext,
) -> Option<f64> {
    if !current.is_finite() || !column.valid() || !context.valid() {
        return None;
    }
    if column.sizing.is_fullscreen() {
        return Some(0.);
    }
    let (area, gaps) = if column.sizing.is_maximized() {
        (context.parent, 0.)
    } else {
        (context.working, context.gaps)
    };
    let cur_x = current + area.x;
    let offset = if area.width <= column.width {
        0.
    } else {
        let padding = ((area.width - column.width) / 2.).clamp(0., gaps);
        let left = column.x - padding;
        let right = column.x + column.width + padding;
        if cur_x <= left && right <= cur_x + area.width {
            -(column.x - cur_x)
        } else if (cur_x - left).abs() <= ((cur_x + area.width) - right).abs() {
            -padding
        } else {
            -(area.width - padding - column.width)
        }
    } - area.x;
    offset.is_finite().then_some(offset)
}

/// Wide and fullscreen columns retain the fitting policy; expanded columns
/// use their parent area rather than the normal work area's exclusive zones.
pub fn center_column_offset(
    current: f64,
    column: PositionedColumn,
    context: FocusContext,
) -> Option<f64> {
    if !current.is_finite() || !column.valid() || !context.valid() {
        return None;
    }
    let area = if column.sizing.is_maximized() {
        context.parent
    } else {
        context.working
    };
    if column.sizing.is_fullscreen() || area.width <= column.width {
        fit_column_offset(current, column, context)
    } else {
        let offset = -(area.width - column.width) / 2. - area.x;
        offset.is_finite().then_some(offset)
    }
}

/// Ordinary focus movement consumes two numeric column observations at most.
/// OnOverflow deliberately checks the adjacent source, including a jump from
/// a distant or subsequently removed column, matching the retained policy.
pub fn focus_column_offset(
    current: f64,
    target: usize,
    previous: Option<usize>,
    count: usize,
    context: FocusContext,
    geometry: impl Fn(usize) -> PositionedColumn,
) -> Option<f64> {
    if target >= count || !context.valid() || !current.is_finite() {
        return None;
    }
    let column = geometry(target);
    if focus_columns_centered(context, count) {
        return center_column_offset(current, column, context);
    }
    if context.centering == FocusCentering::OnOverflow {
        if let Some(previous) = previous.filter(|previous| *previous != target) {
            let source = if previous > target {
                (target + 1).min(count - 1)
            } else {
                target.saturating_sub(1)
            };
            let source = geometry(source);
            if !source.valid() || !column.valid() {
                return None;
            }
            // Retain the established work-area/gap comparison for small fixed
            // maximized columns; changing that behavior requires its own spec.
            let total = if source.x < column.x {
                column.x - source.x + column.width
            } else {
                source.x - column.x + source.width
            } + context.gaps * 2.;
            if total > context.working.width {
                return center_column_offset(current, column, context);
            }
        }
    }
    fit_column_offset(current, column, context)
}
impl ColumnGeometry {
    fn width(&self) -> f64 {
        self.width
    }
    fn sizing_mode(&self) -> ColumnSizing {
        self.sizing
    }
}
#[derive(Debug, Clone, Copy)]
pub struct SnapContext {
    pub working: Rect,
    pub parent: Rect,
    pub view_width: f64,
    pub gaps: f64,
    pub centered: bool,
    pub center_on_overflow: bool,
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SnapDecision {
    pub active: usize,
    pub motion: MotionIntent,
}
fn column_x(columns: &[ColumnGeometry], index: usize, gaps: f64) -> f64 {
    columns
        .iter()
        .take(index)
        .fold(0., |x, column| x + column.width + gaps)
}
/// Release snapping consumes event-time immutable numeric columns only. There
/// are no Tile/configure/renderer calls and no render-frame snapshot allocation.
pub fn snap_gesture(
    columns: &[ColumnGeometry],
    active: usize,
    context: SnapContext,
    release: Release,
) -> Option<SnapDecision> {
    if columns.is_empty()
        || active >= columns.len()
        || context.gaps < 0.
        || ![
            context.working.x,
            context.working.width,
            context.parent.x,
            context.parent.width,
            context.view_width,
            context.gaps,
            release.current,
            release.target,
            release.velocity,
        ]
        .iter()
        .all(|x| x.is_finite())
        || columns
            .iter()
            .any(|column| !column.width.is_finite() || column.width < 0.)
    {
        return None;
    }
    // Compute the snapping points. These are where the view aligns with column boundaries on
    // either side.
    struct Snap {
        // View position relative to x = 0 (the first column).
        view_pos: f64,
        // Column to activate for this snapping point.
        col_idx: usize,
    }

    let mut snapping_points = Vec::new();

    if context.centered {
        let mut col_x = 0.;
        for (col_idx, col) in columns.iter().enumerate() {
            let col_w = col.width();
            let mode = col.sizing_mode();

            let area = if mode.is_maximized() {
                context.parent
            } else {
                context.working
            };

            let left_strut = area.x;

            let view_pos = if mode.is_fullscreen() {
                col_x
            } else if area.width <= col_w {
                col_x - left_strut
            } else {
                col_x - (area.width - col_w) / 2. - left_strut
            };
            snapping_points.push(Snap { view_pos, col_idx });

            col_x += col_w + context.gaps;
        }
    } else {
        let center_on_overflow = context.center_on_overflow;

        let view_width = context.view_width;
        let gaps = context.gaps;

        let snap_points =
            |col_x, col: &ColumnGeometry, prev_col_w: Option<f64>, next_col_w: Option<f64>| {
                let col_w = col.width();
                let mode = col.sizing_mode();

                let area = if mode.is_maximized() {
                    context.parent
                } else {
                    context.working
                };

                let left_strut = area.x;
                let right_strut = context.view_width - area.width - area.x;

                // Normal columns align with the working area, but fullscreen columns align with
                // the view size.
                if mode.is_fullscreen() {
                    let left = col_x;
                    let right = left + col_w;
                    (left, right)
                } else {
                    // Logic from compute_new_view_offset.
                    let padding = if mode.is_maximized() {
                        0.
                    } else {
                        ((area.width - col_w) / 2.).clamp(0., gaps)
                    };

                    let center = if area.width <= col_w {
                        col_x - left_strut
                    } else {
                        col_x - (area.width - col_w) / 2. - left_strut
                    };
                    let is_overflowing = |adj_col_w: Option<f64>| {
                        center_on_overflow
                            && adj_col_w
                                .filter(|adj_col_w| {
                                    // NOTE: This logic won't work entirely correctly with small
                                    // fixed-size maximized windows (they have a different area
                                    // and padding).
                                    center_on_overflow
                                        && adj_col_w + 3.0 * gaps + col_w > area.width
                                })
                                .is_some()
                    };

                    let left = if is_overflowing(next_col_w) {
                        center
                    } else {
                        col_x - padding - left_strut
                    };
                    let right = if is_overflowing(prev_col_w) {
                        center + view_width
                    } else {
                        col_x + col_w + padding + right_strut
                    };
                    (left, right)
                }
            };

        // Prevent the gesture from snapping further than the first/last column, as this is
        // generally undesired.
        //
        // It's ok if leftmost_snap is > rightmost_snap (this happens if the columns on a
        // workspace total up to less than the workspace width).

        // The first column's left snap isn't actually guaranteed to be the *leftmost* snap.
        // With weird enough left strut and perhaps a maximized small fixed-size window, you
        // can make the second window's left snap be further to the left than the first
        // window's. The same goes for the rightmost snap.
        //
        // This isn't actually a big problem because it's very much an obscure edge case. Just
        // need to make sure the code doesn't panic when that happens.
        let leftmost_snap = snap_points(0., &columns[0], None, columns.get(1).map(|c| c.width())).0;
        let last_col_idx = columns.len() - 1;
        let last_col_x = columns
            .iter()
            .take(last_col_idx)
            .fold(0., |col_x, col| col_x + col.width() + gaps);
        let rightmost_snap = snap_points(
            last_col_x,
            &columns[last_col_idx],
            last_col_idx
                .checked_sub(1)
                .and_then(|idx| columns.get(idx).map(|c| c.width())),
            None,
        )
        .1 - view_width;

        snapping_points.push(Snap {
            view_pos: leftmost_snap,
            col_idx: 0,
        });
        snapping_points.push(Snap {
            view_pos: rightmost_snap,
            col_idx: last_col_idx,
        });

        let mut push = |col_idx, left, right| {
            if leftmost_snap < left && left < rightmost_snap {
                snapping_points.push(Snap {
                    view_pos: left,
                    col_idx,
                });
            }

            let right = right - view_width;
            if leftmost_snap < right && right < rightmost_snap {
                snapping_points.push(Snap {
                    view_pos: right,
                    col_idx,
                });
            }
        };

        let mut col_x = 0.;
        for (col_idx, col) in columns.iter().enumerate() {
            let (left, right) = snap_points(
                col_x,
                col,
                col_idx
                    .checked_sub(1)
                    .and_then(|idx| columns.get(idx).map(|c| c.width())),
                columns.get(col_idx + 1).map(|c| c.width()),
            );
            push(col_idx, left, right);

            col_x += col.width() + gaps;
        }
    }

    // Find the closest snapping point.
    if snapping_points
        .iter()
        .any(|snap| !snap.view_pos.is_finite())
    {
        return None;
    }
    snapping_points.sort_by(|left, right| left.view_pos.partial_cmp(&right.view_pos).unwrap());

    let active_col_x = column_x(columns, active, context.gaps);
    let target_view_pos = active_col_x + release.target;
    let target_snap = snapping_points
        .iter()
        .min_by(|left, right| {
            (left.view_pos - target_view_pos)
                .abs()
                .total_cmp(&(right.view_pos - target_view_pos).abs())
        })
        .unwrap();

    let mut new_col_idx = target_snap.col_idx;

    if !context.centered {
        // Focus the furthest window towards the direction of the gesture.
        if release.target >= release.current {
            for col_idx in (new_col_idx + 1)..columns.len() {
                let col = &columns[col_idx];
                let col_x = column_x(columns, col_idx, context.gaps);
                let col_w = col.width();
                let mode = col.sizing_mode();

                let area = if mode.is_maximized() {
                    context.parent
                } else {
                    context.working
                };

                let left_strut = area.x;

                if mode.is_fullscreen() {
                    if target_snap.view_pos + context.view_width < col_x + col_w {
                        break;
                    }
                } else {
                    let padding = if mode.is_maximized() {
                        0.
                    } else {
                        ((area.width - col_w) / 2.).clamp(0., context.gaps)
                    };

                    if target_snap.view_pos + left_strut + area.width < col_x + col_w + padding {
                        break;
                    }
                }

                new_col_idx = col_idx;
            }
        } else {
            for col_idx in (0..new_col_idx).rev() {
                let col = &columns[col_idx];
                let col_x = column_x(columns, col_idx, context.gaps);
                let col_w = col.width();
                let mode = col.sizing_mode();

                let area = if mode.is_maximized() {
                    context.parent
                } else {
                    context.working
                };

                let left_strut = area.x;

                if mode.is_fullscreen() {
                    if col_x < target_snap.view_pos {
                        break;
                    }
                } else {
                    let padding = if mode.is_maximized() {
                        0.
                    } else {
                        ((area.width - col_w) / 2.).clamp(0., context.gaps)
                    };

                    if col_x - padding < target_snap.view_pos + left_strut {
                        break;
                    }
                }

                new_col_idx = col_idx;
            }
        }
    }

    let new_col_x = column_x(columns, new_col_idx, context.gaps);
    Some(SnapDecision {
        active: new_col_idx,
        motion: MotionIntent {
            from: release.current + active_col_x - new_col_x,
            to: target_snap.view_pos - new_col_x,
            velocity: release.velocity,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trajectory::Curve;
    fn time(ms: u64, rate: f64) -> ViewportTime {
        ViewportTime {
            animation: Duration::from_secs_f64(ms as f64 / 1000. * rate),
            event: Duration::from_millis(ms),
            rate,
            complete: false,
        }
    }
    fn motion(time: ViewportTime, intent: MotionIntent) -> Trajectory {
        if intent.velocity == 0. {
            Trajectory::ease_at(
                time.animation,
                intent.from,
                intent.to,
                0.,
                Duration::from_millis(400),
                Curve::Linear,
            )
        } else {
            Trajectory::continuous_ease_at(
                time.animation,
                intent.from,
                intent.to,
                intent.velocity / time.rate,
                Duration::from_millis(400),
            )
        }
    }
    fn norm(kind: GestureKind) -> f64 {
        if kind == GestureKind::Touchpad {
            0.5
        } else {
            1.
        }
    }
    #[test]
    fn static_and_animation_targets_have_one_owner_and_forecasts_are_read_only() {
        let mut viewport = Viewport::new(20.);
        viewport.translate(5.);
        assert_eq!(viewport.target(), 25.);
        let start = time(0, 1.);
        viewport.animate_to(125., 1., start, |intent| motion(start, intent));
        assert_eq!(viewport.target(), 125.);
        assert_eq!(viewport.current_at(time(100, 1.)), 50.);
        let forecast = viewport.current_at(time(1000, 1.));
        assert_eq!(forecast, 125.);
        assert_eq!(viewport.current_at(time(100, 1.)), 50.);
        assert!(viewport.is_animating());
        viewport.advance(time(400, 1.));
        assert!(viewport.is_static());
        assert_eq!(viewport.target(), 125.);
    }
    #[test]
    fn touchpad_normalization_and_release_projection_match_independent_input_history() {
        for kind in [GestureKind::Pointer, GestureKind::Touchpad] {
            let mut viewport = Viewport::new(20.);
            let start = time(0, 1.);
            viewport.begin(kind, 600., start, |intent| motion(start, intent));
            let mut tracker = SwipeTracker::new();
            for (delta, ms) in [(10., 0), (20., 20), (0., 30)] {
                tracker.push(delta, Duration::from_millis(ms));
                if ms != 30 {
                    assert_eq!(
                        viewport.update(
                            delta,
                            Duration::from_millis(ms),
                            kind == GestureKind::Touchpad,
                            600.
                        ),
                        Some(true)
                    );
                }
            }
            let release = viewport
                .release(Some(kind == GestureKind::Touchpad), 600., time(30, 1.))
                .unwrap();
            assert_eq!(release.current, 20. + 30. * norm(kind));
            assert_eq!(release.velocity, tracker.velocity() * norm(kind));
            assert_eq!(
                release.target,
                20. + tracker.projected_end_pos() * norm(kind)
            );
        }
    }
    #[test]
    fn cancellation_preserves_incoming_pose_and_real_velocity_at_each_clock_rate() {
        for rate in [0.5, 1., 2.] {
            for kind in [GestureKind::Pointer, GestureKind::Touchpad] {
                let mut viewport = Viewport::new(20.);
                let start = time(0, rate);
                viewport.begin(kind, 600., start, |intent| motion(start, intent));
                viewport.update(10., Duration::ZERO, kind == GestureKind::Touchpad, 600.);
                viewport.update(
                    20.,
                    Duration::from_millis(20),
                    kind == GestureKind::Touchpad,
                    600.,
                );
                let current = viewport.current_at(time(30, rate));
                let at = time(30, rate);
                assert!(viewport.cancel(None, 600., at, |intent| motion(at, intent)));
                assert_eq!(viewport.current_at(at), current);
                let mut later = at;
                later.animation += Duration::from_secs_f64(0.000001 * rate);
                let velocity = (viewport.current_at(later) - current) / 0.000001;
                let expected = 20. / 0.03 * norm(kind);
                assert!(
                    (velocity - expected).abs() < 0.1,
                    "rate={rate}, {velocity} != {expected}"
                );
                assert_eq!(viewport.target(), 20.);
            }
        }
    }
    #[test]
    fn taking_over_a_running_animation_carries_its_sampled_momentum() {
        for rate in [0.5, 1., 2.] {
            let mut viewport = Viewport::new(0.);
            let start = time(0, rate);
            viewport.settle(
                MotionIntent {
                    from: 0.,
                    to: 10000.,
                    velocity: 0.,
                },
                |intent| motion(start, intent),
            );
            let at = time(50, rate);
            let before = viewport.current_at(at);
            let velocity = 25000. * rate;
            viewport.begin(GestureKind::Touchpad, 600., at, |intent| motion(at, intent));
            assert_eq!(viewport.current_at(at), before);
            let mut later = at;
            later.animation += Duration::from_secs_f64(0.000001 * rate);
            let after_velocity = (viewport.current_at(later) - before) / 0.000001;
            assert!((after_velocity - velocity).abs() < 1., "rate={rate}");
        }
    }
    #[test]
    fn dnd_debounce_uses_only_owner_time_and_resets_when_not_scrolled() {
        let mut viewport = Viewport::new(100.);
        let start = time(0, 1.);
        viewport.begin(GestureKind::Dnd, 600., start, |intent| {
            motion(start, intent)
        });
        for ms in [0, 50] {
            assert!(viewport.dnd_scroll(
                1.,
                Duration::from_millis(ms),
                Duration::from_millis(100),
                500.,
                [-1000., 1000.]
            ));
        }
        assert_eq!(viewport.target(), 100.);
        viewport.dnd_scroll(
            1.,
            Duration::from_millis(100),
            Duration::from_millis(100),
            500.,
            [-1000., 1000.],
        );
        assert_eq!(viewport.target(), 125.);
        assert_eq!(viewport.current_at(time(10000, 1.)), 125.);
        viewport.advance(time(150, 1.));
        viewport.dnd_scroll(
            1.,
            Duration::from_millis(160),
            Duration::from_millis(100),
            500.,
            [-1000., 1000.],
        );
        assert_eq!(viewport.target(), 125.);
        viewport.dnd_scroll(
            10.,
            Duration::from_millis(260),
            Duration::ZERO,
            500.,
            [-100., 150.],
        );
        assert_eq!(viewport.target(), 150.);
        viewport.translate(-40.);
        assert_eq!(viewport.target(), 110.);
    }
    fn context(centered: bool) -> SnapContext {
        SnapContext {
            working: Rect::new(10., 0., 980., 800.),
            parent: Rect::new(0., 0., 1000., 800.),
            view_width: 1000.,
            gaps: 10.,
            centered,
            center_on_overflow: false,
        }
    }
    #[test]
    fn release_snaps_centered_and_fitted_columns_to_fixed_independent_coordinates() {
        let columns = [ColumnGeometry {
            width: 400.,
            sizing: ColumnSizing::Normal,
        }; 3];
        assert_eq!(
            snap_gesture(
                &columns,
                0,
                context(true),
                Release {
                    current: 80.,
                    target: 90.,
                    velocity: 3.
                }
            ),
            Some(SnapDecision {
                active: 1,
                motion: MotionIntent {
                    from: -330.,
                    to: -300.,
                    velocity: 3.
                }
            })
        );
        assert_eq!(
            snap_gesture(
                &columns,
                0,
                context(false),
                Release {
                    current: 200.,
                    target: 210.,
                    velocity: 4.
                }
            ),
            Some(SnapDecision {
                active: 2,
                motion: MotionIntent {
                    from: -620.,
                    to: -580.,
                    velocity: 4.
                }
            })
        );
        let mixed = [
            columns[0],
            ColumnGeometry {
                width: 1000.,
                sizing: ColumnSizing::Fullscreen,
            },
            ColumnGeometry {
                width: 500.,
                sizing: ColumnSizing::Maximized,
            },
        ];
        assert_eq!(
            snap_gesture(
                &mixed,
                0,
                context(true),
                Release {
                    current: 400.,
                    target: 415.,
                    velocity: 5.
                }
            ),
            Some(SnapDecision {
                active: 1,
                motion: MotionIntent {
                    from: -10.,
                    to: 0.,
                    velocity: 5.
                }
            })
        );
    }
    #[test]
    fn live_bounds_need_no_topology_snapshot_and_empty_or_invalid_release_is_rejected() {
        assert_eq!(
            dnd_bounds(
                [400., 500., 600.].into_iter(),
                1,
                10.,
                Rect::new(10., 0., 980., 800.)
            ),
            [-1390., 1100.]
        );
        assert_eq!(
            dnd_bounds(std::iter::empty(), 0, 10., Rect::default()),
            [0., 0.]
        );
        let release = Release {
            current: 0.,
            target: 1.,
            velocity: 0.,
        };
        assert!(snap_gesture(&[], 0, context(false), release).is_none());
        assert!(snap_gesture(
            &[ColumnGeometry {
                width: f64::NAN,
                sizing: ColumnSizing::Normal
            }],
            0,
            context(false),
            release
        )
        .is_none());
    }
}
