# Vulkan backend validation

On a physical TTY, Realand's primary GPU can use the native Vulkan `Renderer`/`Frame` implementation to draw the desktop into Smithay's DRM compositor. It does not render the scene through GLES first. Secondary GPU outputs try the same primary-GPU Vulkan renderer with linear DMA-BUFs allocated on the primary GPU; connection-time KMS scanout and Vulkan target-import probes select GLES per output if cross-device sharing is unsupported. Direct scanout can bypass Vulkan composition. Headless mode has a native Vulkan offscreen renderer. Nested winit mode creates a Vulkan WSI surface and swapchain directly; it composites to a Vulkan texture and queues a GPU copy for presentation without a GLES scene pass. Its two frame slots avoid a per-frame queue-idle wait.

Select `renderer "vulkan"` in the configuration, or start with `--renderer vulkan`. On TTY, check the startup log for `Native Vulkan desktop renderer ready for direct KMS submission`; nested mode logs `nested native Vulkan WSI renderer initialized` and its first presentation. The requested option alone does not prove that Vulkan initialized. TTY initialization or KMS swapchain format negotiation failures select the GLES fallback; an explicitly requested nested Vulkan backend reports an initialization error if the host cannot support Vulkan WSI.

TTY native initialization resolves the render node from DRM topology and defers EGL/GLES registration. A working Vulkan device therefore does not require an EGL probe for normal startup. The compatibility manager activates only when an output or legacy import actually needs GLES; failed activation remains retryable. Explicit GLES selection keeps EGL's device discovery and software-renderer rejection. The policy and device registration lifecycle have independent tests; those tests do not substitute for booting a physical KMS session.

The native renderer returns a GPU completion `SyncPoint`. For a client DMA-BUF associated with a Smithay surface buffer, the frame retains that buffer until GPU completion; this delays both `wl_buffer.release` and the syncobj release point without blocking ordinary frames on the CPU. Raw imported DMA-BUFs without a known surface owner still take the synchronous safety path. Direct scanout keeps its Wayland buffer through the KMS frame. The TTY path waits when Smithay's `needs_sync()` reports that an exportable fence cannot reach KMS, or when the debug option explicitly requests a wait. On supported drivers, Smithay submits the exported fence through KMS `IN_FENCE_FD`; older NVIDIA drivers and unsupported planes need the wait. A supported primary DRM device publishes `linux-drm-syncobj-v1`; client acquire points block the corresponding surface commit until signaled, and release points follow the retained buffer's lifetime.

One completion worker per native renderer retires submitted frames and SHM uploads even when no later frame is drawn. It waits for GPU completion without holding the command-pool lock and then pings the compositor event loop so queued Wayland release events are flushed. The worker sleeps while idle, serves all the renderer's outputs and has a bounded submission queue. Queue saturation uses the existing bounded cleanup path rather than an unbounded channel send. Native `SyncPoint`s retain lightweight retirement state and independent native fence FDs; keeping one after completion does not keep client buffers or GPU textures alive. Explicit `wait()` confirms the submission fence and releases completed GPU references before returning, preserving immediate blur-cache reuse. Ordinary render-time retirement only checks completed fences and does not wait for the worker to run. Renderer teardown lets in-flight owners finish safely without joining a stalled GPU on the event-loop thread.

Implicit DMA-BUF acquire readiness also gates commits. `AlreadyReady` requires no watcher. Failure to register a watcher for a pending producer cancels that commit, just as failure to register an explicit acquire point does; subsequent valid commits can recover.

Vulkan initialization requires API 1.3 and dynamic rendering. Linux external-memory FD, DMA-BUF, DRM-modifier and foreign-queue-family extensions are probed as optional capabilities: SHM, optimal-tiling offscreen targets and nested WSI remain usable without them, while native TTY/KMS scanout requires the complete DMA-BUF import set and falls back to GLES when it is unavailable. Nested WSI requires `VK_KHR_swapchain`; `VK_EXT_surface_maintenance1` plus `VK_EXT_swapchain_maintenance1` are optional. When present, swapchain-maintenance present fences protect semaphore and swapchain retirement; otherwise the WSI path uses a conservative graphics-queue-idle retirement fallback. Device selection matches the requested DRM node. Import intersects image and FD memory-type requirements; implicit modifiers are not guessed. Exportable producer fences are imported as temporary `SYNC_FD` semaphores for GPU-side acquisition, while non-exportable fences require a CPU wait.

Output-scoped work always follows the output's active renderer. This includes animation snapshots, resize retargets, screencopies, color picking, window/output screenshots, screen-cast frames and the screenshot selection UI. If one KMS surface has migrated to GLES while another remains native Vulkan, captures retain per-output texture and help-panel types; selecting a mismatched or removed output reports an error instead of drawing a foreign texture or panicking.

When the driver advertises the required modifier and sampler features, client NV12, YUV420 and P010 DMA-BUFs use Vulkan sampler YCbCr conversion rather than a CPU conversion pass. Multi-FD disjoint planes are bound separately when supported. The linux-dmabuf protocol does not supply a YUV matrix or range description here, so this path currently uses limited range and an SDR 601/709 resolution heuristic; it is not a color-managed or HDR video pipeline.

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

