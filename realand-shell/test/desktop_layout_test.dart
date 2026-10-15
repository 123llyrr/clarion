// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:convert';
import 'dart:io';
import 'dart:isolate';

import 'package:flutter/painting.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:realand_shell/desktop/desktop_entry.dart';
import 'package:realand_shell/desktop/desktop_layout.dart';
import 'package:realand_shell/models.dart';

void main() {
  DesktopEntry entry(String name) =>
      DesktopEntry(fullPath: '/$name', name: name);

  test(
    'full cleanup packs visual order across pages without losing unrelated paths',
    () {
      final grid = DesktopGridSpec.fromSize(
        const Size(300, 400),
        barPosition: BarPosition.top,
      );
      expect(grid.capacity, 4);
      final saved = {
        '/z': const DesktopCell(1, 1),
        '/a': const DesktopCell(0, 1, page: 1),
        '/b': const DesktopCell(1, 1, page: 1),
        '/unrelated': const DesktopCell(0, 0),
      };
      final arranged = DesktopLayout.arrange(
        [entry('b'), entry('a'), entry('z')],
        grid,
        saved,
      );
      expect(arranged, {
        '/unrelated': const DesktopCell(0, 0),
        '/z': const DesktopCell(0, 1),
        '/a': const DesktopCell(1, 0),
        '/b': const DesktopCell(1, 1),
      });
      expect(saved['/b'], const DesktopCell(1, 1, page: 1));
    },
  );

  test(
    'cleanup selection only reorders its original cells and preserves other pages',
    () {
      final grid = DesktopGridSpec.fromSize(
        const Size(300, 400),
        barPosition: BarPosition.top,
      );
      final saved = {
        '/z': const DesktopCell(1, 0),
        '/a': const DesktopCell(1, 1),
        '/unselected': const DesktopCell(0, 1),
        '/another-page': const DesktopCell(0, 0, page: 1),
        '/filtered-out': const DesktopCell(1, 1, page: 1),
      };
      final arranged = DesktopLayout.arrange(
        [entry('z'), entry('a'), entry('unselected'), entry('another-page')],
        grid,
        saved,
        sortKey: DesktopSortKey.name,
        selection: {'/a', '/z', '/missing'},
      );
      expect(arranged, {
        ...saved,
        '/a': const DesktopCell(1, 0),
        '/z': const DesktopCell(1, 1),
      });
      expect(arranged.values.toSet(), hasLength(saved.length));
      expect(
        DesktopLayout.arrange([entry('a')], grid, saved, selection: {}),
        saved,
      );
    },
  );

  test(
    'automatic order spans pages and incoming entries do not restore manual cells',
    () {
      final grid = DesktopGridSpec.fromSize(
        const Size(300, 400),
        barPosition: BarPosition.top,
      );
      final entries = [for (var i = 6; i > 0; i--) entry('file$i')];
      final saved = {'/file1': const DesktopCell(1, 1, page: 1)};
      final ordered = DesktopLayout.resolveEntries(
        entries,
        grid,
        saved,
        sortKey: DesktopSortKey.name,
      );
      expect(ordered['/file1'], const DesktopCell(0, 0));
      expect(ordered['/file5'], const DesktopCell(0, 0, page: 1));
      expect(ordered['/file6'], const DesktopCell(0, 1, page: 1));
      expect(ordered.values.toSet(), hasLength(6));
      final refreshed = DesktopLayout.resolveEntries(
        [...entries.reversed, entry('file0')],
        grid,
        saved,
        sortKey: DesktopSortKey.name,
      );
      expect(refreshed['/file0'], const DesktopCell(0, 0));
      expect(refreshed['/file1'], const DesktopCell(0, 1));
      expect(saved, {'/file1': const DesktopCell(1, 1, page: 1)});
      expect(DesktopLayout.resolveEntries(entries, grid, ordered), ordered);
    },
  );

  test('zero-sized output cannot erase saved positions during cleanup', () {
    final saved = {'/a': const DesktopCell(2, 3, page: 4)};
    final grid = DesktopGridSpec.fromSize(
      const Size(10, 10),
      barPosition: BarPosition.top,
    );
    expect(DesktopLayout.arrange([entry('a')], grid, saved), saved);
    expect(
      DesktopLayout.resolveEntries(
        [entry('a')],
        grid,
        saved,
        sortKey: DesktopSortKey.name,
      ),
      isEmpty,
    );
  });

  test(
    'legacy version-one layouts migrate sorting per output without rewriting the base',
    () async {
      final root = await Directory.systemTemp.createTemp(
        'realand-sort-migration-',
      );
      addTearDown(() => root.delete(recursive: true));
      final file = File('${root.path}/layout.json');
      final legacy = jsonEncode({
        'version': 1,
        'outputs': {
          'DP-1': {'/left': const DesktopCell(1, 0).toJson()},
          'eDP-1': {'/right': const DesktopCell(0, 1).toJson()},
        },
      });
      await file.writeAsString(legacy);
      final store = DesktopLayoutStore(file: file);
      addTearDown(store.dispose);
      await store.load();
      expect(store.sortKey('DP-1'), isNull);
      for (final key in DesktopSortKey.values) {
        await store.setSortKey('DP-1', key);
        final snapshot =
            jsonDecode(await store.fileForOutput('DP-1')!.readAsString())
                as Map;
        expect(snapshot['version'], 1);
        expect(snapshot['sorting'], {
          'version': 1,
          'outputs': {'DP-1': key.name},
        });
      }
      final restored = DesktopLayoutStore(file: file);
      addTearDown(restored.dispose);
      await restored.load();
      expect(restored.sortKey('DP-1'), DesktopSortKey.size);
      expect(restored.positions('DP-1'), {'/left': const DesktopCell(1, 0)});
      expect(restored.sortKey('eDP-1'), isNull);
      expect(restored.positions('eDP-1'), {'/right': const DesktopCell(0, 1)});
      expect(await file.readAsString(), legacy);
    },
  );

  test(
    'choosing manual mode freezes visible cells and ordinary saves preserve sort mode',
    () async {
      final root = await Directory.systemTemp.createTemp(
        'realand-sort-manual-',
      );
      addTearDown(() => root.delete(recursive: true));
      final file = File('${root.path}/layout.json');
      final store = DesktopLayoutStore(file: file);
      addTearDown(store.dispose);
      await store.setSortKey(
        'eDP-1',
        DesktopSortKey.kind,
        positions: {'/other': const DesktopCell(1, 1)},
      );
      await store.setSortKey('DP-1', DesktopSortKey.name);
      final visible = {
        '/a': const DesktopCell(0, 0),
        '/b': const DesktopCell(0, 1),
      };
      await store.savePositions('DP-1', visible);
      expect(store.sortKey('DP-1'), DesktopSortKey.name);
      final observations = <(DesktopSortKey?, Map<String, DesktopCell>)>[];
      store.addListener(
        () =>
            observations.add((store.sortKey('DP-1'), store.positions('DP-1'))),
      );
      await store.setSortKey('DP-1', null, positions: visible);
      expect(observations, hasLength(1));
      expect(observations.single.$1, isNull);
      expect(observations.single.$2, visible);
      final moved = {...visible, '/b': const DesktopCell(1, 0)};
      await store.saveManualPositions('DP-1', moved);
      final restored = DesktopLayoutStore(file: file);
      addTearDown(restored.dispose);
      await restored.load();
      expect(restored.sortKey('DP-1'), isNull);
      expect(restored.positions('DP-1'), moved);
      expect(restored.sortKey('eDP-1'), DesktopSortKey.kind);
      expect(restored.positions('eDP-1'), {'/other': const DesktopCell(1, 1)});
    },
  );

  test(
    'unsupported or malformed sort metadata cannot discard valid old positions',
    () async {
      final root = await Directory.systemTemp.createTemp(
        'realand-sort-invalid-',
      );
      addTearDown(() => root.delete(recursive: true));
      final file = File('${root.path}/layout.json');
      for (final sorting in [
        null,
        [],
        {
          'version': 2,
          'outputs': {'DP-1': 'name'},
        },
        {
          'version': 1,
          'outputs': {'DP-1': 'created'},
        },
        {
          'version': 1,
          'outputs': {'DP-1': true},
        },
      ]) {
        await file.writeAsString(
          jsonEncode({
            'version': 1,
            'outputs': {
              'DP-1': {'/a': const DesktopCell(1, 1).toJson()},
            },
            'sorting': sorting,
          }),
        );
        final store = DesktopLayoutStore(file: file);
        await store.load();
        expect(store.positions('DP-1'), {'/a': const DesktopCell(1, 1)});
        expect(store.sortKey('DP-1'), isNull);
        store.dispose();
      }
    },
  );

  test(
    'failed sorting save retains desired cells and mode and can retry',
    () async {
      final root = await Directory.systemTemp.createTemp('realand-sort-error-');
      addTearDown(() => root.delete(recursive: true));
      final store = DesktopLayoutStore(file: File('${root.path}/layout.json'));
      addTearDown(store.dispose);
      await store.load();
      final blocker = File(store.fileForOutput('DP-1')!.parent.path);
      await blocker.writeAsString('preserve blocker');
      final positions = {'/a': const DesktopCell(1, 0)};
      await store.setSortKey(
        'DP-1',
        DesktopSortKey.modified,
        positions: positions,
      );
      expect(store.error, contains('桌面布局保存失败'));
      expect(store.positions('DP-1'), positions);
      expect(store.sortKey('DP-1'), DesktopSortKey.modified);
      expect(await blocker.readAsString(), 'preserve blocker');
      await blocker.delete();
      await store.savePositions('DP-1', positions);
      expect(store.error, isNull);
      final restored = DesktopLayoutStore(file: store.file);
      addTearDown(restored.dispose);
      await restored.load();
      expect(restored.positions('DP-1'), positions);
      expect(restored.sortKey('DP-1'), DesktopSortKey.modified);
    },
  );

  test(
    'group moves preserve offsets and exchange occupied targets with vacated cells',
    () {
      final grid = DesktopGridSpec.fromSize(
        const Size(700, 700),
        barPosition: BarPosition.top,
      );
      final original = {
        '/a': const DesktopCell(0, 0),
        '/b': const DesktopCell(0, 1),
        '/c': const DesktopCell(0, 2),
        '/elsewhere': const DesktopCell(1, 1, page: 1),
      };
      final moved = DesktopLayout.moveGroup(
        original,
        {'/a', '/b'},
        '/a',
        const DesktopCell(0, 1),
        grid,
      )!;
      expect(moved['/a'], const DesktopCell(0, 1));
      expect(moved['/b'], const DesktopCell(0, 2));
      expect(moved['/c'], const DesktopCell(0, 0));
      expect(moved['/elsewhere'], original['/elsewhere']);
      expect(moved.values.toSet(), hasLength(original.length));
      expect(original['/a'], const DesktopCell(0, 0));
      final free = DesktopLayout.moveGroup(
        original,
        {'/a', '/b'},
        '/a',
        const DesktopCell(1, 0),
        grid,
      )!;
      expect(free['/a'], const DesktopCell(1, 0));
      expect(free['/b'], const DesktopCell(1, 1));
      expect(free['/c'], original['/c']);
    },
  );

  test(
    'group move rejects missing, cross-page and out-of-bounds members atomically',
    () {
      final grid = DesktopGridSpec.fromSize(
        const Size(300, 400),
        barPosition: BarPosition.top,
      );
      final original = {
        '/a': const DesktopCell(0, 0),
        '/b': const DesktopCell(0, 1),
        '/other-page': const DesktopCell(0, 0, page: 1),
      };
      for (final selection in [
        {'/a', '/missing'},
        {'/a', '/other-page'},
      ]) {
        expect(
          DesktopLayout.moveGroup(
            original,
            selection,
            '/a',
            const DesktopCell(1, 0),
            grid,
          ),
          isNull,
        );
      }
      expect(
        DesktopLayout.moveGroup(
          original,
          {'/a', '/b'},
          '/b',
          const DesktopCell(1, 0),
          grid,
        ),
        isNull,
      );
      expect(
        DesktopLayout.moveGroup(
          original,
          {'/a', '/b'},
          '/a',
          DesktopCell(0, grid.rows - 1),
          grid,
        ),
        isNull,
      );
      expect(original['/b'], const DesktopCell(0, 1));
    },
  );

  test('cells round trip and invalid coordinates cannot enter the layout', () {
    const cell = DesktopCell(2, 3, page: 1);
    final restored = DesktopCell.fromJson(cell.toJson());
    expect(restored, cell);
    expect({cell, restored}.length, 1);
    for (final value in [
      null,
      [],
      {'row': 0},
      {'column': -1, 'row': 0},
      {'column': 0, 'row': 0, 'page': 9000},
      {'column': 1.5, 'row': 0},
    ]) {
      expect(DesktopCell.fromJson(value), isNull);
    }
  });

  for (final position in BarPosition.values) {
    for (final scale in [1.0, 1.25, 1.5]) {
      test('logical $position layout fits the output at scale $scale', () {
        final logicalSize = Size(1920 / scale, 1080 / scale);
        final grid = DesktopGridSpec.fromSize(
          logicalSize,
          barPosition: position,
        );
        final paths = List.generate(128, (index) => '/isolated/file-$index');
        final cells = DesktopLayout.resolve(paths, grid, {});
        expect(cells.length, 128);
        expect(cells.values.toSet().length, 128);
        for (final cell in cells.values) {
          final rect = grid.tileRect(cell);
          expect(rect.left, greaterThanOrEqualTo(0));
          expect(rect.right, lessThanOrEqualTo(logicalSize.width));
          expect(
            rect.top,
            greaterThanOrEqualTo(position == BarPosition.top ? 40 : 0),
          );
          expect(
            rect.bottom,
            lessThanOrEqualTo(
              logicalSize.height - (position == BarPosition.bottom ? 40 : 0),
            ),
          );
        }
        expect(
          cells.values.map((cell) => cell.page).toSet().length,
          grid.pageCount(128),
        );
        final first = cells[paths.first]!;
        expect(
          grid.cellAt(grid.tileRect(first).center, page: first.page),
          first,
        );
        expect(grid.cellAt(const Offset(-10, -10)), isNull);
      });
    }
  }

  test('resize resolves collisions and keeps existing desired cells first', () {
    final grid = DesktopGridSpec.fromSize(
      const Size(300, 400),
      barPosition: BarPosition.top,
    );
    final cells = DesktopLayout.resolve(
      ['/isolated/a', '/isolated/b', '/isolated/c'],
      grid,
      {
        '/isolated/a': const DesktopCell(50, 50, page: 10),
        '/isolated/b': const DesktopCell(50, 50, page: 10),
        '/isolated/c': const DesktopCell(0, 0),
      },
    );
    expect(cells.values.toSet().length, 3);
    expect(cells['/isolated/c'], const DesktopCell(0, 0));
    expect(cells['/isolated/a'], DesktopCell(grid.columns - 1, grid.rows - 1));
    expect(
      DesktopLayout.resolve(
        ['/isolated/a'],
        DesktopGridSpec.fromSize(
          const Size(20, 20),
          barPosition: BarPosition.top,
        ),
        {},
      ),
      isEmpty,
    );
  });

  test(
    'layouts persist separately by output and concurrent stores merge',
    () async {
      final root = await Directory.systemTemp.createTemp(
        'realand-desktop-layout-',
      );
      addTearDown(() => root.delete(recursive: true));
      final file = File('${root.path}/config/desktop-layout.json');
      final left = DesktopLayoutStore(file: file);
      final right = DesktopLayoutStore(file: file);
      addTearDown(left.dispose);
      addTearDown(right.dispose);
      await Future.wait([left.load(), right.load()]);
      await Future.wait([
        left.savePositions('DP-1', {
          '${root.path}/file': const DesktopCell(1, 2),
        }),
        right.savePositions('eDP-1', {
          '${root.path}/file': const DesktopCell(2, 1),
        }),
      ]);
      final restored = DesktopLayoutStore(file: file);
      addTearDown(restored.dispose);
      await restored.load();
      expect(
        restored.positions('DP-1')['${root.path}/file'],
        const DesktopCell(1, 2),
      );
      expect(
        restored.positions('eDP-1')['${root.path}/file'],
        const DesktopCell(2, 1),
      );
      expect(restored.positions('missing'), isEmpty);
      final outputFile = restored.fileForOutput('DP-1')!;
      expect(outputFile.path, isNot(restored.fileForOutput('eDP-1')!.path));
      expect(
        await outputFile.parent
            .list()
            .where((entry) => entry.path.contains('.desktop-layout-'))
            .isEmpty,
        isTrue,
      );
      final parsed = jsonDecode(await outputFile.readAsString()) as Map;
      expect(parsed['version'], 1);
    },
  );

  test(
    'corrupt and oversized layout data fall back without deleting files',
    () async {
      final root = await Directory.systemTemp.createTemp(
        'realand-desktop-layout-invalid-',
      );
      addTearDown(() => root.delete(recursive: true));
      final file = File('${root.path}/layout.json');
      for (final contents in [
        '{bad',
        jsonEncode({'version': 2, 'outputs': {}}),
        'x' * (1024 * 1024 + 1),
      ]) {
        await file.writeAsString(contents);
        final store = DesktopLayoutStore(file: file);
        await store.load();
        expect(store.positions('DP-1'), isEmpty);
        expect(await file.readAsString(), contents);
        store.dispose();
      }
    },
  );

  test(
    'independent Dart engines cannot overwrite a different output layout',
    () async {
      final root = await Directory.systemTemp.createTemp(
        'realand-desktop-layout-isolates-',
      );
      addTearDown(() => root.delete(recursive: true));
      final path = '${root.path}/layout.json';
      final itemPath = '${root.path}/item';
      await Future.wait([
        Isolate.run(() async {
          final store = DesktopLayoutStore(file: File(path));
          await store.setSortKey(
            'DP-1',
            DesktopSortKey.name,
            positions: {itemPath: const DesktopCell(0, 2)},
          );
          store.dispose();
        }),
        Isolate.run(() async {
          final store = DesktopLayoutStore(file: File(path));
          await store.setSortKey(
            'eDP-1',
            DesktopSortKey.modified,
            positions: {itemPath: const DesktopCell(2, 0)},
          );
          store.dispose();
        }),
      ]);
      final restored = DesktopLayoutStore(file: File(path));
      addTearDown(restored.dispose);
      await restored.load();
      expect(restored.positions('DP-1')[itemPath], const DesktopCell(0, 2));
      expect(restored.positions('eDP-1')[itemPath], const DesktopCell(2, 0));
      expect(restored.sortKey('DP-1'), DesktopSortKey.name);
      expect(restored.sortKey('eDP-1'), DesktopSortKey.modified);
      expect(
        restored.fileForOutput('../../../unsafe')!.parent.path,
        '${root.path}/desktop-layouts',
      );
    },
  );

  test(
    'a saved output drops stale paths without changing another output',
    () async {
      final root = await Directory.systemTemp.createTemp(
        'realand-desktop-layout-prune-',
      );
      addTearDown(() => root.delete(recursive: true));
      final file = File('${root.path}/layout.json');
      final store = DesktopLayoutStore(file: file);
      addTearDown(store.dispose);
      await store.savePositions('DP-1', {
        '${root.path}/old': const DesktopCell(0, 0),
      });
      await store.savePositions('eDP-1', {
        '${root.path}/old': const DesktopCell(1, 0),
      });
      await store.savePositions('DP-1', {
        '${root.path}/new': const DesktopCell(0, 1),
      });
      final restored = DesktopLayoutStore(file: file);
      addTearDown(restored.dispose);
      await restored.load();
      expect(restored.positions('DP-1').keys, ['${root.path}/new']);
      expect(restored.positions('eDP-1').keys, ['${root.path}/old']);
    },
  );

  test(
    'real filesystem failures report errors and retain in-memory placement',
    () async {
      final root = await Directory.systemTemp.createTemp(
        'realand-desktop-layout-errors-',
      );
      addTearDown(() => root.delete(recursive: true));
      final file = File('${root.path}/layout.json');
      final seed = DesktopLayoutStore(file: file);
      addTearDown(seed.dispose);
      await seed.savePositions('DP-1', {
        '${root.path}/item': const DesktopCell(0, 0),
      });
      expect(seed.error, isNull);
      final directory = seed.fileForOutput('DP-1')!.parent;
      final store = DesktopLayoutStore(file: file);
      addTearDown(store.dispose);
      var notifications = 0;
      store.addListener(() => notifications++);

      final deny = await Process.run('chmod', ['000', '--', directory.path]);
      expect(deny.exitCode, 0);
      try {
        // Listing this real directory raises EACCES. load must finish with
        // feedback instead of leaking the stream's asynchronous exception.
        await expectLater(store.load(), completes);
        expect(store.error, contains('桌面布局读取失败'));
        expect(store.positions('DP-1'), isEmpty);
        expect(notifications, 1);
      } finally {
        final restore = await Process.run('chmod', [
          '700',
          '--',
          directory.path,
        ]);
        expect(restore.exitCode, 0);
      }

      // A regular file where the output directory belongs guarantees ENOTDIR
      // for writes independently of permission semantics or chmod support.
      await directory.delete(recursive: true);
      final blocker = File(directory.path);
      await blocker.writeAsString('keep this blocker');
      final desired = {'${root.path}/item': const DesktopCell(2, 1)};
      await expectLater(store.savePositions('DP-1', desired), completes);
      expect(store.error, contains('桌面布局保存失败'));
      expect(store.positions('DP-1'), desired);
      expect(await blocker.readAsString(), 'keep this blocker');
      expect(notifications, 3);

      // A failed write must not poison the serialized queue: fixing the path
      // allows the next save to finish and clear the error.
      await blocker.delete();
      await expectLater(store.savePositions('DP-1', desired), completes);
      expect(store.error, isNull);
      final restored = DesktopLayoutStore(file: file);
      addTearDown(restored.dispose);
      await restored.load();
      expect(restored.error, isNull);
      expect(restored.positions('DP-1'), desired);
    },
  );
}
