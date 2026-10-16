// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'dart:math' as math;

import 'package:flutter/foundation.dart';
import 'package:flutter/painting.dart';

import '../models.dart';
import '../services/identity.dart';
import 'desktop_entry.dart';
import 'desktop_sort.dart';

export 'desktop_sort.dart' show DesktopSortKey;

class DesktopCell {
  const DesktopCell(this.column, this.row, {this.page = 0});

  final int column;
  final int row;
  final int page;

  Map<String, int> toJson() => {'column': column, 'row': row, 'page': page};

  static DesktopCell? fromJson(Object? value) {
    if (value is! Map) return null;
    final column = value['column'];
    final row = value['row'];
    final page = value['page'] ?? 0;
    if (column is! int || row is! int || page is! int) return null;
    if ([column, row, page].any((number) => number < 0 || number > 4096)) {
      return null;
    }
    return DesktopCell(column, row, page: page);
  }

  @override
  bool operator ==(Object other) =>
      other is DesktopCell &&
      column == other.column &&
      row == other.row &&
      page == other.page;
  @override
  int get hashCode => Object.hash(column, row, page);
}

/// All coordinates are logical pixels. Output scaling is handled by Flutter.
class DesktopGridSpec {
  DesktopGridSpec.fromSize(
    Size size, {
    required BarPosition barPosition,
    double barHeight = 40,
    EdgeInsets? surfaceInsets,
    double padding = 16,
    double cellWidth = 128,
    double cellHeight = 132,
  }) : area = Rect.fromLTWH(
         padding + (surfaceInsets?.left ?? 0),
         padding +
             (surfaceInsets?.top ??
                 (barPosition == BarPosition.top ? barHeight : 0)),
         math.max(
           0,
           size.width - padding * 2 - (surfaceInsets?.horizontal ?? 0),
         ),
         math.max(
           0,
           size.height -
               padding * 2 -
               (surfaceInsets == null
                   ? barHeight
                   : surfaceInsets.top + surfaceInsets.bottom),
         ),
       ) {
    columns = area.width < 1
        ? 0
        : math.max(1, (area.width / cellWidth).floor());
    rows = area.height < 1
        ? 0
        : math.max(1, (area.height / cellHeight).floor());
    slotWidth = math.min(cellWidth, area.width);
    slotHeight = math.min(cellHeight, area.height);
  }

  final Rect area;
  late final int columns;
  late final int rows;
  late final double slotWidth;
  late final double slotHeight;
  int get capacity => columns * rows;
  int pageCount(int entries) =>
      capacity == 0 ? 1 : math.max(1, (entries / capacity).ceil());

  Rect tileRect(DesktopCell cell) => Rect.fromLTWH(
    area.left + cell.column * slotWidth + math.min(4, slotWidth / 2),
    area.top + cell.row * slotHeight + math.min(4, slotHeight / 2),
    math.max(0, slotWidth - 8),
    math.max(0, slotHeight - 8),
  );

  DesktopCell? cellAt(Offset point, {int page = 0}) {
    if (capacity == 0 || !area.contains(point)) return null;
    return DesktopCell(
      ((point.dx - area.left) / slotWidth).floor().clamp(0, columns - 1),
      ((point.dy - area.top) / slotHeight).floor().clamp(0, rows - 1),
      page: page,
    );
  }
}

abstract final class DesktopLayout {
  /// Resolve automatic sorting without rewriting the saved manual positions.
  /// Callers pass the output's full entry snapshot, not just its visible page.
  static Map<String, DesktopCell> resolveEntries(
    List<DesktopEntry> entries,
    DesktopGridSpec grid,
    Map<String, DesktopCell> saved, {
    DesktopSortKey? sortKey,
  }) => sortKey == null
      ? resolve(entries.map((entry) => entry.fullPath).toList(), grid, saved)
      : arrange(entries, grid, const {}, sortKey: sortKey);

