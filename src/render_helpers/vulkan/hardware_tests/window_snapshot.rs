//! Capture must reject deferred preparation rather than retain a transparent or
//! partial three-target image. Exact Wayland attachment ownership is covered by
//! the separate explicit-sync pre-commit/renderer-epoch hardware regressions.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::{Bind, ExportMem, Texture};
use smithay::output::{self, Output};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Scale, Serial, Size};

use super::setup;
use crate::animation::{Animation, Clock};
use crate::layout::opening_window::OpenAnimation;
use crate::layout::window_presentation::WindowPresentationState;
use crate::layout::{
    ConfigureIntent, InteractiveResizeData, LayoutElement, LayoutElementRenderElement,
    LayoutElementRenderSnapshot, Options, SizingMode,
};
use crate::render_helpers::offscreen::OffscreenData;
use crate::render_helpers::renderer::ClarionRenderer;
use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
use crate::render_helpers::vulkan::native::{
    GpuPreparationGateForTest, NativeError, NativeRenderer,
};
use crate::render_helpers::xray::XrayPos;
use crate::render_helpers::{RenderCtx, RenderTarget};
use crate::scene::tile::TileScene;
use crate::scene::window_decorations::WindowDecorations;
use crate::scene::window_resources::{NativeTileSnapshot, WindowResourceRegistry};
use crate::scene::window_snapshot::{capture_native, store_native_unmap_if_empty};
use crate::utils::transaction::Transaction;
use crate::window::ResolvedWindowRules;

struct CaptureWindow {
    rules: ResolvedWindowRules,
    colors: [SolidColorBuffer; 3],
    visits: RefCell<Vec<RenderTarget>>,
    defer_target: Cell<Option<RenderTarget>>,
}

impl CaptureWindow {
    fn new() -> Self {
        Self {
            rules: Default::default(),
            colors: [
                SolidColorBuffer::new((85., 53.), [1., 0., 0., 1.]),
                SolidColorBuffer::new((85., 53.), [0., 1., 0., 1.]),
                SolidColorBuffer::new((85., 53.), [0., 0., 1., 1.]),
            ],
            visits: RefCell::new(Vec::new()),
            defer_target: Cell::new(None),
        }
    }
}

impl LayoutElement for CaptureWindow {
    type Id = u64;
    fn id(&self) -> &u64 {
        &1
    }
    fn core_id(&self) -> u64 {
        1
    }
    fn size(&self) -> Size<i32, Logical> {
        (85, 53).into()
    }
    fn buf_loc(&self) -> Point<i32, Logical> {
        (0, 0).into()
    }
    fn is_in_input_region(&self, _: Point<f64, Logical>) -> bool {
        false
    }
    fn request_size(
        &mut self,
        _: Size<i32, Logical>,
        _: SizingMode,
        _: bool,
        _: Option<Transaction>,
    ) {
    }
    fn min_size(&self) -> Size<i32, Logical> {
        (0, 0).into()
    }
    fn max_size(&self) -> Size<i32, Logical> {
        (0, 0).into()
    }
    fn is_wl_surface(&self, _: &WlSurface) -> bool {
        false
    }
    fn has_ssd(&self) -> bool {
        false
    }
    fn set_preferred_scale_transform(&self, _: output::Scale, _: smithay::utils::Transform) {}
    fn output_enter(&self, _: &Output) {}
    fn output_leave(&self, _: &Output) {}
    fn set_offscreen_data(&self, _: Option<OffscreenData>) {}
    fn set_activated(&mut self, _: bool) {}
    fn set_active_in_column(&mut self, _: bool) {}
    fn set_floating(&mut self, _: bool) {}
    fn set_bounds(&self, _: Size<i32, Logical>) {}
    fn is_ignoring_opacity_window_rule(&self) -> bool {
        false
    }
    fn is_urgent(&self) -> bool {
        false
    }
    fn configure_intent(&self) -> ConfigureIntent {
        ConfigureIntent::CanSend
    }
    fn send_pending_configure(&mut self) {}
    fn sizing_mode(&self) -> SizingMode {
        SizingMode::Normal
    }
    fn pending_sizing_mode(&self) -> SizingMode {
        SizingMode::Normal
    }
    fn requested_size(&self) -> Option<Size<i32, Logical>> {
        None
    }
    fn is_child_of(&self, _: &Self) -> bool {
        false
    }
    fn rules(&self) -> &ResolvedWindowRules {
        &self.rules
    }
    fn refresh(&self) {}
    fn take_animation_snapshot(&mut self) -> Option<LayoutElementRenderSnapshot> {
        None
    }
    fn set_interactive_resize(&mut self, _: Option<InteractiveResizeData>) {}
    fn cancel_interactive_resize(&mut self) {}
    fn interactive_resize_data(&self) -> Option<InteractiveResizeData> {
        None
    }
    fn on_commit(&mut self, _: Serial) {}

