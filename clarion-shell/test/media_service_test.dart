// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:io';

import 'package:dbus/dbus.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/services/media_service.dart';

const _root = 'org.mpris.MediaPlayer2';
const _player = 'org.mpris.MediaPlayer2.Player';

void main() {
  late _Fixture fixture;
  setUp(() async => fixture = await _Fixture.create());
  tearDown(() => fixture.close());

  test(
    'construction is inert and discovery only observes live MPRIS players',
    () async {
      final (_, provider) = await fixture.player('fixture');
      final media = fixture.service();
      expect(media.players, isEmpty);
      expect(provider.reads, 0);
      expect(fixture.observed.single.uniqueName, isEmpty);
      await media.start();
      expect(media.players, hasLength(1));
      expect(media.selected!.identity, 'Fixture');
      expect(media.selected!.title, 'Actual title');
      expect(media.selected!.artist, 'First、Second');
      expect(provider.commands, isEmpty);
      expect(
        fixture.observed.single.sent.where((call) => call.interface == _player),
        isEmpty,
      );
      expect(() => media.players.clear(), throwsUnsupportedError);
    },
  );

  test(
    'disabled service does not connect even when explicitly started',
    () async {
      final media = fixture.service(enabled: false);
      await media.start();
      expect(fixture.observed.single.uniqueName, isEmpty);
      expect(media.players, isEmpty);
    },
  );

  test(
    'property events update snapshots without polling or unrelated rebuilds',
    () async {
      final (_, provider) = await fixture.player('fixture');
      final media = fixture.service();
      await media.start();
      final originalList = media.players;
      var notifications = 0;
      media.addListener(() => notifications++);
      final reads = provider.reads;
      await provider.emitPropertiesChanged(
        _player,
        changedProperties: {'Position': const DBusInt64(4000)},
      );
      await provider.changed({'PlaybackStatus': const DBusString('Playing')});
      await _wait(() => media.selected?.status == MediaPlaybackStatus.playing);
      expect(identical(media.players, originalList), isFalse);
      expect(provider.reads, reads);
      expect(notifications, 1);
      final stable = media.players;
      await provider.changed({'PlaybackStatus': const DBusString('Playing')});
      await Future<void>.delayed(const Duration(milliseconds: 30));
      expect(identical(media.players, stable), isTrue);
      expect(notifications, 1);
    },
  );

  test('controls use unique owners and actual Can flags', () async {
    final (owner, provider) = await fixture.player('fixture');
    final media = fixture.service();
    await media.start();
    var player = media.selected!;
    expect(await media.invoke(player, MediaCommand.previous), isFalse);
    expect(await media.invoke(player, MediaCommand.next), isTrue);
    expect(await media.invoke(player, MediaCommand.playPause), isTrue);
    await _wait(() => media.selected?.status == MediaPlaybackStatus.playing);
    player = media.selected!;
    await provider.changed({'CanPause': const DBusBoolean(false)});
    await _wait(() => media.selected?.canPause == false);
    expect(await media.invoke(player, MediaCommand.playPause), isFalse);
    await provider.changed({'CanControl': const DBusBoolean(false)});
    await _wait(() => media.selected?.canControl == false);
    expect(await media.invoke(player, MediaCommand.next), isFalse);
    expect(provider.commands, ['Next', 'PlayPause']);
    final commands = fixture.observed.single.sent.where(
      (call) => call.interface == _player,
    );
    expect(
      commands.every(
        (call) => call.destination == owner.uniqueName && call.noAutoStart,
      ),
      isTrue,
    );
  });

  test(
    'owner signals discover arrivals and remove departures without refresh polling',
    () async {
      final media = fixture.service();
      await media.start();
      expect(media.selected, isNull);
      final (owner, _) = await fixture.player('arriving');
      await _wait(() => media.players.length == 1);
      await owner.releaseName('$_root.arriving');
      await _wait(() => media.players.isEmpty);
      expect(media.selected, isNull);
    },
  );

  test(
    'player restart rejects a captured old target and late old-owner signals',
    () async {
      final (owner, previous) = await fixture.player('fixture');
      final media = fixture.service();
      await media.start();
      final captured = media.selected!;
      await owner.releaseName('$_root.fixture');
      await _wait(() => media.players.isEmpty);
      final (_, replacement) = await fixture.player(
        'fixture',
        identity: 'Replacement',
      );
      await _wait(() => media.selected?.identity == 'Replacement');
      await previous.changed({'PlaybackStatus': const DBusString('Playing')});
      expect(await media.invoke(captured, MediaCommand.next), isFalse);
      expect(media.selectPlayer(captured), isFalse);
      expect(replacement.commands, isEmpty);
      expect(media.selected!.owner, isNot(captured.owner));
      expect(media.selected!.status, MediaPlaybackStatus.paused);
    },
  );

  test(
    'same unique owner reacquiring its name gets a new connection generation',
    () async {
      final (owner, _) = await fixture.player('fixture');
      final media = fixture.service();
      await media.start();
      final old = media.selected!;
      await owner.releaseName('$_root.fixture');
      await _wait(() => media.players.isEmpty);
      await owner.requestName('$_root.fixture');
      await _wait(() => media.selected != null);
      expect(media.selected!.owner, old.owner);
      expect(media.selected!.connectionId, isNot(old.connectionId));
      expect(await media.invoke(old, MediaCommand.next), isFalse);
    },
  );

  test(
    'signals during initial GetAll cannot be replaced by an older reply',
    () async {
      final (_, provider) = await fixture.player('fixture');
      provider.readGate = Completer<void>();
      final media = fixture.service();
      final started = media.start();
      await _wait(() => provider.reads > 0);
      await provider.changed({'PlaybackStatus': const DBusString('Playing')});
      // Let the signal subscription consume this event while GetAll is held.
      await Future<void>.delayed(const Duration(milliseconds: 20));
      provider.readGate!.complete();
      await started;
      await _wait(() => media.selected?.status == MediaPlaybackStatus.playing);
      expect(provider.commands, isEmpty);
    },
  );

  test(
    'invalidated capabilities disable immediately and refresh once',
    () async {
      final (_, provider) = await fixture.player('fixture');
      final media = fixture.service();
      await media.start();
      provider.readGate = Completer<void>();
      provider.values['CanGoNext'] = const DBusBoolean(false);
      await provider.emitPropertiesChanged(
        _player,
        invalidatedProperties: ['CanGoNext'],
      );
      await _wait(() => media.selected?.canGoNext == false);
      expect(await media.invoke(media.selected!, MediaCommand.next), isFalse);
      provider.readGate!.complete();
      await _wait(() => provider.reads >= 4);
      expect(media.selected!.canGoNext, isFalse);
    },
  );

  test(
    'busy guard and delayed old-player error cannot affect its replacement',
    () async {
      final (owner, provider) = await fixture.player('fixture');
      final media = fixture.service();
      await media.start();
      final old = media.selected!;
      provider.actionGate = Completer<void>();
      provider.deny = true;
      final pending = media.invoke(old, MediaCommand.next);
      await _wait(() => provider.commands.isNotEmpty);
      expect(media.busy, isTrue);
      expect(await media.invoke(old, MediaCommand.next), isFalse);
      await owner.releaseName('$_root.fixture');
      await _wait(() => media.players.isEmpty);
      await fixture.player('fixture', identity: 'New');
      await _wait(() => media.selected?.identity == 'New');
      provider.actionGate!.complete();
      expect(await pending, isFalse);
      expect(media.busy, isFalse);
      expect(media.error, isNull);
    },
  );

  test('player selection is explicit and cannot start playback', () async {
    final (_, first) = await fixture.player('a', identity: 'A');
    first.values['PlaybackStatus'] = const DBusString('Playing');
    final (_, second) = await fixture.player('b', identity: 'B');
    final media = fixture.service();
    await media.start();
    expect(media.selected!.identity, 'A');
    expect(media.selectPlayer(media.players.last), isTrue);
    expect(media.selected!.identity, 'B');
    await first.changed({
      'Metadata': DBusDict.stringVariant({
        'xesam:title': const DBusString('Next title'),
      }),
    });
    await _wait(() => media.players.first.title == 'Next title');
    expect(media.selected!.identity, 'B');
    expect(first.commands, isEmpty);
    expect(second.commands, isEmpty);
  });

  test(
    'a bounded action timeout releases busy without retrying playback',
    () async {
      final (_, provider) = await fixture.player('fixture');
      final media = fixture.service(timeout: const Duration(milliseconds: 80));
      await media.start();
      provider.actionGate = Completer<void>();
      expect(
        await media.invoke(media.selected!, MediaCommand.playPause),
        isFalse,
      );
      expect(media.busy, isFalse);
      expect(media.error, isNotNull);
      expect(provider.commands, ['PlayPause']);
      provider.actionGate!.complete();
      await _wait(() => media.selected?.status == MediaPlaybackStatus.playing);
      expect(provider.commands, ['PlayPause']);
    },
  );

  test(
    'an old initial read cannot resurrect a player after replacement',
    () async {
      final (owner, provider) = await fixture.player('fixture');
      provider.readGate = Completer<void>();
      final media = fixture.service();
      final pending = media.start();
      await _wait(() => provider.reads > 0);
      await owner.releaseName('$_root.fixture');
      await fixture.player('fixture', identity: 'Replacement');
      await _wait(() => media.selected?.identity == 'Replacement');
      provider.readGate!.complete();
      await pending;
      expect(media.players, hasLength(1));
      expect(media.selected!.identity, 'Replacement');
    },
  );

  test('snapshot count and metadata text remain bounded', () async {
    final (_, first) = await fixture.player('a', identity: 'X' * 1000);
    first.values['Metadata'] = DBusDict.stringVariant({
      'xesam:title': DBusString('T' * 2000),
      'xesam:artist': DBusArray.string(List.generate(30, (_) => 'A' * 1000)),
    });
    await fixture.player('b');
    await fixture.player('c');
    final media = fixture.service(maximumPlayers: 2);
    await media.start();
    expect(media.players, hasLength(2));
    final bounded = media.players.firstWhere(
      (player) => player.busName == '$_root.a',
    );
    expect(bounded.identity.length, lessThanOrEqualTo(512));
    expect(bounded.title.length, lessThanOrEqualTo(512));
    expect(bounded.artist.length, lessThanOrEqualTo(8 * 128 + 7));
  });

  test(
    'dispose during a read rejects late replies and stops notifications',
    () async {
      final (_, provider) = await fixture.player('fixture');
      provider.readGate = Completer<void>();
      final media = fixture.service();
      var changes = 0;
      media.addListener(() => changes++);
      final started = media.start();
      await _wait(() => provider.reads > 0);
      media.dispose();
      final before = changes;
      provider.readGate!.complete();
      await started;
      expect(media.players, isEmpty);
      expect(changes, before);
      await media.start();
      expect(media.players, isEmpty);
    },
  );
}

