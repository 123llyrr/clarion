// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';
import 'package:flutter_svg/flutter_svg.dart';

/// A monochrome capture mark; its surrounding control owns the accessible label.
class ScreenshotIcon extends StatelessWidget {
  const ScreenshotIcon({super.key, this.size, this.color});

  final double? size;
  final Color? color;

  @override
  Widget build(BuildContext context) {
    final theme = IconTheme.of(context);
    final extent = size ?? theme.size ?? 24;
    final foreground =
        color ?? theme.color ?? Theme.of(context).colorScheme.onSurface;
    final opacity = theme.opacity ?? 1;
    return SvgPicture.asset(
      'assets/icons/lucide-scan-line.svg',
      width: extent,
      height: extent,
      colorFilter: ColorFilter.mode(
        foreground.withValues(alpha: foreground.a * opacity),
        BlendMode.srcIn,
      ),
      excludeFromSemantics: true,
    );
  }
}
