// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/theme/shell_theme.dart';
import 'package:clarion_shell/theme/mac_shell_theme.dart';
import 'package:clarion_shell/ui/mac_dock_view.dart';
import 'package:clarion_shell/ui/mac_shell_view.dart';
import 'package:clarion_shell/ui/shell_view.dart';

ShellController _controller(ShellStyle style) =>
    ShellController(
        demo: true,
        tray: false,
        notifications: false,
        powerProfilesEnabled: false,
      )
      ..setPinnedAppIds([])
      ..setReduceMotion(true)
      ..setShellStyle(style);

Future<void> _mount(WidgetTester tester, ShellController controller) async {
  tester.view.devicePixelRatio = 1;
  tester.view.physicalSize = const Size(1000, 700);
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  addTearDown(controller.dispose);
  await tester.pumpWidget(
    MaterialApp(
      theme: controller.shellStyle == ShellStyle.macos
          ? buildMacShellTheme(Brightness.light)
          : buildShellTheme(Brightness.light),
      home: AnimatedBuilder(
        animation: controller,
        builder: (context, _) => controller.shellStyle == ShellStyle.material
            ? ShellView(controller: controller)
            : Stack(
                children: [
                  Positioned.fill(child: MacShellView(controller: controller)),
                  Positioned(
                    left: 0,
                    right: 0,
                    bottom: 0,
                    height: MacDockView.height,
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
}

Future<void> _secondaryTap(WidgetTester tester, Finder finder) async {
  final mouse = await tester.createGesture(
    kind: PointerDeviceKind.mouse,
    buttons: kSecondaryMouseButton,
  );
  await mouse.down(tester.getCenter(finder));
  await mouse.up();
  await mouse.removePointer();
  await tester.pumpAndSettle();
}

void main() {
  testWidgets('Mac launcher pins an installed app that has never run', (
    tester,
  ) async {
    final controller = _controller(ShellStyle.macos)
      ..selectPanel(ShellPanel.launcher);
    await _mount(tester, controller);
    expect(controller.dockApps.any((app) => app.id == 'files'), isFalse);
    await _secondaryTap(tester, find.byKey(const ValueKey('app-files')));
    expect(controller.panel, ShellPanel.dockAppMenu);
    expect(controller.selectedDockApp?.id, 'files');
    expect(controller.selectedDockApp?.isRunning, isFalse);
    expect(controller.selectedDockApp?.isPinned, isFalse);
    expect(find.byKey(const ValueKey('dock-menu-pin')), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('dock-menu-pin')));
    await tester.pumpAndSettle();
    expect(controller.pinnedAppIds, contains('files'));
    expect(controller.panel, ShellPanel.none);
    expect(find.byKey(const ValueKey('dock-app-files')), findsOneWidget);
    expect(
      controller.dockApps.singleWhere((app) => app.id == 'files').isRunning,
      isFalse,
    );
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'Material launcher retains its original secondary click behavior',
    (tester) async {
      final controller = _controller(ShellStyle.material)
        ..selectPanel(ShellPanel.launcher);
      await _mount(tester, controller);
      await _secondaryTap(tester, find.byKey(const ValueKey('app-files')));
      expect(controller.panel, ShellPanel.launcher);
      expect(controller.pinnedAppIds, isEmpty);
      expect(find.byKey(const ValueKey('dock-menu-pin')), findsNothing);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('switching styles closes a launcher application menu', (
    tester,
  ) async {
    final controller = _controller(ShellStyle.macos)
      ..selectPanel(ShellPanel.launcher);
    await _mount(tester, controller);
    await _secondaryTap(tester, find.byKey(const ValueKey('app-files')));
    expect(controller.panel, ShellPanel.dockAppMenu);
    controller.setShellStyle(ShellStyle.material);
    await tester.pumpAndSettle();
    expect(controller.panel, ShellPanel.none);
    expect(find.byKey(const ValueKey('dock-menu-pin')), findsNothing);
    expect(find.byType(MacDockView), findsNothing);
    expect(find.byType(ShellView), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('removing a hovered Dock app immediately removes its label', (
    tester,
  ) async {
    final controller = _controller(ShellStyle.macos)
      ..setPinnedAppIds(['files']);
    await _mount(tester, controller);
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: const Offset(0, 0));
    await mouse.moveTo(
      tester.getCenter(find.byKey(const ValueKey('dock-app-files'))),
    );
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('mac-dock-label')), findsOneWidget);
    controller.togglePinnedApp('files');
    await tester.pump();
    expect(find.byKey(const ValueKey('dock-app-files')), findsNothing);
    expect(find.byKey(const ValueKey('mac-dock-label')), findsNothing);
    await mouse.removePointer();
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
  });
}
