use super::*;

fn membership(layout: &Layout<TestWindow>) -> Vec<(usize, Option<String>)> {
    let mut full = Vec::new();
    layout.with_windows(|window, output, _, _| {
        full.push((*window.id(), output.map(Output::name)));
    });
    let mut lightweight = Vec::new();
    layout.with_windows_and_outputs(|window, output| {
        lightweight.push((*window.id(), output.map(Output::name)));
    });
    assert_eq!(
        lightweight, full,
        "membership, ordering and outputs must match"
    );
    let mut unique = std::collections::HashSet::new();
    assert!(lightweight.iter().all(|(id, _)| unique.insert(*id)));
    lightweight
}

fn mixed_layout(mode: realand_ipc::TilingMode) -> Layout<TestWindow> {
    let mut options = Options::default();
    options.layout.auto_stack_new_windows = false;
    let mut layout = check_ops_with_options(
        options,
        [
            Op::AddOutput(1),
            Op::SwitchTilingMode(Some(mode)),
            Op::AddWindow {
                params: TestWindowParams::new(1),
            },
            Op::AddWindow {
                params: TestWindowParams::new(2),
            },
            Op::AddWindow {
                params: TestWindowParams::new(3),
            },
            Op::AddWindow {
                params: TestWindowParams {
                    is_floating: true,
                    ..TestWindowParams::new(4)
                },
            },
            Op::AddWindow {
                params: TestWindowParams {
                    is_floating: true,
                    ..TestWindowParams::new(5)
                },
            },
            Op::FocusWorkspaceDown,
            Op::AddWindow {
                params: TestWindowParams::new(6),
            },
            Op::AddOutput(2),
            Op::FocusOutput(2),
            Op::SwitchTilingMode(Some(mode)),
            Op::AddWindow {
                params: TestWindowParams::new(7),
            },
            Op::AddWindow {
                params: TestWindowParams::new(8),
            },
            Op::CompleteAnimations,
        ],
    );
    assert!(layout.minimize_window(&3));
    Op::CompleteAnimations.apply(&mut layout);
    layout.verify_invariants();
    layout
}

#[test]
fn lightweight_membership_keeps_ipc_order_and_parked_windows() {
    for mode in [
        realand_ipc::TilingMode::Scrolling,
        realand_ipc::TilingMode::Dwindle,
    ] {
        let mut layout = mixed_layout(mode);
        let members = membership(&layout);
        assert_eq!(members.len(), 8);
        assert_eq!(
            members.iter().map(|x| x.0).collect::<Vec<_>>(),
            vec![5, 4, 1, 2, 3, 6, 7, 8]
        );
        assert!(members[..6]
            .iter()
            .all(|x| x.1.as_deref() == Some("output1")));
        assert!(members[6..]
            .iter()
            .all(|x| x.1.as_deref() == Some("output2")));

        // Expanded Dwindle leaves and inactive/offscreen columns stay addressable.
        layout.set_fullscreen(&8, true);
        check_ops_on_layout(&mut layout, [Op::Communicate(8), Op::CompleteAnimations]);
        assert_eq!(membership(&layout), members);
    }
}

#[test]
fn lightweight_membership_keeps_inactive_tabs_and_disconnected_workspaces() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleColumnTabbedDisplay,
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddNamedWorkspace {
            ws_name: 1,
            output_name: Some(1),
            layout_config: None,
        },
        Op::AddWindowToNamedWorkspace {
            params: TestWindowParams::new(3),
            ws_name: 1,
        },
        Op::AddOutput(2),
        Op::FocusOutput(2),
        Op::AddWindow {
            params: TestWindowParams::new(4),
        },
        Op::CompleteAnimations,
    ]);
    assert!(layout.minimize_window(&2));
    assert_eq!(membership(&layout).len(), 4);

    for output in [2, 1] {
        check_ops_on_layout(&mut layout, [Op::RemoveOutput(output)]);
        let members = membership(&layout);
        assert_eq!(members.len(), 4);
        if output == 1 {
            assert!(members.iter().all(|x| x.1.is_none()));
        } else {
            assert!(members.iter().all(|x| x.1.as_deref() == Some("output1")));
        }
    }
    check_ops_on_layout(&mut layout, [Op::AddOutput(1), Op::AddOutput(2)]);
    assert_eq!(membership(&layout).len(), 4);
}

