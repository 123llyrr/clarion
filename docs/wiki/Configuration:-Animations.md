### Overview

Clarion has several animations which you can configure in the same way.
Additionally, you can disable or slow down all animations at once.

Here's a quick glance at the available animations with their default values.

```kdl
animations {
    // Uncomment to turn off all animations.
    // You can also put "off" into each individual animation to disable it.
    // off

    // Slow down all animations by this factor. Values below 1 speed them up instead.
    // slowdown 3.0

    // Individual animations.

    workspace-switch {
        spring damping-ratio=1.0 stiffness=1000 epsilon=0.0001

        cube {
            on
            perspective 0.65
            background-color "#101018"
        }
    }

    window-open {
        duration-ms 200
        curve "cubic-bezier" 0.16 1.4 0.30 1.0
        effect "fade"
        popin 0.94
        glide { off; }
    }

    window-close {
        duration-ms 400
        curve "linear"
        effect "explosion"

        glide {
            on
            tilt-deg 12
            depth 0.08
        }
    }

    window-wobble {
        on
        mesh 12
        stiffness 200
        damping 12
        mass 1
        intensity 0.2
        position-epsilon 0.25
        velocity-epsilon 2
    }

    horizontal-view-movement {
        spring damping-ratio=1.0 stiffness=800 epsilon=0.0001
    }

    window-movement {
        spring damping-ratio=0.92 stiffness=300 epsilon=0.001
    }

    window-resize {
        spring damping-ratio=0.92 stiffness=300 epsilon=0.001
    }

    config-notification-open-close {
        spring damping-ratio=0.6 stiffness=1000 epsilon=0.001
    }

    exit-confirmation-open-close {
        spring damping-ratio=0.6 stiffness=500 epsilon=0.01
    }

    screenshot-ui-open {
        duration-ms 200
        curve "ease-out-quad"
    }

    overview-open-close {
        spring damping-ratio=1.0 stiffness=800 epsilon=0.0001
    }

    recent-windows-close {
        spring damping-ratio=1.0 stiffness=800 epsilon=0.001
    }
}
```

### Animation Types

There are two animation types: easing and spring.
Each animation can be either an easing or a spring.

#### Easing

This is a relatively common animation type that changes the value over a set duration using an interpolation curve.

To use this animation, set the following parameters:

- `duration-ms`: duration of the animation in milliseconds.
- `curve`: the easing curve to use.

```kdl
animations {
    window-open {
        duration-ms 255
        curve "macos-default"
    }
}
```

