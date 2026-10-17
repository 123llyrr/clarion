// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/cupertino.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_localizations/flutter_localizations.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/main.dart';
import 'package:clarion_shell/mac_notification_view.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/preferences.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/theme/mac_shell_theme.dart';
import 'package:clarion_shell/ui/mac_calendar_month.dart';
import 'package:clarion_shell/ui/mac_notification_center.dart';
import 'package:clarion_shell/ui/mac_shell_view.dart';
import 'package:clarion_shell/ui/notifications_panel.dart';
import 'package:clarion_shell/ui/shell_panel_content.dart';

final _today = DateTime(2024, 2, 29, 15, 42);
ShellNotification _item(int id, {DateTime? at, bool actions = true}) =>
    ShellNotification(
      id: id,
      appName: '文本编辑器',
      summary: '文稿已保存',
      body: '本机文稿更新完成。',
      timestamp: at ?? _today,
      actions: actions
          ? const [
              NotificationAction('default', '打开'),
              NotificationAction('show', '显示文稿'),
            ]
          : const [],
    );

class _Notifications extends ShellController {
  _Notifications()
    : super(
        demo: true,
        notifications: false,
        tray: false,
        powerProfilesEnabled: false,
      );
  List<ShellNotification> entries = [];
  final actions = <(int, String)>[];
  final dismissed = <int>[];
  Completer<bool>? actionResult;
  @override
  List<ShellNotification> get notifications => entries;
  @override
  bool get notificationServerAvailable => true;
  void replace(List<ShellNotification> next) {
    entries = next;
    notifyListeners();
  }

  @override
  Future<bool> dismissNotification(
    int id, {
    ShellNotification? expected,
  }) async {
    if (expected != null && !entries.any((item) => identical(item, expected))) {
      return false;
    }
    dismissed.add(id);
    replace(entries.where((item) => item.id != id).toList());
    return true;
  }

  @override
  Future<bool> dismissNotifications(
    Iterable<ShellNotification> expected,
  ) async {
    final ids = expected
        .where((expected) => entries.any((item) => identical(item, expected)))
        .map((item) => item.id)
        .toSet();
    dismissed.addAll(ids);
    replace(entries.where((item) => !ids.contains(item.id)).toList());
    return ids.length == expected.length;
  }

  @override
  Future<bool> invokeNotificationAction(
    int id,
    String key, {
    ShellNotification? expected,
  }) async {
    if (expected != null && !entries.any((item) => identical(item, expected))) {
      return false;
    }
    actions.add((id, key));
    return actionResult == null ? true : actionResult!.future;
  }

  @override
  Future<bool> clearNotifications() async {
    replace([]);
    return true;
  }
}

