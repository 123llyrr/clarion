// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';

class PanelHeading extends StatelessWidget {
  const PanelHeading({
    super.key,
    required this.title,
    required this.onClose,
    this.actions = const [],
  });
  final String title;
  final VoidCallback onClose;
  final List<Widget> actions;
  @override
  Widget build(BuildContext context) => LayoutBuilder(
    builder: (context, constraints) {
      final narrow = constraints.maxWidth < 280;
      final heading = Row(
        children: [
          Expanded(
            child: Text(
              title,
              style: Theme.of(context).textTheme.headlineSmall,
            ),
          ),
          if (!narrow) ...actions,
          IconButton(
            tooltip: '关闭',
            onPressed: onClose,
            icon: const Icon(Icons.close_rounded),
          ),
        ],
      );
      return narrow && actions.isNotEmpty
          ? Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                heading,
                Align(
                  alignment: Alignment.centerRight,
                  child: Wrap(children: actions),
                ),
              ],
            )
          : heading;
    },
  );
}

/// Very short outputs scroll the whole panel; normal outputs keep its header.
class PanelBody extends StatelessWidget {
  const PanelBody({super.key, required this.child, required this.scrollWhole});
  final Widget child;
  final bool scrollWhole;
  @override
  Widget build(BuildContext context) =>
      scrollWhole ? child : Flexible(child: child);
}

class ShellStatusMessage extends StatelessWidget {
  const ShellStatusMessage({super.key, required this.text, this.error = false});
  final String text;
  final bool error;
  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    return Container(
      margin: const EdgeInsets.only(top: 12),
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: error ? colors.errorContainer : colors.surfaceContainerHighest,
        borderRadius: BorderRadius.circular(16),
      ),
      child: Row(
        children: [
          Icon(
            error ? Icons.error_outline_rounded : Icons.info_outline_rounded,
            color: error ? colors.onErrorContainer : colors.onSurfaceVariant,
          ),
          const SizedBox(width: 12),
          Expanded(
            child: Text(
              text,
              style: TextStyle(
                color: error
                    ? colors.onErrorContainer
                    : colors.onSurfaceVariant,
              ),
            ),
          ),
        ],
      ),
    );
  }
}