Clarion supports the base curves plus several named cubic-Bezier presets.
You can get a feel for them on pages like [easings.net](https://easings.net/).

- `ease-out-quad` <sup>Since: 0.1.5</sup>
- `ease-out-cubic`
- `ease-out-expo`
- `linear` (aliases `macos-linear`, `ca-linear`) <sup>Since: 0.1.6</sup>
- `smooth-out` (alias `smoothout`), equivalent to `cubic-bezier(0, 0, 0.58, 1)`
- `overshot` (alias `overshoot`)
- `quick`
- `bouncy`
- `default` (aliases `macos-default`, `ca-default`), equivalent to `cubic-bezier(0.25, 0.1, 0.25, 1)`
- `ease-in` (aliases `macos-ease-in`, `ca-ease-in`), equivalent to `cubic-bezier(0.42, 0, 1, 1)`
- `ease-out` (aliases `macos-ease-out`, `ca-ease-out`), equivalent to `cubic-bezier(0, 0, 0.58, 1)`
- `ease-in-out` (aliases `ease-in-ease-out`, `macos-ease-in-out`, `ca-ease-in-out`), equivalent to `cubic-bezier(0.42, 0, 0.58, 1)`
- `cubic-bezier` <sup>Since: 25.08</sup>
    A custom [cubic Bézier curve](https://www.w3.org/TR/css-easing-1/#cubic-bezier-easing-functions). You need to set 4 numbers defining the control points of the curve, for example:
    ```kdl
    animations {
        window-open {
            // Same as CSS cubic-bezier(0.05, 0.7, 0.1, 1)
            curve "cubic-bezier" 0.05 0.7 0.1 1
        }
    }
    ```
    You can tweak the cubic-bezier parameters on pages like [easings.co](https://easings.co?curve=0.05,0.7,0.1,1).

#### Spring

Spring animations use a model of a physical spring to animate the value.
They notably feel better with touchpad gestures, because they take into account the velocity of your fingers as you release the swipe.
Springs can also oscillate / bounce at the end with the right parameters if you like that sort of thing, but they don't have to (and by default they mostly don't).

Due to springs using a physical model, the animation parameters are less obvious and generally should be tuned with trial and error.
Notably, you cannot directly set the duration.
You can use the [Elastic](https://flathub.org/apps/app.drey.Elastic) app to help visualize how the spring parameters change the animation.

A spring animation can be configured with a named preset or explicit physical parameters:

```kdl
animations {
    // Using a named preset:
    window-movement {
        spring "macos-interactive"
    }

    // Using explicit parameters:
    workspace-switch {
        spring damping-ratio=1.0 stiffness=1000 epsilon=0.0001
    }
}
```

Clarion provides built-in spring presets:
- `"macos-interactive"` (alias `"interactive"`): `damping-ratio=0.92 stiffness=300 epsilon=0.001`. Tuned for direct window manipulation, providing responsive motion with a subtle settle.
- `"macos-default"` (aliases `"ca-default"`, `"default"`): `damping-ratio=0.5 stiffness=100 epsilon=0.001`. Matches native Apple Core Animation (`CASpringAnimation`) API defaults (mass 1, stiffness 100, damping 10).

You can also use a preset and override individual parameters:

```kdl
animations {
    window-movement {
        spring "macos-interactive" stiffness=350
    }
}
```

The `damping-ratio` goes from 0.1 to 10.0 and has the following properties:

- below 1.0: underdamped spring, will oscillate in the end.
- above 1.0: overdamped spring, won't oscillate.
- 1.0: critically damped spring, comes to rest in minimum possible time without oscillations.

Clarion simultaneously bounds both displacement and velocity envelopes, ensuring that an underdamped spring settles smoothly without terminating prematurely upon the first target crossing.

Lower `stiffness` will result in a slower animation more prone to oscillation.

Set `epsilon` to a lower value if the animation "jumps" at the end.

> [!TIP]
> The spring *mass* (which you can see in Elastic) is hardcoded to 1.0 and cannot be changed.
> Instead, change `stiffness` proportionally.
> E.g. increasing mass by 2× is the same as decreasing stiffness by 2×.

### Animations

Now let's go into more detail on the animations that you can configure.

#### `workspace-switch`

Animation when switching workspaces up and down, including after the vertical touchpad gesture (a spring is recommended).

```kdl
animations {
    workspace-switch {
        spring damping-ratio=1.0 stiffness=1000 epsilon=0.0001
    }
}
```

##### `cube`

Clarion enables a vertical 3D cube transition by default. Keyboard switches, touchpad gestures,
gesture cancellation, and multi-workspace jumps all drive the same continuous transition. Only
the adjacent two workspaces are composed; panels, notifications, overlays, the lock screen, and
the pointer remain flat.

- `on` / `off`: enable or disable the cube while retaining the normal workspace animation.
- `perspective`: perspective strength from `0` to `1`.
- `background-color`: color shown behind and on empty cube faces.

#### `window-open`

Window opening defaults to a 200 ms transition from 94% size. The window becomes
fully opaque early, briefly reaches about 100.32% size, and settles at 100% with
zero end velocity. Opacity stays at full strength during the scale rebound.
It starts with the first mapped buffer. The complete window snapshot is cached
and redrawn only on damage; the fade uses the ordinary texture pipeline on both
Vulkan and GLES. This is a Clarion motion profile. The supplied curve is a Clarion
parameter choice; it is not a measured macOS window-opening curve. Timed `fade`
curves retain their scale settling phase; physical springs and `reveal` keep their
first-arrival completion behavior. Turning animations off or enabling reduced
motion removes the complete transform immediately.

```kdl
animations {
    window-open {
        duration-ms 200
        curve "cubic-bezier" 0.16 1.4 0.30 1.0
        effect "fade"
        popin 0.94
        glide { off; }
    }
}
```

`effect "reveal"` selects the previous expanding squircle from a random corner.
`custom-shader` takes precedence over the built-in effect selection.
Explicit duration, curve and `popin` settings still override the defaults.

##### `glide`

`glide` applies perspective tilt and depth to `effect "reveal"` or a custom shader.
It is disabled by default for opening windows. The lightweight `fade` always uses
a flat texture; select `reveal` or a custom shader to use Glide.

- `on` / `off`: enable or disable Glide.
- `tilt-deg`: tilt from `0` to `45` degrees.
- `depth`: depth from `0` to `1`.

##### `custom-shader`

<sup>Since: 0.1.6</sup>

You can write a custom shader for drawing the window during an open animation.

See [this example shader](./examples/open_custom_shader.frag) for a full documentation with several animations to experiment with.

If a custom shader fails to compile, clarion will print a warning and fall back to the default, or previous successfully compiled shader.
When running clarion as a systemd service, you can see the warnings in the journal: `journalctl -ef /usr/bin/clarion`

> [!WARNING]
>
> Custom shaders do not have a backwards compatibility guarantee.
> I may need to change their interface as I'm developing new features.

Example: open will fill the current geometry with a solid gradient that gradually fades in.

```kdl
animations {
    window-open {
        duration-ms 250
        curve "linear"

        custom-shader r"
            vec4 open_color(vec3 coords_geo, vec3 size_geo) {
                vec4 color = vec4(0.0);

                if (0.0 <= coords_geo.x && coords_geo.x <= 1.0
                        && 0.0 <= coords_geo.y && coords_geo.y <= 1.0)
                {
                    vec4 from = vec4(1.0, 0.0, 0.0, 1.0);
                    vec4 to = vec4(0.0, 1.0, 0.0, 1.0);
                    color = mix(from, to, coords_geo.y);
                }

                return color * clarion_clamped_progress;
            }
        "
    }
}
```

#### `window-close`

<sup>Since: 0.1.5</sup>

Window closing animation.

This one uses an easing type by default.

```kdl
animations {
    window-close {
        duration-ms 400
        curve "linear"
        effect "explosion"
    }
}
```

The built-in effects use Denial's controller durations: `explosion` 400 ms, `implode` 280 ms,
and `fade` 220 ms. If `duration-ms`, `curve`, or `spring` is present, that explicit timing wins.
The same `glide` block is available here. Clarion first evaluates the configured
`explosion`/`implode`/`fade` or custom pixel shader, then applies the Glide geometry transform.

#### `window-wobble`

Clarion enables elastic window geometry during interactive move and resize operations. Input and
client geometry stay unchanged; only the cached complete-window texture is deformed. After release,
the mesh settles using fixed simulation steps and stops requesting frames once stable.

- `mesh`: nodes per side, from `2` to `32`.
- `stiffness`, `damping`, `mass`: mass—spring simulation parameters; `mass` must be at least `0.1`.
- `intensity`: how much pointer/size movement is injected into the mesh, from `0` to `3`.
- `position-epsilon`, `velocity-epsilon`: thresholds at which settling stops.

`animations { off; }` disables Cube, Glide, Wobble, and all other animations together.

##### `custom-shader`

<sup>Since: 0.1.6</sup>

You can write a custom shader for drawing the window during a close animation.

See [this example shader](./examples/close_custom_shader.frag) for a full documentation with several animations to experiment with.

If a custom shader fails to compile, clarion will print a warning and fall back to the default, or previous successfully compiled shader.
When running clarion as a systemd service, you can see the warnings in the journal: `journalctl -ef /usr/bin/clarion`

> [!WARNING]
>
> Custom shaders do not have a backwards compatibility guarantee.
> I may need to change their interface as I'm developing new features.

Example: close will fill the current geometry with a solid gradient that gradually fades away.

```kdl
animations {
    window-close {
        custom-shader r"
            vec4 close_color(vec3 coords_geo, vec3 size_geo) {
                vec4 color = vec4(0.0);

                if (0.0 <= coords_geo.x && coords_geo.x <= 1.0
                        && 0.0 <= coords_geo.y && coords_geo.y <= 1.0)
                {
                    vec4 from = vec4(1.0, 0.0, 0.0, 1.0);
                    vec4 to = vec4(0.0, 1.0, 0.0, 1.0);
                    color = mix(from, to, coords_geo.y);
                }

                return color * (1.0 - clarion_clamped_progress);
            }
        "
    }
}
```

#### `horizontal-view-movement`

All horizontal camera view movement animations, such as:

- When a window off-screen is focused and the camera scrolls to it.
- When a new window appears off-screen and the camera scrolls to it.
- After a horizontal touchpad gesture (a spring is recommended).

```kdl
animations {
    horizontal-view-movement {
        spring damping-ratio=1.0 stiffness=800 epsilon=0.0001
    }
}
```

#### `window-movement`

<sup>Since: 0.1.5</sup>

Movement of individual windows within a workspace.

Includes:

- Moving window columns with `move-column-left` and `move-column-right`.
- Moving windows inside a column with `move-window-up` and `move-window-down`.
- Moving windows out of the way upon window opening and closing.
- Window movement between columns when consuming/expelling.

This animation *does not* include the camera view movement, such as scrolling the workspace left and right.

```kdl
animations {
    window-movement {
        spring "macos-interactive"
    }
}
```

#### `window-resize`

<sup>Since: 0.1.5</sup>

Window resize animation.

Only manual window resizes are animated, i.e. when you resize the window with `switch-preset-column-width` or `maximize-column`.
Also, very small resizes (up to 10 pixels) are not animated.

```kdl
animations {
    window-resize {
        spring "macos-interactive"
    }
}
```

##### `custom-shader`

<sup>Since: 0.1.6</sup>

You can write a custom shader for drawing the window during a resize animation.

See [this example shader](./examples/resize_custom_shader.frag) for a full documentation with several animations to experiment with.

If a custom shader fails to compile, clarion will print a warning and fall back to the default, or previous successfully compiled shader.
When running clarion as a systemd service, you can see the warnings in the journal: `journalctl -ef /usr/bin/clarion`

> [!WARNING]
>
> Custom shaders do not have a backwards compatibility guarantee.
> I may need to change their interface as I'm developing new features.

Example: resize will show the next (after resize) window texture right away, stretched to the current geometry.

```kdl
animations {
    window-resize {
        custom-shader r"
            vec4 resize_color(vec3 coords_curr_geo, vec3 size_curr_geo) {
                vec3 coords_tex_next = clarion_geo_to_tex_next * coords_curr_geo;
                vec4 color = texture2D(clarion_tex_next, coords_tex_next.st);
                return color;
            }
        "
    }
}
```

#### `config-notification-open-close`

The open/close animation of the config parse error and new default config notifications.

This one uses an underdamped spring by default (`damping-ratio=0.6`) which causes a slight oscillation in the end.

```kdl
animations {
    config-notification-open-close {
        spring damping-ratio=0.6 stiffness=1000 epsilon=0.001
    }
}
```

#### `exit-confirmation-open-close`

<sup>Since: 25.08</sup>

The open/close animation of the exit confirmation dialog.

This one uses an underdamped spring by default (`damping-ratio=0.6`) which causes a slight oscillation in the end.

```kdl
animations {
    exit-confirmation-open-close {
        spring damping-ratio=0.6 stiffness=500 epsilon=0.01
    }
}
```

#### `screenshot-ui-open`

<sup>Since: 0.1.8</sup>

The open (fade-in) animation of the screenshot UI.

```kdl
animations {
    screenshot-ui-open {
        duration-ms 200
        curve "ease-out-quad"
    }
}
```

#### `overview-open-close`

<sup>Since: 25.05</sup>

The open/close zoom animation of the [Overview](./Overview.md).

```kdl
animations {
    overview-open-close {
        spring damping-ratio=1.0 stiffness=800 epsilon=0.0001
    }
}
```

#### `recent-windows-close`

<sup>Since: 25.11</sup>

The close fade-out animation of the recent windows switcher.

```kdl
animations {
    recent-windows-close {
        spring damping-ratio=1.0 stiffness=800 epsilon=0.001
    }
}
```

### Synchronized Animations

<sup>Since: 0.1.5</sup>

Sometimes, when two animations are meant to play together synchronized, clarion will drive them both with the same configuration.

For example, if a window resize causes the view to move, then that view movement animation will also use the `window-resize` configuration (rather than the `horizontal-view-movement` configuration).
This is especially important for animated resizes to look good when using `center-focused-column "always"`.

As another example, resizing a window in a column vertically causes other windows to move up or down into their new position.
This movement will use the `window-resize` configuration, rather than the `window-movement` configuration, to keep the animations synchronized.

A few actions are still missing this synchronization logic, since in some cases it is difficult to implement properly.
Therefore, for the best results, consider using the same parameters for related animations (they are all the same by default):

- `horizontal-view-movement`
- `window-movement`
- `window-resize`
