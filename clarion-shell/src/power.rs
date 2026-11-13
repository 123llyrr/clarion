// SPDX-License-Identifier: GPL-3.0-or-later
//! Platform power profiles.
//!
//! Laptops expose their ACPI platform profile through sysfs; desktops and
//! newer distros route the same three modes through power-profiles-daemon.
//! Both namespaces are folded into one enum here so the battery popup, the
//! worker and the tests never disagree about what a profile is called.
use std::path::Path;

/// The three profiles every supported platform agrees on. The order is the
/// left-to-right order the battery popup paints them in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PowerProfile {
    LowPower,
    #[default]
    Balanced,
    Performance,
}

impl PowerProfile {
    pub const ALL: [Self; 3] = [Self::LowPower, Self::Balanced, Self::Performance];

    /// The name power-profiles-daemon uses for `powerprofilesctl set`.
    pub fn id(self) -> &'static str {
        match self {
            Self::LowPower => "power-saver",
            Self::Balanced => "balanced",
            Self::Performance => "performance",
        }
    }

    /// The name the kernel's `platform_profile` attribute uses.
    pub fn sysfs_name(self) -> &'static str {
        match self {
            Self::LowPower => "low-power",
            Self::Balanced => "balanced",
            Self::Performance => "performance",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::LowPower => "节能",
            Self::Balanced => "平衡",
            Self::Performance => "性能",
        }
    }

    /// Bundled mark. These stay out of `icons::symbolic` on purpose: the
    /// desktop themes' `power-profile-*-symbolic` icons are tuned for 16 px
    /// status trays and collapse into detail at the popup's 26 px, with the
    /// performance needle in particular reading as a smudge.
    pub fn icon(self) -> &'static str {
        match self {
            Self::LowPower => "power-saver",
            Self::Balanced => "balanced",
            Self::Performance => "performance",
        }
    }

    fn index(self) -> usize {
        match self {
            Self::LowPower => 0,
            Self::Balanced => 1,
            Self::Performance => 2,
        }
    }

    /// Both the kernel and power-profiles-daemon spellings are accepted.
    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "low-power" | "power-saver" | "power_saver" => Some(Self::LowPower),
            "balanced" => Some(Self::Balanced),
            "performance" => Some(Self::Performance),
            _ => None,
        }
    }
}

/// Parse the whitespace-separated `platform_profile_choices` attribute.
pub fn parse_choices(text: &str) -> Vec<PowerProfile> {
    let mut available = [false; 3];
    for name in text.split_whitespace() {
        if let Some(profile) = PowerProfile::from_name(name) {
            available[profile.index()] = true;
        }
    }
    PowerProfile::ALL
        .into_iter()
        .filter(|profile| available[profile.index()])
        .collect()
}

/// Read the kernel's current profile and the choices it advertises.
///
/// `None` means this kernel does not expose the interface at all, so the
/// caller may fall back to power-profiles-daemon. An existing interface with
/// no current value still reports its choices.
pub fn sysfs_snapshot(root: &Path) -> Option<(Option<PowerProfile>, Vec<PowerProfile>)> {
    let choices = std::fs::read_to_string(root.join("platform_profile_choices")).ok()?;
    let profiles = parse_choices(&choices);
    if profiles.is_empty() {
        return None;
    }
    let current = std::fs::read_to_string(root.join("platform_profile"))
        .ok()
        .and_then(|value| PowerProfile::from_name(&value));
    Some((current, profiles))
}

/// Parse `powerprofilesctl list`, which marks the active profile with `*`.
/// Driver lines (e.g. `CpuDriver:`) end in a colon too but map to no profile,
/// so they are ignored naturally.
pub fn parse_powerprofilesctl_list(text: &str) -> (Option<PowerProfile>, Vec<PowerProfile>) {
    let mut available = [false; 3];
    let mut current = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (active, name) = match line.strip_prefix("* ") {
            Some(name) => (true, name),
            None => (false, line),
        };
        let Some(name) = name.strip_suffix(':') else {
            continue;
        };
        let Some(profile) = PowerProfile::from_name(name) else {
            continue;
        };
        available[profile.index()] = true;
        if active {
            current = Some(profile);
        }
    }
    let profiles = PowerProfile::ALL
        .into_iter()
        .filter(|profile| available[profile.index()])
        .collect();
    (current, profiles)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_name_spaces_fold_into_the_same_profiles() {
        assert_eq!(
            PowerProfile::from_name("low-power"),
            Some(PowerProfile::LowPower)
        );
        assert_eq!(
            PowerProfile::from_name("power-saver"),
            Some(PowerProfile::LowPower)
        );
        assert_eq!(
            PowerProfile::from_name(" balanced \n"),
            Some(PowerProfile::Balanced)
        );
        assert_eq!(
            PowerProfile::from_name("performance"),
            Some(PowerProfile::Performance)
        );
        assert_eq!(PowerProfile::from_name("quiet"), None);
        assert_eq!(
            PowerProfile::LowPower.id(),
            "power-saver",
            "daemon spelling"
        );
        assert_eq!(
            PowerProfile::LowPower.sysfs_name(),
            "low-power",
            "kernel spelling"
        );
    }

    #[test]
    fn choices_keep_the_canonical_left_to_right_order() {
        assert_eq!(
            parse_choices("performance low-power balanced\n"),
            PowerProfile::ALL.to_vec()
        );
        assert_eq!(
            parse_choices("balanced performance"),
            vec![PowerProfile::Balanced, PowerProfile::Performance]
        );
        assert!(parse_choices("quiet").is_empty());
    }

    #[test]
    fn powerprofilesctl_list_reports_the_starred_profile() {
        let (current, profiles) = parse_powerprofilesctl_list(
            "  performance:\n    CpuDriver:\tintel_pstate\n    Degraded:   no\n\n\
             * balanced:\n    CpuDriver:\tintel_pstate\n\n  power-saver:\n    CpuDriver:\tintel_pstate\n",
        );
        assert_eq!(current, Some(PowerProfile::Balanced));
        assert_eq!(profiles, PowerProfile::ALL.to_vec());
    }

    #[test]
    fn missing_or_unknown_current_values_leave_the_choices_intact() {
        let (current, profiles) = parse_powerprofilesctl_list("* quiet:\n balanced:\n");
        assert_eq!(current, None);
        assert_eq!(profiles, vec![PowerProfile::Balanced]);
    }

    #[test]
    fn sysfs_snapshot_reads_choices_and_current() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("platform_profile_choices"),
            "low-power balanced performance\n",
        )
        .unwrap();
        std::fs::write(directory.path().join("platform_profile"), "low-power\n").unwrap();
        let (current, profiles) = sysfs_snapshot(directory.path()).unwrap();
        assert_eq!(current, Some(PowerProfile::LowPower));
        assert_eq!(profiles, PowerProfile::ALL.to_vec());

        std::fs::remove_file(directory.path().join("platform_profile_choices")).unwrap();
        assert!(
            sysfs_snapshot(directory.path()).is_none(),
            "a missing interface must let the daemon fallback run"
        );
    }
}
