// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:flutter/foundation.dart';

import '../models.dart';
import '../services/application_launcher.dart';
import '../services/desktop_apps.dart';
import '../services/system_service.dart';
import 'desktop_entry.dart';
import 'desktop_file_details.dart';

/// Desktop files are independent of the compositor's virtual workspaces.
/// Startup and watching only read; mutations require an explicit method call.
class DesktopService extends ChangeNotifier {
  DesktopService({
    String? root,
    Map<String, String>? environment,
    CommandRunner? command,
    ApplicationLauncher? applicationLauncher,
    this.debounceDelay = const Duration(milliseconds: 120),
  }) : _requestedRoot = root,
       _environment = Map.unmodifiable(environment ?? Platform.environment) {
    _applicationLauncher =
        applicationLauncher ?? ApplicationLauncher(environment: _environment);
    _command =
        command ??
        (executable, arguments) => runSystemCommand(
          executable,
          arguments,
          environment: environment == null ? null : _environment,
        );
  }

  final String? _requestedRoot;
  final Map<String, String> _environment;
  late final CommandRunner _command;
  late final ApplicationLauncher _applicationLauncher;
  final Duration debounceDelay;
  List<DesktopEntry> _entries = const [];
  String? _directoryPath;
  String? _canonicalRoot;
  String? _error;
  String? _operationError;
  bool _loading = false;
  bool _busy = false;
  bool _disposed = false;
  bool _rescanRequested = false;
  Future<void>? _startWork;
  Future<void>? _scanWork;
  StreamSubscription<FileSystemEvent>? _watcher;
  StreamSubscription<FileSystemEvent>? _parentWatcher;
  Timer? _debounce;
  final Set<Process> _previewProcesses = {};

  static const previewTextByteLimit = 256 * 1024;
  static const previewImageByteLimit = 8 * 1024 * 1024;

  List<String> _clipboardPaths = const [];
  String? get directoryPath => _directoryPath;
  List<DesktopEntry> get entries => _entries;
  bool get loading => _loading;
  bool get busy => _busy;
  String? get error => _operationError ?? _error;
  List<String> get clipboardPaths => _clipboardPaths;
  bool get canPaste => _clipboardPaths.isNotEmpty;

  Future<void> start() {
    if (_disposed) return Future.value();
    return _startWork ??= _start();
  }

  Future<void> _start() async {
    _loading = true;
    _notify();
    try {
      final requested =
          _requestedRoot ?? await _command('xdg-user-dir', ['DESKTOP']);
      if (_disposed) return;
      final path = _absolutePath(
        _requestedRoot == null ? requested.trim() : requested,
      );
      if (path == '/') throw StateError('不能把系统根目录作为桌面目录');
      if (_requestedRoot == null) {
        final home = _environment['HOME'];
        if (home != null &&
            home.startsWith('/') &&
            (path == _absolutePath(home) || await _sameDirectory(path, home))) {
          throw StateError('桌面目录未设置或已禁用，请设置独立的桌面目录');
        }
      }
      _directoryPath = path;
      await _watchParent();
      await _scan();
    } on Object catch (error) {
      if (!_disposed) _error = '无法读取桌面：$error';
    } finally {
      if (!_disposed) {
        _loading = false;
        _notify();
      }
    }
  }

  Future<void> refresh({bool clearError = true}) async {
    if (_disposed) return;
    if (clearError) _operationError = null;
    if (_startWork == null) return start();
    await _startWork;
    if (_disposed || _directoryPath == null) return;
    final running = _scanWork;
    if (running != null) {
      _rescanRequested = true;
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
      _rescanRequested = false;
      try {
        await _scan();
      } on Object catch (error) {
        if (!_disposed) {
          _error = '无法刷新桌面：$error';
          _notify();
        }
      }
    } while (!_disposed && _rescanRequested);
  }

