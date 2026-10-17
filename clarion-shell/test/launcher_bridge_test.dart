// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/services/launcher_bridge.dart';

class _Server {
  _Server(this.directory);
  final Directory directory;
  ServerSocket? server;
  final clients = <Socket>[];
  final commands = <Map<String, dynamic>>[];
  String get path => '${directory.path}/clarion-launcher.sock';
  Future<void> start() async {
    server = await ServerSocket.bind(
      InternetAddress(path, type: InternetAddressType.unix),
      0,
    );
    server!.listen((client) {
      clients.add(client);
      client
          .cast<List<int>>()
          .transform(utf8.decoder)
          .transform(const LineSplitter())
          .listen(
            (line) => commands.add(
              Map<String, dynamic>.from(jsonDecode(line) as Map),
            ),
            onError: (Object _) {},
          );
    });
  }

  Future<void> send(Map<String, Object?> event) async {
    clients.last.writeln(jsonEncode(event));
    await clients.last.flush();
  }

  Future<void> dispose() async {
    for (final client in clients) {
      client.destroy();
    }
    await server?.close();
  }
}

Future<void> _until(bool Function() ready) async {
  final deadline = DateTime.now().add(const Duration(seconds: 2));
  while (!ready()) {
    if (DateTime.now().isAfter(deadline)) {
      fail('Timed out waiting for launcher fixture');
    }
    await Future<void>.delayed(const Duration(milliseconds: 5));
  }
}

