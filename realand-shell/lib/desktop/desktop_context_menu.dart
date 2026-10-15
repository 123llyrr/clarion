// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/scheduler.dart';
import 'package:flutter/services.dart';

class DesktopContextMenuItem<T> {
  const DesktopContextMenuItem({
    this.key,
    required this.label,
    this.value,
    this.shortcut,
    this.checked = false,
    this.enabled = true,
    this.children = const [],
  }) : isDivider = false;

  const DesktopContextMenuItem.divider({this.key})
    : label = '',
      value = null,
      shortcut = null,
      checked = false,
      enabled = false,
      children = const [],
      isDivider = true;

  final Key? key;
  final String label;
  final T? value;
  final String? shortcut;
  final bool checked;
  final bool enabled;
  final bool isDivider;
  final List<DesktopContextMenuItem<T>> children;
}

/// [position] is local to the nearest Navigator's overlay. All menu columns
/// share one route and one keyboard focus, so no child barrier blocks its parent.
Future<T?> showDesktopContextMenu<T>({
  required BuildContext context,
  required Offset position,
  required List<DesktopContextMenuItem<T>> items,
  bool reduceMotion = false,
}) {
  if (items.isEmpty) return Future<T?>.value();
  final navigator = Navigator.of(context);
  return navigator.push(
    _DesktopMenuRoute<T>(
      position: position,
      items: List.unmodifiable(items),
      themes: InheritedTheme.capture(from: context, to: navigator.context),
      label: MaterialLocalizations.of(context).modalBarrierDismissLabel,
      reduceMotion: reduceMotion || MediaQuery.disableAnimationsOf(context),
    ),
  );
}

class _DesktopMenuRoute<T> extends PopupRoute<T> {
  _DesktopMenuRoute({
    required this.position,
    required this.items,
    required this.themes,
    required this.label,
    required this.reduceMotion,
  });
  final Offset position;
  final List<DesktopContextMenuItem<T>> items;
  final CapturedThemes themes;
  final String label;
  final bool reduceMotion;
  @override
  Color? get barrierColor => null;
  @override
  bool get barrierDismissible => true;
  @override
  String get barrierLabel => label;
  @override
  Duration get transitionDuration =>
      reduceMotion ? Duration.zero : const Duration(milliseconds: 100);
  @override
  Duration get reverseTransitionDuration =>
      reduceMotion ? Duration.zero : const Duration(milliseconds: 80);
  @override
  Widget buildPage(
    BuildContext context,
    Animation<double> animation,
    Animation<double> secondaryAnimation,
  ) => themes.wrap(_DesktopMenu<T>(position: position, items: items));
  @override
  Widget buildTransitions(
    BuildContext context,
    Animation<double> animation,
    Animation<double> secondaryAnimation,
    Widget child,
  ) => FadeTransition(opacity: animation, child: child);
}

class _DesktopMenu<T> extends StatefulWidget {
  const _DesktopMenu({required this.position, required this.items});
  final Offset position;
  final List<DesktopContextMenuItem<T>> items;
  @override
  State<_DesktopMenu<T>> createState() => _DesktopMenuState<T>();
}

class _MenuLevel<T> {
  _MenuLevel(this.items);
  final List<DesktopContextMenuItem<T>> items;
  final scroll = ScrollController();
  int selected = -1;
  int opened = -1;
  double? measuredWidth;
  double get height =>
      10 +
      items.fold<double>(0, (sum, item) => sum + (item.isDivider ? 9 : 26));
  double rowTop(int index) =>
      5 +
      items
          .take(index)
          .fold<double>(0, (sum, item) => sum + (item.isDivider ? 9 : 26));
}

class _DesktopMenuState<T> extends State<_DesktopMenu<T>> {
  final _focus = FocusNode(debugLabel: 'desktop context menu');
  final _levels = <_MenuLevel<T>>[];
  Timer? _hoverTimer;
  (int, int)? _hoverTarget;
  int _active = 0;
  bool _geometryQueued = false;
  bool _closing = false;

  bool get _acceptsInput =>
      mounted && !_closing && ModalRoute.of(context)?.isCurrent == true;

