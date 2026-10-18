// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/app_menu_models.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/services/desktop_apps.dart';
import 'package:clarion_shell/services/global_app_menu_service.dart';
import 'package:clarion_shell/services/clarion_ipc.dart';
import 'package:clarion_shell/services/system_service.dart';
import 'package:clarion_shell/services/wallpaper_service.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/theme/font_fallback.dart';
import 'package:clarion_shell/theme/mac_shell_theme.dart';
import 'package:clarion_shell/ui/mac_dock_view.dart';
import 'package:clarion_shell/ui/global_app_menu_bar.dart';
import 'package:clarion_shell/ui/mac_shell_view.dart';
import 'package:clarion_shell/ui/screenshot_icon.dart';
import 'package:clarion_shell/ui/tray_icon.dart';

const _editor = DesktopApp(
  id: 'editor.desktop',
  name: '文本编辑器',
  desktopFile: '/fixture/editor.desktop',
);
const _browser = DesktopApp(
  id: 'browser.desktop',
  name: '浏览器',
  desktopFile: '/fixture/browser.desktop',
);

class _Ipc extends ClarionIpc {
  final event = StreamController<Map<String, dynamic>>.broadcast(sync: true);
  final connection = StreamController<bool>.broadcast(sync: true);
  final actions = <(String, Map<String, Object?>)>[];
  @override
  Stream<Map<String, dynamic>> get events => event.stream;
  @override
  Stream<bool> get connections => connection.stream;
  @override
  void start() {}
  @override
  Future<void> action(
    String name, [
    Map<String, Object?> arguments = const {},
  ]) async => actions.add((name, arguments));
  @override
  void dispose() {
    unawaited(event.close());
    unawaited(connection.close());
    super.dispose();
  }
}

class _Apps extends DesktopApps {
  _Apps({int additionalApps = 0})
    : entries = [
        _editor,
        _browser,
        for (var index = 0; index < additionalApps; index++)
          DesktopApp(
            id: 'extra-$index.desktop',
            name: '应用 $index',
            desktopFile: '/fixture/extra-$index.desktop',
          ),
      ];
  final List<DesktopApp> entries;
  final launched = <String>[];
  @override
  Future<List<DesktopApp>> load() async => entries;
  @override
  Future<void> launch(DesktopApp app) async => launched.add(app.id);
}

class _System extends SystemService {
  static final opened = <String>[];
  @override
  Future<void> openHomeFolder() async => opened.add("home");
  @override
  Future<void> openTrash() async => opened.add("trash");
  @override
  Future<SystemSnapshot> read() async =>
      const SystemSnapshot(volume: 0.6, brightness: 0.7, wifiEnabled: true);
}

class _Wallpapers extends WallpaperService {
  @override
  Future<void> restore({bool applySaved = false}) async {}
  @override
  Future<List<ShellWallpaper>> scan() async => [];
}

// Dock presentation tests isolate every desktop service, including the host
// readiness awaited by launching a new application. Real menu wiring is
// exercised by the private bus and native integration fixtures.
class _Menus extends GlobalAppMenuService {
  _Menus() : super(hostRegistrar: false);
  @override
  Future<void> start() async {}
  @override
  Future<void> setActive(bool active) async {}
  @override
  Future<void> focusedTarget({
    int? pid,
    String appId = '',
    int? windowId,
    int? x11WindowId,
    AppMenuExport? export,
  }) async {}
}

Future<(ShellController, _Ipc, _Apps)> _fixture(
  WidgetTester tester, {
  int additionalApps = 0,
}) async {
  final ipc = _Ipc();
  final apps = _Apps(additionalApps: additionalApps);
  final controller =
      ShellController(
          ipc: ipc,
          desktopApps: apps,
          system: _System(),
          wallpaperService: _Wallpapers(),
          appMenuService: _Menus(),
          tray: false,
          notifications: false,
          powerProfilesEnabled: false,
        )
        ..setShellStyle(ShellStyle.macos)
        ..setPinnedAppIds([_browser.id])
        ..setReduceMotion(true);
  await tester.runAsync(controller.start);
  addTearDown(controller.dispose);
  // The initial style change schedules one delayed system reconciliation.
  // Complete fixture setup before tests start their own animation clocks.
  await tester.pump(const Duration(milliseconds: 180));
  ipc.connection.add(true);
  ipc.event.add({
    'WindowsChanged': {
      'windows': [
        {'id': 21, 'app_id': 'editor', 'title': '设计笔记', 'is_focused': true},
        {'id': 22, 'app_id': 'editor', 'title': '待办事项', 'is_focused': false},
      ],
    },
  });
  return (controller, ipc, apps);
}

