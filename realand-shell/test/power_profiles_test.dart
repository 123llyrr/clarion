// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:io';

import 'package:dbus/dbus.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:realand_shell/services/power_profiles_service.dart';
import 'package:realand_shell/ui/power_mode_card.dart';

const _modern = 'org.freedesktop.UPower.PowerProfiles';
const _legacy = 'net.hadess.PowerProfiles';

void main() {
  group('power profiles on isolated D-Bus', () {
    late Directory directory;
    late DBusServer server;
    late DBusAddress address;
    final clients = <DBusClient>[];
    final services = <PowerProfilesService>[];

    setUp(() async {
      directory = await Directory.systemTemp.createTemp('realand-power-');
      server = DBusServer();
      address = await server.listenAddress(
        DBusAddress.unix(path: '${directory.path}/bus'),
      );
    });

    tearDown(() async {
      for (final service in services) {
        service.dispose();
      }
      services.clear();
      for (final client in clients) {
        await client.close();
      }
      clients.clear();
      await server.close();
      await directory.delete(recursive: true);
    });

    PowerProfilesService service({Duration? timeout}) {
      final result = PowerProfilesService(
        bus: DBusClient(address),
        requestTimeout: timeout ?? const Duration(seconds: 2),
      );
      services.add(result);
      return result;
    }

    Future<(DBusClient, _Provider)> provider({
      String interface = _modern,
      List<String> profiles = const ['power-saver', 'balanced', 'performance'],
    }) async {
      final owner = DBusClient(address);
      clients.add(owner);
      final object = _Provider(interface, profiles: profiles);
      await owner.registerObject(object);
      await owner.requestName(interface);
      return (owner, object);
    }

    test(
      'reads actual supported profiles and sets the system property',
      () async {
        final (_, object) = await provider();
        final power = service();
        await power.start();
        expect(power.available, isTrue);
        expect(power.loading, isFalse);
        expect(power.profiles.map((profile) => profile.id), [
          'power-saver',
          'balanced',
          'performance',
        ]);
        expect(power.activeProfile, 'balanced');
        await power.setProfile('power-saver');
        expect(object.writes, ['power-saver']);
        expect(power.activeProfile, 'power-saver');
        expect(power.busy, isFalse);
        expect(power.error, isNull);
        await power.setProfile('power-saver');
        expect(object.writes, hasLength(1));
      },
    );

    test(
      'falls back to legacy API and filters unsafe or unsupported profiles',
      () async {
        final (_, object) = await provider(
          interface: _legacy,
          profiles: ['balanced', 'power-saver', 'unknown', 'balanced'],
        );
        final power = service();
        await power.start();
        expect(power.available, isTrue);
        expect(power.profiles.map((profile) => profile.id), [
          'power-saver',
          'balanced',
        ]);
        await power.setProfile('performance');
        await power.setProfile('unknown');
        expect(object.writes, isEmpty);
        expect(power.activeProfile, 'balanced');
        expect(power.error, contains('不可用'));
      },
    );

    test('tracks external changes and invalidated properties', () async {
      final (_, object) = await provider();
      final power = service();
      await power.start();
      object.active = 'performance';
      object.degraded = 'high-operating-temperature';
      await object.emitPropertiesChanged(
        _modern,
        invalidatedProperties: ['ActiveProfile', 'PerformanceDegraded'],
      );
      await _wait(() => power.activeProfile == 'performance' && !power.loading);
      expect(power.degradedReason, contains('温度'));
      object.degraded = 'future-reason';
      await object.emitPropertiesChanged(
        _modern,
        changedProperties: {
          'PerformanceDegraded': const DBusString('future-reason'),
        },
      );
      await _wait(() => power.performanceDegraded == 'future-reason');
      expect(power.degradedReason, contains('保护设备'));
    });

    test(
      'property change while GetAll is pending re-reads its snapshot',
      () async {
        final (_, object) = await provider();
        object.readGate = Completer<void>();
        final power = service();
        final starting = power.start();
        await _wait(() => object.reads > 0);
        object.active = 'power-saver';
        await object.emitPropertiesChanged(
          _modern,
          changedProperties: {'ActiveProfile': const DBusString('power-saver')},
        );
        object.readGate!.complete();
        await starting;
        await _wait(
          () => power.activeProfile == 'power-saver' && !power.loading,
        );
        expect(object.reads, greaterThan(1));
      },
    );

    test(
      'unavailable service recovers after start and owner exit/restart',
      () async {
        final power = service();
        await power.start();
        expect(power.available, isFalse);
        expect(power.profiles, isEmpty);
        expect(power.error, '系统未提供电源模式服务');
        final (owner, _) = await provider();
        await _wait(() => power.available);
        await owner.close();
        await _wait(() => !power.available && !power.loading);
        expect(power.activeProfile, isNull);
        final (_, restarted) = await provider(
          profiles: ['power-saver', 'balanced'],
        );
        await _wait(() => power.available);
        expect(power.profiles, hasLength(2));
        await power.setProfile('power-saver');
        expect(restarted.writes, ['power-saver']);
      },
    );

    test(
      'permission rejection preserves active mode and clears busy state',
      () async {
        final (_, object) = await provider();
        object.deny = true;
        final power = service();
        await power.start();
        await power.setProfile('performance');
        expect(power.activeProfile, 'balanced');
        expect(power.error, contains('权限'));
        expect(power.busy, isFalse);
        object.deny = false;
        await power.setProfile('performance');
        expect(power.activeProfile, 'performance');
        expect(power.error, isNull);
      },
    );

    test(
      'write timeout does not optimistically change the selected profile',
      () async {
        final (_, object) = await provider();
        final power = service(timeout: const Duration(milliseconds: 250));
        await power.start();
        object.writeGate = Completer<void>();
        final changing = power.setProfile('performance');
        await _wait(() => object.writes.isNotEmpty);
        expect(power.busy, isTrue);
        await power.setProfile('power-saver');
        expect(object.writes, ['performance']);
        await changing;
        expect(power.busy, isFalse);
        expect(power.activeProfile, 'balanced');
        expect(power.error, contains('超时'));
        object.writeGate!.complete();
        await _wait(() => object.active == 'performance');
        await power.refresh();
        expect(power.activeProfile, 'performance');
      },
    );

    test('malformed provider data becomes unavailable', () async {
      final (_, object) = await provider();
      object.malformedProfiles = true;
      final power = service();
      await power.start();
      expect(power.available, isFalse);
      expect(power.profiles, isEmpty);
      expect(power.activeProfile, isNull);
      object.malformedProfiles = false;
      await power.refresh();
      expect(power.available, isTrue);
    });

    test(
      'disposed pending read cannot notify or restore stale state',
      () async {
        final (_, object) = await provider();
        object.readGate = Completer<void>();
        final power = service();
        var notifications = 0;
        power.addListener(() => notifications++);
        final starting = power.start();
        await _wait(() => object.reads > 0);
        power.dispose();
        final count = notifications;
        object.readGate!.complete();
        await starting;
        expect(notifications, count);
        expect(power.available, isFalse);
        await power.refresh();
        await power.setProfile('performance');
        expect(object.writes, isEmpty);
      },
    );

    test('owner change during a read discards its old snapshot', () async {
      final (owner, old) = await provider();
      old.readGate = Completer<void>();
      old.active = 'performance';
      final power = service();
      final starting = power.start();
      await _wait(() => old.reads > 0);
      await owner.releaseName(_modern);
      final (_, replacement) = await provider(
        profiles: ['power-saver', 'balanced'],
      );
      replacement.active = 'power-saver';
      old.readGate!.complete();
      await starting;
      await _wait(() => power.activeProfile == 'power-saver' && !power.loading);
      expect(power.profiles, hasLength(2));
      expect(old.writes, isEmpty);
    });
  });

  test(
    'demo never connects to the supplied bus or writes system settings',
    () async {
      final bus = DBusClient(
        DBusAddress.unix(path: '/nonexistent/realand-demo-bus'),
      );
      final power = PowerProfilesService(demo: true, bus: bus);
      addTearDown(power.dispose);
      addTearDown(bus.close);
      await power.start();
      await power.setProfile('performance');
      expect(power.available, isTrue);
      expect(power.activeProfile, 'performance');
      expect(bus.uniqueName, isEmpty);
    },
  );

  for (final width in [180.0, 400.0]) {
    testWidgets('Material power card adapts to $width logical pixels', (
      tester,
    ) async {
      final power = PowerProfilesService(demo: true);
      addTearDown(power.dispose);
      tester.view.physicalSize = Size(width, 650);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: SingleChildScrollView(child: PowerModeCard(service: power)),
          ),
        ),
      );
      expect(find.text('电源模式'), findsOneWidget);
      expect(find.text('省电'), findsOneWidget);
      expect(find.text('均衡'), findsOneWidget);
      expect(find.text('性能'), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('power-mode-performance')));
      await tester.pumpAndSettle();
      expect(power.activeProfile, 'performance');
      expect(
        tester
            .widget<FilterChip>(
              find.byKey(const ValueKey('power-mode-performance')),
            )
            .selected,
        isTrue,
      );
      expect(tester.takeException(), isNull);
    });
  }
}

