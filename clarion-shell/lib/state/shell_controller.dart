// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math' as math;
import 'package:flutter/foundation.dart' show mapEquals, listEquals;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import '../models.dart';
import '../mac_notification_view.dart';
import '../mac_shell_preferences.dart';
import '../app_menu_models.dart';
import '../services/global_app_menu_service.dart';
import '../services/desktop_apps.dart';
import '../services/login_items_service.dart';
import '../services/media_service.dart';
import '../services/audio_output_service.dart';
import '../services/dock_folder_service.dart';
import '../services/notification_service.dart';
import '../services/power_profiles_service.dart';
import '../services/clarion_ipc.dart';
import '../services/system_service.dart';
import '../services/tray_service.dart';
import '../services/sound_service.dart';
import '../services/trash_state_service.dart';
import '../services/wallpaper_service.dart';
import '../services/window_decoration_service.dart';
import '../services/window_preview_service.dart';

class ShellController extends ChangeNotifier {
  ShellController({
    this.demo = false,
    bool notifications = true,
    this.monitor,
    ClarionIpc? ipc,
    SystemService? system,
    DesktopApps? desktopApps,
    LoginItemsService? loginItemsService,
    AudioOutputService? audioOutputService,
    MediaService? mediaService,
    NotificationService? notificationService,
    WallpaperService? wallpaperService,
    TrayService? trayService,
    GlobalAppMenuService? appMenuService,
    PowerProfilesService? powerProfiles,
    DockFolderService? dockFolderService,
    TrashStateService? trashStateService,
    SoundService? soundService,
    WindowDecorationService? windowDecorationService,
    this.powerProfilesEnabled = true,
    this.mediaEnabled = true,
    bool tray = true,
    this.restoreWallpaperOnStart = true,
  }) : _ipc = ipc ?? ClarionIpc(),
       appMenu = appMenuService ?? GlobalAppMenuService(),
       _system = system ?? SystemService(),
       _desktopApps = desktopApps ?? DesktopApps(),
       loginItems = loginItemsService ?? LoginItemsService(demo: demo),
       audioOutputs = audioOutputService ?? AudioOutputService(demo: demo),
       media =
           mediaService ??
           MediaService(
             enabled:
                 !demo &&
                 Platform.isLinux &&
                 !Platform.environment.containsKey('FLUTTER_TEST') &&
                 Platform.environment.containsKey('DBUS_SESSION_BUS_ADDRESS'),
           ),
       dockFolder = dockFolderService ?? DockFolderService(),
       trashState = trashStateService ?? TrashStateService(),
       _soundService =
           soundService ??
           SoundService(
             enabled:
                 !demo && !Platform.environment.containsKey('FLUTTER_TEST'),
           ),
       _windowDecorationService =
           windowDecorationService ??
           WindowDecorationService(
             enabled:
                 !demo && !Platform.environment.containsKey('FLUTTER_TEST'),
           ),
       _wallpaperService = wallpaperService ?? WallpaperService(),
       powerProfiles = powerProfiles ?? PowerProfilesService(demo: demo),
       _notificationService = notifications
           ? notificationService ?? NotificationService()
           : null,
       _trayService = tray ? trayService ?? TrayService() : trayService {
    appMenu.addListener(_appMenuChanged);
    audioOutputs.addListener(_audioOutputsChanged);
    media.addListener(_changed);
    dockFolder.addListener(_changed);
    trashState.addListener(_changed);
    if (demo) _loadDemo();
  }
  final bool demo;
  final int? monitor;
  final bool restoreWallpaperOnStart;
  final bool powerProfilesEnabled;
  final bool mediaEnabled;
  final MediaService media;
  final PowerProfilesService powerProfiles;
  final GlobalAppMenuService appMenu;
  final DockFolderService dockFolder;
  final LoginItemsService loginItems;
  final AudioOutputService audioOutputs;
  int _audioSnapshotRevision = 0;
  int _audioReadGeneration = 0;
  final TrashStateService trashState;
  final SoundService _soundService;
  final WindowDecorationService _windowDecorationService;

  SoundService get soundService => _soundService;
  WindowDecorationService get windowDecorationService =>
      _windowDecorationService;

  Future<void> initializeDockServices() async {
    if (_disposed || demo) return;
    await Future.wait([dockFolder.start(), trashState.start()]);
  }

  Object? _appMenuTargetKey;
  bool? _appMenuActive;
  int? _menuWindowId;
  bool _appMenuOpen = false;
  int _appMenuDismissRevision = 0;
  bool get appMenuOpen => _appMenuOpen;
  int get appMenuDismissRevision => _appMenuDismissRevision;
  bool get canMinimize =>
      demo || _windowState.values.any((w) => w.containsKey('is_minimized'));
  Future<void> Function()? prepareScreenshot;
  final WallpaperService _wallpaperService;
  final ClarionIpc _ipc;
  final SystemService _system;
  final DesktopApps _desktopApps;
  final NotificationService? _notificationService;
  final TrayService? _trayService;
  static const _window = MethodChannel('clarion.shell/window');
  final _workspaceState = <int, Map<String, dynamic>>{};
  final _windowState = <int, Map<String, dynamic>>{};
  final _busy = <String>{};
  final _sliders = <String, _SliderQueue>{};
  final _sliderValues = <String, double>{};
  final _sliderRevisions = <String, int>{};
  final _systemVersions = <String, int>{};
  final _sliderGestures = <String>{};
  final _subscriptions = <StreamSubscription<dynamic>>[];
  Timer? _clock;
  Timer? _poll;
  Timer? _sliderRefresh;
  bool _disposed = false;
  bool _started = false;
  bool _polling = false;
  bool _refreshAgain = false;
  bool _appsLoading = false;
  Future<void>? _outputRefreshWork;
  bool _outputRefreshDirty = false;
  int _outputRefreshGeneration = 0;
  bool _connected = false;
  int _desktopRevision = 0;
  int _dockGeometryRevision = 0;
  DateTime _now = DateTime.now();
  String? _outputName;
  List<DesktopApp> _apps = const [];
  final _appAliases = <String, String?>{};
  Map<String, Map<String, String>> _appMetadata = const {};
  List<String> _pinnedAppIds = const [];
  List<String> _recentAppIds = const [];
  // Desktop model reads occur on hover frames and unrelated service updates.
  // Reuse immutable derived snapshots until one of their actual inputs changes.
  int _windowsSnapshotRevision = -1;
  List<ShellWindow> _windowsSnapshot = const [];
  (int, String?)? _workspacesSnapshotKey;
  List<ShellWorkspace> _workspacesSnapshot = const [];
  (int, List<DesktopApp>, List<String>)? _dockAppsSnapshotKey;
  List<DockApp> _dockAppsSnapshot = const [];
  (List<DockApp>, List<DesktopApp>, List<String>)? _recentDockAppsSnapshotKey;
  List<DockApp> _recentDockAppsSnapshot = const [];
  (int, String)? _observedRecentFocus;
  (int, String)? _unresolvedRecentFocus;
  Rect? _dockMenuAnchor;
  List<String>? _pinnedAppIdsPreference;
  bool _pinDefaultsPending = true;
  String? _selectedDockAppId;
  List<ShellNotification> _notifications = const [];
  ShellPanel _panel = ShellPanel.none;
  MacNotificationView _macNotificationView = MacNotificationView.today;
  int _panelRevision = 0;
  ({Object owner, String message, Offset anchor})? _barTooltip;
  SystemSnapshot _status = const SystemSnapshot();
  bool _doNotDisturb = false;
  String? _error;
  int _errorRevision = 0;
  ThemeMode _themeMode = ThemeMode.dark;
  bool _reduceMotion = false;
  BarPosition _barPosition = BarPosition.top;
  ShellStyle _shellStyle = ShellStyle.material;
  MacShellPreferences _macPreferences = const MacShellPreferences();
  bool _macMenuBarVisible = true;
  bool _notificationServerAvailable = false;
  List<ShellWallpaper> _wallpapers = const [];
  bool _wallpapersLoading = false;
  ServerSocket? _commandServer;
  String? _wallpaperError;
  String? _demoWallpaperPath;
  List<TrayItem> _trayItems = const [];
  TrayItem? _selectedTrayItem;
  List<TrayMenuEntry> _trayMenu = const [];
  bool _trayAvailable = false;
  bool _trayMenuLoading = false;
  String? _trayError;
  int _trayMenuRequest = 0;
  int _screenshotRequest = 0;
  List<WifiNetwork> _wifiNetworks = const [];
  bool _wifiScanning = false;
  List<BluetoothDevice> _bluetoothDevices = const [];
  bool _bluetoothScanning = false;

  bool get connected => _connected;
  int get desktopRevision => _desktopRevision;
  int get dockGeometryRevision => _dockGeometryRevision;
  int get panelRevision => _panelRevision;
  DateTime get now => _now;
  List<DesktopApp> get apps => _apps;
  ShellStyle get shellStyle => _shellStyle;
  MacShellPreferences get macPreferences => _macPreferences;
  bool get macMenuBarVisible =>
      !_macPreferences.menuBarAutoHide || _macMenuBarVisible;
  List<String> get pinnedAppIds => _pinnedAppIds;
  List<String>? get pinnedAppIdsPreference => _pinnedAppIdsPreference;
  List<String> get recentAppIds => _recentAppIds;
  Rect? get dockMenuAnchor => _dockMenuAnchor;
  String? get selectedDockAppId => _selectedDockAppId;

  List<ShellWindow> get windows {
    if (_windowsSnapshotRevision == _desktopRevision) return _windowsSnapshot;
    _windowsSnapshot = List.unmodifiable(
      [
        for (final window in _windowState.values)
          ShellWindow(
            id: window['id'] as int,
            title: window['title'] as String? ?? '',
            appId: window['app_id'] as String? ?? '',
            workspaceId: window['workspace_id'] as int?,
            isFocused: window['is_focused'] == true,
            pid: window['pid'] as int?,
            isMinimized: window['is_minimized'] == true,
            appMenu:
                _canonicalMenuExport(window['app_menu']) ??
                _menuExport(window['gtk_app_menu']),
          ),
      ]..sort((a, b) => a.id.compareTo(b.id)),
    );
    _windowsSnapshotRevision = _desktopRevision;
    return _windowsSnapshot;
  }

  static AppMenuExport? _canonicalMenuExport(Object? raw) {
    if (raw is! Map) return null;
    final bus = raw['bus_name'];
    final path = raw['menu_path'];
    if (bus is! String || bus.isEmpty || path is! String || path.isEmpty) {
      return null;
    }
    return AppMenuExport(
      busName: bus,
      menuPath: path,
      protocol: AppMenuProtocol.dbusmenu,
    );
  }

  static AppMenuExport? _menuExport(Object? raw) {
    if (raw is! Map) return null;
    final bus = raw['unique_bus_name'];
    final menubar = raw['menubar_path'];
    final path = menubar is String && menubar.isNotEmpty
        ? menubar
        : raw['app_menu_path'];
    if (bus is! String || bus.isEmpty || path is! String || path.isEmpty) {
      return null;
    }
    return AppMenuExport(
      busName: bus,
      menuPath: path,
      applicationActionsPath: raw['application_object_path'] is String
          ? raw['application_object_path'] as String
          : null,
      windowActionsPath: raw['window_object_path'] is String
          ? raw['window_object_path'] as String
          : null,
    );
  }

  ShellWindow? get menuFocusedWindow {
    final focused = focusedWindow;
    if (focused != null) {
      _menuWindowId = focused.id;
      return focused;
    }
    if (_appMenuOpen || _panel != ShellPanel.none) {
      final remembered = windows
          .where((w) => w.id == _menuWindowId)
          .firstOrNull;
      if (remembered != null) return remembered;
    }
    return null;
  }

  void _appMenuChanged() {
    if (!_disposed) notifyListeners();
  }

  /// Some Qt clients keep their native menu hidden after the host disappears.
  /// Material offers the already exported menu without advertising a new host.
  bool get materialAppMenuAvailable =>
      _shellStyle == ShellStyle.material &&
      menuFocusedWindow?.appMenu?.protocol == AppMenuProtocol.dbusmenu &&
      appMenu.available;

  void setAppMenuOpen(bool open) {
    if (_disposed ||
        _appMenuOpen == open ||
        (open &&
            _shellStyle != ShellStyle.macos &&
            !materialAppMenuAvailable)) {
      return;
    }
    _appMenuOpen = open;
    if (open) {
      _barTooltip = null;
      if (_panel != ShellPanel.none) _setPanel(ShellPanel.none);
    }
    _changed();
  }

  /// Exporters such as Qt cache registrar availability when creating menus.
  /// Bootstrap the host before showing the shell or launching a new client.
  Future<void> prepareAppMenus() async {
    if (_disposed || demo || _shellStyle != ShellStyle.macos) return;
    try {
      await appMenu.setActive(true);
      if (_disposed || _shellStyle != ShellStyle.macos) return;
      await appMenu.start();
    } on Object {
      // A missing bus must not prevent the desktop or applications from opening.
    }
  }

