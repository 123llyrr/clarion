// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:collection';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/mac_launchpad_layout.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/ui/mac_launcher_panel.dart';

class _Catalogue extends ListBase<DesktopApp> {
  _Catalogue(Iterable<DesktopApp> entries)
    : _entries = List.unmodifiable(entries);
  final List<DesktopApp> _entries;
  int reads = 0;
  @override
  int get length => _entries.length;
  @override
  set length(int value) => throw UnsupportedError('immutable catalogue');
  @override
  DesktopApp operator [](int index) {
    reads++;
    return _entries[index];
  }

  @override
  void operator []=(int index, DesktopApp app) =>
      throw UnsupportedError('immutable catalogue');
}

DesktopApp _app(int id, {String? name, List<String> keywords = const []}) =>
    DesktopApp(
      id: 'app-$id',
      name: name ?? 'Application $id',
      desktopFile: '',
      keywords: keywords,
    );

class _Controller extends ShellController {
  _Controller(this.catalogue)
    : super(
        demo: true,
        tray: false,
        notifications: false,
        powerProfilesEnabled: false,
      ) {
    setShellStyle(ShellStyle.macos);
    setReduceMotion(true);
    selectPanel(ShellPanel.launcher);
  }
  _Catalogue catalogue;
  final launched = <DesktopApp>[];
  @override
  List<DesktopApp> get apps => catalogue;
  @override
  Future<void> launchApp(DesktopApp app) async => launched.add(app);
  void replace(_Catalogue next) {
    catalogue = next;
    notifyListeners();
  }
}

Future<void> _show(
  WidgetTester tester,
  _Controller controller,
  MacLaunchpadLayout layout,
) async {
  tester.view.physicalSize = const Size(1366, 900);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  await tester.pumpWidget(
    MaterialApp(
      home: MacLauncherPanel(controller: controller, layout: layout),
    ),
  );
  await tester.pumpAndSettle();
}

void main() {
  testWidgets('search, paging and folders reuse the large catalogue index', (
    tester,
  ) async {
    final catalogue = _Catalogue([for (var i = 0; i < 1000; i++) _app(i)]);
    final controller = _Controller(catalogue);
    final layout = MacLaunchpadLayout(persist: false);
    addTearDown(controller.dispose);
    addTearDown(layout.dispose);
    await layout.load(controller.apps.map((app) => app.id));
    layout.group('app-0', 'app-1');
    final folder = layout.entries.first;
    catalogue.reads = 0;
    await _show(tester, controller, layout);
    final initialReads = catalogue.reads;
    catalogue.reads = 0;
    await tester.tap(find.byKey(const ValueKey('mac-launchpad-dot-1')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('mac-launchpad-dot-0')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(ValueKey('mac-launchpad-folder-${folder.id}')));
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('app-app-0')), findsOneWidget);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    await tester.enterText(
      find.byKey(const ValueKey('mac-launchpad-search')),
      'Application 999',
    );
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('app-app-999')), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('mac-launchpad-clear-search')));
    await tester.pumpAndSettle();
    expect(
      find.byKey(ValueKey('mac-launchpad-folder-${folder.id}')),
      findsOneWidget,
    );
    debugPrint(
      'Launchpad catalogue element reads: initial=$initialReads, navigation=${catalogue.reads}',
    );
    expect(initialReads, lessThanOrEqualTo(3 * catalogue.length));
    expect(catalogue.reads, 0);
    expect(controller.launched, isEmpty);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'a new catalogue refreshes open folders, search and launch targets',
    (tester) async {
      final controller = _Controller(
        _Catalogue([for (var i = 0; i < 6; i++) _app(i)]),
      );
      final layout = MacLaunchpadLayout(persist: false);
      addTearDown(controller.dispose);
      addTearDown(layout.dispose);
      await layout.load(controller.apps.map((app) => app.id));
      layout.group('app-0', 'app-1');
      final folder = layout.entries.first;
      await _show(tester, controller, layout);
      await tester.tap(
        find.byKey(ValueKey('mac-launchpad-folder-${folder.id}')),
      );
      await tester.pumpAndSettle();
      final updated = _app(
        1,
        name: 'Updated application',
        keywords: ['newkeyword'],
      );
      controller.replace(_Catalogue([updated, _app(2), _app(6)]));
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('app-app-0')), findsNothing);
      expect(find.text('Updated application'), findsOneWidget);
      expect(layout.find(folder.id)!.appIds, ['app-1']);
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('app-app-6')), findsOneWidget);
      await tester.enterText(
        find.byKey(const ValueKey('mac-launchpad-search')),
        'newkeyword',
      );
      await tester.pumpAndSettle();
      expect(find.text('Updated application'), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('app-app-1')));
      await tester.pump();
      expect(controller.launched.single, same(updated));
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'a replacement controller rebuilds the retained panel catalogue',
    (tester) async {
      final old = _Controller(_Catalogue([_app(0), _app(1)]));
      final current = _Controller(_Catalogue([_app(2), _app(3)]));
      final layout = MacLaunchpadLayout(persist: false);
      addTearDown(old.dispose);
      addTearDown(current.dispose);
      addTearDown(layout.dispose);
      await layout.load(old.apps.map((app) => app.id));
      await _show(tester, old, layout);
      await _show(tester, current, layout);
      expect(find.byKey(const ValueKey('app-app-0')), findsNothing);
      expect(find.byKey(const ValueKey('app-app-2')), findsOneWidget);
      old.replace(_Catalogue([_app(4)]));
      await tester.pump();
      expect(find.byKey(const ValueKey('app-app-4')), findsNothing);
      await tester.tap(find.byKey(const ValueKey('app-app-2')));
      await tester.pump();
      expect(current.launched.single, same(current.apps.first));
      expect(old.launched, isEmpty);
      expect(tester.takeException(), isNull);
    },
  );
}
