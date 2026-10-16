use super::*;

fn desktop(mode: clarion_ipc::TilingMode, floating: bool) -> Layout<TestWindow> {
    let mut options = Options::default();
    options.animations.off = true;
    options.layout.auto_stack_new_windows = false;
    check_ops_with_options(
        options,
        [
            Op::AddOutput(1),
            Op::AddOutput(2),
            Op::FocusOutput(1),
            Op::SwitchTilingMode(Some(mode)),
            Op::AddWindow {
                params: TestWindowParams {
                    is_floating: floating,
                    ..TestWindowParams::new(0)
                },
            },
            Op::CompleteAnimations,
        ],
    )
}

fn begin(layout: &mut Layout<TestWindow>) -> Output {
    let output = layout.active_monitor_ref().unwrap().output().clone();
    assert!(layout.interactive_move_begin(0, &output, (100., 100.).into()));
    output
}

#[test]
fn coalesced_move_releases_at_the_latest_pose_in_every_layout() {
    for mode in [
        clarion_ipc::TilingMode::Scrolling,
        clarion_ipc::TilingMode::Dwindle,
    ] {
        for floating in [false, true] {
            let mut queued = desktop(mode, floating);
            let output = begin(&mut queued);
            for n in 1..=100 {
                assert!(queued.queue_interactive_move_update(
                    &0,
                    (4., 0.5).into(),
                    output.clone(),
                    (100. + 4. * n as f64, 100. + 0.5 * n as f64).into()
                ));
            }
            assert!(queued.pending_move.is_some());
            // Releasing before the next rendered frame must apply the final packet.
            queued.interactive_move_end(&0);
            assert!(queued.pending_move.is_none());
            Op::CompleteAnimations.apply(&mut queued);
            queued.verify_invariants();

            let mut direct = desktop(mode, floating);
            let output = begin(&mut direct);
            // The first packet enters the Moving phase synchronously. Compare
            // identical input histories: collapsing that semantic first packet
            // into a single large delta changes its initial pointer anchor.
            for n in 1..=100 {
                assert!(direct.interactive_move_update(
                    &0,
                    (4., 0.5).into(),
                    output.clone(),
                    (100. + 4. * n as f64, 100. + 0.5 * n as f64).into()
                ));
            }
            direct.interactive_move_end(&0);
            Op::CompleteAnimations.apply(&mut direct);
            direct.verify_invariants();
            assert_eq!(
                queued.session_layout().windows[0].position,
                direct.session_layout().windows[0].position
            );
        }
    }
}

#[test]
fn cancel_and_destroy_discard_pending_motion_without_resurrection() {
    for destroy in [false, true] {
        let mut layout = desktop(clarion_ipc::TilingMode::Scrolling, true);
        let output = begin(&mut layout);
        layout.queue_interactive_move_update(&0, (400., 50.).into(), output, (500., 150.).into());
        if destroy {
            assert!(layout.remove_window(&0, Transaction::new()).is_some());
        } else {
            layout.interactive_move_cancel(&0);
        }
        layout.flush_pending_move();
        assert!(layout.pending_move.is_none());
        assert!(layout.interactive_move.is_none());
        Op::CompleteAnimations.apply(&mut layout);
        layout.verify_invariants();
        assert_eq!(layout.session_layout().windows.len(), usize::from(!destroy));
    }
}

#[test]
fn output_removal_flushes_motion_before_detaching_the_destination() {
    let mut layout = desktop(clarion_ipc::TilingMode::Dwindle, true);
    let output = begin(&mut layout);
    layout.queue_interactive_move_update(
        &0,
        (400., 50.).into(),
        output.clone(),
        (500., 150.).into(),
    );
    layout.remove_output(&output);
    assert!(layout.pending_move.is_none());
    layout.interactive_move_end(&0);
    layout.verify_invariants();
    assert_eq!(layout.session_layout().windows.len(), 1);
}
