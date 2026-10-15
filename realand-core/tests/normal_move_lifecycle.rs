//! Interaction-command regressions exercise the authoritative event boundary,
//! rather than a second platform copy of its move controls.
use realand_core::desktop::{Layer, MovePhase};
use realand_core::move_controls::{MovePlacement, MoveStart, MoveStartIntent};
use realand_core::overview::{OverviewAction, OverviewMode};
use realand_core::policy_memory::Width;
use realand_core::{CoreEffect, CoreEvent, DesktopCore, Rect, SpaceId, WindowRecord};

fn desktop() -> DesktopCore<u64> {
    let mut core = DesktopCore::new();
    for (name, space) in [("left", SpaceId(10)), ("right", SpaceId(20))] {
        core.apply(CoreEvent::OutputConnected {
            connector: name.into(),
            bounds: Rect::new(0., 0., 1920., 1080.),
        });
        core.apply(CoreEvent::CreateSpace {
            id: space,
            mode: realand_core::LayoutMode::Scrolling,
        });
        core.apply(CoreEvent::AttachSpace {
            id: space,
            connector: name.into(),
            index: 0,
        });
    }
    let mut window = WindowRecord::new(1, SpaceId(10), Rect::new(10., 20., 400., 300.));
    window.output = Some("left".into());
    core.apply(CoreEvent::MapWindow { window });
    core
}
fn begin(core: &mut DesktopCore<u64>) {
    core.apply(CoreEvent::BeginMove {
        id: 1,
        start: MoveStart {
            pointer: [160., 120.],
            // A scene-zoomed client rectangle differs from the reserved tile
            // slot, so its anchor must come from the supplied visual client.
            visual_window: Rect::new(60., 70., 400., 100.),
            width: Width::Proportion(0.73),
            full_width: true,
        },
    });
}
fn motion(core: &mut DesktopCore<u64>, delta: [f64; 2], zoom: f64) -> Option<MoveStartIntent> {
    core.apply(CoreEvent::StartMoveMotion { id: 1, delta, zoom })
        .into_iter()
        .find_map(|effect| match effect {
            CoreEffect::MoveStartIntent(intent) => Some(intent),
            _ => None,
        })
}
#[test]
fn starting_motion_owns_threshold_anchor_and_current_source_layer() {
    let mut core = desktop();
    begin(&mut core);
    assert_eq!(core.interaction().unwrap().controls.anchor(), [0.25, 0.5]);
    let first = motion(&mut core, [128., 0.], 1.).unwrap();
    assert!(!first.detach);
    assert_eq!(core.interaction().unwrap().phase, MovePhase::Starting);
    assert!(core
        .apply(CoreEvent::SetMoveFloating {
            id: 1,
            floating: true
        })
        .is_empty());
    // An explicit mode/layer transition while the grab is still Starting must
    // be read at the next event, not copied from BeginMove forever.
    core.apply(CoreEvent::SetLayer {
        id: 1,
        layer: Layer::Above,
    });
    assert!(motion(&mut core, [0., 0.], 1.).unwrap().detach);
    assert_eq!(core.interaction().unwrap().phase, MovePhase::Moving);
    assert_eq!(
        core.interaction().unwrap().controls.cumulative_delta(),
        [128., 0.]
    );
    assert!(core.interaction().unwrap().controls.placement().floating);
    assert!(motion(&mut core, [10., 0.], 1.).is_none());
}
#[test]
fn stage_threshold_uses_current_core_mode_and_screen_distance() {
    let mut core = desktop();
    core.apply(CoreEvent::Overview(OverviewAction::SetMode {
        mode: OverviewMode::Stage,
        timestamp: std::time::Duration::ZERO,
    }));
    core.apply(CoreEvent::Overview(OverviewAction::SetOpen(true)));
    begin(&mut core);
    assert!(!motion(&mut core, [15., 0.], 0.25).unwrap().detach);
    assert!(motion(&mut core, [2., 0.], 0.25).unwrap().detach);
    assert_eq!(
        core.interaction().unwrap().controls.cumulative_delta(),
        [68., 0.]
    );
}
#[test]
fn removal_ack_is_once_and_target_float_changes_do_not_rewrite_attached_layer() {
    let mut core = desktop();
    begin(&mut core);
    assert!(motion(&mut core, [256., 0.], 1.).unwrap().detach);
    // Unfullscreen/restoration can update the source layer between threshold
    // crossing and retained policy removal.
    core.apply(CoreEvent::SetLayer {
        id: 1,
        layer: Layer::Above,
    });
    let ack = CoreEvent::MoveDetached {
        id: 1,
        width: Width::Proportion(0.61),
        full_width: false,
    };
    assert!(core
        .apply(ack.clone())
        .contains(&CoreEffect::MoveDetachedAccepted));
    assert!(core.apply(ack).is_empty());
    core.detach_window(&1);
    assert_eq!(core.active_window(), Some(&1));
    let placement = MovePlacement {
        width: Width::Proportion(0.61),
        full_width: false,
        floating: false,
    };
    assert!(core
        .apply(CoreEvent::SetMoveFloating {
            id: 1,
            floating: false
        })
        .contains(&CoreEffect::MovePlacementIntent(placement)));
    assert_eq!(core.window(&1).unwrap().layer, Layer::Above);
    assert_eq!(core.interaction().unwrap().controls.placement(), placement);
    assert!(core
        .apply(CoreEvent::SetMoveFloating {
            id: 99,
            floating: true
        })
        .is_empty());
    core.move_window(&1, SpaceId(20), Some("right".into()));
    core.apply(CoreEvent::SetLayer {
        id: 1,
        layer: Layer::Normal,
    });
    core.apply(CoreEvent::MovePlaced { id: 1 });
    assert!(core.interaction().is_none());
    assert_eq!(core.navigation().selected_output(), Some("right"));
    assert_eq!(core.active_space(), Some(SpaceId(20)));
    assert_eq!(core.active_window(), Some(&1));
}
#[test]
fn invalid_start_and_motion_leave_owner_unchanged_and_unmap_rejects_late_packets() {
    let mut core = desktop();
    assert!(core
        .apply(CoreEvent::BeginMove {
            id: 1,
            start: MoveStart {
                pointer: [f64::NAN, 0.],
                ..MoveStart::default()
            }
        })
        .is_empty());
    assert!(core.interaction().is_none());
    begin(&mut core);
    let before = core.interaction().unwrap().clone();
    assert!(motion(&mut core, [1., 0.], 0.).is_none());
    assert_eq!(core.interaction(), Some(&before));
    core.remove_window(&1);
    assert!(core.interaction().is_none());
    assert!(motion(&mut core, [256., 0.], 1.).is_none());
    assert!(core
        .apply(CoreEvent::MoveDetached {
            id: 1,
            width: Width::Fixed(200.),
            full_width: false
        })
        .is_empty());
    assert!(core.apply(CoreEvent::MovePlaced { id: 1 }).is_empty());
}
