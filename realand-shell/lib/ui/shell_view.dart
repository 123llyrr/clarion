// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../models.dart';
import '../state/shell_controller.dart';
import '../theme/shell_theme.dart';
import 'controller_selector.dart';
import 'global_app_menu_bar.dart';
import 'launcher_surface.dart';
import 'shell_geometry.dart';
import 'shell_panel_content.dart';

import 'app_icon.dart';
import 'tray_icon.dart';

export 'panel_components.dart';

class ShellView extends StatelessWidget {
  const ShellView({
    super.key,
    required this.controller,
    this.preview = false,
    this.onLauncherSurfaceVisible,
  });

  final ShellController controller;
  final bool preview;
  final ValueChanged<bool>? onLauncherSurfaceVisible;

  @override
  Widget build(BuildContext context) => AnimatedBuilder(
    animation: controller,
    child: _SystemBar(controller: controller),
    builder: (context, bar) => _scene(context, bar!),
  );

  Widget _scene(BuildContext context, Widget bar) {
    final colors = Theme.of(context).colorScheme;
    final reduced =
        controller.reduceMotion || MediaQuery.disableAnimationsOf(context);
    return CallbackShortcuts(
      bindings: {
        const SingleActivator(LogicalKeyboardKey.escape): controller.closePanel,
      },
      child: Focus(
        autofocus:
            controller.panel != ShellPanel.launcher &&
            controller.panel != ShellPanel.wallpapers,
        child: LayoutBuilder(
          builder: (context, constraints) {
            final width = constraints.maxWidth;
            final height = constraints.maxHeight;
            final bottom = controller.barPosition == BarPosition.bottom;
            final open = controller.panel != ShellPanel.none;
            final tooltip = controller.barTooltip;
            final widePanel =
                controller.panel == ShellPanel.launcher ||
                controller.panel == ShellPanel.wallpapers;
            // Stable surface keys keep conditional panels from replacing the
            // bar's Material, ink, hover and tray state during child diffing.
            return Stack(
              fit: StackFit.expand,
              children: [
                if (preview)
                  DecoratedBox(
                    decoration: BoxDecoration(
                      gradient: LinearGradient(
                        begin: Alignment.topLeft,
                        end: Alignment.bottomRight,
                        colors: [
                          colors.primaryContainer,
                          colors.surface,
                          colors.secondaryContainer,
                        ],
                      ),
                    ),
                    child: Center(
                      child: Column(
                        mainAxisSize: MainAxisSize.min,
                        children: [
                          Icon(
                            Icons.landscape_rounded,
                            size: 96,
                            color: colors.primary,
                          ),
                          const SizedBox(height: 16),
                          Text(
                            'Realand',
                            style: Theme.of(context).textTheme.displayMedium,
                          ),
                          const SizedBox(height: 8),
                          Text(
                            '你的空间，自在掌控',
                            style: Theme.of(context).textTheme.titleMedium,
                          ),
                        ],
                      ),
                    ),
                  ),
                if (preview)
                  Positioned(
                    left: 20,
                    right: 20,
                    top: bottom ? 16 : null,
                    bottom: bottom ? null : 16,
                    child: Center(
                      child: Text(
                        controller.demo ? '演示预览 · 不执行桌面操作' : '实时预览 · 不替换当前外壳',
                        style: Theme.of(context).textTheme.labelLarge,
                      ),
                    ),
                  ),
                if (open &&
                    controller.panel != ShellPanel.launcher &&
                    height > ShellGeometry.barHeight)
                  Positioned.fill(
                    key: const ValueKey('shell-panel-scrim'),
                    top: bottom ? 0 : ShellGeometry.barHeight,
                    bottom: bottom ? ShellGeometry.barHeight : 0,
                    child: Semantics(
                      label: '关闭面板',
                      button: true,
                      child: GestureDetector(
                        behavior: HitTestBehavior.opaque,
                        onTap: controller.closePanel,
                        child: ColoredBox(
                          color: colors.scrim.withValues(alpha: 0.14),
                        ),
                      ),
                    ),
                  ),
                Positioned.fill(
                  key: const ValueKey('launcher-surface-position'),
                  top: bottom ? 0 : ShellGeometry.barHeight,
                  bottom: bottom ? ShellGeometry.barHeight : 0,
                  child: LauncherSurface(
                    controller: controller,
                    visible: controller.panel == ShellPanel.launcher,
                    reducedMotion: reduced,
                    onSurfaceVisible: onLauncherSurfaceVisible,
                  ),
                ),
                Positioned(
                  key: const ValueKey('shell-system-bar'),
                  top: bottom ? null : 0,
                  bottom: bottom ? 0 : null,
                  left: 0,
                  right: 0,
                  height: ShellGeometry.barHeight,
                  child: RepaintBoundary(child: bar),
                ),
                if (!open &&
                    tooltip != null &&
                    height >= ShellGeometry.tooltipHeight)
                  Positioned(
                    key: const ValueKey('shell-bar-tooltip-position'),
                    top: bottom ? null : 48,
                    bottom: bottom ? 48 : null,
                    left: 0,
                    right: 0,
                    height: 76,
                    child: IgnorePointer(
                      child: ExcludeSemantics(
                        child: CustomSingleChildLayout(
                          delegate: _BarTooltipLayout(
                            anchorX: tooltip.anchor.dx,
                            bottom: bottom,
                          ),
                          child: Material(
                            key: const ValueKey('bar-tooltip-bubble'),
                            color: colors.inverseSurface,
                            elevation: 4,
                            borderRadius: BorderRadius.circular(12),
                            clipBehavior: Clip.antiAlias,
                            child: Padding(
                              padding: const EdgeInsets.symmetric(
                                horizontal: 12,
                                vertical: 8,
                              ),
                              child: Text(
                                tooltip.message,
                                maxLines: 3,
                                overflow: TextOverflow.ellipsis,
                                style: Theme.of(context).textTheme.bodySmall
                                    ?.copyWith(color: colors.onInverseSurface),
                              ),
                            ),
                          ),
                        ),
                      ),
                    ),
                  ),
                if (open &&
                    controller.panel != ShellPanel.launcher &&
                    height > ShellGeometry.panelTop + 28)
                  Positioned(
                    key: const ValueKey('shell-panel-position'),
                    top: bottom ? null : ShellGeometry.panelTop,
                    bottom: bottom ? ShellGeometry.panelTop : null,
                    left: widePanel ? math.max(12, (width - 880) / 2) : null,
                    right: widePanel ? null : 12,
                    width: widePanel
                        ? math.min(880, width - 24)
                        : math.min(440, width - 24),
                    child: ConstrainedBox(
                      constraints: BoxConstraints(
                        maxHeight: math.max(
                          0,
                          height -
                              ShellGeometry.panelTop -
                              ShellGeometry.panelGap,
                        ),
                      ),
                      child: TweenAnimationBuilder<double>(
                        key: ValueKey(controller.panel),
                        tween: Tween(begin: 0, end: 1),
                        duration: ShellMotion.duration(
                          reduced,
                          ShellMotion.enter,
                        ),
                        curve: ShellMotion.curve,
                        builder: (context, value, child) => Transform.translate(
                          offset: Offset(0, (1 - value) * (bottom ? 12 : -12)),
                          child: Opacity(opacity: value, child: child),
                        ),
                        child: Material(
                          color: colors.surfaceContainer,
                          elevation: 6,
                          shadowColor: colors.shadow.withValues(alpha: 0.25),
                          borderRadius: BorderRadius.circular(28),
                          clipBehavior: Clip.antiAlias,
                          child: FocusTraversalGroup(
                            child: ShellPanelContent(controller: controller),
                          ),
                        ),
                      ),
                    ),
                  ),
              ],
            );
          },
        ),
      ),
    );
  }
}

