use core::f64;
use std::cell::RefCell;
use std::rc::Rc;

use clarion_config::utils::MergeWith as _;
use clarion_config::{Color, CornerRadius, GradientInterpolation};
use clarion_ipc::WindowLayout;
use smithay::backend::renderer::element::{Element, Kind};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::{Logical, Physical, Point, Rectangle, Scale, Size};

use super::focus_ring::{FocusRing, FocusRingRenderElement};
use super::opening_window::{OpenAnimation, OpeningWindowRenderElement};
use super::shadow::Shadow;
use super::wobble::{WobbleAnchor, WobbleForecast, WobbleState};
use super::{
    HitType, LayoutElement, LayoutElementRenderElement, LayoutElementRenderSnapshot, Options,
    SizeFrac, RESIZE_ANIMATION_THRESHOLD,
};
use crate::animation::{Animation, Clock};
use crate::clarion_render_elements;
use crate::layout::SizingMode;
use crate::render_helpers::background_effect::BackgroundEffectElement;
use crate::render_helpers::border::BorderRenderElement;
use crate::render_helpers::clipped_surface::{ClippedSurfaceRenderElement, RoundedCornerDamage};
use crate::render_helpers::damage::ExtraDamage;
use crate::render_helpers::mesh::MeshRenderElement;
use crate::render_helpers::offscreen::{OffscreenBuffer, OffscreenData, OffscreenRenderElement};
use crate::render_helpers::renderer::{AnyRenderer, ClarionRenderer};
use crate::render_helpers::resize::{
    capture_resize_texture, CapturedResizeTexture, ResizeRenderElement,
};
use crate::render_helpers::shadow::ShadowRenderElement;
use crate::render_helpers::snapshot::RenderSnapshot;
use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
use crate::render_helpers::xray::{Xray, XrayPos};
use crate::render_helpers::{RenderCtx, RenderTarget};
use crate::utils::transaction::Transaction;
use crate::utils::{
    baba_is_float_offset, round_logical_in_physical, round_logical_in_physical_max1,
};

/// Toplevel window with decorations.
#[derive(Debug)]
pub struct Tile<W: LayoutElement> {
    /// The toplevel window itself.
    window: W,

    /// The border around the window.
    border: FocusRing,

    /// The focus ring around the window.
    focus_ring: FocusRing,

    /// The shadow around the window.
    shadow: Shadow,

    /// This tile's current sizing mode.
    ///
    /// This will update only when the `window` actually goes maximized or fullscreen, rather than
    /// right away, to avoid black backdrop flicker before the window has had a chance to resize.
    sizing_mode: SizingMode,

    /// The black backdrop for fullscreen windows.
    fullscreen_backdrop: SolidColorBuffer,

    /// Whether the tile should float upon unfullscreening.
    pub(super) restore_to_floating: bool,

    /// The size that the window should assume when going floating.
    ///
    /// This is generally the last size the window had when it was floating. It can be unknown if
    /// the window starts out in the tiling layout or fullscreen.
    pub(super) floating_window_size: Option<Size<i32, Logical>>,

    /// The position that the tile should assume when going floating, relative to the floating
    /// space working area.
    ///
    /// This is generally the last position the tile had when it was floating. It can be unknown if
    /// the window starts out in the tiling layout.
    pub(super) floating_pos: Option<Point<f64, SizeFrac>>,

    /// Currently selected preset width index when this tile is floating.
    pub(super) floating_preset_width_idx: Option<usize>,

    /// Currently selected preset height index when this tile is floating.
    pub(super) floating_preset_height_idx: Option<usize>,

    /// The animation upon opening a window.
    open_animation: Option<OpenAnimation>,

    /// The animation of the window resizing.
    resize_animation: Option<ResizeAnimation>,

    /// Prepared at actual commit application, immediately before update_window().
    /// The raw target size drives geometry; the optional flattened image drives content.
    pending_resize_snapshot: Option<(ResizeSnapshot, Option<ResizeSnapshot>)>,

    /// The animation of a tile visually moving horizontally.
    move_x_animation: Option<MoveAnimation>,

    /// The animation of a tile visually moving vertically.
    move_y_animation: Option<MoveAnimation>,

    /// The animation of the tile's opacity.
    pub(super) alpha_animation: Option<AlphaAnimation>,

    /// Elastic geometry and its single cached interaction texture.
    wobble: RefCell<WobbleState>,
    wobble_forecast: RefCell<WobbleForecast>,
    wobble_offscreen: OffscreenBuffer,
    /// Vulkan counterpart keeping damage history across wobble frames.
    wobble_native_offscreen: crate::render_helpers::offscreen::NativeOffscreenBuffer,

    /// Destination supplied by the shell, in this output's logical coordinates.
    minimize_target: Option<clarion_ipc::DockTarget>,
    minimize_reduce_motion: bool,
    genie: RefCell<Option<super::minimized::Genie>>,
    genie_offscreen: [OffscreenBuffer; RenderTarget::COUNT],
    genie_native_offscreen:
        [crate::render_helpers::offscreen::NativeOffscreenBuffer; RenderTarget::COUNT],

    /// Offset during the initial interactive move rubberband.
    pub(super) interactive_move_offset: Point<f64, Logical>,

    /// Snapshot of the last render for use in the close animation.
    unmap_snapshot: Option<TileRenderSnapshot>,
    native_unmap_snapshot: Option<NativeTileSnapshot>,

    /// Extra damage for clipped surface corner radius changes.
    rounded_corner_damage: RoundedCornerDamage,

    /// Whether this tile was active/focused in the last render element update.
    is_active: bool,
    last_deco_size: Option<Size<f64, Logical>>,
    last_deco_view: Option<Rectangle<f64, Logical>>,

    /// The view size for the tile's workspace.
    ///
    /// Used as the fullscreen target size.
    view_size: Size<f64, Logical>,

    /// Scale of the output the tile is on (and rounds its sizes to).
    scale: f64,

    /// Clock for driving animations.
    pub(super) clock: Clock,

    /// Configurable properties of the layout.
    pub(super) options: Rc<Options>,
}

clarion_render_elements! {
    TileRenderElement<R> => {
        LayoutElement = LayoutElementRenderElement<R>,
        FocusRing = FocusRingRenderElement,
        SolidColor = SolidColorRenderElement,
        Opening = OpeningWindowRenderElement,
        PrimaryGpu = crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement,
        Resize = ResizeRenderElement,
        Border = BorderRenderElement,
        Shadow = ShadowRenderElement,
        ClippedSurface = ClippedSurfaceRenderElement<R>,
        Offscreen = OffscreenRenderElement,
        ExtraDamage = ExtraDamage,
        BackgroundEffect = BackgroundEffectElement,
        Mesh = MeshRenderElement,
    }
}

pub type TileRenderSnapshot =
    RenderSnapshot<TileRenderElement<GlesRenderer>, TileRenderElement<GlesRenderer>>;

/// A cached texture together with the physical rectangle it occupies and its logical size.
///
/// Keeping this shape behind a name makes the GLES and native resize paths easier to read while
/// ensuring they continue to return the same data to their callers.
type ResizeTextureInput<T> = ((T, Rectangle<i32, Physical>), Size<f64, Logical>);

/// Separate snapshots preserve each capture target's privacy rules during closing.
#[derive(Debug)]
pub struct NativeTileSnapshot {
    pub targets: [(
        crate::render_helpers::vulkan::VulkanTexture,
        Point<f64, Logical>,
    ); 3],
}

#[derive(Debug)]
enum ResizeSnapshot {
    Gles(LayoutElementRenderSnapshot),
    Native(LayoutElementRenderSnapshot<crate::render_helpers::vulkan::VulkanTexture>),
    CompositedGles(
        [(
            CapturedResizeTexture<smithay::backend::renderer::gles::GlesTexture>,
            bool,
        ); RenderTarget::COUNT],
    ),
    CompositedNative(
        [(
            CapturedResizeTexture<crate::render_helpers::vulkan::VulkanTexture>,
            bool,
        ); RenderTarget::COUNT],
    ),
}
impl ResizeSnapshot {
    fn size(&self) -> Size<f64, Logical> {
        match self {
            Self::Gles(s) => s.size,
            Self::Native(s) => s.size,
            Self::CompositedGles(s) => s[0].0.size(),
            Self::CompositedNative(s) => s[0].0.size(),
        }
    }

    fn blocked_out(&self, target: RenderTarget) -> bool {
        match self {
            Self::Gles(s) => target.should_block_out(s.block_out_from),
            Self::Native(s) => target.should_block_out(s.block_out_from),
            Self::CompositedGles(s) => s[target as usize].1,
            Self::CompositedNative(s) => s[target as usize].1,
        }
    }

    fn gles_texture(
        &self,
        renderer: &mut GlesRenderer,
        target: RenderTarget,
        scale: Scale<f64>,
    ) -> Option<ResizeTextureInput<smithay::backend::renderer::gles::GlesTexture>> {
        match self {
            Self::Gles(s) => {
                let texture = s
                    .texture(
                        RenderCtx {
                            renderer,
                            target,
                            xray: None,
                        },
                        scale,
                    )?
                    .clone();
                let cached_scale = s.texture_scale(target)?;
                let size = Size::from((
                    s.size.w * cached_scale.x / scale.x,
                    s.size.h * cached_scale.y / scale.y,
                ));
                Some((texture, size))
            }
            Self::CompositedGles(s) => Some(s[target as usize].0.as_input(scale)),
            _ => None,
        }
    }

