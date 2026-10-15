// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:io';

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:realand_shell/app_menu_models.dart';
import 'package:realand_shell/mac_shell_preferences.dart';
import 'package:realand_shell/models.dart';
import 'package:realand_shell/services/desktop_apps.dart';
import 'package:realand_shell/services/dock_folder_service.dart';
import 'package:realand_shell/services/global_app_menu_service.dart';
import 'package:realand_shell/services/realand_ipc.dart';
import 'package:realand_shell/services/system_service.dart';
import 'package:realand_shell/services/wallpaper_service.dart';
import 'package:realand_shell/state/shell_controller.dart';
import 'package:realand_shell/theme/mac_shell_theme.dart';
import 'package:realand_shell/ui/mac_dock_view.dart';
import 'package:realand_shell/ui/app_icon.dart';

const _editor = DesktopApp(
  id: 'editor.desktop',
  name: 'Editor',
  desktopFile: '/fixture/editor.desktop',
);
const _browser = DesktopApp(
  id: 'browser.desktop',
  name: 'Browser',
  desktopFile: '/fixture/browser.desktop',
);
const _notes = DesktopApp(
  id: 'notes.desktop',
  name: 'Notes',
  desktopFile: '/fixture/notes.desktop',
);
const _files = DesktopApp(
  id: 'org.gnome.Nautilus.desktop',
  name: 'Files',
  desktopFile: '/fixture/org.gnome.Nautilus.desktop',
  iconName: 'system-file-manager',
);

class _Ipc extends RealandIpc {
  final eventsController = StreamController<Map<String, dynamic>>.broadcast(
    sync: true,
  );
  final connectionsController = StreamController<bool>.broadcast(sync: true);
  final calls = <(String, Map<String, Object?>)>[];
  final minimizeTargets = <int, Map<String, Object?>>{};
  void Function(String, Map<String, Object?>)? onAction;
  @override
  Stream<Map<String, dynamic>> get events => eventsController.stream;
  @override
  Stream<bool> get connections => connectionsController.stream;
  @override
  void start() {}
  @override
  Future<void> action(
    String name, [
    Map<String, Object?> arguments = const {},
  ]) async {
    calls.add((name, arguments));
    if (name == 'SetMinimizeTarget') {
      final id = arguments['id']! as int;
      final target = arguments['target'];
      if (target is Map) {
        minimizeTargets[id] = Map<String, Object?>.from(target);
      } else {
        minimizeTargets.remove(id);
      }
    }
    onAction?.call(name, arguments);
  }

  @override
  void dispose() {
    unawaited(eventsController.close());
    unawaited(connectionsController.close());
    super.dispose();
  }
}

class _Apps extends DesktopApps {
  _Apps({this.includeFileManager = false, this.browserIconName});
  final String? browserIconName;
  final bool includeFileManager;
  final launched = <String>[];
  @override
  Future<List<DesktopApp>> load() async => [
    _editor,
    if (browserIconName == null)
      _browser
    else
      DesktopApp(
        id: _browser.id,
        name: _browser.name,
        desktopFile: _browser.desktopFile,
        iconName: browserIconName,
      ),
    _notes,
    if (includeFileManager) _files,
  ];
  @override
  Future<void> launch(DesktopApp app) async => launched.add(app.id);
}

class _System extends SystemService {
  int downloadsOpened = 0;
  @override
  Future<SystemSnapshot> read() async => const SystemSnapshot();
  @override
  Future<void> openDownloadsFolder() async {
    downloadsOpened++;
  }
}

