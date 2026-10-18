# Vulkan backend validation

On a physical TTY, Clarion's primary GPU can use the native Vulkan `Renderer`/`Frame` implementation to draw the desktop into Smithay's DRM compositor. It does not render the scene through GLES first. Secondary GPU outputs try the same primary-GPU Vulkan renderer with linear DMA-BUFs allocated on the primary GPU; connection-time KMS scanout and Vulkan target-import probes select GLES per output if cross-device sharing is unsupported. After the color-management global is published, outputs requiring GLES are rejected and native-device failures use native recovery. Managed composition disables direct scanout and overlays until per-plane color conversion exists. Hardware cursors are allowed only after SDR signaling is confirmed and the cursor uses compatible default sRGB pixels; HDR/unknown transitions and PQ/scRGB/floating-point cursor surfaces remain composited. Headless mode has a native Vulkan offscreen renderer. Nested winit mode creates a Vulkan WSI surface and swapchain directly; it composites to a Vulkan texture and queues a GPU copy for presentation without a GLES scene pass. Its two frame slots avoid a per-frame queue-idle wait.

Select `renderer "vulkan"` in the configuration, or start with `--renderer vulkan`. On TTY, check the startup log for `Native Vulkan desktop renderer ready for direct KMS submission`; nested mode logs `nested native Vulkan WSI renderer initialized` and its first presentation. The requested option alone does not prove that Vulkan initialized. TTY initialization or KMS swapchain format negotiation failures select the GLES fallback; an explicitly requested nested Vulkan backend reports an initialization error if the host cannot support Vulkan WSI.

TTY native initialization resolves the render node from DRM topology and defers EGL/GLES registration. A working Vulkan device therefore does not require an EGL probe for normal startup. The compatibility manager activates only when an output or legacy import actually needs GLES; failed activation remains retryable. Explicit GLES selection keeps EGL's device discovery and software-renderer rejection. The policy and device registration lifecycle have independent tests; those tests do not substitute for booting a physical KMS session.

The native renderer returns a GPU completion `SyncPoint`. For a client DMA-BUF associated with a Smithay surface buffer, the frame retains that buffer until GPU completion; this delays both `wl_buffer.release` and the syncobj release point without blocking ordinary frames on the CPU. Raw imported DMA-BUFs without a known surface owner still take the synchronous safety path. Direct scanout keeps its Wayland buffer through the KMS frame. The TTY path retains the prepared slot and retries after completion when Smithay's `needs_sync()` reports that an exportable fence cannot reach KMS, or when the debug option requests completion before queueing. On supported drivers, Smithay submits the exported fence through KMS `IN_FENCE_FD`; unsupported planes require completion before queueing, handled by the retained-frame retry rather than an event-loop wait. A supported primary DRM device publishes `linux-drm-syncobj-v1`; client acquire points block the corresponding surface commit until signaled, and release points follow the retained buffer's lifetime.

One completion worker per native renderer retires submitted frames and SHM uploads even when no later frame is drawn. It waits for GPU completion without holding the command-pool lock and then pings the compositor event loop so queued Wayland release events are flushed. The worker sleeps while idle, serves all the renderer's outputs and has a bounded submission queue. Queue saturation uses the existing bounded cleanup path rather than an unbounded channel send. Native `SyncPoint`s retain lightweight retirement state and independent native fence FDs; keeping one after completion does not keep client buffers or GPU textures alive. Explicit `wait()` confirms the submission fence and releases completed GPU references before returning, preserving immediate blur-cache reuse. Ordinary render-time retirement only checks completed fences and does not wait for the worker to run. Renderer teardown lets in-flight owners finish safely without joining a stalled GPU on the event-loop thread.

Implicit DMA-BUF acquire readiness also gates commits. `AlreadyReady` requires no watcher. Failure to register a watcher for a pending producer cancels that commit, just as failure to register an explicit acquire point does; subsequent valid commits can recover.

Vulkan initialization requires API 1.3 and dynamic rendering. Linux external-memory FD, DMA-BUF, DRM-modifier and foreign-queue-family extensions are probed as optional capabilities: SHM, optimal-tiling offscreen targets and nested WSI remain usable without them, while native TTY/KMS scanout requires the complete DMA-BUF import set and falls back to GLES when it is unavailable. Nested WSI requires `VK_KHR_swapchain`; `VK_EXT_surface_maintenance1` plus `VK_EXT_swapchain_maintenance1` are optional. When present, swapchain-maintenance present fences protect semaphore and swapchain retirement; otherwise the WSI path uses a conservative graphics-queue-idle retirement fallback. Device selection matches the requested DRM node. Import intersects image and FD memory-type requirements; implicit modifiers are not guessed. Exportable producer fences are imported as temporary `SYNC_FD` semaphores for GPU-side acquisition, while non-exportable fences require a CPU wait.

