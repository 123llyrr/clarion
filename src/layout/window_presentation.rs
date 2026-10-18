//! Renderer-free window presentation state and geometry.
//! Layout owns numeric trajectories; the scene samples them and owns all pixels.

use std::cell::RefCell;
use std::rc::Rc;

use clarion_config::CornerRadius;
use smithay::utils::{Logical, Point, Size};

use super::minimized::Genie;
use super::opening_window::OpenAnimation;
use super::wobble::WobbleState;
use super::{LayoutElement, Options, SizingMode};
use crate::animation::{Animation, Clock};
use crate::utils::{baba_is_float_offset, round_logical_in_physical};

#[derive(Debug)]
pub(crate) struct WindowPresentationState {
    pub(crate) sizing_mode: SizingMode,
    pub(crate) open_animation: Option<OpenAnimation>,
    pub(crate) resize_animation: Option<ResizeAnimation>,
    pub(crate) alpha_animation: Option<AlphaAnimation>,
    pub(crate) wobble: RefCell<WobbleState>,
    pub(crate) genie: RefCell<Option<Genie>>,
    pub(crate) view_size: Size<f64, Logical>,
    pub(crate) scale: f64,
    pub(crate) clock: Clock,
    pub(crate) options: Rc<Options>,
    pub(crate) border_config: clarion_config::FocusRing,
    pub(crate) focus_ring_config: clarion_config::FocusRing,
}

#[derive(Debug)]
pub(crate) struct ResizeAnimation {
    pub(crate) anim: Animation,
    pub(crate) momentum: Option<ResizeMomentum>,
    pub(crate) size_from: Size<f64, Logical>,
    pub(crate) tile_size_from: Size<f64, Logical>,
    /// Ordinary tiling owns a geometric goal before its client supplies pixels.
    /// The legacy snapshot blend instead derives its target from the committed
    /// buffer. A completed request may hold its goal without scheduling redraws.
    pub(crate) request: Option<ResizeRequest>,
    // If the resize involved the fullscreen state at some point, this is the progress toward the
    // fullscreen state. Used for things like fullscreen backdrop alpha.
    //
    // Note that this can be set even if this specific resize is between two non-fullscreen states,
    // for example when issuing a new resize during an unfullscreen resize.
    pub(crate) fullscreen_progress: Option<Animation>,
    // Similar to above but for fullscreen-or-maximized.
    pub(crate) expanded_progress: Option<Animation>,
}

#[derive(Debug)]
pub(crate) struct ResizeRequest {
    /// Protocol hint can differ from visible geometry when client constraints
    /// cannot fit a cell. Matching commits still belong to this request.
    pub(crate) protocol_size: Size<i32, Logical>,
    pub(crate) window_size: Size<f64, Logical>,
    pub(crate) tile_size: Size<f64, Logical>,
    pub(crate) pending_buffer: bool,
}

/// One homogeneous spring response carries all four size velocities. The main
/// 0..1 animation remains the snapshot blend controller; both responses use the
/// same spring parameters, so their sum is the spring for each new size target.
#[derive(Debug)]
pub(crate) struct ResizeMomentum {
    pub(crate) anim: Animation,
    pub(crate) window_weights: Point<f64, Logical>,
    pub(crate) tile_weights: Point<f64, Logical>,
}

impl ResizeMomentum {
    pub(crate) fn new(
        clock: Clock,
        window_velocity: Point<f64, Logical>,
        tile_velocity: Point<f64, Logical>,
        config: clarion_config::Animation,
    ) -> Option<Self> {
        if config.off || !matches!(config.kind, clarion_config::animations::Kind::Spring(_)) {
            return None;
        }
        let amplitude = window_velocity
            .x
            .abs()
            .max(window_velocity.y.abs())
            .max(tile_velocity.x.abs())
            .max(tile_velocity.y.abs());
        if amplitude <= f64::EPSILON {
            return None;
        }
        Some(Self {
            anim: Animation::new(clock, 0., 0., amplitude, config),
            window_weights: Point::from((
                window_velocity.x / amplitude,
                window_velocity.y / amplitude,
            )),
            tile_weights: Point::from((tile_velocity.x / amplitude, tile_velocity.y / amplitude)),
        })
    }
}

