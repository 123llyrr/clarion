// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:realand_shell/desktop/desktop_entry.dart';
import 'package:realand_shell/desktop/desktop_layout.dart';
import 'package:realand_shell/desktop/desktop_service.dart';
import 'package:realand_shell/desktop/desktop_stacks.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  group('DesktopStacks', () {
    test('groups entries by kind accurately', () {
      final entries = [
        const DesktopEntry(
          fullPath: '/desktop/photo.png',
          name: 'photo.png',
          displayName: 'photo.png',
        ),
        const DesktopEntry(
          fullPath: '/desktop/document.docx',
          name: 'document.docx',
          displayName: 'document.docx',
        ),
        const DesktopEntry(
          fullPath: '/desktop/invoice.pdf',
          name: 'invoice.pdf',
          displayName: 'invoice.pdf',
        ),
        const DesktopEntry(
          fullPath: '/desktop/song.mp3',
          name: 'song.mp3',
          displayName: 'song.mp3',
        ),
        const DesktopEntry(
          fullPath: '/desktop/script.dart',
          name: 'script.dart',
          displayName: 'script.dart',
        ),
        const DesktopEntry(
          fullPath: '/desktop/archive.zip',
          name: 'archive.zip',
          displayName: 'archive.zip',
        ),
        const DesktopEntry(
          fullPath: '/desktop/Folder',
          name: 'Folder',
          displayName: 'Folder',
          isDirectory: true,
        ),
      ];

      final groups = DesktopStacks.groupEntries(
        entries,
        groupBy: DesktopSortKey.kind,
      );

      // Folders must remain excluded from stacks
      expect(groups.any((g) => g.entries.any((e) => e.isDirectory)), isFalse);

      final categories = groups.map((g) => g.category).toSet();
      expect(
        categories,
        containsAll([
          '图像',
          '文稿',
          'PDF 凭证与文稿',
          '音乐与音频',
          '代码与脚本',
          '归档与压缩包',
        ]),
      );

      final imageGroup = groups.firstWhere((g) => g.category == '图像');
      expect(imageGroup.entries, hasLength(1));
      expect(imageGroup.title, '图像 (1)');
    });

    test('groups entries by size', () {
      final entries = [
        const DesktopEntry(
          fullPath: '/desktop/small.txt',
          name: 'small.txt',
          displayName: 'small.txt',
          sizeBytes: 1024, // 1 KB
        ),
        const DesktopEntry(
          fullPath: '/desktop/huge.iso',
          name: 'huge.iso',
          displayName: 'huge.iso',
          sizeBytes: 200 * 1024 * 1024, // 200 MB
        ),
      ];

      final groups = DesktopStacks.groupEntries(
        entries,
        groupBy: DesktopSortKey.size,
      );

      expect(groups.map((g) => g.category).toSet(), {
        '较小 (< 1 MB)',
        '巨大 (> 100 MB)',
      });
    });
  });

  group('DesktopService clipboard and duplicate', () {
    late Directory tempDir;

    setUp(() async {
      tempDir = await Directory.systemTemp.createTemp('realand-desktop-test-');
    });

    tearDown(() async {
      if (await tempDir.exists()) {
        await tempDir.delete(recursive: true);
      }
    });

    test('copySelection updates clipboard state', () async {
      final service = DesktopService(root: tempDir.path);
      addTearDown(service.dispose);
      await service.start();

      expect(service.canPaste, isFalse);
      expect(service.clipboardPaths, isEmpty);

      const entry = DesktopEntry(
        fullPath: '/some/path/file.txt',
        name: 'file.txt',
        displayName: 'file.txt',
      );

      service.copySelection([entry]);
      expect(service.canPaste, isTrue);
      expect(service.clipboardPaths, ['/some/path/file.txt']);
    });

    test('duplicateSelection duplicates files using macOS naming', () async {
      final service = DesktopService(root: tempDir.path);
      addTearDown(service.dispose);
      await service.start();

      final originalFile = File('${tempDir.path}/Report.txt');
      await originalFile.writeAsString('Hello Realand');
      await service.refresh();

      final entries = service.entries;
      expect(entries, hasLength(1));

      // Duplicate Report.txt -> Report 副本.txt
      await service.duplicateSelection(entries);
      expect(File('${tempDir.path}/Report 副本.txt').existsSync(), isTrue);

      // Duplicate again -> Report 副本 2.txt
      await service.duplicateSelection(entries);
      expect(File('${tempDir.path}/Report 副本 2.txt').existsSync(), isTrue);
    });

    test('paste copies external file into desktop root', () async {
      final service = DesktopService(root: tempDir.path);
      addTearDown(service.dispose);
      await service.start();

      final sourceDir = await Directory.systemTemp.createTemp('realand-src-');
      addTearDown(() => sourceDir.delete(recursive: true));
      final srcFile = File('${sourceDir.path}/Import.csv');
      await srcFile.writeAsString('id,name\n1,Realand');

      service.copySelection([
        DesktopEntry(
          fullPath: srcFile.path,
          name: 'Import.csv',
          displayName: 'Import.csv',
        ),
      ]);

      await service.paste();
      expect(File('${tempDir.path}/Import.csv').existsSync(), isTrue);
      expect(
        await File('${tempDir.path}/Import.csv').readAsString(),
        'id,name\n1,Realand',
      );
    });
  });

  group('DesktopLayoutStore Stacks persistence', () {
    late Directory tempDir;

    setUp(() async {
      tempDir = await Directory.systemTemp.createTemp('realand-layout-test-');
    });

    tearDown(() async {
      if (await tempDir.exists()) {
        await tempDir.delete(recursive: true);
      }
    });

    test('persists stacksEnabled and stackGroupBy', () async {
      final file = File('${tempDir.path}/desktop-layout.json');
      final store1 = DesktopLayoutStore(file: file);
      await store1.load();

      expect(store1.stacksEnabled('eDP-1'), isFalse);
      expect(store1.stackGroupBy('eDP-1'), DesktopSortKey.kind);

      await store1.setStacksEnabled('eDP-1', true, groupBy: DesktopSortKey.size);
      expect(store1.stacksEnabled('eDP-1'), isTrue);
      expect(store1.stackGroupBy('eDP-1'), DesktopSortKey.size);

      // Reload store from disk
      final store2 = DesktopLayoutStore(file: file);
      await store2.load();
      expect(store2.stacksEnabled('eDP-1'), isTrue);
      expect(store2.stackGroupBy('eDP-1'), DesktopSortKey.size);
    });
  });
}
