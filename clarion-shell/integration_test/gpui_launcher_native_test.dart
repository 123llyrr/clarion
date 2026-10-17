// SPDX-License-Identifier: GPL-3.0-or-later
// Run only through scripts/test-launcher-apps.py --shell-native-test. All input
// and GIO invocations below belong to that harness's private compositor/catalog.
import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:clarion_shell/main.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/preferences.dart';
import 'package:clarion_shell/services/launcher_bridge.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/ui/mac_launcher_panel.dart';

import 'helpers/launcher_glass_checks.dart';

const _window = MethodChannel('clarion.shell/window');
const _appId = r'org.clarion.Alpha ${HOME} $(touch literal).desktop';

Future<Map<String, dynamic>> _state() async => Map<String, dynamic>.from(
  (await _window.invokeMapMethod<String, dynamic>('windowState'))!,
);

Future<void> _wait(
  WidgetTester tester,
  String description,
  bool Function() ready,
) async {
  final deadline = DateTime.now().add(const Duration(seconds: 12));
  do {
    await tester.pump(const Duration(milliseconds: 16));
    if (ready()) return;
  } while (DateTime.now().isBefore(deadline));
  fail('Timed out waiting for $description');
}

Future<Map<String, dynamic>> _waitState(
  WidgetTester tester,
  bool Function(Map<String, dynamic>) ready,
) async {
  final deadline = DateTime.now().add(const Duration(seconds: 12));
  do {
    await tester.pump(const Duration(milliseconds: 16));
    final state = await _state();
    if (ready(state)) return state;
  } while (DateTime.now().isBefore(deadline));
  fail('Native launcher state did not settle: ${await _state()}');
}

Future<List<Map<String, dynamic>>> _layers(Map<String, String> env) async {
  final result = await Process.run(env['CLARION_SHELL_CLARION_BIN']!, [
    'msg',
    '--json',
    'layers',
  ], environment: env).timeout(const Duration(seconds: 5));
  expect(result.exitCode, 0, reason: '${result.stdout}\n${result.stderr}');
  final decoded = jsonDecode(result.stdout as String);
  final list = decoded is List ? decoded : (decoded as Map)['Layers'] as List;
  return [for (final entry in list) Map<String, dynamic>.from(entry as Map)];
}

Future<List<Map<String, dynamic>>> _waitLayers(
  WidgetTester tester,
  Map<String, String> env,
  bool Function(List<Map<String, dynamic>>) ready,
) async {
  final deadline = DateTime.now().add(const Duration(seconds: 8));
  List<Map<String, dynamic>> layers = [];
  do {
    await tester.pump(const Duration(milliseconds: 16));
    layers = await _layers(env);
    if (ready(layers)) return layers;
  } while (DateTime.now().isBefore(deadline));
  fail('Compositor launcher layers did not settle: $layers');
}

bool _layer(
  List<Map<String, dynamic>> layers,
  String namespace,
  String keyboard,
  String output,
) => layers.any(
  (layer) =>
      layer['namespace'] == namespace &&
      layer['output'] == output &&
      layer['layer'] == 'Overlay' &&
      layer['keyboard_interactivity'] == keyboard,
);

Future<void> _capture(Map<String, String> env, String name) async {
  final file = File('${env['CLARION_SHELL_CAPTURE_DIR']}/$name.png');
  final result = await Process.run(env['CLARION_SHELL_GRIM'] ?? 'grim', [
    '-o',
    env['CLARION_LAUNCHER_TEST_OUTPUT'] ?? 'winit',
    file.path,
  ], environment: env).timeout(const Duration(seconds: 5));
  expect(result.exitCode, 0, reason: '${result.stdout}\n${result.stderr}');
  expect(await file.length(), greaterThan(1000));
}

Future<void> _key(Map<String, String> env, List<String> arguments) async {
  final result = await Process.run(
    env['CLARION_SHELL_WTYPE'] ?? 'wtype',
    arguments,
    environment: env,
  ).timeout(const Duration(seconds: 5));
  expect(result.exitCode, 0, reason: '${result.stdout}\n${result.stderr}');
}

