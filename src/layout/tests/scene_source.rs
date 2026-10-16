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
