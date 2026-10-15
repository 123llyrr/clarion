// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:realand_shell/models.dart';
import 'package:realand_shell/state/shell_controller.dart';
import 'package:realand_shell/theme/shell_theme.dart';
import 'package:realand_shell/ui/app_icon.dart';
import 'package:realand_shell/ui/launcher_panel.dart';

class _Controller extends ShellController {
  _Controller({int count = 80, bool reduced = true})
    : super(demo: true, tray: false, notifications: false) {
    entries = List.generate(
      count,
      (index) => DesktopApp(
        id: 'id-$index',
        name: '应用 ${index.toString().padLeft(3, '0')}',
        desktopFile: '',
        keywords: index % 7 == 0 ? const ['笔记'] : const [],
      ),
    );
    setReduceMotion(reduced);
    selectPanel(ShellPanel.launcher);
  }
  late List<DesktopApp> entries;
  final launched = <String>[];
  int closes = 0;
  bool fail = false;
  bool launching = false;
  String? failure;
  @override
  List<DesktopApp> get apps => entries;
  @override
  String? get error => failure;
  @override
  Set<String> get busy => launching ? {'launch'} : {};
  @override
  Future<void> launchApp(DesktopApp app) async {
    if (launching) return;
    launched.add(app.id);
    if (fail) {
      failure = '启动失败：应用文件已被移除，请重试';
      notifyListeners();
    } else {
      closePanel();
    }
  }

  @override
  void closePanel() {
    closes++;
    super.closePanel();
  }

  void replace(List<DesktopApp> apps) {
    entries = apps;
    notifyListeners();
  }
}

Future<void> _mount(
  WidgetTester tester,
  _Controller controller,
  Size size,
) async {
  tester.view.devicePixelRatio = 1;
  tester.view.physicalSize = size;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  addTearDown(controller.dispose);
  await tester.pumpWidget(
    MaterialApp(
      theme: buildShellTheme(Brightness.dark),
      home: LauncherPanel(controller: controller),
    ),
  );
  await tester.pumpAndSettle();
}

String _label(WidgetTester tester) => tester
    .widget<Text>(find.byKey(const ValueKey('launcher-page-label')))
    .data!;
PageController _pages(WidgetTester tester) => tester
    .widget<PageView>(find.byKey(const ValueKey('launcher-pages')))
    .controller!;
GridView _grid(WidgetTester tester, int page) =>
    tester.widget<GridView>(find.byKey(ValueKey('launcher-page-$page')));
int _capacity(WidgetTester tester) =>
    _grid(tester, 0).childrenDelegate.estimatedChildCount!;

