use std::time::Duration;

use clarion_config::Config;

use super::Fixture;

fn candidate() -> clarion_config::Outputs {
    Config::parse_mem("output \"headless-1\" { scale 2; transform \"90\"; position x=20 y=40; }")
        .unwrap()
        .outputs
}

#[test]
fn preview_cancel_restores_exact_transient_config_and_scale() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    f.clarion_state().apply_transient_output_config(
        "headless-1",
        clarion_ipc::OutputAction::Scale {
            scale: clarion_ipc::ScaleToSet::Specific(1.5),
        },
    );
    let original = f.clarion().config.borrow().outputs.clone();
    let file = f.clarion().config_file_output_config.clone();
    let id = f
        .clarion_state()
        .begin_output_preview(candidate(), Duration::from_secs(15))
        .unwrap();
    assert_eq!(f.clarion_output(1).current_scale().fractional_scale(), 2.0);
    f.clarion_state().finish_output_preview(id, false).unwrap();
    assert_eq!(f.clarion().config.borrow().outputs, original);
    assert_eq!(f.clarion().config_file_output_config, file);
    assert_eq!(f.clarion_output(1).current_scale().fractional_scale(), 1.5);
}

#[test]
fn compositor_timer_restores_without_client() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let original = f.clarion().config.borrow().outputs.clone();
    let id = f
        .clarion_state()
        .begin_output_preview(candidate(), Duration::ZERO)
        .unwrap();
    f.state.server.dispatch();
    assert!(f.clarion().output_preview.is_none());
    assert_eq!(f.clarion().config.borrow().outputs, original);
    assert_eq!(f.clarion_output(1).current_scale().fractional_scale(), 1.0);
    // The client's cancellation can arrive just after the compositor timer.
    f.clarion_state().finish_output_preview(id, false).unwrap();
    assert!(f.clarion_state().finish_output_preview(id, true).is_err());
}

#[test]
fn late_confirmation_restores_even_before_timer_dispatch() {
    let mut f = Fixture::new();
    let original = f.clarion().config.borrow().outputs.clone();
    let id = f
        .clarion_state()
        .begin_output_preview(candidate(), Duration::ZERO)
        .unwrap();
    assert!(f.clarion_state().finish_output_preview(id, true).is_err());
    assert_eq!(f.clarion().config.borrow().outputs, original);
}

#[test]
fn confirmation_keeps_preview_after_timer_deadline() {
    let mut f = Fixture::new();
    let id = f
        .clarion_state()
        .begin_output_preview(candidate(), Duration::from_millis(20))
        .unwrap();
    f.clarion_state().finish_output_preview(id, true).unwrap();
    std::thread::sleep(Duration::from_millis(25));
    f.state.server.dispatch();
    assert_eq!(f.clarion().config.borrow().outputs, candidate());
    assert!(f.clarion().output_preview.is_none());
}

#[test]
fn confirmed_preview_can_be_compensated_if_client_cannot_save() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    f.clarion_state().apply_transient_output_config(
        "headless-1",
        clarion_ipc::OutputAction::Scale {
            scale: clarion_ipc::ScaleToSet::Specific(1.5),
        },
    );
    let original = f.clarion().config.borrow().outputs.clone();
    let id = f
        .clarion_state()
        .begin_output_preview(candidate(), Duration::from_secs(15))
        .unwrap();
    f.clarion_state().finish_output_preview(id, true).unwrap();
    assert!(f
        .clarion_state()
        .finish_output_preview(id.wrapping_add(1), false)
        .is_err());
    assert_eq!(f.clarion().config.borrow().outputs, candidate());
    f.clarion_state().finish_output_preview(id, false).unwrap();
    assert_eq!(f.clarion().config.borrow().outputs, original);
    assert_eq!(f.clarion_output(1).current_scale().fractional_scale(), 1.5);
    // Retrying after a lost rollback reply is safe.
    f.clarion_state().finish_output_preview(id, false).unwrap();
    assert!(f.clarion().confirmed_output_preview.is_none());
}

#[test]
fn external_changes_invalidate_confirmed_compensation() {
    for reload in [false, true] {
        let mut f = Fixture::new();
        let id = f
            .clarion_state()
            .begin_output_preview(candidate(), Duration::from_secs(15))
            .unwrap();
        f.clarion_state().finish_output_preview(id, true).unwrap();
        if reload {
            f.clarion_state().reload_config(Ok(Config::default()));
        } else {
            f.clarion_state()
                .apply_transient_output_config("headless-1", clarion_ipc::OutputAction::Off);
        }
        let current = f.clarion().config.borrow().outputs.clone();
        assert!(f.clarion_state().finish_output_preview(id, false).is_err());
        assert_eq!(f.clarion().config.borrow().outputs, current);
        assert!(f.clarion().confirmed_output_preview.is_none());
    }
}

#[test]
fn newer_preview_cannot_be_cancelled_by_previous_confirmed_token() {
    let mut f = Fixture::new();
    let first = f
        .clarion_state()
        .begin_output_preview(candidate(), Duration::from_secs(15))
        .unwrap();
    f.clarion_state()
        .finish_output_preview(first, true)
        .unwrap();
    let second = f
        .clarion_state()
        .begin_output_preview(Default::default(), Duration::from_secs(15))
        .unwrap();
    assert!(f
        .clarion_state()
        .finish_output_preview(first, false)
        .is_err());
    assert!(f.clarion().config.borrow().outputs.0.is_empty());
    f.clarion_state()
        .finish_output_preview(second, false)
        .unwrap();
    assert_eq!(f.clarion().config.borrow().outputs, candidate());
}