void main() {
  late _Notifications controller;
  setUp(() {
    controller = _Notifications()
      ..setShellStyle(ShellStyle.macos)
      ..selectPanel(ShellPanel.notifications);
  });
  tearDown(() => controller.dispose());

  Future<void> mount(
    WidgetTester tester, {
    Size size = const Size(344, 700),
    Brightness brightness = Brightness.light,
    DateTime Function()? now,
  }) async {
    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = size;
    addTearDown(tester.view.resetDevicePixelRatio);
    addTearDown(tester.view.resetPhysicalSize);
    await tester.pumpWidget(
      MaterialApp(
        locale: const Locale('zh', 'CN'),
        supportedLocales: const [Locale('zh', 'CN')],
        localizationsDelegates: GlobalMaterialLocalizations.delegates,
        theme: buildMacShellTheme(brightness),
        home: RepaintBoundary(
          key: const ValueKey('capture-notification'),
          child: MacNotificationCenter(
            controller: controller,
            now: now ?? () => _today,
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
  }

  Future<void> notifications(WidgetTester tester) async {
    await tester.tap(find.byKey(const ValueKey('mac-notification-tab-1')));
    await tester.pumpAndSettle();
  }

  test(
    'month cells cover leap days, configured week start and year changes',
    () {
      final days = macCalendarMonthDays(DateTime(2024, 2), 1);
      expect(days.first, DateTime(2024, 1, 29));
      expect(days.last, DateTime(2024, 3, 3));
      expect(days.where((date) => date.month == 2).length, 29);
      expect(
        macCalendarMonthDays(DateTime(2024, 9), 0).first,
        DateTime(2024, 9, 1),
      );
      expect(
        macCalendarShiftMonth(DateTime(2024, 1, 31), 1),
        DateTime(2024, 2, 29),
      );
      expect(
        macCalendarShiftMonth(DateTime(2024, 1, 31), -1),
        DateTime(2023, 12, 31),
      );
    },
  );

  testWidgets(
    'Today uses local date and calendar supports month and keyboard navigation',
    (tester) async {
      await mount(tester);
      expect(find.text('2月29日'), findsOneWidget);
      expect(find.text('星期四'), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('mac-calendar-next')));
      await tester.pump();
      expect(find.text('2024年3月'), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('mac-calendar-today')));
      await tester.pump();
      final selected = find.byKey(
        ValueKey('mac-calendar-day-${DateTime(2024, 2, 29).toIso8601String()}'),
      );
      tester.widget<TextButton>(selected).focusNode!.requestFocus();
      await tester.pump();
      await tester.sendKeyEvent(LogicalKeyboardKey.arrowRight);
      await tester.pump();
      expect(find.text('2024年3月'), findsOneWidget);
      expect(
        tester
            .widget<Text>(find.byKey(const ValueKey('mac-calendar-selected')))
            .data,
        contains('3月1日'),
      );
      expect(controller.macNotificationView, MacNotificationView.today);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'tab selection survives close and reopen, with keyboard and real DND state',
    (tester) async {
      await mount(tester);
      await tester.sendKeyEvent(LogicalKeyboardKey.arrowRight);
      await tester.pumpAndSettle();
      expect(controller.macNotificationView, MacNotificationView.notifications);
      await tester.tap(find.byKey(const ValueKey('mac-notification-dnd')));
      await tester.pump();
      expect(controller.doNotDisturb, isTrue);
      controller.closePanel();
      await tester.pumpWidget(const SizedBox());
      controller.selectPanel(ShellPanel.notifications);
      await mount(tester);
      expect(find.text('没有新通知'), findsOneWidget);
      expect(
        tester
            .widget<CupertinoSwitch>(
              find.byKey(const ValueKey('mac-notification-dnd')),
            )
            .value,
        isTrue,
      );
    },
  );

  testWidgets(
    'local midnight updates Today and a calendar still following today',
    (tester) async {
      var time = DateTime(2024, 2, 29, 23, 59, 50);
      await mount(tester, now: () => time);
      time = DateTime(2024, 3, 1);
      await tester.pump(const Duration(seconds: 10));
      expect(find.text('3月1日'), findsOneWidget);
      expect(find.text('2024年3月'), findsOneWidget);
      expect(find.text('星期五'), findsOneWidget);
    },
  );

  testWidgets(
    'default and named actions dispatch once and stale row never acts on replacement',
    (tester) async {
      controller.replace([_item(1), _item(2, actions: false)]);
      await mount(tester);
      await notifications(tester);
      final captured = tester
          .widget<InkWell>(
            find.byKey(const ValueKey('mac-notification-open-1')),
          )
          .onTap!;
      controller.replace([_item(1), _item(2, actions: false)]);
      await tester.pump();
      captured();
      await tester.pumpAndSettle();
      expect(controller.actions, isEmpty);
      expect(controller.panel, ShellPanel.notifications);
      expect(
        tester
            .widget<InkWell>(
              find.byKey(const ValueKey('mac-notification-open-2')),
            )
            .onTap,
        isNull,
      );
      await tester.tap(
        find.byKey(const ValueKey('mac-notification-action-1-show')),
      );
      await tester.pumpAndSettle();
      expect(controller.actions, [(1, 'show')]);
      expect(controller.panel, ShellPanel.none);
      controller.selectPanel(ShellPanel.notifications);
      await tester.tap(find.byKey(const ValueKey('mac-notification-open-1')));
      await tester.pumpAndSettle();
      expect(controller.actions.last, (1, 'default'));
    },
  );

  testWidgets('pending and late actions cannot close a newly opened panel', (
    tester,
  ) async {
    controller.replace([_item(1)]);
    controller.actionResult = Completer<bool>();
    await mount(tester);
    await notifications(tester);
    final captured = tester
        .widget<InkWell>(find.byKey(const ValueKey('mac-notification-open-1')))
        .onTap!;
    captured();
    captured();
    await tester.pump();
    expect(controller.actions.length, 1);
    controller.selectPanel(ShellPanel.quickSettings);
    controller.actionResult!.complete(true);
    await tester.pumpAndSettle();
    expect(controller.panel, ShellPanel.quickSettings);
    expect(tester.takeException(), isNull);
  });

  testWidgets('single, date and all clear act on real notification collections', (
    tester,
  ) async {
    controller.replace([
      _item(1),
      _item(2),
      _item(3, at: DateTime(2024, 2, 28)),
    ]);
    await mount(tester);
    await notifications(tester);
    await tester.tap(find.byKey(const ValueKey('mac-notification-dismiss-1')));
    await tester.pumpAndSettle();
    expect(controller.dismissed, [1]);
    await tester.tap(
      find.byKey(
        ValueKey(
          'mac-notification-clear-day-${DateTime(2024, 2, 29).toIso8601String()}',
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(controller.dismissed, [1, 2]);
    expect(controller.entries.single.id, 3);
    await tester.tap(find.byKey(const ValueKey('mac-notification-clear-all')));
    await tester.pumpAndSettle();
    expect(controller.entries, isEmpty);
  });

  testWidgets('short narrow outputs scroll both views without overflow', (
    tester,
  ) async {
    controller.replace([_item(1)]);
    await mount(tester, size: const Size(180, 180));
    expect(tester.takeException(), isNull);
    await notifications(tester);
    await tester.drag(
      find.byKey(const PageStorageKey('mac-notification-list')),
      const Offset(0, -500),
    );
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'Mac sidebar is edge aligned with clear scrim; Material keeps original panel',
    (tester) async {
      tester.view.physicalSize = const Size(800, 600);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      await tester.pumpWidget(
        MaterialApp(
          theme: buildMacShellTheme(Brightness.light),
          home: MacShellView(controller: controller, preview: true),
        ),
      );
      await tester.pumpAndSettle();
      final rect = tester.getRect(
        find.byKey(const ValueKey('mac-notification-center')),
      );
      expect(rect.right, 800);
      expect(rect.bottom, 600);
      expect(rect.top, controller.macPreferences.menuBarHeight);
      final scrim = tester.widget<ColoredBox>(
        find
            .descendant(
              of: find.byKey(const ValueKey('mac-panel-scrim')),
              matching: find.byType(ColoredBox),
            )
            .first,
      );
      expect(scrim.color.a, 0);
      controller.setShellStyle(ShellStyle.material);
      controller.selectPanel(ShellPanel.notifications);
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(body: ShellPanelContent(controller: controller)),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.byType(NotificationsPanel), findsOneWidget);
      expect(find.byType(MacNotificationCenter), findsNothing);
    },
  );

  testWidgets(
    'native expansion survives sidebar exit without launcher backdrop',
    (tester) async {
      const window = MethodChannel('clarion.shell/window');
      final expanded = <bool>[];
      final backdrop = <bool>[];
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(window, (
        call,
      ) async {
        if (call.method == 'setExpanded') expanded.add(call.arguments as bool);
        if (call.method == 'setLauncherBackdrop') {
          backdrop.add((call.arguments as Map)['visible'] as bool);
        }
        return null;
      });
      addTearDown(
        () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          window,
          null,
        ),
      );
      await tester.pumpWidget(
        ShellApplication(
          controller: controller,
          preferences: const ShellPreferences(),
          persist: false,
          availableOutputSize: const Size(800, 600),
        ),
      );
      await tester.pumpAndSettle();
      expect(expanded.last, isTrue);
      expect(backdrop, isNot(contains(true)));
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      expect(controller.panel, ShellPanel.none);
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 60));
      expect(expanded.last, isTrue);
      controller.selectPanel(ShellPanel.notifications);
      await tester.pumpAndSettle();
      expect(expanded.last, isTrue);
      controller.closePanel();
      await tester.pumpAndSettle();
      expect(expanded.last, isFalse);
      expect(backdrop, isNot(contains(true)));
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
    },
  );

  testWidgets('optional actual widget captures', (tester) async {
    final destination =
        Platform.environment['CLARION_NOTIFICATION_PREVIEW_DIR'];
    if (destination == null) return;
    await tester.runAsync(() async {
      for (final font in {
        'ClarionSystem': 'assets/shell/ui-text.ttf',
        'ClarionChinese': 'assets/shell/cjk-regular.otf',
        'MaterialIcons':
            'build/unit_test_assets/fonts/MaterialIcons-Regular.otf',
      }.entries) {
        if (!File(font.value).existsSync()) continue;
        final loader = FontLoader(font.key)
          ..addFont(
            Future.value(
              ByteData.sublistView(await File(font.value).readAsBytes()),
            ),
          );
        await loader.load();
      }
    });
    controller.replace([
      _item(1),
      _item(2, at: _today.subtract(const Duration(days: 1)), actions: false),
    ]);
    for (final brightness in Brightness.values) {
      await mount(tester, brightness: brightness);
      for (final view in MacNotificationView.values) {
        controller.setMacNotificationView(view);
        await tester.pumpAndSettle();
        final boundary = tester.renderObject<RenderRepaintBoundary>(
          find.byKey(const ValueKey('capture-notification')),
        );
        await tester.runAsync(() async {
          final image = await boundary.toImage(pixelRatio: 1);
          final bytes = await image.toByteData(format: ui.ImageByteFormat.png);
          await Directory(destination).create(recursive: true);
          await File(
            '$destination/${brightness.name}-${view.name}.png',
          ).writeAsBytes(bytes!.buffer.asUint8List());
          image.dispose();
        });
      }
    }
  });
}
