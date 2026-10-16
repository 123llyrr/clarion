// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/ui/retained_surface.dart';

void main() {
  testWidgets('reversal preserves progress and builds content only on layout', (
    tester,
  ) async {
    var visible = true;
    var reduced = false;
    var progress = 0.0;
    var builds = 0;
    final retained = <bool>[];
    late StateSetter update;
    await tester.pumpWidget(
      MaterialApp(
        home: StatefulBuilder(
          builder: (context, setState) {
            update = setState;
            return RetainedSurface(
              visible: visible,
              reducedMotion: reduced,
              duration: const Duration(milliseconds: 140),
              reverseDuration: const Duration(milliseconds: 100),
              onSurfaceVisible: retained.add,
              builder: (context, constraints) {
                builds++;
                return const Text('retained content');
              },
              transitionBuilder: (context, value, child) {
                progress = value;
                return Opacity(opacity: value, child: child);
              },
            );
          },
        ),
      ),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 48));
    final entering = progress;
    expect(entering, inExclusiveRange(0, 1));
    final beforeTicks = builds;
    await tester.pump(const Duration(milliseconds: 16));
    await tester.pump(const Duration(milliseconds: 16));
    expect(builds, beforeTicks);
    final beforeClose = progress;
    update(() => visible = false);
    await tester.pump();
    expect(progress, beforeClose);
    await tester.pump(const Duration(milliseconds: 16));
    expect(progress, lessThan(beforeClose));
    final exiting = progress;
    update(() => visible = true);
    await tester.pump();
    expect(progress, exiting);
    await tester.pumpAndSettle();
    expect(progress, 1);
    expect(retained, [true]);
    expect(tester.binding.transientCallbackCount, 0);

    update(() => visible = false);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 16));
    update(() => reduced = true);
    await tester.pump();
    expect(find.text('retained content'), findsNothing);
    expect(retained, [true, false]);
    expect(tester.binding.transientCallbackCount, 0);
    update(() => visible = true);
    await tester.pump();
    expect(progress, 1);
    await tester.pumpWidget(const SizedBox.shrink());
    expect(retained, [true, false, true, false]);
  });

  testWidgets('close before first layout does not resurrect a surface', (
    tester,
  ) async {
    final retained = <bool>[];
    Widget view(bool visible) => MaterialApp(
      home: SizedBox.shrink(
        child: RetainedSurface(
          visible: visible,
          reducedMotion: false,
          duration: const Duration(milliseconds: 140),
          reverseDuration: const Duration(milliseconds: 100),
          onSurfaceVisible: retained.add,
          builder: (context, constraints) => const Text('panel'),
          transitionBuilder: (context, value, child) =>
              Opacity(opacity: value, child: child),
        ),
      ),
    );
    await tester.pumpWidget(view(true));
    await tester.pumpWidget(view(false));
    await tester.pumpAndSettle();
    expect(find.text('panel'), findsNothing);
    expect(retained, [true, false]);
    expect(tester.binding.transientCallbackCount, 0);
  });
}
