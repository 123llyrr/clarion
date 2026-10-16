// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_localizations/flutter_localizations.dart';

import 'models.dart';
import 'mac_shell_preferences.dart';
import 'desktop/desktop_application.dart';
import 'preferences.dart';
import 'services/global_app_menu_service.dart';
import 'services/shell_startup.dart';
import 'state/shell_controller.dart';
import 'theme/shell_theme.dart';
import 'theme/mac_shell_theme.dart';
import 'ui/shell_view.dart';
import 'ui/mac_dock_view.dart';
import 'ui/mac_shell_view.dart';
import 'ui/controller_selector.dart';
import 'ui/shell_surface_clear.dart';

Future<void> main(List<String> arguments) async {
  WidgetsFlutterBinding.ensureInitialized();
  if (arguments.any(
    (argument) => const {
      '--desktop',
      '--desktop-only',
      '--desktop-preview',
    }.contains(argument),
  )) {
    await startDesktopApplication(arguments);
    return;
  }
  final demo = arguments.contains('--demo');
  final preview = demo || arguments.contains('--preview');
  int? monitor;
  for (var i = 0; i < arguments.length; i++) {
    if (arguments[i].startsWith('--monitor=')) {
      monitor = int.tryParse(arguments[i].substring('--monitor='.length));
    } else if (arguments[i] == '--monitor' && i + 1 < arguments.length) {
      monitor = int.tryParse(arguments[++i]);
    }
  }
  final preferences = await ShellPreferences.load();
  final controller = ShellController(
    demo: demo,
    notifications: !preview && !arguments.contains('--no-notifications'),
    tray: !preview && !arguments.contains('--no-tray'),
    restoreWallpaperOnStart: !preview && !arguments.contains('--no-wallpaper'),
    appMenuService: GlobalAppMenuService(hostRegistrar: !preview),
    monitor: monitor,
  );
  controller.setThemeMode(preferences.themeMode);
  controller.setReduceMotion(preferences.reduceMotion);
  controller.setBarPosition(preferences.barPosition);
  controller.setPinnedAppIds(preferences.pinnedAppIds);
  controller.setRecentAppIds(preferences.recentAppIds);
  controller.setMacPreferences(preferences.macShell);
  controller.setShellStyle(preferences.shellStyle);
  final previewStyle = arguments
      .where((argument) => argument.startsWith('--shell-style='))
      .lastOrNull;
  if (preview && previewStyle != null) {
    controller.setShellStyle(
      previewStyle.substring('--shell-style='.length) == 'macos'
          ? ShellStyle.macos
          : ShellStyle.material,
    );
  }
  runWidget(
    ShellHost(
      controller: controller,
      preferences: preferences,
      preview: preview,
      persist: !demo && previewStyle == null,
    ),
  );
  unawaited(controller.start());
  if (!preview) {
    unawaited(
      announceShellReady(
        firstFrame: WidgetsBinding.instance.waitUntilFirstFrameRasterized,
        prepareAppMenus: controller.prepareAppMenus,
        // Qt clients sample registrar availability while creating their menus.
        // Keep session autostart after preparation, without delaying rendering.
        notifyReady: () async {
          if (Platform.environment['NOTIFY_SOCKET']?.isNotEmpty ?? false) {
            final ready = await Process.run('systemd-notify', [
              '--ready',
            ]).timeout(const Duration(seconds: 2));
            if (ready.exitCode != 0) {
              throw StateError('systemd-notify exited ${ready.exitCode}');
            }
          }
        },
        report: (message) => stderr.writeln('Clarion Shell: $message'),
      ),
    );
  }
}

/// Both shell surfaces render in one engine and own one set of desktop services.
/// Native GTK views may appear after the first frame when macOS mode is chosen.
class ShellHost extends StatefulWidget {
  const ShellHost({
    super.key,
    required this.controller,
    required this.preferences,
    this.preview = false,
    this.persist = true,
  });

  final ShellController controller;
  final ShellPreferences preferences;
  final bool preview;
  final bool persist;

  @override
  State<ShellHost> createState() => _ShellHostState();
}

class _ShellHostState extends State<ShellHost> with WidgetsBindingObserver {
  int? _mainViewId;
  Size? _monitorSize;
  bool _geometryRefreshPending = false;
  bool _geometryRefreshAgain = false;

  @override
  void initState() {
    super.initState();
    final dispatcher = WidgetsBinding.instance.platformDispatcher;
    _mainViewId =
        dispatcher.implicitView?.viewId ?? dispatcher.views.firstOrNull?.viewId;
    WidgetsBinding.instance.addObserver(this);
    unawaited(_refreshOutputGeometry());
  }

