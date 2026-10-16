// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:convert';
import 'dart:io';
import 'dart:ui' as ui;

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
import 'package:clarion_shell/ui/shell_geometry.dart';
import 'package:clarion_shell/ui/tray_menu_panel.dart';

const _window = MethodChannel('clarion.shell/window');
final _deviceEvents = <Map<String, dynamic>>[];
var _nextMouseDevice = 1000;
TestGesture _mouse(WidgetTester tester, {int buttons = kPrimaryButton}) {
  final device = _nextMouseDevice++;
  return TestGesture(
    dispatcher: tester.sendEventToBinding,
    kind: PointerDeviceKind.mouse,
    pointer: device,
    device: device,
    buttons: buttons,
  );
}

void _recordPointer(PointerEvent event) {
  _deviceEvents.add({
    'type': event.runtimeType.toString(),
    'x': event.position.dx,
    'y': event.position.dy,
    'buttons': event.buttons,
  });
}

Future<Map<String, dynamic>> _windowState() async => Map<String, dynamic>.from(
  (await _window.invokeMapMethod<String, dynamic>('windowState'))!,
);

Future<void> _wait(
  WidgetTester tester,
  bool Function() ready,
  String description,
) async {
  final deadline = DateTime.now().add(const Duration(seconds: 15));
  while (!ready()) {
    if (DateTime.now().isAfter(deadline)) fail('Timed out: $description');
    await tester.pump(const Duration(milliseconds: 100));
  }
  await tester.pump();
}

Future<Map<String, dynamic>> _waitNative(
  WidgetTester tester, {
  required bool expanded,
}) async {
  final deadline = DateTime.now().add(const Duration(seconds: 15));
  do {
    await tester.pump(const Duration(milliseconds: 100));
    final state = await _windowState();
    if (state['expanded'] == expanded &&
        state['keyboard'] == (expanded ? 'Exclusive' : 'None') &&
        (expanded
            ? (state['height'] as int) > ShellGeometry.barHeight
            : state['height'] == ShellGeometry.barHeight)) {
      expect(state['exclusiveZone'], ShellGeometry.barHeight);
      return state;
    }
  } while (DateTime.now().isBefore(deadline));
  fail('Native tray window failed to reach expanded=$expanded');
}

Future<List<int>> _capturePixel(String name, Offset position) async {
  final directory = Platform.environment['CLARION_SHELL_CAPTURE_DIR']!;
  final capture = File('$directory/$name.png');
  final result = await Process.run('grim', [
    '-o',
    Platform.environment['CLARION_SHELL_TEST_OUTPUT'] ?? 'winit',
    capture.path,
  ]).timeout(const Duration(seconds: 10));
  expect(result.exitCode, 0, reason: '${result.stdout}\n${result.stderr}');
  final codec = await ui.instantiateImageCodec(await capture.readAsBytes());
  try {
    final frame = await codec.getNextFrame();
    try {
      final data = (await frame.image.toByteData(
        format: ui.ImageByteFormat.rawRgba,
      ))!;
      final index =
          (position.dy.round() * frame.image.width + position.dx.round()) * 4;
      return List.generate(3, (i) => data.getUint8(index + i));
    } finally {
      frame.image.dispose();
    }
  } finally {
    codec.dispose();
  }
}

Future<List<int>> _waitPresented(
  WidgetTester tester,
  String name,
  Offset position,
  List<int> expected,
) async {
  final deadline = DateTime.now().add(const Duration(seconds: 15));
  List<int> actual = const [];
  do {
    await tester.pump(const Duration(milliseconds: 100));
    actual = await _capturePixel(name, position * tester.view.devicePixelRatio);
    if (List.generate(
      3,
      (i) => (actual[i] - expected[i]).abs() <= 8,
    ).every((matches) => matches)) {
      return actual;
    }
  } while (DateTime.now().isBefore(deadline));
  fail('$name not presented: $actual vs $expected');
}

Finder _button(ShellController controller, String title) {
  final item = controller.trayItems.firstWhere((item) => item.title == title);
  return find.byKey(ValueKey('tray-${item.id}'));
}

