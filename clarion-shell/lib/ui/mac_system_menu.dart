// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../models.dart';
import '../services/session_service.dart';
import '../services/system_information_service.dart';
import '../state/shell_controller.dart';
import 'mac_about_dialog.dart';
import 'mac_force_quit_dialog.dart';

/// Catalina's compact system menu. Session actions use host capabilities;
/// information describes the actual host rather than an invented Mac model.
class MacSystemMenu extends StatefulWidget {
  const MacSystemMenu({
    super.key,
    required this.controller,
    this.sessionService,
    this.informationService,
    this.userName,
    this.preview = false,
  });
  final ShellController controller;
  final SessionService? sessionService;
  final SystemInformationService? informationService;
  final String? userName;
  final bool preview;

  @override
  State<MacSystemMenu> createState() => _MacSystemMenuState();
}

class _MacSystemMenuState extends State<MacSystemMenu> {
  late final SessionService _session;
  late final SystemInformationService _information;
  final _recentMenu = MenuController();
  final _recentFocus = FocusNode(debugLabel: 'Recent items');
  final _firstRecentFocus = FocusNode(debugLabel: 'First recent item');
  final _focusScope = FocusScopeNode(
    debugLabel: 'Mac system menu',
    traversalEdgeBehavior: TraversalEdgeBehavior.closedLoop,
  );
  Set<SessionAction> _available = const {};
  bool _busy = false;
  String? _error;
  Route<dynamic>? _dialog;

  @override
  void initState() {
    super.initState();
    _session =
        widget.sessionService ??
        SessionService(demo: widget.preview || widget.controller.demo);
    _information = widget.informationService ?? SystemInformationService();
    unawaited(_readCapabilities());
  }

  Future<void> _readCapabilities() async {
    try {
      final result = await _session.capabilities();
      if (mounted) setState(() => _available = result);
    } on Object {
      if (mounted) setState(() => _error = '无法读取系统操作权限');
    }
  }

  @override
  void dispose() {
    final dialog = _dialog;
    final navigator = dialog?.navigator;
    // A root Navigator outlives this panel. Remove only the route we own if
    // an IPC command or another panel change disposes the menu underneath it.
    if (dialog != null && navigator != null) {
      scheduleMicrotask(() {
        if (navigator.mounted && dialog.isActive) navigator.removeRoute(dialog);
      });
    }
    if (widget.sessionService == null) _session.dispose();
    _focusScope.dispose();
    _recentFocus.dispose();
    _firstRecentFocus.dispose();
    super.dispose();
  }

  Future<T?> _showDialog<T>(WidgetBuilder builder) async {
    final navigator = Navigator.of(context, rootNavigator: true);
    final route = DialogRoute<T>(
      context: context,
      requestFocus: true,
      themes: InheritedTheme.capture(from: context, to: navigator.context),
      builder: builder,
    );
    _dialog = route;
    try {
      return await navigator.push(route);
    } finally {
      if (identical(_dialog, route)) _dialog = null;
    }
  }