  @override
  void didChangeMetrics() {
    if (mounted) setState(() {});
    unawaited(_refreshOutputGeometry());
  }

  Future<void> _refreshOutputGeometry() async {
    if (widget.preview) return;
    if (_geometryRefreshPending) {
      _geometryRefreshAgain = true;
      return;
    }
    _geometryRefreshPending = true;
    try {
      final state = await const MethodChannel(
        'clarion.shell/window',
      ).invokeMapMethod<String, dynamic>('windowState');
      final geometry = state?['monitorGeometry'];
      if (!mounted || geometry is! Map) return;
      final width = (geometry['width'] as num?)?.toDouble();
      final height = (geometry['height'] as num?)?.toDouble();
      if (width == null ||
          height == null ||
          !width.isFinite ||
          !height.isFinite ||
          width <= 0 ||
          height <= 0) {
        return;
      }
      final size = Size(width, height);
      if (_monitorSize != size) setState(() => _monitorSize = size);
    } on MissingPluginException {
      // Unit tests and ordinary previews use their own view bounds.
    } on PlatformException {
      // Removed outputs leave the current view geometry as the fallback.
    } finally {
      _geometryRefreshPending = false;
      if (mounted && _geometryRefreshAgain) {
        _geometryRefreshAgain = false;
        unawaited(_refreshOutputGeometry());
      }
    }
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final views = WidgetsBinding.instance.platformDispatcher.views.toList();
    _mainViewId ??= views.firstOrNull?.viewId;
    final mainView = views
        .where((view) => view.viewId == _mainViewId)
        .firstOrNull;
    final outputWidth = mainView == null
        ? 1200.0
        : mainView.physicalSize.width / mainView.devicePixelRatio;
    final outputHeight = mainView == null
        ? 800.0
        : mainView.physicalSize.height / mainView.devicePixelRatio;
    final outputSize = _monitorSize ?? Size(outputWidth, outputHeight);
    return ViewCollection(
      views: [
        for (final view in views)
          View(
            key: ValueKey('shell-native-view-${view.viewId}'),
            view: view,
            child: view.viewId == _mainViewId
                ? ShellApplication(
                    controller: widget.controller,
                    preferences: widget.preferences,
                    preview: widget.preview,
                    persist: widget.persist,
                    availableOutputSize: outputSize,
                    onOutputGeometryChanged: () =>
                        unawaited(_refreshOutputGeometry()),
                  )
                : DockApplication(
                    controller: widget.controller,
                    preferences: widget.preferences,
                    preview: widget.preview,
                    availableWidth: outputSize.width,
                    availableHeight: outputSize.height,
                  ),
          ),
      ],
    );
  }
}

/// A secondary view shares the controller but never disposes its services.
class DockApplication extends StatefulWidget {
  const DockApplication({
    super.key,
    required this.controller,
    required this.preferences,
    required this.availableWidth,
    this.availableHeight = 800,
    this.preview = false,
  });

  final ShellController controller;
  final ShellPreferences preferences;
  final double availableWidth;
  final double availableHeight;
  final bool preview;

  @override
  State<DockApplication> createState() => _DockApplicationState();
}

class _DockApplicationState extends State<DockApplication> {
  static const _window = MethodChannel('clarion.shell/window');
  Future<void> _geometryWork = Future.value();
  Size? _size;
  MacShellPreferences? _nativeDockPreferences;
  Rect? _inputRegion;
  List<Rect>? _inputRegions;
  Rect? _glassRegion;
  ({MacDockPosition position, Size size, int reservedSize, bool autoHide})?
  _appliedGeometry;
  Rect? _appliedInputRegion;
  List<Rect>? _appliedInputRegions;
  Rect? _appliedGlassRegion;
  Map<int, Rect> _targets = const {};
  Timer? _targetTimer;
  bool _presentationReducedMotion = false;
  int _geometryRevision = 0;
  bool _targetRegistrationRunning = false;
  bool _targetRegistrationDirty = false;
  int _menuRequest = 0;

