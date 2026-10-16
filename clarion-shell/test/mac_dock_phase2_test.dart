// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/theme/mac_shell_theme.dart';
import 'package:clarion_shell/ui/mac_dock_view.dart';
import 'package:clarion_shell/ui/mac_dock_context_menu.dart';
import 'package:clarion_shell/ui/mac_dock_app_menu.dart';

void main() {
  testWidgets(
    'minimizeIntoApp false renders dedicated window tile in Dock utilities',
    (tester) async {
      final controller = ShellController(demo: true)
        ..setShellStyle(ShellStyle.macos)
        ..setReduceMotion(true);

      // Initial preferences: minimizeIntoApp is false
      controller.updateMacPreferences(
        controller.macPreferences.copyWith(minimizeIntoApp: false),
      );

      await tester.pumpWidget(
        MaterialApp(
          theme: buildMacShellTheme(Brightness.light),
          home: Scaffold(
            body: Stack(
              children: [
                Positioned(
                  left: 0,
                  right: 0,
                  bottom: 0,
                  height: 120,
                  child: MacDockView(
                    controller: controller,
                    availableWidth: 1000,
                  ),
                ),
              ],
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();

      // In demo mode, minimize a window
      final windowToMinimize = controller.windows.first;
      await controller.minimizeWindow(windowToMinimize.id);
      await tester.pumpAndSettle();

      // A dedicated thumbnail tile for the window should be present in Dock
      final windowTileKey = ValueKey('dock-window-${windowToMinimize.id}');
      expect(find.byKey(windowTileKey), findsOneWidget);

      // Tapping the thumbnail unminimizes and focuses the window
      await tester.tap(find.byKey(windowTileKey));
      await tester.pumpAndSettle();

      expect(
        controller.windows
            .firstWhere((w) => w.id == windowToMinimize.id)
            .isMinimized,
        isFalse,
      );
      expect(find.byKey(windowTileKey), findsNothing);

      controller.dispose();
    },
  );

  testWidgets(
    'minimizeIntoApp true does not render separate window thumbnail in Dock',
    (tester) async {
      final controller = ShellController(demo: true)
        ..setShellStyle(ShellStyle.macos)
        ..setReduceMotion(true);

      // Default: minimizeIntoApp is true
      expect(controller.macPreferences.minimizeIntoApp, isTrue);

      await tester.pumpWidget(
        MaterialApp(
          theme: buildMacShellTheme(Brightness.light),
          home: Scaffold(
            body: Stack(
              children: [
                Positioned(
                  left: 0,
                  right: 0,
                  bottom: 0,
                  height: 120,
                  child: MacDockView(
                    controller: controller,
                    availableWidth: 1000,
                  ),
                ),
              ],
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();

      final windowToMinimize = controller.windows.first;
      await controller.minimizeWindow(windowToMinimize.id);
      await tester.pumpAndSettle();

      // No separate window tile in utilities section
      final windowTileKey = ValueKey('dock-window-${windowToMinimize.id}');
      expect(find.byKey(windowTileKey), findsNothing);

      controller.dispose();
    },
  );

  testWidgets('Dock context menu has minimizeIntoApp toggle', (tester) async {
    final controller = ShellController(demo: true)
      ..setShellStyle(ShellStyle.macos)
      ..setReduceMotion(true);

    expect(controller.macPreferences.minimizeIntoApp, isTrue);

    await tester.pumpWidget(
      MaterialApp(
        theme: buildMacShellTheme(Brightness.light),
        home: Scaffold(body: MacDockContextMenu(controller: controller)),
      ),
    );
    await tester.pumpAndSettle();

    final toggleKey = const ValueKey('dock-menu-toggle-minimize-into-app');
    expect(find.byKey(toggleKey), findsOneWidget);

    // Tap toggle
    await tester.tap(find.byKey(toggleKey));
    await tester.pumpAndSettle();

    // minimizeIntoApp should now be false
    expect(controller.macPreferences.minimizeIntoApp, isFalse);

    controller.dispose();
  });

  testWidgets('Dock app menu has login toggle option', (tester) async {
    final controller = ShellController(demo: true)
      ..setShellStyle(ShellStyle.macos)
      ..setReduceMotion(true);

    controller.openDockAppMenu(controller.apps.first.id);

    await tester.pumpWidget(
      MaterialApp(
        theme: buildMacShellTheme(Brightness.light),
        home: Scaffold(body: MacDockAppMenu(controller: controller)),
      ),
    );
    await tester.pumpAndSettle();

    final loginKey = const ValueKey('dock-menu-login');
    expect(find.byKey(loginKey), findsOneWidget);

    controller.dispose();
  });

  testWidgets('Dragging pinned app outward triggers unpinApp', (tester) async {
    final controller = ShellController(demo: true)
      ..setShellStyle(ShellStyle.macos)
      ..setReduceMotion(true)
      ..setPinnedAppIds(['terminal', 'browser']);

    await tester.pumpWidget(
      MaterialApp(
        theme: buildMacShellTheme(Brightness.light),
        home: Scaffold(
          body: Stack(
            children: [
              Positioned(
                left: 0,
                right: 0,
                bottom: 0,
                height: 120,
                child: MacDockView(
                  controller: controller,
                  availableWidth: 1000,
                ),
              ),
            ],
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    // Verify 'browser' is pinned initially
    expect(controller.pinnedAppIds, contains('browser'));

    final browserIcon = find.byKey(const ValueKey('dock-app-browser'));
    expect(browserIcon, findsOneWidget);

    // Start a long-press drag on browser icon
    final gesture = await tester.startGesture(tester.getCenter(browserIcon));
    await tester.pump(const Duration(milliseconds: 400));

    // Drag outward away from Dock (upward by 120px)
    await gesture.moveBy(const Offset(0, -120));
    await tester.pump();

    // Release drag outside the Dock
    await gesture.up();
    await tester.pumpAndSettle();

    // 'browser' should now be unpinned
    expect(controller.pinnedAppIds, isNot(contains('browser')));

    controller.dispose();
  });
}
