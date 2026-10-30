use std::any::Any;
use std::collections::hash_map::Entry;
use std::collections::HashSet;
use std::time::Duration;

use calloop::timer::{TimeoutAction, Timer};
use clarion_config::{
    Action, Bind, Binds, Config, Key, ModKey, Modifiers, MruDirection, SwitchBinds, Trigger,
};
use clarion_ipc::LayoutSwitchTarget;
use input::event::gesture::GestureEventCoordinates as _;
use smithay::backend::input::{
    AbsolutePositionEvent, Axis, AxisSource, ButtonState, Device, DeviceCapability, Event,
    GestureBeginEvent, GestureEndEvent, GesturePinchUpdateEvent as _, GestureSwipeUpdateEvent as _,
    InputEvent, KeyState, KeyboardKeyEvent, Keycode, MouseButton, PointerAxisEvent,
    PointerButtonEvent, PointerMotionEvent, ProximityState, Switch, SwitchState, SwitchToggleEvent,
    TabletToolButtonEvent, TabletToolEvent, TabletToolProximityEvent, TabletToolTipEvent,
    TabletToolTipState, TouchEvent,
};
use smithay::backend::libinput::LibinputInputBackend;
use smithay::input::dnd::DnDGrab;
use smithay::input::keyboard::{keysyms, FilterResult, Keysym, Layout, ModifiersState};
use smithay::input::pointer::{
    AxisFrame, ButtonEvent, CursorIcon, CursorImageStatus, Focus, GestureHoldBeginEvent,
    GestureHoldEndEvent, GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent,
    GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent,
    GrabStartData as PointerGrabStartData, MotionEvent, PointerGrab, RelativeMotionEvent,
};
use smithay::input::tablet::tool::GrabStartData as TabletToolGrabStartData;
use smithay::input::tablet::{TabletDescriptor, TabletSeatHandler, TabletSeatTrait};
use smithay::input::touch::{
    DownEvent, GrabStartData as TouchGrabStartData, MotionEvent as TouchMotionEvent, UpEvent,
};
use smithay::input::{tablet, SeatHandler};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_data_source::WlDataSource;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle, Transform, SERIAL_COUNTER};
use smithay::wayland::keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitor;
use smithay::wayland::pointer_constraints::{with_pointer_constraint, PointerConstraint};
use touch_overview_grab::TouchOverviewGrab;

use self::move_grab::MoveGrab;
use self::pick_color_grab::PickColorGrab;
use self::pick_window_grab::PickWindowGrab;
use self::resize_grab::ResizeGrab;
use self::spatial_movement_grab::SpatialMovementGrab;
use crate::compositor::{
    CastTarget, HotCornerKind, KeyboardFocus, LockState, PointerVisibility, State,
};
#[cfg(feature = "dbus")]
use crate::dbus::freedesktop_a11y::KbMonBlock;
use crate::layout::scrolling::ScrollDirection;
use crate::layout::{ActivateWindow, LayoutElement as _};
use crate::overview::OverviewCommand;
use crate::ui::hotkey_overlay::HotkeyOverlay;
use crate::ui::mru::{WindowMru, WindowMruUi};
use crate::ui::screenshot_ui::ScreenshotUi;
use crate::utils::spawning::{spawn, spawn_sh};
use crate::utils::{center, get_monotonic_time, CastSessionId, ResizeEdge};

pub mod backend_ext;
pub mod click_grab;
pub mod hot_corner;
pub mod launcher_gesture;
pub mod move_grab;
pub mod pick_color_grab;
pub mod pick_window_grab;
pub mod resize_grab;
pub mod spatial_movement_grab;
pub mod touch_overview_grab;
use backend_ext::{ClarionInputBackend as InputBackend, ClarionInputDevice as _};

pub use crate::gestures::{scroll_swipe_gesture, scroll_tracker, swipe_tracker};

/// A compositor shortcut repeat belongs to one physical key and modifier chord.
pub struct BindRepeat {
    token: calloop::RegistrationToken,
    key_code: Keycode,
    modifiers: Modifiers,
}

pub const DOUBLE_CLICK_TIME: Duration = Duration::from_millis(400);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TabletData {
    pub aspect_ratio: f64,
}

pub enum AnyStartData<D: SeatHandler + TabletSeatHandler> {
    Pointer(PointerGrabStartData<D>),
    Touch(TouchGrabStartData<D>),
    TabletTool(TabletToolGrabStartData<D>),
}

impl<D: SeatHandler + TabletSeatHandler> AnyStartData<D> {
    pub fn location(&self) -> Point<f64, Logical> {
        match self {
            AnyStartData::Pointer(x) => x.location,
            AnyStartData::Touch(x) => x.location,
            AnyStartData::TabletTool(x) => x.location,
        }
    }

    pub fn unwrap_pointer(&self) -> &PointerGrabStartData<D> {
        match self {
            AnyStartData::Pointer(x) => x,
            AnyStartData::Touch(_) | AnyStartData::TabletTool(_) => {
                panic!("start_data is not Pointer")
            }
        }
    }

    pub fn unwrap_touch(&self) -> &TouchGrabStartData<D> {
        match self {
            AnyStartData::Pointer(_) | AnyStartData::TabletTool(_) => {
                panic!("start_data is not Touch")
            }
            AnyStartData::Touch(x) => x,
        }
    }

    pub fn unwrap_tablet_tool(&self) -> &TabletToolGrabStartData<D> {
        match self {
            AnyStartData::Pointer(_) | AnyStartData::Touch(_) => {
                panic!("start_data is not TabletTool")
            }
            AnyStartData::TabletTool(x) => x,
        }
    }

    pub fn is_pointer(&self) -> bool {
        matches!(self, Self::Pointer(_))
    }

    pub fn is_touch(&self) -> bool {
        matches!(self, Self::Touch(_))
    }

    pub fn is_tablet_tool(&self) -> bool {
        matches!(self, Self::TabletTool(_))
    }
}

impl State {
    pub fn process_input_event<I: InputBackend + 'static>(&mut self, event: InputEvent<I>)
    where
        I::Device: 'static, // Needed for downcasting.
    {
        let _span = clarion_tracy_span!("process_input_event");

        // Discrete events observe all motion before them. Continuous pointer packets
        // keep protocol ordering but defer desktop animation/layout work to the frame.
        if !matches!(
            &event,
            InputEvent::PointerMotion { .. } | InputEvent::PointerMotionAbsolute { .. }
        ) {
            self.clarion.layout.flush_pending_move();
            self.clarion.advance_animations();
        }

        if self.clarion.monitors_active {
            // Notify the idle-notifier of activity.
            if should_notify_activity(&event) {
                self.clarion.notify_activity();
            }
        } else {
            // Power on monitors if they were off.
            if should_activate_monitors(&event) {
                self.clarion.activate_monitors(&mut self.backend);

                // Notify the idle-notifier of activity only if we're also powering on the
                // monitors.
                self.clarion.notify_activity();
            }
        }

        if should_reset_pointer_inactivity_timer(&event) {
            self.clarion.reset_pointer_inactivity_timer();
        }

        let hide_hotkey_overlay =
            self.clarion.hotkey_overlay.is_open() && should_hide_hotkey_overlay(&event);

        let hide_exit_confirm_dialog =
            self.clarion.exit_confirm_dialog.is_open() && should_hide_exit_confirm_dialog(&event);

        let mut consumed_by_a11y = false;
        let mut consumed_by_hotkey_overlay = false;
        use InputEvent::*;
        match event {
            DeviceAdded { device } => self.on_device_added(device),
            DeviceRemoved { device } => self.on_device_removed(device),
            Keyboard { event } => self.on_keyboard::<I>(
                event,
                &mut consumed_by_a11y,
                &mut consumed_by_hotkey_overlay,
            ),
            PointerMotion { event } => self.on_pointer_motion::<I>(event),
            PointerMotionAbsolute { event } => self.on_pointer_motion_absolute::<I>(event),
            PointerButton { event } => self.on_pointer_button::<I>(event),
            PointerAxis { event } => self.on_pointer_axis::<I>(event),
            TabletToolAxis { event } => self.on_tablet_tool_axis::<I>(event),
            TabletToolTip { event } => self.on_tablet_tool_tip::<I>(event),
            TabletToolProximity { event } => self.on_tablet_tool_proximity::<I>(event),
            TabletToolButton { event } => self.on_tablet_tool_button::<I>(event),
            GestureSwipeBegin { event } => self.on_gesture_swipe_begin::<I>(event),
            GestureSwipeUpdate { event } => self.on_gesture_swipe_update::<I>(event),
            GestureSwipeEnd { event } => self.on_gesture_swipe_end::<I>(event),
            GesturePinchBegin { event } => self.on_gesture_pinch_begin::<I>(event),
            GesturePinchUpdate { event } => self.on_gesture_pinch_update::<I>(event),
            GesturePinchEnd { event } => self.on_gesture_pinch_end::<I>(event),
            GestureHoldBegin { event } => self.on_gesture_hold_begin::<I>(event),
            GestureHoldEnd { event } => self.on_gesture_hold_end::<I>(event),
            TouchDown { event } => self.on_touch_down::<I>(event),
            TouchMotion { event } => self.on_touch_motion::<I>(event),
            TouchUp { event } => self.on_touch_up::<I>(event),
            TouchCancel { event } => self.on_touch_cancel::<I>(event),
            TouchFrame { event } => self.on_touch_frame::<I>(event),
            SwitchToggle { event } => self.on_switch_toggle::<I>(event),
            Special(_) => (),
        }

        // Don't hide overlays if consumed by a11y, so that you can use the screen reader
        // navigation keys.
        if consumed_by_a11y || consumed_by_hotkey_overlay {
            return;
        }

        // Do this last so that screenshot still gets it.
        if hide_hotkey_overlay && self.clarion.hotkey_overlay.hide() {
            self.clarion.queue_redraw_all();
        }

        if hide_exit_confirm_dialog && self.clarion.exit_confirm_dialog.hide() {
            self.clarion.queue_redraw_all();
        }
    }

    pub fn process_libinput_event(&mut self, event: &mut InputEvent<LibinputInputBackend>) {
        let _span = clarion_tracy_span!("process_libinput_event");

        match event {
            InputEvent::DeviceAdded { device } => {
                self.clarion.devices.insert(device.clone());

                if device.has_capability(input::DeviceCapability::TabletTool) {
                    match device.size() {
                        Some((w, h)) => {
                            let aspect_ratio = w / h;
                            let data = TabletData { aspect_ratio };
                            self.clarion.tablets.insert(device.clone(), data);
                        }
                        None => {
                            warn!("tablet tool device has no size");
                        }
                    }
                }

                if device.has_capability(input::DeviceCapability::Keyboard) {
                    if let Some(led_state) = self
                        .clarion
                        .seat
                        .get_keyboard()
                        .map(|keyboard| keyboard.led_state())
                    {
                        device.led_update(led_state.into());
                    }
                }

                if device.has_capability(input::DeviceCapability::Touch) {
                    self.clarion.touch.insert(device.clone());
                }

                apply_libinput_settings(&self.clarion.config.borrow().input, device);
            }
            InputEvent::DeviceRemoved { device } => {
                self.clarion.touch.remove(device);
                self.clarion.tablets.remove(device);
                self.clarion.devices.remove(device);
            }
            _ => (),
        }
    }

    fn on_device_added(&mut self, device: impl Device) {
        if device.has_capability(DeviceCapability::TabletTool) {
            let tablet_seat = self.clarion.seat.tablet_seat();

            let desc = TabletDescriptor::from(&device);
            tablet_seat.add_wp_tablet(&self.clarion.display_handle, &desc);
        }
        if device.has_capability(DeviceCapability::Touch) && self.clarion.seat.get_touch().is_none()
        {
            self.clarion.seat.add_touch();
        }
    }

    fn on_device_removed(&mut self, device: impl Device) {
        if device.has_capability(DeviceCapability::Keyboard) {
            self.stop_key_repeat();
        }
        if device.has_capability(DeviceCapability::TabletTool) {
            let tablet_seat = self.clarion.seat.tablet_seat();

            let desc = TabletDescriptor::from(&device);
            tablet_seat.remove_tablet(&desc);

            // If there are no tablets in seat we can remove all tools
            if tablet_seat.count_tablets() == 0 {
                tablet_seat.clear_tools();
            }
        }
        if device.has_capability(DeviceCapability::Touch) && self.clarion.touch.is_empty() {
            self.clarion.seat.remove_touch();
        }
    }

    /// Computes the rectangle that covers all outputs in global space.
    fn global_bounding_rectangle(&self) -> Option<Rectangle<i32, Logical>> {
        self.clarion.global_space.outputs().fold(
            None,
            |acc: Option<Rectangle<i32, Logical>>, output| {
                self.clarion
                    .global_space
                    .output_geometry(output)
                    .map(|geo| acc.map(|acc| acc.merge(geo)).unwrap_or(geo))
            },
        )
    }

    /// Computes the cursor position for the tablet event.
    ///
    /// This function handles the tablet output mapping, as well as coordinate clamping and aspect
    /// ratio correction.
    fn compute_tablet_position<I: InputBackend>(
        &self,
        event: &(impl Event<I> + TabletToolEvent<I>),
    ) -> Option<Point<f64, Logical>>
    where
        I::Device: 'static,
    {
        let device_output = event.device().output(self);
        let device_output = device_output.filter(|output| self.clarion.output_exists(output));
        let device_output = device_output.as_ref();
        let mapped_output = device_output.or_else(|| self.clarion.output_for_tablet());

        // If the tablet is configured to map to the focused window, use that window's geometry on
        // the mapped output (or on the focused output if no specific output is mapped).
        let map_to_focused_window = self
            .clarion
            .config
            .borrow()
            .input
            .tablet
            .map_to_focused_window;
        // But only if the keyboard focus is on the layout, so that it doesn't trigger on the lock
        // screen and such.
        let window_target = if map_to_focused_window && self.clarion.keyboard_focus.is_layout() {
            let output = mapped_output.or_else(|| self.clarion.layout.active_output());
            output.and_then(|output| {
                let monitor = self.clarion.layout.monitor_for_output(output)?;
                let mut rect = monitor.active_window_visual_rectangle()?;
                let output_geo = self.clarion.global_space.output_geometry(output)?;
                rect.loc += output_geo.loc.to_f64();
                Some((rect, output))
            })
        } else {
            None
        };

        let (target_geo, keep_ratio, px, transform) = if let Some((rect, output)) = window_target {
            (
                rect,
                true,
                1. / output.current_scale().fractional_scale(),
                output.current_transform(),
            )
        } else if let Some(output) = mapped_output {
            let geo = self.clarion.global_space.output_geometry(output)?;
            (
                geo.to_f64(),
                true,
                1. / output.current_scale().fractional_scale(),
                output.current_transform(),
            )
        } else {
            let geo = self.global_bounding_rectangle()?.to_f64();

            // FIXME: this 1 px size should ideally somehow be computed for the rightmost output
            // corresponding to the position on the right when clamping.
            let output = self.clarion.global_space.outputs().next()?;
            let scale = output.current_scale().fractional_scale();

            // Do not keep ratio for the unified mode as this is what OpenTabletDriver expects.
            (geo, false, 1. / scale, Transform::Normal)
        };

        let mut pos = {
            let size = transform.invert().transform_size(target_geo.size);
            transform.transform_point_in(event.position_transformed(size.to_i32_round()), &size)
        };

        if keep_ratio {
            pos.x /= target_geo.size.w;
            pos.y /= target_geo.size.h;

            let device = event.device();
            if let Some(device) = (&device as &dyn Any).downcast_ref::<input::Device>() {
                if let Some(data) = self.clarion.tablets.get(device) {
                    // This code does the same thing as mutter with "keep aspect ratio" enabled.
                    let size = transform.invert().transform_size(target_geo.size);
                    let output_aspect_ratio = size.w / size.h;
                    let ratio = data.aspect_ratio / output_aspect_ratio;

                    if ratio > 1. {
                        pos.x *= ratio;
                    } else {
                        pos.y /= ratio;
                    }
                }
            };

            pos.x *= target_geo.size.w;
            pos.y *= target_geo.size.h;
        }

        pos.x = pos.x.clamp(0.0, target_geo.size.w - px);
        pos.y = pos.y.clamp(0.0, target_geo.size.h - px);
        Some(pos + target_geo.loc)
    }

    fn is_inhibiting_shortcuts(&self) -> bool {
        self.clarion
            .keyboard_focus
            .surface()
            .and_then(|surface| {
                self.clarion
                    .keyboard_shortcuts_inhibiting_surfaces
                    .get(surface)
            })
            .is_some_and(KeyboardShortcutsInhibitor::is_active)
    }

    pub(crate) fn desktop_owns_macos_shortcuts(&self) -> bool {
        if !matches!(self.clarion.lock_state, LockState::Unlocked)
            || self.clarion.screenshot_ui.is_open()
            || self.clarion.window_mru_ui.is_open()
            || self.clarion.exit_confirm_dialog.is_open()
            || self.clarion.hotkey_overlay.is_open()
        {
            return false;
        }
        let KeyboardFocus::LayerShell { surface } = &self.clarion.keyboard_focus else {
            return false;
        };
        // Logical focus names the root even while a popup owns the actual seat.
        // Only the base desktop, never an interaction view or popup, opts in.
        if self
            .clarion
            .seat
            .get_keyboard()
            .unwrap()
            .current_focus()
            .as_ref()
            != Some(surface)
        {
            return false;
        }
        self.clarion
            .mapped_layer_surfaces
            .keys()
            .any(|layer| layer.wl_surface() == surface && layer.namespace() == "clarion-desktop")
            && crate::handlers::gtk_shell::surface_application_id_matches(
                surface,
                "io.github.clarion.Desktop.MacOS",
            )
    }

    fn shell_launcher_available(&self) -> bool {
        !self.clarion.is_locked()
            && !self.clarion.screenshot_ui.is_open()
            && !self.clarion.window_mru_ui.is_open()
            && !self.clarion.exit_confirm_dialog.is_open()
            && !self.clarion.hotkey_overlay.is_open()
            && !self.clarion.overview_mode_transition_active()
            && !self.clarion.layout.is_overview_open()
            && !self.is_inhibiting_shortcuts()
            && !self
                .clarion
                .seat
                .get_pointer()
                .is_some_and(|pointer| pointer.is_grabbed())
            && self
                .clarion
                .mapped_layer_surfaces
                .keys()
                .any(|layer| layer.namespace() == "clarion-shell")
    }

    fn on_keyboard<I: InputBackend>(
        &mut self,
        event: I::KeyboardKeyEvent,
        consumed_by_a11y: &mut bool,
        consumed_by_hotkey_overlay: &mut bool,
    ) {
        let mod_key = self.backend.mod_key(&self.clarion.config.borrow());

        let serial = SERIAL_COUNTER.next_serial();
        let time = Event::time(&event);
        let pressed = event.state() == KeyState::Pressed;

        // Releasing an older held shortcut must not cancel the newer one.
        self.update_key_repeat(event.key_code(), pressed, None);

        if pressed {
            self.hide_cursor_if_needed();
        }

        let is_inhibiting_shortcuts = self.is_inhibiting_shortcuts();

        // Accessibility modifier grabs should override XKB state changes (e.g. Caps Lock), so we
        // need to process them before keyboard.input() below.
        //
        // Other accessibility-grabbed keys should still update our XKB state, but not cause any
        // other changes.
        #[cfg(feature = "dbus")]
        let block = {
            let block = self.a11y_process_key(
                Duration::from_micros(time.micros()),
                event.key_code(),
                event.state(),
            );
            if block != KbMonBlock::Pass {
                *consumed_by_a11y = true;
            }
            // The accessibility modifier first press must not change XKB state, so we return
            // early here.
            if block == KbMonBlock::ModifierFirstPress {
                return;
            }
            block
        };
        #[cfg(not(feature = "dbus"))]
        let _ = consumed_by_a11y;

        let Some(Some(bind)) = self.clarion.seat.get_keyboard().unwrap().input(
            self,
            event.key_code(),
            event.state(),
            serial,
            time,
            |this, mods, keysym| {
                let key_code = event.key_code();
                let modified = keysym.modified_sym();
                let raw = keysym.raw_latin_sym_or_raw_current_sym();
                let modifiers = modifiers_from_state(*mods);
                // XKB has now applied this event. Stop a chord when a modifier
                // changes, including when its release is consumed by another UI.
                this.update_key_repeat(key_code, pressed, Some(modifiers));

                // After updating XKB state from accessibility-grabbed keys, return right away and
                // don't handle them.
                #[cfg(feature = "dbus")]
                if block != KbMonBlock::Pass {
                    // HACK: there's a slight problem with this code. Here we filter out keys
                    // consumed by accessibility from getting sent to the Wayland client. However,
                    // the Wayland client can still receive these keys from the wl_keyboard
                    // enter/modifiers events. In particular, this can easily happen when opening
                    // the Orca actions menu with Orca + Shift + A: in most cases, when this menu
                    // opens, Shift is still held down, so the menu receives it in
                    // wl_keyboard.enter/modifiers. Then the menu won't react to Enter presses
                    // until the user taps Shift again to "release" it (since the initial Shift
                    // release will be intercepted here).
                    //
                    // I don't think there's any good way of dealing with this apart from keeping a
                    // separate xkb state for accessibility, so that we can track the pressed
                    // modifiers without accidentally leaking them to wl_keyboard.enter. So for now
                    // let's forward modifier releases to the clients here to deal with the most
                    // common case.
                    if !pressed
                        && matches!(
                            modified,
                            Keysym::Shift_L
                                | Keysym::Shift_R
                                | Keysym::Control_L
                                | Keysym::Control_R
                                | Keysym::Super_L
                                | Keysym::Super_R
                                | Keysym::Alt_L
                                | Keysym::Alt_R
                        )
                    {
                        return FilterResult::Forward;
                    } else {
                        return FilterResult::Intercept(None);
                    }
                }

                if this.clarion.exit_confirm_dialog.is_open() && pressed {
                    if raw == Some(Keysym::Return) {
                        info!("quitting after confirming exit dialog");
                        this.clarion.stop_signal.stop();
                    }

                    // Don't send this press to any clients.
                    this.clarion.suppressed_keys.insert(key_code);
                    return FilterResult::Intercept(None);
                }

                // Check if all modifiers were released while the MRU UI was open. If so, close the
                // UI (which will also transfer the focus to the current MRU UI selection).
                if this.clarion.window_mru_ui.is_open() && !pressed && modifiers.is_empty() {
                    this.do_action(Action::MruConfirm, false);

                    if this.clarion.suppressed_keys.remove(&key_code) {
                        return FilterResult::Intercept(None);
                    } else {
                        return FilterResult::Forward;
                    }
                }

                if pressed && raw == Some(Keysym::Escape) {
                    // Cancel certain grabs on Escape.
                    let pointer = this.clarion.seat.get_pointer().unwrap();
                    if pointer
                        .with_grab(|_, grab| Self::grab_can_be_cancelled_with_esc(grab))
                        .unwrap_or(false)
                    {
                        pointer.unset_grab(this, serial, time);
                        this.clarion.suppressed_keys.insert(key_code);
                        return FilterResult::Intercept(None);
                    }
                }

                // Screenshot, lock, exit confirmation, accessibility and MRU keep priority.
                // Suppress both halves of navigation keys, including releases after closing.
                let allow_hotkey_overlay = !this.clarion.is_locked()
                    && !this.clarion.screenshot_ui.is_open()
                    && !this.clarion.window_mru_ui.is_open();
                if intercept_hotkey_overlay_key(
                    &mut this.clarion.hotkey_overlay,
                    &mut this.clarion.suppressed_keys,
                    key_code,
                    modified,
                    pressed,
                    modifiers,
                    allow_hotkey_overlay,
                ) {
                    *consumed_by_hotkey_overlay = true;
                    this.clarion.queue_redraw_all();
                    return FilterResult::Intercept(None);
                }

                if let Some(Keysym::space) = raw {
                    this.clarion.screenshot_ui.set_space_down(pressed);
                }

                let res = {
                    let desktop_shortcut = is_macos_desktop_shortcut(raw, *mods)
                        && this.desktop_owns_macos_shortcuts();
                    let config = this.clarion.config.borrow();
                    // Keep normal release suppression: changing focus/style or releasing
                    // Super first must not invent a release for an intercepted press.
                    let bindings =
                        make_binds_iter(&config, &mut this.clarion.window_mru_ui, modifiers)
                            .filter(|_| !desktop_shortcut);

                    should_intercept_key(
                        &mut this.clarion.suppressed_keys,
                        bindings,
                        mod_key,
                        key_code,
                        modified,
                        raw,
                        pressed,
                        *mods,
                        &this.clarion.screenshot_ui,
                        this.clarion
                            .config
                            .borrow()
                            .input
                            .disable_power_key_handling,
                        is_inhibiting_shortcuts,
                    )
                };

                if pressed
                    && this.clarion.overview_mode_transition_active()
                    && this.clarion.keyboard_focus.is_overview()
                    && modified != Keysym::Escape
                    && matches!(res, FilterResult::Forward)
                {
                    this.clarion.suppressed_keys.insert(key_code);
                    return FilterResult::Intercept(None);
                }

                if pressed
                    && !mods.ctrl
                    && !mods.alt
                    && !mods.logo
                    && !this.clarion.is_locked()
                    && !this.clarion.screenshot_ui.is_open()
                    && !this.clarion.window_mru_ui.is_open()
                    && this.clarion.keyboard_focus.is_overview()
                    && matches!(res, FilterResult::Forward)
                    && this.overview_command(OverviewCommand::StageKey(modified))
                {
                    this.clarion.suppressed_keys.insert(key_code);
                    this.clarion.queue_redraw_all();
                    return FilterResult::Intercept(None);
                }

                if pressed
                    && !mods.ctrl
                    && !mods.alt
                    && !mods.logo
                    && this.overview_command(OverviewCommand::SpreadKey(modified))
                {
                    this.clarion.suppressed_keys.insert(key_code);
                    this.clarion.queue_redraw_all();
                    return FilterResult::Intercept(None);
                }

                if matches!(res, FilterResult::Forward) {
                    // Preserve user bindings and shortcut inhibitors. Existing configurations
                    // gain the Apps key only when the native Shell is running.
                    if pressed && this.shell_launcher_available() {
                        if let Some(bind) = launcher_key_bind(raw, modifiers) {
                            this.clarion.suppressed_keys.insert(key_code);
                            return FilterResult::Intercept(Some(bind));
                        }
                    }
                    // If we didn't find any bind, try other hardcoded keys.
                    if this.clarion.keyboard_focus.is_overview() && pressed {
                        let is_expo = this.clarion.layout.overview_mode()
                            == crate::layout::OverviewMode::Expo;
                        if let Some(bind) =
                            raw.and_then(|raw| hardcoded_overview_bind(raw, *mods, is_expo))
                        {
                            this.clarion.suppressed_keys.insert(key_code);
                            return FilterResult::Intercept(Some(bind));
                        }
                    }

                    // Interaction with the active window, immediately update the active window's
                    // focus timestamp without waiting for a possible pending MRU lock-in delay.
                    this.clarion.mru_apply_keyboard_commit();
                }

                res
            },
        ) else {
            return;
        };

        if !pressed {
            return;
        }

        self.handle_bind(bind.clone());

        // Session-changing actions clear intercepted keys because their
        // releases may never arrive; do not start a fresh timer afterwards.
        if self.clarion.suppressed_keys.contains(&event.key_code()) {
            self.start_key_repeat(bind, event.key_code());
        }
    }

    fn update_key_repeat(
        &mut self,
        key_code: Keycode,
        pressed: bool,
        modifiers: Option<Modifiers>,
    ) {
        if self
            .clarion
            .bind_repeat_timer
            .as_ref()
            .is_some_and(|repeat| {
                (!pressed && repeat.key_code == key_code)
                    || modifiers.is_some_and(|modifiers| modifiers != repeat.modifiers)
            })
        {
            self.stop_key_repeat();
        }
    }

    fn stop_key_repeat(&mut self) {
        if let Some(repeat) = self.clarion.bind_repeat_timer.take() {
            self.clarion.event_loop.remove(repeat.token);
        }
    }

    fn start_key_repeat(&mut self, bind: Bind, key_code: Keycode) {
        if !bind.repeat {
            return;
        }

        // Stop the previous key repeat if any.
        self.stop_key_repeat();

        let keyboard = self.clarion.seat.get_keyboard().unwrap();
        let modifiers = modifiers_from_state(keyboard.modifier_state());
        let locked = self.clarion.is_locked();
        let lock_epoch = self.clarion.capture_scene_epoch.stamp();
        let config = self.clarion.config.borrow();
        let config = &config.input.keyboard;

        let repeat_rate = config.repeat_rate;
        if repeat_rate == 0 {
            return;
        }
        let repeat_duration = Duration::from_secs_f64(1. / f64::from(repeat_rate));

        let repeat_timer =
            Timer::from_duration(Duration::from_millis(u64::from(config.repeat_delay)));

        let token = self
            .clarion
            .event_loop
            .insert_source(repeat_timer, move |_, _, state| {
                let keyboard = state.clarion.seat.get_keyboard().unwrap();
                // Input focus loss or a session switch can clear the seat's
                // pressed keys without delivering the usual key release here.
                // A lock/unlock must never resume a pre-lock held shortcut.
                let current = state
                    .clarion
                    .bind_repeat_timer
                    .as_ref()
                    .is_some_and(|repeat| {
                        repeat.key_code == key_code && repeat.modifiers == modifiers
                    })
                    && keyboard.pressed_keys().contains(&key_code)
                    && modifiers_from_state(keyboard.modifier_state()) == modifiers
                    && lock_epoch.is_current()
                    && state.clarion.is_locked() == locked
                    && (!bind.allow_inhibiting || !state.is_inhibiting_shortcuts());
                if !current {
                    state.clarion.bind_repeat_timer = None;
                    return TimeoutAction::Drop;
                }
                state.handle_bind(bind.clone());
                TimeoutAction::ToDuration(repeat_duration)
            })
            .unwrap();

        self.clarion.bind_repeat_timer = Some(BindRepeat {
            token,
            key_code,
            modifiers,
        });
    }

