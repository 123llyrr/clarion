//! Desktop event contracts across display/space lifetime changes.
//!
//! These exercise the public state owner rather than the navigation container
//! directly: window identity, local remembered focus and nullable keyboard focus
//! must survive the same event sequences as real output adapters.
use realand_core::{CoreEffect, CoreEvent, DesktopCore, LayoutMode, Rect, SpaceId};

const LEFT: &str = "DP-1";
const RIGHT: &str = "HDMI-A-1";

fn event(core: &mut DesktopCore<u64>, event: CoreEvent<u64>) -> Vec<CoreEffect<u64>> {
    let effects = core.apply(event);
    assert!(core.navigation().verify_invariants());
    effects
}

fn connect(core: &mut DesktopCore<u64>, connector: &str) {
    event(
        core,
        CoreEvent::OutputConnected {
            connector: connector.into(),
            bounds: Rect::new(0.0, 0.0, 1920.0, 1080.0),
        },
    );
}

fn space(core: &mut DesktopCore<u64>, id: u64, connector: &str, index: usize) {
    event(
        core,
        CoreEvent::CreateSpace {
            id: SpaceId(id),
            mode: LayoutMode::Scrolling,
        },
    );
    event(
        core,
        CoreEvent::AttachSpace {
            id: SpaceId(id),
            connector: connector.into(),
            index,
        },
    );
}

fn display(core: &mut DesktopCore<u64>, connector: &str, id: u64) -> Vec<CoreEffect<u64>> {
    event(
        core,
        CoreEvent::SelectOutputSpace {
            connector: connector.into(),
            id: SpaceId(id),
            remember_previous: true,
        },
    )
}

fn map(core: &mut DesktopCore<u64>, id: u64, space: u64, connector: &str) {
    core.add_window(
        id,
        SpaceId(space),
        Some(connector.into()),
        Rect::new(30.0, 40.0, 600.0, 400.0),
    );
    event(
        core,
        CoreEvent::SetSpaceFocus {
            space: SpaceId(space),
            id: Some(id),
            activate: false,
        },
    );
}

fn selected(core: &DesktopCore<u64>, connector: &str, id: u64) {
    assert_eq!(core.navigation().active_space(connector), Some(SpaceId(id)));
}

fn two_outputs() -> DesktopCore<u64> {
    let mut core = DesktopCore::new();
    connect(&mut core, LEFT);
    connect(&mut core, RIGHT);
    space(&mut core, 10, LEFT, 0);
    space(&mut core, 20, RIGHT, 0);
    core
}

#[test]
fn device_metadata_updates_and_reconnect_do_not_duplicate_connector_identity() {
    let mut core = two_outputs();
    for _ in 0..8 {
        connect(&mut core, LEFT);
        event(
            &mut core,
            CoreEvent::OutputRegistered {
                connector: RIGHT.into(),
                bounds: Rect::new(1920.0, 0.0, 2560.0, 1440.0),
            },
        );
    }
    assert_eq!(
        core.navigation().output_order(),
        &[LEFT.to_owned(), RIGHT.to_owned()]
    );
    assert_eq!(core.outputs().count(), 2);
    event(
        &mut core,
        CoreEvent::SelectOutput {
            connector: RIGHT.into(),
        },
    );
    event(
        &mut core,
        CoreEvent::OutputDisconnected {
            connector: RIGHT.into(),
        },
    );
    assert_eq!(core.navigation().selected_output(), Some(LEFT));
    assert_eq!(core.outputs().count(), 1);
    selected(&core, RIGHT, 20); // Disconnection keeps stable placement memory.
    connect(&mut core, RIGHT);
    assert_eq!(
        core.navigation().output_order(),
        &[LEFT.to_owned(), RIGHT.to_owned()]
    );
    assert_eq!(core.outputs().count(), 2);
    assert_eq!(core.navigation().selected_output(), Some(LEFT));
    assert_eq!(core.take_remembered_space(RIGHT), Some(SpaceId(20)));
    assert_eq!(core.take_remembered_space(RIGHT), None);
}