/// Position the actual measured hint, including short text near output edges.
class _BarTooltipLayout extends SingleChildLayoutDelegate {
  const _BarTooltipLayout({required this.anchorX, required this.bottom});
  final double anchorX;
  final bool bottom;

  @override
  BoxConstraints getConstraintsForChild(BoxConstraints constraints) =>
      BoxConstraints(
        maxWidth: math.min(280.0, math.max(0.0, constraints.maxWidth - 16)),
        maxHeight: math.min(76.0, constraints.maxHeight),
      );

  @override
  Offset getPositionForChild(Size size, Size childSize) => Offset(
    (anchorX - childSize.width / 2)
        .clamp(8.0, math.max(8.0, size.width - childSize.width - 8))
        .toDouble(),
    bottom ? size.height - childSize.height : 0,
  );

  @override
  bool shouldRelayout(covariant _BarTooltipLayout oldDelegate) =>
      oldDelegate.anchorX != anchorX || oldDelegate.bottom != bottom;
}

class _SystemBar extends StatelessWidget {
  const _SystemBar({required this.controller});
  final ShellController controller;

  Widget _workspaces(BuildContext context) => SingleChildScrollView(
    scrollDirection: Axis.horizontal,
    child: Row(
      children: controller.workspaces
          .map(
            (workspace) => Padding(
              padding: const EdgeInsets.symmetric(horizontal: 3),
              child: ShellBarTooltip(
                controller: controller,
                message:
                    '工作区 ${workspace.label} · ${workspace.windowCount} 个窗口',
                child: ChoiceChip(
                  key: ValueKey('workspace-${workspace.id}'),
                  label: Text(workspace.label),
                  selected: workspace.isActive || workspace.isFocused,
                  showCheckmark: false,
                  visualDensity: const VisualDensity(
                    horizontal: -2,
                    vertical: -2,
                  ),
                  materialTapTargetSize: MaterialTapTargetSize.shrinkWrap,
                  padding: const EdgeInsets.symmetric(horizontal: 6),
                  labelPadding: const EdgeInsets.symmetric(horizontal: 2),
                  labelStyle: Theme.of(context).textTheme.labelMedium,
                  onSelected: (_) => controller.switchWorkspace(workspace.id),
                ),
              ),
            ),
          )
          .toList(),
    ),
  );

