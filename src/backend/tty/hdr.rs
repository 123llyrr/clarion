//! HDR capability, color signaling, and framebuffer transition ownership.
//!
//! Requested HDR is deliberately separate from a successfully committed output.
//! Connector properties travel in the same atomic commit as the matching image.

use std::time::Duration;

use anyhow::{bail, Context};
use clarion_config::output::{MaxBpc, Output as OutputConfig};
use smithay::backend::allocator::Fourcc;
use smithay::backend::drm::{DrmDevice, DrmDeviceFd, DrmSurface};
use smithay::reexports::drm::control::{connector, property, Device};

use super::{find_drm_property, get_edid_info, set_gamma_for_crtc, ConnectorProperties};
use crate::color::{ColorDescription, HdrOutputSettings};

#[derive(Debug, Clone, Default)]
pub(crate) struct HdrCapabilities {
    pub supported: bool,
    pub peak_luminance: Option<f32>,
    pub min_luminance: Option<f32>,
    pub max_fall: Option<f32>,
    pub reason: Option<String>,
    metadata: Option<property::Handle>,
    colorspace: Option<property::Handle>,
    bt2020_rgb: Option<u64>,
    default_colorspace: Option<u64>,
    max_bpc: Option<(property::Handle, u64, u64)>,
    default_max_bpc: Option<u64>,
}

fn named_enum_value<'a>(
    raw: &[u64],
    names: impl IntoIterator<Item = &'a std::ffi::CStr>,
    wanted: &std::ffi::CStr,
) -> Option<u64> {
    names
        .into_iter()
        .position(|name| name == wanted)
        .and_then(|index| raw.get(index).copied())
}

fn positive(value: f32) -> Option<f32> {
    (value.is_finite() && value > 0.0).then_some(value)
}

pub(crate) fn is_hdr_format(format: Fourcc) -> bool {
    matches!(
        format,
        Fourcc::Argb2101010 | Fourcc::Abgr2101010 | Fourcc::Xrgb2101010 | Fourcc::Xbgr2101010
    )
}

impl HdrCapabilities {
    pub fn detect(device: &DrmDevice, connector: connector::Handle) -> Self {
        let mut result = Self::default();
        let properties = ConnectorProperties::try_new(device, connector).ok();
        if let Some(properties) = &properties {
            result.metadata = properties
                .find(c"HDR_OUTPUT_METADATA")
                .ok()
                .and_then(|(info, _)| {
                    matches!(info.value_type(), property::ValueType::Blob).then_some(info.handle())
                });
            if let Ok((info, _)) = properties.find(c"Colorspace") {
                if let property::ValueType::Enum(values) = info.value_type() {
                    result.colorspace = Some(info.handle());
                    let (raws, entries) = values.values();
                    result.default_colorspace = named_enum_value(
                        raws,
                        entries.iter().map(|entry| entry.name()),
                        c"Default",
                    );
                    result.bt2020_rgb = named_enum_value(
                        raws,
                        entries.iter().map(|entry| entry.name()),
                        c"BT2020_RGB",
                    );
                }
            }
            result.default_max_bpc = properties.find(c"max bpc").ok().and_then(|(info, value)| {
                info.value_type().convert_value(*value).as_unsigned_range()
            });
            result.max_bpc = properties.find(c"max bpc").ok().and_then(|(info, _)| {
                if let property::ValueType::UnsignedRange(min, max) = info.value_type() {
                    Some((info.handle(), min, max))
                } else {
                    None
                }
            });
        }
        let edid = get_edid_info(device, connector).ok();
        let (pq, bt2020) = edid
            .as_ref()
            .map(|info| result.read_edid(info))
            .unwrap_or_default();
        result.reason = if !pq {
            Some("display EDID does not advertise PQ HDR10 static metadata".into())
        } else if !bt2020 {
            Some("display EDID does not advertise BT.2020 RGB".into())
        } else if result.metadata.is_none() {
            Some("DRM connector has no HDR_OUTPUT_METADATA blob property".into())
        } else if result.default_colorspace.is_none() {
            Some("DRM connector has no safe Default colorspace".into())
        } else if result.bt2020_rgb.is_none() {
            Some("DRM connector does not support Colorspace BT2020_RGB".into())
        } else if !result.max_bpc.is_some_and(|(_, _, max)| max >= 10) {
            Some("DRM connector cannot establish a 10 bpc transport".into())
        } else {
            None
        };
        result.supported = result.reason.is_none();
        result
    }