#[test]
fn insertion_reorder_and_cross_output_transfer_keep_displayed_ids_not_slot_numbers() {
    let mut core = two_outputs();
    space(&mut core, 11, LEFT, usize::MAX);
    space(&mut core, 12, LEFT, usize::MAX);
    display(&mut core, LEFT, 11);
    space(&mut core, 13, LEFT, 0);
    event(
        &mut core,
        CoreEvent::ReorderSpace {
            id: SpaceId(11),
            index: usize::MAX,
        },
    );
    assert_eq!(
        core.navigation().output(LEFT).unwrap().spaces(),
        &[SpaceId(13), SpaceId(10), SpaceId(12), SpaceId(11)]
    );
    selected(&core, LEFT, 11);
    assert_eq!(core.navigation().previous_space(LEFT), Some(SpaceId(10)));
    event(
        &mut core,
        CoreEvent::AttachSpace {
            id: SpaceId(11),
            connector: RIGHT.into(),
            index: 0,
        },
    );
    assert_eq!(core.navigation().owner(SpaceId(11)), Some(RIGHT));
    assert_eq!(
        core.navigation().output(LEFT).unwrap().spaces(),
        &[SpaceId(13), SpaceId(10), SpaceId(12)]
    );
    assert_eq!(
        core.navigation().output(RIGHT).unwrap().spaces(),
        &[SpaceId(11), SpaceId(20)]
    );
    selected(&core, LEFT, 12); // Removing active slot chooses its predecessor.
    selected(&core, RIGHT, 20); // Insertion preserves an existing active identity.
    assert_eq!(core.spaces().count(), 5);
}

#[test]
fn inactive_output_display_and_suspended_keyboard_focus_are_independent() {
    let mut core = two_outputs();
    space(&mut core, 21, RIGHT, 1);
    map(&mut core, 100, 10, LEFT);
    map(&mut core, 200, 20, RIGHT);
    event(&mut core, CoreEvent::ActivateSpace { id: SpaceId(10) });
    let effects = display(&mut core, RIGHT, 21);
    selected(&core, RIGHT, 21);
    assert_eq!(core.navigation().selected_output(), Some(LEFT));
    assert_eq!(core.active_space(), Some(SpaceId(10)));
    assert_eq!(core.active_window(), Some(&100));
    assert_eq!(
        effects,
        vec![CoreEffect::OutputSceneChanged {
            connector: RIGHT.into()
        }]
    );
    event(&mut core, CoreEvent::DeactivateSpace);
    assert_eq!(core.active_space(), None);
    assert_eq!(core.active_window(), None);
    selected(&core, LEFT, 10);
    selected(&core, RIGHT, 21);
    assert_eq!(core.space(SpaceId(10)).unwrap().focused, Some(100));
    assert_eq!(core.space(SpaceId(20)).unwrap().focused, Some(200));
    display(&mut core, RIGHT, 20);
    event(&mut core, CoreEvent::ActivateSpace { id: SpaceId(20) });
    assert_eq!(core.navigation().selected_output(), Some(RIGHT));
    assert_eq!(core.navigation().selected_space(), Some(SpaceId(20)));
    assert_eq!(core.active_window(), Some(&200));
}

#[test]
fn selecting_an_empty_desktop_never_reuses_another_desktops_window_focus() {
    let mut core = two_outputs();
    map(&mut core, 100, 10, LEFT);
    space(&mut core, 11, LEFT, 1);
    event(&mut core, CoreEvent::ActivateSpace { id: SpaceId(10) });
    event(&mut core, CoreEvent::ActivateSpace { id: SpaceId(11) });
    selected(&core, LEFT, 11);
    assert_eq!(core.active_space(), Some(SpaceId(11)));
    assert_eq!(core.active_window(), None);
    assert_eq!(core.space(SpaceId(10)).unwrap().focused, Some(100));
    event(&mut core, CoreEvent::DeactivateSpace);
    map(&mut core, 101, 10, LEFT);
    assert_eq!(core.active_space(), None);
    assert_eq!(core.active_window(), None);
    selected(&core, LEFT, 11);
    event(&mut core, CoreEvent::ActivateSpace { id: SpaceId(10) });
    assert_eq!(core.active_window(), Some(&101));
}

