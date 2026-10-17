// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/desktop/desktop_entry.dart';
import 'package:clarion_shell/desktop/desktop_service.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/services/application_launcher.dart';
import 'package:clarion_shell/services/desktop_apps.dart';
import 'package:clarion_shell/services/system_service.dart';

void main() {
  test('scope literal argv never expands paths or invokes a shell', () async {
    final calls = <(String, List<String>)>[];
    var probes = 0;
    final launcher = ApplicationLauncher(
      command: (program, arguments) async {
        calls.add((program, List.of(arguments)));
        return '';
      },
      scopeProbe: () async {
        expect(calls, isEmpty);
        probes++;
        return const ApplicationScope();
      },
    );
    const path = r'/home/应用 名/${HOME} $(touch nope).desktop';
    await DesktopApps(applicationLauncher: launcher).launch(
      const DesktopApp(
        id: 'literal.desktop',
        name: 'Literal',
        desktopFile: path,
      ),
    );
    expect(probes, 1);
    expect(calls, hasLength(1));
    expect(calls.single.$1, 'systemd-run');
    expect(calls.single.$2, [
      '--user',
      '--scope',
      '--quiet',
      '--collect',
      '--slice=app.slice',
      '--expand-environment=no',
      '--',
      'gio',
      'launch',
      path,
    ]);
  });

  test('only an absent manager permits one direct invocation', () async {
    final calls = <(String, List<String>)>[];
    final launcher = ApplicationLauncher(
      command: (program, arguments) async {
        calls.add((program, List.of(arguments)));
        return '';
      },
      scopeProbe: () async => null,
    );
    await launcher.launch('gio', ['open', '--', 'file:///literal%20name']);
    expect(calls, hasLength(1));
    expect(calls.single.$1, 'gio');
    expect(calls.single.$2, ['open', '--', 'file:///literal%20name']);
  });

  test(
    'an absent user-manager socket takes the production direct path',
    () async {
      final runtime = await Directory.systemTemp.createTemp(
        'clarion-no-manager-',
      );
      addTearDown(() => runtime.delete(recursive: true));
      final calls = <String>[];
      final launcher = ApplicationLauncher(
        environment: {'XDG_RUNTIME_DIR': runtime.path},
        command: (program, _) async {
          calls.add(program);
          return '';
        },
      );
      await launcher.launch('gio', ['launch', 'fixture.desktop']);
      expect(calls, ['gio']);
    },
  );

  test(
    'probe errors launch nothing, and scope launch failure never retries',
    () async {
      for (final failsBeforeLaunch in [true, false]) {
        final calls = <String>[];
        final launcher = ApplicationLauncher(
          scopeProbe: () async {
            if (failsBeforeLaunch) {
              throw StateError('manager unavailable to caller');
            }
            return const ApplicationScope();
          },
          command: (program, _) async {
            calls.add(program);
            throw StateError('GIO launch failure');
          },
        );
        await expectLater(
          launcher.launch('gio', ['launch', 'bad.desktop']),
          throwsStateError,
        );
        expect(calls, failsBeforeLaunch ? isEmpty : ['systemd-run']);
      }
    },
  );

  test('feature detection preserves old direct argv scope semantics', () {
    const oldHelp = '--scope --user --slice=SLICE --collect';
    final old = ApplicationScope.fromSystemdHelp(
      oldHelp,
      'systemd 253 (253.9)',
    );
    expect(old.arguments('gio', [r'${HOME}']), [
      '--user',
      '--scope',
      '--quiet',
      '--collect',
      '--slice=app.slice',
      '--',
      'gio',
      r'${HOME}',
    ]);
    final modern = ApplicationScope.fromSystemdHelp(
      '$oldHelp --expand-environment=BOOL',
      'systemd 262 (262-1-arch)',
    );
    expect(
      modern.arguments('gio', []).contains('--expand-environment=no'),
      isTrue,
    );
    expect(
      () => ApplicationScope.fromSystemdHelp(oldHelp, 'systemd 258'),
      throwsStateError,
    );
  });

  test(
    'scope client errors keep their exit status with inherited stdio',
    () async {
      final launcher = ApplicationLauncher(scopeProbe: () async => null);
      for (final waitForExit in [true, false]) {
        await expectLater(
          launcher.launch('/usr/bin/false', [], waitForExit: waitForExit),
          throwsA(
            isA<StateError>().having(
              (error) => error.message,
              'exit',
              contains('(1)'),
            ),
          ),
        );
      }
    },
  );

  test('foreground preference applications return while still alive', () async {
    final directory = await Directory.systemTemp.createTemp(
      'clarion-preferences-lifetime-',
    );
    const programs = ['pavucontrol', 'nm-connection-editor', 'blueman-manager'];
    addTearDown(() async {
      for (final program in programs) {
        final pidFile = File('${directory.path}/$program.pid');
        if (await pidFile.exists()) {
          final child = int.parse((await pidFile.readAsString()).trim());
          Process.killPid(child, ProcessSignal.sigkill);
        }
      }
      await directory.delete(recursive: true);
    });
    for (final program in programs) {
      final executable = File('${directory.path}/$program');
      await executable.writeAsString(
        '#!/bin/sh\n'
        r'echo $$ > "$0.pid"'
        '\nexec /bin/sleep 30\n',
      );
      final permissions = await Process.run('/usr/bin/chmod', [
        '700',
        executable.path,
      ]);
      expect(permissions.exitCode, 0);
    }
    final system = SystemService(
      applicationLauncher: ApplicationLauncher(
        environment: {'PATH': directory.path},
        scopeProbe: () async => null,
      ),
    );
    for (final open in [
      system.openSoundPreferences,
      system.openNetworkPreferences,
      system.openBluetoothPreferences,
    ]) {
      await open().timeout(const Duration(seconds: 3));
    }
    for (final program in programs) {
      final child = int.parse(
        (await File('${directory.path}/$program.pid').readAsString()).trim(),
      );
      expect(
        (await Process.run('/bin/kill', ['-0', '$child'])).exitCode,
        0,
        reason: '$program must remain open for the user',
      );
    }
  });

  test(
    'all Dock GUI folder actions scope GIO; hardware commands stay direct',
    () async {
      final calls = <(String, List<String>)>[];
      Future<String> command(String program, List<String> arguments) async {
        calls.add((program, List.of(arguments)));
        return program == 'xdg-user-dir' ? r'/home/demo/下载 ${HOME}' : '';
      }

      final service = SystemService(
        environment: {'HOME': '/home/demo'},
        command: command,
        applicationLauncher: ApplicationLauncher(
          command: command,
          scopeProbe: () async => const ApplicationScope(),
        ),
      );
      await service.openHomeFolder();
      await service.openTrash();
      await service.openDownloadsFolder();
      expect(calls.map((call) => call.$1), [
        'systemd-run',
        'systemd-run',
        'xdg-user-dir',
        'systemd-run',
      ]);
      expect(calls[0].$2.sublist(7), ['gio', 'open', '--', '/home/demo']);
      expect(calls[1].$2.sublist(7), ['gio', 'open', '--', 'trash:///']);
      expect(calls[3].$2.sublist(7), [
        'gio',
        'open',
        '--',
        r'/home/demo/下载 ${HOME}',
      ]);
    },
  );

  test(
    'desktop document and trusted shortcut use the same scoped launch',
    () async {
      final root = await Directory.systemTemp.createTemp(
        'clarion-launch-unit-',
      );
      addTearDown(() => root.delete(recursive: true));
      final document = File('${root.path}/literal document.txt');
      await document.writeAsString('fixture');
      final shortcut = File('${root.path}/literal app.desktop');
      await shortcut.writeAsString(
        '[Desktop Entry]\nType=Application\nName=Fixture\nExec=unused\n',
      );
      final calls = <(String, List<String>)>[];
      final service = DesktopService(
        root: root.path,
        command: (_, _) async =>
            throw StateError('Unexpected nonlaunch command'),
        applicationLauncher: ApplicationLauncher(
          command: (program, args) async {
            calls.add((program, List.of(args)));
            return '';
          },
          scopeProbe: () async => const ApplicationScope(),
        ),
      );
      addTearDown(service.dispose);
      await service.start();
      DesktopEntry entry(String path) =>
          service.entries.singleWhere((entry) => entry.fullPath == path);
      await service.openEntry(entry(document.path));
      await service.openEntry(
        entry(shortcut.path),
        allowUntrustedShortcut: true,
      );
      expect(service.error, isNull);
      expect(calls.map((call) => call.$1), ['systemd-run', 'systemd-run']);
      expect(calls.first.$2.sublist(7), ['gio', 'open', '--', document.path]);
      expect(calls.last.$2.sublist(7), ['gio', 'launch', shortcut.path]);
    },
  );
}
