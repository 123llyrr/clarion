// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:realand_shell/services/shell_startup.dart';

void main() {
  test('readiness waits for the frame and registrar independently', () async {
    final frame = Completer<void>();
    final menus = Completer<void>();
    var preparations = 0;
    var notifications = 0;
    final messages = <String>[];
    final ready = announceShellReady(
      firstFrame: frame.future,
      prepareAppMenus: () {
        preparations++;
        return menus.future;
      },
      notifyReady: () async => notifications++,
      report: messages.add,
    );
    expect(preparations, 1);
    frame.complete();
    await Future<void>.delayed(Duration.zero);
    expect(notifications, 0);
    menus.complete();
    expect(await ready, isTrue);
    expect(notifications, 1);
    expect(messages, isEmpty);
  });

  test('registrar completion cannot announce an undrawn shell', () async {
    final frame = Completer<void>();
    var notifications = 0;
    final ready = announceShellReady(
      firstFrame: frame.future,
      prepareAppMenus: () async {},
      notifyReady: () async => notifications++,
      report: (_) {},
    );
    await Future<void>.delayed(Duration.zero);
    expect(notifications, 0);
    frame.complete();
    expect(await ready, isTrue);
    expect(notifications, 1);
  });

  test('unresponsive menu service does not cause a startup timeout', () async {
    final menus = Completer<void>();
    final messages = <String>[];
    var notifications = 0;
    final ready = announceShellReady(
      firstFrame: Future.value(),
      prepareAppMenus: () => menus.future,
      notifyReady: () async => notifications++,
      report: messages.add,
      menuTimeout: const Duration(milliseconds: 15),
    );
    expect(await ready, isTrue);
    expect(notifications, 1);
    expect(messages.single, contains('application menu'));
    // Future.timeout cannot cancel the bus operation; a late error is handled.
    menus.completeError(StateError('late failure'));
    await Future<void>.delayed(Duration.zero);
    expect(notifications, 1);
  });

  test('missing frame and failed notification are not called ready', () async {
    var notifications = 0;
    final messages = <String>[];
    expect(
      await announceShellReady(
        firstFrame: Completer<void>().future,
        prepareAppMenus: () async {},
        notifyReady: () async => notifications++,
        report: messages.add,
        frameTimeout: const Duration(milliseconds: 15),
      ),
      isFalse,
    );
    expect(notifications, 0);
    expect(messages.single, contains('first frame'));
    expect(
      await announceShellReady(
        firstFrame: Future.value(),
        prepareAppMenus: () => throw StateError('no bus'),
        notifyReady: () => throw StateError('no manager'),
        report: messages.add,
      ),
      isFalse,
    );
    expect(messages.last, contains('readiness notification'));
  });
}
