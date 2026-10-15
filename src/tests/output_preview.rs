use super::Fixture;
use realand_config::Config;
use std::time::Duration;

fn candidate() -> realand_config::Outputs {
    Config::parse_mem("output \"headless-1\" { scale 2; transform \"90\"; position x=20 y=40; }")
        .unwrap()
        .outputs
}

#[test]
fn preview_cancel_restores_exact_transient_config_and_scale() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    f.realand_state().apply_transient_output_config(
        "headless-1",
        realand_ipc::OutputAction::Scale {
            scale: realand_ipc::ScaleToSet::Specific(1.5),
        },
    );
    let original = f.realand().config.borrow().outputs.clone();
    let file = f.realand().config_file_output_config.clone();
    let id = f
        .realand_state()
        .begin_output_preview(candidate(), Duration::from_secs(15))
        .unwrap();
    assert_eq!(f.realand_output(1).current_scale().fractional_scale(), 2.0);
    f.realand_state().finish_output_preview(id, false).unwrap();
    assert_eq!(f.realand().config.borrow().outputs, original);
    assert_eq!(f.realand().config_file_output_config, file);
    assert_eq!(f.realand_output(1).current_scale().fractional_scale(), 1.5);
}

#[test]
fn compositor_timer_restores_without_client() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let original = f.realand().config.borrow().outputs.clone();
    let id = f
        .realand_state()
        .begin_output_preview(candidate(), Duration::ZERO)
        .unwrap();
    f.state.server.dispatch();
    assert!(f.realand().output_preview.is_none());
    assert_eq!(f.realand().config.borrow().outputs, original);
    assert_eq!(f.realand_output(1).current_scale().fractional_scale(), 1.0);
    // The client's cancellation can arrive just after the compositor timer.
    f.realand_state().finish_output_preview(id, false).unwrap();
    assert!(f.realand_state().finish_output_preview(id, true).is_err());
}

#[test]
fn late_confirmation_restores_even_before_timer_dispatch() {
    let mut f = Fixture::new();
    let original = f.realand().config.borrow().outputs.clone();
    let id = f
        .realand_state()
        .begin_output_preview(candidate(), Duration::ZERO)
        .unwrap();
    assert!(f.realand_state().finish_output_preview(id, true).is_err());
    assert_eq!(f.realand().config.borrow().outputs, original);
}

#[test]
fn confirmation_keeps_preview_after_timer_deadline() {
    let mut f = Fixture::new();
    let id = f
        .realand_state()
        .begin_output_preview(candidate(), Duration::from_millis(20))
        .unwrap();
    f.realand_state().finish_output_preview(id, true).unwrap();
    std::thread::sleep(Duration::from_millis(25));
    f.state.server.dispatch();
    assert_eq!(f.realand().config.borrow().outputs, candidate());
    assert!(f.realand().output_preview.is_none());
}

#[test]
fn confirmed_preview_can_be_compensated_if_client_cannot_save() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    f.realand_state().apply_transient_output_config(
        "headless-1",
        realand_ipc::OutputAction::Scale {
            scale: realand_ipc::ScaleToSet::Specific(1.5),
        },
    );
    let original = f.realand().config.borrow().outputs.clone();
    let id = f
        .realand_state()
        .begin_output_preview(candidate(), Duration::from_secs(15))
        .unwrap();
    f.realand_state().finish_output_preview(id, true).unwrap();
    assert!(f
        .realand_state()
        .finish_output_preview(id.wrapping_add(1), false)
        .is_err());
    assert_eq!(f.realand().config.borrow().outputs, candidate());
    f.realand_state().finish_output_preview(id, false).unwrap();
    assert_eq!(f.realand().config.borrow().outputs, original);
    assert_eq!(f.realand_output(1).current_scale().fractional_scale(), 1.5);
    // Retrying after a lost rollback reply is safe.
    f.realand_state().finish_output_preview(id, false).unwrap();
    assert!(f.realand().confirmed_output_preview.is_none());
}

