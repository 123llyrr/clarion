// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'package:flutter_test/flutter_test.dart';
import 'package:realand_shell/models.dart';
import 'package:realand_shell/services/wallpaper_service.dart';

void main() {
  late _Fixture fixture;
  setUp(() async {
    fixture = await _Fixture.create();
  });
  tearDown(() async {
    await fixture.close();
  });

  test(
    'catalogue is static, sorted, nonrecursive and ignores empty files',
    () async {
      await fixture.image('中文 空格.JPG');
      await fixture.image('a.webp');
      await fixture.image('video.mp4');
      await File('${fixture.root.path}/empty.png').create();
      await Directory('${fixture.root.path}/folder.png').create();
      await Directory('${fixture.root.path}/nested').create();
      await File(
        '${fixture.root.path}/nested/hidden.jpg',
      ).writeAsString('image');
      final images = await fixture.service.scan();
      expect(images.map((image) => image.name), ['a.webp', '中文 空格.JPG']);
      expect(images.every((image) => image.path.startsWith('/')), true);
      expect(images.every((image) => image.sizeBytes > 0), true);
      await expectLater(
        WallpaperService(directoryPath: '${fixture.root.path}/missing').scan(),
        throwsA(isA<FileSystemException>()),
      );
    },
  );

  test(
    'concurrent catalogue reads share a walk but a later refresh restats files',
    () async {
      final entry = await fixture.image('replaced.png');
      final first = fixture.service.scan();
      final concurrent = fixture.service.scan();
      expect(identical(first, concurrent), true);
      final original = (await first).single;
      final file = File(entry.path);
      await file.writeAsString('other');
      await file.setLastModified(
        original.modified!.add(const Duration(seconds: 2)),
      );
      final refreshed = (await fixture.service.scan()).single;
      expect(refreshed.path, original.path);
      expect(refreshed.sizeBytes, original.sizeBytes);
      expect(refreshed.modified, isNot(original.modified));
    },
  );

  test(
    'availability is asynchronously cached outside widget getter reads',
    () async {
      final backend = _CountingBackendFile(File('/usr/bin/true'));
      final preference = fixture.prefs;
      await IOOverrides.runZoned(
        () async {
          final service = fixture.makeService(preferenceFile: preference);
          try {
            await service.changes.firstWhere((_) => service.available);
            expect(backend.asyncStats, 1);
            for (var index = 0; index < 100; index++) {
              expect(service.available, true);
            }
            expect(backend.asyncStats, 1);
            expect(backend.syncStats, 0);
          } finally {
            service.dispose();
          }
        },
        createFile: (path) {
          expect(path, '/usr/bin/true');
          return backend;
        },
      );
    },
  );

  test(
    'selecting the displayed file reuses its decoded provider until it changes',
    () async {
      final entry = await fixture.image('selected.png');
      fixture.onStart = (process) => process.ready();
      await fixture.service.apply(entry);
      final first = fixture.processes.single;
      final preferences = await fixture.prefs.readAsString();
      await fixture.service.apply(entry);
      expect(fixture.processes, [first]);
      expect(first.signals, isEmpty);
      expect(await fixture.prefs.readAsString(), preferences);
      final file = File(entry.path);
      final original = await file.stat();
      await file.writeAsString('other');
      await file.setLastModified(
        original.modified.add(const Duration(seconds: 2)),
      );
      // Pass the old catalogue entry deliberately: apply validates the actual
      // file revision, not metadata captured before an external rewrite.
      await fixture.service.apply(entry);
      expect(fixture.processes.length, 2);
      expect(first.signals, [ProcessSignal.sigterm]);
      expect(fixture.service.ownedPid, fixture.processes.last.pid);
    },
  );

  test('saved selection still starts a provider in a fresh service', () async {
    final entry = await fixture.image('saved-again.png');
    fixture.onStart = (process) => process.ready();
    await fixture.service.apply(entry);
    final restarted = fixture.makeService();
    addTearDown(restarted.dispose);
    await restarted.restore(applySaved: true);
    expect(fixture.processes.length, 2);
    expect(restarted.selectedPath, entry.path);
    expect(restarted.ownedPid, fixture.processes.last.pid);
  });

  for (final applySaved in [false, true]) {
    test(
      'late restore ($applySaved) cannot overwrite an explicit selection',
      () async {
        final saved = await fixture.image('saved-old.png');
        final chosen = await fixture.image('chosen-new.png');
        await fixture.save(saved);
        final preference = _DelayedPreferenceFile(fixture.prefs);
        final service = fixture.makeService(preferenceFile: preference);
        addTearDown(service.dispose);
        final restoring = service.restore(applySaved: applySaved);
        await preference.readStarted.future;
        fixture.onStart = (process) => process.ready();
        await service.apply(chosen);
        preference.releaseRead.complete();
        await restoring;
        expect(service.selectedPath, chosen.path);
        expect(fixture.arguments.map((args) => args[1]), [chosen.path]);
        expect(
          (jsonDecode(await fixture.prefs.readAsString()) as Map)['path'],
          chosen.path,
        );
      },
    );
  }

  test(
    'remembered selection is read without spawning and empty settings apply nothing',
    () async {
      await fixture.service.restore(applySaved: true);
      expect(fixture.processes, isEmpty);
      final image = await fixture.image('saved.png');
      await fixture.save(image);
      await fixture.service.restore();
      expect(fixture.service.selectedPath, image.path);
      expect(fixture.processes, isEmpty);
      await fixture.prefs.writeAsString('{"version":1,"path":"/missing.png"}');
      await expectLater(
        fixture.service.restore(applySaved: true),
        throwsA(isA<FileSystemException>()),
      );
      expect(fixture.service.selectedPath, image.path);
      expect(fixture.processes, isEmpty);
    },
  );

  test(
    'literal argv and atomic preferences stay separate from shell preferences',
    () async {
      final image = await fixture.image('空 格 \$literal.png');
      final prefs = File('${fixture.root.path}/config/preferences.json');
      await prefs.parent.create();
      await prefs.writeAsString('unchanged');
      fixture.onStart = (process) => process.ready();
      await fixture.service.apply(image);
      expect(fixture.arguments.single, [
        '--image',
        image.path,
        '--mode',
        'fill',
      ]);
      expect(fixture.environments.single['WAYLAND_DEBUG'], 'client');
      expect(fixture.environments.single['WAYLAND_DISPLAY'], 'fixture-wayland');
      expect(
        (jsonDecode(await fixture.prefs.readAsString()) as Map)['path'],
        image.path,
      );
      expect(await prefs.readAsString(), 'unchanged');
      expect(fixture.service.ownedPid, fixture.processes.single.pid);
      expect(
        await fixture.prefs.parent
            .list()
            .where((entry) => entry.path.contains('.wallpaper-'))
            .isEmpty,
        true,
      );
    },
  );

  test(
    'both protocol formats require a nonnull attach and every output commit',
    () async {
      fixture.outputs = 2;
      final image = await fixture.image('multi.png');
      final applied = fixture.service.apply(image);
      final process = await fixture.nextProcess();
      var completed = false;
      unawaited(
        applied.then((_) {
          completed = true;
        }),
      );
      process.line('[1] -> wl_surface#3.commit()');
      process.line('[2] -> wl_surface#3.attach(nil, 0, 0)');
      process.line('[3] -> wl_surface#3.commit()');
      await _turn();
      expect(completed, false);
      process.ready(surface: 3, separator: '#');
      await _turn();
      expect(completed, false);
      process.ready(surface: 9, separator: '@');
      await applied;
      expect(fixture.service.selectedPath, image.path);
    },
  );

  test(
    'decode errors reject a living black-buffer candidate and preserve old wallpaper',
    () async {
      final old = await fixture.image('old.png');
      final broken = await fixture.image('broken.png');
      fixture.onStart = (process) => process.ready();
      await fixture.service.apply(old);
      final previous = fixture.processes.single;
      fixture.onStart = (process) {
        process.line('[main.c:333] Failed to load background image (bad PNG)');
        process.line('[main.c:441] Failed to load image');
        process.ready();
      };
      await expectLater(fixture.service.apply(broken), throwsStateError);
      expect(previous.signals, isEmpty);
      expect(fixture.processes.last.signals, [ProcessSignal.sigterm]);
      expect(fixture.service.selectedPath, old.path);
      expect(fixture.service.ownedPid, previous.pid);
      expect(
        (jsonDecode(await fixture.prefs.readAsString()) as Map)['path'],
        old.path,
      );
    },
  );

  test(
    'early exit and bounded timeout retain the old owned provider',
    () async {
      fixture.onStart = (process) => process.ready();
      final old = await fixture.image('old.png');
      await fixture.service.apply(old);
      final previous = fixture.processes.single;
      fixture.onStart = (process) => process.exit(7);
      await expectLater(
        fixture.service.apply(await fixture.image('exit.png')),
        throwsStateError,
      );
      fixture.onStart = (_) {};
      await expectLater(
        fixture.service.apply(await fixture.image('silent.png')),
        throwsStateError,
      );
      expect(previous.signals, isEmpty);
      expect(fixture.service.selectedPath, old.path);
      expect(fixture.processes.last.signals, [ProcessSignal.sigterm]);
    },
  );

  test(
    'persistence failure keeps the previous image and retires only the candidate',
    () async {
      fixture.onStart = (process) => process.ready();
      final old = await fixture.image('old.png');
      await fixture.service.apply(old);
      final previous = fixture.processes.single;
      await fixture.prefs.delete();
      await Directory(fixture.prefs.path).create();
      await expectLater(
        fixture.service.apply(await fixture.image('new.png')),
        throwsA(isA<FileSystemException>()),
      );
      expect(fixture.service.selectedPath, old.path);
      expect(previous.signals, isEmpty);
      expect(fixture.processes.last.signals, [ProcessSignal.sigterm]);
    },
  );

  test(
    'rapid selections serialize and dispose cancels a pending startup',
    () async {
      final first = fixture.service.apply(await fixture.image('first.png'));
      final firstProcess = await fixture.nextProcess();
      final second = fixture.service.apply(await fixture.image('second.png'));
      expect(fixture.processes.length, 1);
      firstProcess.ready();
      await first;
      final secondProcess = await fixture.nextProcess(after: 1);
      secondProcess.ready();
      await second;
      expect(firstProcess.signals, [ProcessSignal.sigterm]);
      expect(fixture.service.ownedPid, secondProcess.pid);
      final pending = fixture.service.apply(await fixture.image('pending.png'));
      final expectation = expectLater(pending, throwsStateError);
      final pendingProcess = await fixture.nextProcess(after: 2);
      fixture.service.dispose();
      await expectation.timeout(const Duration(seconds: 1));
      expect(secondProcess.signals, [ProcessSignal.sigterm]);
      expect(pendingProcess.signals, [ProcessSignal.sigterm]);
      expect(fixture.service.ownedPid, null);
    },
  );

  test(
    'restore applies only the saved path and provider exit is surfaced',
    () async {
      final image = await fixture.image('saved.png');
      await fixture.image('alphabetically-first.png');
      await fixture.save(image);
      fixture.onStart = (process) => process.ready();
      await fixture.service.restore(applySaved: true);
      expect(fixture.arguments.single[1], image.path);
      final changed = fixture.service.changes.first;
      fixture.processes.single.exit(1);
      await changed;
      expect(fixture.service.ownedPid, null);
      expect(fixture.service.error, contains('退出'));
    },
  );

  test(
    'shutdown escalates an unresponsive owned child and bounds cancellation',
    () async {
      fixture.onStart = (process) {
        process.ignoreTerm = true;
      };
      final failed = fixture.service.apply(await fixture.image('silent.png'));
      final expectation = expectLater(failed, throwsStateError);
      final process = await fixture.nextProcess();
      await expectation.timeout(const Duration(seconds: 2));
      expect(process.signals, [ProcessSignal.sigterm, ProcessSignal.sigkill]);
    },
  );

  test(
    'dispose while Process.start is pending closes the late child without saving',
    () async {
      fixture.startWait = Completer<void>();
      final pending = fixture.service.apply(await fixture.image('late.png'));
      final expectation = expectLater(pending, throwsStateError);
      final process = await fixture.nextProcess();
      fixture.service.dispose();
      fixture.startWait!.complete();
      await expectation.timeout(const Duration(seconds: 1));
      expect(process.signals, [ProcessSignal.sigterm]);
      expect(await fixture.prefs.exists(), false);
    },
  );

  test(
    'unsupported paths, malformed settings and absent sessions spawn nothing',
    () async {
      final missingSession = fixture.makeService(environment: {});
      addTearDown(missingSession.dispose);
      await expectLater(
        missingSession.apply(await fixture.image('session.png')),
        throwsStateError,
      );
      await expectLater(
        fixture.service.apply(
          const ShellWallpaper(
            path: 'relative.png',
            name: 'relative',
            sizeBytes: 1,
          ),
        ),
        throwsFormatException,
      );
      await expectLater(
        fixture.service.apply(await fixture.image('movie.mp4')),
        throwsFormatException,
      );
      await fixture.prefs.parent.create(recursive: true);
      await fixture.prefs.writeAsString('{bad');
      await expectLater(fixture.service.restore(), throwsFormatException);
      await fixture.prefs.writeAsString('x' * 65537);
      await expectLater(fixture.service.restore(), throwsFormatException);
      expect(fixture.processes, isEmpty);
    },
  );
}

