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
import 'package:realand_shell/services/wallpaper_service.dart';
import 'package:realand_shell/state/shell_controller.dart';
import 'package:realand_shell/ui/shell_geometry.dart';

const _window = MethodChannel('realand.shell/window');

Future<Map<String, dynamic>> _windowState() async => Map<String, dynamic>.from(
  (await _window.invokeMapMethod<String, dynamic>('windowState'))!,
);

Future<void> _wait(
  WidgetTester tester,
  bool Function() condition,
  String description,
) async {
  final deadline = DateTime.now().add(const Duration(seconds: 20));
  while (!condition()) {
    if (DateTime.now().isAfter(deadline)) fail('Timed out: $description');
    await tester.pump(const Duration(milliseconds: 100));
  }
  await tester.pump();
}

Future<void> _waitBar(WidgetTester tester) async {
  final deadline = DateTime.now().add(const Duration(seconds: 15));
  do {
    await tester.pump(const Duration(milliseconds: 100));
    final state = await _windowState();
    if (state['height'] == ShellGeometry.barHeight &&
        state['expanded'] == false &&
        state['keyboard'] == 'None') {
      expect(state['exclusiveZone'], ShellGeometry.barHeight);
      return;
    }
  } while (DateTime.now().isBefore(deadline));
  fail('Native bar failed to collapse: ${await _windowState()}');
}

Future<void> _waitExpanded(WidgetTester tester) async {
  final deadline = DateTime.now().add(const Duration(seconds: 15));
  do {
    await tester.pump(const Duration(milliseconds: 100));
    final state = await _windowState();
    if (state['expanded'] == true &&
        (state['height'] as int) > ShellGeometry.barHeight &&
        state['keyboard'] == 'Exclusive') {
      return;
    }
  } while (DateTime.now().isBefore(deadline));
  fail('Native gallery failed to expand: ${await _windowState()}');
}

Future<void> _waitExited(WidgetTester tester, int pid) => _wait(
  tester,
  () => !Directory('/proc/$pid').existsSync(),
  'only the owned wallpaper provider $pid must exit',
);

String? _findWtype() {
  final requested = Platform.environment['REALAND_SHELL_WTYPE'] ?? 'wtype';
  final candidates = requested.contains('/')
      ? [requested]
      : [for (final entry in (Platform.environment['PATH'] ?? '').split(':'))
          if (entry.startsWith('/')) '$entry/$requested'];
  for (final candidate in candidates) {
    final file = File(candidate);
    if (file.existsSync() && file.statSync().mode & 0x49 != 0) return candidate;
  }
  return null;
}

Future<void> _realText(String text) async {
  final result = await Process.run(
    _findWtype() ?? (throw StateError('wtype is required for this native input fixture')),
    [text],
  ).timeout(const Duration(seconds: 10));
  expect(result.exitCode, 0, reason: '${result.stdout}\n${result.stderr}');
}

Future<File> _capture(String name) async {
  final directory = Directory(
    Platform.environment['REALAND_SHELL_CAPTURE_DIR']!,
  );
  await directory.create(recursive: true);
  final file = File('${directory.path}/$name.png');
  final result = await Process.run('grim', [
    '-o',
    Platform.environment['REALAND_SHELL_TEST_OUTPUT'] ?? 'winit',
    file.path,
  ]).timeout(const Duration(seconds: 10));
  expect(result.exitCode, 0, reason: '${result.stdout}\n${result.stderr}');
  return file;
}

Future<List<int>> _centerPixel(File file) async {
  final codec = await ui.instantiateImageCodec(await file.readAsBytes());
  try {
    final frame = await codec.getNextFrame();
    try {
      final data = (await frame.image.toByteData(
        format: ui.ImageByteFormat.rawRgba,
      ))!;
      final index =
          ((frame.image.height ~/ 2) * frame.image.width +
              frame.image.width ~/ 2) *
          4;
      return [
        data.getUint8(index),
        data.getUint8(index + 1),
        data.getUint8(index + 2),
      ];
    } finally {
      frame.image.dispose();
    }
  } finally {
    codec.dispose();
  }
}

