// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart' show StringCharacters;

/// Launchpad stores application identities, never launch commands or paths.
class MacLaunchpadEntry {
  MacLaunchpadEntry.app(String appId)
    : id = appId,
      name = '',
      isFolder = false,
      appIds = List.unmodifiable([appId]);
  MacLaunchpadEntry.folder(this.id, this.name, Iterable<String> apps)
    : isFolder = true,
      appIds = List.unmodifiable(apps);
  final String id;
  final String name;
  final List<String> appIds;
  final bool isFolder;
  Map<String, Object> toJson() => {
    'id': id,
    'name': name,
    'apps': appIds,
    'type': isFolder ? 'folder' : 'app',
  };
}

/// Independent, atomic user layout. Removed apps are pruned, new apps append,
/// and named folders survive while they still contain applications.
class MacLaunchpadLayout extends ChangeNotifier {
  MacLaunchpadLayout({File? file, this.persist = true})
    : file = file ?? defaultFile;
  final File? file;
  final bool persist;
  List<MacLaunchpadEntry> _entries = [];
  Set<String> _catalog = {};
  Future<void> _writes = Future.value();
  bool _disposed = false;
  bool _loaded = false;
  int _revision = 0;
  int _folderSequence = 0;
  String? error;
  List<MacLaunchpadEntry> get entries => List.unmodifiable(_entries);
  bool get loaded => _loaded;
  Iterable<String> get orderedAppIds => _entries.expand((item) => item.appIds);
  static File? get defaultFile {
    final config = Platform.environment['XDG_CONFIG_HOME'];
    final home = Platform.environment['HOME'];
    final root = config != null && config.startsWith('/')
        ? config
        : home == null
        ? null
        : '$home/.config';
    return root == null
        ? null
        : File('$root/realand-shell/launchpad-layout.json');
  }

  Future<void> load(Iterable<String> catalog) async {
    if (_loaded || _disposed) return;
    _catalog = catalog.toSet();
    final revision = _revision;
    List<MacLaunchpadEntry>? saved;
    try {
      final source = file;
      if (persist &&
          source != null &&
          await source.exists() &&
          await source.length() <= 1048576) {
        final value = jsonDecode(await source.readAsString());
        if (value is Map && value['version'] == 1 && value['entries'] is List) {
          saved = [];
          for (final item in (value['entries'] as List).take(4096)) {
            if (item is! Map ||
                item['id'] is! String ||
                item['apps'] is! List) {
              continue;
            }
            final id = item['id'] as String;
            if (id.isEmpty || id.length > 512) continue;
            final apps = (item['apps'] as List)
                .whereType<String>()
                .where((id) => id.isNotEmpty && id.length <= 512)
                .take(4096)
                .toList();
            if (apps.isEmpty) continue;
            final name = item['name'];
            final folder =
                item['type'] == 'folder' ||
                id.startsWith('folder:') ||
                apps.length > 1;
            saved.add(
              folder
                  ? MacLaunchpadEntry.folder(
                      id,
                      _name(name is String ? name : '文件夹'),
                      apps,
                    )
                  : MacLaunchpadEntry.app(apps.first),
            );
          }
        }
      }
    } on Object catch (failure) {
      error = '无法读取启动台布局：$failure';
    }
    if (_disposed) return;
    if (revision == _revision && saved != null) _entries = saved;
    _loaded = true;
    _normalize();
    notifyListeners();
  }

  void reconcile(Iterable<String> catalog) {
    final next = catalog.toSet();
    if (setEquals(next, _catalog)) return;
    _catalog = next;
    _normalize();
    // Catalog arrival is independent of user edits. An in-flight disk read
    // must still restore saved folders and normalize them against this set.
    if (_loaded) {
      _changed();
    } else {
      notifyListeners();
    }
  }

  void _normalize() {
    final used = <String>{};
    final entryIds = <String>{};
    final normalized = <MacLaunchpadEntry>[];
    for (final item in _entries) {
      final apps = item.appIds
          .where((id) => _catalog.contains(id) && used.add(id))
          .toList();
      if (apps.isEmpty) continue;
      if (!item.isFolder) {
        normalized.add(MacLaunchpadEntry.app(apps.first));
      } else {
        final id =
            item.id.startsWith('folder:') &&
                entryIds.add(item.id) &&
                !_catalog.contains(item.id)
            ? item.id
            : _newFolderId();
        normalized.add(MacLaunchpadEntry.folder(id, _name(item.name), apps));
      }
    }
    for (final id in _catalog) {
      if (used.add(id)) normalized.add(MacLaunchpadEntry.app(id));
    }
    _entries = normalized;
  }

