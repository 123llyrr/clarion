// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math' as math;
import 'dart:ui' as ui;

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:clarion_shell/app_menu_models.dart';
import 'package:clarion_shell/mac_shell_preferences.dart';
import 'package:clarion_shell/main.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/preferences.dart';
import 'package:clarion_shell/services/desktop_apps.dart';
import 'package:clarion_shell/services/dock_folder_service.dart';
import 'package:clarion_shell/services/global_app_menu_service.dart';
import 'package:clarion_shell/services/power_profiles_service.dart';
import 'package:clarion_shell/services/clarion_ipc.dart';
import 'package:clarion_shell/services/system_service.dart';
import 'package:clarion_shell/services/trash_state_service.dart';
import 'package:clarion_shell/services/wallpaper_service.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/mac_launchpad_layout.dart';
import 'package:clarion_shell/ui/mac_launcher_panel.dart';
import 'package:clarion_shell/ui/mac_dock_view.dart';

const _window = MethodChannel('clarion.shell/window');
const _application = 'native-app-0';

class _Apps extends DesktopApps {
  _Apps(this.root);
  final String root;
  @override
  Future<List<DesktopApp>> load() async => [
    for (var i = 0; i < 120; i++)
      DesktopApp(
        id: 'native-app-$i',
        name: 'Native Application $i',
        desktopFile: '$root/native-app-$i.desktop',
        iconPath: '$root/fixture-icon-${i % 4}.png',
        keywords: i == 119 ? const ['launchpad-needle'] : const [],
      ),
  ];
}

class _Controller extends ShellController {
  _Controller({
    required super.ipc,
    required super.desktopApps,
    required super.appMenuService,
    required super.system,
    required super.wallpaperService,
    required super.dockFolderService,
    required super.trashStateService,
  }) : super(
         notifications: false,
         tray: false,
         powerProfilesEnabled: false,
         restoreWallpaperOnStart: false,
         powerProfiles: PowerProfilesService(demo: true),
       );
  final launches = <String>[];
  @override
  Future<void> launchApp(DesktopApp app) async {
    launches.add(app.id);
    closePanel();
  }
}

class _ViewPacketFilter {
  _ViewPacketFilter(this.binding) {
    original = binding.platformDispatcher.onPointerDataPacket;
    binding.platformDispatcher.onPointerDataPacket = handle;
  }
  final IntegrationTestWidgetsFlutterBinding binding;
  ui.PointerDataPacketCallback? original;
  final ignored = <Map<String, Object?>>[];
  void handle(ui.PointerDataPacket packet) {
    final ids = binding.renderViews.map((v) => v.flutterView.viewId).toSet();
    final valid = <ui.PointerData>[];
    for (final data in packet.data) {
      if (ids.contains(data.viewId)) {
        valid.add(data);
      } else {
        ignored.add({'viewId': data.viewId, 'change': data.change.name});
      }
    }
    if (valid.isNotEmpty) original?.call(ui.PointerDataPacket(data: valid));
  }

  void restore() => binding.platformDispatcher.onPointerDataPacket = original;
}

class _Ipc extends ClarionIpc {
  final connection = StreamController<bool>.broadcast(sync: true);
  final event = StreamController<Map<String, dynamic>>.broadcast(sync: true);
  final actions = <Map<String, Object?>>[];
  int disposals = 0;
  @override
  Stream<bool> get connections => connection.stream;
  @override
  Stream<Map<String, dynamic>> get events => event.stream;
  @override
  void start() {
    connection.add(true);
    event.add({
      'WorkspacesChanged': {
        'workspaces': [
          {
            'id': 1,
            'idx': 1,
            'output': 'winit',
            'is_active': true,
            'is_focused': true,
          },
        ],
      },
      'WindowsChanged': {
        'windows': [
          {
            'id': 91,
            'app_id': _application,
            'title': 'Isolated document',
            'workspace_id': 1,
            'is_focused': true,
            'is_minimized': false,
          },
        ],
      },
    });
  }

  @override
  Future<dynamic> request(Object request) async {
    if (request != 'Outputs') throw StateError('Forbidden fixture IPC request');
    return {
      'Outputs': {
        'winit': {
          'name': 'winit',
          'logical': (await _state())['monitorGeometry'],
        },
      },
    };
  }