  bool _validRow(int depth, int index) =>
      _acceptsInput &&
      depth >= 0 &&
      depth < _levels.length &&
      index >= 0 &&
      index < _levels[depth].items.length;

  void _dismiss([T? value]) {
    if (!_acceptsInput) return;
    _closing = true;
    _cancelHover();
    Navigator.of(context).pop<T>(value);
  }

  @override
  void initState() {
    super.initState();
    _addLevel(widget.items);
  }

  void _addLevel(List<DesktopContextMenuItem<T>> items) {
    final level = _MenuLevel<T>(items);
    level.scroll.addListener(_scrollChanged);
    _levels.add(level);
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    for (final level in _levels) {
      level.measuredWidth = null;
    }
  }

  @override
  void dispose() {
    _hoverTimer?.cancel();
    for (final level in _levels) {
      level.scroll.dispose();
    }
    _focus.dispose();
    super.dispose();
  }

  void _scrollChanged() {
    if (!mounted || _levels.length == 1) return;
    if (SchedulerBinding.instance.schedulerPhase ==
        SchedulerPhase.persistentCallbacks) {
      if (_geometryQueued) return;
      _geometryQueued = true;
      WidgetsBinding.instance.addPostFrameCallback((_) {
        _geometryQueued = false;
        if (mounted) setState(() {});
      });
    } else {
      setState(() {});
    }
  }

  void _cancelHover() {
    _hoverTimer?.cancel();
    _hoverTimer = null;
    _hoverTarget = null;
  }

  void _truncate(int length) {
    while (_levels.length > length) {
      _levels.removeLast().scroll.dispose();
    }
    _levels.last.opened = -1;
    _active = math.min(_active, length - 1);
  }

  void _hover(int depth, int index) {
    if (!_validRow(depth, index)) return;
    _cancelHover();
    final level = _levels[depth];
    final item = level.items[index];
    setState(() {
      if (level.opened != index) _truncate(depth + 1);
      level.selected = item.enabled ? index : -1;
      _active = depth;
    });
    _focus.requestFocus();
    if (!item.enabled || item.children.isEmpty || level.opened == index) return;
    _hoverTarget = (depth, index);
    _hoverTimer = Timer(const Duration(milliseconds: 180), () {
      if (!mounted || _hoverTarget != (depth, index)) return;
      _open(depth, index, keyboard: false);
    });
  }

  void _open(int depth, int index, {required bool keyboard}) {
    if (!_validRow(depth, index)) return;
    _cancelHover();
    final level = _levels[depth];
    final item = level.items[index];
    if (!item.enabled || item.children.isEmpty) return;
    setState(() {
      _truncate(depth + 1);
      level.selected = index;
      level.opened = index;
      _addLevel(item.children);
      if (keyboard) {
        _active = depth + 1;
        _levels.last.selected = _nextEnabled(_levels.last, -1, 1);
      }
    });
  }

  int _nextEnabled(_MenuLevel<T> level, int from, int direction) {
    for (var step = 1; step <= level.items.length; step++) {
      final index = (from + direction * step) % level.items.length;
      final item = level.items[index];
      if (item.enabled && !item.isDivider) return index;
    }
    return -1;
  }

  void _activate(int depth, int index) {
    if (!_validRow(depth, index)) return;
    final item = _levels[depth].items[index];
    if (!item.enabled || item.isDivider) return;
    if (item.children.isNotEmpty) {
      _open(depth, index, keyboard: true);
    } else {
      _dismiss(item.value);
    }
  }

  void _reveal(int depth, _MenuLevel<T> level) {
    if (!mounted ||
        depth >= _levels.length ||
        !identical(_levels[depth], level) ||
        level.selected < 0) {
      return;
    }
    if (!level.scroll.hasClients) {
      WidgetsBinding.instance.addPostFrameCallback(
        (_) => _reveal(depth, level),
      );
      return;
    }
    final top = level.rowTop(level.selected);
    final position = level.scroll.position;
    final visibleBottom = position.pixels + position.viewportDimension;
    final target = top < position.pixels
        ? top
        : top + 26 > visibleBottom
        ? top + 26 - position.viewportDimension
        : position.pixels;
    level.scroll.jumpTo(target.clamp(0.0, position.maxScrollExtent));
  }

