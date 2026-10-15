// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:math' as math;
import 'package:flutter/widgets.dart';
import '../mac_shell_preferences.dart';

/// Output-local placement shared by context menus and folder stacks. The
/// magnified icon is the anchor, rather than the native surface's empty band.
Rect macDockPanelGeometry({
  required Size output,
  required MacDockPosition position,
  required double width,
  required double height,
  required double menuBarHeight,
  required double dockThickness,
  Rect? anchor,
}) {
  final inset = math.min(12.0, output.width / 4);
  final top = math.min(output.height, menuBarHeight + 8);
  final panelWidth = math.min(width, math.max(0.0, output.width - 2 * inset));
  final safeAnchor = anchor != null && anchor.isFinite && !anchor.isEmpty
      ? anchor
      : switch (position) {
          MacDockPosition.bottom => Rect.fromLTWH(
            output.width / 2,
            math.max(top, output.height - dockThickness),
            0,
            0,
          ),
          MacDockPosition.left => Rect.fromLTWH(
            dockThickness,
            output.height / 2,
            0,
            0,
          ),
          MacDockPosition.right => Rect.fromLTWH(
            output.width - dockThickness,
            output.height / 2,
            0,
            0,
          ),
        };
  final bottom = position == MacDockPosition.bottom
      ? (safeAnchor.top - 8).clamp(top, output.height).toDouble()
      : output.height - 12;
  final panelHeight = math.min(height, math.max(0.0, bottom - top));
  final x = switch (position) {
    MacDockPosition.bottom => safeAnchor.center.dx - panelWidth / 2,
    MacDockPosition.left => safeAnchor.right + 8,
    MacDockPosition.right => safeAnchor.left - 8 - panelWidth,
  };
  final y = position == MacDockPosition.bottom
      ? bottom - panelHeight
      : safeAnchor.center.dy - panelHeight / 2;
  return Rect.fromLTWH(
    x
        .clamp(inset, math.max(inset, output.width - panelWidth - inset))
        .toDouble(),
    y.clamp(top, math.max(top, output.height - panelHeight - 12)).toDouble(),
    panelWidth,
    panelHeight,
  );
}
