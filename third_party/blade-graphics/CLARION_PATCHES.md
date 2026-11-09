# Clarion Blade Graphics patch record

Base: the published `blade-graphics` **0.7.1** crate from crates.io (MIT).
The workspace `[patch.crates-io]` selects this directory. Retain the upstream
license, sources and normalized manifest.

The vendored tree is upstream plus the two patch files below; everything else
is byte-identical to the crate. The upstream `etc/` screenshots and animations
(`*.png`, `*.gif`) are omitted because nothing in the build or documentation
uses them.

## Patches

| Patch | Files | Purpose | Upstream |
| --- | --- | --- | --- |
| `clarion-suboptimal-swapchain.patch` | `src/vulkan/surface.rs`, `src/vulkan/mod.rs`, `src/metal/surface.rs`, `src/gles/{egl,web}.rs` | Accept `VK_SUBOPTIMAL_KHR` from `vkCreateSwapchainKHR`, and recover from a success code that returns no swapchain instead of aborting | [kvark/blade#421](https://github.com/kvark/blade/pull/421) |
| `clarion-warning-hygiene.patch` | `src/vulkan/{init,mod,command,pipeline}.rs` | `u64::MAX`, explicit `'_` lifetimes, no behaviour change | Local only (upstream main already differs here) |

`Context::reconfigure_surface` calls `vkCreateSwapchainKHR` through the raw
entry point because the success code means the swapchain exists but does not
match the surface exactly, so the driver may scale presentation to it; the
next drawable-size change recreates it. Stock blade treats every
non-`VK_SUCCESS` result as an error and aborts the process. NVIDIA's Wayland
WSI returns that code intermittently when the requested extent does not match
the surface's current buffer, which killed `clarion-shell` while it opened a
panel (for example the notification center). The patch keeps the returned
handle, reports the event on stderr (the shell does not install a `log`
logger, so journald keeps it), and no longer panics for a missing handle or a
real error.

The same WSI also intermittently answers a valid creation with
`VK_SUBOPTIMAL_KHR` *and* a null handle, most visibly for a fullscreen
layer-shell surface. `Surface` now keeps that configuration as `pending` and
the renderer retries it from `Context::recover_surface` on the following
frames with a 100 ms → 4 s backoff. Until a swapchain exists the renderer skips
the frame rather than drawing into a missing one; `Surface::has_swapchain`
exposes that state. The other backends (`metal`, `gles`) provide no-op
implementations so the renderer stays backend-agnostic, and the failed
configuration still records its format/alpha so pipelines built before the
first successful creation stay valid.

## Replaying

```sh
crate=$(mktemp -d)
tar xf ~/.cargo/registry/cache/*/blade-graphics-0.7.1.crate -C "$crate"
cd "$crate/blade-graphics-0.7.1"
git apply third_party/blade-graphics/clarion-suboptimal-swapchain.patch \
          third_party/blade-graphics/clarion-warning-hygiene.patch
```

Both patches were checked with `git apply --check` against the pristine crate.

## Upgrading

1. Drop in the new crate sources and manifest.
2. Replay both patches; if a hunk fails, port the change by hand and refresh
   the patch file.
3. Run `cargo check --locked -p clarion-shell`, the layout tests, and open a
   fullscreen panel (launchpad, Spotlight, notification center) on the
   installed shell.
4. Remove a patch once upstream ships an equivalent fix, and update the table.
   The swapchain patch can go once a blade release includes `kvark/blade#421`.
