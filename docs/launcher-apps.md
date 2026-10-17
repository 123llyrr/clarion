# Tahoe Apps launcher: implementation and acceptance

Recorded on **2026-10-18**. The independent `clarion-launcher` implementation now
covers the process/data layer, GPUI panel, Flutter bridge and package/install
integration from Phases 1–4. This document separates those implementation changes
from the runtime acceptance evidence. Updating source or preparing an installation
does not replace the running desktop.

The [Phase 0 gate](launcher-phase0.md) was accepted after isolated Clarion tests
of the Wayland role, scale, real Chinese IME, same-process window recreation,
Shell glass and Dock interaction. The final isolated production release run
passed **21 checks**, including real IME, the release Shell's global F4 path,
fullscreen/focus and lifecycle cases. The earlier **r6** run additionally covered
the full Flutter bridge/fallback integration test. Physical multi-output,
hardware gestures, package builds and activation in the physical session remain
separate acceptance scopes.

## Process and surface ownership

- Flutter Shell owns the backdrop, Dock, shared preferences and keyboard handoff.
- `clarion-launcher` owns its XDG application snapshot, search, GPUI panel, launch
  requests and independent Apps view state. It does not own compositor resources.
- Vendored GPUI **0.2.2** owns the client's input, text-input-v3 and GPU renderer.
  The patch creates either a layer role or an xdg role, never both on one surface.
- GIO owns Desktop Entry argument expansion, terminal handling and D-Bus activation.
  Application helpers run in the existing user-manager `app.slice` scope when a
  usable manager exists.

The initial panel is **980×680 logical pixels**, centered on the selected output,
with Overlay placement, exclusive zone `-1` and Exclusive keyboard policy. The
Shell backdrop and Dock use keyboard `None` while Apps owns focus. Hiding drops
the GPUI window, including its layer role; an explicit `keep_alive` option leaves
the application event loop and IPC service running for subsequent Show requests.

The plan initially specified all-edge anchors. Native testing found that Clarion's
Smithay layer arrangement stretches opposite anchors even with an explicit size.
The implementation therefore uses **no anchors** to preserve a centered fixed-size
panel, with compositor clamping on smaller outputs. Native panel dragging and
interactive resizing are not implemented; saved width/height specify the initial
size. This does not modify the compositor's established Shell arrangement.

## Catalog, search, icons and launch behavior

`catalog.rs` scans `applications/**/*.desktop` in ordered, deduplicated XDG data
roots. An absolute `XDG_DATA_HOME` takes priority; otherwise an absolute HOME's
`.local/share` is used. Absolute `XDG_DATA_DIRS` entries follow, with
`/usr/local/share:/usr/share` as the default. Nested desktop IDs replace `/` with
`-`. File symlinks are supported; directory symlinks are not traversed.

A higher-priority ID masks lower-priority copies even if the first entry is
Hidden, NoDisplay, unavailable, malformed or too large. Reads are capped at 1 MiB.
Type, desktop visibility, DBus activation, TryExec and executable availability
follow the existing Dart scanner. Executable checks inspect files and PATH; they
do not run Desktop Entry content. Relative working-directory-dependent commands,
field codes and unfamiliar `env` options remain delegated to GIO.

Name, GenericName, Comment and Keywords use locale fallback from LC_ALL,
LC_MESSAGES and LANG, including territory/modifier and language fallbacks.
Search is case-insensitive and requires every whitespace-separated term to match
localized metadata. Chinese input is supported through GPUI's text input field;
Pinyin transliteration of application names is not synthesized.

Every app has one primary category: 工具、生产力与财务、社交、创意、信息与阅读、娱乐、其他。
Specific XDG subcategories take precedence over broad groups—for example a
WebBrowser belongs to 信息与阅读 and an AudioVideo Player belongs to 娱乐.
Suggested filters the Shell's newest-first recent IDs against currently installed
apps, preserving order and removing duplicates. Category sections initially show
at most eight items and expose an expand/collapse control when needed.

`icons.rs` reads GTK 3/4 icon-theme settings, follows theme inheritance with cycle
protection, and falls back to `hicolor` and pixmaps. It resolves PNG/SVG files and
caches filesystem/theme lookups. The catalog initially requests 128 physical
pixels, suitable for a 64-logical-pixel icon at scale 2; it does not multiply that
size again. Artwork from the first matching theme stays authoritative, while an
adequate-size raster/vector in that theme is preferred. XPM is excluded because
GPUI's decoder does not support it. Missing artwork uses a generic app tile.

