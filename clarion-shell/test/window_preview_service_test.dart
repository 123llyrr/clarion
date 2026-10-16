// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/services/clarion_ipc.dart';
import 'package:clarion_shell/services/window_preview_service.dart';

class PreviewIpc extends ClarionIpc {
  final eventController = StreamController<Map<String, dynamic>>.broadcast();
  final requests = <Map<String, Object?>>[];
  Completer<Map<String, Object?>>? waiting;
  Future<void> Function()? acknowledge;

  @override
  Stream<Map<String, dynamic>> get events => eventController.stream;

  Future<Map<String, Object?>> nextRequest() =>
      (waiting = Completer<Map<String, Object?>>()).future;

  @override
  Future<void> action(
    String name, [
    Map<String, Object?> arguments = const {},
  ]) async {
    expect(name, 'ScreenshotWindow');
    requests.add(arguments);
    waiting?.complete(arguments);
    waiting = null;
    await acknowledge?.call();
  }

  Future<void> complete(Map<String, Object?> request) async {
    final path = request['path']! as String;
    await File(path).writeAsBytes([1, 2, 3]);
    eventController.add({
      'ScreenshotCaptured': {'path': path},
    });
  }
}

void main() {
  late PreviewIpc ipc;
  late WindowPreviewService service;
  setUp(() {
    ipc = PreviewIpc();
    service = WindowPreviewService(ipc);
  });
  tearDown(() async {
    await service.dispose();
    await ipc.eventController.close();
  });

  test(
    'acceptance and unrelated events cannot publish an unfinished file',
    () async {
      var published = false;
      final next = ipc.nextRequest();
      final capture = service.capture(7)..then((_) => published = true);
      final request = await next;
      await Future<void>.delayed(Duration.zero);
      expect(published, isFalse);
      ipc.eventController.add({
        'ScreenshotCaptured': {'path': '/unrelated.png'},
      });
      await Future<void>.delayed(Duration.zero);
      expect(published, isFalse);
      await ipc.complete(request);
      expect(await capture, request['path']);
    },
  );

  test(
    'duplicate requests share one capture and different windows run serially',
    () async {
      final next = ipc.nextRequest();
      final first = service.capture(1);
      expect(identical(first, service.capture(1)), isTrue);
      final second = service.capture(2);
      final request = await next;
      expect(ipc.requests, hasLength(1));
      final nextSecond = ipc.nextRequest();
      await ipc.complete(request);
      final secondRequest = await nextSecond;
      expect(secondRequest['id'], 2);
      await ipc.complete(secondRequest);
      expect(await first, request['path']);
      expect(await second, secondRequest['path']);
    },
  );

  test(
    'completion arriving before action acknowledgement is retained',
    () async {
      final ack = Completer<void>();
      ipc.acknowledge = () => ack.future;
      final next = ipc.nextRequest();
      final capture = service.capture(9);
      final request = await next;
      await ipc.complete(request);
      await Future<void>.delayed(Duration.zero);
      ack.complete();
      expect(await capture, request['path']);
    },
  );

  test(
    'closing a panel drops queued captures without starting a parallel readback',
    () async {
      final next = ipc.nextRequest();
      final first = service.capture(1);
      final cancelled = service.capture(2);
      final request = await next;
      service.cancelQueued();
      final nextFresh = ipc.nextRequest();
      final fresh = service.capture(1);
      expect(ipc.requests, hasLength(1));
      await ipc.complete(request);
      final freshRequest = await nextFresh;
      expect(await first, isNull);
      expect(await cancelled, isNull);
      expect(freshRequest['path'], isNot(request['path']));
      expect(await File(request['path']! as String).exists(), isFalse);
      await ipc.complete(freshRequest);
      expect(await fresh, freshRequest['path']);
      expect(ipc.requests, hasLength(2));
    },
  );

  test(
    'missing completion times out and a later capture can succeed',
    () async {
      await service.dispose();
      service = WindowPreviewService(
        ipc,
        timeout: const Duration(milliseconds: 50),
      );
      final first = service.capture(1);
      final request = await ipc.nextRequest();
      expect(await first, isNull);
      final second = service.capture(1);
      final freshRequest = await ipc.nextRequest();
      expect(freshRequest['path'], isNot(request['path']));
      await ipc.complete(freshRequest);
      expect(await second, freshRequest['path']);
    },
  );

  test('directory and action failures leave the queue usable', () async {
    ipc.acknowledge = () => Future<void>.error(StateError('window closed'));
    expect(await service.capture(1), isNull);
    ipc.acknowledge = null;
    final next = ipc.nextRequest();
    final capture = service.capture(2);
    await ipc.complete(await next);
    expect(await capture, isNotNull);
    await service.dispose();
    service = WindowPreviewService(
      ipc,
      createDirectory: () async => throw const FileSystemException(),
    );
    expect(await service.capture(3), isNull);
    expect(await service.capture(4), isNull);
  });
}
