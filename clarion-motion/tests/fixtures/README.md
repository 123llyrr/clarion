# Big Sur arithmetic oracle

`big-sur-arm64.csv` is an unchanged copy of the 372 numerical samples delivered
with the Big Sur 11.7.2 / Dock 2092.50.2 reverse-engineering report on 2026-10-18.
SHA-256: `5b33ffdc225cf16cab867a8d5804c120f4b7bbd6982f95a8be6237c17106caba`.

The samples were generated with Unicorn 2.1.4 by executing isolated ARM64
instructions, not by evaluating this crate's Rust implementation:

- 240 `velocity` rows: position, velocity, start, target, normalized pixel
  threshold, allow-bounce, step count; expected position, velocity, done.
  Source recurrence: Dock `0x1001b4eec..0x1001b4fcc`.
- 132 `phase` rows: transverse edge deltas, gap, longitudinal travel, extent,
  progress; expected transverse and travel phases.
  Source arithmetic: Dock `0x1000d0650..0x1000d075c`. External log/acos/cos calls
  were replaced by host math results. These cases all use gap=0.

The corresponding complete report is named
`BigSur-Dock与MissionControl完整报告.txt`; its original numerical companion is
`BigSur-动画数值参考.rs`. The sample is the ARM64e slice, whose text is clear;
these observations do not establish Intel equivalence.

`big_sur_reference.rs` compares the independently written crate implementation
to the oracle with an absolute tolerance of 1e-12. This validates the recovered
local arithmetic, not a complete Dock process, window warp, or hardware timing.
The original-unit coordinate adapter, bounded construction, and high-refresh Hermite
interpolation in `WorkspaceFeedback` are Clarion policies with separate tests.
