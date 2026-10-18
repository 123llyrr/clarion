// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:io';

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
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: widget.controller,
    builder: (context, _) => _content(context),
  );

  Widget _content(BuildContext context) {
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
          if (controller.lastScreenshotAt != null) ...[
            const Divider(height: 28),
            Text('最近截图', style: Theme.of(context).textTheme.titleSmall),
            const SizedBox(height: 10),
            if (controller.lastScreenshotPath case final path?)
              Card(
                clipBehavior: Clip.antiAlias,
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    ListTile(
                      key: const ValueKey('screenshot-last-file'),
                      leading: ClipRRect(
                        borderRadius: BorderRadius.circular(6),
                        child: Image.file(
                          File(path),
                          width: 64,
                          height: 48,
                          cacheWidth: 176,
                          fit: BoxFit.contain,
                          semanticLabel: '最近截图预览',
                          errorBuilder: (_, _, _) => const SizedBox(
                            width: 64,
                            height: 48,
                            child: Icon(Icons.image_outlined),
                          ),
                        ),
                      ),
                      title: Text(
                        File(path).uri.pathSegments.last,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                      ),
                      subtitle: const Text('已保存并复制到剪贴板'),
                      onTap: controller.busy.contains('screenshot-open')
                          ? null
                          : () => controller.openLastScreenshot(),
                    ),
                    Padding(
                      padding: const EdgeInsets.fromLTRB(8, 0, 8, 8),
                      child: Wrap(
                        alignment: WrapAlignment.end,
                        spacing: 8,
                        children: [
                          TextButton.icon(
                            key: const ValueKey('screenshot-open-folder'),
                            onPressed:
                                controller.busy.contains('screenshot-open')
                                ? null
                                : () => controller.openLastScreenshot(
                                    containingFolder: true,
                                  ),
                            icon: const Icon(
                              Icons.folder_open_outlined,
                              size: 18,
                            ),
                            label: const Text('打开所在文件夹'),
                          ),
                          FilledButton.tonal(
                            key: const ValueKey('screenshot-open-image'),
                            onPressed:
                                controller.busy.contains('screenshot-open')
                                ? null
                                : () => controller.openLastScreenshot(),
                            child: const Text('打开图片'),
                          ),
                        ],
                      ),
                    ),
                  ],
                ),
              )
            else
              const ShellStatusMessage(text: '已复制到剪贴板，可以直接粘贴'),
          ],
          if (!controller.connected)
            const ShellStatusMessage(text: '桌面连接暂时不可用，请稍后重试。'),
          if (controller.error != null)
            ShellStatusMessage(text: controller.error!, error: true),
        ],
      ),
    );
  }
}
