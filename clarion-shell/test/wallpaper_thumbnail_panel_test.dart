// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/services/wallpaper_thumbnail_service.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/ui/wallpaper_panel.dart';

const _black =
    'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=';
const _white =
    'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII=';

void main() {
  late Directory root;
  late File original;
  late File preview;
  late ShellWallpaper entry;
  setUp(() {
    root = Directory.systemTemp.createTempSync('clarion-thumbnail-panel-');
    original = File('${root.path}/original.png')
      ..writeAsBytesSync(base64Decode(_black));
    preview = File('${root.path}/preview.png')
      ..writeAsBytesSync(base64Decode(_white));
    entry = ShellWallpaper(
      path: original.path,
      name: 'original.png',
      sizeBytes: original.lengthSync(),
      modified: original.lastModifiedSync(),
    );
  });
  tearDown(() => root.delete(recursive: true));

  Future<void> mount(
    WidgetTester tester,
    _Controller controller,
    _Thumbnails thumbnails,
  ) => tester.pumpWidget(
    MaterialApp(
      home: Scaffold(
        body: WallpaperPanel(controller: controller, thumbnails: thumbnails),
      ),
    ),
  );

  testWidgets(
    'cached pixels display while applying the original wallpaper identity',
    (tester) async {
      await tester.runAsync(() async {
        final controller = _Controller([entry]);
        final thumbnails = _Thumbnails();
        addTearDown(controller.dispose);
        addTearDown(thumbnails.dispose);
        await mount(tester, controller, thumbnails);
        expect(find.byType(Image), findsNothing);
        thumbnails.requests.single.done.complete(
          WallpaperThumbnail.preview(preview),
        );
        await Future<void>.delayed(Duration.zero);
        await tester.pump();
        expect(await _pixels(tester), [255, 255, 255, 255]);
        await tester.tap(find.byKey(ValueKey('wallpaper-${entry.path}')));
        expect(controller.applied, [original.path]);
        expect(original.readAsBytesSync(), base64Decode(_black));
        await tester.pumpWidget(const SizedBox.shrink());
      });
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'a late old thumbnail cannot replace the new revision of a tile',
    (tester) async {
      await tester.runAsync(() async {
        final controller = _Controller([entry]);
        final thumbnails = _Thumbnails();
        addTearDown(controller.dispose);
        addTearDown(thumbnails.dispose);
        await mount(tester, controller, thumbnails);
        final old = thumbnails.requests.single;
        controller.replace(
          ShellWallpaper(
            path: entry.path,
            name: entry.name,
            sizeBytes: entry.sizeBytes,
            modified: entry.modified!.add(const Duration(seconds: 3)),
          ),
        );
        await tester.pump();
        expect(thumbnails.requests.length, 2);
        expect(old.stillWanted!(), false);
        thumbnails.requests.last.done.complete(
          WallpaperThumbnail.preview(preview),
        );
        await Future<void>.delayed(Duration.zero);
        await tester.pump();
        expect(await _pixels(tester), [255, 255, 255, 255]);
        old.done.complete(WallpaperThumbnail.preview(original));
        await Future<void>.delayed(Duration.zero);
        await tester.pump();
        expect(await _pixels(tester), [255, 255, 255, 255]);
        await tester.pumpWidget(const SizedBox.shrink());
      });
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'a broken cached PNG falls back once without disabling a valid original',
    (tester) async {
      await tester.runAsync(() async {
        final controller = _Controller([entry]);
        final thumbnails = _Thumbnails();
        addTearDown(controller.dispose);
        addTearDown(thumbnails.dispose);
        await preview.writeAsString('corrupt cache');
        await mount(tester, controller, thumbnails);
        thumbnails.requests.single.done.complete(
          WallpaperThumbnail.preview(preview),
        );
        await Future<void>.delayed(Duration.zero);
        await tester.pump();
        expect(await _pixels(tester), isNull);
        // errorBuilder queues the transition until the end of the frame.
        await tester.pump();
        await tester.pump();
        expect(await _pixels(tester), [0, 0, 0, 255]);
        expect(thumbnails.failures, [false]);
        expect(find.text('图片无法预览'), findsNothing);
        await tester.tap(find.byKey(ValueKey('wallpaper-${entry.path}')));
        expect(controller.applied, [original.path]);
        await tester.pumpWidget(const SizedBox.shrink());
      });
      expect(tester.takeException(), isNull);
    },
  );
}

Future<List<int>?> _pixels(WidgetTester tester) async {
  final finder = find.byType(Image);
  final stream = tester
      .widget<Image>(finder)
      .image
      .resolve(createLocalImageConfiguration(tester.element(finder)));
  final ready = Completer<ImageInfo?>();
  final listener = ImageStreamListener(
    (info, _) {
      if (!ready.isCompleted) ready.complete(info.clone());
    },
    onError: (Object _, StackTrace? _) {
      if (!ready.isCompleted) ready.complete(null);
    },
  );
  stream.addListener(listener);
  ImageInfo? info;
  try {
    info = await ready.future.timeout(const Duration(seconds: 2));
    final pixels = await info?.image.toByteData(
      format: ui.ImageByteFormat.rawRgba,
    );
    return pixels?.buffer.asUint8List().take(4).toList();
  } finally {
    stream.removeListener(listener);
    info?.dispose();
  }
}

class _Controller extends ShellController {
  _Controller(this.entries) : super(demo: true);
  List<ShellWallpaper> entries;
  final applied = <String>[];
  @override
  List<ShellWallpaper> get wallpapers => entries;
  @override
  Future<void> applyWallpaper(ShellWallpaper image) async =>
      applied.add(image.path);
  void replace(ShellWallpaper image) {
    entries = [image];
    notifyListeners();
  }
}

class _Thumbnails extends WallpaperThumbnailService {
  _Thumbnails() : super(enabled: true);
  final requests =
      <({Completer<WallpaperThumbnail> done, bool Function()? stillWanted})>[];
  final failures = <bool>[];
  @override
  Future<WallpaperThumbnail> resolve(
    ShellWallpaper image, {
    bool Function()? stillWanted,
  }) {
    final done = Completer<WallpaperThumbnail>();
    requests.add((done: done, stillWanted: stillWanted));
    return done.future;
  }

  @override
  Future<void> reportFailure(
    WallpaperThumbnail result, {
    required bool original,
  }) async {
    failures.add(original);
  }
}
