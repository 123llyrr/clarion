// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';
import 'package:flutter_svg/flutter_svg.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/ui/screenshot_icon.dart';

void main() {
  testWidgets('bundled vector loads at the compact bar size', (tester) async {
    await tester.pumpWidget(
      const MaterialApp(
        home: Center(
          child: IconTheme(
            data: IconThemeData(size: 16, color: Colors.white),
            child: ScreenshotIcon(),
          ),
        ),
      ),
    );
    final picture = tester.widget<SvgPicture>(find.byType(SvgPicture));
    final bytes = await tester.runAsync(
      () => picture.bytesLoader.loadBytes(
        tester.element(find.byType(SvgPicture)),
      ),
    );
    expect(bytes, isNotNull);
    expect(bytes!.lengthInBytes, greaterThan(0));
    await tester.pumpAndSettle();
    expect(tester.getSize(find.byType(ScreenshotIcon)), const Size(16, 16));
    expect(tester.takeException(), isNull);
  });

  for (final brightness in Brightness.values) {
    testWidgets(
      'capture action keeps its label and selected color in $brightness',
      (tester) async {
        final semantics = tester.ensureSemantics();
        try {
          var selected = false;
          await tester.pumpWidget(
            MaterialApp(
              theme: ThemeData(brightness: brightness),
              home: StatefulBuilder(
                builder: (context, setState) => Center(
                  child: IconButton(
                    tooltip: '截图',
                    isSelected: selected,
                    style: IconButton.styleFrom(
                      iconSize: 18,
                      foregroundColor: selected
                          ? Colors.white
                          : Colors.blueGrey,
                    ),
                    onPressed: () => setState(() => selected = !selected),
                    icon: const ScreenshotIcon(),
                  ),
                ),
              ),
            ),
          );
          await tester.pumpAndSettle();
          expect(tester.getSemantics(find.byType(IconButton)).tooltip, '截图');
          expect(
            tester.widget<SvgPicture>(find.byType(SvgPicture)).colorFilter,
            const ColorFilter.mode(Color(0xff607d8b), BlendMode.srcIn),
          );
          await tester.tap(find.byType(IconButton));
          await tester.pumpAndSettle();
          expect(selected, isTrue);
          expect(tester.getSemantics(find.byType(IconButton)).tooltip, '截图');
          expect(
            tester.widget<SvgPicture>(find.byType(SvgPicture)).colorFilter,
            const ColorFilter.mode(Colors.white, BlendMode.srcIn),
          );
          expect(
            tester.getSize(find.byType(ScreenshotIcon)),
            const Size(18, 18),
          );
          expect(tester.takeException(), isNull);
        } finally {
          semantics.dispose();
        }
      },
    );
  }

  testWidgets('a subdued icon follows the surrounding icon opacity', (
    tester,
  ) async {
    await tester.pumpWidget(
      const MaterialApp(
        home: Center(
          child: IconTheme(
            data: IconThemeData(size: 20, color: Colors.black, opacity: 0.5),
            child: ScreenshotIcon(),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(
      tester.widget<SvgPicture>(find.byType(SvgPicture)).colorFilter,
      ColorFilter.mode(Colors.black.withValues(alpha: 0.5), BlendMode.srcIn),
    );
    expect(tester.getSize(find.byType(ScreenshotIcon)), const Size(20, 20));
    expect(tester.takeException(), isNull);
  });
}