Future<void> _captureReadyThumbnail(
  WidgetTester tester,
  String path,
  String captureName,
) async {
  final tile = find.byKey(ValueKey('wallpaper-$path'));
  // The interactive InkWell is a sibling above the thumbnail in the tile's
  // Stack. Resolve the closest shared Stack, not the hit-test overlay itself.
  final stack = find.ancestor(of: tile, matching: find.byType(Stack)).first;
  final image = find.descendant(of: stack, matching: find.byType(RawImage));
  await _wait(
    tester,
    () =>
        image.evaluate().length == 1 &&
        tester.widget<RawImage>(image).image != null,
    'the actual wallpaper thumbnail has decoded',
  );
  final thumbnail = tester.widget<RawImage>(image).image!;
  final pixels = (await thumbnail.toByteData(
    format: ui.ImageByteFormat.rawRgba,
  ))!;
  final centerIndex =
      ((thumbnail.height ~/ 2) * thumbnail.width + thumbnail.width ~/ 2) * 4;
  final expected = List.generate(3, (i) => pixels.getUint8(centerIndex + i));
  final center = tester.getCenter(image) * tester.view.devicePixelRatio;
  final deadline = DateTime.now().add(const Duration(seconds: 15));
  List<int> actual = const [];
  do {
    await tester.pump(const Duration(milliseconds: 100));
    final codec = await ui.instantiateImageCodec(
      await (await _capture(captureName)).readAsBytes(),
    );
    try {
      final frame = await codec.getNextFrame();
      try {
        final bytes = (await frame.image.toByteData(
          format: ui.ImageByteFormat.rawRgba,
        ))!;
        final pointIndex =
            (center.dy.round() * frame.image.width + center.dx.round()) * 4;
        actual = List.generate(3, (i) => bytes.getUint8(pointIndex + i));
      } finally {
        frame.image.dispose();
      }
    } finally {
      codec.dispose();
    }
    if (List.generate(
      3,
      (i) => (actual[i] - expected[i]).abs() <= 20,
    ).every((value) => value)) {
      return;
    }
  } while (DateTime.now().isBefore(deadline));
  fail(
    'The compositor did not present the decoded thumbnail: $actual vs $expected',
  );
}

Future<List<int>> _waitPixels(
  WidgetTester tester,
  String name,
  List<int> expected, {
  int tolerance = 0,
}) async {
  final deadline = DateTime.now().add(const Duration(seconds: 15));
  List<int> actual = const [];
  do {
    await tester.pump(const Duration(milliseconds: 100));
    actual = await _centerPixel(await _capture(name));
    if (List.generate(
      3,
      (i) => (actual[i] - expected[i]).abs() <= tolerance,
    ).every((value) => value)) {
      return actual;
    }
  } while (DateTime.now().isBefore(deadline));
  fail('Wallpaper $name pixels $actual, expected $expected ±$tolerance');
}

Future<ShellController> _mount(
  WidgetTester tester,
  WallpaperService service,
) async {
  final controller = ShellController(
    notifications: false,
    tray: false,
    wallpaperService: service,
    restoreWallpaperOnStart: true,
  );
  controller.setThemeMode(ThemeMode.dark);
  controller.setReduceMotion(true);
  await tester.pumpWidget(
    ShellApplication(
      controller: controller,
      preferences: const ShellPreferences(
        themeMode: ThemeMode.dark,
        reduceMotion: true,
      ),
      persist: false,
    ),
  );
  await controller.start();
  await _waitBar(tester);
  await _wait(
    tester,
    () => !controller.busy.contains('wallpaper'),
    'saved wallpaper restoration completes',
  );
  expect(controller.wallpaperError, isNull);
  expect(controller.connected, isTrue);
  return controller;
}

Future<void> _openGallery(
  WidgetTester tester,
  ShellController controller,
) async {
  await tester.tap(find.byKey(const ValueKey('bar-quick-settings')));
  await _wait(
    tester,
    () => controller.panel == ShellPanel.quickSettings,
    'control center opens',
  );
  await _waitExpanded(tester);
  final entry = find.byKey(const ValueKey('choose-wallpaper'));
  await tester.ensureVisible(entry);
  await tester.tap(entry);
  await _wait(
    tester,
    () =>
        controller.panel == ShellPanel.wallpapers &&
        !controller.wallpapersLoading,
    'wallpaper catalog opens',
  );
  final state = await _windowState();
  expect(state['expanded'], isTrue);
  expect(state['keyboard'], 'Exclusive');
  expect(tester.takeException(), isNull);
}

