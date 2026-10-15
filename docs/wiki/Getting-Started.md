## Quick start

Realand is not in distribution repositories yet. Build from source:

```sh
cargo build --release -p realand -p realand-settings
./target/release/realand
```

This nested window is enough to try it inside an existing session.
The current default renderer is GLES. To try native Vulkan instead, run:

```sh
./target/release/realand --renderer vulkan
```

For a desktop session, add the following at the top level of
`~/.config/realand/config.kdl` before logging in:

```kdl
renderer "vulkan"
```

The Vulkan backend provides desktop composition, blur, liquid glass and window effects,
with asynchronous GPU completion and per-output frame budgets. Driver capabilities
determine which import, synchronization and presentation paths are available. Renderer
selection takes effect when starting a new compositor process.

To install a display-manager session (SDDM, GDM, Plasma Login Manager):

```sh
sudo bash scripts/install-session.sh
systemctl --user daemon-reload
```

Then log out, choose **Realand**, and log back in.
Or, if not using a display manager, run `realand-session` on a TTY.

The example config starts Waybar. If using Realand Shell, remove or comment out
`spawn-at-startup "waybar"` in your config to avoid starting a second bar on the next login.
Shell is built and installed separately; see its
[build, preview and installation instructions](https://github.com/123llyrr/realand/blob/main/realand-shell/README.md).

See the [Building](#building) section and the [Packaging realand](./Packaging-realand.md) page if you want to package realand.

After installing, start realand from your display manager like GDM.
Press <kbd>Super</kbd><kbd>T</kbd> to run a terminal ([Alacritty]) and <kbd>Super</kbd><kbd>D</kbd> to run an application launcher ([fuzzel]).
To exit realand, press <kbd>Super</kbd><kbd>Shift</kbd><kbd>E</kbd>.

If you're not using a display manager, you should run `realand-session` (systemd/dinit) or `realand --session` (others) from a TTY.
The `--session` flag will make realand import its environment variables globally into the system manager and D-Bus, and start its D-Bus services.
The `realand-session` script will additionally start realand as a systemd/dinit service, which starts up a graphical session target required by some services like portals.

You can also run `realand` inside an existing desktop session.
Then it will open as a window, where you can give it a try.
Note that this windowed mode is mainly meant for development, so it is a bit buggy (in particular, there are issues with hotkeys).

Next, see the [list of important software](./Important-Software.md) required for normal desktop use, like a notification daemon and portals.
Also, check the [configuration introduction](./Configuration:-Introduction.md) page to get started configuring realand.
There you can find links to other pages containing thorough documentation and examples for all options.
Finally, the [Xwayland](./Xwayland.md) page explains how to run X11 applications on realand.

### Desktop environments

Realand Shell integrates the launcher, Dock, control center, notifications and desktop
controls. It supports Material and macOS-inspired appearances; compositing and liquid
glass are handled by the Realand renderer. The compositor also works with external
Wayland desktop components:

- Many [XFCE](https://www.xfce.org/) components work on Wayland. See [their wiki](https://wiki.xfce.org/releng/wayland_roadmap#component_specific_status).
- Desktop shells based on Quickshell, for example [DankMaterialShell](https://github.com/AvengeMedia/DankMaterialShell) and [Noctalia](https://github.com/noctalia-dev/noctalia-shell).
- [LXQt](https://lxqt-project.org/) on Wayland: see [their wiki](https://github.com/lxqt/lxqt/wiki/ConfigWaylandSettings#general).

### NVIDIA

The NVIDIA drivers currently have an issue with high VRAM usage due to a heap reuse quirk.
You're recommended to apply a manual fix documented [here](./Nvidia.md) if you run realand on an NVIDIA GPU.

NVIDIA GPUs can have problems running realand (for example, the screen remains black upon starting from a TTY).
Sometimes, the problems can be fixed.
You can try the following:

1. Update NVIDIA drivers. You need a GPU and drivers recent enough to support GBM.
2. Make sure kernel modesetting is enabled. This usually involves adding `nvidia-drm.modeset=1` to the kernel command line. Find and follow a guide for your distribution. Guides from other Wayland compositors can help.

### Asahi, ARM, and other kmsro devices

On some of these systems, realand fails to correctly detect the primary render device.
If you're getting a black screen when starting realand on a TTY, you can try to set the device manually.

First, find which devices you have:

```
$ ls -l /dev/dri/
drwxr-xr-x@       - root 14 мая 07:07 by-path
crw-rw----@   226,0 root 14 мая 07:07 card0
crw-rw----@   226,1 root 14 мая 07:07 card1
crw-rw-rw-@ 226,128 root 14 мая 07:07 renderD128
crw-rw-rw-@ 226,129 root 14 мая 07:07 renderD129
```

You will likely have one `render` device and two `card` devices.

Open the realand config file at `~/.config/realand/config.kdl` and put your `render` device path like this:

```kdl
debug {
    render-drm-device "/dev/dri/renderD128"
}
```

Save, then try to start realand again.
If you still get a black screen, try using each of the `card` devices.

### Nix/NixOS

There's a common problem of mesa drivers going out of sync with realand, so make sure your system mesa version matches the realand mesa version.
When this happens, you usually see a black screen when trying to start realand from a TTY.

Also, on Intel graphics, you may need a workaround described [here](https://wiki.nixos.org/wiki/Intel_Graphics).

### Virtual Machines

To run realand in a VM, make sure to enable 3D acceleration.

## Main Default Hotkeys

When running on a TTY, the Mod key is <kbd>Super</kbd>.
When running in a window, the Mod key is <kbd>Alt</kbd>.

The general system is: if a hotkey switches somewhere, then adding <kbd>Ctrl</kbd> will move the focused window or column there.

| Hotkey | Description |
| ------ | ----------- |
| <kbd>Mod</kbd><kbd>Shift</kbd><kbd>/</kbd> | Show a list of important realand hotkeys |
| <kbd>Mod</kbd><kbd>T</kbd> | Spawn `alacritty` (terminal) |
| <kbd>Mod</kbd><kbd>D</kbd> | Spawn `fuzzel` (application launcher) |
| <kbd>Super</kbd><kbd>Alt</kbd><kbd>L</kbd> | Spawn `swaylock` (screen locker) |
| <kbd>Mod</kbd><kbd>Q</kbd> | Close the focused window |
| <kbd>Mod</kbd><kbd>H</kbd> or <kbd>Mod</kbd><kbd>←</kbd> | Focus the column to the left |
| <kbd>Mod</kbd><kbd>L</kbd> or <kbd>Mod</kbd><kbd>→</kbd> | Focus the column to the right |
| <kbd>Mod</kbd><kbd>J</kbd> or <kbd>Mod</kbd><kbd>↓</kbd> | Focus the window below in a column |
| <kbd>Mod</kbd><kbd>K</kbd> or <kbd>Mod</kbd><kbd>↑</kbd> | Focus the window above in a column |
| <kbd>Mod</kbd><kbd>Ctrl</kbd><kbd>H</kbd> or <kbd>Mod</kbd><kbd>Ctrl</kbd><kbd>←</kbd> | Move the focused column to the left |
| <kbd>Mod</kbd><kbd>Ctrl</kbd><kbd>L</kbd> or <kbd>Mod</kbd><kbd>Ctrl</kbd><kbd>→</kbd> | Move the focused column to the right |
| <kbd>Mod</kbd><kbd>Ctrl</kbd><kbd>J</kbd> or <kbd>Mod</kbd><kbd>Ctrl</kbd><kbd>↓</kbd> | Move the focused window below in a column |
| <kbd>Mod</kbd><kbd>Ctrl</kbd><kbd>K</kbd> or <kbd>Mod</kbd><kbd>Ctrl</kbd><kbd>↑</kbd> | Move the focused window above in a column |
| <kbd>Mod</kbd><kbd>Shift</kbd><kbd>H</kbd><kbd>J</kbd><kbd>K</kbd><kbd>L</kbd> or <kbd>Mod</kbd><kbd>Shift</kbd><kbd>←</kbd><kbd>↓</kbd><kbd>↑</kbd><kbd>→</kbd> | Focus the monitor to the side |
| <kbd>Mod</kbd><kbd>Ctrl</kbd><kbd>Shift</kbd><kbd>H</kbd><kbd>J</kbd><kbd>K</kbd><kbd>L</kbd> or <kbd>Mod</kbd><kbd>Ctrl</kbd><kbd>Shift</kbd><kbd>←</kbd><kbd>↓</kbd><kbd>↑</kbd><kbd>→</kbd> | Move the focused column to the monitor to the side |
| <kbd>Mod</kbd><kbd>U</kbd> or <kbd>Mod</kbd><kbd>PageDown</kbd> | Switch to the workspace below |
| <kbd>Mod</kbd><kbd>I</kbd> or <kbd>Mod</kbd><kbd>PageUp</kbd> | Switch to the workspace above |
| <kbd>Mod</kbd><kbd>Ctrl</kbd><kbd>U</kbd> or <kbd>Mod</kbd><kbd>Ctrl</kbd><kbd>PageDown</kbd> | Move the focused column to the workspace below |
| <kbd>Mod</kbd><kbd>Ctrl</kbd><kbd>I</kbd> or <kbd>Mod</kbd><kbd>Ctrl</kbd><kbd>PageUp</kbd> | Move the focused column to the workspace above |
| <kbd>Mod</kbd><kbd>Shift</kbd><kbd>U</kbd> or <kbd>Mod</kbd><kbd>Shift</kbd><kbd>PageDown</kbd> | Move the focused workspace down |
| <kbd>Mod</kbd><kbd>Shift</kbd><kbd>I</kbd> or <kbd>Mod</kbd><kbd>Shift</kbd><kbd>PageUp</kbd> | Move the focused workspace up |
| <kbd>Mod</kbd><kbd>[</kbd> | Consume or expel the focused window to the left |
| <kbd>Mod</kbd><kbd>]</kbd> | Consume or expel the focused window to the right |
| <kbd>Mod</kbd><kbd>R</kbd> and <kbd>Mod</kbd><kbd>Shift</kbd><kbd>R</kbd> | Toggle between preset column widths forward and back |
| <kbd>Mod</kbd><kbd>M</kbd> | Maximize window |
| <kbd>Mod</kbd><kbd>C</kbd> | Center column within view |
| <kbd>Mod</kbd><kbd>-</kbd> | Decrease column width by 10% |
| <kbd>Mod</kbd><kbd>=</kbd> | Increase column width by 10% |
| <kbd>Mod</kbd><kbd>Shift</kbd><kbd>-</kbd> | Decrease window height by 10% |
| <kbd>Mod</kbd><kbd>Shift</kbd><kbd>=</kbd> | Increase window height by 10% |
| <kbd>Mod</kbd><kbd>Ctrl</kbd><kbd>R</kbd> | Reset window height back to automatic |
| <kbd>Mod</kbd><kbd>Shift</kbd><kbd>F</kbd> | Toggle full-screen on the focused window |
| <kbd>Mod</kbd><kbd>V</kbd> | Move the focused window between the floating and the tiling layout |
| <kbd>Mod</kbd><kbd>Shift</kbd><kbd>V</kbd> | Switch focus between the floating and the tiling layout |
| <kbd>PrtSc</kbd> | Take an area screenshot. Select the area to screenshot with mouse, then press Space to save the screenshot, or Escape to cancel |
| <kbd>Alt</kbd><kbd>PrtSc</kbd> | Take a screenshot of the focused window to clipboard and to `~/Pictures/Screenshots/` |
| <kbd>Ctrl</kbd><kbd>PrtSc</kbd> | Take a screenshot of the focused monitor to clipboard and to `~/Pictures/Screenshots/` |
| <kbd>Mod</kbd><kbd>Shift</kbd><kbd>E</kbd> or <kbd>Ctrl</kbd><kbd>Alt</kbd><kbd>Delete</kbd> | Exit realand |

## Building

First, install the dependencies for your distribution.

- Ubuntu 24.04:

    ```sh
    sudo apt-get install -y gcc clang libudev-dev libgbm-dev libxkbcommon-dev libegl1-mesa-dev libwayland-dev libinput-dev libdbus-1-dev libsystemd-dev libseat-dev libpipewire-0.3-dev libpango1.0-dev libdisplay-info-dev
    ```

- Fedora:

    ```sh
    sudo dnf install gcc libudev-devel libgbm-devel libxkbcommon-devel wayland-devel libinput-devel dbus-devel systemd-devel libseat-devel pipewire-devel pango-devel cairo-gobject-devel clang libdisplay-info-devel
    ```

Next, get latest stable Rust: https://rustup.rs/

Then, build realand with `cargo build --release`.

Check Cargo.toml for a list of build features.
For example, you can replace systemd integration with dinit integration using `cargo build --release --no-default-features --features dinit,dbus,xdp-gnome-screencast`.

> [!WARNING]
> Do NOT build with `--all-features`!
>
> Some features are meant only for development use.
> For example, one of the features enables collection of profiling data into a memory buffer that will grow indefinitely until you run out of memory.

### NixOS/Nix

We have a community-maintained flake which provides a devshell with required dependencies. Use `nix build` to build realand, and then run `./results/bin/realand`.

If you're not on NixOS, you may need [NixGL](https://github.com/nix-community/nixGL) to run the resulting binary:

```sh
nix run --impure github:guibou/nixGL -- ./results/bin/realand
```

### Manual Installation

If installing directly without a package, the recommended file destinations are slightly different.
In this case, put the files in the directories indicated in the table below.
These may vary depending on your distribution.

Don't forget to make sure that the path to `realand` in realand.service is correct.
This defaults to `/usr/bin/realand`.

| File | Destination |
| ---- | ----------- |
| `target/release/realand` | `/usr/local/bin/` |
| `resources/realand-session` | `/usr/local/bin/` |
| `resources/realand.desktop`  | `/usr/local/share/wayland-sessions/` |
| `resources/realand-portals.conf` | `/usr/local/share/xdg-desktop-portal/` |
| `resources/realand.service` (systemd) | `/etc/systemd/user/` |
| `resources/realand-shutdown.target` (systemd) | `/etc/systemd/user/` |
| `resources/dinit/realand` (dinit) | `/etc/dinit.d/user/` |
| `resources/dinit/realand.target` (dinit) | `/etc/dinit.d/user/` |

[Alacritty]: https://github.com/alacritty/alacritty
[fuzzel]: https://codeberg.org/dnkl/fuzzel
