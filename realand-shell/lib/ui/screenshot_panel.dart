// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';

import '../models.dart';
import '../state/shell_controller.dart';
import 'panel_components.dart';

class ScreenshotPanel extends StatefulWidget {
  const ScreenshotPanel({super.key, required this.controller});
  final ShellController controller;

  @override
  State<ScreenshotPanel> createState() => _ScreenshotPanelState();
}

class _ScreenshotPanelState extends State<ScreenshotPanel> {
  bool _showPointer = false;

  @override
  Widget build(BuildContext context) {
    final controller = widget.controller;
    final busy = controller.busy.contains('screenshot');
    final enabled = controller.connected && !busy;
    return SingleChildScrollView(
      padding: const EdgeInsets.all(20),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        mainAxisSize: MainAxisSize.min,
        children: [
          PanelHeading(title: '截图', onClose: controller.closePanel),
          const SizedBox(height: 16),
          Text('选择截图方式', style: Theme.of(context).textTheme.titleMedium),
          const SizedBox(height: 12),
          for (final mode in ScreenshotMode.values)
            Padding(
              padding: const EdgeInsets.only(bottom: 8),
              child: Card(
                clipBehavior: Clip.antiAlias,
                child: ListTile(
                  key: ValueKey('screenshot-${mode.name}'),
                  leading: Icon(switch (mode) {
                    ScreenshotMode.region => Icons.crop_free_rounded,
                    ScreenshotMode.screen => Icons.desktop_windows_outlined,
                    ScreenshotMode.window => Icons.web_asset_rounded,
                  }),
                  title: Text(switch (mode) {
                    ScreenshotMode.region => '选取区域',
                    ScreenshotMode.screen => '当前屏幕',
                    ScreenshotMode.window => '当前窗口',
                  }),
                  subtitle: Text(switch (mode) {
                    ScreenshotMode.region => '拖动选区，Esc 取消',
                    ScreenshotMode.screen => '截取当前显示器',
                    ScreenshotMode.window => '截取当前应用窗口',
                  }),
                  trailing: const Icon(Icons.chevron_right_rounded),
                  enabled: enabled,
                  onTap: enabled
                      ? () => controller.takeScreenshot(
                          mode,
                          showPointer: _showPointer,
                        )
                      : null,
                ),
              ),
            ),
          SwitchListTile(
            key: const ValueKey('screenshot-pointer'),
            contentPadding: const EdgeInsets.symmetric(horizontal: 8),
            title: const Text('包含鼠标指针'),
            value: _showPointer,
            onChanged: busy
                ? null
                : (value) => setState(() => _showPointer = value),
          ),
          const SizedBox(height: 12),
          Text(
            controller.demo ? '演示预览 · 不执行截图' : '按你的截图设置保存，并复制到剪贴板',
            style: Theme.of(context).textTheme.bodySmall,
          ),
          if (!controller.connected)
            const ShellStatusMessage(text: '桌面连接暂时不可用，请稍后重试。'),
          if (controller.error != null)
            ShellStatusMessage(text: controller.error!, error: true),
        ],
      ),
    );
  }
}
