// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/ui/tray_icon.dart';

const _item = TrayItem(id: 'a', title: 'A');
const _region = ValueKey('tray-pointer-target');

class _Controller extends ShellController {
  _Controller()
    : super(
        demo: true,
        notifications: false,
        tray: false,
        mediaEnabled: false,
        powerProfilesEnabled: false,
      );

  final scrolls = <(String, int, String)>[];
  final primary = <String>[];
  final primaryButtons = <int>[];
  final secondary = <String>[];
  final menus = <(String, int)>[];
  bool _nativeOpen = false;
  TrayItem? _selected;
  int tooltipShows = 0;
  TrayItem buttonItem = _item;

  void updateMetadata() {
    buttonItem = const TrayItem(id: 'a', title: 'Updated title');
    notifyListeners();
  }

  @override
  bool get nativeTrayMenuOpen => _nativeOpen;
  @override
  TrayItem? get selectedTrayItem => _selected;

  void setNativeMenuOpen(bool open) {
    _nativeOpen = open;
    _selected = open ? _item : null;
    notifyListeners();
  }

  @override
  void showBarTooltip(Object owner, String message, Offset anchor) {
    tooltipShows++;
    super.showBarTooltip(owner, message, anchor);
  }

  @override
  Future<void> scrollTrayItem(
    TrayItem item,
    int delta, {
    String orientation = 'vertical',
  }) async => scrolls.add((item.id, delta, orientation));

  @override
  Future<void> activateTrayItem(
    TrayItem item, {
    double x = 0,
    double y = 0,
    int menuButton = 1,
  }) async {
    primary.add(item.id);
    primaryButtons.add(menuButton);
  }

  @override
  Future<void> secondaryActivateTrayItem(
    TrayItem item, {
    double x = 0,
    double y = 0,
  }) async => secondary.add(item.id);

  @override
  Future<void> openTrayMenu(
    TrayItem item, {
    double x = 0,
    double y = 0,
    int button = 3,
  }) async => menus.add((item.id, button));
}

_Controller _controller() {
  final controller = _Controller();
  addTearDown(controller.dispose);
  return controller;
}

Future<void> _mountRegion(
  WidgetTester tester,
  _Controller controller, {
  TrayItem item = _item,
  bool forwardWheel = true,
  Widget? child,
}) => tester.pumpWidget(
  MaterialApp(
    home: Center(
      child: TrayPointerRegion(
        controller: controller,
        item: item,
        forwardWheel: forwardWheel,
        child: SizedBox(
          key: _region,
          width: 80,
          height: 80,
          child: child ?? const ColoredBox(color: Colors.blue),
        ),
      ),
    ),
  ),
);

Future<void> _scroll(WidgetTester tester, Offset delta) =>
    tester.sendEventToBinding(
      PointerScrollEvent(
        position: tester.getCenter(find.byKey(_region)),
        scrollDelta: delta,
      ),
    );

Future<void> _mountButton(WidgetTester tester, _Controller controller) async {
  await tester.pumpWidget(
    MaterialApp(
      home: Center(
        child: ListenableBuilder(
          listenable: controller,
          builder: (_, _) => TrayBarButton(
            controller: controller,
            item: controller.buttonItem,
          ),
        ),
      ),
    ),
  );
  await tester.pump();
}

