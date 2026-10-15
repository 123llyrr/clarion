// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:typed_data';

import 'desktop_entry.dart';

enum DesktopFileKind {
  file,
  directory,
  symbolicLink,
  fifo,
  socket,
  blockDevice,
  characterDevice,
  unknown;

  String get label => switch (this) {
    file => '普通文件',
    directory => '文件夹',
    symbolicLink => '符号链接',
    fifo => '命名管道',
    socket => '套接字',
    blockDevice => '块设备',
    characterDevice => '字符设备',
    unknown => '其他文件系统项目',
  };
}

class DesktopFileDetails {
  const DesktopFileDetails({
    required this.entry,
    required this.name,
    required this.path,
    required this.kind,
    required this.sizeBytes,
    required this.modified,
    required this.mode,
    this.linkTarget,
  });

  final DesktopEntry entry;
  final String name;
  final String path;
  final DesktopFileKind kind;
  final int sizeBytes;
  final DateTime modified;
  final int mode;

  /// Literal link contents, including relative or dangling targets.
  final String? linkTarget;

  String get permissions {
    final result = <String>[];
    for (var group = 2; group >= 0; group--) {
      final bits = mode >> (group * 3);
      result.add(bits & 4 != 0 ? 'r' : '-');
      result.add(bits & 2 != 0 ? 'w' : '-');
      final execute = bits & 1 != 0;
      final special = mode & (1 << (group + 9)) != 0;
      result.add(
        special
            ? group == 0
                  ? (execute ? 't' : 'T')
                  : (execute ? 's' : 'S')
            : execute
            ? 'x'
            : '-',
      );
    }
    return '${result.join()} (${(mode & 0xfff).toRadixString(8).padLeft(4, '0')})';
  }
}

enum DesktopPreviewKind { text, image, unsupported }

class DesktopFilePreview {
  const DesktopFilePreview({
    required this.details,
    required this.kind,
    this.text,
    this.imageBytes,
    this.truncated = false,
    this.message,
  });
  final DesktopFileDetails details;
  final DesktopPreviewKind kind;
  final String? text;
  final Uint8List? imageBytes;
  final bool truncated;
  final String? message;
}

class DesktopInspectionCancelled implements Exception {
  const DesktopInspectionCancelled();
}

/// Cancels pending preview processes; readers also check it after each await.
class DesktopInspectionCancellation {
  bool _cancelled = false;
  final Set<void Function()> _listeners = {};
  bool get isCancelled => _cancelled;

  void cancel() {
    if (_cancelled) return;
    _cancelled = true;
    for (final listener in List.of(_listeners)) {
      listener();
    }
    _listeners.clear();
  }

  void check() {
    if (_cancelled) throw const DesktopInspectionCancelled();
  }

  void addListener(void Function() listener) {
    if (_cancelled) {
      listener();
    } else {
      _listeners.add(listener);
    }
  }

  void removeListener(void Function() listener) => _listeners.remove(listener);
}
