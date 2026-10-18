// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/services/desktop_apps.dart';
import 'package:clarion_shell/services/clarion_ipc.dart';
import 'package:clarion_shell/services/system_service.dart';
import 'package:clarion_shell/services/tray_service.dart';
import 'package:clarion_shell/services/wallpaper_service.dart';
import 'package:clarion_shell/state/shell_controller.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  test(
    'successful overview closes the real panel; failures retain it for retry',
    () async {
      final ipc = _FakeIpc();
      final controller = ShellController(
        notifications: false,
        tray: false,
        wallpaperService: _FakeWallpapers(),
        ipc: ipc,
        system: _FakeSystem(),
        desktopApps: _FakeApps(),
      );
      addTearDown(controller.dispose);
      await controller.start();
      ipc.connection.add(true);
      controller.selectPanel(ShellPanel.quickSettings);
      await controller.toggleOverview();
      expect(ipc.actions.single.$1, 'ToggleOverview');
      expect(controller.panel, ShellPanel.none);
      expect(controller.error, null);
      controller.selectPanel(ShellPanel.quickSettings);
      ipc.fail = true;
      await controller.toggleOverview();
      expect(controller.panel, ShellPanel.quickSettings);
      expect(controller.error, contains('rejected'));
      expect(controller.busy, isEmpty);
      ipc.fail = false;
      await controller.toggleOverview();
      expect(controller.panel, ShellPanel.none);
      expect(controller.error, null);
    },
  );
  test(
    'demo is immediately available and never performs external actions',
    () async {
      final ipc = _FakeIpc();
      final system = _FakeSystem();
      final apps = _FakeApps();
      final controller = ShellController(
        demo: true,
        wallpaperService: _FakeWallpapers(),
        ipc: ipc,
        system: system,
        desktopApps: apps,
      );
      addTearDown(controller.dispose);
      expect(controller.connected, true);
      expect(controller.workspaces, isNotEmpty);
      expect(controller.apps, isNotEmpty);
      await controller.start();
      await controller.switchWorkspace(2);
      await controller.setVolume(.3);
      await controller.toggleMute();
      await controller.toggleWifi();
      await controller.launchApp(controller.apps.first);
      await controller.openSettings();
      await controller.toggleOverview();
      controller.setThemeMode(ThemeMode.light);
      controller.setReduceMotion(true);
      expect(controller.workspaces.singleWhere((w) => w.isFocused).id, 2);
      expect(controller.volume, .3);
      expect(controller.muted, true);
      expect(controller.wifiEnabled, false);
      expect(ipc.actions, isEmpty);
      expect(system.writes, isEmpty);
      expect(system.reads, 0);
      expect(apps.launches, 0);
      expect(controller.reduceMotion, true);
      expect(controller.trayItems, isNotEmpty);
      await controller.activateTrayItem(controller.trayItems.first);
      expect(controller.panel, ShellPanel.trayMenu);
      expect(controller.trayMenu, isNotEmpty);
      await controller.activateTrayMenuEntry(controller.trayMenu.first.id);
      expect(controller.panel, ShellPanel.none);
    },
  );
  test(
    'tray activation falls back to the overlay menu and disappearing items close it',
    () async {
      final tray = _FakeTray();
      final controller = ShellController(
        notifications: false,
        trayService: tray,
        wallpaperService: _FakeWallpapers(),
        ipc: _FakeIpc(),
        system: _FakeSystem(),
        desktopApps: _FakeApps(),
      );
      addTearDown(controller.dispose);
      await controller.start();
      expect(tray.operations, ['start']);
      const item = TrayItem(
        id: 'chat',
        title: '消息',
        menuAvailable: true,
        menuPath: '/Menu',
      );
      tray.itemsOverride = const [item];
      tray.emit();
      await Future<void>.delayed(Duration.zero);
      expect(controller.trayItems.single.id, 'chat');
      tray.failActivate = true;
      await controller.activateTrayItem(item);
      expect(tray.operations, ['start', 'invoke:activate', 'menu']);
      expect(controller.panel, ShellPanel.trayMenu);
      expect(controller.trayMenu.single.label, '打开');
      tray.itemsOverride = const [];
      tray.emit();
      await Future<void>.delayed(Duration.zero);
      expect(controller.panel, ShellPanel.none);
      expect(controller.selectedTrayItem, isNull);
    },
  );
  test(
    'stable workspace IDs and output filters survive incremental changes',
    () async {
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(
            const MethodChannel('clarion.shell/window'),
            (_) async => 'DP-1',
          );
      addTearDown(
        () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
            .setMockMethodCallHandler(
              const MethodChannel('clarion.shell/window'),
              null,
            ),
      );
      final ipc = _FakeIpc();
      final controller = ShellController(
        notifications: false,
        tray: false,
        wallpaperService: _FakeWallpapers(),
        ipc: ipc,
        system: _FakeSystem(),
        desktopApps: _FakeApps(),
      );
      addTearDown(controller.dispose);
      await controller.start();
      ipc.connection.add(true);
      ipc.event.add({
        'WorkspacesChanged': {
          'workspaces': [
            {'id': 991, 'idx': 2, 'output': 'DP-1', 'is_focused': false},
            {'id': 55, 'idx': 1, 'output': 'eDP-1', 'is_focused': true},
          ],
        },
      });
      ipc.event.add({
        'WindowsChanged': {
          'windows': [
            {
              'id': 42,
              'workspace_id': 991,
              'is_focused': true,
              'title': 'Editor',
            },
          ],
        },
      });
      expect(controller.workspaces.map((w) => w.id), [991]);
      expect(controller.workspaces.single.windowCount, 1);
      expect(controller.windowTitle, 'Editor');
      ipc.event.add({
        'WorkspaceActivated': {'id': 991, 'focused': true},
      });
      await controller.switchWorkspace(991);
      expect(ipc.actions.single.$1, 'FocusWorkspace');
      expect(ipc.actions.single.$2, {
        'reference': {'Id': 991},
      });
      ipc.event.add({
        'WindowOpenedOrChanged': {
          'window': {
            'id': 13,
            'title': <String>['bad'],
          },
        },
      });
      expect(controller.windowTitle, 'Editor');
      ipc.event.add({
        'WorkspacesChanged': {
          'workspaces': [
            {'id': 'bad', 'idx': 1, 'output': 'DP-1'},
            {'id': 10, 'idx': 2, 'name': 7, 'output': 'DP-1'},
            {'id': 55, 'idx': 1, 'output': 'eDP-1', 'is_focused': true},
          ],
        },
      });
      expect(controller.workspaces.map((w) => w.id), [55]);
      ipc.event.add({
        'WorkspacesChanged': {'workspaces': []},
      });
      ipc.event.add({
        'WindowClosed': {'id': 42},
      });
      ipc.event.add({
        'WorkspaceActivated': {'id': 991, 'focused': true},
      });
      ipc.event.add({'FutureNewEvent': {}});
      expect(controller.workspaces, isEmpty);
      expect(controller.windowTitle, 'Clarion');
      ipc.connection.add(false);
      expect(controller.connected, false);
      await controller.switchWorkspace(991);
      expect(controller.error, contains('不可用'));
      expect(ipc.actions.length, 1);
    },
  );
  test(
    'slider writes coalesce at 90 ms and always commit the final value',
    () async {
      final system = _FakeSystem();
      final controller = ShellController(
        notifications: false,
        tray: false,
        wallpaperService: _FakeWallpapers(),
        ipc: _FakeIpc(),
        system: system,
        desktopApps: _FakeApps(),
      );
      addTearDown(controller.dispose);
      await controller.start();
      final pending = [
        controller.setVolume(.2),
        controller.setVolume(.4),
        controller.setVolume(.8),
      ];
      expect(system.writes, isEmpty);
      await Future.wait(pending);
      expect(system.writes, [.8]);
      expect(controller.volume, .8);
      expect(controller.busy, isEmpty);
      system.fail = true;
      await controller.setVolume(.3);
      expect(controller.error, contains('failed'));
      expect(controller.volume, .8);
      expect(controller.busy, isEmpty);
    },
  );
  test('dispose resolves queued slider callers without writing', () async {
    final system = _FakeSystem();
    final controller = ShellController(
      notifications: false,
      tray: false,
      wallpaperService: _FakeWallpapers(),
      ipc: _FakeIpc(),
      system: system,
      desktopApps: _FakeApps(),
    );
    await controller.start();
    final pending = controller.setVolume(.2);
    controller.dispose();
    await pending;
    await Future<void>.delayed(const Duration(milliseconds: 130));
    expect(system.writes, isEmpty);
  });
}

