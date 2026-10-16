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
import 'package:clarion_shell/services/application_launcher.dart';
import 'package:clarion_shell/services/desktop_apps.dart';
import 'package:clarion_shell/services/dock_folder_service.dart';
import 'package:clarion_shell/services/global_app_menu_service.dart';
import 'package:clarion_shell/services/power_profiles_service.dart';
import 'package:clarion_shell/services/clarion_ipc.dart';
import 'package:clarion_shell/services/system_service.dart';
import 'package:clarion_shell/services/trash_state_service.dart';
import 'package:clarion_shell/services/wallpaper_service.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/ui/mac_downloads_stack_panel.dart';
import 'package:clarion_shell/ui/mac_dock_view.dart';

const _window = MethodChannel('clarion.shell/window');
const _application = 'isolated-dock-editor';
const _recents = ['isolated-dock-recent-a', 'isolated-dock-recent-b'];

// Live flutter_test hit testing requires a mounted RenderView. Filter only
// native packets for views still being attached or already removed. Injected
// tester events are distinct from physical compositor input certification.
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

class _Apps extends DesktopApps {
  _Apps(this.root);
  final String root;
  @override
  Future<List<DesktopApp>> load() async => [
    for (final id in [_application, ..._recents])
      DesktopApp(
        id: id,
        name: id == _application ? 'Isolated editor' : id,
        desktopFile: '$root/$id.desktop',
        iconName: 'utilities-terminal',
      ),
  ];
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
  Map<String, Object?>? lastMetrics;
  Object? previousGeometry;
  var stableFrames = 0;
  do {
    await tester.pump(const Duration(milliseconds: 20));
    state = await _state();
    final dock = state['dock'] as Map?;
    final geometry = state['monitorGeometry'] as Map;
    if (state['style'] != 'macos' ||
        state['expanded'] != expanded ||
        state['height'] != geometry['height'] ||
        state['width'] != geometry['width'] ||
        dock?['ready'] != true ||
        dock?['position'] != controller.macPreferences.dockPosition.name ||
        (expanded && state['focused'] != true)) {
      continue;
    }
    final blocked = dock!['inputWidth'] == 0 && dock['inputHeight'] == 0;
    if (blocked != expanded) continue;
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
    lastMetrics = {
      'paintedSize': paintedSize,
      'viewport': viewport,
      'nativeSize': nativeSize,
      'glass': _painted(tester, glass),
      'revision': controller.dockGeometryRevision,
      'stableFrames': stableFrames,
      'sizeDelta': [
        paintedSize.width - nativeSize.width,
        paintedSize.height - nativeSize.height,
        viewport.width - nativeSize.width,
        viewport.height - nativeSize.height,
      ],
    };
    if ((paintedSize.width - nativeSize.width).abs() > .01 ||
        (paintedSize.height - nativeSize.height).abs() > .01 ||
        (viewport.width - nativeSize.width).abs() > .01 ||
        (viewport.height - nativeSize.height).abs() > .01) {
      continue;
    }
    final painted = _painted(tester, glass);
    final horizontal =
        controller.macPreferences.dockPosition == MacDockPosition.bottom;
    lastMetrics['inputDelta'] = [
      painted.left - (dock['inputX'] as num),
      painted.width - (dock['inputWidth'] as num),
      painted.top - (dock['inputY'] as num),
      painted.height - (dock['inputHeight'] as num),
    ];
    final nativeStart = (dock[horizontal ? 'inputX' : 'inputY'] as num)
        .toDouble();
    final nativeEnd =
        nativeStart + (dock[horizontal ? 'inputWidth' : 'inputHeight'] as num);
    final paintedStart = horizontal ? painted.left : painted.top;
    final paintedEnd = horizontal ? painted.right : painted.bottom;
    // Each endpoint is enclosed independently in native integer coordinates.
    // The total width difference can approach two pixels for fractional edges.
    if (!expanded &&
        (nativeStart != paintedStart.floor() ||
            nativeEnd != paintedEnd.ceil())) {
      continue;
    }
    final signature = (
      nativeSize,
      paintedSize,
      viewport,
      painted,
      controller.dockGeometryRevision,
    );
    lastMetrics['signatureEqual'] = signature == previousGeometry;
    lastMetrics['animations'] = tester.hasRunningAnimations;
    stableFrames = signature == previousGeometry ? stableFrames + 1 : 0;
    previousGeometry = signature;
    if (stableFrames < 3) continue;
    expect(state['preview'], isFalse);
    expect(state['layer'], expanded ? 'Overlay' : 'Top');
    expect(state['keyboard'], expanded ? 'Exclusive' : 'None');
    expect(state['exclusiveZone'], -1);
    expect(dock['visible'], isTrue);
    expect(dock['layer'], 'Top');
    expect(dock['keyboard'], 'None');
    expect(dock['exclusiveZone'], -1);
    expect(state['mainViewId'], isNot(state['dockViewId']));
    return state;
  } while (DateTime.now().isBefore(deadline));
  fail('Native Dock/panel state did not settle: $state; metrics=$lastMetrics');
}