#[test]
fn concurrent_preview_and_wrong_token_cannot_replace_original() {
    let mut f = Fixture::new();
    let id = f
        .clarion_state()
        .begin_output_preview(candidate(), Duration::from_secs(15))
        .unwrap();
    assert!(f
        .clarion_state()
        .begin_output_preview(Default::default(), Duration::ZERO)
        .is_err());
    assert!(f
        .clarion_state()
        .finish_output_preview(id.wrapping_add(1), true)
        .is_err());
    assert_eq!(f.clarion().config.borrow().outputs, candidate());
    f.clarion_state().finish_output_preview(id, false).unwrap();
    assert!(f.clarion().config.borrow().outputs.0.is_empty());
}

#[test]
fn output_removal_during_preview_does_not_break_recovery() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    f.clarion_state()
        .begin_output_preview(candidate(), Duration::ZERO)
        .unwrap();
    let output = f.clarion_output(1);
    f.clarion().remove_output(&output);
    f.state.server.dispatch();
    assert!(f.clarion().output_preview.is_none());
    assert!(f.clarion().config.borrow().outputs.0.is_empty());
}

#[test]
fn output_reload_still_cancels_an_active_touch_sequence() {
    use smithay::input::touch::DownEvent;
    use smithay::utils::SERIAL_COUNTER;
    let mut f = Fixture::new();
    let touch = f.clarion().seat.add_touch();
    assert!(!touch.is_grabbed());
    touch.down(
        f.clarion_state(),
        None,
        &DownEvent {
            slot: Some(0).into(),
            location: (10.0, 10.0).into(),
            serial: SERIAL_COUNTER.next_serial(),
            time: smithay::backend::input::InputTime::from_millis(0),
        },
    );
    assert!(touch.is_grabbed());
    f.clarion_state().reload_output_config();
    assert!(!touch.is_grabbed());
    touch.down(
        f.clarion_state(),
        None,
        &DownEvent {
            slot: Some(0).into(),
            location: (10.0, 10.0).into(),
            serial: SERIAL_COUNTER.next_serial(),
            time: smithay::backend::input::InputTime::from_millis(1),
        },
    );
    f.clarion_state()
        .do_action(clarion_config::Action::CloseOverview, false);
    assert!(!touch.is_grabbed());
}

#[test]
fn external_reload_invalidates_preview_token() {
    let mut f = Fixture::new();
    let id = f
        .clarion_state()
        .begin_output_preview(candidate(), Duration::from_secs(15))
        .unwrap();
    f.clarion_state().reload_config(Ok(Config::default()));
    assert!(f.clarion_state().finish_output_preview(id, true).is_err());
    assert!(f.clarion().config.borrow().outputs.0.is_empty());
}

#[test]
fn external_output_action_survives_old_preview_deadline() {
    let mut f = Fixture::new();
    let id = f
        .clarion_state()
        .begin_output_preview(candidate(), Duration::ZERO)
        .unwrap();
    f.clarion_state()
        .apply_transient_output_config("headless-1", clarion_ipc::OutputAction::Off);
    f.state.server.dispatch();
    f.clarion_state().finish_output_preview(id, false).unwrap();
    let config = f.clarion().config.borrow();
    assert!(config.outputs.0[0].off);
    assert!(config.outputs.0[0].scale.is_none());
}

#[test]
fn included_output_draft_is_parsed_without_writing() {
    let shell = xshell::Shell::new().unwrap();
    let dir = shell.create_temp_dir().unwrap();
    let root = dir.path().join("config.kdl");
    let include = dir.path().join("display.kdl");
    std::fs::write(&root, "include \"display.kdl\"\n").unwrap();
    std::fs::write(&include, "// unchanged\n").unwrap();
    let outputs = crate::output_preview::parse_outputs(
        root.to_str().unwrap(),
        include.to_str().unwrap(),
        "output \"headless-1\" { scale 2; }".into(),
    )
    .unwrap();
    assert_eq!(outputs.0[0].scale.unwrap().0, 2.0);
    assert_eq!(std::fs::read_to_string(&include).unwrap(), "// unchanged\n");
    assert!(crate::output_preview::parse_outputs(
        root.to_str().unwrap(),
        dir.path().join("other.kdl").to_str().unwrap(),
        String::new()
    )
    .is_err());
    assert!(
        crate::output_preview::parse_outputs("relative.kdl", "relative.kdl", String::new())
            .is_err()
    );
}

#[test]
fn hdr_brightness_updates_only_named_output_without_repositioning() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    f.add_output(2, (2560, 1440));
    let first = f.clarion_output(1).clone();
    let second = f.clarion_output(2).clone();
    let first_mode = first.current_mode();
    let second_mode = second.current_mode();
    f.clarion_state().apply_transient_output_config(
        "headless-1",
        clarion_ipc::OutputAction::HdrBrightness { percent: 35.0 },
    );
    let before_invalid = f.clarion().config.borrow().outputs.clone();
    assert_eq!(
        before_invalid
            .0
            .iter()
            .find(|c| c.name == "headless-1")
            .unwrap()
            .hdr_brightness,
        Some(clarion_config::FloatOrInt(35.0))
    );
    assert!(before_invalid
        .0
        .iter()
        .filter(|c| c.name != "headless-1")
        .all(|c| c.hdr_brightness.is_none()));
    assert_eq!(first.current_mode(), first_mode);
    assert_eq!(second.current_mode(), second_mode);
    f.clarion_state().apply_transient_output_config(
        "headless-1",
        clarion_ipc::OutputAction::HdrBrightness { percent: f64::NAN },
    );
    assert_eq!(f.clarion().config.borrow().outputs, before_invalid);
}