class _FakeTray extends TrayService {
  final tokens = <String, Object>{};
  @override
  Object? registrationToken(TrayItem item) =>
      itemsOverride.any((entry) => entry.id == item.id)
      ? tokens.putIfAbsent(item.id, Object.new)
      : null;
  final operations = <String>[];
  var itemsOverride = <TrayItem>[];
  var failActivate = false;
  final events = StreamController<void>.broadcast(sync: true);
  @override
  List<TrayItem> get items => itemsOverride;
  @override
  Stream<void> get changes => events.stream;
  @override
  Future<bool> start() async {
    operations.add('start');
    available = true;
    return true;
  }

  @override
  Future<bool> invoke(
    TrayItem item,
    TrayAction action, {
    double x = 0,
    double y = 0,
  }) async {
    operations.add('invoke:${action.name}');
    return !failActivate;
  }

  @override
  Future<List<TrayMenuEntry>?> loadMenu(TrayItem item) async {
    operations.add('menu');
    return const [TrayMenuEntry(id: 1, label: '打开')];
  }

  void emit() => events.add(null);

  @override
  void dispose() {
    unawaited(events.close());
  }
}

class _FakeWallpapers extends WallpaperService {
  @override
  Future<void> restore({bool applySaved = false}) async {}
}

class _FakeIpc extends ClarionIpc {
  bool fail = false;
  final event = StreamController<Map<String, dynamic>>.broadcast(sync: true);
  final connection = StreamController<bool>.broadcast(sync: true);
  final actions = <(String, Map<String, Object?>)>[];
  @override
  Stream<Map<String, dynamic>> get events => event.stream;
  @override
  Stream<bool> get connections => connection.stream;
  @override
  void start() {}
  @override
  Future<void> action(
    String name, [
    Map<String, Object?> arguments = const {},
  ]) async {
    actions.add((name, arguments));
    if (fail) throw StateError('rejected');
  }

  @override
  void dispose() {
    unawaited(event.close());
    unawaited(connection.close());
    super.dispose();
  }
}

class _FakeApps extends DesktopApps {
  var launches = 0;
  @override
  Future<List<DesktopApp>> load() async => [];
  @override
  Future<void> launch(DesktopApp app) async {
    launches++;
  }
}

class _FakeSystem extends SystemService {
  var reads = 0;
  var fail = false;
  var value = .6;
  final writes = <double>[];
  @override
  Future<SystemSnapshot> read() async {
    reads++;
    return SystemSnapshot(
      volume: value,
      brightness: .5,
      wifiEnabled: true,
      bluetoothEnabled: true,
    );
  }

  @override
  Future<void> setVolume(double value) async {
    if (fail) throw StateError('failed');
    writes.add(value);
    this.value = value;
  }
}
