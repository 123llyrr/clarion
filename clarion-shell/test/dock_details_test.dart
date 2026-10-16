// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/mac_shell_preferences.dart';
import 'package:clarion_shell/app_menu_models.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/preferences.dart';
import 'package:clarion_shell/main.dart' show DockApplication;
import 'package:clarion_shell/services/desktop_apps.dart';
import 'package:clarion_shell/services/dock_folder_service.dart';
import 'package:clarion_shell/services/global_app_menu_service.dart';
import 'package:clarion_shell/services/clarion_ipc.dart';
import 'package:clarion_shell/services/system_service.dart';
import 'package:clarion_shell/services/trash_state_service.dart';
import 'package:clarion_shell/services/wallpaper_service.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/theme/mac_shell_theme.dart';
import 'package:clarion_shell/ui/mac_dock_view.dart';
import 'package:clarion_shell/ui/mac_dock_panel_geometry.dart';
import 'package:clarion_shell/ui/mac_shell_view.dart';

class _Ipc extends ClarionIpc {
  final changes = StreamController<Map<String, dynamic>>.broadcast(sync: true);
  final connected = StreamController<bool>.broadcast(sync: true);
  final calls = <(String, Map<String, Object?>)>[];
  @override
  Stream<Map<String, dynamic>> get events => changes.stream;
  @override
  Stream<bool> get connections => connected.stream;
  @override
  void start() {}
  @override
  Future<void> action(
    String name, [
    Map<String, Object?> args = const {},
  ]) async {
    calls.add((name, args));
  }

  @override
  void dispose() {
    unawaited(changes.close());
    unawaited(connected.close());
    super.dispose();
  }
}

const _apps = [
  DesktopApp(
    id: 'editor.desktop',
    name: 'Editor',
    desktopFile: '/fixture/editor.desktop',
  ),
  DesktopApp(
    id: 'browser.desktop',
    name: 'Browser',
    desktopFile: '/fixture/browser.desktop',
  ),
  DesktopApp(
    id: 'notes.desktop',
    name: 'Notes',
    desktopFile: '/fixture/notes.desktop',
  ),
  DesktopApp(
    id: 'reader.desktop',
    name: 'Reader',
    desktopFile: '/fixture/reader.desktop',
  ),
];

class _Apps extends DesktopApps {
  @override
  Future<List<DesktopApp>> load() async => _apps;
  @override
  Future<void> launch(DesktopApp app) async {}
}

class _DeferredApps extends _Apps {
  final ready = Completer<List<DesktopApp>>.sync();
  final started = Completer<void>.sync();
  Future<void>? initialization;
  @override
  Future<List<DesktopApp>> load() {
    started.complete();
    return ready.future;
  }
}

class _Folder extends DockFolderService {
  @override
  Future<void> start() async {}
}

class _System extends SystemService {
  final revealed = <String>[];
  @override
  Future<SystemSnapshot> read() async => const SystemSnapshot();
  @override
  Future<void> openLocation(String path) async => revealed.add(path);
}

class _Wallpapers extends WallpaperService {
  @override
  Future<void> restore({bool applySaved = false}) async {}
  @override
  Future<List<ShellWallpaper>> scan() async => [];
}

class _Menus extends GlobalAppMenuService {
  _Menus() : super(hostRegistrar: false);
  @override
  Future<void> start() async {}
  @override
  Future<void> setActive(bool value) async {}
  @override
  Future<void> focusedTarget({
    int? pid,
    String appId = '',
    int? windowId,
    int? x11WindowId,
    AppMenuExport? export,
  }) async {}
}

