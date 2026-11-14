//! Power-profile confirmation without a real platform backend.
use std::collections::HashMap;

use gpui::TestAppContext;

use super::{backend, testing, Action};
use crate::power::PowerProfile;

#[gpui::test]
fn a_stale_heartbeat_cannot_undo_a_power_switch(cx: &mut TestAppContext) {
    let host = cx.update(|cx| testing::test_host(cx, Default::default(), HashMap::new()));
    host.update(cx, |host, cx| {
        host.system.power_profile = Some(PowerProfile::Balanced);
        host.system.power_profiles = PowerProfile::ALL.to_vec();
        host.handle(Action::PowerProfile(PowerProfile::Performance), cx);
    });
    host.read_with(cx, |host, _| {
        assert_eq!(
            host.system.power_profile,
            Some(PowerProfile::Balanced),
            "the accent fill waits for confirmation"
        );
        assert_eq!(
            host.power_profile_pending,
            Some(PowerProfile::Performance),
            "the click starts a pending switch"
        );
    });

    // The one-second heartbeat still carries the hardware cache from before
    // the switch. It must not change the confirmed mode.
    host.update(cx, |host, cx| {
        let mut stale = host.system.clone();
        stale.clock = "12:34:56".into();
        stale.power_profile = Some(PowerProfile::Balanced);
        host.backend_event(backend::Event::System(stale), cx);
    });
    host.read_with(cx, |host, _| {
        assert_eq!(host.system.power_profile, Some(PowerProfile::Balanced));
        assert_eq!(
            host.power_profile_pending,
            Some(PowerProfile::Performance)
        );
    });

    // A poll that still sees the old mode is ignored until the worker answers.
    host.update(cx, |host, cx| {
        host.backend_event(
            backend::Event::PowerProfile {
                profile: Some(PowerProfile::Balanced),
                profiles: PowerProfile::ALL.to_vec(),
                settled: false,
                epoch: 0,
            },
            cx,
        );
    });
    host.read_with(cx, |host, _| {
        assert_eq!(
            host.system.power_profile,
            Some(PowerProfile::Balanced),
            "the old poll is discarded while a switch is in flight"
        );
    });

    // The read-back after the command settles the switch and only now paints
    // the chosen button.
    host.update(cx, |host, cx| {
        host.backend_event(
            backend::Event::PowerProfile {
                profile: Some(PowerProfile::Performance),
                profiles: PowerProfile::ALL.to_vec(),
                settled: true,
                epoch: 1,
            },
            cx,
        );
    });
    host.read_with(cx, |host, _| {
        assert_eq!(
            host.system.power_profile,
            Some(PowerProfile::Performance)
        );
        assert!(host.power_profile_pending.is_none());
    });

    // Even a poll that was queued before the switch and arrives after the
    // read-back carries the old epoch; it must not paint the old mode again.
    host.update(cx, |host, cx| {
        host.backend_event(
            backend::Event::PowerProfile {
                profile: Some(PowerProfile::Balanced),
                profiles: PowerProfile::ALL.to_vec(),
                settled: false,
                epoch: 0,
            },
            cx,
        );
    });
    host.read_with(cx, |host, _| {
        assert_eq!(
            host.system.power_profile,
            Some(PowerProfile::Performance),
            "stale polls cannot overwrite a settled switch"
        );
    });
}

#[gpui::test]
fn a_failed_switch_reads_back_the_old_profile(cx: &mut TestAppContext) {
    let host = cx.update(|cx| testing::test_host(cx, Default::default(), HashMap::new()));
    host.update(cx, |host, cx| {
        host.system.power_profile = Some(PowerProfile::Balanced);
        host.system.power_profiles = PowerProfile::ALL.to_vec();
        host.handle(Action::PowerProfile(PowerProfile::Performance), cx);
    });
    host.read_with(cx, |host, _| {
        assert_eq!(host.system.power_profile, Some(PowerProfile::Balanced));
        assert_eq!(
            host.power_profile_pending,
            Some(PowerProfile::Performance)
        );
    });

    // The platform never left balanced, so the settled read-back restores the
    // real state and never paints the requested fill.
    host.update(cx, |host, cx| {
        host.backend_event(
            backend::Event::PowerProfile {
                profile: Some(PowerProfile::Balanced),
                profiles: PowerProfile::ALL.to_vec(),
                settled: true,
                epoch: 1,
            },
            cx,
        );
    });
    host.read_with(cx, |host, _| {
        assert_eq!(host.system.power_profile, Some(PowerProfile::Balanced));
        assert!(host.power_profile_pending.is_none());
    });
}
