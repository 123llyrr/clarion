use knuffel::errors::DecodeError;
use knuffel::Decode as _;

use crate::utils::{expect_only_children, parse_arg_node, MergeWith};
use crate::{Color, FloatOrInt};

#[derive(Debug, Clone, PartialEq)]
pub struct Animations {
    pub off: bool,
    pub slowdown: f64,
    pub workspace_switch: WorkspaceSwitchAnim,
    pub window_open: WindowOpenAnim,
    pub window_close: WindowCloseAnim,
    pub horizontal_view_movement: HorizontalViewMovementAnim,
    pub window_movement: WindowMovementAnim,
    pub window_resize: WindowResizeAnim,
    pub config_notification_open_close: ConfigNotificationOpenCloseAnim,
    pub exit_confirmation_open_close: ExitConfirmationOpenCloseAnim,
    pub screenshot_ui_open: ScreenshotUiOpenAnim,
    pub overview_open_close: OverviewOpenCloseAnim,
    pub recent_windows_close: RecentWindowsCloseAnim,
    pub window_wobble: WindowWobble,
}

impl Default for Animations {
    fn default() -> Self {
        Self {
            off: false,
            slowdown: 1.,
            workspace_switch: Default::default(),
            horizontal_view_movement: Default::default(),
            window_movement: Default::default(),
            window_open: Default::default(),
            window_close: Default::default(),
            window_resize: Default::default(),
            config_notification_open_close: Default::default(),
            exit_confirmation_open_close: Default::default(),
            screenshot_ui_open: Default::default(),
            overview_open_close: Default::default(),
            recent_windows_close: Default::default(),
            window_wobble: Default::default(),
        }
    }
}

#[derive(knuffel::Decode, Debug, Clone, PartialEq)]
pub struct AnimationsPart {
    #[knuffel(child)]
    pub off: bool,
    #[knuffel(child)]
    pub on: bool,
    #[knuffel(child, unwrap(argument))]
    pub slowdown: Option<FloatOrInt<0, { i32::MAX }>>,
    #[knuffel(child)]
    pub workspace_switch: Option<WorkspaceSwitchAnim>,
    #[knuffel(child)]
    pub window_open: Option<WindowOpenAnim>,
    #[knuffel(child)]
    pub window_close: Option<WindowCloseAnim>,
    #[knuffel(child)]
    pub horizontal_view_movement: Option<HorizontalViewMovementAnim>,
    #[knuffel(child)]
    pub window_movement: Option<WindowMovementAnim>,
    #[knuffel(child)]
    pub window_resize: Option<WindowResizeAnim>,
    #[knuffel(child)]
    pub config_notification_open_close: Option<ConfigNotificationOpenCloseAnim>,
    #[knuffel(child)]
    pub exit_confirmation_open_close: Option<ExitConfirmationOpenCloseAnim>,
    #[knuffel(child)]
    pub screenshot_ui_open: Option<ScreenshotUiOpenAnim>,
    #[knuffel(child)]
    pub overview_open_close: Option<OverviewOpenCloseAnim>,
    #[knuffel(child)]
    pub recent_windows_close: Option<RecentWindowsCloseAnim>,
    #[knuffel(child)]
    pub window_wobble: Option<WindowWobble>,
}

