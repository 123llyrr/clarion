// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_svg/flutter_svg.dart';

import '../models.dart';
import '../state/shell_controller.dart';

/// Bar hints use the shell's transparent hover surface, never a Flutter popup.
class ShellBarTooltip extends StatefulWidget {
  const ShellBarTooltip({
    super.key,
    required this.controller,
    required this.message,
    required this.child,
    this.labelSemantics = true,
  });
  final ShellController controller;
  final String message;
  final Widget child;
  final bool labelSemantics;

  @override
  State<ShellBarTooltip> createState() => _ShellBarTooltipState();
}

class _ShellBarTooltipState extends State<ShellBarTooltip> {
  final _owner = Object();
  Timer? _wait;
  bool _hovering = false;

  bool get _blocked =>
      widget.controller.panel != ShellPanel.none ||
      widget.controller.nativeTrayMenuOpen;

  @override
  void initState() {
    super.initState();
    widget.controller.addListener(_controllerChanged);
  }

  void _controllerChanged() {
    if (_blocked) _cancel();
  }

  void _enter(PointerEnterEvent event) {
    if (_blocked) return;
    _hovering = true;
    _wait?.cancel();
    _wait = Timer(const Duration(milliseconds: 400), _show);
  }

  void _show() {
    _wait = null;
    if (!mounted || !_hovering || _blocked || widget.message.trim().isEmpty) {
      return;
    }
    final box = context.findRenderObject();
    if (box is! RenderBox || !box.hasSize) return;
    widget.controller.showBarTooltip(
      _owner,
      widget.message,
      box.localToGlobal(box.size.center(Offset.zero)),
    );
  }

  void _cancel() {
    _hovering = false;
    _wait?.cancel();
    _wait = null;
    widget.controller.hideBarTooltip(_owner);
  }

  @override
  void didUpdateWidget(covariant ShellBarTooltip oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.controller != widget.controller) {
      oldWidget.controller.removeListener(_controllerChanged);
      _wait?.cancel();
      _hovering = false;
      WidgetsBinding.instance.addPostFrameCallback((_) {
        oldWidget.controller.hideBarTooltip(_owner);
      });
      widget.controller.addListener(_controllerChanged);
    } else if (oldWidget.message != widget.message &&
        _hovering &&
        identical(widget.controller.barTooltip?.owner, _owner)) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted && _hovering) _show();
      });
    }
  }

  @override
  void dispose() {
    _wait?.cancel();
    widget.controller.removeListener(_controllerChanged);
    final controller = widget.controller;
    // Disposing a bar item can happen while the controller is rebuilding it.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      controller.hideBarTooltip(_owner);
    });
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => MouseRegion(
    onEnter: _enter,
    onExit: (_) => _cancel(),
    child: Listener(
      onPointerDown: (_) => _cancel(),
      child: Focus(
        canRequestFocus: false,
        skipTraversal: true,
        onKeyEvent: (_, event) {
          if (event is KeyDownEvent) _cancel();
          return KeyEventResult.ignored;
        },
        child: Semantics(
          label: widget.labelSemantics ? widget.message : null,
          child: widget.child,
        ),
      ),
    ),
  );
}

/// Secondary mouse buttons and wheel deltas never trigger primary activation.
class TrayPointerRegion extends StatefulWidget {
  const TrayPointerRegion({
    super.key,
    required this.controller,
    required this.item,
    required this.child,
    this.forwardWheel = true,
  });
  final ShellController controller;
  final TrayItem item;
  final Widget child;
  final bool forwardWheel;

  @override
  State<TrayPointerRegion> createState() => _TrayPointerRegionState();
}

class _TrayPointerRegionState extends State<TrayPointerRegion> {
  double _horizontal = 0;
  double _vertical = 0;

