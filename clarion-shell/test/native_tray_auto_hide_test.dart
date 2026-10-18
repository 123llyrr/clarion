// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:ui' show PointerDeviceKind;

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/mac_shell_preferences.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/ui/mac_shell_view.dart';

class _Controller extends ShellController {
  _Controller()
    : super(
        demo: true,
        notifications: false,
        tray: false,
        mediaEnabled: false,
        powerProfilesEnabled: false,
      );

  bool _nativeOpen = false;
  @override
  bool get nativeTrayMenuOpen => _nativeOpen;

  void setNativeMenuOpen(bool open) {
    _nativeOpen = open;
    notifyListeners();
  }
}

void main() {
  testWidgets('native tray menu holds auto-hidden bar until its popup closes', (
    tester,
  ) async {
    final controller = _Controller()
      ..setShellStyle(ShellStyle.macos)
      ..setReduceMotion(true)
      ..setMacPreferences(const MacShellPreferences(menuBarAutoHide: true));
    addTearDown(controller.dispose);
    await tester.binding.setSurfaceSize(const Size(1000, 700));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await tester.pumpWidget(
      MaterialApp(home: MacShellView(controller: controller, preview: true)),
    );
    await tester.pump(const Duration(milliseconds: 400));
    expect(controller.macMenuBarVisible, false);

    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: const Offset(500, 200));
    await mouse.moveTo(const Offset(500, 1));
    await tester.pump();
    expect(controller.macMenuBarVisible, true);
    controller.setNativeMenuOpen(true);
    await tester.pump();
    // A GTK popup is outside Flutter's MouseRegion, even on the same output.
    await mouse.moveTo(const Offset(500, 200));
    await tester.pump(const Duration(milliseconds: 600));
    expect(controller.panel, ShellPanel.none);
    expect(controller.macMenuBarVisible, true);

    controller.setNativeMenuOpen(false);
    await tester.pump(const Duration(milliseconds: 300));
    expect(controller.macMenuBarVisible, true);
    // Reopening while the hide timer is pending must cancel that timer too.
    controller.setNativeMenuOpen(true);
    await tester.pump(const Duration(milliseconds: 100));
    expect(controller.macMenuBarVisible, true);
    controller.setNativeMenuOpen(false);
    await tester.pump(const Duration(milliseconds: 351));
    await tester.pump();
    expect(controller.macMenuBarVisible, false);
    await mouse.removePointer();
    await tester.pumpWidget(const SizedBox());
    await tester.pump();
    expect(tester.takeException(), isNull);
  });
}
