// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:io';

import 'clarion_ipc.dart';

/// Serialize GPU readbacks and publish only completed screenshot files.
class WindowPreviewService {
  WindowPreviewService(
    this.ipc, {
    this.timeout = const Duration(seconds: 5),
    Future<Directory> Function()? createDirectory,
  }) : _createDirectory =
           createDirectory ??
           (() => Directory.systemTemp.createTemp('clarion-previews-'));

  final ClarionIpc ipc;
  final Duration timeout;
  final Future<Directory> Function() _createDirectory;
  final _pending = <int, Future<String?>>{};
  Future<Directory>? _directory;
  Future<void> _tail = Future<void>.value();
  int _generation = 0;
  int _serial = 0;
  bool _disposed = false;

  Future<String?> capture(int id) {
    if (_disposed) return Future<String?>.value();
    final existing = _pending[id];
    if (existing != null) return existing;
    final generation = _generation;
    final work = _tail.then((_) async {
      if (_disposed || generation != _generation) return null;
      return _capture(id, generation);
    });
    _pending[id] = work;
    _tail = work.then((_) {
      if (identical(_pending[id], work)) _pending.remove(id);
    });
    return work;
  }

  Future<String?> _capture(int id, int generation) async {
    StreamSubscription<Map<String, dynamic>>? subscription;
    Timer? timer;
    String? path;
    var published = false;
    try {
      final directory = await (_directory ??= _createDirectory());
      if (_disposed || generation != _generation) return null;
      path = '${directory.path}/$id-${_serial++}.png';
      final completed = Completer<bool>();
      // The completion event may arrive before the action socket's reply.
      subscription = ipc.events.listen((event) {
        final capture = event['ScreenshotCaptured'];
        if (capture is Map &&
            capture['path'] == path &&
            !completed.isCompleted) {
          completed.complete(true);
        }
      });
      timer = Timer(timeout, () {
        if (!completed.isCompleted) completed.complete(false);
      });
      await ipc.action('ScreenshotWindow', {
        'id': id,
        'write_to_disk': true,
        'show_pointer': false,
        'path': path,
      });
      if (!await completed.future || _disposed || generation != _generation) {
        return null;
      }
      if (!await File(path).exists()) return null;
      published = true;
      return path;
    } on Object {
      if (path == null) _directory = null;
      // A window can disappear while waiting its turn in the queue.
      return null;
    } finally {
      timer?.cancel();
      await subscription?.cancel();
      if (!published && path != null) await remove(path);
    }
  }

  void cancelQueued() {
    _generation++;
    _pending.clear();
    // Keep the in-flight readback in the queue until it completes or times out.
  }

  static Future<void> remove(String path) async {
    try {
      await File(path).delete();
    } on FileSystemException {
      // Files may already be removed during shutdown.
    }
  }

  Future<void> dispose() async {
    if (_disposed) return;
    _disposed = true;
    cancelQueued();
    await _tail;
    try {
      final directory = await _directory;
      await directory?.delete(recursive: true);
    } on Object {
      // Temporary directory creation or cleanup may fail independently of IPC.
    }
  }
}
