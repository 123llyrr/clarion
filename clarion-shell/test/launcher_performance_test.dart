// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:collection';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/mac_launchpad_layout.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/ui/app_icon.dart';
import 'package:clarion_shell/ui/launcher_surface.dart';
import 'package:clarion_shell/ui/mac_launcher_panel.dart';

class _App extends DesktopApp {
  _App(int index, {String? name})
    : super(
        id: 'app-$index',
        name: name ?? 'Application $index',
        desktopFile: '',
      );
  int searches = 0;
  @override
  bool matches(String query) {
    searches++;
    return super.matches(query);
  }
}

class _Catalogue extends ListBase<DesktopApp> {
  _Catalogue(this.entries);
  final List<_App> entries;
  int reads = 0;
  int get searches => entries.fold(0, (sum, app) => sum + app.searches);
  @override
  int get length => entries.length;
  @override
  set length(int value) => throw UnsupportedError('immutable catalogue');
  @override
  DesktopApp operator [](int index) {
    reads++;
    return entries[index];
  }

  @override
  void operator []=(int index, DesktopApp value) =>
      throw UnsupportedError('immutable catalogue');
}

class _Controller extends ShellController {
  _Controller(this.catalogue, {bool macos = false, bool reduced = true})
    : super(
        demo: true,
        tray: false,
        notifications: false,
        powerProfilesEnabled: false,
      ) {
    setShellStyle(macos ? ShellStyle.macos : ShellStyle.material);
    setReduceMotion(reduced);
    selectPanel(ShellPanel.launcher);
  }
  _Catalogue catalogue;
  bool launching = false;
  String? failure;
  final launched = <DesktopApp>[];
  @override
  List<DesktopApp> get apps => catalogue;
  @override
  Set<String> get busy => launching ? {'launch'} : {};
  @override
  String? get error => failure;
  @override
  Future<void> launchApp(DesktopApp app) async => launched.add(app);
  void changed() => notifyListeners();
  void replace(_Catalogue value) {
    catalogue = value;
    notifyListeners();
  }
}

Future<void> _show(WidgetTester tester, Widget child) async {
  tester.view.physicalSize = const Size(1366, 900);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  await tester.pumpWidget(MaterialApp(home: child));
  await tester.pumpAndSettle();
}

