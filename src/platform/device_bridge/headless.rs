//! Headless backend for tests.
//!
//! The Vulkan mode uses a real offscreen target and advertises DMA-BUF formats.
//! The GLES test mode keeps its lightweight callback-only rendering behavior.

use smithay::backend::renderer::ImportDma;

use crate::backend::headless::{Headless, HeadlessRenderer};
use crate::compositor::Realand;

#[cfg(test)]
use crate::backend::OutputId;
#[cfg(test)]
use crate::utils::logical_output;
#[cfg(test)]
use realand_config::OutputName;
#[cfg(test)]
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
#[cfg(test)]
use smithay::utils::Size;

impl Headless {
    fn platform_init(&mut self, realand: &mut Realand) {
        if let Some(HeadlessRenderer::Native(renderer)) = self.renderer.as_mut() {
            if let Err(error) = super::install_native_shader_wakeup(renderer, &realand.event_loop) {
                warn!("error installing headless native shader completion: {error:#}");
            }
            if let Err(err) = renderer.install_completion_wakeup_with_callbacks(
                &realand.event_loop,
                |sample, state| state.realand.record_gpu_timing(sample),
                crate::compositor::State::on_native_gpu_completion,
            ) {
                warn!("error installing headless Vulkan completion wakeup: {err}");
            }
            // No DRM node is selected for headless rendering, so advertise the
            // supported formats through the v3 global without device feedback.
            self.dmabuf_global = Some(
                realand
                    .dmabuf_state
                    .create_global::<crate::compositor::State>(
                        &realand.display_handle,
                        renderer.dmabuf_formats(),
                    ),
            );
        }
    }

    #[cfg(test)]
    fn platform_add_output(&mut self, realand: &mut Realand, n: u8, size: (u16, u16)) {
        let connector = format!("headless-{n}");
        self.native_targets.retain(|name, _| {
            realand
                .global_space
                .outputs()
                .any(|output| output.name() == *name)
        });
        // Reusing a connector name must not reuse the previous output's damage
        // history or render target.
        self.native_targets.remove(&connector);
        let make = "realand".to_string();
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
            realand_ipc::Output {
                name: output.name(),
                make: physical_properties.make,
                model: physical_properties.model,
                serial: None,
                physical_size: None,
                modes: vec![realand_ipc::Mode {
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
            },
        );

        realand.add_output(output, None, false, false);
    }
}
pub(super) fn initialize(device: &mut Headless, realand: &mut Realand) {
    device.platform_init(realand);
}
#[cfg(test)]
pub(super) fn add_output(
    device: &mut Headless,
    realand: &mut Realand,
    number: u8,
    size: (u16, u16),
) {
    device.platform_add_output(realand, number, size);
}