  Future<void> _openDockMenu(Rect anchor, VoidCallback open) async {
    final request = ++_menuRequest;
    final panelRevision = widget.controller.panelRevision;
    final outputRevision = widget.controller.dockGeometryRevision;
    final preferences = widget.controller.macPreferences;
    bool current() =>
        mounted &&
        request == _menuRequest &&
        widget.controller.panelRevision == panelRevision &&
        widget.controller.dockGeometryRevision == outputRevision &&
        widget.controller.macPreferences == preferences &&
        widget.controller.shellStyle == ShellStyle.macos;
    await _geometryWork;
    if (!current()) return;
    try {
      final origin = await _window.invokeMapMethod<String, dynamic>(
        'surfaceOrigin',
        'dock',
      );
      if (!current()) return;
      final x = (origin?['outputX'] as num?)?.toDouble();
      final y = (origin?['outputY'] as num?)?.toDouble();
      if (x == null || y == null || !x.isFinite || !y.isFinite) return;
      widget.controller.setDockMenuAnchor(anchor.shift(Offset(x, y)));
      open();
    } on MissingPluginException {
      if (current()) {
        widget.controller.setDockMenuAnchor(anchor);
        open();
      }
    } on PlatformException {
      // A removed output invalidates the menu's anchor and pending request.
    }
  }

