//! Scalar pixel measure, no GPU interpolation, packed varying or shader code.
pub(super) fn image(size: [usize; 2]) -> Vec<u8> {
    (0..size[1])
        .flat_map(|y| {
            (0..size[0]).flat_map(move |x| {
                [
                    ((13 * x + 3 * y) % 128) as u8,
                    ((17 * y + x) % 128) as u8,
                    ((7 * x + 11 * y) % 128) as u8,
                    (128 + (3 * x + 5 * y) % 128) as u8,
                ]
            })
        })
        .collect()
}
fn sample(bytes: &[u8], size: [usize; 2], uv: [f64; 2]) -> [f64; 4] {
    let position = [uv[0] * size[0] as f64 - 0.5, uv[1] * size[1] as f64 - 0.5];
    let lower = position.map(f64::floor);
    let phase = [position[0] - lower[0], position[1] - lower[1]];
    let mut color = [0.; 4];
    for y in 0..2 {
        for x in 0..2 {
            let ix = (lower[0] + x as f64).clamp(0., (size[0] - 1) as f64) as usize;
            let iy = (lower[1] + y as f64).clamp(0., (size[1] - 1) as f64) as usize;
            let weight = if x == 0 { 1. - phase[0] } else { phase[0] }
                * if y == 0 { 1. - phase[1] } else { phase[1] };
            for (channel, value) in color.iter_mut().enumerate() {
                *value += f64::from(bytes[(iy * size[0] + ix) * 4 + channel]) * weight;
            }
        }
    }
    color
}
pub(super) fn convolve(
    bytes: &[u8],
    input: [usize; 2],
    output: [usize; 2],
    offset: f32,
    up: bool,
    flip: bool,
) -> Vec<u8> {
    let down = [
        (0., 0., 4.),
        (-1., -1., 1.),
        (1., -1., 1.),
        (-1., 1., 1.),
        (1., 1., 1.),
    ];
    let upward = [
        (-2., 0., 1.),
        (2., 0., 1.),
        (0., -2., 1.),
        (0., 2., 1.),
        (-1., 1., 2.),
        (1., 1., 2.),
        (-1., -1., 2.),
        (1., -1., 2.),
    ];
    let taps = if up { &upward[..] } else { &down[..] };
    let footprint = if up { input } else { output };
    let radius = footprint.map(|side| f64::from(0.5 * offset / side as f32));
    let mut pixels = Vec::with_capacity(output[0] * output[1] * 4);
    for y in 0..output[1] {
        for x in 0..output[0] {
            let center = [
                (x as f64 + 0.5) / output[0] as f64,
                (y as f64 + 0.5) / output[1] as f64,
            ];
            let center = [center[0], if flip { 1. - center[1] } else { center[1] }];
            let mut sum = [0.; 4];
            for &(dx, dy, mass) in taps {
                let sampled = sample(
                    bytes,
                    input,
                    [center[0] + dx * radius[0], center[1] + dy * radius[1]],
                );
                for (channel, value) in sum.iter_mut().enumerate() {
                    *value += sampled[channel] * mass;
                }
            }
            pixels
                .extend(sum.map(|v| (v / if up { 12. } else { 8. }).round().clamp(0., 255.) as u8));
        }
    }
    pixels
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn probability_masses_preserve_every_constant_unorm_channel() {
        for value in 0..=255u8 {
            let pixels = [value; 4].repeat(13 * 9);
            for up in [false, true] {
                assert_eq!(
                    convolve(&pixels, [13, 9], [7, 5], 2.25, up, false),
                    [value; 4].repeat(7 * 5)
                );
            }
        }
    }
    #[test]
    fn zero_footprint_and_flip_follow_exact_pixel_centers() {
        let input = [13, 9];
        let bytes = image(input);
        assert_eq!(convolve(&bytes, input, input, 0., false, false), bytes);
        let rows = bytes
            .chunks_exact(input[0] * 4)
            .rev()
            .flatten()
            .copied()
            .collect::<Vec<_>>();
        assert_eq!(convolve(&bytes, input, input, 0., true, true), rows);
    }
}
