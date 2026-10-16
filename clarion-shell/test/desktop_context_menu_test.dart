// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/desktop/desktop_context_menu.dart';

const _items = <DesktopContextMenuItem<String>>[
  DesktopContextMenuItem(
    key: ValueKey('disabled'),
    label: 'Unavailable',
    enabled: false,
  ),
  DesktopContextMenuItem.divider(),
  DesktopContextMenuItem(
    key: ValueKey('a'),
    label: 'Sort by',
    children: [
      DesktopContextMenuItem(label: 'Unavailable child', enabled: false),
      DesktopContextMenuItem.divider(),
      DesktopContextMenuItem(
        key: ValueKey('a1'),
        label: 'Name',
        value: 'name',
        checked: true,
      ),
      DesktopContextMenuItem(key: ValueKey('a2'), label: 'Date', value: 'date'),
    ],
  ),
  DesktopContextMenuItem(
    key: ValueKey('b'),
    label: 'Group by',
    children: [
      DesktopContextMenuItem(key: ValueKey('b1'), label: 'Kind', value: 'kind'),
    ],
  ),
  DesktopContextMenuItem(
    key: ValueKey('plain'),
    label: 'New folder',
    value: 'folder',
    shortcut: '⇧⌘N',
  ),
];

class _Observer extends NavigatorObserver {
  int menusPushed = 0;
  int routesPopped = 0;

  @override
  void didPush(Route<dynamic> route, Route<dynamic>? previousRoute) {
    if (route is PopupRoute) menusPushed++;
  }

  @override
  void didPop(Route<dynamic> route, Route<dynamic>? previousRoute) {
    routesPopped++;
  }
}

class _Harness {
  final observer = _Observer();
  final results = <String?>[];
  int underlyingTaps = 0;
}

Future<_Harness> _open(
  WidgetTester tester, {
  List<DesktopContextMenuItem<String>> items = _items,
  Offset position = const Offset(80, 80),
  bool reduceMotion = true,
  Size size = const Size(800, 600),
}) async {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  final harness = _Harness();
  await tester.pumpWidget(
    MaterialApp(
      navigatorObservers: [harness.observer],
      home: Scaffold(
        body: Builder(
          builder: (context) {
            return Stack(
              children: [
                Positioned(
                  left: 0,
                  top: 0,
                  child: TextButton(
                    key: const ValueKey('open'),
                    onPressed: () {
                      showDesktopContextMenu<String>(
                        context: context,
                        position: position,
                        items: items,
                        reduceMotion: reduceMotion,
                      ).then(harness.results.add);
                    },
                    child: const Text('Open'),
                  ),
                ),
                Positioned(
                  right: 0,
                  bottom: 0,
                  child: TextButton(
                    key: const ValueKey('underlying'),
                    onPressed: () => harness.underlyingTaps++,
                    child: const Text('Underlying'),
                  ),
                ),
              ],
            );
          },
        ),
      ),
    ),
  );
  await tester.tap(find.byKey(const ValueKey('open')));
  await tester.pumpAndSettle();
  return harness;
}

Future<TestGesture> _mouseAt(WidgetTester tester, String key) async {
  final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
  await mouse.addPointer(location: Offset.zero);
  addTearDown(mouse.removePointer);
  await mouse.moveTo(tester.getCenter(find.byKey(ValueKey(key))));
  await tester.pump();
  return mouse;
}

