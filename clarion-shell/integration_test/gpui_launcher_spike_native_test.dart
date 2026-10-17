// SPDX-License-Identifier: GPL-3.0-or-later
// Phase 0 native focus/backdrop seam, without a bridge or preference change.
import 'dart:convert';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:clarion_shell/main.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/preferences.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/ui/mac_launcher_panel.dart';

import 'helpers/launcher_glass_checks.dart';

const _window = MethodChannel('clarion.shell/window');
const _sampleText = 'phase0-focus-429';

Future<Map<String, dynamic>> _state() async => Map<String, dynamic>.from(
  (await _window.invokeMapMethod<String, dynamic>('windowState'))!,
);

Future<Map<String, dynamic>> _waitState(
  WidgetTester tester,
  bool Function(Map<String, dynamic>) ready,
) async {
  final deadline = DateTime.now().add(const Duration(seconds: 10));
  do {
    await tester.pump(const Duration(milliseconds: 16));
    final state = await _state();
    if (ready(state)) return state;
  } while (DateTime.now().isBefore(deadline));
  fail('Native GPUI handoff state did not settle: ${await _state()}');
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
  final deadline = DateTime.now().add(const Duration(seconds: 5));
  List<Map<String, dynamic>> layers = [];
  do {
    await tester.pump(const Duration(milliseconds: 16));
    layers = await _layers(env);
    if (ready(layers)) return layers;
  } while (DateTime.now().isBefore(deadline));
  fail('Compositor layers did not settle: $layers');
}