Future<void> _wait(bool Function() condition) async {
  final deadline = DateTime.now().add(const Duration(seconds: 3));
  while (!condition()) {
    if (DateTime.now().isAfter(deadline)) {
      throw TimeoutException('power fixture wait');
    }
    await Future<void>.delayed(const Duration(milliseconds: 5));
  }
}

class _Provider extends DBusObject {
  _Provider(this.interface, {required this.profiles})
    : super(
        DBusObjectPath(
          interface == _modern
              ? '/org/freedesktop/UPower/PowerProfiles'
              : '/net/hadess/PowerProfiles',
        ),
      );

  final String interface;
  final List<String> profiles;
  String active = 'balanced';
  String degraded = '';
  bool deny = false;
  bool malformedProfiles = false;
  int reads = 0;
  final writes = <String>[];
  Completer<void>? readGate;
  Completer<void>? writeGate;

  @override
  Future<DBusMethodResponse> getAllProperties(String requestedInterface) async {
    if (requestedInterface != interface) {
      return DBusMethodErrorResponse.unknownInterface();
    }
    reads++;
    final snapshot = DBusGetAllPropertiesResponse({
      'ActiveProfile': DBusString(active),
      'PerformanceDegraded': DBusString(degraded),
      'Profiles': malformedProfiles
          ? const DBusString('invalid')
          : DBusArray(DBusSignature('a{sv}'), [
              for (final profile in profiles)
                DBusDict.stringVariant({
                  'Profile': DBusString(profile),
                  'Driver': const DBusString('fixture'),
                }),
            ]),
    });
    await readGate?.future;
    return snapshot;
  }

  @override
  Future<DBusMethodResponse> setProperty(
    String requestedInterface,
    String name,
    DBusValue value,
  ) async {
    if (requestedInterface != interface ||
        name != 'ActiveProfile' ||
        value is! DBusString) {
      return DBusMethodErrorResponse.invalidArgs();
    }
    if (deny) {
      return DBusMethodErrorResponse('org.freedesktop.DBus.Error.AccessDenied');
    }
    writes.add(value.value);
    await writeGate?.future;
    active = value.value;
    await emitPropertiesChanged(
      interface,
      changedProperties: {'ActiveProfile': DBusString(active)},
    );
    return DBusMethodSuccessResponse();
  }
}
