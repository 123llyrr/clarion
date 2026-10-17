//! Bounded frame timing snapshots. Durations are in milliseconds, not frames per second.

use serde::{Deserialize, Serialize};

/// Recent samples, summarized on request using nearest-rank percentiles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
pub struct TimingDistribution {
    /// Number of recent samples, at most 240.
    pub samples: usize,
    /// Most recently recorded value.
    pub last: f64,
    /// Arithmetic mean.
    pub mean: f64,
    /// Median.
    pub p50: f64,
    /// 95th percentile.
    pub p95: f64,
    /// Maximum within the recent sample window.
    pub max: f64,
}

/// One CPU-side frame work sample, including empty and skipped redraw attempts.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
pub struct CpuFrameSample {
    /// Per-output redraw sequence. Aligns with OutputFrameTimings::redraws.
    pub sequence: u64,
    /// CPU-side wall time preparing and submitting this redraw, in milliseconds.
    pub wall_time_ms: f64,
}

/// Main compositor-thread CPU busy time between accepted hardware-feedback callbacks.
/// Sequence aligns with continuous_active_intervals; null means clock unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
pub struct MainThreadCpuSample {
    /// Per-output qualified active interval sequence, starting at one.
    pub sequence: u64,
    /// Thread CPU busy milliseconds; null for a missing or reset CPU clock.
    pub cpu_time_ms: Option<f64>,
}

/// Genuine GPU timestamp measurement of one native output command buffer.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
pub struct GpuFrameSample {
    /// Associated per-output redraw attempt, independent of completion order.
    pub redraw_sequence: u64,
    /// Elapsed GPU timestamp milliseconds within the tagged command buffer.
    pub gpu_time_ms: f64,
}

/// One unusually long interval during explicitly continuous fixed-refresh work.
/// Records are bounded; collectors deduplicate by output and sequence.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
pub struct LongPresentationInterval {
    /// Per-output monotonic anomaly sequence, starting at one after output creation.
    pub sequence: u64,
    /// Actual monotonic hardware presentation timestamp in nanoseconds.
    pub presentation_time_ns: u64,
    /// Time since the preceding comparable hardware presentation.
    pub interval_ms: f64,
    /// Fixed-refresh period used to classify this interval.
    pub refresh_interval_ms: f64,
}

