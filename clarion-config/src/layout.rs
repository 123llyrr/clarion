use clarion_ipc::{ColumnDisplay, SizeChange};
use knuffel::errors::DecodeError;

use crate::appearance::{
    Border, CornerRadius, FocusRing, InsertHint, Shadow, TabIndicator, DEFAULT_BACKGROUND_COLOR,
};
use crate::utils::{expect_only_children, Flag, MergeWith};
use crate::{BorderRule, Color, FloatOrInt, InsertHintPart, ShadowRule, TabIndicatorPart};

#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub focus_ring: FocusRing,
    pub border: Border,
    pub shadow: Shadow,
    pub tab_indicator: TabIndicator,
    pub insert_hint: InsertHint,
    pub preset_column_widths: Vec<PresetSize>,
    pub default_column_width: Option<PresetSize>,
    pub preset_window_heights: Vec<PresetSize>,
    pub center_focused_column: CenterFocusedColumn,
    pub always_center_single_column: bool,
    pub empty_workspace_above_first: bool,
    pub default_column_display: ColumnDisplay,
    pub auto_stack_new_windows: bool,
    pub max_windows_per_column: u32,
    pub gaps: f64,
    pub struts: Struts,
    pub background_color: Color,
    pub dim_inactive: bool,
    pub dim_factor: f32,
    /// Hyprland-style window corner radius in logical pixels.
    pub rounding: f64,
    /// Superellipse exponent; 2 is a circle, higher values approach a squircle.
    pub rounding_power: f64,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            focus_ring: FocusRing::default(),
            border: Border::default(),
            shadow: Shadow::default(),
            tab_indicator: TabIndicator::default(),
            insert_hint: InsertHint::default(),
            preset_column_widths: vec![
                PresetSize::Proportion(1. / 3.),
                PresetSize::Proportion(0.5),
                PresetSize::Proportion(2. / 3.),
            ],
            default_column_width: Some(PresetSize::Proportion(0.5)),
            center_focused_column: CenterFocusedColumn::Never,
            always_center_single_column: false,
            empty_workspace_above_first: false,
            default_column_display: ColumnDisplay::Normal,
            auto_stack_new_windows: true,
            max_windows_per_column: 3,
            gaps: 16.,
            struts: Struts::default(),
            preset_window_heights: vec![
                PresetSize::Proportion(1. / 3.),
                PresetSize::Proportion(0.5),
                PresetSize::Proportion(2. / 3.),
            ],
            background_color: DEFAULT_BACKGROUND_COLOR,
            dim_inactive: false,
            dim_factor: 0.85,
            rounding: 0.,
            rounding_power: 2.,
        }
    }
}

impl MergeWith<LayoutPart> for Layout {
    fn merge_with(&mut self, part: &LayoutPart) {
        merge!(
            (self, part),
            focus_ring,
            border,
            shadow,
            tab_indicator,
            insert_hint,
            always_center_single_column,
            empty_workspace_above_first,
            auto_stack_new_windows,
            max_windows_per_column,
            gaps,
            dim_inactive,
        );

        if let Some(x) = part.dim_factor {
            self.dim_factor = x;
        }
        if let Some(x) = part.rounding {
            self.rounding = x.0;
        }
        if let Some(x) = part.rounding_power {
            self.rounding_power = x.0.max(2.);
        }

        merge_clone!(
            (self, part),
            preset_column_widths,
            preset_window_heights,
            center_focused_column,
            default_column_display,
            struts,
            background_color,
        );

        if let Some(x) = part.default_column_width {
            self.default_column_width = x.0;
        }

        if self.preset_column_widths.is_empty() {
            self.preset_column_widths = Layout::default().preset_column_widths;
        }

        if self.preset_window_heights.is_empty() {
            self.preset_window_heights = Layout::default().preset_window_heights;
        }
    }
}

#[derive(knuffel::Decode, Debug, Default, Clone, PartialEq)]
pub struct LayoutPart {
    #[knuffel(child)]
    pub focus_ring: Option<BorderRule>,
    #[knuffel(child)]
    pub border: Option<BorderRule>,
    #[knuffel(child)]
    pub shadow: Option<ShadowRule>,
    #[knuffel(child)]
    pub tab_indicator: Option<TabIndicatorPart>,
    #[knuffel(child)]
    pub insert_hint: Option<InsertHintPart>,
    #[knuffel(child, unwrap(children))]
    pub preset_column_widths: Option<Vec<PresetSize>>,
    #[knuffel(child)]
    pub default_column_width: Option<DefaultPresetSize>,
    #[knuffel(child, unwrap(children))]
    pub preset_window_heights: Option<Vec<PresetSize>>,
    #[knuffel(child, unwrap(argument))]
    pub center_focused_column: Option<CenterFocusedColumn>,
    #[knuffel(child)]
    pub always_center_single_column: Option<Flag>,
    #[knuffel(child)]
    pub empty_workspace_above_first: Option<Flag>,
    #[knuffel(child, unwrap(argument, str))]
    pub default_column_display: Option<ColumnDisplay>,
    #[knuffel(child)]
    pub auto_stack_new_windows: Option<Flag>,
    #[knuffel(child, unwrap(argument))]
    pub max_windows_per_column: Option<FloatOrInt<1, 100>>,
    #[knuffel(child, unwrap(argument))]
    pub gaps: Option<FloatOrInt<0, 65535>>,
    #[knuffel(child)]
    pub struts: Option<Struts>,
    #[knuffel(child)]
    pub background_color: Option<Color>,
    #[knuffel(child)]
    pub dim_inactive: Option<Flag>,
    #[knuffel(child, unwrap(argument))]
    pub dim_factor: Option<f32>,
    #[knuffel(child, unwrap(argument))]
    pub rounding: Option<FloatOrInt<0, 65535>>,
    #[knuffel(child, unwrap(argument))]
    pub rounding_power: Option<FloatOrInt<2, 10>>,
}

