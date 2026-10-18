// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:convert';
import 'dart:io';

import 'package:dbus/dbus.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:clarion_shell/main.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/preferences.dart';
import 'package:clarion_shell/services/tray_service.dart';
import 'package:clarion_shell/state/shell_controller.dart';

const _window = MethodChannel('clarion.shell/window');
final _env = Platform.environment;
final _deviceEvents = <Map<String, dynamic>>[];
final _inputCommands = <Map<String, dynamic>>[];
Offset? _lastDevicePosition;
bool get _pointerPhase => _env['CLARION_SHELL_INPUT_PHASE'] == 'pointer';

void _guard() {
  expect(_env['CLARION_SHELL_ISOLATED_TEST'], '1');
  final root = _env['CLARION_SHELL_TEST_ROOT']!;
  final runtime = _env['XDG_RUNTIME_DIR']!;
  expect(runtime, _env['CLARION_SHELL_PRIVATE_RUNTIME']);
  expect(runtime, startsWith('/tmp/clarion-tray-'));
  expect(File('$runtime/test-owner-root').readAsStringSync(), root);
  expect(_env['CLARION_SOCKET'], startsWith('$runtime/'));
  expect(_env['CLARION_SOCKET'], isNot(_env['CLARION_SHELL_PARENT_SOCKET']));
  expect(
    _env['WAYLAND_DISPLAY'],
    isNot(_env['CLARION_SHELL_PARENT_WAYLAND_DISPLAY']),
  );
  final display = _env['WAYLAND_DISPLAY']!;
  expect(
    display.startsWith('/')
        ? display.startsWith('$runtime/')
        : !display.contains('/'),
    true,
  );
  expect(_env['DBUS_SESSION_BUS_ADDRESS'], startsWith('unix:path=$runtime/'));
  expect(
    _env['DBUS_SESSION_BUS_ADDRESS'],
    isNot(_env['CLARION_SHELL_PARENT_DBUS_ADDRESS']),
  );
  for (final name in [
    'XDG_CONFIG_HOME',
    'XDG_STATE_HOME',
    'XDG_CACHE_HOME',
    'XDG_DATA_HOME',
  ]) {
    expect(_env[name], startsWith('$root/'));
  }
}

Future<Map<String, dynamic>> _state() async => Map<String, dynamic>.from(
  (await _window.invokeMapMethod<String, dynamic>('windowState'))!,
);

Future<void> _wait(
  WidgetTester tester,
  bool Function() ready,
  String label,
) async {
  final deadline = DateTime.now().add(const Duration(seconds: 12));
  while (!ready()) {
    if (DateTime.now().isAfter(deadline)) {
      fail('Timed out: $label; native=${await _state()}');
    }
    await tester.pump(const Duration(milliseconds: 60));
  }
  await tester.pump();
}

Future<Map<String, dynamic>> _waitState(
  WidgetTester tester,
  bool Function(Map<String, dynamic>) ready,
  String label,
) async {
  final deadline = DateTime.now().add(const Duration(seconds: 12));
  Map<String, dynamic> state;
  do {
    await tester.pump(const Duration(milliseconds: 60));
    state = await _state();
    if (ready(state)) return state;
  } while (DateTime.now().isBefore(deadline));
  fail('Timed out: $label; native=$state');
}

Future<Map<String, dynamic>> _menu(WidgetTester tester, bool open) =>
    _waitState(
      tester,
      (state) =>
          state['nativeTrayMenuOpen'] == open &&
          (!open || (state['nativeTrayMenuVisibleItems'] as int) > 0),
      'GTK menu open=$open',
    );

Finder _button(ShellController controller, String title) => find.byKey(
  ValueKey(
    'tray-${controller.trayItems.firstWhere((item) => item.title == title).id}',
  ),
);

Future<void> _click(
  WidgetTester tester,
  Finder target, {
  int button = 273,
}) async {
  _guard();
  final point = tester.getCenter(target);
  await _pointer(tester, point, button: button);
}

