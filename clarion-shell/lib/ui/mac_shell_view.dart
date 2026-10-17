// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:math' as math;
import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';

import '../models.dart';
import '../app_menu_models.dart';
import '../mac_shell_preferences.dart';
import '../state/shell_controller.dart';
import '../theme/shell_theme.dart';
import 'controller_selector.dart';
import 'launcher_surface.dart';
import 'retained_surface.dart';
import 'mac_dock_view.dart';
import 'optional_asset_icon.dart';
import 'mac_dock_app_menu.dart';
import 'mac_dock_context_menu.dart';
import 'mac_dock_panel_geometry.dart';
import 'mac_downloads_stack_panel.dart';
import 'mac_spotlight_panel.dart';
import 'mac_mission_control_panel.dart';
import 'global_app_menu_bar.dart';
import 'shell_panel_content.dart';
import 'mac_volume_menu.dart';
import 'mac_force_quit_dialog.dart';
import 'mac_control_center.dart';
import 'mac_notification_center.dart';
import 'tray_icon.dart';

/// The menu bar occupies its own native strip; panels expand the main surface.
class MacShellView extends StatefulWidget {
  const MacShellView({
    super.key,
    required this.controller,
    this.preview = false,
    this.availableWidth,
    this.onLauncherSurfaceVisible,
    this.onNotificationSurfaceVisible,
    this.onPanelGlassChanged,
    this.onControlSurfaceVisible,
  });
  final ShellController controller;
  final bool preview;
  final double? availableWidth;
  final ValueChanged<bool>? onLauncherSurfaceVisible;
  final ValueChanged<bool>? onNotificationSurfaceVisible;
  final ValueChanged<Rect?>? onPanelGlassChanged;
  final ValueChanged<bool>? onControlSurfaceVisible;
  static const double barHeight = 22;

  @override
  State<MacShellView> createState() => _MacShellViewState();
}

class _MacShellViewState extends State<MacShellView> {
  final _volumeRight = ValueNotifier<double?>(null);
  bool _controlPreferences = false;
  bool _controlRetained = false;
  Rect? _panelGlass;
  final _surface = GlobalKey();
  bool _launcherRetained = false;
  bool _launcherRebuildQueued = false;
  ShellController get controller => widget.controller;
  bool get preview => widget.preview;
  double? get availableWidth => widget.availableWidth;
  double get barHeight => controller.macPreferences.menuBarHeight;

  void _volumeAnchorChanged(Rect rect) {
    final surface = _surface.currentContext?.findRenderObject();
    if (mounted && surface is RenderBox) {
      _volumeRight.value = surface.globalToLocal(rect.topRight).dx;
    }
  }

  double? _appMenuLeft;

  void _appMenuAnchorChanged(Rect rect) {
    final surface = _surface.currentContext?.findRenderObject();
    if (mounted && surface is RenderBox) {
      final left = surface.globalToLocal(rect.topLeft).dx;
      if (_appMenuLeft != left) {
        setState(() => _appMenuLeft = left);
      }
    }
  }

  void _controlPreferencesChanged(bool visible) {
    if (!mounted ||
        controller.panel != ShellPanel.quickSettings ||
        _controlPreferences == visible) {
      return;
    }
    setState(() => _controlPreferences = visible);
  }

  // The application owns native state and serializes geometry with expansion.
  // A view only reports layout; it must never run a second native work queue.
  void _queuePanelGlass(Rect? region) {
    if (preview || _panelGlass == region) return;
    _panelGlass = region;
    widget.onPanelGlassChanged?.call(region);
  }

  void _panelGlassChanged(Rect bounds) {
    if (!mounted || !_controlRetained) return;
    final surface = _surface.currentContext?.findRenderObject();
    if (surface is RenderBox) {
      _queuePanelGlass(surface.globalToLocal(bounds.topLeft) & bounds.size);
    }
  }

  void _controlSurfaceChanged(bool visible) {
    _controlRetained = visible;
    if (!visible) {
      _controlPreferences = false;
      _queuePanelGlass(null);
    }
    widget.onControlSurfaceVisible?.call(visible);
  }

  @override
  void dispose() {
    if (!preview && _panelGlass != null) widget.onPanelGlassChanged?.call(null);
    _volumeRight.dispose();
    super.dispose();
  }

