// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:io';
import 'dart:typed_data';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_svg/flutter_svg.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/ui/tray_icon.dart';

const _boundary = ValueKey('tray-svg-pixels');

final class _CountFileReads extends IOOverrides {
  _CountFileReads(this.path);
  final String path;
  var reads = 0;
  @override
  File createFile(String path) {
    final file = super.createFile(path);
    return path == this.path ? _CountedFile(file, () => reads++) : file;
  }
}

class _CountedFile implements File {
  _CountedFile(this.file, this.onRead);
  final File file;
  final void Function() onRead;
  @override
  String get path => file.path;
  @override
  Uint8List readAsBytesSync() {
    onRead();
    return file.readAsBytesSync();
  }

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

String _art(String color) =>
    '<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20">'
    '<rect width="20" height="20" fill="$color"/></svg>';

Future<ByteData> _paint(WidgetTester tester, TrayItem item) async {
  await tester.pumpWidget(
    MaterialApp(
      home: Center(
        child: RepaintBoundary(
          key: _boundary,
          child: TrayGlyph(item: item),
        ),
      ),
    ),
  );
  await tester.pump();
  final picture = tester.widget<SvgPicture>(find.byType(SvgPicture));
  final bytes = await tester.runAsync(
    () =>
        picture.bytesLoader.loadBytes(tester.element(find.byType(SvgPicture))),
  );
  await tester.pump();
  return bytes!;
}

Future<List<int>> _pixel(WidgetTester tester) async {
  final boundary = tester.renderObject<RenderRepaintBoundary>(
    find.byKey(_boundary),
  );
  return (await tester.runAsync(() async {
    final image = await boundary.toImage(pixelRatio: 1);
    try {
      final bytes = await image.toByteData(format: ui.ImageByteFormat.rawRgba);
      final offset = ((image.height ~/ 2) * image.width + image.width ~/ 2) * 4;
      return bytes!.buffer.asUint8List(offset, 4).toList();
    } finally {
      image.dispose();
    }
  }))!;
}

void main() {
  testWidgets('30 metadata updates reuse one decoded SVG and its pixels', (
    tester,
  ) async {
    final dir = Directory.systemTemp.createTempSync('clarion-tray-svg-');
    addTearDown(() => dir.deleteSync(recursive: true));
    final file = File('${dir.path}/status.svg')
      ..writeAsStringSync(_art('#24c878'));
    svg.cache.clear();
    final decoded = <ByteData>{};
    final counter = _CountFileReads(file.path);
    await IOOverrides.runWithIOOverrides(() async {
      for (var update = 0; update <= 30; update++) {
        decoded.add(
          await _paint(
            tester,
            TrayItem(
              id: 'tray',
              title: 'Progress $update',
              iconName: file.path,
              status: update.isEven ? TrayStatus.active : TrayStatus.passive,
              menuAvailable: update.isEven,
            ),
          ),
        );
      }
    }, counter);
    expect(await _pixel(tester), [0x24, 0xc8, 0x78, 0xff]);
    expect(
      decoded,
      hasLength(1),
      reason: 'Metadata changes must not compile the same SVG again.',
    );
    expect(svg.cache.count, 1);
    expect(counter.reads, 1);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  testWidgets(
    'a real icon event reloads rewritten SVG contents at the same path',
    (tester) async {
      final dir = Directory.systemTemp.createTempSync(
        'clarion-tray-svg-rewrite-',
      );
      addTearDown(() => dir.deleteSync(recursive: true));
      final file = File('${dir.path}/status.svg')
        ..writeAsStringSync(_art('#24c878'));
      final counter = _CountFileReads(file.path);
      await IOOverrides.runWithIOOverrides(() async {
        TrayItem item(int revision) => TrayItem(
          id: 'tray',
          title: 'Status',
          iconName: file.path,
          iconRevision: revision,
        );
        final initial = await _paint(tester, item(1));
        expect(await _pixel(tester), [0x24, 0xc8, 0x78, 0xff]);
        file.writeAsStringSync(_art('#e03a26'));
        final updated = await _paint(tester, item(2));
        expect(updated, isNot(same(initial)));
        expect(await _pixel(tester), [0xe0, 0x3a, 0x26, 0xff]);
        expect(await _paint(tester, item(2)), same(updated));
        expect(counter.reads, 2);
      }, counter);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets(
    'switching icon paths displays new artwork without waiting for another event',
    (tester) async {
      final dir = Directory.systemTemp.createTempSync('clarion-tray-svg-path-');
      addTearDown(() => dir.deleteSync(recursive: true));
      final first = File('${dir.path}/first.svg')
        ..writeAsStringSync(_art('#24c878'));
      final second = File('${dir.path}/second.svg')
        ..writeAsStringSync(_art('#e03a26'));
      final old = await _paint(
        tester,
        TrayItem(id: 'tray', title: 'Normal', iconName: first.path),
      );
      final next = await _paint(
        tester,
        TrayItem(id: 'tray', title: 'Attention', iconName: second.path),
      );
      expect(next, isNot(same(old)));
      expect(await _pixel(tester), [0xe0, 0x3a, 0x26, 0xff]);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );
}
