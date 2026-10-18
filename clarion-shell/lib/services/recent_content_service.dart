// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:crypto/crypto.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import '../recent_content_models.dart';

abstract interface class ClipboardTransport {
  Stream<Object?> get events;
  Future<bool> start();
  Future<bool> reconnect();
  Future<void> setPaused(bool paused);
  Future<bool> publish(
    String mime,
    Uint8List data, {
    bool isScreenshot = false,
  });
  Future<void> stop();
}

class NativeClipboardTransport implements ClipboardTransport {
  static const _methods = MethodChannel('clarion/clipboard');
  static const _events = EventChannel('clarion/clipboard-events');
  @override
  Stream<Object?> get events => _events.receiveBroadcastStream();
  @override
  Future<bool> start() async =>
      await _methods.invokeMethod<bool>('start') ?? false;
  @override
  Future<bool> reconnect() async =>
      await _methods.invokeMethod<bool>('reconnect') ?? false;
  @override
  Future<void> setPaused(bool paused) =>
      _methods.invokeMethod<void>('setPaused', {'paused': paused});
  @override
  Future<bool> publish(
    String mime,
    Uint8List data, {
    bool isScreenshot = false,
  }) async =>
      await _methods.invokeMethod<bool>('publish', {
        'mime': mime,
        'data': data,
        'isScreenshot': isScreenshot,
      }) ??
      false;
  @override
  Future<void> stop() => _methods.invokeMethod<void>('stop');
}

String _sha256Payload(Uint8List bytes) => sha256.convert(bytes).toString();

typedef ThumbnailBuilder = Future<Uint8List?> Function(Uint8List bytes);

/// Session-only history. The only source of clipboard contents is the native
/// data-control client; viewing a panel never reads a focused application's data.
class RecentContentService extends ChangeNotifier {
  RecentContentService({
    ClipboardTransport? transport,
    this.enabled = true,
    this.entryLimit = 80,
    this.byteLimit = 64 * 1024 * 1024,
    this.textLimit = 1024 * 1024,
    this.imageLimit = 16 * 1024 * 1024,
    ThumbnailBuilder? buildThumbnail,
    Future<String> Function(Uint8List)? digest,
    DateTime Function()? now,
  }) : _transport = transport ?? NativeClipboardTransport(),
       _thumbnail = buildThumbnail ?? imageThumbnail,
       _digest = digest ?? ((bytes) => compute(_sha256Payload, bytes)),
       _now = now ?? DateTime.now {
    if (entryLimit < 1 || byteLimit < 1 || textLimit < 1 || imageLimit < 1) {
      throw ArgumentError('History limits must be positive');
    }
  }

  final ClipboardTransport _transport;
  final bool enabled;
  final int entryLimit, byteLimit, textLimit, imageLimit;
  final ThumbnailBuilder _thumbnail;
  final Future<String> Function(Uint8List) _digest;
  final DateTime Function() _now;
  final _items = <String, RecentContentEntry>{};
  final _payloads = <String, _Payload>{};
  final _queue = <_Incoming>[];
  final _busy = <String>{};
  final _deletedBefore = <String, int>{};
  int _incomingSerial = 0;
  StreamSubscription<Object?>? _subscription;
  Future<void>? _drainWork;
  Future<void>? _startWork;
  Future<void>? _reconnectWork;
  bool _disposed = false;
  bool _paused = false;
  bool _available = false;
  bool _loading = false;
  bool _started = false;
  int _generation = 0;
  int _nativeGeneration = -1;
  int _bytes = 0;
  int _queueBytes = 0;
  String? _error;

  List<RecentContentEntry> get entries {
    final result = _items.values.toList()
      ..sort((a, b) {
        final pinned = (b.isPinned ? 1 : 0).compareTo(a.isPinned ? 1 : 0);
        return pinned != 0 ? pinned : b.capturedAt.compareTo(a.capturedAt);
      });
    return List.unmodifiable(result);
  }