bool _hasLayer(
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

class _Pixels {
  const _Pixels(this.width, this.height, this.rgba);
  final int width;
  final int height;
  final Uint8List rgba;

  List<int> stripe(int logicalY, double scale) {
    final y = (logicalY * scale).round().clamp(0, height - 1);
    return [for (var x = 8; x < 40; x++) rgba[(y * width + x) * 4]];
  }

  int textDifference(_Pixels other, double scale) {
    expect(other.width, width);
    expect(other.height, height);
    // Compare only the search/header inside the centered 980x680 panel.
    final left = (width - 980 * scale) / 2;
    final top = (height - 680 * scale) / 2;
    final x0 = (left + 65 * scale).round().clamp(0, width);
    final x1 = (left + 900 * scale).round().clamp(0, width);
    final y0 = (top + 70 * scale).round().clamp(0, height);
    final y1 = (top + 240 * scale).round().clamp(0, height);
    var changed = 0;
    for (var y = y0; y < y1; y++) {
      for (var x = x0; x < x1; x++) {
        final offset = (y * width + x) * 4;
        for (var channel = 0; channel < 3; channel++) {
          if ((rgba[offset + channel] - other.rgba[offset + channel]).abs() >
              8) {
            changed++;
            break;
          }
        }
      }
    }
    return changed;
  }
}

Future<_Pixels> _capture(
  Map<String, String> env,
  String output,
  String name,
) async {
  final file = File('${env['CLARION_SHELL_CAPTURE_DIR']}/$name.png');
  final result = await Process.run(env['CLARION_SHELL_GRIM'] ?? 'grim', [
    '-o',
    output,
    file.path,
  ], environment: env).timeout(const Duration(seconds: 5));
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

Future<void> _key(Map<String, String> env, List<String> arguments) async {
  final result = await Process.run(
    env['CLARION_SHELL_WTYPE'] ?? 'wtype',
    arguments,
    environment: env,
  ).timeout(const Duration(seconds: 5));
  expect(result.exitCode, 0, reason: '${result.stdout}\n${result.stderr}');
}

Future<void> _stop(Process? process) async {
  if (process == null) return;
  process.kill();
  await process.exitCode.timeout(
    const Duration(seconds: 3),
    onTimeout: () {
      process.kill(ProcessSignal.sigkill);
      return -1;
    },
  );
}

Future<File> _wallpaper(String root) async {
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
  final file = File('$root/gpui-sharp-stripes.png');
  try {
    await file.writeAsBytes(
      (await image.toByteData(
        format: ui.ImageByteFormat.png,
      ))!.buffer.asUint8List(),
    );
  } finally {
    image.dispose();
    picture.dispose();
  }
  return file;
}

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  final devices = NativeViewDeviceFilter(binding);
  tearDownAll(devices.restore);
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.onlyPumps;
  testWidgets('isolated GPUI owns input above the Shell glass backdrop', (
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
    ]) {
      expect(env[key], isNotEmpty, reason: '$key is required');
    }
    expect(
      env['WAYLAND_DISPLAY'],
      isNot(env['CLARION_SHELL_PARENT_WAYLAND_DISPLAY']),
    );
    expect(env['CLARION_SOCKET'], isNot(env['CLARION_SHELL_PARENT_SOCKET']));
    expect(env['CLARION_LAUNCHER_BIN']!.startsWith('/'), isTrue);
    expect(await File(env['CLARION_LAUNCHER_BIN']!).exists(), isTrue);
    expect(env['CLARION_SHELL_POINTER_HELPER']!.startsWith('/'), isTrue);
    expect(await File(env['CLARION_SHELL_POINTER_HELPER']!).exists(), isTrue);
    final output = env['CLARION_LAUNCHER_TEST_OUTPUT'] ?? 'winit';
    final captures = Directory(env['CLARION_SHELL_CAPTURE_DIR']!);
    await captures.create(recursive: true);
    final wallpaper = await _wallpaper(env['CLARION_SHELL_TEST_ROOT']!);
    final background = await Process.start(
      env['CLARION_SHELL_SWAYBG'] ?? '/usr/bin/swaybg',
      ['-i', wallpaper.path, '-m', 'tile'],
      environment: env,
    );
    final backgroundOutput = background.stdout.drain<void>();
    final backgroundError = background.stderr.drain<void>();
    final controller =
        ShellController(
            demo: true,
            tray: false,
            notifications: false,
            powerProfilesEnabled: false,
          )
          ..setShellStyle(ShellStyle.macos)
          ..setReduceMotion(true);
    controller.setMacPreferences(
      controller.macPreferences.copyWith(magnification: false),
    );
    final evidence = <String, dynamic>{
      'passed': false,
      'output': output,
      'virtualKeyboardText': _sampleText,
      'scope':
          'Phase 0 native seam, no launcher bridge or user preference change',
      'textVerification':
          'Search-region pixel change and saved images; no OCR claim',
      'ime': 'Not tested: wtype text is not Chinese IME composition',
      'dockPointer':
          'Isolated compositor virtual-pointer click; no physical device claim',
    };
    Process? launcher;
    Future<String>? launcherOutput;
    Future<String>? launcherError;
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
        (state) =>
            state['expanded'] == false &&
            state['barBlurRequested'] == true &&
            (state['dock'] as Map?)?['ready'] == true,
      );
      expect(closed['keyboard'], 'None');
      expect(closed['keyboardInteractivityOverride'], isNull);
      final logicalHeight =
          ((closed['monitorGeometry'] as Map)['height'] as num).toDouble();
      final logicalY = (logicalHeight * .55).round();
      late _Pixels baselinePixels;
      List<int> baseline = [];
      final wallpaperDeadline = DateTime.now().add(const Duration(seconds: 10));
      do {
        await tester.pump(const Duration(milliseconds: 16));
        baselinePixels = await _capture(env, output, 'gpui-closed-baseline');
        baseline = baselinePixels.stripe(
          logicalY,
          baselinePixels.height / logicalHeight,
        );
        if (baseline.toSet().containsAll([68, 238])) break;
      } while (DateTime.now().isBefore(wallpaperDeadline));
      expect(baseline.toSet(), containsAll([68, 238]));
      final scale = baselinePixels.height / logicalHeight;
      evidence['baselineStripe'] = baseline;
      evidence['scale'] = scale;
      // Existing modal behavior remains the default.
      await _window.invokeMethod<void>('setExpanded', true);
      expect((await _state())['keyboard'], 'Exclusive');
      await _window.invokeMethod<void>('setExpanded', false);
      await expectLater(
        _window.invokeMethod<void>('setKeyboardInteractivity', 'unsupported'),
        throwsA(isA<PlatformException>()),
      );
      // Pre-arm None before expansion; do not mount the old fullscreen launcher.
      await _window.invokeMethod<void>('setKeyboardInteractivity', 'none');
      await _window.invokeMethod<void>('setLauncherBackdrop', {
        'visible': true,
        'dockInteractive': true,
      });
      await _window.invokeMethod<void>('setExpanded', true);
      final backdrop = await _waitState(
        tester,
        (state) =>
            state['expanded'] == true &&
            state['keyboard'] == 'None' &&
            state['launcherBackdropRequested'] == true &&
            (state['dock'] as Map?)?['layer'] == 'Overlay',
      );
      expectLauncherGlass(backdrop);
      expect(backdrop['keyboardInteractivityOverride'], 'none');
      expect((backdrop['dock'] as Map)['keyboard'], 'None');
      expect((backdrop['dock'] as Map)['inputRegions'], isNotEmpty);
      expect(find.byType(MacLauncherPanel), findsNothing);
      evidence['backdropNative'] = backdrop;
      var spread = 255;
      final blurDeadline = DateTime.now().add(const Duration(seconds: 5));
      do {
        await tester.pump(const Duration(milliseconds: 16));
        final pixels = await _capture(
          env,
          output,
          'gpui-backdrop-before-panel',
        );
        final stripe = pixels.stripe(logicalY, scale)..sort();
        spread = stripe.last - stripe.first;
        evidence['blurredStripe'] = stripe;
      } while (spread >= 30 && DateTime.now().isBefore(blurDeadline));
      expect(
        spread,
        lessThan(30),
        reason: 'Shell glass must blur the striped wallpaper',
      );
      evidence['blurStripeSpread'] = spread;
      final launcherClock = Stopwatch()..start();
      launcher = await Process.start(env['CLARION_LAUNCHER_BIN']!, [
        '--spike',
        '--output',
        output,
        '--quit-after',
        '20',
      ], environment: env);
      launcherOutput = launcher.stdout.transform(utf8.decoder).join();
      launcherError = launcher.stderr.transform(utf8.decoder).join();
      final mapped = await _waitLayers(
        tester,
        env,
        (layers) =>
            _hasLayer(layers, 'clarion-shell', 'None', output) &&
            _hasLayer(layers, 'clarion-shell-dock', 'None', output) &&
            _hasLayer(layers, 'clarion-launcher', 'Exclusive', output),
      );
      evidence['mappedLayers'] = mapped;
      await tester.pump(const Duration(milliseconds: 100));
      final before = await _capture(env, output, 'gpui-input-before');
      await _key(env, [_sampleText]);
      var changedPixels = 0;
      final textDeadline = DateTime.now().add(const Duration(seconds: 3));
      do {
        await tester.pump(const Duration(milliseconds: 16));
        final typed = await _capture(env, output, 'gpui-input-typed');
        changedPixels = before.textDifference(typed, scale);
      } while (changedPixels < 100 && DateTime.now().isBefore(textDeadline));
      expect(
        changedPixels,
        greaterThanOrEqualTo(100),
        reason: 'GPUI must render virtual-keyboard input',
      );
      evidence['changedSearchPixels'] = changedPixels;
      // Flutter global points are relative to their own native view. Resolve
      // the Dock surface's output-local origin before injecting a real pointer.
      final apps = find.byKey(const ValueKey('dock-launcher'));
      expect(apps, findsOneWidget);
      final dockView = View.of(tester.element(apps));
      final nativeBeforeClick = await _state();
      final dockState = nativeBeforeClick['dock'] as Map;
      expect(dockView.viewId, nativeBeforeClick['dockViewId']);
      final dockLogicalSize = dockView.physicalSize / dockView.devicePixelRatio;
      expect(
        dockLogicalSize.width,
        closeTo((dockState['width'] as num).toDouble(), .01),
      );
      expect(
        dockLogicalSize.height,
        closeTo((dockState['height'] as num).toDouble(), .01),
      );
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
      final monitor = nativeBeforeClick['monitorGeometry'] as Map;
      final outputSize = Size(
        (monitor['width'] as num).toDouble(),
        (monitor['height'] as num).toDouble(),
      );
      expect((Offset.zero & outputSize).contains(target), isTrue);
      final panelBounds = Rect.fromCenter(
        center: outputSize.center(Offset.zero),
        width: 980,
        height: 680,
      );
      expect(
        panelBounds.contains(target),
        isFalse,
        reason: 'The Dock tile must lie outside the GPUI input surface',
      );
      // The live test binding normally only prints hit-test suggestions for
      // native input. Propagate this isolated fixture's actual device events.
      final previousDevicePropagation =
          binding.shouldPropagateDevicePointerEvents;
      binding.shouldPropagateDevicePointerEvents = true;
      try {
        final click = await Process.run(env['CLARION_SHELL_POINTER_HELPER']!, [
          '${target.dx.round()}',
          '${target.dy.round()}',
          '${outputSize.width.round()}',
          '${outputSize.height.round()}',
        ], environment: env).timeout(const Duration(seconds: 3));
        expect(click.exitCode, 0, reason: '${click.stdout}\n${click.stderr}');
        final clickDeadline = DateTime.now().add(const Duration(seconds: 2));
        while (controller.panel != ShellPanel.launcher &&
            DateTime.now().isBefore(clickDeadline)) {
          await tester.pump(const Duration(milliseconds: 16));
        }
        expect(
          controller.panel,
          ShellPanel.launcher,
          reason:
              'The real Dock Apps callback must receive the compositor click',
        );
      } finally {
        binding.shouldPropagateDevicePointerEvents = previousDevicePropagation;
      }
      final clickedNative = await _waitState(
        tester,
        (state) =>
            state['keyboard'] == 'None' &&
            state['launcherBackdropRequested'] == true,
      );
      final clickedLayers = await _waitLayers(
        tester,
        env,
        (layers) =>
            _hasLayer(layers, 'clarion-shell', 'None', output) &&
            _hasLayer(layers, 'clarion-shell-dock', 'None', output) &&
            _hasLayer(layers, 'clarion-launcher', 'Exclusive', output),
      );
      // The old Flutter launcher temporarily appears underneath the probe after
      // its real Apps callback, but must not take input from the GPUI field.
      const afterDockText = 'dock-focus-survives';
      await tester.pump(const Duration(milliseconds: 100));
      final beforeDockInput = await _capture(
        env,
        output,
        'gpui-after-dock-before-input',
      );
      await _key(env, ['-M', 'ctrl', '-k', 'a', '-m', 'ctrl']);
      await _key(env, [afterDockText]);
      await tester.pump(const Duration(milliseconds: 16));
      final afterDockInput = await _capture(
        env,
        output,
        'gpui-after-dock-click',
      );
      final dockInputChanges = beforeDockInput.textDifference(
        afterDockInput,
        scale,
      );
      expect(
        dockInputChanges,
        greaterThanOrEqualTo(100),
        reason: 'The GPUI field itself must repaint text after the Dock click',
      );
      await _key(env, ['-M', 'ctrl', '-k', 'a', '-k', 'c', '-m', 'ctrl']);
      final copied = await Process.run(
        env['CLARION_SHELL_WL_PASTE'] ?? 'wl-paste',
        ['--no-newline'],
        environment: env,
      ).timeout(const Duration(seconds: 2));
      expect(copied.exitCode, 0, reason: '${copied.stdout}\n${copied.stderr}');
      expect(
        copied.stdout,
        afterDockText,
        reason: 'GPUI must retain text input after the real Dock click',
      );
      evidence['dockPointer'] = {
        'route': 'wlr virtual pointer on the isolated compositor',
        'viewId': dockView.viewId,
        'surfaceOrigin': origin,
        'local': {'x': local.dx, 'y': local.dy},
        'outputPoint': {'x': target.dx, 'y': target.dy},
        'controllerPanel': controller.panel.name,
        'native': clickedNative,
        'layers': clickedLayers,
        'gpuiClipboardAfterClick': copied.stdout,
        'changedGpuiSearchPixelsAfterClick': dockInputChanges,
      };
      controller.closePanel();
      await _waitState(tester, (state) => state['expanded'] == false);
      await tester.pump(const Duration(milliseconds: 16));
      expect(find.byType(MacLauncherPanel), findsNothing);
      await _window.invokeMethod<void>('setKeyboardInteractivity', 'none');
      await _window.invokeMethod<void>('setLauncherBackdrop', {
        'visible': true,
        'dockInteractive': true,
      });
      await _window.invokeMethod<void>('setExpanded', true);
      await _waitState(
        tester,
        (state) =>
            state['keyboard'] == 'None' &&
            state['launcherBackdropRequested'] == true,
      );
      expect(
        launcherClock.elapsedMilliseconds,
        lessThan(16000),
        reason: 'Escape must be tested before the 20-second fallback timer',
      );
      await _key(env, ['-k', 'Escape']);
      expect(
        launcherClock.elapsedMilliseconds,
        lessThan(17000),
        reason: 'The two-second Escape assertion must finish before auto-quit',
      );
      final exit = await launcher.exitCode.timeout(const Duration(seconds: 2));
      expect(
        exit,
        0,
        reason: 'GPUI must receive Escape through the real virtual keyboard',
      );
      evidence['launcherExit'] = exit;
      evidence['launcherStdout'] = await launcherOutput;
      evidence['launcherStderr'] = await launcherError;
      launcher = null;
      evidence['afterCloseLayers'] = await _waitLayers(
        tester,
        env,
        (layers) =>
            !layers.any((layer) => layer['namespace'] == 'clarion-launcher'),
      );
      expect((await _state())['keyboard'], 'None');
      await _window.invokeMethod<void>('setExpanded', false);
      final restored = await _waitState(
        tester,
        (state) =>
            state['expanded'] == false &&
            state['launcherBackdropRequested'] == false,
      );
      expect(restored['keyboard'], 'None');
      expect(restored['keyboardInteractivityOverride'], isNull);
      expect((restored['dock'] as Map)['layer'], 'Top');
      evidence['restoredNative'] = restored;
      var restoreError = 255;
      final restoreDeadline = DateTime.now().add(const Duration(seconds: 3));
      do {
        await tester.pump(const Duration(milliseconds: 16));
        final restoredPixels = await _capture(
          env,
          output,
          'gpui-closed-restored',
        );
        final stripe = restoredPixels.stripe(logicalY, scale);
        restoreError = 0;
        for (var i = 0; i < stripe.length; i++) {
          final error = (stripe[i] - baseline[i]).abs();
          if (error > restoreError) restoreError = error;
        }
      } while (restoreError > 2 && DateTime.now().isBefore(restoreDeadline));
      expect(restoreError, lessThanOrEqualTo(2));
      evidence['restoredStripeMaxError'] = restoreError;
      await _window.invokeMethod<void>('setExpanded', true);
      expect(
        (await _state())['keyboard'],
        'Exclusive',
        reason: 'Collapse must clear the override',
      );
      await _window.invokeMethod<void>('setKeyboardInteractivity', 'none');
      expect((await _state())['keyboard'], 'None');
      await _window.invokeMethod<void>('setKeyboardInteractivity', 'default');
      expect((await _state())['keyboard'], 'Exclusive');
      await _window.invokeMethod<void>('setExpanded', false);
      evidence['passed'] = true;
    } finally {
      await _stop(launcher);
      if (launcherOutput != null) {
        evidence['launcherStdout'] = await launcherOutput;
      }
      if (launcherError != null) {
        evidence['launcherStderr'] = await launcherError;
      }
      await _window.invokeMethod<void>('setExpanded', false);
      await tester.pumpWidget(
        const ViewCollection(views: []),
        wrapWithView: false,
      );
      await _window.invokeMethod<void>('setShellStyle', 'material');
      controller.dispose();
      await _stop(background);
      await backgroundOutput;
      await backgroundError;
      evidence['ignoredUnattachedDeviceEvents'] = devices.ignored;
      await File(
        '${captures.path}/gpui-launcher-spike-native-evidence.json',
      ).writeAsString(const JsonEncoder.withIndent('  ').convert(evidence));
    }
  });
}
