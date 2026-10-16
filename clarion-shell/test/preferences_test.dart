// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/mac_shell_preferences.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/preferences.dart';

void main() {
  test('invalid preferences retain usable theme defaults', () {
    final preferences = ShellPreferences.fromJson({
      'theme': 'future-mode',
      'accent': '#xyz123',
      'reduce_motion': 'true',
      'bar_position': 'left',
    });
    expect(preferences.themeMode, ThemeMode.system);
    expect(preferences.seed, const Color(0xff6750a4));
    expect(preferences.reduceMotion, false);
    expect(preferences.barPosition, BarPosition.top);
  });

  test(
    'old preferences keep the bar at the top without changing appearance',
    () {
      final preferences = ShellPreferences.fromJson({
        'version': 1,
        'theme': 'dark',
        'accent': '#4285f4',
        'reduce_motion': true,
      });
      expect(preferences.barPosition, BarPosition.top);
      expect(preferences.themeMode, ThemeMode.dark);
      expect(preferences.seed, const Color(0xff4285f4));
      expect(preferences.reduceMotion, isTrue);
      expect(preferences.toJson()['version'], 1);
      expect(preferences.macShell, const MacShellPreferences());
    },
  );

  test('macOS options remain independent from every existing preference', () {
    const macShell = MacShellPreferences(
      dockPosition: MacDockPosition.left,
      dockSize: 40,
      dockAutoHide: true,
      showLaunchpad: false,
      showBatteryPercentage: true,
      clockShowSeconds: true,
      useOriginalIcons: false,
      menuBarWidthPercent: 75,
    );
    final old = <String, dynamic>{
      'version': 1,
      'theme': 'dark',
      'accent': '#123abc',
      'reduce_motion': true,
      'bar_position': 'bottom',
      'shell_style': 'material',
      'pinned_app_ids': ['editor.desktop', 'browser.desktop'],
    };
    final restored = ShellPreferences.fromJson({
      ...old,
      'macos': macShell.toJson(),
    });
    expect(restored.macShell, macShell);
    expect(restored.toJson()['macos'], macShell.toJson());
    for (final entry in old.entries) {
      expect(restored.toJson()[entry.key], entry.value);
    }
    for (final value in [
      null,
      7,
      true,
      'macos',
      [],
      {'dock_size': 'large'},
    ]) {
      final invalid = ShellPreferences.fromJson({...old, 'macos': value});
      expect(invalid.macShell, const MacShellPreferences());
      for (final entry in old.entries) {
        expect(invalid.toJson()[entry.key], entry.value);
      }
    }
  });

  test(
    'bar positions round trip and unsupported values fall back to the top',
    () {
      for (final position in BarPosition.values) {
        final preferences = ShellPreferences(barPosition: position);
        expect(preferences.toJson()['bar_position'], position.name);
        expect(
          ShellPreferences.fromJson(preferences.toJson()).barPosition,
          position,
        );
      }
      for (final value in [null, '', 'left', 'right', 'Bottom', 1, true]) {
        expect(
          ShellPreferences.fromJson({'bar_position': value}).barPosition,
          BarPosition.top,
        );
      }
    },
  );

  test(
    'preferences atomically round trip and tolerate corrupt input',
    () async {
      final dir = await Directory.systemTemp.createTemp('clarion-shell-prefs-');
      addTearDown(() => dir.delete(recursive: true));
      final file = File('${dir.path}/settings/preferences.json');
      const preferences = ShellPreferences(
        themeMode: ThemeMode.dark,
        reduceMotion: true,
        seed: Color(0xff4285f4),
        barPosition: BarPosition.bottom,
        macShell: MacShellPreferences(
          dockPosition: MacDockPosition.right,
          dockSize: 56,
          magnificationScale: 1.75,
          showDownloads: false,
          useOriginalIcons: false,
          menuBarAutoHide: true,
          menuBarWidthPercent: 80,
        ),
      );
      await preferences.save(destination: file);
      final restored = await ShellPreferences.load(source: file);
      expect(restored.toJson(), preferences.toJson());
      expect(await file.parent.list().length, 1);
      await file.writeAsString('{broken');
      expect(
        (await ShellPreferences.load(source: file)).themeMode,
        ThemeMode.system,
      );
    },
  );
}
