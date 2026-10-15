// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:io';
import 'dart:math' as math;

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../mac_shell_preferences.dart';
import '../models.dart';
import '../ui/app_icon.dart';
import '../ui/optional_asset_icon.dart';
import '../ui/shell_geometry.dart';
import 'desktop_entry.dart';
import 'desktop_inspector.dart';
import 'desktop_context_menu.dart';
import 'desktop_layout.dart';
import 'desktop_service.dart';
import 'desktop_stacks.dart';

class DesktopView extends StatefulWidget {
  const DesktopView({
    super.key,
    required this.service,
    required this.layout,
    required this.outputName,
    required this.barPosition,
    this.shellStyle = ShellStyle.material,
    this.macPreferences = const MacShellPreferences(),
    this.apps = const [],
    this.reduceMotion = false,
    this.onInteractionChanged,
    this.prepareInteraction,
    this.interactionNavigator,
    this.inlineRenameFromMenu = true,
  });

  final DesktopService service;
  final DesktopLayoutStore layout;
  final String outputName;
  final BarPosition barPosition;
  final ShellStyle shellStyle;
  final MacShellPreferences macPreferences;
  final List<DesktopApp> apps;
  final bool reduceMotion;
  final ValueChanged<bool>? onInteractionChanged;
  final Future<void> Function()? prepareInteraction;
  final GlobalKey<NavigatorState>? interactionNavigator;

  /// Native popup layers need compositor support to restore the base seat focus.
  final bool inlineRenameFromMenu;

  @override
  State<DesktopView> createState() => _DesktopViewState();
}

enum _DesktopAction {
  open,
  info,
  quickLook,
  rename,
  trash,
  copy,
  paste,
  duplicate,
  useStacks,
  stackGroupBy,
  stackGroupKind,
  stackGroupModified,
  stackGroupSize,
  createFile,
  createFolder,
  createShortcut,
  cleanUp,
  cleanUpSelection,
  cleanUpBy,
  sortBy,
  sortNone,
  sortName,
  sortKind,
  sortModified,
  sortSize,
  cleanUpName,
  cleanUpKind,
  cleanUpModified,
  cleanUpSize,
}

class _DesktopViewState extends State<DesktopView> {
  static const _tooltipNameStyle = TextStyle(fontSize: 14, height: 1.3);
  final _focus = FocusNode(debugLabel: 'desktop-files');
  final _renameFocus = FocusNode(debugLabel: 'desktop-inline-rename');
  final _renameText = TextEditingController();
  DesktopEntry? _renaming;
  DesktopCell? _renameCell;
  bool _renamePending = false;
  bool _renameFailed = false;
  int _renameGeneration = 0;
  final _surface = GlobalKey();
  String? _selected;
  String? _expandedStackId;
  final _selection = <String>{};
  String? _selectionAnchor;
  bool _collapseSelectionOnTap = false;
  String? _dragging;
  Set<String> _dragSelection = const {};
  Offset? _dragPoint;
  Offset? _marqueeStart;
  Rect? _marquee;
  Set<String> _beforeMarquee = const {};
  String? _beforeMarqueeActive;
  String? _beforeMarqueeAnchor;
  bool _marqueeAdditive = false;
  int _page = 0;
  int _interactionDepth = 0;
  DesktopGridSpec? _grid;
  Map<String, DesktopCell> _positions = const {};
  DateTime? _lastWheel;

  EdgeInsets get _surfaceInsets => ShellGeometry.desktopInsets(
    widget.shellStyle,
    widget.barPosition,
    macPreferences: widget.macPreferences,
  );

  @override
  void initState() {
    super.initState();
    widget.service.addListener(_changed);
    widget.layout.addListener(_changed);
    unawaited(widget.layout.load());
  }

