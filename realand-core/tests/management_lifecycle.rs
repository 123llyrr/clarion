use realand_core::management::{
    Arrangement, ArrangementSpace, ArrangementWindow, Group, ManagementAction as Action,
    ManagementOutcome,
};
use realand_core::policy_memory::ColumnDisplay;
use realand_core::{CoreEffect, CoreEvent, DesktopCore, LayoutMode, Rect, SpaceId};

fn desktop() -> DesktopCore<u64> {
    let mut core = DesktopCore::new();
    for space in [SpaceId(10), SpaceId(20)] {
        core.apply(CoreEvent::CreateSpace {
            id: space,
            mode: LayoutMode::Scrolling,
        });
    }
    for id in [1, 2] {
        core.add_window(id, SpaceId(10), None, Rect::new(0., 0., 300., 200.));
    }
    core
}
fn snapshot(core: &DesktopCore<u64>) -> Arrangement<u64> {
    Arrangement {
        spaces: core
            .spaces()
            .map(|space| ArrangementSpace {
                id: space.id,
                name: None,
                index: 0,
                output: None,
                mode: space.mode,
                tree: None,
            })
            .collect(),
        windows: core
            .windows()
            .map(|window| ArrangementWindow {
                id: window.id,
                app_id: "app".into(),
                title: window.id.to_string(),
                space: window.space,
                column: Some(0),
                display: ColumnDisplay::Normal,
                sizing: None,
                floating: false,
                size: [300, 200],
                position: None,
            })
            .collect(),
        groups: Vec::new(), // The owner supplies its real group state at capture.
        focus: core.active_window().copied(),
    }
}
fn apply(core: &mut DesktopCore<u64>, action: Action<u64>) -> ManagementOutcome<u64> {
    core.apply(CoreEvent::Management(action))
        .into_iter()
        .find_map(|effect| match effect {
            CoreEffect::ManagementChanged { outcome } => Some(outcome),
            _ => None,
        })
        .unwrap_or_default()
}
#[test]
fn unmap_prunes_current_groups_but_history_retains_closed_stable_slots_for_exact_once_restore() {
    let mut core = desktop();
    let current = snapshot(&core);
    apply(
        &mut core,
        Action::SetGroup {
            name: "pair".into(),
            members: vec![1, 2],
            current,
        },
    );
    let current = snapshot(&core);
    apply(&mut core, Action::Checkpoint(current));
    core.remove_window(&2);
    assert_eq!(core.management().groups()[0].members, [1]);
    let current = snapshot(&core);
    let restore = apply(
        &mut core,
        Action::Undo {
            redo: false,
            current,
        },
    )
    .restore
    .unwrap();
    assert_eq!(
        restore
            .windows
            .iter()
            .map(|window| window.id)
            .collect::<Vec<_>>(),
        [1, 2]
    );
    assert_eq!(restore.groups[0].members, [1, 2]);
    assert_eq!(core.management().groups()[0].members, [1]);
    assert_eq!(core.management().history_steps(), (1, 1));
    // A fresh client with a new stable identity cannot replace the closed slot.
    core.add_window(3, SpaceId(10), None, Rect::new(0., 0., 300., 200.));
    assert!(!restore.windows.iter().any(|window| window.id == 3));
}
#[test]
fn invalid_group_destination_has_no_history_effect_and_no_outputs_do_not_disable_valid_intents() {
    let mut core = desktop();
    let current = snapshot(&core);
    apply(
        &mut core,
        Action::SetGroup {
            name: "pair".into(),
            members: vec![1, 2, 9],
            current,
        },
    );
    let before = core.management().history_steps();
    let current = snapshot(&core);
    assert!(apply(
        &mut core,
        Action::MoveGroup {
            name: "pair".into(),
            target: SpaceId(99),
            current
        }
    )
    .move_group
    .is_none());
    assert_eq!(core.management().history_steps(), before);
    let current = snapshot(&core);
    let intent = apply(
        &mut core,
        Action::MoveGroup {
            name: "pair".into(),
            target: SpaceId(20),
            current,
        },
    )
    .move_group
    .unwrap();
    assert_eq!(intent.target, SpaceId(20));
    assert_eq!(intent.members, [1, 2]);
    assert_eq!(
        core.window(&1).unwrap().space,
        SpaceId(10),
        "a placement intent is executed by the permanent layout adapter"
    );
}
#[test]
fn active_move_suppresses_history_while_preserving_live_group_commands() {
    let mut core = desktop();
    core.apply(CoreEvent::BeginMove {
        id: 1,
        start: realand_core::move_controls::MoveStart::default(),
    });
    let current = snapshot(&core);
    apply(
        &mut core,
        Action::SetGroup {
            name: "pair".into(),
            members: vec![1, 2],
            current,
        },
    );
    assert_eq!(core.management().history_steps(), (0, 0));
    assert_eq!(core.management().groups()[0].members, [1, 2]);
    let current = snapshot(&core);
    assert!(apply(
        &mut core,
        Action::Undo {
            redo: false,
            current
        }
    )
    .restore
    .is_none());
    core.apply(CoreEvent::MovePlaced { id: 1 });
    let current = snapshot(&core);
    apply(
        &mut core,
        Action::RemoveGroup {
            name: "pair".into(),
            current,
        },
    );
    assert_eq!(core.management().history_steps(), (1, 0));
    let current = snapshot(&core);
    apply(
        &mut core,
        Action::Undo {
            redo: false,
            current,
        },
    );
    assert_eq!(core.management().groups()[0].name, "pair");
}
#[test]
fn replacement_groups_cannot_retain_unknown_or_duplicate_identities() {
    let mut core = desktop();
    apply(
        &mut core,
        Action::ReplaceGroups(vec![
            Group {
                name: "first".into(),
                members: vec![1, 1, 9],
            },
            Group {
                name: "second".into(),
                members: vec![1, 2],
            },
        ]),
    );
    assert_eq!(core.management().groups()[0].members, [1]);
    assert_eq!(core.management().groups()[1].members, [2]);
    core.remove_window(&1);
    assert_eq!(core.management().groups().len(), 1);
    assert_eq!(core.management().groups()[0].name, "second");
}