  @override
  void didUpdateWidget(covariant TrayPointerRegion oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.item.id != widget.item.id ||
        oldWidget.controller != widget.controller ||
        oldWidget.forwardWheel != widget.forwardWheel) {
      _horizontal = 0;
      _vertical = 0;
    }
  }

  void _scroll(PointerScrollEvent event) {
    if (!widget.forwardWheel) return;
    final dx = event.scrollDelta.dx.isFinite ? event.scrollDelta.dx : 0.0;
    final dy = event.scrollDelta.dy.isFinite ? event.scrollDelta.dy : 0.0;
    if (dx == 0 && dy == 0) return;
    GestureBinding.instance.pointerSignalResolver.register(event, (_) {
      // Only the winning region consumes the signal. Keep both signed axes:
      // touchpads often send sub-unit deltas, including diagonal movement.
      _horizontal += dx;
      _vertical += dy;
      final horizontal = _wholeUnits(_horizontal);
      final vertical = _wholeUnits(_vertical);
      _horizontal -= horizontal;
      _vertical -= vertical;
      if (_horizontal.abs() < 1e-9) _horizontal = 0;
      if (_vertical.abs() < 1e-9) _vertical = 0;
      if (horizontal != 0) {
        unawaited(
          widget.controller.scrollTrayItem(
            widget.item,
            horizontal,
            orientation: 'horizontal',
          ),
        );
      }
      if (vertical != 0) {
        unawaited(
          widget.controller.scrollTrayItem(
            widget.item,
            vertical,
            orientation: 'vertical',
          ),
        );
      }
    });
  }

  // Compensate only floating-point summation noise at an integer boundary,
  // rather than rounding each small event away from zero.
  int _wholeUnits(double value) =>
      value >= 0 ? (value + 1e-9).floor() : (value - 1e-9).ceil();

  @override
  Widget build(BuildContext context) => Listener(
    onPointerDown: (event) {
      if (event.buttons == kTertiaryButton) {
        unawaited(
          widget.controller.secondaryActivateTrayItem(
            widget.item,
            x: event.position.dx,
            y: event.position.dy,
          ),
        );
      }
    },
    onPointerSignal: (event) {
      if (event is PointerScrollEvent) _scroll(event);
    },
    child: widget.child,
  );
}

class TrayBarButton extends StatefulWidget {
  const TrayBarButton({
    super.key,
    required this.controller,
    required this.item,
  });
  final ShellController controller;
  final TrayItem item;

  @override
  State<TrayBarButton> createState() => _TrayBarButtonState();
}

class _TrayBarButtonState extends State<TrayBarButton> {
  Offset? _pressed;

