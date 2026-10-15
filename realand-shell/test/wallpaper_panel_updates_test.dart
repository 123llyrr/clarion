// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:realand_shell/models.dart';
import 'package:realand_shell/services/wallpaper_service.dart';
import 'package:realand_shell/state/shell_controller.dart';
import 'package:realand_shell/theme/mac_shell_theme.dart';
import 'package:realand_shell/ui/mac_shell_view.dart';
import 'package:realand_shell/ui/wallpaper_panel.dart';

void main() {
  late Directory root;
  late ShellWallpaper image;
  late _Wallpapers service;
  late _Controller controller;

  setUp(() {
    root = Directory.systemTemp.createTempSync('realand-wallpaper-ui-');
    final file = File('${root.path}/ready.png')
      ..writeAsBytesSync(
        base64Decode(
          'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=',
        ),
      );
    image = ShellWallpaper(
      path: file.path,
      name: 'ready.png',
      sizeBytes: file.lengthSync(),
      modified: file.lastModifiedSync(),
    );
  });
  tearDown(() async {
    controller.dispose();
    await root.delete(recursive: true);
  });

  void createController() {
    // Create controlled Futures inside testWidgets' fakeAsync zone. Completers
    // made in setUp belong to the real zone and can strand fake continuations
    // after runAsync returns, before the next pump gets a chance to run.
    service = _Wallpapers(root.path);
    controller = _Controller(service)..selectPanel(ShellPanel.wallpapers);
  }

  Future<void> loadThumbnails(WidgetTester tester) async {
    final thumbnails = find.descendant(
      of: find.byType(WallpaperPanel),
      matching: find.byType(Image),
    );
    for (final element in thumbnails.evaluate()) {
      await precacheImage((element.widget as Image).image, element);
    }
  }

  Future<void> mount(WidgetTester tester) async {
    tester.view.physicalSize = const Size(1280, 900);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.runAsync(() async {
      await tester.pumpWidget(
        MaterialApp(
          theme: buildMacShellTheme(Brightness.light),
          home: MacShellView(controller: controller),
        ),
      );
      await tester.pump();
      await loadThumbnails(tester);
    });
    await tester.pump();
  }

  testWidgets('macOS gallery displays a delayed catalogue without reopening', (
    tester,
  ) async {
    createController();
    final loading = controller.refreshWallpapers();
    await mount(tester);
    final panel = tester.widget<WallpaperPanel>(find.byType(WallpaperPanel));
    expect(find.byType(LinearProgressIndicator), findsOneWidget);
    expect(find.text(image.name), findsNothing);
    service.catalogue.complete([image]);
    await loading;
    await tester.runAsync(() async {
      await tester.pump();
      await loadThumbnails(tester);
    });
    await tester.pump();
    expect(find.text(image.name), findsOneWidget);
    expect(find.byType(LinearProgressIndicator), findsNothing);
    // The outer shell's selected fields have not changed. The existing panel
    // must own this subscription rather than relying on an unrelated rebuild.
    expect(
      tester.widget<WallpaperPanel>(find.byType(WallpaperPanel)),
      same(panel),
    );
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'macOS gallery updates apply busy, selection and errors locally',
    (tester) async {
      createController();
      service.catalogue.complete([image]);
      await controller.refreshWallpapers();
      await mount(tester);
      final applying = controller.applyWallpaper(image);
      await tester.pump();
      expect(find.byType(LinearProgressIndicator), findsOneWidget);
      final tile = find.byKey(ValueKey('wallpaper-${image.path}'));
      expect(tester.widget<InkWell>(tile).onTap, isNull);
      service.application.complete();
      await applying;
      await tester.pump();
      expect(find.byType(LinearProgressIndicator), findsNothing);
      expect(find.byIcon(Icons.check_rounded), findsOneWidget);
      expect(tester.widget<InkWell>(tile).onTap, isNotNull);

      service.application = Completer<void>();
      final failing = controller.applyWallpaper(image);
      service.application.completeError(StateError('fixture decode failed'));
      await failing;
      await tester.pump();
      expect(find.textContaining('fixture decode failed'), findsOneWidget);
      expect(find.byIcon(Icons.check_rounded), findsOneWidget);
      expect(find.byType(LinearProgressIndicator), findsNothing);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('unrelated controller frames retain the same thumbnail widgets', (
    tester,
  ) async {
    createController();
    service.catalogue.complete([image]);
    await controller.refreshWallpapers();
    await mount(tester);
    final thumbnailFinder = find.descendant(
      of: find.byType(WallpaperPanel),
      matching: find.byType(Image),
    );
    final thumbnail = tester.widget<Image>(thumbnailFinder);
    for (var frame = 0; frame < 30; frame++) {
      controller.unrelatedChange();
      await tester.pump();
      expect(tester.widget<Image>(thumbnailFinder), same(thumbnail));
    }
    expect(tester.takeException(), isNull);
  });
}

class _Controller extends ShellController {
  _Controller(WallpaperService service)
    : super(wallpaperService: service, notifications: false, tray: false);
  @override
  bool get reduceMotion => true;
  void unrelatedChange() => notifyListeners();
}

class _Wallpapers extends WallpaperService {
  _Wallpapers(String directory)
    : super(directoryPath: directory, backendExecutable: '/fixture/missing');
  final catalogue = Completer<List<ShellWallpaper>>();
  Completer<void> application = Completer<void>();
  @override
  bool get available => true;
  @override
  Future<List<ShellWallpaper>> scan() => catalogue.future;
  @override
  Future<void> apply(ShellWallpaper image, {bool persist = true}) async {
    await application.future;
    selectedPath = image.path;
  }
}
