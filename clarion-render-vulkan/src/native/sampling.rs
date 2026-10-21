//! Exact lattice copies must select a single texel. A mathematically centered
//! linear sample can still perturb half-float values through hardware filtering.
//! This gate never changes fractional crops, rescaling or an out-of-image domain.
pub(super) fn pixel_aligned_copy(
    source: [f64; 4],
    transformed_size: [f64; 2],
    destination: [i32; 2],
    texture: [u32; 2],
) -> bool {
    source.iter().all(|v| v.is_finite() && v.fract() == 0.)
        && source[0] >= 0.
        && source[1] >= 0.
        && source[2] > 0.
        && source[3] > 0.
        && source[0] + source[2] <= f64::from(texture[0])
        && source[1] + source[3] <= f64::from(texture[1])
        && texture.iter().all(|side| *side <= (1 << 23))
        && transformed_size == destination.map(f64::from)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_rotated_pixel_lattices_and_integer_crops_are_admitted() {
        for width in [1, 2, 17, 640] {
            for height in [1, 3, 19, 400] {
                for origin in [[0, 0], [1, 7], [20, 20]] {
                    let source = [
                        f64::from(origin[0]),
                        f64::from(origin[1]),
                        f64::from(width),
                        f64::from(height),
                    ];
                    let texture = [(origin[0] + width) as u32, (origin[1] + height) as u32];
                    for swapped in [false, true] {
                        let destination = if swapped {
                            [height, width]
                        } else {
                            [width, height]
                        };
                        assert!(pixel_aligned_copy(
                            source,
                            destination.map(f64::from),
                            destination,
                            texture
                        ));
                        // Orthogonal rotations/reflections only permute pixel centers;
                        // each ideal linear sample has exactly one unit-weight tap.
                        for x in [0, width - 1] {
                            for y in [0, height - 1] {
                                let u = f64::from(origin[0] + x) + 0.5;
                                let v = f64::from(origin[1] + y) + 0.5;
                                assert_eq!((u - 0.5).fract(), 0.);
                                assert_eq!((v - 0.5).fract(), 0.);
                            }
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn fractional_phase_scale_overflow_and_edge_clamping_keep_requested_filter() {
        let source = [2., 3., 16., 12.];
        let mapped = [16., 12.];
        let dst = [16, 12];
        let texture = [30, 30];
        for field in 0..4 {
            let mut fractional = source;
            fractional[field] += 0.125;
            assert!(!pixel_aligned_copy(fractional, mapped, dst, texture));
        }
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut invalid = source;
            invalid[0] = value;
            assert!(!pixel_aligned_copy(invalid, mapped, dst, texture));
        }
        assert!(!pixel_aligned_copy(source, mapped, [32, 24], texture));
        assert!(!pixel_aligned_copy(source, mapped, dst, [17, 30]));
        assert!(!pixel_aligned_copy(
            [-1., 3., 16., 12.],
            mapped,
            dst,
            texture
        ));
        assert!(!pixel_aligned_copy(
            source,
            mapped,
            dst,
            [u32::MAX, u32::MAX]
        ));
    }
}
