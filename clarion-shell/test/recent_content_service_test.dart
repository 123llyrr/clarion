// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:clarion_shell/recent_content_models.dart';
import 'package:clarion_shell/services/recent_content_service.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

// A complete 1 × 1 RGBA PNG. Ordinary service tests inject a thumbnail and
// digest so none of them needs an image decoder, platform clipboard or isolate.
final _png = base64Decode(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==',
);

Future<String> _digest(Uint8List bytes) async => base64Encode(bytes);

class _Publication {
  _Publication(this.mime, Uint8List bytes, this.isScreenshot)
    : bytes = Uint8List.fromList(bytes);
  final String mime;
  final Uint8List bytes;
  final bool isScreenshot;
}

class _Clipboard implements ClipboardTransport {
  final _events = StreamController<Object?>.broadcast(sync: true);
  final publications = <_Publication>[];
  final pauses = <bool>[];
  bool startResult = true;
  bool reconnectResult = true;
  bool publishResult = true;
  int starts = 0;
  int reconnects = 0;
  int stops = 0;
  int sequence = 0;
  Completer<bool>? publishing;

  @override
  Stream<Object?> get events => _events.stream;
  @override
  Future<bool> start() async {
    starts++;
    return startResult;
  }

  @override
  Future<bool> reconnect() async {
    reconnects++;
    return reconnectResult;
  }

  @override
  Future<void> setPaused(bool paused) async => pauses.add(paused);
  @override
  Future<bool> publish(
    String mime,
    Uint8List data, {
    bool isScreenshot = false,
  }) async {
    publications.add(_Publication(mime, data, isScreenshot));
    return publishing == null ? publishResult : publishing!.future;
  }

  @override
  Future<void> stop() async {
    stops++;
  }

  void emit(Object? event) => _events.add(event);
  void text(String text, {bool sensitive = false}) => selection(
    'text/plain',
    Uint8List.fromList(utf8.encode(text)),
    sensitive: sensitive,
  );
  void selection(
    String mime,
    Uint8List data, {
    bool screenshot = false,
    bool sensitive = false,
  }) => emit({
    'type': 'selection',
    'generation': ++sequence,
    'mime': mime,
    'data': data,
    'isScreenshot': screenshot,
    'sensitive': sensitive,
  });
  Future<void> close() => _events.close();
}

class _Fixture {
  _Fixture({
    bool enabled = true,
    int entryLimit = 80,
    int byteLimit = 64 * 1024 * 1024,
    int textLimit = 1024 * 1024,
    int imageLimit = 16 * 1024 * 1024,
    Future<String> Function(Uint8List)? digest,
    ThumbnailBuilder? thumbnail,
  }) {
    var tick = 0;
    service = RecentContentService(
      transport: clipboard,
      enabled: enabled,
      entryLimit: entryLimit,
      byteLimit: byteLimit,
      textLimit: textLimit,
      imageLimit: imageLimit,
      digest: digest ?? _digest,
      buildThumbnail: thumbnail ?? (_) async => Uint8List.fromList(_png),
      now: () => DateTime.utc(2026, 10, 18).add(Duration(seconds: tick++)),
    );
    addTearDown(() async {
      service.dispose();
      await clipboard.close();
    });
  }
  final clipboard = _Clipboard();
  late final RecentContentService service;

  Future<void> start() => service.start();
  Future<void> text(String text) async {
    clipboard.text(text);
    await service.settled;
  }
}

