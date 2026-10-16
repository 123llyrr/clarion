use std::iter::zip;
use std::mem;

use clarion_config::{CornerRadius, Gradient, GradientRelativeTo, TabIndicatorPosition};
use smithay::utils::{Logical, Point, Rectangle, Size};

use super::tile::Tile;
use super::LayoutElement;
use crate::animation::{Animation, Clock};
use crate::clarion_render_elements;
use crate::render_helpers::border::BorderRenderElement;
use crate::render_helpers::renderer::DecorationRenderer;
use crate::utils::{
    floor_logical_in_physical_max1, round_logical_in_physical, round_logical_in_physical_max1,
};

#[derive(Debug)]
pub struct TabIndicator {
    shader_locs: Vec<Point<f64, Logical>>,
    shaders: Vec<BorderRenderElement>,
    open_anim: Option<Animation>,
    config: clarion_config::TabIndicator,
}

#[derive(Debug)]
pub struct TabInfo {
    /// Gradient for the tab indicator.
    pub gradient: Gradient,
    /// Tab geometry in the same coordinate system as the area.
    pub geometry: Rectangle<f64, Logical>,
}

clarion_render_elements! {
    TabIndicatorRenderElement => {
        Gradient = BorderRenderElement,
    }
}

impl TabIndicator {
    pub fn new(config: clarion_config::TabIndicator) -> Self {
        Self {
            shader_locs: Vec::new(),
            shaders: Vec::new(),
            open_anim: None,
            config,
        }
    }

    pub fn update_config(&mut self, config: clarion_config::TabIndicator) {
        self.config = config;
    }

    pub fn update_shaders(&mut self) {
        for elem in &mut self.shaders {
            elem.damage_all();
        }
    }

    pub fn advance_animations(&mut self) {
        if let Some(anim) = &mut self.open_anim {
            if anim.is_done() {
                self.open_anim = None;
            }
        }
    }

    pub fn are_animations_ongoing(&self) -> bool {
        self.open_anim.is_some()
    }

    pub fn start_open_animation(&mut self, clock: Clock, config: clarion_config::Animation) {
        if self.config.off {
            return;
        }

        // A new window can report its opening after its column already started
        // revealing the indicator. Continue that reveal rather than shrinking it
        // back to zero when the window's animation configuration arrives.
        let (from, velocity) = if let Some(anim) = &self.open_anim {
            let value = anim.value();
            let velocity = anim.velocity();
            // tab_rects() hides negative progress. Continue the rendered motion,
            // excluding velocity while below zero or moving into the hidden range.
            let velocity = if value < 0. || (value == 0. && velocity < 0.) {
                0.
            } else {
                velocity
            };
            (value.max(0.), velocity)
        } else if self.shaders.is_empty() {
            (0., 0.)
        } else {
            // Hidden indicators clear their elements during the render update.
            // Existing elements without an animation are already fully revealed.
            (1., 0.)
        };
        if from == 1. && velocity == 0. {
            self.open_anim = None;
            return;
        }
        self.open_anim = Some(Animation::new(clock, from, 1., velocity, config));
    }