  Future<void> _about() async {
    if (_busy) return;
    setState(() => _busy = true);
    final information = _information.read();
    try {
      await _showDialog<void>(
        (context) => MacAboutDialog(
          information: information,
          controller: widget.controller,
        ),
      );
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _perform(SessionAction action, String label) async {
    if (_busy) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    final controller = widget.controller;
    final revision = controller.panelRevision;
    try {
      if (action == SessionAction.restart ||
          action == SessionAction.shutDown ||
          action == SessionAction.logOut) {
        final accepted = await _showDialog<bool>(
          (context) => CallbackShortcuts(
            bindings: {
              const SingleActivator(LogicalKeyboardKey.escape): () =>
                  Navigator.pop(context, false),
            },
            child: AlertDialog(
              scrollable: true,
              title: Text('确定要$label吗？'),
              content: const Text('请先保存所有已打开的文稿。'),
              actions: [
                TextButton(
                  autofocus: true,
                  onPressed: () => Navigator.pop(context, false),
                  child: const Text('取消'),
                ),
                FilledButton(
                  key: const ValueKey('system-menu-confirm'),
                  onPressed: () => Navigator.pop(context, true),
                  child: Text(label),
                ),
              ],
            ),
          ),
        );
        if (!mounted || accepted != true) return;
      }
      if (!mounted ||
          controller.panel != ShellPanel.systemMenu ||
          controller.panelRevision != revision) {
        return;
      }
      await _session.perform(action);
      if (mounted) controller.closePanelAtRevision(revision);
    } on TimeoutException {
      if (mounted) {
        setState(() => _error = '未收到系统操作结果，请检查当前状态');
      }
    } on Object catch (error) {
      if (mounted) setState(() => _error = '操作未完成：$error');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  void _openRecentWithKeyboard() {
    if (_busy) return;
    _recentMenu.open();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted && _recentMenu.isOpen && _firstRecentFocus.context != null) {
        _firstRecentFocus.requestFocus();
      }
    });
  }

  Widget _recentKeys(Widget child) => CallbackShortcuts(
    bindings: {
      const SingleActivator(LogicalKeyboardKey.arrowLeft):
          _closeRecentWithKeyboard,
      const SingleActivator(LogicalKeyboardKey.escape):
          _closeRecentWithKeyboard,
    },
    child: child,
  );

  void _closeRecentWithKeyboard() {
    _recentMenu.close();
    _recentFocus.requestFocus();
  }

  @override
  Widget build(BuildContext context) {
    final controller = widget.controller;
    final apps = {for (final app in controller.apps) app.id: app};
    final recent = [
      for (final id in controller.recentAppIds.take(10)) ?apps[id],
    ];
    final store = [
      'org.gnome.Software.desktop',
      'org.kde.discover.desktop',
    ].map((id) => apps[id]).whereType<DesktopApp>().firstOrNull;
    final name = widget.userName ?? Platform.environment['USER'] ?? '';
    return FocusTraversalGroup(
      child: Shortcuts(
        shortcuts: const {
          SingleActivator(LogicalKeyboardKey.arrowDown): NextFocusIntent(),
          SingleActivator(LogicalKeyboardKey.arrowUp): PreviousFocusIntent(),
        },
        child: FocusScope(
          node: _focusScope,
          autofocus: true,
          child: SingleChildScrollView(
            padding: const EdgeInsets.symmetric(vertical: 5),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                _row('about', '关于本机', _about),
                const _Separator(),
                _row('settings', '系统偏好设置…', controller.openSettings),
                if (store != null)
                  _row(
                    'store',
                    'App Store…',
                    () => controller.focusOrLaunchApp(store.id),
                  ),
                const _Separator(),
                MenuAnchor(
                  controller: _recentMenu,
                  childFocusNode: _recentFocus,
                  alignmentOffset: const Offset(272, -26),
                  style: MenuStyle(
                    padding: const WidgetStatePropertyAll(
                      EdgeInsets.symmetric(vertical: 5),
                    ),
                    backgroundColor: WidgetStatePropertyAll(
                      Theme.of(context).brightness == Brightness.dark
                          ? const Color(0xff363636)
                          : const Color(0xffececec),
                    ),
                    minimumSize: const WidgetStatePropertyAll(Size(220, 0)),
                    shape: const WidgetStatePropertyAll(
                      RoundedRectangleBorder(
                        borderRadius: BorderRadius.all(Radius.circular(6)),
                      ),
                    ),
                  ),
                  menuChildren: [
                    if (recent.isEmpty)
                      const MenuItemButton(
                        onPressed: null,
                        child: Text('无最近使用的应用'),
                      ),
                    for (final app in recent)
                      _recentKeys(
                        MenuItemButton(
                          key: ValueKey('system-recent-${app.id}'),
                          focusNode: identical(app, recent.first)
                              ? _firstRecentFocus
                              : null,
                          onPressed: _busy
                              ? null
                              : () => controller.focusOrLaunchApp(app.id),
                          child: Text(
                            app.name,
                            style: const TextStyle(fontSize: 13),
                          ),
                        ),
                      ),
                    const Divider(height: 9),
                    _recentKeys(
                      MenuItemButton(
                        key: const ValueKey('system-recent-clear'),
                        onPressed: controller.recentAppIds.isEmpty || _busy
                            ? null
                            : () {
                                controller.setRecentAppIds(const []);
                                _recentMenu.close();
                                controller.closePanel();
                              },
                        child: const Text(
                          '清除菜单',
                          style: TextStyle(fontSize: 13),
                        ),
                      ),
                    ),
                  ],
                  builder: (context, menu, child) => CallbackShortcuts(
                    bindings: {
                      const SingleActivator(LogicalKeyboardKey.arrowRight):
                          _openRecentWithKeyboard,
                    },
                    child: _MenuRow(
                      key: const ValueKey('system-menu-recent'),
                      focusNode: _recentFocus,
                      label: '最近使用的项目',
                      trailing: '›',
                      onPressed: _busy
                          ? null
                          : () => menu.isOpen
                                ? menu.close()
                                : _openRecentWithKeyboard(),
                      onHover: () {
                        if (!_busy && !menu.isOpen) menu.open();
                      },
                    ),
                  ),
                ),
                _MenuRow(
                  key: const ValueKey('system-menu-force-quit'),
                  label: '强制退出…',
                  trailing: '⌥⌘⎋',
                  onPressed: () {
                    widget.controller.closePanel();
                    showForceQuitDialog(
                      context: context,
                      controller: widget.controller,
                      useRootNavigator: true,
                    );
                  },
                ),
                const _Separator(),
                _sessionRow(SessionAction.sleep, 'sleep', '睡眠'),
                _sessionRow(SessionAction.restart, 'restart', '重新启动…'),
                _sessionRow(SessionAction.shutDown, 'shutdown', '关机…'),
                const _Separator(),
                _sessionRow(SessionAction.lock, 'lock', '锁定屏幕'),
                _sessionRow(
                  SessionAction.logOut,
                  'logout',
                  '退出登录${name.isEmpty ? '' : '“$name”'}…',
                ),
                if (_error != null || controller.error != null)
                  Padding(
                    padding: const EdgeInsets.fromLTRB(20, 6, 12, 4),
                    child: Text(
                      _error ?? controller.error!,
                      style: TextStyle(
                        fontSize: 12,
                        color: Theme.of(context).colorScheme.error,
                      ),
                    ),
                  ),
              ],
            ),
          ),
        ),
      ),
    );
  }

  Widget _row(String key, String label, VoidCallback onPressed) => _MenuRow(
    key: ValueKey('system-menu-$key'),
    label: label,
    onPressed: _busy ? null : onPressed,
    onHover: _recentMenu.close,
  );

  Widget _sessionRow(SessionAction action, String key, String label) =>
      _MenuRow(
        key: ValueKey('system-menu-$key'),
        label: label,
        onPressed: !_busy && _available.contains(action)
            ? () => _perform(action, label.replaceAll('…', ''))
            : null,
        onHover: _recentMenu.close,
      );
}

