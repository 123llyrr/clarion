use super::*;

fn stage_layout(scale: f64, zoom: f64) -> Layout<TestWindow> {
    stage_layout_with_privacy(scale, zoom, false)
}

fn stage_layout_with_privacy(scale: f64, zoom: f64, block_capture: bool) -> Layout<TestWindow> {
    let mut options = Options::default();
    options.overview.zoom = zoom;
    options.animations.overview_open_close.0.kind =
        realand_config::animations::Kind::Easing(realand_config::animations::EasingParams {
            duration_ms: 1000,
            curve: realand_config::animations::Curve::Linear,
        });
    let mut ops = vec![Op::AddScaledOutput {
        id: 1,
        scale,
        layout_config: None,
    }];
    for id in (1..=9).rev() {
        ops.push(Op::AddNamedWorkspace {
            ws_name: id,
            output_name: Some(1),
            layout_config: None,
        });
        let mut params = TestWindowParams::new(id);
        if block_capture {
            let rules = ResolvedWindowRules {
                block_out_from: Some(realand_config::BlockOutFrom::ScreenCapture),
                ..Default::default()
            };
            params.rules = Some(rules);
        }
        ops.push(Op::AddWindowToNamedWorkspace {
            params,
            ws_name: id,
        });
    }
    ops.extend([Op::FocusWorkspace(4), Op::CompleteAnimations]);
    let mut layout = check_ops_with_options(options, ops);
    layout.toggle_overview();
    assert!(layout.is_stage_open());
    layout
}

fn stage_rects(layout: &Layout<TestWindow>) -> Vec<Rectangle<f64, Logical>> {
    let mon = layout.active_monitor_ref().unwrap();
    mon.workspaces_render_geo()
        .take(mon.workspaces.len())
        .collect()
}

#[test]
fn stage_reserved_bar_keeps_cards_and_entire_workspace_rail_in_the_working_area() {
    use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::{
        zwlr_layer_shell_v1::Layer, zwlr_layer_surface_v1::Anchor,
    };

    for scale in [1., 1.25, 1.5, 2.] {
        for anchor in [
            Anchor::Top | Anchor::Left | Anchor::Right,
            Anchor::Bottom | Anchor::Left | Anchor::Right,
            Anchor::Left | Anchor::Top | Anchor::Bottom,
            Anchor::Right | Anchor::Top | Anchor::Bottom,
        ] {
            let config = realand_config::Config {
                workspaces: ["first", "second", "third"]
                    .into_iter()
                    .map(|name| realand_config::Workspace {
                        name: WorkspaceName(name.into()),
                        open_on_output: None,
                        layout: None,
                    })
                    .collect(),
                ..Default::default()
            };
            let mut f = crate::tests::Fixture::with_config(config);
            f.add_output(
                1,
                if scale == 2. {
                    (3840, 2400)
                } else {
                    (1280, 800)
                },
            );
            let output = f.realand_output(1);
            output.change_current_state(
                None,
                None,
                Some(smithay::output::Scale::Fractional(scale)),
                None,
            );
            f.realand().output_resized(&output);
            let client = f.add_client();
            let panel = f.client(client).create_layer(None, Layer::Top, "stage-bar");
            let surface = panel.surface.clone();
            panel.layer_surface.set_anchor(anchor);
            if anchor.contains(Anchor::Left | Anchor::Right) {
                panel.layer_surface.set_size(0, 40);
            } else {
                panel.layer_surface.set_size(40, 0);
            }
            panel.layer_surface.set_exclusive_zone(40);
            panel.commit();
            f.double_roundtrip(client);
            let panel = f.client(client).layer(&surface);
            panel.attach_new_buffer();
            let (w, h) = panel.configures_received.last().unwrap().1.size;
            panel.set_size(w as u16, h as u16);
            panel.ack_last_and_commit();
            f.double_roundtrip(client);
            f.realand().layout.open_overview();
            f.realand_complete_animations();

            let mon = f.realand().layout.monitor_for_output(&output).unwrap();
            let area = mon.working_area();
            let view = mon.view_size();
            let selected = mon.idx_of_ws(mon.stage_selected()).unwrap();
            let card = mon.workspaces_render_geo().nth(selected).unwrap();
            let tolerance = 1. / scale;
            approx::assert_abs_diff_eq!(
                card.loc.x + card.size.w / 2.,
                area.loc.x + area.size.w / 2.,
                epsilon = tolerance
            );
            approx::assert_abs_diff_eq!(
                card.loc.y + card.size.h / 2.,
                area.loc.y + (area.size.h - area.size.h.min(76.)) / 2.,
                epsilon = tolerance
            );
            approx::assert_abs_diff_eq!(card.size.w / card.size.h, view.w / view.h, epsilon = 1e-9);
            assert!(card.loc.y >= area.loc.y - tolerance);
            assert!(card.loc.y + card.size.h <= area.loc.y + area.size.h - 76. + tolerance);

            let rail_y = area.loc.y + area.size.h - 38.;
            let mut targets = Vec::new();
            for x in 0..view.w.ceil() as i32 {
                let point = Point::from((f64::from(x), rail_y));
                if let Some(target) = mon.stage_rail_under(point) {
                    assert!(area.contains(point));
                    assert_eq!(mon.stage_drop_target(point).unwrap().0, target);
                    if !targets.contains(&target) {
                        targets.push(target);
                    }
                }
            }
            for workspace in &mon.workspaces {
                assert!(
                    targets.contains(&InsertWorkspace::Existing(workspace.id())),
                    "entire rail must remain accessible: scale={scale}, anchor={anchor:?}"
                );
            }
            assert!(targets
                .iter()
                .any(|target| matches!(target, InsertWorkspace::NewAt(_))));
            for y in 0..view.h.ceil() as i32 {
                let point = Point::from((area.loc.x + area.size.w / 2., f64::from(y)));
                if !area.contains(point) {
                    assert!(
                        mon.stage_rail_under(point).is_none(),
                        "reserved bar must never contain a rail target: scale={scale}, anchor={anchor:?}, point={point:?}"
                    );
                }
            }
            f.realand().layout.verify_invariants();
        }
    }
}