Future<void> _pointer(
  WidgetTester tester,
  Offset point, {
  int button = 272,
  bool hover = false,
}) async {
  _guard();
  if (_env['CLARION_SHELL_KEYBOARD_BACKEND'] == 'x11') {
    await _focusPrivateX();
    final window = _env['CLARION_SHELL_PRIVATE_XWINDOW']!;
    final geometryResult = await _xdo(['getwindowgeometry', '--shell', window]);
    expect(geometryResult.exitCode, 0);
    final geometry = <String, String>{
      for (final line in (geometryResult.stdout as String).split('\n'))
        if (line.contains('=')) line.split('=').first: line.split('=').last,
    };
    final output = (await _state())['monitorGeometry'] as Map;
    final x =
        (point.dx * int.parse(geometry['WIDTH']!) / (output['width'] as num))
            .round();
    final y =
        (point.dy * int.parse(geometry['HEIGHT']!) / (output['height'] as num))
            .round();
    final globalX = x + int.parse(geometry['X']!);
    final globalY = y + int.parse(geometry['Y']!);
    // A single XTEST absolute warp can update X's cursor before winit has
    // delivered the corresponding Wayland motion. Approach then finish with
    // a separate genuine device motion before pressing a button.
    for (final dx in [-2, 0]) {
      final moved = await _xdo(['mousemove', '${globalX + dx}', '$globalY']);
      expect(moved.exitCode, 0, reason: '${moved.stderr}');
      await tester.pump(const Duration(milliseconds: 100));
    }
    final location = await _xdo(['getmouselocation', '--shell']);
    _inputCommands.add({
      'logicalX': point.dx,
      'logicalY': point.dy,
      'physicalX': globalX,
      'physicalY': globalY,
      'windowGeometry': geometry,
      'outputGeometry': output,
      'mouseLocation': location.stdout,
      'button': button,
      'hover': hover,
      'flutterPositionBeforeClick': _lastDevicePosition == null
          ? null
          : {'x': _lastDevicePosition!.dx, 'y': _lastDevicePosition!.dy},
    });
    if (hover) {
      final movedAgain = await _xdo([
        'mousemove',
        '${globalX + 1}',
        '$globalY',
      ]);
      expect(movedAgain.exitCode, 0);
      await tester.pump(const Duration(milliseconds: 750));
    } else {
      final clicked = await _xdo([
        'click',
        '${switch (button) {
          272 => 1,
          273 => 3,
          274 => 2,
          _ => throw ArgumentError.value(button),
        }}',
      ]);
      expect(clicked.exitCode, 0, reason: '${clicked.stderr}');
      await tester.pump(const Duration(milliseconds: 100));
    }
    return;
  }
  final geometry = (await _state())['monitorGeometry'] as Map;
  final result = await Process.run(_env['CLARION_SHELL_POINTER']!, [
    hover ? 'move' : 'click',
    '${point.dx.round()}',
    '${point.dy.round()}',
    '${geometry['width']}',
    '${geometry['height']}',
    '$button',
  ]).timeout(const Duration(seconds: 10));
  expect(result.exitCode, 0, reason: '${result.stdout}\n${result.stderr}');
  await tester.pump(const Duration(milliseconds: 80));
}

Future<ProcessResult> _xdo(List<String> arguments) => Process.run(
  '/usr/bin/xdotool',
  arguments,
  environment: {'DISPLAY': _env['CLARION_SHELL_PRIVATE_XDISPLAY']!},
).timeout(const Duration(seconds: 10));

Future<void> _focusPrivateX() async {
  _guard();
  final display = _env['CLARION_SHELL_PRIVATE_XDISPLAY']!;
  final window = _env['CLARION_SHELL_PRIVATE_XWINDOW']!;
  final pid = _env['CLARION_SHELL_PRIVATE_NESTED_PID']!;
  expect(_env['DISPLAY'], display);
  expect(display, matches(RegExp(r'^:[0-9]+$')));
  final owner = await _xdo(['getwindowpid', window]);
  expect(owner.exitCode, 0);
  expect((owner.stdout as String).trim(), pid);
  final focused = await _xdo(['windowfocus', '--sync', window]);
  expect(focused.exitCode, 0);
  expect(((await _xdo(['getwindowfocus'])).stdout as String).trim(), window);
}

