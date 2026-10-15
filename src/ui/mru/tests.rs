use proptest::prelude::*;
use proptest_derive::Arbitrary;

use super::*;

fn create_thumbnail() -> Thumbnail {
    Thumbnail {
        id: MappedId::next(),
        timestamp: None,
        on_current_output: false,
        on_current_workspace: false,
        app_id: None,
        size: Size::new(100, 100),
        clock: Clock::with_time(Duration::ZERO),
        config: realand_config::MruPreviews::default(),
        open_animation: None,
        move_animation: None,
        title_texture: Default::default(),
        background: RefCell::new(FocusRing::new(Default::default())),
        border: RefCell::new(FocusRing::new(Default::default())),
    }
}

fn create_output(name: &str) -> Output {
    use smithay::output::{Mode, PhysicalProperties, Subpixel};

    let output = Output::new(
        name.to_owned(),
        PhysicalProperties {
            size: Size::new(0, 0),
            subpixel: Subpixel::Unknown,
            make: String::new(),
            model: String::new(),
            serial_number: String::new(),
        },
    );
    output.change_current_state(
        Some(Mode {
            size: Size::new(1920, 1080),
            refresh: 60000,
        }),
        None,
        None,
        None,
    );
    output
}

fn create_mru(clock: &Clock) -> WindowMru {
    let mut thumbnails = vec![create_thumbnail(), create_thumbnail()];
    for thumbnail in &mut thumbnails {
        thumbnail.clock = clock.clone();
    }
    WindowMru {
        current_id: Some(thumbnails[0].id),
        thumbnails,
        scope: MruScope::All,
        app_id_filter: None,
    }
}

fn create_ui() -> WindowMruUi {
    let mut config = Config::default();
    config.recent_windows.open_delay_ms = 200;
    WindowMruUi::new(Rc::new(RefCell::new(config)))
}

#[test]
fn quick_switch_keeps_initial_delay_and_selection() {
    let mut clock = Clock::with_time(Duration::ZERO);
    let mut ui = create_ui();
    let mru = create_mru(&clock);
    let next_id = mru.thumbnails[1].id;
    ui.open(clock.clone(), mru, create_output("test"));
    ui.advance(MruDirection::Forward, None);
    clock.set_unadjusted(Duration::from_millis(100));

    let UiState::Open(inner) = &ui.state else {
        panic!("MRU must accept keyboard input during the initial delay");
    };
    assert!(!inner.is_fully_open());
    assert!(inner.reopen_animation.is_none());
    assert_eq!(ui.close(MruCloseRequest::Confirm), Some(next_id));
    assert!(matches!(ui.state, UiState::Closed { .. }));
    assert!(!ui.are_animations_ongoing());
}

#[test]
fn reopening_visible_mru_preserves_fade_and_uses_fresh_selection() {
    for rate in [0.25, 0.73, 1., 2.] {
        let mut clock = Clock::with_time(Duration::ZERO);
        clock.set_rate(rate);
        let mut ui = create_ui();
        let output = create_output("test");
        ui.open(clock.clone(), create_mru(&clock), output.clone());
        clock.set_unadjusted(Duration::from_millis(200));
        ui.close(MruCloseRequest::Cancel);
        clock.set_unadjusted(Duration::from_millis(240));

        let UiState::Closing { anim, .. } = &ui.state else {
            panic!("visible MRU must fade when closed");
        };
        let (alpha, velocity) = animation_alpha_and_velocity(anim);
        assert!(alpha > 0. && alpha < 1.);
        assert!(velocity < 0.);

        let mru = create_mru(&clock);
        let first_id = mru.thumbnails[0].id;
        let next_id = mru.thumbnails[1].id;
        ui.open(clock.clone(), mru, output);
        let UiState::Open(inner) = &ui.state else {
            panic!("reopened MRU must accept keyboard input immediately");
        };
        assert!(
            inner.is_fully_open(),
            "reopening must not blank the overlay"
        );
        let (new_alpha, new_velocity) = inner.open_alpha_and_velocity();
        assert!((alpha - new_alpha).abs() < 1e-10);
        assert!((velocity - new_velocity).abs() < 1e-10);
        assert!(ui.are_animations_ongoing());
        assert_eq!(ui.current_window_id(), Some(first_id));
        ui.advance(MruDirection::Forward, None);
        assert_eq!(ui.current_window_id(), Some(next_id));

        // Reversing again during the resumption must not jump back to full alpha.
        clock.set_unadjusted(Duration::from_millis(241));
        let UiState::Open(inner) = &ui.state else {
            unreachable!();
        };
        let (alpha, velocity) = inner.open_alpha_and_velocity();
        assert_eq!(ui.close(MruCloseRequest::Confirm), Some(next_id));
        let UiState::Closing { anim, .. } = &ui.state else {
            panic!("resumed MRU must fade out when closed again");
        };
        let (new_alpha, new_velocity) = animation_alpha_and_velocity(anim);
        assert!((alpha - new_alpha).abs() < 1e-10);
        assert!((velocity - new_velocity).abs() < 1e-10);
    }
}