  void _launcherSurfaceChanged(bool visible) {
    widget.onLauncherSurfaceVisible?.call(visible);
    if (!mounted || _launcherRetained == visible) return;
    _launcherRetained = visible;
    // The initial retained frame is reported from the child's initState.
    // Rebuild after layout instead of marking an ancestor dirty during build.
    if (_launcherRebuildQueued) return;
    _launcherRebuildQueued = true;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _launcherRebuildQueued = false;
      if (mounted) setState(() {});
    });
  }

  @override
  Widget build(BuildContext context) => ControllerSelector(
    controller: controller,
    select: () => (
      panel: controller.panel,
      panelRevision: controller.panelRevision,
      selectedDockAppId: controller.selectedDockAppId,
      reduceMotion: controller.reduceMotion,
      macPreferences: controller.macPreferences,
      macMenuBarVisible: controller.macMenuBarVisible,
      tooltip: controller.barTooltip,
      dockMenuAnchor: controller.dockMenuAnchor,
      dockFolderCount: controller.dockFolder.entries.length,
      dockFolderEntries: controller.dockFolder.entries,
    ),
    builder: (context, _) {
      if (controller.panel != ShellPanel.quickSettings && !_controlRetained) {
        _controlPreferences = false;
        _queuePanelGlass(null);
      }
      final bar = _MacMenuBar(
        controller: controller,
        volumeAnchorChanged: _volumeAnchorChanged,
        appMenuAnchorChanged: _appMenuAnchorChanged,
      );
      final reduced =
          controller.reduceMotion || MediaQuery.disableAnimationsOf(context);
      final colors = Theme.of(context).colorScheme;
      return CallbackShortcuts(
        bindings: {
          const SingleActivator(LogicalKeyboardKey.escape):
              controller.closePanel,
          const SingleActivator(LogicalKeyboardKey.space, meta: true): () =>
              controller.selectPanel(ShellPanel.spotlight),
          const SingleActivator(LogicalKeyboardKey.arrowUp, control: true):
              controller.toggleSpaces,
          const SingleActivator(LogicalKeyboardKey.arrowUp, meta: true):
              controller.toggleSpaces,
          const SingleActivator(
            LogicalKeyboardKey.escape,
            meta: true,
            alt: true,
          ): () =>
              showForceQuitDialog(context: context, controller: controller),
          const SingleActivator(LogicalKeyboardKey.keyH, meta: true): () {
            final focused = controller.menuFocusedWindow;
            if (focused != null && focused.appId.isNotEmpty) {
              unawaited(controller.hideApp(focused.appId));
            }
          },
          const SingleActivator(
            LogicalKeyboardKey.keyH,
            meta: true,
            alt: true,
          ): () {
            final focused = controller.menuFocusedWindow;
            if (focused != null && focused.appId.isNotEmpty) {
              unawaited(controller.hideOthers(focused.appId));
            }
          },
          const SingleActivator(LogicalKeyboardKey.keyQ, meta: true): () {
            final focused = controller.menuFocusedWindow;
            if (focused != null && focused.appId.isNotEmpty) {
              unawaited(controller.closeAppWindows(focused.appId));
            }
          },
          const SingleActivator(LogicalKeyboardKey.comma, meta: true): () {
            controller.openSettings();
          },
        },
        child: Focus(
          autofocus:
              controller.panel != ShellPanel.launcher &&
              controller.panel != ShellPanel.spotlight &&
              controller.panel != ShellPanel.spaces &&
              controller.panel != ShellPanel.wallpapers,
          child: LayoutBuilder(
            builder: (context, constraints) {
              final width = constraints.maxWidth;
              final height = constraints.maxHeight;
              final panel = controller.panel;
              final launcherScene =
                  panel == ShellPanel.launcher ||
                  (_launcherRetained && panel == ShellPanel.none);
              final open = panel != ShellPanel.none;
              final tooltip = controller.barTooltip;
              final dockMenu =
                  panel == ShellPanel.dockAppMenu ||
                  panel == ShellPanel.dockContextMenu ||
                  panel == ShellPanel.dockTrashMenu ||
                  panel == ShellPanel.dockFinderMenu ||
                  panel == ShellPanel.dockDownloadsMenu;
              final dockStack = panel == ShellPanel.dockDownloads;
              final dockPanel = dockMenu || dockStack;
              final dockPosition = controller.macPreferences.dockPosition;
              final menuWidth = math.min(
                width,
                (availableWidth ?? width) *
                    controller.macPreferences.menuBarWidthPercent /
                    100,
              );
              final menuInset = (width - menuWidth) / 2;
              final wide = panel == ShellPanel.wallpapers;
              final panelWidth = math.min(
                wide
                    ? 880.0
                    : dockStack
                    ? 540.0
                    : panel == ShellPanel.dockTrashMenu
                    ? 220.0
                    : panel == ShellPanel.dockFinderMenu
                    ? 240.0
                    : panel == ShellPanel.dockDownloadsMenu
                    ? 240.0
                    : dockMenu
                    ? 280.0
                    : panel == ShellPanel.systemMenu
                    ? 280.0
                    : panel == ShellPanel.appMenu
                    ? 240.0
                    : 420.0,
                math.max(0.0, width - 24),
              );
              final dockThickness = MacDockView.surfaceThickness(
                controller.macPreferences,
              );
              final dockRect = macDockPanelGeometry(
                output: Size(width, height),
                position: dockPosition,
                width: panelWidth,
                height: dockMenu
                    ? (panel == ShellPanel.dockContextMenu
                          ? MacDockContextMenu.estimatedHeight()
                          : panel == ShellPanel.dockTrashMenu
                          ? MacDockTrashMenu.estimatedHeight()
                          : panel == ShellPanel.dockFinderMenu
                          ? MacDockFinderMenu.estimatedHeight(controller)
                          : panel == ShellPanel.dockDownloadsMenu
                          ? MacDockDownloadsMenu.estimatedHeight()
                          : MacDockAppMenu.estimatedHeight(controller))
                    : 500,
                menuBarHeight: barHeight,
                dockThickness: dockThickness,
                anchor: controller.dockMenuAnchor,
              );
              final panelBottom =
                  dockPanel && dockPosition == MacDockPosition.bottom
                  ? height - dockRect.bottom
                  : 12.0;
              final fanStack =
                  dockStack &&
                  resolveDownloadsView(
                        requested: controller.macPreferences.downloadsView,
                        dockPosition: dockPosition,
                        available: dockRect.size,
                        entryCount: controller.dockFolder.entries.length,
                      ) ==
                      MacDownloadsView.fan;
              final menu =
                  panel == ShellPanel.systemMenu ||
                  panel == ShellPanel.appMenu ||
                  dockMenu;
              final maxPanelHeight = math.max(
                0.0,
                dockPanel
                    ? dockRect.height
                    : height - barHeight - 4 - panelBottom,
              );
              return Stack(
                key: _surface,
                fit: StackFit.expand,
                children: [
                  if (preview)
                    DecoratedBox(
                      decoration: BoxDecoration(
                        gradient: LinearGradient(
                          begin: Alignment.topLeft,
                          end: Alignment.bottomRight,
                          colors:
                              Theme.of(context).brightness == Brightness.dark
                              ? const [
                                  Color(0xff39374f),
                                  Color(0xff5d647b),
                                  Color(0xffb79c98),
                                ]
                              : const [
                                  Color(0xffb9c9d5),
                                  Color(0xffddd7df),
                                  Color(0xffedc6b6),
                                ],
                        ),
                      ),
                    ),
                  if (open &&
                      panel != ShellPanel.launcher &&
                      panel != ShellPanel.spotlight &&
                      panel != ShellPanel.spaces &&
                      height > barHeight)
                    Positioned.fill(
                      key: const ValueKey('mac-panel-scrim'),
                      top: barHeight,
                      child: Semantics(
                        label: '关闭面板',
                        button: true,
                        child: GestureDetector(
                          behavior: HitTestBehavior.opaque,
                          onTap: controller.closePanel,
                          child: ColoredBox(
                            color: colors.scrim.withValues(
                              alpha:
                                  dockPanel ||
                                      panel == ShellPanel.volume ||
                                      panel == ShellPanel.quickSettings ||
                                      panel == ShellPanel.notifications ||
                                      panel == ShellPanel.systemMenu ||
                                      panel == ShellPanel.appMenu
                                  ? 0
                                  : 0.10,
                            ),
                          ),
                        ),
                      ),
                    ),
                  Positioned.fill(
                    key: const ValueKey('mac-launcher-position'),
                    top: launcherScene ? 0 : barHeight,
                    child: LauncherSurface(
                      controller: controller,
                      visible: panel == ShellPanel.launcher,
                      reducedMotion: reduced,
                      onSurfaceVisible: _launcherSurfaceChanged,
                    ),
                  ),
                  Positioned(
                    key: const ValueKey('mac-menu-bar-position'),
                    top: 0,
                    left: menuInset,
                    width: menuWidth,
                    height: barHeight,
                    child: IgnorePointer(
                      ignoring: launcherScene,
                      child: Opacity(
                        key: const ValueKey(
                          'mac-menu-bar-launchpad-visibility',
                        ),
                        opacity: launcherScene ? 0 : 1,
                        child: _MacMenuReveal(
                          controller: controller,
                          child: RepaintBoundary(child: bar),
                        ),
                      ),
                    ),
                  ),
                  if (!open && tooltip != null && height > barHeight + 8)
                    Positioned(
                      key: const ValueKey('mac-bar-tooltip-position'),
                      top: barHeight + 8,
                      left: 0,
                      right: 0,
                      height: math.min(76, height - barHeight - 8),
                      child: IgnorePointer(
                        child: ExcludeSemantics(
                          child: CustomSingleChildLayout(
                            delegate: _MacTooltipLayout(tooltip.anchor.dx),
                            child: Material(
                              key: const ValueKey('bar-tooltip-bubble'),
                              color: colors.inverseSurface,
                              borderRadius: BorderRadius.circular(8),
                              child: Padding(
                                padding: const EdgeInsets.symmetric(
                                  horizontal: 10,
                                  vertical: 6,
                                ),
                                child: Text(
                                  tooltip.message,
                                  maxLines: 3,
                                  overflow: TextOverflow.ellipsis,
                                  style: Theme.of(context).textTheme.bodySmall
                                      ?.copyWith(
                                        color: colors.onInverseSurface,
                                      ),
                                ),
                              ),
                            ),
                          ),
                        ),
                      ),
                    ),
                  if (open &&
                      panel != ShellPanel.launcher &&
                      panel != ShellPanel.spotlight &&
                      panel != ShellPanel.spaces &&
                      panel != ShellPanel.volume &&
                      panel != ShellPanel.quickSettings &&
                      panel != ShellPanel.notifications &&
                      maxPanelHeight > 8)
                    Positioned(
                      key: const ValueKey('mac-panel-position'),
                      top: dockPanel && dockPosition != MacDockPosition.bottom
                          ? dockRect.top
                          : dockPanel
                          ? null
                          : barHeight + (menu ? 4 : 8),
                      bottom:
                          dockPanel && dockPosition == MacDockPosition.bottom
                          ? panelBottom
                          : null,
                      left: dockPanel || wide
                          ? dockPanel
                                ? dockRect.left
                                : (width - panelWidth) / 2
                          : panel == ShellPanel.systemMenu
                          ? (menuInset + 12).clamp(
                              0.0,
                              math.max(0.0, width - panelWidth),
                            )
                          : panel == ShellPanel.appMenu
                          ? (_appMenuLeft ?? (menuInset + 40)).clamp(
                              0.0,
                              math.max(0.0, width - panelWidth),
                            )
                          : null,
                      right:
                          dockPanel ||
                              wide ||
                              panel == ShellPanel.systemMenu ||
                              panel == ShellPanel.appMenu
                          ? null
                          : (menuInset + 12).clamp(
                              0.0,
                              math.max(0.0, width - panelWidth),
                            ),
                      width: panelWidth,
                      child: ConstrainedBox(
                        constraints: BoxConstraints(maxHeight: maxPanelHeight),
                        child: TweenAnimationBuilder<double>(
                          key: ValueKey(panel),
                          tween: Tween(begin: fanStack ? 1 : 0, end: 1),
                          duration: ShellMotion.duration(
                            reduced || fanStack,
                            Duration(milliseconds: menu ? 60 : 180),
                          ),
                          curve: Curves.easeOutCubic,
                          builder: (context, value, child) => Opacity(
                            opacity: value,
                            child: Transform.translate(
                              offset: menu || fanStack
                                  ? Offset.zero
                                  : Offset(0, (1 - value) * -6),
                              child: child,
                            ),
                          ),
                          child: dockStack
                              ? FocusTraversalGroup(
                                  child: ShellPanelContent(
                                    controller: controller,
                                    downloadsFanAnchorX:
                                        controller.dockMenuAnchor == null
                                        ? null
                                        : controller.dockMenuAnchor!.center.dx -
                                              dockRect.left,
                                    downloadsFanAnchorYBelowPanel:
                                        controller.dockMenuAnchor == null
                                        ? null
                                        : controller.dockMenuAnchor!.center.dy -
                                              dockRect.bottom,
                                    downloadsFanOriginIconSize:
                                        controller.dockMenuAnchor?.width,
                                  ),
                                )
                              : _MacGlassPanel(
                                  menu: menu,
                                  child: FocusTraversalGroup(
                                    child: ShellPanelContent(
                                      controller: controller,
                                      preview: preview,
                                    ),
                                  ),
                                ),
                        ),
                      ),
                    ),
                  Positioned(
                    key: const ValueKey('mac-notification-position'),
                    top: barHeight,
                    right: 0,
                    bottom: 0,
                    // The archived notification content/header views are 344
                    // points wide; runtime scale/material still need reference.
                    width: math.min(344.0, width),
                    child: MacNotificationSurface(
                      controller: controller,
                      visible: panel == ShellPanel.notifications,
                      reducedMotion: reduced,
                      onSurfaceVisible: widget.onNotificationSurfaceVisible,
                    ),
                  ),
                  if (panel == ShellPanel.volume && height > barHeight + 8)
                    Positioned.fill(
                      child: CustomSingleChildLayout(
                        delegate: _MacAnchoredPanelLayout(
                          right: _volumeRight,
                          barHeight: barHeight,
                          width: MacVolumeMenu.width,
                        ),
                        child: _MacGlassPanel(
                          key: const ValueKey('mac-volume-glass'),
                          menu: true,
                          child: FocusTraversalGroup(
                            child: MacVolumeMenu(controller: controller),
                          ),
                        ),
                      ),
                    ),
                  if (height > barHeight + 8)
                    Positioned.fill(
                      key: const ValueKey('mac-control-center-position'),
                      child: CustomSingleChildLayout(
                        delegate: _MacAnchoredPanelLayout(
                          edgeRight: width - menuInset - 12,
                          centered: _controlPreferences,
                          barHeight: barHeight,
                          width: _controlPreferences
                              ? MacControlCenter.preferencesWidth
                              : MacControlCenter.width,
                          heightLimit: _controlPreferences ? 720 : null,
                          gap: 6,
                        ),
                        child: _SurfaceAnchor(
                          onChanged: _panelGlassChanged,
                          child: RetainedSurface(
                            visible: panel == ShellPanel.quickSettings,
                            // Switching to another panel must release glass in
                            // the same frame, without fading over its contents.
                            reducedMotion:
                                reduced ||
                                (panel != ShellPanel.none &&
                                    panel != ShellPanel.quickSettings),
                            duration: const Duration(milliseconds: 140),
                            reverseDuration: const Duration(milliseconds: 100),
                            onSurfaceVisible: _controlSurfaceChanged,
                            transitionBuilder: (context, value, child) =>
                                Opacity(
                                  opacity: value,
                                  child: Transform.translate(
                                    offset: Offset(0, (1 - value) * -2),
                                    child: child,
                                  ),
                                ),
                            builder: (context, constraints) => RepaintBoundary(
                              key: const ValueKey('mac-control-center-capture'),
                              child: _MacGlassPanel(
                                key: const ValueKey('mac-control-center-glass'),
                                controlCenter: true,
                                child: FocusTraversalGroup(
                                  child: MacControlCenter(
                                    controller: controller,
                                    onPreferencesChanged:
                                        _controlPreferencesChanged,
                                  ),
                                ),
                              ),
                            ),
                          ),
                        ),
                      ),
                    ),
                  if (panel == ShellPanel.spotlight)
                    Positioned.fill(
                      key: const ValueKey('mac-spotlight-overlay'),
                      top: barHeight,
                      child: RepaintBoundary(
                        child: MacSpotlightPanel(
                          controller: controller,
                          onClose: controller.closePanel,
                        ),
                      ),
                    ),
                  if (panel == ShellPanel.spaces)
                    Positioned.fill(
                      key: const ValueKey('mac-spaces-overlay'),
                      top: barHeight,
                      child: RepaintBoundary(
                        child: MacMissionControlPanel(
                          controller: controller,
                          onClose: controller.closePanel,
                        ),
                      ),
                    ),
                ],
              );
            },
          ),
        ),
      );
    },
  );
}

