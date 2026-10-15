// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';

import '../mac_app_icons.dart';
import '../models.dart';
import 'app_icon.dart';

/// Shared application identity for the macOS Dock, Launchpad, and folders.
///
/// The option affects only presentation. Applications still launch through
/// their installed desktop entries, and other shell styles keep [AppIcon].
class MacAppIcon extends StatelessWidget {
  const MacAppIcon({
    super.key,
    required this.app,
    this.size = 40,
    this.rasterSize,
    this.useOriginalIcons = true,
  }) : assert(rasterSize == null || rasterSize > 0);

  final DesktopApp app;
  final double size;

  /// Maximum logical extent, including a parent's magnification transform.
  final double? rasterSize;
  final bool useOriginalIcons;

  AppIcon _nativeIcon({bool originalFallback = false}) => AppIcon(
    app: app,
    size: size,
    rasterSize: rasterSize,
    fallbackAssetPath: originalFallback ? MacAppIcons.genericApplication : null,
  );

  @override
  Widget build(BuildContext context) {
    if (!useOriginalIcons) return _nativeIcon();
    final asset = MacAppIcons.assetFor(app);
    if (asset == null) return _nativeIcon(originalFallback: true);
    final pixelSize =
        ((rasterSize ?? size) * MediaQuery.devicePixelRatioOf(context))
            .ceil()
            .clamp(16, 512);
    return Image(
      key: ValueKey(asset),
      image: ResizeImage(
        AssetImage(asset),
        width: pixelSize,
        height: pixelSize,
        policy: ResizeImagePolicy.fit,
        allowUpscaling: false,
      ),
      width: size,
      height: size,
      fit: BoxFit.contain,
      filterQuality: FilterQuality.medium,
      gaplessPlayback: true,
      errorBuilder: (_, _, _) => _nativeIcon(originalFallback: true),
    );
  }
}