class _Folder extends DockFolderService {
  @override
  Future<void> start() async {}
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

class _Fixture {
  _Fixture(this.controller, this.ipc, this.apps, this.system);
  final ShellController controller;
  final _Ipc ipc;
  final _Apps apps;
  final _System system;
  final geometry = <(Size, Rect)>[];
  final inputRegions = <List<Rect>>[];
  final presentation = <(Rect, Map<int, Rect>, bool)>[];
}

Future<_Fixture> _fixture(
  WidgetTester tester, {
  MacShellPreferences preferences = const MacShellPreferences(),
  bool includeFileManager = false,
  String? browserIconName,
}) async {
  final ipc = _Ipc();
  final apps = _Apps(
    includeFileManager: includeFileManager,
    browserIconName: browserIconName,
  );
  final system = _System();
  final controller =
      ShellController(
          ipc: ipc,
          desktopApps: apps,
          dockFolderService: _Folder(),
          system: system,
          wallpaperService: _Wallpapers(),
          appMenuService: _Menus(),
          tray: false,
          notifications: false,
          powerProfilesEnabled: false,
          restoreWallpaperOnStart: false,
        )
        ..setShellStyle(ShellStyle.macos)
        ..setPinnedAppIds([_browser.id, _notes.id])
        ..setMacPreferences(preferences)
        ..setReduceMotion(true);
  addTearDown(controller.dispose);
  await tester.runAsync(controller.start);
  await tester.pump(const Duration(milliseconds: 180));
  ipc.connectionsController.add(true);
  ipc.eventsController.add({
    'WindowsChanged': {
      'windows': [
        {
          'id': 21,
          'app_id': 'editor',
          'title': 'Document',
          'is_focused': true,
          'is_minimized': false,
        },
      ],
    },
  });
  return _Fixture(controller, ipc, apps, system);
}

Future<void> _mount(
  WidgetTester tester,
  _Fixture fixture, {
  bool registerTargets = false,
  double devicePixelRatio = 1,
}) async {
  const output = Size(1200, 900);
  tester.view.devicePixelRatio = devicePixelRatio;
  tester.view.physicalSize = output * devicePixelRatio;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  await tester.pumpWidget(
    MaterialApp(
      theme: buildMacShellTheme(Brightness.light),
      home: AnimatedBuilder(
        animation: fixture.controller,
        builder: (context, _) {
          final preferences = fixture.controller.macPreferences;
          final thickness = MacDockView.surfaceThickness(preferences);
          final dock = MacDockView(
            controller: fixture.controller,
            availableWidth: output.width,
            availableHeight: output.height,
            onGeometryChanged: (size, input) =>
                fixture.geometry.add((size, input)),
            onInputRegionsChanged: fixture.inputRegions.add,
            onPresentationChanged: (glass, targets, reduced) {
              fixture.presentation.add((glass, targets, reduced));
              if (registerTargets) {
                unawaited(
                  fixture.controller.setDockMinimizeTargets(
                    targets,
                    reduceMotion: reduced,
                  ),
                );
              }
            },
          );
          return Stack(
            children: [
              switch (preferences.dockPosition) {
                MacDockPosition.bottom => Positioned(
                  left: 0,
                  right: 0,
                  bottom: 0,
                  height: thickness,
                  child: dock,
                ),
                MacDockPosition.left => Positioned(
                  left: 0,
                  top: 0,
                  bottom: 0,
                  width: thickness,
                  child: dock,
                ),
                MacDockPosition.right => Positioned(
                  right: 0,
                  top: 0,
                  bottom: 0,
                  width: thickness,
                  child: dock,
                ),
              },
            ],
          );
        },
      ),
    ),
  );
  await tester.pumpAndSettle();
}

Finder _icon(String id) => find.byKey(ValueKey('dock-app-$id'));
double _scale(WidgetTester tester, String label) => tester
    .widget<Transform>(find.byKey(ValueKey('dock-scale-$label')))
    .transform
    .entry(0, 0);

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  const channel = MethodChannel('realand.shell/window');
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

