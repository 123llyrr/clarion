// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:realand_shell/models.dart';
import 'package:realand_shell/state/shell_controller.dart';
import 'package:realand_shell/ui/launcher_surface.dart';
import 'package:realand_shell/ui/mac_launcher_panel.dart';

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
  fail('Native launcher glass did not settle: ${await _state()}');
}

void expectLauncherGlass(Map<String, dynamic> state) {
  expect(state['blurAvailable'], isTrue);
  expect(state['launcherBackdropRequested'], isTrue);
  expect(state['barBlurRequested'], isFalse);
  expect(state['mainBlurRegions'], [
    {'x': 0, 'y': 0, 'width': state['width'], 'height': state['height']},
  ]);
}

Future<int> _wallpaperError(
  String name,
  int menuHeight,
  int surfaceHeight,
) async {
  final path = '${Platform.environment['REALAND_SHELL_CAPTURE_DIR']}/$name.png';
  final codec = await ui.instantiateImageCodec(await File(path).readAsBytes());
  final frame = await codec.getNextFrame();
  try {
    final bytes = (await frame.image.toByteData(
      format: ui.ImageByteFormat.rawRgba,
    ))!;
    var worst = 0;
    // Probe across the output, excluding the menu and bottom Dock. Odd steps
    // visit both stripe colors, so a retained tint/blur cannot look correct.
    final scale = frame.image.height / surfaceHeight;
    for (
      var y = (menuHeight * scale).ceil() + 5;
      y < frame.image.height - (150 * scale).ceil();
      y += 17
    ) {
      for (var x = 8; x < frame.image.width - 8; x += 17) {
        final expected = x % 4 < 2 ? 238 : 68;
        for (var channel = 0; channel < 3; channel++) {
          final error =
              (bytes.getUint8((y * frame.image.width + x) * 4 + channel) -
                      expected)
                  .abs();
          if (error > worst) worst = error;
        }
      }
    }
    return worst;
  } finally {
    frame.image.dispose();
    codec.dispose();
  }
}

/// Verify compositor pixels, retained/reversed exit, and restoration of a
/// two-pixel striped wallpaper. Native request state alone cannot prove paint.
Future<List<Map<String, Object?>>> exerciseLauncherGlass(
  WidgetTester tester,
  ShellController controller, {
  required List<int> baseline,
  required int y,
  required Future<List<int>> Function(String, int) sample,
}) async {
  final evidence = <Map<String, Object?>>[];
  for (var cycle = 0; cycle < 3; cycle++) {
    controller.selectPanel(ShellPanel.launcher);
    final opened = await _waitState(
      tester,
      (s) => s['expanded'] == true && s['launcherBackdropRequested'] == true,
    );
    expectLauncherGlass(opened);
    expect(
      find.descendant(
        of: find.byType(LauncherSurface),
        matching: find.byType(BackdropFilter),
      ),
      findsNothing,
    );
    var spread = 255;
    var mean = 255.0;
    var alpha = 0.0;
    List<int> glass = [];
    final paintDeadline = DateTime.now().add(const Duration(seconds: 5));
    do {
      await tester.pump(const Duration(milliseconds: 16));
      glass = await sample('launcher-open-$cycle', y);
      final ordered = [...glass]..sort();
      spread = ordered.last - ordered.first;
      mean = glass.reduce((a, b) => a + b) / glass.length;
      alpha = tester
          .widget<Opacity>(
            find
                .ancestor(
                  of: find.byType(MacLauncherPanel),
                  matching: find.byType(Opacity),
                )
                .first,
          )
          .opacity;
    } while ((spread >= 30 || mean >= 145 || alpha != 1) &&
        DateTime.now().isBefore(paintDeadline));
    expect(alpha, 1, reason: 'Test a fully painted launcher before closing it');
    expect(
      spread,
      lessThan(30),
      reason: 'Native glass must filter sharp stripes',
    );
    expect(
      mean,
      inInclusiveRange(90, 145),
      reason: 'The light tint must be painted',
    );

    controller.closePanel();
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 16));
    final exiting = await _state();
    expectLauncherGlass(exiting);
    expect(exiting['mainBlurRegions'], opened['mainBlurRegions']);
    // Reversing must preserve both the surface and its single native mask.
    controller.selectPanel(ShellPanel.launcher);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 220));
    expectLauncherGlass(await _state());
    // Also cover disabling animations while a retained glass surface exists.
    if (cycle == 2) controller.setReduceMotion(true);
    controller.closePanel();
    final closed = await _waitState(
      tester,
      (s) => s['expanded'] == false && s['launcherSceneVisible'] == false,
    );
    expect(closed['launcherBackdropRequested'], isFalse);
    expect(closed['barBlurRequested'], isTrue);
    var worst = 255;
    var gridError = 255;
    List<int> restored = [];
    final clearDeadline = DateTime.now().add(const Duration(seconds: 3));
    do {
      await tester.pump(const Duration(milliseconds: 16));
      restored = await sample('launcher-closed-$cycle', y);
      worst = 0;
      for (var i = 0; i < restored.length; i++) {
        final error = (restored[i] - baseline[i]).abs();
        if (error > worst) worst = error;
      }
      gridError = await _wallpaperError(
        'launcher-closed-$cycle',
        (closed['menuBarHeight'] as num).round(),
        (closed['height'] as num).round(),
      );
    } while ((worst > 2 || gridError > 2) &&
        DateTime.now().isBefore(clearDeadline));
    expect(
      worst,
      lessThanOrEqualTo(2),
      reason: 'Closing must restore sharp wallpaper',
    );
    expect(
      gridError,
      lessThanOrEqualTo(2),
      reason: 'No old glass across the desktop',
    );
    evidence.add({
      'native': opened,
      'stripeRgb': glass,
      'glassStripeSpread': spread,
      'glassMean': mean,
      'retained': exiting,
      'closed': closed,
      'restoredStripeRgb': restored,
      'worstRestoreError': worst,
      'worstDesktopGridError': gridError,
      'reducedMotionClose': cycle == 2,
    });
    controller.setReduceMotion(false);
  }
  return evidence;
}

// flutter_test's live hardware transformer assumes a mounted RenderView.
// Filter only packets for unattached views during creation/teardown, as the
// production hitTestInView does. Injected test events bypass this callback.
// Report these packets; this is not proof of physical device input delivery.
class NativeViewDeviceFilter {
  NativeViewDeviceFilter(this.binding) {
    original = binding.platformDispatcher.onPointerDataPacket;
    binding.platformDispatcher.onPointerDataPacket = handle;
  }
  final IntegrationTestWidgetsFlutterBinding binding;
  ui.PointerDataPacketCallback? original;
  final ignored = <Map<String, Object?>>[];
  void handle(ui.PointerDataPacket packet) {
    final attached = binding.renderViews
        .map((v) => v.flutterView.viewId)
        .toSet();
    final valid = <ui.PointerData>[];
    for (final data in packet.data) {
      if (attached.contains(data.viewId)) {
        valid.add(data);
      } else {
        ignored.add({
          'viewId': data.viewId,
          'change': data.change.name,
          'x': data.physicalX,
          'y': data.physicalY,
        });
      }
    }
    if (valid.isNotEmpty) original?.call(ui.PointerDataPacket(data: valid));
  }

  void restore() => binding.platformDispatcher.onPointerDataPacket = original;
}
