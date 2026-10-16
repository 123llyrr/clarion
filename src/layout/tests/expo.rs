use super::*;

fn opening_expo(scale: f64) -> Layout<TestWindow> {
    let mut options = Options::default();
    options.animations.overview_open_close.0.kind =
        clarion_config::animations::Kind::Easing(clarion_config::animations::EasingParams {
            duration_ms: 1000,
            curve: clarion_config::animations::Curve::Linear,
        });
    let mut ops = vec![Op::AddScaledOutput {
        id: 1,
        scale,
        layout_config: None,
    }];
    for id in (1..=3).rev() {
        ops.push(Op::AddNamedWorkspace {
            ws_name: id,
            output_name: Some(1),
            layout_config: None,
        });
        ops.push(Op::AddWindowToNamedWorkspace {
            params: TestWindowParams::new(id),
            ws_name: id,
        });
    }
    ops.extend([Op::FocusWorkspace(0), Op::CompleteAnimations]);
    let mut layout = check_ops_with_options(options, ops);
    layout.toggle_expo();
    layout
}

fn rects(layout: &Layout<TestWindow>) -> Vec<Rectangle<f64, Logical>> {
    let mon = layout.active_monitor_ref().unwrap();
    mon.workspaces_render_geo()
        .take(mon.workspaces.len())
        .collect()
}

#[test]
fn expo_spring_reversal_preserves_card_velocity_instead_of_freezing() {
    for scale in [1., 1.25, 1.5, 2.] {
        for closing in [false, true] {
            let mut layout = opening_expo(scale);
            let config = clarion_config::Animations::default().overview_open_close.0;
            let mut options = Options::clone(&layout.options);
            options.animations.overview_open_close.0 = config;
            layout.update_options(options);
            if let Some(OverviewProgress::Animation(animation)) = &mut layout.overview_progress {
                animation.replace_config(config);
            }
            layout.set_monitors_overview_state();
            if closing {
                Op::CompleteAnimations.apply(&mut layout);
                layout.toggle_expo();
            }
            let start = layout.clock.now_unadjusted();
            let active = layout.active_monitor_ref().unwrap().active_workspace_idx();
            layout
                .clock
                .set_unadjusted(start + Duration::from_millis(79));
            layout.advance_animations();
            let before = rects(&layout)[active].size.w;
            layout
                .clock
                .set_unadjusted(start + Duration::from_millis(80));
            layout.advance_animations();
            let at = rects(&layout)[active].size.w;
            layout.toggle_expo();
            assert_eq!(rects(&layout)[active].size.w, at);
            layout
                .clock
                .set_unadjusted(start + Duration::from_millis(81));
            layout.advance_animations();
            let after = rects(&layout)[active].size.w;
            let velocity_ratio = (after - at) / (at - before);
            assert!(
                (0.75..1.25).contains(&velocity_ratio),
                "reversing must retain momentum: scale={scale}, closing={closing}, velocity ratio={velocity_ratio}"
            );
            Op::CompleteAnimations.apply(&mut layout);
            layout.verify_invariants();
        }
    }
}

#[test]
fn spring_overview_retargeting_keeps_finite_cards_and_finishes_at_both_refresh_rates() {
    for mode in [OverviewMode::Stage, OverviewMode::Expo] {
        for scale in [1., 1.25, 1.5, 2.] {
            for hz in [60, 144] {
                let mut layout = opening_expo(scale);
                let config = clarion_config::Animations::default().overview_open_close.0;
                let mut options = Options::clone(&layout.options);
                options.animations.overview_open_close.0 = config;
                layout.update_options(options);
                Op::CompleteAnimations.apply(&mut layout);
                layout.close_overview();
                Op::CompleteAnimations.apply(&mut layout);
                let toggle = |layout: &mut Layout<TestWindow>| match mode {
                    OverviewMode::Stage => layout.toggle_overview(),
                    OverviewMode::Expo => layout.toggle_expo(),
                    _ => unreachable!(),
                };
                toggle(&mut layout);
                let tick = Duration::from_secs_f64(1. / f64::from(hz));
                for frame in 0..360 {
                    // Re-enter before the closing spring can turn around,
                    // including after selecting a different desktop anchor.
                    if [1, 3, 4, 7, 8, 12, 14].contains(&frame) {
                        let before = rects(&layout);
                        toggle(&mut layout);
                        assert_eq!(rects(&layout), before, "{mode:?}, {scale}, {hz} Hz");
                    }
                    if frame == 20 {
                        if !layout.is_overview_open() {
                            toggle(&mut layout);
                        }
                        match mode {
                            OverviewMode::Stage => {
                                layout.stage_navigate(None, 1);
                                layout.confirm_stage();
                            }
                            OverviewMode::Expo => {
                                layout.expo_navigate(1, 0);
                                layout.confirm_expo();
                            }
                            _ => unreachable!(),
                        }
                    }
                    layout
                        .clock
                        .set_unadjusted(layout.clock.now_unadjusted() + tick);
                    layout.advance_animations();
                    let mon = layout.active_monitor_ref().unwrap();
                    for rect in rects(&layout) {
                        for value in [rect.loc.x, rect.loc.y, rect.size.w, rect.size.h] {
                            assert!(value.is_finite(), "{mode:?}, {scale}, {hz} Hz, {rect:?}");
                        }
                        assert!(rect.size.w >= 0. && rect.size.h >= 0.);
                        if layout.is_overview_visible() {
                            assert!(rect.size.w <= mon.view_size().w * 2., "{mode:?}: {rect:?}");
                            assert!(rect.size.h <= mon.view_size().h * 2., "{mode:?}: {rect:?}");
                        }
                    }
                }
                assert!(!layout.are_animations_ongoing(None));
                layout.verify_invariants();
            }
        }
    }
}

