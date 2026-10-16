// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:convert';
import 'dart:io';
import '../models.dart';
import 'clarion_ipc.dart';
import 'identity.dart';
import 'user_directories.dart';

typedef WallpaperProcessStarter =
    Future<Process> Function(
      String executable,
      List<String> arguments,
      Map<String, String> environment,
    );

/// One owned swaybg provider displays a static image on every output. Other
/// wallpaper processes are never searched for, signalled, or replaced.
class WallpaperService {
  WallpaperService({
    String? directoryPath,
    File? preferenceFile,
    this.backendExecutable = '/usr/bin/swaybg',
    WallpaperProcessStarter? processStarter,
    Future<int> Function()? outputCounter,
    Map<String, String>? environment,
    this.startupTimeout = const Duration(seconds: 12),
  }) : environment = Map.unmodifiable(environment ?? Platform.environment),
       directoryPath = Directory(
         directoryPath ??
             '${userHomeDirectory(environment ?? Platform.environment)}/图片/Wallpapers',
       ).absolute.path,
       preferenceFile =
           preferenceFile ??
           _defaultPreferenceFile(environment ?? Platform.environment),
       _processStarter = processStarter ?? _startProcess,
       _outputCounter =
           outputCounter ??
           (() => _clarionOutputCount(environment ?? Platform.environment)) {
    unawaited(_refreshAvailability());
  }
  final String directoryPath;
  final File preferenceFile;
  final String backendExecutable;
  final Duration startupTimeout;
  final Map<String, String> environment;
  final WallpaperProcessStarter _processStarter;
  final Future<int> Function() _outputCounter;
  final _changes = StreamController<void>.broadcast();
  Future<void> _selectionWork = Future.value();
  Future<List<ShellWallpaper>>? _scanWork;
  Future<bool>? _availabilityWork;
  bool _available = false;
  int _selectionRevision = 0;
  _WallpaperProvider? _provider;
  _WallpaperProvider? _candidate;
  ShellWallpaper? _providerImage;
  String? _persistedPath;
  bool _disposed = false;
  String? selectedPath;
  String? error;
  int? get ownedPid => _provider?.process.pid;
  Stream<void> get changes => _changes.stream;
  // Widget builds must not issue synchronous filesystem work. Refresh when the
  // service starts, the catalogue is refreshed, or an image is applied.
  bool get available => _available;

  Future<bool> _refreshAvailability() =>
      _availabilityWork ??= _readAvailability().whenComplete(() {
        _availabilityWork = null;
      });

  Future<bool> _readAvailability() async {
    bool available;
    try {
      final stat = await File(backendExecutable).stat();
      available =
          stat.type == FileSystemEntityType.file && stat.mode & 0x49 != 0;
    } on FileSystemException {
      available = false;
    }
    if (!_disposed && available != _available) {
      _available = available;
      _changed();
    }
    return available;
  }

  static const extensions = {
    'png',
    'jpg',
    'jpeg',
    'webp',
    'bmp',
    'gif',
    'tif',
    'tiff',
  };
  static const maximumFileBytes = 128 * 1024 * 1024;
  static const maximumCatalogEntries = 1000;

  Future<List<ShellWallpaper>> scan() {
    if (_disposed) return Future.error(StateError('壁纸服务已关闭'));
    // Concurrent consumers share one walk, but a later refresh always restats
    // files: directory mtime alone does not detect in-place image replacement.
    return _scanWork ??= _scan().whenComplete(() => _scanWork = null);
  }