    fn hide_cursor_if_needed(&mut self) {
        // If the pointer is already invisible, don't reset it back to Hidden causing one frame
        // of hover.
        if !self.clarion.pointer_visibility.is_visible() {
            return;
        }

        if !self.clarion.config.borrow().cursor.hide_when_typing {
            return;
        }

        // clarion keeps this set only while actively using a tablet, which means the cursor
        // position is likely to change almost immediately, causing pointer_visibility to
        // just flicker back and forth.
        if self.clarion.tablet_cursor_location.is_some() {
            return;
        }

        self.clarion.pointer_visibility = PointerVisibility::Hidden;
        self.clarion.queue_redraw_all();
    }

    pub fn handle_bind(&mut self, bind: Bind) {
        let Some(cooldown) = bind.cooldown else {
            self.do_action(bind.action, bind.allow_when_locked);
            return;
        };

        // Check this first so that it doesn't trigger the cooldown.
        if self.clarion.is_locked()
            && !(bind.allow_when_locked || allowed_when_locked(&bind.action))
        {
            return;
        }

        match self.clarion.bind_cooldown_timers.entry(bind.key) {
            // The bind is on cooldown.
            Entry::Occupied(_) => (),
            Entry::Vacant(entry) => {
                let timer = Timer::from_duration(cooldown);
                let token = self
                    .clarion
                    .event_loop
                    .insert_source(timer, move |_, _, state| {
                        if state
                            .clarion
                            .bind_cooldown_timers
                            .remove(&bind.key)
                            .is_none()
                        {
                            error!("bind cooldown timer entry disappeared");
                        }
                        TimeoutAction::Drop
                    })
                    .unwrap();
                entry.insert(token);

                self.do_action(bind.action, bind.allow_when_locked);
            }
        }
    }

