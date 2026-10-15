//! Shared interface and geometry types for Realand's retained layout policies.
//! Only FloatingPolicy, ColumnPolicy and BspTree implement production arrangement;
//! protocol requests and renderer objects stay in platform adapters.

use crate::geometry::{clamp_rect, finite, nonnegative, Axis, Rect};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum LayoutMode {
    Floating,
    #[default]
    Scrolling,
    Dwindle,
}

impl LayoutMode {
    pub const fn index(self) -> usize {
        match self {
            Self::Floating => 0,
            Self::Scrolling => 1,
            Self::Dwindle => 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum BspNode<I> {
    Leaf(I),
    Split {
        axis: Axis,
        ratio: f64,
        first: Box<BspNode<I>>,
        second: Box<BspNode<I>>,
    },
}

/// Shared production interface for retained, device-independent layout policies.
/// Adapters supply numeric geometry only at dirty edges and consume placements;
/// arrangement never constructs a renderer/protocol desktop snapshot. Typed
/// requests preserve each mode's configuration rather than flattening its state.
pub trait LiveLayoutPolicy<I> {
    type Request<'a>
    where
        Self: 'a;

    fn arrange<'a>(&'a mut self, request: Self::Request<'a>, emit: impl FnMut(&I, Rect));
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FloatingPosition {
    #[default]
    Contained,
    /// Permit off-screen movement while keeping part of the window reachable.
    Reachable,
    /// Canvas coordinates are intentionally independent of output bounds.
    Unbounded,
}

/// Shared, allocation-free floating policy used for both full arrangement and
/// pointer-driven position updates. Values are logical pixels.
pub fn resolve_floating_rect(rect: Rect, bounds: Rect, position: FloatingPosition) -> Rect {
    let rect = rect.sanitized();
    let bounds = bounds.sanitized();
    match position {
        FloatingPosition::Contained => clamp_rect(rect, bounds),
        FloatingPosition::Unbounded => rect,
        FloatingPosition::Reachable => {
            let horizontal = (rect.width / 4.).clamp(10., 75.).min(rect.width);
            let vertical = (rect.height / 4.).clamp(10., 75.).min(rect.height);
            Rect::new(
                rect.x
                    .max(bounds.x - (rect.width - horizontal).max(0.))
                    .min(bounds.right() - horizontal),
                rect.y
                    .max(bounds.y - (rect.height - vertical).max(0.))
                    .min(bounds.bottom() - vertical),
                rect.width,
                rect.height,
            )
        }
    }
}
/// Compute horizontal column origins without constraining the scrolling extent
/// to the output. Column widths are resolved by configuration/interaction code.
pub fn column_offsets(origin: f64, scroll_offset: f64, gap: f64, widths: &[f64]) -> Vec<f64> {
    column_origins(origin, scroll_offset, gap, widths.iter().copied()).collect()
}

/// Allocation-free origins for platform adapters and the scrolling policy.
pub fn column_origins<I: Iterator<Item = f64>>(
    origin: f64,
    scroll_offset: f64,
    gap: f64,
    widths: I,
) -> impl Iterator<Item = f64> {
    let x = finite(origin, 0.) - finite(scroll_offset, 0.);
    let gap = nonnegative(gap);
    widths.scan(x, move |x, width| {
        let result = *x;
        *x += nonnegative(width) + gap;
        Some(result)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Size;

    fn live_targets<'a, P: LiveLayoutPolicy<u64>>(
        policy: &'a mut P,
        request: P::Request<'a>,
    ) -> Vec<(u64, Rect)> {
        let mut targets = Vec::new();
        policy.arrange(request, |id, rect| targets.push((*id, rect)));
        targets
    }

    #[test]
    fn common_live_interface_preserves_all_modes_identity_focus_and_geometry() {
        use crate::bsp::{BspArrangement, BspTree};
        use crate::column::{Height, HeightConstraint};
        use crate::floating::{FloatingPolicy, Placement as FloatingPlacement};
        use crate::scrolling::{ColumnArrangement, ColumnDisplay, ColumnPolicy, ColumnWidth};
        let bounds = Rect::new(20., 30., 800., 600.);
        let mut bsp = BspTree::new(bounds, 16., 32);
        for id in 1..=8u64 {
            bsp.insert(id, Some(&(id - 1)), None);
        }
        bsp.activate(&4);
        for bounds in [bounds, Rect::new(1.25, 2.5, 1279.5, 719.75)] {
            let targets = live_targets(&mut bsp, BspArrangement { bounds, gap: 16. });
            let mut identities: Vec<_> = targets.iter().map(|(id, _)| *id).collect();
            identities.sort_unstable();
            assert_eq!(identities, (1..=8).collect::<Vec<_>>());
            for (_, rect) in &targets {
                assert!(rect.width >= 0. && rect.height >= 0.);
                assert!(rect.x >= bounds.x && rect.y >= bounds.y);
                assert!(rect.right() <= bounds.right() && rect.bottom() <= bounds.bottom());
            }
            for (index, (_, a)) in targets.iter().enumerate() {
                for (_, b) in &targets[index + 1..] {
                    assert!(
                        a.right() <= b.x
                            || b.right() <= a.x
                            || a.bottom() <= b.y
                            || b.bottom() <= a.y
                    );
                }
            }
            assert_eq!(
                targets,
                live_targets(&mut bsp, BspArrangement { bounds, gap: 16. })
            );
            assert_eq!(bsp.active(), Some(&4));
        }

        let mut floating = FloatingPolicy::new();
        floating.insert(
            1,
            FloatingPlacement::new(bounds, Size::new(300., 200.), [50., 60.], false),
            0,
            true,
            &[],
        );
        floating.insert(
            2,
            FloatingPlacement::new(bounds, Size::new(400., 300.), [1000., 800.], true),
            1,
            false,
            &[],
        );
        let next = Rect::new(-5., 7., 1024., 768.);
        let targets = live_targets(&mut floating, next);
        let bounded = targets.iter().find(|(id, _)| *id == 1).unwrap().1;
        // Relative working-area position: (50-20)/800, (60-30)/600.
        assert!((bounded.x - 33.4).abs() < 1e-9);
        assert!((bounded.y - 45.4).abs() < 1e-9);
        assert_eq!(bounded.size(), Size::new(300., 200.));
        let canvas = targets.iter().find(|(id, _)| *id == 2).unwrap().1;
        // Normalized coordinates can round by one ULP when the area changes.
        assert!((canvas.x - 1000.).abs() < 1e-9);
        assert!((canvas.y - 800.).abs() < 1e-9);
        assert_eq!(canvas.size(), Size::new(400., 300.));
        assert_eq!(targets, live_targets(&mut floating, next));
        assert_eq!(floating.active(), Some(1));

        let mut column = ColumnPolicy::new(
            10,
            ColumnWidth::Fixed(400.),
            false,
            ColumnDisplay::Normal,
            None,
        );
        for id in 1..=3u64 {
            column.insert(column.rows().len(), id);
        }
        column.activate(1);
        column.height_constraints = (1..=3)
            .map(|weight| HeightConstraint {
                height: Height::Automatic {
                    weight: weight as f64,
                },
                minimum: 1.,
                maximum: 0.,
                decoration: 4.,
            })
            .collect();
        let targets = live_targets(
            &mut column,
            ColumnArrangement::Normal {
                width: 400.,
                working_height: 600.,
                gap: 16.,
                tabbed_limit: 568.,
            },
        );
        assert_eq!(
            targets
                .iter()
                .map(|(_, rect)| rect.height)
                .collect::<Vec<_>>(),
            vec![89., 179., 268.]
        );
        assert_eq!(
            targets.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(column.active_id(), Some(&2));
        column.set_display(ColumnDisplay::Tabbed);
        let targets = live_targets(
            &mut column,
            ColumnArrangement::Normal {
                width: 400.,
                working_height: 600.,
                gap: 16.,
                tabbed_limit: 568.,
            },
        );
        assert!(targets.iter().all(|(_, rect)| rect.y == 0.));
        column.set_display(ColumnDisplay::Dwindle);
        let targets = live_targets(
            &mut column,
            ColumnArrangement::Normal {
                width: 400.,
                working_height: 600.,
                gap: 16.,
                tabbed_limit: 568.,
            },
        );
        assert_eq!(
            targets.iter().map(|(_, rect)| *rect).collect::<Vec<_>>(),
            vec![
                Rect::new(0., 0., 400., 276.),
                Rect::new(0., 292., 192., 276.),
                Rect::new(208., 292., 192., 276.)
            ]
        );
        assert_eq!(column.active_id(), Some(&2));
        let targets = live_targets(&mut column, ColumnArrangement::Expanded(bounds));
        assert!(targets.iter().all(|(_, rect)| *rect == bounds));
    }

    #[test]
    fn retained_scrolling_constraints_preserve_membership_and_scroll_origin() {
        use crate::column::{Height, HeightConstraint};
        use crate::scrolling::{ColumnArrangement, ColumnDisplay, ColumnPolicy, ColumnWidth};
        let mut column = ColumnPolicy::new(
            10,
            ColumnWidth::Fixed(500.),
            false,
            ColumnDisplay::Normal,
            None,
        );
        assert!(column.insert(0, 2u64));
        assert!(column.insert(1, 1));
        assert!(!column.insert(2, 2));
        column.activate(1);
        column.height_constraints = [1., 2.]
            .into_iter()
            .map(|weight| HeightConstraint {
                height: Height::Automatic { weight },
                minimum: 1.,
                maximum: 0.,
                decoration: 0.,
            })
            .collect();
        let width = column.arranged_width(800., 10., 0., [(1., 450.), (1., 0.)].into_iter());
        assert_eq!(width, 450.);
        let targets = live_targets(
            &mut column,
            ColumnArrangement::Normal {
                width,
                working_height: 600.,
                gap: 10.,
                tabbed_limit: 580.,
            },
        );
        // Outer margins and inter-row gaps consume 30 pixels: 570 × 1:2.
        assert_eq!(
            targets,
            vec![
                (2, Rect::new(0., 0., 450., 190.)),
                (1, Rect::new(0., 200., 450., 380.))
            ]
        );
        assert_eq!(column.active_id(), Some(&1));
        assert_eq!(
            column_offsets(0., 80., 10., &[450., 300.]),
            vec![-80., 380.]
        );
    }

    #[test]
    fn retained_bsp_restores_exact_ratio_and_rejects_invalid_topology_atomically() {
        use crate::bsp::{BspArrangement, BspTree, RestoreError};
        let bounds = Rect::new(0., 0., 800., 600.);
        let mut tree = BspTree::new(bounds, 8., 32);
        let topology = BspNode::Split {
            axis: Axis::Horizontal,
            ratio: 0.25,
            first: Box::new(BspNode::Leaf(2u64)),
            second: Box::new(BspNode::Leaf(1)),
        };
        tree.restore_mapped(Some(&topology), |id| Some(*id))
            .unwrap();
        let targets = live_targets(&mut tree, BspArrangement { bounds, gap: 8. });
        assert_eq!(
            targets,
            vec![
                (2, Rect::new(0., 0., 198., 600.)),
                (1, Rect::new(206., 0., 594., 600.))
            ]
        );
        let invalid = BspNode::Split {
            axis: Axis::Vertical,
            ratio: f64::NAN,
            first: Box::new(BspNode::Leaf(1)),
            second: Box::new(BspNode::Leaf(1)),
        };
        assert_eq!(
            tree.restore_mapped(Some(&invalid), |id| Some(*id)),
            Err(RestoreError::InvalidRatio)
        );
        assert_eq!(
            targets,
            live_targets(&mut tree, BspArrangement { bounds, gap: 8. })
        );
    }
}
