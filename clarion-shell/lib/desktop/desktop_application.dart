// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_localizations/flutter_localizations.dart';

import '../models.dart';
import '../preferences.dart';
import '../services/desktop_apps.dart';
import '../services/clarion_ipc.dart';
import '../theme/shell_theme.dart';
import '../theme/mac_shell_theme.dart';
import 'desktop_layout.dart';
import 'desktop_service.dart';
import 'desktop_view.dart';

Future<void> startDesktopApplication(List<String> arguments) async {
  String? option(String name) {
    for (final argument in arguments.reversed) {
      if (argument.startsWith('$name=')) {
        return argument.substring(name.length + 1);
      }
    }
    return null;
  }

  final preferences = await ShellPreferences.load();
  final preview = arguments.contains('--desktop-preview');
  final application = DesktopApplication(
    service: DesktopService(root: option('--desktop-dir')),
    layout: DesktopLayoutStore(),
    preferences: preferences,
    outputName: option('--output-name') ?? 'desktop',
    preview: preview,
    nativeViews: !preview,
  );
  if (preview) {
    runApp(application);
  } else {
    runWidget(application);
  }
}

/// Each output owns a desktop view; files and the saved appearance are shared.
/// This role does not start the bar's device polling or notification servers.
class DesktopApplication extends StatefulWidget {
  const DesktopApplication({
    super.key,
    required this.service,
    required this.layout,
    this.preferences = const ShellPreferences(),
    this.outputName = 'desktop',
    this.preview = false,
    this.watchPreferences = true,
    this.preferencesFile,
    this.applications,
    this.nativeViews = false,
    this.capabilityIpc,
  });

  final DesktopService service;
  final DesktopLayoutStore layout;
  final ShellPreferences preferences;
  final String outputName;
  final bool preview;
  final bool watchPreferences;
  final File? preferencesFile;
  final DesktopApps? applications;
  final bool nativeViews;
  final ClarionIpc? capabilityIpc;

  @override
  State<DesktopApplication> createState() => _DesktopApplicationState();
}

