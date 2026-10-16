// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'package:flutter/foundation.dart' show listEquals;
import 'package:flutter/material.dart';
import '../app_menu_models.dart';
import '../services/global_app_menu_service.dart';

/// Real exported application entries. The host expands its native surface when
/// [onOpenChanged] is true, and provides its existing menu when none are exported.
class GlobalAppMenuBar extends StatefulWidget {
  const GlobalAppMenuBar({
    super.key,
    required this.service,
    this.fallbackMenus,
    this.onOpenChanged,
    this.controller,
    this.compact = false,
    this.barHeight = 22,
    this.barWidth,
  });
  final GlobalAppMenuService service;
  final List<GlobalMenuItem>? fallbackMenus;
  final ValueChanged<bool>? onOpenChanged;
  final MenuController? controller;
  final bool compact;
  final double barHeight;
  // A centered macOS strip retains its logical width while its native surface
  // expands to the whole output for the popup overlay.
  final double? barWidth;
  @override
  State<GlobalAppMenuBar> createState() => _GlobalAppMenuBarState();
}

class _GlobalAppMenuBarState extends State<GlobalAppMenuBar> {
  static const _compactGroup = 'clarion-material-app-menu';
  late MenuController _controller;
  int _revision = 0;
  final _open = <String>{};
  bool _reported = false;
  int _interactionRevision = 0;
  Object? _activationToken;
  @override
  void initState() {
    super.initState();
    _controller = widget.controller ?? MenuController();
    _revision = widget.service.revision;
    widget.service.addListener(_changed);
  }

  void _changed() {
    if (!mounted) return;
    if (!widget.service.available || _revision != widget.service.revision) {
      _interactionRevision++;
      _activationToken = null;
      _revision = widget.service.revision;
      _controller.close();
      _open.clear();
      _report();
    }
    setState(() {});
  }

  void _report() {
    final visible = _open.isNotEmpty;
    if (visible && !_reported) {
      _reported = true;
      widget.onOpenChanged?.call(true);
      return;
    }
    // A closes while B opens when tracking across the bar. Coalesce closing
    // callbacks so the native modal surface is not collapsed between them.
    scheduleMicrotask(() {
      if (!mounted) return;
      final nowVisible = _open.isNotEmpty;
      if (_reported != nowVisible) {
        _reported = nowVisible;
        widget.onOpenChanged?.call(nowVisible);
      }
    });
  }

  void _submenu(GlobalMenuItem item, bool opened) {
    if (opened ? _open.add(item.id) : _open.remove(item.id)) {
      _interactionRevision++;
    }
    _report();
    unawaited(widget.service.submenu(item, opened: opened));
  }

  void _compactOpened(bool opened) {
    if (opened ? _open.add(_compactGroup) : _open.remove(_compactGroup)) {
      _interactionRevision++;
    }
    _report();
  }

  Future<void> _activate(GlobalMenuItem item) async {
    if (_activationToken != null) return;
    final token = _activationToken = Object();
    final interaction = _interactionRevision;
    final service = widget.service;
    final controller = _controller;
    try {
      // MenuItemButton normally closes before its post-frame onPressed callback.
      // Keep the native modal focus/target until the real action is dispatched.
      if (item.onAction != null) {
        try {
          item.onAction!();
        } on Object catch (e) {
          debugPrint('Error executing menu action: $e');
        }
      } else {
        await service.activate(item);
      }
    } on Object catch (e) {
      debugPrint('Error activating menu item: $e');
    } finally {
      if (identical(_activationToken, token)) {
        _activationToken = null;
        if (mounted &&
            _interactionRevision == interaction &&
            widget.service == service &&
            _controller == controller) {
          controller.close();
        }
      }
    }
  }

