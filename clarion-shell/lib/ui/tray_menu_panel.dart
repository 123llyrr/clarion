// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';

import 'package:flutter/material.dart';

import '../models.dart';
import '../state/shell_controller.dart';
import 'panel_components.dart';
import 'tray_icon.dart';

class TrayMenuPanel extends StatelessWidget {
  const TrayMenuPanel({super.key, required this.controller});
  final ShellController controller;

  @override
  Widget build(BuildContext context) {
    final size = MediaQuery.sizeOf(context);
    final compact = size.height < 400 || size.width < 280;
    final item = controller.selectedTrayItem;
    final busy = controller.busy.contains('tray');
    final content = Padding(
      padding: const EdgeInsets.all(20),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          PanelHeading(
            title: item?.title ?? '托盘',
            onClose: controller.closePanel,
            actions: [
              IconButton(
                tooltip: '返回系统托盘',
                onPressed: controller.openTrayItems,
                icon: const Icon(Icons.widgets_outlined),
              ),
              if (item != null)
                Padding(
                  padding: const EdgeInsets.only(right: 4),
                  child: TrayGlyph(item: item, size: 24),
                ),
            ],
          ),
          if (!controller.trayAvailable)
            const ShellStatusMessage(text: '托盘服务未启用或由其他桌面组件提供'),
          const SizedBox(height: 12),
          PanelBody(
            scrollWhole: compact,
            child: controller.trayMenuLoading
                ? const Padding(
                    padding: EdgeInsets.symmetric(vertical: 48),
                    child: Center(child: CircularProgressIndicator()),
                  )
                : controller.trayMenu.isEmpty
                ? Padding(
                    padding: const EdgeInsets.symmetric(vertical: 48),
                    child: Column(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        Icon(
                          Icons.widgets_outlined,
                          size: 48,
                          color: Theme.of(context).colorScheme.onSurfaceVariant,
                        ),
                        const SizedBox(height: 16),
                        Text(
                          '没有可用菜单',
                          style: Theme.of(context).textTheme.titleMedium,
                        ),
                        const SizedBox(height: 6),
                        const Text('左键激活托盘项，右键打开菜单'),
                      ],
                    ),
                  )
                : ListView(
                    shrinkWrap: true,
                    physics: compact
                        ? const NeverScrollableScrollPhysics()
                        : null,
                    children: [
                      for (final entry in controller.trayMenu)
                        ..._tiles(context, entry, busy: busy),
                    ],
                  ),
          ),
          if (controller.trayError != null)
            ShellStatusMessage(text: controller.trayError!, error: true),
        ],
      ),
    );
    return compact ? SingleChildScrollView(child: content) : content;
  }

  List<Widget> _tiles(
    BuildContext context,
    TrayMenuEntry entry, {
    required bool busy,
    bool parentEnabled = true,
  }) {
    if (!entry.visible) return const [];
    if (entry.separator) {
      return const [
        Padding(
          padding: EdgeInsets.symmetric(vertical: 4),
          child: Divider(height: 1),
        ),
      ];
    }
    final colors = Theme.of(context).colorScheme;
    final enabled = parentEnabled && entry.enabled && !busy;
    if (entry.children.isNotEmpty) {
      return [
        ExpansionTile(
          key: ValueKey('tray-submenu-${entry.id}'),
          title: Text(entry.label.isEmpty ? '子菜单' : entry.label),
          leading: _leading(entry, colors),
          enabled: enabled,
          children: [
            for (final child in entry.children)
              ..._tiles(context, child, busy: busy, parentEnabled: enabled),
          ],
        ),
      ];
    }
    return [
      Semantics(
        checked: entry.toggleType == TrayMenuToggleType.none
            ? null
            : entry.toggleState == 1,
        child: ListTile(
          key: ValueKey('tray-menu-entry-${entry.id}'),
          enabled: enabled,
          leading: _leading(entry, colors),
          title: Text(
            entry.label.isEmpty ? '菜单项' : entry.label,
            style: entry.destructive ? TextStyle(color: colors.error) : null,
          ),
          onTap: enabled
              ? () => controller.activateTrayMenuEntry(entry.id)
              : null,
        ),
      ),
    ];
  }

  Widget? _leading(TrayMenuEntry entry, ColorScheme colors) {
    if (entry.toggleType == TrayMenuToggleType.none) return null;
    final selected = entry.toggleState == 1;
    if (entry.toggleType == TrayMenuToggleType.radio) {
      return Icon(
        selected
            ? Icons.radio_button_checked_rounded
            : Icons.radio_button_off_rounded,
        color: colors.onSurfaceVariant,
      );
    }
    return Icon(
      selected ? Icons.check_rounded : Icons.check_box_outline_blank_rounded,
      color: colors.onSurfaceVariant,
    );
  }
}

