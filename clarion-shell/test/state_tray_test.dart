// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/services/desktop_apps.dart';
import 'package:clarion_shell/services/clarion_ipc.dart';
import 'package:clarion_shell/services/system_service.dart';
import 'package:clarion_shell/services/tray_service.dart';
import 'package:clarion_shell/services/wallpaper_service.dart';
import 'package:clarion_shell/state/shell_controller.dart';

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
          const MethodChannel('clarion.shell/window'),
          (call) async =>
              call.method == 'surfaceOrigin' ? {'x': -1280, 'y': 60} : null,
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
      tray.catalogue = const [_a, _b, item];
      final pending = controller.openTrayMenu(item, x: 124, y: 20);
      await pending;
      expect(tray.loads, isEmpty);
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
    'native DBusMenu stays anchored without a Flutter panel or reload storm',
    () async {
      final tray = _Tray()..useNative = true;
      final controller = _controller(tray);
      addTearDown(controller.dispose);
      final calls = <MethodCall>[];
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(
            const MethodChannel('clarion.shell/window'),
            (call) async {
              calls.add(call);
              return call.method == 'showTrayMenu' ? true : null;
            },
          );
      await controller.start();
      await controller.openTrayMenu(_a, x: 82.5, y: 19.25, button: 0);
      expect(controller.panel, ShellPanel.none);
      expect(controller.nativeTrayMenuOpen, true);
      expect(controller.trayMenuLoading, false);
      expect(tray.loads, isEmpty);
      final show = calls.singleWhere((call) => call.method == 'showTrayMenu');
      expect(show.arguments, containsPair('busName', ':1.100'));
      expect(show.arguments, containsPair('menuPath', '/Menu'));
      expect(show.arguments, containsPair('x', 82.5));
      expect(show.arguments, containsPair('y', 19.25));
      expect(show.arguments, containsPair('button', 0));
      for (var i = 0; i < 30; i++) {
        tray.menuChanged.add('a');
      }
      expect(tray.loads, isEmpty);
      expect(
        calls.where((call) => call.method == 'showTrayMenu'),
        hasLength(1),
      );
      final request = (show.arguments as Map)['requestId'] as int;
      controller.nativeTrayMenuClosed(request - 1);
      expect(controller.nativeTrayMenuOpen, true);
      controller.nativeTrayMenuClosed(request);
      expect(controller.nativeTrayMenuOpen, false);
      expect(controller.selectedTrayItem, isNull);
    },
  );

  test(
    'native tray menu releases another panel but preserves tray overflow',
    () async {
      final tray = _Tray()..useNative = true;
      final controller = _controller(tray);
      addTearDown(controller.dispose);
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(
            const MethodChannel('clarion.shell/window'),
            (call) async => call.method == 'showTrayMenu' ? true : null,
          );
      await controller.start();
      controller.selectPanel(ShellPanel.quickSettings);
      await controller.openTrayMenu(_a);
      expect(controller.panel, ShellPanel.none);
      expect(controller.nativeTrayMenuOpen, true);
      controller.closePanel();
      controller.openTrayItems();
      await controller.openTrayMenu(_a);
      expect(controller.panel, ShellPanel.trayItems);
      expect(controller.nativeTrayMenuOpen, true);
    },
  );

  test(
    'repeated native opens coalesce while loading and toggle after display',
    () async {
      final tray = _Tray()..useNative = true;
      final controller = _controller(tray);
      addTearDown(controller.dispose);
      final ready = Completer<bool>();
      final calls = <MethodCall>[];
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(
            const MethodChannel('clarion.shell/window'),
            (call) async {
              calls.add(call);
              return call.method == 'showTrayMenu' ? ready.future : null;
            },
          );
      await controller.start();
      final first = controller.openTrayMenu(_a);
      await Future<void>.delayed(Duration.zero);
      await controller.openTrayMenu(_a);
      expect(
        calls.where((call) => call.method == 'showTrayMenu'),
        hasLength(1),
      );
      ready.complete(true);
      await first;
      expect(controller.nativeTrayMenuOpen, true);
      await controller.openTrayMenu(_a);
      expect(controller.nativeTrayMenuOpen, false);
      expect(
        calls.where((call) => call.method == 'showTrayMenu'),
        hasLength(1),
      );
      await Future<void>.delayed(Duration.zero);
      expect(
        calls.where((call) => call.method == 'dismissTrayMenu'),
        hasLength(1),
      );
    },
  );

  test('native menu owner removal closes only the matching request', () async {
    final tray = _Tray()..useNative = true;
    final controller = _controller(tray);
    addTearDown(controller.dispose);
    final calls = <MethodCall>[];
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(const MethodChannel('clarion.shell/window'), (
          call,
        ) async {
          calls.add(call);
          return call.method == 'showTrayMenu' ? true : null;
        });
    await controller.start();
    await controller.openTrayMenu(_a);
    final first = calls.last.arguments as Map;
    await controller.openTrayMenu(_b);
    controller.nativeTrayMenuClosed(first['requestId'] as int);
    expect(controller.selectedTrayItem, _b);
    tray.catalogue = const [_a];
    tray.changed.add(null);
    await Future<void>.delayed(Duration.zero);
    expect(controller.nativeTrayMenuOpen, false);
    expect(controller.panel, ShellPanel.none);
    expect(
      calls.where((call) => call.method == 'dismissTrayMenu'),
      hasLength(2),
    );
  });

  test(
    'owner replacement closes the old native menu even while the old bus lives',
    () async {
      final tray = _Tray()..useNative = true;
      final controller = _controller(tray);
      addTearDown(controller.dispose);
      final calls = <MethodCall>[];
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(
            const MethodChannel('clarion.shell/window'),
            (call) async {
              calls.add(call);
              return call.method == 'showTrayMenu' ? true : null;
            },
          );
      await controller.start();
      await controller.openTrayMenu(_a);
      final shown = calls.singleWhere((call) => call.method == 'showTrayMenu');
      final request = (shown.arguments as Map)['requestId'];
      tray.tokens['a'] = Object();
      tray.catalogue = const [
        TrayItem(
          id: 'a',
          title: 'New owner',
          menuAvailable: true,
          menuPath: '/Menu',
        ),
        _b,
      ];
      tray.changed.add(null);
      await Future<void>.delayed(Duration.zero);
      expect(controller.nativeTrayMenuOpen, false);
      expect(controller.selectedTrayItem, isNull);
      final dismissed = calls.singleWhere(
        (call) => call.method == 'dismissTrayMenu',
      );
      expect(dismissed.arguments, {'requestId': request});
      await controller.openTrayMenu(_a);
      expect(
        calls.where((call) => call.method == 'showTrayMenu'),
        hasLength(1),
      );
    },
  );

  test(
    'slow activation does not block another tray or lose queued wheel deltas',
    () async {
      final tray = _Tray()..invokeResult = true;
      final controller = _controller(tray);
      addTearDown(controller.dispose);
      await controller.start();
      tray.itemWait['a'] = Completer<bool>();
      tray.scrollWait = Completer<bool>();
      final first = controller.activateTrayItem(_a);
      await Future<void>.delayed(Duration.zero);
      await controller.activateTrayItem(_b);
      expect(tray.invocations.map((call) => call.$1), ['a', 'b']);
      final scrolls = [
        controller.scrollTrayItem(_a, 1),
        controller.scrollTrayItem(_a, 2),
        controller.scrollTrayItem(_a, 3),
      ];
      expect(tray.scrolls, [('a', 1, 'vertical')]);
      tray.scrollWait!.complete(true);
      await Future.wait(scrolls);
      expect(tray.scrolls, [('a', 1, 'vertical'), ('a', 5, 'vertical')]);
      tray.itemWait['a']!.complete(true);
      await first;
    },
  );

  test(
    'a late secondary activation error cannot overwrite another native menu',
    () async {
      final tray = _Tray()
        ..useNative = true
        ..invokeWait = Completer<bool>();
      final controller = _controller(tray);
      addTearDown(controller.dispose);
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(
            const MethodChannel('clarion.shell/window'),
            (call) async => call.method == 'showTrayMenu' ? true : null,
          );
      await controller.start();
      final activation = controller.secondaryActivateTrayItem(_a);
      await Future<void>.delayed(Duration.zero);
      await controller.openTrayMenu(_b);
      tray.invokeWait!.complete(false);
      await activation;
      expect(controller.nativeTrayMenuOpen, true);
      expect(controller.selectedTrayItem, _b);
      expect(controller.trayError, isNull);
    },
  );

  test(
    'metadata refresh preserves wheel deltas queued for the same owner',
    () async {
      final tray = _Tray()..scrollWait = Completer<bool>();
      final controller = _controller(tray);
      addTearDown(controller.dispose);
      await controller.start();
      final first = controller.scrollTrayItem(_a, 1);
      const refreshed = TrayItem(
        id: 'a',
        title: 'Updated title',
        menuAvailable: true,
        menuPath: '/Menu',
      );
      tray.catalogue = const [refreshed, _b];
      tray.changed.add(null);
      final second = controller.scrollTrayItem(refreshed, 2);
      final third = controller.scrollTrayItem(refreshed, 3);
      expect(tray.scrolls, [('a', 1, 'vertical')]);
      tray.scrollWait!.complete(true);
      await Future.wait([first, second, third]);
      expect(tray.scrolls, [('a', 1, 'vertical'), ('a', 5, 'vertical')]);
    },
  );

  test(
    'owner replacement drops old queued wheel input and accepts new input',
    () async {
      final tray = _Tray()..scrollWait = Completer<bool>();
      final controller = _controller(tray);
      addTearDown(controller.dispose);
      await controller.start();
      final first = controller.scrollTrayItem(_a, 1);
      final queued = controller.scrollTrayItem(_a, 2);
      const replacement = TrayItem(
        id: 'a',
        title: 'Restarted application',
        menuAvailable: true,
        menuPath: '/Menu',
      );
      tray.tokens['a'] = Object();
      tray.catalogue = const [replacement, _b];
      tray.changed.add(null);
      final next = controller.scrollTrayItem(replacement, 7);
      await controller.scrollTrayItem(_a, 3);
      expect(tray.scrolls, [('a', 1, 'vertical'), ('a', 7, 'vertical')]);
      tray.scrollWait!.complete(true);
      await Future.wait([first, queued, next]);
      expect(tray.scrolls, [('a', 1, 'vertical'), ('a', 7, 'vertical')]);
    },
  );

  test(
    'cancelled native preparation never opens a late application menu',
    () async {
      final tray = _Tray()..useNative = true;
      final controller = _controller(tray);
      addTearDown(controller.dispose);
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(
            const MethodChannel('clarion.shell/window'),
            (call) async {
              if (call.method == 'showTrayMenu') {
                throw PlatformException(code: 'menu-cancelled');
              }
              return null;
            },
          );
      await controller.start();
      await controller.openTrayMenu(_a);
      expect(controller.nativeTrayMenuOpen, false);
      expect(controller.trayError, isNull);
      expect(tray.loads, isEmpty);
      expect(tray.invocations, isEmpty);
    },
  );

  test(
    'native failure uses ContextMenu once without building a second menu',
    () async {
      final tray = _Tray()
        ..useNative = true
        ..invokeResult = true;
      final controller = _controller(tray);
      addTearDown(controller.dispose);
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(
            const MethodChannel('clarion.shell/window'),
            (call) async {
              if (call.method == 'showTrayMenu') return false;
              if (call.method == 'surfaceOrigin') return {'x': -1280, 'y': 60};
              return null;
            },
          );
      await controller.start();
      await controller.openTrayMenu(_a, x: 124, y: 20);
      expect(controller.panel, ShellPanel.none);
      expect(controller.nativeTrayMenuOpen, false);
      expect(tray.loads, isEmpty);
      expect(tray.invocations, [('a', TrayAction.contextMenu, -1156.0, 80.0)]);
    },
  );

  test(
    'failed keyboard activation retains keyboard provenance for native menu',
    () async {
      final tray = _Tray()..useNative = true;
      final controller = _controller(tray);
      addTearDown(controller.dispose);
      final calls = <MethodCall>[];
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(
            const MethodChannel('clarion.shell/window'),
            (call) async {
              calls.add(call);
              if (call.method == 'showTrayMenu') return true;
              if (call.method == 'surfaceOrigin') return {'x': 0, 'y': 0};
              return null;
            },
          );
      await controller.start();
      await controller.activateTrayItem(_a, menuButton: 0);
      final shown = calls.singleWhere((call) => call.method == 'showTrayMenu');
      expect(shown.arguments, containsPair('button', 0));
      expect(tray.loads, isEmpty);
      expect(tray.invocations.single.$2, TrayAction.activate);
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
  final tokens = <String, Object>{};
  final _publishedTokens = Expando<Object>();
  @override
  Object? registrationToken(TrayItem item) {
    if (catalogue.any((entry) => identical(entry, item))) {
      _publishedTokens[item] ??= tokens.putIfAbsent(item.id, Object.new);
    }
    final token = _publishedTokens[item];
    return catalogue.any((entry) => entry.id == item.id) &&
            identical(token, tokens[item.id])
        ? token
        : null;
  }

  bool useNative = false;
  final itemWait = <String, Completer<bool>>{};
  Completer<bool>? scrollWait;
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
  Map<String, String>? nativeMenuTarget(TrayItem item) =>
      useNative && catalogue.any((entry) => identical(entry, item))
      ? {'busName': ':1.100', 'menuPath': item.menuPath}
      : null;
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
    return await itemWait[item.id]?.future ??
        (invokeWait == null ? invokeResult : await invokeWait!.future);
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
    return await scrollWait?.future ?? true;
  }

  @override
  void dispose() {
    closed = true;
    unawaited(changed.close());
    unawaited(menuChanged.close());
    super.dispose();
  }
}

class _Ipc extends ClarionIpc {
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
