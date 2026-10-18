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
import 'package:clarion_shell/services/global_app_menu_service.dart';
import 'package:clarion_shell/services/power_profiles_service.dart';
import 'package:clarion_shell/services/clarion_ipc.dart';
import 'package:clarion_shell/services/system_service.dart';
import 'package:clarion_shell/services/wallpaper_service.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/ui/launcher_surface.dart';
import 'package:clarion_shell/ui/mac_dock_view.dart';
import 'package:clarion_shell/ui/shell_geometry.dart';

import 'helpers/launcher_glass_checks.dart';

const _window = MethodChannel('clarion.shell/window');
const _appId = 'mac-configuration-fixture';
const _windowId = 91;

// flutter_test's live hardware event transformer assumes each native view has
// a mounted RenderView. Production hitTestInView uses a nullable lookup. Filter
// only packets for an unattached view during creation/teardown; injected test
// events bypass this callback. Report the filter instead of calling it proof of
// physical mouse or keyboard delivery through compositor hit testing.
class _DeviceFilter {
  _DeviceFilter(this.binding) {
    original = binding.platformDispatcher.onPointerDataPacket;
    binding.platformDispatcher.onPointerDataPacket = handle;
  }
  final IntegrationTestWidgetsFlutterBinding binding;
  ui.PointerDataPacketCallback? original;
  final ignored = <Map<String, Object?>>[];
  int deliveredDeviceEvents = 0;
  void handle(ui.PointerDataPacket packet) {
    final attached = binding.renderViews
        .map((view) => view.flutterView.viewId)
        .toSet();
    final valid = <ui.PointerData>[];
    for (final data in packet.data) {
      if (attached.contains(data.viewId)) {
        deliveredDeviceEvents++;
        valid.add(data);
      } else {
        ignored.add({
          'viewId': data.viewId,
          'change': data.change.name,
          'x': data.physicalX,
          'y': data.physicalY,
        });
      }
    }
    if (valid.isNotEmpty) original?.call(ui.PointerDataPacket(data: valid));
  }

  void restore() => binding.platformDispatcher.onPointerDataPacket = original;
}

class _Apps extends DesktopApps {
  int loads = 0;
  @override
  Future<List<DesktopApp>> load() async {
    loads++;
    return const [
      DesktopApp(
        id: _appId,
        name: 'Configuration App',
        desktopFile: '',
        iconName: 'utilities-terminal',
      ),
    ];
  }
}

class _Ipc extends ClarionIpc {
  final connection = StreamController<bool>.broadcast(sync: true);
  final event = StreamController<Map<String, dynamic>>.broadcast(sync: true);
  int starts = 0;
  int disposals = 0;
  final actions = <Map<String, Object?>>[];
  final targets = <int, Rect>{};
  int outputReads = 0;
  @override
  Stream<bool> get connections => connection.stream;
  @override
  Stream<Map<String, dynamic>> get events => event.stream;
  @override
  void start() {
    starts++;
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
            'id': _windowId,
            'app_id': _appId,
            'title': 'Isolated configuration document',
            'workspace_id': 1,
            'is_focused': true,
            'is_minimized': false,
            'app_menu': {
              'bus_name': 'org.test.ConfigurationApp',
              'menu_path': '/Menu',
            },
          },
        ],
      },
    });
  }

  @override
  Future<dynamic> request(Object request) async {
    if (request != 'Outputs') {
      throw StateError('Unauthorized fixture IPC request: $request');
    }
    outputReads++;
    final native = await _state();
    return {
      'Outputs': {
        'winit': {'name': 'winit', 'logical': native['monitorGeometry']},
      },
    };
  }

  @override
  Future<void> action(
    String name, [
    Map<String, Object?> arguments = const {},
  ]) async {
    if (name != 'SetMinimizeTarget') {
      throw StateError('Unauthorized fixture compositor action: $name');
    }
    actions.add(Map.of(arguments));
    final id = arguments['id'] as int;
    final value = arguments['target'];
    if (value == null) {
      targets.remove(id);
      return;
    }
    final target = value as Map;
    if (target['output'] != 'winit') {
      throw StateError('Fixture target escaped the isolated winit output');
    }
    targets[id] = Rect.fromLTWH(
      (target['x'] as num).toDouble(),
      (target['y'] as num).toDouble(),
      (target['width'] as num).toDouble(),
      (target['height'] as num).toDouble(),
    );
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
  final action = GlobalMenuItem(
    id: 'configuration-action',
    label: 'Configuration action',
  );
  late final items = [
    GlobalMenuItem(id: 'file', label: 'File', children: [action]),
  ];
  int activations = 0;
  @override
  List<GlobalMenuItem> get menus => items;
  @override
  bool get available => true;
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
  @override
  Future<void> submenu(GlobalMenuItem item, {required bool opened}) async {}
  @override
  Future<bool> activate(GlobalMenuItem item) async {
    if (!identical(item, action)) {
      throw StateError('Unknown fixture menu action');
    }
    activations++;
    return true;
  }
}

class _System extends SystemService {
  _System()
    : super(
        command: (program, arguments) async {
          throw StateError('Fixture forbids system writes: $program');
        },
      );
  @override
  Future<SystemSnapshot> read() async => const SystemSnapshot(
    volume: .5,
    brightness: .5,
    batteryFraction: .8,
    wifiEnabled: true,
    bluetoothEnabled: true,
  );
  @override
  Future<void> openHomeFolder() async {}
  @override
  Future<void> openDownloadsFolder() async {}
  @override
  Future<void> openTrash() async {}
}

class _Wallpapers extends WallpaperService {
  @override
  Future<void> restore({bool applySaved = false}) async {}
  @override
  Future<List<ShellWallpaper>> scan() async => [];
}

// Services/snapshots are isolated, but native GTK geometry and the controller's
// SetMinimizeTarget serialization are production paths. Fake Outputs mirror the
// native monitor geometry so no real compositor window action is issued.
class _Controller extends ShellController {
  _Controller(this.fixtureIpc, _Apps apps, _Menus menus)
    : super(
        ipc: fixtureIpc,
        desktopApps: apps,
        appMenuService: menus,
        system: _System(),
        wallpaperService: _Wallpapers(),
        powerProfiles: PowerProfilesService(demo: true),
        notifications: false,
        tray: false,
        powerProfilesEnabled: false,
        restoreWallpaperOnStart: false,
      );
  final _Ipc fixtureIpc;
  Map<int, Rect> get targets => fixtureIpc.targets;
}

Future<Map<String, dynamic>> _state() async => Map<String, dynamic>.from(
  (await _window.invokeMapMethod<String, dynamic>('windowState'))!,
);

Future<Map<String, dynamic>> _origin(String role) async =>
    Map<String, dynamic>.from(
      (await _window.invokeMapMethod<String, dynamic>('surfaceOrigin', role))!,
    );

Rect _input(Map dock) => Rect.fromLTWH(
  (dock['inputX'] as num).toDouble(),
  (dock['inputY'] as num).toDouble(),
  (dock['inputWidth'] as num).toDouble(),
  (dock['inputHeight'] as num).toDouble(),
);

Rect _paintedRect(WidgetTester tester, Finder finder) =>
    Rect.fromPoints(tester.getTopLeft(finder), tester.getBottomRight(finder));

Map<String, double> _rect(Rect rect) => {
  'x': rect.left,
  'y': rect.top,
  'width': rect.width,
  'height': rect.height,
};

Future<Map<String, dynamic>> _actualOutputOwnership(
  WidgetTester tester,
  Map<String, dynamic> state, {
  Map<String, dynamic>? outputSnapshot,
}) async {
  final outputs =
      outputSnapshot ??
      await tester.runAsync(() async {
        final result = await Process.run(
          Platform.environment['CLARION_SHELL_CLARION_BIN']!,
          ['msg', '--json', 'outputs'],
        ).timeout(const Duration(seconds: 5));
        expect(
          result.exitCode,
          0,
          reason: '${result.stdout}\n${result.stderr}',
        );
        final decoded = jsonDecode(result.stdout as String) as Map;
        final values = decoded.containsKey('Outputs')
            ? decoded['Outputs'] as Map
            : decoded;
        return Map<String, dynamic>.from(values);
      });
  final geometry = state['monitorGeometry'] as Map;
  final matches = <Map<String, dynamic>>[];
  for (final entry in outputs!.entries) {
    final output = entry.value as Map;
    final logical = output['logical'];
    if (logical is Map &&
        [
          'x',
          'y',
          'width',
          'height',
        ].every((key) => logical[key] == geometry[key])) {
      matches.add({'mapKey': entry.key, ...Map<String, dynamic>.from(output)});
    }
  }
  expect(
    matches,
    hasLength(1),
    reason:
        'Actual private compositor Outputs must uniquely match native GDK '
        'monitorGeometry exactly: native=$geometry, outputs=$outputs',
  );
  expect(matches.single['name'], 'winit');
  return {
    'nativeMonitorGeometry': Map<String, dynamic>.from(geometry),
    'actualOutputs': outputs,
    'uniqueMatchedOutput': matches.single,
    'scope': outputSnapshot == null
        ? 'Fresh read-only real private compositor Outputs; no window actions.'
        : 'Initial real private compositor Outputs snapshot compared exactly '
              'to this phase. This matrix never changes outputs; no window actions.',
  };
}

