# Realand Features

Realand is a Rust Wayland desktop with a device-independent `realand-core`, a native
Vulkan compositor backend and an integrated Shell. The core owns window and workspace
identity, layout policy, interaction state and presentation timing. Platform adapters
handle Wayland, input devices and DRM/KMS through Smithay.

## Native Vulkan Composition

The Vulkan backend composes desktop surfaces, blur, liquid glass and animated window
effects. DMA-BUF import, explicit synchronization, pipeline preparation and asynchronous
GPU completion are part of the rendering path. Each output has its own presentation
clock and adaptive frame budget; diagnostics distinguish CPU work, GPU timing and
hardware presentation intervals.

Select it in `~/.config/realand/config.kdl`:

```kdl
renderer "vulkan"
```

Alternatively, use `realand --renderer vulkan` for a development run. The current default
is GLES when no renderer is selected. Backend changes require a new compositor process;
available import, synchronization and presentation paths depend on the driver and output.

Realand Shell provides the launcher, Dock, control center, notifications and desktop
controls, with Material and macOS-inspired appearances.

## Shared Desktop Core

Floating, scrolling and dynamic BSP layouts share stable window and workspace identities.
The pure core has no Smithay, Wayland or GPU dependency. Layout policy returns geometry;
the platform layer applies client constraints and protocol operations. Immutable scene
plans separate desktop decisions from renderer-specific drawing.

## Dual Tiling Engines, Same Workspace

**Scrollable tiling** and **dynamic dwindle tiling** coexist in the same workspace and
can be switched at any time while retaining window state. Floating windows remain
available alongside either mode.

### Auto-stacking into Dwindle columns

By default, new windows enter the current column and are arranged as a 2D spiral
("dwindle puzzle") until the column fills up, then overflow into a new column to the right.
Each column has its own internal layout within the scrolling horizontal strip.

```kdl
layout {
    // New windows enter the active column as a 2D dwindle puzzle until it fills up,
    // then overflow to a new column. Set to false to get one window per column.
    auto-stack-new-windows true

    // Maximum number of dynamically tiled windows per column.
    max-windows-per-column 3
}
```

A workspace can freely mix **Normal columns** (plain vertical stacks), **Tabbed columns**, and
**Dwindle columns** at the same time.

### Switching the tiling engine

The whole workspace can switch between the two engines with `switch-tiling-mode`. Windows are
migrated between the engines, keeping their state intact.

```kdl
binds {
    // Switch the workspace between scrolling tiling and Dwindle (2D BSP) tiling.
    Mod+Shift+Space { switch-tiling-mode; }

    // Toggle the current column between Normal (vertical stack) and
    // Dwindle (2D spiral split) display.
    Mod+Shift+D { toggle-column-dwindle-display; }

    // In workspace Dwindle mode, force the split direction of the next window.
    Mod+Shift+S { toggle-dwindle-split; }
}
```

Focus navigation is two-dimensional: `focus-window-up` / `focus-window-down` jump to the
geometrically nearest window in the requested direction, and at column edges they naturally slide
into the neighboring column while the view follows.

## Configurable Animations

### Named and custom bezier curves

Named curves include `overshot` (0.05, 0.9, 0.1, 1.05), `smooth-out`, `quick`,
`bouncy`. You can also write a standard 4-control-point cubic bezier:

```kdl
animations {
    window-open {
        duration-ms 250
        curve "overshot"
    }
}
```

Built-in opening and closing effects finish when clamped progress first reaches the target,
including overshooting Bézier curves. Movement animations and custom shader progress retain
the complete curve; the configured duration is not rewritten.

### Popin window opening

Windows can scale up from a fraction of their size, anchored at the window center, while fading in:

```kdl
animations {
    window-open {
        duration-ms 250
        curve "overshot"
        popin 0.8
    }
}
```

### Spring physics

Window movement supports a damped spring model instead of a fixed curve, producing the familiar
elastic overshoot:

```kdl
animations {
    window-movement {
        spring damping-ratio=0.75 stiffness=900 epsilon=0.001
    }
}
```

## Window Opening and Closing Effects

Window opening reveals an expanding squircle over 320 ms as soon as the first buffer maps.
Window closing uses a bounded shader pass plus one batched particle draw:

