// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:realand_shell/main.dart';
import 'package:realand_shell/models.dart';
import 'package:realand_shell/preferences.dart';
import 'package:realand_shell/state/shell_controller.dart';
import 'package:realand_shell/ui/mac_dock_view.dart';
import 'package:realand_shell/ui/mac_shell_view.dart';
import 'package:realand_shell/ui/shell_view.dart';

class _OwnedController extends ShellController {
  _OwnedController()
    : super(
        demo: true,
        notifications: false,
        tray: false,
        powerProfilesEnabled: false,
      );
  int disposals = 0;

  @override
  void dispose() {
    disposals++;
    super.dispose();
  }
}

class _TargetController extends _OwnedController {
  final registrations = <Map<int, Rect>>[];
  @override
  List<DockApp> get dockApps => [
    DockApp(
      app: const DesktopApp(id: 'fixtureapp', name: 'Fixture', desktopFile: ''),
      windows: const [
        ShellWindow(id: 91, title: 'Fixture', appId: 'fixtureapp'),
      ],
    ),
  ];
  @override
  Future<void> setDockMinimizeTargets(
    Map<int, Rect> targets, {
    bool? reduceMotion,
  }) async {
    registrations.add(Map.of(targets));
  }
}

class _LoadingDockController extends _OwnedController {
  int serviceStarts = 0;
  @override
  Future<void> initializeDockServices() {
    serviceStarts++;
    notifyListeners();
    return Future.value();
  }
}

