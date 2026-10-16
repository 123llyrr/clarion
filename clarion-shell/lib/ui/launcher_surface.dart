// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';

import '../models.dart';
import '../mac_launchpad_layout.dart';
import '../state/shell_controller.dart';
import 'retained_surface.dart';
import 'launcher_panel.dart';
import 'mac_launcher_panel.dart';

/// Retain the native surface during exit; reversals continue from the current
/// animation value instead of recreating the application grid.
class LauncherSurface extends StatefulWidget {
  const LauncherSurface({
    super.key,
    required this.controller,
    required this.visible,
    required this.reducedMotion,
    this.onSurfaceVisible,
    this.macLayout,
  });
  final ShellController controller;
  final bool visible;
  final bool reducedMotion;
  final ValueChanged<bool>? onSurfaceVisible;
  final MacLaunchpadLayout? macLayout;

  @override
  State<LauncherSurface> createState() => _LauncherSurfaceState();
}

class _LauncherSurfaceState extends State<LauncherSurface> {
  Widget? _macPanel;
  MacLaunchpadLayout? _macLayout;
  bool _ownsMacLayout = false;
  void _clearMacLayout() {
    _macPanel = null;
    if (_ownsMacLayout) _macLayout?.dispose();
    _macLayout = null;
    _ownsMacLayout = false;
  }

  @override
  void didUpdateWidget(covariant LauncherSurface oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!identical(oldWidget.controller, widget.controller) ||
        !identical(oldWidget.macLayout, widget.macLayout)) {
      _clearMacLayout();
    }
  }

  @override
  void dispose() {
    _clearMacLayout();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    final macos = widget.controller.shellStyle == ShellStyle.macos;
    if (widget.visible && macos && _macLayout == null) {
      _ownsMacLayout = widget.macLayout == null;
      _macLayout =
          widget.macLayout ??
          MacLaunchpadLayout(persist: !widget.controller.demo);
    }
    return RetainedSurface(
      visible: widget.visible,
      reducedMotion: widget.reducedMotion,
      duration: const Duration(milliseconds: 180),
      reverseDuration: const Duration(milliseconds: 140),
      onSurfaceVisible: widget.onSurfaceVisible,
      builder: (context, constraints) => Stack(
        children: [
          if (macos)
            Positioned.fill(
              child: RepaintBoundary(
                child: _macPanel ??= MacLauncherPanel(
                  key: ObjectKey(_macLayout),
                  controller: widget.controller,
                  layout: _macLayout,
                ),
              ),
            )
          else
            Center(
              child: ConstrainedBox(
                constraints: const BoxConstraints(maxWidth: 1200),
                child: Padding(
                  padding: EdgeInsets.symmetric(
                    horizontal: constraints.maxWidth < 500 ? 12 : 32,
                    vertical: constraints.maxHeight < 360 ? 8 : 24,
                  ),
                  child: RepaintBoundary(
                    child: LauncherPanel(controller: widget.controller),
                  ),
                ),
              ),
            ),
        ],
      ),
      transitionBuilder: (context, value, child) {
        // The compositor owns the liquid-glass blur/refraction. Animate only
        // this light tint and the foreground; the native backdrop stays cached.
        final tint = macos
            ? Colors.black.withValues(alpha: .20 * value)
            : colors.surface.withValues(alpha: .96 * value);
        return Stack(
          children: [
            Positioned.fill(
              child: GestureDetector(
                behavior: HitTestBehavior.opaque,
                onTap: widget.controller.closePanel,
                child: DecoratedBox(
                  key: const ValueKey('launcher-desktop-tint'),
                  decoration: BoxDecoration(
                    color: macos ? null : tint,
                    gradient: macos
                        ? LinearGradient(
                            begin: Alignment.topLeft,
                            end: Alignment.bottomRight,
                            colors: [
                              Color.alphaBlend(
                                Colors.white.withValues(alpha: .06 * value),
                                tint,
                              ),
                              tint,
                              Color.alphaBlend(
                                Colors.black.withValues(alpha: .05 * value),
                                tint,
                              ),
                            ],
                            stops: const [0, .5, 1],
                          )
                        : null,
                  ),
                ),
              ),
            ),
            Opacity(
              opacity: value,
              child: Transform.scale(
                scale: macos ? 1.025 - .025 * value : .96 + .04 * value,
                child: child,
              ),
            ),
          ],
        );
      },
    );
  }
}