Future<void> _diagnosticLine(String name, Map<String, dynamic> value) async {
  final root = Platform.environment['CLARION_SHELL_TEST_ROOT']!;
  await File(
    '$root/$name.jsonl',
  ).writeAsString('${jsonEncode(value)}\n', mode: FileMode.append, flush: true);
}

Future<Map<String, dynamic>> _ownedProcessState(int pid) async {
  final state = <String, dynamic>{'pid': pid};
  for (final name in ['stat', 'wchan', 'status']) {
    try {
      final text = await File('/proc/$pid/$name').readAsString();
      state[name] = name == 'status'
          ? text
                .split('\n')
                .where(
                  (line) => RegExp(
                    r'^(Name|State|Pid|PPid|Threads|VmRSS|voluntary_ctxt_switches|nonvoluntary_ctxt_switches):',
                  ).hasMatch(line),
                )
                .join('\n')
          : text.trim();
    } on FileSystemException catch (error) {
      state[name] = error.message;
    }
  }
  final descriptors = <String, dynamic>{};
  try {
    await for (final descriptor in Directory('/proc/$pid/fd').list()) {
      try {
        descriptors[descriptor.path.split('/').last] = await Link(
          descriptor.path,
        ).target();
      } on FileSystemException catch (error) {
        descriptors[descriptor.path.split('/').last] = error.message;
      }
    }
  } on FileSystemException catch (error) {
    state['fdError'] = error.message;
  }
  state['fds'] = descriptors;
  return state;
}

Future<Map<String, dynamic>> _privateIpcProbe(String request) async {
  final path = Platform.environment['CLARION_SOCKET']!;
  final runtime = Platform.environment['XDG_RUNTIME_DIR']!;
  expect(Platform.environment['CLARION_SHELL_ISOLATED_TEST'], '1');
  expect(path.startsWith('$runtime/clarion.'), isTrue);
  expect(path, isNot(Platform.environment['CLARION_SHELL_PARENT_SOCKET']));
  final elapsed = Stopwatch()..start();
  Socket? socket;
  var closed = false;
  try {
    final work =
        Socket.connect(
          InternetAddress(path, type: InternetAddressType.unix),
          0,
        ).then((connected) async {
          socket = connected;
          if (closed) {
            connected.destroy();
            return <String, dynamic>{'lateConnectionDestroyed': true};
          }
          connected.write('${jsonEncode(request)}\n');
          await connected.flush();
          final reply = await connected
              .cast<List<int>>()
              .transform(utf8.decoder)
              .transform(const LineSplitter())
              .first;
          return <String, dynamic>{'reply': reply};
        });
    return {
      'request': request,
      'socket': path,
      ...await work.timeout(const Duration(milliseconds: 750)),
      'elapsedMicros': elapsed.elapsedMicroseconds,
    };
  } catch (error) {
    return {
      'request': request,
      'socket': path,
      'error': '$error',
      'elapsedMicros': elapsed.elapsedMicroseconds,
    };
  } finally {
    closed = true;
    socket?.destroy();
  }
}

Future<Map<String, dynamic>> _layerCommand(Map<String, dynamic> context) async {
  final elapsed = Stopwatch()..start();
  final stdoutBytes = <int>[];
  final stderrBytes = <int>[];
  Process? child;
  Future<int>? exit;
  int? exitCode;
  var abandoned = false;
  final command = [
    Platform.environment['CLARION_SHELL_CLARION_BIN']!,
    'msg',
    '--json',
    'layers',
  ];
  final diagnostic = <String, dynamic>{
    'context': context,
    'command': command,
    'socket': Platform.environment['CLARION_SOCKET'],
    'startedAtUtc': DateTime.now().toUtc().toIso8601String(),
    'deadlineMillis': 5000,
  };
  final work = Process.start(command.first, command.skip(1).toList()).then((
    p,
  ) async {
    child = p;
    diagnostic['pid'] = p.pid;
    diagnostic['startedMicros'] = elapsed.elapsedMicroseconds;
    final stdout = p.stdout.forEach(stdoutBytes.addAll);
    final stderr = p.stderr.forEach(stderrBytes.addAll);
    unawaited(p.stdin.close().then<void>((_) {}, onError: (Object _) {}));
    exit = p.exitCode.then((value) {
      exitCode = value;
      return value;
    });
    if (abandoned) {
      final killed = p.kill(ProcessSignal.sigkill);
      await _diagnosticLine('material-layer-ipc-late-child', {
        'context': context,
        'pid': p.pid,
        'startedMicros': elapsed.elapsedMicroseconds,
        'killSent': killed,
      });
    }
    await Future.wait<dynamic>([exit!, stdout, stderr]);
    return exitCode!;
  });
  try {
    diagnostic['exitCode'] = await work.timeout(const Duration(seconds: 5));
    diagnostic['completedMicros'] = elapsed.elapsedMicroseconds;
  } catch (error) {
    abandoned = true;
    diagnostic['error'] = '$error';
    diagnostic['failedMicros'] = elapsed.elapsedMicroseconds;
    diagnostic['stdoutBytesAtFailure'] = stdoutBytes.length;
    diagnostic['stderrBytesAtFailure'] = stderrBytes.length;
    diagnostic['stdoutAtFailure'] = utf8.decode(
      stdoutBytes,
      allowMalformed: true,
    );
    diagnostic['stderrAtFailure'] = utf8.decode(
      stderrBytes,
      allowMalformed: true,
    );
    final owned = child;
    if (owned != null) {
      diagnostic['childAtFailure'] = await _ownedProcessState(owned.pid);
      diagnostic['termSent'] = owned.kill(ProcessSignal.sigterm);
      try {
        await exit!.timeout(const Duration(milliseconds: 500));
      } on TimeoutException {
        diagnostic['killSent'] = owned.kill(ProcessSignal.sigkill);
        try {
          await exit!.timeout(const Duration(milliseconds: 500));
        } on TimeoutException {
          diagnostic['childAfterKill'] = await _ownedProcessState(owned.pid);
        }
      }
      diagnostic['exitCodeAfterCleanup'] = exitCode;
    }
    diagnostic['privateIpcProbes'] = [
      await _privateIpcProbe('Version'),
      await _privateIpcProbe('Layers'),
    ];
  }
  diagnostic['stdoutBytes'] = stdoutBytes.length;
  diagnostic['stderrBytes'] = stderrBytes.length;
  diagnostic['stdout'] = utf8.decode(stdoutBytes, allowMalformed: true);
  diagnostic['stderr'] = utf8.decode(stderrBytes, allowMalformed: true);
  diagnostic['cleanupCompletedMicros'] = elapsed.elapsedMicroseconds;
  await _diagnosticLine('material-layer-ipc', diagnostic);
  return diagnostic;
}

Future<List<Map<String, dynamic>>> _layers(
  WidgetTester tester,
  Map<String, dynamic> state, {
  Map<String, dynamic> context = const {},
}) async {
  final deadline = DateTime.now().add(const Duration(seconds: 10));
  List<Map<String, dynamic>> layers = [];
  do {
    final value = await tester.runAsync(
      () => _layerCommand({
        ...context,
        'style': state['style'],
        'expanded': state['expanded'],
      }),
    );
    if (value == null) fail('Layers diagnostic callback failed unexpectedly');
    expect(value['error'], isNull, reason: jsonEncode(value));
    expect(
      value['exitCode'],
      0,
      reason: '${value['stdout']}\n${value['stderr']}',
    );
    final decoded = jsonDecode(value['stdout'] as String);
    final items = decoded is List
        ? decoded
        : (decoded as Map)['Layers'] as List;
    layers = [for (final item in items) Map<String, dynamic>.from(item as Map)];
    final main = layers
        .where((surface) => surface['namespace'] == 'clarion-shell')
        .toList();
    final dock = layers
        .where((surface) => surface['namespace'] == 'clarion-shell-dock')
        .toList();
    final menuReservation = layers
        .where(
          (surface) => surface['namespace'] == 'clarion-shell-menu-reservation',
        )
        .toList();
    final dockReservation = layers
        .where(
          (surface) => surface['namespace'] == 'clarion-shell-dock-reservation',
        )
        .toList();
    final mac = state['style'] == 'macos';
    final expectedMenuReservation = mac && (state['reservationZone'] as int) > 0
        ? 1
        : 0;
    final expectedDockReservation =
        mac && ((state['dock'] as Map)['reservationZone'] as int) > 0 ? 1 : 0;
    if (main.length == 1 &&
        main.single['layer'] == state['layer'] &&
        main.single['keyboard_interactivity'] == state['keyboard'] &&
        (mac ? dock.length == 1 : dock.isEmpty) &&
        menuReservation.length == expectedMenuReservation &&
        dockReservation.length == expectedDockReservation) {
      expect(main.single['output'], 'winit');
      for (final surface in [...dock, ...menuReservation, ...dockReservation]) {
        expect(surface['output'], 'winit');
        expect(
          surface['layer'],
          surface['namespace'] == 'clarion-shell-dock'
              ? (state['dock'] as Map)['layer']
              : 'Top',
        );
        expect(surface['keyboard_interactivity'], 'None');
      }
      return layers;
    }
    await tester.pump(const Duration(milliseconds: 20));
  } while (DateTime.now().isBefore(deadline));
  fail(
    'Actual compositor layer map did not match native configuration: $layers',
  );
}