  @override
  void initState() {
    super.initState();
    widget.controller.addListener(_configurationChanged);
    // These services synchronously announce loading. Wait until sibling views
    // have finished mounting before notifying their shared controller.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted && !widget.preview) {
        unawaited(widget.controller.initializeDockServices());
      }
    });
  }

  void _configurationChanged() {
    final prefs = widget.controller.macPreferences;
    if (_nativeDockPreferences == prefs) return;
    _nativeDockPreferences = prefs;
    if (_size != null) _queueGeometry();
  }

  void _geometryChanged(Size size, Rect inputRegion) {
    if (_size == size && _inputRegion == inputRegion) return;
    _size = size;
    _inputRegion = inputRegion;
    _queueGeometry();
  }

  void _inputRegionsChanged(List<Rect> regions) {
    if (listEquals(_inputRegions, regions)) return;
    _inputRegions = List.unmodifiable(regions);
    _queueGeometry();
  }

  void _presentationChanged(
    Rect glass,
    Map<int, Rect> targets,
    bool reducedMotion,
  ) {
    _presentationReducedMotion = reducedMotion;
    _glassRegion = glass;
    _targets = targets;
    _queueGeometry();
    // Coalesce magnification frames. Resting and current visible icon geometry
    // replace stale registrations; IPC is never issued once per render tick.
    _targetTimer?.cancel();
    _targetTimer = Timer(const Duration(milliseconds: 100), () {
      if (mounted && !widget.preview) unawaited(_registerTargets());
    });
  }

  Future<void> _registerTargets() async {
    _targetRegistrationDirty = true;
    if (_targetRegistrationRunning) return;
    _targetRegistrationRunning = true;
    try {
      while (mounted && _targetRegistrationDirty) {
        _targetRegistrationDirty = false;
        final revision = _geometryRevision;
        await _geometryWork;
        if (!mounted || widget.controller.shellStyle != ShellStyle.macos) {
          return;
        }
        if (revision != _geometryRevision) {
          _targetRegistrationDirty = true;
          continue;
        }
        final targets = Map<int, Rect>.of(_targets);
        final reducedMotion = _presentationReducedMotion;
        final origin = await _window.invokeMapMethod<String, dynamic>(
          'surfaceOrigin',
          'dock',
        );
        if (!mounted || widget.controller.shellStyle != ShellStyle.macos) {
          return;
        }
        if (revision != _geometryRevision) {
          _targetRegistrationDirty = true;
          continue;
        }
        if (origin == null) return;
        final dx = (origin['outputX'] as num?)?.toDouble();
        final dy = (origin['outputY'] as num?)?.toDouble();
        if (dx == null || dy == null || !dx.isFinite || !dy.isFinite) return;
        await widget.controller.setDockMinimizeTargets({
          for (final entry in targets.entries)
            entry.key: entry.value.shift(Offset(dx, dy)),
        }, reduceMotion: reducedMotion);
      }
    } on MissingPluginException {
      // No native surface in widget tests.
    } on PlatformException {
      // A removed output invalidates geometry; the compositor clears its targets.
    } finally {
      _targetRegistrationRunning = false;
    }
  }

  void _queueGeometry() {
    final revision = ++_geometryRevision;
    _geometryWork = _geometryWork.then((_) async {
      if (!mounted || widget.preview || revision != _geometryRevision) return;
      try {
        final size = _size;
        if (size == null) return;
        final prefs = widget.controller.macPreferences;
        final geometry = (
          position: prefs.dockPosition,
          size: size,
          reservedSize: (prefs.dockSize + 16).ceil(),
          autoHide: prefs.dockAutoHide,
        );
        // Magnification changes the hit/glass regions on each frame, while
        // the reserved native surface stays fixed. Reconfiguring that surface
        // also resizes GTK and updates reservations and background effects.
        // Cache successful calls, including ones superseded while awaiting a
        // reply, so the next revision sends only what still differs.
        if (_appliedGeometry != geometry) {
          await _window.invokeMethod<void>('setDockGeometry', {
            'position': geometry.position.name,
            'width': geometry.size.width,
            'height': geometry.size.height,
            'reservedSize': geometry.reservedSize,
            'autoHide': geometry.autoHide,
          });
          _appliedGeometry = geometry;
        }
        if (!mounted || revision != _geometryRevision || _inputRegion == null) {
          return;
        }
        final region = _inputRegion!;
        final regions = _inputRegions;
        if (_appliedInputRegion != region ||
            !listEquals(_appliedInputRegions, regions)) {
          await _window.invokeMethod<void>('setDockInputRegion', {
            'x': region.left,
            'y': region.top,
            'width': region.width,
            'height': region.height,
            if (regions != null)
              'regions': [
                for (final rect in regions)
                  {
                    'x': rect.left,
                    'y': rect.top,
                    'width': rect.width,
                    'height': rect.height,
                  },
              ],
          });
          _appliedInputRegion = region;
          _appliedInputRegions = regions;
        }
        if (!mounted || revision != _geometryRevision) return;
        final glass = _glassRegion;
        if (glass != null && _appliedGlassRegion != glass) {
          await _window.invokeMethod<void>('setDockGlassRegion', {
            'x': glass.left,
            'y': glass.top,
            'width': glass.width,
            'height': glass.height,
            'radius': 15,
          });
          _appliedGlassRegion = glass;
        }
      } on MissingPluginException {
        // Unit tests do not host native GTK surfaces.
      } on PlatformException catch (error) {
        stderr.writeln('Clarion Dock geometry: ${error.message}');
      }
    });
  }

  @override
  void dispose() {
    _geometryRevision++;
    _menuRequest++;
    _targetTimer?.cancel();
    widget.controller.removeListener(_configurationChanged);
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => ControllerSelector(
    controller: widget.controller,
    select: () => (
      style: widget.controller.shellStyle,
      theme: widget.controller.themeMode,
      motion: widget.controller.reduceMotion,
      preferences: widget.controller.macPreferences,
    ),
    builder: (context, _) => ShellSurfaceClear(
      child: MaterialApp(
        debugShowCheckedModeBanner: false,
        title: 'Clarion Dock',
        supportedLocales: const [Locale('zh'), Locale('en')],
        localizationsDelegates: GlobalMaterialLocalizations.delegates,
        theme: widget.controller.shellStyle == ShellStyle.macos
            ? buildMacShellTheme(Brightness.light)
            : buildShellTheme(Brightness.light, seed: widget.preferences.seed),
        darkTheme: widget.controller.shellStyle == ShellStyle.macos
            ? buildMacShellTheme(Brightness.dark)
            : buildShellTheme(Brightness.dark, seed: widget.preferences.seed),
        themeMode: widget.controller.themeMode,
        builder: (context, child) => MediaQuery(
          data: MediaQuery.of(context).copyWith(
            disableAnimations:
                widget.controller.reduceMotion ||
                MediaQuery.of(context).disableAnimations,
          ),
          child: child!,
        ),
        home: widget.controller.shellStyle == ShellStyle.macos
            ? MacDockView(
                controller: widget.controller,
                availableWidth: widget.availableWidth,
                availableHeight: widget.availableHeight,
                onGeometryChanged: _geometryChanged,
                onInputRegionsChanged: _inputRegionsChanged,
                onPresentationChanged: _presentationChanged,
                onOpenMenu: _openDockMenu,
              )
            : const SizedBox.expand(),
      ),
    ),
  );
}

class ShellApplication extends StatefulWidget {
  const ShellApplication({
    super.key,
    required this.controller,
    required this.preferences,
    this.preview = false,
    this.persist = true,
    this.availableOutputSize = const Size(1200, 800),
    this.onOutputGeometryChanged,
  });
  final ShellController controller;
  final ShellPreferences preferences;
  final bool preview;
  final bool persist;
  final Size availableOutputSize;
  final VoidCallback? onOutputGeometryChanged;

  @override
  State<ShellApplication> createState() => _ShellApplicationState();
}

typedef _MenuWindowState = ({
  bool autoHide,
  bool visible,
  double widthPercent,
  double height,
});
typedef _LauncherWindowState = ({bool visible, bool interactive});
typedef _DesiredWindowState = ({
  ShellStyle style,
  BarPosition position,
  _MenuWindowState menu,
  _LauncherWindowState launcher,
  bool expanded,
  bool tooltip,
});

