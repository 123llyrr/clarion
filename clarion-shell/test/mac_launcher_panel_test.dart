// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/mac_launchpad_layout.dart';
import 'package:clarion_shell/mac_shell_preferences.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/ui/launcher_panel.dart';
import 'package:clarion_shell/ui/launcher_surface.dart';
import 'package:clarion_shell/ui/mac_launcher_panel.dart';

class _Controller extends ShellController {
  _Controller({int count = 80})
    : super(
        demo: true,
        tray: false,
        notifications: false,
        powerProfilesEnabled: false,
      ) {
    entries = List.generate(
      count,
      (index) => DesktopApp(
        id: 'app-$index',
        name: 'Application $index',
        desktopFile: '',
        keywords: index == 43 ? const ['needle'] : const [],
      ),
    );
    setShellStyle(ShellStyle.macos);
    setReduceMotion(true);
    selectPanel(ShellPanel.launcher);
  }
  late List<DesktopApp> entries;
  final launched = <String>[];
  @override
  List<DesktopApp> get apps => entries;
  @override
  Future<void> launchApp(DesktopApp app) async {
    launched.add(app.id);
    closePanel();
  }

  void unrelatedNotification() => notifyListeners();
  void replaceCatalog(List<DesktopApp> apps) {
    entries = apps;
    notifyListeners();
  }
}

class _PendingLayout extends MacLaunchpadLayout {
  _PendingLayout() : super(persist: false);
  final writes = Completer<void>();
  int saves = 0;
  bool disposed = false;
  @override
  Future<void> save() {
    saves++;
    return writes.future;
  }

  @override
  void dispose() {
    disposed = true;
    super.dispose();
  }
}

Future<void> _mount(
  WidgetTester tester,
  _Controller controller,
  MacLaunchpadLayout layout, {
  Size size = const Size(1366, 900),
  bool surface = false,
  double textScale = 1,
}) async {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  addTearDown(controller.dispose);
  addTearDown(layout.dispose);
  await layout.load(controller.apps.map((app) => app.id));
  await tester.pumpWidget(
    MaterialApp(
      builder: (context, child) => MediaQuery(
        data: MediaQuery.of(
          context,
        ).copyWith(textScaler: TextScaler.linear(textScale)),
        child: child!,
      ),
      home: surface
          ? AnimatedBuilder(
              animation: controller,
              builder: (context, _) => LauncherSurface(
                controller: controller,
                visible: controller.panel == ShellPanel.launcher,
                reducedMotion: true,
              ),
            )
          : MacLauncherPanel(controller: controller, layout: layout),
    ),
  );
  await tester.pumpAndSettle();
}

Finder _app(int id) => find.byKey(ValueKey('app-app-$id'));
double _page(WidgetTester tester) => tester
    .widget<PageView>(find.byKey(const ValueKey('mac-launchpad-pages')))
    .controller!
    .page!;
Future<void> _drag(WidgetTester tester, Finder app, Offset destination) async {
  final gesture = await tester.startGesture(tester.getCenter(app));
  await tester.pump(const Duration(milliseconds: 360));
  await gesture.moveTo(destination);
  await tester.pump();
  await gesture.up();
  await tester.pumpAndSettle();
}

