// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/services/desktop_apps.dart';
import 'package:clarion_shell/services/clarion_ipc.dart';
import 'package:clarion_shell/services/system_service.dart';
import 'package:clarion_shell/services/wallpaper_service.dart';
import 'package:clarion_shell/state/shell_controller.dart';

class _System extends SystemService {
  var snapshot = const SystemSnapshot(
    volume: .4,
    brightness: .5,
    wifiEnabled: true,
    bluetoothEnabled: true,
    networkName: 'Fixture WiFi',
  );
  var reads = 0;
  final writes = <(String, bool)>[];
  final failedControls = <String>{};
  final writeGates = <String, Completer<void>>{};
  var muteWrites = 0;
  Completer<SystemSnapshot>? nextRead;

  void _patch({
    double? volume,
    double? brightness,
    bool? muted,
    bool? wifi,
    bool? bluetooth,
    String? networkName,
  }) {
    snapshot = SystemSnapshot(
      volume: volume ?? snapshot.volume,
      brightness: brightness ?? snapshot.brightness,
      muted: muted ?? snapshot.muted,
      wifiEnabled: wifi ?? snapshot.wifiEnabled,
      bluetoothEnabled: bluetooth ?? snapshot.bluetoothEnabled,
      networkName: networkName ?? snapshot.networkName,
    );
  }

  @override
  Future<SystemSnapshot> read() async {
    reads++;
    final gate = nextRead;
    nextRead = null;
    return gate == null ? snapshot : gate.future;
  }

  @override
  Future<void> setWifi(bool value) async {
    writes.add(('wifi', value));
    await writeGates['wifi']?.future;
    if (failedControls.contains('wifi')) throw StateError('wifi failed');
    _patch(wifi: value, networkName: value ? 'Fixture WiFi' : '');
  }

  @override
  Future<void> setBluetooth(bool value) async {
    writes.add(('bluetooth', value));
    await writeGates['bluetooth']?.future;
    if (failedControls.contains('bluetooth')) {
      throw StateError('bluetooth failed');
    }
    _patch(bluetooth: value);
  }

  @override
  Future<void> setVolume(double value) async => _patch(volume: value);
  @override
  Future<void> setBrightness(double value) async => _patch(brightness: value);
  @override
  Future<void> toggleMute() async {
    muteWrites++;
    _patch(muted: !snapshot.muted);
  }
}

class _Ipc extends ClarionIpc {
  @override
  Future<dynamic> request(Object request) async => null;
  final connectionEvents = StreamController<bool>.broadcast(sync: true);
  final desktopEvents = StreamController<Map<String, dynamic>>.broadcast(
    sync: true,
  );

  @override
  Stream<bool> get connections => connectionEvents.stream;
  @override
  Stream<Map<String, dynamic>> get events => desktopEvents.stream;
  @override
  void start() {}
  @override
  void dispose() {
    unawaited(connectionEvents.close());
    unawaited(desktopEvents.close());
    super.dispose();
  }
}

class _Apps extends DesktopApps {
  @override
  Future<List<DesktopApp>> load() async => const [];
}

class _Wallpapers extends WallpaperService {
  @override
  Future<void> restore({bool applySaved = false}) async {}
}

class _ControllerLifetime extends StatefulWidget {
  const _ControllerLifetime(this.controller);
  final ShellController controller;

  @override
  State<_ControllerLifetime> createState() => _ControllerLifetimeState();
}

class _ControllerLifetimeState extends State<_ControllerLifetime> {
  @override
  Widget build(BuildContext context) => const SizedBox.shrink();
  @override
  void dispose() {
    widget.controller.dispose();
    super.dispose();
  }
}

class _Fixture {
  _Fixture({this.demo = false});
  final bool demo;
  final system = _System();
  final ipc = _Ipc();
  late final controller = ShellController(
    demo: demo,
    notifications: false,
    tray: false,
    powerProfilesEnabled: false,
    ipc: ipc,
    system: system,
    desktopApps: _Apps(),
    wallpaperService: _Wallpapers(),
  );
}

Future<_Fixture> _start(
  WidgetTester tester, {
  bool demo = false,
  SystemSnapshot? snapshot,
}) async {
  final fixture = _Fixture(demo: demo);
  if (snapshot != null) fixture.system.snapshot = snapshot;
  addTearDown(fixture.controller.dispose);
  await tester.pumpWidget(_ControllerLifetime(fixture.controller));
  final started = fixture.controller.start();
  await _drain(tester);
  await started;
  return fixture;
}

Future<void> _drain(WidgetTester tester) async {
  for (var i = 0; i < 8; i++) {
    await tester.pump();
  }
}

bool? _enabled(ShellController controller, String key) =>
    key == 'wifi' ? controller.wifiEnabled : controller.bluetoothEnabled;