class _ShellApplicationState extends State<ShellApplication> {
  static const _window = MethodChannel('clarion.shell/window');
  late ThemeMode _savedTheme;
  late bool _savedMotion;
  late BarPosition _savedBarPosition;
  late ShellStyle _savedStyle;
  late MacShellPreferences _savedMacPreferences;
  Timer? _preferenceSaveTimer;
  List<String>? _savedPins;
  List<String> _savedRecentApps = const [];
  ShellStyle? _appliedStyle;
  bool? _appliedExpanded;
  BarPosition? _nativeBarPosition;
  _MenuWindowState? _nativeMenu;
  _LauncherWindowState? _nativeLauncher;
  _DesiredWindowState? _requestedWindowState;
  final _surfaceClear = ValueNotifier<int>(0);
  bool? _nativeTooltip;
  Rect? _panelGlass;
  Rect? _appliedPanelGlass;
  bool _launcherSurfaceVisible = false;
  bool _notificationSurfaceVisible = false;
  bool _controlSurfaceVisible = false;
  // Immediate close/replacement must also apply before the next widget build.
  bool get _controlExiting =>
      _controlSurfaceVisible &&
      widget.controller.panel == ShellPanel.none &&
      !widget.controller.reduceMotion;
  int _nativeRevision = 0;
  Future<void> _windowWork = Future.value();
  Future<void> _preferenceWork = Future.value();

  @override
  void initState() {
    super.initState();
    _savedTheme = widget.controller.themeMode;
    _savedMotion = widget.controller.reduceMotion;
    _savedBarPosition = widget.controller.barPosition;
    _savedStyle = widget.controller.shellStyle;
    _savedMacPreferences = widget.controller.macPreferences;
    _savedPins = widget.controller.pinnedAppIdsPreference?.toList();
    _savedRecentApps = widget.controller.recentAppIds.toList();
    _window.setMethodCallHandler((call) async {
      if (mounted && call.method == 'dismissBarTooltip') {
        widget.controller.dismissBarTooltip();
      } else if (mounted && call.method == 'outputGeometryChanged') {
        widget.controller.outputGeometryChanged();
        widget.onOutputGeometryChanged?.call();
      } else if (mounted && call.method == 'shellStyleFallback') {
        stderr.writeln('Clarion Dock unavailable: ${call.arguments}');
        if (widget.controller.shellStyle == ShellStyle.macos) {
          widget.controller.shellStyleFailed('macOS 外壳启动失败。');
        }
      }
    });
    widget.controller.addListener(_changed);
    widget.controller.prepareScreenshot = _prepareScreenshot;
    _changed();
  }

  // Derive desired state once. Applied native state remains separate because
  // replies can arrive after a newer close, hover or style change.
  _DesiredWindowState get _desiredWindowState {
    final controller = widget.controller;
    final expanded =
        controller.panel != ShellPanel.none ||
        controller.appMenuOpen ||
        _launcherSurfaceVisible ||
        _notificationSurfaceVisible ||
        _controlExiting;
    final macos = controller.shellStyle == ShellStyle.macos;
    return (
      style: controller.shellStyle,
      position: controller.barPosition,
      menu: (
        autoHide: controller.macPreferences.menuBarAutoHide,
        visible: controller.macMenuBarVisible,
        widthPercent: controller.macPreferences.menuBarWidthPercent,
        height: controller.macPreferences.menuBarHeight,
      ),
      launcher: (
        visible:
            macos &&
            (controller.panel == ShellPanel.launcher ||
                _launcherSurfaceVisible),
        interactive: macos && controller.panel == ShellPanel.launcher,
      ),
      expanded: expanded,
      tooltip: !expanded && controller.barTooltip != null,
    );
  }

  void _requestSurfaceClear() {
    if (mounted && !widget.preview) _surfaceClear.value++;
  }

