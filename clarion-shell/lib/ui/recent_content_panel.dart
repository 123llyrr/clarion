// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../recent_content_models.dart';

enum RecentContentFilter { all, text, images, screenshots }

/// The host owns collecting and copying data. This view only displays supplied
/// metadata; it never reads the clipboard, files, or the compositor itself.
class RecentContentPanel extends StatefulWidget {
  const RecentContentPanel({
    super.key,
    required this.entries,
    required this.onCopy,
    required this.onPin,
    required this.onDelete,
    required this.onClear,
    required this.onClose,
    this.onOpenScreenshot,
    this.onQueryChanged,
    this.onFilterChanged,
    this.onTogglePaused,
    this.onReconnect,
    this.query = '',
    this.clipboardAvailable = true,
    this.loading = false,
    this.paused = false,
    this.error,
    this.busyEntryIds = const {},
    this.sessionOnly = true,
    this.macos = false,
    this.now,
  });

  final List<RecentContentEntry> entries;
  final ValueChanged<RecentContentEntry> onCopy, onPin, onDelete;

  /// Clears unpinned history only. Source files must remain untouched.
  final VoidCallback onClear, onClose;
  final ValueChanged<RecentContentEntry>? onOpenScreenshot;
  final ValueChanged<String>? onQueryChanged;
  final ValueChanged<RecentContentFilter>? onFilterChanged;
  final VoidCallback? onTogglePaused;
  final VoidCallback? onReconnect;
  final String query;
  final bool clipboardAvailable, loading, paused, sessionOnly, macos;
  final String? error;
  final Set<String> busyEntryIds;
  final DateTime? now;

  @override
  State<RecentContentPanel> createState() => _RecentContentPanelState();
}

class _RecentContentPanelState extends State<RecentContentPanel> {
  late final TextEditingController _search;
  final _searchFocus = FocusNode(debugLabel: 'recent-content-search');
  RecentContentFilter _filter = RecentContentFilter.all;

  @override
  void initState() {
    super.initState();
    _search = TextEditingController(text: widget.query);
  }

