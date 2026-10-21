//! Renderer-independent RGB colour descriptions. All luminance is cd/m².
//!
//! Relative transfer functions use `linear_scale` for the luminance represented
//! by code value one, independently of reference white and mastering metadata.
//! PQ is always absolute ST 2084; its black point is not offset by display EDID.

mod pipeline;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Primaries {
    #[default]
    Srgb,
    Bt2020,
    DisplayP3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TransferFunction {
    #[default]
    Srgb,
    Gamma22,
    Linear,
    Pq,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum RenderingIntent {
    #[default]
    Perceptual,
    Relative,
    Absolute,
    RelativeBpc,
    AbsoluteNoAdaptation,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorDescription {
    pub primaries: Primaries,
    pub transfer: TransferFunction,
    pub min_luminance: f32,
    pub reference_luminance: f32,
    pub max_luminance: f32,
    pub max_fall: f32,
    /// Relative-transfer luminance span (code value 1 minus code value 0).
    pub linear_scale: f32,
}

impl Default for ColorDescription {
    fn default() -> Self {
        Self::srgb()
    }
}

impl ColorDescription {
    pub const fn srgb() -> Self {
        Self {
            primaries: Primaries::Srgb,
            transfer: TransferFunction::Srgb,
            min_luminance: 0.0,
            reference_luminance: 80.0,
            max_luminance: 80.0,
            max_fall: 80.0,
            linear_scale: 80.0,
        }
    }

    pub const fn bt2020_pq() -> Self {
        Self {
            primaries: Primaries::Bt2020,
            transfer: TransferFunction::Pq,
            min_luminance: 0.0,
            reference_luminance: 203.0,
            max_luminance: 1000.0,
            max_fall: 400.0,
            linear_scale: 10_000.0,
        }
    }

    pub const fn windows_bt2100() -> Self {
        Self::bt2020_pq()
    }

    pub const fn windows_scrgb() -> Self {
        Self {
            primaries: Primaries::Srgb,
            transfer: TransferFunction::Linear,
            min_luminance: 0.0,
            reference_luminance: 203.0,
            max_luminance: 1000.0,
            max_fall: 400.0,
            linear_scale: 80.0,
        }
    }

    pub fn validate(self) -> bool {
        [
            self.min_luminance,
            self.reference_luminance,
            self.max_luminance,
            self.max_fall,
            self.linear_scale,
        ]
        .into_iter()
        .all(f32::is_finite)
            && self.min_luminance >= 0.0
            && self.reference_luminance > self.min_luminance
            && self.max_luminance >= self.reference_luminance
            && self.max_luminance <= 10_000.0
            && self.max_fall > 0.0
            && self.max_fall <= self.max_luminance
            && self.linear_scale > 0.0
            && self.linear_scale <= 10_000.0
    }

    pub fn decode_channel(self, value: f32) -> f32 {
        let value = finite(value);
        let linear = match self.transfer {
            TransferFunction::Pq => return pq_decode(value),
            TransferFunction::Linear => value,
            TransferFunction::Gamma22 => value.abs().powf(2.2).copysign(value),
            TransferFunction::Srgb => srgb_decode(value),
        };
        linear * self.linear_scale + self.min_luminance
    }

    pub fn encode_channel(self, nits: f32) -> f32 {
        if self.transfer == TransferFunction::Pq {
            return pq_encode(nits);
        }
        let linear = (finite(nits) - self.min_luminance) / self.linear_scale;
        match self.transfer {
            TransferFunction::Linear => linear,
            TransferFunction::Gamma22 => linear.abs().powf(1.0 / 2.2).copysign(linear),
            TransferFunction::Srgb => srgb_encode(linear),
            TransferFunction::Pq => unreachable!(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SurfaceColor {
    pub description: ColorDescription,
    pub intent: RenderingIntent,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HdrOutputSettings {
    pub sdr_white: f32,
    pub peak_luminance: f32,
    pub min_luminance: f32,
    pub max_fall: f32,
}

impl Default for HdrOutputSettings {
    fn default() -> Self {
        Self {
            sdr_white: 203.0,
            peak_luminance: 1000.0,
            min_luminance: 0.005,
            max_fall: 400.0,
        }
    }
}

impl HdrOutputSettings {
    pub fn description(self) -> ColorDescription {
        ColorDescription {
            min_luminance: self.min_luminance,
            reference_luminance: self.sdr_white,
            max_luminance: self.peak_luminance,
            max_fall: self.max_fall,
            ..ColorDescription::bt2020_pq()
        }
    }
}

fn finite(value: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        0.0
    }
}

pub fn srgb_decode(value: f32) -> f32 {
    let magnitude = value.abs();
    let linear = if magnitude <= 0.04045 {
        magnitude / 12.92
    } else {
        ((magnitude + 0.055) / 1.055).powf(2.4)
    };
    linear.copysign(value)
}

pub fn srgb_encode(value: f32) -> f32 {
    let magnitude = value.abs();
    let encoded = if magnitude <= 0.003_130_8 {
        magnitude * 12.92
    } else {
        1.055 * magnitude.powf(1.0 / 2.4) - 0.055
    };
    encoded.copysign(value)
}

pub fn pq_encode(nits: f32) -> f32 {
    let linear = (finite(nits) / 10_000.0).clamp(0.0, 1.0);
    if linear == 0.0 {
        return 0.0;
    }
    let power = linear.powf(2610.0 / 16384.0);
    ((3424.0 / 4096.0 + 2413.0 / 128.0 * power) / (1.0 + 2392.0 / 128.0 * power))
        .powf(2523.0 / 32.0)
}

pub fn pq_decode(value: f32) -> f32 {
    let power = finite(value).clamp(0.0, 1.0).powf(32.0 / 2523.0);
    let numerator = (power - 3424.0 / 4096.0).max(0.0);
    let denominator = 2413.0 / 128.0 - 2392.0 / 128.0 * power;
    10_000.0 * (numerator / denominator.max(f32::MIN_POSITIVE)).powf(16384.0 / 2610.0)
}

impl Primaries {
    pub const fn to_xyz(self) -> [[f32; 3]; 3] {
        match self {
            Self::Srgb => [
                [0.4123908, 0.3575843, 0.1804808],
                [0.212639, 0.7151687, 0.0721923],
                [0.0193308, 0.1191948, 0.9505322],
            ],
            Self::Bt2020 => [
                [0.636958, 0.1446169, 0.1688809],
                [0.2627002, 0.6779981, 0.0593017],
                [0.0, 0.0280727, 1.0609851],
            ],
            Self::DisplayP3 => [
                [0.486571, 0.2656677, 0.1982173],
                [0.2289746, 0.6917385, 0.0792869],
                [0.0, 0.0451134, 1.0439444],
            ],
        }
    }

    pub const fn from_xyz(self) -> [[f32; 3]; 3] {
        match self {
            Self::Srgb => [
                [3.24097, -1.537383, -0.498611],
                [-0.969244, 1.875968, 0.041555],
                [0.055630, -0.203977, 1.056972],
            ],
            Self::Bt2020 => [
                [1.716651, -0.355671, -0.253366],
                [-0.666684, 1.616481, 0.015769],
                [0.017640, -0.042771, 0.942103],
            ],
            Self::DisplayP3 => [
                [2.493497, -0.931384, -0.402711],
                [-0.829489, 1.762664, 0.023625],
                [0.035846, -0.076172, 0.956885],
            ],
        }
    }
}

/// Row-major D65 RGB-to-RGB matrix. These named spaces share a white point.
pub fn primaries_matrix(source: Primaries, target: Primaries) -> [[f32; 3]; 3] {
    pipeline::matrix(source, target)
}

/// Hue-preserving maxRGB modified Reinhard shoulder, mirrored by the GPU.
pub fn tone_map_luminance(value: f32, reference: f32, source_peak: f32, target_peak: f32) -> f32 {
    pipeline::Shoulder::new(reference, source_peak, target_peak).map(value)
}

/// Convert straight (unassociated) RGB. Alpha is handled by the caller.
/// Extended signed RGB survives in floating-point working spaces; final
/// bounded output clipping is the responsibility of the output pass.
pub fn convert_color(rgb: [f32; 3], source: SurfaceColor, target: ColorDescription) -> [f32; 3] {
    pipeline::Conversion::new(source, target).apply(rgb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pq_matches_absolute_standard_reference_values() {
        for (nits, code) in [
            (0.0, 0.0),
            (100.0, 0.5080784),
            (203.0, 0.5806889),
            (1000.0, 0.7518271),
            (10000.0, 1.0),
        ] {
            assert!((pq_encode(nits) - code).abs() < 2e-5);
            assert!((pq_decode(code) - nits).abs() < nits.max(1.0) * 2e-4);
        }
    }

    #[test]
    fn relative_transfers_round_trip_signed_hdr_headroom() {
        for transfer in [
            TransferFunction::Srgb,
            TransferFunction::Gamma22,
            TransferFunction::Linear,
        ] {
            let desc = ColorDescription {
                transfer,
                ..ColorDescription::srgb()
            };
            for value in [-1.0, -0.001, 0.0, 0.01, 0.5, 1.0, 4.0, 12.5] {
                assert!((desc.encode_channel(desc.decode_channel(value)) - value).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn windows_scrgb_scale_is_independent_of_reference_white() {
        let desc = ColorDescription::windows_scrgb();
        assert_eq!(desc.decode_channel(1.0), 80.0);
        assert_eq!(desc.decode_channel(2.5375), 203.0);
        assert_eq!(desc.decode_channel(12.5), 1000.0);
        assert_eq!(desc.reference_luminance, 203.0);
    }

    #[test]
    fn perceptual_mapping_compensates_source_black_and_preserves_white() {
        let source = SurfaceColor {
            description: ColorDescription {
                min_luminance: 0.2,
                linear_scale: 79.8,
                ..ColorDescription::srgb()
            },
            intent: RenderingIntent::Perceptual,
        };
        let target = HdrOutputSettings::default().description();
        for (encoded, expected) in [
            (0.0, target.min_luminance),
            (1.0, target.reference_luminance),
        ] {
            for value in convert_color([encoded; 3], source, target) {
                assert!((target.decode_channel(value) - expected).abs() < expected.max(1.0) * 2e-4);
            }
        }
    }

    #[test]
    fn absolute_intent_preserves_nits_instead_of_rescaling_reference_white() {
        let source = SurfaceColor {
            description: ColorDescription::bt2020_pq(),
            intent: RenderingIntent::Absolute,
        };
        let target = HdrOutputSettings {
            sdr_white: 300.0,
            ..Default::default()
        }
        .description();
        for value in convert_color([pq_encode(100.0); 3], source, target) {
            assert!((pq_decode(value) - 100.0).abs() < 0.03);
        }
    }

    #[test]
    fn sdr_white_maps_to_configured_white_without_peak_expansion() {
        let output = HdrOutputSettings {
            sdr_white: 200.0,
            peak_luminance: 600.0,
            ..Default::default()
        }
        .description();
        let rgb = convert_color([1.0; 3], SurfaceColor::default(), output);
        for channel in rgb {
            assert!((pq_decode(channel) - 200.0).abs() < 0.1);
        }
    }

    #[test]
    fn gamut_matrices_preserve_d65_neutral_and_round_trip() {
        for source in [Primaries::Srgb, Primaries::Bt2020, Primaries::DisplayP3] {
            for target in [Primaries::Srgb, Primaries::Bt2020, Primaries::DisplayP3] {
                let a = primaries_matrix(source, target);
                let b = primaries_matrix(target, source);
                for row in a {
                    assert!((row.into_iter().sum::<f32>() - 1.0).abs() < 3e-6);
                }
                for (row, inverse_row) in b.into_iter().enumerate() {
                    for (col, _) in a[0].into_iter().enumerate() {
                        let value: f32 = (0..3).map(|i| inverse_row[i] * a[i][col]).sum();
                        assert!((value - f32::from(row == col)).abs() < 3e-6);
                    }
                }
            }
        }
    }

    #[test]
    fn tone_map_is_monotonic_bounded_and_hits_peak() {
        for (source, target, reference) in [(1000.0, 600.0, 203.0), (10000.0, 80.0, 80.0)] {
            let mut previous = 0.0;
            for i in 0..=1000 {
                let value = source * i as f32 / 1000.0;
                let mapped = tone_map_luminance(value, reference, source, target);
                assert!(mapped >= previous && mapped <= target && mapped <= value + 1e-3);
                previous = mapped;
            }
            assert!((previous - target).abs() < 1e-3);
        }
    }

    #[test]
    fn scrgb_extended_range_converts_to_absolute_pq_nits() {
        let source = SurfaceColor {
            description: ColorDescription::windows_scrgb(),
            intent: RenderingIntent::Absolute,
        };
        let target = ColorDescription::bt2020_pq();
        for nits in [80.0, 203.0, 1_000.0] {
            // scRGB code value one is 80 nits, independent of the 203-nit
            // reference white. Absolute intent must preserve the nits.
            let output = convert_color([nits / 80.0; 3], source, target);
            for channel in output {
                assert!((pq_decode(channel) - nits).abs() <= nits * 2e-4);
            }
        }
    }

    #[test]
    fn perceptual_sdr_to_hdr_is_bounded_for_extended_scene_values() {
        let target = HdrOutputSettings::default().description();
        let mut previous = 0.0;
        for value in [0.0, 0.5, 1.0, 2.0, 4.0, 16.0, 100.0] {
            let output = convert_color([value; 3], SurfaceColor::default(), target);
            let nits = pq_decode(output[0]);
            assert!(nits.is_finite());
            assert!(nits >= target.min_luminance - 1e-3);
            assert!(nits <= target.max_luminance + 1e-3);
            assert!(nits >= previous - 1e-3);
            previous = nits;
        }
        // SDR white maps to the configured HDR reference white, not PQ peak.
        let white = pq_decode(convert_color([1.0; 3], SurfaceColor::default(), target)[0]);
        assert!((white - target.reference_luminance).abs() < 0.1);
        assert!((previous - target.max_luminance).abs() < 1.0);
    }

    #[test]
    fn invalid_or_nonfinite_metadata_is_rejected() {
        assert!(ColorDescription::srgb().validate());
        assert!(ColorDescription::windows_scrgb().validate());
        for bad in [f32::NAN, f32::INFINITY, -1.0, 0.0] {
            assert!(!ColorDescription {
                reference_luminance: bad,
                ..Default::default()
            }
            .validate());
        }
        assert!(!ColorDescription {
            max_fall: 1001.0,
            ..ColorDescription::bt2020_pq()
        }
        .validate());
        assert_eq!(pq_encode(f32::NAN), 0.0);
    }
}

#[cfg(test)]
mod pipeline_contracts;

pub use crate::color_pipeline::*;