Future<void> _wait(bool Function() condition) async {
  final deadline = DateTime.now().add(const Duration(seconds: 3));
  while (!condition()) {
    if (DateTime.now().isAfter(deadline)) {
      throw TimeoutException('MPRIS fixture');
    }
    await Future<void>.delayed(const Duration(milliseconds: 5));
  }
}

class _Fixture {
  _Fixture(this.directory, this.server, this.address);
  final Directory directory;
  final DBusServer server;
  final DBusAddress address;
  final clients = <DBusClient>[];
  final providers = <_Provider>[];
  final services = <MediaService>[];
  final observed = <_ObservedBus>[];
  static Future<_Fixture> create() async {
    final directory = await Directory.systemTemp.createTemp('clarion-mpris-');
    final server = DBusServer();
    final address = await server.listenAddress(
      DBusAddress.unix(path: '${directory.path}/bus'),
    );
    return _Fixture(directory, server, address);
  }

  MediaService service({
    int maximumPlayers = 16,
    bool enabled = true,
    Duration? timeout,
  }) {
    final bus = _ObservedBus(address);
    observed.add(bus);
    clients.add(bus);
    final service = MediaService(
      bus: bus,
      enabled: enabled,
      maximumPlayers: maximumPlayers,
      requestTimeout: timeout ?? const Duration(seconds: 1),
    );
    services.add(service);
    return service;
  }

