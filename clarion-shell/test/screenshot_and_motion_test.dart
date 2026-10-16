// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/mac_shell_preferences.dart';
import 'package:clarion_shell/main.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/preferences.dart';
import 'package:clarion_shell/services/desktop_apps.dart';
import 'package:clarion_shell/services/clarion_ipc.dart';
import 'package:clarion_shell/services/system_service.dart';
import 'package:clarion_shell/services/wallpaper_service.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/ui/launcher_surface.dart';

const _window = MethodChannel('clarion.shell/window');

class _Ipc extends ClarionIpc {
  final connectionEvents = StreamController<bool>.broadcast(sync: true);
  final compositorEvents = StreamController<Map<String, dynamic>>.broadcast(
    sync: true,
  );
  final actions = <(String, Map<String, Object?>)>[];
  final timeline = <String>[];
  final actionStarted = Completer<void>();
  Completer<void>? actionGate;
  void Function(String, Map<String, Object?>)? onAction;
  bool closed = false;

  @override
  Stream<bool> get connections => connectionEvents.stream;
  @override
  Stream<Map<String, dynamic>> get events => compositorEvents.stream;
  @override
  void start() => connectionEvents.add(true);
  @override
  Future<void> action(
    String name, [
    Map<String, Object?> arguments = const {},
  ]) async {
    onAction?.call(name, arguments);
    timeline.add(name);
    actions.add((name, Map.of(arguments)));
    if (!actionStarted.isCompleted) actionStarted.complete();
    final gate = actionGate;
    if (gate != null) await gate.future;
  }

  @override
  void dispose() {
    if (closed) return;
    closed = true;
    unawaited(connectionEvents.close());
    unawaited(compositorEvents.close());
    super.dispose();
  }
}

class _System extends SystemService {
  @override
  Future<SystemSnapshot> read() async => const SystemSnapshot();
}

class _Apps extends DesktopApps {
  @override
  Future<List<DesktopApp>> load() async => const [];
}

class _Wallpapers extends WallpaperService {
  @override
  Future<void> restore({bool applySaved = false}) async {}
}

class _Fixture {
  _Fixture({bool demo = false}) {
    controller = ShellController(
      demo: demo,
      notifications: false,
      tray: false,
      powerProfilesEnabled: false,
      ipc: ipc,
      system: _System(),
      desktopApps: _Apps(),
      wallpaperService: _Wallpapers(),
    );
  }
  final ipc = _Ipc();
  late final ShellController controller;
}

class _NativeWindow {
  final changes = <(String, bool)>[];
  bool expanded = false;
  bool tooltipVisible = false;
  bool macos = false;
  bool bottom = false;
  bool menuAutoHide = false;
  bool menuVisible = true;
  int menuHeight = 22;
  bool pauseNextExpanded = false;
  Completer<void>? pausedExpanded;
  Completer<Map<String, dynamic>>? queryGate;

  Map<String, dynamic> get state => {
    'expanded': expanded,
    'tooltipVisible': tooltipVisible,
    // Match the runner: collapse changes input/layer/keyboard, not the buffer.
    'height': 800,
    'inputHeight': expanded
        ? 800
        : macos && menuAutoHide && !menuVisible
        ? 2
        : macos
        ? menuHeight
        : 40,
    'inputY': bottom && !macos && !expanded ? 760 : 0,
    'keyboard': expanded ? 'Exclusive' : 'None',
    'layer': expanded ? 'Overlay' : 'Top',
    'shellStyle': macos ? 'macos' : 'material',
  };

  Future<Object?> handle(MethodCall call) async {
    if (call.method == 'windowState') return queryGate?.future ?? state;
    if (call.method == 'setShellStyle') {
      macos = call.arguments == 'macos';
      return null;
    }
    if (call.method == 'setBarPosition') {
      bottom = call.arguments == 'bottom';
      return null;
    }
    if (call.method == 'setMacMenuBarPreferences') {
      final preferences = call.arguments as Map;
      menuAutoHide = preferences['autoHide'] as bool;
      menuVisible = !menuAutoHide || preferences['visible'] == true;
      menuHeight = (preferences['height'] as num).toInt();
      return null;
    }
    if (call.method != 'setExpanded' && call.method != 'setTooltipVisible') {
      return null;
    }
    final value = call.arguments as bool;
    changes.add((call.method, value));
    if (call.method == 'setExpanded') {
      if (pauseNextExpanded) {
        pauseNextExpanded = false;
        pausedExpanded = Completer<void>();
        await pausedExpanded!.future;
      }
      expanded = value;
      if (expanded) tooltipVisible = false;
    } else {
      tooltipVisible = !expanded && value;
    }
    return null;
  }

