// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import '../mac_app_icons.dart';
import '../mac_shell_preferences.dart';
import '../models.dart';
import '../state/shell_controller.dart';

/// Context menu for the Dock divider corresponding to Catalina's `dock` template.
class MacDockContextMenu extends StatelessWidget {
  const MacDockContextMenu({super.key, required this.controller});
  final ShellController controller;

  static double estimatedHeight() => 286.0;

  @override
  Widget build(BuildContext context) {
    final preferences = controller.macPreferences;
    return Shortcuts(
      shortcuts: const {
        SingleActivator(LogicalKeyboardKey.arrowDown): NextFocusIntent(),
        SingleActivator(LogicalKeyboardKey.arrowUp): PreviousFocusIntent(),
      },
      child: SingleChildScrollView(
        padding: const EdgeInsets.symmetric(vertical: 6),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            _DockMenuRow(
              actionKey: 'dock-menu-toggle-autohide',
              label: preferences.dockAutoHide ? '关闭隐藏' : '启用隐藏',
              autofocus: true,
              onPressed: () {
                controller.updateMacPreferences(
                  preferences.copyWith(dockAutoHide: !preferences.dockAutoHide),
                );
                controller.closePanel();
              },
            ),
            _DockMenuRow(
              actionKey: 'dock-menu-toggle-mag',
              label: preferences.magnification ? '关闭放大' : '启用放大',
              onPressed: () {
                controller.updateMacPreferences(
                  preferences.copyWith(
                    magnification: !preferences.magnification,
                  ),
                );
                controller.closePanel();
              },
            ),
            const _DockMenuSeparator(),
            const _DockMenuSectionHeader(label: '置于屏幕上的位置'),
            _DockMenuRow(
              actionKey: 'dock-menu-position-left',
              label: '左边',
              indicator: preferences.dockPosition == MacDockPosition.left
                  ? Icons.check
                  : null,
              onPressed: () {
                controller.updateMacPreferences(
                  preferences.copyWith(dockPosition: MacDockPosition.left),
                );
                controller.closePanel();
              },
            ),
            _DockMenuRow(
              actionKey: 'dock-menu-position-bottom',
              label: '底部',
              indicator: preferences.dockPosition == MacDockPosition.bottom
                  ? Icons.check
                  : null,
              onPressed: () {
                controller.updateMacPreferences(
                  preferences.copyWith(dockPosition: MacDockPosition.bottom),
                );
                controller.closePanel();
              },
            ),
            _DockMenuRow(
              actionKey: 'dock-menu-position-right',
              label: '右边',
              indicator: preferences.dockPosition == MacDockPosition.right
                  ? Icons.check
                  : null,
              onPressed: () {
                controller.updateMacPreferences(
                  preferences.copyWith(dockPosition: MacDockPosition.right),
                );
                controller.closePanel();
              },
            ),
            const _DockMenuSeparator(),
            const _DockMenuSectionHeader(label: '最小化时使用'),
            _DockMenuRow(
              actionKey: 'dock-menu-effect-genie',
              label: '神奇效果',
              indicator: preferences.minimizeEffect == MacMinimizeEffect.genie
                  ? Icons.check
                  : null,
              onPressed: () {
                controller.updateMacPreferences(
                  preferences.copyWith(minimizeEffect: MacMinimizeEffect.genie),
                );
                controller.closePanel();
              },
            ),
            _DockMenuRow(
              actionKey: 'dock-menu-effect-scale',
              label: '缩放效果',
              indicator: preferences.minimizeEffect == MacMinimizeEffect.scale
                  ? Icons.check
                  : null,
              onPressed: () {
                controller.updateMacPreferences(
                  preferences.copyWith(minimizeEffect: MacMinimizeEffect.scale),
                );
                controller.closePanel();
              },
            ),
            _DockMenuRow(
              actionKey: 'dock-menu-toggle-minimize-into-app',
              label: '将窗口最小化为应用程序图标',
              indicator: preferences.minimizeIntoApp ? Icons.check : null,
              onPressed: () {
                controller.updateMacPreferences(
                  preferences.copyWith(
                    minimizeIntoApp: !preferences.minimizeIntoApp,
                  ),
                );
                controller.closePanel();
              },
            ),
            const _DockMenuSeparator(),
            _DockMenuRow(
              actionKey: 'dock-menu-preferences',
              label: '程序坞偏好设置…',
              onPressed: () {
                controller.closePanel();
                controller.selectPanel(ShellPanel.quickSettings);
              },
            ),
          ],
        ),
      ),
    );
  }
}

/// Context menu for Trash corresponding to Catalina's `trash` template.
class MacDockTrashMenu extends StatelessWidget {
  const MacDockTrashMenu({super.key, required this.controller});
  final ShellController controller;

  static double estimatedHeight() => 74.0;

