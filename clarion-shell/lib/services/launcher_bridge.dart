// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:convert';
import 'dart:io';

/// A launcher reply. Old visibility replies are ignored; launch completions
/// still update recent applications after a newer request supersedes them.
class LauncherEvent {
  const LauncherEvent(
    this.type,
    this.requestId, {
    this.appId,
    this.message,
    this.context,
  });
  final String type;
  final int requestId;
  final String? appId;
  final String? message;
  final String? context;
}

typedef LauncherSpawner =
    Future<void> Function(String executable, List<String> arguments);

/// Owns a persistent JSON-lines connection, and only processes it spawned itself.
/// Connecting to an existing launcher never grants permission to terminate it.
class LauncherBridge {
  LauncherBridge({
    Map<String, String>? environment,
    String? shellExecutable,
    LauncherSpawner? spawn,
    this.startupTimeout = const Duration(seconds: 4),
  }) : environment = Map.unmodifiable(environment ?? Platform.environment),
       shellExecutable = shellExecutable ?? Platform.resolvedExecutable,
       _spawn = spawn;

  final Map<String, String> environment;
  final String shellExecutable;
  final Duration startupTimeout;
  final LauncherSpawner? _spawn;
  final _events = StreamController<LauncherEvent>.broadcast();
  Stream<LauncherEvent> get events => _events.stream;
  int _requestId = 0;
  int get requestId => _requestId;
  int beginRequest() => ++_requestId;
  bool _disposed = false;
  bool _hasShown = false;
  Socket? _socket;
  Process? _ownedProcess;
  Future<void>? _connecting;
  Future<void> _work = Future.value();
  int _connection = 0;
  String _lastError = '';

  String get socketPath {
    final runtime = environment['XDG_RUNTIME_DIR'];
    if (runtime == null || !runtime.startsWith('/')) {
      throw StateError('XDG_RUNTIME_DIR is required for the launcher');
    }
    return '$runtime/clarion-launcher.sock';
  }

  bool _executable(String path) {
    try {
      final stat = File(path).statSync();
      return stat.type == FileSystemEntityType.file && (stat.mode & 0x49) != 0;
    } on FileSystemException {
      return false;
    }
  }

  /// Prefer the explicit override and the running deployment generation.
  String resolveBinary() {
    final override = environment['CLARION_LAUNCHER_BIN'];
    if (override != null && override.isNotEmpty) {
      if (!override.startsWith('/') || !_executable(override)) {
        throw StateError('CLARION_LAUNCHER_BIN must be an absolute executable');
      }
      return override;
    }
    String executable;
    try {
      executable = File(shellExecutable).resolveSymbolicLinksSync();
    } on FileSystemException {
      executable = shellExecutable;
    }
    final directory = File(executable).parent;
    for (final candidate in [
      '${directory.path}/clarion-launcher',
      '${directory.parent.path}/clarion-launcher',
      '${directory.parent.path}/bin/clarion-launcher',
      for (final root in (environment['PATH'] ?? '').split(':'))
        if (root.startsWith('/')) '$root/clarion-launcher',
    ]) {
      if (_executable(candidate)) return candidate;
    }
    throw StateError('clarion-launcher executable was not found');
  }

  Future<void> show({
    required int requestId,
    String? output,
    required String theme,
    required String accent,
    required List<String> recent,
  }) => _queue(() async {
    if (!_current(requestId)) return;
    await (_connecting ??= _connect(
      output,
    ).whenComplete(() => _connecting = null));
    if (!_current(requestId)) return;
    // The daemon outlives catalog changes. Queue its background refresh on
    // every open without waiting for directory scanning before showing UI.
    await _reloadCurrent(requestId);
    if (!_current(requestId)) return;
    await _write({
      'type': 'Show',
      'request_id': requestId,
      'output': ?output,
      'theme': theme,
      'accent': accent,
      'recent': recent,
    });
    _hasShown = true;
  }, requestId);

  Future<void> hide(int requestId) => _queue(() async {
    if (!_current(requestId)) return;
    // Closing the Shell UI must never start a new launcher process.
    if (_socket == null && !await _connectExisting()) return;
    if (!_current(requestId)) return;
    await _write({'type': 'Hide', 'request_id': requestId});
    _hasShown = false;
  }, requestId);

  Future<void> preferences({
    required int requestId,
    required String theme,
    required String accent,
    required List<String> recent,
  }) => _queue(() async {
    if (!_current(requestId) || _socket == null) return;
    await _write({
      'type': 'Preferences',
      'request_id': requestId,
      'theme': theme,
      'accent': accent,
      'recent': recent,
    });
  }, requestId);

  Future<void> reload(int requestId) =>
      _queue(() => _reloadCurrent(requestId), requestId);

  Future<void> _reloadCurrent(int requestId) async {
    if (_current(requestId) && _socket != null) {
      await _write({'type': 'Reload', 'request_id': requestId});
    }
  }

  bool _current(int id) => !_disposed && id == _requestId;