#[test]
fn removing_previous_or_current_space_prunes_dead_preferences_and_migrates_members() {
    let mut core = two_outputs();
    space(&mut core, 11, LEFT, 1);
    space(&mut core, 12, LEFT, 2);
    map(&mut core, 100, 10, LEFT);
    map(&mut core, 110, 11, LEFT);
    event(&mut core, CoreEvent::ActivateSpace { id: SpaceId(11) });
    assert_eq!(core.navigation().previous_space(LEFT), Some(SpaceId(10)));
    event(
        &mut core,
        CoreEvent::RememberOutputSpace {
            connector: LEFT.into(),
        },
    );
    event(
        &mut core,
        CoreEvent::RemoveSpace {
            id: SpaceId(10),
            fallback: SpaceId(12),
        },
    );
    assert_eq!(core.space(SpaceId(10)), None);
    assert_eq!(core.navigation().previous_space(LEFT), None);
    assert_eq!(core.window(&100).unwrap().space, SpaceId(12));
    event(
        &mut core,
        CoreEvent::RemoveSpace {
            id: SpaceId(11),
            fallback: SpaceId(12),
        },
    );
    assert_eq!(core.take_remembered_space(LEFT), None);
    assert_eq!(core.active_space(), Some(SpaceId(12)));
    assert!(matches!(core.active_window(), Some(100 | 110)));
    assert_eq!(core.navigation().owner(SpaceId(11)), None);
    assert_eq!(
        core.navigation().output(LEFT).unwrap().spaces(),
        &[SpaceId(12)]
    );
    assert_eq!(core.spaces().count(), 2);
}

#[test]
fn unplug_transfer_reconnect_preserves_windows_desktop_ids_and_focus() {
    let mut core = two_outputs();
    map(&mut core, 200, 20, RIGHT);
    event(&mut core, CoreEvent::ActivateSpace { id: SpaceId(20) });
    event(
        &mut core,
        CoreEvent::OutputDisconnected {
            connector: RIGHT.into(),
        },
    );
    assert_eq!(core.window(&200).unwrap().output.as_deref(), Some(LEFT));
    assert_eq!(
        core.window(&200).unwrap().preferred_output.as_deref(),
        Some(RIGHT)
    );
    event(&mut core, CoreEvent::DetachSpace { id: SpaceId(20) });
    event(
        &mut core,
        CoreEvent::AttachSpace {
            id: SpaceId(20),
            connector: LEFT.into(),
            index: 0,
        },
    );
    event(&mut core, CoreEvent::ActivateSpace { id: SpaceId(20) });
    assert_eq!(core.navigation().selected_output(), Some(LEFT));
    assert_eq!(core.active_window(), Some(&200));
    connect(&mut core, RIGHT);
    assert_eq!(core.take_remembered_space(RIGHT), Some(SpaceId(20)));
    event(
        &mut core,
        CoreEvent::AttachSpace {
            id: SpaceId(20),
            connector: RIGHT.into(),
            index: 0,
        },
    );
    event(&mut core, CoreEvent::ActivateSpace { id: SpaceId(20) });
    selected(&core, RIGHT, 20);
    assert_eq!(core.navigation().selected_output(), Some(RIGHT));
    assert_eq!(core.active_window(), Some(&200));
    assert_eq!(core.window(&200).unwrap().output.as_deref(), Some(RIGHT));
    assert_eq!(core.window(&200).unwrap().space, SpaceId(20));
    assert_eq!(core.spaces().count(), 2);
    assert_eq!(core.windows().count(), 1);
}

#[test]
fn all_outputs_absent_and_later_registration_preserve_unmapped_remapped_identity() {
    let mut core = two_outputs();
    map(&mut core, 100, 10, LEFT);
    event(
        &mut core,
        CoreEvent::OutputDisconnected {
            connector: RIGHT.into(),
        },
    );
    event(
        &mut core,
        CoreEvent::OutputDisconnected {
            connector: LEFT.into(),
        },
    );
    event(&mut core, CoreEvent::DeactivateSpace);
    assert_eq!(core.navigation().selected_output(), None);
    assert_eq!(core.navigation().selected_space(), None);
    assert_eq!(core.window(&100).unwrap().output, None);
    assert_eq!(core.window(&100).unwrap().space, SpaceId(10));
    assert_eq!(core.spaces().count(), 2);
    event(&mut core, CoreEvent::UnmapWindow { id: 100 });
    assert_eq!(core.space(SpaceId(10)).unwrap().focused, None);
    connect(&mut core, LEFT);
    map(&mut core, 101, 10, LEFT);
    assert_eq!(core.active_window(), None);
    event(&mut core, CoreEvent::ActivateSpace { id: SpaceId(10) });
    assert_eq!(core.active_window(), Some(&101));
    assert!(core.window(&100).is_none());
    assert_eq!(core.navigation().selected_output(), Some(LEFT));
    assert_eq!(core.navigation().selected_space(), Some(SpaceId(10)));
}

