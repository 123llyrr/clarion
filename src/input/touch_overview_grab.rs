use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use calloop::timer::{TimeoutAction, Timer};
use calloop::{LoopHandle, RegistrationToken};
use smithay::backend::input::InputTime;
use smithay::desktop::Window;
use smithay::input::pointer::{CursorIcon, CursorImageStatus};
use smithay::input::tablet::tool::{TabletToolGrab, TabletToolInnerHandle};
use smithay::input::tablet::{TabletSeatHandler, TabletSeatTrait};
use smithay::input::touch::{
    DownEvent, GrabStartData as TouchGrabStartData, MotionEvent, OrientationEvent, ShapeEvent,
    TouchGrab, TouchInnerHandle, UpEvent,
};
use smithay::input::{tablet, SeatHandler};
use smithay::output::Output;
use smithay::utils::{IsAlive, Logical, Point, SERIAL_COUNTER};
use tracing::warn;

use crate::compositor::State;
use crate::input::AnyStartData;
use crate::layout::workspace::{Workspace, WorkspaceId};
use crate::layout::OverviewMode;
use crate::overview::OverviewCommand;
use crate::utils::get_monotonic_time;
use crate::window::Mapped;

// When the touch is stationary for this much time, it becomes an interactive move.
const INTERACTIVE_MOVE_THRESHOLD: Duration = Duration::from_millis(500);

pub struct TouchOverviewGrab {
    start_data: AnyStartData<State>,
    start_timestamp: Duration,
    long_press_deadline: Instant,
    long_press: Arc<LongPress>,
    last_location: Point<f64, Logical>,
    output: Output,
    start_pos_within_output: Point<f64, Logical>,
    workspace_id: Option<WorkspaceId>,
    workspace_matched_narrow: bool,
    window: Option<Window>,
    overview_mode: OverviewMode,
    gesture: GestureState,
    // Only a normal release may activate a tap. Cancellation and grab replacement
    // also call unset(), but must not change the selected window or close overview.
    released: bool,
    stage_initial_selection: Option<WorkspaceId>,
    stage_motion: f64,

    // Accumulated and applied in frame().
    new_location: Point<f64, Logical>,
    event_timestamp: Option<Duration>,
}

#[derive(Default)]
struct LongPress {
    ready: AtomicBool,
    token: Mutex<Option<RegistrationToken>>,
}

#[derive(Debug, Clone, Copy)]
enum GestureState {
    Recognizing,
    ViewOffset,
    WorkspaceSwitch,
    InteractiveMove,
    StagePreview,
}

