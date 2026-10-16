// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:convert';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/mac_app_icons.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/ui/app_icon.dart';
import 'package:clarion_shell/ui/mac_app_icon.dart';

const _channel = MethodChannel('clarion.shell/window');
const _finder = DesktopApp(
  id: 'org.kde.dolphin.desktop',
  name: '文件',
  desktopFile: '/fixture/org.kde.dolphin.desktop',
  iconName: 'org.kde.dolphin',
);
const _pixelPng =
    'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=';

Future<void> _paint(
  WidgetTester tester,
  DesktopApp app, {
  double size = 48,
  double? rasterSize,
  double devicePixelRatio = 1,
  bool useOriginalIcons = true,
}) => tester.pumpWidget(
  MaterialApp(
    home: MediaQuery(
      data: MediaQueryData(devicePixelRatio: devicePixelRatio),
      child: Center(
        child: MacAppIcon(
          app: app,
          size: size,
          rasterSize: rasterSize,
          useOriginalIcons: useOriginalIcons,
        ),
      ),
    ),
  ),
);

ui.Image? _decodedImage(WidgetTester tester) => tester
    .widgetList<RawImage>(find.byType(RawImage))
    .map((image) => image.image)
    .whereType<ui.Image>()
    .firstOrNull;

Future<void> _waitForImage(WidgetTester tester, {int? width}) async {
  final elapsed = Stopwatch()..start();
  for (var turn = 0; turn < 200; turn++) {
    await tester.pump();
    final decoded = _decodedImage(tester);
    if (decoded != null && (width == null || decoded.width == width)) return;
    if (elapsed.elapsed >= const Duration(seconds: 3)) break;
    await tester.runAsync(() async {
      await Future<void>.delayed(const Duration(milliseconds: 10));
    });
  }
  fail('The original application icon did not decode within 3 seconds.');
}

