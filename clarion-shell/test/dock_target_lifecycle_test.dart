// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/mac_shell_preferences.dart';
import 'package:clarion_shell/services/desktop_apps.dart';
import 'package:clarion_shell/services/clarion_ipc.dart';
import 'package:clarion_shell/services/system_service.dart';
import 'package:clarion_shell/services/wallpaper_service.dart';
import 'package:clarion_shell/state/shell_controller.dart';

class _Call {
  const _Call(this.session, this.arguments);
  final int session;
  final Map<String, Object?> arguments;
}

/// Each delayed response stays associated with the session that sent it.
/// Nothing in this fixture opens a Wayland, IPC, or D-Bus connection.
class _Ipc extends ClarionIpc {
  final _connections = StreamController<bool>.broadcast(sync: true);
  final _events = StreamController<Map<String, dynamic>>.broadcast(sync: true);
  final calls = <_Call>[];
  int _session = 0;
  Completer<void>? _nextGate;

  @override
  Stream<bool> get connections => _connections.stream;
  @override
  Stream<Map<String, dynamic>> get events => _events.stream;
  @override
  void start() {}

  Completer<void> gateNext() {
    expect(_nextGate, isNull);
    return _nextGate = Completer<void>();
  }

  void connect() {
    _session++;
    _connections.add(true);
  }

  void disconnect() => _connections.add(false);

  void window() => _events.add({
    'WindowsChanged': {
      'windows': [
        {
          'id': 91,
          'app_id': 'org.test.Editor',
          'title': 'Document',
          'workspace_id': 1,
          'is_focused': true,
          'is_minimized': false,
        },
      ],
    },
  });

  @override
  Future<void> action(
    String name, [
    Map<String, Object?> arguments = const {},
  ]) async {
    expect(name, 'SetMinimizeTarget');
    calls.add(_Call(_session, Map.of(arguments)));
    final gate = _nextGate;
    _nextGate = null;
    await gate?.future;
  }

  @override
  void dispose() {
    unawaited(_connections.close());
    unawaited(_events.close());
    super.dispose();
  }
}

