// SPDX-License-Identifier: GPL-3.0-or-later
// Ordinary Flutter entrypoint: no test VM harness is needed for the --preview
// runner. It may only run inside test-recent-content-native.py's private seat.
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:clarion_shell/services/recent_content_service.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

void check(bool condition, String label) {
  if (!condition) throw StateError(label);
}

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  runApp(const MaterialApp(home: SizedBox.shrink()));
  await WidgetsBinding.instance.endOfFrame;
  final env = Platform.environment;
  final root = env['CLARION_CLIPBOARD_TEST_ROOT']!;
  final artifacts = env['CLARION_CLIPBOARD_ARTIFACTS']!;
  Process? producer;
  StreamSubscription<Object?>? subscription;
  final result = <String, Object?>{'passed': false, 'preview': true};
  try {
    final runtime = env['XDG_RUNTIME_DIR']!;
    check(env['CLARION_CLIPBOARD_ISOLATED_TEST'] == '1', 'isolation flag');
    check(runtime.startsWith('$root/'), 'private runtime');
    check(
      Directory(runtime).statSync().mode & 0x1ff == 0x1c0,
      'runtime permissions',
    );
    check(
      File('$runtime/clipboard-test-owner').readAsStringSync() == root,
      'runtime owner',
    );
    check(
      env['WAYLAND_DISPLAY']!.startsWith('$runtime/'),
      'private Wayland display',
    );
    check(
      env['WAYLAND_DISPLAY'] != env['CLARION_CLIPBOARD_PARENT_WAYLAND'],
      'parent display excluded',
    );
    check(env['WAYLAND_SOCKET'] == null, 'inherited descriptor excluded');
    check(
      env['DBUS_SESSION_BUS_ADDRESS']!.startsWith('unix:path=$runtime/'),
      'private D-Bus',
    );
    check(env['CLARION_CLIPBOARD_PEER']!.startsWith('$root/'), 'private peer');
    const window = MethodChannel('clarion.shell/window');
    final state = await window.invokeMapMethod<String, dynamic>('windowState');
    check(state!['preview'] == true, 'actual native preview window');
    final transport = NativeClipboardTransport();
    var selections = 0;
    subscription = transport.events.listen((event) {
      if (event is Map && event['type'] == 'selection') selections++;
    }, onError: (Object _) {});
    var disabled = false;
    try {
      disabled = !await transport.start();
    } on MissingPluginException {
      disabled = true;
    }
    check(disabled, 'native preview start must be disabled');
    final payload = File('$root/preview-only-payload.bin');
    final ready = File('$root/preview-only.ready');
    final requests = File('$root/preview-only.requests');
    await payload.writeAsString('Synthetic preview clipboard fixture only');
    producer = await Process.start(env['CLARION_CLIPBOARD_PEER']!, [
      'offer',
      '--mime',
      'text/plain',
      '--file',
      payload.path,
      '--ready',
      ready.path,
      '--requests',
      requests.path,
    ]);
    unawaited(producer.stdout.drain<void>());
    unawaited(producer.stderr.drain<void>());
    final deadline = DateTime.now().add(const Duration(seconds: 8));
    while (!ready.existsSync()) {
      check(DateTime.now().isBefore(deadline), 'private producer readiness');
      await Future<void>.delayed(const Duration(milliseconds: 30));
    }
    await Future<void>.delayed(const Duration(milliseconds: 600));
    check(
      !requests.existsSync() || requests.lengthSync() == 0,
      'preview must never receive clipboard bytes',
    );
    check(selections == 0, 'preview must never emit captured content');
    var rejected = false;
    try {
      rejected = !await transport.publish(
        'text/plain',
        Uint8List.fromList(utf8.encode('must not publish')),
      );
    } on PlatformException {
      rejected = true;
    } on MissingPluginException {
      rejected = true;
    }
    check(rejected, 'preview must never publish selection');
    result.addAll({
      'passed': true,
      'startDisabled': disabled,
      'publishRejected': rejected,
      'producerReceiveRequests': 0,
      'selectionEvents': selections,
    });
  } on Object catch (error, stack) {
    result['error'] = '$error';
    stderr.writeln('$error\n$stack');
  } finally {
    producer?.kill(ProcessSignal.sigterm);
    if (producer != null) await producer.exitCode;
    await subscription?.cancel();
    await File(
      '$artifacts/preview-evidence.json',
    ).writeAsString('${const JsonEncoder.withIndent('  ').convert(result)}\n');
  }
}
