use super::*;

fn opening_layout(scale: f64, gaps: f64, border: Option<f64>) -> Layout<TestWindow> {
    let mut options = Options::default();
    options.layout.gaps = gaps;
    options.layout.border.off = border.is_none();
    options.layout.border.width = border.unwrap_or_default();
    check_ops_with_options(
        options,
        [Op::AddScaledOutput {
            id: 1,
            scale,
            layout_config: None,
        }],
    )
}

fn opening_size(
    layout: &Layout<TestWindow>,
    rules: &ResolvedWindowRules,
    constraints: (Size<i32, Logical>, Size<i32, Logical>),
    allow_auto_stack: bool,
) -> Size<i32, Logical> {
    let ws = layout.active_workspace().unwrap();
    ws.new_window_size(
        ws.resolve_default_width(rules.default_width, false),
        ws.resolve_default_height(rules.default_height, false),
        false,
        allow_auto_stack,
        rules,
        constraints,
    )
}

#[track_caller]
fn map_predicted_opening(
    layout: &mut Layout<TestWindow>,
    id: usize,
    rules: ResolvedWindowRules,
    constraints: (Size<i32, Logical>, Size<i32, Logical>),
) {
    let predicted = opening_size(layout, &rules, constraints, true);
    let window = TestWindow::new(TestWindowParams {
        bbox: Rectangle::from_size(predicted),
        min_max_size: constraints,
        rules: Some(rules),
        ..TestWindowParams::new(id)
    });
    let (width, height) = {
        let ws = layout.active_workspace().unwrap();
        (
            ws.resolve_default_width(window.rules().default_width, false),
            ws.resolve_default_height(window.rules().default_height, false),
        )
    };
    layout.add_window(
        window.clone(),
        AddWindowTarget::Auto,
        width,
        height,
        false,
        false,
        ActivateWindow::Yes,
    );
    assert_eq!(window.requested_size(), Some(predicted));
    Op::Communicate(id).apply(layout);
    let ws = layout.active_workspace().unwrap();
    let (tile, _, _) = ws
        .tiles_with_render_positions()
        .find(|(tile, _, _)| *tile.window().id() == id)
        .unwrap();
    assert!(
        tile.resize_animation().is_none(),
        "a correctly predicted first buffer must not start a second resize"
    );
    layout.verify_invariants();
}

#[test]
fn auto_stacked_openings_match_their_first_buffer_to_second_and_third_slots() {
    for scale in [1., 1.25, 1.5, 2.] {
        for gaps in [0., 13.3, 16.] {
            for border in [None, Some(1.), Some(2.5)] {
                let mut layout = opening_layout(scale, gaps, border);
                check_ops_on_layout(
                    &mut layout,
                    [
                        Op::AddWindow {
                            params: TestWindowParams::new(1),
                        },
                        Op::Communicate(1),
                        Op::CompleteAnimations,
                    ],
                );
                map_predicted_opening(&mut layout, 2, Default::default(), Default::default());
                map_predicted_opening(&mut layout, 3, Default::default(), Default::default());
            }
        }
    }
}

#[test]
fn auto_stacked_openings_use_existing_constraints_and_new_window_border_rules() {
    for scale in [1., 1.25, 2.] {
        for width_limit in [(400, 0), (1, 300), (80, 500)] {
            let mut layout = opening_layout(scale, 13.3, Some(2.5));
            check_ops_on_layout(
                &mut layout,
                [
                    Op::AddWindow {
                        params: TestWindowParams {
                            min_max_size: (
                                Size::from((width_limit.0, 1)),
                                Size::from((width_limit.1, 0)),
                            ),
                            ..TestWindowParams::new(1)
                        },
                    },
                    Op::Communicate(1),
                    Op::CompleteAnimations,
                ],
            );
            let mut rules = ResolvedWindowRules::default();
            rules.border.width = Some(FloatOrInt(3.3));
            map_predicted_opening(
                &mut layout,
                2,
                rules,
                (Size::from((1, 1)), Size::from((600, 0))),
            );
            map_predicted_opening(&mut layout, 3, Default::default(), Default::default());
        }
    }
}

#[test]
fn auto_stack_opening_prediction_is_read_only_and_stops_at_column_capacity() {
    let mut layout = opening_layout(1.25, 16., Some(2.5));
    check_ops_on_layout(
        &mut layout,
        [
            Op::AddWindow {
                params: TestWindowParams::new(1),
            },
            Op::Communicate(1),
            Op::CompleteAnimations,
        ],
    );
    let before = layout.focus().unwrap().requested_size();
    let rules = ResolvedWindowRules::default();
    let normal = opening_size(&layout, &rules, Default::default(), false);
    let stacked = opening_size(&layout, &rules, Default::default(), true);
    assert_ne!(stacked, normal);
    assert_eq!(layout.focus().unwrap().requested_size(), before);
    assert_eq!(layout.focus().map(LayoutElement::id), Some(&1));
    assert!(!layout.are_transitions_ongoing(None));

    map_predicted_opening(&mut layout, 2, Default::default(), Default::default());
    map_predicted_opening(&mut layout, 3, Default::default(), Default::default());
    assert_eq!(
        opening_size(&layout, &rules, Default::default(), true),
        opening_size(&layout, &rules, Default::default(), false),
        "a full column opens the next window in a new column"
    );
}

#[test]
fn auto_stack_opening_prediction_leaves_expanded_and_independent_layouts_alone() {
    for independent_dwindle in [false, true] {
        let mut layout = opening_layout(1., 16., Some(2.));
        if independent_dwindle {
            Op::SwitchTilingMode(Some(clarion_ipc::TilingMode::Dwindle)).apply(&mut layout);
        }
        check_ops_on_layout(
            &mut layout,
            [
                Op::AddWindow {
                    params: TestWindowParams::new(1),
                },
                Op::Communicate(1),
                Op::CompleteAnimations,
            ],
        );
        if !independent_dwindle {
            Op::FullscreenWindow(1).apply(&mut layout);
        }
        let rules = ResolvedWindowRules::default();
        assert_eq!(
            opening_size(&layout, &rules, Default::default(), true),
            opening_size(&layout, &rules, Default::default(), false)
        );
    }
}

#[test]
fn auto_stack_opening_honors_fixed_client_hints_that_cannot_fit_a_cell() {
    let mut layout = opening_layout(1., 0., None);
    Op::AddWindow {
        params: TestWindowParams::new(1),
    }
    .apply(&mut layout);
    let constraints = (Size::from((1600, 900)), Size::from((1600, 900)));
    let predicted = opening_size(&layout, &ResolvedWindowRules::default(), constraints, true);
    assert_eq!(predicted, (1600, 900).into());
    // The layout's available cell is smaller than this fixed client. Initial
    // configure must keep legal hints; matching an impossible cell is not a
    // reason to advertise an invalid first size or animate the buffer to it.
    assert!(predicted.w > 1280 && predicted.h > 720);
}
