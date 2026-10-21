//! A conversion separates description-only work from the pixel operation.
//! Gamut coefficients are built at compile time from the public D65 matrices;
//! relative intent and the shoulder are selected once per conversion plan.
use super::{ColorDescription, Primaries, RenderingIntent, SurfaceColor};

type Matrix = [[f32; 3]; 3];

const SPACES: [Primaries; 3] = [Primaries::Srgb, Primaries::Bt2020, Primaries::DisplayP3];
const GAMUT: [[Matrix; 3]; 3] = gamut_table();

const fn gamut_table() -> [[Matrix; 3]; 3] {
    let mut table = [[[[0.; 3]; 3]; 3]; 3];
    let mut source = 0;
    while source < SPACES.len() {
        let mut target = 0;
        while target < SPACES.len() {
            let left = SPACES[target].from_xyz();
            let right = SPACES[source].to_xyz();
            let mut row = 0;
            while row < 3 {
                let mut column = 0;
                while column < 3 {
                    table[source][target][row][column] = if source == target {
                        if row == column {
                            1.
                        } else {
                            0.
                        }
                    } else {
                        // Preserve the public f32 matrix product's operation
                        // order, including its initial zero accumulator.
                        let mut sum = 0.;
                        let mut inner = 0;
                        while inner < 3 {
                            sum += left[row][inner] * right[inner][column];
                            inner += 1;
                        }
                        sum
                    };
                    column += 1;
                }
                row += 1;
            }
            target += 1;
        }
        source += 1;
    }
    table
}

const fn slot(space: Primaries) -> usize {
    match space {
        Primaries::Srgb => 0,
        Primaries::Bt2020 => 1,
        Primaries::DisplayP3 => 2,
    }
}

pub(super) fn matrix(source: Primaries, target: Primaries) -> Matrix {
    GAMUT[slot(source)][slot(target)]
}

#[derive(Clone, Copy)]
enum ShoulderShape {
    Clip,
    Compress { input: f32, bend: f32 },
}

#[derive(Clone, Copy)]
pub(super) struct Shoulder {
    reference: f32,
    peak: f32,
    shape: ShoulderShape,
}

impl Shoulder {
    pub(super) fn new(reference: f32, source_peak: f32, target_peak: f32) -> Self {
        let shape = if source_peak <= target_peak * 1.001 {
            ShoulderShape::Clip
        } else {
            let input = source_peak / reference;
            let output = target_peak / reference;
            let bend = ((output * (1. + input) - input) / (input * input)).max(0.);
            ShoulderShape::Compress { input, bend }
        };
        Self {
            reference,
            peak: target_peak,
            shape,
        }
    }

    pub(super) fn map(self, value: f32) -> f32 {
        match self.shape {
            ShoulderShape::Clip => value.min(self.peak),
            ShoulderShape::Compress { input, bend } => {
                let x = (value / self.reference).clamp(0., input);
                (self.reference * x * (1. + x * bend) / (1. + x)).min(self.peak)
            }
        }
    }
}

pub(super) struct Conversion {
    source: ColorDescription,
    target: ColorDescription,
    gamut: Matrix,
    gain: f32,
    bias: f32,
    shoulder: Option<Shoulder>,
}

impl Conversion {
    pub(super) fn new(source: SurfaceColor, target: ColorDescription) -> Self {
        let from = source.description;
        let (gain, bias) = match source.intent {
            RenderingIntent::Absolute | RenderingIntent::AbsoluteNoAdaptation => (1., 0.),
            RenderingIntent::Relative => {
                (target.reference_luminance / from.reference_luminance, 0.)
            }
            RenderingIntent::Perceptual | RenderingIntent::RelativeBpc => {
                let gain = (target.reference_luminance - target.min_luminance)
                    / (from.reference_luminance - from.min_luminance);
                (gain, target.min_luminance - from.min_luminance * gain)
            }
        };
        Self {
            source: from,
            target,
            gamut: matrix(from.primaries, target.primaries),
            gain,
            bias,
            shoulder: (source.intent == RenderingIntent::Perceptual).then(|| {
                Shoulder::new(
                    target.reference_luminance,
                    from.max_luminance * gain + bias,
                    target.max_luminance,
                )
            }),
        }
    }

    pub(super) fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let light = rgb.map(|value| self.source.decode_channel(value));
        let mut output = self.gamut.map(|row| {
            let mut total = 0.;
            for (coefficient, value) in row.into_iter().zip(light) {
                total += coefficient * value;
            }
            total * self.gain + self.bias
        });
        if let Some(shoulder) = self.shoulder {
            let peak = output.into_iter().fold(0.0_f32, f32::max);
            let mapped = shoulder.map(peak);
            if peak > 0. {
                let multiplier = mapped / peak;
                for value in &mut output {
                    *value *= multiplier;
                }
            }
        }
        output.map(|value| self.target.encode_channel(value))
    }
}
