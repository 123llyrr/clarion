// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/main.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/preferences.dart';
import 'package:clarion_shell/state/shell_controller.dart';

const _channel = MethodChannel('clarion.shell/window');

class _Controller extends ShellController {
  _Controller() : super(demo: true);

  bool animate = false;
  @override
  bool get reduceMotion => !animate;
}

/// Delays a native operation's reply while retaining the arguments originally
/// received, so an obsolete queued request would change the simulated window.
class _NativeWindow {
  final calls = <(String, bool)>[];
  final methods = <String>[];
  bool expanded = false;
  bool tooltipVisible = false;
  bool macos = false;
  Rect? panelGlass;
  final glassRequests = <Rect>[];
  String? holdNext;
  Completer<void>? _reply;

  Future<Object?> handle(MethodCall call) async {
    methods.add(call.method);
    if (call.method == 'setShellStyle') {
      final next = call.arguments == 'macos';
      if (next != macos) {
        expanded = false;
        panelGlass = null;
      }
      macos = next;
      return null;
    }
    if (call.method == 'setLauncherBackdrop') {
      if ((call.arguments as Map)['visible'] == true) panelGlass = null;
      return null;
    }
    if (call.method == 'setPanelGlassRegion') {
      final arguments = call.arguments as Map;
      final region = Rect.fromLTWH(
        (arguments['x'] as num).toDouble(),
        (arguments['y'] as num).toDouble(),
        (arguments['width'] as num).toDouble(),
        (arguments['height'] as num).toDouble(),
      );
      glassRequests.add(region);
      // Match the runner: an early report succeeds but is not retained.
      panelGlass = macos && expanded && !region.isEmpty ? region : null;
      return null;
    }
    if (call.method != 'setExpanded' && call.method != 'setTooltipVisible') {
      return null;
    }
    final value = call.arguments as bool;
    calls.add((call.method, value));
    if (holdNext == call.method) {
      holdNext = null;
      _reply = Completer<void>();
      await _reply!.future;
    }
    if (call.method == 'setExpanded') {
      expanded = value;
      if (expanded) tooltipVisible = false;
      if (!expanded) panelGlass = null;
    } else {
      tooltipVisible = !expanded && value;
    }
    return null;
  }

  bool get waiting => _reply != null && !_reply!.isCompleted;

  void release() {
    if (waiting) _reply!.complete();
  }
}

Future<void> _mount(
  WidgetTester tester,
  ShellController controller, {
  bool preview = false,
}) async {
  tester.view.devicePixelRatio = 1;
  tester.view.physicalSize = const Size(1200, 800);
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  await tester.pumpWidget(
    ShellApplication(
      controller: controller,
      preferences: const ShellPreferences(),
      preview: preview,
      persist: false,
    ),
  );
  await _flush(tester);
}

