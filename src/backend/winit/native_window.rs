//! Winit window and input source without Smithay's EGL graphics backend.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use calloop::generic::Generic;
use calloop::{EventSource, Interest, Mode, PostAction, Readiness, Token, TokenFactory};
use smithay::backend::input::{
    AbsolutePositionEvent, Axis, AxisRelativeDirection, AxisSource, ButtonState, Device,
    DeviceCapability, Event, InputBackend, InputEvent, KeyState, KeyboardKeyEvent, Keycode,
    PointerAxisEvent, PointerButtonEvent, PointerMotionAbsoluteEvent, TouchCancelEvent,
    TouchDownEvent, TouchEvent, TouchFrameEvent, TouchMotionEvent, TouchSlot, TouchUpEvent,
    UnusedEvent,
};
use smithay::reexports::winit::application::ApplicationHandler;
use smithay::reexports::winit::dpi::{LogicalSize, PhysicalPosition};
use smithay::reexports::winit::event::{
    ButtonSource, ElementState, MouseButton, MouseScrollDelta, PointerKind, PointerSource,
    WindowEvent,
};
use smithay::reexports::winit::event_loop::pump_events::{EventLoopExtPumpEvents, PumpStatus};
use smithay::reexports::winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use smithay::reexports::winit::platform::scancode::PhysicalKeyExtScancode;
use smithay::reexports::winit::window::{Window, WindowAttributes, WindowId};
use smithay::utils::{Physical, Size};

use crate::utils::get_monotonic_time;

#[derive(Debug)]
pub struct NativeInput;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NativeDevice;

impl Device for NativeDevice {
    fn id(&self) -> String {
        "winit".into()
    }
    fn name(&self) -> String {
        "winit virtual input".into()
    }
    fn has_capability(&self, capability: DeviceCapability) -> bool {
        matches!(
            capability,
            DeviceCapability::Keyboard | DeviceCapability::Pointer | DeviceCapability::Touch
        )
    }
    fn usb_id(&self) -> Option<(u32, u32)> {
        None
    }
    fn syspath(&self) -> Option<PathBuf> {
        None
    }
}

#[derive(Debug, Clone, Copy)]
pub struct KeyEvent {
    time: u64,
    key: u32,
    count: u32,
    state: ElementState,
}
#[derive(Debug, Clone, Copy)]
pub struct MotionEvent {
    time: u64,
    position: PhysicalPosition<f64>,
    size: (f64, f64),
}
#[derive(Debug, Clone, Copy)]
pub struct WheelEvent {
    time: u64,
    delta: MouseScrollDelta,
}
#[derive(Debug, Clone, Copy)]
pub struct ButtonEvent {
    time: u64,
    button: MouseButton,
    state: ElementState,
    is_x11: bool,
}
#[derive(Debug, Clone, Copy)]
pub struct TouchPositionEvent {
    time: u64,
    id: u64,
    position: PhysicalPosition<f64>,
    size: (f64, f64),
}
#[derive(Debug, Clone, Copy)]
pub struct TouchEndEvent {
    time: u64,
    id: u64,
}
#[derive(Debug, Clone, Copy)]
pub struct TouchFrame {
    time: u64,
}

macro_rules! input_event {
    ($ty:ty) => {
        impl Event<NativeInput> for $ty {
            fn time(&self) -> u64 {
                self.time
            }
            fn device(&self) -> NativeDevice {
                NativeDevice
            }
        }
    };
}
input_event!(KeyEvent);
input_event!(MotionEvent);
input_event!(WheelEvent);
input_event!(ButtonEvent);
input_event!(TouchPositionEvent);
input_event!(TouchEndEvent);
input_event!(TouchFrame);

