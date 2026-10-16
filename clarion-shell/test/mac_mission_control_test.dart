// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/theme/mac_shell_theme.dart';
import 'package:clarion_shell/ui/mac_dock_view.dart';
import 'package:clarion_shell/ui/mac_shell_view.dart';

void main() {
  test('toggleSpaces toggles ShellPanel.spaces', () {
    final controller = ShellController(demo: true);
    expect(controller.panel, ShellPanel.none);

    controller.toggleSpaces();
    expect(controller.panel, ShellPanel.spaces);

    controller.toggleSpaces();
    expect(controller.panel, ShellPanel.none);

    controller.dispose();
  });

  testWidgets(
    'MacMissionControlPanel mounts and renders spaces bar and window spread',
    (tester) async {
      final controller = ShellController(demo: true)
        ..setShellStyle(ShellStyle.macos)
        ..setReduceMotion(true);

      await tester.pumpWidget(
        MaterialApp(
          theme: buildMacShellTheme(Brightness.light),
          home: Scaffold(
            body: SizedBox(
              width: 1200,
              height: 800,
              child: MacShellView(controller: controller),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();

      // Open Mission Control
      controller.selectPanel(ShellPanel.spaces);
      await tester.pumpAndSettle();

      expect(find.byKey(const ValueKey('mac-spaces-overlay')), findsOneWidget);
      expect(find.byKey(const ValueKey('mac-spaces-bar')), findsOneWidget);
      expect(find.byKey(const ValueKey('mac-window-spread')), findsOneWidget);

      // Spaces cards exist for demo workspaces 1 through 4
      for (var i = 1; i <= 4; i++) {
        expect(find.byKey(ValueKey('mac-space-card-$i')), findsOneWidget);
      }

      controller.dispose();
    },
  );

  testWidgets(
    'Tapping space card switches workspace and closes Mission Control',
    (tester) async {
      final controller = ShellController(demo: true)
        ..setShellStyle(ShellStyle.macos)
        ..setReduceMotion(true);

      await tester.pumpWidget(
        MaterialApp(
          theme: buildMacShellTheme(Brightness.light),
          home: Scaffold(
            body: SizedBox(
              width: 1200,
              height: 800,
              child: MacShellView(controller: controller),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();

      controller.selectPanel(ShellPanel.spaces);
      await tester.pumpAndSettle();

      // Tap on Space card 2
      await tester.tap(find.byKey(const ValueKey('mac-space-card-2')));
      await tester.pumpAndSettle();

      expect(controller.panel, ShellPanel.none);
      expect(
        controller.workspaces.firstWhere((ws) => ws.id == 2).isActive,
        isTrue,
      );

      controller.dispose();
    },
  );

  testWidgets('Tapping add space button creates a new space', (tester) async {
    final controller = ShellController(demo: true)
      ..setShellStyle(ShellStyle.macos)
      ..setReduceMotion(true);

    final initialCount = controller.workspaces.length;

    await tester.pumpWidget(
      MaterialApp(
        theme: buildMacShellTheme(Brightness.light),
        home: Scaffold(
          body: SizedBox(
            width: 1200,
            height: 800,
            child: MacShellView(controller: controller),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    controller.selectPanel(ShellPanel.spaces);
    await tester.pumpAndSettle();

    // Tap on + button
    await tester.tap(find.byKey(const ValueKey('mac-space-add-button')));
    await tester.pumpAndSettle();

    expect(controller.workspaces.length, initialCount + 1);
    expect(
      find.byKey(ValueKey('mac-space-card-${initialCount + 1}')),
      findsOneWidget,
    );

    controller.dispose();
  });

  testWidgets('Window spread renders open windows and tapping focuses window', (
    tester,
  ) async {
    final controller = ShellController(demo: true)
      ..setShellStyle(ShellStyle.macos)
      ..setReduceMotion(true);

    await tester.pumpWidget(
      MaterialApp(
        theme: buildMacShellTheme(Brightness.light),
        home: Scaffold(
          body: SizedBox(
            width: 1200,
            height: 800,
            child: MacShellView(controller: controller),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    controller.selectPanel(ShellPanel.spaces);
    await tester.pumpAndSettle();

    // Window 1 is in spread
    final windowCard = find.byKey(const ValueKey('mac-window-card-1'));
    expect(windowCard, findsOneWidget);

    // Tapping window focuses it and closes Mission Control
    await tester.tap(windowCard);
    await tester.pumpAndSettle();

    expect(controller.panel, ShellPanel.none);
    expect(controller.windows.firstWhere((w) => w.id == 1).isFocused, isTrue);

    controller.dispose();
  });

  testWidgets(
    'Dragging window card from spread onto space card moves window across spaces',
    (tester) async {
      final controller = ShellController(demo: true)
        ..setShellStyle(ShellStyle.macos)
        ..setReduceMotion(true);

      expect(controller.windows.firstWhere((w) => w.id == 1).workspaceId, 1);

      await tester.pumpWidget(
        MaterialApp(
          theme: buildMacShellTheme(Brightness.light),
          home: Scaffold(
            body: SizedBox(
              width: 1200,
              height: 800,
              child: MacShellView(controller: controller),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();

      controller.selectPanel(ShellPanel.spaces);
      await tester.pumpAndSettle();

      final windowCard = find.byKey(const ValueKey('mac-window-card-1'));
      final targetSpace = find.byKey(const ValueKey('mac-space-card-3'));

      expect(windowCard, findsOneWidget);
      expect(targetSpace, findsOneWidget);

      // Drag window card onto Space 3
      final origin = tester.getCenter(windowCard);
      final destination = tester.getCenter(targetSpace);

      final gesture = await tester.startGesture(origin);
      await tester.pump(const Duration(milliseconds: 50));
      await gesture.moveTo(destination);
      await tester.pump(const Duration(milliseconds: 50));
      await gesture.up();
      await tester.pumpAndSettle();

      // Window 1 should now belong to Workspace 3!
      expect(controller.windows.firstWhere((w) => w.id == 1).workspaceId, 3);

      controller.dispose();
    },
  );

  testWidgets(
    'mac-overview bar button and dock-overview button toggle Mission Control in macOS mode',
    (tester) async {
      final controller = ShellController(demo: true)
        ..setShellStyle(ShellStyle.macos)
        ..setReduceMotion(true);

      await tester.pumpWidget(
        MaterialApp(
          theme: buildMacShellTheme(Brightness.light),
          home: Scaffold(
            body: SizedBox(
              width: 1200,
              height: 800,
              child: Stack(
                children: [
                  MacShellView(controller: controller),
                  Positioned(
                    left: 0,
                    right: 0,
                    bottom: 0,
                    height: 120,
                    child: MacDockView(
                      controller: controller,
                      availableWidth: 1200,
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();

      // 1. Tapping mac-overview in menu bar
      final barButton = find.byKey(const ValueKey('mac-overview'));
      expect(barButton, findsOneWidget);
      await tester.tap(barButton);
      await tester.pumpAndSettle();
      expect(controller.panel, ShellPanel.spaces);

      await tester.tap(barButton);
      await tester.pumpAndSettle();
      expect(controller.panel, ShellPanel.none);

      // 2. Tapping dock-overview on Dock
      final dockButton = find.byKey(const ValueKey('dock-overview'));
      expect(dockButton, findsOneWidget);
      await tester.tap(dockButton);
      await tester.pumpAndSettle();
      expect(controller.panel, ShellPanel.spaces);

      controller.closePanel();
      await tester.pumpAndSettle();
      expect(controller.panel, ShellPanel.none);

      controller.dispose();
    },
  );

  testWidgets('Escape key closes Mission Control', (tester) async {
    final controller = ShellController(demo: true)
      ..setShellStyle(ShellStyle.macos)
      ..setReduceMotion(true);

    await tester.pumpWidget(
      MaterialApp(
        theme: buildMacShellTheme(Brightness.light),
        home: Scaffold(
          body: SizedBox(
            width: 1200,
            height: 800,
            child: MacShellView(controller: controller),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    controller.selectPanel(ShellPanel.spaces);
    await tester.pumpAndSettle();
    expect(controller.panel, ShellPanel.spaces);

    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(controller.panel, ShellPanel.none);

    controller.dispose();
  });
}