  @override
  Widget build(BuildContext context) => ControllerSelector(
    key: const ValueKey('system-bar-selector'),
    controller: controller,
    // Slider values and busy state are absent: the strip never displays them.
    // The desktop revision also avoids deriving workspaces on every pointer
    // event; their labels and window counts are computed only when needed.
    select: () => (
      desktop: controller.desktopRevision,
      hour: controller.now.hour,
      minute: controller.now.minute,
      panel: controller.panel,
      muted: controller.muted,
      battery: controller.batteryFraction,
      charging: controller.charging,
      notifications: controller.notifications.length,
      doNotDisturb: controller.doNotDisturb,
      trayItems: controller.trayItems,
      appMenuAvailable: controller.materialAppMenuAvailable,
      appMenuRevision: controller.appMenu.revision,
      appMenuItems: controller.appMenu.menus,
      appMenuDismissRevision: controller.appMenuDismissRevision,
    ),
    builder: (context, _) => _buildThemedBar(context),
  );

  Widget _buildThemedBar(BuildContext context) {
    final theme = Theme.of(context);
    // Only the mouse-oriented desktop strip is dense. Floating panels keep
    // Material's normal targets, typography and spacing.
    return Theme(
      data: theme.copyWith(
        iconTheme: theme.iconTheme.copyWith(size: 20),
        iconButtonTheme: IconButtonThemeData(
          style: IconButton.styleFrom(
            minimumSize: const Size.square(32),
            fixedSize: const Size.square(36),
            iconSize: 20,
            padding: const EdgeInsets.all(8),
            tapTargetSize: MaterialTapTargetSize.shrinkWrap,
            visualDensity: VisualDensity.standard,
          ),
        ),
      ),
      child: Builder(builder: _buildStrip),
    );
  }

