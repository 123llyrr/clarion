// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:realand_shell/models.dart';
import 'package:realand_shell/services/audio_output_service.dart';
import 'package:realand_shell/services/global_app_menu_service.dart';
import 'package:realand_shell/services/system_service.dart';
import 'package:realand_shell/state/shell_controller.dart';
import 'package:realand_shell/theme/mac_shell_theme.dart';
import 'package:realand_shell/ui/mac_shell_view.dart';
import 'package:realand_shell/ui/mac_volume_menu.dart';
import 'package:realand_shell/ui/mac_control_center.dart';

class _Menus extends GlobalAppMenuService {
  _Menus() : super(hostRegistrar: false);
  @override
  Future<void> start() async {}
  @override
  Future<void> setActive(bool active) async {}
}

class _Backend {
  final events = StreamController<void>.broadcast(sync: true);
  final commands = <List<String>>[];
  String selected = 'speakers';
  Completer<void>? selectGate;
  bool failSelect = false;
  int closes = 0;
  final levels = <String, SystemAudioSnapshot>{
    'speakers': const SystemAudioSnapshot(volume: .25, muted: false),
    'usb': const SystemAudioSnapshot(volume: .7, muted: true),
  };
  Future<String> command(String executable, List<String> args) async {
    commands.add(args);
    if (args.first == '--format=json') {
      return jsonEncode([
        {'name': 'speakers', 'description': '内建扬声器'},
        {'name': 'usb', 'description': 'USB Audio'},
      ]);
    }
    if (args.first == 'get-default-sink') return selected;
    if (args.first == 'set-default-sink') {
      await selectGate?.future;
      if (failSelect) throw StateError('设备已断开');
      selected = args[1];
      events.add(null);
      return '';
    }
    throw StateError('Unexpected $executable $args');
  }

  Future<AudioEventWatch> watch() async =>
      AudioEventWatch(events.stream, () async {
        closes++;
      });
}

class _System extends SystemService {
  _System(this.backend);
  final _Backend backend;
  final writes = <(String, double)>[];
  int audioReads = 0;
  int preferences = 0;
  int fullReads = 0;
  Completer<SystemAudioSnapshot>? audioGate;
  Completer<void>? volumeGate;
  Completer<void>? preferencesGate;
  @override
  Future<SystemAudioSnapshot> readAudio() async {
    audioReads++;
    final gate = audioGate;
    audioGate = null;
    return gate == null ? backend.levels[backend.selected]! : gate.future;
  }

  @override
  Future<SystemSnapshot> read() async {
    fullReads++;
    final audio = backend.levels[backend.selected]!;
    return SystemSnapshot(
      volume: audio.volume,
      muted: audio.muted,
      brightness: .5,
    );
  }

  @override
  Future<void> setVolume(double value) async {
    final target = backend.selected;
    writes.add((target, value));
    await volumeGate?.future;
    backend.levels[target] = SystemAudioSnapshot(
      volume: value,
      muted: backend.levels[target]!.muted,
    );
    backend.events.add(null);
  }

  @override
  Future<void> toggleMute() async {
    final old = backend.levels[backend.selected]!;
    backend.levels[backend.selected] = SystemAudioSnapshot(
      volume: old.volume,
      muted: !old.muted,
    );
    backend.events.add(null);
  }

  @override
  Future<void> openSoundPreferences() async {
    preferences++;
    await preferencesGate?.future;
  }
}

class _Fixture {
  _Fixture() {
    system = _System(backend);
    outputs = AudioOutputService(
      command: backend.command,
      watch: backend.watch,
    );
    controller =
        ShellController(
            system: system,
            audioOutputService: outputs,
            appMenuService: _Menus(),
            notifications: false,
            tray: false,
            powerProfilesEnabled: false,
          )
          ..setShellStyle(ShellStyle.macos)
          ..setReduceMotion(true);
    addTearDown(controller.dispose);
    addTearDown(backend.events.close);
  }
  final backend = _Backend();
  late final _System system;
  late final AudioOutputService outputs;
  late final ShellController controller;
}

Finder _key(String name) => find.byKey(ValueKey(name));
Future<void> _drain(WidgetTester tester) async {
  for (var i = 0; i < 12; i++) {
    await tester.pump();
  }
}

Future<void> _show(
  WidgetTester tester,
  _Fixture fixture, {
  Size size = const Size(1366, 768),
}) async {
  tester.view.devicePixelRatio = 1;
  tester.view.physicalSize = size;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  await tester.pumpWidget(
    MaterialApp(
      theme: buildMacShellTheme(Brightness.light),
      home: RepaintBoundary(
        key: const ValueKey('volume-capture'),
        child: MacShellView(controller: fixture.controller, preview: true),
      ),
    ),
  );
  await tester.pumpAndSettle();
  await tester.pump(const Duration(milliseconds: 200));
  await _drain(tester);
  await tester.tap(_key('mac-volume'));
  await _drain(tester);
}

