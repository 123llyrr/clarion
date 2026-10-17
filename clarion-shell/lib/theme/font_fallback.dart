// SPDX-License-Identifier: GPL-3.0-or-later

/// Font families the shell falls back through when a preferred family is absent.
///
/// A fresh checkout ships no fonts: the imported Apple families (`ClarionSystem`,
/// `ClarionChinese`) only exist after `scripts/import-shell-assets.sh` restores
/// them from an image the user owns, and Apple's SF and PingFang are not
/// redistributable. Without a fallback chain, CJK labels would render as tofu on
/// any system that has no CJK family installed, so the chain names the common
/// open-licensed families by the names fontconfig actually exposes.
const List<String> shellFontFallback = [
  // Optional local import first: it shapes the intended look when present.
  'ClarionChinese',
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
  'Inter',
  'Noto Sans',
  'DejaVu Sans',
  'Liberation Sans',
];

/// Preferred family. It exists only after the optional import, so every use is
/// paired with [shellFontFallback].
const String shellFontFamily = 'ClarionSystem';
