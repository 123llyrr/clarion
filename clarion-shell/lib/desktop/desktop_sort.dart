// SPDX-License-Identifier: GPL-3.0-or-later
import 'desktop_entry.dart';

/// Only metadata actually read by DesktopService is offered. Creation and
/// desktop-addition dates are intentionally absent; POSIX ctime is neither.
enum DesktopSortKey { name, kind, modified, size }

abstract final class DesktopSort {
  static DesktopSortKey? fromName(Object? name) {
    for (final key in DesktopSortKey.values) {
      if (key.name == name) return key;
    }
    return null;
  }

  static List<DesktopEntry> sorted(
    Iterable<DesktopEntry> entries,
    DesktopSortKey key,
  ) => entries.toList()..sort((a, b) => compare(a, b, key));

  /// Name uses case-insensitive numeric ordering, then the literal label/path
  /// for deterministic ties. This is not a claim of Finder's locale collation.
  /// Dates and file sizes are descending; missing values follow known values.
  static int compare(DesktopEntry a, DesktopEntry b, DesktopSortKey key) {
    final primary = switch (key) {
      DesktopSortKey.name => 0,
      DesktopSortKey.kind => _kind(a).compareTo(_kind(b)),
      DesktopSortKey.modified => _newestFirst(a.modified, b.modified),
      // A directory's stat size is not its contents' size. Do not present it
      // as a recursively measured folder size or synchronously walk its files.
      DesktopSortKey.size => _largestFirst(
        a.isDirectory || a.sizeBytes < 0 ? null : a.sizeBytes,
        b.isDirectory || b.sizeBytes < 0 ? null : b.sizeBytes,
      ),
    };
    if (primary != 0) return primary;
    final name = compareNames(a.displayName, b.displayName);
    return name != 0 ? name : a.fullPath.compareTo(b.fullPath);
  }

  static int _newestFirst(DateTime? a, DateTime? b) => a == null
      ? (b == null ? 0 : 1)
      : b == null
      ? -1
      : b.compareTo(a);

  static int _largestFirst(int? a, int? b) => a == null
      ? (b == null ? 0 : 1)
      : b == null
      ? -1
      : b.compareTo(a);

  /// Linux currently supplies directory/launcher flags and the filename, not
  /// macOS UTIs. Extension groups therefore remain an explicit approximation.
  static String _kind(DesktopEntry entry) {
    if (entry.isDirectory) return '0';
    if (entry.isDesktopShortcut) return '1';
    final dot = entry.name.lastIndexOf('.');
    final extension = dot > 0 && dot < entry.name.length - 1
        ? entry.name.substring(dot + 1).toLowerCase()
        : '';
    return '2:$extension';
  }

  static int compareNames(String a, String b) {
    final left = a.toLowerCase();
    final right = b.toLowerCase();
    var i = 0;
    var j = 0;
    while (i < left.length && j < right.length) {
      final x = left.codeUnitAt(i);
      final y = right.codeUnitAt(j);
      if (_digit(x) && _digit(y)) {
        final iStart = i;
        final jStart = j;
        while (i < left.length && _digit(left.codeUnitAt(i))) {
          i++;
        }
        while (j < right.length && _digit(right.codeUnitAt(j))) {
          j++;
        }
        var iNumber = iStart;
        var jNumber = jStart;
        while (iNumber < i - 1 && left.codeUnitAt(iNumber) == 48) {
          iNumber++;
        }
        while (jNumber < j - 1 && right.codeUnitAt(jNumber) == 48) {
          jNumber++;
        }
        final length = (i - iNumber).compareTo(j - jNumber);
        if (length != 0) return length;
        final digits = left
            .substring(iNumber, i)
            .compareTo(right.substring(jNumber, j));
        if (digits != 0) return digits;
      } else {
        if (x != y) return x.compareTo(y);
        i++;
        j++;
      }
    }
    final remaining = (left.length - i).compareTo(right.length - j);
    return remaining != 0 ? remaining : a.compareTo(b);
  }

  static bool _digit(int code) => code >= 48 && code <= 57;
}
