// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/services/wallpaper_thumbnail_service.dart';

final _png = base64Decode(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=',
);

void main() {
  late _Fixture fixture;
  setUp(() async => fixture = await _Fixture.create());
  tearDown(() => fixture.dispose());

  test(
    'persistent warm hits never start a decoder and originals remain unchanged',
    () async {
      final image = await fixture.image('literal %s [page=2] 空格.png');
      final original = await File(image.path).readAsBytes();
      final originalStat = await File(image.path).stat();
      final first = await fixture.service.resolve(image);
      expect(first.file, isNotNull);
      expect(fixture.processes.length, 1);
      expect(fixture.sources.single, image.path);
      expect(fixture.arguments.single, contains('--vips-concurrency=1'));
      expect(fixture.arguments.single, contains('--size=320x>'));
      expect(fixture.arguments.single.last, 'source.png');
      expect(fixture.arguments.single, isNot(contains(image.path)));
      expect(await first.file!.readAsBytes(), _png);
      final secondService = fixture.makeService();
      final warm = await secondService.resolve(image);
      expect(warm.file!.path, first.file!.path);
      expect(fixture.processes.length, 1);
      expect(await File(image.path).readAsBytes(), original);
      expect((await File(image.path).stat()).modified, originalStat.modified);
      expect(await fixture.stagingDirectories(), isEmpty);
    },
  );

  test(
    'same-size overwrites invalidate by actual mtime, even with an old catalogue entry',
    () async {
      final image = await fixture.image('rewrite.png');
      final first = await fixture.service.resolve(image);
      final file = File(image.path);
      final previous = await file.stat();
      await file.writeAsBytes(_png);
      await file.setLastModified(
        previous.modified.add(const Duration(seconds: 3)),
      );
      final second = await fixture.service.resolve(image);
      expect(second.file!.path, isNot(first.file!.path));
      expect(fixture.processes.length, 2);
    },
  );

  test('warm previews bypass two occupied cold decoder slots', () async {
    final warmImage = await fixture.image('already-cached.png');
    final warm = await fixture.service.resolve(warmImage);
    fixture.autoExit = false;
    final first = fixture.service.resolve(
      await fixture.image('slow-first.png'),
    );
    final second = fixture.service.resolve(
      await fixture.image('slow-second.png'),
    );
    await fixture.waitForProcesses(3);
    final again = await fixture.service
        .resolve(warmImage)
        .timeout(const Duration(seconds: 1));
    expect(again.file!.path, warm.file!.path);
    expect(fixture.processes.length, 3);
    expect(fixture.processes.skip(1).every((process) => !process.exited), true);
    for (final process in fixture.processes) {
      process.exit(0);
    }
    await Future.wait([first, second]);
  });

  test(
    'full metadata identity is checked rather than trusting the cache filename',
    () async {
      final image = await fixture.image('identity.png');
      final first = await fixture.service.resolve(image);
      final metadata = File('${first.file!.parent.path}/metadata.json');
      final data =
          jsonDecode(await metadata.readAsString()) as Map<String, dynamic>;
      data['identity'] = 'some other source';
      await metadata.writeAsString(jsonEncode(data));
      await fixture.service.resolve(image);
      expect(fixture.processes.length, 2);
    },
  );

  test(
    'at most two decoders run and identical requests share one job',
    () async {
      fixture.autoExit = false;
      final a = await fixture.image('a.png');
      final b = await fixture.image('b.png');
      final c = await fixture.image('c.png');
      final futures = [
        fixture.service.resolve(a),
        fixture.service.resolve(a),
        fixture.service.resolve(b),
        fixture.service.resolve(c),
      ];
      await fixture.waitForProcesses(2);
      await Future<void>.delayed(const Duration(milliseconds: 20));
      expect(fixture.processes.length, 2);
      fixture.processes.first.exit(0);
      await fixture.waitForProcesses(3);
      expect(fixture.maximumActive, 2);
      for (final process in fixture.processes) {
        process.exit(0);
      }
      final results = await Future.wait(futures);
      expect(results[0].file!.path, results[1].file!.path);
      expect(fixture.processes.length, 3);
      expect(fixture.maximumActive, 2);
    },
  );

  test('scrolled-away queued tiles do not launch a decoder', () async {
    fixture.autoExit = false;
    final first = fixture.service.resolve(await fixture.image('first.png'));
    final second = fixture.service.resolve(await fixture.image('second.png'));
    await fixture.waitForProcesses(2);
    var wanted = true;
    final obsolete = fixture.service.resolve(
      await fixture.image('offscreen.png'),
      stillWanted: () => wanted,
    );
    await Future<void>.delayed(const Duration(milliseconds: 20));
    wanted = false;
    fixture.processes.first.exit(0);
    expect((await obsolete).file, isNull);
    fixture.processes.last.exit(0);
    await Future.wait([first, second]);
    expect(fixture.processes.length, 2);
  });

  test(
    'a source replaced during generation is not published under its old identity',
    () async {
      fixture.autoExit = false;
      final image = await fixture.image('racing.png');
      final pending = fixture.service.resolve(image);
      await fixture.waitForProcesses(1);
      final file = File(image.path);
      final stat = await file.stat();
      await file.setLastModified(stat.modified.add(const Duration(seconds: 3)));
      fixture.processes.single.exit(0);
      expect((await pending).file, isNull);
      expect(await fixture.cacheEntries(), isEmpty);
      fixture.autoExit = true;
      expect((await fixture.service.resolve(image)).file, isNotNull);
    },
  );

  test(
    'decoder failure is negatively cached, and original failure waits for a new revision',
    () async {
      fixture.exitCode = 1;
      final image = await fixture.image('broken.png');
      final failed = await fixture.service.resolve(image);
      expect(failed.file, isNull);
      expect(
        failed.invalid,
        false,
      ); // Flutter gets one chance with the original.
      final secondService = fixture.makeService();
      final again = await secondService.resolve(image);
      expect(again.file, isNull);
      expect(fixture.processes.length, 1);
      await secondService.reportFailure(again, original: true);
      final invalid = await fixture.service.resolve(image);
      expect(invalid.invalid, true);
      expect(fixture.processes.length, 1);
      fixture.exitCode = 0;
      final file = File(image.path);
      await file.setLastModified(
        (await file.stat()).modified.add(const Duration(seconds: 3)),
      );
      expect((await fixture.service.resolve(image)).file, isNotNull);
      expect(fixture.processes.length, 2);
    },
  );

  test(
    'corrupt cache rebuilds once and failed cached decoding does not condemn the original',
    () async {
      final image = await fixture.image('valid.png');
      final cached = await fixture.service.resolve(image);
      await cached.file!.writeAsString('bad PNG');
      final rebuilt = await fixture.service.resolve(image);
      expect(await rebuilt.file!.readAsBytes(), _png);
      expect(fixture.processes.length, 2);
      await fixture.service.reportFailure(rebuilt, original: false);
      final fallback = await fixture.service.resolve(image);
      expect(fallback.file, isNull);
      expect(fallback.invalid, false);
      expect(fixture.processes.length, 2);
    },
  );

  test(
    'absent decoder returns original fallback without creating a process',
    () async {
      final service = fixture.makeService(
        executable: '${fixture.root.path}/missing',
      );
      final result = await service.resolve(await fixture.image('ordinary.png'));
      expect(result.file, isNull);
      expect(result.invalid, false);
      expect(fixture.processes, isEmpty);
    },
  );

  test(
    'concurrent cache failure cannot overwrite an original failure marker',
    () async {
      final image = await fixture.image('failure-order.png');
      final cached = await fixture.service.resolve(image);
      await Future.wait([
        fixture.service.reportFailure(cached, original: true),
        fixture.service.reportFailure(cached, original: false),
      ]);
      expect((await fixture.service.resolve(image)).invalid, true);
      expect(fixture.processes.length, 1);
    },
  );

  test(
    'atomic publication exposes no partial entry while a decoder is running',
    () async {
      fixture.autoExit = false;
      final pending = fixture.service.resolve(
        await fixture.image('atomic.png'),
      );
      await fixture.waitForProcesses(1);
      expect(await fixture.cacheEntries(), isEmpty);
      expect(await fixture.stagingDirectories(), hasLength(1));
      fixture.processes.single.exit(0);
      expect((await pending).file, isNotNull);
      expect(await fixture.cacheEntries(), hasLength(1));
      expect(await fixture.stagingDirectories(), isEmpty);
    },
  );

  test(
    'cache maintenance enforces entry and byte budgets, including failure entries',
    () async {
      final small = fixture.makeService(
        maximumEntries: 2,
        maximumBytes: 100000,
      );
      for (var index = 0; index < 4; index++) {
        await small.resolve(await fixture.image('cache-$index.png'));
      }
      await small.trim();
      expect(await fixture.cacheEntries(), hasLength(2));
      final tiny = fixture.makeService(maximumEntries: 100, maximumBytes: 1);
      await tiny.trim();
      expect(await fixture.cacheEntries(), isEmpty);
      fixture.exitCode = 1;
      for (var index = 0; index < 3; index++) {
        await small.resolve(await fixture.image('failure-$index.png'));
      }
      await small.trim();
      expect(await fixture.cacheEntries(), hasLength(2));
    },
  );

  test(
    'dispose cancels owned workers and queued work without signalling an unrelated process',
    () async {
      fixture.autoExit = false;
      final unrelated = _FakeProcess(99999);
      final requests = [
        fixture.service.resolve(await fixture.image('one.png')),
        fixture.service.resolve(await fixture.image('two.png')),
        fixture.service.resolve(await fixture.image('three.png')),
      ];
      await fixture.waitForProcesses(2);
      fixture.service.dispose();
      final results = await Future.wait(
        requests,
      ).timeout(const Duration(seconds: 2));
      expect(results.every((result) => result.file == null), true);
      expect(fixture.processes.length, 2);
      expect(
        fixture.processes.every(
          (process) => process.signals.contains(ProcessSignal.sigterm),
        ),
        true,
      );
      expect(unrelated.signals, isEmpty);
      expect(await fixture.stagingDirectories(), isEmpty);
      unrelated.exit(0);
    },
  );

  test(
    'a late process-start result after dispose is stopped and never published',
    () async {
      fixture.autoExit = false;
      fixture.startGate = Completer<void>();
      final pending = fixture.service.resolve(await fixture.image('late.png'));
      await fixture.waitForProcesses(1);
      fixture.service.dispose();
      fixture.startGate!.complete();
      expect((await pending).file, isNull);
      expect(fixture.processes.single.signals, [ProcessSignal.sigterm]);
      expect(await fixture.cacheEntries(), isEmpty);
    },
  );
}

