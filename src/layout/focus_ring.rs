use std::iter::zip;
use std::time::Duration;

use clarion_config::{CornerRadius, Gradient, GradientRelativeTo};
use smithay::backend::renderer::element::{Element as _, Kind};
use smithay::utils::{Logical, Point, Rectangle, Size};

use crate::clarion_render_elements;
use crate::render_helpers::border::BorderRenderElement;
use crate::render_helpers::renderer::DecorationRenderer;
use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};

#[derive(Debug)]
pub struct FocusRing {
    buffers: [SolidColorBuffer; 8],
    locations: [Point<f64, Logical>; 8],
    sizes: [Size<f64, Logical>; 8],
    borders: [BorderRenderElement; 8],
    full_size: Size<f64, Logical>,
    is_border: bool,
    use_border_shader: bool,
    config: clarion_config::FocusRing,
    thicken_corners: bool,
    rounding_power: f32,
}

clarion_render_elements! {
    FocusRingRenderElement => {
        SolidColor = SolidColorRenderElement,
        Gradient = BorderRenderElement,
    }
}

impl FocusRing {
    pub fn new(config: clarion_config::FocusRing) -> Self {
        Self {
            buffers: Default::default(),
            locations: Default::default(),
            sizes: Default::default(),
            borders: Default::default(),
            full_size: Default::default(),
            is_border: false,
            use_border_shader: false,
            config,
            thicken_corners: true,
            rounding_power: 2.,
        }
    }

    pub fn set_rounding_power(&mut self, power: f32) {
        let power = power.max(2.);
        if self.rounding_power == power {
            return;
        }
        self.rounding_power = power;
        for border in &mut self.borders {
            border.set_rounding_power(power);
        }
    }

    pub fn update_config(&mut self, config: clarion_config::FocusRing) {
        self.config = config;
    }

