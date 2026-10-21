use super::*;

const SPACES: [Primaries; 3] = [Primaries::Srgb, Primaries::Bt2020, Primaries::DisplayP3];

#[test]
fn cached_gamut_coefficients_match_the_public_d65_product() {
    for from in SPACES {
        for to in SPACES {
            let actual = primaries_matrix(from, to);
            for (row, values) in actual.into_iter().enumerate() {
                for (column, value) in values.into_iter().enumerate() {
                    let expected = if from == to {
                        f32::from(row == column)
                    } else {
                        let a = to.from_xyz()[row];
                        let b = from.to_xyz();
                        let mut sum = 0.0_f32;
                        for index in 0..3 {
                            sum += a[index] * b[index][column];
                        }
                        sum
                    };
                    assert_eq!(value.to_bits(), expected.to_bits(), "{from:?} -> {to:?}");
                }
            }
        }
    }
}

#[test]
fn calibrated_intents_preserve_neutral_black_and_reference_white() {
    for intent in [RenderingIntent::Perceptual, RenderingIntent::RelativeBpc] {
        for from in SPACES {
            for to in SPACES {
                let source = SurfaceColor {
                    description: ColorDescription {
                        primaries: from,
                        transfer: TransferFunction::Linear,
                        min_luminance: 0.5,
                        linear_scale: 79.5,
                        ..ColorDescription::srgb()
                    },
                    intent,
                };
                let target = ColorDescription {
                    primaries: to,
                    transfer: TransferFunction::Linear,
                    min_luminance: 0.25,
                    reference_luminance: 200.,
                    max_luminance: 1000.,
                    max_fall: 400.,
                    linear_scale: 199.75,
                };
                for endpoint in [0., 1.] {
                    for value in convert_color([endpoint; 3], source, target) {
                        assert!(
                            (value - endpoint).abs() < 4e-6,
                            "{from:?} -> {to:?} {intent:?}: {value}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn shoulder_preserves_channel_ratios_and_caps_its_maximum() {
    let source = SurfaceColor {
        description: ColorDescription {
            transfer: TransferFunction::Linear,
            reference_luminance: 100.,
            max_luminance: 4000.,
            max_fall: 400.,
            linear_scale: 100.,
            ..ColorDescription::srgb()
        },
        intent: RenderingIntent::Perceptual,
    };
    let target = ColorDescription {
        transfer: TransferFunction::Linear,
        reference_luminance: 100.,
        max_luminance: 600.,
        max_fall: 400.,
        linear_scale: 100.,
        ..ColorDescription::srgb()
    };
    let conversion = pipeline::Conversion::new(source, target);
    for step in 1..=400 {
        let rgb = [step as f32 / 10., step as f32 / 20., step as f32 / 40.];
        let output = conversion.apply(rgb);
        assert!(output[0] <= 6.00001);
        assert!((output[1] / output[0] - 0.5).abs() < 1e-6);
        assert!((output[2] / output[0] - 0.25).abs() < 1e-6);
    }
}

#[test]
fn relative_and_absolute_intents_keep_their_distinct_luminance_contracts() {
    let target = ColorDescription {
        transfer: TransferFunction::Linear,
        reference_luminance: 200.,
        max_luminance: 1000.,
        max_fall: 400.,
        linear_scale: 100.,
        ..ColorDescription::srgb()
    };
    for (intent, expected) in [
        (RenderingIntent::Relative, 2.),
        (RenderingIntent::Absolute, 0.8),
        (RenderingIntent::AbsoluteNoAdaptation, 0.8),
    ] {
        let output = convert_color(
            [1.; 3],
            SurfaceColor {
                intent,
                ..SurfaceColor::default()
            },
            target,
        );
        assert_eq!(output, [expected; 3]);
    }
}
