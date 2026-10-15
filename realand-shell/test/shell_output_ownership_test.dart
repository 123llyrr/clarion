// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:realand_shell/app_menu_models.dart';
import 'package:realand_shell/models.dart';
import 'package:realand_shell/services/desktop_apps.dart';
import 'package:realand_shell/services/global_app_menu_service.dart';
import 'package:realand_shell/services/realand_ipc.dart';
import 'package:realand_shell/services/system_service.dart';
import 'package:realand_shell/services/wallpaper_service.dart';
import 'package:realand_shell/state/shell_controller.dart';

const _firstGeometry = <String, dynamic>{
  'x': 1920,
  'y': -120,
  'width': 1280,
  'height': 720,
};
const _secondGeometry = <String, dynamic>{
  'x': 0,
  'y': 0,
  'width': 1600,
  'height': 900,
};

Map<String, dynamic> _outputs(String name, Map<String, dynamic> logical) => {
  name: {'name': name, 'logical': logical},
};

class _Ipc extends RealandIpc {
  final connection = StreamController<bool>.broadcast(sync: true);
  final event = StreamController<Map<String, dynamic>>.broadcast(sync: true);
  final actions = <Map<String, Object?>>[];
  Map<String, dynamic> outputs = _outputs('OWNED-A', _firstGeometry);
  int reads = 0;
  Completer<dynamic>? nextResponse;
  @override
  Stream<bool> get connections => connection.stream;
  @override
  Stream<Map<String, dynamic>> get events => event.stream;
  @override
  void start() {}
  @override
  Future<dynamic> request(Object request) async {
    expect(request, 'Outputs');
    reads++;
    final gate = nextResponse;
    nextResponse = null;
    return gate == null ? {'Outputs': outputs} : await gate.future;
  }

  @override
  Future<void> action(
    String name, [
    Map<String, Object?> arguments = const {},
  ]) async {
    expect(name, 'SetMinimizeTarget');
    actions.add(Map.of(arguments));
  }

  void window() => event.add({
    'WindowsChanged': {
      'windows': [
        {
          'id': 91,
          'app_id': 'fixture',
          'is_focused': true,
          'is_minimized': false,
        },
      ],
    },
  });
  @override
  void dispose() {
    unawaited(connection.close());
    unawaited(event.close());
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

class _Apps extends DesktopApps {
  @override
  Future<List<DesktopApp>> load() async => const [];
}

class _System extends SystemService {
  @override
  Future<SystemSnapshot> read() async => const SystemSnapshot();
}

class _Wallpapers extends WallpaperService {
  @override
  Future<void> restore({bool applySaved = false}) async {}
}

Future<ShellController> _fixture(_Ipc ipc) async {
  final controller = ShellController(
    ipc: ipc,
    appMenuService: _Menus(),
    desktopApps: _Apps(),
    system: _System(),
    wallpaperService: _Wallpapers(),
    notifications: false,
    tray: false,
    powerProfilesEnabled: false,
    restoreWallpaperOnStart: false,
  )..setShellStyle(ShellStyle.macos);
  addTearDown(controller.dispose);
  await controller.start();
  ipc.connection.add(true);
  ipc.window();
  await _drain();
  return controller;
}

Future<void> _drain() async {
  for (var step = 0; step < 8; step++) {
    await Future<void>.delayed(Duration.zero);
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  const channel = MethodChannel('realand.shell/window');
  String? nativeName;
  Map<String, dynamic>? geometry;
  int stateReads = 0;
  setUp(() {
    nativeName = null;
    geometry = Map.of(_firstGeometry);
    stateReads = 0;
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, (call) async {
          if (call.method == 'outputName') return nativeName;
          if (call.method == 'windowState') {
            stateReads++;
            return {'monitorGeometry': geometry};
          }
          return null;
        });
  });
  tearDown(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, null);
  });
  const target = Rect.fromLTWH(24, 640, 48, 48);

  test(
    'native output name takes priority without enumerating guessed geometry',
    () async {
      nativeName = 'NATIVE-OUTPUT';
      geometry = null;
      final ipc = _Ipc();
      final controller = await _fixture(ipc);
      await controller.setDockMinimizeTargets({91: target});
      expect((ipc.actions.last['target'] as Map)['output'], 'NATIVE-OUTPUT');
      expect(ipc.reads, 0);
      expect(stateReads, 0);
    },
  );