  for (final dpr in [1.0, 1.25, 1.5, 2.0]) {
    testWidgets('Dock $dpr DPR prepares a sharp 2x icon before hover', (
      tester,
    ) async {
      final requests = <Map<Object?, Object?>>[];
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(channel, (call) async {
            if (call.method == 'outputName') return 'QA-1';
            if (call.method != 'appIcon') return null;
            requests.add(Map<Object?, Object?>.from(call.arguments as Map));
            return File('assets/macos/generic-application.png').absolute.path;
          });
      final f = await _fixture(
        tester,
        browserIconName: 'dock-sharpness-$dpr',
        preferences: const MacShellPreferences(
          dockSize: 64,
          magnificationScale: 2,
          useOriginalIcons: false,
        ),
      );
      await _mount(tester, f, devicePixelRatio: dpr);
      final icon = find.descendant(
        of: _icon(_browser.id),
        matching: find.byType(AppIcon),
      );
      expect(tester.widget<AppIcon>(icon).rasterSize, 128);
      final raw = find.descendant(
        of: _icon(_browser.id),
        matching: find.byType(RawImage),
      );
      for (var attempt = 0; attempt < 100; attempt++) {
        await tester.pump();
        if (raw.evaluate().isNotEmpty &&
            tester.widget<RawImage>(raw).image != null) {
          break;
        }
        await tester.runAsync(
          () => Future<void>.delayed(const Duration(milliseconds: 10)),
        );
      }
      final before = tester.widget<RawImage>(raw).image;
      expect(before, isNotNull);
      expect(before!.width, (128 * dpr).ceil());
      for (final key in ['dock-launcher', 'dock-overview']) {
        final utilityImage = find.descendant(
          of: find.byKey(ValueKey(key)),
          matching: find.byType(RawImage),
        );
        for (var attempt = 0; attempt < 100; attempt++) {
          if (utilityImage.evaluate().isNotEmpty &&
              tester.widget<RawImage>(utilityImage).image != null) {
            break;
          }
          await tester.runAsync(
            () => Future<void>.delayed(const Duration(milliseconds: 10)),
          );
          await tester.pump();
        }
        expect(
          tester.widget<RawImage>(utilityImage).image?.width,
          (128 * dpr).ceil(),
        );
      }
      expect(requests, hasLength(1));
      expect(requests.single['pixelSize'], (128 * dpr).ceil());
      final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
      await mouse.addPointer(location: const Offset(500, 500));
      for (var move = 0; move < 4; move++) {
        await mouse.moveTo(tester.getCenter(_icon(_browser.id)));
        await tester.pumpAndSettle();
      }
      expect(_scale(tester, 'Browser'), greaterThan(1.9));
      expect(identical(tester.widget<RawImage>(raw).image, before), isTrue);
      expect(requests, hasLength(1));
      await mouse.moveTo(const Offset(500, 500));
      await tester.pumpAndSettle();
      expect(_scale(tester, 'Browser'), 1);
      expect(identical(tester.widget<RawImage>(raw).image, before), isTrue);
      expect(requests, hasLength(1));
      await mouse.removePointer();
      await tester.pumpAndSettle();
      expect(tester.takeException(), isNull);
    });
  }

  testWidgets(
    'live size, magnification and utility preferences update usable Dock controls',
    (tester) async {
      final f = await _fixture(
        tester,
        preferences: const MacShellPreferences(
          dockSize: 32,
          magnification: false,
          showRunningIndicators: false,
          showLaunchpad: false,
          showMissionControl: false,
          showDownloads: false,
        ),
      );
      await _mount(tester, f);
      expect(
        tester
            .getSize(
              find.byKey(const ValueKey('dock-icon-size-editor.desktop')),
            )
            .width,
        32,
      );
      expect(find.byKey(const ValueKey('dock-running-Editor')), findsNothing);
      for (final id in ['dock-launcher', 'dock-overview', 'dock-downloads']) {
        expect(find.byKey(ValueKey(id)), findsNothing);
      }
      final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
      await mouse.addPointer(location: const Offset(500, 500));
      await mouse.moveTo(tester.getCenter(_icon(_editor.id)));
      await tester.pumpAndSettle();
      expect(_scale(tester, 'Editor'), 1);
      f.controller.setMacPreferences(
        const MacShellPreferences(dockSize: 64, magnificationScale: 2),
      );
      await tester.pumpAndSettle();
      expect(
        tester
            .getSize(
              find.byKey(const ValueKey('dock-icon-size-editor.desktop')),
            )
            .width,
        64,
      );
      expect(find.byKey(const ValueKey('dock-running-Editor')), findsOneWidget);
      expect(find.byKey(const ValueKey('dock-launcher')), findsOneWidget);
      expect(find.byKey(const ValueKey('dock-overview')), findsOneWidget);
      await mouse.moveTo(const Offset(500, 500));
      await mouse.moveTo(tester.getCenter(_icon(_editor.id)));
      await tester.pumpAndSettle();
      expect(_scale(tester, 'Editor'), greaterThan(1.8));
      await tester.tap(find.byKey(const ValueKey('dock-downloads')));
      await tester.pumpAndSettle();
      expect(f.controller.panel, ShellPanel.dockDownloads);
      expect(f.system.downloadsOpened, 0);
      expect(f.geometry.last.$1.height, 178);
      await mouse.removePointer();
      await tester.pumpAndSettle();
      expect(tester.takeException(), isNull);
    },
  );