/// The native surface keeps a two-pixel edge sensor when this strip slides out.
class _MacMenuReveal extends StatefulWidget {
  const _MacMenuReveal({required this.controller, required this.child});
  final ShellController controller;
  final Widget child;
  @override
  State<_MacMenuReveal> createState() => _MacMenuRevealState();
}

class _MacMenuRevealState extends State<_MacMenuReveal> {
  Timer? _hideTimer;
  bool _hovered = false;
  bool _lastAutoHide = false;
  bool _lastOpen = false;
  bool _lastVisible = true;

  bool get _open =>
      widget.controller.panel != ShellPanel.none ||
      widget.controller.appMenuOpen;

  @override
  void initState() {
    super.initState();
    _lastAutoHide = widget.controller.macPreferences.menuBarAutoHide;
    _lastOpen = _open;
    _lastVisible = widget.controller.macMenuBarVisible;
    widget.controller.addListener(_changed);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _sync();
    });
  }

  void _changed() {
    final autoHide = widget.controller.macPreferences.menuBarAutoHide;
    final open = _open;
    final visible = widget.controller.macMenuBarVisible;
    if (_lastAutoHide == autoHide &&
        _lastOpen == open &&
        _lastVisible == visible) {
      return;
    }
    final needSync = _lastAutoHide != autoHide || _lastOpen != open;
    _lastAutoHide = autoHide;
    _lastOpen = open;
    _lastVisible = visible;
    if (needSync) _sync();
    if (mounted) setState(() {});
  }

  void _sync() {
    if (!widget.controller.macPreferences.menuBarAutoHide ||
        _open ||
        _hovered) {
      _hideTimer?.cancel();
      widget.controller.setMacMenuBarVisible(true);
    } else {
      _hideLater();
    }
  }

  void _enter() {
    _hovered = true;
    _hideTimer?.cancel();
    widget.controller.setMacMenuBarVisible(true);
  }

  void _hideLater() {
    _hideTimer?.cancel();
    if (!widget.controller.macPreferences.menuBarAutoHide ||
        _open ||
        _hovered) {
      return;
    }
    _hideTimer = Timer(const Duration(milliseconds: 350), () {
      if (mounted &&
          !_hovered &&
          !_open &&
          widget.controller.macPreferences.menuBarAutoHide) {
        widget.controller.setMacMenuBarVisible(false);
      }
    });
  }

  @override
  void dispose() {
    _hideTimer?.cancel();
    widget.controller.removeListener(_changed);
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => MouseRegion(
    onEnter: (_) => _enter(),
    onExit: (_) {
      _hovered = false;
      _hideLater();
    },
    child: Stack(
      clipBehavior: Clip.hardEdge,
      children: [
        Positioned.fill(
          child: AnimatedSlide(
            offset: widget.controller.macMenuBarVisible || _open
                ? Offset.zero
                : const Offset(0, -1),
            duration:
                widget.controller.reduceMotion ||
                    MediaQuery.disableAnimationsOf(context)
                ? Duration.zero
                : const Duration(milliseconds: 180),
            curve: Curves.easeOutCubic,
            child: widget.child,
          ),
        ),
        if (widget.controller.macPreferences.menuBarAutoHide &&
            !widget.controller.macMenuBarVisible)
          Positioned(
            left: 0,
            right: 0,
            top: 0,
            height: 2,
            child: MouseRegion(
              key: const ValueKey('mac-menu-edge-sensor'),
              onEnter: (_) => _enter(),
              child: const SizedBox.expand(),
            ),
          ),
      ],
    ),
  );
}

