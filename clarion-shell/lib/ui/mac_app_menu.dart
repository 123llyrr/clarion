// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../state/shell_controller.dart';

/// Authentic macOS Application Menu displayed when clicking the active application name
/// in the menu bar.
class MacAppMenu extends StatelessWidget {
  const MacAppMenu({super.key, required this.controller});
  final ShellController controller;

  @override
  Widget build(BuildContext context) {
    final focused = controller.menuFocusedWindow;
    final isFinder = focused == null;

    final app = focused != null
        ? controller.dockApps
              .where((a) => a.windows.any((w) => w.id == focused.id))
              .firstOrNull
        : null;
    final appName = isFinder
        ? '访达'
        : (app?.name ?? (focused.appId.isNotEmpty ? focused.appId : '访达'));
    final appId = focused?.appId ?? 'finder';

    final hasMinimized = controller.windows.any((w) => w.isMinimized);
    final hasOthers = isFinder
        ? controller.windows.isNotEmpty
        : controller.windows.any((w) => w.appId != focused.appId);

    return FocusTraversalGroup(
      child: Shortcuts(
        shortcuts: const {
          SingleActivator(LogicalKeyboardKey.arrowDown): NextFocusIntent(),
          SingleActivator(LogicalKeyboardKey.arrowUp): PreviousFocusIntent(),
        },
        child: FocusScope(
          autofocus: true,
          child: SingleChildScrollView(
            padding: const EdgeInsets.symmetric(vertical: 5),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                _MenuRow(
                  key: const ValueKey('app-menu-about'),
                  label: '关于“$appName”',
                  onPressed: () {
                    controller.closePanel();
                    controller.openSettings();
                  },
                ),
                _MenuRow(
                  key: const ValueKey('app-menu-preferences'),
                  label: '偏好设置…',
                  trailing: '⌘,',
                  onPressed: () {
                    controller.closePanel();
                    controller.openSettings();
                  },
                ),
                if (isFinder) ...[
                  const _Separator(),
                  _MenuRow(
                    key: const ValueKey('app-menu-empty-trash'),
                    label: '清倒废纸篓…',
                    trailing: '⇧⌘⌫',
                    onPressed: controller.trashState.hasItems
                        ? () {
                            controller.closePanel();
                            controller.emptyTrash();
                          }
                        : null,
                  ),
                ],
                const _Separator(),
                const _MenuRow(
                  key: ValueKey('app-menu-services'),
                  label: '服务',
                  trailing: '›',
                  onPressed: null,
                ),
                const _Separator(),
                _MenuRow(
                  key: const ValueKey('app-menu-hide'),
                  label: '隐藏“$appName”',
                  trailing: '⌘H',
                  onPressed: () {
                    controller.closePanel();
                    controller.hideApp(appId);
                  },
                ),
                _MenuRow(
                  key: const ValueKey('app-menu-hide-others'),
                  label: '隐藏其他',
                  trailing: '⌥⌘H',
                  onPressed: hasOthers
                      ? () {
                          controller.closePanel();
                          controller.hideOthers(appId);
                        }
                      : null,
                ),
                _MenuRow(
                  key: const ValueKey('app-menu-show-all'),
                  label: '显示全部',
                  onPressed: hasMinimized
                      ? () {
                          controller.closePanel();
                          controller.showAll();
                        }
                      : null,
                ),
                if (!isFinder) ...[
                  const _Separator(),
                  _MenuRow(
                    key: const ValueKey('app-menu-quit'),
                    label: '退出“$appName”',
                    trailing: '⌘Q',
                    onPressed: () {
                      controller.closePanel();
                      controller.closeAppWindows(appId);
                    },
                  ),
                ],
              ],
            ),
          ),
        ),
      ),
    );
  }
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
  });
  final String label;
  final String? trailing;
  final VoidCallback? onPressed;

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
      onEnter: (_) => setState(() => _hovered = true),
      onExit: (_) => setState(() => _hovered = false),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 5),
        child: SizedBox(
          width: double.infinity,
          height: 26,
          child: TextButton(
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
