# Realand Documentation

Realand is a Rust Wayland desktop with a device-independent core, a native Vulkan
compositor backend, and an integrated desktop shell. Floating, scrolling and dynamic
tiling share window identity and workspace state.

The Vulkan backend handles composition, blur, liquid glass and animated window
effects. It uses asynchronous GPU completion and per-output presentation budgets;
GLES remains available for compatibility. The shell provides the launcher, Dock,
control center, notifications and desktop controls.

- [Get started](./Getting-Started.md) with building and running Realand.
- [Explore desktop features](./Realand-Features.md), layouts and animation controls.
- [Configure Realand](./Configuration:-Introduction.md) with KDL and the settings app.
- [Read the design principles](./Development:-Design-Principles.md) and
  [animation timing](./Development:-Animation-Timing.md) notes.
- [Develop Realand](./Development:-Developing-realand.md) or contribute to the
  [documentation](./Development:-Documenting-realand.md).

To select Vulkan, add this top-level option to `~/.config/realand/config.kdl`:

```kdl
renderer "vulkan"
```

Renderer changes take effect when starting a new Realand session. Driver and
display capabilities determine which presentation and synchronization paths are
available.

These pages are maintained in the repository's `docs/wiki/` directory.
