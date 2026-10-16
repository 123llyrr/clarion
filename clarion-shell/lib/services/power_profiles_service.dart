// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';

import 'package:dbus/dbus.dart';
import 'package:flutter/foundation.dart';

class PowerProfile {
  const PowerProfile(this.id);
  final String id;

  String get label => switch (id) {
    'power-saver' => '省电',
    'balanced' => '均衡',
    'performance' => '性能',
    _ => '未知模式',
  };

  String get description => switch (id) {
    'power-saver' => '降低能耗，延长电池续航',
    'balanced' => '平衡响应速度和电池续航',
    'performance' => '优先响应速度，可能增加耗电和风扇噪声',
    _ => '',
  };
}

const _supportedProfiles = [
  PowerProfile('power-saver'),
  PowerProfile('balanced'),
  PowerProfile('performance'),
];
const _endpoints = [
  (
    name: 'org.freedesktop.UPower.PowerProfiles',
    path: '/org/freedesktop/UPower/PowerProfiles',
  ),
  (name: 'net.hadess.PowerProfiles', path: '/net/hadess/PowerProfiles'),
];

/// Uses the existing system power-profile provider without activating a daemon.
/// An injected bus is owned and closed by this service once started.
class PowerProfilesService extends ChangeNotifier {
  PowerProfilesService({
    this.demo = false,
    DBusClient? bus,
    this.requestTimeout = const Duration(seconds: 5),
  }) : _injectedBus = bus {
    if (demo) {
      _available = true;
      _profiles = _supportedProfiles;
      _activeProfile = 'balanced';
    }
  }

  final bool demo;
  final DBusClient? _injectedBus;
  final Duration requestTimeout;
  DBusClient? _bus;
  DBusRemoteObject? _remote;
  String? _interface;
  String? _serviceName;
  StreamSubscription<DBusNameOwnerChangedEvent>? _owners;
  StreamSubscription<DBusPropertiesChangedSignal>? _properties;
  Future<void>? _startFuture;
  Future<void>? _refreshFuture;
  bool _started = false;
  bool _disposed = false;
  bool _refreshDirty = false;
  int _revision = 0;
  bool _available = false;
  bool _loading = false;
  bool _busy = false;
  String? _activeProfile;
  List<PowerProfile> _profiles = const [];
  String _performanceDegraded = '';
  String? _error;

  bool get available => _available;
  bool get loading => _loading;
  bool get busy => _busy;
  String? get activeProfile => _activeProfile;
  List<PowerProfile> get profiles => _profiles;
  String get performanceDegraded => _performanceDegraded;
  String? get error => _error;

  String? get degradedReason => switch (_performanceDegraded) {
    '' => null,
    'lap-detected' => '设备检测到放在腿上，已限制性能以降低温度',
    'high-operating-temperature' => '设备温度较高，系统正在限制性能',
    _ => '系统正在限制性能，以保护设备或降低能耗',
  };

  Future<void> start() =>
      _disposed ? Future.value() : _startFuture ??= _start();

  Future<void> _start() async {
    _started = true;
    if (demo) return;
    try {
      _bus = _injectedBus ?? DBusClient.system();
      _owners = _bus!.nameOwnerChanged.listen(
        _ownerChanged,
        onError: (Object _) => _unavailable('无法连接系统电源服务'),
      );
      await refresh();
    } on Object catch (error) {
      if (!_disposed) _unavailable(_errorMessage(error));
    }
  }

  Future<void> refresh() {
    if (_disposed || demo) return Future.value();
    if (!_started) return start();
    _refreshDirty = true;
    return _refreshFuture ??= _refreshLoop().whenComplete(() {
      _refreshFuture = null;
      if (_refreshDirty && !_disposed) {
        scheduleMicrotask(() => unawaited(refresh()));
      }
    });
  }

  Future<void> _refreshLoop() async {
    _loading = true;
    _notify();
    try {
      while (_refreshDirty && !_disposed) {
        _refreshDirty = false;
        final revision = _revision;
        Object? failure;
        var found = false;
        for (final endpoint in _endpoints) {
          StreamSubscription<DBusPropertiesChangedSignal>? pendingProperties;
          try {
            final owner = await _bus!
                .getNameOwner(endpoint.name)
                .timeout(requestTimeout);
            if (_disposed || revision != _revision) break;
            if (owner == null) continue;
            // Bind reads, signals and writes to this owner so a replaced daemon
            // cannot inherit a pending user request meant for its predecessor.
            final remote = DBusRemoteObject(
              _bus!,
              name: owner,
              path: DBusObjectPath(endpoint.path),
            );
            // Subscribe before reading: a property change during GetAll must
            // schedule another read instead of leaving the initial UI stale.
            pendingProperties = remote.propertiesChanged.listen((signal) {
              if (!_disposed &&
                  revision == _revision &&
                  signal.propertiesInterface == endpoint.name) {
                unawaited(refresh());
              }
            }, onError: (Object _) => unawaited(refresh()));
            final values = await remote
                .getAllProperties(endpoint.name)
                .timeout(requestTimeout);
            if (_disposed || revision != _revision) break;
            final profiles = _parseProfiles(values['Profiles']);
            final active = _string(values['ActiveProfile']);
            if (profiles.isEmpty ||
                !profiles.any((profile) => profile.id == active)) {
              throw const FormatException('Invalid power profiles');
            }
            await _cancel(_properties);
            if (_disposed || revision != _revision) break;
            _remote = remote;
            _interface = endpoint.name;
            _serviceName = endpoint.name;
            _properties = pendingProperties;
            pendingProperties = null;
            _available = true;
            _profiles = List.unmodifiable(profiles);
            _activeProfile = active;
            _performanceDegraded = _string(values['PerformanceDegraded']);
            _error = null;
            found = true;
            break;
          } on Object catch (error) {
            failure = error;
          } finally {
            await _cancel(pendingProperties);
          }
        }
        if (_disposed) return;
        if (revision != _revision) {
          _refreshDirty = true;
          continue;
        }
        if (!found) {
          _unavailable(
            failure == null ? '系统未提供电源模式服务' : _errorMessage(failure),
          );
        }
      }
    } finally {
      if (!_disposed) {
        _loading = false;
        _notify();
      }
    }
  }

