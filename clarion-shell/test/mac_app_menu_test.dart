// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/theme/mac_shell_theme.dart';
import 'package:clarion_shell/ui/mac_shell_view.dart';

void main() {
  testWidgets(
    'recent-content popup preserves application context without an app-name menu',
    (tester) async {
      tester.view.physicalSize = const Size(1000, 760);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final controller =
          ShellController(
              demo: true,
              notifications: false,
              tray: false,
              powerProfilesEnabled: false,
              restoreWallpaperOnStart: false,
            )
            ..setShellStyle(ShellStyle.macos)
            ..setReduceMotion(true);
      addTearDown(controller.dispose);
      final focused = controller.contextWindow?.id;
      await tester.pumpWidget(
        MaterialApp(
          theme: buildMacShellTheme(Brightness.light),
          home: Scaffold(body: MacShellView(controller: controller)),
        ),
      );
      expect(find.byKey(const ValueKey('mac-active-app-button')), findsNothing);
      await tester.tap(find.byKey(const ValueKey('mac-recent-content')));
      await tester.pumpAndSettle();
      expect(controller.panel, ShellPanel.recentContent);
      expect(controller.contextWindow?.id, focused);
      expect(
        find.byKey(const ValueKey('recent-content-panel')),
        findsOneWidget,
      );
      expect(find.text('服务'), findsNothing);
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
      expect(controller.contextWindow?.id, focused);
      expect(tester.takeException(), isNull);
    },
  );
}
