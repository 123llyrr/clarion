// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:realand_shell/models.dart';
import 'package:realand_shell/services/desktop_apps.dart';
import 'package:realand_shell/services/notification_service.dart';
import 'package:realand_shell/services/realand_ipc.dart';
import 'package:realand_shell/services/system_service.dart';
import 'package:realand_shell/services/wallpaper_service.dart';
import 'package:realand_shell/state/shell_controller.dart';

ShellNotification _item(String summary, {int id = 1}) => ShellNotification(
  id: id,
  appName: 'Test app',
  summary: summary,
  body: '',
  timestamp: DateTime(2026, 10, 9),
  actions: const [NotificationAction('default', 'Open')],
);

class _Notifications extends NotificationService {
  _Notifications() {
    available = true;
  }
  final updates = StreamController<void>.broadcast();
  List<ShellNotification> items = [_item('Original')];
  Completer<bool>? gate;
  var actions = 0;
  var dismissals = 0;
  var clears = 0;
  var batches = 0;
  bool disposed = false;
  @override
  List<ShellNotification> get notifications => items;
  @override
  Stream<void> get changes => updates.stream;
  @override
  Future<bool> start() async => available;
  @override
  Future<bool> invokeAction(
    int id,
    String key, {
    ShellNotification? expected,
  }) async {
    actions++;
    if (items.isEmpty ||
        key != 'default' ||
        (expected != null && !identical(items.single, expected))) {
      return false;
    }
    final result = await (gate?.future ?? Future.value(true));
    if (result) items = [];
    return result;
  }

  @override
  Future<bool> dismiss(int id, {ShellNotification? expected}) async {
    dismissals++;
    if (items.isEmpty ||
        (expected != null && !identical(items.single, expected))) {
      return false;
    }
    items = [];
    return true;
  }

  @override
  Future<bool> clear() async {
    clears++;
    items = [];
    return true;
  }

  @override
  Future<bool> dismissAll(Iterable<ShellNotification> expected) async {
    batches++;
    final snapshot = expected.toSet();
    final allCurrent = snapshot.every(items.contains);
    items = items.where((item) => !snapshot.contains(item)).toList();
    return allCurrent;
  }

  @override
  void dispose() {
    if (disposed) return;
    disposed = true;
    unawaited(updates.close());
    super.dispose();
  }
}

class _Ipc extends RealandIpc {
  @override
  void start() {}
}

class _System extends SystemService {
  @override
  Future<SystemSnapshot> read() async => const SystemSnapshot();
}

class _Apps extends DesktopApps {
  @override
  Future<List<DesktopApp>> load() async => [];
}

class _Wallpapers extends WallpaperService {
  // Notification update counts must not include the host's asynchronous
  // wallpaper-backend availability probe.
  @override
  Stream<void> get changes => const Stream<void>.empty();
  @override
  Future<void> restore({bool applySaved = false}) async {}
}

