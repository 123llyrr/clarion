// SPDX-License-Identifier: GPL-3.0-or-later
//! Battery state for the bar indicator and the Control Center card.
//!
//! The kernel reports only a handful of raw values; the display state, its
//! icon and its tone are derived here so the bar and the popup can never
//! disagree. The two thresholds match UPower's default policy (20% low, 5%
//! critical), which is what the rest of the desktop already reacts to.
use crate::backend::BatteryStatus;

/// The battery is low enough to warn about while discharging.
pub const LOW: f64 = 0.20;
/// The battery is about to run out while discharging.
pub const CRITICAL: f64 = 0.05;
/// Nothing meaningful is left; the firmware is about to cut the machine off.
pub const EMPTY: f64 = 0.01;

/// What the bar and the Control Center actually show. This is richer than the
/// kernel status because it folds in the charge level and the AC state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatteryState {
    Charging,
    Full,
    /// Mains connected, but the battery is not taking charge (a firmware or
    /// threshold hold, or a battery that has finished a partial charge).
    PluggedIn,
    Discharging,
    Low,
    Critical,
    Empty,
    Missing,
    Unknown,
}

/// How an icon should be tinted. Text never uses this; it stays at full alpha.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatteryTone {
    Normal,
    Charging,
    Low,
    Critical,
    Muted,
}

/// Fold the kernel's raw values into one display state.
pub fn state(percent: Option<f64>, status: BatteryStatus, ac_online: bool) -> BatteryState {
    let Some(percent) = percent
        .filter(|value| value.is_finite())
        .map(|value| value.clamp(0., 1.))
    else {
        return BatteryState::Missing;
    };
    match status {
        BatteryStatus::Full => BatteryState::Full,
        // A firmware that reports "Charging" at 100% is already done; showing
        // a charging bolt there would promise progress that cannot happen.
        BatteryStatus::Charging if percent >= 0.995 => BatteryState::Full,
        BatteryStatus::Charging => BatteryState::Charging,
        BatteryStatus::NotCharging if ac_online => BatteryState::PluggedIn,
        BatteryStatus::NotCharging | BatteryStatus::Discharging => {
            if percent <= EMPTY {
                BatteryState::Empty
            } else if percent <= CRITICAL {
                BatteryState::Critical
            } else if percent <= LOW {
                BatteryState::Low
            } else {
                BatteryState::Discharging
            }
        }
        BatteryStatus::Unknown => BatteryState::Unknown,
    }
}

pub fn tone(state: BatteryState) -> BatteryTone {
    match state {
        BatteryState::Charging | BatteryState::Full => BatteryTone::Charging,
        BatteryState::Low => BatteryTone::Low,
        BatteryState::Critical | BatteryState::Empty => BatteryTone::Critical,
        BatteryState::Missing | BatteryState::Unknown => BatteryTone::Muted,
        BatteryState::PluggedIn | BatteryState::Discharging => BatteryTone::Normal,
    }
}

pub fn label(state: BatteryState) -> &'static str {
    match state {
        BatteryState::Charging => "充电中",
        BatteryState::Full => "已充满",
        BatteryState::PluggedIn => "已接电源",
        BatteryState::Discharging => "使用电池",
        BatteryState::Low => "电量低",
        BatteryState::Critical => "电量极低",
        BatteryState::Empty => "电量耗尽",
        BatteryState::Missing => "无电池",
        BatteryState::Unknown => "状态未知",
    }
}

/// The freedesktop icon name for a state. The desktop theme draws the actual
/// colours (green charge, amber/red low levels); the shell only picks the
/// level bucket and the variant.
pub fn icon(state: BatteryState, percent: Option<f64>) -> String {
    let level = percent.map(level).unwrap_or(50);
    match state {
        BatteryState::Charging => format!("battery-level-{level}-charging-symbolic"),
        BatteryState::Full => "battery-level-100-charged-symbolic".to_owned(),
        BatteryState::PluggedIn => format!("battery-level-{level}-plugged-in-symbolic"),
        BatteryState::Discharging => format!("battery-level-{level}-symbolic"),
        BatteryState::Low => "battery-caution-symbolic".to_owned(),
        BatteryState::Critical => "battery-caution-symbolic".to_owned(),
        BatteryState::Empty => "battery-empty-symbolic".to_owned(),
        BatteryState::Missing => "battery-missing-symbolic".to_owned(),
        BatteryState::Unknown => "battery-symbolic".to_owned(),
    }
}

