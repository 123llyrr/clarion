//! Shared scene encoding and GPU parameters for per-item color management.
use super::pipeline::ColorPushConstants;
use crate::color::{ColorDescription, RenderingIntent, SurfaceColor, TransferFunction};

pub(super) fn scene_description(output: Option<ColorDescription>) -> ColorDescription {
    let mut scene = ColorDescription::srgb();
    if let Some(output) = output {
        scene.min_luminance =
            output.min_luminance * scene.reference_luminance / output.reference_luminance;
        scene.linear_scale = scene.reference_luminance - scene.min_luminance;
        scene.max_luminance =
            output.max_luminance * scene.reference_luminance / output.reference_luminance;
        scene.max_fall = output.max_fall * scene.reference_luminance / output.reference_luminance;
    }
    scene
}

fn transfer_id(transfer: TransferFunction) -> f32 {
    match transfer {
        TransferFunction::Srgb => 0.0,
        TransferFunction::Gamma22 => 1.0,
        TransferFunction::Linear => 2.0,
        TransferFunction::Pq => 3.0,
    }
}

fn parameters(description: ColorDescription) -> [f32; 4] {
    [
        transfer_id(description.transfer),
        description.min_luminance,
        description.linear_scale,
        description.reference_luminance,
    ]
}

pub(super) fn constants(
    source: SurfaceColor,
    target: ColorDescription,
    clamp: bool,
    tone_primaries: crate::color::Primaries,
    output_reference: f32,
) -> ColorPushConstants {
    let rows = crate::color::primaries_matrix(source.description.primaries, target.primaries);
    // The final default-SDR pass is an exact identity in the existing encoded
    // scene. Preserve FP16 composition, but skip six pow operations per pixel.
    // A broader encoding-only check is unsafe: content peak/code-one or the
    // reference adaptation can differ even when the transfer names match.
    let output_identity = clamp
        && source.intent == RenderingIntent::Relative
        && source.description == ColorDescription::srgb()
        && target == ColorDescription::srgb();
    // HDR's non-zero display black changes the scene's luminance numbers, but
    // perceptual SDR black/white adaptation cancels its encode normalization.
    // The shader verifies the sampled premultiplied bounds before using this
    // identity; malformed alpha and extended float samples need the full path.
    let scene_identity = !clamp && perceptual_sdr_scene_identity(source, target);
    ColorPushConstants {
        matrix: [
            2., 0., 0., 0., 0., 2., 0., 0., 0., 0., 1., 0., -1., -1., 0., 1.,
        ],
        uv_bounds: [0., 0., 1., 1.],
        gamut: std::array::from_fn(|i| {
            [
                rows[i][0],
                rows[i][1],
                rows[i][2],
                if i == 0 {
                    target.reference_luminance / output_reference
                } else if i == 1 && output_identity {
                    1.
                } else if i == 1 && scene_identity {
                    2.
                } else if i == 2 {
                    1. // Panel luminance gain; changed only for the final HDR pass.
                } else {
                    0.
                },
            ]
        }),
        source: parameters(source.description),
        target: parameters(target),
        mapping: [
            source.description.max_luminance,
            target.max_luminance,
            match source.intent {
                RenderingIntent::Perceptual => 0.,
                RenderingIntent::Relative | RenderingIntent::RelativeBpc => 1.,
                RenderingIntent::Absolute | RenderingIntent::AbsoluteNoAdaptation => 2.,
            },
            if clamp { 1. } else { 0. },
        ],
        tone_gamut: crate::color::primaries_matrix(target.primaries, tone_primaries)
            .map(|row| [row[0], row[1], row[2], 0.]),
    }
}

pub(super) fn perceptual_sdr_scene_identity(
    source: SurfaceColor,
    target: ColorDescription,
) -> bool {
    source == SurfaceColor::default()
        && target.validate()
        && target.primaries == crate::color::Primaries::Srgb
        && target.transfer == TransferFunction::Srgb
        && target.reference_luminance == 80.
        && target.linear_scale == target.reference_luminance - target.min_luminance
        // Leave room for floating-point error in the output gamut shoulder
        // metric. This only admits HDR scenes; default SDR already matches.
        && target.max_luminance >= target.reference_luminance * 1.001
}

