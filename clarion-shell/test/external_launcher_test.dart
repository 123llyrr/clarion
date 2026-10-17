// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/main.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/preferences.dart';
import 'package:clarion_shell/services/launcher_bridge.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/ui/mac_launcher_panel.dart';
import 'package:clarion_shell/ui/quick_settings_panel.dart';

class _Bridge extends LauncherBridge {
  final replies = StreamController<LauncherEvent>.broadcast(sync: true);
  final calls = <(String, int)>[];
  final outputs = <String?>[];
  final configs = <(String, List<String>)>[];
  final order = <String>[];
  bool failShow = false;
  Completer<void>? holdShow;
  @override
  Stream<LauncherEvent> get events => replies.stream;
  @override
  Future<void> show({
    required int requestId,
    String? output,
    required String theme,
    required String accent,
    required List<String> recent,
  }) async {
    calls.add(('Show', requestId));
    outputs.add(output);
    order.add('Show');
    if (failShow) throw StateError('fixture unavailable');
    await holdShow?.future;
  }

  @override
  Future<void> hide(int requestId) async {
    calls.add(('Hide', requestId));
    order.add('Hide');
  }

  @override
  Future<void> preferences({
    required int requestId,
    required String theme,
    required String accent,
    required List<String> recent,
  }) async {
    configs.add((theme, recent));
  }

  @override
  Future<void> dispose() async {
    await replies.close();
    await super.dispose();
  }
}

ShellController _controller(_Bridge bridge) =>
    ShellController(
        demo: true,
        launcherBridge: bridge,
        notifications: false,
        tray: false,
        powerProfilesEnabled: false,
      )
      ..setShellStyle(ShellStyle.macos)
      ..setReduceMotion(true);

