// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:clarion_shell/main.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/preferences.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/ui/shell_geometry.dart';

const _window = MethodChannel('clarion.shell/window');

Future<Map<String, dynamic>> _windowState() async => Map<String, dynamic>.from(
  (await _window.invokeMapMethod<String, dynamic>('windowState'))!,
);

Future<Map<String, dynamic>> _waitForState(
  WidgetTester tester,
  bool Function(Map<String, dynamic>) matches,
) async {
  final deadline = DateTime.now().add(const Duration(seconds: 15));
  Map<String, dynamic> latest = const {};
  do {
    await tester.pump(const Duration(milliseconds: 100));
    latest = await _windowState();
    if (matches(latest)) return latest;
  } while (DateTime.now().isBefore(deadline));
  fail('Native window did not reach the expected state: $latest');
}

Future<void> _realKey(List<String> arguments) async {
  final process = await Process.run(
    Platform.environment['CLARION_SHELL_WTYPE'] ?? 'wtype',
    arguments,
  ).timeout(const Duration(seconds: 10));
  expect(process.exitCode, 0, reason: '${process.stdout}\n${process.stderr}');
}

Future<Map<String, dynamic>> _outputScale(String scale) async {
  final binary = Platform.environment['CLARION_SHELL_CLARION_BIN'] ?? 'clarion';
  final output = Platform.environment['CLARION_SHELL_TEST_OUTPUT'] ?? 'winit';
  final result = await Process.run(binary, [
    'msg',
    'output',
    output,
    'scale',
    scale,
  ]).timeout(const Duration(seconds: 10));
  expect(result.exitCode, 0, reason: '${result.stdout}\n${result.stderr}');
  final snapshot = await Process.run(binary, ['msg', '-j', 'outputs']);
  expect(snapshot.exitCode, 0, reason: '${snapshot.stderr}');
  final outputs = jsonDecode(snapshot.stdout as String) as Map<String, dynamic>;
  final state = outputs[output] as Map;
  final mode = (state['modes'] as List)[state['current_mode'] as int] as Map;
  // Smithay's actual xdg-output/layer geometry uses to_i32_round. IPC logical
  // dimensions truncate to integers, so fractional scaling can differ by one
  // pixel from the protocol extent configuring GTK.
  final expectedHeight = ((mode['height'] as int) / double.parse(scale))
      .round();
  final evidence = <String, dynamic>{
    'requestedScale': scale,
    'ipcLogical': state['logical'],
    'physicalMode': mode,
    'expectedProtocolHeight': expectedHeight,
    'windowBeforeSettling': await _windowState(),
  };
  final directory = Platform.environment['CLARION_SHELL_CAPTURE_DIR'];
  await File(
    '$directory/scale-$scale-request.json',
  ).writeAsString('${const JsonEncoder.withIndent('  ').convert(evidence)}\n');
  return evidence;
}

