# clarion-ipc

Types and helpers for interfacing with the [Clarion](https://github.com/123llyrr/clarion) Wayland compositor.

## Backwards compatibility

This crate follows the Clarion workspace version.
It is **not** API-stable in terms of the Rust semver.
In particular, expect new struct fields and enum variants to be added in patch version bumps.

Use an exact version requirement to avoid breaking changes:

```toml
[dependencies]
clarion-ipc = "=0.1.0"
```
