// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:convert';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/foundation.dart' show listEquals;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:clarion_shell/main.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/mac_shell_preferences.dart';
import 'package:clarion_shell/preferences.dart';
import 'package:clarion_shell/state/shell_controller.dart';

import 'helpers/launcher_glass_checks.dart';
import 'helpers/presented_capture.dart';

const _window = MethodChannel('clarion.shell/window');
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
  fail('Native Shell state did not settle: ${await _state()}');
}

Future<List<int>> _sample(String name, int y) async {
  final file = '${Platform.environment['CLARION_SHELL_CAPTURE_DIR']}/$name.png';
  final result = await Process.run('grim', ['-o', 'winit', file]);
  expect(result.exitCode, 0, reason: '${result.stderr}');
  final codec = await ui.instantiateImageCodec(await File(file).readAsBytes());
  final frame = await codec.getNextFrame();
  try {
    final native = await _state();
    final scale = (native['scaleFactor'] as num).toInt();
    final monitor = native['monitorGeometry'] as Map;
    expect(frame.image.width, (monitor['width'] as num).toInt() * scale);
    expect(frame.image.height, (monitor['height'] as num).toInt() * scale);
    final physicalY = y * scale;
    expect(physicalY, inInclusiveRange(0, frame.image.height - 1));
    final bytes = (await frame.image.toByteData(
      format: ui.ImageByteFormat.rawRgba,
    ))!;
    return [
      for (var x = 8; x < 40; x++)
        bytes.getUint8((physicalY * frame.image.width + x) * 4),
    ];
  } finally {
    frame.image.dispose();
    codec.dispose();
  }
}

// Compare the region occupied by the actual panel against an outer presented
// baseline. Native coordinates are logical; screenshot dimensions supply scale.
Future<Map<String, int>> _presentedDifference(
  File actualFile,
  File baselineFile,
  Map panel,
  Map monitor,
  int contentScale,
) async {
  final actualCodec = await ui.instantiateImageCodec(
    await actualFile.readAsBytes(),
  );
  final baselineCodec = await ui.instantiateImageCodec(
    await baselineFile.readAsBytes(),
  );
  final actual = (await actualCodec.getNextFrame()).image;
  final original = (await baselineCodec.getNextFrame()).image;
  try {
    expect(actual.width, original.width);
    expect(actual.height, original.height);
    final pixels = (await actual.toByteData(
      format: ui.ImageByteFormat.rawRgba,
    ))!;
    final expected = (await original.toByteData(
      format: ui.ImageByteFormat.rawRgba,
    ))!;
    expect(contentScale, anyOf(1, 2));
    final contentWidth = (monitor['width'] as num).round() * contentScale;
    final contentHeight = (monitor['height'] as num).round() * contentScale;
    // The inner winit client carries libdecor's symmetric 4-logical-pixel
    // sides/bottom and a titlebar. Infer only geometry, never pixel orientation.
    final side = (actual.width - contentWidth) ~/ 2;
    expect(side, 4 * contentScale);
    expect(actual.width, contentWidth + 2 * side);
    final contentTop = actual.height - contentHeight - side;
    expect(contentTop, greaterThanOrEqualTo(side));
    expect(contentTop + contentHeight + side, actual.height);
    final left = (side + ((panel['x'] as num) - 3) * contentScale)
        .floor()
        .clamp(side, side + contentWidth - 1);
    final right =
        (side +
                ((panel['x'] as num) + (panel['width'] as num) + 3) *
                    contentScale)
            .ceil()
            .clamp(side, side + contentWidth);
    final top = (contentTop + ((panel['y'] as num) - 3) * contentScale)
        .floor()
        .clamp(contentTop, contentTop + contentHeight - 1);
    final bottom =
        (contentTop +
                ((panel['y'] as num) + (panel['height'] as num) + 3) *
                    contentScale)
            .ceil()
            .clamp(contentTop, contentTop + contentHeight);
    var wrong = 0;
    var checked = 0;
    for (var y = top; y < bottom; y += 3) {
      for (var x = left; x < right; x += 3) {
        final offset = (y * actual.width + x) * 4;
        checked++;
        if ([0, 1, 2].any(
          (channel) =>
              (pixels.getUint8(offset + channel) -
                      expected.getUint8(offset + channel))
                  .abs() >
              2,
        )) {
          wrong++;
        }
      }
    }
    expect(checked, greaterThan(100));
    return {
      'wrongPixels': wrong,
      'checkedPixels': checked,
      'width': actual.width,
      'height': actual.height,
    };
  } finally {
    actual.dispose();
    original.dispose();
    actualCodec.dispose();
    baselineCodec.dispose();
  }
}

