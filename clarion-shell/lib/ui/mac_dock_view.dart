// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:math' as math;
import 'dart:async';

import 'package:flutter/gestures.dart';
import 'package:flutter/foundation.dart' show listEquals, mapEquals;
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';

import '../mac_shell_preferences.dart';
import '../mac_app_icons.dart';
import '../models.dart';
import '../state/shell_controller.dart';
import 'drag_feedback.dart';
import 'mac_app_icon.dart';
import 'dock_motion.dart';
import 'controller_selector.dart';
import 'mac_downloads_stack_panel.dart';
import 'optional_asset_icon.dart';

typedef DockGeometryChanged = void Function(Size desiredSize, Rect inputRegion);

/// One fixed native surface reserves room for magnification and launch motion.
class MacDockView extends StatefulWidget {
  const MacDockView({
    super.key,
    required this.controller,
    this.availableWidth,
    this.availableHeight,
    this.onGeometryChanged,
    this.onInputRegionsChanged,
    this.onPresentationChanged,
    this.onOpenMenu,
  });
  final ShellController controller;
  final double? availableWidth;
  final double? availableHeight;
  final DockGeometryChanged? onGeometryChanged;

  /// Exact visible regions for native hit testing; the geometry callback keeps
  /// the bounding rectangle for older hosts and layout observers.
  final ValueChanged<List<Rect>>? onInputRegionsChanged;
  final void Function(Rect anchor, VoidCallback open)? onOpenMenu;
  final void Function(
    Rect glass,
    Map<int, Rect> windowTargets,
    bool reducedMotion,
  )?
  onPresentationChanged;
  static const double height = 120;
  static double surfaceThickness(MacShellPreferences preferences) => math.max(
    height,
    preferences.dockSize *
            (preferences.magnification ? preferences.magnificationScale : 1) +
        50,
  );
  @override
  State<MacDockView> createState() => _MacDockViewState();
}

class _MacDockViewState extends State<MacDockView> {
  String? _hoveredAppId;
  String? _hoveredName;
  double _hoverMain = 0;
  Timer? _hideTimer;
  bool _edgeRevealed = false;
  bool _needsPointerMove = false;
  final _itemKeys = <String, GlobalKey<_DockItemState>>{};
  final _magnification = <String, double>{};
  PointerEvent? _pendingPointer;
  int? _pointerFrame;
  final _scroll = ScrollController();
  final _scrollViewportKey = GlobalKey();
  bool _presentationQueued = false;
  Rect? _presentationGlass;
  List<DockApp> _presentationApps = const [];
  bool _presentationReducedMotion = false;
  double? _wheelTarget;
  int _wheelRevision = 0;
  late bool _lastReducedMotion;
  late ShellPanel _lastPanel;
  late ShellStyle _lastStyle;
  late bool _lastAppMenuOpen;
  late MacShellPreferences _lastPreferences;
  bool? _lastPlatformReducedMotion;
  (Size, Rect)? _requestedGeometry;
  Size? _desiredSize;
  Rect _baseInputRegion = Rect.zero;
  bool _acceptIconInput = false;
  List<Rect>? _lastInputRegions;
  Rect? _lastGlass;
  bool? _lastTargetReducedMotion;
  int? _lastTargetDesktopRevision;
  Map<int, Rect> _lastWindowTargets = const {};
  static const _launcher = '__clarion-launcher__';
  static const _overview = '__clarion-overview__';
  static const _finder = '__clarion-finder__';
  static const _trash = '__clarion-trash__';
  static const _downloads = '__clarion-downloads__';
  static const _utilityIds = {
    _launcher,
    _overview,
    _finder,
    _trash,
    _downloads,
  };
  final _poofs = <_PoofInstance>[];

  void _onPoof(Offset globalOffset, String id) {
    final fileManagers = widget.controller.dockApps
        .where(_fileManager)
        .toList();
    final finderApp = fileManagers.isEmpty ? null : fileManagers.first;
    final finderId = finderApp?.id ?? _finder;
    if (id == finderId || id == _finder) {
      return;
    }
    final overlay = Overlay.maybeOf(context);
    if (overlay != null) {
      late OverlayEntry entry;
      entry = OverlayEntry(
        builder: (context) => Positioned(
          left: globalOffset.dx - 32,
          top: globalOffset.dy - 32,
          child: IgnorePointer(
            child: _PoofEffect(
              onComplete: () {
                if (entry.mounted) entry.remove();
              },
            ),
          ),
        ),
      );
      overlay.insert(entry);
    } else {
      final box = context.findRenderObject();
      final localOffset = box is RenderBox
          ? box.globalToLocal(globalOffset)
          : globalOffset;
      final poofId = DateTime.now().microsecondsSinceEpoch;
      setState(() {
        _poofs.add(
          _PoofInstance(id: poofId, offset: localOffset - const Offset(32, 32)),
        );
      });
    }
    widget.controller.unpinApp(id);
  }

  void _handleDockReorder(String draggedId, String targetId, bool isAfter) {
    if (draggedId == targetId) return;
    final fileManagers = widget.controller.dockApps
        .where(_fileManager)
        .toList();
    final finderApp = fileManagers.isEmpty ? null : fileManagers.first;
    final finderId = finderApp?.id ?? _finder;
    if (draggedId == finderId ||
        draggedId == _finder ||
        targetId == finderId ||
        targetId == _finder) {
      return;
    }
    final current = widget.controller.pinnedAppIds.toList();
    current.remove(draggedId);
    final targetIndex = current.indexOf(targetId);
    if (targetIndex == -1) {
      widget.controller.pinAppAt(draggedId);
      return;
    }
    if (!isAfter) {
      widget.controller.pinAppAt(draggedId, beforeId: targetId);
    } else {
      if (targetIndex + 1 < current.length) {
        widget.controller.pinAppAt(
          draggedId,
          beforeId: current[targetIndex + 1],
        );
      } else {
        widget.controller.pinAppAt(draggedId, beforeId: null);
      }
    }
  }

  @override
  void initState() {
    super.initState();
    _lastReducedMotion = widget.controller.reduceMotion;
    _lastPanel = widget.controller.panel;
    _lastStyle = widget.controller.shellStyle;
    _lastAppMenuOpen = widget.controller.appMenuOpen;
    _lastPreferences = widget.controller.macPreferences;
    widget.controller.addListener(_clearRemovedHover);
    _scroll.addListener(_schedulePresentation);
  }

  bool get _panelBlocked =>
      widget.controller.panel != ShellPanel.none &&
      widget.controller.panel != ShellPanel.launcher;
  bool get _dockOwnsPanel =>
      widget.controller.panel == ShellPanel.dockAppMenu ||
      widget.controller.panel == ShellPanel.dockDownloads ||
      widget.controller.panel == ShellPanel.dockContextMenu ||
      widget.controller.panel == ShellPanel.dockTrashMenu ||
      widget.controller.panel == ShellPanel.dockFinderMenu ||
      widget.controller.panel == ShellPanel.dockDownloadsMenu;

  void _clearRemovedHover() {
    final reduced = widget.controller.reduceMotion;
    final panel = widget.controller.panel;
    final style = widget.controller.shellStyle;
    final appMenuOpen = widget.controller.appMenuOpen;
    final preferences = widget.controller.macPreferences;
    final preferencesChanged = preferences != _lastPreferences;
    if ((reduced && !_lastReducedMotion) ||
        (panel != _lastPanel && _panelBlocked) ||
        (style != _lastStyle) ||
        (appMenuOpen && !_lastAppMenuOpen) ||
        preferencesChanged) {
      _stopScroll(
        finishTarget: reduced && !_panelBlocked && style == ShellStyle.macos,
      );
    }
    _lastReducedMotion = reduced;
    _lastPanel = panel;
    _lastStyle = style;
    _lastAppMenuOpen = appMenuOpen;
    _lastPreferences = preferences;
    if (preferencesChanged) {
      _cancelPointerUpdate();
      _hideTimer?.cancel();
      if (mounted) {
        setState(() {
          _hoveredAppId = null;
          _hoveredName = null;
          _magnification.clear();
          _needsPointerMove = true;
          _edgeRevealed = false;
        });
      }
    }
    if (_panelBlocked ||
        widget.controller.appMenuOpen ||
        widget.controller.shellStyle != ShellStyle.macos) {
      _cancelPointerUpdate();
      if (mounted &&
          (_hoveredName != null ||
              _magnification.isNotEmpty ||
              !_needsPointerMove)) {
        setState(() {
          _hoveredAppId = null;
          _hoveredName = null;
          _magnification.clear();
          _needsPointerMove = true;
        });
      }
      if (_edgeRevealed && mounted && !_dockOwnsPanel) {
        _hideTimer?.cancel();
        setState(() => _edgeRevealed = false);
      }
      return;
    }
    final id = _hoveredAppId;
    if (!mounted ||
        id == null ||
        _utilityIds.contains(id) ||
        widget.controller.dockApps.any((app) => app.id == id) ||
        widget.controller.recentDockApps.any((app) => app.id == id)) {
      return;
    }
    _cancelPointerUpdate();
    setState(() {
      _hoveredAppId = null;
      _hoveredName = null;
      _magnification.clear();
      _needsPointerMove = true;
    });
  }

