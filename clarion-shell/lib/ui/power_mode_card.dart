// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';

import 'package:flutter/material.dart';
import '../services/power_profiles_service.dart';

class PowerModeCard extends StatelessWidget {
  const PowerModeCard({super.key, required this.service});
  final PowerProfilesService service;

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: service,
    builder: (context, _) {
      final theme = Theme.of(context);
      final colors = theme.colorScheme;
      final active = service.profiles
          .where((profile) => profile.id == service.activeProfile)
          .firstOrNull;
      return Card(
        key: const ValueKey('power-mode-card'),
        child: Padding(
          padding: const EdgeInsets.all(16),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Row(
                children: [
                  const Icon(Icons.battery_saver_outlined),
                  const SizedBox(width: 12),
                  Expanded(
                    child: Text('电源模式', style: theme.textTheme.titleSmall),
                  ),
                  if (service.loading || service.busy)
                    const SizedBox(
                      width: 18,
                      height: 18,
                      child: CircularProgressIndicator(strokeWidth: 2),
                    ),
                ],
              ),
              const SizedBox(height: 12),
              if (service.available) ...[
                Wrap(
                  spacing: 8,
                  runSpacing: 8,
                  children: [
                    for (final profile in service.profiles)
                      FilterChip(
                        key: ValueKey('power-mode-${profile.id}'),
                        label: Text(profile.label),
                        avatar: Icon(_icon(profile.id), size: 18),
                        selected: profile.id == service.activeProfile,
                        onSelected: service.busy || service.loading
                            ? null
                            : (_) => unawaited(service.setProfile(profile.id)),
                      ),
                  ],
                ),
                if (active != null) ...[
                  const SizedBox(height: 8),
                  Text(active.description, style: theme.textTheme.bodySmall),
                ],
              ] else if (service.loading)
                const Text('正在读取系统电源模式…')
              else
                Text('电源模式暂不可用', style: theme.textTheme.bodyMedium),
              if (service.degradedReason case final reason?) ...[
                const SizedBox(height: 12),
                _Message(text: reason, color: colors.onSurfaceVariant),
              ],
              if (service.error case final error?) ...[
                const SizedBox(height: 12),
                _Message(text: error, color: colors.error),
                Align(
                  alignment: Alignment.centerRight,
                  child: TextButton.icon(
                    key: const ValueKey('power-mode-retry'),
                    onPressed: service.busy || service.loading
                        ? null
                        : () => unawaited(service.refresh()),
                    icon: const Icon(Icons.refresh_rounded, size: 18),
                    label: const Text('重试'),
                  ),
                ),
              ],
            ],
          ),
        ),
      );
    },
  );
}

IconData _icon(String profile) => switch (profile) {
  'power-saver' => Icons.eco_outlined,
  'performance' => Icons.speed_rounded,
  _ => Icons.balance_rounded,
};

class _Message extends StatelessWidget {
  const _Message({required this.text, required this.color});
  final String text;
  final Color color;

  @override
  Widget build(BuildContext context) => Row(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      Icon(Icons.info_outline_rounded, color: color, size: 18),
      const SizedBox(width: 8),
      Expanded(
        child: Text(
          text,
          style: Theme.of(context).textTheme.bodySmall?.copyWith(color: color),
        ),
      ),
    ],
  );
}
