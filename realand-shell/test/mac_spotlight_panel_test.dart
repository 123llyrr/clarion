// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:realand_shell/models.dart';
import 'package:realand_shell/state/shell_controller.dart';
import 'package:realand_shell/ui/mac_spotlight_panel.dart';
import 'package:realand_shell/ui/mac_shell_view.dart';

class _TestController extends ShellController {
  _TestController()
    : super(
        demo: true,
        tray: false,
        notifications: false,
        powerProfilesEnabled: false,
      ) {
    setShellStyle(ShellStyle.macos);
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('MacSpotlightPanel calculator arithmetic evaluation', (
    tester,
  ) async {
    final controller = _TestController();
    addTearDown(controller.dispose);

    var closed = false;
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: MacSpotlightPanel(
            controller: controller,
            onClose: () => closed = true,
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    // Find TextField and enter expression
    final field = find.byType(TextField);
    expect(field, findsOneWidget);

    await tester.enterText(field, '25 * 4 + 10');
    // Debounce timer is 80ms
    await tester.pump(const Duration(milliseconds: 120));
    await tester.pumpAndSettle();

    // Verify calculation result
    expect(find.text('= 110'), findsOneWidget);
    expect(find.text('计算器'), findsOneWidget);

    // Escape closes
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    expect(closed, isTrue);
  });

  testWidgets('MacSpotlightPanel app and settings search and navigation', (
    tester,
  ) async {
    final controller = _TestController();
    addTearDown(controller.dispose);

    var closed = false;
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: MacSpotlightPanel(
            controller: controller,
            onClose: () => closed = true,
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    final field = find.byType(TextField);
    await tester.enterText(field, 'Wi-Fi');
    await tester.pump(const Duration(milliseconds: 120));
    await tester.pumpAndSettle();

    expect(find.text('网络设置'), findsOneWidget);

    // Arrow down and Enter
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
    await tester.pumpAndSettle();

    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pumpAndSettle();

    expect(closed, isTrue);
  });

  testWidgets('MacShellView contains Spotlight button and overlay', (
    tester,
  ) async {
    final controller = _TestController();
    addTearDown(controller.dispose);

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: MacShellView(
            controller: controller,
            preview: true,
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    // Menu bar spotlight button exists
    final spotlightBtn = find.byKey(const ValueKey('mac-spotlight'));
    expect(spotlightBtn, findsOneWidget);

    // Click spotlight button to open
    await tester.tap(spotlightBtn);
    await tester.pumpAndSettle();

    expect(controller.panel, ShellPanel.spotlight);
    expect(find.byType(MacSpotlightPanel), findsOneWidget);

    // Click spotlight button again to toggle close
    await tester.tap(spotlightBtn);
    await tester.pumpAndSettle();

    expect(controller.panel, ShellPanel.none);
    expect(find.byType(MacSpotlightPanel), findsNothing);
  });
}