Future<void> _keys(WidgetTester tester, List<String> keys) async {
  _guard();
  if (_env['CLARION_SHELL_KEYBOARD_BACKEND'] == 'x11') {
    await _focusPrivateX();
    // XTEST is scoped to an owned Xwayland DISPLAY/window. Winit receives
    // ordinary backend key events and its popup keyboard grab runs normally.
    final result = await _xdo([
      'key',
      '--clearmodifiers',
      '--delay',
      '100',
      ...keys,
    ]);
    expect(result.exitCode, 0, reason: '${result.stderr}');
    await tester.pump(const Duration(milliseconds: 150));
    return;
  }
  // Keep one virtual keyboard/keymap for a navigation sequence. Replacing
  // its minimal keymap between each key is a different operation from typing.
  final result = await Process.run(_env['CLARION_SHELL_KEYBOARD']!, [
    '-d',
    '100',
    for (final key in keys) ...['-k', key],
  ]).timeout(const Duration(seconds: 10));
  expect(
    result.exitCode,
    0,
    reason: '$keys: ${result.stdout}\n${result.stderr}',
  );
  await tester.pump(const Duration(milliseconds: 150));
}

Future<void> _menuPointer(
  WidgetTester tester,
  String label, {
  bool hover = false,
}) async {
  final entries = (await _state())['nativeTrayMenuItemBounds'] as List;
  final item = entries.cast<Map>().firstWhere(
    (entry) =>
        (entry['label'] as String).replaceAll('_', '') == label &&
        entry['enabled'] == true,
  );
  var x = (item['x'] as num).toDouble();
  // GDK's nested Wayland popup horizontal pseudo-origin can be parent-relative;
  // the private output starts at (0,0), and the root menu gives its offset.
  final rootLeft =
      (entries.cast<Map>().firstWhere((entry) => entry['depth'] == 0)['x']
              as num)
          .toDouble();
  if ((item['depth'] as int) > 0 && x < rootLeft) x += rootLeft;
  await _pointer(
    tester,
    Offset(
      x + (item['width'] as num) / 2,
      (item['y'] as num).toDouble() + (item['height'] as num) / 2,
    ),
    hover: hover,
  );
}

Future<void> _rootAction(WidgetTester tester) => _pointerPhase
    ? _menuPointer(tester, 'Open fixture')
    : _keys(tester, ['Down', 'Return']);
Future<void> _lazyOpen(WidgetTester tester) => _pointerPhase
    ? _menuPointer(tester, 'Lazy submenu', hover: true)
    : _keys(tester, ['Down', 'Down', 'Right']);
Future<void> _lazyAction(WidgetTester tester) => _pointerPhase
    ? _menuPointer(tester, 'Lazy child')
    : _keys(tester, ['Down', 'Return']);
Future<void> _dismissMenu(WidgetTester tester) => _pointerPhase
    ? _pointer(tester, const Offset(200, 200))
    : _keys(tester, ['Escape']);

