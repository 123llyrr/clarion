// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/mac_launchpad_layout.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/ui/mac_launcher_panel.dart';

class _JiggleTestController extends ShellController {
  _JiggleTestController()
    : super(
        demo: true,
        tray: false,
        notifications: false,
        powerProfilesEnabled: false,
      ) {
    entries = List.generate(
      10,
      (index) =>
          DesktopApp(id: 'app-$index', name: 'App $index', desktopFile: ''),
    );
    setShellStyle(ShellStyle.macos);
    setReduceMotion(false);
    selectPanel(ShellPanel.launcher);
  }
  late List<DesktopApp> entries;
  @override
  List<DesktopApp> get apps => entries;
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  testWidgets(
    'Launchpad Jiggle Mode triggers via Alt key and exits via Escape',
    (tester) async {
      final controller = _JiggleTestController();
      final layout = MacLaunchpadLayout(persist: false);
      addTearDown(controller.dispose);
      addTearDown(layout.dispose);

      await layout.load(controller.apps.map((app) => app.id));

      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: MacLauncherPanel(controller: controller, layout: layout),
          ),
        ),
      );
      await tester.pumpAndSettle();

      // Verify apps are shown
      expect(find.byKey(const ValueKey('app-app-0')), findsOneWidget);

      // Press Alt key to enter Jiggle mode
      await tester.sendKeyEvent(LogicalKeyboardKey.altLeft);
      await tester.pump();

      // Now press Escape to exit Jiggle mode
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pump();

      // Panel should still be open (first escape only exits jiggle mode)
      expect(controller.panel, ShellPanel.launcher);

      // Second escape closes the panel
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pump();
      expect(controller.panel, ShellPanel.none);
    },
  );

  testWidgets('Launchpad Jiggle Mode triggers via long press on tile', (
    tester,
  ) async {
    final controller = _JiggleTestController();
    final layout = MacLaunchpadLayout(persist: false);
    addTearDown(controller.dispose);
    addTearDown(layout.dispose);

    await layout.load(controller.apps.map((app) => app.id));

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: MacLauncherPanel(controller: controller, layout: layout),
        ),
      ),
    );
    await tester.pumpAndSettle();

    final tile = find.byKey(const ValueKey('app-app-0'));
    expect(tile, findsOneWidget);

    // Long press on background to trigger Jiggle Mode
    await tester.longPressAt(const Offset(10, 10));
    await tester.pump();

    // Tap empty background area to exit jiggle mode
    await tester.tapAt(const Offset(10, 10));
    await tester.pump();

    // Launcher should still be open
    expect(controller.panel, ShellPanel.launcher);

    // Tapping background again closes launcher
    await tester.tapAt(const Offset(10, 10));
    await tester.pump();
    expect(controller.panel, ShellPanel.none);
  });

  for (final optionKey in [
    LogicalKeyboardKey.altLeft,
    LogicalKeyboardKey.altRight,
  ]) {
    testWidgets('repeated $optionKey events keep Launchpad editing active', (
      tester,
    ) async {
      final controller = _JiggleTestController();
      final layout = MacLaunchpadLayout(persist: false);
      addTearDown(controller.dispose);
      addTearDown(layout.dispose);
      await layout.load(controller.apps.map((app) => app.id));
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: MacLauncherPanel(controller: controller, layout: layout),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(tester.hasRunningAnimations, isFalse);

      await tester.sendKeyDownEvent(optionKey);
      await tester.pump();
      expect(tester.hasRunningAnimations, isTrue);
      for (var repeat = 0; repeat < 3; repeat++) {
        await tester.sendKeyRepeatEvent(optionKey);
        await tester.pump(const Duration(milliseconds: 40));
        expect(tester.hasRunningAnimations, isTrue);
      }
      await tester.sendKeyUpEvent(optionKey);
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      expect(controller.panel, ShellPanel.launcher);
      expect(tester.hasRunningAnimations, isFalse);
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pump();
      expect(controller.panel, ShellPanel.none);
      await tester.pumpWidget(const SizedBox.shrink());
    });
  }
}