  void _cancelPointerUpdate() {
    final frame = _pointerFrame;
    if (frame != null) {
      WidgetsBinding.instance.cancelFrameCallbackWithId(frame);
    }
    _pointerFrame = null;
    _pendingPointer = null;
  }

  void _pointer(PointerEvent event) {
    _hideTimer?.cancel();
    if (_panelBlocked ||
        widget.controller.appMenuOpen ||
        widget.controller.shellStyle != ShellStyle.macos ||
        (event is PointerEnterEvent && _needsPointerMove)) {
      return;
    }
    // A high-rate mouse may deliver several packets before one display frame.
    // Only the latest position needs a full Dock geometry pass.
    _pendingPointer = event;
    _pointerFrame ??= WidgetsBinding.instance.scheduleFrameCallback((_) {
      _pointerFrame = null;
      final pending = _pendingPointer;
      _pendingPointer = null;
      if (mounted && pending != null) _updatePointer(pending);
    });
  }

  void _updatePointer(PointerEvent event) {
    _hideTimer?.cancel();
    if (_panelBlocked ||
        widget.controller.appMenuOpen ||
        widget.controller.shellStyle != ShellStyle.macos ||
        (event is PointerEnterEvent && _needsPointerMove)) {
      return;
    }
    final preferences = widget.controller.macPreferences;
    if (preferences.dockAutoHide && !_edgeRevealed) return;
    if (!preferences.magnification) {
      if (_magnification.isNotEmpty) {
        setState(() {
          _magnification.clear();
          _needsPointerMove = false;
        });
      } else {
        _needsPointerMove = false;
      }
      return;
    }
    final box = context.findRenderObject();
    if (box is! RenderBox) return;
    final position = box.globalToLocal(event.position);
    final axes = _DockAxes(preferences.dockPosition);
    final scales = <String, double>{};
    bool changed = false;
    for (final entry in _itemKeys.entries) {
      final item = entry.value.currentContext?.findRenderObject();
      if (item is! RenderBox || !item.hasSize) continue;
      final center = item.localToGlobal(item.size.center(Offset.zero));
      final distance =
          (axes.coordinate(event.position) - axes.coordinate(center)).abs();
      // A continuous bell affects nearby icons; parameters are tuned visually.
      final influence = distance < 110
          ? (1 + math.cos(math.pi * distance / 110)) / 2
          : 0.0;
      final scale = 1 + (preferences.magnificationScale - 1) * influence;
      scales[entry.key] = scale;
      final previous = _magnification[entry.key] ?? 1.0;
      if ((previous - scale).abs() > 0.004) {
        changed = true;
      }
    }
    if (scales.length != _magnification.length) {
      changed = true;
    }
    if (!changed && !_needsPointerMove) {
      return;
    }
    setState(() {
      _hoverMain = axes.coordinate(position);
      _needsPointerMove = false;
      _magnification
        ..clear()
        ..addAll(scales);
    });
  }

  void _leave() {
    _cancelPointerUpdate();
    final hadHover =
        _hoveredAppId != null ||
        _hoveredName != null ||
        _magnification.isNotEmpty;
    if (hadHover) {
      setState(() {
        _hoveredAppId = null;
        _hoveredName = null;
        _magnification.clear();
        // A modal panel can empty the input region and synthesize an exit.
        // Preserve its movement guard so closing it cannot reactivate zoom.
      });
    }
    if (widget.controller.macPreferences.dockAutoHide) {
      _hideTimer?.cancel();
      if (_dockOwnsPanel) return;
      _hideTimer = Timer(const Duration(milliseconds: 180), () {
        if (mounted) setState(() => _edgeRevealed = false);
      });
    }
  }

  void _reveal([PointerEvent? event]) {
    _hideTimer?.cancel();
    if (_edgeRevealed ||
        (event is PointerEnterEvent && _needsPointerMove) ||
        _panelBlocked ||
        widget.controller.appMenuOpen ||
        widget.controller.shellStyle != ShellStyle.macos) {
      return;
    }
    setState(() {
      _edgeRevealed = true;
      _needsPointerMove = false;
    });
  }

  void _wheel(PointerSignalEvent event) {
    if (event is! PointerScrollEvent ||
        !_scroll.hasClients ||
        _scroll.position.maxScrollExtent <= 0 ||
        _panelBlocked ||
        widget.controller.appMenuOpen ||
        (widget.controller.macPreferences.dockAutoHide && !_edgeRevealed)) {
      return;
    }
    final delta = event.scrollDelta.dy.abs() >= event.scrollDelta.dx.abs()
        ? event.scrollDelta.dy
        : event.scrollDelta.dx;
    if (delta == 0) return;
    GestureBinding.instance.pointerSignalResolver.register(event, (_) {
      if (!mounted || !_scroll.hasClients) return;
      final target = ((_wheelTarget ?? _scroll.offset) + delta)
          .clamp(0.0, _scroll.position.maxScrollExtent)
          .toDouble();
      _wheelTarget = target;
      final revision = ++_wheelRevision;
      _cancelPointerUpdate();
      setState(() {
        _hoveredAppId = null;
        _hoveredName = null;
        _magnification.clear();
        _needsPointerMove = true;
      });
      if (widget.controller.reduceMotion ||
          MediaQuery.disableAnimationsOf(context)) {
        _scroll.jumpTo(target);
        _wheelTarget = null;
        return;
      }
      unawaited(
        _scroll
            .animateTo(
              target,
              duration: const Duration(milliseconds: 150),
              curve: Curves.easeOutCubic,
            )
            .whenComplete(() {
              if (mounted && revision == _wheelRevision) _wheelTarget = null;
            }),
      );
    });
  }

  void _stopScroll({bool finishTarget = false}) {
    final target = _scroll.hasClients
        ? (finishTarget ? _wheelTarget ?? _scroll.offset : _scroll.offset)
        : null;
    _wheelRevision++;
    _wheelTarget = null;
    if (target != null) {
      _scroll.jumpTo(
        target.clamp(0.0, _scroll.position.maxScrollExtent).toDouble(),
      );
    }
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final disabled = MediaQuery.disableAnimationsOf(context);
    if (disabled && _lastPlatformReducedMotion != true) {
      _stopScroll(finishTarget: true);
    }
    _lastPlatformReducedMotion = disabled;
  }

  void _hover(String id, String? name, Offset globalPosition) {
    if (!mounted ||
        (name == null && _hoveredAppId != id) ||
        (name != null && _needsPointerMove) ||
        _panelBlocked ||
        widget.controller.appMenuOpen ||
        widget.controller.shellStyle != ShellStyle.macos) {
      return;
    }
    final box = context.findRenderObject();
    setState(() {
      _hoveredAppId = name == null ? null : id;
      _hoveredName = name;
      final position = box is RenderBox
          ? box.globalToLocal(globalPosition)
          : globalPosition;
      _hoverMain = _DockAxes(
        widget.controller.macPreferences.dockPosition,
      ).coordinate(position);
    });
  }

  void _reportGeometry(Size size, Rect inputRegion) {
    final geometry = (size, inputRegion);
    if (_requestedGeometry == geometry) return;
    _requestedGeometry = geometry;
    // Called after layout, when transforms describe the sprites actually
    // painted this frame. Drawing space is not a pointer activation region.
    widget.onGeometryChanged?.call(size, inputRegion);
  }

  Iterable<Rect> _inputRegions() sync* {
    if (!_baseInputRegion.isEmpty) yield _baseInputRegion;
    if (!_acceptIconInput) return;
    final root = context.findRenderObject();
    final clip = _scrollViewportKey.currentContext?.findRenderObject();
    if (root is! RenderBox ||
        !root.hasSize ||
        clip is! RenderBox ||
        !clip.hasSize) {
      return;
    }
    final viewport = MatrixUtils.transformRect(
      clip.getTransformTo(root),
      Offset.zero & clip.size,
    ).intersect(Offset.zero & root.size);
    for (final key in _itemKeys.values) {
      final rect = key.currentState?.iconRect(root);
      if (rect == null || !rect.overlaps(viewport)) continue;
      final visible = rect.intersect(viewport);
      if (visible.left >= _baseInputRegion.left &&
          visible.top >= _baseInputRegion.top &&
          visible.right <= _baseInputRegion.right &&
          visible.bottom <= _baseInputRegion.bottom) {
        continue;
      }
      yield visible;
    }
  }

  bool _containsInput(Offset position) =>
      _inputRegions().any((region) => region.contains(position));

  void _reportInputGeometry() {
    final size = _desiredSize;
    if (size == null) return;
    final regions = List<Rect>.unmodifiable(_inputRegions());
    var input = Rect.zero;
    for (final region in regions) {
      input = input.isEmpty ? region : input.expandToInclude(region);
    }
    _reportGeometry(size, input);
    if (!listEquals(_lastInputRegions, regions)) {
      _lastInputRegions = regions;
      widget.onInputRegionsChanged?.call(regions);
    }
  }

  void _reportPresentation(Rect glass, List<DockApp> apps, bool reducedMotion) {
    _presentationGlass = glass;
    _presentationApps = apps;
    _presentationReducedMotion = reducedMotion;
    _schedulePresentation();
  }

