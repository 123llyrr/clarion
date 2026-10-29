# Clarion Blade Graphics patch record

Base: the published `blade-graphics` **0.7.1** crate from crates.io (MIT).
The workspace `[patch.crates-io]` selects this directory. Retain the upstream
license, sources and normalized manifest.

Clarion addition:

- `src/vulkan/surface.rs`: `Context::reconfigure_surface` accepts
  `VK_SUBOPTIMAL_KHR` from `vkCreateSwapchainKHR` instead of unwrapping it.
  The success code means the swapchain exists but does not match the surface
  exactly, so the driver may scale presentation to it; the next drawable-size
  change recreates it. Stock blade treats every non-`VK_SUCCESS` result as an
  error and aborts the process. NVIDIA's Wayland WSI returns that code
  intermittently when the requested extent does not match the surface's
  current buffer, which killed `clarion-shell` while it opened a panel (for
  example the notification center). The patch calls the raw entry point so the
  returned handle is not discarded with the error value, reports the event on
  stderr (the shell does not install a `log` logger, so journald keeps it), and
  still panics for a genuinely missing handle or a real error.

Warning hygiene (no behavior change):

- `src/vulkan/init.rs`: `u64::max_value()` → `u64::MAX`.
- `src/vulkan/{mod,command,pipeline}.rs`: explicit `'_` lifetimes on returned
  encoders, option and shader types (`mismatched_lifetime_syntaxes`).
- `src/vulkan/surface.rs`: dropped the `ash::vk::Handle` import left over from
  an earlier version of the suboptimal-swapchain patch.