  /// One-shot arrangement does not enable automatic sorting. Unrelated saved
  /// paths are retained exactly. A selection is reordered only within its own
  /// existing cells; unselected items, empty gaps and other pages do not move.
  /// Full arrangement packs the supplied entries into free cells across pages.
  /// Without a sort key, current spatial order is retained during packing.
  static Map<String, DesktopCell> arrange(
    Iterable<DesktopEntry> entries,
    DesktopGridSpec grid,
    Map<String, DesktopCell> saved, {
    DesktopSortKey? sortKey,
    Set<String>? selection,
  }) {
    if (grid.capacity == 0) return Map.of(saved);
    final targets = <String, DesktopEntry>{
      for (final entry in entries)
        if (selection == null || selection.contains(entry.fullPath))
          entry.fullPath: entry,
    };
    final ordered = targets.values.toList()
      ..sort((a, b) {
        if (sortKey != null) return DesktopSort.compare(a, b, sortKey);
        final aCell = saved[a.fullPath];
        final bCell = saved[b.fullPath];
        if (aCell != null && bCell != null) {
          final position = _compareCells(aCell, bCell);
          if (position != 0) return position;
        } else if (aCell != null || bCell != null) {
          return aCell == null ? 1 : -1;
        }
        return DesktopSort.compare(a, b, DesktopSortKey.name);
      });
    final result = Map<String, DesktopCell>.of(saved);
    final occupied = {
      for (final entry in saved.entries)
        if (!targets.containsKey(entry.key)) entry.value,
    };
    final retained =
        selection == null
              ? <DesktopCell>[]
              : {
                  for (final path in targets.keys)
                    if (saved[path] case final cell?)
                      if (_fits(cell, grid) && !occupied.contains(cell)) cell,
                }.toList()
          ..sort(_compareCells);
    // Existing selected slots are preferred; new or invalid selected entries
    // fall back to free slots without displacing any unrelated item.
    occupied.addAll(retained);
    var slot = 0;
    for (var index = 0; index < ordered.length; index++) {
      DesktopCell cell;
      if (index < retained.length) {
        cell = retained[index];
      } else {
        while (true) {
          final local = slot % grid.capacity;
          cell = DesktopCell(
            local ~/ grid.rows,
            local % grid.rows,
            page: slot ~/ grid.capacity,
          );
          slot++;
          if (occupied.add(cell)) break;
        }
      }
      result[ordered[index].fullPath] = cell;
    }
    return result;
  }

  static bool _fits(DesktopCell cell, DesktopGridSpec grid) =>
      cell.column >= 0 &&
      cell.column < grid.columns &&
      cell.row >= 0 &&
      cell.row < grid.rows &&
      cell.page >= 0;

  static int _compareCells(DesktopCell a, DesktopCell b) {
    final page = a.page.compareTo(b.page);
    if (page != 0) return page;
    final column = a.column.compareTo(b.column);
    return column != 0 ? column : a.row.compareTo(b.row);
  }

  /// Move a group by one shared grid delta. Occupied destination cells exchange
  /// with vacated source cells; no item is dropped, overlapped or moved off-page.
  static Map<String, DesktopCell>? moveGroup(
    Map<String, DesktopCell> positions,
    Set<String> selection,
    String anchor,
    DesktopCell target,
    DesktopGridSpec grid,
  ) {
    final origin = positions[anchor];
    if (origin == null ||
        !selection.contains(anchor) ||
        origin.page != target.page) {
      return null;
    }
    final destinations = <String, DesktopCell>{};
    final sources = <DesktopCell>{};
    for (final path in selection) {
      final cell = positions[path];
      if (cell == null || cell.page != origin.page) return null;
      final next = DesktopCell(
        cell.column + target.column - origin.column,
        cell.row + target.row - origin.row,
        page: cell.page,
      );
      if (next.column < 0 ||
          next.column >= grid.columns ||
          next.row < 0 ||
          next.row >= grid.rows) {
        return null;
      }
      sources.add(cell);
      destinations[path] = next;
    }
    final targets = destinations.values.toSet();
    final vacancies = sources.difference(targets).toList()
      ..sort(
        (a, b) => (a.column * grid.rows + a.row).compareTo(
          b.column * grid.rows + b.row,
        ),
      );
    final displaced =
        positions.entries
            .where(
              (entry) =>
                  !selection.contains(entry.key) &&
                  targets.contains(entry.value),
            )
            .toList()
          ..sort(
            (a, b) => (a.value.column * grid.rows + a.value.row).compareTo(
              b.value.column * grid.rows + b.value.row,
            ),
          );
    if (displaced.length > vacancies.length) return null;
    return {
      ...positions,
      ...destinations,
      for (var i = 0; i < displaced.length; i++) displaced[i].key: vacancies[i],
    };
  }

