// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:realand_shell/models.dart';
import 'package:realand_shell/services/login_items_service.dart';
import 'package:realand_shell/state/shell_controller.dart';
import 'package:realand_shell/ui/mac_dock_app_menu.dart';

class _LoginItems extends LoginItemsService {
  bool enabled = false;
  bool failWrite = false;
  int writes = 0;
  Completer<bool>? read;
  Completer<void>? write;

  @override
  Future<bool> isEnabled(DesktopApp app) async =>
      read == null ? enabled : read!.future;

  @override
  Future<void> setEnabled(DesktopApp app, bool value) async {
    writes++;
    if (write != null) await write!.future;
    if (failWrite) throw const FileSystemWriteFailure();
    enabled = value;
  }
}

class FileSystemWriteFailure implements Exception {
  const FileSystemWriteFailure();
  @override
  String toString() => '配置目录不可写';
}

void main() {
  late _LoginItems service;
  late ShellController controller;
  const login = ValueKey('dock-menu-login');
  setUp(() {
    service = _LoginItems();
    controller = ShellController(demo: true, loginItemsService: service);
    controller.openDockAppMenu(controller.apps.first.id);
  });
  tearDown(() => controller.dispose());

  Future<void> show(WidgetTester tester) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(body: MacDockAppMenu(controller: controller)),
      ),
    );
    await tester.pump();
  }

  Finder check() => find.descendant(
    of: find.byKey(login),
    matching: find.byIcon(Icons.check),
  );

  testWidgets('login checkbox reads saved state and persists both directions', (
    tester,
  ) async {
    await show(tester);
    expect(check(), findsNothing);
    await tester.tap(find.byKey(login));
    await tester.pump();
    expect(service.enabled, isTrue);
    expect(controller.panel, ShellPanel.none);
    controller.openDockAppMenu(controller.apps.first.id);
    await tester.pumpAndSettle();
    expect(check(), findsOneWidget);
    await tester.tap(find.byKey(login));
    await tester.pump();
    expect(service.enabled, isFalse);
    expect(service.writes, 2);
  });

  testWidgets('disabled during initial read and pending write', (tester) async {
    service.read = Completer<bool>();
    service.write = Completer<void>();
    await show(tester);
    expect(tester.widget<InkWell>(find.byKey(login)).onTap, isNull);
    service.read!.complete(false);
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(login));
    await tester.pump();
    expect(service.writes, 1);
    expect(tester.widget<InkWell>(find.byKey(login)).onTap, isNull);
    service.write!.complete();
    await tester.pump();
    expect(service.writes, 1);
  });

  testWidgets(
    'write failure stays open with actual error and unchanged check',
    (tester) async {
      service.failWrite = true;
      await show(tester);
      await tester.tap(find.byKey(login));
      await tester.pump();
      expect(controller.panel, ShellPanel.dockAppMenu);
      expect(find.textContaining('配置目录不可写'), findsOneWidget);
      expect(check(), findsNothing);
      expect(service.enabled, isFalse);
      expect(tester.widget<InkWell>(find.byKey(login)).onTap, isNotNull);
    },
  );

  testWidgets('completion after dismissal cannot close a newer panel', (
    tester,
  ) async {
    service.write = Completer<void>();
    await show(tester);
    await tester.tap(find.byKey(login));
    await tester.pump();
    controller.openDockContextMenu();
    await tester.pump();
    service.write!.complete();
    await tester.pump();
    expect(controller.panel, ShellPanel.dockContextMenu);
    expect(tester.takeException(), isNull);
  });

  testWidgets('replacing the menu target discards an old pending write', (
    tester,
  ) async {
    service.write = Completer<void>();
    await show(tester);
    await tester.tap(find.byKey(login));
    await tester.pump();
    final replacementService = _LoginItems();
    final replacement = ShellController(
      demo: true,
      loginItemsService: replacementService,
    );
    addTearDown(replacement.dispose);
    replacement.openDockAppMenu(replacement.apps.first.id);
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(body: MacDockAppMenu(controller: replacement)),
      ),
    );
    await tester.pump();
    expect(tester.widget<InkWell>(find.byKey(login)).onTap, isNotNull);
    expect(check(), findsNothing);
    service.write!.complete();
    await tester.pump();
    expect(check(), findsNothing);
    expect(replacement.panel, ShellPanel.dockAppMenu);
    expect(replacementService.writes, 0);
  });

  testWidgets('menu height includes the functional login row', (tester) async {
    await show(tester);
    final content = find.byType(Column).first;
    expect(
      tester.getSize(content).height + 12,
      MacDockAppMenu.estimatedHeight(controller),
    );
  });
}