#[test]
fn stage_bar_edge_changes_refresh_cached_geometry_without_resizing_the_output() {
    use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::{
        zwlr_layer_shell_v1::Layer, zwlr_layer_surface_v1::Anchor,
    };

    for scale in [1., 1.25, 1.5, 2.] {
        let mut f = crate::tests::Fixture::new();
        f.add_output(
            1,
            if scale == 2. {
                (3840, 2400)
            } else {
                (1280, 800)
            },
        );
        let output = f.realand_output(1);
        output.change_current_state(
            None,
            None,
            Some(smithay::output::Scale::Fractional(scale)),
            None,
        );
        f.realand().output_resized(&output);
        let client = f.add_client();
        let panel = f
            .client(client)
            .create_layer(None, Layer::Top, "moving-stage-bar");
        let surface = panel.surface.clone();
        panel
            .layer_surface
            .set_anchor(Anchor::Top | Anchor::Left | Anchor::Right);
        panel.layer_surface.set_size(0, 40);
        panel.layer_surface.set_exclusive_zone(40);
        panel.commit();
        f.double_roundtrip(client);
        let panel = f.client(client).layer(&surface);
        panel.attach_new_buffer();
        let (w, h) = panel.configures_received.last().unwrap().1.size;
        panel.set_size(w as u16, h as u16);
        panel.ack_last_and_commit();
        f.double_roundtrip(client);
        f.realand().layout.open_overview();
        f.realand_complete_animations();

        let mon = f.realand().layout.monitor_for_output(&output).unwrap();
        let selected = mon.idx_of_ws(mon.stage_selected()).unwrap();
        let top_card = mon.workspaces_render_geo().nth(selected).unwrap();
        let top_area = mon.working_area();
        assert_eq!(top_area.loc.y, 40.);
        let original_view = mon.view_size();

        let panel = f.client(client).layer(&surface);
        panel
            .layer_surface
            .set_anchor(Anchor::Bottom | Anchor::Left | Anchor::Right);
        panel.attach_new_buffer();
        panel.commit();
        f.double_roundtrip(client);
        let mon = f.realand().layout.monitor_for_output(&output).unwrap();
        let bottom_area = mon.working_area();
        assert_eq!(bottom_area.loc.y, 0.);
        assert_eq!(bottom_area.size, top_area.size);
        assert_eq!(mon.view_size(), original_view);
        let bottom_card = mon.workspaces_render_geo().nth(selected).unwrap();
        assert_eq!(bottom_card.size, top_card.size);
        approx::assert_abs_diff_eq!(
            top_card.loc.y - bottom_card.loc.y,
            40.,
            epsilon = 1. / scale
        );
        let rail_y = bottom_area.loc.y + bottom_area.size.h - 38.;
        assert!((0..original_view.w.ceil() as i32).any(|x| {
            mon.stage_rail_under(Point::from((f64::from(x), rail_y)))
                .is_some()
        }));
        f.realand().layout.verify_invariants();
    }
}

#[test]
fn stage_tiny_or_exhausted_working_area_has_no_invisible_rail_targets() {
    use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::{
        zwlr_layer_shell_v1::Layer, zwlr_layer_surface_v1::Anchor,
    };

    for scale in [1., 1.25, 1.5, 2.] {
        for (anchor, zone) in [
            (Anchor::Bottom | Anchor::Left | Anchor::Right, 80),
            (Anchor::Top | Anchor::Left | Anchor::Right, 120),
            (Anchor::Left | Anchor::Top | Anchor::Bottom, 1400),
        ] {
            let mut f = crate::tests::Fixture::new();
            f.add_output(1, (1280, 100));
            let output = f.realand_output(1);
            output.change_current_state(
                None,
                None,
                Some(smithay::output::Scale::Fractional(scale)),
                None,
            );
            f.realand().output_resized(&output);
            let client = f.add_client();
            let panel = f
                .client(client)
                .create_layer(None, Layer::Top, "tiny-stage-bar");
            let surface = panel.surface.clone();
            panel.layer_surface.set_anchor(anchor);
            if anchor.contains(Anchor::Left | Anchor::Right) {
                panel.layer_surface.set_size(0, 40);
            } else {
                panel.layer_surface.set_size(40, 0);
            }
            panel.layer_surface.set_exclusive_zone(zone);
            panel.commit();
            f.double_roundtrip(client);
            let panel = f.client(client).layer(&surface);
            panel.attach_new_buffer();
            let (w, h) = panel.configures_received.last().unwrap().1.size;
            panel.set_size(w as u16, h as u16);
            panel.ack_last_and_commit();
            f.double_roundtrip(client);
            f.realand().layout.open_overview();
            f.realand_complete_animations();

            let mon = f.realand().layout.monitor_for_output(&output).unwrap();
            let view = mon.view_size();
            let exhausted = mon
                .working_area()
                .intersection(Rectangle::from_size(view))
                .is_none();
            for geo in mon.workspaces_render_geo() {
                assert!([geo.loc.x, geo.loc.y, geo.size.w, geo.size.h]
                    .into_iter()
                    .all(f64::is_finite));
            }
            for y in 0..view.h.ceil() as i32 {
                for x in (0..view.w.ceil() as i32).step_by(16) {
                    let point = Point::from((f64::from(x), f64::from(y)));
                    assert!(mon.stage_rail_under(point).is_none());
                    assert!(mon.stage_rail_navigation(point).is_none());
                    if exhausted {
                        assert!(mon.stage_drop_target(point).is_none());
                    }
                }
            }
            f.realand().layout.verify_invariants();
        }
    }
}

#[test]
fn stage_spring_reversal_preserves_card_velocity_instead_of_freezing() {
    for scale in [1., 1.25, 1.5, 2.] {
        for closing in [false, true] {
            let mut layout = stage_layout(scale, 0.5);
            let config = realand_config::Animations::default().overview_open_close.0;
            let mut options = Options::clone(&layout.options);
            options.animations.overview_open_close.0 = config;
            layout.update_options(options);
            if let Some(OverviewProgress::Animation(animation)) = &mut layout.overview_progress {
                animation.replace_config(config);
            }
            layout.set_monitors_overview_state();
            if closing {
                Op::CompleteAnimations.apply(&mut layout);
                layout.toggle_overview();
            }
            let start = layout.clock.now_unadjusted();
            let active = layout.active_monitor_ref().unwrap().active_workspace_idx();
            layout
                .clock
                .set_unadjusted(start + Duration::from_millis(79));
            layout.advance_animations();
            let before = stage_rects(&layout)[active].size.w;
            layout
                .clock
                .set_unadjusted(start + Duration::from_millis(80));
            layout.advance_animations();
            let at = stage_rects(&layout)[active].size.w;
            layout.toggle_overview();
            assert_eq!(stage_rects(&layout)[active].size.w, at);
            layout
                .clock
                .set_unadjusted(start + Duration::from_millis(81));
            layout.advance_animations();
            let after = stage_rects(&layout)[active].size.w;
            let velocity_ratio = (after - at) / (at - before);
            assert!(
                (0.75..1.25).contains(&velocity_ratio),
                "reversing must retain momentum: scale={scale}, closing={closing}, velocity ratio={velocity_ratio}"
            );
            Op::CompleteAnimations.apply(&mut layout);
            layout.verify_invariants();
        }
    }
}