  void _schedulePresentation() {
    if ((widget.onPresentationChanged == null &&
            widget.onGeometryChanged == null &&
            widget.onInputRegionsChanged == null) ||
        _presentationQueued ||
        !mounted) {
      return;
    }
    _presentationQueued = true;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _presentationQueued = false;
      if (!mounted) return;
      _reportInputGeometry();
      if (widget.onPresentationChanged == null) return;
      final glass = _presentationGlass;
      if (glass == null) return;
      final apps = _presentationApps;
      final reducedMotion = _presentationReducedMotion;
      if (glass.isEmpty) {
        if (_lastGlass != Rect.zero ||
            _lastWindowTargets.isNotEmpty ||
            _lastTargetReducedMotion != reducedMotion) {
          _lastGlass = Rect.zero;
          _lastWindowTargets = const {};
          _lastTargetReducedMotion = reducedMotion;
          widget.onPresentationChanged?.call(
            Rect.zero,
            const {},
            reducedMotion,
          );
        }
        return;
      }
      final root = context.findRenderObject();
      final clip = _scrollViewportKey.currentContext?.findRenderObject();
      if (root is! RenderBox ||
          !root.hasSize ||
          clip is! RenderBox ||
          !clip.hasSize) {
        return;
      }
      final viewport = MatrixUtils.transformRect(
        clip.getTransformTo(root),
        Offset.zero & clip.size,
      ).intersect(Offset.zero & root.size);
      final targets = <int, Rect>{};
      for (final app in apps) {
        final rect = _itemKeys[app.id]?.currentState?.iconRect(root);
        if (rect == null || !rect.overlaps(viewport)) continue;
        final clipped = rect.intersect(viewport);
        if (clipped.width < 8 || clipped.height < 8) continue;
        final rounded = Rect.fromLTRB(
          clipped.left.ceilToDouble(),
          clipped.top.ceilToDouble(),
          clipped.right.floorToDouble(),
          clipped.bottom.floorToDouble(),
        );
        for (final window in app.windows) {
          targets[window.id] = rounded;
        }
      }
      if (!widget.controller.macPreferences.minimizeIntoApp) {
        for (final window in widget.controller.windows.where(
          (w) => w.isMinimized,
        )) {
          final rect = _itemKeys['window-${window.id}']?.currentState?.iconRect(
            root,
          );
          if (rect == null || !rect.overlaps(viewport)) continue;
          final clipped = rect.intersect(viewport);
          if (clipped.width < 8 || clipped.height < 8) continue;
          targets[window.id] = Rect.fromLTRB(
            clipped.left.ceilToDouble(),
            clipped.top.ceilToDouble(),
            clipped.right.floorToDouble(),
            clipped.bottom.floorToDouble(),
          );
        }
      }
      if (_lastGlass == glass &&
          mapEquals(_lastWindowTargets, targets) &&
          _lastTargetDesktopRevision == widget.controller.desktopRevision &&
          _lastTargetReducedMotion == reducedMotion) {
        return;
      }
      _lastTargetDesktopRevision = widget.controller.desktopRevision;
      _lastTargetReducedMotion = reducedMotion;
      _lastGlass = glass;
      _lastWindowTargets = targets;
      widget.onPresentationChanged?.call(
        glass,
        Map.unmodifiable(targets),
        reducedMotion,
      );
    });
  }

  @override
  void didUpdateWidget(covariant MacDockView oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.controller != widget.controller) {
      _cancelPointerUpdate();
      _stopScroll();
      oldWidget.controller.removeListener(_clearRemovedHover);
      widget.controller.addListener(_clearRemovedHover);
      _hoveredAppId = null;
      _hoveredName = null;
      _magnification.clear();
      _lastReducedMotion = widget.controller.reduceMotion;
      _lastPanel = widget.controller.panel;
      _lastStyle = widget.controller.shellStyle;
      _lastAppMenuOpen = widget.controller.appMenuOpen;
      _lastPreferences = widget.controller.macPreferences;
      _edgeRevealed = false;
      _lastGlass = null;
      _lastWindowTargets = const {};
      _lastTargetDesktopRevision = null;
      _requestedGeometry = null;
      _lastInputRegions = null;
    }
    if (oldWidget.onGeometryChanged == null &&
        widget.onGeometryChanged != null) {
      _requestedGeometry = null;
    }
    if (oldWidget.onInputRegionsChanged == null &&
        widget.onInputRegionsChanged != null) {
      _lastInputRegions = null;
    }
  }

  @override
  void dispose() {
    _cancelPointerUpdate();
    _hideTimer?.cancel();
    widget.controller.removeListener(_clearRemovedHover);
    _scroll.removeListener(_schedulePresentation);
    _scroll.dispose();
    super.dispose();
  }

  bool _fileManager(DockApp app) => MacAppIcons.isFileManager(app.app);

  void _openMenu(String id, VoidCallback open) {
    final root = context.findRenderObject();
    final anchor = root is RenderBox
        ? _itemKeys[id]?.currentState?.iconRect(root)
        : null;
    if (anchor == null || root is! RenderBox) return;
    final native = widget.onOpenMenu;
    if (native != null) {
      native(anchor, open);
    } else {
      widget.controller.setDockMenuAnchor(
        anchor.shift(root.localToGlobal(Offset.zero)),
      );
      open();
    }
  }

  void _openMenuFromBox(RenderBox box, VoidCallback open) {
    final root = context.findRenderObject();
    if (root is! RenderBox || !box.hasSize) return;
    final anchor = MatrixUtils.transformRect(
      box.getTransformTo(root),
      Offset.zero & box.size,
    );
    final native = widget.onOpenMenu;
    if (native != null) {
      native(anchor, open);
    } else {
      widget.controller.setDockMenuAnchor(
        anchor.shift(root.localToGlobal(Offset.zero)),
      );
      open();
    }
  }

  Widget _asset(String name, int pixelSize) => OptionalAssetIcon(
    // The three dock utilities prefer a locally imported original kept out
    // of the tree; the committed WhiteSur icon is the shared fallback.
    preferredAsset: switch (name) {
      'finder' => 'assets/shell/finder.png',
      'launchpad' => 'assets/shell/launchpad.png',
      'mission-control' => 'assets/shell/mission-control.png',
      _ => null,
    },
    asset: switch (name) {
      'finder' => 'assets/icons/home.svg',
      'launchpad' => 'assets/icons/launchpad.png',
      'mission-control' => 'assets/icons/mission-control.svg',
      'trash-full' => 'assets/icons/trash-full.png',
      _ => 'assets/icons/trash-empty.png',
    },
    fallback: switch (name) {
      'finder' => Icons.folder_rounded,
      'launchpad' => Icons.apps_rounded,
      'mission-control' => Icons.dashboard_rounded,
      'trash-full' => Icons.delete_rounded,
      _ => Icons.delete_outline_rounded,
    },
    rasterSize: pixelSize,
    width: 48,
    height: 48,
  );

  @override
  Widget build(BuildContext context) => ControllerSelector(
    controller: widget.controller,
    select: () => (
      desktop: widget.controller.desktopRevision,
      apps: widget.controller.apps,
      pins: widget.controller.pinnedAppIds.join('\u0000'),
      recents: widget.controller.recentAppIds.join('\u0000'),
      trash: widget.controller.trashState.hasItems,
      folderEntries: widget.controller.dockFolder.entries,
      panel: widget.controller.panel,
      appMenuOpen: widget.controller.appMenuOpen,
      reduced: widget.controller.reduceMotion,
      preferences: widget.controller.macPreferences,
    ),
    builder: (context, _) => LayoutBuilder(
      builder: (context, constraints) {
        final controller = widget.controller;
        final preferences = controller.macPreferences;
        // Children retain a stable logical layout throughout pointer motion.
        // Decode textures for the largest configured presentation,
        // including FittedBox scaling and magnification, before hover begins.
        final rasterSize =
            preferences.dockSize *
            (preferences.magnification ? preferences.magnificationScale : 1);
        final pixelSize = (rasterSize * MediaQuery.devicePixelRatioOf(context))
            .ceil()
            .clamp(16, 512);
        final axes = _DockAxes(preferences.dockPosition);
        final apps = controller.dockApps;
        final fileManagers = apps.where(_fileManager).toList();
        final finderApp = fileManagers.isEmpty ? null : fileManagers.first;
        final finderId = finderApp?.id ?? _finder;
        final pinned = apps
            .where(
              (app) => app.isPinned && app.id != finderId && app.id != _finder,
            )
            .toList();
        final running = apps
            .where(
              (app) => !app.isPinned && app.id != finderId && app.id != _finder,
            )
            .toList();
        final recent = controller.recentDockApps;
        final reduced =
            controller.reduceMotion || MediaQuery.disableAnimationsOf(context);
        final blocked =
            (controller.panel != ShellPanel.none &&
                controller.panel != ShellPanel.launcher) ||
            controller.appMenuOpen ||
            controller.shellStyle != ShellStyle.macos;
        final visible =
            !preferences.dockAutoHide ||
            ((_edgeRevealed || _dockOwnsPanel) && (!blocked || _dockOwnsPanel));
        final dark = Theme.of(context).brightness == Brightness.dark;
        final slots = <_DockSlot>[];
        void appSlot(DockApp app) => slots.add(
          _DockSlot(
            id: app.id,
            label: app.name,
            icon: MacAppIcon(
              app: app.app,
              size: 48,
              rasterSize: rasterSize,
              useOriginalIcons: preferences.useOriginalIcons,
            ),
            onTap: () => controller.focusOrLaunchApp(app.id),
            onMenu: () =>
                _openMenu(app.id, () => controller.openDockAppMenu(app.id)),
            running: app.isRunning,
            selected: app.isFocused,
            launches: !app.isRunning,
            pinned: app.isPinned,
            status: app.isRunning ? '正在运行，${app.windows.length} 个窗口' : '固定应用',
          ),
        );
        slots.add(
          _DockSlot(
            id: finderId,
            label: 'Home',
            icon: _asset('finder', pixelSize),
            onTap: () {
              if (finderApp != null) {
                controller.focusOrLaunchApp(finderApp.id);
              } else {
                controller.openHomeFolder();
              }
            },
            onMenu: () => _openMenu(finderId, controller.openDockFinderMenu),
            running: true,
            selected: finderApp?.isFocused ?? false,
            launches: false,
            pinned: true,
            status: finderApp != null && finderApp.windows.isNotEmpty
                ? '正在运行，${finderApp.windows.length} 个窗口'
                : '正在运行',
          ),
        );
        if (preferences.showLaunchpad) {
          slots.add(
            _DockSlot(
              id: _launcher,
              label: '启动台',
              icon: _asset('launchpad', pixelSize),
              onTap: () => controller.selectPanel(ShellPanel.launcher),
              selected: controller.panel == ShellPanel.launcher,
            ),
          );
        }
        for (final app in pinned) {
          appSlot(app);
        }
        for (final app in running) {
          appSlot(app);
        }
        if (preferences.showMissionControl) {
          slots.add(
            _DockSlot(
              id: _overview,
              label: '调度中心',
              icon: _asset('mission-control', pixelSize),
              selected: controller.panel == ShellPanel.spaces,
              onTap: () {
                if (controller.shellStyle == ShellStyle.macos) {
                  controller.toggleSpaces();
                } else {
                  controller.toggleOverview();
                }
              },
            ),
          );
        }
        if (recent.isNotEmpty) {
          slots.add(const _DockSlot.divider('recent-divider'));
          for (final app in recent) {
            appSlot(app);
          }
        }
        slots.add(const _DockSlot.divider('utilities-divider'));
        if (!preferences.minimizeIntoApp) {
          final minimized = controller.windows
              .where((w) => w.isMinimized)
              .toList();
          for (final win in minimized) {
            DesktopApp? parentApp;
            for (final a in controller.apps) {
              if (a.id == win.appId ||
                  a.id == win.appId.replaceFirst(RegExp(r'\.desktop$'), '')) {
                parentApp = a;
                break;
              }
            }
            slots.add(
              _DockSlot(
                id: 'window-${win.id}',
                label: win.title.isNotEmpty ? win.title : '已最小化的窗口',
                icon: _MinimizedWindowIcon(
                  window: win,
                  app: parentApp,
                  rasterSize: rasterSize,
                  useOriginalIcons: preferences.useOriginalIcons,
                ),
                onTap: () => controller.focusWindow(win.id),
                status: '已最小化 · ${win.title}',
              ),
            );
          }
        }
        if (preferences.showDownloads) {
          slots.add(
            _DockSlot(
              id: _downloads,
              label: '下载',
              icon: MacDownloadsStackIcon(
                service: controller.dockFolder,
                preferences: preferences,
                rasterSize: rasterSize,
              ),
              onTap: () => _openMenu(_downloads, controller.openDownloadsStack),
              onMenu: () =>
                  _openMenu(_downloads, controller.openDockDownloadsMenu),
            ),
          );
        }
        slots.add(
          _DockSlot(
            id: _trash,
            label: '废纸篓',
            icon: _asset(
              controller.trashState.hasItems ? 'trash-full' : 'trash-empty',
              pixelSize,
            ),
            onTap: controller.openTrash,
            onMenu: () => _openMenu(_trash, controller.openDockTrashMenu),
          ),
        );
        final activeIds = slots.map((slot) => slot.id).toSet();
        _itemKeys.removeWhere((id, _) => !activeIds.contains(id));
        final thickness = MacDockView.surfaceThickness(preferences);
        final surface = Size(
          constraints.maxWidth.isFinite ? constraints.maxWidth : thickness,
          constraints.maxHeight.isFinite ? constraints.maxHeight : thickness,
        );
        final longExtent = axes.mainSize(surface);
        final crossExtent = axes.crossSize(surface);
        final available = axes.horizontal
            ? widget.availableWidth ?? longExtent
            : widget.availableHeight ?? longExtent;
        final maxLong = math.max(1.0, available - 48);
        final iconCount = slots.where((slot) => !slot.divider).length;
        final dividerExtent = slots.where((slot) => slot.divider).length * 14.0;
        final maxScale = preferences.magnification
            ? preferences.magnificationScale
            : 1.0;
        final reserve = math.max(
          0.0,
          (maxScale - 1) * preferences.dockSize * 4.5,
        );
        final iconSize =
            ((maxLong - 24 - reserve - dividerExtent) / iconCount - 10)
                .clamp(24.0, preferences.dockSize)
                .toDouble();
        final naturalLong =
            24.0 +
            slots.fold<double>(0, (sum, slot) => sum + slot.width(iconSize));
        final requestedLong = math.min(naturalLong + reserve, maxLong);
        final targetLong = math.min(
          requestedLong,
          24.0 +
              slots.fold<double>(
                0,
                (sum, slot) =>
                    sum +
                    slot.width(iconSize) *
                        (slot.divider ? 1 : (_magnification[slot.id] ?? 1)),
              ),
        );
        final contents = SingleChildScrollView(
          key: _scrollViewportKey,
          controller: _scroll,
          scrollDirection: axes.horizontal ? Axis.horizontal : Axis.vertical,
          clipBehavior: Clip.hardEdge,
          child: Listener(
            behavior: HitTestBehavior.translucent,
            onPointerSignal: _wheel,
            child: RepaintBoundary(
              child: Flex(
                direction: axes.horizontal ? Axis.horizontal : Axis.vertical,
                mainAxisSize: MainAxisSize.min,
                children: [
                  for (final slot in slots)
                    if (slot.divider)
                      _DockDivider(
                        key: ValueKey('dock-divider-${slot.id}'),
                        id: slot.id,
                        iconSize: iconSize,
                        axes: axes,
                        crossExtent: crossExtent,
                        preferences: preferences,
                        onResize: (newSize) => controller.setMacPreferences(
                          preferences.copyWith(dockSize: newSize),
                        ),
                        onContextMenu: (box) => _openMenuFromBox(
                          box,
                          controller.openDockContextMenu,
                        ),
                        onAppDropped: (draggedId) =>
                            controller.pinAppAt(draggedId, beforeId: null),
                      )
                    else
                      _DockItem(
                        key: _itemKeys.putIfAbsent(
                          slot.id,
                          GlobalKey<_DockItemState>.new,
                        ),
                        id: slot.id,
                        iconSize: iconSize,
                        label: slot.label,
                        axes: axes,
                        crossExtent: crossExtent,
                        magnification: _magnification[slot.id] ?? 1,
                        maximumMagnification: maxScale,
                        reducedMotion: reduced || blocked || !visible,
                        animateLaunch: preferences.animateOpeningApplications,
                        showRunningIndicator: preferences.showRunningIndicators,
                        running: slot.running,
                        selected: slot.selected,
                        status: slot.status,
                        launches: slot.launches,
                        onHover: _hover,
                        onTap: slot.onTap!,
                        onContextMenu: slot.onMenu,
                        onGeometryChanged: _schedulePresentation,
                        onReorder:
                            (slot.pinned || slot.running) &&
                                slot.id != finderId &&
                                slot.id != _finder &&
                                !slot.id.startsWith('window-') &&
                                slot.id != _downloads &&
                                slot.id != _trash &&
                                slot.id != _launcher &&
                                slot.id != _overview
                            ? (draggedId, isAfter) => _handleDockReorder(
                                draggedId,
                                slot.id,
                                isAfter,
                              )
                            : null,
                        onPoof:
                            slot.pinned &&
                                slot.id != finderId &&
                                slot.id != _finder
                            ? (offset) => _onPoof(offset, slot.id)
                            : null,
                        child: slot.icon!,
                      ),
                ],
              ),
            ),
          ),
        );
        return _DockInputRegion(
          contains: _containsInput,
          child: MouseRegion(
            onEnter: _pointer,
            onHover: _pointer,
            onExit: (_) => _leave(),
            child: Listener(
              onPointerDown: (_) => setState(() => _hoveredName = null),
              child: SizedBox.fromSize(
                size: surface,
                child: Stack(
                  clipBehavior: Clip.hardEdge,
                  children: [
                    DockMotionBuilder(
                      target: math.min(targetLong, longExtent),
                      instant: reduced,
                      lowerBound: 0,
                      upperBound: longExtent,
                      child: contents,
                      builder: (context, length, content) {
                        final start = (longExtent - length) / 2;
                        final sensor = axes.rect(
                          surface,
                          start,
                          0,
                          length,
                          math.min(2, crossExtent),
                        );
                        // Reserve animation space for drawing, but only claim
                        // the resting Dock and the icons visible this frame.
                        _desiredSize = axes.size(requestedLong, thickness);
                        final glass = axes
                            .rect(surface, start, 2, length, iconSize + 14)
                            .intersect(Offset.zero & surface);
                        return Stack(
                          children: [
                            DockMotionBuilder(
                              target: visible ? 1.0 : 0.0,
                              instant: reduced,
                              response: const Duration(milliseconds: 180),
                              lowerBound: 0,
                              upperBound: 1,
                              child: content,
                              builder: (context, reveal, content) {
                                final slide = axes.inward(
                                  -(1 - reveal) * crossExtent,
                                );
                                final paintedInput = axes
                                    .rect(
                                      surface,
                                      start,
                                      0,
                                      length,
                                      math.min(crossExtent, iconSize + 16),
                                    )
                                    .shift(slide)
                                    .intersect(Offset.zero & surface);
                                _baseInputRegion = blocked
                                    ? Rect.zero
                                    : !visible || paintedInput.isEmpty
                                    ? sensor
                                    : paintedInput.expandToInclude(sensor);
                                _acceptIconInput = visible && !blocked;
                                final settled =
                                    visible &&
                                    reveal >= .999 &&
                                    controller.shellStyle == ShellStyle.macos;
                                _reportPresentation(
                                  settled ? glass : Rect.zero,
                                  settled ? apps : const [],
                                  reduced,
                                );
                                return IgnorePointer(
                                  key: const ValueKey('mac-dock-input-guard'),
                                  ignoring: !visible || blocked,
                                  child: Opacity(
                                    opacity: reveal,
                                    child: Transform.translate(
                                      offset: slide,
                                      child: Stack(
                                        children: [
                                          Positioned.fromRect(
                                            rect: glass,
                                            child: _DockGlass(
                                              key: const ValueKey(
                                                'mac-dock-glass',
                                              ),
                                              dark: dark,
                                              onContextMenu: (box) =>
                                                  _openMenuFromBox(
                                                    box,
                                                    controller
                                                        .openDockContextMenu,
                                                  ),
                                            ),
                                          ),
                                          Positioned.fromRect(
                                            rect: axes.rect(
                                              surface,
                                              start + 12,
                                              0,
                                              math.max(0, length - 24),
                                              crossExtent,
                                            ),
                                            child: content!,
                                          ),
                                        ],
                                      ),
                                    ),
                                  ),
                                );
                              },
                            ),
                            if (preferences.dockAutoHide)
                              Positioned.fromRect(
                                rect: sensor,
                                child: MouseRegion(
                                  onEnter: _reveal,
                                  onHover: _reveal,
                                  child: GestureDetector(
                                    key: const ValueKey('mac-dock-edge-sensor'),
                                    behavior: HitTestBehavior.opaque,
                                    onTap: _reveal,
                                    child: const SizedBox.expand(),
                                  ),
                                ),
                              ),
                          ],
                        );
                      },
                    ),
                    if (_hoveredName != null && visible && !blocked)
                      Builder(
                        builder: (context) {
                          final hoveredScale =
                              _magnification[_hoveredAppId] ?? 1.0;
                          final tooltipWidth = math.max(
                            1.0,
                            math.min(180.0, surface.width - 16),
                          );
                          const tooltipHeight = 22.0;
                          final Rect tooltipRect;
                          switch (axes.position) {
                            case MacDockPosition.bottom:
                              final iconTop =
                                  surface.height -
                                  13 -
                                  iconSize * hoveredScale -
                                  (hoveredScale - 1) * 12;
                              final top = (iconTop - tooltipHeight - 8)
                                  .clamp(
                                    2.0,
                                    math.max(
                                      2.0,
                                      surface.height - tooltipHeight,
                                    ),
                                  )
                                  .toDouble();
                              final left = (_hoverMain - tooltipWidth / 2)
                                  .clamp(
                                    8.0,
                                    math.max(
                                      8.0,
                                      surface.width - tooltipWidth - 8,
                                    ),
                                  )
                                  .toDouble();
                              tooltipRect = Rect.fromLTWH(
                                left,
                                top,
                                tooltipWidth,
                                tooltipHeight,
                              );
                            case MacDockPosition.left:
                              final iconRight =
                                  13 +
                                  iconSize * hoveredScale +
                                  (hoveredScale - 1) * 12;
                              final left = (iconRight + 8)
                                  .clamp(
                                    2.0,
                                    math.max(2.0, surface.width - tooltipWidth),
                                  )
                                  .toDouble();
                              final top = (_hoverMain - tooltipHeight / 2)
                                  .clamp(
                                    4.0,
                                    math.max(
                                      4.0,
                                      surface.height - tooltipHeight - 4,
                                    ),
                                  )
                                  .toDouble();
                              tooltipRect = Rect.fromLTWH(
                                left,
                                top,
                                tooltipWidth,
                                tooltipHeight,
                              );
                            case MacDockPosition.right:
                              final iconLeft =
                                  surface.width -
                                  13 -
                                  iconSize * hoveredScale -
                                  (hoveredScale - 1) * 12;
                              final left = (iconLeft - tooltipWidth - 8)
                                  .clamp(
                                    2.0,
                                    math.max(2.0, surface.width - tooltipWidth),
                                  )
                                  .toDouble();
                              final top = (_hoverMain - tooltipHeight / 2)
                                  .clamp(
                                    4.0,
                                    math.max(
                                      4.0,
                                      surface.height - tooltipHeight - 4,
                                    ),
                                  )
                                  .toDouble();
                              tooltipRect = Rect.fromLTWH(
                                left,
                                top,
                                tooltipWidth,
                                tooltipHeight,
                              );
                          }
                          return Positioned.fromRect(
                            rect: tooltipRect,
                            child: IgnorePointer(
                              child: Center(
                                child: Container(
                                  key: const ValueKey('mac-dock-label'),
                                  padding: const EdgeInsets.symmetric(
                                    horizontal: 10,
                                    vertical: 3.5,
                                  ),
                                  decoration: BoxDecoration(
                                    color: const Color(0xeb1f1f23),
                                    borderRadius: BorderRadius.circular(7),
                                    border: Border.all(
                                      color: Colors.white.withValues(
                                        alpha: .20,
                                      ),
                                      width: .6,
                                    ),
                                    boxShadow: [
                                      BoxShadow(
                                        color: Colors.black.withValues(
                                          alpha: .40,
                                        ),
                                        blurRadius: 10,
                                        offset: const Offset(0, 3),
                                      ),
                                    ],
                                  ),
                                  child: Text(
                                    _hoveredName!,
                                    maxLines: 1,
                                    overflow: TextOverflow.ellipsis,
                                    style: const TextStyle(
                                      fontSize: 12,
                                      fontWeight: FontWeight.w500,
                                      color: Colors.white,
                                      letterSpacing: -0.2,
                                      decoration: TextDecoration.none,
                                    ),
                                  ),
                                ),
                              ),
                            ),
                          );
                        },
                      ),
                    for (final poof in _poofs)
                      Positioned(
                        left: poof.offset.dx,
                        top: poof.offset.dy,
                        child: IgnorePointer(
                          child: _PoofEffect(
                            onComplete: () {
                              if (mounted) {
                                setState(
                                  () => _poofs.removeWhere(
                                    (p) => p.id == poof.id,
                                  ),
                                );
                              }
                            },
                          ),
                        ),
                      ),
                  ],
                ),
              ),
            ),
          ),
        );
      },
    ),
  );
}