#[test]
fn reopened_mru_finishes_at_full_opacity() {
    let mut clock = Clock::with_time(Duration::ZERO);
    let mut ui = create_ui();
    let output = create_output("test");
    ui.open(clock.clone(), create_mru(&clock), output.clone());
    clock.set_unadjusted(Duration::from_millis(200));
    ui.close(MruCloseRequest::Cancel);
    clock.set_unadjusted(Duration::from_millis(240));
    ui.open(clock.clone(), create_mru(&clock), output);
    clock.set_unadjusted(Duration::from_secs(10));
    ui.advance_animations();

    let UiState::Open(inner) = &ui.state else {
        panic!("reopened MRU must remain open");
    };
    assert!(inner.reopen_animation.is_none());
    assert_eq!(inner.open_alpha_and_velocity(), (1., 0.));
    assert!(!ui.are_animations_ongoing());
}

#[test]
fn reopening_keeps_configured_fade_duration() {
    let mut clock = Clock::with_time(Duration::ZERO);
    let mut ui = create_ui();
    let output = create_output("test");
    ui.open(clock.clone(), create_mru(&clock), output.clone());
    clock.set_unadjusted(Duration::from_millis(200));
    ui.close(MruCloseRequest::Cancel);

    let UiState::Closing { anim, .. } = &mut ui.state else {
        unreachable!();
    };
    // Fixed-duration user curves retain their full duration when interrupted.
    *anim = Animation::ease(
        clock.clone(),
        1.,
        0.,
        0.,
        320,
        crate::animation::Curve::EaseOutCubic,
    );
    clock.set_unadjusted(Duration::from_millis(240));
    let previous_alpha = anim.clamped_value();
    ui.open(clock.clone(), create_mru(&clock), output);
    let UiState::Open(inner) = &ui.state else {
        unreachable!();
    };
    let anim = inner.reopen_animation.as_ref().unwrap();
    assert_eq!(anim.duration(), Duration::from_millis(320));
    assert!((anim.value() - previous_alpha).abs() < 1e-10);
    clock.set_unadjusted(Duration::from_millis(559));
    assert!(!anim.is_done());
    clock.set_unadjusted(Duration::from_millis(560));
    assert!(anim.is_done());
}

#[test]
fn expired_fade_or_new_output_keeps_initial_delay() {
    for new_output in [false, true] {
        let mut clock = Clock::with_time(Duration::ZERO);
        let mut ui = create_ui();
        let output = create_output("test");
        ui.open(clock.clone(), create_mru(&clock), output.clone());
        clock.set_unadjusted(Duration::from_millis(200));
        ui.close(MruCloseRequest::Cancel);
        let output = if new_output {
            clock.set_unadjusted(Duration::from_millis(240));
            create_output("other")
        } else {
            // Deliberately don't advance animations before reopening: a completed
            // but not yet pruned closing animation must still count as closed.
            clock.set_unadjusted(Duration::from_secs(10));
            output
        };
        ui.open(clock.clone(), create_mru(&clock), output);
        let UiState::Open(inner) = &ui.state else {
            unreachable!();
        };
        assert!(!inner.is_fully_open());
        assert!(inner.reopen_animation.is_none());
        assert_eq!(
            inner.open_at,
            clock.now_unadjusted() + Duration::from_millis(200)
        );
    }
}