  @override
  void didUpdateWidget(covariant DesktopView oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!identical(oldWidget.service, widget.service)) {
      _cancelInlineRename(restoreFocus: false);
      oldWidget.service.removeListener(_changed);
      widget.service.addListener(_changed);
      _selectOnly(null);
      _dragging = null;
    }
    if (!identical(oldWidget.layout, widget.layout)) {
      oldWidget.layout.removeListener(_changed);
      widget.layout.addListener(_changed);
      unawaited(widget.layout.load());
    }
    if (oldWidget.outputName != widget.outputName) {
      _cancelInlineRename(restoreFocus: false);
      _page = 0;
      _dragging = null;
      _positions = const {};
      if (widget.shellStyle == ShellStyle.macos) _selectOnly(null);
      _marquee = null;
    }
    if (oldWidget.shellStyle != widget.shellStyle) {
      _cancelInlineRename(restoreFocus: false);
      _selectOnly(_selected);
      _dragging = null;
      _marquee = null;
    }
  }

  void _changed() {
    if (!mounted) return;
    final paths = widget.service.entries.map((entry) => entry.fullPath).toSet();
    final renaming = _renaming;
    if (renaming != null && !paths.contains(renaming.fullPath)) {
      if (_renamePending) {
        // The successful filesystem rescan arrives before renameEntry returns.
        // Keep the old selection until its replacement path can be selected.
        paths.add(renaming.fullPath);
      } else {
        _cancelInlineRename(restoreFocus: false);
      }
    }
    _selection.retainAll(paths);
    if (!paths.contains(_selected)) _selected = _selection.firstOrNull;
    if (!paths.contains(_selectionAnchor)) _selectionAnchor = _selected;
    if (!paths.contains(_dragging) || !_dragSelection.every(paths.contains)) {
      _dragging = null;
      _dragSelection = const {};
    }
    setState(() {});
  }

  @override
  void dispose() {
    widget.service.removeListener(_changed);
    widget.layout.removeListener(_changed);
    if (_interactionDepth > 0) widget.onInteractionChanged?.call(false);
    _focus.dispose();
    _renameFocus.dispose();
    _renameText.dispose();
    super.dispose();
  }

  BuildContext get _interactionContext {
    final navigator = widget.interactionNavigator;
    if (navigator == null) return context;
    final overlay = navigator.currentState?.overlay;
    if (overlay == null) {
      throw PlatformException(code: 'interaction-unavailable');
    }
    return overlay.context;
  }

  Future<T?> _interact<T>(Future<T?> Function() action) async {
    final first = _interactionDepth++ == 0;
    if (first) {
      Tooltip.dismissAllToolTips();
      setState(() {});
      widget.onInteractionChanged?.call(true);
    }
    try {
      if (first) await widget.prepareInteraction?.call();
      if (!mounted) return null;
      return await action();
    } on PlatformException catch (error) {
      stderr.writeln('Realand desktop interaction: ${error.message}');
      return null;
    } finally {
      _interactionDepth--;
      if (mounted && _interactionDepth == 0) {
        setState(() {});
        widget.onInteractionChanged?.call(false);
      }
    }
  }

  DesktopEntry? get _selectedEntry => widget.service.entries
      .where((entry) => entry.fullPath == _selected)
      .firstOrNull;

  List<DesktopEntry> get _selectedEntries => widget.service.entries
      .where(
        (entry) =>
            (widget.shellStyle == ShellStyle.macos
                ? _selection.contains(entry.fullPath)
                : _selected == entry.fullPath) &&
            _positions[entry.fullPath]?.page == _page,
      )
      .toList();

  List<String> get _visiblePaths {
    final entries =
        _positions.entries.where((entry) => entry.value.page == _page).toList()
          ..sort((a, b) {
            final column = a.value.column.compareTo(b.value.column);
            return column == 0 ? a.value.row.compareTo(b.value.row) : column;
          });
    return entries.map((entry) => entry.key).toList();
  }

  bool get _controlClick {
    final keyboard = HardwareKeyboard.instance;
    return keyboard.isControlPressed &&
        !keyboard.isMetaPressed &&
        !keyboard.isAltPressed &&
        !keyboard.isShiftPressed;
  }

  void _selectOnly(String? path) {
    _selection
      ..clear()
      ..addAll(path == null ? const [] : [path]);
    _selected = path;
    _selectionAnchor = path;
  }

  void _selectRange(String path) {
    final ordered = _visiblePaths;
    final start = ordered.indexOf(_selectionAnchor ?? path);
    final end = ordered.indexOf(path);
    if (start < 0 || end < 0) {
      _selectOnly(path);
      return;
    }
    _selection
      ..clear()
      ..addAll(ordered.sublist(math.min(start, end), math.max(start, end) + 1));
    _selected = path;
    _selectionAnchor ??= path;
  }

  void _pointerSelect(DesktopEntry entry) {
    if (_interactionDepth > 0 || (widget.service.busy && !_renamePending)) {
      return;
    }
    _focus.requestFocus();
    final path = entry.fullPath;
    final keyboard = HardwareKeyboard.instance;
    setState(() {
      _collapseSelectionOnTap = false;
      if (widget.shellStyle != ShellStyle.macos) {
        _selectOnly(path);
      } else if (keyboard.isMetaPressed &&
          !keyboard.isControlPressed &&
          !keyboard.isAltPressed) {
        if (!_selection.add(path)) _selection.remove(path);
        _selected = _selection.contains(path) ? path : _selection.lastOrNull;
        _selectionAnchor = _selected;
      } else if (keyboard.isShiftPressed &&
          !keyboard.isControlPressed &&
          !keyboard.isAltPressed) {
        _selectRange(path);
      } else if (_selection.contains(path)) {
        // A press on a selected icon may become a group drag. Collapse only
        // once the tap wins, never before the pan recognizer can take over.
        _selected = path;
        _collapseSelectionOnTap = true;
      } else {
        _selectOnly(path);
      }
    });
  }

  Future<void> _openSelection(List<DesktopEntry> entries) async {
    if (entries.isEmpty || widget.service.busy) return;
    if (entries.length == 1) return _open(entries.single);
    final untrusted = entries
        .where((entry) => entry.isDesktopShortcut && !entry.trustedShortcut)
        .toList();
    if (untrusted.isNotEmpty) {
      final confirmed = await _interact(
        () => showDialog<bool>(
          context: _interactionContext,
          builder: (context) => AlertDialog(
            title: const Text('运行桌面快捷方式？'),
            scrollable: true,
            content: Text(
              '选中项目中包含 ${untrusted.length} 个可运行应用的快捷方式：\n${untrusted.map((entry) => entry.displayName).join('\n')}\n确认其来源可信后继续打开选中项目。',
            ),
            actions: [
              TextButton(
                onPressed: () => Navigator.pop(context, false),
                child: const Text('取消'),
              ),
              FilledButton(
                key: const ValueKey('desktop-trust-confirm'),
                onPressed: () => Navigator.pop(context, true),
                child: const Text('运行'),
              ),
            ],
          ),
        ),
      );
      if (confirmed != true || !mounted) return;
    }
    await widget.service.openEntries(
      entries,
      allowedUntrustedShortcuts: untrusted
          .map((entry) => entry.fullPath)
          .toSet(),
    );
  }

  Future<void> _open(DesktopEntry entry) async {
    if (widget.service.busy) return;
    var trusted = entry.trustedShortcut;
    if (entry.isDesktopShortcut && !trusted) {
      trusted =
          await _interact(
            () => showDialog<bool>(
              context: _interactionContext,
              builder: (context) => AlertDialog(
                title: const Text('运行桌面快捷方式？'),
                content: Text('“${entry.displayName}”可以运行应用。确认其来源可信后继续。'),
                actions: [
                  TextButton(
                    onPressed: () => Navigator.pop(context, false),
                    child: const Text('取消'),
                  ),
                  FilledButton(
                    key: const ValueKey('desktop-trust-confirm'),
                    onPressed: () => Navigator.pop(context, true),
                    child: const Text('运行'),
                  ),
                ],
              ),
            ),
          ) ??
          false;
      if (!trusted || !mounted) return;
    }
    await widget.service.openEntry(entry, allowUntrustedShortcut: trusted);
  }

  Future<void> _inspectSelection(
    List<DesktopEntry> entries, {
    required bool preview,
  }) async {
    if (entries.isEmpty || widget.service.busy) return;
    final current = _selectedEntry;
    await _interact(() async {
      final open = await (preview
          ? showDesktopQuickLookDialog(
              context: _interactionContext,
              service: widget.service,
              entries: entries,
              current: current,
            )
          : showDesktopInfoDialog(
              context: _interactionContext,
              service: widget.service,
              entries: entries,
              current: current,
            ));
      // Keep the native interaction surface captured through a possible trust
      // dialog. The explicit Open still uses the desktop's normal trust chain.
      if (open != null && mounted) await _open(open);
      return null;
    });
  }

  Future<void> _setSorting(DesktopSortKey? key) async {
    final output = widget.outputName;
    final layout = widget.layout;
    await layout.load();
    if (!mounted || widget.outputName != output) return;
    final work = layout.setSortKey(output, key, positions: _positions);
    final grid = _grid;
    if (key != null && grid != null) {
      final sorted = DesktopLayout.resolveEntries(
        widget.service.entries,
        grid,
        _positions,
        sortKey: key,
      );
      final selected = sorted[_selected];
      if (selected != null) setState(() => _page = selected.page);
    }
    await work;
  }

  Future<void> _cleanUp({
    DesktopSortKey? key,
    bool selectionOnly = false,
  }) async {
    final grid = _grid;
    if (grid == null || widget.layout.sortKey(widget.outputName) != null) {
      return;
    }
    final selected = selectionOnly
        ? _selectedEntries.map((entry) => entry.fullPath).toSet()
        : null;
    if (selectionOnly && (selected == null || selected.length < 2)) return;
    final positions = DesktopLayout.arrange(
      widget.service.entries,
      grid,
      _positions,
      sortKey: key,
      selection: selected,
    );
    await widget.layout.savePositions(widget.outputName, positions);
  }

  List<DesktopContextMenuItem<_DesktopAction>> _orderItems({
    required bool continuous,
  }) {
    final current = widget.layout.sortKey(widget.outputName);
    return [
      if (continuous) ...[
        DesktopContextMenuItem(
          key: const ValueKey('desktop-sort-none'),
          value: _DesktopAction.sortNone,
          label: '无',
          checked: current == null,
        ),
        const DesktopContextMenuItem.divider(),
      ],
      for (final choice in [
        (
          '名称',
          DesktopSortKey.name,
          _DesktopAction.sortName,
          _DesktopAction.cleanUpName,
        ),
        (
          '种类',
          DesktopSortKey.kind,
          _DesktopAction.sortKind,
          _DesktopAction.cleanUpKind,
        ),
        (
          '修改日期',
          DesktopSortKey.modified,
          _DesktopAction.sortModified,
          _DesktopAction.cleanUpModified,
        ),
        (
          '大小',
          DesktopSortKey.size,
          _DesktopAction.sortSize,
          _DesktopAction.cleanUpSize,
        ),
      ])
        DesktopContextMenuItem(
          key: ValueKey(
            'desktop-${continuous ? 'sort' : 'clean'}-${choice.$2.name}',
          ),
          value: continuous ? choice.$3 : choice.$4,
          label: choice.$1,
          checked: continuous && current == choice.$2,
        ),
    ];
  }

  List<DesktopContextMenuItem<_DesktopAction>> _stackOrderItems() {
    final current = widget.layout.stackGroupBy(widget.outputName);
    return [
      DesktopContextMenuItem(
        key: const ValueKey('desktop-stack-kind'),
        value: _DesktopAction.stackGroupKind,
        label: '种类',
        checked: current == DesktopSortKey.kind,
      ),
      DesktopContextMenuItem(
        key: const ValueKey('desktop-stack-modified'),
        value: _DesktopAction.stackGroupModified,
        label: '修改日期',
        checked: current == DesktopSortKey.modified,
      ),
      DesktopContextMenuItem(
        key: const ValueKey('desktop-stack-size'),
        value: _DesktopAction.stackGroupSize,
        label: '大小',
        checked: current == DesktopSortKey.size,
      ),
    ];
  }

  Future<String?> _nameDialog(String title, {String initial = ''}) => _interact(
    () => showDialog<String>(
      context: _interactionContext,
      builder: (context) => _NameDialog(title: title, initial: initial),
    ),
  );

  Future<void> _rename(DesktopEntry entry, {bool fromMenu = false}) async {
    if (widget.shellStyle == ShellStyle.macos &&
        (!fromMenu || widget.inlineRenameFromMenu)) {
      _startInlineRename(entry);
      return;
    }
    final oldCell = _positions[entry.fullPath];
    final name = await _nameDialog('重命名', initial: entry.name);
    if (name == null || !mounted) return;
    await widget.service.renameEntry(entry, name);
    if (!mounted || widget.service.error != null) return;
    final replacement = widget.service.entries
        .where((entry) => entry.name == name)
        .firstOrNull;
    if (replacement != null) {
      setState(() => _selectOnly(replacement.fullPath));
      _focus.requestFocus();
      if (oldCell != null) {
        final cells = Map<String, DesktopCell>.of(_positions)
          ..remove(entry.fullPath)
          ..[replacement.fullPath] = oldCell;
        await widget.layout.savePositions(widget.outputName, cells);
      }
    }
  }

  void _startInlineRename(DesktopEntry entry) {
    if (widget.service.busy || _renamePending) return;
    Tooltip.dismissAllToolTips();
    final extension = entry.isDirectory ? -1 : entry.name.lastIndexOf('.');
    setState(() {
      _renameGeneration++;
      _renaming = entry;
      _renameCell = _positions[entry.fullPath];
      _renameFailed = false;
      _selectOnly(entry.fullPath);
      _renameText.value = TextEditingValue(
        text: entry.name,
        selection: TextSelection(
          baseOffset: 0,
          extentOffset: extension > 0 ? extension : entry.name.length,
        ),
      );
    });
    final generation = _renameGeneration;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted && _renaming != null && generation == _renameGeneration) {
        _renameFocus.requestFocus();
      }
    });
  }

  void _cancelInlineRename({bool restoreFocus = true}) {
    if (_renaming == null) return;
    _renameGeneration++;
    _renaming = null;
    _renamePending = false;
    _renameFailed = false;
    _renameCell = null;
    if (mounted) setState(() {});
    if (restoreFocus) _focus.requestFocus();
  }

  Future<void> _commitInlineRename() async {
    final entry = _renaming;
    if (entry == null || _renamePending || widget.service.busy) return;
    final name = _renameText.text;
    if (name == entry.name) {
      _cancelInlineRename();
      return;
    }
    final generation = _renameGeneration;
    final output = widget.outputName;
    final service = widget.service;
    final layout = widget.layout;
    final cell = _renameCell;
    setState(() {
      _renamePending = true;
      _renameFailed = false;
    });
    await service.renameEntry(entry, name);
    if (!mounted ||
        generation != _renameGeneration ||
        !identical(service, widget.service) ||
        output != widget.outputName) {
      return;
    }
    if (service.error != null) {
      setState(() {
        _renamePending = false;
        _renameFailed = true;
        _selectOnly(entry.fullPath);
      });
      _renameFocus.requestFocus();
      return;
    }
    final replacement = service.entries
        .where((value) => value.name == name)
        .firstOrNull;
    if (replacement == null) {
      _cancelInlineRename();
      return;
    }
    final keepSelected = _selected == entry.fullPath;
    _cancelInlineRename(restoreFocus: keepSelected);
    if (keepSelected) setState(() => _selectOnly(replacement.fullPath));
    if (cell != null && identical(layout, widget.layout)) {
      final cells =
          <String, DesktopCell>{..._positions, ...layout.positions(output)}
            ..remove(entry.fullPath)
            ..[replacement.fullPath] = cell;
      await layout.savePositions(output, cells);
    }
  }

  Future<void> _trash(DesktopEntry entry) => _trashSelection([entry]);

  Future<void> _trashSelection(List<DesktopEntry> entries) async {
    final macos = widget.shellStyle == ShellStyle.macos;
    final confirmed = await _interact(
      () => showDialog<bool>(
        context: _interactionContext,
        builder: (context) => AlertDialog(
          title: Text(macos ? '移到废纸篓？' : '移到回收站？'),
          content: Text(
            entries.length == 1
                ? (macos
                    ? '将“${entries.single.displayName}”移到废纸篓，可在废纸篓中恢复。'
                    : '将“${entries.single.displayName}”移到回收站，可在文件管理器中恢复。')
                : (macos
                    ? '将选中的 ${entries.length} 个项目移到废纸篓，可在废纸篓中恢复。'
                    : '将选中的 ${entries.length} 个项目移到回收站，可在文件管理器中恢复。'),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(context, false),
              child: const Text('取消'),
            ),
            FilledButton(
              key: const ValueKey('desktop-trash-confirm'),
              onPressed: () => Navigator.pop(context, true),
              child: Text(macos ? '移到废纸篓' : '移到回收站'),
            ),
          ],
        ),
      ),
    );
    if (confirmed == true && mounted) {
      if (entries.length == 1) {
        await widget.service.trashEntry(entries.single);
      } else {
        await widget.service.trashEntries(entries);
      }
    }
  }

  Future<void> _create(_DesktopAction action) async {
    if (action == _DesktopAction.createShortcut) {
      final shortcut = await _interact(
        () => showDialog<_ShortcutChoice>(
          context: _interactionContext,
          builder: (context) => _ShortcutDialog(apps: widget.apps),
        ),
      );
      if (shortcut == null || !mounted) return;
      if (shortcut.app != null) {
        final name = shortcut.name;
        await widget.service.createShortcutInstalledApp(
          shortcut.app!,
          name: name == null || name.endsWith('.desktop')
              ? name
              : '$name.desktop',
        );
      } else {
        await widget.service.createShortcutPath(
          shortcut.path!,
          name: shortcut.name,
        );
      }
      return;
    }
    final folder = action == _DesktopAction.createFolder;
    final name = await _nameDialog(folder ? '新建文件夹' : '新建文件');
    if (name == null || !mounted) return;
    if (folder) {
      await widget.service.createDirectory(name);
    } else {
      await widget.service.createFile(name);
    }
  }

  Future<void> _menu(Offset point, [DesktopEntry? entry]) async {
    if (widget.service.busy || _interactionDepth > 0) return;
    _focus.requestFocus();
    if (entry != null &&
        (widget.shellStyle != ShellStyle.macos ||
            !_selection.contains(entry.fullPath))) {
      setState(() => _selectOnly(entry.fullPath));
    }
    final selected = _selectedEntries;
    await _interact(() async {
      final menuContext = _interactionContext;
      final overlay =
          Navigator.of(menuContext).overlay!.context.findRenderObject()!
              as RenderBox;
      final local = overlay.globalToLocal(point);
      final macos = widget.shellStyle == ShellStyle.macos;
      DesktopContextMenuItem<_DesktopAction> item(
        _DesktopAction action,
        String label, {
        String? shortcut,
        bool enabled = true,
        bool checked = false,
        List<DesktopContextMenuItem<_DesktopAction>> children = const [],
      }) => DesktopContextMenuItem(
        key: ValueKey('desktop-menu-${action.name}'),
        value: action,
        label: label,
        shortcut: shortcut,
        enabled: enabled,
        checked: checked,
        children: children,
      );
      final stacksEnabled = widget.layout.stacksEnabled(widget.outputName);
      final items = entry == null
          ? [
              if (macos) item(_DesktopAction.createFolder, '新建文件夹'),
              item(_DesktopAction.createFile, '新建文件'),
              if (!macos) item(_DesktopAction.createFolder, '新建文件夹'),
              item(_DesktopAction.createShortcut, '新建快捷方式'),
              if (macos) ...[
                if (widget.service.canPaste) ...[
                  const DesktopContextMenuItem<_DesktopAction>.divider(),
                  item(
                    _DesktopAction.paste,
                    '粘贴项目',
                    shortcut: '⌘V',
                  ),
                ],
                const DesktopContextMenuItem<_DesktopAction>.divider(),
                item(
                  _DesktopAction.useStacks,
                  '使用叠放',
                  checked: stacksEnabled,
                ),
                if (stacksEnabled)
                  item(
                    _DesktopAction.stackGroupBy,
                    '叠放方式',
                    children: _stackOrderItems(),
                  )
                else if (widget.service.entries.isNotEmpty) ...[
                  if (widget.layout.sortKey(widget.outputName) == null) ...[
                    item(_DesktopAction.cleanUp, '整理'),
                    item(
                      _DesktopAction.cleanUpBy,
                      '整理方式',
                      children: _orderItems(continuous: false),
                    ),
                  ],
                  item(
                    _DesktopAction.sortBy,
                    '排序方式',
                    children: _orderItems(continuous: true),
                  ),
                ],
              ],
            ]
          : [
              item(_DesktopAction.open, '打开'),
              if (macos) ...[
                item(_DesktopAction.trash, '移到废纸篓', shortcut: '⌘⌫'),
                const DesktopContextMenuItem<_DesktopAction>.divider(),
                item(_DesktopAction.copy, '拷贝', shortcut: '⌘C'),
                item(_DesktopAction.duplicate, '复制', shortcut: '⌘D'),
              ],
              const DesktopContextMenuItem<_DesktopAction>.divider(),
              item(_DesktopAction.info, '显示简介', shortcut: macos ? '⌘I' : null),
              if (selected.length == 1) item(_DesktopAction.rename, '重命名'),
              item(
                _DesktopAction.quickLook,
                '快速查看',
                shortcut: macos ? '空格' : null,
              ),
              if (!macos) ...[
                const DesktopContextMenuItem<_DesktopAction>.divider(),
                item(_DesktopAction.trash, '移到回收站'),
              ],
              if (macos &&
                  selected.length > 1 &&
                  widget.layout.sortKey(widget.outputName) == null) ...[
                const DesktopContextMenuItem<_DesktopAction>.divider(),
                item(_DesktopAction.cleanUpSelection, '整理所选内容'),
              ],
            ];
      final action = macos
          ? await showDesktopContextMenu<_DesktopAction>(
              context: menuContext,
              position: local,
              items: items,
              reduceMotion: widget.reduceMotion,
            )
          : await showMenu<_DesktopAction>(
              context: menuContext,
              position: RelativeRect.fromRect(
                Rect.fromLTWH(local.dx, local.dy, 1, 1),
                Offset.zero & overlay.size,
              ),
              requestFocus: true,
              popUpAnimationStyle: widget.reduceMotion
                  ? AnimationStyle.noAnimation
                  : null,
              items: [
                for (final item in items)
                  if (item.isDivider)
                    const PopupMenuDivider(height: 9)
                  else
                    PopupMenuItem(value: item.value, child: Text(item.label)),
              ],
            );
      if (action == null || !mounted) return;
      switch (action) {
        case _DesktopAction.open:
          if (entry != null) await _openSelection(selected);
        case _DesktopAction.info:
          if (entry != null) await _inspectSelection(selected, preview: false);
        case _DesktopAction.quickLook:
          if (entry != null) await _inspectSelection(selected, preview: true);
        case _DesktopAction.rename:
          if (entry != null) await _rename(entry, fromMenu: true);
        case _DesktopAction.trash:
          if (entry != null) await _trashSelection(selected);
        case _DesktopAction.copy:
          if (selected.isNotEmpty) widget.service.copySelection(selected);
        case _DesktopAction.paste:
          await widget.service.paste();
        case _DesktopAction.duplicate:
          if (selected.isNotEmpty) {
            await widget.service.duplicateSelection(selected);
          }
        case _DesktopAction.useStacks:
          final next = !widget.layout.stacksEnabled(widget.outputName);
          await widget.layout.setStacksEnabled(widget.outputName, next);
        case _DesktopAction.stackGroupBy:
          break;
        case _DesktopAction.stackGroupKind:
          await widget.layout.setStackGroupBy(
            widget.outputName,
            DesktopSortKey.kind,
          );
        case _DesktopAction.stackGroupModified:
          await widget.layout.setStackGroupBy(
            widget.outputName,
            DesktopSortKey.modified,
          );
        case _DesktopAction.stackGroupSize:
          await widget.layout.setStackGroupBy(
            widget.outputName,
            DesktopSortKey.size,
          );
        case _DesktopAction.createFile:
        case _DesktopAction.createFolder:
        case _DesktopAction.createShortcut:
          await _create(action);
        case _DesktopAction.cleanUp:
          await _cleanUp();
        case _DesktopAction.cleanUpSelection:
          await _cleanUp(key: DesktopSortKey.name, selectionOnly: true);
        case _DesktopAction.sortNone:
          await _setSorting(null);
        case _DesktopAction.sortName:
          await _setSorting(DesktopSortKey.name);
        case _DesktopAction.sortKind:
          await _setSorting(DesktopSortKey.kind);
        case _DesktopAction.sortModified:
          await _setSorting(DesktopSortKey.modified);
        case _DesktopAction.sortSize:
          await _setSorting(DesktopSortKey.size);
        case _DesktopAction.cleanUpName:
          await _cleanUp(key: DesktopSortKey.name);
        case _DesktopAction.cleanUpKind:
          await _cleanUp(key: DesktopSortKey.kind);
        case _DesktopAction.cleanUpModified:
          await _cleanUp(key: DesktopSortKey.modified);
        case _DesktopAction.cleanUpSize:
          await _cleanUp(key: DesktopSortKey.size);
        case _DesktopAction.cleanUpBy:
        case _DesktopAction.sortBy:
          break;
      }
    });
  }

  Offset _local(Offset point) =>
      (_surface.currentContext!.findRenderObject()! as RenderBox).globalToLocal(
        point,
      );

  void _cancelDrag() => setState(() {
    _dragging = null;
    _dragSelection = const {};
    _dragPoint = null;
  });

  Future<void> _drop() async {
    final path = _dragging;
    final grid = _grid;
    final point = _dragPoint;
    if (path == null || grid == null || point == null) return;
    final target = grid.cellAt(point, page: _page);
    final origin = _positions[path];
    final group = _dragSelection;
    _cancelDrag();
    if (target == null || origin == null || target == origin) return;
    if (widget.shellStyle == ShellStyle.macos &&
        widget.layout.sortKey(widget.outputName) != null) {
      return;
    }
    if (widget.shellStyle == ShellStyle.macos && group.length > 1) {
      final moved = DesktopLayout.moveGroup(
        _positions,
        group,
        path,
        target,
        grid,
      );
      if (moved != null) {
        await widget.layout.savePositions(widget.outputName, moved);
      }
      return;
    }
    final moved = Map<String, DesktopCell>.of(_positions);
    final other = moved.entries
        .where((entry) => entry.value == target)
        .firstOrNull;
    moved[path] = target;
    if (other != null && other.key != path) moved[other.key] = origin;
    await widget.layout.savePositions(widget.outputName, moved);
  }

  void _moveSelection(LogicalKeyboardKey key, {bool extend = false}) {
    final grid = _grid;
    if (grid == null || _positions.isEmpty) return;
    final current = _positions[_selected];
    final candidates = _positions.entries.toList();
    if (current != null) {
      final horizontal =
          key == LogicalKeyboardKey.arrowLeft ||
          key == LogicalKeyboardKey.arrowRight;
      int x(DesktopCell cell) => cell.page * grid.columns + cell.column;
      candidates.removeWhere((entry) {
        final cell = entry.value;
        if (horizontal) {
          return key == LogicalKeyboardKey.arrowRight
              ? x(cell) <= x(current)
              : x(cell) >= x(current);
        }
        return cell.page != current.page ||
            (key == LogicalKeyboardKey.arrowDown
                ? cell.row <= current.row
                : cell.row >= current.row);
      });
      int distance(DesktopCell cell) => horizontal
          ? (x(cell) - x(current)).abs() * 100 +
                (cell.row - current.row).abs() * 120
          : (cell.row - current.row).abs() * 100 +
                (cell.column - current.column).abs() * 120;
      candidates.sort((a, b) => distance(a.value).compareTo(distance(b.value)));
    } else {
      candidates.removeWhere((entry) => entry.value.page != _page);
      candidates.sort((a, b) {
        final aSlot =
            a.value.page * grid.capacity +
            a.value.column * grid.rows +
            a.value.row;
        final bSlot =
            b.value.page * grid.capacity +
            b.value.column * grid.rows +
            b.value.row;
        return aSlot.compareTo(bSlot);
      });
    }
    if (candidates.isEmpty) return;
    setState(() {
      final target = candidates.first;
      if (extend && target.value.page == _page) {
        _selectRange(target.key);
      } else {
        _selectOnly(target.key);
        _page = target.value.page;
      }
    });
  }

  void _changePage(int page) {
    final grid = _grid;
    if (grid == null) return;
    final next = page
        .clamp(0, grid.pageCount(widget.service.entries.length) - 1)
        .toInt();
    if (next == _page) return;
    Tooltip.dismissAllToolTips();
    setState(() {
      _page = next;
      _selectOnly(null);
      _dragging = null;
      _dragSelection = const {};
      _dragPoint = null;
      _marquee = null;
    });
  }

  void _startMarquee(DragStartDetails details) {
    final grid = _grid;
    final start = _marqueeStart;
    if (widget.shellStyle != ShellStyle.macos ||
        _interactionDepth > 0 ||
        widget.service.busy ||
        _dragging != null ||
        grid == null ||
        start == null ||
        !grid.area.contains(start) ||
        _visiblePaths.any(
          (path) => grid.tileRect(_positions[path]!).contains(start),
        )) {
      return;
    }
    Tooltip.dismissAllToolTips();
    _focus.requestFocus();
    _beforeMarquee = Set.of(_selection);
    _beforeMarqueeActive = _selected;
    _beforeMarqueeAnchor = _selectionAnchor;
    final keyboard = HardwareKeyboard.instance;
    _marqueeAdditive = keyboard.isMetaPressed || keyboard.isShiftPressed;
    _updateMarquee(_local(details.globalPosition));
  }

  void _updateMarquee(Offset point) {
    final grid = _grid;
    final start = _marqueeStart;
    if (grid == null || start == null) return;
    final end = Offset(
      point.dx.clamp(grid.area.left, grid.area.right).toDouble(),
      point.dy.clamp(grid.area.top, grid.area.bottom).toDouble(),
    );
    setState(() {
      _marquee = Rect.fromPoints(start, end);
      final hit = _visiblePaths.where(
        (path) => _marquee!.overlaps(grid.tileRect(_positions[path]!)),
      );
      _selection
        ..clear()
        ..addAll(_marqueeAdditive ? _beforeMarquee : const [])
        ..addAll(hit);
      _selected = _selection.lastOrNull;
      _selectionAnchor = _selection.firstOrNull;
    });
  }

  void _cancelMarquee() {
    if (_marquee == null) return;
    setState(() {
      _selection
        ..clear()
        ..addAll(
          _beforeMarquee.where((path) => _positions[path]?.page == _page),
        );
      _selected = _selection.contains(_beforeMarqueeActive)
          ? _beforeMarqueeActive
          : _selection.firstOrNull;
      _selectionAnchor = _selection.contains(_beforeMarqueeAnchor)
          ? _beforeMarqueeAnchor
          : _selected;
      _marquee = null;
      _marqueeStart = null;
    });
  }

  KeyEventResult _key(FocusNode node, KeyEvent event) {
    // Editing chords belong to EditableText, never to desktop file actions.
    if (_renaming != null) return KeyEventResult.ignored;
    if (_interactionDepth > 0) return KeyEventResult.ignored;
    if (event is! KeyDownEvent && event is! KeyRepeatEvent) {
      return KeyEventResult.ignored;
    }
    final key = event.logicalKey;
    if (key == LogicalKeyboardKey.escape) {
      Tooltip.dismissAllToolTips();
      if (_dragging != null) {
        _cancelDrag();
      } else if (_marquee != null) {
        _cancelMarquee();
      } else {
        setState(() => _selectOnly(null));
      }
      return KeyEventResult.handled;
    }
    final macos = widget.shellStyle == ShellStyle.macos;
    final keyboard = HardwareKeyboard.instance;
    final unmodified =
        !keyboard.isMetaPressed &&
        !keyboard.isControlPressed &&
        !keyboard.isAltPressed &&
        !keyboard.isShiftPressed;
    bool matches(SingleActivator shortcut) => shortcut.accepts(event, keyboard);
    if (macos &&
        matches(const SingleActivator(LogicalKeyboardKey.keyA, meta: true))) {
      if (_dragging != null || _marquee != null || widget.service.busy) {
        return KeyEventResult.ignored;
      }
      setState(() {
        _selection
          ..clear()
          ..addAll(_visiblePaths);
        _selected = _selection.firstOrNull;
        _selectionAnchor = _selected;
      });
      return KeyEventResult.handled;
    }
    if (macos &&
        matches(
          const SingleActivator(
            LogicalKeyboardKey.keyN,
            meta: true,
            shift: true,
          ),
        )) {
      if (widget.service.busy || event is KeyRepeatEvent || _dragging != null) {
        return KeyEventResult.ignored;
      }
      unawaited(_create(_DesktopAction.createFolder));
      return KeyEventResult.handled;
    }
    if (macos &&
        matches(const SingleActivator(LogicalKeyboardKey.keyC, meta: true))) {
      if (widget.service.busy || _dragging != null) return KeyEventResult.ignored;
      final selected = _selectedEntries;
      if (selected.isNotEmpty) {
        widget.service.copySelection(selected);
        return KeyEventResult.handled;
      }
      return KeyEventResult.ignored;
    }
    if (macos &&
        matches(const SingleActivator(LogicalKeyboardKey.keyV, meta: true))) {
      if (widget.service.busy || _dragging != null || !widget.service.canPaste) {
        return KeyEventResult.ignored;
      }
      unawaited(widget.service.paste());
      return KeyEventResult.handled;
    }
    if (macos &&
        matches(const SingleActivator(LogicalKeyboardKey.keyD, meta: true))) {
      if (widget.service.busy || _dragging != null) return KeyEventResult.ignored;
      final selected = _selectedEntries;
      if (selected.isNotEmpty) {
        unawaited(widget.service.duplicateSelection(selected));
        return KeyEventResult.handled;
      }
      return KeyEventResult.ignored;
    }
    if ([
          LogicalKeyboardKey.arrowLeft,
          LogicalKeyboardKey.arrowRight,
          LogicalKeyboardKey.arrowUp,
          LogicalKeyboardKey.arrowDown,
        ].contains(key) &&
        (!macos ||
            unmodified ||
            (keyboard.isShiftPressed &&
                !keyboard.isMetaPressed &&
                !keyboard.isAltPressed &&
                !keyboard.isControlPressed))) {
      if (_dragging != null || _marquee != null) return KeyEventResult.ignored;
      _moveSelection(key, extend: macos && keyboard.isShiftPressed);
      return KeyEventResult.handled;
    }
    final entry = _selectedEntry;
    if (entry == null ||
        _positions[entry.fullPath]?.page != _page ||
        widget.service.busy ||
        event is KeyRepeatEvent) {
      return KeyEventResult.ignored;
    }
    if (macos) {
      if (_dragging != null || _marquee != null) return KeyEventResult.ignored;
      // Match complete chords: Cmd+Down opens the current selection instead of
      // moving it. Extra modifiers must not invoke an unrelated file action.
      if (matches(const SingleActivator(LogicalKeyboardKey.keyO, meta: true)) ||
          matches(
            const SingleActivator(LogicalKeyboardKey.arrowDown, meta: true),
          )) {
        unawaited(_openSelection(_selectedEntries));
      } else if (matches(
        const SingleActivator(LogicalKeyboardKey.keyI, meta: true),
      )) {
        unawaited(_inspectSelection(_selectedEntries, preview: false));
      } else if (matches(const SingleActivator(LogicalKeyboardKey.space)) ||
          matches(const SingleActivator(LogicalKeyboardKey.keyY, meta: true))) {
        unawaited(_inspectSelection(_selectedEntries, preview: true));
      } else if (matches(const SingleActivator(LogicalKeyboardKey.enter)) ||
          matches(const SingleActivator(LogicalKeyboardKey.numpadEnter))) {
        if (_selectedEntries.length != 1) return KeyEventResult.ignored;
        unawaited(_rename(entry));
      } else if (matches(
        const SingleActivator(LogicalKeyboardKey.backspace, meta: true),
      )) {
        // Apple labels the backward-delete key Delete (⌫). Flutter reports it
        // as backspace; forward Delete is a different key and stays inert.
        unawaited(_trashSelection(_selectedEntries));
      } else {
        return KeyEventResult.ignored;
      }
    } else if (key == LogicalKeyboardKey.enter) {
      unawaited(_open(entry));
    } else if (key == LogicalKeyboardKey.f2) {
      unawaited(_rename(entry));
    } else if (key == LogicalKeyboardKey.delete) {
      unawaited(_trash(entry));
    } else {
      return KeyEventResult.ignored;
    }
    return KeyEventResult.handled;
  }

  @override
  Widget build(BuildContext context) => Material(
    type: MaterialType.transparency,
    child: Focus(
      focusNode: _focus,
      onKeyEvent: _key,
      child: LayoutBuilder(
        builder: (context, constraints) {
          final largerText = math.max(
            0.0,
            MediaQuery.textScalerOf(context).scale(13) - 13,
          );
          final cellWidth = 128 + largerText * 2;
          final cellHeight = 132 + largerText * 3 * 1.2;
          final initialGrid = DesktopGridSpec.fromSize(
            constraints.biggest,
            barPosition: widget.barPosition,
            surfaceInsets: _surfaceInsets,
            cellWidth: cellWidth,
            cellHeight: cellHeight,
          );
          final needsFooter =
              widget.service.entries.length > initialGrid.capacity ||
              widget.service.error != null ||
              widget.layout.error != null;
          final grid = needsFooter
              ? DesktopGridSpec.fromSize(
                  Size(
                    constraints.maxWidth,
                    (constraints.maxHeight - 56)
                        .clamp(0, double.infinity)
                        .toDouble(),
                  ),
                  barPosition: widget.barPosition,
                  surfaceInsets: _surfaceInsets,
                  cellWidth: cellWidth,
                  cellHeight: cellHeight,
                )
              : initialGrid;
          _grid = grid;
          final macos = widget.shellStyle == ShellStyle.macos;
          final stacksEnabled =
              macos && widget.layout.stacksEnabled(widget.outputName);
          final stackGroups = stacksEnabled
              ? DesktopStacks.groupEntries(
                  widget.service.entries,
                  groupBy: widget.layout.stackGroupBy(widget.outputName),
                )
              : const <DesktopStackGroup>[];
          final unstackedFolders = stacksEnabled
              ? widget.service.entries.where((e) => e.isDirectory).toList()
              : const <DesktopEntry>[];

          if (stacksEnabled) {
            final resolved = <String, DesktopCell>{};
            var slot = 0;
            for (final folder in unstackedFolders) {
              if (grid.capacity > 0) {
                final local = slot % grid.capacity;
                resolved[folder.fullPath] = DesktopCell(
                  local ~/ grid.rows,
                  local % grid.rows,
                  page: slot ~/ grid.capacity,
                );
                slot++;
              }
            }
            for (final group in stackGroups) {
              if (grid.capacity > 0) {
                final local = slot % grid.capacity;
                resolved[group.id] = DesktopCell(
                  local ~/ grid.rows,
                  local % grid.rows,
                  page: slot ~/ grid.capacity,
                );
                slot++;
                if (_expandedStackId == group.id) {
                  for (final entry in group.entries) {
                    final childLocal = slot % grid.capacity;
                    resolved[entry.fullPath] = DesktopCell(
                      childLocal ~/ grid.rows,
                      childLocal % grid.rows,
                      page: slot ~/ grid.capacity,
                    );
                    slot++;
                  }
                }
              }
            }
            _positions = resolved;
          } else {
            _positions = DesktopLayout.resolveEntries(
              widget.service.entries,
              grid,
              {..._positions, ...widget.layout.positions(widget.outputName)},
              sortKey: macos ? widget.layout.sortKey(widget.outputName) : null,
            );
          }
          final totalSlots = stacksEnabled
              ? unstackedFolders.length +
                  stackGroups.length +
                  (stackGroups
                          .where((g) => g.id == _expandedStackId)
                          .firstOrNull
                          ?.entries
                          .length ??
                      0)
              : widget.service.entries.length;
          final pages = grid.pageCount(totalSlots);
          _page = _page.clamp(0, pages - 1);
          if (widget.shellStyle == ShellStyle.macos) {
            _selection.removeWhere((path) {
              if (_renamePending &&
                  path == _renaming?.fullPath &&
                  _renameCell?.page == _page) {
                return false;
              }
              return _positions[path]?.page != _page;
            });
            if (!_selection.contains(_selected)) {
              _selected = _selection.firstOrNull;
            }
            if (!_selection.contains(_selectionAnchor)) {
              _selectionAnchor = _selected;
            }
          }
          return GestureDetector(
            key: _surface,
            behavior: HitTestBehavior.opaque,
            onTap: () {
              if (_interactionDepth > 0) return;
              _focus.requestFocus();
              setState(() => _selectOnly(null));
            },
            onTapDown: widget.shellStyle == ShellStyle.macos
                ? (details) {
                    if (_controlClick) unawaited(_menu(details.globalPosition));
                  }
                : null,
            onPanDown: widget.shellStyle == ShellStyle.macos
                ? (details) => _marqueeStart = _local(details.globalPosition)
                : null,
            onPanStart: widget.shellStyle == ShellStyle.macos
                ? _startMarquee
                : null,
            onPanUpdate: widget.shellStyle == ShellStyle.macos
                ? (details) {
                    if (_marquee != null) {
                      _updateMarquee(_local(details.globalPosition));
                    }
                  }
                : null,
            onPanEnd: widget.shellStyle == ShellStyle.macos
                ? (_) {
                    if (_marquee != null) {
                      setState(() {
                        _marquee = null;
                        _marqueeStart = null;
                      });
                    }
                  }
                : null,
            onPanCancel: widget.shellStyle == ShellStyle.macos
                ? _cancelMarquee
                : null,
            onSecondaryTapUp: (details) =>
                unawaited(_menu(details.globalPosition)),
            child: Listener(
              onPointerSignal: (event) {
                if (event is! PointerScrollEvent ||
                    pages <= 1 ||
                    _interactionDepth > 0 ||
                    _dragging != null ||
                    _marquee != null) {
                  return;
                }
                if (_renaming != null) return;
                final now = DateTime.now();
                if (_lastWheel != null &&
                    now.difference(_lastWheel!).inMilliseconds < 180) {
                  return;
                }
                _lastWheel = now;
                final delta =
                    event.scrollDelta.dx.abs() > event.scrollDelta.dy.abs()
                    ? event.scrollDelta.dx
                    : event.scrollDelta.dy;
                _changePage(_page + (delta > 0 ? 1 : -1));
              },
              child: Stack(
                key: const ValueKey('desktop-surface'),
                fit: StackFit.expand,
                children: [
                  const Positioned.fill(
                    child: CustomPaint(
                      key: ValueKey('desktop-clear-frame'),
                      painter: _DesktopClearPainter(),
                    ),
                  ),
                  IgnorePointer(
                    key: const ValueKey('desktop-icon-layer'),
                    ignoring: _interactionDepth > 0,
                    child: Stack(
                      children: [
                        if (stacksEnabled) ...[
                          for (final folder in unstackedFolders)
                            if (_positions[folder.fullPath]?.page == _page)
                              _tile(folder, grid),
                          for (final group in stackGroups) ...[
                            if (_positions[group.id]?.page == _page)
                              _stackTile(group, grid),
                            if (_expandedStackId == group.id)
                              for (final entry in group.entries)
                                if (_positions[entry.fullPath]?.page == _page)
                                  _tile(entry, grid),
                          ],
                        ] else
                          for (final entry in widget.service.entries)
                            if (_positions[entry.fullPath]?.page == _page)
                              _tile(entry, grid),
                        if (_marquee != null)
                          Positioned.fromRect(
                            rect: _marquee!,
                            child: IgnorePointer(
                              child: DecoratedBox(
                                key: const ValueKey(
                                  'desktop-selection-rectangle',
                                ),
                                decoration: BoxDecoration(
                                  color: const Color(0x26007aff),
                                  border: Border.all(
                                    color: const Color(0xaa007aff),
                                  ),
                                ),
                              ),
                            ),
                          ),
                        if (pages > 1)
                          Positioned(
                            right: _surfaceInsets.right + 16,
                            bottom: _surfaceInsets.bottom + 8,
                            child: Material(
                              color: Theme.of(context)
                                  .colorScheme
                                  .surfaceContainer
                                  .withValues(alpha: .92),
                              borderRadius: BorderRadius.circular(24),
                              child: Row(
                                mainAxisSize: MainAxisSize.min,
                                children: [
                                  IconButton(
                                    key: const ValueKey(
                                      'desktop-previous-page',
                                    ),
                                    onPressed: _page == 0
                                        ? null
                                        : () => _changePage(_page - 1),
                                    icon: const Icon(Icons.chevron_left),
                                  ),
                                  Text('${_page + 1} / $pages'),
                                  IconButton(
                                    key: const ValueKey('desktop-next-page'),
                                    onPressed: _page == pages - 1
                                        ? null
                                        : () => _changePage(_page + 1),
                                    icon: const Icon(Icons.chevron_right),
                                  ),
                                ],
                              ),
                            ),
                          ),
                        if (widget.service.error != null ||
                            widget.layout.error != null)
                          Positioned(
                            left: _surfaceInsets.left + 16,
                            right: _surfaceInsets.right + 16,
                            bottom: _surfaceInsets.bottom + 8,
                            child: Material(
                              key: const ValueKey('desktop-error'),
                              color: Theme.of(
                                context,
                              ).colorScheme.errorContainer,
                              borderRadius: BorderRadius.circular(12),
                              child: Padding(
                                padding: const EdgeInsets.all(12),
                                child: Text(
                                  widget.service.error ?? widget.layout.error!,
                                ),
                              ),
                            ),
                          ),
                      ],
                    ),
                  ),
                ],
              ),
            ),
          );
        },
      ),
    ),
  );

  Widget _nameTooltip(DesktopEntry entry, DesktopGridSpec grid, Widget child) {
    if (_renaming?.fullPath == entry.fullPath) return child;
    if (grid.area.width < 48 || grid.area.height < 48) return child;
    final media = MediaQuery.of(context);
    final colors = Theme.of(context).colorScheme;
    final maximumWidth = math.min(520.0, grid.area.width);
    final maximumHeight = grid.area.height;
    // Leave space for margin, padding and the surrounding rich-text line.
    final bodyHeight = math.max(1.0, math.min(180.0, maximumHeight - 46));
    return MediaQuery(
      // WidgetSpan scales its whole child automatically. Scale the name Text
      // explicitly once so accessibility settings cannot enlarge the viewport.
      data: media.copyWith(textScaler: TextScaler.noScaling),
      child: Tooltip(
        richMessage: WidgetSpan(
          alignment: PlaceholderAlignment.top,
          child: Semantics(
            label: entry.displayName,
            child: ExcludeSemantics(
              child: ConstrainedBox(
                key: ValueKey('desktop-hover-name-viewport-${entry.fullPath}'),
                constraints: BoxConstraints(
                  maxWidth: maximumWidth - 32,
                  maxHeight: bodyHeight,
                ),
                child: SingleChildScrollView(
                  primary: false,
                  child: Text(
                    entry.displayName,
                    key: ValueKey('desktop-hover-name-${entry.fullPath}'),
                    textScaler: media.textScaler,
                    style: _tooltipNameStyle.copyWith(
                      color: colors.onInverseSurface,
                    ),
                  ),
                ),
              ),
            ),
          ),
        ),
        excludeFromSemantics: true,
        ignorePointer: false,
        enableTapToDismiss: false,
        triggerMode: TooltipTriggerMode.manual,
        waitDuration: const Duration(milliseconds: 400),
        showDuration: const Duration(seconds: 5),
        constraints: BoxConstraints(
          maxWidth: maximumWidth,
          maxHeight: maximumHeight,
        ),
        padding: const EdgeInsets.all(8),
        margin: const EdgeInsets.all(8),
        textStyle: _tooltipNameStyle.copyWith(color: colors.onInverseSurface),
        decoration: BoxDecoration(
          color: colors.inverseSurface,
          borderRadius: BorderRadius.circular(12),
        ),
        positionDelegate: (position) {
          final size = position.tooltipSize;
          final top = _surfaceInsets.top + 8;
          final left = _surfaceInsets.left + 8;
          final right = position.overlaySize.width - (_surfaceInsets.right + 8);
          final bottom =
              position.overlaySize.height - (_surfaceInsets.bottom + 8);
          final below = position.target.dy + position.verticalOffset;
          final above =
              position.target.dy - position.verticalOffset - size.height;
          final preferred = below + size.height <= bottom ? below : above;
          return Offset(
            (position.target.dx - size.width / 2)
                .clamp(left, math.max(left, right - size.width))
                .toDouble(),
            preferred
                .clamp(top, math.max(top, bottom - size.height))
                .toDouble(),
          );
        },
        child: MediaQuery(data: media, child: child),
      ),
    );
  }

  Widget _inlineNameEditor(DesktopEntry entry) => CallbackShortcuts(
    bindings: {
      const SingleActivator(LogicalKeyboardKey.enter): () =>
          unawaited(_commitInlineRename()),
      const SingleActivator(LogicalKeyboardKey.numpadEnter): () =>
          unawaited(_commitInlineRename()),
      const SingleActivator(LogicalKeyboardKey.escape): () {
        if (!_renamePending) _cancelInlineRename();
      },
    },
    child: TextField(
      key: const ValueKey('desktop-inline-name'),
      controller: _renameText,
      focusNode: _renameFocus,
      readOnly: _renamePending,
      enableSuggestions: false,
      autocorrect: false,
      smartDashesType: SmartDashesType.disabled,
      smartQuotesType: SmartQuotesType.disabled,
      textAlign: TextAlign.center,
      textInputAction: TextInputAction.done,
      minLines: 1,
      maxLines: 3,
      style: const TextStyle(color: Colors.black, fontSize: 13, height: 1.2),
      cursorColor: const Color(0xff007aff),
      decoration: InputDecoration(
        isDense: true,
        filled: true,
        fillColor: Colors.white,
        contentPadding: const EdgeInsets.symmetric(horizontal: 3, vertical: 2),
        enabledBorder: OutlineInputBorder(
          borderRadius: BorderRadius.circular(2),
          borderSide: BorderSide(
            color: _renameFailed
                ? Theme.of(context).colorScheme.error
                : const Color(0xff8e8e93),
          ),
        ),
        focusedBorder: OutlineInputBorder(
          borderRadius: BorderRadius.circular(2),
          borderSide: BorderSide(
            color: _renameFailed
                ? Theme.of(context).colorScheme.error
                : const Color(0xff007aff),
          ),
        ),
      ),
      onEditingComplete: () {},
      onSubmitted: (_) => unawaited(_commitInlineRename()),
      onTapOutside: (_) => unawaited(_commitInlineRename()),
    ),
  );

  Widget _tile(DesktopEntry entry, DesktopGridSpec grid) {
    final macos = widget.shellStyle == ShellStyle.macos;
    final cell = _positions[entry.fullPath]!;
    final dragging =
        _dragging != null &&
        (macos
            ? _dragSelection.contains(entry.fullPath)
            : _dragging == entry.fullPath);
    final original = grid.tileRect(cell);
    final rect = dragging && _dragPoint != null
        ? macos
              ? original.shift(_groupDragOffset(grid))
              : Rect.fromCenter(
                  center: Offset(
                    _dragPoint!.dx
                        .clamp(
                          grid.area.left + original.width / 2,
                          grid.area.right - original.width / 2,
                        )
                        .toDouble(),
                    _dragPoint!.dy
                        .clamp(
                          grid.area.top + original.height / 2,
                          grid.area.bottom - original.height / 2,
                        )
                        .toDouble(),
                  ),
                  width: original.width,
                  height: original.height,
                )
        : original;
    final selected = macos
        ? _selection.contains(entry.fullPath)
        : _selected == entry.fullPath;
    // Reserve a readable name line before sizing the icon on short outputs.
    // Normal tiles retain the original 48-point icon and 6-point gap.
    final contentHeight = math.max(0.0, rect.height - 14);
    final nameHeight = math.min(
      contentHeight,
      MediaQuery.textScalerOf(context).scale(13) * 1.2 + 6,
    );
    final iconSize = (contentHeight - nameHeight - 6).clamp(0.0, 48.0);
    final iconGap = iconSize == 0
        ? 0.0
        : math.min(6.0, math.max(0.0, contentHeight - nameHeight - iconSize));
    return Positioned.fromRect(
      key: ValueKey('desktop-position-${entry.fullPath}'),
      rect: rect,
      child: Semantics(
        label:
            '${entry.displayName}，${entry.isDirectory
                ? '文件夹'
                : entry.isDesktopShortcut
                ? '快捷方式'
                : '文件'}',
        button: true,
        selected: selected,
        child: Listener(
          onPointerDown: macos
              ? (event) {
                  if (event.buttons == kPrimaryMouseButton) {
                    if (_controlClick) {
                      unawaited(_menu(event.position, entry));
                      return;
                    }
                    _pointerSelect(entry);
                  }
                }
              : null,
          child: GestureDetector(
            key: ValueKey('desktop-entry-${entry.fullPath}'),
            behavior: HitTestBehavior.opaque,
            onTapDown: macos ? null : (_) => _pointerSelect(entry),
            onTap: () {
              if (_collapseSelectionOnTap && _selected == entry.fullPath) {
                setState(() => _selectOnly(entry.fullPath));
              }
              _collapseSelectionOnTap = false;
            },
            onDoubleTap: () {
              // The double-tap recognizer can win before the first tap's
              // selection callback. Keyboard actions must target this icon.
              _focus.requestFocus();
              setState(() => _selectOnly(entry.fullPath));
              unawaited(_open(entry));
            },
            onSecondaryTapUp: (details) =>
                unawaited(_menu(details.globalPosition, entry)),
            onPanStart: (details) {
              if (widget.service.busy || _interactionDepth > 0) return;
              if (_renaming != null) return;
              Tooltip.dismissAllToolTips();
              _focus.requestFocus();
              setState(() {
                if (!macos || !_selection.contains(entry.fullPath)) {
                  _selectOnly(entry.fullPath);
                }
                _collapseSelectionOnTap = false;
                _dragging = entry.fullPath;
                _dragSelection = _selectedEntries
                    .map((entry) => entry.fullPath)
                    .toSet();
                _dragPoint = _local(details.globalPosition);
              });
            },
            onPanUpdate: (details) {
              if (_dragging != entry.fullPath) return;
              setState(() => _dragPoint = _local(details.globalPosition));
            },
            onPanEnd: (_) => unawaited(_drop()),
            onPanCancel: _cancelDrag,
            child: MouseRegion(
              cursor: macos
                  ? SystemMouseCursors.basic
                  : SystemMouseCursors.click,
              child: AnimatedContainer(
                duration: widget.reduceMotion
                    ? Duration.zero
                    : const Duration(milliseconds: 120),
                decoration: BoxDecoration(
                  color: selected && !macos
                      ? Theme.of(
                          context,
                        ).colorScheme.primaryContainer.withValues(alpha: .72)
                      : Colors.transparent,
                  borderRadius: BorderRadius.circular(macos ? 4 : 16),
                  border: Border.all(
                    color: selected && !macos
                        ? Theme.of(
                            context,
                          ).colorScheme.primary.withValues(alpha: .8)
                        : Colors.transparent,
                  ),
                ),
                child: _nameTooltip(
                  entry,
                  grid,
                  Padding(
                    padding: const EdgeInsets.symmetric(
                      horizontal: 4,
                      vertical: 6,
                    ),
                    child: Column(
                      children: [
                        if (macos)
                          DecoratedBox(
                            key: ValueKey(
                              'desktop-icon-selection-${entry.fullPath}',
                            ),
                            decoration: BoxDecoration(
                              color: selected
                                  ? Colors.black.withValues(alpha: .22)
                                  : Colors.transparent,
                              borderRadius: BorderRadius.circular(4),
                            ),
                            child: _entryIcon(entry, size: iconSize),
                          )
                        else
                          _entryIcon(entry, size: iconSize),
                        SizedBox(height: iconGap),
                        Flexible(
                          child: _renaming?.fullPath == entry.fullPath
                              ? _inlineNameEditor(entry)
                              : Container(
                                  padding: const EdgeInsets.symmetric(
                                    horizontal: 6,
                                    vertical: 2,
                                  ),
                                  decoration: BoxDecoration(
                                    color: macos
                                        ? selected
                                              ? const Color(0xff007aff)
                                              : Colors.transparent
                                        : Colors.black.withValues(alpha: .62),
                                    borderRadius: BorderRadius.circular(
                                      macos ? 3 : 7,
                                    ),
                                  ),
                                  child: Text(
                                    entry.displayName,
                                    key: ValueKey(
                                      'desktop-label-${entry.fullPath}',
                                    ),
                                    maxLines: 3,
                                    overflow: TextOverflow.ellipsis,
                                    textAlign: TextAlign.center,
                                    style: TextStyle(
                                      color: Colors.white,
                                      fontSize: 13,
                                      height: 1.2,
                                      shadows: macos
                                          ? const [
                                              Shadow(
                                                color: Colors.black54,
                                                blurRadius: 3,
                                                offset: Offset(0, 1),
                                              ),
                                            ]
                                          : null,
                                    ),
                                  ),
                                ),
                        ),
                      ],
                    ),
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }

  Widget _stackTile(DesktopStackGroup group, DesktopGridSpec grid) {
    final cell = _positions[group.id];
    if (cell == null) return const SizedBox.shrink();
    final original = grid.tileRect(cell);
    final isExpanded = _expandedStackId == group.id;
    final selected = _selected == group.id;

    final contentHeight = math.max(0.0, original.height - 14);
    final nameHeight = math.min(
      contentHeight,
      MediaQuery.textScalerOf(context).scale(13) * 1.2 + 6,
    );
    final iconSize = (contentHeight - nameHeight - 6).clamp(0.0, 48.0);
    final iconGap = iconSize == 0
        ? 0.0
        : math.min(6.0, math.max(0.0, contentHeight - nameHeight - iconSize));

    return Positioned.fromRect(
      key: ValueKey('desktop-position-${group.id}'),
      rect: original,
      child: Semantics(
        label: '${group.title}，叠放',
        button: true,
        selected: selected,
        child: GestureDetector(
          key: ValueKey('desktop-stack-${group.id}'),
          behavior: HitTestBehavior.opaque,
          onTap: () {
            _focus.requestFocus();
            setState(() {
              _selected = group.id;
              _expandedStackId = isExpanded ? null : group.id;
            });
          },
          onSecondaryTapUp: (details) => unawaited(
            _menu(details.globalPosition),
          ),
          child: MouseRegion(
            cursor: SystemMouseCursors.click,
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 4, vertical: 6),
              child: Column(
                children: [
                  SizedBox(
                    width: iconSize,
                    height: iconSize,
                    child: Stack(
                      alignment: Alignment.center,
                      clipBehavior: Clip.none,
                      children: [
                        if (group.entries.length > 2)
                          Transform.translate(
                            offset: const Offset(4, -4),
                            child: Transform.rotate(
                              angle: 0.08,
                              child: Opacity(
                                opacity: 0.55,
                                child: group.primaryEntry != null
                                    ? _entryIcon(
                                        group.primaryEntry!,
                                        size: iconSize * 0.9,
                                      )
                                    : const Icon(Icons.folder, size: 36),
                              ),
                            ),
                          ),
                        if (group.entries.length > 1)
                          Transform.translate(
                            offset: const Offset(-3, -2),
                            child: Transform.rotate(
                              angle: -0.06,
                              child: Opacity(
                                opacity: 0.75,
                                child: group.primaryEntry != null
                                    ? _entryIcon(
                                        group.primaryEntry!,
                                        size: iconSize * 0.95,
                                      )
                                    : const Icon(Icons.folder, size: 38),
                              ),
                            ),
                          ),
                        if (group.primaryEntry != null)
                          _entryIcon(group.primaryEntry!, size: iconSize)
                        else
                          Icon(
                            Icons.folder_copy,
                            size: iconSize,
                            color: const Color(0xff007aff),
                          ),
                        Positioned(
                          right: -4,
                          bottom: -2,
                          child: Container(
                            padding: const EdgeInsets.symmetric(
                              horizontal: 4,
                              vertical: 1,
                            ),
                            decoration: BoxDecoration(
                              color: isExpanded
                                  ? const Color(0xff34c759)
                                  : const Color(0xff007aff),
                              borderRadius: BorderRadius.circular(9),
                              border: Border.all(color: Colors.white, width: 1),
                              boxShadow: const [
                                BoxShadow(
                                  color: Colors.black26,
                                  blurRadius: 2,
                                  offset: Offset(0, 1),
                                ),
                              ],
                            ),
                            child: Text(
                              '${group.count}',
                              style: const TextStyle(
                                color: Colors.white,
                                fontSize: 9,
                                fontWeight: FontWeight.bold,
                              ),
                            ),
                          ),
                        ),
                      ],
                    ),
                  ),
                  SizedBox(height: iconGap),
                  Flexible(
                    child: Container(
                      padding: const EdgeInsets.symmetric(
                        horizontal: 6,
                        vertical: 2,
                      ),
                      decoration: BoxDecoration(
                        color: isExpanded
                            ? const Color(0xcc007aff)
                            : Colors.transparent,
                        borderRadius: BorderRadius.circular(3),
                      ),
                      child: Text(
                        group.title,
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                        textAlign: TextAlign.center,
                        style: const TextStyle(
                          color: Colors.white,
                          fontSize: 13,
                          height: 1.2,
                          shadows: [
                            Shadow(
                              color: Colors.black54,
                              blurRadius: 3,
                              offset: Offset(0, 1),
                            ),
                          ],
                        ),
                      ),
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }

  Offset _groupDragOffset(DesktopGridSpec grid) {
    final anchor = _positions[_dragging];
    if (anchor == null || _dragPoint == null) return Offset.zero;
    var bounds = grid.tileRect(anchor);
    for (final path in _dragSelection) {
      final cell = _positions[path];
      if (cell != null) bounds = bounds.expandToInclude(grid.tileRect(cell));
    }
    final delta = _dragPoint! - grid.tileRect(anchor).center;
    return Offset(
      delta.dx
          .clamp(grid.area.left - bounds.left, grid.area.right - bounds.right)
          .toDouble(),
      delta.dy
          .clamp(grid.area.top - bounds.top, grid.area.bottom - bounds.bottom)
          .toDouble(),
    );
  }

  Widget _entryIcon(DesktopEntry entry, {double size = 48}) {
    final macos = widget.shellStyle == ShellStyle.macos;
    final Widget icon;
    if (entry.isDesktopShortcut || entry.iconPath != null) {
      icon = AppIcon(
        app: DesktopApp(
          id: entry.fullPath,
          name: entry.displayName,
          desktopFile: entry.fullPath,
          iconName: entry.iconName,
          iconPath: entry.iconPath,
        ),
        size: size,
      );
    } else if (macos) {
      icon = OptionalAssetIcon(
        fallback: entry.isDirectory
            ? Icons.folder_rounded
            : Icons.insert_drive_file_rounded,
        asset: 'assets/macos/${entry.isDirectory ? 'generic-folder' : 'generic-document'}.png',
        width: size,
        height: size,
        filterQuality: FilterQuality.high,
      );
    } else {
      icon = Icon(
        entry.isDirectory
            ? Icons.folder_rounded
            : Icons.insert_drive_file_rounded,
        size: size,
        color: entry.isDirectory ? const Color(0xffffd785) : Colors.white,
        shadows: const [Shadow(color: Colors.black54, blurRadius: 8)],
      );
    }
    if (!macos || (!entry.isSymlink && !entry.isDesktopShortcut)) return icon;
    return SizedBox.square(
      dimension: size,
      child: Stack(
        children: [
          icon,
          Positioned(
            left: 0,
            bottom: 0,
            child: OptionalAssetIcon(
              asset: 'assets/macos/alias-badge.png',
              fallback: Icons.shortcut_rounded,
              fallbackColor: Colors.white,
              width: size * 14 / 48,
              height: size * 14 / 48,
            ),
          ),
        ],
      ),
    );
  }
}

/// A compact desktop menu keeps AppKit-style selection feedback while using
/// Flutter's popup route for focus traversal, activation and dismissal.
/// Linux's compositor retains its previous framebuffer for a zero-layer frame.
/// Keep a full-size paint operation even when the final icon disappears. A
/// clear blend discards both source and destination, producing only zero-alpha
/// pixels. Keep Paint's default source: Flutter's DlOpSpy ignores blend modes
/// and would mistake a transparent source for no rendered contents.
class _DesktopClearPainter extends CustomPainter {
  const _DesktopClearPainter();

  @override
  void paint(Canvas canvas, Size size) {
    canvas.drawRect(
      Offset.zero & size,
      Paint()
        ..isAntiAlias = false
        ..blendMode = BlendMode.clear,
    );
  }

  @override
  bool shouldRepaint(covariant _DesktopClearPainter oldDelegate) => false;
}

class _NameDialog extends StatefulWidget {
  const _NameDialog({required this.title, required this.initial});
  final String title;
  final String initial;
  @override
  State<_NameDialog> createState() => _NameDialogState();
}

class _NameDialogState extends State<_NameDialog> {
  late final _name = TextEditingController(text: widget.initial);
  @override
  void dispose() {
    _name.dispose();
    super.dispose();
  }

  void _submit() {
    final name = _name.text.trim();
    if (name.isNotEmpty) Navigator.pop(context, name);
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: Text(widget.title),
    content: TextField(
      key: const ValueKey('desktop-name'),
      controller: _name,
      autofocus: true,
      decoration: const InputDecoration(labelText: '名称'),
      onSubmitted: (_) => _submit(),
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.pop(context),
        child: const Text('取消'),
      ),
      FilledButton(
        key: const ValueKey('desktop-name-confirm'),
        onPressed: _submit,
        child: const Text('确定'),
      ),
    ],
  );
}

class _ShortcutChoice {
  const _ShortcutChoice({this.app, this.path, this.name});
  final DesktopApp? app;
  final String? path;
  final String? name;
}

class _ShortcutDialog extends StatefulWidget {
  const _ShortcutDialog({required this.apps});
  final List<DesktopApp> apps;
  @override
  State<_ShortcutDialog> createState() => _ShortcutDialogState();
}

class _ShortcutDialogState extends State<_ShortcutDialog> {
  final _path = TextEditingController();
  final _name = TextEditingController();
  late bool _application = widget.apps.isNotEmpty;
  DesktopApp? _selected;
  String? _error;
  bool _checking = false;
  @override
  void initState() {
    super.initState();
    _selected = widget.apps.firstOrNull;
  }

  @override
  void dispose() {
    _path.dispose();
    _name.dispose();
    super.dispose();
  }

  Future<void> _submit() async {
    if (_checking) return;
    final name = _name.text.trim();
    if (_application) {
      if (_selected != null) {
        Navigator.pop(
          context,
          _ShortcutChoice(app: _selected, name: name.isEmpty ? null : name),
        );
      }
      return;
    }
    final path = _path.text.trim();
    if (!path.startsWith('/') || path.contains('\x00')) {
      setState(() => _error = '请输入本地文件或文件夹的绝对路径');
      return;
    }
    setState(() => _checking = true);
    final type = await FileSystemEntity.type(path);
    if (!mounted) return;
    setState(() => _checking = false);
    if (type == FileSystemEntityType.notFound) {
      setState(() => _error = '该文件或文件夹不存在');
      return;
    }
    Navigator.pop(
      context,
      _ShortcutChoice(path: path, name: name.isEmpty ? null : name),
    );
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: const Text('新建快捷方式'),
    content: SizedBox(
      width: 420,
      child: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            SegmentedButton<bool>(
              segments: [
                ButtonSegment(
                  value: true,
                  label: const Text('已安装应用'),
                  enabled: widget.apps.isNotEmpty,
                ),
                const ButtonSegment(value: false, label: Text('本地文件')),
              ],
              selected: {_application},
              onSelectionChanged: (value) => setState(() {
                _application = value.single;
                _error = null;
              }),
            ),
            const SizedBox(height: 16),
            if (_application)
              DropdownButtonFormField<DesktopApp>(
                key: const ValueKey('desktop-shortcut-app'),
                initialValue: _selected,
                isExpanded: true,
                items: [
                  for (final app in widget.apps)
                    DropdownMenuItem(
                      value: app,
                      child: Text(app.name, overflow: TextOverflow.ellipsis),
                    ),
                ],
                onChanged: (value) => setState(() => _selected = value),
              )
            else
              TextField(
                key: const ValueKey('desktop-shortcut-path'),
                controller: _path,
                decoration: const InputDecoration(
                  labelText: '文件或文件夹路径',
                  hintText: '/home/…',
                ),
                onSubmitted: (_) => unawaited(_submit()),
              ),
            const SizedBox(height: 12),
            TextField(
              key: const ValueKey('desktop-shortcut-name'),
              controller: _name,
              decoration: const InputDecoration(labelText: '名称（可选）'),
            ),
            if (_error != null)
              Padding(
                padding: const EdgeInsets.only(top: 12),
                child: Text(
                  _error!,
                  style: TextStyle(color: Theme.of(context).colorScheme.error),
                ),
              ),
          ],
        ),
      ),
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.pop(context),
        child: const Text('取消'),
      ),
      FilledButton(
        key: const ValueKey('desktop-shortcut-confirm'),
        onPressed: _checking ? null : () => unawaited(_submit()),
        child: const Text('创建'),
      ),
    ],
  );
}