void main() {
  for (final shrinkRoot in [false, true]) {
    testWidgets(
      'catalog removes an open second-page folder and ${shrinkRoot ? 'clamps' : 'restores'} the saved root page',
      (tester) async {
        final controller = _Controller();
        final layout = MacLaunchpadLayout(persist: false);
        await layout.load(controller.apps.map((app) => app.id));
        layout.group('app-40', 'app-41');
        final folder = layout.entries.firstWhere((entry) => entry.isFolder);
        await _mount(tester, controller, layout);
        await tester.tap(find.byKey(const ValueKey('mac-launchpad-dot-1')));
        await tester.pumpAndSettle();
        await tester.tap(
          find.byKey(ValueKey('mac-launchpad-folder-${folder.id}')),
        );
        await tester.pumpAndSettle();
        expect(_page(tester), 0);

        // One removed member must leave the same open, named folder intact.
        controller.replaceCatalog(
          controller.apps.where((app) => app.id != 'app-40').toList(),
        );
        await tester.pumpAndSettle();
        expect(layout.find(folder.id)!.appIds, ['app-41']);
        expect(
          find.byKey(const ValueKey('mac-launchpad-folder-panel')),
          findsOneWidget,
        );

        final survivors = controller.apps
            .where(
              (app) =>
                  app.id != 'app-41' &&
                  (!shrinkRoot || int.parse(app.id.substring(4)) < 12),
            )
            .toList();
        controller.replaceCatalog(survivors);
        await tester.pumpAndSettle();
        expect(layout.find(folder.id), isNull);
        expect(
          find.byKey(const ValueKey('mac-launchpad-folder-panel')),
          findsNothing,
        );
        expect(_page(tester), shrinkRoot ? 0 : 1);
        expect(
          layout.orderedAppIds.toList(),
          survivors.map((app) => app.id).toList(),
        );
        expect(layout.orderedAppIds.toSet().length, survivors.length);
        expect(_app(shrinkRoot ? 0 : 35), findsOneWidget);
        expect(
          tester
              .widget<TextField>(
                find.byKey(const ValueKey('mac-launchpad-search')),
              )
              .focusNode!
              .hasFocus,
          isTrue,
        );
        expect(controller.panel, ShellPanel.launcher);
        expect(controller.launched, isEmpty);
        expect(tester.takeException(), isNull);
      },
    );
  }

  testWidgets(
    'closing a folder returns to its original root page and app positions',
    (tester) async {
      final controller = _Controller();
      final layout = MacLaunchpadLayout(persist: false);
      await layout.load(controller.apps.map((app) => app.id));
      layout.group('app-40', 'app-41');
      final folder = layout.entries.firstWhere((entry) => entry.isFolder);
      await _mount(tester, controller, layout);
      await tester.tap(find.byKey(const ValueKey('mac-launchpad-dot-1')));
      await tester.pumpAndSettle();
      final oldRect = tester.getRect(_app(35));
      await tester.tap(
        find.byKey(ValueKey('mac-launchpad-folder-${folder.id}')),
      );
      await tester.pumpAndSettle();
      expect(
        find.byKey(const ValueKey('mac-launchpad-folder-backdrop')),
        findsOneWidget,
      );
      expect(
        find.byKey(const ValueKey('mac-launchpad-folder-panel')),
        findsOneWidget,
      );
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      expect(_page(tester), 1);
      expect(tester.getRect(_app(35)), oldRect);
      expect(controller.panel, ShellPanel.launcher);
      expect(controller.launched, isEmpty);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'search clear restores the browsing page and focused Command arrows navigate',
    (tester) async {
      final controller = _Controller();
      await _mount(tester, controller, MacLaunchpadLayout(persist: false));
      final search = find.byKey(const ValueKey('mac-launchpad-search'));
      await tester.tap(find.byKey(const ValueKey('mac-launchpad-dot-2')));
      await tester.pumpAndSettle();
      await tester.enterText(search, 'needle');
      await tester.pumpAndSettle();
      expect(_app(43), findsOneWidget);
      expect(_page(tester), 0);
      await tester.tap(
        find.byKey(const ValueKey('mac-launchpad-clear-search')),
      );
      await tester.pumpAndSettle();
      expect(_page(tester), 2);
      final editable = tester.widget<TextField>(search);
      expect(editable.focusNode!.hasFocus, isTrue);
      await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
      await tester.sendKeyEvent(LogicalKeyboardKey.arrowLeft);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
      await tester.pumpAndSettle();
      expect(_page(tester), 1);
      await tester.enterText(search, '  ');
      await tester.pumpAndSettle();
      expect(_page(tester), 1);
      expect(controller.launched, isEmpty);
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      expect(controller.panel, ShellPanel.none);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'mouse drag is immediate while a touch swipe still changes pages',
    (tester) async {
      final controller = _Controller();
      final layout = MacLaunchpadLayout(persist: false);
      await _mount(tester, controller, layout);
      final target = tester.getRect(_app(0));
      final mouse = await tester.startGesture(
        tester.getCenter(_app(2)),
        kind: PointerDeviceKind.mouse,
      );
      await mouse.moveBy(const Offset(25, 0));
      await tester.pump();
      await mouse.moveTo(Offset(target.left + 5, target.center.dy));
      await tester.pump();
      await mouse.up();
      await mouse.removePointer();
      await tester.pumpAndSettle();
      expect(layout.entries.take(3).map((entry) => entry.id), [
        'app-2',
        'app-0',
        'app-1',
      ]);
      await tester.drag(
        find.byKey(const ValueKey('mac-launchpad-pages')),
        const Offset(-900, 0),
      );
      await tester.pumpAndSettle();
      expect(_page(tester), 1);
      expect(controller.launched, isEmpty);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'cancel and panel closure stop active drag edge paging without editing the layout',
    (tester) async {
      final controller = _Controller();
      final layout = MacLaunchpadLayout(persist: false);
      await _mount(tester, controller, layout);
      final before = layout.orderedAppIds.toList();
      final gesture = await tester.startGesture(tester.getCenter(_app(0)));
      await tester.pump(const Duration(milliseconds: 360));
      await gesture.moveTo(
        tester.getCenter(find.byKey(const ValueKey('mac-launchpad-edge-1'))),
      );
      await tester.pump(const Duration(milliseconds: 200));
      await gesture.cancel();
      await tester.pump(const Duration(seconds: 1));
      expect(_page(tester), 0);
      expect(layout.orderedAppIds, before);
      final closing = await tester.startGesture(tester.getCenter(_app(0)));
      await tester.pump(const Duration(milliseconds: 360));
      await closing.moveTo(
        tester.getCenter(find.byKey(const ValueKey('mac-launchpad-edge-1'))),
      );
      controller.closePanel();
      await tester.pump(const Duration(seconds: 1));
      await closing.up();
      await tester.pumpAndSettle();
      expect(_page(tester), 0);
      expect(layout.orderedAppIds, before);
      expect(controller.launched, isEmpty);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'continuous wheel packets make one page change per gesture gate',
    (tester) async {
      final controller = _Controller(count: 220);
      await _mount(tester, controller, MacLaunchpadLayout(persist: false));
      final point = tester.getCenter(
        find.byKey(const ValueKey('mac-launchpad-pages')),
      );
      for (var i = 0; i < 20; i++) {
        await tester.sendEventToBinding(
          PointerScrollEvent(position: point, scrollDelta: const Offset(60, 0)),
        );
        await tester.pump(const Duration(milliseconds: 5));
      }
      await tester.pumpAndSettle();
      expect(_page(tester), 1);
      expect(controller.launched, isEmpty);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('a trackpad stroke follows the fingers and returns when short', (
    tester,
  ) async {
    final controller = _Controller(count: 220);
    await _mount(tester, controller, MacLaunchpadLayout(persist: false));
    final pages = find.byKey(const ValueKey('mac-launchpad-pages'));
    final point = tester.getCenter(pages);
    Future<double> offset() async => tester
        .widget<PageView>(pages)
        .controller!
        .offset;
    for (var i = 0; i < 3; i++) {
      await tester.sendEventToBinding(
        PointerScrollEvent(
          position: point,
          scrollDelta: const Offset(60, 0),
          kind: PointerDeviceKind.trackpad,
        ),
      );
      await tester.pump(const Duration(milliseconds: 8));
    }
    // The page moves with the stroke instead of waiting for a threshold: the
    // fractional page is the stroke's own progress.
    expect(await offset(), greaterThan(0));
    expect(_page(tester), lessThan(0.5));
    // A stroke shorter than a third of a page returns to where it started.
    await tester.pump(const Duration(milliseconds: 200));
    await tester.pumpAndSettle();
    await tester.pumpAndSettle();
    expect(_page(tester), 0);
    expect(await offset(), lessThan(1));
  });

  testWidgets('a long trackpad stroke commits to the next page', (
    tester,
  ) async {
    final controller = _Controller(count: 220);
    await _mount(tester, controller, MacLaunchpadLayout(persist: false));
    final pages = find.byKey(const ValueKey('mac-launchpad-pages'));
    final point = tester.getCenter(pages);
    for (var i = 0; i < 10; i++) {
      await tester.sendEventToBinding(
        PointerScrollEvent(
          position: point,
          scrollDelta: const Offset(80, 0),
          kind: PointerDeviceKind.trackpad,
        ),
      );
      await tester.pump(const Duration(milliseconds: 8));
    }
    expect(_page(tester), greaterThan(0.3));
    await tester.pump(const Duration(milliseconds: 200));
    await tester.pumpAndSettle();
    await tester.pumpAndSettle();
    expect(_page(tester), closeTo(1, .01));
  });

  testWidgets('a vertical trackpad stroke never pages', (tester) async {
    final controller = _Controller(count: 220);
    await _mount(tester, controller, MacLaunchpadLayout(persist: false));
    final pages = find.byKey(const ValueKey('mac-launchpad-pages'));
    final point = tester.getCenter(pages);
    for (var i = 0; i < 10; i++) {
      await tester.sendEventToBinding(
        PointerScrollEvent(
          position: point,
          scrollDelta: const Offset(12, 90),
          kind: PointerDeviceKind.trackpad,
        ),
      );
      await tester.pump(const Duration(milliseconds: 8));
    }
    await tester.pump(const Duration(milliseconds: 200));
    await tester.pumpAndSettle();
    expect(_page(tester), 0);
    expect(tester.widget<PageView>(pages).controller!.offset, 0);
    expect(controller.launched, isEmpty);
  });

  testWidgets('wall-clock delay does not split an active wheel gesture', (
    tester,
  ) async {
    final controller = _Controller(count: 220);
    await _mount(tester, controller, MacLaunchpadLayout(persist: false));
    final point = tester.getCenter(
      find.byKey(const ValueKey('mac-launchpad-pages')),
    );
    await tester.sendEventToBinding(
      PointerScrollEvent(position: point, scrollDelta: const Offset(60, 0)),
    );
    await tester.pump(const Duration(milliseconds: 5));
    // Exercise the old wall-clock reset independently of the framework's
    // inactivity timer, as can happen during costly rendering or a clock jump.
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 230)),
    );
    await tester.sendEventToBinding(
      PointerScrollEvent(position: point, scrollDelta: const Offset(60, 0)),
    );
    await tester.pumpAndSettle();
    expect(_page(tester), 1);
    expect(controller.launched, isEmpty);
  });

  testWidgets(
    'reversing wheel direction immediately navigates back without delay',
    (tester) async {
      final controller = _Controller(count: 220);
      await _mount(tester, controller, MacLaunchpadLayout(persist: false));
      final point = tester.getCenter(
        find.byKey(const ValueKey('mac-launchpad-pages')),
      );
      // Scroll right to page 1
      await tester.sendEventToBinding(
        PointerScrollEvent(position: point, scrollDelta: const Offset(60, 0)),
      );
      await tester.pumpAndSettle();
      expect(_page(tester), 1);

      // Immediately reverse direction left without waiting for any lockout gate
      await tester.sendEventToBinding(
        PointerScrollEvent(position: point, scrollDelta: const Offset(-60, 0)),
      );
      await tester.pumpAndSettle();
      expect(_page(tester), 0);
      expect(controller.launched, isEmpty);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'discrete wheel ticks after inactivity advance to subsequent pages',
    (tester) async {
      final controller = _Controller(count: 220);
      await _mount(tester, controller, MacLaunchpadLayout(persist: false));
      final point = tester.getCenter(
        find.byKey(const ValueKey('mac-launchpad-pages')),
      );
      // Tick 1 to page 1
      await tester.sendEventToBinding(
        PointerScrollEvent(position: point, scrollDelta: const Offset(60, 0)),
      );
      await tester.pumpAndSettle();
      expect(_page(tester), 1);

      // Pause for gesture reset (>180ms)
      await tester.pump(const Duration(milliseconds: 220));

      // Tick 2 to page 2
      await tester.sendEventToBinding(
        PointerScrollEvent(position: point, scrollDelta: const Offset(60, 0)),
      );
      await tester.pumpAndSettle();
      expect(_page(tester), 2);
      expect(tester.takeException(), isNull);
    },
  );
  testWidgets(
    'mouse opens without a selection ring and keyboard navigation selects a real app',
    (tester) async {
      final controller = _Controller(count: 6);
      await _mount(tester, controller, MacLaunchpadLayout(persist: false));
      Color border() =>
          ((tester
                              .widget<Container>(
                                find.byKey(
                                  const ValueKey(
                                    'mac-launchpad-selection-app-0',
                                  ),
                                ),
                              )
                              .decoration!
                          as BoxDecoration)
                      .border!
                  as Border)
              .top
              .color;
      expect(border(), Colors.transparent);
      await tester.sendKeyEvent(LogicalKeyboardKey.arrowRight);
      await tester.pumpAndSettle();
      expect(border().a, greaterThan(0));
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pumpAndSettle();
      expect(controller.launched, ['app-0']);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'invalid app self drops and folder-on-app drops preserve the complete layout',
    (tester) async {
      final controller = _Controller(count: 6);
      final layout = MacLaunchpadLayout(persist: false);
      await _mount(tester, controller, layout);
      final before = layout.orderedAppIds.toList();
      await _drag(tester, _app(0), tester.getCenter(_app(0)));
      expect(layout.orderedAppIds, before);
      layout.group('app-0', 'app-1');
      await tester.pumpAndSettle();
      final folderId = layout.entries.first.id;
      final grouped = layout.entries.map((entry) => entry.toJson()).toList();
      await _drag(
        tester,
        find.byKey(ValueKey('mac-launchpad-folder-$folderId')),
        tester.getCenter(_app(2)),
      );
      expect(layout.entries.map((entry) => entry.toJson()).toList(), grouped);
      expect(controller.launched, isEmpty);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'replacing an injected layout detaches the old model and uses live replacement data',
    (tester) async {
      final controller = _Controller(count: 6);
      final first = _PendingLayout();
      final second = _PendingLayout();
      addTearDown(controller.dispose);
      addTearDown(first.dispose);
      addTearDown(second.dispose);
      for (final layout in [first, second]) {
        await layout.load(controller.apps.map((app) => app.id));
      }
      first.group('app-0', 'app-1');
      first.rename(first.entries.first.id, 'Old folder');
      second.group('app-2', 'app-3');
      second.rename(second.entries[2].id, 'New folder');
      var current = first;
      late StateSetter update;
      await tester.pumpWidget(
        MaterialApp(
          home: StatefulBuilder(
            builder: (context, setState) {
              update = setState;
              return MacLauncherPanel(controller: controller, layout: current);
            },
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.text('Old folder'), findsOneWidget);
      update(() => current = second);
      await tester.pumpAndSettle();
      expect(find.text('New folder'), findsOneWidget);
      expect(find.text('Old folder'), findsNothing);
      expect(first.disposed, isFalse);
      final grid = tester.widget<GridView>(
        find.byKey(const ValueKey('mac-launchpad-page-0')),
      );
      first.rename(first.entries.first.id, 'Detached');
      await tester.pump();
      expect(
        tester.widget<GridView>(
          find.byKey(const ValueKey('mac-launchpad-page-0')),
        ),
        same(grid),
      );
      second.rename(second.entries[2].id, 'Live name');
      await tester.pumpAndSettle();
      expect(find.text('Live name'), findsOneWidget);
      first.writes.complete();
      second.writes.complete();
      await tester.pumpWidget(const SizedBox.shrink());
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'small logical output and large text keep search, icons and many page dots usable',
    (tester) async {
      final controller = _Controller();
      await _mount(
        tester,
        controller,
        MacLaunchpadLayout(persist: false),
        size: const Size(360, 240),
        textScale: 2,
      );
      expect(_app(0), findsOneWidget);
      expect(
        find.byKey(const ValueKey('mac-launchpad-search')),
        findsOneWidget,
      );
      expect(tester.takeException(), isNull);
      await tester.tap(_app(0));
      await tester.pumpAndSettle();
      expect(controller.launched, ['app-0']);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'reopening reconciles applications removed while the retained layout had no panel listener',
    (tester) async {
      final controller = _Controller(count: 6);
      final layout = MacLaunchpadLayout(persist: false);
      addTearDown(controller.dispose);
      addTearDown(layout.dispose);
      await layout.load(controller.apps.map((app) => app.id));
      layout.group('app-0', 'app-1');
      var visible = true;
      late StateSetter update;
      await tester.pumpWidget(
        MaterialApp(
          home: StatefulBuilder(
            builder: (context, setState) {
              update = setState;
              return LauncherSurface(
                controller: controller,
                visible: visible,
                reducedMotion: true,
                macLayout: layout,
              );
            },
          ),
        ),
      );
      await tester.pumpAndSettle();
      final folderId = layout.entries.first.id;
      await tester.tap(find.byKey(ValueKey('mac-launchpad-folder-$folderId')));
      await tester.pumpAndSettle();
      await tester.tap(
        find.byKey(const ValueKey('mac-launchpad-folder-title')),
      );
      await tester.pumpAndSettle();
      await tester.enterText(
        find.byKey(const ValueKey('mac-launchpad-folder-name')),
        'Saved on close',
      );
      update(() => visible = false);
      await tester.pumpAndSettle();
      controller.replaceCatalog(
        controller.apps.where((app) => app.id != 'app-0').toList(),
      );
      update(() => visible = true);
      await tester.pumpAndSettle();
      expect(layout.orderedAppIds, isNot(contains('app-0')));
      expect(layout.entries.first.appIds, ['app-1']);
      expect(layout.entries.first.name, 'Saved on close');
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'clock and slider notifications do not rebuild the visible app grid',
    (tester) async {
      final controller = _Controller();
      await _mount(
        tester,
        controller,
        MacLaunchpadLayout(persist: false),
        surface: true,
      );
      final gridFinder = find.byKey(const ValueKey('mac-launchpad-page-0'));
      final before = tester.widget<GridView>(gridFinder);
      for (var index = 0; index < 30; index++) {
        controller.unrelatedNotification();
        await tester.pump();
        expect(identical(tester.widget<GridView>(gridFinder), before), isTrue);
      }
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'Catalina grid shows 7 by 5 apps and dot pagination without Material chrome',
    (tester) async {
      final controller = _Controller();
      final layout = MacLaunchpadLayout(persist: false);
      await _mount(tester, controller, layout);
      final grid = tester.widget<GridView>(
        find.byKey(const ValueKey('mac-launchpad-page-0')),
      );
      expect(
        (grid.gridDelegate as SliverGridDelegateWithFixedCrossAxisCount)
            .crossAxisCount,
        7,
      );
      expect(grid.childrenDelegate.estimatedChildCount, 35);
      expect(find.byKey(const ValueKey('mac-launchpad-dot-2')), findsOneWidget);
      expect(find.byKey(const ValueKey('launcher-next')), findsNothing);
      expect(find.byType(LauncherPanel), findsNothing);
      await tester.tap(find.byKey(const ValueKey('mac-launchpad-dot-2')));
      await tester.pumpAndSettle();
      expect(_app(70), findsOneWidget);
      await tester.tap(_app(70));
      await tester.pumpAndSettle();
      expect(controller.launched, ['app-70']);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'search reaches apps on other pages and keyboard opens the matching real app',
    (tester) async {
      final controller = _Controller();
      await _mount(tester, controller, MacLaunchpadLayout(persist: false));
      await tester.enterText(
        find.byKey(const ValueKey('mac-launchpad-search')),
        'needle',
      );
      await tester.pumpAndSettle();
      expect(_app(43), findsOneWidget);
      expect(_app(0), findsNothing);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pumpAndSettle();
      expect(controller.launched, ['app-43']);
      expect(controller.panel, ShellPanel.none);
    },
  );

  testWidgets(
    'app drag creates folder, folder opens and Escape leaves folder before Launchpad',
    (tester) async {
      final controller = _Controller(count: 6);
      final layout = MacLaunchpadLayout(persist: false);
      await _mount(tester, controller, layout);
      await _drag(tester, _app(0), tester.getCenter(_app(1)));
      final folder = layout.entries.first;
      expect(folder.isFolder, isTrue);
      expect(folder.appIds, ['app-1', 'app-0']);
      expect(controller.launched, isEmpty);
      await tester.tap(
        find.byKey(ValueKey('mac-launchpad-folder-${folder.id}')),
      );
      await tester.pumpAndSettle();
      expect(
        find.byKey(const ValueKey('mac-launchpad-folder-title')),
        findsOneWidget,
      );
      expect(_app(0), findsOneWidget);
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      expect(
        find.byKey(const ValueKey('mac-launchpad-folder-title')),
        findsNothing,
      );
      expect(controller.panel, ShellPanel.launcher);
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      expect(controller.panel, ShellPanel.none);
    },
  );

  testWidgets(
    'folder title renames, internal blank cells reorder and only an external drop extracts',
    (tester) async {
      final controller = _Controller(count: 6);
      final layout = MacLaunchpadLayout(persist: false);
      await layout.load(controller.apps.map((app) => app.id));
      layout.group('app-0', 'app-1');
      final folderId = layout.entries.first.id;
      await _mount(tester, controller, layout);
      await tester.tap(find.byKey(ValueKey('mac-launchpad-folder-$folderId')));
      await tester.pumpAndSettle();
      await tester.tap(
        find.byKey(const ValueKey('mac-launchpad-folder-title')),
      );
      await tester.pumpAndSettle();
      expect(find.byType(AlertDialog), findsNothing);
      await tester.enterText(
        find.byKey(const ValueKey('mac-launchpad-folder-name')),
        'Tools',
      );
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pumpAndSettle();
      expect(layout.find(folderId)!.name, 'Tools');
      await tester.tap(
        find.byKey(const ValueKey('mac-launchpad-folder-title')),
      );
      await tester.pumpAndSettle();
      await tester.enterText(
        find.byKey(const ValueKey('mac-launchpad-folder-name')),
        'Cancel this',
      );
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      expect(layout.find(folderId)!.name, 'Tools');
      expect(
        find.byKey(const ValueKey('mac-launchpad-folder-title')),
        findsOneWidget,
      );
      expect(controller.panel, ShellPanel.launcher);
      final grid = tester.getRect(
        find.byKey(const ValueKey('mac-launchpad-pages')),
      );
      await _drag(tester, _app(1), grid.bottomRight - const Offset(20, 20));
      expect(layout.find(folderId)!.appIds, ['app-0', 'app-1']);
      expect(layout.entries.every((entry) => entry.id != 'app-1'), isTrue);
      await _drag(tester, _app(0), const Offset(4, 400));
      expect(layout.find(folderId)!.appIds, ['app-1']);
      expect(
        layout.orderedAppIds.toSet(),
        controller.apps.map((app) => app.id).toSet(),
      );
      expect(layout.entries.any((entry) => entry.id == 'app-0'), isTrue);
      expect(
        find.byKey(const ValueKey('mac-launchpad-folder-panel')),
        findsNothing,
      );
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('gap drop reorders applications without launching or grouping', (
    tester,
  ) async {
    final controller = _Controller(count: 6);
    final layout = MacLaunchpadLayout(persist: false);
    await _mount(tester, controller, layout);
    final target = tester.getRect(_app(0));
    await _drag(tester, _app(2), Offset(target.left + 5, target.center.dy));
    expect(layout.entries.take(3).map((entry) => entry.id), [
      'app-2',
      'app-0',
      'app-1',
    ]);
    expect(layout.entries.every((entry) => !entry.isFolder), isTrue);
    expect(controller.launched, isEmpty);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'Command arrows change pages and dragging at the screen edge moves a real app across pages',
    (tester) async {
      final controller = _Controller();
      final layout = MacLaunchpadLayout(persist: false);
      await _mount(tester, controller, layout);
      await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
      await tester.sendKeyEvent(LogicalKeyboardKey.arrowRight);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
      await tester.pumpAndSettle();
      expect(_app(35), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('mac-launchpad-dot-0')));
      await tester.pumpAndSettle();
      final gesture = await tester.startGesture(tester.getCenter(_app(0)));
      await tester.pump(const Duration(milliseconds: 360));
      await gesture.moveTo(
        tester.getCenter(find.byKey(const ValueKey('mac-launchpad-edge-1'))),
      );
      await tester.pump(const Duration(milliseconds: 430));
      await tester.pumpAndSettle();
      expect(_app(36), findsOneWidget);
      final destination = tester.getRect(_app(36));
      await gesture.moveTo(Offset(destination.left + 5, destination.center.dy));
      await tester.pump();
      await gesture.up();
      await tester.pumpAndSettle();
      expect(layout.orderedAppIds.toList().indexOf('app-0'), 35);
      expect(layout.orderedAppIds.toSet().length, 80);
      expect(controller.launched, isEmpty);
      expect(tester.takeException(), isNull);
    },
  );

  for (final position in MacDockPosition.values) {
    testWidgets('$position grid and page dots avoid the configured 64px Dock', (
      tester,
    ) async {
      final controller = _Controller();
      controller.setMacPreferences(
        MacShellPreferences(dockPosition: position, dockSize: 64),
      );
      await _mount(tester, controller, MacLaunchpadLayout(persist: false));
      final grid = tester.getRect(
        find.byKey(const ValueKey('mac-launchpad-pages')),
      );
      final dots = tester.getRect(
        find.byKey(const ValueKey('mac-launchpad-dot-0')),
      );
      if (position == MacDockPosition.bottom) {
        expect(grid.bottom, lessThanOrEqualTo(820));
        expect(dots.bottom, lessThanOrEqualTo(820));
      } else if (position == MacDockPosition.left) {
        expect(grid.left, greaterThanOrEqualTo(80));
      } else {
        expect(grid.right, lessThanOrEqualTo(1286));
      }
      expect(tester.takeException(), isNull);
    });
  }

  testWidgets(
    'Surface retains current pixels on reversal and changes only Mac branch',
    (tester) async {
      final controller = _Controller(count: 6)..setReduceMotion(false);
      addTearDown(controller.dispose);
      var visible = true;
      late StateSetter update;
      await tester.pumpWidget(
        MaterialApp(
          home: StatefulBuilder(
            builder: (context, setState) {
              update = setState;
              return LauncherSurface(
                controller: controller,
                visible: visible,
                reducedMotion: false,
              );
            },
          ),
        ),
      );
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 80));
      final panel = find.byType(MacLauncherPanel);
      final tint = find.byKey(const ValueKey('launcher-desktop-tint'));
      expect(tint, findsOneWidget);
      expect(find.byType(BackdropFilter), findsNothing);
      expect(
        find.ancestor(of: tint, matching: find.byType(Opacity)),
        findsNothing,
      );
      expect(
        find.ancestor(of: tint, matching: find.byType(Transform)),
        findsNothing,
      );
      double alpha() => tester
          .widget<Opacity>(
            find.ancestor(of: panel, matching: find.byType(Opacity)).first,
          )
          .opacity;
      expect(alpha(), inExclusiveRange(0, 1));
      update(() => visible = false);
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 20));
      final closing = alpha();
      update(() => visible = true);
      await tester.pump();
      expect(alpha(), closeTo(closing, .001));
      await tester.pumpAndSettle();
      expect(alpha(), 1);
      update(() => visible = false);
      await tester.pumpAndSettle();
      expect(panel, findsNothing);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'rapid close and reopen preserves the current folder while saves are pending',
    (tester) async {
      final controller = _Controller(count: 6);
      final layout = _PendingLayout();
      addTearDown(controller.dispose);
      addTearDown(layout.dispose);
      await layout.load(controller.apps.map((app) => app.id));
      layout.group('app-0', 'app-1');
      final folderId = layout.entries.first.id;
      layout.rename(folderId, 'Latest');
      layout.moveBefore('app-5', folderId);
      expect(layout.saves, greaterThan(0));
      expect(layout.writes.isCompleted, isFalse);
      var visible = true;
      late StateSetter update;
      await tester.pumpWidget(
        MaterialApp(
          home: StatefulBuilder(
            builder: (context, setState) {
              update = setState;
              return LauncherSurface(
                controller: controller,
                visible: visible,
                reducedMotion: true,
                macLayout: layout,
              );
            },
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(
        tester.widget<MacLauncherPanel>(find.byType(MacLauncherPanel)).layout,
        same(layout),
      );
      update(() => visible = false);
      await tester.pumpAndSettle();
      expect(find.byType(MacLauncherPanel), findsNothing);
      expect(layout.disposed, isFalse);
      update(() => visible = true);
      await tester.pumpAndSettle();
      expect(
        tester.widget<MacLauncherPanel>(find.byType(MacLauncherPanel)).layout,
        same(layout),
      );
      expect(layout.entries.first.id, 'app-5');
      expect(layout.find(folderId)!.name, 'Latest');
      expect(find.text('Latest'), findsOneWidget);
      layout.writes.complete();
      await tester.pumpWidget(const SizedBox.shrink());
      expect(tester.takeException(), isNull);
    },
  );
}
