# Clarion desktop shell

`clarion-shell/` is the Clarion-owned desktop shell. Its Flutter Material 3
widgets replace the external shell's UI responsibility while Clarion retains
Wayland clients, input routing, workspace/window geometry and Vulkan/GLES
composition. The shell is an independent Wayland process, so its UI toolkit
does not become a second owner of compositor GPU resources.

Applications launched from the launcher, Dock folders and desktop files use
GIO inside an independent user-manager `app.slice` scope. Launch preparation
probes systemd-run capabilities before invoking GIO, keeps literal arguments and
the caller's environment, and never retries after a possible launch. Application
stdout/stderr inherit journal descriptors instead of a temporary shell pipe, so
replacing the shell does not terminate its applications or break their output.
An absent user manager permits one direct invocation; errors from an existing
manager are reported to the user. Wallpaper remains owned by the shell.

The default configured Apps implementation is the independent Rust/GPUI
[`clarion-launcher`](../clarion-launcher) process. Its 980×680 initial overlay
uses Tahoe's seven fixed categories, Suggested, category/name modes and
icon/list views. The Flutter Shell owns the blurred backdrop and Dock; its
expanded main surface relinquishes keyboard focus while the GPUI surface
requests Exclusive focus. Closing Apps restores the Shell's policy. The
[Phase 0 record](launcher-phase0.md) contains the accepted isolated geometry,
real Chinese IME, window lifecycle and Shell/Dock composition evidence.
The complete isolated production r6 run also passed real IME, Flutter Dock
integration, ownership/lifecycle handling, fullscreen layering and native focus
restoration. Physical multi-output, hardware gestures, actual package builds and
pixel-exact Tahoe matching remain separate checks. The
[Apps implementation record](launcher-apps.md) documents Phases 1–4, transport
semantics, package preparation and the detailed production evidence.

The bridge connects to `$XDG_RUNTIME_DIR/clarion-launcher.sock`, starts a hidden
`--serve` instance on demand and sends bounded JSON-lines requests. The GPUI
window can be dropped and recreated without ending the process. GIO retains
Desktop Entry execution and D-Bus activation; failed scope launches never retry
an application. Recent IDs are read from the Shell's shared preferences and
reported successful launches return to the Shell for persistence.

Set `"use_external_launcher": false` in the Shell's `preferences.json` to select
the retained Flutter implementation when preferences are loaded. The Rust
panel saves only `mode`, `display`, `width` and `height` in `apps-view.json`
(defaults: category, icons, 980, 680). It does not import or overwrite the
fallback's folder/order file `launchpad-layout.json`. Both stores follow the
existing canonical `clarion-shell` / legacy `realand-shell` namespace selection.

Build and prepare a matched deployment without changing a running session:

```sh
cargo build --locked --release -p clarion-launcher
scripts/build-shell.sh --release
python3 scripts/install-shell.py \
  --bundle clarion-shell/build/linux/x64/release/bundle \
  --launcher-binary target/release/clarion-launcher
```

Use the architecture-specific bundle path. The installer validates the Apps
binary before writing and copies it into the same immutable release directory
as `clarion-shell`. Existing releases and launcher/command backups remain
available, and failed finalization restores the previous entry points together.
No install step enables, starts or restarts a service. The bridge first honors
an executable absolute `CLARION_LAUNCHER_BIN`; otherwise it searches the running
Flutter executable's directory, its parent, the parent's `bin`, then absolute
PATH entries. Resolving the running Shell's symlink first keeps subsequent Apps
opens pinned to that generation even after another release is installed.

## Switchable layouts