#[test]
fn external_changes_invalidate_confirmed_compensation() {
    for reload in [false, true] {
        let mut f = Fixture::new();
        let id = f
            .realand_state()
            .begin_output_preview(candidate(), Duration::from_secs(15))
            .unwrap();
        f.realand_state().finish_output_preview(id, true).unwrap();
        if reload {
            f.realand_state().reload_config(Ok(Config::default()));
        } else {
            f.realand_state()
                .apply_transient_output_config("headless-1", realand_ipc::OutputAction::Off);
        }
        let current = f.realand().config.borrow().outputs.clone();
        assert!(f.realand_state().finish_output_preview(id, false).is_err());
        assert_eq!(f.realand().config.borrow().outputs, current);
        assert!(f.realand().confirmed_output_preview.is_none());
    }
}

#[test]
fn newer_preview_cannot_be_cancelled_by_previous_confirmed_token() {
    let mut f = Fixture::new();
    let first = f
        .realand_state()
        .begin_output_preview(candidate(), Duration::from_secs(15))
        .unwrap();
    f.realand_state()
        .finish_output_preview(first, true)
        .unwrap();
    let second = f
        .realand_state()
        .begin_output_preview(Default::default(), Duration::from_secs(15))
        .unwrap();
    assert!(f
        .realand_state()
        .finish_output_preview(first, false)
        .is_err());
    assert!(f.realand().config.borrow().outputs.0.is_empty());
    f.realand_state()
        .finish_output_preview(second, false)
        .unwrap();
    assert_eq!(f.realand().config.borrow().outputs, candidate());
}

#[test]
fn concurrent_preview_and_wrong_token_cannot_replace_original() {
    let mut f = Fixture::new();
    let id = f
        .realand_state()
        .begin_output_preview(candidate(), Duration::from_secs(15))
        .unwrap();
    assert!(f
        .realand_state()
        .begin_output_preview(Default::default(), Duration::ZERO)
        .is_err());
    assert!(f
        .realand_state()
        .finish_output_preview(id.wrapping_add(1), true)
        .is_err());
    assert_eq!(f.realand().config.borrow().outputs, candidate());
    f.realand_state().finish_output_preview(id, false).unwrap();
    assert!(f.realand().config.borrow().outputs.0.is_empty());
}

#[test]
fn output_removal_during_preview_does_not_break_recovery() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    f.realand_state()
        .begin_output_preview(candidate(), Duration::ZERO)
        .unwrap();
    let output = f.realand_output(1);
    f.realand().remove_output(&output);
    f.state.server.dispatch();
    assert!(f.realand().output_preview.is_none());
    assert!(f.realand().config.borrow().outputs.0.is_empty());
}

#[test]
fn output_reload_still_cancels_an_active_touch_sequence() {
    use smithay::input::touch::DownEvent;
    use smithay::utils::SERIAL_COUNTER;
    let mut f = Fixture::new();
    let touch = f.realand().seat.add_touch();
    assert!(!touch.is_grabbed());
    touch.down(
        f.realand_state(),
        None,
        &DownEvent {
            slot: Some(0).into(),
            location: (10.0, 10.0).into(),
            serial: SERIAL_COUNTER.next_serial(),
            time: 0,
        },
    );
    assert!(touch.is_grabbed());
    f.realand_state().reload_output_config();
    assert!(!touch.is_grabbed());
    touch.down(
        f.realand_state(),
        None,
        &DownEvent {
            slot: Some(0).into(),
            location: (10.0, 10.0).into(),
            serial: SERIAL_COUNTER.next_serial(),
            time: 1,
        },
    );
    f.realand_state()
        .do_action(realand_config::Action::CloseOverview, false);
    assert!(!touch.is_grabbed());
}

#[test]
fn external_reload_invalidates_preview_token() {
    let mut f = Fixture::new();
    let id = f
        .realand_state()
        .begin_output_preview(candidate(), Duration::from_secs(15))
        .unwrap();
    f.realand_state().reload_config(Ok(Config::default()));
    assert!(f.realand_state().finish_output_preview(id, true).is_err());
    assert!(f.realand().config.borrow().outputs.0.is_empty());
}

#[test]
fn external_output_action_survives_old_preview_deadline() {
    let mut f = Fixture::new();
    let id = f
        .realand_state()
        .begin_output_preview(candidate(), Duration::ZERO)
        .unwrap();
    f.realand_state()
        .apply_transient_output_config("headless-1", realand_ipc::OutputAction::Off);
    f.state.server.dispatch();
    f.realand_state().finish_output_preview(id, false).unwrap();
    let config = f.realand().config.borrow();
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
