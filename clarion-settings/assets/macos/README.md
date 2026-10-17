# Settings reference assets

The Apple preference-pane bitmaps that used to live here were removed on
2026-10-17. Clarion is distributed under GPL-3.0-or-later and Apple's licence
does not permit redistributing its artwork, so no bitmap can ship in this tree.

`provenance.json` is kept as the extraction record: source package paths,
SHA-256 values and conversion method for each removed file. It documents what
was removed, not a right to redistribute it.

## What the app does instead

Every preference pane draws `PreferenceIcon::vector_fallback()` — an original
vector icon in `src/icons.rs`. `PreferenceIcon::asset()` returns `None`, and
`paint_preference` falls back to the vector painter at any DPI.

## Restoring Apple bitmaps locally

    scripts/import-macos-assets.sh --source <dir-with-extracted-files>

The script verifies each file against
`resources/licenses/removed-apple-assets.sha256` and copies it into this
directory. Imported files are git-ignored.

The settings binary only compiles a bitmap that is referenced by
`include_bytes!`; after an import those five entries have to be restored in
`src/icons.rs` by hand. Until then the app keeps drawing vector icons, which is
the shipped default.

Text rendering never depends on these files: `theme::font_definitions()` resolves
SF Pro Text / PingFang SC through fontconfig and otherwise uses the configured
system sans faces, with egui's bundled defaults as the final fallback.
