// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:typed_data';

enum ShellPanel {
  none,
  launcher,
  quickSettings,
  volume,
  notifications,
  wallpapers,
  trayMenu,
  trayItems,
  screenshot,
  systemMenu,
  recentContent,
  dockAppMenu,
  dockDownloads,
  dockContextMenu,
  dockTrashMenu,
  dockFinderMenu,
  dockDownloadsMenu,
  spotlight,
  spaces,
}

enum ScreenshotMode { region, screen, window }

enum BarPosition { top, bottom }

enum ShellStyle { material, macos }

/// A compositor window shared by every shell surface.
class ShellWindow {
  const ShellWindow({
    required this.id,
    required this.title,
    required this.appId,
    this.workspaceId,
    this.isFocused = false,
    this.pid,
    this.isMinimized = false,
  });
  final int id;
  final String title;
  final String appId;
  final int? workspaceId;
  final bool isFocused;
  final int? pid;
  final bool isMinimized;
}

/// One application, even when it is both pinned and has several windows.
class DockApp {
  DockApp({
    required this.app,
    Iterable<ShellWindow> windows = const [],
    this.isPinned = false,
  }) : windows = List.unmodifiable(windows);

  final DesktopApp app;
  final List<ShellWindow> windows;
  final bool isPinned;
  String get id => app.id;
  String get name => app.name;
  String? get iconName => app.iconName;
  String? get iconPath => app.iconPath;
  bool get isRunning => windows.isNotEmpty;
  bool get isFocused => windows.any((window) => window.isFocused);
  bool get canPin => app.desktopFile.isNotEmpty;
}

class ShellWorkspace {
  const ShellWorkspace({
    required this.id,
    required this.index,
    this.name,
    this.output,
    this.isFocused = false,
    this.isActive = false,
    this.windowCount = 0,
  });
  final int id;
  final int index;
  final String? name;
  final String? output;
  final bool isFocused;
  final bool isActive;
  final int windowCount;
  String get label => name ?? '$index';
}

class DesktopApp {
  const DesktopApp({
    required this.id,
    required this.name,
    required this.desktopFile,
    this.description = '',
    this.iconName,
    this.iconPath,
    this.keywords = const [],
  });
  final String id;
  final String name;
  final String desktopFile;
  final String description;
  final String? iconName;
  final String? iconPath;
  final List<String> keywords;
  // Search scans run once per keystroke across every app; compiling a RegExp
  // per app per keystroke dominates that cost. Instances are immutable, so
  // the lowercased haystack can be memoized without keeping them alive.
  static final RegExp _whitespace = RegExp(r'\s+');
  static final Expando<String> _searchTexts = Expando<String>();
  static (String, List<String>)? _preparedQuery;

  String get _searchText {
    var text = _searchTexts[this];
    if (text == null) {
      text = '$name $description ${keywords.join(' ')}'.toLowerCase();
      _searchTexts[this] = text;
    }
    return text;
  }

  bool matches(String query) {
    final lowered = query.toLowerCase();
    if (_preparedQuery?.$1 != lowered) {
      _preparedQuery = (lowered, lowered.split(_whitespace));
    }
    return _preparedQuery!.$2.every(_searchText.contains);
  }
}

class NotificationAction {
  const NotificationAction(this.key, this.label);
  final String key;
  final String label;
}

class ShellNotification {
  const ShellNotification({
    required this.id,
    required this.appName,
    required this.summary,
    required this.body,
    required this.timestamp,
    this.actions = const [],
    this.urgency = 1,
    this.resident = false,
    this.transient = false,
    this.active = true,
  });
  final int id;
  final String appName;
  final String summary;
  final String body;
  final DateTime timestamp;
  final List<NotificationAction> actions;
  final int urgency;
  final bool resident;
  final bool transient;

  /// Expired history is readable but its D-Bus ID and actions are no longer live.
  final bool active;
}

class ShellWallpaper {
  const ShellWallpaper({
    required this.path,
    required this.name,
    required this.sizeBytes,
    this.modified,
  });
  final String path;
  final String name;
  final int sizeBytes;
  final DateTime? modified;
}

enum TrayStatus { passive, active, needsAttention }

enum TrayAction { activate, secondaryActivate, contextMenu }

enum TrayMenuToggleType { none, checkmark, radio }

class TrayIconPixmap {
  const TrayIconPixmap({
    required this.width,
    required this.height,
    required this.rgba,
  });
  final int width;
  final int height;
  final Uint8List rgba;
}

class TrayMenuEntry {
  const TrayMenuEntry({
    required this.id,
    required this.label,
    this.enabled = true,
    this.visible = true,
    this.separator = false,
    this.toggleType = TrayMenuToggleType.none,
    this.toggleState = 0,
    this.destructive = false,
    this.children = const [],
  });
  final int id;
  final String label;
  final bool enabled;
  final bool visible;
  final bool separator;
  final TrayMenuToggleType toggleType;
  final int toggleState;
  final bool destructive;
  final List<TrayMenuEntry> children;
}

class TrayItem {
  const TrayItem({
    required this.id,
    required this.title,
    this.status = TrayStatus.active,
    this.iconName = '',
    this.iconThemePath = '',
    this.iconPixmap,
    this.iconRevision = 0,
    this.menuAvailable = false,
    this.primaryOpensMenu = false,
    this.menuPath = '',
  });
  final String id;
  final String title;
  final TrayStatus status;
  final String iconName;
  final String iconThemePath;
  final TrayIconPixmap? iconPixmap;

  /// Exporters may overwrite an icon file without changing its path.
  final int iconRevision;
  final bool menuAvailable;
  final bool primaryOpensMenu;
  final String menuPath;
}

class WifiNetwork {
  const WifiNetwork({
    required this.ssid,
    required this.signal,
    required this.security,
    this.inUse = false,
  });

  final String ssid;
  final int signal;
  final String security;
  final bool inUse;

  bool get isSecured => security.isNotEmpty && security != '--';

  int get signalBars {
    if (signal >= 75) return 3;
    if (signal >= 50) return 2;
    if (signal >= 25) return 1;
    return 0;
  }

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is WifiNetwork &&
          ssid == other.ssid &&
          signal == other.signal &&
          security == other.security &&
          inUse == other.inUse;

  @override
  int get hashCode => Object.hash(ssid, signal, security, inUse);
}

class BluetoothDevice {
  const BluetoothDevice({
    required this.mac,
    required this.name,
    this.connected = false,
    this.paired = true,
    this.icon = 'bluetooth',
  });

  final String mac;
  final String name;
  final bool connected;
  final bool paired;
  final String icon;

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is BluetoothDevice &&
          mac == other.mac &&
          name == other.name &&
          connected == other.connected &&
          paired == other.paired &&
          icon == other.icon;

  @override
  int get hashCode => Object.hash(mac, name, connected, paired, icon);
}
