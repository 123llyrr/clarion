// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/recent_content_models.dart';
import 'package:clarion_shell/ui/recent_content_panel.dart';

final _now = DateTime(2026, 10, 18, 12);
List<RecentContentEntry> _entries() => [
  RecentContentEntry(
    id: 'new',
    kind: RecentContentKind.text,
    capturedAt: _now,
    text: '会议链接与设计笔记',
    source: 'Editor',
  ),
  RecentContentEntry(
    id: 'pinned',
    kind: RecentContentKind.text,
    capturedAt: _now.subtract(const Duration(hours: 1)),
    text: '固定内容',
    isPinned: true,
  ),
  RecentContentEntry(
    id: 'image',
    kind: RecentContentKind.image,
    capturedAt: _now.subtract(const Duration(minutes: 2)),
  ),
  RecentContentEntry(
    id: 'shot',
    kind: RecentContentKind.screenshot,
    capturedAt: _now.subtract(const Duration(minutes: 1)),
    filePath: '/isolated/fixture.png',
  ),
];

Widget _panel({
  List<RecentContentEntry>? entries,
  List<String>? actions,
  bool available = true,
  bool paused = false,
  String? error,
}) => MaterialApp(
  home: Scaffold(
    body: Align(
      alignment: Alignment.topLeft,
      child: SizedBox(
        width: 420,
        height: 640,
        child: RecentContentPanel(
          entries: entries ?? _entries(),
          now: _now,
          clipboardAvailable: available,
          paused: paused,
          error: error,
          onCopy: (entry) => actions?.add('copy:${entry.id}'),
          onPin: (entry) => actions?.add('pin:${entry.id}'),
          onDelete: (entry) => actions?.add('delete:${entry.id}'),
          onOpenScreenshot: (entry) => actions?.add('open:${entry.id}'),
          onClear: () => actions?.add('clear-unpinned'),
          onClose: () => actions?.add('close'),
          onTogglePaused: () => actions?.add('pause'),
        ),
      ),
    ),
  ),
);

void main() {
  testWidgets(
    'history filters supplied metadata and keeps pinned records first',
    (tester) async {
      await tester.pumpWidget(_panel());
      expect(
        tester.getTopLeft(find.byKey(const ValueKey('recent-entry-pinned'))).dy,
        lessThan(
          tester.getTopLeft(find.byKey(const ValueKey('recent-entry-new'))).dy,
        ),
      );
      await tester.enterText(
        find.byKey(const ValueKey('recent-content-search')),
        'Editor',
      );
      await tester.pump();
      expect(find.byKey(const ValueKey('recent-entry-new')), findsOneWidget);
      expect(find.byKey(const ValueKey('recent-entry-pinned')), findsNothing);
      await tester.enterText(
        find.byKey(const ValueKey('recent-content-search')),
        '不存在的关键字',
      );
      await tester.pump();
      expect(find.text('没有匹配的内容'), findsOneWidget);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'image tab includes screenshots and actions identify the exact item',
    (tester) async {
      final actions = <String>[];
      await tester.pumpWidget(_panel(actions: actions));
      await tester.tap(find.byKey(const ValueKey('recent-filter-images')));
      await tester.pump();
      expect(find.byKey(const ValueKey('recent-entry-new')), findsNothing);
      expect(find.byKey(const ValueKey('recent-entry-shot')), findsOneWidget);
      expect(find.byKey(const ValueKey('recent-entry-image')), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('recent-filter-screenshots')));
      await tester.pump();
      expect(find.byKey(const ValueKey('recent-entry-image')), findsNothing);
      await tester.tap(find.byKey(const ValueKey('recent-copy-shot')));
      await tester.tap(find.byKey(const ValueKey('recent-pin-shot')));
      await tester.tap(find.byKey(const ValueKey('recent-open-shot')));
      await tester.tap(find.byKey(const ValueKey('recent-delete-shot')));
      await tester.tap(find.byKey(const ValueKey('recent-content-clear')));
      expect(actions, [
        'copy:shot',
        'pin:shot',
        'open:shot',
        'delete:shot',
        'clear-unpinned',
      ]);
      expect(find.textContaining('不会删除原文件'), findsOneWidget);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'offline and paused history stays visible with unavailable copy disabled',
    (tester) async {
      final actions = <String>[];
      await tester.pumpWidget(
        _panel(
          entries: [_entries().first],
          actions: actions,
          available: false,
          paused: true,
          error: '模拟连接错误',
        ),
      );
      expect(find.text('会议链接与设计笔记'), findsOneWidget);
      expect(find.text('模拟连接错误'), findsOneWidget);
      expect(find.text('已暂停记录，已有内容仍可使用。'), findsOneWidget);
      final copy = tester.widget<TextButton>(
        find.byKey(const ValueKey('recent-copy-new')),
      );
      expect(copy.onPressed, isNull);
      await tester.tap(find.byKey(const ValueKey('recent-content-pause')));
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      expect(actions, ['pause', 'close']);
    },
  );

  testWidgets(
    'short popup scrolls the entire history and returns focus to search',
    (tester) async {
      final entries = List.generate(
        20,
        (index) => RecentContentEntry(
          id: 'item-$index',
          kind: RecentContentKind.text,
          capturedAt: _now.subtract(Duration(minutes: index)),
          text: 'A history record $index',
        ),
      );
      final actions = <String>[];
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: SizedBox(
              width: 280,
              height: 160,
              child: RecentContentPanel(
                entries: entries,
                clipboardAvailable: false,
                paused: true,
                error: 'Isolated transport is offline',
                onCopy: (_) {},
                onPin: (_) {},
                onDelete: (_) {},
                onClear: () {},
                onClose: () => actions.add('close'),
                onTogglePaused: () {},
                onReconnect: () => actions.add('reconnect'),
              ),
            ),
          ),
        ),
      );
      final scrollable = find
          .descendant(
            of: find.byKey(const ValueKey('recent-content-list')),
            matching: find.byType(Scrollable),
          )
          .first;
      await tester.scrollUntilVisible(
        find.byKey(const ValueKey('recent-entry-item-19')),
        220,
        scrollable: scrollable,
      );
      await tester.pumpAndSettle();
      expect(find.text('A history record 19'), findsOneWidget);
      await tester.scrollUntilVisible(
        find.byKey(const ValueKey('recent-content-reconnect')),
        150,
        scrollable: scrollable,
      );
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const ValueKey('recent-content-reconnect')));
      await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyF);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
      await tester.pumpAndSettle();
      final search = find.byKey(const ValueKey('recent-content-search'));
      expect(tester.widget<TextField>(search).focusNode?.hasFocus, isTrue);
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      expect(actions, ['reconnect', 'close']);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('empty session-only panel fits narrow light and dark surfaces', (
    tester,
  ) async {
    for (final brightness in Brightness.values) {
      await tester.pumpWidget(
        MaterialApp(
          theme: ThemeData(brightness: brightness),
          home: Scaffold(
            body: SizedBox(
              width: 280,
              height: 430,
              child: RecentContentPanel(
                entries: const [],
                onCopy: (_) {},
                onPin: (_) {},
                onDelete: (_) {},
                onClear: () {},
                onClose: () {},
              ),
            ),
          ),
        ),
      );
      expect(find.text('还没有最近内容'), findsOneWidget);
      expect(find.textContaining('仅保留本次会话'), findsOneWidget);
      expect(tester.takeException(), isNull);
    }
  });
}
