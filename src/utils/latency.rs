//! Low-overhead timing of work that can delay input dispatch. Idle poll time is excluded.
use std::cell::RefCell;
use std::time::{Duration, Instant};

const SLOW: Duration = Duration::from_millis(16);
const SLOW_SUBPHASE: Duration = Duration::from_millis(4);
const REPORT_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone, Copy)]
pub enum Operation {
    Input,
    Refresh,
    Render,
    Flush,
    ProducerFence,
    VulkanFence,
    VulkanCleanup,
    ConfigReload,
    Animations,
    ClientDispatch,
    SceneBuild,
    VulkanRender,
    GlesRender,
    KmsSubmit,
    VulkanImport,
    NativeOffscreen,
    NativeBackground,
    NativeBlur,
    NativeAllocation,
    TransitionCapture,
}
impl Operation {
    const COUNT: usize = Self::TransitionCapture as usize + 1;

    fn threshold(self) -> Duration {
        match self {
            Self::SceneBuild
            | Self::VulkanRender
            | Self::GlesRender
            | Self::KmsSubmit
            | Self::VulkanImport
            | Self::NativeOffscreen
            | Self::NativeBackground
            | Self::NativeBlur
            | Self::NativeAllocation
            | Self::TransitionCapture => SLOW_SUBPHASE,
            _ => SLOW,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Input => "input-dispatch",
            Self::Refresh => "state-refresh",
            Self::Render => "output-render",
            Self::Flush => "client-flush",
            Self::ProducerFence => "producer-fence-wait",
            Self::VulkanFence => "vulkan-fence-wait",
            Self::VulkanCleanup => "vulkan-resource-cleanup",
            Self::ConfigReload => "config-reload",
            Self::Animations => "animation-update",
            Self::ClientDispatch => "client-dispatch",
            Self::SceneBuild => "scene-build",
            Self::VulkanRender => "vulkan-render",
            Self::GlesRender => "gles-render",
            Self::KmsSubmit => "kms-submit",
            Self::VulkanImport => "vulkan-dmabuf-import",
            Self::NativeOffscreen => "animation-offscreen",
            Self::NativeBackground => "background-prepare",
            Self::NativeBlur => "background-blur",
            Self::NativeAllocation => "offscreen-allocation",
            Self::TransitionCapture => "transition-capture",
        }
    }
}
#[derive(Default, Clone, Copy)]
struct Samples {
    last_report: Option<Instant>,
    count: u64,
    maximum: Duration,
}
impl Samples {
    fn record(
        &mut self,
        elapsed: Duration,
        now: Instant,
        threshold: Duration,
    ) -> Option<(u64, Duration)> {
        if elapsed < threshold {
            return None;
        }
        self.count = self.count.saturating_add(1);
        self.maximum = self.maximum.max(elapsed);
        if self
            .last_report
            .is_some_and(|last| now.saturating_duration_since(last) < REPORT_INTERVAL)
        {
            return None;
        }
        let report = (self.count, self.maximum);
        self.last_report = Some(now);
        self.count = 0;
        self.maximum = Duration::ZERO;
        Some(report)
    }
}
thread_local! {
    static SAMPLES: RefCell<[Samples;Operation::COUNT]> = RefCell::new([Samples::default();Operation::COUNT]);
}
/// Measures wall time, including scheduler preemption; a slow sample is not proof of GPU fault.
pub struct Span {
    operation: Operation,
    started: Instant,
}
impl Span {
    pub fn new(operation: Operation) -> Self {
        Self {
            operation,
            started: Instant::now(),
        }
    }
}
impl Drop for Span {
    fn drop(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.started);
        // Fast path performs no allocation, locking, TLS access or logging.
        let threshold = self.operation.threshold();
        if elapsed < threshold {
            return;
        }
        let report = SAMPLES.with(|samples| {
            samples.borrow_mut()[self.operation as usize].record(elapsed, now, threshold)
        });
        if let Some((count, maximum)) = report {
            tracing::warn!(target:"realand::latency",operation=self.operation.label(),
                elapsed_ms=elapsed.as_secs_f64()*1000.0,
                threshold_ms=threshold.as_secs_f64()*1000.0,
                thread=std::thread::current().name().unwrap_or("unnamed"),
                max_ms=maximum.as_secs_f64()*1000.0,slow_samples=count,
                "slow compositor operation; wall time includes scheduling delays");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fast_work_and_idle_time_do_not_report() {
        let mut samples = Samples::default();
        let now = Instant::now();
        assert!(samples
            .record(Duration::from_millis(3), now, SLOW)
            .is_none());
        assert!(samples
            .record(
                Duration::from_millis(3),
                now + Duration::from_secs(3600),
                SLOW
            )
            .is_none());
        assert_eq!(samples.count, 0);
        assert!(samples.last_report.is_none());
    }
    #[test]
    fn slow_work_reports_immediately_then_aggregates_with_bounded_frequency() {
        let mut samples = Samples::default();
        let now = Instant::now();
        assert_eq!(samples.record(SLOW, now, SLOW), Some((1, SLOW)));
        assert!(samples
            .record(
                Duration::from_millis(103),
                now + Duration::from_secs(1),
                SLOW
            )
            .is_none());
        assert!(samples
            .record(
                Duration::from_millis(21),
                now + Duration::from_secs(4),
                SLOW
            )
            .is_none());
        assert_eq!(
            samples.record(Duration::from_millis(30), now + REPORT_INTERVAL, SLOW),
            Some((3, Duration::from_millis(103)))
        );
        assert_eq!(samples.count, 0);
    }
    #[test]
    fn phases_have_independent_rate_limits() {
        let now = Instant::now();
        let mut input = Samples::default();
        let mut fence = Samples::default();
        assert!(input.record(SLOW, now, SLOW).is_some());
        assert!(fence.record(SLOW, now, SLOW).is_some());
        assert!(input.record(SLOW, now, SLOW).is_none());
    }
    #[test]
    fn subphases_report_below_frame_threshold_without_changing_parent_limits() {
        let now = Instant::now();
        let mut frame = Samples::default();
        let mut phase = Samples::default();
        assert!(frame
            .record(SLOW_SUBPHASE, now, Operation::Render.threshold())
            .is_none());
        assert_eq!(
            phase.record(SLOW_SUBPHASE, now, Operation::VulkanRender.threshold()),
            Some((1, SLOW_SUBPHASE))
        );
        assert!(phase.record(SLOW_SUBPHASE, now, SLOW_SUBPHASE).is_none());
        assert_eq!(
            phase.record(SLOW_SUBPHASE, now + REPORT_INTERVAL, SLOW_SUBPHASE),
            Some((2, SLOW_SUBPHASE))
        );
    }

    #[test]
    fn every_phase_has_a_unique_counter_and_label() {
        let phases = [
            Operation::Input,
            Operation::Refresh,
            Operation::Render,
            Operation::Flush,
            Operation::ProducerFence,
            Operation::VulkanFence,
            Operation::VulkanCleanup,
            Operation::ConfigReload,
            Operation::Animations,
            Operation::ClientDispatch,
            Operation::SceneBuild,
            Operation::VulkanRender,
            Operation::GlesRender,
            Operation::KmsSubmit,
            Operation::VulkanImport,
            Operation::NativeOffscreen,
            Operation::NativeBackground,
            Operation::NativeBlur,
            Operation::NativeAllocation,
            Operation::TransitionCapture,
        ];
        assert_eq!(phases.len(), Operation::COUNT);
        let mut labels = std::collections::HashSet::new();
        for (index, phase) in phases.into_iter().enumerate() {
            assert_eq!(phase as usize, index);
            assert!(labels.insert(phase.label()));
            assert_eq!(
                phase.threshold(),
                if index < 10 { SLOW } else { SLOW_SUBPHASE }
            );
        }
    }
}
