import 'dart:io';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/services/system_service.dart';

void main() {
  late Directory root;
  late SystemService service;
  late List<(String, List<String>)> commands;
  late List<Object> requests;
  late Map<String, dynamic> outputs;
  late String? selected;
  late bool reject;
  late bool disconnected;
  setUp(() async {
    root = await Directory.systemTemp.createTemp('clarion-hdr-brightness-');
    commands = [];
    requests = [];
    selected = 'eDP-1';
    reject = false;
    disconnected = false;
    outputs = {
      'eDP-1': {
        'name': 'eDP-1',
        'logical': {},
        'hdr_active': true,
        'hdr_brightness': 75,
      },
      'DP-1': {'name': 'DP-1', 'logical': {}, 'hdr_active': false},
    };
    service = SystemService(
      powerSupplyRoot: root.path,
      command: (program, args) async {
        commands.add((program, args));
        return program == 'brightnessctl'
            ? 'intel_backlight,backlight,224,56%,400'
            : '';
      },
    );
    service.bindOutputBrightness((request) async {
      requests.add(request);
      if (disconnected) throw StateError('IPC disconnected');
      if (request == 'Outputs') return {'Outputs': outputs};
      return {'OutputConfigChanged': reject ? 'OutputWasMissing' : 'Applied'};
    }, () => selected);
  });
  tearDown(() => root.delete(recursive: true));

  test(
    'HDR slider reads compositor value and writes selected output, never backlight',
    () async {
      expect((await service.read()).brightness, .75);
      commands.clear();
      requests.clear();
      await service.setBrightness(.35);
      expect(commands, isEmpty);
      expect(requests.last, {
        'Output': {
          'output': 'eDP-1',
          'action': {
            'HdrBrightness': {'percent': 35.0},
          },
        },
      });
    },
  );
  test(
    'SDR monitor retains hardware control even when another monitor is HDR',
    () async {
      selected = 'DP-1';
      expect((await service.read()).brightness, .56);
      commands.clear();
      requests.clear();
      await service.setBrightness(.4);
      expect(requests, ['Outputs']);
      expect(commands, hasLength(1));
      expect(commands.single.$1, 'brightnessctl');
      expect(commands.single.$2, ['--class=backlight', 'set', '40%']);
    },
  );
  test('mode switch is rechecked for each write', () async {
    await service.setBrightness(.6);
    outputs['eDP-1']['hdr_active'] = false;
    await service.setBrightness(.5);
    expect(commands, hasLength(1));
    expect(commands.single.$1, 'brightnessctl');
    expect(commands.single.$2, ['--class=backlight', 'set', '50%']);
  });
  test('HDR remains adjustable without a hardware backlight device', () async {
    service = SystemService(
      powerSupplyRoot: root.path,
      command: (_, _) async => throw StateError('no device'),
    );
    service.bindOutputBrightness(
      (_) async => {'Outputs': outputs},
      () => selected,
    );
    expect((await service.read()).brightness, .75);
  });
  test(
    'old compositor HDR support cannot pretend backlight adjusts PQ brightness',
    () async {
      outputs['eDP-1'].remove('hdr_brightness');
      expect((await service.read()).brightness, isNull);
      commands.clear();
      await expectLater(service.setBrightness(.5), throwsStateError);
      expect(commands, isEmpty);
    },
  );
  test('unknown or ambiguous monitor never changes another output', () async {
    for (final output in <String?>['missing', null]) {
      selected = output;
      expect((await service.read()).brightness, isNull);
      commands.clear();
      requests.clear();
      await expectLater(service.setBrightness(.5), throwsStateError);
      expect(commands, isEmpty);
      expect(requests, ['Outputs']);
    }
  });
  test('one connected monitor resolves during bar startup', () async {
    selected = null;
    outputs.remove('DP-1');
    await service.setBrightness(0);
    expect(requests.last, {
      'Output': {
        'output': 'eDP-1',
        'action': {
          'HdrBrightness': {'percent': 1.0},
        },
      },
    });
  });
  test(
    'IPC disconnection and rejected write are surfaced without backlight fallback',
    () async {
      disconnected = true;
      expect((await service.read()).brightness, isNull);
      commands.clear();
      await expectLater(service.setBrightness(.5), throwsStateError);
      expect(commands, isEmpty);
      disconnected = false;
      reject = true;
      await expectLater(service.setBrightness(.5), throwsStateError);
      expect(commands, isEmpty);
    },
  );
  test('invalid values are rejected before IPC or hardware writes', () async {
    for (final value in [
      double.nan,
      double.infinity,
      double.negativeInfinity,
    ]) {
      await expectLater(service.setBrightness(value), throwsArgumentError);
    }
    expect(requests, isEmpty);
    expect(commands, isEmpty);
  });
}
