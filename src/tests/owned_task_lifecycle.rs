//! Real owner-thread delivery guards for asynchronous preparation.
use super::*;
use crate::tests::Fixture;

fn empty_session() -> realand_ipc::management::SessionLayout {
    crate::ipc::server::parse_session_layout(
        r#"{"version":1,"workspaces":[],"windows":[],"groups":[]}"#,
    )
    .unwrap()
}

#[test]
fn newer_restore_intent_rejects_older_prepared_session() {
    let mut fixture = Fixture::new();
    let state = fixture.realand_state();
    let older = state.begin_session_layout_restore().unwrap();
    let newer = state.begin_session_layout_restore().unwrap();
    assert_eq!(
        state
            .apply_session_layout_restore(older, empty_session())
            .unwrap_err(),
        "新的布局恢复请求已取消此请求"
    );
    assert!(state
        .apply_session_layout_restore(newer, empty_session())
        .is_ok());
}

#[test]
fn lock_epoch_cancels_prepared_restore_even_after_unlock() {
    let mut fixture = Fixture::new();
    let state = fixture.realand_state();
    let old = state.begin_session_layout_restore().unwrap();
    state.realand.capture_scene_epoch.invalidate();
    assert!(!state.realand.is_locked());
    assert_eq!(
        state
            .apply_session_layout_restore(old, empty_session())
            .unwrap_err(),
        "会话锁定已取消布局恢复"
    );
    let current = state.begin_session_layout_restore().unwrap();
    assert!(state
        .apply_session_layout_restore(current, empty_session())
        .is_ok());
}

#[test]
fn failed_old_keymap_read_cannot_apply_fallback_over_newer_config() {
    let shell = xshell::Shell::new().unwrap();
    let directory = shell.create_temp_dir().unwrap();
    let mut fixture = Fixture::new();
    let state = fixture.realand_state();
    // The missing file normally falls back to the default map. Delivery is held
    // until the main loop runs; a newer direct config must supersede that fallback.
    state.queue_xkb_file(
        directory
            .path()
            .join("missing-map")
            .to_string_lossy()
            .into_owned(),
        Some(Xkb::default()),
    );
    state.set_xkb_config(XkbConfig {
        layout: "de",
        ..Default::default()
    });
    let request = state.realand.keymap_request;
    let deadline = Instant::now() + Duration::from_secs(2);
    while fixture.realand().keymap_completions == 0 {
        assert!(
            Instant::now() < deadline,
            "keymap completion was not delivered"
        );
        fixture.state.server.dispatch();
        std::thread::sleep(Duration::from_millis(1));
    }
    let state = fixture.realand_state();
    assert_eq!(state.realand.keymap_request, request);
    let keyboard = state.realand.seat.get_keyboard().unwrap();
    let names: Vec<String> = keyboard.with_xkb_state(state, |context| {
        let keymap = context.xkb().lock().unwrap();
        keymap
            .layouts()
            .map(|layout| keymap.layout_name(layout).to_owned())
            .collect()
    });
    assert_eq!(names, ["German"]);
}

#[test]
fn gamma_adapter_returns_unsupported_without_a_tty_backend() {
    use crate::protocols::gamma_control::GammaControlHandler;
    let mut fixture = Fixture::new();
    fixture.add_output(1, (800, 600));
    let output = fixture.realand_output(1);
    let state = fixture.realand_state();
    assert_eq!(state.get_gamma_size(&output), None);
    assert_eq!(state.set_gamma(&output, Some(vec![0; 3])), None);
    assert_eq!(state.set_gamma(&output, None), None);
}