void main() {
  testWidgets('adjacent icon pages warm up without retaining distant pages', (
    tester,
  ) async {
    final controller = _Controller(count: 250);
    await _mount(tester, controller, const Size(1200, 800));
    expect(
      tester
          .widget<PageView>(find.byKey(const ValueKey('launcher-pages')))
          .allowImplicitScrolling,
      isTrue,
    );
    Finder cachedPage(int page) =>
        find.byKey(ValueKey('launcher-page-$page'), skipOffstage: false);
    expect(cachedPage(1), findsOneWidget);
    expect(cachedPage(3), findsNothing);
    _pages(tester).jumpToPage(3);
    await tester.pumpAndSettle();
    expect(cachedPage(2), findsOneWidget);
    expect(cachedPage(4), findsOneWidget);
    expect(cachedPage(0), findsNothing);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'Launchpad uses bounded pages, large icons and two-line app names',
    (tester) async {
      final controller = _Controller();
      await _mount(tester, controller, const Size(1200, 800));
      final grid = _grid(tester, 0);
      final delegate =
          grid.gridDelegate as SliverGridDelegateWithFixedCrossAxisCount;
      expect(delegate.crossAxisCount, 7);
      final capacity = _capacity(tester);
      expect(capacity, inInclusiveRange(14, 35));
      expect(
        _label(tester),
        '1 / ${(controller.apps.length / capacity).ceil()}',
      );
      expect(tester.widget<AppIcon>(find.byType(AppIcon).first).size, 72);
      final label = find.descendant(
        of: find.byKey(const ValueKey('app-id-0')),
        matching: find.byType(Text),
      );
      expect(tester.widget<Text>(label).maxLines, 2);
      expect(tester.widget<Text>(label).overflow, TextOverflow.ellipsis);
      expect(tester.takeException(), null);
    },
  );

  testWidgets(
    'direction keys select a stable app and Enter launches only once',
    (tester) async {
      final controller = _Controller();
      await _mount(tester, controller, const Size(1200, 800));
      final columns =
          (_grid(tester, 0).gridDelegate
                  as SliverGridDelegateWithFixedCrossAxisCount)
              .crossAxisCount;
      await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
      await tester.sendKeyEvent(LogicalKeyboardKey.arrowRight);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pump();
      expect(controller.launched, ['id-${columns + 1}']);
      expect(controller.panel, ShellPanel.none);
    },
  );

  testWidgets(
    'page controls and dots immediately select pages with reduced motion',
    (tester) async {
      final controller = _Controller();
      await _mount(tester, controller, const Size(1200, 800));
      final last = (controller.apps.length / _capacity(tester)).ceil() - 1;
      await tester.tap(find.byKey(const ValueKey('launcher-next')));
      await tester.pump();
      expect(_pages(tester).page, 1);
      expect(_label(tester), startsWith('2 /'));
      await tester.tap(find.byKey(ValueKey('launcher-dot-$last')));
      await tester.pump();
      expect(_pages(tester).page, last);
      await tester.tap(find.byKey(const ValueKey('launcher-prev')));
      await tester.pump();
      expect(_pages(tester).page, last - 1);
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pump();
      expect(controller.panel, ShellPanel.none);
    },
  );

  testWidgets(
    'wheel bursts advance one page and touch swipes do not activate blank space',
    (tester) async {
      final controller = _Controller();
      await _mount(tester, controller, const Size(1200, 800));
      final viewport = find.byKey(const ValueKey('launcher-pages'));
      Future<void> wheel() => tester.sendEventToBinding(
        PointerScrollEvent(
          position: tester.getCenter(viewport),
          scrollDelta: const Offset(0, 120),
        ),
      );
      await wheel();
      await tester.pump();
      await wheel();
      await tester.pump();
      expect(_pages(tester).page, 1);
      await tester.pump(const Duration(milliseconds: 251));
      await wheel();
      await tester.pump();
      expect(_pages(tester).page, 2);
      await tester.drag(viewport, const Offset(950, 0));
      await tester.pump();
      expect(_pages(tester).page, 1);
      expect(controller.launched, isEmpty);
      expect(controller.closes, 0);
      await tester.pump(const Duration(milliseconds: 251));
    },
  );

  testWidgets(
    'search returns to page zero and preserves a matching visible app ID',
    (tester) async {
      final controller = _Controller();
      await _mount(tester, controller, const Size(1200, 800));
      final capacity = _capacity(tester);
      await tester.sendKeyEvent(LogicalKeyboardKey.pageDown);
      await tester.sendKeyEvent(LogicalKeyboardKey.pageDown);
      await tester.pump();
      expect(_pages(tester).page, 2);
      final selected = capacity * 2;
      expect(selected % 7, 0);
      await tester.enterText(
        find.byKey(const ValueKey('launcher-search')),
        '笔记',
      );
      await tester.pumpAndSettle();
      expect(_pages(tester).page, 0);
      expect(_label(tester), '1 / 1');
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pump();
      expect(controller.launched, ['id-$selected']);
    },
  );

  testWidgets(
    'catalogue reordering and layout changes retain selection by desktop app ID',
    (tester) async {
      final controller = _Controller();
      await _mount(tester, controller, const Size(1200, 800));
      for (var i = 0; i < 5; i++) {
        await tester.sendKeyEvent(LogicalKeyboardKey.arrowRight);
      }
      controller.replace([
        ...controller.entries.skip(10),
        ...controller.entries.take(10),
      ]);
      await tester.pump();
      await tester.pump();
      tester.view.physicalSize = const Size(720, 600);
      await tester.pump();
      await tester.pump();
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pump();
      expect(controller.launched, ['id-5']);
      expect(tester.takeException(), null);
    },
  );

  testWidgets(
    'removed selections and empty searches remain safe and clear search recovers',
    (tester) async {
      final controller = _Controller();
      await _mount(tester, controller, const Size(1200, 800));
      await tester.sendKeyEvent(LogicalKeyboardKey.pageDown);
      controller.replace(controller.entries.take(5).toList());
      await tester.pump();
      await tester.enterText(
        find.byKey(const ValueKey('launcher-search')),
        '完全不存在',
      );
      await tester.pump();
      expect(find.text('没有匹配的应用'), findsOneWidget);
      expect(find.byKey(const ValueKey('launcher-pages')), findsNothing);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      expect(controller.launched, isEmpty);
      await tester.tap(find.byTooltip('清除搜索'));
      await tester.pumpAndSettle();
      expect(_pages(tester).page, 0);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      expect(controller.launched, ['id-0']);
    },
  );

  testWidgets(
    'a failed launch keeps the panel, shows its error and permits retry',
    (tester) async {
      final controller = _Controller(count: 4)..fail = true;
      await _mount(tester, controller, const Size(1200, 800));
      await tester.tap(find.byKey(const ValueKey('app-id-0')));
      await tester.pumpAndSettle();
      expect(controller.panel, ShellPanel.launcher);
      expect(controller.launched, ['id-0']);
      expect(find.textContaining('启动失败'), findsOneWidget);
      controller.fail = false;
      await tester.tap(find.byKey(const ValueKey('app-id-0')));
      await tester.pump();
      expect(controller.launched, ['id-0', 'id-0']);
      expect(controller.panel, ShellPanel.none);
    },
  );

  testWidgets('launch busy state disables Enter and tile clicks', (
    tester,
  ) async {
    final controller = _Controller()..launching = true;
    await _mount(tester, controller, const Size(1200, 800));
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.tap(find.byKey(const ValueKey('app-id-0')));
    await tester.pump();
    expect(controller.launched, isEmpty);
    expect(controller.panel, ShellPanel.launcher);
  });

  testWidgets('a closing launcher ignores stale focus and retained tile taps', (
    tester,
  ) async {
    final controller = _Controller(reduced: false);
    await _mount(tester, controller, const Size(1200, 800));
    controller.closePanel();
    // The previous focus can receive an event before the next frame rebuilds
    // IgnorePointer and FocusScope on the retained exit surface.
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pump();
    await tester.tap(find.byKey(const ValueKey('app-id-0')));
    await tester.pump();
    expect(controller.launched, isEmpty);
    expect(controller.panel, ShellPanel.none);
  });

  testWidgets(
    'grid blank space closes while application taps are claimed by their tiles',
    (tester) async {
      final controller = _Controller(count: 1);
      await _mount(tester, controller, const Size(1200, 800));
      final grid = tester.getRect(
        find.byKey(const ValueKey('launcher-page-0')),
      );
      await tester.tapAt(Offset(grid.right - 20, grid.center.dy));
      await tester.pump();
      expect(controller.panel, ShellPanel.none);
      expect(controller.launched, isEmpty);
    },
  );

  testWidgets(
    'rapid animated reversal and keyboard crossing keep the chosen non-first app',
    (tester) async {
      final controller = _Controller(count: 160, reduced: false);
      await _mount(tester, controller, const Size(1200, 800));
      final capacity = _capacity(tester);
      await tester.tap(find.byKey(const ValueKey('launcher-next')));
      await tester.pump(const Duration(milliseconds: 40));
      await tester.tap(find.byKey(const ValueKey('launcher-prev')));
      await tester.pumpAndSettle();
      expect(_pages(tester).page, 0);
      await tester.tap(find.byKey(const ValueKey('launcher-search')));
      await tester.sendKeyEvent(LogicalKeyboardKey.pageDown);
      await tester.sendKeyEvent(LogicalKeyboardKey.pageDown);
      await tester.sendKeyEvent(LogicalKeyboardKey.arrowRight);
      await tester.sendKeyEvent(LogicalKeyboardKey.arrowRight);
      await tester.pumpAndSettle();
      expect(_pages(tester).page, 2);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pump();
      expect(controller.launched, ['id-${capacity * 2 + 2}']);
    },
  );

  for (final size in [
    const Size(180, 720),
    const Size(240, 180),
    const Size(320, 300),
    const Size(720, 600),
  ]) {
    testWidgets(
      'responsive page grid fits ${size.width} by ${size.height} with enlarged text',
      (tester) async {
        tester.platformDispatcher.textScaleFactorTestValue = 1.4;
        addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
        final controller = _Controller(count: 100);
        await _mount(tester, controller, size);
        expect(find.byKey(const ValueKey('launcher-search')), findsOneWidget);
        expect(find.byKey(const ValueKey('launcher-pages')), findsOneWidget);
        expect(tester.takeException(), null);
      },
    );
  }
}
