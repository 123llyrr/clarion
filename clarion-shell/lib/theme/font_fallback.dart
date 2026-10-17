// SPDX-License-Identifier: GPL-3.0-or-later

/// Font families the shell renders with, resolved through the system font
/// configuration (fontconfig on Linux).
///
/// Clarion ships no font binaries and never requests Apple families such as SF
/// Pro or PingFang: they are proprietary and cannot accompany a GPL-3.0-or-
/// later desktop. The chain names only open-licensed families, CJK before
/// Latin, so Chinese labels keep their glyphs while Latin text gets a matching
/// sans face on systems that have Noto Sans installed.
const List<String> shellFontFallback = [
  // Open-licensed CJK families, most common first.
  'Noto Sans CJK SC',
  'Noto Sans CJK TC',
  'Noto Sans CJK JP',
  'Source Han Sans SC',
  'Source Han Sans CN',
  'Sarasa Gothic SC',
  'WenQuanYi Micro Hei',
  'HarmonyOS Sans SC',
  // Latin coverage for systems without any CJK family at all.
  'Noto Sans',
  'Inter',
  'DejaVu Sans',
  'Liberation Sans',
];

/// Preferred family. Resolved by fontconfig; the chain above covers systems
/// where it is not installed.
const String shellFontFamily = 'Noto Sans';