Future<void> _click(
  WidgetTester tester,
  Finder finder, {
  int buttons = kPrimaryButton,
}) async {
  final point = tester.getCenter(finder);
  if (Platform.environment['CLARION_SHELL_REAL_POINTER'] != '1') {
    final gesture = _mouse(tester, buttons: buttons);
    await gesture.down(point);
    await gesture.up();
    await gesture.removePointer();
    await tester.pump();
    return;
  }
  final state = await _windowState();
  final geometry = state['monitorGeometry'] as Map;
  final button = switch (buttons) {
    kPrimaryButton => 272,
    kSecondaryButton => 273,
    kTertiaryButton => 274,
    _ => throw ArgumentError.value(buttons, 'buttons'),
  };
  final result =
      await Process.run(Platform.environment['CLARION_SHELL_POINTER']!, [
        '${point.dx.round()}',
        '${point.dy.round()}',
        '${geometry['width']}',
        '${geometry['height']}',
        '$button',
      ]).timeout(const Duration(seconds: 10));
  expect(result.exitCode, 0, reason: '${result.stdout}\n${result.stderr}');
  await tester.pump();
}

Future<Map<String, dynamic>> _legacyTooltipProof(WidgetTester tester) async {
  const message = 'Legacy tray tooltip';
  const fill = Color.fromRGBO(240, 120, 20, 1);
  final tooltip = GlobalKey<TooltipState>();
  await _window.invokeMethod<void>('setExpanded', false);
  await _window.invokeMethod<void>('setTooltipVisible', false);
  final state = await _waitNative(tester, expanded: false);
  final underneath = await _capturePixel(
    'legacy-tooltip-before',
    const Offset(200, 45),
  );
  await tester.pumpWidget(
    MaterialApp(
      theme: ThemeData.dark().copyWith(platform: TargetPlatform.linux),
      home: Stack(
        children: [
          Positioned(
            left: 182,
            top: 2,
            width: 36,
            height: 36,
            child: Tooltip(
              key: tooltip,
              message: message,
              triggerMode: TooltipTriggerMode.manual,
              waitDuration: Duration.zero,
              verticalOffset: 24,
              preferBelow: true,
              decoration: const BoxDecoration(color: fill),
              child: const ColoredBox(color: Colors.green),
            ),
          ),
        ],
      ),
    ),
  );
  expect(tooltip.currentState!.ensureTooltipVisible(), isTrue);
  final bubble = find.byWidgetPredicate(
    (widget) =>
        widget is DecoratedBox &&
        widget.decoration is BoxDecoration &&
        (widget.decoration as BoxDecoration).color == fill,
  );
  await _wait(
    tester,
    () => bubble.evaluate().isNotEmpty,
    'legacy tooltip paints',
  );
  final bounds = tester.getRect(bubble);
  expect(bounds.top, 30);
  expect(bounds.bottom, greaterThan(ShellGeometry.barHeight));
  final visiblePixels = await _waitPresented(
    tester,
    'legacy-tooltip-clipped',
    Offset(bounds.left + 2, bounds.top + 2),
    const [240, 120, 20],
  );
  final clippedPixels = await _capturePixel(
    'legacy-tooltip-clipped',
    const Offset(200, 45),
  );
  expect(clippedPixels, underneath);
  await tester.pumpWidget(const SizedBox.shrink());
  await tester.pump();
  return {
    'nativeState': state,
    'tooltipTop': bounds.top,
    'tooltipBottom': bounds.bottom,
    'clippedLogicalPixels': bounds.bottom - ShellGeometry.barHeight,
    'visibleTooltipRgb': visiblePixels,
    'clippedTooltipRgb': clippedPixels,
    'underlyingBackgroundRgb': underneath,
  };
}

Future<Map<String, dynamic>> _waitHoverSurface(
  WidgetTester tester, {
  required bool visible,
}) async {
  final deadline = DateTime.now().add(const Duration(seconds: 15));
  do {
    await tester.pump(const Duration(milliseconds: 100));
    final state = await _windowState();
    if (state['tooltipVisible'] == visible &&
        state['expanded'] == false &&
        state['height'] == (visible ? 128 : ShellGeometry.barHeight)) {
      expect(state['keyboard'], 'None');
      expect(state['exclusiveZone'], ShellGeometry.barHeight);
      expect(state['inputHeight'], ShellGeometry.barHeight);
      expect(state['layer'], 'Top');
      return state;
    }
  } while (DateTime.now().isBefore(deadline));
  fail('Native tooltip surface did not reach visible=$visible');
}

