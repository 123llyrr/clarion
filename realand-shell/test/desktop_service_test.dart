// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:realand_shell/desktop/desktop_entry.dart';
import 'package:realand_shell/desktop/desktop_service.dart';
import 'package:realand_shell/models.dart';
import 'package:realand_shell/services/application_launcher.dart';

class _Sandbox {
  _Sandbox(this.root);
  final Directory root;
  String get desktop => '${root.path}/home/桌面';
  String get home => '${root.path}/home';
  String get data => '${root.path}/data';
  Map<String, String> get environment => {
    'HOME': home,
    'XDG_DATA_HOME': data,
    'XDG_DATA_DIRS': '${root.path}/system-data',
    'LANG': 'zh_CN.UTF-8',
  };

  final commands = <(String, List<String>)>[];
  Future<String?> Function(String, List<String>)? intercept;
  String? xdgResult;

  Future<String> command(String executable, List<String> arguments) async {
    commands.add((executable, List.of(arguments)));
    final overridden = await intercept?.call(executable, arguments);
    if (overridden != null) return overridden;
    if (executable == 'xdg-user-dir') return xdgResult ?? desktop;
    if (executable == 'gio') return '';
    // Coreutils only operate on this temporary sandbox. GIO never launches an
    // actual application or trashes a real file during these tests.
    if (executable == 'mv' || executable == 'chmod' || executable == 'mkdir') {
      final paths = executable == 'mv'
          ? arguments.sublist(arguments.length - 2)
          : [arguments.last];
      if (paths.any((path) => !path.startsWith('${root.path}/'))) {
        throw StateError('Test command escaped its sandbox');
      }
      final result = await Process.run(executable, arguments);
      if (result.exitCode != 0) throw StateError('${result.stderr}');
      return '${result.stdout}';
    }
    throw StateError('Unexpected external command: $executable');
  }

  Future<DesktopService> service({
    String? path,
    bool resolveXdg = false,
  }) async {
    final service = DesktopService(
      root: resolveXdg ? null : path ?? desktop,
      environment: environment,
      command: command,
      applicationLauncher: ApplicationLauncher(
        command: command,
        scopeProbe: () async => null,
      ),
      debounceDelay: const Duration(milliseconds: 15),
    );
    addTearDown(service.dispose);
    await service.start();
    return service;
  }

  Future<DesktopApp> installedApp() async {
    final applications = Directory('$data/applications');
    await applications.create(recursive: true);
    final source = File('${applications.path}/browser.desktop');
    await source.writeAsString(
      '[Desktop Entry]\nType=Application\nName=Browser\nName[zh_CN]=浏览器\n'
      'DBusActivatable=true\nIcon=web-browser\n'
      'Exec=sh -c "touch should-never-run"\n',
    );
    return DesktopApp(
      id: 'browser.desktop',
      name: '浏览器',
      desktopFile: source.path,
    );
  }
}

Future<_Sandbox> _sandbox({bool desktopExists = true}) async {
  final root = await Directory.systemTemp.createTemp('realand-desktop-test-');
  addTearDown(() => root.delete(recursive: true));
  final sandbox = _Sandbox(root);
  await Directory(sandbox.home).create();
  if (desktopExists) await Directory(sandbox.desktop).create();
  return sandbox;
}

DesktopEntry _entry(DesktopService service, String name) =>
    service.entries.singleWhere((entry) => entry.name == name);

Future<void> _awaitCondition(bool Function() condition) async {
  final deadline = DateTime.now().add(const Duration(seconds: 3));
  while (!condition()) {
    if (DateTime.now().isAfter(deadline)) throw StateError('Watcher timed out');
    await Future<void>.delayed(const Duration(milliseconds: 10));
  }
}