  for (final position in MacDockPosition.values) {
    testWidgets(
      '$position 64px 2x launch accepts the full enlarged sprite edge',
      (tester) async {
        final f = await _fixture(
          tester,
          preferences: MacShellPreferences(
            dockPosition: position,
            dockSize: 64,
            magnificationScale: 2,
            showLaunchpad: false,
            showMissionControl: false,
            showDownloads: false,
          ),
        );
        await _mount(tester, f);
        f.controller.setReduceMotion(false);
        await tester.pumpAndSettle();
        final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
        await mouse.addPointer(location: const Offset(500, 500));
        // Settle a continuous magnification field as its neighbours expand.
        for (var move = 0; move < 4; move++) {
          await mouse.moveTo(tester.getCenter(_icon(_browser.id)));
          await tester.pumpAndSettle();
        }
        expect(_scale(tester, 'Browser'), greaterThan(1.9));
        await tester.tap(_icon(_browser.id));
        await tester.pump();
        await tester.pump(const Duration(milliseconds: 120));
        final dock = tester.getRect(find.byType(MacDockView));
        final browser = tester.getRect(_icon(_browser.id));
        final edgePoint = switch (position) {
          MacDockPosition.bottom => Offset(browser.center.dx, browser.top + 1),
          MacDockPosition.left => Offset(browser.right - 1, browser.center.dy),
          MacDockPosition.right => Offset(browser.left + 1, browser.center.dy),
        };
        final input = f.geometry.last.$2;
        final local = edgePoint - dock.topLeft;
        expect(input.contains(local), isTrue);
        expect(
          f.inputRegions.last.any((region) => region.contains(local)),
          isTrue,
        );
        final inset = switch (position) {
          MacDockPosition.bottom => input.top,
          MacDockPosition.left => dock.width - input.right,
          MacDockPosition.right => input.left,
        };
        final paintedInset = switch (position) {
          MacDockPosition.bottom => browser.top - dock.top,
          MacDockPosition.left => dock.right - browser.right,
          MacDockPosition.right => browser.left - dock.left,
        };
        // The 120 ms bounce frame is not its theoretical maximum lift. Input
        // must follow the complete sprite painted now, without claiming the
        // still-transparent pixels reserved for a future animation frame.
        expect(inset, closeTo(paintedInset, .01));
        for (final point in [
          browser.topLeft + const Offset(.01, .01),
          browser.topRight + const Offset(-.01, .01),
          browser.bottomLeft + const Offset(.01, -.01),
          browser.bottomRight - const Offset(.01, .01),
        ]) {
          expect(
            f.inputRegions.last.any(
              (region) => region.contains(point - dock.topLeft),
            ),
            isTrue,
          );
        }
        final count = f.apps.launched.length;
        await tester.tapAt(edgePoint);
        await tester.pumpAndSettle();
        expect(f.apps.launched.length, count + 1);
        await mouse.removePointer();
        await tester.pumpAndSettle();
        expect(tester.takeException(), isNull);
      },
    );

    testWidgets(
      '$position keeps upright icons, hit regions and Genie targets in the same geometry',
      (tester) async {
        final f = await _fixture(
          tester,
          preferences: MacShellPreferences(dockPosition: position),
        );
        await _mount(tester, f);
        final dock = tester.getRect(find.byType(MacDockView));
        final editor = tester.getRect(_icon(_editor.id));
        final browser = tester.getRect(_icon(_browser.id));
        if (position == MacDockPosition.bottom) {
          expect(browser.right, lessThan(editor.left));
          expect(f.geometry.last.$1.height, 120);
        } else {
          expect(browser.bottom, lessThan(editor.top));
          expect(f.geometry.last.$1.width, 120);
        }
        final matrix = tester
            .widget<Transform>(find.byKey(const ValueKey('dock-scale-Editor')))
            .transform;
        expect(matrix.entry(0, 1), 0);
        expect(matrix.entry(1, 0), 0);
        final target = f.presentation.last.$2[21]!;
        final local = editor.shift(-dock.topLeft);
        expect(target.center.dx, closeTo(local.center.dx, 1));
        expect(target.center.dy, closeTo(local.center.dy, 1));
        final input = f.geometry.last.$2;
        expect(input.left, greaterThanOrEqualTo(0));
        expect(input.top, greaterThanOrEqualTo(0));
        expect(input.right, lessThanOrEqualTo(dock.width));
        expect(input.bottom, lessThanOrEqualTo(dock.height));
        await tester.tapAt(editor.topLeft + const Offset(2, 2));
        await tester.pumpAndSettle();
        expect(f.ipc.calls.last.$1, 'FocusWindow');
        expect(f.ipc.calls.last.$2, {'id': 21});
        final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
        await mouse.addPointer(location: const Offset(500, 500));
        await mouse.moveTo(tester.getCenter(_icon(_editor.id)));
        await tester.pumpAndSettle();
        expect(_scale(tester, 'Editor'), greaterThan(1.35));
        expect(find.byKey(const ValueKey('mac-dock-label')), findsOneWidget);
        await mouse.removePointer();
        await tester.pumpAndSettle();
        expect(tester.takeException(), isNull);
      },
    );

    testWidgets(
      '$position auto-hide retains edge input, clears hidden targets and restores safely',
      (tester) async {
        final f = await _fixture(
          tester,
          preferences: MacShellPreferences(
            dockPosition: position,
            dockAutoHide: true,
          ),
        );
        await _mount(tester, f);
        expect(f.presentation.last.$1.isEmpty, isTrue);
        expect(f.presentation.last.$2, isEmpty);
        final edge = find.byKey(const ValueKey('mac-dock-edge-sensor'));
        final edgeSize = tester.getSize(edge);
        expect(
          position == MacDockPosition.bottom ? edgeSize.height : edgeSize.width,
          2,
        );
        final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
        await mouse.addPointer(location: const Offset(500, 500));
        await mouse.moveTo(tester.getCenter(edge));
        await tester.pumpAndSettle();
        expect(f.presentation.last.$1.isEmpty, isFalse);
        expect(f.presentation.last.$2, contains(21));
        f.controller.selectPanel(ShellPanel.quickSettings);
        await tester.pumpAndSettle();
        expect(f.presentation.last.$1.isEmpty, isTrue);
        expect(f.presentation.last.$2, isEmpty);
        f.controller.closePanel();
        await tester.pumpAndSettle();
        expect(f.presentation.last.$2, isEmpty);
        await mouse.moveTo(const Offset(500, 500));
        await mouse.moveTo(tester.getCenter(edge));
        await tester.pumpAndSettle();
        expect(f.presentation.last.$2, contains(21));
        await mouse.moveTo(const Offset(500, 500));
        await tester.pump(const Duration(milliseconds: 181));
        await tester.pumpAndSettle();
        expect(f.presentation.last.$2, isEmpty);
        await mouse.removePointer();
        expect(tester.takeException(), isNull);
      },
    );
  }

