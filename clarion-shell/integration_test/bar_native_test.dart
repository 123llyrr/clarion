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
const _buttonKeys = [
  'bar-launcher',
  'bar-screenshot',
  'bar-notifications',
  'bar-quick-settings',
];

Finder _barItem(String name) => find.byKey(ValueKey(name));
Finder _barButton(String name) =>
    find.descendant(of: _barItem(name), matching: find.byType(IconButton));

Future<Map<String, dynamic>> _state() async => Map<String, dynamic>.from(
  (await _window.invokeMapMethod<String, dynamic>('windowState'))!,
);

Future<void> _wait(
  WidgetTester tester,
  bool Function() ready,
  String reason,
) async {
  final deadline = DateTime.now().add(const Duration(seconds: 15));
  while (!ready()) {
    if (DateTime.now().isAfter(deadline)) {
      fail('Timed out: $reason');
    }
    await tester.pump(const Duration(milliseconds: 16));
  }
}

Future<Map<String, dynamic>> _waitNative(
  WidgetTester tester, {
  required bool expanded,
  bool tooltip = false,
}) async {
  final deadline = DateTime.now().add(const Duration(seconds: 15));
  Map<String, dynamic> state = const {};
  do {
    await tester.pump(const Duration(milliseconds: 16));
    state = await _state();
    final height = expanded
        ? (state['monitorGeometry'] as Map)['height']
        : tooltip
        ? 128
        : 40;
    if (state['expanded'] == expanded &&
        state['tooltipVisible'] == tooltip &&
        state['height'] == height) {
      expect(state['keyboard'], expanded ? 'Exclusive' : 'None');
      expect(state['inputHeight'], expanded ? height : 40);
      expect(state['layer'], expanded ? 'Overlay' : 'Top');
      expect(state['exclusiveZone'], 40);
      return state;
    }
  } while (DateTime.now().isBefore(deadline));
  fail('Native state mismatch: $state');
}

class _Pixels {
  const _Pixels(this.width, this.height, this.rgba);
  final int width;
  final int height;
  final Uint8List rgba;

  List<int> at(Offset point) {
    final x = point.dx.round().clamp(0, width - 1);
    final y = point.dy.round().clamp(0, height - 1);
    final index = (y * width + x) * 4;
    return rgba.sublist(index, index + 3);
  }
}

Future<_Pixels> _captureBar(String name, int width) async {
  final file = File(
    '${Platform.environment['CLARION_SHELL_CAPTURE_DIR']}/$name.png',
  );
  final result = await Process.run('grim', [
    '-s',
    '1',
    '-g',
    '0,0 ${width}x40',
    file.path,
  ]).timeout(const Duration(seconds: 15));
  expect(result.exitCode, 0, reason: '${result.stdout}\n${result.stderr}');
  final codec = await ui.instantiateImageCodec(await file.readAsBytes());
  final frame = await codec.getNextFrame();
  try {
    final bytes = await frame.image.toByteData(
      format: ui.ImageByteFormat.rawRgba,
    );
    return _Pixels(
      frame.image.width,
      frame.image.height,
      bytes!.buffer.asUint8List(),
    );
  } finally {
    frame.image.dispose();
    codec.dispose();
  }
}

class _Apps extends DesktopApps {
  @override
  Future<List<DesktopApp>> load() async => [
    for (var index = 0; index < 24; index++)
      DesktopApp(
        id: 'bar-fixture-$index',
        name: '隔离应用 ${index + 1}',
        desktopFile: '',
        iconName: 'utilities-terminal',
      ),
  ];
}