impl ResizeAnimation {
    fn target_size(&self, committed: Size<f64, Logical>, tile: bool) -> Size<f64, Logical> {
        self.request.as_ref().map_or(committed, |request| {
            if tile {
                request.tile_size
            } else {
                request.window_size
            }
        })
    }

    pub(crate) fn size_value(&self, to: Size<f64, Logical>, tile: bool) -> Size<f64, Logical> {
        let size = self.size_components(to, tile);
        Size::from((size.x.max(1.), size.y.max(1.)))
    }

    pub(crate) fn size_components(
        &self,
        to: Size<f64, Logical>,
        tile: bool,
    ) -> Point<f64, Logical> {
        let to = self.target_size(to, tile);
        let from = if tile {
            self.tile_size_from
        } else {
            self.size_from
        };
        let progress = self.anim.value();
        // Spring overshoot can temporarily make a component negative; construct
        // Smithay's nonnegative Size only after applying the final clamp.
        let mut size = Point::from((
            from.w + (to.w - from.w) * progress,
            from.h + (to.h - from.h) * progress,
        ));
        if let Some(momentum) = &self.momentum {
            let weights = if tile {
                momentum.tile_weights
            } else {
                momentum.window_weights
            };
            let value = momentum.anim.value();
            size.x += weights.x * value;
            size.y += weights.y * value;
        }
        size
    }

    pub(crate) fn size_state(
        &self,
        to: Size<f64, Logical>,
        tile: bool,
    ) -> (Size<f64, Logical>, Point<f64, Logical>) {
        let to = self.target_size(to, tile);
        let from = if tile {
            self.tile_size_from
        } else {
            self.size_from
        };
        let mut size = self.size_components(to, tile);
        let progress_velocity = self.anim.velocity();
        let mut velocity = Point::from((
            (to.w - from.w) * progress_velocity,
            (to.h - from.h) * progress_velocity,
        ));
        if let Some(momentum) = &self.momentum {
            let weights = if tile {
                momentum.tile_weights
            } else {
                momentum.window_weights
            };
            let speed = momentum.anim.velocity();
            velocity.x += weights.x * speed;
            velocity.y += weights.y * speed;
        }
        // A clamped one-pixel size has no visible outward velocity.
        if size.x < 1. || (size.x == 1. && velocity.x < 0.) {
            size.x = 1.;
            velocity.x = 0.;
        }
        if size.y < 1. || (size.y == 1. && velocity.y < 0.) {
            size.y = 1.;
            velocity.y = 0.;
        }
        (Size::from((size.x, size.y)), velocity)
    }

    pub(crate) fn is_done(&self) -> bool {
        self.anim.is_done()
            && self.momentum.as_ref().is_none_or(|m| m.anim.is_done())
            && self
                .fullscreen_progress
                .as_ref()
                .is_none_or(Animation::is_clamped_done)
            && self
                .expanded_progress
                .as_ref()
                .is_none_or(Animation::is_clamped_done)
    }
}

pub(crate) fn resize_decoration_animation(
    previous: Option<Animation>,
    from: f64,
    to: f64,
    clock: Clock,
    config: clarion_config::Animation,
) -> Option<Animation> {
    let (from, velocity) = previous
        .as_ref()
        .map_or((from, 0.), visible_animation_state);
    (from != to || velocity != 0.).then(|| Animation::new(clock, from, to, velocity, config))
}