  @override
  Widget build(BuildContext context) {
    final hasItems = controller.trashState.hasItems;
    return Shortcuts(
      shortcuts: const {
        SingleActivator(LogicalKeyboardKey.arrowDown): NextFocusIntent(),
        SingleActivator(LogicalKeyboardKey.arrowUp): PreviousFocusIntent(),
      },
      child: SingleChildScrollView(
        padding: const EdgeInsets.symmetric(vertical: 6),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            _DockMenuRow(
              actionKey: 'dock-trash-open',
              label: '打开',
              autofocus: true,
              onPressed: () => controller.openTrash(),
            ),
            const _DockMenuSeparator(),
            _DockMenuRow(
              actionKey: 'dock-trash-empty',
              label: '清倒废纸篓',
              onPressed: hasItems ? () => controller.emptyTrash() : null,
            ),
          ],
        ),
      ),
    );
  }
}

/// Context menu for Finder corresponding to Catalina's Finder template.
class MacDockFinderMenu extends StatelessWidget {
  const MacDockFinderMenu({super.key, required this.controller});
  final ShellController controller;

  static double estimatedHeight(ShellController controller) {
    final finderApp = controller.dockApps
        .where((a) => MacAppIcons.isFileManager(a.app))
        .firstOrNull;
    final windows = finderApp?.windows ?? const [];
    return 12.0 +
        (windows.length * 26.0) +
        (windows.isNotEmpty ? 9.0 : 0.0) +
        26.0;
  }

  @override
  Widget build(BuildContext context) {
    final finderApp = controller.dockApps
        .where((a) => MacAppIcons.isFileManager(a.app))
        .firstOrNull;
    final windows = finderApp?.windows ?? const [];
    return Shortcuts(
      shortcuts: const {
        SingleActivator(LogicalKeyboardKey.arrowDown): NextFocusIntent(),
        SingleActivator(LogicalKeyboardKey.arrowUp): PreviousFocusIntent(),
      },
      child: SingleChildScrollView(
        padding: const EdgeInsets.symmetric(vertical: 6),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            for (final window in windows)
              _DockMenuRow(
                actionKey: 'dock-window-${window.id}',
                label: window.title.isEmpty ? '访达' : window.title,
                indicator: window.isFocused
                    ? Icons.check
                    : window.isMinimized
                    ? Icons.diamond_outlined
                    : null,
                onPressed: () {
                  controller.focusWindow(window.id);
                  controller.closePanel();
                },
              ),
            if (windows.isNotEmpty) const _DockMenuSeparator(),
            _DockMenuRow(
              actionKey: 'dock-finder-new-window',
              label: '新建访达窗口',
              autofocus: windows.isEmpty,
              onPressed: () {
                controller.closePanel();
                if (finderApp != null) {
                  controller.launchApp(finderApp.app);
                } else {
                  controller.openHomeFolder();
                }
              },
            ),
          ],
        ),
      ),
    );
  }
}

/// Context menu for Downloads folder stack corresponding to Catalina's folder template.
class MacDockDownloadsMenu extends StatelessWidget {
  const MacDockDownloadsMenu({super.key, required this.controller});
  final ShellController controller;