#[test]
fn spring_early_anchor_change_cannot_magnify_or_invert_workspace_cards() {
    for mode in [OverviewMode::Stage, OverviewMode::Expo] {
        for millis in [1, 2, 4, 8] {
            let mut layout = opening_expo(2.);
            let mut options = Options::clone(&layout.options);
            options.animations.overview_open_close =
                clarion_config::Animations::default().overview_open_close;
            layout.update_options(options);
            Op::CompleteAnimations.apply(&mut layout);
            layout.close_overview();
            Op::CompleteAnimations.apply(&mut layout);
            match mode {
                OverviewMode::Stage => layout.toggle_overview(),
                OverviewMode::Expo => layout.toggle_expo(),
                _ => unreachable!(),
            }
            layout.clock.set_unadjusted(Duration::from_millis(millis));
            layout.advance_animations();
            let before = rects(&layout);
            match mode {
                OverviewMode::Stage => {
                    layout.stage_navigate(None, 1);
                    layout.confirm_stage();
                }
                OverviewMode::Expo => {
                    layout.expo_navigate(1, 0);
                    layout.confirm_expo();
                }
                _ => unreachable!(),
            }
            assert_eq!(rects(&layout), before);
            for tick in 1..=100 {
                layout
                    .clock
                    .set_unadjusted(Duration::from_millis(millis + tick));
                layout.advance_animations();
                let view = layout.active_monitor_ref().unwrap().view_size();
                for rect in rects(&layout) {
                    assert!(
                        (0. ..=view.w).contains(&rect.size.w),
                        "{mode:?}, {millis} ms: {rect:?}"
                    );
                    assert!(
                        (0. ..=view.h).contains(&rect.size.h),
                        "{mode:?}, {millis} ms: {rect:?}"
                    );
                }
            }
            Op::CompleteAnimations.apply(&mut layout);
            layout.verify_invariants();
        }
    }
}

#[test]
fn opening_expo_navigation_keeps_every_card_geometry_at_the_same_animation_time() {
    for scale in [1., 1.25, 1.5] {
        for millis in [100, 400, 800] {
            let mut layout = opening_expo(scale);
            layout.clock.set_unadjusted(Duration::from_millis(millis));
            layout.advance_animations();
            let progress = layout
                .active_monitor_ref()
                .unwrap()
                .overview_progress_value()
                .unwrap();
            assert!(progress > 0. && progress < 1.);
            let before = rects(&layout);

            assert!(layout.expo_navigate(1, 0));
            assert_eq!(
                layout.active_monitor_ref().unwrap().active_workspace_idx(),
                1
            );
            assert_eq!(rects(&layout), before, "scale={scale}, time={millis}ms");
            assert!(layout.expo_navigate(0, 1));
            assert_eq!(
                layout.active_monitor_ref().unwrap().active_workspace_idx(),
                3
            );
            assert_eq!(rects(&layout), before, "scale={scale}, time={millis}ms");
            layout.verify_invariants();
        }
    }
}

