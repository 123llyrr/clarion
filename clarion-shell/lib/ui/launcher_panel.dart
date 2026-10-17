// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../models.dart';
import '../state/shell_controller.dart';
import '../theme/shell_theme.dart';
import 'app_icon.dart';
import 'shell_view.dart';

class LauncherPanel extends StatefulWidget {
  const LauncherPanel({super.key, required this.controller});
  final ShellController controller;

  @override
  State<LauncherPanel> createState() => _LauncherPanelState();
}

class _LauncherPanelState extends State<LauncherPanel> {
  final _query = TextEditingController();
  final _searchFocus = FocusNode();
  final _pages = PageController();
  final _dots = ScrollController();
  Timer? _wheelReset;
  Timer? _wheelGate;
  double _wheelDelta = 0;
  String? _selectedId;
  List<String> _previousIds = const [];
  List<DesktopApp>? _idCatalogue;
  List<String> _catalogueIds = const [];
  int _columns = 1;
  int _capacity = 1;
  int _page = 0;
  int _pageCount = 1;
  int _reflowRevision = 0;
  bool _reflowing = false;
  bool _settling = false;
  int _animationRevision = 0;
  ({int page, String selectedId})? _programmatic;

  List<DesktopApp>? _filteredCatalogue;
  List<DesktopApp> _filteredApps = const [];
  String? _filteredQuery;
  late Object _controllerProjection;

  List<DesktopApp> get _apps {
    final catalogue = widget.controller.apps;
    final query = _query.text.trim();
    if (!identical(catalogue, _filteredCatalogue) || query != _filteredQuery) {
      _filteredCatalogue = catalogue;
      _filteredQuery = query;
      _filteredApps = query.isEmpty
          ? catalogue
          : catalogue.where((app) => app.matches(query)).toList();
    }
    return _filteredApps;
  }

  // Window, clock, tray and slider updates do not change the launcher. Compare
  // the values it displays instead of rebuilding and rescanning every tile.
  Object _projectController() => (
    apps: widget.controller.apps,
    error: widget.controller.error,
    launching: widget.controller.busy.contains('launch'),
    connected: widget.controller.connected,
    reduced: widget.controller.reduceMotion,
    style: widget.controller.shellStyle,
  );

  bool get _reduced =>
      widget.controller.reduceMotion || MediaQuery.disableAnimationsOf(context);