class _Fixture {
  _Fixture(this.root) {
    service = makeService();
  }
  final Directory root;
  Directory get cache => Directory('${root.path}/cache');
  late WallpaperThumbnailService service;
  final services = <WallpaperThumbnailService>[];
  final processes = <_FakeProcess>[];
  final arguments = <List<String>>[];
  final sources = <String>[];
  final started = StreamController<int>.broadcast();
  bool autoExit = true;
  int exitCode = 0;
  int maximumActive = 0;
  Completer<void>? startGate;
  static Future<_Fixture> create() async => _Fixture(
    await Directory.systemTemp.createTemp('clarion-thumbnail-test-'),
  );

  WallpaperThumbnailService makeService({
    String executable = '/usr/bin/true',
    int maximumEntries = 512,
    int maximumBytes = 128 * 1024 * 1024,
  }) {
    final service = WallpaperThumbnailService(
      enabled: true,
      cacheDirectory: cache,
      executable: executable,
      maximumEntries: maximumEntries,
      maximumBytes: maximumBytes,
      processStarter: (executable, args, directory) async {
        arguments.add(List.of(args));
        sources.add(await Link('$directory/source.png').target());
        await File('$directory/thumbnail.png').writeAsBytes(_png);
        final process = _FakeProcess(70000 + processes.length);
        processes.add(process);
        final active = processes.where((process) => !process.exited).length;
        if (active > maximumActive) maximumActive = active;
        started.add(processes.length);
        await startGate?.future;
        if (autoExit) process.exit(exitCode);
        return process;
      },
    );
    services.add(service);
    return service;
  }

