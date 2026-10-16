// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/theme/mac_shell_theme.dart';
import 'package:clarion_shell/ui/mac_control_center.dart';
import 'package:clarion_shell/ui/mac_shell_view.dart';

class _Controller extends ShellController {
  _Controller() : super(demo: true, notifications: false, tray: false);
  bool? wifi = true;
  bool? bluetooth = false;
  double? level = .65;
  double? backlight = .75;
  final pending = <String>{};
  int wifiCalls = 0;
  int bluetoothCalls = 0;
  int settingsCalls = 0;
  void refresh() => notifyListeners();
  @override
  bool? get wifiEnabled => wifi;
  @override
  bool? get bluetoothEnabled => bluetooth;
  @override
  double? get volume => level;
  @override
  double? get brightness => backlight;
  @override
  Set<String> get busy => pending;
  @override
  Future<void> toggleWifi() async {
    wifiCalls++;
  }

  @override
  Future<void> toggleBluetooth() async {
    bluetoothCalls++;
  }

  @override
  Future<void> openSettings() async {
    settingsCalls++;
  }
}

Finder _key(String key) => find.byKey(ValueKey(key));

Future<_Controller> _mount(
  WidgetTester tester, {
  Size size = const Size(1280, 800),
  double menuHeight = 22,
  double menuWidthPercent = 100,
  double dpi = 1,
  Brightness brightness = Brightness.light,
  bool reduceMotion = true,
  bool unavailable = false,
  bool preview = true,
  ValueChanged<Rect?>? onPanelGlassChanged,
}) async {
  tester.view.devicePixelRatio = dpi;
  tester.view.physicalSize = size * dpi;
  addTearDown(tester.view.resetDevicePixelRatio);
  addTearDown(tester.view.resetPhysicalSize);
  final controller = _Controller()
    ..setShellStyle(ShellStyle.macos)
    ..setReduceMotion(reduceMotion);
  if (unavailable) {
    controller.wifi = null;
    controller.bluetooth = null;
    controller.level = null;
    controller.backlight = null;
  }
  controller.setMacPreferences(
    controller.macPreferences.copyWith(
      menuBarHeight: menuHeight,
      menuBarWidthPercent: menuWidthPercent,
    ),
  );
  addTearDown(controller.dispose);
  await tester.pumpWidget(
    MaterialApp(
      theme: buildMacShellTheme(brightness),
      home: MacShellView(
        controller: controller,
        preview: preview,
        onPanelGlassChanged: onPanelGlassChanged,
      ),
    ),
  );
  await tester.pumpAndSettle();
  await tester.tap(_key('mac-quick-settings'));
  await tester.pumpAndSettle();
  return controller;
}

