//! Extended-domain scalar contract, independent of shader branching/operation order.
pub const COLORS: [[f32; 3]; 4] = [
    [0., 0., 0.],
    [0.1, 0.2, 0.3],
    [0.4, 0.1, 0.7],
    [2., -0.5, 0.25],
];
pub const BACKGROUNDS: [[f32; 4]; 2] = [[0.; 4], [0.2, -0.4, 1.5, 0.75]];
pub const ALPHAS: [f32; 6] = [-0.25, 0., 0.5, 2047. / 2048., 1., 1.25];
pub const SATURATIONS: [f32; 4] = [-0.5, 0., 1., 1.5];
pub const NOISES: [f32; 3] = [-0.25, 0., 0.25];
pub const GRAIN_PERIOD: i32 = 251;

/// Integer specification: direct cubic residues, independent of GLSL's staged
/// FP32 reductions. Input names are framebuffer pixel indices, not centers.
pub fn grain_rank(x: i32, y: i32) -> u32 {
    let x = x.rem_euclid(GRAIN_PERIOD) as u64;
    let y = y.rem_euclid(GRAIN_PERIOD) as u64;
    let row = y.pow(3) % 251;
    let position = (x * (1 + y % 250) + row) % 251;
    (position.pow(3) % 251) as u32
}
pub fn grain(x: i32, y: i32) -> f64 {
    (f64::from(grain_rank(x, y)) + 0.5) / 251.
}
pub fn grain_center() -> f64 {
    grain(0, 0)
}
pub fn evaluate(
    rgb: [f32; 3],
    alpha: f32,
    saturation: f32,
    noise: f32,
    background: [f32; 4],
) -> [f64; 4] {
    let rgb = rgb.map(f64::from);
    let alpha = f64::from(alpha);
    let saturation = f64::from(saturation);
    let luminance = rgb
        .into_iter()
        .zip([0.2126, 0.7152, 0.0722])
        .map(|(c, w)| c * w)
        .sum::<f64>();
    let perturbation = if noise > 0. {
        (grain_center() - 0.5) * f64::from(noise) * alpha
    } else {
        0.
    };
    let mut out = [0.; 4];
    for (i, value) in out.iter_mut().enumerate() {
        let foreground = if i == 3 {
            alpha
        } else {
            luminance * (1. - saturation) + rgb[i] * saturation + perturbation
        };
        *value = foreground + f64::from(background[i]) * (1. - alpha);
    }
    out
}
pub fn bytes(values: [f64; 4]) -> [u8; 4] {
    values.map(|v| (((v + 16.) / 32.).clamp(0., 1.) * 255.).round() as u8)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn additive_zero_alpha_is_colored_and_noise_free() {
        let rgb = COLORS[2];
        let without_noise = evaluate(rgb, 0., 1., 0., BACKGROUNDS[1]);
        assert_eq!(without_noise, evaluate(rgb, 0., 1., 0.25, BACKGROUNDS[1]));
        assert_eq!(
            without_noise[0],
            f64::from(rgb[0]) + f64::from(BACKGROUNDS[1][0])
        );
        assert_ne!(without_noise, BACKGROUNDS[1].map(f64::from));
    }
    #[test]
    fn extended_alpha_and_hdr_are_not_clamped_or_inferred() {
        for alpha in ALPHAS {
            let out = evaluate(COLORS[3], alpha, 1., 0., BACKGROUNDS[1]);
            assert_eq!(out[3], f64::from(alpha) + 0.75 * (1. - f64::from(alpha)));
            assert!(out[0] > 1.);
            assert!(out[1] < 0.);
        }
        let near = evaluate([0.; 3], ALPHAS[3], 1., 0., BACKGROUNDS[1]);
        assert!(near[0] > 0.);
        assert_eq!(
            evaluate([0.; 3], 0., -0.5, 0.25, BACKGROUNDS[1]),
            BACKGROUNDS[1].map(f64::from)
        );
    }
    #[test]
    fn signed_noise_is_applied_in_encoded_rgb_with_signed_alpha() {
        let base = evaluate(COLORS[1], -0.25, -0.5, 0., BACKGROUNDS[1]);
        assert_eq!(
            base,
            evaluate(COLORS[1], -0.25, -0.5, -0.25, BACKGROUNDS[1])
        );
        let noisy = evaluate(COLORS[1], -0.25, -0.5, 0.25, BACKGROUNDS[1]);
        let delta = (grain_center() - 0.5) * 0.25 * -0.25;
        for i in 0..3 {
            assert!((noisy[i] - base[i] - delta).abs() < 1e-15);
        }
        assert_eq!(noisy[3], base[3]);
    }
}

