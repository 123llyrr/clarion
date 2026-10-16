// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/app_menu_models.dart';
import 'package:clarion_shell/mac_shell_preferences.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/services/desktop_apps.dart';
import 'package:clarion_shell/services/global_app_menu_service.dart';
import 'package:clarion_shell/services/clarion_ipc.dart';
import 'package:clarion_shell/services/system_service.dart';
import 'package:clarion_shell/services/wallpaper_service.dart';
import 'package:clarion_shell/state/shell_controller.dart';

class _Ipc extends ClarionIpc {
  final connection = StreamController<bool>.broadcast(sync: true);
  final event = StreamController<Map<String, dynamic>>.broadcast(sync: true);
  final actions = <(String, Map<String, Object?>)>[];
  Completer<void>? nextGate;
  @override
  Stream<bool> get connections => connection.stream;
  @override
  Stream<Map<String, dynamic>> get events => event.stream;
  @override
  void start() {}
  @override
  Future<void> action(
    String name, [
    Map<String, Object?> arguments = const {},
  ]) async {
    actions.add((name, Map.of(arguments)));
    final gate = nextGate;
    nextGate = null;
    await gate?.future;
  }

  @override
  void dispose() {
    unawaited(connection.close());
    unawaited(event.close());
    super.dispose();
  }
}

class _Menus extends GlobalAppMenuService {
  _Menus() : super(hostRegistrar: false);
  @override
  Future<void> start() async {}
  @override
  Future<void> setActive(bool active) async {}
  @override
  Future<void> focusedTarget({
    int? pid,
    String appId = '',
    int? windowId,
    int? x11WindowId,
    AppMenuExport? export,
  }) async {}
}

class _Apps extends DesktopApps {
  @override
  Future<List<DesktopApp>> load() async => const [
    DesktopApp(id: 'a.desktop', name: 'A', desktopFile: '/fixture/a.desktop'),
    DesktopApp(id: 'b.desktop', name: 'B', desktopFile: '/fixture/b.desktop'),
    DesktopApp(id: 'c.desktop', name: 'C', desktopFile: '/fixture/c.desktop'),
    DesktopApp(id: 'd.desktop', name: 'D', desktopFile: '/fixture/d.desktop'),
  ];
}

class _System extends SystemService {
  int downloadsCalls = 0;
  bool failDownloads = false;
  Completer<void>? downloadsGate;
  @override
  Future<SystemSnapshot> read() async => const SystemSnapshot();
  @override
  Future<void> openDownloadsFolder() async {
    downloadsCalls++;
    await downloadsGate?.future;
    if (failDownloads) throw StateError('downloads unavailable');
  }
}

class _Wallpapers extends WallpaperService {
  @override
  Future<void> restore({bool applySaved = false}) async {}
}

Future<(ShellController, _Ipc, _System)> _fixture() async {
  final ipc = _Ipc();
  final system = _System();
  final controller = ShellController(
    ipc: ipc,
    system: system,
    desktopApps: _Apps(),
    appMenuService: _Menus(),
    wallpaperService: _Wallpapers(),
    notifications: false,
    tray: false,
    powerProfilesEnabled: false,
    restoreWallpaperOnStart: false,
  )..setShellStyle(ShellStyle.macos);
  addTearDown(controller.dispose);
  await controller.start();
  ipc.connection.add(true);
  ipc.event.add({
    'WindowsChanged': {
      'windows': [
        {
          'id': 91,
          'app_id': 'a',
          'title': 'A document',
          'is_focused': true,
          'is_minimized': false,
        },
      ],
    },
  });
  return (controller, ipc, system);
}