#[test]
fn thumbnail_retarget_preserves_position_and_velocity() {
    for rate in [0.25, 0.73, 1., 2.] {
        for zero_distance in [false, true] {
            let mut clock = Clock::with_time(Duration::ZERO);
            clock.set_rate(rate);
            let mut thumbnail = create_thumbnail();
            thumbnail.clock = clock.clone();
            let config = realand_config::animations::WindowMovementAnim::default().0;
            thumbnail.animate_move_from_with_config(200., config);
            clock.set_unadjusted(Duration::from_millis(80));
            let before = thumbnail.render_offset();
            let previous = thumbnail.move_animation.as_ref().unwrap();
            let velocity = previous.from * previous.anim.velocity();
            assert!(velocity < 0.);

            let shift = if zero_distance { -before } else { -150. };
            thumbnail.animate_move_from_with_config(shift, config);
            assert!((thumbnail.render_offset() - (before + shift)).abs() < 1e-10);
            let current = thumbnail.move_animation.as_ref().unwrap();
            assert!((current.from * current.anim.velocity() - velocity).abs() < 1e-10);

            clock.set_unadjusted(Duration::from_nanos(80_000_100));
            let actual_velocity = (thumbnail.render_offset() - (before + shift)) / 0.0000001;
            assert!(
                (actual_velocity - velocity).abs() < 0.1,
                "rate={rate}, zero_distance={zero_distance}, expected={velocity}, actual={actual_velocity}"
            );
            clock.set_unadjusted(Duration::from_secs(10));
            thumbnail.advance_animations();
            assert_eq!(thumbnail.render_offset(), 0.);
            assert!(!thumbnail.are_animations_ongoing());
        }
    }
}

#[test]
fn unchanged_thumbnail_layout_stays_idle() {
    let mut thumbnail = create_thumbnail();
    let config = realand_config::animations::WindowOpenAnim::default().anim;
    thumbnail.animate_move_from_with_config(0., config);
    assert!(!thumbnail.are_animations_ongoing());
}

#[test]
fn unchanged_thumbnail_layout_keeps_current_animation_deadline() {
    let mut clock = Clock::with_time(Duration::ZERO);
    let mut thumbnail = create_thumbnail();
    thumbnail.clock = clock.clone();
    let config = realand_config::animations::WindowOpenAnim::default().anim;
    thumbnail.animate_move_from_with_config(200., config);
    let end = thumbnail.move_animation.as_ref().unwrap().anim.end_time();
    clock.set_unadjusted(Duration::from_millis(80));
    let offset = thumbnail.render_offset();
    thumbnail.animate_move_from_with_config(0., config);
    let anim = &thumbnail.move_animation.as_ref().unwrap().anim;
    assert_eq!(anim.end_time(), end);
    assert_eq!(thumbnail.render_offset(), offset);
}

fn bouncy_fade_config() -> realand_config::Animation {
    realand_config::Animation {
        off: false,
        kind: realand_config::animations::Kind::Spring(realand_config::animations::SpringParams {
            damping_ratio: 0.5,
            stiffness: 200,
            epsilon: 0.0001,
        }),
    }
}

#[test]
fn opaque_thumbnail_stops_requesting_frames() {
    let mut clock = Clock::with_time(Duration::ZERO);
    let mut thumbnail = create_thumbnail();
    thumbnail.clock = clock.clone();
    thumbnail.animate_open_with_config(bouncy_fade_config());
    clock.set_unadjusted(Duration::from_millis(250));
    let anim = thumbnail.open_animation.as_ref().unwrap();
    assert_eq!(anim.clamped_value(), 1.);
    assert!(!anim.is_done(), "the spring is still settling invisibly");
    thumbnail.advance_animations();
    assert!(!thumbnail.are_animations_ongoing());
}