  void _syncAppMenu() {
    if (_disposed || demo || !_started) return;
    final active = _shellStyle == ShellStyle.macos;
    if (_appMenuActive != active) {
      _appMenuActive = active;
      _appMenuTargetKey = null;
      unawaited(appMenu.setActive(active));
    }
    final candidate = menuFocusedWindow;
    final window =
        active || candidate?.appMenu?.protocol == AppMenuProtocol.dbusmenu
        ? candidate
        : null;
    final export = window?.appMenu;
    final key = (
      window?.id,
      window?.pid,
      window?.appId,
      export?.busName,
      export?.menuPath,
      export?.protocol,
      export?.applicationActionsPath,
      export?.windowActionsPath,
    );
    if (key == _appMenuTargetKey) return;
    _appMenuTargetKey = key;
    _menuWindowId = window?.id;
    unawaited(
      appMenu.focusedTarget(
        pid: window?.pid,
        appId: window?.appId ?? '',
        windowId: window?.id,
        export: export,
      ),
    );
  }

  ShellWindow? get focusedWindow {
    for (final window in windows) {
      if (window.isFocused) return window;
    }
    return null;
  }

  List<DockApp> get dockApps {
    final key = (_desktopRevision, _apps, _pinnedAppIds);
    if (_dockAppsSnapshotKey == key) return _dockAppsSnapshot;
    final installed = {for (final app in _apps) app.id: app};
    final grouped = <String, List<ShellWindow>>{};
    final unknown = <String, DesktopApp>{};
    for (final window in windows) {
      final knownId = _resolveAppId(window.appId);
      final id =
          knownId ??
          (window.appId.trim().isEmpty
              ? 'window:${window.id}'
              : 'running:${_appKey(window.appId)}');
      (grouped[id] ??= []).add(window);
      if (knownId == null) {
        unknown.putIfAbsent(
          id,
          () => DesktopApp(
            id: id,
            name: window.appId.trim().isNotEmpty
                ? window.appId.trim()
                : window.title.isNotEmpty
                ? window.title
                : '窗口 ${window.id}',
            desktopFile: '',
          ),
        );
      }
    }
    final order = <String>{
      for (final id in _pinnedAppIds)
        if (installed.containsKey(id)) id,
      ...grouped.keys,
    };
    _dockAppsSnapshot = List.unmodifiable([
      for (final id in order)
        DockApp(
          app: installed[id] ?? unknown[id]!,
          windows: grouped[id] ?? const [],
          isPinned: _pinnedAppIds.contains(id),
        ),
    ]);
    _dockAppsSnapshotKey = key;
    return _dockAppsSnapshot;
  }

  DockApp? get selectedDockApp {
    for (final app in dockApps) {
      if (app.id == _selectedDockAppId) return app;
    }
    for (final app in _apps) {
      if (app.id == _selectedDockAppId) {
        return DockApp(app: app, isPinned: _pinnedAppIds.contains(app.id));
      }
    }
    return null;
  }

  /// The three most recently used installed applications that are no longer
  /// running or pinned. Running applications remain in the application area.
  List<DockApp> get recentDockApps {
    if (!_macPreferences.showRecentApplications) return const [];
    final dock = dockApps;
    final key = (dock, _apps, _recentAppIds);
    if (_recentDockAppsSnapshotKey == key) return _recentDockAppsSnapshot;
    final occupied = dock.map((app) => app.id).toSet();
    final installed = {for (final app in _apps) app.id: app};
    _recentDockAppsSnapshot = List.unmodifiable(
      [
        for (final id in _recentAppIds)
          if (!occupied.contains(id) && installed.containsKey(id))
            DockApp(app: installed[id]!),
      ].take(3),
    );
    _recentDockAppsSnapshotKey = key;
    return _recentDockAppsSnapshot;
  }

  void setRecentAppIds(Iterable<String> ids) {
    if (_disposed) return;
    final next = List<String>.unmodifiable(
      {
        for (final id in ids)
          if (id.trim().isNotEmpty) id.trim(),
      }.take(24),
    );
    if (listEquals(next, _recentAppIds)) return;
    _recentAppIds = next;
    _changed();
  }

  bool _rememberRecentApp(String appId) {
    final id = _resolveAppId(appId) ?? appId;
    if (!_apps.any((app) => app.id == id)) return false;
    if (_recentAppIds.firstOrNull == id) return true;
    _recentAppIds = List.unmodifiable(
      [id, ..._recentAppIds.where((value) => value != id)].take(24),
    );
    return true;
  }

  void _trackRecentFocus() {
    final window = focusedWindow;
    final identity = window == null ? null : (window.id, window.appId);
    if (identity == _observedRecentFocus) return;
    _observedRecentFocus = identity;
    _unresolvedRecentFocus = null;
    if (identity != null && !_rememberRecentApp(identity.$2)) {
      _unresolvedRecentFocus = identity;
    }
  }

  void setDockMenuAnchor(Rect? anchor) {
    if (_disposed) return;
    _dockMenuAnchor =
        anchor != null &&
            anchor.isFinite &&
            anchor.width > 0 &&
            anchor.height > 0
        ? anchor
        : null;
  }

  void openDownloadsStack() {
    if (_disposed) return;
    _finishSliderGestures();
    _screenshotRequest++;
    _barTooltip = null;
    _clearTrayMenu();
    _setPanel(ShellPanel.dockDownloads);
    if (!demo) unawaited(dockFolder.start());
    _changed();
  }

  List<ShellNotification> get notifications => _notifications;
  ShellPanel get panel => _panel;
  ({Object owner, String message, Offset anchor})? get barTooltip =>
      _barTooltip;

  void showBarTooltip(Object owner, String message, Offset anchor) {
    if (_disposed ||
        _panel != ShellPanel.none ||
        _appMenuOpen ||
        message.isEmpty ||
        !anchor.dx.isFinite ||
        !anchor.dy.isFinite) {
      return;
    }
    _barTooltip = (owner: owner, message: message, anchor: anchor);
    _changed();
  }

  void hideBarTooltip(Object owner) {
    if (_disposed || !identical(_barTooltip?.owner, owner)) return;
    _barTooltip = null;
    _changed();
  }

  void dismissBarTooltip() {
    if (_disposed || _barTooltip == null) return;
    _barTooltip = null;
    _changed();
  }

  double? get volume => _sliderValues['volume'] ?? _status.volume;
  double? get brightness => _sliderValues['brightness'] ?? _status.brightness;
  double? get batteryFraction => _status.batteryFraction;
  bool get muted => _status.muted;
  bool get charging => _status.charging;
  bool get doNotDisturb => _doNotDisturb;
  MacNotificationView get macNotificationView => _macNotificationView;

  void setMacNotificationView(MacNotificationView view) {
    if (_disposed || _macNotificationView == view) return;
    _macNotificationView = view;
    _changed();
  }

  bool? get wifiEnabled => _status.wifiEnabled;
  bool? get bluetoothEnabled => _status.bluetoothEnabled;
  String get networkName => _status.networkName;
  bool get isWirelessNetwork => _status.isWireless;
  String get wifiSsid =>
      (_status.isWireless && (wifiEnabled ?? false)) ? _status.networkName : '';
  List<WifiNetwork> get wifiNetworks => _wifiNetworks;
  bool get wifiScanning => _wifiScanning;
  List<BluetoothDevice> get bluetoothDevices => _bluetoothDevices;
  bool get bluetoothScanning => _bluetoothScanning;
  String? get error => _error;
  Set<String> get busy => Set.unmodifiable(_busy);
  ThemeMode get themeMode => _themeMode;
  bool get reduceMotion => _reduceMotion;
  BarPosition get barPosition => _barPosition;
  bool get notificationServerAvailable => _notificationServerAvailable;
  List<ShellWallpaper> get wallpapers => _wallpapers;
  String get wallpaperDirectory => _wallpaperService.directoryPath;
  String? get selectedWallpaperPath =>
      demo ? _demoWallpaperPath : _wallpaperService.selectedPath;
  bool get wallpapersLoading => _wallpapersLoading;
  String? get wallpaperError => _wallpaperError ?? _wallpaperService.error;
  bool get wallpaperAvailable => demo || _wallpaperService.available;
  List<TrayItem> get trayItems => _trayItems;
  TrayItem? get selectedTrayItem => _selectedTrayItem;
  List<TrayMenuEntry> get trayMenu => _trayMenu;
  bool get trayAvailable => demo || _trayAvailable;
  bool get trayMenuLoading => _trayMenuLoading;
  String? get trayError => _trayError;
  String get windowTitle {
    for (final window in _windowState.values) {
      if (window['is_focused'] == true) {
        return window['title'] as String? ??
            window['app_id'] as String? ??
            'Clarion';
      }
    }
    return 'Clarion';
  }

  List<ShellWorkspace> get workspaces {
    final key = (_desktopRevision, _outputName);
    if (_workspacesSnapshotKey == key) return _workspacesSnapshot;
    var output = _outputName;
    if (output == null ||
        !_workspaceState.values.any((ws) => ws['output'] == output)) {
      output = null;
      for (final ws in _workspaceState.values) {
        if (ws['is_focused'] == true) {
          output = ws['output'] as String?;
          break;
        }
      }
      output ??= _workspaceState.values.isEmpty
          ? null
          : _workspaceState.values.first['output'] as String?;
    }
    final result = <ShellWorkspace>[];
    for (final ws in _workspaceState.values) {
      if (output != null && ws['output'] != output) continue;
      final id = ws['id'] as int;
      result.add(
        ShellWorkspace(
          id: id,
          index: ws['idx'] as int? ?? 1,
          name: ws['name'] as String?,
          output: ws['output'] as String?,
          isFocused: ws['is_focused'] == true,
          isActive: ws['is_active'] == true,
          windowCount: _windowState.values
              .where((w) => w['workspace_id'] == id)
              .length,
        ),
      );
    }
    result.sort((a, b) => a.index.compareTo(b.index));
    _workspacesSnapshot = List.unmodifiable(result);
    _workspacesSnapshotKey = key;
    return _workspacesSnapshot;
  }

  Future<void> start() async {
    if (_started || _disposed) return;
    _started = true;
    if (demo) return;
    if (_shellStyle == ShellStyle.macos) {
      unawaited(_windowDecorationService.applyForStyle(_shellStyle));
    }
    await _refreshOutputName();

    if (_disposed) return;
    _subscriptions.add(
      _ipc.connections.listen((connected) {
        if (_disposed) return;
        final changed =
            _connected != connected ||
            (!connected &&
                (_workspaceState.isNotEmpty || _windowState.isNotEmpty));
        _connected = connected;
        // Disconnect invalidates the snapshot. The next stream supplies all state.
        if (!connected) {
          _observedRecentFocus = null;
          _unresolvedRecentFocus = null;
          _dockGeometryRevision++;
          _outputRefreshGeneration++;
          _outputRefreshDirty = false;
          _outputName = null;
          _workspaceState.clear();
          _windowState.clear();
          _dockConnectionRevision++;
          _dockTargetRevision++;
          _dockTargets.clear();
          _dockDesiredTargets.clear();
          _dockDirtyTargets.clear();
          _dockTargetsOutput = null;
          _dockTargetsReduced = null;
          _pendingDockTargets = null;
          _barTooltip = null;
        }
        if (changed) _desktopRevision++;
        if (connected && changed) unawaited(_refreshOutputName());
        _changed();
      }),
    );
    _subscriptions.add(_ipc.events.listen(_applyEvent));
    _ipc.start();
    if (powerProfilesEnabled) unawaited(powerProfiles.start());
    if (mediaEnabled) unawaited(media.start());
    _subscriptions.add(_wallpaperService.changes.listen((_) => _changed()));
    // Wallpaper decoding is independent of shell startup and catalogue loading.
    // Preview only reads the saved selection; it never restores a provider.
    unawaited(_restoreWallpaper());
    _clock = Timer.periodic(const Duration(seconds: 1), (_) {
      final next = DateTime.now();
      final minuteChanged = next.minute != _now.minute;
      final showSeconds = _macPreferences.clockShowSeconds;
      _now = next;
      if (minuteChanged || showSeconds) {
        _changed();
      }
    });
    _poll = Timer.periodic(const Duration(seconds: 10), (_) {
      unawaited(_refreshSystem());
    });
    final service = _notificationService;
    if (service != null) {
      _subscriptions.add(
        service.changes.listen((_) {
          if (_disposed) return;
          final snapshot = service.notifications;
          if (identical(_notifications, snapshot)) return;
          _notifications = snapshot;
          _changed();
        }),
      );
    }
    final tray = _trayService;
    if (tray != null) {
      _subscriptions.add(
        tray.changes.listen((_) {
          if (_disposed) return;
          _trayAvailable = tray.available;
          _trayItems = tray.items;
          final selected = _selectedTrayItem;
          if (selected != null) {
            TrayItem? current;
            for (final item in _trayItems) {
              if (item.id == selected.id) current = item;
            }
            if (current == null) {
              _clearTrayMenu();
              if (_panel == ShellPanel.trayMenu) _setPanel(ShellPanel.none);
            } else {
              _selectedTrayItem = current;
              if (current.menuPath != selected.menuPath &&
                  _panel == ShellPanel.trayMenu) {
                unawaited(openTrayMenu(current));
              }
            }
          }
          _changed();
        }),
      );
      _subscriptions.add(
        tray.menuChanges.listen((id) {
          final item = _selectedTrayItem;
          if (!_disposed && _panel == ShellPanel.trayMenu && item?.id == id) {
            unawaited(openTrayMenu(item!));
          }
        }),
      );
    }
    await Future.wait([
      _loadApps(),
      _refreshSystem(),
      if (service != null)
        service.start().then((available) {
          if (_disposed) return;
          _notificationServerAvailable = available;
          _notifications = service.notifications;
          _changed();
        }),
      if (tray != null)
        tray.start().then((available) {
          if (_disposed) return;
          _trayAvailable = available;
          _trayItems = tray.items;
          _changed();
        }),
    ]);
    if (!demo) {
      unawaited(_startCommandServer());
    }
  }