Future<Map<String, Object?>> _clickApps(
  WidgetTester tester,
  IntegrationTestWidgetsFlutterBinding binding,
  ShellController controller,
  Map<String, String> env,
) async {
  final apps = find.byKey(const ValueKey('dock-launcher'));
  expect(apps, findsOneWidget);
  final view = View.of(tester.element(apps));
  final state = await _state();
  expect(view.viewId, state['dockViewId']);
  final dock = state['dock'] as Map;
  final logical = view.physicalSize / view.devicePixelRatio;
  expect(logical.width, closeTo((dock['width'] as num).toDouble(), .01));
  expect(logical.height, closeTo((dock['height'] as num).toDouble(), .01));
  final origin = (await _window.invokeMapMethod<String, dynamic>(
    'surfaceOrigin',
    'dock',
  ))!;
  final local = tester.getCenter(apps);
  final target =
      local +
      Offset(
        (origin['outputX'] as num).toDouble(),
        (origin['outputY'] as num).toDouble(),
      );
  final monitor = state['monitorGeometry'] as Map;
  final outputSize = Size(
    (monitor['width'] as num).toDouble(),
    (monitor['height'] as num).toDouble(),
  );
  expect((Offset.zero & outputSize).contains(target), isTrue);
  final previous = binding.shouldPropagateDevicePointerEvents;
  binding.shouldPropagateDevicePointerEvents = true;
  try {
    final result = await Process.run(env['CLARION_SHELL_POINTER_HELPER']!, [
      '${target.dx.round()}',
      '${target.dy.round()}',
      '${outputSize.width.round()}',
      '${outputSize.height.round()}',
    ], environment: env).timeout(const Duration(seconds: 3));
    expect(result.exitCode, 0, reason: '${result.stdout}\n${result.stderr}');
    await _wait(
      tester,
      'real Dock Apps callback',
      () => controller.panel == ShellPanel.launcher,
    );
  } finally {
    binding.shouldPropagateDevicePointerEvents = previous;
  }
  return {
    'route': 'isolated compositor virtual pointer',
    'viewId': view.viewId,
    'surfaceOrigin': origin,
    'local': {'x': local.dx, 'y': local.dy},
    'outputPoint': {'x': target.dx, 'y': target.dy},
  };
}

