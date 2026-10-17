//! Vulkan device objects only. No renderer, Wayland, scene or compositor state crosses this cut.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use ash::vk;
use calloop::channel::Channel;

use super::{gpu_provider, PreparedShader, Program};
use crate::render_helpers::vulkan::device::VulkanContext;
use crate::render_helpers::vulkan::pipeline::YuvPipeline;
use crate::render_helpers::vulkan::VulkanPipelineManager;

pub(crate) const YUV_PREPARATION_KIND: u8 = u8::MAX - 1;
pub(crate) const OFFSCREEN_PREPARATION_KIND: u8 = u8::MAX - 3;

// Ash Display contains prose; the owner needs the stable Vulkan enum identity
// to distinguish transient allocation pressure from an invalid/lost device.
fn offscreen_vk_error(error: vk::Result) -> String {
    format!("{error:?}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OffscreenKey {
    pub width: u32,
    pub height: u32,
    pub format: vk::Format,
    pub alpha: bool,
}

const MAX_OFFSCREEN_PREPARATION_BYTES: u64 = 256 * 1024 * 1024;
static OFFSCREEN_PREPARATION_BYTES: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

pub(crate) struct OffscreenBudget(u64);
impl OffscreenBudget {
    pub fn reserve(key: OffscreenKey) -> Result<Self, String> {
        let estimate = u64::from(key.width)
            .checked_mul(u64::from(key.height))
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| "offscreen allocation size overflow".to_owned())?;
        let mut budget = Self(0);
        budget.grow_to(estimate)?;
        Ok(budget)
    }
    fn grow_to(&mut self, bytes: u64) -> Result<(), String> {
        let additional = bytes.saturating_sub(self.0);
        OFFSCREEN_PREPARATION_BYTES
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current
                    .checked_add(additional)
                    .filter(|total| *total <= MAX_OFFSCREEN_PREPARATION_BYTES)
            })
            .map_err(|_| "offscreen preparation byte budget exhausted".to_owned())?;
        self.0 += additional;
        Ok(())
    }
}
impl Drop for OffscreenBudget {
    fn drop(&mut self) {
        OFFSCREEN_PREPARATION_BYTES.fetch_sub(self.0, Ordering::AcqRel);
    }
}

pub(crate) struct PreparedOffscreen {
    pub key: OffscreenKey,
    pub(in crate::render_helpers::vulkan) image: Arc<super::super::TextureGpu>,
    _budget: OffscreenBudget,
}

pub(crate) enum Work {
    Build {
        ctx: Arc<VulkanContext>,
        prepared: PreparedShader,
        formats: Vec<vk::Format>,
    },
    Rewarm {
        program: Arc<Program>,
        formats: Vec<vk::Format>,
    },
    Formats {
        manager: Arc<VulkanPipelineManager>,
        programs: Vec<Arc<Program>>,
        formats: Vec<vk::Format>,
    },
    YuvTargets {
        targets: Vec<(Arc<YuvPipeline>, Vec<vk::Format>)>,
    },
    Offscreen {
        ctx: Arc<VulkanContext>,
        key: OffscreenKey,
        budget: OffscreenBudget,
    },
    #[cfg(test)]
    Gate {
        entered: std::sync::mpsc::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
    },
}
pub(crate) enum Prepared {
    Program {
        program: Arc<Program>,
        formats: Vec<vk::Format>,
    },
    Formats {
        formats: Vec<vk::Format>,
        failed: Vec<(vk::Format, String)>,
    },
    YuvTargets {
        failed: Vec<(vk::Format, vk::Result)>,
    },
    Offscreen(PreparedOffscreen),
}
pub(crate) type Event = gpu_provider::Event<Work, Prepared>;
pub(crate) type Endpoint = gpu_provider::Endpoint<Work, Prepared>;
pub(crate) use gpu_provider::Identity;