// Flutter's official integration_test screenshot plugin supports Android/iOS,
// so Linux captures the actual nested compositor's output using screencopy.
// The isolation assertion below prevents this helper reading the user desktop.
Future<void> captureScreenshot(String name) async {
  final root = Platform.environment['CLARION_SHELL_CAPTURE_DIR'];
  expect(root, isNotNull, reason: 'Set a directory for native visual evidence');
  final directory = Directory(root!);
  await directory.create(recursive: true);
  final result = await Process.run('grim', [
    '-o',
    Platform.environment['CLARION_SHELL_TEST_OUTPUT'] ?? 'winit',
    '${directory.path}/$name.png',
  ]).timeout(const Duration(seconds: 15));
  expect(result.exitCode, 0, reason: '${result.stdout}\n${result.stderr}');
  expect(await File('${directory.path}/$name.png').length(), greaterThan(100));
}

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('native layer window expands, focuses, switches and closes', (
    tester,
  ) async {
    expect(Platform.isLinux, isTrue);
    expect(Platform.environment['CLARION_SHELL_ISOLATED_TEST'], '1');
    expect(Platform.environment['WAYLAND_DISPLAY'], isNotEmpty);
    expect(
      Platform.environment['WAYLAND_DISPLAY'],
      isNot(Platform.environment['CLARION_SHELL_PARENT_WAYLAND_DISPLAY']),
      reason: 'This test must use a newly started nested compositor',
    );
    expect(Platform.environment['CLARION_SOCKET'], isNotEmpty);
    expect(
      Platform.environment['CLARION_SOCKET'],
      isNot(Platform.environment['CLARION_SHELL_PARENT_SOCKET']),
    );

    final controller = ShellController(notifications: false, tray: false);
    const preferences = ShellPreferences(
      themeMode: ThemeMode.dark,
      reduceMotion: true,
    );
    controller.setThemeMode(preferences.themeMode);
    controller.setReduceMotion(preferences.reduceMotion);
    await tester.pumpWidget(
      ShellApplication(
        controller: controller,
        preferences: preferences,
        persist: false,
      ),
    );
    await controller.start();
    final bar = await _waitForState(
      tester,
      (state) =>
          state['height'] == ShellGeometry.barHeight &&
          state['expanded'] == false,
    );
    expect(bar['preview'], isFalse);
    expect(bar['keyboard'], 'None');
    expect(bar['exclusiveZone'], ShellGeometry.barHeight);
    expect(controller.connected, isTrue);

    await tester.tap(find.byKey(const ValueKey('bar-launcher')));
    final launcher = await _waitForState(
      tester,
      (state) =>
          state['expanded'] == true &&
          (state['height'] as int) > ShellGeometry.barHeight,
    );
    await tester.pump(const Duration(milliseconds: 200));
    expect(launcher['keyboard'], 'Exclusive');
    expect(launcher['exclusiveZone'], ShellGeometry.barHeight);
    final search = find.byKey(const ValueKey('launcher-search'));
    expect(search, findsOneWidget);
    expect(tester.widget<TextField>(search).focusNode!.hasFocus, isTrue);
    await captureScreenshot('launcher');

    // Go through Wayland's real virtual-keyboard protocol, rather than only
    // injecting a Flutter key event. This catches GTK/compositor focus mistakes.
    await _realKey(['native-shell-test']);
    await tester.pump(const Duration(milliseconds: 200));
    expect(
      tester.widget<TextField>(search).controller!.text,
      'native-shell-test',
    );
    await _realKey(['-k', 'Escape']);
    await _waitForState(
      tester,
      (state) =>
          state['expanded'] == false &&
          state['height'] == ShellGeometry.barHeight,
    );
    expect(controller.panel, ShellPanel.none);
    expect(find.byKey(const ValueKey('launcher-search')), findsNothing);

    await tester.tap(find.byKey(const ValueKey('bar-quick-settings')));
    final controls = await _waitForState(
      tester,
      (state) =>
          state['expanded'] == true &&
          (state['height'] as int) > ShellGeometry.barHeight,
    );
    expect(controller.panel, ShellPanel.quickSettings);
    expect(controls['keyboard'], 'Exclusive');
    await captureScreenshot('control');

    await tester.tap(find.byKey(const ValueKey('bar-notifications')));
    await tester.pump(const Duration(milliseconds: 200));
    expect(controller.panel, ShellPanel.notifications);
    expect((await _windowState())['expanded'], isTrue);
    await captureScreenshot('notifications');
    await tester.tap(find.byTooltip('关闭'));
    final closed = await _waitForState(
      tester,
      (state) =>
          state['expanded'] == false &&
          state['height'] == ShellGeometry.barHeight,
    );
    expect(closed['keyboard'], 'None');
    expect(closed['exclusiveZone'], ShellGeometry.barHeight);
    expect(tester.takeException(), isNull);

    // This changes only the output in the isolated compositor. Validate GTK's
    // real geometry notifications and Flutter's real viewport constraints.
    final scales = <String, Map<String, dynamic>>{};
    await tester.tap(find.byKey(const ValueKey('bar-quick-settings')));
    await _waitForState(
      tester,
      (state) =>
          state['expanded'] == true && state['height'] == launcher['height'],
    );
    try {
      for (final scale in ['1.25', '1.5']) {
        final request = await _outputScale(scale);
        final resized = await _waitForState(
          tester,
          (state) =>
              state['expanded'] == true &&
              state['height'] == request['expectedProtocolHeight'],
        );
        await tester.pump(const Duration(milliseconds: 200));
        expect(resized['keyboard'], 'Exclusive');
        expect(resized['exclusiveZone'], ShellGeometry.barHeight);
        expect(tester.takeException(), isNull);
        scales[scale] = {...request, 'windowAfterSettling': resized};
        await captureScreenshot('control-scale-$scale');
      }
    } finally {
      await _outputScale('1');
    }
    await _waitForState(
      tester,
      (state) => state['height'] == launcher['height'],
    );
    await tester.tap(find.byTooltip('关闭'));
    await _waitForState(
      tester,
      (state) =>
          state['expanded'] == false &&
          state['height'] == ShellGeometry.barHeight,
    );
    expect(tester.takeException(), isNull);

    await tester.tap(find.byKey(const ValueKey('bar-launcher')));
    await _waitForState(
      tester,
      (state) =>
          state['expanded'] == true &&
          (state['height'] as int) > ShellGeometry.barHeight,
    );
    await tester.pump(const Duration(milliseconds: 200));
    await tester.tap(find.byTooltip('工作区概览'));
    final overviewBar = await _waitForState(
      tester,
      (state) =>
          state['expanded'] == false &&
          state['height'] == ShellGeometry.barHeight,
    );
    expect(controller.panel, ShellPanel.none);
    expect(overviewBar['keyboard'], 'None');
    final binary =
        Platform.environment['CLARION_SHELL_CLARION_BIN'] ?? 'clarion';
    final overview = await Process.run(binary, ['msg', '-j', 'overview-state']);
    expect(overview.exitCode, 0, reason: '${overview.stderr}');
    final overviewState = jsonDecode(overview.stdout as String);
    expect(overviewState['is_open'], isTrue);
    final restore = await Process.run(binary, [
      'msg',
      'action',
      'close-overview',
    ]);
    expect(restore.exitCode, 0, reason: '${restore.stderr}');
    binding.reportData = {
      'nativeWindow': {
        'bar': bar,
        'launcher': launcher,
        'scaleChanges': scales,
        'overviewBar': overviewBar,
        'overviewState': overviewState,
      },
    };
    final evidence = Platform.environment['CLARION_SHELL_CAPTURE_DIR'];
    await File('$evidence/native-window-state.json').writeAsString(
      '${const JsonEncoder.withIndent('  ').convert(binding.reportData)}\n',
    );
    // Unmounting disposes the controller, IPC streams and polling timers.
    await tester.pumpWidget(const SizedBox.shrink());
  }, timeout: const Timeout(Duration(minutes: 3)));
}