#[test]
fn retiring_empty_spaces_repeatedly_does_not_accumulate_topology_or_history() {
    let mut core = two_outputs();
    for id in 1000..1200 {
        space(&mut core, id, RIGHT, 0);
        display(&mut core, RIGHT, id);
        event(
            &mut core,
            CoreEvent::RememberOutputSpace {
                connector: RIGHT.into(),
            },
        );
        display(&mut core, RIGHT, 20);
        event(&mut core, CoreEvent::ForgetSpace { id: SpaceId(id) });
        assert_eq!(core.navigation().previous_space(RIGHT), None);
        assert_eq!(core.take_remembered_space(RIGHT), None);
        assert_eq!(core.navigation().owner(SpaceId(id)), None);
        assert!(core.space(SpaceId(id)).is_none());
        assert_eq!(
            core.navigation().output(RIGHT).unwrap().spaces(),
            &[SpaceId(20)]
        );
        assert_eq!(core.spaces().count(), 2);
    }
}

#[test]
fn switching_previous_preserves_ids_after_two_reorders_instead_of_saved_indices() {
    let mut core = two_outputs();
    space(&mut core, 11, LEFT, 1);
    space(&mut core, 12, LEFT, 2);
    display(&mut core, LEFT, 11);
    display(&mut core, LEFT, 12);
    event(
        &mut core,
        CoreEvent::ReorderSpace {
            id: SpaceId(10),
            index: 2,
        },
    );
    event(
        &mut core,
        CoreEvent::ReorderSpace {
            id: SpaceId(12),
            index: 0,
        },
    );
    selected(&core, LEFT, 12);
    assert_eq!(core.navigation().previous_space(LEFT), Some(SpaceId(11)));
    let previous = core.navigation().previous_space(LEFT).unwrap();
    display(&mut core, LEFT, previous.0);
    selected(&core, LEFT, 11);
    assert_eq!(core.navigation().previous_space(LEFT), Some(SpaceId(12)));
    assert_eq!(
        core.navigation().output(LEFT).unwrap().spaces(),
        &[SpaceId(12), SpaceId(11), SpaceId(10)]
    );
}

#[test]
fn window_focus_activation_selects_its_owning_output_and_displayed_space() {
    let mut core = two_outputs();
    space(&mut core, 21, RIGHT, 1);
    map(&mut core, 100, 10, LEFT);
    map(&mut core, 210, 21, RIGHT);
    event(&mut core, CoreEvent::ActivateSpace { id: SpaceId(10) });
    event(&mut core, CoreEvent::FocusWindow { id: Some(210) });
    assert_eq!(core.active_space(), Some(SpaceId(21)));
    assert_eq!(core.active_window(), Some(&210));
    assert_eq!(core.navigation().selected_output(), Some(RIGHT));
    assert_eq!(core.navigation().selected_space(), Some(SpaceId(21)));
    assert_eq!(core.navigation().previous_space(RIGHT), Some(SpaceId(20)));
    event(&mut core, CoreEvent::FocusWindow { id: None });
    assert_eq!(core.active_window(), None);
    assert_eq!(core.navigation().selected_space(), Some(SpaceId(21)));
}

#[test]
fn explicit_space_focus_activation_aligns_navigation_but_inactive_memory_does_not() {
    let mut core = two_outputs();
    space(&mut core, 21, RIGHT, 1);
    map(&mut core, 100, 10, LEFT);
    map(&mut core, 210, 21, RIGHT);
    event(&mut core, CoreEvent::ActivateSpace { id: SpaceId(10) });
    event(
        &mut core,
        CoreEvent::SetSpaceFocus {
            space: SpaceId(21),
            id: Some(210),
            activate: false,
        },
    );
    assert_eq!(core.active_window(), Some(&100));
    selected(&core, RIGHT, 20);
    event(
        &mut core,
        CoreEvent::SetSpaceFocus {
            space: SpaceId(21),
            id: Some(210),
            activate: true,
        },
    );
    assert_eq!(core.active_window(), Some(&210));
    assert_eq!(core.navigation().selected_output(), Some(RIGHT));
    assert_eq!(core.navigation().selected_space(), Some(SpaceId(21)));
}