impl KeyboardKeyEvent<NativeInput> for KeyEvent {
    fn key_code(&self) -> Keycode {
        (self.key + 8).into()
    }
    fn state(&self) -> KeyState {
        match self.state {
            ElementState::Pressed => KeyState::Pressed,
            ElementState::Released => KeyState::Released,
        }
    }
    fn count(&self) -> u32 {
        self.count
    }
}
impl AbsolutePositionEvent<NativeInput> for MotionEvent {
    fn x(&self) -> f64 {
        self.position.x
    }
    fn y(&self) -> f64 {
        self.position.y
    }
    fn x_transformed(&self, width: i32) -> f64 {
        (self.position.x / self.size.0 * width as f64).max(0.0)
    }
    fn y_transformed(&self, height: i32) -> f64 {
        (self.position.y / self.size.1 * height as f64).max(0.0)
    }
}
impl PointerMotionAbsoluteEvent<NativeInput> for MotionEvent {}
impl PointerAxisEvent<NativeInput> for WheelEvent {
    fn amount(&self, axis: Axis) -> Option<f64> {
        match (axis, self.delta) {
            (Axis::Horizontal, MouseScrollDelta::PixelDelta(delta)) => Some(-delta.x),
            (Axis::Vertical, MouseScrollDelta::PixelDelta(delta)) => Some(-delta.y),
            _ => None,
        }
    }
    fn amount_v120(&self, axis: Axis) -> Option<f64> {
        match (axis, self.delta) {
            (Axis::Horizontal, MouseScrollDelta::LineDelta(x, _)) => Some(-f64::from(x) * 120.0),
            (Axis::Vertical, MouseScrollDelta::LineDelta(_, y)) => Some(-f64::from(y) * 120.0),
            _ => None,
        }
    }
    fn source(&self) -> AxisSource {
        match self.delta {
            MouseScrollDelta::LineDelta(..) => AxisSource::Wheel,
            MouseScrollDelta::PixelDelta(..) => AxisSource::Continuous,
        }
    }
    fn relative_direction(&self, _: Axis) -> AxisRelativeDirection {
        AxisRelativeDirection::Identical
    }
}
impl PointerButtonEvent<NativeInput> for ButtonEvent {
    fn button_code(&self) -> u32 {
        match self.button {
            MouseButton::Left => 0x110,
            MouseButton::Right => 0x111,
            MouseButton::Middle => 0x112,
            MouseButton::Forward => 0x115,
            MouseButton::Back => 0x116,
            _ if self.is_x11 => match self.button as u32 + 1 {
                button @ 0..=3 => [0, 0x110, 0x112, 0x111][button as usize],
                button => button.saturating_sub(8) + 0x113,
            },
            _ => self.button as u32 + 1,
        }
    }
    fn state(&self) -> ButtonState {
        match self.state {
            ElementState::Pressed => ButtonState::Pressed,
            ElementState::Released => ButtonState::Released,
        }
    }
}
impl AbsolutePositionEvent<NativeInput> for TouchPositionEvent {
    fn x(&self) -> f64 {
        self.position.x
    }
    fn y(&self) -> f64 {
        self.position.y
    }
    fn x_transformed(&self, width: i32) -> f64 {
        (self.position.x / self.size.0 * width as f64).max(0.0)
    }
    fn y_transformed(&self, height: i32) -> f64 {
        (self.position.y / self.size.1 * height as f64).max(0.0)
    }
}
impl TouchEvent<NativeInput> for TouchPositionEvent {
    fn slot(&self) -> TouchSlot {
        Some(self.id as u32).into()
    }
}
impl TouchDownEvent<NativeInput> for TouchPositionEvent {}
impl TouchMotionEvent<NativeInput> for TouchPositionEvent {}
impl TouchEvent<NativeInput> for TouchEndEvent {
    fn slot(&self) -> TouchSlot {
        Some(self.id as u32).into()
    }
}
impl TouchUpEvent<NativeInput> for TouchEndEvent {}
impl TouchCancelEvent<NativeInput> for TouchEndEvent {}
impl TouchFrameEvent<NativeInput> for TouchFrame {}

