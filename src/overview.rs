//! Compositor-level overview commands.
//!
//! Layout owns workspace/window geometry; this boundary owns the ordering of
//! presentation capture, input-grab teardown, layout mutation and redraw. Input
//! paths must not assemble those steps themselves. Layout's lower-level API
//! remains available for its renderer-independent tests and internal lifecycle.
use std::time::Duration;

use smithay::input::keyboard::Keysym;
use smithay::output::Output;
use smithay::utils::{Logical, Point};

use crate::compositor::{LockState, State};
use crate::layout::OverviewMode;

#[derive(Debug)]
pub(crate) enum OverviewCommand {
    Toggle,
    Open,
    Close,
    ToggleExpo,
    CancelExpo,
    ConfirmExpo,
    ToggleSpread,
    StageKey(Keysym),
    SpreadKey(Keysym),
    StageSelect {
        output: Output,
        position: Point<f64, Logical>,
    },
    EnterWorkspace {
        output: Output,
        index: usize,
    },
    GestureBegin,
    GestureUpdate {
        delta: f64,
        timestamp: Duration,
    },
    GestureEnd {
        cancelled: bool,
    },
}

impl State {
    /// Returns whether this command was handled. In particular, a stationary
    /// gesture update can be handled without scheduling another frame.
    pub(crate) fn overview_command(&mut self, command: OverviewCommand) -> bool {
        let target = match command {
            OverviewCommand::Toggle | OverviewCommand::Open => {
                Some(self.clarion.layout.default_overview_mode())
            }
            OverviewCommand::ToggleExpo => Some(OverviewMode::Expo),
            OverviewCommand::ToggleSpread => Some(OverviewMode::Windows),
            _ => None,
        };
        if let Some(target) = target {
            // Capture the currently presented drag/gesture before ungrabbing can
            // restore its source or finish its animation. Application DnD stays.
            self.prepare_overview_mode_transition(target);
            self.cancel_overview_grabs();
        }
        // Confirmation inside an ungrab callback must not recursively cancel
        // that same grab. Only mode-entry commands perform the teardown above.
        let (handled, redraw) = match command {
            OverviewCommand::Toggle => {
                self.clarion.layout.toggle_overview();
                (true, true)
            }
            OverviewCommand::Open => {
                let changed = self.clarion.layout.open_overview();
                (changed, changed)
            }
            OverviewCommand::Close => {
                let changed = self.clarion.layout.close_overview();
                (changed, changed)
            }
            OverviewCommand::ToggleExpo => {
                self.clarion.layout.toggle_expo();
                (true, true)
            }
            OverviewCommand::CancelExpo => {
                self.clarion.layout.cancel_expo();
                (true, true)
            }
            OverviewCommand::ConfirmExpo => {
                self.clarion.layout.confirm_expo();
                (true, true)
            }
            OverviewCommand::ToggleSpread => {
                self.clarion.layout.toggle_window_spread();
                (true, true)
            }
            OverviewCommand::StageKey(key) => {
                let handled = self.clarion.layout.stage_key(key);
                (handled, handled)
            }
            OverviewCommand::SpreadKey(key) => {
                let handled = self.clarion.layout.spread_key(key);
                (handled, handled)
            }
            OverviewCommand::StageSelect { output, position } => {
                let handled = self.clarion.layout.stage_select_at(&output, position);
                (handled, handled)
            }
            OverviewCommand::EnterWorkspace { output, index } => {
                self.clarion.layout.focus_output(&output);
                self.clarion.layout.toggle_overview_to_workspace(index);
                (true, true)
            }
            OverviewCommand::GestureBegin => {
                self.clarion.layout.overview_gesture_begin();
                (true, true)
            }
            OverviewCommand::GestureUpdate { delta, timestamp } => match self
                .clarion
                .layout
                .overview_gesture_update(delta, timestamp)
            {
                Some(redraw) => (true, redraw),
                None => (false, false),
            },
            OverviewCommand::GestureEnd { cancelled } => {
                let changed = if cancelled {
                    self.clarion.layout.overview_gesture_cancel()
                } else {
                    self.clarion.layout.overview_gesture_end()
                };
                (changed, changed)
            }
        };
        if redraw {
            self.clarion.queue_redraw_all();
        }
        handled
    }

    /// Capture the composed frame before changing overview geometry. Each privacy
    /// target gets its own snapshot, including any crossfade already on screen.
    fn prepare_overview_mode_transition(&mut self, target: OverviewMode) {
        if !self.clarion.layout.is_overview_visible()
            || self.clarion.layout.overview_mode() == target
        {
            return;
        }
        self.clarion.gesture_swipe_3f_cumulative = None;
        self.clarion.overview_scroll_swipe_gesture.reset();
        self.clarion.horizontal_wheel_tracker.reset();
        self.clarion.vertical_wheel_tracker.reset();
        let animations_disabled = {
            let config = self.clarion.config.borrow();
            config.animations.off || config.animations.overview_open_close.0.off
        };
        if animations_disabled
            || self.clarion.clock.should_complete_instantly()
            || !matches!(self.clarion.lock_state, LockState::Unlocked)
            || self.clarion.screenshot_ui.is_open()
            || self.clarion.window_mru_ui.is_open()
            || self.clarion.exit_confirm_dialog.is_open()
        {
            self.clarion.clear_overview_mode_transitions();
            return;
        }
        let outputs: Vec<_> = self.clarion.output_state.keys().cloned().collect();
        for output in outputs {
            let clarion = &mut self.clarion;
            let captured = self
                .backend
                .with_renderer_for_output(Some(&output), |renderer| match renderer {
                    crate::render_helpers::renderer::AnyRenderer::Gles(renderer) => {
                        clarion.do_overview_mode_transition(renderer, &output)
                    }
                    crate::render_helpers::renderer::AnyRenderer::Native(renderer) => {
                        clarion.do_overview_mode_transition(renderer, &output)
                    }
                });
            if captured.is_none() {
                self.clarion
                    .output_state
                    .get_mut(&output)
                    .unwrap()
                    .screen_transition = None;
            }
        }
    }

    // Snapshot lifecycle/privacy tests sometimes need a capture without changing
    // layout. Keep that diagnostic facility out of the production API.
    #[cfg(test)]
    pub(crate) fn capture_overview_test_frame(&mut self, target: OverviewMode) {
        self.prepare_overview_mode_transition(target);
    }
}
