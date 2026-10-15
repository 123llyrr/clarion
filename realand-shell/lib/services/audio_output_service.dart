// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';

import 'system_service.dart';

class AudioOutput {
  const AudioOutput({
    required this.name,
    required this.description,
    this.available = true,
  });
  final String name;
  final String description;
  final bool available;
  @override
  bool operator ==(Object other) =>
      other is AudioOutput &&
      name == other.name &&
      description == other.description &&
      available == other.available;
  @override
  int get hashCode => Object.hash(name, description, available);
}

class AudioEventWatch {
  AudioEventWatch(this.events, this.close);
  final Stream<void> events;
  final Future<void> Function() close;
}

typedef AudioEventSource = Future<AudioEventWatch> Function();

/// Enumerates real PulseAudio/PipeWire outputs only while the menu is open.
/// Volume drag echoes are deferred until the controller finishes its write queue.
class AudioOutputService extends ChangeNotifier {
  AudioOutputService({
    CommandRunner? command,
    AudioEventSource? watch,
    this.demo = false,
  }) : _command = command ?? runSystemCommand,
       _watch = watch ?? _watchPactl {
    if (demo) {
      _outputs = const [
        AudioOutput(name: 'preview-speaker', description: '内建扬声器'),
      ];
      _selected = 'preview-speaker';
    }
  }
  final CommandRunner _command;
  final AudioEventSource _watch;
  final bool demo;
  List<AudioOutput> _outputs = const [];
  String? _selected;
  String? _error;
  bool _active = false;
  bool _paused = false;
  bool _busy = false;
  bool _loading = false;
  bool _disposed = false;
  bool _dirty = false;
  int _generation = 0;
  int _watchGeneration = 0;
  int _snapshotRevision = 0;
  Future<void>? _readWork;
  AudioEventWatch? _eventWatch;
  StreamSubscription<void>? _events;
  Timer? _debounce;
  Timer? _fallback;

  List<AudioOutput> get outputs => _outputs;
  String? get selected => _selected;
  String? get error => _error;
  bool get loading => _loading;
  bool get busy => _busy;
  bool get active => _active;
  int get snapshotRevision => _snapshotRevision;

  void setActive(bool value) {
    if (_disposed || value == _active) return;
    _active = value;
    _generation++;
    final generation = ++_watchGeneration;
    _debounce?.cancel();
    _fallback?.cancel();
    _stopWatch();
    _dirty = value;
    if (!value || demo) return;
    unawaited(_startWatch(generation));
    unawaited(refresh());
  }

  void setPaused(bool value) {
    if (_disposed || _paused == value) return;
    _paused = value;
    if (value) {
      _generation++;
      _dirty = true;
    } else if (_active) {
      unawaited(refresh());
    }
  }

  Future<void> _startWatch(int generation) async {
    try {
      final watch = await _watch();
      if (_disposed || !_active || generation != _watchGeneration) {
        await watch.close();
        return;
      }
      _eventWatch = watch;
      _events = watch.events.listen(
        (_) {
          if (generation == _watchGeneration) _scheduleRefresh();
        },
        onError: (Object _) {
          if (generation == _watchGeneration) _watchEnded();
        },
        onDone: () {
          if (generation == _watchGeneration) _watchEnded();
        },
      );
      // Close the subscribe/start race by reading again after registration.
      unawaited(refresh());
    } on Object {
      if (!_disposed && _active && generation == _watchGeneration) {
        _watchEnded();
      }
    }
  }

  void _watchEnded() {
    if (_disposed || !_active) return;
    _stopWatch();
    _fallback?.cancel();
    _fallback = Timer.periodic(
      const Duration(seconds: 2),
      (_) => _scheduleRefresh(),
    );
    _scheduleRefresh();
  }

  void _stopWatch() {
    final events = _events;
    final watch = _eventWatch;
    _events = null;
    _eventWatch = null;
    if (events != null) unawaited(events.cancel());
    if (watch != null) unawaited(watch.close());
  }