    fn native_texture(
        &self,
        renderer: &mut crate::render_helpers::vulkan::native::NativeRenderer,
        target: RenderTarget,
        scale: Scale<f64>,
    ) -> Option<ResizeTextureInput<crate::render_helpers::vulkan::VulkanTexture>> {
        match self {
            Self::Native(s) => {
                let texture = s
                    .texture(
                        RenderCtx {
                            renderer,
                            target,
                            xray: None,
                        },
                        scale,
                    )?
                    .clone();
                let cached_scale = s.texture_scale(target)?;
                let size = Size::from((
                    s.size.w * cached_scale.x / scale.x,
                    s.size.h * cached_scale.y / scale.y,
                ));
                Some((texture, size))
            }
            Self::CompositedNative(s) => Some(s[target as usize].0.as_input(scale)),
            _ => None,
        }
    }
}

#[derive(Debug)]
struct ResizeAnimation {
    anim: Animation,
    momentum: Option<ResizeMomentum>,
    size_from: Size<f64, Logical>,
    snapshot: ResizeSnapshot,
    offscreen: OffscreenBuffer,
    /// Vulkan counterpart of `offscreen`: reused across resize frames so growth
    /// reallocates but unchanged pixels are not re-composited every frame.
    native_offscreen: crate::render_helpers::offscreen::NativeOffscreenBuffer,
    tile_size_from: Size<f64, Logical>,
    // If the resize involved the fullscreen state at some point, this is the progress toward the
    // fullscreen state. Used for things like fullscreen backdrop alpha.
    //
    // Note that this can be set even if this specific resize is between two non-fullscreen states,
    // for example when issuing a new resize during an unfullscreen resize.
    fullscreen_progress: Option<Animation>,
    // Similar to above but for fullscreen-or-maximized.
    expanded_progress: Option<Animation>,
}

/// One homogeneous spring response carries all four size velocities. The main
/// 0..1 animation remains the snapshot blend controller; both responses use the
/// same spring parameters, so their sum is the spring for each new size target.
#[derive(Debug)]
struct ResizeMomentum {
    anim: Animation,
    window_weights: Point<f64, Logical>,
    tile_weights: Point<f64, Logical>,
}

