// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:realand_shell/models.dart';
import 'package:realand_shell/services/desktop_apps.dart';
import 'package:realand_shell/services/realand_ipc.dart';
import 'package:realand_shell/services/system_service.dart';
import 'package:realand_shell/services/tray_service.dart';
import 'package:realand_shell/services/wallpaper_service.dart';
import 'package:realand_shell/state/shell_controller.dart';

const _a = TrayItem(
  id: 'a',
  title: 'A',
  menuAvailable: true,
  menuPath: '/Menu',
);
const _b = TrayItem(
  id: 'b',
  title: 'B',
  menuAvailable: true,
  menuPath: '/Menu',
);
const _entryA = TrayMenuEntry(id: 1, label: 'A菜单');
const _entryB = TrayMenuEntry(id: 2, label: 'B菜单');

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  setUp(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
          const MethodChannel('realand.shell/window'),
          (call) async =>
              call.method == 'surfaceOrigin' ? {'x': -1280, 'y': 60} : null,
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
    'out-of-order tray menus cannot publish entries or errors into another item',
    () async {
      final tray = _Tray();
      final controller = _controller(tray);
      addTearDown(controller.dispose);
      await controller.start();
      final a = controller.openTrayMenu(_a);
      final b = controller.openTrayMenu(_b);
      expect(controller.trayMenu, isEmpty);
      tray.loads[1].complete(const [_entryB]);
      await b;
      tray.loads[0].completeError(StateError('old exporter failed'));
      await a;
      expect(controller.selectedTrayItem?.id, 'b');
      expect(controller.trayMenu.single.label, 'B菜单');
      expect(controller.trayMenuLoading, false);
      expect(controller.trayError, null);
    },
  );

  test(
    'Escape and selecting another panel invalidate in-flight tray loading',
    () async {
      final tray = _Tray();
      final controller = _controller(tray);
      addTearDown(controller.dispose);
      await controller.start();
      final pending = controller.openTrayMenu(_a);
      controller.closePanel();
      controller.selectPanel(ShellPanel.quickSettings);
      tray.loads.single.complete(const [_entryA]);
      await pending;
      expect(controller.panel, ShellPanel.quickSettings);
      expect(controller.selectedTrayItem, null);
      expect(controller.trayMenu, isEmpty);
      expect(controller.trayMenuLoading, false);
    },
  );

  test(
    'removed items and disposal invalidate pending menu operations',
    () async {
      final tray = _Tray();
      final controller = _controller(tray);
      await controller.start();
      final pending = controller.openTrayMenu(_a);
      tray.catalogue = const [_b];
      tray.changed.add(null);
      tray.loads.single.complete(const [_entryA]);
      await pending;
      expect(controller.panel, ShellPanel.none);
      expect(controller.selectedTrayItem, null);
      final disposed = controller.openTrayMenu(_b);
      controller.dispose();
      tray.loads.last.completeError(StateError('destroyed'));
      await disposed;
      expect(tray.closed, true);
      expect(controller.trayMenu, isEmpty);
      expect(controller.trayError, null);
    },
  );

  test(
    'menu signals refresh only the selected open item and update its title',
    () async {
      final tray = _Tray();
      final controller = _controller(tray);
      addTearDown(controller.dispose);
      await controller.start();
      final opened = controller.openTrayMenu(_a);
      tray.loads.single.complete(const [_entryA]);
      await opened;
      tray.menuChanged.add('b');
      expect(tray.loads.length, 1);
      tray.catalogue = const [
        TrayItem(id: 'a', title: '新标题', menuAvailable: true, menuPath: '/Menu'),
        _b,
      ];
      tray.changed.add(null);
      expect(controller.selectedTrayItem?.title, '新标题');
      tray.menuChanged.add('a');
      expect(controller.trayMenuLoading, true);
      expect(controller.trayMenu, isEmpty);
      tray.loads.last.complete(const [TrayMenuEntry(id: 8, label: '已更新')]);
      await Future<void>.delayed(Duration.zero);
      expect(controller.trayMenu.single.label, '已更新');
      controller.closePanel();
      tray.menuChanged.add('a');
      expect(tray.loads.length, 2);
      tray.available = false;
      tray.changed.add(null);
      expect(controller.trayAvailable, false);
    },
  );

  test(
    'ContextMenu without a DBusMenu path uses logical global coordinates',
    () async {
      final tray = _Tray()..invokeResult = true;
      final controller = _controller(tray);
      addTearDown(controller.dispose);
      await controller.start();
      const item = TrayItem(id: 'native-menu', title: '原生菜单');
      final pending = controller.openTrayMenu(item, x: 124, y: 20);
      tray.loads.single.complete(null);
      await pending;
      expect(tray.invocations.single, (
        'native-menu',
        TrayAction.contextMenu,
        -1156.0,
        80.0,
      ));
      expect(controller.panel, ShellPanel.none);
      await controller.secondaryActivateTrayItem(_a, x: 4, y: 8);
      expect(tray.invocations.last, (
        'a',
        TrayAction.secondaryActivate,
        -1276.0,
        68.0,
      ));
      await controller.scrollTrayItem(_a, -120, orientation: 'horizontal');
      expect(tray.scrolls.single, ('a', -120, 'horizontal'));
    },
  );

  test(
    'late activation fallback and completed menu clicks cannot close a newly selected item',
    () async {
      final tray = _Tray()..invokeWait = Completer<bool>();
      final controller = _controller(tray);
      addTearDown(controller.dispose);
      await controller.start();
      final activation = controller.activateTrayItem(_a);
      await Future<void>.delayed(Duration.zero);
      final b = controller.openTrayMenu(_b);
      tray.loads.single.complete(const [_entryB]);
      await b;
      tray.invokeWait!.complete(false);
      await activation;
      expect(tray.loads.length, 1);
      expect(controller.selectedTrayItem?.id, 'b');
      tray.clickWait = Completer<bool>();
      final click = controller.activateTrayMenuEntry(2);
      final a = controller.openTrayMenu(_a);
      tray.loads.last.complete(const [_entryA]);
      await a;
      tray.clickWait!.complete(true);
      await click;
      expect(controller.panel, ShellPanel.trayMenu);
      expect(controller.selectedTrayItem?.id, 'a');
      expect(controller.trayMenu.single.label, 'A菜单');
    },
  );

  test(
    'disabled menu branches cannot invoke and failed clicks retain the menu for retry',
    () async {
      final tray = _Tray();
      final controller = _controller(tray);
      addTearDown(controller.dispose);
      await controller.start();
      final open = controller.openTrayMenu(_a);
      tray.loads.single.complete(const [
        TrayMenuEntry(id: 1, label: '禁用', enabled: false, children: [_entryB]),
        TrayMenuEntry(id: 3, label: '隐藏', visible: false),
        TrayMenuEntry(id: 4, label: '分隔', separator: true),
        TrayMenuEntry(id: 5, label: '打开'),
      ]);
      await open;
      for (final id in [1, 2, 3, 4, 99]) {
        await controller.activateTrayMenuEntry(id);
      }
      expect(tray.clicks, isEmpty);
      await controller.activateTrayMenuEntry(5);
      expect(tray.clicks, [5]);
      expect(controller.panel, ShellPanel.trayMenu);
      expect(controller.trayError, contains('重试'));
      tray.clickResult = true;
      await controller.activateTrayMenuEntry(5);
      expect(controller.panel, ShellPanel.none);
    },
  );
}