#[cfg(test)]
mod grain_tests {
    use super::*;

    #[test]
    fn each_row_is_a_complete_balanced_permutation_with_midpoint_levels() {
        let mut histogram = [0usize; 251];
        let mut sum = 0.;
        let mut square_sum = 0.;
        for y in 0..GRAIN_PERIOD {
            let mut seen = [false; 251];
            for x in 0..GRAIN_PERIOD {
                let rank = grain_rank(x, y) as usize;
                assert!(!seen[rank], "row {y} repeats level {rank}");
                seen[rank] = true;
                histogram[rank] += 1;
                let centered = grain(x, y) - 0.5;
                assert!(centered > -0.5 && centered < 0.5);
                sum += centered;
                square_sum += centered * centered;
            }
            assert!(seen.into_iter().all(|value| value));
        }
        assert_eq!(histogram, [251; 251]);
        let count = f64::from(GRAIN_PERIOD * GRAIN_PERIOD);
        assert!((sum / count).abs() < 1e-14);
        let midpoint_variance = (251f64.powi(2) - 1.) / (12. * 251f64.powi(2));
        assert!((square_sum / count - midpoint_variance).abs() < 1e-13);
    }

    #[test]
    fn grain_is_deterministic_and_repeats_only_its_declared_coordinate_tile() {
        assert_eq!(grain_rank(0, 0), 0);
        for y in [
            -1_000_000, -252, -251, -1, 0, 1, 249, 250, 251, 252, 1_000_000,
        ] {
            for x in [
                -1_000_000, -252, -251, -1, 0, 1, 249, 250, 251, 252, 1_000_000,
            ] {
                assert_eq!(grain_rank(x, y), grain_rank(x + 251, y - 251));
                assert_eq!(grain(x, y).to_bits(), grain(x, y).to_bits());
            }
        }
        // A horizontal displacement shorter than one period cannot repeat an
        // entire row; every selected nonzero stride is invertible modulo 251.
        for shift in 1..GRAIN_PERIOD {
            assert_ne!(grain_rank(0, 0), grain_rank(shift, 0));
        }
    }

    #[test]
    fn local_spatial_correlations_remain_small_over_a_complete_tile() {
        let n = GRAIN_PERIOD as usize;
        let tile: Vec<_> = (0..n * n)
            .map(|index| grain((index % n) as i32, (index / n) as i32) - 0.5)
            .collect();
        let energy: f64 = tile.iter().map(|value| value * value).sum();
        let shifts = (1..=8)
            .flat_map(|offset| [(offset, 0), (0, offset)])
            .chain([(1, 1), (1, -1), (7, 11), (11, 7)]);
        for (dx, dy) in shifts {
            let mut covariance = 0.;
            for y in 0..GRAIN_PERIOD {
                for x in 0..GRAIN_PERIOD {
                    let other_x = (x + dx).rem_euclid(GRAIN_PERIOD) as usize;
                    let other_y = (y + dy).rem_euclid(GRAIN_PERIOD) as usize;
                    covariance += tile[y as usize * n + x as usize] * tile[other_y * n + other_x];
                }
            }
            let correlation = covariance / energy;
            assert!(correlation.abs() < 0.02, "shift ({dx},{dy}): {correlation}");
        }
    }
}
