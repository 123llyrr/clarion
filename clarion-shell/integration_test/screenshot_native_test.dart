// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:convert';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:clarion_shell/mac_shell_preferences.dart';
import 'package:clarion_shell/main.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/preferences.dart';
import 'package:clarion_shell/services/desktop_apps.dart';
import 'package:clarion_shell/services/notification_service.dart';
import 'package:clarion_shell/services/clarion_ipc.dart';
import 'package:clarion_shell/services/sound_service.dart';
import 'package:clarion_shell/services/system_service.dart';
import 'package:clarion_shell/services/wallpaper_service.dart';
import 'package:clarion_shell/services/window_decoration_service.dart';
import 'package:clarion_shell/state/shell_controller.dart';

const _window = MethodChannel('clarion.shell/window');

Future<Map<String, dynamic>> _nativeState() async => Map<String, dynamic>.from(
  (await _window.invokeMapMethod<String, dynamic>('windowState'))!,
);

Future<void> _wait(
  WidgetTester tester,
  bool Function() ready,
  String reason,
) async {
  final deadline = DateTime.now().add(const Duration(seconds: 15));
  while (!ready()) {
    if (DateTime.now().isAfter(deadline)) fail('Timed out: $reason');
    await tester.pump(const Duration(milliseconds: 16));
  }
}

Future<Map<String, dynamic>> _waitNative(
  WidgetTester tester, {
  required bool expanded,
  required ShellStyle style,
}) async {
  final deadline = DateTime.now().add(const Duration(seconds: 15));
  Map<String, dynamic> state = const {};
  do {
    await tester.pump(const Duration(milliseconds: 16));
    state = await _nativeState();
    if (state['expanded'] == expanded &&
        state['tooltipVisible'] == false &&
        state['shellStyle'] == style.name &&
        state['keyboard'] == (expanded ? 'Exclusive' : 'None')) {
      return state;
    }
  } while (DateTime.now().isBefore(deadline));
  fail('Native modal state mismatch: $state');
}

class _Apps extends DesktopApps {
  @override
  Future<List<DesktopApp>> load() async => const [];
}

class _System extends SystemService {
  _System()
    : super(
        command: (program, arguments) async {
          throw StateError('Native screenshot fixture forbids system writes');
        },
      );

  @override
  Future<SystemSnapshot> read() async => const SystemSnapshot(
    volume: .5,
    brightness: .5,
    batteryFraction: .8,
    wifiEnabled: true,
    bluetoothEnabled: true,
  );
}

// The actual private IPC connection is retained. Recording at the action
// boundary proves preparation completed before the production request was sent.
class _Ipc extends ClarionIpc {
  final captures = <Map<String, dynamic>>[];

  @override
  Future<void> action(
    String name, [
    Map<String, Object?> arguments = const {},
  ]) async {
    if (name != 'ScreenshotScreen' && name != 'SetMinimizeTarget') {
      throw StateError('Unexpected fixture compositor action: $name');
    }
    if (name == 'ScreenshotScreen') {
      expect(
        socketPath,
        isNot(Platform.environment['CLARION_SHELL_PARENT_CLARION_SOCKET']),
      );
      captures.add({
        'name': name,
        'socketAtSend': socketPath,
        'arguments': Map<String, Object?>.from(arguments),
        'nativeAtSend': await _nativeState(),
      });
    }
    await super.action(name, arguments);
  }
}

// Flutter's live test binding assumes every native view already has a mounted
// RenderView. Filter packets only for views being attached/detached;
// this fixture uses tester gestures and claims no physical input validation.
class _DeviceFilter {
  _DeviceFilter(this.binding) {
    original = binding.platformDispatcher.onPointerDataPacket;
    binding.platformDispatcher.onPointerDataPacket = handle;
  }
  final IntegrationTestWidgetsFlutterBinding binding;
  ui.PointerDataPacketCallback? original;
  int ignored = 0;
  void handle(ui.PointerDataPacket packet) {
    final attached = binding.renderViews
        .map((view) => view.flutterView.viewId)
        .toSet();
    final valid = <ui.PointerData>[];
    for (final data in packet.data) {
      if (attached.contains(data.viewId)) {
        valid.add(data);
      } else {
        ignored++;
      }
    }
    if (valid.isNotEmpty) original?.call(ui.PointerDataPacket(data: valid));
  }

