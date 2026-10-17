// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';
import '../models.dart';
import '../state/shell_controller.dart';
import 'power_mode_card.dart';
import 'panel_components.dart';
import 'mac_shell_settings.dart';
import 'controller_selector.dart';

class QuickSettingsPanel extends StatelessWidget {
  const QuickSettingsPanel({
    super.key,
    required this.controller,
    this.preferencesOnly = false,
    this.onBack,
  });
  final ShellController controller;
  final bool preferencesOnly;
  final VoidCallback? onBack;

  @override
  Widget build(BuildContext context) => ControllerSelector(
    key: const ValueKey('quick-settings-selector'),
    controller: controller,
    // Sliders subscribe below: dragging must not rebuild every settings row.
    select: () => (
      controller.wifiEnabled,
      controller.bluetoothEnabled,
      controller.networkName,
      controller.busy.contains('wifi'),
      controller.busy.contains('bluetooth'),
      controller.doNotDisturb,
      controller.reduceMotion,
      controller.shellStyle,
      controller.useExternalLauncher,
      controller.themeMode,
      controller.barPosition,
      controller.macPreferences,
      controller.error,
    ),
    builder: (context, _) => _content(context),
  );

  Widget _content(BuildContext context) => SingleChildScrollView(
    padding: preferencesOnly
        ? const EdgeInsets.fromLTRB(12, 6, 12, 12)
        : const EdgeInsets.all(20),
    child: Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (preferencesOnly) ...[
          Row(
            children: [
              IconButton(
                key: const ValueKey('mac-control-back'),
                tooltip: '返回控制中心',
                onPressed: onBack,
                icon: const Icon(Icons.chevron_left_rounded, size: 20),
              ),
              Expanded(
                child: Text(
                  '桌面偏好设置',
                  style: Theme.of(context).textTheme.titleSmall,
                ),
              ),
            ],
          ),
          const SizedBox(height: 8),
        ] else ...[
          PanelHeading(title: '控制中心', onClose: controller.closePanel),
          const SizedBox(height: 16),
          _ConnectivityTiles(controller: controller),
          const SizedBox(height: 16),
          _SystemSliders(controller: controller),
          const SizedBox(height: 12),
        ],
        PowerModeCard(service: controller.powerProfiles),
        const SizedBox(height: 12),
        Card(
          child: Column(
            children: [
              _PreferenceSwitch(
                icon: Icons.notifications_off_outlined,
                title: '勿扰模式',
                subtitle: '暂停通知计数提醒，保留历史',
                value: controller.doNotDisturb,
                onChanged: controller.setDoNotDisturb,
              ),
              _PreferenceSwitch(
                icon: Icons.animation_rounded,
                title: '减少动态效果',
                value: controller.reduceMotion,
                onChanged: controller.setReduceMotion,
              ),
            ],
          ),
        ),
        const SizedBox(height: 20),
        Text('外观', style: Theme.of(context).textTheme.titleSmall),
        const SizedBox(height: 10),
        Card(
          child: Padding(
            padding: const EdgeInsets.all(12),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                if (controller.shellStyle == ShellStyle.macos)
                  SwitchListTile.adaptive(
                    key: const ValueKey('use-external-launcher'),
                    contentPadding: EdgeInsets.zero,
                    title: const Text('新版应用面板'),
                    subtitle: const Text('关闭后使用经典启动器'),
                    value: controller.useExternalLauncher,
                    onChanged: controller.setUseExternalLauncher,
                  ),
                Text('界面风格', style: Theme.of(context).textTheme.titleSmall),
                const SizedBox(height: 10),
                LayoutBuilder(
                  builder: (context, constraints) {
                    if (constraints.maxWidth < 280) {
                      return DropdownButton<ShellStyle>(
                        key: const ValueKey('shell-style'),
                        value: controller.shellStyle,
                        isExpanded: true,
                        items: const [
                          DropdownMenuItem(
                            value: ShellStyle.material,
                            child: Text('Material'),
                          ),
                          DropdownMenuItem(
                            value: ShellStyle.macos,
                            child: Text('Clarion'),
                          ),
                        ],
                        onChanged: (value) {
                          if (value != null) controller.setShellStyle(value);
                        },
                      );
                    }
                    return SegmentedButton<ShellStyle>(
                      key: const ValueKey('shell-style'),
                      segments: const [
                        ButtonSegment(
                          value: ShellStyle.material,
                          label: Text('Material'),
                          icon: Icon(Icons.dashboard_rounded),
                        ),
                        ButtonSegment(
                          value: ShellStyle.macos,
                          label: Text('Clarion'),
                          icon: Icon(Icons.desktop_mac_rounded),
                        ),
                      ],
                      selected: {controller.shellStyle},
                      onSelectionChanged: (values) =>
                          controller.setShellStyle(values.single),
                      showSelectedIcon: false,
                    );
                  },
                ),
              ],
            ),
          ),
        ),
        const SizedBox(height: 10),
        LayoutBuilder(
          builder: (context, constraints) {
            if (constraints.maxWidth < 280) {
              return DropdownButtonFormField<ThemeMode>(
                initialValue: controller.themeMode,
                isExpanded: true,
                items: const [
                  DropdownMenuItem(
                    value: ThemeMode.system,
                    child: Text('跟随系统'),
                  ),
                  DropdownMenuItem(value: ThemeMode.light, child: Text('浅色')),
                  DropdownMenuItem(value: ThemeMode.dark, child: Text('深色')),
                ],
                onChanged: (value) {
                  if (value != null) controller.setThemeMode(value);
                },
              );
            }
            return SegmentedButton<ThemeMode>(
              segments: const [
                ButtonSegment(
                  value: ThemeMode.system,
                  label: Text('自动'),
                  icon: Icon(Icons.brightness_auto_rounded),
                ),
                ButtonSegment(
                  value: ThemeMode.light,
                  label: Text('浅色'),
                  icon: Icon(Icons.light_mode_outlined),
                ),
                ButtonSegment(
                  value: ThemeMode.dark,
                  label: Text('深色'),
                  icon: Icon(Icons.dark_mode_outlined),
                ),
              ],
              selected: {controller.themeMode},
              onSelectionChanged: (value) =>
                  controller.setThemeMode(value.single),
              showSelectedIcon: false,
            );
          },
        ),
        const SizedBox(height: 16),
        Text('栏位置', style: Theme.of(context).textTheme.titleSmall),
        const SizedBox(height: 10),
        LayoutBuilder(
          builder: (context, constraints) {
            if (constraints.maxWidth < 280) {
              return InputDecorator(
                decoration: const InputDecoration(isDense: true),
                child: DropdownButtonHideUnderline(
                  child: DropdownButton<BarPosition>(
                    key: const ValueKey('bar-position'),
                    value: controller.barPosition,
                    isExpanded: true,
                    items: const [
                      DropdownMenuItem(
                        value: BarPosition.top,
                        child: Text('顶部'),
                      ),
                      DropdownMenuItem(
                        value: BarPosition.bottom,
                        child: Text('底部'),
                      ),
                    ],
                    onChanged: controller.shellStyle == ShellStyle.macos
                        ? null
                        : (value) {
                            if (value != null) controller.setBarPosition(value);
                          },
                  ),
                ),
              );
            }
            return SegmentedButton<BarPosition>(
              key: const ValueKey('bar-position'),
              segments: const [
                ButtonSegment(
                  value: BarPosition.top,
                  label: Text('顶部'),
                  icon: Icon(Icons.vertical_align_top_rounded),
                ),
                ButtonSegment(
                  value: BarPosition.bottom,
                  label: Text('底部'),
                  icon: Icon(Icons.vertical_align_bottom_rounded),
                ),
              ],
              selected: {controller.barPosition},
              onSelectionChanged: controller.shellStyle == ShellStyle.macos
                  ? null
                  : (value) => controller.setBarPosition(value.single),
              showSelectedIcon: false,
            );
          },
        ),
        if (controller.shellStyle == ShellStyle.macos)
          MacShellSettings(controller: controller),
        const SizedBox(height: 16),
        Card(
          child: InkWell(
            key: const ValueKey('choose-wallpaper'),
            borderRadius: BorderRadius.circular(20),
            onTap: controller.openWallpapers,
            child: Padding(
              padding: const EdgeInsets.all(16),
              child: Row(
                children: [
                  const Icon(Icons.wallpaper_rounded),
                  const SizedBox(width: 12),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          '壁纸',
                          style: Theme.of(context).textTheme.titleSmall,
                        ),
                        const SizedBox(height: 4),
                        Text(
                          '选择壁纸',
                          style: Theme.of(context).textTheme.bodyMedium,
                        ),
                      ],
                    ),
                  ),
                  const Icon(Icons.chevron_right_rounded, size: 20),
                ],
              ),
            ),
          ),
        ),
        if (controller.error != null)
          ShellStatusMessage(text: controller.error!, error: true),
        const SizedBox(height: 20),
        FilledButton.tonalIcon(
          onPressed: controller.openSettings,
          icon: const Icon(Icons.settings_outlined),
          label: const Text('打开 Clarion 配置中心'),
        ),
      ],
    ),
  );
}

