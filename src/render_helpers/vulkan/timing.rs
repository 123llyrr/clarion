//! Genuine Vulkan command-buffer timings. Query ownership follows submitted GPU
//! work; diagnostics never wait for a result or force another output redraw.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ash::vk;

use super::device::VulkanContext;

const FRAME_SLOTS: u32 = 32;
const COMPLETED_CAPACITY: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GpuTimingTag {
    pub output_generation: u64,
    pub redraw_sequence: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GpuTimingSample {
    pub tag: GpuTimingTag,
    pub gpu_time: Duration,
}

struct CompletedTimings {
    samples: [Option<GpuTimingSample>; COMPLETED_CAPACITY],
    first: usize,
    len: usize,
}

impl Default for CompletedTimings {
    fn default() -> Self {
        Self {
            samples: [None; COMPLETED_CAPACITY],
            first: 0,
            len: 0,
        }
    }
}

impl CompletedTimings {
    fn push(&mut self, sample: GpuTimingSample) {
        let next = (self.first + self.len) % COMPLETED_CAPACITY;
        self.samples[next] = Some(sample);
        if self.len == COMPLETED_CAPACITY {
            self.first = (self.first + 1) % COMPLETED_CAPACITY;
        } else {
            self.len += 1;
        }
    }

    fn drain_into(&mut self, samples: &mut [Option<GpuTimingSample>; COMPLETED_CAPACITY]) -> usize {
        let len = self.len;
        for (offset, target) in samples.iter_mut().take(len).enumerate() {
            *target = self.samples[(self.first + offset) % COMPLETED_CAPACITY].take();
        }
        self.first = 0;
        self.len = 0;
        len
    }
}

struct Slots(u32);

impl Default for Slots {
    fn default() -> Self {
        Self(u32::MAX)
    }
}

impl Slots {
    fn acquire(&mut self) -> Option<u32> {
        if self.0 == 0 {
            return None;
        }
        let slot = self.0.trailing_zeros();
        self.0 &= !(1 << slot);
        Some(slot)
    }

    fn release(&mut self, slot: u32) {
        debug_assert!(slot < FRAME_SLOTS);
        debug_assert_eq!(self.0 & (1 << slot), 0);
        self.0 |= 1 << slot;
    }
}

#[derive(Default)]
struct State {
    slots: Slots,
    completed: CompletedTimings,
}

pub(super) struct GpuTimings {
    ctx: Arc<VulkanContext>,
    pool: vk::QueryPool,
    valid_bits: u32,
    period_ns: f64,
    state: Mutex<State>,
}

impl std::fmt::Debug for GpuTimings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuTimings")
            .field("valid_bits", &self.valid_bits)
            .field("period_ns", &self.period_ns)
            .finish_non_exhaustive()
    }
}

impl GpuTimings {
    pub(super) fn new(ctx: Arc<VulkanContext>) -> Result<Option<Arc<Self>>, vk::Result> {
        let queues = unsafe {
            ctx.instance
                .get_physical_device_queue_family_properties(ctx.physical_device)
        };
        let valid_bits = queues
            .get(ctx.graphics_queue_family as usize)
            .map_or(0, |queue| queue.timestamp_valid_bits);
        let period_ns = f64::from(ctx.physical_device_properties.limits.timestamp_period);
        if !supports_timestamps(valid_bits, period_ns) {
            return Ok(None);
        }
        let pool = unsafe {
            ctx.device.create_query_pool(
                &vk::QueryPoolCreateInfo::default()
                    .query_type(vk::QueryType::TIMESTAMP)
                    .query_count(FRAME_SLOTS * 2),
                None,
            )?
        };
        Ok(Some(Arc::new(Self {
            ctx,
            pool,
            valid_bits,
            period_ns,
            state: Mutex::new(State::default()),
        })))
    }

    pub(super) fn acquire(self: &Arc<Self>, tag: GpuTimingTag) -> Option<QueryLease> {
        let slot = self.state.try_lock().ok()?.slots.acquire()?;
        Some(QueryLease {
            timings: self.clone(),
            slot,
            tag,
            published: AtomicBool::new(false),
        })
    }

    /// Take a bounded batch under a try-lock, then release the lock before any
    /// callback touches platform or diagnostic state. This does not allocate.
    pub(super) fn drain(&self, mut callback: impl FnMut(GpuTimingSample)) {
        let mut samples = [None; COMPLETED_CAPACITY];
        let Ok(mut state) = self.state.try_lock() else {
            return;
        };
        let len = state.completed.drain_into(&mut samples);
        drop(state);
        for sample in samples[..len].iter().flatten() {
            callback(*sample);
        }
    }
}

impl Drop for GpuTimings {
    fn drop(&mut self) {
        // Every active lease retains this owner; failed GPU retirement retains
        // its lease too. Reaching the last owner cannot leave in-flight queries.
        unsafe {
            self.ctx.device.destroy_query_pool(self.pool, None);
        }
    }
}