impl Layout {
    pub fn resolve_clip_to_geometry(&self, rule: Option<bool>) -> bool {
        rule.unwrap_or(self.rounding > 0.)
    }

    pub fn resolve_window_corner_radius(
        &self,
        rule_radius: Option<CornerRadius>,
        rule_clip: Option<bool>,
        windowed_fullscreen: bool,
    ) -> CornerRadius {
        if windowed_fullscreen && !self.resolve_clip_to_geometry(rule_clip) {
            return CornerRadius::default();
        }
        match rule_radius {
            Some(radius) => radius,
            None if self.rounding > 0. => CornerRadius::from(self.rounding as f32),
            None => CornerRadius::default(),
        }
    }

    pub fn rounding_power_f32(&self) -> f32 {
        self.rounding_power.max(2.) as f32
    }
}

#[derive(knuffel::Decode, Debug, Clone, Copy, PartialEq)]
pub enum PresetSize {
    Proportion(#[knuffel(argument)] f64),
    Fixed(#[knuffel(argument)] i32),
}

impl From<PresetSize> for SizeChange {
    fn from(value: PresetSize) -> Self {
        match value {
            PresetSize::Proportion(prop) => SizeChange::SetProportion(prop * 100.),
            PresetSize::Fixed(fixed) => SizeChange::SetFixed(fixed),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DefaultPresetSize(pub Option<PresetSize>);

#[derive(knuffel::Decode, Debug, Default, Clone, Copy, PartialEq)]
pub struct Struts {
    #[knuffel(child, unwrap(argument), default)]
    pub left: FloatOrInt<-65535, 65535>,
    #[knuffel(child, unwrap(argument), default)]
    pub right: FloatOrInt<-65535, 65535>,
    #[knuffel(child, unwrap(argument), default)]
    pub top: FloatOrInt<-65535, 65535>,
    #[knuffel(child, unwrap(argument), default)]
    pub bottom: FloatOrInt<-65535, 65535>,
}

#[derive(knuffel::DecodeScalar, Debug, Default, PartialEq, Eq, Clone, Copy)]
pub enum CenterFocusedColumn {
    /// Focusing a column will not center the column.
    #[default]
    Never,
    /// The focused column will always be centered.
    Always,
    /// Focusing a column will center it if it doesn't fit on the screen together with the
    /// previously focused column.
    OnOverflow,
}

impl<S> knuffel::Decode<S> for DefaultPresetSize
where
    S: knuffel::traits::ErrorSpan,
{
    fn decode_node(
        node: &knuffel::ast::SpannedNode<S>,
        ctx: &mut knuffel::decode::Context<S>,
    ) -> Result<Self, DecodeError<S>> {
        expect_only_children(node, ctx);

        let mut children = node.children();

        if let Some(child) = children.next() {
            if let Some(unwanted_child) = children.next() {
                ctx.emit_error(DecodeError::unexpected(
                    unwanted_child,
                    "node",
                    "expected no more than one child",
                ));
            }
            PresetSize::decode_node(child, ctx).map(Some).map(Self)
        } else {
            Ok(Self(None))
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::Config;
    use std::path::Path;

    #[test]
    fn parse_hyprland_rounding() {
        let config = Config::parse(
            Path::new("test"),
            r#"layout { rounding 12; rounding-power 4; }"#,
        )
        .config
        .unwrap();
        assert_eq!(config.layout.rounding, 12.);
        assert_eq!(config.layout.rounding_power, 4.);
    }

    #[test]
    fn layout_rounding_fills_in_window_geometry() {
        let layout = crate::Layout {
            rounding: 12.,
            rounding_power: 4.,
            ..Default::default()
        };
        assert!(layout.resolve_clip_to_geometry(None));
        assert!(!layout.resolve_clip_to_geometry(Some(false)));
        assert_eq!(
            layout.resolve_window_corner_radius(None, None, false),
            crate::CornerRadius::from(12.)
        );
        assert_eq!(
            layout.resolve_window_corner_radius(None, None, true),
            crate::CornerRadius::from(12.)
        );
        assert_eq!(
            layout.resolve_window_corner_radius(None, Some(false), true),
            crate::CornerRadius::default()
        );
        let rule = crate::CornerRadius::from(3.);
        assert_eq!(
            layout.resolve_window_corner_radius(Some(rule), None, false),
            rule
        );
        assert_eq!(layout.rounding_power_f32(), 4.);
    }
}
