// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';

/// Genuine ChromeOS Material You (Jelly) design tokens extracted from
/// ChromeOS / FydeOS root filesystem (res_57941.html & sys tokens).
abstract final class CrosTokens {
  // Light mode tokens
  static const lightPrimary = Color(0xff405d98);
  static const lightOnPrimary = Color(0xffffffff);
  static const lightPrimaryContainer = Color(0xffd8e2ff);
  static const lightOnPrimaryContainer = Color(0xff001a41);
  static const lightSecondary = Color(0xff585e71);
  static const lightSecondaryContainer = Color(0xffdbe2f9);
  static const lightOnSecondaryContainer = Color(0xff141b2c);
  static const lightSurface = Color(0xfffefbff); // cros-sys-app_base
  static const lightSurfaceContainerLow = Color(0xffeff0fa); // cros-sys-app_base_shaded
  static const lightSurfaceContainer = Color(0xffeff0fa);
  static const lightSurfaceContainerHigh = Color(0xffe7e8f2);
  static const lightSurfaceContainerHighest = Color(0xffe1e2ec);
  static const lightOnSurface = Color(0xff1a1b20);
  static const lightOnSurfaceVariant = Color(0xff44474f);
  static const lightOutline = Color(0xff74777f);
  static const lightOutlineVariant = Color(0xffc4c6d0);
  static const lightSeparator = Color(0x231a1b20);
  static const lightHoverOnSubtle = Color(0x0f1a1b20);
  static const lightFocusRing = Color(0xff3e5e98);
  static const lightRipplePrimary = Color(0x518dabeb);

  // Dark mode tokens
  static const darkPrimary = Color(0xffaec6ff);
  static const darkOnPrimary = Color(0xff002b73);
  static const darkPrimaryContainer = Color(0xff3d475c);
  static const darkOnPrimaryContainer = Color(0xffd8e2ff);
  static const darkSecondary = Color(0xffc1c6dc);
  static const darkSecondaryContainer = Color(0xff3d4758);
  static const darkOnSecondaryContainer = Color(0xffdbe2f9);
  static const darkSurface = Color(0xff16181c); // cros-sys-app_base
  static const darkSurfaceContainerLow = Color(0xff1b1d22);
  static const darkSurfaceContainer = Color(0xff222429);
  static const darkSurfaceContainerHigh = Color(0xff282a31);
  static const darkSurfaceContainerHighest = Color(0xff2d303b); // cros-sys-base_elevated
  static const darkOnSurface = Color(0xffe2e2e9);
  static const darkOnSurfaceVariant = Color(0xffc5c6d0);
  static const darkOutline = Color(0xff8e9099);
  static const darkOutlineVariant = Color(0xff44474f);
  static const darkSeparator = Color(0x23e2e2e9);
  static const darkHoverOnSubtle = Color(0x19fefbff);
  static const darkFocusRing = Color(0xffaec6ff);
  static const darkRipplePrimary = Color(0x517390cf);
}

