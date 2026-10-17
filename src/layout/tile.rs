use core::f64;
use std::cell::RefCell;
use std::rc::Rc;

use clarion_config::utils::MergeWith as _;
use clarion_config::CornerRadius;
use clarion_ipc::WindowLayout;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::{Logical, Point, Rectangle, Size};

use super::opening_window::OpenAnimation;
use super::window_presentation::{
    resize_decoration_animation, AlphaAnimation, ResizeAnimation, ResizeMomentum,
    WindowPresentationState,
};
use super::wobble::{WobbleAnchor, WobbleState};
use super::{HitType, LayoutElement, Options, SizeFrac, RESIZE_ANIMATION_THRESHOLD};
use crate::animation::{Animation, Clock};
use crate::layout::SizingMode;
use crate::render_helpers::renderer::{AnyRenderer, ClarionRenderer};
use crate::render_helpers::xray::{Xray, XrayPos};
use crate::render_helpers::RenderCtx;
#[cfg(test)]
pub(super) use crate::scene::tile::NativeEffect;
use crate::scene::window_decorations::DecorationUpdate;
use crate::scene::window_resources::{ResizeImages, ResizeSnapshot, WindowResourceHandle};
use crate::utils::round_logical_in_physical_max1;
use crate::utils::transaction::Transaction;

/// Desktop-owned window identity and placement, with renderer-free presentation.
/// Retained decoration buffers, animation images and meshes belong to the scene.
#[derive(Debug)]
pub struct Tile<W: LayoutElement> {
    window: W,
    pub(super) presentation: WindowPresentationState,
    pub(super) restore_to_floating: bool,
    /// Last floating size and position survive layout mode switches.
    pub(super) floating_window_size: Option<Size<i32, Logical>>,
    pub(super) floating_pos: Option<Point<f64, SizeFrac>>,
    pub(super) floating_preset_width_idx: Option<usize>,
    pub(super) floating_preset_height_idx: Option<usize>,
    move_x_animation: Option<MoveAnimation>,
    move_y_animation: Option<MoveAnimation>,
    /// Destination supplied by the Shell in output logical coordinates.
    minimize_target: Option<clarion_ipc::DockTarget>,
    minimize_reduce_motion: bool,
    pub(super) interactive_move_offset: Point<f64, Logical>,
    /// Weak registration survives moves and parking without owning GPU caches.
    resources: WindowResourceHandle,
    is_active: bool,
}

pub use crate::scene::tile::{TileRenderElement, TileRenderSnapshot};
pub use crate::scene::window_resources::NativeTileSnapshot;

#[derive(Debug)]
struct MoveAnimation {
    anim: Animation,
    from: f64,
    /// Whether this animation is for moving the tile between workspaces.
    ///
    /// Controls whether the tile is rendered uncropped and above others.
    is_between_workspaces: bool,
}

impl MoveAnimation {
    fn retarget(
        previous: Option<Self>,
        offset: f64,
        clock: Clock,
        config: clarion_config::Animation,
    ) -> Self {
        // Keep an independent amplitude so a zero-distance reversal can still
        // carry momentum, and tiny remaining distances don't amplify velocity.
        let amplitude = offset.abs().max(1.);
        let is_between_workspaces = previous
            .as_ref()
            .is_some_and(|previous| previous.is_between_workspaces);
        let anim = if let Some(previous) = previous {
            let velocity = previous.from * previous.anim.velocity() / amplitude;
            previous.anim.restarted(offset / amplitude, 0., velocity)
        } else {
            Animation::new(clock, offset / amplitude, 0., 0., config)
        };
        Self {
            anim,
            from: amplitude,
            is_between_workspaces,
        }
    }
}

impl<W: LayoutElement> Tile<W> {
    #[cfg(test)]
    pub(crate) fn scene_registration(
        &self,
    ) -> (u64, crate::scene::window_resources::WindowResourceLease) {
        (self.resources.registration_key(), self.resources.lease())
    }

    pub(crate) fn resolved_corner_radius(&self) -> CornerRadius {
        self.presentation.resolved_corner_radius(&self.window)
    }

    fn rounding_power(&self) -> f32 {
        self.presentation.rounding_power()
    }

    pub fn new(
        window: W,
        view_size: Size<f64, Logical>,
        scale: f64,
        clock: Clock,
        options: Rc<Options>,
        resources: WindowResourceHandle,
    ) -> Self {
        let rules = window.rules();
        let border_config = options.layout.border.merged_with(&rules.border);
        let focus_ring_config = options.layout.focus_ring.merged_with(&rules.focus_ring);
        let shadow_config = options.layout.shadow.merged_with(&rules.shadow);
        let sizing_mode = window.sizing_mode();
        let rounding_power = options.layout.rounding_power.max(2.) as f32;
        resources.lease().decorations.borrow_mut().configure(
            border_config.into(),
            focus_ring_config,
            shadow_config,
            rounding_power,
        );
        Self {
            window,
            presentation: WindowPresentationState {
                sizing_mode,
                open_animation: None,
                resize_animation: None,
                alpha_animation: None,
                wobble: RefCell::new(WobbleState::default()),
                genie: RefCell::new(None),
                view_size,
                scale,
                clock,
                options,
                border_config: border_config.into(),
                focus_ring_config,
            },
            restore_to_floating: false,
            floating_window_size: None,
            floating_pos: None,
            floating_preset_width_idx: None,
            floating_preset_height_idx: None,
            move_x_animation: None,
            move_y_animation: None,
            minimize_target: None,
            minimize_reduce_motion: false,
            interactive_move_offset: Point::from((0., 0.)),
            resources,
            is_active: false,
        }
    }