impl TouchOverviewGrab {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        start_data: AnyStartData<State>,
        start_timestamp: Duration,
        output: Output,
        start_pos_within_output: Point<f64, Logical>,
        workspace_id: Option<WorkspaceId>,
        workspace_matched_narrow: bool,
        window: Option<Window>,
        overview_mode: OverviewMode,
    ) -> Self {
        let location = start_data.location();

        Self {
            last_location: location,
            start_timestamp,
            long_press_deadline: Instant::now() + INTERACTIVE_MOVE_THRESHOLD,
            long_press: Arc::default(),
            start_data,
            output,
            start_pos_within_output,
            workspace_id,
            workspace_matched_narrow,
            window,
            overview_mode,
            gesture: GestureState::Recognizing,
            released: false,
            stage_initial_selection: None,
            stage_motion: 0.,
            new_location: location,
            event_timestamp: None,
        }
    }

    fn matches_long_press(&self, long_press: &Arc<LongPress>) -> bool {
        Arc::ptr_eq(&self.long_press, long_press)
    }

    fn cancel_long_press_timer(&self, event_loop: &LoopHandle<'static, State>) {
        if let Some(token) = self.long_press.token.lock().unwrap().take() {
            event_loop.remove(token);
        }
    }

    fn arm_long_press(
        &mut self,
        data: &mut State,
        wake: impl Fn(&mut State, &Arc<LongPress>) + 'static,
    ) {
        if self.long_press.token.lock().unwrap().is_some()
            || self.long_press.ready.load(Ordering::Relaxed)
            || !matches!(self.gesture, GestureState::Recognizing)
            || !self.window.as_ref().is_some_and(|window| window.alive())
        {
            return;
        }

        // The timer must not keep the grab or its device alive after teardown.
        let long_press = Arc::downgrade(&self.long_press);
        match data.clarion.event_loop.insert_source(
            Timer::from_deadline(self.long_press_deadline),
            move |_, _, state| {
                if let Some(long_press) = long_press.upgrade() {
                    long_press.token.lock().unwrap().take();
                    long_press.ready.store(true, Ordering::Relaxed);
                    wake(state, &long_press);
                }
                TimeoutAction::Drop
            },
        ) {
            Ok(source) => {
                *self.long_press.token.lock().unwrap() = Some(source);
            }
            Err(err) => warn!("error scheduling overview long press: {err}"),
        }
    }

    fn arm_touch_long_press(&mut self, data: &mut State) {
        self.arm_long_press(data, move |state, ready| {
            let Some(touch) = state.clarion.seat.get_touch() else {
                return;
            };
            let current = touch
                .with_grab(|_, grab| {
                    grab.as_any()
                        .downcast_ref::<Self>()
                        .is_some_and(|grab| grab.matches_long_press(ready))
                })
                .unwrap_or(false);
            if current {
                touch.frame(state);
            }
        });
    }

    fn arm_tablet_long_press(
        &mut self,
        data: &mut State,
        handle: &TabletToolInnerHandle<'_, State>,
    ) {
        let descriptor = handle.descriptor().clone();
        self.arm_long_press(data, move |state, ready| {
            let Some(tool) = state.clarion.seat.tablet_seat().get_tool(&descriptor) else {
                return;
            };
            let current = tool
                .with_grab(|_, grab| {
                    grab.as_any()
                        .downcast_ref::<Self>()
                        .is_some_and(|grab| grab.matches_long_press(ready))
                })
                .unwrap_or(false);
            if current {
                // Smithay skips tablet frames when no hardware data is pending.
                // An empty axis frame wakes our focus-cleared compositor grab;
                // it changes no pressure, position, tip, or button state.
                tool.axis(state, tablet::tool::AxisFrame::default());
                tool.frame(state, crate::utils::input_time());
            }
        });
    }

    fn on_frame(&mut self, data: &mut State) -> bool {
        if !data.clarion.layout.is_overview_open()
            || data.clarion.layout.overview_mode() != self.overview_mode
            || data
                .clarion
                .layout
                .monitor_for_output(&self.output)
                .is_none()
            || data.clarion.is_locked()
            || data.clarion.screenshot_ui.is_open()
            || data.clarion.exit_confirm_dialog.is_open()
            || data.clarion.window_mru_ui.is_open()
            || data.clarion.overview_mode_transition_active()
            || self.window.as_ref().is_some_and(|window| !window.alive())
        {
            return false;
        }
        // Stationary touchscreens and pens need not send another motion event.
        // The timer only wakes this grab; all modal/lifetime checks above still
        // run before a held contact may pick up its window.
        let Some(timestamp) = self.event_timestamp.take().or_else(|| {
            self.long_press
                .ready
                .load(Ordering::Relaxed)
                .then(get_monotonic_time)
        }) else {
            return true;
        };

        let layout = &mut data.clarion.layout;

        // Check if we should become interactive move.
        if matches!(self.gesture, GestureState::Recognizing) {
            if let Some(window) = self.window.as_ref().filter(|win| win.alive()) {
                let passed = timestamp.saturating_sub(self.start_timestamp);
                if (self.long_press.ready.load(Ordering::Relaxed)
                    || INTERACTIVE_MOVE_THRESHOLD <= passed)
                    && layout.interactive_move_begin(
                        window.clone(),
                        &self.output,
                        self.start_pos_within_output,
                    )
                {
                    self.gesture = GestureState::InteractiveMove;

                    if !self.start_data.is_touch() {
                        data.clarion
                            .cursor_manager
                            .set_cursor_image(CursorImageStatus::Named(CursorIcon::Grabbing));
                    }
                }
            }
        }

        // Check if we should become a spatial scroll.
        if matches!(self.gesture, GestureState::Recognizing) {
            let c = self.new_location - self.start_data.location();

            // Check if the gesture moved far enough to decide. Threshold copied from libadwaita.
            if c.x * c.x + c.y * c.y >= 16. * 16. {
                if layout.is_stage_open() {
                    self.stage_initial_selection = layout.stage_selected_for_output(&self.output);
                    self.gesture = GestureState::StagePreview;
                } else if let Some(ws_id) = self.workspace_id.filter(|_| c.x.abs() > c.y.abs()) {
                    if let Some((ws_idx, ws)) = layout.find_workspace_by_id(ws_id) {
                        if ws.current_output() == Some(&self.output) {
                            layout.view_offset_gesture_begin(&self.output, Some(ws_idx), false);
                            self.gesture = GestureState::ViewOffset;

                            if !self.start_data.is_touch() {
                                data.clarion.cursor_manager.set_cursor_image(
                                    CursorImageStatus::Named(CursorIcon::AllScroll),
                                );
                            }
                        }
                    }
                }

                if matches!(self.gesture, GestureState::Recognizing) {
                    layout.workspace_switch_gesture_begin(&self.output, false);
                    self.gesture = GestureState::WorkspaceSwitch;

                    if !self.start_data.is_touch() {
                        data.clarion
                            .cursor_manager
                            .set_cursor_image(CursorImageStatus::Named(CursorIcon::AllScroll));
                    }
                }
            }
        }

        // Do nothing if still recognizing.
        if matches!(self.gesture, GestureState::Recognizing) {
            return true;
        }
        self.cancel_long_press_timer(&data.clarion.event_loop);

        let delta = self.new_location - self.last_location;
        self.last_location = self.new_location;

        let ongoing = match self.gesture {
            GestureState::Recognizing => unreachable!(),
            GestureState::StagePreview => {
                self.stage_motion -= delta.x;
                let steps = (self.stage_motion / 64.).trunc() as isize;
                if steps != 0 {
                    self.stage_motion %= 64.;
                    layout.stage_navigate(Some(&self.output), steps);
                }
                layout.is_stage_open()
            }
            GestureState::ViewOffset => layout
                .view_offset_gesture_update(-delta.x, timestamp, false)
                .is_some(),
            GestureState::WorkspaceSwitch => layout
                .workspace_switch_gesture_update(-delta.y, timestamp, false)
                .is_some(),
            GestureState::InteractiveMove => {
                // The window may be destroyed while a touch/tablet grab is still
                // active. End the gesture cleanly instead of dereferencing a
                // stale handle during the next motion frame.
                let Some(window) = self.window.as_ref().filter(|window| window.alive()) else {
                    return false;
                };
                if let Some((output, pos_within_output)) =
                    data.clarion.output_under(self.new_location)
                {
                    let output = output.clone();
                    data.clarion.layout.interactive_move_update(
                        window,
                        delta,
                        output,
                        pos_within_output,
                    )
                } else {
                    false
                }
            }
        };

        if ongoing {
            data.clarion.queue_redraw_all();
        }

        ongoing
    }

    fn on_ungrab(&mut self, state: &mut State) {
        self.cancel_long_press_timer(&state.clarion.event_loop);
        match self.gesture {
            GestureState::Recognizing if self.released => {
                // Tap to activate.
                if state.overview_command(OverviewCommand::StageSelect {
                    output: self.output.clone(),
                    position: self.start_pos_within_output,
                }) {
                    state.clarion.queue_redraw_all();
                    return;
                }
                state.clarion.layout.focus_output(&self.output);

                // Activate the workspace if necessary.
                if self.window.is_some() || self.workspace_matched_narrow {
                    // When activating a window, we want to activate the window's current
                    // workspace. Otherwise, find the workspace that we tapped on.
                    let ws_matches = |ws: &Workspace<Mapped>| {
                        if let Some(window) = &self.window {
                            ws.has_window(window)
                        } else if let Some(ws_id) = self.workspace_id {
                            ws.id() == ws_id
                        } else {
                            false
                        }
                    };

                    let ws_idx = if let Some((Some(mon), ws_idx, _)) = state
                        .clarion
                        .layout
                        .workspaces()
                        .find(|(_, _, ws)| ws_matches(ws))
                    {
                        // The workspace could've moved to a different output in the meantime.
                        (*mon.output() == self.output).then_some(ws_idx)
                    } else {
                        None
                    };

                    if let Some(ws_idx) = ws_idx {
                        state.overview_command(OverviewCommand::EnterWorkspace {
                            output: self.output.clone(),
                            index: ws_idx,
                        });
                    }
                }

                if let Some(window) = self.window.as_ref().filter(|window| window.alive()) {
                    state.clarion.layout.activate_window(window);
                }
            }
            GestureState::Recognizing => {}
            GestureState::StagePreview => {
                if !self.released {
                    if let Some(initial) = self.stage_initial_selection {
                        state.clarion.layout.stage_select(&self.output, initial);
                    }
                }
            }
            GestureState::ViewOffset => {
                if self.released {
                    state.clarion.layout.view_offset_gesture_end(Some(false));
                } else {
                    state.clarion.layout.view_offset_gesture_cancel(Some(false));
                }
            }
            GestureState::WorkspaceSwitch => {
                if self.released {
                    state
                        .clarion
                        .layout
                        .workspace_switch_gesture_end(Some(false));
                } else {
                    state
                        .clarion
                        .layout
                        .workspace_switch_gesture_cancel(Some(false));
                }
            }
            GestureState::InteractiveMove => {
                if let Some(window) = self.window.as_ref() {
                    if self.released {
                        state.clarion.layout.interactive_move_end(window);
                    } else {
                        state.clarion.layout.interactive_move_cancel(window);
                    }
                }
            }
        };

        if !self.start_data.is_touch() {
            state
                .clarion
                .cursor_manager
                .set_cursor_image(CursorImageStatus::default_named());
        }

        state.clarion.queue_redraw_all();
    }
}

