// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:io';

import 'package:dbus/dbus.dart';
import 'package:flutter/foundation.dart';

enum MediaPlaybackStatus { playing, paused, stopped, unknown }

enum MediaCommand { playPause, previous, next }

@immutable
class MediaPlayer {
  const MediaPlayer({
    required this.busName,
    required this.owner,
    required this.connectionId,
    required this.identity,
    this.title = '',
    this.artist = '',
    this.status = MediaPlaybackStatus.unknown,
    this.canControl = false,
    this.canPlay = false,
    this.canPause = false,
    this.canGoPrevious = false,
    this.canGoNext = false,
  });
  final String busName, owner, identity, title, artist;
  final int connectionId;
  final MediaPlaybackStatus status;
  final bool canControl, canPlay, canPause, canGoPrevious, canGoNext;

  bool allows(MediaCommand command) =>
      canControl &&
      switch (command) {
        MediaCommand.previous => canGoPrevious,
        MediaCommand.next => canGoNext,
        MediaCommand.playPause => switch (status) {
          MediaPlaybackStatus.playing => canPause,
          MediaPlaybackStatus.paused || MediaPlaybackStatus.stopped => canPlay,
          MediaPlaybackStatus.unknown => false,
        },
      };

  Object get _value => (
    busName,
    owner,
    connectionId,
    identity,
    title,
    artist,
    status,
    canControl,
    canPlay,
    canPause,
    canGoPrevious,
    canGoNext,
  );
  @override
  bool operator ==(Object other) =>
      other is MediaPlayer && other._value == _value;
  @override
  int get hashCode => _value.hashCode;
}

/// Event-driven MPRIS 2 discovery. Construction never connects to D-Bus;
/// start only observes already-running players, and never activates one.
/// An injected bus is owned and closed by this service once start is called.
class MediaService extends ChangeNotifier {
  MediaService({
    DBusClient? bus,
    bool? enabled,
    this.maximumPlayers = 16,
    this.requestTimeout = const Duration(seconds: 4),
  }) : _injectedBus = bus,
       enabled =
           enabled ??
           (bus != null ||
               (Platform.isLinux &&
                   !Platform.environment.containsKey('FLUTTER_TEST') &&
                   Platform.environment.containsKey(
                     'DBUS_SESSION_BUS_ADDRESS',
                   ))),
       assert(maximumPlayers > 0 && maximumPlayers <= 64);

  static const _root = 'org.mpris.MediaPlayer2';
  static const _player = 'org.mpris.MediaPlayer2.Player';
  static final _path = DBusObjectPath('/org/mpris/MediaPlayer2');
  static const _playerProperties = {
    'PlaybackStatus',
    'Metadata',
    'CanControl',
    'CanPlay',
    'CanPause',
    'CanGoPrevious',
    'CanGoNext',
  };
  final bool enabled;
  final int maximumPlayers;
  final Duration requestTimeout;
  final DBusClient? _injectedBus;
  DBusClient? _bus;
  StreamSubscription<DBusNameOwnerChangedEvent>? _owners;
  Future<void>? _startWork, _discoveryWork;
  bool _discoveryDirty = false, _disposed = false;
  int _ownerRevision = 0, _nextConnection = 0;
  final _connections = <String, _PlayerConnection>{};
  List<MediaPlayer> _players = const [];
  MediaPlayer? _selected;
  String? _preferredName;
  String? _error;
  int? _errorConnection;

  List<MediaPlayer> get players => _players;
  MediaPlayer? get selected => _selected;
  bool get busy => _connections[_selected?.busName]?.busy ?? false;
  String? get error =>
      _errorConnection == _selected?.connectionId ? _error : null;

  Future<void> start() =>
      _disposed || !enabled ? Future.value() : _startWork ??= _start();

  Future<void> _start() async {
    try {
      _bus = _injectedBus ?? DBusClient.session();
      _owners = _bus!.nameOwnerChanged.listen(
        _ownerChanged,
        onError: (Object _) => _disconnect(),
        onDone: _disconnect,
      );
      await _discover();
    } on Object {
      _disconnect();
    }
  }

  bool _current(_PlayerConnection connection) =>
      !_disposed && identical(_connections[connection.name], connection);

  void _ownerChanged(DBusNameOwnerChangedEvent event) {
    if (_disposed || !event.name.startsWith('$_root.')) return;
    _ownerRevision++;
    _setOwner(event.name, event.newOwner);
    // A newly free slot may admit a player beyond the bounded initial list.
    unawaited(_discover());
  }

  Future<void> _discover() {
    if (_disposed || _bus == null) return Future.value();
    _discoveryDirty = true;
    return _discoveryWork ??= _discoverLoop().whenComplete(() {
      _discoveryWork = null;
      if (_discoveryDirty && !_disposed) {
        scheduleMicrotask(() => unawaited(_discover()));
      }
    });
  }