    pub fn update_config(
        &mut self,
        view_size: Size<f64, Logical>,
        scale: f64,
        options: Rc<Options>,
    ) {
        // If preset widths or heights changed, clear our stored preset index.
        if self.presentation.options.layout.preset_column_widths
            != options.layout.preset_column_widths
        {
            self.floating_preset_width_idx = None;
        }
        if self.presentation.options.layout.preset_window_heights
            != options.layout.preset_window_heights
        {
            self.floating_preset_height_idx = None;
        }

        if self.presentation.view_size != view_size
            || self.presentation.scale != scale
            || options.animations.off
        {
            self.complete_genie();
        }
        self.presentation.view_size = view_size;
        self.presentation.scale = scale;
        self.presentation.options = options;

        if self.presentation.options.animations.off
            || self.presentation.options.animations.window_wobble.off
        {
            self.presentation.wobble.borrow_mut().cancel();
            self.clear_wobble_render_cache();
        }

        let round_max1 = |logical| round_logical_in_physical_max1(self.presentation.scale, logical);

        let rules = self.window.rules();

        let mut border_config = self
            .presentation
            .options
            .layout
            .border
            .merged_with(&rules.border);
        border_config.width = round_max1(border_config.width);
        self.presentation.border_config = border_config.into();
        let resources = self.resources.lease();
        let mut decorations = resources.decorations.borrow_mut();
        decorations
            .border
            .update_config(self.presentation.border_config);

        let mut focus_ring_config = self
            .presentation
            .options
            .layout
            .focus_ring
            .merged_with(&rules.focus_ring);
        focus_ring_config.width = round_max1(focus_ring_config.width);
        self.presentation.focus_ring_config = focus_ring_config;
        decorations.focus_ring.update_config(focus_ring_config);

        let shadow_config = self
            .presentation
            .options
            .layout
            .shadow
            .merged_with(&rules.shadow);
        decorations.shadow.update_config(shadow_config);

        self.window.update_config(self.presentation.options.blur);
    }

    /// Retire cached pixels from the previous native-device epoch. Client buffer
    /// leases held by submitted GPU work are retained by that work's Arc owners.
    /// Geometry, focus, minimization and interaction state remain unchanged.
    pub(crate) fn on_native_renderer_replaced(&mut self) {
        self.presentation.resize_animation = None;
        self.resources.lease().clear_native();
        let _ = self.window.take_native_animation_snapshot();
        self.window.set_offscreen_data(None);
    }

    pub fn update_shaders(&mut self) {
        self.resources
            .lease()
            .decorations
            .borrow_mut()
            .update_shaders();
    }

    fn take_resize_snapshot(&mut self) -> Option<ResizeSnapshot> {
        self.window
            .take_animation_snapshot()
            .map(ResizeSnapshot::Gles)
            .or_else(|| {
                self.window
                    .take_native_animation_snapshot()
                    .map(ResizeSnapshot::Native)
            })
    }

    /// Flatten at application time, after acquire/transaction blockers have cleared.
    /// The buffer application captures immutable client pixels without freezing animation time.
    pub fn prepare_resize_retarget(&mut self, renderer: AnyRenderer<'_>) {
        let Some(resize) = &self.presentation.resize_animation else {
            return;
        };
        if resize.anim.is_clamped_done()
            && self
                .presentation
                .options
                .animations
                .window_resize
                .custom_shader
                .is_none()
        {
            return;
        }
        let Some(raw) = self.take_resize_snapshot() else {
            return;
        };
        let captured = self
            .capture_resize_retarget(renderer, &raw)
            .map_err(|err| warn!("error capturing interrupted resize: {err:#}"))
            .ok();
        *self.resources.lease().pending_resize.borrow_mut() = Some((raw, captured));
    }

    fn capture_resize_retarget(
        &self,
        renderer: AnyRenderer<'_>,
        next: &ResizeSnapshot,
    ) -> anyhow::Result<ResizeSnapshot> {
        let resize = self.presentation.resize_animation.as_ref().unwrap();
        crate::scene::resize_snapshot::capture_resize_retarget(
            renderer,
            &self.resources,
            next,
            crate::scene::resize_snapshot::ResizeRetargetPose {
                size: resize.size_value(next.size(), false),
                progress: resize.anim.value() as f32,
                clamped: resize.anim.clamped_value().clamp(0., 1.) as f32,
                scale: self.presentation.scale,
                custom_shader: self
                    .presentation
                    .options
                    .animations
                    .window_resize
                    .custom_shader
                    .as_deref(),
            },
        )
    }

    pub fn update_window(&mut self) {
        let prev_sizing_mode = self.presentation.sizing_mode;
        self.presentation.sizing_mode = self.window.sizing_mode();

        let resources = self.resources.lease();
        let pending = resources.pending_resize.borrow_mut().take();
        if let Some((animate_from, continuous)) =
            pending.or_else(|| self.take_resize_snapshot().map(|raw| (raw, None)))
        {
            let previous_images = resources.resize.borrow_mut().take();
            let buffers = previous_images
                .map(|images| images.buffers)
                .unwrap_or_default();
            let params = if let Some(resize) = self.presentation.resize_animation.take() {
                // Compute like in animated_window_size(), but using the snapshot geometry (since
                // the current one is already overwritten).
                let (size, size_velocity) = resize.size_state(animate_from.size(), false);

                let mut tile_size = animate_from.size();
                if prev_sizing_mode.is_fullscreen() {
                    tile_size.w = f64::max(tile_size.w, self.presentation.view_size.w);
                    tile_size.h = f64::max(tile_size.h, self.presentation.view_size.h);
                } else if prev_sizing_mode.is_normal() && !self.presentation.border_config.off {
                    let width = self.presentation.border_config.width;
                    tile_size.w += width * 2.;
                    tile_size.h += width * 2.;
                }

                let (tile_size, tile_velocity) = resize.size_state(tile_size, true);

                // Also try to reuse the existing offscreen buffers if we have one.
                (
                    size,
                    tile_size,
                    size_velocity,
                    tile_velocity,
                    resize.fullscreen_progress,
                    resize.expanded_progress,
                )
            } else {
                let size = animate_from.size();

                // Compute like in tile_size().
                let mut tile_size = size;
                if prev_sizing_mode.is_fullscreen() {
                    tile_size.w = f64::max(tile_size.w, self.presentation.view_size.w);
                    tile_size.h = f64::max(tile_size.h, self.presentation.view_size.h);
                } else if prev_sizing_mode.is_normal() && !self.presentation.border_config.off {
                    let width = self.presentation.border_config.width;
                    tile_size.w += width * 2.;
                    tile_size.h += width * 2.;
                }

                (
                    size,
                    tile_size,
                    Point::from((0., 0.)),
                    Point::from((0., 0.)),
                    None,
                    None,
                )
            };
            let (
                size_from,
                tile_size_from,
                size_velocity,
                tile_velocity,
                fullscreen_previous,
                expanded_previous,
            ) = params;

            let config = self.presentation.options.animations.window_resize.anim;
            let momentum = ResizeMomentum::new(
                self.presentation.clock.clone(),
                size_velocity,
                tile_velocity,
                config,
            );
            let fullscreen_progress = resize_decoration_animation(
                fullscreen_previous,
                if prev_sizing_mode.is_fullscreen() {
                    1.
                } else {
                    0.
                },
                if self.presentation.sizing_mode.is_fullscreen() {
                    1.
                } else {
                    0.
                },
                self.presentation.clock.clone(),
                config,
            );
            let expanded_progress = resize_decoration_animation(
                expanded_previous,
                if prev_sizing_mode.is_normal() { 0. } else { 1. },
                if self.presentation.sizing_mode.is_normal() {
                    0.
                } else {
                    1.
                },
                self.presentation.clock.clone(),
                config,
            );

            let change = self.window.size().to_f64().to_point() - size_from.to_point();
            let change = f64::max(change.x.abs(), change.y.abs());
            let tile_change = self.tile_size().to_f64().to_point() - tile_size_from.to_point();
            let tile_change = f64::max(tile_change.x.abs(), tile_change.y.abs());
            let change = f64::max(change, tile_change);
            if change > RESIZE_ANIMATION_THRESHOLD
                || momentum.is_some()
                || fullscreen_progress.is_some()
                || expanded_progress.is_some()
            {
                let anim = Animation::new(self.presentation.clock.clone(), 0., 1., 0., config);

                *resources.resize.borrow_mut() = Some(ResizeImages {
                    snapshot: continuous.unwrap_or(animate_from),
                    buffers,
                });
                self.presentation.resize_animation = Some(ResizeAnimation {
                    anim,
                    momentum,
                    size_from,
                    tile_size_from,
                    fullscreen_progress,
                    expanded_progress,
                });
            } else {
                self.presentation.resize_animation = None;
            }
        }

        let round_max1 = |logical| round_logical_in_physical_max1(self.presentation.scale, logical);

        let rules = self.window.rules();
        let mut border_config = self
            .presentation
            .options
            .layout
            .border
            .merged_with(&rules.border);
        border_config.width = round_max1(border_config.width);
        self.presentation.border_config = border_config.into();
        let resources = self.resources.lease();
        let mut decorations = resources.decorations.borrow_mut();
        decorations
            .border
            .update_config(self.presentation.border_config);

        let mut focus_ring_config = self
            .presentation
            .options
            .layout
            .focus_ring
            .merged_with(&rules.focus_ring);
        focus_ring_config.width = round_max1(focus_ring_config.width);
        self.presentation.focus_ring_config = focus_ring_config;
        decorations.focus_ring.update_config(focus_ring_config);

        let shadow_config = self
            .presentation
            .options
            .layout
            .shadow
            .merged_with(&rules.shadow);
        decorations.shadow.update_config(shadow_config);

        let window_size = self.window_size();
        let radius = self
            .resolved_corner_radius()
            .fit_to(window_size.w as f32, window_size.h as f32);
        decorations.rounded_corner_damage.set_corner_radius(radius);
        decorations.border.set_rounding_power(self.rounding_power());
        decorations
            .focus_ring
            .set_rounding_power(self.rounding_power());
    }

