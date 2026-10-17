// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import '../models.dart';
import '../state/shell_controller.dart';
import 'controller_selector.dart';

/// Compact application context menu. Actions always refer to real window IDs;
/// closing windows is deliberately named accurately instead of claiming that
/// the compositor can terminate an application's background process.
class MacDockAppMenu extends StatelessWidget {
  const MacDockAppMenu({super.key, required this.controller});
  final ShellController controller;

  static double estimatedHeight(ShellController controller) {
    final app = controller.selectedDockApp;
    if (app == null) return 38;
    final hasOptions = app.canPin || controller.demo;
    final rows =
        app.windows.length +
        (controller.canMinimize ? app.windows.length : 0) +
        (app.isRunning || app.canPin || controller.demo ? 1 : 0) +
        (hasOptions ? 3 : 0) +
        (app.isRunning ? 3 : 0);
    return 12 +
        26 * rows +
        (app.windows.isEmpty ? 0 : 9) +
        (hasOptions ? 31 : 0) +
        (app.isRunning ? 9 : 0);
  }

  @override
  Widget build(BuildContext context) => ControllerSelector(
    controller: controller,
    select: () => (
      controller.desktopRevision,
      controller.apps,
      controller.pinnedAppIds,
      controller.selectedDockAppId,
      controller.canMinimize,
      controller.error,
      controller.busy.contains('close-app-windows'),
      controller.busy.contains('force-quit-app'),
    ),
    builder: (context, _) => _buildMenu(context),
  );

  Widget _buildMenu(BuildContext context) {
    final app = controller.selectedDockApp;
    if (app == null) return const SizedBox(height: 38);
    return Shortcuts(
      shortcuts: const {
        SingleActivator(LogicalKeyboardKey.arrowDown): NextFocusIntent(),
        SingleActivator(LogicalKeyboardKey.arrowUp): PreviousFocusIntent(),
      },
      child: SingleChildScrollView(
        padding: const EdgeInsets.symmetric(vertical: 6),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            for (final window in app.windows)
              _MenuRow(
                actionKey: 'dock-window-${window.id}',
                label: window.title.isEmpty ? app.name : window.title,
                indicator: window.isFocused
                    ? Icons.check
                    : window.isMinimized
                    ? Icons.diamond_outlined
                    : null,
                autofocus: window == app.windows.first,
                onPressed: () => controller.focusWindow(window.id),
              ),
            if (app.windows.isNotEmpty) ...[
              if (controller.canMinimize)
                for (final window in app.windows)
                  _MenuRow(
                    actionKey: 'dock-minimize-${window.id}',
                    label:
                        '${window.isMinimized ? '还原' : '最小化'} · ${window.title.isEmpty ? app.name : window.title}',
                    onPressed: () => window.isMinimized
                        ? controller.focusWindow(window.id)
                        : controller.minimizeWindow(window.id),
                  ),
              const _Separator(),
            ],
            if (app.isRunning || app.canPin || controller.demo)
              _MenuRow(
                actionKey: 'dock-menu-open',
                label: app.isRunning ? '切换到应用' : '打开',
                autofocus: app.windows.isEmpty,
                onPressed: () => controller.focusOrLaunchApp(app.id),
              ),
            if (app.canPin || controller.demo) ...[
              const _Separator(),
              const _MenuSectionHeader(label: '选项'),
              _MenuRow(
                actionKey: 'dock-menu-pin',
                label: app.isPinned ? '从程序坞中移除' : '在程序坞中保留',
                indicator: app.isPinned ? Icons.check : null,
                onPressed: () {
                  controller.togglePinnedApp(app.id);
                  controller.closePanel();
                },
              ),
              _MenuRow(
                actionKey: 'dock-menu-reveal',
                label: '在Home中显示',
                onPressed: () => controller.revealDockApp(app.id),
              ),
              _LoginItemRow(
                key: ValueKey('login-item-${app.id}'),
                app: app.app,
                controller: controller,
              ),
            ],
            if (app.isRunning) ...[
              const _Separator(),
              _MenuRow(
                actionKey: 'dock-menu-hide',
                label:
                    app.windows.isNotEmpty &&
                        app.windows.every((w) => w.isMinimized)
                    ? '显示'
                    : '隐藏',
                onPressed: () {
                  controller.closePanel();
                  if (app.windows.isNotEmpty &&
                      app.windows.every((w) => w.isMinimized)) {
                    controller.focusOrLaunchApp(app.id);
                  } else {
                    controller.hideApp(app.id);
                  }
                },
              ),
              _MenuRow(
                actionKey: 'dock-menu-close-windows',
                label: '退出',
                onPressed: controller.busy.contains('close-app-windows')
                    ? null
                    : () => controller.closeAppWindows(app.id),
              ),
              _MenuRow(
                actionKey: 'dock-menu-force-quit',
                label: '强制退出',
                onPressed: controller.busy.contains('force-quit-app')
                    ? null
                    : () => controller.forceQuitApp(app.id),
              ),
            ],
            if (controller.error != null)
              Padding(
                padding: const EdgeInsets.all(8),
                child: Text(
                  controller.error!,
                  style: TextStyle(color: Theme.of(context).colorScheme.error),
                ),
              ),
          ],
        ),
      ),
    );
  }
}

