// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';

import 'optional_asset_icon.dart';

import 'package:flutter/services.dart';

import '../mac_launchpad_layout.dart';
import '../mac_shell_preferences.dart';
import '../models.dart';
import '../state/shell_controller.dart';
import 'drag_feedback.dart';
import 'mac_app_icon.dart';
import 'shell_geometry.dart';

/// A separate Launchpad surface. Material's launcher remains independent.
class MacLauncherPanel extends StatefulWidget {
  const MacLauncherPanel({super.key, required this.controller, this.layout});
  final ShellController controller;
  final MacLaunchpadLayout? layout;
  @override
  State<MacLauncherPanel> createState() => _MacLauncherPanelState();
}

/// Mouse rearranging starts with pointer movement; touch keeps its long press
/// so a horizontal swipe can still win the gesture arena and change pages.
class _MouseLaunchpadDraggable extends Draggable<String> {
  const _MouseLaunchpadDraggable({
    required super.data,
    required super.feedback,
    required super.childWhenDragging,
    required super.onDragStarted,
    required super.onDragEnd,
    required super.onDragCompleted,
    required super.onDraggableCanceled,
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

class _MacLauncherPanelState extends State<MacLauncherPanel>
    with SingleTickerProviderStateMixin {
  late MacLaunchpadLayout _layout;
  bool _ownsLayout = false;
  final _tileKeys = <String, GlobalKey>{};
  final _query = TextEditingController();
  final _search = FocusNode(debugLabel: 'mac-launchpad-search');
  final _keyboard = FocusNode(debugLabel: 'mac-launchpad');
  final _nameEdit = TextEditingController();
  final _nameFocus = FocusNode(debugLabel: 'mac-launchpad-folder-name');
  String? _editingFolder;
  final _pages = PageController();
  final _dots = ScrollController();
  String? _folder;
  String? _selected;
  int _rootPage = 0;
  int _rootCapacity = 35;
  int _rootColumns = 7;
  String? _rootSelected;
  int _searchReturnPage = 0;
  bool _wasSearching = false;
  final _folderPanelKey = GlobalKey();
  String? _draggingId;
  String? _dragOriginFolder;
  bool _dragCancelled = false;
  int? _dragDirection;
  int _page = 0;
  int _capacity = 35;
  int _columns = 7;
  int _pageCount = 1;
  List<String> _previousIds = const [];
  List<MacLaunchpadEntry>? _idEntries;
  List<String> _entryIds = const [];
  Timer? _wheelReset;
  Timer? _dragPageTimer;
  double _wheelDelta = 0;
  int _wheelGestureSign = 0;
  bool _wheelPageConsumed = false;
  static const _macPageCurve = Cubic(0.25, 0.1, 0.25, 1.0);
  bool _jiggleMode = false;
  late final AnimationController _jiggleClock;
  bool _inheritedReduced = false;
  bool get _reduced =>
      widget.controller.reduceMotion || MediaQuery.disableAnimationsOf(context);
  late List<DesktopApp> _lastApps;
  late MacShellPreferences _lastPreferences;
  late bool _lastReduced;
  late ShellPanel _lastPanel;
  List<DesktopApp>? _indexedCatalogue;
  Map<String, DesktopApp> _appIndex = const {};
  List<MacLaunchpadEntry>? _rootEntries;
  List<MacLaunchpadEntry>? _visibleEntries;
  String? _entriesQuery;
  String? _entriesFolder;
  List<DesktopApp>? _entriesCatalogue;
  bool get _searching => _query.text.trim().isNotEmpty;
  Map<String, DesktopApp> get _apps {
    // The controller replaces its immutable catalogue when applications change.
    // Tiles, folder previews and drag feedback share one index per catalogue.
    final catalogue = widget.controller.apps;
    if (!identical(catalogue, _indexedCatalogue)) {
      _appIndex = {for (final app in catalogue) app.id: app};
      _indexedCatalogue = catalogue;
    }
    return _appIndex;
  }

  List<MacLaunchpadEntry> get _roots => _rootEntries ??= _layout.entries;

  List<MacLaunchpadEntry> get _entries {
    final query = _query.text.trim();
    final catalogue = widget.controller.apps;
    if (_visibleEntries != null &&
        _entriesQuery == query &&
        _entriesFolder == _folder &&
        identical(_entriesCatalogue, catalogue)) {
      return _visibleEntries!;
    }
    _entriesQuery = query;
    _entriesFolder = _folder;
    _entriesCatalogue = catalogue;
    if (query.isNotEmpty) {
      final apps = _apps;
      return _visibleEntries = [
        for (final entry in _roots)
          for (final id in entry.appIds)
            if (apps[id]?.matches(query) ?? false) MacLaunchpadEntry.app(id),
      ];
    }
    final folder = _folder == null ? null : _layout.find(_folder!);
    if (folder != null && folder.isFolder) {
      return _visibleEntries = folder.appIds
          .map(MacLaunchpadEntry.app)
          .toList();
    }
    return _visibleEntries = _roots;
  }

  void _invalidateEntries() {
    _rootEntries = null;
    _visibleEntries = null;
  }

  void _setJiggleMode(bool value) {
    _jiggleMode = value;
    _syncJiggleClock();
  }

  void _syncJiggleClock() {
    final active =
        _jiggleMode &&
        !_inheritedReduced &&
        !widget.controller.reduceMotion &&
        widget.controller.panel == ShellPanel.launcher;
    if (active) {
      if (!_jiggleClock.isAnimating) {
        _jiggleClock.repeat();
      }
    } else {
      _jiggleClock.stop();
    }
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _inheritedReduced = MediaQuery.disableAnimationsOf(context);
    _syncJiggleClock();
  }

  @override
  void initState() {
    super.initState();
    _jiggleClock = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 440),
    );
    _ownsLayout = widget.layout == null;
    _layout =
        widget.layout ?? MacLaunchpadLayout(persist: !widget.controller.demo);
    _lastApps = widget.controller.apps;
    _lastPreferences = widget.controller.macPreferences;
    _lastReduced = widget.controller.reduceMotion;
    _lastPanel = widget.controller.panel;
    _layout.addListener(_changed);
    _nameFocus.addListener(_nameFocusChanged);
    widget.controller.addListener(_catalogChanged);
    _loadCatalog();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _search.requestFocus();
    });
  }

  void _resetWheelState() {
    _wheelReset?.cancel();
    _wheelReset = null;
    _wheelDelta = 0;
    _wheelGestureSign = 0;
    _wheelPageConsumed = false;
  }

  void _catalogChanged() {
    final panel = widget.controller.panel;
    var interactionChanged = false;
    if (panel != _lastPanel) {
      _lastPanel = panel;
      if (panel != ShellPanel.launcher) {
        interactionChanged = _jiggleMode;
        _setJiggleMode(false);
        _dragCancelled = true;
        _stopDragPaging();
        _resetWheelState();
      }
    }
    final apps = widget.controller.apps;
    final preferences = widget.controller.macPreferences;
    final reduced = widget.controller.reduceMotion;
    if (identical(apps, _lastApps) &&
        preferences == _lastPreferences &&
        reduced == _lastReduced) {
      if (interactionChanged && mounted) setState(() {});
      return;
    }
    final catalogueChanged = !identical(apps, _lastApps);
    _lastApps = apps;
    _lastPreferences = preferences;
    _lastReduced = reduced;
    _syncJiggleClock();
    if (catalogueChanged) {
      _invalidateEntries();
      _layout.reconcile(apps.map((app) => app.id));
      final validIds = {
        ...apps.map((app) => app.id),
        ..._roots.map((entry) => entry.id),
      };
      _tileKeys.removeWhere((id, _) => !validIds.contains(id));
    }
    if (mounted) setState(() {});
  }

  void _loadCatalog() {
    final catalog = widget.controller.apps.map((app) => app.id);
    if (_layout.loaded) {
      _layout.reconcile(catalog);
    } else {
      unawaited(_layout.load(catalog));
    }
  }

  void _changed() {
    _invalidateEntries();
    if (!mounted) return;
    if (_folder != null && !(_layout.find(_folder!)?.isFolder ?? false)) {
      // Catalog reconciliation can remove the open folder's last member.
      // Its page number belongs to the folder; restore the saved root page
      // before the next root-grid reconciliation safely clamps that page.
      _folder = null;
      _editingFolder = null;
      _page = _rootPage;
      _selected = _rootSelected;
      _previousIds = const [];
      _dragCancelled = true;
      _stopDragPaging();
      _nameFocus.unfocus();
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted &&
            _folder == null &&
            widget.controller.panel == ShellPanel.launcher) {
          _search.requestFocus();
        }
      });
    }
    setState(() {});
  }

  @override
  void didUpdateWidget(covariant MacLauncherPanel oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!identical(oldWidget.layout, widget.layout)) {
      _layout.removeListener(_changed);
      if (_ownsLayout) _layout.dispose();
      _ownsLayout = widget.layout == null;
      _layout =
          widget.layout ?? MacLaunchpadLayout(persist: !widget.controller.demo);
      _layout.addListener(_changed);
      _invalidateEntries();
      _folder = null;
      _editingFolder = null;
      _selected = null;
      _previousIds = const [];
      _query.clear();
      _wasSearching = false;
      _searchReturnPage = 0;
      _nameFocus.unfocus();
      _resetWheelState();
      _dragPageTimer?.cancel();
      _loadCatalog();
    }
    if (oldWidget.controller != widget.controller) {
      oldWidget.controller.removeListener(_catalogChanged);
      widget.controller.addListener(_catalogChanged);
      _catalogChanged();
    }
  }

  @override
  void dispose() {
    widget.controller.removeListener(_catalogChanged);
    _jiggleClock.dispose();
    _layout.removeListener(_changed);
    if (_editingFolder != null) {
      _layout.rename(_editingFolder!, _nameEdit.text);
    }
    if (_ownsLayout) _layout.dispose();
    _query.dispose();
    _search.dispose();
    _keyboard.dispose();
    _nameFocus.removeListener(_nameFocusChanged);
    _nameFocus.dispose();
    _nameEdit.dispose();
    _pages.dispose();
    _dots.dispose();
    _resetWheelState();
    _dragPageTimer?.cancel();
    super.dispose();
  }

  void _searchChanged(String _) {
    _stopDragPaging();
    _resetWheelState();
    final searching = _searching;
    if (!_wasSearching && searching) _searchReturnPage = _page;
    setState(() {
      _page = searching ? 0 : (_wasSearching ? _searchReturnPage : _page);
      _selected = null;
      _previousIds = const [];
      _wasSearching = searching;
    });
    if (_pages.hasClients) _pages.jumpToPage(_page);
  }

  void _reconcile(List<MacLaunchpadEntry> entries, int capacity) {
    if (!identical(entries, _idEntries)) {
      _idEntries = entries;
      _entryIds = entries.map((entry) => entry.id).toList();
    }
    final ids = _entryIds;
    final changed = _capacity != capacity || !listEquals(ids, _previousIds);
    _capacity = capacity;
    _pageCount = math.max(1, (entries.length / capacity).ceil());
    if (!changed) return;
    _previousIds = ids;
    final index = ids.indexOf(_selected ?? '');
    _selected = index >= 0 ? _selected : null;
    _page = index >= 0 ? index ~/ capacity : _page.clamp(0, _pageCount - 1);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted && _pages.hasClients) _pages.jumpToPage(_page);
    });
    _revealDot();
  }

  void _revealDot() {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || !_dots.hasClients) return;
      final offset =
          ((_page + .5) * 24 +
                  (_folder == null ? 0 : 48) -
                  _dots.position.viewportDimension / 2)
              .clamp(0.0, _dots.position.maxScrollExtent)
              .toDouble();
      _dots.jumpTo(offset);
    });
  }

  void _goPage(int page, {String? selected}) {
    final target = page.clamp(0, _pageCount - 1);
    setState(() {
      _page = target;
      _selected = selected;
    });
    _revealDot();
    if (!_pages.hasClients) return;
    if (_reduced) {
      _pages.jumpToPage(target);
    } else {
      unawaited(
        _pages.animateToPage(
          target,
          duration: const Duration(milliseconds: 260),
          curve: _macPageCurve,
        ),
      );
    }
  }

  void _activate(MacLaunchpadEntry entry) {
    if (widget.controller.panel != ShellPanel.launcher) return;
    if (_editingFolder != null) _commitRename();
    if (entry.isFolder) {
      _rootPage = _page;
      _rootCapacity = _capacity;
      _rootColumns = _columns;
      _rootSelected = _selected;
      setState(() {
        _folder = entry.id;
        _query.clear();
        _selected = null;
        _page = 0;
        _previousIds = const [];
      });
      if (_pages.hasClients) _pages.jumpToPage(0);
      _keyboard.requestFocus();
    } else {
      final app = _apps[entry.id];
      if (app != null && !widget.controller.busy.contains('launch')) {
        unawaited(widget.controller.launchApp(app));
      }
    }
  }

  void _closeFolder() {
    final folderId = _folder;
    if (_editingFolder != null) _commitRename();
    _stopDragPaging();
    final index = _roots.indexWhere((entry) => entry.id == folderId);
    setState(() {
      _folder = null;
      _selected = _rootSelected;
      _page = index < 0 ? _rootPage : index ~/ _rootCapacity;
      _previousIds = const [];
    });
    if (_pages.hasClients) _pages.jumpToPage(_page);
    _search.requestFocus();
  }

  KeyEventResult _key(FocusNode _, KeyEvent event) {
    if (widget.controller.panel != ShellPanel.launcher ||
        (event is! KeyDownEvent && event is! KeyRepeatEvent)) {
      return KeyEventResult.ignored;
    }
    final key = event.logicalKey;
    if (key == LogicalKeyboardKey.escape && _draggingId != null) {
      _dragCancelled = true;
      _stopDragPaging();
      return KeyEventResult.handled;
    }
    if (_editingFolder != null) {
      if (key == LogicalKeyboardKey.escape) {
        _cancelRename();
        return KeyEventResult.handled;
      }
      if (key == LogicalKeyboardKey.enter ||
          key == LogicalKeyboardKey.numpadEnter) {
        _commitRename();
        return KeyEventResult.handled;
      }
      return KeyEventResult.ignored;
    }
    if (key == LogicalKeyboardKey.escape) {
      if (_folder != null) {
        _closeFolder();
        return KeyEventResult.handled;
      }
      if (_jiggleMode) {
        setState(() => _setJiggleMode(false));
        return KeyEventResult.handled;
      }
      widget.controller.closePanel();
      return KeyEventResult.handled;
    }
    if (key == LogicalKeyboardKey.altLeft ||
        key == LogicalKeyboardKey.altRight) {
      if (event is KeyRepeatEvent) return KeyEventResult.handled;
      setState(() => _setJiggleMode(!_jiggleMode));
      return KeyEventResult.handled;
    }
    final entries = _entries;
    if (key == LogicalKeyboardKey.enter ||
        key == LogicalKeyboardKey.numpadEnter) {
      final item =
          entries.where((entry) => entry.id == _selected).firstOrNull ??
          entries.skip(_page * _capacity).firstOrNull;
      if (item != null) _activate(item);
      return KeyEventResult.handled;
    }
    if (key == LogicalKeyboardKey.pageDown ||
        key == LogicalKeyboardKey.pageUp) {
      _goPage(_page + (key == LogicalKeyboardKey.pageDown ? 1 : -1));
      return KeyEventResult.handled;
    }
    if (HardwareKeyboard.instance.isMetaPressed &&
        (key == LogicalKeyboardKey.arrowLeft ||
            key == LogicalKeyboardKey.arrowRight)) {
      _goPage(_page + (key == LogicalKeyboardKey.arrowRight ? 1 : -1));
      return KeyEventResult.handled;
    }
    if (_search.hasFocus &&
        _query.text.isNotEmpty &&
        (key == LogicalKeyboardKey.arrowLeft ||
            key == LogicalKeyboardKey.arrowRight)) {
      return KeyEventResult.ignored;
    }
    final delta = switch (key) {
      LogicalKeyboardKey.arrowLeft => -1,
      LogicalKeyboardKey.arrowRight => 1,
      LogicalKeyboardKey.arrowUp => -_columns,
      LogicalKeyboardKey.arrowDown => _columns,
      _ => 0,
    };
    if (delta == 0 || entries.isEmpty) return KeyEventResult.ignored;
    final old = entries.indexWhere((entry) => entry.id == _selected);
    final index = (old < 0 ? _page * _capacity : old + delta).clamp(
      0,
      entries.length - 1,
    );
    _goPage(index ~/ _capacity, selected: entries[index].id);
    return KeyEventResult.handled;
  }

  void _wheel(PointerSignalEvent event) {
    if (event is! PointerScrollEvent) return;
    // Claim this signal before PageView's Scrollable also changes its offset.
    GestureBinding.instance.pointerSignalResolver.register(event, (_) {
      _pageWheel(event);
    });
  }

  void _pageWheel(PointerScrollEvent event) {
    if (_pageCount < 2) return;

    final dx = event.scrollDelta.dx;
    final dy = event.scrollDelta.dy;

    // In macOS Catalina NSPageController: trackpad vertical swipe is ignored for horizontal paging;
    // mouse wheel vertical delta is translated to horizontal page turning.
    if (event.kind == PointerDeviceKind.trackpad && dx.abs() <= dy.abs()) {
      return;
    }

    final delta = dx.abs() > dy.abs() ? dx : dy;
    // 0.5px deadzone from macOS SkyLight _PKG2DVelocityFilter (0x287fc0)
    if (delta.abs() < 0.5) return;

    final packetSign = delta > 0 ? 1 : -1;
    // The inactivity timer owns the gesture boundary. Wall-clock gaps here
    // also include rendering stalls and clock corrections, which can turn a
    // single continuous stroke into several unwanted page changes.

    // Zero-latency direction reversal (macOS _PKG2DVelocityFilter 0x7df99):
    // If user reverses swipe direction, immediately drop old momentum and allow page turn.
    if (_wheelGestureSign != 0 && _wheelGestureSign != packetSign) {
      _wheelDelta = 0;
      _wheelPageConsumed = false;
      _wheelGestureSign = packetSign;
      _wheelReset?.cancel();
    }

    // Schedule inactivity reset timer (180ms)
    _wheelReset?.cancel();
    _wheelReset = Timer(const Duration(milliseconds: 180), () {
      _wheelDelta = 0;
      _wheelPageConsumed = false;
      _wheelGestureSign = 0;
    });

    // If current continuous swipe stream already consumed a page change in this direction,
    // prevent multiple runaway page turns during the remainder of the same stroke.
    if (_wheelPageConsumed) return;

    _wheelGestureSign = packetSign;
    _wheelDelta += delta;

    if (_wheelDelta.abs() < 30) return;

    _wheelPageConsumed = true;
    final target = (_page + (_wheelDelta > 0 ? 1 : -1)).clamp(
      0,
      _pageCount - 1,
    );
    _wheelDelta = 0;
    _goPage(target);
  }

  bool get _canDrop =>
      !_dragCancelled &&
      _draggingId != null &&
      !_searching &&
      widget.controller.panel == ShellPanel.launcher;

  void _stopDragPaging() {
    _dragDirection = null;
    _dragPageTimer?.cancel();
    _dragPageTimer = null;
  }

  void _startDrag(String id) {
    _stopDragPaging();
    _draggingId = id;
    _dragOriginFolder = _folder;
    _dragCancelled = false;
    // Reorder strips mount only for the drag's lifetime, so the panel must
    // rebuild once the drag becomes active.
    if (mounted) setState(() {});
  }

  void _endDrag() {
    _stopDragPaging();
    _draggingId = null;
    _dragOriginFolder = null;
    _dragCancelled = false;
    if (mounted) setState(() {});
  }

  void _scheduleDragPage(int direction) {
    _dragDirection = direction;
    _dragPageTimer?.cancel();
    _dragPageTimer = Timer(const Duration(milliseconds: 420), () {
      if (!mounted || !_canDrop || _dragDirection != direction) return;
      final next = _page + direction;
      if (next < 0 || next >= _pageCount) return;
      _goPage(next);
      _scheduleDragPage(direction);
    });
  }

  void _moveBefore(String id, String? before) {
    if (!_canDrop) return;
    final folder = _folder;
    if (folder == null) {
      _layout.moveBefore(id, before);
    } else {
      _layout.reorderInFolder(folder, id, before);
    }
  }

  Widget _dragEdge(int direction) => DragTarget<String>(
    key: ValueKey('mac-launchpad-edge-$direction'),
    onWillAcceptWithDetails: (_) {
      if (!_canDrop ||
          _page + direction < 0 ||
          _page + direction >= _pageCount) {
        return false;
      }
      _scheduleDragPage(direction);
      return true;
    },
    onLeave: (_) => _stopDragPaging(),
    onAcceptWithDetails: (details) {
      _stopDragPaging();
      final entries = _entries;
      final first = _page * _capacity;
      _moveBefore(
        details.data,
        first < entries.length ? entries[first].id : null,
      );
    },
    builder: (context, candidates, rejected) => ColoredBox(
      color: candidates.isEmpty
          ? Colors.transparent
          : Colors.white.withValues(alpha: .06),
    ),
  );

  Widget _icon(MacLaunchpadEntry entry, double size) {
    final apps = _apps;
    if (!entry.isFolder) {
      return MacAppIcon(
        app: apps[entry.id]!,
        size: size,
        useOriginalIcons: widget.controller.macPreferences.useOriginalIcons,
      );
    }
    return Container(
      key: ValueKey('mac-launchpad-folder-icon-${entry.id}'),
      width: size,
      height: size,
      padding: EdgeInsets.all(size * 0.11),
      decoration: BoxDecoration(
        color: const Color(0xff3a3a42).withValues(alpha: .48),
        borderRadius: BorderRadius.circular(size * 0.22),
        border: Border.all(
          color: Colors.white.withValues(alpha: .28),
          width: 0.8,
        ),
        boxShadow: [
          BoxShadow(
            color: Colors.black.withValues(alpha: 0.24),
            blurRadius: 8,
            offset: const Offset(0, 3),
          ),
        ],
      ),
      child: GridView.count(
        crossAxisCount: 3,
        padding: EdgeInsets.zero,
        mainAxisSpacing: math.max(2.0, size * 0.04),
        crossAxisSpacing: math.max(2.0, size * 0.04),
        physics: const NeverScrollableScrollPhysics(),
        children: [
          for (final id in entry.appIds.take(9))
            MacAppIcon(
              app: apps[id]!,
              size: size * 0.24,
              useOriginalIcons:
                  widget.controller.macPreferences.useOriginalIcons,
            ),
        ],
      ),
    );
  }

  Widget _tile(
    MacLaunchpadEntry entry,
    double iconSize, {
    bool interactive = true,
  }) {
    final selected = _selected == entry.id;
    final title = entry.isFolder ? entry.name : _apps[entry.id]!.name;
    final child = Semantics(
      key: _tileKeys.putIfAbsent(entry.id, GlobalKey.new),
      label: title,
      button: true,
      selected: selected,
      child: GestureDetector(
        key: ValueKey(
          entry.isFolder
              ? 'mac-launchpad-folder-${entry.id}'
              : 'app-${entry.id}',
        ),
        behavior: HitTestBehavior.opaque,
        onTap: () => _activate(entry),
        onLongPress: () {
          if (!_jiggleMode) {
            setState(() => _setJiggleMode(true));
          }
        },
        onSecondaryTapUp: entry.isFolder
            ? (_) => _beginRename(entry)
            : (_) => widget.controller.openDockAppMenu(entry.id),
        child: _LaunchpadJiggle(
          seed: entry.id.hashCode,
          clock: _jiggleClock,
          active: _jiggleMode,
          reduced: _reduced,
          child: Column(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              Stack(
                clipBehavior: Clip.none,
                children: [
                  Container(
                    key: ValueKey('mac-launchpad-selection-${entry.id}'),
                    padding: const EdgeInsets.all(4),
                    decoration: BoxDecoration(
                      borderRadius: BorderRadius.circular(18),
                      border: Border.all(
                        color: selected
                            ? Colors.white.withValues(alpha: .65)
                            : Colors.transparent,
                        width: 1.5,
                      ),
                    ),
                    child: _icon(entry, iconSize),
                  ),
                  if (_jiggleMode && !entry.isFolder)
                    Positioned(
                      top: -3,
                      left: -3,
                      child: _LaunchpadCloseBox(
                        onTap: () => _confirmDeleteApp(entry.id),
                      ),
                    ),
                ],
              ),
              const SizedBox(height: 4),
              Text(
                title,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                textAlign: TextAlign.center,
                style: const TextStyle(
                  color: Colors.white,
                  fontSize: 13,
                  fontWeight: FontWeight.w500,
                  letterSpacing: -0.1,
                  height: 1.2,
                  shadows: [
                    Shadow(
                      color: Colors.black87,
                      blurRadius: 4,
                      offset: Offset(0, 1.2),
                    ),
                  ],
                ),
              ),
            ],
          ),
        ),
      ),
    );
    if (!interactive || _searching) return child;
    final feedback = DragFeedbackBuilder(
      builder: (context) => Material(
        color: Colors.transparent,
        child: _icon(entry, iconSize),
      ),
    );
    Widget draggable(Widget tile) => _MouseLaunchpadDraggable(
      data: entry.id,
      feedback: feedback,
      childWhenDragging: Opacity(opacity: .3, child: tile),
      onDragStarted: () => _startDrag(entry.id),
      onDragEnd: (_) => _endDrag(),
      onDragCompleted: _endDrag,
      onDraggableCanceled: (_, _) => _endDrag(),
      child: LongPressDraggable<String>(
        data: entry.id,
        delay: const Duration(milliseconds: 350),
        maxSimultaneousDrags: 1,
        dragAnchorStrategy: pointerDragAnchorStrategy,
        feedback: feedback,
        onDragStarted: () => _startDrag(entry.id),
        onDragEnd: (_) => _endDrag(),
        onDragCompleted: _endDrag,
        onDraggableCanceled: (_, _) => _endDrag(),
        childWhenDragging: Opacity(opacity: .3, child: tile),
        child: tile,
      ),
    );
    return DragTarget<String>(
      onWillAcceptWithDetails: (details) =>
          _canDrop &&
          details.data != entry.id &&
          !(_layout.find(details.data)?.isFolder ?? false),
      onAcceptWithDetails: (details) {
        if (!_canDrop) return;
        if (_folder == null) {
          _layout.group(details.data, entry.id);
        } else {
          _layout.reorderInFolder(_folder!, details.data, entry.id);
        }
      },
      builder: (context, candidates, rejected) => draggable(
        DecoratedBox(
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(20),
            color: candidates.isEmpty
                ? Colors.transparent
                : Colors.white.withValues(alpha: .12),
          ),
          child: child,
        ),
      ),
    );
  }

  void _beginRename(MacLaunchpadEntry entry) {
    if (_folder != entry.id) _activate(entry);
    _nameEdit.text = entry.name;
    _nameEdit.selection = TextSelection(
      baseOffset: 0,
      extentOffset: entry.name.length,
    );
    setState(() => _editingFolder = entry.id);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted && _editingFolder == entry.id) _nameFocus.requestFocus();
    });
  }

  void _nameFocusChanged() {
    if (!_nameFocus.hasFocus && _editingFolder != null) _commitRename();
  }

  void _commitRename() {
    final id = _editingFolder;
    if (id == null || !mounted) return;
    setState(() => _editingFolder = null);
    _layout.rename(id, _nameEdit.text);
    _nameFocus.unfocus();
    _keyboard.requestFocus();
  }

  void _cancelRename() {
    if (!mounted) return;
    setState(() => _editingFolder = null);
    _nameFocus.unfocus();
    _keyboard.requestFocus();
  }

  bool _blankDrop(DragTargetDetails<String> details) {
    if (!_canDrop) return false;
    for (final entry in _entries) {
      final box = _tileKeys[entry.id]?.currentContext?.findRenderObject();
      if (box is RenderBox &&
          box.hasSize &&
          (box.localToGlobal(Offset.zero) & box.size).contains(
            details.offset,
          )) {
        return false;
      }
    }
    return true;
  }

  void _dropInBlank(DragTargetDetails<String> details) {
    if (!_blankDrop(details)) return;
    // Empty cells belong to this page, not to the end of the entire catalog.
    final entries = _entries;
    final nextPage = (_page + 1) * _capacity;
    _moveBefore(
      details.data,
      nextPage < entries.length ? entries[nextPage].id : null,
    );
  }

  bool _outsideFolder(Offset position) {
    final box = _folderPanelKey.currentContext?.findRenderObject();
    return box is RenderBox &&
        box.hasSize &&
        !(box.localToGlobal(Offset.zero) & box.size).contains(position);
  }

  void _extractFromFolder(DragTargetDetails<String> details) {
    if (!_canDrop ||
        _dragOriginFolder != _folder ||
        !_outsideFolder(details.offset)) {
      return;
    }
    // Use the visible root page's real cells to place the extracted app.
    String? before;
    final rootEntries = _roots;
    for (final entry
        in rootEntries.skip(_rootPage * _rootCapacity).take(_rootCapacity)) {
      final box = _tileKeys[entry.id]?.currentContext?.findRenderObject();
      if (box is RenderBox && box.hasSize) {
        final rect = box.localToGlobal(Offset.zero) & box.size;
        if (details.offset.dy < rect.top ||
            (details.offset.dy <= rect.bottom &&
                details.offset.dx < rect.right)) {
          before = entry.id;
          break;
        }
      }
    }
    before ??= rootEntries
        .skip((_rootPage + 1) * _rootCapacity)
        .firstOrNull
        ?.id;
    _layout.moveBefore(details.data, before);
    _closeFolder();
  }

  void _confirmDeleteApp(String appId) {
    final app = _apps[appId];
    final appName = app?.name ?? '此应用';
    showDialog<bool>(
      context: context,
      builder: (context) {
        final dark = Theme.of(context).brightness == Brightness.dark;
        return AlertDialog(
          backgroundColor: dark
              ? const Color(0xff2d2d2f)
              : const Color(0xfff6f6f8),
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(10),
          ),
          title: Text(
            '是否从启动台移除“$appName”？',
            style: const TextStyle(fontSize: 14, fontWeight: FontWeight.w600),
          ),
          content: const Text(
            '从启动台移除不会卸载应用，可在应用列表或访达中重新打开。',
            style: TextStyle(fontSize: 12),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(context, false),
              child: const Text('取消'),
            ),
            TextButton(
              style: TextButton.styleFrom(foregroundColor: Colors.redAccent),
              onPressed: () => Navigator.pop(context, true),
              child: const Text('移除'),
            ),
          ],
        );
      },
    ).then((confirmed) {
      if (confirmed == true && mounted) {
        setState(() {
          _layout.remove(appId);
        });
      }
    });
  }

  Widget _searchField(double width, double height) => CallbackShortcuts(
    bindings: {
      const SingleActivator(LogicalKeyboardKey.arrowLeft, meta: true): () =>
          _goPage(_page - 1),
      const SingleActivator(LogicalKeyboardKey.arrowRight, meta: true): () =>
          _goPage(_page + 1),
    },
    child: SizedBox(
      width: math.min(250, width),
      height: math.min(26.0, height),
      child: TextField(
        key: const ValueKey('mac-launchpad-search'),
        controller: _query,
        focusNode: _search,
        onChanged: _searchChanged,
        style: const TextStyle(
          color: Colors.white,
          fontSize: 12.5,
          fontWeight: FontWeight.w400,
        ),
        cursorColor: Colors.white,
        decoration: InputDecoration(
          hintText: '搜索',
          hintStyle: TextStyle(
            color: Colors.white.withValues(alpha: .65),
            fontSize: 12.5,
          ),
          prefixIcon: Padding(
            padding: const EdgeInsets.only(left: 7, right: 5),
            child: OptionalAssetIcon(
              asset: 'assets/macos/ectl_search_loupe@2x.png',
              fallback: Icons.search_rounded,
              fallbackColor: Colors.white,
              width: 12,
              height: 12,
              filterQuality: FilterQuality.high,
            ),
          ),
          prefixIconConstraints: const BoxConstraints(
            minWidth: 24,
            minHeight: 22,
          ),
          suffixIcon: _query.text.isEmpty
              ? null
              : GestureDetector(
                  key: const ValueKey('mac-launchpad-clear-search'),
                  behavior: HitTestBehavior.opaque,
                  onTap: () {
                    _query.clear();
                    _searchChanged('');
                    _search.requestFocus();
                  },
                  child: Padding(
                    padding: const EdgeInsets.symmetric(horizontal: 5),
                    child: OptionalAssetIcon(
                      asset: 'assets/macos/ectl_search_close@2x.png',
                      fallback: Icons.cancel_rounded,
                      fallbackColor: Colors.white,
                      width: 13,
                      height: 13,
                      filterQuality: FilterQuality.high,
                    ),
                  ),
                ),
          suffixIconConstraints: const BoxConstraints(
            minWidth: 24,
            minHeight: 22,
          ),
          filled: true,
          fillColor: Colors.black.withValues(alpha: .28),
          contentPadding: const EdgeInsets.symmetric(
            horizontal: 6,
            vertical: 0,
          ),
          isDense: true,
          border: OutlineInputBorder(
            borderRadius: BorderRadius.circular(13),
            borderSide: BorderSide(
              color: Colors.white.withValues(alpha: .22),
              width: 0.5,
            ),
          ),
          enabledBorder: OutlineInputBorder(
            borderRadius: BorderRadius.circular(13),
            borderSide: BorderSide(
              color: Colors.white.withValues(alpha: .22),
              width: 0.5,
            ),
          ),
          focusedBorder: OutlineInputBorder(
            borderRadius: BorderRadius.circular(13),
            borderSide: BorderSide(
              color: Colors.white.withValues(alpha: .50),
              width: 0.8,
            ),
          ),
        ),
      ),
    ),
  );

  Widget _folderTitle(MacLaunchpadEntry folder, double width, double height) =>
      SizedBox(
        key: const ValueKey('mac-launchpad-folder-title'),
        width: math.min(380, width),
        height: height,
        child: _editingFolder == folder.id
            ? TextField(
                key: const ValueKey('mac-launchpad-folder-name'),
                controller: _nameEdit,
                focusNode: _nameFocus,
                textAlign: TextAlign.center,
                style: const TextStyle(
                  color: Colors.white,
                  fontSize: 30,
                  fontWeight: FontWeight.w400,
                  letterSpacing: -0.5,
                ),
                cursorColor: Colors.white,
                onSubmitted: (_) => _commitRename(),
                decoration: const InputDecoration(
                  border: InputBorder.none,
                  isDense: true,
                  contentPadding: EdgeInsets.zero,
                ),
              )
            : GestureDetector(
                onTap: () => _beginRename(folder),
                child: Text(
                  folder.name,
                  textAlign: TextAlign.center,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(
                    color: Colors.white,
                    fontSize: 30,
                    fontWeight: FontWeight.w400,
                    letterSpacing: -0.5,
                    shadows: [
                      Shadow(
                        color: Colors.black54,
                        blurRadius: 6,
                        offset: Offset(0, 2),
                      ),
                    ],
                  ),
                ),
              ),
      );

  Widget _pagination(MacLaunchpadEntry? folder) => SizedBox(
    height: 38,
    child: Center(
      child: SingleChildScrollView(
        controller: _dots,
        scrollDirection: Axis.horizontal,
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            if (folder != null)
              IconButton(
                key: const ValueKey('mac-launchpad-folder-back'),
                tooltip: '返回启动台',
                onPressed: _closeFolder,
                icon: const Icon(
                  Icons.chevron_left,
                  color: Colors.white70,
                  size: 20,
                ),
              ),
            for (var page = 0; page < _pageCount; page++)
              Semantics(
                label: '第 ${page + 1} 页，共 $_pageCount 页',
                selected: page == _page,
                button: true,
                child: GestureDetector(
                  key: ValueKey('mac-launchpad-dot-$page'),
                  onTap: () => _goPage(page),
                  behavior: HitTestBehavior.opaque,
                  child: SizedBox(
                    width: 20,
                    height: 32,
                    child: Center(
                      child: Container(
                        width: 7.5,
                        height: 7.5,
                        decoration: BoxDecoration(
                          shape: BoxShape.circle,
                          color: page == _page
                              ? Colors.white
                              : Colors.white.withValues(alpha: .36),
                          boxShadow: [
                            if (page == _page)
                              BoxShadow(
                                color: Colors.white.withValues(alpha: .50),
                                blurRadius: 3,
                              )
                            else
                              const BoxShadow(
                                color: Colors.black26,
                                blurRadius: 1.5,
                                offset: Offset(0, 0.5),
                              ),
                          ],
                        ),
                      ),
                    ),
                  ),
                ),
              ),
          ],
        ),
      ),
    ),
  );

  Widget _grid(
    List<MacLaunchpadEntry> entries,
    double cellWidth,
    double cellHeight,
    double iconSize,
  ) => Listener(
    onPointerSignal: _wheel,
    child: DragTarget<String>(
      onWillAcceptWithDetails: (_) => _canDrop,
      onAcceptWithDetails: _dropInBlank,
      builder: (context, candidates, rejected) => entries.isEmpty
          ? const Center(
              child: Text('没有匹配的应用', style: TextStyle(color: Colors.white70)),
            )
          : PageView.builder(
              key: const ValueKey('mac-launchpad-pages'),
              controller: _pages,
              allowImplicitScrolling: true,
              itemCount: _pageCount,
              onPageChanged: (page) {
                if (!mounted) return;
                setState(() {
                  _page = page;
                  final first = page * _capacity;
                  final index = entries.indexWhere(
                    (entry) => entry.id == _selected,
                  );
                  if (index >= 0 &&
                      (index < first || index >= first + _capacity)) {
                    _selected = null;
                  }
                });
                _revealDot();
              },
              itemBuilder: (context, page) => Listener(
                onPointerSignal: _wheel,
                child: GridView.builder(
                  key: ValueKey('mac-launchpad-page-$page'),
                  padding: EdgeInsets.zero,
                  physics: const NeverScrollableScrollPhysics(),
                  gridDelegate: SliverGridDelegateWithFixedCrossAxisCount(
                    crossAxisCount: _columns,
                    childAspectRatio: cellWidth / cellHeight,
                  ),
                  itemCount: math.min(
                    _capacity,
                    entries.length - page * _capacity,
                  ),
                  itemBuilder: (context, index) {
                    final entry = entries[page * _capacity + index];
                    final tile = _tile(entry, iconSize);
                    // Reorder strips exist only while a drag is active. Their
                    // ColoredBox is hit-test opaque, so a permanently mounted
                    // strip would also swallow taps on the tile edge.
                    if (_draggingId == null || _searching) return tile;
                    return Stack(
                      children: [
                        Positioned.fill(child: tile),
                        Positioned(
                          left: 0,
                          top: 0,
                          bottom: 0,
                          width: 12,
                          child: DragTarget<String>(
                            onWillAcceptWithDetails: (details) =>
                                _canDrop && details.data != entry.id,
                            onAcceptWithDetails: (details) =>
                                _moveBefore(details.data, entry.id),
                            builder: (context, candidates, rejected) =>
                                ColoredBox(
                                  color: candidates.isEmpty
                                      ? Colors.transparent
                                      : Colors.white.withValues(alpha: .35),
                                ),
                          ),
                        ),
                      ],
                    );
                  },
                ),
              ),
            ),
    ),
  );

  Widget _folderBackdrop(
    BoxConstraints constraints,
    double padding,
    double topPadding,
    double searchHeight,
    double labelHeight,
  ) {
    final width = math.max(1.0, constraints.maxWidth - padding * 2);
    final height = math.max(
      labelHeight + 46,
      constraints.maxHeight - topPadding - searchHeight - 14 - 48,
    );
    _rootColumns = (width / 140).floor().clamp(1, 7);
    final rows = (height / 120).floor().clamp(1, 5);
    _rootCapacity = _rootColumns * rows;
    final rootEntries = _roots;
    final index = rootEntries.indexWhere((entry) => entry.id == _folder);
    if (index >= 0) _rootPage = index ~/ _rootCapacity;
    final cellWidth = width / _rootColumns;
    final cellHeight = height / rows;
    final iconSize = math.min(
      96.0,
      math.max(24.0, math.min(cellWidth - 30, cellHeight - labelHeight - 22)),
    );
    return Positioned(
      left: padding,
      right: padding,
      top: topPadding + searchHeight + 14,
      height: height,
      child: IgnorePointer(
        child: Opacity(
          opacity: .26,
          child: GridView(
            key: const ValueKey('mac-launchpad-folder-backdrop'),
            padding: EdgeInsets.zero,
            physics: const NeverScrollableScrollPhysics(),
            gridDelegate: SliverGridDelegateWithFixedCrossAxisCount(
              crossAxisCount: _rootColumns,
              childAspectRatio: cellWidth / cellHeight,
            ),
            children: [
              for (final entry
                  in rootEntries
                      .skip(_rootPage * _rootCapacity)
                      .take(_rootCapacity))
                _tile(entry, iconSize, interactive: false),
            ],
          ),
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) => Material(
    type: MaterialType.transparency,
    child: Padding(
      padding: ShellGeometry.desktopInsets(
        ShellStyle.macos,
        widget.controller.barPosition,
        macPreferences: widget.controller.macPreferences,
      ).copyWith(top: 0),
      child: LayoutBuilder(
        builder: (context, constraints) {
          final folder = _folder == null ? null : _layout.find(_folder!);
          final padding = constraints.maxWidth < 600 ? 16.0 : 56.0;
          final textScale = MediaQuery.textScalerOf(context);
          final topPadding = constraints.maxHeight < 400 ? 12.0 : 28.0;
          final searchHeight = math.max(30.0, textScale.scale(14) * 1.2 + 10);
          final titleHeight = math.max(34.0, textScale.scale(28) * 1.2);
          final labelHeight = textScale.scale(13) * 1.2 * 2;
          final panelWidth = folder == null
              ? constraints.maxWidth
              : math.min(1100.0, math.max(1.0, constraints.maxWidth - 32));
          final innerPadding = folder == null ? padding : 24.0;
          final width = math.max(1.0, panelWidth - innerPadding * 2);
          final headerHeight =
              topPadding +
              (folder == null ? searchHeight : titleHeight) +
              14 +
              38 +
              10 +
              (_layout.error == null ? 0 : textScale.scale(11) * 1.2);
          final availableHeight = folder == null
              ? constraints.maxHeight
              : math.min(500.0, math.max(1.0, constraints.maxHeight - 64));
          final stageHeight = math.max(
            availableHeight,
            headerHeight + labelHeight + 46,
          );
          final gridHeight = math.max(1.0, stageHeight - headerHeight);
          _columns = (width / 140).floor().clamp(1, 7);
          final rows = (gridHeight / 120).floor().clamp(
            1,
            folder == null ? 5 : 3,
          );
          final entries = _entries;
          _reconcile(entries, _columns * rows);
          final cellWidth = width / _columns;
          final cellHeight = gridHeight / rows;
          final iconSize = math.min(
            96.0,
            math.max(
              24.0,
              math.min(cellWidth - 30, cellHeight - labelHeight - 22),
            ),
          );
          final stage = SizedBox(
            key: folder == null ? null : _folderPanelKey,
            width: panelWidth,
            height: stageHeight,
            child: GestureDetector(
              behavior: HitTestBehavior.opaque,
              onTap: folder == null ? null : () {},
              child: DecoratedBox(
                key: folder == null
                    ? null
                    : const ValueKey('mac-launchpad-folder-panel'),
                decoration: BoxDecoration(
                  color: folder == null
                      ? Colors.transparent
                      : const Color(0xff1e1e24).withValues(alpha: .72),
                  borderRadius: BorderRadius.circular(30),
                  border: folder == null
                      ? null
                      : Border.all(
                          color: Colors.white.withValues(alpha: .18),
                          width: 0.8,
                        ),
                  boxShadow: folder == null
                      ? const []
                      : [
                          BoxShadow(
                            color: Colors.black.withValues(alpha: 0.48),
                            blurRadius: 48,
                            spreadRadius: 4,
                            offset: const Offset(0, 16),
                          ),
                          BoxShadow(
                            color: Colors.black.withValues(alpha: 0.25),
                            blurRadius: 16,
                            offset: const Offset(0, 4),
                          ),
                        ],
                ),
                child: Stack(
                  children: [
                    Padding(
                      padding: EdgeInsets.symmetric(horizontal: innerPadding),
                      child: Column(
                        children: [
                          SizedBox(height: topPadding),
                          if (folder == null)
                            _searchField(width, searchHeight)
                          else
                            _folderTitle(folder, width, titleHeight),
                          const SizedBox(height: 14),
                          Expanded(
                            child: _grid(
                              entries,
                              cellWidth,
                              cellHeight,
                              iconSize,
                            ),
                          ),
                          _pagination(folder),
                          if (_layout.error != null)
                            Text(
                              _layout.error!,
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: const TextStyle(
                                color: Colors.white70,
                                fontSize: 11,
                              ),
                            ),
                          const SizedBox(height: 10),
                        ],
                      ),
                    ),
                    if (!_searching && _pageCount > 1) ...[
                      Positioned(
                        left: 0,
                        top: topPadding + titleHeight + 14,
                        bottom: 48,
                        width: 24,
                        child: _dragEdge(-1),
                      ),
                      Positioned(
                        right: 0,
                        top: topPadding + titleHeight + 14,
                        bottom: 48,
                        width: 24,
                        child: _dragEdge(1),
                      ),
                    ],
                  ],
                ),
              ),
            ),
          );
          final scrollableStage = SingleChildScrollView(
            primary: false,
            physics: stageHeight > availableHeight
                ? const ClampingScrollPhysics()
                : const NeverScrollableScrollPhysics(),
            child: stage,
          );
          return Focus(
            focusNode: _keyboard,
            autofocus: true,
            onKeyEvent: _key,
            child: GestureDetector(
              key: const ValueKey('mac-launchpad-background'),
              behavior: HitTestBehavior.opaque,
              onTap: () {
                if (_jiggleMode) {
                  setState(() => _setJiggleMode(false));
                  return;
                }
                if (folder != null) {
                  _closeFolder();
                } else {
                  widget.controller.closePanel();
                }
              },
              onLongPress: () {
                if (!_jiggleMode) {
                  setState(() => _setJiggleMode(true));
                }
              },
              child: folder == null
                  ? scrollableStage
                  : DragTarget<String>(
                      key: const ValueKey('mac-launchpad-folder-outside-drop'),
                      onWillAcceptWithDetails: (details) =>
                          _canDrop &&
                          _dragOriginFolder == _folder &&
                          folder.appIds.contains(details.data),
                      onAcceptWithDetails: _extractFromFolder,
                      builder: (context, candidates, rejected) => Stack(
                        children: [
                          _folderBackdrop(
                            constraints,
                            padding,
                            topPadding,
                            searchHeight,
                            labelHeight,
                          ),
                          Center(child: scrollableStage),
                        ],
                      ),
                    ),
            ),
          );
        },
      ),
    ),
  );
}

/// Gentle macOS Catalina icon wobble animation for Launchpad edit mode.
class _LaunchpadJiggle extends StatelessWidget {
  const _LaunchpadJiggle({
    required this.child,
    required this.seed,
    required this.clock,
    required this.active,
    required this.reduced,
  });
  final Widget child;
  final int seed;
  final Animation<double> clock;
  final bool active;
  final bool reduced;

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: clock,
      builder: (context, child) {
        // Keep the icon subtree mounted when edit mode or reduced motion
        // changes. The panel stops the shared clock when wobble is inactive.
        var angle = 0.0;
        var offsetY = 0.0;
        if (active && !reduced) {
          // Seeded phases preserve independent wobble on one panel ticker.
          final phase = (clock.value + (seed.abs() % 100) / 200.0) % 1.0;
          final value = phase <= .5 ? phase * 2 : (1 - phase) * 2;
          final t = (value - 0.5) * 2.0;
          angle = t * (2.2 * math.pi / 180.0);
          offsetY = (1.0 - t.abs()) * 1.5 - 0.75;
        }
        return Transform.translate(
          offset: Offset(0, offsetY),
          child: Transform.rotate(angle: angle, child: child),
        );
      },
      child: child,
    );
  }
}

class _LaunchpadCloseBox extends StatefulWidget {
  const _LaunchpadCloseBox({required this.onTap});
  final VoidCallback onTap;

  @override
  State<_LaunchpadCloseBox> createState() => _LaunchpadCloseBoxState();
}

class _LaunchpadCloseBoxState extends State<_LaunchpadCloseBox> {
  bool _pressed = false;

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      behavior: HitTestBehavior.opaque,
      onTapDown: (_) => setState(() => _pressed = true),
      onTapUp: (_) => setState(() => _pressed = false),
      onTapCancel: () => setState(() => _pressed = false),
      onTap: widget.onTap,
      child: OptionalAssetIcon(
        fallback: Icons.cancel_rounded,
        fallbackColor: Colors.white,
        asset: _pressed
            ? 'assets/macos/ecsb_closebox_pressed@2x.png'
            : 'assets/macos/ecsb_closebox@2x.png',
        key: const ValueKey('mac-launchpad-delete-badge'),
        width: 20,
        height: 20,
        filterQuality: FilterQuality.high,
      ),
    );
  }
}