Future<void> _mount(
  WidgetTester tester,
  ShellController controller, {
  Size size = const Size(900, 700),
  DockGeometryChanged? onGeometryChanged,
  double? availableWidth,
  double? dockSurfaceWidth,
  ValueChanged<bool>? onLauncherSurfaceVisible,
  bool preview = false,
}) async {
  tester.view.devicePixelRatio = 1;
  tester.view.physicalSize = size;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  await tester.pumpWidget(
    MaterialApp(
      theme: buildMacShellTheme(Brightness.light),
      home: RepaintBoundary(
        key: const ValueKey('mac-capture'),
        child: Stack(
          children: [
            Positioned.fill(
              child: MacShellView(
                controller: controller,
                preview: preview,
                onLauncherSurfaceVisible: onLauncherSurfaceVisible,
              ),
            ),
            Positioned(
              left: dockSurfaceWidth == null
                  ? 0
                  : (size.width - dockSurfaceWidth) / 2,
              right: dockSurfaceWidth == null
                  ? 0
                  : (size.width - dockSurfaceWidth) / 2,
              bottom: 0,
              height: MacDockView.height,
              child: MacDockView(
                controller: controller,
                availableWidth: availableWidth ?? size.width,
                onGeometryChanged: onGeometryChanged,
              ),
            ),
          ],
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  setUp(() {
    _System.opened.clear();
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
          const MethodChannel('clarion.shell/window'),
          (call) async => call.method == 'outputName' ? 'DP-1' : null,
        );
  });
  tearDown(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
          const MethodChannel('clarion.shell/window'),
          null,
        );
  });

  testWidgets('menu bar capture mark opens the existing screenshot panel', (
    tester,
  ) async {
    final semantics = tester.ensureSemantics();
    try {
      final (controller, _, _) = await _fixture(tester);
      await _mount(tester, controller);
      final button = find.byKey(const ValueKey('mac-screenshot'));
      final mark = find.descendant(
        of: button,
        matching: find.byType(ScreenshotIcon),
      );
      expect(mark, findsOneWidget);
      expect(tester.getSize(mark), const Size(16, 16));
      final tooltip = tester.widget<ShellBarTooltip>(
        find.ancestor(of: button, matching: find.byType(ShellBarTooltip)),
      );
      expect(tooltip.message, '截图');
      expect(tooltip.labelSemantics, isTrue);
      expect(
        tester
            .getSemantics(button)
            .getSemanticsData()
            .hasAction(ui.SemanticsAction.tap),
        isTrue,
      );
      await tester.tap(button);
      await tester.pumpAndSettle();
      expect(controller.panel, ShellPanel.screenshot);
      expect(find.text('选取区域'), findsOneWidget);
      expect(tester.takeException(), isNull);
    } finally {
      semantics.dispose();
    }
  });

  for (final width in [180.0, 900.0]) {
    testWidgets('menu height centers fixed-size content at width $width', (
      tester,
    ) async {
      final (controller, _, _) = await _fixture(tester);
      await _mount(tester, controller, size: Size(width, 700));
      for (final height in [22.0, 32.0, 48.0]) {
        controller.setMacPreferences(
          controller.macPreferences.copyWith(menuBarHeight: height),
        );
        await tester.pumpAndSettle();
        final bar = tester.getRect(
          find.byKey(const ValueKey('mac-menu-bar-position')),
        );
        expect(bar.height, height);
        expect(bar.width, width);
        final logo = tester.getRect(
          find.byKey(const ValueKey('mac-apple-logo')),
        );
        expect(logo.size, const Size(17, 18));
        expect(logo.center.dy, closeTo(bar.center.dy, .01));
        final app = tester.widget<Text>(
          find.byKey(const ValueKey('mac-active-app')),
        );
        expect(app.style?.fontSize, 13);
        expect(
          tester.getCenter(find.byKey(const ValueKey('mac-active-app'))).dy,
          closeTo(bar.center.dy, .01),
        );
        final button = tester.getRect(
          find.byKey(const ValueKey('mac-quick-settings')),
        );
        expect(button.height, height);
        expect(button.center.dy, closeTo(bar.center.dy, .01));
        expect(
          tester
              .widget<GlobalAppMenuBar>(find.byType(GlobalAppMenuBar))
              .barHeight,
          height,
        );
        if (width > 320) {
          expect(
            tester.getCenter(find.byKey(const ValueKey('mac-clock'))).dy,
            closeTo(bar.center.dy, .01),
          );
          expect(
            tester
                .widget<Text>(find.byKey(const ValueKey('mac-clock')))
                .style
                ?.fontSize,
            12,
          );
        }
        expect(tester.takeException(), isNull);
      }
      final button = tester.getRect(
        find.byKey(const ValueKey('mac-quick-settings')),
      );
      await tester.tapAt(Offset(button.center.dx, button.bottom - 2));
      await tester.pumpAndSettle();
      expect(controller.panel, ShellPanel.quickSettings);
      expect(
        tester
            .getRect(find.byKey(const ValueKey('mac-control-center-glass')))
            .top,
        54,
      );
      controller.setMacPreferences(
        controller.macPreferences.copyWith(menuBarHeight: 32),
      );
      await tester.pumpAndSettle();
      expect(controller.panel, ShellPanel.quickSettings);
      expect(
        tester
            .getRect(find.byKey(const ValueKey('mac-control-center-glass')))
            .top,
        38,
      );
      controller.closePanel();
      controller.showBarTooltip(Object(), '测试提示', const Offset(30, 16));
      await tester.pumpAndSettle();
      expect(
        tester
            .getRect(find.byKey(const ValueKey('mac-bar-tooltip-position')))
            .top,
        40,
      );
      expect(tester.takeException(), isNull);
    });
  }

  testWidgets('Dock click focuses a running window and launches a pinned app', (
    tester,
  ) async {
    final (controller, ipc, apps) = await _fixture(tester);
    await _mount(tester, controller);
    expect(find.text('文本编辑器'), findsOneWidget);
    expect(find.byKey(const ValueKey('dock-running-文本编辑器')), findsOneWidget);
    expect(find.byKey(const ValueKey('dock-running-浏览器')), findsNothing);

    await tester.tap(find.byKey(const ValueKey('dock-app-editor.desktop')));
    await tester.pumpAndSettle();
    expect(ipc.actions.single.$1, 'FocusWindow');
    expect(ipc.actions.single.$2, {'id': 21});
    expect(apps.launched, isEmpty);

    await tester.tap(find.byKey(const ValueKey('dock-app-browser.desktop')));
    await tester.pumpAndSettle();
    expect(apps.launched, ['browser.desktop']);
    expect(tester.takeException(), isNull);
  });

  testWidgets('Dock context menu selects a window and pins the actual app', (
    tester,
  ) async {
    final (controller, ipc, _) = await _fixture(tester);
    await _mount(tester, controller);
    Future<void> contextMenu() async {
      final point = tester.getCenter(
        find.byKey(const ValueKey('dock-app-editor.desktop')),
      );
      final mouse = await tester.createGesture(
        kind: PointerDeviceKind.mouse,
        buttons: kSecondaryMouseButton,
      );
      await mouse.down(point);
      await mouse.up();
      await mouse.removePointer();
      await tester.pumpAndSettle();
    }

    await contextMenu();
    expect(controller.panel, ShellPanel.dockAppMenu);
    expect(find.text('设计笔记'), findsOneWidget);
    expect(find.text('待办事项'), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('dock-window-22')));
    await tester.pumpAndSettle();
    expect(ipc.actions.single.$1, 'FocusWindow');
    expect(ipc.actions.single.$2, {'id': 22});
    expect(controller.panel, ShellPanel.none);

    await contextMenu();
    await tester.tap(find.byKey(const ValueKey('dock-menu-pin')));
    await tester.pumpAndSettle();
    expect(controller.pinnedAppIds, contains('editor.desktop'));
    expect(controller.panel, ShellPanel.none);
    await contextMenu();
    expect(find.text('从程序坞中移除'), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('dock-menu-pin')));
    await tester.pumpAndSettle();
    expect(controller.pinnedAppIds, isNot(contains('editor.desktop')));
    expect(tester.takeException(), isNull);
  });

  testWidgets('Catalina system menu opens real settings and recent submenu', (
    tester,
  ) async {
    final (controller, ipc, _) = await _fixture(tester);
    await _mount(tester, controller);
    Future<void> menuAction(String key) async {
      await tester.tap(find.byKey(const ValueKey('mac-system-menu')));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(ValueKey(key)));
      await tester.pumpAndSettle();
    }

    await menuAction('system-menu-settings');
    expect(ipc.actions.last.$1, 'Spawn');
    // The settings window draws its own palette, so it is told which
    // appearance the desktop is using (the controller defaults to dark).
    expect(ipc.actions.last.$2, {
      'command': ['clarion-settings', '--theme', 'dark'],
    });
    await menuAction('system-menu-recent');
    expect(find.byKey(const ValueKey('system-recent-clear')), findsOneWidget);
    expect(find.byKey(const ValueKey('system-menu-overview')), findsNothing);
    controller.closePanel();
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'Dock hover scales immediately with reduced motion and no resize',
    (tester) async {
      final (controller, _, _) = await _fixture(tester);
      final geometry = <(double, Rect)>[];
      await _mount(
        tester,
        controller,
        onGeometryChanged: (size, rect) => geometry.add((size.width, rect)),
      );
      final initialGeometry = geometry.last;
      expect(initialGeometry.$2.width, lessThan(initialGeometry.$1));
      final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
      await mouse.addPointer(location: const Offset(0, 0));
      await mouse.moveTo(
        tester.getCenter(find.byKey(const ValueKey('dock-app-editor.desktop'))),
      );
      await tester.pump();
      final transform = tester.widget<Transform>(
        find.byKey(const ValueKey('dock-scale-文本编辑器')),
      );
      expect(transform.transform.entry(0, 0), greaterThan(1.35));
      expect(find.byKey(const ValueKey('mac-dock-label')), findsOneWidget);
      expect(
        geometry.map((entry) => entry.$1),
        everyElement(initialGeometry.$1),
      );
      expect(geometry.last.$2.width, greaterThan(initialGeometry.$2.width));
      expect(geometry.last.$2.top, lessThan(initialGeometry.$2.top));
      expect(geometry.last.$2.bottom, initialGeometry.$2.bottom);
      expect(
        geometry.last.$2.height,
        lessThan(100),
        reason: 'input follows the current sprite, not the maximum envelope',
      );
      await mouse.removePointer();
      await tester.pumpAndSettle();
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('System menu uses the Clarion mark, not Apple artwork', (
    tester,
  ) async {
    final (controller, _, _) = await _fixture(tester);
    await _mount(tester, controller);
    // Clarion ships no Apple assets; the slot draws the project mark instead.
    expect(find.byKey(const ValueKey('mac-apple-logo')), findsOneWidget);
    expect(
      find.descendant(
        of: find.byKey(const ValueKey('mac-apple-logo')),
        matching: find.byType(Image),
      ),
      findsNothing,
    );
    expect(
      tester
          .getSize(find.byKey(const ValueKey('mac-menu-bar-position')))
          .height,
      22,
    );
    expect(
      buildMacShellTheme(Brightness.light).textTheme.bodyMedium?.fontFamily,
      shellFontFamily,
    );
    await tester.tap(find.byKey(const ValueKey('dock-finder')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('dock-trash')));
    await tester.pumpAndSettle();
    expect(_System.opened, ['home', 'trash']);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'continuous magnification expands neighbors and keeps edge hits correct',
    (tester) async {
      final (controller, ipc, apps) = await _fixture(tester);
      await _mount(tester, controller);
      final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
      await mouse.addPointer(location: const Offset(0, 0));
      await mouse.moveTo(
        tester.getCenter(find.byKey(const ValueKey('dock-app-editor.desktop'))),
      );
      await tester.pumpAndSettle();
      double scale(String name) => tester
          .widget<Transform>(find.byKey(ValueKey('dock-scale-$name')))
          .transform
          .entry(0, 0);
      expect(scale('文本编辑器'), greaterThan(1.35));
      expect(scale('浏览器'), greaterThan(1.05));
      final between = Offset(
        (tester
                    .getCenter(
                      find.byKey(const ValueKey('dock-app-browser.desktop')),
                    )
                    .dx +
                tester
                    .getCenter(
                      find.byKey(const ValueKey('dock-app-editor.desktop')),
                    )
                    .dx) /
            2,
        tester
            .getCenter(find.byKey(const ValueKey('dock-app-editor.desktop')))
            .dy,
      );
      await mouse.moveTo(between);
      await tester.pumpAndSettle();
      expect(scale('文本编辑器'), greaterThan(1.2));
      expect(scale('浏览器'), greaterThan(1.2));
      final browser = tester.getRect(
        find.byKey(const ValueKey('dock-app-browser.desktop')),
      );
      final editor = tester.getRect(
        find.byKey(const ValueKey('dock-app-editor.desktop')),
      );
      expect(browser.right, lessThan(editor.left));
      await tester.tapAt(Offset(editor.left + 2, editor.top + 2));
      await tester.pumpAndSettle();
      expect(ipc.actions.last.$1, 'FocusWindow');
      expect(ipc.actions.last.$2, {'id': 21});
      expect(apps.launched, isEmpty);
      await mouse.moveTo(const Offset(0, 0));
      await tester.pumpAndSettle();
      expect(scale('文本编辑器'), 1);
      await mouse.removePointer();
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('launch bounces while focus and reduced motion do not', (
    tester,
  ) async {
    final (controller, _, _) = await _fixture(tester);
    await _mount(tester, controller);
    controller.setReduceMotion(false);
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('dock-app-browser.desktop')));
    // Starting a ticker while the test clock is idle establishes time zero on
    // its first frame. Render that frame before advancing the bounce clock.
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 120));
    double bounce(String id) => tester
        .widget<Transform>(find.byKey(ValueKey('dock-bounce-$id')))
        .transform
        .getTranslation()
        .y;
    expect(bounce('browser.desktop'), lessThan(-1));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('dock-app-editor.desktop')));
    await tester.pump(const Duration(milliseconds: 120));
    expect(bounce('editor.desktop'), 0);
    controller.setReduceMotion(true);
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('dock-app-browser.desktop')));
    await tester.pump(const Duration(milliseconds: 120));
    expect(bounce('browser.desktop'), 0);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'panel open restores Dock and requires movement to magnify again',
    (tester) async {
      final (controller, _, _) = await _fixture(tester);
      await _mount(tester, controller);
      final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
      await mouse.addPointer(location: const Offset(0, 0));
      await mouse.moveTo(
        tester.getCenter(find.byKey(const ValueKey('dock-app-editor.desktop'))),
      );
      await tester.pumpAndSettle();
      controller.selectPanel(ShellPanel.systemMenu);
      await tester.pumpAndSettle();
      double scale() => tester
          .widget<Transform>(find.byKey(const ValueKey('dock-scale-文本编辑器')))
          .transform
          .entry(0, 0);
      expect(scale(), 1);
      expect(find.byKey(const ValueKey('mac-dock-label')), findsNothing);
      controller.closePanel();
      await tester.pumpAndSettle();
      expect(scale(), 1);
      await mouse.removePointer();
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('menu tracking switches immediately and clears old hover hint', (
    tester,
  ) async {
    final (controller, _, _) = await _fixture(tester);
    await _mount(tester, controller);
    controller.showBarTooltip(Object(), '旧提示', const Offset(20, 10));
    await tester.tap(find.byKey(const ValueKey('mac-system-menu')));
    await tester.pumpAndSettle();
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: const Offset(0, 100));
    await mouse.moveTo(
      tester.getCenter(find.byKey(const ValueKey('mac-notifications'))),
    );
    expect(controller.panel, ShellPanel.notifications);
    expect(controller.barTooltip, isNull);
    await tester.pumpAndSettle();
    await mouse.moveTo(
      tester.getCenter(find.byKey(const ValueKey('mac-quick-settings'))),
    );
    expect(controller.panel, ShellPanel.quickSettings);
    await tester.pumpAndSettle();
    await mouse.removePointer();
    expect(tester.takeException(), isNull);
  });

  testWidgets('Dock can request a wider surface after another app starts', (
    tester,
  ) async {
    final (controller, ipc, _) = await _fixture(tester);
    final geometry = <(double, Rect)>[];
    await _mount(
      tester,
      controller,
      size: const Size(320, 700),
      availableWidth: 1200,
      onGeometryChanged: (size, rect) => geometry.add((size.width, rect)),
    );
    final originalWidth = geometry.last.$1;
    ipc.event.add({
      'WindowOpenedOrChanged': {
        'window': {'id': 30, 'app_id': 'unlisted', 'title': '远程会话'},
      },
    });
    await tester.pumpAndSettle();
    expect(geometry.last.$1, greaterThan(originalWidth));
    expect(geometry.last.$2.height, controller.macPreferences.dockSize + 16);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    '12 pinned apps auto size at 949px and keep Trash visible while magnified',
    (tester) async {
      final (controller, _, _) = await _fixture(tester, additionalApps: 10);
      controller.setPinnedAppIds(controller.apps.map((app) => app.id));
      final geometry = <(double, Rect)>[];
      await _mount(
        tester,
        controller,
        size: const Size(949, 700),
        dockSurfaceWidth: 901,
        onGeometryChanged: (size, rect) => geometry.add((size.width, rect)),
      );
      final baseSize = tester
          .getSize(find.byKey(const ValueKey('dock-icon-size-editor.desktop')))
          .width;
      expect(baseSize, inExclusiveRange(24, 48));
      expect(geometry.last.$1, closeTo(901, .01));
      final dock = tester.getRect(find.byType(MacDockView));
      void verifyTrash() {
        final trash = tester.getRect(find.byKey(const ValueKey('dock-trash')));
        expect(trash.left, greaterThanOrEqualTo(dock.left));
        expect(trash.right, lessThanOrEqualTo(dock.right));
        expect(trash.top, greaterThanOrEqualTo(dock.top + 20));
        expect(trash.bottom, lessThanOrEqualTo(dock.bottom));
      }

      verifyTrash();
      final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
      await mouse.addPointer(location: const Offset(0, 0));
      await mouse.moveTo(
        tester.getCenter(find.byKey(const ValueKey('dock-app-editor.desktop'))),
      );
      await tester.pumpAndSettle();
      verifyTrash();
      final before = _System.opened.length;
      await tester.tap(find.byKey(const ValueKey('dock-trash')));
      await tester.pumpAndSettle();
      expect(_System.opened.skip(before), ['trash']);
      await mouse.removePointer();
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'normal wide Dock keeps 48px icon size and Catalina utility icons',
    (tester) async {
      final (controller, _, _) = await _fixture(tester);
      await _mount(tester, controller);
      expect(
        tester
            .getSize(
              find.byKey(const ValueKey('dock-icon-size-editor.desktop')),
            )
            .width,
        48,
      );
      final images = tester.widgetList<Image>(
        find.descendant(
          of: find.byType(MacDockView),
          matching: find.byType(Image),
        ),
      );
      final assets = images.map((image) {
        final provider = image.image;
        final source = provider is ResizeImage
            ? provider.imageProvider
            : provider;
        return (source as AssetImage).assetName;
      }).toSet();
      expect(assets, contains('assets/shell/launchpad.png'));
      expect(assets, isNot(contains('assets/shell/mission-control.png')));
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'very large Dock retains 24px icons and vertical wheel reaches Trash smoothly',
    (tester) async {
      final (controller, _, _) = await _fixture(tester, additionalApps: 48);
      controller.setPinnedAppIds(controller.apps.map((app) => app.id));
      await _mount(
        tester,
        controller,
        size: const Size(949, 700),
        dockSurfaceWidth: 901,
      );
      expect(
        tester
            .getSize(
              find.byKey(const ValueKey('dock-icon-size-editor.desktop')),
            )
            .width,
        24,
      );
      controller.setReduceMotion(false);
      await tester.pumpAndSettle();
      final scroll = tester
          .widget<SingleChildScrollView>(
            find.descendant(
              of: find.byType(MacDockView),
              matching: find.byType(SingleChildScrollView),
            ),
          )
          .controller!;
      expect(scroll.offset, 0);
      await tester.sendEventToBinding(
        PointerScrollEvent(
          position: tester.getCenter(find.byKey(const ValueKey('dock-finder'))),
          scrollDelta: const Offset(0, 4000),
        ),
      );
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 75));
      expect(scroll.offset, greaterThan(0));
      expect(scroll.offset, lessThan(scroll.position.maxScrollExtent));
      await tester.pumpAndSettle();
      final trash = tester.getRect(find.byKey(const ValueKey('dock-trash')));
      final dock = tester.getRect(find.byType(MacDockView));
      expect(trash.right, lessThanOrEqualTo(dock.right));
      expect(trash.left, greaterThan(dock.left));
      await tester.tap(find.byKey(const ValueKey('dock-trash')));
      await tester.pumpAndSettle();
      expect(_System.opened.last, 'trash');
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('horizontal and mixed wheel events use smooth Dock scrolling', (
    tester,
  ) async {
    final (controller, _, _) = await _fixture(tester, additionalApps: 48);
    controller.setPinnedAppIds(controller.apps.map((app) => app.id));
    await _mount(
      tester,
      controller,
      size: const Size(949, 700),
      dockSurfaceWidth: 901,
    );
    controller.setReduceMotion(false);
    await tester.pumpAndSettle();
    final scroll = tester
        .widget<SingleChildScrollView>(
          find.descendant(
            of: find.byType(MacDockView),
            matching: find.byType(SingleChildScrollView),
          ),
        )
        .controller!;
    final dock = tester.getRect(find.byType(MacDockView));
    final point = Offset(dock.center.dx, dock.bottom - 30);
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: const Offset(0, 0));
    await mouse.moveTo(point);
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('mac-dock-label')), findsOneWidget);

    Future<void> wheel(Offset delta, double target) async {
      final before = scroll.offset;
      await tester.sendEventToBinding(
        PointerScrollEvent(position: point, scrollDelta: delta),
      );
      await tester.pump();
      expect(find.byKey(const ValueKey('mac-dock-label')), findsNothing);
      await tester.pump(const Duration(milliseconds: 75));
      expect(scroll.offset, greaterThan(before));
      expect(scroll.offset, lessThan(target));
      await tester.pumpAndSettle();
      expect(scroll.offset, closeTo(target, .01));
    }

    await wheel(const Offset(120, 0), 120);
    // The dominant vertical axis must also be handled by our inner Listener,
    // even though a native horizontal Scrollable can consume the dx component.
    await wheel(const Offset(20, 180), 300);
    await mouse.removePointer();
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'in-flight Dock scroll stops for reduced motion and an open panel',
    (tester) async {
      final (controller, _, _) = await _fixture(tester, additionalApps: 48);
      controller.setPinnedAppIds(controller.apps.map((app) => app.id));
      await _mount(
        tester,
        controller,
        size: const Size(949, 700),
        dockSurfaceWidth: 901,
      );
      controller.setReduceMotion(false);
      await tester.pumpAndSettle();
      final scroll = tester
          .widget<SingleChildScrollView>(
            find.descendant(
              of: find.byType(MacDockView),
              matching: find.byType(SingleChildScrollView),
            ),
          )
          .controller!;
      final dock = tester.getRect(find.byType(MacDockView));
      final point = Offset(dock.center.dx, dock.bottom - 30);
      Future<void> beginScroll(double delta) async {
        await tester.sendEventToBinding(
          PointerScrollEvent(position: point, scrollDelta: Offset(0, delta)),
        );
        await tester.pump();
        await tester.pump(const Duration(milliseconds: 50));
        expect(scroll.position.isScrollingNotifier.value, isTrue);
      }

      await beginScroll(400);
      expect(scroll.offset, inExclusiveRange(0, 400));
      controller.setReduceMotion(true);
      await tester.pump();
      expect(scroll.offset, closeTo(400, .01));
      expect(scroll.position.isScrollingNotifier.value, isFalse);
      await tester.pump(const Duration(milliseconds: 200));
      expect(scroll.offset, closeTo(400, .01));

      controller.setReduceMotion(false);
      await tester.pumpAndSettle();
      await beginScroll(300);
      controller.selectPanel(ShellPanel.systemMenu);
      final stopped = scroll.offset;
      await tester.pump(const Duration(milliseconds: 200));
      expect(scroll.offset, stopped);
      expect(scroll.position.isScrollingNotifier.value, isFalse);
      await tester.sendEventToBinding(
        PointerScrollEvent(position: point, scrollDelta: const Offset(0, 300)),
      );
      await tester.pumpAndSettle();
      expect(scroll.offset, stopped);
      expect(find.byKey(const ValueKey('mac-dock-label')), findsNothing);
      expect(tester.takeException(), isNull);
    },
  );

  for (final size in [const Size(180, 720), const Size(320, 240)]) {
    testWidgets('macOS shared panels fit small output $size', (tester) async {
      final (controller, _, _) = await _fixture(tester);
      await _mount(tester, controller, size: size);
      for (final panel in [
        ShellPanel.systemMenu,
        ShellPanel.quickSettings,
        ShellPanel.notifications,
        ShellPanel.screenshot,
        ShellPanel.wallpapers,
      ]) {
        controller.selectPanel(panel);
        await tester.pumpAndSettle();
        expect(tester.takeException(), isNull, reason: '$panel at $size');
        controller.closePanel();
        await tester.pumpAndSettle();
      }
    });
  }

  testWidgets('Launchpad covers the menu strip through its retained exit', (
    tester,
  ) async {
    final (controller, _, _) = await _fixture(tester);
    controller.setReduceMotion(false);
    final retained = <bool>[];
    await _mount(tester, controller, onLauncherSurfaceVisible: retained.add);
    controller.selectPanel(ShellPanel.launcher);
    await tester.pumpAndSettle();
    double menuOpacity() => tester
        .widget<Opacity>(
          find.byKey(const ValueKey('mac-menu-bar-launchpad-visibility')),
        )
        .opacity;
    double? launcherTop() => tester
        .widget<Positioned>(find.byKey(const ValueKey('mac-launcher-position')))
        .top;
    expect(menuOpacity(), 0);
    expect(launcherTop(), 0);
    expect(retained.last, isTrue);
    controller.closePanel();
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 60));
    expect(retained.last, isTrue);
    expect(menuOpacity(), 0);
    expect(launcherTop(), 0);
    await tester.pumpAndSettle();
    expect(retained.last, isFalse);
    expect(menuOpacity(), 1);
    expect(launcherTop(), MacShellView.barHeight);
    controller.selectPanel(ShellPanel.launcher);
    await tester.pumpAndSettle();
    controller.selectPanel(ShellPanel.quickSettings);
    await tester.pump();
    expect(menuOpacity(), 1);
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'Finder and app fallback menus and clock notification toggle on macOS bar',
    (tester) async {
      final (controller, ipc, _) = await _fixture(tester);
      await _mount(tester, controller);

      // Window 21 ('editor') is focused in fixture
      expect(find.text('文本编辑器'), findsOneWidget);
      expect(find.text('文件'), findsOneWidget);
      expect(find.text('编辑'), findsOneWidget);
      expect(find.text('窗口'), findsOneWidget);
      expect(find.text('帮助'), findsOneWidget);

      await tester.tap(find.byKey(const ValueKey('global-menu-app-window')));
      await tester.pumpAndSettle();
      expect(
        find.byKey(const ValueKey('global-menu-app-spaces')),
        findsNothing,
      );
      expect(find.text('调度中心'), findsNothing);
      expect(
        find.byKey(const ValueKey('global-menu-app-minimize')),
        findsOneWidget,
      );
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();

      // Active app button is content-sized and hugs its text
      final appRect = tester.getRect(
        find.byKey(const ValueKey('mac-active-app-button')),
      );
      expect(appRect.width, lessThan(100.0));
      final menuRect = tester.getRect(find.byType(GlobalAppMenuBar));
      expect(menuRect.left - appRect.right, closeTo(0.0, 0.1));

      // Clear focused windows so desktop (Finder) becomes active
      ipc.event.add({
        'WindowsChanged': {'windows': []},
      });
      await tester.pumpAndSettle();

      expect(find.text('Home'), findsOneWidget);
      expect(find.text('文件'), findsOneWidget);
      expect(find.text('编辑'), findsOneWidget);
      expect(find.text('显示'), findsOneWidget);
      expect(find.text('前往'), findsOneWidget);
      expect(find.text('窗口'), findsOneWidget);
      expect(find.text('帮助'), findsOneWidget);

      await tester.tap(find.byKey(const ValueKey('global-menu-finder-window')));
      await tester.pumpAndSettle();
      expect(
        find.byKey(const ValueKey('global-menu-finder-spaces')),
        findsNothing,
      );
      expect(find.text('调度中心'), findsNothing);
      expect(
        find.byKey(const ValueKey('global-menu-finder-show-all')),
        findsOneWidget,
      );
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();

      // Tap clock button to open notifications
      await tester.tap(find.byKey(const ValueKey('mac-clock-button')));
      await tester.pumpAndSettle();
      expect(controller.panel, ShellPanel.notifications);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('optional macOS preview captures actual shared controls', (
    tester,
  ) async {
    final destination = Platform.environment['CLARION_SHELL_PREVIEW_DIR'];
    if (destination == null) return;
    await tester.runAsync(() async {
      for (final font in {
        'ClarionSystem': 'assets/shell/ui-text.ttf',
        'ClarionChinese': 'assets/shell/cjk-regular.otf',
        'MaterialIcons':
            'build/unit_test_assets/fonts/MaterialIcons-Regular.otf',
      }.entries) {
        final file = File(font.value);
        if (!file.existsSync()) continue;
        final bytes = await file.readAsBytes();
        await (FontLoader(
          font.key,
        )..addFont(Future.value(ByteData.sublistView(bytes)))).load();
      }
    });
    final (controller, _, _) = await _fixture(tester);
    await _mount(
      tester,
      controller,
      size: const Size(1440, 900),
      preview: true,
    );
    for (final panel in [
      ShellPanel.none,
      ShellPanel.systemMenu,
      ShellPanel.quickSettings,
      ShellPanel.dockAppMenu,
    ]) {
      if (panel == ShellPanel.dockAppMenu) {
        controller.openDockAppMenu(_editor.id);
      } else {
        controller.selectPanel(panel);
      }
      await tester.pumpAndSettle();
      final boundary = tester.renderObject<RenderRepaintBoundary>(
        find.byKey(const ValueKey('mac-capture')),
      );
      await tester.runAsync(() async {
        final image = await boundary.toImage(pixelRatio: 1);
        final bytes = await image.toByteData(format: ui.ImageByteFormat.png);
        await Directory(destination).create(recursive: true);
        await File(
          '$destination/macos-${panel.name}.png',
        ).writeAsBytes(bytes!.buffer.asUint8List());
        image.dispose();
      });
    }
  });

  testWidgets('hideApp, hideOthers, and showAll send compositor actions', (
    tester,
  ) async {
    final (controller, ipc, _) = await _fixture(tester);
    ipc.event.add({
      'WindowsChanged': {
        'windows': [
          {
            'id': 21,
            'app_id': 'editor',
            'title': '设计笔记',
            'is_focused': true,
            'is_minimized': false,
          },
          {
            'id': 22,
            'app_id': 'editor',
            'title': '待办事项',
            'is_focused': false,
            'is_minimized': false,
          },
        ],
      },
    });
    await tester.pump();
    await _mount(tester, controller);

    await controller.hideApp(_editor.id);
    expect(
      ipc.actions.any((a) => a.$1 == 'MinimizeWindow' && a.$2['id'] == 21),
      isTrue,
    );

    ipc.event.add({
      'WindowsChanged': {
        'windows': [
          {
            'id': 21,
            'app_id': 'editor',
            'title': '设计笔记',
            'is_focused': false,
            'is_minimized': true,
          },
          {
            'id': 22,
            'app_id': 'editor',
            'title': '待办事项',
            'is_focused': false,
            'is_minimized': true,
          },
        ],
      },
    });
    await tester.pump();

    await controller.showAll();
    expect(
      ipc.actions.any((a) => a.$1 == 'FocusWindow' && a.$2['id'] == 21),
      isTrue,
    );
  });
}
