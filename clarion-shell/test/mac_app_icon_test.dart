// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_svg/flutter_svg.dart';
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
}) async {
  await tester.pumpWidget(
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
  await tester.pumpAndSettle();
}

SvgAssetLoader _svgLoader(WidgetTester tester) => tester
    .widgetList<SvgPicture>(find.byType(SvgPicture))
    .map((picture) => picture.bytesLoader)
    .whereType<SvgAssetLoader>()
    .first;

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

  testWidgets('theme role icons render the bundled svg without a native call', (
    tester,
  ) async {
    var nativeCalls = 0;
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(_channel, (_) async {
          nativeCalls++;
          return null;
        });
    await _paint(tester, _finder);
    expect(_svgLoader(tester).assetName, 'assets/icons/home.svg');
    expect(tester.getSize(find.byType(SvgPicture).first), const Size(48, 48));
    expect(find.byType(AppIcon), findsNothing);
    expect(nativeCalls, 0);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  testWidgets('svg role icons keep their layout extent for magnification', (
    tester,
  ) async {
    await _paint(tester, _finder, rasterSize: 128, devicePixelRatio: 2);
    expect(tester.getSize(find.byType(SvgPicture).first), const Size(48, 48));
    expect(_svgLoader(tester).assetName, 'assets/icons/home.svg');
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
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets('a missing native icon uses the bundled generic application', (
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
    expect(_svgLoader(tester).assetName, 'assets/icons/generic-application.svg');
    expect(find.byIcon(Icons.apps_rounded), findsNothing);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  for (final brokenFile in [false, true]) {
    testWidgets(
      'native ${brokenFile ? 'corrupt files' : 'missing theme names'} use the bundled fallback',
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
        // A corrupt native file only surfaces its error through real file IO,
        // which widget-test FakeAsync does not drive by itself.
        await tester.runAsync(() async {
          for (var turn = 0; turn < 50; turn++) {
            await Future<void>.delayed(const Duration(milliseconds: 20));
            await tester.pump();
            if (find.byType(SvgPicture).evaluate().isNotEmpty) return;
          }
        });
        expect(_svgLoader(tester).assetName, 'assets/icons/generic-application.svg');
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
