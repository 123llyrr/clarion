// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';

import '../models.dart';
import '../state/shell_controller.dart';
import 'notifications_panel.dart';
import 'mac_notification_center.dart';
import 'panel_components.dart';
import 'quick_settings_panel.dart';
import 'mac_volume_menu.dart';
import 'mac_control_center.dart';
import 'mac_system_menu.dart';
import 'screenshot_panel.dart';
import 'tray_menu_panel.dart';
import 'wallpaper_panel.dart';
import 'mac_dock_app_menu.dart';
import 'mac_dock_context_menu.dart';
import 'mac_downloads_stack_panel.dart';
import 'mac_app_menu.dart';

/// Both shells share the same controls and controller-owned services.
class ShellPanelContent extends StatelessWidget {
  const ShellPanelContent({
    super.key,
    required this.controller,
    this.preview = false,
    this.downloadsFanAnchorX,
    this.downloadsFanAnchorYBelowPanel,
    this.downloadsFanOriginIconSize,
  });
  final ShellController controller;
  final bool preview;
  final double? downloadsFanAnchorX;
  final double? downloadsFanAnchorYBelowPanel;
  final double? downloadsFanOriginIconSize;

  @override
  Widget build(BuildContext context) => switch (controller.panel) {
    ShellPanel.screenshot => ScreenshotPanel(controller: controller),
    ShellPanel.quickSettings =>
      controller.shellStyle == ShellStyle.macos
          ? MacControlCenter(controller: controller)
          : QuickSettingsPanel(controller: controller),
    ShellPanel.volume => MacVolumeMenu(controller: controller),
    ShellPanel.notifications =>
      controller.shellStyle == ShellStyle.macos
          ? MacNotificationCenter(controller: controller)
          : NotificationsPanel(controller: controller),
    ShellPanel.wallpapers => WallpaperPanel(controller: controller),
    ShellPanel.trayMenu => TrayMenuPanel(controller: controller),
    ShellPanel.trayItems => TrayItemsPanel(controller: controller),
    ShellPanel.systemMenu =>
      controller.shellStyle == ShellStyle.macos
          ? MacSystemMenu(controller: controller, preview: preview)
          : _SystemMenu(controller: controller),
    ShellPanel.appMenu => MacAppMenu(controller: controller),
    ShellPanel.dockAppMenu =>
      controller.shellStyle == ShellStyle.macos
          ? MacDockAppMenu(controller: controller)
          : _DockAppMenu(controller: controller),
    ShellPanel.dockContextMenu => MacDockContextMenu(controller: controller),
    ShellPanel.dockTrashMenu => MacDockTrashMenu(controller: controller),
    ShellPanel.dockFinderMenu => MacDockFinderMenu(controller: controller),
    ShellPanel.dockDownloadsMenu => MacDockDownloadsMenu(controller: controller),
    ShellPanel.dockDownloads => _downloadsPanel(),
    ShellPanel.launcher ||
    ShellPanel.none ||
    ShellPanel.spotlight ||
    ShellPanel.spaces => const SizedBox.shrink(),
  };

  Widget _downloadsPanel() {
    final revision = controller.panelRevision;
    return MacDownloadsStackPanel(
      key: ValueKey('mac-downloads-state-$revision'),
      service: controller.dockFolder,
      preferences: controller.macPreferences,
      fanAnchorX: downloadsFanAnchorX,
      fanAnchorYBelowPanel: downloadsFanAnchorYBelowPanel,
      fanOriginIconSize: downloadsFanOriginIconSize,
      onPreferencesChanged: controller.setMacPreferences,
      onClose: () => controller.closePanelAtRevision(revision),
    );
  }
}

class _SystemMenu extends StatelessWidget {
  const _SystemMenu({required this.controller});
  final ShellController controller;