#[test]
fn closing_expo_expands_the_confirmed_workspace_after_navigation() {
    for scale in [1., 1.25, 1.5] {
        let mut layout = opening_expo(scale);
        Op::CompleteAnimations.apply(&mut layout);
        let grid = rects(&layout);
        assert!(layout.expo_navigate(1, 0));
        let selected = layout.active_workspace().unwrap().id();
        assert!(layout.confirm_expo());
        assert_eq!(rects(&layout), grid);

        layout.clock.set_unadjusted(Duration::from_millis(400));
        layout.advance_animations();
        let mon = layout.active_monitor_ref().unwrap();
        let progress = mon.overview_progress_value().unwrap();
        assert!((progress - 0.6).abs() < 1e-12);
        let geometry = rects(&layout);
        let selected_index = mon.idx_of_ws(selected).unwrap();
        let view = mon.view_size();
        let expected: Size<f64, Logical> = Size::from((
            view.w * (1. - progress) + grid[selected_index].size.w * progress,
            view.h * (1. - progress) + grid[selected_index].size.h * progress,
        ));
        approx::assert_abs_diff_eq!(geometry[selected_index].size.w, expected.w, epsilon = 1e-9);
        approx::assert_abs_diff_eq!(geometry[selected_index].size.h, expected.h, epsilon = 1e-9);
        let old_origin_size = grid[0].size.upscale(progress);
        approx::assert_abs_diff_eq!(geometry[0].size.w, old_origin_size.w, epsilon = 1e-9);
        approx::assert_abs_diff_eq!(geometry[0].size.h, old_origin_size.h, epsilon = 1e-9);

        Op::CompleteAnimations.apply(&mut layout);
        assert_eq!(layout.active_workspace().unwrap().id(), selected);
        layout.verify_invariants();
    }
}

#[test]
fn expo_early_confirmation_and_reopening_preserve_the_current_card_geometry() {
    for scale in [1., 1.25, 1.5] {
        for millis in [100, 400, 800] {
            let mut layout = opening_expo(scale);
            layout.clock.set_unadjusted(Duration::from_millis(millis));
            layout.advance_animations();
            assert!(layout.expo_navigate(1, 0));
            let selected = layout.active_workspace().unwrap().id();
            let opening = rects(&layout);
            assert!(layout.confirm_expo());
            assert_eq!(
                rects(&layout),
                opening,
                "confirm: scale={scale}, time={millis}ms"
            );

            layout
                .clock
                .set_unadjusted(Duration::from_millis(millis + 40));
            layout.advance_animations();
            let closing = rects(&layout);
            assert_ne!(closing, opening);
            layout.toggle_expo();
            assert_eq!(
                rects(&layout),
                closing,
                "reopen: scale={scale}, time={millis}ms"
            );
            layout
                .clock
                .set_unadjusted(Duration::from_millis(millis + 80));
            layout.advance_animations();
            assert_ne!(rects(&layout), closing);
            assert_eq!(layout.active_workspace().unwrap().id(), selected);
            Op::CompleteAnimations.apply(&mut layout);
            assert!(layout.confirm_expo());
            Op::CompleteAnimations.apply(&mut layout);
            assert_eq!(layout.active_workspace().unwrap().id(), selected);
            layout.verify_invariants();
        }
    }
}