impl TouchGrab<State> for TouchOverviewGrab {
    fn down(
        &mut self,
        data: &mut State,
        handle: &mut TouchInnerHandle<'_, State>,
        _focus: Option<(<State as SeatHandler>::TouchFocus, Point<f64, Logical>)>,
        event: &DownEvent,
    ) {
        handle.down(data, None, event);

        if event.slot == self.start_data.unwrap_touch().slot {
            self.arm_touch_long_press(data);
            return;
        }

        if matches!(self.gesture, GestureState::InteractiveMove) {
            if let Some(window) = &self.window.as_ref() {
                data.clarion.layout.toggle_window_floating(Some(window));
                data.clarion.queue_redraw_all();
            }
        }
    }

    fn up(&mut self, data: &mut State, handle: &mut TouchInnerHandle<'_, State>, event: &UpEvent) {
        handle.up(data, event);

        if event.slot != self.start_data.unwrap_touch().slot {
            return;
        }

        // Motion and up may share a hardware frame. Consume the final position
        // before ending the gesture, including recognition of a pending swipe.
        self.released = self.on_frame(data);
        handle.unset_grab(self, data);
    }

    fn motion(
        &mut self,
        data: &mut State,
        handle: &mut TouchInnerHandle<'_, State>,
        _focus: Option<(<State as SeatHandler>::TouchFocus, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);

        if event.slot != self.start_data.unwrap_touch().slot {
            return;
        }

        self.new_location = event.location;
        self.event_timestamp = Some(Duration::from_micros(event.time.micros()));
    }