Future<Map<String, int>> _presentedOrientation(
  File actualFile,
  File innerFile,
  int scale,
) async {
  final actualCodec = await ui.instantiateImageCodec(
    await actualFile.readAsBytes(),
  );
  final innerCodec = await ui.instantiateImageCodec(
    await innerFile.readAsBytes(),
  );
  final actual = (await actualCodec.getNextFrame()).image;
  final inner = (await innerCodec.getNextFrame()).image;
  try {
    final side = 4 * scale;
    expect(actual.width, inner.width + 2 * side);
    final top = actual.height - inner.height - side;
    expect(top, greaterThanOrEqualTo(side));
    final pixels = (await actual.toByteData(
      format: ui.ImageByteFormat.rawRgba,
    ))!;
    final expected = (await inner.toByteData(
      format: ui.ImageByteFormat.rawRgba,
    ))!;
    var barWrong = 0, dockWrong = 0, barChecked = 0, dockChecked = 0;
    for (var y = 0; y < inner.height; y += 2) {
      final bar = y < 30 * scale;
      final dock =
          y >= inner.height - 145 * scale && y < inner.height - 5 * scale;
      if (!bar && !dock) continue;
      for (var x = 10 * scale; x < inner.width - 10 * scale; x += 2) {
        // Exclude the right-side clock and live status indicators.
        if (bar && x > 380 * scale) continue;
        final a = ((y + top) * actual.width + x + side) * 4;
        final b = (y * inner.width + x) * 4;
        final wrong = [0, 1, 2].any(
          (c) => (pixels.getUint8(a + c) - expected.getUint8(b + c)).abs() > 2,
        );
        if (bar) {
          barChecked++;
          if (wrong) barWrong++;
        }
        if (dock) {
          dockChecked++;
          if (wrong) dockWrong++;
        }
      }
    }
    expect(barChecked, greaterThan(100));
    expect(dockChecked, greaterThan(100));
    expect(
      barWrong,
      0,
      reason: 'Actual presented top menu bar must match upright inner content',
    );
    expect(
      dockWrong,
      0,
      reason: 'Actual presented bottom Dock must match upright inner content',
    );
    return {
      'barChecked': barChecked,
      'barWrong': barWrong,
      'dockChecked': dockChecked,
      'dockWrong': dockWrong,
    };
  } finally {
    actual.dispose();
    inner.dispose();
    actualCodec.dispose();
    innerCodec.dispose();
  }
}

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  final deviceFilter = NativeViewDeviceFilter(binding);
  tearDownAll(deviceFilter.restore);
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.onlyPumps;
  testWidgets('Control center closes without glass and reverses cleanly', (
    tester,
  ) async {
    expect(Platform.environment['CLARION_SHELL_ISOLATED_TEST'], '1');
    expect(
      Platform.environment['CLARION_SOCKET'],
      isNot(Platform.environment['CLARION_SHELL_PARENT_SOCKET']),
    );
    final root = Platform.environment['CLARION_SHELL_TEST_ROOT']!;
    final captures = Directory(
      Platform.environment['CLARION_SHELL_CAPTURE_DIR']!,
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
          ..setBarPosition(BarPosition.bottom)
          ..setMacPreferences(
            const MacShellPreferences(menuBarHeight: 32, dockSize: 64),
          )
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
      for (var cycle = 0; cycle < 5; cycle++) {
        controller.selectPanel(ShellPanel.quickSettings);
        var panelState = await _waitState(
          tester,
          (s) => s['expanded'] == true && s['panelBlurRequested'] == true,
        );
        await tester.pump(const Duration(milliseconds: 240));
        if (cycle == 1) {
          await tester.tap(
            find.byKey(const ValueKey('mac-control-desktop-preferences')),
          );
          final monitorWidth =
              ((panelState['monitorGeometry'] as Map)['width'] as num)
                  .toDouble();
          // The preferences page fits within the native output with 12-point
          // side margins; a narrow 2x output cannot carry a 620-point page.
          final preferencesWidth = (monitorWidth - 24).clamp(0.0, 620.0);
          panelState = await _waitState(
            tester,
            (s) =>
                (s['panelGlassRegion'] as Map?)?['width'] == preferencesWidth,
          );
          await tester.pump(const Duration(milliseconds: 180));
        }
        if (cycle == 2 || cycle == 4) {
          controller.closePanel();
          await tester.pump();
          await tester.pump(const Duration(milliseconds: 32));
          expect((await _state())['expanded'], isTrue);
          controller.selectPanel(ShellPanel.quickSettings);
          await tester.pump();
          await tester.pump(const Duration(milliseconds: 180));
        }
        final panel = panelState['panelGlassRegion'] as Map;
        final captureScale = (panelState['scaleFactor'] as num).toInt();
        // A native blur request may precede the first painted Flutter frame.
        // Wait for the actual content, so closing an empty mask cannot pass.
        var contentPixels = 0;
        final paintDeadline = DateTime.now().add(const Duration(seconds: 5));
        do {
          await tester.pump(const Duration(milliseconds: 16));
          await _sample('panel-open-$cycle', y);
          final codec = await ui.instantiateImageCodec(
            await File('${captures.path}/panel-open-$cycle.png').readAsBytes(),
          );
          final frame = await codec.getNextFrame();
          try {
            final bytes = (await frame.image.toByteData(
              format: ui.ImageByteFormat.rawRgba,
            ))!;
            expect(
              frame.image.width,
              ((panelState['monitorGeometry'] as Map)['width'] as num).toInt() *
                  captureScale,
            );
            expect(
              frame.image.height,
              ((panelState['monitorGeometry'] as Map)['height'] as num)
                      .toInt() *
                  captureScale,
            );
            contentPixels = 0;
            final left =
                (((panel['x'] as num) + (panel['width'] as num) * .25) *
                        captureScale)
                    .round();
            final right =
                (((panel['x'] as num) + (panel['width'] as num) * .75) *
                        captureScale)
                    .round();
            final bottom =
                (((panel['y'] as num) + (panel['height'] as num)) *
                        captureScale)
                    .round() -
                16 * captureScale;
            expect(left, greaterThanOrEqualTo(0));
            expect(right, lessThanOrEqualTo(frame.image.width));
            expect(bottom, lessThanOrEqualTo(frame.image.height));
            for (
              var yy =
                  (panel['y'] as num).round() * captureScale +
                  16 * captureScale;
              yy < bottom;
              yy++
            ) {
              for (var xx = left; xx < right; xx++) {
                final offset = (yy * frame.image.width + xx) * 4;
                // Blue controls cannot come from the grayscale wallpaper or
                // its native blur, including the wider preferences page.
                if (bytes.getUint8(offset + 2) > bytes.getUint8(offset) + 30 &&
                    bytes.getUint8(offset + 2) >
                        bytes.getUint8(offset + 1) + 10) {
                  contentPixels++;
                }
              }
            }
          } finally {
            frame.image.dispose();
            codec.dispose();
          }
        } while (contentPixels < 100 && DateTime.now().isBefore(paintDeadline));
        expect(
          contentPixels,
          greaterThanOrEqualTo(100),
          reason:
              'The panel content must be painted before testing its dismissal',
        );
        controller.closePanel();
        await tester.pump(const Duration(milliseconds: 240));
        final closedState = await _waitState(
          tester,
          (s) => s['expanded'] == false && s['panelBlurRequested'] == false,
        );
        expect(closedState['panelGlassRegion'], isNull);
        final output = await Process.run(
          Platform.environment['CLARION_SHELL_CLARION_BIN']!,
          ['msg', '--json', 'layers'],
        );
        await File(
          '${captures.path}/closed-layers-$cycle.json',
        ).writeAsString(output.stdout as String);
        final top = (((panel['y'] as num) - 3) * captureScale).floor();
        final bottom =
            (((panel['y'] as num) + (panel['height'] as num) + 3) *
                    captureScale)
                .ceil();
        final left = (((panel['x'] as num) - 3) * captureScale).floor();
        final right =
            (((panel['x'] as num) + (panel['width'] as num) + 3) * captureScale)
                .ceil();
        var wrong = 0;
        final deadline = DateTime.now().add(const Duration(seconds: 3));
        do {
          await tester.pump(const Duration(milliseconds: 16));
          await _sample('panel-closed-$cycle', y);
          final codec = await ui.instantiateImageCodec(
            await File(
              '${captures.path}/panel-closed-$cycle.png',
            ).readAsBytes(),
          );
          final frame = await codec.getNextFrame();
          try {
            final bytes = (await frame.image.toByteData(
              format: ui.ImageByteFormat.rawRgba,
            ))!;
            expect(top, greaterThanOrEqualTo(0));
            expect(left, greaterThanOrEqualTo(0));
            expect(bottom, lessThanOrEqualTo(frame.image.height));
            expect(right, lessThanOrEqualTo(frame.image.width));
            wrong = 0;
            for (var yy = top; yy < bottom; yy += 3) {
              for (var xx = left; xx < right; xx += 3) {
                final expected = xx % 4 < 2 ? 238 : 68;
                if ((bytes.getUint8((yy * frame.image.width + xx) * 4) -
                            expected)
                        .abs() >
                    2) {
                  wrong++;
                }
              }
            }
          } finally {
            frame.image.dispose();
            codec.dispose();
          }
        } while (wrong > 0 && DateTime.now().isBefore(deadline));
        (evidence['cycles'] as List).add({
          'closed': closedState,
          'wrongPixels': wrong,
          'paintedContentPixels': contentPixels,
        });
        await File(
          '${captures.path}/panel-close-progress.json',
        ).writeAsString(const JsonEncoder.withIndent('  ').convert(evidence));
        expect(
          wrong,
          0,
          reason: 'A closed panel must reveal the original sharp wallpaper',
        );
      }
      evidence['launcherCycles'] = await exerciseLauncherGlass(
        tester,
        controller,
        baseline: baseline,
        y: y,
        sample: _sample,
      );
      controller.selectPanel(ShellPanel.launcher);
      await _waitState(tester, (s) => s['launcherBackdropRequested'] == true);
      await tester.pump(const Duration(milliseconds: 220));
      controller.selectPanel(ShellPanel.quickSettings);
      final switched = await _waitState(
        tester,
        (s) =>
            s['launcherSceneVisible'] == false &&
            s['panelBlurRequested'] == true,
      );
      expect(switched['launcherBackdropRequested'], isFalse);
      expect(
        switched['mainBlurRegions'],
        isNot(
          contains({
            'x': 0,
            'y': 0,
            'width': switched['width'],
            'height': switched['height'],
          }),
        ),
      );
      // The probe lies outside the control center. The old full-screen mask
      // must disappear even though the main surface remains expanded.
      var restored = <int>[];
      final switchDeadline = DateTime.now().add(const Duration(seconds: 3));
      do {
        await tester.pump(const Duration(milliseconds: 16));
        restored = await _sample('launcher-to-control-center', y);
      } while (!listEquals(restored, baseline) &&
          DateTime.now().isBefore(switchDeadline));
      expect(restored, baseline);
      evidence['launcherToControlCenter'] = switched;
      controller.closePanel();
      await _waitState(tester, (s) => s['expanded'] == false);
      // The final close must present its cleanup autonomously. Inner grim can
      // force a new render and hide stale output damage, so use the private host.
      expect(
        Platform.environment.containsKey('CLARION_SHELL_PRESENTED_SOCKET'),
        isTrue,
      );
      binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.fullyLive;
      await Future<void>.delayed(const Duration(milliseconds: 700));
      await _sample('panel-presented-inner-reference', y);
      await Future<void>.delayed(const Duration(milliseconds: 700));
      final presentedBaseline = await capturePresentedWindow(
        'panel-presented-baseline',
      );
      final referenceState = await _state();
      evidence['actualPresentedOrientation'] = await _presentedOrientation(
        presentedBaseline,
        File('${captures.path}/panel-presented-inner-reference.png'),
        (referenceState['scaleFactor'] as num).toInt(),
      );
      final presentedCycles = <Map<String, dynamic>>[];
      for (var cycle = 0; cycle < 4; cycle++) {
        await tester.tap(find.byKey(const ValueKey('mac-quick-settings')));
        await Future<void>.delayed(const Duration(milliseconds: 700));
        if (cycle.isOdd) {
          // Exercise the real bar toggle during both directions of the exit.
          for (var toggle = 0; toggle < 4; toggle++) {
            await tester.tap(find.byKey(const ValueKey('mac-quick-settings')));
            await Future<void>.delayed(const Duration(milliseconds: 40));
          }
          await Future<void>.delayed(const Duration(milliseconds: 700));
        }
        final openState = await _state();
        expect(openState['expanded'], isTrue);
        expect(openState['panelBlurRequested'], isTrue);
        final panel = openState['panelGlassRegion'] as Map;
        final monitor = openState['monitorGeometry'] as Map;
        final open = await capturePresentedWindow(
          'panel-presented-open-$cycle',
        );
        await File(
          '${captures.path}/panel-presented-open-native-$cycle.json',
        ).writeAsString(const JsonEncoder.withIndent('  ').convert(openState));
        await capturePrivateOuterOutput(
          'panel-private-outer-output-open-$cycle',
        );
        final openPixels = await _presentedDifference(
          open,
          presentedBaseline,
          panel,
          monitor,
          (openState['scaleFactor'] as num).toInt(),
        );
        expect(
          openPixels['wrongPixels'],
          greaterThan(100),
          reason:
              'Control center content must actually be presented before dismissal',
        );
        await tester.tap(find.byKey(const ValueKey('mac-quick-settings')));
        // No tester.pump, further input or inner screencopy after this close.
        await Future<void>.delayed(const Duration(milliseconds: 700));
        final idle = await _state();
        final actual = await capturePresentedWindow(
          'panel-presented-idle-$cycle',
        );
        final idlePixels = await _presentedDifference(
          actual,
          presentedBaseline,
          panel,
          monitor,
          (openState['scaleFactor'] as num).toInt(),
        );
        presentedCycles.add({
          'native': idle,
          'rapidBarToggle': cycle.isOdd,
          'open': openPixels,
          'closedIdle': idlePixels,
          'idleMilliseconds': 700,
          'capture': actual.path,
        });
        evidence['presentedIdleCycles'] = presentedCycles;
        await File(
          '${captures.path}/panel-presented-progress.json',
        ).writeAsString(const JsonEncoder.withIndent('  ').convert(evidence));
        expect(idle['expanded'], isFalse);
        expect(idle['panelBlurRequested'], isFalse);
        expect(idle['panelGlassRegion'], isNull);
        expect(
          idlePixels['wrongPixels'],
          0,
          reason:
              'Presented control center pixels must clear after close without another operation',
        );
      }
      binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.onlyPumps;
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
        '${captures.path}/panel-transition-native-evidence.json',
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
