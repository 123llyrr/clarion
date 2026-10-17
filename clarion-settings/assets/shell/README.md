# Settings reference assets

The Apple preference-pane bitmaps that used to live here were removed on
2026-10-17. Clarion is distributed under GPL-3.0-or-later and Apple's licence
does not permit redistributing its artwork, so no Apple bitmap can ship in this
tree.

`provenance.json` is kept as the extraction record: source package paths,
SHA-256 values and conversion method for each removed file. It documents what
was removed, not a right to redistribute it.

## What the app uses instead

The preference home draws the eight Clarion pane icons in
`../icons/` (`PreferenceIcon::asset()` embeds them with `include_bytes!`).
A pane whose bitmap is missing falls back to `PreferenceIcon::vector_fallback()`
— an original vector icon in `src/icons.rs` — so the grid always renders.

## Restoring Apple bitmaps locally

    scripts/import-macos-assets.sh --source <dir-with-extracted-files>

The script verifies each file against
`resources/licenses/removed-apple-assets.sha256` and copies it into this
directory. Imported files are git-ignored.

The settings binary only compiles a bitmap that is referenced by
`include_bytes!`; after an import those five entries would have to be restored
in `src/icons.rs` by hand. The shipped icons live in `../icons/` and are not
affected by this local import path.

Text rendering never depends on these files: `theme::font_definitions()` resolves
SF Pro Text / PingFang SC through fontconfig and otherwise uses the configured
system sans faces, with egui's bundled defaults as the final fallback.