  Future<List<ShellWallpaper>> _scan() async {
    _ensureActive();
    unawaited(_refreshAvailability());
    final result = <ShellWallpaper>[];
    final directory = Directory(directoryPath);
    if (!await directory.exists()) {
      throw FileSystemException('壁纸目录不存在或不可读取', directoryPath);
    }
    var visited = 0;
    await for (final entry in directory.list(followLinks: false)) {
      _ensureActive();
      if (++visited > 10000 || result.length == maximumCatalogEntries) break;
      if (entry is! File || !_imageExtension(entry.path)) continue;
      try {
        final stat = await File(entry.path).stat();
        if (stat.type != FileSystemEntityType.file ||
            stat.size <= 0 ||
            stat.size > maximumFileBytes) {
          continue;
        }
        final file = await File(entry.path).open();
        await file.close();
        final name = entry.path.substring(entry.path.lastIndexOf('/') + 1);
        result.add(
          ShellWallpaper(
            path: File(entry.path).absolute.path,
            name: name,
            sizeBytes: stat.size,
            modified: stat.modified,
          ),
        );
      } on FileSystemException {
        continue;
      }
    }
    result.sort((a, b) => a.name.toLowerCase().compareTo(b.name.toLowerCase()));
    return List.unmodifiable(result);
  }

  /// Preview reads the remembered selection but does not auto-apply it.
  Future<void> restore({bool applySaved = false}) async {
    _ensureActive();
    final revision = _selectionRevision;
    if (!await preferenceFile.exists()) return;
    if (await preferenceFile.length() > 65536) {
      throw const FormatException('壁纸设置文件过大');
    }
    final json = jsonDecode(await preferenceFile.readAsString());
    if (json is! Map || json['version'] != 1 || json['path'] is! String) {
      throw const FormatException('壁纸设置文件格式无效');
    }
    final entry = await _validate(json['path'] as String);
    _ensureActive();
    // A slow startup read must never replace a newer explicit selection.
    if (revision != _selectionRevision) return;
    _persistedPath = entry.path;
    if (applySaved) {
      await apply(entry, persist: false);
    } else {
      selectedPath = entry.path;
      _changed();
    }
  }

  Future<void> apply(ShellWallpaper entry, {bool persist = true}) {
    _selectionRevision++;
    final result = _selectionWork.then((_) => _apply(entry, persist));
    // Keep the serial queue usable after a rejected selection, while returning
    // that failure to its caller. Failed actions are never silently replayed.
    _selectionWork = result.then<void>(
      (_) {},
      onError: (Object e, StackTrace s) {},
    );
    return result;
  }

  Future<void> _apply(ShellWallpaper entry, bool persist) async {
    _ensureActive();
    final image = await _validate(entry.path);
    _ensureActive();
    final displayed = _providerImage;
    if (_provider != null &&
        displayed?.path == image.path &&
        displayed?.sizeBytes == image.sizeBytes &&
        displayed?.modified == image.modified &&
        (!persist || _persistedPath == image.path)) {
      // Keep the already decoded, healthy provider. File revision is checked
      // above, so rewriting the same path still starts a fresh decode.
      _provider!.ensureHealthy();
      if (selectedPath != image.path || error != null) {
        selectedPath = image.path;
        error = null;
        _changed();
      }
      return;
    }
    if (!await _refreshAvailability()) {
      throw StateError('未找到可用的 swaybg 壁纸后端');
    }
    if (environment['WAYLAND_DISPLAY']?.isNotEmpty != true) {
      throw StateError('Wayland 会话不可用');
    }
    final outputs = await _outputCounter();
    _ensureActive();
    if (outputs < 1) throw StateError('当前没有可用的显示器');
    final process = await _processStarter(
      backendExecutable,
      ['--image', image.path, '--mode', 'fill'],
      {...environment, 'WAYLAND_DEBUG': 'client'},
    );
    final candidate = _WallpaperProvider(
      process,
      outputs: outputs,
      timeout: startupTimeout,
    );
    _candidate = candidate;
    Directory? staging;
    try {
      _ensureActive();
      await candidate.ready;
      candidate.ensureHealthy();
      _ensureActive();
      File? pendingPreference;
      if (persist) {
        await preferenceFile.parent.create(recursive: true);
        staging = await preferenceFile.parent.createTemp('.wallpaper-');
        pendingPreference = File('${staging.path}/wallpaper.json');
        await pendingPreference.writeAsString(
          '${jsonEncode({'version': 1, 'path': image.path, 'mode': 'fill'})}\n',
          flush: true,
        );
      }
      candidate.ensureHealthy();
      _ensureActive();
      // No event-loop gap separates the final health check, atomic preference
      // replacement and provider promotion. A cancelled/failed candidate cannot
      // overwrite the remembered image while leaving the old provider active.
      pendingPreference?.renameSync(preferenceFile.path);
      if (persist) _persistedPath = image.path;
      final previous = _provider;
      _provider = candidate;
      _providerImage = image;
      _candidate = null;
      selectedPath = image.path;
      error = null;
      candidate.onUnexpectedExit = (message) {
        if (_disposed || !identical(_provider, candidate)) return;
        _provider = null;
        _providerImage = null;
        error = message;
        _changed();
      };
      _changed();
      // Only this service's previous provider is retired after successful
      // decode/submission and atomic persistence; an unrelated daemon survives.
      await previous?.stop();
    } on Object catch (e) {
      if (identical(_candidate, candidate)) _candidate = null;
      await candidate.stop();
      error = '$e';
      _changed();
      rethrow;
    } finally {
      if (staging != null) {
        try {
          await staging.delete(recursive: true);
        } on FileSystemException {
          /* A leftover staging directory is harmless. */
        }
      }
    }
  }