void main() {
  for (final scenario in [
    (const Size(1280, 800), 22.0, 100.0, 1.0),
    (const Size(1920, 1200), 32.0, 70.0, 2.0),
    (const Size(740, 600), 48.0, 75.0, 1.25),
    (const Size(180, 180), 22.0, 100.0, 1.0),
  ]) {
    testWidgets('control center follows its bar anchor at $scenario', (
      tester,
    ) async {
      final (size, height, percent, dpi) = scenario;
      final controller = await _mount(
        tester,
        size: size,
        menuHeight: height,
        menuWidthPercent: percent,
        dpi: dpi,
      );
      expect(controller.panel, ShellPanel.quickSettings);
      final panel = tester.getRect(_key('mac-control-center-glass'));
      final bar = tester.getRect(_key('mac-menu-bar-position'));
      expect(panel.width, size.width < 344 ? size.width - 24 : 320);
      expect(panel.top, height + 6);
      expect(panel.left, greaterThanOrEqualTo(12));
      expect(panel.right, lessThanOrEqualTo(size.width - 12));
      expect(panel.bottom, lessThanOrEqualTo(size.height - 8));
      if (size.width > 344) {
        expect(panel.right, closeTo(bar.right - 12, .01));
      }
      expect(find.byType(MacControlCenter), findsOneWidget);
      expect(tester.takeException(), isNull);
    });
  }

  testWidgets('outside click and Escape dismiss the compact panel', (
    tester,
  ) async {
    final controller = await _mount(tester);
    await tester.tapAt(const Offset(100, 500));
    await tester.pumpAndSettle();
    expect(controller.panel, ShellPanel.none);
    expect(_key('volume-slider'), findsNothing);
    await tester.tap(_key('mac-quick-settings'));
    await tester.pumpAndSettle();
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(controller.panel, ShellPanel.none);
    expect(tester.takeException(), isNull);
  });

  testWidgets('compact hardware controls retain availability and busy guards', (
    tester,
  ) async {
    final controller = await _mount(tester, unavailable: true);
    expect(tester.widget<Slider>(_key('volume-slider')).onChanged, isNull);
    expect(tester.widget<Slider>(_key('brightness-slider')).onChanged, isNull);
    await tester.tap(find.text('Wi-Fi'));
    await tester.tap(find.text('蓝牙'));
    expect(controller.wifiCalls, 0);
    expect(controller.bluetoothCalls, 0);
    controller.wifi = true;
    controller.bluetooth = true;
    controller.pending.addAll(['wifi', 'bluetooth']);
    controller.refresh();
    await tester.pump();
    await tester.tap(find.text('Wi-Fi'));
    await tester.tap(find.text('蓝牙'));
    expect(controller.wifiCalls, 0);
    expect(controller.bluetoothCalls, 0);
    controller.pending.clear();
    controller.refresh();
    await tester.pumpAndSettle();
    await tester.tap(find.text('Wi-Fi'));
    await tester.tap(find.text('蓝牙'));
    expect(controller.wifiCalls, 1);
    expect(controller.bluetoothCalls, 1);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'primary control center omits the old appearance form and fake services',
    (tester) async {
      await _mount(tester);
      expect(find.text('控制中心'), findsNothing);
      expect(find.byTooltip('关闭'), findsNothing);
      expect(find.text('界面风格'), findsNothing);
      expect(find.text('栏位置'), findsNothing);
      for (final label in [
        'AirDrop',
        '屏幕镜像',
        '播放中',
        'True Tone',
        'Night Shift',
      ]) {
        expect(find.text(label), findsNothing);
      }
      expect(_key('volume-slider'), findsOneWidget);
      expect(_key('brightness-slider'), findsOneWidget);
    },
  );

  testWidgets(
    'desktop preferences remain reachable and return to live controls',
    (tester) async {
      final controller = await _mount(tester);
      await tester.tap(_key('mac-control-desktop-preferences'));
      await tester.pumpAndSettle();
      expect(find.text('桌面偏好设置'), findsOneWidget);
      expect(find.text('界面风格'), findsOneWidget);
      expect(_key('shell-style'), findsOneWidget);
      final preferences = tester.getRect(_key('mac-control-center-glass'));
      expect(preferences.width, MacControlCenter.preferencesWidth);
      expect(preferences.center.dx, 640);
      expect(preferences.center.dy, 400);
      expect(preferences.height, lessThanOrEqualTo(720));
      expect(_key('volume-slider'), findsNothing);
      await tester.tap(_key('mac-control-back'));
      await tester.pumpAndSettle();
      expect(_key('volume-slider'), findsOneWidget);
      expect(_key('brightness-slider'), findsOneWidget);
      expect(controller.panel, ShellPanel.quickSettings);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'quick actions use real controller state and guard settings launch',
    (tester) async {
      final controller = await _mount(tester);
      expect(controller.doNotDisturb, isFalse);
      await tester.tap(_key('mac-control-dnd'));
      await tester.pumpAndSettle();
      expect(controller.doNotDisturb, isTrue);
      await tester.tap(_key('mac-control-dnd'));
      await tester.pumpAndSettle();
      expect(controller.doNotDisturb, isFalse);
      await tester.tap(_key('mac-control-appearance'));
      await tester.pumpAndSettle();
      expect(controller.themeMode, ThemeMode.dark);
      controller.pending.add('settings');
      controller.refresh();
      await tester.pump();
      await tester.tap(_key('mac-control-system-preferences'));
      expect(controller.settingsCalls, 0);
      controller.pending.clear();
      controller.refresh();
      await tester.pumpAndSettle();
      await tester.tap(_key('mac-control-system-preferences'));
      expect(controller.settingsCalls, 1);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('entry animation ends without keeping an idle ticker', (
    tester,
  ) async {
    await _mount(tester, reduceMotion: false);
    expect(tester.binding.transientCallbackCount, 0);
    expect(tester.takeException(), isNull);
  });

  testWidgets('reports actual glass bounds once and clears them on close', (
    tester,
  ) async {
    final regions = <Rect?>[];
    final controller = await _mount(
      tester,
      preview: false,
      onPanelGlassChanged: regions.add,
    );
    expect(regions.last!.width, 320);
    final count = regions.length;
    for (var frame = 0; frame < 5; frame++) {
      controller.refresh();
      await tester.pump();
    }
    expect(regions.length, count);
    await tester.tap(_key('mac-control-desktop-preferences'));
    await tester.pumpAndSettle();
    expect(regions.last!.width, 620);
    expect(regions.last!.height, lessThanOrEqualTo(720));
    expect(regions.last!.left, 330);
    controller.closePanel();
    await tester.pumpAndSettle();
    expect(regions.last, isNull);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'sound expands into real outputs while device changes suspend level edits',
    (tester) async {
      final controller = await _mount(tester);
      controller.pending.add('audio-output');
      controller.refresh();
      await tester.pump();
      expect(tester.widget<Slider>(_key('volume-slider')).onChanged, isNull);
      expect(
        tester.widget<Slider>(_key('brightness-slider')).onChanged,
        isNotNull,
      );
      controller.pending.clear();
      controller.refresh();
      await tester.pumpAndSettle();
      expect(tester.widget<Slider>(_key('volume-slider')).onChanged, isNotNull);
      await tester.tap(_key('mac-control-sound-output'));
      await tester.pumpAndSettle();
      expect(controller.panel, ShellPanel.volume);
      expect(find.text('输出设备：'), findsOneWidget);
      expect(find.byType(MacControlCenter), findsNothing);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('optional actual widget previews use bundled fonts', (
    tester,
  ) async {
    final destination =
        Platform.environment['CLARION_CONTROL_CENTER_PREVIEW_DIR'];
    if (destination == null) return;
    final shadowsDisabled = debugDisableShadows;
    debugDisableShadows = false;
    try {
      await tester.runAsync(() async {
        for (final font in {
          'macOSSystem': 'assets/macos/SFNS.ttf',
          'macOSChinese': 'assets/macos/PingFangSC-Regular.otf',
          'MaterialIcons':
              'build/unit_test_assets/fonts/MaterialIcons-Regular.otf',
        }.entries) {
          if (!File(font.value).existsSync()) continue;
          final bytes = await File(font.value).readAsBytes();
          await (FontLoader(
            font.key,
          )..addFont(Future.value(ByteData.sublistView(bytes)))).load();
        }
      });
      for (final brightness in Brightness.values) {
        await _mount(tester, brightness: brightness);
        final boundary = tester.renderObject<RenderRepaintBoundary>(
          _key('mac-control-center-capture'),
        );
        await tester.runAsync(() async {
          final image = await boundary.toImage(pixelRatio: 2);
          final bytes = await image.toByteData(format: ui.ImageByteFormat.png);
          await Directory(destination).create(recursive: true);
          await File(
            '$destination/control-center-${brightness.name}.png',
          ).writeAsBytes(bytes!.buffer.asUint8List());
          image.dispose();
        });
        await tester.pumpWidget(const SizedBox());
      }
    } finally {
      debugDisableShadows = shadowsDisabled;
    }
  });
}
