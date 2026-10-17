# GPUI launcher Phase 0

The Tahoe Apps launcher plan has a hard feasibility gate before the data layer,
full UI, Flutter integration, or deployment. **The Phase 0 gate was accepted on
2026-10-18** after the isolated native and Flutter composition checks below.
The Flutter launcher remains available as a fallback. Phases 1–4 implementation
and subsequent production acceptance are tracked in the
[Apps implementation record](launcher-apps.md); this file records the original
feasibility gate, not a claim that every later production scenario has passed.

## Build and diagnostics

The vendored dependency is GPUI **0.2.2** in `third_party/gpui`, selected through
the root Cargo patch. Use the checked-in lockfile:

```sh
scripts/check-launcher-spike.sh
cargo check --locked -p clarion-launcher
cargo build --locked -p clarion-launcher
cargo +1.88.0 check --locked -p clarion-launcher
```

The helper defaults to read-only diagnostics: tool versions, development
libraries, Wayland globals, and Clarion output/layer IPC. It never opens a window
without `--run`. `--check` and `--build` explicitly select the corresponding Cargo
operation. Dependency resolution and an updated lockfile must be completed before
the `--locked` commands can succeed.

GPUI declares Rust edition 2024 but no `rust-version`. Edition 2024 and GPUI's
let-chain syntax are compatible with the repository's Rust 1.88 baseline in
principle; this is not proof that the entire selected dependency graph is. Check
on the actual 1.88.0 toolchain. If it fails, record the specific package or compiler
error and the tested minimum version before changing the workspace MSRV. Do not
silently raise the compositor's baseline.

## Non-disruptive runtime checks

The automated basic smoke test creates its own disposable nested Clarion and
private D-Bus session, and writes logs, screenshots, and a JSON result:

```sh
python3 scripts/test-launcher-spike.py --artifacts /tmp/clarion-launcher-native
# Include a second output-scale pass; record compositor clamping on narrow outputs.
python3 scripts/test-launcher-spike.py --artifacts /tmp/clarion-launcher-scale2 --scale2
# Real Pinyin composition through a second disposable nested compositor.
python3 scripts/test-launcher-spike.py --artifacts /tmp/clarion-launcher-ime --ime
# Native Flutter Shell glass, Dock layers and keyboard policy handoff.
python3 scripts/test-launcher-spike.py --artifacts /tmp/clarion-launcher-shell --shell-native-test
```

It uses a minimal test configuration without autostarts or Xwayland. Only the
nested compositor connects to the parent display. Before any test input, the
harness discovers and uses private Wayland and Clarion IPC sockets. The private
bus disables service autoactivation to avoid unrelated portals and FUSE mounts.
Two cycles verify the layer IPC,
ASCII entry through virtual keyboard and clipboard round trip, a screenshot,
Escape exit, and layer removal. The Wayland trace must show a single layer role,
no xdg-shell role, 980×680 configure (clamped to smaller output dimensions), and
configure acknowledgement before the first buffer. Invalid-output, timer-only,
and same-process drop/reopen cases check exit behavior and layer cleanup.
Process groups are terminated in cleanup. The default mode does not start the
Flutter Shell or establish backdrop/Dock acceptance.

`--shell-native-test` runs the dedicated Flutter integration test under the same
isolated outer compositor. It uses striped wallpaper to measure real blur,
queries simultaneous Shell/Dock/GPUI layer and keyboard policies, enters text
into GPUI, clicks the real Dock Apps tile, verifies that GPUI retains input,
closes it with Escape, and verifies sharp wallpaper and native policy
restoration. The initial manual backdrop probe has no Flutter launcher panel;
the Dock callback still opens the fallback panel beneath GPUI until the planned
launcher bridge is implemented. Its native pointer fixture is compiled into
the private test directory and can only connect to a distinct explicitly
isolated display.