  Future<void> _scan() async {
    final root = _directoryPath;
    if (_disposed || root == null) return;
    final directory = Directory(root);
    if (!await directory.exists()) {
      if (_disposed) return;
      _entries = const [];
      _error = '桌面目录尚不存在；新建文件或文件夹时将创建此目录';
      await _watcher?.cancel();
      _watcher = null;
      _notify();
      return;
    }
    await _checkRoot();
    if (_disposed) return;
    _watcher ??= directory
        .watch(recursive: false)
        .listen(
          (_) => _scheduleRefresh(),
          onError: (Object error) {
            if (_disposed) return;
            _error = '桌面目录监视失败：$error';
            _notify();
          },
          onDone: () {
            _watcher = null;
            _scheduleRefresh();
          },
        );
    final children = await directory
        .list(followLinks: false)
        .where((entity) => !_basename(entity.path).startsWith('.'))
        .toList();
    final result = <DesktopEntry>[];
    // Keep filesystem work asynchronous and bound simultaneous metadata reads.
    for (var index = 0; index < children.length && !_disposed; index += 16) {
      final end = index + 16 < children.length ? index + 16 : children.length;
      final batch = await Future.wait(
        children.sublist(index, end).map((entity) => _readEntry(entity.path)),
      );
      result.addAll(batch.whereType<DesktopEntry>());
    }
    if (_disposed) return;
    result.sort((a, b) {
      if (a.isDirectory != b.isDirectory) return a.isDirectory ? -1 : 1;
      final order = a.displayName.toLowerCase().compareTo(
        b.displayName.toLowerCase(),
      );
      return order == 0 ? a.name.compareTo(b.name) : order;
    });
    _entries = List.unmodifiable(result);
    _error = null;
    _notify();
  }

  Future<void> _watchParent() async {
    final root = _directoryPath;
    if (root == null || _disposed) return;
    final parent = Directory(_parent(root));
    if (!await parent.exists() || _disposed) return;
    _parentWatcher = parent.watch(recursive: false).listen((event) {
      if (_absolutePath(event.path) == root) _scheduleRefresh();
    }, onError: (Object _) {});
  }

  void _scheduleRefresh() {
    if (_disposed) return;
    _debounce?.cancel();
    _debounce = Timer(debounceDelay, () {
      _debounce = null;
      unawaited(refresh(clearError: false));
    });
  }

  Future<DesktopEntry?> _readEntry(String path) async {
    try {
      final type = await FileSystemEntity.type(path, followLinks: false);
      if (type == FileSystemEntityType.notFound) return null;
      final symlink = type == FileSystemEntityType.link;
      final targetType = symlink ? await FileSystemEntity.type(path) : type;
      final stat = await FileStat.stat(path);
      String? targetPath;
      if (symlink) {
        final target = await Link(path).target();
        targetPath = _absolutePath(
          target.startsWith('/') ? target : '${_parent(path)}/$target',
        );
      }
      final name = _basename(path);
      String? label;
      String? icon;
      var shortcut = false;
      if (name.endsWith('.desktop') &&
          targetType == FileSystemEntityType.file &&
          stat.size <= 1024 * 1024) {
        try {
          final metadata = parseDesktopEntry(await File(path).readAsString());
          shortcut = metadata['Type'] == 'Application';
          if (shortcut) {
            final locale =
                (_environment['LC_ALL'] ??
                        _environment['LC_MESSAGES'] ??
                        _environment['LANG'] ??
                        '')
                    .split('.')
                    .first;
            label =
                metadata['Name[$locale]'] ??
                metadata['Name[${locale.split('_').first}]'] ??
                metadata['Name'];
            icon = metadata['Icon'];
          }
        } on FormatException {
          // A malformed launcher remains an ordinary file, never an action.
        } on FileSystemException {
          // Lack of metadata read permission must not hide the file itself.
        }
      }
      return DesktopEntry(
        fullPath: _absolutePath(path),
        name: name,
        displayName: label == null || label.isEmpty ? name : label,
        isDirectory: targetType == FileSystemEntityType.directory,
        isSymlink: symlink,
        isDesktopShortcut: shortcut,
        trustedShortcut: shortcut && !symlink && (stat.mode & 0x49) != 0,
        iconName: icon?.startsWith('/') == true ? null : icon,
        iconPath: icon?.startsWith('/') == true ? icon : null,
        targetPath: targetPath,
        sizeBytes: stat.size,
        modified: stat.modified,
      );
    } on FileSystemException {
      // A child can disappear while the directory is being enumerated.
      return null;
    }
  }