Future<Directory> _directory() async {
  final directory = await Directory.systemTemp.createTemp(
    'clarion-history-test-',
  );
  addTearDown(() => directory.delete(recursive: true));
  return directory;
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  test(
    'ingests exact UTF-8 text and PNG through the injected transport',
    () async {
      final fixture = _Fixture();
      await fixture.start();
      await fixture.text('中文 👋\nsecond line');
      fixture.clipboard.selection('image/png', _png);
      await fixture.service.settled;
      expect(fixture.service.available, isTrue);
      expect(fixture.service.entries, hasLength(2));
      final text = fixture.service.entries.singleWhere(
        (entry) => entry.kind == RecentContentKind.text,
      );
      final image = fixture.service.entries.singleWhere(
        (entry) => entry.kind == RecentContentKind.image,
      );
      expect(text.text, '中文 👋\nsecond line');
      expect(text.source, '剪贴板');
      expect(image.thumbnail, orderedEquals(_png));
      expect(image.filePath, isNull);
      expect(
        fixture.service.retainedBytes,
        utf8.encode(text.text).length + _png.length,
      );
    },
  );

  test(
    'deduplicates payloads while refreshing time and preserving pins',
    () async {
      final fixture = _Fixture();
      await fixture.start();
      await fixture.text('pinned');
      final original = fixture.service.entries.single;
      fixture.service.togglePinned(original);
      await fixture.text('other');
      await fixture.text('pinned');
      expect(fixture.service.entries, hasLength(2));
      final refreshed = fixture.service.entries.first;
      expect(refreshed.id, original.id);
      expect(refreshed.isPinned, isTrue);
      expect(refreshed.capturedAt.isAfter(original.capturedAt), isTrue);
      expect(fixture.service.retainedBytes, utf8.encode('pinnedother').length);
    },
  );

  test(
    'entry eviction preserves pins and reports a history filled by pins',
    () async {
      final fixture = _Fixture(entryLimit: 2);
      await fixture.start();
      await fixture.text('first');
      fixture.service.togglePinned(fixture.service.entries.single);
      await fixture.text('second');
      await fixture.text('third');
      expect(fixture.service.entries.map((entry) => entry.text), [
        'first',
        'third',
      ]);
      fixture.service.togglePinned(
        fixture.service.entries.singleWhere((entry) => entry.text == 'third'),
      );
      await fixture.text('fourth');
      expect(
        fixture.service.entries.map((entry) => entry.text),
        containsAll(['first', 'third']),
      );
      expect(fixture.service.entries, hasLength(2));
      expect(fixture.service.error, contains('置顶'));
      expect(fixture.service.retainedBytes, utf8.encode('firstthird').length);
    },
  );

  test(
    'byte eviction removes only unpinned payloads and accounts exactly',
    () async {
      final fixture = _Fixture(byteLimit: 8);
      await fixture.start();
      await fixture.text('aaaaa');
      fixture.service.togglePinned(fixture.service.entries.single);
      await fixture.text('bb');
      await fixture.text('ccc');
      expect(fixture.service.entries.map((entry) => entry.text), [
        'aaaaa',
        'ccc',
      ]);
      expect(fixture.service.retainedBytes, 8);
      fixture.service.delete(
        fixture.service.entries.singleWhere((entry) => entry.text == 'ccc'),
      );
      expect(fixture.service.retainedBytes, 5);
      fixture.service.clear(includePinned: true);
      expect(fixture.service.retainedBytes, 0);
    },
  );

  test(
    'clear retains existing pins and rejects queued and hashing work',
    () async {
      final entered = Completer<void>();
      final hashing = Completer<String>();
      final fixture = _Fixture(
        digest: (bytes) {
          if (utf8.decode(bytes) == 'in flight') {
            entered.complete();
            return hashing.future;
          }
          return _digest(bytes);
        },
      );
      await fixture.start();
      await fixture.text('keep');
      fixture.service.togglePinned(fixture.service.entries.single);
      await fixture.text('remove');
      fixture.clipboard.text('in flight');
      await entered.future;
      fixture.clipboard.text('queued');
      fixture.service.clear();
      expect(fixture.service.entries.single.text, 'keep');
      hashing.complete('discard-this-generation');
      await fixture.service.settled;
      expect(fixture.service.entries.single.text, 'keep');
      fixture.service.clear(includePinned: true);
      expect(fixture.service.entries, isEmpty);
    },
  );

  test(
    'pause rejects in-flight work and resumes only later selections',
    () async {
      final entered = Completer<void>();
      final hashing = Completer<String>();
      final fixture = _Fixture(
        digest: (bytes) {
          if (utf8.decode(bytes) == 'in flight') {
            entered.complete();
            return hashing.future;
          }
          return _digest(bytes);
        },
      );
      await fixture.start();
      fixture.clipboard.text('in flight');
      await entered.future;
      await fixture.service.setPaused(true);
      fixture.clipboard.text('while paused');
      hashing.complete('discard-paused');
      await fixture.service.settled;
      expect(fixture.service.entries, isEmpty);
      expect(fixture.service.paused, isTrue);
      expect(fixture.clipboard.pauses, [true]);
      await fixture.service.setPaused(false);
      await fixture.text('after resume');
      expect(fixture.service.entries.single.text, 'after resume');
      expect(fixture.clipboard.pauses, [true, false]);
    },
  );

  test(
    'thumbnail work completing after clear cannot resurrect an image',
    () async {
      final entered = Completer<void>();
      final thumbnail = Completer<Uint8List?>();
      final fixture = _Fixture(
        thumbnail: (_) {
          entered.complete();
          return thumbnail.future;
        },
      );
      await fixture.start();
      fixture.clipboard.selection('image/png', _png);
      await entered.future;
      fixture.service.clear(includePinned: true);
      thumbnail.complete(Uint8List.fromList(_png));
      await fixture.service.settled;
      expect(fixture.service.entries, isEmpty);
      expect(fixture.service.retainedBytes, 0);
    },
  );

  test(
    'only the screenshot marker classifies a clipboard PNG as a screenshot',
    () async {
      final fixture = _Fixture();
      await fixture.start();
      fixture.clipboard.selection('image/png', _png);
      await fixture.service.settled;
      expect(fixture.service.entries.single.kind, RecentContentKind.image);
      final original = fixture.service.entries.single;
      fixture.service.togglePinned(original);
      fixture.clipboard.selection('image/png', _png, screenshot: true);
      await fixture.service.settled;
      final screenshot = fixture.service.entries.single;
      expect(screenshot.id, original.id);
      expect(screenshot.kind, RecentContentKind.screenshot);
      expect(screenshot.source, '截图');
      expect(screenshot.isPinned, isTrue);
      fixture.clipboard.selection('image/png', _png);
      await fixture.service.settled;
      expect(fixture.service.entries.single.kind, RecentContentKind.screenshot);
      expect(fixture.service.retainedBytes, _png.length);
    },
  );

  test(
    'file completion upgrades the same PNG and deletion never deletes its file',
    () async {
      final directory = await _directory();
      final file = File('${directory.path}/screen.png');
      await file.writeAsBytes(_png);
      final fixture = _Fixture();
      await fixture.start();
      fixture.clipboard.selection('image/png', _png);
      await fixture.service.settled;
      final original = fixture.service.entries.single;
      fixture.service.togglePinned(original);
      fixture.service.recordScreenshot(file.path);
      await fixture.service.settled;
      final screenshot = fixture.service.entries.single;
      expect(screenshot.id, original.id);
      expect(screenshot.kind, RecentContentKind.screenshot);
      expect(screenshot.filePath, file.path);
      expect(screenshot.isPinned, isTrue);
      fixture.service.delete(screenshot);
      expect(fixture.service.entries, isEmpty);
      expect(await file.readAsBytes(), orderedEquals(_png));
      fixture.service.recordScreenshot(file.path);
      await fixture.service.settled;
      fixture.service.clear(includePinned: true);
      expect(fixture.service.entries, isEmpty);
      expect(await file.readAsBytes(), orderedEquals(_png));
    },
  );

  test(
    'deleting a screenshot while its same-SHA file upgrade hashes wins over the old work',
    () async {
      final directory = await _directory();
      final file = File('${directory.path}/upgrade.png');
      await file.writeAsBytes(_png);
      final entered = Completer<void>();
      final hashing = Completer<String>();
      var hashes = 0;
      final fixture = _Fixture(
        digest: (bytes) {
          if (++hashes == 2) {
            entered.complete();
            return hashing.future;
          }
          return _digest(bytes);
        },
      );
      await fixture.start();
      fixture.clipboard.selection('image/png', _png);
      await fixture.service.settled;
      final original = fixture.service.entries.single;
      fixture.service.recordScreenshot(file.path);
      await entered.future;
      fixture.service.delete(original);
      hashing.complete(await _digest(_png));
      await fixture.service.settled;
      expect(fixture.service.entries, isEmpty);
      expect(fixture.service.retainedBytes, 0);
      expect(await file.readAsBytes(), orderedEquals(_png));

      // A new user copy after deletion is a new event and must remain recordable.
      fixture.clipboard.selection('image/png', _png);
      await fixture.service.settled;
      expect(fixture.service.entries.single.id, original.id);
      expect(fixture.service.entries.single.kind, RecentContentKind.image);
      expect(fixture.service.entries.single.filePath, isNull);
    },
  );

  test(
    'deleting a stale entry handle also cancels its pending file thumbnail',
    () async {
      final directory = await _directory();
      final file = File('${directory.path}/thumbnail.png');
      await file.writeAsBytes(_png);
      final entered = Completer<void>();
      final thumbnail = Completer<Uint8List?>();
      var thumbnails = 0;
      final fixture = _Fixture(
        thumbnail: (_) {
          if (++thumbnails == 2) {
            entered.complete();
            return thumbnail.future;
          }
          return Future.value(Uint8List.fromList(_png));
        },
      );
      await fixture.start();
      fixture.clipboard.selection('image/png', _png);
      await fixture.service.settled;
      final original = fixture.service.entries.single;
      fixture.service.delete(original);
      fixture.service.recordScreenshot(file.path);
      await entered.future;
      // A callback may still hold a prior immutable entry after list removal.
      fixture.service.delete(original);
      thumbnail.complete(Uint8List.fromList(_png));
      await fixture.service.settled;
      expect(fixture.service.entries, isEmpty);
      expect(fixture.service.retainedBytes, 0);
      expect(await file.readAsBytes(), orderedEquals(_png));
      fixture.clipboard.selection('image/png', _png);
      await fixture.service.settled;
      expect(fixture.service.entries.single.id, original.id);
    },
  );

  test(
    'preview, paused and invalid screenshot paths never enter history',
    () async {
      final directory = await _directory();
      final file = File('${directory.path}/preview.png');
      await file.writeAsBytes(_png);
      final fixture = _Fixture();
      await fixture.start();
      fixture.service.recordScreenshot(file.path, isPreview: true);
      fixture.service.recordScreenshot(null);
      fixture.service.recordScreenshot('relative.png');
      fixture.service.recordScreenshot('${file.path}\u0000');
      await fixture.service.setPaused(true);
      fixture.service.recordScreenshot(file.path);
      await fixture.service.settled;
      expect(fixture.service.entries, isEmpty);
      expect(await file.readAsBytes(), orderedEquals(_png));
    },
  );

  test('clear during screenshot hashing preserves the original file', () async {
    final directory = await _directory();
    final file = File('${directory.path}/pending.png');
    await file.writeAsBytes(_png);
    final entered = Completer<void>();
    final hashing = Completer<String>();
    final fixture = _Fixture(
      digest: (_) {
        entered.complete();
        return hashing.future;
      },
    );
    await fixture.start();
    fixture.service.recordScreenshot(file.path);
    await entered.future;
    fixture.service.clear(includePinned: true);
    hashing.complete('old-screenshot');
    await fixture.service.settled;
    expect(fixture.service.entries, isEmpty);
    expect(await file.readAsBytes(), orderedEquals(_png));
  });

  test(
    'copy republishes exact payloads with their marker and sends no platform paste',
    () async {
      final platformCalls = <String>[];
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(SystemChannels.platform, (call) async {
            platformCalls.add(call.method);
            return null;
          });
      addTearDown(() {
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
            .setMockMethodCallHandler(SystemChannels.platform, null);
      });
      final fixture = _Fixture();
      await fixture.start();
      const text = '真实内容 🌱\nnext';
      await fixture.text(text);
      final textEntry = fixture.service.entries.single;
      expect(await fixture.service.copy(textEntry), isTrue);
      expect(fixture.clipboard.publications.single.mime, 'text/plain');
      expect(
        fixture.clipboard.publications.single.bytes,
        orderedEquals(utf8.encode(text)),
      );
      expect(fixture.clipboard.publications.single.isScreenshot, isFalse);
      fixture.clipboard.selection('image/png', _png, screenshot: true);
      await fixture.service.settled;
      final screenshot = fixture.service.entries.singleWhere(
        (entry) => entry.kind == RecentContentKind.screenshot,
      );
      expect(await fixture.service.copy(screenshot), isTrue);
      final image = fixture.clipboard.publications.last;
      expect(image.mime, 'image/png');
      expect(image.bytes, orderedEquals(_png));
      expect(image.isScreenshot, isTrue);
      expect(fixture.service.entries, hasLength(2));
      expect(platformCalls, isEmpty);
    },
  );

  test(
    'copy has one in-flight publication and preserves content after failure',
    () async {
      final fixture = _Fixture();
      await fixture.start();
      await fixture.text('copy me');
      final entry = fixture.service.entries.single;
      fixture.clipboard.publishing = Completer<bool>();
      final copying = fixture.service.copy(entry);
      expect(fixture.service.busyEntryIds, contains(entry.id));
      expect(await fixture.service.copy(entry), isFalse);
      expect(fixture.clipboard.publications, hasLength(1));
      fixture.clipboard.publishing!.complete(false);
      expect(await copying, isFalse);
      expect(fixture.service.busyEntryIds, isEmpty);
      expect(fixture.service.error, contains('复制未完成'));
      expect(fixture.service.entries.single.text, 'copy me');
      fixture.clipboard.publishing = null;
      expect(await fixture.service.copy(entry), isTrue);
      expect(fixture.service.error, isNull);
    },
  );

  test(
    'PRIMARY, sensitive, malformed and stale selections are excluded',
    () async {
      final fixture = _Fixture(textLimit: 16, imageLimit: _png.length);
      await fixture.start();
      for (final type in [
        'primary',
        'primary-selection',
        'primary_selection',
      ]) {
        fixture.clipboard.emit({
          'type': type,
          'mime': 'text/plain',
          'data': Uint8List.fromList(utf8.encode('PRIMARY')),
        });
      }
      fixture.clipboard.text('password', sensitive: true);
      fixture.clipboard.selection(
        'text/plain',
        Uint8List.fromList([0xff, 0xfe]),
      );
      fixture.clipboard.selection(
        'text/plain',
        Uint8List.fromList(List.filled(17, 65)),
      );
      fixture.clipboard.selection('image/png', Uint8List.fromList([1, 2, 3]));
      fixture.clipboard.selection(
        'image/png',
        Uint8List.fromList([..._png, 0]),
      );
      fixture.clipboard.selection(
        'application/octet-stream',
        Uint8List.fromList([1]),
      );
      fixture.clipboard.emit({
        'type': 'selection',
        'mime': 'text/plain',
        'data': [65],
      });
      await fixture.service.settled;
      expect(fixture.service.entries, isEmpty);
      await fixture.text('valid');
      fixture.clipboard.emit({
        'type': 'selection',
        'generation': fixture.clipboard.sequence - 1,
        'mime': 'text/plain',
        'data': Uint8List.fromList(utf8.encode('stale')),
      });
      await fixture.service.settled;
      expect(fixture.service.entries.single.text, 'valid');
    },
  );

  test(
    'disabled service never starts transport or records screenshot files',
    () async {
      final directory = await _directory();
      final file = File('${directory.path}/disabled.png');
      await file.writeAsBytes(_png);
      final fixture = _Fixture(enabled: false);
      await fixture.start();
      await fixture.service.reconnect();
      fixture.clipboard.text('ignored');
      fixture.service.recordScreenshot(file.path);
      await fixture.service.settled;
      expect(fixture.clipboard.starts, 0);
      expect(fixture.clipboard.reconnects, 0);
      expect(fixture.service.available, isFalse);
      expect(fixture.service.entries, isEmpty);
      expect(await file.readAsBytes(), orderedEquals(_png));
    },
  );

  test(
    'missing backend is explicit and reconnect preserves pinned history',
    () async {
      final fixture = _Fixture();
      fixture.clipboard.startResult = false;
      await fixture.start();
      expect(fixture.service.available, isFalse);
      expect(fixture.service.error, contains('未提供剪贴板历史协议'));
      expect(fixture.service.loading, isFalse);
      expect(fixture.service.entries, isEmpty);
      await fixture.service.reconnect();
      expect(fixture.clipboard.reconnects, 1);
      expect(fixture.service.available, isTrue);
      expect(fixture.service.error, isNull);
      await fixture.text('retain');
      fixture.service.togglePinned(fixture.service.entries.single);
      fixture.clipboard.emit({
        'type': 'status',
        'available': false,
        'error': 'gone',
      });
      expect(fixture.service.available, isFalse);
      await fixture.service.reconnect();
      expect(fixture.service.entries.single.isPinned, isTrue);
      expect(fixture.service.entries.single.text, 'retain');
    },
  );

  testWidgets('real thumbnails fit 320 pixels and do not upscale a tiny PNG', (
    tester,
  ) async {
    await tester.runAsync(() async {
      final recorder = ui.PictureRecorder();
      final canvas = ui.Canvas(recorder);
      canvas.drawRect(
        const ui.Rect.fromLTWH(0, 0, 1280, 640),
        ui.Paint()..color = const ui.Color(0xff4488cc),
      );
      final picture = recorder.endRecording();
      final source = await picture.toImage(1280, 640);
      late final Uint8List encoded;
      try {
        encoded = (await source.toByteData(
          format: ui.ImageByteFormat.png,
        ))!.buffer.asUint8List();
      } finally {
        source.dispose();
        picture.dispose();
      }
      for (final sample in [(encoded, 320, 160), (_png, 1, 1)]) {
        final thumbnail = await RecentContentService.imageThumbnail(sample.$1);
        expect(thumbnail, isNotNull);
        final codec = await ui.instantiateImageCodec(thumbnail!);
        final decoded = (await codec.getNextFrame()).image;
        try {
          expect(decoded.width, sample.$2);
          expect(decoded.height, sample.$3);
        } finally {
          decoded.dispose();
          codec.dispose();
        }
      }
    });
  });
}