Future<Map<String, dynamic>> _hoverLifecycle(
  WidgetTester tester,
  ShellController controller,
  String firstTitle,
  String secondTitle,
) async {
  final underneath = await _capturePixel(
    'hover-transparent-before',
    const Offset(100, 70),
  );
  final pointer = _mouse(tester);
  await pointer.addPointer(location: const Offset(100, 180));
  final evidence = <String, dynamic>{};
  try {
    await pointer.moveTo(tester.getCenter(_button(controller, firstTitle)));
    await _wait(
      tester,
      () => controller.barTooltip?.message == firstTitle,
      'tray hover schedules its own tooltip',
    );
    evidence['visibleState'] = await _waitHoverSurface(tester, visible: true);
    final bubble = find.byKey(const ValueKey('bar-tooltip-bubble'));
    await _wait(
      tester,
      () => bubble.evaluate().isNotEmpty,
      'hover bubble builds',
    );
    final bounds = tester.getRect(bubble);
    expect(bounds.top, greaterThanOrEqualTo(48));
    expect(bounds.bottom, lessThanOrEqualTo(128));
    final material = tester.widget<Material>(bubble);
    final color = material.color!;
    evidence['tooltipTop'] = bounds.top;
    evidence['tooltipBottom'] = bounds.bottom;
    evidence['tooltipPixels'] = await _waitPresented(
      tester,
      'native-tray-hover',
      Offset(bounds.left + 8, bounds.center.dy),
      [
        (color.r * 255).round(),
        (color.g * 255).round(),
        (color.b * 255).round(),
      ],
    );
    expect(find.text(firstTitle), findsOneWidget);
    evidence['transparentPixels'] = await _capturePixel(
      'native-tray-hover',
      const Offset(100, 70),
    );
    expect(evidence['transparentPixels'], underneath);
    evidence['underlyingBackgroundPixels'] = underneath;
    expect(controller.panel, ShellPanel.none);

    await pointer.moveTo(tester.getCenter(_button(controller, secondTitle)));
    await _wait(
      tester,
      () => controller.barTooltip?.message == secondTitle,
      'switching icons replaces the hovered application',
    );
    await _waitHoverSurface(tester, visible: true);
    expect(find.text(secondTitle), findsOneWidget);
    await pointer.moveTo(const Offset(100, 180));
    await _wait(
      tester,
      () => controller.barTooltip == null,
      'hover leaves bar',
    );
    evidence['hiddenState'] = await _waitHoverSurface(tester, visible: false);

    // Leave again before the next 400 ms dwell expires. A late callback must
    // not reactivate a tooltip or keep a larger input surface alive.
    await pointer.moveTo(tester.getCenter(_button(controller, firstTitle)));
    await tester.pump(const Duration(milliseconds: 50));
    await pointer.moveTo(const Offset(100, 180));
    await tester.pump(const Duration(milliseconds: 650));
    expect(controller.barTooltip, isNull);
    evidence['cancelledDwellState'] = await _waitHoverSurface(
      tester,
      visible: false,
    );
  } finally {
    await pointer.removePointer();
  }
  return evidence;
}

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  // The live test binding normally discards hardware pointer events. These
  // tests deliberately exercise virtual-pointer input through Wayland/GTK.
  binding.shouldPropagateDevicePointerEvents = true;
  GestureBinding.instance.pointerRouter.addGlobalRoute(_recordPointer);
  testWidgets('native StatusNotifier tray, icons, menus and disconnects', (
    tester,
  ) async {
    expect(Platform.environment['CLARION_SHELL_ISOLATED_TEST'], '1');
    expect(
      Platform.environment['WAYLAND_DISPLAY'],
      isNot(Platform.environment['CLARION_SHELL_PARENT_WAYLAND_DISPLAY']),
    );
    expect(
      Platform.environment['CLARION_SOCKET'],
      isNot(Platform.environment['CLARION_SHELL_PARENT_SOCKET']),
    );
    final busAddress = Platform.environment['DBUS_SESSION_BUS_ADDRESS']!;
    final root = Platform.environment['CLARION_SHELL_TEST_ROOT']!;
    expect(busAddress, contains('$root/private-bus'));
    expect(
      busAddress,
      isNot(Platform.environment['CLARION_SHELL_PARENT_DBUS_ADDRESS']),
    );
    for (final variable in [
      'XDG_CONFIG_HOME',
      'XDG_STATE_HOME',
      'XDG_CACHE_HOME',
    ]) {
      expect(Platform.environment[variable], startsWith('$root/'));
    }

    final normalBus = DBusClient.session();
    final menuBus = DBusClient.session();
    final normal = _FixtureItem(
      interface: kdeItemInterface,
      title: 'Native KDE fixture',
      color: const [226, 43, 61],
    );
    final appIndicator = _FixtureItem(
      interface: freedesktopItemInterface,
      title: 'Native AppIndicator fixture',
      color: const [40, 186, 105],
      primaryMenu: true,
    );
    final evidence = <String, dynamic>{
      'privateBus': busAddress,
      'input': Platform.environment['CLARION_SHELL_REAL_POINTER'] == '1'
          ? 'Wayland virtual pointer through GTK'
          : 'WidgetTester mouse gestures in the native GTK runner',
    };
    var normalClosed = false;
    var menuClosed = false;
    final service = TrayService();
    final controller = ShellController(
      notifications: false,
      trayService: service,
      restoreWallpaperOnStart: false,
    );
    controller.setThemeMode(ThemeMode.dark);
    controller.setReduceMotion(true);
    try {
      evidence['legacyTooltipProof'] = await _legacyTooltipProof(tester);
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
      expect(controller.connected, isTrue);
      expect(controller.trayAvailable, isTrue);
      evidence['barState'] = await _waitNative(tester, expanded: false);
      final origin = await _window.invokeMapMethod<String, dynamic>(
        'surfaceOrigin',
      );
      final geometry = (evidence['barState'] as Map)['monitorGeometry'] as Map;
      expect(origin, {'x': geometry['x'], 'y': geometry['y']});
      evidence['surfaceOrigin'] = origin;
      final themedIcon = await _window.invokeMethod<String>('appIcon', {
        'name': 'clarion-native-fixture',
        'themePath': '$root/custom-icons',
        'size': 24,
      });
      expect(
        themedIcon,
        '$root/custom-icons/hicolor/32x32/status/clarion-native-fixture.png',
      );
      evidence['customThemeIcon'] = themedIcon;
      await _register(normalBus, normal, 'org.example.ClarionNativeKde');
      await _register(
        menuBus,
        appIndicator,
        'org.example.ClarionNativeIndicator',
      );
      evidence['registeredAddresses'] =
          (await DBusRemoteObject(
                normalBus,
                name: kdeWatcherName,
                path: DBusObjectPath(watcherPath),
              ).getProperty(
                kdeWatcherInterface,
                'RegisteredStatusNotifierItems',
              ))
              .asStringArray()
              .toList();
      debugPrint(
        'Native tray registered addresses: ${evidence['registeredAddresses']}',
      );
      await _wait(
        tester,
        () => controller.trayItems.length == 2,
        'both real D-Bus items are hosted',
      );
      final normalButton = _button(controller, normal.title);
      final menuButton = _button(controller, appIndicator.title);
      expect(tester.getSize(normalButton), const Size(36, 36));
      evidence['redIconPixels'] = await _waitPresented(
        tester,
        'bar-kde-icon',
        tester.getCenter(normalButton),
        normal.color,
      );
      evidence['greenIconPixels'] = await _waitPresented(
        tester,
        'bar-two-tray-items',
        tester.getCenter(menuButton),
        appIndicator.color,
      );
      evidence['hover'] = await _hoverLifecycle(
        tester,
        controller,
        normal.title,
        appIndicator.title,
      );

      await _click(tester, normalButton);
      await _wait(tester, () => normal.calls.isNotEmpty, 'SNI Activate');
      expect(normal.calls.single['method'], 'Activate');
      final activationPoint = tester.getCenter(normalButton);
      expect(
        normal.calls.single['x'],
        (activationPoint.dx + (origin!['x'] as int)).round(),
      );
      expect(
        normal.calls.single['y'],
        (activationPoint.dy + (origin['y'] as int)).round(),
      );
      expect(controller.panel, ShellPanel.none);
      await _click(tester, normalButton, buttons: kTertiaryButton);
      await _wait(tester, () => normal.calls.length == 2, 'SecondaryActivate');
      expect(normal.calls.last['method'], 'SecondaryActivate');
      final menuHover = _mouse(tester);
      await menuHover.addPointer(location: const Offset(100, 180));
      await menuHover.moveTo(tester.getCenter(normalButton));
      await _wait(
        tester,
        () => controller.barTooltip?.message == normal.title,
        'a shown tooltip precedes the menu transition',
      );
      await _waitHoverSurface(tester, visible: true);
      await _click(tester, normalButton, buttons: kSecondaryButton);
      await menuHover.removePointer();
      await _wait(
        tester,
        () =>
            controller.panel == ShellPanel.trayMenu &&
            !controller.trayMenuLoading &&
            controller.trayMenu.isNotEmpty,
        'right-click loads exported dbusmenu',
      );
      evidence['menuState'] = await _waitNative(tester, expanded: true);
      expect(controller.barTooltip, isNull);
      expect((evidence['menuState'] as Map)['tooltipVisible'], isFalse);
      expect((evidence['menuState'] as Map)['layer'], 'Overlay');
      expect(
        (evidence['menuState'] as Map)['inputHeight'],
        (evidence['menuState'] as Map)['height'],
      );
      // A late native hover request must never shrink a modal menu or release
      // its keyboard/input grab.
      await _window.invokeMethod<void>('setTooltipVisible', true);
      await tester.pump(const Duration(milliseconds: 100));
      final guarded = await _windowState();
      expect(guarded['expanded'], isTrue);
      expect(guarded['tooltipVisible'], isFalse);
      expect(guarded['height'], (evidence['menuState'] as Map)['height']);
      expect(guarded['inputHeight'], guarded['height']);
      expect(guarded['keyboard'], 'Exclusive');
      expect(guarded['exclusiveZone'], ShellGeometry.barHeight);
      expect(guarded['layer'], 'Overlay');
      evidence['modalHoverGuard'] = guarded;
      expect(find.text('显示窗口'), findsOneWidget);
      expect(find.text('不可用操作'), findsOneWidget);
      expect(find.text('隐藏操作'), findsNothing);
      expect(normal.menu.aboutToShowCount, 1);
      // Synchronize the screencopy to actual panel paint, rather than only
      // relying on the native configure or Flutter widget state.
      final panel = find.byType(TrayMenuPanel);
      final context = tester.element(panel);
      final color = Theme.of(context).colorScheme.surfaceContainer;
      final box = tester.getRect(panel);
      evidence['menuSurfacePixels'] = await _waitPresented(
        tester,
        'native-tray-menu',
        Offset(box.left + 10, box.center.dy),
        [
          (color.r * 255).round(),
          (color.g * 255).round(),
          (color.b * 255).round(),
        ],
      );
      await _click(tester, find.text('不可用操作'));
      await tester.pump();
      expect(normal.menu.clicked, isEmpty);
      await _click(tester, find.text('显示窗口'));
      await _wait(
        tester,
        () => normal.menu.clicked.isNotEmpty,
        'dbusmenu Event',
      );
      expect(normal.menu.clicked.single['id'], 1);
      expect(normal.menu.clicked.single['event'], 'clicked');
      await _waitNative(tester, expanded: false);

      // AppIndicators advertising ItemIsMenu open their exported menu on the
      // primary click, with no Activate call sent to their application.
      await _click(tester, menuButton);
      await _wait(
        tester,
        () =>
            controller.panel == ShellPanel.trayMenu &&
            !controller.trayMenuLoading,
        'primary ItemIsMenu',
      );
      await _waitNative(tester, expanded: true);
      expect(appIndicator.calls, isEmpty);
      expect(appIndicator.menu.aboutToShowCount, 1);
      await _click(tester, find.text('退出'));
      await _wait(
        tester,
        () => appIndicator.menu.clicked.isNotEmpty,
        'AppIndicator menu action',
      );
      expect(appIndicator.menu.clicked.single['id'], 3);
      await _waitNative(tester, expanded: false);

      normal.status = 'NeedsAttention';
      await normal.emitSignal(kdeItemInterface, 'NewStatus', [
        const DBusString('NeedsAttention'),
      ]);
      await _wait(
        tester,
        () => controller.trayItems.any(
          (item) =>
              item.title == normal.title &&
              item.status == TrayStatus.needsAttention,
        ),
        'attention status arrives from application signal',
      );
      evidence['attentionIconPixels'] = await _waitPresented(
        tester,
        'bar-needs-attention',
        tester.getCenter(_button(controller, normal.title)),
        const [244, 189, 35],
      );
      await _click(
        tester,
        _button(controller, normal.title),
        buttons: kSecondaryButton,
      );
      await _wait(tester, () => !controller.trayMenuLoading, 'menu reopened');
      await _waitNative(tester, expanded: true);
      await normalBus.close();
      normalClosed = true;
      await _wait(
        tester,
        () => controller.trayItems.length == 1,
        'disconnected exporter is removed',
      );
      expect(controller.selectedTrayItem, isNull);
      expect(controller.panel, ShellPanel.none);
      await _waitNative(tester, expanded: false);
      expect(normalButton, findsNothing);
      await menuBus.close();
      menuClosed = true;
      await _wait(
        tester,
        () => controller.trayItems.isEmpty,
        'all items removed',
      );
      expect(menuButton, findsNothing);
      evidence['finalBarState'] = await _waitNative(tester, expanded: false);
      evidence['normalCalls'] = normal.calls;
      evidence['normalMenuEvents'] = normal.menu.clicked;
      evidence['indicatorMenuEvents'] = appIndicator.menu.clicked;
      expect(tester.takeException(), isNull);
    } finally {
      evidence['deviceEvents'] = _deviceEvents;
      evidence['normalCalls'] = normal.calls;
      evidence['panelAtExit'] = controller.panel.name;
      evidence['itemsAtExit'] = controller.trayItems
          .map((item) => item.title)
          .toList();
      evidence['normalPropertyReads'] = normal.propertyReads;
      evidence['indicatorPropertyReads'] = appIndicator.propertyReads;
      if (!normalClosed) await normalBus.close();
      if (!menuClosed) await menuBus.close();
      await tester.pumpWidget(const SizedBox.shrink());
      service.dispose();
      await File(
        '${Platform.environment['CLARION_SHELL_CAPTURE_DIR']}/tray-state.json',
      ).writeAsString(
        '${const JsonEncoder.withIndent('  ').convert(evidence)}\n',
      );
    }
  }, timeout: const Timeout(Duration(minutes: 3)));
}

