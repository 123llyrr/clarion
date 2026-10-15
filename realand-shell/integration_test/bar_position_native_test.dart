// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:convert';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:realand_shell/main.dart';
import 'package:realand_shell/models.dart';
import 'package:realand_shell/preferences.dart';
import 'package:realand_shell/services/desktop_apps.dart';
import 'package:realand_shell/services/power_profiles_service.dart';
import 'package:realand_shell/services/system_service.dart';
import 'package:realand_shell/services/wallpaper_service.dart';
import 'package:realand_shell/state/shell_controller.dart';

const _window = MethodChannel('realand.shell/window');
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
    if (DateTime.now().isAfter(deadline)) fail('Timed out: $reason');
    await tester.pump(const Duration(milliseconds: 16));
  }
}

Future<Map<String, dynamic>> _waitNative(
  WidgetTester tester, {
  required BarPosition position,
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
    if (state['barPosition'] != position.name ||
        state['expanded'] != expanded ||
        state['tooltipVisible'] != tooltip ||
        state['height'] != height) {
      continue;
    }
    expect(state['keyboard'], expanded ? 'Exclusive' : 'None');
    expect(state['inputHeight'], expanded ? height : 40);
    expect(
      state['inputY'],
      !expanded && position == BarPosition.bottom ? (height as int) - 40 : 0,
    );
    expect(state['layer'], expanded ? 'Overlay' : 'Top');
    expect(state['exclusiveZone'], 40);
    expect(state['anchorTop'], position == BarPosition.top);
    expect(state['anchorBottom'], position == BarPosition.bottom);
    final geometry = state['monitorGeometry'] as Map;
    final origin = (await _window.invokeMapMethod<String, dynamic>(
      'surfaceOrigin',
    ))!;
    expect(origin['x'], geometry['x']);
    expect(
      origin['y'],
      (geometry['y'] as int) +
          (position == BarPosition.bottom
              ? (geometry['height'] as int) - (height as int)
              : 0),
    );
    return state;
  } while (DateTime.now().isBefore(deadline));
  fail('Native state mismatch: $state');
}

Future<void> _waitPreference(WidgetTester tester, BarPosition position) async {
  final deadline = DateTime.now().add(const Duration(seconds: 15));
  while (DateTime.now().isBefore(deadline)) {
    await tester.pump(const Duration(milliseconds: 16));
    if (await ShellPreferences.file!.exists() &&
        (await ShellPreferences.load()).barPosition == position) {
      return;
    }
  }
  fail('Position ${position.name} was not persisted to the private XDG config');
}

