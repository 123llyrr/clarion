// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/services/system_service.dart';

void main() {
  test(
    'failed scans propagate instead of pretending there are no networks',
    () async {
      final unavailable = SystemService(
        command: (_, _) async => throw StateError('NetworkManager unavailable'),
      );
      await expectLater(unavailable.scanWifiNetworks(), throwsStateError);
      final empty = SystemService(command: (_, _) async => '');
      expect(await empty.scanWifiNetworks(), isEmpty);
    },
  );
  const first = '11111111-1111-1111-1111-111111111111';
  const second = '22222222-2222-2222-2222-222222222222';

  test(
    'saved Wi-Fi matches the SSID setting rather than the profile name',
    () async {
      final calls = <List<String>>[];
      final service = SystemService(
        command: (program, arguments) async {
          expect(program, 'nmcli');
          calls.add(arguments);
          if (arguments.contains('UUID,TYPE')) {
            return '$first:802-11-wireless\n$second:802-3-ethernet';
          }
          expect(arguments.last, first);
          return r'Office:5G\Guest';
        },
      );
      expect(await service.hasSavedWifiConnection(r'Office:5G\Guest'), isTrue);
      expect(calls, hasLength(2));
      expect(
        calls.expand((arguments) => arguments),
        isNot(contains('--show-secrets')),
      );
      expect(
        calls.last,
        containsAllInOrder([
          '802-11-wireless.ssid',
          'connection',
          'show',
          'uuid',
          first,
        ]),
      );
    },
  );

  test(
    'missing profile or unavailable NetworkManager permits entering credentials',
    () async {
      final service = SystemService(
        command: (_, _) async => throw StateError('offline'),
      );
      expect(await service.hasSavedWifiConnection('New network'), isFalse);
    },
  );

  test('profile lookup is bounded to four simultaneous reads', () async {
    var active = 0;
    var maxActive = 0;
    final service = SystemService(
      command: (_, arguments) async {
        if (arguments.contains('UUID,TYPE')) {
          return List.generate(
            9,
            (i) =>
                '${i.toString().padLeft(8, '0')}-1111-1111-1111-111111111111:wifi',
          ).join('\n');
        }
        active++;
        if (active > maxActive) maxActive = active;
        await Future<void>.delayed(Duration.zero);
        active--;
        return 'Unrelated';
      },
    );
    expect(await service.hasSavedWifiConnection('Absent'), isFalse);
    expect(maxActive, 4);
  });

  test(
    'connection arguments preserve literal SSIDs and allow a realistic deadline',
    () async {
      late List<String> arguments;
      final service = SystemService(
        command: (_, args) async {
          arguments = args;
          return '';
        },
      );
      await service.connectWifi(
        r'Office $(literal)',
        password: r'secret $word',
      );
      expect(arguments, [
        '--wait',
        '30',
        'device',
        'wifi',
        'connect',
        r'Office $(literal)',
        'password',
        r'secret $word',
      ]);
    },
  );

  test(
    'connection failures never expose a supplied password in feedback',
    () async {
      final service = SystemService(
        command: (_, args) async =>
            throw StateError('Rejected: ${args.join(' ')}'),
      );
      await expectLater(
        service.connectWifi('Network', password: 'private-secret'),
        throwsA(
          isA<StateError>().having(
            (error) => error.toString(),
            'diagnostic',
            allOf(contains('••••••••'), isNot(contains('private-secret'))),
          ),
        ),
      );
    },
  );
}