class _SurfaceAnchor extends SingleChildRenderObjectWidget {
  const _SurfaceAnchor({required this.onChanged, required super.child});
  final ValueChanged<Rect> onChanged;
  @override
  RenderObject createRenderObject(BuildContext context) =>
      _RenderSurfaceAnchor(onChanged);
  @override
  void updateRenderObject(
    BuildContext context,
    _RenderSurfaceAnchor renderObject,
  ) {
    renderObject.onChanged = onChanged;
  }
}

class _RenderSurfaceAnchor extends RenderProxyBox {
  _RenderSurfaceAnchor(this.onChanged);
  ValueChanged<Rect> onChanged;
  bool _pending = false;
  @override
  void paint(PaintingContext context, Offset offset) {
    super.paint(context, offset);
    if (_pending) return;
    _pending = true;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _pending = false;
      if (attached && hasSize) onChanged(localToGlobal(Offset.zero) & size);
    });
  }
}

class _MacAnchoredPanelLayout extends SingleChildLayoutDelegate {
  _MacAnchoredPanelLayout({
    this.right,
    this.edgeRight,
    this.centered = false,
    this.heightLimit,
    required this.barHeight,
    required this.width,
    this.gap = 4,
  }) : super(relayout: right);
  final ValueNotifier<double?>? right;
  final double? edgeRight;
  final bool centered;
  final double? heightLimit;
  final double barHeight;
  final double width;
  final double gap;

  @override
  BoxConstraints getConstraintsForChild(BoxConstraints constraints) =>
      BoxConstraints(
        minWidth: math.min(width, math.max(0, constraints.maxWidth - 24)),
        maxWidth: math.min(width, math.max(0, constraints.maxWidth - 24)),
        maxHeight: math.min(
          heightLimit ?? double.infinity,
          math.max(0, constraints.maxHeight - barHeight - gap - 8),
        ),
      );

  @override
  Offset getPositionForChild(Size size, Size childSize) {
    return Offset(
      (centered
              ? (size.width - childSize.width) / 2
              : (edgeRight ?? right?.value ?? size.width - 12) -
                    childSize.width)
          .clamp(12.0, math.max(12, size.width - childSize.width - 12)),
      centered
          ? ((size.height - childSize.height) / 2).clamp(
              barHeight + gap,
              math.max(barHeight + gap, size.height - childSize.height - 8),
            )
          : barHeight + gap,
    );
  }

