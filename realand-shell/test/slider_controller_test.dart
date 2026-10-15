// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:realand_shell/models.dart';
import 'package:realand_shell/services/desktop_apps.dart';
import 'package:realand_shell/services/realand_ipc.dart';
import 'package:realand_shell/services/system_service.dart';
import 'package:realand_shell/services/wallpaper_service.dart';
import 'package:realand_shell/state/shell_controller.dart';

class _System extends SystemService {
  var snapshot = const SystemSnapshot(volume: .4, brightness: .5);
  var reads = 0;
  var volumeActive = 0;
  var brightnessActive = 0;
  var maximumVolumeActive = 0;
  var maximumBrightnessActive = 0;
  final volumeWrites = <double>[];
  final brightnessWrites = <double>[];
  Completer<SystemSnapshot>? nextRead;
  Completer<void>? volumeGate;
  Completer<void>? brightnessGate;
  bool failVolume = false;
  final failedVolumeValues = <double>{};
  var muteWrites = 0;

  @override
  Future<SystemSnapshot> read() async {
    reads++;
    final pending = nextRead;
    nextRead = null;
    return pending == null ? snapshot : pending.future;
  }

  @override
  Future<void> setVolume(double value) async {
    volumeWrites.add(value);
    volumeActive++;
    maximumVolumeActive = volumeActive > maximumVolumeActive
        ? volumeActive
        : maximumVolumeActive;
    try {
      await volumeGate?.future;
      if (failVolume || failedVolumeValues.contains(value)) {
        throw StateError('volume failed');
      }
      snapshot = SystemSnapshot(
        volume: value,
        brightness: snapshot.brightness,
        muted: snapshot.muted,
      );
    } finally {
      volumeActive--;
    }
  }

  @override
  Future<void> setBrightness(double value) async {
    brightnessWrites.add(value);
    brightnessActive++;
    maximumBrightnessActive = brightnessActive > maximumBrightnessActive
        ? brightnessActive
        : maximumBrightnessActive;
    try {
      await brightnessGate?.future;
      snapshot = SystemSnapshot(
        volume: snapshot.volume,
        brightness: value,
        muted: snapshot.muted,
      );
    } finally {
      brightnessActive--;
    }
  }

  @override
  Future<void> toggleMute() async {
    muteWrites++;
    snapshot = SystemSnapshot(
      volume: snapshot.volume,
      brightness: snapshot.brightness,
      muted: !snapshot.muted,
    );
  }
}

