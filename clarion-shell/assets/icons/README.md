# Clarion icon set

App and place icons for the shell, derived from
[WhiteSur-icon-theme](https://github.com/vinceliuice/WhiteSur-icon-theme)
by vinceliuice and contributors, licensed GPL-3.0-or-later (see LICENSE).
Files were renamed to Clarion's internal role names; the artwork is
otherwise unmodified.

## Rasterized icons

`activity-monitor.png`, `preview.png`, `launchpad.png`, `trash-empty.png`,
`trash-full.png` and `screenshot.png` are shipped as PNGs rendered from the
upstream SVGs with librsvg. Those SVGs use SVG `<filter>` elements, which the
Flutter SVG renderer does not implement, so the vector form would render
without their shading.

`display.png` and `storage.png` are Clarion's own artwork for the About panel's
monitor and storage sections, alongside the project mark.

`clarion-overview.png` is the circular About-panel artwork supplied by the
project owner.
