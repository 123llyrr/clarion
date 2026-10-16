// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/services/session_service.dart';

void main() {
  test(
    'dispose cancels pending capability reads and rejects new actions',
    () async {
      final pending = Completer<String?>();
      var reads = 0;
      final service = SessionService(
        lockerAvailable: () async => true,
        logind: (_, action) {
          expect(action, isFalse);
          reads++;
          return pending.future;
        },
      );
      final capabilities = service.capabilities();
      final expectation = expectLater(capabilities, throwsStateError);
      await Future<void>.delayed(Duration.zero);
      expect(reads, 3);
      service.dispose();
      await expectation;
      pending.complete('yes');
      await expectLater(
        service.perform(SessionAction.restart),
        throwsStateError,
      );
      expect(reads, 3);
    },
  );

  test(
    'closing menu while locker starts never submits a later suspend',
    () async {
      final locker = Completer<String>();
      final actions = <String>[];
      final service = SessionService(
        lockerAvailable: () async => true,
        logind: (method, action) async {
          if (action) actions.add(method);
          return action ? null : 'yes';
        },
        run: (_, _) => locker.future,
      );
      final operation = service.perform(SessionAction.sleep);
      final expectation = expectLater(operation, throwsStateError);
      await Future<void>.delayed(Duration.zero);
      service.dispose();
      locker.complete('');
      await expectation;
      expect(actions, isEmpty);
    },
  );

  test(
    'capabilities accept challenge, reject unsupported and failed reads',
    () async {
      final service = SessionService(
        lockerAvailable: () async => true,
        logind: (method, action) async => switch (method) {
          'CanSuspend' => 'yes',
          'CanReboot' => 'challenge',
          'CanPowerOff' => throw StateError('unavailable'),
          _ => throw StateError('unexpected action'),
        },
      );
      expect(await service.capabilities(), {
        SessionAction.lock,
        SessionAction.sleep,
        SessionAction.restart,
        SessionAction.logOut,
      });
    },
  );

  test(
    'sleep requires successful lock before suspend and never retries',
    () async {
      final events = <String>[];
      var failedLock = false;
      final service = SessionService(
        lockerAvailable: () async => true,
        logind: (method, action) async {
          if (action) events.add(method);
          return action ? null : 'yes';
        },
        run: (executable, args) async {
          expect(executable, '/usr/bin/swaylock');
          expect(args, ['--daemonize']);
          events.add('lock');
          if (failedLock) throw StateError('failed to lock');
          return '';
        },
      );
      await service.perform(SessionAction.sleep);
      expect(events, ['lock', 'Suspend']);
      events.clear();
      failedLock = true;
      await expectLater(service.perform(SessionAction.sleep), throwsStateError);
      expect(events, ['lock']);
    },
  );

  test(
    'rechecks permissions and calls exact real session action once',
    () async {
      final events = <String>[];
      var allowed = true;
      final service = SessionService(
        lockerAvailable: () async => false,
        logind: (method, action) async {
          if (action) events.add(method);
          return action
              ? null
              : allowed
              ? 'yes'
              : 'no';
        },
        logOut: () async {
          events.add('Quit');
        },
        run: (_, _) async => throw StateError('no locker'),
      );
      expect(
        await service.capabilities(),
        isNot(contains(SessionAction.sleep)),
      );
      await service.perform(SessionAction.restart);
      await service.perform(SessionAction.shutDown);
      await service.perform(SessionAction.logOut);
      expect(events, ['Reboot', 'PowerOff', 'Quit']);
      allowed = false;
      await expectLater(
        service.perform(SessionAction.restart),
        throwsStateError,
      );
      await expectLater(service.perform(SessionAction.lock), throwsStateError);
      expect(events, ['Reboot', 'PowerOff', 'Quit']);
    },
  );

  test('demo never executes host commands', () async {
    final service = SessionService(
      demo: true,
      logind: (_, _) async => throw StateError('host bus'),
      run: (_, _) async => throw StateError('host command'),
      logOut: () async => throw StateError('host logout'),
    );
    for (final action in SessionAction.values) {
      await service.perform(action);
    }
    expect(await service.capabilities(), SessionAction.values.toSet());
  });
}
