use realand_core::{CoreEffect, CoreEvent, DesktopCore, LayoutMode, Rect, SpaceId};

fn desktop() -> DesktopCore<u64> {
    let mut core = DesktopCore::new();
    for output in ["LEFT", "RIGHT"] {
        core.apply(CoreEvent::OutputRegistered {
            connector: output.into(),
            bounds: Rect::new(0., 0., 800., 600.),
        });
    }
    for id in [10, 20] {
        core.apply(CoreEvent::CreateSpace {
            id: SpaceId(id),
            mode: LayoutMode::Scrolling,
        });
        core.apply(CoreEvent::AttachSpace {
            id: SpaceId(id),
            connector: "LEFT".into(),
            index: usize::MAX,
        });
    }
    core
}

#[test]
fn names_are_unique_across_desktops_and_same_alias_rename_is_a_noop() {
    let mut core = desktop();
    let first = core.apply(CoreEvent::SetSpaceName {
        space: SpaceId(10),
        name: Some("Editor".into()),
    });
    assert_eq!(
        first,
        vec![
            CoreEffect::SpaceNameChanged { space: SpaceId(10) },
            CoreEffect::OutputSceneChanged {
                connector: "LEFT".into()
            }
        ]
    );
    assert_eq!(core.named_space("EDITOR"), Some(SpaceId(10)));
    for space in [10, 20, 999] {
        assert!(core
            .apply(CoreEvent::SetSpaceName {
                space: SpaceId(space),
                name: Some("editor".into())
            })
            .is_empty());
    }
    assert!(core.space(SpaceId(20)).unwrap().name.is_none());
    assert!(core.space(SpaceId(999)).is_none());
    assert_eq!(
        core.space(SpaceId(10)).unwrap().name.as_deref(),
        Some("Editor")
    );
}

#[test]
fn name_survives_no_outputs_and_retirement_releases_the_alias_without_changing_focus() {
    let mut core = desktop();
    core.apply(CoreEvent::SetSpaceName {
        space: SpaceId(10),
        name: Some("Editor".into()),
    });
    core.apply(CoreEvent::DeactivateSpace);
    for output in ["LEFT", "RIGHT"] {
        core.apply(CoreEvent::OutputDisconnected {
            connector: output.into(),
        });
    }
    assert_eq!(core.named_space("editor"), Some(SpaceId(10)));
    assert!(core.active_window().is_none());
    core.apply(CoreEvent::ForgetSpace { id: SpaceId(10) });
    assert!(core.named_space("Editor").is_none());
    core.apply(CoreEvent::SetSpaceName {
        space: SpaceId(20),
        name: Some("Editor".into()),
    });
    assert_eq!(core.named_space("EDITOR"), Some(SpaceId(20)));
}

#[test]
fn scratchpad_classification_follows_only_the_accepted_name_and_unname_is_once() {
    let mut core = desktop();
    for name in ["Scratchpad", "SPECIAL"] {
        core.apply(CoreEvent::SetSpaceName {
            space: SpaceId(10),
            name: Some(name.into()),
        });
        assert!(core.space_is_scratchpad(SpaceId(10)));
    }
    core.apply(CoreEvent::SetSpaceName {
        space: SpaceId(10),
        name: Some("special extra".into()),
    });
    assert!(!core.space_is_scratchpad(SpaceId(10)));
    assert!(!core.space_is_scratchpad(SpaceId(999)));
    assert!(core
        .apply(CoreEvent::SetSpaceName {
            space: SpaceId(10),
            name: None
        })
        .contains(&CoreEffect::SpaceNameChanged { space: SpaceId(10) }));
    assert!(core
        .apply(CoreEvent::SetSpaceName {
            space: SpaceId(10),
            name: None
        })
        .is_empty());
}

#[test]
fn client_insertion_preserves_named_output_affinity_but_explicit_move_can_replace_it() {
    let mut core = desktop();
    core.apply(CoreEvent::PreferSpaceOutput {
        space: SpaceId(10),
        connector: "RIGHT".into(),
    });
    core.apply(CoreEvent::SetSpaceName {
        space: SpaceId(10),
        name: Some("Editor".into()),
    });
    core.apply(CoreEvent::PreferUnnamedSpaceOutput {
        space: SpaceId(10),
        connector: "LEFT".into(),
    });
    assert_eq!(core.space(SpaceId(10)).unwrap().output_preference, "RIGHT");
    core.apply(CoreEvent::PreferSpaceOutput {
        space: SpaceId(10),
        connector: "LEFT".into(),
    });
    assert_eq!(core.space(SpaceId(10)).unwrap().output_preference, "LEFT");
    core.apply(CoreEvent::SetSpaceName {
        space: SpaceId(10),
        name: None,
    });
    core.apply(CoreEvent::PreferUnnamedSpaceOutput {
        space: SpaceId(10),
        connector: "RIGHT".into(),
    });
    assert_eq!(core.space(SpaceId(10)).unwrap().output_preference, "RIGHT");
}
