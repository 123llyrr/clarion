//! Numeric window-move controls, owned by DesktopCore's one MoveInteraction.
//! The adapter transports the Tile/output and applies returned visual offsets.

use crate::{policy_memory::Width, Rect};
use clarion_motion::rubber_band::RubberBand;

// Existing Clarion drag thresholds; not verified Apple measurements.
const NORMAL_THRESHOLD_SQUARED: f64 = 256. * 256.;
const STAGE_SCREEN_THRESHOLD: f64 = 16.;
const START_BAND: RubberBand = RubberBand {
    stiffness: 1.,
    limit: 0.5,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoveStart {
    pub pointer: [f64; 2],
    /// Current client visual rectangle in output coordinates, including its
    /// scene zoom and excluding compositor/CSD decoration offsets.
    pub visual_window: Rect,
    pub width: Width,
    pub full_width: bool,
}
impl Default for MoveStart {
    fn default() -> Self {
        Self {
            pointer: [0., 0.],
            visual_window: Rect::default(),
            width: Width::Proportion(1.),
            full_width: false,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MovePlacement {
    pub width: Width,
    pub full_width: bool,
    pub floating: bool,
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoveStartIntent {
    pub offset: [f64; 2],
    pub anchor: [f64; 2],
    pub detach: bool,
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoveControls {
    placement: MovePlacement,
    anchor: [f64; 2],
    delta: [f64; 2],
    detached: bool,
}
impl MoveControls {
    pub fn new(start: MoveStart, floating: bool) -> Option<Self> {
        let rect = start.visual_window;
        if !start.pointer.iter().all(|value| value.is_finite())
            || ![rect.x, rect.y, rect.width, rect.height]
                .iter()
                .all(|value| value.is_finite())
            || !valid_width(start.width)
        {
            return None;
        }
        let ratio = |point: f64, origin: f64, size: f64| {
            if size <= 0. {
                0.5
            } else {
                ((point - origin) / size).clamp(0., 1.)
            }
        };
        let anchor = [
            ratio(start.pointer[0], rect.x, rect.width),
            ratio(start.pointer[1], rect.y, rect.height),
        ];
        if !anchor.iter().all(|value| value.is_finite()) {
            return None;
        }
        Some(Self {
            placement: MovePlacement {
                width: start.width,
                full_width: start.full_width,
                floating,
            },
            anchor,
            delta: [0., 0.],
            detached: false,
        })
    }
    pub fn placement(&self) -> MovePlacement {
        self.placement
    }
    pub fn anchor(&self) -> [f64; 2] {
        self.anchor
    }
    pub fn cumulative_delta(&self) -> [f64; 2] {
        self.delta
    }
    /// Starting motion uses screen-normalized Stage distance and the original
    /// logical desktop distance. No renderer clock or platform layout is read.
    pub fn starting_motion(
        &mut self,
        delta: [f64; 2],
        zoom: f64,
        stage_open: bool,
        floating: bool,
    ) -> Option<MoveStartIntent> {
        if self.detached
            || !zoom.is_finite()
            || zoom <= 0.
            || !delta.iter().all(|value| value.is_finite())
        {
            return None;
        }
        let total = [
            self.delta[0] + delta[0] / zoom,
            self.delta[1] + delta[1] / zoom,
        ];
        let threshold = if stage_open {
            (STAGE_SCREEN_THRESHOLD / zoom).powi(2)
        } else {
            NORMAL_THRESHOLD_SQUARED
        };
        if !total.iter().all(|value| value.is_finite()) || !threshold.is_finite() || threshold <= 0.
        {
            return None;
        }
        let distance = total[0] * total[0] + total[1] * total[1];
        let factor = START_BAND.band(distance / threshold);
        let offset = [total[0] * factor, total[1] * factor];
        if !offset.iter().all(|value| value.is_finite()) {
            return None;
        }
        self.delta = total;
        self.placement.floating = floating;
        Some(MoveStartIntent {
            offset,
            anchor: self.anchor,
            detach: floating || distance >= threshold,
        })
    }
    /// Acknowledge the retained pure layout policy's actual removal after an
    /// unfullscreen operation. This is accepted once, not a frame-time mirror.
    pub fn acknowledge_detached(&mut self, width: Width, full_width: bool, floating: bool) -> bool {
        if self.detached || !valid_width(width) {
            return false;
        }
        self.placement = MovePlacement {
            width,
            full_width,
            floating,
        };
        self.detached = true;
        true
    }
    /// Desired placement changes are accepted before protocol/alpha changes.
    pub fn set_floating(&mut self, floating: bool) -> MovePlacement {
        self.placement.floating = floating;
        self.placement
    }
}
fn valid_width(width: Width) -> bool {
    let value = match width {
        Width::Fixed(value) | Width::Proportion(value) => value,
    };
    value.is_finite() && value >= 0.
}

#[cfg(test)]
mod tests {
    use super::*;
    fn controls(floating: bool) -> MoveControls {
        MoveControls::new(
            MoveStart {
                pointer: [60., 95.],
                visual_window: Rect::new(10., 20., 200., 100.),
                width: Width::Proportion(0.73),
                full_width: true,
            },
            floating,
        )
        .unwrap()
    }
    #[test]
    fn actual_client_visual_rect_and_zero_size_define_a_finite_anchor() {
        assert_eq!(controls(false).anchor(), [0.25, 0.75]);
        assert_eq!(
            MoveControls::new(MoveStart::default(), false)
                .unwrap()
                .anchor(),
            [0.5, 0.5]
        );
        assert_eq!(
            MoveControls::new(
                MoveStart {
                    pointer: [-100., 1000.],
                    ..MoveStart::default()
                },
                false
            )
            .unwrap()
            .anchor(),
            [0.5, 0.5]
        );
    }
    #[test]
    fn normal_threshold_and_rubberband_preserve_original_numeric_path() {
        let mut state = controls(false);
        let first = state.starting_motion([128., 0.], 1., false, false).unwrap();
        assert!(!first.detach);
        assert!((first.offset[0] - 128. / 6.).abs() < 1e-12);
        let second = state.starting_motion([128., 0.], 1., false, false).unwrap();
        assert!(second.detach);
        assert!((second.offset[0] - 256. / 3.).abs() < 1e-12);
        assert_eq!(state.cumulative_delta(), [256., 0.]);
    }
    #[test]
    fn stage_threshold_is_screen_distance_at_fractional_scene_zooms() {
        for zoom in [0.2, 0.5, 1., 1.25, 1.5, 2.] {
            let mut state = controls(false);
            assert!(
                !state
                    .starting_motion([15., 0.], zoom, true, false)
                    .unwrap()
                    .detach
            );
            assert!(
                state
                    .starting_motion([2., 0.], zoom, true, false)
                    .unwrap()
                    .detach
            );
            assert!((state.cumulative_delta()[0] * zoom - 17.).abs() < 1e-12);
        }
        assert!(
            controls(true)
                .starting_motion([0., 0.], 1., false, true)
                .unwrap()
                .detach
        );
    }
    #[test]
    fn detach_ack_is_exactly_once_and_target_toggle_preserves_placement_preferences() {
        let mut state = controls(false);
        assert!(state.acknowledge_detached(Width::Proportion(0.61), true, true));
        assert!(!state.acknowledge_detached(Width::Fixed(999.), false, false));
        assert_eq!(
            state.set_floating(false),
            MovePlacement {
                width: Width::Proportion(0.61),
                full_width: true,
                floating: false
            }
        );
        assert_eq!(state.anchor(), [0.25, 0.75]);
    }
    #[test]
    fn invalid_motion_does_not_poison_owner_or_cross_a_threshold() {
        let mut state = controls(false);
        let before = state;
        for (delta, zoom) in [
            ([f64::NAN, 0.], 1.),
            ([1., 1.], 0.),
            ([f64::INFINITY, 0.], 0.5),
        ] {
            assert!(state.starting_motion(delta, zoom, false, false).is_none());
            assert_eq!(state, before);
        }
        assert!(MoveControls::new(
            MoveStart {
                width: Width::Fixed(f64::NAN),
                ..MoveStart::default()
            },
            false
        )
        .is_none());
    }
}
