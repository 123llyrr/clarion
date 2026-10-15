use realand_core::bsp::{BspSnapshot, PaneMode};
use realand_core::geometry::Axis;
use realand_core::layout::BspNode;
use realand_core::stage_drag::{StageCapture, StageDragAction as Action, StageRestoreIntent};
use realand_core::workspace_state::RestoreColumn;
use realand_core::{CoreEffect, CoreEvent, DesktopCore, LayoutMode, Rect, SpaceId};

fn desktop() -> DesktopCore<u64> {
    let mut core = DesktopCore::new();
    for output in ["LEFT", "RIGHT"] {
        core.apply(CoreEvent::OutputRegistered {
            connector: output.into(),
            bounds: Rect::new(0., 0., 1000., 800.),
        });
    }
    for (id, output, index) in [(10, "LEFT", 0), (30, "LEFT", 1), (20, "RIGHT", 0)] {
        core.apply(CoreEvent::CreateSpace {
            id: SpaceId(id),
            mode: LayoutMode::Scrolling,
        });
        core.apply(CoreEvent::AttachSpace {
            id: SpaceId(id),
            connector: output.into(),
            index,
        });
    }
    core.add_window(
        1,
        SpaceId(10),
        Some("LEFT".into()),
        Rect::new(0., 0., 300., 200.),
    );
    core.add_window(
        3,
        SpaceId(20),
        Some("RIGHT".into()),
        Rect::new(0., 0., 300., 200.),
    );
    core
}
fn capture(window: u64) -> CoreEvent<u64> {
    CoreEvent::StageDrag(Action::Capture(StageCapture {
        window,
        columns: Vec::new(),
        bsp: None,
        floating_position: Some([0.2, 0.3]),
        floating_size: Some([300, 200]),
        floating_width: 300.,
        restore_to_floating: false,
        fullscreen: false,
        maximized: false,
    }))
}
fn intent(effects: Vec<CoreEffect<u64>>) -> Option<StageRestoreIntent<u64>> {
    effects.into_iter().find_map(|effect| match effect {
        CoreEffect::StageRestoreIntent(value) => Some(value),
        _ => None,
    })
}

#[test]
fn restore_is_consumed_once_and_insertion_completion_keeps_canvas_identity_across_end_move() {
    let mut core = desktop();
    core.apply(CoreEvent::SetCanvasMode {
        space: SpaceId(10),
        enabled: true,
    });
    core.apply(CoreEvent::SetLayer {
        id: 1,
        layer: realand_core::desktop::Layer::Above,
    });
    core.apply(CoreEvent::SetCanvasOrigin { id: 1, tiled: true });
    assert!(core
        .apply(capture(1))
        .contains(&CoreEffect::StageDragChanged));
    assert!(core.apply(capture(1)).is_empty());
    core.apply(CoreEvent::BeginMove {
        id: 1,
        start: realand_core::move_controls::MoveStart::default(),
    });
    core.detach_window(&1);
    assert_eq!(core.stage_drag_source(), Some(SpaceId(10)));
    assert_eq!(core.stage_drag_destination(), Some(SpaceId(10)));
    core.apply(CoreEvent::StageDrag(Action::Resolve {
        window: 1,
        destination: SpaceId(10),
        columns: Vec::new(),
    }));
    let restore = intent(core.apply(CoreEvent::StageDrag(Action::Take { window: 1 }))).unwrap();
    assert!(restore.floating && restore.canvas_origin);
    assert!(intent(core.apply(CoreEvent::StageDrag(Action::Take { window: 1 }))).is_none());
    // Neither early completion nor EndMove may release the source before the
    // physical tile is attached and the canvas-origin acknowledgement is made.
    assert!(core
        .apply(CoreEvent::StageDrag(Action::Finish { window: 1 }))
        .is_empty());
    core.apply(CoreEvent::MovePlaced { id: 1 });
    assert_eq!(core.stage_drag_source(), Some(SpaceId(10)));
    core.move_window(&1, SpaceId(10), Some("LEFT".into()));
    assert!(core
        .apply(CoreEvent::StageDrag(Action::Finish { window: 3 }))
        .is_empty());
    assert!(core
        .apply(CoreEvent::StageDrag(Action::Finish { window: 1 }))
        .contains(&CoreEffect::StageDragChanged));
    assert_eq!(core.stage_drag_source(), None);
    assert_eq!(
        core.space(SpaceId(10))
            .unwrap()
            .interaction
            .canvas_origins(),
        &[1]
    );
    assert!(core
        .apply(CoreEvent::StageDrag(Action::Finish { window: 1 }))
        .is_empty());
}

