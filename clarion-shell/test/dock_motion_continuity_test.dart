// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/mac_shell_preferences.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/theme/mac_shell_theme.dart';
import 'package:clarion_shell/ui/dock_motion.dart';
import 'package:clarion_shell/ui/mac_dock_view.dart';

const _appId = 'motion-target.desktop';
const _appName = 'Motion target';

class _Controller extends ShellController {
  _Controller()
    : super(
        demo: true,
        notifications: false,
        tray: false,
        powerProfilesEnabled: false,
      );

  bool running = false;
  bool appPresent = true;
  int launches = 0;

  void markRunning() {
    running = true;
    notifyListeners();
  }

  void removeApp() {
    appPresent = false;
    notifyListeners();
  }

  @override
  int get desktopRevision =>
      super.desktopRevision + (running ? 1 : 0) + (appPresent ? 0 : 2);

  @override
  List<DockApp> get dockApps => [
    if (appPresent)
      DockApp(
        app: const DesktopApp(id: _appId, name: _appName, desktopFile: ''),
        isPinned: true,
        windows: running
            ? const [ShellWindow(id: 93, title: 'Target', appId: _appId)]
            : const [],
      ),
  ];

  @override
  List<DockApp> get recentDockApps => const [];

  @override
  Future<void> focusOrLaunchApp(String id) async => launches++;
}

class _Counter extends StatelessWidget {
  const _Counter(this.onBuild);
  final VoidCallback onBuild;

  @override
  Widget build(BuildContext context) {
    onBuild();
    return const SizedBox.square(dimension: 10);
  }
}

Future<_Controller> _mount(
  WidgetTester tester,
  MacDockPosition position,
) async {
  const output = Size(1000, 800);
  tester.view.physicalSize = output;
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  final controller = _Controller()
    ..setShellStyle(ShellStyle.macos)
    ..setReduceMotion(false)
    ..setMacPreferences(
      MacShellPreferences(
        dockPosition: position,
        magnificationScale: 1.8,
        showLaunchpad: false,
        showDownloads: false,
        showRecentApplications: false,
      ),
    );
  addTearDown(controller.dispose);
  final dock = MacDockView(
    controller: controller,
    availableWidth: output.width,
    availableHeight: output.height,
  );
  final thickness = MacDockView.surfaceThickness(controller.macPreferences);
  await tester.pumpWidget(
    MaterialApp(
      theme: buildMacShellTheme(Brightness.light),
      home: Stack(
        children: [
          switch (position) {
            MacDockPosition.bottom => Positioned(
              left: 0,
              right: 0,
              bottom: 0,
              height: thickness,
              child: dock,
            ),
            MacDockPosition.left => Positioned(
              left: 0,
              top: 0,
              bottom: 0,
              width: thickness,
              child: dock,
            ),
            MacDockPosition.right => Positioned(
              right: 0,
              top: 0,
              bottom: 0,
              width: thickness,
              child: dock,
            ),
          },
        ],
      ),
    ),
  );
  await tester.pumpAndSettle();
  return controller;
}

double _scale(WidgetTester tester) => tester
    .widget<Transform>(find.byKey(const ValueKey('dock-scale-$_appName')))
    .transform
    .entry(0, 0);

double _lift(WidgetTester tester, MacDockPosition position) {
  final translation = tester
      .widget<Transform>(find.byKey(const ValueKey('dock-bounce-$_appId')))
      .transform
      .getTranslation();
  return switch (position) {
    MacDockPosition.bottom => -translation.y,
    MacDockPosition.left => translation.x,
    MacDockPosition.right => -translation.x,
  };
}