  Future<ShellWallpaper> image(String name) async {
    final file = await File('${root.path}/$name').writeAsBytes(_png);
    final stat = await file.stat();
    return ShellWallpaper(
      path: file.path,
      name: name,
      sizeBytes: stat.size,
      modified: stat.modified,
    );
  }

  Future<void> waitForProcesses(int count) async {
    if (processes.length < count) {
      await started.stream
          .firstWhere((value) => value >= count)
          .timeout(const Duration(seconds: 2));
    }
  }

  Future<List<FileSystemEntity>> cacheEntries() => cache
      .list()
      .where((entry) => RegExp(r'/[0-9a-f]{16}$').hasMatch(entry.path))
      .toList();
  Future<List<FileSystemEntity>> stagingDirectories() =>
      cache.list().where((entry) => entry.path.contains('/.build-')).toList();
  Future<void> dispose() async {
    for (final service in services) {
      service.dispose();
    }
    for (final process in processes) {
      process.exit(0);
    }
    await started.close();
    await root.delete(recursive: true);
  }
}

class _FakeProcess implements Process {
  _FakeProcess(this.pid);
  @override
  final int pid;
  final _exit = Completer<int>();
  final signals = <ProcessSignal>[];
  bool get exited => _exit.isCompleted;
  @override
  Stream<List<int>> get stdout => const Stream.empty();
  @override
  Stream<List<int>> get stderr => const Stream.empty();
  @override
  Future<int> get exitCode => _exit.future;
  void exit(int code) {
    if (!_exit.isCompleted) _exit.complete(code);
  }

  @override
  bool kill([ProcessSignal signal = ProcessSignal.sigterm]) {
    signals.add(signal);
    exit(0);
    return true;
  }

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}