// Compare actual compositor pixels to opaque flat patches of the original
// extracted Downloads sprite. Native view coordinates alone cannot prove that
// GTK mapped the view at the right output-local position or rendered the icon.
Future<Map<String, Object>> _folderPixels(
  File capture,
  Size output,
  Rect expected,
) async {
  final screenshotCodec = await ui.instantiateImageCodec(
    await capture.readAsBytes(),
  );
  final screenshot = (await screenshotCodec.getNextFrame()).image;
  final bytes = await rootBundle.load('assets/macos/downloads.png');
  final sourceCodec = await ui.instantiateImageCodec(
    bytes.buffer.asUint8List(bytes.offsetInBytes, bytes.lengthInBytes),
  );
  final source = (await sourceCodec.getNextFrame()).image;
  try {
    final actual = (await screenshot.toByteData(
      format: ui.ImageByteFormat.rawRgba,
    ))!;
    final original = (await source.toByteData(
      format: ui.ImageByteFormat.rawRgba,
    ))!;
    final scaleX = screenshot.width / output.width;
    final scaleY = screenshot.height / output.height;
    final samples = <(double, double, int, int, int)>[];
    for (var row = 1; row < 20; row++) {
      for (var column = 1; column < 20; column++) {
        final u = column / 20;
        final v = row / 20;
        final x = (u * source.width).floor();
        final y = (v * source.height).floor();
        final offset = (y * source.width + x) * 4;
        if (original.getUint8(offset + 3) != 255) continue;
        final red = original.getUint8(offset);
        final green = original.getUint8(offset + 1);
        final blue = original.getUint8(offset + 2);
        var flat = true;
        for (final nearby in [
          offset - 8,
          offset + 8,
          offset - source.width * 8,
          offset + source.width * 8,
        ]) {
          if (original.getUint8(nearby + 3) != 255 ||
              (original.getUint8(nearby) - red).abs() > 16 ||
              (original.getUint8(nearby + 1) - green).abs() > 16 ||
              (original.getUint8(nearby + 2) - blue).abs() > 16) {
            flat = false;
          }
        }
        if (flat) samples.add((u, v, red, green, blue));
      }
    }
    expect(samples.length, greaterThan(40));
    var bestError = double.infinity;
    for (var dy = -2; dy <= 2; dy++) {
      for (var dx = -2; dx <= 2; dx++) {
        var error = 0;
        for (final sample in samples) {
          final x =
              ((expected.left + expected.width * sample.$1) * scaleX).floor() +
              dx;
          final y =
              ((expected.top + expected.height * sample.$2) * scaleY).floor() +
              dy;
          expect(x, inInclusiveRange(0, screenshot.width - 1));
          expect(y, inInclusiveRange(0, screenshot.height - 1));
          final offset = (y * screenshot.width + x) * 4;
          error += (actual.getUint8(offset) - sample.$3).abs();
          error += (actual.getUint8(offset + 1) - sample.$4).abs();
          error += (actual.getUint8(offset + 2) - sample.$5).abs();
        }
        bestError = math.min(bestError, error / (samples.length * 3));
      }
    }
    expect(
      bestError,
      lessThan(30),
      reason:
          'Original Downloads sprite must occupy its actual output-local rectangle',
    );
    return {
      'sampleCount': samples.length,
      'averageRgbError': bestError,
      'physicalScaleX': scaleX,
      'physicalScaleY': scaleY,
    };
  } finally {
    screenshot.dispose();
    screenshotCodec.dispose();
    source.dispose();
    sourceCodec.dispose();
  }
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
    expect(
      colors.length,
      greaterThan(monochromeIcon ? 2 : 20),
      reason:
          'Actual output must contain the panel/icon pixels rather than an empty layer',
    );
    expect(bright, greaterThan(30));
    expect(maximum - minimum, greaterThan(100));
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

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.onlyPumps;
  final filter = _ViewPacketFilter(binding);
  testWidgets(
    'native Dock details preserve separate views and literal Downloads actions',
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
      late Directory downloads;
      await tester.runAsync(() async {
        await captures.create(recursive: true);
        downloads = await Directory(
          '$root/fixture-downloads',
        ).create(recursive: true);
        await File(
          '${downloads.path}/Literal \$(); & # document.txt',
        ).writeAsString('Private fixture, never executed.\n');
        await File(
          '${downloads.path}/Second document.txt',
        ).writeAsString('Private fixture.\n');
        for (final id in [_application, ..._recents]) {
          await File('$root/$id.desktop').writeAsString(
            '[Desktop Entry]\nType=Application\nName=Isolated fixture\nExec=false\n',
          );
        }
      });
      final launches = <Map<String, Object>>[];
      final folder = DockFolderService(
        directory: downloads.path,
        environment: {'HOME': root, 'XDG_DATA_HOME': '$root/data'},
        applicationLauncher: ApplicationLauncher(
          scopeProbe: () async => null,
          command: (executable, arguments) async {
            expect(executable, 'gio');
            expect(arguments.take(2).toList(), ['open', '--']);
            expect(arguments.last.startsWith(downloads.path), isTrue);
            launches.add({
              'executable': executable,
              'arguments': List.of(arguments),
            });
            return '';
          },
        ),
      );
      final ipc = _Ipc();
      final controller = ShellController(
        ipc: ipc,
        desktopApps: _Apps(root),
        appMenuService: _Menus(),
        system: _System(),
        wallpaperService: _Wallpapers(),
        dockFolderService: folder,
        trashStateService: TrashStateService(
          environment: {'HOME': root, 'XDG_DATA_HOME': '$root/data'},
          command: (program, arguments) async {
            expect(program, 'gio');
            expect(arguments, ['list', 'trash:///']);
            return '';
          },
        ),
        powerProfiles: PowerProfilesService(demo: true),
        notifications: false,
        tray: false,
        powerProfilesEnabled: false,
        restoreWallpaperOnStart: false,
      );
      const mac = MacShellPreferences(
        downloadsDisplay: MacDownloadsDisplay.folder,
        magnification: false,
      );
      const prefs = ShellPreferences(
        shellStyle: ShellStyle.macos,
        reduceMotion: true,
        macShell: mac,
        pinnedAppIds: [_application],
        recentAppIds: _recents,
      );
      controller.setShellStyle(ShellStyle.macos);
      controller.setReduceMotion(true);
      controller.setPinnedAppIds(prefs.pinnedAppIds);
      controller.setRecentAppIds(prefs.recentAppIds);
      controller.setMacPreferences(mac);
      final phases = <Map<String, Object?>>[];
      final evidence = <String, Object?>{
        'passed': false,
        'phases': phases,
        'scope':
            'Actual private GTK Wayland main/Dock views and grim pixels; bounded real private-folder enumeration/watch; app catalogue/window/menu/system services are fakes. GIO argv are recorded, never launched.',
        'inputScope':
            'Injected Flutter mouse events and actual native input regions, not physical compositor delivery certification.',
      };
      int? mainId, dockId;
      Element? mainElement, dockElement;
      final pointers = <(TestPointer, ui.FlutterView)>[];

      Future<void> tap(Finder finder, {bool secondary = false}) async {
        final view = View.of(tester.element(finder));
        final pointer = TestPointer(
          60 + pointers.length,
          PointerDeviceKind.mouse,
        );
        final pair = (pointer, view);
        pointers.add(pair);
        final point = tester.getCenter(finder);
        await tester.sendEventToBinding(
          pointer.addPointer(location: point, view: view),
        );
        await tester.sendEventToBinding(
          pointer.down(
            point,
            buttons: secondary ? kSecondaryMouseButton : kPrimaryMouseButton,
            view: view,
          ),
        );
        await tester.sendEventToBinding(pointer.up(view: view));
        await tester.sendEventToBinding(pointer.removePointer(view: view));
        pointers.remove(pair);
        await tester.pump();
      }

      Future<void> record(
        String name, {
        bool expanded = false,
        bool originalFolder = false,
        bool appMenu = false,
      }) async {
        var state = await _native(tester, controller, expanded: expanded);
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
        expect(
          tester.widget<ShellApplication>(main).controller,
          same(controller),
        );
        expect(
          tester.widget<DockApplication>(dock).controller,
          same(controller),
        );
        await tester.runAsync(() async {
          final errors = <Object>[];
          await precacheImage(
            const AssetImage('assets/macos/downloads.png'),
            tester.element(dock),
            onError: (e, s) => errors.add(e),
          );
          expect(errors, isEmpty);
        });
        await tester.pump(const Duration(milliseconds: 250));
        state = await _native(tester, controller, expanded: expanded);
        final dockOrigin = await _origin('dock'),
            mainOrigin = await _origin('main');
        final geometry = state['monitorGeometry'] as Map;
        final output = Size(
          (geometry['width'] as num).toDouble(),
          (geometry['height'] as num).toDouble(),
        );
        final dockState = state['dock'] as Map;
        expect(
          dockOrigin['outputX'],
          switch (controller.macPreferences.dockPosition) {
            MacDockPosition.left => 0,
            MacDockPosition.right => output.width - (dockState['width'] as num),
            MacDockPosition.bottom =>
              (output.width - (dockState['width'] as num)) ~/ 2,
          },
        );
        expect(
          dockOrigin['outputY'],
          controller.macPreferences.dockPosition == MacDockPosition.bottom
              ? output.height - (dockState['height'] as num)
              : (output.height - (dockState['height'] as num)) ~/ 2,
        );
        expect(mainOrigin['outputX'], 0);
        expect(mainOrigin['outputY'], 0);
        final target = expanded
            ? _painted(
                tester,
                find.byKey(
                  ValueKey(
                    appMenu
                        ? 'mac-panel-position'
                        : 'mac-downloads-stack-panel',
                  ),
                ),
              )
            : _painted(
                tester,
                find.byKey(
                  ValueKey(
                    originalFolder
                        ? 'downloads-folder-icon'
                        : 'downloads-stack-icon',
                  ),
                ),
              ).shift(
                Offset(
                  (dockOrigin['outputX'] as num).toDouble(),
                  (dockOrigin['outputY'] as num).toDouble(),
                ),
              );
        expect(
          (Offset.zero & output).inflate(.1).contains(target.topLeft),
          isTrue,
        );
        expect(
          (Offset.zero & output).inflate(.1).contains(target.bottomRight),
          isTrue,
        );
        final capture = File('${captures.path}/$name.png');
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
        });
        final pixels = await tester.runAsync(
          () => originalFolder && !expanded
              ? _folderPixels(capture, output, target)
              : _visiblePixels(
                  capture,
                  output,
                  target,
                  monochromeIcon: !expanded,
                ),
        );
        expect(
          tester.takeException(),
          isNull,
          reason: '$name has no rendering/layout exception',
        );
        phases.add({
          'name': name,
          'native': state,
          'origins': {'main': mainOrigin, 'dock': dockOrigin},
          'outputLocalPixels': _rect(target),
          'pixelEvidence': pixels,
          'capture': capture.path,
          'preferences': controller.macPreferences.toJson(),
        });
      }

      Future<void> open() async {
        await tap(find.byKey(const ValueKey('dock-downloads')));
        await _native(tester, controller, expanded: true);
        expect(controller.panel, ShellPanel.dockDownloads);
        expect(controller.dockMenuAnchor, isNotNull);
      }

      try {
        await tester.pumpWidget(
          ShellHost(controller: controller, preferences: prefs, persist: true),
          wrapWithView: false,
        );
        await controller.start();
        await tester.runAsync(folder.start);
        await _until(
          tester,
          () =>
              controller.connected &&
              controller.apps.length == 3 &&
              folder.entries.length == 2,
          'isolated catalogue and real private Downloads',
        );
        // Reapply deterministic recent order after the fake focused-window event.
        controller.setRecentAppIds(_recents);
        await record('01-original-downloads-folder', originalFolder: true);
        // Exercise the real GTK Dock view while animation remains enabled.
        // Native hit regions must track the painted icon in all orientations.
        final motion = <Map<String, Object?>>[];
        for (final position in MacDockPosition.values) {
          controller.setReduceMotion(false);
          controller.setMacPreferences(
            mac.copyWith(
              dockPosition: position,
              magnification: true,
              magnificationScale: 1.8,
            ),
          );
          await _native(tester, controller, expanded: false);
          final icon = find.byKey(const ValueKey('downloads-folder-icon'));
          final scaleFinder = find.byKey(const ValueKey('dock-scale-下载'));
          final view = View.of(tester.element(icon));
          final pointer = TestPointer(
            300 + position.index,
            PointerDeviceKind.mouse,
          );
          final pair = (pointer, view);
          pointers.add(pair);
          await tester.sendEventToBinding(
            pointer.addPointer(location: const Offset(1, 1), view: view),
          );
          final start = tester.getCenter(icon);
          // Multiple packets in one display frame end at the final pointer.
          for (var packet = 0; packet < 40; packet++) {
            await tester.sendEventToBinding(
              pointer.hover(
                start + Offset(packet.isEven ? .5 : 0, 0),
                view: view,
              ),
            );
          }
          final scales = <double>[];
          for (var frame = 0; frame < 16; frame++) {
            await tester.pump(const Duration(milliseconds: 16));
            final scale = tester
                .widget<Transform>(scaleFinder)
                .transform
                .entry(0, 0);
            expect(scale.isFinite, isTrue);
            expect(scale, inInclusiveRange(1.0, 1.81));
            scales.add(scale);
          }
          expect(
            scales.last,
            greaterThan(1.3),
            reason: 'native Dock must visibly magnify',
          );
          await record(
            'motion-${position.name}-magnified',
            originalFolder: true,
          );
          final native = (await _state())['dock'] as Map;
          final center = tester.getCenter(icon);
          final regions = (native['inputRegions'] as List).cast<Map>();
          expect(
            regions.any(
              (r) => Rect.fromLTWH(
                (r['x'] as num).toDouble(),
                (r['y'] as num).toDouble(),
                (r['width'] as num).toDouble(),
                (r['height'] as num).toDouble(),
              ).contains(center),
            ),
            isTrue,
            reason: 'painted magnified icon must remain clickable',
          );
          await tester.sendEventToBinding(
            pointer.hover(const Offset(1, 1), view: view),
          );
          await tester.sendEventToBinding(pointer.removePointer(view: view));
          pointers.remove(pair);
          await tester.pump(const Duration(milliseconds: 800));
          await _native(tester, controller, expanded: false);
          final reset = tester
              .widget<Transform>(scaleFinder)
              .transform
              .entry(0, 0);
          expect(reset, closeTo(1, .001));
          motion.add({
            'position': position.name,
            'scales': scales,
            'reset': reset,
            'nativeInputRegions': List.of(regions),
          });
        }
        evidence['motion'] = motion;
        controller.setReduceMotion(true);
        controller.setMacPreferences(mac);
        await _native(tester, controller, expanded: false);

        final actualOutputs = await tester.runAsync(() async {
          final result = await Process.run(
            Platform.environment['CLARION_SHELL_CLARION_BIN']!,
            ['msg', '--json', 'outputs'],
          ).timeout(const Duration(seconds: 5));
          expect(result.exitCode, 0);
          final json = jsonDecode(result.stdout as String) as Map;
          return json.containsKey('Outputs') ? json['Outputs'] as Map : json;
        });
        final geometry = (await _state())['monitorGeometry'] as Map;
        evidence['actualPrivateOutputs'] = actualOutputs;
        evidence['actualPrivateGeometry'] = geometry;
        final matches = actualOutputs!.values.where((raw) {
          final logical = (raw as Map)['logical'];
          return logical is Map &&
              [
                'x',
                'y',
                'width',
                'height',
              ].every((key) => logical[key] == geometry[key]);
        }).toList();
        expect(
          matches,
          hasLength(1),
          reason: 'native=$geometry outputs=$actualOutputs',
        );
        final recentDivider = _painted(
          tester,
          find.byKey(const ValueKey('dock-divider-recent-divider')),
        );
        final utilitiesDivider = _painted(
          tester,
          find.byKey(const ValueKey('dock-divider-utilities-divider')),
        );
        expect(
          _painted(tester, find.byKey(const ValueKey('dock-overview'))).right,
          lessThan(recentDivider.left),
        );
        expect(
          _painted(
            tester,
            find.byKey(ValueKey('dock-app-${_recents.last}')),
          ).right,
          lessThan(utilitiesDivider.left),
        );
        await tap(
          find.byKey(const ValueKey('dock-app-$_application')),
          secondary: true,
        );
        await _native(tester, controller, expanded: true);
        expect(controller.panel, ShellPanel.dockAppMenu);
        final contextMenu = _painted(
          tester,
          find.byKey(const ValueKey('mac-panel-position')),
        );
        expect(contextMenu.width, lessThanOrEqualTo(280.1));
        expect(
          tester.getSize(find.byKey(const ValueKey('dock-menu-open'))).height,
          26,
        );
        evidence['compactAppMenu'] = _rect(contextMenu);
        await record(
          '01b-compact-application-context-menu',
          expanded: true,
          appMenu: true,
        );
        controller.closePanel();
        await _native(tester, controller, expanded: false);

        controller.setMacPreferences(
          controller.macPreferences.copyWith(
            downloadsDisplay: MacDownloadsDisplay.stack,
          ),
        );
        await record('02-real-file-stack');
        await open();
        final panelBounds = _painted(
          tester,
          find.byKey(const ValueKey('mac-panel-position')),
        );
        final expectedAutomatic = resolveDownloadsView(
          requested: MacDownloadsView.automatic,
          dockPosition: MacDockPosition.bottom,
          available: panelBounds.size,
          entryCount: 2,
        );
        expect(
          find.byKey(ValueKey('downloads-${expectedAutomatic.name}')),
          findsOneWidget,
        );
        evidence['automaticResolvedView'] = expectedAutomatic.name;
        await record('03-bottom-automatic-contents', expanded: true);

        controller.setMacPreferences(
          controller.macPreferences.copyWith(
            downloadsView: MacDownloadsView.grid,
          ),
        );
        await tester.pump();
        expect(find.byKey(const ValueKey('downloads-grid')), findsOneWidget);
        await record('04-bottom-grid', expanded: true);
        await tap(find.byKey(const ValueKey('downloads-stack-options')));
        await tester.pump(const Duration(milliseconds: 250));
        final views = tester
            .widgetList<CheckedPopupMenuItem<Object>>(
              find.byType(CheckedPopupMenuItem<Object>),
            )
            .where((item) => item.value is MacDownloadsView)
            .map((item) => item.value)
            .toList();
        expect(views, [
          MacDownloadsView.fan,
          MacDownloadsView.grid,
          MacDownloadsView.list,
          MacDownloadsView.automatic,
        ]);
        expect(
          tester
              .widget<CheckedPopupMenuItem<Object>>(
                find.byKey(const ValueKey('downloads-sort-dateCreated')),
              )
              .enabled,
          isFalse,
        );
        expect(
          tester
              .widget<CheckedPopupMenuItem<Object>>(
                find.byKey(const ValueKey('downloads-sort-dateAdded')),
              )
              .enabled,
          isFalse,
        );
        await tester.ensureVisible(find.text('列表'));
        await tap(find.text('列表'));
        await tester.pump(const Duration(milliseconds: 250));
        expect(controller.macPreferences.downloadsView, MacDownloadsView.list);
        await record('05-bottom-list', expanded: true);

        await tester.runAsync(() async {
          await File(
            '${downloads.path}/Added by private watcher.txt',
          ).writeAsString('Private directory change only.\n');
          final deadline = DateTime.now().add(const Duration(seconds: 5));
          while (folder.entries.length != 3 &&
              DateTime.now().isBefore(deadline)) {
            await Future<void>.delayed(const Duration(milliseconds: 30));
          }
        });
        expect(folder.entries.length, 3);
        await tester.pump();
        expect(find.text('Added by private watcher.txt'), findsOneWidget);
        final literal = folder.entries.singleWhere(
          (item) => item.name.startsWith('Literal'),
        );
        await tap(find.byKey(ValueKey('downloads-entry-${literal.id}')));
        await _until(
          tester,
          () => controller.panel == ShellPanel.none,
          'successful fake GIO completion closes original panel',
        );
        expect(launches, [
          {
            'executable': 'gio',
            'arguments': ['open', '--', literal.path],
          },
        ]);
        evidence['literalGioLaunches'] = List.of(launches);
        await record('06-open-completed-native-input-restored');

        // A real Downloads folder often contains more than eight files. An
        // explicit Fan selection must remain a fan, with a bounded More tile.
        await tester.runAsync(() async {
          for (var index = 0; index < 12; index++) {
            await File(
              '${downloads.path}/Fan fixture $index.txt',
            ).writeAsString('Private fan overflow fixture.\n');
          }
          await folder.refresh();
        });
        expect(folder.entries.length, 15);
        controller.setMacPreferences(
          controller.macPreferences.copyWith(
            downloadsView: MacDownloadsView.fan,
          ),
        );
        await open();
        expect(find.byKey(const ValueKey('downloads-fan')), findsOneWidget);
        expect(find.byKey(const ValueKey('downloads-grid')), findsNothing);
        final renderedEntries = find.byWidgetPredicate(
          (widget) =>
              widget.key is ValueKey<String> &&
              (widget.key as ValueKey<String>).value.startsWith(
                'downloads-entry-',
              ),
        );
        final renderedCount = renderedEntries.evaluate().length;
        expect(renderedCount, greaterThan(0));
        expect(renderedCount, lessThan(folder.entries.length));
        evidence['explicitFanTotalEntries'] = folder.entries.length;
        evidence['explicitFanVisibleEntries'] = renderedCount;
        await record('06b-explicit-fan-many-files', expanded: true);
        Map<String, Object?> focusState() {
          final focus = FocusManager.instance.primaryFocus;
          final context = focus?.context;
          return {
            'label': focus?.debugLabel,
            'viewId': context == null ? null : View.of(context).viewId,
            'scopeChildLabel': focus?.enclosingScope?.focusedChild?.debugLabel,
          };
        }

        evidence['fanFocusBeforeKeys'] = focusState();
        await tester.sendKeyEvent(LogicalKeyboardKey.end);
        evidence['fanFocusAfterEnd'] = focusState();
        await tester.sendKeyEvent(LogicalKeyboardKey.enter);
        await _until(
          tester,
          () => controller.panel == ShellPanel.none,
          'fan More target opens the folder, never an invisible file',
        );
        expect(launches.last, {
          'executable': 'gio',
          'arguments': ['open', '--', downloads.path],
        });
        evidence['fanMoreGioLaunch'] = launches.last;
        await record('06c-fan-more-native-input-restored');

        for (final position in [MacDockPosition.left, MacDockPosition.right]) {
          controller.setMacPreferences(
            controller.macPreferences.copyWith(
              dockPosition: position,
              downloadsView: MacDownloadsView.fan,
            ),
          );
          await _native(tester, controller, expanded: false);
          await open();
          expect(find.byKey(const ValueKey('downloads-grid')), findsOneWidget);
          expect(find.byKey(const ValueKey('downloads-fan')), findsNothing);
          await record(
            '07-${position.name}-fan-safely-resolves-grid',
            expanded: true,
          );
          controller.closePanel();
          await _native(tester, controller, expanded: false);
        }
        controller.setMacPreferences(
          controller.macPreferences.copyWith(
            dockPosition: MacDockPosition.bottom,
            downloadsDisplay: MacDownloadsDisplay.folder,
            showRecentApplications: false,
          ),
        );
        await tester.pump();
        expect(
          find.byKey(const ValueKey('dock-divider-recent-divider')),
          findsNothing,
        );
        expect(controller.recentAppIds, _recents);
        await record(
          '08-recent-section-hidden-folder-restored',
          originalFolder: true,
        );
        evidence['mainViewId'] = mainId;
        evidence['dockViewId'] = dockId;
        evidence['passed'] = true;
        binding.reportData = evidence;
      } finally {
        evidence['recordedFakeCompositorActions'] = ipc.actions;
        evidence['literalGioLaunches'] = launches;
        for (final (pointer, view) in pointers) {
          await tester.sendEventToBinding(pointer.removePointer(view: view));
        }
        await tester.pumpWidget(
          const ViewCollection(views: []),
          wrapWithView: false,
        );
        await _window.invokeMethod<void>('setExpanded', false);
        await _window.invokeMethod<void>('setShellStyle', 'material');
        evidence['ipcDisposalsAfterUnmount'] = ipc.disposals;
        evidence['ignoredUnattachedDeviceEvents'] = filter.ignored;
        await tester.runAsync(
          () => File('${captures.path}/mac-dock-details-native-evidence.json')
              .writeAsString(
                '${const JsonEncoder.withIndent('  ').convert(evidence)}\n',
              ),
        );
        expect(ipc.disposals, 1);
      }
    },
    timeout: const Timeout(Duration(minutes: 5)),
  );
  tearDownAll(() {
    binding.deviceEventDispatcher = null;
    filter.restore();
  });
}
