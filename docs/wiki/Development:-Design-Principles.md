## General principles

Clarion separates device-independent desktop policy from protocol integration, scene
construction and GPU presentation. These principles guide that design; individual layout
modes may make different geometry choices.

### Opening a new window should not affect the sizes of any existing windows.

This is the main annoyance with traditional tiling: you want to open a new window, but it messes with your existing window sizes.
Especially when you're looking at a big window like a browser or an image editor, want to open a quick terminal for something, and it makes the big window unusably small, or reflows the content, or clips part of the window.

The usual workaround in tiling WMs is to use more workspaces: when you need a new window, you go to an empty workspace and open it there (this way, you also get your entire screen for the new window, rather than a smaller part of it).

Scrollable tiling offers an alternative: for temporary windows, you can just open them, do what you need, and close, all without messing up the other windows or having to go to a new workspace.
It also lets you group together more related windows on the same workspace by having less frequently used ones scrolled out of the view.

### The focused window should not move around on its own.

In particular: windows opening, closing, and resizing to the left of the focused window should not cause it to visually move.

The focused window is the window you're working in.
And stuff happening outside the view shouldn't mess with what you're focused on.

### Actions should apply immediately.

This is important both for compositor responsiveness and predictability, and for keeping the code sane and free of edge cases and unnecessary asynchrony.

- Things like resizing or consuming into column take effect immediately, even if the window needs time to catch up.
- An animated workspace switch makes your input go to the final workspace and window instantly, without waiting for the animation.
- Opening the overview (which has a zoom-out animation) lets you grab windows right away, and closing the overview makes your input immediately go back to the windows, without waiting for the zoom back in.

### When disabled, eye-candy features should not affect the performance.

Things like animations and custom shaders do not run and are not present in the render tree when disabled.
Extra offscreen rendering is avoided.

Animations specifically are still "started" even when disabled, but with a duration of 0 (this way, they end as soon as the time is advanced).
This does not impact performance, but helps avoid a lot of edge cases in the code.

### Eye-candy features should not cause unreasonable excessive rendering.

- For example, clip-to-geometry will prevent direct scanout in many cases (since the window surface is not completely visible). But in the cases where the surface or the subsurface *is* completely visible (fully within the clipped region), it will still allow for direct scanout.
- For example, animations *can* cause damage and even draw to an offscreen every frame, because they are expected to be short (and can be disabled). However, something like the rounded corners shader should not offscreen or cause excessive damage every frame, because it is long-running and constantly active.

### Be mindful of invisible state.

This is clarion state that is not immediately apparent from looking at the screen. This is not bad per se, but you should carefully consider how to reduce the surprise factor.

- For example, when a monitor disconnects, all its workspaces move to another connected monitor. In order to be able to restore these workspaces when the first monitor connects again, these workspaces keep the knowledge of which was their *original monitor*—this is an example of invisible state, since you can't tell it in any way by looking at the screen. This can have surprising consequences: imagine disconnecting a monitor at home, going to work, completely rearranging the windows there, then coming back home, and suddenly some random workspaces end up on your home monitor. In order to reduce this surprise factor, whenever a new window appears on a workspace, that workspace resets its *original monitor* to its current monitor. This way, the workspaces you actively worked on remain where they were.
- For example, clarion preserves the view position whenever a window appears, or whenever a window goes full-screen, to restore it afterward. This way, dealing with temporary things like dialogs opening and closing, or toggling full-screen, becomes less annoying, since it doesn't mess up the view position. This is also invisible state, as you cannot tell by looking at the screen where closing a window will restore the view position. If taken to the extreme (previous view position saved forever for every open window), this can be surprising, as closing long-running windows would result in the view shifting around pretty much randomly. To reduce this surprise factor, clarion remembers only one last view position per workspace, and forgets this stored view position upon window focus change.

## Module boundaries

Clarion keeps one compositor event loop. Modules own their state and expose operations
for the events they handle. The native Vulkan backend retires submitted GPU work on a
completion worker; desktop state remains owned by the compositor thread.

| Owner | Responsibility | Integration contract |
| --- | --- | --- |
| `clarion-core` | Desktop identity, layout strategies, interaction state, scene plans and presentation policy | Pure state and geometry without Smithay, Wayland or GPU dependencies. Returns effects for the platform to apply. |
| `layout` | Client-aware layout integration and visible window state | Applies core decisions to tiles and client constraints; handles protocol operations through the platform. |
| `gestures` | Velocity, scroll accumulation and gesture sampling | Numeric deltas and timestamps, shared by input and layout. The old `input` tracker paths remain compatibility exports. |
| `overview` | Compositor-level overview transitions | Input sends `OverviewCommand`; this boundary orders frame capture, grab teardown, layout changes and redraw. Confirmation during an ungrab must not recursively cancel the same grab. |
| `input::hot_corner` | Per-output corner entry and exit hysteresis | `motion`, `reset` and `reset_output` own the latch. Rendering and frame callbacks cannot rearm it. |
| `output_schedule` | Per-output redraw state, frame timing, retry and timer ownership | Integrates the core scheduler with submission/presentation events. Teardown cancels the owned timers on the event loop. |
| `clarion_config::schema` | Versioned editor metadata for configuration | The configuration crate generates its own descriptions. `clarion-settings` consumes the public API and owns translated labels, groups and examples. `Config::parse` remains the validation authority. |
| `frame_pipeline` | Scene snapshots and frame preparation/feedback | `FramePort` exposes the scene and frame services needed by device rendering. |
| `platform::device_bridge` | Device lifecycle and configuration/feedback | `DevicePort` restricts setup operations; the platform coordinator owns access to desktop integration state. |
| `render_helpers::vulkan` | Native Vulkan composition, effects, resources and GPU synchronization | Implements renderer interfaces, pipeline preparation and asynchronous GPU completion. Output presentation remains a device-backend responsibility. |