  String _newFolderId() =>
      'folder:${DateTime.now().microsecondsSinceEpoch}:${_folderSequence++}';
  static String _name(String name) {
    final clean = name.replaceAll(RegExp(r'[\x00-\x1f\x7f]'), '').trim();
    return clean.isEmpty ? '文件夹' : clean.characters.take(80).toString();
  }

  MacLaunchpadEntry? find(String id) =>
      _entries.where((entry) => entry.id == id).firstOrNull;

  /// A gap drop reorders a root entry or extracts an application from a folder.
  void moveBefore(String id, String? beforeId) {
    if (id == beforeId) return;
    final target = beforeId == null ? null : find(beforeId);
    if (beforeId != null && target == null) return;
    var item = find(id);
    if (item == null && _catalog.contains(id)) {
      _removeApp(id);
      item = MacLaunchpadEntry.app(id);
    } else if (item != null) {
      _entries.remove(item);
    }
    if (item == null) return;
    final index = target == null
        ? _entries.length
        : _entries.indexWhere((entry) => entry.id == target.id);
    _entries.insert(index < 0 ? _entries.length : index, item);
    _changed();
  }

  /// Removes an entry or app from the user's launchpad layout.
  void remove(String id) {
    final before = _entries.length;
    _entries.removeWhere((entry) => entry.id == id);
    _removeApp(id);
    if (_entries.length != before) {
      _changed();
    }
  }

  /// App-on-app creates a real folder; app-on-folder adds to that folder.
  void group(String appId, String targetId) {
    if (!_catalog.contains(appId)) return;
    final target = find(targetId);
    if (target == null || target.appIds.contains(appId)) return;
    final targetApps = target.appIds;
    _removeApp(appId);
    final index = _entries.indexWhere((entry) => entry.id == targetId);
    if (index < 0) return;
    _entries[index] = MacLaunchpadEntry.folder(
      target.isFolder ? target.id : _newFolderId(),
      target.isFolder ? target.name : '文件夹',
      [...targetApps, appId],
    );
    _changed();
  }

  void _removeApp(String id) {
    _entries = _entries
        .map((item) {
          final apps = item.appIds.where((app) => app != id).toList();
          return apps.isEmpty
              ? null
              : item.isFolder
              ? MacLaunchpadEntry.folder(item.id, item.name, apps)
              : MacLaunchpadEntry.app(apps.first);
        })
        .whereType<MacLaunchpadEntry>()
        .toList();
  }

  void rename(String folderId, String name) {
    final index = _entries.indexWhere(
      (entry) => entry.id == folderId && entry.isFolder,
    );
    if (index < 0) return;
    final item = _entries[index];
    _entries[index] = MacLaunchpadEntry.folder(
      item.id,
      _name(name),
      item.appIds,
    );
    _changed();
  }

  void reorderInFolder(String folderId, String appId, String? beforeId) {
    final index = _entries.indexWhere(
      (entry) => entry.id == folderId && entry.isFolder,
    );
    if (index < 0 || appId == beforeId) return;
    final item = _entries[index];
    if (!item.appIds.contains(appId) ||
        (beforeId != null && !item.appIds.contains(beforeId))) {
      return;
    }
    final apps = List<String>.of(item.appIds)..remove(appId);
    apps.insert(beforeId == null ? apps.length : apps.indexOf(beforeId), appId);
    if (listEquals(apps, item.appIds)) return;
    _entries[index] = MacLaunchpadEntry.folder(item.id, item.name, apps);
    _changed();
  }

  void _changed() {
    _revision++;
    if (!_disposed) notifyListeners();
    if (_loaded) unawaited(save());
  }

  Future<void> save() {
    final target = file;
    if (!persist || target == null || _disposed) return Future.value();
    final contents =
        '${jsonEncode({'version': 1, 'entries': _entries.map((entry) => entry.toJson()).toList()})}\n';
    _writes = _writes.then((_) async {
      Directory? staging;
      try {
        await target.parent.create(recursive: true);
        staging = await target.parent.createTemp('.launchpad-');
        final replacement = File('${staging.path}/layout.json');
        await replacement.writeAsString(contents, flush: true);
        await replacement.rename(target.path);
        error = null;
      } on Object catch (failure) {
        error = '无法保存启动台布局：$failure';
      } finally {
        try {
          if (staging != null && await staging.exists()) {
            await staging.delete(recursive: true);
          }
        } on FileSystemException catch (failure) {
          error ??= '无法清理启动台临时布局：$failure';
        }
        if (!_disposed) notifyListeners();
      }
    });
    return _writes;
  }

  @override
  void dispose() {
    _disposed = true;
    super.dispose();
  }
}
