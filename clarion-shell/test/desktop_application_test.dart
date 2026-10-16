// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:io';

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/desktop/desktop_application.dart';
import 'package:clarion_shell/desktop/desktop_layout.dart';
import 'package:clarion_shell/desktop/desktop_service.dart';
import 'package:clarion_shell/desktop/desktop_view.dart';
import 'package:clarion_shell/mac_shell_preferences.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/preferences.dart';
import 'package:clarion_shell/services/desktop_apps.dart';

class _Apps extends DesktopApps {
  @override
  Future<List<DesktopApp>> load() async => const [];
}

Future<void> _until(WidgetTester tester, bool Function() ready) async {
  for (var i = 0; i < 100 && !ready(); i++) {
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 20)),
    );
    await tester.pump(const Duration(milliseconds: 150));
  }
  expect(ready(), isTrue);
}

void main() {
  const window = MethodChannel('clarion.desktop/window');
  const icons = MethodChannel('clarion.shell/window');
  testWidgets(
    'atomic appearance updates and output names reach the desktop without rewriting settings',
    (tester) async {
      final temp = Directory.systemTemp.createTempSync('clarion-desktop-app-');
      addTearDown(() => temp.deleteSync(recursive: true));
      final directory = Directory('${temp.path}/files')..createSync();
      File('${directory.path}/a.txt').writeAsStringSync('a');
      final prefs = File('${temp.path}/config/preferences.json');
      await tester.runAsync(
        () => const ShellPreferences(
          themeMode: ThemeMode.light,
        ).save(destination: prefs),
      );
      var output = 'DP-1';
      final overlays = <bool>[];
      final shortcutStyles = <String>[];
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(window, (
        call,
      ) async {
        if (call.method == 'outputName') return output;
        if (call.method == 'setOverlay') overlays.add(call.arguments as bool);
        if (call.method == 'setShellStyle') {
          shortcutStyles.add(call.arguments as String);
        }
        return null;
      });
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        icons,
        (_) async => null,
      );
      addTearDown(() {
        tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          window,
          null,
        );
        tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          icons,
          null,
        );
      });
      final service = DesktopService(
        root: directory.path,
        command: (_, _) async => throw StateError('Unexpected command'),
      );
      final layout = DesktopLayoutStore(file: File('${temp.path}/layout.json'));
      await tester.pumpWidget(
        DesktopApplication(
          service: service,
          layout: layout,
          preferencesFile: prefs,
          applications: _Apps(),
        ),
      );
      await _until(
        tester,
        () =>
            service.entries.isNotEmpty &&
            tester.widget<DesktopView>(find.byType(DesktopView)).outputName ==
                output,
      );
      expect(shortcutStyles, ['material']);
      const changed = ShellPreferences(
        themeMode: ThemeMode.dark,
        reduceMotion: true,
        barPosition: BarPosition.bottom,
        seed: Color(0xff006e2a),
      );
      await tester.runAsync(() => changed.save(destination: prefs));
      await _until(
        tester,
        () =>
            tester.widget<DesktopView>(find.byType(DesktopView)).barPosition ==
            BarPosition.bottom,
      );
      final app = tester.widget<MaterialApp>(find.byType(MaterialApp));
      expect(app.themeMode, ThemeMode.dark);
      expect(
        tester.widget<DesktopView>(find.byType(DesktopView)).reduceMotion,
        isTrue,
      );
      expect(
        await tester.runAsync(() => prefs.readAsString()),
        contains('"bar_position":"bottom"'),
      );
      const macAppearance = ShellPreferences(
        themeMode: ThemeMode.dark,
        reduceMotion: true,
        barPosition: BarPosition.bottom,
        shellStyle: ShellStyle.macos,
        seed: Color(0xff006e2a),
        macShell: MacShellPreferences(
          dockPosition: MacDockPosition.left,
          dockSize: 64,
        ),
      );
      await tester.runAsync(() => macAppearance.save(destination: prefs));
      await _until(
        tester,
        () =>
            tester
                .widget<DesktopView>(find.byType(DesktopView))
                .macPreferences ==
            macAppearance.macShell,
      );
      expect(
        tester.widget<DesktopView>(find.byType(DesktopView)).shellStyle,
        ShellStyle.macos,
      );
      expect(shortcutStyles, ['material', 'macos']);
      const macOnlyChange = ShellPreferences(
        themeMode: ThemeMode.dark,
        reduceMotion: true,
        barPosition: BarPosition.bottom,
        shellStyle: ShellStyle.macos,
        seed: Color(0xff006e2a),
        macShell: MacShellPreferences(
          dockPosition: MacDockPosition.right,
          dockSize: 32,
          dockAutoHide: true,
          menuBarAutoHide: true,
        ),
      );
      await tester.runAsync(() => macOnlyChange.save(destination: prefs));
      await _until(
        tester,
        () =>
            tester
                .widget<DesktopView>(find.byType(DesktopView))
                .macPreferences ==
            macOnlyChange.macShell,
      );
      expect(
        await tester.runAsync(() => prefs.readAsString()),
        contains('"dock_position":"right"'),
      );
      expect(shortcutStyles, ['material', 'macos']);
      await tester.runAsync(() => changed.save(destination: prefs));
      await _until(
        tester,
        () =>
            tester.widget<DesktopView>(find.byType(DesktopView)).shellStyle ==
            ShellStyle.material,
      );
      expect(shortcutStyles, ['material', 'macos', 'material']);
      output = 'HDMI-A-1';
      await tester.binding.defaultBinaryMessenger.handlePlatformMessage(
        window.name,
        const StandardMethodCodec().encodeMethodCall(
          const MethodCall('outputChanged'),
        ),
        (_) {},
      );
      await tester.pump();
      expect(
        tester.widget<DesktopView>(find.byType(DesktopView)).outputName,
        output,
      );

      await tester.tapAt(
        const Offset(600, 400),
        kind: PointerDeviceKind.mouse,
        buttons: kSecondaryMouseButton,
      );
      await tester.pumpAndSettle();
      expect(overlays, [true]);
      await tester.tap(find.text('新建文件'));
      await tester.pumpAndSettle();
      expect(overlays, [true]);
      await tester.tap(find.text('取消'));
      await tester.pumpAndSettle();
      expect(overlays, [true, false]);
      expect(service.entries.length, 1);
      await tester.pumpWidget(const SizedBox.shrink());
      await tester.runAsync(
        () => Future<void>.delayed(const Duration(milliseconds: 160)),
      );
      expect(tester.takeException(), isNull);
    },
  );

  for (final preview in [false, true]) {
    testWidgets('initial macOS shortcut style respects preview=$preview', (
      tester,
    ) async {
      final temp = Directory.systemTemp.createTempSync(
        'clarion-desktop-style-',
      );
      addTearDown(() => temp.deleteSync(recursive: true));
      final styles = <String>[];
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(window, (
        call,
      ) async {
        if (call.method == 'setShellStyle') {
          styles.add(call.arguments as String);
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
        DesktopApplication(
          service: DesktopService(root: temp.path),
          layout: DesktopLayoutStore(file: File('${temp.path}/layout.json')),
          applications: _Apps(),
          preferences: const ShellPreferences(shellStyle: ShellStyle.macos),
          watchPreferences: false,
          preview: preview,
        ),
      );
      await tester.pumpAndSettle();
      expect(styles, preview ? isEmpty : ['macos']);
      await tester.pumpWidget(const SizedBox.shrink());
      await tester.pumpAndSettle();
      expect(tester.takeException(), isNull);
    });
  }
}
