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
import 'package:realand_shell/state/shell_controller.dart';

import 'helpers/launcher_glass_checks.dart';
import 'helpers/presented_capture.dart';

const _window = MethodChannel('realand.shell/window');
Future<Map<String, dynamic>> _state() async => Map<String, dynamic>.from(
  (await _window.invokeMapMethod<String, dynamic>('windowState'))!,
);

Future<Map<String, dynamic>> _waitState(
  WidgetTester tester,
  bool Function(Map<String, dynamic>) ready,
) async {
  final deadline = DateTime.now().add(const Duration(seconds: 15));
  do {
    await tester.pump(const Duration(milliseconds: 16));
    final state = await _state();
    if (ready(state)) return state;
  } while (DateTime.now().isBefore(deadline));
  fail('Native launcher state did not settle: ${await _state()}');
}

Future<List<int>> _sample(String name, int y) async {
  final file = '${Platform.environment['REALAND_SHELL_CAPTURE_DIR']}/$name.png';
  final result = await Process.run('grim', ['-o', 'winit', file]);
  expect(result.exitCode, 0, reason: '${result.stderr}');
  final codec = await ui.instantiateImageCodec(await File(file).readAsBytes());
  final frame = await codec.getNextFrame();
  try {
    final bytes = (await frame.image.toByteData(
      format: ui.ImageByteFormat.rawRgba,
    ))!;
    return [
      for (var x = 8; x < 40; x++)
        bytes.getUint8((y * frame.image.width + x) * 4),
    ];
  } finally {
    frame.image.dispose();
    codec.dispose();
  }
}

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  final deviceFilter = NativeViewDeviceFilter(binding);
  tearDownAll(deviceFilter.restore);
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.onlyPumps;
  testWidgets(
    'Launchpad glass retains transitions and restores sharp desktop pixels',
    (tester) async {
      expect(Platform.environment['REALAND_SHELL_ISOLATED_TEST'], '1');
      expect(
        Platform.environment['REALAND_SOCKET'],
        isNot(Platform.environment['REALAND_SHELL_PARENT_SOCKET']),
      );
      final root = Platform.environment['REALAND_SHELL_TEST_ROOT']!;
      final captures = Directory(
        Platform.environment['REALAND_SHELL_CAPTURE_DIR']!,
      );
      await captures.create(recursive: true);
      // Two-pixel stripes detect filtering that a solid-color backdrop cannot.
      final recorder = ui.PictureRecorder();
      final canvas = Canvas(recorder);
      for (var x = 0; x < 64; x += 2) {
        canvas.drawRect(
          Rect.fromLTWH(x.toDouble(), 0, 2, 64),
          Paint()
            ..color = x % 4 == 0
                ? const Color(0xffeeeeee)
                : const Color(0xff444444),
        );
      }
      final picture = recorder.endRecording();
      final image = await picture.toImage(64, 64);
      final wallpaper = File('$root/sharp-stripes.png');
      await wallpaper.writeAsBytes(
        (await image.toByteData(
          format: ui.ImageByteFormat.png,
        ))!.buffer.asUint8List(),
      );
      image.dispose();
      picture.dispose();
      final background = await Process.start('/usr/bin/swaybg', [
        '-i',
        wallpaper.path,
        '-m',
        'tile',
      ]);
      final controller =
          ShellController(
              demo: true,
              tray: false,
              notifications: false,
              powerProfilesEnabled: false,
            )
            ..setShellStyle(ShellStyle.macos)
            ..setReduceMotion(false);
      final evidence = <String, dynamic>{
        'passed': false,
        'cycles': <dynamic>[],
      };
      try {
        await tester.pumpWidget(
          ShellHost(
            controller: controller,
            preferences: const ShellPreferences(shellStyle: ShellStyle.macos),
            persist: false,
          ),
          wrapWithView: false,
        );
        final closed = await _waitState(
          tester,
          (s) => s['expanded'] == false && s['barBlurRequested'] == true,
        );
        final y = (((closed['monitorGeometry'] as Map)['height'] as num) * .55)
            .round();
        List<int> baseline = [];
        final deadline = DateTime.now().add(const Duration(seconds: 10));
        do {
          await tester.pump(const Duration(milliseconds: 16));
          baseline = await _sample('closed-baseline', y);
          if (baseline.toSet().containsAll([68, 238])) break;
        } while (DateTime.now().isBefore(deadline));
        expect(baseline.toSet(), containsAll([68, 238]));
        evidence['launcherCycles'] = await exerciseLauncherGlass(
          tester,
          controller,
          baseline: baseline,
          y: y,
          sample: _sample,
        );
        if (Platform.environment.containsKey(
          'REALAND_SHELL_PRESENTED_SOCKET',
        )) {
          binding.framePolicy =
              LiveTestWidgetsFlutterBindingFramePolicy.fullyLive;
          final idleCycles = <Map<String, dynamic>>[];
          for (var cycle = 0; cycle < 4; cycle++) {
            controller.selectPanel(ShellPanel.launcher);
            await Future<void>.delayed(const Duration(milliseconds: 700));
            expectLauncherGlass(await _state());
            final open = await samplePresentedStripe(
              'presented-open-$cycle',
              y,
            );
            expect((open..sort()).last - open.first, lessThan(30));
            controller.closePanel();
            // No tester.pump, input, or inner screencopy after this close.
            await Future<void>.delayed(const Duration(milliseconds: 500));
            final closedIdle = await _state();
            final actual = await samplePresentedStripe(
              'presented-idle-$cycle',
              y,
            );
            idleCycles.add({'native': closedIdle, 'presented': actual});
            evidence['idleCycles'] = idleCycles;
            await File(
              '${captures.path}/idle-progress.json',
            ).writeAsString(jsonEncode(evidence));
            expect(closedIdle['expanded'], isFalse);
            expect(closedIdle['launcherBackdropRequested'], isFalse);
            expect(
              actual,
              baseline,
              reason:
                  'The presented desktop must clear without another operation',
            );
          }
          binding.framePolicy =
              LiveTestWidgetsFlutterBindingFramePolicy.onlyPumps;
        }
        controller.setShellStyle(ShellStyle.material);
        await _waitState(tester, (s) => s['barBlurRequested'] == false);
        controller.selectPanel(ShellPanel.launcher);
        await _waitState(tester, (s) => s['expanded'] == true);
        expect((await _state())['mainBlurRegions'], isEmpty);
        controller.closePanel();
        await tester.pump(const Duration(milliseconds: 220));
        await _waitState(tester, (s) => s['expanded'] == false);
        evidence['ignoredUnattachedDeviceEvents'] = deviceFilter.ignored;
        evidence['passed'] = true;
        evidence['baselineStripeRgb'] = baseline;
        await File(
          '${captures.path}/launcher-backdrop-native-evidence.json',
        ).writeAsString(const JsonEncoder.withIndent('  ').convert(evidence));
      } finally {
        await tester.pumpWidget(
          const ViewCollection(views: []),
          wrapWithView: false,
        );
        await _window.invokeMethod<void>('setExpanded', false);
        await _window.invokeMethod<void>('setShellStyle', 'material');
        controller.dispose();
        background.kill();
        await background.exitCode;
      }
    },
  );
}