#[test]
fn lightweight_membership_attributes_detached_move_once_to_its_current_output() {
    let mut layout = mixed_layout(realand_ipc::TilingMode::Scrolling);
    check_ops_on_layout(
        &mut layout,
        [
            Op::FocusWindow(1),
            Op::CompleteAnimations,
            Op::InteractiveMoveBegin {
                window: 1,
                output_idx: 1,
                px: 10.,
                py: 10.,
            },
            Op::InteractiveMoveUpdate {
                window: 1,
                dx: 1000.,
                dy: 0.,
                output_idx: 2,
                px: 100.,
                py: 100.,
            },
        ],
    );
    assert!(matches!(
        layout.interactive_move,
        Some(InteractiveMoveState::Moving(_))
    ));
    let members = membership(&layout);
    assert_eq!(members.len(), 8);
    assert_eq!(members[0], (1, Some("output2".into())));
    check_ops_on_layout(&mut layout, [Op::InteractiveMoveEnd { window: 1 }]);
    assert_eq!(membership(&layout).len(), 8);
}

#[test]
fn lightweight_membership_does_not_read_window_geometry() {
    for mode in [
        realand_ipc::TilingMode::Scrolling,
        realand_ipc::TilingMode::Dwindle,
    ] {
        let mut layout = mixed_layout(mode);
        for disconnect in [false, true] {
            if disconnect {
                check_ops_on_layout(&mut layout, [Op::RemoveOutput(2), Op::RemoveOutput(1)]);
            }
            let windows = layout.windows().map(|(_, w)| w.clone()).collect::<Vec<_>>();
            for window in &windows {
                window.0.size_reads.set(Some(0));
            }
            let mut full_visits = 0;
            layout.with_windows(|_, _, _, _| full_visits += 1);
            assert_eq!(full_visits, 8);
            assert!(windows
                .iter()
                .all(|window| window.0.size_reads.get().unwrap() > 0));
            for window in &windows {
                window.0.size_reads.set(Some(0));
            }
            let mut lightweight_visits = 0;
            layout.with_windows_and_outputs(|window, output| {
                std::hint::black_box((window.id(), output));
                lightweight_visits += 1;
            });
            assert_eq!(lightweight_visits, full_visits);
            assert!(windows
                .iter()
                .all(|window| window.0.size_reads.get() == Some(0)));
            for window in &windows {
                window.0.size_reads.set(None);
            }
        }
    }
}

#[test]
#[ignore = "opt-in CPU traversal microbenchmark; excludes rendering and presentation"]
fn lightweight_window_traversal_microbenchmark() {
    use std::hint::black_box;
    use std::time::Instant;

    let mut results = Vec::new();
    for mode in [
        realand_ipc::TilingMode::Scrolling,
        realand_ipc::TilingMode::Dwindle,
    ] {
        for count in [16, 64, 128] {
            let mut options = Options::default();
            options.layout.auto_stack_new_windows = false;
            let mut layout = check_ops_with_options(
                options,
                [Op::AddOutput(1), Op::SwitchTilingMode(Some(mode))],
            );
            for id in 0..count {
                let mut params = TestWindowParams::new(id);
                params.is_floating = id % 4 == 0;
                Op::AddWindow { params }.apply(&mut layout);
            }
            Op::CompleteAnimations.apply(&mut layout);
            assert_eq!(membership(&layout).len(), count);
            let rounds = 2000;
            let mut samples = [Vec::new(), Vec::new()];
            let mut visits = [0usize; 2];
            // Alternate order so one traversal does not always get the warm cache.
            for sample in 0..7 {
                for method in [sample % 2, 1 - sample % 2] {
                    let start = Instant::now();
                    for _ in 0..rounds {
                        if method == 0 {
                            black_box(&layout).with_windows(
                                |window, output, workspace, geometry| {
                                    black_box((window.id(), output, workspace, geometry));
                                    visits[method] += 1;
                                },
                            );
                        } else {
                            black_box(&layout).with_windows_and_outputs(|window, output| {
                                black_box((window.id(), output));
                                visits[method] += 1;
                            });
                        }
                    }
                    samples[method].push(start.elapsed().as_nanos() as u64);
                }
            }
            assert_eq!(visits[0], visits[1]);
            for times in &mut samples {
                times.sort_unstable();
            }
            results.push(serde_json::json!({
                "mode": format!("{mode:?}"), "windows": count, "rounds_per_sample": rounds,
                "samples": 7, "visits_per_method": visits[0],
                "full_median_ns": samples[0][3], "lightweight_median_ns": samples[1][3],
                "geometry_counters_enabled": false,
            }));
        }
    }
    println!(
        "PERFORMANCE{}",
        serde_json::json!({"benchmark": "window_membership_traversal", "results": results})
    );
}