- `explosion` (default): a brief contraction followed by 88 shards flying apart, without flash glow or shockwave rings.
- `implode`: the window rotates, contracts with `easeInBack`, and fades away.
- `fade`: a minimal smooth scale-down fade.

```kdl
animations {
    window-close {
        duration-ms 400
        curve "linear"
        effect "explosion"  // "implode", "explosion" or "fade"
    }
}
```

The default effect can also be overridden globally with the `REALAND_WINDOW_CLOSE_EFFECT`
environment variable (`explosion`, `implode` or `fade`).

The opening and closing effect adaptations are documented in the
[Denial source and attribution record](https://github.com/123llyrr/realand/blob/main/docs/denial-reference.txt).

## Classic 3D Effects

Realand provides three effects on both native Vulkan and GLES, enabled by default:

- **Elastic windows** deform only the actively moved or resized window with a configurable
  mass—spring mesh and continue settling after release.
- **Workspace cube** rotates the vertical workspace model around the X axis. Keyboard transitions
  and touchpad gestures share the same continuous state, including cancellation.
- **3D Glide** tilts and moves complete opening and closing window snapshots in depth. Closing
  pixel effects such as `explosion` are evaluated before Glide.

The compositor caches only one interacting window and at most two adjacent workspace faces. Once
the effects settle, they stop scheduling frames and direct scan-out becomes eligible again. See
[Animation Configuration](./Configuration:-Animations.md) for every parameter.

## Animated Rotating Gradient Borders

Focus ring and border gradients support color interpolation in the oklch color space with a hue
interpolation direction, plus continuous rotation of the gradient angle:

```kdl
layout {
    focus-ring {
        width 4
        active-gradient from="#ff007f" to="#00f0ff" angle=0 in="oklch longer hue" rotate-deg-per-sec=180
    }
}
```

`in="oklch longer hue"` makes a two-color gradient sweep the full 360° hue wheel, and
`rotate-deg-per-sec` spins it continuously. The gradient only animates while the focused window is
on screen.

## Workspace Overview: Spatial Stage and Expo Grid

Realand keeps the workspace overview, Expo grid and all-window search independent:

- **Left-top hot corner** (or `Mod+O`): the spatial stage by default, with a central
  workspace card, layered side previews and a workspace strip along the bottom.
  Preview another workspace without entering it; click a card or window, or press Enter,
  to enter. Escape returns to the original desktop. Drag windows to organize workspaces
  or create a new one.
- **Right-top hot corner** (or `Mod+E`): an Expo 2D grid wall. All workspaces are
  shown as a scaled matrix; hover a cell to highlight it, click (or press Enter) to zoom into it.
- **All-window search** keeps its own window grid and search controls.

Choose **空间舞台** or **经典纵向概览** on the settings center's appearance page.
The equivalent KDL option is `overview { style "stage"; }` (default) or
`overview { style "strip"; }`. Style changes retain zoom, shadows and animation
parameters. With animations disabled, overview transitions switch directly between endpoints.

Hot corners are configured in the `gestures` section:

```kdl
gestures {
    hot-corners {
        top-left
        top-right
    }
}
```

By default the bottom-left hot corner opens the realand-settings GUI.

## realand-settings

`realand-settings` is a graphical settings panel (egui-based) that talks to the compositor over IPC.
It provides a dashboard, layout/animation/appearance toggles, hot corner configuration, output
management, and a KDL config editor with syntax highlighting. It is started automatically on login
(or from the bottom-left hot corner) and toggles via its Unix socket.

## IPC

The IPC actions used above are scriptable:

```sh
realand msg action switch-tiling-mode
realand msg action toggle-column-dwindle-display
realand msg action toggle-dwindle-split
```


## Arrange Windows in Expo

Press `Mod+E` (default binding) or use the configured top-right hot corner to open Expo.
Each card is a live preview of one workspace's visible area. Scrolling content is clipped
inside its card, and empty workspaces have a visible outline.

- Click a window to focus it and leave Expo; click an empty card to open that workspace.
- Drag a window onto another card to move it. The destination card gets a thicker outline.
- Drag across displays to move a window to a workspace on another monitor.
- Dropping in a gap selects the nearest card; it does not create an accidental workspace.
- After a drop, Expo stays open for further arrangement. Use Enter to confirm your selected
  workspace or Escape to return to the workspace where you opened Expo.

Both tiling engines and floating windows use the same card coordinates, including fractional
output scales. Escape restores navigation; it does not undo window moves already completed.

For visual review, run `cargo run -p realand-visual-tests` and select **Realand - Expo Arrange**.

## Search, Groups, Layout History, and Session Layouts

The settings application's **窗口** page now provides named groups, undo/redo, and explicit
session layout saving and restoration. These controls require the updated compositor; after
installing it, log out and select Realand at the login screen.

- **Window spread:** `Mod+Ctrl+E` in the default configuration, or **全窗口展开 / 搜索** in
  settings. Each output shows windows from all its workspaces, including offscreen scrolling
  columns and inactive tabs. Type title/application keywords, navigate with arrows, and press
  Enter to focus. Each output initially highlights its focused window. Arrow keys follow the
  visible grid without crossing row boundaries or jumping into missing cells; Tab advances
  through the list. Escape cancels back to the original window. Click a thumbnail to focus it.
  Windows animate from their current desktop positions into the grid and back to the selected
  workspace. Early cancellation and reopening continue from the current frame. Disabling
  overview animations switches directly between the endpoints. During exit, thumbnails do
  not send clicks or hover focus into application windows; normal input resumes at the end.
  Use Expo for window dragging.
  `Mod+Shift+E` keeps its existing quit binding.
- **Named groups:** select windows in settings, enter a name, and save. Saving an existing name
  replaces its membership. A window belongs to at most one group. Drag a group label onto a
  workspace button or use **整组移动**. In Expo, dragging a member to another workspace brings
  the remaining members to that workspace when released; the pointer preview shows the member
  being dragged. Dissolving a group leaves its windows open.
- **Layout history:** the last 32 layout checkpoints can be undone/redone from settings or with
  the new default `Mod+Ctrl+Z` / `Mod+Ctrl+Shift+Z` bindings. Window/workspace movement, resizing,
  tiling-mode changes, column arrangement and group edits are covered. One drag is one step.
  Closed windows are skipped, and windows opened after the checkpoint are retained. Focus-only
  navigation does not add history. History is in memory and is cleared on compositor restart.
- **Session layouts:** **保存会话布局** atomically writes
  `$XDG_STATE_HOME/realand/session-layout.json` (normally
  `~/.local/state/realand/session-layout.json`). **恢复会话布局** restores workspace placement,
  floating positions/sizes, scrolling column membership/display/sizing, Dwindle split trees,
  and named groups. Open the applications first, then restore. Matching uses application ID and
  title, with application-only fallback when unambiguous. Missing windows are skipped; open
  them and restore again. Missing outputs fall back to an available output. Repeated restoration
  reuses restored workspaces. Restoration can itself be undone. Application processes, unsaved
  documents, and application-internal state are not restarted/restored by this feature.

Existing user configurations are not overwritten when installing binaries. Add the three new
bindings to an existing `binds` block, or use the settings buttons. IPC clients can query
`realand msg management-state` and use `set-window-group`, `remove-window-group`,
`move-window-group`, `undo-layout`, `redo-layout`, and `restore-session-layout` actions.
The session JSON has a version field and is validated before changes are applied.

Settings IPC now runs on a bounded background queue with an event stream for windows/workspaces,
plus periodic output/management refresh and automatic reconnection. Config edits use the same
KDL AST parser as the compositor, preserving untouched source. Config and session writes use
atomic replacement; existing configuration symlinks and file permissions are retained.
Expo reuses geometry and border buffers instead of allocating new border IDs every frame.

Regression coverage includes cross-output group dragging at fractional scales, undo after
windows close or open, column sizing and tab restoration, Dwindle tree restoration, absent
outputs, repeated session restoration, background IPC and KDL editing. An EGL test renders
nine offscreen-capable client thumbnails and verifies that an unmatched search removes them.
For interactive visual inspection select **Realand - Spread Transition and Reversal**,
**Realand - Spread Cancel after Navigation**, or **Realand - Window Spread Search** in
`cargo run -p realand-visual-tests`. Physical GPU/display-driver behavior still needs a desktop
session check after installation.