    fn render_normal<R: ClarionRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        location: Point<f64, Logical>,
        _scale: Scale<f64>,
        alpha: f32,
        push: &mut dyn FnMut(LayoutElementRenderElement<R>),
    ) {
        self.visits.borrow_mut().push(ctx.target);
        if self.defer_target.get() == Some(ctx.target) {
            let target = ctx.target;
            let native = ctx
                .as_native()
                .expect("hardware test needs the native path")
                .renderer;
            let _scope = native.async_offscreen_scope();
            // A real allocation request behind the gated GPU worker marks this
            // specific target deferred; no mock renderer flag is set by the test.
            let pending = native.create_offscreen_capacity(
                Fourcc::Abgr8888,
                (257 + target as i32 * 512, 193).into(),
            );
            assert!(pending.as_ref().is_err_and(|error| error.is_busy()));
            return;
        }
        push(
            SolidColorRenderElement::from_buffer(
                &self.colors[ctx.target as usize],
                location,
                alpha,
                Kind::Unspecified,
            )
            .into(),
        );
    }
}

fn assert_rgb_targets(native: &mut NativeRenderer, snapshot: NativeTileSnapshot) {
    for (channel, (mut texture, _offset)) in snapshot.targets.into_iter().enumerate() {
        let size = texture.size();
        assert!(
            size.w >= 80 && size.h >= 50,
            "a deferred target must not become a tiny transparent snapshot"
        );
        let framebuffer = native.bind(&mut texture).unwrap();
        let mapping = native
            .copy_framebuffer(
                &framebuffer,
                smithay::utils::Rectangle::from_size(size),
                Fourcc::Abgr8888,
            )
            .unwrap();
        let pixels = native.map_texture(&mapping).unwrap();
        assert!(
            pixels.as_chunks::<4>().0.iter().any(|pixel| {
                pixel[channel] > 240
                    && pixel[3] > 240
                    && (0..3)
                        .filter(|other| *other != channel)
                        .all(|other| pixel[other] < 10)
            }),
            "target {channel} must retain its own complete colored pixels"
        );
    }
}