Future<Map<String, dynamic>> _spritePixels(
  File capture,
  Size output,
  Map<String, Rect> sprites,
) async {
  final codec = await ui.instantiateImageCodec(await capture.readAsBytes());
  final image = (await codec.getNextFrame()).image;
  try {
    final pixels = (await image.toByteData(
      format: ui.ImageByteFormat.rawRgba,
    ))!;
    final scaleX = image.width / output.width;
    final scaleY = image.height / output.height;
    final evidence = <String, dynamic>{};
    for (final entry in sprites.entries) {
      final rect = Rect.fromLTRB(
        entry.value.left * scaleX,
        entry.value.top * scaleY,
        entry.value.right * scaleX,
        entry.value.bottom * scaleY,
      );
      expect(rect.left, greaterThanOrEqualTo(0));
      expect(rect.top, greaterThanOrEqualTo(0));
      expect(rect.right, lessThanOrEqualTo(image.width));
      expect(rect.bottom, lessThanOrEqualTo(image.height));
      var bright = 0;
      var minimum = 255;
      var maximum = 0;
      for (var y = rect.top.ceil(); y < rect.bottom.floor(); y++) {
        for (var x = rect.left.ceil(); x < rect.right.floor(); x++) {
          final offset = (y * image.width + x) * 4;
          final light =
              (pixels.getUint8(offset) +
                  pixels.getUint8(offset + 1) +
                  pixels.getUint8(offset + 2)) ~/
              3;
          if (light >= 150) bright++;
          minimum = math.min(minimum, light);
          maximum = math.max(maximum, light);
        }
      }
      expect(
        bright,
        greaterThan(4),
        reason: '${entry.key} paints its real light sprite pixels',
      );
      expect(maximum - minimum, greaterThan(30), reason: entry.key);
      evidence[entry.key] = {
        'physicalRect': _rect(rect),
        'brightPixels': bright,
        'brightnessRange': maximum - minimum,
      };
    }
    return evidence;
  } finally {
    image.dispose();
    codec.dispose();
  }
}

Future<Map<String, dynamic>> _barPixels(
  File capture,
  Size output,
  Rect paintedBar,
) async {
  final codec = await ui.instantiateImageCodec(await capture.readAsBytes());
  final image = (await codec.getNextFrame()).image;
  try {
    expect(image.width, greaterThan(0));
    expect(image.height, greaterThan(0));
    final pixels = (await image.toByteData(
      format: ui.ImageByteFormat.rawRgba,
    ))!;
    final scaleX = image.width / output.width;
    final scaleY = image.height / output.height;
    final rect = Rect.fromLTRB(
      paintedBar.left * scaleX,
      paintedBar.top * scaleY,
      paintedBar.right * scaleX,
      paintedBar.bottom * scaleY,
    );
    expect(rect.left, greaterThanOrEqualTo(0));
    expect(rect.top, greaterThanOrEqualTo(0));
    expect(rect.right, lessThanOrEqualTo(image.width));
    expect(rect.bottom, lessThanOrEqualTo(image.height));
    final colors = <int>{};
    var minimum = 255;
    var maximum = 0;
    for (var y = rect.top.ceil(); y < rect.bottom.floor(); y++) {
      for (var x = rect.left.ceil(); x < rect.right.floor(); x++) {
        final offset = (y * image.width + x) * 4;
        final red = pixels.getUint8(offset);
        final green = pixels.getUint8(offset + 1);
        final blue = pixels.getUint8(offset + 2);
        colors.add((red << 16) | (green << 8) | blue);
        final light = (red + green + blue) ~/ 3;
        minimum = math.min(minimum, light);
        maximum = math.max(maximum, light);
      }
    }
    expect(
      colors.length,
      greaterThan(8),
      reason: 'Real bar pixels are painted',
    );
    expect(
      maximum - minimum,
      greaterThan(20),
      reason: 'Bar content is visible',
    );
    return {
      'imageWidth': image.width,
      'imageHeight': image.height,
      'physicalBarRect': _rect(rect),
      'distinctColors': colors.length,
      'brightnessRange': maximum - minimum,
    };
  } finally {
    image.dispose();
    codec.dispose();
  }
}

/// Locate a colorful, opaque sprite in actual compositor pixels. The search
/// uses the source asset, independently of native surface placement formulas.
Future<Map<String, dynamic>> _locateFinder(
  File capture,
  Size output,
  Rect expected,
) async {
  final screenshotCodec = await ui.instantiateImageCodec(
    await capture.readAsBytes(),
  );
  final screenshot = (await screenshotCodec.getNextFrame()).image;
  final sourceBytes = await rootBundle.load('assets/shell/finder.png');
  final sourceCodec = await ui.instantiateImageCodec(
    sourceBytes.buffer.asUint8List(
      sourceBytes.offsetInBytes,
      sourceBytes.lengthInBytes,
    ),
  );
  final source = (await sourceCodec.getNextFrame()).image;
  try {
    final pixels = (await screenshot.toByteData(
      format: ui.ImageByteFormat.rawRgba,
    ))!;
    final asset = (await source.toByteData(
      format: ui.ImageByteFormat.rawRgba,
    ))!;
    final scaleX = screenshot.width / output.width;
    final scaleY = screenshot.height / output.height;
    final width = expected.width * scaleX;
    final height = expected.height * scaleY;
    final samples = <(int, int, int, int, int)>[];
    for (var row = 0; row < 16; row++) {
      for (var column = 0; column < 16; column++) {
        final u = (column + .5) / 16;
        final v = (row + .5) / 16;
        final x = (u * source.width).floor();
        final y = (v * source.height).floor();
        final offset = (y * source.width + x) * 4;
        if (asset.getUint8(offset + 3) != 255) continue;
        final red = asset.getUint8(offset);
        final green = asset.getUint8(offset + 1);
        final blue = asset.getUint8(offset + 2);
        // Keep flat interior patches; antialiasing of sharp edges is renderer
        // dependent and contributes no useful placement evidence.
        var flat = true;
        for (final delta in [-2, 2]) {
          for (final nearby in [
            (y * source.width + x + delta) * 4,
            ((y + delta) * source.width + x) * 4,
          ]) {
            if (asset.getUint8(nearby + 3) != 255 ||
                (asset.getUint8(nearby) - red).abs() > 24 ||
                (asset.getUint8(nearby + 1) - green).abs() > 24 ||
                (asset.getUint8(nearby + 2) - blue).abs() > 24) {
              flat = false;
            }
          }
        }
        if (flat) {
          samples.add((
            (u * width).floor(),
            (v * height).floor(),
            red,
            green,
            blue,
          ));
        }
      }
    }
    expect(samples.length, greaterThan(40));
    final expectedLeft = expected.left * scaleX;
    final expectedTop = expected.top * scaleY;
    final left = math.max(0, (expectedLeft - 64 * scaleX).floor());
    final top = math.max(0, (expectedTop - 64 * scaleY).floor());
    final right = math.min(
      screenshot.width - width.ceil(),
      (expectedLeft + 64 * scaleX).ceil(),
    );
    final bottom = math.min(
      screenshot.height - height.ceil(),
      (expectedTop + 64 * scaleY).ceil(),
    );
    var bestError = double.infinity;
    var bestX = -1;
    var bestY = -1;
    for (var y = top; y <= bottom; y++) {
      for (var x = left; x <= right; x++) {
        var error = 0;
        for (final sample in samples) {
          final offset =
              ((y + sample.$2) * screenshot.width + x + sample.$1) * 4;
          error += (pixels.getUint8(offset) - sample.$3).abs();
          error += (pixels.getUint8(offset + 1) - sample.$4).abs();
          error += (pixels.getUint8(offset + 2) - sample.$5).abs();
          if (error >= bestError) break;
        }
        if (error < bestError) {
          bestError = error.toDouble();
          bestX = x;
          bestY = y;
        }
      }
    }
    final averageError = bestError / (samples.length * 3);
    expect(
      averageError,
      lessThan(30),
      reason: 'Real Finder asset must match the captured output',
    );
    expect(
      bestX,
      closeTo(expectedLeft, 2),
      reason:
          'Compositor x placement must match the native output-local origin',
    );
    expect(
      bestY,
      closeTo(expectedTop, 2),
      reason:
          'Compositor y placement must match the native output-local origin',
    );
    return {
      'actualPhysicalX': bestX,
      'actualPhysicalY': bestY,
      'expectedPhysicalX': expectedLeft,
      'expectedPhysicalY': expectedTop,
      'sampleCount': samples.length,
      'averageRgbError': averageError,
      'searchRadiusLogicalPixels': 64,
    };
  } finally {
    screenshot.dispose();
    screenshotCodec.dispose();
    source.dispose();
    sourceCodec.dispose();
  }
}

Future<void> _until(
  WidgetTester tester,
  bool Function() ready,
  String reason,
) async {
  final deadline = DateTime.now().add(const Duration(seconds: 15));
  while (!ready()) {
    if (DateTime.now().isAfter(deadline)) fail('Timed out: $reason');
    await tester.pump(const Duration(milliseconds: 20));
  }
}