  @override
  void didUpdateWidget(covariant GlobalAppMenuBar oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.service != widget.service) {
      _interactionRevision++;
      _activationToken = null;
      oldWidget.service.removeListener(_changed);
      widget.service.addListener(_changed);
      _revision = widget.service.revision;
      _controller.close();
      _open.clear();
      _report();
    }
    if (oldWidget.controller != widget.controller) {
      _interactionRevision++;
      _activationToken = null;
      _controller.close();
      _controller = widget.controller ?? MenuController();
      _open.clear();
      _report();
    }
    if (oldWidget.compact != widget.compact ||
        oldWidget.barHeight != widget.barHeight ||
        !listEquals(oldWidget.fallbackMenus, widget.fallbackMenus)) {
      _interactionRevision++;
      _activationToken = null;
      if (_open.isEmpty) {
        _controller.close();
      }
      _report();
    }
  }

  @override
  void dispose() {
    widget.service.removeListener(_changed);
    _controller.close();
    if (_reported) {
      final callback = widget.onOpenChanged;
      scheduleMicrotask(() => callback?.call(false));
    }
    super.dispose();
  }

  Widget _item(GlobalMenuItem item, {bool top = false}) {
    if (item.separator) return const Divider(height: 7);
    final checked = item.checked;
    final leading = top || checked == null
        ? null
        : Icon(
            checked
                ? item.radio
                      ? Icons.radio_button_checked
                      : Icons.check
                : null,
            size: 14,
          );
    final style = ButtonStyle(
      padding: WidgetStatePropertyAll(
        EdgeInsets.symmetric(horizontal: top ? 9 : 12),
      ),
      minimumSize: WidgetStatePropertyAll(Size(0, top ? widget.barHeight : 28)),
      maximumSize: top
          ? WidgetStatePropertyAll(Size(double.infinity, widget.barHeight))
          : null,
      tapTargetSize: MaterialTapTargetSize.shrinkWrap,
      // Top-level entries fill the actual menu strip. Compact density would
      // subtract eight pixels and detach the popup from a taller bar's edge.
      visualDensity: top ? VisualDensity.standard : VisualDensity.compact,
      textStyle: const WidgetStatePropertyAll(TextStyle(fontSize: 13)),
    );
    if (item.isSubmenu) {
      return SubmenuButton(
        key: ValueKey('global-menu-${item.id}'),
        style: style,
        leadingIcon: leading,
        onOpen: () => _submenu(item, true),
        onClose: () => _submenu(item, false),
        menuChildren: !item.enabled
            ? const []
            : item.children.isEmpty
            ? const [MenuItemButton(child: Text('没有菜单项'))]
            : [
                for (final child in item.children)
                  if (child.visible) _item(child),
              ],
        child: Text(item.label, maxLines: 1, overflow: TextOverflow.ellipsis),
      );
    }
    return MenuItemButton(
      key: ValueKey('global-menu-${item.id}'),
      style: style,
      closeOnActivate: false,
      leadingIcon: leading,
      trailingIcon: top || item.shortcut.isEmpty
          ? null
          : Text(item.shortcut, style: Theme.of(context).textTheme.labelSmall),
      onPressed: item.enabled ? () => unawaited(_activate(item)) : null,
      child: Text(item.label, maxLines: 1, overflow: TextOverflow.ellipsis),
    );
  }

  Widget _compactEntry() => SubmenuButton(
    key: const ValueKey('material-app-menu-entry'),
    style: ButtonStyle(
      padding: const WidgetStatePropertyAll(EdgeInsets.zero),
      fixedSize: WidgetStatePropertyAll(Size(36, widget.barHeight)),
      minimumSize: WidgetStatePropertyAll(Size(36, widget.barHeight)),
      maximumSize: WidgetStatePropertyAll(Size(36, widget.barHeight)),
      tapTargetSize: MaterialTapTargetSize.shrinkWrap,
      visualDensity: VisualDensity.standard,
    ),
    onOpen: () => _compactOpened(true),
    onClose: () => _compactOpened(false),
    // The wrapper only groups the application's actual exported entries. It
    // has no synthetic action and sends no submenu request to the exporter.
    menuChildren: [
      for (final item in widget.service.menus)
        if (item.visible) _item(item),
    ],
    child: Semantics(
      label: '应用菜单',
      button: true,
      child: const Icon(Icons.menu_open_rounded, size: 20),
    ),
  );

  List<GlobalMenuItem> get _effectiveMenus => widget.service.available
      ? widget.service.menus
      : (widget.fallbackMenus ?? const []);

  @override
  Widget build(BuildContext context) => _effectiveMenus.isNotEmpty
      ? MediaQuery(
          // RawMenuAnchor closes an open menu when MediaQuery.size changes.
          // Opening this fixed-height bar expands the native surface vertically;
          // that change must preserve its popup. Keep the actual width so a
          // horizontal resize still closes it. OverlayPortal supplies the real
          // overlay bounds independently for popup layout and hit testing.
          data: MediaQuery.of(context).copyWith(
            size: Size(
              widget.barWidth ?? MediaQuery.sizeOf(context).width,
              widget.barHeight,
            ),
          ),
          child: SingleChildScrollView(
            scrollDirection: Axis.horizontal,
            child: SizedBox(
              height: widget.barHeight,
              child: MenuBar(
                controller: _controller,
                style: const MenuStyle(
                  padding: WidgetStatePropertyAll(EdgeInsets.zero),
                  backgroundColor: WidgetStatePropertyAll(Colors.transparent),
                  elevation: WidgetStatePropertyAll(0),
                ),
                children: [
                  if (widget.compact)
                    _compactEntry()
                  else
                    for (final item in _effectiveMenus)
                      if (item.visible && !item.separator)
                        _item(item, top: true),
                ],
              ),
            ),
          ),
        )
      : const SizedBox.shrink();
}
