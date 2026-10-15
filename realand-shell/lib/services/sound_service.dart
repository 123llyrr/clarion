// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';

import 'system_service.dart';

enum SoundEffect {
  trashEmpty('trash-empty'),
  cameraShutter('camera-shutter'),
  alert('bell'),
  volumeTick('audio-volume-change');

  const SoundEffect(this.soundId);
  final String soundId;
}

/// System audio service for macOS sound effects.
class SoundService {
  SoundService({
    CommandRunner? command,
    this.enabled = true,
  }) : _command = command ?? runSystemCommand;

  final CommandRunner _command;
  final bool enabled;

  DateTime _lastVolumeTick = DateTime.fromMillisecondsSinceEpoch(0);

  Future<void> play(SoundEffect effect) async {
    if (!enabled) return;
    try {
      await _command('canberra-gtk-play', ['-i', effect.soundId]);
    } on Object {
      // Audio playback is best-effort and must never disrupt shell operation.
      try {
        final fallback =
            '/usr/share/sounds/freedesktop/stereo/${effect.soundId}.oga';
        await _command('pw-play', [fallback]);
      } on Object {
        // Silently ignore if audio subsystem is unavailable.
      }
    }
  }

  Future<void> playTrashEmpty() => play(SoundEffect.trashEmpty);

  Future<void> playCameraShutter() => play(SoundEffect.cameraShutter);

  Future<void> playAlert() => play(SoundEffect.alert);

  Future<void> playVolumeTick() async {
    final now = DateTime.now();
    // Throttle volume ticks to at most once per 80ms to avoid audio queue flooding
    if (now.difference(_lastVolumeTick).inMilliseconds < 80) return;
    _lastVolumeTick = now;
    return play(SoundEffect.volumeTick);
  }
}
