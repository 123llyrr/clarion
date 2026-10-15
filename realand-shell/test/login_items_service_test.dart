// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:io';
import 'package:flutter_test/flutter_test.dart';
import 'package:realand_shell/models.dart';
import 'package:realand_shell/services/desktop_apps.dart';
import 'package:realand_shell/services/login_items_service.dart';

void main() {
  late Directory root;
  late LoginItemsService service;
  late DesktopApp app;
  const source =
      '[Desktop Entry]\nType=Application\nName=Example\n'
      'Name[zh_CN]=示例\nExec=/bin/echo "a b" %U\nIcon=example\n'
      '# retained comment\n\n[Desktop Action new]\nName=New\nExec=/bin/echo new\n';

  Future<File> entry(String location, String text) async {
    final file = File('${root.path}/$location/autostart/example.desktop');
    await file.parent.create(recursive: true);
    return file.writeAsString(text);
  }

  setUp(() async {
    root = await Directory.systemTemp.createTemp('realand-login-test-');
    final desktop = await File(
      '${root.path}/example.desktop',
    ).writeAsString(source);
    app = DesktopApp(
      id: 'example.desktop',
      name: 'Example',
      desktopFile: desktop.path,
    );
    service = LoginItemsService(
      environment: {
        'HOME': root.path,
        'XDG_CONFIG_HOME': '${root.path}/user',
        'XDG_CONFIG_DIRS': '${root.path}/system:${root.path}/vendor',
        'XDG_CURRENT_DESKTOP': 'realand',
      },
    );
  });
  tearDown(() async => root.delete(recursive: true));

  test('reading an absent login item does not create config', () async {
    expect(await service.isEnabled(app), isFalse);
    expect(await Directory('${root.path}/user').exists(), isFalse);
  });

  test(
    'toggle persists across service instances without changing launcher',
    () async {
      await service.setEnabled(app, true);
      expect(
        await LoginItemsService(
          environment: service.environment,
        ).isEnabled(app),
        isTrue,
      );
      final saved = await File(
        '${root.path}/user/autostart/example.desktop',
      ).readAsString();
      expect(saved, contains('Exec=/bin/echo "a b" %U'));
      expect(saved, contains('Name[zh_CN]=示例'));
      expect(
        saved,
        contains('[Desktop Action new]\nName=New\nExec=/bin/echo new'),
      );
      expect(saved, contains('# retained comment'));
      expect(await File(app.desktopFile).readAsString(), source);
      await service.setEnabled(app, false);
      expect(await service.isEnabled(app), isFalse);
      await service.setEnabled(app, true);
      expect(await service.isEnabled(app), isTrue);
      final files = await Directory(
        '${root.path}/user/autostart',
      ).list().toList();
      expect(files.map((file) => file.path), [
        '${root.path}/user/autostart/example.desktop',
      ]);
    },
  );

  test('system item is masked locally and original remains intact', () async {
    final system = await entry('system', source);
    expect(await service.isEnabled(app), isTrue);
    await service.setEnabled(app, false);
    expect(await service.isEnabled(app), isFalse);
    expect(await system.readAsString(), source);
    expect(
      parseDesktopEntry(
        await File(
          '${root.path}/user/autostart/example.desktop',
        ).readAsString(),
      )['Hidden'],
      'true',
    );
  });

  test(
    'highest priority hidden override wins including minimal tombstones',
    () async {
      await entry('vendor', source);
      await entry('system', '[Desktop Entry]\nHidden=true\n');
      expect(await service.isEnabled(app), isFalse);
      await service.setEnabled(app, true);
      expect(await service.isEnabled(app), isTrue);
      expect(
        await File(
          '${root.path}/system/autostart/example.desktop',
        ).readAsString(),
        '[Desktop Entry]\nHidden=true\n',
      );
    },
  );

  test(
    'desktop visibility and desktop-specific disable flags are honored',
    () async {
      await entry(
        'system',
        source.replaceFirst(
          'Type=Application',
          'Type=Application\nOnlyShowIn=GNOME;\nX-GNOME-Autostart-enabled=false\nX-systemd-skip=true',
        ),
      );
      expect(await service.isEnabled(app), isFalse);
      await service.setEnabled(app, true);
      expect(await service.isEnabled(app), isTrue);
      final saved = parseDesktopEntry(
        await File(
          '${root.path}/user/autostart/example.desktop',
        ).readAsString(),
      );
      expect(saved['OnlyShowIn'], 'GNOME;realand;');
      expect(saved['X-GNOME-Autostart-enabled'], 'true');
      expect(saved['X-systemd-skip'], 'false');
    },
  );

  test('enabling preserves other desktops excluded by NotShowIn', () async {
    await entry(
      'system',
      source.replaceFirst(
        'Type=Application',
        'Type=Application\nNotShowIn=realand;KDE;',
      ),
    );
    expect(await service.isEnabled(app), isFalse);
    await service.setEnabled(app, true);
    expect(await service.isEnabled(app), isTrue);
    final saved = parseDesktopEntry(
      await File('${root.path}/user/autostart/example.desktop').readAsString(),
    );
    expect(saved['NotShowIn'], 'KDE;');
  });

  test(
    'an empty OnlyShowIn list excludes every desktop until enabled',
    () async {
      final original = source.replaceFirst(
        'Type=Application',
        'Type=Application\nOnlyShowIn=',
      );
      final system = await entry('system', original);
      expect(await service.isEnabled(app), isFalse);
      await service.setEnabled(app, true);
      expect(await service.isEnabled(app), isTrue);
      expect(await system.readAsString(), original);
      final saved = parseDesktopEntry(
        await File(
          '${root.path}/user/autostart/example.desktop',
        ).readAsString(),
      );
      expect(saved['OnlyShowIn'], 'realand;');
    },
  );

  test('user symlink is replaced without editing system target', () async {
    final system = await entry('system', source);
    final directory = Directory('${root.path}/user/autostart');
    await directory.create(recursive: true);
    final path = '${directory.path}/example.desktop';
    await Link(path).create(system.path);
    await service.setEnabled(app, false);
    expect(
      await FileSystemEntity.type(path, followLinks: false),
      FileSystemEntityType.file,
    );
    expect(await system.readAsString(), source);
  });

  test(
    'invalid IDs and malformed launchers cannot create a login item',
    () async {
      for (final id in [
        '../example.desktop',
        'foo/bar.desktop',
        'foo\\bar.desktop',
        'bad\n.desktop',
      ]) {
        final invalid = DesktopApp(
          id: id,
          name: app.name,
          desktopFile: app.desktopFile,
        );
        await expectLater(
          service.setEnabled(invalid, true),
          throwsArgumentError,
        );
      }
      await File(
        app.desktopFile,
      ).writeAsString('[Desktop Entry]\nType=Application\nName=Broken\n');
      await expectLater(service.setEnabled(app, true), throwsStateError);
      expect(await Directory('${root.path}/user').exists(), isFalse);
    },
  );

  test(
    'conditional startup reports failure and keeps original configuration',
    () async {
      final conditional = source.replaceFirst(
        'Type=Application',
        'Type=Application\nHidden=true\nAutostartCondition=GNOME3 if-session gnome',
      );
      final file = await entry('user', conditional);
      await expectLater(service.setEnabled(app, true), throwsStateError);
      expect(await file.readAsString(), conditional);
    },
  );

  test('demo toggles never touch the filesystem', () async {
    final demo = LoginItemsService(environment: {}, demo: true);
    await demo.setEnabled(app, true);
    expect(await demo.isEnabled(app), isTrue);
    await demo.setEnabled(app, false);
    expect(await demo.isEnabled(app), isFalse);
  });

  test(
    'TryExec accepts executable absolute paths, PATH names and symlinks',
    () async {
      final bin = await Directory('${root.path}/bin').create();
      final executable = await File(
        '${bin.path}/available',
      ).writeAsString('#!/bin/sh\nexit 0\n');
      expect(
        (await Process.run('chmod', ['755', executable.path])).exitCode,
        0,
      );
      final link = await Link('${bin.path}/linked').create(executable.path);
      final withPath = LoginItemsService(
        environment: {...service.environment, 'PATH': bin.path},
      );
      for (final value in [executable.path, 'available', link.path, 'linked']) {
        final original = source.replaceFirst(
          'Type=Application',
          'Type=Application\nTryExec=$value',
        );
        final file = await entry('user', original);
        expect(await withPath.isEnabled(app), isTrue, reason: value);
        expect(await file.readAsString(), original);
        await withPath.setEnabled(app, false);
        expect(await withPath.isEnabled(app), isFalse);
        await withPath.setEnabled(app, true);
        expect(await withPath.isEnabled(app), isTrue);
      }
    },
  );

  test(
    'unavailable or malformed TryExec never overwrites an existing item',
    () async {
      final bin = await Directory('${root.path}/bin').create();
      final plain = await File(
        '${bin.path}/plain',
      ).writeAsString('not executable');
      final folder = await Directory('${bin.path}/subdir').create();
      final nested = await File(
        '${folder.path}/program',
      ).writeAsString('#!/bin/sh\nexit 0\n');
      expect((await Process.run('chmod', ['755', nested.path])).exitCode, 0);
      final withPath = LoginItemsService(
        environment: {...service.environment, 'PATH': bin.path},
      );
      for (final value in [
        '${bin.path}/missing',
        'missing',
        plain.path,
        'plain',
        folder.path,
        'subdir/program',
      ]) {
        final original = source.replaceFirst(
          'Type=Application',
          'Type=Application\nTryExec=$value',
        );
        final file = await entry('user', original);
        expect(await withPath.isEnabled(app), isFalse, reason: value);
        await expectLater(withPath.setEnabled(app, true), throwsStateError);
        expect(await file.readAsString(), original);
      }
    },
  );
}
