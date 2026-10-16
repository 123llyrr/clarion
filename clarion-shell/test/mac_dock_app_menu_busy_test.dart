// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/ui/mac_dock_app_menu.dart';

class _Controller extends ShellController {
  _Controller() : super(demo: true);
  final gate = Completer<void>();
  final _pending = <String>{};
  int requests = 0;

  @override
  Set<String> get busy => Set.unmodifiable(_pending);

  @override
  Future<void> forceQuitApp(String appId) async {
    requests++;
    _pending.add('force-quit-app');
    notifyListeners();
    try {
      await gate.future;
    } finally {
      _pending.remove('force-quit-app');
      notifyListeners();
    }
  }
}

void main() {
  testWidgets('force quit stays disabled for the complete pending operation', (
    tester,
  ) async {
    final controller = _Controller();
    addTearDown(controller.dispose);
    controller.openDockAppMenu(
      controller.dockApps.firstWhere((app) => app.isRunning).id,
    );
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(body: MacDockAppMenu(controller: controller)),
      ),
    );
    await tester.pumpAndSettle();
    final forceQuit = find.byKey(const ValueKey('dock-menu-force-quit'));
    expect(tester.widget<InkWell>(forceQuit).onTap, isNotNull);
    await tester.tap(forceQuit);
    await tester.pump();
    expect(controller.requests, 1);
    expect(tester.widget<InkWell>(forceQuit).onTap, isNull);
    await tester.tap(forceQuit);
    await tester.pump();
    expect(controller.requests, 1);
    controller.gate.complete();
    await tester.pumpAndSettle();
    expect(tester.widget<InkWell>(forceQuit).onTap, isNotNull);
  });
}
