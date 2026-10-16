// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/services/sound_service.dart';

void main() {
  group('SoundService', () {
    test('plays trash empty sound event', () async {
      final calls = <(String, List<String>)>[];
      final service = SoundService(
        command: (executable, arguments) async {
          calls.add((executable, arguments));
          return '';
        },
      );

      await service.playTrashEmpty();

      expect(calls, hasLength(1));
      expect(calls.first.$1, 'canberra-gtk-play');
      expect(calls.first.$2, ['-i', 'trash-empty']);
    });

    test('plays camera shutter sound event', () async {
      final calls = <(String, List<String>)>[];
      final service = SoundService(
        command: (executable, arguments) async {
          calls.add((executable, arguments));
          return '';
        },
      );

      await service.playCameraShutter();

      expect(calls, hasLength(1));
      expect(calls.first.$1, 'canberra-gtk-play');
      expect(calls.first.$2, ['-i', 'camera-shutter']);
    });

    test('plays system alert sound event', () async {
      final calls = <(String, List<String>)>[];
      final service = SoundService(
        command: (executable, arguments) async {
          calls.add((executable, arguments));
          return '';
        },
      );

      await service.playAlert();

      expect(calls, hasLength(1));
      expect(calls.first.$1, 'canberra-gtk-play');
      expect(calls.first.$2, ['-i', 'bell']);
    });

    test('throttles rapid volume ticks', () async {
      final calls = <(String, List<String>)>[];
      final service = SoundService(
        command: (executable, arguments) async {
          calls.add((executable, arguments));
          return '';
        },
      );

      await service.playVolumeTick();
      await service.playVolumeTick();
      await service.playVolumeTick();

      // Only first tick should execute immediately due to throttle
      expect(calls, hasLength(1));
      expect(calls.first.$1, 'canberra-gtk-play');
      expect(calls.first.$2, ['-i', 'audio-volume-change']);
    });

    test('does not play sounds when disabled', () async {
      final calls = <(String, List<String>)>[];
      final service = SoundService(
        enabled: false,
        command: (executable, arguments) async {
          calls.add((executable, arguments));
          return '';
        },
      );

      await service.playTrashEmpty();
      await service.playCameraShutter();
      await service.playAlert();
      await service.playVolumeTick();

      expect(calls, isEmpty);
    });

    test('gracefully handles command errors and attempts fallback', () async {
      final calls = <(String, List<String>)>[];
      final service = SoundService(
        command: (executable, arguments) async {
          calls.add((executable, arguments));
          if (executable == 'canberra-gtk-play') {
            throw StateError('canberra not found');
          }
          return '';
        },
      );

      await service.playTrashEmpty();

      expect(calls, hasLength(2));
      expect(calls[0].$1, 'canberra-gtk-play');
      expect(calls[1].$1, 'pw-play');
      expect(calls[1].$2, [
        '/usr/share/sounds/freedesktop/stereo/trash-empty.oga',
      ]);
    });
  });
}