Launch passes one literal desktop-file path to `gio launch`. Before invoking an
application, the helper checks user-manager presence and systemd-run capabilities.
Modern scope helpers must support `--expand-environment=no`; older supported
helpers retain their direct-argv semantics. Probe output is retained up to 64 KiB,
probes have a three-second limit and the launch helper has a ten-second limit.
Stdout/stderr inherit the caller's descriptors.

The plan's broad “fall back directly after scope failure” wording is narrowed to
match existing Shell behavior: **only an absent manager permits a direct launch**.
Probe failure or any attempted launch failure is reported; no second application
invocation is attempted. Retrying after a possible invocation could open it twice.

## View state and Flutter integration

The panel implements category/name modes, icon/list views, text filtering,
selection, arrow navigation, Enter/click activation and Escape dismissal.
No-match and empty catalogs retain a working search field and dismissal path.
Suggested/category copies of an app have separate scroll anchors, so keyboard
navigation targets the appropriate occurrence. A pending launch is guarded
against repeated Enter; completion releases the busy state even after Hide/reopen.

`apps-view.json` stores `mode`, `display`, `width` and `height`; its defaults are:

```json
{"mode":"category","display":"icons","width":980,"height":680}
```

Writes use a serialized background queue and atomic replacement. The file is
independent of `launchpad-layout.json`: Apps does not import custom folders or
manual icon order. Both Shell and launcher honor the existing configuration
namespace rule: canonical `clarion-shell` wins; an existing `realand-shell` is
used when the canonical path is absent. The launcher reads shared
`preferences.json` and never rewrites it. Successful launch events return to the
Shell, which remains responsible for persisting recent application history.

`use_external_launcher` defaults to `true`. Setting it to `false` in Shell
preferences selects the retained Flutter implementation. Missing binaries,
startup/transport failures and window errors fall back through the Shell's
existing panel path. Launch errors remain visible without silently launching
another app. Dock Apps, F4, gestures, Spotlight and Shell launcher commands use
the common controller entry point rather than maintaining separate visibility
states.

The default compositor configuration binds unmodified **F4** to
`clarion-shell-msg launcher`; a built-in fallback also supports older user
configurations without that binding. Configured bindings retain precedence, and
the fallback does not repeat or bypass lock/inhibition policy. A completed
three- or four-finger inward pinch with final scale at most `0.75` opens the same
entry point. Two-finger zoom is untouched; cancelled, invalid, locked or inhibited
gestures cannot trigger Apps. Sixteen compositor input regression tests passed.
Actual touchpad gesture delivery remains a hardware acceptance item; the updated
release's global F4 path passed in an isolated nested desktop with the real
Shell command server and an empty user binding configuration. It has not been
exercised in the active desktop.

Each Show queues a background catalog Reload before opening, so a persistent
daemon discovers applications installed since its previous opening. Reload
revisions prevent an older scan from replacing newer data. The Shell allows
**eight seconds** for a Visible acknowledgement; a cold release run took
4.906 seconds, so the previous four-second deadline was too short. A missing
acknowledgement still restores the retained Flutter panel.

The bridge prefers an executable absolute `CLARION_LAUNCHER_BIN`. Otherwise it
resolves the running Flutter executable's real path and searches its directory,
its parent, the parent's `bin`, then absolute PATH entries. The installer puts
`clarion-launcher` beside `clarion-shell` in one versioned release. A running old
Shell therefore continues using its own release after a new one is installed.

## IPC and lifecycle

`$XDG_RUNTIME_DIR/clarion-launcher.sock` is a mode-0600 Unix socket guarded by a
single-instance lock. A duplicate daemon preserves the existing socket and exits.
The transport accepts up to 32 peers and limits each command line to 64 KiB.

| Direction | Messages |
| --- | --- |
| Shell → launcher | `Show`, `Hide`, `Toggle`, `Reload`, `Preferences` |
| Launcher → Shell | `Visible`, `Hidden`, `Launched { app_id }`, `Error { context, message }` |

Commands/events carry `request_id`. Show/Toggle/Preferences can include an output
name, theme, accent and recent IDs. The launcher resolves output names through
Wayland display identity and reports invalid output selection instead of silently
mapping an xdg window. Each connection has a server-side peer ID; replies go to
that peer, so identical local request IDs from different Shell connections do not
cross-deliver events. A new owner's Show closes the old owner's view; an inactive
peer's Hide does not close the active owner's window. Owner disconnection hides
its panel.

Window epochs reject obsolete Show/UI actions after Hide or replacement. A launch
already in progress still reports its real completion to the initiating peer,
but its result cannot hide a newer panel. Catalog reload revisions reject an older
scan finishing after a newer request. View-state saves are serialized to preserve
request order while keeping filesystem I/O off GPUI's foreground thread.