void main() {
  test(
    'bulk trash preflights every path before touching a valid selection',
    () async {
      final sandbox = await _sandbox();
      final valid = File('${sandbox.desktop}/a.txt')..writeAsStringSync('keep');
      final outside = File('${sandbox.home}/private.txt')
        ..writeAsStringSync('private');
      final service = await sandbox.service();
      final forged = DesktopEntry(fullPath: outside.path, name: 'private.txt');
      await service.trashEntries([service.entries.single, forged]);
      expect(sandbox.commands, isEmpty);
      expect(service.error, contains('直接项目'));
      expect(valid.existsSync(), isTrue);
      expect(outside.existsSync(), isTrue);
      await service.trashEntries([
        service.entries.single,
        DesktopEntry(
          fullPath: '${sandbox.desktop}/missing.txt',
          name: 'missing.txt',
        ),
      ]);
      expect(sandbox.commands, isEmpty);
      expect(valid.existsSync(), isTrue);
    },
  );

  test(
    'bulk trash stops at failure, refreshes completed entries and never deletes as fallback',
    () async {
      final sandbox = await _sandbox();
      for (final name in ['a.txt', 'b.txt', 'c.txt']) {
        File('${sandbox.desktop}/$name').writeAsStringSync(name);
      }
      final service = await sandbox.service();
      sandbox.intercept = (program, arguments) async {
        expect(service.busy, isTrue);
        expect(program, 'gio');
        expect(arguments.take(2), ['trash', '--']);
        if (arguments.last.endsWith('/b.txt')) {
          throw StateError('fixture failure');
        }
        // Only the disposable fixture simulates the external trash backend.
        await File(arguments.last).delete();
        return '';
      };
      await service.trashEntries(service.entries);
      expect(sandbox.commands.map((command) => command.$2.last), [
        '${sandbox.desktop}/a.txt',
        '${sandbox.desktop}/b.txt',
      ]);
      expect(service.entries.map((entry) => entry.name), ['b.txt', 'c.txt']);
      expect(service.error, contains('已完成 1/3'));
      expect(service.error, contains('b.txt'));
      expect(service.busy, isFalse);
      expect(File('${sandbox.desktop}/b.txt').existsSync(), isTrue);
      expect(File('${sandbox.desktop}/c.txt').existsSync(), isTrue);
    },
  );

  test(
    'bulk open preflights trust and stops without duplicate launches on errors',
    () async {
      final sandbox = await _sandbox();
      File('${sandbox.desktop}/a.txt').writeAsStringSync('a');
      File('${sandbox.desktop}/b.desktop').writeAsStringSync(
        '[Desktop Entry]\nType=Application\nName=B\nExec=never-run\n',
      );
      File('${sandbox.desktop}/c.txt').writeAsStringSync('c');
      final service = await sandbox.service();
      final entries = List<DesktopEntry>.of(service.entries);
      await service.openEntries(entries);
      expect(sandbox.commands, isEmpty);
      expect(service.error, contains('尚未受信任'));
      sandbox.intercept = (program, arguments) async {
        expect(service.busy, isTrue);
        if (arguments.first == 'launch') throw StateError('launch failed');
        return '';
      };
      await service.openEntries(
        [...entries, entries.first],
        allowedUntrustedShortcuts: {'${sandbox.desktop}/b.desktop'},
      );
      expect(sandbox.commands.map((command) => command.$2), [
        ['open', '--', '${sandbox.desktop}/a.txt'],
        ['launch', '${sandbox.desktop}/b.desktop'],
      ]);
      expect(service.error, contains('已完成 1/3'));
      expect(service.entries, hasLength(3));
      expect(service.busy, isFalse);
    },
  );

  test(
    'bulk trash refuses a desktop root replacement between selected items',
    () async {
      final sandbox = await _sandbox();
      for (final name in ['a.txt', 'b.txt']) {
        File('${sandbox.desktop}/$name').writeAsStringSync('original');
      }
      final replacement = Directory('${sandbox.home}/other')..createSync();
      File('${replacement.path}/b.txt').writeAsStringSync('keep');
      final service = await sandbox.service();
      sandbox.intercept = (program, arguments) async {
        await File(arguments.last).delete();
        await Directory(sandbox.desktop).rename('${sandbox.desktop}-original');
        await Link(sandbox.desktop).create(replacement.path);
        return '';
      };
      await service.trashEntries(service.entries);
      expect(sandbox.commands, hasLength(1));
      expect(service.error, contains('指向已改变'));
      expect(File('${replacement.path}/b.txt').readAsStringSync(), 'keep');
      expect(File('${sandbox.desktop}-original/b.txt').existsSync(), isTrue);
    },
  );

  test(
    'xdg resolution uses argv and startup only reads its directory',
    () async {
      final sandbox = await _sandbox();
      final original = File('${sandbox.desktop}/original.txt');
      await original.writeAsString('keep');
      final service = await sandbox.service(resolveXdg: true);
      expect(service.directoryPath, sandbox.desktop);
      expect(service.entries.single.name, 'original.txt');
      expect(service.error, isNull);
      expect(sandbox.commands, hasLength(1));
      expect(sandbox.commands.single.$1, 'xdg-user-dir');
      expect(sandbox.commands.single.$2, ['DESKTOP']);
      expect(await original.readAsString(), 'keep');
    },
  );

  test('an injected root never queries the real desktop directory', () async {
    final sandbox = await _sandbox();
    final service = await sandbox.service();
    expect(service.directoryPath, sandbox.desktop);
    expect(service.entries, isEmpty);
    expect(sandbox.commands, isEmpty);
  });

  test(
    'default directory resolver receives the injected environment',
    () async {
      final sandbox = await _sandbox();
      final bin = Directory('${sandbox.root.path}/bin');
      await bin.create();
      final resolver = File('${bin.path}/xdg-user-dir');
      await resolver.writeAsString('#!/bin/sh\nprintf \'%s\\n\' "\$HOME/桌面"\n');
      final permissions = await Process.run('chmod', [
        'u+x',
        '--',
        resolver.path,
      ]);
      expect(permissions.exitCode, 0);
      final service = DesktopService(
        environment: {...sandbox.environment, 'PATH': bin.path},
      );
      addTearDown(service.dispose);
      await service.start();
      expect(service.directoryPath, sandbox.desktop);
      expect(service.error, isNull);
    },
  );

  test(
    'HOME means desktop disabled and cannot become a write target',
    () async {
      final sandbox = await _sandbox();
      sandbox.xdgResult = sandbox.home;
      final personal = File('${sandbox.home}/private.txt');
      await personal.writeAsString('private');
      final service = await sandbox.service(resolveXdg: true);
      expect(service.directoryPath, isNull);
      expect(service.entries, isEmpty);
      expect(service.error, contains('禁用'));
      await service.createFile('unexpected.txt');
      expect(await File('${sandbox.home}/unexpected.txt').exists(), isFalse);
      expect(await personal.readAsString(), 'private');
    },
  );

  for (final path in ['relative/desktop', '/']) {
    test('unsafe resolved directory $path has no fallback', () async {
      final sandbox = await _sandbox();
      sandbox.xdgResult = path;
      final service = await sandbox.service(resolveXdg: true);
      expect(service.directoryPath, isNull);
      expect(service.entries, isEmpty);
      expect(service.error, isNotNull);
    });
  }

  test(
    'startup does not create a missing directory; explicit creation can',
    () async {
      final sandbox = await _sandbox(desktopExists: false);
      final service = await sandbox.service();
      expect(await Directory(sandbox.desktop).exists(), isFalse);
      expect(service.error, contains('尚不存在'));
      await service.createFile('hello.txt');
      expect(await File('${sandbox.desktop}/hello.txt').exists(), isTrue);
      expect(service.error, isNull);
      expect(service.entries.single.name, 'hello.txt');
    },
  );

  test(
    'scan includes direct files, folders and links without recursing',
    () async {
      final sandbox = await _sandbox();
      final folder = Directory('${sandbox.desktop}/文件夹');
      await folder.create();
      await File('${folder.path}/nested.txt').writeAsString('nested');
      await File('${sandbox.desktop}/document.txt').writeAsString('text');
      await File('${sandbox.desktop}/.hidden').writeAsString('hidden');
      await Link('${sandbox.desktop}/folder-link').create(folder.path);
      await Link(
        '${sandbox.desktop}/broken-link',
      ).create('${sandbox.root.path}/missing');
      final service = await sandbox.service();
      expect(service.entries.map((entry) => entry.name).toSet(), {
        '文件夹',
        'document.txt',
        'folder-link',
        'broken-link',
      });
      expect(_entry(service, '文件夹').isDirectory, isTrue);
      final link = _entry(service, 'folder-link');
      expect(link.isDirectory, isTrue);
      expect(link.isSymlink, isTrue);
      expect(link.targetPath, folder.path);
      expect(_entry(service, 'broken-link').isSymlink, isTrue);
      expect(
        service.entries.any((entry) => entry.name == 'nested.txt'),
        isFalse,
      );
    },
  );

  test(
    'launcher metadata is read without executing and unknown launch needs confirmation',
    () async {
      final sandbox = await _sandbox();
      final launcher = File('${sandbox.desktop}/download.desktop');
      await launcher.writeAsString(
        '[Desktop Entry]\nType=Application\nName=Download\nIcon=test-icon\nExec=touch dangerous\n',
      );
      final service = await sandbox.service();
      final entry = service.entries.single;
      expect(entry.displayName, 'Download');
      expect(entry.iconName, 'test-icon');
      expect(entry.isDesktopShortcut, isTrue);
      expect(entry.trustedShortcut, isFalse);
      expect(sandbox.commands, isEmpty);
      await service.openEntry(entry);
      expect(service.error, contains('确认'));
      expect(sandbox.commands, isEmpty);
      await service.openEntry(entry, allowUntrustedShortcut: true);
      expect(sandbox.commands.single.$1, 'gio');
      expect(sandbox.commands.single.$2, ['launch', launcher.path]);
    },
  );

  test(
    'opening an ordinary filename passes literal argv, never shell code',
    () async {
      final sandbox = await _sandbox();
      const name = r'notes $(touch injected); quote".txt';
      final file = File('${sandbox.desktop}/$name');
      await file.writeAsString('notes');
      final service = await sandbox.service();
      await service.openEntry(service.entries.single);
      expect(sandbox.commands.single.$1, 'gio');
      expect(sandbox.commands.single.$2, ['open', '--', file.path]);
      expect(await File('${sandbox.desktop}/injected').exists(), isFalse);
    },
  );

  test(
    'installed shortcut copies exact data and is trusted after restart',
    () async {
      final sandbox = await _sandbox();
      final app = await sandbox.installedApp();
      final original = await File(app.desktopFile).readAsString();
      final service = await sandbox.service();
      await service.createShortcutInstalledApp(app, name: '浏览器.desktop');
      expect(service.error, isNull);
      final entry = service.entries.single;
      expect(entry.isDesktopShortcut, isTrue);
      expect(entry.trustedShortcut, isTrue);
      expect(entry.displayName, '浏览器');
      expect(await File(entry.fullPath).readAsString(), original);
      expect(sandbox.commands.map((command) => command.$1), ['chmod', 'mv']);
      expect(
        await File('${sandbox.desktop}/should-never-run').exists(),
        isFalse,
      );
      service.dispose();
      final restarted = await sandbox.service();
      expect(restarted.entries.single.trustedShortcut, isTrue);
      await restarted.openEntry(restarted.entries.single);
      expect(sandbox.commands.last.$2, ['launch', entry.fullPath]);
      expect(await File(app.desktopFile).readAsString(), original);
    },
  );

  test('shortcut source outside the installed catalogue is rejected', () async {
    final sandbox = await _sandbox();
    final source = File('${sandbox.root.path}/download.desktop');
    await source.writeAsString(
      '[Desktop Entry]\nType=Application\nName=Download\nDBusActivatable=true\n',
    );
    final service = await sandbox.service();
    await service.createShortcutInstalledApp(
      DesktopApp(
        id: 'download.desktop',
        name: 'Download',
        desktopFile: source.path,
      ),
    );
    expect(service.error, contains('已安装'));
    expect(service.entries, isEmpty);
    expect(sandbox.commands, isEmpty);
  });

  test('an existing shortcut is never overwritten or marked trusted', () async {
    final sandbox = await _sandbox();
    final app = await sandbox.installedApp();
    final existing = File('${sandbox.desktop}/browser.desktop');
    await existing.writeAsString(
      '[Desktop Entry]\nType=Application\nName=Existing\nExec=existing\n',
    );
    final beforeMode = (await existing.stat()).mode;
    final service = await sandbox.service();
    await service.createShortcutInstalledApp(app, name: 'browser.desktop');
    expect(service.error, contains('同名'));
    expect(await existing.readAsString(), contains('Name=Existing'));
    expect((await existing.stat()).mode, beforeMode);
    expect(sandbox.commands, isEmpty);
    expect(service.entries.single.trustedShortcut, isFalse);
  });

  test(
    'a racing shortcut destination leaves existing data and trust untouched',
    () async {
      final sandbox = await _sandbox();
      final app = await sandbox.installedApp();
      final destination = File('${sandbox.desktop}/browser.desktop');
      final service = await sandbox.service();
      sandbox.intercept = (program, arguments) async {
        if (program == 'mv') {
          await destination.writeAsString('racing existing data');
        }
        return null;
      };
      await service.createShortcutInstalledApp(app, name: 'browser.desktop');
      expect(service.error, contains('同名'));
      expect(await destination.readAsString(), 'racing existing data');
      expect((await destination.stat()).mode & 0x49, 0);
      expect(
        sandbox.commands
            .where((command) => command.$1 == 'chmod')
            .single
            .$2
            .last,
        isNot(destination.path),
      );
      expect(
        await Directory(
          sandbox.desktop,
        ).list().map((entry) => entry.path).toList(),
        [destination.path],
      );
      await Future<void>.delayed(const Duration(milliseconds: 80));
      expect(
        service.error,
        contains('同名'),
        reason: 'watch refresh must not erase an operation error',
      );
    },
  );

  test(
    'file and directory links open without scanning their targets',
    () async {
      final sandbox = await _sandbox();
      final outside = Directory('${sandbox.root.path}/outside');
      await outside.create();
      final target = File('${outside.path}/target.txt');
      await target.writeAsString('outside');
      final service = await sandbox.service();
      await service.createShortcutPath(target.path, name: 'file-link');
      await service.createShortcutPath(outside.path, name: 'folder-link');
      expect(service.entries.map((entry) => entry.name).toSet(), {
        'file-link',
        'folder-link',
      });
      expect(_entry(service, 'folder-link').isDirectory, isTrue);
      expect(_entry(service, 'file-link').targetPath, target.path);
      expect(await target.readAsString(), 'outside');
      expect(sandbox.commands, isEmpty);
      await service.openEntry(_entry(service, 'folder-link'));
      expect(sandbox.commands.single.$2, [
        'open',
        '--',
        '${sandbox.desktop}/folder-link',
      ]);
    },
  );

  test('missing or remote shortcut targets never create files', () async {
    final sandbox = await _sandbox();
    final service = await sandbox.service();
    await service.createShortcutPath('${sandbox.root.path}/missing');
    expect(service.error, contains('不存在'));
    await service.createShortcutPath('https://example.com');
    expect(service.error, contains('绝对路径'));
    expect(service.entries, isEmpty);
  });

  test(
    'create collisions include existing files, folders and dangling links',
    () async {
      final sandbox = await _sandbox();
      final file = File('${sandbox.desktop}/existing');
      await file.writeAsString('preserve');
      final folder = Directory('${sandbox.desktop}/folder');
      await folder.create();
      await File('${folder.path}/child').writeAsString('child');
      final broken = Link('${sandbox.desktop}/broken');
      await broken.create('${sandbox.root.path}/missing');
      final service = await sandbox.service();
      for (final name in ['existing', 'folder', 'broken']) {
        await service.createFile(name);
        expect(service.error, contains('同名'));
        await service.createDirectory(name);
        expect(service.error, contains('同名'));
      }
      expect(await file.readAsString(), 'preserve');
      expect(await File('${folder.path}/child').readAsString(), 'child');
      expect(await broken.target(), '${sandbox.root.path}/missing');
    },
  );

  test(
    'creating a directory uses literal argv and creates a single child',
    () async {
      final sandbox = await _sandbox();
      final service = await sandbox.service();
      const name = r'folder $(touch injected); "quote"';
      await service.createDirectory(name);
      expect(service.error, isNull);
      expect(await Directory('${sandbox.desktop}/$name').exists(), isTrue);
      expect(service.entries.single.name, name);
      expect(sandbox.commands.single.$1, 'mkdir');
      expect(sandbox.commands.single.$2, ['--', '${sandbox.desktop}/$name']);
    },
  );

  test(
    'directory creation atomically refuses a racing same-name directory',
    () async {
      final sandbox = await _sandbox();
      final service = await sandbox.service();
      final target = Directory('${sandbox.desktop}/folder');
      final preserved = File('${target.path}/preserved.txt');
      sandbox.intercept = (program, arguments) async {
        if (program == 'mkdir') {
          await target.create();
          await preserved.writeAsString('racing directory');
        }
        return null;
      };
      await service.createDirectory('folder');
      expect(service.error, isNotNull);
      expect(await preserved.readAsString(), 'racing directory');
      expect(sandbox.commands.single.$1, 'mkdir');
      await Future<void>.delayed(const Duration(milliseconds: 60));
      expect(service.error, isNotNull);
      expect(await preserved.readAsString(), 'racing directory');
    },
  );

  test(
    'invalid filenames cannot traverse outside the desktop or exceed filesystem limits',
    () async {
      final sandbox = await _sandbox();
      final service = await sandbox.service();
      for (final name in [
        '',
        '  ',
        '.',
        '..',
        '../outside',
        'sub/file',
        '/absolute',
        'nul\u0000name',
        'line\nbreak',
        '中' * 86,
      ]) {
        await service.createFile(name);
        expect(service.error, contains('名称'), reason: name);
        await service.createDirectory(name);
        expect(service.error, contains('名称'), reason: name);
      }
      expect(await Directory(sandbox.desktop).list().isEmpty, isTrue);
      expect(await File('${sandbox.home}/outside').exists(), isFalse);
    },
  );

  test('rename moves one item and refuses an existing destination', () async {
    final sandbox = await _sandbox();
    await File('${sandbox.desktop}/before.txt').writeAsString('before');
    await File('${sandbox.desktop}/occupied.txt').writeAsString('occupied');
    final service = await sandbox.service();
    await service.renameEntry(_entry(service, 'before.txt'), 'occupied.txt');
    expect(service.error, contains('同名'));
    expect(
      await File('${sandbox.desktop}/occupied.txt').readAsString(),
      'occupied',
    );
    expect(sandbox.commands, isEmpty);
    await service.renameEntry(_entry(service, 'before.txt'), 'after.txt');
    expect(service.error, isNull);
    expect(await File('${sandbox.desktop}/before.txt').exists(), isFalse);
    expect(await File('${sandbox.desktop}/after.txt').readAsString(), 'before');
    expect(sandbox.commands.single.$1, 'mv');
  });

  test(
    'a rename collision appearing after validation is never overwritten',
    () async {
      final sandbox = await _sandbox();
      final source = File('${sandbox.desktop}/source.txt');
      final target = File('${sandbox.desktop}/target.txt');
      await source.writeAsString('source');
      final service = await sandbox.service();
      sandbox.intercept = (program, arguments) async {
        if (program == 'mv') await target.writeAsString('racing target');
        return null;
      };
      await service.renameEntry(service.entries.single, 'target.txt');
      expect(service.error, contains('同名'));
      expect(await source.readAsString(), 'source');
      expect(await target.readAsString(), 'racing target');
    },
  );

  test('rename of a folder link never changes its external target', () async {
    final sandbox = await _sandbox();
    final outside = Directory('${sandbox.root.path}/external');
    await outside.create();
    await File('${outside.path}/child').writeAsString('keep');
    await Link('${sandbox.desktop}/link').create(outside.path);
    final service = await sandbox.service();
    await service.renameEntry(service.entries.single, 'renamed-link');
    expect(service.error, isNull);
    expect(
      await Link('${sandbox.desktop}/renamed-link').target(),
      outside.path,
    );
    expect(await File('${outside.path}/child').readAsString(), 'keep');
  });

  test('forged entries cannot open, rename or trash an outside path', () async {
    final sandbox = await _sandbox();
    final outside = File('${sandbox.home}/private.txt');
    await outside.writeAsString('private');
    final service = await sandbox.service();
    final entry = DesktopEntry(
      fullPath: '${sandbox.desktop}/../private.txt',
      name: 'private.txt',
      trustedShortcut: true,
    );
    await service.openEntry(entry);
    expect(service.error, contains('直接项目'));
    await service.renameEntry(entry, 'renamed.txt');
    expect(service.error, contains('直接项目'));
    await service.trashEntry(entry);
    expect(service.error, contains('直接项目'));
    expect(sandbox.commands, isEmpty);
    expect(await outside.readAsString(), 'private');
  });

  test(
    'trash only delegates to GIO and has no recursive-delete fallback',
    () async {
      final sandbox = await _sandbox();
      final folder = Directory('${sandbox.desktop}/folder');
      await folder.create();
      final child = File('${folder.path}/keep.txt');
      await child.writeAsString('keep');
      final service = await sandbox.service();
      await service.trashEntry(service.entries.single);
      expect(sandbox.commands.single.$1, 'gio');
      expect(sandbox.commands.single.$2, ['trash', '--', folder.path]);
      expect(await child.readAsString(), 'keep');
    },
  );

  test(
    'watching refreshes external changes and dispose stops notifications',
    () async {
      final sandbox = await _sandbox();
      final service = await sandbox.service();
      var notifications = 0;
      service.addListener(() => notifications++);
      await File('${sandbox.desktop}/external.txt').writeAsString('external');
      await _awaitCondition(
        () => service.entries.any((entry) => entry.name == 'external.txt'),
      );
      expect(notifications, greaterThan(0));
      service.dispose();
      final atDispose = notifications;
      await File('${sandbox.desktop}/after-dispose.txt').writeAsString('after');
      await Future<void>.delayed(const Duration(milliseconds: 80));
      expect(notifications, atDispose);
      expect(
        service.entries.any((entry) => entry.name == 'after-dispose.txt'),
        isFalse,
      );
    },
  );

  test(
    'watching the last file removal publishes an empty entry list',
    () async {
      final sandbox = await _sandbox();
      final lastFile = File('${sandbox.desktop}/last-file.txt');
      await lastFile.writeAsString('last file');
      final service = await sandbox.service();
      expect(service.entries.single.name, 'last-file.txt');
      var publishedEmpty = false;
      service.addListener(() {
        if (service.entries.isEmpty) publishedEmpty = true;
      });
      await lastFile.delete();
      await _awaitCondition(() => service.entries.isEmpty && publishedEmpty);
      expect(service.error, isNull);
      expect(await Directory(sandbox.desktop).exists(), isTrue);
      await lastFile.writeAsString('new file');
      await _awaitCondition(() => service.entries.length == 1);
      expect(service.entries.single.name, 'last-file.txt');
      expect(service.error, isNull);
    },
  );

  test(
    'disposed directory resolution cannot publish entries or create files',
    () async {
      final sandbox = await _sandbox();
      final gate = Completer<String>();
      sandbox.intercept = (program, _) async =>
          program == 'xdg-user-dir' ? gate.future : null;
      final service = DesktopService(
        environment: sandbox.environment,
        command: sandbox.command,
      );
      addTearDown(service.dispose);
      var notifications = 0;
      service.addListener(() => notifications++);
      final started = service.start();
      final operation = service.createFile('unexpected.txt');
      service.dispose();
      final atDispose = notifications;
      gate.complete(sandbox.desktop);
      await Future.wait([started, operation]);
      expect(notifications, atDispose);
      expect(service.entries, isEmpty);
      expect(await File('${sandbox.desktop}/unexpected.txt').exists(), isFalse);
    },
  );

  test(
    'a desktop root retargeted to another directory refuses mutations',
    () async {
      final sandbox = await _sandbox();
      final link = Link('${sandbox.root.path}/desktop-link');
      await link.create(sandbox.desktop);
      final outside = Directory('${sandbox.root.path}/outside');
      await outside.create();
      final service = await sandbox.service(path: link.path);
      await link.delete();
      await link.create(outside.path);
      await service.createFile('unexpected.txt');
      expect(service.error, contains('指向已改变'));
      expect(await File('${outside.path}/unexpected.txt').exists(), isFalse);
    },
  );
}
