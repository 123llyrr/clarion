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
  testWidgets('Native glass changes panel edges and covers the launcher', (
    tester,
  ) async {
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
    final evidence = <String, dynamic>{'passed': false, 'cycles': <dynamic>[]};
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
      controller.selectPanel(ShellPanel.quickSettings);
      final panelState = await _waitState(
        tester,
        (s) => s['expanded'] == true && s['panelBlurRequested'] == true,
      );
      await tester.pump(const Duration(milliseconds: 300));
      final panel = Map<String, dynamic>.from(
        panelState['panelGlassRegion'] as Map,
      );
      final edgeY = (panel['y'] as num).round();
      final startX = ((panel['x'] as num) + (panel['width'] as num) * .25)
          .round();
      final endX = ((panel['x'] as num) + (panel['width'] as num) * .75)
          .round();
      var panelContentVisible = false;
      Future<List<int>> edge(String name) async {
        await _sample(name, y);
        final codec = await ui.instantiateImageCodec(
          await File('${captures.path}/$name.png').readAsBytes(),
        );
        final frame = await codec.getNextFrame();
        try {
          final bytes = (await frame.image.toByteData(
            format: ui.ImageByteFormat.rawRgba,
          ))!;
          // Region requests can precede the Flutter buffer. Require painted
          // slider tracks before using a frame as the visual reference.
          var brightPixels = 0;
          for (
            var yy = (panel['y'] as num).round() + 16;
            yy < ((panel['y'] as num) + (panel['height'] as num)).round() - 16;
            yy++
          ) {
            for (var xx = startX; xx < endX; xx++) {
              final offset = (yy * frame.image.width + xx) * 4;
              if (bytes.getUint8(offset) > 220 &&
                  bytes.getUint8(offset + 1) > 220 &&
                  bytes.getUint8(offset + 2) > 220) {
                brightPixels++;
              }
            }
          }
          panelContentVisible = brightPixels > 1000;
          return [
            for (var yy = edgeY; yy < edgeY + 4; yy++)
              for (var x = startX; x < endX; x++)
                bytes.getUint8((yy * frame.image.width + x) * 4),
          ];
        } finally {
          frame.image.dispose();
          codec.dispose();
        }
      }

      final config = File('$root/config.kdl');
      final original = await config.readAsString();
      expect(original, contains('liquid-glass true'));
      List<int> glassEdge = [];
      final panelDeadline = DateTime.now().add(const Duration(seconds: 8));
      do {
        await tester.pump(const Duration(milliseconds: 16));
        glassEdge = await edge('glass-control-center');
      } while (!panelContentVisible && DateTime.now().isBefore(panelDeadline));
      expect(
        panelContentVisible,
        isTrue,
        reason: 'Wait for real panel contents',
      );
      await config.writeAsString(
        original.replaceAll('liquid-glass true', 'liquid-glass false'),
      );
      final reload = await Process.run(
        Platform.environment['REALAND_SHELL_REALAND_BIN']!,
        ['msg', 'action', 'load-config-file'],
      );
      expect(reload.exitCode, 0, reason: '${reload.stderr}');
      List<int> ordinaryEdge = [];
      var changed = 0;
      final reloadDeadline = DateTime.now().add(const Duration(seconds: 8));
      do {
        await tester.pump(const Duration(milliseconds: 32));
        ordinaryEdge = await edge('ordinary-control-center');
        changed = 0;
        for (var i = 0; i < glassEdge.length; i++) {
          if ((glassEdge[i] - ordinaryEdge[i]).abs() > 2) changed++;
        }
      } while (changed < 20 && DateTime.now().isBefore(reloadDeadline));
      expect(
        changed,
        greaterThanOrEqualTo(20),
        reason: 'Glass must change real panel rim pixels',
      );
      expect(
        await _sample('ordinary-outside', y),
        baseline,
        reason: 'Desktop outside the glass mask stays unchanged',
      );
      // Restoring through the built-in default also covers existing user configs.
      await config.writeAsString(
        original
            .split('\n')
            .where((line) => !line.trimLeft().startsWith('layer-rule '))
            .join('\n'),
      );
      expect(
        (await Process.run(Platform.environment['REALAND_SHELL_REALAND_BIN']!, [
          'msg',
          'action',
          'load-config-file',
        ])).exitCode,
        0,
      );
      var restored = false;
      final restoreDeadline = DateTime.now().add(const Duration(seconds: 8));
      do {
        await tester.pump(const Duration(milliseconds: 32));
        final pixels = await edge('restored-glass-control-center');
        restored = true;
        for (var i = 0; i < pixels.length; i++) {
          if ((pixels[i] - glassEdge[i]).abs() > 2) restored = false;
        }
      } while (!restored && DateTime.now().isBefore(restoreDeadline));
      expect(
        restored,
        isTrue,
        reason: 'Toggling glass restores identical material pixels',
      );
      expect(await _sample('glass-outside', y), baseline);
      evidence['panel'] = {
        'native': panelState,
        'rimPixelsChanged': changed,
        'samplePixels': glassEdge.length,
        'toggleRestoredPixels': restored,
        'outsideDesktopUnchanged': true,
        'builtinDefaultWorksWithoutConfigRule': true,
      };
      controller.closePanel();
      await tester.pump(const Duration(milliseconds: 220));
      await _waitState(tester, (s) => s['expanded'] == false);
      evidence['launcherCycles'] = await exerciseLauncherGlass(
        tester,
        controller,
        baseline: baseline,
        y: y,
        sample: _sample,
      );
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
        '${captures.path}/liquid-glass-native-evidence.json',
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
  });
}