/// Narrow strips and large collections keep every application accessible here.
class TrayItemsPanel extends StatelessWidget {
  const TrayItemsPanel({super.key, required this.controller});
  final ShellController controller;

  @override
  Widget build(BuildContext context) {
    final size = MediaQuery.sizeOf(context);
    final compact = size.height < 400 || size.width < 280;
    final items = controller.trayItems;
    final content = Padding(
      padding: const EdgeInsets.all(20),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          PanelHeading(title: '系统托盘', onClose: controller.closePanel),
          const SizedBox(height: 8),
          const Text('点击打开应用，右键或菜单按钮查看操作'),
          const SizedBox(height: 12),
          if (!controller.trayAvailable)
            const ShellStatusMessage(text: '托盘服务暂不可用'),
          PanelBody(
            scrollWhole: compact,
            child: items.isEmpty
                ? const Padding(
                    padding: EdgeInsets.symmetric(vertical: 36),
                    child: Center(child: Text('当前没有托盘应用')),
                  )
                : ListView.builder(
                    key: const ValueKey('tray-items-list'),
                    shrinkWrap: true,
                    physics: compact
                        ? const NeverScrollableScrollPhysics()
                        : null,
                    itemCount: items.length,
                    itemBuilder: (context, index) => _TrayListTile(
                      controller: controller,
                      item: items[index],
                    ),
                  ),
          ),
          if (controller.trayError != null)
            ShellStatusMessage(text: controller.trayError!, error: true),
        ],
      ),
    );
    return compact ? SingleChildScrollView(child: content) : content;
  }
}

class _TrayListTile extends StatefulWidget {
  const _TrayListTile({required this.controller, required this.item});
  final ShellController controller;
  final TrayItem item;

  @override
  State<_TrayListTile> createState() => _TrayListTileState();
}

class _TrayListTileState extends State<_TrayListTile> {
  Offset? pressed;
  Offset? menuPressed;

  @override
  void didUpdateWidget(covariant _TrayListTile oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.item.id != widget.item.id ||
        oldWidget.controller != widget.controller) {
      pressed = menuPressed = null;
    }
  }

  @override
  Widget build(BuildContext context) {
    final controller = widget.controller;
    final item = widget.item;
    Offset center() {
      final box = context.findRenderObject();
      return box is RenderBox
          ? box.localToGlobal(box.size.center(Offset.zero))
          : Offset.zero;
    }

    void openMenu(Offset position, {int button = 3}) => unawaited(
      controller.openTrayMenu(
        item,
        x: position.dx,
        y: position.dy,
        button: button,
      ),
    );
    return TrayPointerRegion(
      controller: controller,
      item: item,
      forwardWheel: false,
      child: InkWell(
        key: ValueKey('tray-list-${item.id}'),
        borderRadius: BorderRadius.circular(16),
        onTapDown: (details) => pressed = details.globalPosition,
        onTapCancel: () => pressed = null,
        onTap: () {
          final menuButton = pressed == null ? 0 : 1;
          final position = pressed ?? center();
          pressed = null;
          unawaited(
            controller.activateTrayItem(
              item,
              x: position.dx,
              y: position.dy,
              menuButton: menuButton,
            ),
          );
        },
        onSecondaryTapUp: (details) => openMenu(details.globalPosition),
        onLongPress: () => openMenu(pressed ?? center(), button: 1),
        child: ListTile(
          leading: TrayGlyph(item: item, size: 28),
          title: Text(item.title, maxLines: 2, overflow: TextOverflow.ellipsis),
          subtitle: item.status == TrayStatus.needsAttention
              ? const Text('需要关注')
              : null,
          trailing: Listener(
            onPointerDown: (event) => menuPressed = event.position,
            onPointerCancel: (_) => menuPressed = null,
            onPointerUp: (_) => scheduleMicrotask(() => menuPressed = null),
            child: IconButton(
              tooltip: '${item.title} · 菜单',
              onPressed: () {
                final point = menuPressed ?? center();
                final button = menuPressed == null ? 0 : 1;
                menuPressed = null;
                openMenu(point, button: button);
              },
              icon: const Icon(Icons.more_vert_rounded),
            ),
          ),
        ),
      ),
    );
  }
}