Future<(ShellController, _Ipc, _System)> _fixture(
  WidgetTester tester, {
  _Apps? applications,
  bool waitForCatalogue = true,
}) async {
  final ipc = _Ipc();
  final system = _System();
  final c =
      ShellController(
          ipc: ipc,
          system: system,
          desktopApps: applications ?? _Apps(),
          dockFolderService: _Folder(),
          wallpaperService: _Wallpapers(),
          appMenuService: _Menus(),
          tray: false,
          notifications: false,
          powerProfilesEnabled: false,
          restoreWallpaperOnStart: false,
        )
        ..setShellStyle(ShellStyle.macos)
        ..setPinnedAppIds(['browser.desktop'])
        ..setReduceMotion(true);
  addTearDown(c.dispose);
  if (waitForCatalogue) {
    await tester.runAsync(c.start);
  } else {
    final deferred = applications! as _DeferredApps;
    await tester.runAsync(() async {
      deferred.initialization = c.start();
      await deferred.started.future;
    });
  }
  ipc.connected.add(true);
  ipc.changes.add({
    'WindowsChanged': {
      'windows': [
        {
          'id': 11,
          'app_id': 'editor',
          'title': 'Document',
          'is_focused': true,
          'is_minimized': false,
        },
        {
          'id': 12,
          'app_id': 'editor',
          'title': 'Other Document',
          'is_focused': false,
          'is_minimized': true,
        },
      ],
    },
  });
  await tester.pump(const Duration(milliseconds: 200));
  return (c, ipc, system);
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  setUp(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
          const MethodChannel('clarion.shell/window'),
          (call) async => call.method == 'outputName' ? 'eDP-1' : null,
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
    'recent history persists, bounds and deduplicates malformed preferences',
    () {
      final p = ShellPreferences.fromJson({
        'recent_app_ids': [
          ' editor.desktop ',
          'editor.desktop',
          null,
          4,
          for (var i = 0; i < 40; i++) '$i.desktop',
        ],
      });
      expect(p.recentAppIds.length, 24);
      expect(p.recentAppIds.first, 'editor.desktop');
      expect(
        ShellPreferences.fromJson(p.toJson()).recentAppIds,
        p.recentAppIds,
      );
      expect(ShellPreferences.fromJson({}).recentAppIds, isEmpty);
    },
  );

  testWidgets(
    'real focus orders recent apps, excluding running and pinned apps',
    (tester) async {
      final (c, ipc, _) = await _fixture(tester);
      c.setRecentAppIds([
        'editor.desktop',
        'browser.desktop',
        'notes.desktop',
        'reader.desktop',
      ]);
      expect(c.recentDockApps.map((a) => a.id), [
        'notes.desktop',
        'reader.desktop',
      ]);
      ipc.changes.add({
        'WindowOpenedOrChanged': {
          'window': {
            'id': 13,
            'app_id': 'reader',
            'title': 'Book',
            'is_focused': true,
            'is_minimized': false,
          },
        },
      });
      expect(c.recentAppIds.first, 'reader.desktop');
      expect(c.recentDockApps.map((a) => a.id), ['notes.desktop']);
      ipc.changes.add({
        'WindowClosed': {'id': 13},
      });
      expect(c.recentDockApps.map((a) => a.id), [
        'reader.desktop',
        'notes.desktop',
      ]);
      c.setMacPreferences(
        c.macPreferences.copyWith(showRecentApplications: false),
      );
      expect(c.recentDockApps, isEmpty);
      expect(c.recentAppIds.first, 'reader.desktop');
    },
  );

  testWidgets(
    'leaving a Dock panel discards its anchor before launcher menus',
    (tester) async {
      final (c, _, _) = await _fixture(tester);
      c.setDockMenuAnchor(const Rect.fromLTWH(40, 700, 48, 48));
      c.openDockAppMenu('editor.desktop');
      expect(c.dockMenuAnchor, isNotNull);
      c.selectPanel(ShellPanel.launcher);
      expect(c.dockMenuAnchor, isNull);
      c.openDockAppMenu('reader.desktop');
      expect(c.dockMenuAnchor, isNull);
      c.closePanel();
      await tester.pump(const Duration(milliseconds: 200));
    },
  );

  testWidgets('unrelated desktop events do not reorder recent launches', (
    tester,
  ) async {
    final (c, ipc, _) = await _fixture(tester);
    await c.launchApp(_apps.last);
    expect(c.recentAppIds.first, 'reader.desktop');
    ipc.changes.add({
      'WindowOpenedOrChanged': {
        'window': {
          'id': 11,
          'app_id': 'editor',
          'title': 'Updated title',
          'is_focused': true,
          'is_minimized': false,
        },
      },
    });
    expect(c.recentAppIds.first, 'reader.desktop');
    ipc.changes.add({
      'WindowFocusChanged': {'id': 12},
    });
    expect(c.recentAppIds.first, 'editor.desktop');
    await tester.pump(const Duration(milliseconds: 200));
  });

  testWidgets('startup focus is reconciled after delayed catalogue loading', (
    tester,
  ) async {
    final apps = _DeferredApps();
    final (c, _, _) = await _fixture(
      tester,
      applications: apps,
      waitForCatalogue: false,
    );
    expect(c.recentAppIds, isEmpty);
    await tester.runAsync(() async {
      apps.ready.complete(_apps);
      await apps.initialization;
    });
    await tester.pump();
    expect(c.recentAppIds.first, 'editor.desktop');
  });

  testWidgets(
    'late surfaceOrigin cannot reopen a menu after output geometry changes',
    (tester) async {
      final (c, _, _) = await _fixture(tester);
      final origins = <Completer<Map<String, Object?>>>[];
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(
            const MethodChannel('clarion.shell/window'),
            (call) async {
              if (call.method == 'surfaceOrigin') {
                final result = Completer<Map<String, Object?>>();
                origins.add(result);
                return result.future;
              }
              return call.method == 'outputName' ? 'eDP-1' : null;
            },
          );
      await tester.pumpWidget(
        DockApplication(
          controller: c,
          preferences: const ShellPreferences(),
          availableWidth: 800,
          preview: true,
        ),
      );
      await tester.pump();
      final view = tester.widget<MacDockView>(find.byType(MacDockView));
      var opened = 0;
      view.onOpenMenu!(const Rect.fromLTWH(20, 10, 48, 48), () => opened++);
      await tester.pump();
      expect(origins, hasLength(1));
      c.outputGeometryChanged();
      origins[0].complete({'outputX': 200.0, 'outputY': 700.0});
      await tester.pump();
      expect(opened, 0);
      expect(c.dockMenuAnchor, isNull);
      view.onOpenMenu!(const Rect.fromLTWH(20, 10, 48, 48), () => opened++);
      await tester.pump();
      origins[1].complete({'outputX': 100.0, 'outputY': 600.0});
      await tester.pump();
      expect(opened, 1);
      expect(c.dockMenuAnchor, const Rect.fromLTWH(120, 610, 48, 48));
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  test(
    'menus clamp around actual icon on all three Dock edges and small outputs',
    () {
      for (final output in [const Size(1200, 900), const Size(260, 240)]) {
        for (final edge in MacDockPosition.values) {
          final r = macDockPanelGeometry(
            output: output,
            position: edge,
            width: 280,
            height: 300,
            menuBarHeight: 30,
            dockThickness: 160,
            anchor: Rect.fromLTWH(
              output.width - 70,
              output.height - 60,
              48,
              48,
            ),
          );
          expect(r.left, greaterThanOrEqualTo(0));
          expect(r.top, greaterThanOrEqualTo(38));
          expect(r.right, lessThanOrEqualTo(output.width));
          expect(r.bottom, lessThanOrEqualTo(output.height));
        }
      }
      final r = macDockPanelGeometry(
        output: const Size(1200, 900),
        position: MacDockPosition.bottom,
        width: 280,
        height: 240,
        menuBarHeight: 30,
        dockThickness: 178,
        anchor: const Rect.fromLTWH(300, 820, 64, 64),
      );
      expect(r.center.dx, 332);
      expect(r.bottom, 812);
    },
  );

  testWidgets(
    'application and recent sections precede the folder/trash section',
    (tester) async {
      final (c, _, _) = await _fixture(tester);
      c.setRecentAppIds(['notes.desktop', 'reader.desktop']);
      await tester.pumpWidget(
        MaterialApp(
          theme: buildMacShellTheme(Brightness.light),
          home: Align(
            alignment: Alignment.bottomCenter,
            child: SizedBox(height: 160, child: MacDockView(controller: c)),
          ),
        ),
      );
      await tester.pump();
      final utility = tester
          .getCenter(
            find.byKey(const ValueKey('dock-divider-utilities-divider')),
          )
          .dx;
      expect(
        tester.getCenter(find.byKey(const ValueKey('dock-overview'))).dx,
        lessThan(utility),
      );
      expect(
        tester
            .getCenter(find.byKey(const ValueKey('dock-app-notes.desktop')))
            .dx,
        lessThan(utility),
      );
      expect(
        find.byKey(const ValueKey('dock-divider-running-divider')),
        findsNothing,
      );
      expect(
        find.byKey(const ValueKey('dock-divider-recent-divider')),
        findsOneWidget,
      );
    },
  );

  testWidgets(
    'compact menu uses anchor, selects real windows and closes politely',
    (tester) async {
      final (c, ipc, system) = await _fixture(tester);
      c.setDockMenuAnchor(const Rect.fromLTWH(500, 500, 48, 48));
      c.openDockAppMenu('editor.desktop');
      await tester.pumpWidget(
        MaterialApp(
          theme: buildMacShellTheme(Brightness.light),
          home: MacShellView(controller: c),
        ),
      );
      await tester.pump();
      expect(
        tester.getSize(find.byKey(const ValueKey('dock-menu-open'))).height,
        26,
      );
      expect(
        tester.getSize(find.byKey(const ValueKey('mac-panel-position'))).width,
        280,
      );
      await c.revealDockApp('editor.desktop');
      expect(system.revealed, ['/fixture']);
      c.openDockAppMenu('editor.desktop');
      await tester.pump();
      expect(find.byKey(const ValueKey('dock-menu-hide')), findsOneWidget);
      await c.closeAppWindows('editor.desktop');
      expect(
        ipc.calls.where((v) => v.$1 == 'CloseWindow').map((v) => v.$2['id']),
        [11, 12],
      );
      expect(ipc.calls.any((v) => v.$1 == 'Spawn'), isFalse);
      await tester.pump(const Duration(milliseconds: 200));
    },
  );

  testWidgets('recent icon hover survives unrelated controller updates', (
    tester,
  ) async {
    final (c, _, _) = await _fixture(tester);
    c.setRecentAppIds(['notes.desktop']);
    await tester.pumpWidget(
      MaterialApp(
        home: Align(
          alignment: Alignment.bottomCenter,
          child: SizedBox(height: 160, child: MacDockView(controller: c)),
        ),
      ),
    );
    await tester.pump();
    final pointer = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await pointer.addPointer(location: const Offset(0, 0));
    addTearDown(pointer.removePointer);
    await pointer.moveTo(
      tester.getCenter(find.byKey(const ValueKey('dock-app-notes.desktop'))),
    );
    await tester.pump();
    expect(find.text('Notes'), findsOneWidget);
    c.setThemeMode(ThemeMode.dark);
    await tester.pump();
    expect(find.text('Notes'), findsOneWidget);
    final transform = tester.widget<Transform>(
      find.byKey(const ValueKey('dock-scale-Notes')),
    );
    expect(transform.transform.storage[0], greaterThan(1));
  });

  testWidgets('auto-hide Dock remains presented while its menu is open', (
    tester,
  ) async {
    final (c, _, _) = await _fixture(tester);
    c.setMacPreferences(c.macPreferences.copyWith(dockAutoHide: true));
    final frames = <Rect>[];
    await tester.pumpWidget(
      MaterialApp(
        home: Align(
          alignment: Alignment.bottomCenter,
          child: SizedBox(
            height: 160,
            child: MacDockView(
              controller: c,
              onPresentationChanged: (glass, _, _) => frames.add(glass),
            ),
          ),
        ),
      ),
    );
    await tester.pump();
    expect(frames.last, Rect.zero);
    c.openDockAppMenu('editor.desktop');
    await tester.pump();
    await tester.pump();
    expect(frames.last.isEmpty, isFalse);
    c.closePanel();
    await tester.pump(const Duration(milliseconds: 200));
    await tester.pump();
    expect(frames.last, Rect.zero);
  });

  test(
    'trash state tracks complete GIO result and preserves state on failures',
    () async {
      var value = '';
      var fail = false;
      final trash = TrashStateService(
        environment: const {},
        command: (program, args) async {
          expect(program, 'gio');
          expect(args, ['list', 'trash:///']);
          if (fail) throw StateError('unavailable');
          return value;
        },
      );
      addTearDown(trash.dispose);
      await trash.refresh();
      expect(trash.hasItems, isFalse);
      value = 'file.txt\nfolder';
      await trash.refresh();
      expect(trash.hasItems, isTrue);
      fail = true;
      await trash.refresh();
      expect(trash.hasItems, isTrue);
      fail = false;
      value = '';
      await trash.refresh();
      expect(trash.hasItems, isFalse);
    },
  );

  test('empty trash calls gio trash --empty', () async {
    final calls = <List<String>>[];
    final trash = TrashStateService(
      environment: const {},
      command: (program, args) async {
        calls.add([program, ...args]);
        return '';
      },
    );
    addTearDown(trash.dispose);
    await trash.emptyTrash();
    expect(calls, [
      ['gio', 'trash', '--empty'],
      ['gio', 'list', 'trash:///'],
    ]);
  });

  testWidgets('dragging Dock divider resizes dockSize dynamically', (
    tester,
  ) async {
    final (c, _, _) = await _fixture(tester);
    c.setMacPreferences(c.macPreferences.copyWith(dockSize: 48.0));
    await tester.pumpWidget(
      MaterialApp(
        home: Align(
          alignment: Alignment.bottomCenter,
          child: SizedBox(height: 160, child: MacDockView(controller: c)),
        ),
      ),
    );
    await tester.pump();
    final dividerFinder = find.byKey(
      const ValueKey('dock-divider-line-utilities-divider'),
    );
    expect(dividerFinder, findsOneWidget);

    // Drag up by 10 pixels (inward)
    await tester.drag(
      dividerFinder,
      const Offset(0, -10),
      kind: PointerDeviceKind.mouse,
    );
    await tester.pump();
    expect(c.macPreferences.dockSize, equals(58.0));

    // Drag down by 80 pixels (clamped to 32.0)
    await tester.drag(
      dividerFinder,
      const Offset(0, 80),
      kind: PointerDeviceKind.mouse,
    );
    await tester.pump();
    expect(c.macPreferences.dockSize, equals(32.0));

    // Drag up by 80 pixels (clamped to 64.0)
    await tester.drag(
      dividerFinder,
      const Offset(0, -80),
      kind: PointerDeviceKind.mouse,
    );
    await tester.pump();
    expect(c.macPreferences.dockSize, equals(64.0));
  });

  testWidgets('right clicking Dock divider opens dock context menu', (
    tester,
  ) async {
    final (c, _, _) = await _fixture(tester);
    await tester.pumpWidget(
      MaterialApp(
        home: Align(
          alignment: Alignment.bottomCenter,
          child: SizedBox(height: 160, child: MacDockView(controller: c)),
        ),
      ),
    );
    await tester.pump();
    final dividerFinder = find.byKey(
      const ValueKey('dock-divider-line-utilities-divider'),
    );
    await tester.tap(
      dividerFinder,
      kind: PointerDeviceKind.mouse,
      buttons: kSecondaryMouseButton,
    );
    await tester.pump();
    expect(c.panel, ShellPanel.dockContextMenu);
    expect(c.dockMenuAnchor, isNotNull);

    // Verify context menu content in MacShellView
    await tester.pumpWidget(MaterialApp(home: MacShellView(controller: c)));
    await tester.pump();
    expect(
      find.byKey(const ValueKey('dock-menu-toggle-autohide')),
      findsOneWidget,
    );
    expect(
      find.byKey(const ValueKey('dock-menu-position-left')),
      findsOneWidget,
    );
    expect(find.byKey(const ValueKey('dock-menu-preferences')), findsOneWidget);

    // Toggle autohide
    await tester.tap(find.byKey(const ValueKey('dock-menu-toggle-autohide')));
    await tester.pump();
    expect(c.macPreferences.dockAutoHide, isTrue);
    expect(c.panel, ShellPanel.none);
  });

  testWidgets('right clicking Trash opens dock trash menu and handles empty', (
    tester,
  ) async {
    final (c, _, _) = await _fixture(tester);
    await tester.pumpWidget(
      MaterialApp(
        home: Align(
          alignment: Alignment.bottomCenter,
          child: SizedBox(height: 160, child: MacDockView(controller: c)),
        ),
      ),
    );
    await tester.pump();
    final trashFinder = find.byKey(const ValueKey('dock-trash'));
    await tester.tap(
      trashFinder,
      kind: PointerDeviceKind.mouse,
      buttons: kSecondaryMouseButton,
    );
    await tester.pump();
    expect(c.panel, ShellPanel.dockTrashMenu);
    expect(c.dockMenuAnchor, isNotNull);

    // Verify trash menu content in MacShellView
    await tester.pumpWidget(MaterialApp(home: MacShellView(controller: c)));
    await tester.pump();
    expect(find.byKey(const ValueKey('dock-trash-open')), findsOneWidget);
    expect(find.byKey(const ValueKey('dock-trash-empty')), findsOneWidget);
  });

  testWidgets('dock app menu displays localized pin and unpin labels', (
    tester,
  ) async {
    final (c, _, _) = await _fixture(tester);
    await tester.pumpWidget(MaterialApp(home: MacShellView(controller: c)));
    // browser.desktop is pinned in fixture
    c.openDockAppMenu('browser.desktop');
    await tester.pump();
    expect(find.text('从程序坞中移除'), findsOneWidget);
    c.closePanel();
    await tester.pump();

    // editor.desktop is unpinned
    c.openDockAppMenu('editor.desktop');
    await tester.pump();
    expect(find.text('在程序坞中保留'), findsOneWidget);
    expect(find.text('选项'), findsOneWidget);
    expect(find.text('在访达中显示'), findsOneWidget);
    expect(find.text('退出'), findsOneWidget);
  });

  testWidgets('right clicking Dock glass shelf opens dock context menu', (
    tester,
  ) async {
    final (c, _, _) = await _fixture(tester);
    await tester.pumpWidget(
      MaterialApp(
        home: Align(
          alignment: Alignment.bottomCenter,
          child: SizedBox(height: 160, child: MacDockView(controller: c)),
        ),
      ),
    );
    await tester.pump();
    final glassFinder = find.byKey(const ValueKey('mac-dock-glass'));
    expect(glassFinder, findsOneWidget);
    final blankEdge = tester.getBottomLeft(glassFinder) + const Offset(4, -4);
    await tester.tapAt(blankEdge, buttons: kSecondaryMouseButton);
    await tester.pump();
    expect(c.panel, ShellPanel.dockContextMenu);
    expect(c.dockMenuAnchor, isNotNull);

    // Verify context menu content in MacShellView
    await tester.pumpWidget(MaterialApp(home: MacShellView(controller: c)));
    await tester.pump();
    expect(
      find.byKey(const ValueKey('dock-menu-toggle-autohide')),
      findsOneWidget,
    );
    expect(find.byKey(const ValueKey('dock-menu-preferences')), findsOneWidget);
  });

  testWidgets(
    'Finder is labeled 访达, running dot is visible, and secondary tap opens Finder menu',
    (tester) async {
      final (c, _, _) = await _fixture(tester);
      await tester.pumpWidget(
        MaterialApp(
          home: Align(
            alignment: Alignment.bottomCenter,
            child: SizedBox(height: 160, child: MacDockView(controller: c)),
          ),
        ),
      );
      await tester.pump();

      // Finder item exists with dock-finder key
      final finder = find.byKey(const ValueKey('dock-finder'));
      expect(finder, findsOneWidget);
      // Running indicator dot exists for 访达
      expect(find.byKey(const ValueKey('dock-running-访达')), findsOneWidget);

      // Launchpad slot is labeled 启动台
      expect(find.byKey(const ValueKey('dock-launcher')), findsOneWidget);

      // Right click Finder opens dockFinderMenu
      await tester.tap(finder, buttons: kSecondaryMouseButton);
      await tester.pump();
      expect(c.panel, ShellPanel.dockFinderMenu);

      // Verify Finder menu content
      await tester.pumpWidget(MaterialApp(home: MacShellView(controller: c)));
      await tester.pump();
      expect(
        find.byKey(const ValueKey('dock-finder-new-window')),
        findsOneWidget,
      );
    },
  );

  testWidgets(
    'Downloads secondary tap opens MacDockDownloadsMenu with sort, view, and display options',
    (tester) async {
      final (c, _, _) = await _fixture(tester);
      await tester.pumpWidget(
        MaterialApp(
          home: Align(
            alignment: Alignment.bottomCenter,
            child: SizedBox(height: 160, child: MacDockView(controller: c)),
          ),
        ),
      );
      await tester.pump();

      final dl = find.byKey(const ValueKey('dock-downloads'));
      expect(dl, findsOneWidget);

      // Secondary click opens dockDownloadsMenu
      await tester.tap(dl, buttons: kSecondaryMouseButton);
      await tester.pump();
      expect(c.panel, ShellPanel.dockDownloadsMenu);

      // Verify downloads menu content
      await tester.pumpWidget(MaterialApp(home: MacShellView(controller: c)));
      await tester.pump();
      expect(
        find.byKey(const ValueKey('dock-downloads-sort-name')),
        findsOneWidget,
      );
      expect(
        find.byKey(const ValueKey('dock-downloads-view-fan')),
        findsOneWidget,
      );
      expect(
        find.byKey(const ValueKey('dock-downloads-display-stack')),
        findsOneWidget,
      );
      expect(find.byKey(const ValueKey('dock-downloads-open')), findsOneWidget);

      // Selecting display as folder changes preference and closes menu
      await tester.tap(
        find.byKey(const ValueKey('dock-downloads-display-folder')),
      );
      await tester.pump();
      expect(c.macPreferences.downloadsDisplay, MacDownloadsDisplay.folder);
      expect(c.panel, ShellPanel.none);
    },
  );
}
