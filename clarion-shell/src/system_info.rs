// SPDX-License-Identifier: GPL-3.0-or-later
//! Static machine facts for the "About This Mac" card.
//!
//! The system worker collects these once at startup: they never change while
//! the shell runs, so they must not ride the one-second hardware poll. Every
//! reader tolerates a missing file and the card hides the empty rows.
use std::ffi::CString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::preferences::Environment;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SystemFacts {
    /// DMI product name (for example "Blade"), the About card's headline.
    pub model: String,
    pub hostname: String,
    pub os_name: String,
    pub kernel: String,
    pub cpu: String,
    /// Display controller name, refined by `lspci` in the worker when present.
    pub gpu: String,
    /// DMI serial when the platform publishes it to unprivileged readers.
    pub serial: String,
    pub memory_total: Option<u64>,
    pub disk_total: Option<u64>,
    pub disk_free: Option<u64>,
}

impl SystemFacts {
    pub fn collect(environment: &Environment) -> Self {
        let os_name = fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|text| parse_os_release(&text))
            .unwrap_or_else(|| "Linux".into());
        let hostname = fs::read_to_string("/etc/hostname")
            .ok()
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty())
            .or_else(|| {
                environment
                    .get("HOSTNAME")
                    .map(|name| name.trim().to_owned())
                    .filter(|name| !name.is_empty())
            })
            .unwrap_or_default();
        let kernel = fs::read_to_string("/proc/sys/kernel/osrelease")
            .map(|line| line.trim().to_owned())
            .unwrap_or_default();
        let cpu = fs::read_to_string("/proc/cpuinfo")
            .ok()
            .and_then(|text| parse_cpu_model(&text))
            .unwrap_or_else(|| std::env::consts::ARCH.into());
        // NVIDIA publishes the marketing name in procfs. The generic sysfs
        // fallback (driver + PCI id) is applied by the worker only when no
        // better source answers; `lspci` is its naming source for the others.
        let gpu = nvidia_model().unwrap_or_default();
        let memory_total = fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|text| parse_mem_total(&text));
        let (disk_total, disk_free) = disk_usage(Path::new("/"));
        let model = read_trim("/sys/devices/virtual/dmi/id/product_name")
            .or_else(|| read_trim("/sys/devices/virtual/dmi/id/product_version"))
            .filter(|model| !model.is_empty())
            .unwrap_or_else(|| hostname.clone());
        let serial = read_trim("/sys/devices/virtual/dmi/id/product_serial").unwrap_or_default();
        Self {
            model,
            hostname,
            os_name,
            kernel,
            cpu,
            gpu,
            serial,
            memory_total,
            disk_total,
            disk_free,
        }
    }
}

/// The NVIDIA driver's own marketing name, e.g. "NVIDIA GeForce GTX 1060 with
/// Max-Q Design". This is the only file-based source with a real model name.
fn nvidia_model() -> Option<String> {
    let entries = fs::read_dir("/proc/driver/nvidia/gpus").ok()?;
    let mut cards: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    cards.sort();
    cards.into_iter().find_map(|card| {
        let information = fs::read_to_string(card.join("information")).ok()?;
        parse_nvidia_information(&information)
    })
}

pub(crate) fn parse_nvidia_information(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let model = line.strip_prefix("Model:")?.trim();
        (!model.is_empty()).then(|| model.to_owned())
    })
}

/// A compact display-controller name from sysfs, used when neither the driver
/// nor `lspci` names the device. Discrete vendors win over an integrated
/// controller so a hybrid laptop shows the GPU the compositor renders with.
pub(crate) fn sysfs_gpu() -> String {
    let Ok(entries) = fs::read_dir("/sys/class/drm") else {
        return String::new();
    };
    let mut cards: Vec<PathBuf> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let index = name.strip_prefix("card")?;
            if index.is_empty() || !index.chars().all(|ch| ch.is_ascii_digit()) {
                return None;
            }
            Some(entry.path())
        })
        .collect();
    cards.sort();
    let mut fallback = String::new();
    for card in cards {
        let Ok(uevent) = fs::read_to_string(card.join("device/uevent")) else {
            continue;
        };
        let driver = uevent
            .lines()
            .find_map(|line| line.strip_prefix("DRIVER="))
            .unwrap_or_default();
        if driver.is_empty() {
            continue;
        }
        let vendor = match driver {
            "nvidia" => "NVIDIA",
            "amdgpu" | "radeon" => "AMD",
            "i915" | "xe" => "Intel",
            other => other,
        };
        let pci = uevent
            .lines()
            .find_map(|line| line.strip_prefix("PCI_ID="))
            .unwrap_or_default();
        let name = if pci.is_empty() {
            vendor.to_owned()
        } else {
            format!("{vendor} ({pci})")
        };
        if matches!(vendor, "NVIDIA" | "AMD") {
            return name;
        }
        if fallback.is_empty() {
            fallback = name;
        }
    }
    fallback
}