#[test]
fn rejected_attachment_to_unknown_output_does_not_create_or_select_ghost_spaces() {
    let mut core = two_outputs();
    event(&mut core, CoreEvent::DeactivateSpace);
    for id in 5000..5010 {
        let effects = event(
            &mut core,
            CoreEvent::AttachSpace {
                id: SpaceId(id),
                connector: "missing-output".into(),
                index: 0,
            },
        );
        assert!(effects.is_empty());
        assert!(core.space(SpaceId(id)).is_none());
        assert_eq!(core.spaces().count(), 2);
        assert_eq!(core.active_space(), None);
    }
    assert_eq!(core.navigation().selected_output(), Some(LEFT));
    selected(&core, LEFT, 10);
}

#[test]
fn retiring_the_selected_empty_desktop_restores_the_same_outputs_preceding_desktop() {
    let mut core = two_outputs();
    space(&mut core, 21, RIGHT, 1);
    map(&mut core, 100, 10, LEFT);
    map(&mut core, 200, 20, RIGHT);
    event(&mut core, CoreEvent::ActivateSpace { id: SpaceId(21) });
    assert_eq!(core.active_window(), None);
    event(&mut core, CoreEvent::ForgetSpace { id: SpaceId(21) });
    assert_eq!(core.navigation().selected_output(), Some(RIGHT));
    selected(&core, RIGHT, 20);
    assert_eq!(core.active_space(), Some(SpaceId(20)));
    assert_eq!(core.active_window(), Some(&200));
    assert_eq!(core.spaces().count(), 2);
}

#[test]
fn unplugged_selected_output_prefers_the_preceding_connected_output_in_order() {
    for count in [3, 4] {
        let mut core = DesktopCore::<u64>::new();
        let names = [LEFT, RIGHT, "DP-3", "DP-4"];
        for (index, name) in names.iter().take(count).enumerate() {
            connect(&mut core, name);
            space(&mut core, 10 + index as u64, name, 0);
        }
        event(
            &mut core,
            CoreEvent::SelectOutput {
                connector: names[2].into(),
            },
        );
        event(
            &mut core,
            CoreEvent::OutputDisconnected {
                connector: names[2].into(),
            },
        );
        assert_eq!(core.navigation().selected_output(), Some(names[1]));
        let expected: Vec<_> = names
            .iter()
            .take(count)
            .filter(|name| **name != names[2])
            .map(|name| (*name).to_owned())
            .collect();
        assert_eq!(core.navigation().output_order(), expected);
    }
}

#[test]
fn finishing_a_cross_output_detached_move_activates_the_destination_by_stable_id() {
    let mut core = two_outputs();
    map(&mut core, 100, 10, LEFT);
    event(&mut core, CoreEvent::ActivateSpace { id: SpaceId(10) });
    event(
        &mut core,
        CoreEvent::BeginMove {
            id: 100,
            start: realand_core::move_controls::MoveStart::default(),
        },
    );
    event(
        &mut core,
        CoreEvent::StartMoveMotion {
            id: 100,
            delta: [256., 0.],
            zoom: 1.,
        },
    );
    event(
        &mut core,
        CoreEvent::MovePose {
            id: 100,
            output: Some(RIGHT.into()),
            geometry: Rect::new(120.0, 80.0, 600.0, 400.0),
            pointer: [130.0, 90.0],
        },
    );
    event(&mut core, CoreEvent::DetachWindow { id: 100 });
    assert_eq!(core.active_window(), Some(&100));
    assert_eq!(core.space(SpaceId(10)).unwrap().focused, None);
    event(
        &mut core,
        CoreEvent::MoveWindow {
            id: 100,
            space: SpaceId(20),
            output: Some(RIGHT.into()),
        },
    );
    event(&mut core, CoreEvent::MovePlaced { id: 100 });
    assert_eq!(core.active_window(), Some(&100));
    assert_eq!(core.active_space(), Some(SpaceId(20)));
    assert_eq!(core.navigation().selected_output(), Some(RIGHT));
    assert_eq!(core.navigation().selected_space(), Some(SpaceId(20)));
    assert_eq!(core.space(SpaceId(20)).unwrap().focused, Some(100));
    assert_eq!(core.window(&100).unwrap().space, SpaceId(20));
    assert!(core.window(&100).unwrap().attached);
    assert!(core.interaction().is_none());
}