  @override
  void didUpdateWidget(covariant TrayBarButton oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.controller != widget.controller ||
        oldWidget.item.id != widget.item.id) {
      _pressed = null;
    }
  }

  @override
  Widget build(BuildContext context) {
    final controller = widget.controller;
    final item = widget.item;
    final selected =
        (controller.panel == ShellPanel.trayMenu ||
            controller.nativeTrayMenuOpen) &&
        controller.selectedTrayItem?.id == item.id;
    final colors = Theme.of(context).colorScheme;
    Offset center() {
      final box = context.findRenderObject();
      return box is RenderBox
          ? box.localToGlobal(box.size.center(Offset.zero))
          : Offset.zero;
    }

    void keyboardActivate() {
      _pressed = null;
      final point = center();
      unawaited(
        controller.activateTrayItem(
          item,
          x: point.dx,
          y: point.dy,
          menuButton: 0,
        ),
      );
    }

    return ShellBarTooltip(
      controller: controller,
      message: item.title,
      labelSemantics: false,
      child: SizedBox(
        width: 36,
        height: 36,
        child: CallbackShortcuts(
          bindings: {
            const SingleActivator(
              LogicalKeyboardKey.enter,
              includeRepeats: false,
            ): keyboardActivate,
            const SingleActivator(
              LogicalKeyboardKey.numpadEnter,
              includeRepeats: false,
            ): keyboardActivate,
            const SingleActivator(
              LogicalKeyboardKey.space,
              includeRepeats: false,
            ): keyboardActivate,
            const SingleActivator(LogicalKeyboardKey.contextMenu): () {
              final point = center();
              unawaited(
                controller.openTrayMenu(
                  item,
                  x: point.dx,
                  y: point.dy,
                  button: 0,
                ),
              );
            },
            const SingleActivator(LogicalKeyboardKey.f10, shift: true): () {
              final point = center();
              unawaited(
                controller.openTrayMenu(
                  item,
                  x: point.dx,
                  y: point.dy,
                  button: 0,
                ),
              );
            },
          },
          child: TrayPointerRegion(
            controller: controller,
            item: item,
            child: Semantics(
              label: item.title,
              button: true,
              selected: selected,
              value: switch (item.status) {
                TrayStatus.needsAttention => '需要关注',
                TrayStatus.passive => '未活动',
                TrayStatus.active => '',
              },
              child: Material(
                color: selected
                    ? colors.secondaryContainer
                    : Colors.transparent,
                borderRadius: BorderRadius.circular(18),
                child: InkWell(
                  key: ValueKey('tray-${item.id}'),
                  customBorder: const CircleBorder(),
                  onTapDown: (details) => _pressed = details.globalPosition,
                  onTapCancel: () => _pressed = null,
                  onTap: () {
                    final menuButton = _pressed == null ? 0 : 1;
                    final point = _pressed ?? center();
                    _pressed = null;
                    unawaited(
                      controller.activateTrayItem(
                        item,
                        x: point.dx,
                        y: point.dy,
                        menuButton: menuButton,
                      ),
                    );
                  },
                  onSecondaryTapUp: (details) => controller.openTrayMenu(
                    item,
                    x: details.globalPosition.dx,
                    y: details.globalPosition.dy,
                    button: 3,
                  ),
                  onLongPress: () {
                    final point = _pressed ?? center();
                    _pressed = null;
                    unawaited(
                      controller.openTrayMenu(
                        item,
                        x: point.dx,
                        y: point.dy,
                        button: 1,
                      ),
                    );
                  },
                  child: Center(
                    child: Opacity(
                      opacity: item.status == TrayStatus.passive ? 0.5 : 1,
                      child: Badge(
                        isLabelVisible:
                            item.status == TrayStatus.needsAttention,
                        smallSize: 8,
                        child: ExcludeSemantics(
                          child: TrayGlyph(item: item, size: 20),
                        ),
                      ),
                    ),
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class TrayGlyph extends StatefulWidget {
  const TrayGlyph({super.key, required this.item, this.size = 20});
  final TrayItem item;
  final double size;

  @override
  State<TrayGlyph> createState() => _TrayGlyphState();
}

class _TrayGlyphState extends State<TrayGlyph> {
  static const _channel = MethodChannel('clarion.shell/window');
  static final _resolved = <String, Future<String?>>{};
  ui.Image? _pixmap;
  int _decodeGeneration = 0;
  late Future<String?> _pathFuture;
  bool _pathMissing = false;
  Brightness? _brightness;
  File? _svgFile;

  @override
  void initState() {
    super.initState();
    _refreshPath();
    _decodePixmap();
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final brightness = Theme.of(context).brightness;
    if (_brightness != null && brightness != _brightness) {
      _resolved.clear();
      _refreshPath();
    }
    _brightness = brightness;
  }

  @override
  void didUpdateWidget(covariant TrayGlyph oldWidget) {
    super.didUpdateWidget(oldWidget);
    final previous = oldWidget.item.iconPixmap;
    final next = widget.item.iconPixmap;
    if (previous?.width != next?.width ||
        previous?.height != next?.height ||
        previous?.rgba != next?.rgba) {
      _decodePixmap();
    }
    final iconChanged =
        oldWidget.item.iconName != widget.item.iconName ||
        oldWidget.item.iconThemePath != widget.item.iconThemePath ||
        oldWidget.item.iconRevision != widget.item.iconRevision ||
        oldWidget.size != widget.size;
    // Exporters create a new TrayItem for title, status and menu changes. Keep
    // the already resolved icon so those updates never paint a waiting fallback.
    // A failed lookup still retries on an exporter update: its icon may have
    // appeared on disk since the last lookup.
    if (iconChanged || (_pathMissing && oldWidget.item != widget.item)) {
      _refreshPath();
    }
  }

  void _refreshPath() {
    _pathMissing = false;
    // SvgFileLoader keys the parsed SVG by File identity. Keep one identity
    // across metadata rebuilds; a real icon event can rewrite this same path.
    _svgFile = null;
    final future = _path(widget.item);
    _pathFuture = future;
    unawaited(
      future.then((path) {
        if (mounted && identical(_pathFuture, future)) {
          _pathMissing = path == null || path.isEmpty;
        }
      }),
    );
  }

  @override
  void dispose() {
    _decodeGeneration++;
    _pixmap?.dispose();
    super.dispose();
  }

  void _decodePixmap() {
    final generation = ++_decodeGeneration;
    final pixmap = widget.item.iconPixmap;
    _pixmap?.dispose();
    _pixmap = null;
    if (pixmap == null ||
        pixmap.width <= 0 ||
        pixmap.height <= 0 ||
        pixmap.width > 1024 ||
        pixmap.height > 1024 ||
        pixmap.rgba.length != pixmap.width * pixmap.height * 4) {
      return;
    }
    ui.decodeImageFromPixels(
      pixmap.rgba,
      pixmap.width,
      pixmap.height,
      ui.PixelFormat.rgba8888,
      (image) {
        if (!mounted || generation != _decodeGeneration) {
          image.dispose();
          return;
        }
        setState(() {
          _pixmap?.dispose();
          _pixmap = image;
        });
      },
    );
  }

  Widget _fallback(BuildContext context) => Icon(
    Icons.widgets_outlined,
    size: widget.size,
    color: Theme.of(context).colorScheme.onSurfaceVariant,
  );

  Widget _file(BuildContext context, String path) {
    final resolution = _pathFuture;
    Widget failed() {
      // A nonempty path can still refer to a file not yet written or a broken
      // image. Preserve the exporter-update retry for those rendering failures,
      // without letting an obsolete decoder invalidate a newer resolution.
      if (mounted && identical(_pathFuture, resolution)) _pathMissing = true;
      return _fallback(context);
    }

    if (path.toLowerCase().endsWith('.svg')) {
      if (_svgFile?.path != path) _svgFile = File(path);
      return SvgPicture.file(
        _svgFile!,
        width: widget.size,
        height: widget.size,
        fit: BoxFit.contain,
        placeholderBuilder: _fallback,
        errorBuilder: (_, _, _) => failed(),
      );
    }
    return Image.file(
      File(path),
      width: widget.size,
      height: widget.size,
      fit: BoxFit.contain,
      cacheWidth: (widget.size * MediaQuery.devicePixelRatioOf(context)).ceil(),
      errorBuilder: (_, _, _) => failed(),
    );
  }

  Future<String?> _resolve(String name, String themePath, double size) {
    final key = '$name\u0000$themePath\u0000$size';
    final cached = _resolved[key];
    if (cached != null) return cached;
    if (_resolved.length >= 128) _resolved.remove(_resolved.keys.first);
    final future = () async {
      try {
        return await _channel.invokeMethod<String>(
          'appIcon',
          themePath.isEmpty
              ? name
              : {'name': name, 'themePath': themePath, 'size': size.round()},
        );
      } on PlatformException {
        return null;
      } on MissingPluginException {
        return null;
      }
    }();
    _resolved[key] = future;
    unawaited(
      future.then((path) {
        // Missing icons can appear after a theme change or an application update.
        if ((path == null || path.isEmpty) &&
            identical(_resolved[key], future)) {
          _resolved.remove(key);
        }
      }),
    );
    return future;
  }

  Future<String?> _path(TrayItem item) async {
    final name = item.iconName;
    final size = widget.size;
    if (name.contains('/') || name.contains(Platform.pathSeparator)) {
      return name;
    }
    if (item.iconThemePath.isNotEmpty && name.isNotEmpty) {
      for (final extension in const ['png', 'svg', 'xpm']) {
        final candidate = File('${item.iconThemePath}/$name.$extension');
        try {
          if (await candidate.exists()) return candidate.path;
        } on FileSystemException {
          /* An exporter may remove its temporary icon directory at any time. */
        }
      }
    }
    return name.isEmpty ? null : _resolve(name, item.iconThemePath, size);
  }

  @override
  Widget build(BuildContext context) {
    if (_pixmap != null) {
      return RawImage(
        image: _pixmap,
        width: widget.size,
        height: widget.size,
        fit: BoxFit.contain,
        filterQuality: FilterQuality.medium,
      );
    }
    return FutureBuilder<String?>(
      future: _pathFuture,
      builder: (context, snapshot) {
        // FutureBuilder can retain the old data while the new icon resolves.
        final path = snapshot.connectionState == ConnectionState.done
            ? snapshot.data
            : null;
        if (path == null || path.isEmpty) return _fallback(context);
        return _file(context, path);
      },
    );
  }
}