#[test]
fn closing_expo_gesture_takeover_tracks_both_directions_without_jumping() {
    for scale in [1., 1.25, 1.5] {
        for reopen in [false, true] {
            let mut layout = opening_expo(scale);
            Op::CompleteAnimations.apply(&mut layout);
            let grid = rects(&layout);
            assert!(layout.expo_navigate(1, 0));
            let selected = layout.active_workspace().unwrap().id();
            assert!(layout.confirm_expo());
            layout.clock.set_unadjusted(Duration::from_millis(400));
            layout.advance_animations();
            let selected_index = layout
                .active_monitor_ref()
                .unwrap()
                .idx_of_ws(selected)
                .unwrap();
            let closing = rects(&layout);

            layout.overview_gesture_begin();
            assert_eq!(rects(&layout), closing, "gesture begin: scale={scale}");
            assert_eq!(
                layout.overview_gesture_update(-60., Duration::from_millis(400)),
                Some(true)
            );
            let lower = rects(&layout);
            assert!(lower[selected_index].size.w > closing[selected_index].size.w);
            approx::assert_abs_diff_eq!(
                layout
                    .active_monitor_ref()
                    .unwrap()
                    .overview_progress_value()
                    .unwrap(),
                0.4,
                epsilon = 1e-12
            );
            layout.clock.set_unadjusted(Duration::from_millis(416));
            layout.advance_animations();
            assert_eq!(rects(&layout), lower, "stationary gesture: scale={scale}");

            layout.overview_gesture_update(30., Duration::from_millis(420));
            let middle = rects(&layout);
            assert!(middle[selected_index].size.w < lower[selected_index].size.w);
            layout.overview_gesture_update(90., Duration::from_millis(440));
            let upper = rects(&layout);
            assert!(upper[selected_index].size.w < closing[selected_index].size.w);
            layout.overview_gesture_update(-120., Duration::from_millis(460));
            let returned = rects(&layout);
            for (actual, expected) in returned.iter().zip(&lower) {
                assert_eq!(actual.loc, expected.loc);
                approx::assert_abs_diff_eq!(actual.size.w, expected.size.w, epsilon = 1e-9);
                approx::assert_abs_diff_eq!(actual.size.h, expected.size.h, epsilon = 1e-9);
            }
            if reopen {
                layout.overview_gesture_update(120., Duration::from_millis(480));
            }

            // An idle history window makes the release target depend on position.
            layout.clock.set_unadjusted(Duration::from_millis(700));
            layout.advance_animations();
            let released = rects(&layout);
            assert!(layout.overview_gesture_end());
            assert_eq!(layout.is_overview_open(), reopen);
            assert_eq!(rects(&layout), released, "gesture release: scale={scale}");
            layout.clock.set_unadjusted(Duration::from_millis(716));
            layout.advance_animations();
            let moving = rects(&layout);
            if reopen {
                assert!(moving[selected_index].size.w < released[selected_index].size.w);
            } else {
                assert!(moving[selected_index].size.w > released[selected_index].size.w);
            }

            Op::CompleteAnimations.apply(&mut layout);
            assert_eq!(layout.active_workspace().unwrap().id(), selected);
            if reopen {
                assert_eq!(rects(&layout), grid);
            } else {
                let mon = layout.active_monitor_ref().unwrap();
                assert!(mon.overview_progress_value().is_none());
                assert_eq!(rects(&layout)[selected_index].size, mon.view_size());
            }
            layout.verify_invariants();
        }
    }
}

#[test]
fn newly_connected_outputs_inherit_the_overview_mode_during_the_transition() {
    for mode in [
        OverviewMode::Stage,
        OverviewMode::Expo,
        OverviewMode::Windows,
        OverviewMode::Strip,
    ] {
        let mut layout = opening_expo(1.25);
        layout.clock.set_unadjusted(Duration::from_millis(400));
        layout.advance_animations();
        match mode {
            OverviewMode::Stage => layout.toggle_overview(),
            OverviewMode::Expo => (),
            OverviewMode::Windows => layout.toggle_window_spread(),
            OverviewMode::Strip => {
                let mut options = (*layout.options).clone();
                options.overview.style = clarion_config::OverviewStyle::Strip;
                layout.update_options(options);
                layout.toggle_overview();
            }
        }
        check_ops_on_layout(&mut layout, [Op::AddOutput(2)]);
        let new_monitor = layout
            .monitors()
            .find(|mon| mon.output().name() == "output2")
            .unwrap();
        assert_eq!(new_monitor.overview_mode(), mode);
        assert_eq!(new_monitor.overview_progress_value(), Some(0.4));

        check_ops_on_layout(
            &mut layout,
            [Op::RemoveOutput(1), Op::RemoveOutput(2), Op::AddOutput(1)],
        );
        let mon = layout.active_monitor_ref().unwrap();
        assert_eq!(mon.overview_mode(), mode);
        assert_eq!(mon.overview_progress_value(), Some(0.4));
        layout.verify_invariants();
    }
}

#[test]
fn closing_expo_retargets_workspace_switches_without_a_geometry_jump() {
    for scale in [1., 1.25, 1.5] {
        let mut layout = opening_expo(scale);
        layout.clock.set_unadjusted(Duration::from_millis(400));
        layout.advance_animations();
        assert!(layout.expo_navigate(1, 0));
        assert!(layout.confirm_expo());
        layout.clock.set_unadjusted(Duration::from_millis(440));
        layout.advance_animations();
        let before = rects(&layout);
        let Some(OverviewProgress::Animation(closing)) = &layout.overview_progress else {
            unreachable!();
        };
        let incoming_progress = closing.value();
        let incoming_velocity = closing.velocity();
        layout.switch_workspace(2);
        let selected = layout.active_workspace().unwrap().id();
        layout.advance_animations();
        assert_eq!(rects(&layout), before, "scale={scale}");
        let Some(OverviewProgress::Animation(closing)) = &layout.overview_progress else {
            unreachable!();
        };
        assert_eq!(closing.value(), incoming_progress);
        assert_eq!(closing.velocity(), incoming_velocity);

        layout.clock.set_unadjusted(Duration::from_millis(700));
        layout.advance_animations();
        // Momentum is retained before the turn, rather than instantaneously
        // reversing the preset slope. The view still follows the active curve.
        let Some(OverviewProgress::Animation(closing)) = &layout.overview_progress else {
            unreachable!();
        };
        assert!(closing.value() > incoming_progress);
        assert!(rects(&layout)
            .iter()
            .all(|rect| rect.size.w >= 0. && rect.size.h >= 0.));
        layout.clock.set_unadjusted(Duration::from_millis(900));
        layout.advance_animations();
        let mon = layout.active_monitor_ref().unwrap();
        let selected_index = mon.idx_of_ws(selected).unwrap();
        assert!(rects(&layout)[selected_index].size.w > before[selected_index].size.w);
        Op::CompleteAnimations.apply(&mut layout);
        let mon = layout.active_monitor_ref().unwrap();
        assert_eq!(layout.active_workspace().unwrap().id(), selected);
        assert!(mon.overview_progress_value().is_none());
        assert_eq!(
            rects(&layout)[mon.idx_of_ws(selected).unwrap()].size,
            mon.view_size()
        );
        layout.verify_invariants();
    }
}