  Future<void> _discoverLoop() async {
    while (_discoveryDirty && !_disposed) {
      _discoveryDirty = false;
      final revision = _ownerRevision;
      try {
        final names = (await _bus!.listNames().timeout(
          requestTimeout,
        )).where((name) => name.startsWith('$_root.')).toList()..sort();
        if (_disposed) return;
        for (final name in names) {
          if (_disposed || revision != _ownerRevision) break;
          if (_connections.containsKey(name)) continue;
          if (_connections.length >= maximumPlayers) break;
          final owner = await _bus!.getNameOwner(name).timeout(requestTimeout);
          if (_disposed || revision != _ownerRevision) break;
          _setOwner(name, owner);
        }
        // A start() caller can wait for the initial bounded snapshot. Later
        // players refresh independently so one slow player blocks no other.
        await Future.wait(
          _connections.values.map(
            (connection) => connection.readWork ?? Future<void>.value(),
          ),
        );
      } on Object {
        // A transient discovery failure must not resurrect exited players or
        // start polling. A later owner change triggers another discovery.
      }
    }
  }

  void _setOwner(String name, String? owner) {
    final old = _connections[name];
    if (old != null && old.owner == owner) return;
    if (old != null) {
      _connections.remove(name);
      unawaited(_cancel(old.subscription));
      if (_errorConnection == old.id) {
        _error = null;
        _errorConnection = null;
      }
      if (_preferredName == name) _preferredName = null;
    }
    if (owner != null &&
        owner.isNotEmpty &&
        _connections.length < maximumPlayers &&
        !_disposed) {
      final connection = _PlayerConnection(
        name,
        owner,
        ++_nextConnection,
        DBusRemoteObject(_bus!, name: owner, path: _path),
      );
      _connections[name] = connection;
      connection.subscription = connection.remote.propertiesChanged.listen(
        (signal) => _propertiesChanged(connection, signal),
        onError: (Object _) {
          if (_current(connection)) unawaited(_refresh(connection));
        },
      );
      unawaited(_refresh(connection));
    }
    _publish();
  }

  void _propertiesChanged(
    _PlayerConnection connection,
    DBusPropertiesChangedSignal signal,
  ) {
    if (!_current(connection)) return;
    try {
      final interface = signal.propertiesInterface;
      final keys = interface == _root
          ? const {'Identity'}
          : interface == _player
          ? _playerProperties
          : const <String>{};
      if (keys.isEmpty) return;
      final changed = signal.changedProperties;
      final invalid = signal.invalidatedProperties.where(keys.contains).toSet();
      if (!changed.keys.any(keys.contains) && invalid.isEmpty) return;
      connection.revision++;
      final values = interface == _root ? connection.root : connection.values;
      for (final key in keys) {
        if (changed.containsKey(key)) values[key] = changed[key]!;
        if (invalid.contains(key)) values.remove(key);
      }
      if (connection.snapshot != null) _updateSnapshot(connection);
      // Signals with actual values require no round-trip. Invalidation and a
      // signal racing initial GetAll request one coalesced re-read.
      if (invalid.isNotEmpty ||
          connection.readWork != null ||
          connection.snapshot == null) {
        unawaited(_refresh(connection));
      }
    } on Object {
      // A malformed signal from one player must not tear down all players.
    }
  }

  Future<void> _refresh(_PlayerConnection connection) {
    if (!_current(connection)) return Future.value();
    connection.dirty = true;
    return connection.readWork ??= _readLoop(connection).whenComplete(() {
      connection.readWork = null;
      if (connection.dirty && _current(connection)) {
        scheduleMicrotask(() => unawaited(_refresh(connection)));
      }
    });
  }

  Future<Map<String, DBusValue>> _getAll(
    _PlayerConnection connection,
    String interface,
  ) async {
    final response = await connection.remote
        .callMethod(
          'org.freedesktop.DBus.Properties',
          'GetAll',
          [DBusString(interface)],
          replySignature: DBusSignature('a{sv}'),
          noAutoStart: true,
        )
        .timeout(requestTimeout);
    return response.returnValues.single.asStringVariantDict();
  }

  Future<void> _readLoop(_PlayerConnection connection) async {
    while (_current(connection) && connection.dirty) {
      connection.dirty = false;
      final revision = connection.revision;
      try {
        final values = await Future.wait([
          _getAll(connection, _root),
          _getAll(connection, _player),
        ]);
        if (!_current(connection)) return;
        if (revision != connection.revision) continue;
        connection.root = {'Identity': ?values[0]['Identity']};
        connection.values = {
          for (final key in _playerProperties) key: ?values[1][key],
        };
        _updateSnapshot(connection);
      } on Object {
        if (!_current(connection)) return;
        // Retaining enabled controls after a failed full read would act on
        // unverified capabilities. Hide this snapshot until a real event.
        connection.snapshot = null;
        _publish();
      }
    }
  }

