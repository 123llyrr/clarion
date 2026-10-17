// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';

import 'optional_asset_icon.dart';

import 'package:flutter/services.dart';

import '../mac_shell_preferences.dart';
import '../models.dart';
import '../services/dock_folder_service.dart';
import 'app_icon.dart';

/// Folder/Stack controls the closed tile; Fan/Grid/List controls its contents.
class MacDownloadsStackIcon extends StatelessWidget {
  const MacDownloadsStackIcon({
    super.key,
    required this.service,
    required this.preferences,
    this.size = 48,
    this.rasterSize,
  });

  final DockFolderService service;
  final MacShellPreferences preferences;
  final double size;
  final double? rasterSize;

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: service,
    builder: (context, _) {
      final items = service
          .sortedEntries(preferences.downloadsSort)
          .take(3)
          .toList();
      if (preferences.downloadsDisplay == MacDownloadsDisplay.folder ||
          items.isEmpty) {
        return OptionalAssetIcon(
          key: const ValueKey('downloads-folder-icon'),
          asset: 'assets/icons/downloads.svg',
          fallback: Icons.folder_special_rounded,
          rasterSize:
              ((rasterSize ?? size) * MediaQuery.devicePixelRatioOf(context))
                  .ceil(),
          width: size,
          height: size,
        );
      }
      return SizedBox.square(
        key: const ValueKey('downloads-stack-icon'),
        dimension: size,
        child: Stack(
          clipBehavior: Clip.none,
          alignment: Alignment.center,
          children: [
            for (var index = items.length - 1; index >= 0; index--)
              Transform.translate(
                offset: Offset(index * size * .055, -index * size * .055),
                child: Transform.rotate(
                  angle: index * .07,
                  child: _entryIcon(items[index], size, rasterSize: rasterSize),
                ),
              ),
          ],
        ),
      );
    },
  );
}

const _fanMaximumFiles = 8;
const _fanChrome = 48.0;
const _fanTileHeight = 64.0;
const _fanStep = 48.0;
const _fanPadding = 16.0;
const _fanFolderId = '__fan_folder__';

int _fanSlots(Size available) =>
    ((available.height - _fanChrome - _fanTileHeight - _fanPadding) / _fanStep)
        .floor()
        .clamp(0, _fanMaximumFiles) +
    1;

/// Explicit Fan keeps its layout for large folders, with a bounded set of
/// files followed by Open in File Manager. Automatic may choose the grid.
MacDownloadsView resolveDownloadsView({
  required MacDownloadsView requested,
  required MacDockPosition dockPosition,
  required Size available,
  required int entryCount,
}) {
  if (requested == MacDownloadsView.list) return MacDownloadsView.list;
  if (requested == MacDownloadsView.grid) return MacDownloadsView.grid;
  final hasFanSpace =
      dockPosition == MacDockPosition.bottom &&
      available.width >= 300 &&
      entryCount > 0 &&
      available.height >= _fanChrome + _fanTileHeight + _fanPadding + _fanStep;
  if (!hasFanSpace) return MacDownloadsView.grid;
  if (requested == MacDownloadsView.fan) return MacDownloadsView.fan;
  final automaticFan =
      available.width >= 360 &&
      entryCount <= _fanMaximumFiles &&
      _fanSlots(available) >= entryCount + 1;
  return automaticFan ? MacDownloadsView.fan : MacDownloadsView.grid;
}

class MacDownloadsStackPanel extends StatefulWidget {
  const MacDownloadsStackPanel({
    super.key,
    required this.service,
    required this.preferences,
    required this.onPreferencesChanged,
    required this.onClose,
    this.fanAnchorX,
    this.fanAnchorYBelowPanel,
    this.fanOriginIconSize,
  });

  final DockFolderService service;
  final MacShellPreferences preferences;
  final ValueChanged<MacShellPreferences> onPreferencesChanged;
  final VoidCallback onClose;

  /// The Dock icon centre relative to this panel's actual native position.
  final double? fanAnchorX;

  /// Dock icon centre relative to the panel's bottom, stable across its height.
  final double? fanAnchorYBelowPanel;

