// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';

import '../mac_shell_preferences.dart';
import '../state/shell_controller.dart';
import 'controller_selector.dart';

/// macOS choices are independent from the Material bar's saved position.
class MacShellSettings extends StatelessWidget {
  const MacShellSettings({super.key, required this.controller});
  final ShellController controller;

  @override
  Widget build(BuildContext context) => ControllerSelector(
    key: const ValueKey('mac-shell-settings-selector'),
    controller: controller,
    select: () => controller.macPreferences,
    builder: (context, _) => _build(context),
  );

  Widget _build(BuildContext context) {
    final p = controller.macPreferences;
    void update(MacShellPreferences next) => controller.setMacPreferences(next);
    Widget toggle(
      String key,
      String title,
      bool value,
      ValueChanged<bool> changed, {
      String? subtitle,
    }) => SwitchListTile.adaptive(
      key: ValueKey(key),
      title: Text(title),
      subtitle: subtitle == null ? null : Text(subtitle),
      value: value,
      onChanged: changed,
      contentPadding: EdgeInsets.zero,
    );
    Widget slider(
      String key,
      String label,
      double value,
      double min,
      double max,
      ValueChanged<double>? changed,
      String unit, {
      int? divisions,
    }) => Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text('$label  ${value.round()}$unit'),
        Slider(
          key: ValueKey(key),
          value: value,
          min: min,
          max: max,
          divisions: divisions,
          onChanged: changed,
          semanticFormatterCallback: (v) => '$label ${v.round()}$unit',
        ),
      ],
    );
    return Card(
      key: const ValueKey('mac-shell-settings'),
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text('菜单栏', style: Theme.of(context).textTheme.titleSmall),
            const SizedBox(height: 12),
            slider(
              'mac-menu-height',
              '高度',
              p.menuBarHeight,
              MacShellPreferences.minMenuBarHeight,
              MacShellPreferences.maxMenuBarHeight,
              (v) => update(p.copyWith(menuBarHeight: v)),
              ' px',
              divisions: 26,
            ),
            toggle(
              'mac-menu-auto-hide',
              '自动隐藏菜单栏',
              p.menuBarAutoHide,
              (v) => update(p.copyWith(menuBarAutoHide: v)),
              subtitle: '指针移到顶栏所在的顶部边缘时显示，打开菜单时保持显示',
            ),
            toggle(
              'mac-battery-percentage',
              '显示电池百分比',
              p.showBatteryPercentage,
              (v) => update(p.copyWith(showBatteryPercentage: v)),
            ),
            toggle(
              'mac-clock-date',
              '时钟显示日期',
              p.clockShowDate,
              (v) => update(p.copyWith(clockShowDate: v)),
            ),
            toggle(
              'mac-clock-seconds',
              '时钟显示秒数',
              p.clockShowSeconds,
              (v) => update(p.copyWith(clockShowSeconds: v)),
            ),
            const Divider(height: 28),
            Text('图标', style: Theme.of(context).textTheme.titleSmall),
            toggle(
              'mac-original-icons',
              '应用使用 Clarion 系统图标',
              p.useOriginalIcons,
              (v) => update(p.copyWith(useOriginalIcons: v)),
              subtitle: 'Dock 与启动器统一图标；第三方应用保留自己的标识',
            ),
            const Divider(height: 28),
            Text('Dock', style: Theme.of(context).textTheme.titleSmall),
            const SizedBox(height: 12),
            DropdownButtonFormField<MacDockPosition>(
              key: ValueKey('mac-dock-position-${p.dockPosition.name}'),
              initialValue: p.dockPosition,
              isExpanded: true,
              decoration: const InputDecoration(
                labelText: '屏幕位置',
                isDense: true,
                contentPadding: EdgeInsets.symmetric(
                  horizontal: 6,
                  vertical: 10,
                ),
              ),
              items: const [
                DropdownMenuItem(
                  value: MacDockPosition.bottom,
                  child: Text('底部'),
                ),
                DropdownMenuItem(
                  value: MacDockPosition.left,
                  child: Text('左侧'),
                ),
                DropdownMenuItem(
                  value: MacDockPosition.right,
                  child: Text('右侧'),
                ),
              ],
              onChanged: (v) {
                if (v != null) update(p.copyWith(dockPosition: v));
              },
            ),
            const SizedBox(height: 16),
            slider(
              'mac-dock-size',
              '图标尺寸',
              p.dockSize,
              32,
              64,
              (v) => update(p.copyWith(dockSize: v)),
              ' px',
            ),
            toggle(
              'mac-dock-magnification',
              '悬停放大',
              p.magnification,
              (v) => update(p.copyWith(magnification: v)),
            ),
            slider(
              'mac-dock-magnification-size',
              '放大尺寸',
              p.magnificationScale * 100,
              100,
              200,
              p.magnification
                  ? (v) => update(p.copyWith(magnificationScale: v / 100))
                  : null,
              '%',
            ),
            DropdownButtonFormField<MacDockVisibility>(
              key: ValueKey('mac-dock-visibility-${p.dockVisibility.name}'),
              initialValue: p.dockVisibility,
              isExpanded: true,
              decoration: const InputDecoration(
                labelText: 'Dock 显示方式',
                helperText: '隐藏会连同屏幕边缘的唤起区一起移除',
                isDense: true,
                contentPadding: EdgeInsets.symmetric(
                  horizontal: 6,
                  vertical: 10,
                ),
              ),
              items: const [
                DropdownMenuItem(
                  value: MacDockVisibility.always,
                  child: Text('始终显示'),
                ),
                DropdownMenuItem(
                  value: MacDockVisibility.autoHide,
                  child: Text('自动隐藏'),
                ),
                DropdownMenuItem(
                  value: MacDockVisibility.hidden,
                  child: Text('隐藏'),
                ),
              ],
              onChanged: (v) {
                if (v != null) update(p.withDockVisibility(v));
              },
            ),
            const SizedBox(height: 8),
            toggle(
              'mac-dock-indicators',
              '显示运行指示点',
              p.showRunningIndicators,
              (v) => update(p.copyWith(showRunningIndicators: v)),
            ),
            toggle(
              'mac-dock-recents',
              '显示最近使用的应用',
              p.showRecentApplications,
              (v) => update(p.copyWith(showRecentApplications: v)),
              subtitle: '在独立分区显示最多三个未固定的最近应用',
            ),
            toggle(
              'mac-dock-launch-animation',
              '打开应用时弹跳',
              p.animateOpeningApplications,
              (v) => update(p.copyWith(animateOpeningApplications: v)),
            ),
            const SizedBox(height: 12),
            DropdownButtonFormField<MacMinimizeEffect>(
              key: ValueKey('mac-minimize-effect-${p.minimizeEffect.name}'),
              initialValue: p.minimizeEffect,
              isExpanded: true,
              decoration: const InputDecoration(
                labelText: '最小化窗口时使用',
                isDense: true,
                contentPadding: EdgeInsets.symmetric(
                  horizontal: 6,
                  vertical: 10,
                ),
              ),
              items: const [
                DropdownMenuItem(
                  value: MacMinimizeEffect.genie,
                  child: Text('神奇效果'),
                ),
                DropdownMenuItem(
                  value: MacMinimizeEffect.scale,
                  child: Text('缩放效果'),
                ),
              ],
              onChanged: (v) {
                if (v != null) update(p.copyWith(minimizeEffect: v));
              },
            ),
            const SizedBox(height: 6),
            Text(
              '窗口收回 Dock 图标；减少动态时立即完成',
              style: Theme.of(context).textTheme.bodySmall,
            ),
            const Divider(height: 24),
            toggle(
              'mac-dock-launchpad',
              '显示 Launchpad',
              p.showLaunchpad,
              (v) => update(p.copyWith(showLaunchpad: v)),
            ),
            toggle(
              'mac-dock-downloads',
              '显示下载文件夹',
              p.showDownloads,
              (v) => update(p.copyWith(showDownloads: v)),
            ),
            const SizedBox(height: 12),
            DropdownButtonFormField<MacDownloadsDisplay>(
              key: ValueKey('mac-downloads-display-${p.downloadsDisplay.name}'),
              initialValue: p.downloadsDisplay,
              isExpanded: true,
              decoration: const InputDecoration(
                labelText: '下载图标显示为',
                isDense: true,
                contentPadding: EdgeInsets.symmetric(
                  horizontal: 6,
                  vertical: 10,
                ),
              ),
              items: const [
                DropdownMenuItem(
                  value: MacDownloadsDisplay.stack,
                  child: Text('叠放'),
                ),
                DropdownMenuItem(
                  value: MacDownloadsDisplay.folder,
                  child: Text('文件夹'),
                ),
              ],
              onChanged: p.showDownloads
                  ? (v) {
                      if (v != null) update(p.copyWith(downloadsDisplay: v));
                    }
                  : null,
            ),
            const SizedBox(height: 16),
            DropdownButtonFormField<MacDownloadsView>(
              key: ValueKey('mac-downloads-view-${p.downloadsView.name}'),
              initialValue: p.downloadsView,
              isExpanded: true,
              decoration: const InputDecoration(
                labelText: '打开下载叠放时的视图',
                isDense: true,
                contentPadding: EdgeInsets.symmetric(
                  horizontal: 6,
                  vertical: 10,
                ),
              ),
              items: const [
                DropdownMenuItem(
                  value: MacDownloadsView.automatic,
                  child: Text('自动'),
                ),
                DropdownMenuItem(
                  value: MacDownloadsView.fan,
                  child: Text('扇状'),
                ),
                DropdownMenuItem(
                  value: MacDownloadsView.grid,
                  child: Text('网格'),
                ),
                DropdownMenuItem(
                  value: MacDownloadsView.list,
                  child: Text('列表'),
                ),
              ],
              onChanged: p.showDownloads
                  ? (v) {
                      if (v != null) update(p.copyWith(downloadsView: v));
                    }
                  : null,
            ),
            const SizedBox(height: 6),
            TextButton(
              key: const ValueKey('mac-shell-reset'),
              onPressed: () => update(const MacShellPreferences()),
              child: const Text('恢复 Clarion 外壳默认设置'),
            ),
          ],
        ),
      ),
    );
  }
}
