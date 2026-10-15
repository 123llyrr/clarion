//! Pointer dwell identity and hysteresis, independent of shell repaint events.
use smithay::output::Output;
use smithay::utils::{Logical, Point, Rectangle, Size};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotCornerKind {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Debug, Default)]
pub(crate) struct HotCornerState {
    entered: Option<(Output, HotCornerKind)>,
}

impl HotCornerState {
    pub(crate) fn reset(&mut self) {
        self.entered = None;
    }

    pub(crate) fn reset_output(&mut self, output: &Output) {
        if self
            .entered
            .as_ref()
            .is_some_and(|(latched, _)| latched == output)
        {
            self.reset();
        }
    }

    /// Rearm only after leaving the trigger's four-logical-pixel halo. A direct
    /// absolute jump to another output/corner is a new entry, even without an
    /// intermediate event outside the old corner.
    pub(crate) fn motion(
        &mut self,
        pos: Point<f64, Logical>,
        candidate: Option<(Output, HotCornerKind)>,
        output_geometry: impl FnOnce(&Output) -> Option<Rectangle<i32, Logical>>,
    ) -> bool {
        if let Some((output, corner)) = &self.entered {
            let still_inside = output_geometry(output).is_some_and(|geo| {
                let size = geo.size.to_f64();
                let loc = match corner {
                    HotCornerKind::TopLeft => Point::from((0., 0.)),
                    HotCornerKind::TopRight => Point::from((size.w - 8., 0.)),
                    HotCornerKind::BottomLeft => Point::from((0., size.h - 8.)),
                    HotCornerKind::BottomRight => Point::from((size.w - 8., size.h - 8.)),
                };
                Rectangle::new(loc - Point::from((4., 4.)), Size::from((16., 16.)))
                    .contains(pos - geo.loc.to_f64())
            });
            if !still_inside {
                self.reset();
            }
        }
        let Some(candidate) = candidate else {
            return false;
        };
        if self.entered.as_ref() == Some(&candidate) {
            return false;
        }
        self.entered = Some(candidate);
        true
    }

    #[cfg(test)]
    pub(crate) fn entered(&self) -> Option<(Output, HotCornerKind)> {
        self.entered.clone()
    }
}