  /// Read lstat metadata for this direct desktop child. FileStat follows links,
  /// so it cannot truthfully describe a link's own size, mtime or permissions.
  Future<DesktopFileDetails> inspectEntry(
    DesktopEntry entry, {
    DesktopInspectionCancellation? cancellation,
  }) async {
    cancellation?.check();
    _ensureActive();
    await start();
    cancellation?.check();
    final path = await _entryPath(entry);
    final output = await _command('stat', [
      '--printf=%f\\0%s\\0%y',
      '--',
      path,
    ]);
    cancellation?.check();
    _ensureActive();
    final fields = output.split('\u0000');
    if (fields.length != 3) throw StateError('无法解析文件属性');
    final mode = int.tryParse(fields[0], radix: 16);
    final size = int.tryParse(fields[1]);
    final modified = DateTime.tryParse(fields[2].trim());
    if (mode == null || size == null || size < 0 || modified == null) {
      throw StateError('文件属性不完整');
    }
    final kind = switch (mode & 0xf000) {
      0x8000 => DesktopFileKind.file,
      0x4000 => DesktopFileKind.directory,
      0xa000 => DesktopFileKind.symbolicLink,
      0x1000 => DesktopFileKind.fifo,
      0xc000 => DesktopFileKind.socket,
      0x6000 => DesktopFileKind.blockDevice,
      0x2000 => DesktopFileKind.characterDevice,
      _ => DesktopFileKind.unknown,
    };
    final target = kind == DesktopFileKind.symbolicLink
        ? await Link(path).target()
        : null;
    await _checkRoot();
    cancellation?.check();
    _ensureActive();
    return DesktopFileDetails(
      entry: entry,
      name: _basename(path),
      path: path,
      kind: kind,
      sizeBytes: size,
      modified: modified,
      mode: mode,
      linkTarget: target,
    );
  }

