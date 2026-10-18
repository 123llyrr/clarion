// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:ui' show PointerDeviceKind;
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/mac_shell_preferences.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/ui/mac_shell_settings.dart';
import 'package:clarion_shell/ui/mac_shell_view.dart';
import 'package:clarion_shell/ui/tray_icon.dart';

void main() {
  testWidgets('system slider frames leave the Mac preferences form unchanged', (
    tester,
  ) async {
    final controller = ShellController(demo: true)
      ..setShellStyle(ShellStyle.macos);
    addTearDown(controller.dispose);
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SingleChildScrollView(
            child: MacShellSettings(controller: controller),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(
      find.byKey(const ValueKey('mac-dock-mission-control')),
      findsNothing,
    );
    final form = find.byKey(const ValueKey('mac-shell-settings'));
    final heightSlider = find.byKey(const ValueKey('mac-menu-height'));
    final oldForm = tester.widget(form);
    final oldSlider = tester.widget(heightSlider);
    for (var frame = 0; frame < 30; frame++) {
      await controller.setVolume((frame + 1) / 40);
      await controller.setBrightness((40 - frame) / 50);
      await tester.pump(const Duration(milliseconds: 16));
      expect(tester.widget(form), same(oldForm));
      expect(tester.widget(heightSlider), same(oldSlider));
    }
    controller.setMacPreferences(
      controller.macPreferences.copyWith(menuBarHeight: 38),
    );
    await tester.pump();
    expect(tester.widget(form), isNot(same(oldForm)));
    expect(tester.widget<Slider>(heightSlider).value, 38);
    expect(find.text('高度  38 px'), findsOneWidget);
    await tester.pumpWidget(const SizedBox.shrink());
    expect(tester.takeException(), isNull);
  });

  for (final width in [260.0, 520.0]) {
    testWidgets('macOS settings work and fit at width $width', (tester) async {
      final controller = ShellController(demo: true);
      addTearDown(controller.dispose);
      controller.setBarPosition(BarPosition.bottom);
      controller.setShellStyle(ShellStyle.macos);
      controller.setMacPreferences(
        const MacShellPreferences(menuBarWidthPercent: 65),
      );
      await tester.binding.setSurfaceSize(Size(width, 700));
      addTearDown(() => tester.binding.setSurfaceSize(null));
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: SingleChildScrollView(
              child: MacShellSettings(controller: controller),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('mac-menu-width')), findsNothing);
      final heightSlider = find.byKey(const ValueKey('mac-menu-height'));
      final heightControl = tester.widget<Slider>(heightSlider);
      expect(heightControl.min, 22);
      expect(heightControl.max, 48);
      expect(heightControl.divisions, 26);
      heightControl.onChanged!(38);
      await tester.pump();
      expect(controller.macPreferences.menuBarHeight, 38);
      expect(controller.macPreferences.menuBarWidthPercent, 65);
      expect(find.text('高度  38 px'), findsOneWidget);
      final sizeSlider = find.byKey(const ValueKey('mac-dock-size'));
      await tester.ensureVisible(sizeSlider);
      tester.widget<Slider>(sizeSlider).onChanged!(64);
      await tester.pump();
      expect(controller.macPreferences.dockSize, 64);
      final position = find.byKey(const ValueKey('mac-dock-position-bottom'));
      await tester.ensureVisible(position);
      await tester.tap(position);
      await tester.pumpAndSettle();
      await tester.tap(find.text('左侧').last);
      await tester.pumpAndSettle();
      expect(controller.macPreferences.dockPosition, MacDockPosition.left);
      final magnification = find.byKey(
        const ValueKey('mac-dock-magnification'),
      );
      await tester.ensureVisible(magnification);
      await tester.tap(magnification);
      await tester.pumpAndSettle();
      expect(controller.macPreferences.magnification, isFalse);
      expect(
        tester
            .widget<Slider>(
              find.byKey(const ValueKey('mac-dock-magnification-size')),
            )
            .onChanged,
        isNull,
      );
      final minimizeEffect = find.byKey(
        const ValueKey('mac-minimize-effect-genie'),
      );
      await tester.ensureVisible(minimizeEffect);
      await tester.tap(minimizeEffect);
      await tester.pumpAndSettle();
      await tester.tap(find.text('缩放效果').last);
      await tester.pumpAndSettle();
      expect(controller.macPreferences.minimizeEffect, MacMinimizeEffect.scale);
      expect(controller.macPreferences.dockPosition, MacDockPosition.left);
      expect(controller.barPosition, BarPosition.bottom);
      final originalIcons = find.byKey(const ValueKey('mac-original-icons'));
      await tester.ensureVisible(originalIcons);
      await tester.tap(originalIcons);
      await tester.pumpAndSettle();
      expect(controller.macPreferences.useOriginalIcons, isFalse);
      expect(controller.macPreferences.dockPosition, MacDockPosition.left);
      expect(controller.macPreferences.menuBarHeight, 38);
      expect(controller.barPosition, BarPosition.bottom);
      final recents = find.byKey(const ValueKey('mac-dock-recents'));
      await tester.ensureVisible(recents);
      await tester.tap(recents);
      await tester.pumpAndSettle();
      expect(controller.macPreferences.showRecentApplications, isFalse);
      final downloadsDisplay = find.byKey(
        const ValueKey('mac-downloads-display-stack'),
      );
      await tester.ensureVisible(downloadsDisplay);
      await tester.tap(downloadsDisplay);
      await tester.pumpAndSettle();
      await tester.tap(find.text('文件夹').last);
      await tester.pumpAndSettle();
      expect(
        controller.macPreferences.downloadsDisplay,
        MacDownloadsDisplay.folder,
      );
      for (final (view, label) in [
        (MacDownloadsView.fan, '扇状'),
        (MacDownloadsView.grid, '网格'),
        (MacDownloadsView.list, '列表'),
      ]) {
        final downloadsView = find.byKey(
          ValueKey(
            'mac-downloads-view-${controller.macPreferences.downloadsView.name}',
          ),
        );
        await tester.ensureVisible(downloadsView);
        await tester.tap(downloadsView);
        await tester.pumpAndSettle();
        await tester.tap(find.text(label).last);
        await tester.pumpAndSettle();
        expect(controller.macPreferences.downloadsView, view);
        expect(
          controller.macPreferences.downloadsDisplay,
          MacDownloadsDisplay.folder,
        );
        expect(controller.macPreferences.showRecentApplications, isFalse);
      }
      final downloads = find.byKey(const ValueKey('mac-dock-downloads'));
      await tester.ensureVisible(downloads);
      await tester.tap(downloads);
      await tester.pumpAndSettle();
      expect(controller.macPreferences.showDownloads, isFalse);
      expect(
        tester
            .widget<DropdownButtonFormField<MacDownloadsDisplay>>(
              find.byKey(const ValueKey('mac-downloads-display-folder')),
            )
            .onChanged,
        isNull,
      );
      expect(
        tester
            .widget<DropdownButtonFormField<MacDownloadsView>>(
              find.byKey(const ValueKey('mac-downloads-view-list')),
            )
            .onChanged,
        isNull,
      );
      await tester.tap(downloads);
      await tester.pumpAndSettle();
      expect(controller.macPreferences.showDownloads, isTrue);
      expect(controller.macPreferences.downloadsView, MacDownloadsView.list);
      expect(
        controller.macPreferences.downloadsDisplay,
        MacDownloadsDisplay.folder,
      );
      expect(
        controller.macPreferences.downloadsSort,
        MacDownloadsSort.dateModified,
      );
      expect(controller.macPreferences.dockPosition, MacDockPosition.left);
      expect(controller.macPreferences.minimizeEffect, MacMinimizeEffect.scale);
      expect(controller.macPreferences.menuBarHeight, 38);
      expect(controller.barPosition, BarPosition.bottom);
      final reset = find.byKey(const ValueKey('mac-shell-reset'));
      await tester.ensureVisible(reset);
      await tester.tap(reset);
      await tester.pumpAndSettle();
      expect(controller.macPreferences, const MacShellPreferences());
      expect(controller.barPosition, BarPosition.bottom);
      expect(tester.takeException(), isNull);
    });
  }

  testWidgets('dock visibility offers hiding and keeps the modes exclusive', (
    tester,
  ) async {
    final controller = ShellController(demo: true);
    addTearDown(controller.dispose);
    controller.setBarPosition(BarPosition.bottom);
    controller.setShellStyle(ShellStyle.macos);
    // Tall enough for the whole form: this mounts the panel without the
    // scrolling wrapper the real control center provides.
    await tester.binding.setSurfaceSize(const Size(520, 2600));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: AnimatedBuilder(
            animation: controller,
            builder: (context, _) => MacShellSettings(controller: controller),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    final always = find.byKey(const ValueKey('mac-dock-visibility-always'));
    await tester.ensureVisible(always);
    await tester.pumpAndSettle();
    await tester.tap(always);
    await tester.pumpAndSettle();
    await tester.tap(find.text('隐藏').last);
    await tester.pumpAndSettle();
    expect(controller.macPreferences.dockHidden, isTrue);
    expect(controller.macPreferences.dockAutoHide, isFalse);

    final hidden = find.byKey(const ValueKey('mac-dock-visibility-hidden'));
    await tester.ensureVisible(hidden);
    await tester.pumpAndSettle();
    await tester.tap(hidden);
    await tester.pumpAndSettle();
    await tester.tap(find.text('自动隐藏').last);
    await tester.pumpAndSettle();
    expect(controller.macPreferences.dockHidden, isFalse);
    expect(controller.macPreferences.dockAutoHide, isTrue);
    expect(tester.takeException(), isNull);
  });

  testWidgets('centered menu width and edge reveal preserve open panels', (
    tester,
  ) async {
    final controller = ShellController(demo: true);
    addTearDown(controller.dispose);
    controller.setShellStyle(ShellStyle.macos);
    controller.setReduceMotion(true);
    controller.setMacPreferences(
      const MacShellPreferences(
        menuBarWidthPercent: 65,
        menuBarHeight: 40,
        menuBarAutoHide: true,
        clockShowSeconds: true,
        showBatteryPercentage: true,
      ),
    );
    await tester.binding.setSurfaceSize(const Size(1000, 700));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await tester.pumpWidget(
      MaterialApp(
        home: MacShellView(
          controller: controller,
          availableWidth: 1000,
          preview: true,
        ),
      ),
    );
    await tester.pump(const Duration(milliseconds: 400));
    await tester.pump();
    final bounds = tester.getRect(
      find.byKey(const ValueKey('mac-menu-bar-position')),
    );
    expect(bounds.width, 650);
    expect(bounds.left, 175);
    expect(bounds.height, 40);
    expect(controller.macMenuBarVisible, isFalse);
    expect(
      tester.getSize(find.byKey(const ValueKey('mac-menu-edge-sensor'))).height,
      2,
    );
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: const Offset(200, 100));
    addTearDown(mouse.removePointer);
    await mouse.moveTo(const Offset(300, 1));
    await tester.pump();
    expect(controller.macMenuBarVisible, isTrue);
    controller.selectPanel(ShellPanel.quickSettings);
    await mouse.moveTo(const Offset(500, 500));
    await tester.pump(const Duration(milliseconds: 400));
    expect(controller.macMenuBarVisible, isTrue);
    expect(
      tester
          .getRect(find.byKey(const ValueKey('mac-control-center-glass')))
          .right,
      bounds.right - 12,
    );
    expect(
      tester
          .getRect(find.byKey(const ValueKey('mac-control-center-glass')))
          .top,
      46,
    );
    controller.selectPanel(ShellPanel.systemMenu);
    await tester.pumpAndSettle();
    expect(
      tester.getRect(find.byKey(const ValueKey('mac-panel-position'))).left,
      bounds.left + 12,
    );
    controller.closePanel();
    await tester.pump(const Duration(milliseconds: 400));
    await tester.pump();
    expect(controller.macMenuBarVisible, isFalse);
    controller.setMacPreferences(
      controller.macPreferences.copyWith(menuBarAutoHide: false),
    );
    await tester.pump();
    expect(controller.macMenuBarVisible, isTrue);
    expect(tester.takeException(), isNull);
  });

  testWidgets('tray fills a taller menu bar without scaling its glyph', (
    tester,
  ) async {
    final controller = ShellController(demo: true);
    addTearDown(controller.dispose);
    controller.setShellStyle(ShellStyle.macos);
    controller.setReduceMotion(true);
    controller.setMacPreferences(const MacShellPreferences(menuBarHeight: 48));
    await tester.binding.setSurfaceSize(const Size(1000, 700));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await tester.pumpWidget(
      MaterialApp(home: MacShellView(controller: controller, preview: true)),
    );
    await tester.pumpAndSettle();
    final tray = tester.getRect(
      find.byKey(const ValueKey('tray-demo-network')),
    );
    expect(tray.height, 48);
    expect(tray.center.dy, 24);
    expect(tester.widgetList<TrayGlyph>(find.byType(TrayGlyph)).first.size, 20);
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: const Offset(100, 100));
    addTearDown(mouse.removePointer);
    await mouse.moveTo(tray.center);
    await tester.pump(const Duration(milliseconds: 450));
    expect(controller.barTooltip?.anchor.dy, 24);
    await tester.tapAt(Offset(tray.center.dx, tray.bottom - 2));
    await tester.pumpAndSettle();
    expect(controller.panel, ShellPanel.trayMenu);
    expect(tester.takeException(), isNull);
  });
}
