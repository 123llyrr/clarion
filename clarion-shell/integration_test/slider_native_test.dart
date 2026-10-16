// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:convert';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:clarion_shell/main.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/preferences.dart';
import 'package:clarion_shell/services/desktop_apps.dart';
import 'package:clarion_shell/services/power_profiles_service.dart';
import 'package:clarion_shell/services/system_service.dart';
import 'package:clarion_shell/services/wallpaper_service.dart';
import 'package:clarion_shell/state/shell_controller.dart';

const _window = MethodChannel('clarion.shell/window');
const _barKeys = [
  'bar-launcher',
  'bar-screenshot',
  'bar-notifications',
  'bar-quick-settings',
];

Finder _key(String key) => find.byKey(ValueKey(key));
Finder _slider(String key) => _key('$key-slider');

Future<void> _wait(
  WidgetTester tester,
  bool Function() ready,
  String reason,
) async {
  final deadline = DateTime.now().add(const Duration(seconds: 15));
  while (!ready()) {
    if (DateTime.now().isAfter(deadline)) fail('Timed out: $reason');
    await tester.pump(const Duration(milliseconds: 16));
  }
}

Future<Map<String, dynamic>> _waitNative(
  WidgetTester tester,
  BarPosition position, {
  required bool expanded,
}) async {
  final deadline = DateTime.now().add(const Duration(seconds: 15));
  Map<String, dynamic> state = const {};
  while (DateTime.now().isBefore(deadline)) {
    await tester.pump(const Duration(milliseconds: 16));
    state = Map<String, dynamic>.from(
      (await _window.invokeMapMethod<String, dynamic>('windowState'))!,
    );
    final height = expanded ? (state['monitorGeometry'] as Map)['height'] : 40;
    if (state['barPosition'] != position.name ||
        state['expanded'] != expanded ||
        state['height'] != height) {
      continue;
    }
    expect(state['exclusiveZone'], 40);
    expect(state['layer'], expanded ? 'Overlay' : 'Top');
    expect(state['keyboard'], expanded ? 'Exclusive' : 'None');
    expect(state['inputHeight'], height);
    expect(state['scaleFactor'], 1);
    return state;
  }
  fail('Native state mismatch: $state');
}

Future<String> _forbiddenCommand(String executable, List<String> args) async {
  throw StateError('Native slider fixture cannot execute $executable $args');
}

class _System extends SystemService {
  _System() : super(command: _forbiddenCommand);

  double volume = .6;
  double brightness = .7;
  bool muted = false;
  bool staleNextRead = false;
  int reads = 0;
  int staleReads = 0;
  int readsInFlight = 0;
  int completedStaleReads = 0;
  final writes = <Map<String, dynamic>>[];
  final _inFlight = <String, int>{};
  final maxInFlight = <String, int>{};

  double current(String key) => key == 'volume' ? volume : brightness;
  int completed(String key) => writes
      .where((write) => write['key'] == key && write['completed'] == true)
      .length;
  bool committed(String key, double value, int completedBefore) {
    final matching = writes.where((write) => write['key'] == key);
    return matching.isNotEmpty &&
        completed(key) > completedBefore &&
        matching.last['completed'] == true &&
        ((matching.last['value'] as double) - value).abs() < .0001 &&
        (_inFlight[key] ?? 0) == 0 &&
        (current(key) - value).abs() < .0001;
  }

  @override
  Future<SystemSnapshot> read() async {
    reads++;
    readsInFlight++;
    final stale = staleNextRead;
    staleNextRead = false;
    if (stale) staleReads++;
    try {
      await Future<void>.delayed(const Duration(milliseconds: 20));
      return SystemSnapshot(
        volume: stale ? .12 : volume,
        brightness: stale ? .14 : brightness,
        muted: muted,
        batteryFraction: .8,
        wifiEnabled: true,
        bluetoothEnabled: true,
        networkName: '隔离滑杆测试网络',
      );
    } finally {
      readsInFlight--;
      if (stale) completedStaleReads++;
    }
  }