Future<void> _register(DBusClient bus, _FixtureItem item, String name) async {
  expect(await bus.requestName(name), DBusRequestNameReply.primaryOwner);
  await bus.registerObject(item);
  await bus.registerObject(item.menu);
  await DBusRemoteObject(
    bus,
    name: kdeWatcherName,
    path: DBusObjectPath(watcherPath),
  ).callMethod(kdeWatcherInterface, 'RegisterStatusNotifierItem', [
    DBusString('$name/StatusNotifierItem'),
  ], replySignature: DBusSignature(''));
}

class _FixtureItem extends DBusObject {
  _FixtureItem({
    required this.interface,
    required this.title,
    required this.color,
    this.primaryMenu = false,
  }) : super(DBusObjectPath('/StatusNotifierItem'));
  final String interface;
  final String title;
  final List<int> color;
  final bool primaryMenu;
  String status = 'Active';
  final calls = <Map<String, dynamic>>[];
  final propertyReads = <String>[];
  final menu = _FixtureMenu();

  DBusValue _pixmap(List<int> rgb) => DBusArray(DBusSignature('(iiay)'), [
    DBusStruct([
      const DBusInt32(24),
      const DBusInt32(24),
      DBusArray.byte(
        List.generate(24 * 24, (_) => [255, ...rgb]).expand((v) => v),
      ),
    ]),
  ]);