HDR final conversion records writes in attachment coordinates, unions them
into disjoint rectangles and converts those pixels to PQ once. Empty damage
skips the conversion. Retained output pixels are already encoded and must not
pass through the conversion again. The opaque draw paths also record damage
when no background clear was needed. HDR and scaled-capture scratch images
share a cache keyed by device, dimensions, format, alpha and required usage;
images return only after GPU completion. Idle retention is capped at eight
images and 64 MiB of actual Vulkan allocation sizes, including driver padding.

This is a partial-redraw correctness and allocation-reuse fix. HDR still copies
the full framebuffer before the final pass, and the framebuffer mixes freshly
composed SDR pixels with retained PQ pixels. Effects that sample retained
framebuffer content need a persistent SDR scene target and a complete color
pipeline; this change does not provide that larger redesign.

## Hardware check

Install `vulkan-tools` and Khronos Vulkan validation layers for your distribution. Run from the repository root:

```sh
./scripts/check-vulkan.sh /dev/dri/renderD128
# Or omit the node argument to check every accessible render node.
```

On a multi-GPU system, run once per device with its matching ICD, for example:

```sh
VK_DRIVER_FILES=/usr/share/vulkan/icd.d/intel_icd.json \
    REALAND_VULKAN_LOG_DIR=target/vulkan-check-intel \
    ./scripts/check-vulkan.sh /dev/dri/renderD128
VK_DRIVER_FILES=/usr/share/vulkan/icd.d/nvidia_icd.json \
    REALAND_VULKAN_LOG_DIR=target/vulkan-check-nvidia \
    ./scripts/check-vulkan.sh /dev/dri/renderD129
```

Adjust the node and ICD paths to the machine. The checker also runs the native
headless output and both Wayland syncobj acquire/release integrations. Every
integration must execute exactly one test and report the same actual GPU as
the render-node suite. Headless device selection follows the loader/ICD, not
the render-node environment variable. The namespace total includes CPU/cache
tests and optional YUV probes; it is not a count of GPU cases exercised.

For a locally unpacked validation layer, set `VK_LAYER_PATH` to its `explicit_layer.d` directory and `LD_LIBRARY_PATH` to its library directory. The script requires the layer, enables synchronization validation, fails on validation diagnostics and requires actual GPU tests. It reports unsupported optional YUV cases in a per-device `*.unsupported.log`; these do not count as exercised. Any other skipped GPU case is an error. It does not take display master or restart the desktop. When testing headless backend integration on a multi-GPU machine, also select the matching loader ICD with `VK_DRIVER_FILES`; the render-node test variable alone does not select the headless renderer's device.

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
- Interrupted producer waits during initial, partial and empty-damage composition, followed by successful retries with preserved damage and pixel equality.
- Rejection of zero/negative DMA-BUF dimensions and extents exceeding the queried format limits, before Vulkan image creation for both sampled textures and targets.
- A 1080p final-pass timing measurement including CPU overhead and GPU fence wait, with and without the import/scratch cache. Validation layers affect timing; it is not a desktop FPS benchmark.

The original GLES-to-Vulkan composition path remains available for regression tests. Its target cache identifies underlying DMA-BUF allocations, retains their FDs against inode reuse and is limited to 16 entries. Empty frames skip that Vulkan pass. Each allocation keeps its own damage commit. Unknown or expired damage history conservatively repaints fully. A clipped bounding rectangle limits partial updates. Command buffers and completion fences are reused after completion (at most eight idle slots).

Hardware tests exercise both native Vulkan rendering and the earlier GLES-to-Vulkan composition path without taking DRM display master. They do not replace TTY checks for KMS presentation, VRR, monitor hotplug, suspend/resume or long-duration use.

The opt-in `wayland_client_acquire_and_release_follow_gpu_use` integration test also uses a real render node and GBM buffer. It verifies a Wayland client's syncobj acquire point blocks the commit, then checks the release point after a Vulkan frame and buffer detach:

```sh
cargo test --lib --no-default-features wayland_client_acquire_and_release_follow_gpu_use -- --ignored --test-threads=1
```

Nested Vulkan WSI can be checked inside an existing Wayland session with a separate, non-session `realand --renderer vulkan` instance. Look for `nested Vulkan WSI presented first frame`, then resize and close that nested window. This does not exercise physical KMS scanout.

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
`REALAND_VULKAN_TEST_NODE` set as described above and enable the validation layer.

For a 1920×1080 capture, a 24-pixel-high required output area, three passes and
offset 2, the sum of blur pass render areas falls from 3,402,000 to 131,040 pixels
(96.15% less). This is a deterministic reduction in blur fragment/clear area,
not a measured desktop frame-rate increase. Capture bandwidth, texture memory
and Smithay's framebuffer-effect damage propagation are unchanged; this change
does not establish the cause of intermittent desktop stalls.