class _Apps extends DesktopApps {
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

Future<(ShellController, _Ipc)> _fixture() async {
  final ipc = _Ipc();
  final controller = ShellController(
    notifications: false,
    tray: false,
    powerProfilesEnabled: false,
    restoreWallpaperOnStart: false,
    ipc: ipc,

    desktopApps: _Apps(),
    system: _System(),
    wallpaperService: _Wallpapers(),
  )..setShellStyle(ShellStyle.macos);
  addTearDown(controller.dispose);
  await controller.start();
  ipc.connect();
  ipc.window();
  expect(controller.canMinimize, isTrue);
  expect(ipc.calls, isEmpty);
  return (controller, ipc);
}

Future<void> _drain() async {
  // Fake actions only await explicitly controlled gates. Drain finite followup
  // microtasks; no wall-clock retry, real socket, or external process is used.
  for (var tick = 0; tick < 6; tick++) {
    await Future<void>.delayed(Duration.zero);
  }
}

void _expectTarget(
  _Call call,
  Rect rect, {
  bool reducedMotion = false,
  MacMinimizeEffect effect = MacMinimizeEffect.genie,
  MacDockPosition edge = MacDockPosition.bottom,
}) {
  expect(call.arguments, {
    'id': 91,
    'target': {
      'output': 'HDMI-A-1',
      'x': rect.left,
      'y': rect.top,
      'width': rect.width,
      'height': rect.height,
      'edge': edge.name,
      'effect': effect.name,
    },
    'reduce_motion': reducedMotion,
  });
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  const channel = MethodChannel('clarion.shell/window');
  const first = Rect.fromLTWH(20, 400, 40, 40);
  const second = Rect.fromLTWH(70, 405, 48, 48);

  setUp(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
          channel,
          (call) async => call.method == 'outputName' ? 'HDMI-A-1' : null,
        );
  });
  tearDown(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, null);
  });

  test(
    'pending first target survives a same-frame Material then Mac switch',
    () async {
      final (controller, ipc) = await _fixture();
      final gate = ipc.gateNext();
      final pending = controller.setDockMinimizeTargets({91: first});
      expect(ipc.calls, hasLength(1));

      // No intermediate widget frame can recreate the Dock or report its rect.
      controller.setShellStyle(ShellStyle.material);
      controller.setShellStyle(ShellStyle.macos);
      gate.complete();
      await pending;
      await _drain();

      expect(controller.shellStyle, ShellStyle.macos);
      _expectTarget(ipc.calls.last, first);
    },
  );

  test('A then delayed B then A resends the final A after B was sent', () async {
    final (controller, ipc) = await _fixture();
    await controller.setDockMinimizeTargets({91: first});
    _expectTarget(ipc.calls.single, first);
    final gate = ipc.gateNext();
    final pending = controller.setDockMinimizeTargets({91: second});
    expect(ipc.calls, hasLength(2));
    _expectTarget(ipc.calls.last, second);

    await controller.setDockMinimizeTargets({91: first});
    gate.complete();
    await pending;
    await _drain();

    expect(
      ipc.calls.length,
      greaterThanOrEqualTo(3),
      reason:
          'B reached the compositor, so an old cached A cannot prove the final wire state is A',
    );
    _expectTarget(ipc.calls.last, first);
  });

  test(
    'late initial reply preserves the latest reduce-motion flag and target',
    () async {
      final (controller, ipc) = await _fixture();
      final gate = ipc.gateNext();
      final pending = controller.setDockMinimizeTargets({91: first});
      _expectTarget(ipc.calls.single, first);

      controller.setReduceMotion(true);
      gate.complete();
      await pending;
      await _drain();

      expect(controller.reduceMotion, isTrue);
      _expectTarget(ipc.calls.last, first, reducedMotion: true);
    },
  );

  test(
    'changing only the effect retains and republishes the visible destination',
    () async {
      final (controller, ipc) = await _fixture();
      await controller.setDockMinimizeTargets({91: first});
      controller.setMacPreferences(
        controller.macPreferences.copyWith(
          minimizeEffect: MacMinimizeEffect.scale,
        ),
      );
      await _drain();
      expect(ipc.calls, hasLength(2));
      _expectTarget(ipc.calls.last, first, effect: MacMinimizeEffect.scale);
      expect(controller.shellStyle, ShellStyle.macos);
    },
  );

  test('late effect reply cannot swallow a newer return to Genie', () async {
    final (controller, ipc) = await _fixture();
    await controller.setDockMinimizeTargets({91: first});
    final gate = ipc.gateNext();
    controller.setMacPreferences(
      controller.macPreferences.copyWith(
        minimizeEffect: MacMinimizeEffect.scale,
      ),
    );
    expect(ipc.calls, hasLength(2));
    _expectTarget(ipc.calls.last, first, effect: MacMinimizeEffect.scale);
    controller.setMacPreferences(
      controller.macPreferences.copyWith(
        minimizeEffect: MacMinimizeEffect.genie,
      ),
    );
    gate.complete();
    await _drain();
    expect(ipc.calls.length, greaterThanOrEqualTo(3));
    _expectTarget(ipc.calls.last, first);
  });

  test(
    'false then delayed true then false resends the final motion policy',
    () async {
      final (controller, ipc) = await _fixture();
      await controller.setDockMinimizeTargets({91: first});
      final gate = ipc.gateNext();
      controller.setReduceMotion(true);
      expect(ipc.calls, hasLength(2));
      _expectTarget(ipc.calls.last, first, reducedMotion: true);

      controller.setReduceMotion(false);
      gate.complete();
      await _drain();

      expect(controller.reduceMotion, isFalse);
      expect(ipc.calls.length, greaterThanOrEqualTo(3));
      _expectTarget(ipc.calls.last, first);
    },
  );

  test(
    'disconnect discards a late old-session reply and re-registers equal geometry',
    () async {
      final (controller, ipc) = await _fixture();
      final gate = ipc.gateNext();
      final oldSession = controller.setDockMinimizeTargets({91: first});
      expect(ipc.calls.single.session, 1);
      ipc.disconnect();
      gate.complete();
      await oldSession;
      await _drain();
      expect(controller.windows, isEmpty);

      // IDs and geometry may repeat after a new compositor snapshot. Neither
      // proves that this session has received a target registration already.
      ipc.connect();
      ipc.window();
      // Native output ownership is revalidated asynchronously on reconnect.
      // A widget reports fresh geometry after that revision, just as the live
      // Dock does. Coordinates sent before it completes are safely cleared.
      await _drain();
      await controller.setDockMinimizeTargets({91: first});
      await _drain();

      final currentSession = ipc.calls
          .where((call) => call.session == 2)
          .toList();
      expect(currentSession, isNotEmpty);
      _expectTarget(currentSession.last, first);
    },
  );

  test(
    'output geometry invalidation clears late targets until fresh coordinates arrive',
    () async {
      final (controller, ipc) = await _fixture();
      await controller.setDockMinimizeTargets({91: first});
      final gate = ipc.gateNext();
      final pending = controller.setDockMinimizeTargets({91: second});
      _expectTarget(ipc.calls.last, second);
      final oldCalls = ipc.calls.length;
      final desktopRevision = controller.desktopRevision;

      // The old origin is invalid even if the connector and both view sizes
      // remain unchanged. An already-sent action may still acknowledge later.
      controller.outputGeometryChanged();
      expect(controller.desktopRevision, greaterThan(desktopRevision));
      controller.setShellStyle(ShellStyle.material);
      controller.setShellStyle(ShellStyle.macos);
      gate.complete();
      await pending;
      await _drain();

      expect(controller.shellStyle, ShellStyle.macos);
      expect(ipc.calls.length, greaterThan(oldCalls));
      expect(ipc.calls.last.arguments['target'], isNull);
      for (final call in ipc.calls.skip(oldCalls)) {
        expect(
          call.arguments['target'],
          isNull,
          reason:
              'Neither a late reply nor a style switch restores old origins',
        );
      }

      // A later style toggle must also leave targets cleared while native
      // geometry is unavailable, rather than replaying the old desired map.
      final clearedCalls = ipc.calls.length;
      controller.setShellStyle(ShellStyle.material);
      controller.setShellStyle(ShellStyle.macos);
      await _drain();
      for (final call in ipc.calls.skip(clearedCalls)) {
        expect(call.arguments['target'], isNull);
      }

      const fresh = Rect.fromLTWH(120, 500, 40, 40);
      final beforeFresh = ipc.calls.length;
      await controller.setDockMinimizeTargets({91: fresh});
      await _drain();
      expect(ipc.calls.length, greaterThan(beforeFresh));
      _expectTarget(ipc.calls.last, fresh);
    },
  );
}
