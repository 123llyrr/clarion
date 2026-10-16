use clarion_core::workspace_switch::WorkspaceSwitchAction as Action;
use clarion_core::{CoreEffect, CoreEvent, DesktopCore, LayoutMode, Rect, SpaceId};
use std::time::Duration;

fn desktop() -> DesktopCore<u64> {
    let mut core = DesktopCore::new();
    for output in ["LEFT", "RIGHT"] {
        core.apply(CoreEvent::OutputRegistered {
            connector: output.into(),
            bounds: Rect::new(0., 0., 1000., 800.),
        });
    }
    for (id, output, index) in [
        (10, "LEFT", 0),
        (20, "LEFT", 1),
        (30, "LEFT", 2),
        (40, "RIGHT", 0),
    ] {
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
    core.apply(CoreEvent::DeactivateSpace);
    core
}
fn begin(connector: &str, center: u64, current: f64) -> CoreEvent<u64> {
    CoreEvent::WorkspaceSwitch(Action::Begin {
        connector: connector.into(),
        center: SpaceId(center),
        current,
        is_touchpad: true,
        is_clamped: true,
        dnd_timestamp: None,
    })
}
fn finish(cancelled: bool) -> CoreEvent<u64> {
    let connector = "LEFT".into();
    let is_touchpad = Some(true);
    let timestamp = Duration::from_millis(25);
    let zoom = 1.;
    let viewport_height = 1000.;
    let extra_position = 0.;
    let extra_velocity = 0.;
    CoreEvent::WorkspaceSwitch(if cancelled {
        Action::Cancel {
            connector,
            is_touchpad,
            timestamp,
            zoom,
            viewport_height,
            extra_position,
            extra_velocity,
        }
    } else {
        Action::End {
            connector,
            is_touchpad,
            timestamp,
            zoom,
            viewport_height,
            extra_position,
            extra_velocity,
        }
    })
}

#[test]
fn gesture_order_rebases_by_center_identity_and_release_selects_the_same_core_navigation() {
    let mut core = desktop();
    core.apply(begin("LEFT", 20, 1.));
    core.apply(CoreEvent::WorkspaceSwitch(Action::Update {
        connector: "LEFT".into(),
        delta_y: 120.,
        timestamp: Duration::from_millis(10),
        is_touchpad: true,
        zoom: 1.,
        viewport_height: 1000.,
    }));
    let before = core.workspace_switch().gesture("LEFT").unwrap().current();
    core.apply(CoreEvent::ReorderSpace {
        id: SpaceId(10),
        index: 2,
    });
    let gesture = core.workspace_switch().gesture("LEFT").unwrap();
    assert_eq!(gesture.center(), SpaceId(20));
    assert!((gesture.current() - (before - 1.)).abs() < 1e-12);
    let effects = core.apply(finish(false));
    let settlement = effects
        .iter()
        .find_map(|effect| match effect {
            CoreEffect::WorkspaceSwitchChanged { outcome } => outcome.settlement,
            _ => None,
        })
        .unwrap();
    assert!(settlement.remember_previous);
    assert_eq!(
        core.navigation().active_space("LEFT"),
        Some(settlement.target)
    );
    assert!(core.workspace_switch().gesture("LEFT").is_none());
    // Display settlement cannot lift a lock/capture's suspended focus scope.
    assert_eq!(core.active_space(), None);
    assert_eq!(core.active_window(), None);
}

#[test]
fn retired_centers_repair_without_stale_ids_and_output_removal_clears_only_its_own_gesture() {
    let mut core = desktop();
    core.apply(begin("LEFT", 20, 1.));
    core.apply(begin("RIGHT", 40, 0.));
    core.apply(CoreEvent::ForgetSpace { id: SpaceId(20) });
    assert_eq!(
        core.workspace_switch().gesture("LEFT").unwrap().center(),
        SpaceId(30)
    );
    core.apply(CoreEvent::OutputDisconnected {
        connector: "LEFT".into(),
    });
    assert!(core.workspace_switch().gesture("LEFT").is_none());
    assert_eq!(
        core.workspace_switch().gesture("RIGHT").unwrap().center(),
        SpaceId(40)
    );
    core.apply(CoreEvent::OutputConnected {
        connector: "LEFT".into(),
        bounds: Rect::new(0., 0., 1000., 800.),
    });
    assert!(core.workspace_switch().gesture("LEFT").is_none());
    assert_eq!(core.active_space(), None);
}

#[test]
fn foreign_begin_is_rejected_and_cancel_retains_existing_display_selection() {
    let mut core = desktop();
    assert!(core.apply(begin("UNKNOWN", 20, 1.)).is_empty());
    assert!(core.apply(begin("RIGHT", 20, 1.)).is_empty());
    assert!(core.workspace_switch().gesture("RIGHT").is_none());
    core.apply(CoreEvent::SelectOutputSpace {
        connector: "LEFT".into(),
        id: SpaceId(20),
        remember_previous: true,
    });
    core.apply(begin("LEFT", 20, 1.));
    core.apply(CoreEvent::SelectOutputSpace {
        connector: "LEFT".into(),
        id: SpaceId(30),
        remember_previous: true,
    });
    let previous = core.navigation().previous_space("LEFT");
    let effects = core.apply(finish(true));
    let settlement = effects
        .iter()
        .find_map(|effect| match effect {
            CoreEffect::WorkspaceSwitchChanged { outcome } => outcome.settlement,
            _ => None,
        })
        .unwrap();
    assert!(!settlement.remember_previous);
    assert_eq!(settlement.target, SpaceId(20));
    assert_eq!(core.navigation().active_space("LEFT"), Some(SpaceId(20)));
    assert_eq!(core.navigation().previous_space("LEFT"), previous);
    assert!(core.workspace_switch().gesture("LEFT").is_none());
    assert_eq!(core.active_space(), None);
}
