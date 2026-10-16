// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';

import 'package:flutter/material.dart';
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
import 'package:clarion_shell/ui/mac_shell_view.dart';
import 'package:clarion_shell/ui/shell_panel_content.dart';

class _MenuService extends GlobalAppMenuService {
  _MenuService() : super(hostRegistrar: false);
  final targets = <AppMenuTarget>[];
  final activeChanges = <bool>[];
  bool disposed = false;
  int starts = 0;
  Completer<void>? startGate;
  int _revision = 0;
  List<GlobalMenuItem> _items = const [];
  @override
  int get revision => _revision;
  @override
  AppMenuTarget? get target => targets.lastOrNull;
  @override
  List<GlobalMenuItem> get menus => _items;
  @override
  bool get available => _items.isNotEmpty;
  @override
  Future<void> start() async {
    starts++;
    await startGate?.future;
  }

  @override
  Future<void> setActive(bool active) async => activeChanges.add(active);
  @override
  Future<void> focusedTarget({
    int? pid,
    String appId = '',
    int? windowId,
    int? x11WindowId,
    AppMenuExport? export,
  }) async {
    targets.add(
      AppMenuTarget(
        pid: pid,
        appId: appId,
        windowId: windowId,
        x11WindowId: x11WindowId,
        export: export,
      ),
    );
    _revision++;
    _items = export == null
        ? const []
        : [
            GlobalMenuItem(
              id: 'file',
              label: 'File',
              children: [GlobalMenuItem(id: 'save', label: 'Save')],
            ),
          ];
    notifyListeners();
  }

  @override
  Future<void> submenu(GlobalMenuItem item, {required bool opened}) async {}
  @override
  void dispose() {
    disposed = true;
    super.dispose();
  }
}

class _Ipc extends ClarionIpc {
  final connection = StreamController<bool>.broadcast(sync: true);
  final event = StreamController<Map<String, dynamic>>.broadcast(sync: true);
  final actions = <(String, Map<String, Object?>)>[];
  Completer<void>? targetGate;
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
    if (name == 'SetMinimizeTarget') await targetGate?.future;
  }

  void windows(List<Map<String, dynamic>> windows) => event.add({
    'WindowsChanged': {'windows': windows},
  });
  @override
  void dispose() {
    unawaited(connection.close());
    unawaited(event.close());
    super.dispose();
  }
}

class _Apps extends DesktopApps {
  final launched = <DesktopApp>[];
  @override
  Future<void> launch(DesktopApp app) async => launched.add(app);
  @override
  Future<List<DesktopApp>> load() async => const [
    DesktopApp(id: 'org.test.Editor.desktop', name: 'Editor', desktopFile: ''),
  ];
}

class _System extends SystemService {
  @override
  Future<SystemSnapshot> read() async => const SystemSnapshot();
}

class _Wallpapers extends WallpaperService {
  @override
  Future<void> restore({bool applySaved = false}) async {}
}

const _metadata = <String, Object?>{
  'unique_bus_name': ':1.902',
  'menubar_path': '/org/test/Editor/menus/menubar',
  'app_menu_path': '/org/test/Editor/menus/appmenu',
  'application_object_path': '/org/test/Editor',
  'window_object_path': '/org/test/Editor/window/7',
};
Map<String, dynamic> _window(
  int id, {
  bool focused = true,
  bool? minimized,
  Object? metadata = _metadata,
  Object? canonical,
}) => {
  'id': id,
  'pid': 54321,
  'app_id': 'org.test.Editor',
  'title': 'Document $id',
  'workspace_id': 1,
  'is_focused': focused,
  'is_minimized': ?minimized,
  'gtk_app_menu': ?metadata,
  'app_menu': ?canonical,
};

Future<(ShellController, _Ipc, _MenuService)> _start({
  bool demo = false,
  _Apps? apps,
}) async {
  final ipc = _Ipc();
  final menus = _MenuService();
  final controller = ShellController(
    demo: demo,
    notifications: false,
    tray: false,
    powerProfilesEnabled: false,
    ipc: ipc,
    desktopApps: apps ?? _Apps(),
    system: _System(),
    wallpaperService: _Wallpapers(),
    appMenuService: menus,
  )..setShellStyle(ShellStyle.macos);
  addTearDown(controller.dispose);
  await controller.start();
  if (!demo) ipc.connection.add(true);
  return (controller, ipc, menus);
}