  Future<void> _write(String key, double value) async {
    final active = (_inFlight[key] ?? 0) + 1;
    _inFlight[key] = active;
    if (active > (maxInFlight[key] ?? 0)) maxInFlight[key] = active;
    final write = <String, dynamic>{
      'key': key,
      'value': value,
      'completed': false,
      'startedAt': DateTime.now().toIso8601String(),
    };
    writes.add(write);
    try {
      await Future<void>.delayed(const Duration(milliseconds: 150));
      if (key == 'volume') {
        volume = value;
      } else {
        brightness = value;
      }
      write['completed'] = true;
      write['completedAt'] = DateTime.now().toIso8601String();
    } finally {
      _inFlight[key] = active - 1;
    }
  }

  @override
  Future<void> setVolume(double value) => _write('volume', value);
  @override
  Future<void> setBrightness(double value) => _write('brightness', value);
  @override
  Future<void> toggleMute() async {
    muted = !muted;
  }

  @override
  Future<void> setWifi(bool enabled) async {}
  @override
  Future<void> setBluetooth(bool enabled) async {}
}

class _Apps extends DesktopApps {
  @override
  Future<List<DesktopApp>> load() async => const [];
}

class _Controller extends ShellController {
  _Controller(String root, _System system)
    : super(
        notifications: false,
        tray: false,
        desktopApps: _Apps(),
        system: system,
        powerProfiles: PowerProfilesService(demo: true),
        wallpaperService: WallpaperService(
          directoryPath: '$root/wallpapers',
          preferenceFile: File('$root/wallpaper.json'),
        ),
        restoreWallpaperOnStart: false,
      );

  void pulse() => notifyListeners();
}