  /// Actual presented Dock icon width, including fit/magnification transforms.
  final double? fanOriginIconSize;

  @override
  State<MacDownloadsStackPanel> createState() => _MacDownloadsStackPanelState();
}

class _MacDownloadsStackPanelState extends State<MacDownloadsStackPanel>
    with SingleTickerProviderStateMixin {
  final _scroll = ScrollController();
  final _keyboard = FocusNode(debugLabel: 'mac-downloads-stack');
  late final AnimationController _fanAnimation;
  bool _reducedMotion = false;
  bool _closing = false;
  bool _fanActive = false;
  List<DockFolderEntry> _visibleItems = const [];
  String? _selectedId;
  String? _openingId;
  String? _operationError;
  int _columns = 1;
  double _rowExtent = 40;
  double _rowSpacing = 0;

  @override
  void initState() {
    super.initState();
    _fanAnimation = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 320),
      reverseDuration: const Duration(milliseconds: 220),
    );
    // The separate Dock app can own primary focus while this app's scope
    // remembers a previous menu. Autofocus cannot replace that focused child.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted && !_closing) _keyboard.requestFocus();
    });
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _reducedMotion = MediaQuery.disableAnimationsOf(context);
    if (_reducedMotion) {
      _fanAnimation.stop();
      _fanAnimation.value = 1;
    }
  }

  void _setFanActive(bool active) {
    if (_fanActive == active) return;
    _fanActive = active;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || _fanActive != active || _closing) return;
      if (!active || _reducedMotion) {
        _fanAnimation.value = 1;
      } else {
        _fanAnimation.forward(from: 0);
      }
    });
  }

  Future<void> _requestClose([VoidCallback? completed]) async {
    final callback = completed ?? widget.onClose;
    // A launch can complete after this State was reused for a newer panel.
    // Its captured revision callback still belongs to the original panel.
    if (completed != null &&
        widget.key == null &&
        !identical(callback, widget.onClose)) {
      callback();
      return;
    }
    if (_closing) return;
    setState(() => _closing = true);
    if (_fanActive && !_reducedMotion && _fanAnimation.value > 0) {
      try {
        await _fanAnimation.reverse().orCancel;
      } on TickerCanceled {
        return;
      }
    }
    if (mounted) callback();
  }

  Future<void> _open(DockFolderEntry? entry) async {
    if (_openingId != null || _closing) return;
    final onCompleted = widget.onClose;
    setState(() {
      _openingId = entry?.id ?? '__folder__';
      _operationError = null;
    });
    try {
      if (entry == null) {
        await widget.service.openFolder();
      } else {
        await widget.service.open(entry);
      }
      if (mounted) await _requestClose(onCompleted);
    } on Object catch (exception) {
      if (mounted) setState(() => _operationError = '无法打开：$exception');
    } finally {
      if (mounted) setState(() => _openingId = null);
    }
  }

  KeyEventResult _key(KeyEvent event) {
    if (event is! KeyDownEvent && event is! KeyRepeatEvent) {
      return KeyEventResult.ignored;
    }
    final key = event.logicalKey;
    if (key == LogicalKeyboardKey.escape) {
      unawaited(_requestClose());
      return KeyEventResult.handled;
    }
    if (_closing || _openingId != null) return KeyEventResult.handled;
    if (_fanActive && !_reducedMotion && _fanAnimation.value < .7) {
      return KeyEventResult.handled;
    }
    final items = _visibleItems;
    if (items.isEmpty) return KeyEventResult.ignored;
    final navigationLength = items.length + (_fanActive ? 1 : 0);
    var index = _selectedId == _fanFolderId && _fanActive
        ? items.length
        : items.indexWhere((entry) => entry.id == _selectedId);
    if (index < 0) index = 0;
    if (key == LogicalKeyboardKey.enter ||
        key == LogicalKeyboardKey.numpadEnter) {
      unawaited(_open(index == items.length ? null : items[index]));
      return KeyEventResult.handled;
    }
    final delta = switch (key) {
      LogicalKeyboardKey.arrowLeft => -1,
      LogicalKeyboardKey.arrowRight => 1,
      LogicalKeyboardKey.arrowUp => _fanActive ? 1 : -_columns,
      LogicalKeyboardKey.arrowDown => _fanActive ? -1 : _columns,
      _ => 0,
    };
    if (delta == 0 &&
        key != LogicalKeyboardKey.home &&
        key != LogicalKeyboardKey.end) {
      return KeyEventResult.ignored;
    }
    index = key == LogicalKeyboardKey.home
        ? 0
        : key == LogicalKeyboardKey.end
        ? navigationLength - 1
        : (index + delta).clamp(0, navigationLength - 1);
    setState(
      () =>
          _selectedId = index == items.length ? _fanFolderId : items[index].id,
    );
    if (_scroll.hasClients) {
      final target = ((index ~/ _columns) * (_rowExtent + _rowSpacing)).clamp(
        0.0,
        _scroll.position.maxScrollExtent,
      );
      if (MediaQuery.disableAnimationsOf(context)) {
        _scroll.jumpTo(target);
      } else {
        _scroll.animateTo(
          target,
          duration: const Duration(milliseconds: 160),
          curve: Curves.easeOutCubic,
        );
      }
    }
    return KeyEventResult.handled;
  }

  PopupMenuEntry<Object> _heading(String text) => PopupMenuItem<Object>(
    enabled: false,
    height: 24,
    child: Text(
      text,
      style: const TextStyle(fontSize: 11, fontWeight: FontWeight.w600),
    ),
  );

  Widget _settings() => PopupMenuButton<Object>(
    key: const ValueKey('downloads-stack-options'),
    enabled: !_closing && _openingId == null,
    tooltip: '下载叠放选项',
    iconSize: 17,
    padding: const EdgeInsets.all(5),
    constraints: const BoxConstraints(
      minWidth: 210,
      maxWidth: 250,
      maxHeight: 420,
    ),
    onSelected: (value) {
      // The popup is a route snapshot: metadata may change while it is open.
      if (value is MacDownloadsSort && !widget.service.supportsSort(value)) {
        return;
      }
      final updated = switch (value) {
        MacDownloadsSort sort => widget.preferences.copyWith(
          downloadsSort: sort,
        ),
        MacDownloadsDisplay display => widget.preferences.copyWith(
          downloadsDisplay: display,
        ),
        MacDownloadsView view => widget.preferences.copyWith(
          downloadsView: view,
        ),
        _ => widget.preferences,
      };
      widget.onPreferencesChanged(updated);
    },
    itemBuilder: (_) => [
      _heading('排序方式'),
      for (final sort in MacDownloadsSort.values)
        CheckedPopupMenuItem<Object>(
          key: ValueKey('downloads-sort-${sort.name}'),
          value: sort,
          height: 28,
          enabled: widget.service.supportsSort(sort),
          checked:
              widget.service.effectiveSort(widget.preferences.downloadsSort) ==
              sort,
          child: Tooltip(
            message: widget.service.supportsSort(sort) ? '' : '文件系统未提供可靠的此日期',
            child: Text(switch (sort) {
              MacDownloadsSort.name => '名称',
              MacDownloadsSort.dateAdded => '添加日期',
              MacDownloadsSort.dateModified => '修改日期',
              MacDownloadsSort.dateCreated => '创建日期',
              MacDownloadsSort.kind => '种类',
            }, style: const TextStyle(fontSize: 12)),
          ),
        ),
      const PopupMenuDivider(height: 8),
      _heading('显示为'),
      for (final display in MacDownloadsDisplay.values)
        CheckedPopupMenuItem<Object>(
          value: display,
          height: 28,
          checked: widget.preferences.downloadsDisplay == display,
          child: Text(
            display == MacDownloadsDisplay.folder ? '文件夹' : '叠放',
            style: const TextStyle(fontSize: 12),
          ),
        ),
      const PopupMenuDivider(height: 8),
      _heading('显示内容为'),
      for (final view in const [
        MacDownloadsView.fan,
        MacDownloadsView.grid,
        MacDownloadsView.list,
        MacDownloadsView.automatic,
      ])
        CheckedPopupMenuItem<Object>(
          value: view,
          height: 28,
          checked: widget.preferences.downloadsView == view,
          child: Text(switch (view) {
            MacDownloadsView.automatic => '自动',
            MacDownloadsView.fan => '扇状',
            MacDownloadsView.grid => '网格',
            MacDownloadsView.list => '列表',
          }, style: const TextStyle(fontSize: 12)),
        ),
    ],
    child: const Padding(
      padding: EdgeInsets.all(7),
      child: Icon(Icons.more_horiz, size: 18),
    ),
  );

  Widget _tile(DockFolderEntry entry, {bool list = false, bool fan = false}) {
    final selected = _selectedId == entry.id;
    final label = Text(
      entry.name,
      maxLines: list || fan ? 1 : 2,
      overflow: TextOverflow.ellipsis,
      textAlign: list || fan ? TextAlign.left : TextAlign.center,
      style: TextStyle(fontSize: 12, color: selected ? Colors.white : null),
    );
    final icon = _entryIcon(
      entry,
      list
          ? 24
          : fan
          ? 48
          : 52,
    );
    return Semantics(
      selected: selected,
      button: true,
      label: entry.name,
      child: Tooltip(
        message: entry.name,
        child: Material(
          color: selected
              ? const Color(0xff007aff)
              : fan
              ? Theme.of(context).colorScheme.surface.withValues(alpha: .92)
              : Colors.transparent,
          borderRadius: BorderRadius.circular(fan ? 9 : 6),
          child: InkWell(
            key: ValueKey('downloads-entry-${entry.id}'),
            borderRadius: BorderRadius.circular(fan ? 9 : 6),
            onTap: _openingId == null ? () => _open(entry) : null,
            onHover: (hovered) {
              if (hovered) setState(() => _selectedId = entry.id);
            },
            child: Padding(
              padding: EdgeInsets.symmetric(
                horizontal: list || fan ? 9 : 4,
                vertical: 4,
              ),
              child: list || fan
                  ? Row(
                      children: [
                        icon,
                        const SizedBox(width: 9),
                        Expanded(child: label),
                      ],
                    )
                  : Column(
                      children: [
                        icon,
                        const SizedBox(height: 5),
                        Expanded(child: label),
                      ],
                    ),
            ),
          ),
        ),
      ),
    );
  }

  Widget _contents(
    List<DockFolderEntry> items,
    MacDownloadsView view,
    double width,
  ) {
    if (widget.service.loading && items.isEmpty) {
      return const Center(
        child: SizedBox.square(
          dimension: 22,
          child: CircularProgressIndicator(strokeWidth: 2),
        ),
      );
    }
    if (items.isEmpty) {
      return Center(
        child: Text(
          widget.service.error == null ? '下载目录是空的' : '暂时无法显示下载项目',
          style: const TextStyle(fontSize: 13),
        ),
      );
    }
    if (view == MacDownloadsView.fan) {
      _columns = 1;
      _rowSpacing = 0;
      return _fanContents(items, width);
    }
    if (view == MacDownloadsView.list) {
      _columns = 1;
      _rowSpacing = 0;
      _rowExtent = math.max(
        38,
        MediaQuery.textScalerOf(context).scale(12) * 1.5 + 12,
      );
      return ListView.builder(
        key: const ValueKey('downloads-list'),
        controller: _scroll,
        padding: const EdgeInsets.symmetric(horizontal: 8),
        itemCount: items.length,
        itemExtent: _rowExtent,
        itemBuilder: (_, index) => _tile(items[index], list: true),
      );
    }
    _columns = (width / 108).floor().clamp(1, 5);
    _rowSpacing = 5;
    _rowExtent = 70 + MediaQuery.textScalerOf(context).scale(12) * 2.4;
    return GridView.builder(
      key: const ValueKey('downloads-grid'),
      controller: _scroll,
      padding: const EdgeInsets.all(8),
      gridDelegate: SliverGridDelegateWithFixedCrossAxisCount(
        crossAxisCount: _columns,
        mainAxisExtent: _rowExtent,
        crossAxisSpacing: 5,
        mainAxisSpacing: _rowSpacing,
      ),
      itemCount: items.length,
      itemBuilder: (_, index) => _tile(items[index]),
    );
  }

  Widget _fanContents(List<DockFolderEntry> items, double width) {
    final requestedAnchor = widget.fanAnchorX;
    final anchor =
        (requestedAnchor != null && requestedAnchor.isFinite
                ? requestedAnchor
                : width / 2)
            .clamp(44.0, width - 44);
    final labelsOnLeft = anchor >= width / 2;
    final direction = labelsOnLeft ? -1.0 : 1.0;
    return AnimatedBuilder(
      animation: _fanAnimation,
      builder: (context, _) => Stack(
        key: const ValueKey('downloads-fan'),
        clipBehavior: Clip.none,
        children: [
          // Draw the lowest/nearest icon last. Only the bounded visible items
          // and the explicit folder terminus participate in painting/hit tests.
          for (var index = items.length; index >= 0; index--)
            _fanPosition(
              index: index,
              width: width,
              anchor: anchor,
              direction: direction,
              labelsOnLeft: labelsOnLeft,
              entry: index == items.length ? null : items[index],
              hidden:
                  widget.service.overflow ||
                  widget.service.entries.length > items.length,
            ),
        ],
      ),
    );
  }

  Widget _fanPosition({
    required int index,
    required double width,
    required double anchor,
    required double direction,
    required bool labelsOnLeft,
    required DockFolderEntry? entry,
    required bool hidden,
  }) {
    final delay = math.min(.28, index * .035);
    final raw = _reducedMotion
        ? 1.0
        : ((_fanAnimation.value - delay) / (1 - delay)).clamp(0.0, 1.0);
    final progress = Curves.easeOutCubic.transform(raw);
    final centre = (anchor + direction * index * index * .75).clamp(
      44.0,
      width - 44,
    );
    final rowWidth = math.min(
      320.0,
      labelsOnLeft ? centre + 20 : width - centre + 20,
    );
    final left = labelsOnLeft ? centre + 32 - rowWidth : centre - 32;
    final originLeft = labelsOnLeft ? anchor + 32 - rowWidth : anchor - 32;
    final requestedY = widget.fanAnchorYBelowPanel;
    final belowPanel = requestedY != null && requestedY.isFinite
        ? requestedY
        : 0.0;
    // The 64px icon is centred in its row. A negative bottom puts that
    // centre exactly at the native Dock centre, below the expanded panel.
    final originBottom = -(belowPanel + _fanTileHeight / 2);
    final requestedSize = widget.fanOriginIconSize;
    final originSize =
        requestedSize != null && requestedSize.isFinite && requestedSize > 0
        ? requestedSize
        : widget.preferences.dockSize;
    final originScale = (originSize / _fanTileHeight).clamp(.25, 2.0);
    return Positioned(
      key: ValueKey('downloads-fan-position-${entry?.id ?? 'folder'}'),
      left: originLeft + (left - originLeft) * progress,
      bottom: originBottom + (8 + index * _fanStep - originBottom) * progress,
      width: rowWidth,
      height: _fanTileHeight,
      child: IgnorePointer(
        ignoring: _closing || _openingId != null || progress < .7,
        child: _fanTile(
          entry,
          labelsOnLeft: labelsOnLeft,
          angle: direction * index * .033 * progress,
          scale: originScale + (1 - originScale) * progress,
          labelOpacity: progress * progress,
          hidden: hidden,
        ),
      ),
    );
  }

  Widget _fanTile(
    DockFolderEntry? entry, {
    required bool labelsOnLeft,
    required double angle,
    required double scale,
    required double labelOpacity,
    required bool hidden,
  }) {
    final id = entry?.id ?? _fanFolderId;
    final selected = _selectedId == id;
    final label = entry?.name ?? (hidden ? '在文件管理器中显示更多' : '在文件管理器中打开');
    final caption = Expanded(
      child: Opacity(
        opacity: labelOpacity,
        child: Align(
          alignment: labelsOnLeft
              ? Alignment.centerRight
              : Alignment.centerLeft,
          child: DecoratedBox(
            key: ValueKey('downloads-fan-caption-$id'),
            decoration: BoxDecoration(
              color: selected
                  ? const Color(0xff007aff)
                  : const Color(0xee29292d),
              borderRadius: BorderRadius.circular(12),
              boxShadow: const [
                BoxShadow(
                  color: Color(0x50000000),
                  blurRadius: 4,
                  offset: Offset(0, 2),
                ),
              ],
            ),
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 4),
              child: Text(
                label,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: const TextStyle(fontSize: 12, color: Colors.white),
              ),
            ),
          ),
        ),
      ),
    );
    final icon = SizedBox.square(
      dimension: _fanTileHeight,
      child: Transform.scale(
        scale: scale,
        child: Transform.rotate(
          angle: angle,
          child: entry != null
              ? _entryIcon(entry, _fanTileHeight)
              : Center(
                  child: Container(
                    width: 48,
                    height: 48,
                    decoration: const BoxDecoration(
                      shape: BoxShape.circle,
                      color: Color(0xe62e2e32),
                      boxShadow: [
                        BoxShadow(
                          color: Color(0x50000000),
                          blurRadius: 5,
                          offset: Offset(0, 2),
                        ),
                      ],
                    ),
                    child: const Icon(
                      Icons.arrow_forward,
                      size: 27,
                      color: Colors.white,
                    ),
                  ),
                ),
        ),
      ),
    );
    return Semantics(
      label: label,
      selected: selected,
      button: true,
      child: Tooltip(
        message: label,
        child: MouseRegion(
          cursor: SystemMouseCursors.click,
          onEnter: (_) {
            if (!_closing) setState(() => _selectedId = id);
          },
          child: GestureDetector(
            key: ValueKey(
              entry == null
                  ? 'downloads-fan-more'
                  : 'downloads-entry-${entry.id}',
            ),
            behavior: HitTestBehavior.translucent,
            onTap: _closing || _openingId != null
                ? null
                : () => unawaited(_open(entry)),
            child: Row(
              key: entry == null
                  ? const ValueKey('downloads-open-folder')
                  : null,
              children: labelsOnLeft
                  ? [caption, const SizedBox(width: 9), icon]
                  : [icon, const SizedBox(width: 9), caption],
            ),
          ),
        ),
      ),
    );
  }

  Widget _header(bool fan, double width) {
    final row = Row(
      children: [
        const SizedBox(width: 12),
        Expanded(
          child: Text(
            '下载',
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(
              fontSize: 13,
              fontWeight: FontWeight.w600,
              color: fan ? Colors.white : null,
            ),
          ),
        ),
        _settings(),
        IconButton(
          key: const ValueKey('downloads-stack-close'),
          tooltip: '收起下载叠放',
          padding: const EdgeInsets.all(5),
          constraints: const BoxConstraints.tightFor(width: 30, height: 30),
          icon: Icon(Icons.close, size: 16, color: fan ? Colors.white : null),
          onPressed: _closing ? null : () => unawaited(_requestClose()),
        ),
        const SizedBox(width: 5),
      ],
    );
    if (!fan) return SizedBox(height: 36, child: row);
    final requested = widget.fanAnchorX;
    final anchor =
        (requested != null && requested.isFinite ? requested : width / 2).clamp(
          82.0,
          width - 82,
        );
    return SizedBox(
      height: 36,
      child: Align(
        alignment: Alignment(
          ((anchor - width / 2) / ((width - 164) / 2)).clamp(-1.0, 1.0),
          0,
        ),
        child: Material(
          key: const ValueKey('downloads-fan-toolbar'),
          color: const Color(0xd929292d),
          borderRadius: BorderRadius.circular(16),
          child: SizedBox(
            width: 164,
            height: 32,
            child: IconTheme(
              data: const IconThemeData(color: Colors.white),
              child: row,
            ),
          ),
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: widget.service,
    builder: (context, _) => LayoutBuilder(
      builder: (context, bounds) {
        final width = math.min(
          540.0,
          bounds.hasBoundedWidth ? bounds.maxWidth : 540.0,
        );
        final maximumHeight = bounds.hasBoundedHeight
            ? bounds.maxHeight
            : MediaQuery.sizeOf(context).height - 80;
        if (width < 100 || maximumHeight < 100) return const SizedBox.shrink();
        final items = widget.service.sortedEntries(
          widget.preferences.downloadsSort,
        );
        final error = _operationError ?? widget.service.error;
        final errorHeight = error == null ? 0.0 : 40.0;
        final available = Size(width, maximumHeight - errorHeight);
        final view = resolveDownloadsView(
          requested: widget.preferences.downloadsView,
          dockPosition: widget.preferences.dockPosition,
          available: available,
          entryCount: items.length,
        );
        final fan = view == MacDownloadsView.fan;
        _setFanActive(fan);
        _visibleItems = fan
            ? items
                  .take(math.min(_fanMaximumFiles, _fanSlots(available) - 1))
                  .toList()
            : items;
        final desiredHeight = fan
            ? _fanChrome +
                  _fanTileHeight +
                  _fanPadding +
                  _visibleItems.length * _fanStep
            : view == MacDownloadsView.list
            ? 84 + math.min(10, math.max(1, items.length)) * 38.0
            : 86 +
                  math.max(
                        1,
                        (items.length / (width / 108).floor().clamp(1, 5))
                            .ceil(),
                      ) *
                      104.0;
        final dark = Theme.of(context).brightness == Brightness.dark;
        return Focus(
          focusNode: _keyboard,
          autofocus: true,
          onKeyEvent: (_, event) => _key(event),
          child: SizedBox(
            key: const ValueKey('mac-downloads-stack-panel'),
            width: width,
            height: math.min(
              maximumHeight,
              math.max(170.0, desiredHeight + (error == null ? 0 : 40)),
            ),
            child: Material(
              color: fan
                  ? Colors.transparent
                  : (dark ? const Color(0xff262629) : const Color(0xfff3f3f5))
                        .withValues(alpha: .96),
              borderRadius: BorderRadius.circular(11),
              clipBehavior: fan ? Clip.none : Clip.antiAlias,
              child: Column(
                children: [
                  _header(fan, width),
                  if (error != null)
                    Padding(
                      padding: const EdgeInsets.symmetric(
                        horizontal: 12,
                        vertical: 4,
                      ),
                      child: Text(
                        error,
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          fontSize: 11,
                          color: fan
                              ? Colors.white
                              : Theme.of(context).colorScheme.error,
                        ),
                      ),
                    ),
                  Expanded(child: _contents(_visibleItems, view, width)),
                  if (widget.service.overflow && !fan)
                    const Padding(
                      padding: EdgeInsets.symmetric(horizontal: 12),
                      child: Text(
                        '项目较多，打开文件夹可查看全部',
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 11),
                      ),
                    ),
                  if (!fan)
                    SizedBox(
                      height: 36,
                      child: TextButton.icon(
                        key: const ValueKey('downloads-open-folder'),
                        onPressed: _openingId == null && !_closing
                            ? () => _open(null)
                            : null,
                        icon: const Icon(Icons.folder_open_outlined, size: 16),
                        label: const Text(
                          '在文件管理器中打开',
                          style: TextStyle(fontSize: 12),
                        ),
                        style: fan
                            ? TextButton.styleFrom(
                                foregroundColor: Colors.white,
                              )
                            : null,
                      ),
                    ),
                ],
              ),
            ),
          ),
        );
      },
    ),
  );

  @override
  void dispose() {
    _keyboard.dispose();
    _fanAnimation.dispose();
    _scroll.dispose();
    super.dispose();
  }
}

Widget _entryIcon(
  DockFolderEntry entry,
  double size, {
  double? rasterSize,
}) => AppIcon(
  app: DesktopApp(
    id: entry.id,
    name: entry.name,
    desktopFile: '',
    iconName: entry.iconName,
    iconPath: entry.iconPath,
  ),
  size: size,
  rasterSize: rasterSize,
  fallbackIcon: entry.isDirectory
      ? Icons.folder_rounded
      : Icons.insert_drive_file_rounded,
  fallbackAssetPath:
      'assets/icons/${entry.isDirectory ? 'generic-folder' : 'generic-document'}.svg',
);
