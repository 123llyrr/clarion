// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart';

import '../mac_shell_preferences.dart';
import 'application_launcher.dart';
import 'system_service.dart';

/// A direct directory child. Reading a tile never executes downloaded content.
class DockFolderEntry {
  const DockFolderEntry({
    required this.path,
    required this.name,
    this.isDirectory = false,
    this.isSymlink = false,
    this.kind = '文件',
    this.iconName = 'text-x-generic',
    this.iconPath,
    this.modified,
    this.created,
    this.added,
  });

  final String path;
  final String name;
  final bool isDirectory;
  final bool isSymlink;
  final String kind;
  final String iconName;
  final String? iconPath;
  final DateTime? modified;
  // POSIX ctime is a metadata-change time, not a creation or arrival time.
  final DateTime? created;
  final DateTime? added;
  String get id => path;
}

/// Downloads listing, watching, and GUI opening belong to one owned service.
/// The shell starts it explicitly; constructing a panel has no I/O side effects.
class DockFolderService extends ChangeNotifier {
  DockFolderService({
    String? directory,
    Map<String, String>? environment,
    CommandRunner? command,
    ApplicationLauncher? applicationLauncher,
    this.maximumEntries = 256,
    this.debounceDelay = const Duration(milliseconds: 120),
  }) : assert(maximumEntries > 0 && maximumEntries <= 256),
       _requestedDirectory = directory,
       _environment = Map.unmodifiable(environment ?? Platform.environment) {
    _command =
        command ??
        (executable, arguments) =>
            runSystemCommand(executable, arguments, environment: _environment);
    _launcher =
        applicationLauncher ?? ApplicationLauncher(environment: _environment);
  }

  final String? _requestedDirectory;
  final Map<String, String> _environment;
  final int maximumEntries;
  final Duration debounceDelay;
  late final CommandRunner _command;
  late final ApplicationLauncher _launcher;
  String? _path;
  List<DockFolderEntry> _entries = const [];
  String? _scanError;
  String? _watchError;
  bool _loading = false;
  bool _overflow = false;
  bool _disposed = false;
  bool _scanAgain = false;
  Future<void>? _startWork;
  Future<void>? _scanWork;
  StreamSubscription<FileSystemEvent>? _watcher;
  StreamSubscription<FileSystemEvent>? _parentWatcher;
  Timer? _debounce;

  String? get path => _path;
  List<DockFolderEntry> get entries => _entries;
  bool get loading => _loading;
  bool get overflow => _overflow;
  String? get error => _scanError ?? _watchError;

  Future<void> start() {
    if (_disposed) return Future.value();
    return _startWork ??= _start();
  }

  Future<void> _start() async {
    _loading = true;
    _notify();
    try {
      var requested = _requestedDirectory;
      if (requested == null) {
        try {
          final configured = (await _command('xdg-user-dir', [
            'DOWNLOAD',
          ])).trim();
          if (configured.startsWith('/') && !configured.contains('\u0000')) {
            requested = configured;
          }
        } on Object {
          // xdg-user-dirs is optional; a literal home-relative fallback is safe.
        }
        if (requested == null) {
          final home = _environment['HOME'];
          if (home == null || !home.startsWith('/')) {
            throw StateError('用户下载目录不可用');
          }
          requested = '$home/Downloads';
        }
      }
      if (_disposed) return;
      final resolved = _absolute(requested);
      if (resolved == '/') throw StateError('系统根目录不能作为下载目录');
      if (_environment['HOME'] case final home?
          when _requestedDirectory == null &&
              home.startsWith('/') &&
              resolved == _absolute(home)) {
        throw StateError('下载目录已停用，请先配置独立的下载目录');
      }
      _path = resolved;
      await _watchParent();
      await _scan();
    } on Object catch (exception) {
      if (!_disposed) _scanError = '无法读取下载目录：$exception';
    } finally {
      if (!_disposed) {
        _loading = false;
        _notify();
      }
    }
  }

  Future<void> refresh() async {
    if (_disposed) return;
    if (_startWork == null) return start();
    await _startWork;
    if (_disposed || _path == null) return;
    if (_scanWork case final running?) {
      _scanAgain = true;
      return running;
    }
    final work = _scanLoop();
    _scanWork = work;
    try {
      await work;
    } finally {
      if (identical(_scanWork, work)) _scanWork = null;
    }
  }