  /// Limited content inspection only. Neither launchers nor other executable
  /// files are passed to an application launcher during preview.
  Future<DesktopFilePreview> previewEntry(
    DesktopEntry entry, {
    DesktopInspectionCancellation? cancellation,
  }) async {
    final details = await inspectEntry(entry, cancellation: cancellation);
    DesktopFilePreview unsupported(String message) => DesktopFilePreview(
      details: details,
      kind: DesktopPreviewKind.unsupported,
      message: message,
    );
    if (details.kind != DesktopFileKind.file) {
      return unsupported(
        details.kind == DesktopFileKind.symbolicLink
            ? '符号链接不自动跟随预览；可查看链接目标或选择打开。'
            : details.kind == DesktopFileKind.directory
            ? '文件夹不提供内容预览；选择打开可查看其中的项目。'
            : '此类文件系统项目不支持快速查看。',
      );
    }
    var bytes = await _readPreviewBytes(
      details.path,
      previewTextByteLimit + 4,
      cancellation,
    );
    final image = _isPreviewImage(bytes);
    if (image && details.sizeBytes > previewImageByteLimit) {
      return unsupported('图片超过 8 MiB，未加载预览；可选择打开。');
    }
    if (image && details.sizeBytes > bytes.length) {
      bytes = await _readPreviewBytes(
        details.path,
        previewImageByteLimit + 1,
        cancellation,
      );
    }
    // Recheck root and metadata after reading: changes must not produce a
    // preview attributed to the old path/size. No content is published on error.
    final after = await inspectEntry(entry, cancellation: cancellation);
    if (after.kind != details.kind ||
        after.sizeBytes != details.sizeBytes ||
        after.modified != details.modified ||
        after.mode != details.mode) {
      throw StateError('文件在读取期间已改变，请重新快速查看');
    }
    if (image) {
      if (bytes.length > previewImageByteLimit ||
          bytes.length != details.sizeBytes) {
        return unsupported('图片大小超出预览限制或内容不完整。');
      }
      return DesktopFilePreview(
        details: details,
        kind: DesktopPreviewKind.image,
        imageBytes: bytes,
      );
    }
    final truncated = details.sizeBytes > previewTextByteLimit;
    final textBytes = Uint8List.sublistView(
      bytes,
      0,
      bytes.length > previewTextByteLimit ? previewTextByteLimit : bytes.length,
    );
    if (textBytes.any(
      (byte) => byte < 32 && byte != 9 && byte != 10 && byte != 13,
    )) {
      return unsupported('此文件格式暂不支持快速查看；可选择打开。');
    }
    String? text;
    // A byte-limited UTF-8 preview may end in the middle of its last character.
    for (
      var trim = 0;
      trim <= (truncated ? 3 : 0) && trim <= textBytes.length;
      trim++
    ) {
      try {
        if (trim > 0) {
          final start = textBytes.length - trim;
          final lead = bytes[start];
          final length = lead >= 0xc2 && lead <= 0xdf
              ? 2
              : lead >= 0xe0 && lead <= 0xef
              ? 3
              : lead >= 0xf0 && lead <= 0xf4
              ? 4
              : 0;
          if (length <= trim || start + length > bytes.length) continue;
          // The lookahead must complete a real UTF-8 character. Do not erase
          // malformed trailing bytes just because removing them decodes cleanly.
          utf8.decode(Uint8List.sublistView(bytes, start, start + length));
        }
        text = utf8.decode(
          Uint8List.sublistView(textBytes, 0, textBytes.length - trim),
        );
        break;
      } on FormatException {
        // Only incomplete trailing UTF-8 may be removed; invalid content still fails.
      }
    }
    if (text == null) return unsupported('暂不支持此文件的文本编码或格式。');
    return DesktopFilePreview(
      details: details,
      kind: DesktopPreviewKind.text,
      text: text,
      truncated: truncated,
      message: truncated ? '仅显示前 256 KiB 文本。' : null,
    );
  }

  Future<Uint8List> _readPreviewBytes(
    String path,
    int limit,
    DesktopInspectionCancellation? cancellation,
  ) async {
    cancellation?.check();
    await _checkRoot();
    // O_NOFOLLOW/O_NONBLOCK avoid following a raced symlink or hanging on a
    // FIFO replacing the checked regular file. dd is only a bounded byte reader.
    final process = await Process.start(
      'dd',
      [
        'if=$path',
        'iflag=nofollow,nonblock,count_bytes',
        'bs=65536',
        'count=$limit',
        'status=none',
      ],
      environment: _environment,
      includeParentEnvironment: false,
    );
    _previewProcesses.add(process);
    void cancel() {
      process.kill(ProcessSignal.sigkill);
    }

    if (_disposed) cancel();
    cancellation?.addListener(cancel);
    var timedOut = false;
    final timer = Timer(const Duration(seconds: 5), () {
      timedOut = true;
      cancel();
    });
    final bytes = BytesBuilder(copy: false);
    final errors = BytesBuilder(copy: false);
    var oversized = false;
    try {
      final output = process.stdout.forEach((chunk) {
        if (bytes.length + chunk.length > limit) {
          oversized = true;
          cancel();
        } else {
          bytes.add(chunk);
        }
      });
      final errorOutput = process.stderr.forEach((chunk) {
        final remaining = 2048 - errors.length;
        if (remaining > 0) errors.add(chunk.take(remaining).toList());
      });
      var code = -1;
      await Future.wait<void>([
        process.exitCode.then((value) {
          code = value;
        }),
        output,
        errorOutput,
      ]);
      cancellation?.check();
      _ensureActive();
      if (timedOut) throw StateError('文件预览读取超时');
      if (oversized) throw StateError('文件预览超出读取限制');
      if (code != 0) {
        throw FileSystemException(
          '无法读取预览：${utf8.decode(errors.takeBytes(), allowMalformed: true).trim()}',
          path,
        );
      }
      return bytes.takeBytes();
    } finally {
      timer.cancel();
      cancellation?.removeListener(cancel);
      _previewProcesses.remove(process);
      cancel();
    }
  }

