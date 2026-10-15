// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:realand_shell/main.dart';
import 'package:realand_shell/models.dart';
import 'package:realand_shell/preferences.dart';
import 'package:realand_shell/state/shell_controller.dart';
import 'package:realand_shell/theme/shell_theme.dart';
import 'package:realand_shell/ui/shell_view.dart';

const _channel = MethodChannel('realand.shell/window');
Finder _key(String value) => find.byKey(ValueKey(value));
ShellController _controller() => ShellController(
  demo: true,
  tray: false,
  notifications: false,
  powerProfilesEnabled: false,
)..setReduceMotion(true);

Future<void> _mount(
  WidgetTester tester,
  ShellController controller,
  Size size,
) async {
  tester.view.devicePixelRatio = 1;
  tester.view.physicalSize = size;
  addTearDown(tester.view.resetDevicePixelRatio);
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(controller.dispose);
  await tester.pumpWidget(
    MaterialApp(
      theme: buildShellTheme(Brightness.dark),
      home: ShellView(controller: controller),
    ),
  );
  await tester.pumpAndSettle();
}

void main() {
  for (final size in [const Size(1280, 800), const Size(320, 300)]) {
    testWidgets('bar and all panel layouts follow both edges at $size', (
      tester,
    ) async {
      final controller = _controller();
      await _mount(tester, controller, size);
      final original = tester.element(_key('bar-launcher'));
      for (final position in BarPosition.values) {
        controller.setBarPosition(position);
        await tester.pumpAndSettle();
        final bar = tester.getRect(_key('shell-system-bar'));
        expect(bar.height, 40);
        expect(bar.top, position == BarPosition.top ? 0 : size.height - 40);
        expect(tester.element(_key('bar-launcher')), same(original));
        for (final panel in ShellPanel.values.where(
          (p) => p != ShellPanel.none,
        )) {
          if (panel == ShellPanel.dockAppMenu) {
            controller.openDockAppMenu(controller.apps.first.id);
          } else {
            controller.selectPanel(panel);
          }
          await tester.pumpAndSettle();
          final rect = tester.getRect(
            _key(
              panel == ShellPanel.launcher
                  ? 'launcher-surface-position'
                  : 'shell-panel-position',
            ),
          );
          if (position == BarPosition.top) {
            expect(rect.top, greaterThanOrEqualTo(40));
            expect(rect.bottom, lessThanOrEqualTo(size.height));
          } else {
            expect(rect.top, greaterThanOrEqualTo(0));
            expect(rect.bottom, lessThanOrEqualTo(size.height - 40));
          }
          expect(tester.element(_key('bar-launcher')), same(original));
          controller.closePanel();
          await tester.pumpAndSettle();
        }
      }
      expect(tester.takeException(), isNull);
    });
  }

  testWidgets(
    'bottom hints align above the bar and changing edges dismisses them',
    (tester) async {
      final controller = _controller()..setBarPosition(BarPosition.bottom);
      await _mount(tester, controller, const Size(1280, 128));
      controller.showBarTooltip(Object(), '提示', const Offset(1000, 108));
      await tester.pumpAndSettle();
      final bar = tester.getRect(_key('shell-system-bar'));
      final hint = tester.getRect(_key('bar-tooltip-bubble'));
      expect(bar.top, 88);
      expect(hint.bottom, 80);
      expect(hint.bottom, lessThan(bar.top));
      controller.setBarPosition(BarPosition.top);
      await tester.pumpAndSettle();
      expect(_key('bar-tooltip-bubble'), findsNothing);
      controller.showBarTooltip(Object(), '顶部提示', const Offset(1000, 20));
      await tester.pumpAndSettle();
      expect(tester.getRect(_key('bar-tooltip-bubble')).top, 48);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'latest bar edge wins a delayed native reply without closing the panel',
    (tester) async {
      final controller = _controller();
      final hold = Completer<void>();
      var position = 'top';
      var expanded = false;
      var holdBottom = true;
      var waiting = false;
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(_channel, (call) async {
            if (call.method == 'setBarPosition') {
              final requested = call.arguments as String;
              if (holdBottom && requested == 'bottom') {
                holdBottom = false;
                waiting = true;
                await hold.future;
              }
              position = requested;
            } else if (call.method == 'setExpanded') {
              expanded = call.arguments as bool;
            }
            return null;
          });
      addTearDown(() {
        if (!hold.isCompleted) hold.complete();
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
            .setMockMethodCallHandler(_channel, null);
      });
      tester.view.devicePixelRatio = 1;
      tester.view.physicalSize = const Size(1280, 800);
      addTearDown(tester.view.resetDevicePixelRatio);
      addTearDown(tester.view.resetPhysicalSize);
      await tester.pumpWidget(
        ShellApplication(
          controller: controller,
          preferences: const ShellPreferences(),
          persist: false,
        ),
      );
      for (var i = 0; i < 8; i++) {
        await tester.pump();
      }
      controller.selectPanel(ShellPanel.quickSettings);
      controller.setBarPosition(BarPosition.bottom);
      for (var i = 0; i < 8; i++) {
        await tester.pump();
      }
      expect(waiting, isTrue);
      controller.setBarPosition(BarPosition.top);
      hold.complete();
      for (var i = 0; i < 12; i++) {
        await tester.pump();
      }
      await tester.pumpAndSettle();
      expect(position, 'top');
      expect(expanded, isTrue);
      expect(controller.panel, ShellPanel.quickSettings);
      expect(tester.getRect(_key('shell-system-bar')).top, 0);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );
}