  @override
  void initState() {
    super.initState();
    _searchFocus.onKeyEvent = _key;
    _controllerProjection = _projectController();
    widget.controller.addListener(_controllerChanged);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _searchFocus.requestFocus();
    });
  }

  void _controllerChanged() {
    if (!mounted) return;
    final projection = _projectController();
    if (_controllerProjection == projection) return;
    setState(() => _controllerProjection = projection);
  }

  @override
  void didUpdateWidget(covariant LauncherPanel oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.controller != widget.controller) {
      oldWidget.controller.removeListener(_controllerChanged);
      widget.controller.addListener(_controllerChanged);
      _controllerProjection = _projectController();
    }
  }

  @override
  void dispose() {
    widget.controller.removeListener(_controllerChanged);
    _wheelReset?.cancel();
    _wheelGate?.cancel();
    _query.dispose();
    _searchFocus.dispose();
    _pages.dispose();
    _dots.dispose();
    super.dispose();
  }

  void _reconcile(List<DesktopApp> apps, int columns, int capacity) {
    if (!identical(apps, _idCatalogue)) {
      _idCatalogue = apps;
      _catalogueIds = apps.map((app) => app.id).toList();
    }
    final ids = _catalogueIds;
    _columns = columns;
    if (_capacity == capacity && listEquals(ids, _previousIds)) return;
    _capacity = capacity;
    _animationRevision++;
    _programmatic = null;
    _previousIds = ids;
    _pageCount = math.max(1, (apps.length / capacity).ceil());
    final index = apps.indexWhere((app) => app.id == _selectedId);
    if (index < 0) _selectedId = apps.isEmpty ? null : apps.first.id;
    _page = index < 0 ? 0 : index ~/ capacity;
    final revision = ++_reflowRevision;
    _reflowing = true;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || revision != _reflowRevision) return;
      if (_pages.hasClients) _pages.jumpToPage(_page);
      _reflowing = false;
      // The grid is measured after its sibling footer has been built. Publish
      // its capacity once so count, dots and button availability stay in sync.
      setState(() {});
      _revealDot();
    });
  }

  void _revealDot() {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || !_dots.hasClients) return;
      final position = _dots.position;
      _dots.jumpTo(
        ((_page + .5) * 48 - position.viewportDimension / 2).clamp(
          0.0,
          position.maxScrollExtent,
        ),
      );
    });
  }

  void _pageChanged(int page) {
    if (_reflowing || page < 0 || page >= _pageCount) return;
    final target = _programmatic;
    if (target != null && page != target.page) return;
    final apps = _apps;
    final selected = apps.indexWhere((app) => app.id == _selectedId);
    setState(() {
      _page = page;
      if (target != null) _selectedId = target.selectedId;
      if (target == null &&
          (selected < page * _capacity || selected >= (page + 1) * _capacity)) {
        final first = page * _capacity;
        _selectedId = first < apps.length ? apps[first].id : null;
      }
    });
    _revealDot();
  }

  void _goToPage(int page, {String? selectedId}) {
    final target = page.clamp(0, _pageCount - 1);
    final apps = _apps;
    final first = target * _capacity;
    if (first >= apps.length) return;
    final id = selectedId ?? apps[first].id;
    if (target == _page && id == _selectedId) return;
    _reflowRevision++;
    final revision = ++_animationRevision;
    _reflowing = false;
    setState(() {
      _page = target;
      _selectedId = id;
    });
    if (_pages.hasClients) {
      _programmatic = (page: target, selectedId: id);
      if ((_pages.page! - target).abs() < .001) {
        // A reverse request can arrive before an animation's first tick. Its
        // current pixels are already correct, but the old activity must stop.
        _pages.jumpToPage(target);
        _programmatic = null;
        _revealDot();
        return;
      } else if (_reduced) {
        _pages.jumpToPage(target);
        _programmatic = null;
      } else {
        unawaited(
          _pages
              .animateToPage(
                target,
                duration: ShellMotion.change,
                curve: ShellMotion.curve,
              )
              .then((_) {
                if (!mounted || revision != _animationRevision) return;
                _programmatic = null;
                if (_pages.hasClients) _pageChanged(_pages.page!.round());
              }),
        );
      }
    }
    _revealDot();
  }

  void _searchChanged() {
    final apps = _apps;
    setState(() {
      _page = 0;
      // A search starts on its first page. Preserve an ID only if it is still
      // visible there, rather than silently selecting an off-screen result.
      final index = apps.indexWhere((app) => app.id == _selectedId);
      if (index < 0 || index >= _capacity) {
        _selectedId = apps.isEmpty ? null : apps.first.id;
      }
      _previousIds = const [];
    });
    _wheelDelta = 0;
    _animationRevision++;
    _programmatic = null;
    _wheelReset?.cancel();
    _wheelGate?.cancel();
    _wheelGate = null;
    if (_pages.hasClients) _pages.jumpToPage(0);
  }

  void _launch([DesktopApp? app]) {
    if (widget.controller.panel != ShellPanel.launcher ||
        widget.controller.busy.contains('launch')) {
      return;
    }
    final apps = _apps;
    final index = apps.indexWhere((entry) => entry.id == _selectedId);
    final selected = app ?? (index < 0 ? null : apps[index]);
    if (selected != null) unawaited(widget.controller.launchApp(selected));
  }

  KeyEventResult _key(FocusNode _, KeyEvent event) {
    if (widget.controller.panel != ShellPanel.launcher) {
      return KeyEventResult.ignored;
    }
    if (event is! KeyDownEvent && event is! KeyRepeatEvent) {
      return KeyEventResult.ignored;
    }
    final key = event.logicalKey;
    if (key == LogicalKeyboardKey.escape) {
      widget.controller.closePanel();
      return KeyEventResult.handled;
    }
    if (key == LogicalKeyboardKey.enter ||
        key == LogicalKeyboardKey.numpadEnter) {
      _launch();
      return KeyEventResult.handled;
    }
    if (key == LogicalKeyboardKey.pageDown ||
        key == LogicalKeyboardKey.pageUp) {
      _goToPage(_page + (key == LogicalKeyboardKey.pageDown ? 1 : -1));
      return KeyEventResult.handled;
    }
    final apps = _apps;
    if (apps.isEmpty) return KeyEventResult.ignored;
    final editing = _searchFocus.hasFocus && _query.text.isNotEmpty;
    final delta = switch (key) {
      LogicalKeyboardKey.arrowDown => _columns,
      LogicalKeyboardKey.arrowUp => -_columns,
      LogicalKeyboardKey.arrowRight when !editing => 1,
      LogicalKeyboardKey.arrowLeft when !editing => -1,
      _ => 0,
    };
    if (delta == 0) return KeyEventResult.ignored;
    final current = apps.indexWhere((app) => app.id == _selectedId);
    final index = (math.max(0, current) + delta).clamp(0, apps.length - 1);
    _goToPage(index ~/ _capacity, selectedId: apps[index].id);
    return KeyEventResult.handled;
  }

  void _pointerSignal(PointerSignalEvent event) {
    if (event is! PointerScrollEvent || _pageCount <= 1) return;
    final delta = event.scrollDelta.dx.abs() > event.scrollDelta.dy.abs()
        ? event.scrollDelta.dx
        : event.scrollDelta.dy;
    if (delta == 0) return;
    GestureBinding.instance.pointerSignalResolver.register(event, (_) {
      if (_wheelGate?.isActive ?? false) return;
      _wheelDelta += delta;
      _wheelReset?.cancel();
      _wheelReset = Timer(
        const Duration(milliseconds: 180),
        () => _wheelDelta = 0,
      );
      if (_wheelDelta.abs() < 60) return;
      _goToPage(_page + (_wheelDelta > 0 ? 1 : -1));
      _wheelDelta = 0;
      _wheelGate = Timer(const Duration(milliseconds: 250), () {});
    });
  }

  @override
  Widget build(BuildContext context) {
    final controller = widget.controller;
    final apps = _apps;
    final colors = Theme.of(context).colorScheme;
    return Material(
      type: MaterialType.transparency,
      child: Focus(
        onKeyEvent: _key,
        child: LayoutBuilder(
          builder: (context, constraints) {
            final compact =
                constraints.maxHeight < 420 || constraints.maxWidth < 480;
            final padding = compact ? 12.0 : 24.0;
            final content = Padding(
              padding: EdgeInsets.all(padding),
              child: Column(
                children: [
                  _header(
                    context,
                    wide: constraints.maxWidth - padding * 2 >= 800,
                    width: constraints.maxWidth - padding * 2,
                  ),
                  SizedBox(height: compact ? 8 : 24),
                  Expanded(
                    child: LayoutBuilder(
                      builder: (context, grid) {
                        final columns = math.max(
                          1,
                          math.min(7, (grid.maxWidth / 142).floor()),
                        );
                        final rows = math.max(
                          1,
                          math.min(5, (grid.maxHeight / 148).floor()),
                        );
                        final extent = math.max(
                          0.0,
                          (grid.maxHeight - (rows - 1) * 12) / rows,
                        );
                        _reconcile(apps, columns, columns * rows);
                        if (apps.isEmpty) return _empty(context);
                        return Listener(
                          onPointerSignal: _pointerSignal,
                          child: ScrollConfiguration(
                            behavior: ScrollConfiguration.of(context).copyWith(
                              dragDevices: {
                                ...ScrollConfiguration.of(context).dragDevices,
                                PointerDeviceKind.mouse,
                                PointerDeviceKind.trackpad,
                              },
                            ),
                            child: NotificationListener<ScrollNotification>(
                              onNotification: (notification) {
                                if (notification.depth != 0) return false;
                                if (notification is ScrollStartNotification &&
                                    notification.dragDetails != null) {
                                  _animationRevision++;
                                  _programmatic = null;
                                }
                                if (notification is ScrollEndNotification &&
                                    !_settling &&
                                    _reduced &&
                                    _pages.hasClients) {
                                  final nearest = _pages.page!.round().clamp(
                                    0,
                                    _pageCount - 1,
                                  );
                                  if ((_pages.page! - nearest).abs() > .001) {
                                    _settling = true;
                                    try {
                                      _pages.jumpToPage(nearest);
                                    } finally {
                                      _settling = false;
                                    }
                                  }
                                }
                                return false;
                              },
                              child: PageView.builder(
                                key: const ValueKey('launcher-pages'),
                                controller: _pages,
                                // Prepare the adjacent page's icons before it
                                // enters the viewport, without retaining every
                                // visited page and its decoded images.
                                allowImplicitScrolling: true,
                                pageSnapping: !_reduced,
                                physics: _reduced
                                    ? const _ReducedPagePhysics()
                                    : null,
                                onPageChanged: _pageChanged,
                                itemCount: _pageCount,
                                itemBuilder: (context, page) {
                                  final first = page * _capacity;
                                  final count = math.min(
                                    _capacity,
                                    apps.length - first,
                                  );
                                  return GestureDetector(
                                    behavior: HitTestBehavior.opaque,
                                    onTap: controller.closePanel,
                                    child: GridView.builder(
                                      key: ValueKey('launcher-page-$page'),
                                      physics:
                                          const NeverScrollableScrollPhysics(),
                                      padding: EdgeInsets.zero,
                                      gridDelegate:
                                          SliverGridDelegateWithFixedCrossAxisCount(
                                            crossAxisCount: columns,
                                            mainAxisExtent: extent,
                                            crossAxisSpacing: 12,
                                            mainAxisSpacing: 12,
                                          ),
                                      itemCount: count,
                                      itemBuilder: (context, offset) =>
                                          _appTile(
                                            context,
                                            apps[first + offset],
                                          ),
                                    ),
                                  );
                                },
                              ),
                            ),
                          ),
                        );
                      },
                    ),
                  ),
                  if (controller.error != null)
                    ConstrainedBox(
                      constraints: const BoxConstraints(maxHeight: 76),
                      child: SingleChildScrollView(
                        child: ShellStatusMessage(
                          text: controller.error!,
                          error: true,
                        ),
                      ),
                    ),
                  const SizedBox(height: 8),
                  _pagination(context),
                  if (constraints.maxHeight >= 500) ...[
                    const SizedBox(height: 8),
                    Text(
                      controller.connected
                          ? '滑动或滚轮翻页 · 方向键选择 · Enter 打开 · Esc 返回'
                          : '应用仍可打开 · Esc 返回',
                      style: Theme.of(context).textTheme.bodySmall?.copyWith(
                        color: colors.onSurfaceVariant,
                      ),
                      textAlign: TextAlign.center,
                    ),
                  ],
                ],
              ),
            );
            // Extremely short outputs keep the search and page controls reachable.
            return constraints.maxHeight < 220
                ? SingleChildScrollView(
                    child: SizedBox(height: 280, child: content),
                  )
                : content;
          },
        ),
      ),
    );
  }

  Widget _header(
    BuildContext context, {
    required bool wide,
    required double width,
  }) {
    final narrow = width < 200;
    final close = IconButton(
      key: const ValueKey('launcher-close'),
      tooltip: '关闭',
      onPressed: widget.controller.closePanel,
      icon: const Icon(Icons.close_rounded),
    );
    final clear = IconButton(
      tooltip: '清除搜索',
      onPressed: () {
        _query.clear();
        _searchChanged();
        _searchFocus.requestFocus();
      },
      icon: const Icon(Icons.clear_rounded),
    );
    final search = TextField(
      key: const ValueKey('launcher-search'),
      controller: _query,
      focusNode: _searchFocus,
      autofocus: true,
      textInputAction: TextInputAction.search,
      onSubmitted: (_) => _launch(),
      onChanged: (_) => _searchChanged(),
      decoration: InputDecoration(
        hintText: '搜索应用',
        prefixIcon: narrow ? null : const Icon(Icons.search_rounded),
        suffixIcon: narrow
            ? close
            : _query.text.isEmpty
            ? null
            : clear,
      ),
    );
    if (narrow) return search;
    if (!wide) {
      return Row(
        children: [
          Expanded(child: search),
          const SizedBox(width: 8),
          close,
        ],
      );
    }
    return SizedBox(
      height: 56,
      child: Stack(
        alignment: Alignment.center,
        children: [
          Align(
            alignment: Alignment.centerLeft,
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                const ChromeLauncherIcon(isOpen: true, size: 24),
                const SizedBox(width: 10),
                Text('应用', style: Theme.of(context).textTheme.headlineSmall),
              ],
            ),
          ),
          ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 480),
            child: search,
          ),
          Align(
            alignment: Alignment.centerRight,
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                IconButton(
                  tooltip: '工作区概览',
                  onPressed: widget.controller.toggleOverview,
                  icon: const Icon(Icons.view_carousel_outlined),
                ),
                IconButton(
                  tooltip: 'Clarion 配置中心',
                  onPressed: widget.controller.openSettings,
                  icon: const Icon(Icons.settings_outlined),
                ),
                close,
              ],
            ),
          ),
        ],
      ),
    );
  }

  Widget _empty(BuildContext context) => Center(
    child: SingleChildScrollView(
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(
              Icons.search_off_rounded,
              size: 48,
              color: Theme.of(context).colorScheme.onSurfaceVariant,
            ),
            const SizedBox(height: 12),
            Text(
              _query.text.isEmpty ? '尚未找到已安装的应用' : '没有匹配的应用',
              style: Theme.of(context).textTheme.titleMedium,
            ),
            const SizedBox(height: 8),
            Text(
              _query.text.isEmpty ? '应用载入后会显示在这里' : '试试名称、用途或关键词',
              textAlign: TextAlign.center,
            ),
          ],
        ),
      ),
    ),
  );

  Widget _appTile(BuildContext context, DesktopApp app) {
    final colors = Theme.of(context).colorScheme;
    final selected = _selectedId == app.id;
    final busy = widget.controller.busy.contains('launch');
    return Center(
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 136, maxHeight: 160),
        child: Tooltip(
          message: app.description.isEmpty
              ? app.name
              : '${app.name}\n${app.description}',
          child: Semantics(
            selected: selected,
            enabled: !busy,
            label: app.name,
            button: true,
            child: Material(
              animationDuration: ShellMotion.duration(_reduced),
              color: selected
                  ? colors.secondaryContainer.withValues(alpha: .72)
                  : Colors.transparent,
              borderRadius: BorderRadius.circular(24),
              child: InkWell(
                key: ValueKey('app-${app.id}'),
                borderRadius: BorderRadius.circular(24),
                onFocusChange: (focused) {
                  if (focused) {
                    final index = _apps.indexWhere(
                      (entry) => entry.id == app.id,
                    );
                    if (index >= 0) {
                      _goToPage(index ~/ _capacity, selectedId: app.id);
                    }
                  }
                },
                // Keep the tile's tap recognizer while busy so the background's
                // blank-space dismissal cannot treat an app tile as empty space.
                onTap: busy ? () {} : () => _launch(app),
                onSecondaryTapUp:
                    widget.controller.shellStyle == ShellStyle.macos
                    ? (_) => widget.controller.openDockAppMenu(app.id)
                    : null,
                child: LayoutBuilder(
                  builder: (context, tile) {
                    final iconSize = math.min(
                      72.0,
                      math.max(16.0, tile.maxHeight - 64),
                    );
                    return Padding(
                      padding: const EdgeInsets.all(8),
                      child: Column(
                        mainAxisAlignment: MainAxisAlignment.center,
                        children: [
                          ExcludeSemantics(
                            child: AppIcon(app: app, size: iconSize),
                          ),
                          const SizedBox(height: 8),
                          Flexible(
                            child: Text(
                              app.name,
                              maxLines: 2,
                              overflow: TextOverflow.ellipsis,
                              textAlign: TextAlign.center,
                              style: Theme.of(context).textTheme.labelLarge
                                  ?.copyWith(
                                    color: selected
                                        ? colors.onSecondaryContainer
                                        : colors.onSurface,
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
          ),
        ),
      ),
    );
  }

  Widget _pagination(BuildContext context) => SizedBox(
    height: 48,
    child: LayoutBuilder(
      builder: (context, constraints) {
        final previous = IconButton(
          key: const ValueKey('launcher-prev'),
          tooltip: '上一页',
          onPressed: _page > 0 ? () => _goToPage(_page - 1) : null,
          icon: const Icon(Icons.chevron_left_rounded),
        );
        final next = IconButton(
          key: const ValueKey('launcher-next'),
          tooltip: '下一页',
          onPressed: _page + 1 < _pageCount ? () => _goToPage(_page + 1) : null,
          icon: const Icon(Icons.chevron_right_rounded),
        );
        final label = Text(
          '${_page + 1} / $_pageCount',
          key: const ValueKey('launcher-page-label'),
          style: Theme.of(context).textTheme.labelMedium,
        );
        if (constraints.maxWidth < 320) {
          return Row(
            children: [
              previous,
              Expanded(
                child: Center(
                  child: FittedBox(fit: BoxFit.scaleDown, child: label),
                ),
              ),
              next,
            ],
          );
        }
        return Row(
          children: [
            previous,
            Expanded(
              child: LayoutBuilder(
                builder: (context, dots) => Center(
                  child: SizedBox(
                    width: math.min(dots.maxWidth, _pageCount * 48.0),
                    child: ListView.builder(
                      controller: _dots,
                      scrollDirection: Axis.horizontal,
                      itemExtent: 48,
                      itemCount: _pageCount,
                      itemBuilder: (context, page) => Tooltip(
                        message: '第 ${page + 1} 页',
                        child: Semantics(
                          selected: _page == page,
                          button: true,
                          label: '第 ${page + 1} 页，共 $_pageCount 页',
                          child: InkResponse(
                            key: ValueKey('launcher-dot-$page'),
                            onTap: () => _goToPage(page),
                            radius: 24,
                            child: Center(
                              child: AnimatedContainer(
                                duration: ShellMotion.duration(_reduced),
                                width: _page == page ? 20 : 8,
                                height: 8,
                                decoration: BoxDecoration(
                                  color: _page == page
                                      ? Theme.of(context).colorScheme.primary
                                      : Theme.of(
                                          context,
                                        ).colorScheme.outlineVariant,
                                  borderRadius: BorderRadius.circular(8),
                                ),
                              ),
                            ),
                          ),
                        ),
                      ),
                    ),
                  ),
                ),
              ),
            ),
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 8),
              child: label,
            ),
            next,
          ],
        );
      },
    ),
  );
}

/// Direct manipulation remains available; releasing a reduced-motion swipe
/// selects its nearest page immediately instead of running a spring.
class _ReducedPagePhysics extends ScrollPhysics {
  const _ReducedPagePhysics({super.parent});

  @override
  _ReducedPagePhysics applyTo(ScrollPhysics? ancestor) =>
      _ReducedPagePhysics(parent: buildParent(ancestor));

  @override
  Simulation? createBallisticSimulation(
    ScrollMetrics position,
    double velocity,
  ) => null;
}
