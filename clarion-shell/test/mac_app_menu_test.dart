// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:ui';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/theme/mac_shell_theme.dart';
import 'package:clarion_shell/ui/mac_app_menu.dart';
import 'package:clarion_shell/ui/mac_shell_view.dart';

void main() {
  late ShellController controller;

  setUp(() {
    controller = ShellController(demo: true);
    controller.setShellStyle(ShellStyle.macos);
  });

  tearDown(() => controller.dispose());

  Future<void> mountMenu(WidgetTester tester) async {
    await tester.pumpWidget(
      MaterialApp(
        theme: buildMacShellTheme(Brightness.light),
        home: Scaffold(body: MacAppMenu(controller: controller)),
      ),
    );
    await tester.pumpAndSettle();
  }

  testWidgets('Finder active shows authentic Finder application menu items', (
    tester,
  ) async {
    // In demo mode, minimize window 1 so Finder is active
    await controller.minimizeWindow(1);
    await mountMenu(tester);

    expect(find.text('关于“Home”'), findsOneWidget);
    expect(find.text('偏好设置…'), findsOneWidget);
    expect(find.text('清倒废纸篓…'), findsOneWidget);
    expect(find.text('服务'), findsOneWidget);
    expect(find.text('隐藏“Home”'), findsOneWidget);
    expect(find.text('隐藏其他'), findsOneWidget);
    expect(find.text('显示全部'), findsOneWidget);
    // Finder has no Quit option
    expect(find.textContaining('退出'), findsNothing);

    // Clicking preferences closes panel and opens settings
    controller.selectPanel(ShellPanel.appMenu);
    await tester.tap(find.byKey(const ValueKey('app-menu-preferences')));
    await tester.pumpAndSettle();
    expect(controller.panel, ShellPanel.none);
  });

  testWidgets(
    'Running application active shows authentic app menu with Quit option',
    (tester) async {
      // Focus a demo window
      final firstWindow = controller.windows.first;
      await controller.focusWindow(firstWindow.id);

      await mountMenu(tester);

      expect(find.text('关于“${firstWindow.title}”'), findsOneWidget);
      expect(find.text('偏好设置…'), findsOneWidget);
      expect(find.text('服务'), findsOneWidget);
      expect(find.text('隐藏“${firstWindow.title}”'), findsOneWidget);
      expect(find.text('隐藏其他'), findsOneWidget);
      expect(find.text('显示全部'), findsOneWidget);
      expect(find.text('退出“${firstWindow.title}”'), findsOneWidget);
      // Non-Finder app has no Empty Trash option
      expect(find.text('清倒废纸篓…'), findsNothing);

      // Clicking quit closes app windows and panel
      controller.selectPanel(ShellPanel.appMenu);
      await tester.tap(find.byKey(const ValueKey('app-menu-quit')));
      await tester.pumpAndSettle();
      expect(controller.panel, ShellPanel.none);
    },
  );

  testWidgets(
    'MacShellView integrates MacAppMenu and switches on hover with Apple menu',
    (tester) async {
      await controller.minimizeWindow(1);
      tester.view.physicalSize = const Size(1280, 800);
      tester.view.devicePixelRatio = 1.0;
      addTearDown(tester.view.resetPhysicalSize);

      await tester.pumpWidget(
        MaterialApp(
          theme: buildMacShellTheme(Brightness.light),
          home: Scaffold(
            body: MacShellView(controller: controller, preview: true),
          ),
        ),
      );
      await tester.pumpAndSettle();

      // Tapping active app button opens MacAppMenu
      await tester.tap(find.byKey(const ValueKey('mac-active-app-button')));
      await tester.pumpAndSettle();
      expect(controller.panel, ShellPanel.appMenu);
      expect(find.text('关于“Home”'), findsOneWidget);

      // Moving hover to Apple system menu switches to ShellPanel.systemMenu
      final appleGesture = await tester.createGesture(
        kind: PointerDeviceKind.mouse,
      );
      await appleGesture.addPointer(location: Offset.zero);
      await appleGesture.moveTo(
        tester.getCenter(find.byKey(const ValueKey('mac-system-menu'))),
      );
      await tester.pumpAndSettle();
      expect(controller.panel, ShellPanel.systemMenu);

      // Moving hover back to active app button switches back to ShellPanel.appMenu
      await appleGesture.moveTo(
        tester.getCenter(find.byKey(const ValueKey('mac-active-app-button'))),
      );
      await tester.pumpAndSettle();
      expect(controller.panel, ShellPanel.appMenu);

      // Pressing escape closes the menu
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      expect(controller.panel, ShellPanel.none);
    },
  );
}
