// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';

import 'mac_shell_preferences.dart';
import 'models.dart';
import 'services/identity.dart';

class ShellPreferences {
  const ShellPreferences({
    this.themeMode = ThemeMode.system,
    this.reduceMotion = false,
    this.seed = const Color(0xff6750a4),
    this.barPosition = BarPosition.top,
    this.shellStyle = ShellStyle.material,
    this.pinnedAppIds,
    this.recentAppIds = const [],
    this.macShell = const MacShellPreferences(),
  });
  final ThemeMode themeMode;
  final bool reduceMotion;
  final Color seed;
  final BarPosition barPosition;
  final ShellStyle shellStyle;
  // null means choose defaults from installed applications; [] is an explicit
  // choice to keep no applications pinned.
  final List<String>? pinnedAppIds;
  final List<String> recentAppIds;
  final MacShellPreferences macShell;

  factory ShellPreferences.fromJson(Map<String, dynamic> json) {
    final mode = switch (json['theme']) {
      'light' => ThemeMode.light,
      'dark' => ThemeMode.dark,
      _ => ThemeMode.system,
    };
    final text = json['accent'];
    final rgb = text is String && RegExp(r'^#[0-9a-fA-F]{6}$').hasMatch(text)
        ? int.parse(text.substring(1), radix: 16)
        : 0x6750a4;
    return ShellPreferences(
      themeMode: mode,
      reduceMotion: json['reduce_motion'] == true,
      seed: Color(0xff000000 | rgb),
      barPosition: json['bar_position'] == 'bottom'
          ? BarPosition.bottom
          : BarPosition.top,
      shellStyle: json['shell_style'] == 'macos'
          ? ShellStyle.macos
          : ShellStyle.material,
      pinnedAppIds: json['pinned_app_ids'] is List
          ? List.unmodifiable({
              for (final id in json['pinned_app_ids'] as List)
                if (id is String && id.trim().isNotEmpty) id.trim(),
            })
          : null,
      recentAppIds: json['recent_app_ids'] is List
          ? List.unmodifiable(
              {
                for (final id in json['recent_app_ids'] as List)
                  if (id is String && id.trim().isNotEmpty) id.trim(),
              }.take(24),
            )
          : const [],
      macShell: json['macos'] is Map<String, dynamic>
          ? MacShellPreferences.fromJson(json['macos'] as Map<String, dynamic>)
          : const MacShellPreferences(),
    );
  }

  Map<String, dynamic> toJson() => {
    'version': 1,
    'theme': themeMode.name,
    'reduce_motion': reduceMotion,
    'bar_position': barPosition.name,
    'shell_style': shellStyle.name,
    if (pinnedAppIds != null) 'pinned_app_ids': pinnedAppIds,
    'recent_app_ids': recentAppIds,
    'macos': macShell.toJson(),
    'accent':
        '#${(seed.toARGB32() & 0xffffff).toRadixString(16).padLeft(6, '0')}',
  };

  static File? get file => shellConfigurationFile('preferences.json');

  static Future<ShellPreferences> load({File? source}) async {
    final path = source ?? file;
    if (path == null || !await path.exists()) return const ShellPreferences();
    try {
      // Preferences are small; do not load an accidentally replaced large file.
      if (await path.length() > 65536) return const ShellPreferences();
      final json = jsonDecode(await path.readAsString());
      return json is Map<String, dynamic>
          ? ShellPreferences.fromJson(json)
          : const ShellPreferences();
    } on FormatException {
      return const ShellPreferences();
    } on FileSystemException {
      return const ShellPreferences();
    }
  }

  Future<void> save({File? destination}) async {
    final path = destination ?? file;
    if (path == null) return;
    await path.parent.create(recursive: true);
    final staging = await path.parent.createTemp('.preferences-');
    try {
      final temp = File('${staging.path}/preferences.json');
      await temp.writeAsString('${jsonEncode(toJson())}\n', flush: true);
      await temp.rename(path.path);
    } finally {
      await staging.delete(recursive: true);
    }
  }
}