  @override
  bool shouldRelayout(_MacAnchoredPanelLayout oldDelegate) =>
      oldDelegate.right != right ||
      oldDelegate.edgeRight != edgeRight ||
      oldDelegate.centered != centered ||
      oldDelegate.heightLimit != heightLimit ||
      oldDelegate.barHeight != barHeight ||
      oldDelegate.width != width ||
      oldDelegate.gap != gap;
}

class _MacGlassPanel extends StatelessWidget {
  const _MacGlassPanel({
    super.key,
    required this.child,
    this.menu = false,
    this.controlCenter = false,
  });
  final Widget child;
  final bool menu;
  final bool controlCenter;

  @override
  Widget build(BuildContext context) {
    final dark = Theme.of(context).brightness == Brightness.dark;
    final radius = BorderRadius.circular(
      menu
          ? 6
          : controlCenter
          ? 16
          : 13,
    );
    return Container(
      decoration: BoxDecoration(
        color: dark
            ? const Color(
                0xff29292d,
              ).withValues(alpha: controlCenter ? .86 : .90)
            : const Color(
                0xfff2f2f5,
              ).withValues(alpha: controlCenter ? .84 : .88),
        borderRadius: radius,
        border: Border.all(
          color: dark
              ? Colors.white.withValues(alpha: 0.15)
              : Colors.black.withValues(alpha: 0.12),
          width: .5,
        ),
        boxShadow: [
          BoxShadow(
            color: Colors.black.withValues(alpha: dark ? .45 : .18),
            blurRadius: 18,
            spreadRadius: 1,
            offset: const Offset(0, 8),
          ),
          BoxShadow(
            color: Colors.black.withValues(alpha: dark ? .30 : .10),
            blurRadius: 4,
            offset: const Offset(0, 1),
          ),
        ],
      ),
      clipBehavior: Clip.antiAlias,
      child: Material(type: MaterialType.transparency, child: child),
    );
  }
}

class _MacMenuBar extends StatelessWidget {
  const _MacMenuBar({
    required this.controller,
    this.volumeAnchorChanged,
    this.appMenuAnchorChanged,
  });
  final ShellController controller;
  final ValueChanged<Rect>? volumeAnchorChanged;
  final ValueChanged<Rect>? appMenuAnchorChanged;

  String get _activeAppName {
    final focused = controller.menuFocusedWindow;
    if (focused == null) return 'Home';
    for (final app in controller.dockApps) {
      if (app.windows.any((window) => window.id == focused.id)) return app.name;
    }
    return focused.appId.isEmpty ? 'Home' : focused.appId;
  }

  static String _weekdayName(int weekday) {
    const names = ['周一', '周二', '周三', '周四', '周五', '周六', '周日'];
    return (weekday >= 1 && weekday <= 7) ? names[weekday - 1] : '';
  }

