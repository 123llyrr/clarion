// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/ui/app_icon.dart';

const _channel = MethodChannel('clarion.shell/window');
const _boundary = ValueKey('app-icon-test-boundary');
const _pixelPng =
    'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=';
const _greenSvg =
    '<svg xmlns="http://www.w3.org/2000/svg" width="72" height="72" '
    'viewBox="0 0 72 72"><rect width="72" height="72" fill="#24c878"/></svg>';

Future<void> _paint(
  WidgetTester tester,
  DesktopApp app, {
  double size = 72,
  double? rasterSize,
  double? displaySize,
  double devicePixelRatio = 1,
  Brightness brightness = Brightness.dark,
}) => tester.pumpWidget(
  MaterialApp(
    theme: ThemeData(brightness: brightness),
    themeAnimationDuration: Duration.zero,
    home: MediaQuery(
      data: MediaQueryData(devicePixelRatio: devicePixelRatio),
      child: Center(
        child: RepaintBoundary(
          key: _boundary,
          child: SizedBox(
            width: displaySize ?? size,
            height: displaySize ?? size,
            child: FittedBox(
              child: AppIcon(app: app, size: size, rasterSize: rasterSize),
            ),
          ),
        ),
      ),
    ),
  ),
);

bool _hasDecodedPng(WidgetTester tester) => tester
    .widgetList<RawImage>(find.byType(RawImage))
    .any((image) => image.image != null);

ui.Image? _decodedPng(WidgetTester tester) => tester
    .widgetList<RawImage>(find.byType(RawImage))
    .map((image) => image.image)
    .whereType<ui.Image>()
    .firstOrNull;

Future<void> _writeRasterFixture(
  WidgetTester tester,
  File file,
  int size, {
  bool checker = false,
}) async {
  await tester.runAsync(() async {
    final recorder = ui.PictureRecorder();
    final canvas = Canvas(recorder);
    final paint = Paint();
    if (checker) {
      for (var y = 0; y < size; y++) {
        for (var x = 0; x < size; x++) {
          paint.color = (x + y).isEven ? Colors.white : Colors.black;
          canvas.drawRect(
            Rect.fromLTWH(x.toDouble(), y.toDouble(), 1, 1),
            paint,
          );
        }
      }
    } else {
      paint.color = const Color(0xff24c878);
      canvas.drawRect(
        Rect.fromLTWH(0, 0, size.toDouble(), size.toDouble()),
        paint,
      );
    }
    final picture = recorder.endRecording();
    final image = await picture.toImage(size, size);
    try {
      final bytes = await image.toByteData(format: ui.ImageByteFormat.png);
      if (bytes == null) throw StateError('Could not encode PNG fixture.');
      file.writeAsBytesSync(bytes.buffer.asUint8List());
    } finally {
      image.dispose();
      picture.dispose();
    }
  });
}

Future<double> _checkerContrast(WidgetTester tester) async {
  final boundary = tester.renderObject<RenderRepaintBoundary>(
    find.byKey(_boundary),
  );
  return await tester.runAsync(() async {
        final image = await boundary.toImage(pixelRatio: 1);
        try {
          final bytes = await image.toByteData(
            format: ui.ImageByteFormat.rawRgba,
          );
          if (bytes == null) throw StateError('Could not read checker pixels.');
          var contrast = 0;
          var samples = 0;
          final y = image.height ~/ 2;
          // Exclude edges where the surrounding surface could affect sampling.
          for (var x = 8; x < image.width - 8; x++) {
            final offset = (y * image.width + x) * 4;
            contrast += (bytes.getUint8(offset) - bytes.getUint8(offset - 4))
                .abs();
            samples++;
          }
          return contrast / samples;
        } finally {
          image.dispose();
        }
      }) ??
      0;
}

