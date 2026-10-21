# Specialized scene composition and launcher work (2026-10-16)

Stage, Spread and Cube now follow the same ownership boundary as the ordinary
desktop and Expo. Layout projects numeric geometry, visibility and narrow source
capabilities without accepting a renderer. The scene consumes that captured plan;
the production dispatch stores unit presentation tags and typed compositions,
without retaining a complete monitor or closing over a desktop owner.

`scene/stage_composition.rs` owns card composition, ordered window channels,
clipping, rail text, borders and layer callbacks. `scene/spread_composition.rs`
owns overview tiles, closing sources, shadows, labels and the faded desktop.
`scene/cube_composition.rs` composes both faces from the same source projection
used by its ordinary fallback. Cube geometry lives in `scene/cube_geometry.rs`.
GLES and native Vulkan continue to consume the original render target so capture
privacy remains distinct from visible output.

Cube targets, mesh fingerprints, commits and sampling feedback live with the
scene resource owner. A temporary GPU Busy result preserves pending work and
restores feedback by workspace identity, including when the two faces exchange
drawing slots. Fatal or unsupported rendering retires offscreen feedback before
falling back to the ordinary scene. Leaving Cube clears its targets once;
subsequent ordinary frames neither traverse window feedback nor clear scratch
buffers again. Context replacement invalidates native images and feedback.

The launcher retains its panel across unrelated controller notifications and
caches catalog and search projections. Icon editing uses one shared animation
clock with per-icon phase offsets. Closing stops that clock synchronously and
publishes the state change; a rapid reverse transition cannot revive the previous
editing animation. These changes preserve the existing liquid glass presentation,
application identity and launch behavior.

Native nested Vulkan publishes DMA-BUF v4 default feedback when its selected
physical device reports a valid DRM render-device identity and import formats.
This gives EGL clients a device-discovery path without adding a legacy EGL or
`wl_drm` dependency to Vulkan. Unsupported devices retain the explicit v3
fallback. Device numbers are derived from Vulkan properties, with checked
conversion, rather than a fixed render-node path. An isolated EGL probe and the
same launcher workload must verify hardware selection and its performance;
protocol advertisement alone does not establish a frame-time result.

Regression tests cover captured source membership, mode transition geometry,
fractional scale, Cube exit and Busy feedback, catalog invalidation, panel
retention and editing-clock lifecycle. The native launcher profile fixture uses
natural vsync in profile mode and records raw engine frame timings separately
from warmup. Nested compositor results are not physical KMS acceptance results.
Final verification receipts and raw logs are maintained outside the source tree.

This migration removes the specialized full-monitor rendering adapters. It does
not complete every remaining runtime boundary: Tile still exposes a narrow
internal-effects render source, close/snapshot preparation still has compatibility
entry points in layout, and cold exact snapshots and unowned DMA-BUF imports still
have documented wait boundaries. Performance claims require the final binary and
the actual tested display path; older measurements cannot establish this build's
frame-time targets.