  @override
  void didUpdateWidget(covariant RecentContentPanel oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.query != oldWidget.query && widget.query != _search.text) {
      _search.value = TextEditingValue(
        text: widget.query,
        selection: TextSelection.collapsed(offset: widget.query.length),
      );
    }
  }

  @override
  void dispose() {
    _search.dispose();
    _searchFocus.dispose();
    super.dispose();
  }

  void _queryChanged(String value) {
    setState(() {});
    widget.onQueryChanged?.call(value);
  }

  List<RecentContentEntry> get _visibleEntries {
    final query = _search.text.trim().toLowerCase();
    final entries = widget.entries.where((entry) {
      final matchesKind = switch (_filter) {
        RecentContentFilter.all => true,
        RecentContentFilter.text => entry.kind == RecentContentKind.text,
        RecentContentFilter.images => entry.kind != RecentContentKind.text,
        RecentContentFilter.screenshots =>
          entry.kind == RecentContentKind.screenshot,
      };
      final haystack =
          '${entry.text}\n${entry.source ?? ''}\n'
                  '${entry.filePath ?? ''}\n${entry.capturedAt.toLocal().toIso8601String()}'
              .toLowerCase();
      return matchesKind && (query.isEmpty || haystack.contains(query));
    }).toList();
    entries.sort((a, b) {
      final pinned = (b.isPinned ? 1 : 0).compareTo(a.isPinned ? 1 : 0);
      if (pinned != 0) return pinned;
      final date = b.capturedAt.compareTo(a.capturedAt);
      return date != 0 ? date : a.id.compareTo(b.id);
    });
    return entries;
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final colors = theme.colorScheme;
    final entries = _visibleEntries;
    return CallbackShortcuts(
      bindings: {
        const SingleActivator(LogicalKeyboardKey.escape): widget.onClose,
        const SingleActivator(LogicalKeyboardKey.keyF, control: true):
            _searchFocus.requestFocus,
      },
      child: FocusScope(
        autofocus: true,
        child: Material(
          key: const ValueKey('recent-content-panel'),
          color: Colors.transparent,
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxHeight: 640),
            child: CustomScrollView(
              key: const ValueKey('recent-content-list'),
              shrinkWrap: true,
              slivers: [
                SliverPadding(
                  padding: const EdgeInsets.fromLTRB(16, 16, 16, 0),
                  sliver: SliverToBoxAdapter(
                    child: Column(
                      mainAxisSize: MainAxisSize.min,
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        Row(
                          children: [
                            Expanded(
                              child: Text(
                                '最近内容',
                                style: theme.textTheme.titleLarge?.copyWith(
                                  fontSize: 20,
                                  fontWeight: FontWeight.w600,
                                ),
                              ),
                            ),
                            if (widget.onTogglePaused != null)
                              _smallButton(
                                key: const ValueKey('recent-content-pause'),
                                label: widget.paused ? '恢复记录' : '暂停记录',
                                icon: widget.paused
                                    ? Icons.play_circle_outline_rounded
                                    : Icons.pause_circle_outline_rounded,
                                selected: widget.paused,
                                onPressed: widget.onTogglePaused,
                              ),
                            _smallButton(
                              key: const ValueKey('recent-content-clear'),
                              label: '清空未置顶记录，不删除原文件',
                              icon: Icons.delete_sweep_outlined,
                              onPressed:
                                  widget.entries.any((entry) => !entry.isPinned)
                                  ? widget.onClear
                                  : null,
                            ),
                            _smallButton(
                              key: const ValueKey('recent-content-close'),
                              label: '关闭最近内容',
                              icon: Icons.close_rounded,
                              onPressed: widget.onClose,
                            ),
                          ],
                        ),
                        const SizedBox(height: 12),
                        TextField(
                          key: const ValueKey('recent-content-search'),
                          controller: _search,
                          focusNode: _searchFocus,
                          autofocus: true,
                          onChanged: _queryChanged,
                          decoration: InputDecoration(
                            hintText: '搜索文字、来源或日期',
                            prefixIcon: const Icon(
                              Icons.search_rounded,
                              size: 19,
                            ),
                            isDense: true,
                            filled: true,
                            fillColor: colors.surfaceContainerHighest
                                .withValues(alpha: .65),
                            border: OutlineInputBorder(
                              borderRadius: BorderRadius.circular(
                                widget.macos ? 8 : 14,
                              ),
                              borderSide: BorderSide.none,
                            ),
                            suffixIcon: _search.text.isEmpty
                                ? null
                                : IconButton(
                                    tooltip: '清除搜索',
                                    onPressed: () {
                                      _search.clear();
                                      _queryChanged('');
                                    },
                                    icon: const Icon(
                                      Icons.close_rounded,
                                      size: 17,
                                    ),
                                  ),
                          ),
                        ),
                        const SizedBox(height: 10),
                        SingleChildScrollView(
                          scrollDirection: Axis.horizontal,
                          child: Row(
                            children: [
                              for (final filter in RecentContentFilter.values)
                                Padding(
                                  padding: const EdgeInsets.only(right: 6),
                                  child: ChoiceChip(
                                    key: ValueKey(
                                      'recent-filter-${filter.name}',
                                    ),
                                    label: Text(switch (filter) {
                                      RecentContentFilter.all => '全部',
                                      RecentContentFilter.text => '文本',
                                      RecentContentFilter.images => '图片',
                                      RecentContentFilter.screenshots => '截图',
                                    }),
                                    selected: filter == _filter,
                                    showCheckmark: false,
                                    onSelected: (_) {
                                      setState(() => _filter = filter);
                                      widget.onFilterChanged?.call(filter);
                                    },
                                  ),
                                ),
                            ],
                          ),
                        ),
                        if (widget.loading) ...[
                          const SizedBox(height: 8),
                          const LinearProgressIndicator(minHeight: 2),
                        ],
                        if (widget.paused || !widget.clipboardAvailable) ...[
                          const SizedBox(height: 8),
                          _StatusLine(
                            icon: widget.paused
                                ? Icons.pause_rounded
                                : Icons.link_off_rounded,
                            text: widget.paused
                                ? '已暂停记录，已有内容仍可使用。'
                                : '剪贴板监听未连接，已有记录与最近截图仍会保留。',
                          ),
                        ],
                        if (widget.error case final error?) ...[
                          const SizedBox(height: 8),
                          _StatusLine(
                            icon: Icons.error_outline_rounded,
                            text: error,
                            error: true,
                          ),
                        ],
                        const SizedBox(height: 10),
                      ],
                    ),
                  ),
                ),
                SliverPadding(
                  padding: const EdgeInsets.symmetric(horizontal: 16),
                  sliver: entries.isEmpty
                      ? SliverToBoxAdapter(child: _emptyState(context))
                      : SliverList.separated(
                          itemCount: entries.length,
                          separatorBuilder: (_, _) => const SizedBox(height: 8),
                          itemBuilder: (context, index) => _EntryCard(
                            entry: entries[index],
                            macos: widget.macos,
                            now: widget.now ?? DateTime.now(),
                            busy: widget.busyEntryIds.contains(
                              entries[index].id,
                            ),
                            copyAvailable: widget.clipboardAvailable,
                            onCopy: widget.onCopy,
                            onPin: widget.onPin,
                            onDelete: widget.onDelete,
                            onOpen: widget.onOpenScreenshot,
                          ),
                        ),
                ),
                SliverPadding(
                  padding: const EdgeInsets.fromLTRB(16, 0, 16, 16),
                  sliver: SliverToBoxAdapter(
                    child: Column(
                      mainAxisSize: MainAxisSize.min,
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        const SizedBox(height: 10),
                        Text(
                          widget.sessionOnly
                              ? '仅保留本次会话的历史。清空未置顶记录不会删除原文件。'
                              : '清空仅移除未置顶的历史记录，不会删除原文件。',
                          key: const ValueKey('recent-content-retention'),
                          style: theme.textTheme.bodySmall?.copyWith(
                            color: colors.onSurfaceVariant,
                            fontSize: 11,
                          ),
                        ),
                        if (!widget.clipboardAvailable &&
                            widget.onReconnect != null)
                          Align(
                            alignment: Alignment.centerRight,
                            child: TextButton.icon(
                              key: const ValueKey('recent-content-reconnect'),
                              onPressed: widget.loading
                                  ? null
                                  : widget.onReconnect,
                              icon: const Icon(Icons.refresh_rounded, size: 16),
                              label: const Text('重新连接'),
                            ),
                          ),
                      ],
                    ),
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }

  Widget _emptyState(BuildContext context) {
    final searching = _search.text.trim().isNotEmpty;
    final title = searching
        ? '没有匹配的内容'
        : _filter == RecentContentFilter.screenshots
        ? '还没有最近截图'
        : '还没有最近内容';
    return SingleChildScrollView(
      child: Padding(
        padding: const EdgeInsets.symmetric(vertical: 30, horizontal: 12),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(
              searching
                  ? Icons.search_off_rounded
                  : Icons.content_paste_rounded,
              size: 34,
              color: Theme.of(context).colorScheme.onSurfaceVariant,
            ),
            const SizedBox(height: 12),
            Text(title, style: Theme.of(context).textTheme.titleSmall),
            const SizedBox(height: 6),
            Text(
              searching ? '换个关键词，或切换内容类型。' : '复制文字、图片或完成截图后，可在这里再次使用。',
              textAlign: TextAlign.center,
              style: Theme.of(context).textTheme.bodySmall,
            ),
          ],
        ),
      ),
    );
  }
}

Widget _smallButton({
  Key? key,
  required String label,
  required IconData icon,
  required VoidCallback? onPressed,
  bool selected = false,
}) => IconButton(
  key: key,
  tooltip: label,
  onPressed: onPressed,
  isSelected: selected,
  padding: const EdgeInsets.all(6),
  constraints: const BoxConstraints.tightFor(width: 32, height: 32),
  iconSize: 18,
  visualDensity: VisualDensity.compact,
  icon: Icon(icon),
);

class _StatusLine extends StatelessWidget {
  const _StatusLine({
    required this.icon,
    required this.text,
    this.error = false,
  });
  final IconData icon;
  final String text;
  final bool error;
  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    final color = error ? colors.error : colors.onSurfaceVariant;
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Icon(icon, size: 15, color: color),
        const SizedBox(width: 6),
        Expanded(
          child: Text(text, style: TextStyle(fontSize: 11.5, color: color)),
        ),
      ],
    );
  }
}