  Future<void> _scanLoop() async {
    do {
      _scanAgain = false;
      try {
        await _scan();
      } on Object catch (exception) {
        if (!_disposed) {
          _scanError = '无法刷新下载目录：$exception';
          _notify();
        }
      }
    } while (!_disposed && _scanAgain);
  }

  Future<void> _scan() async {
    if (_disposed || _path == null) return;
    final directory = Directory(_path!);
    if (!await directory.exists()) {
      if (_disposed) return;
      await _watcher?.cancel();
      _watcher = null;
      _entries = const [];
      _overflow = false;
      _scanError = '下载目录尚不存在';
      _notify();
      return;
    }
    if (_disposed) return;
    _installWatcher(directory);
    final children = <FileSystemEntity>[];
    var scanned = 0;
    var extra = false;
    // Stop the stream rather than materializing an unbounded directory first.
    // Hidden children also count against this filesystem-work limit.
    await for (final entity in directory.list(
      recursive: false,
      followLinks: false,
    )) {
      if (_disposed) return;
      if (scanned++ >= maximumEntries) {
        extra = true;
        break;
      }
      if (!_name(entity.path).startsWith('.')) children.add(entity);
    }
    final result = <DockFolderEntry>[];
    for (var index = 0; index < children.length && !_disposed; index += 8) {
      final end = (index + 8).clamp(0, children.length);
      final batch = await Future.wait(
        children.sublist(index, end).map(_readEntry),
      );
      result.addAll(batch.whereType<DockFolderEntry>());
    }
    if (_disposed) return;
    _entries = List.unmodifiable(result);
    _overflow = extra;
    _scanError = null;
    _notify();
  }

  void _installWatcher(Directory directory) {
    if (_watcher != null || _disposed) return;
    try {
      _watcher = directory
          .watch(recursive: false)
          .listen(
            (_) => _scheduleRefresh(),
            onError: (Object exception) {
              if (_disposed) return;
              _watchError = '目录变化监听暂不可用：$exception';
              _notify();
            },
            onDone: () {
              _watcher = null;
              _scheduleRefresh();
            },
          );
      _watchError = null;
    } on FileSystemException catch (exception) {
      _watchError = '目录变化监听暂不可用：$exception';
    }
  }

  Future<void> _watchParent() async {
    final parent = Directory(_path!).parent;
    if (!await parent.exists() || _disposed) return;
    try {
      _parentWatcher = parent.watch(recursive: false).listen((event) {
        if (_absolute(event.path) == _path) _scheduleRefresh();
      }, onError: (Object _) {});
    } on FileSystemException {
      // A readable directory can still be listed without parent-watch support.
    }
  }

  void _scheduleRefresh() {
    if (_disposed) return;
    _debounce?.cancel();
    _debounce = Timer(debounceDelay, () {
      _debounce = null;
      unawaited(refresh());
    });
  }

  Future<DockFolderEntry?> _readEntry(FileSystemEntity entity) async {
    try {
      final type = await FileSystemEntity.type(entity.path, followLinks: false);
      if (type == FileSystemEntityType.notFound) return null;
      final link = type == FileSystemEntityType.link;
      final directory = type == FileSystemEntityType.directory;
      // Do not follow links for metadata or recurse into their target directory.
      final modified = link
          ? null
          : (await FileStat.stat(entity.path)).modified;
      final name = _name(entity.path);
      final (kind, icon) = _kind(name, directory, link);
      return DockFolderEntry(
        path: _absolute(entity.path),
        name: name,
        isDirectory: directory,
        isSymlink: link,
        kind: kind,
        iconName: icon,
        modified: modified,
      );
    } on FileSystemException {
      // A download can be renamed or removed while the scan is in flight.
      return null;
    }
  }

  bool supportsSort(MacDownloadsSort sort) => switch (sort) {
    MacDownloadsSort.dateAdded =>
      _entries.isNotEmpty && _entries.every((e) => e.added != null),
    MacDownloadsSort.dateCreated =>
      _entries.isNotEmpty && _entries.every((e) => e.created != null),
    _ => true,
  };