impl InputBackend for NativeInput {
    type Device = NativeDevice;
    type KeyboardKeyEvent = KeyEvent;
    type PointerAxisEvent = WheelEvent;
    type PointerButtonEvent = ButtonEvent;
    type PointerMotionEvent = UnusedEvent;
    type PointerMotionAbsoluteEvent = MotionEvent;
    type GestureSwipeBeginEvent = UnusedEvent;
    type GestureSwipeUpdateEvent = UnusedEvent;
    type GestureSwipeEndEvent = UnusedEvent;
    type GesturePinchBeginEvent = UnusedEvent;
    type GesturePinchUpdateEvent = UnusedEvent;
    type GesturePinchEndEvent = UnusedEvent;
    type GestureHoldBeginEvent = UnusedEvent;
    type GestureHoldEndEvent = UnusedEvent;
    type TouchDownEvent = TouchPositionEvent;
    type TouchUpEvent = TouchEndEvent;
    type TouchMotionEvent = TouchPositionEvent;
    type TouchCancelEvent = TouchEndEvent;
    type TouchFrameEvent = TouchFrame;
    type TabletToolAxisEvent = UnusedEvent;
    type TabletToolProximityEvent = UnusedEvent;
    type TabletToolTipEvent = UnusedEvent;
    type TabletToolButtonEvent = UnusedEvent;
    type SwitchToggleEvent = UnusedEvent;
    type SpecialEvent = UnusedEvent;
}

#[derive(Debug)]
pub enum NativeWindowEvent {
    Resized(Size<i32, Physical>),
    Input(InputEvent<NativeInput>),
    Focus,
    Redraw,
    CloseRequested,
}

struct WindowState {
    attributes: WindowAttributes,
    window: Option<Arc<dyn Window>>,
    creation_error: Option<smithay::reexports::winit::error::RequestError>,
    key_count: u32,
    is_x11: bool,
}

impl WindowState {
    fn timestamp(&self) -> u64 {
        get_monotonic_time().as_micros() as u64
    }
    fn dimensions(&self) -> (f64, f64) {
        let size = self.window.as_ref().unwrap().surface_size();
        (f64::from(size.width.max(1)), f64::from(size.height.max(1)))
    }
}

struct WindowApp<'a, F: FnMut(NativeWindowEvent)> {
    state: &'a mut WindowState,
    callback: F,
}

impl<F: FnMut(NativeWindowEvent)> WindowApp<'_, F> {
    fn touch_event(&mut self, event: InputEvent<NativeInput>) {
        let frame_time = match &event {
            InputEvent::TouchDown { event } | InputEvent::TouchMotion { event } => Some(event.time),
            InputEvent::TouchUp { event } => Some(event.time),
            InputEvent::TouchCancel { .. } => None,
            _ => unreachable!("only touch events are dispatched here"),
        };
        (self.callback)(NativeWindowEvent::Input(event));
        // Winit reports individual touch events, without a separate frame boundary.
        // Finish each atomic update so both compositor grabs and clients see it now.
        if let Some(time) = frame_time {
            (self.callback)(NativeWindowEvent::Input(InputEvent::TouchFrame {
                event: TouchFrame { time },
            }));
        }
    }
}