  static double estimatedHeight() => 443.0;

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: controller,
    builder: (context, _) => _buildMenu(context),
  );

  Widget _buildMenu(BuildContext context) {
    final preferences = controller.macPreferences;
    final effectiveSort = controller.dockFolder.effectiveSort(
      preferences.downloadsSort,
    );

    VoidCallback? sortAction(MacDownloadsSort sort) {
      if (!controller.dockFolder.supportsSort(sort)) return null;
      return () {
        if (!controller.dockFolder.supportsSort(sort)) return;
        controller.updateMacPreferences(
          preferences.copyWith(downloadsSort: sort),
        );
        controller.closePanel();
      };
    }

    return Shortcuts(
      shortcuts: const {
        SingleActivator(LogicalKeyboardKey.arrowDown): NextFocusIntent(),
        SingleActivator(LogicalKeyboardKey.arrowUp): PreviousFocusIntent(),
      },
      child: SingleChildScrollView(
        padding: const EdgeInsets.symmetric(vertical: 6),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            const _DockMenuSectionHeader(label: '排序方式'),
            _DockMenuRow(
              actionKey: 'dock-downloads-sort-name',
              label: '名称',
              indicator: effectiveSort == MacDownloadsSort.name
                  ? Icons.check
                  : null,
              onPressed: sortAction(MacDownloadsSort.name),
            ),
            _DockMenuRow(
              actionKey: 'dock-downloads-sort-added',
              label: '添加日期',
              indicator: effectiveSort == MacDownloadsSort.dateAdded
                  ? Icons.check
                  : null,
              onPressed: sortAction(MacDownloadsSort.dateAdded),
            ),
            _DockMenuRow(
              actionKey: 'dock-downloads-sort-mtime',
              label: '修改日期',
              indicator: effectiveSort == MacDownloadsSort.dateModified
                  ? Icons.check
                  : null,
              onPressed: sortAction(MacDownloadsSort.dateModified),
            ),
            _DockMenuRow(
              actionKey: 'dock-downloads-sort-created',
              label: '创建日期',
              indicator: effectiveSort == MacDownloadsSort.dateCreated
                  ? Icons.check
                  : null,
              onPressed: sortAction(MacDownloadsSort.dateCreated),
            ),
            _DockMenuRow(
              actionKey: 'dock-downloads-sort-kind',
              label: '种类',
              indicator: effectiveSort == MacDownloadsSort.kind
                  ? Icons.check
                  : null,
              onPressed: sortAction(MacDownloadsSort.kind),
            ),
            const _DockMenuSeparator(),
            const _DockMenuSectionHeader(label: '显示内容为'),
            _DockMenuRow(
              actionKey: 'dock-downloads-view-fan',
              label: '扇状',
              indicator: preferences.downloadsView == MacDownloadsView.fan
                  ? Icons.check
                  : null,
              onPressed: () {
                controller.updateMacPreferences(
                  preferences.copyWith(downloadsView: MacDownloadsView.fan),
                );
                controller.closePanel();
              },
            ),
            _DockMenuRow(
              actionKey: 'dock-downloads-view-grid',
              label: '网格',
              indicator: preferences.downloadsView == MacDownloadsView.grid
                  ? Icons.check
                  : null,
              onPressed: () {
                controller.updateMacPreferences(
                  preferences.copyWith(downloadsView: MacDownloadsView.grid),
                );
                controller.closePanel();
              },
            ),
            _DockMenuRow(
              actionKey: 'dock-downloads-view-list',
              label: '列表',
              indicator: preferences.downloadsView == MacDownloadsView.list
                  ? Icons.check
                  : null,
              onPressed: () {
                controller.updateMacPreferences(
                  preferences.copyWith(downloadsView: MacDownloadsView.list),
                );
                controller.closePanel();
              },
            ),
            _DockMenuRow(
              actionKey: 'dock-downloads-view-auto',
              label: '自动',
              indicator: preferences.downloadsView == MacDownloadsView.automatic
                  ? Icons.check
                  : null,
              onPressed: () {
                controller.updateMacPreferences(
                  preferences.copyWith(
                    downloadsView: MacDownloadsView.automatic,
                  ),
                );
                controller.closePanel();
              },
            ),
            const _DockMenuSeparator(),
            const _DockMenuSectionHeader(label: '显示为'),
            _DockMenuRow(
              actionKey: 'dock-downloads-display-stack',
              label: '叠放',
              indicator:
                  preferences.downloadsDisplay == MacDownloadsDisplay.stack
                  ? Icons.check
                  : null,
              onPressed: () {
                controller.updateMacPreferences(
                  preferences.copyWith(
                    downloadsDisplay: MacDownloadsDisplay.stack,
                  ),
                );
                controller.closePanel();
              },
            ),
            _DockMenuRow(
              actionKey: 'dock-downloads-display-folder',
              label: '文件夹',
              indicator:
                  preferences.downloadsDisplay == MacDownloadsDisplay.folder
                  ? Icons.check
                  : null,
              onPressed: () {
                controller.updateMacPreferences(
                  preferences.copyWith(
                    downloadsDisplay: MacDownloadsDisplay.folder,
                  ),
                );
                controller.closePanel();
              },
            ),
            const _DockMenuSeparator(),
            _DockMenuRow(
              actionKey: 'dock-downloads-reveal',
              label: '在访达中显示',
              onPressed: () {
                controller.openDownloadsFolder();
                controller.closePanel();
              },
            ),
            _DockMenuRow(
              actionKey: 'dock-downloads-open',
              label: '打开“下载”',
              onPressed: () {
                controller.openDownloadsFolder();
                controller.closePanel();
              },
            ),
          ],
        ),
      ),
    );
  }
}

class _DockMenuSectionHeader extends StatelessWidget {
  const _DockMenuSectionHeader({required this.label});
  final String label;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.only(left: 28, top: 4, bottom: 2, right: 12),
    child: Text(
      label,
      style: TextStyle(
        fontSize: 11,
        fontWeight: FontWeight.w600,
        color: Theme.of(context).colorScheme.onSurface.withValues(alpha: .5),
      ),
    ),
  );
}

class _DockMenuSeparator extends StatelessWidget {
  const _DockMenuSeparator();

  @override
  Widget build(BuildContext context) => const Padding(
    padding: EdgeInsets.symmetric(vertical: 4),
    child: Divider(height: 1, thickness: .5),
  );
}

class _DockMenuRow extends StatefulWidget {
  const _DockMenuRow({
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
  State<_DockMenuRow> createState() => _DockMenuRowState();
}

class _DockMenuRowState extends State<_DockMenuRow> {
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
