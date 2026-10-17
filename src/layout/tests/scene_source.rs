use super::*;
use crate::layout::scene_source::WindowLookup;

#[test]
fn output_scene_freezes_visible_workspace_identity_geometry_and_background() {
    for mode in [
        clarion_ipc::TilingMode::Scrolling,
        clarion_ipc::TilingMode::Dwindle,
    ] {
        for floating in [false, true] {
            for scale in [1., 1.25, 2.] {
                let mut layout = check_ops([
                    Op::AddScaledOutput {
                        id: 1,
                        scale,
                        layout_config: None,
                    },
                    Op::AddOutput(2),
                    Op::FocusOutput(1),
                    Op::SwitchTilingMode(Some(mode)),
                    Op::AddWindow {
                        params: TestWindowParams {
                            is_floating: floating,
                            ..TestWindowParams::new(1)
                        },
                    },
                    Op::CompleteAnimations,
                ]);
                let output = layout.active_output().unwrap().clone();
                for overview in [false, true] {
                    if overview {
                        layout.toggle_expo();
                    }
                    layout.update_render_elements(Some(&output));
                    let source = layout.scene_source(&output);
                    let captured = source.monitor().unwrap();
                    let live = layout.monitor_for_output(&output).unwrap();
                    let expected: Vec<_> = live.workspaces_with_render_geo().collect();
                    assert_eq!(captured.workspaces().len(), expected.len());
                    assert_eq!(captured.desktop_above_top(), live.render_above_top_layer());
                    assert_eq!(captured.stage(), live.is_stage_visible());
                    assert_eq!(
                        captured.overview_progress(),
                        live.overview_backdrop_progress()
                    );
                    for (workspace, (live, geometry)) in captured.workspaces().iter().zip(expected)
                    {
                        assert_eq!(workspace.namespace, live.id().get() as usize);
                        assert_eq!(workspace.geometry, geometry);
                        assert_eq!(
                            workspace.zoom,
                            layout
                                .monitor_for_output(&output)
                                .unwrap()
                                .workspace_render_zoom(geometry)
                        );
                        assert_eq!(
                            workspace.background.color(),
                            live.render_background().color()
                        );
                        assert_eq!(
                            workspace.xray_position().pos_in_backdrop,
                            XrayPos::new(geometry.loc, workspace.zoom).pos_in_backdrop
                        );
                    }
                    assert!(source.focus_ring());
                    layout.verify_invariants();
                }
            }
        }
    }
}

#[test]
fn scene_sources_isolate_dragged_output_while_lookup_keeps_minimized_membership() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddOutput(2),
        Op::FocusOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::AddWindow {
            params: TestWindowParams {
                is_floating: true,
                ..TestWindowParams::new(1)
            },
        },
        Op::CompleteAnimations,
    ]);
    assert!(layout.minimize_window(&0));
    let first = layout.active_output().unwrap().clone();
    let second = layout
        .outputs()
        .find(|output| **output != first)
        .unwrap()
        .clone();
    assert!(layout.interactive_move_begin(1, &first, (100., 100.).into()));
    assert!(layout.interactive_move_update(
        &1,
        (500., 100.).into(),
        first.clone(),
        (600., 200.).into()
    ));
    layout.update_render_elements(None);
    assert!(!layout.scene_source(&first).focus_ring());
    assert!(layout.scene_source(&second).focus_ring());
    let lookup: &dyn WindowLookup<TestWindow> = &layout;
    assert_eq!(lookup.window(0).map(LayoutElement::id), Some(&0));
    assert_eq!(lookup.window(1).map(LayoutElement::id), Some(&1));
    assert!(lookup.window(99).is_none());
    assert!(layout.interactive_move_update(
        &1,
        (500., 100.).into(),
        second.clone(),
        (600., 200.).into()
    ));
    layout.update_render_elements(None);
    assert!(layout.scene_source(&first).focus_ring());
    assert!(!layout.scene_source(&second).focus_ring());
    layout.verify_invariants();
}