  Map<String, DBusValue> get properties => {
    'Id': DBusString(title),
    'Title': DBusString(title),
    'Status': DBusString(status),
    'IconName': const DBusString(''),
    'AttentionIconName': const DBusString(''),
    'IconThemePath': const DBusString(''),
    'ItemIsMenu': DBusBoolean(primaryMenu),
    'Menu': menu.path,
    'IconPixmap': _pixmap(color),
    'AttentionIconPixmap': _pixmap(const [244, 189, 35]),
  };

  @override
  List<DBusIntrospectInterface> introspect() => [
    DBusIntrospectInterface(
      interface,
      methods: [
        for (final name in ['Activate', 'SecondaryActivate', 'ContextMenu'])
          DBusIntrospectMethod(
            name,
            args: [
              for (var i = 0; i < 2; i++)
                DBusIntrospectArgument(
                  DBusSignature('i'),
                  DBusArgumentDirection.in_,
                ),
            ],
          ),
      ],
      properties: [
        for (final entry in properties.entries)
          DBusIntrospectProperty(
            entry.key,
            entry.value.signature,
            access: DBusPropertyAccess.read,
          ),
      ],
    ),
  ];

  @override
  Future<DBusMethodResponse> getProperty(String requested, String name) async {
    if (requested != interface) {
      return DBusMethodErrorResponse.unknownInterface();
    }
    final value = properties[name];
    return value == null
        ? DBusMethodErrorResponse.unknownProperty()
        : DBusGetPropertyResponse(value);
  }