fn prepare(work: Work, cancel: &AtomicBool) -> Result<Prepared, String> {
    #[cfg(test)]
    let start = std::time::Instant::now();
    #[cfg(test)]
    let cpu_start = thread_cpu();
    let check = || {
        if cancel.load(Ordering::Acquire) {
            Err("GPU preparation cancelled".to_owned())
        } else {
            Ok(())
        }
    };
    check()?;
    let (result, scope) = match work {
        Work::Build {
            ctx,
            prepared,
            formats,
        } => {
            let program =
                Program::from_prepared(ctx, prepared).map_err(|error| error.to_string())?;
            for &format in &formats {
                check()?;
                program
                    .prepare_format(format)
                    .map_err(|error| error.to_string())?;
            }
            (
                Prepared::Program { program, formats },
                "new_program_and_known_formats",
            )
        }
        Work::Rewarm { program, formats } => {
            for &format in &formats {
                check()?;
                program
                    .prepare_format(format)
                    .map_err(|error| error.to_string())?;
            }
            (
                Prepared::Program { program, formats },
                "program_format_growth",
            )
        }
        Work::Formats {
            manager,
            programs,
            formats,
        } => {
            let (formats, failed) = prepare_formats(formats, |format| {
                // Cancellation stays cooperative between driver calls; an executing
                // driver operation retains its one bounded worker slot until it returns.
                if cancel.load(Ordering::Acquire) {
                    return Err(vk::Result::ERROR_UNKNOWN);
                }
                manager.get_pipelines(format)?;
                for program in &programs {
                    program.prepare_format(format)?;
                }
                Ok(())
            })?;
            check()?;
            (
                Prepared::Formats { formats, failed },
                "base_and_cached_fallback_format_growth",
            )
        }
        Work::YuvTargets { targets } => {
            let mut failed = Vec::new();
            for (variant, formats) in targets {
                for format in formats {
                    check()?;
                    match variant.prepare_format(format) {
                        Ok(_) => {}
                        Err(vk::Result::ERROR_DEVICE_LOST) => {
                            return Err("ERROR_DEVICE_LOST".into())
                        }
                        Err(error) => failed.push((format, error)),
                    }
                }
            }
            (
                Prepared::YuvTargets { failed },
                "yuv_variant_target_formats",
            )
        }
        Work::Offscreen {
            ctx,
            key,
            mut budget,
        } => {
            let usage = vk::ImageUsageFlags::COLOR_ATTACHMENT
                | vk::ImageUsageFlags::SAMPLED
                | vk::ImageUsageFlags::TRANSFER_SRC
                | vk::ImageUsageFlags::TRANSFER_DST;
            let limits = unsafe {
                ctx.instance.get_physical_device_image_format_properties(
                    ctx.physical_device,
                    key.format,
                    vk::ImageType::TYPE_2D,
                    vk::ImageTiling::OPTIMAL,
                    usage,
                    vk::ImageCreateFlags::empty(),
                )
            }
            .map_err(offscreen_vk_error)?;
            if key.width > limits.max_extent.width || key.height > limits.max_extent.height {
                return Err("unsupported Vulkan target dimensions".into());
            }
            check()?;
            let texture = super::super::allocate_texture(
                &ctx,
                key.width,
                key.height,
                key.format,
                vk::ImageTiling::OPTIMAL,
                usage,
                vk::ImageLayout::UNDEFINED,
                key.alpha,
            )
            .map_err(offscreen_vk_error)?;
            // Account for driver tiling/alignment padding before publication.
            budget.grow_to(texture.inner.allocation_size)?;
            check()?;
            (
                Prepared::Offscreen(PreparedOffscreen {
                    key,
                    image: texture.inner,
                    _budget: budget,
                }),
                "offscreen_allocation",
            )
        }
        #[cfg(test)]
        Work::Gate { entered, release } => {
            entered.send(()).map_err(|error| error.to_string())?;
            release.recv().map_err(|error| error.to_string())?;
            check()?;
            (
                Prepared::YuvTargets { failed: Vec::new() },
                "test_worker_gate",
            )
        }
    };
    #[cfg(test)]
    eprintln!(
        "GPU_PREPARATION_WORK scope={scope} worker_wall_ms={} worker_thread_cpu_ms={:?}",
        start.elapsed().as_secs_f64() * 1000.,
        thread_cpu()
            .zip(cpu_start)
            .and_then(|(end, start)| end.checked_sub(start))
            .map(|value| value.as_secs_f64() * 1000.)
    );
    #[cfg(not(test))]
    let _ = scope;
    Ok(result)
}
#[cfg(test)]
pub(crate) fn thread_cpu() -> Option<std::time::Duration> {
    let mut value = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: clock_gettime writes only this owned timespec.
    if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut value) } != 0
        || value.tv_sec < 0
        || !(0..1_000_000_000).contains(&value.tv_nsec)
    {
        return None;
    }
    Some(std::time::Duration::new(
        value.tv_sec as u64,
        value.tv_nsec as u32,
    ))
}
pub(crate) fn endpoint() -> Result<(Endpoint, Channel<Event>), String> {
    static SERVICE: super::super::lifecycle::ServiceCell<gpu_provider::Service<Work, Prepared>> =
        super::super::lifecycle::ServiceCell::new();
    SERVICE
        .get_or_try_init(|| gpu_provider::Service::new(prepare))
        .map(|service| service.endpoint())
        .map_err(|error| error.to_string())
}