List<Map<String, dynamic>> _gioCalls(File log) => log.existsSync()
    ? [
        for (final line in log.readAsLinesSync())
          if (line.isNotEmpty)
            Map<String, dynamic>.from(jsonDecode(line) as Map),
      ]
    : [];

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  final devices = NativeViewDeviceFilter(binding);
  tearDownAll(devices.restore);
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.onlyPumps;
  testWidgets('production bridge, real Dock input, launch and fallback', (
    tester,
  ) async {
    final env = Map<String, String>.from(Platform.environment);
    expect(Platform.isLinux, isTrue);
    expect(env['CLARION_SHELL_ISOLATED_TEST'], '1');
    for (final key in [
      'WAYLAND_DISPLAY',
      'CLARION_SOCKET',
      'CLARION_SHELL_PARENT_WAYLAND_DISPLAY',
      'CLARION_SHELL_PARENT_SOCKET',
      'CLARION_SHELL_TEST_ROOT',
      'CLARION_SHELL_CAPTURE_DIR',
      'CLARION_SHELL_CLARION_BIN',
      'CLARION_LAUNCHER_BIN',
      'CLARION_SHELL_POINTER_HELPER',
      'CLARION_FAKE_GIO_LOG',
      'CLARION_FAKE_GIO_FAIL',
      'XDG_DATA_HOME',
      'XDG_RUNTIME_DIR',
    ]) {
      expect(env[key], isNotEmpty, reason: '$key is required');
    }
    expect(
      env['WAYLAND_DISPLAY'],
      isNot(env['CLARION_SHELL_PARENT_WAYLAND_DISPLAY']),
    );
    expect(env['CLARION_SOCKET'], isNot(env['CLARION_SHELL_PARENT_SOCKET']));
    for (final key in [
      'CLARION_LAUNCHER_BIN',
      'CLARION_SHELL_POINTER_HELPER',
    ]) {
      expect(env[key]!.startsWith('/'), isTrue);
      expect(await File(env[key]!).exists(), isTrue);
    }
    final app = File('${env['XDG_DATA_HOME']}/applications/$_appId');
    expect(
      await app.exists(),
      isTrue,
      reason: 'Only the private fake-GIO catalog is allowed',
    );
    final firstPath = env['PATH']!.split(':').first;
    final gio = File('$firstPath/gio');
    expect(await gio.exists(), isTrue);
    expect(await gio.readAsString(), contains('CLARION_FAKE_GIO_LOG'));
    final gioLog = File(env['CLARION_FAKE_GIO_LOG']!);
    final failure = File(env['CLARION_FAKE_GIO_FAIL']!);
    expect(await failure.exists(), isFalse);
    final beforeGio = _gioCalls(gioLog).length;
    final captures = Directory(env['CLARION_SHELL_CAPTURE_DIR']!);
    await captures.create(recursive: true);
    final output = env['CLARION_LAUNCHER_TEST_OUTPUT'] ?? 'winit';
    final saved = await ShellPreferences.load();
    final bridge = LauncherBridge();
    final controller =
        ShellController(
            demo: true,
            launcherBridge: bridge,
            tray: false,
            notifications: false,
            powerProfilesEnabled: false,
          )
          ..setShellStyle(ShellStyle.macos)
          ..setThemeMode(saved.themeMode)
          ..setRecentAppIds(saved.recentAppIds)
          ..setReduceMotion(true);
    controller.setMacPreferences(
      controller.macPreferences.copyWith(magnification: false),
    );
    final evidence = <String, Object?>{
      'passed': false,
      'output': output,
      'appId': _appId,
      'scope': 'Real production LauncherBridge with a private fake-GIO catalog',
      'pointer': 'Isolated compositor virtual pointer; no physical input claim',
    };
    Future<Map<String, dynamic>> opened() async {
      await _wait(
        tester,
        'production Visible event',
        () => controller.externalLauncherVisible,
      );
      expect(
        controller.externalLauncherDesired,
        isTrue,
        reason: controller.error,
      );
      expect(find.byType(MacLauncherPanel), findsNothing);
      final state = await _waitState(
        tester,
        (state) =>
            state['expanded'] == true &&
            state['keyboard'] == 'None' &&
            state['launcherBackdropRequested'] == true,
      );
      expectLauncherGlass(state);
      expect(state['keyboardInteractivityOverride'], 'none');
      expect((state['dock'] as Map)['keyboard'], 'None');
      expect((state['dock'] as Map)['inputRegions'], isNotEmpty);
      evidence['layers'] = await _waitLayers(
        tester,
        env,
        (layers) =>
            _layer(layers, 'clarion-shell', 'None', output) &&
            _layer(layers, 'clarion-shell-dock', 'None', output) &&
            _layer(layers, 'clarion-launcher', 'Exclusive', output),
      );
      await tester.pump(const Duration(milliseconds: 100));
      return state;
    }

    Future<Map<String, dynamic>> closed() async {
      await _wait(
        tester,
        'Hidden event closes Shell',
        () => controller.panel == ShellPanel.none,
      );
      final state = await _waitState(
        tester,
        (state) =>
            state['expanded'] == false &&
            state['launcherBackdropRequested'] == false,
      );
      expect(state['keyboard'], 'None');
      expect(state['keyboardInteractivityOverride'], isNull);
      expect((state['dock'] as Map)['layer'], 'Top');
      await _waitLayers(
        tester,
        env,
        (layers) =>
            !layers.any((layer) => layer['namespace'] == 'clarion-launcher'),
      );
      expect(
        controller.externalLauncherEnabled,
        isTrue,
        reason: controller.error,
      );
      return state;
    }

    try {
      await tester.pumpWidget(
        ShellHost(
          controller: controller,
          preferences: ShellPreferences(
            shellStyle: ShellStyle.macos,
            themeMode: saved.themeMode,
            seed: saved.seed,
            reduceMotion: true,
          ),
          persist: false,
        ),
        wrapWithView: false,
      );
      evidence['initial'] = await _waitState(
        tester,
        (state) =>
            state['expanded'] == false &&
            (state['dock'] as Map?)?['ready'] == true,
      );
      evidence['dockOpen'] = await _clickApps(tester, binding, controller, env);
      evidence['opened'] = await opened();
      await _capture(env, 'gpui-launcher-native-apps');

      await _key(env, ['uniquealpha']);
      await _key(env, ['-M', 'ctrl', '-k', 'a', '-k', 'c', '-m', 'ctrl']);
      final copied = await Process.run(
        env['CLARION_SHELL_WL_PASTE'] ?? 'wl-paste',
        ['--no-newline'],
        environment: env,
      ).timeout(const Duration(seconds: 2));
      expect(copied.exitCode, 0);
      expect(
        copied.stdout,
        'uniquealpha',
        reason: 'GPUI receives focus after Dock click',
      );
      await tester.pump(const Duration(milliseconds: 100));
      await _capture(env, 'gpui-launcher-native-search');
      await failure.writeAsString('fail only the private GIO fixture');
      await _key(env, ['-k', 'Return']);
      await _wait(
        tester,
        'launch failure event',
        () => controller.error?.contains('应用启动失败') == true,
      );
      expect(controller.externalLauncherDesired, isTrue);
      expect(controller.externalLauncherVisible, isTrue);
      expect(_gioCalls(gioLog).length, beforeGio + 1);
      expect(_gioCalls(gioLog).last['argv'], ['launch', app.path]);
      evidence['launchError'] = controller.error;
      await _capture(env, 'gpui-launcher-native-launch-error');
      await failure.delete();
      await _key(env, ['-k', 'Return']);
      evidence['afterLaunch'] = await closed();
      expect(controller.recentAppIds.first, _appId);
      expect(controller.error, isNull);
      final calls = _gioCalls(gioLog).skip(beforeGio).toList();
      expect(
        calls,
        hasLength(2),
        reason: 'One launch attempt per Enter, with no retry after dispatch',
      );
      for (final call in calls) {
        expect(call['argv'], ['launch', app.path]);
      }
      evidence['gioCalls'] = calls;
      evidence['recentApps'] = controller.recentAppIds;

      evidence['dockReopen'] = await _clickApps(
        tester,
        binding,
        controller,
        env,
      );
      evidence['reopened'] = await opened();
      await _key(env, ['-k', 'Escape']);
      evidence['afterEscape'] = await closed();
      await _capture(env, 'gpui-launcher-native-closed');

      await _clickApps(tester, binding, controller, env);
      await opened();
      controller.setUseExternalLauncher(false);
      await _wait(
        tester,
        'classic Flutter fallback',
        () => find.byType(MacLauncherPanel).evaluate().length == 1,
      );
      final legacy = await _waitState(
        tester,
        (state) =>
            state['expanded'] == true &&
            state['keyboard'] == 'Exclusive' &&
            state['keyboardInteractivityOverride'] == null,
      );
      await _waitLayers(
        tester,
        env,
        (layers) =>
            !layers.any((layer) => layer['namespace'] == 'clarion-launcher'),
      );
      expect(controller.panel, ShellPanel.launcher);
      expect(controller.useExternalLauncher, isFalse);
      evidence['legacy'] = legacy;
      await _capture(env, 'gpui-launcher-native-legacy');
      controller.setUseExternalLauncher(true);
      evidence['reenabled'] = await opened();
      expect(controller.useExternalLauncher, isTrue);
      await _capture(env, 'gpui-launcher-native-reenabled');
      await _key(env, ['-k', 'Escape']);
      evidence['restored'] = await closed();
      evidence['passed'] = true;
    } catch (error, stack) {
      evidence['error'] = '$error';
      evidence['stack'] = '$stack';
      rethrow;
    } finally {
      if (await failure.exists()) await failure.delete();
      controller.closePanel();
      await tester.pumpWidget(
        const ViewCollection(views: []),
        wrapWithView: false,
      );
      await bridge.dispose();
      controller.dispose();
      await _window.invokeMethod<void>('setExpanded', false);
      await _window.invokeMethod<void>('setShellStyle', 'material');
      evidence['ignoredUnattachedDeviceEvents'] = devices.ignored;
      await File(
        '${captures.path}/gpui-launcher-native-evidence.json',
      ).writeAsString(const JsonEncoder.withIndent('  ').convert(evidence));
    }
  });
}