  Set<String> get busyEntryIds => Set.unmodifiable(_busy);
  bool get paused => _paused;
  bool get available => _available;
  bool get loading => _loading;
  String? get error => _error;
  int get retainedBytes => _bytes;

  Future<void> start() => _startWork ??= _start();
  Future<void> reconnect() => _reconnectWork ??= _reconnect().whenComplete(() {
    _reconnectWork = null;
  });
  Future<void> _reconnect() async {
    if (_disposed || !enabled) return;
    _loading = true;
    _notify();
    try {
      if (!_started) {
        await start();
      } else {
        _available = await _transport.reconnect();
        if (_paused && !_disposed) await _transport.setPaused(true);
        _error = _available ? null : '当前会话未提供剪贴板历史协议';
      }
    } on Object {
      _available = false;
      _error = '无法重新连接剪贴板服务';
    } finally {
      _loading = false;
      _notify();
    }
  }

  Future<void> _start() async {
    if (_disposed || !enabled) return;
    _started = true;
    _loading = true;
    _notify();
    _subscription = _transport.events.listen(
      _event,
      onError: (Object _) {
        if (_disposed) return;
        _available = false;
        _loading = false;
        _error = '剪贴板连接暂不可用，可重新连接后继续记录';
        _notify();
      },
    );
    try {
      final available = await _transport.start();
      if (_disposed) {
        await _transport.stop();
        return;
      }
      _available = available;
      if (_paused && !_disposed) await _transport.setPaused(true);
      if (!_available) _error = '当前会话未提供剪贴板历史协议';
    } on Object {
      _available = false;
      _error = '无法连接剪贴板服务';
    } finally {
      _loading = false;
      _notify();
    }
  }

  void _event(Object? raw) {
    if (_disposed || raw is! Map) return;
    if (raw['type'] == 'status') {
      final available = raw['available'];
      if (available is bool) {
        if (!_available && available) _nativeGeneration = -1;
        _available = available;
      }
      final message = raw['error'];
      if (message is String && message.isNotEmpty) {
        _error = '剪贴板连接暂不可用';
      } else if (_available) {
        _error = null;
      }
      _notify();
      return;
    }
    if (raw['type'] != 'selection' || _paused || raw['sensitive'] == true) {
      return;
    }
    final sequence = raw['generation'];
    if (sequence is int) {
      if (sequence <= _nativeGeneration) return;
      _nativeGeneration = sequence;
    }
    final mime = raw['mime'];
    final bytes = raw['data'];
    if ((mime != 'text/plain' && mime != 'image/png') || bytes is! Uint8List) {
      return;
    }
    final limit = mime == 'image/png' ? imageLimit : textLimit;
    if (bytes.isEmpty || bytes.length > limit) return;
    _enqueue(
      _Incoming(
        generation: _generation,
        mime: mime as String,
        bytes: bytes,
        screenshot: mime == 'image/png' && raw['isScreenshot'] == true,
        capturedAt: _now(),
      ),
    );
  }

  /// Only the compositor's successful, non-preview completion event calls this.
  void recordScreenshot(String? path, {bool isPreview = false}) {
    if (_disposed ||
        !enabled ||
        _paused ||
        isPreview ||
        path == null ||
        !File(path).isAbsolute ||
        path.contains('\u0000') ||
        path.length > 8192) {
      return;
    }
    _enqueue(
      _Incoming(
        generation: _generation,
        mime: 'image/png',
        filePath: path,
        screenshot: true,
        capturedAt: _now(),
      ),
    );
  }

  void _enqueue(_Incoming item) {
    item.serial = ++_incomingSerial;
    final size = item.bytes?.length ?? 0;
    // Bound work waiting behind hashing/thumbnail generation independently of
    // the retained history budget. Always prefer the more recent selection.
    while (_queue.isNotEmpty &&
        (_queue.length >= 8 || _queueBytes + size > 32 * 1024 * 1024)) {
      _queueBytes -= _queue.removeAt(0).bytes?.length ?? 0;
    }
    _queue.add(item);
    _queueBytes += size;
    _scheduleDrain();
  }