    fn read_edid(&mut self, info: &libdisplay_info::info::Info) -> (bool, bool) {
        let mut pq = false;
        let mut bt2020 = false;
        if let Some(edid) = info.edid() {
            for extension in edid.extensions() {
                let Some(cta) = libdisplay_info::cta::CTA::from_extension(extension) else {
                    continue;
                };
                for block in cta.data_blocks() {
                    if let Some(colorimetry) = block.colorimetry() {
                        bt2020 |= colorimetry.bt2020_rgb;
                    }
                    if let Some(metadata) = block.hdr_static_metadata() {
                        pq |= metadata.eotfs.is_some_and(|eotfs| eotfs.pq)
                            && metadata
                                .descriptors
                                .is_some_and(|descriptors| descriptors.type1);
                        self.peak_luminance = metadata
                            .desired_content_max_luminance
                            .and_then(positive)
                            .map(|v| v.clamp(80.0, 10_000.0));
                        self.min_luminance =
                            metadata.desired_content_min_luminance.and_then(positive);
                        self.max_fall = metadata
                            .desired_content_max_frame_avg_luminance
                            .and_then(positive);
                    }
                }
            }
        }
        (pq, bt2020)
    }

    fn settings(&self, config: &OutputConfig) -> HdrOutputSettings {
        let peak_luminance = config
            .hdr_peak_luminance
            .map(|v| v.0 as f32)
            .or(self.peak_luminance)
            .unwrap_or(1000.0)
            .clamp(80.0, 10_000.0);
        let sdr_white = config
            .sdr_white_level
            .map(|v| v.0 as f32)
            .unwrap_or(203.0)
            .clamp(80.0, peak_luminance);
        HdrOutputSettings {
            sdr_white,
            peak_luminance,
            // An implausible EDID black point must not invert the reference
            // range or exceed the HDR10 metadata's 0.0001-nit u16 field.
            min_luminance: self
                .min_luminance
                .filter(|minimum| *minimum < sdr_white && *minimum <= 6.5535)
                .unwrap_or(0.005),
            max_fall: self
                .max_fall
                .unwrap_or(peak_luminance)
                .clamp(0.0, peak_luminance),
        }
    }
}

struct MetadataBlob {
    device: DrmDeviceFd,
    id: u64,
}

impl Drop for MetadataBlob {
    fn drop(&mut self) {
        if let Err(error) = self.device.destroy_property_blob(self.id) {
            warn!("error releasing HDR metadata creation reference: {error}");
        }
    }
}

pub(crate) struct HdrOutputState {
    pub capabilities: HdrCapabilities,
    pub requested: bool,
    pub brightness: f32,
    pub target: Option<HdrOutputSettings>,
    pub active: Option<HdrOutputSettings>,
    pub status: Option<String>,
    configured: Option<(bool, Option<MaxBpc>, HdrOutputSettings, bool, Fourcc, bool)>,
    staged: Option<HdrOutputSettings>,
    staged_valid: bool,
    signaling_confirmed: bool,
    active_blob: Option<MetadataBlob>,
    staged_blob: Option<MetadataBlob>,
    pub needs_stage: bool,
    pub ipc_dirty: bool,
    max_bpc: Option<MaxBpc>,
    pub retry_after: Duration,
    failures: u32,
}

impl HdrOutputState {
    pub fn new(capabilities: HdrCapabilities) -> Self {
        Self {
            capabilities,
            requested: false,
            brightness: 1.0,
            target: None,
            active: None,
            status: None,
            configured: None,
            staged: None,
            staged_valid: false,
            signaling_confirmed: false,
            active_blob: None,
            staged_blob: None,
            needs_stage: true,
            ipc_dirty: false,
            max_bpc: None,
            retry_after: Duration::ZERO,
            failures: 0,
        }
    }