#[test]
fn stage_spring_preview_retargeting_preserves_card_momentum() {
    for scale in [1., 1.25, 1.5, 2.] {
        for rate in [0.5, 1., 2.] {
            let mut layout = stage_layout(scale, 0.5);
            let mut options = Options::clone(&layout.options);
            options.animations.overview_open_close =
                realand_config::Animations::default().overview_open_close;
            layout.update_options(options);
            Op::CompleteAnimations.apply(&mut layout);
            layout.clock.set_rate(rate);
            let time = |millis: u64| Duration::from_secs_f64(millis as f64 / 1000. / rate);
            let active = layout.active_monitor_ref().unwrap().active_workspace_idx();
            assert!(layout.stage_navigate(None, 1));
            for (millis, direction) in [(80, -1), (120, 1), (160, 1)] {
                layout.clock.set_unadjusted(time(millis - 1));
                layout.advance_animations();
                let before = stage_rects(&layout)[active].size.w;
                layout.clock.set_unadjusted(time(millis));
                layout.advance_animations();
                let at = stage_rects(&layout)[active].size.w;
                assert!(layout.stage_navigate(None, direction));
                assert_eq!(stage_rects(&layout)[active].size.w, at);
                layout.clock.set_unadjusted(time(millis + 1));
                layout.advance_animations();
                let after = stage_rects(&layout)[active].size.w;
                let velocity_ratio = (after - at) / (at - before);
                assert!(
                    (0.75..1.25).contains(&velocity_ratio),
                    "scale={scale}, rate={rate}, time={millis}, velocity ratio={velocity_ratio}"
                );
            }
            assert_eq!(
                layout.active_monitor_ref().unwrap().active_workspace_idx(),
                active
            );
            Op::CompleteAnimations.apply(&mut layout);
            assert!(!layout.are_animations_ongoing(None));
            layout.verify_invariants();
        }
    }
}

#[test]
fn stage_spring_confirm_and_cancel_preserve_preview_velocity() {
    for scale in [1., 1.25, 1.5, 2.] {
        for rate in [0.5, 1., 2.] {
            for confirm in [false, true] {
                let mut layout = stage_layout(scale, 0.5);
                let mut options = Options::clone(&layout.options);
                options.animations.overview_open_close =
                    realand_config::Animations::default().overview_open_close;
                layout.update_options(options);
                Op::CompleteAnimations.apply(&mut layout);
                layout.clock.set_rate(rate);
                let time = |micros: u64| Duration::from_secs_f64(micros as f64 / 1_000_000. / rate);
                let active = layout.active_monitor_ref().unwrap().active_workspace_idx();
                assert!(layout.stage_navigate(None, 1));
                layout.clock.set_unadjusted(time(79_000));
                layout.advance_animations();
                let before = stage_rects(&layout)[active].size.w;
                layout.clock.set_unadjusted(time(79_999));
                layout.advance_animations();
                let fine_before = stage_rects(&layout)[active].size.w;
                layout.clock.set_unadjusted(time(80_000));
                layout.advance_animations();
                let at = stage_rects(&layout)[active].size.w;
                if confirm {
                    assert!(layout.confirm_stage());
                } else {
                    assert!(layout.cancel_stage());
                }
                assert_eq!(stage_rects(&layout)[active].size.w, at);
                layout.clock.set_unadjusted(time(80_001));
                layout.advance_animations();
                let fine_after = stage_rects(&layout)[active].size.w;
                let launch_ratio = (fine_after - at) / (at - fine_before);
                assert!(
                    (0.98..1.02).contains(&launch_ratio),
                    "scale={scale}, rate={rate}, confirm={confirm}, launch ratio={launch_ratio}"
                );
                layout.clock.set_unadjusted(time(81_000));
                layout.advance_animations();
                let after = stage_rects(&layout)[active].size.w;
                assert!(
                    (after - at) * (at - before) > 0.,
                    "card must retain direction across confirm/cancel"
                );

                // Reopen while the carried closing motion is still in flight.
                layout.clock.set_unadjusted(time(99_999));
                layout.advance_animations();
                let before_reopen = stage_rects(&layout)[active].size.w;
                layout.clock.set_unadjusted(time(100_000));
                layout.advance_animations();
                let at_reopen = stage_rects(&layout)[active].size.w;
                layout.toggle_overview();
                assert_eq!(stage_rects(&layout)[active].size.w, at_reopen);
                layout.clock.set_unadjusted(time(100_001));
                layout.advance_animations();
                let after_reopen = stage_rects(&layout)[active].size.w;
                let ratio = (after_reopen - at_reopen) / (at_reopen - before_reopen);
                assert!(
                    (0.98..1.02).contains(&ratio),
                    "scale={scale}, rate={rate}, confirm={confirm}, reopening ratio={ratio}"
                );
                layout.close_overview();
                for millis in 101..=1600 {
                    layout.clock.set_unadjusted(time(millis * 1000));
                    layout.advance_animations();
                }
                assert!(!layout.is_overview_visible());
                assert!(!layout.are_animations_ongoing(None));
                let mon = layout.active_monitor_ref().unwrap();
                assert_eq!(
                    stage_rects(&layout)[mon.active_workspace_idx()],
                    Rectangle::from_size(mon.view_size())
                );
                layout.verify_invariants();
            }
        }
    }
}

#[test]
fn stage_spring_preview_momentum_stays_bounded_and_respects_disabled_motion() {
    for scale in [1., 1.25, 1.5, 2.] {
        for hz in [60, 144] {
            for off in [false, true] {
                let mut layout = stage_layout(scale, 0.5);
                let mut options = Options::clone(&layout.options);
                options.animations.overview_open_close =
                    realand_config::Animations::default().overview_open_close;
                options.animations.overview_open_close.0.off = off;
                layout.update_options(options);
                Op::CompleteAnimations.apply(&mut layout);
                let active = layout.active_monitor_ref().unwrap().active_workspace_idx();
                let initial = stage_rects(&layout);
                let tick = Duration::from_secs_f64(1. / f64::from(hz));
                for frame in 0..360 {
                    if frame < 40 && frame % 2 == 0 {
                        let before = stage_rects(&layout);
                        let direction = if frame % 8 < 4 { 1 } else { -1 };
                        assert!(layout.stage_navigate(None, direction));
                        if !off {
                            assert_eq!(stage_rects(&layout), before);
                        }
                    }
                    layout
                        .clock
                        .set_unadjusted(layout.clock.now_unadjusted() + tick);
                    layout.advance_animations();
                    let mon = layout.active_monitor_ref().unwrap();
                    assert_eq!(mon.active_workspace_idx(), active);
                    for rect in stage_rects(&layout) {
                        assert!(rect.loc.x.is_finite() && rect.loc.y.is_finite());
                        assert!((0. ..=mon.view_size().w).contains(&rect.size.w));
                        assert!((0. ..=mon.view_size().h).contains(&rect.size.h));
                        if rect.size.w > 0.001 {
                            approx::assert_abs_diff_eq!(
                                rect.size.w / rect.size.h,
                                mon.view_size().w / mon.view_size().h,
                                epsilon = 1e-7
                            );
                        }
                    }
                    if off {
                        assert!(!layout.are_animations_ongoing(None));
                    }
                }
                assert!(!layout.are_animations_ongoing(None));
                assert_eq!(stage_rects(&layout), initial);
                layout.verify_invariants();
            }
        }
    }
}

