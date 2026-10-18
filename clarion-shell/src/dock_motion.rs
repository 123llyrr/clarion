// SPDX-License-Identifier: GPL-3.0-or-later
//! Clock-independent Dock geometry and finite animation samples.
//!
//! Raised-cosine magnification, the critical response, and the launch bounce
//! below are Clarion tuning choices. The available Big Sur evidence does not
//! establish these as Apple's magnification, bounce, or autohide constants.
//! Callers sample at their frame clock and request another frame only while a
//! sample is active; no timer, platform object, or renderer lives in this module.

use std::f32::consts::PI;
use std::time::Duration;

const SHADOW: f32 = 12.;
const TRAY_PADDING: f32 = 10.;
const MAGNIFICATION_SUPPORT: f32 = 2.5;
const RESPONSE_FREQUENCY: f64 = 26.;
const POSITION_EPSILON: f64 = 0.001;

/// A stable surface budget; hover and bounce do not resize the Wayland surface.
#[derive(Clone, Debug)]
pub struct DockMetrics {
    /// Idle tray extent along the row/column.
    pub length: f32,
    /// Idle tray extent perpendicular to the row/column.
    pub thickness: f32,
    pub surface_width: f32,
    pub surface_height: f32,
    /// Surface-local leading edge of the idle tray.
    pub main_padding: f32,
    /// Empty space on the side facing away from the screen edge.
    pub cross_padding: f32,
    pub item: f32,
    pub icon_size: f32,
    pub scale: f32,
    pub growth: f32,
    pub bounce_amplitude: f32,
    pub visible_apps: usize,
    pub slot_count: usize,
    pub vertical: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slot {
    /// Surface-local leading edge of the icon (not the enclosing hit cell).
    pub origin_main: f32,
    pub size: f32,
    /// Growth away from the screen edge; keep the icon's edge baseline fixed.
    pub lift: f32,
    /// Enclosing cell extent, including the unchanged 12-point separation.
    pub extent: f32,
}

fn finite(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        fallback
    }
}