#[test]
fn creation_mapping_and_display_changes_do_not_resume_suspended_keyboard_focus() {
    let mut core = two_outputs();
    event(&mut core, CoreEvent::DeactivateSpace);
    space(&mut core, 30, RIGHT, 1);
    event(
        &mut core,
        CoreEvent::ReorderSpace {
            id: SpaceId(30),
            index: 0,
        },
    );
    map(&mut core, 300, 30, RIGHT);
    display(&mut core, RIGHT, 30);
    assert_eq!(core.active_space(), None);
    assert_eq!(core.active_window(), None);
    assert_eq!(core.space(SpaceId(30)).unwrap().focused, Some(300));
    selected(&core, RIGHT, 30);
    event(&mut core, CoreEvent::ActivateSpace { id: SpaceId(30) });
    assert_eq!(core.active_space(), Some(SpaceId(30)));
    assert_eq!(core.active_window(), Some(&300));
    assert_eq!(core.navigation().selected_output(), Some(RIGHT));
}

#[test]
fn no_output_creation_after_suspension_requires_explicit_focus_activation() {
    let mut core = DesktopCore::<u64>::new();
    event(&mut core, CoreEvent::DeactivateSpace);
    event(
        &mut core,
        CoreEvent::CreateSpace {
            id: SpaceId(10),
            mode: LayoutMode::Floating,
        },
    );
    core.add_window(100, SpaceId(10), None, Rect::new(10.0, 20.0, 300.0, 200.0));
    event(
        &mut core,
        CoreEvent::SetSpaceFocus {
            space: SpaceId(10),
            id: Some(100),
            activate: false,
        },
    );
    assert_eq!(core.active_space(), None);
    assert_eq!(core.active_window(), None);
    assert_eq!(core.window(&100).unwrap().output, None);
    event(&mut core, CoreEvent::FocusWindow { id: Some(100) });
    assert_eq!(core.active_space(), Some(SpaceId(10)));
    assert_eq!(core.active_window(), Some(&100));
    event(&mut core, CoreEvent::DeactivateSpace);
    event(
        &mut core,
        CoreEvent::CreateSpace {
            id: SpaceId(20),
            mode: LayoutMode::Dwindle,
        },
    );
    assert_eq!(core.active_space(), None);
    assert_eq!(core.active_window(), None);
    assert_eq!(core.navigation().selected_space(), None);
}

#[test]
fn detached_desktop_creation_and_reorder_preserve_insertion_ids_without_focus_activation() {
    let mut core = DesktopCore::<u64>::new();
    event(&mut core, CoreEvent::DeactivateSpace);
    for id in [30, 10, 20, 10] {
        event(
            &mut core,
            CoreEvent::CreateSpace {
                id: SpaceId(id),
                mode: LayoutMode::Scrolling,
            },
        );
    }
    assert_eq!(
        core.navigation().detached_spaces(),
        &[SpaceId(30), SpaceId(10), SpaceId(20)]
    );
    event(
        &mut core,
        CoreEvent::ReorderDetachedSpace {
            id: SpaceId(20),
            index: 0,
        },
    );
    assert_eq!(
        core.navigation().detached_spaces(),
        &[SpaceId(20), SpaceId(30), SpaceId(10)]
    );
    event(
        &mut core,
        CoreEvent::ReorderDetachedSpace {
            id: SpaceId(30),
            index: usize::MAX,
        },
    );
    assert_eq!(
        core.navigation().detached_spaces(),
        &[SpaceId(20), SpaceId(10), SpaceId(30)]
    );
    event(
        &mut core,
        CoreEvent::ReorderDetachedSpace {
            id: SpaceId(999),
            index: 0,
        },
    );
    assert_eq!(
        core.navigation().detached_spaces().first(),
        Some(&SpaceId(20))
    );
    assert!(core.space(SpaceId(999)).is_none());
    assert_eq!(core.spaces().count(), 3);
    assert_eq!(core.active_space(), None);
    assert_eq!(core.active_window(), None);
}

