//! A deferred native target must warm against the same retained swapchain slot.
use std::cell::Cell;
use std::convert::Infallible;
use std::rc::Rc;

use smithay::backend::allocator::{Allocator, Buffer, Format, Fourcc, Modifier, Swapchain};
use smithay::backend::renderer::Renderer;
use smithay::utils::{Buffer as BufferCoords, Size};

use crate::render_helpers::vulkan::native::{NativeBusy, NativeError, NativeRenderer};

#[derive(Debug)]
struct CountedBuffer {
    id: usize,
    size: Size<i32, BufferCoords>,
    format: Format,
}

impl Buffer for CountedBuffer {
    fn size(&self) -> Size<i32, BufferCoords> {
        self.size
    }

    fn format(&self) -> Format {
        self.format
    }
}

struct CountedAllocator(Rc<Cell<usize>>);

impl Allocator for CountedAllocator {
    type Buffer = CountedBuffer;
    type Error = Infallible;

    fn create_buffer(
        &mut self,
        width: u32,
        height: u32,
        fourcc: Fourcc,
        _modifiers: &[Modifier],
    ) -> Result<CountedBuffer, Infallible> {
        let id = self.0.get() + 1;
        self.0.set(id);
        Ok(CountedBuffer {
            id,
            size: (width as i32, height as i32).into(),
            format: Format {
                code: fourcc,
                modifier: Modifier::Linear,
            },
        })
    }
}

#[test]
fn deferred_age_resets_keep_held_slots_and_prepared_userdata() {
    let allocated = Rc::new(Cell::new(0));
    let mut pool = Swapchain::new(
        CountedAllocator(allocated.clone()),
        3840,
        2400,
        Fourcc::Xrgb8888,
        vec![Modifier::Linear],
    );
    let front = pool.acquire().unwrap().unwrap();
    pool.submitted(&front);
    let rendering = pool.acquire().unwrap().unwrap();
    let target_id = rendering.id;
    rendering
        .userdata()
        .insert_if_missing_threadsafe(|| String::from("prepared FP16 scene"));
    let third = pool.acquire().unwrap().unwrap();
    let fourth = pool.acquire().unwrap().unwrap();

    pool.reset_buffer_ages();
    assert_eq!(front.age(), 0);
    assert!(
        pool.acquire().unwrap().is_none(),
        "age reset must not release held buffers"
    );
    assert_eq!(allocated.get(), 4);
    drop(rendering);
    for _ in 0..64 {
        let retry = pool.acquire().unwrap().unwrap();
        assert_eq!(retry.id, target_id);
        assert_eq!(retry.age(), 0);
        assert_eq!(
            retry.userdata().get::<String>().unwrap(),
            "prepared FP16 scene"
        );
        pool.reset_buffer_ages();
        assert!(pool.acquire().unwrap().is_none());
        drop(retry);
    }
    assert_eq!(
        allocated.get(),
        4,
        "retries must not allocate new 4K buffers"
    );
    drop((front, third, fourth));
}

#[test]
fn explicit_buffer_reset_still_discards_unknown_contents() {
    let allocated = Rc::new(Cell::new(0));
    let mut pool = Swapchain::new(
        CountedAllocator(allocated.clone()),
        3840,
        2400,
        Fourcc::Xrgb8888,
        vec![Modifier::Linear],
    );
    let front = pool.acquire().unwrap().unwrap();
    pool.submitted(&front);
    pool.reset_buffers();
    let replacement = pool.acquire().unwrap().unwrap();
    assert_ne!(replacement.id, front.id);
    assert_eq!(
        front.age(),
        1,
        "old in-flight owners retain their original slot"
    );
    assert_eq!(allocated.get(), 2);
}

#[test]
fn native_deferred_policy_never_classifies_a_hard_error_as_retryable() {
    for busy in [
        NativeBusy::Frame,
        NativeBusy::Upload,
        NativeBusy::Completion,
        NativeBusy::OutputFormat,
        NativeBusy::YuvPipelines,
        NativeBusy::ProducerFence,
        NativeBusy::OffscreenAllocation,
    ] {
        assert!(NativeRenderer::is_render_deferred(&NativeError::Busy(busy)));
    }
    assert!(!NativeRenderer::is_render_deferred(&NativeError::Other(
        String::from("device lost")
    )));
}
