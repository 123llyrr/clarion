// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter_svg/flutter_svg.dart';

/// Appearance assets retain their images; built-in icons cover missing files.
class OptionalAssetIcon extends StatelessWidget {
  const OptionalAssetIcon({
    super.key,
    required this.asset,
    required this.fallback,
    required this.width,
    required this.height,
    this.preferredAsset,
    this.rasterSize,
    this.fallbackColor,
    this.filterQuality = FilterQuality.medium,
  });

  final String asset;
  final IconData fallback;
  final double width;
  final double height;

  /// A locally imported look-alike rendered ahead of [asset] when present,
  /// e.g. a user-extracted original kept out of the distribution tree.
  final String? preferredAsset;
  final int? rasterSize;
  final Color? fallbackColor;
  final FilterQuality filterQuality;

  Widget _fallbackWidget(BuildContext context) => SizedBox(
    width: width,
    height: height,
    child: Icon(
      fallback,
      size: math.min(width, height),
      color: fallbackColor ?? Theme.of(context).colorScheme.primary,
    ),
  );

  Widget _assetImage(
    BuildContext context,
    String asset,
    WidgetBuilder onMissing,
  ) {
    if (asset.endsWith('.svg')) {
      return SvgPicture.asset(
        asset,
        width: width,
        height: height,
        fit: BoxFit.contain,
        errorBuilder: (_, _, _) => onMissing(context),
      );
    }
    final ImageProvider image = rasterSize == null
        ? AssetImage(asset)
        : ResizeImage(
            AssetImage(asset),
            width: rasterSize,
            height: rasterSize,
            policy: ResizeImagePolicy.fit,
            allowUpscaling: false,
          );
    return Image(
      image: image,
      width: width,
      height: height,
      fit: BoxFit.contain,
      filterQuality: filterQuality,
      gaplessPlayback: true,
      excludeFromSemantics: true,
      errorBuilder: (_, _, _) => onMissing(context),
    );
  }

  @override
  Widget build(BuildContext context) {
    final preferred = preferredAsset;
    if (preferred == null) {
      return _assetImage(context, asset, _fallbackWidget);
    }
    return _assetImage(
      context,
      preferred,
      (context) => _assetImage(context, asset, _fallbackWidget),
    );
  }
}

/// Text-rendered Clarion identity; needs no private artwork or bundled font.
class ClarionMark extends StatelessWidget {
  const ClarionMark({super.key, this.size = 16, this.width, this.color});

  final double size;
  final double? width;
  final Color? color;

  @override
  Widget build(BuildContext context) => SizedBox(
    width: width ?? size,
    height: size,
    child: FittedBox(
      child: Text(
        'C',
        style: TextStyle(
          fontFamily: 'sans-serif',
          fontSize: size,
          fontWeight: FontWeight.w800,
          color: color ?? Theme.of(context).colorScheme.primary,
        ),
      ),
    ),
  );
}
