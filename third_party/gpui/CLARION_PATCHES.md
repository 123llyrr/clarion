# Clarion GPUI patch record

Base: the published `gpui` **0.2.2** crate from crates.io (Apache-2.0).
The workspace pins `=0.2.2` and `[patch.crates-io]` selects this directory.
Retain the upstream license, production sources, tests and normalized manifest.
Clarion omits the upstream example executables and their demo media; neither the
normalized manifest nor its original-form companion declares example targets.
The Apache-2.0 attribution for the adapted Shell text input remains in
`../../clarion-shell/src/input.rs`; the upstream license remains here.
Production shaders and build resources are retained.

Clarion additions:

- `interactive.rs`: public `ExternalPaths::new` so embedders and tests can
  construct file-drop payloads without exposing GPUI internals.
- `window.rs`: `Frame::clear` also drops the query-only `debug_bounds` map.
  Frames are reused, so without this `VisualTestContext::debug_bounds` kept
  reporting selectors from earlier frames after their element was removed.
- `platform.rs`: public `WindowKind::LayerShell` and copyable role options.
- `platform.rs` / `window.rs` / `platform/linux/wayland/window.rs`:
  `set_glass_region`, a frosted shape independent of the input region. `None`
  falls back to the input region, so callers that only set one shape are
  unchanged. The Dock frosts only its resting tray while lifted icons stay
  clickable outside it. Region requests keep every rectangle up to the protocol
  encoder's bound: scanline masks (one row per rectangle for a tilted Dock
  label) reach a few hundred rectangles, and a tighter cap silently starves the
  shapes reported last.
- `platform/linux/wayland/client.rs`: optional layer-shell global, role event
  dispatch, output resolution, optional xdg parent selection, and an opt-in
  keep-alive policy for applications reopening windows after IPC commands.
- `platform/linux/wayland/window.rs`: mutually exclusive xdg/layer roles,
  layer configuration acknowledgement and resize, authoritative close, ordered
  destruction, and guarded xdg-only operations. Drawing waits for configure.
- `platform.rs` / `window.rs` / `platform/linux/wayland/window.rs`:
  `WindowParams::app_id` carries `WindowOptions::app_id` into window creation and
  the Wayland window sets it on the toplevel before its first surface commit.
  The initial configure therefore already carries the application id, so
  compositor window rules (for example, open-floating) apply to the first frame
  instead of the window being tiled once and resized afterwards.
- `platform/mac/window.rs`: exhaustive matching of the new kind as a normal
  window; the Clarion launcher itself requires Wayland.
- Both manifests: optional `wayland-protocols-wlr` client dependency in `wayland`.

The fixed-size launcher uses no edge anchors, Overlay, exclusive zone -1,
and exclusive keyboard focus. Blur is still owned by the Flutter Shell.
There is no silent xdg fallback when layer-shell is unavailable.

`LayerShellOptions.keep_alive` defaults to `false`, preserving GPUI's existing
quit-on-last-window-close behavior. Successfully opening a layer window with
`keep_alive: true` opts the Wayland client into a sticky lifetime policy: it keeps
processing its foreground executor and Wayland events while no windows exist,
so IPC can open a fresh layer surface in the same process. The application must
call `App::quit` for explicit shutdown. Failed window creation does not enable
the policy. This option is application lifetime behavior and sends no protocol
request; it does not keep any hidden window or layer surface mapped.

The plan initially proposed all-edge anchors. Native testing found that Clarion's
Smithay layer arrangement stretches opposite anchors to the entire output even
with a nonzero requested size. No anchors preserves a centered fixed-size panel
without modifying the compositor's existing Shell arrangement.

The Phase 0 gate passed, and the production Apps daemon uses this patch. See
`../../docs/launcher-phase0.md` for the surface/IME feasibility evidence and
`../../docs/launcher-apps.md` for production integration, runtime acceptance and
remaining hardware checks.