    fn frame(&mut self, data: &mut State, handle: &mut TouchInnerHandle<'_, State>) {
        handle.frame(data);
        self.arm_touch_long_press(data);

        if !self.on_frame(data) {
            // The gesture is no longer ongoing.
            handle.unset_grab(self, data);
        }
    }

    fn cancel(&mut self, data: &mut State, handle: &mut TouchInnerHandle<'_, State>) {
        handle.cancel(data);
        handle.unset_grab(self, data);
    }

    fn shape(
        &mut self,
        data: &mut State,
        handle: &mut TouchInnerHandle<'_, State>,
        event: &ShapeEvent,
    ) {
        handle.shape(data, event);
    }

    fn orientation(
        &mut self,
        data: &mut State,
        handle: &mut TouchInnerHandle<'_, State>,
        event: &OrientationEvent,
    ) {
        handle.orientation(data, event);
    }

    fn start_data(&self) -> &TouchGrabStartData<State> {
        self.start_data.unwrap_touch()
    }

    fn unset(&mut self, data: &mut State) {
        self.on_ungrab(data);
    }
}

impl TabletToolGrab<State> for TouchOverviewGrab {
    fn start_data(&self) -> &tablet::tool::GrabStartData<State> {
        self.start_data.unwrap_tablet_tool()
    }