/// Keep Flutter preview hit testing aligned with the native input region.
/// Transparent animation/tooltip margins must not swallow underlying clicks.
class _DockInputRegion extends SingleChildRenderObjectWidget {
  const _DockInputRegion({required this.contains, required super.child});
  final bool Function(Offset) contains;

  @override
  RenderObject createRenderObject(BuildContext context) =>
      _RenderDockInputRegion(contains);

  @override
  void updateRenderObject(
    BuildContext context,
    _RenderDockInputRegion renderObject,
  ) {
    renderObject.contains = contains;
  }
}

class _RenderDockInputRegion extends RenderProxyBox {
  _RenderDockInputRegion(this.contains);
  bool Function(Offset) contains;

  @override
  bool hitTest(BoxHitTestResult result, {required Offset position}) =>
      contains(position) && super.hitTest(result, position: position);
}

class _DockSlot {
  const _DockSlot({
    required this.id,
    required this.label,
    required this.icon,
    required this.onTap,
    this.onMenu,
    this.running = false,
    this.selected = false,
    this.launches = false,
    this.pinned = false,
    this.status = '',
  }) : divider = false;
  const _DockSlot.divider(this.id)
    : label = '',
      icon = null,
      onTap = null,
      onMenu = null,
      running = false,
      selected = false,
      launches = false,
      pinned = false,
      status = '',
      divider = true;
  final String id;
  final String label;
  final Widget? icon;
  final VoidCallback? onTap;
  final VoidCallback? onMenu;
  final bool running;
  final bool selected;
  final bool launches;
  final bool pinned;
  final bool divider;
  final String status;
  double width(double iconSize) => divider ? 14 : iconSize + 10;
}