/// Prepared attachment formats and individual format failures.
type PreparedFormats<T> = (Vec<T>, Vec<(T, String)>);

/// Failure of one attachment format does not discard successful sibling preparation.
/// A lost device invalidates every format and must enter the existing recovery path.
fn prepare_formats<T: Copy>(
    formats: Vec<T>,
    mut prepare: impl FnMut(T) -> Result<(), vk::Result>,
) -> Result<PreparedFormats<T>, String> {
    let mut ready = Vec::with_capacity(formats.len());
    let mut failed = Vec::new();
    for format in formats {
        match prepare(format) {
            Ok(()) => ready.push(format),
            Err(vk::Result::ERROR_DEVICE_LOST) => {
                return Err(format!("{:?}", vk::Result::ERROR_DEVICE_LOST))
            }
            Err(error) => failed.push((format, error.to_string())),
        }
    }
    Ok((ready, failed))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn offscreen_vk_errors_keep_real_oom_and_device_loss_identity() {
        for (error, marker) in [
            (vk::Result::ERROR_OUT_OF_HOST_MEMORY, "OUT_OF_HOST_MEMORY"),
            (
                vk::Result::ERROR_OUT_OF_DEVICE_MEMORY,
                "OUT_OF_DEVICE_MEMORY",
            ),
            (vk::Result::ERROR_DEVICE_LOST, "DEVICE_LOST"),
        ] {
            let transferred = offscreen_vk_error(error);
            assert!(transferred.contains(marker), "{error:?}: {transferred}");
        }
    }
    #[test]
    fn rejected_format_keeps_supported_siblings_and_failure_identity() {
        let mut visited = Vec::new();
        let (ready, failed) = prepare_formats(vec![1, 2, 3], |format| {
            visited.push(format);
            if format == 2 {
                Err(vk::Result::ERROR_FORMAT_NOT_SUPPORTED)
            } else {
                Ok(())
            }
        })
        .unwrap();
        assert_eq!(visited, [1, 2, 3]);
        assert_eq!(ready, [1, 3]);
        assert_eq!(
            failed,
            [(2, vk::Result::ERROR_FORMAT_NOT_SUPPORTED.to_string())]
        );
    }
    #[test]
    fn lost_device_stops_preparation_instead_of_becoming_format_negative_cache() {
        let mut visited = Vec::new();
        let result = prepare_formats(vec![1, 2, 3], |format| {
            visited.push(format);
            if format == 2 {
                Err(vk::Result::ERROR_DEVICE_LOST)
            } else {
                Ok(())
            }
        });
        assert_eq!(visited, [1, 2]);
        assert_eq!(result.unwrap_err(), "ERROR_DEVICE_LOST");
    }
}