pub(super) struct QueryLease {
    timings: Arc<GpuTimings>,
    slot: u32,
    tag: GpuTimingTag,
    published: AtomicBool,
}

impl QueryLease {
    /// Called inside the frame command buffer before attachment transitions.
    pub(super) fn begin(&self, command: vk::CommandBuffer) {
        unsafe {
            self.timings.ctx.device.cmd_reset_query_pool(
                command,
                self.timings.pool,
                self.slot * 2,
                2,
            );
            self.timings.ctx.device.cmd_write_timestamp(
                command,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                self.timings.pool,
                self.slot * 2,
            );
        }
    }

    /// Called after all rendering, captures, HDR and ownership transitions.
    pub(super) fn end(&self, command: vk::CommandBuffer) {
        unsafe {
            self.timings.ctx.device.cmd_write_timestamp(
                command,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                self.timings.pool,
                self.slot * 2 + 1,
            );
        }
    }

    /// The caller has confirmed this frame's actual VkFence completion. There
    /// is no WAIT flag, even for a healthy device. Failed/unavailable queries
    /// do not fabricate a timing from CPU submission or fence wait duration.
    pub(super) fn publish_completed(&self) {
        if self.published.swap(true, Ordering::AcqRel) {
            return;
        }
        let mut results = [[0u64; 2]; 2];
        let result = unsafe {
            self.timings.ctx.device.get_query_pool_results(
                self.timings.pool,
                self.slot * 2,
                &mut results,
                vk::QueryResultFlags::TYPE_64 | vk::QueryResultFlags::WITH_AVAILABILITY,
            )
        };
        if result.is_err() || results.iter().any(|result| result[1] == 0) {
            return;
        }
        let Some(gpu_time) = timestamp_duration(
            results[0][0],
            results[1][0],
            self.timings.valid_bits,
            self.timings.period_ns,
        ) else {
            return;
        };
        self.timings
            .state
            .lock()
            .unwrap()
            .completed
            .push(GpuTimingSample {
                tag: self.tag,
                gpu_time,
            });
    }
}

impl Drop for QueryLease {
    fn drop(&mut self) {
        self.timings.state.lock().unwrap().slots.release(self.slot);
    }
}

fn supports_timestamps(valid_bits: u32, period_ns: f64) -> bool {
    (1..=64).contains(&valid_bits) && period_ns.is_finite() && period_ns > 0.
}

fn timestamp_duration(begin: u64, end: u64, valid_bits: u32, period_ns: f64) -> Option<Duration> {
    if !supports_timestamps(valid_bits, period_ns) {
        return None;
    }
    let mask = if valid_bits == 64 {
        u64::MAX
    } else {
        (1u64 << valid_bits) - 1
    };
    let ticks = end.wrapping_sub(begin) & mask;
    Duration::try_from_secs_f64(ticks as f64 * period_ns / 1_000_000_000.).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_period_and_valid_bit_wrap_use_device_ticks() {
        assert_eq!(
            timestamp_duration(250, 4, 8, 2.5),
            Some(Duration::from_nanos(25))
        );
        assert_eq!(
            timestamp_duration(u64::MAX - 4, 5, 64, 10.),
            Some(Duration::from_nanos(100))
        );
        assert_eq!(
            timestamp_duration(0x1fa, 0x204, 8, 2.5),
            Some(Duration::from_nanos(25))
        );
        for (bits, period) in [
            (0, 1.),
            (65, 1.),
            (32, 0.),
            (32, f64::NAN),
            (32, f64::INFINITY),
        ] {
            assert_eq!(timestamp_duration(2, 9, bits, period), None);
        }
    }

    #[test]
    fn query_slots_are_bounded_and_reused_only_after_release() {
        let mut slots = Slots::default();
        let ids: Vec<_> = (0..FRAME_SLOTS).map(|_| slots.acquire().unwrap()).collect();
        assert_eq!(ids, (0..FRAME_SLOTS).collect::<Vec<_>>());
        assert_eq!(slots.acquire(), None);
        slots.release(17);
        assert_eq!(slots.acquire(), Some(17));
        assert_eq!(slots.acquire(), None);
    }

    #[test]
    fn completed_mailbox_is_bounded_and_preserves_recent_sample_identity() {
        let mut completed = CompletedTimings::default();
        for sequence in 0..1000 {
            completed.push(GpuTimingSample {
                tag: GpuTimingTag {
                    output_generation: 7,
                    redraw_sequence: sequence,
                },
                gpu_time: Duration::from_nanos(sequence),
            });
        }
        let mut samples = [None; COMPLETED_CAPACITY];
        assert_eq!(completed.drain_into(&mut samples), COMPLETED_CAPACITY);
        for (offset, sample) in samples.into_iter().enumerate() {
            let sample = sample.unwrap();
            assert_eq!(sample.tag.redraw_sequence, 744 + offset as u64);
            assert_eq!(sample.gpu_time, Duration::from_nanos(744 + offset as u64));
        }
        assert_eq!(completed.drain_into(&mut samples), 0);
    }
}