impl<F: FnMut(NativeWindowEvent)> ApplicationHandler for WindowApp<'_, F> {
    fn can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        if self.state.window.is_none() {
            match event_loop.create_window(self.state.attributes.clone()) {
                Ok(window) => self.state.window = Some(Arc::from(window)),
                Err(err) => self.state.creation_error = Some(err),
            }
            (self.callback)(NativeWindowEvent::Input(InputEvent::DeviceAdded {
                device: NativeDevice,
            }));
        }
    }

    fn window_event(&mut self, _: &dyn ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(window) = self.state.window.as_ref() else {
            return;
        };
        let time = self.state.timestamp();
        match event {
            WindowEvent::SurfaceResized(size) => {
                (self.callback)(NativeWindowEvent::Resized(
                    (size.width as i32, size.height as i32).into(),
                ));
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                let size = window.surface_size();
                (self.callback)(NativeWindowEvent::Resized(
                    (size.width as i32, size.height as i32).into(),
                ));
            }
            WindowEvent::RedrawRequested => (self.callback)(NativeWindowEvent::Redraw),
            WindowEvent::CloseRequested => (self.callback)(NativeWindowEvent::CloseRequested),
            WindowEvent::Focused(_) => (self.callback)(NativeWindowEvent::Focus),
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } if !event.repeat => {
                match event.state {
                    ElementState::Pressed => self.state.key_count += 1,
                    ElementState::Released => {
                        self.state.key_count = self.state.key_count.saturating_sub(1)
                    }
                }
                (self.callback)(NativeWindowEvent::Input(InputEvent::Keyboard {
                    event: KeyEvent {
                        time,
                        key: event.physical_key.to_scancode().unwrap_or(0),
                        count: self.state.key_count,
                        state: event.state,
                    },
                }));
            }
            WindowEvent::PointerMoved {
                position, source, ..
            } => {
                let size = self.state.dimensions();
                let event = MotionEvent {
                    time,
                    position,
                    size,
                };
                match source {
                    PointerSource::Mouse => (self.callback)(NativeWindowEvent::Input(
                        InputEvent::PointerMotionAbsolute { event },
                    )),
                    PointerSource::Touch { finger_id, .. } => {
                        self.touch_event(InputEvent::TouchMotion {
                            event: TouchPositionEvent {
                                time,
                                id: finger_id.into_raw() as u64,
                                position,
                                size,
                            },
                        })
                    }
                    _ => (),
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                (self.callback)(NativeWindowEvent::Input(InputEvent::PointerAxis {
                    event: WheelEvent { time, delta },
                }));
            }
            WindowEvent::PointerButton {
                state,
                button,
                position,
                ..
            } => match button {
                ButtonSource::Mouse(button) => {
                    (self.callback)(NativeWindowEvent::Input(InputEvent::PointerButton {
                        event: ButtonEvent {
                            time,
                            button,
                            state,
                            is_x11: self.state.is_x11,
                        },
                    }))
                }
                ButtonSource::Touch { finger_id, .. } => {
                    let id = finger_id.into_raw() as u64;
                    match state {
                        ElementState::Pressed => self.touch_event(InputEvent::TouchDown {
                            event: TouchPositionEvent {
                                time,
                                id,
                                position,
                                size: self.state.dimensions(),
                            },
                        }),
                        ElementState::Released => self.touch_event(InputEvent::TouchUp {
                            event: TouchEndEvent { time, id },
                        }),
                    }
                }
                _ => (),
            },
            WindowEvent::PointerLeft {
                kind: PointerKind::Touch(finger_id),
                ..
            } => {
                self.touch_event(InputEvent::TouchCancel {
                    event: TouchEndEvent {
                        time,
                        id: finger_id.into_raw() as u64,
                    },
                });
            }
            _ => (),
        }
    }
}

/// Owns Winit's event loop and presents input events to Calloop.
pub struct NativeWindowLoop {
    state: WindowState,
    event_loop: Generic<EventLoop>,
    pending: Vec<NativeWindowEvent>,
    initial: Vec<NativeWindowEvent>,
    fake_token: Option<Token>,
}

impl NativeWindowLoop {
    pub fn create(attributes: WindowAttributes) -> anyhow::Result<(Arc<dyn Window>, Self)> {
        let mut event_loop = EventLoop::builder().build()?;
        let mut state = WindowState {
            attributes: attributes.with_surface_size(LogicalSize::new(1280.0, 800.0)),
            window: None,
            creation_error: None,
            key_count: 0,
            is_x11: false,
        };
        let mut initial = Vec::new();
        while state.window.is_none() {
            let status = event_loop.pump_app_events(
                None,
                &mut WindowApp {
                    state: &mut state,
                    callback: |event| initial.push(event),
                },
            );
            if let Some(err) = state.creation_error.take() {
                return Err(err.into());
            }
            if let PumpStatus::Exit(code) = status {
                anyhow::bail!("host Winit event loop exited before window creation: {code:?}");
            }
        }
        let window = state.window.as_ref().unwrap().clone();
        use smithay::reexports::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        state.is_x11 = matches!(
            window.window_handle()?.as_raw(),
            RawWindowHandle::Xlib(_) | RawWindowHandle::Xcb(_)
        );
        event_loop.set_control_flow(ControlFlow::Poll);
        Ok((
            window,
            Self {
                state,
                event_loop: Generic::new(event_loop, Interest::READ, Mode::Level),
                pending: Vec::new(),
                initial,
                fake_token: None,
            },
        ))
    }

