// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:clarion_shell/recent_content_models.dart';
import 'package:clarion_shell/services/recent_content_service.dart';
import 'package:clarion_shell/ui/recent_content_panel.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

const _window = MethodChannel('clarion.shell/window');
const _native = MethodChannel('clarion/clipboard');
final _env = Platform.environment;
final _processes = <Process>[];
var _sequence = 0;
String get _root => _env['CLARION_CLIPBOARD_TEST_ROOT']!;

void _guard() {
  expect(_env['CLARION_CLIPBOARD_ISOLATED_TEST'], '1');
  final runtime = _env['XDG_RUNTIME_DIR']!;
  expect(runtime, startsWith('$_root/'));
  expect(Directory(runtime).statSync().mode & 0x1ff, 0x1c0);
  expect(File('$runtime/clipboard-test-owner').readAsStringSync(), _root);
  expect(_env['WAYLAND_DISPLAY'], startsWith('$runtime/'));
  expect(
    _env['WAYLAND_DISPLAY'],
    isNot(_env['CLARION_CLIPBOARD_PARENT_WAYLAND']),
  );
  expect(_env['WAYLAND_SOCKET'], isNull);
  expect(_env['CLARION_SOCKET'], startsWith('$runtime/'));
  expect(_env['DBUS_SESSION_BUS_ADDRESS'], startsWith('unix:path=$runtime/'));
  for (final variable in [
    'XDG_CONFIG_HOME',
    'XDG_DATA_HOME',
    'XDG_STATE_HOME',
    'XDG_CACHE_HOME',
  ]) {
    expect(_env[variable], startsWith('$_root/'));
  }
  expect(_env['CLARION_CLIPBOARD_PEER'], startsWith('$_root/'));
}

Future<void> _wait(
  WidgetTester tester,
  bool Function() ready,
  String label,
) async {
  final deadline = DateTime.now().add(const Duration(seconds: 12));
  while (!ready()) {
    if (DateTime.now().isAfter(deadline)) fail('Timed out: $label');
    await tester.pump(const Duration(milliseconds: 40));
  }
}

Future<void> _settle(WidgetTester tester, [int millis = 450]) async {
  final deadline = DateTime.now().add(Duration(milliseconds: millis));
  while (DateTime.now().isBefore(deadline)) {
    await tester.pump(const Duration(milliseconds: 40));
  }
}

class _RealTransport implements ClipboardTransport {
  final real = NativeClipboardTransport();
  final observed = <Map<String, dynamic>>[];
  late final Stream<Object?> _stream = real.events.map((raw) {
    if (raw is Map) observed.add(Map<String, dynamic>.from(raw));
    return raw;
  }).asBroadcastStream();
  @override
  Stream<Object?> get events => _stream;
  @override
  Future<bool> start() => real.start();
  @override
  Future<bool> reconnect() => real.reconnect();
  @override
  Future<void> setPaused(bool paused) => real.setPaused(paused);
  @override
  Future<void> stop() => real.stop();
  @override
  Future<bool> publish(
    String mime,
    Uint8List data, {
    bool isScreenshot = false,
  }) => real.publish(mime, data, isScreenshot: isScreenshot);
  int get selectionCount =>
      observed.where((event) => event['type'] == 'selection').length;
}

class _Producer {
  const _Producer(this.requests);
  final File requests;
  List<String> get requested =>
      requests.existsSync() ? requests.readAsLinesSync() : const [];
}

Future<_Producer> _offer(
  WidgetTester tester,
  Uint8List bytes, {
  String mime = 'text/plain;charset=utf-8',
  bool primary = false,
  bool sensitive = false,
  bool screenshot = false,
}) async {
  _guard();
  final id = ++_sequence;
  final payload = File('$_root/offer-$id.bin');
  await payload.writeAsBytes(bytes, flush: true);
  final ready = File('$_root/offer-$id.ready');
  final requests = File('$_root/offer-$id.requests');
  final process = await Process.start(_env['CLARION_CLIPBOARD_PEER']!, [
    'offer',
    '--mime',
    mime,
    '--file',
    payload.path,
    '--ready',
    ready.path,
    '--requests',
    requests.path,
    if (primary) '--primary',
    if (sensitive) '--sensitive',
    if (screenshot) '--screenshot',
  ]);
  _processes.add(process);
  unawaited(process.stdout.drain<void>());
  final errors = StringBuffer();
  process.stderr.transform(utf8.decoder).listen(errors.write);
  await _wait(
    tester,
    ready.existsSync,
    'real Wayland producer ready ($errors)',
  );
  return _Producer(requests);
}

Future<Map<String, dynamic>> _get(Uint8List expected, String mime) async {
  _guard();
  final output = File('$_root/received-${++_sequence}.bin');
  final result = await Process.run(_env['CLARION_CLIPBOARD_PEER']!, [
    'get',
    '--mime',
    mime,
    '--file',
    output.path,
  ]);
  expect(result.exitCode, 0, reason: '${result.stderr}');
  expect(await output.readAsBytes(), orderedEquals(expected));
  return Map<String, dynamic>.from(jsonDecode(result.stdout as String) as Map);
}

