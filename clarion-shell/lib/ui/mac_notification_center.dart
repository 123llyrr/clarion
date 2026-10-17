// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';

import 'package:flutter/cupertino.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import '../theme/font_fallback.dart';

import '../mac_notification_view.dart';
import '../models.dart';
import '../state/shell_controller.dart';
import 'controller_selector.dart';
import 'mac_calendar_month.dart';
import 'retained_surface.dart';

/// Catalina's two separate views. Geometry and animation are Clarion choices;
/// the installer resources establish labels, not measured runtime pixels.
class MacNotificationCenter extends StatefulWidget {
  const MacNotificationCenter({super.key, required this.controller, this.now});
  final ShellController controller;
  final DateTime Function()? now;

  @override
  State<MacNotificationCenter> createState() => _MacNotificationCenterState();
}

class _MacNotificationCenterState extends State<MacNotificationCenter> {
  Timer? _clock;
  late DateTime _now;
  bool _pending = false;
  final _tabs = [FocusNode(), FocusNode()];

  @override
  void initState() {
    super.initState();
    _now = (widget.now ?? DateTime.now)().toLocal();
    _tickLater();
  }

  void _tickLater() {
    _clock = Timer(Duration(seconds: 60 - _now.second), () {
      if (!mounted) return;
      setState(() => _now = (widget.now ?? DateTime.now)().toLocal());
      _tickLater();
    });
  }

  @override
  void dispose() {
    _clock?.cancel();
    for (final node in _tabs) {
      node.dispose();
    }
    super.dispose();
  }

  Future<void> _run(
    Future<bool> Function() operation, {
    bool close = false,
  }) async {
    if (_pending || widget.controller.panel != ShellPanel.notifications) return;
    final controller = widget.controller;
    final revision = controller.panelRevision;
    setState(() => _pending = true);
    try {
      final success = await operation();
      if (mounted && success && close) {
        controller.closePanelAtRevision(revision);
      }
    } finally {
      if (mounted) setState(() => _pending = false);
    }
  }

  void _selectTab(int index) {
    widget.controller.setMacNotificationView(MacNotificationView.values[index]);
    _tabs[index].requestFocus();
  }