  Future<void> _loadApps() async {
    if (_disposed || _appsLoading) return;
    _appsLoading = true;
    final errorRevision = _errorRevision;
    try {
      final apps = await _desktopApps.load();
      final metadata = await _readAppMetadata(apps);
      if (!_disposed) {
        _apps = List.unmodifiable(apps);
        _indexApps(metadata);
        _resolvePinnedApps(metadata);
        final pending = _unresolvedRecentFocus;
        if (pending != null &&
            pending == _observedRecentFocus &&
            _rememberRecentApp(pending.$2)) {
          _unresolvedRecentFocus = null;
        }
      }
    } on Object catch (e) {
      if (!_disposed && errorRevision == _errorRevision) {
        _setError('应用列表加载失败：$e');
      }
    } finally {
      _appsLoading = false;
    }
    _changed();
  }

  Future<Map<String, Map<String, String>>> _readAppMetadata(
    List<DesktopApp> apps,
  ) async {
    final result = <String, Map<String, String>>{};
    await Future.wait([
      for (final app in apps)
        () async {
          if (app.desktopFile.isEmpty) return;
          try {
            final file = File(app.desktopFile);
            if (await file.length() > 131072) return;
            result[app.id] = parseDesktopEntry(await file.readAsString());
          } on FileSystemException {
            // A removed entry can still have live windows in the compositor.
          } on FormatException {
            // The catalogue remains usable when optional metadata is damaged.
          }
        }(),
    ]);
    return result;
  }

  void _indexApps(Map<String, Map<String, String>> metadata) {
    _appMetadata = metadata;
    _appAliases.clear();
    for (final app in _apps) {
      final entry = metadata[app.id] ?? const <String, String>{};
      final aliases = <String>{
        app.id,
        app.name,
        if (app.iconName != null) app.iconName!,
        if (entry['StartupWMClass'] != null) entry['StartupWMClass']!,
        ..._commandAliases(entry['Exec'] ?? ''),
      };
      for (final alias in aliases) {
        final key = _appKey(alias);
        if (key.isEmpty) continue;
        if (_appAliases.containsKey(key) && _appAliases[key] != app.id) {
          // Shared executables/classes must not merge unrelated applications.
          _appAliases[key] = null;
        } else {
          _appAliases[key] = app.id;
        }
      }
    }
  }

  static String _appKey(String value) {
    final key = value
        .trim()
        .replaceAll('\\', '/')
        .split('/')
        .last
        .toLowerCase();
    return key.endsWith('.desktop') ? key.substring(0, key.length - 8) : key;
  }

  String? _resolveAppId(String value) {
    final key = _appKey(value);
    if (key.isEmpty) return null;
    // Exact desktop IDs win over descriptive names and executable aliases.
    for (final app in _apps) {
      if (_appKey(app.id) == key) return app.id;
    }
    return _appAliases[key];
  }

  static Iterable<String> _commandAliases(String command) sync* {
    final words = _desktopCommandWords(command);
    if (words.isEmpty) return;
    var index = 0;
    if (_appKey(words.first) == 'env') {
      index++;
      while (index < words.length &&
          (words[index] == '--' ||
              RegExp(r'^[A-Za-z_][A-Za-z0-9_]*=').hasMatch(words[index]))) {
        index++;
      }
    }
    if (index >= words.length) return;
    final program = _appKey(words[index]);
    if (program == 'wine' || program == 'wine64') {
      for (final word in words.skip(index + 1)) {
        if (word.toLowerCase().endsWith('.exe')) {
          yield word;
          yield _appKey(word).replaceFirst(RegExp(r'\.exe$'), '');
        }
      }
    } else if (program == 'flatpak' || program == 'snap') {
      final run = words.indexOf('run', index + 1);
      if (run >= 0) {
        for (final word in words.skip(run + 1)) {
          if (!word.startsWith('-')) {
            yield word;
            break;
          }
        }
      }
    } else if (!const {
      'env',
      'sh',
      'bash',
      'python',
      'python3',
      'java',
      'electron',
    }.contains(program)) {
      yield words[index];
    }
  }

  static List<String> _desktopCommandWords(String command) {
    final words = <String>[];
    var token = StringBuffer();
    var quoted = false;
    var started = false;
    for (var index = 0; index < command.length; index++) {
      final character = command[index];
      if (character == '\u0000' || character == '\n' || character == '\r') {
        return const [];
      }
      if (character == '\\') {
        if (++index >= command.length) return const [];
        token.write(command[index]);
        started = true;
      } else if (character == '"') {
        quoted = !quoted;
        started = true;
      } else if (!quoted && (character == ' ' || character == '\t')) {
        if (started) words.add(token.toString());
        token = StringBuffer();
        started = false;
      } else {
        token.write(character);
        started = true;
      }
    }
    if (quoted) return const [];
    if (started) words.add(token.toString());
    return words;
  }

  void _resolvePinnedApps(Map<String, Map<String, String>> metadata) {
    if (!_pinDefaultsPending) {
      _pinnedAppIds = List.unmodifiable({
        for (final id in _pinnedAppIdsPreference ?? const <String>[])
          _resolveAppId(id) ?? id,
      });
      return;
    }
    final defaults = <String>{};
    const roles = {
      'WebBrowser': ['web-browser', 'browser', 'firefox', 'chromium', 'chrome'],
      'FileManager': [
        'system-file-manager',
        'files',
        'nautilus',
        'dolphin',
        'thunar',
      ],
      'TerminalEmulator': [
        'utilities-terminal',
        'terminal',
        'kitty',
        'foot',
        'alacritty',
      ],
      'Settings': ['clarion-settings', 'preferences-system', 'settings'],
    };
    for (final role in roles.entries) {
      for (final app in _apps) {
        final categories = (metadata[app.id]?['Categories'] ?? '').split(';');
        final keys = {_appKey(app.id), _appKey(app.iconName ?? '')};
        if (categories.contains(role.key) || role.value.any(keys.contains)) {
          defaults.add(app.id);
          break;
        }
      }
    }
    if (defaults.isEmpty) defaults.addAll(_apps.take(4).map((app) => app.id));
    _pinnedAppIds = List.unmodifiable(defaults);
  }

  bool _systemProtected(String key) =>
      _sliderGestures.contains(key) ||
      (_sliders[key]?.hasWork ?? false) ||
      _busy.contains(key) ||
      (key == 'volume' &&
          (_busy.contains('mute') || _busy.contains('audio-output')));

  Future<void> _refreshSystem({bool reconcile = false}) async {
    if (_disposed || demo) return;
    // A full snapshot spawns several processes and cannot replace a value the
    // user is adjusting. The idle reconciliation will fetch it after release.
    if (_sliderGestures.isNotEmpty ||
        _sliders.values.any((slider) => slider.hasWork)) {
      return;
    }
    if (_polling) {
      if (reconcile) _refreshAgain = true;
      return;
    }
    _polling = true;
    final errorRevision = _errorRevision;
    final versions = Map<String, int>.of(_systemVersions);
    final protectedAtStart = {
      for (final key in ['volume', 'brightness', 'wifi', 'bluetooth'])
        if (_systemProtected(key)) key,
    };
    try {
      final snapshot = await _system.read();
      if (_disposed) return;
      bool retain(String key) =>
          protectedAtStart.contains(key) ||
          _systemProtected(key) ||
          (versions[key] ?? 0) != (_systemVersions[key] ?? 0);
      final retainVolume = retain('volume');
      final retainBrightness = retain('brightness');
      final retainWifi = retain('wifi');
      final retainBluetooth = retain('bluetooth');
      final nextStatus = SystemSnapshot(
        volume: retainVolume ? _status.volume : snapshot.volume,
        brightness: retainBrightness ? _status.brightness : snapshot.brightness,
        muted: retainVolume ? _status.muted : snapshot.muted,
        batteryFraction: snapshot.batteryFraction,
        charging: snapshot.charging,
        wifiEnabled: retainWifi ? _status.wifiEnabled : snapshot.wifiEnabled,
        bluetoothEnabled: retainBluetooth
            ? _status.bluetoothEnabled
            : snapshot.bluetoothEnabled,
        networkName: retainWifi ? _status.networkName : snapshot.networkName,
        isWireless: retainWifi ? _status.isWireless : snapshot.isWireless,
      );
      final statusChanged = _status != nextStatus;
      _status = nextStatus;
      if (!retainVolume) _sliderValues.remove('volume');
      if (!retainBrightness) _sliderValues.remove('brightness');
      if (statusChanged) _changed();
    } on Object catch (error) {
      if (!_disposed && errorRevision == _errorRevision) {
        _setError('系统状态读取失败：$error');
      }
    } finally {
      _polling = false;
      if (_refreshAgain && !_disposed) {
        _refreshAgain = false;
        unawaited(_refreshSystem());
      }
    }
  }

  void _applyEvent(Map<String, dynamic> event) {
    if (_disposed) return;
    var desktopChanged = false;
    final changedWorkspaces = event['WorkspacesChanged'];
    if (changedWorkspaces is Map && changedWorkspaces['workspaces'] is List) {
      final workspaces = <int, Map<String, dynamic>>{};
      for (final ws in changedWorkspaces['workspaces'] as List) {
        if (ws is Map<String, dynamic> &&
            ws['id'] is int &&
            ws['idx'] is int &&
            (ws['name'] == null || ws['name'] is String) &&
            (ws['output'] == null || ws['output'] is String)) {
          workspaces[ws['id'] as int] = Map.of(ws);
        }
      }
      desktopChanged = _replaceDesktopState(_workspaceState, workspaces);
      unawaited(_refreshOutputName());
    }
    final changedWindows = event['WindowsChanged'];
    if (changedWindows is Map && changedWindows['windows'] is List) {
      final windows = <int, Map<String, dynamic>>{};
      for (final window in changedWindows['windows'] as List) {
        if (_validWindow(window)) {
          windows[window['id'] as int] = Map<String, dynamic>.from(
            window as Map,
          );
        }
      }
      desktopChanged =
          _replaceDesktopState(_windowState, windows) || desktopChanged;
    }
    final activated = event['WorkspaceActivated'];
    if (activated is Map && _workspaceState.containsKey(activated['id'])) {
      final output = _workspaceState[activated['id']]!['output'];
      for (final ws in _workspaceState.values) {
        if (ws['output'] == output) {
          desktopChanged =
              _setDesktopFlag(ws, 'is_active', ws['id'] == activated['id']) ||
              desktopChanged;
        }
        if (activated['focused'] == true) {
          desktopChanged =
              _setDesktopFlag(ws, 'is_focused', ws['id'] == activated['id']) ||
              desktopChanged;
        }
      }
    }
    final changedWindow = event['WindowOpenedOrChanged'];
    final window = changedWindow is Map ? changedWindow['window'] : null;
    if (_validWindow(window)) {
      final data = Map<String, dynamic>.from(window as Map);
      if (data['is_focused'] == true) {
        for (final w in _windowState.values) {
          if (w['id'] != data['id']) {
            desktopChanged =
                _setDesktopFlag(w, 'is_focused', false) || desktopChanged;
          }
        }
      }
      if (!mapEquals(_windowState[data['id']], data)) {
        _windowState[data['id'] as int] = data;
        desktopChanged = true;
      }
    }
    final closed = event['WindowClosed'];
    if (closed is Map) {
      desktopChanged =
          _windowState.remove(closed['id']) != null || desktopChanged;
    }
    final focus = event['WindowFocusChanged'];
    if (focus is Map) {
      for (final w in _windowState.values) {
        desktopChanged =
            _setDesktopFlag(w, 'is_focused', w['id'] == focus['id']) ||
            desktopChanged;
      }
    }
    final config = event['ConfigLoaded'];
    if (config is Map && config['failed'] == true) {
      _setError('Clarion 配置加载失败，请打开配置中心检查。');
    }
    if (desktopChanged) {
      _desktopRevision++;
      _trackRecentFocus();
      _changed();
    } else if (config is Map && config['failed'] == true) {
      _changed();
    }
  }

  bool _replaceDesktopState(
    Map<int, Map<String, dynamic>> current,
    Map<int, Map<String, dynamic>> replacement,
  ) {
    if (current.length == replacement.length &&
        current.entries.every(
          (entry) => mapEquals(entry.value, replacement[entry.key]),
        )) {
      return false;
    }
    current
      ..clear()
      ..addAll(replacement);
    return true;
  }