  Future<void> createFile(String name) => _operation(() async {
    final path = await _newPath(name);
    _ensureActive();
    await File(path).create(exclusive: true);
  });

  Future<void> createDirectory(String name) => _operation(() async {
    final path = await _newPath(name);
    _ensureActive();
    // Directory.create accepts an existing directory. mkdir atomically rejects
    // a same-name directory created after the missing-path check.
    await _command('mkdir', ['--', path]);
  });

  Future<void> renameEntry(DesktopEntry entry, String name) =>
      _operation(() async {
        final source = await _entryPath(entry);
        _validateName(name);
        final target = '${_directoryPath!}/$name';
        if (source == target) return;
        await _requireMissing(target);
        _ensureActive();
        await _command('mv', [
          '--no-clobber',
          '--no-target-directory',
          '--',
          source,
          target,
        ]);
        // GNU mv -n returns success when a racing destination makes it skip.
        if (await FileSystemEntity.type(source, followLinks: false) !=
            FileSystemEntityType.notFound) {
          throw StateError('同名项目已存在，未重命名');
        }
        if (await FileSystemEntity.type(target, followLinks: false) ==
            FileSystemEntityType.notFound) {
          throw StateError('重命名未完成');
        }
      });

  Future<void> trashEntry(DesktopEntry entry) => _operation(() async {
    final path = await _entryPath(entry);
    _ensureActive();
    await _command('gio', ['trash', '--', path]);
  });

  /// Validate the whole selection before changing anything. GIO may still fail
  /// partway through; stop there, report progress, and rescan what really exists.
  Future<void> trashEntries(
    Iterable<DesktopEntry> entries,
  ) => _operation(() async {
    final selected = {
      for (final entry in entries) entry.fullPath: entry,
    }.values.toList();
    for (final entry in selected) {
      try {
        await _entryPath(entry);
      } on Object catch (error) {
        throw StateError('无法处理“${entry.displayName}”：$error');
      }
    }
    var completed = 0;
    try {
      for (final entry in selected) {
        try {
          final path = await _entryPath(entry);
          _ensureActive();
          await _command('gio', ['trash', '--', path]);
          completed++;
        } on Object catch (error) {
          throw StateError(
            '处理“${entry.displayName}”时发生错误；此前已完成 $completed/${selected.length} 项，其余项目未处理：$error',
          );
        }
      }
    } finally {
      if (!_disposed) await refresh(clearError: false);
    }
  }, refreshAfter: false);

  void copySelection(Iterable<DesktopEntry> entries) {
    _clipboardPaths = entries.map((entry) => entry.fullPath).toList();
    _notify();
  }

  Future<void> paste() => _operation(() async {
    _ensureActive();
    if (_clipboardPaths.isEmpty) return;
    final root = _directoryPath;
    if (root == null) throw StateError('桌面目录未设置');
    await _checkRoot();
    for (final sourcePath in _clipboardPaths) {
      final entityType = await FileSystemEntity.type(
        sourcePath,
        followLinks: false,
      );
      if (entityType == FileSystemEntityType.notFound) continue;
      final base = _basename(sourcePath);
      final destination = await _uniqueDestinationPath(root, base);
      if (entityType == FileSystemEntityType.directory) {
        await _copyDirectory(Directory(sourcePath), Directory(destination));
      } else if (entityType == FileSystemEntityType.file) {
        await File(sourcePath).copy(destination);
      } else if (entityType == FileSystemEntityType.link) {
        final target = await Link(sourcePath).target();
        await Link(destination).create(target);
      }
    }
  });

