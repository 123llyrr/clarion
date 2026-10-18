// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/mac_shell_preferences.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/theme/mac_shell_theme.dart';
import 'package:clarion_shell/ui/mac_dock_view.dart';

const _output = Size(1000, 800);
const _appId = 'input-target.desktop';
const _appName = 'Input target';

class _Controller extends ShellController {
  _Controller({bool running = true})
    : _apps = [
        DockApp(
          app: const DesktopApp(id: _appId, name: _appName, desktopFile: ''),
          windows: running
              ? const [ShellWindow(id: 7, title: 'Target', appId: _appId)]
              : const [],
          isPinned: !running,
        ),
      ],
      super(
        demo: true,
        notifications: false,
        tray: false,
        powerProfilesEnabled: false,
      );

  final focused = <String>[];
  final List<DockApp> _apps;

  @override
  List<DockApp> get dockApps => _apps;
  @override
  List<DockApp> get recentDockApps => const [];
  @override
  Future<void> focusOrLaunchApp(String appId) async => focused.add(appId);
}

class _Fixture {
  _Fixture(this.controller, this.position);
  final _Controller controller;
  final MacDockPosition position;
  final regions = <Rect>[];
  final exactRegions = <List<Rect>>[];
  int windowClicks = 0;

  Rect input(WidgetTester tester) =>
      regions.last.shift(tester.getTopLeft(find.byType(MacDockView)));

  bool accepts(WidgetTester tester, Offset position) {
    final local = position - tester.getTopLeft(find.byType(MacDockView));
    return exactRegions.last.any((region) => region.contains(local));
  }

  double scale(WidgetTester tester) => tester
      .widget<Transform>(find.byKey(const ValueKey('dock-scale-$_appName')))
      .transform
      .entry(0, 0);

  Offset inwardOf(Rect rect, double distance) => switch (position) {
    MacDockPosition.bottom => Offset(rect.center.dx, rect.top - distance),
    MacDockPosition.left => Offset(rect.right + distance, rect.center.dy),
    MacDockPosition.right => Offset(rect.left - distance, rect.center.dy),
  };

  Offset get screenMargin => switch (position) {
    MacDockPosition.bottom => Offset(10, _output.height - 25),
    MacDockPosition.left => const Offset(25, 10),
    MacDockPosition.right => Offset(_output.width - 25, 10),
  };

  double depth(Rect rect) =>
      position == MacDockPosition.bottom ? rect.height : rect.width;
}