  bool _setDesktopFlag(Map<String, dynamic> state, String key, bool value) {
    if (state[key] == value) return false;
    state[key] = value;
    return true;
  }

  void outputGeometryChanged() {
    if (_disposed) return;
    // Until the new geometry has a uniquely identified output, fresh icon
    // coordinates must not be registered against the previous connector.
    _outputName = null;
    _dockGeometryRevision++;
    _dockMenuAnchor = null;
    _dockDirtyTargets.addAll(_dockTargets.keys);
    _dockDirtyTargets.addAll(_dockDesiredTargets.keys);
    unawaited(setDockMinimizeTargets(const {}));
    _barTooltip = null;
    _desktopRevision++;
    unawaited(_refreshOutputName());
    _changed();
  }

  Future<void> _refreshOutputName() {
    if (_disposed || demo) return Future.value();
    _outputRefreshGeneration++;
    _outputRefreshDirty = true;
    return _outputRefreshWork ??= _resolveOutputNames();
  }

  Future<void> _resolveOutputNames() async {
    try {
      while (_outputRefreshDirty && !_disposed) {
        _outputRefreshDirty = false;
        final generation = _outputRefreshGeneration;
        String? name;
        try {
          name = await _resolveOutputName();
        } on Object {
          // A failed read cannot establish which output owns these coordinates.
          name = null;
        }
        if (_disposed) return;
        if (generation != _outputRefreshGeneration) continue;
        if (name != _outputName) {
          _dockGeometryRevision++;
          _outputName = name;
          unawaited(setDockMinimizeTargets(const {}));
          _desktopRevision++;
          _barTooltip = null;
          _changed();
        }
      }
    } finally {
      _outputRefreshWork = null;
    }
  }

  Future<String?> _resolveOutputName() async {
    final explicit = await _window.invokeMethod<String>('outputName');
    if (explicit != null && explicit.trim().isNotEmpty) return explicit;
    final state = await _window.invokeMapMethod<String, dynamic>('windowState');
    final geometry = state?['monitorGeometry'];
    if (geometry is! Map) return null;
    const keys = ['x', 'y', 'width', 'height'];
    for (final key in keys) {
      final value = geometry[key];
      if (value is! num || !value.isFinite) return null;
    }
    if ((geometry['width'] as num) <= 0 || (geometry['height'] as num) <= 0) {
      return null;
    }
    final response = await _ipc.request('Outputs');
    final outputs = response is Map ? response['Outputs'] : null;
    if (outputs is! Map) return null;
    String? match;
    for (final entry in outputs.entries) {
      final output = entry.value;
      if (output is! Map) continue;
      final name = output['name'];
      final logical = output['logical'];
      if (name is! String || name.trim().isEmpty || logical is! Map) continue;
      if (!keys.every(
        (key) =>
            logical[key] is num &&
            (logical[key] as num).isFinite &&
            logical[key] == geometry[key],
      )) {
        continue;
      }
      // Mirrored/equal logical rectangles are ambiguous even if a workspace
      // happens to be focused on one of them. Never guess its connector.
      if (match != null) return null;
      match = name;
    }
    return match;
  }

  void selectPanel(ShellPanel value) {
    if (_disposed) return;
    if (_panel == ShellPanel.quickSettings || _panel == ShellPanel.volume) {
      _finishSliderGestures();
    }
    _screenshotRequest++;
    _barTooltip = null;
    if (value != ShellPanel.dockAppMenu) _selectedDockAppId = null;
    if (_panel == ShellPanel.trayMenu || value != ShellPanel.trayMenu) {
      _clearTrayMenu();
    }
    _setPanel(_panel == value ? ShellPanel.none : value);
    if (_panel != ShellPanel.dockAppMenu) _selectedDockAppId = null;
    if ((_panel == ShellPanel.launcher || _panel == ShellPanel.spotlight) &&
        !demo) {
      unawaited(_loadApps());
    }
    if (_panel == ShellPanel.spaces && !demo) {
      unawaited(captureWindowPreviews());
    }
    _setError(null);
    _changed();
  }

  void toggleSpotlight() => selectPanel(ShellPanel.spotlight);

  void closePanel() {
    final hadAppMenu = _appMenuOpen;
    _appMenuOpen = false;
    _appMenuDismissRevision++;
    _menuWindowId = null;
    _screenshotRequest++;
    if (_disposed ||
        (_panel == ShellPanel.none && _barTooltip == null && !hadAppMenu)) {
      return;
    }
    _finishSliderGestures();
    _barTooltip = null;
    _selectedDockAppId = null;
    _clearTrayMenu();
    _setPanel(ShellPanel.none);
    _changed();
  }

  void openWallpapers() {
    if (_disposed) return;
    _finishSliderGestures();
    _clearTrayMenu();
    _selectedDockAppId = null;
    _setPanel(ShellPanel.wallpapers);
    _changed();
    unawaited(refreshWallpapers());
  }

  Future<void> refreshWallpapers() async {
    if (_disposed || _wallpapersLoading || demo) return;
    _wallpapersLoading = true;
    _wallpaperError = null;
    _changed();
    try {
      final images = await _wallpaperService.scan();
      if (!_disposed) _wallpapers = images;
    } on Object catch (e) {
      _wallpaperError = '壁纸列表加载失败：$e';
    } finally {
      _wallpapersLoading = false;
      _changed();
    }
  }

  Future<void> _restoreWallpaper() async {
    _busy.add('wallpaper');
    _changed();
    try {
      await _wallpaperService.restore(applySaved: restoreWallpaperOnStart);
    } on Object catch (e) {
      _wallpaperError = '壁纸恢复失败：$e';
    } finally {
      _busy.remove('wallpaper');
      _changed();
    }
  }

  Future<void> applyWallpaper(ShellWallpaper entry) async {
    if (_disposed || _busy.contains('wallpaper')) return;
    if (demo) {
      _demoWallpaperPath = entry.path;
      _changed();
      return;
    }
    _busy.add('wallpaper');
    _wallpaperError = null;
    _changed();
    try {
      await _wallpaperService.apply(entry);
    } on Object catch (e) {
      _wallpaperError = '壁纸应用失败：$e';
    } finally {
      _busy.remove('wallpaper');
      _changed();
    }
  }

  void setDoNotDisturb(bool value) {
    if (_doNotDisturb == value) return;
    _doNotDisturb = value;
    _changed();
  }

  void setThemeMode(ThemeMode value) {
    if (_themeMode == value) return;
    _themeMode = value;
    _changed();
  }

  void setReduceMotion(bool value) {
    if (_reduceMotion == value) return;
    _reduceMotion = value;
    unawaited(setDockMinimizeTargets(Map.of(_dockDesiredTargets)));
    _changed();
  }

  void setBarPosition(BarPosition value) {
    if (_disposed || _barPosition == value) return;
    _barPosition = value;
    _barTooltip = null;
    _changed();
  }

  void updateMacPreferences(MacShellPreferences value) =>
      setMacPreferences(value);

  void setMacPreferences(MacShellPreferences value) {
    if (_disposed) return;
    final next = value.copyWith();
    final previous = _macPreferences;
    if (previous == next) return;
    _macPreferences = next;
    if (!next.menuBarAutoHide) _macMenuBarVisible = true;
    _barTooltip = null;
    if (previous.dockPosition != next.dockPosition ||
        previous.dockSize != next.dockSize ||
        previous.magnification != next.magnification ||
        previous.magnificationScale != next.magnificationScale ||
        previous.dockAutoHide != next.dockAutoHide ||
        previous.showLaunchpad != next.showLaunchpad ||
        previous.showMissionControl != next.showMissionControl ||
        previous.showDownloads != next.showDownloads ||
        previous.showRecentApplications != next.showRecentApplications ||
        previous.minimizeIntoApp != next.minimizeIntoApp) {
      _invalidateDockLayoutTargets();
    } else if (previous.minimizeEffect != next.minimizeEffect) {
      // The visible geometry has not moved. Update its presentation policy
      // without clearing the destination or interrupting a live transition.
      unawaited(setDockMinimizeTargets(Map.of(_dockDesiredTargets)));
    }
    // Leave the settings panel open for consecutive adjustments. The native
    // host and role views observe this same immutable preferences value.
    _changed();
  }

  void setMacMenuBarVisible(bool visible) {
    if (_disposed) return;
    final next = !_macPreferences.menuBarAutoHide || visible;
    if (_macMenuBarVisible == next) return;
    _macMenuBarVisible = next;
    _barTooltip = null;
    _changed();
  }

  void _invalidateDockLayoutTargets() {
    _dockDirtyTargets.addAll(_dockTargets.keys);
    _dockDirtyTargets.addAll(_dockDesiredTargets.keys);
    unawaited(setDockMinimizeTargets(const {}));
    _desktopRevision++;
  }

  void setShellStyle(ShellStyle value) {
    if (_disposed || _shellStyle == value) return;
    _finishSliderGestures(flushPending: true);
    _screenshotRequest++;
    _barTooltip = null;
    _clearTrayMenu();
    _selectedDockAppId = null;
    _setPanel(ShellPanel.none);
    _shellStyle = value;
    if (!demo) {
      unawaited(_windowDecorationService.applyForStyle(value));
    }
    unawaited(setDockMinimizeTargets(Map.of(_dockDesiredTargets)));
    _appMenuOpen = false;
    _appMenuDismissRevision++;
    _changed();
    if (_started && value == ShellStyle.macos) {
      unawaited(prepareAppMenus());
    }
  }

  void shellStyleFailed(String message) {
    if (_disposed || _shellStyle != ShellStyle.macos) return;
    setShellStyle(ShellStyle.material);
    selectPanel(ShellPanel.quickSettings);
    final detail = message.trim();
    _setError(
      'Clarion 界面暂时不可用，已恢复 Material。'
      '${detail.isEmpty ? '' : '\n$detail'}',
    );
    _changed();
  }

  void setPinnedAppIds(Iterable<String>? values) {
    if (_disposed) return;
    _pinDefaultsPending = values == null;
    _pinnedAppIds = List.unmodifiable({
      for (final value in values ?? const <String>[])
        if (value.trim().isNotEmpty) value.trim(),
    });
    _pinnedAppIdsPreference = values == null ? null : _pinnedAppIds;
    _resolvePinnedApps(_appMetadata);
    _changed();
  }

  void togglePinnedApp(String appId) {
    if (_disposed) return;
    final id = _resolveAppId(appId);
    if (id == null) return;
    // Unknown compositor windows cannot be relaunched after they disappear.
    if (!demo &&
        !_apps.any((app) => app.id == id && app.desktopFile.isNotEmpty)) {
      return;
    }
    _pinDefaultsPending = false;
    final next = _pinnedAppIds.toList();
    if (!next.remove(id)) next.add(id);
    _pinnedAppIds = List.unmodifiable(next);
    _pinnedAppIdsPreference = _pinnedAppIds;
    _changed();
  }

  void unpinApp(String appId) {
    if (_disposed) return;
    final id = _resolveAppId(appId);
    if (id == null) return;
    if (!_pinnedAppIds.contains(id)) return;
    _pinDefaultsPending = false;
    final next = _pinnedAppIds.toList()..remove(id);
    _pinnedAppIds = List.unmodifiable(next);
    _pinnedAppIdsPreference = _pinnedAppIds;
    _invalidateDockLayoutTargets();
    _changed();
  }

  void reorderPinnedApp(String appId, String beforeId) {
    if (_disposed) return;
    final id = _resolveAppId(appId);
    final before = _resolveAppId(beforeId);
    if (id == null || before == null || id == before) return;
    bool canMove(String candidate) =>
        _pinnedAppIds.contains(candidate) &&
        _apps.any(
          (app) => app.id == candidate && (demo || app.desktopFile.isNotEmpty),
        );
    if (!canMove(id) || !canMove(before)) return;
    final next = _pinnedAppIds.toList()..remove(id);
    next.insert(next.indexOf(before), id);
    if (_pinnedAppIds.indexOf(id) == next.indexOf(id)) return;
    _pinDefaultsPending = false;
    _pinnedAppIds = List.unmodifiable(next);
    _pinnedAppIdsPreference = _pinnedAppIds;
    _invalidateDockLayoutTargets();
    _changed();
  }

  void reorderPinnedAppToEnd(String appId) {
    if (_disposed) return;
    final id = _resolveAppId(appId);
    if (id == null) return;
    bool canMove(String candidate) =>
        _pinnedAppIds.contains(candidate) &&
        _apps.any(
          (app) => app.id == candidate && (demo || app.desktopFile.isNotEmpty),
        );
    if (!canMove(id)) return;
    final next = _pinnedAppIds.toList()..remove(id);
    next.add(id);
    if (listEquals(_pinnedAppIds, next)) return;
    _pinDefaultsPending = false;
    _pinnedAppIds = List.unmodifiable(next);
    _pinnedAppIdsPreference = _pinnedAppIds;
    _invalidateDockLayoutTargets();
    _changed();
  }