  Future<(DBusClient, _Provider)> player(
    String suffix, {
    String identity = 'Fixture',
  }) async {
    final client = DBusClient(address);
    clients.add(client);
    final provider = _Provider(identity);
    providers.add(provider);
    await client.registerObject(provider);
    await client.requestName('$_root.$suffix');
    return (client, provider);
  }

  Future<void> close() async {
    for (final provider in providers) {
      for (final gate in [provider.readGate, provider.actionGate]) {
        if (gate != null && !gate.isCompleted) gate.complete();
      }
    }
    for (final service in services) {
      service.dispose();
    }
    for (final client in clients) {
      await client.close();
    }
    await server.close();
    await directory.delete(recursive: true);
  }
}

class _ObservedBus extends DBusClient {
  _ObservedBus(super.address);
  final sent =
      <
        ({
          String? destination,
          String? interface,
          String name,
          bool noAutoStart,
        })
      >[];
  @override
  Future<DBusMethodSuccessResponse> callMethod({
    String? destination,
    required DBusObjectPath path,
    String? interface,
    required String name,
    Iterable<DBusValue> values = const [],
    DBusSignature? replySignature,
    bool noReplyExpected = false,
    bool noAutoStart = false,
    bool allowInteractiveAuthorization = false,
  }) {
    sent.add((
      destination: destination,
      interface: interface,
      name: name,
      noAutoStart: noAutoStart,
    ));
    return super.callMethod(
      destination: destination,
      path: path,
      interface: interface,
      name: name,
      values: values,
      replySignature: replySignature,
      noReplyExpected: noReplyExpected,
      noAutoStart: noAutoStart,
      allowInteractiveAuthorization: allowInteractiveAuthorization,
    );
  }
}

