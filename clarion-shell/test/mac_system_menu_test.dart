// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/services/session_service.dart';
import 'package:clarion_shell/services/system_information_service.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/theme/mac_shell_theme.dart';
import 'package:clarion_shell/ui/mac_system_menu.dart';
import 'package:clarion_shell/ui/shell_panel_content.dart';

class _Session extends SessionService {
  Completer<Set<SessionAction>>? reading;
  Completer<void>? work;
  Object? failure;
  final actions = <SessionAction>[];
  @override
  Future<Set<SessionAction>> capabilities() async =>
      reading == null ? SessionAction.values.toSet() : reading!.future;
  @override
  Future<void> perform(SessionAction action) async {
    actions.add(action);
    if (work != null) await work!.future;
    if (failure != null) throw failure!;
  }
}

void main() {
  late ShellController controller;
  late _Session session;
  setUp(() {
    controller = ShellController(demo: true);
    controller.setShellStyle(ShellStyle.macos);
    controller.selectPanel(ShellPanel.systemMenu);
    session = _Session();
  });
  tearDown(() => controller.dispose());
  testWidgets('external panel change removes the menu owned modal route', (
    tester,
  ) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: ListenableBuilder(
            listenable: controller,
            builder: (context, _) => SizedBox(
              width: 280,
              child: ShellPanelContent(controller: controller, preview: true),
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('system-menu-shutdown')));
    await tester.pumpAndSettle();
    expect(find.byType(AlertDialog), findsOneWidget);
    controller.closePanel();
    await tester.pumpAndSettle();
    expect(find.byType(AlertDialog), findsNothing);
    expect(controller.panel, ShellPanel.none);
    expect(tester.takeException(), isNull);
  });
  Finder row(String key) => find.byKey(ValueKey('system-menu-$key'));
  TextButton button(WidgetTester tester, String key) => tester.widget(
    find.descendant(of: row(key), matching: find.byType(TextButton)),
  );
  Future<void> mount(WidgetTester tester, {double height = 600}) async {
    await tester.pumpWidget(
      MaterialApp(
        theme: buildMacShellTheme(Brightness.light),
        home: Scaffold(
          body: SizedBox(
            width: 280,
            height: height,
            child: MacSystemMenu(
              controller: controller,
              sessionService: session,
              userName: 'tester',
              informationService: SystemInformationService(
                read: (_) async => '',
                run: (_, _) async => '{"compositor":"active-build"}',
              ),
            ),
          ),
        ),
      ),
    );
    await tester.pump();
  }

  testWidgets('compact Catalina order and capabilities, no launchpad heading', (
    tester,
  ) async {
    session.reading = Completer();
    await mount(tester);
    expect(find.text('关于本机'), findsOneWidget);
    expect(find.text('控制中心与外观'), findsNothing);
    expect(find.text('退出登录“tester”…'), findsOneWidget);
    expect(button(tester, 'sleep').onPressed, isNull);
    expect(button(tester, 'force-quit').onPressed, isNotNull);
    session.reading!.complete({SessionAction.lock, SessionAction.logOut});
    await tester.pumpAndSettle();
    expect(button(tester, 'sleep').onPressed, isNull);
    expect(button(tester, 'lock').onPressed, isNotNull);
    expect(tester.getSize(row('settings')).height, 26);
    expect(
      tester.getTopLeft(row('about')).dy,
      lessThan(tester.getTopLeft(row('settings')).dy),
    );
    expect(
      tester.getTopLeft(row('shutdown')).dy,
      lessThan(tester.getTopLeft(row('lock')).dy),
    );
  });

  testWidgets('cancel power confirmation makes no system call', (tester) async {
    await mount(tester);
    await tester.tap(row('restart'));
    await tester.pumpAndSettle();
    expect(session.actions, isEmpty);
    await tester.tap(find.text('取消'));
    await tester.pumpAndSettle();
    expect(session.actions, isEmpty);
    expect(button(tester, 'restart').onPressed, isNotNull);
    expect(controller.panel, ShellPanel.systemMenu);
  });

  testWidgets(
    'confirmation acts once, pending work disables rows then closes',
    (tester) async {
      session.work = Completer();
      await mount(tester);
      await tester.tap(row('shutdown'));
      await tester.pumpAndSettle();
      await tester.tap(row('confirm'));
      await tester.pumpAndSettle();
      expect(session.actions, [SessionAction.shutDown]);
      expect(button(tester, 'lock').onPressed, isNull);
      session.work!.complete();
      await tester.pump();
      expect(controller.panel, ShellPanel.none);
    },
  );

  testWidgets('stale confirmation cannot execute after panel changes', (
    tester,
  ) async {
    await mount(tester);
    await tester.tap(row('logout'));
    await tester.pumpAndSettle();
    controller.selectPanel(ShellPanel.quickSettings);
    await tester.tap(row('confirm'));
    await tester.pumpAndSettle();
    expect(session.actions, isEmpty);
    expect(controller.panel, ShellPanel.quickSettings);
  });

  testWidgets('action failure is visible and does not dismiss menu', (
    tester,
  ) async {
    session.failure = StateError('denied');
    await mount(tester);
    await tester.tap(row('lock'));
    await tester.pump();
    expect(session.actions, [SessionAction.lock]);
    expect(find.textContaining('denied'), findsOneWidget);
    expect(controller.panel, ShellPanel.systemMenu);
    expect(button(tester, 'lock').onPressed, isNotNull);
  });

  testWidgets('action timeout does not claim the action was cancelled', (
    tester,
  ) async {
    session.failure = TimeoutException('no reply');
    await mount(tester);
    await tester.tap(row('lock'));
    await tester.pump();
    expect(find.text('未收到系统操作结果，请检查当前状态'), findsOneWidget);
    expect(session.actions.length, 1);
  });

  testWidgets(
    'recent submenu contains persisted applications and clears history',
    (tester) async {
      controller.setRecentAppIds(controller.apps.take(2).map((app) => app.id));
      await mount(tester);
      await tester.tap(row('recent'));
      await tester.pumpAndSettle();
      expect(
        find.byKey(ValueKey('system-recent-${controller.apps.first.id}')),
        findsOneWidget,
      );
      await tester.tap(find.byKey(const ValueKey('system-recent-clear')));
      await tester.pumpAndSettle();
      expect(controller.recentAppIds, isEmpty);
      expect(controller.panel, ShellPanel.none);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'about reads active compositor information and fits short outputs',
    (tester) async {
      await mount(tester, height: 180);
      await tester.tap(row('about'));
      await tester.pumpAndSettle();
      expect(find.text('版本  active-build'), findsOneWidget);
      await tester.tap(find.text('好'));
      await tester.pumpAndSettle();
      await tester.drag(
        find.byType(SingleChildScrollView).first,
        const Offset(0, -450),
      );
      await tester.pumpAndSettle();
      expect(row('logout').hitTestable(), findsOneWidget);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('late capability reply after disposal is ignored', (
    tester,
  ) async {
    session.reading = Completer();
    await mount(tester);
    await tester.pumpWidget(const SizedBox());
    session.reading!.complete(SessionAction.values.toSet());
    await tester.pump();
    expect(tester.takeException(), isNull);
  });
}