  void pinAppAt(String appId, {String? beforeId}) {
    if (_disposed) return;
    final id = _resolveAppId(appId);
    if (id == null) return;
    if (!demo &&
        !_apps.any((app) => app.id == id && app.desktopFile.isNotEmpty)) {
      return;
    }
    final before = beforeId == null ? null : _resolveAppId(beforeId);
    if (before != null && !_pinnedAppIds.contains(before)) return;
    final next = _pinnedAppIds.toList()..remove(id);
    if (before != null && next.contains(before)) {
      next.insert(next.indexOf(before), id);
    } else {
      next.add(id);
    }
    if (listEquals(_pinnedAppIds, next)) return;
    _pinDefaultsPending = false;
    _pinnedAppIds = List.unmodifiable(next);
    _pinnedAppIdsPreference = _pinnedAppIds;
    _invalidateDockLayoutTargets();
    _changed();
  }

  void openDockAppMenu(String appId) {
    if (_disposed) return;
    final id = _resolveAppId(appId) ?? appId;
    if (!_apps.any((app) => app.id == id) &&
        !dockApps.any((app) => app.id == id)) {
      return;
    }
    _finishSliderGestures();
    _screenshotRequest++;
    _barTooltip = null;
    _clearTrayMenu();
    _selectedDockAppId = id;
    _setPanel(ShellPanel.dockAppMenu);
    _changed();
  }

  void openDockContextMenu() {
    if (_disposed) return;
    _finishSliderGestures();
    _screenshotRequest++;
    _barTooltip = null;
    _clearTrayMenu();
    _selectedDockAppId = null;
    _setPanel(ShellPanel.dockContextMenu);
    _changed();
  }

  void openDockTrashMenu() {
    if (_disposed) return;
    _finishSliderGestures();
    _screenshotRequest++;
    _barTooltip = null;
    _clearTrayMenu();
    _selectedDockAppId = null;
    _setPanel(ShellPanel.dockTrashMenu);
    _changed();
  }

  void openDockFinderMenu() {
    if (_disposed) return;
    _finishSliderGestures();
    _screenshotRequest++;
    _barTooltip = null;
    _clearTrayMenu();
    _selectedDockAppId = null;
    _setPanel(ShellPanel.dockFinderMenu);
    _changed();
  }

  void openDockDownloadsMenu() {
    if (_disposed) return;
    _finishSliderGestures();
    _screenshotRequest++;
    _barTooltip = null;
    _clearTrayMenu();
    _selectedDockAppId = null;
    _setPanel(ShellPanel.dockDownloadsMenu);
    _changed();
  }

  void closePanelAtRevision(int revision) {
    if (!_disposed && revision == _panelRevision) closePanel();
  }

  Future<void> _perform(String key, Future<void> Function() operation) async {
    if (_disposed || _busy.contains(key)) return;
    if (demo) return;
    _busy.add(key);
    final panelRevision = _panelRevision;
    _setError(null);
    final errorRevision = _errorRevision;
    _changed();
    try {
      await operation();
    } on Object catch (e) {
      if (!_disposed &&
          panelRevision == _panelRevision &&
          errorRevision == _errorRevision) {
        _setError('操作失败：$e');
      }
    } finally {
      _busy.remove(key);
      _changed();
    }
  }

  Future<void> _compositorAction(
    String key,
    String action, [
    Map<String, Object?> args = const {},
  ]) => _perform(key, () async {
    if (!_connected) throw StateError('合成器连接暂时不可用');
    await _ipc.action(action, args);
  });
  Future<void> switchWorkspace(int id) async {
    if (_disposed) return;
    if (demo) {
      if (!_workspaceState.containsKey(id)) return;
      var changed = false;
      for (final ws in _workspaceState.values) {
        changed = _setDesktopFlag(ws, 'is_active', ws['id'] == id) || changed;
        changed = _setDesktopFlag(ws, 'is_focused', ws['id'] == id) || changed;
      }
      if (changed) _desktopRevision++;
      _changed();
      return;
    }
    await _compositorAction('workspace', 'FocusWorkspace', {
      'reference': {'Id': id},
    });
  }

  WindowPreviewService? _windowPreviewService;
  int _windowPreviewRevision = 0;
  int get windowPreviewRevision => _windowPreviewRevision;
  final Map<int, String> _windowPreviews = {};
  Map<int, String> get windowPreviews => _windowPreviews;

  void toggleSpaces() {
    if (_panel == ShellPanel.spaces) {
      closePanel();
    } else {
      selectPanel(ShellPanel.spaces);
    }
  }

  Future<void> captureWindowPreviews([List<int>? targetWindowIds]) async {
    if (demo || _disposed || Platform.environment.containsKey('FLUTTER_TEST')) {
      return;
    }
    final service = _windowPreviewService ??= WindowPreviewService(_ipc);
    final revision = _panelRevision;
    final liveIds = windows.map((window) => window.id).toSet();
    for (final id in _windowPreviews.keys.toList()) {
      if (!liveIds.contains(id)) {
        unawaited(WindowPreviewService.remove(_windowPreviews.remove(id)!));
      }
    }
    final ids =
        targetWindowIds ??
        windows.where((w) => !w.isMinimized).map((w) => w.id).toList();
    for (final id in ids.toSet()) {
      if (_disposed || revision != _panelRevision) break;
      final path = await service.capture(id);
      if (path == null) continue;
      if (_disposed || revision != _panelRevision) {
        unawaited(WindowPreviewService.remove(path));
        break;
      }
      final previous = _windowPreviews[id];
      if (previous == path) continue;
      _windowPreviews[id] = path;
      _windowPreviewRevision++;
      _changed();
      if (previous != null) unawaited(WindowPreviewService.remove(previous));
    }
  }

  Future<void> createWorkspace() async {
    if (_disposed) return;
    if (demo) {
      final nextId =
          (_workspaceState.keys.isEmpty
              ? 0
              : _workspaceState.keys.reduce(math.max)) +
          1;
      _workspaceState[nextId] = {
        'id': nextId,
        'idx': nextId,
        'name': '桌面 $nextId',
        'output': _outputName ?? 'Preview',
        'is_active': false,
        'is_focused': false,
      };
      _desktopRevision++;
      _changed();
      return;
    }
    await _compositorAction('workspace', 'FocusWorkspaceDown');
  }

  Future<void> removeWorkspace(int id) async {
    if (_disposed) return;
    if (demo) {
      if (_workspaceState.length > 1) {
        _workspaceState.remove(id);
        final fallback = _workspaceState.keys.firstOrNull;
        for (final w in _windowState.values) {
          if (w['workspace_id'] == id) {
            w['workspace_id'] = fallback;
          }
        }
        _desktopRevision++;
        _changed();
      }
      return;
    }
  }

  Future<void> moveWindowToWorkspace(int windowId, int workspaceId) async {
    if (_disposed) return;
    if (demo) {
      if (_windowState.containsKey(windowId)) {
        _windowState[windowId] = {
          ..._windowState[windowId]!,
          'workspace_id': workspaceId,
        };
        _desktopRevision++;
        _changed();
      }
      return;
    }
    await _compositorAction('workspace', 'MoveWindowToWorkspace', {
      'window_id': windowId,
      'reference': {'Id': workspaceId},
      'focus': false,
    });
  }

  Future<void> toggleOverview() async {
    if (demo) {
      closePanel();
      return;
    }
    final panelRevision = _panelRevision;
    await _perform('overview', () async {
      if (!_connected) throw StateError('合成器连接暂时不可用');
      await _ipc.action('ToggleOverview');
      // Release the modal layer and keyboard only after the compositor accepts
      // the action. Errors leave the control visible so the user can retry.
      _closePanelIfCurrent(panelRevision);
    });
  }

  Future<void> takeScreenshot(
    ScreenshotMode mode, {
    bool showPointer = false,
  }) async {
    if (_disposed || _busy.contains('screenshot')) return;
    if (demo) {
      closePanel();
      return;
    }
    closePanel();
    final request = ++_screenshotRequest;
    var failed = false;
    await _perform('screenshot', () async {
      // The shell must release its modal surface before the compositor picks
      // a window or captures the desktop. Never screenshot the launcher itself.
      try {
        if (!_connected) throw StateError('合成器连接暂时不可用');
        await prepareScreenshot?.call();
        if (_disposed ||
            request != _screenshotRequest ||
            _panel != ShellPanel.none) {
          return;
        }
        await _ipc.action(
          switch (mode) {
            ScreenshotMode.region => 'Screenshot',
            ScreenshotMode.screen => 'ScreenshotScreen',
            ScreenshotMode.window => 'ScreenshotWindow',
          },
          {
            'show_pointer': showPointer,
            'path': null,
            if (mode != ScreenshotMode.region) 'write_to_disk': true,
            if (mode == ScreenshotMode.window) 'id': null,
          },
        );
        unawaited(_soundService.playCameraShutter());
      } on Object {
        if (_disposed || request != _screenshotRequest) return;
        failed = true;
        rethrow;
      }
    });
    if (!_disposed &&
        failed &&
        request == _screenshotRequest &&
        _panel == ShellPanel.none) {
      _setPanel(ShellPanel.screenshot);
      _changed();
    }
  }

  Future<void> launchApp(DesktopApp app) async {
    if (demo) {
      closePanel();
      return;
    }
    final panelRevision = _panelRevision;
    await _perform('launch', () async {
      await prepareAppMenus();
      if (_disposed) return;
      await _desktopApps.launch(app);
      _rememberRecentApp(app.id);
      _unresolvedRecentFocus = null;
      _closePanelIfCurrent(panelRevision);
    });
  }

  Future<void> focusWindow(int id) async {
    if (_disposed || !_windowState.containsKey(id)) return;
    final panelRevision = _panelRevision;
    if (demo) {
      for (final window in _windowState.values) {
        window['is_focused'] = window['id'] == id;
        if (window['id'] == id) window['is_minimized'] = false;
      }
      // switchWorkspace can notify synchronously, including when the active
      // workspace already matches. Those readers must see the restored window.
      _desktopRevision++;
      final workspaceId = _windowState[id]!['workspace_id'];
      if (workspaceId is int) await switchWorkspace(workspaceId);
      _closePanelIfCurrent(panelRevision);
      _changed();
      return;
    }
    await _perform('focus', () async {
      if (!_connected) throw StateError('合成器连接暂时不可用');
      await _ipc.action('FocusWindow', {'id': id});
      _closePanelIfCurrent(panelRevision);
    });
  }

  final _dockTargets = <int, Rect>{};
  final _dockDesiredTargets = <int, Rect>{};
  final _dockDirtyTargets = <int>{};
  int _dockTargetRevision = 0;
  int _dockConnectionRevision = 0;
  Map<int, Rect>? _pendingDockTargets;
  bool _dockTargetsRunning = false;
  String? _dockTargetsOutput;
  bool? _dockTargetsReduced;
  MacDockPosition? _dockTargetsEdge;
  MacMinimizeEffect? _dockTargetsEffect;
  bool _dockPlatformReducedMotion = false;

  Future<void> setDockMinimizeTargets(
    Map<int, Rect> targets, {
    bool? reduceMotion,
  }) async {
    if (reduceMotion != null) _dockPlatformReducedMotion = reduceMotion;
    if (_disposed || demo || !canMinimize) return;
    _dockDesiredTargets.clear();
    _dockDesiredTargets.addAll(targets);
    _pendingDockTargets = Map.of(_dockDesiredTargets);
    _dockTargetRevision++;
    if (_dockTargetsRunning) return;
    _dockTargetsRunning = true;
    try {
      while (!_disposed && _pendingDockTargets != null && _connected) {
        final desired = _pendingDockTargets!;
        _pendingDockTargets = null;
        final revision = _dockTargetRevision;
        final connectionRevision = _dockConnectionRevision;
        final output = _shellStyle == ShellStyle.macos ? _outputName : null;
        final motion = _reduceMotion || _dockPlatformReducedMotion;
        final edge = _macPreferences.dockPosition;
        final effect = _macPreferences.minimizeEffect;
        final refresh =
            output != _dockTargetsOutput ||
            motion != _dockTargetsReduced ||
            edge != _dockTargetsEdge ||
            effect != _dockTargetsEffect;
        final ids = {
          ..._dockTargets.keys,
          ..._dockDirtyTargets,
          ...desired.keys,
        };
        for (final id in ids) {
          if (_disposed ||
              !_connected ||
              connectionRevision != _dockConnectionRevision ||
              revision != _dockTargetRevision) {
            break;
          }
          final rect = output == null ? null : desired[id];
          if (!refresh &&
              !_dockDirtyTargets.contains(id) &&
              _dockTargets[id] == rect) {
            continue;
          }
          if (!_windowState.containsKey(id)) {
            _dockTargets.remove(id);
            _dockDirtyTargets.remove(id);
            continue;
          }
          await _ipc.action('SetMinimizeTarget', {
            'id': id,
            'target': rect == null
                ? null
                : {
                    'output': output,
                    'x': rect.left,
                    'y': rect.top,
                    'width': rect.width,
                    'height': rect.height,
                    'edge': edge.name,
                    'effect': effect.name,
                  },
            'reduce_motion': motion,
          });
          if (_disposed || connectionRevision != _dockConnectionRevision) break;
          if (revision != _dockTargetRevision) {
            // The old request may have reached the compositor. Its acknowledged
            // cache must not cause a newer A→B→A request or clear to be skipped.
            _dockTargets.remove(id);
            _dockDirtyTargets.add(id);
            break;
          }
          _dockDirtyTargets.remove(id);
          if (rect == null) {
            _dockTargets.remove(id);
          } else {
            _dockTargets[id] = rect;
          }
        }
        if (revision == _dockTargetRevision &&
            connectionRevision == _dockConnectionRevision) {
          _dockTargetsOutput = output;
          _dockTargetsReduced = motion;
          _dockTargetsEdge = edge;
          _dockTargetsEffect = effect;
        }
      }
    } on Object {
      // Geometry is advisory. Drop failed transport work rather than replaying
      // a window action into a new session. Fresh layout events register again.
      _dockTargets.clear();
      _dockDirtyTargets.clear();
      _pendingDockTargets = null;
    } finally {
      _dockTargetsRunning = false;
    }
  }

