// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:io';
import '../models.dart';
import 'application_launcher.dart';
import 'desktop_entry_locale.dart';

class DesktopApps {
  DesktopApps({
    Map<String, String>? environment,
    ApplicationLauncher? applicationLauncher,
  }) : environment = environment ?? Platform.environment,
       _applicationLauncher =
           applicationLauncher ?? ApplicationLauncher(environment: environment);
  final Map<String, String> environment;
  final ApplicationLauncher _applicationLauncher;

  List<String> get dataRoots {
    final home = environment['HOME'] ?? '';
    final dataHome = environment['XDG_DATA_HOME'];
    final systemRoots = (environment['XDG_DATA_DIRS'] ?? '')
        .split(':')
        .where((root) => root.startsWith('/'))
        .toList();
    return <String>{
      if (dataHome != null && dataHome.startsWith('/'))
        dataHome
      else if (home.startsWith('/'))
        '$home/.local/share',
      ...(systemRoots.isEmpty
          ? ['/usr/local/share', '/usr/share']
          : systemRoots),
    }.toList();
  }

  Future<List<DesktopApp>> load() async {
    final seen = <String>{};
    final apps = <DesktopApp>[];
    final executableCache = <String, bool>{};
    final desktops = (environment['XDG_CURRENT_DESKTOP'] ?? 'clarion')
        .split(':')
        .toSet();
    final locale = desktopEntryLocale(environment);
    for (final root in dataRoots) {
      final dir = Directory('$root/applications');
      if (!await dir.exists()) continue;
      try {
        final files = await dir
            .list(recursive: true, followLinks: false)
            .where((entry) => entry.path.endsWith('.desktop'))
            .toList();
        files.sort((a, b) => a.path.compareTo(b.path));
        for (final entry in files) {
          final id = entry.path
              .substring(dir.path.length + 1)
              .replaceAll('/', '-');
          // Even Hidden and NoDisplay overrides mask a system entry.
          if (!seen.add(id)) continue;
          try {
            final values = parseDesktopEntry(
              await File(entry.path).readAsString(),
            );
            if (values['Type'] != 'Application' ||
                values['Hidden'] == 'true' ||
                values['NoDisplay'] == 'true') {
              continue;
            }
            final only = _list(values['OnlyShowIn']);
            final not = _list(values['NotShowIn']);
            if (only.isNotEmpty && !only.any(desktops.contains)) continue;
            if (not.any(desktops.contains)) continue;
            final exec = values['Exec'];
            if (values['DBusActivatable'] != 'true' &&
                (exec == null ||
                    exec.trim().isEmpty ||
                    !_execAvailable(exec, executableCache))) {
              continue;
            }
            final tryExec = values['TryExec'];
            if (tryExec != null &&
                !_executableExists(tryExec, cache: executableCache)) {
              continue;
            }
            String localized(String key) =>
                values['$key[$locale]'] ??
                values['$key[${locale.split('_').first}]'] ??
                values[key] ??
                '';
            final name = localized('Name');
            if (name.isEmpty) continue;
            final icon = values['Icon'];
            apps.add(
              DesktopApp(
                id: id,
                name: name,
                desktopFile: entry.path,
                description: localized('Comment'),
                keywords: _list(localized('Keywords')),
                iconName: icon,
                iconPath:
                    icon != null &&
                        icon.startsWith('/') &&
                        await File(icon).exists()
                    ? icon
                    : null,
              ),
            );
          } on FileSystemException {
            continue;
          } on FormatException {
            continue;
          }
        }
      } on FileSystemException {
        continue;
      }
    }
    apps.sort((a, b) => a.name.toLowerCase().compareTo(b.name.toLowerCase()));
    return List.unmodifiable(apps);
  }

