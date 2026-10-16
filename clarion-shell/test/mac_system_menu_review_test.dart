// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/services/session_service.dart';
import 'package:clarion_shell/services/system_information_service.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/theme/mac_shell_theme.dart';
import 'package:clarion_shell/ui/mac_system_menu.dart';

class _PendingSession extends SessionService {
  late final completion = Completer<void>();
  final actions = <SessionAction>[];

  @override
  Future<Set<SessionAction>> capabilities() async =>
      SessionAction.values.toSet();

  @override
  Future<void> perform(SessionAction action) async {
    actions.add(action);
    await completion.future;
  }
}

void main() {
  late ShellController controller;
  late _PendingSession session;

  setUp(() {
    controller = ShellController(demo: true)
      ..setShellStyle(ShellStyle.macos)
      ..selectPanel(ShellPanel.systemMenu);
    session = _PendingSession();
  });

  tearDown(() => controller.dispose());

  Finder row(String key) => find.byKey(ValueKey('system-menu-$key'));

  Future<void> mount(
    WidgetTester tester, {
    Size output = const Size(800, 600),
    double left = 12,
    double menuHeight = 500,
  }) async {
    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = output;
    addTearDown(tester.view.resetDevicePixelRatio);
    addTearDown(tester.view.resetPhysicalSize);
    await tester.pumpWidget(
      MaterialApp(
        theme: buildMacShellTheme(Brightness.light),
        home: Scaffold(
          body: CallbackShortcuts(
            bindings: {
              const SingleActivator(LogicalKeyboardKey.escape):
                  controller.closePanel,
            },
            child: Stack(
              fit: StackFit.expand,
              children: [
                Positioned(
                  left: left,
                  top: 28,
                  width: 280,
                  child: ConstrainedBox(
                    constraints: BoxConstraints(maxHeight: menuHeight),
                    child: MacSystemMenu(
                      controller: controller,
                      sessionService: session,
                      userName: 'tester',
                      informationService: SystemInformationService(
                        read: (path) async => path.endsWith('cpuinfo')
                            ? 'model name: Test processor'
                            : '',
                        run: (_, _) async => '{"compositor":"review-build"}',
                      ),
                    ),
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
  }

  testWidgets('dialogs use full expanded output rather than menu bounds', (
    tester,
  ) async {
    await mount(tester);
    await tester.tap(row('about'));
    await tester.pumpAndSettle();
    final about = tester.getRect(find.byType(AlertDialog));
    expect(about.center.dx, closeTo(400, 1));
    expect(about.center.dy, closeTo(300, 1));
    expect(about.width, greaterThan(280));
    expect(find.text('版本  review-build'), findsOneWidget);
    await tester.tap(find.text('好'));
    await tester.pumpAndSettle();
    await tester.tap(row('shutdown'));
    await tester.pumpAndSettle();
    final confirmation = tester.getRect(find.byType(AlertDialog));
    expect(confirmation.center.dx, closeTo(400, 1));
    expect(confirmation.center.dy, closeTo(300, 1));
    await tester.tap(find.text('取消'));
    await tester.pumpAndSettle();
    expect(session.actions, isEmpty);
    expect(tester.takeException(), isNull);
  });

  testWidgets('about stays usable on short physical outputs', (tester) async {
    await mount(tester, output: const Size(480, 240), menuHeight: 196);
    await tester.tap(row('about'));
    await tester.pumpAndSettle();
    expect(find.text('好').hitTestable(), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('recent submenu stays inside a narrow output at its right edge', (
    tester,
  ) async {
    controller.setRecentAppIds(controller.apps.take(4).map((app) => app.id));
    await mount(
      tester,
      output: const Size(480, 400),
      left: 188,
      menuHeight: 350,
    );
    await tester.tap(row('recent'));
    await tester.pumpAndSettle();
    for (final app in controller.apps.take(4)) {
      final item = find.byKey(ValueKey('system-recent-${app.id}'));
      final rect = tester.getRect(item);
      expect(rect.left, greaterThanOrEqualTo(0));
      expect(rect.right, lessThanOrEqualTo(480));
      expect(rect.top, greaterThanOrEqualTo(0));
      expect(rect.bottom, lessThanOrEqualTo(400));
      expect(item.hitTestable(), findsOneWidget);
    }
    expect(tester.takeException(), isNull);
  });

  testWidgets('recent submenu opens to the right and returns with left arrow', (
    tester,
  ) async {
    controller.setRecentAppIds(controller.apps.take(2).map((app) => app.id));
    await mount(tester);
    Focus.of(tester.element(find.text('最近使用的项目'))).requestFocus();
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowRight);
    await tester.pumpAndSettle();
    final first = find.byKey(
      ValueKey('system-recent-${controller.apps.first.id}'),
    );
    expect(first, findsOneWidget);
    expect(
      Focus.of(
        tester.element(find.descendant(of: first, matching: find.byType(Text))),
      ).hasFocus,
      isTrue,
    );
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowLeft);
    await tester.pumpAndSettle();
    expect(first, findsNothing);
    expect(controller.panel, ShellPanel.systemMenu);
    expect(Focus.of(tester.element(find.text('最近使用的项目'))).hasFocus, isTrue);
  });

  testWidgets('late session completion cannot close a different panel', (
    tester,
  ) async {
    await mount(tester);
    await tester.tap(row('lock'));
    await tester.pump();
    expect(session.actions, [SessionAction.lock]);
    controller.selectPanel(ShellPanel.quickSettings);
    session.completion.complete();
    await tester.pumpAndSettle();
    expect(controller.panel, ShellPanel.quickSettings);
    expect(tester.takeException(), isNull);
  });

  testWidgets('arrow traversal wraps inside menu and Escape dismisses dialog', (
    tester,
  ) async {
    await mount(tester);
    Focus.of(tester.element(find.text('关于本机'))).requestFocus();
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowUp);
    await tester.pump();
    expect(
      Focus.of(tester.element(find.text('退出登录“tester”…'))).hasFocus,
      isTrue,
    );
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
    await tester.pump();
    expect(Focus.of(tester.element(find.text('关于本机'))).hasFocus, isTrue);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pumpAndSettle();
    expect(find.byType(AlertDialog), findsOneWidget);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(find.byType(AlertDialog), findsNothing);
    expect(controller.panel, ShellPanel.systemMenu);
    expect(tester.takeException(), isNull);
  });

  testWidgets('keyboard power confirmation captures Escape without action', (
    tester,
  ) async {
    await mount(tester);
    Focus.of(tester.element(find.text('重新启动…'))).requestFocus();
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pumpAndSettle();
    expect(find.byType(AlertDialog), findsOneWidget);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(find.byType(AlertDialog), findsNothing);
    expect(controller.panel, ShellPanel.systemMenu);
    expect(session.actions, isEmpty);
    expect(tester.takeException(), isNull);
  });

  testWidgets('late operation failure after menu disposal is ignored', (
    tester,
  ) async {
    await mount(tester);
    await tester.tap(row('lock'));
    await tester.pump();
    expect(session.actions, [SessionAction.lock]);
    await tester.pumpWidget(const SizedBox());
    controller.selectPanel(ShellPanel.quickSettings);
    session.completion.completeError(StateError('late failure'));
    await tester.pump();
    expect(controller.panel, ShellPanel.quickSettings);
    expect(tester.takeException(), isNull);
  });
}