#[test]
fn attaching_detaching_and_retiring_desktops_maintain_exactly_one_ordered_association() {
    let mut core = two_outputs();
    event(&mut core, CoreEvent::DeactivateSpace);
    for id in [30, 40] {
        event(
            &mut core,
            CoreEvent::CreateSpace {
                id: SpaceId(id),
                mode: LayoutMode::Floating,
            },
        );
    }
    assert_eq!(
        core.navigation().detached_spaces(),
        &[SpaceId(30), SpaceId(40)]
    );
    event(&mut core, CoreEvent::DetachSpace { id: SpaceId(10) });
    event(&mut core, CoreEvent::DetachSpace { id: SpaceId(10) });
    assert_eq!(
        core.navigation().detached_spaces(),
        &[SpaceId(30), SpaceId(40), SpaceId(10)]
    );
    event(
        &mut core,
        CoreEvent::AttachSpace {
            id: SpaceId(30),
            connector: RIGHT.into(),
            index: 0,
        },
    );
    assert_eq!(
        core.navigation().detached_spaces(),
        &[SpaceId(40), SpaceId(10)]
    );
    assert_eq!(core.navigation().owner(SpaceId(30)), Some(RIGHT));
    event(
        &mut core,
        CoreEvent::ReorderDetachedSpace {
            id: SpaceId(10),
            index: 0,
        },
    );
    event(
        &mut core,
        CoreEvent::AttachSpace {
            id: SpaceId(10),
            connector: LEFT.into(),
            index: 0,
        },
    );
    assert_eq!(core.navigation().detached_spaces(), &[SpaceId(40)]);
    assert_eq!(
        core.navigation().output(RIGHT).unwrap().spaces(),
        &[SpaceId(30), SpaceId(20)]
    );
    event(&mut core, CoreEvent::ForgetSpace { id: SpaceId(40) });
    event(&mut core, CoreEvent::ForgetSpace { id: SpaceId(30) });
    assert!(core.navigation().detached_spaces().is_empty());
    assert_eq!(
        core.navigation().output(RIGHT).unwrap().spaces(),
        &[SpaceId(20)]
    );
    assert_eq!(core.spaces().count(), 2);
    assert_eq!(core.active_space(), None);
    assert_eq!(core.active_window(), None);
}

#[test]
fn detached_order_survives_all_outputs_absent_and_guides_reconnection_without_resuming_focus() {
    let mut core = two_outputs();
    space(&mut core, 11, LEFT, 1);
    map(&mut core, 100, 10, LEFT);
    map(&mut core, 110, 11, LEFT);
    map(&mut core, 200, 20, RIGHT);
    event(&mut core, CoreEvent::DeactivateSpace);
    event(
        &mut core,
        CoreEvent::OutputDisconnected {
            connector: RIGHT.into(),
        },
    );
    event(&mut core, CoreEvent::DetachSpace { id: SpaceId(20) });
    event(
        &mut core,
        CoreEvent::OutputDisconnected {
            connector: LEFT.into(),
        },
    );
    event(&mut core, CoreEvent::DetachSpace { id: SpaceId(10) });
    event(&mut core, CoreEvent::DetachSpace { id: SpaceId(11) });
    assert_eq!(
        core.navigation().detached_spaces(),
        &[SpaceId(20), SpaceId(10), SpaceId(11)]
    );
    assert_eq!(core.outputs().count(), 0);
    assert!(core.windows().all(|window| window.output.is_none()));
    event(
        &mut core,
        CoreEvent::ReorderDetachedSpace {
            id: SpaceId(11),
            index: 0,
        },
    );
    assert_eq!(
        core.navigation().detached_spaces(),
        &[SpaceId(11), SpaceId(20), SpaceId(10)]
    );
    assert_eq!(core.active_space(), None);
    assert_eq!(core.active_window(), None);
    connect(&mut core, LEFT);
    for (index, id) in [11, 20, 10].into_iter().enumerate() {
        event(
            &mut core,
            CoreEvent::AttachSpace {
                id: SpaceId(id),
                connector: LEFT.into(),
                index,
            },
        );
    }
    assert!(core.navigation().detached_spaces().is_empty());
    assert_eq!(
        core.navigation().output(LEFT).unwrap().spaces(),
        &[SpaceId(11), SpaceId(20), SpaceId(10)]
    );
    connect(&mut core, RIGHT);
    event(
        &mut core,
        CoreEvent::AttachSpace {
            id: SpaceId(20),
            connector: RIGHT.into(),
            index: 0,
        },
    );
    assert_eq!(
        core.navigation().output(LEFT).unwrap().spaces(),
        &[SpaceId(11), SpaceId(10)]
    );
    assert_eq!(
        core.navigation().output(RIGHT).unwrap().spaces(),
        &[SpaceId(20)]
    );
    assert_eq!(core.window(&200).unwrap().output.as_deref(), Some(RIGHT));
    assert_eq!(core.window(&110).unwrap().space, SpaceId(11));
    assert_eq!(core.spaces().count(), 3);
    assert_eq!(core.windows().count(), 3);
    assert_eq!(core.active_space(), None);
    assert_eq!(core.active_window(), None);
    event(&mut core, CoreEvent::ActivateSpace { id: SpaceId(11) });
    assert_eq!(core.active_window(), Some(&110));
}