impl ResizeMomentum {
    fn new(
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
    fn size_value(&self, to: Size<f64, Logical>, tile: bool) -> Size<f64, Logical> {
        let size = self.size_components(to, tile);
        Size::from((size.x.max(1.), size.y.max(1.)))
    }

    fn size_components(&self, to: Size<f64, Logical>, tile: bool) -> Point<f64, Logical> {
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

    fn size_state(
        &self,
        to: Size<f64, Logical>,
        tile: bool,
    ) -> (Size<f64, Logical>, Point<f64, Logical>) {
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

    fn is_done(&self) -> bool {
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

fn resize_decoration_animation(
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

fn visible_animation_state(anim: &Animation) -> (f64, f64) {
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

#[derive(Debug)]
pub(super) struct AlphaAnimation {
    pub(super) anim: Animation,
    /// Whether the animation should persist after it's done.
    ///
    /// This is used by things like interactive move which need to animate alpha to
    /// semitransparent, then hold it at semitransparent for a while, until the operation
    /// completes.
    pub(super) hold_after_done: bool,
    offscreen: OffscreenBuffer,
    /// Vulkan counterpart of `offscreen`: the snapshot target and its damage history
    /// are reused across fade frames instead of being re-allocated every frame.
    native_offscreen: crate::render_helpers::offscreen::NativeOffscreenBuffer,
}

impl AlphaAnimation {
    fn retarget(
        previous: Option<Self>,
        from: f64,
        to: f64,
        clock: Clock,
        config: clarion_config::Animation,
    ) -> Self {
        let (current, velocity, offscreen, native_offscreen) = if let Some(previous) = previous {
            let (current, velocity) = previous.visible_state();
            (
                current,
                velocity,
                previous.offscreen,
                previous.native_offscreen,
            )
        } else {
            (
                from.clamp(0., 1.),
                0.,
                OffscreenBuffer::default(),
                Default::default(),
            )
        };

        Self {
            anim: Animation::new(clock, current, to.clamp(0., 1.), velocity, config),
            hold_after_done: false,
            offscreen,
            native_offscreen,
        }
    }

    fn value(&self) -> f64 {
        self.anim.clamped_value().clamp(0., 1.)
    }

    fn visible_state(&self) -> (f64, f64) {
        // Opacity stops at its target and at the valid alpha bounds. Carry only
        // visible motion into a reversal, not the spring's hidden overshoot.
        visible_animation_state(&self.anim)
    }

    fn ensure_opaque(&mut self) {
        if self.anim.to() != 1. {
            let (current, velocity) = self.visible_state();
            self.anim = self.anim.restarted(current, 1., velocity);
        }
        self.hold_after_done = false;
    }
}

impl<W: LayoutElement> Tile<W> {
    pub(crate) fn resolved_corner_radius(&self) -> CornerRadius {
        let rules = self.window.rules();
        self.options.layout.resolve_window_corner_radius(
            rules.geometry_corner_radius,
            rules.clip_to_geometry,
            self.window.is_windowed_fullscreen(),
        )
    }

    pub(super) fn resolved_clip_to_geometry(&self) -> bool {
        self.options
            .layout
            .resolve_clip_to_geometry(self.window.rules().clip_to_geometry)
    }

    fn rounding_power(&self) -> f32 {
        self.options.layout.rounding_power_f32()
    }

    pub fn new(
        window: W,
        view_size: Size<f64, Logical>,
        scale: f64,
        clock: Clock,
        options: Rc<Options>,
    ) -> Self {
        let rules = window.rules();
        let border_config = options.layout.border.merged_with(&rules.border);
        let focus_ring_config = options.layout.focus_ring.merged_with(&rules.focus_ring);
        let shadow_config = options.layout.shadow.merged_with(&rules.shadow);
        let sizing_mode = window.sizing_mode();
        let rounding_power = options.layout.rounding_power.max(2.) as f32;
        let mut border = FocusRing::new(border_config.into());
        let mut focus_ring = FocusRing::new(focus_ring_config);
        border.set_rounding_power(rounding_power);
        focus_ring.set_rounding_power(rounding_power);

        Self {
            window,
            border,
            focus_ring,
            shadow: Shadow::new(shadow_config),
            sizing_mode,
            fullscreen_backdrop: SolidColorBuffer::new((0., 0.), [0., 0., 0., 1.]),
            restore_to_floating: false,
            floating_window_size: None,
            floating_pos: None,
            floating_preset_width_idx: None,
            floating_preset_height_idx: None,
            open_animation: None,
            resize_animation: None,
            pending_resize_snapshot: None,
            move_x_animation: None,
            move_y_animation: None,
            alpha_animation: None,
            wobble: RefCell::new(WobbleState::default()),
            wobble_forecast: RefCell::new(WobbleForecast::default()),
            minimize_target: None,
            minimize_reduce_motion: false,
            genie: RefCell::new(None),
            genie_offscreen: std::array::from_fn(|_| OffscreenBuffer::default()),
            genie_native_offscreen: std::array::from_fn(|_| {
                crate::render_helpers::offscreen::NativeOffscreenBuffer::default()
            }),
            wobble_offscreen: OffscreenBuffer::default(),
            wobble_native_offscreen:
                crate::render_helpers::offscreen::NativeOffscreenBuffer::default(),
            interactive_move_offset: Point::from((0., 0.)),
            unmap_snapshot: None,
            native_unmap_snapshot: None,
            rounded_corner_damage: Default::default(),
            is_active: false,
            last_deco_size: None,
            last_deco_view: None,
            view_size,
            scale,
            clock,
            options,
        }
    }

    pub fn update_config(
        &mut self,
        view_size: Size<f64, Logical>,
        scale: f64,
        options: Rc<Options>,
    ) {
        // If preset widths or heights changed, clear our stored preset index.
        if self.options.layout.preset_column_widths != options.layout.preset_column_widths {
            self.floating_preset_width_idx = None;
        }
        if self.options.layout.preset_window_heights != options.layout.preset_window_heights {
            self.floating_preset_height_idx = None;
        }

        if self.view_size != view_size || self.scale != scale || options.animations.off {
            self.complete_genie();
        }
        self.view_size = view_size;
        self.scale = scale;
        self.options = options;
        self.last_deco_size = None;
        self.last_deco_view = None;

        if self.options.animations.off || self.options.animations.window_wobble.off {
            self.wobble.borrow_mut().cancel();
            self.clear_wobble_render_cache();
        }

        let round_max1 = |logical| round_logical_in_physical_max1(self.scale, logical);

        let rules = self.window.rules();

        let mut border_config = self.options.layout.border.merged_with(&rules.border);
        border_config.width = round_max1(border_config.width);
        self.border.update_config(border_config.into());

        let mut focus_ring_config = self
            .options
            .layout
            .focus_ring
            .merged_with(&rules.focus_ring);
        focus_ring_config.width = round_max1(focus_ring_config.width);
        self.focus_ring.update_config(focus_ring_config);

        let shadow_config = self.options.layout.shadow.merged_with(&rules.shadow);
        self.shadow.update_config(shadow_config);

        self.window.update_config(self.options.blur);
    }

    /// Retire cached pixels from the previous native-device epoch. Client buffer
    /// leases held by submitted GPU work are retained by that work's Arc owners.
    /// Geometry, focus, minimization and interaction state remain unchanged.
    pub(crate) fn on_native_renderer_replaced(&mut self) {
        self.resize_animation = None;
        self.pending_resize_snapshot = None;
        self.native_unmap_snapshot = None;
        let _ = self.window.take_native_animation_snapshot();
        self.window.set_offscreen_data(None);
        if let Some(open) = &self.open_animation {
            open.clear_native_cache();
        }
        if let Some(alpha) = &self.alpha_animation {
            alpha.native_offscreen.clear();
        }
        self.wobble_native_offscreen.clear();
        for buffer in &self.genie_native_offscreen {
            buffer.clear();
        }
    }

    pub fn update_shaders(&mut self) {
        self.border.update_shaders();
        self.focus_ring.update_shaders();
        self.shadow.update_shaders();
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
        let Some(resize) = &self.resize_animation else {
            return;
        };
        if resize.anim.is_clamped_done()
            && self
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
        self.pending_resize_snapshot = Some((raw, captured));
    }

    fn capture_resize_retarget(
        &self,
        renderer: AnyRenderer<'_>,
        next: &ResizeSnapshot,
    ) -> anyhow::Result<ResizeSnapshot> {
        let resize = self.resize_animation.as_ref().unwrap();
        let scale = Scale::from(self.scale);
        let size = resize
            .size_value(next.size(), false)
            .to_physical_precise_round(scale)
            .to_logical(scale);
        let area = Rectangle::from_size(size);
        let progress = resize.anim.value() as f32;
        let clamped = resize.anim.clamped_value().clamp(0., 1.) as f32;
        let targets = [
            RenderTarget::Output,
            RenderTarget::Screencast,
            RenderTarget::ScreenCapture,
        ];
        match renderer {
            AnyRenderer::Gles(renderer) => {
                anyhow::ensure!(
                    ResizeRenderElement::has_shader(renderer),
                    "resize shader unavailable"
                );
                let mut captured = Vec::with_capacity(RenderTarget::COUNT);
                for target in targets {
                    let (previous, previous_size) = resize
                        .snapshot
                        .gles_texture(renderer, target, scale)
                        .ok_or_else(|| {
                            anyhow::anyhow!("previous GLES resize snapshot unavailable")
                        })?;
                    let (next_texture, next_size) =
                        next.gles_texture(renderer, target, scale).ok_or_else(|| {
                            anyhow::anyhow!("committed GLES resize snapshot unavailable")
                        })?;
                    // Keep decoration masks and opacity outside this owned content image.
                    let elem = ResizeRenderElement::new(
                        area,
                        scale,
                        previous,
                        previous_size,
                        next_texture,
                        next_size,
                        progress,
                        clamped,
                        CornerRadius::default(),
                        false,
                        1.,
                    );
                    let texture = capture_resize_texture(renderer, &elem, scale, size)?;
                    captured.push((
                        texture,
                        resize.snapshot.blocked_out(target) && next.blocked_out(target),
                    ));
                }
                Ok(ResizeSnapshot::CompositedGles(captured.try_into().unwrap()))
            }
            AnyRenderer::Native(renderer) => {
                let mut captured = Vec::with_capacity(RenderTarget::COUNT);
                for target in targets {
                    let (previous, previous_size) = resize
                        .snapshot
                        .native_texture(renderer, target, scale)
                        .ok_or_else(|| {
                            anyhow::anyhow!("previous Vulkan resize snapshot unavailable")
                        })?;
                    let (next_texture, next_size) = next
                        .native_texture(renderer, target, scale)
                        .ok_or_else(|| {
                            anyhow::anyhow!("committed Vulkan resize snapshot unavailable")
                        })?;
                    let elem = ResizeRenderElement::new_native(
                        renderer,
                        self.options
                            .animations
                            .window_resize
                            .custom_shader
                            .as_deref(),
                        area,
                        scale,
                        previous,
                        previous_size,
                        next_texture,
                        next_size,
                        progress,
                        clamped,
                        CornerRadius::default(),
                        false,
                        1.,
                    )?;
                    let texture = capture_resize_texture(renderer, &elem, scale, size)?;
                    captured.push((
                        texture,
                        resize.snapshot.blocked_out(target) && next.blocked_out(target),
                    ));
                }
                Ok(ResizeSnapshot::CompositedNative(
                    captured.try_into().unwrap(),
                ))
            }
        }
    }

    pub fn update_window(&mut self) {
        let prev_sizing_mode = self.sizing_mode;
        self.sizing_mode = self.window.sizing_mode();

        if let Some((animate_from, continuous)) = self
            .pending_resize_snapshot
            .take()
            .or_else(|| self.take_resize_snapshot().map(|raw| (raw, None)))
        {
            let params = if let Some(resize) = self.resize_animation.take() {
                // Compute like in animated_window_size(), but using the snapshot geometry (since
                // the current one is already overwritten).
                let (size, size_velocity) = resize.size_state(animate_from.size(), false);

                let mut tile_size = animate_from.size();
                if prev_sizing_mode.is_fullscreen() {
                    tile_size.w = f64::max(tile_size.w, self.view_size.w);
                    tile_size.h = f64::max(tile_size.h, self.view_size.h);
                } else if prev_sizing_mode.is_normal() && !self.border.is_off() {
                    let width = self.border.width();
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
                    resize.offscreen,
                    resize.native_offscreen,
                )
            } else {
                let size = animate_from.size();

                // Compute like in tile_size().
                let mut tile_size = size;
                if prev_sizing_mode.is_fullscreen() {
                    tile_size.w = f64::max(tile_size.w, self.view_size.w);
                    tile_size.h = f64::max(tile_size.h, self.view_size.h);
                } else if prev_sizing_mode.is_normal() && !self.border.is_off() {
                    let width = self.border.width();
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
                    OffscreenBuffer::default(),
                    Default::default(),
                )
            };
            let (
                size_from,
                tile_size_from,
                size_velocity,
                tile_velocity,
                fullscreen_previous,
                expanded_previous,
                offscreen,
                native_offscreen,
            ) = params;

            let config = self.options.animations.window_resize.anim;
            let momentum =
                ResizeMomentum::new(self.clock.clone(), size_velocity, tile_velocity, config);
            let fullscreen_progress = resize_decoration_animation(
                fullscreen_previous,
                if prev_sizing_mode.is_fullscreen() {
                    1.
                } else {
                    0.
                },
                if self.sizing_mode.is_fullscreen() {
                    1.
                } else {
                    0.
                },
                self.clock.clone(),
                config,
            );
            let expanded_progress = resize_decoration_animation(
                expanded_previous,
                if prev_sizing_mode.is_normal() { 0. } else { 1. },
                if self.sizing_mode.is_normal() { 0. } else { 1. },
                self.clock.clone(),
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
                let anim = Animation::new(self.clock.clone(), 0., 1., 0., config);

                self.resize_animation = Some(ResizeAnimation {
                    anim,
                    momentum,
                    size_from,
                    snapshot: continuous.unwrap_or(animate_from),
                    offscreen,
                    native_offscreen,
                    tile_size_from,
                    fullscreen_progress,
                    expanded_progress,
                });
            } else {
                self.resize_animation = None;
            }
        }

        let round_max1 = |logical| round_logical_in_physical_max1(self.scale, logical);

        let rules = self.window.rules();
        let mut border_config = self.options.layout.border.merged_with(&rules.border);
        border_config.width = round_max1(border_config.width);
        self.border.update_config(border_config.into());

        let mut focus_ring_config = self
            .options
            .layout
            .focus_ring
            .merged_with(&rules.focus_ring);
        focus_ring_config.width = round_max1(focus_ring_config.width);
        self.focus_ring.update_config(focus_ring_config);

        let shadow_config = self.options.layout.shadow.merged_with(&rules.shadow);
        self.shadow.update_config(shadow_config);

        let window_size = self.window_size();
        let radius = self
            .resolved_corner_radius()
            .fit_to(window_size.w as f32, window_size.h as f32);
        self.rounded_corner_damage.set_corner_radius(radius);
        self.border.set_rounding_power(self.rounding_power());
        self.focus_ring.set_rounding_power(self.rounding_power());
    }

    pub fn advance_animations(&mut self) {
        if self.genie.borrow().as_ref().is_some_and(|g| g.is_done()) {
            self.complete_genie();
        }
        if let Some(open) = &mut self.open_animation {
            if open.is_done() {
                self.open_animation = None;
            }
        }

        if let Some(resize) = &mut self.resize_animation {
            if resize.is_done() {
                self.resize_animation = None;
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

        if let Some(alpha) = &mut self.alpha_animation {
            if !alpha.hold_after_done && alpha.anim.is_clamped_done() {
                self.alpha_animation = None;
            }
        }
    }

    pub fn has_effect_pose(&self) -> bool {
        self.wobble.borrow().is_active() || self.genie.borrow().is_some()
    }

    /// Update canonical effects from the actual event-time scene pose. Output
    /// forecasts may subsequently sample this state without advancing it.
    pub fn advance_effect_pose(&self, location: Point<f64, Logical>) {
        if !self.has_effect_pose() {
            return;
        }
        let area = Rectangle::new(location, self.animated_tile_size());
        let now = self.clock.now();
        if self.wobble.borrow().is_active() {
            self.wobble
                .borrow_mut()
                .advance(area, self.options.animations.window_wobble, now);
            if !self.wobble.borrow().is_active() {
                self.clear_wobble_render_cache();
            }
        }
        if let Some(genie) = self.genie.borrow_mut().as_mut() {
            genie.retarget_destination(area);
        }
    }

    pub fn are_animations_ongoing(&self) -> bool {
        let is_active = self.is_active;
        self.are_transitions_ongoing()
            || (self.decoration_time().is_some()
                && (self.window.rules().baba_is_float == Some(true)
                    || self
                        .border
                        .has_animated_gradient(is_active, self.window.is_urgent())
                    || self
                        .focus_ring
                        .has_animated_gradient(is_active, self.window.is_urgent())))
    }

    pub fn are_transitions_ongoing(&self) -> bool {
        self.genie.borrow().is_some()
            || self.open_animation.is_some()
            || self.resize_animation.is_some()
            || self.move_x_animation.is_some()
            || self.move_y_animation.is_some()
            || self
                .alpha_animation
                .as_ref()
                .is_some_and(|alpha| !alpha.anim.is_clamped_done())
            || self.wobble.borrow().is_ongoing()
    }

    pub fn start_wobble(&mut self, anchor: WobbleAnchor) {
        if self.options.animations.off || self.clock.should_complete_instantly() {
            return;
        }
        if self.wobble.borrow().is_active() {
            return;
        }
        self.wobble.borrow_mut().begin(
            anchor,
            self.options.animations.window_wobble,
            self.clock.now(),
        );
    }

    pub fn add_pending_wobble_displacement(&mut self, delta: Point<f64, Logical>) {
        self.wobble.borrow_mut().add_pending_displacement(delta);
    }

    pub fn finish_wobble(&mut self) {
        self.wobble.borrow_mut().finish();
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
        let animated_tile_size = self.animated_tile_size();
        let decoration_time = self.decoration_time();
        let gradient_only = self.is_active == is_active
            && !self.are_transitions_ongoing()
            && self.last_deco_size == Some(animated_tile_size)
            && self.last_deco_view == Some(view_rect);

        self.is_active = is_active;
        self.last_deco_size = Some(animated_tile_size);
        self.last_deco_view = Some(view_rect);

        let rules = self.window.rules();
        let expanded_progress = self.expanded_progress();

        let draw_border_with_background = rules
            .draw_border_with_background
            .unwrap_or_else(|| !self.window.has_ssd());
        let border_width = self.visual_border_width().unwrap_or(0.);

        // Do the inverse of tile_size() in order to handle the unfullscreen animation for windows
        // that were smaller than the fullscreen size, and therefore their animated_window_size() is
        // currently much smaller than the tile size.
        let mut border_window_size = animated_tile_size;
        border_window_size.w -= border_width * 2.;
        border_window_size.h -= border_width * 2.;

        // FIXME: this takes into account the animation from normal sizing mode to
        // maximized/fullscreen, but it doesn't take into account the corner radius animation from
        // the window itself.
        //
        // Currently, an easy way to see the problem is to start from a window with a nonzero
        // radius, then go from windowed fullscreen (that forces 0 radius) to regular fullscreen.
        // At the start of the animation, windowed fullscreen becomes false, but the window hasn't
        // animated to the normal fullscreen yet, so the radius here jumps to its nonzero value,
        // even though it should remain zero throughout.
        //
        // Later, when windows get the surface shape protocol with radii, this issue will happen
        // when that changes between animated commits.
        let radius = self
            .resolved_corner_radius()
            .expanded_by(border_width as f32)
            .scaled_by(1. - expanded_progress as f32);
        if gradient_only {
            self.border.update_render_elements(
                border_window_size,
                is_active,
                !draw_border_with_background,
                self.window.is_urgent(),
                Rectangle::new(
                    view_rect.loc - Point::from((border_width, border_width)),
                    view_rect.size,
                ),
                radius,
                self.scale,
                1. - expanded_progress as f32,
                decoration_time,
            );
            let radius = if self.visual_border_width().is_some() {
                radius
            } else {
                self.resolved_corner_radius()
                    .scaled_by(1. - expanded_progress as f32)
            };
            self.shadow.update_render_elements(
                animated_tile_size,
                is_active,
                radius,
                self.scale,
                1. - expanded_progress as f32,
            );
            let radius = radius.expanded_by(self.focus_ring.width() as f32);
            self.focus_ring.update_render_elements(
                animated_tile_size,
                is_active,
                if self.border.is_off() {
                    !draw_border_with_background
                } else {
                    true
                },
                self.window.is_urgent(),
                view_rect,
                radius,
                self.scale,
                1. - expanded_progress as f32,
                decoration_time,
            );
            return;
        }

        self.border.update_render_elements(
            border_window_size,
            is_active,
            !draw_border_with_background,
            self.window.is_urgent(),
            Rectangle::new(
                view_rect.loc - Point::from((border_width, border_width)),
                view_rect.size,
            ),
            radius,
            self.scale,
            1. - expanded_progress as f32,
            decoration_time,
        );

        let radius = if self.visual_border_width().is_some() {
            radius
        } else {
            self.resolved_corner_radius()
                .scaled_by(1. - expanded_progress as f32)
        };
        self.shadow.update_render_elements(
            animated_tile_size,
            is_active,
            radius,
            self.scale,
            1. - expanded_progress as f32,
        );

        let draw_focus_ring_with_background = if self.border.is_off() {
            draw_border_with_background
        } else {
            false
        };
        let radius = radius.expanded_by(self.focus_ring.width() as f32);
        self.focus_ring.update_render_elements(
            animated_tile_size,
            is_active,
            !draw_focus_ring_with_background,
            self.window.is_urgent(),
            view_rect,
            radius,
            self.scale,
            1. - expanded_progress as f32,
            decoration_time,
        );

        self.fullscreen_backdrop.resize(animated_tile_size);
    }

    pub fn scale(&self) -> f64 {
        self.scale
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
        let popin_ratio = self.options.animations.window_open.popin_ratio;
        self.open_animation = Some(OpenAnimation::new(
            Animation::new(
                self.clock.clone(),
                0.,
                1.,
                0.,
                self.options.animations.window_open.anim,
            ),
            popin_ratio,
            self.options.animations.window_open.glide,
            self.options.animations.window_open.custom_shader.is_none(),
            self.options.animations.window_open.effect,
        ));
    }

    pub fn resize_animation(&self) -> Option<&Animation> {
        self.resize_animation.as_ref().map(|resize| &resize.anim)
    }

    pub fn animate_move_from(&mut self, from: Point<f64, Logical>) {
        self.animate_move_from_with_config(from, self.options.animations.window_movement.0);
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
        self.animate_move_x_from_with_config(from, self.options.animations.window_movement.0);
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
            self.clock.clone(),
            config,
        ));
    }

    pub fn animate_move_y_from(&mut self, from: f64) {
        self.animate_move_y_from_with_config(from, self.options.animations.window_movement.0);
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
            self.clock.clone(),
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
            self.clock.clone(),
            self.options.animations.window_movement.0,
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
        self.alpha_animation = Some(AlphaAnimation::retarget(
            self.alpha_animation.take(),
            from,
            to,
            self.clock.clone(),
            config,
        ));
    }

    pub fn ensure_alpha_animates_to_1(&mut self) {
        if let Some(alpha) = &mut self.alpha_animation {
            alpha.ensure_opaque();
        }
    }

    pub fn hold_alpha_animation_after_done(&mut self) {
        if let Some(alpha) = &mut self.alpha_animation {
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
        (!self.options.animations.off && !self.clock.should_complete_instantly())
            .then(|| self.clock.now())
    }

    pub fn window_mut(&mut self) -> &mut W {
        &mut self.window
    }

    pub fn sizing_mode(&self) -> SizingMode {
        self.sizing_mode
    }

    fn fullscreen_progress(&self) -> f64 {
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

    fn expanded_progress(&self) -> f64 {
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

    /// Returns `None` if the border is hidden and `Some(width)` if it should be shown.
    pub fn effective_border_width(&self) -> Option<f64> {
        if !self.sizing_mode.is_normal() {
            return None;
        }

        if self.border.is_off() {
            return None;
        }

        Some(self.border.width())
    }

    fn visual_border_width(&self) -> Option<f64> {
        if self.border.is_off() {
            return None;
        }

        let expanded_progress = self.expanded_progress();

        // Only hide the border when fully expanded to avoid jarring border appearance.
        if expanded_progress == 1. {
            return None;
        }

        // FIXME: would be cool to, like, gradually resize the border from full width to 0 during
        // fullscreening, but the rest of the code isn't quite ready for that yet. It needs to
        // handle things like computing intermediate tile size when an animated resize starts during
        // an animated unfullscreen resize.
        Some(self.border.width())
    }

    /// Returns the location of the window's visual geometry within this Tile.
    pub fn window_loc(&self) -> Point<f64, Logical> {
        let mut loc = Point::from((0., 0.));

        let window_size = self.animated_window_size();
        let target_size = self.animated_tile_size();

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

    pub fn tile_size(&self) -> Size<f64, Logical> {
        let mut size = self.window_size();

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

    pub fn tile_expected_or_current_size(&self) -> Size<f64, Logical> {
        let mut size = self.window_expected_or_current_size();

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

    pub fn window_size(&self) -> Size<f64, Logical> {
        let mut size = self.window.size().to_f64();
        size = size
            .to_physical_precise_round(self.scale)
            .to_logical(self.scale);
        size
    }

    pub fn window_expected_or_current_size(&self) -> Size<f64, Logical> {
        let size = self.window.expected_size();
        let mut size = size.unwrap_or_else(|| self.window.size()).to_f64();
        size = size
            .to_physical_precise_round(self.scale)
            .to_logical(self.scale);
        size
    }

    pub fn animated_window_size(&self) -> Size<f64, Logical> {
        let mut size = self.window_size();

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

    pub fn animated_tile_size(&self) -> Size<f64, Logical> {
        let mut size = self.tile_size();

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
        if !self.border.is_off() {
            let width = self.border.width();
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
        if self.border.is_off() {
            size
        } else {
            size + self.border.width() * 2.
        }
    }

    pub fn tile_height_for_window_height(&self, size: f64) -> f64 {
        if self.border.is_off() {
            size
        } else {
            size + self.border.width() * 2.
        }
    }

    pub fn window_width_for_tile_width(&self, size: f64) -> f64 {
        if self.border.is_off() {
            size
        } else {
            size - self.border.width() * 2.
        }
    }

    pub fn window_height_for_tile_height(&self, size: f64) -> f64 {
        if self.border.is_off() {
            size
        } else {
            size - self.border.width() * 2.
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
            self.view_size.to_i32_round(),
            SizingMode::Fullscreen,
            animate,
            transaction,
        );
    }

    pub fn min_size_nonfullscreen(&self) -> Size<f64, Logical> {
        let mut size = self.window.min_size().to_f64();

        // Can't go through effective_border_width() because we might be fullscreen.
        if !self.border.is_off() {
            let width = self.border.width();

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
        if !self.border.is_off() {
            let width = self.border.width();

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
        if self.window.rules().baba_is_float != Some(true) {
            return Point::from((0., 0.));
        }

        let Some(time) = self.decoration_time() else {
            return Point::from((0., 0.));
        };
        let y = baba_is_float_offset(time, self.view_size.h);
        let y = round_logical_in_physical(self.scale, y);
        Point::from((0., y))
    }

    fn render_inner<R: ClarionRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        location: Point<f64, Logical>,
        mut xray_pos: XrayPos,
        focus_ring: bool,
        push: &mut dyn FnMut(TileRenderElement<R>),
    ) {
        let _span = clarion_tracy_span!("Tile::render_inner");

        let scale = Scale::from(self.scale);
        let fullscreen_progress = self.fullscreen_progress();
        let expanded_progress = self.expanded_progress();

        let win_alpha = if self.window.is_ignoring_opacity_window_rule() {
            1.
        } else {
            let alpha = self.window.rules().opacity.unwrap_or(1.).clamp(0., 1.);

            // Interpolate towards alpha = 1. at fullscreen.
            let p = fullscreen_progress as f32;
            alpha * (1. - p) + 1. * p
        };

        // This is here rather than in render_offset() because render_offset() is currently assumed
        // by the code to be temporary. So, for example, interactive move will try to "grab" the
        // tile at its current render offset and reset the render offset to zero by cancelling the
        // tile move animations. On the other hand, bob_offset() is not resettable, so adding it in
        // render_offset() would cause obvious animation glitches.
        //
        // This isn't to say that adding it here is perfect; indeed, it kind of breaks view_rect
        // passed to update_render_elements(). But, it works well enough for what it is.
        let bob_offset = self.bob_offset();
        let location = location + bob_offset;
        xray_pos = xray_pos.offset(bob_offset);

        let window_loc = self.window_loc();
        let window_size = self.window_size();
        let animated_window_size = self.animated_window_size();
        let window_render_loc = location + window_loc;
        let area = Rectangle::new(window_render_loc, animated_window_size);
        xray_pos = xray_pos.offset(window_loc);

        let rules = self.window.rules();

        // Clip to geometry including during the fullscreen animation to help with buggy clients
        // that submit a full-sized buffer before acking the fullscreen state (Firefox).
        let clip_to_geometry = fullscreen_progress < 1. && self.resolved_clip_to_geometry();
        let radius = self
            .resolved_corner_radius()
            .scaled_by(1. - expanded_progress as f32);

        // Popups go on top, whether it's resize or not.
        self.window.render_popups(
            ctx.r(),
            window_render_loc,
            scale,
            win_alpha,
            xray_pos,
            &mut |elem| push(elem.into()),
        );

        let mut pushed_resize = false;
        if let Some(resize) = &self.resize_animation {
            if ResizeRenderElement::has_shader(ctx.renderer) {
                if let Some(mut ctx) = ctx.as_gles() {
                    if let Some((texture_from, snapshot_size)) =
                        resize
                            .snapshot
                            .gles_texture(ctx.renderer, ctx.target, scale)
                    {
                        let mut window_elements = Vec::new();
                        self.window.render_normal(
                            ctx.r(),
                            Point::from((0., 0.)),
                            scale,
                            1.,
                            &mut |elem| window_elements.push(elem),
                        );

                        let current = resize
                            .offscreen
                            .render(ctx.renderer, scale, &window_elements)
                            .map_err(|err| warn!("error rendering window to texture: {err:?}"))
                            .ok();

                        // Clip blocked-out resizes unconditionally because they use solid color render
                        // elements.
                        let clip_to_geometry = if resize.snapshot.blocked_out(ctx.target)
                            && ctx.target.should_block_out(rules.block_out_from)
                        {
                            true
                        } else {
                            clip_to_geometry
                        };

                        if let Some((elem_current, _sync_point, mut data)) = current {
                            let texture_current = elem_current.texture().clone();
                            // The offset and size are computed in physical pixels and converted to
                            // logical with the same `scale`, so converting them back with rounding
                            // inside the geometry() call gives us the same physical result back.
                            let texture_current_geo = elem_current.geometry(scale);

                            let elem = ResizeRenderElement::new(
                                area,
                                scale,
                                texture_from.clone(),
                                snapshot_size,
                                (texture_current, texture_current_geo),
                                window_size,
                                resize.anim.value() as f32,
                                resize.anim.clamped_value().clamp(0., 1.) as f32,
                                radius,
                                clip_to_geometry,
                                win_alpha,
                            );

                            // We're drawing the resize shader, not the offscreen directly.
                            data.id = elem.id().clone();

                            // This is not a problem for split popups as the code will look for them by
                            // original id when it doesn't find them on the offscreen.
                            self.window.set_offscreen_data(Some(data));
                            push(elem.into());
                            pushed_resize = true;
                        }
                    }
                }
            }
            if let Some(renderer) = ctx.renderer.as_native_renderer() {
                let result = (|| -> anyhow::Result<_> {
                    let mut native_ctx = RenderCtx {
                        renderer,
                        target: ctx.target,
                        xray: ctx.xray,
                    };
                    let (texture_from, snapshot_size) = resize
                        .snapshot
                        .native_texture(native_ctx.renderer, ctx.target, scale)
                        .ok_or_else(|| anyhow::anyhow!("missing native resize snapshot"))?;
                    let mut elements = Vec::new();
                    self.window.render_normal(
                        native_ctx.r(),
                        Point::from((0., 0.)),
                        scale,
                        1.,
                        &mut |elem| elements.push(elem),
                    );
                    // The persistent target keeps its allocation across the whole
                    // resize; the damage tracker redraws only what the client
                    // actually changed, and the sampling barriers order each frame
                    // against the previous one.
                    let (texture, _sync, geo, states, _commit) =
                        resize
                            .native_offscreen
                            .render(native_ctx.renderer, scale, &elements)?;
                    let clip = clip_to_geometry
                        || (resize.snapshot.blocked_out(ctx.target)
                            && ctx.target.should_block_out(rules.block_out_from));
                    let elem = ResizeRenderElement::new_native(
                        native_ctx.renderer,
                        self.options
                            .animations
                            .window_resize
                            .custom_shader
                            .as_deref(),
                        area,
                        scale,
                        texture_from,
                        snapshot_size,
                        (texture, geo),
                        window_size,
                        resize.anim.value() as f32,
                        resize.anim.clamped_value().clamp(0., 1.) as f32,
                        radius,
                        clip,
                        win_alpha,
                    )?;
                    Ok((elem, states))
                })();
                match result {
                    Ok((elem, states)) => {
                        self.window.set_offscreen_data(Some(OffscreenData {
                            id: elem.id().clone(),
                            states,
                        }));
                        push(elem.into());
                        pushed_resize = true;
                    }
                    Err(err) => warn!("error rendering native resize: {err:#}"),
                }
            }
        }

        // If we're not resizing, render the window itself.
        let has_border_shader = BorderRenderElement::has_shader(ctx.renderer);
        if !pushed_resize {
            let geo = Rectangle::new(window_render_loc, window_size);
            let radius = radius.fit_to(window_size.w as f32, window_size.h as f32);

            let clip_shader = ClippedSurfaceRenderElement::clip_program(ctx.renderer);
            let rounding_power = self.rounding_power();
            let clip = |elem| match elem {
                LayoutElementRenderElement::Wayland(elem) => {
                    // If we should clip to geometry, render a clipped window.
                    if clip_to_geometry {
                        if let Some(shader) = clip_shader.clone() {
                            if ClippedSurfaceRenderElement::will_clip(&elem, scale, geo, radius) {
                                return ClippedSurfaceRenderElement::new(
                                    elem,
                                    scale,
                                    geo,
                                    shader.clone(),
                                    radius,
                                )
                                .with_rounding_power(rounding_power)
                                .into();
                            }
                        }
                    }

                    // Otherwise, render it normally.
                    LayoutElementRenderElement::Wayland(elem).into()
                }
                LayoutElementRenderElement::SolidColor(elem) => {
                    // In this branch we're rendering a blocked-out window with a solid
                    // color. We need to render it with a rounded corner shader even if
                    // clip_to_geometry is false, because in this case we're assuming that
                    // the unclipped window CSD already has corners rounded to the
                    // user-provided radius, so our blocked-out rendering should match that
                    // radius.
                    if radius != CornerRadius::default() && has_border_shader {
                        return BorderRenderElement::new(
                            geo.size,
                            Rectangle::from_size(geo.size),
                            GradientInterpolation::default(),
                            Color::from_color32f(elem.color()),
                            Color::from_color32f(elem.color()),
                            0.,
                            Rectangle::from_size(geo.size),
                            0.,
                            radius,
                            scale.x as f32,
                            1.,
                        )
                        .with_rounding_power(rounding_power)
                        .with_location(geo.loc)
                        .into();
                    }

                    // Otherwise, render the solid color as is.
                    LayoutElementRenderElement::SolidColor(elem).into()
                }
                elem @ LayoutElementRenderElement::BackgroundEffect(_) => {
                    // This is only used on popups for now. If subsurface blur is implemented, this
                    // will need to be handled somehow.
                    error!("background effect clipping is unimplemented");
                    elem.into()
                }
            };

            if clip_to_geometry && clip_shader.is_some() {
                let damage = self.rounded_corner_damage.render(geo);
                push(damage.into());
            }

            self.window
                .render_normal(ctx.r(), window_render_loc, scale, win_alpha, &mut |elem| {
                    push(clip(elem))
                });

            // Dim inactive windows if enabled.
            let dim_enabled = self.options.layout.dim_inactive;
            let rule_dim = self.window.rules().dim_inactive;
            let should_dim =
                rule_dim.unwrap_or(dim_enabled) && !focus_ring && fullscreen_progress < 1.0;
            if should_dim {
                let factor = self
                    .window
                    .rules()
                    .dim_factor
                    .unwrap_or(self.options.layout.dim_factor)
                    .clamp(0.0, 1.0);
                let dim_alpha = (1.0 - factor) * (1.0 - fullscreen_progress as f32);
                if dim_alpha > 0.001 {
                    if has_border_shader {
                        let dim_elem = BorderRenderElement::new(
                            geo.size,
                            Rectangle::from_size(geo.size),
                            GradientInterpolation::default(),
                            Color::from_rgba8_unpremul(0, 0, 0, 255),
                            Color::from_rgba8_unpremul(0, 0, 0, 255),
                            0.,
                            Rectangle::from_size(geo.size),
                            0.,
                            radius,
                            scale.x as f32,
                            dim_alpha,
                        )
                        .with_rounding_power(self.rounding_power())
                        .with_location(geo.loc);
                        push(dim_elem.into());
                    } else {
                        let black_buffer = SolidColorBuffer::new(geo.size, [0., 0., 0., 1.]);
                        let dim_elem = SolidColorRenderElement::from_buffer(
                            &black_buffer,
                            geo.loc,
                            dim_alpha,
                            Kind::Unspecified,
                        );
                        push(dim_elem.into());
                    }
                }
            }
        }

        if fullscreen_progress > 0. {
            let alpha = fullscreen_progress as f32;

            // During the un/fullscreen animation, render a border element in order to use the
            // animated corner radius.
            if fullscreen_progress < 1. && has_border_shader {
                let border_width = self.visual_border_width().unwrap_or(0.);
                let radius = self
                    .resolved_corner_radius()
                    .expanded_by(border_width as f32)
                    .scaled_by(1. - expanded_progress as f32);

                let size = self.fullscreen_backdrop.size();
                let color = self.fullscreen_backdrop.color();
                let elem = BorderRenderElement::new(
                    size,
                    Rectangle::from_size(size),
                    GradientInterpolation::default(),
                    Color::from_color32f(color),
                    Color::from_color32f(color),
                    0.,
                    Rectangle::from_size(size),
                    0.,
                    radius,
                    scale.x as f32,
                    alpha,
                )
                .with_rounding_power(self.rounding_power())
                .with_location(location);
                push(elem.into());
            } else {
                let elem = SolidColorRenderElement::from_buffer(
                    &self.fullscreen_backdrop,
                    location,
                    alpha,
                    Kind::Unspecified,
                );
                push(elem.into());
            }
        }

        if let Some(width) = self.visual_border_width() {
            self.border.render(
                ctx.renderer,
                location + Point::from((width, width)),
                &mut |elem| push(elem.into()),
            );
        }

        // Hide the focus ring when maximized/fullscreened. It's not normally visible anyway due to
        // being outside the monitor or obscured by a solid colored bar, but it is visible under
        // semitransparent bars in maximized state (which is a bit weird) and in the overview (also
        // a bit weird).
        if focus_ring && expanded_progress < 1. {
            self.focus_ring
                .render(ctx.renderer, location, &mut |elem| push(elem.into()));
        }

        if expanded_progress < 1. {
            self.shadow
                .render(ctx.renderer, location, &mut |elem| push(elem.into()));
        }

        let surface_anim_scale = animated_window_size / window_size;
        self.window.render_background_effect(
            ctx.r(),
            area,
            self.scale,
            clip_to_geometry,
            surface_anim_scale,
            radius,
            xray_pos,
            &mut |elem| push(elem.into()),
        );
    }

    pub fn render<R: ClarionRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        location: Point<f64, Logical>,
        xray_pos: XrayPos,
        focus_ring: bool,
        push: &mut dyn FnMut(TileRenderElement<R>),
    ) {
        if self.render_genie(ctx.r(), xray_pos, push) {
            return;
        }
        let wobble_active = self.wobble.borrow().is_active()
            && !self.options.animations.off
            && !self.options.animations.window_wobble.off;

        if wobble_active {
            if let Some(mut ctx) = ctx.as_gles() {
                if MeshRenderElement::is_available(ctx.renderer) {
                    let scale = Scale::from(self.scale);
                    let mut elements = Vec::new();
                    self.render_without_wobble(
                        ctx.r(),
                        Point::from((0., 0.)),
                        xray_pos,
                        focus_ring,
                        &mut |element| elements.push(element),
                    );

                    match self.wobble_offscreen.render(ctx.renderer, scale, &elements) {
                        Ok((element, _sync, mut data)) => {
                            let offset = element.offset();
                            let source_area =
                                Rectangle::new(location + offset, element.logical_size());
                            let source_commit = element.current_commit();
                            let source_texture = u64::from(element.texture().tex_id());
                            let mesh = self.wobble_forecast.borrow_mut().sample(
                                &self.wobble.borrow(),
                                source_area,
                                self.options.animations.window_wobble,
                                self.clock.now(),
                                source_commit,
                                source_texture,
                            );
                            if let Some(mesh) = mesh {
                                let element = MeshRenderElement::new(
                                    mesh.id,
                                    mesh.commit,
                                    element.texture().clone(),
                                    element.src().size,
                                    mesh.area,
                                    mesh.vertices,
                                    1.,
                                    Kind::Unspecified,
                                );
                                data.id = element.id().clone();
                                self.window().set_offscreen_data(Some(data));
                                push(element.into());
                                return;
                            }
                            self.clear_wobble_render_cache();
                        }
                        Err(err) => {
                            warn!("error rendering elastic window to texture: {err:?}");
                            self.wobble.borrow_mut().cancel();
                            self.clear_wobble_render_cache();
                        }
                    }
                } else {
                    self.wobble.borrow_mut().cancel();
                    self.clear_wobble_render_cache();
                }
            } else if let Some(mut ctx) = ctx.as_native() {
                let scale = Scale::from(self.scale);
                let mut elements = Vec::new();
                self.render_without_wobble(
                    ctx.r(),
                    Point::from((0., 0.)),
                    xray_pos,
                    focus_ring,
                    &mut |element| elements.push(element),
                );
                match self
                    .wobble_native_offscreen
                    .render(ctx.renderer, scale, &elements)
                {
                    Ok((texture, _sync, geo, states, source_commit)) => {
                        // Native submissions share a queue; sampling barriers order this
                        // snapshot without stalling the compositor on a CPU fence wait.
                        // The persistent tracker redraws only what changed since the
                        // previous wobble frame.
                        let offset = geo.loc.to_f64().to_logical(scale);
                        let source_area =
                            Rectangle::new(location + offset, geo.size.to_f64().to_logical(scale));
                        // A pooled image keeps its identity when pixels change; the
                        // damage-bag commit marks real content updates instead.
                        let source_texture = texture.cache_key();
                        let mesh = self.wobble_forecast.borrow_mut().sample(
                            &self.wobble.borrow(),
                            source_area,
                            self.options.animations.window_wobble,
                            self.clock.now(),
                            source_commit,
                            source_texture,
                        );
                        if let Some(mesh) = mesh {
                            let used = smithay::utils::Size::from((
                                f64::from(geo.size.w),
                                f64::from(geo.size.h),
                            ));
                            let element = MeshRenderElement::new_vulkan(
                                mesh.id,
                                mesh.commit,
                                texture,
                                used,
                                mesh.area,
                                mesh.vertices,
                                1.,
                                Kind::Unspecified,
                            );
                            let data = crate::render_helpers::offscreen::OffscreenData {
                                id: element.id().clone(),
                                states,
                            };
                            self.window().set_offscreen_data(Some(data));
                            push(element.into());
                            return;
                        }
                        self.clear_wobble_render_cache();
                    }
                    Err(err) => {
                        warn!("error rendering Vulkan elastic window to texture: {err:?}");
                        self.wobble.borrow_mut().cancel();
                        self.clear_wobble_render_cache();
                    }
                }
            } else {
                self.wobble.borrow_mut().cancel();
                self.clear_wobble_render_cache();
            }
        }

        self.render_without_wobble(ctx, location, xray_pos, focus_ring, push);
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
            || self.options.animations.off
            || self.clock.should_complete_instantly()
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
        self.wobble.borrow_mut().cancel();
        self.clear_wobble_render_cache();
        self.open_animation = None;
        // Freeze incidental movement before capturing the original tile.
        self.move_x_animation = None;
        self.move_y_animation = None;
        self.alpha_animation = None;
        let mut genie = self.genie.borrow_mut();
        if let Some(genie) = genie.as_mut() {
            genie.reverse(self.clock.clone(), minimizing);
        } else {
            *genie = Some(super::minimized::Genie::with_style(
                self.clock.clone(),
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
        self.genie.borrow().is_some()
    }

    pub fn complete_genie(&self) {
        self.genie.borrow_mut().take();
        for buffer in &self.genie_offscreen {
            buffer.clear();
        }
        for buffer in &self.genie_native_offscreen {
            buffer.clear();
        }
        self.window.set_offscreen_data(None);
    }

    /// A parked tile only uses this presentation path; failure therefore cannot
    /// accidentally reveal a logically minimized window.
    pub fn render_genie<R: ClarionRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        xray_pos: XrayPos,
        push: &mut dyn FnMut(TileRenderElement<R>),
    ) -> bool {
        if self.genie.borrow().is_none() {
            return false;
        }
        let idx = ctx.target as usize;
        let scale = Scale::from(self.scale);
        if let Some(mut ctx) = ctx.as_gles() {
            if !MeshRenderElement::is_available(ctx.renderer) {
                self.complete_genie();
                return false;
            }
            let mut elements = Vec::new();
            self.render_without_wobble(ctx.r(), (0., 0.).into(), xray_pos, false, &mut |element| {
                elements.push(element)
            });
            match self.genie_offscreen[idx].render(ctx.renderer, scale, &elements) {
                Ok((element, _sync, mut data)) => {
                    let texture_area = Rectangle::new(element.offset(), element.logical_size());
                    let mesh = self.genie.borrow_mut().as_mut().unwrap().mesh(
                        texture_area,
                        self.animated_tile_size(),
                        element.current_commit(),
                        u64::from(element.texture().tex_id()),
                    );
                    let element = MeshRenderElement::new(
                        mesh.id,
                        mesh.commit,
                        element.texture().clone(),
                        element.src().size,
                        mesh.area,
                        mesh.vertices,
                        mesh.alpha,
                        Kind::Unspecified,
                    );
                    data.id = element.id().clone();
                    self.window.set_offscreen_data(Some(data));
                    push(element.into());
                    return true;
                }
                Err(err) => warn!("error rendering Genie window to texture: {err:?}"),
            }
        } else if let Some(mut ctx) = ctx.as_native() {
            let mut elements = Vec::new();
            self.render_without_wobble(ctx.r(), (0., 0.).into(), xray_pos, false, &mut |element| {
                elements.push(element)
            });
            match self.genie_native_offscreen[idx].render(ctx.renderer, scale, &elements) {
                Ok((texture, _sync, geo, states, source_commit)) => {
                    let texture_area = geo.to_f64().to_logical(scale);
                    let mesh = self.genie.borrow_mut().as_mut().unwrap().mesh(
                        texture_area,
                        self.animated_tile_size(),
                        source_commit,
                        texture.cache_key(),
                    );
                    let used: smithay::utils::Size<f64, smithay::utils::Buffer> =
                        (f64::from(geo.size.w), f64::from(geo.size.h)).into();
                    let element = MeshRenderElement::new_vulkan(
                        mesh.id,
                        mesh.commit,
                        texture,
                        used,
                        mesh.area,
                        mesh.vertices,
                        mesh.alpha,
                        Kind::Unspecified,
                    );
                    self.window.set_offscreen_data(Some(OffscreenData {
                        id: element.id().clone(),
                        states,
                    }));
                    push(element.into());
                    return true;
                }
                Err(err) => warn!("error rendering Vulkan Genie window to texture: {err:?}"),
            }
        }
        self.complete_genie();
        false
    }

    fn clear_wobble_render_cache(&self) {
        self.wobble_forecast.borrow_mut().clear();
        self.wobble_offscreen.clear();
        self.wobble_native_offscreen.clear();
        self.window().set_offscreen_data(None);
    }

    fn render_without_wobble<R: ClarionRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        location: Point<f64, Logical>,
        xray_pos: XrayPos,
        focus_ring: bool,
        push: &mut dyn FnMut(TileRenderElement<R>),
    ) {
        let _span = clarion_tracy_span!("Tile::render");

        let scale = Scale::from(self.scale);

        let tile_alpha = self
            .alpha_animation
            .as_ref()
            .map_or(1., AlphaAnimation::value) as f32;

        let mut pushed = false;
        self.window().set_offscreen_data(None);

        if let Some(open) = &self.open_animation {
            if let Some(mut ctx) = ctx.as_gles() {
                let mut elements = Vec::new();
                self.render_inner(
                    ctx.r(),
                    Point::new(0., 0.),
                    xray_pos,
                    focus_ring,
                    &mut |elem| elements.push(elem),
                );
                match open.render(
                    ctx.renderer,
                    &elements,
                    self.animated_tile_size(),
                    location,
                    scale,
                    tile_alpha,
                ) {
                    Ok((elem, data)) => {
                        self.window().set_offscreen_data(Some(data));
                        push(elem.into());
                        pushed = true;
                    }
                    Err(err) => {
                        warn!("error rendering window opening animation: {err:?}");
                    }
                }
            } else if let Some(mut ctx) = ctx.as_native() {
                let mut elements = Vec::new();
                self.render_inner(
                    ctx.r(),
                    Point::new(0., 0.),
                    xray_pos,
                    focus_ring,
                    &mut |elem| elements.push(elem),
                );
                match open.render_native(
                    ctx.renderer,
                    self.options.animations.window_open.custom_shader.as_deref(),
                    &elements,
                    self.animated_tile_size(),
                    location,
                    scale,
                    tile_alpha,
                ) {
                    Ok((elem, data)) => {
                        self.window().set_offscreen_data(Some(data));
                        push(elem.into());
                        pushed = true;
                    }
                    Err(err) => {
                        warn!("error rendering Vulkan window opening animation: {err:?}");
                    }
                }
            } else {
                let current_scale = open.scale();
                let tile_size = self.animated_tile_size();
                let center: Point<f64, Logical> = Point::new(
                    location.x + tile_size.w * 0.5,
                    location.y + tile_size.h * 0.5,
                );
                let scaled_loc = Point::new(
                    center.x - tile_size.w * 0.5 * current_scale,
                    center.y - tile_size.h * 0.5 * current_scale,
                );
                self.render_inner(ctx.r(), scaled_loc, xray_pos, focus_ring, &mut |elem| {
                    push(elem)
                });
                pushed = true;
            }
        } else if let Some(alpha) = &self.alpha_animation {
            if let Some(mut ctx) = ctx.as_gles() {
                let mut elements = Vec::new();
                self.render_inner(
                    ctx.r(),
                    Point::new(0., 0.),
                    xray_pos,
                    focus_ring,
                    &mut |elem| elements.push(elem),
                );
                match alpha.offscreen.render(ctx.renderer, scale, &elements) {
                    Ok((elem, _sync, data)) => {
                        let offset = elem.offset();
                        let elem = elem.with_alpha(tile_alpha).with_offset(location + offset);

                        self.window().set_offscreen_data(Some(data));
                        push(elem.into());
                        pushed = true;
                    }
                    Err(err) => {
                        warn!("error rendering tile to offscreen for alpha animation: {err:?}");
                    }
                }
            } else if let Some(mut ctx) = ctx.as_native() {
                let mut elements = Vec::new();
                self.render_inner(
                    ctx.r(),
                    Point::new(0., 0.),
                    xray_pos,
                    focus_ring,
                    &mut |elem| elements.push(elem),
                );
                match alpha
                    .native_offscreen
                    .render(ctx.renderer, scale, &elements)
                {
                    Ok((texture, _sync, geo, states, _commit)) => {
                        let offset = geo.loc.to_f64().to_logical(scale);
                        let used = Size::from((
                            f64::from(geo.size.w) / scale.x,
                            f64::from(geo.size.h) / scale.y,
                        ));
                        let buf = crate::render_helpers::texture::TextureBuffer::from_texture(
                            ctx.renderer,
                            texture,
                            scale,
                            smithay::utils::Transform::Normal,
                            Vec::new(),
                        );
                        // The pooled allocation may be larger than this frame's
                        // encompassing geometry; only the [0, geo.size) prefix is
                        // this window.
                        let elem =
                            crate::render_helpers::texture::TextureRenderElement::from_texture_buffer(
                                buf,
                                location + offset,
                                tile_alpha,
                                Some(Rectangle::new(Point::from((0., 0.)), used)),
                                Some(used),
                                Kind::Unspecified,
                            );
                        self.window().set_offscreen_data(Some(OffscreenData {
                            id: elem.id().clone(),
                            states,
                        }));
                        push(
                            crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement::from(
                                elem,
                            )
                            .into(),
                        );
                        pushed = true;
                    }
                    Err(err) => {
                        warn!(
                            "error rendering Vulkan tile to offscreen for alpha animation: {err:?}"
                        );
                    }
                }
            } else {
                self.render_inner(ctx.r(), location, xray_pos, focus_ring, &mut |elem| {
                    push(elem)
                });
                pushed = true;
            }
        }

        if !pushed {
            self.render_inner(ctx, location, xray_pos, focus_ring, &mut |elem| push(elem));
        }
    }

    pub fn snapshot_native(
        &mut self,
        renderer: &mut crate::render_helpers::vulkan::native::NativeRenderer,
    ) -> anyhow::Result<NativeTileSnapshot> {
        if let Some(snapshot) = self.native_unmap_snapshot.take() {
            return Ok(snapshot);
        }
        self.render_snapshot_native(renderer, None, XrayPos::default())
    }

    fn render_snapshot_native(
        &self,
        renderer: &mut crate::render_helpers::vulkan::native::NativeRenderer,
        xray: Option<&Xray>,
        xray_pos: XrayPos,
    ) -> anyhow::Result<NativeTileSnapshot> {
        let scale = Scale::from(self.scale);
        let mut targets = Vec::with_capacity(3);
        for target in [
            RenderTarget::Output,
            RenderTarget::Screencast,
            RenderTarget::ScreenCapture,
        ] {
            let mut elements = Vec::new();
            self.render(
                RenderCtx {
                    renderer,
                    target,
                    xray,
                },
                Point::from((0., 0.)),
                xray_pos,
                false,
                &mut |elem| elements.push(elem),
            );
            let (texture, _sync, geo) = crate::render_helpers::render_to_encompassing_texture(
                renderer,
                scale,
                smithay::utils::Transform::Normal,
                smithay::backend::allocator::Fourcc::Abgr8888,
                &elements,
            )?;
            targets.push((texture, geo.loc.to_f64().to_logical(scale)));
        }
        Ok(NativeTileSnapshot {
            targets: targets.try_into().unwrap(),
        })
    }

    pub fn store_unmap_snapshot_native_if_empty(
        &mut self,
        renderer: &mut crate::render_helpers::vulkan::native::NativeRenderer,
        xray: Option<&mut Xray>,
        _xray_has_blocked_out_layers: bool,
        xray_pos: XrayPos,
    ) -> bool {
        if self.native_unmap_snapshot.is_none() {
            match self.render_snapshot_native(renderer, xray.as_deref(), xray_pos) {
                Ok(snapshot) => self.native_unmap_snapshot = Some(snapshot),
                Err(err) => warn!("error capturing native unmap snapshot: {err:?}"),
            }
        }
        self.native_unmap_snapshot.is_some()
    }

    pub fn store_unmap_snapshot_if_empty(
        &mut self,
        renderer: &mut GlesRenderer,
        xray: Option<&mut Xray>,
        xray_has_blocked_out_layers: bool,
        xray_pos: XrayPos,
    ) {
        if self.unmap_snapshot.is_some() {
            return;
        }

        self.unmap_snapshot =
            Some(self.render_snapshot(renderer, xray, xray_has_blocked_out_layers, xray_pos));
    }

    fn render_snapshot(
        &self,
        renderer: &mut GlesRenderer,
        mut xray: Option<&mut Xray>,
        xray_has_blocked_out_layers: bool,
        xray_pos: XrayPos,
    ) -> TileRenderSnapshot {
        let _span = clarion_tracy_span!("Tile::render_snapshot");

        let mut contents = Vec::new();
        self.render(
            RenderCtx {
                target: RenderTarget::Output,
                renderer,
                xray: xray.as_deref(),
            },
            Point::from((0., 0.)),
            xray_pos,
            false,
            &mut |elem| contents.push(elem),
        );

        let mut contents_with_blocked_out_bg = None;

        // Do a bit of pointer surgery on Xray.
        //
        // The idea is to avoid the combinatorial combination of rendering snapshots for target
        // (Output, Screencast) × Xray target (Output, Screencast, ScreenCapture).
        //
        // Our main goals:
        // - Everything must look unblocked for RenderTarget::Output.
        // - If anything is potentially blocked-out, it must not show up on any screen capture.
        //
        // Right above we rendered a fully-unblocked snapshot for the Output, so that's covered.
        //
        // Next, *only if Xray has any blocked-out surfaces* (which is a rare case), we will render
        // a snapshot where the window itself is unblocked, but the Xray background is blocked. To
        // do this, we swap the Output target buffers in Xray with the Screencast target buffers
        // (which were prepared for us higher up the stack).
        //
        // Finally, we render a fully blocked-out snapshot. If Xray has blocked-out surfaces, then
        // Xray's Screencast buffers are already filled-in, but if not, then we swap in the Output
        // buffers, to avoid an extra render. This is safe since we know there are no blocked
        // surfaces there.
        let output_idx = RenderTarget::Output as usize;
        let screencast_idx = RenderTarget::Screencast as usize;
        let mut screencast_background = None;
        let mut screencast_backdrop = None;
        let mut output_background = None;
        let mut output_backdrop = None;
        if let Some(xray) = &mut xray {
            screencast_background = Some(Rc::clone(&xray.background[screencast_idx]));
            screencast_backdrop = Some(Rc::clone(&xray.backdrop[screencast_idx]));
            output_background = Some(Rc::clone(&xray.background[output_idx]));
            output_backdrop = Some(Rc::clone(&xray.backdrop[output_idx]));

            if xray_has_blocked_out_layers {
                xray.background[output_idx] = screencast_background.clone().unwrap();
                xray.backdrop[output_idx] = screencast_backdrop.clone().unwrap();

                let mut contents = Vec::new();
                self.render(
                    RenderCtx {
                        target: RenderTarget::Output,
                        renderer,
                        xray: Some(xray),
                    },
                    Point::from((0., 0.)),
                    xray_pos,
                    false,
                    &mut |elem| contents.push(elem),
                );
                contents_with_blocked_out_bg = Some(contents);
            } else {
                xray.background[screencast_idx] = output_background.clone().unwrap();
                xray.backdrop[screencast_idx] = output_backdrop.clone().unwrap();
            }
        }

        // A bit of a hack to render blocked out as for screencast, but I think it's fine here.
        let mut blocked_out_contents = Vec::new();
        self.render(
            RenderCtx {
                target: RenderTarget::Screencast,
                renderer,
                xray: xray.as_deref(),
            },
            Point::from((0., 0.)),
            xray_pos,
            false,
            &mut |elem| blocked_out_contents.push(elem),
        );

        // Put everything back to normal.
        if let Some(xray) = &mut xray {
            if xray_has_blocked_out_layers {
                xray.background[output_idx] = output_background.take().unwrap();
                xray.backdrop[output_idx] = output_backdrop.take().unwrap();
            } else {
                xray.background[screencast_idx] = screencast_background.take().unwrap();
                xray.backdrop[screencast_idx] = screencast_backdrop.take().unwrap();
            }
        }

        RenderSnapshot {
            contents,
            contents_with_blocked_out_bg,
            blocked_out_contents,
            block_out_from: self.window.rules().block_out_from,
            size: self.animated_tile_size(),
            texture: Default::default(),
            texture_with_blocked_out_bg: Default::default(),
            blocked_out_texture: Default::default(),
        }
    }

    pub fn take_unmap_snapshot(&mut self) -> Option<TileRenderSnapshot> {
        self.native_unmap_snapshot = None;
        self.unmap_snapshot.take()
    }

    pub fn border(&self) -> &FocusRing {
        &self.border
    }

    pub fn focus_ring(&self) -> &FocusRing {
        &self.focus_ring
    }

    pub fn options(&self) -> &Rc<Options> {
        &self.options
    }

    pub fn view_size(&self) -> Size<f64, Logical> {
        self.view_size
    }

    #[cfg(test)]
    pub fn verify_invariants(&self) {
        use approx::assert_abs_diff_eq;

        assert_eq!(self.sizing_mode, self.window.sizing_mode());

        let scale = self.scale;
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
                offscreen: OffscreenBuffer::default(),
                native_offscreen: Default::default(),
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
            snapshot: ResizeSnapshot::Gles(RenderSnapshot {
                contents: Vec::new(),
                contents_with_blocked_out_bg: None,
                blocked_out_contents: Vec::new(),
                block_out_from: None,
                size: Size::from((500., 800.)),
                texture: Default::default(),
                texture_with_blocked_out_bg: Default::default(),
                blocked_out_texture: Default::default(),
            }),
            offscreen: OffscreenBuffer::default(),
            native_offscreen: Default::default(),
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