  void _scheduleDrain() {
    if (_drainWork != null || _disposed || _queue.isEmpty) return;
    _drainWork = _drain().whenComplete(() {
      _drainWork = null;
      if (_queue.isEmpty) _deletedBefore.clear();
      _scheduleDrain();
    });
  }

  Future<void> _drain() async {
    while (_queue.isNotEmpty && !_disposed) {
      final item = _queue.removeAt(0);
      _queueBytes -= item.bytes?.length ?? 0;
      if (_paused || item.generation != _generation) continue;
      try {
        var bytes = item.bytes;
        if (bytes == null) {
          final file = File(item.filePath!);
          final handle = await file.open();
          try {
            final size = await handle.length();
            if (size < 8 || size > imageLimit) continue;
            bytes = await handle.read(imageLimit + 1);
          } finally {
            await handle.close();
          }
        }
        if (_paused || _disposed || item.generation != _generation) continue;
        final image = item.mime == 'image/png';
        if (bytes.isEmpty || bytes.length > (image ? imageLimit : textLimit)) {
          continue;
        }
        if (image && !_isPng(bytes)) continue;
        final text = image ? '' : utf8.decode(bytes);
        final id = '${image ? 'image' : 'text'}:${await _digest(bytes)}';
        if (_paused || _disposed || item.generation != _generation) continue;
        if (item.serial <= (_deletedBefore[id] ?? -1)) continue;
        final existing = _items[id];
        final thumbnail = image
            ? existing?.thumbnail ?? await _thumbnail(bytes)
            : null;
        if (image && thumbnail == null) continue;
        if (item.serial <= (_deletedBefore[id] ?? -1)) continue;
        if (_paused || _disposed || item.generation != _generation) continue;
        if (existing == null && !_makeRoom(bytes.length)) continue;
        if (existing == null) {
          _payloads[id] = _Payload(item.mime, bytes);
          _bytes += bytes.length;
        }
        _items[id] = RecentContentEntry(
          id: id,
          kind:
              item.screenshot || existing?.kind == RecentContentKind.screenshot
              ? RecentContentKind.screenshot
              : image
              ? RecentContentKind.image
              : RecentContentKind.text,
          capturedAt: item.capturedAt,
          text: text,
          thumbnail: thumbnail,
          isPinned: existing?.isPinned ?? false,
          filePath: item.filePath ?? existing?.filePath,
          source:
              item.screenshot || existing?.kind == RecentContentKind.screenshot
              ? '截图'
              : '剪贴板',
        );
        _error = null;
        _notify();
      } on FormatException {
        // Binary or malformed text is not a clipboard-history entry.
      } on Object {
        if (!_disposed && item.generation == _generation) {
          _error = item.filePath != null ? '无法载入这张截图，原文件未改动' : '部分内容无法生成预览';
          _notify();
        }
      }
    }
  }

  bool _makeRoom(int incoming) {
    if (incoming > byteLimit) return false;
    while (_items.length >= entryLimit || _bytes + incoming > byteLimit) {
      final unpinned = _items.values.where((entry) => !entry.isPinned).toList()
        ..sort((a, b) => a.capturedAt.compareTo(b.capturedAt));
      if (unpinned.isEmpty) {
        _error = '置顶内容已占满历史空间，请取消部分置顶后继续记录';
        _notify();
        return false;
      }
      _remove(unpinned.first.id);
    }
    return true;
  }

  Future<bool> copy(RecentContentEntry entry) async {
    final payload = _payloads[entry.id];
    if (_disposed || payload == null || !_busy.add(entry.id)) return false;
    _notify();
    try {
      final success = await _transport.publish(
        payload.mime,
        payload.bytes,
        isScreenshot: entry.kind == RecentContentKind.screenshot,
      );
      if (_disposed) return false;
      if (!success) throw StateError('unavailable');
      _error = null;
      return true;
    } on Object {
      if (!_disposed) _error = '复制未完成，请检查剪贴板连接后重试';
      return false;
    } finally {
      _busy.remove(entry.id);
      _notify();
    }
  }

