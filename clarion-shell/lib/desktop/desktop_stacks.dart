// SPDX-License-Identifier: GPL-3.0-or-later
import 'desktop_entry.dart';
import 'desktop_sort.dart';

class DesktopStackGroup {
  const DesktopStackGroup({
    required this.id,
    required this.title,
    required this.entries,
    required this.category,
  });

  final String id;
  final String title;
  final List<DesktopEntry> entries;
  final String category;

  DesktopEntry? get primaryEntry => entries.isNotEmpty ? entries.first : null;
  int get count => entries.length;
}

abstract final class DesktopStacks {
  static List<DesktopStackGroup> groupEntries(
    List<DesktopEntry> entries, {
    required DesktopSortKey groupBy,
  }) {
    // Only group non-directory regular files. In macOS, folders remain unstacked on desktop.
    final files = entries.where((e) => !e.isDirectory).toList();
    final groups = <String, List<DesktopEntry>>{};

    for (final entry in files) {
      final category = _categorize(entry, groupBy);
      groups.putIfAbsent(category, () => []).add(entry);
    }

    final result = <DesktopStackGroup>[];
    for (final entry in groups.entries) {
      final list = List<DesktopEntry>.of(entry.value)
        ..sort((a, b) => DesktopSort.compare(a, b, DesktopSortKey.modified));
      result.add(
        DesktopStackGroup(
          id: '__stack__${groupBy.name}__${entry.key}',
          title: '${entry.key} (${list.length})',
          entries: list,
          category: entry.key,
        ),
      );
    }

    result.sort((a, b) => DesktopSort.compareNames(a.category, b.category));
    return result;
  }

  static String _categorize(DesktopEntry entry, DesktopSortKey groupBy) {
    switch (groupBy) {
      case DesktopSortKey.kind:
        return _categorizeKind(entry);
      case DesktopSortKey.modified:
        return _categorizeModified(entry.modified);
      case DesktopSortKey.size:
        return _categorizeSize(entry.sizeBytes);
      case DesktopSortKey.name:
        return _categorizeKind(entry);
    }
  }

  static String _categorizeKind(DesktopEntry entry) {
    if (entry.isDesktopShortcut) return '快捷方式';
    final dot = entry.name.lastIndexOf('.');
    if (dot <= 0 || dot >= entry.name.length - 1) return '其他';
    final ext = entry.name.substring(dot + 1).toLowerCase();

    const imageExts = {
      'png',
      'jpg',
      'jpeg',
      'gif',
      'webp',
      'bmp',
      'svg',
      'tiff',
      'tif',
      'heic',
      'ico',
    };
    if (imageExts.contains(ext)) return '图像';

    if (ext == 'pdf') return 'PDF 凭证与文稿';

    const docExts = {
      'txt',
      'md',
      'markdown',
      'doc',
      'docx',
      'odt',
      'rtf',
      'pages',
      'xls',
      'xlsx',
      'csv',
      'ppt',
      'pptx',
      'key',
      'json',
      'yaml',
      'yml',
      'xml',
      'log',
      'html',
      'htm',
    };
    if (docExts.contains(ext)) return '文稿';

    const movieExts = {'mp4', 'mov', 'mkv', 'avi', 'webm', 'flv', 'wmv', 'm4v'};
    if (movieExts.contains(ext)) return '影片';

    const audioExts = {
      'mp3',
      'wav',
      'm4a',
      'flac',
      'aac',
      'ogg',
      'wma',
      'opus',
    };
    if (audioExts.contains(ext)) return '音乐与音频';

    const archiveExts = {'zip', 'tar', 'gz', 'xz', '7z', 'rar', 'bz2', 'tgz'};
    if (archiveExts.contains(ext)) return '归档与压缩包';

    const codeExts = {
      'dart',
      'rs',
      'py',
      'c',
      'cpp',
      'h',
      'hpp',
      'sh',
      'bash',
      'js',
      'ts',
      'go',
      'java',
      'kt',
      'swift',
      'sql',
    };
    if (codeExts.contains(ext)) return '代码与脚本';

    return '其他';
  }

  static String _categorizeModified(DateTime? modified) {
    if (modified == null) return '更早';
    final now = DateTime.now();
    final difference = now.difference(modified);
    if (difference.inDays == 0 && now.day == modified.day) return '今天';
    if (difference.inDays <= 1) return '昨天';
    if (difference.inDays <= 7) return '过去 7 天';
    if (difference.inDays <= 30) return '过去 30 天';
    return '更早';
  }

  static String _categorizeSize(int sizeBytes) {
    if (sizeBytes < 0) return '较小 (< 1 MB)';
    if (sizeBytes >= 100 * 1024 * 1024) return '巨大 (> 100 MB)';
    if (sizeBytes >= 10 * 1024 * 1024) return '较大 (10 MB – 100 MB)';
    if (sizeBytes >= 1024 * 1024) return '中等 (1 MB – 10 MB)';
    return '较小 (< 1 MB)';
  }
}