When started by a Shell, `--parent-pid` installs a parent-death signal. SIGTERM and
SIGINT cause explicit GPUI shutdown and socket cleanup. Socket ownership transfer
does not transfer the original parent lifetime: the original spawning Shell still
owns its child process. Continued daemon survival after that creator exits while
a different Shell has adopted the connection remains a multi-Shell limitation.

## Build, tests and installation

Use the checked-in lockfile and preserve the workspace MSRV of Rust **1.88**:

```sh
cargo check --locked -p clarion-launcher
cargo build --locked -p clarion-launcher
cargo test --locked -p clarion-launcher
cargo clippy --locked -p clarion-launcher --all-targets -- -D warnings
cargo +1.88.0 check --locked -p clarion-launcher

env -u WAYLAND_DISPLAY -u DISPLAY cargo test --locked -p clarion-launcher \
  --bin clarion-launcher ui::headless_tests -- --test-threads=1
python3 -m unittest scripts.test_packaging_sources scripts.test_identity_compat scripts.test_session -v
```

The headless tests use GPUI's actual TestAppContext, rendered input handlers and
key dispatch, with private catalog/config fixtures and no display connection.
They exercise English/Chinese filtering, arrow/Enter/Escape requests, mode/display
persistence, empty/narrow catalogs and Suggested/search scroll-anchor selection.
Catalog/icon fixtures cover XDG precedence, executable checks, Unicode metadata,
symlink exports and theme inheritance; launch fixtures verify literal argv,
capability handling, bounded output and failure/timeout behavior.

At this record, **41 launcher Rust tests**, including **five GPUI headless tests**,
passed. The packaging/identity/session subset passed **19 Python tests**; the
repository's complete Python suite passed **108 tests**, and the module-boundary
check passed. Current launcher Clippy across all targets with `-D warnings`, the
package formatting check and the actual Rust 1.88 production check all passed.
The Flutter Shell passed **1,090 tests**, analysis and its release build. The
Rust compositor and launcher release build also passed (8 minutes 14 seconds).
These are reported implementation checks; the separate native run below supplies
runtime evidence.

For a release generation, build both clients and supply the matched launcher:

```sh
cargo build --locked --release -p clarion -p clarion-settings -p clarion-launcher
scripts/build-shell.sh --release
python3 scripts/install-shell.py \
  --bundle clarion-shell/build/linux/x64/release/bundle \
  --launcher-binary target/release/clarion-launcher
```

Replace `x64` with the actual Flutter build architecture. Installation validates
both products, copies the Apps binary into the same immutable Shell release,
retains the previous generation and records hashes/backups. Failed finalization
restores Shell, Apps and command aliases together. It neither enables nor starts
services and does not restart the active session. Final hash/receipt failures also
restore the public entry points; both failure paths have regression tests.
`install-session.sh`, RPM/DEB
assets and the RPM spec include the launcher binary. Nix includes the workspace,
vendored GPUI and embedded resources, builds with the lockfile, installs the
launcher and retains fontconfig/freetype/Wayland/xkbcommon/Vulkan/GL libraries in
its build/runtime closure. GPUI's headless `test-support` also compiles its X11
backend: Nix includes `xorg.libxcb`, while `libxkbcommon` supplies xkbcommon-x11.
The selected x11rb/xim features do not require libX11. Nix, RPM and DEB build tools
were unavailable, so actual package builds remain unverified; source/install
checks establish their inputs and staging behavior.

## Native evidence and remaining acceptance

The session evidence root is
`/home/liuxue/Documents/Codex/2026-10-18/zhe/work`.

- Phase 0: `launcher-native-smoke-20261018-r8/result.json` records geometry,
  scale 2, real Pinyin composition/candidates/你好 and same-process recreation.
- Shell composition: `launcher-native-shell-20261018-r4/shell-native/`
  `gpui-launcher-spike-native-evidence.json` records measured blur, a real Dock
  Apps click, retained GPUI keyboard input and restoration.
- Complete production acceptance: **`launcher-production-apps-20261018-r6/result.json`
  passed** with `--toolbar --fullscreen --ime --shell-native-test`. It covers
  Suggested, literal special-character Desktop Entry paths, English/Chinese
  filtering, real Down/Enter and app-tile pointer activation, Launched/Hidden,
  same-PID reopen/Escape, rapid visibility epochs, duplicate socket preservation,
  launch failure without a second invocation, Reload and Toggle. All four toolbar
  controls persisted independent view state and shared preferences stayed unchanged.