// MacShellView intentionally filters controller notifications at the scene
// boundary. Dynamic controls own their subscription in either shell style.
class _SystemSliders extends StatelessWidget {
  const _SystemSliders({required this.controller});
  final ShellController controller;

  @override
  Widget build(BuildContext context) => ControllerSelector(
    key: const ValueKey('system-sliders-selector'),
    controller: controller,
    select: () => (
      controller.volume,
      controller.brightness,
      controller.muted,
      controller.busy.contains('mute'),
    ),
    builder: (context, _) => RepaintBoundary(
      key: const ValueKey('system-sliders'),
      child: Card(
        child: Padding(
          padding: const EdgeInsets.fromLTRB(12, 8, 16, 8),
          child: Column(
            children: [
              Row(
                children: [
                  IconButton(
                    tooltip: controller.muted ? '取消静音' : '静音',
                    onPressed:
                        controller.volume == null ||
                            controller.busy.contains('mute')
                        ? null
                        : controller.toggleMute,
                    icon: Icon(
                      controller.muted
                          ? Icons.volume_off_rounded
                          : Icons.volume_up_rounded,
                    ),
                  ),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          '音量',
                          style: Theme.of(context).textTheme.titleSmall,
                        ),
                        Text(
                          controller.volume == null
                              ? '不可用'
                              : '${(controller.volume! * 100).round()}%',
                          style: Theme.of(context).textTheme.bodySmall,
                        ),
                      ],
                    ),
                  ),
                ],
              ),
              Slider(
                key: const ValueKey('volume-slider'),
                value: (controller.volume ?? 0).clamp(0, 1).toDouble(),
                semanticFormatterCallback: (value) =>
                    '音量 ${(value * 100).round()}%',
                onChangeStart: controller.volume == null
                    ? null
                    : (_) => controller.beginVolumeAdjustment(),
                onChanged: controller.volume == null
                    ? null
                    : controller.setVolume,
                onChangeEnd: controller.volume == null
                    ? null
                    : controller.endVolumeAdjustment,
              ),
              Row(
                children: [
                  const SizedBox(width: 12),
                  const Icon(Icons.brightness_6_outlined),
                  const SizedBox(width: 16),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          '亮度',
                          style: Theme.of(context).textTheme.titleSmall,
                        ),
                        Text(
                          controller.brightness == null
                              ? '不可用'
                              : '${(controller.brightness! * 100).round()}%',
                          style: Theme.of(context).textTheme.bodySmall,
                        ),
                      ],
                    ),
                  ),
                ],
              ),
              Slider(
                key: const ValueKey('brightness-slider'),
                min: .01,
                value: (controller.brightness ?? .01).clamp(.01, 1).toDouble(),
                semanticFormatterCallback: (value) =>
                    '亮度 ${(value * 100).round()}%',
                onChangeStart: controller.brightness == null
                    ? null
                    : (_) => controller.beginBrightnessAdjustment(),
                onChanged: controller.brightness == null
                    ? null
                    : controller.setBrightness,
                onChangeEnd: controller.brightness == null
                    ? null
                    : controller.endBrightnessAdjustment,
              ),
            ],
          ),
        ),
      ),
    ),
  );
}

