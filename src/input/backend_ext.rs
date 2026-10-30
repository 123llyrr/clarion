use ::input as libinput;
use smithay::backend::input;
use smithay::backend::winit::WinitVirtualDevice;
use smithay::output::Output;
use smithay::wayland::virtual_keyboard::VirtualKeyboardDevice;

use crate::compositor::State;
use crate::protocols::virtual_pointer::VirtualPointer;

pub trait ClarionInputBackend: input::InputBackend<Device = Self::ClarionDevice> {
    type ClarionDevice: ClarionInputDevice;
}
impl<T: input::InputBackend> ClarionInputBackend for T
where
    Self::Device: ClarionInputDevice,
{
    type ClarionDevice = Self::Device;
}

pub trait ClarionInputDevice: input::Device {
    // FIXME: this should maybe be per-event, not per-device,
    // but it's not clear that this matters in practice?
    // it might be more obvious once we implement it for libinput
    fn output(&self, state: &State) -> Option<Output>;
}

impl ClarionInputDevice for libinput::Device {
    fn output(&self, _state: &State) -> Option<Output> {
        // FIXME: Allow specifying the output per-device?
        None
    }
}

impl ClarionInputDevice for WinitVirtualDevice {
    fn output(&self, _state: &State) -> Option<Output> {
        // FIXME: we should be returning the single output that the winit backend creates,
        // but for now, that will cause issues because the output is normally upside down,
        // so we apply Transform::Flipped180 to it and that would also cause
        // the cursor position to be flipped, which is not what we want.
        //
        // instead, we just return None and rely on the fact that it has only one output.
        // doing so causes the cursor to be placed in *global* output coordinates,
        // which are not flipped, and happen to be what we want.
        None
    }
}

impl ClarionInputDevice for VirtualPointer {
    fn output(&self, _: &State) -> Option<Output> {
        self.output().cloned()
    }
}

impl ClarionInputDevice for VirtualKeyboardDevice {
    fn output(&self, _: &State) -> Option<Output> {
        None
    }
}
