// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/painting.dart';

import '../mac_shell_preferences.dart';
import '../models.dart';

/// Shared logical geometry for the desktop strip and its floating menus.
abstract final class ShellGeometry {
  static const double barHeight = 40;
  static const double tooltipHeight = 128;
  static const double panelGap = 12;
  static const double panelTop = barHeight + panelGap;
  static const double menuBarHeight = 22;
  static const double dockHeight = 64;
  // Magnification draws above the reserved desktop edge without resizing the
  // Wayland surface on every pointer frame.
  static const double dockSurfaceHeight = 120;

  static double barHeightFor(
    ShellStyle style, {
    MacShellPreferences macPreferences = const MacShellPreferences(),
  }) => style == ShellStyle.macos ? macPreferences.menuBarHeight : barHeight;

  static double tooltipHeightFor(
    ShellStyle style, {
    MacShellPreferences macPreferences = const MacShellPreferences(),
  }) =>
      barHeightFor(style, macPreferences: macPreferences) +
      (tooltipHeight - barHeight);

  /// Desktop files, pagination and tooltips share the same reserved edges.
  static EdgeInsets desktopInsets(
    ShellStyle style,
    BarPosition position, {
    MacShellPreferences macPreferences = const MacShellPreferences(),
  }) {
    if (style == ShellStyle.macos) {
      final top = macPreferences.menuBarAutoHide
          ? 0.0
          : macPreferences.menuBarHeight;
      final dock = macPreferences.dockAutoHide || macPreferences.dockHidden
          ? 0.0
          : macPreferences.dockSize + 16;
      return switch (macPreferences.dockPosition) {
        MacDockPosition.bottom => EdgeInsets.only(top: top, bottom: dock),
        MacDockPosition.left => EdgeInsets.only(top: top, left: dock),
        MacDockPosition.right => EdgeInsets.only(top: top, right: dock),
      };
    }
    return position == BarPosition.top
        ? const EdgeInsets.only(top: barHeight)
        : const EdgeInsets.only(bottom: barHeight);
  }
}