ResizeImage _assetProvider(WidgetTester tester) => tester
    .widgetList<Image>(find.byType(Image))
    .map((image) => image.image)
    .whereType<ResizeImage>()
    .firstWhere((image) => image.imageProvider is AssetImage);

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  setUp(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(_channel, (_) async => null);
  });
  tearDown(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(_channel, null);
  });

  for (final ratio in [1.0, 1.25, 1.5, 2.0]) {
    testWidgets('original Finder texture uses physical pixels at DPR $ratio', (
      tester,
    ) async {
      var nativeCalls = 0;
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(_channel, (_) async {
            nativeCalls++;
            return null;
          });
      await _paint(tester, _finder, devicePixelRatio: ratio);
      final provider = _assetProvider(tester);
      expect(
        (provider.imageProvider as AssetImage).assetName,
        'assets/macos/finder.png',
      );
      expect(provider.width, (48 * ratio).ceil());
      expect(provider.height, (48 * ratio).ceil());
      expect(provider.policy, ResizeImagePolicy.fit);
      expect(provider.allowUpscaling, isFalse);
      await _waitForImage(tester, width: (48 * ratio).ceil());
      final image = tester.widget<Image>(find.byType(Image).first);
      expect(image.width, 48);
      expect(image.height, 48);
      expect(image.filterQuality, FilterQuality.medium);
      expect(image.gaplessPlayback, isTrue);
      expect(find.byType(AppIcon), findsNothing);
      expect(nativeCalls, 0);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    });
  }

  testWidgets(
    'magnification reserves a larger texture without growing layout',
    (tester) async {
      await _paint(
        tester,
        _finder,
        size: 48,
        rasterSize: 128,
        devicePixelRatio: 2,
      );
      final originalElement = tester.element(find.byType(Image).first);
      expect(_assetProvider(tester).width, 256);
      expect(tester.getSize(find.byType(Image).first), const Size(48, 48));
      await _waitForImage(tester, width: 256);
      await _paint(
        tester,
        _finder,
        size: 48,
        rasterSize: 128,
        devicePixelRatio: 2,
      );
      expect(tester.element(find.byType(Image).first), same(originalElement));
      expect(_decodedImage(tester)?.width, 256);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets('oversized physical requests are bounded and never upscale', (
    tester,
  ) async {
    await _paint(tester, _finder, rasterSize: 320, devicePixelRatio: 2);
    expect(_assetProvider(tester).width, 512);
    expect(_assetProvider(tester).height, 512);
    expect(_assetProvider(tester).allowUpscaling, isFalse);
    await _waitForImage(tester);
    expect(_decodedImage(tester)!.width, lessThanOrEqualTo(512));
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  testWidgets('turning off original icons restores the native resolver', (
    tester,
  ) async {
    final calls = <MethodCall>[];
    final iconName = 'original-toggle-${DateTime.now().microsecondsSinceEpoch}';
    final app = DesktopApp(
      id: _finder.id,
      name: _finder.name,
      desktopFile: _finder.desktopFile,
      iconName: iconName,
    );
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(_channel, (call) async {
          calls.add(call);
          return null;
        });
    await _paint(tester, app, rasterSize: 100, devicePixelRatio: 1.5);
    await _waitForImage(tester);
    expect(calls, isEmpty);
    await _paint(
      tester,
      app,
      rasterSize: 100,
      devicePixelRatio: 1.5,
      useOriginalIcons: false,
    );
    await tester.pump();
    expect(find.byType(AppIcon), findsOneWidget);
    expect(
      tester.widget<AppIcon>(find.byType(AppIcon)).fallbackAssetPath,
      isNull,
    );
    expect(calls.single.method, 'appIcon');
    expect(calls.single.arguments, {
      'name': iconName,
      'size': 48,
      'pixelSize': 150,
    });
    expect(find.byIcon(Icons.apps_rounded), findsOneWidget);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  testWidgets(
    'third-party native textures and maximum resolution are retained',
    (tester) async {
      final fixtures = Directory.systemTemp.createTempSync('clarion-mac-icon-');
      addTearDown(() => fixtures.deleteSync(recursive: true));
      final file = File('${fixtures.path}/firefox.png')
        ..writeAsBytesSync(base64Decode(_pixelPng));
      final app = DesktopApp(
        id: 'firefox.desktop',
        name: 'Firefox',
        desktopFile: '/fixture/firefox.desktop',
        iconName: 'web-browser',
        iconPath: file.path,
      );
      await _paint(tester, app, rasterSize: 128, devicePixelRatio: 2);
      final native = tester.widget<AppIcon>(find.byType(AppIcon));
      expect(native.app, same(app));
      expect(native.rasterSize, 128);
      expect(native.fallbackAssetPath, MacAppIcons.genericApplication);
      final image = tester.widget<Image>(find.byType(Image).first);
      final provider = image.image as ResizeImage;
      expect(provider.imageProvider, isA<FileImage>());
      expect(provider.width, 256);
      await _waitForImage(tester, width: 1);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets('a missing native icon uses the original generic application', (
    tester,
  ) async {
    await _paint(
      tester,
      const DesktopApp(
        id: 'missing-native.desktop',
        name: '未安装图标',
        desktopFile: '/fixture/missing-native.desktop',
      ),
      devicePixelRatio: 2,
    );
    expect(find.byType(AppIcon), findsOneWidget);
    expect(
      (_assetProvider(tester).imageProvider as AssetImage).assetName,
      MacAppIcons.genericApplication,
    );
    await _waitForImage(tester, width: 96);
    expect(find.byIcon(Icons.apps_rounded), findsNothing);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  for (final brokenFile in [false, true]) {
    testWidgets(
      'native ${brokenFile ? 'corrupt files' : 'missing theme names'} use the original fallback',
      (tester) async {
        final fixtures = Directory.systemTemp.createTempSync(
          'clarion-mac-icon-fallback-',
        );
        addTearDown(() => fixtures.deleteSync(recursive: true));
        final file = File('${fixtures.path}/broken.png')
          ..writeAsStringSync('not an image');
        var nativeCalls = 0;
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
            .setMockMethodCallHandler(_channel, (call) async {
              if (call.method == 'appIcon') nativeCalls++;
              return null;
            });
        await _paint(
          tester,
          DesktopApp(
            id: 'unmapped-${fixtures.path}',
            name: 'Unmapped',
            desktopFile: '',
            iconName: 'missing-${fixtures.path}',
            iconPath: brokenFile ? file.path : null,
          ),
          devicePixelRatio: 2,
        );
        await _waitForImage(tester, width: 96);
        expect(
          (_assetProvider(tester).imageProvider as AssetImage).assetName,
          MacAppIcons.genericApplication,
        );
        expect(nativeCalls, brokenFile ? 0 : 1);
        expect(find.byIcon(Icons.apps_rounded), findsNothing);
        expect(tester.takeException(), isNull);
        await tester.pumpWidget(const SizedBox.shrink());
      },
    );
  }

  testWidgets('Material AppIcon keeps its original fallback and presentation', (
    tester,
  ) async {
    await tester.pumpWidget(
      const MaterialApp(
        home: Center(
          child: AppIcon(
            app: DesktopApp(
              id: 'org.kde.dolphin.desktop',
              name: '文件',
              desktopFile: '/fixture/org.kde.dolphin.desktop',
            ),
          ),
        ),
      ),
    );
    expect(find.byType(MacAppIcon), findsNothing);
    expect(find.byIcon(Icons.apps_rounded), findsOneWidget);
    expect(find.byType(Image), findsNothing);
    expect(tester.takeException(), isNull);
  });
}