Future<Map<String, dynamic>> _capture(
  WidgetTester tester,
  String key,
  String phase,
) async {
  final rect = tester.getRect(_slider(key)).inflate(8);
  final origin = (await _window.invokeMapMethod<String, dynamic>(
    'surfaceOrigin',
  ))!;
  final width = rect.width.ceil();
  final height = rect.height.ceil();
  final file = File(
    '${Platform.environment['CLARION_SHELL_CAPTURE_DIR']}/$phase.png',
  );
  final result = await Process.run('grim', [
    '-s',
    '1',
    '-g',
    '${(origin['x'] as num).round() + rect.left.floor()},'
        '${(origin['y'] as num).round() + rect.top.floor()} ${width}x$height',
    file.path,
  ]).timeout(const Duration(seconds: 15));
  expect(result.exitCode, 0, reason: '${result.stdout}\n${result.stderr}');
  final codec = await ui.instantiateImageCodec(await file.readAsBytes());
  final frame = await codec.getNextFrame();
  try {
    expect(frame.image.width, width);
    expect(frame.image.height, height);
  } finally {
    frame.image.dispose();
    codec.dispose();
  }
  return {'phase': phase, 'path': file.path, 'width': width, 'height': height};
}

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.onlyPumps;
  testWidgets('native sliders follow pointer before slow writes and stale reads', (
    tester,
  ) async {
    expect(Platform.environment['CLARION_SHELL_ISOLATED_TEST'], '1');
    expect(
      Platform.environment['WAYLAND_DISPLAY'],
      isNot(Platform.environment['CLARION_SHELL_PARENT_WAYLAND_DISPLAY']),
    );
    expect(
      Platform.environment['CLARION_SOCKET'],
      isNot(Platform.environment['CLARION_SHELL_PARENT_SOCKET']),
    );
    expect(
      Platform.environment['DBUS_SESSION_BUS_ADDRESS'],
      isNot(Platform.environment['CLARION_SHELL_PARENT_DBUS_ADDRESS']),
    );
    final root = Platform.environment['CLARION_SHELL_TEST_ROOT']!;
    for (final variable in [
      'XDG_CONFIG_HOME',
      'XDG_DATA_HOME',
      'XDG_CACHE_HOME',
      'XDG_STATE_HOME',
    ]) {
      expect(Platform.environment[variable]!.startsWith('$root/'), isTrue);
    }
    final system = _System();
    final controller = _Controller(root, system);
    controller.setThemeMode(ThemeMode.dark);
    final evidence = <String, dynamic>{
      'condition':
          'Real GTK Flutter runner and Material panel on nested Clarion Vulkan '
          'at scale 1; onlyPumps. Non-demo controller; fake system writer takes '
          '150 ms. No hardware writer is reachable. Sampled next pumped frame '
          'and settled screenshots are functional evidence, not a frame-rate '
          'or physical input-latency benchmark.',
      'input':
          'Pointer drags and keyboard events target this isolated Flutter '
          'binding only. Main-session physical input is untouched.',
      'samples': <Map<String, dynamic>>[],
      'captures': <Map<String, dynamic>>[],
      'identityChecks': <String>[],
      'cases': <Map<String, dynamic>>[],
    };
    final barElements = <String, Element>{};
    final barButtons = <String, Element>{};
    TestGesture? activeGesture;
    try {
      await tester.pumpWidget(
        ShellApplication(
          controller: controller,
          preferences: const ShellPreferences(themeMode: ThemeMode.dark),
          persist: false,
        ),
      );
      await controller.start();
      await _wait(tester, () => controller.connected, 'nested IPC');
      await _waitNative(tester, BarPosition.top, expanded: false);
      for (final key in _barKeys) {
        barElements[key] = tester.element(_key(key));
        barButtons[key] = tester.element(
          find.descendant(of: _key(key), matching: find.byType(IconButton)),
        );
      }

      void checkIdentity(String phase) {
        for (final key in _barKeys) {
          expect(tester.element(_key(key)), same(barElements[key]));
          expect(
            tester.element(
              find.descendant(of: _key(key), matching: find.byType(IconButton)),
            ),
            same(barButtons[key]),
          );
        }
        expect(tester.takeException(), isNull, reason: phase);
        (evidence['identityChecks'] as List).add(phase);
      }

      double value(String key) => tester.widget<Slider>(_slider(key)).value;

      void sample(String key, String phase, {double? pointerFraction}) {
        final current = value(key);
        final row = find
            .ancestor(
              of: find.text(key == 'volume' ? '音量' : '亮度'),
              matching: find.byType(Row),
            )
            .first;
        expect(
          find.descendant(
            of: row,
            matching: find.text('${(current * 100).round()}%'),
          ),
          findsOneWidget,
          reason: '$phase percentage follows the visible slider in this frame',
        );
        if (pointerFraction != null) {
          // The real Material track has side padding inside the Slider widget.
          expect(current, closeTo(pointerFraction, .13), reason: phase);
        }
        (evidence['samples'] as List).add({
          'phase': phase,
          'key': key,
          'visibleValue': current,
          'fakeHardwareValue': system.current(key),
          'completedWrites': system.completed(key),
          'pointerFraction': ?pointerFraction,
        });
        checkIdentity(phase);
      }

      for (final position in BarPosition.values) {
        controller.setBarPosition(position);
        for (final reduced in [false, true]) {
          controller.setReduceMotion(reduced);
          controller.selectPanel(ShellPanel.quickSettings);
          await tester.pump();
          await _waitNative(tester, position, expanded: true);
          await tester.pump(const Duration(milliseconds: 300));
          final label = '${position.name}-${reduced ? 'reduced' : 'animated'}';

          for (final key in ['volume', 'brightness']) {
            final phase = '$label-$key';
            await tester.ensureVisible(_slider(key));
            await tester.pump();
            final rect = tester.getRect(_slider(key));
            Offset point(double fraction) =>
                Offset(rect.left + rect.width * fraction, rect.center.dy);
            final originalSlider = tester.element(_slider(key));
            final completedBefore = system.completed(key);
            activeGesture = await tester.startGesture(
              point(.2),
              kind: PointerDeviceKind.mouse,
            );
            await tester.pump(const Duration(milliseconds: 16));
            sample(key, '$phase-start-frame', pointerFraction: .2);
            expect(system.completed(key), completedBefore);
            (evidence['captures'] as List).add(
              await _capture(tester, key, '$phase-start'),
            );

            var previous = value(key);
            var priorFraction = .2;
            var index = 0;
            for (final fraction in [.4, .65, .84, .55, .25, .7, .35]) {
              await activeGesture.moveTo(point(fraction));
              await tester.pump(const Duration(milliseconds: 16));
              final current = value(key);
              if (fraction > priorFraction) {
                expect(current, greaterThan(previous + .03));
              } else {
                expect(current, lessThan(previous - .03));
              }
              sample(key, '$phase-move-$index', pointerFraction: fraction);
              expect(tester.element(_slider(key)), same(originalSlider));
              previous = current;
              priorFraction = fraction;
              if (index == 2) {
                controller.pulse();
                await tester.pump(const Duration(milliseconds: 16));
                expect(value(key), closeTo(current, .0001));
                await _wait(
                  tester,
                  () => system.readsInFlight == 0,
                  '$phase no older system read blocks the stale injection',
                );
                final staleBefore = system.staleReads;
                final completedStaleBefore = system.completedStaleReads;
                system.staleNextRead = true;
                // Polling continues during a gesture. Wait for its next fake
                // read instead of using a connectivity operation: successful
                // toggles now reconcile after the interaction becomes idle.
                // This read starts AND completes before the pointer is released.
                await _wait(
                  tester,
                  () => system.completedStaleReads == completedStaleBefore + 1,
                  '$phase periodic stale read completes during the gesture',
                );
                expect(system.staleReads, staleBefore + 1);
                expect(system.completedStaleReads, completedStaleBefore + 1);
                expect(system.readsInFlight, 0);
                expect(activeGesture, isNotNull);
                await tester.pump(const Duration(milliseconds: 16));
                expect(value(key), closeTo(current, .0001));
                sample(key, '$phase-stale-read-protected');
                (evidence['captures'] as List).add(
                  await _capture(tester, key, '$phase-mid'),
                );
              }
              index++;
            }
            final finalValue = value(key);
            await activeGesture.up();
            activeGesture = null;
            await tester.pump(const Duration(milliseconds: 16));
            sample(key, '$phase-release-frame');
            expect(value(key), closeTo(finalValue, .0001));
            await _wait(
              tester,
              () =>
                  system.committed(key, finalValue, completedBefore) &&
                  !controller.busy.contains(key),
              '$phase final value is committed',
            );
            await tester.pump();
            expect(value(key), closeTo(finalValue, .0001));
            sample(key, '$phase-settled');
            (evidence['captures'] as List).add(
              await _capture(tester, key, '$phase-end'),
            );
            (evidence['cases'] as List).add({
              'phase': phase,
              'finalValue': finalValue,
              'committedValue': system.current(key),
              'completedWrites': system.completed(key) - completedBefore,
            });
          }

          final focus = tester.widget<FocusableActionDetector>(
            find.descendant(
              of: _slider('volume'),
              matching: find.byType(FocusableActionDetector),
            ),
          );
          expect(focus.focusNode, isNotNull);
          focus.focusNode!.requestFocus();
          await tester.pump();
          final beforeKey = value('volume');
          final completedBeforeKey = system.completed('volume');
          await tester.sendKeyEvent(LogicalKeyboardKey.arrowRight);
          await tester.pump(const Duration(milliseconds: 16));
          final afterKey = value('volume');
          expect(afterKey, greaterThan(beforeKey));
          sample('volume', '$label-keyboard-right');
          await _wait(
            tester,
            () =>
                system.committed('volume', afterKey, completedBeforeKey) &&
                !controller.busy.contains('volume'),
            '$label keyboard value is committed',
          );
          expect(value('volume'), closeTo(afterKey, .0001));
          controller.closePanel();
          await tester.pump();
          await _waitNative(tester, position, expanded: false);
          checkIdentity('$label-closed');
        }
      }
      expect(system.staleReads, 8);
      expect(system.completedStaleReads, 8);
      expect(system.maxInFlight['volume'], 1);
      expect(system.maxInFlight['brightness'], 1);
      expect(controller.error, isNull);
      evidence['passed'] = true;
    } finally {
      if (activeGesture != null) await activeGesture.cancel();
      evidence['fakeReads'] = system.reads;
      evidence['staleReads'] = system.staleReads;
      evidence['completedStaleReads'] = system.completedStaleReads;
      evidence['readsInFlightAtEnd'] = system.readsInFlight;
      evidence['fakeWrites'] = system.writes;
      evidence['maxInFlight'] = system.maxInFlight;
      await File(
        '${Platform.environment['CLARION_SHELL_CAPTURE_DIR']}/slider-native-evidence.json',
      ).writeAsString(
        '${const JsonEncoder.withIndent('  ').convert(evidence)}\n',
      );
      await tester.pumpWidget(const SizedBox.shrink());
    }
  });
}
