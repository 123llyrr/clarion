// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:io';

import 'package:dbus/dbus.dart';

import 'clarion_ipc.dart';
import 'system_service.dart';

enum SessionAction { sleep, restart, shutDown, lock, logOut }

typedef LogindCall = Future<String?> Function(String method, bool action);

/// Every request is bounded and owns its bus. Merely opening a menu never
/// starts a locker or changes session state.
class SessionService {
  SessionService({
    this.demo = false,
    LogindCall? logind,
    CommandRunner? run,
    Future<bool> Function()? lockerAvailable,
    Future<void> Function()? logOut,
  }) : _injectedLogind = logind,
       _run = run ?? runSystemCommand,
       _lockerAvailable = lockerAvailable ?? _hasLocker,
       _logOut = logOut ?? _quitCompositor;

  final bool demo;
  final LogindCall? _injectedLogind;
  final CommandRunner _run;
  final Future<bool> Function() _lockerAvailable;
  final Future<void> Function() _logOut;
  final _closed = Completer<void>();
  final _buses = <DBusClient>{};
  bool _disposed = false;

  void _ensureActive() {
    if (_disposed) throw StateError('会话菜单已关闭');
  }

  void dispose() {
    if (_disposed) return;
    _disposed = true;
    _closed.complete();
    for (final bus in _buses) {
      unawaited(bus.close());
    }
  }

  Future<Set<SessionAction>> capabilities() async {
    _ensureActive();
    if (demo) return SessionAction.values.toSet();
    final result = <SessionAction>{SessionAction.logOut};
    if (await _lockerAvailable()) result.add(SessionAction.lock);
    _ensureActive();
    await Future.wait([
      for (final entry in const {
        SessionAction.sleep: 'CanSuspend',
        SessionAction.restart: 'CanReboot',
        SessionAction.shutDown: 'CanPowerOff',
      }.entries)
        () async {
          try {
            final response = await _logind(entry.value, false);
            if ((response == 'yes' || response == 'challenge') &&
                (entry.key != SessionAction.sleep ||
                    result.contains(SessionAction.lock))) {
              result.add(entry.key);
            }
          } on Object {
            // Unsupported or inaccessible actions remain disabled.
          }
        }(),
    ]);
    _ensureActive();
    return Set.unmodifiable(result);
  }

  Future<void> perform(SessionAction action) async {
    _ensureActive();
    if (demo) return;
    // Recheck at activation: menu capabilities may have become stale.
    if (!(await capabilities()).contains(action)) {
      throw StateError('系统当前不支持此操作');
    }
    _ensureActive();
    switch (action) {
      case SessionAction.lock:
        await _run('/usr/bin/swaylock', ['--daemonize']);
      case SessionAction.sleep:
        // --daemonize returns only after the compositor has locked the
        // session. A failed locker must never be followed by suspend.
        await _run('/usr/bin/swaylock', ['--daemonize']);
        _ensureActive();
        await _logind('Suspend', true);
      case SessionAction.restart:
        await _logind('Reboot', true);
      case SessionAction.shutDown:
        await _logind('PowerOff', true);
      case SessionAction.logOut:
        await _logOut();
    }
  }

  Future<String?> _logind(String method, bool action) async {
    _ensureActive();
    return Future.any<String?>([
      _injectedLogind?.call(method, action) ?? _hostLogind(method, action),
      _closed.future.then<String?>((_) => throw StateError('会话菜单已关闭')),
    ]).timeout(Duration(seconds: action ? 60 : 5));
  }

  Future<String?> _hostLogind(String method, bool action) async {
    final bus = DBusClient.system();
    _buses.add(bus);
    try {
      final remote = DBusRemoteObject(
        bus,
        name: 'org.freedesktop.login1',
        path: DBusObjectPath('/org/freedesktop/login1'),
      );
      final response = await remote.callMethod(
        'org.freedesktop.login1.Manager',
        method,
        action ? [const DBusBoolean(true)] : [],
        replySignature: DBusSignature(action ? '' : 's'),
        noAutoStart: true,
        allowInteractiveAuthorization: action,
      );
      return action ? null : (response.values.single as DBusString).value;
    } finally {
      _buses.remove(bus);
      await bus.close();
    }
  }
}

Future<bool> _hasLocker() async {
  try {
    final stat = await File('/usr/bin/swaylock').stat();
    return stat.type == FileSystemEntityType.file && stat.mode & 0x49 != 0;
  } on FileSystemException {
    return false;
  }
}

Future<void> _quitCompositor() async {
  final ipc = ClarionIpc();
  try {
    await ipc.action('Quit', {'skip_confirmation': true});
  } finally {
    ipc.dispose();
  }
}