- The same r6 report passed two-peer identical-request-ID ownership transfer,
  inactive-peer Hide rejection, owner-disconnect hiding, graceful exit/socket
  cleanup and killed-parent cleanup. Apps covered a native fullscreen fixture;
  Escape restored actual native keyboard delivery to that application.
- Production IME used real Fcitx Pinyin: preedit/candidates committed **你好** and
  activated the matching fixture through fake GIO. The name/list and candidate
  alignment screenshots were visually inspected. The full Flutter bridge test
  exercised a real Dock click, startup/launch failure handling, retry, success,
  recent history, Escape and `use_external_launcher=false` legacy Exclusive focus
  followed by `true` with Shell keyboard None and GPUI ownership. The first Show
  took **3.115 seconds**. Later cold release testing justified the current
  **eight-second** acknowledgement timeout. Production r6 ran at scale 1; scale 2 remains covered by the prior Phase 0
  native record. Tests use private Desktop Entries and fake GIO, so no installed
  application was launched.
- Release acceptance: `launcher-production-apps-release-20261018-r7/result.json`
  and `launcher-production-apps-release-20261018-r8/result.json` passed all 20
  requested checks, including global F4 opening and closing through the release
  Shell's real private command server. The r8 run used the eight-second deadline;
  both runs preceded the subsequent per-Show catalog Reload addition, which has
  its own Flutter regression tests and updated release build.
- Final release acceptance:
  **`launcher-production-release-final-20261018/result.json` passed all 21 checks**
  with `--toolbar --fullscreen --ime --f4`, release core/launcher and the latest
  eight-second/Reload Shell. First Show took **2.410 seconds**, and the real F4
  route opened its initially empty-cache daemon in **0.902 seconds**. Preedit
  arrived within the **500-millisecond** bound (measured waiting after typing:
  **0.001 seconds**); the fixture then committed 你好 and launched through fake
  GIO. The report also confirms exit code zero, socket removal and cleanup.

The isolated native harness can be run with:

```sh
python3 scripts/test-launcher-apps.py --artifacts /tmp/clarion-apps-native --toolbar
# Optional extended cases, with the required private fixture tools installed:
python3 scripts/test-launcher-apps.py --artifacts /tmp/clarion-apps-extended \
  --toolbar --shell-native-test --ime --fullscreen --f4
```

It starts disposable nested Clarion/private D-Bus instances and cleans its own
process groups. The real-IME mode uses private Fcitx Pinyin; the fullscreen mode
uses a native fixture and tests returning a real key to it after Escape. These
modes do not inject input into the parent user's session. Logs, screenshots,
protocol traces and `result.json` provide the evidence for each run.

Additional nested Vulkan testing exposed an **existing Winit/WSI limitation** on
the NVIDIA GeForce RTX 2050: after three presented frames, image acquisition
remained pending. FIFO barriers, window visibility and server-side buffer leases
were ruled out; the parent sent releases for replaced buffers. The exact
driver/private-event-queue cause remains unresolved. The already-installed core
binary reproduced the same behavior in an isolated control run
(`launcher-production-ime-old-core-20261018-r17`, 0.804-second preedit delay),
so no speculative WSI change is included in this launcher implementation.

The active desktop uses **Intel Iris Xe with direct TTY/KMS submission**, which
does not use that WSI path. The nested Vulkan IME screenshots are readbacks of the
inner scene and do not prove continuous visible presentation of its host window.
They also cannot establish actual TTY/KMS acceptance. Keep this existing nested
backend limitation separate from the passing GLES-hosted launcher acceptance.

Physical multi-output selection/hot removal, activation across different active
outputs, hardware touchpad gestures, actual Nix/RPM/DEB builds and exact Tahoe
visual matching remain separate checks. Fullscreen layering and native focus
restoration passed in the isolated production fixture. Clarion arbitrates
exclusive layer focus on its active output; selecting another output alone does
not prove a correct focus handoff. The panel remains fixed and centered. These
isolated tests do not restart the main session or build/install Nix, RPM or DEB
packages.

After acceptance, the matched source-built generation was installed as
`~/.local/lib/clarion/releases/clarion-gpui-apps-20261018-023716-179616`, with the
standalone Shell/Apps release at
`~/.local/lib/clarion-shell/releases/20261018-023717-526657`. Core, Shell, Apps and
the actual service release's `lib/libapp.so` hashes were independently verified.
The previous generations and entry-point backups remain available. The core and
Shell processes stayed at PIDs **1275** and **1349** with their previous executable
paths; no service was restarted. The installed core/Shell become active on the
next login. The task's `outputs/Clarion-Apps-installation.json` records hashes,
previous paths and the unchanged active processes.
