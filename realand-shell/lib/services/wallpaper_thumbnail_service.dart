// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:collection';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import '../models.dart';
import 'user_directories.dart';

typedef WallpaperThumbnailProcessStarter =
    Future<Process> Function(
      String executable,
      List<String> arguments,
      String workingDirectory,
    );

/// A thumbnail is only a gallery preview. The wallpaper provider always gets
/// the original file, never this cache path.
class WallpaperThumbnail {
  const WallpaperThumbnail.fallback()
    : file = null,
      invalid = false,
      _source = null;
  const WallpaperThumbnail.preview(this.file) : invalid = false, _source = null;
  const WallpaperThumbnail._(this.file, this.invalid, this._source);
  final File? file;
  final bool invalid;
  final _Source? _source;
  WallpaperThumbnail get original => WallpaperThumbnail._(null, false, _source);
}

/// Bounded, disposable workers and persistent revisioned gallery previews.
/// No process or filesystem scan starts until a visible tile requests a file.
class WallpaperThumbnailService {
  WallpaperThumbnailService({
    Directory? cacheDirectory,
    this.executable = '/usr/bin/vipsthumbnail',
    bool? enabled,
    this.maximumEntries = 512,
    this.maximumBytes = 128 * 1024 * 1024,
    this.generationTimeout = const Duration(seconds: 20),
    WallpaperThumbnailProcessStarter? processStarter,
  }) : cacheDirectory = cacheDirectory ?? _defaultCacheDirectory(),
       enabled = enabled ?? !Platform.environment.containsKey('FLUTTER_TEST'),
       _start = processStarter ?? _startProcess,
       assert(maximumEntries > 0),
       assert(maximumBytes > 0);

  final Directory cacheDirectory;
  final String executable;
  final bool enabled;
  final int maximumEntries;
  final int maximumBytes;
  final Duration generationTimeout;
  final WallpaperThumbnailProcessStarter _start;
  final _jobs = <String, _Job>{};
  final _queue = Queue<_Job>();
  final _owned = <Process>{};
  final _stops = <Process, Future<void>>{};
  final _statusWork = <String, Future<void>>{};
  final _cacheReaders = <String, int>{};
  Future<void>? _prepared;
  Future<List<String>?>? _command;
  Future<void>? _maintenance;
  Timer? _maintenanceTimer;
  int _running = 0;
  bool _disposed = false;

  static const width = 320;
  static const maximumPreviewBytes = 4 * 1024 * 1024;
  static final _cacheName = RegExp(r'^[0-9a-f]{16}$');

  Future<WallpaperThumbnail> resolve(
    ShellWallpaper image, {
    bool Function()? stillWanted,
  }) async {
    if (!enabled || _disposed) return const WallpaperThumbnail.fallback();
    try {
      final source = await _Source.read(image.path);
      if (_disposed || !(stillWanted?.call() ?? true)) {
        return const WallpaperThumbnail.fallback();
      }
      // Warm hits must not wait behind two slow, cold source decodes. Only
      // cache misses enter the bounded worker queue.
      _cacheReaders.update(source.key, (value) => value + 1, ifAbsent: () => 1);
      try {
        await _prepare();
        final cached = await _readEntry(source);
        if (_disposed || !(stillWanted?.call() ?? true)) {
          return const WallpaperThumbnail.fallback();
        }
        if (cached != null) {
          return await source.unchanged()
              ? cached
              : const WallpaperThumbnail.fallback();
        }
      } finally {
        final readers = _cacheReaders[source.key]! - 1;
        if (readers == 0) {
          _cacheReaders.remove(source.key);
        } else {
          _cacheReaders[source.key] = readers;
        }
      }
      final waiter = _Waiter(stillWanted);
      var job = _jobs[source.key];
      if (job != null && job.source.identity == source.identity) {
        job.waiters.add(waiter);
      } else {
        // Only the visible/cache-extent cells enqueue work. Fast scrolling can
        // leave obsolete cells in this queue; they are skipped before spawning.
        if (_queue.length >= 1024) return const WallpaperThumbnail.fallback();
        job = _Job(source, waiter);
        _jobs[source.key] = job;
        _queue.add(job);
        _drain();
      }
      return await waiter.done.future;
    } on Object {
      return const WallpaperThumbnail.fallback();
    }
  }

  void _drain() {
    while (!_disposed && _running < 2 && _queue.isNotEmpty) {
      final job = _queue.removeFirst();
      if (!job.wanted) {
        _finish(job, const WallpaperThumbnail.fallback());
        continue;
      }
      _running++;
      unawaited(
        _run(job)
            .then(
              (result) {
                _finish(job, result);
              },
              onError: (Object _, StackTrace _) {
                _finish(job, const WallpaperThumbnail.fallback());
              },
            )
            .whenComplete(() {
              _running--;
              _drain();
            }),
      );
    }
  }

