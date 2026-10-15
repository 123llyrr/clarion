use realand_core::desktop::{CoreEffect, CoreEvent, DesktopCore, SpaceId};
use realand_core::geometry::Rect;
use realand_core::layout::LayoutMode;
use realand_core::overview::{OverviewAction, OverviewMode};
use realand_core::spread::{SpreadAction, SpreadCandidate, SpreadKey};

fn desktop() -> DesktopCore<u64> {
    let mut core = DesktopCore::new();
    for (connector, space) in [("LEFT", SpaceId(10)), ("RIGHT", SpaceId(20))] {
        core.apply(CoreEvent::OutputRegistered {
            connector: connector.into(),
            bounds: Rect::new(0., 0., 1000., 800.),
        });
        core.apply(CoreEvent::CreateSpace {
            id: space,
            mode: LayoutMode::Scrolling,
        });
        core.apply(CoreEvent::AttachSpace {
            id: space,
            connector: connector.into(),
            index: 0,
        });
    }
    for (id, space, output) in [
        (1, SpaceId(10), "LEFT"),
        (2, SpaceId(10), "LEFT"),
        (3, SpaceId(20), "RIGHT"),
        (4, SpaceId(10), "LEFT"),
    ] {
        core.add_window(
            id,
            space,
            Some(output.into()),
            Rect::new(0., 0., 300., 200.),
        );
    }
    core.set_minimized(&4, true);
    core
}

fn candidates(connector: &str, ids: &[u64]) -> CoreEvent<u64> {
    CoreEvent::Spread(SpreadAction::Candidates {
        connector: connector.into(),
        windows: ids
            .iter()
            .map(|id| SpreadCandidate {
                id: *id,
                search_text: Some(format!("Window {id}")),
            })
            .collect(),
    })
}

#[test]
fn candidate_acknowledgements_reject_unknown_connectors_and_foreign_ids_but_include_parked_windows()
{
    let mut core = desktop();
    core.focus_window(Some(2));
    assert!(core.apply(candidates("UNKNOWN", &[1])).is_empty());
    assert!(core.spread().output("UNKNOWN").is_none());
    core.apply(candidates("LEFT", &[1, 3, 99, 4]));
    let output = core.spread().output("LEFT").unwrap();
    assert_eq!(output.order(), &[1, 4]);
    assert_eq!(output.matching(), &[1, 4]);
    // Search selection is not keyboard focus and cannot select a foreign ID.
    core.apply(CoreEvent::Spread(SpreadAction::Reset {
        connector: "LEFT".into(),
        preferred: Some(3),
    }));
    assert_eq!(core.spread().output("LEFT").unwrap().selected(), Some(&1));
    assert_eq!(core.active_window(), Some(&2));
    // Equal acknowledged candidates produce no output damage or state intent.
    assert!(core.apply(candidates("LEFT", &[1, 3, 99, 4])).is_empty());
}

#[test]
fn cross_output_move_repairs_selection_and_preserves_cancel_identity_until_real_unmap() {
    let mut core = desktop();
    core.apply(CoreEvent::Spread(SpreadAction::Begin { initial: Some(2) }));
    core.apply(candidates("LEFT", &[1, 2]));
    core.apply(CoreEvent::Spread(SpreadAction::Key {
        connector: "LEFT".into(),
        key: SpreadKey::Right,
        columns: 2,
    }));
    assert_eq!(core.spread().output("LEFT").unwrap().selected(), Some(&2));
    let effects = core.move_window(&2, SpaceId(20), Some("RIGHT".into()));
    assert!(effects.contains(&CoreEffect::SpreadChanged));
    assert_eq!(core.spread().output("LEFT").unwrap().order(), &[1]);
    assert_eq!(core.spread().output("LEFT").unwrap().selected(), Some(&1));
    assert_eq!(core.spread().initial(), Some(&2));
    core.apply(candidates("RIGHT", &[3, 2]));
    core.apply(CoreEvent::Spread(SpreadAction::Reset {
        connector: "RIGHT".into(),
        preferred: Some(2),
    }));
    assert_eq!(core.spread().output("RIGHT").unwrap().selected(), Some(&2));
    core.remove_window(&2);
    assert_eq!(core.spread().initial(), None);
    assert_eq!(core.spread().output("RIGHT").unwrap().selected(), Some(&3));
    core.apply(CoreEvent::OutputDisconnected {
        connector: "RIGHT".into(),
    });
    assert!(core.spread().output("RIGHT").is_none());
}