Future<void> _capture(String name) async {
  _guard();
  final file = File('${_env['CLARION_SHELL_CAPTURE_DIR']}/$name.png');
  final result = await Process.run(_env['CLARION_SHELL_CLARION_BIN']!, [
    'msg',
    'action',
    'screenshot-screen',
    '--path',
    file.path,
    '--show-pointer',
    'false',
  ]).timeout(const Duration(seconds: 10));
  expect(result.exitCode, 0, reason: '${result.stderr}');
  final deadline = DateTime.now().add(const Duration(seconds: 8));
  while (!await file.exists()) {
    if (DateTime.now().isAfter(deadline)) {
      fail('Private capture missing: $name');
    }
    await Future<void>.delayed(const Duration(milliseconds: 20));
  }
}

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  binding.shouldPropagateDevicePointerEvents = true;
  GestureBinding.instance.pointerRouter.addGlobalRoute((event) {
    _lastDevicePosition = event.position;
    if (event is PointerDownEvent || event is PointerUpEvent) {
      _deviceEvents.add({
        'type': event.runtimeType.toString(),
        'x': event.position.dx,
        'y': event.position.dy,
        'buttons': event.buttons,
      });
    }
  });
  testWidgets('SNI uses native GTK DBusMenu and real Wayland input', (
    tester,
  ) async {
    _guard();
    final normalBus = DBusClient.session();
    final indicatorBus = DBusClient.session();
    final normal = _Item('Native protocol fixture', primaryMenu: false);
    final indicator = _Item('Native ItemIsMenu fixture', primaryMenu: true);
    final service = TrayService();
    final controller = ShellController(
      notifications: false,
      trayService: service,
      restoreWallpaperOnStart: false,
      powerProfilesEnabled: false,
      mediaEnabled: false,
    )..setReduceMotion(true);
    final evidence = <String, dynamic>{
      'passed': false,
      'keyboardAttempted': !_pointerPhase,
      'keyboardVerified': false,
      'input': _pointerPhase
          ? 'Real Wayland virtual-pointer only'
          : _env['CLARION_SHELL_KEYBOARD_BACKEND'] == 'x11'
          ? 'Private Xwayland/XTEST pointer and keyboard to normal winit input; GTK client on private Wayland'
          : 'Wayland virtual-pointer + wtype virtual-keyboard through GTK',
      'privateRuntime': _env['XDG_RUNTIME_DIR'],
      'popupTimingScope':
          'Wall time from test input process launch to observed native mapping, including fixed test waits; not production input latency',
      'steps': <String>[],
    };
    final steps = evidence['steps'] as List<String>;
    final failures = <Map<String, String>>[];
    evidence['phaseFailures'] = failures;
    Future<void> stage(String name, Future<void> Function() action) async {
      try {
        await action();
      } on Object catch (error) {
        failures.add({'phase': name, 'error': '$error'});
        await _capture('$name-failure');
      } finally {
        if ((await _state())['nativeTrayMenuOpen'] == true) {
          await _pointer(tester, const Offset(200, 200));
          await _menu(tester, false);
        }
        if (controller.panel != ShellPanel.none) {
          controller.closePanel();
          await tester.pump(const Duration(milliseconds: 150));
        }
      }
    }

    var normalClosed = false;
    try {
      await tester.pumpWidget(
        ShellApplication(
          controller: controller,
          preferences: const ShellPreferences(
            themeMode: ThemeMode.dark,
            reduceMotion: true,
          ),
          persist: false,
        ),
      );
      await controller.start();
      await _wait(
        tester,
        () => controller.connected && controller.trayAvailable,
        'shell connected',
      );
      await _register(normalBus, normal, 'org.example.ClarionProtocol');
      await _register(indicatorBus, indicator, 'org.example.ClarionIndicator');
      await _register(
        normalBus,
        normal,
        'org.example.ClarionProtocolAlias',
        objects: false,
      );
      await _wait(
        tester,
        () => controller.trayItems.length == 2,
        'alias deduplicated to two exporters',
      );
      expect(
        controller.trayItems.where((item) => item.title == normal.title).length,
        1,
      );
      steps.add('duplicate aliases share one owner/path item');
      final initial = await _waitState(
        tester,
        (state) => state['expanded'] == false,
        'compact bar',
      );
      evidence['initialState'] = initial;
      final item = controller.trayItems.firstWhere(
        (item) => item.title == normal.title,
      );
      final target = service.nativeMenuTarget(item)!;
      // A fabricated platform call cannot provide a real Wayland event serial.
      expect(
        await _window.invokeMethod<bool>('showTrayMenu', {
          ...target,
          'x': 20.0,
          'y': 20.0,
          'requestId': 90001,
          'button': 3,
        }),
        false,
      );
      await _menu(tester, false);
      steps.add('menu rejects a platform call without a real input event');

      await stage('root-activation', () async {
        final coldPopup = Stopwatch()..start();
        await _click(tester, _button(controller, normal.title));
        final opened = await _menu(tester, true);
        evidence['coldPopupMs'] = coldPopup.elapsedMicroseconds / 1000;
        expect(opened['expanded'], false);
        expect(opened['inputHeight'], initial['inputHeight']);
        expect(opened['requestedHeight'], initial['requestedHeight']);
        expect(controller.panel, ShellPanel.none);
        expect(normal.calls, isEmpty);
        expect(controller.nativeTrayMenuOpen, true);
        expect(controller.barTooltip, isNull);
        evidence['rightClickState'] = opened;
        await _capture('native-gtk-root');
        await _rootAction(tester);
        await _wait(
          tester,
          () => normal.menu.clicked(1) == 1,
          'GTK activates root item',
        );
        await _menu(tester, false);
        await _wait(
          tester,
          () => !controller.nativeTrayMenuOpen,
          'native close callback',
        );
        expect(normal.calls, isEmpty);
        steps.add(
          'real right click opens native root; native activation fires once; bar stays compact',
        );
      });

      await stage('lazy-submenu', () async {
        await _click(tester, _button(controller, indicator.title), button: 272);
        await _menu(tester, true);
        expect(indicator.calls, isEmpty);
        await _lazyOpen(tester);
        await _waitState(
          tester,
          (state) => (state['nativeTrayMenuVisibleSubmenus'] as int) > 0,
          'lazy GTK submenu mapped',
        );
        expect(indicator.menu.aboutToShow, contains(10));
        expect(indicator.menu.lazyReady, true);
        await tester.pump(const Duration(milliseconds: 300));
        evidence['lazyState'] = await _state();
        await _capture('native-gtk-lazy-submenu');
        await _lazyAction(tester);
        await _wait(
          tester,
          () => indicator.menu.clicked(100) == 1,
          'lazy child activation',
        );
        await _menu(tester, false);
        expect(indicator.calls, isEmpty);
        steps.add(
          'ItemIsMenu left click skips Activate; AboutToShow(child) populates a lazy submenu',
        );
      });

      await stage('dynamic-updates', () async {
        await _click(tester, _button(controller, normal.title));
        final beforeUpdate = await _menu(tester, true);
        final rootAbout = normal.menu.aboutToShow.where((id) => id == 0).length;
        final request = beforeUpdate['nativeTrayMenuRequestId'];
        normal.menu.addDynamic();
        final updated = await _waitState(
          tester,
          (state) =>
              (state['nativeTrayMenuVisibleItems'] as int) >
              (beforeUpdate['nativeTrayMenuVisibleItems'] as int),
          'LayoutUpdated materializes a new native entry',
        );
        expect(updated['nativeTrayMenuRequestId'], request);
        normal.menu.hideOther();
        final hidden = await _waitState(
          tester,
          (state) =>
              (state['nativeTrayMenuVisibleItems'] as int) <
              (updated['nativeTrayMenuVisibleItems'] as int),
          'ItemsPropertiesUpdated updates the existing native widget',
        );
        expect(hidden['nativeTrayMenuRequestId'], request);
        expect(
          normal.menu.aboutToShow.where((id) => id == 0).length,
          rootAbout,
        );
        await _capture('native-gtk-dynamic');
        await _dismissMenu(tester);
        await _menu(tester, false);
        steps.add(
          'layout/property signals update the open menu without a new open request',
        );
      });

      await stage('cached-reopen', () async {
        final layoutReads = normal.menu.layoutReads;
        final clicks = normal.menu.clicked(1);
        final cacheAbout = normal.menu.aboutToShow
            .where((id) => id == 0)
            .length;
        final cachedPopupMs = <double>[];
        for (var i = 0; i < 3; i++) {
          final cachedPopup = Stopwatch()..start();
          await _click(tester, _button(controller, normal.title));
          await _menu(tester, true);
          cachedPopupMs.add(cachedPopup.elapsedMicroseconds / 1000);
          await _rootAction(tester);
          await _menu(tester, false);
          await _wait(
            tester,
            () => normal.menu.clicked(1) == clicks + i + 1,
            'one cached activation per open',
          );
        }
        expect(normal.menu.layoutReads, layoutReads);
        evidence['cachedPopupMs'] = cachedPopupMs;
        evidence['cacheLayoutReadsBefore'] = layoutReads;
        evidence['cacheLayoutReadsAfter'] = normal.menu.layoutReads;
        evidence['cacheAboutBefore'] = cacheAbout;
        evidence['cacheAboutAfter'] = normal.menu.aboutToShow
            .where((id) => id == 0)
            .length;
        expect(
          normal.menu.aboutToShow.where((id) => id == 0).length,
          greaterThanOrEqualTo(cacheAbout + 3),
        );
        steps.add(
          'three cached opens keep one layout client and one click per activation',
        );
      });

      await stage('keyboard-open', () async {
        if (!_pointerPhase) {
          // The compact bar deliberately has no keyboard focus. The existing
          // expanded overflow gives the bar a real keyboard focus for Menu/ShiftF10.
          controller.selectPanel(ShellPanel.trayItems);
          await _waitState(
            tester,
            (state) =>
                state['expanded'] == true && state['keyboard'] == 'Exclusive',
            'overflow keyboard focus',
          );
          final gesture = find
              .descendant(
                of: _button(controller, indicator.title),
                matching: find.byType(GestureDetector),
              )
              .first;
          Focus.of(tester.element(gesture)).requestFocus();
          await tester.pump();
          await _keys(tester, ['Menu']);
          await _menu(tester, true);
          await _dismissMenu(tester);
          await _menu(tester, false);
          controller.closePanel();
          await _waitState(
            tester,
            (state) => state['expanded'] == false,
            'overflow closes',
          );
          steps.add(
            'real keyboard Menu opens GTK from the focused overflow and Escape closes it',
          );
          evidence['keyboardVerified'] = true;
        } else {
          steps.add(
            'pointer phase: keyboard verification intentionally not claimed',
          );
        }
      });

      await stage('owner-disconnect', () async {
        await _click(tester, _button(controller, normal.title));
        await _menu(tester, true);
        await normalBus.close();
        normalClosed = true;
        await _menu(tester, false);
        await _wait(
          tester,
          () =>
              controller.trayItems.length == 1 &&
              !controller.nativeTrayMenuOpen,
          'owner disconnect removes aliases and closes menu',
        );
        steps.add(
          'exporter disconnect removes all aliases and closes the active native menu',
        );
      });

      evidence['finalState'] = await _state();
      expect(failures, isEmpty, reason: 'Native protocol phases must all pass');
      evidence['passed'] = true;
    } finally {
      if (evidence['passed'] != true) {
        try {
          await _capture('failure-native');
        } on Object catch (error) {
          evidence['failureCaptureError'] = '$error';
        }
      }
      evidence['normalCalls'] = normal.calls;
      evidence['inputCommands'] = _inputCommands;
      evidence['pointerEvents'] = _deviceEvents;
      evidence['indicatorCalls'] = indicator.calls;
      evidence['normalMenu'] = normal.menu.evidence;
      evidence['indicatorMenu'] = indicator.menu.evidence;
      evidence['controllerError'] = controller.trayError;
      evidence['lastNativeState'] = await _state();
      File(
        '${_env['CLARION_SHELL_CAPTURE_DIR']}/tray-protocol-evidence.json',
      ).writeAsStringSync(const JsonEncoder.withIndent('  ').convert(evidence));
      if (!normalClosed) await normalBus.close();
      await indicatorBus.close();
      await tester.pumpWidget(const SizedBox());
      controller.dispose();
      await tester.pump();
    }
  }, timeout: const Timeout(Duration(minutes: 5)));
}