Future<void> _drain() => Future<void>.delayed(Duration.zero);

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  setUp(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
          const MethodChannel('clarion.shell/window'),
          (call) async => call.method == 'outputName' ? 'HDMI-A-1' : null,
        );
  });
  tearDown(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
          const MethodChannel('clarion.shell/window'),
          null,
        );
  });

  test(
    'empty desktop prepares the registrar before launching a client',
    () async {
      final apps = _Apps();
      final (controller, _, menus) = await _start(apps: apps);
      expect(controller.windows, isEmpty);
      menus.startGate = Completer<void>();
      final app = (await apps.load()).single;
      final launch = controller.launchApp(app);
      await _drain();
      expect(menus.starts, 1);
      expect(apps.launched, isEmpty);
      menus.startGate!.complete();
      await launch;
      expect(apps.launched, [app]);
    },
  );

  test('disposed controller cannot launch after registrar startup', () async {
    final apps = _Apps();
    final (controller, _, menus) = await _start(apps: apps);
    menus.startGate = Completer<void>();
    final launch = controller.launchApp((await apps.load()).single);
    await _drain();
    controller.dispose();
    menus.startGate!.complete();
    await launch;
    expect(apps.launched, isEmpty);
  });

  test('Material and demo bootstrap do not start a global menu host', () async {
    final (controller, _, menus) = await _start();
    controller.setShellStyle(ShellStyle.material);
    await controller.prepareAppMenus();
    expect(menus.starts, 0);
    final (demo, _, demoMenus) = await _start(demo: true);
    await demo.prepareAppMenus();
    expect(demoMenus.starts, 0);
  });

  test(
    'canonical Wayland metadata selects the surface address and switches protocol at the same path',
    () async {
      final (controller, ipc, menus) = await _start();
      final canonical = {
        'bus_name': ':1.902',
        'menu_path': _metadata['menubar_path'],
      };
      ipc.windows([_window(91, canonical: canonical)]);
      final target = menus.targets.last;
      expect(target.pid, 54321);
      expect(target.windowId, 91);
      expect(target.x11WindowId, isNull);
      expect(target.export!.busName, ':1.902');
      expect(target.export!.protocol, AppMenuProtocol.dbusmenu);
      expect(target.export!.applicationActionsPath, isNull);
      expect(target.export!.windowActionsPath, isNull);

      // Identical address strings cannot retain the previous GTK adapter.
      final previous = menus.targets.length;
      ipc.windows([_window(91)]);
      expect(menus.targets.length, previous + 1);
      expect(menus.targets.last.export!.protocol, AppMenuProtocol.gtk);
      ipc.windows([_window(91, metadata: null, canonical: canonical)]);
      expect(
        controller.focusedWindow!.appMenu!.protocol,
        AppMenuProtocol.dbusmenu,
      );
      ipc.windows([
        _window(
          91,
          metadata: null,
          canonical: {'bus_name': false, 'menu_path': 42},
        ),
      ]);
      expect(controller.focusedWindow!.appMenu, isNull);
      expect(menus.available, isFalse);
    },
  );

  test(
    'Material retains explicit canonical menus without a registrar host',
    () async {
      final (controller, ipc, menus) = await _start();
      final canonical = {'bus_name': ':1.902', 'menu_path': '/MenuBar/7'};
      ipc.windows([_window(91, metadata: null, canonical: canonical)]);
      controller.setShellStyle(ShellStyle.material);
      await _drain();
      expect(menus.activeChanges.last, isFalse);
      expect(controller.materialAppMenuAvailable, isTrue);
      expect(menus.targets.last.export!.menuPath, '/MenuBar/7');
      expect(menus.targets.last.pid, 54321);
      controller.setAppMenuOpen(true);
      expect(controller.appMenuOpen, isTrue);
      ipc.windows([
        _window(91, focused: false, metadata: null, canonical: canonical),
      ]);
      expect(controller.materialAppMenuAvailable, isTrue);
      controller.setAppMenuOpen(false);
      expect(controller.materialAppMenuAvailable, isFalse);
      expect(menus.targets.last.pid, isNull);
    },
  );

  test(
    'GTK IPC metadata selects exact application/window paths without inferring an XID',
    () async {
      final (controller, ipc, menus) = await _start();
      ipc.windows([_window(91)]);
      final target = menus.targets.last;
      expect(target.pid, 54321);
      expect(target.appId, 'org.test.Editor');
      expect(target.windowId, 91);
      expect(target.x11WindowId, isNull);
      expect(target.export!.busName, ':1.902');
      expect(target.export!.menuPath, _metadata['menubar_path']);
      expect(
        target.export!.applicationActionsPath,
        _metadata['application_object_path'],
      );
      // GtkApplicationWindow ID 7 is unrelated to compositor window ID 91.
      expect(target.export!.windowActionsPath, '/org/test/Editor/window/7');
      expect(controller.focusedWindow!.appMenu!.protocol, AppMenuProtocol.gtk);

      ipc.windows([
        _window(
          91,
          metadata: {
            ..._metadata,
            'menubar_path': '',
            'application_object_path': 42,
            'window_object_path': false,
          },
        ),
      ]);
      expect(menus.targets.last.export!.menuPath, _metadata['app_menu_path']);
      expect(menus.targets.last.export!.applicationActionsPath, isNull);
      expect(menus.targets.last.export!.windowActionsPath, isNull);
      ipc.windows([
        _window(91, metadata: {'unique_bus_name': ':1.902'}),
      ]);
      expect(controller.focusedWindow!.appMenu, isNull);
      expect(menus.available, isFalse);
    },
  );

  test(
    'native overlay focus is retained only while a menu is open, and closing notifies',
    () async {
      final (controller, ipc, menus) = await _start();
      ipc.windows([_window(91)]);
      controller.setAppMenuOpen(true);
      final count = menus.targets.length;
      ipc.event.add({
        'WindowFocusChanged': {'id': null},
      });
      expect(controller.focusedWindow, isNull);
      expect(controller.menuFocusedWindow!.id, 91);
      expect(menus.targets, hasLength(count));

      var notifications = 0;
      controller.addListener(() => notifications++);
      final epoch = controller.appMenuDismissRevision;
      controller.closePanel();
      expect(controller.appMenuOpen, isFalse);
      expect(controller.appMenuDismissRevision, greaterThan(epoch));
      expect(controller.menuFocusedWindow, isNull);
      expect(menus.targets.last.pid, isNull);
      expect(notifications, greaterThan(0));

      ipc.event.add({
        'WindowFocusChanged': {'id': 91},
      });
      controller.setAppMenuOpen(true);
      ipc.event.add({
        'WindowFocusChanged': {'id': null},
      });
      ipc.event.add({
        'WindowClosed': {'id': 91},
      });
      expect(controller.menuFocusedWindow, isNull);
      expect(menus.targets.last.windowId, isNull);
      expect(menus.available, isFalse);
    },
  );

  test(
    'Material clears exported menus, blocks opening, and disposal owns the injected service',
    () async {
      final (controller, ipc, menus) = await _start();
      ipc.windows([_window(91)]);
      controller.setAppMenuOpen(true);
      controller.setShellStyle(ShellStyle.material);
      expect(controller.appMenuOpen, isFalse);
      expect(menus.targets.last.pid, isNull);
      expect(menus.available, isFalse);
      controller.setAppMenuOpen(true);
      expect(controller.appMenuOpen, isFalse);
      controller.setShellStyle(ShellStyle.macos);
      expect(menus.targets.last.windowId, 91);
      controller.dispose();
      expect(menus.disposed, isTrue);
    },
  );

  testWidgets(
    'Escape closes an exported popup and releases the native expanded state',
    (tester) async {
      late ShellController controller;
      late _Ipc ipc;
      await tester.runAsync(() async {
        (controller, ipc, _) = await _start();
      });
      ipc.windows([_window(91)]);
      await tester.pumpWidget(
        MaterialApp(home: MacShellView(controller: controller)),
      );
      await tester.pump(const Duration(milliseconds: 200));
      await tester.tap(find.text('File'));
      await tester.pumpAndSettle();
      expect(controller.appMenuOpen, isTrue);
      expect(find.text('Save'), findsOneWidget);
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      expect(controller.appMenuOpen, isFalse);
      expect(find.text('Save'), findsNothing);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets(
    'old compositor snapshots omit minimize actions; the optional capability enables minimize/restore',
    (tester) async {
      late ShellController controller;
      late _Ipc ipc;
      await tester.runAsync(() async {
        (controller, ipc, _) = await _start();
      });
      ipc.windows([_window(91, metadata: null)]);
      controller.openDockAppMenu('org.test.Editor.desktop');
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: AnimatedBuilder(
              animation: controller,
              builder: (_, _) => ShellPanelContent(controller: controller),
            ),
          ),
        ),
      );
      await tester.pump(const Duration(milliseconds: 200));
      expect(controller.canMinimize, isFalse);
      expect(find.byKey(const ValueKey('dock-minimize-91')), findsNothing);
      await controller.minimizeWindow(91);
      expect(ipc.actions.where((a) => a.$1 == 'MinimizeWindow'), isEmpty);

      ipc.windows([_window(91, minimized: false, metadata: null)]);
      await tester.pump();
      expect(find.text('最小化 · Document 91'), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('dock-minimize-91')));
      await tester.pumpAndSettle();
      expect(ipc.actions.where((a) => a.$1 == 'MinimizeWindow').single.$2, {
        'id': 91,
      });
      ipc.windows([_window(91, minimized: true, metadata: null)]);
      controller.openDockAppMenu('org.test.Editor.desktop');
      await tester.pump();
      expect(find.text('还原 · Document 91'), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('dock-minimize-91')));
      await tester.pumpAndSettle();
      expect(ipc.actions.where((a) => a.$1 == 'FocusWindow').single.$2, {
        'id': 91,
      });
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  test(
    'Dock targets preserve output-local rectangles, refresh motion, and clear on Material',
    () async {
      final (controller, ipc, _) = await _start();
      ipc.windows([_window(91, minimized: false)]);
      const target = Rect.fromLTWH(17, 19, 40, 42);
      await controller.setDockMinimizeTargets({91: target});
      expect(ipc.actions.single.$1, 'SetMinimizeTarget');
      expect(ipc.actions.single.$2, {
        'id': 91,
        'target': {
          'output': 'HDMI-A-1',
          'x': 17.0,
          'y': 19.0,
          'width': 40.0,
          'height': 42.0,
          'edge': 'bottom',
          'effect': 'genie',
        },
        'reduce_motion': false,
      });
      await controller.setDockMinimizeTargets({91: target});
      expect(ipc.actions, hasLength(1));
      controller.setReduceMotion(true);
      await _drain();
      expect(ipc.actions.last.$2['target'], ipc.actions.first.$2['target']);
      expect(ipc.actions.last.$2['reduce_motion'], isTrue);
      controller.setShellStyle(ShellStyle.material);
      await _drain();
      expect(ipc.actions.last.$2, {
        'id': 91,
        'target': null,
        'reduce_motion': true,
      });
      await controller.setDockMinimizeTargets({91: target});
      expect(ipc.actions, hasLength(3));
    },
  );

  test(
    'queued Dock geometry skips destroyed windows and applies the latest rectangle',
    () async {
      final (controller, ipc, _) = await _start();
      ipc.windows([
        _window(91, minimized: false),
        _window(92, focused: false, minimized: false),
      ]);
      final gate = ipc.targetGate = Completer<void>();
      const old = Rect.fromLTWH(10, 20, 32, 32);
      const latest = Rect.fromLTWH(30, 20, 32, 32);
      final pending = controller.setDockMinimizeTargets({91: old, 92: old});
      expect(ipc.actions, hasLength(1));
      ipc.event.add({
        'WindowClosed': {'id': 92},
      });
      await controller.setDockMinimizeTargets({91: latest, 92: latest});
      ipc.targetGate = null;
      gate.complete();
      await pending;
      expect(ipc.actions.map((a) => a.$2['id']), [91, 91]);
      expect((ipc.actions.last.$2['target'] as Map)['x'], 30.0);
    },
  );

  test(
    'demo minimize and restore update shared windows without bus or IPC actions',
    () async {
      final (controller, ipc, menus) = await _start(demo: true);
      final id = controller.focusedWindow!.id;
      expect(controller.canMinimize, isTrue);
      await controller.minimizeWindow(id);
      expect(controller.windows.single.isMinimized, isTrue);
      expect(controller.focusedWindow, isNull);
      await controller.focusWindow(id);
      expect(controller.focusedWindow!.id, id);
      expect(controller.focusedWindow!.isMinimized, isFalse);
      await controller.setDockMinimizeTargets({
        id: const Rect.fromLTWH(1, 2, 3, 4),
      });
      expect(ipc.actions, isEmpty);
      expect(menus.targets, isEmpty);
    },
  );
}
