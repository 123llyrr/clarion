// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/preferences.dart';
import 'package:clarion_shell/services/identity.dart';

void main() {
  test('canonical socket takes precedence and empty values fall back', () {
    expect(
      ipcSocketPath({'CLARION_SOCKET': '/new', 'REALAND_SOCKET': '/old'}),
      '/new',
    );
    expect(ipcSocketPath({'REALAND_SOCKET': '/old'}), '/old');
    expect(
      ipcSocketPath({'CLARION_SOCKET': '', 'REALAND_SOCKET': '/old'}),
      '/old',
    );
    expect(ipcSocketPath({'CLARION_SOCKET': '', 'REALAND_SOCKET': ''}), isNull);
  });

  test(
    'legacy preferences and pins survive without copying the directory',
    () async {
      final root = Directory.systemTemp.createTempSync('clarion-identity-');
      addTearDown(() => root.deleteSync(recursive: true));
      final environment = {'XDG_CONFIG_HOME': root.path};
      final canonical = Directory('${root.path}/clarion-shell');
      final legacy = Directory('${root.path}/realand-shell')..createSync();
      final source = File('${legacy.path}/preferences.json')
        ..writeAsStringSync('{"pinned_app_ids":["org.example.Editor"]}');
      final selected = shellConfigurationFile(
        'preferences.json',
        environment: environment,
      )!;
      expect(selected.path, source.path);
      final preferences = await ShellPreferences.load(source: selected);
      expect(preferences.pinnedAppIds, ['org.example.Editor']);
      expect(canonical.existsSync(), isFalse);
      await preferences.save(destination: selected);
      expect(source.existsSync(), isTrue);
      expect(canonical.existsSync(), isFalse);
      canonical.createSync();
      expect(shellConfigDirectory(environment), canonical.path);
      expect(source.readAsStringSync(), contains('org.example.Editor'));
    },
  );

  test(
    'new configuration path is canonical and missing HOME does not guess',
    () {
      final root = Directory.systemTemp.createTempSync('clarion-identity-');
      addTearDown(() => root.deleteSync(recursive: true));
      expect(
        shellConfigurationFile(
          'preferences.json',
          environment: {'XDG_CONFIG_HOME': root.path},
        )!.path,
        '${root.path}/clarion-shell/preferences.json',
      );
      expect(
        shellConfigurationFile('preferences.json', environment: {}),
        isNull,
      );
      expect(
        shellConfigurationFile(
          'preferences.json',
          environment: {'HOME': 'relative'},
        ),
        isNull,
      );
    },
  );

  test('broken canonical symlink does not resurrect old preferences', () {
    final root = Directory.systemTemp.createTempSync('clarion-identity-');
    addTearDown(() => root.deleteSync(recursive: true));
    Directory('${root.path}/realand-shell').createSync();
    Link('${root.path}/clarion-shell').createSync('${root.path}/missing');
    expect(
      shellConfigDirectory({'XDG_CONFIG_HOME': root.path}),
      '${root.path}/clarion-shell',
    );
  });
}