Future<void> _register(
  DBusClient bus,
  _Item item,
  String name, {
  bool objects = true,
}) async {
  expect(await bus.requestName(name), DBusRequestNameReply.primaryOwner);
  if (objects) {
    await bus.registerObject(item);
    await bus.registerObject(item.menu);
  }
  await DBusRemoteObject(
    bus,
    name: kdeWatcherName,
    path: DBusObjectPath(watcherPath),
  ).callMethod(kdeWatcherInterface, 'RegisterStatusNotifierItem', [
    DBusString('$name/StatusNotifierItem'),
  ], replySignature: DBusSignature(''));
}

class _Item extends DBusObject {
  _Item(this.title, {required this.primaryMenu})
    : super(DBusObjectPath('/StatusNotifierItem'));
  final String title;
  final bool primaryMenu;
  final menu = _Menu();
  final calls = <String>[];
  Map<String, DBusValue> get properties => {
    'Id': DBusString(title),
    'Title': DBusString(title),
    'Status': const DBusString('Active'),
    'IconName': const DBusString(''),
    'IconThemePath': const DBusString(''),
    'ItemIsMenu': DBusBoolean(primaryMenu),
    'Menu': menu.path,
    'IconPixmap': DBusArray(DBusSignature('(iiay)'), [
      DBusStruct([
        const DBusInt32(20),
        const DBusInt32(20),
        DBusArray.byte(
          List.generate(
            400,
            (_) => [255, primaryMenu ? 50 : 230, 150, 100],
          ).expand((pixel) => pixel),
        ),
      ]),
    ]),
  };
  @override
  Future<DBusMethodResponse> getProperty(String interface, String name) async =>
      interface != kdeItemInterface
      ? DBusMethodErrorResponse.unknownInterface()
      : properties[name] == null
      ? DBusMethodErrorResponse.unknownProperty()
      : DBusGetPropertyResponse(properties[name]!);
  @override
  Future<DBusMethodResponse> getAllProperties(String interface) async =>
      interface == kdeItemInterface
      ? DBusGetAllPropertiesResponse(properties)
      : DBusMethodErrorResponse.unknownInterface();
  @override
  Future<DBusMethodResponse> handleMethodCall(DBusMethodCall call) async {
    if (call.interface != kdeItemInterface) {
      return DBusMethodErrorResponse.unknownInterface();
    }
    calls.add(call.name);
    return DBusMethodSuccessResponse();
  }
}