  Future<ShellWallpaper> _validate(String path) async {
    if (!path.startsWith('/') ||
        path.contains('\u0000') ||
        !_imageExtension(path)) {
      throw const FormatException('请选择绝对路径的静态图片');
    }
    final file = File(path);
    final stat = await file.stat();
    if (stat.type != FileSystemEntityType.file ||
        stat.size <= 0 ||
        stat.size > maximumFileBytes) {
      throw FileSystemException('图片不存在、不是普通文件或超过 128 MiB', path);
    }
    final handle = await file.open();
    await handle.close();
    return ShellWallpaper(
      path: file.absolute.path,
      name: path.substring(path.lastIndexOf('/') + 1),
      sizeBytes: stat.size,
      modified: stat.modified,
    );
  }

  void _ensureActive() {
    if (_disposed) throw StateError('壁纸服务已关闭');
  }

  void _changed() {
    if (!_disposed) _changes.add(null);
  }

  void dispose() {
    if (_disposed) return;
    _disposed = true;
    final providers = {_provider, _candidate}..remove(null);
    _provider = null;
    _providerImage = null;
    _candidate = null;
    for (final provider in providers) {
      unawaited(provider!.stop());
    }
    unawaited(_changes.close());
  }
}

bool _imageExtension(String path) => WallpaperService.extensions.contains(
  path.substring(path.lastIndexOf('.') + 1).toLowerCase(),
);
File _defaultPreferenceFile(Map<String, String> environment) =>
    File('${shellConfigDirectory(environment)}/wallpaper.json');

Future<Process> _startProcess(
  String executable,
  List<String> arguments,
  Map<String, String> environment,
) => Process.start(
  executable,
  arguments,
  environment: environment,
  includeParentEnvironment: false,
);
Future<int> _clarionOutputCount(Map<String, String> environment) async {
  final ipc = ClarionIpc(socketPath: ipcSocketPath(environment));
  try {
    final reply = await ipc.request('Outputs');
    if (reply is! Map || reply['Outputs'] is! Map) {
      throw StateError('无法读取显示器状态');
    }
    return (reply['Outputs'] as Map).values
        .where((value) => value is Map && value['logical'] != null)
        .length;
  } finally {
    ipc.dispose();
  }
}