#[test]
fn forgotten_detached_desktops_are_removed_from_reconnect_memory_and_sequence() {
    let mut core = two_outputs();
    event(&mut core, CoreEvent::DeactivateSpace);
    event(
        &mut core,
        CoreEvent::OutputDisconnected {
            connector: RIGHT.into(),
        },
    );
    event(&mut core, CoreEvent::DetachSpace { id: SpaceId(20) });
    assert_eq!(core.navigation().detached_spaces(), &[SpaceId(20)]);
    event(&mut core, CoreEvent::ForgetSpace { id: SpaceId(20) });
    assert!(core.navigation().detached_spaces().is_empty());
    assert_eq!(core.take_remembered_space(RIGHT), None);
    for id in 2000..2100 {
        event(
            &mut core,
            CoreEvent::CreateSpace {
                id: SpaceId(id),
                mode: LayoutMode::Dwindle,
            },
        );
        event(&mut core, CoreEvent::ForgetSpace { id: SpaceId(id) });
        assert!(core.navigation().detached_spaces().is_empty());
        assert_eq!(core.spaces().count(), 1);
    }
    connect(&mut core, RIGHT);
    assert!(core.navigation().output(RIGHT).unwrap().spaces().is_empty());
    assert_eq!(core.navigation().active_space(RIGHT), None);
    assert_eq!(core.active_space(), None);
    assert_eq!(core.active_window(), None);
}

#[test]
fn output_connection_generation_changes_on_reconnect_but_not_bounds_or_metadata_updates() {
    let mut core = DesktopCore::<u64>::new();
    let original = Rect::new(0., 0., 800., 600.);
    core.apply(CoreEvent::OutputConnected {
        connector: "LEFT".into(),
        bounds: original,
    });
    let left = core.output("LEFT").unwrap().generation;
    assert_ne!(left, 0);
    core.apply(CoreEvent::OutputRegistered {
        connector: "RIGHT".into(),
        bounds: original,
    });
    let right = core.output("RIGHT").unwrap().generation;
    assert_ne!(left, right);
    for event in [
        CoreEvent::OutputConnected {
            connector: "LEFT".into(),
            bounds: Rect::new(0., 0., 900., 700.),
        },
        CoreEvent::OutputRegistered {
            connector: "LEFT".into(),
            bounds: original,
        },
        CoreEvent::OutputConnected {
            connector: "LEFT".into(),
            bounds: original,
        },
    ] {
        core.apply(event);
        assert_eq!(core.output("LEFT").unwrap().generation, left);
    }
    core.apply(CoreEvent::OutputDisconnected {
        connector: "LEFT".into(),
    });
    core.apply(CoreEvent::OutputRegistered {
        connector: "LEFT".into(),
        bounds: original,
    });
    let replacement = core.output("LEFT").unwrap().generation;
    assert_ne!(replacement, left);
    assert_ne!(replacement, right);
    assert_eq!(core.output("RIGHT").unwrap().generation, right);
}