void main() {
  testWidgets(
    'Material ignores unrelated notifications and reuses search results',
    (tester) async {
      final catalogue = _Catalogue([for (var i = 0; i < 1000; i++) _App(i)]);
      final controller = _Controller(catalogue);
      addTearDown(controller.dispose);
      await _show(
        tester,
        ListenableBuilder(
          listenable: controller,
          builder: (context, _) => LauncherSurface(
            controller: controller,
            visible: controller.panel == ShellPanel.launcher,
            reducedMotion: controller.reduceMotion,
          ),
        ),
      );
      final grid = find.byKey(const ValueKey('launcher-page-0'));
      final before = tester.widget<GridView>(grid);
      catalogue.reads = 0;
      for (var i = 0; i < 30; i++) {
        controller.changed();
        await tester.pump();
        expect(tester.widget<GridView>(grid), same(before));
      }
      expect(catalogue.reads, 0);

      await tester.enterText(
        find.byKey(const ValueKey('launcher-search')),
        'Application',
      );
      await tester.pumpAndSettle();
      expect(catalogue.searches, 1000);
      await tester.tap(find.byKey(const ValueKey('launcher-next')));
      await tester.pumpAndSettle();
      for (var i = 0; i < 5; i++) {
        await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
        await tester.pumpAndSettle();
      }
      expect(catalogue.searches, 1000);

      controller.launching = true;
      controller.failure = 'Launch temporarily unavailable';
      controller.changed();
      await tester.pumpAndSettle();
      expect(find.text(controller.failure!), findsOneWidget);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pump();
      expect(controller.launched, isEmpty);
      final replacement = _App(42, name: 'Application updated');
      controller.launching = false;
      controller.replace(_Catalogue([replacement]));
      await tester.pumpAndSettle();
      expect(find.text('Application updated'), findsOneWidget);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pump();
      expect(controller.launched.single, same(replacement));
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'Mac search is scanned once until its catalogue or layout changes',
    (tester) async {
      final catalogue = _Catalogue([for (var i = 0; i < 1000; i++) _App(i)]);
      final controller = _Controller(catalogue, macos: true);
      final layout = MacLaunchpadLayout(persist: false);
      addTearDown(controller.dispose);
      addTearDown(layout.dispose);
      await layout.load(controller.apps.map((app) => app.id));
      await _show(
        tester,
        MacLauncherPanel(controller: controller, layout: layout),
      );
      await tester.enterText(
        find.byKey(const ValueKey('mac-launchpad-search')),
        'Application',
      );
      await tester.pumpAndSettle();
      expect(catalogue.searches, 1000);
      await tester.tap(find.byKey(const ValueKey('mac-launchpad-dot-1')));
      await tester.pumpAndSettle();
      for (var i = 0; i < 5; i++) {
        await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
        await tester.pumpAndSettle();
      }
      expect(catalogue.searches, 1000);
      layout.moveBefore('app-999', 'app-0');
      await tester.pumpAndSettle();
      expect(catalogue.searches, 2000);
      await tester.tap(find.byKey(const ValueKey('mac-launchpad-dot-0')));
      await tester.pumpAndSettle();
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pump();
      expect(controller.launched.single.id, 'app-999');
      expect(catalogue.searches, 2000);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('large Launchpad uses one edit ticker and closing stops it', (
    tester,
  ) async {
    final controller = _Controller(
      _Catalogue([for (var i = 0; i < 1000; i++) _App(i)]),
      macos: true,
      reduced: false,
    );
    final layout = MacLaunchpadLayout(persist: false);
    addTearDown(controller.dispose);
    addTearDown(layout.dispose);
    await layout.load(controller.apps.map((app) => app.id));
    await _show(
      tester,
      MacLauncherPanel(controller: controller, layout: layout),
    );
    final keyboard = tester.widget<Focus>(
      find.byWidgetPredicate(
        (widget) =>
            widget is Focus && widget.focusNode?.debugLabel == 'mac-launchpad',
      ),
    );
    keyboard.focusNode!.requestFocus();
    await tester.pumpAndSettle();
    expect(tester.binding.transientCallbackCount, 0);
    await tester.sendKeyEvent(LogicalKeyboardKey.altLeft);
    await tester.pump();
    expect(
      find.byKey(const ValueKey('mac-launchpad-delete-badge')),
      findsWidgets,
    );
    for (var frame = 0; frame < 10; frame++) {
      await tester.pump(const Duration(milliseconds: 16));
      expect(tester.binding.transientCallbackCount, 1);
    }
    controller.closePanel();
    await tester.pumpAndSettle();
    expect(
      find.byKey(const ValueKey('mac-launchpad-delete-badge')),
      findsNothing,
    );
    expect(tester.binding.transientCallbackCount, 0);
    controller.selectPanel(ShellPanel.launcher);
    await tester.pumpAndSettle();
    expect(
      find.byKey(const ValueKey('mac-launchpad-delete-badge')),
      findsNothing,
    );
    expect(tester.binding.transientCallbackCount, 0);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'closing edit mode reverses without retaining an idle ticker or blur',
    (tester) async {
      final controller = _Controller(
        _Catalogue([for (var i = 0; i < 100; i++) _App(i)]),
        macos: true,
        reduced: false,
      );
      final layout = MacLaunchpadLayout(persist: false);
      final surfaces = <bool>[];
      addTearDown(controller.dispose);
      addTearDown(layout.dispose);
      await layout.load(controller.apps.map((app) => app.id));
      await _show(
        tester,
        ListenableBuilder(
          listenable: controller,
          builder: (context, _) => LauncherSurface(
            controller: controller,
            visible: controller.panel == ShellPanel.launcher,
            reducedMotion: controller.reduceMotion,
            macLayout: layout,
            onSurfaceVisible: surfaces.add,
          ),
        ),
      );
      await tester.sendKeyEvent(LogicalKeyboardKey.altLeft);
      await tester.pump();
      controller.closePanel();
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 32));
      controller.selectPanel(ShellPanel.launcher);
      await tester.pumpAndSettle();
      expect(surfaces, [true]);
      expect(
        find.byKey(const ValueKey('mac-launchpad-delete-badge')),
        findsNothing,
      );
      expect(tester.binding.transientCallbackCount, 0);
      controller.closePanel();
      await tester.pumpAndSettle();
      expect(find.byType(MacLauncherPanel), findsNothing);
      expect(find.byKey(const ValueKey('launcher-desktop-tint')), findsNothing);
      expect(surfaces, [true, false]);
      expect(tester.binding.transientCallbackCount, 0);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('editing and close reversal preserve mounted app icons', (
    tester,
  ) async {
    final controller = _Controller(
      _Catalogue([for (var i = 0; i < 70; i++) _App(i)]),
      macos: true,
      reduced: false,
    );
    final layout = MacLaunchpadLayout(persist: false);
    final surfaces = <bool>[];
    addTearDown(controller.dispose);
    addTearDown(layout.dispose);
    await layout.load(controller.apps.map((app) => app.id));
    await _show(
      tester,
      ListenableBuilder(
        listenable: controller,
        builder: (context, _) => LauncherSurface(
          controller: controller,
          visible: controller.panel == ShellPanel.launcher,
          reducedMotion: controller.reduceMotion,
          macLayout: layout,
          onSurfaceVisible: surfaces.add,
        ),
      ),
    );
    final keyboard = tester.widget<Focus>(
      find.byWidgetPredicate(
        (widget) =>
            widget is Focus && widget.focusNode?.debugLabel == 'mac-launchpad',
      ),
    );
    keyboard.focusNode!.requestFocus();
    await tester.pumpAndSettle();
    final icon = find.descendant(
      of: find.byKey(const ValueKey('app-app-0')),
      matching: find.byType(AppIcon),
    );
    expect(icon, findsOneWidget);
    final original = tester.element(icon);

    await tester.sendKeyEvent(LogicalKeyboardKey.altLeft);
    await tester.pump();
    expect(tester.element(icon), same(original));
    expect(
      find.byKey(const ValueKey('mac-launchpad-delete-badge')),
      findsWidgets,
    );
    await tester.pump(const Duration(milliseconds: 32));
    expect(tester.binding.transientCallbackCount, 1);
    await tester.sendKeyEvent(LogicalKeyboardKey.altLeft);
    await tester.pumpAndSettle();
    expect(tester.element(icon), same(original));
    expect(
      find.byKey(const ValueKey('mac-launchpad-delete-badge')),
      findsNothing,
    );
    expect(tester.binding.transientCallbackCount, 0);

    await tester.sendKeyEvent(LogicalKeyboardKey.altLeft);
    await tester.pump();
    expect(tester.element(icon), same(original));
    controller.closePanel();
    await tester.pump();
    expect(tester.element(icon), same(original));
    expect(
      find.byKey(const ValueKey('mac-launchpad-delete-badge')),
      findsNothing,
    );
    await tester.pump(const Duration(milliseconds: 32));
    controller.selectPanel(ShellPanel.launcher);
    await tester.pumpAndSettle();
    expect(tester.element(icon), same(original));
    expect(surfaces, [true]);
    expect(tester.binding.transientCallbackCount, 0);

    controller.closePanel();
    await tester.pumpAndSettle();
    expect(icon, findsNothing);
    expect(surfaces, [true, false]);
    expect(tester.binding.transientCallbackCount, 0);
    expect(tester.takeException(), isNull);
  });

  testWidgets('reduced motion preserves edit icon identity without ticking', (
    tester,
  ) async {
    final controller = _Controller(
      _Catalogue([for (var i = 0; i < 70; i++) _App(i)]),
      macos: true,
      reduced: false,
    );
    final layout = MacLaunchpadLayout(persist: false);
    addTearDown(controller.dispose);
    addTearDown(layout.dispose);
    await layout.load(controller.apps.map((app) => app.id));
    await _show(
      tester,
      MacLauncherPanel(controller: controller, layout: layout),
    );
    final keyboard = tester.widget<Focus>(
      find.byWidgetPredicate(
        (widget) =>
            widget is Focus && widget.focusNode?.debugLabel == 'mac-launchpad',
      ),
    );
    keyboard.focusNode!.requestFocus();
    await tester.pumpAndSettle();
    final icon = find.descendant(
      of: find.byKey(const ValueKey('app-app-0')),
      matching: find.byType(AppIcon),
    );
    expect(icon, findsOneWidget);
    final original = tester.element(icon);
    await tester.sendKeyEvent(LogicalKeyboardKey.altLeft);
    await tester.pump();
    expect(tester.element(icon), same(original));
    // Entering edit mode also starts a one-shot ticker that ends on the next
    // frame; the mode's steady state is the single shared jiggle clock.
    await tester.pump(const Duration(milliseconds: 16));
    expect(tester.binding.transientCallbackCount, 1);

    controller.setReduceMotion(true);
    await tester.pumpAndSettle();
    expect(tester.element(icon), same(original));
    expect(
      find.byKey(const ValueKey('mac-launchpad-delete-badge')),
      findsWidgets,
    );
    expect(tester.binding.transientCallbackCount, 0);
    await tester.sendKeyEvent(LogicalKeyboardKey.altLeft);
    await tester.pumpAndSettle();
    expect(tester.element(icon), same(original));
    expect(
      find.byKey(const ValueKey('mac-launchpad-delete-badge')),
      findsNothing,
    );
    await tester.sendKeyEvent(LogicalKeyboardKey.altLeft);
    await tester.pumpAndSettle();
    expect(tester.element(icon), same(original));
    expect(
      find.byKey(const ValueKey('mac-launchpad-delete-badge')),
      findsWidgets,
    );
    expect(tester.binding.transientCallbackCount, 0);
    controller.closePanel();
    await tester.pumpAndSettle();
    expect(tester.element(icon), same(original));
    expect(
      find.byKey(const ValueKey('mac-launchpad-delete-badge')),
      findsNothing,
    );
    expect(tester.binding.transientCallbackCount, 0);
    expect(tester.takeException(), isNull);
  });
}
