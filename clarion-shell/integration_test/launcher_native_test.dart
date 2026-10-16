// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
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
import 'package:clarion_shell/ui/launcher_surface.dart';

const _window = MethodChannel('clarion.shell/window');

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
  required bool expanded,
}) async {
  final deadline = DateTime.now().add(const Duration(seconds: 15));
  Map<String, dynamic> state = const {};
  do {
    await tester.pump(const Duration(milliseconds: 16));
    state = await _state();
    if (state['expanded'] == expanded &&
        (expanded
            ? state['height'] == (state['monitorGeometry'] as Map)['height']
            : state['height'] == 40)) {
      expect(state['keyboard'], expanded ? 'Exclusive' : 'None');
      expect(state['inputHeight'], expanded ? state['height'] : 40);
      expect(state['layer'], expanded ? 'Overlay' : 'Top');
      expect(state['exclusiveZone'], 40);
      return state;
    }
  } while (DateTime.now().isBefore(deadline));
  fail('Native state mismatch: $state');
}

Finder get _search => find.byKey(const ValueKey('launcher-search'));
Finder get _opacity => find.descendant(
  of: find.byType(LauncherSurface),
  matching: find.byType(Opacity),
);
double _progress(WidgetTester tester) =>
    tester.widget<Opacity>(_opacity).opacity;

Future<void> _key(List<String> args) async {
  final result = await Process.run(
    Platform.environment['CLARION_SHELL_WTYPE']!,
    args,
  ).timeout(const Duration(seconds: 10));
  expect(result.exitCode, 0, reason: '${result.stdout}\n${result.stderr}');
}

Future<Map<String, dynamic>> _focusedWindow() async {
  final result = await Process.run(
    Platform.environment['CLARION_SHELL_CLARION_BIN']!,
    ['msg', '-j', 'focused-window'],
  ).timeout(const Duration(seconds: 10));
  expect(result.exitCode, 0, reason: '${result.stdout}\n${result.stderr}');
  return Map<String, dynamic>.from(jsonDecode(result.stdout as String) as Map);
}