/// Main coordinates run along the Dock; inset coordinates run into the output.
/// Rendering and all interaction paths use this orientation without rotation.
class _DockAxes {
  const _DockAxes(this.position);
  final MacDockPosition position;
  bool get horizontal => position == MacDockPosition.bottom;
  double coordinate(Offset point) => horizontal ? point.dx : point.dy;
  double mainSize(Size size) => horizontal ? size.width : size.height;
  double crossSize(Size size) => horizontal ? size.height : size.width;
  Size size(double main, double cross) =>
      horizontal ? Size(main, cross) : Size(cross, main);
  Offset inward(double amount) => switch (position) {
    MacDockPosition.bottom => Offset(0, -amount),
    MacDockPosition.left => Offset(amount, 0),
    MacDockPosition.right => Offset(-amount, 0),
  };
  Alignment get anchor => switch (position) {
    MacDockPosition.bottom => Alignment.bottomCenter,
    MacDockPosition.left => Alignment.centerLeft,
    MacDockPosition.right => Alignment.centerRight,
  };
  Rect rect(
    Size surface,
    double main,
    double inset,
    double length,
    double depth,
  ) => switch (position) {
    MacDockPosition.bottom => Rect.fromLTWH(
      main,
      surface.height - inset - depth,
      length,
      depth,
    ),
    MacDockPosition.left => Rect.fromLTWH(inset, main, depth, length),
    MacDockPosition.right => Rect.fromLTWH(
      surface.width - inset - depth,
      main,
      depth,
      length,
    ),
  };
}

