// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/services/system_information_service.dart';

const _outputsJson = '''
{
  "eDP-1": {
    "name": "eDP-1",
    "make": "Samsung Display Corp.",
    "model": "0x4173",
    "serial": null,
    "physical_size": [340, 220],
    "modes": [
      {"width": 3840, "height": 2400, "refresh_rate": 60000, "is_preferred": true},
      {"width": 1920, "height": 1200, "refresh_rate": 59990, "is_preferred": false}
    ],
    "current_mode": 0,
    "is_custom_mode": false,
    "vrr_supported": true,
    "vrr_enabled": true,
    "logical": {"x": 0, "y": 0, "width": 1920, "height": 1200, "scale": 2.0},
    "max_bpc": 10,
    "hdr_active": false
  },
  "DP-2": {
    "name": "DP-2",
    "make": "",
    "model": "",
    "physical_size": [600, 340],
    "modes": [{"width": 2560, "height": 1440, "refresh_rate": 144000}],
    "current_mode": 0,
    "vrr_enabled": false,
    "logical": {"scale": 1.25},
    "hdr_active": true
  }
}
''';

const _dfOutput = '''
1B-blocks       Used    Avail
999123456789 678123456789 320999999999
''';

void main() {
  test('reports actual compositor version and real host data', () async {
    final service = SystemInformationService(
      read: (path) async => switch (path) {
        '/etc/os-release' => 'NAME=Linux\nPRETTY_NAME="Actual Host 1"\n',
        '/proc/cpuinfo' => 'model name : Test CPU\nmodel name : Test CPU\n',
        '/proc/meminfo' => 'MemTotal: 16777216 kB\nMemFree: 123 kB\n',
        _ => throw StateError('unexpected read'),
      },
      run: (executable, args) async => switch (args.join(' ')) {
        'msg --json version' => '{"cli":"x","compositor":"active compositor"}',
        'msg --json outputs' => _outputsJson,
        _ => throw StateError('unexpected command'),
      },
    );
    final value = await service.read();
    expect(value.operatingSystem, 'Actual Host 1');
    expect(value.processor, 'Test CPU');
    expect(value.memory, '16.0 GB');
    expect(value.compositorVersion, 'active compositor');
  });

  test('parses connected outputs, modes and refresh rates', () async {
    final service = SystemInformationService(
      read: (_) async => throw StateError('unavailable'),
      run: (executable, args) async => switch (args.join(' ')) {
        'msg --json version' => '{"compositor":"0.1.0-test"}',
        'msg --json outputs' => _outputsJson,
        _ => throw StateError('unexpected command'),
      },
    );
    final value = await service.read();
    expect(value.displays, hasLength(2));
    final panel = value.displays.first;
    expect(panel.connector, 'eDP-1');
    expect(panel.name, 'Samsung Display Corp. 0x4173');
    expect(panel.resolution, '3840 × 2400');
    expect(panel.refresh, '60 Hz');
    expect(panel.scale, 2);
    expect(panel.physical, '340 × 220 mm');
    expect(panel.vrr, isTrue);
    expect(panel.hdr, isFalse);
    final external = value.displays[1];
    expect(external.name, 'DP-2');
    expect(external.refresh, '144 Hz');
    expect(external.scale, 1.25);
    expect(external.hdr, isTrue);
  });

  test('parses df into total and available gigabytes', () async {
    final service = SystemInformationService(
      read: (_) async => throw StateError('unavailable'),
      run: (executable, args) async => switch (args.join(' ')) {
        'msg --json version' => '{"compositor":"0.1.0-test"}',
        '-B1 --output=size,used,avail /' => _dfOutput,
        _ => throw StateError('unexpected command'),
      },
    );
    final value = await service.read();
    expect(value.storageTotalGb, closeTo(999.12, .01));
    expect(value.storageFreeGb, closeTo(321.0, .01));
  });

  test('missing data never invents hardware or fakes measurements', () async {
    final service = SystemInformationService(
      read: (_) async => throw StateError('unavailable'),
      run: (_, _) async => throw StateError('unavailable'),
    );
    final value = await service.read();
    expect(value.operatingSystem, '无法读取');
    expect(value.processor, '无法读取');
    expect(value.memory, '无法读取');
    expect(value.compositorVersion, '无法读取');
    expect(value.displays, isEmpty);
    expect(value.storageTotalGb, 0);
    expect(value.storageFreeGb, 0);
  });
}