  void _scheduleRefresh() {
    if (_disposed || !_active) return;
    _dirty = true;
    _debounce?.cancel();
    _debounce = Timer(const Duration(milliseconds: 100), () {
      _debounce = null;
      unawaited(refresh());
    });
  }

  Future<void> refresh() {
    if (_disposed || !_active || demo) return Future.value();
    _dirty = true;
    if (_paused || _busy) return Future.value();
    return _readWork ??= _readLoop().whenComplete(() {
      _readWork = null;
      if (_dirty && !_disposed && _active && !_paused && !_busy) {
        unawaited(refresh());
      }
    });
  }

  Future<void> _readLoop() async {
    _loading = _outputs.isEmpty;
    _notify();
    try {
      while (_dirty && _active && !_paused && !_busy && !_disposed) {
        _dirty = false;
        final generation = _generation;
        try {
          final result = await Future.wait([
            _command('pactl', ['--format=json', 'list', 'sinks']),
            _command('pactl', ['get-default-sink']),
          ]);
          if (_disposed || !_active || generation != _generation) continue;
          final outputs = parseOutputs(result[0]);
          final selected = result[1].trim();
          if (!listEquals(outputs, _outputs)) _outputs = outputs;
          _selected = outputs.any((output) => output.name == selected)
              ? selected
              : null;
          _error = null;
          _snapshotRevision++;
        } on Object catch (error) {
          if (_disposed || !_active || generation != _generation) continue;
          _outputs = const [];
          _selected = null;
          _error = '无法读取输出设备：$error';
          _snapshotRevision++;
        }
        _notify();
      }
    } finally {
      _loading = false;
      _notify();
    }
  }

  Future<void> select(String name) async {
    if (_disposed || _busy) return;
    if (!_outputs.any((output) => output.name == name && output.available)) {
      throw StateError('输出设备已不可用');
    }
    if (_selected == name) return;
    if (demo) {
      _selected = name;
      _notify();
      return;
    }
    _busy = true;
    _generation++;
    _dirty = true;
    _notify();
    try {
      // Stable sink names avoid targeting another device after numeric ID reuse.
      // WirePlumber owns the default route and follows it for unpinned streams.
      await _command('pactl', ['set-default-sink', name]);
    } finally {
      _busy = false;
      if (!_disposed) {
        _notify();
        await refresh();
      }
    }
  }

  static List<AudioOutput> parseOutputs(String source) {
    final values = jsonDecode(source);
    if (values is! List) throw const FormatException('音频设备列表格式无效');
    final found = <String, AudioOutput>{};
    for (final value in values) {
      if (value is! Map) continue;
      final name = value['name'];
      if (name is! String ||
          name.isEmpty ||
          name.startsWith('-') ||
          name.contains('\u0000')) {
        continue;
      }
      final description = value['description'];
      final ports = value['ports'];
      final available =
          ports is! List ||
          ports.isEmpty ||
          ports.any(
            (port) => port is Map && port['availability'] != 'not available',
          );
      found[name] = AudioOutput(
        name: name,
        description: description is String && description.trim().isNotEmpty
            ? description
            : name,
        available: available,
      );
    }
    return List.unmodifiable(found.values);
  }

  void _notify() {
    if (!_disposed) notifyListeners();
  }

  @override
  void dispose() {
    _disposed = true;
    _generation++;
    _watchGeneration++;
    _debounce?.cancel();
    _fallback?.cancel();
    _stopWatch();
    super.dispose();
  }
}

Future<AudioEventWatch> _watchPactl() async {
  final process = await Process.start(
    'pactl',
    ['subscribe'],
    environment: {'LC_ALL': 'C'},
  );
  final errors = process.stderr.listen((_) {});
  final events = process.stdout
      .transform(const Utf8Decoder(allowMalformed: true))
      .transform(const LineSplitter())
      .where((line) => RegExp(r'on (sink|server|card) #').hasMatch(line))
      .map<void>((_) {});
  return AudioEventWatch(events, () async {
    process.kill();
    await errors.cancel();
  });
}