  void release() {
    if (pausedExpanded != null && !pausedExpanded!.isCompleted) {
      pausedExpanded!.complete();
    }
    if (queryGate != null && !queryGate!.isCompleted) {
      queryGate!.complete(state);
    }
  }
}

Future<void> _frames(WidgetTester tester) async {
  // Drain serialized channel replies without advancing an animation's clock.
  for (var turn = 0; turn < 6; turn++) {
    await tester.pump();
  }
}

Future<void> _mount(WidgetTester tester, ShellController controller) async {
  tester.view.devicePixelRatio = 1;
  tester.view.physicalSize = const Size(1200, 800);
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  await tester.pumpWidget(
    ShellApplication(
      controller: controller,
      preferences: const ShellPreferences(),
      persist: false,
    ),
  );
  await _frames(tester);
}

Finder get _surfaceOpacity => find
    .descendant(
      of: find.byType(LauncherSurface),
      matching: find.byType(Opacity),
    )
    .first;

double _opacity(WidgetTester tester) =>
    tester.widget<Opacity>(_surfaceOpacity).opacity;

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  late _NativeWindow native;
  setUp(() {
    native = _NativeWindow();
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(_window, native.handle);
  });
  tearDown(() {
    native.release();
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(_window, null);
  });

  for (final mode in ScreenshotMode.values) {
    test(
      '$mode releases the modal before preparing and sends its exact IPC payload',
      () async {
        final fixture = _Fixture();
        final controller = fixture.controller;
        addTearDown(controller.dispose);
        await controller.start();
        controller.selectPanel(ShellPanel.launcher);
        controller.showBarTooltip(
          Object(),
          'blocked hover',
          const Offset(100, 20),
        );
        controller.prepareScreenshot = () async {
          expect(controller.panel, ShellPanel.none);
          expect(controller.barTooltip, isNull);
          expect(fixture.ipc.actions, isEmpty);
          fixture.ipc.timeline.add('prepare');
        };
        await controller.takeScreenshot(mode, showPointer: true);
        final action = switch (mode) {
          ScreenshotMode.region => 'Screenshot',
          ScreenshotMode.screen => 'ScreenshotScreen',
          ScreenshotMode.window => 'ScreenshotWindow',
        };
        expect(fixture.ipc.timeline, ['prepare', action]);
        expect(fixture.ipc.actions.single.$1, action);
        expect(fixture.ipc.actions.single.$2, <String, Object?>{
          'show_pointer': true,
          'path': null,
          if (mode != ScreenshotMode.region) 'write_to_disk': true,
          if (mode == ScreenshotMode.window) 'id': null,
        });
        expect(controller.panel, ShellPanel.none);
        expect(controller.busy, isNot(contains('screenshot')));
        expect(controller.error, isNull);
      },
    );
  }

  // Cover all capture commands, both Material anchors, every supported sample
  // menu height and the hidden menu's 2px input strip through real preparation.
  const nativeCases = [
    (
      label: 'Material top region',
      style: ShellStyle.material,
      position: BarPosition.top,
      menuHeight: 22.0,
      autoHide: false,
      mode: ScreenshotMode.region,
    ),
    (
      label: 'Material bottom window',
      style: ShellStyle.material,
      position: BarPosition.bottom,
      menuHeight: 22.0,
      autoHide: false,
      mode: ScreenshotMode.window,
    ),
    (
      label: 'macOS 22px screen',
      style: ShellStyle.macos,
      position: BarPosition.top,
      menuHeight: 22.0,
      autoHide: false,
      mode: ScreenshotMode.screen,
    ),
    (
      label: 'macOS 32px region',
      style: ShellStyle.macos,
      position: BarPosition.top,
      menuHeight: 32.0,
      autoHide: false,
      mode: ScreenshotMode.region,
    ),
    (
      label: 'macOS 48px window',
      style: ShellStyle.macos,
      position: BarPosition.top,
      menuHeight: 48.0,
      autoHide: false,
      mode: ScreenshotMode.window,
    ),
    (
      label: 'macOS 48px auto-hidden screen',
      style: ShellStyle.macos,
      position: BarPosition.top,
      menuHeight: 48.0,
      autoHide: true,
      mode: ScreenshotMode.screen,
    ),
  ];
  for (final configuration in nativeCases) {
    testWidgets(
      '${configuration.label} captures once after collapsing a full-output surface',
      (tester) async {
        final fixture = _Fixture();
        final controller = fixture.controller
          ..setShellStyle(configuration.style)
          ..setBarPosition(configuration.position)
          ..setMacPreferences(
            MacShellPreferences(
              menuBarHeight: configuration.menuHeight,
              menuBarAutoHide: configuration.autoHide,
            ),
          )
          ..setReduceMotion(true);
        await _mount(tester, controller);
        final starting = controller.start();
        await _frames(tester);
        await starting;
        controller.showBarTooltip(Object(), '截图', const Offset(100, 20));
        await _frames(tester);
        expect(native.tooltipVisible, isTrue);
        expect(native.state['height'], 800);

        controller.selectPanel(ShellPanel.screenshot);
        await _frames(tester);
        expect(native.expanded, isTrue);
        expect(native.tooltipVisible, isFalse);
        if (configuration.autoHide) controller.setMacMenuBarVisible(false);

        final expectedInputHeight = configuration.autoHide
            ? 2
            : configuration.style == ShellStyle.macos
            ? configuration.menuHeight.toInt()
            : 40;
        final expectedAction = switch (configuration.mode) {
          ScreenshotMode.region => 'Screenshot',
          ScreenshotMode.screen => 'ScreenshotScreen',
          ScreenshotMode.window => 'ScreenshotWindow',
        };
        final dispatchStates = <Map<String, Object?>>[];
        // Assertions inside the dispatch callback would be caught by the
        // controller's production error handler. Inspect the snapshot outside
        // that handler so a failed matcher reports the actual native state.
        fixture.ipc.onAction = (name, _) => dispatchStates.add({
          ...native.state,
          'action': name,
          'panel': controller.panel,
        });
        var completed = false;
        final capture = controller
            .takeScreenshot(configuration.mode, showPointer: true)
            .whenComplete(() => completed = true);
        await _frames(tester);
        // Polling preparation uses 16ms timers without scheduling a frame.
        // Keep driving those timers, but bound the test independently of the
        // production wall-clock deadline (fake time does not advance it).
        for (var turn = 0; turn < 200 && !completed; turn++) {
          await tester.pump(const Duration(milliseconds: 16));
        }
        if (!completed) {
          final diagnostic =
              'timeline=${fixture.ipc.timeline}, native=${native.state}, '
              'error=${controller.error}';
          // Unmounting disposes the controller and lets the next polling turn
          // finish without publishing an action or leaving a pending timer.
          await tester.pumpWidget(const SizedBox.shrink());
          await tester.pump(const Duration(milliseconds: 16));
          await capture;
          fail(
            'Screenshot preparation did not complete in 200 turns: '
            '$diagnostic',
          );
        }
        await capture;

        expect(
          controller.error,
          isNull,
          reason: 'timeline=${fixture.ipc.timeline}, native=${native.state}',
        );
        expect(fixture.ipc.actions, hasLength(1));
        expect(dispatchStates, hasLength(1));
        final dispatched = dispatchStates.single;
        expect(dispatched['action'], expectedAction);
        expect(dispatched['expanded'], isFalse);
        expect(dispatched['tooltipVisible'], isFalse);
        expect(dispatched['height'], 800);
        expect(dispatched['inputHeight'], expectedInputHeight);
        expect(dispatched['keyboard'], 'None');
        expect(dispatched['layer'], 'Top');
        expect(dispatched['panel'], ShellPanel.none);
        expect(fixture.ipc.actions.single.$1, expectedAction);
        expect(fixture.ipc.actions.single.$2, <String, Object?>{
          'show_pointer': true,
          'path': null,
          if (configuration.mode != ScreenshotMode.region)
            'write_to_disk': true,
          if (configuration.mode == ScreenshotMode.window) 'id': null,
        });
        expect(controller.busy, isNot(contains('screenshot')));
        expect(tester.takeException(), isNull);
      },
    );
  }

  test(
    'failed preparation returns the screenshot panel with actionable error feedback',
    () async {
      final fixture = _Fixture();
      final controller = fixture.controller;
      addTearDown(controller.dispose);
      await controller.start();
      controller.selectPanel(ShellPanel.screenshot);
      controller.prepareScreenshot = () async =>
          throw TimeoutException('surface is still modal');
      await controller.takeScreenshot(ScreenshotMode.screen);
      expect(fixture.ipc.actions, isEmpty);
      expect(controller.panel, ShellPanel.screenshot);
      expect(controller.error, contains('surface is still modal'));
      expect(controller.busy, isNot(contains('screenshot')));
    },
  );

  for (final reopen in [true, false]) {
    test(
      'pending screenshot is canceled by ${reopen ? 'reopening a panel' : 'Escape while already collapsed'}',
      () async {
        final fixture = _Fixture();
        final controller = fixture.controller;
        addTearDown(controller.dispose);
        await controller.start();
        final gate = Completer<void>();
        controller.prepareScreenshot = () => gate.future;
        final capture = controller.takeScreenshot(ScreenshotMode.region);
        expect(controller.busy, contains('screenshot'));
        if (reopen) {
          controller.selectPanel(ShellPanel.launcher);
        } else {
          controller.closePanel();
        }
        gate.complete();
        await capture;
        expect(fixture.ipc.actions, isEmpty);
        expect(
          controller.panel,
          reopen ? ShellPanel.launcher : ShellPanel.none,
        );
        expect(controller.error, isNull);
        expect(controller.busy, isNot(contains('screenshot')));
      },
    );
  }

  test(
    'canceled preparation cannot publish its late failure into another panel',
    () async {
      final fixture = _Fixture();
      final controller = fixture.controller;
      addTearDown(controller.dispose);
      await controller.start();
      final gate = Completer<void>();
      controller.prepareScreenshot = () => gate.future;
      final capture = controller.takeScreenshot(ScreenshotMode.window);
      controller.selectPanel(ShellPanel.quickSettings);
      gate.completeError(StateError('obsolete prepare failure'));
      await capture;
      expect(fixture.ipc.actions, isEmpty);
      expect(controller.panel, ShellPanel.quickSettings);
      expect(controller.error, isNull);
    },
  );

  for (final reopen in [false, true]) {
    test(
      'sent screenshot late IPC failure is ignored after ${reopen ? 'opening another panel' : 'closing the panel'}',
      () async {
        final fixture = _Fixture();
        final controller = fixture.controller;
        addTearDown(controller.dispose);
        await controller.start();
        controller.selectPanel(ShellPanel.screenshot);
        final gate = Completer<void>();
        fixture.ipc.actionGate = gate;
        final capture = controller.takeScreenshot(ScreenshotMode.screen);
        await fixture.ipc.actionStarted.future;
        expect(fixture.ipc.actions.single.$1, 'ScreenshotScreen');
        expect(controller.busy, contains('screenshot'));
        if (reopen) {
          controller.selectPanel(ShellPanel.quickSettings);
        } else {
          controller.closePanel();
        }
        gate.completeError(StateError('obsolete IPC failure'));
        await capture;
        expect(fixture.ipc.actions, hasLength(1));
        expect(
          controller.panel,
          reopen ? ShellPanel.quickSettings : ShellPanel.none,
        );
        expect(controller.error, isNull);
        expect(controller.busy, isNot(contains('screenshot')));
      },
    );
  }

  test(
    'current screenshot IPC failure restores the panel and allows a successful retry',
    () async {
      final fixture = _Fixture();
      final controller = fixture.controller;
      addTearDown(controller.dispose);
      await controller.start();
      controller.selectPanel(ShellPanel.screenshot);
      final gate = Completer<void>();
      fixture.ipc.actionGate = gate;
      final capture = controller.takeScreenshot(ScreenshotMode.screen);
      await fixture.ipc.actionStarted.future;
      expect(controller.panel, ShellPanel.none);
      gate.completeError(StateError('current IPC failure'));
      await capture;
      expect(controller.panel, ShellPanel.screenshot);
      expect(controller.error, contains('current IPC failure'));
      expect(controller.busy, isNot(contains('screenshot')));
      expect(fixture.ipc.actions, hasLength(1));

      fixture.ipc.actionGate = null;
      await controller.takeScreenshot(ScreenshotMode.screen);
      expect(fixture.ipc.actions, hasLength(2));
      expect(controller.panel, ShellPanel.none);
      expect(controller.error, isNull);
      expect(controller.busy, isNot(contains('screenshot')));
    },
  );

  test(
    'disposing a pending screenshot prevents compositor actions and late notifications',
    () async {
      final fixture = _Fixture();
      final controller = fixture.controller;
      await controller.start();
      final gate = Completer<void>();
      controller.prepareScreenshot = () => gate.future;
      var notifications = 0;
      controller.addListener(() => notifications++);
      final capture = controller.takeScreenshot(ScreenshotMode.screen);
      controller.dispose();
      final afterDispose = notifications;
      gate.complete();
      await capture;
      expect(fixture.ipc.actions, isEmpty);
      expect(fixture.ipc.closed, isTrue);
      expect(notifications, afterDispose);
    },
  );

  test('double clicking capture prepares and dispatches only once', () async {
    final fixture = _Fixture();
    final controller = fixture.controller;
    addTearDown(controller.dispose);
    await controller.start();
    final gate = Completer<void>();
    var prepared = 0;
    controller.prepareScreenshot = () {
      prepared++;
      return gate.future;
    };
    final first = controller.takeScreenshot(ScreenshotMode.region);
    await controller.takeScreenshot(ScreenshotMode.region);
    expect(prepared, 1);
    expect(fixture.ipc.actions, isEmpty);
    gate.complete();
    await first;
    expect(fixture.ipc.actions, hasLength(1));
    expect(controller.busy, isNot(contains('screenshot')));
  });

  test(
    'demo captures neither call native preparation nor send compositor actions',
    () async {
      final fixture = _Fixture(demo: true);
      final controller = fixture.controller;
      addTearDown(controller.dispose);
      var prepared = 0;
      controller.prepareScreenshot = () async {
        prepared++;
      };
      controller.selectPanel(ShellPanel.screenshot);
      for (final mode in ScreenshotMode.values) {
        await controller.takeScreenshot(mode);
      }
      expect(prepared, 0);
      expect(fixture.ipc.actions, isEmpty);
      expect(controller.panel, ShellPanel.none);
    },
  );

  testWidgets(
    'launcher reverses from its current opacity and retains the native surface until exit',
    (tester) async {
      final controller = _Fixture(demo: true).controller;
      await _mount(tester, controller);
      native.changes.clear();
      controller.selectPanel(ShellPanel.launcher);
      await _frames(tester);
      await tester.pump(const Duration(milliseconds: 50));
      final openingOpacity = _opacity(tester);
      expect(openingOpacity, greaterThan(0));
      expect(openingOpacity, lessThan(1));
      expect(native.expanded, isTrue);

      controller.closePanel();
      await tester.pump();
      expect(_opacity(tester), closeTo(openingOpacity, 0.0001));
      await tester.pump(const Duration(milliseconds: 8));
      final closingOpacity = _opacity(tester);
      expect(closingOpacity, greaterThan(0));
      expect(closingOpacity, lessThan(openingOpacity));
      expect(native.expanded, isTrue);
      final pointer = tester.widget<IgnorePointer>(
        find
            .descendant(
              of: find.byType(LauncherSurface),
              matching: find.byType(IgnorePointer),
            )
            .first,
      );
      expect(pointer.ignoring, isTrue);

      controller.selectPanel(ShellPanel.launcher);
      await tester.pump();
      expect(_opacity(tester), closeTo(closingOpacity, 0.0001));
      await tester.pump(const Duration(milliseconds: 250));
      await _frames(tester);
      expect(_opacity(tester), 1);
      expect(native.expanded, isTrue);
      expect(native.changes, isNot(contains(('setExpanded', false))));

      controller.closePanel();
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 40));
      expect(_opacity(tester), greaterThan(0));
      expect(native.expanded, isTrue);
      await tester.pump(const Duration(milliseconds: 200));
      await _frames(tester);
      expect(
        find.descendant(
          of: find.byType(LauncherSurface),
          matching: find.byType(Opacity),
        ),
        findsNothing,
      );
      expect(native.expanded, isFalse);
      expect(native.tooltipVisible, isFalse);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('switching panels during launcher exit preserves the new modal', (
    tester,
  ) async {
    final controller = _Fixture(demo: true).controller;
    await _mount(tester, controller);
    controller.selectPanel(ShellPanel.launcher);
    await _frames(tester);
    await tester.pump(const Duration(milliseconds: 250));
    controller.selectPanel(ShellPanel.quickSettings);
    await tester.pump();
    expect(native.expanded, isTrue);
    await tester.pump(const Duration(milliseconds: 200));
    await _frames(tester);
    expect(controller.panel, ShellPanel.quickSettings);
    expect(native.expanded, isTrue);
    controller.selectPanel(ShellPanel.screenshot);
    await _frames(tester);
    expect(native.expanded, isTrue);
    controller.closePanel();
    await _frames(tester);
    expect(native.expanded, isFalse);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'enabling reduced motion during exit finishes and releases immediately',
    (tester) async {
      final controller = _Fixture(demo: true).controller;
      await _mount(tester, controller);
      controller.selectPanel(ShellPanel.launcher);
      await _frames(tester);
      await tester.pump(const Duration(milliseconds: 250));
      controller.closePanel();
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 40));
      expect(native.expanded, isTrue);
      expect(_opacity(tester), greaterThan(0));
      controller.setReduceMotion(true);
      await _frames(tester);
      expect(
        find.descendant(
          of: find.byType(LauncherSurface),
          matching: find.byType(Opacity),
        ),
        findsNothing,
      );
      expect(native.expanded, isFalse);
      await tester.pump(const Duration(milliseconds: 500));
      expect(native.expanded, isFalse);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'launcher disposal cancels animation and queued native transitions',
    (tester) async {
      final controller = _Fixture(demo: true).controller;
      await _mount(tester, controller);
      native.pauseNextExpanded = true;
      controller.selectPanel(ShellPanel.launcher);
      await _frames(tester);
      await tester.pump(const Duration(milliseconds: 30));
      expect(native.pausedExpanded, isNotNull);
      final beforeDispose = native.changes.length;
      await tester.pumpWidget(const SizedBox.shrink());
      native.release();
      await _frames(tester);
      await tester.pump(const Duration(milliseconds: 500));
      expect(native.changes.length, beforeDispose);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'a stalled native window query times out preparation without an IPC capture',
    (tester) async {
      final fixture = _Fixture();
      final controller = fixture.controller;
      await _mount(tester, controller);
      final starting = controller.start();
      await _frames(tester);
      await starting;
      native.queryGate = Completer<Map<String, dynamic>>();
      final capture = controller.takeScreenshot(ScreenshotMode.region);
      await _frames(tester);
      expect(controller.busy, contains('screenshot'));
      await tester.pump(const Duration(seconds: 3));
      await _frames(tester);
      await capture;
      expect(fixture.ipc.actions, isEmpty);
      expect(controller.busy, isNot(contains('screenshot')));
      expect(controller.panel, ShellPanel.screenshot);
      expect(controller.error, contains('截图界面尚未准备好'));
      native.release();
      await _frames(tester);
      expect(fixture.ipc.actions, isEmpty);
      expect(tester.takeException(), isNull);
    },
  );
}