Future<void> _flush(WidgetTester tester) async {
  // Channel responses and the serialized window queue can finish without
  // scheduling a visual frame. Pump each microtask turn explicitly.
  for (var turn = 0; turn < 8; turn++) {
    await tester.pump();
  }
  await tester.pumpAndSettle();
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  test('late exit from tooltip A preserves the newer tooltip B', () {
    final controller = _Controller();
    addTearDown(controller.dispose);
    final first = Object();
    final second = Object();
    var notifications = 0;
    controller.addListener(() => notifications++);

    controller.showBarTooltip(first, '控制中心', const Offset(1100, 20));
    controller.showBarTooltip(second, '通知', const Offset(1064, 20));
    controller.hideBarTooltip(first);
    expect(identical(controller.barTooltip?.owner, second), isTrue);
    expect(controller.barTooltip?.message, '通知');
    expect(controller.barTooltip?.anchor, const Offset(1064, 20));
    expect(notifications, 2);

    controller.hideBarTooltip(second);
    expect(controller.barTooltip, isNull);
    expect(notifications, 3);
  });

  test(
    'panels take priority and closePanel clears a tooltip without a panel',
    () {
      final controller = _Controller();
      addTearDown(controller.dispose);
      final owner = Object();
      controller.showBarTooltip(owner, '控制中心', const Offset(1100, 20));
      controller.selectPanel(ShellPanel.quickSettings);
      expect(controller.barTooltip, isNull);
      controller.showBarTooltip(owner, '不能遮住面板', const Offset(1100, 20));
      expect(controller.barTooltip, isNull);
      expect(controller.panel, ShellPanel.quickSettings);

      controller.closePanel();
      controller.showBarTooltip(owner, '通知', const Offset(1064, 20));
      expect(controller.barTooltip, isNotNull);
      controller.closePanel();
      expect(controller.panel, ShellPanel.none);
      expect(controller.barTooltip, isNull);

      controller.showBarTooltip(owner, '', const Offset(1064, 20));
      controller.showBarTooltip(owner, '非法位置', const Offset(double.nan, 20));
      expect(controller.barTooltip, isNull);
    },
  );

  test('disposed controller ignores late hover and exit callbacks', () {
    final controller = _Controller();
    final owner = Object();
    controller.showBarTooltip(owner, '通知', const Offset(1064, 20));
    controller.dispose();
    expect(controller.barTooltip, isNull);
    expect(() {
      controller.showBarTooltip(owner, '迟到悬停', const Offset(1064, 20));
      controller.hideBarTooltip(owner);
      controller.dismissBarTooltip();
      controller.closePanel();
      controller.selectPanel(ShellPanel.quickSettings);
    }, returnsNormally);
    expect(controller.barTooltip, isNull);
    expect(controller.panel, ShellPanel.none);
  });

  group('serialized native window state', () {
    late _NativeWindow native;

    setUp(() {
      native = _NativeWindow();
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(_channel, native.handle);
    });

    tearDown(() {
      native.release();
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(_channel, null);
    });

    testWidgets('panel glass waits for expansion and clears on close', (
      tester,
    ) async {
      final controller = _Controller()..setShellStyle(ShellStyle.macos);
      await _mount(tester, controller);
      native.glassRequests.clear();
      native.holdNext = 'setExpanded';
      controller.selectPanel(ShellPanel.quickSettings);
      await _flush(tester);
      expect(native.waiting, isTrue);
      expect(native.glassRequests.where((rect) => !rect.isEmpty), isEmpty);
      native.release();
      await _flush(tester);
      expect(
        native.panelGlass,
        tester.getRect(find.byKey(const ValueKey('mac-control-center-glass'))),
      );
      controller.closePanel();
      await _flush(tester);
      expect(native.panelGlass, isNull);
      expect(native.glassRequests.last.isEmpty, isTrue);
      expect(tester.takeException(), isNull);
    });

    testWidgets('same-bounds reopen restores glass after delayed collapse', (
      tester,
    ) async {
      final controller = _Controller()..setShellStyle(ShellStyle.macos);
      await _mount(tester, controller);
      controller.selectPanel(ShellPanel.quickSettings);
      await _flush(tester);
      final initial = native.panelGlass;
      expect(initial, isNotNull);
      native.glassRequests.clear();
      native.holdNext = 'setExpanded';
      controller.closePanel();
      // Drain messages without painting a closed frame: the child and its
      // geometry cache survive this close/reopen, while native clears glass.
      await tester.idle();
      expect(native.waiting, isTrue);
      controller.selectPanel(ShellPanel.quickSettings);
      native.release();
      await _flush(tester);
      expect(native.expanded, isTrue);
      expect(native.panelGlass, initial);
      expect(native.glassRequests.where((rect) => !rect.isEmpty), [initial]);
      expect(tester.takeException(), isNull);
    });

    testWidgets('late native collapse schedules its own final clear frame', (
      tester,
    ) async {
      final controller = _Controller()..setShellStyle(ShellStyle.macos);
      await _mount(tester, controller);
      controller.selectPanel(ShellPanel.launcher);
      await _flush(tester);
      native.holdNext = 'setExpanded';
      controller.closePanel();
      await _flush(tester);
      expect(native.waiting, isTrue);
      expect(tester.binding.hasScheduledFrame, isFalse);
      native.release();
      await tester.idle();
      expect(native.expanded, isFalse);
      expect(
        tester.binding.hasScheduledFrame,
        isTrue,
        reason:
            'Native effect cleanup must repaint without input or a clock tick',
      );
      await _flush(tester);
      expect(tester.binding.hasScheduledFrame, isFalse);
      expect(tester.binding.transientCallbackCount, 0);
    });

    testWidgets('closing during pending expansion never publishes old glass', (
      tester,
    ) async {
      final controller = _Controller()..setShellStyle(ShellStyle.macos);
      await _mount(tester, controller);
      native.glassRequests.clear();
      native.holdNext = 'setExpanded';
      controller.selectPanel(ShellPanel.quickSettings);
      await _flush(tester);
      controller.closePanel();
      native.release();
      await _flush(tester);
      expect(native.expanded, isFalse);
      expect(native.panelGlass, isNull);
      expect(native.glassRequests.where((rect) => !rect.isEmpty), isEmpty);
      expect(tester.takeException(), isNull);
    });

    testWidgets(
      'position-only relayout moves glass without resizing the panel',
      (tester) async {
        final controller = _Controller()..setShellStyle(ShellStyle.macos);
        await _mount(tester, controller);
        controller.selectPanel(ShellPanel.quickSettings);
        await _flush(tester);
        final initial = native.panelGlass!;
        native.glassRequests.clear();
        controller.setMacPreferences(
          controller.macPreferences.copyWith(menuBarWidthPercent: 80),
        );
        await _flush(tester);
        final actual = tester.getRect(
          find.byKey(const ValueKey('mac-control-center-glass')),
        );
        expect(actual.size, initial.size);
        expect(actual.left, lessThan(initial.left));
        expect(native.panelGlass, actual);
        expect(native.glassRequests.where((rect) => !rect.isEmpty), [actual]);
        expect(tester.takeException(), isNull);
      },
    );

    testWidgets('entry animation reports final glass bounds once', (
      tester,
    ) async {
      final controller = _Controller()
        ..animate = true
        ..setShellStyle(ShellStyle.macos);
      await _mount(tester, controller);
      native.glassRequests.clear();
      controller.selectPanel(ShellPanel.quickSettings);
      await tester.pump();
      await tester.idle();
      await tester.pump(const Duration(milliseconds: 40));
      await _flush(tester);
      final actual = tester.getRect(
        find.byKey(const ValueKey('mac-control-center-glass')),
      );
      expect(native.glassRequests.where((rect) => !rect.isEmpty), [actual]);
      expect(tester.binding.transientCallbackCount, 0);
      expect(tester.takeException(), isNull);
    });

    testWidgets('hover opens only the drawing area and leaving returns it', (
      tester,
    ) async {
      final controller = _Controller();
      await _mount(tester, controller);
      final owner = Object();
      controller.showBarTooltip(owner, '控制中心', const Offset(1100, 20));
      await _flush(tester);
      expect(native.expanded, isFalse);
      expect(native.tooltipVisible, isTrue);

      controller.hideBarTooltip(owner);
      await _flush(tester);
      expect(native.expanded, isFalse);
      expect(native.tooltipVisible, isFalse);
      expect(tester.takeException(), isNull);
    });

    testWidgets('Escape and rapid hover beat a delayed panel-open reply', (
      tester,
    ) async {
      final controller = _Controller();
      await _mount(tester, controller);
      native.calls.clear();
      native.holdNext = 'setExpanded';
      controller.selectPanel(ShellPanel.quickSettings);
      await _flush(tester);
      expect(native.waiting, isTrue);
      expect(native.calls, [('setExpanded', true)]);

      final first = Object();
      final second = Object();
      controller.showBarTooltip(first, '面板打开期间悬停', const Offset(1100, 20));
      expect(controller.barTooltip, isNull);
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      expect(controller.panel, ShellPanel.none);
      controller.showBarTooltip(first, '控制中心', const Offset(1100, 20));
      controller.showBarTooltip(second, '通知', const Offset(1064, 20));
      controller.hideBarTooltip(first);
      expect(identical(controller.barTooltip?.owner, second), isTrue);
      controller.hideBarTooltip(second);
      native.release();
      await _flush(tester);

      expect(native.expanded, isFalse);
      expect(native.tooltipVisible, isFalse);
      expect(
        native.calls
            .where((call) => call.$1 == 'setTooltipVisible')
            .every((call) => !call.$2),
        isTrue,
      );
      expect(tester.takeException(), isNull);
    });

    testWidgets('a delayed tooltip reply cannot restore hover over a menu', (
      tester,
    ) async {
      final controller = _Controller();
      await _mount(tester, controller);
      native.holdNext = 'setTooltipVisible';
      final owner = Object();
      controller.showBarTooltip(owner, '托盘项目', const Offset(1000, 20));
      await _flush(tester);
      expect(native.waiting, isTrue);

      await controller.openTrayMenu(controller.trayItems.first);
      expect(controller.barTooltip, isNull);
      native.release();
      await _flush(tester);
      expect(native.expanded, isTrue);
      expect(native.tooltipVisible, isFalse);
      expect(controller.panel, ShellPanel.trayMenu);

      controller.closePanel();
      await _flush(tester);
      expect(native.expanded, isFalse);
      expect(native.tooltipVisible, isFalse);
      expect(tester.takeException(), isNull);
    });

    testWidgets(
      'unmounting while a native reply waits stops subsequent calls',
      (tester) async {
        final controller = _Controller();
        await _mount(tester, controller);
        native.calls.clear();
        native.holdNext = 'setTooltipVisible';
        final owner = Object();
        controller.showBarTooltip(owner, '控制中心', const Offset(1100, 20));
        await _flush(tester);
        expect(native.waiting, isTrue);
        await tester.pumpWidget(const SizedBox.shrink());
        native.release();
        await _flush(tester);
        expect(native.calls, [('setTooltipVisible', true)]);
        expect(controller.barTooltip, isNull);
        expect(tester.takeException(), isNull);
      },
    );

    testWidgets('panel toggles send only changed native configuration', (
      tester,
    ) async {
      final controller = _Controller()..setShellStyle(ShellStyle.macos);
      await _mount(tester, controller);
      native.methods.clear();
      for (var cycle = 0; cycle < 3; cycle++) {
        controller.selectPanel(ShellPanel.quickSettings);
        await _flush(tester);
        controller.closePanel();
        await _flush(tester);
      }
      expect(native.methods.where((m) => m == 'setExpanded').length, 6);
      for (final unchanged in [
        'setShellStyle',
        'setBarPosition',
        'setMacMenuBarPreferences',
        'setLauncherBackdrop',
      ]) {
        expect(native.methods, isNot(contains(unchanged)));
      }
      expect(native.panelGlass, isNull);
      expect(tester.binding.transientCallbackCount, 0);
    });

    testWidgets(
      'control exit retains glass and reverses without recreating content',
      (tester) async {
        final controller = _Controller()
          ..setShellStyle(ShellStyle.macos)
          ..animate = true;
        await _mount(tester, controller);
        controller.selectPanel(ShellPanel.quickSettings);
        await _flush(tester);
        final panel = find.byKey(const ValueKey('mac-control-center-capture'));
        final original = tester.element(panel);
        final glass = native.panelGlass;
        controller.closePanel();
        await tester.pump();
        await tester.pump(const Duration(milliseconds: 32));
        expect(native.expanded, isTrue);
        expect(native.panelGlass, glass);
        expect(tester.element(panel), same(original));
        controller.selectPanel(ShellPanel.quickSettings);
        await tester.pump();
        expect(tester.element(panel), same(original));
        await _flush(tester);
        controller.closePanel();
        await _flush(tester);
        expect(panel, findsNothing);
        expect(native.expanded, isFalse);
        expect(native.panelGlass, isNull);
        expect(tester.binding.transientCallbackCount, 0);
        // Replacement panels never inherit a fading control-center blur.
        controller.selectPanel(ShellPanel.quickSettings);
        await _flush(tester);
        controller.selectPanel(ShellPanel.volume);
        await tester.pump();
        await tester.idle();
        expect(panel, findsNothing);
        expect(native.panelGlass, isNull);
      },
    );

    testWidgets('preview hover and panels never change native layer state', (
      tester,
    ) async {
      final controller = _Controller();
      await _mount(tester, controller, preview: true);
      final owner = Object();
      controller.showBarTooltip(owner, '控制中心', const Offset(1100, 20));
      await _flush(tester);
      controller.selectPanel(ShellPanel.quickSettings);
      await _flush(tester);
      controller.closePanel();
      await _flush(tester);
      expect(native.calls, isEmpty);
      expect(tester.takeException(), isNull);
    });

    testWidgets(
      'native geometry dismissal clears hover and preserves an open panel',
      (tester) async {
        final controller = _Controller();
        await _mount(tester, controller);
        final owner = Object();
        controller.showBarTooltip(owner, '控制中心', const Offset(1100, 20));
        await _flush(tester);
        expect(native.tooltipVisible, isTrue);

        Future<Object?> sendDismissal() => TestDefaultBinaryMessengerBinding
            .instance
            .defaultBinaryMessenger
            .handlePlatformMessage(
              _channel.name,
              const StandardMethodCodec().encodeMethodCall(
                const MethodCall('dismissBarTooltip'),
              ),
              null,
            );
        final first = sendDismissal();
        await _flush(tester);
        await first;
        expect(controller.barTooltip, isNull);
        expect(native.tooltipVisible, isFalse);

        controller.selectPanel(ShellPanel.quickSettings);
        await _flush(tester);
        final whileOpen = sendDismissal();
        await _flush(tester);
        await whileOpen;
        expect(controller.panel, ShellPanel.quickSettings);
        expect(native.expanded, isTrue);
        expect(native.tooltipVisible, isFalse);
        expect(tester.takeException(), isNull);
      },
    );
  });
}