  void restore() => binding.platformDispatcher.onPointerDataPacket = original;
}

Future<Map<String, dynamic>> _pixels(File file, Map output) async {
  final codec = await ui.instantiateImageCodec(await file.readAsBytes());
  final image = (await codec.getNextFrame()).image;
  try {
    final mode =
        (output['modes'] as List)[output['current_mode'] as int] as Map;
    expect(image.width, mode['width']);
    expect(image.height, mode['height']);
    final bytes = (await image.toByteData(format: ui.ImageByteFormat.rawRgba))!;
    final center = ((image.height ~/ 2) * image.width + image.width ~/ 2) * 4;
    final rgba = [for (var i = 0; i < 4; i++) bytes.getUint8(center + i)];
    // The harness configures the isolated compositor background to #204060.
    // A retained screenshot panel would cover this exact center desktop pixel.
    for (var i = 0; i < 4; i++) {
      expect(rgba[i], closeTo([32, 64, 96, 255][i], 2));
    }
    return {
      'width': image.width,
      'height': image.height,
      'centerRgba': rgba,
      'pngBytes': await file.length(),
    };
  } finally {
    image.dispose();
    codec.dispose();
  }
}

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.onlyPumps;
  testWidgets('native full-output collapse saves screen for both shell styles', (
    tester,
  ) async {
    final env = Platform.environment;
    expect(env['CLARION_SHELL_ISOLATED_TEST'], '1');
    for (final key in [
      'WAYLAND_DISPLAY',
      'CLARION_SOCKET',
      'DBUS_SESSION_BUS_ADDRESS',
    ]) {
      expect(env[key], isNotEmpty, reason: key);
      expect(env[key], isNot(env['CLARION_SHELL_PARENT_$key']), reason: key);
    }
    final root = env['CLARION_SHELL_TEST_ROOT']!;
    final saved = File(env['CLARION_SHELL_SAVED_SCREEN']!);
    expect(saved.absolute.path.startsWith('$root/'), isTrue);
    final ipc = _Ipc();
    final events = <Map<String, dynamic>>[];
    final eventKeys = <List<String>>[];
    final screenshotCandidateEvents = <Map<String, dynamic>>[];
    final connections = <bool>[];
    final connectionSubscription = ipc.connections.listen(connections.add);
    final eventSubscription = ipc.events.listen((event) {
      eventKeys.add(event.keys.toList());
      if (event.keys.any((key) => key.toLowerCase().contains('screen'))) {
        screenshotCandidateEvents.add(Map<String, dynamic>.from(event));
      }
      if (event['ScreenshotCaptured'] case final Map value) {
        events.add(Map<String, dynamic>.from(value));
      }
    });
    // Completion is sent after the compositor calls Notify. Keep that real
    // protocol exchange on the private bus instead of activating a host daemon.
    // This standalone service does not attach notification widgets to the shell.
    final notifications = NotificationService();
    final filter = _DeviceFilter(binding);
    final controller = ShellController(
      ipc: ipc,
      notifications: false,
      tray: false,
      powerProfilesEnabled: false,
      mediaEnabled: false,
      desktopApps: _Apps(),
      system: _System(),
      soundService: SoundService(enabled: false),
      windowDecorationService: WindowDecorationService(enabled: false),

      wallpaperService: WallpaperService(
        directoryPath: '$root/wallpapers',
        preferenceFile: File('$root/wallpaper.json'),
      ),
      restoreWallpaperOnStart: false,
    );
    controller.setReduceMotion(true);
    final phases = <Map<String, dynamic>>[];
    final evidence = <String, dynamic>{
      'passed': false,
      'input': 'WidgetTester tap on screenshot-screen; no physical input sent',
      'ipc': 'Production ClarionIpc to private nested compositor',
      'rendererBoundary':
          'Nested winit desktop, not live KMS screenshot coverage',
      'phases': phases,
    };
    try {
      expect(await notifications.start(), isTrue);
      await tester.pumpWidget(
        ShellHost(
          controller: controller,
          preferences: const ShellPreferences(reduceMotion: true),
          persist: false,
        ),
        wrapWithView: false,
      );
      await controller.start();
      await _wait(tester, () => controller.connected, 'private IPC connects');
      evidence['initialEventKeys'] = List<List<String>>.from(eventKeys);
      for (final style in [ShellStyle.material, ShellStyle.macos]) {
        controller.setMacPreferences(
          const MacShellPreferences(menuBarHeight: 32),
        );
        controller.setShellStyle(style);
        final folded = await _waitNative(tester, expanded: false, style: style);
        final barHeight = style == ShellStyle.macos ? 32 : 40;
        expect(folded['height'], greaterThan(barHeight));
        expect(folded['height'], (folded['monitorGeometry'] as Map)['height']);
        expect(folded['inputHeight'], barHeight);
        if (await saved.exists()) await saved.delete();
        controller.selectPanel(ShellPanel.screenshot);
        final opened = await _waitNative(tester, expanded: true, style: style);
        final button = find.byKey(const ValueKey('screenshot-screen'));
        expect(button, findsOneWidget);
        await tester.tap(button);
        await _wait(
          tester,
          () =>
              !controller.busy.contains('screenshot') &&
              ipc.captures.length == phases.length + 1,
          'production screenshot action after native collapse (${style.name})',
        );
        expect(controller.error, isNull);
        expect(controller.panel, ShellPanel.none);
        final send = ipc.captures.last;
        final native = send['nativeAtSend'] as Map;
        expect(native['expanded'], isFalse);
        expect(native['tooltipVisible'], isFalse);
        expect(native['keyboard'], 'None');
        expect(native['height'], greaterThan(barHeight));
        expect(native['inputHeight'], barHeight);
        expect(send['arguments'], {
          'show_pointer': false,
          'path': null,
          'write_to_disk': true,
        });
        await _wait(
          tester,
          () => saved.existsSync() && events.length == phases.length + 1,
          'PNG and ScreenshotCaptured completion (${style.name})',
        );
        expect(events.last['path'], saved.path);
        expect(notifications.notifications.length, phases.length + 1);
        final outputs = (await ipc.request('Outputs') as Map)['Outputs'] as Map;
        final pixels = await tester.runAsync(
          () => _pixels(saved, outputs['winit'] as Map),
        );
        final copy = '$root/${style.name}-screen.png';
        await saved.copy(copy);
        phases.add({
          'style': style.name,
          'barHeight': barHeight,
          'foldedBefore': folded,
          'openedBeforeTap': opened,
          'action': send,
          'completion': events.last,
          'pixels': pixels,
          'capture': copy,
        });
      }
      expect(phases.length, 2);
      evidence['passed'] = true;
    } finally {
      evidence['ignoredUnattachedDevicePackets'] = filter.ignored;
      evidence['capturedActions'] = ipc.captures;
      evidence['completionEvents'] = events;
      evidence['allEventKeys'] = eventKeys;
      evidence['screenshotCandidateEvents'] = screenshotCandidateEvents;
      evidence['ipcConnections'] = connections;
      evidence['privateNotificationCount'] = notifications.notifications.length;
      await File('$root/screenshot-native-evidence.json').writeAsString(
        '${const JsonEncoder.withIndent('  ').convert(evidence)}\n',
      );
      await eventSubscription.cancel();
      await connectionSubscription.cancel();
      await tester.pumpWidget(
        const ViewCollection(views: []),
        wrapWithView: false,
      );
      notifications.dispose();
      filter.restore();
    }
  });
}