  Future<void> _queue(Future<void> Function() operation, int id) {
    final next = _work.then((_) async {
      try {
        await operation();
      } on Object catch (error) {
        if (_current(id)) _failure('Launcher connection failed: $error', id);
        rethrow;
      }
    });
    _work = next.catchError((Object _) {});
    return next;
  }

  Future<void> _write(Map<String, Object?> command) async {
    final socket = _socket;
    if (socket == null) throw StateError('Launcher connection was closed');
    socket.writeln(jsonEncode(command));
    await socket.flush().timeout(const Duration(seconds: 1));
  }

  Future<bool> _connectExisting() async {
    if (_disposed) return false;
    if (_socket != null) return true;
    try {
      final socket = await Socket.connect(
        InternetAddress(socketPath, type: InternetAddressType.unix),
        0,
        timeout: const Duration(milliseconds: 200),
      );
      if (_disposed) {
        socket.destroy();
        return false;
      }
      _attach(socket);
      return true;
    } on SocketException {
      return false;
    }
  }

  Future<void> _connect(String? output) async {
    if (await _connectExisting()) return;
    if (_disposed) return;
    final binary = resolveBinary();
    final arguments = [
      '--serve',
      if (output != null) ...['--output', output],
      '--parent-pid',
      '$pid',
    ];
    if (_spawn != null) {
      await _spawn(binary, arguments).timeout(startupTimeout);
    } else {
      final process = await Process.start(
        binary,
        arguments,
        environment: environment,
      );
      if (_disposed) {
        process.kill();
        await process.exitCode;
        return;
      }
      _ownedProcess = process;
      unawaited(process.stdout.drain<void>());
      process.stderr.transform(const Utf8Decoder(allowMalformed: true)).listen((
        text,
      ) {
        _lastError = '$_lastError$text';
        if (_lastError.length > 4096) {
          _lastError = _lastError.substring(_lastError.length - 4096);
        }
      });
      unawaited(
        process.exitCode.then((code) {
          if (!_disposed && identical(_ownedProcess, process)) {
            _ownedProcess = null;
            // A concurrently started single instance may reuse an existing
            // daemon and exit successfully. A live socket remains authoritative.
            if (_socket == null && code != 0) {
              _failure(
                'Launcher exited ($code)${_lastError.isEmpty ? '' : ': $_lastError'}',
                _requestId,
              );
            }
          }
        }),
      );
    }
    final deadline = DateTime.now().add(startupTimeout);
    while (!_disposed && DateTime.now().isBefore(deadline)) {
      if (await _connectExisting()) return;
      await Future<void>.delayed(const Duration(milliseconds: 40));
    }
    if (!_disposed) {
      _ownedProcess?.kill();
      throw TimeoutException(
        'Launcher did not create its socket',
        startupTimeout,
      );
    }
  }

  void _attach(Socket socket) {
    _socket = socket;
    final generation = ++_connection;
    socket
        .cast<List<int>>()
        .transform(utf8.decoder)
        .transform(const LineSplitter())
        .listen(
          (line) {
            if (_disposed || generation != _connection) return;
            try {
              final value = jsonDecode(line);
              if (value is! Map ||
                  value['request_id'] is! int ||
                  value['type'] is! String) {
                throw const FormatException('Invalid launcher event');
              }
              final id = value['request_id'] as int;
              final type = value['type'] as String;
              if (!_current(id) && type != 'Launched') return;
              if (!{'Visible', 'Hidden', 'Launched', 'Error'}.contains(type)) {
                return;
              }
              _events.add(
                LauncherEvent(
                  type,
                  id,
                  appId: value['app_id'] is String
                      ? value['app_id'] as String
                      : null,
                  message: value['message'] is String
                      ? value['message'] as String
                      : null,
                  context: value['context'] is String
                      ? value['context'] as String
                      : null,
                ),
              );
            } on Object catch (error) {
              _disconnect('Invalid launcher response: $error');
            }
          },
          onError: (Object error) {
            if (generation == _connection) {
              _disconnect('Launcher socket failed: $error');
            }
          },
          onDone: () {
            if (generation == _connection) {
              _disconnect('Launcher connection closed');
            }
          },
          cancelOnError: true,
        );
  }

  void _disconnect(String message) {
    _connection++;
    _socket?.destroy();
    _socket = null;
    if (!_disposed) _failure(message, _requestId);
  }

  void _failure(String message, int id) {
    if (_current(id)) {
      _events.add(LauncherEvent('Error', id, message: message, context: 'ipc'));
    }
  }

  Future<void> dispose() async {
    if (_disposed) return;
    if (_hasShown && _socket != null) {
      try {
        await _write({'type': 'Hide', 'request_id': beginRequest()});
      } on Object {
        /* The connection may already have closed. */
      }
    }
    _disposed = true;
    _connection++;
    _socket?.destroy();
    _socket = null;
    final owned = _ownedProcess;
    _ownedProcess = null;
    if (owned != null) {
      owned.kill();
      await owned.exitCode.timeout(
        const Duration(seconds: 2),
        onTimeout: () {
          owned.kill(ProcessSignal.sigkill);
          return -1;
        },
      );
    }
    await _events.close();
  }
}