  Future<void> minimizeWindow(int id) async {
    if (_disposed || !_windowState.containsKey(id) || !canMinimize) return;
    final revision = _panelRevision;
    if (demo) {
      _windowState[id]!['is_minimized'] = true;
      _windowState[id]!['is_focused'] = false;
      _desktopRevision++;
      _closePanelIfCurrent(revision);
      _changed();
      return;
    }
    await _perform('minimize', () async {
      if (!_connected) throw StateError('合成器连接暂时不可用');
      await _ipc.action('MinimizeWindow', {'id': id});
      _closePanelIfCurrent(revision);
    });
  }

  Future<void> hideApp(String appId) async {
    if (_disposed) return;
    final resolved = _resolveAppId(appId) ?? appId;
    for (final app in dockApps) {
      if (app.id == resolved || app.id == appId) {
        for (final window in app.windows) {
          if (!window.isMinimized) {
            await minimizeWindow(window.id);
          }
        }
      }
    }
  }

  Future<void> hideOthers(String appId) async {
    if (_disposed) return;
    final resolved = _resolveAppId(appId) ?? appId;
    for (final app in dockApps) {
      if (app.id != resolved && app.id != appId) {
        for (final window in app.windows) {
          if (!window.isMinimized) {
            await minimizeWindow(window.id);
          }
        }
      }
    }
  }

  Future<void> showAll() async {
    if (_disposed) return;
    for (final window in windows) {
      if (window.isMinimized) {
        await focusWindow(window.id);
      }
    }
  }

  Future<void> focusOrLaunchApp(String appId) async {
    if (_disposed) return;
    final id = _resolveAppId(appId) ?? appId;
    for (final entry in dockApps) {
      if (entry.id != id || entry.windows.isEmpty) continue;
      final focused = entry.windows.where((window) => window.isFocused);
      await focusWindow(
        focused.isEmpty ? entry.windows.last.id : focused.first.id,
      );
      return;
    }
    for (final app in _apps) {
      if (app.id == id && (demo || app.desktopFile.isNotEmpty)) {
        await launchApp(app);
        return;
      }
    }
  }

  Future<void> closeAppWindows(String appId) async {
    if (_disposed) return;
    final app = dockApps.where((app) => app.id == appId).firstOrNull;
    if (app == null || app.windows.isEmpty) return;
    final revision = _panelRevision;
    final ids = app.windows.map((window) => window.id).toList();
    if (demo) {
      for (final id in ids) {
        _windowState.remove(id);
      }
      _desktopRevision++;
      _closePanelIfCurrent(revision);
      _changed();
      return;
    }
    await _perform('close-app-windows', () async {
      if (!_connected) throw StateError('合成器连接暂时不可用');
      final connection = _dockConnectionRevision;
      for (final id in ids) {
        if (_disposed || !_connected || connection != _dockConnectionRevision) {
          return;
        }
        if (!dockApps.any(
          (current) =>
              current.id == appId &&
              current.windows.any((window) => window.id == id),
        )) {
          continue;
        }
        await _ipc.action('CloseWindow', {'id': id});
      }
      _closePanelIfCurrent(revision);
    });
  }

  Future<void> forceQuitApp(String appId) async {
    if (_disposed) return;
    final revision = _panelRevision;
    final pids = <int>{};
    for (final window in windows) {
      final knownId = _resolveAppId(window.appId);
      final id =
          knownId ??
          (window.appId.trim().isEmpty
              ? 'window:${window.id}'
              : 'running:${_appKey(window.appId)}');
      if (id == appId || window.appId == appId) {
        if (window.pid != null && window.pid! > 1) {
          pids.add(window.pid!);
        }
      }
    }
    if (demo) {
      final app = dockApps.where((a) => a.id == appId).firstOrNull;
      if (app != null) {
        for (final window in app.windows) {
          _windowState.remove(window.id);
        }
        _desktopRevision++;
        _closePanelIfCurrent(revision);
        _changed();
      }
      return;
    }
    await _perform('force-quit-app', () async {
      await closeAppWindows(appId);
      for (final pid in pids) {
        try {
          await _system.command('kill', ['-KILL', pid.toString()]);
        } catch (_) {}
      }
      _closePanelIfCurrent(revision);
      _changed();
    });
  }

  Future<void> revealDockApp(String appId) async {
    final app = _apps.where((app) => app.id == appId).firstOrNull;
    if (_disposed || app == null || app.desktopFile.isEmpty) return;
    final revision = _panelRevision;
    await _perform('reveal-app', () async {
      await _system.openLocation(File(app.desktopFile).parent.path);
      _closePanelIfCurrent(revision);
    });
  }

  Future<void> openSettings() async {
    if (demo) {
      closePanel();
      return;
    }
    final panelRevision = _panelRevision;
    await _perform('settings', () async {
      await _ipc.action('Spawn', {
        'command': ['clarion-settings'],
      });
      _closePanelIfCurrent(panelRevision);
    });
  }

  Future<void> openHomeFolder() async {
    final panelRevision = _panelRevision;
    await _perform('home-folder', () async {
      await _system.openHomeFolder();
      _closePanelIfCurrent(panelRevision);
    });
  }

  Future<void> openDownloadsFolder() async {
    final panelRevision = _panelRevision;
    await _perform('downloads-folder', () async {
      await _system.openDownloadsFolder();
      _closePanelIfCurrent(panelRevision);
    });
  }

  Future<void> openDesktopFolder() async {
    final panelRevision = _panelRevision;
    await _perform('desktop-folder', () async {
      await _system.openDesktopFolder();
      _closePanelIfCurrent(panelRevision);
    });
  }

  Future<void> openTrash() async {
    final panelRevision = _panelRevision;
    await _perform('trash-folder', () async {
      await _system.openTrash();
      _closePanelIfCurrent(panelRevision);
    });
  }

  Future<void> emptyTrash() async {
    final panelRevision = _panelRevision;
    await _perform('empty-trash', () async {
      await trashState.emptyTrash();
      unawaited(_soundService.playTrashEmpty());
      _closePanelIfCurrent(panelRevision);
    });
  }

  void _setPanel(ShellPanel panel) {
    if (_panel == ShellPanel.spaces && panel != ShellPanel.spaces) {
      _windowPreviewService?.cancelQueued();
    }
    if (panel != ShellPanel.dockAppMenu &&
        panel != ShellPanel.dockDownloads &&
        panel != ShellPanel.dockContextMenu &&
        panel != ShellPanel.dockTrashMenu &&
        panel != ShellPanel.dockFinderMenu &&
        panel != ShellPanel.dockDownloadsMenu) {
      _dockMenuAnchor = null;
    }
    if (panel != ShellPanel.none) {
      _appMenuOpen = false;
      _appMenuDismissRevision++;
    }
    _panelRevision++;
    _panel = panel;
    audioOutputs.setActive(panel == ShellPanel.volume);
  }

  void _closePanelIfCurrent(int revision) {
    if (!_disposed && revision == _panelRevision) closePanel();
  }

  void _setError(String? message) {
    if (message != null && message.trim().isNotEmpty && _error != message) {
      unawaited(_soundService.playAlert());
    }
    _errorRevision++;
    _error = message;
  }

  Future<void> _slider(
    String key,
    double value,
    Future<void> Function(double) operation,
  ) {
    if (_disposed ||
        !value.isFinite ||
        (key == 'volume' && _busy.contains('audio-output'))) {
      return Future.value();
    }
    final bounded = value.clamp(key == 'brightness' ? .01 : 0.0, 1.0);
    if ((key == 'volume' ? volume : brightness) == null) {
      _setError('此设备的${key == 'volume' ? '音量' : '亮度'}服务不可用');
      _changed();
      return Future.value();
    }
    if (demo) {
      _commitSlider(key, bounded);
      _changed();
      return Future.value();
    }
    _sliderRefresh?.cancel();
    _sliderValues[key] = bounded;
    final revision = (_sliderRevisions[key] ?? 0) + 1;
    _sliderRevisions[key] = revision;
    _invalidateSystemRead(key);
    _setError(null);
    final errorRevision = _errorRevision;
    // Pointer feedback must never wait for a process or a system snapshot.
    _changed();
    return (_sliders[key] ??= _SliderQueue(
      (change) async {
        if (_disposed) return;
        _busy.add(key);
        _changed();
        try {
          await operation(change.value);
          if (_disposed) return;
          _commitSlider(key, change.value);
        } on Object catch (error) {
          if (!_disposed && change.revision == _sliderRevisions[key]) {
            _sliderValues.remove(key);
            if (change.errorRevision == _errorRevision) {
              _setError('操作失败：$error');
            }
          }
        } finally {
          if (!_disposed) {
            _invalidateSystemRead(key);
            _busy.remove(key);
            _changed();
          }
        }
      },
      () {
        if (_disposed || _sliderGestures.contains(key)) return;
        _sliderValues.remove(key);
        _scheduleSliderRefresh();
        _changed();
      },
    )).add(_SliderChange(bounded, revision, errorRevision));
  }

  void _commitSlider(String key, double value) {
    _status = SystemSnapshot(
      volume: key == 'volume' ? value : _status.volume,
      brightness: key == 'brightness' ? value : _status.brightness,
      batteryFraction: _status.batteryFraction,
      muted: _status.muted,
      charging: _status.charging,
      wifiEnabled: _status.wifiEnabled,
      bluetoothEnabled: _status.bluetoothEnabled,
      networkName: _status.networkName,
      isWireless: _status.isWireless,
    );
  }

  void _invalidateSystemRead(String key) {
    _systemVersions[key] = (_systemVersions[key] ?? 0) + 1;
  }

  void _scheduleSliderRefresh() {
    if (_disposed || demo || _sliderGestures.isNotEmpty) return;
    if (_busy.any((key) => ['mute', 'wifi', 'bluetooth'].contains(key))) return;
    if (_sliders.values.any((slider) => slider.hasWork)) return;
    _sliderRefresh?.cancel();
    // Reconcile once after both controls settle, never between drag frames.
    _sliderRefresh = Timer(const Duration(milliseconds: 180), () {
      _sliderRefresh = null;
      unawaited(_refreshSystem(reconcile: true));
    });
  }

  void _beginSliderAdjustment(String key) {
    if (_disposed ||
        (key == 'volume' && _busy.contains('audio-output')) ||
        (key == 'volume' ? volume : brightness) == null) {
      return;
    }
    _sliderRefresh?.cancel();
    _sliderGestures.add(key);
    _invalidateSystemRead(key);
    if (key == 'volume') audioOutputs.setPaused(true);
  }

  Future<void> _endSliderAdjustment(String key, double value) async {
    if (_disposed) return;
    _sliderGestures.remove(key);
    final current = key == 'volume' ? volume : brightness;
    final bounded = value.isFinite
        ? value.clamp(key == 'brightness' ? .01 : 0.0, 1.0)
        : current;
    if (bounded != null && bounded != current) {
      unawaited(
        _slider(
          key,
          bounded,
          key == 'volume' ? _system.setVolume : _system.setBrightness,
        ),
      );
    }
    final revision = _sliderRevisions[key];
    await _sliders[key]?.flush();
    if (_disposed ||
        _sliderGestures.contains(key) ||
        revision != _sliderRevisions[key]) {
      return;
    }
    _sliderValues.remove(key);
    _scheduleSliderRefresh();
    _changed();
  }

  void _finishSliderGestures({bool flushPending = false}) {
    if (_sliderGestures.isEmpty && !flushPending) return;
    _sliderGestures.clear();
    for (final queue in _sliders.values) {
      unawaited(queue.flush());
    }
    _scheduleSliderRefresh();
  }

  void _audioOutputsChanged() {
    if (_disposed || _audioSnapshotRevision == audioOutputs.snapshotRevision) {
      return;
    }
    _audioSnapshotRevision = audioOutputs.snapshotRevision;
    _invalidateSystemRead('volume');
    unawaited(_refreshAudio());
  }

