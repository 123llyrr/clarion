// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:realand_shell/app_menu_models.dart';
import 'package:realand_shell/models.dart';
import 'package:realand_shell/services/global_app_menu_service.dart';
import 'package:realand_shell/state/shell_controller.dart';
import 'package:realand_shell/ui/global_app_menu_bar.dart';
import 'package:realand_shell/ui/shell_view.dart';

class _Menus extends GlobalAppMenuService {
  _Menus({bool loaded = true}) : super(hostRegistrar: false) {
    if (loaded) _items = _exported;
  }

  final action = GlobalMenuItem(id: 'verify', label: 'Verify');
  final disabled = GlobalMenuItem(
    id: 'disabled',
    label: 'Disabled',
    enabled: false,
  );
  late final _exported = <GlobalMenuItem>[
    GlobalMenuItem(id: 'file', label: 'File', children: [action, disabled]),
  ];
  List<GlobalMenuItem> _items = const [];
  int _revision = 0;
  final activated = <String>[];
  final submenuRequests = <(String, bool)>[];
  Completer<bool>? activationGate;

  @override
  List<GlobalMenuItem> get menus => _items;
  @override
  bool get available => _items.isNotEmpty;
  @override
  int get revision => _revision;

  void publish() {
    _items = List.of(_exported);
    _revision++;
    notifyListeners();
  }

  void clear() {
    _items = const [];
    _revision++;
    notifyListeners();
  }

  @override
  Future<bool> activate(GlobalMenuItem item) async {
    if (!identical(item, action)) return false;
    activated.add(item.id);
    return activationGate == null ? true : await activationGate!.future;
  }

  @override
  Future<void> submenu(GlobalMenuItem item, {required bool opened}) async {
    submenuRequests.add((item.id, opened));
  }
}

// Isolate bar presentation from IPC discovery. The real canonical target guard
// is covered by the controller and private Qt integration fixtures.
class _Controller extends ShellController {
  _Controller(_Menus menus)
    : super(
        demo: true,
        appMenuService: menus,
        notifications: false,
        tray: false,
        powerProfilesEnabled: false,
        restoreWallpaperOnStart: false,
      );

  bool canonicalFocused = true;
  @override
  bool get materialAppMenuAvailable => canonicalFocused && appMenu.available;

  void setCanonicalFocused(bool value) {
    canonicalFocused = value;
    notifyListeners();
  }
}

const _entry = ValueKey('material-app-menu-entry');

void _view(WidgetTester tester, Size size) {
  tester.view.devicePixelRatio = 1;
  tester.view.physicalSize = size;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
}

Future<void> _openFile(WidgetTester tester) async {
  await tester.tap(find.byKey(_entry));
  await tester.pumpAndSettle();
  await tester.tap(find.text('File'));
  await tester.pumpAndSettle();
}

Future<void> _bar(
  WidgetTester tester,
  _Menus service,
  MenuController controller,
  List<bool> opened,
) => tester.pumpWidget(
  MaterialApp(
    home: Scaffold(
      body: Align(
        alignment: Alignment.topLeft,
        child: SizedBox(
          width: 36,
          child: GlobalAppMenuBar(
            service: service,
            controller: controller,
            compact: true,
            barHeight: 40,
            onOpenChanged: opened.add,
          ),
        ),
      ),
    ),
  ),
);

