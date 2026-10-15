// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:realand_shell/desktop/desktop_entry.dart';
import 'package:realand_shell/desktop/desktop_file_details.dart';
import 'package:realand_shell/desktop/desktop_service.dart';

class _Files {
  _Files(this.root);
  final Directory root;
  final calls = <String>[];
  Future<void> Function()? beforeStat;
  late final service = DesktopService(
    root: root.path,
    command: (executable, arguments) async {
      expect(executable, 'stat');
      expect(arguments.last, startsWith('${root.path}/'));
      calls.add(executable);
      await beforeStat?.call();
      final result = await Process.run(executable, arguments);
      if (result.exitCode != 0) throw FileSystemException('${result.stderr}');
      return result.stdout as String;
    },
  );
  DesktopEntry entry(String name) =>
      DesktopEntry(fullPath: '${root.path}/$name', name: name);
  Future<File> write(String name, String text) =>
      File('${root.path}/$name').writeAsString(text);
}

Future<_Files> _files() async {
  final files = _Files(
    await Directory.systemTemp.createTemp('realand-inspector-'),
  );
  addTearDown(() async {
    files.service.dispose();
    await files.root.delete(recursive: true);
  });
  return files;
}

void main() {
  test(
    'inspection reads real size mode and mtime without treating ctime as creation',
    () async {
      final files = await _files();
      final file = await files.write('真实 文件.txt', '你好\n');
      final time = DateTime.utc(2024, 3, 2, 1, 2, 3);
      await file.setLastModified(time);
      expect(
        (await Process.run('chmod', ['640', '--', file.path])).exitCode,
        0,
      );
      final details = await files.service.inspectEntry(
        files.entry('真实 文件.txt'),
      );
      expect(details.name, '真实 文件.txt');
      expect(details.path, file.path);
      expect(details.kind, DesktopFileKind.file);
      expect(details.sizeBytes, utf8.encode('你好\n').length);
      expect(details.modified.toUtc(), time);
      expect(details.permissions, 'rw-r----- (0640)');
      expect(details.linkTarget, isNull);
      expect(await file.readAsString(), '你好\n');
      expect(files.service.busy, isFalse);
    },
  );

  test(
    'link inspection uses link metadata and supports relative dangling targets',
    () async {
      final files = await _files();
      final target = await files.write(
        'target.txt',
        'target content is much larger than link text',
      );
      await Link('${files.root.path}/link').create('target.txt');
      await Link('${files.root.path}/broken').create('不存在.txt');
      final link = await files.service.inspectEntry(files.entry('link'));
      expect(link.kind, DesktopFileKind.symbolicLink);
      expect(link.sizeBytes, utf8.encode('target.txt').length);
      expect(link.sizeBytes, isNot((await target.stat()).size));
      expect(link.linkTarget, 'target.txt');
      final broken = await files.service.inspectEntry(files.entry('broken'));
      expect(broken.kind, DesktopFileKind.symbolicLink);
      expect(broken.linkTarget, '不存在.txt');
      final preview = await files.service.previewEntry(files.entry('link'));
      expect(preview.kind, DesktopPreviewKind.unsupported);
      expect(preview.message, contains('不自动跟随'));
    },
  );

  test(
    'directory and FIFO are truthful unsupported previews without reading them',
    () async {
      final files = await _files();
      await Directory('${files.root.path}/folder').create();
      final fifo = '${files.root.path}/pipe';
      expect((await Process.run('mkfifo', ['--', fifo])).exitCode, 0);
      final folder = await files.service.previewEntry(files.entry('folder'));
      final pipe = await files.service
          .previewEntry(files.entry('pipe'))
          .timeout(const Duration(seconds: 2));
      expect(folder.details.kind, DesktopFileKind.directory);
      expect(pipe.details.kind, DesktopFileKind.fifo);
      expect(folder.kind, DesktopPreviewKind.unsupported);
      expect(pipe.kind, DesktopPreviewKind.unsupported);
    },
  );

  test(
    'UTF-8 text is bounded and a partial final multibyte character is omitted',
    () async {
      final files = await _files();
      final prefix = 'a' * (DesktopService.previewTextByteLimit - 1);
      await files.write('long.txt', '$prefix文末');
      final preview = await files.service.previewEntry(files.entry('long.txt'));
      expect(preview.kind, DesktopPreviewKind.text);
      expect(preview.text, prefix);
      expect(preview.truncated, isTrue);
      expect(preview.message, contains('256 KiB'));
      await files.write('unicode.txt', '第二行\n日本語 🚀');
      final unicode = await files.service.previewEntry(
        files.entry('unicode.txt'),
      );
      expect(unicode.text, '第二行\n日本語 🚀');
      expect(unicode.truncated, isFalse);
      await File(
        '${files.root.path}/bad-boundary.txt',
      ).writeAsBytes([...utf8.encode(prefix), 0xff, 0x61, 0x62, 0x63]);
      expect(
        (await files.service.previewEntry(
          files.entry('bad-boundary.txt'),
        )).kind,
        DesktopPreviewKind.unsupported,
      );
    },
  );

  test(
    'desktop launchers are plain preview text and are never executed',
    () async {
      final files = await _files();
      final sentinel = '${files.root.path}/MUST_NOT_EXIST';
      final text =
          '[Desktop Entry]\nType=Application\nName=Example\nExec=touch $sentinel\n';
      await files.write('example.desktop', text);
      final preview = await files.service.previewEntry(
        files.entry('example.desktop'),
      );
      expect(preview.kind, DesktopPreviewKind.text);
      expect(preview.text, text);
      expect(await File(sentinel).exists(), isFalse);
      expect(files.calls.every((call) => call == 'stat'), isTrue);
    },
  );

  test('image bytes are bounded and binary text is not fabricated', () async {
    final files = await _files();
    final png = base64Decode(
      'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=',
    );
    await File('${files.root.path}/small.png').writeAsBytes(png);
    final small = await files.service.previewEntry(files.entry('small.png'));
    expect(small.kind, DesktopPreviewKind.image);
    expect(small.imageBytes, png);
    final large = await File(
      '${files.root.path}/large.png',
    ).open(mode: FileMode.write);
    await large.writeFrom(png);
    await large.truncate(DesktopService.previewImageByteLimit + 1);
    await large.close();
    final oversized = await files.service.previewEntry(
      files.entry('large.png'),
    );
    expect(oversized.kind, DesktopPreviewKind.unsupported);
    expect(oversized.message, contains('8 MiB'));
    await File('${files.root.path}/binary').writeAsBytes([0, 255, 1, 2]);
    expect(
      (await files.service.previewEntry(files.entry('binary'))).kind,
      DesktopPreviewKind.unsupported,
    );
    await File(
      '${files.root.path}/unknown-encoding',
    ).writeAsBytes([0xff, 0xfe, 0x41, 0x42]);
    expect(
      (await files.service.previewEntry(files.entry('unknown-encoding'))).kind,
      DesktopPreviewKind.unsupported,
    );
  });

  test(
    'file changed during preview is rejected instead of publishing mixed metadata',
    () async {
      final files = await _files();
      final file = await files.write('changed.txt', 'old');
      var reads = 0;
      files.beforeStat = () async {
        if (++reads == 2) {
          await file.writeAsString('replacement with another length');
        }
      };
      await expectLater(
        files.service.previewEntry(files.entry('changed.txt')),
        throwsA(isA<StateError>()),
      );
      expect(files.service.error, isNull);
    },
  );

  test(
    'outside entries and cancelled late reads are rejected without mutation',
    () async {
      final files = await _files();
      await files.write('one.txt', 'one');
      await expectLater(
        files.service.inspectEntry(
          const DesktopEntry(fullPath: '/etc/passwd', name: 'passwd'),
        ),
        throwsArgumentError,
      );
      expect(files.calls, isEmpty);
      final started = Completer<void>();
      final finish = Completer<void>();
      files.beforeStat = () async {
        started.complete();
        await finish.future;
      };
      final cancellation = DesktopInspectionCancellation();
      final pending = files.service.inspectEntry(
        files.entry('one.txt'),
        cancellation: cancellation,
      );
      final rejected = expectLater(
        pending,
        throwsA(isA<DesktopInspectionCancelled>()),
      );
      await started.future;
      cancellation.cancel();
      finish.complete();
      await rejected;
      expect(await File('${files.root.path}/one.txt').readAsString(), 'one');
    },
  );

  test(
    'redirecting the desktop root while metadata is pending rejects the result',
    () async {
      final files = await _files();
      await files.write('one.txt', 'original');
      await files.service.start();
      final moved = '${files.root.path}-original';
      addTearDown(() async {
        if (await Directory(moved).exists()) {
          await Directory(moved).delete(recursive: true);
        }
      });
      files.beforeStat = () async {
        await files.root.rename(moved);
        await Link(files.root.path).create(moved);
      };
      await expectLater(
        files.service.inspectEntry(files.entry('one.txt')),
        throwsA(isA<StateError>()),
      );
      expect(await File('$moved/one.txt').readAsString(), 'original');
      await Link(files.root.path).delete();
      await files.root.create();
    },
  );
}
