// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:dbus/dbus.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:realand_shell/services/notification_service.dart';

const _interface = 'org.freedesktop.Notifications';
final _path = DBusObjectPath('/org/freedesktop/Notifications');

typedef _Sent = ({String? destination, String name, List<DBusValue> values});

class _Bus extends DBusClient {
  _Bus(super.address);
  final sent = <_Sent>[];
  Completer<void>? actionGate;
  Completer<void>? closeGate;
  bool failActions = false;

  @override
  Future<void> emitSignal({
    String? destination,
    required DBusObjectPath path,
    required String interface,
    required String name,
    Iterable<DBusValue> values = const [],
  }) async {
    if (name == 'ActionInvoked') {
      await actionGate?.future;
      if (failActions) throw StateError('signal delivery failed');
    }
    if (name == 'NotificationClosed') await closeGate?.future;
    await super.emitSignal(
      destination: destination,
      path: path,
      interface: interface,
      name: name,
      values: values,
    );
    sent.add((destination: destination, name: name, values: values.toList()));
  }
}

class _Fixture {
  _Fixture(this.directory, this.server, this.address, this.bus, this.service);
  final Directory directory;
  final DBusServer server;
  final DBusAddress address;
  final _Bus bus;
  final NotificationService service;
  Process? daemon;
  Future<void>? daemonErrors;
  final clients = <DBusClient>[];
  final subscriptions = <StreamSubscription<DBusSignal>>[];

  static Future<_Fixture> create({
    int historyLimit = 100,
    bool realDaemon = false,
  }) async {
    final directory = await Directory.systemTemp.createTemp('realand-notify-');
    final server = DBusServer();
    Process? daemon;
    Future<void>? errors;
    final DBusAddress address;
    if (realDaemon) {
      // The Dart DBusServer also delivers directed signals to match-rule
      // observers. Verify destination privacy against the real daemon instead.
      daemon = await Process.start('/usr/bin/dbus-daemon', [
        '--session',
        '--nofork',
        '--nopidfile',
        '--print-address=1',
        '--address=unix:path=${directory.path}/bus',
      ]);
      errors = daemon.stderr.drain<void>();
      address = DBusAddress(
        await daemon.stdout
            .transform(utf8.decoder)
            .transform(const LineSplitter())
            .first
            .timeout(const Duration(seconds: 3)),
      );
    } else {
      address = await server.listenAddress(
        DBusAddress.unix(path: '${directory.path}/bus'),
      );
    }
    final bus = _Bus(address);
    final service = NotificationService(bus: bus, historyLimit: historyLimit);
    final fixture = _Fixture(directory, server, address, bus, service)
      ..daemon = daemon
      ..daemonErrors = errors;
    addTearDown(fixture.close);
    expect(await service.start(), true);
    return fixture;
  }

  DBusClient client() {
    final client = DBusClient(address);
    clients.add(client);
    return client;
  }

  DBusRemoteObject object(DBusClient client) =>
      DBusRemoteObject(client, name: _interface, path: _path);

  Future<int> notify(
    DBusClient client, {
    int replace = 0,
    int timeout = 0,
    String summary = 'A notification',
    List<String> actions = const ['default', 'Open', 'reply', 'Reply'],
    Map<String, DBusValue> hints = const {},
  }) async {
    final result = await object(client).callMethod(_interface, 'Notify', [
      const DBusString('Real sender'),
      DBusUint32(replace),
      const DBusString(''),
      DBusString(summary),
      const DBusString('Body'),
      DBusArray.string(actions),
      DBusDict.stringVariant(hints),
      DBusInt32(timeout),
    ], replySignature: DBusSignature('u'));
    return result.returnValues.single.asUint32();
  }

  Future<void> closeNotification(DBusClient client, int id) async {
    await object(client).callMethod(_interface, 'CloseNotification', [
      DBusUint32(id),
    ], replySignature: DBusSignature(''));
  }

