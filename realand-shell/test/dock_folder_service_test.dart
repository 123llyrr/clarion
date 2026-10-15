// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:realand_shell/mac_shell_preferences.dart';
import 'package:realand_shell/services/application_launcher.dart';
import 'package:realand_shell/services/dock_folder_service.dart';

Future<void> _wait(bool Function() ready) async {
  final deadline = DateTime.now().add(const Duration(seconds: 6));
  while (!ready()) {
    if (DateTime.now().isAfter(deadline)) {
      fail('Directory watcher did not settle');
    }
    await Future<void>.delayed(const Duration(milliseconds: 20));
  }
}

void main() {
  late Directory temporary;
  late Directory downloads;
  late List<List<Object>> launches;
  late ApplicationLauncher launcher;
  final services = <DockFolderService>[];

  setUp(() async {
    temporary = await Directory.systemTemp.createTemp(
      'realand-downloads-test-',
    );
    downloads = await Directory('${temporary.path}/Downloads').create();
    launches = [];
    launcher = ApplicationLauncher(
      environment: {'HOME': temporary.path},
      scopeProbe: () async => null,
      command: (program, arguments) async {
        launches.add([program, List.of(arguments)]);
        return '';
      },
    );
  });

  tearDown(() async {
    for (final service in services) {
      service.dispose();
    }
    services.clear();
    await temporary.delete(recursive: true);
  });

  DockFolderService service({String? directory, int maximumEntries = 256}) {
    final result = DockFolderService(
      directory: directory ?? downloads.path,
      environment: {'HOME': temporary.path},
      applicationLauncher: launcher,
      maximumEntries: maximumEntries,
      debounceDelay: const Duration(milliseconds: 20),
    );
    services.add(result);
    return result;
  }

  test(
    'resolves localized XDG Downloads once and only opens literal argv',
    () async {
      final localized = await Directory('${temporary.path}/下载 空格').create();
      const dangerous = "\$(touch should-not-exist); ' & #.txt";
      await File('${localized.path}/$dangerous').writeAsString('ordinary data');
      var resolutions = 0;
      final folders = DockFolderService(
        environment: {'HOME': temporary.path},
        command: (program, args) async {
          expect(program, 'xdg-user-dir');
          expect(args, ['DOWNLOAD']);
          resolutions++;
          return '${localized.path}\n';
        },
        applicationLauncher: launcher,
      );
      services.add(folders);
      await Future.wait([folders.start(), folders.start()]);
      expect(resolutions, 1);
      expect(folders.path, localized.path);
      expect(launches, isEmpty);
      await folders.open(folders.entries.single);
      await folders.openFolder();
      expect(launches, [
        [
          'gio',
          ['open', '--', '${localized.path}/$dangerous'],
        ],
        [
          'gio',
          ['open', '--', localized.path],
        ],
      ]);
      expect(
        await File('${temporary.path}/should-not-exist').exists(),
        isFalse,
      );
    },
  );

  test(
    'caps filesystem enumeration before collecting a large directory',
    () async {
      for (var i = 0; i < 8; i++) {
        await File('${downloads.path}/$i.txt').writeAsString('$i');
      }
      final folders = service(maximumEntries: 3);
      await folders.start();
      expect(folders.entries.length, 3);
      expect(folders.overflow, isTrue);
      expect(folders.error, isNull);
    },
  );

  test(
    'sorts real modification dates and never substitutes POSIX ctime',
    () async {
      final a = await File('${downloads.path}/a.txt').writeAsString('old');
      final b = await File('${downloads.path}/b.png').writeAsString('new');
      await a.setLastModified(DateTime(2020, 1));
      await b.setLastModified(DateTime(2024, 1));
      final folders = service();
      await folders.start();
      expect(
        folders.sortedEntries(MacDownloadsSort.dateModified).map((e) => e.name),
        ['b.png', 'a.txt'],
      );
      expect(folders.sortedEntries(MacDownloadsSort.name).map((e) => e.name), [
        'a.txt',
        'b.png',
      ]);
      expect(folders.entries.map((e) => e.kind), containsAll(['图像', '文件']));
      expect(
        folders.entries.every((e) => e.created == null && e.added == null),
        isTrue,
      );
      expect(folders.supportsSort(MacDownloadsSort.dateCreated), isFalse);
      expect(folders.supportsSort(MacDownloadsSort.dateAdded), isFalse);
      expect(
        folders.sortedEntries(MacDownloadsSort.dateCreated).map((e) => e.name),
        ['a.txt', 'b.png'],
      );
    },
  );

  test(
    'watcher updates additions and survives directory deletion/recreation',
    () async {
      final folders = service();
      await folders.start();
      await File('${downloads.path}/arrived.txt').writeAsString('download');
      await _wait(() => folders.entries.any((e) => e.name == 'arrived.txt'));
      await downloads.delete(recursive: true);
      await _wait(() => folders.entries.isEmpty && folders.error != null);
      await downloads.create();
      await File(
        '${downloads.path}/recreated.txt',
      ).writeAsString('replacement');
      await _wait(
        () =>
            folders.entries.any((e) => e.name == 'recreated.txt') &&
            folders.error == null,
      );
      expect(launches, isEmpty);
    },
  );

  test('ignores hidden content and never follows a linked directory', () async {
    final outside = await Directory('${temporary.path}/outside').create();
    await File('${outside.path}/private.txt').writeAsString('not enumerated');
    await Link('${downloads.path}/linked').create(outside.path);
    await File('${downloads.path}/.hidden').writeAsString('hidden');
    final folders = service();
    await folders.start();
    expect(folders.entries.length, 1);
    expect(folders.entries.single.name, 'linked');
    expect(folders.entries.single.isSymlink, isTrue);
    expect(folders.entries.single.isDirectory, isFalse);
    expect(folders.entries.single.modified, isNull);
  });

  test(
    'deleted entries reject opening instead of activating a stale tile',
    () async {
      final file = await File(
        '${downloads.path}/removed.txt',
      ).writeAsString('temporary');
      final folders = service();
      await folders.start();
      final entry = folders.entries.single;
      await file.delete();
      await expectLater(folders.open(entry), throwsStateError);
      expect(launches, isEmpty);
    },
  );

  test(
    'missing xdg tool falls back without creating the Downloads directory',
    () async {
      await downloads.delete();
      final folders = DockFolderService(
        environment: {'HOME': temporary.path},
        command: (_, _) async => throw StateError('tool unavailable'),
        applicationLauncher: launcher,
      );
      services.add(folders);
      await folders.start();
      expect(folders.path, downloads.path);
      expect(folders.error, isNotNull);
      expect(await downloads.exists(), isFalse);
      await downloads.create();
      await File('${downloads.path}/later.txt').writeAsString('ready');
      await _wait(() => folders.entries.length == 1);
    },
  );

  test(
    'dispose prevents pending XDG reply from installing watchers or notifying',
    () async {
      final reply = Completer<String>();
      var notifications = 0;
      final folders = DockFolderService(
        environment: {'HOME': temporary.path},
        command: (_, _) => reply.future,
        applicationLauncher: launcher,
      );
      folders.addListener(() => notifications++);
      final start = folders.start();
      folders.dispose();
      reply.complete(downloads.path);
      await start;
      expect(notifications, 1);
      expect(folders.path, isNull);
      expect(launches, isEmpty);
    },
  );

  test(
    'invalid or disabled roots never become a later refresh target',
    () async {
      final folders = service(directory: '/');
      await folders.start();
      await folders.refresh();
      expect(folders.path, isNull);
      expect(folders.entries, isEmpty);
      expect(folders.error, contains('根目录'));
      final disabled = DockFolderService(
        environment: {'HOME': temporary.path},
        command: (_, _) async => temporary.path,
        applicationLauncher: launcher,
      );
      services.add(disabled);
      await disabled.start();
      expect(disabled.path, isNull);
      expect(disabled.error, contains('停用'));
    },
  );
}