void main() {
  testWidgets('hover delay and parent switching share one route', (
    tester,
  ) async {
    final harness = await _open(tester);
    final mouse = await _mouseAt(tester, 'a');
    await tester.pump(const Duration(milliseconds: 179));
    expect(find.byKey(const ValueKey('a1')), findsNothing);
    await tester.pump(const Duration(milliseconds: 2));
    expect(find.byKey(const ValueKey('a1')), findsOneWidget);
    expect(harness.observer.menusPushed, 1);

    // A child menu must not place a modal barrier over its parent.
    await mouse.moveTo(tester.getCenter(find.byKey(const ValueKey('b'))));
    await tester.pump();
    expect(find.byKey(const ValueKey('a1')), findsNothing);
    await tester.pump(const Duration(milliseconds: 181));
    expect(find.byKey(const ValueKey('b1')), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('b1')));
    await tester.pumpAndSettle();
    expect(harness.results, ['kind']);
    expect(harness.observer.routesPopped, 1);
  });

  testWidgets(
    'hovering a parent leaf closes child and keeps parent clickable',
    (tester) async {
      final harness = await _open(tester);
      final mouse = await _mouseAt(tester, 'a');
      await tester.pump(const Duration(milliseconds: 181));
      await mouse.moveTo(tester.getCenter(find.byKey(const ValueKey('plain'))));
      await tester.pump();
      expect(find.byKey(const ValueKey('a1')), findsNothing);
      await tester.tap(find.byKey(const ValueKey('plain')));
      await tester.pumpAndSettle();
      expect(harness.results, ['folder']);
    },
  );

  testWidgets('Right then immediate Left restores parent keyboard navigation', (
    tester,
  ) async {
    final harness = await _open(tester);
    await _mouseAt(tester, 'a');
    // No hover delay, child hover, Down, or frame between Right and Left.
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowRight);
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowRight);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pumpAndSettle();
    expect(harness.results, ['kind']);
    expect(harness.observer.menusPushed, 1);
  });

  testWidgets(
    'keyboard wraps and skips disabled entries and dividers at every level',
    (tester) async {
      final harness = await _open(tester);
      for (final key in [
        LogicalKeyboardKey.home,
        LogicalKeyboardKey.arrowUp,
        LogicalKeyboardKey.arrowDown,
        LogicalKeyboardKey.arrowRight,
        LogicalKeyboardKey.end,
        LogicalKeyboardKey.arrowUp,
        LogicalKeyboardKey.enter,
      ]) {
        await tester.sendKeyEvent(key);
      }
      await tester.pumpAndSettle();
      expect(harness.results, ['name']);
    },
  );

  testWidgets(
    'Escape closes the entire cascade once during reverse animation',
    (tester) async {
      final harness = await _open(tester, reduceMotion: false);
      await _mouseAt(tester, 'a');
      await tester.sendKeyEvent(LogicalKeyboardKey.arrowRight);
      await tester.pump();
      expect(find.byKey(const ValueKey('a1')), findsOneWidget);
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      expect(harness.results, [null]);
      expect(harness.observer.routesPopped, 1);
      expect(find.byKey(const ValueKey('open')), findsOneWidget);
    },
  );

  testWidgets('outside click dismisses without activating underlying content', (
    tester,
  ) async {
    final harness = await _open(tester);
    await _mouseAt(tester, 'a');
    await tester.tapAt(
      tester.getCenter(find.byKey(const ValueKey('underlying'))),
    );
    await tester.pumpAndSettle();
    await tester.pump(const Duration(milliseconds: 300));
    expect(harness.results, [null]);
    expect(harness.underlyingTaps, 0);
    expect(harness.observer.menusPushed, 1);
    expect(find.byKey(const ValueKey('a1')), findsNothing);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'disabled rows stay open and checked rows retain compact geometry',
    (tester) async {
      final harness = await _open(tester);
      await tester.tap(find.byKey(const ValueKey('disabled')));
      await tester.pump();
      expect(harness.results, isEmpty);
      expect(tester.getSize(find.byKey(const ValueKey('plain'))).height, 26);
      expect(find.text('⇧⌘N'), findsOneWidget);
      await _mouseAt(tester, 'a');
      await tester.pump(const Duration(milliseconds: 181));
      expect(
        find.descendant(
          of: find.byKey(const ValueKey('a1')),
          matching: find.byIcon(Icons.check),
        ),
        findsOneWidget,
      );
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
    },
  );

  testWidgets(
    'near right and bottom edges cascade opens left within viewport',
    (tester) async {
      await _open(tester, position: const Offset(795, 595));
      await _mouseAt(tester, 'a');
      await tester.pump(const Duration(milliseconds: 181));
      final parent = tester.getRect(
        find.byKey(const ValueKey('desktop-context-menu-0')),
      );
      final child = tester.getRect(
        find.byKey(const ValueKey('desktop-context-menu-1')),
      );
      expect(child.left, lessThan(parent.left));
      for (final rect in [parent, child]) {
        expect(rect.left, greaterThanOrEqualTo(8));
        expect(rect.top, greaterThanOrEqualTo(8));
        expect(rect.right, lessThanOrEqualTo(792));
        expect(rect.bottom, lessThanOrEqualTo(592));
      }
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
    },
  );

  testWidgets('short viewport scrolls keyboard selection into view', (
    tester,
  ) async {
    final harness = await _open(
      tester,
      size: const Size(240, 120),
      items: List.generate(
        100,
        (i) => DesktopContextMenuItem(
          key: ValueKey('row-$i'),
          label: 'Entry $i',
          value: '$i',
        ),
      ),
    );
    await tester.sendKeyEvent(LogicalKeyboardKey.end);
    await tester.pump();
    final row = tester.getRect(find.byKey(const ValueKey('row-99')));
    expect(row.top, greaterThanOrEqualTo(8));
    expect(row.bottom, lessThanOrEqualTo(112));
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pumpAndSettle();
    expect(harness.results, ['99']);
    expect(tester.takeException(), isNull);
  });

  testWidgets('long localized labels and shortcuts fit a narrow viewport', (
    tester,
  ) async {
    final harness = await _open(
      tester,
      size: const Size(190, 180),
      items: [
        DesktopContextMenuItem(
          key: const ValueKey('long'),
          label: List.filled(30, 'Very long label').join(),
          shortcut: List.filled(30, '⇧⌘N').join(),
          value: 'long',
        ),
      ],
    );
    expect(tester.takeException(), isNull);
    await tester.tap(find.byKey(const ValueKey('long')));
    await tester.pumpAndSettle();
    expect(harness.results, ['long']);
  });
}
