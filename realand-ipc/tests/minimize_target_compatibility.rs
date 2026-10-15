use realand_ipc::{Action, DockEdge, DockTarget, MinimizeEffect};
use serde_json::json;

#[test]
fn legacy_target_and_action_keep_bottom_genie_and_normal_motion() {
    let target_json = json!({"output":"eDP-1", "x":20., "y":700., "width":48., "height":48.});
    let target: DockTarget = serde_json::from_value(target_json.clone()).unwrap();
    assert!(target.is_valid());
    assert_eq!(target.edge, DockEdge::Bottom);
    assert_eq!(target.effect, MinimizeEffect::Genie);
    let action: Action =
        serde_json::from_value(json!({"SetMinimizeTarget":{"id":4,"target":target_json}})).unwrap();
    let Action::SetMinimizeTarget {
        id,
        target,
        reduce_motion,
    } = action
    else {
        panic!("wrong action")
    };
    assert_eq!(id, 4);
    assert!(!reduce_motion);
    assert_eq!(target.unwrap().effect, MinimizeEffect::Genie);
}

#[test]
fn every_edge_and_effect_round_trips_and_unknown_values_are_rejected() {
    for edge in [
        DockEdge::Bottom,
        DockEdge::Left,
        DockEdge::Right,
        DockEdge::Top,
    ] {
        for effect in [MinimizeEffect::Genie, MinimizeEffect::Scale] {
            let target = DockTarget {
                output: "HDMI-A-1".into(),
                x: 20.,
                y: 700.,
                width: 48.,
                height: 48.,
                edge,
                effect,
            };
            assert_eq!(
                serde_json::from_value::<DockTarget>(serde_json::to_value(&target).unwrap())
                    .unwrap(),
                target
            );
        }
    }
    let target =
        json!({"output":"eDP-1", "x":20., "y":700., "width":48., "height":48., "edge":"sideways"});
    assert!(serde_json::from_value::<DockTarget>(target).is_err());
    let target =
        json!({"output":"eDP-1", "x":20., "y":700., "width":48., "height":48., "effect":"magic"});
    assert!(serde_json::from_value::<DockTarget>(target).is_err());
}