pub(super) fn opaque_sdr_scene_identity(
    source: SurfaceColor,
    target: ColorDescription,
    format: ash::vk::Format,
    has_alpha: bool,
) -> bool {
    !has_alpha
        && matches!(
            format,
            ash::vk::Format::R8G8B8A8_UNORM
                | ash::vk::Format::B8G8R8A8_UNORM
                | ash::vk::Format::A2B10G10R10_UNORM_PACK32
                | ash::vk::Format::A2R10G10B10_UNORM_PACK32
                | ash::vk::Format::R5G6B5_UNORM_PACK16
                | ash::vk::Format::B5G6R5_UNORM_PACK16
        )
        && perceptual_sdr_scene_identity(source, target)
}

pub(super) fn encoding_matches(source: SurfaceColor, target: ColorDescription) -> bool {
    let source = source.description;
    source.primaries == target.primaries
        && source.transfer == target.transfer
        && source.min_luminance == target.min_luminance
        && source.linear_scale == target.linear_scale
        && source.reference_luminance == target.reference_luminance
        && source.max_luminance <= target.max_luminance * 1.001
}

/// Decode binary16 without relying on the host CPU's FP16 instruction set.
pub(super) fn half_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = (bits >> 10) & 31;
    let mantissa = bits & 1023;
    if exponent == 0 {
        sign * (mantissa as f32) * 2.0_f32.powi(-24)
    } else if exponent == 31 {
        if mantissa == 0 {
            sign * f32::INFINITY
        } else {
            f32::NAN
        }
    } else {
        sign * (1.0 + mantissa as f32 / 1024.0) * 2.0_f32.powi(exponent as i32 - 15)
    }
}

pub(super) fn float_readback_to_packed(
    pixels: &[u8],
    source: SurfaceColor,
    output: ash::vk::Format,
    input_alpha: bool,
    output_alpha: bool,
) -> Result<Vec<u8>, ash::vk::Result> {
    use ash::vk;
    if !pixels.len().is_multiple_of(8) {
        return Err(vk::Result::ERROR_INITIALIZATION_FAILED);
    }
    let mut result = Vec::with_capacity(pixels.len() / 2);
    for pixel in pixels.as_chunks::<8>().0 {
        let rgba: [f32; 4] = std::array::from_fn(|i| {
            half_to_f32(u16::from_le_bytes([pixel[i * 2], pixel[i * 2 + 1]]))
        });
        let alpha = if input_alpha {
            rgba[3].clamp(0.0, 1.0)
        } else {
            1.0
        };
        let inv = if alpha > 0.0 { 1.0 / alpha } else { 0.0 };
        let rgb = crate::color::convert_color(
            [rgba[0] * inv, rgba[1] * inv, rgba[2] * inv],
            source,
            ColorDescription::srgb(),
        );
        let premultiplied = rgb.map(|c| (c * alpha).clamp(0.0, 1.0));
        let a = if output_alpha { alpha } else { 1.0 };
        let word = match output {
            vk::Format::R8G8B8A8_UNORM | vk::Format::B8G8R8A8_UNORM => {
                let [r, g, b] = premultiplied.map(|c| (c * 255.0).round() as u32);
                let a = (a * 255.0).round() as u32;
                if output == vk::Format::R8G8B8A8_UNORM {
                    r | g << 8 | b << 16 | a << 24
                } else {
                    b | g << 8 | r << 16 | a << 24
                }
            }
            vk::Format::A2B10G10R10_UNORM_PACK32 | vk::Format::A2R10G10B10_UNORM_PACK32 => {
                let [r, g, b] = premultiplied.map(|c| (c * 1023.0).round() as u32);
                let a = (a * 3.0).round() as u32;
                if output == vk::Format::A2B10G10R10_UNORM_PACK32 {
                    r | g << 10 | b << 20 | a << 30
                } else {
                    b | g << 10 | r << 20 | a << 30
                }
            }
            _ => return Err(vk::Result::ERROR_FORMAT_NOT_SUPPORTED),
        };
        result.extend(word.to_le_bytes());
    }
    Ok(result)
}

#[cfg(test)]
mod identity_tests {
    use super::*;

    fn identity(source: SurfaceColor, target: ColorDescription, clamp: bool) -> bool {
        constants(
            source,
            target,
            clamp,
            target.primaries,
            target.reference_luminance,
        )
        .gamut[1][3]
            == 1.
    }