Control center → Appearance → Desktop shell selects Material or macOS. Existing
preferences remain Material, including the saved top/bottom bar edge. macOS
uses a top menu bar with configurable 22–48 logical-pixel height (22 by default)
and a Dock on the bottom, left or right. Each
surface uses the same Flutter engine, controller and backend services through
[`ViewCollection`](https://api.flutter.dev/flutter/widgets/ViewCollection-class.html).
`ShellHost` renders the additional native view without starting notification,
tray, IPC or device services again. Only the main application owns controller
disposal. Hidden Dock views are reused across switches; native teardown removes
them when the main window closes.

Panels share their existing controls through `ShellPanelContent`. The retained
Flutter fallback has separate Material and macOS launcher layouts with common
catalog and launch services.
The Dock shows installed pinned apps and actual running windows. Clicking a
running app focuses its existing window, including another workspace/output;
its compact icon-anchored menu lists individual windows, minimize/restore,
pin/remove, the application-entry directory and an accurately named close-all-windows action. A fallback macOS
launcher tile can also be right-clicked to pin an app before launching it.
App identities use desktop IDs, executable names and StartupWMClass; unknown
running windows remain focusable without becoming an unlaunchable pinned app.

The Dock chooses a fixed native drawing thickness for the current size and
magnification preferences, with a minimum of 120 logical pixels. Hover does not
resize that surface. The exclusive reservation is the configured base icon size
plus 16 pixels (64 at the default 48), or zero with automatic hiding. Magnified
icons and their drag targets share the same transforms and remain within the
drawing surface. Resting icons adapt down to 24 pixels to fit smaller outputs.
Larger app collections scroll along the Dock axis; reduced motion skips animation.
Explicit clear-frame display lists prevent stale transparent buffers after
hover/menu disappearance. Native input follows the resting glass and the currently painted, clipped icon
bounds after each animation frame. Transparent space reserved for future
magnification or bounce passes through to windows below; drawing thickness is
independent of pointer activation. Flutter previews use the same visible hit
regions, and automatic hiding retains its separate two-pixel edge sensor. Input
becomes empty while a modal control panel or recent-content panel is active; macOS
Launchpad keeps the Dock interactive. Keyboard mode stays None.
The menu reservation follows the configured height, or zero with automatic hiding. Desktop icons,
page controls and tooltips use the configured menu and Dock edges, including
left/right reservations, after an atomic preference update. A failed Dock startup
restores Material with a visible explanation;
macOS preferences wait for native style acknowledgement and the Dock first frame.

In macOS mode the two visible surfaces use layer-shell `exclusive=-1` to position
against the whole output. Independent transparent, input-free GTK surfaces named
`clarion-shell-menu-reservation` and `clarion-shell-dock-reservation` reserve only
the configured desktop edges. They have no Flutter view, keyboard focus or
background effect. This avoids centering a side Dock inside the menu bar's
reduced working area, or squeezing the menu and modal surface into the Dock's
reservation. Automatic hiding and Material mode unmap the applicable reservation;
output replacement and destruction update or destroy both surfaces together.

Control center → macOS settings provides menu-bar height (22–48 logical pixels), automatic
hiding, battery percentage and clock date/seconds, plus Dock position, icon size
(32–64), magnification (100–200%), automatic hiding, running indicators, launch
bounce and utility visibility. These choices persist under the optional `macos`
object in the shell preferences; old files remain readable and the Material bar
edge remains independent. Height updates the native strip, blur, input, menu
anchors and desktop reservation together; labels and icons keep their existing
size and are vertically centered. The legacy width percentage remains readable
and is preserved in saved files, but has no settings control. Configurable menu
height is a Clarion extension, not a recovered Catalina preference. Automatic hiding retains a two-pixel sensor on the strip's
top edge or the Dock's selected edge. Open menus keep their surface visible.
Fixed applications can be reordered by a 350 ms press and drag; Finder retains
its first position. A separate, optionally visible recent-app section shows at
most three installed apps from successful launch and actual focus history. Apps
already displayed as pinned/running are excluded to avoid duplicate icons;
this placement policy is not proof that Catalina's recent section requires an
app to have exited. The history is bounded and persisted independently of pins.
Mission Control has no button in the bar or Dock. Existing keyboard access to
the workspace overview remains available.

Downloads now reads the real XDG directory through `DockFolderService`: bounded
one-level enumeration, coalesced watching, directory deletion/recreation recovery
and literal GIO opening. Its closed icon independently selects Folder or Stack;
the content selects Automatic, Fan, Grid or List, with real item selection,
keyboard navigation and an open-folder action. Fan uses a bottom anchor with few
items and enough space; side/limited layouts use Grid. Name, kind and real mtime
sorting are available; creation/arrival dates are disabled without true metadata.
Downloads right-click opens the stack panel and its options, rather than claiming
to reproduce the entire native directory context-menu hierarchy. Menus/stacks
keep an auto-hidden Dock visible. Trash uses GIO's virtual trash and local watches
plus polling to update its original empty/full icon; empty-trash and volume eject
actions are not yet provided.

The [current Dock comparison](macos-dock-detail-comparison.md) distinguishes
implemented behavior from missing window thumbnails, application lifecycle/menu
capabilities, cross-output target ownership and unmeasured Catalina parameters.
Raw mirror evidence remains in the [image research](macos-menu-dock-research.md).

Both native shell surfaces opt into `ext-background-effect-v1`: the menu bar
requests its configured-height strip and the Dock reports only its visible rounded glass,
not its transparent magnification margins. Discovery runs asynchronously on GDK's
Wayland queue. Region changes are committed with Flutter's next frame; Material
mode clears them, and unrealizing or removing a surface destroys its effect.
GLES and native Vulkan use the compositor's existing blur path. Explicit protocol
regions sample actual windows behind the glass by default; a configured `xray`
rule remains authoritative. Screenshot/recording privacy stays in the compositor.
macOS Launchpad temporarily requests blur over the main output-sized surface,
including its retained exit frames. The Dock moves to the Overlay layer for those
frames so its icons remain above the blurred scene; interaction is disabled
during exit and when another panel takes over. Closing or switching to Material
restores the ordinary strip region and Dock layer.
Launchpad covers the menu strip and hides its controls through the retained exit;
the menu returns when the launcher finishes or another control panel opens.
Independent protocol tests exercised GLES and Vulkan on an NVIDIA RTX 2050 at
scales 1, 1.25 and 2, including region clearing/remapping, a second simulated
output and protected screenshot/recording pixels. These fixtures do not establish
physical display hotplug or support across other GPU and driver combinations.

## Recent content and application menus

Both bar layouts offer a **Recent content** button for clipboard history and
recent screenshots. The strip shows only an icon and item count; copied text,
image previews and file names appear only inside the opened panel. The macOS
bar retains the focused application's name as a plain label. It does not expose
an application-name menu or synthesize File/Edit/View menus.

The shared panel searches text, source and capture date, with All, Text, Images
and Screenshots filters. Images includes screenshots. Pinned items appear first,
then newer records. Copy restores the selected payload to the clipboard; Open
uses the screenshot's existing file. Delete removes one history item and Clear
removes unpinned history. Neither operation deletes screenshot files. Pinning
protects an item from normal eviction and clearing, but does not persist it
across a Shell restart. Escape closes the panel and Ctrl+F focuses search.
The panel and its history scroll within the available output height.

History stays in the running Shell process: it is not saved to disk. The default
limits are 80 records and 64 MiB of retained payloads, with up to 1 MiB per text
record and 16 MiB per PNG. Duplicate payloads are coalesced by content hash.
Thumbnails are generated separately at a maximum of 320 pixels. Pause stops new
recording while preserving existing items; the bar shows the paused state.
An unavailable clipboard connection keeps existing history visible and offers
reconnection, while copying is disabled until the transport is available.

The Linux runner uses `ext-data-control-v1` asynchronously, with bounded transfer
sizes; it does not poll a focused application's clipboard. Successful compositor
screenshot completion events add recent screenshots, while `is_preview` captures
are ignored. Shell preview and demo sessions disable collection. The installed
session starts one `--monitor=0` Shell engine; additional views do not create
another collector. Independently started Shell processes keep independent
in-memory histories; shared history across multiple Shell processes is not
implemented.

Application commands now remain in the application's own menu bar or header-bar
menu. Clarion no longer hosts a D-Bus global-menu registrar, renders GTK/Qt
exported application menus, or advertises Qt's appmenu-manager protocol. GTK
Wayland identity integration remains available for matching windows to their
application; it does not request clients to hide menus. StatusNotifier tray
items retain their native D-Bus menu support independently of application menus.
Dock app menus still provide Clarion-owned window actions such as focus,
minimize/restore, pin and close-all-windows.

A real session removes only the absolute GTK exporter module/schema paths
previously managed by Clarion from its launch and activation environments.
Other GTK modules and custom schema directories are preserved; installed
exporter files and release backups are retained for rollback. The retired
`scripts/install-gtk-appmenu.py` entry point reports retirement without installing
or removing files. Existing application processes may have cached their old
menu-export state; start them again after logging into the updated session to
use their normal in-window menus. No Qt platform theme is forced.

The installed systemd Shell unit starts after `graphical-session.target` and
before `xdg-desktop-autostart.target`. GTK can synchronously activate a Portal
proxy while registering its application; starting the Shell after the graphical
target avoids that activation cycle. The Shell notifies the manager after its
first rendered frame, without waiting for menu discovery. Preview mode sends no
session-readiness notification. `CLARION_SHELL_STARTUP_TRACE=1` records native
entry, GTK registration, view creation and the main/Dock first frames using a
monotonic clock. It is off by default and does not alter the session environment.

The Dock context menu offers real minimize/restore when the compositor advertises
minimized-window state. A minimized tile retains its stable window/workspace ID,
commits and lifetime, while leaving ordinary focus, input and tiled layout. The
shell registers visible icon geometry in output-local logical coordinates. GLES
and Vulkan use the same animation mesh for the actual window snapshot, with separate
output/screenshot/recording targets. Restore reverses current progress and returns
to the original layout position where possible. Missing targets, reduced motion,
lock and changed output geometry safely finish the state change. Older running
compositors keep the existing Dock functionality until the next login.
The macOS appearance settings select **Genie** or **Scale** for minimization.
Genie contracts toward the registered bottom, left or right Dock edge (the runtime
target also supports top); Scale preserves the window's aspect ratio as it moves
into the icon. Old preferences and target payloads default to bottom/Genie.
Changing the effect republishes existing icon geometry; a late target reply
cannot suppress the newer choice. Both effects honor reduced motion and the
existing animation cleanup rules. The curves are an original implementation;
pixel equality with Catalina's private Dock animation has not been established.
Mirror evidence, target compatibility and verification scope are documented in
[`macos-minimize-effects.md`](macos-minimize-effects.md).
Opening a control panel or the Dock context menu blocks ordinary Dock input but
retains the targets of icons that remain visible, allowing the context-menu
minimize action to use its icon destination. Hidden or unsettled Dock icons,
off-screen icons and Material mode clear their targets.
When no explicit output name is supplied, the shell matches native monitor
geometry against the compositor's output snapshot. It requires one exact match;
ambiguous, stale or missing geometry clears animation targets instead of guessing
the focused output. Reconnection waits for fresh ownership before publishing
new icon rectangles.
This local build includes Apple logos, system icons and SFNS/PingFang fonts
recovered from the user-supplied Catalina image. Full Core.pkg resources replace
BaseSystem's smaller Finder, Trash, folder, Terminal and Safari sources with
unchanged 1024px PNG chunks. TextEdit, Calendar, Preview and Font Book also use
1024px originals; Calculator, Screenshot and Activity Monitor use their actual
largest 256px PNG chunks. Launchpad/Mission Control remain 1024px originals.
Selected ICNS files match their embedded CodeResources SHA-1 and SHA-256 entries;
this checks resource integrity, not the incomplete image or Apple certificate trust.
Sources and SHA-256 checksums are recorded in
`clarion-shell/assets/shell/provenance.json`; macOS applications are never run.

`MacAppIcons` resolves exact desktop IDs or semantic icon names, and `MacAppIcon`
applies that same result to Dock, Launchpad/search, folder previews and drag feedback.
Application names/actions remain their real Linux counterparts. Known third-party
brands keep native artwork; unsupported roles also stay native. System Preferences
could not be recovered from the missing image tail, so Settings stays native.
Calendar's original asset has a fixed JUL/17 date; dynamic date rendering is not
implemented. The macOS settings switch `use_original_icons` defaults to true for
old files, supports live changes and remains independent from Material.

Icons resolve and decode for physical pixels, including output DPR and maximum
configured Dock magnification. Hover reuses that texture and size upgrades retain
the previous image while loading. Rasters are only downsampled; SVGs remain vectors.
Missing/corrupt native sources use the original generic application icon in macOS
mode. GTK lookup preserves theme/style priority and checks actual raster dimensions
before accepting a smaller threshold match. Legacy callers without `pixelSize`
keep their logical-size/GTK-scale behavior.
Use `--preview --shell-style=macos` for a single-window preview that does not
save the explicit style override. Runtime changes do not restart Clarion or
client applications.

The compositor leaves newly mapped `clarion-desktop` on-demand surfaces unfocused
until the user clicks them, including after a shell restart or output attachment.
Desktop dialogs can still request exclusive overlay focus. A valid `FocusWindow`
action releases on-demand layer focus before returning keyboard input to the
application; invalid window IDs preserve the existing focus. Real Wayland tests
cover mapping, remapping, pointer clicks and both focus policies.

Dock behavior follows Apple's documented [magnification, launch bounce and
running indicators](https://support.apple.com/en-gb/guide/mac-help/mchlp1119/mac).
Motion uses a shared frame clock and continues from current presentation values
when interrupted, following the timing principles in Apple's
[Core Animation guide](https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/CoreAnimation_guide/AdvancedAnimationTricks/AdvancedAnimationTricks.html).
Public documentation does not specify Catalina's private Dock curves. This
implementation has not passed frame-by-frame comparison with a running Catalina
desktop, so its motion constants are not presented as recovered Apple constants.

The Linux runner uses GTK layer-shell. In Material mode the collapsed system bar
reserves 40 logical pixels at the selected top or bottom edge of its output and
uses compact icon buttons and chips;
expanded panels keep their normal control sizes. The bar never takes keyboard focus. A menu
expands into an output-sized modal overlay with exclusive keyboard interaction.
Closing it releases that interaction. The top corners remain available to
Clarion's existing hot corners when the bar is collapsed.

Material bar tooltips use a temporary 128-pixel transparent surface, with input
confined to the original 40-pixel strip and the same exclusive reservation. The tooltip
is drawn below the strip only after native allocation has completed. Leaving,
clicking, opening a panel or changing outputs dismisses it; the transparent
remainder passes input to applications and never takes keyboard focus.
The macOS menu bar uses a 110-pixel tooltip surface with input and reservation
remaining confined to its 22-pixel strip.

Panels enter with a 250 ms Material fade and short vertical slide; reduced
motion completes immediately. The fallback full-screen launcher uses a reversible
250 ms scale/fade entry and 160 ms exit, retaining the native modal surface until
exit completes. Reopening continues from the current value. Other panels close
and release keyboard focus immediately.

## Module ownership

| Module | Responsibility |
| --- | --- |
| `lib/theme` | Material 3 color roles, type, shape and motion |
| `lib/ui` | System bar, launcher, controls, notifications, tray icons and menus |
| `lib/state` | Immutable UI snapshots, selections, busy/error feedback |
| `lib/services` | Clarion IPC, XDG application catalog, system services, D-Bus notifications, StatusNotifier tray, wallpaper catalog/provider |
| `lib/services/power_profiles_service.dart` | Existing system power-profile provider, supported profiles, authorization and property/owner changes |
| `lib/ui/launcher_surface.dart` | Reversible launcher motion and native-surface lifetime coordination |
| `lib/ui/mac_launcher_panel.dart` | Retained Flutter fallback search, grid, pages, folders and drag interactions |
| `lib/services/launcher_bridge.dart` | Generation-aware process resolution, launcher IPC and visibility/launch/error events |
| `../clarion-launcher` | Independent GPUI Tahoe Apps panel, XDG catalog and `apps-view.json` |
| `lib/mac_launchpad_layout.dart` | Stable application order, folder membership and atomic layout persistence |
| `lib/mac_shell_preferences.dart` | Validated, immutable menu-bar and Dock preferences |
| `lib/services/dock_folder_service.dart` | Bounded XDG Downloads listing, watching, real metadata and GIO opening |
| `lib/ui/mac_downloads_stack_panel.dart` | Independent Folder/Stack icon and Fan/Grid/List/Automatic contents |
| `lib/ui/mac_dock_app_menu.dart`, `lib/ui/mac_dock_panel_geometry.dart` | Compact real-window menu and output-local icon-anchored placement |
| `lib/services/trash_state_service.dart` | Virtual Trash empty/full state, local watches and fallback polling |
| `lib/preferences.dart` | Shell-owned preferences, independent of personal compositor KDL |
| `linux/runner` | Layer, monitor, native window/input lifetime and GTK icon lookup |

IPC events and commands use separate Unix connections. Workspace actions use
stable workspace IDs, and initial event snapshots rebuild state after a
disconnect. UI widgets do not receive Wayland object handles, DRM FDs or Vulkan
textures. System commands pass argv directly; desktop applications launch
through GIO's desktop-entry support.

The fallback Material launcher borrows the classic Launchpad interaction pattern:
full-screen grid, search and horizontal pages. It retains
Material color roles, shapes and feedback, with lazy [PageView](https://api.flutter.dev/flutter/widgets/PageView-class.html)
pages, bounded native icon lookup and decoded image sizes. It does not claim
macOS folders or drag-reordering. Missing launch commands are filtered without
executing desktop-entry content or hiding applications based on names.

The fallback macOS launcher follows the documented
[Catalina Launchpad interactions](https://support.apple.com/zh-cn/guide/mac-help/mh35840/10.15/mac/10.15):
search across pages and folders, horizontal paging, Command–Left/Right, opening
real applications, dragging to an edge to change pages, grouping applications,
and editing folder names in place. Background clicks or Escape leave Launchpad;
Escape first leaves an open folder or cancels name editing. A roomy output uses
seven columns and five rows, adapting down to fit smaller outputs and the chosen
Dock edge. This grid choice and animation constants have not been extracted from
protected Catalina code or validated frame by frame against Catalina.

Folders and icon order use stable desktop IDs and a separate
`~/.config/clarion-shell/launchpad-layout.json`; writes are serialized and atomic.
Catalog changes remove obsolete entries and add newly installed applications
without duplicating IDs or overwriting edits made while loading. Search results
use the real application catalog, including applications inside folders. Dragging
into gaps reorders entries; dragging onto an application groups them. Dropping a
folder item into the outer grid extracts it. Single-application folders retain
their names until emptied. Apple-specific App Store uninstallation and private
Launchpad animation curves are not implemented.

Power controls use the existing provider's [PowerProfiles D-Bus API](https://upower.pages.freedesktop.org/power-profiles-daemon/gdbus-org.freedesktop.UPower.PowerProfiles.html).
The service does not start or replace daemons, and profile changes retain normal
D-Bus/Polkit authorization. Screenshot controls reuse Clarion IPC rather than a
second capture backend; preparation waits for modal release and a rendered
frame, is bounded by two seconds, and cancels obsolete requests.
Request identity also guards delayed IPC errors, so a closed or replaced panel
cannot be restored by an old failure. Retained launcher content ignores taps
and keyboard activation as soon as its panel starts closing.

## Denial reference

Reviewed local Denial revision `85b2303e2f09ae7b7b993641f90061a200f03d53`
on 2026-10-07. Reference files include:

- `dart_shell/lib/src/desktop/desktop_system_bar*.dart`
- `dart_shell/lib/src/desktop/desktop_application_launcher*.dart`
- `dart_shell/lib/src/desktop/desktop_panel_transition.dart`
- `dart_shell/lib/src/widgets/shade/quick_settings_tiles.dart`, `range_bar.dart`
- `dart_shell/lib/src/widgets/notification_card*.dart`, `notification_center.dart`
- `dart_shell/lib/src/state/quick_settings.dart`, `desktop_notifications.dart`
- `dart_shell/lib/src/theme/motion.dart`, `tokens.dart`, `shell_theme.dart`
- `dart_shell/lib/src/system/status_notifier_*.dart`, `system_tray*.dart`

The adaptation uses Clarion's own Material 3 components and service boundary.
Denial's custom Flutter engine, GBM atlas, external window textures, DENW wire
protocol and fork-only blur APIs are not prerequisites for this package.
Original Denial sources are copyright 2026 Doctor Logix, GPL-3.0-or-later;
see `resources/licenses/denial-GPL-3.0-or-later.txt`. Bundled Denial wallpapers,
fonts and cursors are not copied.

## Running safely

Use the build and preview instructions in `clarion-shell/README.md`. Preview
mode uses a normal application window and does not reserve output space or
claim the notification or StatusNotifier watcher names. Demo mode is explicitly
labeled and never dispatches desktop actions. The current desktop shell can keep running while
the new shell is previewed. The package does not restart the compositor or
rewrite `~/.config/clarion/config.kdl`.

Control center → Appearance → Bar position selects Top or Bottom. Top remains
the default for existing preferences. `bar_position` in the shell's independent
`preferences.json` stores `top` or `bottom`; invalid values fall back to `top`.
Floating panels and hints open toward the output interior, and the launcher
excludes the selected 40-pixel strip. Position changes keep the current panel
open, discard obsolete hints, and retain the bar's widget identities. Native
anchors, the exclusive reservation, tooltip input region and tray screen
coordinates follow the same selected edge.

The implementation covers the system bar, workspace selection,
application launcher, quick settings, notification center, StatusNotifier tray
and static wallpaper selection. Tray icons live in the compact bar; exported
DBusMenu objects are rendered by libdbusmenu-gtk3 as native GtkMenu Wayland
popups, anchored to the existing surface without expanding the bar. Menus are
cached by unique D-Bus owner and object path (at most eight entries); the native
client handles submenus and layout/property updates. Popup grabs retain the
original GTK input event, temporarily acquire keyboard focus and restore the
previous interaction mode on close. Cancelled preparation does not open a
second fallback menu. SNI ContextMenu is used when no menu is exported or native
loading fails; Flutter menu panels remain for previews without a native runner.
The shell requests `org.kde.StatusNotifierWatcher` with
`doNotQueue` only. If that name is already owned it registers as a host and
imports `RegisteredStatusNotifierItems` instead of replacing the watcher.
Tray actions translate surface-local pointer positions using the current native
output origin. Duplicate watcher addresses are normalized before the item limit;
per-item refreshes and action queues prevent a slow exporter blocking others.
In-flight scroll deltas remain associated with the original owner while metadata
updates preserve fractional wheel input. Overflow remains accessible on narrow outputs. Pure X11/XEmbed
tray icons are not hosted by this module.
The wallpaper gallery loads `~/图片/Wallpapers` and manages its own `swaybg`
child process. Selection is stored independently in `wallpaper.json` and
restored when the shell starts; preview mode skips automatic restoration and
demo mode never applies or saves wallpaper. Provider handoff waits for a
successfully loaded candidate to submit its own buffer before replacing the
previous owned process. The package does not terminate unrelated wallpaper providers.

MPRIS media, screen locking and a centralized multi-output
shell supervisor remain separate integration work; existing providers may be
retained while these are implemented.

Each process manages one selected output. `--monitor` and `--output-name`
should identify the same output; without an explicit name, workspace data
follows the focused output. Separate processes do not share panel state,
notification history or tray watcher ownership, and only one may own the
standard notification D-Bus name or StatusNotifier watcher. Physical display hotplug and coordinated per-output shell state still
need hardware acceptance testing.

## Desktop files

Normal shell startup also creates a transparent desktop view for each output in
the same process. No second service is needed. The bar continues to use its
selected output; desktop views follow monitor additions and removals separately.
Every desktop view lists the same XDG Desktop folder, resolved through
`xdg-user-dir DESKTOP`, with independent icon placement. A missing or disabled
desktop directory produces visible feedback rather than creating a directory or
treating HOME as the desktop.

The Material view supports files, folders and desktop-entry shortcuts:

- A click selects; double-click or Enter opens. Arrow keys move selection and F2
  renames the selected entry.
- Ordinary files and directories use `gio open` and the user's default application.
  A directory opening in a terminal can be diagnosed with `gio mime inode/directory`
  and corrected in the system's default-application settings.
- Labels use three lines with ellipsis for longer names. Only mouse hover reveals
  the complete name in a scrollable tooltip, kept clear of the bar. Selection
  does not display a full-name panel, move icons or promote the desktop surface.
  Larger text increases the grid cell dimensions.
- Delete or the item context menu asks for confirmation before using the system
  trash. Esc cancels without applying the operation.
- The background context menu creates a file, folder or shortcut. A shortcut
  chooses an installed application or an existing absolute local file/folder
  path; it does not accept arbitrary shell commands.
- Untrusted application `.desktop` entries require explicit confirmation before
  launch. Shortcuts copied from the installed application catalog are trusted;
  creation and rename reject an existing destination.
- Dragging snaps icons to the grid and swaps occupied positions. Invalid drops
  and canceled gestures restore the previous placement. Stable path keys retain
  icon identity across directory-watch refreshes; overflow remains accessible
  through pages and the wheel. Ordinary page changes clear selection so keyboard
  actions cannot affect an entry outside the current page.

`lib/desktop/desktop_service.dart` owns directory discovery, bounded scans,
watching, validation and argv-based GIO/file operations. `desktop_view.dart`
owns Material feedback, selection, menus and dialogs. `desktop_layout.dart`
owns geometry and persistence. `desktop_application.dart` manages their
lifetime and follows shell appearance preferences without starting device
polling, notifications or tray services in each desktop engine.

Positions are written atomically to separate files under
`$XDG_CONFIG_HOME/clarion-shell/desktop-layouts/` (normally
`~/.config/clarion-shell/desktop-layouts/`). A stable output identifier names
each file, and its payload retains the output name. Separate files avoid
cross-output overwrites between independent Dart isolates in one native
process. The old `desktop-layout.json` is read for migration only. Resizing
clamps cells to the usable output and resolves collisions; the selected top or
bottom bar edge leaves a 40-logical-pixel margin. Dark/light appearance and
reduced motion follow the shell's saved preferences.

The native desktop surface stays on the layer-shell `Bottom` layer with
`OnDemand` keyboard focus. Its full-output bounds reserve no extra exclusive
zone. Menus and dialogs render in a separate transparent `Overlay` view in the
same Flutter engine, with `Exclusive` keyboard focus only while open. The icon
grid keeps its pixels, position and selection; application windows keep covering
the same icons. A single interaction lifetime spans menu-to-dialog handoff.
After its first frame, the interaction view stays mapped to avoid GTK remapping
and lost redraws. Dismissal clears the view, assigns an empty input region and
releases its keyboard grab. Its empty frame explicitly clears old popup pixels. First-frame failure, late
native replies during disposal, and output removal clean up the interaction.
Four 8-by-8-logical-pixel corner cutouts preserve compositor hot corners on the
desktop; the interaction view receives corner clicks for modal dismissal.

macOS desktop menus use compact rows, blue selection feedback and a folder-first
blank menu. Their layer separation follows the original-view/contextual-menu
lifecycle described in [Apple's AppKit documentation](https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/MenuList/Articles/DisplayContextMenu.html).
Recovered Catalina Finder nib and localization resources confirm distinct
contextual commands; they do not establish all private window/animation logic.

```bash
# Desktop views on all outputs, without the bar.
scripts/run-shell.sh --desktop-only

# A regular desktop preview window, using an isolated folder and layout.
mkdir -p /tmp/clarion-desktop-preview
XDG_CONFIG_HOME=/tmp/clarion-desktop-preview-config \
  scripts/run-shell.sh --desktop-preview --desktop-dir=/tmp/clarion-desktop-preview

# Bar and panels without attached desktop views.
scripts/run-shell.sh --no-desktop
```

`--desktop-dir=/absolute/path` overrides directory discovery in either desktop
mode. Preview file operations still affect that directory, so an isolated
fixture is appropriate for acceptance tests. Existing `--preview` and `--demo`
shell previews do not attach desktop views automatically; `--desktop-preview`
selects the desktop preview role. `--no-desktop` cannot be combined with a
desktop-only role.

Desktop service, layout, view and application tests use temporary folders,
isolated configuration and fake launch/trash commands. They cover directory
changes, no-clobber operations, shortcut trust, selection and keyboard actions,
drag/cancel, paging, top/bottom geometry, independent output persistence and
native-interaction requests without modifying the user's desktop or launching
applications. Physical output hotplug still needs hardware acceptance.


## 控制中心滑块更新与交互成本（2026-10-09）

macOS 场景只订阅面板布局相关状态，因此动态控件必须自行订阅数据。控制中心的音量和亮度此前依赖父级重建，导致内部值已变化、旋钮和百分比却停在原处；Material 场景的父级全量监听掩盖了这个问题。现在控制中心分别订阅设置状态和滑块状态，macOS 外观表单仅订阅自己的偏好。连续调整音量或亮度不会重建完整外观表单。静音、服务可用性以及系统外部数值变化仍正常同步。

macOS Slider 使用细轨道和圆形旋钮，保留 Flutter 的键盘、焦点及无障碍交互。3 px 轨道、14 px 旋钮是项目的 Catalina 风格参数，并非从镜像测得的像素值。用户镜像是 Catalina 10.15.7 / 19H2，其原始音量入口为 Volume.menu；当前集中控制中心属于 Clarion 扩展，不能称为镜像原有控制中心的 1:1 复制。

系统快照按字段比较，未变化的轮询不再通知外壳。拖动或写入未结束时推迟完整系统读取；松手并完成写入后统一同步。写入队列把命令自身耗时计入 90 ms 间隔，慢命令完成后直接处理最新值，避免额外再等 90 ms。Dock 仅在各自参数改变时更新原生几何、输入区或玻璃区，放大动画不再重复提交不变的 GTK 尺寸和桌面保留空间。

回归覆盖两种外壳的双向拖动与取消、macOS 连续 30 帧、外部数值与可用性变化、偏好表单重建隔离、慢写入、无变化轮询、Dock 放大和原生异步调用。测试通过表示这些路径已修复，不代表已完成 Catalina 实机逐帧/逐像素比较，也不能证明所有 GPU 或合成器卡顿均已消除。