  void _updateSnapshot(_PlayerConnection connection) {
    final values = connection.values;
    final metadata = values['Metadata'];
    Map<String, DBusValue> track = const {};
    if (metadata?.signature == DBusSignature('a{sv}')) {
      track = metadata!.asStringVariantDict();
    }
    final artists = track['xesam:artist'];
    final identity = _text(connection.root['Identity']);
    final snapshot = MediaPlayer(
      busName: connection.name,
      owner: connection.owner,
      connectionId: connection.id,
      identity: identity.isEmpty
          ? connection.name.substring(_root.length + 1)
          : identity,
      title: _text(track['xesam:title']),
      artist: artists?.signature == DBusSignature('as')
          ? artists!
                .asStringArray()
                .take(8)
                .map((artist) => _clean(artist, 128))
                .join('、')
          : '',
      status: switch (_text(values['PlaybackStatus'])) {
        'Playing' => MediaPlaybackStatus.playing,
        'Paused' => MediaPlaybackStatus.paused,
        'Stopped' => MediaPlaybackStatus.stopped,
        _ => MediaPlaybackStatus.unknown,
      },
      canControl: _boolean(values['CanControl']),
      canPlay: _boolean(values['CanPlay']),
      canPause: _boolean(values['CanPause']),
      canGoPrevious: _boolean(values['CanGoPrevious']),
      canGoNext: _boolean(values['CanGoNext']),
    );
    if (snapshot == connection.snapshot) return;
    connection.snapshot = snapshot;
    _publish();
  }

  void _publish({bool force = false}) {
    if (_disposed) return;
    final players =
        _connections.values
            .map((value) => value.snapshot)
            .whereType<MediaPlayer>()
            .toList()
          ..sort((a, b) {
            final identity = a.identity.toLowerCase().compareTo(
              b.identity.toLowerCase(),
            );
            return identity != 0 ? identity : a.busName.compareTo(b.busName);
          });
    final selected =
        players
            .where((player) => player.busName == _preferredName)
            .firstOrNull ??
        players
            .where((player) => player.status == MediaPlaybackStatus.playing)
            .firstOrNull ??
        players.firstOrNull;
    final changed = !listEquals(players, _players) || selected != _selected;
    if (!listEquals(players, _players)) _players = List.unmodifiable(players);
    _selected = selected;
    if (force || changed) notifyListeners();
  }

  bool selectPlayer(MediaPlayer player) {
    final connection = _connections[player.busName];
    if (_disposed ||
        connection?.id != player.connectionId ||
        connection?.owner != player.owner) {
      return false;
    }
    _preferredName = player.busName;
    _publish(force: true);
    return true;
  }

  Future<bool> invoke(MediaPlayer player, MediaCommand command) async {
    final connection = _connections[player.busName];
    if (_disposed ||
        connection == null ||
        connection.id != player.connectionId ||
        connection.owner != player.owner ||
        connection.busy ||
        connection.snapshot?.allows(command) != true) {
      return false;
    }
    connection.busy = true;
    _error = null;
    _errorConnection = connection.id;
    _publish(force: true);
    try {
      await connection.remote
          .callMethod(
            _player,
            switch (command) {
              MediaCommand.playPause => 'PlayPause',
              MediaCommand.previous => 'Previous',
              MediaCommand.next => 'Next',
            },
            const [],
            replySignature: DBusSignature(''),
            noAutoStart: true,
          )
          .timeout(requestTimeout);
      return _current(connection);
    } on Object {
      if (_current(connection)) {
        _errorConnection = connection.id;
        _error = '播放器未完成操作，请检查播放状态';
      }
      return false;
    } finally {
      connection.busy = false;
      if (_current(connection)) _publish(force: true);
    }
  }

  void _disconnect() {
    if (_disposed) return;
    _ownerRevision++;
    for (final connection in _connections.values) {
      unawaited(_cancel(connection.subscription));
    }
    _connections.clear();
    _preferredName = null;
    _error = null;
    _publish();
  }

  @override
  void dispose() {
    if (_disposed) return;
    _disposed = true;
    unawaited(_cancel(_owners));
    for (final connection in _connections.values) {
      unawaited(_cancel(connection.subscription));
    }
    _connections.clear();
    _players = const [];
    _selected = null;
    if (_bus != null) unawaited(_close(_bus!));
    super.dispose();
  }
}

class _PlayerConnection {
  _PlayerConnection(this.name, this.owner, this.id, this.remote);
  final String name, owner;
  final int id;
  final DBusRemoteObject remote;
  StreamSubscription<DBusPropertiesChangedSignal>? subscription;
  Future<void>? readWork;
  bool dirty = false, busy = false;
  int revision = 0;
  Map<String, DBusValue> root = {}, values = {};
  MediaPlayer? snapshot;
}

bool _boolean(DBusValue? value) => value is DBusBoolean && value.value;
String _text(DBusValue? value) =>
    value is DBusString ? _clean(value.value, 512) : '';
String _clean(String value, int maximum) => value
    .substring(0, value.length.clamp(0, maximum))
    .replaceAll(RegExp(r'[\x00-\x1f\x7f]'), ' ')
    .trim();
Future<void> _cancel(StreamSubscription<dynamic>? subscription) async {
  try {
    await subscription?.cancel();
  } on Object {
    /* Already disconnected. */
  }
}

Future<void> _close(DBusClient bus) async {
  try {
    await bus.close();
  } on Object {
    /* Already disconnected. */
  }
}
