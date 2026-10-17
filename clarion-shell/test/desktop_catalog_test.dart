// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/services/desktop_apps.dart';

class _CatalogFixture {
  _CatalogFixture(this.root);
  final Directory root;
  Directory get user => Directory('${root.path}/user/applications');
  Directory get system => Directory('${root.path}/system/applications');
  Directory get bin => Directory('${root.path}/bin');

  static Future<_CatalogFixture> create() async {
    final fixture = _CatalogFixture(
      await Directory.systemTemp.createTemp('clarion-catalog-'),
    );
    await fixture.user.create(recursive: true);
    await fixture.system.create(recursive: true);
    await fixture.bin.create();
    return fixture;
  }

  DesktopApps service({Map<String, String> overrides = const {}}) =>
      DesktopApps(
        environment: {
          'HOME': root.path,
          'XDG_DATA_HOME': '${root.path}/user',
          'XDG_DATA_DIRS': '${root.path}/system',
          'XDG_CURRENT_DESKTOP': 'clarion',
          'PATH': bin.path,
          ...overrides,
        },
      );

  Future<File> executable(String name) async {
    final file = File('${bin.path}/$name');
    await file.writeAsString('#!/bin/sh\nexit 99\n');
    final result = await Process.run('/usr/bin/chmod', ['700', file.path]);
    expect(result.exitCode, 0);
    return file;
  }

  Future<File> entry(
    String id, {
    String? exec,
    String name = '',
    String extra = '',
    bool systemEntry = false,
  }) async {
    final directory = systemEntry ? system : user;
    final file = File('${directory.path}/$id.desktop');
    await file.parent.create(recursive: true);
    await file.writeAsString(
      '[Desktop Entry]\nType=Application\n'
      'Name=${name.isEmpty ? id : name}\n'
      '${exec == null ? '' : 'Exec=$exec\n'}$extra',
    );
    return file;
  }
}

