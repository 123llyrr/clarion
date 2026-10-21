//! Independent pixel-space oracle for the five documented resize policies.
#[derive(Clone, Copy, Debug)]
pub(super) enum Policy {
    Fill,
    Crossfade,
    Stretch,
    PerAxisCrop,
    CropOrCrossfade,
}
impl Policy {
    pub(super) const ALL: [Self; 5] = [
        Self::Fill,
        Self::Crossfade,
        Self::Stretch,
        Self::PerAxisCrop,
        Self::CropOrCrossfade,
    ];
    pub(super) fn entry(self) -> &'static str {
        match self {
            Self::Fill => "solid_gradient",
            Self::Crossfade => "crossfade",
            Self::Stretch => "stretch_next",
            Self::PerAxisCrop => "stretch_or_crop_next",
            Self::CropOrCrossfade => "crossfade_or_crop_next",
        }
    }
}
fn inside(point: [f64; 2]) -> bool {
    point.into_iter().all(|x| (0. ..=1.).contains(&x))
}
pub(super) fn source(previous: bool) -> Vec<u8> {
    (0..8)
        .flat_map(|y| {
            (0..8).flat_map(move |x| {
                if previous {
                    [128 - x * 8, y * 8, 16, 128]
                } else {
                    [x * 24, y * 24, 40, 255]
                }
            })
        })
        .collect()
}
fn sample(image: &[u8], uv: [f64; 2]) -> [f64; 4] {
    if !inside(uv) {
        return [0.; 4];
    }
    let p = uv.map(|u| u * 8. - 0.5);
    let base = p.map(f64::floor);
    let phase = [p[0] - base[0], p[1] - base[1]];
    let mut out = [0.; 4];
    for dy in 0..2 {
        for dx in 0..2 {
            let x = base[0] as i32 + dx;
            let y = base[1] as i32 + dy;
            if !(0..8).contains(&x) || !(0..8).contains(&y) {
                continue;
            }
            let weight = (if dx == 0 { 1. - phase[0] } else { phase[0] })
                * (if dy == 0 { 1. - phase[1] } else { phase[1] });
            for (channel, out) in out.iter_mut().enumerate() {
                *out += f64::from(image[((y * 8 + x) * 4) as usize + channel]) * weight;
            }
        }
    }
    out
}
pub(super) fn pixel(
    policy: Policy,
    point: [f64; 2],
    ratio: [f64; 2],
    progress: f64,
    previous: &[u8],
    next: &[u8],
) -> [u8; 4] {
    let crop = [ratio[0] <= 1., ratio[1] <= 1.];
    let fade = || {
        let before = sample(previous, point);
        let after = sample(next, point);
        std::array::from_fn(|channel| before[channel] * (1. - progress) + after[channel] * progress)
    };
    let color = match policy {
        Policy::Fill => {
            if inside(point) {
                [(1. - point[1]) * 255., point[1] * 255., 0., 255.]
            } else {
                [0.; 4]
            }
        }
        Policy::Crossfade => fade(),
        Policy::Stretch => sample(next, point),
        Policy::PerAxisCrop => {
            if (crop[0] && !(0. ..=1.).contains(&point[0]))
                || (crop[1] && !(0. ..=1.).contains(&point[1]))
            {
                [0.; 4]
            } else {
                sample(
                    next,
                    std::array::from_fn(|axis| {
                        point[axis] * if crop[axis] { ratio[axis] } else { 1. }
                    }),
                )
            }
        }
        Policy::CropOrCrossfade => {
            if crop.into_iter().all(|axis| axis) {
                if inside(point) {
                    sample(next, [point[0] * ratio[0], point[1] * ratio[1]])
                } else {
                    [0.; 4]
                }
            } else {
                fade()
            }
        }
    };
    color.map(|value| (value * 0.5).round().clamp(0., 255.) as u8)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn per_axis_crop_uses_natural_pixels_only_on_shrinking_axes() {
        let image = source(false);
        let previous = source(true);
        let expected = sample(&image, [0.5 * 0.5, 0.5]).map(|x| (x * 0.5).round() as u8);
        assert_eq!(
            pixel(
                Policy::PerAxisCrop,
                [0.5, 0.5],
                [0.5, 2.],
                0.,
                &previous,
                &image
            ),
            expected
        );
        assert_eq!(
            pixel(
                Policy::PerAxisCrop,
                [-0.1, 0.5],
                [0.5, 2.],
                0.,
                &previous,
                &image
            ),
            [0; 4]
        );
    }
    #[test]
    fn every_policy_rejects_nan_sampling_domain_and_preserves_premultiplication() {
        let previous = source(true);
        let next = source(false);
        for policy in Policy::ALL {
            assert_eq!(
                pixel(policy, [f64::NAN, 0.5], [0.5, 2.], 0.5, &previous, &next),
                [0; 4]
            );
            for x in 0..8 {
                for y in 0..8 {
                    let out = pixel(
                        policy,
                        [(f64::from(x) + 0.5) / 8., (f64::from(y) + 0.5) / 8.],
                        [0.5, 2.],
                        0.5,
                        &previous,
                        &next,
                    );
                    assert!(out[..3].iter().all(|channel| *channel <= out[3]));
                }
            }
        }
    }
}