  Future<List<DBusSignal>> listen(DBusClient client) async {
    final received = <DBusSignal>[];
    subscriptions.add(
      DBusSignalStream(
        client,
        sender: _interface,
        path: _path,
        interface: _interface,
      ).listen(received.add),
    );
    await client.getNameOwner(_interface);
    return received;
  }

  Future<void> close() async {
    for (final subscription in subscriptions) {
      await subscription.cancel();
    }
    service.dispose();
    await bus.close();
    for (final client in clients) {
      await client.close();
    }
    await server.close();
    final process = daemon;
    if (process != null) {
      process.kill(ProcessSignal.sigterm);
      await process.exitCode.timeout(
        const Duration(seconds: 3),
        onTimeout: () {
          process.kill(ProcessSignal.sigkill);
          return -1;
        },
      );
      await daemonErrors;
    }
    await directory.delete(recursive: true);
  }
}

Future<void> _eventually(bool Function() condition) async {
  for (var attempt = 0; attempt < 200; attempt++) {
    if (condition()) return;
    await Future<void>.delayed(const Duration(milliseconds: 5));
  }
  expect(condition(), true, reason: 'D-Bus event did not arrive');
}

void main() {
  test('history capacity must be positive', () {
    expect(() => NotificationService(historyLimit: 0), throwsRangeError);
    expect(() => NotificationService(historyLimit: -1), throwsRangeError);
  });

  test(
    'default action and dismissal are ordered and directed to the sender',
    () async {
      final f = await _Fixture.create(realDaemon: true);
      final caller = f.client();
      final received = await f.listen(caller);
      final observer = await f.listen(f.client());
      final id = await f.notify(caller);
      final item = f.service.notifications.single;
      expect(await f.service.invokeAction(id, 'default', expected: item), true);
      expect(f.service.notifications, isEmpty);
      await _eventually(() => received.length == 2);
      expect(received.map((s) => s.name), [
        'ActionInvoked',
        'NotificationClosed',
      ]);
      expect(received[0].values, [DBusUint32(id), const DBusString('default')]);
      expect(received[1].values, [DBusUint32(id), const DBusUint32(2)]);
      expect(
        f.bus.sent.every((signal) => signal.destination == caller.uniqueName),
        true,
      );
      expect(observer, isEmpty);
      expect(await f.service.invokeAction(id, 'default'), false);
      expect(await f.service.dismiss(id), false);
      expect(f.bus.sent, hasLength(2));
    },
  );

  test(
    'resident actions remain live and preserve opaque action keys',
    () async {
      final f = await _Fixture.create();
      final caller = f.client();
      final key = 'reply:${'x' * 300}';
      final id = await f.notify(
        caller,
        timeout: -1,
        actions: [key, 'Reply'],
        hints: {'resident': const DBusBoolean(true)},
      );
      final item = f.service.notifications.single;
      expect(item.resident, true);
      await Future<void>.delayed(const Duration(milliseconds: 50));
      expect(item.active, true);
      expect(identical(f.service.notifications.single, item), true);
      expect(f.bus.sent, isEmpty);
      expect(() => item.actions.clear(), throwsUnsupportedError);
      expect(await f.service.invokeAction(id, key, expected: item), true);
      expect(identical(f.service.notifications.single, item), true);
      expect(f.bus.sent.single.values, [DBusUint32(id), DBusString(key)]);
      expect(await f.service.invokeAction(id, 'missing'), false);
      expect(await f.service.dismiss(id, expected: item), true);
      expect(f.bus.sent.last.values.last, const DBusUint32(2));
    },
  );

  test('resident actions honor an explicit sender expiry deadline', () async {
    final f = await _Fixture.create();
    final id = await f.notify(
      f.client(),
      timeout: 150,
      hints: {'resident': const DBusBoolean(true)},
    );
    final item = f.service.notifications.single;
    expect(await f.service.invokeAction(id, 'default', expected: item), true);
    expect(identical(f.service.notifications.single, item), true);
    await _eventually(() => f.bus.sent.length == 2);
    expect(f.bus.sent.map((signal) => signal.name), [
      'ActionInvoked',
      'NotificationClosed',
    ]);
    expect(f.bus.sent.last.values.last, const DBusUint32(1));
    expect(f.service.notifications.single.active, false);
  });

  test(
    'duplicate, empty, oversized and incomplete actions cannot break the UI',
    () async {
      final f = await _Fixture.create();
      await f.notify(
        f.client(),
        actions: [
          'default',
          'First',
          'default',
          'Duplicate',
          '',
          'Empty',
          'x' * 4097,
          'Oversized',
          'reply',
          'Reply',
          'incomplete',
        ],
      );
      final actions = f.service.notifications.single.actions;
      expect(actions.map((action) => action.key), ['default', 'reply']);
      expect(actions.first.label, 'First');
    },
  );

  test(
    'sender close cannot overtake a pending action or target a new notification',
    () async {
      final f = await _Fixture.create();
      final caller = f.client();
      final id = await f.notify(caller);
      f.bus.actionGate = Completer<void>();
      final action = f.service.invokeAction(id, 'default');
      final closing = f.closeNotification(caller, id);
      await _eventually(() => f.service.notifications.isEmpty);
      final fresh = await f.notify(f.client(), replace: id);
      expect(fresh, isNot(id));
      expect(f.bus.sent, isEmpty);
      f.bus.actionGate!.complete();
      expect(await action, true);
      await closing;
      expect(f.bus.sent.map((signal) => signal.name), [
        'ActionInvoked',
        'NotificationClosed',
      ]);
      expect(
        f.bus.sent.every((signal) => signal.destination == caller.uniqueName),
        true,
      );
      expect(f.bus.sent.last.values, [DBusUint32(id), const DBusUint32(3)]);
      expect(f.service.notifications.single.id, fresh);
      expect(f.service.notifications.single.actions, isNotEmpty);
    },
  );

  test(
    'group dismissal skips replaced identities and preserves other notifications',
    () async {
      final f = await _Fixture.create();
      final caller = f.client();
      final first = await f.notify(caller);
      final second = await f.notify(caller);
      final snapshot = f.service.notifications;
      await f.notify(caller, replace: second, summary: 'Replacement');
      final fresh = await f.notify(caller);
      f.bus.closeGate = Completer<void>();
      final dismissed = f.service.dismissAll(snapshot);
      expect(f.service.notifications.map((item) => item.id), [fresh, second]);
      f.bus.closeGate!.complete();
      expect(await dismissed, false);
      expect(f.bus.sent.single.values, [
        DBusUint32(first),
        const DBusUint32(2),
      ]);
    },
  );

  test(
    'failed action remains retryable and duplicate in-flight action is rejected',
    () async {
      final f = await _Fixture.create();
      final id = await f.notify(f.client());
      final item = f.service.notifications.single;
      f.bus.actionGate = Completer<void>();
      f.bus.failActions = true;
      final action = f.service.invokeAction(id, 'default', expected: item);
      final failed = expectLater(action, throwsStateError);
      expect(
        await f.service.invokeAction(id, 'default', expected: item),
        false,
      );
      f.bus.actionGate!.complete();
      await failed;
      expect(identical(f.service.notifications.single, item), true);
      expect(f.bus.sent, isEmpty);
      f.bus.actionGate = null;
      f.bus.failActions = false;
      expect(await f.service.invokeAction(id, 'default', expected: item), true);
    },
  );

  test(
    'replacement during pending action keeps new identity and new sender',
    () async {
      final f = await _Fixture.create();
      final oldClient = f.client();
      final newClient = f.client();
      final id = await f.notify(oldClient);
      final oldItem = f.service.notifications.single;
      f.bus.actionGate = Completer<void>();
      final action = f.service.invokeAction(id, 'default', expected: oldItem);
      expect(
        await f.notify(newClient, replace: id, summary: 'Replacement'),
        id,
      );
      final replacement = f.service.notifications.single;
      expect(await f.service.dismiss(id, expected: oldItem), false);
      expect(
        await f.service.invokeAction(id, 'default', expected: oldItem),
        false,
      );
      f.bus.actionGate!.complete();
      expect(await action, true);
      expect(identical(f.service.notifications.single, replacement), true);
      expect(replacement.active, true);
      expect(f.bus.sent, hasLength(1));
      expect(f.bus.sent.single.destination, oldClient.uniqueName);
      await f.closeNotification(oldClient, id);
      expect(f.bus.sent.last.name, 'NotificationClosed');
      expect(f.bus.sent.last.destination, newClient.uniqueName);
      expect(f.bus.sent.last.values.last, const DBusUint32(3));
    },
  );

  test(
    'replacement cancels previous expiry and closed IDs are not reused',
    () async {
      final f = await _Fixture.create();
      final caller = f.client();
      final id = await f.notify(caller, timeout: 60);
      expect(await f.notify(caller, replace: id, timeout: 0), id);
      await Future<void>.delayed(const Duration(milliseconds: 100));
      expect(f.service.notifications.single.active, true);
      expect(f.bus.sent, isEmpty);
      await f.closeNotification(caller, id);
      final next = await f.notify(caller, replace: id);
      expect(next, isNot(id));
      expect(f.service.notifications.single.id, next);
    },
  );

  test(
    'expiration invalidates history and critical notifications never autoexpire',
    () async {
      final f = await _Fixture.create();
      final caller = f.client();
      final expiring = await f.notify(caller, timeout: 20);
      final critical = await f.notify(
        caller,
        timeout: 20,
        hints: {'urgency': const DBusByte(2)},
      );
      final transient = await f.notify(
        caller,
        timeout: 20,
        hints: {'transient': const DBusBoolean(true)},
      );
      await _eventually(() => f.bus.sent.length == 2);
      expect(f.service.notifications.map((item) => item.id), [
        critical,
        expiring,
      ]);
      expect(f.service.notifications.first.active, true);
      final history = f.service.notifications.last;
      expect(history.active, false);
      expect(history.actions, isEmpty);
      expect(await f.service.invokeAction(expiring, 'default'), false);
      expect(f.bus.sent.map((s) => s.values.first.asUint32()).toSet(), {
        expiring,
        transient,
      });
      expect(
        f.bus.sent.every((s) => s.values.last == const DBusUint32(1)),
        true,
      );
      expect(await f.service.dismiss(expiring, expected: history), true);
      expect(f.bus.sent, hasLength(2));
      await expectLater(
        f.closeNotification(caller, expiring),
        throwsA(isA<DBusMethodResponseException>()),
      );
    },
  );

  test(
    'sender close returns an empty error for unknown or already closed IDs',
    () async {
      final f = await _Fixture.create();
      final caller = f.client();
      Future<void> expectUnknown(int id) async {
        try {
          await f.closeNotification(caller, id);
          fail('unknown ID must fail');
        } on DBusMethodResponseException catch (error) {
          expect(error.response.values, isEmpty);
        }
      }

      await expectUnknown(100);
      final id = await f.notify(caller);
      await f.closeNotification(caller, id);
      await expectUnknown(id);
      expect(f.bus.sent.single.values, [DBusUint32(id), const DBusUint32(3)]);
    },
  );

  test(
    'clear invalidates its whole snapshot before awaiting signal delivery',
    () async {
      final f = await _Fixture.create();
      final caller = f.client();
      final first = await f.notify(caller);
      final second = await f.notify(caller);
      f.bus.closeGate = Completer<void>();
      final clear = f.service.clear();
      expect(f.service.notifications, isEmpty);
      final fresh = await f.notify(caller, replace: second);
      expect(fresh, isNot(second));
      f.bus.closeGate!.complete();
      expect(await clear, true);
      expect(f.service.notifications.single.id, fresh);
      expect(f.bus.sent.map((s) => s.values.first.asUint32()).toSet(), {
        first,
        second,
      });
      expect(
        f.bus.sent.every((s) => s.values.last == const DBusUint32(2)),
        true,
      );
    },
  );

  test(
    'history eviction is bounded immediately and uses undefined close reason',
    () async {
      final f = await _Fixture.create(historyLimit: 2);
      final caller = f.client();
      final first = await f.notify(caller);
      await f.notify(caller);
      f.bus.closeGate = Completer<void>();
      await f.notify(caller);
      await f.notify(caller);
      expect(f.service.notifications, hasLength(2));
      expect(f.service.notifications.any((item) => item.id == first), false);
      f.bus.closeGate!.complete();
      await _eventually(() => f.bus.sent.length == 2);
      expect(
        f.bus.sent.every((s) => s.values.last == const DBusUint32(4)),
        true,
      );
    },
  );

  test(
    'burst and clear coalesce changes while snapshots stay immutable',
    () async {
      final f = await _Fixture.create();
      var changes = 0;
      final subscription = f.service.changes.listen((_) => changes++);
      addTearDown(subscription.cancel);
      // Direct method dispatch keeps the producer burst in one event-loop turn;
      // the other tests exercise these exact methods over private D-Bus sockets.
      final requests = List.generate(
        100,
        (index) => f.service.handleMethodCall(
          DBusMethodCall(
            sender: null,
            interface: _interface,
            name: 'Notify',
            values: [
              const DBusString('Burst'),
              const DBusUint32(0),
              const DBusString(''),
              DBusString('$index'),
              const DBusString(''),
              DBusArray.string([]),
              DBusDict.stringVariant({}),
              const DBusInt32(0),
            ],
          ),
        ),
      );
      await Future.wait(requests);
      final snapshot = f.service.notifications;
      expect(snapshot, hasLength(100));
      expect(identical(snapshot, f.service.notifications), true);
      expect(() => snapshot.clear(), throwsUnsupportedError);
      await _eventually(() => changes == 1);
      await f.service.clear();
      expect(snapshot, hasLength(100));
      expect(f.service.notifications, isEmpty);
      await _eventually(() => changes == 2);
      await Future<void>.delayed(const Duration(milliseconds: 30));
      expect(changes, 2);
    },
  );

  test(
    'concurrent starts share ownership and disposal cancels expiry and change timers',
    () async {
      final directory = await Directory.systemTemp.createTemp(
        'realand-notify-start-',
      );
      final server = DBusServer();
      final address = await server.listenAddress(
        DBusAddress.unix(path: '${directory.path}/bus'),
      );
      final bus = _Bus(address);
      final service = NotificationService(bus: bus);
      final f = _Fixture(directory, server, address, bus, service);
      addTearDown(f.close);
      expect(await Future.wait([service.start(), service.start()]), [
        true,
        true,
      ]);
      var changes = 0;
      final subscription = service.changes.listen((_) => changes++);
      addTearDown(subscription.cancel);
      final request = service.handleMethodCall(
        DBusMethodCall(
          sender: null,
          interface: _interface,
          name: 'Notify',
          values: [
            const DBusString('Dispose'),
            const DBusUint32(0),
            const DBusString(''),
            const DBusString('Summary'),
            const DBusString(''),
            DBusArray.string([]),
            DBusDict.stringVariant({}),
            const DBusInt32(20),
          ],
        ),
      );
      service.dispose();
      await request;
      await Future<void>.delayed(const Duration(milliseconds: 50));
      expect(changes, 0);
      expect(bus.sent, isEmpty);
      expect(await service.clear(), false);
      expect(await service.start(), false);
    },
  );
}
