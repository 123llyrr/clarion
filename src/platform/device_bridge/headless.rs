//! Headless backend for tests.
//!
//! The Vulkan mode uses a real offscreen target and advertises DMA-BUF formats.
//! The GLES test mode keeps its lightweight callback-only rendering behavior.

#[cfg(test)]
use clarion_config::OutputName;
use smithay::backend::renderer::ImportDma;
#[cfg(test)]
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
#[cfg(test)]
use smithay::utils::Size;

use crate::backend::headless::{Headless, HeadlessRenderer};
#[cfg(test)]
use crate::backend::OutputId;
use crate::compositor::Clarion;
#[cfg(test)]
use crate::utils::logical_output;

impl Headless {
    fn platform_init(&mut self, clarion: &mut Clarion) {
        if let Some(HeadlessRenderer::Native(renderer)) = self.renderer.as_mut() {
            if let Err(error) = super::install_native_shader_wakeup(renderer, &clarion.event_loop) {
                warn!("error installing headless native shader completion: {error:#}");
            }
            if let Err(err) = renderer.install_completion_wakeup_with_callbacks(
                &clarion.event_loop,
                |sample, state| state.clarion.record_gpu_timing(sample),
                crate::compositor::State::on_native_gpu_completion,
            ) {
                warn!("error installing headless Vulkan completion wakeup: {err}");
            }
            // No DRM node is selected for headless rendering, so advertise the
            // supported formats through the v3 global without device feedback.
            self.dmabuf_global = Some(
                clarion
                    .dmabuf_state
                    .create_global::<crate::compositor::State>(
                        &clarion.display_handle,
                        renderer.dmabuf_formats(),
                    ),
            );
        }
    }

    #[cfg(test)]
    fn platform_add_output(&mut self, clarion: &mut Clarion, n: u8, size: (u16, u16)) {
        let connector = format!("headless-{n}");
        self.native_targets.retain(|name, _| {
            clarion
                .global_space
                .outputs()
                .any(|output| output.name() == *name)
        });
        // Reusing a connector name must not reuse the previous output's damage
        // history or render target.
        self.native_targets.remove(&connector);
        let make = "clarion".to_string();
        let model = "headless".to_string();
        let serial = n.to_string();

        let output = Output::new(
            connector.clone(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: make.clone(),
                model: model.clone(),
                serial_number: serial.clone(),
            },
        );

        let mode = Mode {
            size: Size::from((i32::from(size.0), i32::from(size.1))),
            refresh: 60_000,
        };
        output.change_current_state(Some(mode), None, None, None);
        output.set_preferred(mode);

        output.user_data().insert_if_missing(|| OutputName {
            connector,
            make: Some(make),
            model: Some(model),
            serial: Some(serial),
        });

        let physical_properties = output.physical_properties();
        self.ipc_outputs.lock().unwrap().insert(
            OutputId::next(),
            clarion_ipc::Output {
                name: output.name(),
                make: physical_properties.make,
                model: physical_properties.model,
                serial: None,
                physical_size: None,
                modes: vec![clarion_ipc::Mode {
                    width: size.0,
                    height: size.1,
                    refresh_rate: 60_000,
                    is_preferred: true,
                }],
                current_mode: Some(0),
                is_custom_mode: true,
                vrr_supported: false,
                vrr_enabled: false,
                logical: Some(logical_output(&output)),
                max_bpc: None,
                hdr_active: false,
                hdr_supported: false,
                hdr_status: Some(
                    "HDR physical output requires the native Vulkan KMS backend".into(),
                ),
                hdr_peak_luminance: None,
                sdr_white_level: None,
                hdr_brightness: None,
            },
        );

        clarion.add_output(output, None, false, false);
    }
}
pub(super) fn initialize(device: &mut Headless, clarion: &mut Clarion) {
    device.platform_init(clarion);
}
#[cfg(test)]
pub(super) fn add_output(
    device: &mut Headless,
    clarion: &mut Clarion,
    number: u8,
    size: (u16, u16),
) {
    device.platform_add_output(clarion, number, size);
}