  @override
  Future<DBusMethodResponse> getAllProperties(String requested) async {
    propertyReads.add(requested);
    return requested == interface
        ? DBusGetAllPropertiesResponse(properties)
        : DBusMethodErrorResponse.unknownInterface();
  }

  @override
  Future<DBusMethodResponse> handleMethodCall(DBusMethodCall call) async {
    if (call.interface != interface) {
      return DBusMethodErrorResponse.unknownInterface();
    }
    if (call.signature != DBusSignature('ii')) {
      return DBusMethodErrorResponse.invalidArgs();
    }
    if (!['Activate', 'SecondaryActivate', 'ContextMenu'].contains(call.name)) {
      return DBusMethodErrorResponse.unknownMethod();
    }
    calls.add({
      'method': call.name,
      'x': call.values[0].asInt32(),
      'y': call.values[1].asInt32(),
    });
    return DBusMethodSuccessResponse();
  }
}

class _FixtureMenu extends DBusObject {
  _FixtureMenu() : super(DBusObjectPath('/Menu'));
  int aboutToShowCount = 0;
  final clicked = <Map<String, dynamic>>[];

  DBusValue _entry(
    int id,
    String label, {
    bool enabled = true,
    bool visible = true,
  }) => DBusStruct([
    DBusInt32(id),
    DBusDict.stringVariant({
      'label': DBusString(label),
      'enabled': DBusBoolean(enabled),
      'visible': DBusBoolean(visible),
    }),
    DBusArray.variant(const []),
  ]);

  DBusValue get layout => DBusStruct([
    const DBusInt32(0),
    DBusDict.stringVariant(const {}),
    DBusArray.variant([
      _entry(1, '_显示窗口'),
      _entry(2, '不可用操作', enabled: false),
      _entry(3, '_退出'),
      _entry(4, '隐藏操作', visible: false),
    ]),
  ]);

  @override
  Future<DBusMethodResponse> handleMethodCall(DBusMethodCall call) async {
    if (call.interface != menuInterface) {
      return DBusMethodErrorResponse.unknownInterface();
    }
    switch (call.name) {
      case 'AboutToShow':
        aboutToShowCount++;
        return DBusMethodSuccessResponse([const DBusBoolean(false)]);
      case 'GetLayout':
        return DBusMethodSuccessResponse([const DBusUint32(1), layout]);
      case 'Event':
        clicked.add({
          'id': call.values[0].asInt32(),
          'event': call.values[1].asString(),
        });
        return DBusMethodSuccessResponse();
      default:
        return DBusMethodErrorResponse.unknownMethod();
    }
  }
}
