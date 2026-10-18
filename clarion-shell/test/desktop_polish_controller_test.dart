import 'dart:async';
import 'dart:io';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/services/clarion_ipc.dart';
import 'package:clarion_shell/services/desktop_apps.dart';
import 'package:clarion_shell/services/notification_service.dart';
import 'package:clarion_shell/services/sound_service.dart';
import 'package:clarion_shell/services/system_service.dart';
import 'package:clarion_shell/services/wallpaper_service.dart';
import 'package:clarion_shell/state/shell_controller.dart';

class _Ipc extends ClarionIpc {
  final online = StreamController<bool>.broadcast(sync: true);
  final updates = StreamController<Map<String, dynamic>>.broadcast(sync: true);
  final actions = <(String, Map<String, Object?>)>[];
  bool supported = true;
  bool? motionChoice;
  Completer<void>? motionGate;
  void Function(Map<String, Object?>)? onMotionCommit;
  @override
  Stream<bool> get connections => online.stream;
  @override
  Stream<Map<String, dynamic>> get events => updates.stream;
  @override
  void start() {
    online.add(true);
    if (motionChoice != null) {
      updates.add({
        'ReducedMotionChanged': {'enabled': motionChoice},
      });
    }
  }

  @override
  Future<dynamic> request(Object request) async => request == 'Capabilities'
      ? {
          'Capabilities': [if (supported) 'runtime-reduced-motion'],
        }
      : request == 'ReducedMotionState'
      ? {'ReducedMotionState': motionChoice}
      : null;
  @override
  Future<void> action(
    String name, [
    Map<String, Object?> arguments = const {},
  ]) async {
    actions.add((name, Map.of(arguments)));
    if (name == 'SetReducedMotion') {
      await motionGate?.future;
      if (arguments['initialize_only'] != true || motionChoice == null) {
        motionChoice = arguments['enabled'] as bool;
      }
      onMotionCommit?.call(arguments);
    }
  }

  @override
  void dispose() {
    unawaited(online.close());
    unawaited(updates.close());
    super.dispose();
  }
}

class _System extends SystemService {
  final opened = <String>[];
  Completer<void>? connectGate;
  String? failure;
  @override
  Future<SystemSnapshot> read() async => const SystemSnapshot();
  @override
  Future<bool> hasSavedWifiConnection(String ssid) async => ssid == 'Saved';
  @override
  Future<void> connectWifi(String ssid, {String? password}) async {
    await connectGate?.future;
    if (failure case final message?) throw StateError(message);
  }

  @override
  Future<List<WifiNetwork>> scanWifiNetworks() async => [];
  @override
  Future<void> openLocation(String path) async => opened.add(path);
}

class _Apps extends DesktopApps {
  @override
  Future<List<DesktopApp>> load() async => [];
}

class _Wallpapers extends WallpaperService {
  @override
  Stream<void> get changes => const Stream.empty();
  @override
  Future<void> restore({bool applySaved = false}) async {}
}

class _Notifications extends NotificationService {
  @override
  Future<bool> start() async => available = true;
}

class _Sounds extends SoundService {
  final effects = <SoundEffect>[];
  @override
  Future<void> play(SoundEffect effect) async => effects.add(effect);
}

class _Fixture {
  _Fixture() {
    controller = ShellController(
      ipc: ipc,
      system: system,
      notificationService: notifications,
      soundService: sounds,
      desktopApps: _Apps(),
      wallpaperService: _Wallpapers(),
      tray: false,
      powerProfilesEnabled: false,
      mediaEnabled: false,
    );
    addTearDown(controller.dispose);
  }
  final ipc = _Ipc();
  final system = _System();
  final sounds = _Sounds();
  final notifications = _Notifications();
  late final ShellController controller;
}