    #[test]
    fn perceptual_sdr_black_white_adaptation_is_identity_in_hdr_scene() {
        for min_luminance in [0., 0.0001, 0.005, 0.2, 1., 10.] {
            let output = crate::color::HdrOutputSettings {
                min_luminance,
                ..Default::default()
            }
            .description();
            let scene = scene_description(Some(output));
            assert!(perceptual_sdr_scene_identity(
                SurfaceColor::default(),
                scene
            ));
            for value in 0..=1023 {
                let value = value as f32 / 1023.;
                let rgb = [value, value * 0.5, 1. - value];
                let expected = crate::color::convert_color(rgb, SurfaceColor::default(), scene);
                assert!(expected
                    .into_iter()
                    .zip(rgb)
                    .all(|(a, b)| (a - b).abs() < 2e-6));
            }
        }
    }

    #[test]
    fn sdr_scene_bypass_requires_opaque_bounded_rgb_and_exact_perceptual_encoding() {
        use ash::vk;
        let source = SurfaceColor::default();
        let scene = scene_description(Some(
            crate::color::HdrOutputSettings::default().description(),
        ));
        for format in [
            vk::Format::R8G8B8A8_UNORM,
            vk::Format::A2R10G10B10_UNORM_PACK32,
        ] {
            assert!(opaque_sdr_scene_identity(source, scene, format, false));
            assert!(!opaque_sdr_scene_identity(source, scene, format, true));
        }
        for format in [
            vk::Format::R16G16B16A16_SFLOAT,
            vk::Format::G8_B8R8_2PLANE_420_UNORM,
        ] {
            assert!(!opaque_sdr_scene_identity(source, scene, format, false));
        }
        for intent in [
            RenderingIntent::Relative,
            RenderingIntent::RelativeBpc,
            RenderingIntent::Absolute,
            RenderingIntent::AbsoluteNoAdaptation,
        ] {
            assert!(!perceptual_sdr_scene_identity(
                SurfaceColor { intent, ..source },
                scene
            ));
        }
        for description in [
            ColorDescription::bt2020_pq(),
            ColorDescription::windows_scrgb(),
            ColorDescription {
                linear_scale: 100.,
                ..source.description
            },
            ColorDescription {
                max_luminance: 81.,
                ..source.description
            },
        ] {
            assert!(!perceptual_sdr_scene_identity(
                SurfaceColor {
                    description,
                    ..source
                },
                scene
            ));
        }
        for target in [
            ColorDescription::bt2020_pq(),
            ColorDescription {
                linear_scale: 80.,
                ..scene
            },
            ColorDescription {
                reference_luminance: 81.,
                ..scene
            },
            ColorDescription {
                max_luminance: 79.,
                ..scene
            },
        ] {
            assert!(!perceptual_sdr_scene_identity(source, target));
        }
    }

    #[test]
    fn final_identity_requires_exact_default_sdr_and_relative_intent() {
        let source = SurfaceColor {
            description: ColorDescription::srgb(),
            intent: RenderingIntent::Relative,
        };
        assert!(identity(source, ColorDescription::srgb(), true));
        assert!(!identity(source, ColorDescription::srgb(), false));
        for intent in [
            RenderingIntent::Perceptual,
            RenderingIntent::Absolute,
            RenderingIntent::RelativeBpc,
            RenderingIntent::AbsoluteNoAdaptation,
        ] {
            assert!(!identity(
                SurfaceColor { intent, ..source },
                source.description,
                true
            ));
        }
        for description in [
            ColorDescription::bt2020_pq(),
            ColorDescription::windows_scrgb(),
            ColorDescription {
                min_luminance: 0.2,
                ..source.description
            },
            ColorDescription {
                max_luminance: 400.,
                ..source.description
            },
            ColorDescription {
                reference_luminance: 203.,
                ..source.description
            },
            ColorDescription {
                linear_scale: 100.,
                ..source.description
            },
            ColorDescription {
                primaries: crate::color::Primaries::DisplayP3,
                ..source.description
            },
        ] {
            assert!(!identity(source, description, true));
            assert!(!identity(
                SurfaceColor {
                    description,
                    ..source
                },
                source.description,
                true
            ));
            assert!(!identity(
                SurfaceColor {
                    description,
                    ..source
                },
                description,
                true
            ));
        }
    }
}
