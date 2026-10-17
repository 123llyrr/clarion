use super::*;
use crate::scene::workspace_composition::{Channel, Item, TileSpace};

fn projected_stage_layout(scale: f64, mode: clarion_ipc::TilingMode) -> Layout<TestWindow> {
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
    for workspace in (1..=5).rev() {
        ops.push(Op::AddNamedWorkspace {
            ws_name: workspace,
            output_name: Some(1),
            layout_config: None,
        });
        for offset in 0..3 {
            ops.push(Op::AddWindowToNamedWorkspace {
                params: TestWindowParams {
                    is_floating: offset == 2,
                    ..TestWindowParams::new(workspace * 10 + offset)
                },
                ws_name: workspace,
            });
        }
    }
    ops.extend([Op::FocusWorkspace(3), Op::CompleteAnimations]);
    let mut layout = check_ops_with_options(options, ops);
    for workspace in layout.workspaces_mut() {
        workspace.set_tiling_mode(mode);
    }
    Op::CompleteAnimations.apply(&mut layout);
    layout.open_overview();
    assert!(layout.is_stage_open());
    layout
}

#[test]
fn stage_projection_preserves_card_order_three_spaces_and_shared_layer_poses() {
    for mode in [
        clarion_ipc::TilingMode::Scrolling,
        clarion_ipc::TilingMode::Dwindle,
    ] {
        for scale in [1., 1.25, 1.5, 2.] {
            let mut layout = projected_stage_layout(scale, mode);
            let start = layout.clock.now_unadjusted();
            for millis in [0, 400, 1000] {
                layout
                    .clock
                    .set_unadjusted(start + Duration::from_millis(millis));
                layout.advance_animations();
                layout.update_render_elements(None);
                let monitor = layout.active_monitor_ref().unwrap();
                let composition = monitor.stage_composition();
                let expected = monitor.workspaces_with_render_geo().collect::<Vec<_>>();
                assert_eq!(composition.cards.len(), expected.len());
                assert_eq!(composition.scale, scale);
                let layers = composition.workspace_scenes().collect::<Vec<_>>();
                for ((card, layer), (workspace, geometry)) in
                    composition.cards.iter().zip(layers).zip(expected)
                {
                    assert_eq!(card.id, workspace.id());
                    assert_eq!(card.geometry, geometry);
                    assert_eq!(card.zoom, monitor.workspace_render_zoom(geometry));
                    assert_eq!(
                        card.crop,
                        Rectangle::from_size(
                            workspace.view_size().to_physical_precise_round(scale)
                        )
                    );
                    assert_eq!(layer.namespace, card.id.get() as usize);
                    assert_eq!(layer.geometry, card.geometry);
                    assert_eq!(layer.zoom, card.zoom);
                    assert_eq!(
                        layer.background.color(),
                        workspace.render_background().color()
                    );
                    for (space, expected_floating) in [
                        (TileSpace::Floating, true),
                        (
                            if mode == clarion_ipc::TilingMode::Dwindle {
                                TileSpace::Dwindle
                            } else {
                                TileSpace::Scrolling
                            },
                            false,
                        ),
                    ] {
                        let actual = card
                            .sources
                            .items
                            .iter()
                            .filter_map(|ordered| match &ordered.item {
                                Item::Tile {
                                    source,
                                    position,
                                    space: candidate,
                                    ..
                                } if std::mem::discriminant(candidate)
                                    == std::mem::discriminant(&space) =>
                                {
                                    assert_eq!(
                                        ordered.channel,
                                        if expected_floating {
                                            Channel::FloatingNormal
                                        } else {
                                            Channel::TilingNormal
                                        }
                                    );
                                    Some((*source.window().id(), *position))
                                }
                                _ => None,
                            })
                            .collect::<Vec<_>>();
                        let expected = workspace
                            .tiles_with_render_positions()
                            .filter(|(tile, _, visible)| {
                                *visible
                                    && workspace.is_floating(tile.window().id())
                                        == expected_floating
                            })
                            .map(|(tile, position, _)| (*tile.window().id(), position))
                            .collect::<Vec<_>>();
                        assert_eq!(actual, expected);
                    }
                }
            }
            layout.verify_invariants();
        }
    }
}

#[test]
fn stage_projection_keeps_one_minimize_source_and_private_window_identity() {
    for mode in [
        clarion_ipc::TilingMode::Scrolling,
        clarion_ipc::TilingMode::Dwindle,
    ] {
        let mut layout = projected_stage_layout(1.25, mode);
        Op::CompleteAnimations.apply(&mut layout);
        let selected = layout.active_monitor_ref().unwrap().stage_selected();
        let workspace = layout
            .active_monitor_ref()
            .unwrap()
            .workspaces
            .iter()
            .find(|workspace| workspace.id() == selected)
            .unwrap();
        let id = *workspace
            .windows()
            .find(|window| !workspace.is_floating(window.id()))
            .unwrap()
            .id();
        assert!(layout.minimize_window(&id));
        layout.update_render_elements(None);
        let composition = layout.active_monitor_ref().unwrap().stage_composition();
        let mut genie_count = 0;
        let mut normal_count = 0;
        for card in &composition.cards {
            for ordered in &card.sources.items {
                match &ordered.item {
                    Item::Genie { source, .. } if *source.window().id() == id => {
                        assert_eq!(card.id, selected);
                        assert_eq!(ordered.channel, Channel::TilingNormal);
                        genie_count += 1;
                    }
                    Item::Tile { source, .. } if *source.window().id() == id => {
                        normal_count += 1;
                    }
                    _ => {}
                }
            }
        }
        assert_eq!(genie_count, 1);
        assert_eq!(normal_count, 0);
        assert!(!composition.rail.is_empty());
        assert!(composition.rail.iter().all(|item| {
            item.geometry.size.w.is_finite()
                && item.geometry.size.h.is_finite()
                && item.highlight.is_finite()
        }));
    }
}