Future<Map<String, dynamic>> _native(
  WidgetTester tester,
  _Controller controller, {
  bool expanded = false,
  bool menuVisible = true,
  bool dockSensor = false,
  int? dockId,
}) async {
  final deadline = DateTime.now().add(const Duration(seconds: 15));
  Map<String, dynamic> state = {};
  do {
    await tester.pump(const Duration(milliseconds: 20));
    state = await _state();
    final mac = controller.shellStyle == ShellStyle.macos;
    final prefs = controller.macPreferences;
    final geometry = state['monitorGeometry'] as Map;
    final monitorWidth = geometry['width'] as int;
    final monitorHeight = geometry['height'] as int;
    // Auto-hide exposes only its 2px sensor; reveal restores the configured
    // bar input height without recreating the view.
    // The drawing surface stays full-output sized across panel/style changes;
    // only its input strip and desktop reservation follow the painted bar.
    final height = monitorHeight;
    final barInputHeight = expanded
        ? monitorHeight
        : mac
        ? prefs.menuBarHeight.toInt()
        : 40;
    final inputHeight = mac && !expanded && !menuVisible ? 2 : barInputHeight;
    final width = !mac || expanded
        ? monitorWidth
        : (monitorWidth * prefs.menuBarWidthPercent / 100).round();
    final dock = state['dock'] as Map?;
    if (state['style'] != controller.shellStyle.name ||
        state['expanded'] != expanded ||
        state['height'] != height ||
        state['width'] != width ||
        state['tooltipVisible'] != false ||
        state['barBlurRequested'] != (mac && (menuVisible || expanded)) ||
        (mac && state['blurAvailable'] != true) ||
        (expanded && state['focused'] != true) ||
        (mac &&
            (state['menuBarVisible'] != menuVisible ||
                state['menuBarAutoHide'] != prefs.menuBarAutoHide ||
                state['menuBarWidthPercent'] != prefs.menuBarWidthPercent ||
                state['menuBarHeight'] != prefs.menuBarHeight ||
                dock?['position'] != prefs.dockPosition.name ||
                dock?['ready'] != true))) {
      continue;
    }
    if (mac && dock != null) {
      final horizontal = prefs.dockPosition == MacDockPosition.bottom;
      final cross = (horizontal ? dock['height'] : dock['width']) as int;
      final safeThickness = math
          .max(
            120,
            prefs.dockSize *
                    (prefs.magnification ? prefs.magnificationScale : 1) +
                50,
          )
          .ceil();
      final desiredThickness = MacDockView.surfaceThickness(prefs).ceil();
      final expectedZone = prefs.dockAutoHide
          ? 0
          : (prefs.dockSize + 16).ceil();
      final inputCross =
          (horizontal ? dock['inputHeight'] : dock['inputWidth']) as int;
      if (cross < safeThickness ||
          cross != desiredThickness ||
          dock['exclusiveZone'] != -1 ||
          dock['reservationZone'] != expectedZone ||
          dock['autoHide'] != prefs.dockAutoHide ||
          (!expanded && (dockSensor ? inputCross != 2 : inputCross <= 2))) {
        continue;
      }
      if (!expanded) {
        final glass = find.byKey(const ValueKey('mac-dock-glass'));
        if (glass.evaluate().isEmpty) continue;
        final painted = _paintedRect(tester, glass);
        final input = _input(dock);
        if ((horizontal ? input.left - painted.left : input.top - painted.top)
                    .abs() >
                1.01 ||
            (horizontal
                        ? input.width - painted.width
                        : input.height - painted.height)
                    .abs() >
                1.01) {
          continue;
        }
      }
    }
    expect(state['preview'], isFalse);
    expect(state['keyboard'], expanded ? 'Exclusive' : 'None');
    expect(state['layer'], expanded ? 'Overlay' : 'Top');
    expect(
      state['reservationZone'],
      mac
          ? prefs.menuBarAutoHide
                ? 0
                : prefs.menuBarHeight
          : 40,
    );
    expect(state['exclusiveZone'], mac ? -1 : 40);
    expect(state['inputHeight'], inputHeight);
    expect(
      state['inputY'],
      !mac && !expanded && controller.barPosition == BarPosition.bottom
          ? monitorHeight - 40
          : 0,
    );
    expect(state['barPosition'], mac ? 'top' : controller.barPosition.name);
    expect(
      state['anchorTop'],
      mac || controller.barPosition == BarPosition.top,
    );
    expect(
      state['anchorBottom'],
      !mac && controller.barPosition == BarPosition.bottom,
    );
    final bothSides = !mac || expanded || prefs.menuBarWidthPercent == 100;
    expect(state['anchorLeft'], bothSides);
    expect(state['anchorRight'], bothSides);
    if (dockId != null) expect(state['dockViewId'], dockId);
    if (dock != null) {
      expect(state['mainViewId'], isNot(state['dockViewId']));
      expect(dock['visible'], mac);
      expect(dock['layer'], 'Top');
      expect(dock['keyboard'], 'None');
      expect(dock['exclusiveZone'], -1);
      expect(dock['anchorTop'], isFalse);
      expect(
        dock['anchorBottom'],
        prefs.dockPosition == MacDockPosition.bottom,
      );
      expect(dock['anchorLeft'], prefs.dockPosition == MacDockPosition.left);
      expect(dock['anchorRight'], prefs.dockPosition == MacDockPosition.right);
      if (!mac || expanded) {
        expect(dock['inputWidth'], 0);
        expect(dock['inputHeight'], 0);
        if (!mac) expect(dock['reservationZone'], 0);
      } else {
        final bounds = Rect.fromLTWH(
          0,
          0,
          (dock['width'] as num).toDouble(),
          (dock['height'] as num).toDouble(),
        );
        final input = _input(dock);
        expect(input.isEmpty, isFalse);
        expect(bounds.inflate(.01).contains(input.topLeft), isTrue);
        expect(bounds.inflate(.01).contains(input.bottomRight), isTrue);
        if (dockSensor) {
          switch (prefs.dockPosition) {
            case MacDockPosition.bottom:
              expect(input.bottom, bounds.bottom);
            case MacDockPosition.left:
              expect(input.left, 0);
            case MacDockPosition.right:
              expect(input.right, bounds.right);
          }
        }
      }
    }
    return state;
  } while (DateTime.now().isBefore(deadline));
  fail('Native configuration did not settle: $state');
}

Future<Map<String, dynamic>> _placement(Map<String, dynamic> state) async {
  final geometry = state['monitorGeometry'] as Map;
  final dock = state['dock'] as Map;
  final width = geometry['width'] as int;
  final height = geometry['height'] as int;
  final dockWidth = dock['width'] as int;
  final dockHeight = dock['height'] as int;
  final dx = switch (dock['position']) {
    'left' => 0,
    'right' => width - dockWidth,
    _ => (width - dockWidth) ~/ 2,
  };
  final dy = dock['position'] == 'bottom'
      ? height - dockHeight
      : (height - dockHeight) ~/ 2;
  final origin = await _origin('dock');
  expect(origin['outputX'], dx);
  expect(origin['outputY'], dy);
  expect(origin['x'], (geometry['x'] as int) + dx);
  expect(origin['y'], (geometry['y'] as int) + dy);
  final main = await _origin('main');
  final mainX = state['style'] == 'macos' && state['expanded'] == false
      ? (width - (state['width'] as int)) ~/ 2
      : 0;
  final mainY = state['barPosition'] == 'bottom'
      ? height - (state['height'] as int)
      : 0;
  expect(main['outputX'], mainX);
  expect(main['outputY'], mainY);
  return {'main': main, 'dock': origin};
}

