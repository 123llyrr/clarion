//! Shared design tokens for the compositor's built-in Cairo-rendered UI.
//!
//! These values intentionally mirror the Material 3 dark scheme used by
//! realand-shell (`realand-shell/lib/theme/shell_theme.dart`) and the palette
//! in realand-settings (`realand-settings/src/theme.rs`), so the built-in
//! panels, the shell and the settings app read as one product. Colors are
//! normalized 0..=1 RGB tuples ready for `cairo::Context::set_source_rgb`.

/// Font description used by every built-in panel.
pub const FONT: &str = "sans 14px";

/// Panel background.
pub const SURFACE: (f64, f64, f64) = (0.1, 0.1, 0.1);
/// Primary text and icons on `SURFACE`.
pub const ON_SURFACE: (f64, f64, f64) = (1., 1., 1.);
/// Secondary text, dividers and disabled glyphs.
pub const ON_SURFACE_MUTED: (f64, f64, f64) = (0.5, 0.5, 0.5);
/// Hairline borders and separators.
pub const OUTLINE: (f64, f64, f64) = (0.3, 0.3, 0.3);
/// Accent for the focused/selected entry (matches the shell's primary).
pub const PRIMARY: (f64, f64, f64) = (0.5, 0.8, 1.0);
/// Destructive actions and error text.
pub const ERROR: (f64, f64, f64) = (1., 0.3, 0.3);