`--ime` starts Fcitx Pinyin on the private bus and a second disposable compositor.
This extra hop is necessary because Smithay's virtual-keyboard handler bypasses
input-method grabs. A test helper with a fixed US keymap sends real evdev codes
to the first test compositor; the second receives native winit keyboard events,
which Fcitx can process. The harness checks real text-input-v3 preedit/commit
events, a cursor rectangle published before composition, screenshots, and a
clipboard round trip of **你好**. No input is sent to
the parent session. Building this small helper requires `cc`, `wayland-scanner`,
Wayland/xkbcommon development libraries, and the cached virtual-keyboard XML.

The native smoke found and corrected two plan assumptions: requested outputs
must wait for GPUI's initial display discovery, and fixed centered panels use
**no anchors**. Clarion's Smithay layer arrangement stretches opposite anchors
even with a requested size, so the plan's all-edge anchors produced a fullscreen
surface. With no anchors, 980×680 is centered and smaller outputs clamp the size.

The spike's explicit interface is:

```sh
target/debug/clarion-launcher --spike --output eDP-1 --quit-after 5
```

That command creates a real exclusive-keyboard overlay. Do not run it, change
focus, open the live Shell launcher, or inject input into the user's working
session as part of routine checks. First use a disposable nested Clarion
compositor with an independent configuration, runtime/test data, Shell IPC, and
Wayland socket. Start the nested compositor **without `--session`**, which would
otherwise publish its environment and services to the user's session. Run its
Shell with test preferences and its own socket. Do not stop or replace the main
Clarion, Shell, or input-method processes.

For the helper's explicit runtime mode, preserve the original session identifiers
before entering the nested session, then point `WAYLAND_DISPLAY` and
`CLARION_SOCKET` at that test compositor:

```sh
export CLARION_LAUNCHER_PARENT_DISPLAY="$WAYLAND_DISPLAY"
export CLARION_LAUNCHER_PARENT_SOCKET="$CLARION_SOCKET"
# Start and connect to an independent nested Clarion with test configuration.
# Set WAYLAND_DISPLAY and CLARION_SOCKET to the new test session's values.
export CLARION_LAUNCHER_ISOLATED_TEST=1
scripts/check-launcher-spike.sh --run --output winit --quit-after 5
```

Use `clarion msg -j outputs` to select the test output's actual name (`winit` is
only an example). `CLARION_LAUNCHER_PARENT_SOCKET` must be the main compositor's
actual socket path; obtain it from the existing environment or identify it under
`$XDG_RUNTIME_DIR` before starting the test. Merely setting the isolation flag
does not create an isolated compositor. The helper rejects matching or missing
parent/test display and compositor socket values.

While the bounded spike is mapped, use read-only IPC against that same test
compositor:

```sh
clarion msg -j layers
clarion msg -j outputs
```

Expected layer evidence is the launcher namespace on the requested output with
`layer: Overlay` and `keyboard_interactivity: Exclusive`. IPC currently reports
layer/output/focus policy but not bounds, so it cannot prove 980×680 sizing,
centering, presentation, input delivery, or blur. Capture those with a screenshot,
Wayland trace, and explicit runtime assertions. After the spike exits, its layer
must disappear and the test Shell/application must regain its previous focus.

## Environment observed on 2026-10-18

These are prerequisite observations, not acceptance results:

- Main session: Clarion, `WAYLAND_DISPLAY=wayland-1`, with its Flutter Shell and
  `fcitx5` running. No main-session application was opened, closed, or replaced.
- One physical output: `eDP-1`, logical **1920×1200**, physical mode
  **3840×2400**, scale **2**. No multi-output result has been established.
- `wayland-info` advertises `zwlr_layer_shell_v1` version 5,
  `zwp_text_input_manager_v3`, `zwp_input_method_manager_v2`, and
  `ext_background_effect_manager_v1`.
- Active compiler at initial inspection: Rust/Cargo **1.98.0**. The CI MSRV job
  explicitly uses **1.88.0**. The cached GPUI manifest has no declared MSRV.
  Rust **1.88.0** was subsequently installed with the minimal profile and its
  compiler version verified; the default toolchain was not changed. A launcher
  check on that compiler subsequently passed with the checked-in lockfile:
  `cargo +1.88.0 check --locked -p clarion-launcher -j2 --target-dir <separate-target>`.
  The workspace MSRV was not raised.