pub(crate) fn visible_animation_state(anim: &Animation) -> (f64, f64) {
    let raw = anim.clamped_value();
    let current = raw.clamp(0., 1.);
    let mut velocity = anim.velocity();
    if anim.is_clamped_done()
        || raw != current
        || (current == 0. && velocity < 0.)
        || (current == 1. && velocity > 0.)
    {
        velocity = 0.;
    }
    (current, velocity)
}

#[derive(Debug)]
pub(crate) struct AlphaAnimation {
    pub(crate) anim: Animation,
    /// Whether the animation should persist after it's done.
    ///
    /// This is used by things like interactive move which need to animate alpha to
    /// semitransparent, then hold it at semitransparent for a while, until the operation
    /// completes.
    pub(crate) hold_after_done: bool,
}

impl AlphaAnimation {
    pub(crate) fn retarget(
        previous: Option<Self>,
        from: f64,
        to: f64,
        clock: Clock,
        config: clarion_config::Animation,
    ) -> Self {
        let (current, velocity) = previous
            .as_ref()
            .map_or((from.clamp(0., 1.), 0.), Self::visible_state);

        Self {
            anim: Animation::new(clock, current, to.clamp(0., 1.), velocity, config),
            hold_after_done: false,
        }
    }

    pub(crate) fn value(&self) -> f64 {
        self.anim.clamped_value().clamp(0., 1.)
    }

    pub(crate) fn visible_state(&self) -> (f64, f64) {
        // Opacity stops at its target and at the valid alpha bounds. Carry only
        // visible motion into a reversal, not the spring's hidden overshoot.
        visible_animation_state(&self.anim)
    }

    pub(crate) fn ensure_opaque(&mut self) {
        if self.anim.to() != 1. {
            let (current, velocity) = self.visible_state();
            self.anim = self.anim.restarted(current, 1., velocity);
        }
        self.hold_after_done = false;
    }
}

impl WindowPresentationState {
    pub(crate) fn resolved_corner_radius(&self, window: &impl LayoutElement) -> CornerRadius {
        let rules = window.rules();
        self.options.layout.resolve_window_corner_radius(
            rules.geometry_corner_radius,
            rules.clip_to_geometry,
            window.is_windowed_fullscreen(),
        )
    }

    pub(crate) fn resolved_clip_to_geometry(&self, window: &impl LayoutElement) -> bool {
        self.options
            .layout
            .resolve_clip_to_geometry(window.rules().clip_to_geometry)
    }

    pub(crate) fn rounding_power(&self) -> f32 {
        self.options.layout.rounding_power_f32()
    }

    pub(crate) fn decoration_time(&self) -> Option<std::time::Duration> {
        (!self.options.animations.off && !self.clock.should_complete_instantly())
            .then(|| self.clock.now())
    }

    pub(crate) fn fullscreen_progress(&self) -> f64 {
        if let Some(resize) = &self.resize_animation {
            if let Some(anim) = &resize.fullscreen_progress {
                return anim.clamped_value().clamp(0., 1.);
            }
        }

        if self.sizing_mode.is_fullscreen() {
            1.
        } else {
            0.
        }
    }

    pub(crate) fn expanded_progress(&self) -> f64 {
        if let Some(resize) = &self.resize_animation {
            if let Some(anim) = &resize.expanded_progress {
                return anim.clamped_value().clamp(0., 1.);
            }
        }

        if self.sizing_mode.is_normal() {
            0.
        } else {
            1.
        }
    }

    pub(crate) fn effective_border_width(&self) -> Option<f64> {
        if !self.sizing_mode.is_normal() {
            return None;
        }

        if self.border_config.off {
            return None;
        }

        Some(self.border_config.width)
    }

    pub(crate) fn visual_border_width(&self) -> Option<f64> {
        if self.border_config.off {
            return None;
        }

        let expanded_progress = self.expanded_progress();

        // Only hide the border when fully expanded to avoid jarring border appearance.
        if expanded_progress == 1. {
            return None;
        }

        // The tile and client geometries already retain independent size and
        // velocity across interrupted resizes. Use the same continuous expanded
        // trajectory for the visible border instead of removing its full width
        // on the final frame.
        Some(self.border_config.width * (1. - expanded_progress))
    }