Future<void> _flush(WidgetTester tester) async {
  for (var i = 0; i < 8; i++) {
    await tester.pump();
  }
  await tester.pumpAndSettle();
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  test('old preferences enable new launcher; explicit false round trips', () {
    expect(ShellPreferences.fromJson({}).useExternalLauncher, isTrue);
    const legacy = ShellPreferences(useExternalLauncher: false);
    expect(
      ShellPreferences.fromJson(legacy.toJson()).useExternalLauncher,
      isFalse,
    );
  });
  test(
    'stale launch updates recents without closing a newer launcher',
    () async {
      final bridge = _Bridge();
      final controller = _controller(bridge);
      controller.selectPanel(ShellPanel.launcher);
      final old = controller.externalLauncherRequest;
      controller.closePanel();
      controller.selectPanel(ShellPanel.launcher);
      final current = controller.externalLauncherRequest;
      bridge.replies.add(LauncherEvent('Launched', old, appId: 'late.desktop'));
      bridge.replies.add(LauncherEvent('Hidden', old));
      expect(controller.recentAppIds.first, 'late.desktop');
      expect(controller.panel, ShellPanel.launcher);
      bridge.replies.add(
        LauncherEvent(
          'Error',
          current,
          context: 'launch',
          message: 'launch rejected',
        ),
      );
      expect(controller.externalLauncherDesired, isTrue);
      expect(controller.error, contains('launch rejected'));
      bridge.replies.add(LauncherEvent('Hidden', current));
      expect(controller.panel, ShellPanel.none);
      controller.dispose();
      await Future<void>.delayed(Duration.zero);
    },
  );

  test(
    'window/process failure keeps launcher panel and restores Flutter',
    () async {
      final bridge = _Bridge();
      final controller = _controller(bridge);
      controller.selectPanel(ShellPanel.launcher);
      bridge.replies.add(
        LauncherEvent(
          'Error',
          controller.externalLauncherRequest,
          context: 'window',
          message: 'GPU failure',
        ),
      );
      expect(controller.panel, ShellPanel.launcher);
      expect(controller.externalLauncherEnabled, isFalse);
      expect(controller.error, contains('已恢复 Flutter'));
      controller.setUseExternalLauncher(false);
      controller.setUseExternalLauncher(true);
      expect(controller.externalLauncherDesired, isTrue);
      controller.dispose();
      await Future<void>.delayed(Duration.zero);
    },
  );

  testWidgets(
    'native keyboard/backdrop commits before Show; close is independent of startup',
    (tester) async {
      final bridge = _Bridge()..holdShow = Completer<void>();
      final controller = _controller(bridge);
      final native = <String>[];
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(
            const MethodChannel('clarion.shell/window'),
            (call) async {
              final entry = '${call.method}:${call.arguments}';
              native.add(entry);
              bridge.order.add(entry);
              if (call.method == 'outputName') return 'winit';
              return null;
            },
          );
      addTearDown(
        () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
            .setMockMethodCallHandler(
              const MethodChannel('clarion.shell/window'),
              null,
            ),
      );
      tester.view.devicePixelRatio = 1;
      tester.view.physicalSize = const Size(1200, 900);
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      await tester.pumpWidget(
        ShellApplication(
          controller: controller,
          preferences: const ShellPreferences(),
          persist: false,
        ),
      );
      await _flush(tester);
      controller.selectPanel(ShellPanel.launcher);
      await _flush(tester);
      expect(find.byType(MacLauncherPanel), findsNothing);
      expect(
        find.byKey(const ValueKey('external-launcher-backdrop')),
        findsOneWidget,
      );
      final show = bridge.order.indexOf('Show');
      expect(bridge.outputs, ['winit']);
      expect(
        show,
        greaterThan(bridge.order.indexOf('setKeyboardInteractivity:none')),
      );
      expect(show, greaterThan(bridge.order.indexOf('setExpanded:true')));
      expect(
        bridge.order
            .take(show)
            .any(
              (s) =>
                  s.startsWith('setLauncherBackdrop:') &&
                  s.contains('visible: true'),
            ),
        isTrue,
      );
      controller.setThemeMode(ThemeMode.light);
      controller.setRecentAppIds(['updated.desktop']);
      await _flush(tester);
      expect(bridge.configs.last.$1, 'light');
      expect(bridge.configs.last.$2, ['updated.desktop']);
      expect(bridge.calls.where((call) => call.$1 == 'Show'), hasLength(1));
      controller.closePanel();
      await _flush(tester);
      expect(native, contains('setExpanded:false'));
      expect(
        native.lastWhere((s) => s.startsWith('setKeyboardInteractivity:')),
        'setKeyboardInteractivity:default',
      );
      expect(bridge.calls.last.$1, 'Hide');
      bridge.holdShow!.complete();
      controller.selectPanel(ShellPanel.launcher);
      await _flush(tester);
      controller.setUseExternalLauncher(false);
      await _flush(tester);
      expect(find.byType(MacLauncherPanel), findsOneWidget);
      controller.setUseExternalLauncher(true);
      await _flush(tester);
      expect(find.byType(MacLauncherPanel), findsNothing);
      controller.closePanel();
      await _flush(tester);
      expect(
        native.lastWhere((s) => s.startsWith('setExpanded:')),
        'setExpanded:false',
      );
      expect(
        tester
            .widget<Opacity>(
              find.byKey(const ValueKey('mac-menu-bar-launchpad-visibility')),
            )
            .opacity,
        1,
        reason:
            'Switching launcher implementations must clear retained menu hiding',
      );
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets('startup failure mounts fallback and toggle is exposed', (
    tester,
  ) async {
    final bridge = _Bridge()..failShow = true;
    final controller = _controller(bridge);
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
          const MethodChannel('clarion.shell/window'),
          (_) async => null,
        );
    addTearDown(
      () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(
            const MethodChannel('clarion.shell/window'),
            null,
          ),
    );
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(
      ShellApplication(
        controller: controller,
        preferences: const ShellPreferences(),
        persist: false,
      ),
    );
    await _flush(tester);
    controller.selectPanel(ShellPanel.launcher);
    await _flush(tester);
    expect(controller.externalLauncherEnabled, isFalse);
    expect(find.byType(MacLauncherPanel), findsOneWidget);
    controller.selectPanel(ShellPanel.quickSettings);
    await _flush(tester);
    await tester.tap(
      find.byKey(const ValueKey('mac-control-desktop-preferences')),
    );
    await _flush(tester);
    expect(find.byKey(const ValueKey('use-external-launcher')), findsOneWidget);
    final settings = tester.widget<QuickSettingsPanel>(
      find.byType(QuickSettingsPanel),
    );
    expect(settings.controller, controller);
    final toggle = tester.widget<SwitchListTile>(
      find.byKey(const ValueKey('use-external-launcher')),
    );
    toggle.onChanged!(false);
    await _flush(tester);
    expect(controller.useExternalLauncher, isFalse);
    await tester.pumpWidget(const SizedBox.shrink());
  });
  testWidgets('reopen rearms None after a superseded native collapse', (
    tester,
  ) async {
    final bridge = _Bridge();
    final controller = _controller(bridge);
    final native = <String>[];
    Completer<void>? collapse;
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(const MethodChannel('clarion.shell/window'), (
          call,
        ) async {
          native.add('${call.method}:${call.arguments}');
          if (call.method == 'outputName') return 'winit';
          if (call.method == 'setExpanded' && call.arguments == false) {
            await collapse?.future;
          }
          return null;
        });
    addTearDown(
      () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(
            const MethodChannel('clarion.shell/window'),
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
    await _flush(tester);
    controller.selectPanel(ShellPanel.launcher);
    await _flush(tester);
    native.clear();
    collapse = Completer<void>();
    controller.closePanel();
    await _flush(tester);
    expect(native, contains('setExpanded:false'));
    controller.selectPanel(ShellPanel.launcher);
    await _flush(tester);
    collapse.complete();
    await _flush(tester);
    final collapsed = native.indexOf('setExpanded:false');
    final rearm = native.lastIndexOf('setKeyboardInteractivity:none');
    final reopened = native.lastIndexOf('setExpanded:true');
    expect(rearm, greaterThan(collapsed));
    expect(reopened, greaterThan(rearm));
    await tester.pumpWidget(const SizedBox.shrink());
  });
  testWidgets(
    'connected launcher without Visible reply falls back within a bound',
    (tester) async {
      final bridge = _Bridge();
      final controller = ShellController(
        demo: true,
        launcherBridge: bridge,
        launcherVisibleTimeout: const Duration(milliseconds: 80),
        notifications: false,
        tray: false,
        powerProfilesEnabled: false,
      )..setShellStyle(ShellStyle.macos);
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(
            const MethodChannel('clarion.shell/window'),
            (call) async => call.method == 'outputName' ? 'winit' : null,
          );
      addTearDown(
        () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
            .setMockMethodCallHandler(
              const MethodChannel('clarion.shell/window'),
              null,
            ),
      );
      controller.selectPanel(ShellPanel.launcher);
      await controller.applyExternalLauncher(
        visible: true,
        requestId: controller.externalLauncherRequest,
        accent: '#000000',
      );
      expect(controller.externalLauncherDesired, isTrue);
      await tester.pump(const Duration(milliseconds: 79));
      expect(controller.externalLauncherDesired, isTrue);
      await tester.pump(const Duration(milliseconds: 1));
      expect(controller.externalLauncherEnabled, isFalse);
      expect(controller.panel, ShellPanel.launcher);
      expect(controller.error, contains('未响应'));
      controller.dispose();
    },
  );
}