void main() {
  testWidgets(
    'menu bar volume opens the anchored Catalina menu and retains Control Center',
    (tester) async {
      final fixture = _Fixture();
      await _show(tester, fixture);
      expect(fixture.controller.panel, ShellPanel.volume);
      expect(find.byType(MacControlCenter), findsNothing);
      expect(find.text('音量：'), findsOneWidget);
      expect(find.text('输出设备：'), findsOneWidget);
      expect(find.text('声音偏好设置…'), findsOneWidget);
      final button = tester.getRect(_key('mac-volume'));
      final menu = tester.getRect(_key('mac-volume-glass'));
      expect(menu.width, MacVolumeMenu.width);
      expect(menu.right, closeTo(button.right, .01));
      expect(menu.top, fixture.controller.macPreferences.menuBarHeight + 4);
      expect(menu.height, lessThan(220));
      expect(tester.widget<Slider>(_key('mac-volume-slider')).value, .25);
      expect(
        fixture.backend.commands.where(
          (args) => args.first == 'set-default-sink',
        ),
        isEmpty,
      );
      fixture.controller.selectPanel(ShellPanel.quickSettings);
      await tester.pumpAndSettle();
      expect(find.byType(MacControlCenter), findsOneWidget);
      expect(find.byType(MacVolumeMenu), findsNothing);
      expect(fixture.outputs.active, isFalse);
      expect(fixture.backend.closes, 1);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    '30 drag frames move the thumb without rebuilding output rows or enumerating devices',
    (tester) async {
      final fixture = _Fixture();
      await _show(tester, fixture);
      final before = tester.widget<Column>(_key('mac-volume-output-rows'));
      final reads = fixture.backend.commands.length;
      final rect = tester.getRect(_key('mac-volume-slider'));
      final gesture = await tester.startGesture(
        Offset(rect.left + 20, rect.center.dy),
      );
      for (var frame = 1; frame <= 30; frame++) {
        await gesture.moveTo(
          Offset(
            rect.left + 20 + (rect.width - 40) * frame / 30,
            rect.center.dy,
          ),
        );
        await tester.pump(const Duration(milliseconds: 16));
        expect(
          tester.widget<Column>(_key('mac-volume-output-rows')),
          same(before),
        );
      }
      expect(
        tester.widget<Slider>(_key('mac-volume-slider')).value,
        greaterThan(.85),
      );
      expect(fixture.backend.commands, hasLength(reads));
      await gesture.up();
      await tester.pump(const Duration(milliseconds: 300));
      await _drain(tester);
      expect(fixture.system.writes.last.$2, greaterThan(.85));
      expect(fixture.system.writes.length, lessThan(12));
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'external volume and mute changes update locally, and mute sends a real command',
    (tester) async {
      final fixture = _Fixture();
      await _show(tester, fixture);
      final before = tester.widget<Column>(_key('mac-volume-output-rows'));
      fixture.backend.levels['speakers'] = const SystemAudioSnapshot(
        volume: .6,
        muted: true,
      );
      fixture.backend.events.add(null);
      await tester.pump(const Duration(milliseconds: 110));
      await _drain(tester);
      expect(tester.widget<Slider>(_key('mac-volume-slider')).value, .6);
      expect(fixture.controller.muted, isTrue);
      expect(
        tester.widget<Column>(_key('mac-volume-output-rows')),
        same(before),
      );
      await tester.tap(_key('mac-volume-mute'));
      await _drain(tester);
      expect(fixture.backend.levels['speakers']!.muted, isFalse);
      expect(fixture.controller.muted, isFalse);
      await tester.pump(const Duration(milliseconds: 400));
    },
  );

  testWidgets(
    'output selection drains old volume writes and retains the new device level',
    (tester) async {
      final fixture = _Fixture();
      await _show(tester, fixture);
      fixture.system.volumeGate = Completer<void>();
      unawaited(fixture.controller.setVolume(.45));
      await _drain(tester);
      unawaited(fixture.controller.setVolume(.5));
      await tester.tap(_key('mac-audio-output-usb'));
      await _drain(tester);
      expect(fixture.backend.selected, 'speakers');
      expect(fixture.controller.busy, contains('audio-output'));
      expect(
        tester.widget<Slider>(_key('mac-volume-slider')).onChanged,
        isNull,
      );
      fixture.system.volumeGate!.complete();
      await _drain(tester);
      expect(
        fixture.system.writes.map((write) => write.$1),
        everyElement('speakers'),
      );
      expect(fixture.system.writes.last.$2, .5);
      expect(fixture.backend.selected, 'usb');
      expect(fixture.controller.volume, .7);
      expect(fixture.controller.muted, isTrue);
      expect(fixture.controller.panel, ShellPanel.none);
      await tester.pump(const Duration(milliseconds: 400));
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'failed device selection stays open with the real error and original check',
    (tester) async {
      final fixture = _Fixture();
      await _show(tester, fixture);
      fixture.backend.failSelect = true;
      await tester.tap(_key('mac-audio-output-usb'));
      await _drain(tester);
      expect(fixture.controller.panel, ShellPanel.volume);
      expect(fixture.outputs.selected, 'speakers');
      expect(find.textContaining('设备已断开'), findsOneWidget);
      expect(fixture.controller.busy, isNot(contains('audio-output')));
      await tester.pump(const Duration(milliseconds: 400));
    },
  );

  testWidgets('late external reads cannot overwrite a drag or a newer menu', (
    tester,
  ) async {
    final fixture = _Fixture();
    await _show(tester, fixture);
    final pending = Completer<SystemAudioSnapshot>();
    fixture.system.audioGate = pending;
    unawaited(fixture.outputs.refresh());
    await _drain(tester);
    fixture.controller.beginVolumeAdjustment();
    unawaited(fixture.controller.setVolume(.8));
    await tester.pump(const Duration(milliseconds: 90));
    pending.complete(const SystemAudioSnapshot(volume: .1, muted: true));
    await _drain(tester);
    expect(fixture.controller.volume, .8);
    expect(fixture.controller.muted, isFalse);
    await fixture.controller.endVolumeAdjustment(.8);
    fixture.controller.selectPanel(ShellPanel.quickSettings);
    await tester.pump(const Duration(milliseconds: 400));
    expect(fixture.controller.panel, ShellPanel.quickSettings);
    expect(fixture.controller.volume, .8);
    expect(tester.takeException(), isNull);
  });

  testWidgets('sound preferences completion never closes a newer panel', (
    tester,
  ) async {
    final fixture = _Fixture();
    await _show(tester, fixture);
    fixture.system.preferencesGate = Completer<void>();
    await tester.tap(_key('mac-sound-preferences'));
    await _drain(tester);
    expect(fixture.system.preferences, 1);
    fixture.controller.selectPanel(ShellPanel.quickSettings);
    fixture.system.preferencesGate!.complete();
    await tester.pumpAndSettle();
    expect(fixture.controller.panel, ShellPanel.quickSettings);
    expect(tester.takeException(), isNull);
  });

  testWidgets('a small output scrolls the menu without overflowing', (
    tester,
  ) async {
    final fixture = _Fixture();
    await _show(tester, fixture, size: const Size(640, 150));
    final rect = tester.getRect(_key('mac-volume-glass'));
    expect(rect.left, greaterThanOrEqualTo(12));
    expect(rect.right, lessThanOrEqualTo(628));
    expect(rect.bottom, lessThanOrEqualTo(150));
    expect(tester.takeException(), isNull);
  });

  testWidgets('optional local volume menu preview', (tester) async {
    final destination = Platform.environment['REALAND_VOLUME_PREVIEW_DIR'];
    if (destination == null) return;
    await tester.runAsync(() async {
      for (final entry in {
        'macOSSystem': 'assets/macos/SFNS.ttf',
        'macOSChinese': 'assets/macos/PingFangSC-Regular.otf',
        'MaterialIcons':
            'build/unit_test_assets/fonts/MaterialIcons-Regular.otf',
      }.entries) {
        final file = File(entry.value);
        if (await file.exists()) {
          final loader = FontLoader(entry.key)
            ..addFont(
              file.readAsBytes().then((bytes) => ByteData.sublistView(bytes)),
            );
          await loader.load();
        }
      }
    });
    final fixture = _Fixture();
    await _show(tester, fixture);
    final boundary = tester.renderObject<RenderRepaintBoundary>(
      _key('volume-capture'),
    );
    await tester.runAsync(() async {
      final image = await boundary.toImage(pixelRatio: 1);
      final bytes = await image.toByteData(format: ui.ImageByteFormat.png);
      await Directory(destination).create(recursive: true);
      await File(
        '$destination/macos-volume.png',
      ).writeAsBytes(bytes!.buffer.asUint8List());
      image.dispose();
    });
  });
}
