use super::*;

fn desktop() -> Layout<TestWindow> {
    check_ops([
        Op::AddOutput(1),
        Op::AddOutput(2),
        Op::FocusOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
    ])
}

#[test]
fn expired_connector_cannot_supply_a_protocol_focus_payload() {
    let layout = desktop();
    assert_eq!(layout.core_focus_id(), Some(0));
    let original = layout.core.borrow().window(&0).unwrap().output.clone();
    // Exercise a temporarily stale device descriptor at the protocol boundary.
    // Desktop identity survives; a wl_surface cannot be focused on a dead output.
    layout
        .core
        .borrow_mut()
        .set_output(&0, Some("retired-output".into()), false);
    assert_eq!(layout.core_focus_id(), Some(0));
    assert!(layout.core_focus_window(0).is_none());
    layout.core.borrow_mut().set_output(&0, original, false);
    assert_eq!(layout.core_focus_window(0).map(LayoutElement::id), Some(&0));
    layout.verify_invariants();
}

#[test]
fn detached_cross_output_move_supplies_only_its_live_focus_payload() {
    let mut layout = desktop();
    let source = layout.active_output().unwrap().clone();
    let destination = layout
        .outputs()
        .find(|output| output.name() == "output2")
        .unwrap()
        .clone();
    assert!(layout.interactive_move_begin(0, &source, (100., 100.).into()));
    assert!(layout.interactive_move_update(
        &0,
        (500., 100.).into(),
        destination.clone(),
        (200., 200.).into(),
    ));
    assert!(!layout.core.borrow().window(&0).unwrap().attached);
    assert_eq!(layout.core_focus_id(), Some(0));
    assert_eq!(layout.core_focus_window(0).map(LayoutElement::id), Some(&0));
    assert_eq!(layout.focus_with_output().unwrap().1, &destination);
    assert!(layout.remove_window(&0, Transaction::new()).is_some());
    assert!(layout.core_focus_window(0).is_none());
    assert!(layout.core_focus_id().is_none());
    layout.verify_invariants();
}

#[test]
fn closing_background_floating_windows_keeps_keyboard_focus_on_the_foreground() {
    for mode in [
        clarion_ipc::TilingMode::Scrolling,
        clarion_ipc::TilingMode::Dwindle,
    ] {
        let mut layout = check_ops([
            Op::AddOutput(1),
            Op::SwitchTilingMode(Some(mode)),
            Op::AddWindow {
                params: TestWindowParams {
                    is_floating: true,
                    ..TestWindowParams::new(1)
                },
            },
            Op::AddWindow {
                params: TestWindowParams {
                    is_floating: true,
                    ..TestWindowParams::new(2)
                },
            },
            Op::AddWindow {
                params: TestWindowParams {
                    is_floating: true,
                    ..TestWindowParams::new(3)
                },
            },
            Op::CompleteAnimations,
        ]);
        assert_eq!(layout.core_focus_id(), Some(3));
        assert!(layout.remove_window(&2, Transaction::new()).is_some());
        assert_eq!(layout.core_focus_id(), Some(3));
        assert_eq!(layout.core_focus_window(3).map(LayoutElement::id), Some(&3));
        assert!(layout.core_focus_window(2).is_none());
        assert!(layout.remove_window(&3, Transaction::new()).is_some());
        assert_eq!(layout.core_focus_id(), Some(1));
        assert_eq!(layout.core_focus_window(1).map(LayoutElement::id), Some(&1));
        layout.verify_invariants();
    }
}

#[test]
fn closing_floating_dialog_restores_parent_and_closing_parked_window_keeps_focus() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams {
                is_floating: true,
                ..TestWindowParams::new(1)
            },
        },
        Op::AddWindow {
            params: TestWindowParams {
                is_floating: true,
                parent_id: Some(1),
                ..TestWindowParams::new(2)
            },
        },
        Op::CompleteAnimations,
    ]);
    assert_eq!(layout.core_focus_id(), Some(2));
    assert!(layout.remove_window(&2, Transaction::new()).is_some());
    assert_eq!(layout.core_focus_id(), Some(1));
    Op::AddWindow {
        params: TestWindowParams {
            is_floating: true,
            ..TestWindowParams::new(3)
        },
    }
    .apply(&mut layout);
    assert!(layout.minimize_window(&3));
    assert_eq!(layout.core_focus_id(), Some(1));
    assert!(layout.remove_window(&3, Transaction::new()).is_some());
    assert_eq!(layout.core_focus_id(), Some(1));
    assert!(layout.core_focus_window(3).is_none());
    layout.verify_invariants();
}
