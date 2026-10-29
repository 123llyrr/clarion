# Clarion Smithay patch record

Upstream: https://github.com/Smithay/smithay
Revision: `4cf0b62028039661477d482ec4758b687d8f4392`
Version: 0.7.0
License: MIT (upstream `LICENSE.txt` retained)

All tracked files were copied from the pinned Cargo checkout. The local copy
lives outside cargo vendor's output directory so release packaging cannot
delete it. Nix sources include this directory. `Cargo.toml` at the repository
root applies this local dependency through `[patch]`; the shared Cargo
checkout is untouched.

The vendored tree is exactly the pinned revision plus the two patches below.
Applying both to a pristine `4cf0b62` checkout reproduces
`third_party/smithay/src` byte for byte (verified with `git apply` and
`diff -r`).

## Patches

| Patch | Files | Upstream |
| --- | --- | --- |
| `upstream-2923f22485.patch` | `src/utils/geometry.rs` | Exact backport of [Smithay@2923f2248](https://github.com/Smithay/smithay/commit/2923f2248508307687de4e1d84e4d208b937ad42); drop it once the pinned revision includes that commit |
| `clarion-smithay.patch` | nine files, see below | Local downstream work, not submitted |

### `upstream-2923f22485.patch`

`Rectangle::subtract_rects_many_in_place` walked the original slots forwards
while `swap_remove` moved the last element into the removed slot and splitting
appended fragments. A newly created fragment could be processed twice and an
original rectangle skipped, so subtracting 24 overlapping rectangles from the
same set could leave a nonempty region. `OutputDamageTracker` then reported
damage for an unchanged scene, and clipping could retain pixels meant to be
excluded.

Upstream fixed this on 2026-10-18 by walking the original slots backwards and
added `rectangle_subtract_rects_many_checks_all_rects`. The local copy is that
commit verbatim, test included; it is a backport because the workspace still
pins `4cf0b62`, whose parent predates the fix. Run the upstream test with:

```sh
cd third_party/smithay
cargo test --lib --no-default-features rectangle_subtract_rects_many_checks_all_rects
```

The default feature set also builds the winit backend against whatever beta
the standalone workspace resolves, which can be newer than the pinned
workspace lock; `--no-default-features` keeps that unrelated failure out of
the way.

### `clarion-smithay.patch`

| Area | Files | Change |
| --- | --- | --- |
| HDR / color signaling | `backend/drm/surface/{atomic,mod}.rs`, `backend/drm/compositor/mod.rs`, `backend/renderer/utils/wayland.rs` | Stage connector and CRTC color properties in the same atomic commit as the framebuffer, expose `has_pending_frames`, and invalidate imported textures when a surface commits a new color description. |
| Deferred render retry | `backend/renderer/mod.rs`, `backend/drm/compositor/mod.rs` | `Renderer::is_render_deferred` lets a retry keep output allocations instead of resetting the swapchain on every asynchronous target preparation. |
| Buffer ages | `backend/allocator/swapchain.rs` | `reset_buffer_ages` clears age in place instead of replacing shared slots (which released buffers that were still held), and uses `try_update` to match the newer atomic API. |
| Seat handoff | `backend/session/libseat.rs`, `backend/session/libseat/pause.rs` | Acknowledge `SeatEvent::Disable` only after the pause callback released HDR signaling, without holding the seat borrow across the callback, with a one-second warning and error logging. |
| winit 0.31 beta | `backend/winit/{input,mod}.rs` | Follow the non-exhaustive `MouseScrollDelta`, pointer source and window event enums. |

These are Clarion product requirements (HDR, seat switching, deferred retry)
or upstream API tracking, so they stay a downstream fork. The damage-tracker
regression coverage in `src/render_helpers/damage_region.rs` still applies:
subtract a region from itself, and compare seeded subtraction against an
independent per-pixel coverage oracle.

## Replaying

```sh
base=$(mktemp -d)
git -C ~/.cargo/git/checkouts/smithay-*/4cf0b62 archive HEAD | tar x -C "$base"
cd "$base"
git apply third_party/smithay/upstream-2923f22485.patch
git apply third_party/smithay/clarion-smithay.patch
```

## Upgrading

1. Drop in the new pinned sources and manifest.
2. Replay both patches. If a hunk fails, the upstream revision changed that
   area: port the change by hand, refresh the patch file, and re-verify that
   the two patches reproduce the vendored tree.
3. Delete `upstream-2923f22485.patch` once the pinned revision contains
   `2923f22485`. If it is the only remaining patch, the vendored directory and
   the `[patch]` entry can go too.
4. Run `cargo check --locked`, the damage-region tests, and a session that
   changes output scale/rotation, swaps seats, and toggles HDR.