ShellController _controller(_Notifications service) {
  final controller = ShellController(
    notificationService: service,
    tray: false,
    powerProfilesEnabled: false,
    ipc: _Ipc(),
    system: _System(),
    desktopApps: _Apps(),
    wallpaperService: _Wallpapers(),
  );
  addTearDown(controller.dispose);
  return controller;
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  setUp(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
          const MethodChannel('realand.shell/window'),
          (_) async => null,
        );
  });
  tearDown(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
          const MethodChannel('realand.shell/window'),
          null,
        );
  });

  test(
    'actions return actual success and block duplicate operations while busy',
    () async {
      final service = _Notifications()..gate = Completer<bool>();
      final controller = _controller(service);
      final item = service.items.single;
      final action = controller.invokeNotificationAction(
        1,
        'default',
        expected: item,
      );
      expect(controller.busy, contains('notifications'));
      expect(
        await controller.invokeNotificationAction(1, 'default', expected: item),
        false,
      );
      expect(await controller.dismissNotification(1, expected: item), false);
      expect(await controller.clearNotifications(), false);
      expect(await controller.dismissNotifications([item]), false);
      expect(service.actions, 1);
      expect(service.dismissals, 0);
      expect(service.clears, 0);
      expect(service.batches, 0);
      service.gate!.complete(true);
      expect(await action, true);
      expect(controller.busy, isNot(contains('notifications')));
      expect(controller.notifications, isEmpty);
    },
  );

  test('stale rows cannot invoke or remove a same-ID replacement', () async {
    final service = _Notifications();
    final controller = _controller(service);
    final stale = service.items.single;
    service.items = [_item('Replacement')];
    expect(
      await controller.invokeNotificationAction(1, 'default', expected: stale),
      false,
    );
    expect(await controller.dismissNotification(1, expected: stale), false);
    expect(controller.notifications.single.summary, 'Replacement');
    expect(
      await controller.dismissNotification(1, expected: service.items.single),
      true,
    );
    expect(controller.notifications, isEmpty);
  });

  test('failure stays on originating panel and remains retryable', () async {
    final service = _Notifications()..gate = Completer<bool>();
    final controller = _controller(service);
    controller.selectPanel(ShellPanel.notifications);
    final action = controller.invokeNotificationAction(1, 'default');
    service.gate!.completeError(StateError('sender disconnected'));
    expect(await action, false);
    expect(controller.panel, ShellPanel.notifications);
    expect(controller.error, contains('sender disconnected'));
    expect(controller.busy, isEmpty);
    expect(controller.notifications, hasLength(1));
    service.gate = null;
    expect(await controller.invokeNotificationAction(1, 'default'), true);
    expect(controller.error, isNull);
  });

  test('late failure cannot overwrite a newly opened panel', () async {
    final service = _Notifications()..gate = Completer<bool>();
    final controller = _controller(service);
    controller.selectPanel(ShellPanel.notifications);
    final action = controller.invokeNotificationAction(1, 'default');
    controller.selectPanel(ShellPanel.quickSettings);
    service.gate!.completeError(StateError('late error'));
    expect(await action, false);
    expect(controller.panel, ShellPanel.quickSettings);
    expect(controller.error, isNull);
    expect(controller.busy, isEmpty);
  });

  test(
    'unavailable service reports failure without pretending to clear',
    () async {
      final service = _Notifications()..available = false;
      final controller = _controller(service);
      expect(await controller.clearNotifications(), false);
      expect(service.clears, 0);
      expect(controller.error, contains('通知服务暂时不可用'));
      expect(controller.notifications, hasLength(1));
    },
  );

  test(
    'late action completion after disposal cannot report success or notify',
    () async {
      final service = _Notifications()..gate = Completer<bool>();
      final controller = _controller(service);
      final action = controller.invokeNotificationAction(1, 'default');
      controller.dispose();
      service.gate!.complete(true);
      expect(await action, false);
      expect(await controller.dismissNotification(1), false);
      expect(await controller.clearNotifications(), false);
    },
  );

  test(
    'listener reuses service snapshots and ignores unchanged notifications',
    () async {
      final service = _Notifications();
      final controller = _controller(service);
      await controller.start();
      expect(identical(controller.notifications, service.items), true);
      var updates = 0;
      controller.addListener(() => updates++);
      service.updates.add(null);
      await Future<void>.delayed(Duration.zero);
      expect(updates, 0);
      service.items = [_item('Changed')];
      service.updates.add(null);
      await Future<void>.delayed(Duration.zero);
      expect(updates, 1);
      expect(identical(controller.notifications, service.items), true);
      controller.dispose();
    },
  );

  test(
    'group clear performs one service batch and two controller updates',
    () async {
      final service = _Notifications()
        ..items = List.generate(
          100,
          (i) => _item('Notification $i', id: i + 1),
        );
      final controller = _controller(service);
      var updates = 0;
      controller.addListener(() => updates++);
      expect(await controller.dismissNotifications(service.items), true);
      expect(service.batches, 1);
      expect(service.dismissals, 0);
      expect(updates, 2);
      expect(controller.notifications, isEmpty);
    },
  );
}
