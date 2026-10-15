// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'package:dbus/dbus.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:realand_shell/services/desktop_apps.dart';
import 'package:realand_shell/services/notification_service.dart';
import 'package:realand_shell/services/realand_ipc.dart';
import 'package:realand_shell/services/system_service.dart';

void main() {
  group('Bounded IPC', () {
    test('dispose closes an outstanding action connection', () async {
      final fixture = await _IpcFixture.create();
      addTearDown(fixture.close);
      final ready = Completer<void>();
      fixture.server.listen((socket) {
        fixture.clients.add(socket);
        socket.listen((_) {
          if (!ready.isCompleted) ready.complete();
        });
      });
      final ipc = RealandIpc(socketPath: fixture.path);
      addTearDown(ipc.dispose);
      final response = ipc.request('Version');
      final expectation = expectLater(response, throwsA(isA<StateError>()));
      await ready.future;
      ipc.dispose();
      await expectation.timeout(const Duration(milliseconds: 300));
    });
    test('reassembles split UTF-8 and several lines per chunk', () async {
      final bytes = utf8.encode('中文\n{}\n');
      expect(
        await boundedJsonLines(
          Stream.fromIterable([
            bytes.sublist(0, 2),
            bytes.sublist(2, 5),
            bytes.sublist(5),
          ]),
        ).toList(),
        ['中文', '{}'],
      );
    });
    test('size counts newline and truncated or invalid UTF-8 fails', () async {
      expect(
        await boundedJsonLines(Stream.value([1, 2, 10]), maximum: 3).toList(),
        ['\u0001\u0002'],
      );
      await expectLater(
        boundedJsonLines(Stream.value([1, 2, 3, 10]), maximum: 3).toList(),
        throwsFormatException,
      );
      await expectLater(
        boundedJsonLines(Stream.value([1, 2]), maximum: 3).toList(),
        throwsFormatException,
      );
      await expectLater(
        boundedJsonLines(Stream.value([0xff, 10])).toList(),
        throwsFormatException,
      );
    });
    test(
      'requests have their own connection and return explicit errors',
      () async {
        final fixture = await _IpcFixture.create();
        addTearDown(fixture.close);
        final requests = <dynamic>[];
        fixture.server.listen((socket) {
          fixture.clients.add(socket);
          boundedJsonLines(socket).listen((line) {
            requests.add(jsonDecode(line));
            socket.write(
              line.contains('ReturnError')
                  ? '{"Err":"denied"}\n'
                  : '{"Ok":"Handled"}\n',
            );
          });
        });
        final ipc = RealandIpc(socketPath: fixture.path);
        addTearDown(ipc.dispose);
        await ipc.action('FocusWorkspace', {
          'reference': {'Id': 9001},
        });
        await expectLater(ipc.request('ReturnError'), throwsStateError);
        expect(requests.first, {
          'Action': {
            'FocusWorkspace': {
              'reference': {'Id': 9001},
            },
          },
        });
        expect(fixture.clients.length, 2);
      },
    );
    test(
      'handshake timeout reconnects but accepted idle stream remains open',
      () async {
        final fixture = await _IpcFixture.create();
        addTearDown(fixture.close);
        var connections = 0;
        fixture.server.listen((socket) {
          fixture.clients.add(socket);
          connections++;
          boundedJsonLines(socket).listen((_) {
            if (connections > 1) {
              socket.write(
                '{"Ok":"Handled"}\n{"WorkspacesChanged":{"workspaces":[]}}\n',
              );
            }
          });
        });
        final ipc = RealandIpc(
          socketPath: fixture.path,
          timeout: const Duration(milliseconds: 60),
        );
        addTearDown(ipc.dispose);
        final accepted = ipc.connections.firstWhere((v) => v);
        ipc.start();
        await accepted.timeout(const Duration(seconds: 2));
        await Future<void>.delayed(const Duration(milliseconds: 160));
        expect(connections, 2);
        ipc.dispose();
        await Future<void>.delayed(const Duration(milliseconds: 400));
        expect(connections, 2);
      },
    );
    test('action response timeout is bounded', () async {
      final fixture = await _IpcFixture.create();
      addTearDown(fixture.close);
      fixture.server.listen((socket) {
        fixture.clients.add(socket);
        socket.listen((_) {});
      });
      final ipc = RealandIpc(
        socketPath: fixture.path,
        timeout: const Duration(milliseconds: 40),
      );
      addTearDown(ipc.dispose);
      await expectLater(
        ipc.request('Version'),
        throwsA(isA<TimeoutException>()),
      );
    });
  });

  test(
    'XDG precedence masks hidden entries and desktop visibility is honored',
    () async {
      final root = await Directory.systemTemp.createTemp(
        'realand-desktop-test-',
      );
      addTearDown(() => root.delete(recursive: true));
      final user = Directory('${root.path}/user/applications');
      final system = Directory('${root.path}/system/applications');
      await user.create(recursive: true);
      await system.create(recursive: true);
      Future<void> entry(
        Directory dir,
        String id,
        String content,
      ) => File('${dir.path}/$id.desktop')
          .writeAsString(
            '[Desktop Entry]\nType=Application\nName=$id\nExec=/bin/true --literal\n$content',
          )
          .then((_) {});
      await entry(user, 'masked', 'Hidden=true\n');
      await entry(system, 'masked', '');
      await entry(system, 'hidden', 'NoDisplay=true\n');
      await entry(system, 'other', 'OnlyShowIn=GNOME;\n');
      await entry(
        system,
        'realand',
        'OnlyShowIn=realand;\nName[zh_CN]=空间应用\nComment=Test\\sapp\n',
      );
      final service = DesktopApps(
        environment: {
          'XDG_DATA_HOME': '${root.path}/user',
          'XDG_DATA_DIRS': '${root.path}/system',
          'XDG_CURRENT_DESKTOP': 'realand',
          'LANG': 'zh_CN.UTF-8',
        },
      );
      final apps = await service.load();
      expect(apps.map((a) => a.id), ['realand.desktop']);
      expect(apps.single.name, '空间应用');
      expect(apps.single.description, 'Test app');
    },
  );
  test('system parsing and unsupported hardware stay truthful', () async {
    final root = await Directory.systemTemp.createTemp('realand-system-test-');
    addTearDown(() => root.delete(recursive: true));
    final battery = Directory('${root.path}/BAT0');
    await battery.create();
    await File('${battery.path}/type').writeAsString('Battery\n');
    await File('${battery.path}/capacity').writeAsString('85\n');
    await File('${battery.path}/status').writeAsString('Charging\n');
    final service = SystemService(
      powerSupplyRoot: root.path,
      command: (exe, args) async => switch (exe) {
        'wpctl' => 'Volume: 0.65 [MUTED]',
        'brightnessctl' => 'intel_backlight,backlight,350,35%,1000',
        'bluetoothctl' => 'Controller AA:BB\n\tPowered: yes',
        'nmcli' =>
          args.first == 'radio' ? 'disabled' : r'Studio\:WiFi:802-11-wireless',
        _ => '',
      },
    );
    final snapshot = await service.read();
    expect(snapshot.volume, .65);
    expect(snapshot.muted, true);
    expect(snapshot.brightness, .35);
    expect(snapshot.batteryFraction, .85);
    expect(snapshot.charging, true);
    expect(snapshot.networkName, 'Studio:WiFi');
    expect(snapshot.wifiEnabled, false);
    final missing = await SystemService(
      powerSupplyRoot: '${root.path}/missing',
      command: (_, _) async => throw const ProcessException('missing', []),
    ).read();
    expect(missing.volume, null);
    expect(missing.brightness, null);
    expect(missing.batteryFraction, null);
    expect(missing.wifiEnabled, null);
    expect(missing.bluetoothEnabled, null);
  });
  test(
    'commands cannot block forever when a descendant holds their pipes',
    () async {
      final watch = Stopwatch()..start();
      expect(
        await runSystemCommand('python3', [
          '-c',
          "import subprocess; subprocess.Popen(['python3','-c','import time; time.sleep(2)']); print('ok')",
        ]),
        'ok',
      );
      expect(watch.elapsed, lessThan(const Duration(seconds: 2)));
    },
  );
  test('silent command ignoring SIGTERM is killed at the deadline', () async {
    final watch = Stopwatch()..start();
    await expectLater(
      runSystemCommand('python3', [
        '-c',
        'import signal,time; signal.signal(signal.SIGTERM,signal.SIG_IGN); time.sleep(30)',
      ]),
      throwsA(isA<TimeoutException>()),
    );
    expect(watch.elapsed, lessThan(const Duration(seconds: 7)));
  }, timeout: const Timeout(Duration(seconds: 10)));

  group('Notifications on isolated D-Bus', () {
    late Directory directory;
    late DBusServer bus;
    late DBusAddress address;
    late DBusClient caller;
    setUp(() async {
      directory = await Directory.systemTemp.createTemp('realand-dbus-test-');
      bus = DBusServer();
      address = await bus.listenAddress(
        DBusAddress.unix(path: '${directory.path}/bus'),
      );
      caller = DBusClient(address);
    });
    tearDown(() async {
      await caller.close();
      await bus.close();
      await directory.delete(recursive: true);
    });
    test('does not replace an existing notification owner', () async {
      final owner = DBusClient(address);
      addTearDown(owner.close);
      expect(
        await owner.requestName(
          'org.freedesktop.Notifications',
          flags: {DBusRequestNameFlag.allowReplacement},
        ),
        DBusRequestNameReply.primaryOwner,
      );
      final service = NotificationService(bus: DBusClient(address));
      addTearDown(service.dispose);
      expect(await service.start(), false);
      expect(
        await caller.getNameOwner('org.freedesktop.Notifications'),
        owner.uniqueName,
      );
    });
    test(
      'receives, replaces, expires and bounds real protocol messages',
      () async {
        final service = NotificationService(
          bus: DBusClient(address),
          historyLimit: 2,
        );
        addTearDown(service.dispose);
        expect(await service.start(), true);
        final object = DBusRemoteObject(
          caller,
          name: 'org.freedesktop.Notifications',
          path: DBusObjectPath('/org/freedesktop/Notifications'),
        );
        Future<int> notify({
          int replace = 0,
          int timeout = 0,
          String body = '',
          List<String> actions = const [],
        }) async {
          final reply = await object
              .callMethod('org.freedesktop.Notifications', 'Notify', [
                DBusString('Test'),
                DBusUint32(replace),
                DBusString(''),
                DBusString('Hello'),
                DBusString(body),
                DBusArray.string(actions),
                DBusDict.stringVariant({}),
                DBusInt32(timeout),
              ], replySignature: DBusSignature('u'));
          return reply.returnValues.single.asUint32();
        }

        final id = await notify(
          body: 'x' * 10000,
          actions: List.generate(50, (i) => '$i'),
        );
        expect(service.notifications.single.body.length, 8192);
        expect(service.notifications.single.actions.length, 16);
        expect(
          await notify(replace: id, timeout: 30, actions: ['open', 'Open']),
          id,
        );
        await Future<void>.delayed(const Duration(milliseconds: 80));
        expect(service.notifications.single.actions, isEmpty);
        await notify();
        await notify();
        expect(service.notifications.length, 2);
        await service.clear();
        expect(service.notifications, isEmpty);
      },
    );
  });
}

class _IpcFixture {
  _IpcFixture(this.directory, this.server, this.path);
  final Directory directory;
  final ServerSocket server;
  final String path;
  final clients = <Socket>[];
  static Future<_IpcFixture> create() async {
    final directory = await Directory.systemTemp.createTemp(
      'realand-ipc-test-',
    );
    final path = '${directory.path}/ipc';
    final server = await ServerSocket.bind(
      InternetAddress(path, type: InternetAddressType.unix),
      0,
    );
    return _IpcFixture(directory, server, path);
  }

  Future<void> close() async {
    for (final client in clients) {
      client.destroy();
    }
    await server.close();
    await directory.delete(recursive: true);
  }
}
