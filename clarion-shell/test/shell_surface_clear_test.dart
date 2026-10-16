// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/main.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/preferences.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/ui/mac_dock_view.dart';
import 'package:clarion_shell/ui/shell_surface_clear.dart';

Future<List<int>> _pixel(
  WidgetTester tester,
  GlobalKey boundaryKey,
  int x,
  int y,
) async {
  return (await tester.runAsync(() async {
    final boundary =
        boundaryKey.currentContext!.findRenderObject()!
            as RenderRepaintBoundary;
    final image = await boundary.toImage(pixelRatio: 1);
    try {
      final bytes = (await image.toByteData(
        format: ui.ImageByteFormat.rawRgba,
      ))!;
      final offset = (y * image.width + x) * 4;
      return List.generate(4, (channel) => bytes.getUint8(offset + channel));
    } finally {
      image.dispose();
    }
  }))!;
}

void main() {
  testWidgets('native root clears old pixels around retained sprites', (
    tester,
  ) async {
    final key = GlobalKey();
    var showSprite = true;
    var previousFrameColor = const Color(0xffcc4411);
    late StateSetter update;
    await tester.pumpWidget(
      Center(
        child: RepaintBoundary(
          key: key,
          child: SizedBox(
            width: 160,
            height: 120,
            child: StatefulBuilder(
              builder: (context, setState) {
                update = setState;
                return Stack(
                  fit: StackFit.expand,
                  alignment: Alignment.topLeft,
                  children: [
                    // Simulate old pixels in the view's root display list.
                    ColoredBox(color: previousFrameColor),
                    ShellSurfaceClear(
                      child: Stack(
                        alignment: Alignment.topLeft,
                        children: [
                          if (showSprite)
                            const Positioned(
                              left: 50,
                              top: 75,
                              width: 40,
                              height: 30,
                              child: RepaintBoundary(
                                child: ColoredBox(color: Color(0xff2277dd)),
                              ),
                            ),
                        ],
                      ),
                    ),
                  ],
                );
              },
            ),
          ),
        ),
      ),
    );
    for (var frame = 0; frame < 5; frame++) {
      await tester.pump(const Duration(milliseconds: 16));
    }
    for (final point in const [Offset(1, 0), Offset(159, 0), Offset(1, 119)]) {
      expect(await _pixel(tester, key, point.dx.toInt(), point.dy.toInt()), [
        0,
        0,
        0,
        0,
      ]);
    }
    expect(await _pixel(tester, key, 60, 85), [34, 119, 221, 255]);

    update(() {
      previousFrameColor = const Color(0xff11bb66);
      showSprite = false;
    });
    await tester.pump();
    expect(await _pixel(tester, key, 1, 0), [0, 0, 0, 0]);
    expect(await _pixel(tester, key, 60, 85), [0, 0, 0, 0]);
    update(() => showSprite = true);
    await tester.pump();
    expect(await _pixel(tester, key, 1, 0), [0, 0, 0, 0]);
    expect(await _pixel(tester, key, 60, 85), [34, 119, 221, 255]);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  testWidgets('main native view erases closed panel pixels at the root', (
    tester,
  ) async {
    final key = GlobalKey();
    final controller = ShellController(demo: true)
      ..setShellStyle(ShellStyle.macos)
      ..setReduceMotion(true);
    tester.view.physicalSize = const Size(1200, 800);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(
      RepaintBoundary(
        key: key,
        child: Stack(
          fit: StackFit.expand,
          alignment: Alignment.topLeft,
          children: [
            // Old framebuffer contents must be erased by the native root,
            // including when child display lists are retained or removed.
            const ColoredBox(color: Color(0xffaa3355)),
            ShellApplication(
              controller: controller,
              preferences: const ShellPreferences(),
              persist: false,
            ),
          ],
        ),
      ),
    );
    for (var cycle = 0; cycle < 3; cycle++) {
      controller.selectPanel(ShellPanel.quickSettings);
      await tester.pumpAndSettle();
      final rect = tester.getRect(
        find.byKey(const ValueKey('mac-control-center-glass')),
      );
      controller.closePanel();
      await tester.pumpAndSettle();
      expect(
        await _pixel(
          tester,
          key,
          rect.center.dx.round(),
          rect.center.dy.round(),
        ),
        [0, 0, 0, 0],
      );
      expect(await _pixel(tester, key, 50, 500), [0, 0, 0, 0]);
    }
    controller.setShellStyle(ShellStyle.material);
    await tester.pumpAndSettle();
    expect(await _pixel(tester, key, 50, 500), [0, 0, 0, 0]);
    await tester.pumpWidget(const SizedBox.shrink());
    controller.dispose();
  });

  testWidgets('inline Dock preview preserves the wallpaper underneath it', (
    tester,
  ) async {
    final key = GlobalKey();
    final controller = ShellController(demo: true)
      ..setShellStyle(ShellStyle.macos)
      ..setReduceMotion(true);
    await tester.pumpWidget(
      MaterialApp(
        home: Center(
          child: RepaintBoundary(
            key: key,
            child: SizedBox(
              width: 600,
              height: 120,
              child: Stack(
                fit: StackFit.expand,
                children: [
                  const ColoredBox(color: Color(0xff2e5590)),
                  MacDockView(controller: controller, availableWidth: 600),
                ],
              ),
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(await _pixel(tester, key, 1, 0), [46, 85, 144, 255]);
    expect(await _pixel(tester, key, 599, 0), [46, 85, 144, 255]);
    await tester.pumpWidget(const SizedBox.shrink());
    controller.dispose();
  });
}