`State` remains the integration point for the Wayland server, backend and desktop.
Pure calculations and state machines should be testable without constructing it.
Cross-mode overview tests must exercise the compositor command entry as well as the
layout algorithms, because capture and input-grab ordering is observable behavior.
Scheduling teardown tests use an independent `calloop::EventLoop<()>`; configuration
metadata tests preserve the public descriptions when private source modules move.

## Window layout

Here are some design considerations for the window layout logic.

1. If a window or popup is larger than the screen, it should be aligned in the top left corner.

    The top left area of a window is more likely to contain something important, so it should always be visible.

1. Setting window width or height to a fixed pixel size (e.g. `set-column-width 1280` or `default-column-width { fixed 1280; }`) will set the size of the window itself, however setting to a proportional size (e.g. `set-column-width 50%`) will set the size of the tile, including the border added by clarion.

    - With proportions, the user is looking to tile multiple windows on the screen, so they should include borders.
    - With fixed sizes, the user wants to test a specific client size or take a specifically sized screenshot, so they should affect the window directly.
    - After the size is set, it is always converted to a value that includes the borders, to make the code sane. That is, `set-column-width 1000` followed by changing the clarion border width will resize the window accordingly.

1. Fullscreen windows are a normal part of the scrolling layout.

    This is a cool idea that scrollable tiling is uniquely positioned to implement.
    Fullscreen windows aren't on some "special" layer that covers everything; instead, they are normal tiles that you can switch away from, without disturbing the fullscreen status.

    Of course, you do want to cover your entire monitor when focused on a fullscreen window.
    This is specifically hardcoded into the logic: when the view is stationary on a focused fullscreen window, the top layer-shell layer and the floating windows hide away.

    This is also why fullscreening a floating window makes it go into the scrolling layout.

## Default config

The [default config](https://github.com/123llyrr/clarion/blob/main/resources/default-config.kdl) is intended to give a familiar, helpful, and not too jarring experience to new clarion users.
Importantly, it is not a "suggested rice config"; we don't want to startle people with full-on rainbow borders and crazy shaders.

Clarion provides an integrated Shell and settings app while keeping the compositor
usable with external desktop components. The example config starts Waybar; Shell users
should remove that startup line and follow the Shell installation instructions.
Users can adjust the KDL config directly or use Clarion's graphical settings app.

The default config is therefore thoroughly commented with links to the relevant wiki sections.
We don't include every possible option in the default config to avoid overwhelming users too much; anything overly specific or uncommon can stay on the wiki.
The general rule is to include things that users are reasonably expected to want to change or know how to do.
We do also advertise our more unique features though like screencast block-out-from.

We default to CSD (`prefer-no-csd` is commented out).
This gives new users easy and familiar way to move and close windows via their titlebars, especially considering that clarion doesn't have serverside titlebars (so far at least).

Focus rings are drawn fully behind windows by default.
While this unfortunately messes with window transparency, [which is a common source of confusion](./FAQ.md#why-are-transparent-windows-tinted-why-is-the-borderfocus-ring-showing-up-through-semitransparent-windows), defaulting to drawing focus rings only around windows would be even worse because it has holes inside clientside rounded corners.
The ideal solution here would be to propose a Wayland protocol for windows to report their corner radius to the compositor (which would generally help for serverside decorations in different compositors).

The default focus ring is quite thick at 4 px to look well with clientside-decorated windows and be obviously noticeable, and the default gaps are also quite big at 16 px to look well with the default focus ring width.

The default input settings like touchpad tap and natural-scroll are how I believe most people want to use their computers.

Shadows default to off because they are a fairly performance-intensive shader, and because many clientside-decorated windows already draw their own shadows.

The default screenshot-path matches GNOME Shell.

Default window rules are limited to fixing known severe issues (WezTerm) and doing something the absolute majority likely wants (make Firefox Picture-in-Picture player floating—it can't do that on its own currently, maybe the pip protocol will change that).

The default binds assume QWERTY and group related navigation and movement actions.
The binds are ordered in a way to gradually introduce you to different bind configuration concepts.

The general system is: if a hotkey switches somewhere, then adding <kbd>Ctrl</kbd> will move the focused window or column there.
Adding <kbd>Shift</kbd> does an alternative action: for focus and movement it starts going across monitors, for resizes it starts acting on window height rather than width, etc.
Workspace switching on <kbd>Mod</kbd><kbd>U</kbd>/<kbd>I</kbd> is one key up from <kbd>Mod</kbd><kbd>J</kbd>/<kbd>K</kbd> used for window switching.

Since <kbd>Alt</kbd> is a modifier in nested clarion, binds with explicit <kbd>Alt</kbd> are mainly the ones only useful on the host, for example spawning a screen locker.