Future<bool> _hasGreenSvgPixels(WidgetTester tester) async {
  final boundary = tester.renderObject<RenderRepaintBoundary>(
    find.byKey(_boundary),
  );
  return await tester.runAsync(() async {
        final image = await boundary.toImage(pixelRatio: 1);
        try {
          final bytes = await image.toByteData(
            format: ui.ImageByteFormat.rawRgba,
          );
          if (bytes == null) return false;
          final offset =
              ((image.height ~/ 2) * image.width + image.width ~/ 2) * 4;
          return bytes.getUint8(offset) == 0x24 &&
              bytes.getUint8(offset + 1) == 0xc8 &&
              bytes.getUint8(offset + 2) == 0x78 &&
              bytes.getUint8(offset + 3) == 0xff;
        } finally {
          image.dispose();
        }
      }) ??
      false;
}

Future<void> _waitForDecodedIcon(
  WidgetTester tester, {
  required bool svg,
  int? pngSize,
}) async {
  final elapsed = Stopwatch()..start();
  for (var turn = 0; turn < 200; turn++) {
    await tester.pump();
    final decoded = _decodedPng(tester);
    final ready = svg
        ? await _hasGreenSvgPixels(tester)
        : pngSize == null
        ? decoded != null
        : decoded?.width == pngSize && decoded?.height == pngSize;
    if (ready) return;
    if (elapsed.elapsed >= const Duration(seconds: 3)) break;
    // File reads and engine decoding need real event-loop turns. Check the
    // rendered result after each turn rather than trusting a fixed delay.
    await tester.runAsync(() async {
      await Future<void>.delayed(const Duration(milliseconds: 10));
    });
  }
  fail(
    'The ${svg ? 'SVG' : 'PNG'} icon did not finish rendering within 3 seconds.',
  );
}

