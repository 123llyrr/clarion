// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:convert';
import 'dart:async';
import 'dart:io';
import 'package:flutter_test/flutter_test.dart';
import 'package:realand_shell/mac_launchpad_layout.dart';

class _ReadGateFile implements File {
  _ReadGateFile(this.source);
  final File source;
  final reading = Completer<void>();
  final resume = Completer<void>();
  @override
  String get path => source.path;
  @override
  Directory get parent => source.parent;
  @override
  Future<bool> exists() => source.exists();
  @override
  Future<int> length() => source.length();
  @override
  Future<String> readAsString({Encoding encoding = utf8}) async {
    reading.complete();
    await resume.future;
    return source.readAsString(encoding: encoding);
  }

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

void main() {
  test(
    'folder end drop only reorders members and rejects foreign IDs',
    () async {
      final layout = MacLaunchpadLayout(persist: false);
      addTearDown(layout.dispose);
      await layout.load(['a', 'b', 'c', 'd']);
      layout.group('a', 'b');
      final folder = layout.entries.first;
      layout.group('c', folder.id);
      layout.reorderInFolder(folder.id, 'b', null);
      expect(layout.find(folder.id)!.appIds, ['a', 'c', 'b']);
      layout.reorderInFolder(folder.id, 'd', null);
      layout.reorderInFolder(folder.id, 'a', 'missing');
      expect(layout.find(folder.id)!.appIds, ['a', 'c', 'b']);
      expect(layout.orderedAppIds.toSet(), {'a', 'b', 'c', 'd'});
    },
  );
  test(
    'real folder, rename, ordering and extraction survive atomic file reload',
    () async {
      final temp = Directory.systemTemp.createTempSync(
        'realand-launchpad-model-',
      );
      addTearDown(() => temp.deleteSync(recursive: true));
      final file = File('${temp.path}/config/layout.json');
      final layout = MacLaunchpadLayout(file: file);
      addTearDown(layout.dispose);
      await layout.load(['a', 'b', 'c', 'd']);
      layout.group('a', 'b');
      final folder = layout.entries.first;
      expect(folder.isFolder, isTrue);
      expect(folder.appIds, ['b', 'a']);
      layout.group('c', folder.id);
      layout.rename(folder.id, '工作');
      layout.reorderInFolder(folder.id, 'c', 'b');
      layout.moveBefore('d', folder.id);
      await layout.save();
      final restored = MacLaunchpadLayout(file: file);
      addTearDown(restored.dispose);
      await restored.load(['d', 'c', 'b', 'a']);
      expect(restored.entries.map((entry) => entry.id), ['d', folder.id]);
      expect(restored.find(folder.id)!.name, '工作');
      expect(restored.find(folder.id)!.appIds, ['c', 'b', 'a']);
      restored.moveBefore('a', 'd');
      expect(restored.entries.map((entry) => entry.id), ['a', 'd', folder.id]);
      restored.moveBefore('c', 'a');
      expect(restored.entries.map((entry) => entry.id), [
        'c',
        'a',
        'd',
        folder.id,
      ]);
      expect(restored.find(folder.id)!.appIds, ['b']);
      restored.moveBefore('b', null);
      expect(restored.entries.map((entry) => entry.id), ['c', 'a', 'd', 'b']);
      await restored.save();
      expect(file.readAsStringSync(), contains('"version":1'));
      expect(file.parent.listSync().whereType<Directory>(), isEmpty);
    },
  );

  test(
    'catalog uninstall cleans folders, duplicates, missing and new applications',
    () async {
      final temp = Directory.systemTemp.createTempSync(
        'realand-launchpad-prune-',
      );
      addTearDown(() => temp.deleteSync(recursive: true));
      final file = File('${temp.path}/layout.json')
        ..writeAsStringSync(
          jsonEncode({
            'version': 1,
            'entries': [
              {
                'id': 'folder:one',
                'name': '旧文件夹',
                'apps': ['a', 'a', 'removed', 'b'],
              },
              {
                'id': 'b',
                'apps': ['b'],
              },
              {
                'id': 'gone',
                'apps': ['removed'],
              },
              {
                'id': 'malformed',
                'apps': [null, 1],
              },
            ],
          }),
        );
      final layout = MacLaunchpadLayout(file: file);
      addTearDown(layout.dispose);
      await layout.load(['a', 'b', 'new']);
      expect(layout.orderedAppIds, ['a', 'b', 'new']);
      expect(layout.entries.first.name, '旧文件夹');
      layout.reconcile(['b', 'new', 'newer']);
      expect(layout.entries.map((entry) => entry.id), [
        'folder:one',
        'new',
        'newer',
      ]);
      expect(layout.entries.first.appIds, ['b']);
      await layout.save();
    },
  );

  test(
    'catalog arriving during disk read preserves saved folder identity and name',
    () async {
      final temp = Directory.systemTemp.createTempSync(
        'realand-launchpad-race-',
      );
      addTearDown(() => temp.deleteSync(recursive: true));
      final file = File('${temp.path}/layout.json')
        ..writeAsStringSync(
          jsonEncode({
            'version': 1,
            'entries': [
              {
                'id': 'folder:kept',
                'name': 'Saved',
                'apps': ['a', 'b'],
                'type': 'folder',
              },
            ],
          }),
        );
      final gated = _ReadGateFile(file);
      final layout = MacLaunchpadLayout(file: gated);
      addTearDown(layout.dispose);
      final loading = layout.load(const []);
      await gated.reading.future;
      layout.reconcile(['a', 'b', 'new']);
      gated.resume.complete();
      await loading;
      expect(layout.entries.first.id, 'folder:kept');
      expect(layout.entries.first.name, 'Saved');
      expect(layout.entries.first.appIds, ['a', 'b']);
      expect(layout.orderedAppIds, ['a', 'b', 'new']);
      layout.rename('folder:kept', '🌻' * 81);
      expect(layout.entries.first.name, '🌻' * 80);
      await layout.save();
    },
  );

  test(
    'invalid drop targets and stale IDs cannot remove or duplicate applications',
    () async {
      final layout = MacLaunchpadLayout(persist: false);
      addTearDown(layout.dispose);
      await layout.load(['a', 'b', 'c']);
      layout.moveBefore('a', 'missing');
      layout.group('missing', 'b');
      layout.group('a', 'a');
      layout.moveBefore('a', 'a');
      expect(layout.orderedAppIds, ['a', 'b', 'c']);
      layout.group('a', 'b');
      final folder = layout.entries.first;
      layout.group(folder.id, 'c');
      expect(layout.orderedAppIds, ['b', 'a', 'c']);
      layout.rename(folder.id, ' \u0000\n ');
      expect(layout.find(folder.id)!.name, '文件夹');
    },
  );
}