class _Menu extends DBusObject {
  _Menu() : super(DBusObjectPath('/Menu'));
  int revision = 1;
  int layoutReads = 0;
  bool lazyReady = false;
  bool dynamicReady = false;
  bool otherVisible = true;
  final aboutToShow = <int>[];
  final events = <Map<String, dynamic>>[];
  int clicked(int id) => events
      .where((event) => event['id'] == id && event['event'] == 'clicked')
      .length;
  Map<String, dynamic> get evidence => {
    'layoutReads': layoutReads,
    'aboutToShow': aboutToShow,
    'events': events,
  };
  Map<String, DBusValue> props(int id) => switch (id) {
    0 => {'children-display': const DBusString('submenu')},
    1 => {
      'label': const DBusString('_Open fixture'),
      'enabled': const DBusBoolean(true),
    },
    10 => {
      'label': const DBusString('_Lazy submenu'),
      'children-display': const DBusString('submenu'),
    },
    2 => {
      'label': const DBusString('_Other fixture'),
      'visible': DBusBoolean(otherVisible),
    },
    100 => {
      'label': const DBusString('_Lazy child'),
      'enabled': const DBusBoolean(true),
    },
    3 => {'label': const DBusString('_Dynamic fixture')},
    _ => {},
  };
  List<int> children(int id) => id == 0
      ? [1, 10, 2, if (dynamicReady) 3]
      : id == 10 && lazyReady
      ? [100]
      : [];
  DBusValue layout(int id) => DBusStruct([
    DBusInt32(id),
    DBusDict.stringVariant(props(id)),
    DBusArray.variant(children(id).map(layout)),
  ]);
  Map<String, DBusValue> get properties => {
    'Version': const DBusUint32(3),
    'TextDirection': const DBusString('ltr'),
    'Status': const DBusString('normal'),
    'IconThemePath': DBusArray.string([]),
  };
  @override
  Future<DBusMethodResponse> getProperty(String interface, String name) async =>
      interface != menuInterface
      ? DBusMethodErrorResponse.unknownInterface()
      : properties[name] == null
      ? DBusMethodErrorResponse.unknownProperty()
      : DBusGetPropertyResponse(properties[name]!);
  @override
  Future<DBusMethodResponse> getAllProperties(String interface) async =>
      interface == menuInterface
      ? DBusGetAllPropertiesResponse(properties)
      : DBusMethodErrorResponse.unknownInterface();
  void addDynamic() {
    dynamicReady = true;
    revision++;
    emitSignal(menuInterface, 'LayoutUpdated', [
      DBusUint32(revision),
      const DBusInt32(0),
    ]);
  }