void main() {
  test('retargeting preserves current position and velocity', () {
    final rising = const DockMotionState(
      value: 1,
      target: 1.8,
    ).advance(.032, rate: 75);
    final reversed = rising.retarget(1);
    expect(reversed.value, rising.value);
    expect(reversed.velocity, rising.velocity);
    expect(
      reversed.advance(.000001, rate: 75).velocity,
      closeTo(rising.velocity, .01),
    );
    expect(reversed.advance(.16, rate: 75).value, closeTo(1, .0001));
  });

  test('same elapsed time produces the same motion at 30, 60 and 144 Hz', () {
    const initial = DockMotionState(value: 1, target: 1.8);
    final direct = initial.advance(.12, rate: 75);
    for (final hz in [30, 60, 144]) {
      var sample = initial;
      const total = .12;
      final frames = (hz * total).floor();
      for (var frame = 0; frame < frames; frame++) {
        sample = sample.advance(1 / hz, rate: 75);
      }
      sample = sample.advance(total - frames / hz, rate: 75);
      expect(sample.value, closeTo(direct.value, 1e-12));
      expect(sample.velocity, closeTo(direct.velocity, 1e-10));
    }
  });

  test('Dock length follows the sum of magnified icon widths', () {
    var a = const DockMotionState(value: 1, target: 1.8);
    var b = const DockMotionState(value: 1, target: 1.3);
    var width = const DockMotionState(value: 124, target: 179);
    for (var frame = 0; frame < 12; frame++) {
      if (frame == 3) {
        a = a.retarget(1.2);
        b = b.retarget(1.8);
        width = width.retarget(24 + 50 * (1.2 + 1.8));
      }
      a = a.advance(.016, rate: 75);
      b = b.advance(.016, rate: 75);
      width = width.advance(.016, rate: 75);
      expect(width.value, closeTo(24 + 50 * (a.value + b.value), 1e-10));
    }
  });

  testWidgets('static icon subtree is not rebuilt on motion frames', (
    tester,
  ) async {
    var target = 1.0;
    var childBuilds = 0;
    late StateSetter update;
    await tester.pumpWidget(
      Directionality(
        textDirection: TextDirection.ltr,
        child: StatefulBuilder(
          builder: (context, setState) {
            update = setState;
            return DockMotionBuilder(
              target: target,
              child: _Counter(() => childBuilds++),
              builder: (context, value, child) =>
                  Transform.scale(scale: value, child: child),
            );
          },
        ),
      ),
    );
    update(() => target = 1.8);
    await tester.pump();
    final beforeFrames = childBuilds;
    for (var frame = 0; frame < 10; frame++) {
      await tester.pump(const Duration(milliseconds: 16));
    }
    expect(childBuilds, beforeFrames);
    await tester.pumpAndSettle();
    expect(tester.hasRunningAnimations, isFalse);
  });

  for (final position in MacDockPosition.values) {
    testWidgets(
      '${position.name} removed hovered app discards its queued pointer packet',
      (tester) async {
        final controller = await _mount(tester, position);
        final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
        await mouse.addPointer(location: const Offset(500, 400));
        final center = tester.getCenter(
          find.byKey(const ValueKey('dock-app-$_appId')),
        );
        await mouse.moveTo(center);
        await tester.pumpAndSettle();
        expect(_scale(tester), greaterThan(1.3));
        await mouse.moveTo(center + const Offset(.5, .5));
        controller.removeApp();
        await tester.pumpAndSettle();
        for (final label in ['Home', '废纸篓']) {
          final scale = tester
              .widget<Transform>(find.byKey(ValueKey('dock-scale-$label')))
              .transform
              .entry(0, 0);
          expect(scale, 1);
        }
        expect(find.byKey(const ValueKey('mac-dock-label')), findsNothing);
        await mouse.removePointer();
        await tester.pumpWidget(const SizedBox.shrink());
      },
    );

    testWidgets(
      '${position.name} smaller magnification preference bounds the next painted frame',
      (tester) async {
        final controller = await _mount(tester, position);
        final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
        await mouse.addPointer(location: const Offset(500, 400));
        await mouse.moveTo(
          tester.getCenter(find.byKey(const ValueKey('dock-app-$_appId'))),
        );
        await tester.pump();
        await tester.pump(const Duration(milliseconds: 32));
        expect(_scale(tester), greaterThan(1.1));
        controller.setMacPreferences(
          controller.macPreferences.copyWith(
            magnificationScale: 1.05,
            dockSize: 32,
          ),
        );
        await tester.pump();
        for (var frame = 0; frame < 12; frame++) {
          expect(_scale(tester), inInclusiveRange(1, 1.05));
          await tester.pump(const Duration(milliseconds: 16));
        }
        await tester.pumpAndSettle();
        expect(_scale(tester), 1);
        await mouse.removePointer();
        await tester.pumpWidget(const SizedBox.shrink());
      },
    );

    testWidgets(
      '${position.name} repeated launch does not reset airborne icon',
      (tester) async {
        final controller = await _mount(tester, position);
        final icon = find.byKey(const ValueKey('dock-app-$_appId'));
        await tester.tap(icon);
        await tester.pump();
        await tester.pump(const Duration(milliseconds: 120));
        final airborne = _lift(tester, position);
        expect(airborne, greaterThan(1));
        await tester.tap(icon);
        await tester.pump();
        expect(_lift(tester, position), closeTo(airborne, .001));
        expect(controller.launches, 2);
        await tester.pumpAndSettle();
        expect(_lift(tester, position), 0);
        await tester.pumpWidget(const SizedBox.shrink());
      },
    );

    testWidgets(
      '${position.name} launch arrival completes continuous landing',
      (tester) async {
        final controller = await _mount(tester, position);
        await tester.tap(find.byKey(const ValueKey('dock-app-$_appId')));
        await tester.pump();
        await tester.pump(const Duration(milliseconds: 120));
        final airborne = _lift(tester, position);
        controller.markRunning();
        await tester.pump();
        expect(_lift(tester, position), closeTo(airborne, .001));
        await tester.pumpAndSettle();
        expect(_lift(tester, position), 0);
        await tester.pumpWidget(const SizedBox.shrink());
      },
    );

    testWidgets(
      '${position.name} hover reversal keeps position and can stop instantly',
      (tester) async {
        final controller = await _mount(tester, position);
        final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
        await mouse.addPointer(location: const Offset(500, 400));
        await mouse.moveTo(
          tester.getCenter(find.byKey(const ValueKey('dock-app-$_appId'))),
        );
        await tester.pump();
        await tester.pump(const Duration(milliseconds: 32));
        final rising = _scale(tester);
        expect(rising, greaterThan(1));
        await mouse.moveTo(const Offset(500, 400));
        await tester.pump();
        expect(_scale(tester), closeTo(rising, .001));
        await tester.pump(const Duration(milliseconds: 16));
        expect(_scale(tester), greaterThanOrEqualTo(1));
        controller.setReduceMotion(true);
        await tester.pump();
        expect(_scale(tester), 1);
        await mouse.removePointer();
        await tester.pumpAndSettle();
        expect(tester.hasRunningAnimations, isFalse);
        await tester.pumpWidget(const SizedBox.shrink());
      },
    );
  }
}
