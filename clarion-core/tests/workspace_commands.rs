use clarion_core::overview::OverviewAction;
use clarion_core::policy_memory::Width;
use clarion_core::workspace_state::{
    MinimizeRestore, RestoreColumn, RestoreDestination, ScrollingRestore, SpaceLayerFocus,
};
use clarion_core::{CoreEffect, CoreEvent, DesktopCore, Layer, LayoutMode, Rect, SpaceId};

fn desktop() -> DesktopCore<u64> {
    let mut core = DesktopCore::new();
    for name in ["A", "B"] {
        core.apply(CoreEvent::OutputRegistered {
            connector: name.into(),
            bounds: Rect::new(0., 0., 1000., 800.),
        });
    }
    for (id, output) in [(1, "A"), (2, "B")] {
        core.apply(CoreEvent::CreateSpace {
            id: SpaceId(id),
            mode: LayoutMode::Scrolling,
        });
        core.apply(CoreEvent::AttachSpace {
            id: SpaceId(id),
            connector: output.into(),
            index: 0,
        });
    }
    core
}

fn memory() -> MinimizeRestore<u64> {
    MinimizeRestore {
        was_floating: false,
        width: Width::Proportion(0.5),
        full_width: false,
        display: None,
        neighbor: Some(2),
        scrolling: Some(ScrollingRestore {
            column_id: 31,
            column_index: 3,
            row_index: 1,
            sibling: Some(2),
        }),
        canvas_origin: false,
    }
}

fn map(core: &mut DesktopCore<u64>, id: u64, space: u64) {
    core.add_window(
        id,
        SpaceId(space),
        Some(if space == 1 { "A" } else { "B" }.into()),
        Rect::new(10., 20., 300., 200.),
    );
}

#[test]
fn minimize_is_an_exact_once_command_and_restore_uses_authoritative_layer() {
    let mut core = desktop();
    for id in [1, 2] {
        map(&mut core, id, 1);
    }
    core.apply(CoreEvent::SetLayer {
        id: 2,
        layer: Layer::Above,
    });
    core.focus_window(Some(1));
    core.apply(CoreEvent::SetSpaceLayerFocus {
        space: SpaceId(1),
        focus: SpaceLayerFocus::TilingWithFloatingRaised,
    });
    assert_eq!(core.active_window(), Some(&1));
    assert!(!core
        .space(SpaceId(1))
        .unwrap()
        .interaction
        .layer_focus()
        .is_floating());
    // A caller hint claiming tiled does not override the mapped window layer.
    let events = core.apply(CoreEvent::MinimizeWindow {
        id: 2,
        restoration: memory(),
    });
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, CoreEffect::MinimizeIntent(_)))
            .count(),
        1
    );
    let window = core.window(&2).unwrap();
    assert!(window.minimized);
    assert!(window.restoration.as_ref().unwrap().was_floating);
    assert_eq!(
        core.space(SpaceId(1)).unwrap().interaction.layer_focus(),
        SpaceLayerFocus::Tiling
    );
    assert!(core
        .apply(CoreEvent::MinimizeWindow {
            id: 2,
            restoration: memory()
        })
        .is_empty());
    let effects = core.apply(CoreEvent::RestoreWindow {
        id: 2,
        columns: Vec::new(),
    });
    let intent = effects
        .iter()
        .find_map(|effect| {
            if let CoreEffect::MinimizeIntent(intent) = effect {
                Some(intent)
            } else {
                None
            }
        })
        .unwrap();
    assert!(!intent.minimized);
    assert_eq!(
        intent.restore.as_ref().unwrap().destination,
        RestoreDestination::Floating
    );
    assert!(!core.window(&2).unwrap().minimized);
    assert!(core.window(&2).unwrap().restoration.is_none());
    assert!(core
        .apply(CoreEvent::RestoreWindow {
            id: 2,
            columns: Vec::new()
        })
        .is_empty());
}

#[test]
fn canvas_membership_moves_into_core_restore_memory_without_stale_exit_actions() {
    let mut core = desktop();
    map(&mut core, 1, 1);
    core.apply(CoreEvent::SetCanvasMode {
        space: SpaceId(1),
        enabled: true,
    });
    core.apply(CoreEvent::SetLayer {
        id: 1,
        layer: Layer::Above,
    });
    core.apply(CoreEvent::SetCanvasOrigin { id: 1, tiled: true });
    assert_eq!(
        core.space(SpaceId(1)).unwrap().interaction.canvas_origins(),
        &[1]
    );
    core.apply(CoreEvent::MinimizeWindow {
        id: 1,
        restoration: memory(),
    });
    assert!(core
        .space(SpaceId(1))
        .unwrap()
        .interaction
        .canvas_origins()
        .is_empty());
    assert!(
        core.window(&1)
            .unwrap()
            .restoration
            .as_ref()
            .unwrap()
            .canvas_origin
    );
    let exit = core.apply(CoreEvent::SetCanvasMode {
        space: SpaceId(1),
        enabled: false,
    });
    assert!(exit.iter().any(|effect| matches!(effect, CoreEffect::CanvasChanged { transition, .. } if transition.restore_tiled.is_empty())));
    let restore = core.apply(CoreEvent::RestoreWindow {
        id: 1,
        columns: Vec::new(),
    });
    assert!(restore.iter().any(|effect| matches!(effect, CoreEffect::MinimizeIntent(intent) if !intent.restore.as_ref().unwrap().canvas_origin)));
    assert!(restore.iter().any(|effect| matches!(effect, CoreEffect::MinimizeIntent(intent) if !intent.restore.as_ref().unwrap().is_floating())));
    assert!(core
        .space(SpaceId(1))
        .unwrap()
        .interaction
        .canvas_origins()
        .is_empty());
}