  Future<void> duplicateSelection(Iterable<DesktopEntry> entries) =>
      _operation(() async {
        _ensureActive();
        final root = _directoryPath;
        if (root == null) throw StateError('桌面目录未设置');
        await _checkRoot();
        final selected = {
          for (final entry in entries) entry.fullPath: entry,
        }.values.toList();
        for (final entry in selected) {
          final sourcePath = await _entryPath(entry);
          final base = _basename(sourcePath);
          final destination = await _duplicateDestinationPath(root, base);
          final entityType = await FileSystemEntity.type(
            sourcePath,
            followLinks: false,
          );
          if (entityType == FileSystemEntityType.directory) {
            await _copyDirectory(Directory(sourcePath), Directory(destination));
          } else if (entityType == FileSystemEntityType.file) {
            await File(sourcePath).copy(destination);
          } else if (entityType == FileSystemEntityType.link) {
            final target = await Link(sourcePath).target();
            await Link(destination).create(target);
          }
        }
      });

  Future<void> openEntries(
    Iterable<DesktopEntry> entries, {
    Set<String> allowedUntrustedShortcuts = const {},
  }) => _operation(() async {
    final selected = {
      for (final entry in entries) entry.fullPath: entry,
    }.values.toList();
    // In particular, reject an unconfirmed launcher before opening any files.
    for (final entry in selected) {
      await _prepareOpen(
        entry,
        allowedUntrustedShortcuts.contains(entry.fullPath),
      );
    }
    var completed = 0;
    for (final entry in selected) {
      try {
        final current = await _prepareOpen(
          entry,
          allowedUntrustedShortcuts.contains(entry.fullPath),
        );
        await _launchEntry(current);
        completed++;
      } on Object catch (error) {
        throw StateError(
          '处理“${entry.displayName}”时发生错误；此前已完成 $completed/${selected.length} 项，其余项目未处理：$error',
        );
      }
    }
  }, refreshAfter: false);

  Future<DesktopEntry> _prepareOpen(
    DesktopEntry entry,
    bool allowUntrusted,
  ) async {
    final path = await _entryPath(entry);
    final current = await _readEntry(path);
    _ensureActive();
    if (current == null) throw StateError('此项目已不存在');
    if (current.isDesktopShortcut &&
        !current.trustedShortcut &&
        !allowUntrusted) {
      throw StateError('此应用快捷方式尚未受信任，请确认后再运行');
    }
    return current;
  }

  Future<void> _launchEntry(DesktopEntry entry) async {
    _ensureActive();
    await _applicationLauncher.launch(
      'gio',
      entry.isDesktopShortcut
          ? ['launch', entry.fullPath]
          : ['open', '--', entry.fullPath],
    );
  }

  Future<void> openEntry(
    DesktopEntry entry, {
    bool allowUntrustedShortcut = false,
  }) => _operation(() async {
    await _launchEntry(await _prepareOpen(entry, allowUntrustedShortcut));
  }, refreshAfter: false);

  Future<void> createShortcutPath(
    String targetPath, {
    String? name,
  }) => _operation(() async {
    final target = _absolutePath(targetPath);
    if (await FileSystemEntity.type(target) == FileSystemEntityType.notFound) {
      throw StateError('快捷方式目标不存在');
    }
    final path = await _newPath(name ?? _defaultName(_basename(target), '链接'));
    _ensureActive();
    await Link(path).create(target);
  });