  static Map<String, DesktopCell> resolve(
    List<String> paths,
    DesktopGridSpec grid,
    Map<String, DesktopCell> saved,
  ) {
    if (grid.capacity == 0) return const {};
    final result = <String, DesktopCell>{};
    final occupied = <DesktopCell>{};
    final ordered = paths.toSet().toList()..sort();
    final lastPage = grid.pageCount(ordered.length) - 1;
    for (final path in ordered) {
      final old = saved[path];
      if (old == null) continue;
      final cell = DesktopCell(
        old.column.clamp(0, grid.columns - 1),
        old.row.clamp(0, grid.rows - 1),
        page: old.page.clamp(0, lastPage),
      );
      if (occupied.add(cell)) result[path] = cell;
    }
    var slot = 0;
    for (final path in ordered) {
      if (result.containsKey(path)) continue;
      while (true) {
        final local = slot % grid.capacity;
        final cell = DesktopCell(
          local ~/ grid.rows,
          local % grid.rows,
          page: slot ~/ grid.capacity,
        );
        slot++;
        if (!occupied.add(cell)) continue;
        result[path] = cell;
        break;
      }
    }
    return result;
  }
}

class _LayoutDocument {
  final outputs = <String, Map<String, DesktopCell>>{};
  final sorting = <String, DesktopSortKey?>{};
  final stacks = <String, bool>{};
  final stackGroupBy = <String, DesktopSortKey>{};

  void addAll(_LayoutDocument other) {
    outputs.addAll(other.outputs);
    sorting.addAll(other.sorting);
    stacks.addAll(other.stacks);
    stackGroupBy.addAll(other.stackGroupBy);
  }
}

/// Each output owns a separate file, including across independent Dart engines.
class DesktopLayoutStore extends ChangeNotifier {
  DesktopLayoutStore({File? file, this.persist = true})
    : file = file ?? defaultFile;

  final File? file;
  final bool persist;
  final _outputs = <String, Map<String, DesktopCell>>{};
  final _sorting = <String, DesktopSortKey?>{};
  final _stacks = <String, bool>{};
  final _stackGroupBy = <String, DesktopSortKey>{};
  static final _fileWork = <String, Future<void>>{};
  Future<void>? _loadWork;
  Future<void> _saveWork = Future.value();
  bool _disposed = false;
  String? error;

  static File? get defaultFile => shellConfigurationFile('desktop-layout.json');

  File? fileForOutput(String outputName) {
    final base = file;
    if (base == null) return null;
    // Fixed-length FNV-1a identifiers avoid filesystem limits and keep output
    // names containing slashes out of paths. The payload also stores the name.
    var hash = 0xcbf29ce484222325;
    for (final byte in utf8.encode(outputName)) {
      hash = ((hash ^ byte) * 0x100000001b3).toUnsigned(64);
    }
    final identifier = hash.toRadixString(16).replaceAll('-', 'n');
    return File('${base.parent.path}/desktop-layouts/output-$identifier.json');
  }