Future<void> _select(
  WidgetTester tester,
  ShellController controller,
  String path,
) async {
  final tile = find.byKey(ValueKey('wallpaper-$path'));
  await tester.ensureVisible(tile);
  await tester.tap(tile);
  await _wait(
    tester,
    () => !controller.busy.contains('wallpaper'),
    'wallpaper selection completes',
  );
}

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  final hasWtype = _findWtype() != null;
  if (!hasWtype) {
    debugPrint('SKIP native wallpaper input fixture: install wtype on PATH or set REALAND_SHELL_WTYPE');
  }
  testWidgets('native wallpaper gallery, pixels, rollback and restoration', (
    tester,
  ) async {
    expect(Platform.isLinux, isTrue);
    expect(Platform.environment['REALAND_SHELL_ISOLATED_TEST'], '1');
    expect(
      Platform.environment['WAYLAND_DISPLAY'],
      isNot(Platform.environment['REALAND_SHELL_PARENT_WAYLAND_DISPLAY']),
    );
    expect(
      Platform.environment['REALAND_SOCKET'],
      isNot(Platform.environment['REALAND_SHELL_PARENT_SOCKET']),
    );
    final testRoot = Platform.environment['REALAND_SHELL_TEST_ROOT']!;
    expect(testRoot, startsWith('/'));
    for (final variable in [
      'XDG_CONFIG_HOME',
      'XDG_STATE_HOME',
      'XDG_CACHE_HOME',
    ]) {
      expect(Platform.environment[variable], startsWith('$testRoot/'));
    }
    final fixtures = '$testRoot/wallpapers';
    final preferenceFile = File('$testRoot/config-home/fixture-wallpaper.json');
    final red = '$fixtures/red.png';
    final green = '$fixtures/green.png';
    final broken = '$fixtures/broken.png';
    final providerPids = <int>[];
    final evidence = <String, dynamic>{};
    WallpaperService? activeService;
    try {
      activeService = WallpaperService(
        directoryPath: fixtures,
        preferenceFile: preferenceFile,
      );
      var controller = await _mount(tester, activeService);
      await _openGallery(tester, controller);
      expect(controller.wallpapers.length, 3, reason: 'skip the video fixture');
      await _select(tester, controller, red);
      expect(controller.selectedWallpaperPath, red);
      expect(controller.wallpaperError, isNull);
      final redPid = activeService.ownedPid!;
      providerPids.add(redPid);
      await _capture('fixture-gallery-red');
      controller.closePanel();
      await _waitBar(tester);
      evidence['redPixels'] = await _waitPixels(tester, 'fixture-red', [
        226,
        43,
        61,
      ]);
      expect(jsonDecode(await preferenceFile.readAsString())['path'], red);

      await _openGallery(tester, controller);
      // UI disables thumbnails that failed to decode. Exercise the service's
      // own decoder-failure protection through its controller action as well.
      await controller.applyWallpaper(
        controller.wallpapers.firstWhere((entry) => entry.path == broken),
      );
      await tester.pump();
      expect(controller.wallpaperError, isNotNull);
      expect(controller.selectedWallpaperPath, red);
      expect(activeService.ownedPid, redPid);
      expect(jsonDecode(await preferenceFile.readAsString())['path'], red);
      await _capture('fixture-gallery-error');
      controller.closePanel();
      await _waitBar(tester);
      evidence['rollbackPixels'] = await _waitPixels(
        tester,
        'fixture-failed-selection-restores-red',
        [226, 43, 61],
      );

      await _openGallery(tester, controller);
      await _select(tester, controller, green);
      expect(controller.selectedWallpaperPath, green);
      expect(controller.wallpaperError, isNull);
      final greenPid = activeService.ownedPid!;
      providerPids.add(greenPid);
      expect(greenPid, isNot(redPid));
      await _waitExited(tester, redPid);
      controller.closePanel();
      await _waitBar(tester);
      evidence['greenPixels'] = await _waitPixels(tester, 'fixture-green', [
        40,
        186,
        105,
      ]);
      expect(jsonDecode(await preferenceFile.readAsString())['path'], green);

      // A fresh controller and provider restore from the real on-disk record.
      // No fake backend or in-memory selection is carried into this instance.
      await tester.pumpWidget(const SizedBox.shrink());
      await _waitExited(tester, greenPid);
      activeService = WallpaperService(
        directoryPath: fixtures,
        preferenceFile: preferenceFile,
      );
      controller = await _mount(tester, activeService);
      expect(controller.selectedWallpaperPath, green);
      final restoredPid = activeService.ownedPid!;
      providerPids.add(restoredPid);
      expect(restoredPid, isNot(greenPid));
      evidence['restoredPixels'] = await _waitPixels(
        tester,
        'fixture-restored-green',
        [40, 186, 105],
      );
      await tester.pumpWidget(const SizedBox.shrink());
      await _waitExited(tester, restoredPid);

      final actualDirectory =
          Platform.environment['REALAND_SHELL_ACTUAL_WALLPAPER_DIR']!;
      activeService = WallpaperService(
        directoryPath: actualDirectory,
        preferenceFile: File('$testRoot/config-home/actual-wallpaper.json'),
      );
      controller = await _mount(tester, activeService);
      await _openGallery(tester, controller);
      expect(controller.wallpapers.length, 57);
      evidence['actualCatalogCount'] = controller.wallpapers.length;
      final search = find.byKey(const ValueKey('wallpaper-search'));
      expect(tester.widget<TextField>(search).focusNode!.hasFocus, isTrue);
      await _realText('lawson_fuji');
      await _wait(
        tester,
        () =>
            tester.widget<TextField>(search).controller!.text == 'lawson_fuji',
        'real Wayland keyboard reaches gallery search',
      );
      final actualPath = '$actualDirectory/lawson_fuji.webp';
      expect(find.byKey(ValueKey('wallpaper-$actualPath')), findsOneWidget);
      expect(tester.widget<TextField>(search).focusNode!.hasFocus, isTrue);
      await _captureReadyThumbnail(tester, actualPath, 'actual-gallery-search');
      await _select(tester, controller, actualPath);
      expect(controller.selectedWallpaperPath, actualPath);
      expect(controller.wallpaperError, isNull);
      final actualPid = activeService.ownedPid!;
      providerPids.add(actualPid);
      controller.closePanel();
      await _waitBar(tester);
      final expected = List<int>.from(
        jsonDecode(Platform.environment['REALAND_SHELL_ACTUAL_EXPECTED_RGB']!)
            as List,
      );
      evidence['actualWebpPixels'] = await _waitPixels(
        tester,
        'actual-lawson-fuji-wallpaper',
        expected,
        tolerance: 20,
      );
      evidence['actualWebpExpectedRgb'] = expected;
      await _openGallery(tester, controller);
      await tester.enterText(search, '【哲风壁纸】暗黑-极简-深色');
      await tester.pump();
      final chinesePath = '$actualDirectory/【哲风壁纸】暗黑-极简-深色.jpg';
      expect(find.byKey(ValueKey('wallpaper-$chinesePath')), findsOneWidget);
      await _select(tester, controller, chinesePath);
      expect(controller.selectedWallpaperPath, chinesePath);
      expect(controller.wallpaperError, isNull);
      providerPids.add(activeService.ownedPid!);
      await _waitExited(tester, actualPid);
      controller.closePanel();
      await _waitBar(tester);
      final chineseExpected = List<int>.from(
        jsonDecode(Platform.environment['REALAND_SHELL_CHINESE_EXPECTED_RGB']!)
            as List,
      );
      evidence['actualChineseJpegPixels'] = await _waitPixels(
        tester,
        'actual-chinese-jpeg-wallpaper',
        chineseExpected,
        tolerance: 20,
      );
      evidence['actualChineseJpegExpectedRgb'] = chineseExpected;
      await _openGallery(tester, controller);
      await tester.tap(find.byTooltip('返回控制中心'));
      await tester.pump();
      expect(controller.panel, ShellPanel.quickSettings);
      await tester.tap(find.byTooltip('关闭'));
      await _waitBar(tester);
      expect(tester.takeException(), isNull);
    } finally {
      final finalPid = activeService?.ownedPid;
      await tester.pumpWidget(const SizedBox.shrink());
      activeService?.dispose();
      if (finalPid != null) await _waitExited(tester, finalPid);
      for (final pid in providerPids) {
        expect(Directory('/proc/$pid').existsSync(), isFalse);
      }
      evidence['ownedProviderPids'] = providerPids;
      await File(
        '${Platform.environment['REALAND_SHELL_CAPTURE_DIR']}/wallpaper-state.json',
      ).writeAsString(
        '${const JsonEncoder.withIndent('  ').convert(evidence)}\n',
      );
    }
  }, skip: !hasWtype, timeout: const Timeout(Duration(minutes: 3)));
}