class _EntryCard extends StatelessWidget {
  const _EntryCard({
    required this.entry,
    required this.macos,
    required this.now,
    required this.busy,
    required this.copyAvailable,
    required this.onCopy,
    required this.onPin,
    required this.onDelete,
    this.onOpen,
  });
  final RecentContentEntry entry;
  final bool macos, busy, copyAvailable;
  final DateTime now;
  final ValueChanged<RecentContentEntry> onCopy, onPin, onDelete;
  final ValueChanged<RecentContentEntry>? onOpen;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final colors = theme.colorScheme;
    final label = switch (entry.kind) {
      RecentContentKind.text => '文字',
      RecentContentKind.image => '图片',
      RecentContentKind.screenshot => '截图',
    };
    return Card(
      key: ValueKey('recent-entry-${entry.id}'),
      margin: EdgeInsets.zero,
      elevation: 0,
      color: colors.surfaceContainerHigh.withValues(alpha: macos ? .65 : 1),
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(macos ? 10 : 16),
        side: BorderSide(
          color: entry.isPinned
              ? colors.primary.withValues(alpha: .45)
              : colors.outlineVariant.withValues(alpha: .5),
        ),
      ),
      clipBehavior: Clip.antiAlias,
      child: Padding(
        padding: const EdgeInsets.fromLTRB(12, 8, 8, 6),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Row(
              children: [
                Icon(
                  entry.kind == RecentContentKind.text
                      ? Icons.notes_rounded
                      : entry.kind == RecentContentKind.screenshot
                      ? Icons.screenshot_monitor_rounded
                      : Icons.image_outlined,
                  size: 15,
                  color: colors.onSurfaceVariant,
                ),
                const SizedBox(width: 6),
                Expanded(
                  child: Text(
                    entry.source?.isNotEmpty == true
                        ? '$label · ${entry.source}'
                        : label,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: theme.textTheme.labelSmall?.copyWith(
                      color: colors.onSurfaceVariant,
                    ),
                  ),
                ),
                const SizedBox(width: 6),
                Text(
                  _timeLabel(entry.capturedAt, now),
                  style: theme.textTheme.labelSmall?.copyWith(
                    color: colors.onSurfaceVariant,
                    fontSize: 10,
                  ),
                ),
                _smallButton(
                  key: ValueKey('recent-pin-${entry.id}'),
                  label: entry.isPinned ? '取消置顶' : '置顶记录',
                  icon: entry.isPinned
                      ? Icons.push_pin_rounded
                      : Icons.push_pin_outlined,
                  selected: entry.isPinned,
                  onPressed: busy ? null : () => onPin(entry),
                ),
              ],
            ),
            if (entry.kind == RecentContentKind.text)
              Padding(
                padding: const EdgeInsets.only(top: 5, bottom: 8),
                child: Text(
                  entry.text,
                  maxLines: 4,
                  overflow: TextOverflow.ellipsis,
                  style: theme.textTheme.bodyMedium?.copyWith(height: 1.4),
                ),
              )
            else
              Padding(
                padding: const EdgeInsets.only(top: 6, right: 4, bottom: 8),
                child: ClipRRect(
                  borderRadius: BorderRadius.circular(6),
                  child: ColoredBox(
                    color: colors.surfaceContainerHighest,
                    child: SizedBox(
                      height: 112,
                      child: entry.thumbnail == null
                          ? const Center(
                              child: Icon(Icons.image_outlined, size: 32),
                            )
                          : Image.memory(
                              entry.thumbnail!,
                              fit: BoxFit.contain,
                              semanticLabel: '$label缩略图',
                              gaplessPlayback: true,
                              errorBuilder: (_, _, _) => const Center(
                                child: Icon(
                                  Icons.broken_image_outlined,
                                  size: 32,
                                ),
                              ),
                            ),
                    ),
                  ),
                ),
              ),
            Row(
              children: [
                TextButton.icon(
                  key: ValueKey('recent-copy-${entry.id}'),
                  onPressed: busy || !copyAvailable
                      ? null
                      : () => onCopy(entry),
                  icon: const Icon(Icons.copy_rounded, size: 15),
                  label: const Text('复制'),
                ),
                if (entry.kind == RecentContentKind.screenshot &&
                    entry.filePath != null &&
                    onOpen != null)
                  TextButton.icon(
                    key: ValueKey('recent-open-${entry.id}'),
                    onPressed: busy ? null : () => onOpen!(entry),
                    icon: const Icon(Icons.open_in_new_rounded, size: 15),
                    label: const Text('打开'),
                  ),
                const Spacer(),
                if (busy)
                  const Padding(
                    padding: EdgeInsets.only(right: 8),
                    child: SizedBox.square(
                      dimension: 14,
                      child: CircularProgressIndicator(strokeWidth: 2),
                    ),
                  ),
                _smallButton(
                  key: ValueKey('recent-delete-${entry.id}'),
                  label: '从历史中移除，不删除原文件',
                  icon: Icons.close_rounded,
                  onPressed: busy ? null : () => onDelete(entry),
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }
}

String _timeLabel(DateTime date, DateTime now) {
  final minutes = now.difference(date).inMinutes;
  if (minutes < 1) return '刚刚';
  if (minutes < 60) return '$minutes 分钟前';
  final local = date.toLocal();
  final time =
      '${local.hour.toString().padLeft(2, '0')}:${local.minute.toString().padLeft(2, '0')}';
  final today = now.toLocal();
  return local.year == today.year &&
          local.month == today.month &&
          local.day == today.day
      ? time
      : '${local.month}月${local.day}日 $time';
}

/// The bar never reveals copied text or image previews.
class RecentContentBarButton extends StatelessWidget {
  const RecentContentBarButton({
    super.key,
    required this.count,
    required this.onPressed,
    this.selected = false,
    this.paused = false,
    this.macos = false,
    this.compact = true,
    this.tooltipEnabled = true,
  });
  final int count;
  final VoidCallback? onPressed;
  final bool selected, paused, macos, compact, tooltipEnabled;
  @override
  Widget build(BuildContext context) {
    final title = count > 0 ? '最近内容，$count 项' : '最近内容';
    final label = paused ? '$title，已暂停记录' : title;
    final child = Semantics(
      button: true,
      selected: selected,
      label: label,
      child: Material(
        color: selected
            ? Theme.of(context).colorScheme.primary.withValues(alpha: .15)
            : Colors.transparent,
        borderRadius: BorderRadius.circular(macos ? 5 : 18),
        child: InkWell(
          onTap: onPressed,
          borderRadius: BorderRadius.circular(macos ? 5 : 18),
          child: Padding(
            padding: EdgeInsets.symmetric(
              horizontal: compact ? 7 : 10,
              vertical: macos ? 1 : 7,
            ),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Icon(
                  paused
                      ? Icons.pause_circle_outline_rounded
                      : Icons.content_paste_rounded,
                  size: 17,
                ),
                if (!compact) ...[const SizedBox(width: 6), const Text('最近内容')],
                if (count > 0) ...[
                  const SizedBox(width: 5),
                  Flexible(
                    child: FittedBox(
                      fit: BoxFit.scaleDown,
                      child: Text(
                        count > 99 ? '99+' : '$count',
                        style: Theme.of(context).textTheme.labelSmall,
                      ),
                    ),
                  ),
                ],
              ],
            ),
          ),
        ),
      ),
    );
    return tooltipEnabled
        ? Tooltip(message: label, excludeFromSemantics: true, child: child)
        : child;
  }
}
