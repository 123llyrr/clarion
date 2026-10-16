// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'identity.dart';

/// Matches clarion-ipc::socket::MAX_LINE_BYTES, including the newline.
const maxIpcLineBytes = 16 * 1024 * 1024;

/// Old compositors reject unknown requests. Preserve their working modal menu
/// rename until the running compositor can return focus to the desktop layer.
Future<bool> desktopMenuFocusRestoreSupported(ClarionIpc ipc) async {
  try {
    final response = await ipc.request('Capabilities');
    return response is Map &&
        response['Capabilities'] is List &&
        (response['Capabilities'] as List).contains(
          'desktop-menu-focus-restore',
        );
  } on Object {
    return false;
  }
}

Stream<String> boundedJsonLines(
  Stream<List<int>> source, {
  int maximum = maxIpcLineBytes,
}) async* {
  var bytes = BytesBuilder(copy: false);
  var size = 0;
  await for (final chunk in source) {
    var start = 0;
    for (var index = 0; index < chunk.length; index++) {
      if (chunk[index] != 10) continue;
      final length = index - start;
      if (size + length + 1 > maximum) {
        throw const FormatException('IPC line exceeds its size limit');
      }
      bytes.add(chunk.sublist(start, index));
      yield utf8.decode(bytes.takeBytes());
      size = 0;
      start = index + 1;
    }
    if (start < chunk.length) {
      size += chunk.length - start;
      if (size >= maximum) {
        throw const FormatException('IPC line exceeds its size limit');
      }
      bytes.add(chunk.sublist(start));
    }
  }
  if (size != 0) throw const FormatException('Incomplete IPC line');
}

class ClarionIpc {
  ClarionIpc({String? socketPath, this.timeout = const Duration(seconds: 3)})
    : socketPath = socketPath ?? ipcSocketPath(Platform.environment);
  final String? socketPath;
  final Duration timeout;
  final _events = StreamController<Map<String, dynamic>>.broadcast();
  final _connections = StreamController<bool>.broadcast();
  Socket? _eventSocket;
  final _actionSockets = <Socket>{};
  Timer? _retry;
  Completer<void>? _retryCompleter;
  bool _disposed = false;
  bool _started = false;

  Stream<Map<String, dynamic>> get events => _events.stream;
  Stream<bool> get connections => _connections.stream;

  void start() {
    if (_started || _disposed) return;
    _started = true;
    unawaited(_listen());
  }

  Future<Socket> _connect() async {
    if (socketPath == null || socketPath!.isEmpty) {
      throw const SocketException('CLARION_SOCKET is unavailable');
    }
    return Socket.connect(
      InternetAddress(socketPath!, type: InternetAddressType.unix),
      0,
      timeout: timeout,
    );
  }

  Future<void> _listen() async {
    var delay = 250;
    while (!_disposed) {
      Socket? socket;
      Timer? handshake;
      try {
        socket = await _connect();
        if (_disposed) {
          socket.destroy();
          break;
        }
        _eventSocket = socket;
        socket.write('"EventStream"\n');
        await socket.flush();
        var accepted = false;
        handshake = Timer(timeout, socket.destroy);
        await for (final line in boundedJsonLines(socket)) {
          if (_disposed) break;
          final value = jsonDecode(line);
          if (!accepted) {
            if (value is! Map || value['Ok'] != 'Handled') {
              throw const FormatException('Event stream was rejected');
            }
            accepted = true;
            handshake.cancel();
            delay = 250;
            _connections.add(true);
          } else if (value is Map<String, dynamic>) {
            _events.add(value);
          }
        }
      } on Object {
        // Reconnecting receives a new authoritative snapshot. Never keep an
        // action pending for replay: it might refer to an obsolete window ID.
      } finally {
        handshake?.cancel();
        socket?.destroy();
        _eventSocket = null;
        if (!_disposed) _connections.add(false);
      }
      if (_disposed) break;
      final completer = Completer<void>();
      _retryCompleter = completer;
      _retry = Timer(Duration(milliseconds: delay), completer.complete);
      await completer.future;
      _retryCompleter = null;
      _retry = null;
      delay = (delay * 2).clamp(250, 5000);
    }
  }

  Future<dynamic> request(Object request) async {
    if (_disposed) throw StateError('IPC client is disposed');
    final bytes = utf8.encode('${jsonEncode(request)}\n');
    if (bytes.length > maxIpcLineBytes) {
      throw const FormatException('IPC request exceeds its size limit');
    }
    final socket = await _connect();
    if (_disposed) {
      socket.destroy();
      throw StateError('IPC client is disposed');
    }
    _actionSockets.add(socket);
    try {
      socket.add(bytes);
      await socket.flush().timeout(timeout);
      final line = await boundedJsonLines(socket).first.timeout(timeout);
      final reply = jsonDecode(line);
      if (reply is! Map || !reply.containsKey('Ok')) {
        throw StateError(
          reply is Map ? '${reply['Err']}' : 'Invalid IPC reply',
        );
      }
      return reply['Ok'];
    } finally {
      _actionSockets.remove(socket);
      socket.destroy();
    }
  }

  Future<void> action(
    String name, [
    Map<String, Object?> arguments = const {},
  ]) async {
    await request({
      'Action': {name: arguments},
    });
  }

  void dispose() {
    if (_disposed) return;
    _disposed = true;
    _eventSocket?.destroy();
    final actions = _actionSockets.toList();
    _actionSockets.clear();
    for (final socket in actions) {
      socket.destroy();
    }
    _retry?.cancel();
    if (_retryCompleter case final completer? when !completer.isCompleted) {
      completer.complete();
    }
    unawaited(_events.close());
    unawaited(_connections.close());
  }
}