Future<void> _stopFixture(Process? process, String root) async {
  final pidFile = File('$root/window-child.pid');
  if (await pidFile.exists()) {
    final pid = int.parse(await pidFile.readAsString());
    final environ = File('/proc/$pid/environ');
    if (await environ.exists()) {
      final values = utf8
          .decode(await environ.readAsBytes(), allowMalformed: true)
          .split('\u0000');
      if (values.contains('CLARION_SHELL_TEST_ROOT=$root')) {
        Process.killPid(pid, ProcessSignal.sigterm);
      }
    }
  }
  if (process != null) {
    await process.exitCode.timeout(
      const Duration(seconds: 3),
      onTimeout: () {
        process.kill(ProcessSignal.sigterm);
        return -1;
      },
    );
    await process.exitCode.timeout(
      const Duration(seconds: 3),
      onTimeout: () {
        process.kill(ProcessSignal.sigkill);
        return -1;
      },
    );
  }
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

Future<_Pixels> _pixels(File file) async {
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

Future<_Pixels> _capture(String name) async {
  final file = File(
    '${Platform.environment['CLARION_SHELL_CAPTURE_DIR']}/$name.png',
  );
  final result = await Process.run('grim', [
    '-o',
    'winit',
    file.path,
  ]).timeout(const Duration(seconds: 15));
  expect(result.exitCode, 0, reason: '${result.stdout}\n${result.stderr}');
  return _pixels(file);
}

List<int> _rgb(Color color) => [
  (color.r * 255).round(),
  (color.g * 255).round(),
  (color.b * 255).round(),
];

Future<List<int>> _captureColor(
  WidgetTester tester,
  String name,
  Offset point,
  List<int> expected, {
  int tolerance = 2,
}) async {
  final deadline = DateTime.now().add(const Duration(seconds: 10));
  List<int> actual = const [];
  do {
    await tester.pump(const Duration(milliseconds: 16));
    actual = (await _capture(name)).at(point);
    if (List.generate(
      3,
      (index) => (actual[index] - expected[index]).abs() <= tolerance,
    ).every((value) => value)) {
      return actual;
    }
  } while (DateTime.now().isBefore(deadline));
  fail('Screenshot $name at $point = $actual, expected $expected');
}

class _Apps extends DesktopApps {
  final launched = <String>[];
  @override
  Future<List<DesktopApp>> load() async => [
    for (var index = 0; index < 50; index++)
      DesktopApp(
        id: 'fixture-$index',
        name: '应用 ${(index + 1).toString().padLeft(2, '0')}',
        desktopFile: '',
        description: '隔离验收应用',
        iconName: const [
          'utilities-terminal',
          'system-file-manager',
          'web-browser',
          'accessories-calculator',
          'multimedia-player',
        ][index % 5],
      ),
  ];
  @override
  Future<void> launch(DesktopApp app) async => launched.add(app.id);
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
  binding.shouldPropagateDevicePointerEvents = true;
  // Native round-trips and grim can consume the remainder of a short animation.
  // Extra pointer-fade frames make its intermediate value nondeterministic.
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.onlyPumps;
  testWidgets('native full-screen launcher, interrupted motion, capture and power card', (
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
    final saved = File(Platform.environment['CLARION_SHELL_SAVED_SCREEN']!);
    expect(saved.absolute.path.startsWith('$root/'), isTrue);
    final apps = _Apps();
    final controller = ShellController(
      demo: false,
      notifications: false,
      tray: false,
      desktopApps: apps,
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
    const preferences = ShellPreferences(
      themeMode: ThemeMode.dark,
      reduceMotion: false,
    );
    final evidence = <String, dynamic>{
      'input':
          'WidgetTester gestures; wtype virtual keyboard restricted to isolated Wayland',
      'fixtureApps': 50,
      'performanceCondition':
          'Debug GTK Flutter Linux runner; nested installed Clarion Vulkan output; compositor animations off; Flutter animations on; binding framePolicy=onlyPumps. FrameTiming callback samples from manual test pumping, not a hardware refresh-rate measurement.',
    };
    final timings = <Map<String, int>>[];
    Process? ordinaryWindow;
    final fixtureLogs = <Future<void>>[];
    void onTimings(List<ui.FrameTiming> frames) {
      timings.addAll(
        frames.map(
          (frame) => {
            'buildMicros': frame.buildDuration.inMicroseconds,
            'rasterMicros': frame.rasterDuration.inMicroseconds,
            'totalMicros': frame.totalSpan.inMicroseconds,
          },
        ),
      );
    }

    binding.addTimingsCallback(onTimings);
    try {
      await tester.pumpWidget(
        ShellApplication(
          controller: controller,
          preferences: preferences,
          persist: false,
        ),
      );
      await controller.start();
      await _wait(
        tester,
        () => controller.connected && controller.apps.length == 50,
        'real nested IPC connects and fake app inventory loads',
      );
      final bar = evidence['barState'] = await _waitNative(
        tester,
        expanded: false,
      );
      final geometry = (bar as Map)['monitorGeometry'] as Map;
      final outputWidth = geometry['width'] as int;
      final outputHeight = geometry['height'] as int;
      final desktopPoint = Offset(outputWidth / 2, 300);
      final baseline = await _capture('native-bar-baseline');
      evidence['baselineDesktopRgb'] = baseline.at(desktopPoint);

      await tester.tap(find.byKey(const ValueKey('bar-launcher')));
      evidence['launcherState'] = await _waitNative(tester, expanded: true);
      await _wait(
        tester,
        () => _opacity.evaluate().isNotEmpty && _progress(tester) >= .999,
        'launcher enter completes',
      );
      expect(tester.widget<TextField>(_search).focusNode!.hasFocus, isTrue);
      expect((await _state())['focused'], isTrue);
      final surface = tester.getRect(find.byType(LauncherSurface));
      expect(surface.left, 0);
      expect(surface.top, 40);
      expect(surface.width, outputWidth.toDouble());
      expect(surface.height, (outputHeight - 40).toDouble());
      evidence['launcherSurfaceRect'] = [
        surface.left,
        surface.top,
        surface.width,
        surface.height,
      ];
      final background = Theme.of(tester.element(_search)).colorScheme.surface;
      evidence['launcherBackdropRgb'] = await _captureColor(
        tester,
        'native-launcher-first',
        Offset(20, 120),
        _rgb(
          Color.alphaBlend(
            background.withValues(alpha: .96),
            const Color(0xff404040),
          ),
        ),
      );

      await _key(['49']);
      await _wait(
        tester,
        () => tester.widget<TextField>(_search).controller!.text == '49',
        'actual isolated virtual keyboard reaches first-open search',
      );
      expect(find.byKey(const ValueKey('app-fixture-48')), findsOneWidget);
      evidence['firstOpenSearch'] = '49';
      await _capture('native-launcher-search');
      await tester.tap(find.byTooltip('清除搜索'));
      await _wait(
        tester,
        () => tester.widget<TextField>(_search).controller!.text.isEmpty,
        'clear-search UI resets query',
      );
      await tester.pump(const Duration(milliseconds: 200));
      final label = find.byKey(const ValueKey('launcher-page-label'));
      await _wait(
        tester,
        () =>
            int.parse(tester.widget<Text>(label).data!.split('/').last.trim()) >
            1,
        'post-layout footer publishes the restored app inventory',
      );
      final pageText = tester.widget<Text>(label).data!;
      evidence['queryAfterClear'] = tester
          .widget<TextField>(_search)
          .controller!
          .text;
      evidence['pageTextAfterClear'] = pageText;
      expect(int.parse(pageText.split('/').last.trim()), greaterThan(1));
      evidence['pagesBeforeScroll'] = pageText;
      final pageView = find.byKey(const ValueKey('launcher-pages'));
      final mouse = TestGesture(
        dispatcher: tester.sendEventToBinding,
        kind: PointerDeviceKind.mouse,
        pointer: 8000,
        device: 8000,
      );
      await mouse.addPointer(location: tester.getCenter(pageView));
      await tester.sendEventToBinding(
        PointerScrollEvent(
          position: tester.getCenter(pageView),
          scrollDelta: const Offset(0, 100),
          device: 8000,
          kind: PointerDeviceKind.mouse,
        ),
      );
      await tester.pump(const Duration(milliseconds: 250));
      await _wait(
        tester,
        () =>
            (tester.widget<PageView>(pageView).controller!.page! - 1).abs() <
            .001,
        'wheel animation finishes on the second actual page',
      );
      await mouse.removePointer();
      expect(tester.widget<Text>(label).data!.startsWith('2 /'), isTrue);
      evidence['pagesAfterScroll'] = tester.widget<Text>(label).data;
      await _capture('native-launcher-page-2');

      // Close retains geometry and keyboard until the exit animation finishes.
      controller.closePanel();
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 30));
      final closing = evidence['closingState'] = await _state();
      expect((closing as Map)['expanded'], isTrue);
      expect(closing['height'], outputHeight);
      expect(closing['keyboard'], 'Exclusive');
      expect(_progress(tester), lessThan(1));
      expect(_progress(tester), greaterThan(0));
      evidence['closingOpacity'] = _progress(tester);
      evidence['closedState'] = await _waitNative(tester, expanded: false);
      expect(_search, findsNothing);

      // Reduced motion closes an actually opened launcher immediately.
      controller.selectPanel(ShellPanel.launcher);
      await _waitNative(tester, expanded: true);
      await _wait(
        tester,
        () => _opacity.evaluate().isNotEmpty && _progress(tester) >= .999,
        'launcher before reduced-motion close',
      );

      controller.setReduceMotion(true);
      await tester.pump();
      controller.closePanel();
      await tester.pump();
      expect(_search, findsNothing);
      evidence['reducedCloseState'] = await _waitNative(
        tester,
        expanded: false,
      );

      await tester.tap(find.byKey(const ValueKey('bar-quick-settings')));
      await _waitNative(tester, expanded: true);
      await tester.pump(const Duration(milliseconds: 100));
      final powerCard = find.byKey(const ValueKey('power-mode-card'));
      await tester.ensureVisible(powerCard);
      await tester.pump();
      expect(find.text('省电'), findsOneWidget);
      expect(find.text('均衡'), findsOneWidget);
      expect(find.text('性能'), findsOneWidget);
      final powerRect = tester.getRect(powerCard);
      final cardColor = Theme.of(tester.element(powerCard)).cardTheme.color!;
      evidence['powerCardRect'] = [
        powerRect.left,
        powerRect.top,
        powerRect.width,
        powerRect.height,
      ];
      evidence['powerCardPixels'] = await _captureColor(
        tester,
        'native-power-card',
        Offset(powerRect.left + 6, powerRect.center.dy),
        _rgb(cardColor),
      );
      await tester.tap(find.byKey(const ValueKey('power-mode-performance')));
      await tester.pump();
      expect(controller.powerProfiles.activeProfile, 'performance');
      await _capture('native-power-performance-demo');

      // The controller must wait for the native modal layer to release before
      // dispatching its genuine ScreenshotScreen IPC action.
      await tester.tap(find.byKey(const ValueKey('bar-screenshot')));
      await _waitNative(tester, expanded: true);
      await tester.pump();
      await _capture('native-screenshot-panel');
      final prepare = controller.prepareScreenshot!;
      final prepared = <Map<String, dynamic>>[];
      controller.prepareScreenshot = () async {
        await prepare();
        prepared.add(await _state());
      };
      await tester.tap(find.byKey(const ValueKey('screenshot-screen')));
      // The UI action owns this asynchronous operation; pump until it leaves
      // busy state so endOfFrame/native resize can settle before IPC capture.
      await _wait(
        tester,
        () => !controller.busy.contains('screenshot') && prepared.isNotEmpty,
        'ScreenshotScreen action releases shell',
      );
      expect(controller.error, isNull);
      evidence['screenshotPreparedState'] = prepared.single;
      expect(prepared.single['height'], 40);
      expect(prepared.single['keyboard'], 'None');
      expect(controller.panel, ShellPanel.none);
      await _wait(
        tester,
        saved.existsSync,
        'compositor writes configured safe screenshot',
      );
      final screenshot = await _pixels(saved);
      expect(screenshot.width, baseline.width);
      expect(screenshot.height, baseline.height);
      final panelPoint = Offset(outputWidth - 220.0, 180);
      expect(screenshot.at(desktopPoint), baseline.at(desktopPoint));
      expect(screenshot.at(panelPoint), baseline.at(panelPoint));
      evidence['savedScreen'] = {
        'path': saved.path,
        'width': screenshot.width,
        'height': screenshot.height,
        'centerRgb': screenshot.at(desktopPoint),
        'formerPanelRgb': screenshot.at(panelPoint),
      };
      await _capture('native-screenshot-released');

      // Also capture directly from a running launcher with motion enabled: the
      // preparation hook must wait through the retained exit surface.
      await saved.copy(
        '${Platform.environment['CLARION_SHELL_CAPTURE_DIR']}/saved-screen-ui.png',
      );
      await saved.delete();
      controller.setReduceMotion(false);
      controller.selectPanel(ShellPanel.launcher);
      await _waitNative(tester, expanded: true);
      await _wait(
        tester,
        () => _opacity.evaluate().isNotEmpty && _progress(tester) >= .999,
        'launcher before animated capture',
      );
      var animatedCaptureDone = false;
      unawaited(
        controller
            .takeScreenshot(ScreenshotMode.screen)
            .then((_) => animatedCaptureDone = true),
      );
      await tester.pump();
      expect((await _state())['expanded'], isTrue);
      await _wait(
        tester,
        () => animatedCaptureDone,
        'animated launcher closes before compositor capture',
      );
      expect(controller.error, isNull);
      expect(prepared.last['height'], 40);
      expect(prepared.last['keyboard'], 'None');
      evidence['animatedScreenshotPreparedState'] = prepared.last;
      await _wait(tester, saved.existsSync, 'animated capture saves');
      final animatedScreenshot = await _pixels(saved);
      expect(animatedScreenshot.at(desktopPoint), baseline.at(desktopPoint));
      expect(animatedScreenshot.at(panelPoint), baseline.at(panelPoint));
      evidence['animatedScreenshotPixels'] = animatedScreenshot.at(
        desktopPoint,
      );

      // A real ordinary client is needed to exercise ScreenshotWindow rather
      // than merely checking its IPC payload. Its config and child stay inside
      // this harness identity, with a distinctive solid background for pixels.
      final footConfig = File('$root/foot.ini');
      await footConfig.writeAsString(
        '[main]\nfont=monospace:size=16\n[colors-dark]\nbackground=27475f\nforeground=ffffff\n[colors-light]\nbackground=27475f\nforeground=ffffff\n',
      );
      final footValidation = await Process.run('/usr/bin/foot', [
        '-C',
        '-c',
        footConfig.path,
      ]);
      expect(footValidation.exitCode, 0, reason: '${footValidation.stderr}');
      ordinaryWindow = await Process.start(
        '/usr/bin/foot',
        [
          '-c',
          footConfig.path,
          '-a',
          'clarion-native-window-fixture',
          '-T',
          'Clarion Screenshot Fixture',
          '-D',
          root,
          'python3',
          '-u',
          '-c',
          'import os,pathlib,time;pathlib.Path(${jsonEncode('$root/window-child.pid')}).write_text(str(os.getpid()));print("CLARION NATIVE WINDOW FIXTURE",flush=True);time.sleep(300)',
        ],
        environment: Map<String, String>.from(Platform.environment),
        workingDirectory: root,
      );
      fixtureLogs.add(ordinaryWindow.stdout.drain<void>());
      fixtureLogs.add(
        ordinaryWindow.stderr.transform(utf8.decoder).join().then((text) async {
          await File('$root/foot.log').writeAsString(text);
        }),
      );
      await _wait(
        tester,
        () => controller.windowTitle == 'Clarion Screenshot Fixture',
        'real ordinary fixture maps and becomes focused',
      );
      evidence['windowBeforeCapture'] = await _focusedWindow();
      await _capture('native-window-fixture');
      await saved.copy(
        '${Platform.environment['CLARION_SHELL_CAPTURE_DIR']}/saved-screen-before-window.png',
      );
      await saved.delete();
      await tester.tap(find.byKey(const ValueKey('bar-screenshot')));
      await _waitNative(tester, expanded: true);
      await tester.tap(find.byKey(const ValueKey('screenshot-window')));
      await _wait(
        tester,
        () => !controller.busy.contains('screenshot') && prepared.length == 3,
        'real ScreenshotWindow returns after surface release',
      );
      expect(controller.error, isNull);
      await _wait(tester, saved.existsSync, 'ordinary window screenshot saves');
      final windowScreenshot = await _pixels(saved);
      final windowRgb = windowScreenshot.at(
        Offset(windowScreenshot.width / 2, windowScreenshot.height / 2),
      );
      expect(windowRgb, [39, 71, 95]);
      expect(windowScreenshot.height, lessThan(baseline.height));
      evidence['windowScreenshot'] = {
        'path': saved.path,
        'width': windowScreenshot.width,
        'height': windowScreenshot.height,
        'centerRgb': windowRgb,
        'nativeState': await _waitNative(tester, expanded: false),
        'preparedState': prepared.last,
        'focusedWindowAfter': await _focusedWindow(),
      };
      expect(
        (evidence['windowScreenshot'] as Map)['focusedWindowAfter']['id'],
        (evidence['windowBeforeCapture'] as Map)['id'],
      );
      await saved.copy(
        '${Platform.environment['CLARION_SHELL_CAPTURE_DIR']}/saved-window.png',
      );
      // Keep wall-clock partial-frame sampling last: GTK/D-Bus round trips in
      // a debug native runner can finish a short animation between two pumps.
      // Strict trajectory continuity is separately covered by fake-clock widget
      // regressions; here only assert directions while a partial frame exists.
      controller.setReduceMotion(false);
      await _window.invokeMethod<void>('setExpanded', true);
      await _waitNative(tester, expanded: true);
      controller.selectPanel(ShellPanel.launcher);
      await _wait(
        tester,
        () => _opacity.evaluate().isNotEmpty && _progress(tester) >= .85,
        'launcher starts before isolated reversal probe',
      );
      final probe = <String, dynamic>{'openingOpacity': _progress(tester)};
      evidence['reversalProbe'] = probe;
      final reversalClock = Stopwatch()..start();
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pump();
      probe['escapePumpWallMicros'] = reversalClock.elapsedMicroseconds;
      if (_opacity.evaluate().isNotEmpty) {
        final reverseStart = _progress(tester);
        probe['reverseStartOpacity'] = reverseStart;
        reversalClock.reset();
        await tester.pump(const Duration(milliseconds: 16));
        probe['reverseFramePumpWallMicros'] = reversalClock.elapsedMicroseconds;
        if (_opacity.evaluate().isNotEmpty) {
          final beforeReopen = _progress(tester);
          probe['reverseFrameOpacity'] = beforeReopen;
          expect(beforeReopen, lessThanOrEqualTo(reverseStart + .001));
          controller.selectPanel(ShellPanel.launcher);
          // Requesting a reversal must retain the currently rendered frame.
          expect(_progress(tester), beforeReopen);
          probe['requestRetainedCurrentFrame'] = true;
          await tester.pump();
          if (_opacity.evaluate().isNotEmpty) {
            final reopen = _progress(tester);
            probe['reopenFrameOpacity'] = reopen;
            expect(reopen, greaterThanOrEqualTo(beforeReopen - .001));
          }
        } else {
          probe['samplingLimit'] =
              'exit completed between debug wall-clock pumps; no partial '
              'reverse frame remained to compare';
          controller.selectPanel(ShellPanel.launcher);
        }
      } else {
        probe['samplingLimit'] =
            'exit completed during Escape dispatch and debug wall-clock pump';
        controller.selectPanel(ShellPanel.launcher);
      }
      reversalClock.stop();
      await _waitNative(tester, expanded: true);
      await _wait(
        tester,
        () => _opacity.evaluate().isNotEmpty && _progress(tester) >= .999,
        'reversal probe can reopen and complete',
      );
      probe['finalOpenState'] = await _state();
      await _capture('native-launcher-reopened');
      controller.closePanel();
      probe['finalClosedState'] = await _waitNative(tester, expanded: false);
      expect(_search, findsNothing);

      await _stopFixture(ordinaryWindow, root);
      ordinaryWindow = null;
      await Future.wait(fixtureLogs);
      fixtureLogs.clear();
      await _wait(
        tester,
        () => controller.windowTitle == 'Clarion',
        'ordinary fixture exits before final region picker',
      );
      await _captureColor(
        tester,
        'native-window-exited',
        desktopPoint,
        baseline.at(desktopPoint),
      );

      // Region picking is the compositor's real Screenshot action. Cancel only
      // on the isolated virtual keyboard and verify no saved image is replaced.
      // Record actual picker disappearance, since virtual keys can bypass grabs.
      controller.setReduceMotion(true);
      await tester.tap(find.byKey(const ValueKey('bar-screenshot')));
      await _waitNative(tester, expanded: true);
      await tester.tap(find.byKey(const ValueKey('screenshot-region')));
      await _wait(
        tester,
        () => !controller.busy.contains('screenshot') && prepared.length == 4,
        'real region action starts after surface release',
      );
      expect(controller.error, isNull);
      final savedBeforeCancel = await saved.readAsBytes();
      await tester.pump(const Duration(milliseconds: 100));
      await _capture('native-region-before-cancel');
      await _key(['-k', 'Escape']);
      await tester.pump(const Duration(milliseconds: 100));
      expect(await saved.readAsBytes(), savedBeforeCancel);
      final afterEscape = await _capture('native-region-after-isolated-escape');
      final cancellationObserved =
          afterEscape.at(desktopPoint).join(',') ==
          baseline.at(desktopPoint).join(',');
      evidence['regionCancel'] = {
        'actionAccepted': true,
        'isolatedEscapeSent': true,
        'cancellationObserved': cancellationObserved,
        'afterEscapeRgb': afterEscape.at(desktopPoint),
        if (!cancellationObserved)
          'limitation':
              'installed compositor virtual keyboard does not route Escape '
              'through its screenshot grab; physical keyboard was not injected. '
              'The isolated compositor exit cleans this picker.',
        'savedImageUnchanged': true,
        'nativeState': await _waitNative(tester, expanded: false),
      };

      expect(apps.launched, isEmpty);
      expect(tester.takeException(), isNull);
      await tester.pump(const Duration(milliseconds: 200));
      evidence['frameTimings'] = timings;
      for (final metric in ['buildMicros', 'rasterMicros', 'totalMicros']) {
        final sorted = timings.map((frame) => frame[metric]!).toList()..sort();
        evidence['$metric-summary'] = sorted.isEmpty
            ? {'count': 0}
            : {
                'count': sorted.length,
                'p50':
                    sorted[(sorted.length * .5).floor().clamp(
                      0,
                      sorted.length - 1,
                    )],
                'p95':
                    sorted[(sorted.length * .95).floor().clamp(
                      0,
                      sorted.length - 1,
                    )],
                'max': sorted.last,
              };
      }
    } finally {
      binding.removeTimingsCallback(onTimings);
      await File(
        '${Platform.environment['CLARION_SHELL_CAPTURE_DIR']}/launcher-state.json',
      ).writeAsString(
        '${const JsonEncoder.withIndent('  ').convert(evidence)}\n',
      );
      await tester.pumpWidget(const SizedBox.shrink());
      await _window.invokeMethod<void>('setExpanded', false);
      await tester.pump(const Duration(milliseconds: 100));
      await _stopFixture(ordinaryWindow, root);
      await Future.wait(fixtureLogs);
    }
  });
}