    pub fn advance_animations(&mut self) {
        // Only active effects can emit scene retirement. Ordinary tiles never
        // need a resource-registry lookup to advance their numeric state.
        if self.has_effect_pose() {
            let retirements = self.resources.lease().take_effect_retirements();
            if retirements.wobble {
                self.presentation.wobble.borrow_mut().cancel();
                self.clear_wobble_render_cache();
            }
            if retirements.genie {
                self.complete_genie();
            }
        }
        if self
            .presentation
            .genie
            .borrow()
            .as_ref()
            .is_some_and(|g| g.is_done())
        {
            self.complete_genie();
        }
        if let Some(open) = &mut self.presentation.open_animation {
            if open.is_done() {
                self.presentation.open_animation = None;
                self.resources.lease().open.clear();
            }
        }

        if let Some(resize) = &mut self.presentation.resize_animation {
            if resize.is_done() {
                self.presentation.resize_animation = None;
                self.resources.lease().resize.borrow_mut().take();
            }
        }

        if let Some(move_) = &mut self.move_x_animation {
            if move_.anim.is_done() {
                self.move_x_animation = None;
            }
        }
        if let Some(move_) = &mut self.move_y_animation {
            if move_.anim.is_done() {
                self.move_y_animation = None;
            }
        }

        if let Some(alpha) = &mut self.presentation.alpha_animation {
            if !alpha.hold_after_done && alpha.anim.is_clamped_done() {
                self.presentation.alpha_animation = None;
                self.resources.lease().alpha.clear();
            }
        }
    }

    pub fn has_effect_pose(&self) -> bool {
        self.presentation.wobble.borrow().is_active() || self.presentation.genie.borrow().is_some()
    }

    /// Update canonical effects from the actual event-time scene pose. Output
    /// forecasts may subsequently sample this state without advancing it.
    pub fn advance_effect_pose(&self, location: Point<f64, Logical>) {
        if !self.has_effect_pose() {
            return;
        }
        let area = Rectangle::new(location, self.animated_tile_size());
        let now = self.presentation.clock.now();
        if self.presentation.wobble.borrow().is_active() {
            self.presentation.wobble.borrow_mut().advance(
                area,
                self.presentation.options.animations.window_wobble,
                now,
            );
            if !self.presentation.wobble.borrow().is_active() {
                self.clear_wobble_render_cache();
            }
        }
        if let Some(genie) = self.presentation.genie.borrow_mut().as_mut() {
            genie.retarget_destination(area);
        }
    }

    pub fn are_animations_ongoing(&self) -> bool {
        self.are_transitions_ongoing()
            || (self.decoration_time().is_some()
                && (self.window.rules().baba_is_float == Some(true)
                    || self
                        .presentation
                        .has_animated_gradient(self.is_active, self.window.is_urgent())))
    }

    pub fn are_transitions_ongoing(&self) -> bool {
        self.presentation.genie.borrow().is_some()
            || self.presentation.open_animation.is_some()
            || self.presentation.resize_animation.is_some()
            || self.move_x_animation.is_some()
            || self.move_y_animation.is_some()
            || self
                .presentation
                .alpha_animation
                .as_ref()
                .is_some_and(|alpha| !alpha.anim.is_clamped_done())
            || self.presentation.wobble.borrow().is_ongoing()
    }

    pub fn start_wobble(&mut self, anchor: WobbleAnchor) {
        if self.presentation.options.animations.off
            || self.presentation.clock.should_complete_instantly()
        {
            return;
        }
        if self.presentation.wobble.borrow().is_active() {
            return;
        }
        self.presentation.wobble.borrow_mut().begin(
            anchor,
            self.presentation.options.animations.window_wobble,
            self.presentation.clock.now(),
        );
    }

    pub fn add_pending_wobble_displacement(&mut self, delta: Point<f64, Logical>) {
        self.presentation
            .wobble
            .borrow_mut()
            .add_pending_displacement(delta);
    }