  bool _execAvailable(String exec, Map<String, bool> cache) {
    final words = _execWords(exec);
    // Unrecognized quoting/options stay delegated to GIO. Never guess that an
    // application is obsolete from its name or an interpreter's arguments.
    if (words == null || words.isEmpty) return true;
    final program = words.first;
    if (!_executableExists(program, cache: cache)) return false;
    if (program.split('/').last != 'env') return true;
    var path = environment['PATH'] ?? '/usr/bin:/bin';
    var index = 1;
    while (index < words.length) {
      final word = words[index];
      if (word == '--') {
        index++;
        break;
      }
      if (RegExp(r'^[A-Za-z_][A-Za-z0-9_]*=').hasMatch(word)) {
        if (word.startsWith('PATH=')) path = word.substring(5);
        index++;
        continue;
      }
      if (word.startsWith('-')) return true;
      break;
    }
    if (index >= words.length) return true;
    return _executableExists(words[index], searchPath: path, cache: cache);
  }

  bool _executableExists(
    String executable, {
    String? searchPath,
    required Map<String, bool> cache,
  }) {
    final path = searchPath ?? environment['PATH'] ?? '/usr/bin:/bin';
    final key = '$path\u0000$executable';
    return cache.putIfAbsent(key, () {
      if (executable.isEmpty) return false;
      // Relative paths and field-code expressions need GIO's working directory
      // and expansion context; they do not prove an application was removed.
      if (executable.contains('%') ||
          (executable.contains('/') && !executable.startsWith('/'))) {
        return true;
      }
      if (executable.startsWith('/')) return _isExecutableFile(executable);
      final directories = path.split(':');
      if (directories
          .where((dir) => dir.startsWith('/'))
          .any((dir) => _isExecutableFile('$dir/$executable'))) {
        return true;
      }
      // A relative/empty PATH entry depends on the launch working directory.
      // Its presence prevents proving that this bare executable is absent.
      return directories.any((dir) => !dir.startsWith('/'));
    });
  }

  /// GIO owns Desktop Entry Exec expansion, quoting and D-Bus activation.
  /// Never turn a .desktop Exec value into shell code.
  Future<void> launch(DesktopApp app) async {
    await _applicationLauncher.launch('gio', ['launch', app.desktopFile]);
  }
}

bool _isExecutableFile(String path) {
  try {
    final stat = File(path).statSync();
    return stat.type == FileSystemEntityType.file && (stat.mode & 0x49) != 0;
  } on FileSystemException {
    return false;
  }
}

/// Reads Desktop Entry argument boundaries, without invoking a shell or
/// expanding variables. Backslashes are already unescaped at the entry level.
List<String>? _execWords(String value) {
  final words = <String>[];
  var token = StringBuffer();
  var quoted = false;
  var started = false;
  for (var index = 0; index < value.length; index++) {
    final character = value[index];
    if (character == '\u0000' || character == '\n' || character == '\r') {
      return null;
    }
    if (character == '\\') {
      if (++index >= value.length) return null;
      token.write(value[index]);
      started = true;
    } else if (character == '"') {
      quoted = !quoted;
      started = true;
    } else if (!quoted && (character == ' ' || character == '\t')) {
      if (started) words.add(token.toString());
      token = StringBuffer();
      started = false;
    } else {
      token.write(character);
      started = true;
    }
  }
  if (quoted) return null;
  if (started) words.add(token.toString());
  return words;
}

List<String> _list(String? value) =>
    (value ?? '').split(';').where((v) => v.isNotEmpty).toList();

Map<String, String> parseDesktopEntry(String source) {
  final values = <String, String>{};
  var entry = false;
  for (final raw in source.split('\n')) {
    final line = raw.trim();
    if (line.startsWith('[')) {
      entry = line == '[Desktop Entry]';
      continue;
    }
    if (!entry || line.isEmpty || line.startsWith('#')) continue;
    final split = line.indexOf('=');
    if (split <= 0) continue;
    values[line.substring(0, split).trim()] = _unescape(
      line.substring(split + 1),
    );
  }
  return values;
}

String _unescape(String value) => value.replaceAllMapped(
  RegExp(r'\\([sntr\\])'),
  (m) => switch (m[1]) {
    's' => ' ',
    'n' => '\n',
    't' => '\t',
    'r' => '\r',
    _ => '\\',
  },
);