  test(
    'exact unique logical geometry selects output, independent of focused workspace',
    () async {
      final ipc = _Ipc();
      ipc.outputs.addAll(_outputs('FOCUSED-OTHER', _secondGeometry));
      final controller = await _fixture(ipc);
      ipc.event.add({
        'WorkspacesChanged': {
          'workspaces': [
            {
              'id': 1,
              'idx': 1,
              'output': 'FOCUSED-OTHER',
              'is_focused': true,
              'is_active': true,
            },
          ],
        },
      });
      await _drain();
      await controller.setDockMinimizeTargets({91: target});
      expect(ipc.actions.last['target'], {
        'output': 'OWNED-A',
        'x': 24.0,
        'y': 640.0,
        'width': 48.0,
        'height': 48.0,
        'edge': 'bottom',
        'effect': 'genie',
      });
      expect(ipc.reads, greaterThan(0));
    },
  );

  test(
    'nonmatching and equal mirrored rectangles both safely clear targets',
    () async {
      final ipc = _Ipc();
      final controller = await _fixture(ipc);
      await controller.setDockMinimizeTargets({91: target});
      expect((ipc.actions.last['target'] as Map)['output'], 'OWNED-A');
      ipc.outputs = _outputs('WRONG-SIZE', _secondGeometry);
      controller.outputGeometryChanged();
      await _drain();
      await controller.setDockMinimizeTargets({91: target});
      expect(ipc.actions.last['target'], isNull);
      ipc.outputs = {
        ..._outputs('MIRROR-A', _firstGeometry),
        ..._outputs('MIRROR-B', _firstGeometry),
      };
      controller.outputGeometryChanged();
      await _drain();
      await controller.setDockMinimizeTargets({91: target});
      expect(ipc.actions.last['target'], isNull);
    },
  );

  test(
    'missing, nonfinite and disabled output geometry do not infer a connector',
    () async {
      final ipc = _Ipc();
      geometry = {'x': 0, 'y': 0, 'width': double.nan, 'height': 720};
      final controller = await _fixture(ipc);
      await controller.setDockMinimizeTargets({91: target});
      expect(ipc.actions.lastOrNull?['target'], isNull);
      expect(ipc.reads, 0);
      geometry = Map.of(_firstGeometry);
      ipc.outputs = {
        'DISABLED': {'name': 'DISABLED', 'logical': null},
      };
      controller.outputGeometryChanged();
      await _drain();
      await controller.setDockMinimizeTargets({91: target});
      expect(ipc.actions.lastOrNull?['target'], isNull);
    },
  );

  test(
    'late ownership for old geometry retries current output and cannot restore old target',
    () async {
      final ipc = _Ipc();
      final controller = await _fixture(ipc);
      await controller.setDockMinimizeTargets({91: target});
      final gate = ipc.nextResponse = Completer<dynamic>();
      final reads = ipc.reads;
      controller.outputGeometryChanged();
      await _drain();
      expect(ipc.reads, reads + 1);
      geometry = Map.of(_secondGeometry);
      ipc.outputs = _outputs('OWNED-B', _secondGeometry);
      controller.outputGeometryChanged();
      await controller.setDockMinimizeTargets({91: target});
      expect(
        ipc.actions.last['target'],
        isNull,
        reason:
            'new geometry cannot use old connector while ownership is pending',
      );
      gate.complete({'Outputs': _outputs('OWNED-A', _firstGeometry)});
      await _drain();
      expect(ipc.reads, greaterThan(reads + 1));
      await controller.setDockMinimizeTargets({91: target});
      expect((ipc.actions.last['target'] as Map)['output'], 'OWNED-B');
    },
  );

  test(
    'disconnect discards in-flight ownership and reconnect identifies a fresh output',
    () async {
      final ipc = _Ipc();
      final controller = await _fixture(ipc);
      final gate = ipc.nextResponse = Completer<dynamic>();
      controller.outputGeometryChanged();
      await _drain();
      ipc.connection.add(false);
      geometry = Map.of(_secondGeometry);
      ipc.outputs = _outputs('RECONNECTED-B', _secondGeometry);
      ipc.connection.add(true);
      ipc.window();
      gate.complete({'Outputs': _outputs('STALE-A', _firstGeometry)});
      await _drain();
      await controller.setDockMinimizeTargets({91: target});
      expect((ipc.actions.last['target'] as Map)['output'], 'RECONNECTED-B');
      expect(controller.connected, isTrue);
      expect(controller.windows.single.id, 91);
    },
  );
}