ShellController _controller(_Tray tray) => ShellController(
  notifications: false,
  trayService: tray,
  ipc: _Ipc(),
  system: _System(),
  desktopApps: _Apps(),
  wallpaperService: _Wallpapers(),
);

class _Tray extends TrayService {
  final changed = StreamController<void>.broadcast(sync: true);
  final menuChanged = StreamController<String>.broadcast(sync: true);
  List<TrayItem> catalogue = const [_a, _b];
  final loads = <Completer<List<TrayMenuEntry>?>>[];
  final invocations = <(String, TrayAction, double, double)>[];
  final scrolls = <(String, int, String)>[];
  final clicks = <int>[];
  Completer<bool>? invokeWait;
  Completer<bool>? clickWait;
  bool invokeResult = false;
  bool clickResult = false;
  bool closed = false;
  @override
  List<TrayItem> get items => catalogue;
  @override
  Stream<void> get changes => changed.stream;
  @override
  Stream<String> get menuChanges => menuChanged.stream;
  @override
  Future<bool> start() async {
    available = true;
    return true;
  }

  @override
  Future<List<TrayMenuEntry>?> loadMenu(TrayItem item) {
    final pending = Completer<List<TrayMenuEntry>?>();
    loads.add(pending);
    return pending.future;
  }

  @override
  Future<bool> invoke(
    TrayItem item,
    TrayAction action, {
    double x = 0,
    double y = 0,
  }) async {
    invocations.add((item.id, action, x, y));
    return invokeWait == null ? invokeResult : await invokeWait!.future;
  }

  @override
  Future<bool> activateMenuEntry(TrayItem item, int entryId) async {
    clicks.add(entryId);
    return clickWait == null ? clickResult : await clickWait!.future;
  }

  @override
  Future<bool> scroll(
    TrayItem item,
    int delta, {
    String orientation = 'vertical',
  }) async {
    scrolls.add((item.id, delta, orientation));
    return true;
  }

  @override
  void dispose() {
    closed = true;
    unawaited(changed.close());
    unawaited(menuChanged.close());
    super.dispose();
  }
}

class _Ipc extends RealandIpc {
  @override
  Stream<Map<String, dynamic>> get events => const Stream.empty();
  @override
  Stream<bool> get connections => const Stream.empty();
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
  Future<void> restore({bool applySaved = false}) async {}
}