  void _finish(_Job job, WallpaperThumbnail result) {
    if (identical(_jobs[job.source.key], job)) _jobs.remove(job.source.key);
    for (final waiter in job.waiters) {
      if (!waiter.done.isCompleted) waiter.done.complete(result);
    }
  }

  Future<void> _prepare() => _prepared ??= () async {
    await cacheDirectory.create(recursive: true);
    // Once per service, not once per tile. Cache hits need not wait for a full
    // budget scan; currently requested keys are protected from eviction.
    unawaited(trim());
  }();

  Future<WallpaperThumbnail> _run(_Job job) async {
    final source = job.source;
    await _prepare();
    if (_disposed || !job.wanted) return const WallpaperThumbnail.fallback();
    final cached = await _readEntry(source);
    if (cached != null) {
      return await source.unchanged()
          ? cached
          : const WallpaperThumbnail.fallback();
    }
    final command = await (_command ??= _findCommand());
    if (_disposed || !job.wanted) return const WallpaperThumbnail.fallback();
    if (command == null) return const WallpaperThumbnail.fallback();
    if (!await source.unchanged()) return const WallpaperThumbnail.fallback();

    final staging = await cacheDirectory.createTemp('.build-');
    Process? process;
    StreamSubscription<List<int>>? stdout;
    StreamSubscription<List<int>>? stderr;
    try {
      // libvips treats filename suffix brackets as loader options. A safe local
      // symlink makes every original basename literal, including brackets/%/-.
      await Link(
        '${staging.path}/source.${source.extension}',
      ).create(source.path);
      if (_disposed || !job.wanted) return const WallpaperThumbnail.fallback();
      process = await _start(command.first, [
        ...command.skip(1),
        '--vips-concurrency=1',
        '--vips-cache-max=0',
        '--size=${width}x>',
        '--path=thumbnail.png',
        '--',
        'source.${source.extension}',
      ], staging.path);
      _owned.add(process);
      // Drain without retaining unbounded diagnostics from a failed decoder.
      stdout = process.stdout.listen((_) {}, onError: (Object _) {});
      stderr = process.stderr.listen((_) {}, onError: (Object _) {});
      if (_disposed) {
        await _stop(process);
        return const WallpaperThumbnail.fallback();
      }
      final code = await process.exitCode.timeout(generationTimeout);
      if (_disposed || !job.wanted) return const WallpaperThumbnail.fallback();
      if (!await source.unchanged()) return const WallpaperThumbnail.fallback();
      final preview = File('${staging.path}/thumbnail.png');
      final bytes = await _validPreview(preview);
      if (code != 0 || bytes == null) {
        await _publishStatus(source, 'fallback');
        return WallpaperThumbnail._(null, false, source);
      }
      await Link('${staging.path}/source.${source.extension}').delete();
      await _writeMetadata(staging, source, 'ready', bytes: bytes);
      if (_disposed || !job.wanted || !await source.unchanged()) {
        return const WallpaperThumbnail.fallback();
      }
      final target = await _publish(staging, source);
      _scheduleTrim();
      return WallpaperThumbnail._(
        File('${target.path}/thumbnail.png'),
        false,
        source,
      );
    } on Object {
      if (process != null) await _stop(process);
      if (!_disposed && await source.unchanged()) {
        try {
          await _publishStatus(source, 'fallback');
        } on Object {
          /* Read-only/unavailable cache falls back to original. */
        }
      }
      return WallpaperThumbnail._(null, false, source);
    } finally {
      if (process != null) {
        _owned.remove(process);
        _stops.remove(process);
      }
      await stdout?.cancel();
      await stderr?.cancel();
      await _deleteDirectory(staging);
    }
  }

  Future<List<String>?> _findCommand() async {
    Future<bool> executableAt(String path) async {
      final stat = await File(path).stat();
      return stat.type == FileSystemEntityType.file && stat.mode & 0x49 != 0;
    }

    if (!await executableAt(executable)) return null;
    return [
      if (await executableAt('/usr/bin/nice')) ...['/usr/bin/nice', '-n', '15'],
      if (await executableAt('/usr/bin/ionice')) ...[
        '/usr/bin/ionice',
        '-c',
        '3',
      ],
      executable,
    ];
  }

