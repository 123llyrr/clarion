// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_svg/flutter_svg.dart';

import '../models.dart';

class AppIcon extends StatelessWidget {
  const AppIcon({
    super.key,
    required this.app,
    this.size = 40,
    this.rasterSize,
    this.fallbackAssetPath,
    this.fallbackIcon,
  }) : assert(rasterSize == null || rasterSize > 0);
  final DesktopApp app;
  final double size;

  /// Largest logical size at which a parent will display this icon, including
  /// transforms and fitted boxes. Defaults to [size] and does not affect layout.
  final double? rasterSize;

  /// Optional appearance-specific fallback; native app artwork remains first.
  final String? fallbackAssetPath;
  final IconData? fallbackIcon;
  static const _channel = MethodChannel('clarion.shell/window');
  static final _resolved = <String, Future<String?>>{};
  static final _paths = <String, String>{};
  static final _missedAt = <String, DateTime>{};
  static final _svgFiles = <String, File>{};

  static File _svgFile(String path) {
    // SvgFileLoader compares File identity, so rebuilding with a new File for
    // the same path would reparse an otherwise warm icon on every page change.
    final file = _svgFiles.remove(path) ?? File(path);
    if (_svgFiles.length >= 256) _svgFiles.remove(_svgFiles.keys.first);
    _svgFiles[path] = file;
    return file;
  }

  static Future<String?> _resolve(String name, int size, int pixelSize) async {
    try {
      return await _channel.invokeMethod<String>('appIcon', {
        'name': name,
        'size': size,
        'pixelSize': pixelSize,
      });
    } on PlatformException {
      return null;
    } on MissingPluginException {
      return null;
    }
  }

  Widget _missing(BuildContext context) => Icon(
    fallbackIcon ?? Icons.apps_rounded,
    size: size,
    color: Theme.of(context).colorScheme.primary,
  );

  Widget _fallback(BuildContext context) {
    final path = fallbackAssetPath;
    if (path == null) return _missing(context);
    if (path.toLowerCase().endsWith('.svg')) {
      return SvgPicture.asset(
        path,
        key: ValueKey('fallback:$path'),
        width: size,
        height: size,
        fit: BoxFit.contain,
        errorBuilder: (_, _, _) => _missing(context),
      );
    }
    final pixels =
        ((rasterSize ?? size) * MediaQuery.devicePixelRatioOf(context))
            .ceil()
            .clamp(16, 512);
    return Image(
      key: ValueKey('fallback:$path'),
      image: ResizeImage(
        AssetImage(path),
        width: pixels,
        height: pixels,
        policy: ResizeImagePolicy.fit,
        allowUpscaling: false,
      ),
      width: size,
      height: size,
      fit: BoxFit.contain,
      filterQuality: FilterQuality.medium,
      gaplessPlayback: true,
      errorBuilder: (_, _, _) => _missing(context),
    );
  }

  Widget _image(BuildContext context, String? path, int pixelSize) {
    if (path == null || path.isEmpty) return _fallback(context);
    if (path.toLowerCase().endsWith('.svg')) {
      return SvgPicture.file(
        _svgFile(path),
        key: ValueKey(app.iconPath ?? app.iconName),
        width: size,
        height: size,
        fit: BoxFit.contain,
        placeholderBuilder: _fallback,
        errorBuilder: (_, _, _) => _fallback(context),
      );
    }
    return Image(
      key: ValueKey(app.iconPath ?? app.iconName),
      image: ResizeImage(
        FileImage(File(path)),
        width: pixelSize,
        height: pixelSize,
        policy: ResizeImagePolicy.fit,
        allowUpscaling: false,
      ),
      width: size,
      height: size,
      fit: BoxFit.contain,
      // The texture is sized for maximum magnification, so its usual display
      // may shrink it by half. Mipmaps preserve detail during that animation.
      filterQuality: FilterQuality.medium,
      // Keep the decoded texture while a larger request loads its replacement.
      gaplessPlayback: true,
      errorBuilder: (_, _, _) => _fallback(context),
    );
  }

  @override
  Widget build(BuildContext context) {
    final pixelSize =
        ((rasterSize ?? size) * MediaQuery.devicePixelRatioOf(context))
            .ceil()
            .clamp(16, 512);
    if (app.iconPath != null) return _image(context, app.iconPath, pixelSize);
    final name = app.iconName;
    if (name == null || name.isEmpty) return _fallback(context);
    final iconSize = size.ceil().clamp(16, 128);
    final iconIdentity = '$name:${Theme.of(context).brightness.name}';
    final cacheKey = '$iconIdentity:$pixelSize';
    final missed = _missedAt[cacheKey];
    if (missed != null && DateTime.now().difference(missed).inSeconds >= 30) {
      _resolved.remove(cacheKey);
      _paths.remove(cacheKey);
      _missedAt.remove(cacheKey);
    }
    var resolution = _resolved[cacheKey];
    if (resolution == null) {
      if (_resolved.length >= 256) {
        final oldest = _resolved.keys.first;
        _resolved.remove(oldest);
        _paths.remove(oldest);
        _missedAt.remove(oldest);
      }
      final pending = _resolve(name, iconSize, pixelSize);
      _resolved[cacheKey] = pending;
      resolution = pending;
      pending.then((path) {
        if (!identical(_resolved[cacheKey], pending)) return;
        if (path == null || path.isEmpty) {
          _missedAt[cacheKey] = DateTime.now();
        } else {
          _paths[cacheKey] = path;
        }
      });
    }
    return FutureBuilder<String?>(
      // Preserve the prior path while the same icon's larger physical-size
      // request resolves. A different app or theme must get fresh state.
      key: ValueKey(iconIdentity),
      future: resolution,
      // A completed Future still starts a newly mounted FutureBuilder in its
      // waiting state. Returning to a page must paint its resolved icon now.
      initialData: _paths[cacheKey],
      builder: (context, snapshot) => _image(context, snapshot.data, pixelSize),
    );
  }
}