  Widget _buildStrip(BuildContext context) => Material(
    color: Theme.of(context).colorScheme.surfaceContainer,
    elevation: 2,
    child: Padding(
      padding: const EdgeInsets.symmetric(horizontal: 8),
      child: LayoutBuilder(
        builder: (context, constraints) {
          final wide = constraints.maxWidth > 720;
          final time =
              '${controller.now.hour.toString().padLeft(2, '0')}:${controller.now.minute.toString().padLeft(2, '0')}';
          final launcher = ShellBarTooltip(
            key: const ValueKey('bar-launcher'),
            controller: controller,
            message: '应用与搜索',
            child: IconButton.filledTonal(
              isSelected: controller.panel == ShellPanel.launcher,
              onPressed: () => controller.selectPanel(ShellPanel.launcher),
              icon: ChromeLauncherIcon(
                isOpen: controller.panel == ShellPanel.launcher,
                size: 20,
              ),
            ),
          );
          final menuRevision = controller.appMenuDismissRevision;
          final appMenu = controller.materialAppMenuAvailable
              ? SizedBox(
                  key: const ValueKey('material-app-menu'),
                  width: 36,
                  height: ShellGeometry.barHeight,
                  child: ShellBarTooltip(
                    controller: controller,
                    message: '应用菜单',
                    labelSemantics: false,
                    child: GlobalAppMenuBar(
                      key: ValueKey(('material-app-menu', menuRevision)),
                      service: controller.appMenu,
                      compact: true,
                      barHeight: ShellGeometry.barHeight,
                      onOpenChanged: (open) {
                        if (controller.appMenuDismissRevision == menuRevision &&
                            controller.shellStyle == ShellStyle.material) {
                          controller.setAppMenuOpen(open);
                        }
                      },
                    ),
                  ),
                )
              : null;
          final clock = Padding(
            padding: const EdgeInsets.symmetric(horizontal: 12),
            child: Semantics(
              label: '当前时间 $time',
              child: Text(time, style: Theme.of(context).textTheme.titleSmall),
            ),
          );
          final trayLimit = constraints.maxWidth > 850
              ? 4
              : constraints.maxWidth > 720
              ? 3
              : constraints.maxWidth > 650
              ? 4
              : constraints.maxWidth > 550
              ? 2
              : 0;
          final visibleTray = controller.trayItems.take(trayLimit).toList();
          final hiddenTray = controller.trayItems.length - visibleTray.length;
          final status = <Widget>[
            ShellBarTooltip(
              key: const ValueKey('bar-screenshot'),
              controller: controller,
              message: '截图',
              child: IconButton(
                isSelected: controller.panel == ShellPanel.screenshot,
                onPressed: () => controller.selectPanel(ShellPanel.screenshot),
                icon: const Icon(Icons.screenshot_monitor_rounded),
              ),
            ),
            if (constraints.maxWidth > 960 &&
                controller.batteryFraction != null)
              ShellBarTooltip(
                controller: controller,
                message:
                    '${controller.charging ? '正在充电 · ' : ''}电量 ${(controller.batteryFraction! * 100).round()}%',
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Icon(
                      controller.charging
                          ? Icons.battery_charging_full_rounded
                          : Icons.battery_full_rounded,
                      size: 18,
                    ),
                    const SizedBox(width: 4),
                    Text(
                      '${(controller.batteryFraction! * 100).round()}%',
                      style: Theme.of(context).textTheme.labelMedium,
                    ),
                  ],
                ),
              ),
            if (constraints.maxWidth > 550)
              ShellBarTooltip(
                key: const ValueKey('bar-volume'),
                controller: controller,
                message: controller.muted ? '音量已静音' : '音量控制',
                child: IconButton(
                  onPressed: () =>
                      controller.selectPanel(ShellPanel.quickSettings),
                  icon: Icon(
                    controller.muted
                        ? Icons.volume_off_rounded
                        : Icons.volume_up_rounded,
                  ),
                ),
              ),
            for (final item in visibleTray)
              TrayBarButton(
                key: ValueKey('tray-bar-item-${item.id}'),
                controller: controller,
                item: item,
              ),
            if (hiddenTray > 0)
              ShellBarTooltip(
                key: const ValueKey('tray-overflow'),
                controller: controller,
                message: '系统托盘',
                child: IconButton(
                  isSelected: controller.panel == ShellPanel.trayItems,
                  onPressed: controller.openTrayItems,
                  icon: Badge(
                    label: Text('$hiddenTray'),
                    child: const Icon(Icons.keyboard_arrow_down_rounded),
                  ),
                ),
              ),
            ShellBarTooltip(
              key: const ValueKey('bar-notifications'),
              controller: controller,
              message: '通知',
              child: IconButton(
                isSelected: controller.panel == ShellPanel.notifications,
                onPressed: () =>
                    controller.selectPanel(ShellPanel.notifications),
                icon: Badge(
                  isLabelVisible:
                      !controller.doNotDisturb &&
                      controller.notifications.isNotEmpty,
                  label: Text('${controller.notifications.length}'),
                  child: Icon(
                    controller.doNotDisturb
                        ? Icons.notifications_off_outlined
                        : Icons.notifications_outlined,
                  ),
                ),
              ),
            ),
            ShellBarTooltip(
              key: const ValueKey('bar-quick-settings'),
              controller: controller,
              message: '控制中心',
              child: IconButton(
                isSelected: controller.panel == ShellPanel.quickSettings,
                onPressed: () =>
                    controller.selectPanel(ShellPanel.quickSettings),
                icon: const Icon(Icons.tune_rounded),
              ),
            ),
          ];
          if (!wide) {
            // Keep every action reachable even on narrow portrait outputs.
            // Leave room for an application's retained menu entry when present.
            // A scrollable strip preserves each existing 36px action target.
            if (constraints.maxWidth < 180 + (appMenu == null ? 0 : 36)) {
              return SingleChildScrollView(
                scrollDirection: Axis.horizontal,
                child: Row(
                  children: [
                    launcher,
                    ?appMenu,
                    ...status,
                    _workspaces(context),
                  ],
                ),
              );
            }
            return Row(
              children: [
                launcher,
                ?appMenu,
                Expanded(child: _workspaces(context)),
                if (constraints.maxWidth >= 320) clock,
                ...status,
              ],
            );
          }
          final statusPod = Material(
            color: Theme.of(context)
                .colorScheme
                .surfaceContainerHighest
                .withValues(alpha: 0.55),
            borderRadius: BorderRadius.circular(20),
            clipBehavior: Clip.antiAlias,
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 4, vertical: 2),
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: status,
              ),
            ),
          );
          return Row(
            children: [
              Expanded(
                child: Row(
                  children: [
                    launcher,
                    ?appMenu,
                    const SizedBox(width: 8),
                    _workspaces(context),
                    const SizedBox(width: 8),
                    Expanded(child: _ShelfApps(controller: controller)),
                  ],
                ),
              ),
              clock,
              Expanded(
                child: Row(
                  mainAxisAlignment: MainAxisAlignment.end,
                  children: [
                    if (constraints.maxWidth > 1120)
                      Expanded(
                        child: Padding(
                          padding: const EdgeInsets.symmetric(horizontal: 16),
                          child: Text(
                            controller.windowTitle,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            textAlign: TextAlign.right,
                            style: Theme.of(context).textTheme.bodySmall,
                          ),
                        ),
                      ),
                    statusPod,
                  ],
                ),
              ),
            ],
          );
        },
      ),
    ),
  );
}