/// Readiness means this candidate successfully decoded and submitted a real
/// buffer on each initial output. Swaybg has no frame callbacks; protocol trace
/// is not a claim of presentation. Actual pixels are covered by nested tests.
class _WallpaperProvider {
  _WallpaperProvider(
    this.process, {
    required int outputs,
    required Duration timeout,
  }) : _requiredOutputs = outputs {
    // Disposal can race a process-start future before the caller awaits ready.
    // Observe rejection immediately; await ready still receives the same error.
    unawaited(
      _ready.future.then<void>((_) {}, onError: (Object e, StackTrace s) {}),
    );
    _deadline = Timer(timeout, () {
      _fail('壁纸加载超时，请检查图片和显示器');
    });
    _stdout = process.stdout.listen((_) {}, onError: (Object _) {});
    _stderr = boundedJsonLines(process.stderr, maximum: 32 * 1024).listen(
      _line,
      onError: (Object e) {
        _fail('壁纸后端输出无效：$e');
      },
      onDone: () {
        if (!_ready.isCompleted) _fail('壁纸后端未完成图片提交');
      },
    );
    unawaited(
      process.exitCode.then((code) {
        _exited = true;
        _deadline.cancel();
        _fail('壁纸后端已退出（$code）${_diagnostic.isEmpty ? '' : '：$_diagnostic'}');
      }),
    );
  }
  final Process process;
  final int _requiredOutputs;
  final _ready = Completer<void>();
  final _attached = <String>{};
  final _committed = <String>{};
  late final StreamSubscription<List<int>> _stdout;
  late final StreamSubscription<String> _stderr;
  late final Timer _deadline;
  String _diagnostic = '';
  String? _failure;
  bool _exited = false;
  bool _stopping = false;
  Future<void>? _stopWork;
  void Function(String)? onUnexpectedExit;
  Future<void> get ready => _ready.future;
  static final _attach = RegExp(
    r'->\s+wl_surface[@#](\d+)\.attach\(\s*wl_buffer[@#]\d+',
  );
  static final _commit = RegExp(r'->\s+wl_surface[@#](\d+)\.commit\(');
  static final _decodeError = RegExp(
    r'(Failed to (load|read|allocate|create)|Unable to connect|Missing a required Wayland|wl_display_roundtrip failed|Error loading|Protocol error)',
    caseSensitive: false,
  );
  void _line(String line) {
    if (_decodeError.hasMatch(line)) {
      _fail('图片加载失败：${line.length > 1000 ? line.substring(0, 1000) : line}');
      return;
    }
    if (!line.contains(' -> ') &&
        line.isNotEmpty &&
        _diagnostic.length < 4096) {
      _diagnostic += '${line.length > 1024 ? line.substring(0, 1024) : line}\n';
    }
    final attach = _attach.firstMatch(line);
    if (attach != null) _attached.add(attach[1]!);
    final commit = _commit.firstMatch(line);
    if (commit != null && _attached.contains(commit[1])) {
      _committed.add(commit[1]!);
      if (_failure == null &&
          _committed.length >= _requiredOutputs &&
          !_ready.isCompleted) {
        _deadline.cancel();
        _ready.complete();
      }
    }
  }

  void _fail(String message) {
    _failure ??= message;
    if (!_ready.isCompleted) {
      _deadline.cancel();
      _ready.completeError(StateError(_failure!));
    } else if (!_stopping && onUnexpectedExit != null) {
      final notify = onUnexpectedExit;
      onUnexpectedExit = null;
      notify!(_failure!);
      unawaited(stop());
    }
  }

  void ensureHealthy() {
    if (_failure != null || _exited) throw StateError(_failure ?? '壁纸后端已退出');
  }

  Future<void> stop() => _stopWork ??= _stop();
  Future<void> _stop() async {
    _stopping = true;
    _deadline.cancel();
    if (!_ready.isCompleted) _fail('壁纸加载已取消');
    if (!_exited) {
      process.kill(ProcessSignal.sigterm);
      try {
        await process.exitCode.timeout(const Duration(milliseconds: 700));
      } on TimeoutException {
        process.kill(ProcessSignal.sigkill);
        await process.exitCode.timeout(
          const Duration(milliseconds: 700),
          onTimeout: () => -1,
        );
      }
    }
    await Future.wait([
      _stdout.cancel(),
      _stderr.cancel(),
    ]).timeout(const Duration(milliseconds: 700), onTimeout: () => <void>[]);
  }
}