  KeyEventResult _key(FocusNode node, KeyEvent event) {
    if (event is! KeyDownEvent && event is! KeyRepeatEvent) {
      return KeyEventResult.ignored;
    }
    if (!_acceptsInput) return KeyEventResult.handled;
    final key = event.logicalKey;
    if (key == LogicalKeyboardKey.escape) {
      _dismiss();
      return KeyEventResult.handled;
    }
    final level = _levels[_active];
    if (key == LogicalKeyboardKey.arrowLeft) {
      _cancelHover();
      if (_active > 0) setState(() => _truncate(_active));
      return KeyEventResult.handled;
    }
    if (key == LogicalKeyboardKey.arrowRight ||
        key == LogicalKeyboardKey.enter ||
        key == LogicalKeyboardKey.numpadEnter) {
      _cancelHover();
      if (level.selected >= 0) {
        if (key != LogicalKeyboardKey.arrowRight ||
            level.items[level.selected].children.isNotEmpty) {
          _activate(_active, level.selected);
        }
      }
      return KeyEventResult.handled;
    }
    if (key == LogicalKeyboardKey.arrowDown ||
        key == LogicalKeyboardKey.arrowUp ||
        key == LogicalKeyboardKey.home ||
        key == LogicalKeyboardKey.end ||
        key == LogicalKeyboardKey.tab) {
      _cancelHover();
      final backwards =
          key == LogicalKeyboardKey.arrowUp ||
          key == LogicalKeyboardKey.end ||
          (key == LogicalKeyboardKey.tab &&
              HardwareKeyboard.instance.isShiftPressed);
      final from = key == LogicalKeyboardKey.home
          ? -1
          : key == LogicalKeyboardKey.end
          ? 0
          : level.selected < 0 && backwards
          ? 0
          : level.selected;
      setState(() {
        _truncate(_active + 1);
        level.selected = _nextEnabled(level, from, backwards ? -1 : 1);
      });
      _reveal(_active, level);
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  double _width(_MenuLevel<T> level) => level.measuredWidth ??= () {
    var width = 196.0;
    final style = Theme.of(
      context,
    ).textTheme.bodyMedium!.copyWith(fontSize: 13, fontWeight: FontWeight.w400);
    for (final item in level.items.where((item) => !item.isDivider)) {
      final painter = TextPainter(
        text: TextSpan(
          text: '${item.label}${item.shortcut ?? ''}',
          style: style,
        ),
        textDirection: Directionality.of(context),
        textScaler: MediaQuery.textScalerOf(context),
      )..layout();
      width = math.max(
        width,
        painter.width +
            36 +
            (item.shortcut == null ? 0 : 16) +
            (item.children.isEmpty ? 0 : 30),
      );
      painter.dispose();
    }
    return math.min(300.0, width);
  }();

  @override
  Widget build(BuildContext context) => Focus(
    focusNode: _focus,
    autofocus: true,
    onKeyEvent: _key,
    child: LayoutBuilder(
      builder: (context, constraints) {
        final marginX = math.min(8.0, constraints.maxWidth / 2);
        final marginY = math.min(8.0, constraints.maxHeight / 2);
        final available = Rect.fromLTRB(
          marginX,
          marginY,
          constraints.maxWidth - marginX,
          constraints.maxHeight - marginY,
        );
        final rects = <Rect>[];
        for (var depth = 0; depth < _levels.length; depth++) {
          final level = _levels[depth];
          final width = math.min(_width(level), available.width);
          final height = math.min(level.height, available.height);
          var left = widget.position.dx;
          var top = widget.position.dy;
          if (depth > 0) {
            final parent = _levels[depth - 1];
            final parentRect = rects.last;
            final towardsLeft =
                depth > 1 && parentRect.left < rects[depth - 2].left;
            left = towardsLeft
                ? parentRect.left - width + 1
                : parentRect.right - 1;
            if (towardsLeft && left < available.left) {
              left = parentRect.right - 1;
            } else if (!towardsLeft && left + width > available.right) {
              left = parentRect.left - width + 1;
            }
            top =
                parentRect.top +
                parent.rowTop(parent.opened) -
                5 -
                (parent.scroll.hasClients ? parent.scroll.offset : 0);
          }
          rects.add(
            Rect.fromLTWH(
              left.clamp(available.left, available.right - width),
              top.clamp(available.top, available.bottom - height),
              width,
              height,
            ),
          );
        }
        return Stack(
          children: [
            for (var depth = 0; depth < _levels.length; depth++)
              Positioned.fromRect(rect: rects[depth], child: _column(depth)),
          ],
        );
      },
    ),
  );

  Widget _column(int depth) {
    final colors = Theme.of(context).colorScheme;
    final level = _levels[depth];
    return Material(
      key: ValueKey('desktop-context-menu-$depth'),
      color: colors.surfaceContainer,
      elevation: 10,
      clipBehavior: Clip.antiAlias,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(6),
        side: BorderSide(color: colors.outlineVariant, width: .5),
      ),
      child: ListView.builder(
        controller: level.scroll,
        primary: false,
        padding: const EdgeInsets.symmetric(vertical: 5),
        physics: const ClampingScrollPhysics(),
        itemCount: level.items.length,
        itemBuilder: (context, index) => _row(depth, index),
      ),
    );
  }

  Widget _row(int depth, int index) {
    final level = _levels[depth];
    final item = level.items[index];
    final colors = Theme.of(context).colorScheme;
    final highlighted = item.enabled && level.selected == index;
    final foreground = !item.enabled
        ? Theme.of(context).disabledColor
        : highlighted
        ? colors.onPrimary
        : colors.onSurface;
    return MouseRegion(
      onEnter: (_) => _hover(depth, index),
      onExit: (_) {
        if (_hoverTarget == (depth, index)) _cancelHover();
      },
      child: item.isDivider
          ? SizedBox(
              key: item.key,
              height: 9,
              child: Divider(
                height: 9,
                thickness: .5,
                indent: 8,
                endIndent: 8,
                color: colors.outlineVariant,
              ),
            )
          : Semantics(
              key: item.key,
              button: true,
              enabled: item.enabled,
              selected: highlighted,
              checked: item.checked ? true : null,
              child: GestureDetector(
                behavior: HitTestBehavior.opaque,
                onTap: item.enabled ? () => _activate(depth, index) : null,
                child: ColoredBox(
                  color: highlighted ? colors.primary : Colors.transparent,
                  child: SizedBox(
                    height: 26,
                    child: LayoutBuilder(
                      builder: (context, constraints) {
                        return Padding(
                          padding: const EdgeInsets.only(left: 4, right: 12),
                          child: DefaultTextStyle(
                            style: Theme.of(context).textTheme.bodyMedium!
                                .copyWith(
                                  fontSize: 13,
                                  fontWeight: FontWeight.w400,
                                  color: foreground,
                                ),
                            child: Row(
                              children: [
                                if (constraints.maxWidth >= 40)
                                  SizedBox(
                                    width: 16,
                                    child: item.checked
                                        ? Icon(
                                            Icons.check,
                                            size: 12,
                                            color: foreground,
                                          )
                                        : null,
                                  ),
                                Expanded(
                                  child: Text(
                                    item.label,
                                    maxLines: 1,
                                    overflow: TextOverflow.ellipsis,
                                  ),
                                ),
                                if (item.shortcut != null &&
                                    constraints.maxWidth >= 160) ...[
                                  const SizedBox(width: 16),
                                  ConstrainedBox(
                                    constraints: BoxConstraints(
                                      maxWidth: math.min(
                                        120.0,
                                        constraints.maxWidth * .45,
                                      ),
                                    ),
                                    child: Text(
                                      item.shortcut!,
                                      maxLines: 1,
                                      overflow: TextOverflow.ellipsis,
                                    ),
                                  ),
                                ],
                                if (item.children.isNotEmpty &&
                                    constraints.maxWidth >= 70) ...[
                                  const SizedBox(width: 16),
                                  Icon(
                                    Icons.chevron_right,
                                    size: 14,
                                    color: foreground,
                                  ),
                                ],
                              ],
                            ),
                          ),
                        );
                      },
                    ),
                  ),
                ),
              ),
            ),
    );
  }
}
