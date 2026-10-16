// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/services/system_information_service.dart';

void main() {
  test('reports actual compositor version and real host data', () async {
    final service = SystemInformationService(
      read: (path) async => switch (path) {
        '/etc/os-release' => 'NAME=Linux\nPRETTY_NAME="Actual Host 1"\n',
        '/proc/cpuinfo' => 'model name : Test CPU\nmodel name : Test CPU\n',
        '/proc/meminfo' => 'MemTotal: 16777216 kB\nMemFree: 123 kB\n',
        _ => throw StateError('unexpected read'),
      },
      run: (executable, args) async {
        expect(executable, 'clarion');
        expect(args, ['msg', '--json', 'version']);
        return '{"cli":"different CLI", "compositor":"active compositor"}';
      },
    );
    final value = await service.read();
    expect(value.operatingSystem, 'Actual Host 1');
    expect(value.processor, 'Test CPU');
    expect(value.memory, '16.0 GB');
    expect(value.compositorVersion, 'active compositor');
  });

  test(
    'missing data never invents a Mac model or substitutes CLI version',
    () async {
      final service = SystemInformationService(
        read: (_) async => throw StateError('unavailable'),
        run: (_, _) async => '{"cli":"cli only"}',
      );
      final value = await service.read();
      expect(value.operatingSystem, '无法读取');
      expect(value.processor, '无法读取');
      expect(value.memory, '无法读取');
      expect(value.compositorVersion, '无法读取');
    },
  );
}
