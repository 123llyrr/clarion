// SPDX-License-Identifier: GPL-3.0-or-later

/// One direct child of the user's desktop directory. Reading an entry never
/// executes its content, including Desktop Entry launchers.
class DesktopEntry {
  const DesktopEntry({
    required this.fullPath,
    required this.name,
    String? displayName,
    this.isDirectory = false,
    this.isSymlink = false,
    this.isDesktopShortcut = false,
    this.trustedShortcut = false,
    this.iconName,
    this.iconPath,
    this.targetPath,
    this.sizeBytes = 0,
    this.modified,
  }) : displayName = displayName ?? name;

  final String fullPath;
  final String name;
  final String displayName;
  final bool isDirectory;
  final bool isSymlink;
  final bool isDesktopShortcut;
  final bool trustedShortcut;
  final String? iconName;
  final String? iconPath;
  final String? targetPath;
  final int sizeBytes;
  final DateTime? modified;

  String get id => fullPath;
}