  Future<void> setProfile(String profile) async {
    if (_disposed || _busy) return;
    if (!_available ||
        !_supportedProfiles.any((entry) => entry.id == profile) ||
        !_profiles.any((entry) => entry.id == profile)) {
      _error = '此电源模式不可用';
      _notify();
      return;
    }
    if (profile == _activeProfile) return;
    if (demo) {
      _activeProfile = profile;
      _error = null;
      _notify();
      return;
    }
    final remote = _remote;
    final interface = _interface;
    if (remote == null || interface == null) return;
    final revision = _revision;
    _busy = true;
    _error = null;
    _notify();
    try {
      await remote
          .callMethod(
            'org.freedesktop.DBus.Properties',
            'Set',
            [
              DBusString(interface),
              const DBusString('ActiveProfile'),
              DBusVariant(DBusString(profile)),
            ],
            replySignature: DBusSignature(''),
            noAutoStart: true,
            allowInteractiveAuthorization: true,
          )
          .timeout(requestTimeout);
      if (!_disposed && revision == _revision) await refresh();
    } on Object catch (error) {
      if (!_disposed && revision == _revision) {
        _error = _errorMessage(error);
      }
    } finally {
      if (!_disposed) {
        _busy = false;
        _notify();
      }
    }
  }

  void _ownerChanged(DBusNameOwnerChangedEvent event) {
    if (_disposed || !_endpoints.any((item) => item.name == event.name)) return;
    if (event.oldOwner != null) {
      _revision++;
      if (event.name == _serviceName) {
        _unavailable('系统电源服务已断开，等待恢复');
      }
    }
    unawaited(refresh());
  }

  void _unavailable(String reason) {
    if (_disposed) return;
    _available = false;
    _profiles = const [];
    _activeProfile = null;
    _performanceDegraded = '';
    _remote = null;
    _interface = null;
    _serviceName = null;
    final properties = _properties;
    _properties = null;
    unawaited(_cancel(properties));
    _error = reason;
    _notify();
  }

  void _notify() {
    if (!_disposed) notifyListeners();
  }

  Future<void> _cancel(StreamSubscription<dynamic>? subscription) async {
    if (subscription == null) return;
    try {
      await subscription.cancel().timeout(requestTimeout);
    } on Object {
      // A bus disappearing during RemoveMatch must not escape an asynchronous
      // refresh or prevent the service from discarding its previous owner.
    }
  }

  Future<void> _close(DBusClient bus) async {
    try {
      await bus.close().timeout(requestTimeout);
    } on Object {
      // Disposal still finishes if the system bus is already disconnected.
    }
  }

  @override
  void dispose() {
    if (_disposed) return;
    _disposed = true;
    _revision++;
    final owners = _owners;
    final properties = _properties;
    final bus = _bus;
    unawaited(_cancel(owners));
    unawaited(_cancel(properties));
    if (bus != null) unawaited(_close(bus));
    super.dispose();
  }
}

String _string(DBusValue? value) => value is DBusString ? value.value : '';

List<PowerProfile> _parseProfiles(DBusValue? value) {
  if (value is! DBusArray || value.signature != DBusSignature('aa{sv}')) {
    return const [];
  }
  final ids = <String>{};
  for (final entry in value.children.take(16)) {
    if (entry is! DBusDict) continue;
    final id = _string(entry.asStringVariantDict()['Profile']);
    if (_supportedProfiles.any((profile) => profile.id == id)) ids.add(id);
  }
  return [
    for (final profile in _supportedProfiles)
      if (ids.contains(profile.id)) profile,
  ];
}

String _errorMessage(Object error) {
  if (error is TimeoutException) return '系统电源服务响应超时，请重试';
  if (error is DBusMethodResponseException) {
    final name = error.response.errorName;
    if (name.contains('AccessDenied') ||
        name.contains('NotAuthorized') ||
        name.contains('Auth') ||
        name.contains('PermissionDenied')) {
      return '没有切换电源模式的权限，请检查系统授权';
    }
  }
  return '无法读取或切换系统电源模式，请重试';
}