/// Pick a display controller from `lspci -mm`. The machine-readable form keeps
/// each field quoted, so no locale or column parsing is involved. NVIDIA/AMD
/// entries win over the integrated controller on hybrid laptops.
pub(crate) fn parse_lspci_gpu(text: &str) -> Option<String> {
    let mut fallback = None;
    for line in text.lines() {
        let fields = quoted_fields(line);
        if fields.len() < 3 {
            continue;
        }
        if !matches!(
            fields[0],
            "VGA compatible controller" | "3D controller" | "Display controller"
        ) {
            continue;
        }
        let vendor = fields[1];
        let device = fields[2];
        let name = if device.is_empty() { vendor } else { device };
        if vendor.contains("NVIDIA") || vendor.contains("AMD") || vendor.contains("Advanced Micro") {
            return Some(name.to_owned());
        }
        if fallback.is_none() {
            fallback = Some(name.to_owned());
        }
    }
    fallback
}

fn quoted_fields(line: &str) -> Vec<&str> {
    let mut fields = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find('"') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('"') else {
            break;
        };
        fields.push(&after[..end]);
        rest = &after[end + 1..];
    }
    fields
}

/// A one-line sysfs file, trimmed. Empty or unreadable files read as `None`.
fn read_trim(path: &str) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// `PRETTY_NAME` first, then `NAME` (+ `VERSION` when it adds something).
pub(crate) fn parse_os_release(text: &str) -> Option<String> {
    let mut name = None;
    let mut version = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = unquote(value.trim());
        match key.trim() {
            "PRETTY_NAME" if !value.is_empty() => return Some(value),
            "NAME" => name = Some(value),
            "VERSION" => version = Some(value),
            _ => {}
        }
    }
    match (name, version) {
        (Some(name), Some(version)) if !name.is_empty() && !version.is_empty() => {
            Some(format!("{name} {version}"))
        }
        (Some(name), _) if !name.is_empty() => Some(name),
        _ => None,
    }
}

fn unquote(value: &str) -> String {
    let value = value.trim();
    let quoted = value.len() >= 2 && value.starts_with('"') && value.ends_with('"');
    let value = if quoted { &value[1..value.len() - 1] } else { value };
    if !quoted {
        return value.to_owned();
    }
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// `MemTotal` is reported in kibibytes.
pub(crate) fn parse_mem_total(text: &str) -> Option<u64> {
    for line in text.lines() {
        let Some(value) = line.strip_prefix("MemTotal:") else {
            continue;
        };
        let kib = value.split_whitespace().next()?.parse::<u64>().ok()?;
        return Some(kib.saturating_mul(1024));
    }
    None
}

/// x86 exposes `model name`; some ARM kernels only publish `Hardware`.
pub(crate) fn parse_cpu_model(text: &str) -> Option<String> {
    for key in ["model name", "Hardware", "cpu model"] {
        for line in text.lines() {
            let Some((label, value)) = line.split_once(':') else {
                continue;
            };
            if label.trim() == key {
                let value = value.trim();
                if !value.is_empty() {
                    return Some(value.to_owned());
                }
            }
        }
    }
    None
}

/// Bytes available to the user and the filesystem's total size.
// The `statvfs` typedefs are `u64` on 64-bit Linux and narrower elsewhere; the
// casts are needed on other targets even though this one flags them as no-ops.
#[allow(clippy::unnecessary_cast)]
pub(crate) fn disk_usage(path: &Path) -> (Option<u64>, Option<u64>) {
    let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
        return (None, None);
    };
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return (None, None);
    }
    let stat = unsafe { stat.assume_init() };
    let fragment = stat.f_frsize as u64;
    let total = (stat.f_blocks as u64).saturating_mul(fragment);
    let free = (stat.f_bavail as u64).saturating_mul(fragment);
    (Some(total), Some(free))
}