  @override
  Future<void> action(
    String name, [
    Map<String, Object?> arguments = const {},
  ]) async {
    if (name != 'SetMinimizeTarget') {
      throw StateError('Fixture forbids actual application/window actions');
    }
    actions.add({'name': name, 'arguments': Map.of(arguments)});
  }

  @override
  void dispose() {
    disposals++;
    unawaited(connection.close());
    unawaited(event.close());
    super.dispose();
  }
}

class _Menus extends GlobalAppMenuService {
  _Menus() : super(hostRegistrar: false);
  @override
  bool get available => false;
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

class _System extends SystemService {
  _System()
    : super(command: (p, a) async => throw StateError('No system writes'));
  @override
  Future<SystemSnapshot> read() async =>
      const SystemSnapshot(volume: .5, brightness: .5, batteryFraction: .8);
}

class _Wallpapers extends WallpaperService {
  @override
  Future<void> restore({bool applySaved = false}) async {}
  @override
  Future<List<ShellWallpaper>> scan() async => [];
}

Future<Map<String, dynamic>> _state() async => Map<String, dynamic>.from(
  (await _window.invokeMapMethod<String, dynamic>('windowState'))!,
);

Future<Map<String, dynamic>> _origin(String role) async =>
    Map<String, dynamic>.from(
      (await _window.invokeMapMethod<String, dynamic>('surfaceOrigin', role))!,
    );

Rect _painted(WidgetTester tester, Finder finder) =>
    Rect.fromPoints(tester.getTopLeft(finder), tester.getBottomRight(finder));

Map<String, double> _rect(Rect value) => {
  'x': value.left,
  'y': value.top,
  'width': value.width,
  'height': value.height,
};

Future<void> _until(
  WidgetTester tester,
  bool Function() ready,
  String why,
) async {
  final deadline = DateTime.now().add(const Duration(seconds: 15));
  while (!ready()) {
    if (DateTime.now().isAfter(deadline)) fail('Timed out: $why');
    await tester.pump(const Duration(milliseconds: 20));
  }
}

Future<Map<String, dynamic>> _native(
  WidgetTester tester,
  ShellController controller, {
  required bool expanded,
}) async {
  final deadline = DateTime.now().add(const Duration(seconds: 15));
  Map<String, dynamic> state = {};
  Object? previousGeometry;
  var stableFrames = 0;
  do {
    await tester.pump(const Duration(milliseconds: 20));
    state = await _state();
    final dock = state['dock'] as Map?;
    final geometry = state['monitorGeometry'] as Map;
    final launcher = expanded && controller.panel == ShellPanel.launcher;
    if (state['style'] != 'macos' ||
        state['expanded'] != expanded ||
        state['height'] !=
            (expanded
                ? geometry['height']
                : controller.macPreferences.menuBarHeight.toInt()) ||
        state['width'] != geometry['width'] ||
        dock?['ready'] != true ||
        dock?['position'] != controller.macPreferences.dockPosition.name ||
        (expanded && state['focused'] != true)) {
      continue;
    }
    final blocked = dock!['inputWidth'] == 0 && dock['inputHeight'] == 0;
    // Launchpad raises the Dock to Overlay and keeps its real icon input
    // available. Other expanded panels block Dock input to prevent click-through.
    if (blocked != (expanded && !launcher)) continue;
    final dockView = find.byType(MacDockView);
    final glass = find.byKey(const ValueKey('mac-dock-glass'));
    if (dockView.evaluate().isEmpty || glass.evaluate().isEmpty) continue;
    final paintedSize = tester.getSize(dockView);
    final view = View.of(tester.element(dockView));
    final viewport = view.physicalSize / view.devicePixelRatio;
    final nativeSize = Size(
      (dock['width'] as num).toDouble(),
      (dock['height'] as num).toDouble(),
    );
    if ((paintedSize.width - nativeSize.width).abs() > .01 ||
        (paintedSize.height - nativeSize.height).abs() > .01 ||
        (viewport.width - nativeSize.width).abs() > .01 ||
        (viewport.height - nativeSize.height).abs() > .01) {
      continue;
    }
    final painted = _painted(tester, glass);
    final horizontal =
        controller.macPreferences.dockPosition == MacDockPosition.bottom;
    if (!expanded &&
        ((horizontal
                        ? painted.left - (dock['inputX'] as num)
                        : painted.top - (dock['inputY'] as num))
                    .abs() >
                1.01 ||
            (horizontal
                        ? painted.width - (dock['inputWidth'] as num)
                        : painted.height - (dock['inputHeight'] as num))
                    .abs() >
                1.01)) {
      continue;
    }
    final signature = (
      nativeSize,
      paintedSize,
      viewport,
      painted,
      controller.dockGeometryRevision,
    );
    stableFrames = signature == previousGeometry ? stableFrames + 1 : 0;
    previousGeometry = signature;
    if (stableFrames < 3) continue;
    expect(state['preview'], isFalse);
    expect(state['layer'], expanded ? 'Overlay' : 'Top');
    expect(state['keyboard'], expanded ? 'Exclusive' : 'None');
    expect(state['exclusiveZone'], -1);
    expect(dock['visible'], isTrue);
    expect(dock['layer'], launcher ? 'Overlay' : 'Top');
    if (expanded && !launcher) {
      expect(dock['inputWidth'], 0);
      expect(dock['inputHeight'], 0);
    } else {
      expect(dock['inputWidth'] as num, greaterThan(0));
      expect(dock['inputHeight'] as num, greaterThan(0));
    }
    expect(dock['keyboard'], 'None');
    expect(dock['exclusiveZone'], -1);
    expect(state['mainViewId'], isNot(state['dockViewId']));
    return state;
  } while (DateTime.now().isBefore(deadline));
  fail('Native Dock/panel state did not settle: $state');
}

Future<Map<String, Object>> _visiblePixels(
  File capture,
  Size output,
  Rect expected, {
  bool monochromeIcon = false,
}) async {
  final codec = await ui.instantiateImageCodec(await capture.readAsBytes());
  final image = (await codec.getNextFrame()).image;
  try {
    final bytes = (await image.toByteData(format: ui.ImageByteFormat.rawRgba))!;
    final colors = <int>{};
    var bright = 0;
    var minimum = 255;
    var maximum = 0;
    final x0 = math.max(0, (expected.left * image.width / output.width).ceil());
    final y0 = math.max(
      0,
      (expected.top * image.height / output.height).ceil(),
    );
    final x1 = math.min(
      image.width,
      (expected.right * image.width / output.width).floor(),
    );
    final y1 = math.min(
      image.height,
      (expected.bottom * image.height / output.height).floor(),
    );
    for (var y = y0; y < y1; y += 2) {
      for (var x = x0; x < x1; x += 2) {
        final offset = (y * image.width + x) * 4;
        final r = bytes.getUint8(offset),
            g = bytes.getUint8(offset + 1),
            b = bytes.getUint8(offset + 2);
        colors.add((r ~/ 8 << 10) | (g ~/ 8 << 5) | (b ~/ 8));
        final light = (r + g + b) ~/ 3;
        minimum = math.min(minimum, light);
        maximum = math.max(maximum, light);
        if (math.max(r, math.max(g, b)) > 150) bright++;
      }
    }
    return {
      'quantizedColors': colors.length,
      'brightSamples': bright,
      'brightnessRange': maximum - minimum,
      'monochromeIcon': monochromeIcon,
    };
  } finally {
    image.dispose();
    codec.dispose();
  }
}

Future<int> _changedPixels(
  File baseline,
  File capture,
  Size output,
  Rect roi,
) async {
  final codecs = [
    await ui.instantiateImageCodec(await baseline.readAsBytes()),
    await ui.instantiateImageCodec(await capture.readAsBytes()),
  ];
  final images = [
    (await codecs[0].getNextFrame()).image,
    (await codecs[1].getNextFrame()).image,
  ];
  try {
    final data = [
      (await images[0].toByteData(format: ui.ImageByteFormat.rawRgba))!,
      (await images[1].toByteData(format: ui.ImageByteFormat.rawRgba))!,
    ];
    var changed = 0;
    for (var y = roi.top.ceil(); y < roi.bottom.floor(); y++) {
      for (var x = roi.left.ceil(); x < roi.right.floor(); x++) {
        if (x < 0 || y < 0 || x >= output.width || y >= output.height) continue;
        final offsets = [
          for (final image in images)
            (((y * image.height / output.height).floor() * image.width) +
                    (x * image.width / output.width).floor()) *
                4,
        ];
        var different = false;
        for (var channel = 0; channel < 3; channel++) {
          if ((data[0].getUint8(offsets[0] + channel) -
                      data[1].getUint8(offsets[1] + channel))
                  .abs() >
              8) {
            different = true;
          }
        }
        if (different) changed++;
      }
    }
    return changed;
  } finally {
    for (final image in images) {
      image.dispose();
    }
    for (final codec in codecs) {
      codec.dispose();
    }
  }
}

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.onlyPumps;
  final filter = _ViewPacketFilter(binding);
  testWidgets(
    'private native Launchpad preserves page, folder membership and cancellation',
    (tester) async {
      expect(Platform.isLinux, isTrue);
      expect(Platform.environment['CLARION_SHELL_ISOLATED_TEST'], '1');
      final root = Platform.environment['CLARION_SHELL_TEST_ROOT']!;
      for (final (child, parent) in [
        ('WAYLAND_DISPLAY', 'CLARION_SHELL_PARENT_WAYLAND_DISPLAY'),
        ('CLARION_SOCKET', 'CLARION_SHELL_PARENT_SOCKET'),
        ('DBUS_SESSION_BUS_ADDRESS', 'CLARION_SHELL_PARENT_DBUS_ADDRESS'),
      ]) {
        expect(Platform.environment[child], isNotEmpty);
        expect(
          Platform.environment[child],
          isNot(Platform.environment[parent]),
        );
      }
      for (final key in [
        'XDG_CONFIG_HOME',
        'XDG_DATA_HOME',
        'XDG_CACHE_HOME',
        'XDG_STATE_HOME',
      ]) {
        expect(Platform.environment[key]!.startsWith('$root/'), isTrue);
      }
      final captures = Directory(
        Platform.environment['CLARION_SHELL_CAPTURE_DIR']!,
      );
      expect(captures.path.startsWith('$root/'), isTrue);
      await tester.runAsync(() async {
        await captures.create(recursive: true);
        await Directory('$root/fixture-downloads').create(recursive: true);
        final assets = ['calendar', 'textedit', 'finder', 'preview'];
        for (var i = 0; i < assets.length; i++) {
          final bytes = await rootBundle.load('assets/macos/${assets[i]}.png');
          await File('$root/fixture-icon-$i.png').writeAsBytes(
            bytes.buffer.asUint8List(bytes.offsetInBytes, bytes.lengthInBytes),
          );
        }
        for (var i = 0; i < 120; i++) {
          await File('$root/native-app-$i.desktop').writeAsString(
            '[Desktop Entry]\nType=Application\nName=Native fixture $i\nExec=false\n',
          );
        }
      });
      final ipc = _Ipc();
      final controller = _Controller(
        ipc: ipc,
        desktopApps: _Apps(root),
        appMenuService: _Menus(),
        system: _System(),
        wallpaperService: _Wallpapers(),
        dockFolderService: DockFolderService(
          directory: '$root/fixture-downloads',
        ),
        trashStateService: TrashStateService(
          environment: {'HOME': root, 'XDG_DATA_HOME': '$root/data'},
          command: (p, a) async {
            expect(p, 'gio');
            expect(a, ['list', 'trash:///']);
            return '';
          },
        ),
      );
      const mac = MacShellPreferences(magnification: false);
      const prefs = ShellPreferences(
        shellStyle: ShellStyle.macos,
        reduceMotion: true,
        macShell: mac,
        pinnedAppIds: [_application],
      );
      controller.setShellStyle(ShellStyle.macos);
      controller.setReduceMotion(true);
      controller.setMacPreferences(mac);
      controller.setPinnedAppIds(prefs.pinnedAppIds);
      final phases = <Map<String, Object?>>[];
      final evidence = <String, Object?>{
        'passed': false,
        'fixture': 'launchpad',
        'phases': phases,
        'scope':
            'Private GTK main/Dock views, actual native geometry and grim pixels. Application catalogue, window/menu/system services are fakes; app launch override only records identities. Layout files stay inside private XDG directories.',
        'inputScope':
            'Injected Flutter mouse/key events with explicit view IDs; not physical compositor event delivery certification.',
        'renderer':
            Platform.environment['CLARION_SHELL_TEST_RENDERER'] ?? 'vulkan',
      };
      int? mainId, dockId;
      Element? mainElement, dockElement;
      var pointerSequence = 100;
      final activePointers = <(TestPointer, ui.FlutterView)>[];
      Finder app(int id) => find.byKey(ValueKey('app-native-app-$id'));
      double page() => tester
          .widget<PageView>(find.byKey(const ValueKey('mac-launchpad-pages')))
          .controller!
          .page!;
      MacLaunchpadLayout layout() => tester
          .widget<MacLauncherPanel>(find.byType(MacLauncherPanel))
          .layout!;
      Future<void> flushEvidence() async => tester.runAsync(() async {
        await File(
          '${captures.path}/mac-launchpad-interaction-native-evidence.json',
        ).writeAsString(const JsonEncoder.withIndent('  ').convert(evidence));
      });
      Future<(TestPointer, ui.FlutterView)> down(Finder finder) async {
        final view = View.of(tester.element(finder));
        final pointer = TestPointer(pointerSequence++, PointerDeviceKind.mouse);
        final pair = (pointer, view);
        activePointers.add(pair);
        final point = tester.getCenter(finder);
        await tester.sendEventToBinding(
          pointer.addPointer(location: point, view: view),
        );
        await tester.sendEventToBinding(pointer.down(point, view: view));
        return pair;
      }

      Future<void> finish(
        (TestPointer, ui.FlutterView) pair, {
        bool cancelled = false,
      }) async {
        await tester.sendEventToBinding(
          cancelled ? pair.$1.cancel(view: pair.$2) : pair.$1.up(view: pair.$2),
        );
        await tester.sendEventToBinding(pair.$1.removePointer(view: pair.$2));
        activePointers.remove(pair);
        await tester.pump();
      }

      Future<void> tap(Finder finder) async => finish(await down(finder));
      Future<void> drag(
        Finder finder,
        Offset destination, {
        bool cancelled = false,
      }) async {
        final pair = await down(finder);
        final initial = tester.getCenter(finder);
        await tester.sendEventToBinding(
          pair.$1.move(initial + const Offset(25, 0), view: pair.$2),
        );
        await tester.pump();
        await tester.sendEventToBinding(
          pair.$1.move(destination, view: pair.$2),
        );
        await tester.pump();
        await finish(pair, cancelled: cancelled);
        await tester.pumpAndSettle();
      }

      Future<File> record(
        String name,
        Finder target, {
        File? baseline,
        int? minimumChanged,
      }) async {
        final state = await _native(tester, controller, expanded: true);
        mainId ??= state['mainViewId'] as int;
        dockId ??= state['dockViewId'] as int;
        expect(state['mainViewId'], mainId);
        expect(state['dockViewId'], dockId);
        expect(binding.platformDispatcher.views.length, 2);
        final main = find.byType(ShellApplication),
            dock = find.byType(DockApplication);
        mainElement ??= tester.element(main);
        dockElement ??= tester.element(dock);
        expect(tester.element(main), same(mainElement));
        expect(tester.element(dock), same(dockElement));
        final origin = await _origin('main');
        expect(origin['outputX'], 0);
        expect(origin['outputY'], 0);
        final geometry = state['monitorGeometry'] as Map;
        final output = Size(
          (geometry['width'] as num).toDouble(),
          (geometry['height'] as num).toDouble(),
        );
        final bounds = _painted(tester, target);
        expect(
          (Offset.zero & output).inflate(.1).contains(bounds.topLeft),
          isTrue,
        );
        expect(
          (Offset.zero & output).inflate(.1).contains(bounds.bottomRight),
          isTrue,
        );
        final capture = File('${captures.path}/$name.png');
        final started = DateTime.now(),
            deadline = DateTime.now().add(const Duration(seconds: 5));
        var attempts = 0;
        Map<String, Object>? pixels;
        int? changed;
        while (true) {
          attempts++;
          await tester.pump(const Duration(milliseconds: 20));
          await tester.runAsync(() async {
            final result = await Process.run('grim', [
              '-o',
              Platform.environment['CLARION_SHELL_TEST_OUTPUT'] ?? 'winit',
              capture.path,
            ]).timeout(const Duration(seconds: 15));
            expect(
              result.exitCode,
              0,
              reason: '${result.stdout}\n${result.stderr}',
            );
            pixels = await _visiblePixels(capture, output, bounds);
            if (baseline != null) {
              changed = await _changedPixels(baseline, capture, output, bounds);
            }
          });
          final ready =
              (pixels!['quantizedColors'] as int) > 20 &&
              (pixels!['brightSamples'] as int) > 30 &&
              (pixels!['brightnessRange'] as int) > 100 &&
              (minimumChanged == null || (changed ?? 0) >= minimumChanged);
          if (ready) break;
          if (DateTime.now().isAfter(deadline)) {
            evidence['lastPresentationFailure'] = {
              'name': name,
              'pixels': pixels,
              'changed': changed,
              'native': await _state(),
            };
            await flushEvidence();
            fail(
              'Actual Launchpad pixels did not present: $name $pixels changed=$changed',
            );
          }
        }
        phases.add({
          'name': name,
          'page': page(),
          'native': state,
          'origin': origin,
          'outputLocalRect': _rect(bounds),
          'pixelEvidence': pixels,
          'changedFromBaseline': changed,
          'captureAttempts': attempts,
          'presentationElapsedMs': DateTime.now()
              .difference(started)
              .inMilliseconds,
          'capture': capture.path,
          'layout': layout().entries.map((entry) => entry.toJson()).toList(),
          'views': [
            for (final view in binding.platformDispatcher.views)
              {
                'viewId': view.viewId,
                'dpr': view.devicePixelRatio,
                'physicalWidth': view.physicalSize.width,
                'physicalHeight': view.physicalSize.height,
              },
          ],
        });
        expect(tester.takeException(), isNull);
        await flushEvidence();
        return capture;
      }

      try {
        await tester.pumpWidget(
          ShellHost(controller: controller, preferences: prefs, persist: false),
          wrapWithView: false,
        );
        await controller.start();
        await _until(
          tester,
          () => controller.apps.length == 120,
          'private app catalogue',
        );
        controller.selectPanel(ShellPanel.launcher);
        await _native(tester, controller, expanded: true);
        await _until(
          tester,
          () =>
              find.byType(MacLauncherPanel).evaluate().isNotEmpty &&
              layout().loaded &&
              app(0).evaluate().isNotEmpty,
          'Launchpad layout load',
        );
        await tester.pumpAndSettle();
        final model = layout();
        final capacity = tester
            .widget<GridView>(
              find.byKey(const ValueKey('mac-launchpad-page-0')),
            )
            .childrenDelegate
            .estimatedChildCount!;
        expect(capacity, greaterThan(0));
        final a = capacity + 5, b = capacity + 6, c = capacity + 7;
        model.group('native-app-$a', 'native-app-$b');
        final folder = model.entries.firstWhere((entry) => entry.isFolder);
        model.group('native-app-$c', folder.id);
        await tester.pumpAndSettle();
        await tap(find.byKey(const ValueKey('mac-launchpad-dot-1')));
        await tester.pumpAndSettle();
        expect(page(), 1);
        final rootAppRect = _painted(tester, app(capacity));
        final rootCapture = await record(
          '01-launchpad-root-second-page',
          app(capacity),
        );
        evidence['capacity'] = capacity;
        evidence['folderId'] = folder.id;
        evidence['actualPrivateOutputs'] = await tester.runAsync(() async {
          final result = await Process.run(
            Platform.environment['CLARION_SHELL_CLARION_BIN']!,
            ['msg', '--json', 'outputs'],
          ).timeout(const Duration(seconds: 5));
          expect(result.exitCode, 0);
          return jsonDecode(result.stdout as String);
        });
        await tap(find.byKey(ValueKey('mac-launchpad-folder-${folder.id}')));
        await tester.pumpAndSettle();
        final panel = find.byKey(const ValueKey('mac-launchpad-folder-panel'));
        expect(panel, findsOneWidget);
        final rootCount = model.entries.length;
        final grid = _painted(
          tester,
          find.byKey(const ValueKey('mac-launchpad-pages')),
        );
        await drag(app(b), grid.bottomRight - const Offset(20, 20));
        expect(model.find(folder.id)!.appIds, [
          'native-app-$a',
          'native-app-$c',
          'native-app-$b',
        ]);
        expect(model.entries.length, rootCount);
        final insideCapture = await record(
          '02-launchpad-folder-internal-drop',
          panel,
          baseline: rootCapture,
          minimumChanged: 1000,
        );
        final ordered = model.orderedAppIds.toList();
        final background = _painted(
          tester,
          find.byKey(const ValueKey('mac-launchpad-background')),
        );
        final outside = Offset(background.left + 4, background.center.dy);
        await drag(app(a), outside, cancelled: true);
        expect(model.orderedAppIds, ordered);
        expect(panel, findsOneWidget);
        final cancelledCapture = await record(
          '03-launchpad-folder-cancel',
          panel,
        );
        final native = await _state();
        final geometry = native['monitorGeometry'] as Map;
        final output = Size(
          (geometry['width'] as num).toDouble(),
          (geometry['height'] as num).toDouble(),
        );
        final cancelChanged = await tester.runAsync(
          () => _changedPixels(
            insideCapture,
            cancelledCapture,
            output,
            _painted(tester, panel),
          ),
        );
        expect(
          cancelChanged,
          0,
          reason: 'Cancelled drag returns to the same actual folder pixels',
        );
        evidence['cancelledPixelChanges'] = cancelChanged;
        await tester.sendKeyEvent(LogicalKeyboardKey.escape);
        await tester.pumpAndSettle();
        expect(page(), 1);
        expect(_painted(tester, app(capacity)), rootAppRect);
        final search = find.byKey(const ValueKey('mac-launchpad-search'));
        await tester.enterText(search, 'launchpad-needle');
        await tester.pumpAndSettle();
        expect(app(119), findsOneWidget);
        expect(page(), 0);
        await tap(find.byKey(const ValueKey('mac-launchpad-clear-search')));
        await tester.pumpAndSettle();
        expect(page(), 1);
        expect(_painted(tester, app(capacity)), rootAppRect);
        await record('04-launchpad-search-clear-original-page', app(capacity));
        await tap(find.byKey(ValueKey('mac-launchpad-folder-${folder.id}')));
        await tester.pumpAndSettle();
        await drag(app(a), outside);
        expect(panel, findsNothing);
        expect(model.find(folder.id)!.appIds, [
          'native-app-$c',
          'native-app-$b',
        ]);
        expect(
          model.entries.any((entry) => entry.id == 'native-app-$a'),
          isTrue,
        );
        expect(model.orderedAppIds.toSet().length, 120);
        expect(page(), 1);
        await record('05-launchpad-external-drop-root-page', app(a));
        await tester.runAsync(model.save);
        final saved = await tester.runAsync(() => model.file!.readAsString());
        expect(saved, contains('native-app-$a'));
        evidence['persistedPrivateLayout'] = model.file!.path;
        expect(controller.launches, isEmpty);
        await tap(app(a));
        await _native(tester, controller, expanded: false);
        expect(controller.launches, ['native-app-$a']);
        evidence['fakeLaunches'] = controller.launches;
        evidence['ignoredNativePackets'] = filter.ignored;
        evidence['passed'] = true;
        await flushEvidence();
      } finally {
        for (final pair in List.of(activePointers)) {
          await finish(pair, cancelled: true);
        }
        controller.closePanel();
        await tester.pump();
        filter.restore();
        await tester.pumpWidget(
          const ViewCollection(views: []),
          wrapWithView: false,
        );
      }
    },
    timeout: const Timeout(Duration(minutes: 4)),
  );
}
