use super::*;

fn tiny_window(id: usize) -> Op {
    Op::AddWindow {
        params: TestWindowParams {
            bbox: Rectangle::from_size(Size::from((1, 1))),
            ..TestWindowParams::new(id)
        },
    }
}

#[test]
fn auto_stacking_two_windows_without_outputs_handles_exhausted_working_height() {
    // The exact minimized random-operations failure: the first normal column
    // turns into Dwindle when the second window is auto-stacked into it.
    let mut options = Options::default();
    options.layout.struts.top = FloatOrInt(31783.93378725406);
    let layout = check_ops_with_options(options, [tiny_window(1), tiny_window(2)]);
    assert_eq!(layout.windows().count(), 2);
    layout.verify_invariants();
}

#[test]
fn dwindle_columns_keep_positive_window_sizes_across_strut_gap_and_output_changes() {
    for top in [-720., 688., 704., 720., 31783.93378725406] {
        for gaps in [0., 16., 800.] {
            for initially_connected in [false, true] {
                let mut options = Options::default();
                options.layout.struts.top = FloatOrInt(top);
                options.layout.gaps = gaps;
                let mut ops = Vec::new();
                if initially_connected {
                    ops.push(Op::AddOutput(1));
                }
                ops.extend([tiny_window(1), tiny_window(2), tiny_window(3)]);
                // Resize the same Dwindle column through both monitor-set paths.
                ops.extend([
                    Op::AddOutput(1),
                    Op::CompleteAnimations,
                    Op::RemoveOutput(1),
                    Op::AddOutput(1),
                    Op::CompleteAnimations,
                ]);
                let layout = check_ops_with_options(options, ops);
                let windows = layout.windows().collect::<Vec<_>>();
                assert_eq!(windows.len(), 3);
                for (_, window) in windows {
                    let size = window.requested_size().unwrap();
                    assert!(
                        size.w >= 1 && size.h >= 1,
                        "top={top}, gaps={gaps}, size={size:?}"
                    );
                }
                layout.verify_invariants();
            }
        }
    }
}

fn check_new_column_hint_height(column_index: usize) {
    for (top, gaps, expected_height) in [
        (0., 16., 688.),
        (31783.93378725406, 16., 1.),
        (688., 16., 1.),
        (720., 16., 1.),
        (0., 800., 1.),
        (-720., 16., 1408.),
    ] {
        for initially_connected in [false, true] {
            let mut options = Options::default();
            options.layout.struts.top = FloatOrInt(top);
            options.layout.gaps = gaps;
            options.layout.auto_stack_new_windows = false;
            let mut ops = Vec::new();
            if initially_connected {
                ops.push(Op::AddOutput(1));
            }
            ops.extend([tiny_window(1), tiny_window(2)]);
            let layout = check_ops_with_options(options, ops);
            let ws = layout
                .workspaces()
                .find(|(_, _, ws)| ws.has_windows())
                .unwrap()
                .2;
            assert_eq!(ws.scrolling().columns_with_render_positions().count(), 2);
            let hint = ws
                .scrolling()
                .insert_hint_area(InsertPosition::NewColumn(column_index))
                .unwrap();
            assert_eq!(hint.size.w, 300.);
            assert_eq!(
                hint.size.h, expected_height,
                "column={column_index}, top={top}, gaps={gaps}"
            );
            layout.verify_invariants();
        }
    }
}

#[test]
fn new_column_insert_hints_at_workspace_edges_handle_exhausted_working_height() {
    check_new_column_hint_height(0);
    check_new_column_hint_height(2);
}

#[test]
fn new_column_insert_hints_between_columns_handle_exhausted_working_height() {
    check_new_column_hint_height(1);
}