class _ConnectivityTiles extends StatelessWidget {
  const _ConnectivityTiles({required this.controller});
  final ShellController controller;

  @override
  Widget build(BuildContext context) => LayoutBuilder(
    builder: (context, constraints) {
      final wifi = _QuickTile(
        label: 'Wi-Fi',
        subtitle: controller.wifiEnabled == null
            ? '服务不可用'
            : controller.wifiEnabled!
            ? controller.networkName.isEmpty
                  ? '已开启'
                  : controller.networkName
            : '已关闭',
        icon: Icons.wifi_rounded,
        active: controller.wifiEnabled == true,
        busy: controller.busy.contains('wifi'),
        onPressed: controller.wifiEnabled == null
            ? null
            : controller.toggleWifi,
      );
      final bluetooth = _QuickTile(
        label: '蓝牙',
        subtitle: controller.bluetoothEnabled == null
            ? '服务不可用'
            : controller.bluetoothEnabled!
            ? '已开启'
            : '已关闭',
        icon: Icons.bluetooth_rounded,
        active: controller.bluetoothEnabled == true,
        busy: controller.busy.contains('bluetooth'),
        onPressed: controller.bluetoothEnabled == null
            ? null
            : controller.toggleBluetooth,
      );
      final dnd = _QuickTile(
        label: '勿扰模式',
        subtitle: controller.doNotDisturb ? '已开启' : '已关闭',
        icon: controller.doNotDisturb
            ? Icons.notifications_off_rounded
            : Icons.notifications_outlined,
        active: controller.doNotDisturb,
        busy: false,
        onPressed: () => controller.setDoNotDisturb(!controller.doNotDisturb),
      );
      final screenshot = _QuickTile(
        label: '屏幕截图',
        subtitle: '截取屏幕',
        icon: Icons.screenshot_monitor_rounded,
        active: controller.panel == ShellPanel.screenshot,
        busy: false,
        onPressed: () => controller.selectPanel(ShellPanel.screenshot),
      );
      final darkTheme = _QuickTile(
        label: '深色模式',
        subtitle: controller.themeMode == ThemeMode.dark ? '已开启' : '已关闭',
        icon: controller.themeMode == ThemeMode.dark
            ? Icons.dark_mode_rounded
            : Icons.light_mode_outlined,
        active: controller.themeMode == ThemeMode.dark,
        busy: false,
        onPressed: () => controller.setThemeMode(
          controller.themeMode == ThemeMode.dark
              ? ThemeMode.light
              : ThemeMode.dark,
        ),
      );
      final motion = _QuickTile(
        label: '减弱动效',
        subtitle: controller.reduceMotion ? '已开启' : '完整动效',
        icon: Icons.animation_rounded,
        active: controller.reduceMotion,
        busy: false,
        onPressed: () => controller.setReduceMotion(!controller.reduceMotion),
      );

      final pods = [wifi, bluetooth, dnd, screenshot, darkTheme, motion];

      if (constraints.maxWidth < 280) {
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            for (var i = 0; i < pods.length; i++) ...[
              if (i > 0) const SizedBox(height: 10),
              pods[i],
            ],
          ],
        );
      }
      return Column(
        children: [
          Row(
            children: [
              Expanded(child: wifi),
              const SizedBox(width: 10),
              Expanded(child: bluetooth),
            ],
          ),
          const SizedBox(height: 10),
          Row(
            children: [
              Expanded(child: dnd),
              const SizedBox(width: 10),
              Expanded(child: screenshot),
            ],
          ),
          const SizedBox(height: 10),
          Row(
            children: [
              Expanded(child: darkTheme),
              const SizedBox(width: 10),
              Expanded(child: motion),
            ],
          ),
        ],
      );
    },
  );
}