    pub fn finish_wobble(&mut self) {
        self.presentation.wobble.borrow_mut().finish();
    }

    pub fn is_moving_between_workspaces(&self) -> bool {
        self.move_y_animation
            .as_ref()
            .is_some_and(|anim| anim.is_between_workspaces)
            || self
                .move_x_animation
                .as_ref()
                .is_some_and(|anim| anim.is_between_workspaces)
    }

    pub fn update_render_elements(&mut self, is_active: bool, view_rect: Rectangle<f64, Logical>) {
        self.is_active = is_active;
        let tile_size = self.animated_tile_size();
        let expanded = self.expanded_progress();
        let border_width = self.visual_border_width();
        let window_radius = self
            .resolved_corner_radius()
            .scaled_by(1. - expanded as f32);
        let radius = self
            .resolved_corner_radius()
            .expanded_by(border_width.unwrap_or(0.) as f32)
            .scaled_by(1. - expanded as f32);
        let update = DecorationUpdate {
            tile_size,
            border_width: border_width.unwrap_or(0.),
            border_visible: border_width.is_some(),
            draw_border_with_background: self
                .window
                .rules()
                .draw_border_with_background
                .unwrap_or_else(|| !self.window.has_ssd()),
            is_active,
            is_urgent: self.window.is_urgent(),
            view: view_rect,
            radius,
            window_radius,
            scale: self.presentation.scale,
            alpha: 1. - expanded as f32,
            time: self.decoration_time(),
            geometry_changed: self.are_transitions_ongoing(),
        };
        self.resources
            .lease()
            .decorations
            .borrow_mut()
            .update(update);
    }

    pub fn scale(&self) -> f64 {
        self.presentation.scale
    }

    pub fn render_offset(&self) -> Point<f64, Logical> {
        let mut offset = Point::from((0., 0.));

        if let Some(move_) = &self.move_x_animation {
            offset.x += move_.from * move_.anim.value();
        }
        if let Some(move_) = &self.move_y_animation {
            offset.y += move_.from * move_.anim.value();
        }

        offset += self.interactive_move_offset;

        offset
    }

    pub fn start_open_animation(&mut self) {
        self.resources.lease().open.clear();
        let popin_ratio = self.presentation.options.animations.window_open.popin_ratio;
        self.presentation.open_animation = Some(OpenAnimation::new(
            Animation::new(
                self.presentation.clock.clone(),
                0.,
                1.,
                0.,
                self.presentation.options.animations.window_open.anim,
            ),
            popin_ratio,
            self.presentation.options.animations.window_open.glide,
            self.presentation
                .options
                .animations
                .window_open
                .custom_shader
                .is_none(),
            self.presentation.options.animations.window_open.effect,
        ));
    }

    pub fn resize_animation(&self) -> Option<&Animation> {
        self.presentation
            .resize_animation
            .as_ref()
            .map(|resize| &resize.anim)
    }

    pub fn animate_move_from(&mut self, from: Point<f64, Logical>) {
        self.animate_move_from_with_config(
            from,
            self.presentation.options.animations.window_movement.0,
        );
    }

    pub fn animate_move_from_with_config(
        &mut self,
        from: Point<f64, Logical>,
        config: clarion_config::Animation,
    ) {
        self.animate_move_x_from_with_config(from.x, config);
        self.animate_move_y_from_with_config(from.y, config);
    }

    pub fn animate_move_x_from(&mut self, from: f64) {
        self.animate_move_x_from_with_config(
            from,
            self.presentation.options.animations.window_movement.0,
        );
    }

    pub fn animate_move_x_from_with_config(
        &mut self,
        from: f64,
        config: clarion_config::Animation,
    ) {
        let current_offset = self.render_offset().x;

        // Preserve the previous config if ongoing.
        self.move_x_animation = Some(MoveAnimation::retarget(
            self.move_x_animation.take(),
            from + current_offset,
            self.presentation.clock.clone(),
            config,
        ));
    }

    pub fn animate_move_y_from(&mut self, from: f64) {
        self.animate_move_y_from_with_config(
            from,
            self.presentation.options.animations.window_movement.0,
        );
    }

    pub fn animate_move_y_from_with_config(
        &mut self,
        from: f64,
        config: clarion_config::Animation,
    ) {
        let current_offset = self.render_offset().y;

        // Preserve the previous config if ongoing.
        self.move_y_animation = Some(MoveAnimation::retarget(
            self.move_y_animation.take(),
            from + current_offset,
            self.presentation.clock.clone(),
            config,
        ));
    }

    pub fn offset_move_y_anim_current(&mut self, offset: f64) {
        let Some(previous) = self.move_y_animation.take() else {
            return;
        };
        let current_offset = previous.from * previous.anim.value();
        // Changing only the amplitude preserves one position but also scales
        // velocity. Treat a cancelled resize as the same motion retarget used
        // for other layout changes, retaining both pose and incoming momentum.
        self.move_y_animation = Some(MoveAnimation::retarget(
            Some(previous),
            offset + current_offset,
            self.presentation.clock.clone(),
            self.presentation.options.animations.window_movement.0,
        ));
    }

    pub fn stop_move_animations(&mut self) {
        self.move_x_animation = None;
        self.move_y_animation = None;
    }

    pub fn set_anim_y_between_workspaces(&mut self) {
        if let Some(anim) = &mut self.move_y_animation {
            anim.is_between_workspaces = true;
        }
    }

    pub fn animate_alpha(&mut self, from: f64, to: f64, config: clarion_config::Animation) {
        if self.presentation.alpha_animation.is_none() {
            self.resources.lease().alpha.clear();
        }
        self.presentation.alpha_animation = Some(AlphaAnimation::retarget(
            self.presentation.alpha_animation.take(),
            from,
            to,
            self.presentation.clock.clone(),
            config,
        ));
    }

    pub fn ensure_alpha_animates_to_1(&mut self) {
        if let Some(alpha) = &mut self.presentation.alpha_animation {
            alpha.ensure_opaque();
        }
    }

    pub fn hold_alpha_animation_after_done(&mut self) {
        if let Some(alpha) = &mut self.presentation.alpha_animation {
            alpha.hold_after_done = true;
        }
    }

    pub fn window(&self) -> &W {
        &self.window
    }

    pub fn is_active(&self) -> bool {
        self.is_active
    }

    /// Continuous decoration effects obey the same reduced-motion switch as transitions.
    pub fn decoration_time(&self) -> Option<std::time::Duration> {
        self.presentation.decoration_time()
    }

    pub fn window_mut(&mut self) -> &mut W {
        &mut self.window
    }

