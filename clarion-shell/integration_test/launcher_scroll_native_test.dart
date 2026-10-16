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
import 'package:clarion_shell/ui/app_icon.dart';

const _window = MethodChannel('clarion.shell/window');
const _pngRgb = [21, 170, 102];
const _svgRgb = [202, 65, 145];

Finder get _viewport => find.byKey(const ValueKey('launcher-pages'));
Finder get _fallbacks =>
    find.descendant(of: _viewport, matching: find.byIcon(Icons.apps_rounded));
PageController _pages(WidgetTester tester) =>
    tester.widget<PageView>(_viewport).controller!;

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
    final height = expanded ? (state['monitorGeometry'] as Map)['height'] : 40;
    if (state['expanded'] == expanded && state['height'] == height) {
      expect(state['keyboard'], expanded ? 'Exclusive' : 'None');
      expect(state['inputHeight'], expanded ? height : 40);
      expect(state['layer'], expanded ? 'Overlay' : 'Top');
      expect(state['exclusiveZone'], 40);
      return state;
    }
  } while (DateTime.now().isBefore(deadline));
  fail('Native state mismatch: $state');
}

Future<void> _fixtures(String root) async {
  final recorder = ui.PictureRecorder();
  final canvas = ui.Canvas(recorder);
  canvas.drawRect(
    const ui.Rect.fromLTWH(0, 0, 72, 72),
    ui.Paint()..color = const ui.Color(0xff15aa66),
  );
  final picture = recorder.endRecording();
  final image = await picture.toImage(72, 72);
  try {
    final data = await image.toByteData(format: ui.ImageByteFormat.png);
    await File('$root/icon.png').writeAsBytes(data!.buffer.asUint8List());
  } finally {
    image.dispose();
    picture.dispose();
  }
  await File('$root/icon.svg').writeAsString(
    '<svg xmlns="http://www.w3.org/2000/svg" width="72" height="72" '
    'viewBox="0 0 72 72"><rect width="72" height="72" fill="#ca4191"/></svg>',
  );
}

class _Apps extends DesktopApps {
  _Apps(this.root);
  final String root;