  @override
  Widget build(BuildContext context) => SingleChildScrollView(
    padding: const EdgeInsets.all(16),
    child: Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        PanelHeading(title: 'Realand', onClose: controller.closePanel),
        const SizedBox(height: 4),
        _MenuAction(
          actionKey: 'system-menu-launcher',
          icon: Icons.apps_rounded,
          label: '应用与搜索',
          onTap: () => controller.selectPanel(ShellPanel.launcher),
        ),
        _MenuAction(
          actionKey: 'system-menu-overview',
          icon: Icons.window_rounded,
          label: '工作区概览',
          onTap: controller.busy.contains('overview')
              ? null
              : controller.toggleOverview,
        ),
        const Divider(),
        _MenuAction(
          actionKey: 'system-menu-settings',
          icon: Icons.settings_outlined,
          label: '系统设置',
          onTap: controller.busy.contains('settings')
              ? null
              : controller.openSettings,
        ),
        _MenuAction(
          actionKey: 'system-menu-control-center',
          icon: Icons.tune_rounded,
          label: '控制中心与外观',
          onTap: () => controller.selectPanel(ShellPanel.quickSettings),
        ),
        _MenuAction(
          actionKey: 'system-menu-wallpapers',
          icon: Icons.wallpaper_rounded,
          label: '更换壁纸',
          onTap: controller.openWallpapers,
        ),
        const Divider(),
        _MenuAction(
          actionKey: 'system-menu-notifications',
          icon: Icons.notifications_outlined,
          label: '通知中心',
          onTap: () => controller.selectPanel(ShellPanel.notifications),
        ),
        _MenuAction(
          actionKey: 'system-menu-screenshot',
          icon: Icons.screenshot_monitor_rounded,
          label: '截图',
          onTap: () => controller.selectPanel(ShellPanel.screenshot),
        ),
        if (controller.error != null)
          ShellStatusMessage(text: controller.error!, error: true),
      ],
    ),
  );
}

class _DockAppMenu extends StatelessWidget {
  const _DockAppMenu({required this.controller});
  final ShellController controller;

  @override
  Widget build(BuildContext context) {
    final app = controller.selectedDockApp;
    if (app == null) {
      return Padding(
        padding: const EdgeInsets.all(16),
        child: PanelHeading(title: '应用已关闭', onClose: controller.closePanel),
      );
    }
    return SingleChildScrollView(
      padding: const EdgeInsets.all(16),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          PanelHeading(title: app.name, onClose: controller.closePanel),
          const SizedBox(height: 4),
          if (app.windows.isNotEmpty) ...[
            Padding(
              padding: const EdgeInsets.fromLTRB(12, 4, 12, 8),
              child: Text(
                '打开的窗口',
                style: Theme.of(context).textTheme.labelMedium,
              ),
            ),
            for (final window in app.windows)
              _MenuAction(
                actionKey: 'dock-window-${window.id}',
                icon: window.isFocused
                    ? Icons.check_rounded
                    : Icons.web_asset_rounded,
                label: window.title.isEmpty ? app.name : window.title,
                onTap: () => controller.focusWindow(window.id),
              ),
            if (controller.canMinimize)
              for (final window in app.windows)
                _MenuAction(
                  actionKey: 'dock-minimize-${window.id}',
                  icon: window.isMinimized
                      ? Icons.open_in_new_rounded
                      : Icons.minimize_rounded,
                  label:
                      '${window.isMinimized ? '还原' : '最小化'} · ${window.title.isEmpty ? app.name : window.title}',
                  onTap: () => window.isMinimized
                      ? controller.focusWindow(window.id)
                      : controller.minimizeWindow(window.id),
                ),
            const Divider(),
          ],
          if (app.isRunning || app.canPin || controller.demo)
            _MenuAction(
              actionKey: 'dock-menu-open',
              icon: app.isRunning
                  ? Icons.open_in_new_rounded
                  : Icons.play_arrow_rounded,
              label: app.isRunning ? '切换到应用' : '打开应用',
              onTap: () => controller.focusOrLaunchApp(app.id),
            ),
          if (app.canPin || controller.demo)
            _MenuAction(
              actionKey: 'dock-menu-pin',
              icon: app.isPinned
                  ? Icons.push_pin_rounded
                  : Icons.push_pin_outlined,
              label: app.isPinned ? '从 Dock 取消固定' : '固定到 Dock',
              onTap: () {
                controller.togglePinnedApp(app.id);
                controller.closePanel();
              },
            ),
          if (controller.error != null)
            ShellStatusMessage(text: controller.error!, error: true),
        ],
      ),
    );
  }
}

class _MenuAction extends StatelessWidget {
  const _MenuAction({
    required this.actionKey,
    required this.icon,
    required this.label,
    required this.onTap,
  });
  final String actionKey;
  final IconData icon;
  final String label;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) => ListTile(
    key: ValueKey(actionKey),
    leading: Icon(icon, size: 20),
    title: Text(label, maxLines: 2, overflow: TextOverflow.ellipsis),
    dense: true,
    shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(10)),
    onTap: onTap,
    enabled: onTap != null,
    visualDensity: const VisualDensity(vertical: -1),
  );
}