impl MergeWith<AnimationsPart> for Animations {
    fn merge_with(&mut self, part: &AnimationsPart) {
        self.off |= part.off;
        if part.on {
            self.off = false;
        }

        merge!((self, part), slowdown);

        // Animation properties are fairly tied together, except maybe `off`. So let's just save
        // ourselves the work and not merge within individual animations.
        merge_clone!(
            (self, part),
            workspace_switch,
            window_open,
            window_close,
            horizontal_view_movement,
            window_movement,
            window_resize,
            config_notification_open_close,
            exit_confirmation_open_close,
            screenshot_ui_open,
            overview_open_close,
            recent_windows_close,
            window_wobble,
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Animation {
    pub off: bool,
    pub kind: Kind,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Easing(EasingParams),
    Spring(SpringParams),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EasingParams {
    pub duration_ms: u32,
    pub curve: Curve,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Curve {
    Linear,
    EaseOutQuad,
    EaseOutCubic,
    EaseOutExpo,
    CubicBezier(f64, f64, f64, f64),
}

impl Curve {
    /// Native macOS Core Animation default curve: kCAMediaTimingFunctionDefault (0.25, 0.1, 0.25,
    /// 1.0) Raw IEEE 754 float32 at QuartzCore `0x236f10`: [0.25, 0.10000000149011612, 0.25,
    /// 1.0]
    pub const MACOS_DEFAULT: Self = Self::CubicBezier(0.25, 0.1, 0.25, 1.0);
    pub const CA_DEFAULT: Self = Self::MACOS_DEFAULT;
    /// Native macOS Core Animation linear curve: kCAMediaTimingFunctionLinear (0.0, 0.0, 1.0, 1.0)
    /// Raw IEEE 754 float32 at QuartzCore `0x236f20`: [0.0, 0.0, 1.0, 1.0]
    pub const MACOS_LINEAR: Self = Self::Linear;
    pub const CA_LINEAR: Self = Self::Linear;
    /// Native macOS Core Animation ease-in curve: kCAMediaTimingFunctionEaseIn (0.42, 0.0, 1.0,
    /// 1.0) Raw IEEE 754 float32 at QuartzCore `0x236f30`: [0.41999998688697815, 0.0, 1.0, 1.0]
    pub const MACOS_EASE_IN: Self = Self::CubicBezier(0.42, 0.0, 1.0, 1.0);
    pub const CA_EASE_IN: Self = Self::MACOS_EASE_IN;
    /// Native macOS Core Animation ease-out curve: kCAMediaTimingFunctionEaseOut (0.0, 0.0, 0.58,
    /// 1.0) Raw IEEE 754 float32 at QuartzCore `0x236f40`: [0.0, 0.0, 0.5799999833106995, 1.0]
    pub const MACOS_EASE_OUT: Self = Self::CubicBezier(0.0, 0.0, 0.58, 1.0);
    pub const CA_EASE_OUT: Self = Self::MACOS_EASE_OUT;
    /// Native macOS Core Animation ease-in-ease-out curve: kCAMediaTimingFunctionEaseInEaseOut
    /// (0.42, 0.0, 0.58, 1.0) Raw IEEE 754 float32 at QuartzCore `0x236f50`:
    /// [0.41999998688697815, 0.0, 0.5799999833106995, 1.0]
    pub const MACOS_EASE_IN_EASE_OUT: Self = Self::CubicBezier(0.42, 0.0, 0.58, 1.0);
    pub const CA_EASE_IN_EASE_OUT: Self = Self::MACOS_EASE_IN_EASE_OUT;
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpringParams {
    pub damping_ratio: f64,
    pub stiffness: u32,
    pub epsilon: f64,
}

impl SpringParams {
    /// Reference spring from CASpringAnimation defaults in QuartzCore
    /// (mass 1, stiffness 100, damping 10 -> damping ratio 0.5, settlingDuration epsilon 0.001).
    pub const MACOS_DEFAULT: Self = Self {
        damping_ratio: 0.5,
        stiffness: 100,
        epsilon: 0.001,
    };
    pub const CA_DEFAULT: Self = Self::MACOS_DEFAULT;
    /// Clarion interactive tuning from design: slight underdamping with a short tail for direct
    /// manipulation.
    pub const MACOS_INTERACTIVE: Self = Self {
        damping_ratio: 0.92,
        stiffness: 300,
        epsilon: 0.001,
    };
    pub const INTERACTIVE: Self = Self::MACOS_INTERACTIVE;
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorkspaceSwitchAnim(pub Animation, pub WorkspaceCube);

impl Default for WorkspaceSwitchAnim {
    fn default() -> Self {
        Self(
            Animation {
                off: false,
                kind: Kind::Spring(SpringParams {
                    damping_ratio: 1.,
                    stiffness: 1000,
                    epsilon: 0.0001,
                }),
            },
            WorkspaceCube::default(),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorkspaceCube {
    pub off: bool,
    pub perspective: f64,
    pub background_color: Color,
}

impl Default for WorkspaceCube {
    fn default() -> Self {
        Self {
            off: false,
            perspective: 0.65,
            background_color: Color::from_rgba8_unpremul(16, 16, 24, 255),
        }
    }
}

#[derive(knuffel::Decode, Debug, Default, Clone, Copy, PartialEq)]
struct WorkspaceCubePart {
    #[knuffel(child)]
    off: bool,
    #[knuffel(child)]
    on: bool,
    #[knuffel(child, unwrap(argument))]
    perspective: Option<FloatOrInt<0, 1>>,
    #[knuffel(child)]
    background_color: Option<Color>,
}

impl WorkspaceCube {
    fn merge(&mut self, part: WorkspaceCubePart) {
        self.off |= part.off;
        if part.on {
            self.off = false;
        }
        if let Some(value) = part.perspective {
            self.perspective = value.0;
        }
        if let Some(value) = part.background_color {
            self.background_color = value;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glide {
    pub off: bool,
    pub tilt_deg: f64,
    pub depth: f64,
}

impl Default for Glide {
    fn default() -> Self {
        Self {
            off: false,
            tilt_deg: 12.,
            depth: 0.08,
        }
    }
}

#[derive(knuffel::Decode, Debug, Default, Clone, Copy, PartialEq)]
struct GlidePart {
    #[knuffel(child)]
    off: bool,
    #[knuffel(child)]
    on: bool,
    #[knuffel(child, unwrap(argument))]
    tilt_deg: Option<FloatOrInt<0, 45>>,
    #[knuffel(child, unwrap(argument))]
    depth: Option<FloatOrInt<0, 1>>,
}

impl Glide {
    fn merge(&mut self, part: GlidePart) {
        self.off |= part.off;
        if part.on {
            self.off = false;
        }
        if let Some(value) = part.tilt_deg {
            self.tilt_deg = value.0;
        }
        if let Some(value) = part.depth {
            self.depth = value.0;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowWobble {
    pub off: bool,
    pub mesh: u8,
    pub stiffness: f64,
    pub damping: f64,
    pub mass: f64,
    pub intensity: f64,
    pub position_epsilon: f64,
    pub velocity_epsilon: f64,
}

impl Default for WindowWobble {
    fn default() -> Self {
        Self {
            off: false,
            mesh: 12,
            stiffness: 200.,
            damping: 12.,
            mass: 1.,
            intensity: 0.2,
            position_epsilon: 0.25,
            velocity_epsilon: 2.,
        }
    }
}

#[derive(knuffel::Decode, Debug, Default, Clone, Copy, PartialEq)]
struct WindowWobblePart {
    #[knuffel(child)]
    off: bool,
    #[knuffel(child)]
    on: bool,
    #[knuffel(child, unwrap(argument))]
    mesh: Option<u8>,
    #[knuffel(child, unwrap(argument))]
    stiffness: Option<FloatOrInt<0, 1000>>,
    #[knuffel(child, unwrap(argument))]
    damping: Option<FloatOrInt<0, 1000>>,
    #[knuffel(child, unwrap(argument))]
    mass: Option<FloatOrInt<0, 1000>>,
    #[knuffel(child, unwrap(argument))]
    intensity: Option<FloatOrInt<0, 3>>,
    #[knuffel(child, unwrap(argument))]
    position_epsilon: Option<FloatOrInt<0, 100>>,
    #[knuffel(child, unwrap(argument))]
    velocity_epsilon: Option<FloatOrInt<0, 1000>>,
}

impl<S> knuffel::Decode<S> for WindowWobble
where
    S: knuffel::traits::ErrorSpan,
{
    fn decode_node(
        node: &knuffel::ast::SpannedNode<S>,
        ctx: &mut knuffel::decode::Context<S>,
    ) -> Result<Self, DecodeError<S>> {
        let part = WindowWobblePart::decode_node(node, ctx)?;
        let mut value = Self::default();
        value.off |= part.off;
        if part.on {
            value.off = false;
        }
        if let Some(mesh) = part.mesh {
            if !(2..=32).contains(&mesh) {
                ctx.emit_error(DecodeError::unexpected(
                    &node.node_name,
                    "window-wobble mesh",
                    "mesh must be between 2 and 32",
                ));
            } else {
                value.mesh = mesh;
            }
        }
        if let Some(v) = part.stiffness {
            value.stiffness = v.0.max(0.0001);
        }
        if let Some(v) = part.damping {
            value.damping = v.0;
        }
        if let Some(v) = part.mass {
            if v.0 < 0.1 {
                ctx.emit_error(DecodeError::unexpected(
                    &node.node_name,
                    "window-wobble mass",
                    "mass must be at least 0.1",
                ));
            } else {
                value.mass = v.0;
            }
        }
        if let Some(v) = part.intensity {
            value.intensity = v.0;
        }
        if let Some(v) = part.position_epsilon {
            value.position_epsilon = v.0;
        }
        if let Some(v) = part.velocity_epsilon {
            value.velocity_epsilon = v.0;
        }
        Ok(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WindowOpenEffect {
    #[default]
    Fade,
    Reveal,
}

impl std::str::FromStr for WindowOpenEffect {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "fade" => Ok(Self::Fade),
            "reveal" => Ok(Self::Reveal),
            _ => Err("expected \"fade\" or \"reveal\""),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WindowOpenAnim {
    pub anim: Animation,
    pub custom_shader: Option<String>,
    pub popin_ratio: Option<f64>,
    pub glide: Glide,
    pub effect: WindowOpenEffect,
}

impl Default for WindowOpenAnim {
    fn default() -> Self {
        Self {
            anim: Animation {
                off: false,
                // A lightly underdamped spring: one visible overshoot, then a
                // short settle. Duration comes from the spring rather than a
                // fixed timeline, so interrupts keep their velocity.
                kind: Kind::Spring(SpringParams {
                    damping_ratio: 0.62,
                    stiffness: 600,
                    epsilon: 0.0015,
                }),
            },
            custom_shader: None,
            popin_ratio: None,
            glide: Glide {
                off: true,
                ..Glide::default()
            },
            effect: WindowOpenEffect::Fade,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WindowCloseEffect {
    #[default]
    Explosion,
    Implode,
    Fade,
}

impl WindowCloseEffect {
    const fn denial_duration_ms(self) -> u32 {
        match self {
            Self::Explosion => 400,
            Self::Implode => 280,
            Self::Fade => 220,
        }
    }
}

impl std::str::FromStr for WindowCloseEffect {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "explosion" | "explode" | "particles" => Ok(Self::Explosion),
            "implode" | "implosion" | "shrink" => Ok(Self::Implode),
            "fade" => Ok(Self::Fade),
            _ => Err("invalid window close effect, can be 'explosion', 'implode', or 'fade'"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WindowCloseAnim {
    pub anim: Animation,
    pub custom_shader: Option<String>,
    pub effect: WindowCloseEffect,
    pub glide: Glide,
}

impl Default for WindowCloseAnim {
    fn default() -> Self {
        let effect = std::env::var("CLARION_WINDOW_CLOSE_EFFECT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(WindowCloseEffect::Explosion);

        Self {
            anim: Animation {
                off: false,
                kind: Kind::Easing(EasingParams {
                    duration_ms: effect.denial_duration_ms(),
                    // Denial drives close effects with a linear controller; each visual phase
                    // applies its own curve and interval.
                    curve: Curve::Linear,
                }),
            },
            custom_shader: None,
            effect,
            glide: Glide::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HorizontalViewMovementAnim(pub Animation);

impl Default for HorizontalViewMovementAnim {
    fn default() -> Self {
        Self(Animation {
            off: false,
            kind: Kind::Spring(SpringParams {
                damping_ratio: 1.,
                stiffness: 800,
                epsilon: 0.0001,
            }),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowMovementAnim(pub Animation);

impl Default for WindowMovementAnim {
    fn default() -> Self {
        Self(Animation {
            off: false,
            kind: Kind::Spring(SpringParams::MACOS_INTERACTIVE),
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WindowResizeAnim {
    pub anim: Animation,
    pub custom_shader: Option<String>,
}

impl Default for WindowResizeAnim {
    fn default() -> Self {
        Self {
            anim: Animation {
                off: false,
                kind: Kind::Spring(SpringParams::MACOS_INTERACTIVE),
            },
            custom_shader: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConfigNotificationOpenCloseAnim(pub Animation);

impl Default for ConfigNotificationOpenCloseAnim {
    fn default() -> Self {
        Self(Animation {
            off: false,
            kind: Kind::Spring(SpringParams {
                damping_ratio: 0.6,
                stiffness: 1000,
                epsilon: 0.001,
            }),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExitConfirmationOpenCloseAnim(pub Animation);

impl Default for ExitConfirmationOpenCloseAnim {
    fn default() -> Self {
        Self(Animation {
            off: false,
            kind: Kind::Spring(SpringParams {
                damping_ratio: 0.6,
                stiffness: 500,
                epsilon: 0.01,
            }),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenshotUiOpenAnim(pub Animation);

impl Default for ScreenshotUiOpenAnim {
    fn default() -> Self {
        Self(Animation {
            off: false,
            kind: Kind::Easing(EasingParams {
                duration_ms: 200,
                curve: Curve::EaseOutQuad,
            }),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverviewOpenCloseAnim(pub Animation);

impl Default for OverviewOpenCloseAnim {
    fn default() -> Self {
        Self(Animation {
            off: false,
            kind: Kind::Spring(SpringParams {
                damping_ratio: 1.,
                stiffness: 800,
                epsilon: 0.0001,
            }),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RecentWindowsCloseAnim(pub Animation);

impl Default for RecentWindowsCloseAnim {
    fn default() -> Self {
        Self(Animation {
            off: false,
            kind: Kind::Spring(SpringParams {
                damping_ratio: 1.,
                stiffness: 800,
                epsilon: 0.001,
            }),
        })
    }
}

impl<S> knuffel::Decode<S> for WorkspaceSwitchAnim
where
    S: knuffel::traits::ErrorSpan,
{
    fn decode_node(
        node: &knuffel::ast::SpannedNode<S>,
        ctx: &mut knuffel::decode::Context<S>,
    ) -> Result<Self, DecodeError<S>> {
        let default = Self::default();
        let mut cube = default.1;
        let anim = Animation::decode_node(node, ctx, default.0, |child, ctx| {
            if &**child.node_name == "cube" {
                cube.merge(WorkspaceCubePart::decode_node(child, ctx)?);
                Ok(true)
            } else {
                Ok(false)
            }
        })?;
        Ok(Self(anim, cube))
    }
}

impl<S> knuffel::Decode<S> for HorizontalViewMovementAnim
where
    S: knuffel::traits::ErrorSpan,
{
    fn decode_node(
        node: &knuffel::ast::SpannedNode<S>,
        ctx: &mut knuffel::decode::Context<S>,
    ) -> Result<Self, DecodeError<S>> {
        let default = Self::default().0;
        Ok(Self(Animation::decode_node(node, ctx, default, |_, _| {
            Ok(false)
        })?))
    }
}

impl<S> knuffel::Decode<S> for WindowMovementAnim
where
    S: knuffel::traits::ErrorSpan,
{
    fn decode_node(
        node: &knuffel::ast::SpannedNode<S>,
        ctx: &mut knuffel::decode::Context<S>,
    ) -> Result<Self, DecodeError<S>> {
        let default = Self::default().0;
        Ok(Self(Animation::decode_node(node, ctx, default, |_, _| {
            Ok(false)
        })?))
    }
}

impl<S> knuffel::Decode<S> for WindowOpenAnim
where
    S: knuffel::traits::ErrorSpan,
{
    fn decode_node(
        node: &knuffel::ast::SpannedNode<S>,
        ctx: &mut knuffel::decode::Context<S>,
    ) -> Result<Self, DecodeError<S>> {
        let default = Self::default().anim;
        let mut custom_shader = None;
        let mut popin_ratio = None;
        let mut glide = Self::default().glide;
        let mut effect = Self::default().effect;
        let anim = Animation::decode_node(node, ctx, default, |child, ctx| {
            if &**child.node_name == "custom-shader" {
                custom_shader = parse_arg_node("custom-shader", child, ctx)?;
                Ok(true)
            } else if &**child.node_name == "popin" || &**child.node_name == "popin-ratio" {
                popin_ratio = parse_arg_node("popin", child, ctx)?;
                Ok(true)
            } else if &**child.node_name == "glide" {
                glide.merge(GlidePart::decode_node(child, ctx)?);
                Ok(true)
            } else if &**child.node_name == "effect" {
                let value: String = parse_arg_node("effect", child, ctx)?;
                match value.parse() {
                    Ok(value) => effect = value,
                    Err(err) => {
                        ctx.emit_error(DecodeError::unexpected(&child.node_name, "effect", err))
                    }
                }
                Ok(true)
            } else {
                Ok(false)
            }
        })?;

        Ok(Self {
            anim,
            custom_shader,
            popin_ratio,
            glide,
            effect,
        })
    }
}

impl<S> knuffel::Decode<S> for WindowCloseAnim
where
    S: knuffel::traits::ErrorSpan,
{
    fn decode_node(
        node: &knuffel::ast::SpannedNode<S>,
        ctx: &mut knuffel::decode::Context<S>,
    ) -> Result<Self, DecodeError<S>> {
        let default = Self::default();
        let has_explicit_timing = node
            .children()
            .any(|child| matches!(&**child.node_name, "duration-ms" | "curve" | "spring"));
        let mut custom_shader = None;
        let mut effect = default.effect;
        let mut glide = default.glide;
        let mut anim = Animation::decode_node(node, ctx, default.anim, |child, ctx| {
            if &**child.node_name == "custom-shader" {
                custom_shader = parse_arg_node("custom-shader", child, ctx)?;
                Ok(true)
            } else if &**child.node_name == "effect" {
                let eff_str: String = parse_arg_node("effect", child, ctx)?;
                match eff_str.parse() {
                    Ok(eff) => effect = eff,
                    Err(err) => {
                        ctx.emit_error(DecodeError::unexpected(&child.node_name, "effect", err));
                    }
                }
                Ok(true)
            } else if &**child.node_name == "glide" {
                glide.merge(GlidePart::decode_node(child, ctx)?);
                Ok(true)
            } else {
                Ok(false)
            }
        })?;

        // Denial gives every built-in terminal effect its own controller duration. Explicit
        // timing remains authoritative, including for user-supplied shaders.
        if !has_explicit_timing && custom_shader.is_none() {
            anim.kind = Kind::Easing(EasingParams {
                duration_ms: effect.denial_duration_ms(),
                curve: Curve::Linear,
            });
        }

        Ok(Self {
            anim,
            custom_shader,
            effect,
            glide,
        })
    }
}

impl<S> knuffel::Decode<S> for WindowResizeAnim
where
    S: knuffel::traits::ErrorSpan,
{
    fn decode_node(
        node: &knuffel::ast::SpannedNode<S>,
        ctx: &mut knuffel::decode::Context<S>,
    ) -> Result<Self, DecodeError<S>> {
        let default = Self::default().anim;
        let mut custom_shader = None;
        let anim = Animation::decode_node(node, ctx, default, |child, ctx| {
            if &**child.node_name == "custom-shader" {
                custom_shader = parse_arg_node("custom-shader", child, ctx)?;
                Ok(true)
            } else {
                Ok(false)
            }
        })?;

        Ok(Self {
            anim,
            custom_shader,
        })
    }
}

impl<S> knuffel::Decode<S> for ConfigNotificationOpenCloseAnim
where
    S: knuffel::traits::ErrorSpan,
{
    fn decode_node(
        node: &knuffel::ast::SpannedNode<S>,
        ctx: &mut knuffel::decode::Context<S>,
    ) -> Result<Self, DecodeError<S>> {
        let default = Self::default().0;
        Ok(Self(Animation::decode_node(node, ctx, default, |_, _| {
            Ok(false)
        })?))
    }
}

impl<S> knuffel::Decode<S> for ExitConfirmationOpenCloseAnim
where
    S: knuffel::traits::ErrorSpan,
{
    fn decode_node(
        node: &knuffel::ast::SpannedNode<S>,
        ctx: &mut knuffel::decode::Context<S>,
    ) -> Result<Self, DecodeError<S>> {
        let default = Self::default().0;
        Ok(Self(Animation::decode_node(node, ctx, default, |_, _| {
            Ok(false)
        })?))
    }
}

impl<S> knuffel::Decode<S> for ScreenshotUiOpenAnim
where
    S: knuffel::traits::ErrorSpan,
{
    fn decode_node(
        node: &knuffel::ast::SpannedNode<S>,
        ctx: &mut knuffel::decode::Context<S>,
    ) -> Result<Self, DecodeError<S>> {
        let default = Self::default().0;
        Ok(Self(Animation::decode_node(node, ctx, default, |_, _| {
            Ok(false)
        })?))
    }
}

impl<S> knuffel::Decode<S> for OverviewOpenCloseAnim
where
    S: knuffel::traits::ErrorSpan,
{
    fn decode_node(
        node: &knuffel::ast::SpannedNode<S>,
        ctx: &mut knuffel::decode::Context<S>,
    ) -> Result<Self, DecodeError<S>> {
        let default = Self::default().0;
        Ok(Self(Animation::decode_node(node, ctx, default, |_, _| {
            Ok(false)
        })?))
    }
}

impl<S> knuffel::Decode<S> for RecentWindowsCloseAnim
where
    S: knuffel::traits::ErrorSpan,
{
    fn decode_node(
        node: &knuffel::ast::SpannedNode<S>,
        ctx: &mut knuffel::decode::Context<S>,
    ) -> Result<Self, DecodeError<S>> {
        let default = Self::default().0;
        Ok(Self(Animation::decode_node(node, ctx, default, |_, _| {
            Ok(false)
        })?))
    }
}

impl Animation {
    pub fn new_off() -> Self {
        Self {
            off: true,
            kind: Kind::Easing(EasingParams {
                duration_ms: 0,
                curve: Curve::Linear,
            }),
        }
    }

    fn decode_node<S: knuffel::traits::ErrorSpan>(
        node: &knuffel::ast::SpannedNode<S>,
        ctx: &mut knuffel::decode::Context<S>,
        default: Self,
        mut process_children: impl FnMut(
            &knuffel::ast::SpannedNode<S>,
            &mut knuffel::decode::Context<S>,
        ) -> Result<bool, DecodeError<S>>,
    ) -> Result<Self, DecodeError<S>> {
        #[derive(Default, PartialEq)]
        struct OptionalEasingParams {
            duration_ms: Option<u32>,
            curve: Option<Curve>,
        }

        expect_only_children(node, ctx);

        let mut off = false;
        let mut easing_params = OptionalEasingParams::default();
        let mut spring_params = None;

        for child in node.children() {
            match &**child.node_name {
                "off" => {
                    knuffel::decode::check_flag_node(child, ctx);
                    if off {
                        ctx.emit_error(DecodeError::unexpected(
                            &child.node_name,
                            "node",
                            "duplicate node `off`, single node expected",
                        ));
                    } else {
                        off = true;
                    }
                }
                "spring" => {
                    if easing_params != OptionalEasingParams::default() {
                        ctx.emit_error(DecodeError::unexpected(
                            child,
                            "node",
                            "cannot set both spring and easing parameters at once",
                        ));
                    }
                    if spring_params.is_some() {
                        ctx.emit_error(DecodeError::unexpected(
                            &child.node_name,
                            "node",
                            "duplicate node `spring`, single node expected",
                        ));
                    }

                    spring_params = Some(SpringParams::decode_node(child, ctx)?);
                }
                "duration-ms" => {
                    if spring_params.is_some() {
                        ctx.emit_error(DecodeError::unexpected(
                            child,
                            "node",
                            "cannot set both spring and easing parameters at once",
                        ));
                    }
                    if easing_params.duration_ms.is_some() {
                        ctx.emit_error(DecodeError::unexpected(
                            &child.node_name,
                            "node",
                            "duplicate node `duration-ms`, single node expected",
                        ));
                    }

                    easing_params.duration_ms = Some(parse_arg_node("duration-ms", child, ctx)?);
                }
                "curve" => {
                    if spring_params.is_some() {
                        ctx.emit_error(DecodeError::unexpected(
                            child,
                            "node",
                            "cannot set both spring and easing parameters at once",
                        ));
                    }
                    if easing_params.curve.is_some() {
                        ctx.emit_error(DecodeError::unexpected(
                            &child.node_name,
                            "node",
                            "duplicate node `curve`, single node expected",
                        ));
                    }

                    let mut iter_args = child.arguments.iter();
                    let val = iter_args.next().ok_or_else(|| {
                        DecodeError::missing(child, "additional argument `curve` is required")
                    })?;
                    let animation_curve_string: String =
                        knuffel::traits::DecodeScalar::decode(val, ctx)?;

                    let animation_curve = match animation_curve_string.as_str() {
                        "linear" | "macos-linear" | "ca-linear" => Some(Curve::Linear),
                        "ease-out-quad" => Some(Curve::EaseOutQuad),
                        "ease-out-cubic" => Some(Curve::EaseOutCubic),
                        "ease-out-expo" => Some(Curve::EaseOutExpo),
                        // Native macOS Core Animation presets:
                        "default" | "macos-default" | "ca-default" => Some(Curve::MACOS_DEFAULT),
                        "ease-in" | "macos-ease-in" | "ca-ease-in" => Some(Curve::MACOS_EASE_IN),
                        "ease-out" | "macos-ease-out" | "ca-ease-out" => {
                            Some(Curve::MACOS_EASE_OUT)
                        }
                        "ease-in-out"
                        | "ease-in-ease-out"
                        | "macos-ease-in-out"
                        | "macos-ease-in-ease-out"
                        | "ca-ease-in-out"
                        | "ca-ease-in-ease-out" => Some(Curve::MACOS_EASE_IN_EASE_OUT),
                        // Hyprland presets:
                        "overshot" | "overshoot" => Some(Curve::CubicBezier(0.05, 0.9, 0.1, 1.05)),
                        "smooth-out" | "smoothout" => Some(Curve::CubicBezier(0.0, 0.0, 0.58, 1.0)),
                        "quick" => Some(Curve::CubicBezier(0.15, 0.0, 0.05, 1.0)),
                        "bouncy" => Some(Curve::CubicBezier(0.68, -0.55, 0.265, 1.55)),
                        "cubic-bezier" | "bezier" => {
                            let val = iter_args.next().ok_or_else(|| {
                                DecodeError::missing(
                                    child,
                                    "missing x1 coordinate for cubic Bézier curve control point",
                                )
                            })?;
                            // the X axis represents time frame so it cannot be negative
                            // or larger than 1
                            let x1: FloatOrInt<0, 1> =
                                knuffel::traits::DecodeScalar::decode(val, ctx)?;
                            let val = iter_args.next().ok_or_else(|| {
                                DecodeError::missing(
                                    child,
                                    "missing y1 coordinate for cubic Bézier curve control point",
                                )
                            })?;
                            let y1: FloatOrInt<{ i32::MIN }, { i32::MAX }> =
                                knuffel::traits::DecodeScalar::decode(val, ctx)?;
                            let val = iter_args.next().ok_or_else(|| {
                                DecodeError::missing(
                                    child,
                                    "missing x2 coordinate for cubic Bézier curve control point",
                                )
                            })?;
                            let x2: FloatOrInt<0, 1> =
                                knuffel::traits::DecodeScalar::decode(val, ctx)?;
                            let val = iter_args.next().ok_or_else(|| {
                                DecodeError::missing(
                                    child,
                                    "missing y2 coordinate for cubic Bézier curve control point",
                                )
                            })?;
                            let y2: FloatOrInt<{ i32::MIN }, { i32::MAX }> =
                                knuffel::traits::DecodeScalar::decode(val, ctx)?;

                            Some(Curve::CubicBezier(x1.0, y1.0, x2.0, y2.0))
                        }
                        unexpected_curve => {
                            ctx.emit_error(DecodeError::unexpected(
                                &val.literal,
                                "argument",
                                format!(
                                    "unexpected animation curve `{unexpected_curve}`. \
                                    supported curves are `ease-out-quad`, `ease-out-cubic`, \
                                    `ease-out-expo`, `linear` (or `macos-linear`, `ca-linear`), \
                                    `cubic-bezier`, `smooth-out`, `overshot`, `quick`, `bouncy`, \
                                    `default` (or `macos-default`, `ca-default`), \
                                    `ease-in` (or `macos-ease-in`, `ca-ease-in`), \
                                    `ease-out` (or `macos-ease-out`, `ca-ease-out`), \
                                    and `ease-in-out` (or `macos-ease-in-out`, `ca-ease-in-out`)."
                                ),
                            ));

                            None
                        }
                    };

                    if let Some(val) = iter_args.next() {
                        ctx.emit_error(DecodeError::unexpected(
                            &val.literal,
                            "argument",
                            "unexpected argument",
                        ));
                    }
                    for name in child.properties.keys() {
                        ctx.emit_error(DecodeError::unexpected(
                            name,
                            "property",
                            format!("unexpected property `{}`", name.escape_default()),
                        ));
                    }
                    for child in child.children() {
                        ctx.emit_error(DecodeError::unexpected(
                            child,
                            "node",
                            format!("unexpected node `{}`", child.node_name.escape_default()),
                        ));
                    }

                    easing_params.curve = animation_curve;
                }
                name_str => {
                    if !process_children(child, ctx)? {
                        ctx.emit_error(DecodeError::unexpected(
                            child,
                            "node",
                            format!("unexpected node `{}`", name_str.escape_default()),
                        ));
                    }
                }
            }
        }

        let kind = if let Some(spring_params) = spring_params {
            // Configured spring.
            Kind::Spring(spring_params)
        } else if easing_params == OptionalEasingParams::default() {
            // Did not configure anything.
            default.kind
        } else {
            // Configured easing.
            let default = if let Kind::Easing(easing) = default.kind {
                easing
            } else {
                // Generic fallback values for when the default animation is spring, but the user
                // configured an easing animation.
                EasingParams {
                    duration_ms: 250,
                    curve: Curve::EaseOutCubic,
                }
            };

            Kind::Easing(EasingParams {
                duration_ms: easing_params.duration_ms.unwrap_or(default.duration_ms),
                curve: easing_params.curve.unwrap_or(default.curve),
            })
        };

        Ok(Self { off, kind })
    }
}

impl<S> knuffel::Decode<S> for SpringParams
where
    S: knuffel::traits::ErrorSpan,
{
    fn decode_node(
        node: &knuffel::ast::SpannedNode<S>,
        ctx: &mut knuffel::decode::Context<S>,
    ) -> Result<Self, DecodeError<S>> {
        if let Some(type_name) = &node.type_name {
            ctx.emit_error(DecodeError::unexpected(
                type_name,
                "type name",
                "no type name expected for this node",
            ));
        }

        let mut base_params = None;
        if let Some(val) = node.arguments.first() {
            let preset_name: String = knuffel::traits::DecodeScalar::decode(val, ctx)?;
            match preset_name.as_str() {
                "default" | "macos-default" | "ca-default" => {
                    base_params = Some(Self::MACOS_DEFAULT);
                }
                "macos-interactive" | "interactive" => {
                    base_params = Some(Self::MACOS_INTERACTIVE);
                }
                other => {
                    ctx.emit_error(DecodeError::unexpected(
                        &val.literal,
                        "argument",
                        format!(
                            "unexpected spring preset `{other}`. supported presets are \
                            `default` (or `macos-default`, `ca-default`), and \
                            `macos-interactive` (or `interactive`)."
                        ),
                    ));
                }
            }
            if node.arguments.len() > 1 {
                for extra in &node.arguments[1..] {
                    ctx.emit_error(DecodeError::unexpected(
                        &extra.literal,
                        "argument",
                        "unexpected extra argument for spring",
                    ));
                }
            }
        }

        for child in node.children() {
            ctx.emit_error(DecodeError::unexpected(
                child,
                "node",
                format!("unexpected node `{}`", child.node_name.escape_default()),
            ));
        }

        let mut damping_ratio = None;
        let mut stiffness = None;
        let mut epsilon = None;
        for (name, val) in &node.properties {
            match &***name {
                "damping-ratio" => {
                    damping_ratio = Some(knuffel::traits::DecodeScalar::decode(val, ctx)?);
                }
                "stiffness" => {
                    stiffness = Some(knuffel::traits::DecodeScalar::decode(val, ctx)?);
                }
                "epsilon" => {
                    epsilon = Some(knuffel::traits::DecodeScalar::decode(val, ctx)?);
                }
                name_str => {
                    ctx.emit_error(DecodeError::unexpected(
                        name,
                        "property",
                        format!("unexpected property `{}`", name_str.escape_default()),
                    ));
                }
            }
        }

        let (default_damping, default_stiffness, default_epsilon) = match base_params {
            Some(base) => (
                Some(base.damping_ratio),
                Some(base.stiffness),
                Some(base.epsilon),
            ),
            None => (None, None, None),
        };

        let damping_ratio = damping_ratio
            .or(default_damping)
            .ok_or_else(|| DecodeError::missing(node, "property `damping-ratio` is required"))?;
        let stiffness = stiffness
            .or(default_stiffness)
            .ok_or_else(|| DecodeError::missing(node, "property `stiffness` is required"))?;
        let epsilon = epsilon
            .or(default_epsilon)
            .ok_or_else(|| DecodeError::missing(node, "property `epsilon` is required"))?;

        if !(0.1..=10.).contains(&damping_ratio) {
            ctx.emit_error(DecodeError::conversion(
                node,
                "damping-ratio must be between 0.1 and 10.0",
            ));
        }
        if stiffness < 1 {
            ctx.emit_error(DecodeError::conversion(node, "stiffness must be >= 1"));
        }
        if !(0.00001..=0.1).contains(&epsilon) {
            ctx.emit_error(DecodeError::conversion(
                node,
                "epsilon must be between 0.00001 and 0.1",
            ));
        }

        Ok(SpringParams {
            damping_ratio,
            stiffness,
            epsilon,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, knuffel::Decode)]
    struct AnimationConfig {
        #[knuffel(child)]
        window_movement: WindowMovementAnim,
        #[knuffel(child)]
        window_resize: WindowResizeAnim,
    }

    #[test]
    fn interactive_defaults_are_used_without_overwriting_explicit_springs() {
        assert_eq!(
            WindowMovementAnim::default().0.kind,
            Kind::Spring(SpringParams::MACOS_INTERACTIVE)
        );
        assert_eq!(
            WindowResizeAnim::default().anim.kind,
            Kind::Spring(SpringParams::MACOS_INTERACTIVE)
        );
        let config: AnimationConfig = knuffel::parse(
            "test.kdl",
            r#"
            window-movement { spring damping-ratio=1.15 stiffness=850 epsilon=0.0001; }
            window-resize { spring damping-ratio=0.5 stiffness=100 epsilon=0.01; }
        "#,
        )
        .unwrap();
        assert_eq!(
            config.window_movement.0.kind,
            Kind::Spring(SpringParams {
                damping_ratio: 1.15,
                stiffness: 850,
                epsilon: 0.0001
            })
        );
        assert_eq!(
            config.window_resize.anim.kind,
            Kind::Spring(SpringParams {
                damping_ratio: 0.5,
                stiffness: 100,
                epsilon: 0.01
            })
        );
    }

    #[test]
    fn opening_reference_timing_keeps_explicit_overrides() {
        let reference = WindowOpenAnim::default();
        assert_eq!(
            reference.anim.kind,
            Kind::Spring(SpringParams {
                damping_ratio: 0.62,
                stiffness: 600,
                epsilon: 0.0015,
            })
        );
        assert_eq!(reference.effect, WindowOpenEffect::Fade);
        assert!(reference.glide.off);
        #[derive(knuffel::Decode)]
        struct OpenConfig {
            #[knuffel(child)]
            window_open: WindowOpenAnim,
        }
        let explicit: OpenConfig = knuffel::parse(
            "test.kdl",
            r#"
            window-open { duration-ms 713; curve "linear"; effect "reveal"; glide { on; }; }
        "#,
        )
        .unwrap();
        assert_eq!(explicit.window_open.effect, WindowOpenEffect::Reveal);
        assert!(!explicit.window_open.glide.off);
        assert!(
            knuffel::parse::<OpenConfig>("test.kdl", r#"window-open { effect "typo"; }"#).is_err()
        );
        assert_eq!(
            explicit.window_open.anim.kind,
            Kind::Easing(EasingParams {
                duration_ms: 713,
                curve: Curve::Linear,
            })
        );
    }

    #[test]
    fn core_animation_curves_decode_and_keep_custom_duration() {
        for (name, curve) in [
            ("default", Curve::MACOS_DEFAULT),
            ("macos-default", Curve::MACOS_DEFAULT),
            ("ca-default", Curve::MACOS_DEFAULT),
            ("linear", Curve::Linear),
            ("macos-linear", Curve::Linear),
            ("ca-linear", Curve::Linear),
            ("ease-in", Curve::MACOS_EASE_IN),
            ("macos-ease-in", Curve::MACOS_EASE_IN),
            ("ca-ease-in", Curve::MACOS_EASE_IN),
            ("ease-out", Curve::MACOS_EASE_OUT),
            ("macos-ease-out", Curve::MACOS_EASE_OUT),
            ("ca-ease-out", Curve::MACOS_EASE_OUT),
            ("ease-in-out", Curve::MACOS_EASE_IN_EASE_OUT),
            ("ease-in-ease-out", Curve::MACOS_EASE_IN_EASE_OUT),
            ("macos-ease-in-out", Curve::MACOS_EASE_IN_EASE_OUT),
            ("macos-ease-in-ease-out", Curve::MACOS_EASE_IN_EASE_OUT),
            ("ca-ease-in-out", Curve::MACOS_EASE_IN_EASE_OUT),
            ("ca-ease-in-ease-out", Curve::MACOS_EASE_IN_EASE_OUT),
        ] {
            let text = format!(
                r#"window-movement {{ curve "{name}"; duration-ms 255; }}
                window-resize {{ off; }}"#
            );
            let config: AnimationConfig = knuffel::parse("test.kdl", &text).unwrap();
            assert_eq!(
                config.window_movement.0.kind,
                Kind::Easing(EasingParams {
                    duration_ms: 255,
                    curve
                })
            );
            assert!(config.window_resize.anim.off);
        }
    }

    #[test]
    fn spring_presets_decode_and_support_property_overrides() {
        for name in ["default", "macos-default", "ca-default"] {
            let text = format!(
                r#"window-movement {{ spring "{name}"; }}
                window-resize {{ off; }}"#
            );
            let config: AnimationConfig = knuffel::parse("test.kdl", &text).unwrap();
            assert_eq!(
                config.window_movement.0.kind,
                Kind::Spring(SpringParams::MACOS_DEFAULT)
            );
        }

        for name in ["macos-interactive", "interactive"] {
            let text = format!(
                r#"window-movement {{ spring "{name}"; }}
                window-resize {{ off; }}"#
            );
            let config: AnimationConfig = knuffel::parse("test.kdl", &text).unwrap();
            assert_eq!(
                config.window_movement.0.kind,
                Kind::Spring(SpringParams::MACOS_INTERACTIVE)
            );
        }

        // Test property overrides on preset
        let text = r#"
            window-movement { spring "macos-default" stiffness=250; }
            window-resize { spring "macos-interactive" epsilon=0.0001; }
        "#;
        let config: AnimationConfig = knuffel::parse("test.kdl", text).unwrap();
        assert_eq!(
            config.window_movement.0.kind,
            Kind::Spring(SpringParams {
                damping_ratio: 0.5,
                stiffness: 250,
                epsilon: 0.001,
            })
        );
        assert_eq!(
            config.window_resize.anim.kind,
            Kind::Spring(SpringParams {
                damping_ratio: 0.92,
                stiffness: 300,
                epsilon: 0.0001,
            })
        );

        // Unknown preset emits error
        let err = knuffel::parse::<AnimationConfig>(
            "test.kdl",
            r#"window-movement { spring "unknown-spring"; }"#,
        )
        .unwrap_err();
        assert!(format!("{err:?}").contains("unexpected spring preset `unknown-spring`"));
    }
}