    /// Returns whether an already prepared frame must be invalidated.
    pub fn configure(
        &mut self,
        config: &OutputConfig,
        native: bool,
        format: Fourcc,
        atomic: bool,
    ) -> bool {
        let brightness = config.hdr_brightness.map_or(1.0, |v| (v.0 / 100.0) as f32);
        let brightness = if brightness.is_finite() {
            brightness.clamp(0.01, 1.0)
        } else {
            1.0
        };
        let brightness_changed = self.brightness != brightness;
        self.brightness = brightness;
        self.ipc_dirty |= brightness_changed;
        let settings = self.capabilities.settings(config);
        let configuration = (config.hdr, config.max_bpc, settings, native, format, atomic);
        if self.configured.as_ref() == Some(&configuration) {
            // Brightness changes repaint the framebuffer, not KMS metadata.
            return brightness_changed;
        }
        self.configured = Some(configuration);
        self.signaling_confirmed = false;
        self.requested = config.hdr;
        self.max_bpc = config.max_bpc;
        let bpc_supported = config.max_bpc.is_none_or(|bpc| {
            self.capabilities
                .max_bpc
                .is_none_or(|(_, min, max)| (min..=max).contains(&(bpc.0 as u64)))
        });
        self.status = if !config.hdr {
            Some("disabled".into())
        } else if !self.capabilities.supported {
            self.capabilities.reason.clone()
        } else if !atomic {
            Some("HDR requires atomic KMS".into())
        } else if !native {
            Some("HDR requires the native Vulkan renderer".into())
        } else if !is_hdr_format(format) {
            Some("HDR requires a 10 bit scanout framebuffer".into())
        } else if !bpc_supported {
            Some("configured max-bpc is outside the connector range".into())
        } else if config.max_bpc.is_some_and(|bpc| (bpc.0 as u8) < 10) {
            Some("configured max-bpc is below 10".into())
        } else {
            Some("pending atomic framebuffer commit".into())
        };
        self.target = (config.hdr
            && self.capabilities.supported
            && atomic
            && native
            && is_hdr_format(format)
            && bpc_supported
            && !config.max_bpc.is_some_and(|bpc| (bpc.0 as u8) < 10))
        .then_some(settings);
        self.needs_stage = atomic;
        self.retry_after = Duration::ZERO;
        self.failures = 0;
        self.ipc_dirty = true;
        true
    }

    /// Cleanup also covers an initial or failed transition whose signal state
    /// is not represented by `active`: the connector may predate this session.
    pub fn manages_color_signaling(&self) -> bool {
        self.capabilities.metadata.is_some()
            || self.capabilities.colorspace.is_some()
            || self.target.is_some()
            || self.active.is_some()
            || self.staged_blob.is_some()
            || self.active_blob.is_some()
    }

    fn inactive_signaling_properties(&self) -> Vec<(property::Handle, u64, u64)> {
        let mut properties = Vec::new();
        if let Some(property) = self.capabilities.metadata {
            properties.push((property, 0, 0));
        }
        if let (Some(property), Some(default)) = (
            self.capabilities.colorspace,
            self.capabilities.default_colorspace,
        ) {
            properties.push((property, default, default));
        }
        if let (Some((property, min, max)), Some(default)) =
            (self.capabilities.max_bpc, self.capabilities.default_max_bpc)
        {
            if (min..=max).contains(&default) {
                properties.push((property, default, default));
            }
        }
        properties
    }

    /// Register safe inactive values without creating/releasing metadata blobs
    /// or changing the live connector. `clear()` submits them together with
    /// disabling the CRTC before libseat acknowledges device handoff.
    pub fn prepare_pause(
        &self,
        surface: &DrmSurface,
        connector: connector::Handle,
    ) -> anyhow::Result<()> {
        if surface.is_legacy() {
            return Ok(());
        }
        surface
            .set_connector_properties(connector, &self.inactive_signaling_properties())
            .context("staging inactive color signaling for seat handoff")
    }