  Future<WallpaperThumbnail?> _readEntry(_Source source) async {
    final directory = Directory('${cacheDirectory.path}/${source.key}');
    final metadata = File('${directory.path}/metadata.json');
    try {
      if (await metadata.length() > 16384) return null;
      final data = jsonDecode(await metadata.readAsString());
      if (data is! Map || data['identity'] != source.identity) return null;
      final state = data['state'];
      if (state == 'ready') {
        final file = File('${directory.path}/thumbnail.png');
        final bytes = await _validPreview(file);
        if (bytes == null || bytes != data['bytes']) return null;
        await metadata.setLastModified(DateTime.now());
        return WallpaperThumbnail._(file, false, source);
      }
      if (state == 'fallback' || state == 'invalid') {
        await metadata.setLastModified(DateTime.now());
        return WallpaperThumbnail._(null, state == 'invalid', source);
      }
    } on Object {
      /* A torn/corrupt entry is a miss, never the original image. */
    }
    return null;
  }

  /// Cache decoding failure must fall back to the original once. Only failure
  /// of that original may suppress a preview until the source revision changes.
  Future<void> reportFailure(
    WallpaperThumbnail result, {
    required bool original,
  }) async {
    final source = result._source;
    if (_disposed || source == null) return;
    try {
      if (await source.unchanged()) {
        await _publishStatus(source, original ? 'invalid' : 'fallback');
      }
    } on Object {
      /* Failure reporting is best effort, never a retry loop. */
    }
  }

  Future<void> _publishStatus(_Source source, String state) {
    final previous = _statusWork[source.key] ?? Future<void>.value();
    final work = previous.then(
      (_) => _writeStatus(source, state),
      onError: (Object _, StackTrace _) => _writeStatus(source, state),
    );
    _statusWork[source.key] = work;
    void finished() {
      if (identical(_statusWork[source.key], work)) {
        _statusWork.remove(source.key);
      }
    }

    unawaited(
      work.then(
        (_) => finished(),
        onError: (Object _, StackTrace _) => finished(),
      ),
    );
    return work;
  }

  Future<void> _writeStatus(_Source source, String state) async {
    if (_disposed) return;
    if (state == 'fallback') {
      // A delayed cache-decode failure must not undo a newer failure of the
      // original. Both callbacks can arrive before either disk write finishes.
      final entry = await _readEntry(source);
      if (entry?.invalid == true) return;
    }
    final staging = await cacheDirectory.createTemp('.build-');
    try {
      await _writeMetadata(staging, source, state);
      if (!_disposed && await source.unchanged()) {
        await _publish(staging, source);
        _scheduleTrim();
      }
    } finally {
      await _deleteDirectory(staging);
    }
  }

  Future<void> _writeMetadata(
    Directory directory,
    _Source source,
    String state, {
    int? bytes,
  }) async {
    await File('${directory.path}/metadata.json').writeAsString(
      jsonEncode({'identity': source.identity, 'state': state, 'bytes': bytes}),
      flush: true,
    );
  }

  Future<Directory> _publish(Directory staging, _Source source) async {
    final target = Directory('${cacheDirectory.path}/${source.key}');
    await _deleteDirectory(target);
    return staging.rename(target.path);
  }

  void _scheduleTrim() {
    if (_disposed || _maintenanceTimer != null) return;
    _maintenanceTimer = Timer(const Duration(seconds: 1), () {
      _maintenanceTimer = null;
      unawaited(trim());
    });
  }

  /// Maintenance is serialized and debounced; never a separate directory walk
  /// per tile. Limits count negative entries as well as successful previews.
  Future<void> trim() =>
      _maintenance ??= _trim().whenComplete(() => _maintenance = null);

  Future<void> _trim() async {
    if (_disposed) return;
    try {
      final entries =
          <({Directory directory, String key, DateTime used, int bytes})>[];
      await for (final entry in cacheDirectory.list(followLinks: false)) {
        if (_disposed) return;
        if (entry is! Directory) continue;
        final key = entry.path.substring(entry.path.lastIndexOf('/') + 1);
        if (key.startsWith('.build-')) {
          final stat = await entry.stat();
          if (DateTime.now().difference(stat.modified) >
              const Duration(days: 1)) {
            await _deleteDirectory(entry);
          }
          continue;
        }
        if (!_cacheName.hasMatch(key)) continue;
        final metadata = await File('${entry.path}/metadata.json').stat();
        final preview = await File('${entry.path}/thumbnail.png').stat();
        entries.add((
          directory: entry,
          key: key,
          used: metadata.modified,
          bytes:
              (metadata.size > 0 ? metadata.size : 0) +
              (preview.size > 0 ? preview.size : 0),
        ));
      }
      entries.sort((a, b) => a.used.compareTo(b.used));
      var count = entries.length;
      var bytes = entries.fold<int>(0, (sum, entry) => sum + entry.bytes);
      for (final entry in entries) {
        if (_disposed || (count <= maximumEntries && bytes <= maximumBytes)) {
          break;
        }
        if (_jobs.containsKey(entry.key) ||
            _cacheReaders.containsKey(entry.key)) {
          continue;
        }
        await _deleteDirectory(entry.directory);
        count--;
        bytes -= entry.bytes;
      }
    } on FileSystemException {
      /* Cache is expendable and may be removed externally. */
    }
  }