class _Separator extends StatelessWidget {
  const _Separator();
  @override
  Widget build(BuildContext context) => const Padding(
    padding: EdgeInsets.symmetric(horizontal: 10, vertical: 4),
    child: Divider(height: 1, thickness: 0.5),
  );
}

class _MenuRow extends StatefulWidget {
  const _MenuRow({
    super.key,
    required this.label,
    this.trailing,
    this.onPressed,
    this.onHover,
    this.focusNode,
  });
  final String label;
  final String? trailing;
  final VoidCallback? onPressed;
  final VoidCallback? onHover;
  final FocusNode? focusNode;
  @override
  State<_MenuRow> createState() => _MenuRowState();
}

class _MenuRowState extends State<_MenuRow> {
  bool _hovered = false;
  @override
  Widget build(BuildContext context) {
    final enabled = widget.onPressed != null;
    final dark = Theme.of(context).brightness == Brightness.dark;
    final highlighted = enabled && _hovered;
    final color = !enabled
        ? (dark ? Colors.white38 : Colors.black38)
        : highlighted
        ? Colors.white
        : (dark ? Colors.white : Colors.black87);
    return MouseRegion(
      onEnter: (_) {
        setState(() => _hovered = true);
        widget.onHover?.call();
      },
      onExit: (_) => setState(() => _hovered = false),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 5),
        child: SizedBox(
          width: double.infinity,
          height: 26,
          child: TextButton(
            focusNode: widget.focusNode,
            onPressed: widget.onPressed,
            style:
                TextButton.styleFrom(
                  foregroundColor: color,
                  disabledForegroundColor: color,
                  backgroundColor: highlighted
                      ? const Color(0xff007aff)
                      : Colors.transparent,
                  overlayColor: Colors.transparent,
                  padding: const EdgeInsets.fromLTRB(15, 0, 11, 0),
                  minimumSize: Size.zero,
                  shape: RoundedRectangleBorder(
                    borderRadius: BorderRadius.circular(4),
                  ),
                  textStyle: const TextStyle(
                    fontFamily: 'ClarionSystem',
                    fontFamilyFallback: ['ClarionChinese'],
                    fontSize: 13,
                    fontWeight: FontWeight.w400,
                    height: 1.1,
                  ),
                ).copyWith(
                  backgroundColor: WidgetStateProperty.resolveWith(
                    (states) =>
                        enabled &&
                            (highlighted ||
                                states.contains(WidgetState.focused))
                        ? const Color(0xff007aff)
                        : Colors.transparent,
                  ),
                  foregroundColor: WidgetStateProperty.resolveWith(
                    (states) =>
                        enabled &&
                            (highlighted ||
                                states.contains(WidgetState.focused))
                        ? Colors.white
                        : color,
                  ),
                ),
            child: Row(
              children: [
                Expanded(
                  child: Text(
                    widget.label,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                  ),
                ),
                if (widget.trailing != null) Text(widget.trailing!),
              ],
            ),
          ),
        ),
      ),
    );
  }
}