void main() {
  testWidgets(
    'sub-unit touchpad deltas accumulate without losing either axis',
    (tester) async {
      final controller = _controller();
      await _mountRegion(tester, controller);
      for (var i = 0; i < 9; i++) {
        await _scroll(tester, const Offset(.1, -.1));
      }
      expect(controller.scrolls, isEmpty);
      await _scroll(tester, const Offset(.1, -.1));
      expect(controller.scrolls, [
        ('a', 1, 'horizontal'),
        ('a', -1, 'vertical'),
      ]);
    },
  );

  testWidgets('reversing fractional movement cancels before integer dispatch', (
    tester,
  ) async {
    final controller = _controller();
    await _mountRegion(tester, controller);
    for (final dy in [.75, -.5, .25, -.5, -.6]) {
      await _scroll(tester, Offset(0, dy));
    }
    expect(controller.scrolls, isEmpty);
    await _scroll(tester, const Offset(0, -.4));
    expect(controller.scrolls, [('a', -1, 'vertical')]);
  });

  testWidgets(
    'diagonal scrolling preserves integer amounts and both residuals',
    (tester) async {
      final controller = _controller();
      await _mountRegion(tester, controller);
      await _scroll(tester, const Offset(1.25, -2.75));
      await _scroll(tester, const Offset(.75, -.25));
      expect(controller.scrolls, [
        ('a', 1, 'horizontal'),
        ('a', -2, 'vertical'),
        ('a', 1, 'horizontal'),
        ('a', -1, 'vertical'),
      ]);
    },
  );

  testWidgets(
    'metadata preserves residuals but item and disabled changes clear them',
    (tester) async {
      final controller = _controller();
      await _mountRegion(tester, controller);
      await _scroll(tester, const Offset(0, .8));
      await _mountRegion(
        tester,
        controller,
        item: const TrayItem(id: 'a', title: 'New title'),
      );
      await _scroll(tester, const Offset(0, .2));
      expect(controller.scrolls, [('a', 1, 'vertical')]);
      const other = TrayItem(id: 'b', title: 'B');
      await _scroll(tester, const Offset(0, .8));
      await _mountRegion(tester, controller, item: other);
      await _scroll(tester, const Offset(0, .6));
      await _mountRegion(tester, controller, item: other, forwardWheel: false);
      await _scroll(tester, const Offset(0, 4));
      await _mountRegion(tester, controller, item: other);
      await _scroll(tester, const Offset(0, .4));
      expect(controller.scrolls.length, 1);
      await _scroll(tester, const Offset(0, .6));
      expect(controller.scrolls.last, ('b', 1, 'vertical'));
    },
  );

  testWidgets('a child winning pointer resolution does not leak residuals', (
    tester,
  ) async {
    final controller = _controller();
    var childConsumes = true;
    await _mountRegion(
      tester,
      controller,
      child: Listener(
        behavior: HitTestBehavior.opaque,
        onPointerSignal: (event) {
          if (childConsumes) {
            GestureBinding.instance.pointerSignalResolver.register(
              event,
              (_) {},
            );
          }
        },
      ),
    );
    await _scroll(tester, const Offset(0, .75));
    childConsumes = false;
    await _scroll(tester, const Offset(0, .25));
    expect(controller.scrolls, isEmpty);
    await _scroll(tester, const Offset(0, .75));
    expect(controller.scrolls, [('a', 1, 'vertical')]);
  });

  testWidgets('middle and right click do not activate the primary action', (
    tester,
  ) async {
    final controller = _controller();
    await _mountButton(tester, controller);
    final point = tester.getCenter(find.byType(TrayBarButton));
    for (final button in [kTertiaryButton, kSecondaryButton]) {
      final mouse = await tester.createGesture(
        kind: PointerDeviceKind.mouse,
        buttons: button,
      );
      await mouse.down(point);
      await mouse.up();
      await tester.pump();
    }
    expect(controller.primary, isEmpty);
    expect(controller.secondary, ['a']);
    expect(controller.menus, [('a', 3)]);
    await tester.longPress(find.byType(TrayBarButton));
    expect(controller.primary, isEmpty);
    expect(controller.menus.last, ('a', 1));
  });

  testWidgets('metadata rebuild between press and release keeps mouse origin', (
    tester,
  ) async {
    final controller = _controller();
    await _mountButton(tester, controller);
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.down(tester.getCenter(find.byType(TrayBarButton)));
    await tester.pump(const Duration(milliseconds: 150));
    controller.updateMetadata();
    await tester.pump();
    await mouse.up();
    await tester.pump();
    expect(controller.primary, ['a']);
    expect(controller.primaryButtons, [1]);
    expect(controller.menus, isEmpty);
    await tester.pumpWidget(const SizedBox());
    await tester.pump();
  });

  testWidgets('Enter and Space activation keep keyboard origin', (
    tester,
  ) async {
    final controller = _controller();
    await _mountButton(tester, controller);
    await tester.sendKeyEvent(LogicalKeyboardKey.tab);
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.sendKeyEvent(LogicalKeyboardKey.space);
    expect(controller.primary, ['a', 'a']);
    expect(controller.primaryButtons, [0, 0]);
    expect(controller.menus, isEmpty);
    await tester.pumpWidget(const SizedBox());
    await tester.pump();
  });

  testWidgets('keyboard menu shortcuts carry button zero without activation', (
    tester,
  ) async {
    final controller = _controller();
    await _mountButton(tester, controller);
    await tester.sendKeyEvent(LogicalKeyboardKey.tab);
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.contextMenu);
    await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.f10);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
    expect(controller.menus, [('a', 0), ('a', 0)]);
    expect(controller.primary, isEmpty);
    expect(controller.secondary, isEmpty);
  });

  testWidgets('native menu cancels a pending tooltip and prevents new ones', (
    tester,
  ) async {
    final controller = _controller();
    await _mountButton(tester, controller);
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: const Offset(2, 2));
    final point = tester.getCenter(find.byType(TrayBarButton));
    await mouse.moveTo(point);
    await tester.pump(const Duration(milliseconds: 200));
    controller.setNativeMenuOpen(true);
    await tester.pump(const Duration(milliseconds: 500));
    expect(controller.tooltipShows, 0);
    expect(controller.barTooltip, isNull);
    await mouse.moveTo(const Offset(2, 2));
    await mouse.moveTo(point);
    await tester.pump(const Duration(milliseconds: 500));
    expect(controller.tooltipShows, 0);
    final semantics = tester.widget<Semantics>(
      find.descendant(
        of: find.byType(TrayPointerRegion),
        matching: find.byWidgetPredicate(
          (widget) => widget is Semantics && widget.properties.label == 'A',
        ),
      ),
    );
    expect(semantics.properties.selected, true);
    controller.setNativeMenuOpen(false);
    await tester.pump();
    await mouse.moveTo(const Offset(2, 2));
    await mouse.moveTo(point);
    await tester.pump(const Duration(milliseconds: 400));
    expect(controller.tooltipShows, 1);
    expect(controller.barTooltip?.message, 'A');
    controller.setNativeMenuOpen(true);
    await tester.pump();
    expect(controller.barTooltip, isNull);
    await mouse.removePointer();
    await tester.pumpWidget(const SizedBox());
    await tester.pump();
  });
}
