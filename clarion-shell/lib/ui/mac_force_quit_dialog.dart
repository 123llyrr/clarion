// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../mac_app_icons.dart';
import '../models.dart';
import '../state/shell_controller.dart';
import 'app_icon.dart';

/// Shows the authentic macOS Force Quit Applications dialog (⌥⌘⎋).
Future<void> showForceQuitDialog({
  required BuildContext context,
  required ShellController controller,
  bool useRootNavigator = false,
}) => showDialog<void>(
  context: context,
  useRootNavigator: useRootNavigator,
  builder: (context) => MacForceQuitDialog(controller: controller),
);

class MacForceQuitDialog extends StatefulWidget {
  const MacForceQuitDialog({super.key, required this.controller});

  final ShellController controller;

  @override
  State<MacForceQuitDialog> createState() => _MacForceQuitDialogState();
}

class _ForceQuitItem {
  const _ForceQuitItem({
    required this.id,
    required this.name,
    required this.app,
    this.isFinder = false,
  });

  final String id;
  final String name;
  final DesktopApp app;
  final bool isFinder;
}

class _MacForceQuitDialogState extends State<MacForceQuitDialog> {
  static const _finderItem = _ForceQuitItem(
    id: 'finder',
    name: '访达',
    app: DesktopApp(id: 'finder', name: '访达', desktopFile: ''),
    isFinder: true,
  );

  late List<_ForceQuitItem> _items;
  int _selectedIndex = 0;
  bool _busy = false;
  final _focusNode = FocusNode();

  @override
  void initState() {
    super.initState();
    _rebuildItems();
  }

  @override
  void dispose() {
    _focusNode.dispose();
    super.dispose();
  }

  void _rebuildItems() {
    final running = widget.controller.dockApps
        .where((app) => app.windows.isNotEmpty)
        .map((app) => _ForceQuitItem(id: app.id, name: app.name, app: app.app))
        .toList();

    // Check if finder is already represented by an open window app
    final hasFinder = running.any(
      (item) => MacAppIcons.isFileManager(item.app),
    );
    _items = [...running, if (!hasFinder) _finderItem];

    // Pre-select focused window app if any
    final focused = widget.controller.menuFocusedWindow;
    if (focused != null) {
      final index = _items.indexWhere(
        (item) =>
            item.id == focused.appId ||
            widget.controller.dockApps.any(
              (d) =>
                  d.id == item.id && d.windows.any((w) => w.id == focused.id),
            ),
      );
      if (index >= 0) {
        _selectedIndex = index;
        return;
      }
    }
    _selectedIndex = _selectedIndex.clamp(
      0,
      _items.isEmpty ? 0 : _items.length - 1,
    );
  }

