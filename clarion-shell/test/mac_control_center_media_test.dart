// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/services/media_service.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/theme/mac_shell_theme.dart';
import 'package:clarion_shell/ui/mac_control_center.dart';

MediaPlayer _player({
  String name = 'Fixture player',
  String title = 'Real title',
  String artist = 'Real artist',
  String suffix = 'fixture',
  int generation = 1,
  bool previous = false,
  bool next = true,
  bool pause = true,
  MediaPlaybackStatus status = MediaPlaybackStatus.playing,
}) => MediaPlayer(
  busName: 'org.mpris.MediaPlayer2.$suffix',
  owner: ':1.$generation',
  connectionId: generation,
  identity: name,
  title: title,
  artist: artist,
  status: status,
  canControl: true,
  canPlay: true,
  canPause: pause,
  canGoPrevious: previous,
  canGoNext: next,
);

class _Media extends MediaService {
  _Media() : super(enabled: false);
  List<MediaPlayer> current = const [];
  String? chosen;
  bool waiting = false;
  int starts = 0;
  bool closed = false;
  final commands = <(MediaPlayer, MediaCommand)>[];
  @override
  List<MediaPlayer> get players => current;
  @override
  MediaPlayer? get selected =>
      current.where((p) => p.busName == chosen).firstOrNull ??
      current.firstOrNull;
  @override
  bool get busy => waiting;
  @override
  Future<void> start() async {
    starts++;
  }

  void replace(List<MediaPlayer> players) {
    current = List.unmodifiable(players);
    notifyListeners();
  }

  bool _valid(MediaPlayer player) => current.any(
    (p) =>
        p.busName == player.busName &&
        p.owner == player.owner &&
        p.connectionId == player.connectionId,
  );
  @override
  bool selectPlayer(MediaPlayer player) {
    if (!_valid(player)) return false;
    chosen = player.busName;
    notifyListeners();
    return true;
  }

  @override
  Future<bool> invoke(MediaPlayer player, MediaCommand command) async {
    if (!_valid(player) || waiting || !selected!.allows(command)) return false;
    commands.add((player, command));
    return true;
  }

  @override
  void dispose() {
    closed = true;
    super.dispose();
  }
}

Finder _key(String key) => find.byKey(ValueKey(key));

void main() {
  late _Media media;
  late ShellController controller;
  setUp(() {
    media = _Media();
    controller = ShellController(demo: true, mediaService: media);
  });
  tearDown(() => controller.dispose());

  Future<void> mount(
    WidgetTester tester, {
    Size size = const Size(320, 900),
    double scale = 1,
  }) async {
    tester.view.physicalSize = size;
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(
      MaterialApp(
        theme: buildMacShellTheme(Brightness.light),
        builder: (context, child) => MediaQuery(
          data: MediaQuery.of(
            context,
          ).copyWith(textScaler: TextScaler.linear(scale)),
          child: child!,
        ),
        home: Scaffold(body: MacControlCenter(controller: controller)),
      ),
    );
    await tester.pumpAndSettle();
  }

  test(
    'controller construction and demo start never start a media connection',
    () async {
      expect(media.starts, 0);
      await controller.start();
      expect(media.starts, 0);
      expect(media.commands, isEmpty);
      final defaults = ShellController(demo: true);
      expect(defaults.media.enabled, isFalse);
      defaults.dispose();
    },
  );

  testWidgets('no player has no card or fake playback content', (tester) async {
    await mount(tester);
    expect(_key('mac-control-media'), findsNothing);
    expect(find.text('正在播放'), findsNothing);
    expect(media.starts, 0);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'live title artist and status update locally and disappear on exit',
    (tester) async {
      await mount(tester);
      media.replace([_player()]);
      await tester.pump();
      expect(find.text('Real title'), findsOneWidget);
      expect(find.text('Real artist'), findsOneWidget);
      expect(find.text('正在播放'), findsOneWidget);
      media.replace([
        _player(
          title: 'Changed title',
          artist: 'New artist',
          status: MediaPlaybackStatus.paused,
        ),
      ]);
      await tester.pump();
      expect(find.text('Changed title'), findsOneWidget);
      expect(find.text('已暂停'), findsOneWidget);
      expect(find.text('Real title'), findsNothing);
      media.replace([]);
      await tester.pump();
      expect(_key('mac-control-media'), findsNothing);
      expect(media.commands, isEmpty);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('button capabilities and busy state gate actual commands', (
    tester,
  ) async {
    media.replace([_player(pause: false)]);
    await mount(tester);
    expect(
      tester.widget<IconButton>(_key('mac-media-previous')).onPressed,
      isNull,
    );
    expect(
      tester.widget<IconButton>(_key('mac-media-playPause')).onPressed,
      isNull,
    );
    await tester.ensureVisible(_key('mac-media-next'));
    await tester.tap(_key('mac-media-next'));
    expect(media.commands.map((call) => call.$2), [MediaCommand.next]);
    media.waiting = true;
    media.replace([_player()]);
    await tester.pump();
    expect(tester.widget<IconButton>(_key('mac-media-next')).onPressed, isNull);
    media.waiting = false;
    media.replace([_player(status: MediaPlaybackStatus.paused)]);
    await tester.pump();
    await tester.tap(_key('mac-media-playPause'));
    expect(media.commands.last.$2, MediaCommand.playPause);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'selecting a real player changes target without playback side effects',
    (tester) async {
      media.replace([
        _player(name: 'Player A', suffix: 'a'),
        _player(name: 'Player B', suffix: 'b', generation: 2),
      ]);
      await mount(tester);
      await tester.ensureVisible(_key('mac-media-player-selector'));
      await tester.tap(_key('mac-media-player-selector'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Player B'));
      await tester.pumpAndSettle();
      expect(media.selected!.identity, 'Player B');
      expect(media.commands, isEmpty);
      await tester.ensureVisible(_key('mac-media-next'));
      await tester.tap(_key('mac-media-next'));
      expect(media.commands.single.$1.identity, 'Player B');
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('an old button callback cannot control a restarted player', (
    tester,
  ) async {
    media.replace([_player()]);
    await mount(tester);
    final old = tester.widget<IconButton>(_key('mac-media-next')).onPressed!;
    media.replace([_player(generation: 2)]);
    old();
    await tester.pump();
    expect(media.commands, isEmpty);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'long metadata remains usable on a short narrow output with large text',
    (tester) async {
      media.replace([
        _player(name: '播放器' * 200, title: '标题' * 200, artist: '作者' * 200),
      ]);
      await mount(tester, size: const Size(180, 180), scale: 2.5);
      await tester.ensureVisible(_key('mac-media-next'));
      await tester.pumpAndSettle();
      expect(
        tester.widget<Text>(_key('mac-media-title')).overflow,
        TextOverflow.ellipsis,
      );
      expect(tester.widget<Text>(_key('mac-media-artist')).maxLines, 1);
      final card = tester.getRect(_key('mac-control-media'));
      for (final command in MediaCommand.values) {
        final bounds = tester.getRect(_key('mac-media-${command.name}'));
        expect(bounds.left, greaterThanOrEqualTo(card.left));
        expect(bounds.right, lessThanOrEqualTo(card.right));
      }
      await tester.tap(_key('mac-media-next'));
      expect(media.commands.single.$2, MediaCommand.next);
      expect(tester.takeException(), isNull);
    },
  );
}