    pub fn configured_settings(&self) -> Option<HdrOutputSettings> {
        self.configured
            .as_ref()
            .map(|configuration| configuration.2)
    }

    pub fn description(&self) -> ColorDescription {
        self.target
            .map(|settings| settings.description())
            .unwrap_or_default()
    }

    pub fn gamma_allowed(&self) -> bool {
        self.target.is_none() && self.active.is_none() && self.active_blob.is_none()
    }

    /// Cursor pixels bypass the output transform on their hardware plane.
    /// Allow SDR cursor scanout only after the matching real SDR signal commit;
    /// resume, a pending transition, and retained HDR blobs remain composited.
    pub fn hardware_cursor_allowed(&self) -> bool {
        self.signaling_confirmed
            && self.target.is_none()
            && self.active.is_none()
            && self.active_blob.is_none()
            && self.staged_blob.is_none()
            && !self.transition_pending()
    }

    pub fn transition_pending(&self) -> bool {
        self.needs_stage || self.staged_valid || self.staged != self.active
    }

    /// Called only after all older queued/in-flight frames have drained.
    pub fn stage(
        &mut self,
        device: &DrmDevice,
        surface: &DrmSurface,
        connector: connector::Handle,
    ) -> anyhow::Result<()> {
        if !self.needs_stage {
            return Ok(());
        }
        if surface.is_legacy() {
            if self.target.is_some() {
                bail!("HDR requires atomic KMS");
            }
            self.needs_stage = false;
            return Ok(());
        }
        let mut properties = Vec::new();
        let capabilities = &self.capabilities;
        let blob = if let Some(settings) = self.target {
            if !capabilities.supported {
                bail!("HDR is unsupported");
            }
            let metadata = crate::render_helpers::vulkan::hdr::hdr10_metadata_blob_for(
                settings.peak_luminance,
                settings.min_luminance,
                settings.max_fall,
            );
            let id = device
                .create_property_blob(&metadata)
                .context("creating HDR10 metadata")?
                .into();
            Some(MetadataBlob {
                device: surface.device_fd().clone(),
                id,
            })
        } else {
            None
        };
        if let Some(property) = capabilities.metadata {
            properties.push((property, blob.as_ref().map_or(0, |blob| blob.id), 0));
        }
        if let (Some(property), Some(default)) =
            (capabilities.colorspace, capabilities.default_colorspace)
        {
            properties.push((
                property,
                if self.target.is_some() {
                    capabilities.bt2020_rgb.unwrap()
                } else {
                    default
                },
                default,
            ));
        }
        if let Some((property, min, max)) = capabilities.max_bpc {
            let default = if self.target.is_some() {
                10.max(min)
            } else {
                capabilities.default_max_bpc.unwrap_or(max.min(10))
            };
            let requested = self
                .max_bpc
                .map(|bpc| bpc.0 as u64)
                .filter(|value| (min..=max).contains(value))
                .unwrap_or(default);
            if !(min..=max).contains(&requested) {
                bail!("invalid connector max-bpc range");
            }
            properties.push((
                property,
                requested,
                capabilities.default_max_bpc.unwrap_or(requested),
            ));
        }
        if !properties.is_empty() {
            surface
                .set_connector_properties(connector, &properties)
                .context("staging connector color properties")?;
        }
        let mut crtc_properties = Vec::new();
        if self.target.is_some() {
            for name in ["DEGAMMA_LUT", "CTM"] {
                if let Some((property, info, _)) = find_drm_property(device, surface.crtc(), name) {
                    if !matches!(info.value_type(), property::ValueType::Blob) {
                        bail!("{name} has invalid property type");
                    }
                    crtc_properties.push((property, 0));
                }
            }
            if let Some((property, info, _)) =
                find_drm_property(device, surface.crtc(), "GAMMA_LUT")
            {
                if !matches!(info.value_type(), property::ValueType::Blob) {
                    bail!("GAMMA_LUT has invalid property type");
                }
                crtc_properties.push((property, 0));
            } else {
                // A legacy gamma ramp must not survive into PQ. Clear it before
                // the matching HDR frame if this CRTC has only the old API.
                let gamma_length = device.get_crtc(surface.crtc())?.gamma_length();
                if gamma_length > 0 {
                    set_gamma_for_crtc(device, surface.crtc(), None)
                        .context("resetting legacy gamma before HDR")?;
                }
            }
        }
        if !surface.is_legacy() {
            surface
                .set_crtc_properties(&crtc_properties)
                .context("staging color-correct CRTC gamma")?;
        }
        self.staged_blob = blob;
        self.staged = self.target;
        self.staged_valid = true;
        self.needs_stage = false;
        Ok(())
    }