/// One decimal for most sizes, a whole number when it lands close enough.
pub(crate) fn format_gib(bytes: u64) -> String {
    let gib = bytes as f64 / (1024. * 1024. * 1024.);
    if (gib - gib.round()).abs() < 0.05 {
        format!("{} GB", gib.round() as u64)
    } else {
        format!("{gib:.1} GB")
    }
}

#[cfg(test)]
mod tests {
    use ::core::prelude::v1::test;
    use std::path::Path;

    use super::*;

    #[test]
    fn os_release_prefers_the_pretty_name_and_unescapes_quotes() {
        let text = "NAME=\"Arch Linux\"\nPRETTY_NAME=\"Arch Linux \\\"Rolling\\\"\"\n";
        assert_eq!(
            parse_os_release(text).as_deref(),
            Some("Arch Linux \"Rolling\"")
        );
        let text = "NAME=\"Debian GNU/Linux\"\nVERSION=\"12 (bookworm)\"\n";
        assert_eq!(
            parse_os_release(text).as_deref(),
            Some("Debian GNU/Linux 12 (bookworm)")
        );
        assert_eq!(parse_os_release(""), None);
    }

    #[test]
    fn meminfo_and_cpuinfo_fall_back_cleanly() {
        assert_eq!(
            parse_mem_total("MemTotal:       16316384 kB\nMemFree: 1 kB\n"),
            Some(16_316_384 * 1024)
        );
        assert_eq!(parse_mem_total("MemFree: 1 kB\n"), None);
        let cpu = "processor\t: 0\nmodel name\t: Intel(R) Core(TM) i7-9750H\n";
        assert_eq!(
            parse_cpu_model(cpu).as_deref(),
            Some("Intel(R) Core(TM) i7-9750H")
        );
        assert_eq!(
            parse_cpu_model("processor\t: 0\nHardware\t: BCM2835\n").as_deref(),
            Some("BCM2835")
        );
        assert_eq!(parse_cpu_model("processor\t: 0\n"), None);
    }

    #[test]
    fn disk_usage_reads_a_real_filesystem() {
        let (total, free) = disk_usage(Path::new("/"));
        let total = total.expect("the root filesystem reports a size");
        let free = free.expect("the root filesystem reports free space");
        assert!(total > 0);
        assert!(free <= total);
    }

    #[test]
    fn sizes_read_as_gigabytes() {
        assert_eq!(format_gib(16 * 1024 * 1024 * 1024), "16 GB");
        assert_eq!(format_gib(31_200_000_000), "29.1 GB");
    }

    #[test]
    fn nvidia_procfs_exposes_the_marketing_name() {
        let text = "Model: \t\t NVIDIA GeForce GTX 1060 with Max-Q Design\nIRQ: \t\t 143\n";
        assert_eq!(
            parse_nvidia_information(text).as_deref(),
            Some("NVIDIA GeForce GTX 1060 with Max-Q Design")
        );
        assert_eq!(parse_nvidia_information("IRQ: 1\n"), None);
    }

    #[test]
    fn lspci_prefers_the_discrete_display_controller() {
        let text = concat!(
            "00:02.0 \"VGA compatible controller\" \"Intel Corporation\" ",
            "\"CoffeeLake-H GT2 [UHD Graphics 630]\" -p00 \"Razer USA Ltd.\" \"Device 2001\"\n",
            "01:00.0 \"VGA compatible controller\" \"NVIDIA Corporation\" ",
            "\"GP106M [GeForce GTX 1060 Mobile]\" -ra1 -p00 \"Razer USA Ltd.\" \"Device 2001\"\n",
        );
        assert_eq!(
            parse_lspci_gpu(text).as_deref(),
            Some("GP106M [GeForce GTX 1060 Mobile]")
        );
    }

    #[test]
    fn a_lone_integrated_controller_is_still_reported() {
        let text = "00:02.0 \"VGA compatible controller\" \"Intel Corporation\" \
                    \"UHD Graphics 630\" -p00 \"Vendor\" \"Device\"\n";
        assert_eq!(parse_lspci_gpu(text).as_deref(), Some("UHD Graphics 630"));
        assert_eq!(parse_lspci_gpu("no display controller here\n"), None);
    }
}