- `pkg-config` found fontconfig 2.18.3, freetype2 26.7.20, wayland-client 1.26.0,
  xkbcommon 1.13.2, Vulkan 1.4.363, and EGL 1.5. GTK 3.24.52 and gtk-layer-shell
  0.10.1 are available for the Flutter Shell.
- The Flutter SDK is available at
  `/home/liuxue/.cache/realand/toolchains/flutter`; it is not on the default PATH.
- No applicable ancestor or repository `AGENTS.md` was found.

## Acceptance record

The native probe and dedicated Shell composition test demonstrated the Phase 0
requirements, including actual compositor Dock input and retained GPUI keyboard
focus. The later complete production r6 run additionally passed fullscreen
layering, native previous-application keyboard restoration, real production IME
and the complete Flutter bridge. Its evidence is
`work/launcher-production-apps-20261018-r6/result.json`; the
[Apps implementation record](launcher-apps.md) describes its scope. Physical
multi-output selection and output removal remain unverified.
Development evidence is in this session's `work/launcher-native-smoke-20261018-r8`
(native input, scale 2, actual Pinyin, same-process remapping), with a JSON report,
Wayland traces, and screenshots.
The dedicated Shell evidence is in
`work/launcher-native-shell-20261018-r4/shell-native/gpui-launcher-spike-native-evidence.json`:
blur stripe spread **11**, GPUI search delta **2624** pixels, Escape exit **0**,
and restored wallpaper maximum error **0**. A real isolated compositor pointer
click on Dock Apps reached the controller, after which GPUI retained the exact
clipboard text `dock-focus-survives` and its search field changed **1257** pixels.
Shell/Dock remained keyboard `None` while GPUI remained `Exclusive`. The composed
and IME screenshots were visually inspected.

| Check | Required evidence | Status |
| --- | --- | --- |
| Reproducible compile | Locked launcher check/build and actual MSRV check | Passed: Rust 1.88 launcher check; stable build/test/Clippy reported by implementation checks |
| Layer/output/geometry | Overlay IPC, target output, centered 980×680 logical bounds | Passed on isolated single-output Clarion; screenshot inspected |
| Keyboard | Text entry, Enter/Esc delivery, previous focus restored | ASCII entry/clipboard/Esc passed; production r6 also restored real keyboard input to the previous native fullscreen fixture |
| Chinese IME | Actual fcitx composition, candidate selection, committed Chinese text | Passed: Pinyin preedit/candidates/你好 commit, cursor rectangle before preedit, candidate panel visually aligned with field |
| Backdrop and Dock | Test Shell blur behind panel; Dock remains visible and clickable | Passed: measured blur, actual compositor Dock Apps click, retained GPUI keyboard input, restoration |
| Lifecycle | Repeated show/hide/drop, bounded exit, no residual layer/socket/process | Passed: repeated processes; Esc/timer/error exits; same-process 3 maps, 2 last-window drops, 3 destroys |
| Scaling and outputs | Scale 2, narrow output behavior, output selection/removal | Scale 2 and narrow-output clamping passed; physical multi-output/removal pending |
| Fullscreen layering | Apps above a fullscreen test window with focus restoration | Passed in production r6: Apps Overlay covered a native fullscreen fixture and Escape restored native key delivery; physical multi-output coverage remains unverified |

Backdrop/focus acceptance above used the full test Shell. IME acceptance above used
an actual input method connected to the isolated compositor; advertised
text-input protocol alone was not treated as proof. Main-session verification, if performed
later, must be a deliberate visible test after isolated evidence is reviewed.

Clarion currently chooses layer keyboard focus on its active output. In a
multi-output test, explicitly verify launching on a different output and the
intended focus transition. The Shell backdrop must relinquish its exclusive
keyboard policy while the GPUI overlay owns focus; two exclusive overlay
surfaces are not a proven handoff.

If the Wayland role patch becomes large or correct stacking/focus cannot be
demonstrated, stop at this gate and report the failure. Evaluate the plan's Flutter
visual rewrite or compositor-specific xdg-shell fallback before further phases.