class _DesktopApplicationState extends State<DesktopApplication>
    with WidgetsBindingObserver {
  static const _window = MethodChannel('clarion.desktop/window');
  late ShellPreferences _preferences;
  late String _outputName;
  List<DesktopApp> _apps = const [];
  StreamSubscription<FileSystemEvent>? _preferenceWatcher;
  Timer? _preferenceDebounce;
  Future<void> _nativeWork = Future.value();
  Future<void> _preferenceWork = Future.value();
  bool _interaction = false;
  int? _mainViewId;
  int? _interactionViewId;
  final _interactionNavigator = GlobalKey<NavigatorState>();
  Object? _interactionFailure;
  bool _inlineRenameFromMenu = false;
  ClarionIpc? _capabilityIpc;

  @override
  void initState() {
    super.initState();
    _preferences = widget.preferences;
    _outputName = widget.outputName;
    _inlineRenameFromMenu = !widget.nativeViews;
    if (widget.nativeViews) unawaited(_probeMenuFocusRestore());
    if (widget.nativeViews) {
      final dispatcher = WidgetsBinding.instance.platformDispatcher;
      _mainViewId =
          dispatcher.implicitView?.viewId ??
          dispatcher.views.firstOrNull?.viewId;
      WidgetsBinding.instance.addObserver(this);
    }
    _window.setMethodCallHandler((call) async {
      if (mounted && call.method == 'outputChanged') {
        await _refreshOutputName();
      } else if (mounted && call.method == 'interactionViewFailed') {
        final failedId = call.arguments is Map
            ? (call.arguments as Map)['viewId']
            : null;
        if (failedId is int &&
            _interactionViewId != null &&
            failedId != _interactionViewId) {
          return;
        }
        _interactionFailure = PlatformException(
          code: 'interaction-view-failed',
          message: 'The desktop interaction view closed',
        );
        _interactionNavigator.currentState?.popUntil((route) => route.isFirst);
        setState(() => _interactionViewId = null);
      }
    });
    unawaited(_startService());
    unawaited(widget.layout.load());
    unawaited(_loadApplications());
    unawaited(_refreshOutputName());
    _preferenceWork = _syncNativeStyle(_preferences.shellStyle);
    if (widget.watchPreferences) unawaited(_watchPreferences());
  }

  @override
  void didChangeMetrics() {
    if (mounted) setState(() {});
  }

  Future<void> _waitForInteractionView({bool geometry = false}) async {
    final deadline = DateTime.now().add(const Duration(seconds: 5));
    while (mounted) {
      if (_interactionFailure != null || !_interaction) {
        throw PlatformException(code: 'interaction-cancelled');
      }
      final views = WidgetsBinding.instance.platformDispatcher.views;
      final main = views
          .where((view) => view.viewId == _mainViewId)
          .firstOrNull;
      final popup = views
          .where((view) => view.viewId == _interactionViewId)
          .firstOrNull;
      if (_interactionNavigator.currentState?.overlay != null &&
          (!geometry ||
              main != null &&
                  popup != null &&
                  main.physicalSize / main.devicePixelRatio ==
                      popup.physicalSize / popup.devicePixelRatio)) {
        return;
      }
      if (DateTime.now().isAfter(deadline)) {
        throw TimeoutException('Desktop interaction view did not become ready');
      }
      WidgetsBinding.instance.scheduleFrame();
      await WidgetsBinding.instance.endOfFrame;
    }
  }

  Future<void> _startService() async {
    try {
      await widget.service.start();
    } on Object catch (error) {
      stderr.writeln('Clarion desktop directory: $error');
    }
  }

  Future<void> _probeMenuFocusRestore() async {
    final ipc = _capabilityIpc =
        widget.capabilityIpc ?? ClarionIpc(timeout: const Duration(seconds: 1));
    try {
      final supported = await desktopMenuFocusRestoreSupported(ipc);
      if (mounted && supported) setState(() => _inlineRenameFromMenu = true);
    } finally {
      ipc.dispose();
      if (identical(_capabilityIpc, ipc)) _capabilityIpc = null;
    }
  }

  Future<void> _loadApplications() async {
    try {
      final apps = await (widget.applications ?? DesktopApps()).load();
      if (mounted) setState(() => _apps = apps);
    } on Object catch (error) {
      stderr.writeln('Clarion desktop applications: $error');
    }
  }

  Future<void> _refreshOutputName() async {
    if (widget.preview) return;
    try {
      final name = await _window.invokeMethod<String>('outputName');
      if (mounted && name != null && name.isNotEmpty && name != _outputName) {
        setState(() => _outputName = name);
      }
    } on MissingPluginException {
      // A desktop preview or widget test has no native output binding.
    } on PlatformException catch (error) {
      stderr.writeln('Clarion desktop output: ${error.message}');
    }
  }

  Future<void> _syncNativeStyle(ShellStyle style) async {
    if (widget.preview) return;
    try {
      await _window.invokeMethod<void>('setShellStyle', style.name);
    } on MissingPluginException {
      // Older runners and previews retain compositor bindings by default.
    } on PlatformException catch (error) {
      stderr.writeln('Clarion desktop shortcut style: ${error.message}');
    }
  }

  void _interactionChanged(bool open) {
    if (!mounted || _interaction == open) return;
    _interaction = open;
    _nativeWork = _nativeWork.then((_) async {
      if (!mounted || widget.preview) return;
      if (open) _interactionFailure = null;
      try {
        if (open && widget.nativeViews) {
          final state = await _window.invokeMapMethod<String, dynamic>(
            'ensureInteractionView',
          );
          final id = state?['viewId'];
          if (id is! int) {
            throw const FormatException('Missing desktop interaction view');
          }
          if (!mounted) return;
          if (_interactionViewId != id) setState(() => _interactionViewId = id);
          await _waitForInteractionView();
        }
        if (!mounted) return;
        if (open && !_interaction) return;
        await _window.invokeMethod<void>('setOverlay', open);
        if (open && widget.nativeViews) {
          await _waitForInteractionView(geometry: true);
        }
      } on MissingPluginException {
        // Native focus is not necessary for an ordinary preview window.
        if (widget.nativeViews) {
          _interactionFailure = PlatformException(
            code: 'interaction-plugin-unavailable',
          );
        }
      } on PlatformException catch (error) {
        stderr.writeln('Clarion desktop interaction: ${error.message}');
        _interactionFailure = error;
      } on Object catch (error) {
        stderr.writeln('Clarion desktop interaction: $error');
        _interactionFailure = error;
      }
      if (open && _interactionFailure != null) {
        try {
          await _window.invokeMethod<void>('setOverlay', false);
        } on Object {
          // An output may already have disappeared. The base stays Bottom.
        }
      }
    });
  }

  Future<void> _prepareInteraction() async {
    await _nativeWork;
    if (_interactionFailure != null) {
      throw PlatformException(
        code: 'interaction-unavailable',
        message: 'The desktop interaction surface is unavailable',
      );
    }
  }

  Future<void> _watchPreferences() async {
    final file = widget.preferencesFile ?? ShellPreferences.file;
    if (file == null) return;
    var directory = file.parent;
    while (!await directory.exists()) {
      final parent = directory.parent;
      if (parent.path == directory.path) return;
      directory = parent;
    }
    if (!mounted) return;
    await _preferenceWatcher?.cancel();
    if (!mounted) return;
    try {
      _preferenceWatcher = directory.watch().listen(
        (_) {
          _preferenceDebounce?.cancel();
          _preferenceDebounce = Timer(const Duration(milliseconds: 120), () {
            _preferenceWork = _preferenceWork.then((_) async {
              if (!mounted) return;
              final preferences = await ShellPreferences.load(source: file);
              if (!mounted) return;
              if (_preferences.shellStyle != preferences.shellStyle) {
                await _syncNativeStyle(preferences.shellStyle);
                if (!mounted) return;
              }
              if (_preferences.themeMode != preferences.themeMode ||
                  _preferences.reduceMotion != preferences.reduceMotion ||
                  _preferences.barPosition != preferences.barPosition ||
                  _preferences.shellStyle != preferences.shellStyle ||
                  _preferences.macShell != preferences.macShell ||
                  _preferences.seed != preferences.seed) {
                setState(() => _preferences = preferences);
              }
              // When a previously absent config directory appears, watch it
              // directly so later atomic file replacements are observed.
              if (directory.path != file.parent.path &&
                  await file.parent.exists()) {
                await _watchPreferences();
              }
            });
          });
        },
        onError: (Object error) {
          stderr.writeln('Clarion desktop preferences: $error');
        },
      );
    } on FileSystemException catch (error) {
      stderr.writeln('Clarion desktop preferences: $error');
    }
  }

  @override
  void dispose() {
    _capabilityIpc?.dispose();
    _capabilityIpc = null;
    _interaction = false;
    if (!widget.preview) {
      // A native open reply can arrive after this state is removed. Close after
      // queued work completes so it cannot leave an exclusive keyboard grab.
      unawaited(
        _nativeWork.then((_) async {
          try {
            await _window.invokeMethod<void>('setOverlay', false);
          } on Object {
            // Output removal may already have destroyed the native window.
          }
        }),
      );
    }
    _window.setMethodCallHandler(null);
    if (widget.nativeViews) WidgetsBinding.instance.removeObserver(this);
    _preferenceDebounce?.cancel();
    unawaited(_preferenceWatcher?.cancel());
    widget.service.dispose();
    widget.layout.dispose();
    super.dispose();
  }

  Widget _application({
    required Widget home,
    GlobalKey<NavigatorState>? navigatorKey,
  }) => MaterialApp(
    navigatorKey: navigatorKey,
    debugShowCheckedModeBanner: false,
    title: 'Clarion 桌面',
    supportedLocales: const [Locale('zh'), Locale('en')],
    localizationsDelegates: GlobalMaterialLocalizations.delegates,
    theme: _preferences.shellStyle == ShellStyle.macos
        ? buildMacShellTheme(Brightness.light)
        : buildShellTheme(Brightness.light, seed: _preferences.seed),
    darkTheme: _preferences.shellStyle == ShellStyle.macos
        ? buildMacShellTheme(Brightness.dark)
        : buildShellTheme(Brightness.dark, seed: _preferences.seed),
    themeMode: _preferences.themeMode,
    builder: (context, child) => MediaQuery(
      data: MediaQuery.of(context).copyWith(
        disableAnimations:
            _preferences.reduceMotion ||
            MediaQuery.of(context).disableAnimations,
      ),
      child: child!,
    ),
    home: home,
  );

  Widget _desktop() => _application(
    home: DesktopView(
      service: widget.service,
      layout: widget.layout,
      outputName: _outputName,
      barPosition: _preferences.barPosition,
      shellStyle: _preferences.shellStyle,
      macPreferences: _preferences.macShell,
      reduceMotion: _preferences.reduceMotion,
      apps: _apps,
      onInteractionChanged: _interactionChanged,
      prepareInteraction: _prepareInteraction,
      interactionNavigator: widget.nativeViews ? _interactionNavigator : null,
      inlineRenameFromMenu: _inlineRenameFromMenu,
    ),
  );

  @override
  Widget build(BuildContext context) {
    if (!widget.nativeViews) return _desktop();
    final views = WidgetsBinding.instance.platformDispatcher.views.toList();
    _mainViewId ??= views.firstOrNull?.viewId;
    return ViewCollection(
      views: [
        for (final view in views)
          View(
            key: ValueKey('desktop-native-view-${view.viewId}'),
            view: view,
            child: view.viewId == _mainViewId
                ? _desktop()
                : _application(
                    navigatorKey: view.viewId == _interactionViewId
                        ? _interactionNavigator
                        : null,
                    home: const CustomPaint(
                      key: ValueKey('desktop-interaction-clear'),
                      painter: _InteractionClearPainter(),
                      child: Material(
                        type: MaterialType.transparency,
                        child: SizedBox.expand(),
                      ),
                    ),
                  ),
          ),
      ],
    );
  }
}

class _InteractionClearPainter extends CustomPainter {
  const _InteractionClearPainter();

  @override
  void paint(Canvas canvas, Size size) =>
      canvas.drawRect(Offset.zero & size, Paint()..blendMode = BlendMode.clear);

  @override
  bool shouldRepaint(_InteractionClearPainter oldDelegate) => false;
}