  Future<void> _refreshAudio() async {
    if (_disposed ||
        demo ||
        _panel != ShellPanel.volume ||
        _systemProtected('volume')) {
      return;
    }
    final generation = ++_audioReadGeneration;
    final panelRevision = _panelRevision;
    final volumeRevision = _systemVersions['volume'];
    SystemAudioSnapshot audio;
    try {
      audio = await _system.readAudio();
    } on Object {
      audio = const SystemAudioSnapshot(volume: null, muted: false);
    }
    if (_disposed ||
        generation != _audioReadGeneration ||
        panelRevision != _panelRevision ||
        volumeRevision != _systemVersions['volume'] ||
        _systemProtected('volume')) {
      return;
    }
    final next = SystemSnapshot(
      volume: audio.volume,
      muted: audio.muted,
      brightness: _status.brightness,
      batteryFraction: _status.batteryFraction,
      charging: _status.charging,
      wifiEnabled: _status.wifiEnabled,
      bluetoothEnabled: _status.bluetoothEnabled,
      networkName: _status.networkName,
      isWireless: _status.isWireless,
    );
    _sliderValues.remove('volume');
    if (next != _status) {
      _status = next;
      _changed();
    }
  }

  Future<void> selectAudioOutput(String name) async {
    if (_disposed || _busy.contains('mute') || _busy.contains('audio-output')) {
      return;
    }
    if (demo) {
      await audioOutputs.select(name);
      return;
    }
    final panelRevision = _panelRevision;
    var completed = false;
    await _perform('audio-output', () async {
      _invalidateSystemRead('volume');
      _finishSliderGestures(flushPending: true);
      await _sliders['volume']?.flush();
      if (_disposed) return;
      await audioOutputs.select(name);
      if (!_disposed) {
        _invalidateSystemRead('volume');
        completed = true;
      }
    });
    // _perform releases the protection before accepting the new device's own
    // stored level. Existing per-device volume is never copied to the new sink.
    if (!_disposed) {
      await audioOutputs.refresh();
      if (completed && panelRevision == _panelRevision) {
        await _refreshAudio();
        if (audioOutputs.selected == name) {
          _closePanelIfCurrent(panelRevision);
        } else if (panelRevision == _panelRevision) {
          _setError('输出设备切换未生效，请重试');
          _changed();
        }
      }
    }
  }

  Future<void> openSoundPreferences() async {
    if (_disposed || demo) return;
    final revision = _panelRevision;
    await _perform('sound-preferences', () async {
      await _system.openSoundPreferences();
      _closePanelIfCurrent(revision);
    });
  }

  void beginVolumeAdjustment() => _beginSliderAdjustment('volume');
  Future<void> endVolumeAdjustment(double value) {
    unawaited(_soundService.playVolumeTick());
    return _endSliderAdjustment('volume', value);
  }

  void beginBrightnessAdjustment() => _beginSliderAdjustment('brightness');
  Future<void> endBrightnessAdjustment(double value) =>
      _endSliderAdjustment('brightness', value);

  Future<void> setVolume(double value) {
    if (_busy.contains('audio-output')) return Future.value();
    unawaited(_soundService.playVolumeTick());
    return _slider('volume', value, _system.setVolume);
  }

  Future<void> setBrightness(double value) =>
      _slider('brightness', value, _system.setBrightness);
  Future<void> toggleMute() async {
    if (_disposed || _busy.contains('audio-output')) return;
    if (demo) {
      _demoStatus(mutedValue: !muted);
      return;
    }
    await _perform('mute', () async {
      if (volume == null) throw StateError('音频服务不可用');
      final nextMuted = !muted;
      _sliderRefresh?.cancel();
      _invalidateSystemRead('volume');
      await _system.toggleMute();
      if (_disposed) return;
      _demoStatus(mutedValue: nextMuted);
      _invalidateSystemRead('volume');
    });
    // The actual sink may have changed since the last UI snapshot. Read after
    // _perform releases the mute guard so the result can replace the guess.
    _scheduleSliderRefresh();
  }

  Future<void> toggleWifi() async {
    if (_disposed) return;
    if (demo) {
      _demoStatus(wifi: !(wifiEnabled ?? false));
      return;
    }
    await _toggleConnectivity('wifi', _system.setWifi);
  }

  Future<void> toggleBluetooth() async {
    if (_disposed) return;
    if (demo) {
      _demoStatus(bluetooth: !(bluetoothEnabled ?? false));
      return;
    }
    await _toggleConnectivity('bluetooth', _system.setBluetooth);
  }

  Future<void> _toggleConnectivity(
    String key,
    Future<void> Function(bool) operation,
  ) async {
    await _perform(key, () async {
      final enabled = key == 'wifi' ? wifiEnabled : bluetoothEnabled;
      if (enabled == null) {
        throw StateError(key == 'wifi' ? 'NetworkManager 不可用' : '蓝牙控制器不可用');
      }
      final target = !enabled;
      _sliderRefresh?.cancel();
      _invalidateSystemRead(key);
      try {
        await operation(target);
        if (_disposed) return;
        _demoStatus(
          wifi: key == 'wifi' ? target : null,
          bluetooth: key == 'bluetooth' ? target : null,
        );
      } finally {
        if (!_disposed) _invalidateSystemRead(key);
      }
    });
    // Confirm the individual write immediately. Reconcile the combined system
    // snapshot once all controls settle, outside their busy guards.
    _scheduleSliderRefresh();
  }

  void _demoStatus({bool? mutedValue, bool? wifi, bool? bluetooth}) {
    _status = SystemSnapshot(
      volume: _status.volume,
      brightness: _status.brightness,
      batteryFraction: batteryFraction,
      muted: mutedValue ?? muted,
      charging: charging,
      wifiEnabled: wifi ?? wifiEnabled,
      bluetoothEnabled: bluetooth ?? bluetoothEnabled,
      networkName: networkName,
      isWireless: _status.isWireless,
    );
    _changed();
  }

  Future<void> scanWifiNetworks() async {
    if (_disposed || _busy.contains('wifi-scan')) return;
    _wifiScanning = true;
    _changed();
    if (demo) {
      _wifiNetworks = [
        WifiNetwork(
          ssid: wifiSsid.isNotEmpty ? wifiSsid : 'Clarion Studio',
          signal: 95,
          security: 'WPA2',
          inUse: true,
        ),
        const WifiNetwork(ssid: 'Office-5G', signal: 80, security: 'WPA2'),
        const WifiNetwork(ssid: 'Guest-WiFi', signal: 60, security: 'WPA2'),
        const WifiNetwork(ssid: 'Public-Airport', signal: 35, security: ''),
      ];
      _wifiScanning = false;
      _changed();
      return;
    }
    await _perform('wifi-scan', () async {
      try {
        _wifiNetworks = await _system.scanWifiNetworks();
      } finally {
        _wifiScanning = false;
        _changed();
      }
    });
  }

  Future<void> connectWifi(String ssid, {String? password}) async {
    if (_disposed) return;
    if (demo) {
      _status = SystemSnapshot(
        volume: _status.volume,
        brightness: _status.brightness,
        batteryFraction: _status.batteryFraction,
        muted: _status.muted,
        charging: _status.charging,
        wifiEnabled: true,
        bluetoothEnabled: _status.bluetoothEnabled,
        networkName: ssid,
        isWireless: true,
      );
      await scanWifiNetworks();
      return;
    }
    await _perform('wifi-connect', () async {
      await _system.connectWifi(ssid, password: password);
      await _refreshSystem(reconcile: true);
      await scanWifiNetworks();
    });
  }

  Future<void> refreshBluetoothDevices() async {
    if (_disposed || _busy.contains('bluetooth-scan')) return;
    _bluetoothScanning = true;
    _changed();
    if (demo) {
      _bluetoothDevices = const [
        BluetoothDevice(
          mac: '14:0A:29:52:ED:11',
          name: 'Mi Air2 SE',
          connected: false,
          icon: 'audio-headset',
        ),
        BluetoothDevice(
          mac: '00:11:22:33:44:55',
          name: 'Magic Keyboard',
          connected: true,
          icon: 'input-keyboard',
        ),
      ];
      _bluetoothScanning = false;
      _changed();
      return;
    }
    await _perform('bluetooth-scan', () async {
      try {
        _bluetoothDevices = await _system.listBluetoothDevices();
      } finally {
        _bluetoothScanning = false;
        _changed();
      }
    });
  }

  Future<void> toggleBluetoothDevice(BluetoothDevice device) async {
    if (_disposed) return;
    if (demo) {
      final updated = _bluetoothDevices.map((d) {
        if (d.mac == device.mac) {
          return BluetoothDevice(
            mac: d.mac,
            name: d.name,
            connected: !d.connected,
            icon: d.icon,
          );
        }
        return d;
      }).toList();
      _bluetoothDevices = updated;
      _changed();
      return;
    }
    await _perform('bluetooth-device', () async {
      if (device.connected) {
        await _system.disconnectBluetooth(device.mac);
      } else {
        await _system.connectBluetooth(device.mac);
      }
      await refreshBluetoothDevices();
    });
  }

  Future<void> openNetworkPreferences() async {
    if (_disposed) return;
    await _perform('network-preferences', () async {
      await _system.openNetworkPreferences();
    });
  }

  Future<void> openBluetoothPreferences() async {
    if (_disposed) return;
    await _perform('bluetooth-preferences', () async {
      await _system.openBluetoothPreferences();
    });
  }

  Future<bool> dismissNotification(
    int id, {
    ShellNotification? expected,
  }) async {
    if (_disposed) return false;
    if (demo) {
      final item = _notificationAt(id, expected);
      if (item == null) return false;
      _notifications = _notifications.where((n) => n.id != id).toList();
      _changed();
      return true;
    }
    return _performNotification(
      (service) => service.dismiss(id, expected: expected),
    );
  }

  Future<bool> invokeNotificationAction(
    int id,
    String key, {
    ShellNotification? expected,
  }) async {
    if (_disposed) return false;
    if (demo) {
      final item = _notificationAt(id, expected);
      if (item == null ||
          !item.active ||
          !item.actions.any((action) => action.key == key)) {
        return false;
      }
      if (!item.resident) await dismissNotification(id, expected: item);
      return true;
    }
    return _performNotification(
      (service) => service.invokeAction(id, key, expected: expected),
    );
  }

  ShellNotification? _notificationAt(int id, ShellNotification? expected) {
    for (final item in _notifications) {
      if (item.id == id && (expected == null || identical(item, expected))) {
        return item;
      }
    }
    return null;
  }

  Future<bool> clearNotifications() async {
    if (_disposed) return false;
    if (demo) {
      _notifications = const [];
      _changed();
      return true;
    }
    return _performNotification((service) => service.clear());
  }

  Future<bool> dismissNotifications(
    Iterable<ShellNotification> expected,
  ) async {
    if (_disposed) return false;
    final snapshot = expected.toSet();
    if (demo) {
      final allCurrent = snapshot.every(
        (item) => _notifications.contains(item),
      );
      _notifications = _notifications
          .where((item) => !snapshot.contains(item))
          .toList();
      _changed();
      return allCurrent;
    }
    return _performNotification((service) => service.dismissAll(snapshot));
  }

  Future<bool> _performNotification(
    Future<bool> Function(NotificationService) operation,
  ) async {
    if (_disposed || _busy.contains('notifications')) return false;
    _busy.add('notifications');
    final panelRevision = _panelRevision;
    _setError(null);
    final errorRevision = _errorRevision;
    _changed();
    try {
      final service = _notificationService;
      if (service == null || !service.available) {
        throw StateError('通知服务暂时不可用');
      }
      final succeeded = await operation(service);
      return !_disposed && succeeded;
    } on Object catch (e) {
      if (!_disposed &&
          panelRevision == _panelRevision &&
          errorRevision == _errorRevision) {
        _setError('操作失败：$e');
      }
      return false;
    } finally {
      _busy.remove('notifications');
      if (!_disposed) {
        _notifications = _notificationService?.notifications ?? const [];
        _changed();
      }
    }
  }

  void openTrayItems() {
    if (_disposed) return;
    _finishSliderGestures();
    _clearTrayMenu();
    _setPanel(
      _panel == ShellPanel.trayItems ? ShellPanel.none : ShellPanel.trayItems,
    );
    _changed();
  }

  void _clearTrayMenu() {
    _trayMenuRequest++;
    _selectedTrayItem = null;
    _trayMenu = const [];
    _trayMenuLoading = false;
    _trayError = null;
  }

  bool _currentTrayMenu(int request, String id) =>
      !_disposed &&
      _trayMenuRequest == request &&
      _panel == ShellPanel.trayMenu &&
      _selectedTrayItem?.id == id;

  Future<Offset> _trayCoordinates(double x, double y) async {
    // GTK provides logical monitor coordinates; pointer positions are logical
    // surface coordinates. Preserve negative origins on outputs to the left.
    try {
      final origin = await _window.invokeMapMethod<String, dynamic>(
        'surfaceOrigin',
      );
      final dx = origin?['x'];
      final dy = origin?['y'];
      if (dx is num && dy is num) return Offset(x + dx, y + dy);
    } on MissingPluginException {
      /* A normal preview/test has no layer-shell monitor origin. */
    } on PlatformException {
      /* Retain a valid local pointer coordinate if the output disappeared. */
    }
    return Offset(x, y);
  }

  Future<void> _performTray(Future<void> Function() operation) async {
    if (_disposed || _busy.contains('tray') || demo) return;
    _busy.add('tray');
    _trayError = null;
    _changed();
    try {
      await operation();
    } on Object catch (error) {
      if (!_disposed) _trayError = '托盘操作失败：$error';
    } finally {
      _busy.remove('tray');
      _changed();
    }
  }