class _Ipc extends RealandIpc {
  @override
  void start() {}
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

Future<ShellController> _start(WidgetTester tester, _System system) async {
  final controller = ShellController(
    notifications: false,
    tray: false,
    powerProfilesEnabled: false,
    ipc: _Ipc(),
    desktopApps: _Apps(),
    wallpaperService: _Wallpapers(),
    system: system,
  );
  addTearDown(controller.dispose);
  // Flutter removes the test tree before asserting that no timers survive.
  // Also keep addTearDown for failures while the fixture is being created.
  await tester.pumpWidget(_ControllerLifetime(controller));
  final started = controller.start();
  await _drain(tester);
  await started;
  return controller;
}

Future<void> _drain(WidgetTester tester) async {
  for (var i = 0; i < 8; i++) {
    await tester.pump();
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  setUp(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
          const MethodChannel('realand.shell/window'),
          (_) async => null,
        );
  });
  tearDown(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
          const MethodChannel('realand.shell/window'),
          null,
        );
  });

  testWidgets('real hardware sliders reflect each input before commands run', (
    tester,
  ) async {
    final system = _System();
    final controller = await _start(tester, system);
    unawaited(controller.setVolume(.73));
    unawaited(controller.setBrightness(.86));
    expect(controller.volume, .73);
    expect(controller.brightness, .86);
    expect(system.volumeWrites, isEmpty);
    expect(system.brightnessWrites, isEmpty);
    await tester.pump(const Duration(milliseconds: 90));
    await _drain(tester);
    expect(system.volumeWrites, [.73]);
    expect(system.brightnessWrites, [.86]);
  });

  testWidgets('unchanged system polls do not rebuild the shell', (
    tester,
  ) async {
    final system = _System();
    final controller = await _start(tester, system);
    var notifications = 0;
    controller.addListener(() => notifications++);

    await tester.pump(const Duration(seconds: 10));
    await _drain(tester);
    expect(system.reads, 2);
    expect(notifications, 0);

    system.snapshot = const SystemSnapshot(
      volume: .4,
      brightness: .5,
      muted: true,
    );
    await tester.pump(const Duration(seconds: 10));
    await _drain(tester);
    expect(controller.muted, true);
    expect(notifications, 1);
  });

  testWidgets(
    'an old system poll cannot pull a newer volume gesture backward',
    (tester) async {
      final system = _System();
      final controller = await _start(tester, system);
      final oldRead = Completer<SystemSnapshot>();
      system.nextRead = oldRead;
      await tester.pump(const Duration(seconds: 10));
      expect(system.reads, 2);
      final write = controller.setVolume(.9);
      await tester.pump(const Duration(milliseconds: 90));
      await _drain(tester);
      await write;
      expect(controller.volume, .9);
      oldRead.complete(const SystemSnapshot(volume: .4, brightness: .55));
      await _drain(tester);
      expect(controller.volume, .9);
      expect(controller.brightness, .55);
    },
  );

  testWidgets(
    'gesture writes coalesce and release flushes without 90 ms delay',
    (tester) async {
      final system = _System();
      final controller = await _start(tester, system);
      controller.beginVolumeAdjustment();
      final pending = [
        controller.setVolume(.6),
        controller.setVolume(.7),
        controller.setVolume(.8),
      ];
      expect(controller.volume, .8);
      final finished = controller.endVolumeAdjustment(.8);
      expect(system.volumeWrites, [.8]);
      await _drain(tester);
      await Future.wait([...pending, finished]);
      expect(system.reads, 1);
      await tester.pump(const Duration(milliseconds: 180));
      await _drain(tester);
      expect(system.reads, 2);
      expect(controller.volume, .8);
    },
  );

  testWidgets(
    'a slow volume command keeps only newest value without blocking brightness',
    (tester) async {
      final system = _System()..volumeGate = Completer<void>();
      final controller = await _start(tester, system);
      controller.beginVolumeAdjustment();
      final first = controller.setVolume(.6);
      await tester.pump(const Duration(milliseconds: 90));
      expect(system.volumeWrites, [.6]);
      final middle = controller.setVolume(.7);
      final last = controller.setVolume(.9);
      final volumeDone = controller.endVolumeAdjustment(.9);
      controller.beginBrightnessAdjustment();
      final brightness = controller.setBrightness(.2);
      final brightnessDone = controller.endBrightnessAdjustment(.2);
      await _drain(tester);
      await Future.wait([brightness, brightnessDone]);
      expect(system.brightnessWrites, [.2]);
      expect(system.volumeWrites, [.6]);
      expect(controller.volume, .9);
      system.volumeGate!.complete();
      await _drain(tester);
      await Future.wait([first, middle, last, volumeDone]);
      expect(system.volumeWrites, [.6, .9]);
      expect(system.maximumVolumeActive, 1);
      expect(system.maximumBrightnessActive, 1);
      expect(controller.volume, .9);
      expect(controller.brightness, .2);
    },
  );

  testWidgets('dragging never performs a full system read between writes', (
    tester,
  ) async {
    final system = _System();
    final controller = await _start(tester, system);
    controller.beginVolumeAdjustment();
    for (final value in [.6, .7, .8]) {
      final applied = controller.setVolume(value);
      await tester.pump(const Duration(milliseconds: 90));
      await _drain(tester);
      await applied;
      await tester.pump(const Duration(milliseconds: 200));
    }
    expect(system.volumeWrites, [.6, .7, .8]);
    expect(system.reads, 1);
    await controller.endVolumeAdjustment(.8);
    await tester.pump(const Duration(milliseconds: 180));
    await _drain(tester);
    expect(system.reads, 2);
  });

  testWidgets('a long drag defers periodic system polling until release', (
    tester,
  ) async {
    final system = _System();
    final controller = await _start(tester, system);
    controller.beginBrightnessAdjustment();
    final applied = controller.setBrightness(.8);
    await tester.pump(const Duration(milliseconds: 90));
    await _drain(tester);
    await applied;
    await tester.pump(const Duration(seconds: 20));
    await _drain(tester);
    expect(system.reads, 1);
    expect(controller.brightness, .8);

    await controller.endBrightnessAdjustment(.8);
    await tester.pump(const Duration(milliseconds: 180));
    await _drain(tester);
    expect(system.reads, 2);
  });

  testWidgets(
    'a slow command applies the latest drag value without extra delay',
    (tester) async {
      final system = _System()..volumeGate = Completer<void>();
      final controller = await _start(tester, system);
      controller.beginVolumeAdjustment();
      final first = controller.setVolume(.6);
      await tester.pump(const Duration(milliseconds: 90));
      final middle = controller.setVolume(.7);
      final last = controller.setVolume(.9);
      await tester.pump(const Duration(milliseconds: 200));
      expect(system.volumeWrites, [.6]);

      system.volumeGate!.complete();
      await _drain(tester);
      expect(system.volumeWrites, [.6, .9]);
      await Future.wait([first, middle, last]);
      expect(system.maximumVolumeActive, 1);
      expect(controller.volume, .9);
      await controller.endVolumeAdjustment(.9);
    },
  );

  testWidgets(
    'a slow whole-system read never holds up subsequent slider commands',
    (tester) async {
      final system = _System();
      final controller = await _start(tester, system);
      final readGate = Completer<SystemSnapshot>();
      system.nextRead = readGate;
      final first = controller.setVolume(.6);
      await tester.pump(const Duration(milliseconds: 90));
      await _drain(tester);
      await first;
      await tester.pump(const Duration(milliseconds: 180));
      expect(system.reads, 2);
      final second = controller.setVolume(.9);
      await tester.pump(const Duration(milliseconds: 90));
      await _drain(tester);
      await second;
      expect(system.volumeWrites, [.6, .9]);
      expect(controller.volume, .9);
      readGate.complete(const SystemSnapshot(volume: .6, brightness: .5));
      await _drain(tester);
      expect(controller.volume, .9);
    },
  );

  testWidgets(
    'a final failed command restores confirmed value and reports error',
    (tester) async {
      final system = _System()..failVolume = true;
      final controller = await _start(tester, system);
      controller.beginVolumeAdjustment();
      final applied = controller.setVolume(.9);
      expect(controller.volume, .9);
      final finished = controller.endVolumeAdjustment(.9);
      await _drain(tester);
      await Future.wait([applied, finished]);
      expect(controller.volume, .4);
      expect(controller.error, contains('volume failed'));
      expect(controller.busy, isEmpty);
    },
  );

  testWidgets('failure of a superseded command never rolls back newer input', (
    tester,
  ) async {
    final system = _System()
      ..volumeGate = Completer<void>()
      ..failedVolumeValues.add(.6);
    final controller = await _start(tester, system);
    controller.beginVolumeAdjustment();
    final first = controller.setVolume(.6);
    await tester.pump(const Duration(milliseconds: 90));
    final last = controller.setVolume(.9);
    final finished = controller.endVolumeAdjustment(.9);
    expect(controller.volume, .9);
    system.volumeGate!.complete();
    await _drain(tester);
    await Future.wait([first, last, finished]);
    expect(controller.volume, .9);
    expect(controller.error, isNull);
  });

  for (final destination in ['closed', 'tray items', 'tray menu']) {
    testWidgets(
      'leaving control center for $destination flushes drag and resumes external updates',
      (tester) async {
        final system = _System();
        final controller = await _start(tester, system);
        controller.selectPanel(ShellPanel.quickSettings);
        controller.beginVolumeAdjustment();
        final pending = controller.setVolume(.8);
        switch (destination) {
          case 'closed':
            controller.closePanel();
          case 'tray items':
            controller.openTrayItems();
          case 'tray menu':
            unawaited(
              controller.openTrayMenu(
                const TrayItem(id: 'fixture', title: 'Fixture'),
              ),
            );
        }
        expect(system.volumeWrites, [.8]);
        await _drain(tester);
        await pending;
        system.snapshot = const SystemSnapshot(volume: .2, brightness: .5);
        await tester.pump(const Duration(milliseconds: 180));
        await _drain(tester);
        expect(controller.volume, .2);
      },
    );
  }

  testWidgets('a new gesture survives an older release waiting for a command', (
    tester,
  ) async {
    final system = _System()..volumeGate = Completer<void>();
    final controller = await _start(tester, system);
    controller.beginVolumeAdjustment();
    final first = controller.setVolume(.6);
    final oldEnd = controller.endVolumeAdjustment(.6);
    controller.beginVolumeAdjustment();
    final latest = controller.setVolume(.9);
    system.volumeGate!.complete();
    await _drain(tester);
    await Future.wait([first, oldEnd, latest]);
    system.snapshot = const SystemSnapshot(volume: .2, brightness: .5);
    await tester.pump(const Duration(seconds: 10));
    await _drain(tester);
    expect(controller.volume, .9);
    await controller.endVolumeAdjustment(.9);
    await tester.pump(const Duration(milliseconds: 180));
    await _drain(tester);
    expect(controller.volume, .2);
  });

  testWidgets(
    'dispose resolves an in-flight write and release without late notifications',
    (tester) async {
      final system = _System()..volumeGate = Completer<void>();
      final controller = await _start(tester, system);
      var notifications = 0;
      controller.addListener(() => notifications++);
      controller.beginVolumeAdjustment();
      final first = controller.setVolume(.6);
      final finished = controller.endVolumeAdjustment(.6);
      expect(system.volumeWrites, [.6]);
      controller.dispose();
      final atDispose = notifications;
      await _drain(tester);
      await Future.wait([first, finished]);
      system.volumeGate!.complete();
      await _drain(tester);
      expect(notifications, atDispose);
      expect(system.volumeWrites, [.6]);
      expect(system.reads, 1);
    },
  );

  testWidgets(
    'brightness preserves visible minimum and invalid input never queues',
    (tester) async {
      final system = _System();
      final controller = await _start(tester, system);
      final applied = controller.setBrightness(0);
      expect(controller.brightness, .01);
      await controller.setVolume(double.nan);
      await controller.setVolume(double.infinity);
      await tester.pump(const Duration(milliseconds: 90));
      await _drain(tester);
      await applied;
      expect(system.brightnessWrites, [.01]);
      expect(system.volumeWrites, isEmpty);
      expect(controller.volume, .4);
    },
  );

  testWidgets('unavailable controls do not invent values or call hardware', (
    tester,
  ) async {
    final system = _System()..snapshot = const SystemSnapshot();
    final controller = await _start(tester, system);
    controller.beginVolumeAdjustment();
    await controller.setVolume(.9);
    await controller.endVolumeAdjustment(.9);
    await controller.setBrightness(.6);
    expect(controller.volume, isNull);
    expect(controller.brightness, isNull);
    expect(controller.error, contains('不可用'));
    expect(system.volumeWrites, isEmpty);
    expect(system.brightnessWrites, isEmpty);
  });

  testWidgets(
    'volume writes preserve mute and an old read cannot undo mute toggle',
    (tester) async {
      final system = _System()
        ..snapshot = const SystemSnapshot(
          volume: .4,
          brightness: .5,
          muted: true,
        );
      final controller = await _start(tester, system);
      final oldRead = Completer<SystemSnapshot>();
      system.nextRead = oldRead;
      await tester.pump(const Duration(seconds: 10));
      controller.beginVolumeAdjustment();
      final applied = controller.setVolume(.9);
      final finished = controller.endVolumeAdjustment(.9);
      await _drain(tester);
      await Future.wait([applied, finished]);
      expect(controller.muted, true);
      final toggled = controller.toggleMute();
      await _drain(tester);
      await toggled;
      expect(controller.muted, false);
      oldRead.complete(
        const SystemSnapshot(volume: .4, brightness: .5, muted: true),
      );
      await _drain(tester);
      expect(controller.volume, .9);
      expect(controller.muted, false);
      expect(system.muteWrites, 1);
    },
  );

  testWidgets(
    'mute toggle reconciles an externally changed hardware mute state after becoming idle',
    (tester) async {
      final system = _System();
      final controller = await _start(tester, system);
      expect(controller.muted, false);
      // Another client mutes the sink after the last shell snapshot.
      system.snapshot = const SystemSnapshot(
        volume: .4,
        brightness: .5,
        muted: true,
      );
      final toggled = controller.toggleMute();
      await _drain(tester);
      await toggled;
      expect(system.snapshot.muted, false);
      await tester.pump(const Duration(milliseconds: 180));
      await _drain(tester);
      expect(controller.muted, false);
      expect(system.muteWrites, 1);
      expect(system.reads, 2);
    },
  );
}
