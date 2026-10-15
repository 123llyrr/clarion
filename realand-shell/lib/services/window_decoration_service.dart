// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:io';

import '../models.dart';
import 'system_service.dart';

/// Manages window titlebar button layout between macOS traffic lights (left)
/// and Material / standard layout (right).
class WindowDecorationService {
  WindowDecorationService({
    CommandRunner? command,
    String? homeDirectory,
    this.enabled = true,
  }) : _command = command ?? runSystemCommand,
       _home = homeDirectory ?? Platform.environment['HOME'] ?? '';

  final CommandRunner _command;
  final String _home;
  final bool enabled;

  static const macosLayout = 'close,minimize,maximize:';
  static const materialLayout = 'icon:minimize,maximize,close';

  Future<void> applyForStyle(ShellStyle style) async {
    if (!enabled) return;
    final layout = style == ShellStyle.macos ? macosLayout : materialLayout;
    await applyLayout(layout);
  }

  Future<void> applyLayout(String layout) async {
    if (!enabled) return;
    // 1. GNOME / GSettings
    try {
      await _command('gsettings', [
        'set',
        'org.gnome.desktop.wm.preferences',
        'button-layout',
        layout,
      ]);
    } on Object {
      // Ignore if gsettings is unavailable in test / headless
    }

    // 2. GTK 3.0 / 4.0 settings.ini
    if (_home.isNotEmpty) {
      await _updateGtkSettings('$_home/.config/gtk-3.0/settings.ini', layout);
      await _updateGtkSettings('$_home/.config/gtk-4.0/settings.ini', layout);
    }
  }

  Future<void> _updateGtkSettings(String path, String layout) async {
    try {
      final file = File(path);
      if (!await file.exists()) return;
      final content = await file.readAsString();
      final regex = RegExp(r'^gtk-decoration-layout=.*$', multiLine: true);
      String newContent;
      if (regex.hasMatch(content)) {
        newContent = content.replaceAll(regex, 'gtk-decoration-layout=$layout');
      } else {
        newContent = '$content\ngtk-decoration-layout=$layout\n';
      }
      await file.writeAsString(newContent);
    } on Object {
      // File update is best-effort.
    }
  }
}