  void _changed() {
    final controller = widget.controller;
    final requested = _desiredWindowState;
    if (_requestedWindowState != requested) {
      _requestedWindowState = requested;
      final revision = ++_nativeRevision;
      // Apply one immutable snapshot through the serialized queue. Revision
      // checks stop old work when a close or hover supersedes a slow reply.
      _windowWork = _windowWork.then((_) async {
        if (!mounted || widget.preview || revision != _nativeRevision) return;
        final panelRevision = controller.panelRevision;
        var clearAfterCommit = false;
        try {
          final requestedStyle = requested.style;
          if (_appliedStyle != requestedStyle) {
            _appliedPanelGlass = null;
            await _window.invokeMethod<void>(
              'setShellStyle',
              requestedStyle.name,
            );
            _appliedStyle = requestedStyle;
            // The runner resets these when a native view changes style.
            _appliedExpanded = null;
            _nativeLauncher = null;
            _nativeTooltip = null;
          }
          if (!mounted || revision != _nativeRevision) return;
          final position = requested.position;
          if (_nativeBarPosition != position) {
            await _window.invokeMethod<void>('setBarPosition', position.name);
            _nativeBarPosition = position;
          }
          if (!mounted || revision != _nativeRevision) return;
          final menu = requested.menu;
          if (_nativeMenu != menu) {
            await _window.invokeMethod<void>('setMacMenuBarPreferences', {
              'autoHide': menu.autoHide,
              'visible': menu.visible,
              'widthPercent': menu.widthPercent,
              'height': menu.height,
            });
            _nativeMenu = menu;
          }
          if (!mounted || revision != _nativeRevision) return;
          final launcher = requested.launcher;
          if (_nativeLauncher != launcher) {
            clearAfterCommit |=
                _nativeLauncher?.visible == true && !launcher.visible;
            if (launcher.visible) _appliedPanelGlass = null;
            await _window.invokeMethod<void>('setLauncherBackdrop', {
              'visible': launcher.visible,
              'dockInteractive': launcher.interactive,
            });
            _nativeLauncher = launcher;
          }
          if (!mounted || revision != _nativeRevision) return;
          final requestedExpanded = requested.expanded;
          if (_appliedExpanded != requestedExpanded) {
            clearAfterCommit |= _appliedExpanded == true && !requestedExpanded;
            _appliedPanelGlass = null;
            await _window.invokeMethod<void>('setExpanded', requestedExpanded);
            _appliedExpanded = requestedExpanded;
            if (requestedExpanded) _nativeTooltip = false;
          }
          if (!mounted || revision != _nativeRevision) return;
          clearAfterCommit |= await _syncPanelGlass();
          if (!mounted || revision != _nativeRevision) return;
          final tooltip = requested.tooltip;
          if (_nativeTooltip != tooltip) {
            await _window.invokeMethod<void>('setTooltipVisible', tooltip);
            _nativeTooltip = tooltip;
          }
        } on MissingPluginException {
          if (mounted && revision == _nativeRevision) {
            _appliedStyle = controller.shellStyle;
          }
          // Flutter widget tests have no Linux runner.
        } on PlatformException catch (error) {
          stderr.writeln('Clarion Shell window: ${error.message}');
          if (!mounted || revision != _nativeRevision) return;
          if (controller.shellStyle != ShellStyle.material) {
            controller.shellStyleFailed('macOS 外壳启动失败。');
          } else if (controller.panel != ShellPanel.none &&
              panelRevision == controller.panelRevision) {
            controller.closePanel();
          }
        } finally {
          // A delayed native reply can outlive the last animation frame. Send
          // one explicit root clear after removing effects, even while idle.
          if (clearAfterCommit) _requestSurfaceClear();
        }
      });
    }
    if (_savedTheme != controller.themeMode ||
        _savedMotion != controller.reduceMotion ||
        _savedBarPosition != controller.barPosition ||
        _savedStyle != controller.shellStyle ||
        _savedMacPreferences != controller.macPreferences ||
        !listEquals(_savedRecentApps, controller.recentAppIds) ||
        !listEquals(_savedPins, controller.pinnedAppIdsPreference)) {
      _savedTheme = controller.themeMode;
      _savedMotion = controller.reduceMotion;
      _savedBarPosition = controller.barPosition;
      _savedStyle = controller.shellStyle;
      _savedMacPreferences = controller.macPreferences;
      _savedPins = controller.pinnedAppIdsPreference?.toList();
      _savedRecentApps = controller.recentAppIds.toList();
      setState(() {});
      if (!widget.persist) return;
      _preferenceSaveTimer?.cancel();
      _preferenceSaveTimer = Timer(const Duration(milliseconds: 220), () {
        _preferenceWork = _preferenceWork.then((_) async {
          if (!await _waitAppliedStyle()) return;
          if (!mounted) return;
          // Persist the current, successfully applied choice after native calls.
          // A failed Dock setup must never leave macOS selected on next login.
          final preferences = ShellPreferences(
            themeMode: controller.themeMode,
            reduceMotion: controller.reduceMotion,
            barPosition: controller.barPosition,
            shellStyle: controller.shellStyle,
            pinnedAppIds: controller.pinnedAppIdsPreference,
            recentAppIds: controller.recentAppIds,
            seed: widget.preferences.seed,
            macShell: controller.macPreferences,
          );
          try {
            await preferences.save();
          } on FileSystemException catch (error) {
            stderr.writeln('Clarion Shell preferences: ${error.message}');
          }
        });
      });
    }
  }