  void togglePinned(RecentContentEntry entry) {
    final current = _items[entry.id];
    if (_disposed || current == null) return;
    _items[entry.id] = current.copyWith(isPinned: !current.isPinned);
    _notify();
  }

  void delete(RecentContentEntry entry) {
    if (_disposed) return;
    // Discard captures already queued for this entry, including a delayed
    // screenshot-file upgrade. A later deliberate copy may enter again.
    if (_drainWork != null) _deletedBefore[entry.id] = _incomingSerial;
    _remove(entry.id);
    _notify();
  }

  void _remove(String id) {
    _items.remove(id);
    _bytes -= _payloads.remove(id)?.bytes.length ?? 0;
  }

  void clear({bool includePinned = false}) {
    if (_disposed) return;
    _generation++;
    _queue.clear();
    _queueBytes = 0;
    for (final entry in _items.values.toList()) {
      if (includePinned || !entry.isPinned) _remove(entry.id);
    }
    _error = null;
    _notify();
  }

  Future<void> setPaused(bool paused) async {
    if (_disposed || _paused == paused) return;
    _paused = paused;
    _generation++;
    _queue.clear();
    _queueBytes = 0;
    _notify();
    if (!enabled || !_started) return;
    try {
      await _transport.setPaused(paused);
    } on Object {
      if (!_disposed) _error = '记录状态未能同步，当前暂停状态仍会保留';
      _notify();
    }
  }

  static bool _isPng(Uint8List bytes) =>
      bytes.length >= 8 &&
      listEquals(bytes.sublist(0, 8), const [137, 80, 78, 71, 13, 10, 26, 10]);

  static Future<Uint8List?> imageThumbnail(Uint8List bytes) async {
    final buffer = await ui.ImmutableBuffer.fromUint8List(bytes);
    ui.ImageDescriptor? descriptor;
    ui.Codec? codec;
    ui.Image? image;
    try {
      descriptor = await ui.ImageDescriptor.encoded(buffer);
      final width = descriptor.width, height = descriptor.height;
      if (width <= 0 ||
          height <= 0 ||
          width > 16384 ||
          height > 16384 ||
          width * height > 40 * 1024 * 1024) {
        return null;
      }
      final factor = 320 / (width > height ? width : height);
      codec = await descriptor.instantiateCodec(
        targetWidth: factor < 1
            ? (width * factor).round().clamp(1, 320)
            : width,
        targetHeight: factor < 1
            ? (height * factor).round().clamp(1, 320)
            : height,
      );
      image = (await codec.getNextFrame()).image;
      final data = await image.toByteData(format: ui.ImageByteFormat.png);
      return data?.buffer.asUint8List();
    } finally {
      image?.dispose();
      codec?.dispose();
      descriptor?.dispose();
      buffer.dispose();
    }
  }

  void _notify() {
    if (!_disposed) notifyListeners();
  }

  @visibleForTesting
  Future<void> get settled async {
    while (_drainWork != null) {
      await _drainWork;
    }
  }

  @override
  void dispose() {
    if (_disposed) return;
    _disposed = true;
    _generation++;
    _queue.clear();
    _items.clear();
    _payloads.clear();
    _deletedBefore.clear();
    _bytes = _queueBytes = 0;
    unawaited(_subscription?.cancel());
    if (_started) unawaited(_transport.stop().catchError((Object _) {}));
    super.dispose();
  }
}

class _Payload {
  _Payload(this.mime, this.bytes);
  final String mime;
  final Uint8List bytes;
}

class _Incoming {
  _Incoming({
    required this.generation,
    required this.mime,
    required this.capturedAt,
    this.bytes,
    this.filePath,
    this.screenshot = false,
  });
  final int generation;
  late final int serial;
  final String mime;
  final DateTime capturedAt;
  final Uint8List? bytes;
  final String? filePath;
  final bool screenshot;
}