class _Provider extends DBusObject {
  _Provider(this.identity) : super(DBusObjectPath('/org/mpris/MediaPlayer2'));
  final String identity;
  int reads = 0;
  bool deny = false;
  Completer<void>? readGate, actionGate;
  final commands = <String>[];
  final values = <String, DBusValue>{
    'PlaybackStatus': const DBusString('Paused'),
    'CanControl': const DBusBoolean(true),
    'CanPlay': const DBusBoolean(true),
    'CanPause': const DBusBoolean(true),
    'CanGoPrevious': const DBusBoolean(false),
    'CanGoNext': const DBusBoolean(true),
    'Metadata': DBusDict.stringVariant({
      'xesam:title': const DBusString('Actual title'),
      'xesam:artist': DBusArray.string(['First', 'Second']),
    }),
  };
  Future<void> changed(Map<String, DBusValue> delta) async {
    values.addAll(delta);
    await emitPropertiesChanged(_player, changedProperties: delta);
  }

  @override
  Future<DBusMethodResponse> getAllProperties(String interface) async {
    reads++;
    final response = interface == _root
        ? DBusGetAllPropertiesResponse({'Identity': DBusString(identity)})
        : interface == _player
        ? DBusGetAllPropertiesResponse(Map.of(values))
        : DBusMethodErrorResponse.unknownInterface();
    await readGate?.future;
    return response;
  }

  @override
  Future<DBusMethodResponse> handleMethodCall(DBusMethodCall call) async {
    if (call.interface != _player ||
        !['PlayPause', 'Previous', 'Next'].contains(call.name)) {
      return DBusMethodErrorResponse.unknownMethod();
    }
    commands.add(call.name);
    await actionGate?.future;
    if (deny) {
      return DBusMethodErrorResponse('org.freedesktop.DBus.Error.Failed');
    }
    if (call.name == 'PlayPause') {
      await changed({
        'PlaybackStatus': DBusString(
          values['PlaybackStatus'] == const DBusString('Playing')
              ? 'Paused'
              : 'Playing',
        ),
      });
    }
    return DBusMethodSuccessResponse();
  }
}