void main() {
  testWidgets(
    'compact entry groups actual menus and survives native 40px expansion',
    (tester) async {
      _view(tester, const Size(800, 40));
      final service = _Menus();
      addTearDown(service.dispose);
      final menu = MenuController();
      final opened = <bool>[];
      await _bar(tester, service, menu, opened);
      expect(tester.getSize(find.byKey(_entry)), const Size(36, 40));
      expect(find.text('File'), findsNothing);
      expect(
        find.byWidgetPredicate(
          (widget) => widget is Semantics && widget.properties.label == '应用菜单',
        ),
        findsOneWidget,
      );

      await tester.tap(find.byKey(_entry));
      await tester.pump();
      expect(menu.isOpen, isTrue);
      expect(opened, [true]);
      // The local grouping entry never pretends to be an exported submenu.
      expect(service.submenuRequests, isEmpty);

      tester.view.physicalSize = const Size(800, 600);
      await tester.pumpAndSettle();
      expect(menu.isOpen, isTrue);
      expect(opened, [true]);
      await tester.tap(find.text('File'));
      await tester.pumpAndSettle();
      expect(service.submenuRequests, contains(('file', true)));
      expect(tester.getCenter(find.text('Verify')).dy, greaterThan(40));
      final disabled = tester.widget<MenuItemButton>(
        find.byKey(const ValueKey('global-menu-disabled')),
      );
      expect(disabled.onPressed, isNull);
      await tester.tap(find.text('Verify'));
      await tester.pumpAndSettle();
      expect(service.activated, ['verify']);
      expect(menu.isOpen, isFalse);
      expect(opened, [true, false]);

      await _openFile(tester);
      tester.view.physicalSize = const Size(640, 600);
      await tester.pumpAndSettle();
      expect(menu.isOpen, isFalse);
      expect(opened, [true, false, true, false]);
      expect(service.activated, ['verify']);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets('late action cannot close a refreshed compact menu', (
    tester,
  ) async {
    _view(tester, const Size(800, 600));
    final service = _Menus();
    addTearDown(service.dispose);
    final menu = MenuController();
    final opened = <bool>[];
    await _bar(tester, service, menu, opened);
    await _openFile(tester);
    final gate = service.activationGate = Completer<bool>();
    await tester.tap(find.text('Verify'));
    await tester.pump();
    expect(service.activated, ['verify']);
    expect(menu.isOpen, isTrue);

    service.publish();
    await tester.pumpAndSettle();
    expect(menu.isOpen, isFalse);
    await _openFile(tester);
    gate.complete(true);
    await tester.pumpAndSettle();
    expect(menu.isOpen, isTrue);
    expect(find.text('Verify'), findsOneWidget);
    expect(opened, [true, false, true]);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  testWidgets(
    'Material tracks async availability, closes outside and Escape, and fits either edge',
    (tester) async {
      _view(tester, const Size(800, 600));
      final service = _Menus(loaded: false);
      final controller = _Controller(service);
      addTearDown(controller.dispose);
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(body: ShellView(controller: controller)),
        ),
      );
      expect(find.byKey(_entry), findsNothing);
      // A service-only notification must reach the bar's selector: loading a
      // menu does not change desktopRevision or a panel/system status field.
      final desktopRevision = controller.desktopRevision;
      service.publish();
      await tester.pumpAndSettle();
      expect(controller.desktopRevision, desktopRevision);
      expect(find.byKey(_entry), findsOneWidget);
      expect(tester.getRect(find.byKey(_entry)).top, 0);

      await _openFile(tester);
      expect(controller.appMenuOpen, isTrue);
      await tester.tapAt(const Offset(700, 500));
      await tester.pumpAndSettle();
      expect(controller.appMenuOpen, isFalse);
      expect(find.text('Verify'), findsNothing);
      await _openFile(tester);
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      expect(controller.appMenuOpen, isFalse);
      expect(find.text('Verify'), findsNothing);

      controller.setBarPosition(BarPosition.bottom);
      await tester.pumpAndSettle();
      expect(tester.getRect(find.byKey(_entry)).bottom, 600);
      for (final width in [240.0, 160.0]) {
        tester.view.physicalSize = Size(width, 600);
        await tester.pumpAndSettle();
        expect(tester.getSize(find.byKey(_entry)), const Size(36, 40));
        expect(tester.takeException(), isNull);
      }

      controller.setCanonicalFocused(false);
      await tester.pumpAndSettle();
      expect(find.byKey(_entry), findsNothing);
      controller.setCanonicalFocused(true);
      await tester.pumpAndSettle();
      expect(find.byKey(_entry), findsOneWidget);
      service.clear();
      await tester.pumpAndSettle();
      expect(find.byKey(_entry), findsNothing);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );
}
