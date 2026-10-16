// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/services/clarion_ipc.dart';

class _Ipc extends ClarionIpc {
  _Ipc(this.reply, {this.fail = false});
  final Object? reply;
  final bool fail;
  final requests = <Object>[];
  @override
  Future<dynamic> request(Object request) async {
    requests.add(request);
    if (fail) throw StateError('Unknown request');
    return reply;
  }
}

void main() {
  test(
    'only advertised running-compositor support enables inline menu rename',
    () async {
      final ipc = _Ipc({
        'Capabilities': ['future-feature', 'desktop-menu-focus-restore'],
      });
      expect(await desktopMenuFocusRestoreSupported(ipc), isTrue);
      expect(ipc.requests, ['Capabilities']);
      ipc.dispose();
    },
  );
  test(
    'old, malformed and unrelated replies preserve working modal rename',
    () async {
      for (final reply in [
        null,
        'Handled',
        {'Capabilities': 'desktop-menu-focus-restore'},
        {
          'Capabilities': ['future-feature'],
        },
      ]) {
        final ipc = _Ipc(reply);
        expect(await desktopMenuFocusRestoreSupported(ipc), isFalse);
        ipc.dispose();
      }
      final old = _Ipc(null, fail: true);
      expect(await desktopMenuFocusRestoreSupported(old), isFalse);
      old.dispose();
    },
  );
}