#[test]
fn same_named_output_replacement_is_only_a_fallback_for_the_captured_connection_epoch() {
    for source_retired in [false, true] {
        let mut core = desktop();
        core.apply(capture(1));
        core.detach_window(&1);
        if source_retired {
            core.apply(CoreEvent::ForgetSpace { id: SpaceId(10) });
        }
        core.apply(CoreEvent::SelectOutput {
            connector: "RIGHT".into(),
        });
        core.apply(CoreEvent::OutputDisconnected {
            connector: "LEFT".into(),
        });
        core.apply(CoreEvent::OutputRegistered {
            connector: "LEFT".into(),
            bounds: Rect::new(0., 0., 1000., 800.),
        });
        assert_eq!(
            core.stage_drag_destination(),
            Some(if source_retired {
                SpaceId(20)
            } else {
                SpaceId(10)
            })
        );
    }
}

#[test]
fn advisory_destination_is_validated_and_foreign_or_closed_topology_ids_are_pruned() {
    let mut core = desktop();
    core.set_layout_mode(SpaceId(10), LayoutMode::Dwindle);
    let CoreEvent::StageDrag(Action::Capture(mut value)) = capture(1) else {
        unreachable!()
    };
    value.bsp = Some(BspSnapshot {
        topology: Some(BspNode::Split {
            axis: Axis::Horizontal,
            ratio: 0.5,
            first: Box::new(BspNode::Leaf(1)),
            second: Box::new(BspNode::Split {
                axis: Axis::Vertical,
                ratio: 0.5,
                first: Box::new(BspNode::Leaf(3)),
                second: Box::new(BspNode::Leaf(99)),
            }),
        }),
        active: Some(3),
        expanded: vec![(1, PaneMode::Fullscreen), (3, PaneMode::Maximized)],
    });
    core.apply(CoreEvent::StageDrag(Action::Capture(value)));
    core.detach_window(&1);
    assert!(core
        .apply(CoreEvent::StageDrag(Action::Resolve {
            window: 1,
            destination: SpaceId(20),
            columns: Vec::new()
        }))
        .is_empty());
    assert_eq!(core.stage_drag_destination(), Some(SpaceId(10)));
    core.apply(CoreEvent::StageDrag(Action::Resolve {
        window: 1,
        destination: SpaceId(10),
        columns: vec![RestoreColumn {
            id: 100,
            windows: vec![3, 99],
        }],
    }));
    let restore = intent(core.apply(CoreEvent::StageDrag(Action::Take { window: 1 }))).unwrap();
    let bsp = restore.bsp.unwrap();
    assert_eq!(bsp.topology, Some(BspNode::Leaf(1)));
    assert_eq!(bsp.active, None);
    assert_eq!(bsp.expanded, [(1, PaneMode::Fullscreen)]);
}

#[test]
fn real_unmap_releases_captured_or_consumed_restore_state_without_stale_pins() {
    for consumed in [false, true] {
        let mut core = desktop();
        core.apply(capture(1));
        if consumed {
            core.detach_window(&1);
            core.apply(CoreEvent::StageDrag(Action::Resolve {
                window: 1,
                destination: SpaceId(10),
                columns: Vec::new(),
            }));
            assert!(intent(core.apply(CoreEvent::StageDrag(Action::Take { window: 1 }))).is_some());
        }
        core.remove_window(&1);
        assert_eq!(core.stage_drag().window(), None);
        assert_eq!(core.stage_drag_source(), None);
        assert!(core.apply(capture(99)).is_empty());
        assert!(core
            .apply(CoreEvent::StageDrag(Action::Take { window: 1 }))
            .is_empty());
    }
}

#[test]
fn retired_resolved_destination_never_publishes_a_false_restore_intent_or_keeps_a_stale_session() {
    let mut core = desktop();
    core.apply(capture(1));
    core.detach_window(&1);
    core.apply(CoreEvent::StageDrag(Action::Resolve {
        window: 1,
        destination: SpaceId(10),
        columns: Vec::new(),
    }));
    core.apply(CoreEvent::ForgetSpace { id: SpaceId(10) });
    let effects = core.apply(CoreEvent::StageDrag(Action::Take { window: 1 }));
    assert!(effects.contains(&CoreEffect::StageDragChanged));
    assert!(intent(effects).is_none());
    assert_eq!(core.stage_drag().window(), None);
    core.move_window(&1, SpaceId(20), Some("RIGHT".into()));
    assert!(core
        .apply(capture(1))
        .contains(&CoreEffect::StageDragChanged));
}