void _mockIconResolver(FutureOr<Object?> Function(MethodCall) handler) {
  TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
      .setMockMethodCallHandler(_channel, (call) async => handler(call));
  addTearDown(
    () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(_channel, null),
  );
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  for (final svg in [false, true]) {
    for (final themed in [false, true]) {
      testWidgets(
        'warm ${svg ? 'SVG' : 'PNG'} ${themed ? 'theme names' : 'absolute paths'} render on the first rebuild and remount frame',
        (tester) async {
          final fixtures = Directory.systemTemp.createTempSync(
            'clarion-app-icon-continuity-',
          );
          addTearDown(() => fixtures.deleteSync(recursive: true));
          final file = File('${fixtures.path}/icon.${svg ? 'svg' : 'png'}');
          if (svg) {
            file.writeAsStringSync(_greenSvg);
          } else {
            file.writeAsBytesSync(base64Decode(_pixelPng));
          }
          var calls = 0;
          _mockIconResolver((call) {
            if (call.method != 'appIcon') return null;
            calls++;
            return file.path;
          });
          final name = 'continuity-${fixtures.path}';
          DesktopApp app(String title) => DesktopApp(
            id: name,
            name: title,
            desktopFile: '',
            iconName: themed ? name : null,
            iconPath: themed ? null : file.path,
          );
          await _paint(tester, app('Initial title'));
          await _waitForDecodedIcon(tester, svg: svg);
          final original = tester.element(find.byType(AppIcon));

          // Ordinary parent rebuilds must not recreate the SVG loader with a
          // different file identity or show an icon-resolution placeholder.
          await _paint(tester, app('Updated title'));
          expect(tester.element(find.byType(AppIcon)), same(original));
          expect(find.byIcon(Icons.apps_rounded), findsNothing);
          if (svg) {
            expect(await _hasGreenSvgPixels(tester), isTrue);
          } else {
            expect(_hasDecodedPng(tester), isTrue);
          }

          // A disposed page gets fresh AppIcon/FutureBuilder state when it
          // returns. Inspect that first frame, before another pump can hide a
          // brief fallback or an empty SVG rendering surface.
          await tester.pumpWidget(const SizedBox.shrink());
          await _paint(tester, app('Returned page'));
          expect(tester.element(find.byType(AppIcon)), isNot(same(original)));
          expect(find.byIcon(Icons.apps_rounded), findsNothing);
          if (svg) {
            expect(await _hasGreenSvgPixels(tester), isTrue);
          } else {
            expect(_hasDecodedPng(tester), isTrue);
          }
          expect(calls, themed ? 1 : 0);
          expect(tester.takeException(), isNull);
          await tester.pumpWidget(const SizedBox.shrink());
        },
      );
    }
  }

  testWidgets('icon resolution caches separate requested sizes and themes', (
    tester,
  ) async {
    final fixtures = Directory.systemTemp.createTempSync(
      'clarion-app-icon-cache-isolation-',
    );
    addTearDown(() => fixtures.deleteSync(recursive: true));
    final icon = File('${fixtures.path}/icon.png')
      ..writeAsBytesSync(base64Decode(_pixelPng));
    final calls = <Object?>[];
    _mockIconResolver((call) {
      if (call.method != 'appIcon') return null;
      calls.add(call.arguments);
      return icon.path;
    });
    final name = 'cache-isolation-${fixtures.path}';
    final app = DesktopApp(
      id: name,
      name: 'Cache isolation',
      desktopFile: '',
      iconName: name,
    );

    for (final variant in [
      (size: 40.0, brightness: Brightness.dark),
      (size: 40.0, brightness: Brightness.dark),
      (size: 72.0, brightness: Brightness.dark),
      (size: 72.0, brightness: Brightness.light),
      (size: 40.0, brightness: Brightness.dark),
    ]) {
      await _paint(
        tester,
        app,
        size: variant.size,
        brightness: variant.brightness,
      );
      await _waitForDecodedIcon(tester, svg: false);
    }
    expect(calls, [
      {'name': name, 'size': 40, 'pixelSize': 40},
      {'name': name, 'size': 72, 'pixelSize': 72},
      {'name': name, 'size': 72, 'pixelSize': 72},
    ]);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  for (final sourceSize in [96, 128]) {
    testWidgets(
      '$sourceSize pixel sources decode for physical DPI without upscaling',
      (tester) async {
        final fixtures = Directory.systemTemp.createTempSync(
          'clarion-app-icon-dpi-',
        );
        addTearDown(() => fixtures.deleteSync(recursive: true));
        final file = File('${fixtures.path}/icon.png');
        await _writeRasterFixture(tester, file, sourceSize);
        final requests = <Object?>[];
        _mockIconResolver((call) {
          if (call.method != 'appIcon') return null;
          requests.add(call.arguments);
          return file.path;
        });
        final name = 'dpi-${fixtures.path}';
        final app = DesktopApp(
          id: name,
          name: 'Physical DPI',
          desktopFile: '',
          iconName: name,
        );

        for (final ratio in [1.0, 1.25, 1.5, 2.0]) {
          final pixels = (72 * ratio).ceil();
          final expected = pixels < sourceSize ? pixels : sourceSize;
          await _paint(tester, app, devicePixelRatio: ratio);
          await _waitForDecodedIcon(tester, svg: false, pngSize: expected);
          final provider = tester.widget<Image>(find.byType(Image)).image;
          expect(provider, isA<ResizeImage>());
          final resize = provider as ResizeImage;
          expect(resize.width, pixels);
          expect(resize.height, pixels);
          expect(resize.allowUpscaling, isFalse);
          expect(resize.policy, ResizeImagePolicy.fit);
          expect(_decodedPng(tester)?.width, expected);
        }
        expect(requests, [
          for (final ratio in [1.0, 1.25, 1.5, 2.0])
            {'name': name, 'size': 72, 'pixelSize': (72 * ratio).ceil()},
        ]);
        expect(tester.takeException(), isNull);
        await tester.pumpWidget(const SizedBox.shrink());
      },
    );
  }

  testWidgets(
    'maximum Dock magnification has enough decoded pixels at each DPI',
    (tester) async {
      final fixtures = Directory.systemTemp.createTempSync(
        'clarion-app-icon-magnification-',
      );
      addTearDown(() => fixtures.deleteSync(recursive: true));
      final file = File('${fixtures.path}/icon.png');
      await _writeRasterFixture(tester, file, 512);
      final requests = <Object?>[];
      _mockIconResolver((call) {
        if (call.method != 'appIcon') return null;
        requests.add(call.arguments);
        return file.path;
      });
      final name = 'magnification-${fixtures.path}';
      final app = DesktopApp(
        id: name,
        name: 'Maximum magnification',
        desktopFile: '',
        iconName: name,
      );
      for (final ratio in [1.0, 1.25, 1.5, 2.0]) {
        // Dock fits its 48 logical pixel child to 64, then magnifies up to 2x.
        final pixels = (64 * 2 * ratio).ceil();
        await _paint(
          tester,
          app,
          size: 48,
          rasterSize: 128,
          displaySize: 128,
          devicePixelRatio: ratio,
        );
        await _waitForDecodedIcon(tester, svg: false, pngSize: pixels);
        expect(tester.widget<AppIcon>(find.byType(AppIcon)).size, 48);
        expect(_decodedPng(tester)?.width, pixels);
      }
      expect(requests, [
        for (final ratio in [1.0, 1.25, 1.5, 2.0])
          {'name': name, 'size': 48, 'pixelSize': (128 * ratio).ceil()},
      ]);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets('fractional pixel requirements round up and stay bounded', (
    tester,
  ) async {
    final fixtures = Directory.systemTemp.createTempSync(
      'clarion-app-icon-pixel-bounds-',
    );
    addTearDown(() => fixtures.deleteSync(recursive: true));
    final file = File('${fixtures.path}/icon.png');
    await _writeRasterFixture(tester, file, 512);
    final requests = <Object?>[];
    _mockIconResolver((call) {
      if (call.method != 'appIcon') return null;
      requests.add(call.arguments);
      return file.path;
    });
    final name = 'pixel-bounds-${fixtures.path}';
    final app = DesktopApp(
      id: name,
      name: 'Pixel bounds',
      desktopFile: '',
      iconName: name,
    );
    for (final variant in [
      (raster: 48.1, ratio: 1.25, pixels: 61),
      (raster: 1.0, ratio: 1.0, pixels: 16),
      (raster: 300.0, ratio: 2.0, pixels: 512),
    ]) {
      await _paint(
        tester,
        app,
        size: 48,
        rasterSize: variant.raster,
        devicePixelRatio: variant.ratio,
      );
      await _waitForDecodedIcon(tester, svg: false, pngSize: variant.pixels);
    }
    expect(requests, [
      {'name': name, 'size': 48, 'pixelSize': 61},
      {'name': name, 'size': 48, 'pixelSize': 16},
      {'name': name, 'size': 48, 'pixelSize': 512},
    ]);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  testWidgets(
    'equivalent physical sizes share resolution while themes stay isolated',
    (tester) async {
      final fixtures = Directory.systemTemp.createTempSync(
        'clarion-app-icon-physical-cache-',
      );
      addTearDown(() => fixtures.deleteSync(recursive: true));
      final file = File('${fixtures.path}/icon.png');
      await _writeRasterFixture(tester, file, 128);
      final requests = <Object?>[];
      _mockIconResolver((call) {
        if (call.method != 'appIcon') return null;
        requests.add(call.arguments);
        return file.path;
      });
      final name = 'physical-cache-${fixtures.path}';
      final app = DesktopApp(
        id: name,
        name: 'Shared physical cache',
        desktopFile: '',
        iconName: name,
      );
      for (final (visit, variant) in [
        (size: 40.0, raster: 80.0, ratio: 1.0, brightness: Brightness.dark),
        (size: 64.0, raster: 64.0, ratio: 1.25, brightness: Brightness.dark),
        (size: 64.0, raster: 64.0, ratio: 1.25, brightness: Brightness.light),
        (size: 40.0, raster: 80.0, ratio: 1.0, brightness: Brightness.dark),
      ].indexed) {
        await tester.pumpWidget(const SizedBox.shrink());
        await _paint(
          tester,
          app,
          size: variant.size,
          rasterSize: variant.raster,
          devicePixelRatio: variant.ratio,
          brightness: variant.brightness,
        );
        if (visit == 3) {
          // The last visit has both a warm path and texture, so its first frame
          // must paint without waiting on another FutureBuilder notification.
          expect(find.byIcon(Icons.apps_rounded), findsNothing);
          expect(_hasDecodedPng(tester), isTrue);
        }
        await _waitForDecodedIcon(tester, svg: false, pngSize: 80);
      }
      expect(requests, [
        {'name': name, 'size': 40, 'pixelSize': 80},
        {'name': name, 'size': 64, 'pixelSize': 80},
      ]);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets('simultaneous physical-size requests resolve once', (
    tester,
  ) async {
    final fixtures = Directory.systemTemp.createTempSync(
      'clarion-app-icon-pending-cache-',
    );
    addTearDown(() => fixtures.deleteSync(recursive: true));
    final file = File('${fixtures.path}/icon.png');
    await _writeRasterFixture(tester, file, 128);
    final pending = Completer<String?>();
    var calls = 0;
    _mockIconResolver((call) {
      if (call.method != 'appIcon') return null;
      calls++;
      return pending.future;
    });
    final name = 'pending-cache-${fixtures.path}';
    final app = DesktopApp(
      id: name,
      name: 'Shared pending icon',
      desktopFile: '',
      iconName: name,
    );
    Widget icons() => MaterialApp(
      home: MediaQuery(
        data: const MediaQueryData(devicePixelRatio: 1),
        child: Row(
          children: [
            AppIcon(app: app, size: 40, rasterSize: 128),
            AppIcon(app: app, size: 72, rasterSize: 128),
          ],
        ),
      ),
    );
    await tester.pumpWidget(icons());
    await tester.pumpWidget(icons());
    expect(calls, 1);
    pending.complete(file.path);
    await _waitForDecodedIcon(tester, svg: false, pngSize: 128);
    expect(
      tester
          .widgetList<RawImage>(find.byType(RawImage))
          .where((image) => image.image?.width == 128),
      hasLength(2),
    );
    expect(calls, 1);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  for (final themed in [false, true]) {
    testWidgets(
      '${themed ? 'theme resolution' : 'absolute path'} size upgrades retain the decoded icon until replacement is ready',
      (tester) async {
        final fixtures = Directory.systemTemp.createTempSync(
          'clarion-app-icon-size-upgrade-',
        );
        addTearDown(() => fixtures.deleteSync(recursive: true));
        final smaller = File('${fixtures.path}/small.png');
        final larger = File('${fixtures.path}/large.png');
        await _writeRasterFixture(tester, smaller, 96);
        await _writeRasterFixture(tester, larger, 128);
        final pending = Completer<String?>();
        var calls = 0;
        _mockIconResolver((call) {
          if (call.method != 'appIcon') return null;
          calls++;
          final arguments = call.arguments as Map<Object?, Object?>;
          return arguments['pixelSize'] == 40 ? smaller.path : pending.future;
        });
        final name = 'size-upgrade-${fixtures.path}';
        final app = DesktopApp(
          id: name,
          name: 'Size upgrade',
          desktopFile: '',
          iconName: themed ? name : null,
          iconPath: themed ? null : larger.path,
        );
        await _paint(tester, app, size: 40);
        await _waitForDecodedIcon(tester, svg: false, pngSize: 40);
        final original = _decodedPng(tester);

        await _paint(tester, app, size: 40, rasterSize: 128);
        expect(find.byIcon(Icons.apps_rounded), findsNothing);
        expect(_decodedPng(tester), same(original));
        if (themed) {
          for (var frame = 0; frame < 3; frame++) {
            await tester.pump();
            expect(find.byIcon(Icons.apps_rounded), findsNothing);
            expect(_hasDecodedPng(tester), isTrue);
          }
          expect(calls, 2);
          pending.complete(larger.path);
          await tester.pump();
          expect(find.byIcon(Icons.apps_rounded), findsNothing);
          expect(_hasDecodedPng(tester), isTrue);
        }
        await _waitForDecodedIcon(tester, svg: false, pngSize: 128);
        expect(calls, themed ? 2 : 0);
        expect(tester.takeException(), isNull);
        await tester.pumpWidget(const SizedBox.shrink());
      },
    );
  }

  testWidgets('maximum magnification retains single-pixel checker detail', (
    tester,
  ) async {
    final fixtures = Directory.systemTemp.createTempSync(
      'clarion-app-icon-checker-',
    );
    addTearDown(() => fixtures.deleteSync(recursive: true));
    final file = File('${fixtures.path}/checker.png');
    await _writeRasterFixture(tester, file, 128, checker: true);
    final app = DesktopApp(
      id: 'checker-${fixtures.path}',
      name: 'Checker detail',
      desktopFile: '',
      iconPath: file.path,
    );
    await _paint(tester, app, size: 48, rasterSize: 128, displaySize: 128);
    await _waitForDecodedIcon(tester, svg: false, pngSize: 128);
    final sharpContrast = await _checkerContrast(tester);
    expect(
      tester.widget<Image>(find.byType(Image)).filterQuality,
      FilterQuality.medium,
    );
    expect(sharpContrast, greaterThan(230));

    // Reproduce the old 48-pixel decode enlarged through a 64-pixel Dock tile
    // and its 2x magnification. The sampling filter cannot recover lost pixels.
    await _paint(tester, app, size: 48, displaySize: 128);
    await _waitForDecodedIcon(tester, svg: false, pngSize: 48);
    final undersampledContrast = await _checkerContrast(tester);
    expect(undersampledContrast, lessThan(80));
    expect(sharpContrast, greaterThan(undersampledContrast * 3));
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  testWidgets('SVG icons stay vector rendered at maximum size and high DPI', (
    tester,
  ) async {
    final fixtures = Directory.systemTemp.createTempSync(
      'clarion-app-icon-vector-',
    );
    addTearDown(() => fixtures.deleteSync(recursive: true));
    final file = File('${fixtures.path}/icon.svg')
      ..writeAsStringSync(_greenSvg);
    final app = DesktopApp(
      id: 'vector-${fixtures.path}',
      name: 'Vector icon',
      desktopFile: '',
      iconPath: file.path,
    );
    await _paint(
      tester,
      app,
      size: 48,
      rasterSize: 128,
      displaySize: 128,
      devicePixelRatio: 2,
    );
    await _waitForDecodedIcon(tester, svg: true);
    expect(find.byType(RawImage), findsNothing);
    expect(await _hasGreenSvgPixels(tester), isTrue);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  testWidgets('missing icons do not repeatedly query the theme while paging', (
    tester,
  ) async {
    final fixtures = Directory.systemTemp.createTempSync(
      'clarion-app-icon-negative-cache-',
    );
    addTearDown(() => fixtures.deleteSync(recursive: true));
    var calls = 0;
    _mockIconResolver((call) {
      if (call.method == 'appIcon') calls++;
      return null;
    });
    final name = 'missing-${fixtures.path}';
    final app = DesktopApp(
      id: name,
      name: 'Missing icon',
      desktopFile: '',
      iconName: name,
    );
    await _paint(tester, app);
    await tester.pumpAndSettle();
    expect(find.byIcon(Icons.apps_rounded), findsOneWidget);
    for (var visit = 0; visit < 3; visit++) {
      await tester.pumpWidget(const SizedBox.shrink());
      await _paint(tester, app);
      await tester.pumpAndSettle();
      expect(find.byIcon(Icons.apps_rounded), findsOneWidget);
    }
    expect(calls, 1);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  });
}