class _System extends SystemService {
  @override
  Future<SystemSnapshot> read() async => const SystemSnapshot(
    volume: .6,
    brightness: .7,
    batteryFraction: .8,
    wifiEnabled: true,
    bluetoothEnabled: true,
    networkName: '隔离测试网络',
  );
}

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.onlyPumps;
  testWidgets('native bar identity and pixels survive modal and hover motion', (
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
    final controller = ShellController(
      notifications: false,
      tray: false,
      desktopApps: _Apps(),
      system: _System(),
      powerProfiles: PowerProfilesService(demo: true),
      wallpaperService: WallpaperService(
        directoryPath: '$root/wallpapers',
        preferenceFile: File('$root/wallpaper.json'),
      ),
      restoreWallpaperOnStart: false,
    );
    controller.setThemeMode(ThemeMode.dark);
    controller.setReduceMotion(false);
    final evidence = <String, dynamic>{
      'input':
          'WidgetTester pointer events restricted to this test binding; no physical pointer or live session keyboard injection.',
      'condition':
          'Debug GTK Flutter Linux runner; nested Clarion Vulkan, scale 1; Material dark theme; framePolicy=onlyPumps. Pixel checks use actual grim captures after native geometry settles, without assuming a short animation remains at an intermediate fraction.',
      'proofs': <Map<String, dynamic>>[],
    };
    final identityChecks = <String>[];
    final items = <String, Element>{};
    final buttons = <String, Element>{};
    TestGesture? pointer;
    try {
      await tester.pumpWidget(
        ShellApplication(
          controller: controller,
          preferences: const ShellPreferences(themeMode: ThemeMode.dark),
          persist: false,
        ),
      );
      await controller.start();
      await _wait(
        tester,
        () => controller.connected && controller.apps.length == 24,
        'nested IPC and fixture applications',
      );
      final initial = await _waitNative(tester, expanded: false);
      final geometry = initial['monitorGeometry'] as Map;
      final width = geometry['width'] as int;
      expect(initial['scaleFactor'], 1);
      expect(width, greaterThan(720));
      for (final name in _buttonKeys) {
        items[name] = tester.element(_barItem(name));
        buttons[name] = tester.element(_barButton(name));
      }
      final color = Theme.of(
        items['bar-launcher']!,
      ).colorScheme.surfaceContainer;
      final expectedRgb = [
        (color.r * 255).round(),
        (color.g * 255).round(),
        (color.b * 255).round(),
      ];
      // The upper pixel row is outside every 36 px button's ink and label.
      // Sampling five independent locations detects a blank/replaced strip,
      // while selected/hovered buttons may legitimately change their colors.
      final points = [
        const Offset(5, 1),
        Offset(width * .25, 1),
        Offset(width * .5, 1),
        Offset(width * .75, 1),
        Offset(width - 5, 1),
      ];
      evidence['barColorRgb'] = expectedRgb;
      evidence['samplePoints'] = [
        for (final point in points) [point.dx, point.dy],
      ];

      void checkIdentity(String phase) {
        for (final name in _buttonKeys) {
          expect(
            identical(tester.element(_barItem(name)), items[name]),
            isTrue,
            reason: '$phase must retain $name tooltip state',
          );
          expect(
            identical(tester.element(_barButton(name)), buttons[name]),
            isTrue,
            reason: '$phase must retain $name button and ink state',
          );
        }
        expect(tester.takeException(), isNull, reason: phase);
        identityChecks.add(phase);
      }

      Future<void> proof(String phase) async {
        checkIdentity(phase);
        final pixels = await _captureBar(phase, width);
        expect(pixels.width, width);
        expect(pixels.height, 40);
        final actual = [for (final point in points) pixels.at(point)];
        for (var point = 0; point < actual.length; point++) {
          for (var channel = 0; channel < 3; channel++) {
            expect(
              (actual[point][channel] - expectedRgb[channel]).abs(),
              lessThanOrEqualTo(3),
              reason:
                  '$phase bar point ${points[point]} must remain opaque Material background: ${actual[point]} vs $expectedRgb',
            );
          }
        }
        (evidence['proofs'] as List).add({
          'phase': phase,
          'rgb': actual,
          'nativeState': await _state(),
        });
      }

      await proof('bar-baseline');
      for (final reduced in [false, true]) {
        final label = reduced ? 'reduced' : 'animated';
        controller.setReduceMotion(reduced);
        await tester.pump();
        checkIdentity('$label-preferences');
        controller.selectPanel(ShellPanel.launcher);
        await tester.pump();
        checkIdentity('$label-launcher-request');
        await _waitNative(tester, expanded: true);
        await tester.pump(const Duration(milliseconds: 350));
        await proof('$label-launcher-open');
        controller.selectPanel(ShellPanel.quickSettings);
        await tester.pump();
        checkIdentity('$label-switch-to-control-center');
        await _waitNative(tester, expanded: true);
        await tester.pump(const Duration(milliseconds: 350));
        await proof('$label-control-center-open');
        controller.closePanel();
        await tester.pump();
        checkIdentity('$label-close-request');
        await _waitNative(tester, expanded: false);
        await proof('$label-panels-closed');
        await tester.tap(_barButton('bar-quick-settings'));
        await tester.pump();
        checkIdentity('$label-control-center-click');
        await _waitNative(tester, expanded: true);
        await proof('$label-control-center-click-open');
        controller.closePanel();
        await tester.pump();
        await _waitNative(tester, expanded: false);
        await proof('$label-control-center-click-closed');
      }

      controller.setReduceMotion(false);
      controller.selectPanel(ShellPanel.launcher);
      await tester.pump();
      await _waitNative(tester, expanded: true);
      controller.closePanel();
      await tester.pump(const Duration(milliseconds: 20));
      checkIdentity('rapid-launcher-close');
      controller.selectPanel(ShellPanel.launcher);
      await tester.pump(const Duration(milliseconds: 20));
      checkIdentity('rapid-launcher-reopen');
      await _waitNative(tester, expanded: true);
      await tester.pump(const Duration(milliseconds: 350));
      await proof('rapid-launcher-reopened');
      controller.closePanel();
      await tester.pump(const Duration(milliseconds: 20));
      checkIdentity('motion-reduction-during-close');
      controller.setReduceMotion(true);
      await tester.pump();
      await _waitNative(tester, expanded: false);
      await proof('motion-reduction-closed');

      pointer = TestGesture(
        dispatcher: tester.sendEventToBinding,
        kind: PointerDeviceKind.mouse,
        pointer: 9100,
        device: 9100,
      );
      await pointer.addPointer(location: const Offset(5, 20));
      await pointer.moveTo(tester.getCenter(_barButton('bar-launcher')));
      await _wait(
        tester,
        () => controller.barTooltip?.message == '应用与搜索',
        'launcher hover tooltip',
      );
      await _waitNative(tester, expanded: false, tooltip: true);
      await proof('bar-tooltip-visible');
      await tester.pump(const Duration(milliseconds: 500));
      await proof('bar-tooltip-retained');
      await pointer.moveTo(const Offset(5, 20));
      await _wait(tester, () => controller.barTooltip == null, 'tooltip exits');
      await _waitNative(tester, expanded: false);
      await proof('bar-tooltip-hidden');
      await pointer.removePointer();
      pointer = null;
      evidence['identityChecks'] = identityChecks;
      evidence['allBarElementsRetained'] = true;
      evidence['finalState'] = await _waitNative(tester, expanded: false);
      evidence['passed'] = true;
    } finally {
      if (pointer != null) {
        await pointer.removePointer();
      }
      evidence['identityChecks'] = identityChecks;
      await File(
        '${Platform.environment['CLARION_SHELL_CAPTURE_DIR']}/bar-native-evidence.json',
      ).writeAsString(
        '${const JsonEncoder.withIndent('  ').convert(evidence)}\n',
      );
      await tester.pumpWidget(const SizedBox.shrink());
    }
  });
}