  List<GlobalMenuItem> _buildFallbackMenus(BuildContext context) {
    final focused = controller.menuFocusedWindow;
    final isFinder = focused == null;

    if (isFinder) {
      return [
        GlobalMenuItem(
          id: 'finder-file',
          label: '文件',
          children: [
            GlobalMenuItem(
              id: 'finder-new-window',
              label: '新建Home窗口',
              shortcut: '⌘N',
              onAction: () => controller.openHomeFolder(),
            ),
            GlobalMenuItem(
              id: 'finder-new-folder',
              label: '新建文件夹',
              shortcut: '⇧⌘N',
              onAction: () => controller.openHomeFolder(),
            ),
            GlobalMenuItem(id: 'finder-file-sep1', label: '', separator: true),
            GlobalMenuItem(
              id: 'finder-quick-look',
              label: '快速查看',
              shortcut: '空格',
              onAction: () => controller.showBarTooltip(
                Object(),
                '按下空格键或 ⌘Y 可快速查看所选项目',
                const Offset(160, 24),
              ),
            ),
            GlobalMenuItem(
              id: 'finder-get-info',
              label: '显示简介',
              shortcut: '⌘I',
              onAction: () => controller.showBarTooltip(
                Object(),
                '按下 ⌘I 可显示当前所选项目简介',
                const Offset(160, 24),
              ),
            ),
            GlobalMenuItem(id: 'finder-file-sep2', label: '', separator: true),
            GlobalMenuItem(
              id: 'finder-empty-trash',
              label: '清倒废纸篓…',
              onAction: () => controller.emptyTrash(),
            ),
          ],
        ),
        GlobalMenuItem(
          id: 'finder-edit',
          label: '编辑',
          children: [
            GlobalMenuItem(
              id: 'finder-undo',
              label: '撤销',
              shortcut: '⌘Z',
              enabled: false,
            ),
            GlobalMenuItem(
              id: 'finder-redo',
              label: '重做',
              shortcut: '⇧⌘Z',
              enabled: false,
            ),
            GlobalMenuItem(id: 'finder-edit-sep', label: '', separator: true),
            GlobalMenuItem(
              id: 'finder-cut',
              label: '剪切',
              shortcut: '⌘X',
              enabled: false,
            ),
            GlobalMenuItem(id: 'finder-copy', label: '拷贝', shortcut: '⌘C'),
            GlobalMenuItem(id: 'finder-paste', label: '粘贴', shortcut: '⌘V'),
            GlobalMenuItem(
              id: 'finder-select-all',
              label: '全选',
              shortcut: '⌘A',
            ),
          ],
        ),
        GlobalMenuItem(
          id: 'finder-view',
          label: '显示',
          children: [
            GlobalMenuItem(
              id: 'finder-use-stacks',
              label: '使用叠放',
              onAction: () => controller.showBarTooltip(
                Object(),
                '在桌面上右键点按可开启或配置叠放功能',
                const Offset(220, 24),
              ),
            ),
            GlobalMenuItem(
              id: 'finder-clean-up',
              label: '整理',
              onAction: () => controller.showBarTooltip(
                Object(),
                '在桌面上右键可执行整理与排序',
                const Offset(220, 24),
              ),
            ),
            GlobalMenuItem(
              id: 'finder-preferences',
              label: '桌面偏好设置…',
              onAction: () => controller.openSettings(),
            ),
          ],
        ),
        GlobalMenuItem(
          id: 'finder-go',
          label: '前往',
          children: [
            GlobalMenuItem(
              id: 'finder-go-home',
              label: '个人',
              shortcut: '⇧⌘H',
              onAction: () => controller.openHomeFolder(),
            ),
            GlobalMenuItem(
              id: 'finder-go-desktop',
              label: '桌面',
              shortcut: '⇧⌘D',
              onAction: () => controller.openDesktopFolder(),
            ),
            GlobalMenuItem(
              id: 'finder-go-apps',
              label: '应用程序',
              shortcut: '⇧⌘A',
              onAction: () => controller.selectPanel(ShellPanel.launcher),
            ),
            GlobalMenuItem(
              id: 'finder-go-downloads',
              label: '下载',
              shortcut: '⌥⌘L',
              onAction: () => controller.openDownloadsFolder(),
            ),
            GlobalMenuItem(
              id: 'finder-go-trash',
              label: '废纸篓',
              onAction: () => controller.openTrash(),
            ),
          ],
        ),
        GlobalMenuItem(
          id: 'finder-window',
          label: '窗口',
          children: [
            GlobalMenuItem(
              id: 'finder-minimize',
              label: '最小化',
              shortcut: '⌘M',
              enabled: false,
            ),
            GlobalMenuItem(
              id: 'finder-spaces',
              label: '调度中心',
              shortcut: 'F3',
              onAction: () => controller.toggleSpaces(),
            ),
            GlobalMenuItem(
              id: 'finder-show-all',
              label: '全部显示',
              onAction: () => controller.showAll(),
            ),
          ],
        ),
        GlobalMenuItem(
          id: 'finder-help',
          label: '帮助',
          children: [
            GlobalMenuItem(
              id: 'finder-help-about',
              label: 'Clarion 帮助',
              onAction: () => controller.openSettings(),
            ),
          ],
        ),
      ];
    } else {
      return [
        GlobalMenuItem(
          id: 'app-file',
          label: '文件',
          children: [
            GlobalMenuItem(
              id: 'app-close-window',
              label: '关闭窗口',
              shortcut: '⌘W',
              onAction: () {
                if (focused.appId.isNotEmpty) {
                  controller.closeAppWindows(focused.appId);
                }
              },
            ),
          ],
        ),
        GlobalMenuItem(
          id: 'app-edit',
          label: '编辑',
          children: [
            GlobalMenuItem(
              id: 'app-undo',
              label: '撤销',
              shortcut: '⌘Z',
              enabled: false,
            ),
            GlobalMenuItem(
              id: 'app-redo',
              label: '重做',
              shortcut: '⇧⌘Z',
              enabled: false,
            ),
            GlobalMenuItem(id: 'app-edit-sep', label: '', separator: true),
            GlobalMenuItem(
              id: 'app-cut',
              label: '剪切',
              shortcut: '⌘X',
              enabled: false,
            ),
            GlobalMenuItem(
              id: 'app-copy',
              label: '复制',
              shortcut: '⌘C',
              enabled: false,
            ),
            GlobalMenuItem(
              id: 'app-paste',
              label: '粘贴',
              shortcut: '⌘V',
              enabled: false,
            ),
            GlobalMenuItem(
              id: 'app-select-all',
              label: '全选',
              shortcut: '⌘A',
              enabled: false,
            ),
          ],
        ),
        GlobalMenuItem(
          id: 'app-window',
          label: '窗口',
          children: [
            GlobalMenuItem(
              id: 'app-minimize',
              label: '最小化',
              shortcut: '⌘M',
              onAction: () => controller.minimizeWindow(focused.id),
            ),
            GlobalMenuItem(
              id: 'app-spaces',
              label: '调度中心',
              shortcut: 'F3',
              onAction: () => controller.toggleSpaces(),
            ),
            GlobalMenuItem(
              id: 'app-show-all',
              label: '全部显示',
              onAction: () => controller.showAll(),
            ),
          ],
        ),
        GlobalMenuItem(
          id: 'app-help',
          label: '帮助',
          children: [
            GlobalMenuItem(
              id: 'app-help-doc',
              label: '帮助',
              onAction: () => controller.openSettings(),
            ),
          ],
        ),
      ];
    }
  }