Future<void> _turn() => Future<void>.delayed(Duration.zero);

class _Fixture {
  _Fixture(this.root) {
    service = makeService();
  }
  final Directory root;
  File get prefs => File('${root.path}/config/wallpaper.json');
  late final WallpaperService service;
  final processes = <_FakeProcess>[];
  final arguments = <List<String>>[];
  final environments = <Map<String, String>>[];
  final started = StreamController<_FakeProcess>.broadcast(sync: true);
  int outputs = 1;
  Completer<void>? startWait;
  void Function(_FakeProcess)? onStart;
  static Future<_Fixture> create() async => _Fixture(
    await Directory.systemTemp.createTemp('realand-wallpaper-test-'),
  );
  WallpaperService makeService({
    Map<String, String>? environment,
    File? preferenceFile,
  }) => WallpaperService(
    directoryPath: root.path,
    preferenceFile: preferenceFile ?? prefs,
    backendExecutable: '/usr/bin/true',
    environment: environment ?? {'WAYLAND_DISPLAY': 'fixture-wayland'},
    outputCounter: () async => outputs,
    startupTimeout: const Duration(milliseconds: 150),
    processStarter: (executable, args, env) async {
      expect(executable, '/usr/bin/true');
      final process = _FakeProcess(50000 + processes.length);
      processes.add(process);
      arguments.add(List.of(args));
      environments.add(Map.of(env));
      started.add(process);
      onStart?.call(process);
      await startWait?.future;
      return process;
    },
  );
  Future<_FakeProcess> nextProcess({int after = 0}) async =>
      processes.length > after ? processes[after] : started.stream.first;
  Future<ShellWallpaper> image(String name) async {
    final file = await File('${root.path}/$name').writeAsString('image');
    return ShellWallpaper(path: file.path, name: name, sizeBytes: 5);
  }

