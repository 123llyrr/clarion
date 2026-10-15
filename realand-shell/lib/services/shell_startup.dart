// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';

/// Render independently of menu discovery, but keep autostart clients behind
/// both the first frame and a bounded registrar preparation. A failed optional
/// service must not leave systemd waiting until it kills the whole shell.
Future<bool> announceShellReady({
  required Future<void> firstFrame,
  required Future<void> Function() prepareAppMenus,
  required Future<void> Function() notifyReady,
  required void Function(String message) report,
  Duration menuTimeout = const Duration(seconds: 2),
  Duration frameTimeout = const Duration(seconds: 10),
}) async {
  final menus = Future<void>.sync(prepareAppMenus)
      .timeout(menuTimeout)
      .catchError((Object error) {
        report('application menu preparation unavailable: $error');
      });
  try {
    await Future.wait([firstFrame.timeout(frameTimeout), menus]);
  } on Object catch (error) {
    report('first frame unavailable: $error');
    return false;
  }
  try {
    await notifyReady();
    return true;
  } on Object catch (error) {
    report('session readiness notification unavailable: $error');
    return false;
  }
}