    /// Refresh from Smithay's last successful real atomic commit, never a request.
    pub fn refresh_active(&mut self, surface: &DrmSurface, connector: connector::Handle) {
        if !self.staged_valid {
            return;
        }
        let Some(metadata_property) = self.capabilities.metadata else {
            // SDR-only connectors have no metadata to inspect. Staging is only
            // done after older frames drain, so queue success is a real commit.
            self.staged_valid = false;
            self.signaling_confirmed = self.staged.is_none();
            self.failures = 0;
            self.retry_after = Duration::ZERO;
            return;
        };
        let expected = self.staged_blob.as_ref().map_or(0, |blob| blob.id);
        if surface.current_connector_property(connector, metadata_property) != Some(expected) {
            return;
        }
        if self.staged.is_some() {
            let Some(colorspace) = self.capabilities.colorspace else {
                return;
            };
            if surface.current_connector_property(connector, colorspace)
                != self.capabilities.bt2020_rgb
            {
                return;
            }
        }
        let changed = self.active != self.staged;
        self.active = self.staged;
        self.signaling_confirmed = true;
        self.active_blob = self
            .staged_blob
            .take()
            .or_else(|| self.active_blob.take().filter(|blob| blob.id == expected));
        self.staged_valid = false;
        self.failures = 0;
        self.retry_after = Duration::ZERO;
        if self.active.is_some()
            && self
                .status
                .as_ref()
                .is_none_or(|status| status.starts_with("pending"))
        {
            self.status = Some("active".into());
        } else if self.active.is_none()
            && self.status.as_ref().is_some_and(|status| {
                status.starts_with("pending SDR framebuffer commit after resume")
            })
        {
            self.status = Some(if self.requested {
                "SDR active; requested HDR unavailable".into()
            } else {
                "disabled".into()
            });
        }
        if changed {
            self.ipc_dirty = true;
        }
    }

    /// A failed test/commit never changes the renderer to an uncommitted target.
    pub fn reject_transition(&mut self, error: &str) {
        if !self.transition_pending() {
            return;
        }
        // Turning HDR off must remain an SDR transition if the previous HDR
        // signal cannot be cleared. Keep the old framebuffer on screen and
        // retry the matching SDR atomic commit; never send SDR under PQ.
        if self.target.is_some() {
            self.target = self.active;
        }
        self.staged = self.active;
        self.needs_stage = true;
        self.staged_valid = false;
        self.failures = self.failures.saturating_add(1);
        self.retry_after =
            crate::utils::get_monotonic_time() + transition_retry_delay(self.failures);
        self.status = Some(format!("atomic HDR transition rejected: {error}"));
        self.ipc_dirty = true;
    }

    pub fn resume(&mut self) {
        self.signaling_confirmed = false;
        self.retry_after = Duration::ZERO;
        self.failures = 0;
        self.needs_stage = self
            .configured
            .as_ref()
            .is_none_or(|configuration| configuration.5);
        self.staged_valid = false;
        self.active = None;
        self.ipc_dirty = true;
        if self.target.is_some() {
            self.status = Some("pending atomic framebuffer commit after resume".into());
        } else if self.status.as_ref().is_some_and(|status| {
            status.starts_with("native Vulkan renderer unavailable") || status.starts_with("seat ")
        }) {
            self.status = Some(if self.requested {
                "pending SDR framebuffer commit after resume; requested HDR unavailable".into()
            } else {
                "pending SDR framebuffer commit after resume".into()
            });
        }
    }
}

