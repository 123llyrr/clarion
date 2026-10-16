// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/main.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/preferences.dart';
import 'package:clarion_shell/services/desktop_apps.dart';
import 'package:clarion_shell/services/global_app_menu_service.dart';
import 'package:clarion_shell/services/clarion_ipc.dart';
import 'package:clarion_shell/services/sound_service.dart';
import 'package:clarion_shell/services/system_service.dart';
import 'package:clarion_shell/services/wallpaper_service.dart';
import 'package:clarion_shell/state/shell_controller.dart';

class _System extends SystemService {
  var snapshot = const SystemSnapshot(volume: .4, brightness: .5);
  final gate = Completer<void>();
  final writes = <(String, double)>[];

  @override
  Future<SystemSnapshot> read() async => snapshot;
  @override
  Future<void> setVolume(double value) async {
    writes.add(('volume', value));
    await gate.future;
    snapshot = SystemSnapshot(volume: value, brightness: snapshot.brightness);
  }

  @override
  Future<void> setBrightness(double value) async {
    writes.add(('brightness', value));
    await gate.future;
    snapshot = SystemSnapshot(volume: snapshot.volume, brightness: value);
  }
}

class _Ipc extends ClarionIpc {
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

class _Menus extends GlobalAppMenuService {
  _Menus() : super(hostRegistrar: false);
  @override
  Future<void> start() async {}
  @override
  Future<void> setActive(bool active) async {}
}

Future<void> _drain(WidgetTester tester) async {
  for (var i = 0; i < 8; i++) {
    await tester.pump();
  }
}

Future<ShellController> _mount(
  WidgetTester tester,
  _System system, {
  ShellStyle style = ShellStyle.material,
}) async {
  tester.view.physicalSize = const Size(1280, 800);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  final controller = ShellController(
    notifications: false,
    tray: false,
    powerProfilesEnabled: false,
    system: system,
    ipc: _Ipc(),
    desktopApps: _Apps(),
    wallpaperService: _Wallpapers(),
    appMenuService: _Menus(),
    soundService: SoundService(enabled: false),
  );
  addTearDown(controller.dispose);
  addTearDown(() {
    if (!system.gate.isCompleted) system.gate.complete();
  });
  final started = controller.start();
  await _drain(tester);
  await started;
  controller.setReduceMotion(true);
  controller.setShellStyle(style);
  controller.selectPanel(ShellPanel.quickSettings);
  await tester.pumpWidget(
    ShellApplication(
      controller: controller,
      preferences: const ShellPreferences(),
      preview: true,
      persist: false,
    ),
  );
  await _drain(tester);
  return controller;
}

Finder _slider(String key) => find.byKey(ValueKey('$key-slider'));

void main() {
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

  for (final style in ShellStyle.values) {
    for (final key in ['volume', 'brightness']) {
      testWidgets(
        '${style.name} $key follows each drag frame and cancel flushes latest value',
        (tester) async {
          final system = _System();
          final controller = await _mount(tester, system, style: style);
          final finder = _slider(key);
          final element = tester.element(finder);
          final slider = tester.widget<Slider>(finder);
          expect(slider.divisions, isNull);
          expect(slider.onChangeStart, isNotNull);
          expect(slider.onChangeEnd, isNotNull);
          final center = tester.getCenter(finder);
          final gesture = await tester.startGesture(center);
          await gesture.moveTo(center + const Offset(70, 0));
          await tester.pump(const Duration(milliseconds: 16));
          final forward = tester.widget<Slider>(finder).value;
          expect(forward, greaterThan(.5));
          expect(
            forward,
            key == 'volume' ? controller.volume : controller.brightness,
          );
          expect(system.writes, isEmpty);
          await gesture.moveTo(center - const Offset(70, 0));
          await tester.pump(const Duration(milliseconds: 16));
          final backward = tester.widget<Slider>(finder).value;
          expect(backward, lessThan(forward));
          expect(
            backward,
            key == 'volume' ? controller.volume : controller.brightness,
          );
          expect(tester.element(finder), same(element));
          await gesture.cancel();
          await _drain(tester);
          // PointerCancel must take the same final-value path as a normal release,
          // bypassing the 90 ms timer while the system command is still blocked.
          expect(system.writes.single.$1, key);
          expect(system.writes.single.$2, closeTo(backward, 1e-9));
          system.gate.complete();
          await _drain(tester);
          expect(
            key == 'volume'
                ? system.snapshot.volume
                : system.snapshot.brightness,
            closeTo(backward, 1e-9),
          );
          expect(tester.widget<Slider>(finder).value, closeTo(backward, 1e-9));
          controller.closePanel();
          await tester.pumpWidget(const SizedBox());
          expect(tester.takeException(), isNull);
        },
      );
    }
  }

  for (final key in ['volume', 'brightness']) {
    testWidgets('macos $key follows 30 frames without rebuilding preferences', (
      tester,
    ) async {
      final system = _System();
      final controller = await _mount(tester, system, style: ShellStyle.macos);
      final finder = _slider(key);
      final connectivity = tester.widget(
        find.byKey(const ValueKey('mac-control-connectivity-card')),
      );
      final footer = tester.widget(
        find.byKey(const ValueKey('mac-control-footer')),
      );
      final center = tester.getCenter(finder);
      final gesture = await tester.startGesture(center);
      var previous = tester.widget<Slider>(finder).value;
      for (var frame = 0; frame < 30; frame++) {
        final dx = frame < 15 ? -70 + frame * 10 : 70 - (frame - 14) * 10;
        await gesture.moveTo(center + Offset(dx.toDouble(), 0));
        await tester.pump(const Duration(milliseconds: 16));
        final value = tester.widget<Slider>(finder).value;
        expect((value - previous).abs(), greaterThan(.001));
        expect(
          value,
          key == 'volume' ? controller.volume : controller.brightness,
        );
        expect(
          tester.widget(
            find.byKey(const ValueKey('mac-control-connectivity-card')),
          ),
          same(connectivity),
        );
        expect(
          tester.widget(find.byKey(const ValueKey('mac-control-footer'))),
          same(footer),
        );
        previous = value;
      }
      await gesture.up();
      system.gate.complete();
      await _drain(tester);
      expect(system.writes.last.$2, closeTo(previous, 1e-9));
      expect(tester.widget<Slider>(finder).value, closeTo(previous, 1e-9));
      controller.closePanel();
      await tester.pumpWidget(const SizedBox());
      expect(tester.takeException(), isNull);
    });
  }

  testWidgets('macos sliders follow external system changes and availability', (
    tester,
  ) async {
    final system = _System();
    final controller = await _mount(tester, system, style: ShellStyle.macos);
    system.snapshot = const SystemSnapshot(
      volume: .8,
      brightness: .3,
      muted: true,
    );
    await tester.pump(const Duration(seconds: 10));
    await _drain(tester);
    expect(tester.widget<Slider>(_slider('volume')).value, .8);
    expect(tester.widget<Slider>(_slider('brightness')).value, .3);
    expect(find.byTooltip('取消静音'), findsOneWidget);
    system.snapshot = const SystemSnapshot();
    await tester.pump(const Duration(seconds: 10));
    await _drain(tester);
    expect(tester.widget<Slider>(_slider('volume')).onChanged, isNull);
    expect(tester.widget<Slider>(_slider('brightness')).onChanged, isNull);
    system.snapshot = const SystemSnapshot(volume: .2, brightness: .6);
    await tester.pump(const Duration(seconds: 10));
    await _drain(tester);
    expect(tester.widget<Slider>(_slider('volume')).onChanged, isNotNull);
    expect(tester.widget<Slider>(_slider('brightness')).value, .6);
    controller.closePanel();
    await tester.pumpWidget(const SizedBox());
  });

  testWidgets('brightness UI range matches visible hardware minimum', (
    tester,
  ) async {
    final system = _System();
    system.snapshot = const SystemSnapshot(volume: .4, brightness: 0);
    final controller = await _mount(tester, system);
    final slider = tester.widget<Slider>(_slider('brightness'));
    expect(slider.min, .01);
    expect(slider.value, .01);
    controller.closePanel();
    await tester.pumpWidget(const SizedBox());
  });
}
