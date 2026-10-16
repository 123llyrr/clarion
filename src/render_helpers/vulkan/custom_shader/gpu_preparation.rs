//! Vulkan device objects only. No renderer, Wayland, scene or compositor state crosses this cut.
use ash::vk;
use calloop::channel::Channel;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::{gpu_provider, PreparedShader, Program};
use crate::render_helpers::vulkan::device::VulkanContext;
use crate::render_helpers::vulkan::pipeline::YuvPipeline;
use crate::render_helpers::vulkan::VulkanPipelineManager;

pub(crate) const YUV_PREPARATION_KIND: u8 = u8::MAX - 1;

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
                return Err(vk::Result::ERROR_DEVICE_LOST.to_string())
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
        assert_eq!(
            result.unwrap_err(),
            vk::Result::ERROR_DEVICE_LOST.to_string()
        );
    }
}