#[test]
fn expo_opening_origin_follows_workspace_identity_through_reordering_and_removal() {
    let mut layout = opening_expo(1.25);
    let origin = layout.active_workspace().unwrap().id();
    layout.clock.set_unadjusted(Duration::from_millis(400));
    layout.advance_animations();
    assert!(layout.expo_navigate(1, 0));
    assert!(layout.expo_navigate(0, 1));
    check_ops_on_layout(
        &mut layout,
        [Op::AddNamedWorkspace {
            ws_name: 4,
            output_name: Some(1),
            layout_config: None,
        }],
    );
    let largest_card = |layout: &Layout<TestWindow>| {
        let mon = layout.active_monitor_ref().unwrap();
        mon.workspaces
            .iter()
            .zip(rects(layout))
            .max_by(|(_, a), (_, b)| a.size.w.total_cmp(&b.size.w))
            .unwrap()
            .0
            .id()
    };
    assert_eq!(largest_card(&layout), origin);
    let before_active = layout.active_monitor_ref().unwrap().active_workspace_idx();
    let before_count = rects(&layout).len();
    check_ops_on_layout(
        &mut layout,
        [Op::MoveWorkspaceToIndex {
            ws_name: Some(1),
            target_idx: 2,
        }],
    );
    assert_eq!(
        layout.active_monitor_ref().unwrap().active_workspace_idx(),
        before_active
    );
    assert_eq!(rects(&layout).len(), before_count);
    assert_eq!(largest_card(&layout), origin);

    check_ops_on_layout(
        &mut layout,
        [
            Op::AddOutput(2),
            Op::MoveWorkspaceToMonitor {
                ws_name: Some(1),
                output_id: 2,
            },
        ],
    );
    let mon = layout.active_monitor_ref().unwrap();
    assert!(mon.idx_of_ws(origin).is_none());
    let geometry = rects(&layout);
    // Removing the original card must not turn another workspace at its old index into
    // the full-size origin. All remaining cards now use their collapsed grid origins.
    assert!(geometry.iter().all(|rect| rect.size == geometry[0].size));
    assert!(layout.expo_navigate(-1, 0));
    assert_eq!(rects(&layout), geometry);
    assert!(layout.confirm_expo());
    Op::CompleteAnimations.apply(&mut layout);
    layout.verify_invariants();
}

#[test]
fn reversing_expo_keeps_captured_cards_bound_to_workspace_ids() {
    let mut layout = opening_expo(1.5);
    layout.clock.set_unadjusted(Duration::from_millis(400));
    layout.advance_animations();
    assert!(layout.expo_navigate(1, 0));
    assert!(layout.confirm_expo());
    let captured = layout
        .active_monitor_ref()
        .unwrap()
        .workspaces
        .iter()
        .map(Workspace::id)
        .zip(rects(&layout))
        .collect::<std::collections::HashMap<_, _>>();
    check_ops_on_layout(
        &mut layout,
        [Op::MoveWorkspaceToIndex {
            ws_name: Some(1),
            target_idx: 2,
        }],
    );
    let current = layout
        .active_monitor_ref()
        .unwrap()
        .workspaces
        .iter()
        .map(Workspace::id)
        .zip(rects(&layout))
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(current, captured);
    layout.toggle_expo();
    let reopened = layout
        .active_monitor_ref()
        .unwrap()
        .workspaces
        .iter()
        .map(Workspace::id)
        .zip(rects(&layout))
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(reopened, captured);
    layout.verify_invariants();
}
