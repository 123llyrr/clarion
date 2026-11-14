# Clarion Smithay patch record

Upstream: https://github.com/Smithay/smithay
Revision: `2923f2248508307687de4e1d84e4d208b937ad42` (2026-10-18)
Version: 0.7.0
License: MIT (upstream `LICENSE.txt` retained)

All tracked files were copied from the pinned Cargo checkout. The local copy
lives outside cargo vendor's output directory so release packaging cannot
delete it. Nix sources include this directory. `Cargo.toml` at the repository
root applies this local dependency through `[patch]`; the shared Cargo
checkout is untouched.

The pin contains the upstream `Rectangle::subtract_rects_many_in_place` fix
([Smithay@2923f2248](https://github.com/Smithay/smithay/commit/2923f2248508307687de4e1d84e4d208b937ad42),
closing upstream issue #2195), so no backport is needed. The vendored tree is
exactly that revision plus `clarion-smithay.patch`; applying the patch to a
pristine checkout reproduces `third_party/smithay/src` byte for byte (verified
with `git apply` and `diff -r`).

## Patches

| Patch | Files | Upstream |
| --- | --- | --- |
| `clarion-smithay.patch` | eight files, see below | Local downstream work, not submitted |

| Area | Files | Change |
| --- | --- | --- |
| HDR / color signaling | `backend/drm/surface/{atomic,mod}.rs`, `backend/drm/compositor/mod.rs`, `backend/renderer/utils/wayland.rs` | Stage connector and CRTC color properties in the same atomic commit as the framebuffer, expose `has_pending_frames`, and invalidate imported textures when a surface commits a new color description. |
| Deferred render retry | `backend/renderer/mod.rs`, `backend/drm/compositor/mod.rs` | `Renderer::is_render_deferred` lets a retry keep output allocations instead of resetting the swapchain on every asynchronous target preparation. |
| Buffer ages | `backend/allocator/swapchain.rs` | `reset_buffer_ages` clears age in place instead of replacing shared slots, which released buffers that were still held. |
| Seat handoff | `backend/session/libseat.rs`, `backend/session/libseat/pause.rs` | Acknowledge `SeatEvent::Disable` only after the pause callback released HDR signaling, without holding the seat borrow across the callback, with a one-second warning and error logging. |

These are Clarion product requirements (HDR, seat switching, deferred retry),
so they stay a downstream fork. The winit backend follows upstream as-is:
earlier local beta-adapter changes are gone because the pinned revision
already carries them.

Damage-tracker regression coverage lives in Clarion's
`src/render_helpers/damage_region.rs`: subtract a region from itself, and
compare seeded subtraction against an independent per-pixel coverage oracle.
The upstream regression test for the loop fix is in `src/utils/geometry.rs`
and can be run from the vendored workspace with:

```sh
cd third_party/smithay
cargo test --lib --no-default-features rectangle_subtract_rects_many_checks_all_rects
```

The default feature set also builds the winit backend against whatever beta
the standalone workspace resolves, which can differ from the pinned workspace
lock; `--no-default-features` keeps that unrelated failure out of the way.

## Replaying

```sh
base=$(mktemp -d)
git -C ~/.cargo/git/checkouts/smithay-*/2923f22 archive HEAD | tar x -C "$base"
cd "$base"
git apply third_party/smithay/clarion-smithay.patch
```

## Upgrading

1. Move both `smithay` and `smithay-drm-extras` workspace pins to the new
   revision, then run `cargo fetch` to refresh `Cargo.lock`.
2. Drop in the new pinned sources and replay `clarion-smithay.patch`. If a hunk
   fails, port the change by hand, refresh the patch, and re-verify that it
   reproduces the vendored tree.
3. Run `cargo check --locked`, the damage-region tests, and a session that
   changes output scale/rotation, swaps seats, and toggles HDR.
4. If a change lands upstream, delete it from the patch. When the patch is
   empty, remove the vendored directory and the `[patch]` entry.