    pub fn sizing_mode(&self) -> SizingMode {
        self.presentation.sizing_mode
    }

    fn expanded_progress(&self) -> f64 {
        self.presentation.expanded_progress()
    }

    /// Returns `None` if the border is hidden and `Some(width)` if it should be shown.
    pub fn effective_border_width(&self) -> Option<f64> {
        self.presentation.effective_border_width()
    }

    fn visual_border_width(&self) -> Option<f64> {
        self.presentation.visual_border_width()
    }

    /// Returns the location of the window's visual geometry within this Tile.
    pub fn window_loc(&self) -> Point<f64, Logical> {
        self.presentation.window_loc(&self.window)
    }

    pub fn tile_size(&self) -> Size<f64, Logical> {
        self.presentation.tile_size(&self.window)
    }

    pub fn tile_expected_or_current_size(&self) -> Size<f64, Logical> {
        let mut size = self.window_expected_or_current_size();

        if self.presentation.sizing_mode.is_fullscreen() {
            // Normally we'd just return the fullscreen size here, but this makes things a bit
            // nicer if a fullscreen window is bigger than the fullscreen size for some reason.
            size.w = f64::max(size.w, self.presentation.view_size.w);
            size.h = f64::max(size.h, self.presentation.view_size.h);
            return size;
        }

        if let Some(width) = self.effective_border_width() {
            size.w += width * 2.;
            size.h += width * 2.;
        }

        size
    }

    pub fn window_size(&self) -> Size<f64, Logical> {
        self.presentation.window_size(&self.window)
    }

    pub fn window_expected_or_current_size(&self) -> Size<f64, Logical> {
        let size = self.window.expected_size();
        let mut size = size.unwrap_or_else(|| self.window.size()).to_f64();
        size = size
            .to_physical_precise_round(self.presentation.scale)
            .to_logical(self.presentation.scale);
        size
    }

    pub fn animated_window_size(&self) -> Size<f64, Logical> {
        self.presentation.animated_window_size(&self.window)
    }

    pub fn animated_tile_size(&self) -> Size<f64, Logical> {
        self.presentation.animated_tile_size(&self.window)
    }

    pub fn buf_loc(&self) -> Point<f64, Logical> {
        let mut loc = Point::from((0., 0.));
        loc += self.window_loc();
        loc += self.window.buf_loc().to_f64();
        loc
    }

    /// Returns a partially-filled [`WindowLayout`].
    ///
    /// Only the sizing properties that a [`Tile`] can fill are filled.
    pub fn ipc_layout_template(&self) -> WindowLayout {
        WindowLayout {
            pos_in_scrolling_layout: None,
            tile_size: self.tile_size().into(),
            window_size: self.window().size().into(),
            tile_pos_in_workspace_view: None,
            window_offset_in_tile: self.window_loc().into(),
        }
    }

    fn is_in_input_region(&self, mut point: Point<f64, Logical>) -> bool {
        point -= self.window_loc().to_f64();
        self.window.is_in_input_region(point)
    }

    fn is_in_activation_region(&self, point: Point<f64, Logical>) -> bool {
        let activation_region = Rectangle::from_size(self.tile_size());
        activation_region.contains(point)
    }

    pub fn hit(&self, point: Point<f64, Logical>) -> Option<HitType> {
        if self.genie_ongoing() {
            return None;
        }
        let offset = self.bob_offset();
        let point = point - offset;

        if self.is_in_input_region(point) {
            let win_pos = self.buf_loc() + offset;
            Some(HitType::Input { win_pos })
        } else if self.is_in_activation_region(point) {
            Some(HitType::Activate {
                is_tab_indicator: false,
            })
        } else {
            None
        }
    }

    pub fn request_tile_size(
        &mut self,
        mut size: Size<f64, Logical>,
        animate: bool,
        transaction: Option<Transaction>,
    ) {
        // Can't go through effective_border_width() because we might be fullscreen.
        if !self.presentation.border_config.off {
            let width = self.presentation.border_config.width;
            size.w = f64::max(1., size.w - width * 2.);
            size.h = f64::max(1., size.h - width * 2.);
        }

        // The size request has to be i32 unfortunately, due to Wayland. We floor here instead of
        // round to avoid situations where proportionally-sized columns don't fit on the screen
        // exactly.
        self.window.request_size(
            size.to_i32_floor(),
            SizingMode::Normal,
            animate,
            transaction,
        );
    }

    pub fn tile_width_for_window_width(&self, size: f64) -> f64 {
        if self.presentation.border_config.off {
            size
        } else {
            size + self.presentation.border_config.width * 2.
        }
    }

    pub fn tile_height_for_window_height(&self, size: f64) -> f64 {
        if self.presentation.border_config.off {
            size
        } else {
            size + self.presentation.border_config.width * 2.
        }
    }

    pub fn window_width_for_tile_width(&self, size: f64) -> f64 {
        if self.presentation.border_config.off {
            size
        } else {
            size - self.presentation.border_config.width * 2.
        }
    }

    pub fn window_height_for_tile_height(&self, size: f64) -> f64 {
        if self.presentation.border_config.off {
            size
        } else {
            size - self.presentation.border_config.width * 2.
        }
    }

    pub fn request_maximized(
        &mut self,
        size: Size<f64, Logical>,
        animate: bool,
        transaction: Option<Transaction>,
    ) {
        self.window.request_size(
            size.to_i32_round(),
            SizingMode::Maximized,
            animate,
            transaction,
        );
    }

    pub fn request_fullscreen(&mut self, animate: bool, transaction: Option<Transaction>) {
        self.window.request_size(
            self.presentation.view_size.to_i32_round(),
            SizingMode::Fullscreen,
            animate,
            transaction,
        );
    }

    pub fn min_size_nonfullscreen(&self) -> Size<f64, Logical> {
        let mut size = self.window.min_size().to_f64();

        // Can't go through effective_border_width() because we might be fullscreen.
        if !self.presentation.border_config.off {
            let width = self.presentation.border_config.width;

            size.w = f64::max(1., size.w);
            size.h = f64::max(1., size.h);

            size.w += width * 2.;
            size.h += width * 2.;
        }

        size
    }

    pub fn max_size_nonfullscreen(&self) -> Size<f64, Logical> {
        let mut size = self.window.max_size().to_f64();

        // Can't go through effective_border_width() because we might be fullscreen.
        if !self.presentation.border_config.off {
            let width = self.presentation.border_config.width;

            if size.w > 0. {
                size.w += width * 2.;
            }
            if size.h > 0. {
                size.h += width * 2.;
            }
        }

        size
    }