  Future<void> activateTrayItem(
    TrayItem item, {
    double x = 0,
    double y = 0,
  }) async {
    if (_disposed) return;
    if (demo || item.primaryOpensMenu) {
      await openTrayMenu(item, x: x, y: y);
      return;
    }
    final request = _trayMenuRequest;
    await _performTray(() async {
      final point = await _trayCoordinates(x, y);
      if (_disposed) return;
      final activated =
          await _trayService?.invoke(
            item,
            TrayAction.activate,
            x: point.dx,
            y: point.dy,
          ) ??
          false;
      if (_disposed || request != _trayMenuRequest) return;
      if (!activated && item.menuAvailable) {
        await openTrayMenu(item, x: x, y: y);
      } else if (activated &&
          (_panel == ShellPanel.trayItems || _panel == ShellPanel.trayMenu)) {
        closePanel();
      } else if (!activated) {
        _trayError = '无法激活托盘项';
      }
    });
  }

  Future<void> secondaryActivateTrayItem(
    TrayItem item, {
    double x = 0,
    double y = 0,
  }) async {
    if (_disposed || demo) return;
    await _performTray(() async {
      final point = await _trayCoordinates(x, y);
      if (_disposed) return;
      final activated = await _trayService?.invoke(
        item,
        TrayAction.secondaryActivate,
        x: point.dx,
        y: point.dy,
      );
      if (!_disposed && activated != true) _trayError = '无法执行托盘辅助操作';
    });
  }

  Future<void> scrollTrayItem(
    TrayItem item,
    int delta, {
    String orientation = 'vertical',
  }) async {
    if (_disposed || demo || delta == 0) return;
    await _performTray(() async {
      await _trayService?.scroll(item, delta, orientation: orientation);
    });
  }

  Future<void> openTrayMenu(TrayItem item, {double x = 0, double y = 0}) async {
    if (_disposed) return;
    _finishSliderGestures();
    final request = ++_trayMenuRequest;
    _selectedTrayItem = item;
    _trayError = null;
    _trayMenu = const [];
    _setPanel(ShellPanel.trayMenu);
    if (demo) {
      _trayMenuLoading = false;
      _trayMenu = const [
        TrayMenuEntry(id: 1, label: '显示窗口'),
        TrayMenuEntry(id: 2, label: '退出'),
      ];
      _changed();
      return;
    }
    _trayMenuLoading = true;
    _changed();
    try {
      final entries = await _trayService?.loadMenu(item);
      if (!_currentTrayMenu(request, item.id)) return;
      _trayMenu = entries ?? const [];
      if (_trayMenu.isEmpty) {
        // SNI ContextMenu can exist without a DBusMenu object path.
        final point = await _trayCoordinates(x, y);
        if (!_currentTrayMenu(request, item.id)) return;
        final opened = await _trayService?.invoke(
          item,
          TrayAction.contextMenu,
          x: point.dx,
          y: point.dy,
        );
        if (_currentTrayMenu(request, item.id) && opened == true) closePanel();
      }
    } on Object catch (error) {
      if (_currentTrayMenu(request, item.id)) {
        _trayError = '托盘菜单加载失败：$error';
        _trayMenu = const [];
      }
    } finally {
      if (_currentTrayMenu(request, item.id)) {
        _trayMenuLoading = false;
        _changed();
      }
    }
  }

  Future<void> activateTrayMenuEntry(int id) async {
    final item = _selectedTrayItem;
    if (_disposed ||
        item == null ||
        _trayMenuLoading ||
        _panel != ShellPanel.trayMenu) {
      return;
    }
    bool actionable(List<TrayMenuEntry> entries) {
      for (final entry in entries) {
        if (!entry.visible || !entry.enabled || entry.separator) continue;
        if (entry.id == id && entry.children.isEmpty) return true;
        if (actionable(entry.children)) return true;
      }
      return false;
    }

    if (!actionable(_trayMenu)) return;
    if (demo) {
      closePanel();
      return;
    }
    final request = _trayMenuRequest;
    await _performTray(() async {
      final activated = await _trayService?.activateMenuEntry(item, id);
      if (!_currentTrayMenu(request, item.id)) return;
      if (activated == true) {
        closePanel();
      } else {
        _trayError = '菜单操作未完成，请重试';
      }
    });
  }

  void _changed() {
    audioOutputs.setPaused(_systemProtected('volume'));
    _syncAppMenu();
    if (_panel == ShellPanel.dockAppMenu && selectedDockApp == null) {
      _selectedDockAppId = null;
      _setPanel(ShellPanel.none);
    }
    if (_panel != ShellPanel.none) _barTooltip = null;
    if (!_disposed) notifyListeners();
  }

  void _loadDemo() {
    _connected = true;
    _notificationServerAvailable = true;
    _outputName = 'Preview';
    for (var i = 1; i <= 4; i++) {
      _workspaceState[i] = {
        'id': i,
        'idx': i,
        'name': ['创作', '开发', '生活', '空间'][i - 1],
        'output': 'Preview',
        'is_active': i == 1,
        'is_focused': i == 1,
      };
    }
    _windowState[1] = {
      'id': 1,
      'title': '欢迎来到 Clarion',
      'workspace_id': 1,
      'is_focused': true,
    };
    _status = const SystemSnapshot(
      volume: .64,
      brightness: .72,
      batteryFraction: .86,
      wifiEnabled: true,
      bluetoothEnabled: true,
      networkName: 'Clarion Studio',
      isWireless: true,
    );
    _apps = const [
      DesktopApp(
        id: 'terminal',
        name: '终端',
        desktopFile: '',
        description: '命令行与开发工具',
        iconName: 'utilities-terminal',
      ),
      DesktopApp(
        id: 'files',
        name: '文件',
        desktopFile: '',
        description: '浏览和管理文件',
        iconName: 'system-file-manager',
      ),
      DesktopApp(
        id: 'browser',
        name: '浏览器',
        desktopFile: '',
        description: '探索网络',
        iconName: 'web-browser',
      ),
      DesktopApp(
        id: 'settings',
        name: '配置中心',
        desktopFile: '',
        description: '打磨你的 Clarion',
        iconName: 'preferences-system',
      ),
    ];
    _indexApps(const {});
    _resolvePinnedApps(const {});
    _notifications = [
      ShellNotification(
        id: 1,
        appName: 'Clarion',
        summary: '属于你的桌面',
        body: '空间、应用和系统控制，统一在这里。',
        timestamp: _now,
      ),
    ];
    _trayAvailable = true;
    _trayItems = const [
      TrayItem(
        id: 'demo-network',
        title: '网络',
        iconName: 'network-wireless',
        menuAvailable: true,
        primaryOpensMenu: true,
      ),
      TrayItem(
        id: 'demo-chat',
        title: '消息',
        status: TrayStatus.needsAttention,
        iconName: 'mail-unread',
        menuAvailable: true,
      ),
    ];
  }

  @override
  void dispose() {
    if (_disposed) return;
    _barTooltip = null;
    _clearTrayMenu();
    _disposed = true;
    unawaited(_windowPreviewService?.dispose());
    _clock?.cancel();
    _poll?.cancel();
    _sliderRefresh?.cancel();
    for (final slider in _sliders.values) {
      slider.dispose();
    }
    for (final subscription in _subscriptions) {
      unawaited(subscription.cancel());
    }
    appMenu.removeListener(_appMenuChanged);
    audioOutputs.removeListener(_audioOutputsChanged);
    audioOutputs.dispose();
    media.removeListener(_changed);
    media.dispose();
    dockFolder.removeListener(_changed);
    dockFolder.dispose();
    trashState.removeListener(_changed);
    trashState.dispose();
    appMenu.dispose();
    _ipc.dispose();
    _notificationService?.dispose();
    _trayService?.dispose();
    _wallpaperService.dispose();
    prepareScreenshot = null;
    powerProfiles.dispose();
    if (_commandServer != null) {
      _commandServer!.close();
      try {
        final runtimeDir = Platform.environment['XDG_RUNTIME_DIR'] ?? '/tmp';
        final f = File('$runtimeDir/clarion-shell.sock');
        if (f.existsSync()) f.deleteSync();
      } on Object catch (_) {}
    }
    super.dispose();
  }

  Future<void> _startCommandServer() async {
    // Fake non-demo controllers still start in Flutter tests. They must never
    // unlink or replace the command socket belonging to the running desktop.
    if (Platform.environment.containsKey('FLUTTER_TEST')) return;
    try {
      final runtimeDir = Platform.environment['XDG_RUNTIME_DIR'] ?? '/tmp';
      final socketPath = '$runtimeDir/clarion-shell.sock';
      final socketFile = File(socketPath);
      if (await socketFile.exists()) {
        try {
          await socketFile.delete();
        } on Object catch (_) {}
      }
      final server = await ServerSocket.bind(
        InternetAddress(socketPath, type: InternetAddressType.unix),
        0,
      );
      if (_disposed) {
        await server.close();
        return;
      }
      _commandServer = server;
      server.listen((client) {
        client
            .cast<List<int>>()
            .transform(utf8.decoder)
            .transform(const LineSplitter())
            .listen((cmd) {
              final trimmed = cmd.trim();
              if (trimmed == 'spotlight') {
                selectPanel(ShellPanel.spotlight);
              } else if (trimmed == 'launcher') {
                selectPanel(ShellPanel.launcher);
              } else if (trimmed == 'close') {
                closePanel();
              } else if (trimmed == 'spaces' || trimmed == 'mission-control') {
                toggleSpaces();
              } else if (trimmed == 'control-center' ||
                  trimmed == 'quick-settings') {
                selectPanel(ShellPanel.quickSettings);
              } else if (trimmed == 'notifications') {
                selectPanel(ShellPanel.notifications);
              } else if (trimmed == 'overview') {
                if (shellStyle == ShellStyle.macos) {
                  toggleSpaces();
                } else {
                  toggleOverview();
                }
              }
            });
      });
    } on Object catch (_) {}
  }
}

class _SliderChange {
  const _SliderChange(this.value, this.revision, this.errorRevision);
  final double value;
  final int revision;
  final int errorRevision;
}

/// Coalesces pointer updates with one command in flight per control. Releasing
/// the thumb bypasses the timer; a pending final value follows an active command
/// immediately. System reads never occupy this queue.
class _SliderQueue {
  _SliderQueue(this.apply, this.onIdle);
  final Future<void> Function(_SliderChange) apply;
  final void Function() onIdle;
  _SliderChange? _value;
  Timer? _timer;
  bool _running = false;
  bool _disposed = false;
  bool _flushRequested = false;
  final _pending = <Completer<void>>[];
  final _active = <Completer<void>>[];
  final _drained = <Completer<void>>[];

  bool get hasWork => _running || _value != null;

  Future<void> add(_SliderChange value) {
    if (_disposed) return Future.value();
    _value = value;
    final completer = Completer<void>();
    _pending.add(completer);
    _schedule();
    return completer.future;
  }

  void _schedule() {
    if (!_running && _timer == null && !_disposed && _value != null) {
      if (_flushRequested) {
        unawaited(_run());
        return;
      }
      _timer = Timer(const Duration(milliseconds: 90), () {
        _timer = null;
        unawaited(_run());
      });
    }
  }

  Future<void> _run() async {
    final value = _value;
    if (value == null || _disposed) return;
    _value = null;
    _running = true;
    if (!_flushRequested) {
      // Count the command's own time toward the write interval. A slow backend
      // must not add another full delay before applying the newest drag value.
      _timer = Timer(const Duration(milliseconds: 90), () {
        _timer = null;
        if (!_running && !_disposed && _value != null) unawaited(_run());
      });
    }
    _active.addAll(_pending);
    _pending.clear();
    try {
      await apply(value);
    } finally {
      for (final c in _active) {
        if (!c.isCompleted) c.complete();
      }
      _active.clear();
      _running = false;
      if (!_disposed) {
        if (_value != null) {
          if (_timer == null) unawaited(_run());
        } else {
          _timer?.cancel();
          _timer = null;
          _flushRequested = false;
          for (final c in _drained) {
            if (!c.isCompleted) c.complete();
          }
          _drained.clear();
          onIdle();
        }
      }
    }
  }

  Future<void> flush() {
    if (_disposed || !hasWork) return Future.value();
    final completer = Completer<void>();
    _drained.add(completer);
    _flushRequested = true;
    _timer?.cancel();
    _timer = null;
    _schedule();
    return completer.future;
  }

  void dispose() {
    _disposed = true;
    _timer?.cancel();
    _timer = null;
    _value = null;
    for (final c in [..._pending, ..._active, ..._drained]) {
      if (!c.isCompleted) c.complete();
    }
    _pending.clear();
    _active.clear();
    _drained.clear();
  }
}

bool _validWindow(Object? window) =>
    window is Map<String, dynamic> &&
    window['id'] is int &&
    (window['title'] == null || window['title'] is String) &&
    (window['app_id'] == null || window['app_id'] is String) &&
    (window['workspace_id'] == null || window['workspace_id'] is int);