  Future<void> createShortcutInstalledApp(
    DesktopApp app, {
    String? name,
  }) => _operation(() async {
    final source = _absolutePath(app.desktopFile);
    final catalogue = await DesktopApps(environment: _environment).load();
    if (!catalogue.any(
      (installed) =>
          installed.id == app.id &&
          _absolutePath(installed.desktopFile) == source,
    )) {
      throw StateError('只能为当前已安装的应用创建快捷方式');
    }
    final stat = await File(source).stat();
    if (stat.type != FileSystemEntityType.file || stat.size > 1024 * 1024) {
      throw StateError('应用快捷方式来源无效或过大');
    }
    final bytes = await File(source).readAsBytes();
    if (parseDesktopEntry(utf8.decode(bytes))['Type'] != 'Application') {
      throw StateError('应用快捷方式来源已改变');
    }
    final filename = name ?? '${_defaultName(app.name, '应用')}.desktop';
    if (!filename.endsWith('.desktop')) {
      throw ArgumentError('应用快捷方式名称必须以 .desktop 结尾');
    }
    final destination = await _newPath(filename);
    _ensureActive();
    final staging = await Directory(
      _directoryPath!,
    ).createTemp('.clarion-shortcut-');
    final staged = File('${staging.path}/shortcut.desktop');
    try {
      await staged.writeAsBytes(bytes, flush: true);
      _ensureActive();
      // Trust is set on our private staging file, never on a colliding target.
      await _command('chmod', ['u+x', '--', staged.path]);
      await _checkRoot();
      _ensureActive();
      await _command('mv', [
        '--no-clobber',
        '--no-target-directory',
        '--',
        staged.path,
        destination,
      ]);
      if (await staged.exists()) {
        throw StateError('同名项目已存在，未创建快捷方式');
      }
      if (await FileSystemEntity.type(destination, followLinks: false) !=
          FileSystemEntityType.file) {
        throw StateError('快捷方式发布未完成');
      }
      if (!listEquals(await File(destination).readAsBytes(), bytes)) {
        throw StateError('快捷方式目标已改变，未确认创建成功');
      }
    } finally {
      // Only these known private staging objects are removed; user directories
      // and their contents never have a recursive-delete fallback.
      if (await staged.exists()) await staged.delete();
      try {
        await staging.delete();
      } on FileSystemException {
        // Preserve unexpected content rather than deleting it recursively.
      }
    }
  });

  Future<void> _operation(
    Future<void> Function() action, {
    bool refreshAfter = true,
  }) async {
    if (_disposed || _busy) return;
    _busy = true;
    _operationError = null;
    _notify();
    try {
      await start();
      if (_disposed) return;
      if (_directoryPath == null) throw StateError('桌面目录未设置');
      await action();
      if (!_disposed && refreshAfter) await refresh(clearError: false);
    } on Object catch (error) {
      if (!_disposed) _operationError = '桌面操作失败：$error';
    } finally {
      if (!_disposed) {
        _busy = false;
        _notify();
      }
    }
  }

  Future<String> _newPath(String name) async {
    _validateName(name);
    final root = _directoryPath!;
    if (!await Directory(root).exists()) {
      _ensureActive();
      await Directory(root).create(recursive: true);
    }
    await _checkRoot();
    final path = '$root/$name';
    await _requireMissing(path);
    return path;
  }

  Future<String> _entryPath(DesktopEntry entry) async {
    await _checkRoot();
    final path = _absolutePath(entry.fullPath);
    if (_parent(path) != _directoryPath) {
      throw ArgumentError('只能操作桌面目录内的直接项目');
    }
    if (await FileSystemEntity.type(path, followLinks: false) ==
        FileSystemEntityType.notFound) {
      throw StateError('此项目已不存在');
    }
    return path;
  }

  Future<void> _checkRoot() async {
    _ensureActive();
    final root = _directoryPath;
    if (root == null) throw StateError('桌面目录未设置');
    final resolved = await Directory(root).resolveSymbolicLinks();
    _ensureActive();
    if (_canonicalRoot != null && resolved != _canonicalRoot) {
      throw StateError('桌面目录指向已改变，请重新载入桌面服务');
    }
    _canonicalRoot = resolved;
  }

  Future<void> _requireMissing(String path) async {
    if (await FileSystemEntity.type(path, followLinks: false) !=
        FileSystemEntityType.notFound) {
      throw StateError('同名项目已存在');
    }
  }

  void _notify() {
    if (!_disposed) notifyListeners();
  }

  void _ensureActive() {
    if (_disposed) throw StateError('桌面服务已关闭');
  }

  @override
  void dispose() {
    if (_disposed) return;
    _disposed = true;
    for (final process in _previewProcesses) {
      process.kill(ProcessSignal.sigkill);
    }
    _previewProcesses.clear();
    _debounce?.cancel();
    unawaited(_watcher?.cancel());
    unawaited(_parentWatcher?.cancel());
    super.dispose();
  }
}

