// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:math' as math;

import 'package:flutter/material.dart';

/// Appearance assets retain their images; built-in icons cover missing files.
class OptionalAssetIcon extends StatelessWidget {
  const OptionalAssetIcon({
    super.key,
    required this.asset,
    required this.fallback,
    required this.width,
    required this.height,
    this.rasterSize,
    this.fallbackColor,
    this.filterQuality = FilterQuality.medium,
  });

  final String asset;
  final IconData fallback;
  final double width;
  final double height;
  final int? rasterSize;
  final Color? fallbackColor;
  final FilterQuality filterQuality;

  @override
  Widget build(BuildContext context) {
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
      errorBuilder: (context, error, stackTrace) => SizedBox(
        width: width,
        height: height,
        child: Icon(
          fallback,
          size: math.min(width, height),
          color: fallbackColor ?? Theme.of(context).colorScheme.primary,
        ),
      ),
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
        'R',
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
