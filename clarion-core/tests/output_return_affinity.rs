use clarion_core::output_identity::OutputIdentity;
use clarion_core::{CoreEvent, DesktopCore, LayoutMode, Rect, SpaceId};

fn register(core: &mut DesktopCore<u64>, connector: &str, serial: &str) {
    core.apply(CoreEvent::OutputRegistered {
        connector: connector.into(),
        bounds: Rect::new(0., 0., 800., 600.),
    });
    core.apply(CoreEvent::OutputIdentityUpdated {
        connector: connector.into(),
        identity: OutputIdentity {
            connector: connector.into(),
            make: Some("Maker".into()),
            model: Some("Screen".into()),
            serial: Some(serial.into()),
        },
    });
}

fn space(core: &mut DesktopCore<u64>, id: u64, output: &str, selector: &str) {
    core.apply(CoreEvent::CreateSpace {
        id: SpaceId(id),
        mode: LayoutMode::Scrolling,
    });
    core.apply(CoreEvent::AttachSpace {
        id: SpaceId(id),
        connector: output.into(),
        index: usize::MAX,
    });
    core.apply(CoreEvent::SetSpaceOutputPreference {
        space: SpaceId(id),
        selector: selector.into(),
    });
}

#[test]
fn return_selection_uses_owned_alias_facts_and_stable_source_order() {
    let mut core = DesktopCore::new();
    register(&mut core, "LEFT", "L");
    register(&mut core, "RIGHT", "R");
    for (id, selector) in [(10, "Maker Screen R"), (20, "left"), (30, "rIgHt")] {
        space(&mut core, id, "LEFT", selector);
    }
    assert_eq!(
        core.returning_spaces("LEFT", "RIGHT"),
        [SpaceId(10), SpaceId(30)]
    );
    core.apply(CoreEvent::ReorderSpace {
        id: SpaceId(30),
        index: 0,
    });
    assert_eq!(
        core.returning_spaces("LEFT", "RIGHT"),
        [SpaceId(30), SpaceId(10)]
    );
    core.apply(CoreEvent::DetachSpace { id: SpaceId(30) });
    assert_eq!(core.returning_spaces("LEFT", "RIGHT"), [SpaceId(10)]);
    core.apply(CoreEvent::ForgetSpace { id: SpaceId(10) });
    assert!(core.returning_spaces("LEFT", "RIGHT").is_empty());
}

#[test]
fn normalized_preference_survives_no_outputs_and_connector_rename_but_rejects_foreign_display() {
    let mut core = DesktopCore::new();
    register(&mut core, "DP-1", "OWN");
    space(&mut core, 10, "DP-1", "dp-1");
    core.apply(CoreEvent::NormalizeSpaceOutputPreference {
        space: SpaceId(10),
        connector: "DP-1".into(),
    });
    assert_eq!(
        core.space(SpaceId(10)).unwrap().output_preference,
        "Maker Screen OWN"
    );
    core.apply(CoreEvent::OutputDisconnected {
        connector: "DP-1".into(),
    });
    core.apply(CoreEvent::DetachSpace { id: SpaceId(10) });
    assert_eq!(
        core.space(SpaceId(10)).unwrap().output_preference,
        "Maker Screen OWN"
    );
    register(&mut core, "DP-1", "OTHER");
    assert!(!core.space_prefers_output(SpaceId(10), "DP-1"));
    register(&mut core, "DP-9", "OWN");
    assert!(core.space_prefers_output(SpaceId(10), "DP-9"));
    core.apply(CoreEvent::AttachSpace {
        id: SpaceId(10),
        connector: "DP-1".into(),
        index: 0,
    });
    assert_eq!(core.returning_spaces("DP-1", "DP-9"), [SpaceId(10)]);
}

#[test]
fn explicit_destination_changes_preference_even_when_current_owner_is_already_correct() {
    let mut core = DesktopCore::new();
    register(&mut core, "LEFT", "L");
    register(&mut core, "RIGHT", "R");
    space(&mut core, 10, "LEFT", "Maker Screen R");
    core.apply(CoreEvent::NormalizeSpaceOutputPreference {
        space: SpaceId(10),
        connector: "LEFT".into(),
    });
    assert_eq!(core.returning_spaces("LEFT", "RIGHT"), [SpaceId(10)]);
    core.apply(CoreEvent::PreferSpaceOutput {
        space: SpaceId(10),
        connector: "LEFT".into(),
    });
    assert!(core.returning_spaces("LEFT", "RIGHT").is_empty());
    assert_eq!(
        core.space(SpaceId(10)).unwrap().output_preference,
        "Maker Screen L"
    );
}

#[test]
fn unknown_identity_and_preference_commands_cannot_create_outputs_or_desktops() {
    let mut core = DesktopCore::<u64>::new();
    core.apply(CoreEvent::OutputIdentityUpdated {
        connector: "UNKNOWN".into(),
        identity: OutputIdentity::connector_only("UNKNOWN".into()),
    });
    core.apply(CoreEvent::SetSpaceOutputPreference {
        space: SpaceId(99),
        selector: "UNKNOWN".into(),
    });
    core.apply(CoreEvent::PreferSpaceOutput {
        space: SpaceId(99),
        connector: "UNKNOWN".into(),
    });
    core.apply(CoreEvent::NormalizeSpaceOutputPreference {
        space: SpaceId(99),
        connector: "UNKNOWN".into(),
    });
    assert!(core.space(SpaceId(99)).is_none());
    assert!(core.output("UNKNOWN").is_none());
    assert!(core.returning_spaces("UNKNOWN", "UNKNOWN").is_empty());
}

#[test]
fn mutable_output_bounds_and_facts_do_not_replace_connection_identity_or_remembered_preference() {
    let mut core = DesktopCore::new();
    register(&mut core, "LEFT", "L");
    space(&mut core, 10, "LEFT", "Maker Screen L");
    let generation = core.output("LEFT").unwrap().generation;
    core.apply(CoreEvent::OutputRegistered {
        connector: "LEFT".into(),
        bounds: Rect::new(0., 0., 1280., 800.),
    });
    assert_eq!(core.output("LEFT").unwrap().generation, generation);
    assert!(core.space_prefers_output(SpaceId(10), "LEFT"));
    register(&mut core, "LEFT", "NEW");
    assert_eq!(core.output("LEFT").unwrap().generation, generation);
    assert!(!core.space_prefers_output(SpaceId(10), "LEFT"));
    assert_eq!(
        core.space(SpaceId(10)).unwrap().output_preference,
        "Maker Screen L"
    );
}
