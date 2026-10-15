use realand_core::dnd_hold::{DndActivateIntent, DndHoldAction as Action, DndTarget as Target};
use realand_core::{
    CoreEffect, CoreEvent, DesktopCore, Layer, LayoutMode, Rect, SpaceId, WindowRecord,
};
use std::time::Duration;

fn desktop() -> DesktopCore<u64> {
    let mut core = DesktopCore::new();
    for connector in ["left", "right"] {
        core.apply(CoreEvent::OutputRegistered {
            connector: connector.into(),
            bounds: Rect::new(0., 0., 1920., 1080.),
        });
    }
    for (space, connector) in [(10, "left"), (11, "left"), (20, "right")] {
        core.apply(CoreEvent::CreateSpace {
            id: SpaceId(space),
            mode: LayoutMode::Scrolling,
        });
        core.apply(CoreEvent::AttachSpace {
            id: SpaceId(space),
            connector: connector.into(),
            index: usize::MAX,
        });
    }
    let mut window = WindowRecord::new(1, SpaceId(10), Rect::new(10., 20., 500., 300.));
    window.output = Some("left".into());
    core.apply(CoreEvent::MapWindow { window });
    core.apply(CoreEvent::DndHold(Action::Motion {
        connector: "left".into(),
    }));
    core
}
fn observe(
    core: &mut DesktopCore<u64>,
    connector: &str,
    target: Target<u64>,
    ms: u64,
) -> Option<DndActivateIntent<u64>> {
    core.apply(CoreEvent::DndHold(Action::Observe {
        connector: connector.into(),
        target: Some(target),
        timestamp: Duration::from_millis(ms),
        scrolled: false,
    }))
    .into_iter()
    .find_map(|effect| match effect {
        CoreEffect::DndActivateIntent(intent) => Some(intent),
        _ => None,
    })
}
#[test]
fn hover_intent_contains_validated_stable_placement_without_a_second_focus_owner() {
    let mut core = desktop();
    core.apply(CoreEvent::ActivateSpace { id: SpaceId(20) });
    assert!(observe(&mut core, "left", Target::Window(1), 100).is_none());
    assert!(observe(&mut core, "left", Target::Window(1), 849).is_none());
    assert_eq!(
        observe(&mut core, "left", Target::Window(1), 850),
        Some(DndActivateIntent {
            connector: "left".into(),
            space: SpaceId(10),
            window: Some(1)
        })
    );
    // Executing the intent selects the actual retained layout via its adapter;
    // publishing an intent cannot silently select a different keyboard scope.
    assert_eq!(core.active_space(), Some(SpaceId(20)));
    assert!(core.dnd_hold().target().is_none());
    assert!(observe(&mut core, "left", Target::Workspace(SpaceId(11)), 900).is_none());
    assert_eq!(
        observe(&mut core, "left", Target::Workspace(SpaceId(11)), 1650)
            .unwrap()
            .window,
        None
    );
}
#[test]
fn same_connector_window_transfer_and_layer_or_minimize_change_restart_stationary_time() {
    let mut core = desktop();
    observe(&mut core, "left", Target::Window(1), 0);
    core.move_window(&1, SpaceId(11), Some("left".into()));
    assert!(core.dnd_hold().target().is_none());
    assert!(observe(&mut core, "left", Target::Window(1), 750).is_none());
    assert_eq!(
        observe(&mut core, "left", Target::Window(1), 1500)
            .unwrap()
            .space,
        SpaceId(11)
    );
    observe(&mut core, "left", Target::Window(1), 1600);
    core.apply(CoreEvent::SetLayer {
        id: 1,
        layer: Layer::Above,
    });
    assert!(core.dnd_hold().target().is_none());
    observe(&mut core, "left", Target::Window(1), 1700);
    core.set_minimized(&1, true);
    assert!(core.dnd_hold().target().is_none());
    assert!(observe(&mut core, "left", Target::Window(1), 3000).is_none());
    core.set_minimized(&1, false);
    assert!(observe(&mut core, "left", Target::Window(1), 3100).is_none());
    assert!(observe(&mut core, "left", Target::Window(1), 3850).is_some());
}
#[test]
fn retired_or_foreign_targets_never_publish_activation_and_clear_previous_timer() {
    let mut core = desktop();
    observe(&mut core, "left", Target::Workspace(SpaceId(11)), 0);
    core.apply(CoreEvent::ForgetSpace { id: SpaceId(11) });
    assert!(core.dnd_hold().target().is_none());
    assert!(observe(&mut core, "left", Target::Workspace(SpaceId(11)), 1000).is_none());
    observe(&mut core, "left", Target::Window(1), 1100);
    assert!(observe(&mut core, "left", Target::Workspace(SpaceId(20)), 2000).is_none());
    assert!(core.dnd_hold().target().is_none());
    observe(&mut core, "left", Target::Window(1), 2100);
    core.remove_window(&1);
    assert!(core.dnd_hold().target().is_none());
    assert!(observe(&mut core, "left", Target::Window(1), 4000).is_none());
}
#[test]
fn reconnecting_the_same_connector_requires_new_motion_and_a_full_new_hold() {
    let mut core = desktop();
    let old_generation = core.output("left").unwrap().generation;
    observe(&mut core, "left", Target::Window(1), 0);
    core.apply(CoreEvent::OutputDisconnected {
        connector: "left".into(),
    });
    assert!(core.dnd_hold().connection().is_none());
    core.apply(CoreEvent::OutputRegistered {
        connector: "left".into(),
        bounds: Rect::new(0., 0., 1920., 1080.),
    });
    core.apply(CoreEvent::SetOutput {
        id: 1,
        output: Some("left".into()),
        remember: true,
    });
    assert_ne!(core.output("left").unwrap().generation, old_generation);
    assert!(observe(&mut core, "left", Target::Window(1), 1000).is_none());
    core.apply(CoreEvent::DndHold(Action::Motion {
        connector: "left".into(),
    }));
    assert!(observe(&mut core, "left", Target::Window(1), 1100).is_none());
    assert!(observe(&mut core, "left", Target::Window(1), 1849).is_none());
    assert!(observe(&mut core, "left", Target::Window(1), 1850).is_some());
    core.apply(CoreEvent::DndHold(Action::End));
    assert!(core.dnd_hold().connection().is_none());
}