void main() {
  late _CatalogFixture fixture;
  setUp(() async => fixture = await _CatalogFixture.create());
  tearDown(() => fixture.root.delete(recursive: true));

  test(
    'empty locale overrides preserve localized names and search text',
    () async {
      await fixture.entry(
        'localized',
        name: 'Browser',
        extra:
            'DBusActivatable=true\n'
            'Name[zh_CN]=浏览器\nName[fr_FR]=Navigateur\n'
            'Comment=Browse the web\nComment[zh_CN]=浏览网页\n'
            'Keywords=web;\nKeywords[zh_CN]=网页;上网;\n',
      );
      for (final locale in [
        {'LC_ALL': '', 'LC_MESSAGES': '', 'LANG': 'zh_CN.UTF-8'},
        {'LC_ALL': '', 'LC_MESSAGES': 'zh_CN.UTF-8', 'LANG': 'fr_FR.UTF-8'},
        {'LC_ALL': 'zh_CN.UTF-8', 'LC_MESSAGES': 'fr_FR.UTF-8', 'LANG': 'C'},
      ]) {
        final app = (await fixture.service(overrides: locale).load()).single;
        expect(app.name, '浏览器');
        expect(app.description, '浏览网页');
        expect(app.keywords, ['网页', '上网']);
      }
      final fallback =
          (await fixture
                  .service(
                    overrides: {
                      'LC_ALL': 'C',
                      'LC_MESSAGES': 'zh_CN.UTF-8',
                      'LANG': 'zh_CN.UTF-8',
                    },
                  )
                  .load())
              .single;
      expect(fallback.name, 'Browser');
    },
  );

  test(
    'missing launchers are excluded while installed VM, Wine and native apps survive',
    () async {
      final vm = await fixture.executable('winpodx');
      final wine = await fixture.executable('wine');
      final native = await fixture.executable('native');
      await fixture.entry(
        'old-vm',
        exec: '${fixture.bin.path}/removed-vm app run old',
        name: '先前的 Windows 虚拟机应用',
      );
      await fixture.entry(
        'installed-vm',
        exec: '${vm.path} app run calculator %u',
        name: 'Windows Calculator',
      );
      await fixture.entry(
        'wine-app',
        exec: '${wine.path} "C:\\Program Files\\Office.exe" %f',
        name: 'Microsoft Office',
      );
      await fixture.entry(
        'native-edge',
        exec: native.path,
        name: 'Microsoft Edge',
      );
      await fixture.entry(
        'native-windows-title',
        exec: native.path,
        name: 'Windows 编辑器',
        extra: 'Keywords=Windows;\n',
      );
      expect(
        (await fixture.service().load()).map((app) => app.id),
        containsAll([
          'installed-vm.desktop',
          'wine-app.desktop',
          'native-edge.desktop',
          'native-windows-title.desktop',
        ]),
      );
      expect(
        (await fixture.service().load()).any(
          (app) => app.id == 'old-vm.desktop',
        ),
        isFalse,
      );
    },
  );

  test(
    'PATH and common env assignments verify the real program without executing it',
    () async {
      await fixture.executable('env');
      await fixture.executable('wine');
      await fixture.entry(
        'wine',
        exec:
            'env "WINEPREFIX=${fixture.root.path}/prefix" wine "C:\\App.exe" %f',
      );
      await fixture.entry(
        'missing-wine',
        exec: 'env WINEPREFIX=/preserved missing-wine %f',
      );
      await fixture.entry('missing-bare', exec: 'removed-native --open %U');
      await fixture.entry(
        'env-path',
        exec: 'env "PATH=${fixture.bin.path}" -- wine %f',
      );
      final apps = await fixture.service().load();
      expect(apps.map((app) => app.id), ['env-path.desktop', 'wine.desktop']);
      expect(await Directory('${fixture.root.path}/prefix').exists(), isFalse);
    },
  );

  test(
    'quoted executable paths and metacharacter arguments stay literal',
    () async {
      final spaced = await fixture.executable('a program');
      final quote = await fixture.executable('a"program');
      final marker = '${fixture.root.path}/unexpected';
      await fixture.entry(
        'space',
        exec: '"${spaced.path}" "\$(touch $marker)" %U',
      );
      final quotedPath = quote.path.replaceAll('"', '\\"');
      await fixture.entry('quote', exec: '"$quotedPath" --option');
      await fixture.entry(
        'missing-space',
        exec: '"${fixture.bin.path}/missing program" --option',
      );
      expect((await fixture.service().load()).map((app) => app.id), [
        'quote.desktop',
        'space.desktop',
      ]);
      expect(await File(marker).exists(), isFalse);
    },
  );

  test(
    'activation entries do not require Exec and TryExec still masks unavailable apps',
    () async {
      final program = await fixture.executable('available');
      await fixture.entry('dbus', extra: 'DBusActivatable=true\n');
      await fixture.entry(
        'dbus-optional-exec',
        exec: '${fixture.bin.path}/old-fallback',
        extra: 'DBusActivatable=true\n',
      );
      await fixture.entry('missing-exec');
      await fixture.entry('empty-exec', exec: '');
      await fixture.entry(
        'tryexec-missing',
        exec: program.path,
        extra: 'TryExec=${fixture.bin.path}/removed\n',
      );
      await fixture.entry(
        'tryexec-valid',
        exec: program.path,
        extra: 'TryExec=available\n',
      );
      expect((await fixture.service().load()).map((app) => app.id), [
        'dbus.desktop',
        'dbus-optional-exec.desktop',
        'tryexec-valid.desktop',
      ]);
    },
  );

  test(
    'files without execute bits and directories never count as executable launchers',
    () async {
      final plain = File('${fixture.bin.path}/document');
      await plain.writeAsString('not executable');
      final folder = Directory('${fixture.bin.path}/folder');
      await folder.create();
      final available = await fixture.executable('available');
      await fixture.entry('plain', exec: plain.path);
      await fixture.entry('folder', exec: folder.path);
      await fixture.entry(
        'bad-tryexec',
        exec: available.path,
        extra: 'TryExec=${plain.path}\n',
      );
      await fixture.entry('good', exec: available.path);
      expect((await fixture.service().load()).map((app) => app.id), [
        'good.desktop',
      ]);
    },
  );

  test(
    'Hidden and unavailable user overrides mask system entries with the same desktop ID',
    () async {
      final available = await fixture.executable('available');
      for (final id in ['hidden', 'nodisplay', 'removed']) {
        await fixture.entry(id, exec: available.path, systemEntry: true);
      }
      await fixture.entry('hidden', extra: 'Hidden=true\n');
      await fixture.entry('nodisplay', extra: 'NoDisplay=true\n');
      await fixture.entry('removed', exec: '${fixture.bin.path}/obsolete');
      await fixture.entry('visible', exec: available.path, systemEntry: true);
      expect((await fixture.service().load()).map((app) => app.id), [
        'visible.desktop',
      ]);
    },
  );

  test(
    'reload sees installation, executable removal, desktop removal and symlink exports',
    () async {
      final service = fixture.service();
      final program = await fixture.executable('application');
      final entry = await fixture.entry('app', exec: program.path);
      expect((await service.load()).map((app) => app.id), ['app.desktop']);
      await program.delete();
      expect(await service.load(), isEmpty);
      await fixture.executable('application');
      expect((await service.load()).map((app) => app.id), ['app.desktop']);
      await Link('${fixture.user.path}/export.desktop').create(entry.path);
      expect(
        (await service.load()).map((app) => app.id),
        unorderedEquals(['app.desktop', 'export.desktop']),
      );
      await entry.delete();
      expect(await service.load(), isEmpty);
    },
  );

  test(
    'interpreter payloads and complex env options remain owned by GIO',
    () async {
      final shell = await fixture.executable('sh');
      await fixture.executable('env');
      await fixture.entry('shell', exec: '${shell.path} -c "run-something"');
      await fixture.entry(
        'env-split',
        exec: 'env -S "program --different grammar"',
      );
      await fixture.entry(
        'relative',
        exec: './program --from-working-directory',
      );
      expect((await fixture.service().load()).map((app) => app.id), [
        'env-split.desktop',
        'relative.desktop',
        'shell.desktop',
      ]);
    },
  );

  test('XDG roots are absolute, ordered and deduplicated', () {
    final service = fixture.service(
      overrides: {
        'XDG_DATA_HOME': 'relative',
        'XDG_DATA_DIRS':
            '${fixture.root.path}/system:relative:${fixture.root.path}/system',
      },
    );
    expect(service.dataRoots, [
      '${fixture.root.path}/.local/share',
      '${fixture.root.path}/system',
    ]);
  });

  test(
    'relative or empty PATH entries do not prove a bare launcher is missing',
    () async {
      await fixture.entry('working-directory-tool', exec: 'relative-program');
      for (final path in [
        '${fixture.bin.path}:relative',
        '${fixture.bin.path}:',
      ]) {
        expect(
          (await fixture.service(overrides: {'PATH': path}).load()).map(
            (app) => app.id,
          ),
          ['working-directory-tool.desktop'],
        );
      }
      expect(await fixture.service().load(), isEmpty);
    },
  );
}
