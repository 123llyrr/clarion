// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/services/shell_startup.dart';

void main() {
  test('readiness waits only for the first rendered frame', () async {
    final frame = Completer<void>();
    var notifications = 0;
    final messages = <String>[];
    final ready = announceShellReady(
      firstFrame: frame.future,
      notifyReady: () async => notifications++,
      report: messages.add,
    );
    await Future<void>.delayed(Duration.zero);
    expect(notifications, 0);
    frame.complete();
    expect(await ready, isTrue);
    expect(notifications, 1);
    expect(messages, isEmpty);
  });

  test('a failed frame never announces readiness', () async {
    var notifications = 0;
    final messages = <String>[];
    expect(
      await announceShellReady(
        firstFrame: Future<void>.error(StateError('frame failed')),
        notifyReady: () async => notifications++,
        report: messages.add,
      ),
      isFalse,
    );
    expect(notifications, 0);
    expect(messages.single, contains('first frame'));
  });

  test('a missing frame times out and a late failure is handled', () async {
    final frame = Completer<void>();
    var notifications = 0;
    final messages = <String>[];
    expect(
      await announceShellReady(
        firstFrame: frame.future,
        notifyReady: () async => notifications++,
        report: messages.add,
        frameTimeout: const Duration(milliseconds: 15),
      ),
      isFalse,
    );
    expect(notifications, 0);
    expect(messages.single, contains('first frame'));
    frame.completeError(StateError('late frame failure'));
    await Future<void>.delayed(Duration.zero);
    expect(notifications, 0);
  });

  test('failed readiness notification is reported without success', () async {
    final messages = <String>[];
    expect(
      await announceShellReady(
        firstFrame: Future.value(),
        notifyReady: () => throw StateError('no manager'),
        report: messages.add,
      ),
      isFalse,
    );
    expect(messages.single, contains('readiness notification'));
  });
}
