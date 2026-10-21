//! Independent double-precision image-space contracts for the wiki effects.
#[derive(Clone, Copy, Debug)]
pub(super) enum Effect {
    OpenGradient,
    OpenScale,
    OpenCircle,
    CloseGradient,
    CloseScale,
    CloseFall,
}
impl Effect {
    pub(super) const ALL: [Self; 6] = [
        Self::OpenGradient,
        Self::OpenScale,
        Self::OpenCircle,
        Self::CloseGradient,
        Self::CloseScale,
        Self::CloseFall,
    ];
    pub(super) fn opening(self) -> bool {
        matches!(
            self,
            Self::OpenGradient | Self::OpenScale | Self::OpenCircle
        )
    }
    pub(super) fn entry(self) -> &'static str {
        match self {
            Self::OpenGradient | Self::CloseGradient => "solid_gradient",
            Self::OpenScale => "default_open",
            Self::OpenCircle => "expanding_circle",
            Self::CloseScale => "default_close",
            Self::CloseFall => "fall_and_rotate",
        }
    }
}
fn inside(point: [f64; 2]) -> bool {
    point.into_iter().all(|x| (0. ..=1.).contains(&x))
}
pub(super) fn image() -> Vec<u8> {
    (0..8)
        .flat_map(|y| (0..8).flat_map(move |x| [128 - x * 8, y * 8, 16, 128]))
        .collect()
}
fn sample(image: &[u8], uv: [f64; 2]) -> [f64; 4] {
    if !inside(uv) {
        return [0.; 4];
    }
    let p = uv.map(|x| x * 8. - 0.5);
    let start = p.map(f64::floor);
    let fraction = [p[0] - start[0], p[1] - start[1]];
    let mut out = [0.; 4];
    for y in 0..2 {
        for x in 0..2 {
            let ix = start[0] as i32 + x;
            let iy = start[1] as i32 + y;
            if !(0..8).contains(&ix) || !(0..8).contains(&iy) {
                continue;
            }
            let weight = (if x == 0 {
                1. - fraction[0]
            } else {
                fraction[0]
            }) * (if y == 0 {
                1. - fraction[1]
            } else {
                fraction[1]
            });
            for (channel, out) in out.iter_mut().enumerate() {
                *out += f64::from(image[((iy * 8 + ix) * 4) as usize + channel]) * weight;
            }
        }
    }
    out
}
pub(super) fn pixel(
    effect: Effect,
    point: [f64; 2],
    size: [f64; 2],
    progress: f64,
    clamped: f64,
    seed: f64,
    image: &[u8],
) -> [u8; 4] {
    let opacity = if effect.opening() {
        clamped
    } else {
        1. - clamped
    };
    let sampled = match effect {
        Effect::OpenGradient | Effect::CloseGradient => {
            if opacity > 0. && inside(point) {
                [
                    (1. - point[1]) * 255. * opacity,
                    point[1] * 255. * opacity,
                    0.,
                    255. * opacity,
                ]
            } else {
                [0.; 4]
            }
        }
        Effect::OpenScale | Effect::CloseScale => {
            let scale = if effect.opening() {
                0.5 + progress / 2.
            } else {
                0.8 + opacity / 5.
            };
            if opacity > 0. && scale > 0. {
                sample(image, point.map(|x| (x - 0.5) / scale + 0.5)).map(|x| x * opacity)
            } else {
                [0.; 4]
            }
        }
        Effect::OpenCircle => {
            let distance = ((point[0] - 0.5) * size[0]).hypot((point[1] - 0.5) * size[1]);
            let limit = clamped * size[0].hypot(size[1]) / 2.;
            if clamped > 0. && size.into_iter().all(|axis| axis > 0.) && distance < limit {
                sample(image, point)
            } else {
                [0.; 4]
            }
        }
        Effect::CloseFall => {
            if opacity <= 0. || size.into_iter().any(|axis| axis <= 0.) {
                [0.; 4]
            } else {
                let travel = clamped.powi(2);
                let x = (point[0] - 0.5) * size[0];
                let y = (point[1] - 1.) * size[1] - 200. * travel;
                let raw = (seed - 0.5) / 2.;
                let direction = (if raw < 0. {
                    -1.
                } else if raw > 0. {
                    1.
                } else {
                    0.
                }) - raw;
                let (sine, cosine) = (travel * direction * 0.05).sin_cos();
                let uv = [
                    (cosine * x + sine * y) / size[0] + 0.5,
                    (-sine * x + cosine * y) / size[1] + 1.,
                ];
                sample(image, uv).map(|x| x * opacity)
            }
        }
    };
    sampled.map(|x| (x * 0.5).round().clamp(0., 255.) as u8)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absent_and_zero_opacity_domains_never_produce_color() {
        let image = image();
        for effect in Effect::ALL {
            assert_eq!(
                pixel(effect, [f64::NAN, 0.5], [256., 192.], 0.5, 0.5, 0.5, &image),
                [0; 4]
            );
            if !matches!(effect, Effect::OpenCircle) {
                let end = if effect.opening() { 0. } else { 1. };
                assert_eq!(
                    pixel(effect, [0.5, 0.5], [256., 192.], end, end, 0.5, &image),
                    [0; 4]
                );
            }
        }
        assert_eq!(
            pixel(
                Effect::OpenScale,
                [0.5, 0.5],
                [256., 192.],
                -1.,
                0.5,
                0.5,
                &image
            ),
            [0; 4]
        );
    }
    #[test]
    fn circle_coverage_is_geometry_aspect_aware_and_stays_premultiplied() {
        let image = image();
        assert_eq!(
            pixel(
                Effect::OpenCircle,
                [0.9, 0.5],
                [256., 192.],
                0.5,
                0.5,
                0.5,
                &image
            ),
            [0; 4]
        );
        assert_ne!(
            pixel(
                Effect::OpenCircle,
                [0.5, 0.9],
                [256., 192.],
                0.5,
                0.5,
                0.5,
                &image
            ),
            [0; 4]
        );
        for effect in Effect::ALL {
            for x in 0..8 {
                for y in 0..8 {
                    let p = pixel(
                        effect,
                        [(f64::from(x) + 0.5) / 8., (f64::from(y) + 0.5) / 8.],
                        [256., 192.],
                        0.5,
                        0.5,
                        0.1,
                        &image,
                    );
                    assert!(p[..3].iter().all(|c| *c <= p[3]));
                }
            }
        }
    }
    #[test]
    fn overshoot_scale_exact_left_and_right_texture_boundaries_keep_border_coverage() {
        let image = image();
        assert_eq!(
            pixel(
                Effect::OpenScale,
                [-0.0625, 0.0625],
                [256., 192.],
                1.25,
                1.,
                0.1,
                &image
            ),
            [32, 1, 4, 32]
        );
        assert_eq!(
            pixel(
                Effect::OpenScale,
                [1.0625, 0.0625],
                [256., 192.],
                1.25,
                1.,
                0.1,
                &image
            ),
            [18, 1, 4, 32]
        );
    }
}
