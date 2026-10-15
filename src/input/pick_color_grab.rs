use realand_ipc::PickedColor;
use smithay::backend::allocator::Fourcc;
use smithay::backend::input::ButtonState;
use smithay::backend::renderer::element::utils::{Relocate, RelocateRenderElement};
use smithay::backend::renderer::{Bind, Offscreen, RendererSuper, Texture};
use smithay::input::pointer::{
    AxisFrame, ButtonEvent, CursorImageStatus, GestureHoldBeginEvent, GestureHoldEndEvent,
    GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent,
    GestureSwipeEndEvent, GestureSwipeUpdateEvent, GrabStartData as PointerGrabStartData,
    MotionEvent, PointerGrab, PointerInnerHandle, RelativeMotionEvent,
};
use smithay::input::SeatHandler;
use smithay::output::Output;
use smithay::utils::{Logical, Physical, Point, Scale, Size, Transform};

use crate::compositor::State;
use crate::platform::pixel_readback::{DeferredCaptureRenderer, PendingPixels};
use crate::render_helpers::{render_to_texture, RenderCtx, RenderTarget};

pub struct PickColorGrab {
    start_data: PointerGrabStartData<State>,
}

impl PickColorGrab {
    pub fn new(start_data: PointerGrabStartData<State>) -> Self {
        Self { start_data }
    }

    fn on_ungrab(&mut self, state: &mut State) {
        if let Some(tx) = state.realand.pick_color.take() {
            let _ = tx.try_send(None);
        }
        state
            .realand
            .cursor_manager
            .set_cursor_image(CursorImageStatus::default_named());
        state.realand.queue_redraw_all();
    }

    pub(crate) fn pick_color_at_point(
        location: Point<f64, Logical>,
        data: &mut State,
        reply: async_channel::Sender<Option<PickedColor>>,
    ) -> anyhow::Result<()> {
        let (output, pos_within_output) = data
            .realand
            .output_under(location)
            .ok_or_else(|| anyhow::anyhow!("color picker output disappeared"))?;
        let output = output.clone();
        let permit = data.realand.capture_permit()?;
        let job = data
            .backend
            .with_renderer_for_output(Some(&output), |renderer| match renderer {
                crate::render_helpers::renderer::AnyRenderer::Gles(gles) => {
                    Self::sample_pixel(gles, &mut data.realand, &output, pos_within_output)
                }
                crate::render_helpers::renderer::AnyRenderer::Native(native) => {
                    Self::sample_pixel(native, &mut data.realand, &output, pos_within_output)
                }
            })
            .ok_or_else(|| anyhow::anyhow!("color picker renderer disappeared"))??;
        data.realand
            .enqueue_pixel_readback(Some(output), job, move |state, result| {
                let _permit = permit;
                let color = if state.realand.is_locked() {
                    None
                } else {
                    result
                        .ok()
                        .filter(|pixels| pixels.len() == 4)
                        .map(|pixels| PickedColor {
                            rgb: [
                                f64::from(pixels[0]) / 255.,
                                f64::from(pixels[1]) / 255.,
                                f64::from(pixels[2]) / 255.,
                            ],
                        })
                };
                let _ = reply.try_send(color);
            })
    }

    fn sample_pixel<R>(
        renderer: &mut R,
        realand: &mut crate::compositor::Realand,
        output: &Output,
        pos_within_output: Point<f64, Logical>,
    ) -> anyhow::Result<PendingPixels>
    where
        R: DeferredCaptureRenderer
            + Offscreen<<R as RendererSuper>::TextureId>
            + Bind<<R as RendererSuper>::TextureId>,
        R::Error: Send + Sync + 'static,
        crate::compositor::OutputRenderElements<R>:
            smithay::backend::renderer::element::RenderElement<R>,
        for<'a> R::Framebuffer<'a>: Texture,
    {
        realand.update_render_elements(Some(output));

        let scale = Scale::from(output.current_scale().fractional_scale());
        // FIXME: perhaps replace floor with round once we figure out the pointer behavior
        // at the bottom/right edges of the monitors.
        let pos = pos_within_output.to_physical_precise_floor(scale);
        let size = Size::<i32, Physical>::from((1, 1));

        let ctx = RenderCtx {
            renderer,
            // This is an interactive operation so we can render without blocking out.
            target: RenderTarget::Output,
            xray: None,
        };
        let elements = realand.render_to_vec(ctx, output, false);

        let (texture, _) = render_to_texture(
            renderer,
            size,
            scale,
            Transform::Normal,
            Fourcc::Abgr8888,
            elements.iter().rev().map(|elem| {
                RelocateRenderElement::from_element(elem, pos.upscale(-1), Relocate::Relative)
            }),
        )?;
        renderer.enqueue_pixels(
            &texture,
            smithay::utils::Rectangle::from_size(texture.size()),
            Fourcc::Abgr8888,
        )
    }
}

impl PointerGrab<State> for PickColorGrab {
    fn motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        _focus: Option<(<State as SeatHandler>::PointerFocus, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);
    }

    fn relative_motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        _focus: Option<(<State as SeatHandler>::PointerFocus, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, None, event);
    }

    fn button(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &ButtonEvent,
    ) {
        if event.state != ButtonState::Pressed {
            return;
        }

        // We're handling this press, don't send the release to the window.
        data.realand.suppressed_buttons.insert(event.button);

        if let Some(tx) = data.realand.pick_color.take() {
            if let Err(error) =
                Self::pick_color_at_point(handle.current_location(), data, tx.clone())
            {
                warn!("cannot start color readback: {error:#}");
                let _ = tx.try_send(None);
            }
        }

        handle.unset_grab(self, data, event.serial, event.time, true);
    }

    fn axis(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        details: AxisFrame,
    ) {
        handle.axis(data, details);
    }

    fn frame(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event);
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event);
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event);
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event);
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event);
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event);
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event);
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event);
    }

    fn start_data(&self) -> &PointerGrabStartData<State> {
        &self.start_data
    }

    fn unset(&mut self, data: &mut State) {
        self.on_ungrab(data);
    }
}