#[test]
fn restore_selects_live_stable_column_and_rejects_moved_or_closed_neighbor_ids() {
    let mut core = desktop();
    for id in [1, 2, 3] {
        map(&mut core, id, 1);
    }
    core.apply(CoreEvent::MinimizeWindow {
        id: 1,
        restoration: memory(),
    });
    core.move_window(&2, SpaceId(2), Some("B".into()));
    let mut closed = core.clone();
    let topology = vec![
        RestoreColumn {
            id: 77,
            windows: vec![2],
        },
        RestoreColumn {
            id: 31,
            windows: vec![3],
        },
    ];
    let effects = core.apply(CoreEvent::RestoreWindow {
        id: 1,
        columns: topology,
    });
    assert!(effects.iter().any(|effect| matches!(effect, CoreEffect::MinimizeIntent(intent)
        if matches!(intent.restore.as_ref().unwrap().destination, RestoreDestination::Scrolling { column: Some(31), row_index: 1, .. }))));
    closed.remove_window(&3);
    let effects = closed.apply(CoreEvent::RestoreWindow {
        id: 1,
        columns: vec![RestoreColumn {
            id: 77,
            windows: vec![2],
        }],
    });
    assert!(effects.iter().any(|effect| matches!(effect, CoreEffect::MinimizeIntent(intent)
        if matches!(intent.restore.as_ref().unwrap().destination, RestoreDestination::Scrolling { column: None, column_index: 3, .. }))));
    let mut mode_change = desktop();
    for id in [1, 2] {
        map(&mut mode_change, id, 1);
    }
    mode_change.apply(CoreEvent::MinimizeWindow {
        id: 1,
        restoration: memory(),
    });
    mode_change.set_layout_mode(SpaceId(1), LayoutMode::Dwindle);
    mode_change.remove_window(&2);
    let effects = mode_change.apply(CoreEvent::RestoreWindow {
        id: 1,
        columns: Vec::new(),
    });
    assert!(effects.iter().any(|effect| matches!(effect, CoreEffect::MinimizeIntent(intent)
        if matches!(intent.restore.as_ref().unwrap().destination, RestoreDestination::Tiled { neighbor: None }))));
}

#[test]
fn unknown_commands_never_create_ghost_spaces_or_minimize_unmapped_identity() {
    let mut core = desktop();
    for event in [
        CoreEvent::SetSpaceLayerFocus {
            space: SpaceId(99),
            focus: SpaceLayerFocus::Floating,
        },
        CoreEvent::SetCanvasMode {
            space: SpaceId(99),
            enabled: true,
        },
        CoreEvent::MinimizeWindow {
            id: 99,
            restoration: memory(),
        },
        CoreEvent::RestoreWindow {
            id: 99,
            columns: Vec::new(),
        },
    ] {
        assert!(core.apply(event).is_empty());
    }
    assert!(core.space(SpaceId(99)).is_none());
    assert!(core.window(&99).is_none());
}

#[test]
fn stage_events_validate_output_membership_and_retirement_prunes_global_origins() {
    let mut core = desktop();
    assert!(core
        .apply(CoreEvent::Overview(OverviewAction::StageReset {
            connector: "A".into(),
            space: SpaceId(2)
        }))
        .is_empty());
    assert!(core.overview().stage("A").is_none());
    core.apply(CoreEvent::Overview(OverviewAction::StageReset {
        connector: "A".into(),
        space: SpaceId(1),
    }));
    core.apply(CoreEvent::Overview(OverviewAction::RememberStage(Some(
        SpaceId(1),
    ))));
    core.apply(CoreEvent::Overview(OverviewAction::RememberExpo(Some(
        SpaceId(1),
    ))));
    core.apply(CoreEvent::AttachSpace {
        id: SpaceId(1),
        connector: "B".into(),
        index: 1,
    });
    assert_eq!(core.overview().stage("A").unwrap().selected, None);
    assert_eq!(core.overview().initial_stage(), Some(SpaceId(1)));
    assert_eq!(core.overview().initial_expo(), Some(SpaceId(1)));
    core.apply(CoreEvent::ForgetSpace { id: SpaceId(1) });
    assert_eq!(core.overview().initial_stage(), None);
    assert_eq!(core.overview().initial_expo(), None);
}

#[test]
fn originally_floating_canvas_member_keeps_its_identity_after_minimized_canvas_exit() {
    let mut core = desktop();
    map(&mut core, 2, 1);
    core.apply(CoreEvent::SetLayer {
        id: 2,
        layer: Layer::Above,
    });
    core.apply(CoreEvent::SetCanvasMode {
        space: SpaceId(1),
        enabled: true,
    });
    core.apply(CoreEvent::MinimizeWindow {
        id: 2,
        restoration: memory(),
    });
    core.apply(CoreEvent::SetCanvasMode {
        space: SpaceId(1),
        enabled: false,
    });
    let effects = core.apply(CoreEvent::RestoreWindow {
        id: 2,
        columns: Vec::new(),
    });
    assert!(effects.iter().any(|effect| matches!(effect, CoreEffect::MinimizeIntent(intent) if intent.restore.as_ref().unwrap().is_floating())));
}
