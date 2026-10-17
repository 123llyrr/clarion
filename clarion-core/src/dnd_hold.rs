//! Drag-and-drop stationary-hover activation, owned by DesktopCore.
//! Input and lifecycle events advance the timer; render forecasts cannot.
use std::time::Duration;

use crate::stage_drag::StageOutput;
use crate::SpaceId;

/// Existing Clarion delay, inherited from the former GNOME-style adapter.
/// This is not an Apple measurement.
const HOLD_DELAY: Duration = Duration::from_millis(750);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DndTarget<I> {
    Window(I),
    Workspace(SpaceId),
}
#[derive(Debug, Clone, PartialEq)]
pub enum DndHoldAction<I> {
    /// Every actual pointer motion resets stationary-hover time, even when
    /// hit testing later returns the same target.
    Motion {
        connector: String,
    },
    Observe {
        connector: String,
        target: Option<DndTarget<I>>,
        timestamp: Duration,
        scrolled: bool,
    },
    End,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DndActivateIntent<I> {
    pub connector: String,
    pub space: SpaceId,
    pub window: Option<I>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct Hold<I> {
    target: DndTarget<I>,
    started: Duration,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DndHoldState<I> {
    connection: Option<StageOutput>,
    hold: Option<Hold<I>>,
}
impl<I> Default for DndHoldState<I> {
    fn default() -> Self {
        Self {
            connection: None,
            hold: None,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DndHoldOutcome<I> {
    pub changed: bool,
    pub activate: Option<DndTarget<I>>,
}
impl<I> Default for DndHoldOutcome<I> {
    fn default() -> Self {
        Self {
            changed: false,
            activate: None,
        }
    }
}
impl<I: Clone + Eq> DndHoldState<I> {
    pub fn connection(&self) -> Option<&StageOutput> {
        self.connection.as_ref()
    }
    pub fn target(&self) -> Option<&DndTarget<I>> {
        self.hold.as_ref().map(|hold| &hold.target)
    }
    /// The engine supplies the live connector generation and validates target
    /// membership first; the adapter cannot select a stale Output object.
    pub fn apply(&mut self, action: DndHoldAction<I>, generation: u64) -> DndHoldOutcome<I> {
        match action {
            DndHoldAction::Motion { connector } => {
                if connector.is_empty() || generation == 0 {
                    return DndHoldOutcome::default();
                }
                let connection = StageOutput {
                    connector,
                    generation,
                };
                let changed = self.connection.as_ref() != Some(&connection) || self.hold.is_some();
                self.connection = Some(connection);
                self.hold = None;
                DndHoldOutcome {
                    changed,
                    activate: None,
                }
            }
            DndHoldAction::Observe {
                connector,
                target,
                timestamp,
                scrolled,
            } => {
                if !self.connection.as_ref().is_some_and(|connection| {
                    connection.connector == connector && connection.generation == generation
                }) {
                    return DndHoldOutcome::default();
                }
                let Some(target) = target.filter(|_| !scrolled) else {
                    return DndHoldOutcome {
                        changed: self.hold.take().is_some(),
                        activate: None,
                    };
                };
                if self.hold.as_ref().is_none_or(|hold| hold.target != target) {
                    self.hold = Some(Hold {
                        target,
                        started: timestamp,
                    });
                    return DndHoldOutcome {
                        changed: true,
                        activate: None,
                    };
                }
                let hold = self.hold.as_ref().unwrap();
                if timestamp.saturating_sub(hold.started) < HOLD_DELAY {
                    return DndHoldOutcome::default();
                }
                // Consume once. A later stationary observation starts another
                // hold, matching established repeat-hover behavior.
                DndHoldOutcome {
                    changed: true,
                    activate: self.hold.take().map(|hold| hold.target),
                }
            }
            DndHoldAction::End => DndHoldOutcome {
                changed: self.end(),
                activate: None,
            },
        }
    }
    pub fn end(&mut self) -> bool {
        let changed = self.connection.is_some() || self.hold.is_some();
        self.connection = None;
        self.hold = None;
        changed
    }
    pub fn output_removed(&mut self, connector: &str) -> bool {
        if self
            .connection
            .as_ref()
            .is_some_and(|connection| connection.connector == connector)
        {
            self.end()
        } else {
            false
        }
    }
    /// Called on real unmap, move, layer/minimized changes. A window moving
    /// within the same connector still changes its target's spatial identity.
    pub fn invalidate_window(&mut self, id: &I) -> bool {
        if self.target() == Some(&DndTarget::Window(id.clone())) {
            self.hold.take().is_some()
        } else {
            false
        }
    }
    pub fn invalidate_space(&mut self, id: SpaceId) -> bool {
        if self.target() == Some(&DndTarget::Workspace(id)) {
            self.hold.take().is_some()
        } else {
            false
        }
    }
    pub fn retain_target(&mut self, mut valid: impl FnMut(&DndTarget<I>) -> bool) -> bool {
        if self.target().is_some_and(|target| !valid(target)) {
            self.hold.take().is_some()
        } else {
            false
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn start() -> DndHoldState<u64> {
        let mut state = DndHoldState::default();
        state.apply(
            DndHoldAction::Motion {
                connector: "left".into(),
            },
            7,
        );
        state
    }
    fn observe(
        state: &mut DndHoldState<u64>,
        target: Option<DndTarget<u64>>,
        ms: u64,
        scrolled: bool,
    ) -> DndHoldOutcome<u64> {
        state.apply(
            DndHoldAction::Observe {
                connector: "left".into(),
                target,
                timestamp: Duration::from_millis(ms),
                scrolled,
            },
            7,
        )
    }
    #[test]
    fn stationary_boundary_is_exact_and_each_intent_is_consumed_once() {
        let mut state = start();
        let target = Some(DndTarget::Window(1));
        assert!(observe(&mut state, target.clone(), 100, false).changed);
        assert_eq!(
            observe(&mut state, target.clone(), 849, false).activate,
            None
        );
        assert_eq!(
            observe(&mut state, target.clone(), 850, false).activate,
            target
        );
        assert_eq!(state.target(), None);
        assert_eq!(
            observe(&mut state, target.clone(), 850, false).activate,
            None
        );
        assert_eq!(
            observe(&mut state, target.clone(), 1600, false).activate,
            target
        );
    }
    #[test]
    fn actual_motion_restarts_even_the_same_target_without_extending_connection_lifetime() {
        let mut state = start();
        let target = Some(DndTarget::Window(1));
        observe(&mut state, target.clone(), 100, false);
        state.apply(
            DndHoldAction::Motion {
                connector: "left".into(),
            },
            7,
        );
        assert_eq!(
            observe(&mut state, target.clone(), 800, false).activate,
            None
        );
        assert_eq!(
            observe(&mut state, target.clone(), 1549, false).activate,
            None
        );
        assert_eq!(
            observe(&mut state, target.clone(), 1550, false).activate,
            target
        );
    }
    #[test]
    fn new_target_scroll_and_missing_target_reset_stationary_time() {
        let mut state = start();
        observe(&mut state, Some(DndTarget::Window(1)), 0, false);
        let target = Some(DndTarget::Workspace(SpaceId(20)));
        assert_eq!(
            observe(&mut state, target.clone(), 700, false).activate,
            None
        );
        assert_eq!(
            observe(&mut state, target.clone(), 1000, true).activate,
            None
        );
        assert_eq!(state.target(), None);
        observe(&mut state, target.clone(), 1200, false);
        observe(&mut state, None, 1500, false);
        assert_eq!(observe(&mut state, target, 2000, false).activate, None);
    }
    #[test]
    fn connection_epoch_and_output_retirement_reject_late_hover() {
        let mut state = start();
        observe(&mut state, Some(DndTarget::Window(1)), 0, false);
        let old = state.clone();
        assert_eq!(
            state
                .apply(
                    DndHoldAction::Observe {
                        connector: "left".into(),
                        target: Some(DndTarget::Window(1)),
                        timestamp: Duration::from_secs(2),
                        scrolled: false
                    },
                    8
                )
                .activate,
            None
        );
        assert_eq!(state, old);
        assert!(state.output_removed("left"));
        assert_eq!(
            observe(&mut state, Some(DndTarget::Window(1)), 2000, false).activate,
            None
        );
        state.apply(
            DndHoldAction::Motion {
                connector: "left".into(),
            },
            8,
        );
        assert_eq!(state.connection().unwrap().generation, 8);
        assert_eq!(state.target(), None);
    }
    #[test]
    fn window_moves_space_retirement_and_invalid_membership_clear_only_matching_target() {
        let mut state = start();
        observe(&mut state, Some(DndTarget::Window(1)), 0, false);
        assert!(!state.invalidate_window(&2));
        assert!(state.invalidate_window(&1));
        observe(
            &mut state,
            Some(DndTarget::Workspace(SpaceId(20))),
            800,
            false,
        );
        assert!(!state.invalidate_space(SpaceId(10)));
        assert!(state.invalidate_space(SpaceId(20)));
        observe(&mut state, Some(DndTarget::Window(2)), 1600, false);
        assert!(state.retain_target(|_| false));
        assert!(state.connection().is_some());
        assert!(state.end());
        assert!(!state.end());
    }
    #[test]
    fn an_earlier_event_timestamp_never_counts_forecast_elapsed_time() {
        let mut state = start();
        let target = Some(DndTarget::Window(1));
        observe(&mut state, target.clone(), 1000, false);
        assert_eq!(observe(&mut state, target.clone(), 0, false).activate, None);
        assert_eq!(
            observe(&mut state, target.clone(), 1749, false).activate,
            None
        );
        assert_eq!(
            observe(&mut state, target.clone(), 1750, false).activate,
            target
        );
    }
}