impl DockMetrics {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        app_count: usize,
        icon_size: f32,
        scale: f32,
        magnification_enabled: bool,
        show_launcher: bool,
        vertical: bool,
        output_width: f32,
        output_height: f32,
    ) -> Self {
        let width = finite(output_width, 1200.).max(1.);
        let height = finite(output_height, 800.).max(1.);
        let (output_main, output_cross) = if vertical {
            (height, width)
        } else {
            (width, height)
        };
        // Eight points at each screen edge leave room for the surface margin.
        let main_budget = (output_main - 16.).max(1.);
        let cross_budget = (output_cross - 16.).max(1.);
        let utilities = 1 + usize::from(show_launcher);
        let icon_size = finite(icon_size, 48.)
            .clamp(1., 256.)
            .min(((main_budget - 44.) / utilities as f32 - 12.).max(1.))
            .min((cross_budget - 44.).max(1.));
        let item = icon_size + 12.;
        let thickness = icon_size + 32.;
        let bounce_amplitude = (icon_size * 0.35).min((cross_budget - thickness - SHADOW).max(0.));
        let desired_growth = if magnification_enabled {
            icon_size * (finite(scale, 1.).clamp(1., 2.) - 1.)
        } else {
            0.
        };
        // Five equally spaced raised-cosine samples sum to 2.5 at every phase.
        // This bounds the expansion even while the pointer moves between slots.
        let main_growth_limit =
            ((main_budget - utilities as f32 * item - 2. * TRAY_PADDING - 2. * SHADOW)
                / MAGNIFICATION_SUPPORT)
                .max(0.);
        let cross_growth_limit = (cross_budget - thickness - SHADOW - bounce_amplitude).max(0.);
        let growth = desired_growth
            .min(main_growth_limit)
            .min(cross_growth_limit);
        let expansion = (MAGNIFICATION_SUPPORT * growth).ceil();
        let available_slots = ((main_budget - expansion - 2. * TRAY_PADDING - 2. * SHADOW) / item)
            .floor()
            .max(utilities as f32) as usize;
        let visible_apps = app_count
            .min(18)
            .min(available_slots.saturating_sub(utilities));
        let slot_count = visible_apps + utilities;
        let length = slot_count as f32 * item + 2. * TRAY_PADDING;
        // Layer-shell configure sizes are integer logical points. Retaining a
        // fractional requested size would cause every Host snapshot to request
        // another resize, and rounding down could crop the tray's outer edge.
        let surface_main = (length + 2. * SHADOW + expansion)
            .ceil()
            .min(output_main.floor().max(1.));
        let surface_cross = (thickness + SHADOW + growth + bounce_amplitude)
            .ceil()
            .min(output_cross.floor().max(1.));
        let main_padding = (surface_main - length) / 2.;
        let cross_padding = surface_cross - thickness;
        let (surface_width, surface_height) = if vertical {
            (surface_cross, surface_main)
        } else {
            (surface_main, surface_cross)
        };
        Self {
            length,
            thickness,
            surface_width,
            surface_height,
            main_padding,
            cross_padding,
            item,
            icon_size,
            scale: 1. + growth / icon_size,
            growth,
            bounce_amplitude,
            visible_apps,
            slot_count,
            vertical,
        }
    }

    /// Pointer and sampled icon positions use the same surface-local main axis.
    /// Influence uses idle centers, avoiding a moving-target hover feedback loop.
    pub fn samples(&self, pointer_main: Option<f32>, strength: f32) -> Vec<Slot> {
        let strength = finite(strength, 0.).clamp(0., 1.);
        let pointer = pointer_main.filter(|position| position.is_finite());
        let inset = if self.vertical { 3.5 } else { 6. };
        let idle_start = self.main_padding + TRAY_PADDING;
        let growth = (0..self.slot_count)
            .map(|index| {
                let center = idle_start + index as f32 * self.item + inset + self.icon_size / 2.;
                let influence = pointer
                    .map(|pointer| {
                        let normalized =
                            (center - pointer).abs() / (MAGNIFICATION_SUPPORT * self.item);
                        if normalized >= 1. {
                            0.
                        } else {
                            (1. + (PI * normalized).cos()) / 2.
                        }
                    })
                    .unwrap_or(0.);
                self.growth * influence * strength
            })
            .collect::<Vec<_>>();
        let total_growth: f32 = growth.iter().sum();
        let mut cursor = idle_start - total_growth / 2.;
        growth
            .into_iter()
            .map(|growth| {
                let slot = Slot {
                    origin_main: cursor + inset,
                    size: self.icon_size + growth,
                    lift: growth,
                    extent: self.item + growth,
                };
                cursor += slot.extent;
                slot
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionSample {
    pub value: f32,
    pub velocity: f32,
    pub active: bool,
}

/// Analytic critically damped scalar response with velocity-preserving retargets.
#[derive(Clone, Debug)]
pub struct Motion {
    from: f64,
    target: f64,
    velocity: f64,
    started: Duration,
    frequency: f64,
}

impl Motion {
    pub fn new(value: f32) -> Self {
        Self::with_frequency(value, RESPONSE_FREQUENCY)
    }

    pub fn with_frequency(value: f32, frequency: f64) -> Self {
        let value = finite(value, 0.) as f64;
        Self {
            from: value,
            target: value,
            velocity: 0.,
            started: Duration::ZERO,
            frequency: if frequency.is_finite() {
                frequency.clamp(1., 1000.)
            } else {
                RESPONSE_FREQUENCY
            },
        }
    }

    pub fn target(&self) -> f32 {
        self.target as f32
    }

    /// Set an immediate endpoint, for reduced motion or a newly mapped surface.
    pub fn snap(&mut self, value: f32) {
        self.target = finite(value, 0.) as f64;
        self.from = self.target;
        self.velocity = 0.;
    }

    pub fn retarget(&mut self, target: f32, now: Duration) {
        let target = finite(target, self.target as f32) as f64;
        if self.target == target {
            return;
        }
        let current = self.sample(now);
        self.from = current.value as f64;
        self.velocity = current.velocity as f64;
        self.target = target;
        self.started = now;
    }

    pub fn sample(&self, now: Duration) -> MotionSample {
        let elapsed = now.saturating_sub(self.started).as_secs_f64();
        let difference = self.from - self.target;
        let coefficient = self.velocity + self.frequency * difference;
        let decay = (-self.frequency * elapsed).exp();
        let displacement = (difference + coefficient * elapsed) * decay;
        let velocity = (self.velocity - self.frequency * coefficient * elapsed) * decay;
        let active = displacement.abs() > POSITION_EPSILON
            || velocity.abs() > POSITION_EPSILON * self.frequency;
        MotionSample {
            value: if active {
                (self.target + displacement) as f32
            } else {
                self.target as f32
            },
            velocity: if active { velocity as f32 } else { 0. },
            active,
        }
    }
}

pub const LAUNCH_BOUNCE_DURATION: Duration = Duration::from_millis(880);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BounceSample {
    /// Positive displacement away from the output edge, in logical points.
    pub offset: f32,
    pub active: bool,
}

/// Three finite, diminishing hops. The caller starts this only for a new launch,
/// never for focusing an existing window; reduced motion bypasses it entirely.
pub fn launch_bounce(elapsed: Duration, icon_size: f32) -> BounceSample {
    if elapsed >= LAUNCH_BOUNCE_DURATION {
        return BounceSample {
            offset: 0.,
            active: false,
        };
    }
    let mut milliseconds = elapsed.as_secs_f32() * 1000.;
    let amplitude = finite(icon_size, 0.).max(0.) * 0.35;
    for (duration, attenuation) in [(360., 1.), (280., 0.48), (240., 0.2)] {
        if milliseconds < duration {
            let phase = PI * milliseconds / duration;
            return BounceSample {
                offset: amplitude * attenuation * phase.sin().powi(2),
                active: amplitude > 0.,
            };
        }
        milliseconds -= duration;
    }
    BounceSample {
        offset: 0.,
        active: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics(vertical: bool) -> DockMetrics {
        DockMetrics::new(10, 48., 1.45, true, true, vertical, 1440., 900.)
    }

    #[test]
    fn idle_icons_retain_the_original_dock_coordinates() {
        for vertical in [false, true] {
            let metrics = metrics(vertical);
            let inset = if vertical { 13.5 } else { 16. };
            for (index, slot) in metrics.samples(None, 1.).iter().enumerate() {
                assert_eq!(
                    slot.origin_main,
                    metrics.main_padding + inset + index as f32 * metrics.item
                );
                assert_eq!(slot.size, 48.);
                assert_eq!(slot.lift, 0.);
            }
        }
    }

    #[test]
    fn magnification_is_continuous_nonoverlapping_and_within_stable_canvas() {
        for vertical in [false, true] {
            let metrics = metrics(vertical);
            let surface_main = if vertical {
                metrics.surface_height
            } else {
                metrics.surface_width
            };
            for step in 0..2001 {
                let pointer = step as f32 * surface_main / 2000.;
                let slots = metrics.samples(Some(pointer), 1.);
                for pair in slots.windows(2) {
                    assert!(pair[1].origin_main - pair[0].origin_main - pair[0].size >= 11.99);
                }
                for slot in slots {
                    assert!(slot.origin_main >= 0.);
                    assert!(slot.origin_main + slot.size <= surface_main);
                    assert!(slot.size >= 48. && slot.size <= 48. * 1.45 + 0.001);
                    assert!(slot.lift <= metrics.growth + 0.001);
                }
                let next = metrics.samples(Some(pointer + 0.01), 1.);
                let current = metrics.samples(Some(pointer), 1.);
                for (before, after) in current.iter().zip(&next) {
                    assert!((before.origin_main - after.origin_main).abs() < 0.1);
                    assert!((before.size - after.size).abs() < 0.1);
                }
            }
        }
    }

    #[test]
    fn output_budgets_limit_count_and_growth_without_changing_idle_geometry() {
        for (width, height) in [(320., 240.), (640., 480.), (1440., 900.), (3840., 2160.)] {
            for vertical in [false, true] {
                for enabled in [false, true] {
                    let metrics = DockMetrics::new(
                        usize::MAX,
                        64.,
                        2.,
                        enabled,
                        true,
                        vertical,
                        width,
                        height,
                    );
                    assert!(metrics.surface_width <= width && metrics.surface_height <= height);
                    assert!(metrics.visible_apps <= 18);
                    assert_eq!(metrics.thickness, metrics.icon_size + 32.);
                    let center = if vertical {
                        metrics.surface_height / 2.
                    } else {
                        metrics.surface_width / 2.
                    };
                    for slot in metrics.samples(Some(center), 1.) {
                        assert!(slot.size.is_finite());
                    }
                    if !enabled {
                        assert_eq!(metrics.growth, 0.);
                    }
                }
            }
        }
    }

    #[test]
    fn negotiated_surfaces_use_integer_sizes_and_keep_the_idle_tray_centered() {
        for scale in [1., 1.1, 1.45, 1.75, 2.] {
            for vertical in [false, true] {
                let metrics = DockMetrics::new(10, 48., scale, true, true, vertical, 1440., 900.);
                assert_eq!(metrics.surface_width.fract(), 0.);
                assert_eq!(metrics.surface_height.fract(), 0.);
                let (main, cross) = if vertical {
                    (metrics.surface_height, metrics.surface_width)
                } else {
                    (metrics.surface_width, metrics.surface_height)
                };
                assert!((metrics.main_padding * 2. + metrics.length - main).abs() < 0.001);
                assert!((metrics.cross_padding + metrics.thickness - cross).abs() < 0.001);
                assert!(
                    metrics.cross_padding >= SHADOW + metrics.growth + metrics.bounce_amplitude
                );
                assert!(metrics.main_padding >= SHADOW);
            }
        }
    }

    #[test]
    fn a_retarget_starts_at_the_visible_position_and_velocity() {
        let mut motion = Motion::new(0.);
        motion.retarget(1., Duration::ZERO);
        let now = Duration::from_millis(73);
        let before = motion.sample(now);
        motion.retarget(0., now);
        let after = motion.sample(now);
        assert!((before.value - after.value).abs() < 1e-6);
        assert!((before.velocity - after.velocity).abs() < 1e-6);
        assert!(after.active);
        assert_eq!(
            motion.sample(Duration::from_secs(2)),
            MotionSample {
                value: 0.,
                velocity: 0.,
                active: false
            }
        );
    }

    #[test]
    fn motion_sampling_is_independent_of_frame_rate_and_goes_dormant() {
        let mut motion = Motion::new(0.);
        motion.retarget(1., Duration::ZERO);
        let target_time = Duration::from_millis(137);
        let expected = motion.sample(target_time);
        for refresh in [30, 60, 120, 144, 240] {
            let mut now = Duration::ZERO;
            while now < target_time {
                let _ = motion.sample(now);
                now += Duration::from_secs_f64(1. / f64::from(refresh));
            }
            assert_eq!(motion.sample(target_time), expected);
        }
        assert_eq!(
            motion.sample(Duration::from_secs(1)),
            MotionSample {
                value: 1.,
                velocity: 0.,
                active: false
            }
        );
        motion.snap(0.);
        assert!(!motion.sample(Duration::from_secs(1)).active);
    }

    #[test]
    fn launch_hops_start_and_finish_at_rest_and_never_exceed_the_budget() {
        assert_eq!(launch_bounce(Duration::ZERO, 48.).offset, 0.);
        for milliseconds in 0..=1200 {
            let sample = launch_bounce(Duration::from_millis(milliseconds), 48.);
            assert!(sample.offset >= 0. && sample.offset <= 48. * 0.35 + 0.001);
            if milliseconds >= 880 {
                assert!(!sample.active && sample.offset == 0.);
            }
        }
        for boundary in [360, 640, 880] {
            let sample = launch_bounce(Duration::from_millis(boundary), 48.);
            assert!(sample.offset.abs() < 0.001);
        }
        assert!(!launch_bounce(Duration::from_millis(100), f32::NAN).active);
    }

    #[test]
    fn invalid_inputs_do_not_poison_frame_state() {
        let metrics = DockMetrics::new(10, f32::NAN, f32::INFINITY, true, true, false, 1440., 900.);
        assert!(metrics
            .samples(Some(f32::NAN), f32::NAN)
            .iter()
            .all(|slot| slot.size.is_finite()));
        let mut motion = Motion::new(f32::NAN);
        motion.retarget(f32::NAN, Duration::ZERO);
        assert_eq!(
            motion.sample(Duration::from_secs(1)),
            MotionSample {
                value: 0.,
                velocity: 0.,
                active: false
            }
        );
    }
}