Output-scoped work always follows the output's active renderer. This includes animation snapshots, resize retargets, screencopies, color picking, window/output screenshots, screen-cast frames and the screenshot selection UI. If one KMS surface has migrated to GLES while another remains native Vulkan, captures retain per-output texture and help-panel types; selecting a mismatched or removed output reports an error instead of drawing a foreign texture or panicking.

When the driver advertises the required modifier and sampler features, client NV12, YUV420 and P010 DMA-BUFs use Vulkan sampler YCbCr conversion rather than a CPU conversion pass. Multi-FD disjoint planes are bound separately when supported. Tagged BT.2020 surfaces select the BT.2020 sampler model before the RGB color transform; untagged content uses the existing 601/709 resolution heuristic. Sampling uses limited range. The color-representation protocol and general YUV range/chroma-location control are not implemented.

Native frames wait for imported producer semaphores at `ALL_COMMANDS`. Their
FOREIGN queue-family acquire barriers do not opt into maintenance8's
all-stages ownership-transfer dependency flag, so waiting only at the fragment
stage does not order the ownership acquisition. See the
[Vulkan queue-family ownership-transfer rules](https://docs.vulkan.org/spec/latest/chapters/synchronization.html#synchronization-queue-transfers).

Synchronous texture uploads and readbacks hold the command-pool/queue lock only
while allocating, recording, submitting or recycling commands. GPU fence waits
and CPU staging copies happen outside that lock, allowing another producer or
output to make progress while a transfer is pending. Animation shader draws
lease their own descriptors until their final GPU owner releases them; each
program retains at most 32 idle slots for subsequent draws.

Color-managed output retains a persistent RGBA16F extended-sRGB scene for each
live framebuffer. Per-surface transforms decode SDR/PQ/scRGB, adapt color and
luminance, and preserve negative values and highlight headroom. The final pass
converts damaged regions to the output's SDR or BT.2020/PQ encoding once; empty
damage skips conversion. Blur and effects sample the scene rather than mixing
new SDR and retained PQ pixels. A first partial repaint of an initialized target
reconstructs its scene from the previous encoding. Cache identity includes color
description and precision, including changes of HDR peak and reference white.
Scene retention follows framebuffer lifetime and has a 1 GiB allocation budget;
exhaustion defers a new frame instead of losing live partial-damage history.

HDR and scaled-capture scratch images share a cache keyed by device, dimensions,
format, alpha, usage and reuse class. Capture/blur images return after GPU
completion. Private, fully overwritten color intermediates can return after
successful submission on the same graphics queue: barriers order later writes
after earlier reads, and completion owners still keep every image alive.
These intermediates cannot be acquired as capture/blur history. Both classes
share the existing total of eight cached images and 256 MiB of actual Vulkan
allocation sizes, including driver padding. Device/queue changes invalidate the
ordered reuse scope. See [HDR implementation](hdr.md) for protocol, brightness,
gamut and KMS transition semantics.

Default perceptual SDR can bypass the redundant scene transform on HDR outputs
whose nonzero black point cancels through black/white adaptation and sRGB
normalization. A texture bypass requires an opaque, bounded UNORM RGB source;
alpha sources retain their pass and check premultiplied bounds per sample.
Extended values, PQ/scRGB and other metadata or intents retain the full transform.
`hdr_sdr_scene_identity_matches_full_transform_with_alpha_and_extended_samples`
compares both paths on actual GPUs, including 10-bit, scaled sampling and malformed
premultiplied pixels. `hdr_sdr_scene_identity_4k_gpu_timing` interleaves both paths
for opaque, alpha and mixed 3840×2400 scenes, including FP16 composition and the
unchanged final PQ pass. Its GPU timestamps and fixture CPU measurements do not
measure physical presentation cadence or input latency.
`hdr_sdr_scene_identity_three_frames_reuse_scratch_without_hot_allocations`
retains three real asynchronous completions before retiring their images: at
3840×2400, two full-screen ARGB sources reuse two ordered color images instead
of retaining six distinct images that repeatedly overflow the 256 MiB cache.
It checks allocation counts, alternating source inversion and image reuse after
warmup without increasing the memory limit.
`ordered_color_pool_preserves_each_gated_frame_pixels_before_retirement`
blocks the GPU before submitting three different frames, verifies that their
fences are pending, then checks each output's pixels after releasing the gate.

## Hardware check

Install `vulkan-tools` and Khronos Vulkan validation layers for your distribution. Run from the repository root:

```sh
./scripts/check-vulkan.sh /dev/dri/renderD128
# Or omit the node argument to check every accessible render node.
```

On a multi-GPU system, run once per device with its matching ICD, for example:

```sh
VK_DRIVER_FILES=/usr/share/vulkan/icd.d/intel_icd.json \
    CLARION_VULKAN_LOG_DIR=target/vulkan-check-intel \
    ./scripts/check-vulkan.sh /dev/dri/renderD128
VK_DRIVER_FILES=/usr/share/vulkan/icd.d/nvidia_icd.json \
    CLARION_VULKAN_LOG_DIR=target/vulkan-check-nvidia \
    ./scripts/check-vulkan.sh /dev/dri/renderD129
```

Adjust the node and ICD paths to the machine. The checker also runs the native
headless output, both Wayland syncobj acquire/release integrations, FP16
Wayland color-client/capture and resize-snapshot integrations, and three effect-cache color-switch
integrations. Every
integration must execute exactly one test and report the same actual GPU as
the render-node suite. The test headless fixture selects the node through
`CLARION_VULKAN_TEST_NODE`; the selected loader/ICD must expose that node.
Normal, non-test headless sessions use loader/ICD device selection. The namespace total includes CPU/cache
tests and optional YUV probes; it is not a count of GPU cases exercised.
The private lifecycle subprocess entry point is excluded from the parent suite;
its lifecycle tests invoke it with the required fixture environment themselves.

`CLARION_VULKAN_TEST_SCOPE=rgb-hdr` runs the RGB/HDR acceptance scope. It excludes
exactly three experimental CPU-dumb NV12 interoperability probes in `yuv_cold`,
while retaining GPU-produced NV12 sampling and all RGB/HDR cases. On this task's
NVIDIA driver those CPU-dumb probes expose an allocation-size mismatch or wrong
pixels; they have not passed acceptance. The default `all` scope keeps these
checks strict and reports their failures. RGB/HDR acceptance does not certify
that producer path.

For a locally unpacked validation layer, set `VK_LAYER_PATH` to its `explicit_layer.d` directory and `LD_LIBRARY_PATH` to its library directory. The script requires the layer, enables synchronization validation, fails on validation diagnostics and requires actual GPU tests. It reports unsupported optional YUV cases in a per-device `*.unsupported.log`; these do not count as exercised. Any other skipped GPU case is an error. It does not take display master or restart the desktop. When testing headless backend integration on a multi-GPU machine, also select the matching loader ICD with `VK_DRIVER_FILES` so it can expose the node requested by the test fixture.

Coverage includes:

- Exact DRM device selection and 8/10-bit Vulkan pipelines.
- Required compositor formats ARGB8888, ABGR8888 and ABGR2101010, plus optional formats allocatable by the driver.
- Packed RGB565/BGR565 client DMA-BUF sampling when the Vulkan driver advertises the format and modifier; these are not output target formats.
- A real linear NV12 DMA-BUF imported into the Intel GPU, sampled with a Vulkan YCbCr immutable sampler and compared against expected RGB pixels.
- GLES → Vulkan → GLES pixel comparisons with asymmetric regions and alpha, repeated buffer reuse and fresh GBM exports.
- Solid texture uploads, abandoned draws, draw resource lifetime, and 2,200 descriptor allocations/reclaims.
- GPU producer-fence acquisition with a counted CPU-wait sentinel, partial-damage pixel equality, expired-history fallback and command/fence handle reuse.
- A blocked GPU queue released by a later command-lock owner during synchronous readback and partial upload, with exact pixel comparisons.
- Animation descriptor lifetime across asynchronous GPU completion, bounded idle reuse, and changed texture/uniform pixel comparisons.
- HDR 10-bit partial redraw with overlapping and opaque damage across all eight output transforms, unchanged-region byte equality, and empty-damage stability.
- Mixed SDR/PQ/scRGB, BT.2020 primaries, configurable reference white, absolute luminance, black-point adaptation, FP16 snapshots and HDR-to-SDR capture against a CPU reference.
- Color-managed partial repaint through more than sixteen live owned and GBM/DMA-BUF targets, target retirement and scene reconstruction on the first managed repaint.
- A real FP16 Wayland SHM client changing only its image description, checked against the composed output and production PNG screenshot action.
- A resize commit replacing buffers and color descriptions on a parent and synchronized child, checking the old snapshot and new output independently.
- Interrupted producer waits during initial, partial and empty-damage composition, followed by successful retries with preserved damage and pixel equality.
- Rejection of zero/negative DMA-BUF dimensions and extents exceeding the queried format limits, before Vulkan image creation for both sampled textures and targets.
- A 1080p final-pass timing measurement including CPU overhead and GPU fence wait, with and without the import/scratch cache. Validation layers affect timing; it is not a desktop FPS benchmark.

The original GLES-to-Vulkan composition path remains available for regression tests. The target cache identifies underlying DMA-BUF allocations and retains imported images while their swapchain slots remain live. Dead slots are pruned; a 1 GiB allocation budget bounds admission without evicting live color-managed history. Empty frames skip that Vulkan pass. Each allocation keeps its own damage commit. Unknown or expired damage history conservatively repaints fully. A clipped bounding rectangle limits partial updates. Command buffers and completion fences are reused after completion (at most eight idle slots).

Hardware tests exercise both native Vulkan rendering and the earlier GLES-to-Vulkan composition path without taking DRM display master. They do not replace TTY checks for KMS presentation, VRR, monitor hotplug, suspend/resume or long-duration use.

The opt-in `wayland_client_acquire_and_release_follow_gpu_use` integration test also uses a real render node and GBM buffer. It verifies a Wayland client's syncobj acquire point blocks the commit, then checks the release point after a Vulkan frame and buffer detach:

```sh
cargo test --lib --no-default-features wayland_client_acquire_and_release_follow_gpu_use -- --ignored --test-threads=1
```

Nested Vulkan WSI can be checked inside an existing Wayland session with a separate, non-session `clarion --renderer vulkan` instance. Look for `nested Vulkan WSI presented first frame`, then resize and close that nested window. This does not exercise physical KMS scanout.

## Nourish reference

This adaptation was checked against Nourish commit `d6893ce06974204b04a5aad4521a5847ad58e61d`:

- `compositor.kernel/kernel.vulkan/vulkan.renderer/renderer.core/core.base/renderer/submit.rs`: damage-aware submission, completion before reuse/retirement.
- `compositor.kernel/kernel.vulkan/vulkan.command/command.record/record.base/record.rs`: LOAD plus damage-limited clears/draws.
- `compositor.kernel/kernel.vulkan/vulkan.sync/sync.import/import.base/import.rs`: TEMPORARY SYNC_FD acquisition and FD ownership on failure.
- `compositor.kernel/kernel.vulkan/vulkan.sync/sync.export/export.base/export.rs`: native render-completion fence, reviewed as the next integration seam.

The selected upstream license is preserved in `resources/licenses/nourish-MIT.txt`. The reference above explains the earlier GLES-to-Vulkan path; current primary-GPU TTY composition uses the native adapter. KMS fence delivery depends on Smithay and driver capability, while client acquire/release synchronization requires the DRM syncobj protocol state and buffer-owner lifetime handling described above. These paths require real-session validation in addition to the offscreen tests.

## Regional framebuffer blur

The native non-xray framebuffer effect keeps its original capture rectangle,
texture dimensions, viewport and UV coordinates. When a surface requests blur
only for part of its surface (for example a thin menu bar on a large transparent
shell window), it limits the blur passes' render area and scissor instead of
cropping the capture. This preserves the dual-Kawase sample phase for odd image
sizes and fractional scaling.

The final sampled area uses the same transformed subregion pixel boundaries as
the draw path. It is mapped into the captured texture's orientation, expanded
for bilinear sampling, then propagated backwards through every down/up pass.
Each pass includes the shader's complete tap reach and two extra source texels
for filtering/rounding. Disjoint regions use a conservative bounding rectangle;
the final draw still clips to the exact region. Every capture computes fresh
dependencies, including after region moves, growth, output transforms or crop
changes. A full-surface region uses the original full-pass path. An empty visible
region skips capture and blur; cached image dimensions and lifetime rules remain
unchanged.

The `regional_blur` CPU tests enumerate bilinear source taps for odd, even and
one-pixel dimensions, offsets 0/1.25/8, and 1/3/31 passes. The opt-in
`native_regional_framebuffer_blur_matches_full_pass_pixels` test compares final
GPU readback byte-for-byte against the original full-pass path. It covers all
eight output transforms, fractional scaling, zoom, crop, rounded clipping,
noise/saturation, disjoint/empty regions and repeated movement/growth using the
same blur cache. It also exercises zero offset and the 31-pass limit. Run with
`CLARION_VULKAN_TEST_NODE` set as described above and enable the validation layer.

For a 1920×1080 capture, a 24-pixel-high required output area, three passes and
offset 2, the sum of blur pass render areas falls from 3,402,000 to 131,040 pixels
(96.15% less). This is a deterministic reduction in blur fragment/clear area,
not a measured desktop frame-rate increase. Capture bandwidth, texture memory
and Smithay's framebuffer-effect damage propagation are unchanged; this change
does not establish the cause of intermittent desktop stalls.