    fn proximity_out(
        &mut self,
        data: &mut State,
        handle: &mut TabletToolInnerHandle<'_, State>,
        event: &tablet::tool::ProximityOutEvent,
    ) {
        handle.proximity_out(data, event);
        handle.unset_grab(self, data, event.serial, event.time, false);
    }

    fn motion(
        &mut self,
        data: &mut State,
        handle: &mut TabletToolInnerHandle<'_, State>,
        _focus: Option<(<State as TabletSeatHandler>::ToolFocus, Point<f64, Logical>)>,
        event: &tablet::tool::MotionEvent,
    ) {
        handle.motion(data, None, event);

        self.new_location = event.location;
        self.event_timestamp = Some(Duration::from_micros(event.time.micros()));
    }

    fn down(
        &mut self,
        data: &mut State,
        handle: &mut TabletToolInnerHandle<'_, State>,
        event: &tablet::tool::DownEvent,
    ) {
        handle.down(data, event);
        self.arm_tablet_long_press(data, handle);
    }

    fn up(
        &mut self,
        data: &mut State,
        handle: &mut TabletToolInnerHandle<'_, State>,
        event: &tablet::tool::UpEvent,
    ) {
        handle.up(data, event);
        self.released = self.on_frame(data);
        handle.unset_grab(self, data, event.serial, event.time, true);
    }

    fn button(
        &mut self,
        data: &mut State,
        handle: &mut TabletToolInnerHandle<'_, State>,
        event: &tablet::tool::ButtonEvent,
    ) {
        handle.button(data, event);
    }

    fn axis(
        &mut self,
        data: &mut State,
        handle: &mut TabletToolInnerHandle<'_, State>,
        frame: tablet::tool::AxisFrame,
    ) {
        handle.axis(data, frame);
    }

    fn frame(
        &mut self,
        data: &mut State,
        handle: &mut TabletToolInnerHandle<'_, State>,
        time: InputTime,
    ) {
        handle.frame(data, time);
        self.arm_tablet_long_press(data, handle);

        if !self.on_frame(data) {
            // The gesture is no longer ongoing.
            handle.unset_grab(
                self,
                data,
                SERIAL_COUNTER.next_serial(),
                crate::utils::input_time(),
                true,
            );
        }
    }

    fn unset(&mut self, data: &mut State) {
        self.on_ungrab(data);
    }
}
