// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/ui/shell_view.dart';

void main() {
  testWidgets(
    'Material bar opens recent content and has no global menu entry',
    (tester) async {
      tester.view.devicePixelRatio = 1;
      tester.view.physicalSize = const Size(900, 700);
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final controller = ShellController(
        demo: true,
        notifications: false,
        tray: false,
        powerProfilesEnabled: false,
        restoreWallpaperOnStart: false,
      )..setReduceMotion(true);
      addTearDown(controller.dispose);
      await tester.pumpWidget(
        MaterialApp(home: ShellView(controller: controller)),
      );
      expect(
        find.byKey(const ValueKey('material-app-menu-entry')),
        findsNothing,
      );
      expect(find.byKey(const ValueKey('bar-recent-content')), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('bar-recent-content')));
      await tester.pumpAndSettle();
      expect(controller.panel, ShellPanel.recentContent);
      expect(
        find.byKey(const ValueKey('recent-content-panel')),
        findsOneWidget,
      );
      expect(find.text('调度中心'), findsNothing);
      await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyF);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
      await tester.pump();
      final search = find.byKey(const ValueKey('recent-content-search'));
      expect(tester.widget<TextField>(search).focusNode?.hasFocus, isTrue);
      await tester.enterText(search, 'isolated search');
      controller.setReduceMotion(false);
      await tester.pump();
      expect(tester.widget<TextField>(search).focusNode?.hasFocus, isTrue);
      expect(
        tester.widget<TextField>(search).controller?.text,
        'isolated search',
      );
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      expect(controller.panel, ShellPanel.none);
      expect(tester.takeException(), isNull);
    },
  );
}
