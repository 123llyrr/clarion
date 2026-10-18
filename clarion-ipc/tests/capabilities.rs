use clarion_ipc::{Request, Response};

#[test]
fn optional_capabilities_use_a_stable_read_only_wire_format() {
    assert_eq!(
        serde_json::to_string(&Request::Capabilities).unwrap(),
        "\"Capabilities\""
    );
    let response = Response::Capabilities(vec!["desktop-menu-focus-restore".to_owned()]);
    assert_eq!(
        serde_json::to_string(&response).unwrap(),
        "{\"Capabilities\":[\"desktop-menu-focus-restore\"]}"
    );
    let Response::Capabilities(names) = serde_json::from_str::<Response>(
        "{\"Capabilities\":[\"future-feature\",\"desktop-menu-focus-restore\"]}",
    )
    .unwrap() else {
        panic!("expected capabilities")
    };
    assert_eq!(names.len(), 2);
}

#[test]
fn runtime_reduced_motion_action_has_explicit_boolean_state() {
    use clarion_ipc::Action;
    for enabled in [false, true] {
        let action = Action::SetReducedMotion {
            enabled,
            platform: false,
            initialize_only: false,
        };
        let json = serde_json::to_string(&action).unwrap();
        assert_eq!(
            json,
            format!("{{\"SetReducedMotion\":{{\"enabled\":{enabled}}}}}")
        );
        assert!(matches!(serde_json::from_str::<Action>(&json).unwrap(),
            Action::SetReducedMotion { enabled: actual, platform: false, initialize_only: false } if actual == enabled));
    }
}

#[test]
fn screenshot_preview_fields_are_backward_compatible_and_distinguishable() {
    use clarion_ipc::{Action, Event};
    let old =
        r#"{"ScreenshotWindow":{"id":1,"write_to_disk":false,"show_pointer":false,"path":null}}"#;
    assert!(matches!(
        serde_json::from_str::<Action>(old).unwrap(),
        Action::ScreenshotWindow {
            is_preview: false,
            ..
        }
    ));
    let old_event = r#"{"ScreenshotCaptured":{"path":null}}"#;
    assert!(matches!(
        serde_json::from_str::<Event>(old_event).unwrap(),
        Event::ScreenshotCaptured {
            is_preview: false,
            ..
        }
    ));
    assert_eq!(
        serde_json::to_string(&Event::ScreenshotCaptured {
            path: None,
            is_preview: false
        })
        .unwrap(),
        old_event
    );
    let preview = Event::ScreenshotCaptured {
        path: Some("/tmp/preview.png".to_owned()),
        is_preview: true,
    };
    assert!(matches!(
        serde_json::from_str::<Event>(&serde_json::to_string(&preview).unwrap()).unwrap(),
        Event::ScreenshotCaptured {
            is_preview: true,
            ..
        }
    ));
}

#[test]
fn reduced_motion_session_snapshot_is_absent_until_initialized() {
    use clarion_ipc::state::{EventStreamState, EventStreamStatePart};
    use clarion_ipc::Event;
    let mut state = EventStreamState::default();
    assert_eq!(state.reduced_motion.enabled, None);
    assert!(!state
        .replicate()
        .iter()
        .any(|event| matches!(event, Event::ReducedMotionChanged { .. })));
    for enabled in [true, false, true] {
        assert!(state
            .apply(Event::ReducedMotionChanged { enabled })
            .is_none());
        assert_eq!(state.reduced_motion.enabled, Some(enabled));
        let motion: Vec<_> = state
            .replicate()
            .into_iter()
            .filter_map(|event| match event {
                Event::ReducedMotionChanged { enabled } => Some(enabled),
                _ => None,
            })
            .collect();
        assert_eq!(motion, vec![enabled]);
    }
    assert_eq!(
        serde_json::to_string(&Request::ReducedMotionState).unwrap(),
        "\"ReducedMotionState\""
    );
    assert_eq!(
        serde_json::to_string(&Response::ReducedMotionState(None)).unwrap(),
        "{\"ReducedMotionState\":null}"
    );
    assert_eq!(
        serde_json::to_string(&Response::ReducedMotionState(Some(false))).unwrap(),
        "{\"ReducedMotionState\":false}"
    );
}
