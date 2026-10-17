// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_svg/flutter_svg.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/ui/app_icon.dart';
import 'package:clarion_shell/ui/mac_app_icon.dart';
import 'package:clarion_shell/ui/optional_asset_icon.dart';

const _channel = MethodChannel('clarion.shell/window');
const _missingAsset = 'assets/shell/public-test-unbundled.png';

class _OwnedImageBundle extends CachingAssetBundle {
  @override
  Future<ByteData> load(String key) async {
    if (key == 'fixtures/owned.png') {
      return ByteData.sublistView(
        base64Decode(
          'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=',
        ),
      );
    }
    // Delegate manifests and unrelated assets; never hide other test errors.
    return rootBundle.load(key);
  }
}

class _MissingAppearanceBundle extends CachingAssetBundle {
  @override
  Future<ByteData> load(String key) {
    if (key.startsWith('assets/shell/') && key.endsWith('.png')) {
      return Future.error(
        FlutterError('Unable to load optional test asset: $key'),
      );
    }
    return rootBundle.load(key);
  }
}

Future<void> _finishImages(WidgetTester tester) async {
  for (var i = 0; i < 10; i++) {
    await tester.pump();
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 5)),
    );
  }
  await tester.pumpAndSettle();
}

void main() {
  setUp(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(_channel, (_) async => null);
  });
  tearDown(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(_channel, null);
  });

  testWidgets('missing private artwork keeps icon bounds and contrast', (
    tester,
  ) async {
    await tester.pumpWidget(
      const MaterialApp(
        home: Center(
          child: OptionalAssetIcon(
            key: ValueKey('optional-icon'),
            asset: _missingAsset,
            fallback: Icons.search_rounded,
            fallbackColor: Colors.white,
            width: 13,
            height: 16,
            rasterSize: 26,
          ),
        ),
      ),
    );
    await _finishImages(tester);
    expect(
      tester.getSize(find.byKey(const ValueKey('optional-icon'))),
      const Size(13, 16),
    );
    final icon = tester.widget<Icon>(find.byIcon(Icons.search_rounded));
    expect(icon.size, 13);
    expect(icon.color, Colors.white);
    expect(tester.takeException(), isNull);
  });

  testWidgets('available owned artwork remains an image', (tester) async {
    await tester.pumpWidget(
      MaterialApp(
        home: DefaultAssetBundle(
          bundle: _OwnedImageBundle(),
          child: const Center(
            child: OptionalAssetIcon(
              asset: 'fixtures/owned.png',
              fallback: Icons.folder_rounded,
              width: 48,
              height: 48,
              rasterSize: 96,
            ),
          ),
        ),
      ),
    );
    await _finishImages(tester);
    expect(find.byIcon(Icons.folder_rounded), findsNothing);
    expect(tester.widget<RawImage>(find.byType(RawImage)).image, isNotNull);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'missing generic folder falls back to a folder rather than an app',
    (tester) async {
      await tester.pumpWidget(
        const MaterialApp(
          home: Center(
            child: AppIcon(
              app: DesktopApp(
                id: 'public-folder',
                name: 'Folder',
                desktopFile: '',
              ),
              fallbackAssetPath: _missingAsset,
              fallbackIcon: Icons.folder_rounded,
              size: 48,
            ),
          ),
        ),
      );
      await _finishImages(tester);
      expect(find.byIcon(Icons.folder_rounded), findsOneWidget);
      expect(find.byIcon(Icons.apps_rounded), findsNothing);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('Clarion mark works without private artwork or fonts', (
    tester,
  ) async {
    await tester.pumpWidget(
      const MaterialApp(home: Center(child: ClarionMark(size: 64))),
    );
    expect(find.text('C'), findsOneWidget);
    expect(find.byType(Image), findsNothing);
    expect(tester.getSize(find.byType(ClarionMark)), const Size(64, 64));
    expect(tester.takeException(), isNull);
  });

  test('bundled font declarations point to shipped font files', () {
    final pubspec = File('pubspec.yaml').readAsStringSync();
    expect(pubspec, contains('family: ClarionSystem'));
    expect(pubspec, contains('family: ClarionChinese'));
    final fonts = RegExp(r'- asset: (assets/shell/[^\n]+)').allMatches(pubspec);
    expect(fonts.length, 3);
    for (final font in fonts) {
      expect(File(font.group(1)!).lengthSync(), greaterThan(0));
    }
  });

  testWidgets('a missing imported bundle still renders the shipped icons', (
    tester,
  ) async {
    await tester.pumpWidget(
      MaterialApp(
        home: DefaultAssetBundle(
          bundle: _MissingAppearanceBundle(),
          child: const Center(
            child: MacAppIcon(
              app: DesktopApp(
                id: 'org.kde.dolphin.desktop',
                name: 'Files',
                desktopFile: '',
              ),
              size: 48,
            ),
          ),
        ),
      ),
    );
    await _finishImages(tester);
    expect(find.byType(SvgPicture), findsOneWidget);
    expect(find.byType(AppIcon), findsNothing);
    expect(tester.takeException(), isNull);
  });
  testWidgets('disabling optional artwork keeps native mode available', (
    tester,
  ) async {
    const app = DesktopApp(
      id: 'org.kde.dolphin.desktop',
      name: 'Files',
      desktopFile: '',
    );
    await tester.pumpWidget(
      const MaterialApp(
        home: Center(child: MacAppIcon(app: app, useOriginalIcons: false)),
      ),
    );
    await _finishImages(tester);
    final native = tester.widget<AppIcon>(find.byType(AppIcon));
    expect(native.fallbackAssetPath, isNull);
    expect(find.byIcon(Icons.apps_rounded), findsOneWidget);
    expect(find.byType(Image), findsNothing);
    expect(tester.takeException(), isNull);
  });
}