/// Material 3 roles are shared by the bar, menus and every control.
ThemeData buildShellTheme(
  Brightness brightness, {
  Color seed = const Color(0xff6750a4),
}) {
  final isDefaultSeed = seed == const Color(0xff6750a4) || seed == CrosTokens.lightPrimary;
  final colors = isDefaultSeed
      ? (brightness == Brightness.light
          ? const ColorScheme.light(
              primary: CrosTokens.lightPrimary,
              onPrimary: CrosTokens.lightOnPrimary,
              primaryContainer: CrosTokens.lightPrimaryContainer,
              onPrimaryContainer: CrosTokens.lightOnPrimaryContainer,
              secondary: CrosTokens.lightSecondary,
              secondaryContainer: CrosTokens.lightSecondaryContainer,
              onSecondaryContainer: CrosTokens.lightOnSecondaryContainer,
              surface: CrosTokens.lightSurface,
              onSurface: CrosTokens.lightOnSurface,
              onSurfaceVariant: CrosTokens.lightOnSurfaceVariant,
              outline: CrosTokens.lightOutline,
              outlineVariant: CrosTokens.lightOutlineVariant,
              surfaceContainerLowest: Color(0xffffffff),
              surfaceContainerLow: CrosTokens.lightSurfaceContainerLow,
              surfaceContainer: CrosTokens.lightSurfaceContainer,
              surfaceContainerHigh: CrosTokens.lightSurfaceContainerHigh,
              surfaceContainerHighest: CrosTokens.lightSurfaceContainerHighest,
            )
          : const ColorScheme.dark(
              primary: CrosTokens.darkPrimary,
              onPrimary: CrosTokens.darkOnPrimary,
              primaryContainer: CrosTokens.darkPrimaryContainer,
              onPrimaryContainer: CrosTokens.darkOnPrimaryContainer,
              secondary: CrosTokens.darkSecondary,
              secondaryContainer: CrosTokens.darkSecondaryContainer,
              onSecondaryContainer: CrosTokens.darkOnSecondaryContainer,
              surface: CrosTokens.darkSurface,
              onSurface: CrosTokens.darkOnSurface,
              onSurfaceVariant: CrosTokens.darkOnSurfaceVariant,
              outline: CrosTokens.darkOutline,
              outlineVariant: CrosTokens.darkOutlineVariant,
              surfaceContainerLowest: Color(0xff0f1114),
              surfaceContainerLow: CrosTokens.darkSurfaceContainerLow,
              surfaceContainer: CrosTokens.darkSurfaceContainer,
              surfaceContainerHigh: CrosTokens.darkSurfaceContainerHigh,
              surfaceContainerHighest: CrosTokens.darkSurfaceContainerHighest,
            ))
      : ColorScheme.fromSeed(seedColor: seed, brightness: brightness);

  return ThemeData(
    useMaterial3: true,
    brightness: brightness,
    colorScheme: colors,
    scaffoldBackgroundColor: Colors.transparent,
    visualDensity: VisualDensity.standard,
    materialTapTargetSize: MaterialTapTargetSize.padded,
    fontFamilyFallback: const ['Noto Sans CJK SC', 'Source Han Sans CN'],
    cardTheme: CardThemeData(
      color: colors.surfaceContainerLow,
      elevation: 0,
      margin: EdgeInsets.zero,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(16)),
    ),
    iconButtonTheme: IconButtonThemeData(
      style: IconButton.styleFrom(minimumSize: const Size.square(48)),
    ),
    filledButtonTheme: FilledButtonThemeData(
      style: FilledButton.styleFrom(minimumSize: const Size(48, 48)),
    ),
    outlinedButtonTheme: OutlinedButtonThemeData(
      style: OutlinedButton.styleFrom(minimumSize: const Size(48, 48)),
    ),
    textButtonTheme: TextButtonThemeData(
      style: TextButton.styleFrom(minimumSize: const Size(48, 48)),
    ),
    inputDecorationTheme: InputDecorationTheme(
      filled: true,
      fillColor: colors.surfaceContainerHighest,
      border: OutlineInputBorder(
        borderRadius: BorderRadius.circular(28),
        borderSide: BorderSide.none,
      ),
      focusedBorder: OutlineInputBorder(
        borderRadius: BorderRadius.circular(28),
        borderSide: BorderSide(color: colors.primary, width: 2),
      ),
    ),
    tooltipTheme: const TooltipThemeData(
      waitDuration: Duration(milliseconds: 450),
    ),
  );
}

/// Motion follows Material's emphasized curves; reduced motion is immediate.
abstract final class ShellMotion {
  static const enter = Duration(milliseconds: 250);
  static const change = Duration(milliseconds: 180);
  static const curve = Cubic(0.05, 0.7, 0.1, 1.0);
  static Duration duration(bool reduceMotion, [Duration value = change]) =>
      reduceMotion ? Duration.zero : value;
}
