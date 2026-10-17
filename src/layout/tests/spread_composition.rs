use smithay::backend::renderer::element::Element as _;
use smithay::input::keyboard::Keysym;

use super::*;

fn projected_spread_layout(
    mode: clarion_ipc::TilingMode,
    floating: bool,
    scale: f64,
) -> Layout<TestWindow> {
    let mut options = Options::default();
    options.layout.auto_stack_new_windows = false;
    options.animations.overview_open_close.0.kind =
        clarion_config::animations::Kind::Easing(clarion_config::animations::EasingParams {
            duration_ms: 1000,
            curve: clarion_config::animations::Curve::Linear,
        });
    check_ops_with_options(
        options,
        [
            Op::AddScaledOutput {
                id: 1,
                scale,
                layout_config: None,
            },
            Op::SwitchTilingMode(Some(mode)),
            Op::AddWindow {
                params: TestWindowParams {
                    is_floating: floating,
                    ..TestWindowParams::new(1)
                },
            },
            Op::AddWindow {
                params: TestWindowParams {
                    is_floating: floating,
                    ..TestWindowParams::new(2)
                },
            },
            Op::FocusWorkspaceDown,
            Op::AddWindow {
                params: TestWindowParams::new(3),
            },
            Op::FocusWorkspace(0),
            Op::CompleteAnimations,
        ],
    )
}

fn projected_tiles(layout: &Layout<TestWindow>) -> Vec<(usize, Rectangle<f64, Logical>, f64)> {
    let monitor = layout.active_monitor_ref().unwrap();
    let composition = monitor.spread_composition();
    composition
        .tiles
        .iter()
        .zip(&composition.geometry.tiles)
        .map(|(source, placement)| (*source.window().id(), placement.geometry, placement.zoom))
        .collect()
}

#[test]
fn spread_scene_freezes_geometry_focus_backgrounds_and_lower_layer_namespaces() {
    for mode in [
        clarion_ipc::TilingMode::Scrolling,
        clarion_ipc::TilingMode::Dwindle,
    ] {
        for floating in [false, true] {
            for scale in [1., 1.25, 2.] {
                let mut layout = projected_spread_layout(mode, floating, scale);
                layout.toggle_window_spread();
                for complete in [false, true] {
                    if complete {
                        Op::CompleteAnimations.apply(&mut layout);
                    } else {
                        layout.clock.set_unadjusted(Duration::from_millis(400));
                        layout.advance_animations();
                    }
                    let monitor = layout.active_monitor_ref().unwrap();
                    let composition = monitor.spread_composition();
                    assert_eq!(composition.geometry.scale, scale);
                    assert_eq!(composition.geometry.view_size, monitor.view_size());
                    assert_eq!(composition.tiles.len(), composition.geometry.tiles.len());
                    assert_eq!(
                        composition.closing.len(),
                        composition.geometry.closing.len()
                    );
                    assert_eq!(composition.workspaces.len(), monitor.workspaces.len());
                    let live_tiles = monitor.spread_tiles();
                    for (source, placement) in
                        composition.tiles.iter().zip(&composition.geometry.tiles)
                    {
                        let (_, expected_geometry, expected_zoom) = live_tiles
                            .iter()
                            .find(|(tile, _, _)| tile.window().id() == source.window().id())
                            .unwrap();
                        assert_eq!(placement.geometry, *expected_geometry);
                        assert_eq!(placement.zoom, *expected_zoom);
                        assert_eq!(
                            placement.focused,
                            monitor.spread_selected().as_ref() == Some(source.window().id())
                        );
                    }
                    let captured: Vec<_> = composition.workspace_scenes().collect();
                    let live: Vec<_> = monitor.workspaces_with_render_geo().collect();
                    assert_eq!(captured.len(), live.len());
                    for (captured, (workspace, geometry)) in captured.iter().zip(live) {
                        let background = workspace.render_background();
                        assert_eq!(captured.namespace, workspace.id().get() as usize);
                        assert_eq!(captured.geometry, geometry);
                        assert_eq!(captured.zoom, monitor.workspace_render_zoom(geometry));
                        assert_eq!(captured.background.id(), background.id());
                        assert_eq!(
                            captured.background.current_commit(),
                            background.current_commit()
                        );
                        assert_eq!(captured.background.color(), background.color());
                    }
                    layout.verify_invariants();
                }
            }
        }
    }
}

#[test]
fn spread_scene_keeps_source_order_and_captured_poses_across_close_reversal() {
    for scale in [1., 1.25, 2.] {
        let mut layout = projected_spread_layout(clarion_ipc::TilingMode::Scrolling, true, scale);
        layout.toggle_window_spread();
        layout.clock.set_unadjusted(Duration::from_millis(400));
        layout.advance_animations();
        let opening = projected_tiles(&layout);
        layout.spread_key(Keysym::Escape);
        assert_eq!(projected_tiles(&layout), opening);
        layout.clock.set_unadjusted(Duration::from_millis(450));
        layout.advance_animations();
        let closing = projected_tiles(&layout);
        assert_ne!(closing, opening);
        layout.toggle_window_spread();
        assert_eq!(projected_tiles(&layout), closing);
        layout.verify_invariants();
    }
}

#[test]
fn spread_scene_retains_minimized_sources_search_selection_and_returning_windows() {
    for mode in [
        clarion_ipc::TilingMode::Scrolling,
        clarion_ipc::TilingMode::Dwindle,
    ] {
        let mut layout = projected_spread_layout(mode, false, 1.25);
        assert!(layout.minimize_window(&2));
        Op::CompleteAnimations.apply(&mut layout);
        layout.toggle_window_spread();
        Op::CompleteAnimations.apply(&mut layout);
        let mut ids = projected_tiles(&layout)
            .into_iter()
            .map(|(id, _, _)| id)
            .collect::<Vec<_>>();
        ids.sort_unstable();
        assert_eq!(ids, [1, 2, 3]);
        for ch in "app2".chars() {
            layout.spread_key(Keysym::from_char(ch));
        }
        let selected = projected_tiles(&layout);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].0, 2);
        {
            let monitor = layout.active_monitor_ref().unwrap();
            let composition = monitor.spread_composition();
            assert!(composition.geometry.tiles[0].focused);
            assert!(composition.title.contains("app2"));
        }
        layout.spread_key(Keysym::Return);
        assert_eq!(layout.active_window().unwrap().id(), &2);
        assert!(layout.is_window_spread_closing());
        let returning = projected_tiles(&layout);
        assert_eq!(
            returning.iter().find(|(id, _, _)| *id == 2),
            Some(&selected[0])
        );
        assert!(returning
            .iter()
            .filter(|(id, _, _)| *id != 2)
            .all(|(_, _, zoom)| *zoom == 0.));
        layout.verify_invariants();
    }
}
