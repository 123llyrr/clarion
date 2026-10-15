// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:realand_shell/app_menu_models.dart';
import 'package:realand_shell/services/global_app_menu_service.dart';
import 'package:realand_shell/ui/global_app_menu_bar.dart';

class _Menus extends GlobalAppMenuService {
  _Menus() : super(hostRegistrar: false);
  final action = GlobalMenuItem(id: 'verify', label: 'Verify');
  late final items = [
    GlobalMenuItem(id: 'file', label: 'File', children: [action]),
  ];
  final activated = <String>[];
  @override
  List<GlobalMenuItem> get menus => items;
  @override
  bool get available => true;
  @override
  Future<bool> activate(GlobalMenuItem item) async {
    if (!identical(item, action)) return false;
    activated.add(item.id);
    return true;
  }

  @override
  Future<void> submenu(GlobalMenuItem item, {required bool opened}) async {}
}

void main() {
  for (final height in [22.0, 32.0, 48.0]) {
    testWidgets('top menu fills $height px and popup meets its bottom edge', (
      tester,
    ) async {
      await tester.binding.setSurfaceSize(const Size(800, 600));
      addTearDown(() => tester.binding.setSurfaceSize(null));
      final service = _Menus();
      addTearDown(service.dispose);
      final controller = MenuController();
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: Align(
              alignment: Alignment.topLeft,
              child: GlobalAppMenuBar(
                service: service,
                controller: controller,
                barHeight: height,
              ),
            ),
          ),
        ),
      );
      final entry = find.byKey(const ValueKey('global-menu-file'));
      final entryRect = tester.getRect(entry);
      expect(entryRect.top, 0);
      expect(entryRect.height, height);
      await tester.tapAt(Offset(entryRect.center.dx, entryRect.bottom - 2));
      await tester.pumpAndSettle();
      expect(controller.isOpen, isTrue);
      final popup = find
          .ancestor(
            of: find.byKey(const ValueKey('global-menu-verify')),
            matching: find.byType(Material),
          )
          .first;
      expect(tester.getRect(popup).top, closeTo(entryRect.bottom, .01));
      expect(
        tester
            .widget<MenuItemButton>(
              find.byKey(const ValueKey('global-menu-verify')),
            )
            .style
            ?.visualDensity,
        VisualDensity.compact,
      );
      await tester.tap(find.text('Verify'));
      await tester.pumpAndSettle();
      expect(service.activated, ['verify']);
      expect(controller.isOpen, isFalse);
      expect(tester.takeException(), isNull);
    });
  }

  testWidgets(
    'native vertical expansion preserves popup and real action hit testing; horizontal resize closes it',
    (tester) async {
      tester.view.devicePixelRatio = 1;
      tester.view.physicalSize = const Size(800, 22);
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final service = _Menus();
      addTearDown(service.dispose);
      final controller = MenuController();
      final opened = <bool>[];
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: Align(
              alignment: Alignment.topLeft,
              child: GlobalAppMenuBar(
                service: service,
                controller: controller,
                onOpenChanged: opened.add,
              ),
            ),
          ),
        ),
      );
      await tester.tap(find.text('File'));
      await tester.pump();
      expect(controller.isOpen, isTrue);
      expect(opened, [true]);

      // This is the native surface's 22px strip -> full output transition. Both
      // the real Overlay constraints and outer MediaQuery change with the view.
      tester.view.physicalSize = const Size(800, 600);
      await tester.pumpAndSettle();
      expect(controller.isOpen, isTrue);
      expect(opened, [true]);
      expect(find.text('Verify'), findsOneWidget);
      expect(tester.getCenter(find.text('Verify')).dy, greaterThan(22));
      await tester.tap(find.text('Verify'));
      await tester.pumpAndSettle();
      expect(service.activated, ['verify']);
      expect(controller.isOpen, isFalse);
      expect(opened, [true, false]);

      await tester.tap(find.text('File'));
      await tester.pumpAndSettle();
      tester.view.physicalSize = const Size(640, 600);
      await tester.pumpAndSettle();
      expect(controller.isOpen, isFalse);
      expect(opened, [true, false, true, false]);
      expect(service.activated, ['verify']);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  test('GlobalMenuItem equality and hashCode compare properties and children by value', () {
    final itemA = GlobalMenuItem(
      id: 'file',
      label: 'File',
      children: [
        GlobalMenuItem(id: 'new', label: 'New', shortcut: '⌘N'),
      ],
    );
    final itemB = GlobalMenuItem(
      id: 'file',
      label: 'File',
      children: [
        GlobalMenuItem(id: 'new', label: 'New', shortcut: '⌘N'),
      ],
    );
    final itemC = GlobalMenuItem(
      id: 'file',
      label: 'File',
      children: [
        GlobalMenuItem(id: 'open', label: 'Open', shortcut: '⌘O'),
      ],
    );
    expect(itemA, equals(itemB));
    expect(itemA.hashCode, equals(itemB.hashCode));
    expect(itemA, isNot(equals(itemC)));
  });

  testWidgets(
    'Rebuild with value-equal fallback menus keeps open menu intact without dismissal',
    (tester) async {
      final service = GlobalAppMenuService(hostRegistrar: false);
      addTearDown(service.dispose);
      final controller = MenuController();

      List<GlobalMenuItem> makeFallbacks() => [
            GlobalMenuItem(
              id: 'file',
              label: 'File',
              children: [
                GlobalMenuItem(id: 'save', label: 'Save'),
              ],
            ),
          ];

      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: Align(
              alignment: Alignment.topLeft,
              child: GlobalAppMenuBar(
                service: service,
                controller: controller,
                fallbackMenus: makeFallbacks(),
              ),
            ),
          ),
        ),
      );

      await tester.tap(find.text('File'));
      await tester.pumpAndSettle();
      expect(controller.isOpen, isTrue);
      expect(find.text('Save'), findsOneWidget);

      // Rebuild with newly instantiated but structurally identical fallback menus
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: Align(
              alignment: Alignment.topLeft,
              child: GlobalAppMenuBar(
                service: service,
                controller: controller,
                fallbackMenus: makeFallbacks(),
              ),
            ),
          ),
        ),
      );
      await tester.pump();
      expect(controller.isOpen, isTrue);
      expect(find.text('Save'), findsOneWidget);
    },
  );
}
