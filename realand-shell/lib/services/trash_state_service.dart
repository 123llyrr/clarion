// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:io';
import 'package:flutter/foundation.dart';
import 'system_service.dart';

/// GIO reports the complete virtual trash, including mounted volumes. The local
/// trash watch accelerates the usual case; polling also detects other volumes.
class TrashStateService extends ChangeNotifier {
  TrashStateService({CommandRunner? command, Map<String, String>? environment})
    : _command = command ?? runSystemCommand,
      _environment = environment ?? Platform.environment;
  final CommandRunner _command;
  final Map<String, String> _environment;
  bool hasItems = false;
  bool _disposed = false;
  bool _started = false;
  bool _reading = false;
  bool _again = false;
  Timer? _poll;
  Timer? _debounce;
  StreamSubscription<FileSystemEvent>? _watch;
  StreamSubscription<FileSystemEvent>? _fileWatch;

  Future<void> start() async {
    if (_started || _disposed) return;
    _started = true;
    final data = _environment['XDG_DATA_HOME'];
    final home = _environment['HOME'];
    final base = data != null && data.startsWith('/')
        ? data
        : home != null && home.startsWith('/')
        ? '$home/.local/share'
        : null;
    if (base != null) {
      final directory = Directory('$base/Trash');
      try {
        if (await directory.exists() && !_disposed) {
          void changed(FileSystemEvent _) {
            _debounce?.cancel();
            _debounce = Timer(const Duration(milliseconds: 100), refresh);
          }

          _watch = directory.watch().listen(changed, onError: (Object _) {});
          final files = Directory('$base/Trash/files');
          if (await files.exists() && !_disposed) {
            _fileWatch = files.watch().listen(changed, onError: (Object _) {});
          }
        }
      } on FileSystemException {
        // A disappearing or unreadable trash does not prevent fallback polling.
      }
    }
    if (_disposed) return;
    _poll = Timer.periodic(
      const Duration(seconds: 10),
      (_) => unawaited(refresh()),
    );
    await refresh();
  }

  Future<void> refresh() async {
    if (_disposed) return;
    _again = true;
    if (_reading) return;
    _reading = true;
    try {
      do {
        _again = false;
        final output = await _command('gio', ['list', 'trash:///']);
        if (_disposed) return;
        final next = output.trim().isNotEmpty;
        if (hasItems != next) {
          hasItems = next;
          notifyListeners();
        }
      } while (_again && !_disposed);
    } on Object {
      // An unavailable virtual filesystem keeps its last confirmed state.
    } finally {
      _reading = false;
    }
  }

  Future<void> emptyTrash() async {
    if (_disposed) return;
    try {
      await _command('gio', ['trash', '--empty']);
    } finally {
      await refresh();
    }
  }

  @override
  void dispose() {
    if (_disposed) return;
    _disposed = true;
    _poll?.cancel();
    _debounce?.cancel();
    unawaited(_watch?.cancel());
    unawaited(_fileWatch?.cancel());
    super.dispose();
  }
}
