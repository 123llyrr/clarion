// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:realand_shell/mac_shell_preferences.dart';

void main() {
  const defaults = MacShellPreferences();
  const configured = MacShellPreferences(
    dockPosition: MacDockPosition.right,
    minimizeEffect: MacMinimizeEffect.scale,
    dockSize: 64,
    magnification: false,
    magnificationScale: 2,
    dockAutoHide: true,
    showRunningIndicators: false,
    animateOpeningApplications: false,
    showLaunchpad: false,
    showMissionControl: false,
    showDownloads: false,
    showRecentApplications: false,
    downloadsView: MacDownloadsView.list,
    downloadsDisplay: MacDownloadsDisplay.folder,
    downloadsSort: MacDownloadsSort.kind,
    menuBarAutoHide: true,
    menuBarHeight: 48,
    menuBarWidthPercent: 75,
    showBatteryPercentage: true,
    clockShowDate: false,
    clockShowSeconds: true,
    useOriginalIcons: false,
    minimizeIntoApp: false,
  );

  test('defaults retain the macOS bar and Dock behavior', () {
    expect(defaults.toJson(), {
      'dock_position': 'bottom',
      'minimize_effect': 'genie',
      'dock_size': 48.0,
      'magnification': true,
      'magnification_scale': 1.45,
      'dock_auto_hide': false,
      'show_running_indicators': true,
      'animate_opening_applications': true,
      'show_launchpad': true,
      'show_mission_control': true,
      'show_downloads': true,
      'show_recent_applications': true,
      'downloads_view': 'automatic',
      'downloads_display': 'stack',
      'downloads_sort': 'dateModified',
      'menu_bar_auto_hide': false,
      'menu_bar_height': 22.0,
      'menu_bar_width_percent': 100.0,
      'show_battery_percentage': false,
      'clock_show_date': true,
      'clock_show_seconds': false,
      'use_original_icons': true,
      'minimize_into_app': true,
    });
    expect(MacShellPreferences.fromJson({}), defaults);
  });

  test('all settings round trip as JSON and use value equality', () {
    final restored = MacShellPreferences.fromJson(
      jsonDecode(jsonEncode(configured.toJson())) as Map<String, dynamic>,
    );
    expect(restored, configured);
    expect(restored.hashCode, configured.hashCode);
    expect(configured.copyWith(), configured);
    expect({configured, restored, configured.copyWith()}, hasLength(1));
    for (final position in MacDockPosition.values) {
      expect(
        MacShellPreferences.fromJson({
          'dock_position': position.name,
        }).dockPosition,
        position,
      );
    }
    for (final key in configured.toJson().keys) {
      final oneChanged = defaults.toJson();
      oneChanged[key] = configured.toJson()[key];
      expect(MacShellPreferences.fromJson(oneChanged), isNot(defaults));
    }
    // Exported maps are fresh snapshots, so a caller cannot mutate the model.
    configured.toJson()['dock_size'] = 32;
    expect(configured.dockSize, 64);
  });

  test('malformed booleans, numbers and future positions use defaults', () {
    final booleanKeys = defaults
        .toJson()
        .entries
        .where((entry) => entry.value is bool)
        .map((entry) => entry.key);
    for (final value in [null, 'false', 'true', 0, 1, [], {}]) {
      expect(
        MacShellPreferences.fromJson({
          for (final key in booleanKeys) key: value,
          'dock_position': value,
          'dock_size': value,
          'magnification_scale': value,
          'menu_bar_width_percent': value,
          'menu_bar_height': value,
        }),
        // Finite 0/1 are valid numeric input and clamp at their lower limits.
        value is num
            ? defaults.copyWith(
                dockSize: 32,
                magnificationScale: 1,
                menuBarWidthPercent: 50,
              )
            : defaults,
      );
    }
    for (final value in [
      double.nan,
      double.infinity,
      double.negativeInfinity,
      'Infinity',
      'NaN',
    ]) {
      expect(
        MacShellPreferences.fromJson({
          'dock_size': value,
          'magnification_scale': value,
          'menu_bar_width_percent': value,
          'menu_bar_height': value,
        }),
        defaults,
      );
    }
    for (final value in ['top', 'Bottom', 'LEFT', '', 3, true]) {
      expect(
        MacShellPreferences.fromJson({'dock_position': value}).dockPosition,
        MacDockPosition.bottom,
      );
    }
  });

  test(
    'minimize effect round trips and unknown choices keep the Genie default',
    () {
      for (final effect in MacMinimizeEffect.values) {
        final changed = defaults.copyWith(minimizeEffect: effect);
        expect(changed.minimizeEffect, effect);
        expect(MacShellPreferences.fromJson(changed.toJson()), changed);
      }
      for (final invalid in [null, 'GENIE', 'Scale', 'suck', 0, true, [], {}]) {
        expect(
          MacShellPreferences.fromJson({'minimize_effect': invalid}),
          defaults,
        );
      }
      expect(
        configured.copyWith(dockSize: 40).minimizeEffect,
        MacMinimizeEffect.scale,
      );
    },
  );

  test(
    'Downloads display, view and sort remain independent and round trip',
    () {
      for (final display in MacDownloadsDisplay.values) {
        for (final view in MacDownloadsView.values) {
          for (final sort in MacDownloadsSort.values) {
            final changed = configured.copyWith(
              downloadsDisplay: display,
              downloadsView: view,
              downloadsSort: sort,
            );
            expect(changed.downloadsDisplay, display);
            expect(changed.downloadsView, view);
            expect(changed.downloadsSort, sort);
            expect(changed.showDownloads, isFalse);
            expect(changed.showRecentApplications, isFalse);
            expect(changed.minimizeEffect, MacMinimizeEffect.scale);
            expect(changed.dockPosition, MacDockPosition.right);
            final restored = MacShellPreferences.fromJson(
              jsonDecode(jsonEncode(changed.toJson())) as Map<String, dynamic>,
            );
            expect(restored, changed);
            expect(restored.hashCode, changed.hashCode);
          }
        }
      }
      final changed = configured.copyWith(showRecentApplications: true);
      expect(changed.showRecentApplications, isTrue);
      expect(changed.downloadsDisplay, configured.downloadsDisplay);
      expect(changed.downloadsView, configured.downloadsView);
      expect(changed.downloadsSort, configured.downloadsSort);
      expect(MacShellPreferences.fromJson(changed.toJson()), changed);
    },
  );

  test(
    'legacy preferences and unknown Downloads choices keep usable defaults',
    () {
      final legacy = MacShellPreferences.fromJson({
        'dock_position': 'left',
        'dock_size': 56,
        'show_downloads': false,
        'minimize_effect': 'scale',
      });
      expect(legacy.dockPosition, MacDockPosition.left);
      expect(legacy.dockSize, 56);
      expect(legacy.minimizeEffect, MacMinimizeEffect.scale);
      expect(legacy.showDownloads, isFalse);
      expect(legacy.showRecentApplications, isTrue);
      expect(legacy.downloadsDisplay, MacDownloadsDisplay.stack);
      expect(legacy.downloadsView, MacDownloadsView.automatic);
      expect(legacy.downloadsSort, MacDownloadsSort.dateModified);
      for (final invalid in [
        null,
        'GRID',
        'Stack',
        'unknown',
        0,
        true,
        [],
        {},
      ]) {
        expect(
          MacShellPreferences.fromJson({
            'downloads_display': invalid,
            'downloads_view': invalid,
            'downloads_sort': invalid,
            'show_recent_applications': invalid,
          }),
          defaults,
        );
      }
      expect(
        MacShellPreferences.fromJson({
          'show_recent_applications': false,
        }).showRecentApplications,
        isFalse,
      );
    },
  );

  test('finite numeric values clamp while copyWith preserves other fields', () {
    for (final (input, expected) in [
      (-100.0, 32.0),
      (31.99, 32.0),
      (32.0, 32.0),
      (48.25, 48.25),
      (64.0, 64.0),
      (64.01, 64.0),
      (1e100, 64.0),
    ]) {
      expect(
        MacShellPreferences.fromJson({'dock_size': input}).dockSize,
        expected,
      );
      expect(configured.copyWith(dockSize: input).dockSize, expected);
    }
    for (final (input, expected) in [
      (-1.0, 1.0),
      (1.0, 1.0),
      (1.75, 1.75),
      (2.0, 2.0),
      (3.0, 2.0),
    ]) {
      expect(
        MacShellPreferences.fromJson({
          'magnification_scale': input,
        }).magnificationScale,
        expected,
      );
    }
    final invalidCopy = configured.copyWith(
      dockSize: double.nan,
      magnificationScale: double.infinity,
      menuBarWidthPercent: double.negativeInfinity,
      menuBarHeight: double.nan,
    );
    expect(invalidCopy.dockSize, 48);
    expect(invalidCopy.magnificationScale, 1.45);
    expect(invalidCopy.menuBarWidthPercent, 100);
    expect(invalidCopy.menuBarHeight, 22);
    expect(invalidCopy.dockPosition, MacDockPosition.right);
    expect(invalidCopy.showLaunchpad, isFalse);
    expect(invalidCopy.clockShowSeconds, isTrue);
    expect(configured.dockSize, 64);
    expect(defaults.dockSize, 48);
    for (final (input, expected) in [
      (0.0, 50.0),
      (49.99, 50.0),
      (50.0, 50.0),
      (75.25, 75.25),
      (100.0, 100.0),
      (101.0, 100.0),
    ]) {
      expect(
        MacShellPreferences.fromJson({
          'menu_bar_width_percent': input,
        }).menuBarWidthPercent,
        expected,
      );
      expect(
        configured.copyWith(menuBarWidthPercent: input).menuBarWidthPercent,
        expected,
      );
    }
  });

  test(
    'menu height uses integer logical pixels and preserves legacy width',
    () {
      for (final (input, expected) in [
        (-1.0, 22.0),
        (21.99, 22.0),
        (22.0, 22.0),
        (31.49, 31.0),
        (31.5, 32.0),
        (48.0, 48.0),
        (48.1, 48.0),
        (1e100, 48.0),
      ]) {
        final parsed = MacShellPreferences.fromJson({
          'menu_bar_height': input,
          'menu_bar_width_percent': 65,
        });
        expect(parsed.menuBarHeight, expected);
        expect(parsed.menuBarWidthPercent, 65);
        final changed = configured.copyWith(menuBarHeight: input);
        expect(changed.menuBarHeight, expected);
        expect(changed.menuBarWidthPercent, configured.menuBarWidthPercent);
        expect(changed.dockPosition, configured.dockPosition);
      }
      final legacy = MacShellPreferences.fromJson({
        'menu_bar_width_percent': 65,
      });
      expect(legacy.menuBarHeight, 22);
      expect(legacy.menuBarWidthPercent, 65);
      expect(legacy.toJson()['menu_bar_width_percent'], 65);
      for (final height in [21.0, 48.01, 32.5]) {
        expect(
          () => MacShellPreferences(menuBarHeight: height),
          throwsAssertionError,
        );
      }
    },
  );
}