/// The nearest ten-percent mark the icon family provides. Rounding rather
/// than truncating keeps 95–99% on the full mark, which is what the battery
/// really looks like; the themes use the same convention.
fn level(percent: f64) -> u32 {
    ((percent.clamp(0., 1.) * 10.).round() as u32 * 10).min(100)
}

pub fn percent_text(percent: f64) -> String {
    format!("{}%", (percent.clamp(0., 1.) * 100.).round() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charging_and_full_are_distinguished() {
        assert_eq!(
            state(Some(0.54), BatteryStatus::Charging, true),
            BatteryState::Charging
        );
        assert_eq!(
            state(Some(1.), BatteryStatus::Charging, true),
            BatteryState::Full
        );
        assert_eq!(tone(BatteryState::Charging), BatteryTone::Charging);
        assert_eq!(tone(BatteryState::Full), BatteryTone::Charging);
    }

    #[test]
    fn plugged_in_and_discharging_are_distinguished() {
        assert_eq!(
            state(Some(0.72), BatteryStatus::NotCharging, true),
            BatteryState::PluggedIn
        );
        assert_eq!(
            state(Some(0.72), BatteryStatus::Discharging, false),
            BatteryState::Discharging
        );
        // "Not charging" without mains is a discharging battery, not a plug.
        assert_eq!(
            state(Some(0.72), BatteryStatus::NotCharging, false),
            BatteryState::Discharging
        );
        assert_eq!(tone(BatteryState::PluggedIn), BatteryTone::Normal);
        assert_eq!(tone(BatteryState::Discharging), BatteryTone::Normal);
    }

    #[test]
    fn low_critical_and_empty_earn_their_own_tones() {
        for (percent, expected, expected_tone) in [
            (0.20, BatteryState::Low, BatteryTone::Low),
            (0.10, BatteryState::Low, BatteryTone::Low),
            (0.05, BatteryState::Critical, BatteryTone::Critical),
            (0.02, BatteryState::Critical, BatteryTone::Critical),
            (0.01, BatteryState::Empty, BatteryTone::Critical),
            (0., BatteryState::Empty, BatteryTone::Critical),
        ] {
            let state = state(Some(percent), BatteryStatus::Discharging, false);
            assert_eq!(state, expected, "{percent} should read as {expected:?}");
            assert_eq!(
                tone(state),
                expected_tone,
                "{percent} should use {expected_tone:?}"
            );
        }
        // Just above the low mark stays an ordinary charge level.
        assert_eq!(
            state(Some(0.21), BatteryStatus::Discharging, false),
            BatteryState::Discharging
        );
    }

    #[test]
    fn missing_unknown_and_percent_text_are_predictable() {
        assert_eq!(
            state(None, BatteryStatus::Unknown, false),
            BatteryState::Missing
        );
        assert_eq!(
            state(Some(0.5), BatteryStatus::Unknown, false),
            BatteryState::Unknown
        );
        assert_eq!(tone(BatteryState::Missing), BatteryTone::Muted);
        assert_eq!(tone(BatteryState::Unknown), BatteryTone::Muted);
        assert_eq!(percent_text(0.537), "54%");
        assert_eq!(percent_text(-1.), "0%");
        assert_eq!(percent_text(2.), "100%");
    }

    #[test]
    fn icons_follow_the_level_and_the_state() {
        assert_eq!(
            icon(BatteryState::Charging, Some(0.54)),
            "battery-level-50-charging-symbolic"
        );
        assert_eq!(
            icon(BatteryState::Charging, Some(0.99)),
            "battery-level-100-charging-symbolic"
        );
        assert_eq!(
            icon(BatteryState::Full, Some(1.)),
            "battery-level-100-charged-symbolic"
        );
        assert_eq!(
            icon(BatteryState::PluggedIn, Some(0.72)),
            "battery-level-70-plugged-in-symbolic"
        );
        assert_eq!(
            icon(BatteryState::Discharging, Some(0.72)),
            "battery-level-70-symbolic"
        );
        assert_eq!(
            icon(BatteryState::Low, Some(0.2)),
            "battery-caution-symbolic"
        );
        assert_eq!(
            icon(BatteryState::Critical, Some(0.05)),
            "battery-caution-symbolic"
        );
        assert_eq!(
            icon(BatteryState::Empty, Some(0.)),
            "battery-empty-symbolic"
        );
        assert_eq!(
            icon(BatteryState::Missing, None),
            "battery-missing-symbolic"
        );
        assert_eq!(
            icon(BatteryState::Unknown, Some(0.5)),
            "battery-symbolic"
        );
        assert_eq!(level(0.99), 100);
        assert_eq!(level(0.94), 90);
        assert_eq!(level(0.), 0);
    }
}
