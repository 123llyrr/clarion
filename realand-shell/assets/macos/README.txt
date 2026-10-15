Original macOS Catalina resources from the user-supplied image.

Apple logos: original AuthKitUI PDFs preserved in sources/; PNGs use transparent 512px Poppler rasterization and SVGs preserve original vector paths/page canvas. No tracing or redrawing.
Finder, Trash, Applications, Desktop Folder and Generic Application icons: original 1024x1024 PNG chunks extracted unchanged from the full Core.pkg CoreTypes ICNS files, with original alpha. These replace the earlier reduced 256px BaseSystem copies.
SFNS.ttf: unchanged original variable TrueType .SF NS font; default weight 400, wght axis 1-1000, Latin/system symbols only.
PingFangSC-Regular.otf and PingFangSC-Semibold.otf: complete original SC CFF faces extracted from PingFang.ttc, indices 2/8, for Chinese fallback. No subsetting or glyph alterations.
HelveticaNeue.ttc inspected and recorded; not copied into the runtime assets.

provenance.json maps every asset to its source path, conversion method and SHA-256. Apple source resources keep their original attribution; extraction does not change their ownership or licensing. The source DMG was read only and its SHA-256 was unchanged after extraction.

Terminal and Safari now use original 1024x1024 PNG chunks recovered from the full Core.pkg application/template bundles, replacing the earlier reduced 256px BaseSystem icon references. These asset files do not install, launch or emulate their macOS applications.
The -tight Apple logo variants only remove wholly transparent outer canvas padding for centered menu-bar alignment; their silhouette pixels/colors are unchanged.
Detailed original font metadata is in font-inspection.json. PingFang standalone CFF table, advance widths and Unicode cmap were verified identical to their source faces.
Generic Folder, Generic Document, Downloads and Alias Badge use the full Core.pkg original 1024x1024 CoreTypes PNG chunks unchanged, including their original alpha.

launchpad.png and mission-control.png are unchanged 1024x1024 PNG chunks (ic10) from /System/Applications/Launchpad.app/Contents/Resources/Launchpad.icns and /System/Applications/Mission Control.app/Contents/Resources/Expose.icns in Core.pkg. They replace the previous 60x60 Apple Help web icons. No upscaling, redraw or pixel modification was performed. Both original ICNS files match their bundles' embedded CodeResources SHA-1 and SHA-256 entries. The image tail is incomplete; recovery of these selected resources and their hashes does not verify Apple's certificate trust or the complete image/package.

System application icon completion:
- TextEdit, Calendar, Preview and Font Book use their original 1024x1024 ic10 PNG frames, selected by the actual app Info.plist CFBundleIconFile.
- Calculator, Screenshot and Activity Monitor use the largest unchanged PNG present in their corresponding original CFBundleIconFile ICNS: 256x256 ic13. These particular fallback ICNS files do not contain 512px/1024px or JPEG2000 frames. Their source resolution is recorded honestly; no invented upscale or replacement artwork is used.
- System Preferences.app lies beyond the recoverable source tail. No system-preferences.png has been added; the shell retains the actual installed Linux settings application's native icon instead of substituting an unrelated resource.
Every updated/new full-package ICNS matches both embedded CodeResources resource hashes (SHA-1 and SHA-256). Original PNG bytes are copied directly, including their alpha; no interpolation, redraw, color changes or Apple executable execution occurs. Per-asset provenance retains previous BaseSystem records under previous_versions. Recovery checks resource integrity and does not verify Apple's certificate trust, the entire package or completeness of the user-supplied DMG.
Third party Linux applications retain their own identities and original icons; recovered macOS icons are intended for explicit matching system application roles only.
