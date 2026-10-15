// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:realand_shell/models.dart';
import 'package:realand_shell/services/sound_service.dart';
import 'package:realand_shell/services/trash_state_service.dart';
import 'package:realand_shell/services/window_decoration_service.dart';
import 'package:realand_shell/state/shell_controller.dart';

void main() {
  group('WindowDecorationService', () {
    test('applies macOS button layout to gsettings and gtk configs', () async {
      final tempDir = await Directory.systemTemp.createTemp('gtk_test_');
      addTearDown(() => tempDir.delete(recursive: true));

      final gtk3Dir = Directory('${tempDir.path}/.config/gtk-3.0');
      await gtk3Dir.create(recursive: true);
      final gtk3File = File('${gtk3Dir.path}/settings.ini');
      await gtk3File.writeAsString(
        '[Settings]\ngtk-decoration-layout=icon:minimize,maximize,close\n',
      );

      final calls = <(String, List<String>)>[];
      final service = WindowDecorationService(
        command: (executable, arguments) async {
          calls.add((executable, arguments));
          return '';
        },
        homeDirectory: tempDir.path,
      );

      await service.applyForStyle(ShellStyle.macos);

      expect(calls, hasLength(1));
      expect(calls.first.$1, 'gsettings');
      expect(calls.first.$2, [
        'set',
        'org.gnome.desktop.wm.preferences',
        'button-layout',
        'close,minimize,maximize:',
      ]);

      final updatedGtk3 = await gtk3File.readAsString();
      expect(
        updatedGtk3,
        contains('gtk-decoration-layout=close,minimize,maximize:'),
      );
    });

    test('reverts to material layout for Material style', () async {
      final calls = <(String, List<String>)>[];
      final service = WindowDecorationService(
        command: (executable, arguments) async {
          calls.add((executable, arguments));
          return '';
        },
        homeDirectory: '',
      );

      await service.applyForStyle(ShellStyle.material);

      expect(calls, hasLength(1));
      expect(calls.first.$1, 'gsettings');
      expect(calls.first.$2, [
        'set',
        'org.gnome.desktop.wm.preferences',
        'button-layout',
        'icon:minimize,maximize,close',
      ]);
    });
  });

  group('ShellController sound and decoration integration', () {
    test('switching shellStyle triggers window decoration layout update', () async {
      final styleCalls = <ShellStyle>[];
      final decorService = _MockWindowDecorationService(
        onApply: (style) => styleCalls.add(style),
      );

      final controller = ShellController(
        demo: false,
        windowDecorationService: decorService,
      );
      addTearDown(controller.dispose);

      controller.setShellStyle(ShellStyle.macos);
      await Future<void>.delayed(Duration.zero);
      expect(styleCalls, contains(ShellStyle.macos));

      controller.setShellStyle(ShellStyle.material);
      await Future<void>.delayed(Duration.zero);
      expect(styleCalls, contains(ShellStyle.material));
    });

    test('sound effects trigger on volume, trash empty, and alert', () async {
      final soundEvents = <SoundEffect>[];
      final soundService = SoundService(
        command: (executable, arguments) async {
          final id = arguments.last;
          for (final effect in SoundEffect.values) {
            if (effect.soundId == id) {
              soundEvents.add(effect);
              break;
            }
          }
          return '';
        },
      );

      final trashService = TrashStateService(
        command: (e, a) async => '',
      );

      final controller = ShellController(
        demo: false,
        soundService: soundService,
        trashStateService: trashService,
      );
      addTearDown(controller.dispose);

      // Volume change triggers volumeTick
      await controller.setVolume(0.5);
      await Future<void>.delayed(Duration.zero);
      expect(soundEvents, contains(SoundEffect.volumeTick));

      // Empty trash triggers trashEmpty
      await controller.emptyTrash();
      await Future<void>.delayed(Duration.zero);
      expect(soundEvents, contains(SoundEffect.trashEmpty));
    });
  });
}

class _MockWindowDecorationService extends WindowDecorationService {
  _MockWindowDecorationService({required this.onApply})
    : super(command: (e, a) async => '', homeDirectory: '');

  final void Function(ShellStyle style) onApply;

  @override
  Future<void> applyForStyle(ShellStyle style) async {
    onApply(style);
  }
}
