// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:clarion_shell/mac_notification_view.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/services/clarion_ipc.dart';
import 'package:clarion_shell/services/desktop_apps.dart';
import 'package:clarion_shell/services/notification_service.dart';
import 'package:clarion_shell/services/system_service.dart';
import 'package:clarion_shell/services/wallpaper_service.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/theme/mac_shell_theme.dart';
import 'package:clarion_shell/ui/mac_shell_view.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

class _LocalNotifications extends NotificationService {
  _LocalNotifications() {
    available = true;
  }
  @override
  Future<bool> start() async => true;
}

class _Ipc extends ClarionIpc {
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
  @override
  Stream<void> get changes => const Stream<void>.empty();
  @override
  Future<void> restore({bool applySaved = false}) async {}
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  setUp(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
          const MethodChannel('clarion.shell/window'),
          (_) async => null,
        );
  });
  tearDown(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
          const MethodChannel('clarion.shell/window'),
          null,
        );
  });

  Future<(ShellController, _LocalNotifications)> create() async {
    final service = _LocalNotifications();
    final controller = ShellController(
      notificationService: service,
      tray: false,
      powerProfilesEnabled: false,
      mediaEnabled: false,
      ipc: _Ipc(),
      system: _System(),
      desktopApps: _Apps(),
      wallpaperService: _Wallpapers(),
    );
    addTearDown(controller.dispose);
    controller.setShellStyle(ShellStyle.macos);
    await controller.start();
    return (controller, service);
  }

  void arrive(NotificationService service, String filename) {
    service.showScreenshot('/tmp/$filename.png', openLocation: (_) async {});
  }

  test(
    'calendar does not mark unseen notifications read; the notification list does',
    () async {
      final (controller, service) = await create();
      arrive(service, 'First');
      expect(controller.unreadNotificationCount, 1);
      controller.selectPanel(ShellPanel.notifications);
      expect(controller.macNotificationView, MacNotificationView.today);
      expect(controller.unreadNotificationCount, 1);
      controller.setMacNotificationView(MacNotificationView.notifications);
      expect(controller.unreadNotificationCount, 0);
      expect(service.notifications.single.actions, hasLength(2));
      arrive(service, 'Already visible');
      expect(controller.unreadNotificationCount, 0);
      controller.closePanel();
      arrive(service, 'Unseen');
      expect(controller.unreadNotificationCount, 1);
      controller.selectPanel(ShellPanel.notifications);
      expect(controller.unreadNotificationCount, 0);
      expect(service.notifications, hasLength(3));
    },
  );

  test(
    'Do Not Disturb preserves unread history; explicit reading and clearing are separate',
    () async {
      final (controller, service) = await create();
      controller.setDoNotDisturb(true);
      arrive(service, 'Quiet');
      expect(controller.unreadNotificationCount, 1);
      controller.setDoNotDisturb(false);
      expect(controller.unreadNotificationCount, 1);
      controller.markNotificationsRead();
      expect(controller.unreadNotificationCount, 0);
      expect(service.notifications, hasLength(1));
      arrive(service, 'Next');
      expect(controller.unreadNotificationCount, 1);
      expect(await controller.clearNotifications(), isTrue);
      expect(controller.unreadNotificationCount, 0);
      expect(service.notifications, isEmpty);
    },
  );

  testWidgets(
    'bar badge follows unread arrivals, reading, and DND without deleting history',
    (tester) async {
      final (controller, service) = await create();
      controller.setShellStyle(ShellStyle.macos);
      controller.setReduceMotion(true);
      tester.view.devicePixelRatio = 1;
      tester.view.physicalSize = const Size(1280, 800);
      addTearDown(tester.view.resetDevicePixelRatio);
      addTearDown(tester.view.resetPhysicalSize);
      await tester.pumpWidget(
        MaterialApp(
          theme: buildMacShellTheme(Brightness.light),
          home: MacShellView(controller: controller, preview: true),
        ),
      );
      await tester.pumpAndSettle();
      final badge = find.byKey(const ValueKey('mac-notification-unread-badge'));
      expect(tester.widget<Badge>(badge).isLabelVisible, isFalse);
      arrive(service, 'First');
      await tester.pump(const Duration(milliseconds: 25));
      await tester.pump();
      expect(tester.widget<Badge>(badge).isLabelVisible, isTrue);
      controller.setDoNotDisturb(true);
      await tester.pump();
      expect(tester.widget<Badge>(badge).isLabelVisible, isFalse);
      controller.setDoNotDisturb(false);
      await tester.pump();
      expect(tester.widget<Badge>(badge).isLabelVisible, isTrue);
      controller.selectPanel(ShellPanel.notifications);
      controller.setMacNotificationView(MacNotificationView.notifications);
      await tester.pumpAndSettle();
      expect(tester.widget<Badge>(badge).isLabelVisible, isFalse);
      expect(service.notifications, hasLength(1));
      controller.closePanel();
      arrive(service, 'Second');
      await tester.pump(const Duration(milliseconds: 25));
      await tester.pump();
      expect(tester.widget<Badge>(badge).isLabelVisible, isTrue);
      expect(service.notifications, hasLength(2));
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
      controller.dispose();
      await tester.pump();
    },
  );
}