    pub fn bob_offset(&self) -> Point<f64, Logical> {
        self.presentation.bob_offset(&self.window)
    }

    pub fn render<R: ClarionRenderer>(
        &self,
        ctx: RenderCtx<R>,
        location: Point<f64, Logical>,
        xray_pos: XrayPos,
        focus_ring: bool,
        push: &mut dyn FnMut(TileRenderElement<R>),
    ) {
        self.scene()
            .render(ctx, location, xray_pos, focus_ring, push);
    }

    /// Update the destination for the next transition. Returns true only when
    /// this registration removes an existing presentation that needs repainting.
    pub fn set_minimize_target(
        &mut self,
        target: Option<clarion_ipc::DockTarget>,
        reduce_motion: bool,
    ) -> bool {
        self.minimize_target = target.filter(clarion_ipc::DockTarget::is_valid);
        self.minimize_reduce_motion = reduce_motion;
        if reduce_motion || self.minimize_target.is_none() {
            let repaint = self.genie_ongoing();
            self.complete_genie();
            return repaint;
        }
        false
    }

    pub fn invalidate_minimize_target(&mut self) {
        self.minimize_target = None;
        self.complete_genie();
    }

    pub fn start_genie(
        &mut self,
        source: Rectangle<f64, Logical>,
        output: Option<&smithay::output::Output>,
        minimizing: bool,
        visible: bool,
    ) {
        if !visible
            || self.minimize_reduce_motion
            || self.presentation.options.animations.off
            || self.presentation.clock.should_complete_instantly()
        {
            self.complete_genie();
            return;
        }
        let Some(target) = self.minimize_target.as_ref().filter(|target| {
            output.is_some_and(|output| {
                let size = crate::utils::output_size(output);
                target.output == output.name()
                    && target.x + target.width <= size.w
                    && target.y + target.height <= size.h
            })
        }) else {
            self.complete_genie();
            return;
        };
        self.presentation.wobble.borrow_mut().cancel();
        self.clear_wobble_render_cache();
        self.presentation.open_animation = None;
        self.resources.lease().open.clear();
        // Freeze incidental movement before capturing the original tile.
        self.move_x_animation = None;
        self.move_y_animation = None;
        self.presentation.alpha_animation = None;
        self.resources.lease().alpha.clear();
        // A fresh owner interaction supersedes a previous scene failure.
        self.resources.lease().clear_genie_retirement();
        let mut genie = self.presentation.genie.borrow_mut();
        if let Some(genie) = genie.as_mut() {
            genie.reverse(self.presentation.clock.clone(), minimizing);
        } else {
            *genie = Some(super::minimized::Genie::with_style(
                self.presentation.clock.clone(),
                source,
                Rectangle::new(
                    (target.x, target.y).into(),
                    (target.width, target.height).into(),
                ),
                minimizing,
                target.edge,
                target.effect,
            ));
        }
    }

    pub fn genie_ongoing(&self) -> bool {
        self.presentation.genie.borrow().is_some()
    }

    pub fn complete_genie(&self) {
        self.presentation.genie.borrow_mut().take();
        self.resources.lease().clear_genie_retirement();
        self.scene().clear_genie_pixels();
    }

    /// A parked tile only uses this presentation path; failure therefore cannot
    /// accidentally reveal a logically minimized window.
    pub fn render_genie<R: ClarionRenderer>(
        &self,
        ctx: RenderCtx<R>,
        xray_pos: XrayPos,
        push: &mut dyn FnMut(TileRenderElement<R>),
    ) -> bool {
        self.scene().render_genie(ctx, xray_pos, push)
    }

    fn clear_wobble_render_cache(&self) {
        self.resources.lease().clear_wobble_retirement();
        self.scene().clear_wobble_pixels();
    }

    /// Native queue pressure defers presentation rather than completing an effect.
    /// Fatal errors retain the existing cleanup/fallback behavior.
    #[cfg(test)]
    pub(super) fn handle_native_effect_error(
        &self,
        effect: NativeEffect,
        error: &(dyn std::error::Error + 'static),
    ) -> bool {
        self.scene().handle_native_effect_error(effect, error)
    }

    pub(crate) fn scene(&self) -> crate::scene::tile::TileScene<'_, W> {
        crate::scene::tile::TileScene {
            window: &self.window,
            state: &self.presentation,
            resources: self.resources.lease(),
        }
    }

    pub fn snapshot_native(
        &mut self,
        renderer: &mut crate::render_helpers::vulkan::native::NativeRenderer,
    ) -> anyhow::Result<NativeTileSnapshot> {
        crate::scene::window_snapshot::snapshot_native(&self.scene(), &self.resources, renderer)
    }
    #[allow(dead_code)]
    pub(crate) fn take_native_close_source(
        &self,
    ) -> Option<crate::scene::window_resources::NativeCloseSource> {
        crate::scene::window_snapshot::take_native_close_source(&self.window, &self.resources)
    }
    pub(crate) fn create_closing_window(
        &self,
        renderer: &mut AnyRenderer<'_>,
        scale: smithay::utils::Scale<f64>,
        tile_pos: Point<f64, Logical>,
        blocker: crate::utils::transaction::TransactionBlocker,
        anim: Animation,
        options: &clarion_config::animations::WindowCloseAnim,
        closing_allocator: crate::scene::retained::ResourceAllocator<
            crate::scene::closing_window::ClosingResources,
        >,
    ) -> anyhow::Result<crate::scene::closing_window::ClosingWindow> {
        crate::scene::closing_window::ClosingWindow::new_any(
            renderer,
            &self.scene(),
            &self.resources,
            scale,
            self.tile_size(),
            tile_pos,
            blocker,
            anim,
            options.glide,
            options.effect,
            options.custom_shader.as_deref(),
            closing_allocator,
        )
    }
    pub fn store_unmap_snapshot_native_if_empty(
        &mut self,
        renderer: &mut crate::render_helpers::vulkan::native::NativeRenderer,
        xray: Option<&mut Xray>,
        blocked: bool,
        pos: XrayPos,
    ) -> bool {
        crate::scene::window_snapshot::store_native_unmap_if_empty(
            &self.scene(),
            &self.resources,
            renderer,
            xray,
            blocked,
            pos,
        )
    }
    pub fn store_unmap_snapshot_if_empty(
        &mut self,
        renderer: &mut GlesRenderer,
        xray: Option<&mut Xray>,
        blocked: bool,
        pos: XrayPos,
    ) {
        crate::scene::window_snapshot::store_gles_unmap_if_empty(
            &self.scene(),
            &self.resources,
            renderer,
            xray,
            blocked,
            pos,
        )
    }
    pub fn store_unmap_snapshot_any_if_empty(
        &mut self,
        renderer: &mut AnyRenderer<'_>,
        xray: Option<&mut Xray>,
        blocked: bool,
        pos: XrayPos,
    ) {
        crate::scene::window_snapshot::store_any_unmap_if_empty(
            &self.scene(),
            &self.resources,
            renderer,
            xray,
            blocked,
            pos,
        )
    }
    pub fn take_unmap_snapshot(&mut self) -> Option<TileRenderSnapshot> {
        crate::scene::window_snapshot::take_unmap_snapshot(&self.resources)
    }

