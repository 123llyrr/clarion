// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:realand_shell/models.dart';
import 'package:realand_shell/state/shell_controller.dart';
import 'package:realand_shell/theme/shell_theme.dart';
import 'package:realand_shell/ui/shell_view.dart';

void main() {
  testWidgets('bar and button state survive panel insertion and removal', (
    tester,
  ) async {
    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = const Size(1280, 800);
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final controller = ShellController(
      demo: true,
      tray: false,
      notifications: false,
      powerProfilesEnabled: false,
    );
    addTearDown(controller.dispose);
    await tester.pumpWidget(
      MaterialApp(
        theme: buildShellTheme(Brightness.dark),
        home: ShellView(controller: controller),
      ),
    );
    await tester.pumpAndSettle();
    final launcher = find.byKey(const ValueKey('bar-launcher'));
    final workspace = find.byKey(const ValueKey('workspace-1'));
    final button = find.descendant(
      of: launcher,
      matching: find.byType(InkWell),
    );
    final initialLauncher = tester.element(launcher);
    final initialWorkspace = tester.element(workspace);
    final initialInk = tester.element(button);
    for (final reduced in [false, true]) {
      controller.setReduceMotion(reduced);
      for (final panel in [
        ShellPanel.quickSettings,
        ShellPanel.launcher,
        ShellPanel.notifications,
        ShellPanel.screenshot,
      ]) {
        controller.selectPanel(panel);
        await tester.pump();
        expect(
          tester.element(launcher),
          same(initialLauncher),
          reason: '$panel opens',
        );
        expect(tester.element(workspace), same(initialWorkspace));
        expect(tester.element(button), same(initialInk));
        await tester.pump(const Duration(milliseconds: 60));
        controller.closePanel();
        await tester.pump();
        expect(
          tester.element(launcher),
          same(initialLauncher),
          reason: '$panel closes',
        );
        expect(tester.element(workspace), same(initialWorkspace));
        expect(tester.element(button), same(initialInk));
        await tester.pumpAndSettle();
      }
    }
    expect(tester.takeException(), null);
  });
}
