// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/ui/quick_settings_panel.dart';

void main() {
  for (final width in [520.0, 260.0]) {
    testWidgets('shell styles remain selectable at width $width', (
      tester,
    ) async {
      final controller = ShellController(demo: true);
      addTearDown(controller.dispose);
      controller.setBarPosition(BarPosition.bottom);
      controller.selectPanel(ShellPanel.quickSettings);
      await tester.binding.setSurfaceSize(Size(width, 1200));
      addTearDown(() => tester.binding.setSurfaceSize(null));
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
      final style = find.byKey(const ValueKey('shell-style'));
      await tester.ensureVisible(style);
      if (width < 320) {
        await tester.tap(style);
        await tester.pumpAndSettle();
        await tester.tap(find.text('macOS').last);
      } else {
        await tester.tap(
          find.descendant(of: style, matching: find.text('macOS')),
        );
      }
      await tester.pumpAndSettle();
      expect(controller.shellStyle, ShellStyle.macos);
      expect(controller.panel, ShellPanel.none);
      expect(controller.barPosition, BarPosition.bottom);
      final position = find.byKey(const ValueKey('bar-position'));
      await tester.ensureVisible(position);
      if (width < 320) {
        expect(
          tester.widget<DropdownButton<BarPosition>>(position).onChanged,
          isNull,
        );
      } else {
        expect(
          tester
              .widget<SegmentedButton<BarPosition>>(position)
              .onSelectionChanged,
          isNull,
        );
      }
      controller.setShellStyle(ShellStyle.material);
      await tester.pumpAndSettle();
      expect(controller.barPosition, BarPosition.bottom);
      if (width < 320) {
        expect(
          tester.widget<DropdownButton<BarPosition>>(position).onChanged,
          isNotNull,
        );
      } else {
        expect(
          tester
              .widget<SegmentedButton<BarPosition>>(position)
              .onSelectionChanged,
          isNotNull,
        );
      }
      expect(tester.takeException(), isNull);
    });
  }
}
