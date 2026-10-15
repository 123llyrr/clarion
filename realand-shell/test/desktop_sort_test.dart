// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:realand_shell/desktop/desktop_entry.dart';
import 'package:realand_shell/desktop/desktop_sort.dart';

DesktopEntry _entry(
  String name, {
  String? path,
  String? label,
  int size = 0,
  DateTime? modified,
  bool directory = false,
  bool shortcut = false,
}) => DesktopEntry(
  fullPath: path ?? '/desktop/$name',
  name: name,
  displayName: label,
  sizeBytes: size,
  modified: modified,
  isDirectory: directory,
  isDesktopShortcut: shortcut,
);

void main() {
  test('supported keys exclude unavailable creation and addition dates', () {
    for (final key in DesktopSortKey.values) {
      expect(DesktopSort.fromName(key.name), key);
    }
    for (final value in [null, false, {}, 'created', 'added', 'NAME']) {
      expect(DesktopSort.fromName(value), isNull);
    }
  });

  test(
    'names sort numerically using displayed labels without mutating input',
    () {
      final input = [
        _entry('file10'),
        _entry('launcher.desktop', label: 'file2', shortcut: true),
        _entry('file1'),
        _entry('file02'),
        _entry('File2'),
      ];
      final original = input.toList();
      final sorted = DesktopSort.sorted(input, DesktopSortKey.name);
      expect(sorted.map((entry) => entry.displayName), [
        'file1',
        'File2',
        'file02',
        'file2',
        'file10',
      ]);
      expect(input, original);
      expect(
        DesktopSort.compareNames('file${'9' * 100}', 'file1${'0' * 100}'),
        lessThan(0),
      );
    },
  );

  test('same labels and metadata have a deterministic full-path tie break', () {
    final items = [
      _entry('same', path: '/z/same', modified: DateTime.utc(2026)),
      _entry('same', path: '/a/same', modified: DateTime.utc(2026)),
    ];
    for (final key in DesktopSortKey.values) {
      expect(DesktopSort.sorted(items, key).map((entry) => entry.fullPath), [
        '/a/same',
        '/z/same',
      ]);
      expect(DesktopSort.compare(items.first, items.first, key), 0);
    }
  });

  test('modification date is newest first with unknown dates last', () {
    final items = [
      _entry('unknown'),
      _entry('older', modified: DateTime.utc(2025)),
      _entry('newer', modified: DateTime.utc(2026)),
    ];
    expect(
      DesktopSort.sorted(items, DesktopSortKey.modified).map((e) => e.name),
      ['newer', 'older', 'unknown'],
    );
  });

  test('size uses file bytes and does not claim a directory contents size', () {
    final items = [
      _entry('folder', directory: true, size: 999999),
      _entry('empty', size: 0),
      _entry('small', size: 2),
      _entry('large', size: 100),
      _entry('unknown', size: -1),
    ];
    expect(DesktopSort.sorted(items, DesktopSortKey.size).map((e) => e.name), [
      'large',
      'small',
      'empty',
      'folder',
      'unknown',
    ]);
  });

  test('kind uses directory and launcher flags plus actual extensions', () {
    final items = [
      _entry('z.TXT'),
      _entry('picture.png'),
      _entry('.hidden'),
      _entry('folder.txt', directory: true),
      _entry('app.desktop', shortcut: true),
      _entry('a.txt'),
    ];
    expect(DesktopSort.sorted(items, DesktopSortKey.kind).map((e) => e.name), [
      'folder.txt',
      'app.desktop',
      '.hidden',
      'picture.png',
      'a.txt',
      'z.TXT',
    ]);
  });

  test(
    'filtering or scanner enumeration order never changes relative ordering',
    () {
      final entries = [
        for (var index = 12; index >= 0; index--)
          _entry(
            'item$index.${index.isEven ? 'txt' : 'png'}',
            size: index % 3,
            modified: DateTime.utc(2026, 1, index % 3 + 1),
          ),
      ];
      for (final key in DesktopSortKey.values) {
        final sorted = DesktopSort.sorted(entries, key);
        expect(DesktopSort.sorted(entries.reversed, key), sorted);
        expect(
          DesktopSort.sorted(
            entries.where((entry) => entry.sizeBytes != 1),
            key,
          ),
          sorted.where((entry) => entry.sizeBytes != 1),
        );
        for (final a in entries) {
          for (final b in entries) {
            expect(
              DesktopSort.compare(a, b, key).sign,
              -DesktopSort.compare(b, a, key).sign,
            );
          }
        }
      }
    },
  );

  test('actual isolated file size and mtime drive ordering', () async {
    final root = await Directory.systemTemp.createTemp(
      'realand-sort-metadata-',
    );
    addTearDown(() => root.delete(recursive: true));
    final older = File('${root.path}/older');
    final newer = File('${root.path}/newer');
    await older.writeAsString('0123456789');
    await newer.writeAsString('a');
    await older.setLastModified(DateTime.utc(2023, 1, 1));
    await newer.setLastModified(DateTime.utc(2024, 1, 1));
    final entries = <DesktopEntry>[];
    for (final file in [older, newer]) {
      final stat = await file.stat();
      entries.add(
        _entry(
          file.uri.pathSegments.last,
          path: file.path,
          size: stat.size,
          modified: stat.modified,
        ),
      );
    }
    expect(
      DesktopSort.sorted(entries, DesktopSortKey.size).first.fullPath,
      older.path,
    );
    expect(
      DesktopSort.sorted(entries, DesktopSortKey.modified).first.fullPath,
      newer.path,
    );
  });
}