  testWidgets(
    'permanent Dock keeps painted animation targets while panel input stays blocked',
    (tester) async {
      final f = await _fixture(tester);
      await _mount(tester, f);
      expect(f.presentation.last.$2, contains(21));
      f.controller.selectPanel(ShellPanel.quickSettings);
      await tester.pumpAndSettle();
      expect(f.presentation.last.$1.isEmpty, isFalse);
      expect(f.presentation.last.$2, contains(21));
      final before = f.ipc.calls.length;
      await tester.tapAt(tester.getCenter(_icon(_editor.id)));
      await tester.pumpAndSettle();
      expect(f.ipc.calls.length, before);
      expect(f.controller.panel, ShellPanel.quickSettings);
      f.controller.closePanel();
      await tester.pumpAndSettle();
      expect(f.presentation.last.$2, contains(21));
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'right-click menu retains the registered Genie target through the real minimize action',
    (tester) async {
      final f = await _fixture(tester);
      f.controller.setReduceMotion(false);
      await _mount(tester, f, registerTargets: true);
      expect(f.ipc.minimizeTargets, contains(21));
      await tester.tap(
        _icon(_editor.id),
        kind: PointerDeviceKind.mouse,
        buttons: kSecondaryMouseButton,
      );
      await tester.pumpAndSettle();
      expect(f.controller.panel, ShellPanel.dockAppMenu);
      expect(f.presentation.last.$2, contains(21));
      expect(f.ipc.minimizeTargets, contains(21));
      expect(
        tester
            .widget<IgnorePointer>(
              find
                  .ancestor(
                    of: _icon(_editor.id),
                    matching: find.byKey(
                      const ValueKey('mac-dock-input-guard'),
                    ),
                  )
                  .first,
            )
            .ignoring,
        isTrue,
      );
      final before = f.ipc.calls.length;
      await tester.tapAt(tester.getCenter(_icon(_editor.id)));
      await tester.pumpAndSettle();
      expect(f.ipc.calls.length, before);
      expect(f.controller.panel, ShellPanel.dockAppMenu);
      var checkedAtMinimize = false;
      f.ipc.onAction = (name, arguments) {
        if (name != 'MinimizeWindow') return;
        checkedAtMinimize = true;
        expect(arguments, {'id': 21});
        expect(f.controller.panel, ShellPanel.dockAppMenu);
        final target = f.ipc.minimizeTargets[21]!;
        final rect = f.presentation.last.$2[21]!;
        expect(target, {
          'output': 'QA-1',
          'x': rect.left,
          'y': rect.top,
          'width': rect.width,
          'height': rect.height,
          'edge': 'bottom',
          'effect': 'genie',
        });
        expect(rect.isEmpty, isFalse);
        final registration = f.ipc.calls.lastWhere(
          (call) => call.$1 == 'SetMinimizeTarget',
        );
        expect(registration.$2['reduce_motion'], isFalse);
      };
      await f.controller.minimizeWindow(21);
      await tester.pumpAndSettle();
      expect(checkedAtMinimize, isTrue);
      expect(f.controller.panel, ShellPanel.none);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'Launchpad leaves Dock targets and real window activation available',
    (tester) async {
      final f = await _fixture(tester);
      await _mount(tester, f);
      f.controller.selectPanel(ShellPanel.launcher);
      await tester.pumpAndSettle();
      expect(f.presentation.last.$2, contains(21));
      final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
      await mouse.addPointer(location: const Offset(500, 500));
      await mouse.moveTo(tester.getCenter(_icon(_editor.id)));
      await tester.pumpAndSettle();
      await mouse.moveTo(
        tester.getCenter(_icon(_editor.id)) + const Offset(1, 0),
      );
      await tester.pumpAndSettle();
      expect(_scale(tester, _editor.name), greaterThan(1));
      expect(find.byKey(const ValueKey('mac-dock-label')), findsOneWidget);
      await mouse.removePointer();
      await tester.tap(_icon(_editor.id));
      await tester.pumpAndSettle();
      expect(f.ipc.calls.last.$1, 'FocusWindow');
      expect(f.ipc.calls.last.$2, {'id': 21});
      expect(f.controller.panel, ShellPanel.none);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'auto-hide Dock edge still reveals and activates while Launchpad is open',
    (tester) async {
      final f = await _fixture(
        tester,
        preferences: const MacShellPreferences(dockAutoHide: true),
      );
      await _mount(tester, f);
      f.controller.selectPanel(ShellPanel.launcher);
      await tester.pumpAndSettle();
      expect(f.presentation.last.$2, isEmpty);
      final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
      await mouse.addPointer(location: const Offset(500, 500));
      await mouse.moveTo(
        tester.getCenter(find.byKey(const ValueKey('mac-dock-edge-sensor'))),
      );
      await tester.pumpAndSettle();
      expect(f.presentation.last.$2, contains(21));
      await tester.tap(_icon(_editor.id));
      await tester.pumpAndSettle();
      expect(f.ipc.calls.last.$1, 'FocusWindow');
      expect(f.controller.panel, ShellPanel.none);
      await mouse.removePointer();
      await tester.pump(const Duration(milliseconds: 181));
      await tester.pumpAndSettle();
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'Finder stays first without accepting or initiating pin reorders',
    (tester) async {
      final f = await _fixture(tester, includeFileManager: true);
      f.controller.setPinnedAppIds([_browser.id, _files.id, _notes.id]);
      await _mount(tester, f);
      final finder = _icon(_files.id);
      expect(
        find.ancestor(
          of: finder,
          matching: find.byType(LongPressDraggable<String>),
        ),
        findsNothing,
      );
      expect(
        find.ancestor(of: finder, matching: find.byType(DragTarget<String>)),
        findsNothing,
      );
      final pinsBefore = List<String>.of(f.controller.pinnedAppIds);
      final gesture = await tester.startGesture(
        tester.getCenter(_icon(_notes.id)),
      );
      await tester.pump(const Duration(milliseconds: 360));
      await gesture.moveTo(tester.getCenter(finder));
      await tester.pump();
      await gesture.up();
      await tester.pumpAndSettle();
      expect(f.controller.pinnedAppIds, pinsBefore);
      expect(f.apps.launched, isEmpty);
      await tester.tap(finder);
      await tester.pumpAndSettle();
      expect(f.apps.launched, [_files.id]);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'launch animation switch stops bounce and pinned drag does not steal ordinary clicks',
    (tester) async {
      final f = await _fixture(tester);
      await _mount(tester, f);
      f.controller.setReduceMotion(false);
      await tester.pumpAndSettle();
      await tester.tap(_icon(_browser.id));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 120));
      expect(
        tester
            .widget<Transform>(
              find.byKey(const ValueKey('dock-bounce-browser.desktop')),
            )
            .transform
            .getTranslation()
            .y,
        lessThan(-1),
      );
      f.controller.setMacPreferences(
        f.controller.macPreferences.copyWith(animateOpeningApplications: false),
      );
      await tester.pumpAndSettle();
      expect(
        tester
            .widget<Transform>(
              find.byKey(const ValueKey('dock-bounce-browser.desktop')),
            )
            .transform
            .getTranslation()
            .y,
        0,
      );
      expect(f.apps.launched, [_browser.id]);
      final gesture = await tester.startGesture(
        tester.getCenter(_icon(_notes.id)),
      );
      await tester.pump(const Duration(milliseconds: 360));
      await gesture.moveTo(tester.getCenter(_icon(_browser.id)));
      await tester.pump();
      await gesture.up();
      await tester.pumpAndSettle();
      expect(f.controller.pinnedAppIds.take(2), [_notes.id, _browser.id]);
      expect(f.apps.launched, [_browser.id]);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'mouse drag reorders pinned apps freely to the right, left, and end divider',
    (tester) async {
      final f = await _fixture(tester);
      f.controller.setPinnedAppIds([_editor.id, _browser.id, _notes.id]);
      await _mount(tester, f);

      // 1. Drag Editor to the right of Notes (trailing half)
      final mouse = await tester.startGesture(
        tester.getCenter(_icon(_editor.id)),
        kind: PointerDeviceKind.mouse,
      );
      await mouse.moveBy(const Offset(15, 0));
      await tester.pump();
      final notesRect = tester.getRect(_icon(_notes.id));
      await mouse.moveTo(Offset(notesRect.right - 4, notesRect.center.dy));
      await tester.pump();
      await mouse.up();
      await mouse.removePointer();
      await tester.pumpAndSettle();
      expect(f.controller.pinnedAppIds, [_browser.id, _notes.id, _editor.id]);

      // 2. Drag Editor to the left of Notes (leading half)
      final mouse2 = await tester.startGesture(
        tester.getCenter(_icon(_editor.id)),
        kind: PointerDeviceKind.mouse,
      );
      await mouse2.moveBy(const Offset(-15, 0));
      await tester.pump();
      final notesRect2 = tester.getRect(_icon(_notes.id));
      await mouse2.moveTo(Offset(notesRect2.left + 4, notesRect2.center.dy));
      await tester.pump();
      await mouse2.up();
      await mouse2.removePointer();
      await tester.pumpAndSettle();
      expect(f.controller.pinnedAppIds, [_browser.id, _editor.id, _notes.id]);

      // 3. Drag Browser onto divider to move it to the end
      final dividerFinder = find.byKey(
        const ValueKey('dock-divider-utilities-divider'),
      );
      expect(dividerFinder, findsOneWidget);
      final mouse3 = await tester.startGesture(
        tester.getCenter(_icon(_browser.id)),
        kind: PointerDeviceKind.mouse,
      );
      await mouse3.moveBy(const Offset(15, 0));
      await tester.pump();
      await mouse3.moveTo(tester.getCenter(dividerFinder));
      await tester.pump();
      await mouse3.up();
      await mouse3.removePointer();
      await tester.pumpAndSettle();
      expect(f.controller.pinnedAppIds, [_editor.id, _notes.id, _browser.id]);
      expect(tester.takeException(), isNull);
    },
  );
}
