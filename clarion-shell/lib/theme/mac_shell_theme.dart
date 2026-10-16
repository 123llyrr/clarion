// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';

import 'shell_theme.dart';

/// macOS typography and control roles are scoped to the selected Mac shell.
ThemeData buildMacShellTheme(Brightness brightness) {
  final dark = brightness == Brightness.dark;
  final base = buildShellTheme(brightness, seed: const Color(0xff007aff));
  final accent = dark ? const Color(0xff0a84ff) : const Color(0xff007aff);
  final colors = base.colorScheme.copyWith(
    primary: accent,
    onPrimary: Colors.white,
    primaryContainer: dark ? const Color(0xff164370) : const Color(0xffd9eaff),
    onPrimaryContainer: dark ? Colors.white : const Color(0xff003c80),
    secondary: accent,
    onSecondary: Colors.white,
    secondaryContainer: dark
        ? const Color(0xff164370)
        : const Color(0xffd9eaff),
    onSecondaryContainer: dark ? Colors.white : const Color(0xff003c80),
    surface: dark ? const Color(0xff242426) : const Color(0xfff5f5f7),
    onSurface: dark ? const Color(0xfff5f5f7) : const Color(0xff1d1d1f),
    onSurfaceVariant: dark ? const Color(0xffb8b8bd) : const Color(0xff626266),
    surfaceContainerLow: dark ? const Color(0xff303033) : Colors.white,
    surfaceContainer: dark ? const Color(0xff2c2c2e) : const Color(0xffededf0),
    surfaceContainerHighest: dark
        ? const Color(0xff454549)
        : const Color(0xffdedee2),
    outline: dark ? const Color(0xff777780) : const Color(0xffb9b9bf),
    outlineVariant: dark ? const Color(0xff4c4c50) : const Color(0xffd6d6db),
  );
  final text = base.textTheme.apply(
    fontFamily: 'macOSSystem',
    fontFamilyFallback: const ['macOSChinese', 'Noto Sans CJK SC'],
    bodyColor: colors.onSurface,
    displayColor: colors.onSurface,
  );
  return base.copyWith(
    colorScheme: colors,
    splashFactory: NoSplash.splashFactory,
    splashColor: Colors.transparent,
    highlightColor: Colors.transparent,
    // Catalina-inspired controls. These dimensions are Clarion choices, not
    // measurements recovered from the system image. Keep Slider's desktop
    // focus, keyboard and accessibility support while replacing its paint.
    sliderTheme: SliderThemeData(
      trackHeight: 3,
      activeTrackColor: accent,
      inactiveTrackColor: colors.outlineVariant,
      disabledActiveTrackColor: colors.outline,
      disabledInactiveTrackColor: colors.outlineVariant,
      thumbColor: Colors.white,
      disabledThumbColor: colors.surfaceContainerHighest,
      trackShape: const RoundedRectSliderTrackShape(),
      thumbShape: const RoundSliderThumbShape(
        enabledThumbRadius: 7,
        disabledThumbRadius: 7,
        elevation: 1,
        pressedElevation: 2,
      ),
      // Transparent feedback retains the full hit area around the small knob.
      overlayShape: const RoundSliderOverlayShape(overlayRadius: 14),
      overlayColor: accent.withValues(alpha: .12),
      showValueIndicator: ShowValueIndicator.never,
      trackGap: 0,
    ),
    listTileTheme: const ListTileThemeData(
      minTileHeight: 30,
      horizontalTitleGap: 10,
      minLeadingWidth: 20,
    ),
    textTheme: text.copyWith(
      headlineSmall: text.headlineSmall?.copyWith(
        fontSize: 20,
        fontWeight: FontWeight.w600,
      ),
      titleLarge: text.titleLarge?.copyWith(
        fontSize: 18,
        fontWeight: FontWeight.w600,
      ),
      titleMedium: text.titleMedium?.copyWith(
        fontSize: 15,
        fontWeight: FontWeight.w600,
      ),
      titleSmall: text.titleSmall?.copyWith(
        fontSize: 13,
        fontWeight: FontWeight.w600,
      ),
      bodyLarge: text.bodyLarge?.copyWith(fontSize: 13),
      bodyMedium: text.bodyMedium?.copyWith(fontSize: 13),
      bodySmall: text.bodySmall?.copyWith(fontSize: 11),
      labelLarge: text.labelLarge?.copyWith(fontSize: 13),
      labelMedium: text.labelMedium?.copyWith(fontSize: 12),
      labelSmall: text.labelSmall?.copyWith(fontSize: 11),
    ),
    primaryTextTheme: base.primaryTextTheme.apply(
      fontFamily: 'macOSSystem',
      fontFamilyFallback: const ['macOSChinese', 'Noto Sans CJK SC'],
    ),
    cardTheme: CardThemeData(
      color: colors.surfaceContainerLow,
      elevation: 0,
      margin: EdgeInsets.zero,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(12)),
    ),
    inputDecorationTheme: InputDecorationTheme(
      filled: true,
      isDense: true,
      fillColor: colors.surfaceContainerLow,
      border: OutlineInputBorder(
        borderRadius: BorderRadius.circular(9),
        borderSide: BorderSide(color: colors.outlineVariant),
      ),
      enabledBorder: OutlineInputBorder(
        borderRadius: BorderRadius.circular(9),
        borderSide: BorderSide(color: colors.outlineVariant),
      ),
      focusedBorder: OutlineInputBorder(
        borderRadius: BorderRadius.circular(9),
        borderSide: BorderSide(color: accent, width: 2),
      ),
    ),
    segmentedButtonTheme: SegmentedButtonThemeData(
      style: SegmentedButton.styleFrom(
        minimumSize: const Size(40, 34),
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(8)),
        textStyle: text.labelLarge,
      ),
    ),
    filledButtonTheme: FilledButtonThemeData(
      style: FilledButton.styleFrom(
        minimumSize: const Size(40, 36),
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(8)),
      ),
    ),
    outlinedButtonTheme: OutlinedButtonThemeData(
      style: OutlinedButton.styleFrom(
        minimumSize: const Size(40, 36),
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(8)),
      ),
    ),
  );
}