#[test]
fn invisible_mru_releases_closing_state_and_stops_requesting_frames() {
    let mut clock = Clock::with_time(Duration::ZERO);
    let mut ui = create_ui();
    ui.config.borrow_mut().animations.recent_windows_close.0 = bouncy_fade_config();
    ui.open(clock.clone(), create_mru(&clock), create_output("test"));
    clock.set_unadjusted(Duration::from_millis(200));
    ui.close(MruCloseRequest::Cancel);
    clock.set_unadjusted(Duration::from_millis(450));
    let UiState::Closing { anim, .. } = &ui.state else {
        unreachable!();
    };
    assert_eq!(anim.clamped_value(), 0.);
    assert!(!anim.is_done(), "the spring is still settling invisibly");
    ui.advance_animations();
    assert!(matches!(ui.state, UiState::Closed { .. }));
    assert!(!ui.are_animations_ongoing());
}

#[test]
fn remove_last_window_out_of_two() {
    let ops = [Op::Backward, Op::Remove(1)];

    let thumbnails = vec![create_thumbnail(), create_thumbnail()];
    let current_id = thumbnails.first().map(|t| t.id);
    let mut mru = WindowMru {
        thumbnails,
        current_id,
        scope: MruScope::All,
        app_id_filter: None,
    };

    check_ops(&mut mru, &ops);
}

fn arbitrary_scope() -> impl Strategy<Value = MruScope> {
    prop_oneof![
        Just(MruScope::All),
        Just(MruScope::Output),
        Just(MruScope::Workspace),
    ]
}

fn arbitrary_filter() -> impl Strategy<Value = MruFilter> {
    prop_oneof![Just(MruFilter::All), Just(MruFilter::AppId)]
}

fn arbitrary_app_id() -> impl Strategy<Value = Option<String>> {
    prop_oneof![Just(None), Just(Some(1)), Just(Some(2))]
        .prop_map(|id| id.map(|id| format!("app-{id}")))
}

prop_compose! {
    fn arbitrary_thumbnail()(
        timestamp: Option<Duration>,
        on_current_output: bool,
        on_current_workspace: bool,
        app_id in arbitrary_app_id(),
    ) -> Thumbnail {
        let mut thumbnail = create_thumbnail();
        thumbnail.timestamp = timestamp;
        thumbnail.on_current_workspace = on_current_workspace;
        thumbnail.on_current_output = on_current_output;
        thumbnail.app_id = app_id;
        thumbnail
    }
}

prop_compose! {
    fn arbitrary_mru()(
        thumbnails in proptest::collection::vec(arbitrary_thumbnail(), 1..10),
    ) -> WindowMru {
        let current_id = thumbnails.first().map(|t| t.id);
        WindowMru {
            thumbnails,
            current_id,
            scope: MruScope::All,
            app_id_filter: None,
        }
    }
}

#[derive(Debug, Clone, Arbitrary)]
enum Op {
    Forward,
    Backward,
    First,
    Last,
    SetScope(#[proptest(strategy = "arbitrary_scope()")] MruScope),
    SetFilter(#[proptest(strategy = "arbitrary_filter()")] MruFilter),
    Remove(#[proptest(strategy = "1..10usize")] usize),
}

impl Op {
    fn apply(&self, mru: &mut WindowMru) {
        match self {
            Op::Forward => mru.forward(),
            Op::Backward => mru.backward(),
            Op::First => mru.first(),
            Op::Last => mru.last(),
            Op::SetScope(scope) => {
                mru.set_scope(*scope);
            }
            Op::SetFilter(filter) => {
                mru.set_filter(*filter);
            }
            Op::Remove(idx) => {
                if *idx < mru.thumbnails.len() {
                    mru.remove_by_idx(*idx);
                }
            }
        }
    }
}

fn check_ops(mru: &mut WindowMru, ops: &[Op]) {
    for op in ops {
        op.apply(mru);
        mru.verify_invariants();
    }
}

proptest! {
    #[test]
    fn random_operations_dont_panic(
        mut mru in arbitrary_mru(),
        ops: Vec<Op>,
    ) {
        check_ops(&mut mru, &ops);
    }
}