  @override
  Widget build(BuildContext context) => ControllerSelector(
    controller: widget.controller,
    select: () => (
      widget.controller.macNotificationView,
      widget.controller.notifications,
      widget.controller.doNotDisturb,
      widget.controller.notificationServerAvailable,
      widget.controller.busy.contains('notifications'),
      widget.controller.error,
    ),
    builder: (context, _) {
      final controller = widget.controller;
      final today = controller.macNotificationView == MacNotificationView.today;
      final dark = Theme.of(context).brightness == Brightness.dark;
      final disabled = _pending || controller.busy.contains('notifications');
      return Material(
        key: const ValueKey('mac-notification-center'),
        color: dark
            ? const Color(0xff252527).withValues(alpha: .96)
            : const Color(0xffeeeeef).withValues(alpha: .96),
        child: DecoratedBox(
          decoration: BoxDecoration(
            border: Border(
              left: BorderSide(
                color: dark ? Colors.white12 : Colors.black12,
                width: .5,
              ),
            ),
          ),
          child: FocusTraversalGroup(
            child: FocusScope(
              autofocus: true,
              child: Column(
                children: [
                  Padding(
                    padding: const EdgeInsets.fromLTRB(22, 12, 22, 13),
                    child: Row(
                      children: [
                        for (var index = 0; index < 2; index++)
                          Expanded(
                            child: CallbackShortcuts(
                              bindings: {
                                const SingleActivator(
                                  LogicalKeyboardKey.arrowLeft,
                                ): () =>
                                    _selectTab(0),
                                const SingleActivator(
                                  LogicalKeyboardKey.arrowRight,
                                ): () =>
                                    _selectTab(1),
                              },
                              child: Semantics(
                                selected:
                                    controller.macNotificationView.index ==
                                    index,
                                child: TextButton(
                                  key: ValueKey('mac-notification-tab-$index'),
                                  focusNode: _tabs[index],
                                  autofocus:
                                      controller.macNotificationView.index ==
                                      index,
                                  onPressed: () => _selectTab(index),
                                  style: TextButton.styleFrom(
                                    minimumSize: const Size(0, 25),
                                    tapTargetSize:
                                        MaterialTapTargetSize.shrinkWrap,
                                    padding: const EdgeInsets.symmetric(
                                      horizontal: 4,
                                    ),
                                    foregroundColor: dark
                                        ? Colors.white
                                        : Colors.black87,
                                    backgroundColor:
                                        controller.macNotificationView.index ==
                                            index
                                        ? (dark ? Colors.white24 : Colors.white)
                                        : (dark
                                              ? Colors.black12
                                              : Colors.black.withValues(
                                                  alpha: .06,
                                                )),
                                    shape: RoundedRectangleBorder(
                                      borderRadius: BorderRadius.circular(4),
                                    ),
                                    textStyle: const TextStyle(
                                      fontFamily: shellFontFamily,
                                      fontFamilyFallback: shellFontFallback,
                                      fontSize: 13,
                                    ),
                                  ),
                                  child: Text(index == 0 ? '今天' : '通知'),
                                ),
                              ),
                            ),
                          ),
                      ],
                    ),
                  ),
                  Expanded(
                    child: today
                        ? SingleChildScrollView(
                            key: const PageStorageKey(
                              'mac-notification-today-scroll',
                            ),
                            padding: const EdgeInsets.fromLTRB(20, 14, 20, 24),
                            child: Column(
                              crossAxisAlignment: CrossAxisAlignment.stretch,
                              children: [
                                Text(
                                  '${_now.month}月${_now.day}日',
                                  key: const ValueKey('mac-today-date'),
                                  style: const TextStyle(
                                    fontSize: 34,
                                    fontWeight: FontWeight.w300,
                                    height: 1.2,
                                  ),
                                ),
                                Text(
                                  const [
                                    '星期一',
                                    '星期二',
                                    '星期三',
                                    '星期四',
                                    '星期五',
                                    '星期六',
                                    '星期日',
                                  ][_now.weekday - 1],
                                  style: const TextStyle(
                                    fontSize: 22,
                                    fontWeight: FontWeight.w300,
                                  ),
                                ),
                                const SizedBox(height: 28),
                                _TodayCalendar(now: _now),
                              ],
                            ),
                          )
                        : ListView(
                            key: const PageStorageKey('mac-notification-list'),
                            padding: const EdgeInsets.fromLTRB(16, 0, 16, 18),
                            children: [
                              Row(
                                children: [
                                  const Expanded(
                                    child: Text(
                                      '勿扰模式',
                                      style: TextStyle(fontSize: 13),
                                    ),
                                  ),
                                  SizedBox(
                                    width: 64,
                                    height: 44,
                                    child: Center(
                                      child: SizedBox(
                                        height: 24,
                                        child: FittedBox(
                                          child: CupertinoSwitch(
                                            key: const ValueKey(
                                              'mac-notification-dnd',
                                            ),
                                            value: controller.doNotDisturb,
                                            activeTrackColor: const Color(
                                              0xff2684ee,
                                            ),
                                            onChanged:
                                                controller.setDoNotDisturb,
                                          ),
                                        ),
                                      ),
                                    ),
                                  ),
                                ],
                              ),
                              if (!controller.notificationServerAvailable)
                                const Padding(
                                  padding: EdgeInsets.symmetric(vertical: 12),
                                  child: Text(
                                    '通知服务未启用或由其他桌面组件提供',
                                    style: TextStyle(fontSize: 12),
                                  ),
                                ),
                              if (controller.notifications.isEmpty)
                                const Padding(
                                  padding: EdgeInsets.symmetric(vertical: 52),
                                  child: Center(
                                    child: Text(
                                      '没有新通知',
                                      style: TextStyle(
                                        fontSize: 18,
                                        fontWeight: FontWeight.w300,
                                      ),
                                    ),
                                  ),
                                ),
                              if (controller.notifications.isNotEmpty)
                                Align(
                                  alignment: Alignment.centerRight,
                                  child: TextButton(
                                    key: const ValueKey(
                                      'mac-notification-clear-all',
                                    ),
                                    onPressed: disabled
                                        ? null
                                        : () => _run(
                                            controller.clearNotifications,
                                          ),
                                    child: const Text(
                                      '清除全部',
                                      style: TextStyle(fontSize: 12),
                                    ),
                                  ),
                                ),
                              ..._notificationRows(context, disabled),
                              if (controller.error != null)
                                Padding(
                                  padding: const EdgeInsets.only(top: 10),
                                  child: Text(
                                    controller.error!,
                                    style: TextStyle(
                                      fontSize: 12,
                                      color: Theme.of(
                                        context,
                                      ).colorScheme.error,
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
    },
  );

  List<Widget> _notificationRows(BuildContext context, bool disabled) {
    final groups = <DateTime, List<ShellNotification>>{};
    final ordered = widget.controller.notifications.toList()
      ..sort((a, b) => b.timestamp.compareTo(a.timestamp));
    for (final item in ordered) {
      (groups[DateUtils.dateOnly(item.timestamp.toLocal())] ??= []).add(item);
    }
    return [
      for (final group in groups.entries) ...[
        Row(
          children: [
            Expanded(
              child: Text(
                DateUtils.isSameDay(group.key, _now)
                    ? '今天'
                    : MaterialLocalizations.of(
                        context,
                      ).formatMediumDate(group.key),
                style: const TextStyle(
                  fontSize: 13,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ),
            IconButton(
              key: ValueKey(
                'mac-notification-clear-day-${group.key.toIso8601String()}',
              ),
              tooltip: '清除此日期的通知',
              constraints: const BoxConstraints.tightFor(width: 28, height: 28),
              padding: EdgeInsets.zero,
              icon: const Icon(Icons.cancel, size: 15),
              onPressed: disabled
                  ? null
                  : () => _run(
                      () => widget.controller.dismissNotifications(group.value),
                    ),
            ),
          ],
        ),
        for (final item in group.value)
          _NotificationEntry(
            notification: item,
            busy: disabled,
            onDismiss: () => _run(
              () => widget.controller.dismissNotification(
                item.id,
                expected: item,
              ),
            ),
            onAction: (key) => _run(
              () => widget.controller.invokeNotificationAction(
                item.id,
                key,
                expected: item,
              ),
              close: true,
            ),
          ),
        const SizedBox(height: 14),
      ],
    ];
  }
}

class _NotificationEntry extends StatelessWidget {
  const _NotificationEntry({
    required this.notification,
    required this.busy,
    required this.onDismiss,
    required this.onAction,
  });
  final ShellNotification notification;
  final bool busy;
  final VoidCallback onDismiss;
  final ValueChanged<String> onAction;

  @override
  Widget build(BuildContext context) {
    final item = notification;
    final dark = Theme.of(context).brightness == Brightness.dark;
    final hasDefault = item.actions.any((action) => action.key == 'default');
    final actions = item.actions.where((action) => action.key != 'default');
    return Container(
      key: ValueKey('mac-notification-${item.id}'),
      margin: const EdgeInsets.only(top: 7),
      decoration: BoxDecoration(
        color: dark
            ? Colors.white.withValues(alpha: .09)
            : Colors.white.withValues(alpha: .72),
        borderRadius: BorderRadius.circular(7),
      ),
      clipBehavior: Clip.antiAlias,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Padding(
            padding: const EdgeInsets.only(left: 12, right: 5, top: 3),
            child: Row(
              children: [
                Expanded(
                  child: Text(
                    item.appName,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: const TextStyle(
                      fontSize: 11,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                ),
                Text(
                  '${item.timestamp.toLocal().hour.toString().padLeft(2, '0')}:${item.timestamp.toLocal().minute.toString().padLeft(2, '0')}',
                  style: const TextStyle(fontSize: 10),
                ),
                IconButton(
                  key: ValueKey('mac-notification-dismiss-${item.id}'),
                  tooltip: '清除通知',
                  onPressed: busy ? null : onDismiss,
                  constraints: const BoxConstraints.tightFor(
                    width: 27,
                    height: 27,
                  ),
                  padding: EdgeInsets.zero,
                  icon: const Icon(Icons.close, size: 13),
                ),
              ],
            ),
          ),
          InkWell(
            key: ValueKey('mac-notification-open-${item.id}'),
            onTap: !busy && hasDefault ? () => onAction('default') : null,
            child: Padding(
              padding: const EdgeInsets.fromLTRB(12, 1, 12, 12),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Text(
                    item.summary,
                    style: const TextStyle(
                      fontSize: 13,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                  if (item.body.isNotEmpty) ...[
                    const SizedBox(height: 3),
                    Text(
                      item.body,
                      style: const TextStyle(fontSize: 12, height: 1.3),
                    ),
                  ],
                ],
              ),
            ),
          ),
          if (actions.isNotEmpty) ...[
            const Divider(height: 1),
            Wrap(
              alignment: WrapAlignment.end,
              children: [
                for (final action in actions)
                  TextButton(
                    key: ValueKey(
                      'mac-notification-action-${item.id}-${action.key}',
                    ),
                    onPressed: busy ? null : () => onAction(action.key),
                    child: Text(
                      action.label.isEmpty ? action.key : action.label,
                      style: const TextStyle(fontSize: 12),
                    ),
                  ),
              ],
            ),
          ],
        ],
      ),
    );
  }
}

class _TodayCalendar extends StatefulWidget {
  const _TodayCalendar({required this.now});
  final DateTime now;
  @override
  State<_TodayCalendar> createState() => _TodayCalendarState();
}

class _TodayCalendarState extends State<_TodayCalendar> {
  late DateTime _selected = DateUtils.dateOnly(widget.now);
  final _dayFocus = FocusNode();
  @override
  void didUpdateWidget(covariant _TodayCalendar oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (DateUtils.isSameDay(_selected, oldWidget.now)) {
      _selected = DateUtils.dateOnly(widget.now);
    }
  }

  @override
  void dispose() {
    _dayFocus.dispose();
    super.dispose();
  }

  void _select(DateTime date, {bool focus = false}) {
    setState(() => _selected = date);
    if (focus) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) _dayFocus.requestFocus();
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final localizations = MaterialLocalizations.of(context);
    final days = macCalendarMonthDays(
      _selected,
      localizations.firstDayOfWeekIndex,
    );
    final dark = Theme.of(context).brightness == Brightness.dark;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const Text(
          '日历',
          style: TextStyle(fontSize: 13, fontWeight: FontWeight.w600),
        ),
        const SizedBox(height: 12),
        Row(
          children: [
            Expanded(
              child: Text(
                '${_selected.year}年${_selected.month}月',
                key: const ValueKey('mac-calendar-month'),
                style: const TextStyle(fontSize: 14),
              ),
            ),
            IconButton(
              key: const ValueKey('mac-calendar-previous'),
              tooltip: '上个月',
              constraints: const BoxConstraints.tightFor(width: 26, height: 28),
              padding: EdgeInsets.zero,
              onPressed: () => _select(macCalendarShiftMonth(_selected, -1)),
              icon: const Icon(Icons.chevron_left, size: 18),
            ),
            IconButton(
              key: const ValueKey('mac-calendar-next'),
              tooltip: '下个月',
              constraints: const BoxConstraints.tightFor(width: 26, height: 28),
              padding: EdgeInsets.zero,
              onPressed: () => _select(macCalendarShiftMonth(_selected, 1)),
              icon: const Icon(Icons.chevron_right, size: 18),
            ),
          ],
        ),
        const SizedBox(height: 8),
        Row(
          children: [
            for (var index = 0; index < 7; index++)
              Expanded(
                child: Center(
                  child: Text(
                    localizations.narrowWeekdays[(localizations
                                .firstDayOfWeekIndex +
                            index) %
                        7],
                    style: const TextStyle(fontSize: 10),
                  ),
                ),
              ),
          ],
        ),
        const SizedBox(height: 4),
        Focus(
          onKeyEvent: (_, event) {
            if (event is KeyUpEvent) return KeyEventResult.ignored;
            final delta = {
              LogicalKeyboardKey.arrowLeft: -1,
              LogicalKeyboardKey.arrowRight: 1,
              LogicalKeyboardKey.arrowUp: -7,
              LogicalKeyboardKey.arrowDown: 7,
            }[event.logicalKey];
            if (delta != null) {
              _select(
                DateTime(
                  _selected.year,
                  _selected.month,
                  _selected.day + delta,
                ),
                focus: true,
              );
            } else if (event.logicalKey == LogicalKeyboardKey.pageUp ||
                event.logicalKey == LogicalKeyboardKey.pageDown) {
              _select(
                macCalendarShiftMonth(
                  _selected,
                  event.logicalKey == LogicalKeyboardKey.pageUp ? -1 : 1,
                ),
                focus: true,
              );
            } else {
              return KeyEventResult.ignored;
            }
            return KeyEventResult.handled;
          },
          child: GridView.count(
            shrinkWrap: true,
            physics: const NeverScrollableScrollPhysics(),
            crossAxisCount: 7,
            childAspectRatio: 1.2,
            children: [
              for (final date in days)
                Semantics(
                  label: localizations.formatFullDate(date),
                  selected: DateUtils.isSameDay(date, _selected),
                  child: TextButton(
                    key: ValueKey('mac-calendar-day-${date.toIso8601String()}'),
                    focusNode: DateUtils.isSameDay(date, _selected)
                        ? _dayFocus
                        : null,
                    onPressed: () => _select(date),
                    style: TextButton.styleFrom(
                      padding: EdgeInsets.zero,
                      minimumSize: Size.zero,
                      foregroundColor: DateUtils.isSameDay(date, widget.now)
                          ? const Color(0xffeb4b4b)
                          : date.month == _selected.month
                          ? (dark ? Colors.white : Colors.black87)
                          : (dark ? Colors.white30 : Colors.black26),
                      backgroundColor: DateUtils.isSameDay(date, _selected)
                          ? (dark
                                ? Colors.white12
                                : Colors.black.withValues(alpha: .07))
                          : Colors.transparent,
                      shape: const CircleBorder(),
                    ),
                    child: Text(
                      '${date.day}',
                      style: const TextStyle(fontSize: 12),
                    ),
                  ),
                ),
            ],
          ),
        ),
        Align(
          alignment: Alignment.centerRight,
          child: TextButton(
            key: const ValueKey('mac-calendar-today'),
            onPressed: () => _select(DateUtils.dateOnly(widget.now)),
            child: const Text('回到今天', style: TextStyle(fontSize: 12)),
          ),
        ),
        Text(
          localizations.formatFullDate(_selected),
          key: const ValueKey('mac-calendar-selected'),
          style: const TextStyle(fontSize: 12),
        ),
      ],
    );
  }
}

/// Keeps the layer-shell window expanded while the right sidebar slides out.
class MacNotificationSurface extends StatelessWidget {
  const MacNotificationSurface({
    super.key,
    required this.controller,
    required this.visible,
    required this.reducedMotion,
    this.onSurfaceVisible,
  });
  final ShellController controller;
  final bool visible;
  final bool reducedMotion;
  final ValueChanged<bool>? onSurfaceVisible;
  @override
  Widget build(BuildContext context) => RetainedSurface(
    visible: visible,
    reducedMotion: reducedMotion,
    duration: const Duration(milliseconds: 180),
    reverseDuration: const Duration(milliseconds: 140),
    onSurfaceVisible: onSurfaceVisible,
    builder: (context, constraints) =>
        MacNotificationCenter(controller: controller),
    transitionBuilder: (context, value, child) => ClipRect(
      child: FractionalTranslation(
        translation: Offset(1 - value, 0),
        child: child,
      ),
    ),
  );
}
