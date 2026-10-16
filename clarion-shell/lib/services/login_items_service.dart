// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:io';
import '../models.dart';
import 'desktop_apps.dart';

/// User-level XDG autostart overrides. Reading a menu never writes a file, and
/// changing a login item never starts the application in the current session.
class LoginItemsService {
  LoginItemsService({Map<String, String>? environment, this.demo = false})
    : environment = environment ?? Platform.environment;

  final Map<String, String> environment;
  final bool demo;
  final _demoItems = <String>{};

  String get _userRoot {
    final config = environment['XDG_CONFIG_HOME'];
    if (config != null && config.startsWith('/')) return config;
    final home = environment['HOME'];
    if (home == null || !home.startsWith('/')) {
      throw StateError('无法确定用户配置目录');
    }
    return '$home/.config';
  }

  Iterable<String> get _roots sync* {
    yield _userRoot;
    final system = (environment['XDG_CONFIG_DIRS'] ?? '/etc/xdg')
        .split(':')
        .where((path) => path.startsWith('/'));
    yield* system.isEmpty ? ['/etc/xdg'] : system;
  }

  Set<String> get _desktops => (environment['XDG_CURRENT_DESKTOP'] ?? 'clarion')
      .split(':')
      .where((name) => name.isNotEmpty)
      .toSet();

  String _filename(DesktopApp app) {
    final name = app.id;
    if (!name.endsWith('.desktop') ||
        name.contains('/') ||
        name.contains('\\') ||
        name.contains('\u0000') ||
        name.contains('\n') ||
        name.contains('\r')) {
      throw ArgumentError('无效的应用启动文件名');
    }
    return name;
  }

  Future<String?> _existing(DesktopApp app) async {
    final name = _filename(app);
    for (final root in _roots) {
      final file = File('$root/autostart/$name');
      try {
        return await file.readAsString();
      } on FileSystemException catch (error) {
        if (error.osError?.errorCode != 2) rethrow;
      }
    }
    return null;
  }

  Future<bool> isEnabled(DesktopApp app) async {
    if (demo) return _demoItems.contains(app.id);
    final source = await _existing(app);
    if (source == null) return false;
    final values = parseDesktopEntry(source);
    if (values['Hidden'] == 'true' ||
        values['X-GNOME-Autostart-enabled'] == 'false' ||
        values['X-systemd-skip'] == 'true') {
      return false;
    }
    final only = _list(values['OnlyShowIn']);
    final not = _list(values['NotShowIn']);
    if (values.containsKey('OnlyShowIn') && !only.any(_desktops.contains)) {
      return false;
    }
    if (not.any(_desktops.contains)) return false;
    if (!await _tryExecAvailable(values['TryExec'])) return false;
    return values['Type'] == 'Application' &&
        ((values['Exec'] ?? '').trim().isNotEmpty ||
            values['DBusActivatable'] == 'true');
  }

  Future<void> setEnabled(DesktopApp app, bool enabled) async {
    if (demo) {
      enabled ? _demoItems.add(app.id) : _demoItems.remove(app.id);
      return;
    }
    final name = _filename(app);
    var source = await _existing(app);
    if (!enabled && source == null) return;
    var values = parseDesktopEntry(source ?? '');
    if (enabled &&
        (values['Type'] != 'Application' ||
            ((values['Exec'] ?? '').trim().isEmpty &&
                values['DBusActivatable'] != 'true'))) {
      source = await File(app.desktopFile).readAsString();
      values = parseDesktopEntry(source);
    }
    if (enabled &&
        (values['Type'] != 'Application' ||
            ((values['Exec'] ?? '').trim().isEmpty &&
                values['DBusActivatable'] != 'true'))) {
      throw StateError('此应用没有可用的启动命令');
    }
    if (enabled && (values['AutostartCondition'] ?? '').isNotEmpty) {
      throw StateError('此应用使用条件启动，请在系统登录项中设置');
    }
    if (enabled && !await _tryExecAvailable(values['TryExec'])) {
      throw StateError('此应用的启动程序不可用');
    }
    final changes = <String, String>{'Hidden': enabled ? 'false' : 'true'};
    if (enabled) {
      changes['X-GNOME-Autostart-enabled'] = 'true';
      changes['X-systemd-skip'] = 'false';
      final only = _list(values['OnlyShowIn']);
      final not = _list(values['NotShowIn']);
      if (values.containsKey('OnlyShowIn') && !only.any(_desktops.contains)) {
        changes['OnlyShowIn'] = '${{...only, ..._desktops}.join(';')};';
      }
      if (not.any(_desktops.contains)) {
        changes['NotShowIn'] =
            '${not.where((d) => !_desktops.contains(d)).join(';')};';
      }
    }
    final directory = Directory('$_userRoot/autostart');
    await directory.create(recursive: true);
    // Rename in the same filesystem: readers never see half a Desktop Entry,
    // and an existing user symlink is replaced without writing through it.
    final temporary = await directory.createTemp('.clarion-login-');
    try {
      final pending = File('${temporary.path}/$name');
      await pending.writeAsString(_patch(source!, changes), flush: true);
      await pending.rename('${directory.path}/$name');
    } finally {
      await temporary.delete(recursive: true);
    }
  }

  static List<String> _list(String? value) =>
      (value ?? '').split(';').where((part) => part.isNotEmpty).toList();

  Future<bool> _tryExecAvailable(String? executable) async {
    if (executable == null || executable.isEmpty) return true;
    if (!executable.startsWith('/') && executable.contains('/')) return false;
    final paths = executable.startsWith('/')
        ? [executable]
        : (environment['PATH'] ?? '/usr/bin:/bin')
              .split(':')
              .where((path) => path.startsWith('/'))
              .map((path) => '$path/$executable');
    for (final path in paths) {
      final stat = await File(path).stat();
      if (stat.type == FileSystemEntityType.file && (stat.mode & 0x49) != 0) {
        return true;
      }
    }
    return false;
  }

  /// Preserve Exec quoting, localized names, comments and Desktop Action groups.
  static String _patch(String source, Map<String, String> changes) {
    final output = <String>[];
    var inEntry = false;
    var foundEntry = false;
    void flush() {
      if (!inEntry) return;
      output.addAll(changes.entries.map((e) => '${e.key}=${e.value}'));
      inEntry = false;
    }

    for (final line in source.split('\n')) {
      final trimmed = line.trim();
      if (trimmed.startsWith('[') && trimmed.endsWith(']')) {
        flush();
        inEntry = trimmed == '[Desktop Entry]';
        foundEntry |= inEntry;
      }
      final equals = trimmed.indexOf('=');
      if (inEntry &&
          equals > 0 &&
          changes.containsKey(trimmed.substring(0, equals).trim())) {
        continue;
      }
      output.add(line);
    }
    flush();
    if (!foundEntry) {
      throw const FormatException('应用启动文件缺少 Desktop Entry');
    }
    return '${output.join('\n')}\n';
  }
}