#[test]
fn stage_hot_corner_rail_clicks_preview_confirm_and_enter_empty_desktops_at_two_times_scale() {
    use smithay::reexports::wayland_protocols_wlr::virtual_pointer::v1::client::{
        zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
    };
    use wayland_client::protocol::wl_pointer::ButtonState;
    for click_new in [false, true] {
        let mut config = realand_config::Config::default();
        config.gestures.hot_corners.top_left = true;
        config.hotkey_overlay.skip_at_startup = true;
        let mut f = crate::tests::Fixture::with_config(config);
        f.add_output(1, (3840, 2400));
        let output = f.realand_output(1);
        output.change_current_state(None, None, Some(smithay::output::Scale::Integer(2)), None);
        f.realand().output_resized(&output);
        let client = f.add_client();
        let window = f.client(client).create_window();
        let surface = window.surface.clone();
        window.commit();
        f.double_roundtrip(client);
        let window = f.client(client).window(&surface);
        window.attach_new_buffer();
        window.set_size(500, 400);
        window.ack_last_and_commit();
        f.double_roundtrip(client);
        f.realand_complete_animations();
        let initial = f.realand().layout.active_workspace().unwrap().id();
        let c = f.client(client);
        let global = c
            .state
            .globals
            .iter()
            .find(|global| global.interface == "zwlr_virtual_pointer_manager_v1")
            .unwrap();
        let registry = c.display.get_registry(&c.qh, ());
        let manager: ZwlrVirtualPointerManagerV1 = registry.bind(global.name, 1, &c.qh, ());
        let pointer = manager.create_virtual_pointer(None, &c.qh, ());
        pointer.motion_absolute(1, 4, 4, 1920, 1200);
        pointer.frame();
        f.double_roundtrip(client);
        assert!(f.realand().layout.is_stage_open());
        f.realand_complete_animations();
        let mon = f.realand().layout.monitor_for_output(&output).unwrap();
        let target = mon.workspaces.last().unwrap().id();
        let preview_point = (0..1920)
            .find_map(|x| {
                let p = Point::from((x as f64, 1160.));
                (mon.stage_rail_under(p) == Some(InsertWorkspace::Existing(target))).then_some(p)
            })
            .unwrap();
        let point = (0..1920)
            .find_map(|x| {
                let p = Point::from((x as f64, 1160.));
                let hit = mon.stage_rail_under(p);
                (if click_new {
                    matches!(hit, Some(InsertWorkspace::NewAt(_)))
                } else {
                    hit == Some(InsertWorkspace::Existing(target))
                })
                .then_some(p)
            })
            .unwrap();
        assert_ne!(initial, target);
        pointer.motion_absolute(
            2,
            preview_point.x as u32 + 8,
            preview_point.y as u32,
            1920,
            1200,
        );
        pointer.frame();
        f.double_roundtrip(client);
        pointer.button(3, 0x110, ButtonState::Pressed);
        pointer.frame();
        pointer.button(4, 0x110, ButtonState::Released);
        pointer.frame();
        f.double_roundtrip(client);
        assert!(f.realand().layout.is_stage_open(), "first click previews");
        assert_eq!(f.realand().layout.active_workspace().unwrap().id(), initial);
        assert_eq!(
            f.realand().layout.stage_selected_for_output(&output),
            Some(target)
        );
        // Retarget a running preview using the same real input path, with no final motion.
        pointer.motion_absolute(5, point.x as u32 + 8, point.y as u32, 1920, 1200);
        pointer.frame();
        f.double_roundtrip(client);
        pointer.button(6, 0x110, ButtonState::Pressed);
        pointer.frame();
        pointer.button(7, 0x110, ButtonState::Released);
        pointer.frame();
        f.double_roundtrip(client);
        assert!(
            !f.realand().layout.is_overview_open(),
            "selected workspace and New workspace must execute their click action"
        );
        f.realand_complete_animations();
        assert_eq!(f.realand().layout.active_workspace().unwrap().id(), target);
        assert!(f.realand().layout.active_window().is_none());
    }
}

#[test]
fn stage_close_with_spring_and_cube_has_no_second_workspace_animation() {
    for distance in [1, 4] {
        for close_at in [80, 1200] {
            let mut layout = stage_layout(2., 0.5);
            let mut options = Options::clone(&layout.options);
            options.animations.overview_open_close =
                realand_config::Animations::default().overview_open_close;
            options.animations.workspace_switch.1.off = false;
            layout.update_options(options);
            layout.clock.set_unadjusted(Duration::from_millis(close_at));
            layout.advance_animations();
            assert!(layout.stage_navigate(None, distance));
            assert!(layout.confirm_stage());
            for tick in 1..=500 {
                layout
                    .clock
                    .set_unadjusted(Duration::from_millis(close_at + tick * 8));
                layout.advance_animations();
                if !layout.is_overview_visible() {
                    let mon = layout.active_monitor_ref().unwrap();
                    assert!(mon.workspace_switch.is_none(), "Stage close must finish at a settled desktop, distance={distance}, close_at={close_at}");
                    let geo = mon
                        .workspaces_render_geo()
                        .nth(mon.active_workspace_idx())
                        .unwrap();
                    assert_eq!(geo.loc, Point::from((0., 0.)));
                    assert_eq!(geo.size, mon.view_size());
                    break;
                }
                assert!(tick < 500, "Stage close must finish");
            }
        }
    }
}

#[test]
fn stage_opening_offscreen_cards_start_collapsed_instead_of_flying_from_the_strip() {
    let layout = stage_layout(2., 0.5);
    let mon = layout.active_monitor_ref().unwrap();
    for (idx, geo) in mon
        .workspaces_render_geo()
        .take(mon.workspaces.len())
        .enumerate()
    {
        if idx == mon.active_workspace_idx() {
            assert_eq!(geo, Rectangle::from_size(mon.view_size()));
        } else {
            assert_eq!(
                geo.size,
                Size::from((0., 0.)),
                "unseen workspace {idx} must enter as a card, not a full desktop"
            );
        }
    }
}

