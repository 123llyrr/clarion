// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/services/system_service.dart';
import 'package:clarion_shell/services/application_launcher.dart';

Future<Directory> _powerSupplyRoot() async {
  final root = await Directory.systemTemp.createTemp('clarion-power-parsing-');
  addTearDown(() => root.delete(recursive: true));
  return root;
}

Future<void> _battery(
  Directory root,
  String capacity, {
  String name = 'BAT0',
  String? scope,
  String status = 'Discharging',
}) async {
  final device = Directory('${root.path}/$name');
  await device.create();
  await File('${device.path}/type').writeAsString('Battery\n');
  if (scope != null) {
    await File('${device.path}/scope').writeAsString('$scope\n');
  }
  await File('${device.path}/capacity').writeAsString('$capacity\n');
  await File('${device.path}/status').writeAsString('$status\n');
}

void main() {
  group('Dock folder actions use the GUI location handler', () {
    test('home and trash are passed to GIO as literal locations', () async {
      final commands = <(String, List<String>)>[];
      const home = r'/home/User name/$(do-not-run)';
      Future<String> command(String program, List<String> arguments) async {
        commands.add((program, arguments));
        return '';
      }

      final service = SystemService(
        environment: {'HOME': home},
        command: command,
        applicationLauncher: ApplicationLauncher(
          command: command,
          scopeProbe: () async => null,
        ),
      );
      await service.openHomeFolder();
      await service.openTrash();
      expect(commands, hasLength(2));
      expect(commands[0].$1, 'gio');
      expect(commands[0].$2, ['open', '--', home]);
      expect(commands[1].$1, 'gio');
      expect(commands[1].$2, ['open', '--', 'trash:///']);
    });

    test(
      'unavailable home cannot launch an accidental relative path',
      () async {
        var commands = 0;
        final service = SystemService(
          environment: {'HOME': 'relative-home'},
          command: (_, _) async {
            commands++;
            return '';
          },
        );
        await expectLater(service.openHomeFolder(), throwsStateError);
        expect(commands, 0);
      },
    );
  });

  group('NetworkManager escaped connection fields', () {
    test('a separator after an escaped backslash remains a separator', () {
      expect(activeNetworkName(r'Studio\\:802-11-wireless'), 'Studio\\');
      expect(activeNetworkName(r'Office\\\\:ethernet'), 'Office\\\\');
    });

    test('escaped colons and backslashes decode in their original order', () {
      expect(activeNetworkName(r'Studio\:WiFi:802-11-wireless'), 'Studio:WiFi');
      expect(activeNetworkName(r'Folder\\\:WiFi:wifi'), r'Folder\:WiFi');
      expect(activeNetworkName(r'\:WiFi:ethernet'), ':WiFi');
      expect(activeNetworkName(r'C\:\\Office\\:802-3-ethernet'), r'C:\Office\');
    });

    test('unsupported and incomplete entries do not hide a later network', () {
      expect(
        activeNetworkName('Meta:tun\nMalformed\n${r'Studio\\:wifi'}'),
        'Studio\\',
      );
      expect(activeNetworkName(r'Broken\:wifi'), isEmpty);
    });
  });

  group('Non-finite hardware readings', () {
    for (final value in ['NaN', 'Infinity', '-Infinity']) {
      test('brightness current $value stays unavailable', () async {
        final root = await _powerSupplyRoot();
        final service = SystemService(
          powerSupplyRoot: root.path,
          command: (program, _) async => program == 'brightnessctl'
              ? 'intel_backlight,backlight,$value,0%,1000'
              : '',
        );
        expect((await service.read()).brightness, isNull);
      });

      test('brightness maximum $value stays unavailable', () async {
        final root = await _powerSupplyRoot();
        final service = SystemService(
          powerSupplyRoot: root.path,
          command: (program, _) async => program == 'brightnessctl'
              ? 'intel_backlight,backlight,350,35%,$value'
              : '',
        );
        expect((await service.read()).brightness, isNull);
      });

      test('battery capacity $value stays unavailable', () async {
        final root = await _powerSupplyRoot();
        await _battery(root, value);
        final (capacity, charging) = await readBattery(root.path);
        expect(capacity, isNull);
        expect(charging, isFalse);
      });
    }

    test(
      'an invalid brightness row does not hide a later valid device',
      () async {
        final root = await _powerSupplyRoot();
        final service = SystemService(
          powerSupplyRoot: root.path,
          command: (program, _) async => program == 'brightnessctl'
              ? 'bad,backlight,NaN,0%,1000\nintel_backlight,backlight,350,35%,1000'
              : '',
        );
        expect((await service.read()).brightness, .35);
      },
    );
  });

  group('System batteries exclude peripheral power supplies', () {
    test('a charging peripheral cannot change the system battery', () async {
      final root = await _powerSupplyRoot();
      await _battery(root, '80');
      await _battery(
        root,
        '20',
        name: 'hid-mouse-battery',
        scope: 'Device',
        status: 'Charging',
      );
      expect(await readBattery(root.path), (.8, false));
    });

    test('peripherals alone do not invent a system battery', () async {
      final root = await _powerSupplyRoot();
      await _battery(
        root,
        '90',
        name: 'hid-keyboard-battery',
        scope: 'Device',
        status: 'Full',
      );
      expect(await readBattery(root.path), (null, false));
    });

    test(
      'an explicit system battery still supplies capacity and charge',
      () async {
        final root = await _powerSupplyRoot();
        await _battery(root, '60', scope: 'System', status: 'Charging');
        expect(await readBattery(root.path), (.6, true));
      },
    );
  });

  group('System setter input validation', () {
    for (final value in [
      double.nan,
      double.infinity,
      double.negativeInfinity,
    ]) {
      test('volume $value is rejected before executing a command', () async {
        var commands = 0;
        final service = SystemService(
          command: (_, _) async {
            commands++;
            return '';
          },
        );
        await expectLater(service.setVolume(value), throwsArgumentError);
        expect(commands, 0);
      });

      test(
        'brightness $value is rejected before executing a command',
        () async {
          var commands = 0;
          final service = SystemService(
            command: (_, _) async {
              commands++;
              return '';
            },
          );
          await expectLater(service.setBrightness(value), throwsArgumentError);
          expect(commands, 0);
        },
      );
    }

    test(
      'finite values keep the existing bounds and visible minimum',
      () async {
        final commands = <(String, List<String>)>[];
        final service = SystemService(
          command: (program, arguments) async {
            commands.add((program, arguments));
            return '';
          },
        );
        await service.setVolume(2);
        await service.setBrightness(0);
        expect(commands, hasLength(2));
        expect(commands[0].$1, 'wpctl');
        expect(commands[0].$2, [
          'set-volume',
          '@DEFAULT_AUDIO_SINK@',
          '1.000',
          '--limit',
          '1.0',
        ]);
        expect(commands[1].$1, 'brightnessctl');
        expect(commands[1].$2, ['--class=backlight', 'set', '1%']);
      },
    );
  });
}