  void hideOther() {
    otherVisible = false;
    emitSignal(menuInterface, 'ItemsPropertiesUpdated', [
      DBusArray(DBusSignature('(ia{sv})'), [
        DBusStruct([
          const DBusInt32(2),
          DBusDict.stringVariant({'visible': const DBusBoolean(false)}),
        ]),
      ]),
      DBusArray(DBusSignature('(ias)'), []),
    ]);
  }

  bool _about(int id) {
    aboutToShow.add(id);
    if (id != 10 || lazyReady) return false;
    lazyReady = true;
    revision++;
    emitSignal(menuInterface, 'LayoutUpdated', [
      DBusUint32(revision),
      const DBusInt32(10),
    ]);
    return true;
  }

  void _event(List<DBusValue> values) =>
      events.add({'id': values[0].asInt32(), 'event': values[1].asString()});
  @override
  Future<DBusMethodResponse> handleMethodCall(DBusMethodCall call) async {
    if (call.interface != menuInterface) {
      return DBusMethodErrorResponse.unknownInterface();
    }
    switch (call.name) {
      case 'GetLayout':
        layoutReads++;
        return DBusMethodSuccessResponse([
          DBusUint32(revision),
          layout(call.values[0].asInt32()),
        ]);
      case 'GetGroupProperties':
        return DBusMethodSuccessResponse([
          DBusArray(
            DBusSignature('(ia{sv})'),
            call.values[0].asInt32Array().map(
              (id) => DBusStruct([
                DBusInt32(id),
                DBusDict.stringVariant(props(id)),
              ]),
            ),
          ),
        ]);
      case 'GetProperty':
        final value = props(
          call.values[0].asInt32(),
        )[call.values[1].asString()];
        return value == null
            ? DBusMethodErrorResponse.unknownProperty()
            : DBusMethodSuccessResponse([DBusVariant(value)]);
      case 'AboutToShow':
        return DBusMethodSuccessResponse([
          DBusBoolean(_about(call.values[0].asInt32())),
        ]);
      case 'AboutToShowGroup':
        return DBusMethodSuccessResponse([
          DBusArray.int32(call.values[0].asInt32Array().where(_about)),
          DBusArray.int32([]),
        ]);
      case 'Event':
        _event(call.values);
        return DBusMethodSuccessResponse();
      case 'EventGroup':
        for (final event in call.values[0].asArray()) {
          _event(event.asStruct());
        }
        return DBusMethodSuccessResponse([DBusArray.int32([])]);
      default:
        return DBusMethodErrorResponse.unknownMethod();
    }
  }
}
