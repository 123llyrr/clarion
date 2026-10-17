//! Output-local workspace gesture policy. Stable space identities survive
//! topology edits; presentation animations remain in the scene adapter.

use std::collections::HashMap;
use std::time::Duration;

use clarion_motion::rubber_band::RubberBand;

use crate::gestures::swipe_tracker::SwipeTracker;
use crate::SpaceId;

// Retained Clarion parameters, not measured Apple trajectories.
const TOUCHPAD_MOVEMENT: f64 = 300.;
const DND_MOVEMENT: f64 = 1500.;
const RUBBER_BAND: RubberBand = RubberBand {
    stiffness: 0.5,
    limit: 0.05,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DndGeometry {
    pub position: [f64; 2],
    pub viewport_width: f64,
    pub working_top: f64,
    pub working_height: f64,
    pub zoom: f64,
    pub trigger_height: f64,
    pub speed: f64,
    pub max_speed: f64,
    pub delay: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WorkspaceSwitchAction {
    Begin {
        connector: String,
        center: SpaceId,
        current: f64,
        is_touchpad: bool,
        is_clamped: bool,
        dnd_timestamp: Option<Duration>,
    },
    Update {
        connector: String,
        delta_y: f64,
        timestamp: Duration,
        is_touchpad: bool,
        zoom: f64,
        viewport_height: f64,
    },
    DndScroll {
        connector: String,
        timestamp: Duration,
        geometry: DndGeometry,
    },
    DndIdle {
        connector: String,
        timestamp: Duration,
    },
    Activate {
        connector: String,
        target: SpaceId,
    },
    Cancel {
        connector: String,
        is_touchpad: Option<bool>,
        timestamp: Duration,
        zoom: f64,
        viewport_height: f64,
        extra_position: f64,
        extra_velocity: f64,
    },
    End {
        connector: String,
        is_touchpad: Option<bool>,
        timestamp: Duration,
        zoom: f64,
        viewport_height: f64,
        extra_position: f64,
        extra_velocity: f64,
    },
    Clear {
        connector: String,
    },
    OutputRemoved(String),
}
impl WorkspaceSwitchAction {
    pub fn connector(&self) -> &str {
        match self {
            Self::Begin { connector, .. }
            | Self::Update { connector, .. }
            | Self::DndScroll { connector, .. }
            | Self::DndIdle { connector, .. }
            | Self::Activate { connector, .. }
            | Self::Cancel { connector, .. }
            | Self::End { connector, .. }
            | Self::Clear { connector } => connector,
            Self::OutputRemoved(connector) => connector,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorkspaceSettlement {
    pub target: SpaceId,
    pub from: f64,
    pub to: f64,
    pub velocity: f64,
    pub remember_previous: bool,
}
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct WorkspaceSwitchOutcome {
    pub changed: bool,
    pub settlement: Option<WorkspaceSettlement>,
    /// Retargeting during DnD shifts the gesture immediately; the scene carries
    /// this offset in its existing animation, preserving its sampled velocity.
    pub animation_offset: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct WorkspaceGesture {
    center: SpaceId,
    center_index: usize,
    start: f64,
    current: f64,
    tracker: SwipeTracker,
    is_touchpad: bool,
    is_clamped: bool,
    dnd_last_event: Option<Duration>,
    dnd_nonzero_start: Option<Duration>,
}
impl WorkspaceGesture {
    pub fn center(&self) -> SpaceId {
        self.center
    }
    pub fn current(&self) -> f64 {
        self.current
    }
    pub fn is_touchpad(&self) -> bool {
        self.is_touchpad
    }
    pub fn is_dnd(&self) -> bool {
        self.dnd_last_event.is_some()
    }
    fn min_max(&self, count: usize) -> (f64, f64) {
        if self.is_clamped {
            (
                self.center_index.saturating_sub(1) as f64,
                (self.center_index + 1).min(count.saturating_sub(1)) as f64,
            )
        } else {
            (0., count.saturating_sub(1) as f64)
        }
    }
    fn total_height(&self, viewport_height: f64) -> f64 {
        if self.is_dnd() {
            DND_MOVEMENT
        } else if self.is_touchpad {
            TOUCHPAD_MOVEMENT
        } else {
            viewport_height
        }
    }
    /// Instantaneous velocity from the last owner event. Presentation reads
    /// never push a synthetic future sample or consume the gesture.
    pub fn velocity(&self, zoom: f64, viewport_height: f64, count: usize) -> f64 {
        if !valid_metrics(zoom, viewport_height) {
            return 0.;
        }
        let distance = self.total_height(viewport_height);
        let mut rubber_band = RUBBER_BAND;
        rubber_band.limit /= zoom;
        let (min, max) = self.min_max(count);
        let value = self.tracker.velocity() / distance
            * rubber_band.clamp_derivative(min, max, self.start + self.tracker.pos() / distance);
        if value.is_finite() {
            value
        } else {
            0.
        }
    }
    fn velocity_at(
        &mut self,
        timestamp: Duration,
        zoom: f64,
        viewport_height: f64,
        count: usize,
    ) -> f64 {
        self.tracker.push(0., timestamp);
        self.velocity(zoom, viewport_height, count)
    }
}
#[derive(Debug, Clone, Default)]
pub struct WorkspaceSwitchState {
    gestures: HashMap<String, WorkspaceGesture>,
}
impl WorkspaceSwitchState {
    pub fn gesture(&self, connector: &str) -> Option<&WorkspaceGesture> {
        self.gestures.get(connector)
    }
    /// Called only at navigation mutation edges. No workspace order is copied
    /// or mirrored here; the navigation owner supplies its actual ordered IDs.
    pub fn reconcile_order(&mut self, connector: &str, order: &[SpaceId]) -> bool {
        let Some(gesture) = self.gestures.get_mut(connector) else {
            return false;
        };
        if order.is_empty() {
            self.gestures.remove(connector);
            return true;
        }
        let index = order
            .iter()
            .position(|id| *id == gesture.center)
            .unwrap_or_else(|| gesture.center_index.min(order.len() - 1));
        let changed = gesture.center != order[index] || gesture.center_index != index;
        if changed {
            let shift = index as f64 - gesture.center_index as f64;
            gesture.start += shift;
            gesture.current += shift;
            gesture.center = order[index];
            gesture.center_index = index;
        }
        changed
    }
    pub fn apply(
        &mut self,
        action: WorkspaceSwitchAction,
        order: &[SpaceId],
    ) -> WorkspaceSwitchOutcome {
        let connector = action.connector().to_owned();
        let mut outcome = WorkspaceSwitchOutcome::default();
        let cancelled = matches!(&action, WorkspaceSwitchAction::Cancel { .. });
        match action {
            WorkspaceSwitchAction::Begin {
                center,
                current,
                is_touchpad,
                is_clamped,
                dnd_timestamp,
                ..
            } => {
                let Some(center_index) = order.iter().position(|id| *id == center) else {
                    return outcome;
                };
                if !current.is_finite() {
                    return outcome;
                }
                self.gestures.insert(
                    connector,
                    WorkspaceGesture {
                        center,
                        center_index,
                        start: current,
                        current,
                        tracker: SwipeTracker::new(),
                        is_touchpad,
                        is_clamped,
                        dnd_last_event: dnd_timestamp,
                        dnd_nonzero_start: None,
                    },
                );
                outcome.changed = true;
            }
            WorkspaceSwitchAction::Clear { .. } | WorkspaceSwitchAction::OutputRemoved(_) => {
                outcome.changed = self.gestures.remove(&connector).is_some();
            }
            WorkspaceSwitchAction::Update {
                delta_y,
                timestamp,
                is_touchpad,
                zoom,
                viewport_height,
                ..
            } => {
                let Some(gesture) = self.gestures.get_mut(&connector) else {
                    return outcome;
                };
                if gesture.is_touchpad != is_touchpad
                    || gesture.is_dnd()
                    || !valid_metrics(zoom, viewport_height)
                    || !delta_y.is_finite()
                {
                    return outcome;
                }
                let delta_scale = if is_touchpad {
                    (zoom - 1.) / 2.5 + 1.
                } else {
                    zoom
                };
                gesture.tracker.push(delta_y / delta_scale, timestamp);
                let distance = gesture.total_height(viewport_height);
                let mut rubber_band = RUBBER_BAND;
                rubber_band.limit /= zoom;
                let (min, max) = gesture.min_max(order.len());
                let current =
                    rubber_band.clamp(min, max, gesture.start + gesture.tracker.pos() / distance);
                outcome.changed = gesture.current != current;
                gesture.current = current;
            }
            WorkspaceSwitchAction::DndScroll {
                timestamp,
                geometry: g,
                ..
            } => {
                let Some(gesture) = self.gestures.get_mut(&connector) else {
                    return outcome;
                };
                let Some(last) = gesture.dnd_last_event else {
                    return outcome;
                };
                if ![
                    g.position[0],
                    g.position[1],
                    g.viewport_width,
                    g.working_top,
                    g.working_height,
                    g.zoom,
                    g.trigger_height,
                    g.speed,
                    g.max_speed,
                ]
                .iter()
                .all(|v| v.is_finite())
                    || g.working_height < 0.
                    || g.zoom <= 0.
                {
                    return outcome;
                }
                let width = g.viewport_width * g.zoom;
                let x = g.position[0] - (g.viewport_width - width) / 2.;
                let y = (g.position[1] - g.working_top).clamp(0., g.working_height);
                let trigger = g.trigger_height.clamp(0., g.working_height / 2.);
                let delta = if x < 0. || x >= width || trigger < 0.01 {
                    0.
                } else if y < trigger {
                    -(trigger - y) / trigger
                } else if g.working_height - y < trigger {
                    (trigger - (g.working_height - y)) / trigger
                } else {
                    0.
                };
                let delta = delta * g.speed;
                gesture.dnd_last_event = Some(timestamp);
                if delta == 0. {
                    gesture.dnd_nonzero_start = None;
                    return outcome;
                }
                outcome.changed = true; // Active edge also suppresses DnD hold during its debounce.
                let start = *gesture.dnd_nonzero_start.get_or_insert(timestamp);
                if timestamp.saturating_sub(start) < g.delay {
                    return outcome;
                }
                gesture.tracker.push(
                    delta * timestamp.saturating_sub(last).as_secs_f64() * g.max_speed,
                    timestamp,
                );
                let unclamped = gesture.start + gesture.tracker.pos() / DND_MOVEMENT;
                let (min, max) = gesture.min_max(order.len());
                let current = unclamped.clamp(min, max);
                gesture.start += current - unclamped;
                gesture.current = current;
            }
            WorkspaceSwitchAction::DndIdle { timestamp, .. } => {
                if let Some(gesture) = self.gestures.get_mut(&connector) {
                    if gesture.dnd_last_event.is_some_and(|last| last != timestamp) {
                        gesture.dnd_last_event = Some(timestamp);
                        gesture.dnd_nonzero_start = None;
                    }
                }
            }
            WorkspaceSwitchAction::Activate { target, .. } => {
                let Some(gesture) = self
                    .gestures
                    .get_mut(&connector)
                    .filter(|gesture| gesture.is_dnd())
                else {
                    return outcome;
                };
                let Some(index) = order.iter().position(|id| *id == target) else {
                    return outcome;
                };
                let current = gesture.current;
                gesture.start = index as f64 - (gesture.current - gesture.start);
                gesture.current = index as f64;
                gesture.center = target;
                gesture.center_index = index;
                outcome.changed = true;
                outcome.animation_offset = Some(current - gesture.current);
            }
            WorkspaceSwitchAction::Cancel {
                is_touchpad,
                timestamp,
                zoom,
                viewport_height,
                extra_position,
                extra_velocity,
                ..
            }
            | WorkspaceSwitchAction::End {
                is_touchpad,
                timestamp,
                zoom,
                viewport_height,
                extra_position,
                extra_velocity,
                ..
            } => {
                let Some(gesture) = self.gestures.get_mut(&connector) else {
                    return outcome;
                };
                if order.is_empty()
                    || is_touchpad.is_some_and(|value| value != gesture.is_touchpad)
                    || !valid_metrics(zoom, viewport_height)
                    || !extra_position.is_finite()
                    || !extra_velocity.is_finite()
                {
                    return outcome;
                }
                let velocity = gesture.velocity_at(timestamp, zoom, viewport_height, order.len())
                    + extra_velocity;
                let target = if cancelled {
                    gesture.center_index.min(order.len() - 1)
                } else {
                    let (min, max) = gesture.min_max(order.len());
                    (gesture.start
                        + gesture.tracker.projected_end_pos()
                            / gesture.total_height(viewport_height))
                    .clamp(min, max)
                    .round() as usize
                };
                let from = gesture.current + extra_position;
                if !from.is_finite() || !velocity.is_finite() {
                    return outcome;
                }
                outcome.changed = true;
                outcome.settlement = Some(WorkspaceSettlement {
                    target: order[target],
                    from,
                    to: target as f64,
                    velocity,
                    remember_previous: !cancelled,
                });
                self.gestures.remove(&connector);
            }
        }
        outcome
    }
}
fn valid_metrics(zoom: f64, height: f64) -> bool {
    zoom.is_finite() && zoom > 0. && height.is_finite() && height > 0.
}

#[cfg(test)]
mod tests {
    use super::*;
    fn begin(state: &mut WorkspaceSwitchState, order: &[SpaceId], dnd: bool) {
        state.apply(
            WorkspaceSwitchAction::Begin {
                connector: "DP-1".into(),
                center: order[1],
                current: 1.,
                is_touchpad: !dnd,
                is_clamped: !dnd,
                dnd_timestamp: dnd.then_some(Duration::ZERO),
            },
            order,
        );
    }
    fn update(state: &mut WorkspaceSwitchState, order: &[SpaceId], delta: f64, ms: u64) {
        state.apply(
            WorkspaceSwitchAction::Update {
                connector: "DP-1".into(),
                delta_y: delta,
                timestamp: Duration::from_millis(ms),
                is_touchpad: true,
                zoom: 1.,
                viewport_height: 1000.,
            },
            order,
        );
    }
    #[test]
    fn release_and_cancel_carry_tracker_and_extra_animation_velocity() {
        let order = [SpaceId(10), SpaceId(20), SpaceId(30), SpaceId(40)];
        for cancel in [false, true] {
            let mut state = WorkspaceSwitchState::default();
            begin(&mut state, &order, false);
            update(&mut state, &order, 20., 10);
            update(&mut state, &order, 35., 20);
            let before = state.gesture("DP-1").unwrap().current();
            let mut oracle = SwipeTracker::new();
            oracle.push(20., Duration::from_millis(10));
            oracle.push(35., Duration::from_millis(20));
            oracle.push(0., Duration::from_millis(25));
            let action = if cancel {
                WorkspaceSwitchAction::Cancel {
                    connector: "DP-1".into(),
                    is_touchpad: Some(true),
                    timestamp: Duration::from_millis(25),
                    zoom: 1.,
                    viewport_height: 1000.,
                    extra_position: 0.2,
                    extra_velocity: 0.7,
                }
            } else {
                WorkspaceSwitchAction::End {
                    connector: "DP-1".into(),
                    is_touchpad: Some(true),
                    timestamp: Duration::from_millis(25),
                    zoom: 1.,
                    viewport_height: 1000.,
                    extra_position: 0.2,
                    extra_velocity: 0.7,
                }
            };
            let result = state.apply(action, &order).settlement.unwrap();
            assert_eq!(result.from, before + 0.2);
            assert_eq!(result.velocity, oracle.velocity() / 300. + 0.7);
            assert_eq!(result.remember_previous, !cancel);
            if cancel {
                assert_eq!(result.target, SpaceId(20));
            }
            assert!(state.gesture("DP-1").is_none());
        }
    }
    #[test]
    fn topology_mutations_rebase_coordinates_around_stable_center() {
        let order = [SpaceId(10), SpaceId(20), SpaceId(30)];
        let mut state = WorkspaceSwitchState::default();
        begin(&mut state, &order, false);
        update(&mut state, &order, 90., 10);
        assert_eq!(state.gesture("DP-1").unwrap().current(), 1.3);
        state.reconcile_order("DP-1", &[SpaceId(5), SpaceId(10), SpaceId(20), SpaceId(30)]);
        assert_eq!(state.gesture("DP-1").unwrap().center(), SpaceId(20));
        assert_eq!(state.gesture("DP-1").unwrap().current(), 2.3);
        state.reconcile_order("DP-1", &[SpaceId(5), SpaceId(10), SpaceId(30)]);
        assert_eq!(state.gesture("DP-1").unwrap().center(), SpaceId(30));
        state.reconcile_order("DP-1", &[]);
        assert!(state.gesture("DP-1").is_none());
    }
    #[test]
    fn mismatched_source_and_nonfinite_input_do_not_end_or_poison_gesture() {
        let order = [SpaceId(10), SpaceId(20), SpaceId(30)];
        let mut state = WorkspaceSwitchState::default();
        begin(&mut state, &order, false);
        update(&mut state, &order, f64::INFINITY, 10);
        assert_eq!(state.gesture("DP-1").unwrap().current(), 1.);
        assert!(state
            .apply(
                WorkspaceSwitchAction::End {
                    connector: "DP-1".into(),
                    is_touchpad: Some(false),
                    timestamp: Duration::from_millis(20),
                    zoom: 1.,
                    viewport_height: 1000.,
                    extra_position: 0.,
                    extra_velocity: 0.
                },
                &order
            )
            .settlement
            .is_none());
        assert!(state.gesture("DP-1").is_some());
    }
    #[test]
    fn dnd_debounce_idle_and_retained_animation_retarget() {
        let order = [SpaceId(10), SpaceId(20), SpaceId(30)];
        let mut state = WorkspaceSwitchState::default();
        begin(&mut state, &order, true);
        let geometry = DndGeometry {
            position: [50., 1000.],
            viewport_width: 100.,
            working_top: 0.,
            working_height: 1000.,
            zoom: 1.,
            trigger_height: 100.,
            speed: 1.,
            max_speed: 1500.,
            delay: Duration::from_millis(100),
        };
        for ms in [10, 50] {
            assert!(
                state
                    .apply(
                        WorkspaceSwitchAction::DndScroll {
                            connector: "DP-1".into(),
                            timestamp: Duration::from_millis(ms),
                            geometry
                        },
                        &order
                    )
                    .changed
            );
        }
        assert_eq!(state.gesture("DP-1").unwrap().current(), 1.);
        state.apply(
            WorkspaceSwitchAction::DndScroll {
                connector: "DP-1".into(),
                timestamp: Duration::from_millis(110),
                geometry,
            },
            &order,
        );
        assert_eq!(state.gesture("DP-1").unwrap().current(), 1.06);
        let result = state.apply(
            WorkspaceSwitchAction::Activate {
                connector: "DP-1".into(),
                target: SpaceId(30),
            },
            &order,
        );
        assert_eq!(result.animation_offset, Some(-0.94));
        assert_eq!(state.gesture("DP-1").unwrap().current(), 2.);
        state.apply(
            WorkspaceSwitchAction::DndIdle {
                connector: "DP-1".into(),
                timestamp: Duration::from_millis(120),
            },
            &order,
        );
        state.apply(
            WorkspaceSwitchAction::DndScroll {
                connector: "DP-1".into(),
                timestamp: Duration::from_millis(130),
                geometry,
            },
            &order,
        );
        assert_eq!(state.gesture("DP-1").unwrap().current(), 2.);
    }
    #[test]
    fn dnd_clamping_does_not_accumulate_hidden_overscroll() {
        let order = [SpaceId(10), SpaceId(20), SpaceId(30)];
        let mut state = WorkspaceSwitchState::default();
        begin(&mut state, &order, true);
        let mut geometry = DndGeometry {
            position: [50., 1000.],
            viewport_width: 100.,
            working_top: 0.,
            working_height: 1000.,
            zoom: 1.,
            trigger_height: 100.,
            speed: 1.,
            max_speed: 1500.,
            delay: Duration::ZERO,
        };
        state.apply(
            WorkspaceSwitchAction::DndScroll {
                connector: "DP-1".into(),
                timestamp: Duration::from_secs(10),
                geometry,
            },
            &order,
        );
        assert_eq!(state.gesture("DP-1").unwrap().current(), 2.);
        geometry.position[1] = 0.;
        state.apply(
            WorkspaceSwitchAction::DndScroll {
                connector: "DP-1".into(),
                timestamp: Duration::from_millis(10010),
                geometry,
            },
            &order,
        );
        assert!((state.gesture("DP-1").unwrap().current() - 1.99).abs() < 1e-12);
    }
}