fn transition_retry_delay(failures: u32) -> Duration {
    Duration::from_millis(250 * (1u64 << failures.saturating_sub(1).min(2)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capable() -> HdrCapabilities {
        HdrCapabilities {
            supported: true,
            peak_luminance: Some(616.884),
            min_luminance: Some(0.005),
            max_fall: Some(400.0),
            ..Default::default()
        }
    }

    #[test]
    fn pause_cleanup_covers_unrequested_and_uncommitted_hdr_signaling() {
        let handle = |id| property::Handle::from(std::num::NonZeroU32::new(id).unwrap());
        let caps = HdrCapabilities {
            metadata: Some(handle(1)),
            colorspace: Some(handle(2)),
            default_colorspace: Some(42),
            max_bpc: Some((handle(3), 8, 12)),
            default_max_bpc: Some(8),
            ..Default::default()
        };
        let state = HdrOutputState::new(caps);
        assert!(!state.requested && state.active.is_none());
        assert!(
            state.manages_color_signaling(),
            "a prior session's metadata can still be live"
        );
        assert_eq!(
            state.inactive_signaling_properties(),
            vec![(handle(1), 0, 0), (handle(2), 42, 42), (handle(3), 8, 8),]
        );
    }

    #[test]
    fn sdr_default_colorspace_is_found_without_rgb_hdr_support() {
        let names = [c"BT2020_YCC", c"Default"];
        let values = [7, 42];
        assert_eq!(named_enum_value(&values, names, c"Default"), Some(42));
        assert_eq!(named_enum_value(&values, names, c"BT2020_RGB"), None);
    }

    #[test]
    fn cta_edid_decodes_hdr10_capabilities_and_actual_luminance() {
        // SDC ATNA60YV06-0's public signal descriptors, with an anonymized
        // synthetic EDID base block. CTA values encode 616.884/400/.00465 nits.
        let mut bytes = [0u8; 256];
        bytes[..8].copy_from_slice(&[0, 255, 255, 255, 255, 255, 255, 0]);
        bytes[8..12].copy_from_slice(&[0x4c, 0x83, 0x73, 0x41]);
        bytes[18] = 1;
        bytes[19] = 4;
        bytes[20] = 0xb5;
        bytes[23] = 120;
        bytes[126] = 1;
        bytes[127] = 0u8.wrapping_sub(
            bytes[..127]
                .iter()
                .fold(0u8, |sum, value| sum.wrapping_add(*value)),
        );
        bytes[128..143]
            .copy_from_slice(&[2, 3, 15, 0, 0xe3, 5, 0x80, 0, 0xe6, 6, 5, 1, 116, 96, 7]);
        bytes[255] = 0u8.wrapping_sub(
            bytes[128..255]
                .iter()
                .fold(0u8, |sum, value| sum.wrapping_add(*value)),
        );
        let info = libdisplay_info::info::Info::parse_edid(&bytes).unwrap();
        let mut caps = HdrCapabilities::default();
        assert_eq!(caps.read_edid(&info), (true, true));
        assert!((caps.peak_luminance.unwrap() - 616.884).abs() < 0.01);
        assert_eq!(caps.max_fall, Some(400.0));
        assert!(caps.min_luminance.unwrap() < 0.01);
        // PQ absence must not be inferred merely from the presence of a blob.
        bytes[138] = 1;
        bytes[255] = 0u8.wrapping_sub(
            bytes[128..255]
                .iter()
                .fold(0u8, |sum, value| sum.wrapping_add(*value)),
        );
        let info = libdisplay_info::info::Info::parse_edid(&bytes).unwrap();
        assert_eq!(caps.read_edid(&info), (false, true));
    }

    #[test]
    fn hdr_target_requires_real_renderer_transport_and_framebuffer() {
        let config = OutputConfig {
            hdr: true,
            ..Default::default()
        };
        for (native, format, atomic, allowed) in [
            (true, Fourcc::Abgr2101010, true, true),
            (true, Fourcc::Abgr8888, true, false),
            (false, Fourcc::Abgr2101010, true, false),
            (true, Fourcc::Abgr2101010, false, false),
        ] {
            let mut state = HdrOutputState::new(capable());
            state.configure(&config, native, format, atomic);
            assert_eq!(state.target.is_some(), allowed);
            assert!(state.active.is_none());
            if !atomic {
                assert!(
                    !state.needs_stage,
                    "legacy SDR must not wait for atomic staging"
                );
            }
        }
    }

    #[test]
    fn capability_failure_and_explicit_eight_bpc_keep_sdr() {
        let mut state = HdrOutputState::new(HdrCapabilities::default());
        let mut config = OutputConfig {
            hdr: true,
            ..Default::default()
        };
        state.configure(&config, true, Fourcc::Abgr2101010, true);
        assert!(state.target.is_none());
        state = HdrOutputState::new(capable());
        config.max_bpc = Some(MaxBpc(clarion_ipc::MaxBpc::_8));
        state.configure(&config, true, Fourcc::Abgr2101010, true);
        assert!(state.target.is_none());
        assert!(state.status.unwrap().contains("below 10"));
    }

    #[test]
    fn configured_depth_outside_connector_range_does_not_arm_hdr() {
        let mut caps = capable();
        let property = property::Handle::from(std::num::NonZeroU32::new(1).unwrap());
        caps.max_bpc = Some((property, 8, 10));
        let mut state = HdrOutputState::new(caps);
        let config = OutputConfig {
            hdr: true,
            max_bpc: Some(MaxBpc(clarion_ipc::MaxBpc::_16)),
            ..Default::default()
        };
        state.configure(&config, true, Fourcc::Abgr2101010, true);
        assert!(state.target.is_none());
        assert!(state.status.unwrap().contains("outside"));
    }

    #[test]
    fn edid_luminance_is_used_and_white_never_exceeds_peak() {
        let mut state = HdrOutputState::new(capable());
        let mut config = OutputConfig {
            hdr: true,
            ..Default::default()
        };
        state.configure(&config, true, Fourcc::Abgr2101010, true);
        let target = state.target.unwrap();
        assert!((target.peak_luminance - 616.884).abs() < 0.01);
        assert_eq!(target.max_fall, 400.0);
        config.hdr_peak_luminance = Some(clarion_config::FloatOrInt(100.0));
        state.configure(&config, true, Fourcc::Abgr2101010, true);
        assert_eq!(state.target.unwrap().sdr_white, 100.0);
    }

    #[test]
    fn malformed_edid_black_level_keeps_output_description_valid() {
        let caps = HdrCapabilities {
            min_luminance: Some(300.0),
            ..capable()
        };
        let config = OutputConfig {
            hdr: true,
            sdr_white_level: Some(clarion_config::FloatOrInt(80.0)),
            ..Default::default()
        };
        let settings = caps.settings(&config);
        assert_eq!(settings.min_luminance, 0.005);
        assert!(settings.description().validate());
    }

    #[test]
    fn hardware_cursor_requires_confirmed_sdr_and_stays_composited_through_hdr_transitions() {
        let mut state = HdrOutputState::new(capable());
        assert!(
            !state.hardware_cursor_allowed(),
            "initial signal state is unknown"
        );
        state.configure(&OutputConfig::default(), true, Fourcc::Abgr2101010, true);
        assert!(
            !state.hardware_cursor_allowed(),
            "SDR has not been committed yet"
        );
        state.needs_stage = false;
        state.staged_valid = true;
        assert!(
            !state.hardware_cursor_allowed(),
            "a staged request is not applied"
        );
        state.staged_valid = false;
        state.signaling_confirmed = true;
        assert!(state.hardware_cursor_allowed());

        let config = OutputConfig {
            hdr: true,
            ..Default::default()
        };
        state.configure(&config, true, Fourcc::Abgr2101010, true);
        assert!(
            !state.hardware_cursor_allowed(),
            "arming HDR disables the cursor plane first"
        );
        state.needs_stage = false;
        state.active = state.target;
        state.signaling_confirmed = true;
        assert!(!state.hardware_cursor_allowed());
        state.configure(&OutputConfig::default(), true, Fourcc::Abgr2101010, true);
        assert!(
            !state.hardware_cursor_allowed(),
            "old PQ signal survives the SDR request"
        );
        state.active = None;
        state.needs_stage = false;
        state.staged = None;
        state.staged_valid = false;
        state.signaling_confirmed = true;
        assert!(state.hardware_cursor_allowed());
        state.resume();
        assert!(
            !state.hardware_cursor_allowed(),
            "VT resume must reconfirm signal state"
        );
    }

    #[test]
    fn rejected_transition_cannot_enable_hardware_cursor_before_sdr_recommit() {
        let mut state = HdrOutputState::new(capable());
        state.configure(
            &OutputConfig {
                hdr: true,
                ..Default::default()
            },
            true,
            Fourcc::Abgr2101010,
            true,
        );
        state.reject_transition("TEST_ONLY failed");
        assert!(state.target.is_none());
        assert!(
            !state.hardware_cursor_allowed(),
            "HDR failure still needs safe SDR staging"
        );
    }

    #[test]
    fn gamma_adjustment_is_rejected_through_both_hdr_transition_directions() {
        let mut state = HdrOutputState::new(capable());
        assert!(state.gamma_allowed());
        let config = OutputConfig {
            hdr: true,
            ..Default::default()
        };
        state.configure(&config, true, Fourcc::Abgr2101010, true);
        assert!(!state.gamma_allowed());
        state.active = state.target;
        state.configure(&OutputConfig::default(), true, Fourcc::Abgr2101010, true);
        assert!(
            !state.gamma_allowed(),
            "old PQ framebuffer remains until SDR atomic commit"
        );
        state.active = None;
        assert!(state.gamma_allowed());
    }

    #[test]
    fn failed_atomic_transitions_have_a_bounded_retry_delay() {
        assert_eq!(transition_retry_delay(1), Duration::from_millis(250));
        assert_eq!(transition_retry_delay(2), Duration::from_millis(500));
        assert_eq!(transition_retry_delay(3), Duration::from_secs(1));
        assert_eq!(transition_retry_delay(u32::MAX), Duration::from_secs(1));
    }

    #[test]
    fn failed_transition_rolls_back_without_claiming_hdr_active() {
        let mut state = HdrOutputState::new(capable());
        let config = OutputConfig {
            hdr: true,
            ..Default::default()
        };
        state.configure(&config, true, Fourcc::Abgr2101010, true);
        state.reject_transition("TEST_ONLY failed");
        assert!(state.active.is_none());
        assert!(state.target.is_none());
        assert!(state.status.as_ref().unwrap().contains("TEST_ONLY"));
        assert!(!state.configure(&config, true, Fourcc::Abgr2101010, true));
        assert!(
            state.target.is_none(),
            "unchanged config must not loop failing HDR commits"
        );
    }
    #[test]
    fn brightness_repaints_without_replacing_hdr_signaling_or_reference_white() {
        let mut state = HdrOutputState::new(capable());
        let mut config = OutputConfig {
            hdr: true,
            ..Default::default()
        };
        assert!(state.configure(&config, true, Fourcc::Abgr2101010, true));
        state.active = state.target;
        state.signaling_confirmed = true;
        state.needs_stage = false;
        let settings = state.target;
        config.hdr_brightness = Some(clarion_config::FloatOrInt(50.0));
        assert!(state.configure(&config, true, Fourcc::Abgr2101010, true));
        assert_eq!(state.brightness, 0.5);
        assert_eq!(state.target, settings);
        assert_eq!(state.active, settings);
        assert!(state.signaling_confirmed);
        assert!(!state.needs_stage);
        assert!(state.ipc_dirty);
        assert!(!state.configure(&config, true, Fourcc::Abgr2101010, true));
        config.hdr_brightness = Some(clarion_config::FloatOrInt(100.0));
        assert!(state.configure(&config, true, Fourcc::Abgr2101010, true));
        assert_eq!(state.brightness, 1.0);
        assert!(!state.needs_stage);
    }
}