    fn tab_rects(
        &self,
        area: Rectangle<f64, Logical>,
        count: usize,
        scale: f64,
    ) -> impl Iterator<Item = Rectangle<f64, Logical>> {
        let round = |logical: f64| round_logical_in_physical(scale, logical);
        let round_max1 = |logical: f64| round_logical_in_physical_max1(scale, logical);

        let progress = self.open_anim.as_ref().map_or(1., |a| a.value().max(0.));

        let width = round_max1(self.config.width);
        let gap = self.config.gap;
        let gap = round_max1(gap.abs()).copysign(gap);
        let gaps_between = round_max1(self.config.gaps_between_tabs);

        let position = self.config.position;
        let side = match position {
            TabIndicatorPosition::Left | TabIndicatorPosition::Right => area.size.h,
            TabIndicatorPosition::Top | TabIndicatorPosition::Bottom => area.size.w,
        };
        let total_prop = self.config.length.total_proportion.unwrap_or(0.5);
        let min_length = round(side * total_prop.clamp(0., 2.));

        // Compute px_per_tab before applying the animation to gaps_between in order to avoid it
        // growing and shrinking over the duration of the animation.
        let pixel = 1. / scale;
        let shortest_length = count as f64 * (pixel + gaps_between) - gaps_between;
        let length = f64::max(min_length, shortest_length);
        let px_per_tab = (length + gaps_between) / count as f64 - gaps_between;

        let px_per_tab = px_per_tab * progress;
        let gaps_between = round(self.config.gaps_between_tabs * progress);

        let length = count as f64 * (px_per_tab + gaps_between) - gaps_between;
        let px_per_tab = floor_logical_in_physical_max1(scale, px_per_tab);
        let floored_length = count as f64 * (px_per_tab + gaps_between) - gaps_between;
        let mut ones_left = ((length - floored_length) / pixel).round() as usize;

        let mut shader_loc = Point::from((-gap - width, round((side - length) / 2.)));
        match position {
            TabIndicatorPosition::Left => (),
            TabIndicatorPosition::Right => shader_loc.x = area.size.w + gap,
            TabIndicatorPosition::Top => mem::swap(&mut shader_loc.x, &mut shader_loc.y),
            TabIndicatorPosition::Bottom => {
                shader_loc.x = shader_loc.y;
                shader_loc.y = area.size.h + gap;
            }
        }
        shader_loc += area.loc;

        (0..count).map(move |_| {
            let mut px_per_tab = px_per_tab;
            if ones_left > 0 {
                ones_left -= 1;
                px_per_tab += pixel;
            }

            let loc = shader_loc;

            match position {
                TabIndicatorPosition::Left | TabIndicatorPosition::Right => {
                    shader_loc.y += px_per_tab + gaps_between
                }
                TabIndicatorPosition::Top | TabIndicatorPosition::Bottom => {
                    shader_loc.x += px_per_tab + gaps_between
                }
            }

            let size = match position {
                TabIndicatorPosition::Left | TabIndicatorPosition::Right => {
                    Size::from((width, px_per_tab))
                }
                TabIndicatorPosition::Top | TabIndicatorPosition::Bottom => {
                    Size::from((px_per_tab, width))
                }
            };

            Rectangle::new(loc, size)
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_render_elements(
        &mut self,
        enabled: bool,
        // Geometry of the tabs area.
        area: Rectangle<f64, Logical>,
        // View rect relative to the tabs area.
        area_view_rect: Rectangle<f64, Logical>,
        // Tab count, should match the tabs iterator length.
        tab_count: usize,
        tabs: impl Iterator<Item = TabInfo>,
        is_active: bool,
        scale: f64,
    ) {
        let count = tab_count;
        if !enabled
            || self.config.off
            || count == 0
            || (self.config.hide_when_single_tab && count == 1)
        {
            self.shader_locs.clear();
            self.shaders.clear();
            self.open_anim = None;
            return;
        }

        self.shaders.resize_with(count, Default::default);
        self.shader_locs.resize_with(count, Default::default);

        let position = self.config.position;
        let radius = self.config.corner_radius as f32;
        let shared_rounded_corners = self.config.gaps_between_tabs == 0.;
        let mut tabs_left = tab_count;

        let rects = self.tab_rects(area, count, scale);
        for ((shader, loc), (tab, rect)) in zip(
            zip(&mut self.shaders, &mut self.shader_locs),
            zip(tabs, rects),
        ) {
            *loc = rect.loc;

            let mut gradient_area = match tab.gradient.relative_to {
                GradientRelativeTo::Window => tab.geometry,
                GradientRelativeTo::WorkspaceView => area_view_rect,
            };
            gradient_area.loc -= *loc;

            let mut color_from = tab.gradient.from;
            let mut color_to = tab.gradient.to;
            if !is_active {
                color_from *= 0.5;
                color_to *= 0.5;
            }

            let radius = if shared_rounded_corners && tab_count > 1 {
                if tabs_left == tab_count {
                    // First tab.
                    match position {
                        TabIndicatorPosition::Left | TabIndicatorPosition::Right => CornerRadius {
                            top_left: radius,
                            top_right: radius,
                            bottom_right: 0.,
                            bottom_left: 0.,
                        },
                        TabIndicatorPosition::Top | TabIndicatorPosition::Bottom => CornerRadius {
                            top_left: radius,
                            top_right: 0.,
                            bottom_right: 0.,
                            bottom_left: radius,
                        },
                    }
                } else if tabs_left == 1 {
                    // Last tab.
                    match position {
                        TabIndicatorPosition::Left | TabIndicatorPosition::Right => CornerRadius {
                            top_left: 0.,
                            top_right: 0.,
                            bottom_right: radius,
                            bottom_left: radius,
                        },
                        TabIndicatorPosition::Top | TabIndicatorPosition::Bottom => CornerRadius {
                            top_left: 0.,
                            top_right: radius,
                            bottom_right: radius,
                            bottom_left: 0.,
                        },
                    }
                } else {
                    // Tab in the middle.
                    CornerRadius::default()
                }
            } else {
                // Separate tabs, or the only tab.
                CornerRadius::from(radius)
            };
            let radius = radius.fit_to(rect.size.w as f32, rect.size.h as f32);
            tabs_left -= 1;

            shader.update(
                rect.size,
                gradient_area,
                tab.gradient.in_,
                color_from,
                color_to,
                ((tab.gradient.angle as f32) - 90.).to_radians(),
                Rectangle::from_size(rect.size),
                0.,
                radius,
                scale as f32,
                1.,
            );
        }
    }

    pub fn hit(
        &self,
        area: Rectangle<f64, Logical>,
        tab_count: usize,
        scale: f64,
        point: Point<f64, Logical>,
    ) -> Option<usize> {
        if self.config.off {
            return None;
        }

        let count = tab_count;
        if self.config.hide_when_single_tab && count == 1 {
            return None;
        }

        self.tab_rects(area, count, scale)
            .enumerate()
            .find_map(|(idx, rect)| rect.contains(point).then_some(idx))
    }

    pub fn render(
        &self,
        renderer: &mut impl DecorationRenderer,
        pos: Point<f64, Logical>,
        push: &mut dyn FnMut(TabIndicatorRenderElement),
    ) {
        let has_border_shader = BorderRenderElement::has_shader(renderer);
        if !has_border_shader {
            return;
        }

        for (shader, loc) in zip(&self.shaders, &self.shader_locs) {
            let elem = shader.clone().with_location(pos + *loc);
            push(TabIndicatorRenderElement::from(elem));
        }
    }

    /// Extra size occupied by the tab indicator.
    pub fn extra_size(&self, tab_count: usize, scale: f64) -> Size<f64, Logical> {
        if self.config.off
            || !self.config.place_within_column
            || (self.config.hide_when_single_tab && tab_count == 1)
        {
            return Size::from((0., 0.));
        }

        let round = |logical: f64| round_logical_in_physical(scale, logical);
        let width = round(self.config.width);
        let gap = round(self.config.gap);

        // No, I am *not* falling into the rabbit hole of "what if the tab indicator is wide enough
        // that it peeks from the other side of the window".
        let size = f64::max(0., width + gap);

        match self.config.position {
            TabIndicatorPosition::Left | TabIndicatorPosition::Right => Size::from((size, 0.)),
            TabIndicatorPosition::Top | TabIndicatorPosition::Bottom => Size::from((0., size)),
        }
    }

    /// Offset of the tabbed content due to space occupied by the tab indicator.
    pub fn content_offset(&self, tab_count: usize, scale: f64) -> Point<f64, Logical> {
        match self.config.position {
            TabIndicatorPosition::Left | TabIndicatorPosition::Top => {
                self.extra_size(tab_count, scale).to_point()
            }
            TabIndicatorPosition::Right | TabIndicatorPosition::Bottom => Point::from((0., 0.)),
        }
    }

    pub fn config(&self) -> clarion_config::TabIndicator {
        self.config
    }
}

impl TabInfo {
    pub fn from_tile<W: LayoutElement>(
        tile: &Tile<W>,
        position: Point<f64, Logical>,
        is_active: bool,
        is_urgent: bool,
        config: &clarion_config::TabIndicator,
    ) -> Self {
        let rules = tile.window().rules();
        let rule = rules.tab_indicator;

        let gradient_from_rule = || {
            let (color, gradient) = if is_urgent {
                (rule.urgent_color, rule.urgent_gradient)
            } else if is_active {
                (rule.active_color, rule.active_gradient)
            } else {
                (rule.inactive_color, rule.inactive_gradient)
            };
            let color = color.map(Gradient::from);
            gradient.or(color)
        };

        let gradient_from_config = || {
            let (color, gradient) = if is_urgent {
                (config.urgent_color, config.urgent_gradient)
            } else if is_active {
                (config.active_color, config.active_gradient)
            } else {
                (config.inactive_color, config.inactive_gradient)
            };
            let color = color.map(Gradient::from);
            gradient.or(color)
        };

        let gradient_from_border = || {
            // Come up with tab indicator gradient matching the focus ring or the border, whichever
            // one is enabled.
            let focus_ring_config = tile.focus_ring().config();
            let border_config = tile.border().config();
            let config = if focus_ring_config.off {
                border_config
            } else {
                focus_ring_config
            };

            let (color, gradient) = if is_urgent {
                (config.urgent_color, config.urgent_gradient)
            } else if is_active {
                (config.active_color, config.active_gradient)
            } else {
                (config.inactive_color, config.inactive_gradient)
            };
            gradient.unwrap_or_else(|| Gradient::from(color))
        };

        let gradient = gradient_from_rule()
            .or_else(gradient_from_config)
            .unwrap_or_else(gradient_from_border);

        let geometry = Rectangle::new(position, tile.animated_tile_size());

        TabInfo { gradient, geometry }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use smithay::backend::renderer::element::Element;

    use super::*;

    fn update(indicator: &mut TabIndicator, enabled: bool, count: usize) {
        let area = Rectangle::from_size(Size::new(800., 600.));
        let tabs = (0..count).map(|_| TabInfo {
            gradient: Gradient::from(clarion_config::Color::new_unpremul(1., 1., 1., 1.)),
            geometry: area,
        });
        indicator.update_render_elements(enabled, area, area, count, tabs, true, 1.);
    }

    #[test]
    fn hidden_indicator_stops_animation_and_releases_elements() {
        for hidden_by in ["disabled", "off", "single", "empty"] {
            let mut clock = Clock::with_time(Duration::ZERO);
            let mut indicator = TabIndicator::new(Default::default());
            let animation = clarion_config::animations::WindowMovementAnim::default().0;
            indicator.start_open_animation(clock.clone(), animation);
            clock.set_unadjusted(Duration::from_millis(50));
            update(&mut indicator, true, 2);
            assert_eq!(indicator.shaders.len(), 2);
            let mut config = indicator.config();
            config.off = hidden_by == "off";
            config.hide_when_single_tab = hidden_by == "single";
            indicator.update_config(config);
            let count = match hidden_by {
                "single" => 1,
                "empty" => 0,
                _ => 2,
            };
            update(&mut indicator, hidden_by != "disabled", count);
            assert!(indicator.shaders.is_empty());
            assert!(indicator.shader_locs.is_empty());
            assert!(
                !indicator.are_animations_ongoing(),
                "hidden by {hidden_by} still requests animation frames"
            );
        }
    }

    #[test]
    fn disabled_indicator_does_not_start_animation() {
        let config = clarion_config::TabIndicator {
            off: true,
            ..Default::default()
        };
        let mut indicator = TabIndicator::new(config);
        indicator.start_open_animation(
            Clock::with_time(Duration::ZERO),
            clarion_config::animations::WindowMovementAnim::default().0,
        );
        assert!(!indicator.are_animations_ongoing());
    }

    #[test]
    fn repeated_open_preserves_indicator_position_and_velocity() {
        for rate in [0.25, 0.73, 1., 2.] {
            let mut clock = Clock::with_time(Duration::ZERO);
            clock.set_rate(rate);
            let mut indicator = TabIndicator::new(Default::default());
            let config = clarion_config::animations::WindowMovementAnim::default().0;
            indicator.start_open_animation(clock.clone(), config);
            clock.set_unadjusted(Duration::from_millis(80));
            let before = indicator.open_anim.as_ref().unwrap();
            let value = before.value();
            let velocity = before.velocity();
            let area = Rectangle::from_size(Size::new(800., 600.));
            let rects: Vec<_> = indicator.tab_rects(area, 3, 1.25).collect();
            indicator.start_open_animation(clock.clone(), config);
            let after = indicator.open_anim.as_ref().unwrap();
            assert!((after.value() - value).abs() < 1e-10);
            assert!((after.velocity() - velocity).abs() < 1e-10);
            assert_eq!(
                indicator.tab_rects(area, 3, 1.25).collect::<Vec<_>>(),
                rects
            );
            clock.set_unadjusted(Duration::from_secs(10));
            indicator.advance_animations();
            assert!(!indicator.are_animations_ongoing());
        }
    }

    #[test]
    fn completed_or_pruned_open_does_not_restart_visible_indicator() {
        for (prune, rendered) in [(false, false), (false, true), (true, true)] {
            let mut clock = Clock::with_time(Duration::ZERO);
            let mut indicator = TabIndicator::new(Default::default());
            let config = clarion_config::animations::WindowOpenAnim::default().anim;
            indicator.start_open_animation(clock.clone(), config);
            clock.set_unadjusted(Duration::from_secs(10));
            if rendered {
                update(&mut indicator, true, 2);
            }
            if prune {
                indicator.advance_animations();
            }

            let area = Rectangle::from_size(Size::new(800., 600.));
            let rects: Vec<_> = indicator.tab_rects(area, 2, 1.).collect();
            indicator.start_open_animation(clock.clone(), config);
            assert!(!indicator.are_animations_ongoing());
            assert_eq!(indicator.tab_rects(area, 2, 1.).collect::<Vec<_>>(), rects);
        }
    }

    #[test]
    fn hidden_indicator_restarts_reveal_after_becoming_visible() {
        let clock = Clock::with_time(Duration::ZERO);
        let mut indicator = TabIndicator::new(Default::default());
        update(&mut indicator, true, 2);
        update(&mut indicator, false, 2);
        indicator.start_open_animation(
            clock,
            clarion_config::animations::WindowMovementAnim::default().0,
        );
        assert_eq!(indicator.open_anim.as_ref().unwrap().value(), 0.);
    }

    #[test]
    fn reopening_uses_velocity_of_visible_indicator_progress() {
        for (value, velocity, expected) in [
            (-0.25, -2., 0.),
            (-0.25, 2., 0.),
            (0., -2., 0.),
            (0., 2., 2.),
        ] {
            let clock = Clock::with_time(Duration::ZERO);
            let mut indicator = TabIndicator::new(Default::default());
            let config = clarion_config::animations::WindowMovementAnim::default().0;
            indicator.open_anim = Some(Animation::new(clock.clone(), value, 1., velocity, config));
            indicator.start_open_animation(clock, config);
            let anim = indicator.open_anim.as_ref().unwrap();
            assert_eq!(anim.value(), 0.);
            assert!((anim.velocity() - expected).abs() < 1e-10);
        }
    }

    #[test]
    fn unchanged_indicator_elements_keep_damage_commit() {
        let mut indicator = TabIndicator::new(Default::default());
        update(&mut indicator, true, 2);
        let commits: Vec<_> = indicator
            .shaders
            .iter()
            .map(Element::current_commit)
            .collect();
        let ids: Vec<_> = indicator
            .shaders
            .iter()
            .map(|elem| elem.id().clone())
            .collect();
        update(&mut indicator, true, 2);
        assert_eq!(
            indicator
                .shaders
                .iter()
                .map(Element::current_commit)
                .collect::<Vec<_>>(),
            commits
        );
        assert_eq!(
            indicator
                .shaders
                .iter()
                .map(|elem| elem.id().clone())
                .collect::<Vec<_>>(),
            ids
        );
    }
}
