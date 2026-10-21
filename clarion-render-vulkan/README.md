# clarion-render-vulkan

[![License: GPL-3.0-or-later](https://img.shields.io/badge/License-GPL--3.0--or--later-blue.svg)](https://www.gnu.org/licenses/gpl-3.0.html)
[![Vulkan 1.3](https://img.shields.io/badge/Vulkan-1.3-red.svg)](https://www.vulkan.org/)
[![Smithay](https://img.shields.io/badge/Smithay-0.7.0-orange.svg)](https://github.com/Smithay/smithay)

High-performance, zero-copy native **Vulkan rendering backend** and Smithay `Renderer` adapter for Wayland compositors, with complete **HDR10 / scRGB wide gamut color management**, DRM format modifier negotiation, and hardware-accelerated composition effects.

---

## Key Architectural Capabilities

### 1. Zero-Copy DMA-BUF Import & DRM Format Modifiers
* **Direct GPU Memory Import**: Leverages `VK_EXT_image_drm_format_modifier` and `VK_KHR_external_memory_fd` to wrap Wayland client DMA-BUF file descriptors into `VkImage` without intermediate CPU copies.
* **Tiling & Compression**: Full awareness of hardware-specific tiling formats (e.g., Intel Tile4, AMD DCC) preserving GPU memory bandwidth.
* **LRU Texture Cache**: Bounded allocation cache (`CachedDmabufTexture`) with memory quotas avoiding descriptor recreation on double/triple buffered commits.

### 2. Full-Chain HDR & Wide-Gamut Color Management
* **Color Spaces & Primaries**: Native support for **BT.2020**, **Display-P3**, and standard **sRGB**.
* **Transfer Functions**: Perceptual Quantizer (**PQ / SMPTE ST 2084**), sRGB, Gamma 2.2, and linear float pipeline.
* **Hardware YCbCr Conversion**: Uses `vk::SamplerYcbcrConversion` for zero-overhead YUV420/NV12 video texture conversions directly in GPU texture samplers.
* **Tone Mapping & Gamut Remapping**: High-dynamic-range compositing shader pipeline with luminance mastering adaptation.

### 3. Vulkan 1.3 Dynamic Rendering
* **No Legacy RenderPass**: Built entirely on Vulkan 1.3 **Dynamic Rendering** (`cmd_begin_rendering`), eliminating boilerplate `VkRenderPass` and `VkFramebuffer` state tracking.
* **Fast Target Switching**: Effortlessly switches between offscreen scratch buffers, nested WSI swapchains, and direct KMS scanout targets.

### 4. Wayland Explicit Synchronization (`linux-drm-syncobj-v1`)
* **Timeline Semaphores**: Bridges Linux DRM sync objects with Vulkan `VK_KHR_timeline_semaphore` (`vk::SemaphoreType::TIMELINE`).
* **Non-Blocking Pipeling**: True asynchronous queue scheduling between client commits, compositor rendering passes, and display controller scanout.
* **Timeout Watchdog**: Bounded fence wait protection (`FENCE_WAIT_TIMEOUT_NS`) guarding the Wayland event loop against hung GPU devices.

### 5. Asynchronous Device Recovery (`VulkanRecoveryWorker`)
* **Device Lost Tolerance**: Captures `VK_ERROR_DEVICE_LOST` and asynchronously reinitializes the Vulkan logical device without aborting the compositor process.

### 6. Hardware Composition Effects
* **Dual-Kawase Blur**: Multi-pass downsample/upsample blur pipeline with bounded scratch image recycling.
* **Decorations**: Hardware anti-aliased rounded corners, borders with gradient color spaces (Oklab / OKLCh), and Gaussian drop shadows.
* **Custom Shader Engine**: Real-time user fragment animations compiled to SPIR-V with strict interface bounds.

---

## Module Layout

```
clarion-render-vulkan/
├── Cargo.toml
├── README.md
└── src/
    ├── lib.rs                  # Crate root, VulkanRenderer and NativeRenderer
    ├── device.rs               # VkInstance, DRM node matching (/dev/dri/renderD*)
    ├── dmabuf.rs               # DMA-BUF zero-copy importer & modifier cache
    ├── frame.rs                # Smithay Frame trait implementation & command recording
    ├── native.rs               # Smithay Renderer, ImportDma, ExportMem traits
    ├── color.rs                # ColorDescription, Primaries, PQ encode/decode
    ├── color_pipeline.rs       # HDR push constants, gamut conversion matrix
    ├── hdr.rs                  # HDR swapchain capabilities & presentation
    ├── blur.rs                 # Native Dual-Kawase compute/fragment blur
    ├── custom_shader.rs        # SPIR-V user shader engine & animation programs
    ├── memory.rs               # Staging buffers, host visible memory quotas
    ├── pipeline.rs             # Vulkan pipeline layout, YUV samplers, descriptor sets
    ├── lifecycle.rs            # Asynchronous resource retirement & device recovery
    ├── wsi.rs                  # Nested Winit VK_KHR_swapchain integration
    └── shaders/                # Precompiled SPIR-V binaries & GLSL sources
```

---

## Integration Example

```rust
use std::sync::Arc;
use clarion_render_vulkan::{VulkanRenderer, VulkanContext};
use smithay::backend::drm::DrmNode;

// 1. Initialize Vulkan context matching the KMS primary render node
let render_node = DrmNode::from_path("/dev/dri/renderD128").unwrap();
let mut renderer = VulkanRenderer::new();
renderer.init_device(&render_node).expect("Failed to initialize Vulkan renderer");

// 2. Wrap into Smithay NativeRenderer
let native_renderer = renderer.into_native_renderer().expect("Failed to create NativeRenderer");
```

---

## License

This crate is licensed under the **GNU General Public License v3.0 or later (GPL-3.0-or-later)**.
Part of the [Clarion Compositor](https://github.com/123llyrr/clarion) project.
