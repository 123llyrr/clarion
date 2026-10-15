//! Stage drag restoration owns stable identities and logical placement memory.
//! Capturing and resolving are discrete commands, never render-frame snapshots.

use crate::bsp::BspSnapshot;
use crate::layout::{BspNode, LayoutMode};
use crate::navigation::OutputSpacePolicy;
use crate::policy_memory::{ColumnDisplay, ColumnSnapshot, Width};
use crate::workspace_state::RestoreColumn;
use crate::SpaceId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageOutput {
    pub connector: String,
    /// A connector name alone does not identify a replacement Output object.
    pub generation: u64,
}
#[derive(Debug, Clone, PartialEq)]
pub struct StageCapture<I> {
    pub window: I,
    pub columns: Vec<ColumnSnapshot<I>>,
    pub bsp: Option<BspSnapshot<I>>,
    pub floating_position: Option<[f64; 2]>,
    pub floating_size: Option<[i32; 2]>,
    pub floating_width: f64,
    pub restore_to_floating: bool,
    pub fullscreen: bool,
    pub maximized: bool,
}
/// Filled by DesktopCore from its window/space/output records.
#[derive(Debug, Clone, PartialEq)]
pub struct StageCaptureContext {
    pub space: SpaceId,
    pub output: Option<StageOutput>,
    pub floating: bool,
    pub canvas_origin: bool,
}
#[derive(Debug, Clone, PartialEq)]
pub enum StageDragAction<I> {
    Capture(StageCapture<I>),
    Resolve {
        window: I,
        destination: SpaceId,
        columns: Vec<RestoreColumn<I>>,
    },
    Take {
        window: I,
    },
    Finish {
        window: I,
    },
    Discard {
        window: I,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageRestorePosition {
    Auto,
    NewColumn(usize),
    InColumn(usize, usize),
}
#[derive(Debug, Clone, PartialEq)]
pub struct StageRestoreIntent<I> {
    pub window: I,
    pub source_space: SpaceId,
    pub space: SpaceId,
    pub position: StageRestorePosition,
    /// Only topology is restored. Other windows retain their current expanded
    /// mode; this drag's own fullscreen/maximized flags are explicit below.
    pub bsp: Option<BspSnapshot<I>>,
    pub display: Option<ColumnDisplay>,
    pub floating: bool,
    pub floating_position: Option<[f64; 2]>,
    pub floating_size: Option<[i32; 2]>,
    pub width: Width,
    pub full_width: bool,
    pub restore_to_floating: bool,
    pub fullscreen: bool,
    pub maximized: bool,
    pub canvas_origin: bool,
}
#[derive(Debug, Clone, PartialEq)]
pub struct StageRestoreCompletion<I> {
    pub window: I,
    pub space: SpaceId,
    pub canvas_origin: bool,
}
#[derive(Debug, Clone, PartialEq)]
struct ColumnOrigin<I> {
    id: u64,
    index: usize,
    previous: Option<u64>,
    next: Option<u64>,
    row: usize,
    previous_window: Option<I>,
    next_window: Option<I>,
    display: ColumnDisplay,
}
#[derive(Debug, Clone, PartialEq)]
struct Origin<I> {
    window: I,
    context: StageCaptureContext,
    column: Option<ColumnOrigin<I>>,
    bsp: Option<BspSnapshot<I>>,
    floating_position: Option<[f64; 2]>,
    floating_size: Option<[i32; 2]>,
    width: Width,
    full_width: bool,
    restore_to_floating: bool,
    fullscreen: bool,
    maximized: bool,
}
#[derive(Debug, Clone)]
enum Session<I> {
    Captured(Origin<I>),
    Resolved(StageRestoreIntent<I>),
    Restoring {
        window: I,
        source_space: SpaceId,
        destination: SpaceId,
        canvas_origin: bool,
    },
}
#[derive(Debug, Clone)]
pub struct StageDragState<I> {
    session: Option<Session<I>>,
}
impl<I> Default for StageDragState<I> {
    fn default() -> Self {
        Self { session: None }
    }
}
impl<I: Clone + Eq> StageDragState<I> {
    pub fn window(&self) -> Option<&I> {
        self.session.as_ref().map(|session| match session {
            Session::Captured(origin) => &origin.window,
            Session::Resolved(intent) => &intent.window,
            Session::Restoring { window, .. } => window,
        })
    }
    /// Historical source identity; DesktopCore filters this against live spaces
    /// before using it to pin an empty workspace. It survives output transfers.
    pub fn source_space(&self) -> Option<SpaceId> {
        self.session.as_ref().map(|session| match session {
            Session::Captured(origin) => origin.context.space,
            Session::Resolved(intent) => intent.source_space,
            Session::Restoring { source_space, .. } => *source_space,
        })
    }
    pub fn resolved_space(&self) -> Option<SpaceId> {
        match &self.session {
            Some(Session::Resolved(intent)) => Some(intent.space),
            _ => None,
        }
    }
    pub fn restoring_space(&self) -> Option<SpaceId> {
        match &self.session {
            Some(Session::Restoring { destination, .. }) => Some(*destination),
            _ => None,
        }
    }
    pub fn capture(&mut self, capture: StageCapture<I>, context: StageCaptureContext) -> bool {
        if self.session.is_some() {
            return false;
        }
        let column = capture
            .columns
            .iter()
            .enumerate()
            .find_map(|(index, column)| {
                column
                    .windows
                    .iter()
                    .position(|id| id == &capture.window)
                    .map(|row| (index, row, column))
            });
        let (placement, width, full_width, maximized) = if let Some((index, row, column)) = column {
            (
                Some(ColumnOrigin {
                    id: column.id,
                    index,
                    previous: index.checked_sub(1).map(|index| capture.columns[index].id),
                    next: capture.columns.get(index + 1).map(|column| column.id),
                    row,
                    previous_window: row.checked_sub(1).map(|row| column.windows[row].clone()),
                    next_window: column.windows.get(row + 1).cloned(),
                    display: column.display,
                }),
                column.width,
                column.full_width,
                column.maximized,
            )
        } else {
            (
                None,
                if capture.bsp.is_some() {
                    Width::Proportion(1.)
                } else {
                    Width::Fixed(capture.floating_width)
                },
                false,
                capture.maximized,
            )
        };
        self.session = Some(Session::Captured(Origin {
            window: capture.window,
            context,
            column: placement,
            bsp: capture.bsp,
            floating_position: capture.floating_position,
            floating_size: capture.floating_size,
            width,
            full_width,
            restore_to_floating: capture.restore_to_floating,
            fullscreen: capture.fullscreen,
            maximized,
        }));
        true
    }
    /// Current source-space association wins over the captured connector. A
    /// retired source can fall back only to the same live connection generation.
    pub fn destination(
        &self,
        navigation: &OutputSpacePolicy,
        mut known: impl FnMut(SpaceId) -> bool,
        mut generation: impl FnMut(&str) -> Option<u64>,
    ) -> Option<SpaceId> {
        let Some(Session::Captured(origin)) = self.session.as_ref() else {
            return None;
        };
        let source = origin.context.space;
        let associated = navigation
            .owner(source)
            .is_some_and(|connector| generation(connector).is_some());
        if known(source)
            && (associated
                || navigation.output_order().is_empty()
                    && navigation.detached_spaces().contains(&source))
        {
            return Some(source);
        }
        if let Some(output) = &origin.context.output {
            if generation(&output.connector) == Some(output.generation) {
                if let Some(space) = navigation
                    .active_space(&output.connector)
                    .filter(|space| known(*space))
                {
                    return Some(space);
                }
            }
        }
        navigation
            .selected_space()
            .filter(|space| known(*space))
            .or_else(|| {
                navigation.output_order().iter().find_map(|connector| {
                    navigation
                        .active_space(connector)
                        .filter(|space| known(*space))
                })
            })
            .or_else(|| {
                navigation
                    .detached_spaces()
                    .iter()
                    .copied()
                    .find(|space| known(*space))
            })
    }
    pub fn resolve(
        &mut self,
        window: &I,
        destination: SpaceId,
        mode: LayoutMode,
        canvas_enabled: bool,
        columns: &[RestoreColumn<I>],
        mut live: impl FnMut(&I) -> bool,
    ) -> bool {
        if self.window() != Some(window) || !matches!(self.session, Some(Session::Captured(_))) {
            return false;
        }
        let Some(Session::Captured(mut origin)) = self.session.take() else {
            unreachable!();
        };
        let floating = origin.context.floating && (!origin.context.canvas_origin || canvas_enabled);
        let position = if !floating && mode == LayoutMode::Scrolling {
            origin
                .column
                .as_ref()
                .map_or(StageRestorePosition::Auto, |column| {
                    resolve_column(column, columns)
                })
        } else {
            StageRestorePosition::Auto
        };
        let display = if matches!(position, StageRestorePosition::NewColumn(_)) {
            origin.column.as_ref().map(|column| column.display)
        } else {
            None
        };
        let bsp = if !floating && mode == LayoutMode::Dwindle {
            origin.bsp.take().map(|mut snapshot| {
                snapshot.topology = snapshot
                    .topology
                    .and_then(|node| retain_node(node, &mut live));
                snapshot.active = snapshot.active.filter(|id| live(id));
                snapshot.expanded.retain(|(id, _)| id == window && live(id));
                snapshot
            })
        } else {
            None
        };
        self.session = Some(Session::Resolved(StageRestoreIntent {
            window: origin.window,
            source_space: origin.context.space,
            space: destination,
            position,
            bsp,
            display,
            floating,
            floating_position: origin.floating_position,
            floating_size: origin.floating_size,
            width: origin.width,
            full_width: origin.full_width,
            restore_to_floating: origin.restore_to_floating,
            fullscreen: origin.fullscreen,
            maximized: origin.maximized,
            canvas_origin: origin.context.canvas_origin && canvas_enabled,
        }));
        true
    }
    pub fn take(&mut self, window: &I) -> Option<StageRestoreIntent<I>> {
        if self.window() != Some(window) || !matches!(self.session, Some(Session::Resolved(_))) {
            return None;
        }
        let Some(Session::Resolved(intent)) = self.session.take() else {
            unreachable!();
        };
        self.session = Some(Session::Restoring {
            window: intent.window.clone(),
            source_space: intent.source_space,
            destination: intent.space,
            canvas_origin: intent.canvas_origin,
        });
        Some(intent)
    }
    /// The insertion acknowledgement applies canvas identity before releasing
    /// the source pin. A MovePlaced notification alone must not erase this pending completion.
    pub fn finish(&mut self, window: &I) -> Option<StageRestoreCompletion<I>> {
        if self.window() != Some(window) || !matches!(self.session, Some(Session::Restoring { .. }))
        {
            return None;
        }
        let Some(Session::Restoring {
            window,
            destination,
            canvas_origin,
            ..
        }) = self.session.take()
        else {
            unreachable!();
        };
        Some(StageRestoreCompletion {
            window,
            space: destination,
            canvas_origin,
        })
    }
    pub fn discard(&mut self, window: &I) -> bool {
        if self.window() != Some(window) {
            return false;
        }
        self.session = None;
        true
    }
    pub fn end_move(&mut self, window: &I) -> bool {
        if matches!(self.session, Some(Session::Restoring { .. })) {
            return false;
        }
        self.discard(window)
    }
}
fn resolve_column<I: Eq>(
    origin: &ColumnOrigin<I>,
    columns: &[RestoreColumn<I>],
) -> StageRestorePosition {
    if let Some((index, column)) = columns
        .iter()
        .enumerate()
        .find(|(_, column)| column.id == origin.id)
    {
        let row = origin
            .next_window
            .as_ref()
            .and_then(|next| column.windows.iter().position(|id| id == next))
            .or_else(|| {
                origin.previous_window.as_ref().and_then(|previous| {
                    column
                        .windows
                        .iter()
                        .position(|id| id == previous)
                        .map(|row| row + 1)
                })
            })
            .unwrap_or(origin.row.min(column.windows.len()));
        return StageRestorePosition::InColumn(index, row);
    }
    let index = origin
        .next
        .and_then(|next| columns.iter().position(|column| column.id == next))
        .or_else(|| {
            origin.previous.and_then(|previous| {
                columns
                    .iter()
                    .position(|column| column.id == previous)
                    .map(|index| index + 1)
            })
        })
        .unwrap_or(origin.index.min(columns.len()));
    StageRestorePosition::NewColumn(index)
}
fn retain_node<I>(node: BspNode<I>, live: &mut impl FnMut(&I) -> bool) -> Option<BspNode<I>> {
    match node {
        BspNode::Leaf(id) => live(&id).then_some(BspNode::Leaf(id)),
        BspNode::Split {
            axis,
            ratio,
            first,
            second,
        } => match (retain_node(*first, live), retain_node(*second, live)) {
            (Some(first), Some(second)) => Some(BspNode::Split {
                axis,
                ratio,
                first: Box::new(first),
                second: Box::new(second),
            }),
            (Some(node), None) | (None, Some(node)) => Some(node),
            (None, None) => None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Axis;
    use crate::policy_memory::Height;
    fn column(id: u64, windows: &[u64]) -> ColumnSnapshot<u64> {
        ColumnSnapshot {
            id,
            windows: windows.to_vec(),
            heights: vec![Height::Auto(1.); windows.len()],
            width: Width::Proportion(0.6),
            preset_width: None,
            full_width: false,
            fullscreen: false,
            maximized: false,
            display: ColumnDisplay::Tabbed,
            active: windows.first().copied(),
        }
    }
    fn capture(columns: Vec<ColumnSnapshot<u64>>) -> StageCapture<u64> {
        StageCapture {
            window: 2,
            columns,
            bsp: None,
            floating_position: Some([0.2, 0.3]),
            floating_size: Some([500, 400]),
            floating_width: 500.,
            restore_to_floating: false,
            fullscreen: false,
            maximized: false,
        }
    }
    fn context() -> StageCaptureContext {
        StageCaptureContext {
            space: SpaceId(10),
            output: Some(StageOutput {
                connector: "DP-1".into(),
                generation: 1,
            }),
            floating: false,
            canvas_origin: false,
        }
    }
    #[test]
    fn row_neighbors_and_current_column_identity_determine_restore() {
        let mut state = StageDragState::default();
        state.capture(capture(vec![column(101, &[1, 2, 3])]), context());
        assert!(state.resolve(
            &2,
            SpaceId(10),
            LayoutMode::Scrolling,
            false,
            &[
                RestoreColumn {
                    id: 777,
                    windows: vec![9]
                },
                RestoreColumn {
                    id: 101,
                    windows: vec![4, 1, 3]
                }
            ],
            |_| true
        ));
        let intent = state.take(&2).unwrap();
        assert_eq!(intent.position, StageRestorePosition::InColumn(1, 2));
        assert_eq!(intent.display, None);
        assert_eq!(intent.width, Width::Proportion(0.6));
    }
    #[test]
    fn deleted_column_prefers_surviving_next_then_previous_and_preserves_display() {
        for (columns, expected) in [
            (
                vec![
                    RestoreColumn {
                        id: 100,
                        windows: vec![1],
                    },
                    RestoreColumn {
                        id: 999,
                        windows: vec![8],
                    },
                    RestoreColumn {
                        id: 102,
                        windows: vec![3],
                    },
                ],
                2,
            ),
            (
                vec![
                    RestoreColumn {
                        id: 100,
                        windows: vec![1],
                    },
                    RestoreColumn {
                        id: 999,
                        windows: vec![8],
                    },
                ],
                1,
            ),
            (vec![], 0),
        ] {
            let mut state = StageDragState::default();
            state.capture(
                capture(vec![
                    column(100, &[1]),
                    column(101, &[2]),
                    column(102, &[3]),
                ]),
                context(),
            );
            state.resolve(
                &2,
                SpaceId(10),
                LayoutMode::Scrolling,
                false,
                &columns,
                |_| true,
            );
            let intent = state.take(&2).unwrap();
            assert_eq!(intent.position, StageRestorePosition::NewColumn(expected));
            assert_eq!(intent.display, Some(ColumnDisplay::Tabbed));
        }
    }
    #[test]
    fn moving_source_space_wins_and_retired_connector_requires_original_generation() {
        let mut navigation = OutputSpacePolicy::new();
        navigation.connect_output("DP-1".into());
        navigation.connect_output("HDMI-A-1".into());
        for (space, output, index) in [(10, "DP-1", 0), (20, "DP-1", 1), (30, "HDMI-A-1", 0)] {
            navigation.register_space(SpaceId(space));
            navigation.attach_space(output, SpaceId(space), index);
        }
        navigation.select_space("DP-1", SpaceId(20), false);
        navigation.select_output("HDMI-A-1");
        let mut state = StageDragState::default();
        state.capture(capture(vec![]), context());
        navigation.attach_space("HDMI-A-1", SpaceId(10), 1);
        assert_eq!(
            state.destination(&navigation, |_| true, |_| Some(99)),
            Some(SpaceId(10))
        );
        navigation.forget_space(SpaceId(10));
        assert_eq!(
            state.destination(
                &navigation,
                |id| id != SpaceId(10),
                |name| Some(if name == "DP-1" { 1 } else { 2 })
            ),
            Some(SpaceId(20))
        );
        navigation.disconnect_output("DP-1");
        navigation.connect_output("DP-1".into());
        navigation.attach_space("DP-1", SpaceId(20), 0);
        assert_eq!(
            state.destination(
                &navigation,
                |id| id != SpaceId(10),
                |name| Some(if name == "DP-1" { 3 } else { 2 })
            ),
            Some(SpaceId(30))
        );
        navigation.disconnect_output("DP-1");
        navigation.disconnect_output("HDMI-A-1");
        assert_eq!(
            state.destination(&navigation, |id| id != SpaceId(10), |_| None),
            navigation.detached_spaces().first().copied()
        );
    }
    #[test]
    fn restore_is_consumed_once_and_source_pin_survives_until_canvas_acknowledgement() {
        let mut state = StageDragState::default();
        let mut ctx = context();
        ctx.floating = true;
        ctx.canvas_origin = true;
        assert!(state.capture(capture(vec![]), ctx.clone()));
        assert!(!state.capture(capture(vec![]), ctx));
        assert!(state.take(&2).is_none());
        assert!(
            !state.resolve(&999, SpaceId(10), LayoutMode::Scrolling, true, &[], |_| {
                true
            })
        );
        assert!(state.resolve(&2, SpaceId(10), LayoutMode::Scrolling, true, &[], |_| true));
        let intent = state.take(&2).unwrap();
        assert!(intent.floating && intent.canvas_origin);
        assert!(state.take(&2).is_none());
        assert_eq!(state.source_space(), Some(SpaceId(10)));
        assert!(!state.end_move(&2));
        assert_eq!(state.source_space(), Some(SpaceId(10)));
        let completion = state.finish(&2).unwrap();
        assert!(completion.canvas_origin);
        assert!(state.finish(&2).is_none());
        assert_eq!(state.window(), None);
    }
    #[test]
    fn leaving_canvas_restores_original_tiling_but_original_float_stays_float() {
        for canvas_origin in [false, true] {
            let mut state = StageDragState::default();
            let mut ctx = context();
            ctx.floating = true;
            ctx.canvas_origin = canvas_origin;
            state.capture(capture(vec![]), ctx);
            state.resolve(&2, SpaceId(10), LayoutMode::Scrolling, false, &[], |_| true);
            let intent = state.take(&2).unwrap();
            assert_eq!(intent.floating, !canvas_origin);
            assert!(!intent.canvas_origin);
        }
    }
    #[test]
    fn bsp_restore_keeps_stable_ratios_and_prunes_closed_or_foreign_ids() {
        let mut state = StageDragState::default();
        let mut input = capture(vec![]);
        input.bsp = Some(BspSnapshot {
            topology: Some(BspNode::Split {
                axis: Axis::Horizontal,
                ratio: 0.37,
                first: Box::new(BspNode::Leaf(2)),
                second: Box::new(BspNode::Split {
                    axis: Axis::Vertical,
                    ratio: 0.61,
                    first: Box::new(BspNode::Leaf(3)),
                    second: Box::new(BspNode::Leaf(4)),
                }),
            }),
            active: Some(2),
            expanded: vec![],
        });
        state.capture(input, context());
        state.resolve(&2, SpaceId(20), LayoutMode::Dwindle, false, &[], |id| {
            *id == 2 || *id == 4
        });
        let intent = state.take(&2).unwrap();
        assert_eq!(
            intent.bsp.unwrap().topology,
            Some(BspNode::Split {
                axis: Axis::Horizontal,
                ratio: 0.37,
                first: Box::new(BspNode::Leaf(2)),
                second: Box::new(BspNode::Leaf(4))
            })
        );
        assert_eq!(intent.width, Width::Proportion(1.));
    }
}
