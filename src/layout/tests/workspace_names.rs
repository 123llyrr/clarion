use super::*;

fn fixture() -> Layout<TestWindow> {
    check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::FocusWorkspaceDown,
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
    ])
}

#[test]
fn name_accessor_borrows_the_single_core_string_and_lookup_uses_stable_identity() {
    let mut layout = fixture();
    layout.set_workspace_name("Editor".into(), None);
    let id = layout.active_workspace().unwrap().id();
    {
        let workspace = layout.active_workspace().unwrap();
        let borrowed = workspace.name().unwrap();
        let core = layout.core.borrow();
        let owned = core
            .space(clarion_core::SpaceId(id.get()))
            .unwrap()
            .name
            .as_ref()
            .unwrap();
        assert_eq!(borrowed.as_ptr(), owned.as_ptr());
        assert_eq!(&*borrowed, "Editor");
    }
    assert_eq!(layout.find_workspace_by_name("EDITOR").unwrap().1.id(), id);
    layout.switch_workspace(0);
    let other = layout.active_workspace().unwrap().id();
    assert_ne!(other, id);
    layout.set_workspace_name("editor".into(), None);
    assert!(layout.active_workspace().unwrap().name().is_none());
    assert_eq!(layout.find_workspace_by_name("Editor").unwrap().1.id(), id);
    layout.verify_invariants();
}

#[test]
fn scratchpad_name_and_serialized_session_follow_the_core_after_rename_and_unname() {
    let mut layout = fixture();
    layout.set_workspace_name("Scratchpad".into(), None);
    let id = layout.active_workspace().unwrap().id();
    assert!(layout.active_workspace().unwrap().is_scratchpad());
    let session = layout.session_layout();
    assert!(session
        .workspaces
        .iter()
        .any(|workspace| workspace.name.as_deref() == Some("Scratchpad")));
    let json = serde_json::to_value(session).unwrap();
    assert!(json["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .any(|workspace| workspace["name"] == "Scratchpad"));
    layout.set_workspace_name("Editor".into(), None);
    assert!(!layout.active_workspace().unwrap().is_scratchpad());
    assert!(layout.find_workspace_by_name("scratchpad").is_none());
    assert_eq!(layout.find_workspace_by_name("editor").unwrap().1.id(), id);
    layout.unset_workspace_name(None);
    assert!(layout.active_workspace().unwrap().name().is_none());
    assert!(!layout.active_workspace().unwrap().is_scratchpad());
    assert!(!layout
        .session_layout()
        .workspaces
        .iter()
        .any(|workspace| workspace.name.is_some()));
    layout.verify_invariants();
}

#[test]
fn named_lookup_survives_all_outputs_absent_and_new_configuration_updates_drop_the_borrow() {
    let mut layout = fixture();
    layout.set_workspace_name("Editor".into(), None);
    let id = layout.active_workspace().unwrap().id();
    Op::RemoveOutput(1).apply(&mut layout);
    assert_eq!(layout.find_workspace_by_name("editor").unwrap().1.id(), id);
    assert!(layout
        .find_workspace_by_ref(WorkspaceReference::Name("EDITOR".into()))
        .is_some());
    Op::AddOutput(2).apply(&mut layout);
    // A borrowed getter must not live across a configuration mutation.
    layout.update_config(&Config::default());
    assert_eq!(layout.find_workspace_by_name("Editor").unwrap().1.id(), id);
    layout.verify_invariants();
}
