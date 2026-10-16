// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/services/audio_output_service.dart';

String _outputs([String description = 'Speakers']) => jsonEncode([
  {
    'name': 'speakers',
    'description': description,
    'ports': [
      {'availability': 'availability unknown'},
    ],
  },
  {'name': 'usb', 'description': 'USB Audio', 'ports': []},
  {
    'name': 'hdmi',
    'description': 'HDMI',
    'ports': [
      {'availability': 'not available'},
    ],
  },
]);

class _Backend {
  final events = StreamController<void>.broadcast(sync: true);
  final commands = <List<String>>[];
  String selected = 'speakers';
  String source = _outputs();
  int watching = 0;
  int closed = 0;
  Completer<String>? pendingList;
  Completer<void>? pendingWrite;
  Object? writeFailure;
  Future<String> command(String executable, List<String> args) async {
    expectSync(executable, 'pactl');
    commands.add(args);
    if (args.first == '--format=json') {
      final pending = pendingList;
      pendingList = null;
      return pending?.future ?? source;
    }
    if (args.first == 'get-default-sink') return selected;
    if (args.first == 'set-default-sink') {
      await pendingWrite?.future;
      if (writeFailure != null) throw writeFailure!;
      selected = args[1];
      return '';
    }
    throw StateError('Unexpected command $args');
  }

  Future<AudioEventWatch> watch() async {
    watching++;
    return AudioEventWatch(events.stream, () async {
      closed++;
    });
  }
}

Future<void> _settle(WidgetTester tester) async {
  for (var i = 0; i < 8; i++) {
    await tester.pump();
  }
}

void main() {
  test(
    'JSON parsing retains real names, port availability and immutable data',
    () {
      final outputs = AudioOutputService.parseOutputs(_outputs());
      expect(outputs.map((output) => output.name), ['speakers', 'usb', 'hdmi']);
      expect(outputs.map((output) => output.available), [true, true, false]);
      expect(() => outputs.clear(), throwsUnsupportedError);
      expect(
        AudioOutputService.parseOutputs(
          '[{"name":"-bad"},{"name":""},{"name":"safe"}]',
        ).single.description,
        'safe',
      );
      expect(
        () => AudioOutputService.parseOutputs('{}'),
        throwsFormatException,
      );
    },
  );

  testWidgets(
    'opening only reads and closing releases the event subscription',
    (tester) async {
      final backend = _Backend();
      final service = AudioOutputService(
        command: backend.command,
        watch: backend.watch,
      );
      addTearDown(service.dispose);
      addTearDown(backend.events.close);
      expect(backend.commands, isEmpty);
      service.setActive(true);
      await _settle(tester);
      expect(service.error, isNull);
      expect(service.outputs, hasLength(3));
      expect(service.selected, 'speakers');
      expect(service.loading, isFalse);
      expect(
        backend.commands.every((args) => args.first != 'set-default-sink'),
        isTrue,
      );
      expect(backend.watching, 1);
      service.setActive(false);
      final reads = backend.commands.length;
      backend.events.add(null);
      await tester.pump(const Duration(seconds: 3));
      expect(backend.commands, hasLength(reads));
      expect(backend.closed, 1);
    },
  );

  testWidgets(
    'drag event bursts defer reads and resume with one fresh snapshot',
    (tester) async {
      final backend = _Backend();
      final service = AudioOutputService(
        command: backend.command,
        watch: backend.watch,
      );
      addTearDown(service.dispose);
      addTearDown(backend.events.close);
      service.setActive(true);
      await _settle(tester);
      backend.commands.clear();
      service.setPaused(true);
      backend.selected = 'usb';
      backend.source = _outputs('New external name');
      for (var frame = 0; frame < 30; frame++) {
        backend.events.add(null);
        await tester.pump(const Duration(milliseconds: 16));
      }
      await tester.pump(const Duration(milliseconds: 110));
      expect(backend.commands, isEmpty);
      expect(service.selected, 'speakers');
      service.setPaused(false);
      await _settle(tester);
      expect(backend.commands, hasLength(2));
      expect(service.selected, 'usb');
      expect(service.outputs.first.description, 'New external name');
      expect(backend.watching, 1);
    },
  );

  testWidgets(
    'an old read cannot replace a closed and reopened menu snapshot',
    (tester) async {
      final backend = _Backend();
      final service = AudioOutputService(
        command: backend.command,
        watch: backend.watch,
      );
      addTearDown(service.dispose);
      addTearDown(backend.events.close);
      service.setActive(true);
      await _settle(tester);
      final pending = Completer<String>();
      backend.pendingList = pending;
      unawaited(service.refresh());
      await tester.pump();
      service.setActive(false);
      backend.selected = 'usb';
      backend.source = _outputs('Current speakers');
      service.setActive(true);
      pending.complete(_outputs('Stale speakers'));
      await _settle(tester);
      expect(service.outputs.first.description, 'Current speakers');
      expect(service.selected, 'usb');
      expect(service.error, isNull);
    },
  );

  testWidgets(
    'device writes serialize, reject missing outputs and confirm the server result',
    (tester) async {
      final backend = _Backend();
      final service = AudioOutputService(
        command: backend.command,
        watch: backend.watch,
      );
      addTearDown(service.dispose);
      addTearDown(backend.events.close);
      service.setActive(true);
      await _settle(tester);
      backend.commands.clear();
      final write = Completer<void>();
      backend.pendingWrite = write;
      final changing = service.select('usb');
      await tester.pump();
      expect(service.busy, isTrue);
      expect(service.selected, 'speakers');
      await service.select('speakers');
      expect(
        backend.commands.where((args) => args.first == 'set-default-sink'),
        hasLength(1),
      );
      write.complete();
      await changing;
      expect(service.selected, 'usb');
      expect(service.busy, isFalse);
      await expectLater(service.select('missing'), throwsStateError);
      await expectLater(service.select('hdmi'), throwsStateError);
      backend.pendingWrite = null;
      backend.writeFailure = StateError('permission denied');
      await expectLater(service.select('speakers'), throwsStateError);
      expect(service.selected, 'usb');
      expect(service.busy, isFalse);
    },
  );

  testWidgets(
    'pending watcher completion after closing is immediately released',
    (tester) async {
      final backend = _Backend();
      final pendingWatch = Completer<AudioEventWatch>();
      final service = AudioOutputService(
        command: backend.command,
        watch: () => pendingWatch.future,
      );
      addTearDown(service.dispose);
      addTearDown(backend.events.close);
      service.setActive(true);
      await _settle(tester);
      service.setActive(false);
      pendingWatch.complete(await backend.watch());
      await _settle(tester);
      expect(backend.closed, 1);
    },
  );
}