class _LoginItemRow extends StatefulWidget {
  const _LoginItemRow({super.key, required this.app, required this.controller});
  final DesktopApp app;
  final ShellController controller;

  @override
  State<_LoginItemRow> createState() => _LoginItemRowState();
}

class _LoginItemRowState extends State<_LoginItemRow> {
  bool? _enabled;
  bool _busy = false;
  String? _error;
  int _request = 0;

  @override
  void initState() {
    super.initState();
    _load();
  }

  @override
  void didUpdateWidget(covariant _LoginItemRow oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.app.desktopFile != widget.app.desktopFile ||
        oldWidget.controller != widget.controller) {
      _enabled = null;
      _busy = false;
      _error = null;
      _load();
    }
  }

  Future<void> _load() async {
    final request = ++_request;
    try {
      final enabled = await widget.controller.loginItems.isEnabled(widget.app);
      if (!mounted || request != _request) return;
      setState(() => _enabled = enabled);
    } on Object catch (error) {
      if (!mounted || request != _request) return;
      setState(() => _error = '无法读取登录项：$error');
    }
  }

  Future<void> _toggle() async {
    if (_busy || _enabled == null) return;
    final controller = widget.controller;
    final panelRevision = controller.panelRevision;
    final request = ++_request;
    final enabled = !_enabled!;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await controller.loginItems.setEnabled(widget.app, enabled);
      if (!mounted || request != _request) return;
      setState(() => _enabled = enabled);
      controller.closePanelAtRevision(panelRevision);
    } on Object catch (error) {
      if (mounted && request == _request) {
        setState(() => _error = '无法更新登录项：$error');
      }
    } finally {
      if (mounted && request == _request) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) => Column(
    mainAxisSize: MainAxisSize.min,
    children: [
      _MenuRow(
        actionKey: 'dock-menu-login',
        label: '登录时打开',
        indicator: _enabled == true ? Icons.check : null,
        onPressed: _enabled == null || _busy ? null : _toggle,
      ),
      if (_error != null)
        Padding(
          padding: const EdgeInsets.all(8),
          child: Text(
            _error!,
            style: TextStyle(
              fontSize: 12,
              color: Theme.of(context).colorScheme.error,
            ),
          ),
        ),
    ],
  );
}

class _Separator extends StatelessWidget {
  const _Separator();
  @override
  Widget build(BuildContext context) => const Padding(
    padding: EdgeInsets.symmetric(vertical: 4),
    child: Divider(height: 1, thickness: .5),
  );
}

class _MenuSectionHeader extends StatelessWidget {
  const _MenuSectionHeader({required this.label});
  final String label;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.only(left: 28, top: 4, bottom: 2, right: 12),
    child: Align(
      alignment: Alignment.centerLeft,
      child: Text(
        label,
        style: TextStyle(
          fontSize: 11,
          fontWeight: FontWeight.w600,
          color: Theme.of(context).colorScheme.onSurface.withValues(alpha: .5),
        ),
      ),
    ),
  );
}

class _MenuRow extends StatefulWidget {
  const _MenuRow({
    required this.actionKey,
    required this.label,
    this.indicator,
    this.autofocus = false,
    this.onPressed,
  });
  final String actionKey;
  final String label;
  final IconData? indicator;
  final bool autofocus;
  final VoidCallback? onPressed;
  @override
  State<_MenuRow> createState() => _MenuRowState();
}

class _MenuRowState extends State<_MenuRow> {
  bool _hovered = false;
  bool _focused = false;
  @override
  Widget build(BuildContext context) {
    final enabled = widget.onPressed != null;
    final selected = enabled && (_hovered || _focused);
    final foreground = selected
        ? Colors.white
        : enabled
        ? Theme.of(context).colorScheme.onSurface
        : Theme.of(context).disabledColor;
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 4),
      child: Material(
        color: selected ? const Color(0xff007aff) : Colors.transparent,
        borderRadius: BorderRadius.circular(3),
        child: InkWell(
          key: ValueKey(widget.actionKey),
          autofocus: widget.autofocus,
          onTap: widget.onPressed,
          onHover: (v) => setState(() => _hovered = v),
          onFocusChange: (v) => setState(() => _focused = v),
          borderRadius: BorderRadius.circular(3),
          splashFactory: NoSplash.splashFactory,
          overlayColor: const WidgetStatePropertyAll(Colors.transparent),
          child: SizedBox(
            height: 26,
            child: Row(
              children: [
                SizedBox(
                  width: 24,
                  child: widget.indicator == null
                      ? null
                      : Icon(widget.indicator, size: 12, color: foreground),
                ),
                Expanded(
                  child: Text(
                    widget.label,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: 13, color: foreground),
                  ),
                ),
                const SizedBox(width: 12),
              ],
            ),
          ),
        ),
      ),
    );
  }
}