#[test]
fn detached_and_privacy_sources_have_no_desktop_capabilities() {
    let mut layout = check_ops([Op::AddOutput(1)]);
    let output = layout.active_output().unwrap().clone();
    Op::RemoveOutput(1).apply(&mut layout);
    layout.update_render_elements(None);
    let detached = layout.scene_source(&output);
    assert!(detached.monitor().is_none());
    assert!(detached.focus_ring());
    assert_eq!(layout.scene_backgrounds(&output).count(), 0);
    let hidden = crate::layout::scene_source::OutputSceneSource::<TestWindow>::empty();
    assert!(hidden.monitor().is_none());
    assert!(hidden.focus_ring());
}

#[test]
fn ordinary_scene_projects_three_spaces_once_with_existing_geometry_and_stacking() {
    use crate::scene::workspace_composition::{Channel, Item, TileSpace};
    for mode in [
        clarion_ipc::TilingMode::Scrolling,
        clarion_ipc::TilingMode::Dwindle,
    ] {
        for scale in [1., 1.25, 2.] {
            let mut layout = check_ops([
                Op::AddScaledOutput {
                    id: 1,
                    scale,
                    layout_config: None,
                },
                Op::SwitchTilingMode(Some(mode)),
                Op::AddWindow {
                    params: TestWindowParams::new(1),
                },
                Op::AddWindow {
                    params: TestWindowParams::new(2),
                },
                Op::AddWindow {
                    params: TestWindowParams {
                        is_floating: true,
                        ..TestWindowParams::new(3)
                    },
                },
                Op::AddWindow {
                    params: TestWindowParams {
                        is_floating: true,
                        ..TestWindowParams::new(4)
                    },
                },
                Op::CompleteAnimations,
            ]);
            let output = layout.active_output().unwrap().clone();
            for expo in [false, true] {
                if expo {
                    layout.toggle_expo();
                }
                layout.update_render_elements(Some(&output));
                let source = layout.scene_source(&output);
                let monitor = layout.active_monitor_ref().unwrap();
                let composition = source
                    .monitor()
                    .unwrap()
                    .ordinary_composition()
                    .expect("ordinary and Expo use the independent scene compositor");
                assert_eq!(
                    composition.geometry.placements.len(),
                    monitor.workspaces.len()
                );
                assert_eq!(composition.workspaces.len(), monitor.workspaces.len());
                for ((placement, sources), (workspace, geometry)) in composition
                    .geometry
                    .placements
                    .iter()
                    .zip(&composition.workspaces)
                    .zip(
                        monitor
                            .workspaces
                            .iter()
                            .zip(monitor.workspaces_render_geo()),
                    )
                {
                    assert_eq!(placement.id, workspace.id());
                    assert_eq!(placement.geometry, geometry);
                    assert_eq!(placement.zoom, monitor.workspace_render_zoom(geometry));
                    let actual = |space: TileSpace| {
                        sources
                            .items
                            .iter()
                            .filter_map(|item| match &item.item {
                                Item::Tile {
                                    source,
                                    position,
                                    space: candidate,
                                    ..
                                } if std::mem::discriminant(candidate)
                                    == std::mem::discriminant(&space) =>
                                {
                                    assert!(
                                        item.channel == Channel::FloatingNormal
                                            || item.channel == Channel::TilingNormal
                                    );
                                    Some((*source.window().id(), *position))
                                }
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                    };
                    let floating = if placement.normal_visible && workspace.is_floating_visible() {
                        workspace
                            .floating()
                            .tiles_with_render_positions()
                            .map(|(tile, position)| (*tile.window().id(), position))
                            .collect::<Vec<_>>()
                    } else {
                        Vec::new()
                    };
                    assert_eq!(actual(TileSpace::Floating), floating);
                    let tiling = if placement.normal_visible {
                        workspace
                            .tiles_with_render_positions()
                            .filter(|(tile, _, visible)| {
                                *visible && !workspace.is_floating(tile.window().id())
                            })
                            .map(|(tile, position, _)| (*tile.window().id(), position))
                            .collect::<Vec<_>>()
                    } else {
                        Vec::new()
                    };
                    let tile_space = if mode == clarion_ipc::TilingMode::Dwindle {
                        TileSpace::Dwindle
                    } else {
                        TileSpace::Scrolling
                    };
                    assert_eq!(actual(tile_space), tiling);
                }
                if expo {
                    assert!(composition.geometry.expo.is_some());
                    assert_eq!(
                        composition.geometry.normal_crop,
                        composition.geometry.moving_crop
                    );
                }
            }
        }
    }
}

#[test]
fn ordinary_scene_keeps_offscreen_moving_windows_but_culls_normal_sources() {
    use crate::scene::workspace_composition::{Channel, Item};
    for mode in [
        clarion_ipc::TilingMode::Scrolling,
        clarion_ipc::TilingMode::Dwindle,
    ] {
        let mut layout = check_ops([
            Op::AddOutput(1),
            Op::SwitchTilingMode(Some(mode)),
            Op::AddWindow {
                params: TestWindowParams::new(1),
            },
            Op::AddWindow {
                params: TestWindowParams {
                    is_floating: true,
                    ..TestWindowParams::new(2)
                },
            },
            Op::AddWindow {
                params: TestWindowParams::new(3),
            },
            Op::CompleteAnimations,
        ]);
        layout.move_to_workspace(Some(&2), 1, ActivateWindow::No);
        layout.move_to_workspace(Some(&3), 1, ActivateWindow::No);
        Op::CompleteAnimations.apply(&mut layout);
        let destination = layout
            .workspaces_mut()
            .find(|workspace| workspace.has_window(&2))
            .unwrap();
        let tile = destination
            .tiles_mut()
            .find(|tile| *tile.window().id() == 2)
            .unwrap();
        tile.animate_move_y_from(300.);
        tile.set_anim_y_between_workspaces();
        assert!(tile.is_moving_between_workspaces());
        let output = layout.active_output().unwrap().clone();
        layout.update_render_elements(Some(&output));
        let source = layout.scene_source(&output);
        let composition = source.monitor().unwrap().ordinary_composition().unwrap();
        let (placement, sources) = composition.geometry.placements.iter().zip(&composition.workspaces).find(|(_, sources)| sources.items.iter().any(|item| matches!(&item.item, Item::Tile { source, .. } if *source.window().id() == 2))).unwrap();
        assert!(!placement.normal_visible);
        assert!(sources
            .items
            .iter()
            .any(|item| item.channel == Channel::FloatingMoving
                && matches!(&item.item, Item::Tile { source, .. } if *source.window().id() == 2)));
        assert!(!sources.items.iter().any(|item| item.channel.normal()));
        assert_eq!(composition.geometry.moving_crop.size.w, i32::MAX);
        assert_eq!(composition.geometry.moving_crop.size.h, i32::MAX);
    }
}

#[test]
fn ordinary_scene_retains_exclusive_minimized_source_and_explicit_specialized_routes() {
    use crate::scene::workspace_composition::{Channel, Item};
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::CompleteAnimations,
    ]);
    assert!(layout.minimize_window(&1));
    let output = layout.active_output().unwrap().clone();
    layout.update_render_elements(Some(&output));
    {
        let source = layout.scene_source(&output);
        let composition = source.monitor().unwrap().ordinary_composition().unwrap();
        let items: Vec<_> = composition
            .workspaces
            .iter()
            .flat_map(|workspace| &workspace.items)
            .collect();
        assert!(!items.iter().any(
            |item| matches!(&item.item, Item::Tile { source, .. } if *source.window().id() == 1)
        ));
        assert_eq!(items.iter().filter(|item| item.channel == Channel::TilingNormal && matches!(&item.item, Item::Genie { source, .. } if *source.window().id() == 1)).count(), 1);
    }
    for spread in [false, true] {
        if spread {
            layout.toggle_window_spread();
        } else {
            layout.toggle_overview();
        }
        layout.update_render_elements(Some(&output));
        assert!(
            layout
                .scene_source(&output)
                .monitor()
                .unwrap()
                .ordinary_composition()
                .is_none(),
            "Stage and window spread keep explicit specialized adapters"
        );
        layout.close_overview();
        Op::CompleteAnimations.apply(&mut layout);
    }
    layout.update_render_elements(Some(&output));
    assert!(layout
        .scene_source(&output)
        .monitor()
        .unwrap()
        .ordinary_composition()
        .is_some());
}
