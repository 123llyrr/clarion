// SPDX-License-Identifier: GPL-3.0-or-later

import 'package:flutter/foundation.dart';

enum RecentContentKind { text, image, screenshot }

/// Display metadata only. Full clipboard payloads stay owned by the service.
@immutable
class RecentContentEntry {
  const RecentContentEntry({
    required this.id,
    required this.kind,
    required this.capturedAt,
    this.text = '',
    this.thumbnail,
    this.isPinned = false,
    this.source,
    this.filePath,
  });

  final String id;
  final RecentContentKind kind;
  final DateTime capturedAt;
  final String text;
  final Uint8List? thumbnail;
  final bool isPinned;
  final String? source;
  final String? filePath;

  RecentContentEntry copyWith({bool? isPinned, Uint8List? thumbnail}) =>
      RecentContentEntry(
        id: id,
        kind: kind,
        capturedAt: capturedAt,
        text: text,
        thumbnail: thumbnail ?? this.thumbnail,
        isPinned: isPinned ?? this.isPinned,
        source: source,
        filePath: filePath,
      );
}