Future<List<List<int>>> _captureBar(
  String name,
  Map<String, dynamic> state,
) async {
  final geometry = state['monitorGeometry'] as Map;
  final width = geometry['width'] as int;
  final y =
      (geometry['y'] as int) +
      (state['barPosition'] == 'bottom' ? (geometry['height'] as int) - 40 : 0);
  final file = File(
    '${Platform.environment['REALAND_SHELL_CAPTURE_DIR']}/$name.png',
  );
  final result = await Process.run('grim', [
    '-s',
    '1',
    '-g',
    '${geometry['x']},$y ${width}x40',
    file.path,
  ]).timeout(const Duration(seconds: 15));
  expect(result.exitCode, 0, reason: '${result.stdout}\n${result.stderr}');
  final codec = await ui.instantiateImageCodec(await file.readAsBytes());
  final frame = await codec.getNextFrame();
  try {
    expect(frame.image.width, width);
    expect(frame.image.height, 40);
    final bytes = (await frame.image.toByteData(
      format: ui.ImageByteFormat.rawRgba,
    ))!.buffer.asUint8List();
    // Row one is outside the 36 px button ink, on either display edge.
    return [
      for (final x in [5, width ~/ 4, width ~/ 2, width * 3 ~/ 4, width - 5])
        bytes.sublist((width + x) * 4, (width + x) * 4 + 3),
    ];
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
        id: 'position-fixture-$index',
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
  testWidgets('native top and bottom bar anchor, modal, tooltip and persistence', (
    tester,
  ) async {
    expect(Platform.environment['REALAND_SHELL_ISOLATED_TEST'], '1');
    expect(
      Platform.environment['WAYLAND_DISPLAY'],
      isNot(Platform.environment['REALAND_SHELL_PARENT_WAYLAND_DISPLAY']),
    );
    expect(
      Platform.environment['REALAND_SOCKET'],
      isNot(Platform.environment['REALAND_SHELL_PARENT_SOCKET']),
    );
    expect(
      Platform.environment['DBUS_SESSION_BUS_ADDRESS'],
      isNot(Platform.environment['REALAND_SHELL_PARENT_DBUS_ADDRESS']),
    );
    final root = Platform.environment['REALAND_SHELL_TEST_ROOT']!;
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
          'WidgetTester clicks restricted to this binding; no physical '
          'pointer or live session keyboard injection. Tooltip uses a fixture '
          'owner on the real controller to validate native/render geometry.',
      'condition':
          'Debug GTK Flutter runner; nested Realand Vulkan at scale 1; '
          'Material dark theme, framePolicy=onlyPumps. Actual grim edge crops '
          'validate settled pixels. Fractional scale and physical hover resize '
          'continuity are not certified by this isolated fixture.',
      'proofs': <Map<String, dynamic>>[],
    };
    final identityChecks = <String>[];
    final items = <String, Element>{};
    final buttons = <String, Element>{};
    try {
      await tester.pumpWidget(
        ShellApplication(
          controller: controller,
          preferences: const ShellPreferences(themeMode: ThemeMode.dark),
          persist: true,
        ),
      );
      await controller.start();
      await _wait(
        tester,
        () => controller.connected && controller.apps.length == 24,
        'nested IPC and fixture applications',
      );
      final initial = await _waitNative(
        tester,
        position: BarPosition.top,
        expanded: false,
      );
      expect(initial['scaleFactor'], 1);
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
      evidence['barColorRgb'] = expectedRgb;

      void identity(String phase) {
        for (final name in _buttonKeys) {
          expect(
            identical(tester.element(_barItem(name)), items[name]),
            isTrue,
            reason: '$phase retains $name tooltip state',
          );
          expect(
            identical(tester.element(_barButton(name)), buttons[name]),
            isTrue,
            reason: '$phase retains $name ink state',
          );
        }
        expect(tester.takeException(), isNull, reason: phase);
        identityChecks.add(phase);
      }

      Future<void> proof(String phase, Map<String, dynamic> state) async {
        identity(phase);
        final actual = await _captureBar(phase, state);
        for (final point in actual) {
          for (var channel = 0; channel < 3; channel++) {
            expect(
              (point[channel] - expectedRgb[channel]).abs(),
              lessThanOrEqualTo(3),
              reason: '$phase remains opaque: $point',
            );
          }
        }
        (evidence['proofs'] as List).add({
          'phase': phase,
          'rgb': actual,
          'nativeState': state,
        });
      }

      await proof('top-baseline', initial);
      for (final position in BarPosition.values) {
        controller.setBarPosition(position);
        await tester.pump();
        var state = await _waitNative(
          tester,
          position: position,
          expanded: false,
        );
        await proof('${position.name}-closed', state);
        for (final reduced in [false, true]) {
          final label = '${position.name}-${reduced ? 'reduced' : 'animated'}';
          controller.setReduceMotion(reduced);
          await tester.pump();
          controller.selectPanel(ShellPanel.launcher);
          await tester.pump();
          identity('$label-launcher-request');
          state = await _waitNative(tester, position: position, expanded: true);
          await tester.pump(const Duration(milliseconds: 350));
          await proof('$label-launcher-open', state);
          controller.selectPanel(ShellPanel.quickSettings);
          await tester.pump();
          state = await _waitNative(tester, position: position, expanded: true);
          await tester.pump(const Duration(milliseconds: 350));
          final bar = tester.getRect(
            find.byKey(const ValueKey('shell-system-bar')),
          );
          final panel = tester.getRect(
            find.byKey(const ValueKey('shell-panel-position')),
          );
          if (position == BarPosition.bottom) {
            expect(panel.bottom, lessThan(bar.top));
          } else {
            expect(panel.top, greaterThan(bar.bottom));
          }
          await proof('$label-control-center-open', state);
          controller.closePanel();
          await tester.pump();
          state = await _waitNative(
            tester,
            position: position,
            expanded: false,
          );
          await proof('$label-closed', state);
        }
        final owner = Object();
        controller.showBarTooltip(
          owner,
          '应用与搜索',
          tester.getCenter(_barButton('bar-launcher')),
        );
        await tester.pump();
        state = await _waitNative(
          tester,
          position: position,
          expanded: false,
          tooltip: true,
        );
        final bubble = tester.getRect(
          find.byKey(const ValueKey('bar-tooltip-bubble')),
        );
        final bar = tester.getRect(
          find.byKey(const ValueKey('shell-system-bar')),
        );
        if (position == BarPosition.bottom) {
          expect(bubble.bottom, lessThan(bar.top));
        } else {
          expect(bubble.top, greaterThan(bar.bottom));
        }
        await proof('${position.name}-tooltip', state);
        controller.hideBarTooltip(owner);
        await tester.pump();
        state = await _waitNative(tester, position: position, expanded: false);
        await proof('${position.name}-tooltip-closed', state);
      }

      controller.selectPanel(ShellPanel.quickSettings);
      await tester.pump();
      await _waitNative(tester, position: BarPosition.bottom, expanded: true);
      for (final position in [BarPosition.top, BarPosition.bottom]) {
        final control = find.byKey(const ValueKey('bar-position'));
        final label = find.descendant(
          of: control,
          matching: find.text(position == BarPosition.top ? '顶部' : '底部'),
        );
        await tester.ensureVisible(label);
        await tester.tap(label);
        await tester.pump();
        expect(controller.panel, ShellPanel.quickSettings);
        expect(controller.barPosition, position);
        final state = await _waitNative(
          tester,
          position: position,
          expanded: true,
        );
        await _waitPreference(tester, position);
        await proof('${position.name}-control-center-position-ui', state);
      }
      evidence['privatePreference'] = jsonDecode(
        await ShellPreferences.file!.readAsString(),
      );
      expect((await ShellPreferences.load()).barPosition, BarPosition.bottom);
      evidence['persistentPositionUiPassed'] = true;
      controller.closePanel();
      await tester.pump();
      final finalState = await _waitNative(
        tester,
        position: BarPosition.bottom,
        expanded: false,
      );
      await proof('bottom-final-closed', finalState);
      evidence['finalState'] = finalState;
      evidence['allBarElementsRetained'] = true;
      evidence['passed'] = true;
    } finally {
      evidence['identityChecks'] = identityChecks;
      await File(
        '${Platform.environment['REALAND_SHELL_CAPTURE_DIR']}/'
        'bar-position-native-evidence.json',
      ).writeAsString(
        '${const JsonEncoder.withIndent('  ').convert(evidence)}\n',
      );
      await tester.pumpWidget(const SizedBox.shrink());
    }
  });
}