#[test]
fn stage_has_seven_live_cards_with_shared_aspect_scale_and_front_order() {
    for scale in [1., 1.25, 1.5] {
        let mut layout = stage_layout(scale, 0.5);
        Op::CompleteAnimations.apply(&mut layout);
        let mon = layout.active_monitor_ref().unwrap();
        let selected = mon.stage_selected();
        let selected_index = mon.idx_of_ws(selected).unwrap();
        let view = mon.view_size();
        let geometry = stage_rects(&layout);
        assert_eq!(geometry.iter().filter(|geo| geo.size.w > 0.).count(), 7);
        let ratio = [1., 0.75, 0.60, 0.48];
        for (idx, geo) in geometry.iter().enumerate() {
            let depth = idx.abs_diff(selected_index);
            if depth <= 3 {
                approx::assert_abs_diff_eq!(
                    geo.size.w,
                    view.w * 0.5 * ratio[depth],
                    epsilon = 1e-9
                );
                approx::assert_abs_diff_eq!(
                    geo.size.w / geo.size.h,
                    view.w / view.h,
                    epsilon = 1e-9
                );
                approx::assert_abs_diff_eq!(
                    mon.workspace_render_zoom(*geo),
                    0.5 * ratio[depth],
                    epsilon = 1e-9
                );
            }
        }
        let cards: Vec<_> = mon.workspaces_with_render_geo().collect();
        assert_eq!(cards[0].0.id(), selected);
        for pair in cards.windows(2) {
            let first = mon
                .idx_of_ws(pair[0].0.id())
                .unwrap()
                .abs_diff(selected_index);
            let second = mon
                .idx_of_ws(pair[1].0.id())
                .unwrap()
                .abs_diff(selected_index);
            assert!(first <= second);
        }
        // Each rear depth contributes an exposed top edge, rather than being
        // wholly hidden behind the nearest neighbor inside the output bounds.
        for direction in [-1isize, 1] {
            for depth in [2usize, 3] {
                let idx = selected_index.saturating_add_signed(direction * depth as isize);
                let geo = geometry[idx];
                let point = Point::from((
                    if direction < 0 {
                        geo.loc.x.max(0.) + 4.
                    } else {
                        (geo.loc.x + geo.size.w).min(view.w) - 4.
                    },
                    geo.loc.y + 4.,
                ));
                assert_eq!(
                    mon.workspace_under(point).unwrap().0.id(),
                    mon.workspaces[idx].id(),
                    "depth={depth} side={direction} must retain an exposed card edge"
                );
            }
        }
        // The selected card occludes its neighbors in every consumer.
        let center_geo = geometry[selected_index];
        let side_geo = geometry[selected_index + 1];
        let overlap = center_geo.intersection(side_geo).unwrap();
        let point = overlap.loc + overlap.size.to_point().downscale(2.);
        assert_eq!(mon.workspace_under(point).unwrap().0.id(), selected);
        assert_eq!(mon.workspace_under_narrow(point).unwrap().id(), selected);
        assert_eq!(
            mon.stage_drop_target(point).unwrap().0,
            InsertWorkspace::Existing(selected)
        );

        let ws = cards[0].0;
        let (tile, pos, visible) = ws.tiles_with_render_positions().next().unwrap();
        assert!(visible);
        let window_center = pos + tile.window_loc() + tile.window_size().to_point().downscale(2.);
        let point = center_geo.loc + window_center.upscale(mon.workspace_render_zoom(center_geo));
        assert_eq!(mon.window_under(point).unwrap().0.id(), tile.window().id());
        layout.verify_invariants();
    }
}

#[test]
fn stage_preview_confirmation_cancel_and_reversal_preserve_the_current_frame() {
    for scale in [1., 1.25, 1.5] {
        for millis in [100, 400, 800] {
            let mut layout = stage_layout(scale, 0.5);
            let initial = layout.active_workspace().unwrap().id();
            layout.clock.set_unadjusted(Duration::from_millis(millis));
            layout.advance_animations();
            let before_navigation = stage_rects(&layout);
            let before_order: Vec<_> = layout
                .active_monitor_ref()
                .unwrap()
                .workspaces_with_render_geo()
                .map(|(ws, _)| ws.id())
                .collect();
            assert!(layout.stage_navigate(None, 1));
            assert_eq!(
                layout
                    .active_monitor_ref()
                    .unwrap()
                    .workspaces_with_render_geo()
                    .map(|(ws, _)| ws.id())
                    .collect::<Vec<_>>(),
                before_order,
                "first-frame card occlusion must not jump"
            );
            assert_eq!(layout.active_workspace().unwrap().id(), initial);
            assert_eq!(stage_rects(&layout), before_navigation);
            let selected = layout.active_monitor_ref().unwrap().stage_selected();
            assert_ne!(selected, initial);
            layout
                .clock
                .set_unadjusted(Duration::from_millis(millis + 40));
            layout.advance_animations();
            let before_close = stage_rects(&layout);
            assert!(layout.confirm_stage());
            assert_eq!(
                stage_rects(&layout),
                before_close,
                "confirm scale={scale}, time={millis}"
            );
            layout
                .clock
                .set_unadjusted(Duration::from_millis(millis + 80));
            layout.advance_animations();
            let closing = stage_rects(&layout);
            assert_ne!(closing, before_close);
            layout.toggle_overview();
            assert_eq!(
                stage_rects(&layout),
                closing,
                "reverse scale={scale}, time={millis}"
            );
            Op::CompleteAnimations.apply(&mut layout);
            assert_eq!(
                layout.active_monitor_ref().unwrap().stage_selected(),
                selected
            );
            assert!(layout.stage_navigate(None, 1));
            let before_cancel = stage_rects(&layout);
            assert!(layout.cancel_stage());
            assert_eq!(stage_rects(&layout), before_cancel);
            Op::CompleteAnimations.apply(&mut layout);
            assert_eq!(layout.active_workspace().unwrap().id(), initial);
            layout.verify_invariants();
        }
    }
}

#[test]
fn stage_cancel_immediately_reopened_during_preview_preserves_poses_and_depth() {
    for scale in [1., 1.25, 1.5] {
        let mut layout = stage_layout(scale, 0.5);
        Op::CompleteAnimations.apply(&mut layout);
        assert!(layout.stage_navigate(None, 1));
        layout.clock.set_unadjusted(Duration::from_millis(200));
        layout.advance_animations();
        let poses = stage_rects(&layout);
        let order = |layout: &Layout<TestWindow>| {
            layout
                .active_monitor_ref()
                .unwrap()
                .workspaces_with_render_geo()
                .map(|(ws, _)| ws.id())
                .collect::<Vec<_>>()
        };
        let depths = order(&layout);
        assert!(layout.cancel_stage());
        assert_eq!(stage_rects(&layout), poses);
        layout.toggle_overview();
        assert_eq!(stage_rects(&layout), poses);
        assert_eq!(order(&layout), depths);
        assert!(layout.are_transitions_ongoing(None));
        Op::CompleteAnimations.apply(&mut layout);
        assert!(!layout.are_transitions_ongoing(None));
        layout.verify_invariants();
    }
}