/// ChromeOS concentric rings launcher icon, matching extracted ChromeOS Ash res_31799.svg.
class ChromeLauncherIcon extends StatelessWidget {
  const ChromeLauncherIcon({
    super.key,
    required this.isOpen,
    this.size = 20,
  });
  final bool isOpen;
  final double size;

  @override
  Widget build(BuildContext context) {
    final color = isOpen
        ? Theme.of(context).colorScheme.primary
        : Theme.of(context).colorScheme.onSurface;
    return CustomPaint(
      size: Size.square(size),
      painter: _ChromeLauncherPainter(color: color, isOpen: isOpen),
    );
  }
}

class _ChromeLauncherPainter extends CustomPainter {
  _ChromeLauncherPainter({required this.color, required this.isOpen});
  final Color color;
  final bool isOpen;

  @override
  void paint(Canvas canvas, Size size) {
    final center = Offset(size.width / 2, size.height / 2);
    final scale = size.width / 20.0;
    final fillPaint = Paint()
      ..color = color
      ..style = PaintingStyle.fill;
    final ringPaint = Paint()
      ..color = color
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1.5 * scale;

    // Center disc: 5px radius in 20x20
    canvas.drawCircle(center, (isOpen ? 5.8 : 4.8) * scale, fillPaint);
    // Outer concentric ring: 7.75px radius with 1.5px stroke
    canvas.drawCircle(center, 7.75 * scale, ringPaint);
  }

  @override
  bool shouldRepaint(covariant _ChromeLauncherPainter oldDelegate) =>
      oldDelegate.color != color || oldDelegate.isOpen != isOpen;
}

/// ChromeOS Shelf app icon items with 4px active running indicator dots.
class _ShelfApps extends StatelessWidget {
  const _ShelfApps({required this.controller});
  final ShellController controller;

  @override
  Widget build(BuildContext context) {
    final apps = controller.dockApps;
    if (apps.isEmpty) return const SizedBox.shrink();
    final colors = Theme.of(context).colorScheme;
    return SingleChildScrollView(
      scrollDirection: Axis.horizontal,
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          for (final dockApp in apps)
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 2),
              child: ShellBarTooltip(
                controller: controller,
                message: dockApp.name,
                child: InkWell(
                  borderRadius: BorderRadius.circular(12),
                  onTap: () {
                    if (dockApp.windows.isNotEmpty) {
                      final focusedWindow = dockApp.windows.firstWhere(
                        (w) => w.isFocused,
                        orElse: () => dockApp.windows.first,
                      );
                      controller.focusWindow(focusedWindow.id);
                    } else {
                      controller.launchApp(dockApp.app);
                    }
                  },
                  child: SizedBox(
                    width: 36,
                    height: 36,
                    child: Column(
                      mainAxisAlignment: MainAxisAlignment.center,
                      children: [
                        AppIcon(app: dockApp.app, size: 20),
                        const SizedBox(height: 2),
                        if (dockApp.isRunning)
                          Container(
                            width: dockApp.isFocused ? 8 : 4,
                            height: dockApp.isFocused ? 3 : 4,
                            decoration: BoxDecoration(
                              color: dockApp.isFocused
                                  ? colors.primary
                                  : colors.onSurfaceVariant,
                              borderRadius: BorderRadius.circular(2),
                            ),
                          )
                        else
                          const SizedBox(height: 4),
                      ],
                    ),
                  ),
                ),
              ),
            ),
        ],
      ),
    );
  }
}