    pub fn border_config(&self) -> &clarion_config::FocusRing {
        &self.presentation.border_config
    }
    pub fn focus_ring_config(&self) -> &clarion_config::FocusRing {
        &self.presentation.focus_ring_config
    }
    #[cfg(test)]
    pub(crate) fn render_decoration_rings(
        &self,
        renderer: &mut impl crate::render_helpers::renderer::DecorationRenderer,
        push: &mut dyn FnMut(crate::scene::focus_ring::FocusRingRenderElement),
    ) {
        self.scene().render_decoration_rings(renderer, push);
    }

    pub fn options(&self) -> &Rc<Options> {
        &self.presentation.options
    }

    pub fn view_size(&self) -> Size<f64, Logical> {
        self.presentation.view_size
    }

    #[cfg(test)]
    pub fn verify_invariants(&self) {
        use approx::assert_abs_diff_eq;

        assert_eq!(self.presentation.sizing_mode, self.window.sizing_mode());

        let scale = self.presentation.scale;
        let size = self.tile_size();
        let rounded = size.to_physical_precise_round(scale).to_logical(scale);
        assert_abs_diff_eq!(size.w, rounded.w, epsilon = 1e-5);
        assert_abs_diff_eq!(size.h, rounded.h, epsilon = 1e-5);
    }
}

#[cfg(test)]
mod move_tests {
    use std::time::Duration;

    use clarion_config::animations::{Kind, SpringParams, WindowMovementAnim};

    use super::*;

    #[test]
    fn interactive_tile_move_uses_configured_spring_and_keeps_reversal_momentum() {
        for rate in [0.25, 1., 2.] {
            let mut clock = Clock::with_time(Duration::ZERO);
            clock.set_rate(rate);
            let config = WindowMovementAnim::default().0;
            assert_eq!(config.kind, Kind::Spring(SpringParams::MACOS_INTERACTIVE));
            let previous = MoveAnimation::retarget(None, 240., clock.clone(), config);
            clock.set_unadjusted(Duration::from_millis(80));
            let before = previous.from * previous.anim.value();
            let velocity = previous.from * previous.anim.velocity();
            // Changing layout coordinates adds a fixed offset to the visible
            // position but must not discard the ongoing physical velocity.
            let current =
                MoveAnimation::retarget(Some(previous), before - 300., clock.clone(), config);
            assert!((current.from * current.anim.value() - (before - 300.)).abs() < 1e-10);
            assert!((current.from * current.anim.velocity() - velocity).abs() < 1e-8);
            clock.set_unadjusted(Duration::from_secs(4));
            assert_eq!(current.anim.value(), 0.);
            assert!(current.anim.is_done());
        }
    }
}

#[cfg(test)]
mod alpha_tests {
    use std::time::Duration;

    use clarion_config::animations::{Curve, EasingParams, Kind, WindowMovementAnim};

    use super::*;

    #[test]
    fn drag_release_preserves_visible_alpha_and_velocity_across_rates() {
        for rate in [0.25, 0.73, 1., 2.] {
            for hz in [60., 120., 144., 240., 360.] {
                let mut clock = Clock::with_time(Duration::ZERO);
                clock.set_rate(rate);
                let config = WindowMovementAnim::default().0;
                let mut alpha = AlphaAnimation::retarget(None, 1., 0.75, clock.clone(), config);
                alpha.hold_after_done = true;
                let at = Duration::from_secs_f64(5. / hz);
                clock.set_unadjusted(at);
                let before = alpha.value();
                let velocity = alpha.anim.velocity();
                assert!(before > 0.75 && before < 1.);
                assert!(velocity < 0.);

                let alpha = AlphaAnimation::retarget(Some(alpha), 0.75, 1., clock.clone(), config);
                assert!((alpha.value() - before).abs() < 1e-12);
                assert!((alpha.anim.velocity() - velocity).abs() < 1e-12);
                assert!(!alpha.hold_after_done);

                let delta = Duration::from_nanos(100);
                clock.set_unadjusted(at + delta);
                let slope = (alpha.value() - before) / delta.as_secs_f64();
                assert!((slope - velocity).abs() < 0.001, "rate={rate}, hz={hz}");
                clock.set_unadjusted(Duration::from_secs(10));
                assert_eq!(alpha.value(), 1.);
                assert!(alpha.anim.is_done());
            }
        }
    }

    #[test]
    fn activating_a_fading_tile_returns_to_opaque_without_a_snap() {
        let mut clock = Clock::with_time(Duration::ZERO);
        let mut alpha =
            AlphaAnimation::retarget(None, 1., 0., clock.clone(), WindowMovementAnim::default().0);
        alpha.hold_after_done = true;
        clock.set_unadjusted(Duration::from_millis(40));
        let before = alpha.value();
        let velocity = alpha.anim.velocity();
        alpha.ensure_opaque();
        assert!((alpha.value() - before).abs() < 1e-12);
        assert!((alpha.anim.velocity() - velocity).abs() < 1e-12);
        assert!(!alpha.hold_after_done);

        // Repeated activation must not restart an already returning transition.
        let start = alpha.anim.start_time();
        clock.set_unadjusted(Duration::from_millis(60));
        alpha.ensure_opaque();
        assert_eq!(alpha.anim.start_time(), start);
        clock.set_unadjusted(Duration::from_secs(2));
        assert_eq!(alpha.value(), 1.);
        assert!(alpha.anim.is_done());
    }