#[test]
fn stage_rail_targets_existing_cards_and_only_the_explicit_new_slot_creates() {
    let mut layout = stage_layout(1.25, 0.5);
    Op::CompleteAnimations.apply(&mut layout);
    let mon = layout.active_monitor_ref().unwrap();
    let view = mon.view_size();
    let mut existing = std::collections::HashSet::new();
    let mut new_slot = None;
    let mut arrows = std::collections::HashSet::new();
    for x in 0..view.w.ceil() as i32 {
        let point = Point::from((x as f64, view.h - 38.));
        if let Some(delta) = mon.stage_rail_navigation(point) {
            arrows.insert(delta);
        }
        match mon.stage_rail_under(point) {
            Some(InsertWorkspace::Existing(id)) => {
                existing.insert(id);
                let (target, geo) = mon.stage_drop_target(point).unwrap();
                assert_eq!(target, InsertWorkspace::Existing(id));
                assert_eq!(geo, stage_rects(&layout)[mon.idx_of_ws(id).unwrap()]);
            }
            Some(InsertWorkspace::NewAt(idx)) => {
                assert_eq!(idx, mon.workspaces.len());
                let (_, geo) = mon.stage_drop_target(point).unwrap();
                assert_eq!(
                    geo,
                    stage_rects(&layout)[mon.idx_of_ws(mon.stage_selected()).unwrap()]
                );
                assert!(
                    geo.size.w > 0. && geo.size.h > 0.,
                    "NewAt must carry a real card transform"
                );
                new_slot = Some(point);
            }
            None => (),
        }
    }
    assert_eq!(existing.len(), 7);
    assert_eq!(arrows, [-1, 1].into_iter().collect());
    assert_eq!(
        mon.stage_drop_target(new_slot.unwrap()).unwrap().0,
        InsertWorkspace::NewAt(mon.workspaces.len())
    );
    assert!(mon.stage_drop_target(Point::from((0., 0.))).is_none());
    // Both ends of long workspace lists remain reachable using the same selection.
    while layout.stage_navigate(None, 1) {
        Op::CompleteAnimations.apply(&mut layout);
    }
    let mon = layout.active_monitor_ref().unwrap();
    assert_eq!(
        mon.idx_of_ws(mon.stage_selected()).unwrap(),
        mon.workspaces.len() - 1
    );
    while layout.stage_navigate(None, -1) {
        Op::CompleteAnimations.apply(&mut layout);
    }
    let mon = layout.active_monitor_ref().unwrap();
    assert_eq!(mon.idx_of_ws(mon.stage_selected()).unwrap(), 0);
    layout.verify_invariants();
}

#[test]
fn stage_rail_hidden_existing_workspaces_keep_a_real_drop_transform() {
    for scale in [1., 1.25, 1.5] {
        let mut layout = stage_layout(scale, 0.5);
        for delta in [-100, 100] {
            assert!(layout.stage_navigate(None, delta));
            Op::CompleteAnimations.apply(&mut layout);
            let mon = layout.active_monitor_ref().unwrap();
            let selected = mon.idx_of_ws(mon.stage_selected()).unwrap();
            let mut hidden = 0;
            for x in 0..mon.view_size().w.ceil() as i32 {
                let point = Point::from((x as f64, mon.view_size().h - 38.));
                let Some(InsertWorkspace::Existing(id)) = mon.stage_rail_under(point) else {
                    continue;
                };
                let idx = mon.idx_of_ws(id).unwrap();
                if idx.abs_diff(selected) <= 3 {
                    continue;
                }
                hidden += 1;
                let (target, proxy) = mon.stage_drop_target(point).unwrap();
                assert_eq!(target, InsertWorkspace::Existing(id));
                assert!(proxy.size.w > 0. && proxy.size.h > 0.);
                assert!(proxy.size.w / mon.view_size().w > 0.01);
            }
            assert!(
                hidden > 0,
                "the rail retains drop access to distant workspaces"
            );
        }
        layout.verify_invariants();
    }
}

#[test]
fn stage_legal_zoom_values_fit_above_the_rail_and_remain_finite() {
    for scale in [1., 1.25, 1.5, 100.] {
        for zoom in [0., 0.000_001, 0.5, 0.9, 1.] {
            let mut layout = stage_layout(scale, zoom);
            Op::CompleteAnimations.apply(&mut layout);
            let mon = layout.active_monitor_ref().unwrap();
            let idx = mon.idx_of_ws(mon.stage_selected()).unwrap();
            let center = stage_rects(&layout)[idx];
            let view = mon.view_size();
            for geo in stage_rects(&layout) {
                assert!([geo.loc.x, geo.loc.y, geo.size.w, geo.size.h]
                    .into_iter()
                    .all(f64::is_finite));
                assert!(geo.size.w >= 0. && geo.size.h >= 0.);
                assert!(mon.workspace_render_zoom(geo).is_finite());
            }
            assert!(center.size.w > 0. && center.size.h > 0.);
            approx::assert_abs_diff_eq!(
                center.size.w / center.size.h,
                view.w / view.h,
                epsilon = 1e-9
            );
            assert!(
                center.loc.y + center.size.h <= (view.h - view.h.min(76.)).max(0.) + 1. / scale
            );
            assert_eq!(mon.options.overview.zoom, zoom);
            if scale < 100. && zoom == 0.5 {
                approx::assert_abs_diff_eq!(
                    mon.workspace_render_zoom(center),
                    zoom,
                    epsilon = 1e-9
                );
            }
            layout.verify_invariants();
        }
    }
}

#[test]
#[ignore = "CPU-only microbenchmark; excludes GPU rendering and presentation"]
fn stage_expo_update_cost_for_128_windows() {
    for mode in [OverviewMode::Stage, OverviewMode::Expo] {
        let mut options = Options::default();
        options.layout.auto_stack_new_windows = false;
        let mut ops = vec![Op::AddOutput(1)];
        // Sixteen windows on each of eight workspaces exercise overview culling
        // as well as window traversal, rather than a single enormous column.
        for id in 1..=128 {
            ops.push(Op::AddWindow {
                params: TestWindowParams::new(id),
            });
            if id % 16 == 0 && id < 128 {
                ops.push(Op::FocusWorkspaceDown);
            }
        }
        ops.extend([Op::FocusWorkspace(3), Op::CompleteAnimations]);
        let mut layout = check_ops_with_options(options, ops);
        match mode {
            OverviewMode::Stage => layout.toggle_overview(),
            OverviewMode::Expo => layout.toggle_expo(),
            _ => unreachable!(),
        }
        Op::CompleteAnimations.apply(&mut layout);
        for _ in 0..32 {
            layout.update_render_elements(None);
        }
        let mut samples = Vec::new();
        for _ in 0..7 {
            let start = std::time::Instant::now();
            for _ in 0..200 {
                let monitor = layout.active_monitor_ref().unwrap();
                std::hint::black_box(monitor.workspaces_render_geo().count());
                std::hint::black_box(monitor.workspace_under(Point::from((640., 320.))));
                std::hint::black_box(&mut layout).update_render_elements(None);
            }
            samples.push(start.elapsed().as_secs_f64() * 1_000_000. / 200.);
        }
        samples.sort_by(f64::total_cmp);
        eprintln!(
            "overview-update mode={mode:?} windows=128 workspaces=8 median_us={:.3}",
            samples[3]
        );
        if mode == OverviewMode::Stage {
            let mut samples = Vec::new();
            for _ in 0..7 {
                let start = std::time::Instant::now();
                for index in 0..200 {
                    let direction = if index % 2 == 0 { 1 } else { -1 };
                    layout.stage_navigate(None, direction);
                    layout.clock.set_unadjusted(
                        layout.clock.now_unadjusted() + Duration::from_micros(6944),
                    );
                    layout.advance_animations();
                    std::hint::black_box(&mut layout).update_render_elements(None);
                }
                samples.push(start.elapsed().as_secs_f64() * 1_000_000. / 200.);
            }
            samples.sort_by(f64::total_cmp);
            eprintln!(
                "stage-retarget windows=128 workspaces=8 simulated_hz=144 median_us={:.3}",
                samples[3]
            );
            Op::CompleteAnimations.apply(&mut layout);
        }
        layout.verify_invariants();
    }
}

