// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';

/// A rendered first frame is enough to make the desktop available. Optional
/// services connect independently and cannot delay session autostart.
Future<bool> announceShellReady({
  required Future<void> firstFrame,
  required Future<void> Function() notifyReady,
  required void Function(String message) report,
  Duration frameTimeout = const Duration(seconds: 10),
}) async {
  try {
    await firstFrame.timeout(frameTimeout);
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