  void _panelGlassChanged(Rect? region) {
    if (!mounted || widget.preview || _panelGlass == region) return;
    _panelGlass = region;
    // Geometry and window state must share one queue: the runner discards
    // regions received before expansion and clears them again on collapse.
    _windowWork = _windowWork.then((_) async {
      if (await _syncPanelGlass()) _requestSurfaceClear();
    });
  }

  Future<bool> _syncPanelGlass() async {
    if (!mounted || widget.preview) return false;
    final region =
        widget.controller.shellStyle == ShellStyle.macos &&
            (widget.controller.panel == ShellPanel.quickSettings ||
                _controlExiting) &&
            _appliedExpanded == true
        ? _panelGlass ?? Rect.zero
        : Rect.zero;
    if (_appliedPanelGlass == region) return false;
    final removed = _appliedPanelGlass?.isEmpty == false;
    try {
      await _window.invokeMethod<void>('setPanelGlassRegion', {
        'x': region.left,
        'y': region.top,
        'width': region.width,
        'height': region.height,
        'radius': 16.0,
      });
      _appliedPanelGlass = region;
      return removed;
    } on MissingPluginException {
      // Older runners and widget previews may not expose regional glass.
    } on PlatformException {
      // Losing an output must not block later window-state transitions.
    }
    return false;
  }

  Future<void> _drainWindowWork() async {
    while (mounted) {
      final work = _windowWork;
      await work;
      if (identical(work, _windowWork)) return;
    }
  }

  Future<bool> _waitAppliedStyle() async {
    final deadline = DateTime.now().add(const Duration(seconds: 12));
    while (mounted) {
      await _drainWindowWork();
      if (!mounted) return false;
      if (widget.preview) return true;
      final desired = widget.controller.shellStyle;
      if (_appliedStyle != desired) {
        if (desired == ShellStyle.material ||
            DateTime.now().isAfter(deadline)) {
          return false;
        }
        await Future<void>.delayed(const Duration(milliseconds: 30));
        continue;
      }
      if (desired != ShellStyle.macos) return true;
      try {
        final state = await _window.invokeMapMethod<String, dynamic>(
          'windowState',
        );
        if (state == null) {
          return true; // A unit test may not expose window geometry.
        }
        if (widget.controller.shellStyle != desired) continue;
        if (state['shellStyle'] == desired.name &&
            (state['dock'] as Map?)?['ready'] == true) {
          // Recheck the queue after the native reply; a rapid reversal may
          // have appended another style change while that reply was pending.
          final work = _windowWork;
          await work;
          if (identical(work, _windowWork) &&
              widget.controller.shellStyle == desired) {
            return true;
          }
          continue;
        }
      } on MissingPluginException {
        return true;
      } on PlatformException catch (error) {
        stderr.writeln('Clarion Shell readiness: ${error.message}');
      }
      if (DateTime.now().isAfter(deadline)) {
        widget.controller.shellStyleFailed('macOS 外壳未能完成启动。');
        await _drainWindowWork();
        return mounted && _appliedStyle == widget.controller.shellStyle;
      }
      await Future<void>.delayed(const Duration(milliseconds: 30));
    }
    return false;
  }

  void _launcherSurfaceChanged(bool visible) {
    if (!mounted || _launcherSurfaceVisible == visible) return;
    _launcherSurfaceVisible = visible;
    _changed();
  }

  void _notificationSurfaceChanged(bool visible) {
    if (!mounted || _notificationSurfaceVisible == visible) return;
    _notificationSurfaceVisible = visible;
    _changed();
  }

  void _controlSurfaceChanged(bool visible) {
    if (!mounted || _controlSurfaceVisible == visible) return;
    _controlSurfaceVisible = visible;
    _changed();
  }