Future<_Fixture> _mount(
  WidgetTester tester,
  MacDockPosition position, {
  bool autoHide = false,
  bool reducedMotion = true,
  bool running = true,
}) async {
  tester.view.physicalSize = _output;
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  final controller = _Controller(running: running)
    ..setShellStyle(ShellStyle.macos)
    ..setReduceMotion(reducedMotion)
    ..setMacPreferences(
      MacShellPreferences(
        dockPosition: position,
        magnificationScale: 1.8,
        dockAutoHide: autoHide,
        showLaunchpad: false,
        showDownloads: false,
        showRecentApplications: false,
      ),
    );
  addTearDown(controller.dispose);
  final fixture = _Fixture(controller, position);
  final dock = MacDockView(
    controller: controller,
    availableWidth: _output.width,
    availableHeight: _output.height,
    onGeometryChanged: (_, input) => fixture.regions.add(input),
    onInputRegionsChanged: (regions) => fixture.exactRegions.add(regions),
  );
  final thickness = MacDockView.surfaceThickness(controller.macPreferences);
  await tester.pumpWidget(
    MaterialApp(
      theme: buildMacShellTheme(Brightness.light),
      home: Stack(
        children: [
          Positioned.fill(
            child: GestureDetector(
              key: const ValueKey('underlying-window'),
              behavior: HitTestBehavior.opaque,
              onTap: () => fixture.windowClicks++,
              child: const ColoredBox(color: Colors.white),
            ),
          ),
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
  return fixture;
}

Finder get _icon => find.byKey(const ValueKey('dock-app-$_appId'));
Finder get _glass => find.byKey(const ValueKey('mac-dock-glass'));

void _expectPaintedInput(WidgetTester tester, _Fixture fixture, String frame) {
  final input = fixture.input(tester);
  var painted = tester.getRect(_glass);
  for (final icon in [
    find.byKey(const ValueKey('dock-finder')),
    _icon,
    find.byKey(const ValueKey('dock-trash')),
  ]) {
    // getRect transforms both corners, including the current scale and bounce.
    final rect = tester.getRect(icon);
    painted = painted.expandToInclude(rect);
    expect(input.left, lessThanOrEqualTo(rect.left + .01), reason: frame);
    expect(input.top, lessThanOrEqualTo(rect.top + .01), reason: frame);
    expect(input.right, greaterThanOrEqualTo(rect.right - .01), reason: frame);
    expect(
      input.bottom,
      greaterThanOrEqualTo(rect.bottom - .01),
      reason: frame,
    );
    for (final corner in [
      rect.topLeft + const Offset(.01, .01),
      rect.topRight + const Offset(-.01, .01),
      rect.bottomLeft + const Offset(.01, -.01),
      rect.bottomRight - const Offset(.01, .01),
    ]) {
      expect(fixture.accepts(tester, corner), isTrue, reason: frame);
    }
  }
  expect(
    input.contains(fixture.inwardOf(painted, 4)),
    isFalse,
    reason: '$frame must not reserve input beyond currently painted content',
  );
}

void main() {
  for (final position in MacDockPosition.values) {
    testWidgets(
      '${position.name} idle native input excludes transparent space',
      (tester) async {
        final fixture = await _mount(tester, position);
        final input = fixture.input(tester);
        final transparent = fixture.inwardOf(tester.getRect(_glass), 12);
        expect(
          tester.getRect(find.byType(MacDockView)).contains(transparent),
          isTrue,
        );
        expect(input.contains(tester.getCenter(_icon)), isTrue);
        expect(
          input.contains(transparent),
          isFalse,
          reason:
              'space reserved for future magnification belongs to the window',
        );
        expect(input.contains(fixture.screenMargin), isFalse);
        await tester.pumpWidget(const SizedBox.shrink());
      },
    );

    testWidgets(
      '${position.name} transparent Dock margins pass clicks through',
      (tester) async {
        final fixture = await _mount(tester, position);
        final transparent = fixture.inwardOf(tester.getRect(_glass), 12);
        final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
        await mouse.addPointer(location: _output.center(Offset.zero));
        for (final point in [transparent, fixture.screenMargin]) {
          await mouse.moveTo(point);
          await tester.pumpAndSettle();
          expect(fixture.scale(tester), 1);
          expect(find.byKey(const ValueKey('mac-dock-label')), findsNothing);
          await tester.tapAt(point);
          await tester.pumpAndSettle();
        }
        expect(fixture.windowClicks, 2);
        expect(fixture.controller.focused, isEmpty);
        await mouse.removePointer();
        await tester.pumpWidget(const SizedBox.shrink());
        expect(tester.takeException(), isNull);
      },
    );

    testWidgets(
      '${position.name} enlarged icon remains clickable then contracts',
      (tester) async {
        final fixture = await _mount(tester, position);
        final restingInput = fixture.input(tester);
        final restingIcon = tester.getRect(_icon);
        final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
        await mouse.addPointer(location: _output.center(Offset.zero));
        await mouse.moveTo(restingIcon.center);
        await tester.pumpAndSettle();
        expect(fixture.scale(tester), greaterThan(1.3));
        final edge = fixture.inwardOf(tester.getRect(_icon), -3);
        expect(restingIcon.contains(edge), isFalse);
        expect(fixture.input(tester).contains(edge), isTrue);
        await mouse.moveTo(edge);
        await tester.pumpAndSettle();
        expect(fixture.scale(tester), greaterThan(1.3));
        await tester.tapAt(edge);
        await tester.pumpAndSettle();
        expect(fixture.controller.focused, [_appId]);
        expect(fixture.windowClicks, 0);
        await mouse.moveTo(_output.center(Offset.zero));
        await tester.pumpAndSettle();
        expect(fixture.scale(tester), 1);
        expect(fixture.input(tester), restingInput);
        expect(fixture.input(tester).contains(edge), isFalse);
        await mouse.removePointer();
        await tester.pumpWidget(const SizedBox.shrink());
        expect(tester.takeException(), isNull);
      },
    );

    testWidgets(
      '${position.name} exact native regions exclude blank space beside a raised icon',
      (tester) async {
        final fixture = await _mount(tester, position);
        final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
        await mouse.addPointer(location: _output.center(Offset.zero));
        await mouse.moveTo(tester.getCenter(_icon));
        await tester.pumpAndSettle();
        expect(fixture.scale(tester), greaterThan(1.3));
        final bounds = fixture.input(tester);
        final neighbor = tester.getRect(
          find.byKey(const ValueKey('dock-finder')),
        );
        final gap = switch (position) {
          MacDockPosition.bottom => Offset(neighbor.center.dx, bounds.top + 2),
          MacDockPosition.left => Offset(bounds.right - 2, neighbor.center.dy),
          MacDockPosition.right => Offset(bounds.left + 2, neighbor.center.dy),
        };
        expect(bounds.contains(gap), isTrue);
        expect(neighbor.contains(gap), isFalse);
        expect(tester.getRect(_icon).contains(gap), isFalse);
        expect(
          fixture.accepts(tester, gap),
          isFalse,
          reason: 'Wayland must receive the exact union, not its bounding box',
        );
        await tester.tapAt(gap);
        await tester.pumpAndSettle();
        expect(fixture.windowClicks, 1);
        expect(fixture.controller.focused, isEmpty);
        expect(fixture.scale(tester), greaterThan(1.3));
        await mouse.removePointer();
        await tester.pumpWidget(const SizedBox.shrink());
        expect(tester.takeException(), isNull);
      },
    );

    testWidgets(
      '${position.name} hidden Dock keeps its two-pixel edge sensor',
      (tester) async {
        final fixture = await _mount(tester, position, autoHide: true);
        final sensor = fixture.input(tester);
        expect(fixture.depth(sensor), 2);
        final justInsideWindow = fixture.inwardOf(sensor, 4);
        expect(sensor.contains(justInsideWindow), isFalse);
        await tester.tapAt(justInsideWindow);
        await tester.pumpAndSettle();
        expect(fixture.windowClicks, 1);
        expect(fixture.input(tester), sensor);
        final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
        await mouse.addPointer(location: _output.center(Offset.zero));
        await mouse.moveTo(sensor.center);
        await tester.pumpAndSettle();
        expect(fixture.depth(fixture.input(tester)), greaterThan(2));
        await mouse.moveTo(tester.getCenter(_icon));
        await tester.pumpAndSettle();
        expect(fixture.scale(tester), greaterThan(1.3));
        await mouse.moveTo(_output.center(Offset.zero));
        await tester.pump(const Duration(milliseconds: 250));
        await tester.pumpAndSettle();
        expect(fixture.input(tester), sensor);
        await mouse.removePointer();
        await tester.pumpWidget(const SizedBox.shrink());
        expect(tester.takeException(), isNull);
      },
    );

    testWidgets(
      '${position.name} animated hover input follows each painted frame',
      (tester) async {
        final fixture = await _mount(tester, position, reducedMotion: false);
        final restingInput = fixture.input(tester);
        final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
        await mouse.addPointer(location: _output.center(Offset.zero));
        await mouse.moveTo(tester.getCenter(_icon));
        await tester.pump();
        _expectPaintedInput(tester, fixture, 'initial entry');
        expect(
          fixture.depth(fixture.input(tester)),
          fixture.depth(restingInput),
        );
        var intermediateFrames = 0;
        for (var frame = 0; frame < 10; frame++) {
          await tester.pump(const Duration(milliseconds: 16));
          _expectPaintedInput(tester, fixture, 'entry frame $frame');
          final scale = fixture.scale(tester);
          if (scale > 1.01 && scale < 1.7) intermediateFrames++;
        }
        expect(intermediateFrames, greaterThan(1));
        expect(fixture.scale(tester), greaterThan(1.3));
        expect(
          fixture.depth(fixture.input(tester)),
          greaterThan(fixture.depth(restingInput)),
        );
        await mouse.moveTo(_output.center(Offset.zero));
        await tester.pump();
        _expectPaintedInput(tester, fixture, 'initial exit');
        for (var frame = 0; frame < 10; frame++) {
          await tester.pump(const Duration(milliseconds: 16));
          _expectPaintedInput(tester, fixture, 'exit frame $frame');
        }
        expect(fixture.scale(tester), 1);
        expect(fixture.input(tester), restingInput);
        await mouse.removePointer();
        await tester.pumpWidget(const SizedBox.shrink());
        expect(tester.takeException(), isNull);
      },
    );

    testWidgets(
      '${position.name} launch bounce input follows the lifted icon each frame',
      (tester) async {
        final fixture = await _mount(
          tester,
          position,
          reducedMotion: false,
          running: false,
        );
        final restingInput = fixture.input(tester);
        await tester.tap(_icon);
        await tester.pump();
        _expectPaintedInput(tester, fixture, 'initial launch');
        var liftedFrames = 0;
        for (var frame = 0; frame < 45; frame++) {
          await tester.pump(const Duration(milliseconds: 16));
          _expectPaintedInput(tester, fixture, 'bounce frame $frame');
          if (fixture.depth(fixture.input(tester)) >
              fixture.depth(restingInput) + 1) {
            liftedFrames++;
          }
        }
        expect(liftedFrames, greaterThan(1));
        expect(fixture.controller.focused, [_appId]);
        expect(fixture.input(tester), restingInput);
        await tester.pumpWidget(const SizedBox.shrink());
        expect(tester.takeException(), isNull);
      },
    );
  }
}