  @override
  Future<List<DesktopApp>> load() async => [
    for (var index = 0; index < 128; index++)
      DesktopApp(
        id: 'scroll-fixture-$index',
        name: '隔离应用 ${index.toString().padLeft(3, '0')}',
        desktopFile: '',
        iconPath: '$root/icon.${index.isEven ? 'png' : 'svg'}',
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

class _Controller extends ShellController {
  _Controller(String root)
    : super(
        notifications: false,
        tray: false,
        desktopApps: _Apps(root),
        system: _System(),
        powerProfiles: PowerProfilesService(demo: true),
        wallpaperService: WallpaperService(
          directoryPath: '$root/wallpapers',
          preferenceFile: File('$root/wallpaper.json'),
        ),
        restoreWallpaperOnStart: false,
      );

  void pulse() => notifyListeners();
}

Future<List<int>> _captureIcon(
  WidgetTester tester,
  int index,
  String phase,
) async {
  final tile = find.byKey(ValueKey('app-scroll-fixture-$index'));
  final icon = find.descendant(of: tile, matching: find.byType(AppIcon));
  expect(icon, findsOneWidget);
  final center = tester.getCenter(icon);
  final x = (center.dx - 6).round();
  final y = (center.dy - 6).round();
  final file = File(
    '${Platform.environment['CLARION_SHELL_CAPTURE_DIR']}/$phase-$index.png',
  );
  final result = await Process.run('grim', [
    '-s',
    '1',
    '-g',
    '$x,$y 12x12',
    file.path,
  ]).timeout(const Duration(seconds: 15));
  expect(result.exitCode, 0, reason: '${result.stdout}\n${result.stderr}');
  final codec = await ui.instantiateImageCodec(await file.readAsBytes());
  final frame = await codec.getNextFrame();
  try {
    expect(frame.image.width, 12);
    expect(frame.image.height, 12);
    final data = await frame.image.toByteData(
      format: ui.ImageByteFormat.rawRgba,
    );
    final bytes = data!.buffer.asUint8List();
    final actual = bytes.sublist((6 * 12 + 6) * 4, (6 * 12 + 6) * 4 + 3);
    final expected = index.isEven ? _pngRgb : _svgRgb;
    for (var channel = 0; channel < 3; channel++) {
      expect(
        (actual[channel] - expected[channel]).abs(),
        lessThanOrEqualTo(3),
        reason:
            '$phase icon $index must contain real pixels: $actual vs $expected',
      );
    }
    return actual;
  } finally {
    frame.image.dispose();
    codec.dispose();
  }
}

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.onlyPumps;
  testWidgets('native launcher PNG and SVG survive scrolling and page remounts', (
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
    await _fixtures(root);
    final controller = _Controller(root);
    controller.setThemeMode(ThemeMode.dark);
    controller.setReduceMotion(false);
    final evidence = <String, dynamic>{
      'input':
          'WidgetTester wheel and touch events target this isolated binding only.',
      'condition':
          '128 fixture applications sharing real local PNG and SVG; debug GTK Flutter Linux runner; nested Clarion Vulkan scale 1; onlyPumps. No main-session physical input, screenshot action, or performance claim.',
      'checks': <Map<String, dynamic>>[],
      'pixels': <Map<String, dynamic>>[],
    };
    TestGesture? mouse;
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
        () => controller.connected && controller.apps.length == 128,
        'nested IPC and 128 fixture applications',
      );
      final initial = await _waitNative(tester, expanded: false);
      expect(initial['scaleFactor'], 1);
      controller.selectPanel(ShellPanel.launcher);
      await tester.pump();
      evidence['openNativeState'] = await _waitNative(tester, expanded: true);
      await _wait(
        tester,
        () => _viewport.evaluate().isNotEmpty,
        'launcher grid',
      );
      await tester.pump(const Duration(milliseconds: 350));
      await _wait(
        tester,
        () => _fallbacks.evaluate().isEmpty,
        'initial SVG loads',
      );
      final pageCount = tester
          .widget<PageView>(_viewport)
          .childrenDelegate
          .estimatedChildCount!;
      expect(pageCount, greaterThanOrEqualTo(4));
      evidence['pageCount'] = pageCount;

      void check(String phase) {
        expect(
          _fallbacks,
          findsNothing,
          reason: '$phase must not repaint warm icons as placeholders',
        );
        expect(tester.takeException(), isNull, reason: phase);
        expect(controller.panel, ShellPanel.launcher);
        (evidence['checks'] as List).add({
          'phase': phase,
          'page': _pages(tester).page,
          'fallbackCount': _fallbacks.evaluate().length,
        });
      }

      Future<void> settle(int page, String phase) async {
        await _wait(
          tester,
          () => (_pages(tester).page! - page).abs() < .001,
          '$phase finishes on page $page',
        );
        await tester.pump();
        check(phase);
      }

      Future<void> pixels(String phase) async {
        (evidence['pixels'] as List).add({
          'phase': phase,
          'png': await _captureIcon(tester, 0, phase),
          'svg': await _captureIcon(tester, 1, phase),
        });
      }

      // Warm and dispose at least four distinct page subtrees before returning
      // to page zero. The test deliberately keeps PageView's bounded lifecycle.
      for (var page = 1; page <= 3; page++) {
        _pages(tester).jumpToPage(page);
        await tester.pump();
        await _wait(
          tester,
          () => _fallbacks.evaluate().isEmpty,
          'warm page $page',
        );
        check('warm-page-$page');
      }
      _pages(tester).jumpToPage(0);
      await tester.pump();
      check('remounted-page-zero-first-frame');
      await pixels('remounted-page-zero');

      mouse = TestGesture(
        dispatcher: tester.sendEventToBinding,
        kind: PointerDeviceKind.mouse,
        pointer: 9400,
        device: 9400,
      );
      await mouse.addPointer(location: tester.getCenter(_viewport));
      Future<void> wheel(int delta, int page, String phase) async {
        await tester.pump(const Duration(milliseconds: 300));
        await tester.sendEventToBinding(
          PointerScrollEvent(
            position: tester.getCenter(_viewport),
            scrollDelta: Offset(0, delta.toDouble()),
            device: 9400,
          ),
        );
        await tester.pump();
        check('$phase-first-frame');
        await settle(page, phase);
      }

      for (final reduced in [false, true]) {
        final label = reduced ? 'reduced' : 'animated';
        controller.setReduceMotion(reduced);
        await tester.pump();
        check('$label-preference');
        for (var page = 1; page <= 3; page++) {
          await wheel(120, page, '$label-wheel-forward-$page');
        }
        for (var page = 2; page >= 0; page--) {
          await wheel(-120, page, '$label-wheel-back-$page');
        }
        for (var page = 1; page <= 3; page++) {
          await tester.drag(
            _viewport,
            Offset(-tester.getSize(_viewport).width * .8, 0),
          );
          await tester.pump();
          check('$label-touch-forward-$page-first-frame');
          await settle(page, '$label-touch-forward-$page');
        }
        for (var page = 2; page >= 0; page--) {
          await tester.drag(
            _viewport,
            Offset(tester.getSize(_viewport).width * .8, 0),
          );
          await tester.pump();
          check('$label-touch-back-$page-first-frame');
          await settle(page, '$label-touch-back-$page');
        }
        controller.pulse();
        await tester.pump();
        check('$label-ordinary-controller-notification');
        await pixels('$label-after-wheel-touch');
      }

      controller.setReduceMotion(false);
      await tester.pump();
      for (var turn = 0; turn < 3; turn++) {
        await tester.tap(find.byKey(const ValueKey('launcher-next')));
        await tester.pump(const Duration(milliseconds: 16));
        check('rapid-next-$turn');
      }
      for (var turn = 0; turn < 3; turn++) {
        await tester.tap(find.byKey(const ValueKey('launcher-prev')));
        await tester.pump(const Duration(milliseconds: 16));
        check('rapid-prev-$turn');
      }
      await settle(0, 'rapid-direction-reversal');
      await pixels('rapid-direction-reversal');
      await mouse.removePointer();
      mouse = null;
      controller.closePanel();
      await tester.pump();
      evidence['closedNativeState'] = await _waitNative(
        tester,
        expanded: false,
      );
      evidence['passed'] = true;
    } finally {
      if (mouse != null) await mouse.removePointer();
      await File(
        '${Platform.environment['CLARION_SHELL_CAPTURE_DIR']}/launcher-scroll-native-evidence.json',
      ).writeAsString(
        '${const JsonEncoder.withIndent('  ').convert(evidence)}\n',
      );
      await tester.pumpWidget(const SizedBox.shrink());
    }
  });
}
