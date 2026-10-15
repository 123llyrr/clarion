//! Overview interaction policy. The platform supplies event time and sampled
//! presentation progress; no renderer, clock, surface, or output handle is owned here.

use std::collections::HashMap;
use std::time::Duration;

use crate::SpaceId;

use crate::gestures::swipe_tracker::SwipeTracker;
use crate::rubber_band::RubberBand;

// Retained Realand gesture parameters, not verified Apple measurements.
const MOVEMENT: f64 = 300.;
const RUBBER_BAND: RubberBand = RubberBand {
    stiffness: 0.5,
    limit: 0.05,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OverviewMode {
    #[default]
    Stage,
    Strip,
    Expo,
    Windows,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OverviewAction {
    SetMode {
        mode: OverviewMode,
        timestamp: Duration,
    },
    SetOpen(bool),
    GestureBegin {
        value: f64,
    },
    GestureUpdate {
        delta_y: f64,
        timestamp: Duration,
    },
    GestureEnd {
        timestamp: Duration,
    },
    GestureCancel {
        timestamp: Duration,
    },
    PreviewOutput(Option<String>),
    OutputRemoved(String),
    PresentationSettled,
    StageReset {
        connector: String,
        space: SpaceId,
    },
    StageSelect {
        connector: String,
        space: SpaceId,
    },
    StageCloseAnchor {
        connector: String,
        space: Option<SpaceId>,
    },
    RememberExpo(Option<SpaceId>),
    RememberStage(Option<SpaceId>),
    SpaceRemoved(SpaceId),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverviewSettlement {
    pub from: f64,
    pub to: f64,
    pub velocity: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct OverviewOutcome {
    pub changed: bool,
    pub settlement: Option<OverviewSettlement>,
}

#[derive(Debug, Clone)]
struct Gesture {
    tracker: SwipeTracker,
    start: f64,
    value: f64,
    initial_open: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lifecycle {
    Hidden,
    Transitioning,
    Visible,
    Gesture,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StageSelection {
    pub selected: Option<SpaceId>,
    pub initial: Option<SpaceId>,
    pub close_anchor: Option<SpaceId>,
}

#[derive(Debug, Clone)]
pub struct OverviewState {
    mode: OverviewMode,
    desired_open: bool,
    lifecycle: Lifecycle,
    gesture: Option<Gesture>,
    preview_connector: Option<String>,
    stage: HashMap<String, StageSelection>,
    initial_expo: Option<SpaceId>,
    initial_stage: Option<SpaceId>,
}

impl Default for OverviewState {
    fn default() -> Self {
        Self {
            // Existing Layout constructors start in Strip even though the enum
            // default is Stage. Opening still chooses the configured mode.
            mode: OverviewMode::Strip,
            desired_open: false,
            lifecycle: Lifecycle::Hidden,
            gesture: None,
            preview_connector: None,
            stage: HashMap::new(),
            initial_expo: None,
            initial_stage: None,
        }
    }
}

impl OverviewState {
    pub fn mode(&self) -> OverviewMode {
        self.mode
    }
    pub fn desired_open(&self) -> bool {
        self.desired_open
    }
    pub fn visible(&self) -> bool {
        self.lifecycle != Lifecycle::Hidden
    }
    pub fn gesture_value(&self) -> Option<f64> {
        self.gesture.as_ref().map(|g| g.value)
    }
    pub fn preview_connector(&self) -> Option<&str> {
        self.preview_connector.as_deref()
    }
    pub fn stage(&self, connector: &str) -> Option<&StageSelection> {
        self.stage.get(connector)
    }
    pub fn initial_expo(&self) -> Option<SpaceId> {
        self.initial_expo
    }
    pub fn initial_stage(&self) -> Option<SpaceId> {
        self.initial_stage
    }

    /// Prune per-output references at a navigation mutation edge. Global cancel
    /// origins survive a transfer while the identity remains alive.
    pub fn retain_stage_spaces(&mut self, mut contains: impl FnMut(&str, SpaceId) -> bool) -> bool {
        let mut changed = false;
        for (connector, state) in &mut self.stage {
            for slot in [
                &mut state.selected,
                &mut state.initial,
                &mut state.close_anchor,
            ] {
                if slot.is_some_and(|id| !contains(connector, id)) {
                    *slot = None;
                    changed = true;
                }
            }
        }
        changed
    }

    pub fn gesture_velocity(&self) -> f64 {
        self.gesture.as_ref().map_or(0., |g| {
            g.tracker.velocity() / MOVEMENT
                * RUBBER_BAND.clamp_derivative(0., 1., g.start + g.tracker.pos() / MOVEMENT)
        })
    }

    pub fn apply(&mut self, action: OverviewAction) -> OverviewOutcome {
        let mut outcome = OverviewOutcome::default();
        match action {
            OverviewAction::SetMode { mode, timestamp } => {
                if self.mode == mode {
                    return outcome;
                }
                self.mode = mode;
                if mode != OverviewMode::Stage {
                    for state in self.stage.values_mut() {
                        state.close_anchor = None;
                    }
                }
                outcome.changed = true;
                if let Some(mut g) = self.gesture.take() {
                    g.tracker.push(0., timestamp);
                    let velocity = g.tracker.velocity() / MOVEMENT
                        * RUBBER_BAND.clamp_derivative(
                            0.,
                            1.,
                            g.start + g.tracker.pos() / MOVEMENT,
                        );
                    self.desired_open = true;
                    self.lifecycle = Lifecycle::Transitioning;
                    outcome.settlement = Some(OverviewSettlement {
                        from: g.value,
                        to: 1.,
                        velocity,
                    });
                }
            }
            OverviewAction::SetOpen(open) => {
                outcome.changed = self.desired_open != open || self.gesture.is_some();
                self.gesture = None;
                self.desired_open = open;
                if outcome.changed {
                    self.lifecycle = Lifecycle::Transitioning;
                }
            }
            OverviewAction::GestureBegin { value } => {
                if !value.is_finite() {
                    return outcome;
                }
                self.gesture = Some(Gesture {
                    tracker: SwipeTracker::new(),
                    start: value,
                    value,
                    initial_open: self.desired_open,
                });
                self.desired_open = true;
                self.lifecycle = Lifecycle::Gesture;
                outcome.changed = true;
            }
            OverviewAction::GestureUpdate { delta_y, timestamp } => {
                if !delta_y.is_finite() {
                    return outcome;
                }
                let Some(g) = &mut self.gesture else {
                    return outcome;
                };
                g.tracker.push(delta_y, timestamp);
                let value = RUBBER_BAND.clamp(0., 1., g.start + g.tracker.pos() / MOVEMENT);
                outcome.changed = g.value != value;
                g.value = value;
            }
            OverviewAction::GestureEnd { timestamp } => {
                let Some(mut g) = self.gesture.take() else {
                    return outcome;
                };
                g.tracker.push(0., timestamp);
                let to = (g.start + g.tracker.projected_end_pos() / MOVEMENT)
                    .clamp(0., 1.)
                    .round();
                let velocity = g.tracker.velocity() / MOVEMENT
                    * RUBBER_BAND.clamp_derivative(0., 1., g.start + g.tracker.pos() / MOVEMENT);
                self.settle(&mut outcome, g.value, to, velocity);
            }
            OverviewAction::GestureCancel { timestamp } => {
                let Some(mut g) = self.gesture.take() else {
                    return outcome;
                };
                g.tracker.push(0., timestamp);
                let velocity = g.tracker.velocity() / MOVEMENT
                    * RUBBER_BAND.clamp_derivative(0., 1., g.start + g.tracker.pos() / MOVEMENT);
                self.settle(
                    &mut outcome,
                    g.value,
                    if g.initial_open { 1. } else { 0. },
                    velocity,
                );
            }
            OverviewAction::PreviewOutput(connector) => {
                outcome.changed = self.preview_connector != connector;
                self.preview_connector = connector;
            }
            OverviewAction::OutputRemoved(connector) => {
                outcome.changed = self.stage.remove(&connector).is_some();
                if self.preview_connector.as_deref() == Some(&connector) {
                    self.preview_connector = None;
                    outcome.changed = true;
                }
            }
            OverviewAction::PresentationSettled => {
                if self.gesture.is_none() {
                    let lifecycle = if self.desired_open {
                        Lifecycle::Visible
                    } else {
                        Lifecycle::Hidden
                    };
                    outcome.changed = self.lifecycle != lifecycle;
                    self.lifecycle = lifecycle;
                    if !self.desired_open {
                        for state in self.stage.values_mut() {
                            state.close_anchor = None;
                        }
                    }
                }
            }
            OverviewAction::StageReset { connector, space } => {
                let next = StageSelection {
                    selected: Some(space),
                    initial: Some(space),
                    close_anchor: None,
                };
                outcome.changed = self.stage.get(&connector) != Some(&next);
                self.stage.insert(connector, next);
            }
            OverviewAction::StageSelect { connector, space } => {
                let state = self.stage.entry(connector).or_default();
                outcome.changed = state.selected != Some(space);
                state.selected = Some(space);
            }
            OverviewAction::StageCloseAnchor { connector, space } => {
                let state = self.stage.entry(connector).or_default();
                outcome.changed = state.close_anchor != space;
                state.close_anchor = space;
            }
            OverviewAction::RememberExpo(space) => {
                outcome.changed = self.initial_expo != space;
                self.initial_expo = space;
            }
            OverviewAction::RememberStage(space) => {
                outcome.changed = self.initial_stage != space;
                self.initial_stage = space;
            }
            OverviewAction::SpaceRemoved(space) => {
                for state in self.stage.values_mut() {
                    if state.selected == Some(space) {
                        state.selected = None;
                        outcome.changed = true;
                    }
                    if state.initial == Some(space) {
                        state.initial = None;
                        outcome.changed = true;
                    }
                    if state.close_anchor == Some(space) {
                        state.close_anchor = None;
                        outcome.changed = true;
                    }
                }
                for origin in [&mut self.initial_expo, &mut self.initial_stage] {
                    if *origin == Some(space) {
                        *origin = None;
                        outcome.changed = true;
                    }
                }
            }
        }
        outcome
    }

    fn settle(&mut self, outcome: &mut OverviewOutcome, from: f64, to: f64, velocity: f64) {
        self.desired_open = to == 1.;
        self.lifecycle = Lifecycle::Transitioning;
        outcome.changed = true;
        outcome.settlement = Some(OverviewSettlement { from, to, velocity });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enum_and_session_defaults_preserve_existing_distinction() {
        assert_eq!(OverviewMode::default(), OverviewMode::Stage);
        let state = OverviewState::default();
        assert_eq!(state.mode(), OverviewMode::Strip);
        assert!(!state.desired_open() && !state.visible());
    }

    #[test]
    fn cancel_restores_the_target_captured_before_grabbing_a_transition() {
        for initial in [false, true] {
            let mut s = OverviewState::default();
            s.apply(OverviewAction::SetOpen(initial));
            s.apply(OverviewAction::GestureBegin { value: 0.4 });
            s.apply(OverviewAction::GestureUpdate {
                delta_y: 90.,
                timestamp: Duration::from_millis(10),
            });
            let result = s
                .apply(OverviewAction::GestureCancel {
                    timestamp: Duration::from_millis(10),
                })
                .settlement
                .unwrap();
            assert_eq!(
                result,
                OverviewSettlement {
                    from: 0.7,
                    to: if initial { 1. } else { 0. },
                    velocity: 0.
                }
            );
            assert_eq!(s.desired_open(), initial);
            assert!(s.visible()); // Final cleanup remains until presentation settles.
            s.apply(OverviewAction::PresentationSettled);
            assert_eq!(s.visible(), initial);
        }
    }

    #[test]
    fn release_preserves_filtered_momentum_and_rubber_band_derivative() {
        let mut s = OverviewState::default();
        s.apply(OverviewAction::GestureBegin { value: 0. });
        for (ms, delta) in [(0, 30.), (10, 30.), (20, 30.)] {
            s.apply(OverviewAction::GestureUpdate {
                delta_y: delta,
                timestamp: Duration::from_millis(ms),
            });
        }
        let velocity = s.gesture_velocity();
        let r = s
            .apply(OverviewAction::GestureEnd {
                timestamp: Duration::from_millis(20),
            })
            .settlement
            .unwrap();
        assert_eq!(r.from, 0.3);
        assert_eq!(r.to, 1.);
        assert_eq!(r.velocity, velocity);
        assert_eq!(r.velocity, 10.);
        assert!(s.gesture_value().is_none());
    }

    #[test]
    fn mode_switch_releases_gesture_once_and_keeps_open_target() {
        let mut s = OverviewState::default();
        s.apply(OverviewAction::GestureBegin { value: 0.25 });
        let r = s.apply(OverviewAction::SetMode {
            mode: OverviewMode::Windows,
            timestamp: Duration::ZERO,
        });
        assert_eq!(
            r.settlement,
            Some(OverviewSettlement {
                from: 0.25,
                to: 1.,
                velocity: 0.
            })
        );
        assert!(s.desired_open() && s.gesture_value().is_none());
        assert!(s
            .apply(OverviewAction::GestureCancel {
                timestamp: Duration::from_millis(10)
            })
            .settlement
            .is_none());
        assert!(
            !s.apply(OverviewAction::SetMode {
                mode: OverviewMode::Windows,
                timestamp: Duration::ZERO
            })
            .changed
        );
    }

    #[test]
    fn preview_connector_is_not_keyboard_activation_and_unplug_prunes_it() {
        let mut s = OverviewState::default();
        s.apply(OverviewAction::PreviewOutput(Some("DP-2".into())));
        assert!(!s.desired_open() && !s.visible());
        s.apply(OverviewAction::OutputRemoved("DP-1".into()));
        assert_eq!(s.preview_connector(), Some("DP-2"));
        s.apply(OverviewAction::OutputRemoved("DP-2".into()));
        assert_eq!(s.preview_connector(), None);
    }

    #[test]
    fn invalid_motion_and_future_presentation_cannot_end_an_active_gesture() {
        let mut s = OverviewState::default();
        s.apply(OverviewAction::GestureBegin { value: 0.4 });
        for delta in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(
                !s.apply(OverviewAction::GestureUpdate {
                    delta_y: delta,
                    timestamp: Duration::MAX
                })
                .changed
            );
        }
        s.apply(OverviewAction::PresentationSettled);
        assert_eq!(s.gesture_value(), Some(0.4));
        assert!(s.desired_open() && s.visible());
    }
    #[test]
    fn cancel_and_mode_change_keep_event_time_momentum_and_idle_decay() {
        for sign in [-1., 1.] {
            for delayed in [false, true] {
                for mode_change in [false, true] {
                    let mut s = OverviewState::default();
                    s.apply(OverviewAction::GestureBegin { value: 0.5 });
                    let mut oracle = SwipeTracker::new();
                    for (ms, delta) in [(0, 15.), (10, 15.), (20, 15.)] {
                        let timestamp = Duration::from_millis(ms);
                        oracle.push(sign * delta, timestamp);
                        s.apply(OverviewAction::GestureUpdate {
                            delta_y: sign * delta,
                            timestamp,
                        });
                    }
                    let before = s.gesture_value().unwrap();
                    let timestamp = Duration::from_millis(if delayed { 300 } else { 20 });
                    oracle.push(0., timestamp);
                    let expected = oracle.velocity() / 300.
                        * RubberBand {
                            stiffness: 0.5,
                            limit: 0.05,
                        }
                        .clamp_derivative(
                            0.,
                            1.,
                            0.5 + oracle.pos() / 300.,
                        );
                    let action = if mode_change {
                        OverviewAction::SetMode {
                            mode: OverviewMode::Windows,
                            timestamp,
                        }
                    } else {
                        OverviewAction::GestureCancel { timestamp }
                    };
                    let result = s.apply(action).settlement.unwrap();
                    assert_eq!(result.from, before);
                    assert_eq!(result.velocity.to_bits(), expected.to_bits());
                    assert_eq!(result.velocity, if delayed { 0. } else { sign * 5. });
                    assert_eq!(result.to, if mode_change { 1. } else { 0. });
                }
            }
        }
    }
    #[test]
    fn stage_outputs_keep_independent_preview_and_global_cancel_origins() {
        let mut s = OverviewState::default();
        for (connector, id) in [("LEFT", 10), ("RIGHT", 20)] {
            s.apply(OverviewAction::StageReset {
                connector: connector.into(),
                space: SpaceId(id),
            });
        }
        s.apply(OverviewAction::StageSelect {
            connector: "LEFT".into(),
            space: SpaceId(11),
        });
        s.apply(OverviewAction::StageCloseAnchor {
            connector: "LEFT".into(),
            space: Some(SpaceId(11)),
        });
        s.apply(OverviewAction::RememberStage(Some(SpaceId(10))));
        s.apply(OverviewAction::RememberExpo(Some(SpaceId(20))));
        assert_eq!(s.stage("LEFT").unwrap().selected, Some(SpaceId(11)));
        assert_eq!(s.stage("LEFT").unwrap().initial, Some(SpaceId(10)));
        assert_eq!(s.stage("RIGHT").unwrap().selected, Some(SpaceId(20)));
        assert!(!s.desired_open()); // Preview is not keyboard activation.
        s.apply(OverviewAction::OutputRemoved("LEFT".into()));
        assert!(s.stage("LEFT").is_none());
        assert_eq!(s.initial_stage(), Some(SpaceId(10)));
        s.apply(OverviewAction::SpaceRemoved(SpaceId(10)));
        assert_eq!(s.initial_stage(), None);
        assert_eq!(s.initial_expo(), Some(SpaceId(20)));
    }

    #[test]
    fn transfer_prunes_output_local_selection_but_preserves_alive_cancel_identity() {
        let mut s = OverviewState::default();
        s.apply(OverviewAction::StageReset {
            connector: "LEFT".into(),
            space: SpaceId(10),
        });
        s.apply(OverviewAction::StageCloseAnchor {
            connector: "LEFT".into(),
            space: Some(SpaceId(10)),
        });
        s.apply(OverviewAction::RememberStage(Some(SpaceId(10))));
        s.apply(OverviewAction::RememberExpo(Some(SpaceId(10))));
        assert!(s.retain_stage_spaces(|connector, id| connector == "RIGHT" && id == SpaceId(10)));
        assert_eq!(*s.stage("LEFT").unwrap(), StageSelection::default());
        assert_eq!(s.initial_stage(), Some(SpaceId(10)));
        assert_eq!(s.initial_expo(), Some(SpaceId(10)));
        s.apply(OverviewAction::StageReset {
            connector: "RIGHT".into(),
            space: SpaceId(20),
        });
        s.apply(OverviewAction::StageSelect {
            connector: "RIGHT".into(),
            space: SpaceId(10),
        });
        s.apply(OverviewAction::SpaceRemoved(SpaceId(10)));
        assert_eq!(s.stage("RIGHT").unwrap().selected, None);
        assert_eq!(s.stage("RIGHT").unwrap().initial, Some(SpaceId(20)));
        assert_eq!(s.initial_stage(), None);
        assert_eq!(s.initial_expo(), None);
    }
    #[test]
    fn close_anchor_clears_at_owner_lifecycle_edges_not_forecast_updates() {
        let mut s = OverviewState::default();
        s.apply(OverviewAction::SetMode {
            mode: OverviewMode::Stage,
            timestamp: Duration::ZERO,
        });
        s.apply(OverviewAction::StageReset {
            connector: "LEFT".into(),
            space: SpaceId(10),
        });
        s.apply(OverviewAction::SetOpen(true));
        s.apply(OverviewAction::PresentationSettled);
        s.apply(OverviewAction::StageCloseAnchor {
            connector: "LEFT".into(),
            space: Some(SpaceId(10)),
        });
        s.apply(OverviewAction::SetOpen(false));
        assert_eq!(s.stage("LEFT").unwrap().close_anchor, Some(SpaceId(10)));
        assert!(s.visible());
        s.apply(OverviewAction::PresentationSettled);
        assert_eq!(s.stage("LEFT").unwrap().close_anchor, None);
        assert!(!s.visible());
        s.apply(OverviewAction::StageCloseAnchor {
            connector: "LEFT".into(),
            space: Some(SpaceId(10)),
        });
        s.apply(OverviewAction::SetMode {
            mode: OverviewMode::Expo,
            timestamp: Duration::ZERO,
        });
        assert_eq!(s.stage("LEFT").unwrap().close_anchor, None);
        assert_eq!(s.stage("LEFT").unwrap().selected, Some(SpaceId(10)));
    }
}