Future<void> _drain() async {
  for (var tick = 0; tick < 6; tick++) {
    await Future<void>.delayed(Duration.zero);
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  const channel = MethodChannel('clarion.shell/window');
  setUp(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
          channel,
          (call) async => call.method == 'outputName' ? 'QA-1' : null,
        );
  });
  tearDown(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, null);
  });

  test(
    'configuration updates preserve Material choices and the open panel',
    () {
      final controller = ShellController(
        demo: true,
        notifications: false,
        tray: false,
        powerProfilesEnabled: false,
      );
      addTearDown(controller.dispose);
      expect(controller.macPreferences, const MacShellPreferences());
      controller.setThemeMode(ThemeMode.light);
      controller.setBarPosition(BarPosition.bottom);
      controller.setReduceMotion(true);
      controller.setPinnedAppIds(['browser.desktop', 'terminal.desktop']);
      controller.selectPanel(ShellPanel.quickSettings);
      final pins = controller.pinnedAppIdsPreference;
      var notifications = 0;
      controller.addListener(() => notifications++);
      const configured = MacShellPreferences(
        dockPosition: MacDockPosition.left,
        dockSize: 40,
        showBatteryPercentage: true,
        clockShowSeconds: true,
        useOriginalIcons: false,
      );
      controller.setMacPreferences(configured);
      expect(controller.macPreferences, configured);
      expect(notifications, 1);
      expect(controller.shellStyle, ShellStyle.material);
      expect(controller.themeMode, ThemeMode.light);
      expect(controller.barPosition, BarPosition.bottom);
      expect(controller.reduceMotion, isTrue);
      expect(controller.pinnedAppIdsPreference, pins);
      expect(controller.panel, ShellPanel.quickSettings);
      controller.setMacPreferences(configured.copyWith());
      expect(notifications, 1);
      controller.setShellStyle(ShellStyle.macos);
      controller.setShellStyle(ShellStyle.material);
      expect(controller.macPreferences, configured);
      expect(controller.barPosition, BarPosition.bottom);
    },
  );

  test('menu-bar visibility is transient and auto-hide opt-in is enforced', () {
    final controller = ShellController(demo: true);
    addTearDown(controller.dispose);
    expect(controller.macMenuBarVisible, isTrue);
    var notifications = 0;
    controller.addListener(() => notifications++);
    controller.setMacMenuBarVisible(false);
    expect(controller.macMenuBarVisible, isTrue);
    expect(notifications, 0);
    controller.setMacPreferences(
      controller.macPreferences.copyWith(menuBarAutoHide: true),
    );
    controller.setMacMenuBarVisible(false);
    expect(controller.macMenuBarVisible, isFalse);
    final before = notifications;
    controller.setMacMenuBarVisible(false);
    expect(notifications, before);
    expect(
      controller.macPreferences.toJson().keys.any(
        (key) => key.contains('visible'),
      ),
      isFalse,
    );
    controller.setMacPreferences(
      controller.macPreferences.copyWith(menuBarAutoHide: false),
    );
    expect(controller.macMenuBarVisible, isTrue);
    controller.setMacPreferences(
      controller.macPreferences.copyWith(menuBarAutoHide: true),
    );
    expect(controller.macMenuBarVisible, isTrue);
    controller.setMacMenuBarVisible(false);
    controller.setMacMenuBarVisible(true);
    expect(controller.macMenuBarVisible, isTrue);
  });

  test(
    'Dock layout changes clear late old targets until fresh geometry',
    () async {
      final (controller, ipc, _) = await _fixture();
      const oldTarget = Rect.fromLTWH(30, 400, 48, 48);
      const freshTarget = Rect.fromLTWH(5, 120, 40, 40);
      final gate = ipc.nextGate = Completer<void>();
      final pending = controller.setDockMinimizeTargets({91: oldTarget});
      expect(ipc.actions, hasLength(1));
      final revision = controller.desktopRevision;
      controller.setMacPreferences(
        controller.macPreferences.copyWith(
          dockPosition: MacDockPosition.left,
          dockSize: 40,
        ),
      );
      expect(controller.desktopRevision, revision + 1);
      // A style switch before layout catches up must not replay old coordinates.
      controller.setShellStyle(ShellStyle.material);
      controller.setShellStyle(ShellStyle.macos);
      gate.complete();
      await pending;
      await _drain();
      expect(ipc.actions.last.$1, 'SetMinimizeTarget');
      expect(ipc.actions.last.$2['target'], isNull);
      await controller.setDockMinimizeTargets({91: freshTarget});
      expect(ipc.actions.last.$2['target'], {
        'output': 'QA-1',
        'x': 5.0,
        'y': 120.0,
        'width': 40.0,
        'height': 40.0,
        'edge': 'left',
        'effect': 'genie',
      });
    },
  );

  test(
    'display options retain targets, utility visibility invalidates them',
    () async {
      final (controller, ipc, _) = await _fixture();
      await controller.setDockMinimizeTargets({
        91: const Rect.fromLTWH(30, 400, 48, 48),
      });
      final calls = ipc.actions.length;
      final revision = controller.desktopRevision;
      controller.setMacPreferences(
        controller.macPreferences.copyWith(
          clockShowSeconds: true,
          useOriginalIcons: false,
          showBatteryPercentage: true,
          showRunningIndicators: false,
          animateOpeningApplications: false,
          menuBarWidthPercent: 75,
        ),
      );
      await _drain();
      expect(ipc.actions, hasLength(calls));
      expect(controller.desktopRevision, revision);
      controller.setMacPreferences(
        controller.macPreferences.copyWith(showDownloads: false),
      );
      await _drain();
      expect(controller.desktopRevision, revision + 1);
      expect(ipc.actions.last.$2['target'], isNull);
    },
  );

  test(
    'reordering only moves launchable pinned apps and preserves unknown pins',
    () async {
      final (controller, _, _) = await _fixture();
      controller.setPinnedAppIds([
        'a.desktop',
        'uninstalled.desktop',
        'b.desktop',
        'c.desktop',
      ]);
      var notifications = 0;
      controller.addListener(() => notifications++);
      controller.reorderPinnedApp('c.desktop', 'a.desktop');
      expect(controller.pinnedAppIdsPreference, [
        'c.desktop',
        'a.desktop',
        'uninstalled.desktop',
        'b.desktop',
      ]);
      expect(notifications, 1);
      for (final (app, before) in [
        ('c.desktop', 'a.desktop'),
        ('a.desktop', 'a.desktop'),
        ('uninstalled.desktop', 'a.desktop'),
        ('a.desktop', 'uninstalled.desktop'),
        ('d.desktop', 'a.desktop'),
        ('a.desktop', 'missing.desktop'),
      ]) {
        controller.reorderPinnedApp(app, before);
      }
      expect(notifications, 1);
      expect(
        () => controller.pinnedAppIdsPreference!.add('d.desktop'),
        throwsUnsupportedError,
      );
    },
  );

  test('Downloads action is serialized and preserves a newer panel', () async {
    final (controller, _, system) = await _fixture();
    controller.selectPanel(ShellPanel.launcher);
    system.downloadsGate = Completer<void>();
    final pending = controller.openDownloadsFolder();
    await controller.openDownloadsFolder();
    expect(system.downloadsCalls, 1);
    controller.selectPanel(ShellPanel.quickSettings);
    system.downloadsGate!.complete();
    await pending;
    expect(controller.panel, ShellPanel.quickSettings);
    system.downloadsGate = null;
    await controller.openDownloadsFolder();
    expect(system.downloadsCalls, 2);
    expect(controller.panel, ShellPanel.none);
  });

  test(
    'Downloads failure remains visible and demo never touches the system',
    () async {
      final (controller, _, system) = await _fixture();
      system.failDownloads = true;
      controller.selectPanel(ShellPanel.quickSettings);
      await controller.openDownloadsFolder();
      expect(controller.panel, ShellPanel.quickSettings);
      expect(controller.error, contains('downloads unavailable'));
      final demoSystem = _System();
      final demo = ShellController(demo: true, system: demoSystem);
      addTearDown(demo.dispose);
      await demo.openDownloadsFolder();
      expect(demoSystem.downloadsCalls, 0);
    },
  );
}