  static Future<_LayoutDocument> _read(File? file) async {
    final document = _LayoutDocument();
    if (file == null) return document;
    try {
      if (!await file.exists()) return document;
      if (await file.length() > 1024 * 1024) return document;
      final data = jsonDecode(await file.readAsString());
      if (data is! Map || data['version'] != 1 || data['outputs'] is! Map) {
        return document;
      }
      final sorting = data['sorting'];
      final modes =
          sorting is Map && sorting['version'] == 1 && sorting['outputs'] is Map
          ? sorting['outputs'] as Map
          : const {};
      final stacks = data['stacks'];
      final stackModes =
          stacks is Map && stacks['version'] == 1 && stacks['outputs'] is Map
          ? stacks['outputs'] as Map
          : const {};
      final stackGroupBys = stacks is Map && stacks['groupBy'] is Map
          ? stacks['groupBy'] as Map
          : const {};
      final maps = data['outputs'] as Map;
      for (final entry in maps.entries.take(64)) {
        if (entry.key is! String || entry.value is! Map) continue;
        final name = entry.key as String;
        if (name.isEmpty || name.length > 256) continue;
        final cells = <String, DesktopCell>{};
        for (final position in (entry.value as Map).entries.take(4096)) {
          final path = position.key;
          final cell = DesktopCell.fromJson(position.value);
          if (path is! String ||
              !path.startsWith('/') ||
              path.contains('\x00')) {
            continue;
          }
          if (path.length <= 4096 && cell != null) cells[path] = cell;
        }
        document.outputs[name] = cells;
        // Missing or unknown sort metadata is manual mode. Keep reading the
        // version-1 cells even when a newer sorting extension is unsupported.
        document.sorting[name] = DesktopSort.fromName(modes[name]);
        document.stacks[name] = stackModes[name] == true;
        document.stackGroupBy[name] =
            DesktopSort.fromName(stackGroupBys[name]) ?? DesktopSortKey.kind;
      }
    } on FormatException {
      return document;
    } on FileSystemException {
      return document;
    }
    return document;
  }

  Future<void> load() => _loadWork ??= () async {
    try {
      final stored = await _read(file);
      if (file != null) {
        final directory = Directory('${file!.parent.path}/desktop-layouts');
        if (await directory.exists()) {
          final files = await directory
              .list(followLinks: false)
              .where((entry) => entry is File && entry.path.endsWith('.json'))
              .take(64)
              .toList();
          for (final path in files) {
            stored.addAll(await _read(path as File));
          }
        }
      }
      if (_disposed) return;
      _outputs.addAll(stored.outputs);
      _sorting.addAll(stored.sorting);
      _stacks.addAll(stored.stacks);
      _stackGroupBy.addAll(stored.stackGroupBy);
    } on FileSystemException catch (failure) {
      if (_disposed) return;
      error = '桌面布局读取失败：$failure';
    }
    if (_disposed) return;
    notifyListeners();
  }();

  Map<String, DesktopCell> positions(String outputName) =>
      Map.unmodifiable(_outputs[outputName] ?? const <String, DesktopCell>{});

  DesktopSortKey? sortKey(String outputName) => _sorting[outputName];

  bool stacksEnabled(String outputName) => _stacks[outputName] ?? false;
  DesktopSortKey stackGroupBy(String outputName) =>
      _stackGroupBy[outputName] ?? DesktopSortKey.kind;

  Future<void> setStacksEnabled(
    String outputName,
    bool enabled, {
    DesktopSortKey? groupBy,
    Map<String, DesktopCell>? positions,
  }) async {
    if (!_validOutput(outputName)) return;
    await load();
    if (_disposed) return;
    _stacks[outputName] = enabled;
    if (groupBy != null) _stackGroupBy[outputName] = groupBy;
    await _saveState(
      outputName,
      positions ?? Map.of(this.positions(outputName)),
      _sorting[outputName],
    );
  }

  Future<void> setStackGroupBy(
    String outputName,
    DesktopSortKey groupBy, {
    Map<String, DesktopCell>? positions,
  }) async {
    if (!_validOutput(outputName)) return;
    await load();
    if (_disposed) return;
    _stackGroupBy[outputName] = groupBy;
    await _saveState(
      outputName,
      positions ?? Map.of(this.positions(outputName)),
      _sorting[outputName],
    );
  }

  /// Explicitly enter/leave automatic sorting. Pass the currently resolved
  /// positions when choosing manual mode to freeze what the user sees.
  Future<void> setSortKey(
    String outputName,
    DesktopSortKey? key, {
    Map<String, DesktopCell>? positions,
  }) async {
    if (!_validOutput(outputName)) return;
    final copy = positions == null
        ? null
        : Map<String, DesktopCell>.of(positions);
    await load();
    if (_disposed) return;
    await _saveState(
      outputName,
      copy ?? Map.of(this.positions(outputName)),
      key,
    );
  }