  Future<void> _prepareScreenshot() async {
    final deadline = DateTime.now().add(const Duration(seconds: 2));
    Future<T> bounded<T>(Future<T> operation) {
      final remaining = deadline.difference(DateTime.now());
      if (remaining <= Duration.zero) {
        throw TimeoutException('截图界面尚未准备好，请稍后重试。');
      }
      return operation.timeout(
        remaining,
        onTimeout: () {
          throw TimeoutException('截图界面尚未准备好，请稍后重试。');
        },
      );
    }

    while (mounted) {
      await bounded(_drainWindowWork());
      if (widget.controller.panel != ShellPanel.none) return;
      if (widget.preview) break;
      try {
        final state = await bounded(
          _window.invokeMapMethod<String, dynamic>('windowState'),
        );
        // Native surfaces stay output-sized when collapsed. Wait for modal
        // state to clear, rather than a resize back to the bar's visual height.
        if (state == null ||
            (state['expanded'] == false &&
                state['tooltipVisible'] == false &&
                !_launcherSurfaceVisible &&
                !_notificationSurfaceVisible &&
                !_controlSurfaceVisible)) {
          break;
        }
      } on MissingPluginException {
        break;
      }
      if (DateTime.now().isAfter(deadline)) {
        throw StateError('截图界面尚未准备好，请稍后重试。');
      }
      await bounded(Future<void>.delayed(const Duration(milliseconds: 16)));
    }
    if (mounted) await bounded(WidgetsBinding.instance.endOfFrame);
  }

  @override
  void dispose() {
    _preferenceSaveTimer?.cancel();
    _window.setMethodCallHandler(null);
    widget.controller.prepareScreenshot = null;
    widget.controller.removeListener(_changed);
    widget.controller.dispose();
    _surfaceClear.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final app = MaterialApp(
      debugShowCheckedModeBanner: false,
      title: 'Clarion Shell',
      supportedLocales: const [Locale('zh'), Locale('en')],
      localizationsDelegates: GlobalMaterialLocalizations.delegates,
      theme: widget.controller.shellStyle == ShellStyle.macos
          ? buildMacShellTheme(Brightness.light)
          : buildShellTheme(Brightness.light, seed: widget.preferences.seed),
      darkTheme: widget.controller.shellStyle == ShellStyle.macos
          ? buildMacShellTheme(Brightness.dark)
          : buildShellTheme(Brightness.dark, seed: widget.preferences.seed),
      themeMode: widget.controller.themeMode,
      builder: (context, child) => MediaQuery(
        data: MediaQuery.of(context).copyWith(
          disableAnimations:
              widget.controller.reduceMotion ||
              MediaQuery.of(context).disableAnimations,
        ),
        child: child!,
      ),
      home: Builder(
        builder: (context) {
          if (widget.controller.shellStyle != ShellStyle.macos) {
            return ShellView(
              controller: widget.controller,
              preview: widget.preview,
              onLauncherSurfaceVisible: _launcherSurfaceChanged,
            );
          }
          final menu = MacShellView(
            controller: widget.controller,
            preview: widget.preview,
            availableWidth: widget.preview
                ? MediaQuery.sizeOf(context).width
                : widget.availableOutputSize.width,
            onLauncherSurfaceVisible: _launcherSurfaceChanged,
            onNotificationSurfaceVisible: _notificationSurfaceChanged,
            onPanelGlassChanged: _panelGlassChanged,
            onControlSurfaceVisible: _controlSurfaceChanged,
          );
          if (!widget.preview) return menu;
          // Ordinary previews have one GTK window and need no layer-shell Dock.
          return Stack(
            fit: StackFit.expand,
            children: [
              menu,
              Positioned(
                left:
                    widget.controller.macPreferences.dockPosition ==
                        MacDockPosition.right
                    ? null
                    : 0,
                right:
                    widget.controller.macPreferences.dockPosition ==
                        MacDockPosition.left
                    ? null
                    : 0,
                top:
                    widget.controller.macPreferences.dockPosition ==
                        MacDockPosition.bottom
                    ? null
                    : 0,
                bottom: 0,
                height:
                    widget.controller.macPreferences.dockPosition ==
                        MacDockPosition.bottom
                    ? MacDockView.surfaceThickness(
                        widget.controller.macPreferences,
                      )
                    : null,
                width:
                    widget.controller.macPreferences.dockPosition ==
                        MacDockPosition.bottom
                    ? null
                    : MacDockView.surfaceThickness(
                        widget.controller.macPreferences,
                      ),
                child: IgnorePointer(
                  ignoring: widget.controller.panel != ShellPanel.none,
                  child: MacDockView(
                    controller: widget.controller,
                    availableWidth: MediaQuery.sizeOf(context).width,
                    availableHeight: MediaQuery.sizeOf(context).height,
                  ),
                ),
              ),
            ],
          );
        },
      ),
    );
    return widget.preview
        ? app
        : ShellSurfaceClear(repaint: _surfaceClear, child: app);
  }
}