  Future<void> save(ShellWallpaper image) async {
    await prefs.parent.create(recursive: true);
    await prefs.writeAsString(
      jsonEncode({'version': 1, 'path': image.path, 'mode': 'fill'}),
    );
  }

  Future<void> close() async {
    service.dispose();
    for (final process in processes) {
      process.exit(0);
    }
    await _turn();
    await started.close();
    await root.delete(recursive: true);
  }
}

class _CountingBackendFile implements File {
  _CountingBackendFile(this.delegate);
  final File delegate;
  int asyncStats = 0;
  int syncStats = 0;

  @override
  Future<FileStat> stat() {
    asyncStats++;
    return delegate.stat();
  }

  @override
  FileStat statSync() {
    syncStats++;
    return delegate.statSync();
  }

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

class _DelayedPreferenceFile implements File {
  _DelayedPreferenceFile(this.delegate);
  final File delegate;
  final readStarted = Completer<void>();
  final releaseRead = Completer<void>();

  @override
  String get path => delegate.path;
  @override
  Directory get parent => delegate.parent;
  @override
  Future<bool> exists() => delegate.exists();
  @override
  Future<int> length() => delegate.length();
  @override
  Future<String> readAsString({Encoding encoding = utf8}) async {
    final value = await delegate.readAsString(encoding: encoding);
    readStarted.complete();
    await releaseRead.future;
    return value;
  }

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

class _FakeProcess implements Process {
  _FakeProcess(this.pid);
  @override
  final int pid;
  final _out = StreamController<List<int>>();
  final _err = StreamController<List<int>>();
  final _exit = Completer<int>();
  final signals = <ProcessSignal>[];
  bool ignoreTerm = false;
  @override
  Stream<List<int>> get stdout => _out.stream;
  @override
  Stream<List<int>> get stderr => _err.stream;
  @override
  Future<int> get exitCode => _exit.future;
  void line(String value) => _err.add(utf8.encode('$value\n'));
  void ready({int surface = 3, String separator = '#'}) {
    line(
      '[1] -> wl_surface$separator$surface.attach(wl_buffer${separator}14, 0, 0)',
    );
    line('[2] -> wl_surface$separator$surface.commit()');
  }

  void exit(int code) {
    if (_exit.isCompleted) return;
    _exit.complete(code);
    unawaited(_out.close());
    unawaited(_err.close());
  }

  @override
  bool kill([ProcessSignal signal = ProcessSignal.sigterm]) {
    signals.add(signal);
    if (ignoreTerm && signal == ProcessSignal.sigterm) return true;
    exit(0);
    return true;
  }

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}