  /// The menu checkmark and file ordering share this metadata fallback.
  MacDownloadsSort effectiveSort(MacDownloadsSort requested) =>
      supportsSort(requested) ? requested : MacDownloadsSort.name;

  List<DockFolderEntry> sortedEntries(MacDownloadsSort requested) {
    final sort = effectiveSort(requested);
    final result = List<DockFolderEntry>.of(_entries);
    result.sort((a, b) {
      final order = switch (sort) {
        MacDownloadsSort.name => a.name.toLowerCase().compareTo(
          b.name.toLowerCase(),
        ),
        MacDownloadsSort.kind => a.kind.compareTo(b.kind),
        MacDownloadsSort.dateModified => _compareDates(a.modified, b.modified),
        MacDownloadsSort.dateAdded => _compareDates(a.added, b.added),
        MacDownloadsSort.dateCreated => _compareDates(a.created, b.created),
      };
      return order == 0 ? a.path.compareTo(b.path) : order;
    });
    return List.unmodifiable(result);
  }

  Future<void> open(DockFolderEntry entry) async {
    if (_disposed) throw StateError('下载目录服务已关闭');
    if (!_entries.any((current) => current.id == entry.id)) {
      throw StateError('此下载项目已移除，请刷新后重试');
    }
    if (await FileSystemEntity.type(entry.path, followLinks: false) ==
        FileSystemEntityType.notFound) {
      throw StateError('此下载项目已不存在');
    }
    if (_disposed) throw StateError('下载目录服务已关闭');
    await _launcher.launch('gio', ['open', '--', entry.path]);
  }

  Future<void> openFolder() async {
    if (_disposed || _path == null) throw StateError('下载目录不可用');
    await _launcher.launch('gio', ['open', '--', _path!]);
  }

  void _notify() {
    if (!_disposed) notifyListeners();
  }

  @override
  void dispose() {
    if (_disposed) return;
    _disposed = true;
    _debounce?.cancel();
    unawaited(_watcher?.cancel());
    unawaited(_parentWatcher?.cancel());
    super.dispose();
  }
}

int _compareDates(DateTime? a, DateTime? b) {
  if (a == null) return b == null ? 0 : 1;
  if (b == null) return -1;
  return b.compareTo(a); // Latest first; ties remain stable by path.
}

String _absolute(String path) {
  if (!path.startsWith('/') || path.contains('\u0000')) {
    throw ArgumentError('下载目录必须为绝对路径');
  }
  final normalized = Uri.directory(path).normalizePath().toFilePath();
  return normalized.length > 1 && normalized.endsWith('/')
      ? normalized.substring(0, normalized.length - 1)
      : normalized;
}

String _name(String path) => path.substring(path.lastIndexOf('/') + 1);

(String, String) _kind(String name, bool directory, bool link) {
  if (directory) return ('文件夹', 'folder');
  if (link) return ('快捷方式', 'emblem-symbolic-link');
  final extension = name.contains('.')
      ? name.split('.').last.toLowerCase()
      : '';
  return switch (extension) {
    'png' ||
    'jpg' ||
    'jpeg' ||
    'gif' ||
    'webp' ||
    'svg' ||
    'bmp' ||
    'heic' => ('图像', 'image-x-generic'),
    'pdf' => ('PDF 文稿', 'application-pdf'),
    'mp3' ||
    'wav' ||
    'flac' ||
    'ogg' ||
    'm4a' ||
    'aac' => ('音频', 'audio-x-generic'),
    'mp4' || 'mkv' || 'webm' || 'mov' || 'avi' => ('视频', 'video-x-generic'),
    'zip' ||
    'gz' ||
    'xz' ||
    'tar' ||
    '7z' ||
    'rar' ||
    'deb' ||
    'rpm' => ('归档', 'package-x-generic'),
    'odt' || 'doc' || 'docx' || 'pages' => ('文稿', 'x-office-document'),
    'xls' || 'xlsx' || 'ods' || 'numbers' => ('电子表格', 'x-office-spreadsheet'),
    'ppt' || 'pptx' || 'odp' || 'key' => ('演示文稿', 'x-office-presentation'),
    'desktop' || 'appimage' || 'exe' => ('应用文件', 'application-x-executable'),
    _ => ('文件', 'text-x-generic'),
  };
}