/// Timing for one connected output. Counters start when the output is added and reset on reconnect.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
pub struct OutputFrameTimings {
    /// Connector name, as used by the outputs request.
    pub output: String,
    /// Unique scheduling-instance identity within the running compositor process.
    /// Changes when an output reconnects, even if its name and counters are reused.
    /// None for older compositor versions; combine with process identity in collectors.
    #[serde(default)]
    pub output_generation: Option<u64>,
    /// Fixed refresh interval, or minimum interval with VRR. Unknown on some backends.
    pub refresh_interval_ms: Option<f64>,
    /// Whether variable refresh rate is currently enabled.
    pub variable_refresh_rate: bool,
    /// Current render budget reserved before the predicted KMS presentation deadline.
    /// This budget is used only by the physical TTY backend.
    #[serde(default)]
    pub kms_latch_budget_ms: Option<f64>,
    /// Number of physical-output redraws deferred to a KMS latch deadline.
    #[serde(default)]
    pub kms_latch_deferrals: u64,
    /// Scheduled delay for physical-output redraws deferred to a KMS latch deadline.
    #[serde(default)]
    pub kms_latch_delay_ms: Option<TimingDistribution>,
    /// How late the latch timer callback ran relative to its requested deadline.
    /// Zero means it ran on time or slightly early; absent until a timer fires.
    #[serde(default)]
    pub kms_latch_wakeup_lateness_ms: Option<TimingDistribution>,
    /// Time from the latch timer callback to the start of its redraw, excluding rendering.
    /// Includes intervening event dispatch, state refresh, and work for other outputs.
    #[serde(default)]
    pub kms_latch_to_render_delay_ms: Option<TimingDistribution>,
    /// Total redraw attempts, including skipped attempts and empty damage.
    pub redraws: u64,
    /// Frames successfully submitted to the backend.
    pub submitted: u64,
    /// Successful renders that did not require a new frame.
    pub no_damage: u64,
    /// Attempts skipped due to inactive outputs, rendering errors or other backend conditions.
    pub skipped: u64,
    /// Subset of skipped attempts deferred by bounded GPU work. These preserve
    /// continuous-activity timing; None means an older server lacks this count.
    #[serde(default)]
    pub gpu_work_deferrals: Option<u64>,
    /// Frames with a valid hardware presentation timestamp. Nested/headless backends report zero.
    pub presentations: u64,
    /// Fixed-refresh presentations with a valid target prediction, used as the late-frame
    /// denominator.
    pub fixed_refresh_presentations: u64,
    /// Fixed-refresh presentations over half an interval later than predicted.
    /// This measures prediction misses, not application dropped frames or input latency.
    pub late_presentations: u64,
    /// Wall time spent preparing and submitting redraws, excluding screencasting and idle polling.
    /// This includes CPU waits and scheduling delays, and is not GPU execution time.
    pub render_time_ms: Option<TimingDistribution>,
    /// CPU-side wall time preparing and submitting frames. Includes submission
    /// waits; currently the same measurement as render_time_ms for compatibility.
    #[serde(default)]
    pub cpu_frame_work_ms: Option<TimingDistribution>,
    /// Last 240 CPU-side redraw samples ordered by sequence. Collectors can
    /// deduplicate overlapping snapshots and detect gaps before computing a
    /// whole-run percentile; recent summary percentiles cannot be combined.
    #[serde(default)]
    pub cpu_frame_work_samples: Vec<CpuFrameSample>,
    /// Maximum CPU-side frame work since output creation, including samples no
    /// longer retained by the recent distribution.
    #[serde(default)]
    pub cpu_frame_work_max_ms: Option<f64>,
    /// Scope of the stronger CPU metric. All main-thread work between qualified
    /// feedback callbacks is counted; worker threads and blocked/preempted time
    /// are excluded. Output intervals intentionally overlap in multi-output sessions.
    #[serde(default)]
    pub compositor_main_thread_cpu_scope: Option<String>,
    /// Recent available main-thread CPU busy samples.
    #[serde(default)]
    pub compositor_main_thread_cpu_ms: Option<TimingDistribution>,
    /// Bounded raw records, including unavailable clocks as null, aligned with
    /// continuous_active_intervals rather than redraw attempts.
    #[serde(default)]
    pub compositor_main_thread_cpu_samples: Vec<MainThreadCpuSample>,
    /// Maximum available main-thread CPU busy time since output creation.
    #[serde(default)]
    pub compositor_main_thread_cpu_max_ms: Option<f64>,
    /// Qualified intervals without a comparable pair of CPU-clock samples.
    #[serde(default)]
    pub compositor_main_thread_cpu_unavailable_intervals: u64,
    /// Raw CPU records evicted from bounded storage, including unavailable samples.
    #[serde(default)]
    pub compositor_main_thread_cpu_samples_overwritten: u64,
    /// GPU execution measured by device timestamps, never inferred from CPU
    /// submission or presentation delay. None when the backend has no measurement.
    #[serde(default)]
    pub gpu_frame_work_ms: Option<TimingDistribution>,
    /// `native_output_command_buffer` after receiving supported device timestamps.
    /// Excludes KMS scanout and GPU work in earlier offscreen/upload submissions.
    #[serde(default)]
    pub gpu_frame_work_scope: Option<String>,
    /// Last 240 accepted GPU records, sorted by redraw sequence on request.
    /// Missing sequences may be skipped redraws, unavailable timestamps, or pending work.
    #[serde(default)]
    pub gpu_frame_work_samples: Vec<GpuFrameSample>,
    /// Maximum accepted genuine GPU command-buffer elapsed time since output creation.
    #[serde(default)]
    pub gpu_frame_work_max_ms: Option<f64>,
    /// Accepted GPU records evicted from bounded storage; this is not collector loss.
    #[serde(default)]
    pub gpu_frame_work_samples_overwritten: u64,
    /// Interval between accepted monotonic hardware presentation timestamps.
    /// Includes intentional idle; use continuous_active_interval_ms for smoothness.
    #[serde(default)]
    pub presented_interval_ms: Option<TimingDistribution>,
    /// Fixed-refresh intervals whose preceding presentation explicitly required
    /// another frame (pending damage or ongoing compositor animation).
    /// Excludes idle wakeups, VRR, mode changes, and known interrupted rendering.
    #[serde(default)]
    pub continuous_active_interval_ms: Option<TimingDistribution>,
    /// Qualified fixed-refresh intervals, the denominator of the long-interval rate.
    /// This does not classify every application buffer gap as an active animation.
    #[serde(default)]
    pub continuous_active_intervals: u64,
    /// Qualified intervals strictly longer than 1.5 times the fixed refresh period.
    #[serde(default)]
    pub continuous_active_long_intervals: u64,
    /// Maximum qualified interval since output creation, unaffected by ring expiry.
    #[serde(default)]
    pub continuous_active_interval_max_ms: Option<f64>,
    /// Last 240 qualified long intervals, ordered by sequence. Repeated snapshots
    /// repeat records; collectors must deduplicate and observe the overwritten count.
    #[serde(default)]
    pub continuous_active_long_interval_samples: Vec<LongPresentationInterval>,
    /// Older raw anomaly records retired from the bounded ring since output creation.
    /// This is total ring eviction, not proof that a regularly polling collector lost data.
    #[serde(default)]
    pub continuous_active_long_interval_samples_overwritten: u64,
    /// Actual hardware presentation time minus predicted time; negative values mean early.
    /// Absent until both a valid hardware timestamp and a prior presentation establish a target.
    pub presentation_error_ms: Option<TimingDistribution>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_latch_fields_default_when_reading_an_older_snapshot() {
        let json = r#"{
            "output":"winit",
            "refresh_interval_ms":null,
            "variable_refresh_rate":false,
            "redraws":2,
            "submitted":2,
            "no_damage":0,
            "skipped":0,
            "presentations":0,
            "fixed_refresh_presentations":0,
            "late_presentations":0,
            "render_time_ms":null,
            "presentation_error_ms":null
        }"#;
        let snapshot: OutputFrameTimings = serde_json::from_str(json).unwrap();
        assert_eq!(snapshot.output_generation, None);
        assert_eq!(snapshot.compositor_main_thread_cpu_scope, None);
        assert_eq!(snapshot.compositor_main_thread_cpu_ms, None);
        assert!(snapshot.compositor_main_thread_cpu_samples.is_empty());
        assert_eq!(snapshot.compositor_main_thread_cpu_unavailable_intervals, 0);
        assert_eq!(snapshot.kms_latch_budget_ms, None);
        assert_eq!(snapshot.kms_latch_deferrals, 0);
        assert_eq!(snapshot.kms_latch_delay_ms, None);
        assert_eq!(snapshot.kms_latch_wakeup_lateness_ms, None);
        assert_eq!(snapshot.kms_latch_to_render_delay_ms, None);
        assert_eq!(snapshot.cpu_frame_work_ms, None);
        assert!(snapshot.cpu_frame_work_samples.is_empty());
        assert_eq!(snapshot.cpu_frame_work_max_ms, None);
        assert_eq!(snapshot.gpu_frame_work_ms, None);
        assert_eq!(snapshot.gpu_frame_work_scope, None);
        assert!(snapshot.gpu_frame_work_samples.is_empty());
        assert_eq!(snapshot.gpu_frame_work_max_ms, None);
        assert_eq!(snapshot.gpu_frame_work_samples_overwritten, 0);
        assert_eq!(snapshot.presented_interval_ms, None);
        assert_eq!(snapshot.continuous_active_interval_ms, None);
        assert_eq!(snapshot.continuous_active_intervals, 0);
        assert_eq!(snapshot.continuous_active_long_intervals, 0);
        assert_eq!(snapshot.continuous_active_interval_max_ms, None);
        assert!(snapshot.continuous_active_long_interval_samples.is_empty());
        assert_eq!(
            snapshot.continuous_active_long_interval_samples_overwritten,
            0
        );
    }
}