// Separate fault-injection phases must not compete with preparation still
// finishing for the previous successful capture. Wait outside the capture
// calls being checked; the actual gate still runs on the real bounded worker.
fn gate_after_pending_work(native: &mut NativeRenderer) -> GpuPreparationGateForTest {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        native.poll_preparation_for_test().unwrap();
        match native.gate_gpu_preparation_for_test() {
            Ok(gate) => return gate,
            Err(error) => {
                assert!(
                    Instant::now() < deadline,
                    "GPU gate did not become ready: {error:#}"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
}

#[test]
#[ignore = "requires real DRM render node and native Vulkan"]
fn native_window_snapshot_deferral_is_atomic_and_parent_flag_survives() {
    let (vk, reference, allocator) = setup();
    drop((reference, allocator));
    let mut native = NativeRenderer::new(vk).unwrap();
    native.set_completion_export_required(false);
    native
        .prepare_output_format_for_test(Fourcc::Abgr8888)
        .unwrap();
    let owner = WindowResourceRegistry::new();
    let handle = owner.allocator().allocate();
    let mut options = Options::default();
    options.layout.border.off = true;
    options.layout.focus_ring.off = true;
    options.layout.shadow.on = false;
    let mut clock = Clock::with_time(Duration::ZERO);
    let open_config = options.animations.window_open.clone();
    let state = WindowPresentationState {
        sizing_mode: SizingMode::Normal,
        open_animation: Some(OpenAnimation::new(
            Animation::new(clock.clone(), 0., 1., 0., open_config.anim),
            open_config.popin_ratio,
            open_config.glide,
            true,
            open_config.effect,
        )),
        resize_animation: None,
        alpha_animation: None,
        wobble: Default::default(),
        genie: Default::default(),
        view_size: (500., 400.).into(),
        scale: 1.,
        clock: clock.clone(),
        border_config: options.layout.border.into(),
        focus_ring_config: options.layout.focus_ring,
        options: Rc::new(options),
    };
    *handle.lease().decorations.borrow_mut() = WindowDecorations::new(
        state.border_config,
        state.focus_ring_config,
        state.options.layout.shadow,
        2.,
    );
    clock.set_unadjusted(Duration::from_millis(90));
    let window = CaptureWindow::new();
    let scene = TileScene {
        window: &window,
        state: &state,
        resources: handle.lease(),
    };
    let progress = state.open_animation.as_ref().unwrap().progress();

    // The actual opening effect's first retained target is cold. Its Busy
    // branch emits no element and previously could be captured as empty pixels.
    native.begin_scene();
    let gate = gate_after_pending_work(&mut native);
    {
        let _async = native.async_offscreen_scope();
        let result = capture_native(&scene, &mut native, None, XrayPos::default());
        assert!(result
            .as_ref()
            .is_err_and(|error| NativeError::is_backpressure(error.as_ref())));
    }
    assert!(native.scene_was_deferred());
    assert_eq!(*window.visits.borrow(), [RenderTarget::Output]);
    assert!(handle.lease().native_unmap.borrow().is_none());
    assert_eq!(state.open_animation.as_ref().unwrap().progress(), progress);
    drop(gate);

    // Keep the inherited flag true: a successful independent capture must
    // neither treat it as its own failure nor clear the enclosing retry request.
    let deadline = Instant::now() + Duration::from_secs(15);
    let snapshot = loop {
        native.poll_preparation_for_test().unwrap();
        match capture_native(&scene, &mut native, None, XrayPos::default()) {
            Ok(snapshot) => break snapshot,
            Err(error) if NativeError::is_backpressure(error.as_ref()) => {}
            Err(error) => panic!("capture retry failed without backpressure: {error:#}"),
        }
        assert!(
            Instant::now() < deadline,
            "opening capture did not become ready"
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    assert!(native.scene_was_deferred());
    assert_rgb_targets(&mut native, snapshot);

    for deferred in [RenderTarget::Screencast, RenderTarget::ScreenCapture] {
        native.retire_completed().unwrap();
        window.visits.borrow_mut().clear();
        window.defer_target.set(Some(deferred));
        let gate = gate_after_pending_work(&mut native);
        assert!(!store_native_unmap_if_empty(
            &scene,
            &handle,
            &mut native,
            None,
            false,
            XrayPos::default(),
        ));
        assert_eq!(
            window.visits.borrow().len(),
            deferred as usize + 1,
            "later privacy targets must not be visited after deferral"
        );
        assert!(
            handle.lease().native_unmap.borrow().is_none(),
            "no partial target set may enter the close cache"
        );
        assert!(native.scene_was_deferred());
        assert_eq!(state.open_animation.as_ref().unwrap().progress(), progress);
        drop(gate);
        window.defer_target.set(None);
        let deadline = Instant::now() + Duration::from_secs(15);
        while !store_native_unmap_if_empty(
            &scene,
            &handle,
            &mut native,
            None,
            false,
            XrayPos::default(),
        ) {
            native.poll_preparation_for_test().unwrap();
            assert!(
                Instant::now() < deadline,
                "deferred capture failed to retry"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        let snapshot = handle.lease().native_unmap.borrow_mut().take().unwrap();
        assert_rgb_targets(&mut native, snapshot);
        assert!(native.scene_was_deferred());
    }
}
