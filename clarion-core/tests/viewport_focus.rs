use clarion_core::viewport::{
    center_column_offset, fit_column_offset, focus_column_offset, ColumnSizing, FocusCentering,
    FocusContext, PositionedColumn,
};
use clarion_core::Rect;

fn context() -> FocusContext {
    FocusContext {
        working: Rect::new(40., 0., 800., 600.),
        parent: Rect::new(10., 0., 1000., 600.),
        gaps: 10.,
        centering: FocusCentering::OnOverflow,
        always_center_single: false,
    }
}

#[test]
fn expanded_and_oversized_focus_keeps_distinct_parent_work_and_fullscreen_alignment() {
    let column = PositionedColumn {
        x: 500.,
        width: 600.,
        sizing: ColumnSizing::Normal,
    };
    assert_eq!(center_column_offset(0., column, context()), Some(-140.));
    let expanded = PositionedColumn {
        sizing: ColumnSizing::Maximized,
        ..column
    };
    assert_eq!(center_column_offset(0., expanded, context()), Some(-210.));
    let oversized = PositionedColumn {
        width: 2000.,
        ..column
    };
    assert_eq!(center_column_offset(0., oversized, context()), Some(-40.));
    let fullscreen = PositionedColumn {
        sizing: ColumnSizing::Fullscreen,
        ..column
    };
    assert_eq!(fit_column_offset(0., fullscreen, context()), Some(0.));
    assert_eq!(center_column_offset(0., fullscreen, context()), Some(0.));
}

#[test]
fn overflow_checks_the_immediate_neighbor_after_a_distant_focus_jump_and_clamps_removed_source() {
    let columns = [
        PositionedColumn {
            x: 0.,
            width: 340.,
            sizing: ColumnSizing::Normal,
        },
        PositionedColumn {
            x: 350.,
            width: 140.,
            sizing: ColumnSizing::Normal,
        },
        PositionedColumn {
            x: 500.,
            width: 500.,
            sizing: ColumnSizing::Normal,
        },
    ];
    let mut context = context();
    context.working.x = 0.;
    assert_eq!(
        focus_column_offset(0., 2, Some(0), columns.len(), context, |i| columns[i]),
        Some(-290.)
    );
    // Previous indices can refer to a removed column. Only their direction
    // selects the live adjacent source; the solver must not index that slot.
    assert_eq!(
        focus_column_offset(0., 0, Some(usize::MAX), columns.len(), context, |i| columns
            [i]),
        Some(-10.)
    );
    context.always_center_single = true;
    context.centering = FocusCentering::Never;
    assert_eq!(
        focus_column_offset(0., 0, None, 1, context, |i| columns[i]),
        Some(-230.)
    );
}

#[test]
fn empty_or_invalid_geometry_cannot_call_a_payload_accessor_or_emit_nonfinite_targets() {
    assert_eq!(
        focus_column_offset(0., 0, None, 0, context(), |_| panic!("empty payload read")),
        None
    );
    let mut invalid = context();
    invalid.gaps = -1.;
    assert_eq!(
        focus_column_offset(0., 0, None, 1, invalid, |_| panic!("invalid payload read")),
        None
    );
    let column = PositionedColumn {
        x: 0.,
        width: f64::NAN,
        sizing: ColumnSizing::Normal,
    };
    assert_eq!(fit_column_offset(0., column, context()), None);
}