    pub fn do_action(&mut self, action: Action, allow_when_locked: bool) {
        if self.clarion.is_locked() && !(allow_when_locked || allowed_when_locked(&action)) {
            return;
        }

        // IPC and bindings must observe all drag motion preceding the command.
        self.clarion.layout.flush_pending_move();

        // Mode-entry commands own capture-before-ungrab ordering. Other actions
        // retain their existing touch cancellation before changing layout.
        let mode_entry = matches!(
            action,
            Action::ToggleOverview
                | Action::OpenOverview
                | Action::ToggleWindowSpread
                | Action::ToggleExpo
        );
        if let Some(touch) = (!mode_entry)
            .then(|| self.clarion.seat.get_touch())
            .flatten()
        {
            if touch.is_grabbed() {
                touch.cancel(self);
            }
        }

        // Focus/navigation and visual toggles do not consume the layout history.
        if matches!(
            &action,
            Action::MoveColumnLeft
                | Action::MoveColumnRight
                | Action::MoveColumnToFirst
                | Action::MoveColumnToLast
                | Action::MoveColumnLeftOrToMonitorLeft
                | Action::MoveColumnRightOrToMonitorRight
                | Action::MoveColumnToIndex(..)
                | Action::MoveWindowDown
                | Action::MoveWindowUp
                | Action::MoveWindowDownOrToWorkspaceDown
                | Action::MoveWindowUpOrToWorkspaceUp
                | Action::ConsumeOrExpelWindowLeft
                | Action::ConsumeOrExpelWindowLeftById(..)
                | Action::ConsumeOrExpelWindowRight
                | Action::ConsumeOrExpelWindowRightById(..)
                | Action::ConsumeWindowIntoColumn
                | Action::ExpelWindowFromColumn
                | Action::SwapWindowLeft
                | Action::SwapWindowRight
                | Action::ToggleColumnTabbedDisplay
                | Action::ToggleColumnDwindleDisplay
                | Action::SetColumnDisplay(..)
                | Action::MoveWindowToWorkspaceDown(..)
                | Action::MoveWindowToWorkspaceUp(..)
                | Action::MoveWindowToWorkspace(..)
                | Action::MoveWindowToWorkspaceById { .. }
                | Action::MoveColumnToWorkspaceDown(..)
                | Action::MoveColumnToWorkspaceUp(..)
                | Action::MoveColumnToWorkspace(..)
                | Action::MoveWorkspaceDown
                | Action::MoveWorkspaceUp
                | Action::MoveWorkspaceToIndex(..)
                | Action::MoveWorkspaceToIndexByRef { .. }
                | Action::MoveWorkspaceToMonitorByRef { .. }
                | Action::MoveWorkspaceToMonitor(..)
                | Action::SetWorkspaceName(..)
                | Action::SetWorkspaceNameByRef { .. }
                | Action::UnsetWorkspaceName
                | Action::UnsetWorkSpaceNameByRef(..)
                | Action::MoveWindowToMonitorLeft
                | Action::MoveWindowToMonitorRight
                | Action::MoveWindowToMonitorDown
                | Action::MoveWindowToMonitorUp
                | Action::MoveWindowToMonitorPrevious
                | Action::MoveWindowToMonitorNext
                | Action::MoveWindowToMonitor(..)
                | Action::MoveWindowToMonitorById { .. }
                | Action::MoveColumnToMonitorLeft
                | Action::MoveColumnToMonitorRight
                | Action::MoveColumnToMonitorDown
                | Action::MoveColumnToMonitorUp
                | Action::MoveColumnToMonitorPrevious
                | Action::MoveColumnToMonitorNext
                | Action::MoveColumnToMonitor(..)
                | Action::SetWindowWidth(..)
                | Action::SetWindowWidthById { .. }
                | Action::SetWindowHeight(..)
                | Action::SetWindowHeightById { .. }
                | Action::ResetWindowHeight
                | Action::ResetWindowHeightById(..)
                | Action::SwitchPresetColumnWidth
                | Action::SwitchPresetColumnWidthBack
                | Action::SwitchPresetWindowWidth
                | Action::SwitchPresetWindowWidthBack
                | Action::SwitchPresetWindowWidthById(..)
                | Action::SwitchPresetWindowWidthBackById(..)
                | Action::SwitchPresetWindowHeight
                | Action::SwitchPresetWindowHeightBack
                | Action::SwitchPresetWindowHeightById(..)
                | Action::SwitchPresetWindowHeightBackById(..)
                | Action::SetColumnWidth(..)
                | Action::MoveWorkspaceToMonitorLeft
                | Action::MoveWorkspaceToMonitorRight
                | Action::MoveWorkspaceToMonitorDown
                | Action::MoveWorkspaceToMonitorUp
                | Action::MoveWorkspaceToMonitorPrevious
                | Action::MoveWorkspaceToMonitorNext
                | Action::ToggleWindowFloating
                | Action::ToggleWindowFloatingById(..)
                | Action::MoveWindowToFloating
                | Action::MoveWindowToFloatingById(..)
                | Action::MoveWindowToTiling
                | Action::MoveWindowToTilingById(..)
                | Action::MoveFloatingWindowById { .. }
                | Action::MoveWindowToScratchpad
                | Action::SwitchTilingMode(..)
                | Action::ToggleDwindleSplit
        ) {
            self.clarion.layout.checkpoint_layout();
        }

        // Explicit mutations of a parked window first rejoin its original
        // workspace. Closing and screenshots keep it parked; FocusWindow restores
        // through the same activation path used by xdg-activation and MRU.
        let mutate_id = match &action {
            Action::FullscreenWindowById(id)
            | Action::ToggleWindowedFullscreenById(id)
            | Action::ConsumeOrExpelWindowLeftById(id)
            | Action::ConsumeOrExpelWindowRightById(id)
            | Action::CenterWindowById(id)
            | Action::ResetWindowHeightById(id)
            | Action::SwitchPresetWindowWidthById(id)
            | Action::SwitchPresetWindowWidthBackById(id)
            | Action::SwitchPresetWindowHeightById(id)
            | Action::SwitchPresetWindowHeightBackById(id)
            | Action::MaximizeWindowToEdgesById(id)
            | Action::ToggleWindowFloatingById(id)
            | Action::MoveWindowToFloatingById(id)
            | Action::MoveWindowToTilingById(id) => Some(*id),
            Action::MoveWindowToWorkspaceById { window_id: id, .. }
            | Action::MoveWindowToMonitorById { id, .. }
            | Action::SetWindowWidthById { id, .. }
            | Action::SetWindowHeightById { id, .. } => Some(*id),
            Action::MoveFloatingWindowById { id, .. } => *id,
            _ => None,
        };
        if let Some(id) = mutate_id {
            let window = self
                .clarion
                .layout
                .windows()
                .find(|(_, mapped)| mapped.id().get() == id)
                .map(|(_, mapped)| mapped.window.clone());
            if let Some(window) = window {
                self.clarion.layout.restore_window(&window);
            }
        }

        match action {
            Action::Quit(skip_confirmation) => {
                if !skip_confirmation && self.clarion.exit_confirm_dialog.show() {
                    self.clarion.queue_redraw_all();
                    return;
                }

                info!("quitting as requested");
                self.clarion.stop_signal.stop()
            }
            Action::ChangeVt(vt) => {
                self.stop_key_repeat();
                self.backend.change_vt(vt);
                // Changing VT may not deliver the key releases, so clear the state.
                self.clarion.suppressed_keys.clear();
            }
            Action::Suspend => {
                self.stop_key_repeat();
                self.backend.suspend();
                // Suspend may not deliver the key releases, so clear the state.
                self.clarion.suppressed_keys.clear();
            }
            Action::PowerOffMonitors => {
                self.clarion.deactivate_monitors(&mut self.backend);
            }
            Action::PowerOnMonitors => {
                self.clarion.activate_monitors(&mut self.backend);
            }
            Action::SetReducedMotion {
                enabled,
                platform,
                initialize_only,
            } => {
                let enabled = if initialize_only {
                    self.clarion
                        .ipc_server
                        .as_ref()
                        .and_then(|server| server.reduced_motion_choice())
                        .unwrap_or(enabled)
                } else {
                    enabled
                };
                self.clarion.layout.set_reduced_motion(enabled || platform);
                self.ipc_reduced_motion_changed(enabled);
                self.clarion.queue_redraw_all();
            }
            Action::ToggleDebugTint => {
                self.backend.toggle_debug_tint();
                self.clarion.queue_redraw_all();
            }
            Action::DebugToggleOpaqueRegions => {
                self.clarion.debug_draw_opaque_regions = !self.clarion.debug_draw_opaque_regions;
                self.clarion.queue_redraw_all();
            }
            Action::DebugToggleDamage => {
                self.clarion.debug_toggle_damage();
            }
            Action::Spawn(command) => {
                let (token, _) = self.clarion.activation_state.create_external_token(None);
                spawn(command, Some(token.clone()));
            }
            Action::SpawnSh(command) => {
                let (token, _) = self.clarion.activation_state.create_external_token(None);
                spawn_sh(command, Some(token.clone()));
            }
            Action::DoScreenTransition(delay_ms) => {
                self.backend.with_renderer(|renderer| match renderer {
                    crate::render_helpers::renderer::AnyRenderer::Gles(renderer) => {
                        self.clarion.do_screen_transition(renderer, delay_ms)
                    }
                    crate::render_helpers::renderer::AnyRenderer::Native(renderer) => {
                        self.clarion.do_screen_transition(renderer, delay_ms)
                    }
                });
            }
            Action::ScreenshotScreen(write_to_disk, show_pointer, path) => {
                let active = self.clarion.layout.active_output().cloned();
                if let Some(active) = active {
                    self.backend
                        .with_renderer_for_output(Some(&active), |renderer| {
                            let res = match renderer {
                                crate::render_helpers::renderer::AnyRenderer::Gles(gles) => self
                                    .clarion
                                    .screenshot(gles, &active, write_to_disk, show_pointer, path),
                                crate::render_helpers::renderer::AnyRenderer::Native(native) => {
                                    self.clarion.screenshot(
                                        native,
                                        &active,
                                        write_to_disk,
                                        show_pointer,
                                        path,
                                    )
                                }
                            };
                            if let Err(err) = res {
                                warn!("error taking screenshot: {err:?}");
                            }
                        });
                }
            }
            Action::ConfirmScreenshot { write_to_disk } => {
                self.confirm_screenshot(write_to_disk);
            }
            Action::CancelScreenshot => {
                if !self.clarion.screenshot_ui.is_open() {
                    return;
                }

                self.clarion.screenshot_ui.close();
                self.clarion
                    .cursor_manager
                    .set_cursor_image(CursorImageStatus::default_named());
                self.clarion.queue_redraw_all();
            }
            Action::ScreenshotTogglePointer => {
                self.clarion.screenshot_ui.toggle_pointer();
                self.clarion.queue_redraw_all();
            }
            Action::Screenshot(show_cursor, path) => {
                self.open_screenshot_ui(show_cursor, path);
                self.clarion.cancel_mru();
            }
            Action::ScreenshotWindow(write_to_disk, show_pointer, path) => {
                let focus = self.clarion.layout.focus_with_output();
                if let Some((mapped, output)) = focus {
                    self.backend
                        .with_renderer_for_output(Some(output), |renderer| {
                            let res = match renderer {
                                crate::render_helpers::renderer::AnyRenderer::Gles(gles) => {
                                    self.clarion.screenshot_window(
                                        gles,
                                        output,
                                        mapped,
                                        write_to_disk,
                                        show_pointer,
                                        path,
                                    )
                                }
                                crate::render_helpers::renderer::AnyRenderer::Native(native) => {
                                    self.clarion.screenshot_window(
                                        native,
                                        output,
                                        mapped,
                                        write_to_disk,
                                        show_pointer,
                                        path,
                                    )
                                }
                            };
                            if let Err(err) = res {
                                warn!("error taking screenshot: {err:?}");
                            }
                        });
                }
            }
            Action::ScreenshotWindowById {
                id,
                write_to_disk,
                show_pointer,
                path,
                is_preview,
            } => {
                let mut windows = self.clarion.layout.windows();
                let window = windows.find(|(_, m)| m.id().get() == id);
                if let Some((Some(monitor), mapped)) = window {
                    let output = monitor.output();
                    self.backend
                        .with_renderer_for_output(Some(output), |renderer| {
                            let res = match renderer {
                                crate::render_helpers::renderer::AnyRenderer::Gles(gles) => {
                                    self.clarion.screenshot_window_with_preview(
                                        gles,
                                        output,
                                        mapped,
                                        write_to_disk,
                                        show_pointer,
                                        path,
                                        is_preview,
                                    )
                                }
                                crate::render_helpers::renderer::AnyRenderer::Native(native) => {
                                    self.clarion.screenshot_window_with_preview(
                                        native,
                                        output,
                                        mapped,
                                        write_to_disk,
                                        show_pointer,
                                        path,
                                        is_preview,
                                    )
                                }
                            };
                            if let Err(err) = res {
                                warn!("error taking screenshot: {err:?}");
                            }
                        });
                }
            }
            Action::ToggleKeyboardShortcutsInhibit => {
                if let Some(inhibitor) = self.clarion.keyboard_focus.surface().and_then(|surface| {
                    self.clarion
                        .keyboard_shortcuts_inhibiting_surfaces
                        .get(surface)
                }) {
                    if inhibitor.is_active() {
                        inhibitor.inactivate();
                    } else {
                        inhibitor.activate();
                    }
                }
            }
            Action::CloseWindow => {
                if let Some(mapped) = self.clarion.layout.focus() {
                    mapped.toplevel().send_close();
                }
            }
            Action::CloseWindowById(id) => {
                let window = self
                    .clarion
                    .layout
                    .windows()
                    .find(|(_, m)| m.id().get() == id);
                if let Some((_, mapped)) = window {
                    mapped.toplevel().send_close();
                }
            }
            Action::FullscreenWindow => {
                let focus = self.clarion.layout.focus().map(|m| m.window.clone());
                if let Some(window) = focus {
                    self.clarion.layout.toggle_fullscreen(&window);
                    // FIXME: granular
                    self.clarion.queue_redraw_all();
                }
            }
            Action::FullscreenWindowById(id) => {
                let window = self
                    .clarion
                    .layout
                    .windows()
                    .find(|(_, m)| m.id().get() == id);
                let window = window.map(|(_, m)| m.window.clone());
                if let Some(window) = window {
                    self.clarion.layout.toggle_fullscreen(&window);
                    // FIXME: granular
                    self.clarion.queue_redraw_all();
                }
            }
            Action::ToggleWindowedFullscreen => {
                let focus = self.clarion.layout.focus().map(|m| m.window.clone());
                if let Some(window) = focus {
                    self.clarion.layout.toggle_windowed_fullscreen(&window);
                    // FIXME: granular
                    self.clarion.queue_redraw_all();
                }
            }
            Action::ToggleWindowedFullscreenById(id) => {
                let window = self
                    .clarion
                    .layout
                    .windows()
                    .find(|(_, m)| m.id().get() == id);
                let window = window.map(|(_, m)| m.window.clone());
                if let Some(window) = window {
                    self.clarion.layout.toggle_windowed_fullscreen(&window);
                    // FIXME: granular
                    self.clarion.queue_redraw_all();
                }
            }
            Action::MinimizeWindow => {
                let id = self.clarion.layout.focus().map(|mapped| mapped.id().get());
                if let Some(id) = id {
                    self.minimize_window_by_id(id);
                }
            }
            Action::MinimizeWindowById(id) => self.minimize_window_by_id(id),
            Action::SetMinimizeTarget {
                id,
                target,
                reduce_motion,
            } => {
                let window = self
                    .clarion
                    .layout
                    .windows()
                    .find(|(_, mapped)| mapped.id().get() == id)
                    .map(|(_, mapped)| mapped.window.clone());
                if let Some(window) = window {
                    if let Some(output) =
                        self.clarion
                            .layout
                            .set_minimize_target(&window, target, reduce_motion)
                    {
                        self.clarion.queue_redraw(&output);
                    }
                }
            }
            Action::FocusWindow(id) => {
                if !matches!(
                    self.clarion.lock_state,
                    crate::compositor::LockState::Unlocked
                ) {
                    return;
                }
                let window = self
                    .clarion
                    .layout
                    .windows()
                    .find(|(_, m)| m.id().get() == id);
                let window = window.map(|(_, m)| m.window.clone());
                if let Some(window) = window {
                    self.overview_command(OverviewCommand::Close);
                    self.focus_window(&window);
                    // Dock/IPC activation explicitly returns focus to an app.
                    // Invalid IDs leave the current on-demand layer untouched.
                    self.clarion.layer_shell_on_demand_focus = None;
                }
            }
            Action::FocusWindowInColumn(index) => {
                self.clarion.layout.focus_window_in_column(index);
                self.maybe_warp_cursor_to_focus();
                self.clarion.layer_shell_on_demand_focus = None;
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::FocusWindowPrevious => {
                let current = self.clarion.layout.focus().map(|win| win.id());
                if let Some(window) = self
                    .clarion
                    .layout
                    .windows()
                    .map(|(_, win)| win)
                    .filter(|win| Some(win.id()) != current)
                    .max_by_key(|win| win.get_focus_timestamp())
                    .map(|win| win.window.clone())
                {
                    // Commit current focus so repeated focus-window-previous works as expected.
                    self.clarion.mru_apply_keyboard_commit();

                    self.focus_window(&window);
                }
            }
            Action::SwitchLayout(action) => {
                let keyboard = &self.clarion.seat.get_keyboard().unwrap();
                keyboard.with_xkb_state(self, |mut state| match action {
                    LayoutSwitchTarget::Next => state.cycle_next_layout(),
                    LayoutSwitchTarget::Prev => state.cycle_prev_layout(),
                    LayoutSwitchTarget::Index(layout) => {
                        let num_layouts = state.xkb().lock().unwrap().layouts().count();
                        if usize::from(layout) >= num_layouts {
                            warn!("requested layout doesn't exist")
                        } else {
                            state.set_layout(Layout(layout.into()))
                        }
                    }
                });
            }
            Action::MoveColumnLeft => {
                if self.clarion.screenshot_ui.is_open() {
                    self.clarion.screenshot_ui.move_left();
                } else {
                    self.clarion.layout.move_left();
                    self.maybe_warp_cursor_to_focus();
                }

                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::MoveColumnRight => {
                if self.clarion.screenshot_ui.is_open() {
                    self.clarion.screenshot_ui.move_right();
                } else {
                    self.clarion.layout.move_right();
                    self.maybe_warp_cursor_to_focus();
                }

                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::MoveColumnToFirst => {
                self.clarion.layout.move_column_to_first();
                self.maybe_warp_cursor_to_focus();
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::MoveColumnToLast => {
                self.clarion.layout.move_column_to_last();
                self.maybe_warp_cursor_to_focus();
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::MoveColumnLeftOrToMonitorLeft => {
                if self.clarion.screenshot_ui.is_open() {
                    self.clarion.screenshot_ui.move_left();
                } else if let Some(output) = self.clarion.output_left() {
                    if self.clarion.layout.move_column_left_or_to_output(&output)
                        && !self.maybe_warp_cursor_to_focus_centered()
                    {
                        self.move_cursor_to_output(&output);
                    } else {
                        self.maybe_warp_cursor_to_focus();
                    }
                } else {
                    self.clarion.layout.move_left();
                    self.maybe_warp_cursor_to_focus();
                }

                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::MoveColumnRightOrToMonitorRight => {
                if self.clarion.screenshot_ui.is_open() {
                    self.clarion.screenshot_ui.move_right();
                } else if let Some(output) = self.clarion.output_right() {
                    if self.clarion.layout.move_column_right_or_to_output(&output)
                        && !self.maybe_warp_cursor_to_focus_centered()
                    {
                        self.move_cursor_to_output(&output);
                    } else {
                        self.maybe_warp_cursor_to_focus();
                    }
                } else {
                    self.clarion.layout.move_right();
                    self.maybe_warp_cursor_to_focus();
                }

                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::MoveWindowDown => {
                if self.clarion.screenshot_ui.is_open() {
                    self.clarion.screenshot_ui.move_down();
                } else {
                    self.clarion.layout.move_down();
                    self.maybe_warp_cursor_to_focus();
                }

                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::MoveWindowUp => {
                if self.clarion.screenshot_ui.is_open() {
                    self.clarion.screenshot_ui.move_up();
                } else {
                    self.clarion.layout.move_up();
                    self.maybe_warp_cursor_to_focus();
                }

                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::MoveWindowDownOrToWorkspaceDown => {
                if self.clarion.screenshot_ui.is_open() {
                    self.clarion.screenshot_ui.move_down();
                } else {
                    self.clarion.layout.move_down_or_to_workspace_down();
                    self.maybe_warp_cursor_to_focus();
                }
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::MoveWindowUpOrToWorkspaceUp => {
                if self.clarion.screenshot_ui.is_open() {
                    self.clarion.screenshot_ui.move_up();
                } else {
                    self.clarion.layout.move_up_or_to_workspace_up();
                    self.maybe_warp_cursor_to_focus();
                }
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::ConsumeOrExpelWindowLeft => {
                self.clarion.layout.consume_or_expel_window_left(None);
                self.maybe_warp_cursor_to_focus();
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::ConsumeOrExpelWindowLeftById(id) => {
                let window = self
                    .clarion
                    .layout
                    .windows()
                    .find(|(_, m)| m.id().get() == id);
                let window = window.map(|(_, m)| m.window.clone());
                if let Some(window) = window {
                    self.clarion
                        .layout
                        .consume_or_expel_window_left(Some(&window));
                    self.maybe_warp_cursor_to_focus();
                    // FIXME: granular
                    self.clarion.queue_redraw_all();
                }
            }
            Action::ConsumeOrExpelWindowRight => {
                self.clarion.layout.consume_or_expel_window_right(None);
                self.maybe_warp_cursor_to_focus();
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::ConsumeOrExpelWindowRightById(id) => {
                let window = self
                    .clarion
                    .layout
                    .windows()
                    .find(|(_, m)| m.id().get() == id);
                let window = window.map(|(_, m)| m.window.clone());
                if let Some(window) = window {
                    self.clarion
                        .layout
                        .consume_or_expel_window_right(Some(&window));
                    self.maybe_warp_cursor_to_focus();
                    // FIXME: granular
                    self.clarion.queue_redraw_all();
                }
            }
            Action::FocusColumnLeft => {
                self.clarion.layout.focus_left();
                self.maybe_warp_cursor_to_focus();
                self.clarion.layer_shell_on_demand_focus = None;
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::FocusColumnLeftUnderMouse => {
                if let Some((output, ws)) = self.clarion.workspace_under_cursor(true) {
                    let ws_id = ws.id();
                    let ws = {
                        let mut workspaces = self.clarion.layout.workspaces_mut();
                        workspaces.find(|ws| ws.id() == ws_id).unwrap()
                    };
                    ws.focus_left();
                    self.maybe_warp_cursor_to_focus();
                    self.clarion.layer_shell_on_demand_focus = None;
                    self.clarion.queue_redraw(&output);
                }
            }
            Action::FocusColumnRight => {
                self.clarion.layout.focus_right();
                self.maybe_warp_cursor_to_focus();
                self.clarion.layer_shell_on_demand_focus = None;
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::FocusColumnRightUnderMouse => {
                if let Some((output, ws)) = self.clarion.workspace_under_cursor(true) {
                    let ws_id = ws.id();
                    let ws = {
                        let mut workspaces = self.clarion.layout.workspaces_mut();
                        workspaces.find(|ws| ws.id() == ws_id).unwrap()
                    };
                    ws.focus_right();
                    self.maybe_warp_cursor_to_focus();
                    self.clarion.layer_shell_on_demand_focus = None;
                    self.clarion.queue_redraw(&output);
                }
            }
            Action::FocusColumnFirst => {
                self.clarion.layout.focus_column_first();
                self.maybe_warp_cursor_to_focus();
                self.clarion.layer_shell_on_demand_focus = None;
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::FocusColumnLast => {
                self.clarion.layout.focus_column_last();
                self.maybe_warp_cursor_to_focus();
                self.clarion.layer_shell_on_demand_focus = None;
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::FocusColumnRightOrFirst => {
                self.clarion.layout.focus_column_right_or_first();
                self.maybe_warp_cursor_to_focus();
                self.clarion.layer_shell_on_demand_focus = None;
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::FocusColumnLeftOrLast => {
                self.clarion.layout.focus_column_left_or_last();
                self.maybe_warp_cursor_to_focus();
                self.clarion.layer_shell_on_demand_focus = None;
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::FocusColumn(index) => {
                self.clarion.layout.focus_column(index);
                self.maybe_warp_cursor_to_focus();
                self.clarion.layer_shell_on_demand_focus = None;
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::FocusWindowOrMonitorUp => {
                if let Some(output) = self.clarion.output_up() {
                    if self.clarion.layout.focus_window_up_or_output(&output)
                        && !self.maybe_warp_cursor_to_focus_centered()
                    {
                        self.move_cursor_to_output(&output);
                    } else {
                        self.maybe_warp_cursor_to_focus();
                    }
                } else {
                    self.clarion.layout.focus_up();
                    self.maybe_warp_cursor_to_focus();
                }
                self.clarion.layer_shell_on_demand_focus = None;

                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::FocusWindowOrMonitorDown => {
                if let Some(output) = self.clarion.output_down() {
                    if self.clarion.layout.focus_window_down_or_output(&output)
                        && !self.maybe_warp_cursor_to_focus_centered()
                    {
                        self.move_cursor_to_output(&output);
                    } else {
                        self.maybe_warp_cursor_to_focus();
                    }
                } else {
                    self.clarion.layout.focus_down();
                    self.maybe_warp_cursor_to_focus();
                }
                self.clarion.layer_shell_on_demand_focus = None;

                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::FocusColumnOrMonitorLeft => {
                if let Some(output) = self.clarion.output_left() {
                    if self.clarion.layout.focus_column_left_or_output(&output)
                        && !self.maybe_warp_cursor_to_focus_centered()
                    {
                        self.move_cursor_to_output(&output);
                    } else {
                        self.maybe_warp_cursor_to_focus();
                    }
                } else {
                    self.clarion.layout.focus_left();
                    self.maybe_warp_cursor_to_focus();
                }
                self.clarion.layer_shell_on_demand_focus = None;

                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::FocusColumnOrMonitorRight => {
                if let Some(output) = self.clarion.output_right() {
                    if self.clarion.layout.focus_column_right_or_output(&output)
                        && !self.maybe_warp_cursor_to_focus_centered()
                    {
                        self.move_cursor_to_output(&output);
                    } else {
                        self.maybe_warp_cursor_to_focus();
                    }
                } else {
                    self.clarion.layout.focus_right();
                    self.maybe_warp_cursor_to_focus();
                }
                self.clarion.layer_shell_on_demand_focus = None;

                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::FocusWindowDown => {
                self.clarion.layout.focus_down();
                self.maybe_warp_cursor_to_focus();
                self.clarion.layer_shell_on_demand_focus = None;
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::FocusWindowUp => {
                self.clarion.layout.focus_up();
                self.maybe_warp_cursor_to_focus();
                self.clarion.layer_shell_on_demand_focus = None;
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::FocusWindowDownOrColumnLeft => {
                self.clarion.layout.focus_down_or_left();
                self.maybe_warp_cursor_to_focus();
                self.clarion.layer_shell_on_demand_focus = None;
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::FocusWindowDownOrColumnRight => {
                self.clarion.layout.focus_down_or_right();
                self.maybe_warp_cursor_to_focus();
                self.clarion.layer_shell_on_demand_focus = None;
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::FocusWindowUpOrColumnLeft => {
                self.clarion.layout.focus_up_or_left();
                self.maybe_warp_cursor_to_focus();
                self.clarion.layer_shell_on_demand_focus = None;
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::FocusWindowUpOrColumnRight => {
                self.clarion.layout.focus_up_or_right();
                self.maybe_warp_cursor_to_focus();
                self.clarion.layer_shell_on_demand_focus = None;
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::FocusWindowOrWorkspaceDown => {
                self.clarion.layout.focus_window_or_workspace_down();
                self.maybe_warp_cursor_to_focus();
                self.clarion.layer_shell_on_demand_focus = None;
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::FocusWindowOrWorkspaceUp => {
                self.clarion.layout.focus_window_or_workspace_up();
                self.maybe_warp_cursor_to_focus();
                self.clarion.layer_shell_on_demand_focus = None;
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::FocusWindowTop => {
                self.clarion.layout.focus_window_top();
                self.maybe_warp_cursor_to_focus();
                self.clarion.layer_shell_on_demand_focus = None;
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::FocusWindowBottom => {
                self.clarion.layout.focus_window_bottom();
                self.maybe_warp_cursor_to_focus();
                self.clarion.layer_shell_on_demand_focus = None;
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::FocusWindowDownOrTop => {
                self.clarion.layout.focus_window_down_or_top();
                self.maybe_warp_cursor_to_focus();
                self.clarion.layer_shell_on_demand_focus = None;
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::FocusWindowUpOrBottom => {
                self.clarion.layout.focus_window_up_or_bottom();
                self.maybe_warp_cursor_to_focus();
                self.clarion.layer_shell_on_demand_focus = None;
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::MoveWindowToWorkspaceDown(focus) => {
                self.clarion.layout.move_to_workspace_down(focus);
                self.maybe_warp_cursor_to_focus();
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::MoveWindowToWorkspaceUp(focus) => {
                self.clarion.layout.move_to_workspace_up(focus);
                self.maybe_warp_cursor_to_focus();
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::MoveWindowToWorkspace(reference, focus) => {
                if let Some((mut output, index)) =
                    self.clarion.find_output_and_workspace_index(reference)
                {
                    // The source output is always the active output, so if the target output is
                    // also the active output, we don't need to use move_to_output().
                    if let Some(active) = self.clarion.layout.active_output() {
                        if output.as_ref() == Some(active) {
                            output = None;
                        }
                    }

                    let activate = if focus {
                        ActivateWindow::Smart
                    } else {
                        ActivateWindow::No
                    };

                    if let Some(output) = output {
                        self.clarion
                            .layout
                            .move_to_output(None, &output, Some(index), activate);

                        if focus {
                            if !self.maybe_warp_cursor_to_focus_centered() {
                                self.move_cursor_to_output(&output);
                            }
                        } else {
                            self.maybe_warp_cursor_to_focus();
                        }
                    } else {
                        self.clarion.layout.move_to_workspace(None, index, activate);
                        self.maybe_warp_cursor_to_focus();
                    }

                    // FIXME: granular
                    self.clarion.queue_redraw_all();
                }
            }
            Action::MoveWindowToWorkspaceById {
                window_id: id,
                reference,
                focus,
            } => {
                let window = self
                    .clarion
                    .layout
                    .windows()
                    .find(|(_, m)| m.id().get() == id);
                let window = window.map(|(_, m)| m.window.clone());
                if let Some(window) = window {
                    if let Some((output, index)) =
                        self.clarion.find_output_and_workspace_index(reference)
                    {
                        let target_was_active = self
                            .clarion
                            .layout
                            .active_output()
                            .is_some_and(|active| output.as_ref() == Some(active));

                        let activate = if focus {
                            ActivateWindow::Smart
                        } else {
                            ActivateWindow::No
                        };

                        if let Some(output) = output {
                            self.clarion.layout.move_to_output(
                                Some(&window),
                                &output,
                                Some(index),
                                activate,
                            );

                            // If the active output changed (window was moved and focused).
                            #[allow(clippy::collapsible_if)]
                            if !target_was_active
                                && self.clarion.layout.active_output() == Some(&output)
                            {
                                if !self.maybe_warp_cursor_to_focus_centered() {
                                    self.move_cursor_to_output(&output);
                                }
                            }
                        } else {
                            self.clarion
                                .layout
                                .move_to_workspace(Some(&window), index, activate);

                            // If we focused the target window.
                            let new_focus = self.clarion.layout.focus();
                            if new_focus.is_some_and(|win| win.window == window) {
                                self.maybe_warp_cursor_to_focus();
                            }
                        }

                        // FIXME: granular
                        self.clarion.queue_redraw_all();
                    }
                }
            }
            Action::MoveColumnToWorkspaceDown(focus) => {
                self.clarion.layout.move_column_to_workspace_down(focus);
                self.maybe_warp_cursor_to_focus();
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::MoveColumnToWorkspaceUp(focus) => {
                self.clarion.layout.move_column_to_workspace_up(focus);
                self.maybe_warp_cursor_to_focus();
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::MoveColumnToWorkspace(reference, focus) => {
                if let Some((mut output, index)) =
                    self.clarion.find_output_and_workspace_index(reference)
                {
                    if let Some(active) = self.clarion.layout.active_output() {
                        if output.as_ref() == Some(active) {
                            output = None;
                        }
                    }

                    if let Some(output) = output {
                        self.clarion
                            .layout
                            .move_column_to_output(&output, Some(index), focus);
                        if focus && !self.maybe_warp_cursor_to_focus_centered() {
                            self.move_cursor_to_output(&output);
                        }
                    } else {
                        self.clarion.layout.move_column_to_workspace(index, focus);
                        if focus {
                            self.maybe_warp_cursor_to_focus();
                        }
                    }

                    // FIXME: granular
                    self.clarion.queue_redraw_all();
                }
            }
            Action::MoveColumnToIndex(idx) => {
                self.clarion.layout.move_column_to_index(idx);
                self.maybe_warp_cursor_to_focus();
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::FocusWorkspaceDown => {
                self.clarion.layout.switch_workspace_down();
                self.maybe_warp_cursor_to_focus();
                self.clarion.layer_shell_on_demand_focus = None;
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::FocusWorkspaceDownUnderMouse => {
                if let Some(output) = self.clarion.output_under_cursor() {
                    if let Some(mon) = self.clarion.layout.monitor_for_output_mut(&output) {
                        mon.switch_workspace_down();
                        self.maybe_warp_cursor_to_focus();
                        self.clarion.layer_shell_on_demand_focus = None;
                        self.clarion.queue_redraw(&output);
                    }
                }
            }
            Action::FocusWorkspaceUp => {
                self.clarion.layout.switch_workspace_up();
                self.maybe_warp_cursor_to_focus();
                self.clarion.layer_shell_on_demand_focus = None;
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::FocusWorkspaceUpUnderMouse => {
                if let Some(output) = self.clarion.output_under_cursor() {
                    if let Some(mon) = self.clarion.layout.monitor_for_output_mut(&output) {
                        mon.switch_workspace_up();
                        self.maybe_warp_cursor_to_focus();
                        self.clarion.layer_shell_on_demand_focus = None;
                        self.clarion.queue_redraw(&output);
                    }
                }
            }
            Action::FocusWorkspace(reference) => {
                if let Some((mut output, index)) =
                    self.clarion.find_output_and_workspace_index(reference)
                {
                    if let Some(active) = self.clarion.layout.active_output() {
                        if output.as_ref() == Some(active) {
                            output = None;
                        }
                    }

                    if let Some(output) = output {
                        self.clarion.layout.focus_output(&output);
                        self.clarion.layout.switch_workspace(index);
                        if !self.maybe_warp_cursor_to_focus_centered() {
                            self.move_cursor_to_output(&output);
                        }
                    } else {
                        let config = &self.clarion.config;
                        if config.borrow().input.workspace_auto_back_and_forth {
                            self.clarion
                                .layout
                                .switch_workspace_auto_back_and_forth(index);
                        } else {
                            self.clarion.layout.switch_workspace(index);
                        }
                        self.maybe_warp_cursor_to_focus();
                    }
                    self.clarion.layer_shell_on_demand_focus = None;

                    // FIXME: granular
                    self.clarion.queue_redraw_all();
                }
            }
            Action::FocusWorkspacePrevious => {
                self.clarion.layout.switch_workspace_previous();
                self.maybe_warp_cursor_to_focus();
                self.clarion.layer_shell_on_demand_focus = None;
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::MoveWorkspaceDown => {
                self.clarion.layout.move_workspace_down();
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::MoveWorkspaceUp => {
                self.clarion.layout.move_workspace_up();
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::MoveWorkspaceToIndex(new_idx) => {
                let new_idx = new_idx.saturating_sub(1);
                self.clarion.layout.move_workspace_to_idx(None, new_idx);
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::MoveWorkspaceToIndexByRef { new_idx, reference } => {
                if let Some(res) = self.clarion.find_output_and_workspace_index(reference) {
                    let new_idx = new_idx.saturating_sub(1);
                    self.clarion
                        .layout
                        .move_workspace_to_idx(Some(res), new_idx);
                    // FIXME: granular
                    self.clarion.queue_redraw_all();
                }
            }
            Action::SetWorkspaceName(name) => {
                self.clarion.layout.set_workspace_name(name, None);
            }
            Action::SetWorkspaceNameByRef { name, reference } => {
                self.clarion
                    .layout
                    .set_workspace_name(name, Some(reference));
            }
            Action::UnsetWorkspaceName => {
                self.clarion.layout.unset_workspace_name(None);
            }
            Action::UnsetWorkSpaceNameByRef(reference) => {
                self.clarion.layout.unset_workspace_name(Some(reference));
            }
            Action::ConsumeWindowIntoColumn => {
                self.clarion.layout.consume_into_column();
                // This does not cause immediate focus or window size change, so warping mouse to
                // focus won't do anything here.
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::ExpelWindowFromColumn => {
                self.clarion.layout.expel_from_column();
                self.maybe_warp_cursor_to_focus();
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::SwapWindowRight => {
                self.clarion
                    .layout
                    .swap_window_in_direction(ScrollDirection::Right);
                self.maybe_warp_cursor_to_focus();
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::SwapWindowLeft => {
                self.clarion
                    .layout
                    .swap_window_in_direction(ScrollDirection::Left);
                self.maybe_warp_cursor_to_focus();
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::ToggleColumnTabbedDisplay => {
                self.clarion.layout.toggle_column_tabbed_display();
                self.maybe_warp_cursor_to_focus();
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::ToggleColumnDwindleDisplay => {
                self.clarion.layout.toggle_column_dwindle_display();
                self.maybe_warp_cursor_to_focus();
                self.clarion.queue_redraw_all();
            }
            Action::SetColumnDisplay(display) => {
                self.clarion.layout.set_column_display(display);
                self.maybe_warp_cursor_to_focus();
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::SwitchPresetColumnWidth => {
                self.clarion.layout.toggle_width(true);
            }
            Action::SwitchPresetColumnWidthBack => {
                self.clarion.layout.toggle_width(false);
            }
            Action::SwitchPresetWindowWidth => {
                self.clarion.layout.toggle_window_width(None, true);
            }
            Action::SwitchPresetWindowWidthBack => {
                self.clarion.layout.toggle_window_width(None, false);
            }
            Action::SwitchPresetWindowWidthById(id) => {
                let window = self
                    .clarion
                    .layout
                    .windows()
                    .find(|(_, m)| m.id().get() == id);
                let window = window.map(|(_, m)| m.window.clone());
                if let Some(window) = window {
                    self.clarion.layout.toggle_window_width(Some(&window), true);
                }
            }
            Action::SwitchPresetWindowWidthBackById(id) => {
                let window = self
                    .clarion
                    .layout
                    .windows()
                    .find(|(_, m)| m.id().get() == id);
                let window = window.map(|(_, m)| m.window.clone());
                if let Some(window) = window {
                    self.clarion
                        .layout
                        .toggle_window_width(Some(&window), false);
                }
            }
            Action::SwitchPresetWindowHeight => {
                self.clarion.layout.toggle_window_height(None, true);
            }
            Action::SwitchPresetWindowHeightBack => {
                self.clarion.layout.toggle_window_height(None, false);
            }
            Action::SwitchPresetWindowHeightById(id) => {
                let window = self
                    .clarion
                    .layout
                    .windows()
                    .find(|(_, m)| m.id().get() == id);
                let window = window.map(|(_, m)| m.window.clone());
                if let Some(window) = window {
                    self.clarion
                        .layout
                        .toggle_window_height(Some(&window), true);
                }
            }
            Action::SwitchPresetWindowHeightBackById(id) => {
                let window = self
                    .clarion
                    .layout
                    .windows()
                    .find(|(_, m)| m.id().get() == id);
                let window = window.map(|(_, m)| m.window.clone());
                if let Some(window) = window {
                    self.clarion
                        .layout
                        .toggle_window_height(Some(&window), false);
                }
            }
            Action::CenterColumn => {
                self.clarion.layout.center_column();
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::CenterWindow => {
                self.clarion.layout.center_window(None);
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::CenterWindowById(id) => {
                let window = self
                    .clarion
                    .layout
                    .windows()
                    .find(|(_, m)| m.id().get() == id);
                let window = window.map(|(_, m)| m.window.clone());
                if let Some(window) = window {
                    self.clarion.layout.center_window(Some(&window));
                    // FIXME: granular
                    self.clarion.queue_redraw_all();
                }
            }
            Action::CenterVisibleColumns => {
                self.clarion.layout.center_visible_columns();
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::MaximizeColumn => {
                self.clarion.layout.toggle_full_width();
            }
            Action::MaximizeWindowToEdges => {
                let focus = self.clarion.layout.focus().map(|m| m.window.clone());
                if let Some(window) = focus {
                    self.clarion.layout.toggle_maximized(&window);
                    // FIXME: granular
                    self.clarion.queue_redraw_all();
                }
            }
            Action::MaximizeWindowToEdgesById(id) => {
                let window = self
                    .clarion
                    .layout
                    .windows()
                    .find(|(_, m)| m.id().get() == id);
                let window = window.map(|(_, m)| m.window.clone());
                if let Some(window) = window {
                    self.clarion.layout.toggle_maximized(&window);
                    // FIXME: granular
                    self.clarion.queue_redraw_all();
                }
            }
            Action::FocusMonitorLeft => {
                if let Some(output) = self.clarion.output_left() {
                    self.clarion.layout.focus_output(&output);
                    if !self.maybe_warp_cursor_to_focus_centered() {
                        self.move_cursor_to_output(&output);
                    }
                    self.clarion.layer_shell_on_demand_focus = None;
                }
            }
            Action::FocusMonitorRight => {
                if let Some(output) = self.clarion.output_right() {
                    self.clarion.layout.focus_output(&output);
                    if !self.maybe_warp_cursor_to_focus_centered() {
                        self.move_cursor_to_output(&output);
                    }
                    self.clarion.layer_shell_on_demand_focus = None;
                }
            }
            Action::FocusMonitorDown => {
                if let Some(output) = self.clarion.output_down() {
                    self.clarion.layout.focus_output(&output);
                    if !self.maybe_warp_cursor_to_focus_centered() {
                        self.move_cursor_to_output(&output);
                    }
                    self.clarion.layer_shell_on_demand_focus = None;
                }
            }
            Action::FocusMonitorUp => {
                if let Some(output) = self.clarion.output_up() {
                    self.clarion.layout.focus_output(&output);
                    if !self.maybe_warp_cursor_to_focus_centered() {
                        self.move_cursor_to_output(&output);
                    }
                    self.clarion.layer_shell_on_demand_focus = None;
                }
            }
            Action::FocusMonitorPrevious => {
                if let Some(output) = self.clarion.output_previous() {
                    self.clarion.layout.focus_output(&output);
                    if !self.maybe_warp_cursor_to_focus_centered() {
                        self.move_cursor_to_output(&output);
                    }
                    self.clarion.layer_shell_on_demand_focus = None;
                }
            }
            Action::FocusMonitorNext => {
                if let Some(output) = self.clarion.output_next() {
                    self.clarion.layout.focus_output(&output);
                    if !self.maybe_warp_cursor_to_focus_centered() {
                        self.move_cursor_to_output(&output);
                    }
                    self.clarion.layer_shell_on_demand_focus = None;
                }
            }
            Action::FocusMonitor(output) => {
                if let Some(output) = self.clarion.output_by_name_match(&output).cloned() {
                    self.clarion.layout.focus_output(&output);
                    if !self.maybe_warp_cursor_to_focus_centered() {
                        self.move_cursor_to_output(&output);
                    }
                    self.clarion.layer_shell_on_demand_focus = None;
                }
            }
            Action::MoveWindowToMonitorLeft => {
                if let Some(current_output) = self.clarion.screenshot_ui.selection_output() {
                    if let Some(target_output) = self.clarion.output_left_of(current_output) {
                        self.move_cursor_to_output(&target_output);
                        self.clarion.screenshot_ui.move_to_output(target_output);
                    }
                } else if let Some(output) = self.clarion.output_left() {
                    self.clarion
                        .layout
                        .move_to_output(None, &output, None, ActivateWindow::Smart);
                    self.clarion.layout.focus_output(&output);
                    if !self.maybe_warp_cursor_to_focus_centered() {
                        self.move_cursor_to_output(&output);
                    }
                }
            }
            Action::MoveWindowToMonitorRight => {
                if let Some(current_output) = self.clarion.screenshot_ui.selection_output() {
                    if let Some(target_output) = self.clarion.output_right_of(current_output) {
                        self.move_cursor_to_output(&target_output);
                        self.clarion.screenshot_ui.move_to_output(target_output);
                    }
                } else if let Some(output) = self.clarion.output_right() {
                    self.clarion
                        .layout
                        .move_to_output(None, &output, None, ActivateWindow::Smart);
                    self.clarion.layout.focus_output(&output);
                    if !self.maybe_warp_cursor_to_focus_centered() {
                        self.move_cursor_to_output(&output);
                    }
                }
            }
            Action::MoveWindowToMonitorDown => {
                if let Some(current_output) = self.clarion.screenshot_ui.selection_output() {
                    if let Some(target_output) = self.clarion.output_down_of(current_output) {
                        self.move_cursor_to_output(&target_output);
                        self.clarion.screenshot_ui.move_to_output(target_output);
                    }
                } else if let Some(output) = self.clarion.output_down() {
                    self.clarion
                        .layout
                        .move_to_output(None, &output, None, ActivateWindow::Smart);
                    self.clarion.layout.focus_output(&output);
                    if !self.maybe_warp_cursor_to_focus_centered() {
                        self.move_cursor_to_output(&output);
                    }
                }
            }
            Action::MoveWindowToMonitorUp => {
                if let Some(current_output) = self.clarion.screenshot_ui.selection_output() {
                    if let Some(target_output) = self.clarion.output_up_of(current_output) {
                        self.move_cursor_to_output(&target_output);
                        self.clarion.screenshot_ui.move_to_output(target_output);
                    }
                } else if let Some(output) = self.clarion.output_up() {
                    self.clarion
                        .layout
                        .move_to_output(None, &output, None, ActivateWindow::Smart);
                    self.clarion.layout.focus_output(&output);
                    if !self.maybe_warp_cursor_to_focus_centered() {
                        self.move_cursor_to_output(&output);
                    }
                }
            }
            Action::MoveWindowToMonitorPrevious => {
                if let Some(current_output) = self.clarion.screenshot_ui.selection_output() {
                    if let Some(target_output) = self.clarion.output_previous_of(current_output) {
                        self.move_cursor_to_output(&target_output);
                        self.clarion.screenshot_ui.move_to_output(target_output);
                    }
                } else if let Some(output) = self.clarion.output_previous() {
                    self.clarion
                        .layout
                        .move_to_output(None, &output, None, ActivateWindow::Smart);
                    self.clarion.layout.focus_output(&output);
                    if !self.maybe_warp_cursor_to_focus_centered() {
                        self.move_cursor_to_output(&output);
                    }
                }
            }
            Action::MoveWindowToMonitorNext => {
                if let Some(current_output) = self.clarion.screenshot_ui.selection_output() {
                    if let Some(target_output) = self.clarion.output_next_of(current_output) {
                        self.move_cursor_to_output(&target_output);
                        self.clarion.screenshot_ui.move_to_output(target_output);
                    }
                } else if let Some(output) = self.clarion.output_next() {
                    self.clarion
                        .layout
                        .move_to_output(None, &output, None, ActivateWindow::Smart);
                    self.clarion.layout.focus_output(&output);
                    if !self.maybe_warp_cursor_to_focus_centered() {
                        self.move_cursor_to_output(&output);
                    }
                }
            }
            Action::MoveWindowToMonitor(output) => {
                if let Some(output) = self.clarion.output_by_name_match(&output).cloned() {
                    if self.clarion.screenshot_ui.is_open() {
                        self.move_cursor_to_output(&output);
                        self.clarion.screenshot_ui.move_to_output(output);
                    } else {
                        self.clarion.layout.move_to_output(
                            None,
                            &output,
                            None,
                            ActivateWindow::Smart,
                        );
                        self.clarion.layout.focus_output(&output);
                        if !self.maybe_warp_cursor_to_focus_centered() {
                            self.move_cursor_to_output(&output);
                        }
                    }
                }
            }
            Action::MoveWindowToMonitorById { id, output } => {
                if let Some(output) = self.clarion.output_by_name_match(&output).cloned() {
                    let window = self
                        .clarion
                        .layout
                        .windows()
                        .find(|(_, m)| m.id().get() == id);
                    let window = window.map(|(_, m)| m.window.clone());

                    if let Some(window) = window {
                        let target_was_active = self
                            .clarion
                            .layout
                            .active_output()
                            .is_some_and(|active| output == *active);

                        self.clarion.layout.move_to_output(
                            Some(&window),
                            &output,
                            None,
                            ActivateWindow::Smart,
                        );

                        // If the active output changed (window was moved and focused).
                        #[allow(clippy::collapsible_if)]
                        if !target_was_active
                            && self.clarion.layout.active_output() == Some(&output)
                        {
                            if !self.maybe_warp_cursor_to_focus_centered() {
                                self.move_cursor_to_output(&output);
                            }
                        }
                    }
                }
            }
            Action::MoveColumnToMonitorLeft => {
                if let Some(current_output) = self.clarion.screenshot_ui.selection_output() {
                    if let Some(target_output) = self.clarion.output_left_of(current_output) {
                        self.move_cursor_to_output(&target_output);
                        self.clarion.screenshot_ui.move_to_output(target_output);
                    }
                } else if let Some(output) = self.clarion.output_left() {
                    self.clarion
                        .layout
                        .move_column_to_output(&output, None, true);
                    self.clarion.layout.focus_output(&output);
                    if !self.maybe_warp_cursor_to_focus_centered() {
                        self.move_cursor_to_output(&output);
                    }
                }
            }
            Action::MoveColumnToMonitorRight => {
                if let Some(current_output) = self.clarion.screenshot_ui.selection_output() {
                    if let Some(target_output) = self.clarion.output_right_of(current_output) {
                        self.move_cursor_to_output(&target_output);
                        self.clarion.screenshot_ui.move_to_output(target_output);
                    }
                } else if let Some(output) = self.clarion.output_right() {
                    self.clarion
                        .layout
                        .move_column_to_output(&output, None, true);
                    self.clarion.layout.focus_output(&output);
                    if !self.maybe_warp_cursor_to_focus_centered() {
                        self.move_cursor_to_output(&output);
                    }
                }
            }
            Action::MoveColumnToMonitorDown => {
                if let Some(current_output) = self.clarion.screenshot_ui.selection_output() {
                    if let Some(target_output) = self.clarion.output_down_of(current_output) {
                        self.move_cursor_to_output(&target_output);
                        self.clarion.screenshot_ui.move_to_output(target_output);
                    }
                } else if let Some(output) = self.clarion.output_down() {
                    self.clarion
                        .layout
                        .move_column_to_output(&output, None, true);
                    self.clarion.layout.focus_output(&output);
                    if !self.maybe_warp_cursor_to_focus_centered() {
                        self.move_cursor_to_output(&output);
                    }
                }
            }
            Action::MoveColumnToMonitorUp => {
                if let Some(current_output) = self.clarion.screenshot_ui.selection_output() {
                    if let Some(target_output) = self.clarion.output_up_of(current_output) {
                        self.move_cursor_to_output(&target_output);
                        self.clarion.screenshot_ui.move_to_output(target_output);
                    }
                } else if let Some(output) = self.clarion.output_up() {
                    self.clarion
                        .layout
                        .move_column_to_output(&output, None, true);
                    self.clarion.layout.focus_output(&output);
                    if !self.maybe_warp_cursor_to_focus_centered() {
                        self.move_cursor_to_output(&output);
                    }
                }
            }
            Action::MoveColumnToMonitorPrevious => {
                if let Some(current_output) = self.clarion.screenshot_ui.selection_output() {
                    if let Some(target_output) = self.clarion.output_previous_of(current_output) {
                        self.move_cursor_to_output(&target_output);
                        self.clarion.screenshot_ui.move_to_output(target_output);
                    }
                } else if let Some(output) = self.clarion.output_previous() {
                    self.clarion
                        .layout
                        .move_column_to_output(&output, None, true);
                    self.clarion.layout.focus_output(&output);
                    if !self.maybe_warp_cursor_to_focus_centered() {
                        self.move_cursor_to_output(&output);
                    }
                }
            }
            Action::MoveColumnToMonitorNext => {
                if let Some(current_output) = self.clarion.screenshot_ui.selection_output() {
                    if let Some(target_output) = self.clarion.output_next_of(current_output) {
                        self.move_cursor_to_output(&target_output);
                        self.clarion.screenshot_ui.move_to_output(target_output);
                    }
                } else if let Some(output) = self.clarion.output_next() {
                    self.clarion
                        .layout
                        .move_column_to_output(&output, None, true);
                    self.clarion.layout.focus_output(&output);
                    if !self.maybe_warp_cursor_to_focus_centered() {
                        self.move_cursor_to_output(&output);
                    }
                }
            }
            Action::MoveColumnToMonitor(output) => {
                if let Some(output) = self.clarion.output_by_name_match(&output).cloned() {
                    if self.clarion.screenshot_ui.is_open() {
                        self.move_cursor_to_output(&output);
                        self.clarion.screenshot_ui.move_to_output(output);
                    } else {
                        self.clarion
                            .layout
                            .move_column_to_output(&output, None, true);
                        self.clarion.layout.focus_output(&output);
                        if !self.maybe_warp_cursor_to_focus_centered() {
                            self.move_cursor_to_output(&output);
                        }
                    }
                }
            }
            Action::SetColumnWidth(change) => {
                if self.clarion.screenshot_ui.is_open() {
                    self.clarion.screenshot_ui.set_width(change);

                    // FIXME: granular
                    self.clarion.queue_redraw_all();
                } else {
                    self.clarion.layout.set_column_width(change);
                }
            }
            Action::SetWindowWidth(change) => {
                if self.clarion.screenshot_ui.is_open() {
                    self.clarion.screenshot_ui.set_width(change);

                    // FIXME: granular
                    self.clarion.queue_redraw_all();
                } else {
                    self.clarion.layout.set_window_width(None, change);
                }
            }
            Action::SetWindowWidthById { id, change } => {
                let window = self
                    .clarion
                    .layout
                    .windows()
                    .find(|(_, m)| m.id().get() == id);
                let window = window.map(|(_, m)| m.window.clone());
                if let Some(window) = window {
                    self.clarion.layout.set_window_width(Some(&window), change);
                }
            }
            Action::SetWindowHeight(change) => {
                if self.clarion.screenshot_ui.is_open() {
                    self.clarion.screenshot_ui.set_height(change);

                    // FIXME: granular
                    self.clarion.queue_redraw_all();
                } else {
                    self.clarion.layout.set_window_height(None, change);
                }
            }
            Action::SetWindowHeightById { id, change } => {
                let window = self
                    .clarion
                    .layout
                    .windows()
                    .find(|(_, m)| m.id().get() == id);
                let window = window.map(|(_, m)| m.window.clone());
                if let Some(window) = window {
                    self.clarion.layout.set_window_height(Some(&window), change);
                }
            }
            Action::ResetWindowHeight => {
                self.clarion.layout.reset_window_height(None);
            }
            Action::ResetWindowHeightById(id) => {
                let window = self
                    .clarion
                    .layout
                    .windows()
                    .find(|(_, m)| m.id().get() == id);
                let window = window.map(|(_, m)| m.window.clone());
                if let Some(window) = window {
                    self.clarion.layout.reset_window_height(Some(&window));
                }
            }
            Action::ExpandColumnToAvailableWidth => {
                self.clarion.layout.expand_column_to_available_width();
            }
            Action::ShowHotkeyOverlay => {
                if self.clarion.hotkey_overlay.show() {
                    self.clarion.queue_redraw_all();

                    #[cfg(feature = "dbus")]
                    self.clarion.a11y_announce_hotkey_overlay();
                }
            }
            Action::MoveWorkspaceToMonitorLeft => {
                if let Some(output) = self.clarion.output_left() {
                    self.clarion.layout.move_workspace_to_output(&output);
                    if !self.maybe_warp_cursor_to_focus_centered() {
                        self.move_cursor_to_output(&output);
                    }
                }
            }
            Action::MoveWorkspaceToMonitorRight => {
                if let Some(output) = self.clarion.output_right() {
                    self.clarion.layout.move_workspace_to_output(&output);
                    if !self.maybe_warp_cursor_to_focus_centered() {
                        self.move_cursor_to_output(&output);
                    }
                }
            }
            Action::MoveWorkspaceToMonitorDown => {
                if let Some(output) = self.clarion.output_down() {
                    self.clarion.layout.move_workspace_to_output(&output);
                    if !self.maybe_warp_cursor_to_focus_centered() {
                        self.move_cursor_to_output(&output);
                    }
                }
            }
            Action::MoveWorkspaceToMonitorUp => {
                if let Some(output) = self.clarion.output_up() {
                    self.clarion.layout.move_workspace_to_output(&output);
                    if !self.maybe_warp_cursor_to_focus_centered() {
                        self.move_cursor_to_output(&output);
                    }
                }
            }
            Action::MoveWorkspaceToMonitorPrevious => {
                if let Some(output) = self.clarion.output_previous() {
                    self.clarion.layout.move_workspace_to_output(&output);
                    if !self.maybe_warp_cursor_to_focus_centered() {
                        self.move_cursor_to_output(&output);
                    }
                }
            }
            Action::MoveWorkspaceToMonitorNext => {
                if let Some(output) = self.clarion.output_next() {
                    self.clarion.layout.move_workspace_to_output(&output);
                    if !self.maybe_warp_cursor_to_focus_centered() {
                        self.move_cursor_to_output(&output);
                    }
                }
            }
            Action::MoveWorkspaceToMonitor(new_output) => {
                if let Some(new_output) = self.clarion.output_by_name_match(&new_output).cloned() {
                    if self.clarion.layout.move_workspace_to_output(&new_output)
                        && !self.maybe_warp_cursor_to_focus_centered()
                    {
                        self.move_cursor_to_output(&new_output);
                    }
                }
            }
            Action::MoveWorkspaceToMonitorByRef {
                output_name,
                reference,
            } => {
                if let Some((output, old_idx)) =
                    self.clarion.find_output_and_workspace_index(reference)
                {
                    if let Some(new_output) =
                        self.clarion.output_by_name_match(&output_name).cloned()
                    {
                        if self.clarion.layout.move_workspace_to_output_by_id(
                            old_idx,
                            output,
                            &new_output,
                        ) {
                            // Cursor warp already calls `queue_redraw_all`
                            if !self.maybe_warp_cursor_to_focus_centered() {
                                self.move_cursor_to_output(&new_output);
                            }
                        }
                    }
                }
            }
            Action::ToggleWindowFloating => {
                self.clarion.layout.toggle_window_floating(None);
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::ToggleWindowFloatingById(id) => {
                let window = self
                    .clarion
                    .layout
                    .windows()
                    .find(|(_, m)| m.id().get() == id);
                let window = window.map(|(_, m)| m.window.clone());
                if let Some(window) = window {
                    self.clarion.layout.toggle_window_floating(Some(&window));
                    // FIXME: granular
                    self.clarion.queue_redraw_all();
                }
            }
            Action::MoveWindowToFloating => {
                self.clarion.layout.set_window_floating(None, true);
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::MoveWindowToFloatingById(id) => {
                let window = self
                    .clarion
                    .layout
                    .windows()
                    .find(|(_, m)| m.id().get() == id);
                let window = window.map(|(_, m)| m.window.clone());
                if let Some(window) = window {
                    self.clarion.layout.set_window_floating(Some(&window), true);
                    // FIXME: granular
                    self.clarion.queue_redraw_all();
                }
            }
            Action::MoveWindowToTiling => {
                self.clarion.layout.set_window_floating(None, false);
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::MoveWindowToTilingById(id) => {
                let window = self
                    .clarion
                    .layout
                    .windows()
                    .find(|(_, m)| m.id().get() == id);
                let window = window.map(|(_, m)| m.window.clone());
                if let Some(window) = window {
                    self.clarion
                        .layout
                        .set_window_floating(Some(&window), false);
                    // FIXME: granular
                    self.clarion.queue_redraw_all();
                }
            }
            Action::FocusFloating => {
                self.clarion.layout.focus_floating();
                self.maybe_warp_cursor_to_focus();
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::FocusTiling => {
                self.clarion.layout.focus_tiling();
                self.maybe_warp_cursor_to_focus();
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::SwitchFocusBetweenFloatingAndTiling => {
                self.clarion.layout.switch_focus_floating_tiling();
                self.maybe_warp_cursor_to_focus();
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::MoveFloatingWindowById { id, x, y } => {
                let window = if let Some(id) = id {
                    let window = self
                        .clarion
                        .layout
                        .windows()
                        .find(|(_, m)| m.id().get() == id);
                    let window = window.map(|(_, m)| m.window.clone());
                    if window.is_none() {
                        return;
                    }
                    window
                } else {
                    None
                };

                self.clarion
                    .layout
                    .move_floating_window(window.as_ref(), x, y, true);
                // FIXME: granular
                self.clarion.queue_redraw_all();
            }
            Action::ToggleWindowRuleOpacity => {
                let active_window = self
                    .clarion
                    .layout
                    .active_workspace_mut()
                    .and_then(|ws| ws.active_window_mut());
                if let Some(window) = active_window {
                    if window.rules().opacity.is_some_and(|o| o != 1.) {
                        window.toggle_ignore_opacity_window_rule();
                        // FIXME: granular
                        self.clarion.queue_redraw_all();
                    }
                }
            }
            Action::ToggleWindowRuleOpacityById(id) => {
                let window = self
                    .clarion
                    .layout
                    .workspaces_mut()
                    .find_map(|ws| ws.windows_mut().find(|w| w.id().get() == id));
                if let Some(window) = window {
                    if window.rules().opacity.is_some_and(|o| o != 1.) {
                        window.toggle_ignore_opacity_window_rule();
                        // FIXME: granular
                        self.clarion.queue_redraw_all();
                    }
                }
            }
            Action::SetDynamicCastWindow => {
                let id = self
                    .clarion
                    .layout
                    .active_workspace()
                    .and_then(|ws| ws.active_window())
                    .map(|mapped| mapped.id().get());
                if let Some(id) = id {
                    self.set_dynamic_cast_target(CastTarget::Window { id });
                }
            }
            Action::SetDynamicCastWindowById(id) => {
                let layout = &self.clarion.layout;
                if layout.windows().any(|(_, mapped)| mapped.id().get() == id) {
                    self.set_dynamic_cast_target(CastTarget::Window { id });
                }
            }
            Action::SetDynamicCastMonitor(output) => {
                let output = match output {
                    None => self.clarion.layout.active_output(),
                    Some(name) => self.clarion.output_by_name_match(&name),
                };
                if let Some(output) = output {
                    self.set_dynamic_cast_target(CastTarget::output(output));
                }
            }
            Action::ClearDynamicCastTarget => {
                self.set_dynamic_cast_target(CastTarget::Nothing);
            }
            Action::StopCast(session_id) => {
                self.clarion.stop_cast(CastSessionId::from(session_id));
            }
            Action::ToggleOverview => {
                self.overview_command(OverviewCommand::Toggle);
            }
            Action::OpenOverview => {
                self.overview_command(OverviewCommand::Open);
            }
            Action::CloseOverview => {
                self.overview_command(OverviewCommand::Close);
            }
            Action::ToggleWindowUrgent(id) => {
                let window = self
                    .clarion
                    .layout
                    .workspaces_mut()
                    .find_map(|ws| ws.windows_mut().find(|w| w.id().get() == id));
                if let Some(window) = window {
                    let urgent = window.is_urgent();
                    window.set_urgent(!urgent);
                }
                self.clarion.queue_redraw_all();
            }
            Action::SetWindowUrgent(id) => {
                let window = self
                    .clarion
                    .layout
                    .workspaces_mut()
                    .find_map(|ws| ws.windows_mut().find(|w| w.id().get() == id));
                if let Some(window) = window {
                    window.set_urgent(true);
                }
                self.clarion.queue_redraw_all();
            }
            Action::UnsetWindowUrgent(id) => {
                let window = self
                    .clarion
                    .layout
                    .workspaces_mut()
                    .find_map(|ws| ws.windows_mut().find(|w| w.id().get() == id));
                if let Some(window) = window {
                    window.set_urgent(false);
                }
                self.clarion.queue_redraw_all();
            }
            Action::LoadConfigFile(path) => {
                if let Some(watcher) = &self.clarion.config_file_watcher {
                    watcher.load_config(path);
                }
            }
            Action::MruConfirm => {
                self.confirm_mru();
            }
            Action::MruCancel => {
                self.clarion.cancel_mru();
            }
            Action::MruAdvance {
                direction,
                scope,
                filter,
            } => {
                if self.clarion.window_mru_ui.is_open() {
                    self.clarion.window_mru_ui.advance(direction, filter);
                    self.clarion.queue_redraw_mru_output();
                } else if self.clarion.config.borrow().recent_windows.on {
                    self.clarion.mru_apply_keyboard_commit();

                    let config = self.clarion.config.borrow();
                    let scope = scope.unwrap_or(self.clarion.window_mru_ui.scope());

                    let mut wmru = WindowMru::new(&self.clarion);
                    if !wmru.is_empty() {
                        wmru.set_scope(scope);
                        if let Some(filter) = filter {
                            wmru.set_filter(filter);
                        }

                        if let Some(output) = self.clarion.layout.active_output() {
                            self.clarion.window_mru_ui.open(
                                self.clarion.clock.clone(),
                                wmru,
                                output.clone(),
                            );

                            // Only select the *next* window if some window (which should be the
                            // first one) is already focused. If nothing is focused, keep the first
                            // window (which is logically the "previously selected" one).
                            let keep_first = direction == MruDirection::Forward
                                && self.clarion.layout.focus().is_none();
                            if !keep_first {
                                self.clarion.window_mru_ui.advance(direction, None);
                            }

                            drop(config);
                            self.clarion.queue_redraw_all();
                        }
                    }
                }
            }
            Action::MruCloseCurrentWindow => {
                if self.clarion.window_mru_ui.is_open() {
                    if let Some(id) = self.clarion.window_mru_ui.current_window_id() {
                        if let Some(w) = self.clarion.find_window_by_id(id) {
                            if let Some(tl) = w.toplevel() {
                                tl.send_close();
                            }
                        }
                    }
                }
            }
            Action::MruFirst => {
                if self.clarion.window_mru_ui.is_open() {
                    self.clarion.window_mru_ui.first();
                    self.clarion.queue_redraw_mru_output();
                }
            }
            Action::MruLast => {
                if self.clarion.window_mru_ui.is_open() {
                    self.clarion.window_mru_ui.last();
                    self.clarion.queue_redraw_mru_output();
                }
            }
            Action::MruSetScope(scope) => {
                if self.clarion.window_mru_ui.is_open() {
                    self.clarion.window_mru_ui.set_scope(scope);
                    self.clarion.queue_redraw_mru_output();
                }
            }
            Action::MruCycleScope => {
                if self.clarion.window_mru_ui.is_open() {
                    self.clarion.window_mru_ui.cycle_scope();
                    self.clarion.queue_redraw_mru_output();
                }
            }
            Action::ToggleCanvas => {
                if let Some(ws) = self.clarion.layout.active_workspace_mut() {
                    ws.toggle_canvas();
                }
                self.clarion.queue_redraw_all();
            }
            Action::PanCanvas(x, y) => {
                if let Some(ws) = self.clarion.layout.active_workspace_mut() {
                    ws.pan_canvas(Point::from((f64::from(x), f64::from(y))), false);
                }
                self.clarion.queue_redraw_all();
            }
            Action::ResetCanvas => {
                if let Some(ws) = self.clarion.layout.active_workspace_mut() {
                    ws.reset_canvas();
                }
                self.clarion.queue_redraw_all();
            }
            Action::SwitchTilingMode(mode) => {
                self.clarion.layout.switch_tiling_mode(mode);
                self.clarion.queue_redraw_all();
            }
            Action::ToggleDwindleSplit => {
                self.clarion.layout.toggle_dwindle_split();
            }
            Action::UndoLayout => {
                self.clarion.layout.undo_layout(false);
                self.clarion.queue_redraw_all();
            }
            Action::RedoLayout => {
                self.clarion.layout.undo_layout(true);
                self.clarion.queue_redraw_all();
            }
            Action::SetWindowGroup { name, windows } => {
                let ids = self
                    .clarion
                    .layout
                    .windows()
                    .filter(|(_, w)| windows.contains(&w.id().get()))
                    .map(|(_, w)| w.window.clone())
                    .collect();
                self.clarion.layout.set_window_group(name, ids);
            }
            Action::RemoveWindowGroup { name } => self.clarion.layout.remove_window_group(&name),
            Action::MoveWindowGroup { name, workspace_id } => {
                self.clarion.layout.move_window_group(
                    &name,
                    crate::layout::workspace::WorkspaceId::specific(workspace_id),
                );
                self.clarion.queue_redraw_all();
            }
            Action::RestoreSessionLayout { json } => {
                self.queue_session_layout_restore(json);
            }
            Action::ToggleWindowSpread => {
                self.overview_command(OverviewCommand::ToggleSpread);
            }
            Action::ToggleExpo => {
                self.overview_command(OverviewCommand::ToggleExpo);
            }
            Action::CloseExpo => {
                self.overview_command(OverviewCommand::CancelExpo);
            }
            Action::ExpoConfirm => {
                self.overview_command(OverviewCommand::ConfirmExpo);
            }
            Action::ExpoNavigateLeft => {
                if self.clarion.layout.expo_navigate(-1, 0) {
                    self.clarion.queue_redraw_all();
                }
            }
            Action::ExpoNavigateRight => {
                if self.clarion.layout.expo_navigate(1, 0) {
                    self.clarion.queue_redraw_all();
                }
            }
            Action::ExpoNavigateUp => {
                if self.clarion.layout.expo_navigate(0, -1) {
                    self.clarion.queue_redraw_all();
                }
            }
            Action::ExpoNavigateDown => {
                if self.clarion.layout.expo_navigate(0, 1) {
                    self.clarion.queue_redraw_all();
                }
            }
            Action::ToggleScratchpad => {
                self.clarion.layout.toggle_scratchpad();
                self.maybe_warp_cursor_to_focus();
                self.clarion.layer_shell_on_demand_focus = None;
                self.clarion.queue_redraw_all();
            }
            Action::MoveWindowToScratchpad => {
                self.clarion.layout.move_window_to_scratchpad();
                self.maybe_warp_cursor_to_focus();
                self.clarion.layer_shell_on_demand_focus = None;
                self.clarion.queue_redraw_all();
            }
            Action::ToggleWindowWobble => {
                self.clarion.layout.toggle_window_wobble();
                self.clarion.queue_redraw_all();
            }
            Action::ToggleWorkspaceCube => {
                self.clarion.layout.toggle_workspace_cube();
                self.clarion.queue_redraw_all();
            }
            Action::ToggleWindowGlide => {
                self.clarion.layout.toggle_window_glide();
                self.clarion.queue_redraw_all();
            }
        }
    }

    fn on_pointer_motion<I: InputBackend>(&mut self, event: I::PointerMotionEvent) {
        // We need an output to be able to move the pointer.
        if self.clarion.global_space.outputs().next().is_none() {
            self.clarion.reset_hot_corner();
            return;
        }

        let serial = SERIAL_COUNTER.next_serial();

        let pointer = self.clarion.seat.get_pointer().unwrap();

        let pos = pointer.current_location();

        // We have an output, so we can compute the new location and focus.
        let mut new_pos = pos + event.delta();

        // We received an event for the regular pointer, so show it now.
        self.clarion.pointer_visibility = PointerVisibility::Visible;
        self.clarion.tablet_cursor_location = None;

        // Check if we have an active pointer constraint.
        //
        // FIXME: ideally this should use the pointer focus with up-to-date global location.
        let mut pointer_confined = None;
        if let Some(under) = &self.clarion.pointer_contents.surface {
            // No need to check if the pointer focus surface matches, because here we're checking
            // for an already-active constraint, and the constraint is deactivated when the focused
            // surface changes.
            let pos_within_surface = pos - under.1;

            let mut pointer_locked = false;
            with_pointer_constraint(&under.0, &pointer, |constraint| {
                let Some(constraint) = constraint else { return };
                if !constraint.is_active() {
                    return;
                }

                // Constraint does not apply if not within region.
                if let Some(region) = constraint.region() {
                    if !region.contains(pos_within_surface.to_i32_round()) {
                        return;
                    }
                }

                match &*constraint {
                    PointerConstraint::Locked(_locked) => {
                        pointer_locked = true;
                    }
                    PointerConstraint::Confined(confine) => {
                        pointer_confined = Some((under.clone(), confine.region().cloned()));
                    }
                }
            });

            // If the pointer is locked, only send relative motion.
            if pointer_locked {
                pointer.relative_motion(
                    self,
                    Some(under.clone()),
                    &RelativeMotionEvent {
                        delta: event.delta(),
                        delta_unaccel: event.delta_unaccel(),
                        time: event.time(),
                    },
                );

                pointer.frame(self);

                // I guess a redraw to hide the tablet cursor could be nice? Doesn't matter too
                // much here I think.
                return;
            }
        }

        // Warp pointer across the screen during the spatial movement grabs.
        let spatial_grab = pointer.with_grab(|_, grab| {
            let grab = grab.as_any();
            if let Some(grab) = grab.downcast_ref::<SpatialMovementGrab>() {
                if let Some(output) = grab.view_offset_output() {
                    return Some((output.clone(), true));
                } else if let Some(output) = grab.workspace_switch_output() {
                    return Some((output.clone(), false));
                }
            } else if let Some(grab) = grab.downcast_ref::<MoveGrab>() {
                if let Some(output) = grab.view_offset_output() {
                    return Some((output.clone(), true));
                }
            }
            None
        });
        if let Some((output, horizontal)) = spatial_grab.flatten() {
            if let Some(geo) = self.clarion.global_space.output_geometry(&output) {
                let geo = geo.to_f64();
                if horizontal {
                    new_pos.x = (new_pos.x - geo.loc.x).rem_euclid(geo.size.w) + geo.loc.x;
                    new_pos.y = new_pos.y.clamp(geo.loc.y, geo.loc.y + geo.size.h - 1.);
                } else {
                    new_pos.x = new_pos.x.clamp(geo.loc.x, geo.loc.x + geo.size.w - 1.);
                    new_pos.y = (new_pos.y - geo.loc.y).rem_euclid(geo.size.h) + geo.loc.y;
                }
            }
        }

        if self
            .clarion
            .global_space
            .output_under(new_pos)
            .next()
            .is_none()
        {
            // We ended up outside the outputs and need to clip the movement.
            if let Some(output) = self.clarion.global_space.output_under(pos).next() {
                // The pointer was previously on some output. Clip the movement against its
                // boundaries.
                let Some(geom) = self.clarion.global_space.output_geometry(output) else {
                    return;
                };
                new_pos.x = new_pos
                    .x
                    .clamp(geom.loc.x as f64, (geom.loc.x + geom.size.w - 1) as f64);
                new_pos.y = new_pos
                    .y
                    .clamp(geom.loc.y as f64, (geom.loc.y + geom.size.h - 1) as f64);
            } else {
                // The pointer was not on any output in the first place. Find one for it.
                // Let's do the simple thing and just put it on the first output.
                let Some(output) = self.clarion.global_space.outputs().next() else {
                    // Outputs can temporarily be absent while they are being reconfigured.
                    return;
                };
                let Some(geom) = self.clarion.global_space.output_geometry(output) else {
                    return;
                };
                new_pos = center(geom).to_f64();
            }
        }

        if let Some(output) = self.clarion.screenshot_ui.selection_output() {
            let Some(geom) = self.clarion.global_space.output_geometry(output) else {
                return;
            };
            let point = (new_pos - geom.loc.to_f64())
                .to_physical(output.current_scale().fractional_scale())
                .to_i32_round::<i32>();

            self.clarion.screenshot_ui.pointer_motion(point, None);
        }

        if let Some(mru_output) = self.clarion.window_mru_ui.output() {
            if let Some((output, pos_within_output)) = self.clarion.output_under(new_pos) {
                if mru_output == output {
                    self.clarion.window_mru_ui.pointer_motion(pos_within_output);
                }
            }
        }

        let under = self.clarion.contents_under(new_pos);

        // Handle confined pointer.
        if let Some((focus_surface, region)) = pointer_confined {
            let mut prevent = false;

            // Prevent the pointer from leaving the focused surface.
            if Some(&focus_surface.0) != under.surface.as_ref().map(|(s, _)| s) {
                prevent = true;
            }

            // Prevent the pointer from leaving the confine region, if any.
            if let Some(region) = region {
                let new_pos_within_surface = new_pos - focus_surface.1;
                if !region.contains(new_pos_within_surface.to_i32_round()) {
                    prevent = true;
                }
            }

            if prevent {
                pointer.relative_motion(
                    self,
                    Some(focus_surface),
                    &RelativeMotionEvent {
                        delta: event.delta(),
                        delta_unaccel: event.delta_unaccel(),
                        time: event.time(),
                    },
                );

                pointer.frame(self);

                return;
            }
        }

        self.clarion.handle_focus_follows_mouse(&under);

        self.clarion.pointer_contents.clone_from(&under);

        pointer.motion(
            self,
            under.surface.clone(),
            &MotionEvent {
                location: new_pos,
                serial,
                time: event.time(),
            },
        );

        pointer.relative_motion(
            self,
            under.surface,
            &RelativeMotionEvent {
                delta: event.delta(),
                delta_unaccel: event.delta_unaccel(),
                time: event.time(),
            },
        );

        pointer.frame(self);

        self.process_hot_corner_motion(new_pos, under.output.zip(under.hot_corner));

        // Activate a new confinement if necessary.
        self.clarion.maybe_activate_pointer_constraint();

        // Inform the layout of an ongoing DnD operation.
        let is_dnd_grab = pointer
            .with_grab(|_, grab| Self::is_dnd_grab(grab.as_any()))
            .unwrap_or(false);
        if is_dnd_grab {
            if let Some((output, pos_within_output)) = self.clarion.output_under(new_pos) {
                let output = output.clone();
                self.clarion.layout.dnd_update(output, pos_within_output);
            }
        }

        // Notify a11y.
        #[cfg(feature = "dbus")]
        self.a11y_notify_pointer_motion();

        // Redraw to update the cursor position.
        // FIXME: redraw only outputs overlapping the cursor.
        self.clarion.queue_redraw_all();
    }

    fn on_pointer_motion_absolute<I: InputBackend>(
        &mut self,
        event: I::PointerMotionAbsoluteEvent,
    ) {
        let Some(pos) = self.compute_absolute_location(&event, None).or_else(|| {
            self.global_bounding_rectangle().map(|output_geo| {
                event.position_transformed(output_geo.size) + output_geo.loc.to_f64()
            })
        }) else {
            return;
        };

        let serial = SERIAL_COUNTER.next_serial();

        let pointer = self.clarion.seat.get_pointer().unwrap();

        if let Some(output) = self.clarion.screenshot_ui.selection_output() {
            let Some(geom) = self.clarion.global_space.output_geometry(output) else {
                return;
            };
            let point = (pos - geom.loc.to_f64())
                .to_physical(output.current_scale().fractional_scale())
                .to_i32_round::<i32>();

            self.clarion.screenshot_ui.pointer_motion(point, None);
        }

        if let Some(mru_output) = self.clarion.window_mru_ui.output() {
            if let Some((output, pos_within_output)) = self.clarion.output_under(pos) {
                if mru_output == output {
                    self.clarion.window_mru_ui.pointer_motion(pos_within_output);
                }
            }
        }

        let under = self.clarion.contents_under(pos);

        self.clarion.handle_focus_follows_mouse(&under);

        self.clarion.pointer_contents.clone_from(&under);

        pointer.motion(
            self,
            under.surface,
            &MotionEvent {
                location: pos,
                serial,
                time: event.time(),
            },
        );

        pointer.frame(self);

        self.process_hot_corner_motion(pos, under.output.zip(under.hot_corner));

        self.clarion.maybe_activate_pointer_constraint();

        // We moved the pointer, show it.
        self.clarion.pointer_visibility = PointerVisibility::Visible;

        // We moved the regular pointer, so show it now.
        self.clarion.tablet_cursor_location = None;

        // Inform the layout of an ongoing DnD operation.
        let is_dnd_grab = pointer
            .with_grab(|_, grab| Self::is_dnd_grab(grab.as_any()))
            .unwrap_or(false);
        if is_dnd_grab {
            if let Some((output, pos_within_output)) = self.clarion.output_under(pos) {
                let output = output.clone();
                self.clarion.layout.dnd_update(output, pos_within_output);
            }
        }

        // Notify a11y.
        #[cfg(feature = "dbus")]
        self.a11y_notify_pointer_motion();

        // Redraw to update the cursor position.
        // FIXME: redraw only outputs overlapping the cursor.
        self.clarion.queue_redraw_all();
    }

    /// Shared by relative and absolute pointer motion, after updating pointer focus.
    pub(crate) fn process_hot_corner_motion(
        &mut self,
        pos: Point<f64, Logical>,
        candidate: Option<(Output, HotCornerKind)>,
    ) {
        if !matches!(self.clarion.lock_state, LockState::Unlocked)
            || self.clarion.screenshot_ui.is_open()
            || self.clarion.window_mru_ui.is_open()
            || self.clarion.exit_confirm_dialog.is_open()
        {
            self.clarion.reset_hot_corner();
            return;
        }
        let pointer = self.clarion.seat.get_pointer().unwrap();
        let touch_grabbed = self
            .clarion
            .seat
            .get_touch()
            .is_some_and(|touch| touch.is_grabbed());
        let tablet_grabbed = self
            .clarion
            .seat
            .tablet_seat()
            .get_tools()
            .values()
            .any(|tool| tool.is_grabbed());
        let allowed = pointer.current_focus().is_none()
            && !touch_grabbed
            && !tablet_grabbed
            && pointer
                .with_grab(|_, grab| grab_allows_hot_corner(grab))
                .unwrap_or(true);
        let candidate = candidate.filter(|_| allowed);
        let corner = candidate.as_ref().map(|(_, corner)| *corner);
        if !self.clarion.hot_corner_motion_entered(pos, candidate) {
            return;
        }
        match corner.unwrap() {
            HotCornerKind::TopRight => {
                self.overview_command(OverviewCommand::ToggleExpo);
            }
            // The bottom-left corner must not open the settings center.
            HotCornerKind::BottomLeft => {}
            _ => {
                self.overview_command(OverviewCommand::Toggle);
            }
        }
    }

    /// Clear grabs and ordinary focus before detaching a still-mapped window.
    /// Other applications' active operations are left untouched.
    pub(crate) fn minimize_window_by_id(&mut self, id: u64) {
        if !matches!(
            self.clarion.lock_state,
            crate::compositor::LockState::Unlocked
        ) || self.clarion.screenshot_ui.is_open()
        {
            return;
        }
        let window = self
            .clarion
            .layout
            .windows()
            .find(|(_, mapped)| mapped.id().get() == id)
            .map(|(_, mapped)| mapped.window.clone());
        let Some(window) = window else {
            return;
        };
        if self.clarion.layout.is_minimized(&window) {
            return;
        }
        let Some(surface) = window.toplevel().map(|top| top.wl_surface().clone()) else {
            return;
        };
        let time = crate::utils::input_time();
        if let Some(pointer) = self.clarion.seat.get_pointer() {
            let targets_window =
                pointer.current_focus().as_ref().is_some_and(|focused| {
                    self.clarion.find_root_shell_surface(focused) == surface
                }) || pointer
                    .with_grab(|_, grab| {
                        grab.start_data()
                            .focus
                            .as_ref()
                            .is_some_and(|(focused, _)| {
                                self.clarion.find_root_shell_surface(focused) == surface
                            })
                    })
                    .unwrap_or(false);
            if targets_window {
                pointer.unset_grab(self, SERIAL_COUNTER.next_serial(), time);
            }
        }
        if let Some(touch) = self.clarion.seat.get_touch() {
            let targets_window = touch
                .with_grab(|_, grab| {
                    grab.start_data()
                        .focus
                        .as_ref()
                        .is_some_and(|(focused, _)| {
                            self.clarion.find_root_shell_surface(focused) == surface
                        })
                })
                .unwrap_or(false);
            if targets_window {
                touch.cancel(self);
            }
        }
        let tools: Vec<_> = self
            .clarion
            .seat
            .tablet_seat()
            .get_tools()
            .into_values()
            .collect();
        for tool in tools {
            let targets_window = tool
                .with_grab(|_, grab| {
                    grab.start_data()
                        .focus
                        .as_ref()
                        .is_some_and(|(focused, _)| {
                            self.clarion.find_root_shell_surface(focused) == surface
                        })
                })
                .unwrap_or(false);
            if targets_window {
                tool.unset_grab(self, SERIAL_COUNTER.next_serial(), time);
            }
        }
        if self.clarion.layout.minimize_window(&window) {
            self.update_keyboard_focus();
            self.update_pointer_contents();
            if let Some(pointer) = self.clarion.seat.get_pointer() {
                pointer.frame(self);
            }
            self.clarion.queue_redraw_all();
        }
    }

    /// Release compositor-owned operations while retaining application DnD.
    /// Call after capturing the visible frame and before changing overview mode.
    pub(crate) fn cancel_overview_grabs(&mut self) {
        self.cancel_layout_gestures();
        let time = crate::utils::input_time();
        if let Some(pointer) = self.clarion.seat.get_pointer() {
            let compositor_grab = pointer
                .with_grab(|_, grab| {
                    let grab = grab.as_any();
                    grab.is::<MoveGrab>()
                        || grab.is::<SpatialMovementGrab>()
                        || grab.is::<ResizeGrab>()
                })
                .unwrap_or(false);
            if compositor_grab {
                pointer.unset_grab(self, SERIAL_COUNTER.next_serial(), time);
            }
        }
        if let Some(touch) = self.clarion.seat.get_touch() {
            let compositor_grab = touch
                .with_grab(|_, grab| {
                    let grab = grab.as_any();
                    grab.is::<MoveGrab>()
                        || grab.is::<TouchOverviewGrab>()
                        || grab.is::<ResizeGrab>()
                })
                .unwrap_or(false);
            if compositor_grab {
                touch.cancel(self);
            }
        }
        let tools: Vec<_> = self
            .clarion
            .seat
            .tablet_seat()
            .get_tools()
            .into_values()
            .collect();
        for tool in tools {
            let compositor_grab = tool
                .with_grab(|_, grab| {
                    let grab = grab.as_any();
                    grab.is::<MoveGrab>()
                        || grab.is::<TouchOverviewGrab>()
                        || grab.is::<ResizeGrab>()
                })
                .unwrap_or(false);
            if compositor_grab {
                tool.unset_grab(self, SERIAL_COUNTER.next_serial(), time);
            }
        }
    }

    fn cancel_layout_gestures(&mut self) {
        self.clarion.gesture_swipe_3f_cumulative = None;
        if let Some(pinch) = &mut self.clarion.launcher_pinch {
            pinch.cancel();
        }
        self.clarion.overview_scroll_swipe_gesture.reset();
        while self
            .clarion
            .layout
            .workspace_switch_gesture_cancel(None)
            .is_some()
        {}
        while self
            .clarion
            .layout
            .view_offset_gesture_cancel(None)
            .is_some()
        {}
        self.overview_command(OverviewCommand::GestureEnd { cancelled: true });
    }

    /// A lock request owns input immediately, including the surface-wait phase.
    pub(crate) fn cancel_grabs_for_lock(&mut self) {
        self.cancel_layout_gestures();
        let time = crate::utils::input_time();
        if let Some(pointer) = self.clarion.seat.get_pointer() {
            pointer.unset_grab(self, SERIAL_COUNTER.next_serial(), time);
        }
        if let Some(touch) = self.clarion.seat.get_touch() {
            if touch.is_grabbed() {
                touch.cancel(self);
            }
        }
        let tools: Vec<_> = self
            .clarion
            .seat
            .tablet_seat()
            .get_tools()
            .into_values()
            .collect();
        for tool in tools {
            tool.unset_grab(self, SERIAL_COUNTER.next_serial(), time);
        }
    }

    pub fn toggle_settings_gui(&mut self) {
        let existing = self.clarion.layout.windows().find(|(_, m)| {
            crate::utils::with_toplevel_role(m.toplevel(), |role| {
                role.app_id.as_deref() == Some("clarion-settings")
                    || role.app_id.as_deref() == Some("rs.bxt.clarion.settings")
                    || role.title.as_deref() == Some("Clarion Settings")
            })
        });

        if let Some((_, mapped)) = existing {
            if let Some(focused) = self.clarion.layout.focus() {
                if focused.id() == mapped.id() {
                    mapped.toplevel().send_close();
                    return;
                }
            }
            let window = mapped.window.clone();
            self.focus_window(&window);
        } else {
            let (token, _) = self.clarion.activation_state.create_external_token(None);
            spawn(vec![String::from("clarion-settings")], Some(token.clone()));
        }
    }

    fn on_pointer_button<I: InputBackend>(&mut self, event: I::PointerButtonEvent) {
        let pointer = self.clarion.seat.get_pointer().unwrap();

        let serial = SERIAL_COUNTER.next_serial();

        let button = event.button();

        let button_code = event.button_code();

        let button_state = event.state();

        let mod_key = self.backend.mod_key(&self.clarion.config.borrow());

        // Ignore release events for mouse clicks that triggered a bind.
        if self.clarion.suppressed_buttons.remove(&button_code) {
            return;
        }

        let mods = self.clarion.seat.get_keyboard().unwrap().modifier_state();
        let modifiers = modifiers_from_state(mods);
        let mod_down = modifiers.contains(mod_key.to_modifiers());

        if ButtonState::Pressed == button_state {
            // A new click/drag takes control from a coasting canvas before hit testing.
            if !self.clarion.is_locked() {
                for ws in self.clarion.layout.workspaces_mut() {
                    ws.stop_canvas_pan();
                }
            }

            let mut is_mru_open = false;
            if let Some(mru_output) = self.clarion.window_mru_ui.output() {
                is_mru_open = true;
                if let Some(MouseButton::Left) = button {
                    let location = pointer.current_location();
                    let Some((output, pos_within_output)) = self.clarion.output_under(location)
                    else {
                        self.clarion.cancel_mru();
                        self.clarion.suppressed_buttons.insert(button_code);
                        return;
                    };
                    if mru_output == output {
                        let id = self.clarion.window_mru_ui.pointer_motion(pos_within_output);
                        if id.is_some() {
                            self.confirm_mru();
                        } else {
                            self.clarion.cancel_mru();
                        }
                    } else {
                        self.clarion.cancel_mru();
                    }

                    self.clarion.suppressed_buttons.insert(button_code);
                    return;
                }
            }

            if is_mru_open || self.clarion.mods_with_mouse_binds.contains(&modifiers) {
                if let Some(bind) = match button {
                    Some(MouseButton::Left) => Some(Trigger::MouseLeft),
                    Some(MouseButton::Right) => Some(Trigger::MouseRight),
                    Some(MouseButton::Middle) => Some(Trigger::MouseMiddle),
                    Some(MouseButton::Back) => Some(Trigger::MouseBack),
                    Some(MouseButton::Forward) => Some(Trigger::MouseForward),
                    _ => None,
                }
                .and_then(|trigger| {
                    let config = self.clarion.config.borrow();
                    let bindings =
                        make_binds_iter(&config, &mut self.clarion.window_mru_ui, modifiers);
                    find_configured_bind(bindings, mod_key, trigger, mods)
                })
                .filter(|bind| {
                    !self.clarion.screenshot_ui.is_open() || allowed_during_screenshot(&bind.action)
                }) {
                    self.clarion.suppressed_buttons.insert(button_code);
                    self.handle_bind(bind.clone());
                    return;
                };
            }

            // We received an event for the regular pointer, so show it now.
            self.clarion.pointer_visibility = PointerVisibility::Visible;
            self.clarion.tablet_cursor_location = None;

            if self.clarion.overview_mode_transition_active()
                && self.clarion.pointer_contents.layer.is_none()
            {
                self.clarion.suppressed_buttons.insert(button_code);
                return;
            }

            let is_overview_open = self.clarion.layout.is_overview_open();

            if self.clarion.layout.is_stage_open()
                && !pointer.is_grabbed()
                && button == Some(MouseButton::Left)
                && self.clarion.pointer_contents.layer.is_none()
                && !self.clarion.is_locked()
                && !self.clarion.screenshot_ui.is_open()
            {
                let location = pointer.current_location();
                if let Some((output, position)) = self.clarion.output_under(location) {
                    let output = output.clone();
                    if self.overview_command(OverviewCommand::StageSelect { output, position }) {
                        self.clarion.suppressed_buttons.insert(button_code);
                        self.clarion.queue_redraw_all();
                        return;
                    }
                }
            }

            if is_overview_open && !pointer.is_grabbed() && button == Some(MouseButton::Right) {
                if let Some((output, ws)) = self.clarion.workspace_under_cursor(true) {
                    let ws_id = ws.id();
                    let ws_idx = self.clarion.layout.find_workspace_by_id(ws_id).unwrap().0;

                    self.clarion.layout.focus_output(&output);

                    let location = pointer.current_location();
                    let start_data = PointerGrabStartData {
                        focus: None,
                        button: button_code,
                        location,
                    };
                    self.clarion
                        .layout
                        .view_offset_gesture_begin(&output, Some(ws_idx), false);
                    let grab = SpatialMovementGrab::new(start_data, output, ws_id, true, false);
                    pointer.set_grab(self, grab, serial, Focus::Clear);
                    self.clarion
                        .cursor_manager
                        .set_cursor_image(CursorImageStatus::Named(CursorIcon::AllScroll));

                    // FIXME: granular.
                    self.clarion.queue_redraw_all();
                    return;
                }
            }

            if button == Some(MouseButton::Middle)
                && !pointer.is_grabbed()
                && mod_down
                && !self.clarion.layout.is_window_spread_closing()
            {
                let output_ws = if is_overview_open {
                    self.clarion.workspace_under_cursor(true)
                } else {
                    // We don't want to accidentally "catch" the wrong workspace during
                    // animations.
                    self.clarion.output_under_cursor().and_then(|output| {
                        let mon = self.clarion.layout.monitor_for_output(&output)?;
                        Some((output, mon.active_workspace_ref()))
                    })
                };

                if let Some((output, ws)) = output_ws {
                    let ws_id = ws.id();
                    let canvas = ws.canvas_enabled() && !is_overview_open;

                    self.clarion.layout.focus_output(&output);

                    let location = pointer.current_location();
                    let start_data = PointerGrabStartData {
                        focus: None,
                        button: button_code,
                        location,
                    };
                    let grab = SpatialMovementGrab::new(start_data, output, ws_id, false, canvas);
                    pointer.set_grab(self, grab, serial, Focus::Clear);
                    self.clarion
                        .cursor_manager
                        .set_cursor_image(CursorImageStatus::Named(CursorIcon::AllScroll));

                    // FIXME: granular.
                    self.clarion.queue_redraw_all();

                    // Don't activate the window under the cursor to avoid unnecessary
                    // scrolling when e.g. Mod+MMB clicking on a partially off-screen window.
                    return;
                }
            }

            if self.clarion.screenshot_ui.is_open() {
                // Screenshot UI handles this click below; don't activate the layout underneath.
            } else if let Some(mapped) = self.clarion.window_under_cursor() {
                let window = mapped.window.clone();

                // Check if we need to start an interactive move.
                if button == Some(MouseButton::Left) && !pointer.is_grabbed() {
                    if is_overview_open || mod_down {
                        let location = pointer.current_location();

                        if !is_overview_open {
                            self.clarion.layout.activate_window(&window);
                        }

                        let start_data = PointerGrabStartData {
                            focus: None,
                            button: button_code,
                            location,
                        };
                        let start_data = AnyStartData::Pointer(start_data);
                        let icon = CursorIcon::Grabbing;
                        if let Some(grab) =
                            MoveGrab::new(self, start_data, window.clone(), false, Some(icon))
                        {
                            pointer.set_grab(self, grab, serial, Focus::Clear);

                            // Set the cursor to Grabbing right away for Mod+LMB since it doesn't
                            // do any other gesture.
                            //
                            // In the overview, we click to activate window and close the overview,
                            // in this case setting the cursor right away would be distracting.
                            if !is_overview_open {
                                self.clarion
                                    .cursor_manager
                                    .set_cursor_image(CursorImageStatus::Named(icon));
                            }
                        }
                    }
                }
                // Check if we need to start an interactive resize.
                else if button == Some(MouseButton::Right) && !pointer.is_grabbed() && mod_down {
                    let location = pointer.current_location();
                    let Some((output, pos_within_output)) = self.clarion.output_under(location)
                    else {
                        // A pointer can briefly be in the gap between outputs (or outside all
                        // outputs while they are being reconfigured).  There is no output on
                        // which to perform an interactive resize in that case.
                        return;
                    };
                    let edges = self
                        .clarion
                        .layout
                        .resize_edges_under(output, pos_within_output)
                        .unwrap_or(ResizeEdge::empty());

                    if !edges.is_empty() {
                        // See if we got a double resize-click gesture.
                        // FIXME: deduplicate with resize_request in xdg-shell somehow.
                        let time = get_monotonic_time();
                        let last_cell = mapped.last_interactive_resize_start();
                        let mut last = last_cell.get();
                        last_cell.set(Some((time, edges)));

                        // Floating windows don't have either of the double-resize-click
                        // gestures, so just allow it to resize.
                        if mapped.is_floating() {
                            last = None;
                            last_cell.set(None);
                        }

                        if let Some((last_time, last_edges)) = last {
                            if time.saturating_sub(last_time) <= DOUBLE_CLICK_TIME {
                                // Allow quick resize after a triple click.
                                last_cell.set(None);

                                let intersection = edges.intersection(last_edges);
                                if intersection.intersects(ResizeEdge::LEFT_RIGHT) {
                                    // FIXME: don't activate once we can pass specific windows
                                    // to actions.
                                    self.clarion.layout.activate_window(&window);
                                    self.clarion.layout.toggle_full_width();
                                }
                                if intersection.intersects(ResizeEdge::TOP_BOTTOM) {
                                    self.clarion.layout.activate_window(&window);
                                    self.clarion.layout.reset_window_height(Some(&window));
                                }
                                // FIXME: granular.
                                self.clarion.queue_redraw_all();
                                return;
                            }
                        }

                        self.clarion.layout.activate_window(&window);

                        if self
                            .clarion
                            .layout
                            .interactive_resize_begin(window.clone(), edges)
                        {
                            let start_data = PointerGrabStartData {
                                focus: None,
                                button: button_code,
                                location,
                            };
                            let start_data = AnyStartData::Pointer(start_data);
                            let grab = ResizeGrab::new(start_data, window.clone());
                            pointer.set_grab(self, grab, serial, Focus::Clear);
                            self.clarion
                                .cursor_manager
                                .set_cursor_image(CursorImageStatus::Named(edges.cursor_icon()));
                        }
                    }
                }

                if !is_overview_open {
                    self.clarion.layout.activate_window(&window);
                }

                // FIXME: granular.
                self.clarion.queue_redraw_all();
            } else if let Some((output, ws)) = is_overview_open
                .then(|| self.clarion.workspace_under_cursor(false))
                .flatten()
            {
                let ws_idx = self.clarion.layout.find_workspace_by_id(ws.id()).unwrap().0;

                self.overview_command(OverviewCommand::EnterWorkspace {
                    output,
                    index: ws_idx,
                });
            } else if let Some(output) = self.clarion.output_under_cursor() {
                self.clarion.layout.focus_output(&output);

                // FIXME: granular.
                self.clarion.queue_redraw_all();
            }
        };

        self.update_pointer_contents();

        if ButtonState::Pressed == button_state {
            let layer_under = self.clarion.pointer_contents.layer.clone();
            self.clarion.focus_layer_surface_if_on_demand(layer_under);
        }

        if button == Some(MouseButton::Left) && self.clarion.screenshot_ui.is_open() {
            if button_state == ButtonState::Pressed {
                let pos = pointer.current_location();

                // If we'll be moving the existing selection, use the selection output.
                let output = if mod_down {
                    self.clarion.screenshot_ui.selection_output()
                } else {
                    self.clarion.output_under(pos).map(|(out, _)| out)
                };

                if let Some(output) = output.cloned() {
                    let Some(geom) = self.clarion.global_space.output_geometry(&output) else {
                        return;
                    };
                    let point = (pos - geom.loc.to_f64())
                        .to_physical(output.current_scale().fractional_scale())
                        .to_i32_round();

                    if self
                        .clarion
                        .screenshot_ui
                        .pointer_down(output, point, None, mod_down)
                    {
                        self.clarion.queue_redraw_all();
                    }
                }
            } else if let Some(capture) = self.clarion.screenshot_ui.pointer_up(None) {
                if capture {
                    self.confirm_screenshot(true);
                } else {
                    self.clarion.queue_redraw_all();
                }
            }
        }

        pointer.button(
            self,
            &ButtonEvent {
                button: button_code,
                state: button_state,
                serial,
                time: event.time(),
            },
        );
        pointer.frame(self);
    }

    fn on_pointer_axis<I: InputBackend>(&mut self, event: I::PointerAxisEvent) {
        let pointer = &self.clarion.seat.get_pointer().unwrap();

        let source = event.source();

        let mod_key = self.backend.mod_key(&self.clarion.config.borrow());

        // We received an event for the regular pointer, so show it now. This is also needed for
        // update_pointer_contents() below to return the real contents, necessary for the pointer
        // axis event to reach the window.
        self.clarion.pointer_visibility = PointerVisibility::Visible;
        self.clarion.tablet_cursor_location = None;

        let timestamp = Duration::from_micros(event.time().micros());

        let horizontal_amount_v120 = event.amount_v120(Axis::Horizontal);
        let vertical_amount_v120 = event.amount_v120(Axis::Vertical);

        let is_overview_open = self.clarion.layout.is_overview_open();

        // We should only handle scrolling in the overview if the pointer is not over a (top or
        // overlay) layer surface.
        let should_handle_in_overview = if is_overview_open
            && !self.clarion.is_locked()
            && !self.clarion.screenshot_ui.is_open()
            && !self.clarion.window_mru_ui.is_open()
            && !self.clarion.exit_confirm_dialog.is_open()
        {
            // FIXME: ideally this should happen after updating the pointer contents, which happens
            // below. However, our pointer actions are supposed to act on the old surface, before
            // updating the pointer contents.
            pointer
                .current_focus()
                .map(|surface| self.clarion.find_root_shell_surface(&surface))
                .is_none_or(|root| {
                    !self
                        .clarion
                        .mapped_layer_surfaces
                        .keys()
                        .any(|layer| *layer.wl_surface() == root)
                })
        } else {
            false
        };

        let is_mru_open = self.clarion.window_mru_ui.is_open();

        if should_handle_in_overview
            && self.clarion.overview_mode_transition_active()
            && !pointer.is_grabbed()
        {
            return;
        }

        let stage_modifiers = self.clarion.seat.get_keyboard().unwrap().modifier_state();
        if self.clarion.layout.is_stage_open()
            && should_handle_in_overview
            && !self.clarion.is_locked()
            && !self.clarion.screenshot_ui.is_open()
            && !is_mru_open
            && !pointer.is_grabbed()
            && modifiers_from_state(stage_modifiers).is_empty()
        {
            let horizontal = if source == AxisSource::Wheel {
                horizontal_amount_v120.unwrap_or(0.)
            } else {
                event.amount(Axis::Horizontal).unwrap_or(0.) * 3.
            };
            let vertical = if source == AxisSource::Wheel {
                vertical_amount_v120.unwrap_or(0.)
            } else {
                event.amount(Axis::Vertical).unwrap_or(0.) * 3.
            };
            let ticks = if horizontal.abs() > vertical.abs() {
                self.clarion.vertical_wheel_tracker.reset();
                self.clarion.horizontal_wheel_tracker.accumulate(horizontal)
            } else {
                self.clarion.horizontal_wheel_tracker.reset();
                self.clarion.vertical_wheel_tracker.accumulate(vertical)
            };
            if ticks != 0 {
                if let Some(output) = self.clarion.output_under_cursor() {
                    self.clarion
                        .layout
                        .stage_navigate(Some(&output), isize::from(ticks));
                    self.clarion.queue_redraw(&output);
                }
            }
            return;
        }

        let mods = self.clarion.seat.get_keyboard().unwrap().modifier_state();
        let canvas_mod = modifiers_from_state(mods) == mod_key.to_modifiers();
        if canvas_mod
            && !self.clarion.is_locked()
            && !is_overview_open
            && !self.clarion.layout.is_window_spread_closing()
            && !is_mru_open
            && !self.clarion.screenshot_ui.is_open()
            && !pointer.is_grabbed()
        {
            if let Some(output) = self.clarion.output_under_cursor() {
                if let Some(mon) = self.clarion.layout.monitor_for_output_mut(&output) {
                    let ws = mon.active_workspace();
                    if ws.canvas_enabled() {
                        let x = event
                            .amount(Axis::Horizontal)
                            .unwrap_or_else(|| horizontal_amount_v120.unwrap_or(0.) / 120. * 80.);
                        let y = event
                            .amount(Axis::Vertical)
                            .unwrap_or_else(|| vertical_amount_v120.unwrap_or(0.) / 120. * 80.);
                        if x != 0. || y != 0. {
                            ws.pan_canvas(Point::from((x, y)), source == AxisSource::Finger);
                            self.clarion.queue_redraw(&output);
                        }
                        return;
                    }
                }
            }
        }

        // Handle wheel scroll bindings.
        if source == AxisSource::Wheel {
            // If we have a scroll bind with current modifiers, then accumulate and don't pass to
            // Wayland. If there's no bind, reset the accumulator.
            let mods = self.clarion.seat.get_keyboard().unwrap().modifier_state();
            let modifiers = modifiers_from_state(mods);
            let should_handle = should_handle_in_overview
                || is_mru_open
                || self.clarion.mods_with_wheel_binds.contains(&modifiers);
            if should_handle {
                let horizontal = horizontal_amount_v120.unwrap_or(0.);
                let ticks = self.clarion.horizontal_wheel_tracker.accumulate(horizontal);
                if ticks != 0 {
                    let (bind_left, bind_right) = if should_handle_in_overview
                        && modifiers.is_empty()
                    {
                        let bind_left = Some(Bind {
                            key: Key {
                                trigger: Trigger::WheelScrollLeft,
                                modifiers: Modifiers::empty(),
                            },
                            action: Action::FocusColumnLeftUnderMouse,
                            repeat: true,
                            cooldown: None,
                            allow_when_locked: false,
                            allow_inhibiting: false,
                            hotkey_overlay_title: None,
                        });
                        let bind_right = Some(Bind {
                            key: Key {
                                trigger: Trigger::WheelScrollRight,
                                modifiers: Modifiers::empty(),
                            },
                            action: Action::FocusColumnRightUnderMouse,
                            repeat: true,
                            cooldown: None,
                            allow_when_locked: false,
                            allow_inhibiting: false,
                            hotkey_overlay_title: None,
                        });
                        (bind_left, bind_right)
                    } else {
                        let config = self.clarion.config.borrow();
                        let bindings =
                            make_binds_iter(&config, &mut self.clarion.window_mru_ui, modifiers);
                        let bind_left = find_configured_bind(
                            bindings.clone(),
                            mod_key,
                            Trigger::WheelScrollLeft,
                            mods,
                        )
                        .filter(|bind| {
                            !self.clarion.screenshot_ui.is_open()
                                || allowed_during_screenshot(&bind.action)
                        });
                        let bind_right = find_configured_bind(
                            bindings,
                            mod_key,
                            Trigger::WheelScrollRight,
                            mods,
                        )
                        .filter(|bind| {
                            !self.clarion.screenshot_ui.is_open()
                                || allowed_during_screenshot(&bind.action)
                        });
                        (bind_left, bind_right)
                    };

                    if let Some(right) = bind_right {
                        for _ in 0..ticks {
                            self.handle_bind(right.clone());
                        }
                    }
                    if let Some(left) = bind_left {
                        for _ in ticks..0 {
                            self.handle_bind(left.clone());
                        }
                    }
                }

                let vertical = vertical_amount_v120.unwrap_or(0.);
                let ticks = self.clarion.vertical_wheel_tracker.accumulate(vertical);
                if ticks != 0 {
                    let (bind_up, bind_down) = if should_handle_in_overview && modifiers.is_empty()
                    {
                        let bind_up = Some(Bind {
                            key: Key {
                                trigger: Trigger::WheelScrollUp,
                                modifiers: Modifiers::empty(),
                            },
                            action: Action::FocusWorkspaceUpUnderMouse,
                            repeat: true,
                            cooldown: Some(Duration::from_millis(50)),
                            allow_when_locked: false,
                            allow_inhibiting: false,
                            hotkey_overlay_title: None,
                        });
                        let bind_down = Some(Bind {
                            key: Key {
                                trigger: Trigger::WheelScrollDown,
                                modifiers: Modifiers::empty(),
                            },
                            action: Action::FocusWorkspaceDownUnderMouse,
                            repeat: true,
                            cooldown: Some(Duration::from_millis(50)),
                            allow_when_locked: false,
                            allow_inhibiting: false,
                            hotkey_overlay_title: None,
                        });
                        (bind_up, bind_down)
                    } else if should_handle_in_overview && modifiers == Modifiers::SHIFT {
                        let bind_up = Some(Bind {
                            key: Key {
                                trigger: Trigger::WheelScrollUp,
                                modifiers: Modifiers::empty(),
                            },
                            action: Action::FocusColumnLeftUnderMouse,
                            repeat: true,
                            cooldown: Some(Duration::from_millis(50)),
                            allow_when_locked: false,
                            allow_inhibiting: false,
                            hotkey_overlay_title: None,
                        });
                        let bind_down = Some(Bind {
                            key: Key {
                                trigger: Trigger::WheelScrollDown,
                                modifiers: Modifiers::empty(),
                            },
                            action: Action::FocusColumnRightUnderMouse,
                            repeat: true,
                            cooldown: Some(Duration::from_millis(50)),
                            allow_when_locked: false,
                            allow_inhibiting: false,
                            hotkey_overlay_title: None,
                        });
                        (bind_up, bind_down)
                    } else {
                        let config = self.clarion.config.borrow();
                        let bindings =
                            make_binds_iter(&config, &mut self.clarion.window_mru_ui, modifiers);
                        let bind_up = find_configured_bind(
                            bindings.clone(),
                            mod_key,
                            Trigger::WheelScrollUp,
                            mods,
                        )
                        .filter(|bind| {
                            !self.clarion.screenshot_ui.is_open()
                                || allowed_during_screenshot(&bind.action)
                        });
                        let bind_down =
                            find_configured_bind(bindings, mod_key, Trigger::WheelScrollDown, mods)
                                .filter(|bind| {
                                    !self.clarion.screenshot_ui.is_open()
                                        || allowed_during_screenshot(&bind.action)
                                });
                        (bind_up, bind_down)
                    };

                    if let Some(down) = bind_down {
                        for _ in 0..ticks {
                            self.handle_bind(down.clone());
                        }
                    }
                    if let Some(up) = bind_up {
                        for _ in ticks..0 {
                            self.handle_bind(up.clone());
                        }
                    }
                }

                return;
            } else {
                self.clarion.horizontal_wheel_tracker.reset();
                self.clarion.vertical_wheel_tracker.reset();
            }
        }

        let horizontal_amount = event.amount(Axis::Horizontal);
        let vertical_amount = event.amount(Axis::Vertical);

        // Handle touchpad and continuous scroll bindings.
        if source == AxisSource::Finger || source == AxisSource::Continuous {
            let mods = self.clarion.seat.get_keyboard().unwrap().modifier_state();
            let modifiers = modifiers_from_state(mods);

            let horizontal = horizontal_amount.unwrap_or(0.);
            let vertical = vertical_amount.unwrap_or(0.);

            if should_handle_in_overview && modifiers.is_empty() {
                let mut redraw = false;

                let action = self
                    .clarion
                    .overview_scroll_swipe_gesture
                    .update(horizontal, vertical);
                let is_vertical = self.clarion.overview_scroll_swipe_gesture.is_vertical();

                if action.end() {
                    if is_vertical {
                        redraw |= self
                            .clarion
                            .layout
                            .workspace_switch_gesture_end(Some(true))
                            .is_some();
                    } else {
                        redraw |= self
                            .clarion
                            .layout
                            .view_offset_gesture_end(Some(true))
                            .is_some();
                    }
                } else {
                    // Maybe begin, then update.
                    if is_vertical {
                        if action.begin() {
                            if let Some(output) = self.clarion.output_under_cursor() {
                                self.clarion
                                    .layout
                                    .workspace_switch_gesture_begin(&output, true);
                                redraw = true;
                            }
                        }

                        let res = self
                            .clarion
                            .layout
                            .workspace_switch_gesture_update(vertical, timestamp, true);
                        if let Some(Some(_)) = res {
                            redraw = true;
                        }
                    } else {
                        if action.begin() {
                            if let Some((output, ws)) = self.clarion.workspace_under_cursor(true) {
                                let ws_id = ws.id();
                                let ws_idx =
                                    self.clarion.layout.find_workspace_by_id(ws_id).unwrap().0;

                                self.clarion.layout.view_offset_gesture_begin(
                                    &output,
                                    Some(ws_idx),
                                    true,
                                );
                                redraw = true;
                            }
                        }

                        let res = self
                            .clarion
                            .layout
                            .view_offset_gesture_update(horizontal, timestamp, true);
                        if let Some(Some(_)) = res {
                            redraw = true;
                        }
                    }
                }

                if redraw {
                    self.clarion.queue_redraw_all();
                }

                return;
            } else {
                let mut redraw = false;
                if self.clarion.overview_scroll_swipe_gesture.reset() {
                    if self.clarion.overview_scroll_swipe_gesture.is_vertical() {
                        redraw |= self
                            .clarion
                            .layout
                            .workspace_switch_gesture_end(Some(true))
                            .is_some();
                    } else {
                        redraw |= self
                            .clarion
                            .layout
                            .view_offset_gesture_end(Some(true))
                            .is_some();
                    }
                }
                if redraw {
                    self.clarion.queue_redraw_all();
                }
            }

            if is_mru_open
                || self
                    .clarion
                    .mods_with_finger_scroll_binds
                    .contains(&modifiers)
            {
                let ticks = self
                    .clarion
                    .horizontal_finger_scroll_tracker
                    .accumulate(horizontal);
                if ticks != 0 {
                    let config = self.clarion.config.borrow();
                    let bindings =
                        make_binds_iter(&config, &mut self.clarion.window_mru_ui, modifiers);
                    let bind_left = find_configured_bind(
                        bindings.clone(),
                        mod_key,
                        Trigger::TouchpadScrollLeft,
                        mods,
                    )
                    .filter(|bind| {
                        !self.clarion.screenshot_ui.is_open()
                            || allowed_during_screenshot(&bind.action)
                    });
                    let bind_right =
                        find_configured_bind(bindings, mod_key, Trigger::TouchpadScrollRight, mods)
                            .filter(|bind| {
                                !self.clarion.screenshot_ui.is_open()
                                    || allowed_during_screenshot(&bind.action)
                            });
                    drop(config);

                    if let Some(right) = bind_right {
                        for _ in 0..ticks {
                            self.handle_bind(right.clone());
                        }
                    }
                    if let Some(left) = bind_left {
                        for _ in ticks..0 {
                            self.handle_bind(left.clone());
                        }
                    }
                }

                let ticks = self
                    .clarion
                    .vertical_finger_scroll_tracker
                    .accumulate(vertical);
                if ticks != 0 {
                    let config = self.clarion.config.borrow();
                    let bindings =
                        make_binds_iter(&config, &mut self.clarion.window_mru_ui, modifiers);
                    let bind_up = find_configured_bind(
                        bindings.clone(),
                        mod_key,
                        Trigger::TouchpadScrollUp,
                        mods,
                    )
                    .filter(|bind| {
                        !self.clarion.screenshot_ui.is_open()
                            || allowed_during_screenshot(&bind.action)
                    });
                    let bind_down =
                        find_configured_bind(bindings, mod_key, Trigger::TouchpadScrollDown, mods)
                            .filter(|bind| {
                                !self.clarion.screenshot_ui.is_open()
                                    || allowed_during_screenshot(&bind.action)
                            });
                    drop(config);

                    if let Some(down) = bind_down {
                        for _ in 0..ticks {
                            self.handle_bind(down.clone());
                        }
                    }
                    if let Some(up) = bind_up {
                        for _ in ticks..0 {
                            self.handle_bind(up.clone());
                        }
                    }
                }

                return;
            } else {
                self.clarion.horizontal_finger_scroll_tracker.reset();
                self.clarion.vertical_finger_scroll_tracker.reset();
            }
        }

        self.update_pointer_contents();

        let device_scroll_factor = {
            let config = self.clarion.config.borrow();
            match source {
                AxisSource::Wheel => config.input.mouse.scroll_factor,
                AxisSource::Finger => config.input.touchpad.scroll_factor,
                _ => None,
            }
        };

        // Get window-specific scroll factor
        let window_scroll_factor = pointer
            .current_focus()
            .map(|focused| self.clarion.find_root_shell_surface(&focused))
            .and_then(|root| self.clarion.layout.find_window_and_output(&root).unzip().0)
            .and_then(|window| window.rules().scroll_factor)
            .unwrap_or(1.);

        // Determine final scroll factors based on configuration
        let (horizontal_factor, vertical_factor) = device_scroll_factor
            .map(|x| x.h_v_factors())
            .unwrap_or((1.0, 1.0));
        let (horizontal_factor, vertical_factor) = (
            horizontal_factor * window_scroll_factor,
            vertical_factor * window_scroll_factor,
        );

        let horizontal_amount = horizontal_amount.unwrap_or_else(|| {
            // Winit backend, discrete scrolling.
            horizontal_amount_v120.unwrap_or(0.0) / 120. * 15.
        }) * horizontal_factor;

        let vertical_amount = vertical_amount.unwrap_or_else(|| {
            // Winit backend, discrete scrolling.
            vertical_amount_v120.unwrap_or(0.0) / 120. * 15.
        }) * vertical_factor;

        let horizontal_amount_v120 = horizontal_amount_v120.map(|x| x * horizontal_factor);
        let vertical_amount_v120 = vertical_amount_v120.map(|x| x * vertical_factor);

        let mut frame = AxisFrame::new(event.time()).source(source);
        if horizontal_amount != 0.0 {
            frame = frame
                .relative_direction(Axis::Horizontal, event.relative_direction(Axis::Horizontal));
            frame = frame.value(Axis::Horizontal, horizontal_amount);
            if let Some(v120) = horizontal_amount_v120 {
                frame = frame.v120(Axis::Horizontal, v120 as i32);
            }
        }
        if vertical_amount != 0.0 {
            frame =
                frame.relative_direction(Axis::Vertical, event.relative_direction(Axis::Vertical));
            frame = frame.value(Axis::Vertical, vertical_amount);
            if let Some(v120) = vertical_amount_v120 {
                frame = frame.v120(Axis::Vertical, v120 as i32);
            }
        }

        if source == AxisSource::Finger {
            if event.amount(Axis::Horizontal) == Some(0.0) {
                frame = frame.stop(Axis::Horizontal);
            }
            if event.amount(Axis::Vertical) == Some(0.0) {
                frame = frame.stop(Axis::Vertical);
            }
        }

        pointer.axis(self, frame);
        pointer.frame(self);
    }

    fn on_tablet_tool_axis<I: InputBackend>(&mut self, event: I::TabletToolAxisEvent)
    where
        I::Device: 'static, // Needed for downcasting.
    {
        let Some(pos) = self.compute_tablet_position(&event) else {
            return;
        };

        if let Some(output) = self.clarion.screenshot_ui.selection_output() {
            let Some(geom) = self.clarion.global_space.output_geometry(output) else {
                return;
            };
            let point = (pos - geom.loc.to_f64())
                .to_physical(output.current_scale().fractional_scale())
                .to_i32_round::<i32>();

            self.clarion.screenshot_ui.pointer_motion(point, None);
        }

        if let Some(mru_output) = self.clarion.window_mru_ui.output() {
            if let Some((output, pos_within_output)) = self.clarion.output_under(pos) {
                if mru_output == output {
                    self.clarion.window_mru_ui.pointer_motion(pos_within_output);
                }
            }
        }

        let under = self.clarion.contents_under(pos);

        let tablet_seat = self.clarion.seat.tablet_seat();
        let tool = tablet_seat.get_tool(&event.tool());
        if let Some(tool) = tool {
            let time = event.time();

            let frame = tablet::tool::AxisFrame {
                pressure: event.pressure_has_changed().then(|| event.pressure()),
                distance: event.distance_has_changed().then(|| event.distance()),
                tilt: event.tilt_has_changed().then(|| event.tilt()),
                rotation: event.rotation_has_changed().then(|| event.rotation()),
                slider: event.slider_has_changed().then(|| event.slider_position()),
                wheel: event
                    .wheel_has_changed()
                    .then(|| (event.wheel_delta(), event.wheel_delta_discrete())),
            };
            tool.axis(self, frame);

            tool.motion(
                self,
                under.surface,
                &tablet::tool::MotionEvent {
                    location: pos,
                    serial: SERIAL_COUNTER.next_serial(),
                    time,
                },
            );

            tool.frame(self, time);

            self.clarion.pointer_visibility = PointerVisibility::Visible;
            self.clarion.tablet_cursor_location = Some(pos);
        }

        // Redraw to update the cursor position.
        // FIXME: redraw only outputs overlapping the cursor.
        self.clarion.queue_redraw_all();
    }

    fn on_tablet_tool_tip<I: InputBackend>(&mut self, event: I::TabletToolTipEvent) {
        let tool = self.clarion.seat.tablet_seat().get_tool(&event.tool());

        let Some(tool) = tool else {
            return;
        };
        let tip_state = event.tip_state();

        let serial = SERIAL_COUNTER.next_serial();
        let time = event.time();

        match tip_state {
            TabletToolTipState::Down => {
                if let Some(pos) = self.clarion.tablet_cursor_location {
                    let under = self.clarion.contents_under(pos);

                    let mod_key = self.backend.mod_key(&self.clarion.config.borrow());
                    let mods = self.clarion.seat.get_keyboard().unwrap().modifier_state();
                    let modifiers = modifiers_from_state(mods);
                    let mod_down = modifiers.contains(mod_key.to_modifiers());

                    if self.clarion.screenshot_ui.is_open() {
                        // If we'll be moving the existing selection, use the selection output.
                        let output = if mod_down {
                            self.clarion.screenshot_ui.selection_output()
                        } else {
                            under.output.as_ref()
                        };

                        if let Some(output) = output.cloned() {
                            let Some(geom) = self.clarion.global_space.output_geometry(&output)
                            else {
                                return;
                            };
                            let point = (pos - geom.loc.to_f64())
                                .to_physical(output.current_scale().fractional_scale())
                                .to_i32_round();

                            if self
                                .clarion
                                .screenshot_ui
                                .pointer_down(output, point, None, mod_down)
                            {
                                self.clarion.queue_redraw_all();
                            }
                        }
                    } else if let Some(mru_output) = self.clarion.window_mru_ui.output() {
                        if let Some((output, pos_within_output)) = self.clarion.output_under(pos) {
                            if mru_output == output {
                                let id =
                                    self.clarion.window_mru_ui.pointer_motion(pos_within_output);
                                if id.is_some() {
                                    self.confirm_mru();
                                } else {
                                    self.clarion.cancel_mru();
                                }
                            } else {
                                self.clarion.cancel_mru();
                            }
                        }
                    } else if !tool.is_grabbed() {
                        if self.clarion.layout.is_overview_open()
                            && !mod_down
                            && under.layer.is_none()
                            && under.output.is_some()
                        {
                            if self.clarion.overview_mode_transition_active() {
                                return;
                            }
                            let Some((output, pos_within_output)) = self.clarion.output_under(pos)
                            else {
                                return;
                            };
                            let output = output.clone();

                            let mut matched_narrow = true;
                            let mut ws = self.clarion.workspace_under(false, pos);
                            if ws.is_none() {
                                matched_narrow = false;
                                ws = self.clarion.workspace_under(true, pos);
                            }
                            let ws_id = ws.map(|(_, ws)| ws.id());

                            let mapped = self.clarion.window_under(pos);
                            let window = mapped.map(|mapped| mapped.window.clone());

                            let start_data = TabletToolGrabStartData {
                                focus: None,
                                trigger: tablet::tool::GrabTrigger::Tip,
                                location: pos,
                            };
                            let start_data = AnyStartData::TabletTool(start_data);
                            let start_timestamp = Duration::from_micros(event.time().micros());
                            let grab = TouchOverviewGrab::new(
                                start_data,
                                start_timestamp,
                                output,
                                pos_within_output,
                                ws_id,
                                matched_narrow,
                                window,
                                self.clarion.layout.overview_mode(),
                            );
                            tool.set_grab(self, grab, time, serial, Focus::Clear);
                        } else if let Some((window, _)) = under.window {
                            self.clarion.layout.activate_window(&window);

                            // Check if we need to start a tablet tool move grab.
                            if mod_down {
                                let start_data = TabletToolGrabStartData {
                                    focus: None,
                                    trigger: tablet::tool::GrabTrigger::Tip,
                                    location: pos,
                                };
                                let start_data = AnyStartData::TabletTool(start_data);
                                let icon = CursorIcon::Grabbing;
                                if let Some(grab) = MoveGrab::new(
                                    self,
                                    start_data,
                                    window.clone(),
                                    true,
                                    Some(icon),
                                ) {
                                    tool.set_grab(self, grab, time, serial, Focus::Clear);
                                }
                            }

                            // FIXME: granular.
                            self.clarion.queue_redraw_all();
                        } else if let Some(output) = under.output {
                            self.clarion.layout.focus_output(&output);

                            // FIXME: granular.
                            self.clarion.queue_redraw_all();
                        }
                        self.clarion.focus_layer_surface_if_on_demand(under.layer);
                    }
                }

                tool.down(self, &tablet::tool::DownEvent { serial, time });
            }
            TabletToolTipState::Up => {
                if let Some(capture) = self.clarion.screenshot_ui.pointer_up(None) {
                    if capture {
                        self.confirm_screenshot(true);
                    } else {
                        self.clarion.queue_redraw_all();
                    }
                }

                tool.up(self, &tablet::tool::UpEvent { serial, time });
            }
        }

        tool.frame(self, time);
    }

    fn on_tablet_tool_proximity<I: InputBackend>(&mut self, event: I::TabletToolProximityEvent)
    where
        I::Device: 'static, // Needed for downcasting.
    {
        let Some(pos) = self.compute_tablet_position(&event) else {
            return;
        };

        let under = self.clarion.contents_under(pos);

        let tablet_seat = self.clarion.seat.tablet_seat();
        let display_handle = self.clarion.display_handle.clone();
        let tool = tablet_seat
            .get_tool(&event.tool())
            .unwrap_or_else(|| tablet_seat.add_wp_tool(self, &display_handle, &event.tool()));
        let tablet = tablet_seat.get_tablet(&TabletDescriptor::from(&event.device()));
        if let Some(tablet) = tablet {
            let serial = SERIAL_COUNTER.next_serial();
            let time = event.time();

            match event.state() {
                ProximityState::In => {
                    let frame = tablet::tool::AxisFrame {
                        pressure: event.pressure_has_changed().then(|| event.pressure()),
                        distance: event.distance_has_changed().then(|| event.distance()),
                        tilt: event.tilt_has_changed().then(|| event.tilt()),
                        rotation: event.rotation_has_changed().then(|| event.rotation()),
                        slider: event.slider_has_changed().then(|| event.slider_position()),
                        wheel: event
                            .wheel_has_changed()
                            .then(|| (event.wheel_delta(), event.wheel_delta_discrete())),
                    };

                    tool.proximity_in(
                        self,
                        under.surface,
                        tablet,
                        &tablet::tool::ProximityInEvent {
                            location: pos,
                            axis: Some(frame),
                            serial,
                            time,
                        },
                    );

                    // Is proximity in usually immediatelly followed by other events like button? If
                    // so, then it might be worth delaying this frame() until the loop callback to
                    // batch all of them in.
                    tool.frame(self, time);

                    self.clarion.pointer_visibility = PointerVisibility::Visible;
                    self.clarion.tablet_cursor_location = Some(pos);
                }
                ProximityState::Out => {
                    tool.proximity_out(self, &tablet::tool::ProximityOutEvent { serial, time });
                    tool.frame(self, time);

                    // Move the mouse pointer here to avoid discontinuity.
                    //
                    // Plus, Wayland SDL2 currently warps the pointer into some weird
                    // location on proximity out, so this should help it a little.
                    if let Some(pos) = self.clarion.tablet_cursor_location {
                        self.move_cursor(pos);
                    }

                    self.clarion.pointer_visibility = PointerVisibility::Visible;
                    self.clarion.tablet_cursor_location = None;
                }
            }

            // FIXME: granular.
            self.clarion.queue_redraw_all();
        }
    }

    fn on_tablet_tool_button<I: InputBackend>(&mut self, event: I::TabletToolButtonEvent) {
        const BTN_STYLUS3: u32 = 0x149;
        const BTN_STYLUS: u32 = 0x14b;
        const BTN_STYLUS2: u32 = 0x14c;

        let tool = self.clarion.seat.tablet_seat().get_tool(&event.tool());

        if let Some(tool) = tool {
            let button = event.button();

            if self.clarion.suppressed_buttons.remove(&button) {
                return;
            }

            let trigger = match button {
                BTN_STYLUS => Some(Trigger::TabletStylusButton1),
                BTN_STYLUS2 => Some(Trigger::TabletStylusButton2),
                BTN_STYLUS3 => Some(Trigger::TabletStylusButton3),
                _ => None,
            };

            if let Some(trigger) = trigger {
                if event.button_state() == ButtonState::Pressed {
                    let mod_key = self.backend.mod_key(&self.clarion.config.borrow());
                    let mods = self.clarion.seat.get_keyboard().unwrap().modifier_state();
                    let modifiers = modifiers_from_state(mods);

                    if self
                        .clarion
                        .mods_with_tablet_stylus_binds
                        .contains(&modifiers)
                    {
                        let bind = {
                            let config = self.clarion.config.borrow();
                            let bindings = config.binds.0.iter();
                            find_configured_bind(bindings, mod_key, trigger, mods)
                        }
                        .filter(|bind| {
                            !self.clarion.screenshot_ui.is_open()
                                || allowed_during_screenshot(&bind.action)
                        });
                        if let Some(bind) = bind {
                            self.clarion.suppressed_buttons.insert(button);
                            self.handle_bind(bind.clone());
                            return;
                        }
                    }
                }
            }

            let time = event.time();

            tool.button(
                self,
                &tablet::tool::ButtonEvent {
                    serial: SERIAL_COUNTER.next_serial(),
                    button,
                    state: event.button_state(),
                    time,
                },
            );

            tool.frame(self, time);
        }
    }

    fn on_gesture_swipe_begin<I: InputBackend>(&mut self, event: I::GestureSwipeBeginEvent) {
        if self.clarion.window_mru_ui.is_open()
            || self.clarion.screenshot_ui.is_open()
            || self.clarion.is_locked()
            || self.clarion.exit_confirm_dialog.is_open()
            || self.clarion.overview_mode_transition_active()
            || self
                .clarion
                .seat
                .get_pointer()
                .is_some_and(|pointer| pointer.is_grabbed())
        {
            // Don't start swipe gestures while a compositor overlay owns the session.
            return;
        }

        if event.fingers() == 3 {
            self.clarion.gesture_swipe_3f_cumulative = Some((0., 0.));

            // We handled this event.
            return;
        } else if event.fingers() == 4 {
            self.overview_command(OverviewCommand::GestureBegin);
            self.clarion.queue_redraw_all();

            // We handled this event.
            return;
        }

        let serial = SERIAL_COUNTER.next_serial();
        let pointer = self.clarion.seat.get_pointer().unwrap();

        if self.update_pointer_contents() {
            pointer.frame(self);
        }

        pointer.gesture_swipe_begin(
            self,
            &GestureSwipeBeginEvent {
                serial,
                time: event.time(),
                fingers: event.fingers(),
            },
        );
    }

    fn on_gesture_swipe_update<I: InputBackend + 'static>(
        &mut self,
        event: I::GestureSwipeUpdateEvent,
    ) where
        I::Device: 'static,
    {
        if self.clarion.window_mru_ui.is_open()
            || self.clarion.screenshot_ui.is_open()
            || self.clarion.is_locked()
            || self.clarion.exit_confirm_dialog.is_open()
            || self.clarion.overview_mode_transition_active()
        {
            self.clarion.gesture_swipe_3f_cumulative = None;
            self.clarion
                .layout
                .workspace_switch_gesture_cancel(Some(true));
            self.clarion.layout.view_offset_gesture_cancel(Some(true));
            self.overview_command(OverviewCommand::GestureEnd { cancelled: true });
            self.clarion.queue_redraw_all();
            return;
        }
        let mut delta_x = event.delta_x();
        let mut delta_y = event.delta_y();

        if let Some(libinput_event) =
            (&event as &dyn Any).downcast_ref::<input::event::gesture::GestureSwipeUpdateEvent>()
        {
            delta_x = libinput_event.dx_unaccelerated();
            delta_y = libinput_event.dy_unaccelerated();
        }

        let uninverted_delta_y = delta_y;

        let device = event.device();
        if let Some(device) = (&device as &dyn Any).downcast_ref::<input::Device>() {
            if device.config_scroll_natural_scroll_enabled() {
                delta_x = -delta_x;
                delta_y = -delta_y;
            }
        }

        let is_overview_open = self.clarion.layout.is_overview_open();

        if let Some((cx, cy)) = &mut self.clarion.gesture_swipe_3f_cumulative {
            if self.clarion.layout.is_stage_open() {
                *cx += delta_x;
                *cy += delta_y;
                let steps = (-*cx / 64.).trunc() as isize;
                if steps != 0 {
                    *cx %= 64.;
                    if let Some(output) = self.clarion.output_under_cursor() {
                        self.clarion.layout.stage_navigate(Some(&output), steps);
                        self.clarion.queue_redraw(&output);
                    }
                }
                return;
            }
            *cx += delta_x;
            *cy += delta_y;

            // Check if the gesture moved far enough to decide. Threshold copied from GNOME Shell.
            let (cx, cy) = (*cx, *cy);
            if cx * cx + cy * cy >= 16. * 16. {
                self.clarion.gesture_swipe_3f_cumulative = None;

                if let Some(output) = self.clarion.output_under_cursor() {
                    if cx.abs() > cy.abs() {
                        let output_ws = if is_overview_open {
                            self.clarion.workspace_under_cursor(true)
                        } else {
                            // We don't want to accidentally "catch" the wrong workspace during
                            // animations.
                            self.clarion.output_under_cursor().and_then(|output| {
                                let mon = self.clarion.layout.monitor_for_output(&output)?;
                                Some((output, mon.active_workspace_ref()))
                            })
                        };

                        if let Some((output, ws)) = output_ws {
                            let ws_idx =
                                self.clarion.layout.find_workspace_by_id(ws.id()).unwrap().0;
                            self.clarion.layout.view_offset_gesture_begin(
                                &output,
                                Some(ws_idx),
                                true,
                            );
                        }
                    } else {
                        self.clarion
                            .layout
                            .workspace_switch_gesture_begin(&output, true);
                    }
                }
            }
        }

        let timestamp = Duration::from_micros(event.time().micros());

        let mut handled = false;
        let res = self
            .clarion
            .layout
            .workspace_switch_gesture_update(delta_y, timestamp, true);
        if let Some(output) = res {
            if let Some(output) = output {
                self.clarion.queue_redraw(&output);
            }
            handled = true;
        }

        let res = self
            .clarion
            .layout
            .view_offset_gesture_update(delta_x, timestamp, true);
        if let Some(output) = res {
            if let Some(output) = output {
                self.clarion.queue_redraw(&output);
            }
            handled = true;
        }

        if self.overview_command(OverviewCommand::GestureUpdate {
            delta: -uninverted_delta_y,
            timestamp,
        }) {
            handled = true;
        }

        if handled {
            // We handled this event.
            return;
        }

        let pointer = self.clarion.seat.get_pointer().unwrap();

        if self.update_pointer_contents() {
            pointer.frame(self);
        }

        pointer.gesture_swipe_update(
            self,
            &GestureSwipeUpdateEvent {
                time: event.time(),
                delta: event.delta(),
            },
        );
    }

    fn on_gesture_swipe_end<I: InputBackend>(&mut self, event: I::GestureSwipeEndEvent) {
        let mut handled = self.clarion.gesture_swipe_3f_cumulative.take().is_some();
        let cancelled = event.cancelled()
            || self.clarion.is_locked()
            || self.clarion.screenshot_ui.is_open()
            || self.clarion.window_mru_ui.is_open()
            || self.clarion.exit_confirm_dialog.is_open()
            || self.clarion.overview_mode_transition_active();
        let res = if cancelled {
            self.clarion
                .layout
                .workspace_switch_gesture_cancel(Some(true))
        } else {
            self.clarion.layout.workspace_switch_gesture_end(Some(true))
        };
        if let Some(output) = res {
            self.clarion.queue_redraw(&output);
            handled = true;
        }

        let res = if cancelled {
            self.clarion.layout.view_offset_gesture_cancel(Some(true))
        } else {
            self.clarion.layout.view_offset_gesture_end(Some(true))
        };
        if let Some(output) = res {
            self.clarion.queue_redraw(&output);
            handled = true;
        }

        if self.overview_command(OverviewCommand::GestureEnd { cancelled }) {
            handled = true;
        }

        if handled {
            // We handled this event.
            return;
        }

        let serial = SERIAL_COUNTER.next_serial();
        let pointer = self.clarion.seat.get_pointer().unwrap();

        if self.update_pointer_contents() {
            pointer.frame(self);
        }

        pointer.gesture_swipe_end(
            self,
            &GestureSwipeEndEvent {
                serial,
                time: event.time(),
                cancelled: event.cancelled(),
            },
        );
    }

    fn on_gesture_pinch_begin<I: InputBackend>(&mut self, event: I::GesturePinchBeginEvent) {
        self.clarion.launcher_pinch = launcher_gesture::LauncherPinch::begin(
            event.fingers(),
            self.shell_launcher_available(),
        );
        if self.clarion.launcher_pinch.is_some() {
            return;
        }
        let serial = SERIAL_COUNTER.next_serial();
        let pointer = self.clarion.seat.get_pointer().unwrap();

        if self.update_pointer_contents() {
            pointer.frame(self);
        }

        pointer.gesture_pinch_begin(
            self,
            &GesturePinchBeginEvent {
                serial,
                time: event.time(),
                fingers: event.fingers(),
            },
        );
    }

    fn on_gesture_pinch_update<I: InputBackend>(&mut self, event: I::GesturePinchUpdateEvent) {
        let allowed = self.shell_launcher_available();
        if let Some(pinch) = &mut self.clarion.launcher_pinch {
            pinch.update(event.scale(), allowed);
            return;
        }
        let pointer = self.clarion.seat.get_pointer().unwrap();

        if self.update_pointer_contents() {
            pointer.frame(self);
        }

        pointer.gesture_pinch_update(
            self,
            &GesturePinchUpdateEvent {
                time: event.time(),
                delta: event.delta(),
                scale: event.scale(),
                rotation: event.rotation(),
            },
        );
    }

    fn on_gesture_pinch_end<I: InputBackend>(&mut self, event: I::GesturePinchEndEvent) {
        if let Some(pinch) = self.clarion.launcher_pinch.take() {
            if pinch.finish(event.cancelled(), self.shell_launcher_available()) {
                self.do_action(
                    Action::Spawn(vec!["clarion-shell-msg".into(), "launcher".into()]),
                    false,
                );
            }
            return;
        }
        let serial = SERIAL_COUNTER.next_serial();
        let pointer = self.clarion.seat.get_pointer().unwrap();

        if self.update_pointer_contents() {
            pointer.frame(self);
        }

        pointer.gesture_pinch_end(
            self,
            &GesturePinchEndEvent {
                serial,
                time: event.time(),
                cancelled: event.cancelled(),
            },
        );
    }

    fn on_gesture_hold_begin<I: InputBackend>(&mut self, event: I::GestureHoldBeginEvent) {
        let serial = SERIAL_COUNTER.next_serial();
        let pointer = self.clarion.seat.get_pointer().unwrap();

        if self.update_pointer_contents() {
            pointer.frame(self);
        }

        pointer.gesture_hold_begin(
            self,
            &GestureHoldBeginEvent {
                serial,
                time: event.time(),
                fingers: event.fingers(),
            },
        );
    }

    fn on_gesture_hold_end<I: InputBackend>(&mut self, event: I::GestureHoldEndEvent) {
        let serial = SERIAL_COUNTER.next_serial();
        let pointer = self.clarion.seat.get_pointer().unwrap();

        if self.update_pointer_contents() {
            pointer.frame(self);
        }

        pointer.gesture_hold_end(
            self,
            &GestureHoldEndEvent {
                serial,
                time: event.time(),
                cancelled: event.cancelled(),
            },
        );
    }

    fn compute_absolute_location<I: InputBackend>(
        &self,
        evt: &impl AbsolutePositionEvent<I>,
        fallback_output: Option<&Output>,
    ) -> Option<Point<f64, Logical>> {
        let output = evt.device().output(self);
        let output = output.filter(|output| self.clarion.output_exists(output));
        let output = output.as_ref().or(fallback_output)?;
        let output_geo = self.clarion.global_space.output_geometry(output)?;
        let transform = output.current_transform();
        let size = transform.invert().transform_size(output_geo.size);
        Some(
            transform.transform_point_in(evt.position_transformed(size), &size.to_f64())
                + output_geo.loc.to_f64(),
        )
    }

    /// Computes the cursor position for the touch event.
    ///
    /// This function handles the touch output mapping, as well as coordinate transform
    fn compute_touch_location<I: InputBackend>(
        &self,
        evt: &impl AbsolutePositionEvent<I>,
    ) -> Option<Point<f64, Logical>> {
        self.compute_absolute_location(evt, self.clarion.output_for_touch())
    }

    fn on_touch_down<I: InputBackend>(&mut self, evt: I::TouchDownEvent) {
        let Some(handle) = self.clarion.seat.get_touch() else {
            return;
        };
        let Some(pos) = self.compute_touch_location(&evt) else {
            return;
        };
        let slot = evt.slot();

        let serial = SERIAL_COUNTER.next_serial();

        let under = self.clarion.contents_under(pos);

        let mod_key = self.backend.mod_key(&self.clarion.config.borrow());
        let mods = self.clarion.seat.get_keyboard().unwrap().modifier_state();
        let mods = modifiers_from_state(mods);
        let mod_down = mods.contains(mod_key.to_modifiers());

        if self.clarion.screenshot_ui.is_open() {
            // If we'll be moving the existing selection, use the selection output.
            let output = if mod_down {
                self.clarion.screenshot_ui.selection_output()
            } else {
                under.output.as_ref()
            };

            if let Some(output) = output.cloned() {
                let Some(geom) = self.clarion.global_space.output_geometry(&output) else {
                    return;
                };
                let point = (pos - geom.loc.to_f64())
                    .to_physical(output.current_scale().fractional_scale())
                    .to_i32_round();

                if self
                    .clarion
                    .screenshot_ui
                    .pointer_down(output, point, Some(slot), mod_down)
                {
                    self.clarion.queue_redraw_all();
                }
            }
        } else if let Some(mru_output) = self.clarion.window_mru_ui.output() {
            if let Some((output, pos_within_output)) = self.clarion.output_under(pos) {
                if mru_output == output {
                    let id = self.clarion.window_mru_ui.pointer_motion(pos_within_output);
                    if id.is_some() {
                        self.confirm_mru();
                    } else {
                        self.clarion.cancel_mru();
                    }
                } else {
                    self.clarion.cancel_mru();
                }
            }
        } else if !handle.is_grabbed() {
            if self.clarion.layout.is_overview_open()
                && !mod_down
                && under.layer.is_none()
                && under.output.is_some()
            {
                if self.clarion.overview_mode_transition_active() {
                    return;
                }
                let Some((output, pos_within_output)) = self.clarion.output_under(pos) else {
                    return;
                };
                let output = output.clone();

                let mut matched_narrow = true;
                let mut ws = self.clarion.workspace_under(false, pos);
                if ws.is_none() {
                    matched_narrow = false;
                    ws = self.clarion.workspace_under(true, pos);
                }
                let ws_id = ws.map(|(_, ws)| ws.id());

                let mapped = self.clarion.window_under(pos);
                let window = mapped.map(|mapped| mapped.window.clone());

                let start_data = TouchGrabStartData {
                    focus: None,
                    slot,
                    location: pos,
                };
                let start_data = AnyStartData::Touch(start_data);
                let start_timestamp = Duration::from_micros(evt.time().micros());
                let grab = TouchOverviewGrab::new(
                    start_data,
                    start_timestamp,
                    output,
                    pos_within_output,
                    ws_id,
                    matched_narrow,
                    window,
                    self.clarion.layout.overview_mode(),
                );
                handle.set_grab(self, grab, serial);
            } else if let Some((window, _)) = under.window {
                self.clarion.layout.activate_window(&window);

                // Check if we need to start a touch move grab.
                if mod_down {
                    let start_data = TouchGrabStartData {
                        focus: None,
                        slot,
                        location: pos,
                    };
                    let start_data = AnyStartData::Touch(start_data);
                    if let Some(grab) = MoveGrab::new(self, start_data, window.clone(), true, None)
                    {
                        handle.set_grab(self, grab, serial);
                    }
                }

                // FIXME: granular.
                self.clarion.queue_redraw_all();
            } else if let Some(output) = under.output {
                self.clarion.layout.focus_output(&output);

                // FIXME: granular.
                self.clarion.queue_redraw_all();
            }
            self.clarion.focus_layer_surface_if_on_demand(under.layer);
        };

        handle.down(
            self,
            under.surface,
            &DownEvent {
                slot,
                location: pos,
                serial,
                time: evt.time(),
            },
        );

        // We're using touch, hide the pointer.
        self.clarion.pointer_visibility = PointerVisibility::Disabled;
    }
    fn on_touch_up<I: InputBackend>(&mut self, evt: I::TouchUpEvent) {
        let Some(handle) = self.clarion.seat.get_touch() else {
            return;
        };
        let slot = evt.slot();

        if let Some(capture) = self.clarion.screenshot_ui.pointer_up(Some(slot)) {
            if capture {
                self.confirm_screenshot(true);
            } else {
                self.clarion.queue_redraw_all();
            }
        }

        let serial = SERIAL_COUNTER.next_serial();
        handle.up(
            self,
            &UpEvent {
                slot,
                serial,
                time: evt.time(),
            },
        )
    }
    fn on_touch_motion<I: InputBackend>(&mut self, evt: I::TouchMotionEvent) {
        let Some(handle) = self.clarion.seat.get_touch() else {
            return;
        };
        let Some(pos) = self.compute_touch_location(&evt) else {
            return;
        };
        let slot = evt.slot();

        if let Some(output) = self.clarion.screenshot_ui.selection_output().cloned() {
            let Some(geom) = self.clarion.global_space.output_geometry(&output) else {
                return;
            };
            let point = (pos - geom.loc.to_f64())
                .to_physical(output.current_scale().fractional_scale())
                .to_i32_round::<i32>();

            self.clarion.screenshot_ui.pointer_motion(point, Some(slot));
            self.clarion.queue_redraw(&output);
        }

        let under = self.clarion.contents_under(pos);
        handle.motion(
            self,
            under.surface,
            &TouchMotionEvent {
                slot,
                location: pos,
                time: evt.time(),
            },
        );

        // Inform the layout of an ongoing DnD operation.
        let is_dnd_grab = handle
            .with_grab(|_, grab| Self::is_dnd_grab(grab.as_any()))
            .unwrap_or(false);
        if is_dnd_grab {
            if let Some((output, pos_within_output)) = self.clarion.output_under(pos) {
                let output = output.clone();
                self.clarion.layout.dnd_update(output, pos_within_output);
            }
        }
    }
    fn on_touch_frame<I: InputBackend>(&mut self, _evt: I::TouchFrameEvent) {
        let Some(handle) = self.clarion.seat.get_touch() else {
            return;
        };
        handle.frame(self);
    }
    fn on_touch_cancel<I: InputBackend>(&mut self, _evt: I::TouchCancelEvent) {
        let Some(handle) = self.clarion.seat.get_touch() else {
            return;
        };
        handle.cancel(self);
    }

    fn on_switch_toggle<I: InputBackend>(&mut self, evt: I::SwitchToggleEvent) {
        let Some(switch) = evt.switch() else {
            return;
        };

        if switch == Switch::Lid {
            let is_closed = evt.state() == SwitchState::On;
            trace!("lid switch {}", if is_closed { "closed" } else { "opened" });
            self.set_lid_closed(is_closed);
        }

        let action = {
            let bindings = &self.clarion.config.borrow().switch_events;
            find_configured_switch_action(bindings, switch, evt.state())
        };

        if let Some(action) = action {
            self.do_action(action, true);
        }
    }

    pub fn is_dnd_grab(grab: &dyn Any) -> bool {
        // Normal DnD
        grab.is::<DnDGrab<Self, WlDataSource, WlSurface>>()
            // Null-source DnD: weston-dnd --self-only
            || grab.is::<DnDGrab<Self, WlSurface, WlSurface>>()
    }

    fn grab_can_be_cancelled_with_esc(grab: &(dyn PointerGrab<State> + 'static)) -> bool {
        let grab = grab.as_any();

        grab.is::<PickWindowGrab>() || grab.is::<PickColorGrab>() || Self::is_dnd_grab(grab)
    }
}

/// Overlay navigation follows the same press/release suppression contract as configured binds.
#[allow(clippy::too_many_arguments)]
fn intercept_hotkey_overlay_key(
    overlay: &mut HotkeyOverlay,
    suppressed_keys: &mut HashSet<Keycode>,
    key_code: Keycode,
    keysym: Keysym,
    pressed: bool,
    modifiers: Modifiers,
    allowed: bool,
) -> bool {
    // Leave releases to should_intercept_key, after screenshot key state is updated.
    if !pressed {
        return false;
    }
    if allowed && overlay.handle_key(keysym, modifiers) {
        suppressed_keys.insert(key_code);
        true
    } else {
        false
    }
}

fn launcher_key_bind(raw: Option<Keysym>, modifiers: Modifiers) -> Option<Bind> {
    (raw == Some(Keysym::F4) && modifiers.is_empty()).then(|| Bind {
        key: Key {
            trigger: Trigger::Keysym(Keysym::F4),
            modifiers: Modifiers::empty(),
        },
        action: Action::Spawn(vec!["clarion-shell-msg".into(), "launcher".into()]),
        repeat: false,
        cooldown: None,
        allow_when_locked: false,
        allow_inhibiting: true,
        hotkey_overlay_title: Some(Some("Apps".into())),
    })
}

fn is_macos_desktop_shortcut(raw: Option<Keysym>, mods: ModifiersState) -> bool {
    let modifiers = modifiers_from_state(mods);
    match raw {
        Some(
            Keysym::a
            | Keysym::A
            | Keysym::i
            | Keysym::I
            | Keysym::o
            | Keysym::O
            | Keysym::y
            | Keysym::Y
            | Keysym::Down
            | Keysym::BackSpace,
        ) => modifiers == Modifiers::SUPER,
        Some(Keysym::n | Keysym::N) => modifiers == (Modifiers::SUPER | Modifiers::SHIFT),
        _ => false,
    }
}

/// Check whether the key should be intercepted and mark intercepted
/// pressed keys as `suppressed`, thus preventing `releases` corresponding
/// to them from being delivered.
#[allow(clippy::too_many_arguments)]
fn should_intercept_key<'a>(
    suppressed_keys: &mut HashSet<Keycode>,
    bindings: impl IntoIterator<Item = &'a Bind>,
    mod_key: ModKey,
    key_code: Keycode,
    modified: Keysym,
    raw: Option<Keysym>,
    pressed: bool,
    mods: ModifiersState,
    screenshot_ui: &ScreenshotUi,
    disable_power_key_handling: bool,
    is_inhibiting_shortcuts: bool,
) -> FilterResult<Option<Bind>> {
    // Actions are only triggered on presses, release of the key
    // shouldn't try to intercept anything unless we have marked
    // the key to suppress.
    if !pressed && !suppressed_keys.contains(&key_code) {
        return FilterResult::Forward;
    }

    let mut final_bind = find_bind(
        bindings,
        mod_key,
        modified,
        raw,
        mods,
        disable_power_key_handling,
    );

    // Allow only a subset of compositor actions while the screenshot UI is open, since the user
    // cannot see the screen.
    if screenshot_ui.is_open() {
        let mut use_screenshot_ui_action = true;

        if let Some(bind) = &final_bind {
            if allowed_during_screenshot(&bind.action) {
                use_screenshot_ui_action = false;
            }
        }

        if use_screenshot_ui_action {
            if let Some(raw) = raw {
                final_bind = screenshot_ui.action(raw, mods).map(|action| Bind {
                    key: Key {
                        trigger: Trigger::Keysym(raw),
                        // Not entirely correct but it doesn't matter in how we currently use it.
                        modifiers: Modifiers::empty(),
                    },
                    action,
                    repeat: true,
                    cooldown: None,
                    allow_when_locked: false,
                    // The screenshot UI owns the focus anyway, so this doesn't really matter.
                    // But logically, nothing can inhibit its actions. Only opening it can be
                    // inhibited.
                    allow_inhibiting: false,
                    hotkey_overlay_title: None,
                });
            }
        }
    }

    match (final_bind, pressed) {
        (Some(bind), true) => {
            if is_inhibiting_shortcuts && bind.allow_inhibiting {
                FilterResult::Forward
            } else {
                suppressed_keys.insert(key_code);
                FilterResult::Intercept(Some(bind))
            }
        }
        (_, false) => {
            // By this point, we know that the key was suppressed on press. Even if we're inhibiting
            // shortcuts, we should still suppress the release.
            // But we don't need to check for shortcuts inhibition here, because
            // if it was inhibited on press (forwarded to the client), it wouldn't be suppressed,
            // so the release would already have been forwarded at the start of this function.
            suppressed_keys.remove(&key_code);
            FilterResult::Intercept(None)
        }
        (None, true) => FilterResult::Forward,
    }
}

fn find_bind<'a>(
    bindings: impl IntoIterator<Item = &'a Bind>,
    mod_key: ModKey,
    modified: Keysym,
    raw: Option<Keysym>,
    mods: ModifiersState,
    disable_power_key_handling: bool,
) -> Option<Bind> {
    use keysyms::*;

    // Handle hardcoded binds.
    #[allow(non_upper_case_globals)] // wat
    let hardcoded_action = match modified.raw() {
        modified @ KEY_XF86Switch_VT_1..=KEY_XF86Switch_VT_12 => {
            let vt = (modified - KEY_XF86Switch_VT_1 + 1) as i32;
            Some(Action::ChangeVt(vt))
        }
        KEY_XF86PowerOff if !disable_power_key_handling => Some(Action::Suspend),
        _ => None,
    };

    if let Some(action) = hardcoded_action {
        return Some(Bind {
            key: Key {
                // Not entirely correct but it doesn't matter in how we currently use it.
                trigger: Trigger::Keysym(modified),
                modifiers: Modifiers::empty(),
            },
            action,
            repeat: true,
            cooldown: None,
            allow_when_locked: false,
            // In a worst-case scenario, the user has no way to unlock the compositor and a
            // misbehaving client has a keyboard shortcuts inhibitor, "jailing" the user.
            // The user must always be able to change VTs to recover from such a situation.
            // It also makes no sense to inhibit the default power key handling.
            // Hardcoded binds must never be inhibited.
            allow_inhibiting: false,
            hotkey_overlay_title: None,
        });
    }

    let trigger = Trigger::Keysym(raw?);
    find_configured_bind(bindings, mod_key, trigger, mods)
}

fn find_configured_bind<'a>(
    bindings: impl IntoIterator<Item = &'a Bind>,
    mod_key: ModKey,
    trigger: Trigger,
    mods: ModifiersState,
) -> Option<Bind> {
    // Handle configured binds.
    let mut modifiers = modifiers_from_state(mods);

    let mod_down = modifiers_from_state(mods).contains(mod_key.to_modifiers());
    if mod_down {
        modifiers |= Modifiers::COMPOSITOR;
    }

    for bind in bindings {
        if bind.key.trigger != trigger {
            continue;
        }

        let mut bind_modifiers = bind.key.modifiers;
        if bind_modifiers.contains(Modifiers::COMPOSITOR) {
            bind_modifiers |= mod_key.to_modifiers();
        } else if bind_modifiers.contains(mod_key.to_modifiers()) {
            bind_modifiers |= Modifiers::COMPOSITOR;
        }

        if bind_modifiers == modifiers {
            return Some(bind.clone());
        }
    }

    None
}

fn find_configured_switch_action(
    bindings: &SwitchBinds,
    switch: Switch,
    state: SwitchState,
) -> Option<Action> {
    let switch_action = match (switch, state) {
        (Switch::Lid, SwitchState::Off) => &bindings.lid_open,
        (Switch::Lid, SwitchState::On) => &bindings.lid_close,
        (Switch::TabletMode, SwitchState::Off) => &bindings.tablet_mode_off,
        (Switch::TabletMode, SwitchState::On) => &bindings.tablet_mode_on,
        _ => unreachable!(),
    };
    switch_action
        .as_ref()
        .map(|switch_action| Action::Spawn(switch_action.spawn.clone()))
}

fn modifiers_from_state(mods: ModifiersState) -> Modifiers {
    let mut modifiers = Modifiers::empty();
    if mods.ctrl {
        modifiers |= Modifiers::CTRL;
    }
    if mods.shift {
        modifiers |= Modifiers::SHIFT;
    }
    if mods.alt {
        modifiers |= Modifiers::ALT;
    }
    if mods.logo {
        modifiers |= Modifiers::SUPER;
    }
    if mods.iso_level3_shift {
        modifiers |= Modifiers::ISO_LEVEL3_SHIFT;
    }
    if mods.iso_level5_shift {
        modifiers |= Modifiers::ISO_LEVEL5_SHIFT;
    }
    modifiers
}

fn should_activate_monitors<I: InputBackend>(event: &InputEvent<I>) -> bool {
    match event {
        InputEvent::Keyboard { event } if event.state() == KeyState::Pressed => true,
        InputEvent::PointerButton { event } if event.state() == ButtonState::Pressed => true,
        InputEvent::PointerMotion { .. }
        | InputEvent::PointerMotionAbsolute { .. }
        | InputEvent::PointerAxis { .. }
        | InputEvent::GestureSwipeBegin { .. }
        | InputEvent::GesturePinchBegin { .. }
        | InputEvent::GestureHoldBegin { .. }
        | InputEvent::TouchDown { .. }
        | InputEvent::TouchMotion { .. }
        | InputEvent::TabletToolAxis { .. }
        | InputEvent::TabletToolProximity { .. }
        | InputEvent::TabletToolTip { .. }
        | InputEvent::TabletToolButton { .. } => true,
        // Ignore events like device additions and removals, key releases, gesture ends.
        _ => false,
    }
}

fn should_hide_hotkey_overlay<I: InputBackend>(event: &InputEvent<I>) -> bool {
    match event {
        InputEvent::Keyboard { event } if event.state() == KeyState::Pressed => true,
        InputEvent::PointerButton { event } if event.state() == ButtonState::Pressed => true,
        InputEvent::PointerAxis { .. }
        | InputEvent::GestureSwipeBegin { .. }
        | InputEvent::GesturePinchBegin { .. }
        | InputEvent::TouchDown { .. }
        | InputEvent::TouchMotion { .. }
        | InputEvent::TabletToolTip { .. }
        | InputEvent::TabletToolButton { .. } => true,
        _ => false,
    }
}

fn should_hide_exit_confirm_dialog<I: InputBackend>(event: &InputEvent<I>) -> bool {
    match event {
        InputEvent::Keyboard { event } if event.state() == KeyState::Pressed => true,
        InputEvent::PointerButton { event } if event.state() == ButtonState::Pressed => true,
        InputEvent::PointerAxis { .. }
        | InputEvent::GestureSwipeBegin { .. }
        | InputEvent::GesturePinchBegin { .. }
        | InputEvent::TouchDown { .. }
        | InputEvent::TouchMotion { .. }
        | InputEvent::TabletToolTip { .. }
        | InputEvent::TabletToolButton { .. } => true,
        _ => false,
    }
}

fn should_notify_activity<I: InputBackend>(event: &InputEvent<I>) -> bool {
    !matches!(
        event,
        InputEvent::DeviceAdded { .. } | InputEvent::DeviceRemoved { .. }
    )
}

fn should_reset_pointer_inactivity_timer<I: InputBackend>(event: &InputEvent<I>) -> bool {
    matches!(
        event,
        InputEvent::PointerAxis { .. }
            | InputEvent::PointerButton { .. }
            | InputEvent::PointerMotion { .. }
            | InputEvent::PointerMotionAbsolute { .. }
            | InputEvent::TabletToolAxis { .. }
            | InputEvent::TabletToolButton { .. }
            | InputEvent::TabletToolProximity { .. }
            | InputEvent::TabletToolTip { .. }
    )
}

fn allowed_when_locked(action: &Action) -> bool {
    matches!(
        action,
        Action::Quit(_)
            | Action::ChangeVt(_)
            | Action::Suspend
            | Action::PowerOffMonitors
            | Action::PowerOnMonitors
            | Action::SwitchLayout(_)
            | Action::ToggleKeyboardShortcutsInhibit
    )
}

fn allowed_during_screenshot(action: &Action) -> bool {
    matches!(
        action,
        Action::Quit(_)
            | Action::ChangeVt(_)
            | Action::Suspend
            | Action::PowerOffMonitors
            | Action::PowerOnMonitors
            // Intended for binds such as volume up/down, lock the screen, etc.
            | Action::Spawn(_)
            | Action::SpawnSh(_)
            // The screenshot UI can handle these.
            | Action::MoveColumnLeft
            | Action::MoveColumnLeftOrToMonitorLeft
            | Action::MoveColumnRight
            | Action::MoveColumnRightOrToMonitorRight
            | Action::MoveWindowUp
            | Action::MoveWindowUpOrToWorkspaceUp
            | Action::MoveWindowDown
            | Action::MoveWindowDownOrToWorkspaceDown
            | Action::MoveColumnToMonitorLeft
            | Action::MoveColumnToMonitorRight
            | Action::MoveColumnToMonitorUp
            | Action::MoveColumnToMonitorDown
            | Action::MoveColumnToMonitorPrevious
            | Action::MoveColumnToMonitorNext
            | Action::MoveColumnToMonitor(_)
            | Action::MoveWindowToMonitorLeft
            | Action::MoveWindowToMonitorRight
            | Action::MoveWindowToMonitorUp
            | Action::MoveWindowToMonitorDown
            | Action::MoveWindowToMonitorPrevious
            | Action::MoveWindowToMonitorNext
            | Action::MoveWindowToMonitor(_)
            | Action::SetWindowWidth(_)
            | Action::SetWindowHeight(_)
            | Action::SetColumnWidth(_)
    )
}

fn hardcoded_overview_bind(raw: Keysym, mods: ModifiersState, is_expo: bool) -> Option<Bind> {
    let mods = modifiers_from_state(mods);
    if !mods.is_empty() {
        return None;
    }

    let mut repeat = true;
    let action = if is_expo {
        match raw {
            Keysym::Escape => {
                repeat = false;
                Action::CloseExpo
            }
            Keysym::Return | Keysym::KP_Enter | Keysym::space => {
                repeat = false;
                Action::ExpoConfirm
            }
            Keysym::Left | Keysym::h | Keysym::H => Action::ExpoNavigateLeft,
            Keysym::Right | Keysym::l | Keysym::L => Action::ExpoNavigateRight,
            Keysym::Up | Keysym::k | Keysym::K => Action::ExpoNavigateUp,
            Keysym::Down | Keysym::j | Keysym::J => Action::ExpoNavigateDown,
            _ => {
                return None;
            }
        }
    } else {
        match raw {
            Keysym::Escape | Keysym::Return => {
                repeat = false;
                Action::ToggleOverview
            }
            Keysym::Left | Keysym::h | Keysym::H => Action::FocusColumnLeft,
            Keysym::Right | Keysym::l | Keysym::L => Action::FocusColumnRight,
            Keysym::Up | Keysym::k | Keysym::K => Action::FocusWindowOrWorkspaceUp,
            Keysym::Down | Keysym::j | Keysym::J => Action::FocusWindowOrWorkspaceDown,
            _ => {
                return None;
            }
        }
    };

    Some(Bind {
        key: Key {
            trigger: Trigger::Keysym(raw),
            modifiers: Modifiers::empty(),
        },
        action,
        repeat,
        cooldown: None,
        allow_when_locked: false,
        allow_inhibiting: false,
        hotkey_overlay_title: None,
    })
}

pub fn apply_libinput_settings(config: &clarion_config::Input, device: &mut input::Device) {
    // According to Mutter code, this setting is specific to touchpads.
    let is_touchpad = device.config_tap_finger_count() > 0;
    if is_touchpad {
        let c = &config.touchpad;
        let _ = device.config_send_events_set_mode(if c.off {
            input::SendEventsMode::DISABLED
        } else if c.disabled_on_external_mouse {
            input::SendEventsMode::DISABLED_ON_EXTERNAL_MOUSE
        } else {
            input::SendEventsMode::ENABLED
        });
        let _ = device.config_tap_set_enabled(c.tap);
        let _ = device.config_dwt_set_enabled(c.dwt);
        let _ = device.config_dwtp_set_enabled(c.dwtp);
        let _ = device.config_tap_set_drag_lock_enabled(if c.drag_lock {
            input::DragLockState::EnabledTimeout
        } else {
            input::DragLockState::Disabled
        });
        let _ = device.config_scroll_set_natural_scroll_enabled(c.natural_scroll);
        let _ = device.config_accel_set_speed(c.accel_speed.0);
        let _ = device.config_left_handed_set(c.left_handed);
        let _ = device.config_middle_emulation_set_enabled(c.middle_emulation);

        if let Some(drag) = c.drag {
            let _ = device.config_tap_set_drag_enabled(drag);
        } else {
            let default = device.config_tap_default_drag_enabled();
            let _ = device.config_tap_set_drag_enabled(default);
        }

        if let Some(accel_profile) = c.accel_profile {
            let _ = device.config_accel_set_profile(accel_profile.into());
        } else if let Some(default) = device.config_accel_default_profile() {
            let _ = device.config_accel_set_profile(default);
        }

        if let Some(method) = c.scroll_method {
            let _ = device.config_scroll_set_method(method.into());

            if method == clarion_config::ScrollMethod::OnButtonDown {
                if let Some(button) = c.scroll_button {
                    let _ = device.config_scroll_set_button(button);
                }
                let _ = device.config_scroll_set_button_lock(if c.scroll_button_lock {
                    input::ScrollButtonLockState::Enabled
                } else {
                    input::ScrollButtonLockState::Disabled
                });
            }
        } else if let Some(default) = device.config_scroll_default_method() {
            let _ = device.config_scroll_set_method(default);

            if default == input::ScrollMethod::OnButtonDown {
                if let Some(button) = c.scroll_button {
                    let _ = device.config_scroll_set_button(button);
                }
                let _ = device.config_scroll_set_button_lock(if c.scroll_button_lock {
                    input::ScrollButtonLockState::Enabled
                } else {
                    input::ScrollButtonLockState::Disabled
                });
            }
        }

        if let Some(tap_button_map) = c.tap_button_map {
            let _ = device.config_tap_set_button_map(tap_button_map.into());
        } else if let Some(default) = device.config_tap_default_button_map() {
            let _ = device.config_tap_set_button_map(default);
        }

        if let Some(method) = c.click_method {
            let _ = device.config_click_set_method(method.into());
        } else if let Some(default) = device.config_click_default_method() {
            let _ = device.config_click_set_method(default);
        }
    }

    // This is how Mutter tells apart mice.
    let mut is_trackball = false;
    let mut is_trackpoint = false;
    if let Some(udev_device) = unsafe { device.udev_device() } {
        if udev_device.property_value("ID_INPUT_TRACKBALL").is_some() {
            is_trackball = true;
        }
        if udev_device
            .property_value("ID_INPUT_POINTINGSTICK")
            .is_some()
        {
            is_trackpoint = true;
        }
    }

    let is_mouse = device.has_capability(input::DeviceCapability::Pointer)
        && !is_touchpad
        && !is_trackball
        && !is_trackpoint;
    if is_mouse {
        let c = &config.mouse;
        let _ = device.config_send_events_set_mode(if c.off {
            input::SendEventsMode::DISABLED
        } else {
            input::SendEventsMode::ENABLED
        });
        let _ = device.config_scroll_set_natural_scroll_enabled(c.natural_scroll);
        let _ = device.config_accel_set_speed(c.accel_speed.0);
        let _ = device.config_left_handed_set(c.left_handed);
        let _ = device.config_middle_emulation_set_enabled(c.middle_emulation);

        if let Some(accel_profile) = c.accel_profile {
            let _ = device.config_accel_set_profile(accel_profile.into());
        } else if let Some(default) = device.config_accel_default_profile() {
            let _ = device.config_accel_set_profile(default);
        }

        if let Some(method) = c.scroll_method {
            let _ = device.config_scroll_set_method(method.into());

            if method == clarion_config::ScrollMethod::OnButtonDown {
                if let Some(button) = c.scroll_button {
                    let _ = device.config_scroll_set_button(button);
                }
                let _ = device.config_scroll_set_button_lock(if c.scroll_button_lock {
                    input::ScrollButtonLockState::Enabled
                } else {
                    input::ScrollButtonLockState::Disabled
                });
            }
        } else if let Some(default) = device.config_scroll_default_method() {
            let _ = device.config_scroll_set_method(default);

            if default == input::ScrollMethod::OnButtonDown {
                if let Some(button) = c.scroll_button {
                    let _ = device.config_scroll_set_button(button);
                }
                let _ = device.config_scroll_set_button_lock(if c.scroll_button_lock {
                    input::ScrollButtonLockState::Enabled
                } else {
                    input::ScrollButtonLockState::Disabled
                });
            }
        }
    }

    if is_trackball {
        let c = &config.trackball;
        let _ = device.config_send_events_set_mode(if c.off {
            input::SendEventsMode::DISABLED
        } else {
            input::SendEventsMode::ENABLED
        });
        let _ = device.config_scroll_set_natural_scroll_enabled(c.natural_scroll);
        let _ = device.config_accel_set_speed(c.accel_speed.0);
        let _ = device.config_middle_emulation_set_enabled(c.middle_emulation);
        let _ = device.config_left_handed_set(c.left_handed);

        if let Some(accel_profile) = c.accel_profile {
            let _ = device.config_accel_set_profile(accel_profile.into());
        } else if let Some(default) = device.config_accel_default_profile() {
            let _ = device.config_accel_set_profile(default);
        }

        if let Some(method) = c.scroll_method {
            let _ = device.config_scroll_set_method(method.into());

            if method == clarion_config::ScrollMethod::OnButtonDown {
                if let Some(button) = c.scroll_button {
                    let _ = device.config_scroll_set_button(button);
                }
                let _ = device.config_scroll_set_button_lock(if c.scroll_button_lock {
                    input::ScrollButtonLockState::Enabled
                } else {
                    input::ScrollButtonLockState::Disabled
                });
            }
        } else if let Some(default) = device.config_scroll_default_method() {
            let _ = device.config_scroll_set_method(default);

            if default == input::ScrollMethod::OnButtonDown {
                if let Some(button) = c.scroll_button {
                    let _ = device.config_scroll_set_button(button);
                }
                let _ = device.config_scroll_set_button_lock(if c.scroll_button_lock {
                    input::ScrollButtonLockState::Enabled
                } else {
                    input::ScrollButtonLockState::Disabled
                });
            }
        }
    }

    if is_trackpoint {
        let c = &config.trackpoint;
        let _ = device.config_send_events_set_mode(if c.off {
            input::SendEventsMode::DISABLED
        } else {
            input::SendEventsMode::ENABLED
        });
        let _ = device.config_scroll_set_natural_scroll_enabled(c.natural_scroll);
        let _ = device.config_accel_set_speed(c.accel_speed.0);
        let _ = device.config_left_handed_set(c.left_handed);
        let _ = device.config_middle_emulation_set_enabled(c.middle_emulation);

        if let Some(accel_profile) = c.accel_profile {
            let _ = device.config_accel_set_profile(accel_profile.into());
        } else if let Some(default) = device.config_accel_default_profile() {
            let _ = device.config_accel_set_profile(default);
        }

        if let Some(method) = c.scroll_method {
            let _ = device.config_scroll_set_method(method.into());

            if method == clarion_config::ScrollMethod::OnButtonDown {
                if let Some(button) = c.scroll_button {
                    let _ = device.config_scroll_set_button(button);
                }
                let _ = device.config_scroll_set_button_lock(if c.scroll_button_lock {
                    input::ScrollButtonLockState::Enabled
                } else {
                    input::ScrollButtonLockState::Disabled
                });
            }
        } else if let Some(default) = device.config_scroll_default_method() {
            let _ = device.config_scroll_set_method(default);

            if default == input::ScrollMethod::OnButtonDown {
                if let Some(button) = c.scroll_button {
                    let _ = device.config_scroll_set_button(button);
                }
                let _ = device.config_scroll_set_button_lock(if c.scroll_button_lock {
                    input::ScrollButtonLockState::Enabled
                } else {
                    input::ScrollButtonLockState::Disabled
                });
            }
        }
    }

    let is_tablet = device.has_capability(input::DeviceCapability::TabletTool);
    if is_tablet {
        let c = &config.tablet;
        let _ = device.config_send_events_set_mode(if c.off {
            input::SendEventsMode::DISABLED
        } else {
            input::SendEventsMode::ENABLED
        });

        #[rustfmt::skip]
        const IDENTITY_MATRIX: [f32; 6] = [
            1., 0., 0.,
            0., 1., 0.,
        ];

        let _ = device.config_calibration_set_matrix(
            c.calibration_matrix
                .as_deref()
                .and_then(|m| m.try_into().ok())
                .or(device.config_calibration_default_matrix())
                .unwrap_or(IDENTITY_MATRIX),
        );

        let _ = device.config_left_handed_set(c.left_handed);
    }

    let is_touch = device.has_capability(input::DeviceCapability::Touch);
    if is_touch {
        let c = &config.touch;
        let _ = device.config_send_events_set_mode(if c.off {
            input::SendEventsMode::DISABLED
        } else {
            input::SendEventsMode::ENABLED
        });

        #[rustfmt::skip]
        const IDENTITY_MATRIX: [f32; 6] = [
            1., 0., 0.,
            0., 1., 0.,
        ];

        let _ = device.config_calibration_set_matrix(
            c.calibration_matrix
                .as_deref()
                .and_then(|m| m.try_into().ok())
                .or(device.config_calibration_default_matrix())
                .unwrap_or(IDENTITY_MATRIX),
        );
    }
}

pub fn mods_with_binds(mod_key: ModKey, binds: &Binds, triggers: &[Trigger]) -> HashSet<Modifiers> {
    let mut rv = HashSet::new();
    for bind in &binds.0 {
        if !triggers.contains(&bind.key.trigger) {
            continue;
        }

        let mut mods = bind.key.modifiers;
        if mods.contains(Modifiers::COMPOSITOR) {
            mods.remove(Modifiers::COMPOSITOR);
            mods.insert(mod_key.to_modifiers());
        }

        rv.insert(mods);
    }

    rv
}

pub fn mods_with_mouse_binds(mod_key: ModKey, binds: &Binds) -> HashSet<Modifiers> {
    mods_with_binds(
        mod_key,
        binds,
        &[
            Trigger::MouseLeft,
            Trigger::MouseRight,
            Trigger::MouseMiddle,
            Trigger::MouseBack,
            Trigger::MouseForward,
        ],
    )
}

pub fn mods_with_wheel_binds(mod_key: ModKey, binds: &Binds) -> HashSet<Modifiers> {
    mods_with_binds(
        mod_key,
        binds,
        &[
            Trigger::WheelScrollUp,
            Trigger::WheelScrollDown,
            Trigger::WheelScrollLeft,
            Trigger::WheelScrollRight,
        ],
    )
}

pub fn mods_with_finger_scroll_binds(mod_key: ModKey, binds: &Binds) -> HashSet<Modifiers> {
    mods_with_binds(
        mod_key,
        binds,
        &[
            Trigger::TouchpadScrollUp,
            Trigger::TouchpadScrollDown,
            Trigger::TouchpadScrollLeft,
            Trigger::TouchpadScrollRight,
        ],
    )
}

pub fn mods_with_tablet_stylus_binds(mod_key: ModKey, binds: &Binds) -> HashSet<Modifiers> {
    mods_with_binds(
        mod_key,
        binds,
        &[
            Trigger::TabletStylusButton1,
            Trigger::TabletStylusButton2,
            Trigger::TabletStylusButton3,
        ],
    )
}

fn grab_allows_hot_corner(grab: &(dyn PointerGrab<State> + 'static)) -> bool {
    let grab = grab.as_any();

    // We lean on the blocklist approach here since it's not a terribly big deal if hot corner
    // works where it shouldn't, but it could prevent some workflows if the hot corner doesn't work
    // when it should.
    //
    // Some notable grabs not mentioned here:
    // - DnDGrab allows hot corner to DnD across workspaces.
    // - ClickGrab keeps pointer focus on the window, so the hot corner doesn't trigger.
    // - Touch grabs: touch doesn't trigger the hot corner.
    if grab.is::<ResizeGrab>() || grab.is::<SpatialMovementGrab>() || grab.is::<MoveGrab>() {
        return false;
    }

    true
}

/// Returns an iterator over bindings.
///
/// Includes dynamically populated bindings like the MRU UI.
fn make_binds_iter<'a>(
    config: &'a Config,
    mru: &'a mut WindowMruUi,
    mods: Modifiers,
) -> impl Iterator<Item = &'a Bind> + Clone {
    // Figure out the binds to use depending on whether the MRU is enabled and/or open.
    let general_binds = (!mru.is_open()).then_some(config.binds.0.iter());
    let general_binds = general_binds.into_iter().flatten();

    let mru_binds =
        (config.recent_windows.on || mru.is_open()).then_some(config.recent_windows.binds.iter());
    let mru_binds = mru_binds.into_iter().flatten();

    let mru_open_binds = mru.is_open().then(|| mru.opened_bindings(mods));
    let mru_open_binds = mru_open_binds.into_iter().flatten();

    // General binds take precedence over the MRU binds.
    general_binds.chain(mru_binds).chain(mru_open_binds)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::animation::Clock;

    fn repeat_fixture() -> crate::tests::Fixture {
        let mut config = Config::default();
        config.input.keyboard.repeat_delay = 0;
        config.hotkey_overlay.skip_at_startup = true;
        crate::tests::Fixture::with_config(config)
    }

    fn repeat_bind() -> Bind {
        Bind {
            key: Key {
                trigger: Trigger::Keysym(Keysym::F1),
                modifiers: Modifiers::empty(),
            },
            action: Action::ShowHotkeyOverlay,
            repeat: true,
            cooldown: None,
            allow_when_locked: false,
            allow_inhibiting: false,
            hotkey_overlay_title: None,
        }
    }

    fn seat_key(state: &mut State, code: Keycode, pressed: bool) {
        state.clarion.seat.get_keyboard().unwrap().input(
            state,
            code,
            if pressed {
                KeyState::Pressed
            } else {
                KeyState::Released
            },
            SERIAL_COUNTER.next_serial(),
            smithay::backend::input::InputTime::from_millis(0),
            |_, _, _| FilterResult::Intercept(()),
        );
    }

    #[test]
    fn shortcut_repeat_keeps_newer_key_when_previous_key_released() {
        let mut f = repeat_fixture();
        let older = Keycode::from(38_u32);
        let current = Keycode::from(56_u32);
        let state = f.clarion_state();
        seat_key(state, older, true);
        state.start_key_repeat(repeat_bind(), older);
        seat_key(state, current, true);
        state.start_key_repeat(repeat_bind(), current);
        seat_key(state, older, false);
        // The pre-XKB release path and post-XKB modifier path both preserve
        // the new owner, and its actual timer still executes the action.
        state.update_key_repeat(older, false, None);
        state.update_key_repeat(older, false, Some(Modifiers::empty()));
        assert_eq!(
            state.clarion.bind_repeat_timer.as_ref().unwrap().key_code,
            current
        );
        f.state.server.dispatch();
        assert!(f.clarion().hotkey_overlay.is_open());
        let state = f.clarion_state();
        seat_key(state, current, false);
        state.update_key_repeat(current, false, None);
        assert!(state.clarion.bind_repeat_timer.is_none());
    }

    #[test]
    fn shortcut_repeat_stops_on_modifier_changes() {
        let mut f = repeat_fixture();
        let state = f.clarion_state();
        let key = Keycode::from(38_u32);
        let other = Keycode::from(56_u32);
        seat_key(state, key, true);
        state.start_key_repeat(repeat_bind(), key);
        // An unrelated press with the same chord is harmless.
        state.update_key_repeat(other, true, Some(Modifiers::empty()));
        assert!(state.clarion.bind_repeat_timer.is_some());
        state.update_key_repeat(Keycode::from(50_u32), true, Some(Modifiers::SHIFT));
        assert!(state.clarion.bind_repeat_timer.is_none());
        f.state.server.dispatch();
        assert!(!f.clarion().hotkey_overlay.is_open());
    }

    #[test]
    fn shortcut_repeat_drops_when_seat_loses_pressed_key_without_routed_release() {
        let mut f = repeat_fixture();
        let state = f.clarion_state();
        let key = Keycode::from(38_u32);
        seat_key(state, key, true);
        state.start_key_repeat(repeat_bind(), key);
        // Input backend focus loss may update XKB without the shortcut filter.
        seat_key(state, key, false);
        f.state.server.dispatch();
        assert!(f.clarion().bind_repeat_timer.is_none());
        assert!(!f.clarion().hotkey_overlay.is_open());
    }

    #[test]
    fn shortcut_repeat_does_not_survive_lock_epoch_change() {
        let mut f = repeat_fixture();
        let state = f.clarion_state();
        let key = Keycode::from(38_u32);
        seat_key(state, key, true);
        state.start_key_repeat(repeat_bind(), key);
        state.clarion.capture_scene_epoch.invalidate();
        f.state.server.dispatch();
        assert!(f.clarion().bind_repeat_timer.is_none());
        assert!(!f.clarion().hotkey_overlay.is_open());
    }

    #[test]
    fn macos_desktop_shortcuts_match_only_implemented_command_chords() {
        let command = ModifiersState {
            logo: true,
            ..Default::default()
        };
        for key in [
            Keysym::a,
            Keysym::A,
            Keysym::i,
            Keysym::I,
            Keysym::o,
            Keysym::O,
            Keysym::y,
            Keysym::Y,
            Keysym::Down,
            Keysym::BackSpace,
        ] {
            assert!(is_macos_desktop_shortcut(Some(key), command));
            for mods in [
                ModifiersState::default(),
                ModifiersState {
                    shift: true,
                    ..command
                },
                ModifiersState {
                    ctrl: true,
                    ..command
                },
                ModifiersState {
                    alt: true,
                    ..command
                },
                ModifiersState {
                    iso_level3_shift: true,
                    ..command
                },
                ModifiersState {
                    iso_level5_shift: true,
                    ..command
                },
            ] {
                assert!(!is_macos_desktop_shortcut(Some(key), mods));
            }
        }
        for key in [Keysym::n, Keysym::N] {
            assert!(is_macos_desktop_shortcut(
                Some(key),
                ModifiersState {
                    shift: true,
                    ..command
                }
            ));
            assert!(!is_macos_desktop_shortcut(Some(key), command));
            assert!(!is_macos_desktop_shortcut(
                Some(key),
                ModifiersState {
                    shift: true,
                    alt: true,
                    ..command
                }
            ));
        }
        for key in [
            None,
            Some(Keysym::q),
            Some(Keysym::space),
            Some(Keysym::Delete),
            Some(Keysym::Return),
            Some(Keysym::Left),
        ] {
            assert!(!is_macos_desktop_shortcut(key, command));
        }
    }

    #[test]
    fn macos_desktop_routing_keeps_press_release_pairs_across_focus_and_modifier_changes() {
        let binding = Bind {
            key: Key {
                trigger: Trigger::Keysym(Keysym::o),
                modifiers: Modifiers::COMPOSITOR,
            },
            action: Action::ToggleOverview,
            repeat: false,
            cooldown: None,
            allow_when_locked: false,
            allow_inhibiting: false,
            hotkey_overlay_title: None,
        };
        let screenshot_ui = ScreenshotUi::new(Clock::default(), Default::default());
        let key = Keycode::from(Keysym::o.raw() + 8);
        let command = ModifiersState {
            logo: true,
            ..Default::default()
        };
        let mut suppressed = HashSet::new();
        let mut input = |desktop, pressed, mods| {
            let desktop_shortcut = desktop && is_macos_desktop_shortcut(Some(Keysym::o), mods);
            should_intercept_key(
                &mut suppressed,
                std::iter::once(&binding).filter(|_| !desktop_shortcut),
                ModKey::Super,
                key,
                Keysym::o,
                Some(Keysym::o),
                pressed,
                mods,
                &screenshot_ui,
                false,
                false,
            )
        };
        // Base macOS desktop receives both halves, even if focus/style changes
        // before release or the Command key is released first.
        assert!(matches!(input(true, true, command), FilterResult::Forward));
        assert!(matches!(
            input(false, false, command),
            FilterResult::Forward
        ));
        assert!(matches!(input(true, true, command), FilterResult::Forward));
        assert!(matches!(
            input(true, false, ModifiersState::default()),
            FilterResult::Forward
        ));
        // The reverse transition must not leak an unmatched release to the desktop.
        assert!(matches!(
            input(false, true, command),
            FilterResult::Intercept(Some(_))
        ));
        assert!(matches!(
            input(true, false, command),
            FilterResult::Intercept(None)
        ));
        assert!(matches!(
            input(false, true, command),
            FilterResult::Intercept(Some(_))
        ));
        assert!(matches!(
            input(true, false, ModifiersState::default()),
            FilterResult::Intercept(None)
        ));
        assert!(suppressed.is_empty());
    }

    #[test]
    fn hotkey_overlay_navigation_suppresses_press_and_release() {
        let mut overlay = HotkeyOverlay::new(Default::default(), ModKey::Super);
        let mut suppressed = HashSet::new();
        let key = Keycode::from(Keysym::Next.raw() + 8);
        overlay.show();
        let screenshot_ui = ScreenshotUi::new(Clock::default(), Default::default());
        let mut input = |overlay: &mut HotkeyOverlay, keysym, pressed, modifiers, allowed| {
            let intercepted = intercept_hotkey_overlay_key(
                overlay,
                &mut suppressed,
                key,
                keysym,
                pressed,
                modifiers,
                allowed,
            );
            if pressed {
                intercepted
            } else {
                assert!(
                    !intercepted,
                    "release must pass through normal screenshot/bind processing"
                );
                matches!(
                    should_intercept_key(
                        &mut suppressed,
                        std::iter::empty::<&Bind>(),
                        ModKey::Super,
                        key,
                        keysym,
                        Some(keysym),
                        false,
                        ModifiersState::default(),
                        &screenshot_ui,
                        false,
                        false,
                    ),
                    FilterResult::Intercept(None)
                )
            }
        };
        // Page navigation stays open and does not reach a client or configured bind.
        assert!(input(
            &mut overlay,
            Keysym::Next,
            true,
            Modifiers::empty(),
            true
        ));
        assert!(overlay.is_open());
        assert!(input(
            &mut overlay,
            Keysym::Next,
            false,
            Modifiers::empty(),
            true
        ));
        assert!(!input(
            &mut overlay,
            Keysym::Next,
            false,
            Modifiers::empty(),
            true
        ));
        // Closing still consumes its matching release even after the overlay is hidden.
        assert!(input(
            &mut overlay,
            Keysym::Escape,
            true,
            Modifiers::empty(),
            true
        ));
        assert!(!overlay.is_open());
        assert!(input(
            &mut overlay,
            Keysym::Escape,
            false,
            Modifiers::empty(),
            true
        ));
        overlay.show();
        // Modified shortcuts and keys owned by higher-priority UIs retain existing behavior.
        assert!(!input(
            &mut overlay,
            Keysym::Next,
            true,
            Modifiers::CTRL,
            true
        ));
        assert!(!input(
            &mut overlay,
            Keysym::Next,
            false,
            Modifiers::empty(),
            true
        ));
        assert!(!input(
            &mut overlay,
            Keysym::Next,
            true,
            Modifiers::empty(),
            false
        ));
        assert!(!input(
            &mut overlay,
            Keysym::Next,
            false,
            Modifiers::empty(),
            false
        ));
        assert!(!input(
            &mut overlay,
            Keysym::q,
            true,
            Modifiers::empty(),
            true
        ));
        assert!(overlay.is_open());
    }

    #[test]
    fn bindings_suppress_keys() {
        let close_keysym = Keysym::q;
        let bindings = Binds(vec![Bind {
            key: Key {
                trigger: Trigger::Keysym(close_keysym),
                modifiers: Modifiers::COMPOSITOR | Modifiers::CTRL,
            },
            action: Action::CloseWindow,
            repeat: true,
            cooldown: None,
            allow_when_locked: false,
            allow_inhibiting: true,
            hotkey_overlay_title: None,
        }]);

        let comp_mod = ModKey::Super;
        let mut suppressed_keys = HashSet::new();

        let screenshot_ui = ScreenshotUi::new(Clock::default(), Default::default());
        let disable_power_key_handling = false;
        let is_inhibiting_shortcuts = Cell::new(false);

        // The key_code we pick is arbitrary, the only thing
        // that matters is that they are different between cases.

        let close_key_code = Keycode::from(close_keysym.raw() + 8u32);
        let close_key_event = |suppr: &mut HashSet<Keycode>, mods: ModifiersState, pressed| {
            should_intercept_key(
                suppr,
                &bindings.0,
                comp_mod,
                close_key_code,
                close_keysym,
                Some(close_keysym),
                pressed,
                mods,
                &screenshot_ui,
                disable_power_key_handling,
                is_inhibiting_shortcuts.get(),
            )
        };

        // Key event with the code which can't trigger any action.
        let none_key_event = |suppr: &mut HashSet<Keycode>, mods: ModifiersState, pressed| {
            should_intercept_key(
                suppr,
                &bindings.0,
                comp_mod,
                Keycode::from(Keysym::l.raw() + 8),
                Keysym::l,
                Some(Keysym::l),
                pressed,
                mods,
                &screenshot_ui,
                disable_power_key_handling,
                is_inhibiting_shortcuts.get(),
            )
        };

        let mut mods = ModifiersState {
            logo: true,
            ctrl: true,
            ..Default::default()
        };

        // Action press/release.

        let filter = close_key_event(&mut suppressed_keys, mods, true);
        assert!(matches!(
            filter,
            FilterResult::Intercept(Some(Bind {
                action: Action::CloseWindow,
                ..
            }))
        ));
        assert!(suppressed_keys.contains(&close_key_code));

        let filter = close_key_event(&mut suppressed_keys, mods, false);
        assert!(matches!(filter, FilterResult::Intercept(None)));
        assert!(suppressed_keys.is_empty());

        // Remove mod to make it for a binding.

        mods.shift = true;
        let filter = close_key_event(&mut suppressed_keys, mods, true);
        assert!(matches!(filter, FilterResult::Forward));

        mods.shift = false;
        let filter = close_key_event(&mut suppressed_keys, mods, false);
        assert!(matches!(filter, FilterResult::Forward));

        // Just none press/release.

        let filter = none_key_event(&mut suppressed_keys, mods, true);
        assert!(matches!(filter, FilterResult::Forward));

        let filter = none_key_event(&mut suppressed_keys, mods, false);
        assert!(matches!(filter, FilterResult::Forward));

        // Press action, press arbitrary, release action, release arbitrary.

        let filter = close_key_event(&mut suppressed_keys, mods, true);
        assert!(matches!(
            filter,
            FilterResult::Intercept(Some(Bind {
                action: Action::CloseWindow,
                ..
            }))
        ));

        let filter = none_key_event(&mut suppressed_keys, mods, true);
        assert!(matches!(filter, FilterResult::Forward));

        let filter = close_key_event(&mut suppressed_keys, mods, false);
        assert!(matches!(filter, FilterResult::Intercept(None)));

        let filter = none_key_event(&mut suppressed_keys, mods, false);
        assert!(matches!(filter, FilterResult::Forward));

        // Trigger and remove all mods.

        let filter = close_key_event(&mut suppressed_keys, mods, true);
        assert!(matches!(
            filter,
            FilterResult::Intercept(Some(Bind {
                action: Action::CloseWindow,
                ..
            }))
        ));

        mods = Default::default();
        let filter = close_key_event(&mut suppressed_keys, mods, false);
        assert!(matches!(filter, FilterResult::Intercept(None)));

        // Ensure that no keys are being suppressed.
        assert!(suppressed_keys.is_empty());

        // Now test shortcut inhibiting.

        // With inhibited shortcuts, we don't intercept our shortcut.
        is_inhibiting_shortcuts.set(true);

        mods = ModifiersState {
            logo: true,
            ctrl: true,
            ..Default::default()
        };

        let filter = close_key_event(&mut suppressed_keys, mods, true);
        assert!(matches!(filter, FilterResult::Forward));
        assert!(suppressed_keys.is_empty());

        let filter = close_key_event(&mut suppressed_keys, mods, false);
        assert!(matches!(filter, FilterResult::Forward));
        assert!(suppressed_keys.is_empty());

        // Toggle it off after pressing the shortcut.
        let filter = close_key_event(&mut suppressed_keys, mods, true);
        assert!(matches!(filter, FilterResult::Forward));
        assert!(suppressed_keys.is_empty());

        is_inhibiting_shortcuts.set(false);

        let filter = close_key_event(&mut suppressed_keys, mods, false);
        assert!(matches!(filter, FilterResult::Forward));
        assert!(suppressed_keys.is_empty());

        // Toggle it on after pressing the shortcut.
        let filter = close_key_event(&mut suppressed_keys, mods, true);
        assert!(matches!(
            filter,
            FilterResult::Intercept(Some(Bind {
                action: Action::CloseWindow,
                ..
            }))
        ));
        assert!(suppressed_keys.contains(&close_key_code));

        is_inhibiting_shortcuts.set(true);

        let filter = close_key_event(&mut suppressed_keys, mods, false);
        assert!(matches!(filter, FilterResult::Intercept(None)));
        assert!(suppressed_keys.is_empty());
    }

    #[test]
    fn comp_mod_handling() {
        let bindings = Binds(vec![
            Bind {
                key: Key {
                    trigger: Trigger::Keysym(Keysym::q),
                    modifiers: Modifiers::COMPOSITOR,
                },
                action: Action::CloseWindow,
                repeat: true,
                cooldown: None,
                allow_when_locked: false,
                allow_inhibiting: true,
                hotkey_overlay_title: None,
            },
            Bind {
                key: Key {
                    trigger: Trigger::Keysym(Keysym::h),
                    modifiers: Modifiers::SUPER,
                },
                action: Action::FocusColumnLeft,
                repeat: true,
                cooldown: None,
                allow_when_locked: false,
                allow_inhibiting: true,
                hotkey_overlay_title: None,
            },
            Bind {
                key: Key {
                    trigger: Trigger::Keysym(Keysym::j),
                    modifiers: Modifiers::empty(),
                },
                action: Action::FocusWindowDown,
                repeat: true,
                cooldown: None,
                allow_when_locked: false,
                allow_inhibiting: true,
                hotkey_overlay_title: None,
            },
            Bind {
                key: Key {
                    trigger: Trigger::Keysym(Keysym::k),
                    modifiers: Modifiers::COMPOSITOR | Modifiers::SUPER,
                },
                action: Action::FocusWindowUp,
                repeat: true,
                cooldown: None,
                allow_when_locked: false,
                allow_inhibiting: true,
                hotkey_overlay_title: None,
            },
            Bind {
                key: Key {
                    trigger: Trigger::Keysym(Keysym::l),
                    modifiers: Modifiers::SUPER | Modifiers::ALT,
                },
                action: Action::FocusColumnRight,
                repeat: true,
                cooldown: None,
                allow_when_locked: false,
                allow_inhibiting: true,
                hotkey_overlay_title: None,
            },
        ]);

        assert_eq!(
            find_configured_bind(
                &bindings.0,
                ModKey::Super,
                Trigger::Keysym(Keysym::q),
                ModifiersState {
                    logo: true,
                    ..Default::default()
                }
            )
            .as_ref(),
            Some(&bindings.0[0])
        );
        assert_eq!(
            find_configured_bind(
                &bindings.0,
                ModKey::Super,
                Trigger::Keysym(Keysym::q),
                ModifiersState::default(),
            ),
            None,
        );

        assert_eq!(
            find_configured_bind(
                &bindings.0,
                ModKey::Super,
                Trigger::Keysym(Keysym::h),
                ModifiersState {
                    logo: true,
                    ..Default::default()
                }
            )
            .as_ref(),
            Some(&bindings.0[1])
        );
        assert_eq!(
            find_configured_bind(
                &bindings.0,
                ModKey::Super,
                Trigger::Keysym(Keysym::h),
                ModifiersState::default(),
            ),
            None,
        );

        assert_eq!(
            find_configured_bind(
                &bindings.0,
                ModKey::Super,
                Trigger::Keysym(Keysym::j),
                ModifiersState {
                    logo: true,
                    ..Default::default()
                }
            ),
            None,
        );
        assert_eq!(
            find_configured_bind(
                &bindings.0,
                ModKey::Super,
                Trigger::Keysym(Keysym::j),
                ModifiersState::default(),
            )
            .as_ref(),
            Some(&bindings.0[2])
        );

        assert_eq!(
            find_configured_bind(
                &bindings.0,
                ModKey::Super,
                Trigger::Keysym(Keysym::k),
                ModifiersState {
                    logo: true,
                    ..Default::default()
                }
            )
            .as_ref(),
            Some(&bindings.0[3])
        );
        assert_eq!(
            find_configured_bind(
                &bindings.0,
                ModKey::Super,
                Trigger::Keysym(Keysym::k),
                ModifiersState::default(),
            ),
            None,
        );

        assert_eq!(
            find_configured_bind(
                &bindings.0,
                ModKey::Super,
                Trigger::Keysym(Keysym::l),
                ModifiersState {
                    logo: true,
                    alt: true,
                    ..Default::default()
                }
            )
            .as_ref(),
            Some(&bindings.0[4])
        );
        assert_eq!(
            find_configured_bind(
                &bindings.0,
                ModKey::Super,
                Trigger::Keysym(Keysym::l),
                ModifiersState {
                    logo: true,
                    ..Default::default()
                },
            ),
            None,
        );
    }
}

#[cfg(test)]
mod launcher_key_tests {
    use super::*;

    #[test]
    fn apps_key_is_unmodified_f4_with_no_repeat_or_lock_bypass() {
        let bind = launcher_key_bind(Some(Keysym::F4), Modifiers::empty()).unwrap();
        assert_eq!(
            bind.action,
            Action::Spawn(vec!["clarion-shell-msg".into(), "launcher".into()])
        );
        assert!(!bind.repeat && !bind.allow_when_locked && bind.allow_inhibiting);
        for modifiers in [
            Modifiers::SUPER,
            Modifiers::SHIFT,
            Modifiers::CTRL,
            Modifiers::ALT,
        ] {
            assert!(launcher_key_bind(Some(Keysym::F4), modifiers).is_none());
        }
        assert!(launcher_key_bind(Some(Keysym::F3), Modifiers::empty()).is_none());
        assert!(launcher_key_bind(None, Modifiers::empty()).is_none());
    }
}