Future<Uint8List> _png([ui.Color color = const ui.Color(0xff4285f4)]) async {
  final recorder = ui.PictureRecorder();
  ui.Canvas(recorder).drawColor(color, ui.BlendMode.src);
  final picture = recorder.endRecording();
  final image = await picture.toImage(16, 16);
  try {
    final data = await image.toByteData(format: ui.ImageByteFormat.png);
    return data!.buffer.asUint8List();
  } finally {
    image.dispose();
    picture.dispose();
  }
}

Future<void> _evidence(String name, Map<String, Object?> value) async {
  final directory = Directory(_env['CLARION_CLIPBOARD_ARTIFACTS']!);
  await directory.create(recursive: true);
  await File(
    '${directory.path}/$name',
  ).writeAsString('${const JsonEncoder.withIndent('  ').convert(value)}\n');
}

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  testWidgets('isolated native clipboard and session history', (tester) async {
    _guard();
    await tester.pumpWidget(const MaterialApp(home: SizedBox.shrink()));
    final window = await _window.invokeMapMethod<String, dynamic>(
      'windowState',
    );
    final preview = _env['CLARION_CLIPBOARD_PREVIEW_TEST'] == '1';
    expect(window!['preview'], preview);
    final transport = _RealTransport();
    final service = RecentContentService(transport: transport);
    try {
      if (preview) {
        var disabled = false;
        try {
          disabled = !await transport.start();
        } on MissingPluginException {
          disabled = true;
        }
        expect(disabled, true);
        final bytes = Uint8List.fromList(
          utf8.encode('preview must not read this fixture'),
        );
        final producer = await _offer(tester, bytes);
        await _settle(tester);
        expect(producer.requested, isEmpty);
        expect(transport.selectionCount, 0);
        await expectLater(
          transport.publish('text/plain', bytes),
          throwsA(
            anyOf(isA<PlatformException>(), isA<MissingPluginException>()),
          ),
        );
        await _evidence('preview-evidence.json', {
          'passed': true,
          'preview': true,
          'startDisabled': disabled,
          'producerReceiveRequests': producer.requested.length,
        });
        return;
      }

      await service.start();
      expect(service.available, true, reason: service.error);
      final text = Uint8List.fromList(utf8.encode('Native clipboard α 中文'));
      await _offer(tester, text);
      await _wait(
        tester,
        () => service.entries.length == 1,
        'text enters real history',
      );
      expect(service.entries.single.text, utf8.decode(text));
      final textEntry = service.entries.single;
      final firstGeneration =
          transport.observed.lastWhere(
                (event) => event['type'] == 'selection',
              )['generation']
              as int;
      await _offer(tester, text);
      await _wait(
        tester,
        () => transport.selectionCount >= 2,
        'second real selection',
      );
      await _settle(tester);
      expect(
        service.entries.length,
        1,
        reason: 'service deduplicates payload SHA',
      );
      service.clear(includePinned: true);
      expect(service.entries, isEmpty);
      await _offer(tester, text);
      await _wait(
        tester,
        () => service.entries.length == 1,
        'clear followed by the same content records again',
      );

      final beforeSensitive = transport.selectionCount;
      final sensitive = await _offer(
        tester,
        Uint8List.fromList(utf8.encode('synthetic secret fixture')),
        sensitive: true,
      );
      await _wait(
        tester,
        () => transport.observed.any((event) => event['reason'] == 'sensitive'),
        'sensitive MIME skipped',
      );
      await _settle(tester);
      expect(
        sensitive.requested,
        isEmpty,
        reason: 'sensitive content must not even be received',
      );
      expect(transport.selectionCount, beforeSensitive);
      expect(service.entries.single.text, utf8.decode(text));

      final generationBeforePrimary = (await _native
          .invokeMapMethod<String, dynamic>('status'))!['generation'];
      final primary = await _offer(
        tester,
        Uint8List.fromList(utf8.encode('PRIMARY fixture only')),
        primary: true,
      );
      await _settle(tester);
      expect(primary.requested, isEmpty);
      expect(transport.selectionCount, beforeSensitive);
      expect(
        (await _native.invokeMapMethod<String, dynamic>(
          'status',
        ))!['generation'],
        generationBeforePrimary,
      );

      await _offer(
        tester,
        Uint8List(1024 * 1024 + 1)..fillRange(0, 1024 * 1024 + 1, 65),
      );
      await _wait(
        tester,
        () => transport.observed.any((event) => event['reason'] == 'too-large'),
        'oversize text is rejected',
      );
      expect(service.entries.length, 1);

      await service.setPaused(true);
      final beforePaused = transport.selectionCount;
      await _offer(tester, Uint8List.fromList(utf8.encode('paused fixture')));
      await _settle(tester);
      expect(transport.selectionCount, beforePaused);
      await service.setPaused(false);
      await _settle(tester);
      expect(
        transport.selectionCount,
        beforePaused,
        reason: 'resume cannot retroactively capture paused content',
      );
      await service.setPaused(true);
      expect(await service.copy(textEntry), true);
      await _get(text, 'text/plain');
      expect(
        (await _native.invokeMapMethod<String, dynamic>('status'))!['paused'],
        true,
      );
      await service.setPaused(false);
      await _offer(tester, Uint8List.fromList(utf8.encode('after pause')));
      await _wait(
        tester,
        () => service.entries.any((entry) => entry.text == 'after pause'),
        'recording resumes for next copy',
      );

      final png = await _png();
      expect(
        await transport.publish('image/png', png, isScreenshot: true),
        true,
      );
      final pngPaste = await _get(png, 'image/png');
      expect(pngPaste['mimes'], contains('application/x-clarion-screenshot'));
      await _offer(tester, png, mime: 'image/png', screenshot: true);
      await _wait(
        tester,
        () => service.entries.any(
          (entry) => entry.kind == RecentContentKind.screenshot,
        ),
        'real screenshot marker classification',
      );
      final screenshot = service.entries.firstWhere(
        (entry) => entry.kind == RecentContentKind.screenshot,
      );
      expect(screenshot.thumbnail, isNotNull);
      expect(await service.copy(screenshot), true);
      final repasted = await _get(png, 'image/png');
      expect(repasted['mimes'], contains('application/x-clarion-screenshot'));
      await transport.stop();
      await _get(png, 'image/png');
      expect(await transport.reconnect(), true);
      expect(
        (await _native.invokeMapMethod<String, dynamic>(
          'status',
        ))!['available'],
        true,
      );
      final green = await _png(const ui.Color(0xff35b48a));
      await _offer(tester, green, mime: 'image/png');
      await _wait(
        tester,
        () => service.entries.any(
          (entry) => entry.kind == RecentContentKind.image,
        ),
        'independent image entry',
      );
      service.togglePinned(
        service.entries.firstWhere((entry) => entry.text == utf8.decode(text)),
      );
      await _window.invokeMethod<void>('setExpanded', true);
      await tester.pumpWidget(
        MaterialApp(
          theme: ThemeData(
            useMaterial3: true,
            brightness: Brightness.dark,
            colorSchemeSeed: const Color(0xff6a9bff),
          ),
          home: Scaffold(
            backgroundColor: const Color(0xff111a2b),
            body: Center(
              child: SizedBox(
                width: 520,
                child: Card(
                  clipBehavior: Clip.antiAlias,
                  child: AnimatedBuilder(
                    animation: service,
                    builder: (context, _) => RecentContentPanel(
                      entries: service.entries,
                      onCopy: (entry) {
                        unawaited(service.copy(entry));
                      },
                      onPin: service.togglePinned,
                      onDelete: service.delete,
                      onClear: service.clear,
                      onClose: () {},
                      clipboardAvailable: service.available,
                      macos: true,
                    ),
                  ),
                ),
              ),
            ),
          ),
        ),
      );
      await _settle(tester, 650);
      _guard();
      final captured = File(
        '${_env['CLARION_CLIPBOARD_ARTIFACTS']}/native-panel.png',
      );
      final capture =
          await Process.run(_env['CLARION_CLIPBOARD_CLARION_BIN']!, [
            'msg',
            'action',
            'screenshot-screen',
            '--path',
            captured.path,
            '--show-pointer',
            'false',
          ]);
      expect(capture.exitCode, 0, reason: '${capture.stderr}');
      await _wait(
        tester,
        () => captured.existsSync() && captured.lengthSync() > 33,
        'presented panel screenshot',
      );
      await _wait(
        tester,
        () =>
            service.entries
                .where((entry) => entry.kind == RecentContentKind.screenshot)
                .length >=
            2,
        'actual core screenshot publishes its marker',
      );
      final generations = transport.observed
          .where((event) => event['type'] == 'selection')
          .map((event) => event['generation'] as int)
          .toList();
      expect(generations.first, firstGeneration);
      for (var index = 1; index < generations.length; index++) {
        expect(generations[index], greaterThan(generations[index - 1]));
      }
      await _evidence('native-evidence.json', {
        'passed': true,
        'preview': false,
        'protocol': 'ext-data-control-v1',
        'textRoundTrip': true,
        'deduplicated': true,
        'clearThenSameCopy': true,
        'sensitiveReceiveRequests': sensitive.requested.length,
        'primaryReceiveRequests': primary.requested.length,
        'oversizeRejected': true,
        'pauseResume': true,
        'publishWhilePaused': true,
        'pngRoundTrip': true,
        'pngBytes': png.length,
        'screenshotMarker': true,
        'ownershipSurvivesStop': true,
        'reconnected': true,
        'selectionGenerations': generations,
        'actualCoreScreenshotMarker': true,
        'panelCapture': captured.path,
        'retainedEntries': service.entries.length,
      });
    } finally {
      service.dispose();
      for (final process in _processes) {
        process.kill(ProcessSignal.sigterm);
      }
      for (final process in _processes) {
        await process.exitCode;
      }
    }
  }, timeout: const Timeout(Duration(minutes: 2)));
}