bool _isPreviewImage(Uint8List bytes) {
  bool starts(List<int> magic) =>
      bytes.length >= magic.length &&
      List.generate(
        magic.length,
        (index) => bytes[index] == magic[index],
      ).every((value) => value);
  return starts([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]) ||
      starts([0xff, 0xd8, 0xff]) ||
      starts(ascii.encode('GIF87a')) ||
      starts(ascii.encode('GIF89a')) ||
      starts(ascii.encode('BM')) ||
      (bytes.length >= 12 &&
          starts(ascii.encode('RIFF')) &&
          ascii.decode(bytes.sublist(8, 12), allowInvalid: true) == 'WEBP');
}

void _validateName(String name) {
  if (name.trim().isEmpty ||
      name == '.' ||
      name == '..' ||
      name.contains('/') ||
      name.runes.any((rune) => rune < 32 || rune == 127) ||
      utf8.encode(name).length > 255) {
    throw ArgumentError('名称必须是非空的单个文件名，不能包含路径或控制字符，最多 255 字节');
  }
}

String _defaultName(String name, String fallback) {
  final safe = name.replaceAll(RegExp(r'[/\x00-\x1f\x7f]'), '-').trim();
  return safe.isEmpty || safe == '.' || safe == '..' ? fallback : safe;
}

String _absolutePath(String path) {
  if (!path.startsWith('/') || path.contains('\u0000')) {
    throw ArgumentError('路径必须是本地绝对路径');
  }
  final components = <String>[];
  for (final component in path.split('/')) {
    if (component.isEmpty || component == '.') continue;
    if (component == '..') {
      if (components.isNotEmpty) components.removeLast();
    } else {
      components.add(component);
    }
  }
  return '/${components.join('/')}';
}

String _basename(String path) => path.substring(path.lastIndexOf('/') + 1);
String _parent(String path) {
  final index = path.lastIndexOf('/');
  return index <= 0 ? '/' : path.substring(0, index);
}

Future<bool> _sameDirectory(String first, String second) async {
  try {
    return await Directory(first).resolveSymbolicLinks() ==
        await Directory(second).resolveSymbolicLinks();
  } on FileSystemException {
    return false;
  }
}

Future<void> _copyDirectory(Directory source, Directory destination) async {
  await destination.create(recursive: true);
  await for (final entity in source.list(
    recursive: false,
    followLinks: false,
  )) {
    final name = _basename(entity.path);
    final targetPath = '${destination.path}/$name';
    if (entity is Directory) {
      await _copyDirectory(entity, Directory(targetPath));
    } else if (entity is File) {
      await entity.copy(targetPath);
    } else if (entity is Link) {
      final target = await entity.target();
      await Link(targetPath).create(target);
    }
  }
}

Future<String> _uniqueDestinationPath(String root, String filename) async {
  final initialPath = '$root/$filename';
  if (await FileSystemEntity.type(initialPath, followLinks: false) ==
      FileSystemEntityType.notFound) {
    return initialPath;
  }
  return _duplicateDestinationPath(root, filename);
}

Future<String> _duplicateDestinationPath(String root, String filename) async {
  String stem;
  String ext;
  final dotIndex = filename.lastIndexOf('.');
  if (dotIndex > 0) {
    stem = filename.substring(0, dotIndex);
    ext = filename.substring(dotIndex);
  } else {
    stem = filename;
    ext = '';
  }

  final copyPattern = RegExp(r'^(.*?) 副本(?: (\d+))?$');
  final match = copyPattern.firstMatch(stem);
  final baseStem = match != null ? match.group(1)! : stem;

  var index = 1;
  while (true) {
    final candidateName = index == 1
        ? '$baseStem 副本$ext'
        : '$baseStem 副本 $index$ext';
    final candidatePath = '$root/$candidateName';
    if (await FileSystemEntity.type(candidatePath, followLinks: false) ==
        FileSystemEntityType.notFound) {
      return candidatePath;
    }
    index++;
  }
}