Future<void> _toggle(ShellController controller, String key) =>
    key == 'wifi' ? controller.toggleWifi() : controller.toggleBluetooth();

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  setUp(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
          const MethodChannel('clarion.shell/window'),
          (_) async => null,
        );
  });
  tearDown(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
          const MethodChannel('clarion.shell/window'),
          null,
        );
  });

  for (final key in ['wifi', 'bluetooth']) {
    testWidgets(
      '$key keeps confirmed toggle when an older poll finishes later',
      (tester) async {
        final fixture = await _start(tester);
        final system = fixture.system;
        final controller = fixture.controller;
        final oldSnapshot = system.snapshot;
        final oldRead = Completer<SystemSnapshot>();
        system.nextRead = oldRead;
        await tester.pump(const Duration(seconds: 10));
        expect(system.reads, 2);
        final changed = _toggle(controller, key);
        await _drain(tester);
        await changed;
        expect(system.writes, [(key, false)]);
        expect(_enabled(controller, key), false);
        oldRead.complete(oldSnapshot);
        await _drain(tester);
        expect(_enabled(controller, key), false);
        await tester.pump(const Duration(milliseconds: 180));
        await _drain(tester);
        expect(system.reads, 3);
        expect(_enabled(controller, key), false);
        await _toggle(controller, key);
        expect(system.writes, [(key, false), (key, true)]);
      },
    );

    testWidgets(
      '$key releases busy without waiting for unrelated system reads',
      (tester) async {
        final fixture = await _start(tester);
        final system = fixture.system;
        final controller = fixture.controller;
        final pendingRead = Completer<SystemSnapshot>();
        system.nextRead = pendingRead;
        var operationFinished = false;
        unawaited(
          _toggle(controller, key).then((_) => operationFinished = true),
        );
        await _drain(tester);
        expect(system.writes, [(key, false)]);
        expect(operationFinished, true);
        expect(controller.busy.contains(key), false);
        expect(_enabled(controller, key), false);
        expect(system.reads, 1);
        await tester.pump(const Duration(milliseconds: 180));
        expect(system.reads, 2);
        pendingRead.complete(system.snapshot);
        await _drain(tester);
        expect(_enabled(controller, key), false);
      },
    );

    testWidgets('$key failure retains confirmed state and reconciles once', (
      tester,
    ) async {
      final fixture = await _start(tester);
      fixture.system.failedControls.add(key);
      final changed = _toggle(fixture.controller, key);
      await _drain(tester);
      await changed;
      expect(_enabled(fixture.controller, key), true);
      expect(fixture.controller.error, contains('$key failed'));
      expect(fixture.controller.busy, isEmpty);
      expect(fixture.system.reads, 1);
      await tester.pump(const Duration(milliseconds: 180));
      await _drain(tester);
      expect(fixture.system.reads, 2);
      expect(_enabled(fixture.controller, key), true);
    });

    testWidgets('$key unavailable service never sends a write', (tester) async {
      final fixture = await _start(tester, snapshot: const SystemSnapshot());
      await _toggle(fixture.controller, key);
      expect(_enabled(fixture.controller, key), isNull);
      expect(fixture.controller.error, contains('不可用'));
      expect(fixture.system.writes, isEmpty);
      expect(fixture.controller.busy, isEmpty);
    });
  }

  testWidgets(
    'independent radio commands and mute reconcile once after all writes',
    (tester) async {
      final fixture = await _start(tester);
      final controller = fixture.controller;
      final wifiGate = Completer<void>();
      fixture.system.writeGates['wifi'] = wifiGate;
      final wifi = controller.toggleWifi();
      expect(controller.busy.contains('wifi'), true);
      final bluetooth = controller.toggleBluetooth();
      final mute = controller.toggleMute();
      await _drain(tester);
      await Future.wait([bluetooth, mute]);
      expect(controller.bluetoothEnabled, false);
      expect(controller.muted, true);
      expect(controller.wifiEnabled, true);
      expect(controller.busy, {'wifi'});
      await controller.toggleWifi();
      expect(fixture.system.writes, [('wifi', false), ('bluetooth', false)]);
      await tester.pump(const Duration(milliseconds: 180));
      expect(fixture.system.reads, 1);
      wifiGate.complete();
      await _drain(tester);
      await wifi;
      expect(controller.wifiEnabled, false);
      expect(controller.busy, isEmpty);
      await tester.pump(const Duration(milliseconds: 180));
      await _drain(tester);
      expect(fixture.system.reads, 2);
      expect(controller.networkName, isEmpty);
      expect(controller.bluetoothEnabled, false);
      expect(controller.muted, true);
    },
  );

  testWidgets(
    'desktop revision ignores system controls, clock, panels and config errors',
    (tester) async {
      final fixture = await _start(tester);
      final controller = fixture.controller;
      final revision = controller.desktopRevision;
      controller.selectPanel(ShellPanel.quickSettings);
      final volume = controller.setVolume(.7);
      expect(controller.desktopRevision, revision);
      await tester.pump(const Duration(milliseconds: 90));
      await _drain(tester);
      await volume;
      final brightness = controller.setBrightness(.6);
      await tester.pump(const Duration(milliseconds: 90));
      await _drain(tester);
      await brightness;
      await controller.toggleMute();
      await controller.toggleWifi();
      await controller.toggleBluetooth();
      fixture.ipc.desktopEvents.add({
        'ConfigLoaded': {'failed': true},
      });
      await tester.pump(const Duration(seconds: 1));
      await _drain(tester);
      expect(controller.desktopRevision, revision);
      expect(controller.volume, .7);
      expect(controller.brightness, .6);
    },
  );

  testWidgets(
    'desktop revision covers IPC state changes while ignoring desktop no-ops',
    (tester) async {
      final fixture = await _start(tester);
      final controller = fixture.controller;
      void emit(Map<String, dynamic> event) =>
          fixture.ipc.desktopEvents.add(event);
      var expected = controller.desktopRevision;
      final workspaces = {
        'WorkspacesChanged': {
          'workspaces': [
            {
              'id': 1,
              'idx': 1,
              'output': 'DP-1',
              'is_active': true,
              'is_focused': true,
            },
            {
              'id': 2,
              'idx': 2,
              'output': 'DP-1',
              'is_active': false,
              'is_focused': false,
            },
          ],
        },
      };
      emit(workspaces);
      expect(controller.desktopRevision, ++expected);
      emit(workspaces);
      expect(controller.desktopRevision, expected);
      final window = {
        'id': 42,
        'workspace_id': 1,
        'title': 'Editor',
        'is_focused': true,
      };
      emit({
        'WindowsChanged': {
          'windows': [window],
        },
      });
      expect(controller.desktopRevision, ++expected);
      emit({
        'WindowOpenedOrChanged': {'window': window},
      });
      expect(controller.desktopRevision, expected);
      emit({
        'WindowOpenedOrChanged': {
          'window': {...window, 'title': 'Terminal'},
        },
      });
      expect(controller.desktopRevision, ++expected);
      emit({
        'WindowFocusChanged': {'id': null},
      });
      expect(controller.desktopRevision, ++expected);
      emit({
        'WindowFocusChanged': {'id': null},
      });
      expect(controller.desktopRevision, expected);
      emit({
        'WorkspaceActivated': {'id': 2, 'focused': true},
      });
      expect(controller.desktopRevision, ++expected);
      emit({
        'WorkspaceActivated': {'id': 2, 'focused': true},
      });
      emit({
        'WindowClosed': {'id': 991},
      });
      emit({
        'WorkspaceActivated': {'id': 991, 'focused': true},
      });
      emit({
        'WindowOpenedOrChanged': {
          'window': {'id': 'invalid'},
        },
      });
      emit({'FutureEvent': {}});
      expect(controller.desktopRevision, expected);
      emit({
        'WindowClosed': {'id': 42},
      });
      expect(controller.desktopRevision, ++expected);
      fixture.ipc.connectionEvents.add(false);
      expect(controller.desktopRevision, ++expected);
      expect(controller.workspaces, isEmpty);
      fixture.ipc.connectionEvents.add(false);
      expect(controller.desktopRevision, expected);
      fixture.ipc.connectionEvents.add(true);
      expect(controller.desktopRevision, ++expected);
      await _drain(tester);
    },
  );

  testWidgets(
    'output selection advances desktop revision when workspace snapshot stays equal',
    (tester) async {
      var outputName = 'DP-1';
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(
            const MethodChannel('clarion.shell/window'),
            (call) async => call.method == 'outputName' ? outputName : null,
          );
      final fixture = await _start(tester);
      final workspaces = {
        'WorkspacesChanged': {
          'workspaces': [
            {'id': 1, 'idx': 1, 'output': 'DP-1', 'is_focused': true},
            {'id': 2, 'idx': 1, 'output': 'DP-2'},
          ],
        },
      };
      fixture.ipc.desktopEvents.add(workspaces);
      await _drain(tester);
      expect(fixture.controller.workspaces.single.id, 1);
      final revision = fixture.controller.desktopRevision;
      outputName = 'DP-2';
      fixture.ipc.desktopEvents.add(workspaces);
      await _drain(tester);
      expect(fixture.controller.desktopRevision, revision + 1);
      expect(fixture.controller.workspaces.single.id, 2);
      fixture.ipc.desktopEvents.add(workspaces);
      await _drain(tester);
      expect(fixture.controller.desktopRevision, revision + 1);
    },
  );

  testWidgets(
    'demo workspace selection advances desktop revision only for changes',
    (tester) async {
      final fixture = await _start(tester, demo: true);
      final controller = fixture.controller;
      final revision = controller.desktopRevision;
      await controller.switchWorkspace(2);
      expect(controller.desktopRevision, revision + 1);
      expect(
        controller.workspaces
            .singleWhere((workspace) => workspace.isFocused)
            .id,
        2,
      );
      await controller.switchWorkspace(2);
      await controller.switchWorkspace(991);
      await controller.setVolume(.3);
      expect(controller.desktopRevision, revision + 1);
    },
  );
}
