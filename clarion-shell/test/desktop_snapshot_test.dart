// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/app_menu_models.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/services/desktop_apps.dart';
import 'package:clarion_shell/services/global_app_menu_service.dart';
import 'package:clarion_shell/services/clarion_ipc.dart';
import 'package:clarion_shell/services/system_service.dart';
import 'package:clarion_shell/services/wallpaper_service.dart';
import 'package:clarion_shell/state/shell_controller.dart';

class _Ipc extends ClarionIpc {
  final connectionsController = StreamController<bool>.broadcast(sync: true);
  final eventsController = StreamController<Map<String, dynamic>>.broadcast(
    sync: true,
  );
  @override
  Stream<bool> get connections => connectionsController.stream;
  @override
  Stream<Map<String, dynamic>> get events => eventsController.stream;
  @override
  void start() {}
  @override
  Future<void> action(
    String name, [
    Map<String, Object?> args = const {},
  ]) async {}
  @override
  void dispose() {
    unawaited(connectionsController.close());
    unawaited(eventsController.close());
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

class _CountedApp extends DesktopApp {
  _CountedApp(int number, {String? name})
    : super(
        id: 'app-$number.desktop',
        name: name ?? 'App $number',
        desktopFile: '',
      );
  static int idReads = 0;
  @override
  String get id {
    idReads++;
    return super.id;
  }
}

class _Apps extends DesktopApps {
  _Apps(int count) : entries = [for (var i = 0; i < count; i++) _CountedApp(i)];
  List<DesktopApp> entries;
  @override
  Future<List<DesktopApp>> load() async => entries;
}

class _System extends SystemService {
  @override
  Future<SystemSnapshot> read() async =>
      const SystemSnapshot(volume: .5, brightness: .5);
}

class _Wallpapers extends WallpaperService {
  @override
  Future<void> restore({bool applySaved = false}) async {}
}

Map<String, dynamic> _window(
  int id, {
  String? appId,
  String? title,
  bool focused = false,
  bool minimized = false,
  int workspace = 1,
  Map<String, Object?>? menu,
}) => {
  'id': id,
  'app_id': appId ?? 'app-$id',
  'title': title ?? 'Document $id',
  'workspace_id': workspace,
  'is_focused': focused,
  'is_minimized': minimized,
  'app_menu': ?menu,
};

Future<(ShellController, _Ipc, _Apps)> _fixture({int apps = 8}) async {
  final ipc = _Ipc();
  final catalogue = _Apps(apps);
  final controller = ShellController(
    ipc: ipc,
    desktopApps: catalogue,
    appMenuService: _Menus(),
    system: _System(),
    wallpaperService: _Wallpapers(),
    notifications: false,
    tray: false,
    powerProfilesEnabled: false,
    restoreWallpaperOnStart: false,
  )..setPinnedAppIds(['app-0.desktop']);
  addTearDown(controller.dispose);
  await controller.start();
  ipc.connectionsController.add(true);
  ipc.eventsController.add({
    'WorkspacesChanged': {
      'workspaces': [
        {
          'id': 1,
          'idx': 1,
          'output': 'QA-1',
          'name': 'One',
          'is_active': true,
          'is_focused': true,
        },
        {'id': 2, 'idx': 2, 'output': 'QA-1', 'name': 'Two'},
      ],
    },
  });
  return (controller, ipc, catalogue);
}

Future<void> _drain() async {
  for (var tick = 0; tick < 10; tick++) {
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
    'test controllers never access or bind the desktop command socket',
    () async {
      expect(Platform.environment.containsKey('FLUTTER_TEST'), isTrue);
      final touched = <String>[];
      final controller = ShellController(
        ipc: _Ipc(),
        desktopApps: _Apps(0),
        appMenuService: _Menus(),
        system: _System(),
        wallpaperService: _Wallpapers(),
        notifications: false,
        tray: false,
        powerProfilesEnabled: false,
        restoreWallpaperOnStart: false,
      );
      addTearDown(controller.dispose);
      await IOOverrides.runZoned(
        () async {
          await controller.start();
          await _drain();
          controller.dispose();
        },
        createFile: (path) {
          touched.add(path);
          throw FileSystemException(
            'No filesystem access in this fixture',
            path,
          );
        },
        serverSocketBind:
            (address, port, {backlog = 0, v6Only = false, shared = false}) {
              touched.add('bind $address');
              throw const SocketException('No command server in tests');
            },
      );
      expect(touched, isEmpty);
    },
  );

  test(
    'repeated Dock frame reads never rescan an unchanged large catalogue',
    () async {
      final (controller, ipc, _) = await _fixture(apps: 1000);
      ipc.eventsController.add({
        'WindowsChanged': {
          'windows': [
            for (var id = 950; id < 1000; id++) _window(id, focused: id == 999),
          ],
        },
      });
      controller.setRecentAppIds([
        'app-10.desktop',
        'app-11.desktop',
        'app-12.desktop',
      ]);
      await _drain();
      // Warm the same reads used by Dock layout, hover-removal checks and menu
      // tracking. Timing is diagnostic only; the stable no-rescan assertion does
      // not depend on host load or a debug/JIT wall-clock threshold.
      for (var i = 0; i < 4; i++) {
        expect(controller.dockApps, isNotEmpty);
        expect(controller.recentDockApps, hasLength(3));
        expect(controller.windows, hasLength(50));
        expect(controller.workspaces, hasLength(2));
      }
      _CountedApp.idReads = 0;
      final timer = Stopwatch()..start();
      var consumed = 0;
      for (var frame = 0; frame < 120; frame++) {
        consumed += controller.dockApps.length;
        consumed += controller.recentDockApps.length;
        consumed += controller.windows.length;
        consumed += controller.workspaces.length;
      }
      timer.stop();
      debugPrint(
        'Dock snapshot reads: 120 frames, ${timer.elapsedMicroseconds} us, '
        '${_CountedApp.idReads} catalogue ID reads, checksum $consumed',
      );
      expect(consumed, 120 * 106);
      expect(_CountedApp.idReads, 0);
    },
  );

  test(
    'desktop events replace immutable snapshots and preserve older readers',
    () async {
      final (controller, ipc, _) = await _fixture();
      ipc.eventsController.add({
        'WindowsChanged': {
          'windows': [_window(2), _window(1, focused: true)],
        },
      });
      final before = controller.windows;
      final appsBefore = controller.dockApps;
      final workspacesBefore = controller.workspaces;
      expect(before.map((w) => w.id), [1, 2]);
      expect(() => before.clear(), throwsUnsupportedError);
      expect(() => appsBefore.clear(), throwsUnsupportedError);
      expect(() => appsBefore.last.windows.clear(), throwsUnsupportedError);
      expect(() => workspacesBefore.clear(), throwsUnsupportedError);
      controller.setDoNotDisturb(true);
      expect(controller.windows, same(before));
      expect(controller.dockApps, same(appsBefore));
      expect(controller.workspaces, same(workspacesBefore));
      ipc.eventsController.add({
        'WindowOpenedOrChanged': {
          'window': _window(
            1,
            title: 'Renamed',
            focused: true,
            minimized: true,
            workspace: 2,
            menu: {'bus_name': ':1.99', 'menu_path': '/Menu'},
          ),
        },
      });
      expect(controller.windows.first.title, 'Renamed');
      expect(controller.windows.first.isMinimized, isTrue);
      expect(controller.windows.first.appMenu?.busName, ':1.99');
      expect(
        controller.dockApps
            .firstWhere((app) => app.id == 'app-1.desktop')
            .windows
            .single
            .workspaceId,
        2,
      );
      expect(controller.workspaces.map((ws) => ws.windowCount), [1, 1]);
      expect(before.first.title, 'Document 1');
      expect(workspacesBefore.first.windowCount, 2);
      ipc.eventsController.add({
        'WindowFocusChanged': {'id': 2},
      });
      expect(controller.focusedWindow?.id, 2);
      ipc.eventsController.add({
        'WindowClosed': {'id': 1},
      });
      expect(controller.windows.map((w) => w.id), [2]);
      expect(controller.dockApps.map((app) => app.id), [
        'app-0.desktop',
        'app-2.desktop',
      ]);
      ipc.eventsController.add({
        'WorkspaceActivated': {'id': 2, 'focused': true},
      });
      expect(controller.workspaces.last.isFocused, isTrue);
      ipc.connectionsController.add(false);
      expect(controller.windows, isEmpty);
      expect(controller.workspaces, isEmpty);
      expect(controller.dockApps.map((app) => app.id), ['app-0.desktop']);
      ipc.connectionsController.add(true);
      ipc.eventsController.add({
        'WindowsChanged': {
          'windows': [_window(1, title: 'Reused ID')],
        },
      });
      expect(controller.windows.single.title, 'Reused ID');
    },
  );

  test(
    'pin, recent, visibility and catalogue updates invalidate their own snapshots',
    () async {
      final (controller, ipc, catalogue) = await _fixture();
      ipc.eventsController.add({
        'WindowsChanged': {
          'windows': [_window(1)],
        },
      });
      controller.setRecentAppIds([
        'app-2.desktop',
        'app-3.desktop',
        'app-4.desktop',
      ]);
      expect(controller.recentDockApps.map((app) => app.id), [
        'app-2.desktop',
        'app-3.desktop',
        'app-4.desktop',
      ]);
      final windows = controller.windows;
      controller.setPinnedAppIds(['app-3.desktop', 'app-2.desktop']);
      expect(controller.dockApps.map((app) => app.id), [
        'app-3.desktop',
        'app-2.desktop',
        'app-1.desktop',
      ]);
      expect(controller.recentDockApps.map((app) => app.id), ['app-4.desktop']);
      expect(controller.windows, same(windows));
      controller.setRecentAppIds(['app-5.desktop', 'app-6.desktop']);
      expect(controller.recentDockApps.map((app) => app.id), [
        'app-5.desktop',
        'app-6.desktop',
      ]);
      controller.setMacPreferences(
        controller.macPreferences.copyWith(showRecentApplications: false),
      );
      expect(controller.recentDockApps, isEmpty);
      controller.setMacPreferences(
        controller.macPreferences.copyWith(showRecentApplications: true),
      );
      expect(controller.recentDockApps.map((app) => app.id), [
        'app-5.desktop',
        'app-6.desktop',
      ]);
      catalogue.entries = [
        _CountedApp(1, name: 'New app name'),
        _CountedApp(5, name: 'New recent name'),
      ];
      controller.selectPanel(ShellPanel.launcher);
      await _drain();
      expect(controller.dockApps.single.name, 'New app name');
      expect(controller.recentDockApps.single.name, 'New recent name');
      expect(() => controller.apps.clear(), throwsUnsupportedError);
      catalogue.entries.clear();
      expect(controller.apps, hasLength(2));
      expect(controller.dockApps.single.name, 'New app name');
      expect(controller.recentDockApps.single.name, 'New recent name');
    },
  );

  test(
    'demo restore announces fresh state before its workspace notification',
    () async {
      final controller = ShellController(
        demo: true,
        notifications: false,
        tray: false,
        powerProfilesEnabled: false,
      );
      addTearDown(controller.dispose);
      await controller.minimizeWindow(1);
      expect(controller.windows.single.isMinimized, isTrue);
      final announcements = <bool>[];
      controller.addListener(
        () => announcements.add(controller.windows.single.isMinimized),
      );
      await controller.focusWindow(1);
      expect(announcements, isNotEmpty);
      expect(announcements, everyElement(isFalse));
      await controller.moveWindowToWorkspace(1, 2);
      expect(controller.windows.single.workspaceId, 2);
      await controller.removeWorkspace(2);
      expect(controller.windows.single.workspaceId, 1);
    },
  );
}
