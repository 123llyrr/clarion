# Clarion icon set

App and place icons for the shell, derived from
[WhiteSur-icon-theme](https://github.com/vinceliuice/WhiteSur-icon-theme)
by vinceliuice and contributors, licensed GPL-3.0-or-later (see LICENSE).
Files were renamed to Clarion's internal role names; the artwork is
otherwise unmodified.

## Screenshot action mark

`lucide-scan-line.svg` is the unmodified `scan-line` icon from
[Lucide](https://github.com/lucide-icons/lucide/blob/1ef60af944de207e0d5176905a0759ba12e6ee8d/icons/scan-line.svg),
revision `1ef60af944de207e0d5176905a0759ba12e6ee8d`, distributed under the ISC
license. Its complete upstream license is retained in `LUCIDE-LICENSE`, including
the notices for Feather-derived icons. It is used as a monochrome action mark in
the native menu bar and screenshot controls; the themed screenshot
application artwork remains separate.

## Rasterized icons

`activity-monitor.png`, `preview.png`, `launchpad.png`, `trash-empty.png`,
`trash-full.png` and `screenshot.png` are shipped as PNGs rendered from the
upstream SVGs with librsvg. These pre-rendered assets retain the upstream filter shading with a fixed
appearance across clients.

`display.png` and `storage.png` are Clarion's own artwork for the About panel's
monitor and storage sections, alongside the project mark.

`clarion-overview.png` is the circular About-panel artwork supplied by the
project owner.

## Native action icons

`bell.svg`, `circle.svg`, `clipboard.svg`, `grid.svg`, `scan.svg`, `sliders.svg`,
`x.svg`, `control-center.svg` and `moon.svg` are compact geometric action marks authored for the native Shell and
distributed under the project GPL-3.0-or-later license. The native GPUI interface
uses a single macOS-inspired appearance; the retired Flutter and Material UIs
are not shipped as alternative styles.

`sun.svg`, `speaker.svg`, `speaker-muted.svg`, `display.svg`, `settings.svg` and
`wallpaper.svg` are also original Clarion action marks under GPL-3.0-or-later,
used by the native Control Center. They contain no Apple artwork.

`capture-screen.svg`, `capture-window.svg`, and `capture-region.svg` are original capture-toolbar glyphs under GPL-3.0-or-later.