  Future<void> _executeAction() async {
    if (_busy || _items.isEmpty) return;
    final item = _items[_selectedIndex];
    setState(() => _busy = true);
    try {
      if (item.isFinder || MacAppIcons.isFileManager(item.app)) {
        widget.controller.closePanel();
        if (mounted) Navigator.pop(context);
      } else {
        await widget.controller.forceQuitApp(item.id);
        if (mounted) Navigator.pop(context);
      }
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  void _handleKey(KeyEvent event) {
    if (event is! KeyDownEvent) return;
    if (event.logicalKey == LogicalKeyboardKey.arrowDown) {
      if (_selectedIndex < _items.length - 1) {
        setState(() => _selectedIndex++);
      }
    } else if (event.logicalKey == LogicalKeyboardKey.arrowUp) {
      if (_selectedIndex > 0) {
        setState(() => _selectedIndex--);
      }
    } else if (event.logicalKey == LogicalKeyboardKey.enter ||
        event.logicalKey == LogicalKeyboardKey.numpadEnter) {
      unawaited(_executeAction());
    } else if (event.logicalKey == LogicalKeyboardKey.escape) {
      Navigator.pop(context);
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final dark = theme.brightness == Brightness.dark;
    final selectedItem = _selectedIndex < _items.length
        ? _items[_selectedIndex]
        : null;
    final isFinder =
        selectedItem != null &&
        (selectedItem.isFinder || MacAppIcons.isFileManager(selectedItem.app));

    return KeyboardListener(
      focusNode: _focusNode..requestFocus(),
      onKeyEvent: _handleKey,
      child: Dialog(
        backgroundColor: dark
            ? const Color(0xff2d2d2f)
            : const Color(0xffececef),
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(14),
          side: BorderSide(
            color: dark ? Colors.white12 : Colors.black12,
            width: 0.5,
          ),
        ),
        insetPadding: const EdgeInsets.symmetric(horizontal: 24, vertical: 24),
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 390, maxHeight: 420),
          child: Padding(
            padding: const EdgeInsets.fromLTRB(20, 20, 20, 16),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Text(
                  '强制退出应用程序',
                  style: theme.textTheme.titleMedium?.copyWith(
                    fontWeight: FontWeight.w700,
                    fontSize: 14,
                  ),
                ),
                const SizedBox(height: 8),
                Text(
                  '如果某个应用程序没有响应，请在列表中将其选中，然后点按“强制退出”。',
                  style: theme.textTheme.bodySmall?.copyWith(
                    fontSize: 11,
                    height: 1.35,
                    color: theme.textTheme.bodySmall?.color?.withValues(
                      alpha: 0.75,
                    ),
                  ),
                ),
                const SizedBox(height: 14),
                // Inset scroll view with running apps
                Expanded(
                  child: Container(
                    decoration: BoxDecoration(
                      color: dark ? const Color(0xff1f1f21) : Colors.white,
                      borderRadius: BorderRadius.circular(6),
                      border: Border.all(
                        color: dark ? Colors.white12 : Colors.black12,
                        width: 0.8,
                      ),
                    ),
                    clipBehavior: Clip.antiAlias,
                    child: ListView.builder(
                      itemCount: _items.length,
                      padding: const EdgeInsets.symmetric(vertical: 2),
                      itemBuilder: (context, index) {
                        final item = _items[index];
                        final isSelected = index == _selectedIndex;
                        return Listener(
                          key: ValueKey('force-quit-item-${item.id}'),
                          behavior: HitTestBehavior.opaque,
                          onPointerDown: (_) =>
                              setState(() => _selectedIndex = index),
                          child: GestureDetector(
                            behavior: HitTestBehavior.opaque,
                            onDoubleTap: _executeAction,
                            child: Container(
                              padding: const EdgeInsets.symmetric(
                                horizontal: 10,
                                vertical: 4,
                              ),
                              color: isSelected
                                  ? const Color(0xff0a60ff)
                                  : Colors.transparent,
                              child: Row(
                                children: [
                                  SizedBox(
                                    width: 22,
                                    height: 22,
                                    child: AppIcon(
                                      app: item.app,
                                      size: 22,
                                      fallbackIcon:
                                          (item.isFinder ||
                                              MacAppIcons.isFileManager(
                                                item.app,
                                              ))
                                          ? Icons.folder_rounded
                                          : null,
                                      fallbackAssetPath:
                                          (item.isFinder ||
                                              MacAppIcons.isFileManager(
                                                item.app,
                                              ))
                                          ? 'assets/shell/finder.png'
                                          : null,
                                    ),
                                  ),
                                  const SizedBox(width: 8),
                                  Expanded(
                                    child: Text(
                                      item.name,
                                      style: TextStyle(
                                        fontSize: 13,
                                        fontWeight: isSelected
                                            ? FontWeight.w600
                                            : FontWeight.w400,
                                        color: isSelected
                                            ? Colors.white
                                            : (dark
                                                  ? Colors.white
                                                  : Colors.black),
                                      ),
                                      maxLines: 1,
                                      overflow: TextOverflow.ellipsis,
                                    ),
                                  ),
                                ],
                              ),
                            ),
                          ),
                        );
                      },
                    ),
                  ),
                ),
                const SizedBox(height: 12),
                Text(
                  '您可以按住“Option-Command-Esc”来直接打开此窗口。',
                  style: theme.textTheme.bodySmall?.copyWith(
                    fontSize: 10.5,
                    color: theme.textTheme.bodySmall?.color?.withValues(
                      alpha: 0.6,
                    ),
                  ),
                ),
                const SizedBox(height: 14),
                // Action buttons row
                Row(
                  mainAxisAlignment: MainAxisAlignment.end,
                  children: [
                    TextButton(
                      key: const ValueKey('force-quit-cancel'),
                      onPressed: () => Navigator.pop(context),
                      style: TextButton.styleFrom(
                        padding: const EdgeInsets.symmetric(
                          horizontal: 16,
                          vertical: 8,
                        ),
                      ),
                      child: const Text('取消'),
                    ),
                    const SizedBox(width: 10),
                    FilledButton(
                      key: const ValueKey('force-quit-confirm'),
                      onPressed: _busy ? null : _executeAction,
                      style: FilledButton.styleFrom(
                        backgroundColor: isFinder
                            ? const Color(0xff0a60ff)
                            : (dark
                                  ? const Color(0xffd32f2f)
                                  : const Color(0xffc62828)),
                        padding: const EdgeInsets.symmetric(
                          horizontal: 16,
                          vertical: 8,
                        ),
                      ),
                      child: Text(
                        isFinder ? '重新开启' : '强制退出',
                        style: const TextStyle(fontWeight: FontWeight.w600),
                      ),
                    ),
                  ],
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}