  @override
  Widget build(BuildContext context) => ControllerSelector(
    controller: controller,
    select: () => (
      desktop: controller.desktopRevision,
      apps: controller.apps,
      day: controller.now.day,
      hour: controller.now.hour,
      minute: controller.now.minute,
      second: controller.macPreferences.clockShowSeconds
          ? controller.now.second
          : null,
      preferences: controller.macPreferences,
      panel: controller.panel,
      appMenuDismiss: controller.appMenuDismissRevision,
      muted: controller.muted,
      battery: controller.batteryFraction,
      charging: controller.charging,
      notifications: controller.notifications.length,
      dnd: controller.doNotDisturb,
      tray: controller.trayItems,
    ),
    builder: (context, _) {
      final menuRevision = controller.appMenuDismissRevision;
      final theme = Theme.of(context);
      final dark = theme.brightness == Brightness.dark;
      return Theme(
        data: theme.copyWith(
          iconButtonTheme: IconButtonThemeData(
            style: IconButton.styleFrom(
              minimumSize: Size(20, controller.macPreferences.menuBarHeight),
              maximumSize: Size(22, controller.macPreferences.menuBarHeight),
              iconSize: 16,
              padding: const EdgeInsets.symmetric(horizontal: 2),
              tapTargetSize: MaterialTapTargetSize.shrinkWrap,
              shape: RoundedRectangleBorder(
                borderRadius: BorderRadius.circular(3),
              ),
            ),
          ),
        ),
        child: Material(
          color: dark
              ? const Color(0xff222225).withValues(alpha: .62)
              : const Color(0xffffffff).withValues(alpha: .60),
          child: DecoratedBox(
            decoration: BoxDecoration(
              border: Border(
                bottom: BorderSide(
                  color: dark
                      ? Colors.white.withValues(alpha: 0.12)
                      : Colors.black.withValues(alpha: 0.14),
                  width: 0.5,
                ),
              ),
            ),
            child: LayoutBuilder(
              builder: (context, constraints) {
                final width = constraints.maxWidth;
                final horizontalPadding = width > 240 ? 10.0 : 7.0;
                final appleButtonWidth = width > 240 ? 28.0 : 22.0;
                final time =
                    '${controller.now.hour.toString().padLeft(2, '0')}:${controller.now.minute.toString().padLeft(2, '0')}'
                    '${controller.macPreferences.clockShowSeconds ? ':${controller.now.second.toString().padLeft(2, '0')}' : ''}';
                final trayLimit = width > 1000
                    ? 4
                    : width > 720
                    ? 2
                    : 0;
                final trays = controller.trayItems.take(trayLimit);
                final hiddenTrays = controller.trayItems.length - trays.length;
                return Padding(
                  padding: EdgeInsets.symmetric(horizontal: horizontalPadding),
                  child: Row(
                    children: [
                      _MacBarButton(
                        buttonKey: 'mac-system-menu',
                        controller: controller,
                        label: 'Clarion 系统菜单',
                        panel: ShellPanel.systemMenu,
                        selected: controller.panel == ShellPanel.systemMenu,
                        width: appleButtonWidth,
                        onPressed: () =>
                            controller.selectPanel(ShellPanel.systemMenu),
                        child: Image.asset(
                          dark
                              ? 'assets/icons/clarion-mark-white.png'
                              : 'assets/icons/clarion-mark-black.png',
                          key: const ValueKey('mac-apple-logo'),
                          width: 17,
                          height: 18,
                          fit: BoxFit.contain,
                          filterQuality: FilterQuality.high,
                          excludeFromSemantics: true,
                          errorBuilder: (_, _, _) => ClarionMark(
                            width: 17,
                            size: 18,
                            color: dark ? Colors.white : Colors.black87,
                          ),
                        ),
                      ),
                      _MacAppMenuButton(
                        controller: controller,
                        appName: _activeAppName,
                        width: math.min(
                          math.min(
                            180.0,
                            math.max(width > 240 ? 60.0 : 40.0, width * .16),
                          ),
                          // Reserve the always-present status buttons and the
                          // overflow tray before giving the app name a minimum.
                          math.max(
                            0,
                            width - 22 * (hiddenTrays > 0 ? 5 : 4) - 8,
                          ),
                        ),
                        selected: controller.panel == ShellPanel.appMenu,
                        anchorChanged: appMenuAnchorChanged,
                      ),
                      Expanded(
                        child: GlobalAppMenuBar(
                          key: ValueKey(controller.appMenuDismissRevision),
                          service: controller.appMenu,
                          fallbackMenus: _buildFallbackMenus(context),
                          barHeight: controller.macPreferences.menuBarHeight,
                          barWidth: constraints.maxWidth,
                          onOpenChanged: (open) {
                            // A closing menu from a previous host revision
                            // cannot collapse a newly opened native surface.
                            if (controller.appMenuDismissRevision ==
                                menuRevision) {
                              controller.setAppMenuOpen(open);
                            }
                          },
                        ),
                      ),
                      if (width > 680)
                        _MacBarButton(
                          buttonKey: 'mac-overview',
                          panel: ShellPanel.spaces,
                          controller: controller,
                          selected: controller.panel == ShellPanel.spaces,
                          label: '调度中心',
                          onPressed: () {
                            if (controller.shellStyle == ShellStyle.macos) {
                              controller.toggleSpaces();
                            } else {
                              controller.toggleOverview();
                            }
                          },
                          // Overlapping panels, matching the Mission Control
                          // silhouette instead of a bare window outline.
                          child: const Icon(Icons.space_dashboard_outlined),
                        ),
                      if (width > 420)
                        _MacBarButton(
                          buttonKey: 'mac-screenshot',
                          panel: ShellPanel.screenshot,
                          controller: controller,
                          label: '截图',
                          selected: controller.panel == ShellPanel.screenshot,
                          onPressed: () =>
                              controller.selectPanel(ShellPanel.screenshot),
                          child: const Icon(Icons.screenshot_monitor_rounded),
                        ),
                      if (width > 620 && controller.batteryFraction != null)
                        ShellBarTooltip(
                          controller: controller,
                          message:
                              '${controller.charging ? '正在充电 · ' : ''}电量 ${(controller.batteryFraction! * 100).round()}%',
                          child: Padding(
                            padding: const EdgeInsets.symmetric(horizontal: 7),
                            child: Row(
                              mainAxisSize: MainAxisSize.min,
                              children: [
                                if (controller
                                    .macPreferences
                                    .showBatteryPercentage) ...[
                                  Text(
                                    '${(controller.batteryFraction! * 100).round()}%',
                                    key: const ValueKey(
                                      'mac-battery-percentage-label',
                                    ),
                                    style: theme.textTheme.labelSmall,
                                  ),
                                  const SizedBox(width: 3),
                                ],
                                Icon(
                                  controller.charging
                                      ? Icons.battery_charging_full_rounded
                                      : Icons.battery_full_rounded,
                                  size: 18,
                                ),
                              ],
                            ),
                          ),
                        ),
                      if (width > 540)
                        _MacBarButton(
                          buttonKey: 'mac-volume',
                          anchorChanged: volumeAnchorChanged,
                          panel: ShellPanel.volume,
                          selected: controller.panel == ShellPanel.volume,
                          controller: controller,
                          label: controller.muted ? '音量已静音' : '音量控制',
                          onPressed: () =>
                              controller.selectPanel(ShellPanel.volume),
                          child: Icon(
                            controller.muted
                                ? Icons.volume_off_rounded
                                : Icons.volume_up_rounded,
                          ),
                        ),
                      for (final item in trays)
                        SizedBox(
                          height: controller.macPreferences.menuBarHeight,
                          child: TrayBarButton(
                            key: ValueKey('mac-tray-${item.id}'),
                            controller: controller,
                            item: item,
                          ),
                        ),
                      if (hiddenTrays > 0)
                        _MacBarButton(
                          buttonKey: 'mac-tray-overflow',
                          panel: ShellPanel.trayItems,
                          controller: controller,
                          label: '系统托盘',
                          selected: controller.panel == ShellPanel.trayItems,
                          onPressed: controller.openTrayItems,
                          child: const Icon(Icons.more_horiz_rounded),
                        ),
                      _MacBarButton(
                        buttonKey: 'mac-spotlight',
                        panel: ShellPanel.spotlight,
                        controller: controller,
                        label: 'Spotlight 搜索',
                        selected: controller.panel == ShellPanel.spotlight,
                        onPressed: () =>
                            controller.selectPanel(ShellPanel.spotlight),
                        child: const Icon(Icons.search_rounded, size: 16),
                      ),
                      _MacBarButton(
                        buttonKey: 'mac-notifications',
                        panel: ShellPanel.notifications,
                        controller: controller,
                        label: '通知中心',
                        selected: controller.panel == ShellPanel.notifications,
                        onPressed: () =>
                            controller.selectPanel(ShellPanel.notifications),
                        child: Badge(
                          isLabelVisible:
                              !controller.doNotDisturb &&
                              controller.notifications.isNotEmpty,
                          smallSize: 5,
                          child: Icon(
                            controller.doNotDisturb
                                ? Icons.notifications_off_outlined
                                : Icons.notifications_outlined,
                          ),
                        ),
                      ),
                      _MacBarButton(
                        buttonKey: 'mac-quick-settings',
                        panel: ShellPanel.quickSettings,
                        controller: controller,
                        label: '控制中心',
                        selected: controller.panel == ShellPanel.quickSettings,
                        onPressed: () =>
                            controller.selectPanel(ShellPanel.quickSettings),
                        child: const _ControlCenterBarIcon(),
                      ),
                      if (width > 320)
                        InkWell(
                          key: const ValueKey('mac-clock-button'),
                          borderRadius: BorderRadius.circular(4),
                          onTap: () =>
                              controller.selectPanel(ShellPanel.notifications),
                          child: Padding(
                            padding: const EdgeInsets.symmetric(
                              horizontal: 6,
                              vertical: 2,
                            ),
                            child: Semantics(
                              label: '当前时间 $time',
                              button: true,
                              child: Text(
                                width > 820 &&
                                        controller.macPreferences.clockShowDate
                                    ? '${controller.now.month}月${controller.now.day}日 ${_weekdayName(controller.now.weekday)}  $time'
                                    : time,
                                key: const ValueKey('mac-clock'),
                                style: theme.textTheme.labelMedium,
                              ),
                            ),
                          ),
                        ),
                    ],
                  ),
                );
              },
            ),
          ),
        ),
      );
    },
  );
}

