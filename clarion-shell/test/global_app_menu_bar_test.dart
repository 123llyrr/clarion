// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/ui/recent_content_panel.dart';

void main() {
  testWidgets('replacement bar entry exposes only a count and opens once', (
    tester,
  ) async {
    final semantics = tester.ensureSemantics();
    try {
      var opened = 0;
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: Align(
              alignment: Alignment.topLeft,
              child: RecentContentBarButton(
                count: 123,
                onPressed: () => opened++,
                selected: true,
                paused: true,
                macos: true,
              ),
            ),
          ),
        ),
      );
      expect(find.text('99+'), findsOneWidget);
      expect(find.byIcon(Icons.pause_circle_outline_rounded), findsOneWidget);
      expect(find.bySemanticsLabel(RegExp('最近内容，123 项，已暂停记录')), findsOneWidget);
      for (final oldMenu in ['文件', '编辑', '显示', '前往', '窗口', '帮助']) {
        expect(find.text(oldMenu), findsNothing);
      }
      await tester.tap(find.byType(RecentContentBarButton));
      expect(opened, 1);
      expect(tester.takeException(), isNull);
    } finally {
      semantics.dispose();
    }
  });
}