void main() {
  const window = MethodChannel('realand.shell/window');

  testWidgets('Dock loading does not notify a sibling during view mounting', (
    tester,
  ) async {
    final controller = _LoadingDockController()
      ..setShellStyle(ShellStyle.macos);
    addTearDown(controller.dispose);
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      window,
      (_) async => null,
    );
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        window,
        null,
      ),
    );
    await tester.pumpWidget(
      Directionality(
        textDirection: TextDirection.ltr,
        child: Column(
          children: [
            AnimatedBuilder(
              animation: controller,
              builder: (_, _) =>
                  Text('Loading starts: ${controller.serviceStarts}'),
            ),
            Expanded(
              child: DockApplication(
                controller: controller,
                preferences: const ShellPreferences(),
                availableWidth: 800,
              ),
            ),
          ],
        ),
      ),
    );
    expect(tester.takeException(), isNull);
    await tester.pumpAndSettle();
    expect(find.text('Loading starts: 1'), findsOneWidget);
    expect(controller.serviceStarts, 1);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  testWidgets(
    'height adjustments reach native geometry and screenshot readiness',
    (tester) async {
      final controller = _OwnedController()
        ..setShellStyle(ShellStyle.macos)
        ..setReduceMotion(true);
      final heights = <double>[];
      var currentHeight = 22.0;
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(window, (
        call,
      ) async {
        if (call.method == 'setMacMenuBarPreferences') {
          currentHeight = ((call.arguments as Map)['height'] as num).toDouble();
          heights.add(currentHeight);
        }
        if (call.method == 'windowState') {
          return {
            'expanded': false,
            'tooltipVisible': false,
            'height': currentHeight,
          };
        }
        return null;
      });
      addTearDown(
        () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          window,
          null,
        ),
      );
      await tester.pumpWidget(
        ShellApplication(
          controller: controller,
          preferences: const ShellPreferences(),
          persist: false,
        ),
      );
      await tester.pumpAndSettle();
      for (final height in [32.0, 48.0, 22.0]) {
        controller.setMacPreferences(
          controller.macPreferences.copyWith(menuBarHeight: height),
        );
        await tester.pumpAndSettle();
        expect(heights.last, height);
        expect(
          tester
              .getSize(find.byKey(const ValueKey('mac-menu-bar-position')))
              .height,
          height,
        );
        var prepared = false;
        final preparation = controller.prepareScreenshot!().then((_) {
          prepared = true;
        });
        for (var frame = 0; frame < 6 && !prepared; frame++) {
          await tester.pump(const Duration(milliseconds: 16));
        }
        expect(prepared, isTrue);
        await preparation;
        expect(controller.panel, ShellPanel.none);
      }
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
      expect(controller.disposals, 1);
    },
  );

  testWidgets(
    'late Dock geometry retries the latest icon target registration',
    (tester) async {
      final controller = _TargetController()..setShellStyle(ShellStyle.macos);
      addTearDown(controller.dispose);
      controller.setReduceMotion(true);
      controller.registrations.clear();
      final gate = Completer<void>();
      var geometryCalls = 0;
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(window, (
        call,
      ) async {
        if (call.method == 'setDockGeometry' && geometryCalls++ == 0) {
          await gate.future;
        }
        if (call.method == 'surfaceOrigin') {
          return {'outputX': 100, 'outputY': 400};
        }
        return null;
      });
      addTearDown(
        () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          window,
          null,
        ),
      );
      tester.view.physicalSize = const Size(800, 180);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      await tester.pumpWidget(
        DockApplication(
          controller: controller,
          preferences: const ShellPreferences(),
          availableWidth: 800,
        ),
      );
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 150));
      expect(controller.registrations, isEmpty);
      controller.setMacPreferences(
        controller.macPreferences.copyWith(dockSize: 64),
      );
      await tester.pump();
      gate.complete();
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 150));
      await tester.pumpAndSettle();
      expect(geometryCalls, greaterThan(1));
      expect(controller.registrations, isNotEmpty);
      final target = controller.registrations.last[91]!;
      expect(target.width, 64);
      expect(target.left, greaterThanOrEqualTo(100));
      expect(target.top, greaterThanOrEqualTo(400));
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets(
    'Launchpad retains backdrop on exit and blocks Dock for another panel',
    (tester) async {
      final controller = _OwnedController()..setShellStyle(ShellStyle.macos);
      final calls = <Map<dynamic, dynamic>>[];
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(window, (
        call,
      ) async {
        if (call.method == 'setLauncherBackdrop') {
          calls.add(call.arguments as Map);
        }
        return null;
      });
      addTearDown(
        () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          window,
          null,
        ),
      );
      await tester.pumpWidget(
        ShellApplication(
          controller: controller,
          preferences: const ShellPreferences(),
          persist: false,
          availableOutputSize: const Size(800, 600),
        ),
      );
      await tester.pumpAndSettle();
      controller.selectPanel(ShellPanel.launcher);
      await tester.pumpAndSettle();
      expect(calls.last, {'visible': true, 'dockInteractive': true});
      controller.selectPanel(ShellPanel.quickSettings);
      await tester.pump();
      await tester.pump();
      expect(calls.last, {'visible': true, 'dockInteractive': false});
      await tester.pump(const Duration(seconds: 1));
      await tester.pumpAndSettle();
      expect(calls.last, {'visible': false, 'dockInteractive': false});
      expect(controller.panel, ShellPanel.quickSettings);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets(
    'output height refreshes without a view resize and retries late reads',
    (tester) async {
      final controller = _OwnedController()..setShellStyle(ShellStyle.macos);
      final first = Completer<Map<String, dynamic>>();
      var reads = 0;
      var height = 900;
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(window, (
        call,
      ) async {
        if (call.method != 'windowState') return null;
        reads++;
        if (reads == 1) return first.future;
        return {
          'monitorGeometry': {'width': 800, 'height': height},
        };
      });
      addTearDown(
        () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          window,
          null,
        ),
      );
      await tester.pumpWidget(
        ShellHost(
          controller: controller,
          preferences: const ShellPreferences(),
          preview: false,
          persist: false,
        ),
        wrapWithView: false,
      );
      await tester.pump();
      expect(reads, 1);
      Future<void> outputChanged() =>
          tester.binding.defaultBinaryMessenger.handlePlatformMessage(
            window.name,
            const StandardMethodCodec().encodeMethodCall(
              const MethodCall('outputGeometryChanged'),
            ),
            (_) {},
          );
      await outputChanged();
      first.complete({
        'monitorGeometry': {'width': 800, 'height': 600},
      });
      await tester.pumpAndSettle();
      expect(reads, 2);
      expect(
        tester
            .widget<ShellApplication>(find.byType(ShellApplication))
            .availableOutputSize,
        const Size(800, 900),
      );
      height = 1200;
      await outputChanged();
      await tester.pumpAndSettle();
      expect(reads, 3);
      expect(
        tester
            .widget<ShellApplication>(find.byType(ShellApplication))
            .availableOutputSize,
        const Size(800, 1200),
      );
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
      expect(controller.disposals, 1);
    },
  );

  testWidgets('one main owner changes layouts and preserves Material edge', (
    tester,
  ) async {
    final controller = _OwnedController()..setBarPosition(BarPosition.bottom);
    final calls = <MethodCall>[];
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(window, (
      call,
    ) async {
      calls.add(call);
      return null;
    });
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        window,
        null,
      ),
    );
    await tester.pumpWidget(
      ShellApplication(
        controller: controller,
        preferences: const ShellPreferences(),
        persist: false,
      ),
    );
    await tester.pumpAndSettle();
    expect(find.byType(ShellView), findsOneWidget);
    controller.selectPanel(ShellPanel.quickSettings);
    controller.setShellStyle(ShellStyle.macos);
    await tester.pumpAndSettle();
    expect(find.byType(MacShellView), findsOneWidget);
    expect(controller.panel, ShellPanel.none);
    expect(controller.barPosition, BarPosition.bottom);
    final index = calls.indexWhere(
      (call) => call.method == 'setShellStyle' && call.arguments == 'macos',
    );
    expect(index, greaterThanOrEqualTo(0));
    expect(
      calls
          .where((call) => call.method == 'setBarPosition')
          .map((call) => call.arguments),
      ['bottom'],
    );
    controller.setShellStyle(ShellStyle.material);
    await tester.pumpAndSettle();
    expect(find.byType(ShellView), findsOneWidget);
    expect(controller.barPosition, BarPosition.bottom);
    expect(controller.disposals, 0);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
    expect(controller.disposals, 1);
  });

  testWidgets('failed native Dock restores Material with visible explanation', (
    tester,
  ) async {
    final controller = _OwnedController();
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(window, (
      call,
    ) async {
      if (call.method == 'setShellStyle' && call.arguments == 'macos') {
        throw PlatformException(code: 'dock-unavailable');
      }
      return null;
    });
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        window,
        null,
      ),
    );
    await tester.pumpWidget(
      ShellApplication(
        controller: controller,
        preferences: const ShellPreferences(),
        persist: false,
      ),
    );
    await tester.pumpAndSettle();
    controller.setShellStyle(ShellStyle.macos);
    await tester.pumpAndSettle();
    expect(controller.shellStyle, ShellStyle.material);
    expect(controller.panel, ShellPanel.quickSettings);
    expect(controller.error, contains('已恢复 Material'));
    expect(find.text(controller.error!), findsOneWidget);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
    expect(controller.disposals, 1);
  });

  testWidgets('secondary Dock removal never disposes shared desktop services', (
    tester,
  ) async {
    final controller = _OwnedController()..setShellStyle(ShellStyle.macos);
    addTearDown(controller.dispose);
    await tester.pumpWidget(
      DockApplication(
        controller: controller,
        preferences: const ShellPreferences(),
        availableWidth: 800,
        preview: true,
      ),
    );
    await tester.pumpAndSettle();
    expect(find.byType(MacDockView), findsOneWidget);
    await tester.pumpWidget(const SizedBox.shrink());
    expect(controller.disposals, 0);
    controller.setShellStyle(ShellStyle.material);
    expect(controller.shellStyle, ShellStyle.material);
    expect(tester.takeException(), isNull);
  });

  testWidgets('Dock geometry skips obsolete frames while native work is busy', (
    tester,
  ) async {
    final controller = _OwnedController()
      ..setShellStyle(ShellStyle.macos)
      ..setReduceMotion(true);
    addTearDown(controller.dispose);
    final pendingWidth = Completer<void>();
    final calls = <MethodCall>[];
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(window, (
      call,
    ) async {
      calls.add(call);
      if (call.method == 'setDockGeometry' && calls.length == 1) {
        await pendingWidth.future;
      }
      return null;
    });
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        window,
        null,
      ),
    );
    Future<void> resize(double width) async {
      await tester.pumpWidget(
        DockApplication(
          controller: controller,
          preferences: const ShellPreferences(),
          availableWidth: width,
        ),
      );
      await tester.pump();
    }

    await resize(800);
    expect(
      calls.where((call) => call.method == 'setDockGeometry'),
      hasLength(1),
    );
    for (final width in [700.0, 600.0, 500.0, 400.0, 320.0]) {
      await resize(width);
    }
    pendingWidth.complete();
    await tester.pumpAndSettle();
    final widthCalls = calls.where((call) => call.method == 'setDockGeometry');
    final inputCalls = calls.where(
      (call) => call.method == 'setDockInputRegion',
    );
    expect(widthCalls, hasLength(2));
    expect(inputCalls, hasLength(1));
    expect(
      (widthCalls.last.arguments as Map)['width'],
      lessThan((widthCalls.first.arguments as Map)['width']),
    );
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  testWidgets('Dock magnification updates regions without resizing GTK', (
    tester,
  ) async {
    final controller = _TargetController()..setShellStyle(ShellStyle.macos);
    addTearDown(controller.dispose);
    final calls = <MethodCall>[];
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(window, (
      call,
    ) async {
      calls.add(call);
      if (call.method == 'surfaceOrigin') {
        return {'outputX': 100, 'outputY': 400};
      }
      return null;
    });
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        window,
        null,
      ),
    );
    tester.view.physicalSize = const Size(800, 180);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(
      DockApplication(
        controller: controller,
        preferences: const ShellPreferences(),
        availableWidth: 800,
      ),
    );
    await tester.pumpAndSettle();
    await tester.pump(const Duration(milliseconds: 150));
    expect(
      calls.where((call) => call.method == 'setDockGeometry'),
      hasLength(1),
    );
    calls.clear();
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: const Offset(0, 0));
    final icon = find.byKey(const ValueKey('dock-app-fixtureapp'));
    await mouse.moveTo(tester.getCenter(icon));
    for (var frame = 0; frame < 12; frame++) {
      await tester.pump(const Duration(milliseconds: 16));
    }
    await tester.pumpAndSettle();
    expect(
      calls.where((call) => call.method == 'setDockGeometry'),
      isEmpty,
      reason: 'the magnification envelope already reserves native space',
    );
    expect(
      calls.where((call) => call.method == 'setDockInputRegion'),
      isNotEmpty,
      reason: 'expanded icons must still receive pointer input',
    );
    expect(
      calls.where((call) => call.method == 'setDockGlassRegion'),
      isNotEmpty,
      reason: 'the compositor blur must follow the animated glass',
    );
    await mouse.removePointer();
    await tester.pumpAndSettle();
    await tester.pumpWidget(const SizedBox.shrink());
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'Dock sends exact regions even when their bounding box is unchanged',
    (tester) async {
      final controller = _OwnedController()
        ..setShellStyle(ShellStyle.macos)
        ..setReduceMotion(true);
      addTearDown(controller.dispose);
      final calls = <MethodCall>[];
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(window, (
        call,
      ) async {
        calls.add(call);
        return null;
      });
      addTearDown(
        () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          window,
          null,
        ),
      );
      await tester.pumpWidget(
        DockApplication(
          controller: controller,
          preferences: const ShellPreferences(),
          availableWidth: 800,
        ),
      );
      await tester.pumpAndSettle();
      await tester.pump(const Duration(milliseconds: 150));
      final dock = tester.widget<MacDockView>(find.byType(MacDockView));
      const bounds = Rect.fromLTWH(10, 10, 300, 110);
      const base = Rect.fromLTWH(10, 60, 300, 60);
      const firstIcon = Rect.fromLTWH(50, 10, 50, 80);
      const movedIcon = Rect.fromLTWH(150, 10, 50, 80);
      calls.clear();
      dock.onGeometryChanged!(const Size(400, 120), bounds);
      dock.onInputRegionsChanged!(const [base, firstIcon]);
      await tester.pump();
      final first =
          calls
                  .singleWhere((call) => call.method == 'setDockInputRegion')
                  .arguments
              as Map;
      expect(first['regions'], [
        {'x': 10.0, 'y': 60.0, 'width': 300.0, 'height': 60.0},
        {'x': 50.0, 'y': 10.0, 'width': 50.0, 'height': 80.0},
      ]);
      calls.clear();
      dock.onInputRegionsChanged!(const [base, movedIcon]);
      await tester.pump();
      expect(calls.map((call) => call.method), ['setDockInputRegion']);
      final moved = calls.single.arguments as Map;
      expect(moved['x'], first['x']);
      expect(moved['y'], first['y']);
      expect(moved['width'], first['width']);
      expect(moved['height'], first['height']);
      expect((moved['regions'] as List).last, {
        'x': 150.0,
        'y': 10.0,
        'width': 50.0,
        'height': 80.0,
      });
      calls.clear();
      dock.onInputRegionsChanged!([base, movedIcon]);
      await tester.pump();
      expect(calls, isEmpty);
      await tester.pumpWidget(const SizedBox.shrink());
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('target-only Dock updates do not resend native surface regions', (
    tester,
  ) async {
    final controller = _TargetController()
      ..setShellStyle(ShellStyle.macos)
      ..setReduceMotion(true);
    addTearDown(controller.dispose);
    final calls = <MethodCall>[];
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(window, (
      call,
    ) async {
      calls.add(call);
      if (call.method == 'surfaceOrigin') {
        return {'outputX': 100, 'outputY': 400};
      }
      return null;
    });
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        window,
        null,
      ),
    );
    await tester.pumpWidget(
      DockApplication(
        controller: controller,
        preferences: const ShellPreferences(),
        availableWidth: 800,
      ),
    );
    await tester.pumpAndSettle();
    await tester.pump(const Duration(milliseconds: 150));
    final arguments =
        calls.lastWhere((call) => call.method == 'setDockGlassRegion').arguments
            as Map;
    final glass = Rect.fromLTWH(
      arguments['x'] as double,
      arguments['y'] as double,
      arguments['width'] as double,
      arguments['height'] as double,
    );
    calls.clear();
    controller.registrations.clear();
    const target = Rect.fromLTWH(30, 70, 48, 48);
    tester.widget<MacDockView>(find.byType(MacDockView)).onPresentationChanged!(
      glass,
      const {91: target},
      true,
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 150));
    expect(calls.map((call) => call.method), ['surfaceOrigin']);
    expect(controller.registrations, [
      {91: target.shift(const Offset(100, 400))},
    ]);
    await tester.pumpWidget(const SizedBox.shrink());
    expect(tester.takeException(), isNull);
  });

  testWidgets('late native failure cannot undo a newer Mac selection', (
    tester,
  ) async {
    final controller = _OwnedController();
    final firstMac = Completer<Object?>();
    var macRequests = 0;
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(window, (
      call,
    ) async {
      if (call.method == 'setShellStyle' && call.arguments == 'macos') {
        macRequests++;
        if (macRequests == 1) return firstMac.future;
      }
      return null;
    });
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        window,
        null,
      ),
    );
    await tester.pumpWidget(
      ShellApplication(
        controller: controller,
        preferences: const ShellPreferences(),
        persist: false,
      ),
    );
    await tester.pumpAndSettle();
    controller.setShellStyle(ShellStyle.macos);
    await tester.pump();
    expect(macRequests, 1);
    controller.setShellStyle(ShellStyle.material);
    controller.setShellStyle(ShellStyle.macos);
    firstMac.completeError(PlatformException(code: 'obsolete-dock-failure'));
    await tester.pumpAndSettle();
    expect(macRequests, 2);
    expect(controller.shellStyle, ShellStyle.macos);
    expect(controller.error, isNull);
    expect(find.byType(MacShellView), findsOneWidget);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  testWidgets('ordinary macOS preview includes Dock without native surfaces', (
    tester,
  ) async {
    final controller = _OwnedController()..setShellStyle(ShellStyle.macos);
    await tester.pumpWidget(
      ShellApplication(
        controller: controller,
        preferences: const ShellPreferences(),
        preview: true,
        persist: false,
      ),
    );
    await tester.pumpAndSettle();
    expect(find.byType(MacShellView), findsOneWidget);
    expect(find.byType(MacDockView), findsOneWidget);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  });
}