class _MacBarButton extends StatelessWidget {
  const _MacBarButton({
    required this.buttonKey,
    required this.controller,
    required this.label,
    required this.child,
    required this.onPressed,
    this.selected = false,
    this.panel,
    this.anchorChanged,
    this.width,
  });
  final String buttonKey;
  final ShellController controller;
  final String label;
  final Widget child;
  final VoidCallback onPressed;
  final bool selected;
  final ShellPanel? panel;
  final ValueChanged<Rect>? anchorChanged;
  final double? width;

  @override
  Widget build(BuildContext context) {
    final button = MouseRegion(
      onEnter: (_) {
        if (panel != null &&
            controller.panel != ShellPanel.none &&
            controller.panel != panel) {
          controller.dismissBarTooltip();
          controller.selectPanel(panel!);
        }
      },
      child: ShellBarTooltip(
        controller: controller,
        message: label,
        child: IconButton(
          key: ValueKey(buttonKey),
          isSelected: selected,
          style: IconButton.styleFrom(
            backgroundColor: selected ? const Color(0xff007aff) : null,
            foregroundColor: selected ? Colors.white : null,
            minimumSize: width != null
                ? Size(width!, controller.macPreferences.menuBarHeight)
                : null,
            maximumSize: width != null
                ? Size(width!, controller.macPreferences.menuBarHeight)
                : null,
            padding: width != null ? EdgeInsets.zero : null,
            shape: RoundedRectangleBorder(
              borderRadius: BorderRadius.circular(3.5),
            ),
          ),
          onPressed: () {
            controller.dismissBarTooltip();
            onPressed();
          },
          icon: child,
        ),
      ),
    );
    return anchorChanged == null
        ? button
        : _SurfaceAnchor(onChanged: anchorChanged!, child: button);
  }
}

class _MacAppMenuButton extends StatelessWidget {
  const _MacAppMenuButton({
    required this.controller,
    required this.appName,
    required this.width,
    required this.selected,
    this.anchorChanged,
  });

  final ShellController controller;
  final String appName;
  final double width;
  final bool selected;
  final ValueChanged<Rect>? anchorChanged;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final dark = theme.brightness == Brightness.dark;
    final isSelected = selected;

    final button = MouseRegion(
      onEnter: (_) {
        if (controller.panel != ShellPanel.none &&
            controller.panel != ShellPanel.appMenu) {
          controller.dismissBarTooltip();
          controller.selectPanel(ShellPanel.appMenu);
        }
      },
      child: ShellBarTooltip(
        controller: controller,
        message: '$appName 菜单',
        child: InkWell(
          key: const ValueKey('mac-active-app-button'),
          borderRadius: BorderRadius.circular(3.5),
          onTap: () {
            controller.dismissBarTooltip();
            if (controller.panel == ShellPanel.appMenu) {
              controller.closePanel();
            } else {
              controller.selectPanel(ShellPanel.appMenu);
            }
          },
          child: Container(
            height: controller.macPreferences.menuBarHeight,
            padding: EdgeInsets.symmetric(horizontal: width > 50 ? 9 : 4),
            alignment: Alignment.center,
            decoration: BoxDecoration(
              color: isSelected ? const Color(0xff007aff) : Colors.transparent,
              borderRadius: BorderRadius.circular(3.5),
            ),
            child: ConstrainedBox(
              constraints: BoxConstraints(maxWidth: width),
              child: Text(
                appName,
                key: const ValueKey('mac-active-app'),
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: theme.textTheme.labelLarge?.copyWith(
                  fontWeight: FontWeight.w700,
                  fontSize: 13,
                  color: isSelected
                      ? Colors.white
                      : (dark ? Colors.white : Colors.black87),
                ),
              ),
            ),
          ),
        ),
      ),
    );

    return anchorChanged == null
        ? button
        : _SurfaceAnchor(onChanged: anchorChanged!, child: button);
  }
}

class _MacTooltipLayout extends SingleChildLayoutDelegate {
  const _MacTooltipLayout(this.anchorX);
  final double anchorX;

  @override
  BoxConstraints getConstraintsForChild(BoxConstraints constraints) =>
      BoxConstraints(
        maxWidth: math.min(280, math.max(0, constraints.maxWidth - 16)),
        maxHeight: constraints.maxHeight,
      );

  @override
  Offset getPositionForChild(Size size, Size childSize) => Offset(
    (anchorX - childSize.width / 2)
        .clamp(8.0, math.max(8.0, size.width - childSize.width - 8))
        .toDouble(),
    0,
  );

  @override
  bool shouldRelayout(covariant _MacTooltipLayout oldDelegate) =>
      oldDelegate.anchorX != anchorX;
}

/// Two horizontal toggles, matching the recognizable Control Center silhouette.
class _ControlCenterBarIcon extends StatelessWidget {
  const _ControlCenterBarIcon();
  @override
  Widget build(BuildContext context) => SizedBox(
    width: 17,
    height: 17,
    child: CustomPaint(
      painter: _ControlCenterIconPainter(
        IconTheme.of(context).color ?? Colors.black,
      ),
    ),
  );
}

class _ControlCenterIconPainter extends CustomPainter {
  const _ControlCenterIconPainter(this.color);
  final Color color;
  @override
  void paint(Canvas canvas, Size size) {
    final sx = size.width / 17;
    final sy = size.height / 17;
    canvas.save();
    canvas.scale(sx, sy);
    final paint = Paint()..color = color;
    for (final row in [(2.0, 5.0), (9.5, 12.0)]) {
      canvas.drawRRect(
        RRect.fromRectAndRadius(
          Rect.fromLTWH(1, row.$1, 15, 5.5),
          const Radius.circular(2.75),
        ),
        paint
          ..style = PaintingStyle.stroke
          ..strokeWidth = 1.2,
      );
      canvas.drawCircle(
        Offset(row.$2, row.$1 + 2.75),
        1.5,
        paint..style = PaintingStyle.fill,
      );
    }
    canvas.restore();
  }

  @override
  bool shouldRepaint(_ControlCenterIconPainter oldDelegate) =>
      oldDelegate.color != color;
}
