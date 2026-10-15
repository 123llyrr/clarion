// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter_test/flutter_test.dart';
import 'package:realand_shell/services/application_launcher.dart';
import 'package:realand_shell/services/system_service.dart';

void main() {
  test(
    'event-driven audio reads invoke only the existing volume backend',
    () async {
      final commands = <(String, List<String>)>[];
      final system = SystemService(
        command: (program, arguments) async {
          commands.add((program, arguments));
          return 'Volume: 0.37 [MUTED]';
        },
      );
      final value = await system.readAudio();
      expect(value.volume, .37);
      expect(value.muted, isTrue);
      expect(commands, hasLength(1));
      expect(commands.single.$1, 'wpctl');
      expect(commands.single.$2, ['get-volume', '@DEFAULT_AUDIO_SINK@']);
    },
  );

  test(
    'audio values stay finite and bounded and missing output is unavailable',
    () {
      expect(SystemAudioSnapshot.parse('Volume: 1.5').volume, 1);
      expect(SystemAudioSnapshot.parse('Volume: 0.0').volume, 0);
      expect(SystemAudioSnapshot.parse('Volume: NaN').volume, isNull);
      expect(SystemAudioSnapshot.parse('no default sink').volume, isNull);
    },
  );

  test(
    'sound preferences launches the real mixer through the app launcher',
    () async {
      final commands = <(String, List<String>)>[];
      final launcher = ApplicationLauncher(
        scopeProbe: () async => const ApplicationScope(),
        command: (program, arguments) async {
          commands.add((program, arguments));
          return '';
        },
      );
      final system = SystemService(applicationLauncher: launcher);
      await system.openSoundPreferences();
      expect(commands.single.$1, 'systemd-run');
      expect(
        commands.single.$2,
        const ApplicationScope().arguments('pavucontrol', const []),
      );
    },
  );
}
