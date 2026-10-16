// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/ui/tray_icon.dart';

const _channel = MethodChannel('clarion.shell/window');
const _pixelPng =
    'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=';

Future<void> _paintGlyph(WidgetTester tester, TrayItem item) =>
    tester.pumpWidget(
      MaterialApp(
        home: Center(
          child: TrayGlyph(key: const ValueKey('stable-glyph'), item: item),
        ),
      ),
    );

FutureBuilder<String?> _iconResolution(WidgetTester tester) =>
    tester.widget<FutureBuilder<String?>>(
      find.byWidgetPredicate((widget) => widget is FutureBuilder<String?>),
    );

Future<void> _waitForImageState(
  WidgetTester tester,
  bool Function() ready, {
  required String reason,
}) async {
  final elapsed = Stopwatch()..start();
  // File IO and engine decoding each return to Flutter on a separate turn.
  // Alternate real asynchronous progress with pumps until the visual state is
  // ready, rather than assuming the whole pipeline finishes in one delay.
  for (var turn = 0; turn < 200; turn++) {
    await tester.pump();
    if (ready()) return;
    if (elapsed.elapsed >= const Duration(seconds: 3)) break;
    await tester.runAsync(() async {
      await Future<void>.delayed(const Duration(milliseconds: 10));
    });
  }
  await tester.pump();
  expect(ready(), isTrue, reason: reason);
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  testWidgets(
    'successful tray icons survive title status and menu refreshes without a fallback frame',
    (tester) async {
      final fixtures = Directory.systemTemp.createTempSync(
        'clarion-bar-continuity-',
      );
      addTearDown(() => fixtures.deleteSync(recursive: true));
      final icon = File('${fixtures.path}/icon.png')
        ..writeAsBytesSync(base64Decode(_pixelPng));
      var calls = 0;
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(_channel, (call) async {
            if (call.method != 'appIcon') return null;
            calls++;
            return icon.path;
          });
      addTearDown(
        () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
            .setMockMethodCallHandler(_channel, null),
      );

      const name = 'bar-continuity-successful-icon';
      await _paintGlyph(
        tester,
        const TrayItem(id: 'app', title: '正在下载', iconName: name),
      );
      await tester.pumpAndSettle();
      expect(find.byType(Image), findsOneWidget);
      final initial = _iconResolution(tester).future;

      for (final item in const [
        TrayItem(id: 'app', title: '下载完成', iconName: name),
        TrayItem(
          id: 'app',
          title: '下载完成',
          iconName: name,
          status: TrayStatus.needsAttention,
        ),
        TrayItem(
          id: 'app',
          title: '下载完成',
          iconName: name,
          menuAvailable: true,
          menuPath: '/UpdatedMenu',
          primaryOpensMenu: true,
        ),
      ]) {
        // Check the first frame rather than allowing a replacement future to
        // finish: one fallback frame is enough to create a visible flash.
        await _paintGlyph(tester, item);
        expect(_iconResolution(tester).future, same(initial));
        expect(find.byType(Image), findsOneWidget);
        expect(find.byIcon(Icons.widgets_outlined), findsNothing);
      }
      expect(calls, 1);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('tray metadata changes preserve an in-flight icon resolution', (
    tester,
  ) async {
    final pending = Completer<String?>();
    var calls = 0;
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(_channel, (call) async {
          if (call.method != 'appIcon') return null;
          calls++;
          return pending.future;
        });
    addTearDown(
      () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(_channel, null),
    );
    const name = 'bar-continuity-pending-icon';
    await _paintGlyph(
      tester,
      const TrayItem(id: 'app', title: '初始标题', iconName: name),
    );
    final initial = _iconResolution(tester).future;
    await _paintGlyph(
      tester,
      const TrayItem(
        id: 'app',
        title: '更新标题',
        iconName: name,
        status: TrayStatus.passive,
      ),
    );
    expect(_iconResolution(tester).future, same(initial));
    expect(calls, 1);
    pending.complete(null);
    await tester.pumpAndSettle();
    expect(find.byIcon(Icons.widgets_outlined), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  for (final broken in [false, true]) {
    testWidgets(
      '${broken ? 'undecodable' : 'missing'} absolute tray icons recover after an exporter update',
      (tester) async {
        final fixtures = Directory.systemTemp.createTempSync(
          'clarion-bar-icon-recovery-',
        );
        addTearDown(() => fixtures.deleteSync(recursive: true));
        final icon = File('${fixtures.path}/icon.png');
        if (broken) icon.writeAsStringSync('Not a PNG image.');

        await _paintGlyph(
          tester,
          TrayItem(id: 'app', title: '准备图标', iconName: icon.path),
        );
        await _waitForImageState(
          tester,
          () =>
              find.byType(Image).evaluate().length == 1 &&
              find.byIcon(Icons.widgets_outlined).evaluate().length == 1,
          reason: 'The initial image load must finish with the error fallback.',
        );
        expect(find.byIcon(Icons.widgets_outlined), findsOneWidget);
        final failedResolution = _iconResolution(tester).future;

        icon.writeAsBytesSync(base64Decode(_pixelPng));
        await _paintGlyph(
          tester,
          TrayItem(id: 'app', title: '图标已准备好', iconName: icon.path),
        );
        expect(_iconResolution(tester).future, isNot(same(failedResolution)));
        await _waitForImageState(
          tester,
          () => tester
              .widgetList<RawImage>(find.byType(RawImage))
              .any((image) => image.image != null),
          reason: 'The repaired image must finish decoding after the retry.',
        );
        expect(find.byIcon(Icons.widgets_outlined), findsNothing);
        expect(find.byType(RawImage), findsOneWidget);
        expect(tester.widget<RawImage>(find.byType(RawImage)).image, isNotNull);
        expect(tester.takeException(), isNull);
        await tester.pumpWidget(const SizedBox.shrink());
      },
    );
  }
}