    fn dispatch(&mut self, mut callback: impl FnMut(NativeWindowEvent)) -> PumpStatus {
        for event in self.initial.drain(..) {
            callback(event);
        }
        // SAFETY: Winit's event loop is owned until this EventSource is dropped.
        unsafe { self.event_loop.get_mut() }.pump_app_events(
            Some(Duration::ZERO),
            &mut WindowApp {
                state: &mut self.state,
                callback,
            },
        )
    }
}

impl EventSource for NativeWindowLoop {
    type Event = NativeWindowEvent;
    type Metadata = ();
    type Ret = ();
    type Error = io::Error;
    const NEEDS_EXTRA_LIFECYCLE_EVENTS: bool = true;

    fn before_sleep(&mut self) -> calloop::Result<Option<(Readiness, Token)>> {
        let mut pending = std::mem::take(&mut self.pending);
        self.dispatch(|event| pending.push(event));
        self.pending = pending;
        Ok(if self.pending.is_empty() {
            None
        } else {
            Some((Readiness::EMPTY, self.fake_token.unwrap()))
        })
    }

    fn process_events<F>(
        &mut self,
        _: Readiness,
        _: Token,
        mut callback: F,
    ) -> Result<PostAction, Self::Error>
    where
        F: FnMut(Self::Event, &mut Self::Metadata) -> Self::Ret,
    {
        for event in self.pending.drain(..) {
            callback(event, &mut ());
        }
        Ok(match self.dispatch(|event| callback(event, &mut ())) {
            PumpStatus::Continue => PostAction::Continue,
            PumpStatus::Exit(_) => PostAction::Remove,
        })
    }

    fn register(
        &mut self,
        poll: &mut calloop::Poll,
        tokens: &mut TokenFactory,
    ) -> calloop::Result<()> {
        self.fake_token = Some(tokens.token());
        self.event_loop.register(poll, tokens)
    }
    fn reregister(
        &mut self,
        poll: &mut calloop::Poll,
        tokens: &mut TokenFactory,
    ) -> calloop::Result<()> {
        self.event_loop.reregister(poll, tokens)
    }
    fn unregister(&mut self, poll: &mut calloop::Poll) -> calloop::Result<()> {
        self.event_loop.unregister(poll)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_touch_events_are_followed_by_a_frame() {
        let mut state = WindowState {
            attributes: WindowAttributes::default(),
            window: None,
            creation_error: None,
            key_count: 0,
            is_x11: false,
        };
        let mut delivered = Vec::new();
        let mut app = WindowApp {
            state: &mut state,
            callback: |event| {
                let NativeWindowEvent::Input(event) = event else {
                    panic!("unexpected window event");
                };
                delivered.push(match event {
                    InputEvent::TouchDown { event } => ("down", event.time()),
                    InputEvent::TouchMotion { event } => ("motion", event.time()),
                    InputEvent::TouchUp { event } => ("up", event.time()),
                    InputEvent::TouchFrame { event } => ("frame", event.time()),
                    InputEvent::TouchCancel { event } => ("cancel", event.time()),
                    _ => panic!("unexpected input event"),
                });
            },
        };
        let position = TouchPositionEvent {
            time: 1000,
            id: 7,
            position: PhysicalPosition::new(20., 30.),
            size: (1280., 800.),
        };
        app.touch_event(InputEvent::TouchDown { event: position });
        app.touch_event(InputEvent::TouchMotion {
            event: TouchPositionEvent {
                time: 2000,
                position: PhysicalPosition::new(40., 60.),
                ..position
            },
        });
        app.touch_event(InputEvent::TouchUp {
            event: TouchEndEvent { time: 3000, id: 7 },
        });
        app.touch_event(InputEvent::TouchCancel {
            event: TouchEndEvent { time: 4000, id: 7 },
        });
        assert_eq!(
            delivered,
            [
                ("down", 1000),
                ("frame", 1000),
                ("motion", 2000),
                ("frame", 2000),
                ("up", 3000),
                ("frame", 3000),
                ("cancel", 4000),
            ]
        );
    }
}
