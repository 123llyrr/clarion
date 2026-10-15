// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';
import '../models.dart';
import '../state/shell_controller.dart';
import 'panel_components.dart';

class NotificationsPanel extends StatelessWidget {
  const NotificationsPanel({super.key, required this.controller});
  final ShellController controller;

  @override
  Widget build(BuildContext context) {
    final size = MediaQuery.sizeOf(context);
    final compact = size.height < 400 || size.width < 280;
    final content = Padding(
      padding: const EdgeInsets.all(20),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          PanelHeading(
            title: '通知',
            onClose: controller.closePanel,
            actions: [
              IconButton(
                tooltip: '清除全部通知',
                onPressed: controller.notifications.isEmpty
                    ? null
                    : controller.clearNotifications,
                icon: const Icon(Icons.done_all_rounded),
              ),
            ],
          ),
          SwitchListTile(
            contentPadding: EdgeInsets.zero,
            title: const Text('勿扰模式'),
            subtitle: const Text('暂停通知计数提醒，保留历史'),
            value: controller.doNotDisturb,
            onChanged: controller.setDoNotDisturb,
          ),
          if (!controller.notificationServerAvailable)
            const ShellStatusMessage(text: '通知服务未启用或由其他桌面组件提供'),
          const SizedBox(height: 12),
          PanelBody(
            scrollWhole: compact,
            child: controller.notifications.isEmpty
                ? Padding(
                    padding: const EdgeInsets.symmetric(vertical: 48),
                    child: Column(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        Icon(
                          Icons.notifications_none_rounded,
                          size: 48,
                          color: Theme.of(context).colorScheme.onSurfaceVariant,
                        ),
                        const SizedBox(height: 16),
                        Text(
                          '全部看完了',
                          style: Theme.of(context).textTheme.titleMedium,
                        ),
                        const SizedBox(height: 6),
                        const Text('新的通知会显示在这里'),
                      ],
                    ),
                  )
                : ListView.separated(
                    shrinkWrap: true,
                    physics: compact
                        ? const NeverScrollableScrollPhysics()
                        : null,
                    itemCount: controller.notifications.length,
                    separatorBuilder: (_, _) => const SizedBox(height: 12),
                    itemBuilder: (context, index) => _NotificationCard(
                      notification: controller.notifications[index],
                      controller: controller,
                    ),
                  ),
          ),
          if (controller.error != null)
            ShellStatusMessage(text: controller.error!, error: true),
        ],
      ),
    );
    return compact ? SingleChildScrollView(child: content) : content;
  }
}

class _NotificationCard extends StatelessWidget {
  const _NotificationCard({
    required this.notification,
    required this.controller,
  });
  final ShellNotification notification;
  final ShellController controller;

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    final timestamp =
        '${notification.timestamp.hour.toString().padLeft(2, '0')}:${notification.timestamp.minute.toString().padLeft(2, '0')}';
    final hasDefault = notification.actions.any(
      (action) => action.key == 'default',
    );
    final named = notification.actions
        .where((action) => action.key != 'default')
        .toList();
    return Card(
      key: ValueKey('notification-${notification.id}'),
      child: InkWell(
        borderRadius: BorderRadius.circular(20),
        onTap: hasDefault
            ? () => controller.invokeNotificationAction(
                notification.id,
                'default',
                expected: notification,
              )
            : null,
        child: Padding(
          padding: const EdgeInsets.fromLTRB(16, 8, 8, 16),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              LayoutBuilder(
                builder: (context, constraints) {
                  final close = IconButton(
                    tooltip: '关闭通知',
                    onPressed: () => controller.dismissNotification(
                      notification.id,
                      expected: notification,
                    ),
                    icon: const Icon(Icons.close_rounded, size: 20),
                  );
                  final name = Text(
                    notification.appName,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: Theme.of(context).textTheme.labelLarge,
                  );
                  if (constraints.maxWidth < 240) {
                    return Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Row(
                          children: [
                            Expanded(child: name),
                            close,
                          ],
                        ),
                        Text(
                          timestamp,
                          style: Theme.of(context).textTheme.labelSmall,
                        ),
                        const SizedBox(height: 8),
                      ],
                    );
                  }
                  return Row(
                    children: [
                      Icon(
                        notification.urgency == 2
                            ? Icons.priority_high_rounded
                            : Icons.notifications_outlined,
                        size: 18,
                        color: notification.urgency == 2
                            ? colors.error
                            : colors.primary,
                      ),
                      const SizedBox(width: 8),
                      Expanded(
                        child: Text(
                          notification.appName,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: Theme.of(context).textTheme.labelLarge,
                        ),
                      ),
                      Text(
                        timestamp,
                        style: Theme.of(context).textTheme.labelSmall,
                      ),
                      close,
                    ],
                  );
                },
              ),
              Text(
                notification.summary,
                style: Theme.of(context).textTheme.titleSmall,
              ),
              if (notification.body.isNotEmpty) ...[
                const SizedBox(height: 6),
                Text(
                  notification.body,
                  style: Theme.of(context).textTheme.bodyMedium?.copyWith(
                    color: colors.onSurfaceVariant,
                  ),
                ),
              ],
              if (named.isNotEmpty) ...[
                const SizedBox(height: 12),
                Wrap(
                  spacing: 8,
                  runSpacing: 4,
                  children: named
                      .map(
                        (action) => TextButton(
                          onPressed: () => controller.invokeNotificationAction(
                            notification.id,
                            action.key,
                            expected: notification,
                          ),
                          child: Text(
                            action.label.isEmpty ? action.key : action.label,
                          ),
                        ),
                      )
                      .toList(),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}