    pub(crate) fn window_loc(&self, window: &impl LayoutElement) -> Point<f64, Logical> {
        let mut loc = Point::from((0., 0.));

        let window_size = self.animated_window_size(window);
        let target_size = self.animated_tile_size(window);

        // Center the window within its tile.
        //
        // - Without borders, the sizes match, so this difference is zero.
        // - Borders always match from all sides, so this difference is pre-rounded to physical.
        // - In fullscreen, if the window is smaller than the tile, then it gets centered, otherwise
        //   the tile size matches the window.
        // - During animations, the window remains centered within the tile; this is important for
        //   the to/from fullscreen animation.
        loc.x += (target_size.w - window_size.w) / 2.;
        loc.y += (target_size.h - window_size.h) / 2.;

        // Round to physical pixels.
        loc = loc
            .to_physical_precise_round(self.scale)
            .to_logical(self.scale);

        loc
    }

    pub(crate) fn tile_size(&self, window: &impl LayoutElement) -> Size<f64, Logical> {
        let mut size = self.window_size(window);

        if self.sizing_mode.is_fullscreen() {
            // Normally we'd just return the fullscreen size here, but this makes things a bit
            // nicer if a fullscreen window is bigger than the fullscreen size for some reason.
            size.w = f64::max(size.w, self.view_size.w);
            size.h = f64::max(size.h, self.view_size.h);
            return size;
        }

        if let Some(width) = self.effective_border_width() {
            size.w += width * 2.;
            size.h += width * 2.;
        }

        size
    }

    pub(crate) fn window_size(&self, window: &impl LayoutElement) -> Size<f64, Logical> {
        let mut size = window.size().to_f64();
        size = size
            .to_physical_precise_round(self.scale)
            .to_logical(self.scale);
        size
    }

    pub(crate) fn animated_window_size(&self, window: &impl LayoutElement) -> Size<f64, Logical> {
        let mut size = self.window_size(window);

        if let Some(resize) = &self.resize_animation {
            size = resize.size_value(size, false);
            size.w = f64::max(1., size.w);
            size.h = f64::max(1., size.h);
            size = size
                .to_physical_precise_round(self.scale)
                .to_logical(self.scale);
        }

        size
    }

    pub(crate) fn animated_tile_size(&self, window: &impl LayoutElement) -> Size<f64, Logical> {
        let mut size = self.tile_size(window);

        if let Some(resize) = &self.resize_animation {
            size = resize.size_value(size, true);
            size.w = f64::max(1., size.w);
            size.h = f64::max(1., size.h);
            size = size
                .to_physical_precise_round(self.scale)
                .to_logical(self.scale);
        }

        size
    }

    pub(crate) fn bob_offset(&self, window: &impl LayoutElement) -> Point<f64, Logical> {
        if window.rules().baba_is_float != Some(true) {
            return Point::from((0., 0.));
        }

        let Some(time) = self.decoration_time() else {
            return Point::from((0., 0.));
        };
        let y = baba_is_float_offset(time, self.view_size.h);
        let y = round_logical_in_physical(self.scale, y);
        Point::from((0., y))
    }
}

impl WindowPresentationState {
    pub(crate) fn has_animated_gradient(&self, active: bool, urgent: bool) -> bool {
        [&self.border_config, &self.focus_ring_config]
            .into_iter()
            .any(|config| {
                if config.off || config.width <= 0. {
                    return false;
                }
                let gradient = if urgent {
                    config.urgent_gradient
                } else if active {
                    config.active_gradient
                } else {
                    config.inactive_gradient
                };
                gradient.is_some_and(|g| g.rotate_deg_per_sec.is_some_and(|speed| speed.0 != 0.))
            })
    }
}
