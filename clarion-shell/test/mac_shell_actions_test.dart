// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/services/desktop_apps.dart';
import 'package:clarion_shell/services/clarion_ipc.dart';
import 'package:clarion_shell/services/system_service.dart';
import 'package:clarion_shell/services/wallpaper_service.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/ui/mac_shell_view.dart';
import 'package:clarion_shell/ui/shell_panel_content.dart';

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

Map<String, dynamic> _window(int id, {bool focused = true, bool? minimized}) =>
    {
      'id': id,
      'pid': 54321,
      'app_id': 'org.test.Editor',
      'title': 'Document $id',
      'workspace_id': 1,
      'is_focused': focused,
      'is_minimized': ?minimized,
    };

Future<(ShellController, _Ipc, _Apps)> _start({
  bool demo = false,
  _Apps? apps,
}) async {
  final ipc = _Ipc();
  final catalog = apps ?? _Apps();
  final controller = ShellController(
    demo: demo,
    notifications: false,
    tray: false,
    powerProfilesEnabled: false,
    ipc: ipc,
    desktopApps: catalog,
    system: _System(),
    wallpaperService: _Wallpapers(),
  )..setShellStyle(ShellStyle.macos);
  addTearDown(controller.dispose);
  await controller.start();
  if (!demo) ipc.connection.add(true);
  return (controller, ipc, catalog);
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
    'empty desktop launches clients without a global menu dependency',
    () async {
      final apps = _Apps();
      final (controller, _, _) = await _start(apps: apps);
      expect(controller.windows, isEmpty);
      final app = (await apps.load()).single;
      await controller.launchApp(app);
      expect(apps.launched, [app]);
    },
  );

  test('disposed controller cannot launch another client', () async {
    final apps = _Apps();
    final (controller, _, _) = await _start(apps: apps);
    controller.dispose();
    await controller.launchApp((await apps.load()).single);
    expect(apps.launched, isEmpty);
  });

  test(
    'recent-content overlay retains only a live application context',
    () async {
      final (controller, ipc, _) = await _start();
      ipc.windows([_window(91)]);
      expect(controller.contextWindow?.id, 91);
      controller.selectPanel(ShellPanel.recentContent);
      ipc.event.add({
        'WindowFocusChanged': {'id': null},
      });
      expect(controller.focusedWindow, isNull);
      expect(controller.contextWindow?.id, 91);
      var notifications = 0;
      controller.addListener(() => notifications++);
      controller.closePanel();
      expect(controller.panel, ShellPanel.none);
      expect(controller.contextWindow, isNull);
      expect(notifications, greaterThan(0));
      ipc.event.add({
        'WindowFocusChanged': {'id': 91},
      });
      expect(controller.contextWindow?.id, 91);
      controller.selectPanel(ShellPanel.recentContent);
      ipc.event.add({
        'WindowFocusChanged': {'id': null},
      });
      ipc.event.add({
        'WindowClosed': {'id': 91},
      });
      expect(controller.contextWindow, isNull);
    },
  );

  test(
    'hide, show-all and quit continue to target application window IDs',
    () async {
      final (controller, ipc, _) = await _start();
      ipc.windows([
        _window(91, minimized: false),
        _window(92, focused: false, minimized: false),
      ]);
      await controller.hideApp('org.test.Editor.desktop');
      expect(
        ipc.actions
            .where((action) => action.$1 == 'MinimizeWindow')
            .map((action) => action.$2['id']),
        [91, 92],
      );
      ipc.windows([
        _window(91, minimized: true),
        _window(92, focused: false, minimized: true),
      ]);
      await controller.showAll();
      expect(
        ipc.actions
            .where((action) => action.$1 == 'FocusWindow')
            .map((action) => action.$2['id']),
        [91, 92],
      );
      await controller.closeAppWindows('org.test.Editor.desktop');
      expect(
        ipc.actions
            .where((action) => action.$1 == 'CloseWindow')
            .map((action) => action.$2['id']),
        [91, 92],
      );
    },
  );

  testWidgets(
    'Escape closes recent content and releases the native expanded state',
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
      await tester.tap(find.byKey(const ValueKey('mac-recent-content')));
      await tester.pumpAndSettle();
      expect(controller.panel, ShellPanel.recentContent);
      expect(
        find.byKey(const ValueKey('recent-content-panel')),
        findsOneWidget,
      );
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      expect(controller.panel, ShellPanel.none);
      expect(find.byKey(const ValueKey('recent-content-panel')), findsNothing);
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
      ipc.windows([_window(91)]);
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

      ipc.windows([_window(91, minimized: false)]);
      await tester.pump();
      expect(find.text('最小化 · Document 91'), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('dock-minimize-91')));
      await tester.pumpAndSettle();
      expect(ipc.actions.where((a) => a.$1 == 'MinimizeWindow').single.$2, {
        'id': 91,
      });
      ipc.windows([_window(91, minimized: true)]);
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
      final (controller, ipc, _) = await _start(demo: true);
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
    },
  );
}