#[test]
#[ignore = "requires a DRM render node, EGL and Vulkan; never acquires display master"]
fn stage_pixels_share_card_occlusion_rail_and_privacy_on_gles_and_vulkan() {
    use crate::render_helpers::vulkan::{hardware_tests::setup, native::NativeRenderer};
    use crate::render_helpers::{render_to_vec, shaders, RenderTarget};
    use smithay::backend::allocator::Fourcc;
    let (vk, mut gles, _) = setup();
    crate::render_helpers::resources::init(&mut gles);
    shaders::init(&mut gles);
    let mut native = NativeRenderer::new(vk).unwrap();
    for scale in [1., 1.25, 1.5, 2.] {
        let mut layout = stage_layout_with_privacy(scale, 0.5, true);
        let mut options = Options::clone(&layout.options);
        options.layout.border.off = true;
        options.layout.focus_ring.off = true;
        options.layout.shadow.on = false;
        options.animations.window_wobble.off = true;
        layout.update_options(options);
        let monitor = layout.active_monitor_ref().unwrap();
        let selected_id = monitor.stage_selected();
        let selected_index = monitor.idx_of_ws(selected_id).unwrap();
        let central_window = *monitor.workspaces[selected_index]
            .windows()
            .next()
            .unwrap()
            .id();
        let rear_window = *monitor.workspaces[selected_index + 1]
            .windows()
            .next()
            .unwrap()
            .id();
        let mut ops = vec![Op::SetWindowFloating {
            id: Some(rear_window),
            floating: true,
        }];
        let rear_size = monitor.view_size().to_i32_round();
        ops.push(Op::SetForcedSize {
            id: rear_window,
            size: Some(rear_size),
        });
        ops.push(Op::Communicate(rear_window));
        for id in 1..=9 {
            if id == rear_window {
                continue;
            }
            ops.push(Op::SetFullscreenWindow {
                window: id,
                is_fullscreen: true,
            });
            ops.push(Op::Communicate(id));
        }
        ops.push(Op::CompleteAnimations);
        check_ops_on_layout(&mut layout, ops);
        for (_, window) in layout.windows() {
            window
                .0
                .render_color
                .set(Some(if *window.id() == central_window {
                    [0.15, 0.8, 0.2, 1.]
                } else {
                    [0.15, 0.2, 0.8, 1.]
                }));
        }
        layout.update_render_elements(None);
        let monitor = layout.active_monitor_ref().unwrap();
        assert!(!monitor.workspaces[selected_index].is_floating(&central_window));
        assert!(monitor.workspaces[selected_index + 1].is_floating(&rear_window));
        let geometry = stage_rects(&layout);
        let overlap = geometry[selected_index]
            .intersection(geometry[selected_index + 1])
            .unwrap();
        let sample = (overlap.loc + overlap.size.to_point().downscale(2.))
            .to_physical_precise_round::<f64, i32>(scale);
        assert_eq!(
            monitor
                .window_under(sample.to_f64().to_logical(scale))
                .unwrap()
                .0
                .id(),
            &central_window
        );
        macro_rules! draw {
            ($renderer:expr, $target:expr) => {{
                let renderer = &mut $renderer;
                let mut elements = Vec::new();
                layout.active_monitor_ref().unwrap().render_workspaces(
                    RenderCtx {
                        renderer,
                        target: $target,
                        xray: None,
                    },
                    true,
                    &mut |element| elements.push(element),
                );
                render_to_vec(
                    renderer,
                    (1280, 720).into(),
                    scale.into(),
                    Transform::Normal,
                    Fourcc::Abgr8888,
                    elements.iter().rev(),
                )
                .unwrap()
            }};
        }
        let output = draw!(native, RenderTarget::Output);
        let expected = draw!(gles, RenderTarget::Output);
        if let Some(directory) = std::env::var_os("REALAND_STAGE_PNG_DIR") {
            for (name, pixels) in [("vulkan", &output), ("gles", &expected)] {
                let file = std::fs::File::create(
                    std::path::Path::new(&directory)
                        .join(format!("realand-stage-{scale}-{name}.png")),
                )
                .unwrap();
                let mut encoder = png::Encoder::new(file, 1280, 720);
                encoder.set_color(png::ColorType::Rgba);
                encoder
                    .write_header()
                    .unwrap()
                    .write_image_data(pixels)
                    .unwrap();
            }
        }
        let worst = output
            .iter()
            .zip(&expected)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        if worst > 2 {
            let (index, _) = output
                .iter()
                .zip(&expected)
                .enumerate()
                .max_by_key(|(_, (a, b))| a.abs_diff(**b))
                .unwrap();
            let offset = index / 4 * 4;
            eprintln!(
                "Stage discrepancy at ({}, {}): native={:?}, GLES={:?}",
                index / 4 % 1280,
                index / 4 / 1280,
                &output[offset..offset + 4],
                &expected[offset..offset + 4]
            );
        }
        assert!(
            worst <= 2,
            "Stage output scale={scale}, worst channel difference={worst}"
        );
        let offset = (sample.y as usize * 1280 + sample.x as usize) * 4;
        let pixel = &output[offset..offset + 4];
        assert!(pixel[1] > 150 && pixel[0] < 100 && pixel[2] < 100,
            "front tiling card must cover rear floating card at rendered/hit-tested overlap: {pixel:?}");
        let capture = draw!(native, RenderTarget::ScreenCapture);
        let rear_center = (geometry[selected_index + 1].loc
            + geometry[selected_index + 1].size.to_point().downscale(2.))
        .to_physical_precise_round::<f64, i32>(scale);
        let rear_offset = (rear_center.y as usize * 1280 + rear_center.x as usize) * 4;
        let rear_pixel = &output[rear_offset..rear_offset + 4];
        assert!(
            rear_pixel[2] > 150 && rear_pixel[0] < 100 && rear_pixel[1] < 100,
            "rear floating card must be live outside the front card: {rear_pixel:?}"
        );
        let expected_capture = draw!(gles, RenderTarget::ScreenCapture);
        let worst = capture
            .iter()
            .zip(&expected_capture)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        assert!(
            worst <= 2,
            "Stage capture scale={scale}, worst channel difference={worst}"
        );
        let blocked = &capture[offset..offset + 4];
        assert!(
            blocked[..3].iter().all(|channel| *channel < 10),
            "capture target must propagate to the private live window: {blocked:?}"
        );
        // The bottom rail remains readable in both output and privacy captures.
        let rail_top = ((monitor.view_size().h - 76.).max(0.) * scale).round() as usize;
        assert!(
            output[rail_top * 1280 * 4..]
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[..3].iter().all(|channel| *channel > 200)),
            "workspace labels and counts must render as real text"
        );
        if let Some(directory) = std::env::var_os("REALAND_STAGE_PNG_DIR") {
            for (name, pixels) in [("output", &output), ("capture", &capture)] {
                let file = std::fs::File::create(
                    std::path::Path::new(&directory)
                        .join(format!("realand-stage-{scale}-{name}.png")),
                )
                .unwrap();
                let mut encoder = png::Encoder::new(file, 1280, 720);
                encoder.set_color(png::ColorType::Rgba);
                encoder
                    .write_header()
                    .unwrap()
                    .write_image_data(pixels)
                    .unwrap();
            }
        }
        let before = output;
        assert!(layout.stage_navigate(None, 1));
        assert_eq!(draw!(native, RenderTarget::Output), before, "preview retargeting must preserve geometry, occlusion, border emphasis and rail pixels at scale={scale}");
        assert_eq!(
            layout
                .active_monitor_ref()
                .unwrap()
                .window_under(sample.to_f64().to_logical(scale))
                .unwrap()
                .0
                .id(),
            &central_window,
            "the visible front card remains the first hit at retarget t=0"
        );
        let now = layout.clock.now_unadjusted();
        layout
            .clock
            .set_unadjusted(now + Duration::from_millis(200));
        layout.advance_animations();
        layout.update_render_elements(None);
        let previewing = draw!(native, RenderTarget::Output);
        assert_ne!(previewing, before);
        let previewing_gles = draw!(gles, RenderTarget::Output);
        assert!(
            previewing
                .iter()
                .zip(&previewing_gles)
                .all(|(a, b)| a.abs_diff(*b) <= 2),
            "preview animation must match on GLES/native"
        );
        assert!(layout.cancel_stage());
        assert_eq!(
            draw!(native, RenderTarget::Output),
            previewing,
            "Esc after preview navigation must preserve the composed first closing frame"
        );
        layout.toggle_overview();
        assert!(
            draw!(native, RenderTarget::Output) == previewing,
            "reopening after Esc continues from the current frame"
        );
        Op::CompleteAnimations.apply(&mut layout);
        layout.update_render_elements(None);
        let before = draw!(native, RenderTarget::Output);
        assert!(layout.stage_navigate(None, 1));
        assert_eq!(draw!(native, RenderTarget::Output), before);
        assert!(layout.confirm_stage());
        assert_eq!(
            draw!(native, RenderTarget::Output),
            before,
            "same-mode close begins from the rendered frame"
        );
        let now = layout.clock.now_unadjusted();
        layout
            .clock
            .set_unadjusted(now + Duration::from_millis(400));
        layout.advance_animations();
        layout.update_render_elements(None);
        let closing = draw!(native, RenderTarget::Output);
        assert_ne!(closing, before);
        let closing_gles = draw!(gles, RenderTarget::Output);
        let worst = closing
            .iter()
            .zip(&closing_gles)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        assert!(
            worst <= 2,
            "Stage closing scale={scale}, worst channel difference={worst}"
        );
        layout.toggle_overview();
        assert_eq!(
            draw!(native, RenderTarget::Output),
            closing,
            "reversal begins from the rendered frame"
        );
        Op::CompleteAnimations.apply(&mut layout);
        let mut options = Options::clone(&layout.options);
        options.animations.overview_open_close =
            realand_config::Animations::default().overview_open_close;
        layout.update_options(options);
        assert!(layout.stage_navigate(None, 1));
        let start = layout.clock.now_unadjusted();
        layout
            .clock
            .set_unadjusted(start + Duration::from_millis(80));
        layout.advance_animations();
        layout.update_render_elements(None);
        let spring_frame = draw!(native, RenderTarget::Output);
        assert!(layout.stage_navigate(None, -1));
        assert_eq!(
            draw!(native, RenderTarget::Output),
            spring_frame,
            "spring retarget starts at the presented pixels"
        );
        layout
            .clock
            .set_unadjusted(start + Duration::from_millis(87));
        layout.advance_animations();
        layout.update_render_elements(None);
        let carried = draw!(native, RenderTarget::Output);
        let carried_gles = draw!(gles, RenderTarget::Output);
        assert_ne!(
            carried, spring_frame,
            "spring momentum moves on the next 144 Hz frame"
        );
        assert!(
            carried
                .iter()
                .zip(&carried_gles)
                .all(|(a, b)| a.abs_diff(*b) <= 2),
            "carried spring geometry matches on GLES/native"
        );
        assert!(layout.confirm_stage());
        assert_eq!(
            draw!(native, RenderTarget::Output),
            carried,
            "confirm carries the current spring pixels into closing"
        );
        layout
            .clock
            .set_unadjusted(start + Duration::from_millis(94));
        layout.advance_animations();
        layout.update_render_elements(None);
        let closing_spring = draw!(native, RenderTarget::Output);
        let closing_spring_gles = draw!(gles, RenderTarget::Output);
        assert!(
            closing_spring
                .iter()
                .zip(&closing_spring_gles)
                .all(|(a, b)| a.abs_diff(*b) <= 2),
            "closing momentum matches on GLES/native"
        );
        let capture_spring = draw!(native, RenderTarget::ScreenCapture);
        let capture_spring_gles = draw!(gles, RenderTarget::ScreenCapture);
        assert!(
            capture_spring
                .iter()
                .zip(&capture_spring_gles)
                .all(|(a, b)| a.abs_diff(*b) <= 2),
            "privacy captures follow the same moving geometry"
        );
        layout.verify_invariants();
    }
    eprintln!("PASS: Stage real pixels preserve front-card occlusion, rail labels/counts, privacy and same-mode transitions on GLES/Vulkan at four scales");
}