/// Mouse dragging begins immediately on primary drag motion; touch keeps a 350ms
/// long press so gestures do not conflict with taps.
class _MouseDockDraggable extends Draggable<String> {
  const _MouseDockDraggable({
    required super.data,
    required super.feedback,
    required super.childWhenDragging,
    super.onDraggableCanceled,
    required super.child,
  }) : super(
         maxSimultaneousDrags: 1,
         dragAnchorStrategy: pointerDragAnchorStrategy,
       );

  @override
  ImmediateMultiDragGestureRecognizer createRecognizer(
    GestureMultiDragStartCallback onStart,
  ) => ImmediateMultiDragGestureRecognizer(
    supportedDevices: const {PointerDeviceKind.mouse},
    allowedButtonsFilter: (buttons) => buttons == kPrimaryMouseButton,
  )..onStart = onStart;
}

class _DockItem extends StatefulWidget {
  const _DockItem({
    super.key,
    required this.id,
    required this.iconSize,
    required this.label,
    required this.magnification,
    required this.maximumMagnification,
    required this.reducedMotion,
    required this.onHover,
    required this.onTap,
    required this.child,
    required this.axes,
    required this.crossExtent,
    required this.onGeometryChanged,
    this.onContextMenu,
    this.onReorder,
    this.onPoof,
    this.animateLaunch = true,
    this.showRunningIndicator = true,
    this.running = false,
    this.selected = false,
    this.launches = false,
    this.status = '',
  });
  final String id;
  final double iconSize;
  final String label;
  final String status;
  final double magnification;
  final double maximumMagnification;
  final bool reducedMotion;
  final bool animateLaunch;
  final bool showRunningIndicator;
  final bool running;
  final bool selected;
  final bool launches;
  final _DockAxes axes;
  final double crossExtent;
  final void Function(String id, String? label, Offset globalPosition) onHover;
  final VoidCallback onTap;
  final VoidCallback onGeometryChanged;
  final VoidCallback? onContextMenu;
  final void Function(String draggedId, bool isAfter)? onReorder;
  final ValueChanged<Offset>? onPoof;
  final Widget child;
  @override
  State<_DockItem> createState() => _DockItemState();
}

class _DockItemState extends State<_DockItem>
    with SingleTickerProviderStateMixin {
  late final AnimationController _bounce;
  bool? _dropAfter;

  Widget _buildInsertionIndicator(bool isAfter) {
    final horizontal = widget.axes.horizontal;
    return Positioned(
      left: horizontal ? (isAfter ? null : -3.0) : 4.0,
      right: horizontal ? (isAfter ? -3.0 : null) : 4.0,
      top: horizontal ? 4.0 : (isAfter ? null : -3.0),
      bottom: horizontal ? 4.0 : (isAfter ? -3.0 : null),
      width: horizontal ? 3.5 : null,
      height: horizontal ? null : 3.5,
      child: IgnorePointer(
        child: DecoratedBox(
          decoration: BoxDecoration(
            color: const Color(0xFF0A84FF),
            borderRadius: BorderRadius.circular(2.0),
            boxShadow: const [
              BoxShadow(
                color: Color(0x990A84FF),
                blurRadius: 5.0,
                spreadRadius: 1.0,
              ),
            ],
          ),
        ),
      ),
    );
  }

  late final Animation<double> _lift;
  final _iconKey = GlobalKey();
  Rect? iconRect(RenderBox root) {
    final icon = _iconKey.currentContext?.findRenderObject();
    if (icon is! RenderBox || !icon.hasSize) return null;
    return MatrixUtils.transformRect(
      icon.getTransformTo(root),
      Offset.zero & icon.size,
    );
  }

  @override
  void initState() {
    super.initState();
    _bounce = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 620),
    );
    _bounce.addStatusListener(_bounceStatusChanged);
    _lift = TweenSequence<double>([
      TweenSequenceItem(
        tween: Tween(
          begin: 0.0,
          end: 1.0,
        ).chain(CurveTween(curve: Curves.easeOut)),
        weight: 26,
      ),
      TweenSequenceItem(
        tween: Tween(
          begin: 1.0,
          end: 0.0,
        ).chain(CurveTween(curve: Curves.easeIn)),
        weight: 30,
      ),
      TweenSequenceItem(
        tween: Tween(
          begin: 0.0,
          end: .22,
        ).chain(CurveTween(curve: Curves.easeOut)),
        weight: 19,
      ),
      TweenSequenceItem(
        tween: Tween(
          begin: .22,
          end: 0.0,
        ).chain(CurveTween(curve: Curves.easeIn)),
        weight: 25,
      ),
    ]).animate(_bounce);
  }

  void _bounceStatusChanged(AnimationStatus status) {
    if (status == AnimationStatus.completed ||
        status == AnimationStatus.dismissed) {
      widget.onGeometryChanged();
    }
  }

  @override
  void didUpdateWidget(covariant _DockItem oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.reducedMotion ||
        !widget.animateLaunch ||
        widget.axes.position != oldWidget.axes.position) {
      _bounce.stop();
      _bounce.value = 0;
    }
  }

  @override
  void dispose() {
    _bounce.removeStatusListener(_bounceStatusChanged);
    _bounce.dispose();
    super.dispose();
  }

  void _hover(bool entered) {
    final box = context.findRenderObject();
    final center = box is RenderBox
        ? box.localToGlobal(box.size.center(Offset.zero))
        : Offset.zero;
    widget.onHover(widget.id, entered ? widget.label : null, center);
  }

  void _tap() {
    if (widget.launches &&
        widget.animateLaunch &&
        !widget.reducedMotion &&
        !_bounce.isAnimating) {
      // A second launch request must not pull an airborne icon to its origin.
      _bounce.forward(from: 0);
    }
    widget.onTap();
  }

  @override
  Widget build(BuildContext context) {
    final dark = Theme.of(context).brightness == Brightness.dark;
    Widget icon = Material(
      key: _iconKey,
      color: Colors.transparent,
      child: InkWell(
        key: ValueKey(switch (widget.id) {
          _MacDockViewState._finder => 'dock-finder',
          _MacDockViewState._launcher => 'dock-launcher',
          _MacDockViewState._overview => 'dock-overview',
          _MacDockViewState._downloads => 'dock-downloads',
          _MacDockViewState._trash => 'dock-trash',
          _ =>
            widget.id.startsWith('window-')
                ? 'dock-${widget.id}'
                : 'dock-app-${widget.id}',
        }),
        borderRadius: BorderRadius.circular(8),
        onTap: _tap,
        onSecondaryTapUp: widget.onContextMenu == null
            ? null
            : (_) => widget.onContextMenu!(),
        onLongPress: widget.onReorder == null ? widget.onContextMenu : null,
        child: SizedBox.square(
          key: ValueKey('dock-icon-size-${widget.id}'),
          dimension: widget.iconSize,
          child: FittedBox(
            fit: BoxFit.contain,
            child: RepaintBoundary(
              child: SizedBox.square(dimension: 48, child: widget.child),
            ),
          ),
        ),
      ),
    );
    if (widget.onReorder != null) {
      final dragChild = icon;
      // Pointer motion rebuilds every item each frame; the duplicated icon
      // tree is only needed once a drag actually starts.
      final feedbackWidget = DragFeedbackBuilder(
        builder: (context) => Material(
          color: Colors.transparent,
          child: Transform.scale(
            scale: 1.15,
            child: Container(
              decoration: const BoxDecoration(
                boxShadow: [
                  BoxShadow(
                    color: Color(0x55000000),
                    blurRadius: 18,
                    spreadRadius: 2,
                    offset: Offset(0, 8),
                  ),
                ],
              ),
              child: SizedBox.square(
                dimension: widget.iconSize,
                child: widget.child,
              ),
            ),
          ),
        ),
      );
      final draggingPlaceholder = Opacity(opacity: .25, child: dragChild);
      void handleDragCanceled(Velocity velocity, Offset offset) {
        final box =
            _iconKey.currentContext?.findRenderObject() ??
            context.findRenderObject();
        if (box is RenderBox && box.hasSize) {
          final itemOrigin = box.localToGlobal(Offset.zero);
          final outward = switch (widget.axes.position) {
            MacDockPosition.bottom => itemOrigin.dy - offset.dy,
            MacDockPosition.left =>
              offset.dx - (itemOrigin.dx + box.size.width),
            MacDockPosition.right => itemOrigin.dx - offset.dx,
          };
          if (outward >= 48.0) {
            widget.onPoof?.call(offset);
          }
        }
      }

      icon = DragTarget<String>(
        onWillAcceptWithDetails: (details) => details.data != widget.id,
        onMove: (details) {
          final box = context.findRenderObject();
          if (box is RenderBox && box.hasSize) {
            final local = box.globalToLocal(details.offset);
            final isAfter = widget.axes.horizontal
                ? local.dx > box.size.width / 2
                : local.dy > box.size.height / 2;
            if (_dropAfter != isAfter) {
              setState(() => _dropAfter = isAfter);
            }
          }
        },
        onLeave: (_) {
          if (_dropAfter != null) {
            setState(() => _dropAfter = null);
          }
        },
        onAcceptWithDetails: (details) {
          final box = context.findRenderObject();
          final bool isAfter;
          if (box is RenderBox && box.hasSize) {
            final local = box.globalToLocal(details.offset);
            isAfter = widget.axes.horizontal
                ? local.dx > box.size.width / 2
                : local.dy > box.size.height / 2;
          } else {
            isAfter = _dropAfter ?? false;
          }
          setState(() => _dropAfter = null);
          widget.onReorder?.call(details.data, isAfter);
        },
        builder: (context, candidates, rejected) {
          final showIndicator = candidates.isNotEmpty;
          final isAfter = _dropAfter ?? false;
          final touchDraggable = LongPressDraggable<String>(
            data: widget.id,
            delay: const Duration(milliseconds: 350),
            maxSimultaneousDrags: 1,
            dragAnchorStrategy: pointerDragAnchorStrategy,
            feedback: feedbackWidget,
            childWhenDragging: draggingPlaceholder,
            onDraggableCanceled: handleDragCanceled,
            child: Stack(
              clipBehavior: Clip.none,
              children: [
                dragChild,
                if (showIndicator) _buildInsertionIndicator(isAfter),
              ],
            ),
          );
          return _MouseDockDraggable(
            data: widget.id,
            feedback: feedbackWidget,
            childWhenDragging: draggingPlaceholder,
            onDraggableCanceled: handleDragCanceled,
            child: touchDraggable,
          );
        },
      );
    }
    return DockMotionBuilder(
      target: widget.magnification,
      instant: widget.reducedMotion,
      lowerBound: 1,
      upperBound: widget.maximumMagnification,
      onEnd: widget.onGeometryChanged,
      child: icon,
      builder: (context, scale, child) {
        widget.onGeometryChanged();
        final extent = (widget.iconSize + 10) * scale;
        final size = widget.axes.size(extent, widget.crossExtent);
        // Transform the complete interactive subtree so the drag targets and
        // gesture recognizers share the painted icon's enlarged hit geometry.
        final animatedIcon = AnimatedBuilder(
          animation: _bounce,
          child: Transform.translate(
            offset: widget.axes.inward((scale - 1) * 12),
            child: Transform.scale(
              key: ValueKey('dock-scale-${widget.label}'),
              scale: scale,
              alignment: widget.axes.anchor,
              child: child,
            ),
          ),
          builder: (context, child) {
            widget.onGeometryChanged();
            return Transform.translate(
              key: ValueKey('dock-bounce-${widget.id}'),
              offset: widget.axes.inward(
                widget.reducedMotion ? 0 : 12 * _lift.value,
              ),
              child: child,
            );
          },
        );
        return SizedBox.fromSize(
          size: size,
          child: Semantics(
            label: widget.label,
            value: widget.status,
            button: true,
            selected: widget.selected,
            child: CallbackShortcuts(
              bindings: {
                if (widget.onContextMenu != null) ...{
                  const SingleActivator(LogicalKeyboardKey.contextMenu):
                      widget.onContextMenu!,
                  const SingleActivator(LogicalKeyboardKey.f10, shift: true):
                      widget.onContextMenu!,
                },
              },
              child: MouseRegion(
                onEnter: (_) => _hover(true),
                onExit: (_) => _hover(false),
                child: Stack(
                  clipBehavior: Clip.none,
                  children: [
                    Positioned.fromRect(
                      rect: widget.axes.rect(
                        size,
                        (extent - widget.iconSize) / 2,
                        13,
                        widget.iconSize,
                        widget.iconSize,
                      ),
                      child: animatedIcon,
                    ),
                    if (widget.running && widget.showRunningIndicator)
                      Positioned.fromRect(
                        rect: widget.axes.rect(
                          size,
                          extent / 2 - 2.25,
                          3.5,
                          4.5,
                          4.5,
                        ),
                        child: Container(
                          key: ValueKey('dock-running-${widget.label}'),
                          decoration: BoxDecoration(
                            color: dark
                                ? const Color(0xffffffff)
                                : const Color(0xff333336),
                            shape: BoxShape.circle,
                            boxShadow: [
                              if (dark)
                                BoxShadow(
                                  color: Colors.white.withValues(alpha: .75),
                                  blurRadius: 3.5,
                                  spreadRadius: 0.5,
                                )
                              else ...[
                                BoxShadow(
                                  color: Colors.white.withValues(alpha: .60),
                                  offset: const Offset(0, 0.75),
                                  blurRadius: 0.5,
                                ),
                                BoxShadow(
                                  color: Colors.black.withValues(alpha: .20),
                                  blurRadius: 1.5,
                                  offset: const Offset(0, -0.5),
                                ),
                              ],
                            ],
                          ),
                        ),
                      ),
                  ],
                ),
              ),
            ),
          ),
        );
      },
    );
  }
}

