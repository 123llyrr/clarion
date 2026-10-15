# realand-ipc

Types and helpers for interfacing with the [Realand](https://github.com/realand-wm/realand) Wayland compositor.

## Backwards compatibility

This crate follows the Realand workspace version.
It is **not** API-stable in terms of the Rust semver.
In particular, expect new struct fields and enum variants to be added in patch version bumps.

Use an exact version requirement to avoid breaking changes:

```toml
[dependencies]
realand-ipc = "=0.1.0"
```