Future<void> _drain() async {
  for (var i = 0; i < 6; i++) {
    await Future<void>.delayed(Duration.zero);
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  setUp(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
          const MethodChannel('clarion.shell/window'),
          (_) async => null,
        );
  });
  tearDown(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
          const MethodChannel('clarion.shell/window'),
          null,
        );
  });

  test(
    'a region request and cancellation are silent; only completion gives feedback',
    () async {
      final f = _Fixture();
      await f.controller.start();
      await f.controller.takeScreenshot(ScreenshotMode.region);
      expect(f.sounds.effects, isEmpty);
      expect(f.controller.lastScreenshotAt, isNull);
      f.controller.closePanel();
      expect(f.notifications.notifications, isEmpty);
      f.ipc.updates.add({
        'ScreenshotCaptured': {'path': null},
      });
      expect(f.sounds.effects, [SoundEffect.cameraShutter]);
      expect(f.controller.lastScreenshotAt, isNotNull);
      expect(f.notifications.notifications.single.summary, '截图已复制');
    },
  );

  test(
    'overview previews cannot play a shutter, create a notification, or replace the last capture',
    () async {
      final f = _Fixture();
      await f.controller.start();
      f.ipc.updates.add({
        'ScreenshotCaptured': {'path': '/tmp/user.png'},
      });
      final at = f.controller.lastScreenshotAt;
      f.ipc.updates.add({
        'ScreenshotCaptured': {'path': '/tmp/preview.png', 'is_preview': true},
      });
      expect(f.sounds.effects, [SoundEffect.cameraShutter]);
      expect(f.notifications.notifications, hasLength(1));
      expect(f.controller.lastScreenshotPath, '/tmp/user.png');
      expect(f.controller.lastScreenshotAt, same(at));
    },
  );

  test(
    'completed file actions use literal paths and missing files are retryable errors',
    () async {
      final f = _Fixture();
      final directory = await Directory.systemTemp.createTemp('clarion-shot-');
      addTearDown(() => directory.delete(recursive: true));
      final file = File('${directory.path}/name with \$ and spaces.png');
      await file.writeAsBytes([1, 2, 3]);
      await f.controller.start();
      f.ipc.updates.add({
        'ScreenshotCaptured': {'path': file.path},
      });
      await f.controller.openLastScreenshot();
      await f.controller.openLastScreenshot(containingFolder: true);
      expect(f.system.opened, [file.path, directory.path]);
      await file.delete();
      f.controller.selectPanel(ShellPanel.screenshot);
      await f.controller.openLastScreenshot();
      expect(f.controller.error, contains('已移动或删除'));
      f.controller.openWallpapers();
      expect(f.controller.error, isNull);
    },
  );

  test(
    'runtime motion is serialized, tracks platform preference, and restores after reconnect',
    () async {
      final f = _Fixture();
      await f.controller.start();
      await _drain();
      f.ipc.actions.clear();
      f.ipc.motionGate = Completer<void>();
      f.controller.setReduceMotion(true);
      await _drain();
      f.controller.setReduceMotion(false);
      f.ipc.motionGate!.complete();
      await _drain();
      expect(f.ipc.actions.map((action) => action.$1), [
        'SetReducedMotion',
        'SetReducedMotion',
      ]);
      expect(f.ipc.actions.map((action) => action.$2['enabled']), [
        true,
        false,
      ]);
      await f.controller.setDockMinimizeTargets({}, reduceMotion: true);
      await _drain();
      expect(f.ipc.actions.last.$1, 'SetReducedMotion');
      expect(f.ipc.actions.last.$2['enabled'], false);
      expect(f.ipc.actions.last.$2['platform'], true);
      f.ipc.online.add(false);
      f.ipc.online.add(true);
      await _drain();
      expect(f.ipc.actions.last.$2['enabled'], false);
      expect(f.ipc.actions.last.$2['platform'], true);
    },
  );

  test(
    'old cores keep Shell motion preferences without unsupported actions',
    () async {
      final f = _Fixture();
      f.ipc.supported = false;
      await f.controller.start();
      f.controller.setReduceMotion(true);
      await _drain();
      expect(f.controller.reduceMotion, true);
      expect(f.ipc.actions, isEmpty);
      expect(f.controller.error, isNull);
    },
  );

  test(
    'a reconnected screen adopts the global choice without overwriting it',
    () async {
      final f = _Fixture();
      f.ipc.motionChoice = true;
      await f.controller.start();
      await _drain();
      expect(f.controller.reduceMotion, true);
      expect(f.ipc.motionChoice, true);
      expect(
        f.ipc.actions.every((action) => action.$2['enabled'] == true),
        true,
      );
      expect(
        f.ipc.actions.every((action) => action.$2['initialize_only'] == true),
        true,
      );
    },
  );

  test(
    'a newer remote choice during a local write is reconciled after its acknowledgement',
    () async {
      final f = _Fixture();
      await f.controller.start();
      await _drain();
      f.ipc.onMotionCommit = (arguments) {
        f.ipc.onMotionCommit = null;
        f.ipc.motionChoice = false;
        f.ipc.updates.add({
          'ReducedMotionChanged': {'enabled': false},
        });
      };
      f.controller.setReduceMotion(true);
      await _drain();
      expect(f.controller.reduceMotion, false);
      expect(f.ipc.motionChoice, false);
    },
  );

  test(
    'an initial user-choice snapshot cannot acknowledge an unsent platform setting',
    () async {
      final f = _Fixture();
      f.ipc.motionChoice = false;
      f.controller.setPlatformReducedMotion(true);
      await f.controller.start();
      await _drain();
      expect(f.controller.reduceMotion, false);
      expect(
        f.ipc.actions.any((action) => action.$2['platform'] == true),
        true,
      );
      f.controller.setPlatformReducedMotion(false);
      await _drain();
      expect(f.ipc.actions.last.$2['platform'], false);
      expect(f.controller.reduceMotion, false);
    },
  );

  test('Wi-Fi retries keep an exact target and an independent error', () async {
    final f = _Fixture();
    expect(await f.controller.hasSavedWifiConnection('Saved'), true);
    f.system.connectGate = Completer<void>();
    f.system.failure = 'authentication failed';
    final work = f.controller.connectWifi('Office');
    expect(f.controller.wifiConnectingSsid, 'Office');
    expect(await f.controller.connectWifi('Other'), false);
    f.system.connectGate!.complete();
    expect(await work, false);
    expect(f.controller.wifiConnectingSsid, isNull);
    expect(f.controller.wifiConnectionError, contains('authentication failed'));
    expect(f.controller.error, isNull);
    f.system.failure = null;
    expect(await f.controller.connectWifi('Office'), true);
    expect(f.controller.wifiConnectionError, isNull);
  });
  test(
    'the Material notification list marks messages read without discarding history',
    () async {
      final f = _Fixture();
      await f.controller.start();
      f.notifications.showScreenshot(null, openLocation: (_) async {});
      expect(f.controller.unreadNotificationCount, 1);
      f.controller.selectPanel(ShellPanel.notifications);
      expect(f.controller.unreadNotificationCount, 0);
      expect(f.notifications.notifications, hasLength(1));
    },
  );
}
