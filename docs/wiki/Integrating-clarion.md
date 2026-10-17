This page contains various bits of information helpful for integrating clarion in a distribution.
First, for creating a clarion package, see the [Packaging](./Packaging-clarion.md) page.

### Configuration

Clarion will load configuration from `$XDG_CONFIG_HOME/clarion/config.kdl` or `~/.config/clarion/config.kdl`, falling back to `/etc/clarion/config.kdl`.
Existing Realand paths are used only when their Clarion equivalents are absent; the
[migration guide](../clarion-migration.md) describes CLI, Shell and IPC compatibility.
If both of these files are missing, clarion will create `$XDG_CONFIG_HOME/clarion/config.kdl` with the contents of [the default configuration file](https://github.com/123llyrr/clarion/blob/main/resources/default-config.kdl), which are embedded into the clarion binary at build time.

This means that you can customize your distribution defaults by creating `/etc/clarion/config.kdl`.
When this file is present, clarion *will not* automatically create a config at `~/.config/clarion/`, so you'll need to direct your users how to do it themselves.

Keep in mind that we update the default config in new releases, so if you have a custom `/etc/clarion/config.kdl`, you likely want to inspect and apply the relevant changes too.

The default configuration locations can be overridden with the `CLARION_CONFIG` environment variable.

<sup>Since: 26.04</sup> You can also change the configuration path at runtime via the clarion IPC or using the command `clarion msg action load-config-file --path <path-to-config.kdl>`.

<sup>Since: 25.11</sup> You can split the clarion config file into multiple files using [`include`](./Configuration:-Include.md).

### Xwayland

Xwayland is required for running X11 apps and games, and also the Orca screen reader.

<sup>Since: 25.08</sup> Clarion integrates with [xwayland-satellite](https://github.com/Supreeeme/xwayland-satellite) out of the box.
The integration requires xwayland-satellite >= 0.7 available in `$PATH`.
Please consider making clarion depend on (or at least recommend) the xwayland-satellite package.
If you had a custom config which manually started `xwayland-satellite` and set `$DISPLAY`, you should remove those customizations for the automatic integration to work.

You can change the path where clarion looks for xwayland-satellite using the [`xwayland-satellite` top-level option](./Configuration:-Miscellaneous.md#xwayland-satellite).

### Keyboard layout

<sup>Since: 25.08</sup> By default (unless [manually configured](./Configuration:-Input.md#layout) otherwise), clarion reads keyboard layout settings from systemd-localed at `org.freedesktop.locale1` over D-Bus.
Make sure your system installer sets the keyboard layout via systemd-localed, and clarion should pick it up.

### Autostart

Clarion works with the normal systemd autostart.
The default [clarion.service](https://github.com/123llyrr/clarion/blob/main/resources/clarion.service) brings up `graphical-session.target` as well as `xdg-desktop-autostart.target`.

To make a program run at clarion startup without editing the clarion config, you can either link its .desktop to `~/.config/autostart/`, or use a .service file with `WantedBy=graphical-session.target`.
See the [example systemd setup](./Example-systemd-Setup.md) page for some examples.

If this is inconvenient, you can also add [`spawn-at-startup`](./Configuration:-Miscellaneous.md#spawn-at-startup) lines in the clarion config.

### Screen readers

<sup>Since: 25.08</sup> Clarion works with the [Orca](https://orca.gnome.org) screen reader.
Please see the [Accessibility](./Accessibility.md) page for details and advice for accessibility-focused distributions.

### Desktop components

You very likely want to run at least a notification daemon, portals, and an authentication agent.
This is detailed on the [Important Software](./Important-Software.md) page.

On top of that, you may want to preconfigure some desktop shell components to make the experience less barebones.
Clarion's default config spawns [Waybar](https://github.com/Alexays/Waybar), which is a good starting point, but you may want to consider changing its default configuration to be less of a kitchen sink, and adding a workspaces module.
You will probably also want a desktop background tool ([swaybg](https://github.com/swaywm/swaybg) or [awww (which used to be swww)](https://codeberg.org/LGFae/awww/)), and a nicer screen locker (compared to the default `swaylock`), like [hyprlock](https://github.com/hyprwm/hyprlock/).

Alternatively, some desktop environments and shells work with clarion, and can give a more cohesive experience in one package:

- [LXQt](https://lxqt-project.org/) on Wayland: see [their wiki](https://lxqt-project.org/wiki/Wayland-Session).
- Many [XFCE](https://www.xfce.org/) components work on Wayland. See [their wiki](https://wiki.xfce.org/releng/wayland_roadmap#component_specific_status).
- Desktop shells based on Quickshell, for example [DankMaterialShell](https://github.com/AvengeMedia/DankMaterialShell) and [Noctalia](https://github.com/noctalia-dev/noctalia-shell).

### Security model

See the [Security Model](./Security-Model.md) page for an overview of clarion's security model.