    #[test]
    fn returning_to_opaque_preserves_custom_easing_and_disabled_animations() {
        for off in [false, true] {
            let mut clock = Clock::with_time(Duration::ZERO);
            let config = clarion_config::Animation {
                off,
                kind: Kind::Easing(EasingParams {
                    duration_ms: 400,
                    curve: Curve::Linear,
                }),
            };
            let mut alpha = AlphaAnimation::retarget(None, 1., 0.75, clock.clone(), config);
            clock.set_unadjusted(Duration::from_millis(100));
            let before = alpha.value();
            alpha.ensure_opaque();
            if off {
                assert_eq!(alpha.value(), 1.);
                assert!(alpha.anim.is_done());
                assert_eq!(alpha.anim.velocity(), 0.);
            } else {
                assert_eq!(alpha.anim.duration(), Duration::from_millis(400));
                assert_eq!(alpha.anim.velocity(), -0.625);
                clock.set_unadjusted(Duration::from_nanos(100_000_100));
                let measured = (alpha.value() - before) / 1e-7;
                assert!((measured + 0.625).abs() < 0.00001);
                clock.set_unadjusted(Duration::from_millis(300));
                // The incoming downward velocity is retained during the turn,
                // rather than immediately jumping to the new preset's slope.
                let expected = (before + 1.) / 2. - 0.625 * 0.4 / 8.;
                assert!((alpha.value() - expected).abs() < 1e-12);
                clock.set_unadjusted(Duration::from_millis(500));
                assert_eq!(alpha.value(), 1.);
                assert!(alpha.anim.is_done());
            }
        }
    }

    #[test]
    fn held_alpha_does_not_release_with_hidden_spring_momentum() {
        let mut clock = Clock::with_time(Duration::ZERO);
        let mut config = WindowMovementAnim::default().0;
        let Kind::Spring(params) = &mut config.kind else {
            unreachable!()
        };
        params.damping_ratio = 0.2;
        let mut alpha = AlphaAnimation::retarget(None, 1., 0.75, clock.clone(), config);
        alpha.hold_after_done = true;
        clock.set_unadjusted(Duration::from_millis(150));
        assert!(alpha.anim.is_clamped_done());
        assert!(!alpha.anim.is_done());
        assert!(alpha.anim.velocity().abs() > 0.001);
        assert_eq!(alpha.value(), 0.75);

        alpha.ensure_opaque();
        assert_eq!(alpha.value(), 0.75);
        assert_eq!(alpha.anim.velocity(), 0.);
        clock.set_unadjusted(Duration::from_millis(160));
        assert!(alpha.value() > 0.75);
    }

    #[test]
    fn reversal_at_alpha_bounds_uses_visible_opacity_and_velocity() {
        for (from, target, velocity) in [(0.99, 0.75, 3.), (0.01, 0.25, -3.)] {
            let mut clock = Clock::with_time(Duration::ZERO);
            let config = WindowMovementAnim::default().0;
            let alpha = AlphaAnimation {
                anim: Animation::new(clock.clone(), from, target, velocity, config),
                hold_after_done: true,
            };
            clock.set_unadjusted(Duration::from_millis(10));
            let raw = alpha.anim.clamped_value();
            assert!(!(0. ..=1.).contains(&raw));
            let visible = raw.clamp(0., 1.);
            assert_eq!(alpha.value(), visible);

            let alpha = AlphaAnimation::retarget(Some(alpha), from, target, clock, config);
            assert_eq!(alpha.value(), visible);
            assert_eq!(alpha.anim.velocity(), 0.);
        }
    }
}

#[cfg(test)]
mod resize_tests {
    use std::time::Duration;

    use clarion_config::animations::{Curve, EasingParams, Kind, WindowResizeAnim};

    use super::*;

    #[test]
    fn fullscreen_decoration_reversal_keeps_visible_velocity() {
        for rate in [0.25, 1., 2.] {
            let mut clock = Clock::with_time(Duration::ZERO);
            clock.set_rate(rate);
            let config = WindowResizeAnim::default().anim;
            let first = resize_decoration_animation(None, 0., 1., clock.clone(), config).unwrap();
            clock.set_unadjusted(Duration::from_millis(60));
            let before = first.clamped_value();
            let velocity = first.velocity();
            let reversed =
                resize_decoration_animation(Some(first), 1., 0., clock.clone(), config).unwrap();
            assert_eq!(reversed.clamped_value(), before);
            assert!((reversed.velocity() - velocity).abs() < 1e-10);
            clock.set_unadjusted(Duration::from_nanos(60_000_100));
            let slope = (reversed.clamped_value() - before) / 0.0000001;
            assert!((slope - velocity).abs() < 0.001);
        }
    }

    #[test]
    fn resize_lifetime_includes_momentum_but_not_clamped_decoration_tail() {
        let mut clock = Clock::with_time(Duration::ZERO);
        let config = WindowResizeAnim::default().anim;
        let mut decoration_config = config;
        let Kind::Spring(params) = &mut decoration_config.kind else {
            unreachable!()
        };
        params.damping_ratio = 0.2;
        let resize = ResizeAnimation {
            anim: Animation::new(clock.clone(), 0., 1., 0., config),
            momentum: ResizeMomentum::new(
                clock.clone(),
                Point::from((9000., -2000.)),
                Point::from((12000., -3000.)),
                config,
            ),
            size_from: Size::from((500., 800.)),
            tile_size_from: Size::from((508., 808.)),
            fullscreen_progress: resize_decoration_animation(
                None,
                0.,
                1.,
                clock.clone(),
                decoration_config,
            ),
            expanded_progress: None,
        };
        let end = resize
            .anim
            .end_time()
            .max(resize.momentum.as_ref().unwrap().anim.end_time());
        clock.set_unadjusted(end - Duration::from_nanos(1));
        assert!(!resize.is_done());
        let before = resize.size_value(Size::from((300., 1000.)), false);
        assert!((before.w - 300.).abs() < 0.25);
        assert!((before.h - 1000.).abs() < 0.25);
        clock.set_unadjusted(end);
        assert!(resize
            .fullscreen_progress
            .as_ref()
            .unwrap()
            .is_clamped_done());
        assert!(!resize.fullscreen_progress.as_ref().unwrap().is_done());
        assert!(resize.is_done());
        assert_eq!(
            resize.size_value(Size::from((300., 1000.)), false),
            Size::from((300., 1000.))
        );
    }

    #[test]
    fn resize_momentum_preserves_custom_easing_and_animation_off() {
        let clock = Clock::with_time(Duration::ZERO);
        for config in [
            clarion_config::Animation {
                off: false,
                kind: Kind::Easing(EasingParams {
                    duration_ms: 300,
                    curve: Curve::Linear,
                }),
            },
            clarion_config::Animation {
                off: true,
                ..WindowResizeAnim::default().anim
            },
        ] {
            assert!(ResizeMomentum::new(
                clock.clone(),
                Point::from((3000., -1000.)),
                Point::from((3000., -1000.)),
                config
            )
            .is_none());
        }
    }
}