    pub fn update_shaders(&mut self) {
        for elem in &mut self.borders {
            elem.damage_all();
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_render_elements(
        &mut self,
        win_size: Size<f64, Logical>,
        is_active: bool,
        is_border: bool,
        is_urgent: bool,
        view_rect: Rectangle<f64, Logical>,
        radius: CornerRadius,
        scale: f64,
        alpha: f32,
        current_time: Option<Duration>,
    ) {
        let width = self.config.width;
        self.full_size = win_size + Size::from((width, width)).upscale(2.);
        self.is_border = is_border;

        let color = if is_urgent {
            self.config.urgent_color
        } else if is_active {
            self.config.active_color
        } else {
            self.config.inactive_color
        };

        for buf in &mut self.buffers {
            buf.set_color(color);
        }

        let radius = radius.fit_to(self.full_size.w as f32, self.full_size.h as f32);

        let gradient = if is_urgent {
            self.config.urgent_gradient
        } else if is_active {
            self.config.active_gradient
        } else {
            self.config.inactive_gradient
        };

        self.use_border_shader = radius != CornerRadius::default() || gradient.is_some();

        // Set the defaults for solid color + rounded corners.
        let gradient = gradient.unwrap_or_else(|| Gradient::from(color));

        let angle = match (gradient.rotate_deg_per_sec, current_time) {
            (Some(speed), Some(t)) => {
                let speed_f64 = speed.0;
                let elapsed_secs = t.as_secs_f64();
                let dynamic_angle =
                    (gradient.angle as f64 + elapsed_secs * speed_f64).rem_euclid(360.0);
                ((dynamic_angle as f32) - 90.).to_radians()
            }
            _ => ((gradient.angle as f32) - 90.).to_radians(),
        };

        let full_rect = Rectangle::new(Point::from((-width, -width)), self.full_size);
        let gradient_area = match gradient.relative_to {
            GradientRelativeTo::Window => full_rect,
            GradientRelativeTo::WorkspaceView => view_rect,
        };

        let rounded_corner_border_width = if is_border {
            // HACK: increase the border width used for the inner rounded corners a tiny bit to
            // reduce background bleed.
            let extra = if self.thicken_corners { 0.5 } else { 0. };
            width as f32 + extra
        } else {
            0.
        };

        let ceil = |logical: f64| (logical * scale).ceil() / scale;

        // All of this stuff should end up aligned to physical pixels because:
        // * Window size and border width are rounded to physical pixels before being passed to this
        //   function.
        // * We will ceil the corner radii below.
        // * We do not divide anything, only add, subtract and multiply by integers.
        // * At rendering time, tile positions are rounded to physical pixels.

        if is_border {
            let top_left = f64::max(width, ceil(f64::from(radius.top_left)));
            let top_right = f64::min(
                self.full_size.w - top_left,
                f64::max(width, ceil(f64::from(radius.top_right))),
            );
            let bottom_left = f64::min(
                self.full_size.h - top_left,
                f64::max(width, ceil(f64::from(radius.bottom_left))),
            );
            let bottom_right = f64::min(
                self.full_size.h - top_right,
                f64::min(
                    self.full_size.w - bottom_left,
                    f64::max(width, ceil(f64::from(radius.bottom_right))),
                ),
            );

            // Top edge.
            self.sizes[0] = Size::from((win_size.w + width * 2. - top_left - top_right, width));
            self.locations[0] = Point::from((-width + top_left, -width));

            // Bottom edge.
            self.sizes[1] =
                Size::from((win_size.w + width * 2. - bottom_left - bottom_right, width));
            self.locations[1] = Point::from((-width + bottom_left, win_size.h));

            // Left edge.
            self.sizes[2] = Size::from((width, win_size.h + width * 2. - top_left - bottom_left));
            self.locations[2] = Point::from((-width, -width + top_left));

            // Right edge.
            self.sizes[3] = Size::from((width, win_size.h + width * 2. - top_right - bottom_right));
            self.locations[3] = Point::from((win_size.w, -width + top_right));

            // Top-left corner.
            self.sizes[4] = Size::from((top_left, top_left));
            self.locations[4] = Point::from((-width, -width));

            // Top-right corner.
            self.sizes[5] = Size::from((top_right, top_right));
            self.locations[5] = Point::from((win_size.w + width - top_right, -width));

            // Bottom-right corner.
            self.sizes[6] = Size::from((bottom_right, bottom_right));
            self.locations[6] = Point::from((
                win_size.w + width - bottom_right,
                win_size.h + width - bottom_right,
            ));

            // Bottom-left corner.
            self.sizes[7] = Size::from((bottom_left, bottom_left));
            self.locations[7] = Point::from((-width, win_size.h + width - bottom_left));

            for (buf, size) in zip(&mut self.buffers, self.sizes) {
                buf.resize(size);
            }

            for (border, (loc, size)) in zip(&mut self.borders, zip(self.locations, self.sizes)) {
                border.update(
                    size,
                    Rectangle::new(gradient_area.loc - loc, gradient_area.size),
                    gradient.in_,
                    gradient.from,
                    gradient.to,
                    angle,
                    Rectangle::new(full_rect.loc - loc, full_rect.size),
                    rounded_corner_border_width,
                    radius,
                    scale as f32,
                    alpha,
                );
            }
        } else {
            self.sizes[0] = self.full_size;
            self.buffers[0].resize(self.sizes[0]);
            self.locations[0] = Point::from((-width, -width));

            self.borders[0].update(
                self.sizes[0],
                Rectangle::new(gradient_area.loc - self.locations[0], gradient_area.size),
                gradient.in_,
                gradient.from,
                gradient.to,
                angle,
                Rectangle::new(full_rect.loc - self.locations[0], full_rect.size),
                rounded_corner_border_width,
                radius,
                scale as f32,
                alpha,
            );
        }
    }

    pub fn render(
        &self,
        renderer: &mut impl DecorationRenderer,
        location: Point<f64, Logical>,
        push: &mut dyn FnMut(FocusRingRenderElement),
    ) {
        if self.config.off {
            return;
        }

        let border_width = -self.locations[0].y;

        // If drawing as a border with width = 0, then there's nothing to draw.
        if self.is_border && border_width == 0. {
            return;
        }

        let has_border_shader = BorderRenderElement::has_shader(renderer);

        let mut push = |buffer, border: &BorderRenderElement, location: Point<f64, Logical>| {
            let elem = if self.use_border_shader && has_border_shader {
                border.clone().with_location(location).into()
            } else {
                let alpha = border.alpha();
                SolidColorRenderElement::from_buffer(buffer, location, alpha, Kind::Unspecified)
                    .into()
            };
            push(elem);
        };

        if self.is_border {
            for ((buf, border), loc) in zip(zip(&self.buffers, &self.borders), self.locations) {
                push(buf, border, location + loc);
            }
        } else {
            push(
                &self.buffers[0],
                &self.borders[0],
                location + self.locations[0],
            );
        }
    }

    pub fn width(&self) -> f64 {
        self.config.width
    }

    pub fn is_off(&self) -> bool {
        self.config.off
    }

    pub fn has_animated_gradient(&self, is_active: bool, is_urgent: bool) -> bool {
        if self.is_off() {
            return false;
        }
        let gradient = if is_urgent {
            self.config.urgent_gradient
        } else if is_active {
            self.config.active_gradient
        } else {
            self.config.inactive_gradient
        };
        self.width() > 0.
            && gradient.is_some_and(|g| g.rotate_deg_per_sec.is_some_and(|speed| speed.0 != 0.))
    }

    pub fn set_thicken_corners(&mut self, value: bool) {
        self.thicken_corners = value;
    }

    pub fn config(&self) -> &clarion_config::FocusRing {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clarion_config::{Color, FloatOrInt, Gradient, GradientInterpolation, GradientRelativeTo};

    #[test]
    fn test_animated_gradient_detection() {
        let config = clarion_config::FocusRing {
            off: false,
            active_gradient: Some(Gradient {
                from: Color::from_rgba8_unpremul(255, 0, 0, 255),
                to: Color::from_rgba8_unpremul(0, 0, 255, 255),
                angle: 0,
                rotate_deg_per_sec: Some(FloatOrInt(180.0)),
                relative_to: GradientRelativeTo::Window,
                in_: GradientInterpolation::default(),
            }),
            ..Default::default()
        };

        let ring = FocusRing::new(config);
        assert!(ring.has_animated_gradient(true, false));
        assert!(!ring.has_animated_gradient(false, false));
    }

    #[test]
    fn static_or_invisible_gradients_do_not_request_frames() {
        let mut config = clarion_config::FocusRing::default();
        let mut gradient = Gradient::from(config.active_color);
        gradient.rotate_deg_per_sec = Some(FloatOrInt(0.));
        config.active_gradient = Some(gradient);
        assert!(!FocusRing::new(config).has_animated_gradient(true, false));
        config.active_gradient.as_mut().unwrap().rotate_deg_per_sec = Some(FloatOrInt(-90.));
        assert!(FocusRing::new(config).has_animated_gradient(true, false));
        config.width = 0.;
        assert!(!FocusRing::new(config).has_animated_gradient(true, false));
    }

    #[test]
    fn test_dynamic_angle_calculation() {
        let initial_angle: i16 = 45;
        let speed = 90.0; // 90 deg / sec
        let elapsed = Duration::from_millis(1500); // 1.5 sec -> +135 deg -> 180 deg

        let speed_f64 = speed;
        let elapsed_secs = elapsed.as_secs_f64();
        let dynamic_angle = (initial_angle as f64 + elapsed_secs * speed_f64).rem_euclid(360.0);
        assert!((dynamic_angle - 180.0).abs() < 1e-6);
    }
}