Future<Rect> _target(
  WidgetTester tester,
  _Controller controller,
  Map<String, dynamic> state, {
  bool interactive = true,
}) async {
  final origin = await _origin('dock');
  final icon = find.byKey(const ValueKey('dock-app-$_appId'));
  final dock = state['dock'] as Map;
  final painted = _paintedRect(tester, icon);
  if (interactive) {
    expect(_input(dock).inflate(1).contains(painted.center), isTrue);
  } else {
    expect(dock['inputWidth'], 0);
    expect(dock['inputHeight'], 0);
  }
  final expected =
      Rect.fromLTRB(
        painted.left.ceilToDouble(),
        painted.top.ceilToDouble(),
        painted.right.floorToDouble(),
        painted.bottom.floorToDouble(),
      ).shift(
        Offset(
          (origin['outputX'] as num).toDouble(),
          (origin['outputY'] as num).toDouble(),
        ),
      );
  await _until(tester, () {
    final actual = controller.targets[_windowId];
    return actual != null &&
        (actual.left - expected.left).abs() <= 1 &&
        (actual.top - expected.top).abs() <= 1 &&
        (actual.width - expected.width).abs() <= 1 &&
        (actual.height - expected.height).abs() <= 1;
  }, 'fresh output-local icon target $expected');
  final target = controller.targets[_windowId]!;
  final geometry = state['monitorGeometry'] as Map;
  final output = Rect.fromLTWH(
    0,
    0,
    (geometry['width'] as num).toDouble(),
    (geometry['height'] as num).toDouble(),
  );
  expect(output.contains(target.center), isTrue);
  return target;
}

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  final filter = _DeviceFilter(binding);
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.onlyPumps;
  testWidgets('real GTK layers apply isolated macOS configuration in place', (
    tester,
  ) async {
    expect(Platform.isLinux, isTrue);
    expect(Platform.environment['CLARION_SHELL_ISOLATED_TEST'], '1');
    final root = Platform.environment['CLARION_SHELL_TEST_ROOT']!;
    for (final (child, parent) in [
      ('WAYLAND_DISPLAY', 'CLARION_SHELL_PARENT_WAYLAND_DISPLAY'),
      ('CLARION_SOCKET', 'CLARION_SHELL_PARENT_SOCKET'),
      ('DBUS_SESSION_BUS_ADDRESS', 'CLARION_SHELL_PARENT_DBUS_ADDRESS'),
    ]) {
      expect(Platform.environment[child], isNotEmpty);
      expect(Platform.environment[child], isNot(Platform.environment[parent]));
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
    await captures.create(recursive: true);
    final ipc = _Ipc();
    final apps = _Apps();
    final menus = _Menus();
    final controller = _Controller(ipc, apps, menus);
    final heightOnly =
        Platform.environment['CLARION_SHELL_MENU_HEIGHT_ONLY'] == '1';
    final lifecycleOnly =
        Platform.environment['CLARION_SHELL_MATERIAL_LIFECYCLE_ONLY'] == '1';
    expect(heightOnly && lifecycleOnly, isFalse);
    controller.setReduceMotion(true);
    controller.setBarPosition(BarPosition.bottom);
    controller.setPinnedAppIds(const [_appId]);
    controller.setShellStyle(ShellStyle.macos);
    const preferences = ShellPreferences(
      reduceMotion: true,
      barPosition: BarPosition.bottom,
      shellStyle: ShellStyle.macos,
      pinnedAppIds: [_appId],
    );
    final phases = <Map<String, dynamic>>[];
    final evidence = <String, dynamic>{
      'passed': false,
      'phases': phases,
      'scope':
          'Actual GTK Wayland layer surfaces, shared Flutter views and native '
          'surface origins. Menu/app/system services and window snapshots are isolated fakes.',
      'pointerScope':
          'Injected Flutter TestPointer events; windowState inspects native '
          'input shapes. Physical compositor mouse/keyboard delivery is not certified.',
      'targetScope':
          'Production native surfaceOrigin, monitor geometry ownership '
          'matching and controller SetMinimizeTarget serialization. Fake Outputs '
          'mirror native geometry; a fake IPC records actual arguments without '
          'sending an actual compositor minimize action. Each phase separately '
          'compares native monitor geometry exactly and uniquely against the '
          'initial real private Outputs snapshot; this matrix never changes '
          'outputs. Launchpad additionally reads fresh actual Outputs.',
    };
    int? dockId;
    int? mainId;
    Object? mainElement;
    Object? dockElement;
    var assetsDecoded = false;
    Map<String, dynamic>? actualOutputSnapshot;
    final activePointers = <(TestPointer, ui.FlutterView)>[];

    Future<void> record(
      String name, {
      bool expanded = false,
      bool menuVisible = true,
      bool dockSensor = false,
      bool target = false,
    }) async {
      var state = await _native(
        tester,
        controller,
        expanded: expanded,
        menuVisible: menuVisible,
        dockSensor: dockSensor,
        dockId: dockId,
      );
      dockId ??= state['dockViewId'] as int;
      mainId ??= state['mainViewId'] as int;
      expect(state['mainViewId'], mainId);
      expect(binding.platformDispatcher.views.length, 2);
      final mainFinder = find.byType(ShellApplication);
      final dockFinder = find.byType(DockApplication);
      mainElement ??= tester.element(mainFinder);
      dockElement ??= tester.element(dockFinder);
      expect(tester.element(mainFinder), same(mainElement));
      expect(tester.element(dockFinder), same(dockElement));
      expect(
        tester.widget<ShellApplication>(mainFinder).controller,
        same(controller),
      );
      expect(
        tester.widget<DockApplication>(dockFinder).controller,
        same(controller),
      );
      expect(ipc.starts, 1);
      expect(ipc.disposals, 0);
      if (!assetsDecoded) {
        final manifest = await AssetManifest.loadFromAssetBundle(rootBundle);
        final assets = manifest
            .listAssets()
            .where(
              (path) =>
                  path.startsWith('assets/shell/') && path.endsWith('.png'),
            )
            .toList();
        expect(assets, isNotEmpty);
        final errors = <String>[];
        await tester.runAsync(
          () => Future.wait([
            for (final asset in assets)
              precacheImage(
                AssetImage(asset),
                tester.element(mainFinder),
                onError: (error, _) => errors.add('$asset: $error'),
              ),
          ]),
        );
        expect(errors, isEmpty, reason: 'All real sprite ImageStreams decode');
        evidence['decodedAssetsBeforeScreenshots'] = assets;
        assetsDecoded = true;
      }
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 250));
      // Asset decoding can span native configure events. Validate the current
      // allocation again rather than using a state captured before async I/O.
      state = await _native(
        tester,
        controller,
        expanded: expanded,
        menuVisible: menuVisible,
        dockSensor: dockSensor,
        dockId: dockId,
      );
      final placement = await _placement(state);
      final mappedLayers = await _layers(tester, state);
      final actualOutput = await _actualOutputOwnership(
        tester,
        state,
        outputSnapshot: actualOutputSnapshot,
      );
      actualOutputSnapshot ??=
          actualOutput['actualOutputs'] as Map<String, dynamic>;
      Map<String, double>? panelPlacement;
      if (expanded &&
          (controller.panel == ShellPanel.quickSettings ||
              controller.panel == ShellPanel.systemMenu)) {
        final panel = _paintedRect(
          tester,
          find.byKey(
            ValueKey(
              controller.panel == ShellPanel.quickSettings
                  ? 'mac-control-center-glass'
                  : 'mac-panel-position',
            ),
          ),
        );
        final bar = _paintedRect(
          tester,
          find.byKey(const ValueKey('mac-menu-bar-position')),
        );
        final outputWidth = (state['width'] as num).toDouble();
        final inset = (bar.left + 12).clamp(0.0, outputWidth - panel.width);
        if (controller.panel == ShellPanel.systemMenu) {
          expect(panel.left, closeTo(inset, 1.1));
          expect(
            panel.top,
            closeTo(controller.macPreferences.menuBarHeight + 4, 1.1),
          );
        } else {
          expect(panel.right, closeTo(outputWidth - inset, 1.1));
          expect(
            panel.top,
            closeTo(controller.macPreferences.menuBarHeight + 6, 1.1),
          );
        }
        panelPlacement = _rect(panel);
      }
      final capture = '${captures.path}/$name.png';
      await tester.runAsync(() async {
        final image = await Process.run('grim', [
          '-o',
          Platform.environment['CLARION_SHELL_TEST_OUTPUT'] ?? 'winit',
          capture,
        ]).timeout(const Duration(seconds: 15));
        expect(image.exitCode, 0, reason: '${image.stdout}\n${image.stderr}');
        expect(await File(capture).length(), greaterThan(100));
      });
      Map<String, dynamic>? actualFinderPlacement;
      if (!heightOnly &&
          state['style'] == 'macos' &&
          (!controller.macPreferences.dockAutoHide ||
              (!dockSensor && !expanded))) {
        final geometry = state['monitorGeometry'] as Map;
        final origin = placement['dock'] as Map;
        final finder = _paintedRect(
          tester,
          find.byKey(const ValueKey('dock-finder')),
        );
        actualFinderPlacement = await tester.runAsync(
          () => _locateFinder(
            File(capture),
            Size(
              (geometry['width'] as num).toDouble(),
              (geometry['height'] as num).toDouble(),
            ),
            finder.shift(
              Offset(
                (origin['outputX'] as num).toDouble(),
                (origin['outputY'] as num).toDouble(),
              ),
            ),
          ),
        );
      }
      if (name == '01-default-menu-100-dock-bottom-48') {
        final geometry = state['monitorGeometry'] as Map;
        final origin = placement['dock'] as Map;
        final offset = Offset(
          (origin['outputX'] as num).toDouble(),
          (origin['outputY'] as num).toDouble(),
        );
        evidence['actualDefaultSpritePixels'] = await tester.runAsync(
          () => _spritePixels(
            File(capture),
            Size(
              (geometry['width'] as num).toDouble(),
              (geometry['height'] as num).toDouble(),
            ),
            {
              for (final key in ['dock-finder', 'dock-trash'])
                key: _paintedRect(
                  tester,
                  find.byKey(ValueKey(key)),
                ).shift(offset),
            },
          ),
        );
      }
      expect(
        tester.takeException(),
        isNull,
        reason: '$name has no layout overflow',
      );
      final capturedTarget = target
          ? await _target(tester, controller, state, interactive: !expanded)
          : null;
      phases.add({
        'name': name,
        'native': await _state(),
        'origins': placement,
        'actualCompositorLayers': mappedLayers,
        'actualCompositorOutputOwnership': actualOutput,
        'panelOutputLocalBounds': ?panelPlacement,
        'actualCompositorFinderPlacement': ?actualFinderPlacement,
        'preferences': controller.macPreferences.toJson(),
        'capture': capture,
        if (capturedTarget != null) 'outputLocalTarget': _rect(capturedTarget),
      });
    }

    Future<(TestPointer, ui.FlutterView)> pointer(Finder widget, int id) async {
      final view = View.of(tester.element(widget));
      final mouse = TestPointer(id, PointerDeviceKind.mouse, id);
      await tester.sendEventToBinding(
        mouse.addPointer(location: const Offset(1, 1), view: view),
      );
      final pair = (mouse, view);
      activePointers.add(pair);
      await tester.sendEventToBinding(
        mouse.hover(tester.getCenter(widget), view: view),
      );
      await tester.pump();
      return pair;
    }

    Future<void> remove((TestPointer, ui.FlutterView) pair) async {
      await tester.sendEventToBinding(pair.$1.removePointer(view: pair.$2));
      activePointers.remove(pair);
      await tester.pump();
    }

    Future<void> runStyleSurfaceLifecycle() async {
      // Exercise the actual GTK layer changes with the same two Flutter views.
      // Opening settings changes Top to Overlay; switching out of that panel
      // also changes the Material bottom anchor and maps/unmaps the Dock.
      // One-frame reversals keep native replies/render work in flight instead
      // of hiding the lifecycle race behind a long settle between selections.
      final lifecyclePhases = <Map<String, dynamic>>[];
      final lifecycleEvidence = <String, dynamic>{
        'cycles': 20,
        'completedCycles': 0,
        'phases': lifecyclePhases,
        'scope':
            'Real private compositor layer mapping and GTK allocations; '
            'shared Flutter views retained during settings/style reversals. '
            'No physical input or primary desktop restart.',
      };
      evidence['styleSurfaceLifecycle'] = lifecycleEvidence;
      Future<void> lifecycleState(
        int cycle,
        String transition, {
        bool expanded = false,
      }) async {
        final phaseElapsed = Stopwatch()..start();
        final phase = <String, dynamic>{
          'cycle': cycle,
          'transition': transition,
          'requestedStyle': controller.shellStyle.name,
          'requestedExpanded': expanded,
          'startedAtUtc': DateTime.now().toUtc().toIso8601String(),
          'status': 'waiting-native',
        };
        lifecyclePhases.add(phase);
        await _diagnosticLine('material-lifecycle-phases', phase);
        final state = await _native(
          tester,
          controller,
          expanded: expanded,
          dockId: dockId,
        );
        phase['native'] = state;
        phase['nativeReadyMicros'] = phaseElapsed.elapsedMicroseconds;
        phase['status'] = 'waiting-layers';
        await _diagnosticLine('material-lifecycle-phases', phase);
        final layers = await _layers(
          tester,
          state,
          context: {'cycle': cycle, 'transition': transition},
        );
        phase['actualCompositorLayers'] = layers;
        expect(state['mainViewId'], mainId);
        expect(state['dockViewId'], dockId);
        expect(binding.platformDispatcher.views.length, 2);
        expect(
          tester.element(find.byType(ShellApplication)),
          same(mainElement),
        );
        expect(tester.element(find.byType(DockApplication)), same(dockElement));
        expect(ipc.starts, 1);
        expect(ipc.disposals, 0);
        expect(controller.barPosition, BarPosition.bottom);
        final material = controller.shellStyle == ShellStyle.material;
        final bar = _paintedRect(
          tester,
          find.byKey(
            ValueKey(material ? 'shell-system-bar' : 'mac-menu-bar-position'),
          ),
        );
        expect(
          bar.height,
          closeTo(material ? 40 : controller.macPreferences.menuBarHeight, .01),
        );
        if (material) {
          expect(bar.bottom, closeTo((state['height'] as num).toDouble(), .01));
          expect(state['anchorTop'], isFalse);
          expect(state['anchorBottom'], isTrue);
          expect((state['dock'] as Map)['visible'], isFalse);
        } else {
          expect(bar.top, closeTo(0, .01));
          expect((state['dock'] as Map)['visible'], isTrue);
          expect((state['dock'] as Map)['ready'], isTrue);
        }
        expect(tester.takeException(), isNull, reason: '$cycle $transition');
        phase['paintedBarRect'] = _rect(bar);
        phase['status'] = 'passed';
        phase['completedMicros'] = phaseElapsed.elapsedMicroseconds;
        await _diagnosticLine('material-lifecycle-phases', phase);
      }

      for (var cycle = 1; cycle <= 20; cycle++) {
        controller.selectPanel(ShellPanel.quickSettings);
        await lifecycleState(cycle, 'macos-settings-overlay', expanded: true);
        controller.setShellStyle(ShellStyle.material);
        await tester.pump();
        controller.setShellStyle(ShellStyle.macos);
        controller.setShellStyle(ShellStyle.material);
        await lifecycleState(cycle, 'material-bottom-dock-unmapped');
        if (cycle == 20) {
          await record('25a-material-bottom-after-20-style-reversals');
        }
        controller.selectPanel(ShellPanel.quickSettings);
        await lifecycleState(
          cycle,
          'material-settings-overlay',
          expanded: true,
        );
        controller.closePanel();
        controller.selectPanel(ShellPanel.quickSettings);
        await tester.pump();
        controller.setShellStyle(ShellStyle.macos);
        await lifecycleState(cycle, 'macos-top-dock-remapped');
        controller.selectPanel(ShellPanel.quickSettings);
        await lifecycleState(cycle, 'macos-settings-restored', expanded: true);
        controller.closePanel();
        await lifecycleState(cycle, 'macos-settings-closed');
        lifecycleEvidence['completedCycles'] = cycle;
      }
      await record('25b-macos-top-after-20-style-reversals', target: true);
      final lifecyclePixels = <String, dynamic>{};
      for (final name in [
        '25a-material-bottom-after-20-style-reversals',
        '25b-macos-top-after-20-style-reversals',
      ]) {
        final phase = phases.singleWhere((phase) => phase['name'] == name);
        final state = phase['native'] as Map;
        final geometry = state['monitorGeometry'] as Map;
        final origin = (phase['origins'] as Map)['main'] as Map;
        final material = state['style'] == 'material';
        final paintedBar = Rect.fromLTWH(
          (origin['outputX'] as num).toDouble(),
          (origin['outputY'] as num).toDouble() +
              (material ? (state['height'] as num).toDouble() - 40 : 0),
          (state['width'] as num).toDouble(),
          material ? 40 : controller.macPreferences.menuBarHeight,
        );
        lifecyclePixels[state['style'] as String] = await tester.runAsync(
          () => _barPixels(
            File(phase['capture'] as String),
            Size(
              (geometry['width'] as num).toDouble(),
              (geometry['height'] as num).toDouble(),
            ),
            paintedBar,
          ),
        );
      }
      lifecycleEvidence['actualBarPixelsAfterCycles'] = lifecyclePixels;
    }

    try {
      await tester.pumpWidget(
        ShellHost(
          controller: controller,
          preferences: preferences,
          persist: true,
        ),
        wrapWithView: false,
      );
      await controller.start();
      await _until(
        tester,
        () => controller.connected && controller.apps.length == 1,
        'isolated desktop snapshot/catalogue',
      );
      await record('01-default-menu-100-dock-bottom-48', target: true);
      if (lifecycleOnly) {
        await runStyleSurfaceLifecycle();
        final deadline = DateTime.now().add(const Duration(seconds: 15));
        var saved = await ShellPreferences.load();
        while (saved.macShell != controller.macPreferences ||
            saved.shellStyle != ShellStyle.macos) {
          if (DateTime.now().isAfter(deadline)) {
            fail('Current style lifecycle configuration did not persist');
          }
          await tester.pump(const Duration(milliseconds: 20));
          saved = await ShellPreferences.load();
        }
        expect(saved.macShell, controller.macPreferences);
        expect(saved.barPosition, BarPosition.bottom);
        expect(saved.pinnedAppIds, [_appId]);
        evidence['mode'] = 'material-surface-lifecycle';
        evidence['savedPreferences'] = saved.toJson();
        evidence['mainViewId'] = mainId;
        evidence['dockViewId'] = dockId;
        evidence['sharedControllerIdentity'] = identityHashCode(controller);
        evidence['passed'] = true;
        binding.reportData = evidence;
        return;
      }
      await tester.tap(
        find.byKey(const ValueKey('dock-app-$_appId')),
        kind: PointerDeviceKind.mouse,
        buttons: kSecondaryMouseButton,
      );
      // Opening a native Dock menu awaits surfaceOrigin; tap dispatch alone
      // does not complete that platform-channel reply.
      await _until(
        tester,
        () => controller.panel == ShellPanel.dockAppMenu,
        'Dock context menu after the native origin reply',
      );
      expect(controller.panel, ShellPanel.dockAppMenu);
      await record(
        '01b-visible-dock-context-menu-keeps-target-and-blocks-input',
        expanded: true,
        target: true,
      );
      expect(controller.targets[_windowId], isNotNull);
      controller.closePanel();
      await record('01c-dock-context-menu-closed-input-restored', target: true);
      controller.setMacPreferences(
        controller.macPreferences.copyWith(menuBarWidthPercent: 65),
      );
      await record('02-menu-65-collapsed', target: true);
      controller.selectPanel(ShellPanel.quickSettings);
      await record('03-menu-65-expanded-full-output', expanded: true);
      final bar = _paintedRect(
        tester,
        find.byKey(const ValueKey('mac-menu-bar-position')),
      );
      final geometry = (await _state())['monitorGeometry'] as Map;
      expect(bar.width, closeTo((geometry['width'] as num) * .65, 1.1));
      expect(bar.center.dx, closeTo((geometry['width'] as num) / 2, 1.1));
      controller.closePanel();
      controller.selectPanel(ShellPanel.systemMenu);
      await record(
        '03b-menu-65-system-panel-follows-left-edge',
        expanded: true,
      );
      controller.closePanel();
      await record('04-menu-65-collapsed-again', target: true);

      // User-configurable thickness must change actual GTK geometry, input and
      // desktop reservations together. Legacy width stays readable independently.
      for (final menuHeight in [32.0, 48.0]) {
        controller.setMacPreferences(
          controller.macPreferences.copyWith(menuBarHeight: menuHeight),
        );
        await record(
          '04a-menu-height-${menuHeight.toInt()}-collapsed',
          target: true,
        );
        expect(
          _paintedRect(
            tester,
            find.byKey(const ValueKey('mac-menu-bar-position')),
          ).height,
          closeTo(menuHeight, .01),
        );
        controller.selectPanel(ShellPanel.quickSettings);
        await record(
          '04b-menu-height-${menuHeight.toInt()}-panel',
          expanded: true,
        );
        controller.closePanel();
      }
      for (final invalidHeight in [21, 49, 22.5, double.nan, '32']) {
        await expectLater(
          _window.invokeMethod<void>('setMacMenuBarPreferences', {
            'autoHide': false,
            'visible': true,
            'widthPercent': 65,
            'height': invalidHeight,
          }),
          throwsA(isA<PlatformException>()),
        );
        expect((await _state())['menuBarHeight'], 48);
      }

      controller.setMacPreferences(
        controller.macPreferences.copyWith(menuBarAutoHide: true),
      );
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 400));
      await record(
        '05-menu-hidden-2px-zone-zero',
        menuVisible: false,
        target: true,
      );
      final menuMouse = await pointer(
        find.byKey(const ValueKey('mac-menu-edge-sensor')),
        880,
      );
      await record('06-menu-sensor-revealed-48px-zone-zero', target: true);
      await tester.tap(find.byKey(const ValueKey('global-menu-file')));
      await remove(menuMouse);
      await tester.pump(const Duration(milliseconds: 500));
      await record(
        '07-open-application-menu-keeps-bar-visible',
        expanded: true,
      );
      expect(controller.appMenuOpen, isTrue);
      await tester.tap(find.text('Configuration action'));
      await tester.pump();
      expect(menus.activations, 1);
      await tester.pump(const Duration(milliseconds: 500));
      await record(
        '08-application-menu-closed-bar-hidden',
        menuVisible: false,
        target: true,
      );
      controller.selectPanel(ShellPanel.quickSettings);
      await record('09-open-panel-keeps-autohide-bar-visible', expanded: true);
      controller.closePanel();
      await tester.pump(const Duration(milliseconds: 500));
      await record(
        '10-panel-closed-bar-hidden',
        menuVisible: false,
        target: true,
      );
      controller.setMacPreferences(
        controller.macPreferences.copyWith(
          menuBarAutoHide: false,
          menuBarHeight: 22,
        ),
      );
      await record('11-menu-autohide-disabled-visible', target: true);

      if (heightOnly) {
        // A focused matrix avoids unrelated full-output Launchpad blur when
        // validating strip geometry on a second scale. Full mode remains intact.
        controller.setMacPreferences(
          controller.macPreferences.copyWith(
            menuBarHeight: 48,
            menuBarWidthPercent: 100,
          ),
        );
        controller.setShellStyle(ShellStyle.material);
        await record('11a-height-preference-keeps-material-40px');
        controller.setShellStyle(ShellStyle.macos);
        await record('11b-height-48-restored-after-style-switch', target: true);
        final deadline = DateTime.now().add(const Duration(seconds: 15));
        var saved = await ShellPreferences.load();
        while (saved.macShell != controller.macPreferences ||
            saved.shellStyle != ShellStyle.macos) {
          if (DateTime.now().isAfter(deadline)) {
            fail('Menu height did not persist');
          }
          await tester.pump(const Duration(milliseconds: 50));
          saved = await ShellPreferences.load();
        }
        expect(saved.macShell.menuBarHeight, 48);
        expect(saved.pinnedAppIds, [_appId]);
        evidence['mode'] = 'menu-height';
        evidence['finderTemplateMatching'] =
            'covered by full matrix; omitted in focused height matrix';
        evidence['savedPreferences'] = saved.toJson();
        evidence['mainViewId'] = mainId;
        evidence['dockViewId'] = dockId;
        evidence['sharedControllerIdentity'] = identityHashCode(controller);
        evidence['passed'] = true;
        binding.reportData = evidence;
        return;
      }

      // Keep enough native length for a real 64px icon at 2x even on scale-2
      // outputs. This also verifies utilities disappear through shared config.
      controller.setMacPreferences(
        controller.macPreferences.copyWith(
          showLaunchpad: false,
          showDownloads: false,
          magnificationScale: 2,
          dockSize: 32,
        ),
      );
      await record('12-dock-bottom-32-magnification-2', target: true);
      expect(
        tester
            .getSize(find.byKey(const ValueKey('dock-icon-size-$_appId')))
            .width,
        32,
      );
      expect(find.byKey(const ValueKey('dock-downloads')), findsNothing);
      controller.setMacPreferences(
        controller.macPreferences.copyWith(dockSize: 64),
      );
      await record('13-dock-bottom-64-magnification-2', target: true);
      expect(
        tester
            .getSize(find.byKey(const ValueKey('dock-icon-size-$_appId')))
            .width,
        64,
      );
      controller.setReduceMotion(false);
      final iconMouse = await pointer(
        find.byKey(const ValueKey('dock-app-$_appId')),
        881,
      );
      await tester.pump(const Duration(milliseconds: 250));
      final transform = tester.widget<Transform>(
        find.byKey(const ValueKey('dock-scale-Configuration App')),
      );
      expect(transform.transform.entry(0, 0), closeTo(2, .02));
      await record('14-dock-bottom-64-real-2x-magnification', target: true);
      await remove(iconMouse);
      await tester.pump(const Duration(milliseconds: 250));
      controller.setReduceMotion(true);

      for (final position in [MacDockPosition.left, MacDockPosition.right]) {
        controller.setMacPreferences(
          controller.macPreferences.copyWith(
            dockPosition: position,
            dockSize: 32,
          ),
        );
        await record('15-${position.name}-32-orientation-origin', target: true);
        controller.setMacPreferences(
          controller.macPreferences.copyWith(dockSize: 64),
        );
        await record(
          '16-${position.name}-64-adaptive-fit-origin',
          target: true,
        );
        controller.setMacPreferences(
          controller.macPreferences.copyWith(dockSize: 32, dockAutoHide: true),
        );
        await record(
          '17-${position.name}-hidden-edge-input-zone-zero',
          dockSensor: true,
        );
        await _until(
          tester,
          () => controller.targets.isEmpty,
          'hidden Dock clears icon targets',
        );
        final dockMouse = await pointer(
          find.byKey(const ValueKey('mac-dock-edge-sensor')),
          882,
        );
        await record(
          '18-${position.name}-sensor-revealed-zone-zero',
          target: true,
        );
        await remove(dockMouse);
        await tester.pump(const Duration(milliseconds: 450));
        await record(
          '19-${position.name}-pointer-exit-hidden',
          dockSensor: true,
        );
        controller.setMacPreferences(
          controller.macPreferences.copyWith(dockAutoHide: false),
        );
      }
      controller.setMacPreferences(
        controller.macPreferences.copyWith(
          dockPosition: MacDockPosition.bottom,
          dockAutoHide: true,
        ),
      );
      await record('20-bottom-hidden-edge-input-zone-zero', dockSensor: true);
      final bottomMouse = await pointer(
        find.byKey(const ValueKey('mac-dock-edge-sensor')),
        883,
      );
      await record('21-bottom-sensor-revealed-zone-zero', target: true);
      await remove(bottomMouse);
      await tester.pump(const Duration(milliseconds: 450));
      await record('22-bottom-pointer-exit-hidden', dockSensor: true);

      controller.setShellStyle(ShellStyle.material);
      await record('23-material-original-bottom-dock-unmapped-zone-zero');
      expect(controller.barPosition, BarPosition.bottom);
      await _until(
        tester,
        () => controller.targets.isEmpty,
        'Material clears icon targets',
      );
      controller.setShellStyle(ShellStyle.macos);
      await record(
        '24-macos-return-reuses-view-and-hidden-dock',
        dockSensor: true,
      );
      controller.setMacPreferences(
        controller.macPreferences.copyWith(dockAutoHide: false),
      );
      await record('25-macos-return-fresh-visible-target', target: true);

      await runStyleSurfaceLifecycle();
      final saveDeadline = DateTime.now().add(const Duration(seconds: 15));
      var saved = await ShellPreferences.load();
      while (saved.macShell != controller.macPreferences ||
          saved.shellStyle != ShellStyle.macos) {
        if (DateTime.now().isAfter(saveDeadline)) {
          fail('Current macOS configuration did not persist');
        }
        await tester.pump(const Duration(milliseconds: 20));
        saved = await ShellPreferences.load();
      }
      expect(saved.macShell, controller.macPreferences);
      expect(saved.barPosition, BarPosition.bottom);
      expect(saved.pinnedAppIds, [_appId]);
      evidence['savedPreferences'] = saved.toJson();
      // The frozen configuration snapshot predates Launchpad's native scene
      // contract. The final original-project run exercises it when advertised.
      if ((await _state()).containsKey('launcherBackdropRequested')) {
        final previousPreferences = controller.macPreferences;
        controller.setMacPreferences(
          previousPreferences.copyWith(showLaunchpad: true),
        );
        controller.setReduceMotion(false);
        await record('26-original-launchpad-entry-restored', target: true);
        final restingFinder =
            phases.last['actualCompositorFinderPlacement'] as Map;
        final restingDock = (phases.last['native'] as Map)['dock'] as Map;
        await tester.tap(find.byKey(const ValueKey('dock-launcher')));
        final launchDeadline = DateTime.now().add(const Duration(seconds: 15));
        Map<String, dynamic> launch = {};
        do {
          await tester.pump(const Duration(milliseconds: 20));
          launch = await _state();
          final dock = launch['dock'] as Map;
          if (launch['launcherSceneVisible'] == true &&
              launch['expanded'] == true &&
              launch['focused'] == true &&
              dock['layer'] == 'Overlay' &&
              (dock['inputWidth'] as int) > 0 &&
              (dock['inputHeight'] as int) > 0 &&
              find
                  .byKey(const ValueKey('mac-launchpad-background'))
                  .evaluate()
                  .isNotEmpty &&
              (_paintedRect(tester, find.byType(LauncherSurface)).height -
                          ((launch['monitorGeometry'] as Map)['height'] as num))
                      .abs() <=
                  1) {
            break;
          }
        } while (DateTime.now().isBefore(launchDeadline));
        expect(controller.panel, ShellPanel.launcher);
        expectLauncherGlass(launch);
        expect(launch['barBlurRequested'], isFalse);
        expect(launch['blurAvailable'], isTrue);
        expect(launch['height'], (launch['monitorGeometry'] as Map)['height']);
        expect(launch['keyboard'], 'Exclusive');
        expect((launch['dock'] as Map)['layer'], 'Overlay');
        expect((launch['dock'] as Map)['keyboard'], 'None');
        await tester.pump(const Duration(milliseconds: 300));
        launch = await _state();
        final menuVisibility = tester.widget<Opacity>(
          find.byKey(const ValueKey('mac-menu-bar-launchpad-visibility')),
        );
        expect(menuVisibility.opacity, 0);
        final background = _paintedRect(
          tester,
          find.byKey(const ValueKey('mac-launchpad-background')),
        );
        final fullScene = _paintedRect(tester, find.byType(LauncherSurface));
        final geometry = launch['monitorGeometry'] as Map;
        final insets = ShellGeometry.desktopInsets(
          ShellStyle.macos,
          controller.barPosition,
          macPreferences: controller.macPreferences,
        ).copyWith(top: 0);
        expect(
          tester
              .widget<Positioned>(
                find.byKey(const ValueKey('mac-launcher-position')),
              )
              .top,
          0,
        );
        expect(fullScene.top, closeTo(0, 1));
        expect(fullScene.left, closeTo(0, 1));
        expect(fullScene.height, closeTo(geometry['height'] as num, 1));
        expect(fullScene.width, closeTo(geometry['width'] as num, 1));
        expect(background.top, closeTo(insets.top, 1));
        expect(background.left, closeTo(insets.left, 1));
        expect(
          background.height,
          closeTo((geometry['height'] as num) - insets.vertical, 1),
        );
        expect(
          background.width,
          closeTo((geometry['width'] as num) - insets.horizontal, 1),
        );
        await tester.pump(const Duration(milliseconds: 300));
        final launcherLayers = await _layers(tester, launch);
        final launcherOutput = await _actualOutputOwnership(tester, launch);
        final launcherOrigin = await _origin('dock');
        final launcherCapture =
            '${captures.path}/27-original-launchpad-full-scene.png';
        final actualFinder = await tester.runAsync(() async {
          final result = await Process.run('grim', [
            '-o',
            'winit',
            launcherCapture,
          ]).timeout(const Duration(seconds: 15));
          expect(
            result.exitCode,
            0,
            reason: '${result.stdout}\n${result.stderr}',
          );
          final geometry = launch['monitorGeometry'] as Map;
          return _locateFinder(
            File(launcherCapture),
            Size(
              (geometry['width'] as num).toDouble(),
              (geometry['height'] as num).toDouble(),
            ),
            _paintedRect(
              tester,
              find.byKey(const ValueKey('dock-finder')),
            ).shift(
              Offset(
                (launcherOrigin['outputX'] as num).toDouble(),
                (launcherOrigin['outputY'] as num).toDouble(),
              ),
            ),
          );
        });
        expect((launch['dock'] as Map)['width'], restingDock['width']);
        expect((launch['dock'] as Map)['height'], restingDock['height']);
        final finderRgbError = actualFinder!['averageRgbError'] as num;
        final restingFinderRgbError = restingFinder['averageRgbError'] as num;
        expect(
          finderRgbError,
          lessThanOrEqualTo(restingFinderRgbError + 3),
          reason:
              'Opaque Finder samples retain their pre-Launchpad colors; '
              'the Overlay Dock must not be darkened by the launcher scrim',
        );
        phases.add({
          'name': '27-original-launchpad-full-scene',
          'native': launch,
          'capture': launcherCapture,
          'actualCompositorLayers': launcherLayers,
          'actualCompositorOutputOwnership': launcherOutput,
          'fullSceneBounds': _rect(fullScene),
          'dockAvoidingPanelBounds': _rect(background),
          'actualCompositorFinderPlacement': actualFinder,
          'finderOpaqueSampleComparison': {
            'restingAverageRgbError': restingFinderRgbError,
            'launcherAverageRgbError': finderRgbError,
            'allowedIncrease': 3,
            'scope':
                'The same opaque source samples, geometry and template matcher '
                'before and during Launchpad, in addition to the existing '
                'absolute RGB and actual placement limits.',
          },
          'scope':
              'Native full-surface glass request, actual Overlay Dock layer/input and injected UI activation; no frame-for-frame macOS claim.',
        });
        await tester.sendKeyEvent(LogicalKeyboardKey.escape);
        expect(controller.panel, ShellPanel.none);
        await tester.pump(const Duration(milliseconds: 20));
        final retained = await _state();
        expect(
          retained['launcherSceneVisible'],
          isTrue,
          reason: 'Launcher scene survives the real retained exit frame',
        );
        expectLauncherGlass(retained);
        expect((retained['dock'] as Map)['layer'], 'Overlay');
        expect((retained['dock'] as Map)['inputWidth'], 0);
        expect((retained['dock'] as Map)['inputHeight'], 0);
        expect(
          tester
              .widget<Opacity>(
                find.byKey(const ValueKey('mac-menu-bar-launchpad-visibility')),
              )
              .opacity,
          0,
        );
        phases.add({
          'name': '28-original-launchpad-retained-exit',
          'native': retained,
        });
        await tester.pump(const Duration(milliseconds: 300));
        controller.setReduceMotion(true);
        controller.setMacPreferences(previousPreferences);
        await record(
          '29-original-launchpad-closed-top-dock-and-menu-restored',
          target: true,
        );
        expect((await _state())['launcherBackdropRequested'], isFalse);
        expect(
          tester
              .widget<Opacity>(
                find.byKey(const ValueKey('mac-menu-bar-launchpad-visibility')),
              )
              .opacity,
          1,
        );
        expect(tester.takeException(), isNull);
        evidence['launchpadOriginalSceneSmoke'] = true;
      } else {
        evidence['launchpadOriginalSceneSmoke'] = 'not part of frozen snapshot';
      }
      evidence['mainViewId'] = mainId;
      evidence['dockViewId'] = dockId;
      evidence['sharedControllerIdentity'] = identityHashCode(controller);
      evidence['serializedMinimizeTargetCalls'] = ipc.actions;
      evidence['outputOwnershipReads'] = ipc.outputReads;
      evidence['appCatalogueLoads'] = apps.loads;
      evidence['passed'] = true;
      binding.reportData = evidence;
    } finally {
      evidence['serializedMinimizeTargetCalls'] = List.of(ipc.actions);
      evidence['outputOwnershipReads'] = ipc.outputReads;
      evidence['targetsBeforeUnmount'] = {
        for (final entry in controller.targets.entries)
          '${entry.key}': _rect(entry.value),
      };
      evidence['nativeBeforeUnmount'] = await _state();
      evidence['connectedBeforeUnmount'] = controller.connected;
      evidence['canMinimizeBeforeUnmount'] = controller.canMinimize;
      evidence['desktopRevisionBeforeUnmount'] = controller.desktopRevision;
      evidence['dockAppsBeforeUnmount'] = [
        for (final app in controller.dockApps)
          {
            'id': app.id,
            'windows': [for (final window in app.windows) window.id],
          },
      ];
      for (final pair in activePointers.toList()) {
        await remove(pair);
      }
      await tester.pumpWidget(
        const ViewCollection(views: []),
        wrapWithView: false,
      );
      await _window.invokeMethod<void>('setExpanded', false);
      await _window.invokeMethod<void>('setShellStyle', 'material');
      evidence['ipcDisposalsAfterUnmount'] = ipc.disposals;
      evidence['deliveredPhysicalDeviceEvents'] = filter.deliveredDeviceEvents;
      evidence['ignoredUnattachedDeviceEvents'] = filter.ignored;
      await File(
        '${captures.path}/mac-shell-configuration-native-evidence.json',
      ).writeAsString(
        '${const JsonEncoder.withIndent('  ').convert(evidence)}\n',
      );
      expect(ipc.disposals, 1);
    }
  }, timeout: const Timeout(Duration(minutes: 10)));
  tearDownAll(() {
    binding.deviceEventDispatcher = null;
    filter.restore();
  });
}
