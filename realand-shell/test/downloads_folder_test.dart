// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter_test/flutter_test.dart';
import 'package:realand_shell/services/system_service.dart';
import 'package:realand_shell/services/application_launcher.dart';

void main() {
  test('Downloads uses localized XDG path as literal argv', () async {
    final calls = <(String, List<String>)>[];
    Future<String> command(String program, List<String> args) async {
      calls.add((program, args));
      return program == 'xdg-user-dir' ? r'/home/demo/下载 $(literal)' : '';
    }

    final service = SystemService(
      environment: {'HOME': '/home/demo'},
      command: command,
      applicationLauncher: ApplicationLauncher(
        command: command,
        scopeProbe: () async => null,
      ),
    );
    await service.openDownloadsFolder();
    expect(calls.map((call) => call.$1), ['xdg-user-dir', 'gio']);
    expect(calls.first.$2, ['DOWNLOAD']);
    expect(calls.last.$2, ['open', '--', r'/home/demo/下载 $(literal)']);
  });
  test(
    'unavailable or malformed xdg-user-dir retains a literal default',
    () async {
      for (final response in [null, 'relative/path']) {
        List<String>? opened;
        Future<String> command(String program, List<String> args) async {
          if (program == 'xdg-user-dir') {
            if (response == null) throw StateError('missing');
            return response;
          }
          opened = args;
          return '';
        }

        final service = SystemService(
          environment: {'HOME': '/home/demo'},
          command: command,
          applicationLauncher: ApplicationLauncher(
            command: command,
            scopeProbe: () async => null,
          ),
        );
        await service.openDownloadsFolder();
        expect(opened, ['open', '--', '/home/demo/Downloads']);
      }
    },
  );
}