  Future<void> _stop(Process process) => _stops.putIfAbsent(process, () async {
    process.kill(ProcessSignal.sigterm);
    try {
      await process.exitCode.timeout(const Duration(milliseconds: 500));
    } on TimeoutException {
      process.kill(ProcessSignal.sigkill);
      await process.exitCode.timeout(
        const Duration(milliseconds: 500),
        onTimeout: () => -1,
      );
    }
  });

  void dispose() {
    if (_disposed) return;
    _disposed = true;
    _maintenanceTimer?.cancel();
    while (_queue.isNotEmpty) {
      _finish(_queue.removeFirst(), const WallpaperThumbnail.fallback());
    }
    for (final process in _owned.toList()) {
      unawaited(_stop(process));
    }
  }
}

class _Source {
  _Source(this.path, this.size, this.modified);
  final String path;
  final int size;
  final DateTime modified;
  String get extension =>
      path.substring(path.lastIndexOf('.') + 1).toLowerCase();
  String get identity => jsonEncode([
    1,
    path,
    size,
    modified.microsecondsSinceEpoch,
    WallpaperThumbnailService.width,
  ]);
  // This is a filename index, not a trust hash: every hit verifies the full
  // identity above, including the original path, before returning pixels.
  String get key {
    var hash = 0x4bf29ce484222325;
    for (final byte in utf8.encode(identity)) {
      hash = ((hash ^ byte) * 0x100000001b3) & 0x7fffffffffffffff;
    }
    return hash.toRadixString(16).padLeft(16, '0');
  }

  static Future<_Source> read(String path) async {
    final file = File(path);
    final stat = await file.stat();
    final extension = path.substring(path.lastIndexOf('.') + 1).toLowerCase();
    if (!path.startsWith('/') ||
        !const {
          'png',
          'jpg',
          'jpeg',
          'webp',
          'bmp',
          'gif',
          'tif',
          'tiff',
        }.contains(extension) ||
        stat.type != FileSystemEntityType.file ||
        stat.size <= 0 ||
        stat.size > 128 * 1024 * 1024) {
      throw const FormatException('Invalid wallpaper file');
    }
    return _Source(file.absolute.path, stat.size, stat.modified);
  }

  Future<bool> unchanged() async {
    try {
      return (await read(path)).identity == identity;
    } on Object {
      return false;
    }
  }
}

class _Waiter {
  _Waiter(this.stillWanted);
  final bool Function()? stillWanted;
  final done = Completer<WallpaperThumbnail>();
}

class _Job {
  _Job(this.source, _Waiter waiter) : waiters = [waiter];
  final _Source source;
  final List<_Waiter> waiters;
  bool get wanted =>
      waiters.any((waiter) => waiter.stillWanted?.call() ?? true);
}

Future<int?> _validPreview(File file) async {
  try {
    final stat = await file.stat();
    if (stat.type != FileSystemEntityType.file ||
        stat.size < 24 ||
        stat.size > WallpaperThumbnailService.maximumPreviewBytes) {
      return null;
    }
    final input = await file.open();
    late Uint8List header;
    try {
      header = await input.read(24);
    } finally {
      await input.close();
    }
    const png = [137, 80, 78, 71, 13, 10, 26, 10];
    if (header.length < 24 ||
        List.generate(8, (i) => header[i] == png[i]).contains(false)) {
      return null;
    }
    final dimensions = ByteData.sublistView(header);
    final width = dimensions.getUint32(16), height = dimensions.getUint32(20);
    if (width == 0 ||
        width > WallpaperThumbnailService.width ||
        height == 0 ||
        height > 16384) {
      return null;
    }
    return stat.size;
  } on FileSystemException {
    return null;
  }
}

Future<void> _deleteDirectory(Directory directory) async {
  try {
    await directory.delete(recursive: true);
  } on FileSystemException {
    /* Already gone. */
  }
}

Directory _defaultCacheDirectory() => Directory(
  '${userCacheDirectory(Platform.environment)}/realand-shell/wallpaper-thumbnails-v1',
);

Future<Process> _startProcess(
  String executable,
  List<String> arguments,
  String workingDirectory,
) => Process.start(executable, arguments, workingDirectory: workingDirectory);