void main() {
  late Directory directory;
  late _Server server;
  setUp(() async {
    directory = await Directory.systemTemp.createTemp(
      'clarion-launcher-bridge-',
    );
    server = _Server(directory);
  });
  tearDown(() async {
    await server.dispose();
    await directory.delete(recursive: true);
  });

  test(
    'reuses existing Unix server, filters stale visibility, preserves launched',
    () async {
      await server.start();
      var spawns = 0;
      final bridge = LauncherBridge(
        environment: {
          'XDG_RUNTIME_DIR': directory.path,
          'CLARION_LAUNCHER_BIN': '/not/installed',
        },
        spawn: (_, _) async {
          spawns++;
        },
      );
      final events = <LauncherEvent>[];
      final subscription = bridge.events.listen(events.add);
      final first = bridge.beginRequest();
      await bridge.show(
        requestId: first,
        output: 'test-output',
        theme: 'dark',
        accent: '#123456',
        recent: ['one.desktop'],
      );
      await _until(() => server.commands.length == 2);
      expect(server.commands.first, {'type': 'Reload', 'request_id': first});
      expect(server.commands.last, {
        'type': 'Show',
        'request_id': first,
        'output': 'test-output',
        'theme': 'dark',
        'accent': '#123456',
        'recent': ['one.desktop'],
      });
      await server.send({'type': 'Visible', 'request_id': first});
      await _until(() => events.length == 1);
      final hide = bridge.beginRequest();
      await bridge.hide(hide);
      await server.send({'type': 'Hidden', 'request_id': first});
      await server.send({
        'type': 'Launched',
        'request_id': first,
        'app_id': 'one.desktop',
      });
      await server.send({'type': 'Hidden', 'request_id': hide});
      await _until(() => events.length == 3);
      expect(events.map((e) => e.type), ['Visible', 'Launched', 'Hidden']);
      expect(events[1].requestId, first);
      expect(spawns, 0);
      await bridge.dispose();
      // The reused daemon and its socket belong to the daemon, not this bridge.
      final another = await Socket.connect(
        InternetAddress(server.path, type: InternetAddressType.unix),
        0,
      );
      another.destroy();
      await subscription.cancel();
    },
  );

  test(
    'starts once with serve/output/parent arguments and bounded socket startup',
    () async {
      final invocations = <List<String>>[];
      final bridge = LauncherBridge(
        environment: {
          'XDG_RUNTIME_DIR': directory.path,
          'CLARION_LAUNCHER_BIN': '/bin/true',
        },
        spawn: (binary, arguments) async {
          expect(binary, '/bin/true');
          invocations.add(arguments);
          await server.start();
        },
      );
      final id = bridge.beginRequest();
      await bridge.show(
        requestId: id,
        output: 'winit',
        theme: 'system',
        accent: '#ffffff',
        recent: [],
      );
      expect(invocations.single, [
        '--serve',
        '--output',
        'winit',
        '--parent-pid',
        '$pid',
      ]);
      final second = bridge.beginRequest();
      await bridge.show(
        requestId: second,
        theme: 'light',
        accent: '#000000',
        recent: [],
      );
      await _until(() => server.commands.length == 4);
      expect(server.commands.map((command) => command['type']), [
        'Reload',
        'Show',
        'Reload',
        'Show',
      ]);
      expect(server.commands[2]['request_id'], second);
      expect(invocations.length, 1);
      await bridge.dispose();
    },
  );

  test('startup failure is reported and a close never spawns', () async {
    var spawns = 0;
    final bridge = LauncherBridge(
      environment: {
        'XDG_RUNTIME_DIR': directory.path,
        'CLARION_LAUNCHER_BIN': '/bin/true',
      },
      spawn: (_, _) async {
        spawns++;
        throw StateError('fixture startup failure');
      },
    );
    final events = <LauncherEvent>[];
    final subscription = bridge.events.listen(events.add);
    final id = bridge.beginRequest();
    await expectLater(
      bridge.show(requestId: id, theme: 'dark', accent: '#000000', recent: []),
      throwsStateError,
    );
    await _until(() => events.isNotEmpty);
    expect(events.single.context, 'ipc');
    await bridge.hide(bridge.beginRequest());
    expect(spawns, 1);
    await subscription.cancel();
    await bridge.dispose();
  });

  test(
    'startup without socket times out rather than leaving UI pending',
    () async {
      final bridge = LauncherBridge(
        environment: {
          'XDG_RUNTIME_DIR': directory.path,
          'CLARION_LAUNCHER_BIN': '/bin/true',
        },
        spawn: (_, _) async {},
        startupTimeout: const Duration(milliseconds: 70),
      );
      final id = bridge.beginRequest();
      await expectLater(
        bridge.show(
          requestId: id,
          theme: 'dark',
          accent: '#000000',
          recent: [],
        ),
        throwsA(isA<TimeoutException>()),
      );
      await bridge.dispose();
    },
  );

  test(
    'generation binary precedes PATH and explicit override is authoritative',
    () async {
      final bundle = Directory('${directory.path}/generation/bundle');
      await bundle.create(recursive: true);
      final shell = await File(
        '${bundle.path}/clarion-shell',
      ).writeAsString('fixture');
      final launcher = await File(
        '${bundle.parent.path}/clarion-launcher',
      ).writeAsString('fixture');
      await Process.run('chmod', ['+x', launcher.path]);
      final bridge = LauncherBridge(
        environment: {'PATH': '/usr/bin'},
        shellExecutable: shell.path,
      );
      expect(bridge.resolveBinary(), launcher.path);
      await bridge.dispose();
      final explicit = LauncherBridge(
        environment: {'CLARION_LAUNCHER_BIN': 'relative-launcher'},
        shellExecutable: shell.path,
      );
      expect(explicit.resolveBinary, throwsStateError);
      await explicit.dispose();
    },
  );
  test(
    'two reused connections keep their identical local request IDs separate',
    () async {
      await server.start();
      final first = LauncherBridge(
        environment: {'XDG_RUNTIME_DIR': directory.path},
      );
      final second = LauncherBridge(
        environment: {'XDG_RUNTIME_DIR': directory.path},
      );
      final firstEvents = <LauncherEvent>[], secondEvents = <LauncherEvent>[];
      final one = first.events.listen(firstEvents.add),
          two = second.events.listen(secondEvents.add);
      final idOne = first.beginRequest(), idTwo = second.beginRequest();
      expect(idOne, idTwo);
      await first.show(
        requestId: idOne,
        theme: 'dark',
        accent: '#000000',
        recent: [],
      );
      await second.show(
        requestId: idTwo,
        theme: 'light',
        accent: '#ffffff',
        recent: [],
      );
      await _until(
        () => server.clients.length == 2 && server.commands.length == 4,
      );
      server.clients[0].writeln(
        jsonEncode({'type': 'Visible', 'request_id': idOne}),
      );
      await server.clients[0].flush();
      await _until(() => firstEvents.isNotEmpty);
      expect(secondEvents, isEmpty);
      await first.dispose();
      await server.send({'type': 'Visible', 'request_id': idTwo});
      await _until(() => secondEvents.isNotEmpty);
      expect(secondEvents.single.type, 'Visible');
      await second.dispose();
      await one.cancel();
      await two.cancel();
    },
  );
  test(
    'reopening a reused daemon refreshes its catalog for the new request',
    () async {
      await server.start();
      final bridge = LauncherBridge(
        environment: {'XDG_RUNTIME_DIR': directory.path},
      );
      final first = bridge.beginRequest();
      await bridge.show(
        requestId: first,
        theme: 'dark',
        accent: '#ffffff',
        recent: [],
      );
      final closed = bridge.beginRequest();
      await bridge.hide(closed);
      final reopened = bridge.beginRequest();
      await bridge.show(
        requestId: reopened,
        theme: 'dark',
        accent: '#ffffff',
        recent: [],
      );
      await _until(() => server.commands.length == 5);
      expect(
        server.commands.map(
          (command) => (command['type'], command['request_id']),
        ),
        [
          ('Reload', first),
          ('Show', first),
          ('Hide', closed),
          ('Reload', reopened),
          ('Show', reopened),
        ],
      );
      expect(
        server.clients,
        hasLength(1),
        reason: 'Refresh must reuse the persistent connection',
      );
      await bridge.dispose();
    },
  );

  test(
    'superseded startup requests cannot refresh or show after the next open',
    () async {
      final starting = Completer<void>();
      final continueStartup = Completer<void>();
      final bridge = LauncherBridge(
        environment: {
          'XDG_RUNTIME_DIR': directory.path,
          'CLARION_LAUNCHER_BIN': '/bin/true',
        },
        spawn: (_, _) async {
          starting.complete();
          await continueStartup.future;
        },
      );
      final old = bridge.beginRequest();
      final oldShow = bridge.show(
        requestId: old,
        theme: 'dark',
        accent: '#ffffff',
        recent: [],
      );
      await starting.future;
      final hidden = bridge.beginRequest();
      final obsoleteHide = bridge.hide(hidden);
      final latest = bridge.beginRequest();
      final latestShow = bridge.show(
        requestId: latest,
        theme: 'dark',
        accent: '#ffffff',
        recent: [],
      );
      await server.start();
      continueStartup.complete();
      await Future.wait([
        oldShow,
        obsoleteHide,
        latestShow,
      ]).timeout(const Duration(seconds: 2));
      await _until(() => server.commands.length == 2);
      expect(
        server.commands.map(
          (command) => (command['type'], command['request_id']),
        ),
        [('Reload', latest), ('Show', latest)],
      );
      await bridge.dispose();
    },
  );
}