class _PreferenceSwitch extends StatelessWidget {
  const _PreferenceSwitch({
    required this.title,
    this.subtitle,
    required this.icon,
    required this.value,
    required this.onChanged,
  });
  final String title;
  final String? subtitle;
  final IconData icon;
  final bool value;
  final ValueChanged<bool> onChanged;
  @override
  Widget build(BuildContext context) => LayoutBuilder(
    builder: (context, constraints) => SwitchListTile(
      contentPadding: const EdgeInsets.symmetric(horizontal: 8),
      secondary: constraints.maxWidth > 280 ? Icon(icon) : null,
      title: Text(title),
      subtitle: subtitle == null ? null : Text(subtitle!),
      value: value,
      onChanged: onChanged,
    ),
  );
}

class _QuickTile extends StatelessWidget {
  const _QuickTile({
    required this.label,
    required this.subtitle,
    required this.icon,
    required this.active,
    required this.busy,
    this.onPressed,
  });
  final String label;
  final String subtitle;
  final IconData icon;
  final bool active;
  final bool busy;
  final VoidCallback? onPressed;

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    final enabled = onPressed != null;
    final foreground = active
        ? colors.onPrimaryContainer
        : colors.onSurfaceVariant;
    return Semantics(
      toggled: active,
      enabled: enabled && !busy,
      label: '$label，$subtitle${busy ? '，正在处理' : ''}',
      child: Material(
        color: active
            ? colors.primaryContainer
            : colors.surfaceContainerHighest,
        borderRadius: BorderRadius.circular(16),
        child: InkWell(
          onTap: busy ? null : onPressed,
          borderRadius: BorderRadius.circular(16),
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 12),
            child: Row(
              children: [
                Container(
                  height: 36,
                  width: 36,
                  decoration: BoxDecoration(
                    color: active
                        ? colors.primary.withValues(alpha: 0.18)
                        : colors.surface.withValues(alpha: 0.6),
                    shape: BoxShape.circle,
                  ),
                  child: Center(
                    child: busy
                        ? SizedBox(
                            width: 18,
                            height: 18,
                            child: CircularProgressIndicator(
                              strokeWidth: 2,
                              color: foreground,
                            ),
                          )
                        : Icon(icon, color: foreground, size: 20),
                  ),
                ),
                const SizedBox(width: 12),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Text(
                        label,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: Theme.of(context).textTheme.titleSmall?.copyWith(
                          color: foreground,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                      const SizedBox(height: 2),
                      Tooltip(
                        message: subtitle,
                        child: Text(
                          busy ? '正在处理…' : subtitle,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: Theme.of(context).textTheme.bodySmall
                              ?.copyWith(
                                color: foreground.withValues(alpha: 0.85),
                              ),
                        ),
                      ),
                    ],
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}
