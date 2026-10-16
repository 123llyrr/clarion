use clarion_core::{CoreEffect, CoreEvent, DesktopCore, LayoutMode, Rect, SpaceId};
fn take(core: &mut DesktopCore<u64>) -> Option<SpaceId> {
    core.apply(CoreEvent::TakeScratchpadReturn)
        .into_iter()
        .find_map(|effect| match effect {
            CoreEffect::ScratchpadReturnTaken(space) => space,
            _ => None,
        })
}
#[test]
fn return_target_is_stable_across_output_transfer_and_is_consumed_once() {
    let mut core = DesktopCore::new();
    for connector in ["left", "right"] {
        core.apply(CoreEvent::OutputRegistered {
            connector: connector.into(),
            bounds: Rect::new(0., 0., 1000., 800.),
        });
    }
    core.apply(CoreEvent::CreateSpace {
        id: SpaceId(10),
        mode: LayoutMode::Scrolling,
    });
    core.apply(CoreEvent::AttachSpace {
        id: SpaceId(10),
        connector: "left".into(),
        index: 0,
    });
    core.apply(CoreEvent::RememberScratchpadReturn {
        space: Some(SpaceId(10)),
    });
    core.apply(CoreEvent::AttachSpace {
        id: SpaceId(10),
        connector: "right".into(),
        index: 0,
    });
    core.apply(CoreEvent::OutputDisconnected {
        connector: "right".into(),
    });
    assert_eq!(take(&mut core), Some(SpaceId(10)));
    assert_eq!(take(&mut core), None);
}
#[test]
fn retired_targets_are_cleared_and_unknown_remember_does_not_create_a_space() {
    let mut core = DesktopCore::<u64>::new();
    for id in [10, 20] {
        core.apply(CoreEvent::CreateSpace {
            id: SpaceId(id),
            mode: LayoutMode::Scrolling,
        });
    }
    core.apply(CoreEvent::RememberScratchpadReturn {
        space: Some(SpaceId(10)),
    });
    core.apply(CoreEvent::RememberScratchpadReturn {
        space: Some(SpaceId(99)),
    });
    assert_eq!(core.scratchpad_return(), Some(SpaceId(10)));
    assert!(core.space(SpaceId(99)).is_none());
    core.apply(CoreEvent::RemoveSpace {
        id: SpaceId(10),
        fallback: SpaceId(20),
    });
    assert_eq!(take(&mut core), None);
    core.apply(CoreEvent::RememberScratchpadReturn {
        space: Some(SpaceId(20)),
    });
    core.apply(CoreEvent::ForgetSpace { id: SpaceId(20) });
    assert_eq!(take(&mut core), None);
}