  /// For a deliberate transition to free placement, not an ordinary drag in
  /// automatic mode. Position and manual-mode state are saved atomically.
  Future<void> saveManualPositions(
    String outputName,
    Map<String, DesktopCell> positions,
  ) => setSortKey(outputName, null, positions: positions);

  bool _validOutput(String name) =>
      !_disposed && name.isNotEmpty && name.length <= 256;

  /// Routine placement updates (e.g. renaming) retain the output's sort mode.
  Future<void> savePositions(
    String outputName,
    Map<String, DesktopCell> positions,
  ) async {
    if (!_validOutput(outputName)) return;
    final copy = Map<String, DesktopCell>.of(positions);
    await load();
    if (_disposed) return;
    await _saveState(outputName, copy, _sorting[outputName]);
  }

  Future<void> _saveState(
    String outputName,
    Map<String, DesktopCell> copy,
    DesktopSortKey? sortKey,
  ) async {
    _outputs[outputName] = copy;
    _sorting[outputName] = sortKey;
    error = null;
    notifyListeners();
    if (!persist || file == null) return;
    _saveWork = _saveWork.then((_) async {
      // A shared base JSON would race across Flutter engines in one PID, where
      // Dart statics are independent and POSIX locks can belong to the PID.
      // Separate output files remove that cross-output read/merge/write race.
      final name = fileForOutput(outputName)!.absolute.path;
      final previous = _fileWork[name] ?? Future<void>.value();
      final work = previous.then((_) => _saveOutput(outputName, copy, sortKey));
      _fileWork[name] = work;
      try {
        await work;
      } finally {
        if (identical(_fileWork[name], work)) _fileWork.remove(name);
      }
    });
    await _saveWork;
  }

  Future<void> _saveOutput(
    String output,
    Map<String, DesktopCell> cells,
    DesktopSortKey? sortKey,
  ) async {
    final path = fileForOutput(output)!;
    RandomAccessFile? lock;
    var locked = false;
    Directory? staging;
    try {
      await path.parent.create(recursive: true);
      lock = await File('${path.path}.lock').open(mode: FileMode.append);
      await lock.lock(FileLock.blockingExclusive);
      locked = true;
      final merged = await _read(path);
      merged.outputs[output] = cells;
      merged.sorting[output] = sortKey;
      merged.stacks[output] = _stacks[output] ?? false;
      merged.stackGroupBy[output] =
          _stackGroupBy[output] ?? DesktopSortKey.kind;
      staging = await path.parent.createTemp('.desktop-layout-');
      final temporary = File('${staging.path}/layout.json');
      await temporary.writeAsString(
        '${jsonEncode({
          'version': 1,
          'outputs': {
            for (final output in merged.outputs.entries) output.key: {for (final cell in output.value.entries) cell.key: cell.value.toJson()},
          },
          'sorting': {
            'version': 1,
            'outputs': {for (final output in merged.outputs.keys) output: merged.sorting[output]?.name},
          },
          'stacks': {
            'version': 1,
            'outputs': {for (final output in merged.outputs.keys) output: merged.stacks[output] ?? false},
            'groupBy': {for (final output in merged.outputs.keys) output: (merged.stackGroupBy[output] ?? DesktopSortKey.kind).name},
          },
        })}\n',
        flush: true,
      );
      await temporary.rename(path.path);
    } on FileSystemException catch (failure) {
      error = '桌面布局保存失败：$failure';
      if (!_disposed) notifyListeners();
    } finally {
      try {
        if (staging != null && await staging.exists()) {
          await staging.delete(recursive: true);
        }
      } on FileSystemException catch (failure) {
        if (!_disposed) {
          error = '桌面布局临时文件清理失败：$failure';
          notifyListeners();
        }
      } finally {
        if (lock != null) {
          try {
            try {
              if (locked) await lock.unlock();
            } finally {
              await lock.close();
            }
          } on FileSystemException catch (failure) {
            if (!_disposed) {
              error = '桌面布局锁释放失败：$failure';
              notifyListeners();
            }
          }
        }
      }
    }
  }

  @override
  void dispose() {
    _disposed = true;
    super.dispose();
  }
}
