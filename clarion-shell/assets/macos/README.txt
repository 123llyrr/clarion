macOS reference assets: removed from this repository
====================================================

This directory used to carry artwork and fonts extracted from a macOS Catalina
10.15.7 (19H2) image: Finder, Dock, Launchpad, System Preferences and system
application icons, the Apple wordmark, Dock backgrounds, search/close controls,
and the SF (SFNS.ttf) and PingFang SC fonts.

They are gone on purpose.

Clarion is distributed under GPL-3.0-or-later. Those files remain Apple's
property; Apple's licence does not permit redistribution, and the GPL cannot
grant a right the copyright holder never gave. Shipping them in a GPL repository
made every published tarball, package and container image unlicensed.

What is still here
------------------
provenance.json       Original extraction record: source package paths, SHA-256
                      values and conversion method for each removed file. Kept as
                      an audit trail, not as a claim of redistribution rights.
font-inspection.json  Font table metadata recorded during the original
                      extraction; contains no glyph outlines or font binaries.

Where the removed files went
----------------------------
resources/licenses/removed-apple-assets.sha256 lists all removed files with
their SHA-256. That manifest is the contract for restoring them locally.

Restoring them on a machine you own
-----------------------------------
    scripts/import-macos-assets.sh --source <dir-with-extracted-files>

The script verifies every file against the manifest, copies it back into this
directory (and clarion-settings/assets/macos/), and re-adds the Flutter asset and
font declarations to clarion-shell/pubspec.yaml. Imported files are ignored by
git, so a local import never re-enters the repository.

Without an import the shell and settings app keep working:

  * Bitmap icons fall back to bundled vector or Material symbols
    (OptionalAssetIcon.errorBuilder, PreferenceIcon::vector_fallback).
  * The settings app resolves its text face through fontconfig, preferring an
    installed SF Pro / PingFang SC and otherwise using the configured system
    sans faces.
  * The menu-bar slot draws the Clarion mark instead of the Apple wordmark.

Trademark note
--------------
The Apple logo and the macOS application icons are registered trademarks of
Apple Inc. Even where a user lawfully owns a macOS image, redistributing those
marks is a separate restriction from copyright. Do not commit imported files.