#[test]
fn workspace_transfer_and_detached_move_remove_search_membership_without_destroying_live_cancel_origin(
) {
    let mut core = desktop();
    core.apply(CoreEvent::Spread(SpreadAction::Begin { initial: Some(1) }));
    core.apply(candidates("LEFT", &[1, 2]));
    // Owner reassociation invalidates the old connector before payload output
    // acknowledgements arrive; it does not retire the living window identity.
    core.apply(CoreEvent::AttachSpace {
        id: SpaceId(10),
        connector: "RIGHT".into(),
        index: 1,
    });
    assert!(core.spread().output("LEFT").unwrap().order().is_empty());
    assert_eq!(core.spread().initial(), Some(&1));
    core.apply(CoreEvent::SetOutput {
        id: 1,
        output: Some("RIGHT".into()),
        remember: true,
    });
    core.apply(candidates("RIGHT", &[1, 3]));
    core.detach_window(&1);
    assert_eq!(core.spread().output("RIGHT").unwrap().order(), &[3]);
    assert_eq!(core.spread().initial(), Some(&1));
    core.move_window(&1, SpaceId(10), Some("RIGHT".into()));
    core.apply(candidates("RIGHT", &[1, 3]));
    core.apply(CoreEvent::DetachSpace { id: SpaceId(10) });
    assert_eq!(core.spread().output("RIGHT").unwrap().order(), &[3]);
    assert_eq!(core.spread().initial(), Some(&1));
    core.apply(CoreEvent::Spread(SpreadAction::Close));
    assert!(core.spread().output("RIGHT").is_none());
    assert_eq!(core.spread().initial(), None);
}

fn open_spread(core: &mut DesktopCore<u64>) {
    core.apply(CoreEvent::Overview(OverviewAction::SetMode {
        mode: OverviewMode::Windows,
        timestamp: Default::default(),
    }));
    core.apply(CoreEvent::Overview(OverviewAction::SetOpen(true)));
    core.apply(candidates("LEFT", &[1, 2]));
    core.apply(CoreEvent::Spread(SpreadAction::Key {
        connector: "LEFT".into(),
        key: SpreadKey::Character('1'),
        columns: 2,
    }));
}

#[test]
fn metadata_change_refreshes_the_existing_query_and_damages_only_its_current_output() {
    let mut core = desktop();
    open_spread(&mut core);
    assert_eq!(core.spread().output("LEFT").unwrap().matching(), &[1]);
    let effects = core.apply(CoreEvent::WindowMetadataChanged {
        id: 2,
        search_text: Some("Window 1 renamed".into()),
    });
    assert_eq!(core.spread().output("LEFT").unwrap().matching(), &[1, 2]);
    assert_eq!(core.spread().output("LEFT").unwrap().selected(), Some(&1));
    assert_eq!(
        effects,
        vec![
            CoreEffect::SpreadChanged,
            CoreEffect::OutputSceneChanged {
                connector: "LEFT".into()
            },
        ]
    );
    // Original title capitalization can change the painted label even though
    // query membership and normalized cached metadata are exactly unchanged.
    let effects = core.apply(CoreEvent::WindowMetadataChanged {
        id: 2,
        search_text: Some("WINDOW 1 RENAMED".into()),
    });
    assert_eq!(
        effects,
        vec![CoreEffect::OutputSceneChanged {
            connector: "LEFT".into(),
        }]
    );
    let effects = core.apply(CoreEvent::WindowMetadataChanged {
        id: 2,
        search_text: None,
    });
    assert_eq!(
        effects,
        vec![CoreEffect::OutputSceneChanged {
            connector: "LEFT".into(),
        }]
    );
}

#[test]
fn metadata_from_foreign_retired_detached_or_hidden_windows_cannot_resurrect_spread_membership() {
    let mut core = desktop();
    open_spread(&mut core);
    for id in [3, 99] {
        assert!(core
            .apply(CoreEvent::WindowMetadataChanged {
                id,
                search_text: Some("Window 1".into()),
            })
            .is_empty());
    }
    // A formerly LEFT candidate cannot be registered on RIGHT by its metadata.
    core.move_window(&2, SpaceId(20), Some("RIGHT".into()));
    assert!(core
        .apply(CoreEvent::WindowMetadataChanged {
            id: 2,
            search_text: Some("Window 1".into()),
        })
        .is_empty());
    assert!(core.spread().output("RIGHT").is_none());
    core.detach_window(&1);
    assert!(core
        .apply(CoreEvent::WindowMetadataChanged {
            id: 1,
            search_text: Some("Window 1".into()),
        })
        .is_empty());
    core.remove_window(&1);
    assert!(core
        .apply(CoreEvent::WindowMetadataChanged {
            id: 1,
            search_text: Some("Window 1".into()),
        })
        .is_empty());
    core.apply(CoreEvent::Overview(OverviewAction::SetOpen(false)));
    core.apply(CoreEvent::Overview(OverviewAction::PresentationSettled));
    assert!(core
        .apply(CoreEvent::WindowMetadataChanged {
            id: 4,
            search_text: None,
        })
        .is_empty());
    assert!(core.spread().output("LEFT").is_none());
}