class _DockDivider extends StatelessWidget {
  const _DockDivider({
    super.key,
    required this.id,
    required this.iconSize,
    required this.axes,
    required this.crossExtent,
    required this.preferences,
    required this.onResize,
    this.onContextMenu,
    this.onAppDropped,
  });
  final String id;
  final double iconSize;
  final _DockAxes axes;
  final double crossExtent;
  final MacShellPreferences preferences;
  final ValueChanged<double> onResize;
  final void Function(RenderBox box)? onContextMenu;
  final ValueChanged<String>? onAppDropped;

  @override
  Widget build(BuildContext context) {
    final size = axes.size(14, crossExtent);
    final cursor = axes.horizontal
        ? SystemMouseCursors.resizeUpDown
        : SystemMouseCursors.resizeLeftRight;
    void openContext() {
      final box = context.findRenderObject();
      if (box is RenderBox && box.hasSize) {
        onContextMenu?.call(box);
      }
    }

    final dividerBody = Semantics(
      label: '程序坞分隔条',
      button: true,
      child: CallbackShortcuts(
        bindings: {
          if (onContextMenu != null) ...{
            const SingleActivator(LogicalKeyboardKey.contextMenu): openContext,
            const SingleActivator(LogicalKeyboardKey.f10, shift: true):
                openContext,
          },
        },
        child: MouseRegion(
          cursor: cursor,
          child: GestureDetector(
            behavior: HitTestBehavior.opaque,
            onPanUpdate: (details) {
              final inward = switch (axes.position) {
                MacDockPosition.bottom => -details.delta.dy,
                MacDockPosition.left => details.delta.dx,
                MacDockPosition.right => -details.delta.dx,
              };
              final newSize = (preferences.dockSize + inward)
                  .clamp(32.0, 64.0)
                  .roundToDouble();
              if (newSize != preferences.dockSize) {
                onResize(newSize);
              }
            },
            onSecondaryTapUp: onContextMenu == null
                ? null
                : (_) => openContext(),
            child: SizedBox.fromSize(
              size: size,
              child: Stack(
                children: [
                  Positioned.fromRect(
                    rect: axes.rect(size, 6.0, 12, 2.0, iconSize * .75 + 2),
                    child: Stack(
                      key: ValueKey('dock-divider-line-$id'),
                      children: [
                        Positioned(
                          left: 0,
                          top: 0,
                          width: axes.horizontal ? 1.0 : null,
                          height: axes.horizontal ? null : 1.0,
                          right: axes.horizontal ? null : 0,
                          bottom: axes.horizontal ? 0 : null,
                          child: ColoredBox(
                            color:
                                Theme.of(context).brightness == Brightness.dark
                                ? Colors.black.withValues(alpha: .40)
                                : Colors.black.withValues(alpha: .22),
                          ),
                        ),
                        Positioned(
                          left: axes.horizontal ? 1.0 : 0,
                          top: axes.horizontal ? 0 : 1.0,
                          width: axes.horizontal ? 1.0 : null,
                          height: axes.horizontal ? null : 1.0,
                          right: axes.horizontal ? null : 0,
                          bottom: axes.horizontal ? 0 : null,
                          child: ColoredBox(
                            color:
                                Theme.of(context).brightness == Brightness.dark
                                ? Colors.white.withValues(alpha: .18)
                                : Colors.white.withValues(alpha: .45),
                          ),
                        ),
                      ],
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
    if (onAppDropped != null) {
      return DragTarget<String>(
        onWillAcceptWithDetails: (details) => true,
        onAcceptWithDetails: (details) => onAppDropped!(details.data),
        builder: (context, candidates, rejected) {
          if (candidates.isEmpty) return dividerBody;
          final horizontal = axes.horizontal;
          return Stack(
            clipBehavior: Clip.none,
            children: [
              dividerBody,
              Positioned(
                left: horizontal ? -1.0 : 4.0,
                right: horizontal ? null : 4.0,
                top: horizontal ? 4.0 : -1.0,
                bottom: horizontal ? 4.0 : null,
                width: horizontal ? 3.5 : null,
                height: horizontal ? null : 3.5,
                child: IgnorePointer(
                  child: DecoratedBox(
                    decoration: BoxDecoration(
                      color: const Color(0xFF0A84FF),
                      borderRadius: BorderRadius.circular(2.0),
                      boxShadow: const [
                        BoxShadow(
                          color: Color(0x990A84FF),
                          blurRadius: 5.0,
                          spreadRadius: 1.0,
                        ),
                      ],
                    ),
                  ),
                ),
              ),
            ],
          );
        },
      );
    }
    return dividerBody;
  }
}

class _DockGlass extends StatelessWidget {
  const _DockGlass({super.key, required this.dark, this.onContextMenu});
  final bool dark;
  final void Function(RenderBox box)? onContextMenu;
  @override
  Widget build(BuildContext context) => RepaintBoundary(
    child: GestureDetector(
      behavior: HitTestBehavior.translucent,
      onSecondaryTapUp: onContextMenu == null
          ? null
          : (details) {
              final box = context.findRenderObject();
              if (box is RenderBox && box.hasSize) {
                onContextMenu!(box);
              }
            },
      child: Container(
        decoration: BoxDecoration(
          gradient: LinearGradient(
            begin: Alignment.topCenter,
            end: Alignment.bottomCenter,
            colors: dark
                ? [
                    const Color(0xff2a2a2e).withValues(alpha: .72),
                    const Color(0xff161618).withValues(alpha: .60),
                  ]
                : [
                    const Color(0xffffffff).withValues(alpha: .76),
                    const Color(0xffe8e8ec).withValues(alpha: .62),
                  ],
          ),
          borderRadius: BorderRadius.circular(22),
          border: Border.all(
            color: dark
                ? Colors.white.withValues(alpha: .22)
                : Colors.white.withValues(alpha: .80),
            width: .6,
          ),
          boxShadow: [
            BoxShadow(
              color: Colors.black.withValues(alpha: dark ? .40 : .16),
              blurRadius: 28,
              spreadRadius: 1,
              offset: const Offset(0, 8),
            ),
            BoxShadow(
              color: Colors.black.withValues(alpha: dark ? .22 : .08),
              blurRadius: 8,
              offset: const Offset(0, 2),
            ),
          ],
        ),
      ),
    ),
  );
}

class _PoofInstance {
  const _PoofInstance({required this.id, required this.offset});
  final int id;
  final Offset offset;
}

class _PoofEffect extends StatefulWidget {
  const _PoofEffect({required this.onComplete});
  final VoidCallback onComplete;

  @override
  State<_PoofEffect> createState() => _PoofEffectState();
}

class _PoofEffectState extends State<_PoofEffect>
    with SingleTickerProviderStateMixin {
  late final AnimationController _controller;

  @override
  void initState() {
    super.initState();
    _controller = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 320),
    );
    _controller.forward().whenComplete(() {
      if (mounted) widget.onComplete();
    });
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => AnimatedBuilder(
    animation: _controller,
    builder: (context, _) => CustomPaint(
      size: const Size(64, 64),
      painter: _PoofCloudPainter(progress: _controller.value),
    ),
  );
}

class _PoofCloudPainter extends CustomPainter {
  const _PoofCloudPainter({required this.progress});
  final double progress;

  @override
  void paint(Canvas canvas, Size size) {
    if (progress <= 0 || progress >= 1.0) return;
    final center = Offset(size.width / 2, size.height / 2);
    final alpha = ((1.0 - progress) * 0.9).clamp(0.0, 1.0);
    final fillPaint = Paint()
      ..color = Colors.white.withValues(alpha: alpha)
      ..style = PaintingStyle.fill;
    final outlinePaint = Paint()
      ..color = const Color(0xffb0b0b8).withValues(alpha: alpha * 0.5)
      ..style = PaintingStyle.stroke
      ..strokeWidth = 0.8;

    const count = 6;
    final distance = 6.0 + progress * 22.0;
    final puffRadius = (8.0 + progress * 7.0) * (1.0 - progress * 0.35);

    final centerRadius = (10.0 + progress * 6.0) * (1.0 - progress * 0.4);
    canvas.drawCircle(center, centerRadius, fillPaint);
    canvas.drawCircle(center, centerRadius, outlinePaint);

    for (var i = 0; i < count; i++) {
      final angle = (i * 2 * math.pi / count) + (progress * 0.2);
      final offset =
          center +
          Offset(math.cos(angle) * distance, math.sin(angle) * distance);
      canvas.drawCircle(offset, puffRadius, fillPaint);
      canvas.drawCircle(offset, puffRadius, outlinePaint);
    }

    final dropDistance = 10.0 + progress * 28.0;
    final dropRadius = (2.2 * (1.0 - progress)).clamp(0.0, 3.0);
    if (dropRadius > 0.4) {
      final dropPaint = Paint()
        ..color = const Color(0xffd8d8de).withValues(alpha: alpha * 0.7)
        ..style = PaintingStyle.fill;
      for (var i = 0; i < 4; i++) {
        final angle = (i * 2 * math.pi / 4) + (math.pi / 4);
        final offset =
            center +
            Offset(
              math.cos(angle) * dropDistance,
              math.sin(angle) * dropDistance,
            );
        canvas.drawCircle(offset, dropRadius, dropPaint);
      }
    }
  }

  @override
  bool shouldRepaint(covariant _PoofCloudPainter oldDelegate) =>
      oldDelegate.progress != progress;
}

class _MinimizedWindowIcon extends StatelessWidget {
  const _MinimizedWindowIcon({
    required this.window,
    this.app,
    required this.rasterSize,
    required this.useOriginalIcons,
  });

  final ShellWindow window;
  final DesktopApp? app;
  final double rasterSize;
  final bool useOriginalIcons;

  @override
  Widget build(BuildContext context) {
    final dark = Theme.of(context).brightness == Brightness.dark;
    final parentApp = app;
    return RepaintBoundary(
      child: SizedBox.square(
        dimension: 48,
        child: Stack(
          clipBehavior: Clip.none,
          children: [
            Positioned(
              left: 2,
              top: 4,
              right: 6,
              bottom: 4,
              child: Container(
                decoration: BoxDecoration(
                  color: dark
                      ? const Color(0xff222225)
                      : const Color(0xfff7f7f9),
                  borderRadius: BorderRadius.circular(4),
                  border: Border.all(
                    color: dark
                        ? Colors.white.withValues(alpha: 0.15)
                        : Colors.black.withValues(alpha: 0.12),
                    width: 0.6,
                  ),
                  boxShadow: [
                    BoxShadow(
                      color: Colors.black.withValues(alpha: dark ? 0.35 : 0.18),
                      blurRadius: 5,
                      offset: const Offset(0, 2),
                    ),
                  ],
                ),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    Container(
                      height: 8,
                      padding: const EdgeInsets.symmetric(horizontal: 4),
                      decoration: BoxDecoration(
                        color: dark
                            ? const Color(0xff2d2d32)
                            : const Color(0xffe8e8ec),
                        borderRadius: const BorderRadius.vertical(
                          top: Radius.circular(3.4),
                        ),
                      ),
                      child: Row(
                        children: [
                          Container(
                            width: 2.8,
                            height: 2.8,
                            decoration: const BoxDecoration(
                              color: Color(0xffff5f56),
                              shape: BoxShape.circle,
                            ),
                          ),
                          const SizedBox(width: 2),
                          Container(
                            width: 2.8,
                            height: 2.8,
                            decoration: const BoxDecoration(
                              color: Color(0xffffbd2e),
                              shape: BoxShape.circle,
                            ),
                          ),
                          const SizedBox(width: 2),
                          Container(
                            width: 2.8,
                            height: 2.8,
                            decoration: const BoxDecoration(
                              color: Color(0xff27c93f),
                              shape: BoxShape.circle,
                            ),
                          ),
                        ],
                      ),
                    ),
                    Expanded(
                      child: Container(
                        padding: const EdgeInsets.all(4),
                        alignment: Alignment.center,
                        child: Text(
                          window.title.isNotEmpty ? window.title : window.appId,
                          maxLines: 2,
                          overflow: TextOverflow.ellipsis,
                          textAlign: TextAlign.center,
                          style: TextStyle(
                            fontSize: 7,
                            fontWeight: FontWeight.w400,
                            color: dark ? Colors.white60 : Colors.black54,
                            height: 1.1,
                          ),
                        ),
                      ),
                    ),
                  ],
                ),
              ),
            ),
            if (parentApp != null)
              Positioned(
                right: 0,
                bottom: 0,
                width: 20,
                height: 20,
                child: Container(
                  padding: const EdgeInsets.all(1.5),
                  decoration: BoxDecoration(
                    color: dark ? const Color(0xff1e1e20) : Colors.white,
                    borderRadius: BorderRadius.circular(5),
                    boxShadow: [
                      BoxShadow(
                        color: Colors.black.withValues(alpha: 0.28),
                        blurRadius: 3,
                        offset: const Offset(0, 1),
                      ),
                    ],
                  ),
                  child: MacAppIcon(
                    app: parentApp,
                    size: 17,
                    rasterSize: rasterSize,
                    useOriginalIcons: useOriginalIcons,
                  ),
                ),
              ),
          ],
        ),
      ),
    );
  }
}
