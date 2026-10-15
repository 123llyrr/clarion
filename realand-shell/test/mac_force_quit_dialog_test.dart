// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:realand_shell/models.dart';
import 'package:realand_shell/state/shell_controller.dart';
import 'package:realand_shell/theme/mac_shell_theme.dart';
import 'package:realand_shell/ui/mac_force_quit_dialog.dart';

void main() {
  late ShellController controller;

  setUp(() {
    controller = ShellController(demo: true);
    controller.setShellStyle(ShellStyle.macos);
  });

  tearDown(() => controller.dispose());

  Future<void> mount(WidgetTester tester) async {
    await tester.pumpWidget(
      MaterialApp(
        theme: buildMacShellTheme(Brightness.light),
        home: Scaffold(
          body: Builder(
            builder: (context) => ElevatedButton(
              onPressed: () => showForceQuitDialog(
                context: context,
                controller: controller,
              ),
              child: const Text('Open'),
            ),
          ),
        ),
      ),
    );
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();
  }

  testWidgets('renders authentic macOS Force Quit dialog with running apps and Finder', (
    tester,
  ) async {
    await mount(tester);

    expect(find.text('强制退出应用程序'), findsOneWidget);
    expect(
      find.text('如果某个应用程序没有响应，请在列表中将其选中，然后点按“强制退出”。'),
      findsOneWidget,
    );
    expect(find.text('您可以按住“Option-Command-Esc”来直接打开此窗口。'), findsOneWidget);
    expect(find.text('访达'), findsOneWidget);

    // Cancel button works
    await tester.tap(find.byKey(const ValueKey('force-quit-cancel')));
    await tester.pumpAndSettle();
    expect(find.text('强制退出应用程序'), findsNothing);
  });

  testWidgets('selecting Finder displays 重新开启 and selecting running app displays 强制退出', (
    tester,
  ) async {
    await mount(tester);

    // In demo mode, editor and browser have windows
    expect(find.text('强制退出应用程序'), findsOneWidget);

    // Select Finder
    await tester.tap(find.text('访达'));
    await tester.pumpAndSettle();
    expect(find.text('重新开启'), findsOneWidget);

    // Select an app if available
    final editorItem = find.byKey(const ValueKey('force-quit-item-editor.desktop'));
    if (editorItem.evaluate().isNotEmpty) {
      await tester.tap(editorItem);
      await tester.pumpAndSettle();
      expect(find.text('强制退出'), findsOneWidget);

      // Tap 强制退出
      await tester.tap(find.byKey(const ValueKey('force-quit-confirm')));
      await tester.pumpAndSettle();
      expect(find.text('强制退出应用程序'), findsNothing);
    }
  });

  testWidgets('keyboard navigation and Escape handling', (tester) async {
    await mount(tester);
    expect(find.text('强制退出应用程序'), findsOneWidget);

    // Down arrow moves selection
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
    await tester.pumpAndSettle();

    // Escape closes dialog
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(find.text('强制退出应用程序'), findsNothing);
  });
}
