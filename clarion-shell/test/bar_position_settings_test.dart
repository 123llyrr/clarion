// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/ui/quick_settings_panel.dart';

ShellController _controller() => ShellController(
  demo: true,
  notifications: false,
  tray: false,
  powerProfilesEnabled: false,
);

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  test('bar position changes dismiss old anchors without closing a panel', () {
    final controller = _controller();
    addTearDown(controller.dispose);
    var changes = 0;
    controller.addListener(() => changes++);
    final owner = Object();

    expect(controller.barPosition, BarPosition.top);
    controller.showBarTooltip(owner, '控制中心', const Offset(100, 20));
    expect(controller.barTooltip, isNotNull);
    changes = 0;
    controller.setBarPosition(BarPosition.bottom);
    expect(controller.barPosition, BarPosition.bottom);
    expect(controller.barTooltip, isNull);
    expect(changes, 1);
    controller.hideBarTooltip(owner);
    controller.setBarPosition(BarPosition.bottom);
    expect(changes, 1);

    controller.selectPanel(ShellPanel.quickSettings);
    changes = 0;
    controller.setBarPosition(BarPosition.top);
    expect(controller.barPosition, BarPosition.top);
    expect(controller.panel, ShellPanel.quickSettings);
    expect(changes, 1);
  });

  test('disposed controller ignores late position changes', () {
    final controller = _controller();
    controller.dispose();
    expect(
      () => controller.setBarPosition(BarPosition.bottom),
      returnsNormally,
    );
    expect(controller.barPosition, BarPosition.top);
  });

  for (final width in [520.0, 260.0]) {
    testWidgets('bar position setting works at width $width', (tester) async {
      final controller = _controller();
      addTearDown(controller.dispose);
      await tester.binding.setSurfaceSize(Size(width, 1200));
      addTearDown(() => tester.binding.setSurfaceSize(null));
      controller.selectPanel(ShellPanel.quickSettings);
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: AnimatedBuilder(
              animation: controller,
              builder: (context, _) =>
                  QuickSettingsPanel(controller: controller),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      final setting = find.byKey(const ValueKey('bar-position'));
      expect(setting, findsOneWidget);
      await tester.ensureVisible(setting);
      if (width < 320) {
        expect(tester.widget(setting), isA<DropdownButton<BarPosition>>());
        await tester.tap(setting);
        await tester.pumpAndSettle();
        await tester.tap(find.text('底部').last);
      } else {
        expect(tester.widget(setting), isA<SegmentedButton<BarPosition>>());
        await tester.tap(
          find.descendant(of: setting, matching: find.text('底部')),
        );
      }
      await tester.pumpAndSettle();
      expect(controller.barPosition, BarPosition.bottom);
      expect(controller.panel, ShellPanel.quickSettings);
      expect(tester.takeException(), isNull);

      controller.setBarPosition(BarPosition.top);
      await tester.pumpAndSettle();
      if (width < 320) {
        expect(
          tester.widget<DropdownButton<BarPosition>>(setting).value,
          BarPosition.top,
        );
      } else {
        expect(tester.widget<SegmentedButton<BarPosition>>(setting).selected, {
          BarPosition.top,
        });
      }
      expect(tester.takeException(), isNull);
    });
  }
}
