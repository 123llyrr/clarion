// SPDX-License-Identifier: GPL-3.0-or-later
// External harness starts test/fixtures/gtk_menu_app.c on a private session bus
// and nested compositor. Required QA env: CLARION_MENU_QA_PID, _RESULT, _BUS_ID,
// plus CLARION_SHELL_PARENT_DBUS_SESSION_BUS_ADDRESS and normal isolation guards.
import 'dart:convert';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:dbus/dbus.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:clarion_shell/app_menu_models.dart';
import 'package:clarion_shell/main.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/preferences.dart';
import 'package:clarion_shell/services/desktop_apps.dart';
import 'package:clarion_shell/services/global_app_menu_service.dart';
import 'package:clarion_shell/services/system_service.dart';
import 'package:clarion_shell/services/wallpaper_service.dart';
import 'package:clarion_shell/state/shell_controller.dart';

const _window = MethodChannel('clarion.shell/window');
const _applicationId = 'org.test.ClarionMenuFixture';

class _ObservedMenus extends GlobalAppMenuService {
  _ObservedMenus({required super.bus, required this.events});
  final List<Map<String, dynamic>> events;
  GlobalMenuItem? _currentItem(String id, List<GlobalMenuItem> items) {
    for (final item in items) {
      if (item.id == id) return item;
      final nested = _currentItem(id, item.children);
      if (nested != null) return nested;
    }
    return null;
  }

  void record(String stage, {GlobalMenuItem? item, bool? result}) {
    events.add({
      'stage': stage,
      'time': DateTime.now().toIso8601String(),
      'revision': revision,
      'windowId': target?.windowId,
      'pid': target?.pid,
      'menuPath': target?.export?.menuPath,
      'loading': loading,
      'error': error,
      'available': available,
      if (item != null) ...{
        'itemId': item.id,
        'itemLabel': item.label,
        'itemEnabled': item.enabled,
        'itemIsCurrent': identical(_currentItem(item.id, menus), item),
      },
      'result': ?result,
    });
  }

  @override
  Future<bool> activate(GlobalMenuItem item) async {
    record('activateRequested', item: item);
    final result = await super.activate(item);
    record('activateCompleted', item: item, result: result);
    return result;
  }

  @override
  Future<void> focusedTarget({
    int? pid,
    String appId = '',
    int? windowId,
    int? x11WindowId,
    AppMenuExport? export,
  }) async {
    record('targetRequested:$windowId');
    await super.focusedTarget(
      pid: pid,
      appId: appId,
      windowId: windowId,
      x11WindowId: x11WindowId,
      export: export,
    );
    record('targetCompleted:$windowId');
  }
}

class _Apps extends DesktopApps {
  @override
  Future<List<DesktopApp>> load() async => const [
    DesktopApp(id: _applicationId, name: 'GTK Menu Fixture', desktopFile: ''),
  ];
}

class _System extends SystemService {
  @override
  Future<SystemSnapshot> read() async => const SystemSnapshot();
}

class _Wallpapers extends WallpaperService {
  @override
  Future<void> restore({bool applySaved = false}) async {}
}

// The live test binding assumes every device packet already has a RenderView.
// Native Dock creation/removal briefly precedes/follows its Flutter attachment.
class _DeviceFilter {
  _DeviceFilter(this.binding) {
    original = binding.platformDispatcher.onPointerDataPacket;
    binding.platformDispatcher.onPointerDataPacket = (packet) {
      final views = binding.renderViews
          .map((v) => v.flutterView.viewId)
          .toSet();
      final valid = packet.data
          .where((data) => views.contains(data.viewId))
          .toList();
      if (valid.isNotEmpty) original?.call(ui.PointerDataPacket(data: valid));
    };
  }
  final IntegrationTestWidgetsFlutterBinding binding;
  ui.PointerDataPacketCallback? original;
  void restore() => binding.platformDispatcher.onPointerDataPacket = original;
}

Future<void> _until(
  WidgetTester tester,
  bool Function() ready,
  String reason,
) async {
  final deadline = DateTime.now().add(const Duration(seconds: 15));
  while (!ready()) {
    if (DateTime.now().isAfter(deadline)) fail('Timed out: $reason');
    await tester.pump(const Duration(milliseconds: 30));
  }
}

Future<Map<String, dynamic>> _native(
  WidgetTester tester,
  bool expanded, {
  String style = 'macos',
}) async {
  final deadline = DateTime.now().add(const Duration(seconds: 15));
  Map<String, dynamic> state = {};
  do {
    await tester.pump(const Duration(milliseconds: 30));
    state = Map<String, dynamic>.from(
      (await _window.invokeMapMethod<String, dynamic>('windowState'))!,
    );
    final barHeight = style == 'macos' ? 22 : 40;
    if (state['style'] == style &&
        state['expanded'] == expanded &&
        state['height'] ==
            (expanded
                ? (state['monitorGeometry'] as Map)['height']
                : barHeight)) {
      // GTK allocation is visible to windowState before its engine metrics
      // packet is laid out. A real menu tap must wait for the same main view's
      // Flutter viewport and RenderView to reflect that allocation as well.
      await tester.pump();
      final render = WidgetsBinding.instance.renderViews
          .where((view) => view.flutterView.viewId == state['mainViewId'])
          .firstOrNull;
      if (render == null) continue;
      final viewport =
          render.flutterView.physicalSize / render.flutterView.devicePixelRatio;
      final expected = Size(
        (state['width'] as num).toDouble(),
        (state['height'] as num).toDouble(),
      );
      bool matches(Size size) =>
          (size.width - expected.width).abs() <= 1.01 &&
          (size.height - expected.height).abs() <= 1.01;
      if (!matches(viewport) || !matches(render.size)) continue;
      expect(state['keyboard'], expanded ? 'Exclusive' : 'None');
      expect(state['layer'], expanded ? 'Overlay' : 'Top');
      expect(state['exclusiveZone'], barHeight);
      expect(state['inputHeight'], expanded ? state['height'] : barHeight);
      return state;
    }
  } while (DateTime.now().isBefore(deadline));
  fail('Native menu state did not settle: $state');
}

int _count(File result) => result.existsSync()
    ? int.tryParse(result.readAsStringSync().trim()) ?? -1
    : -1;

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  testWidgets(
    'real GTK metadata and DBus menus activate in the native macOS shell',
    (tester) async {
      final env = Platform.environment;
      expect(Platform.isLinux, isTrue);
      expect(env['CLARION_SHELL_ISOLATED_TEST'], '1');
      for (final pair in [
        ('WAYLAND_DISPLAY', 'CLARION_SHELL_PARENT_WAYLAND_DISPLAY'),
        ('CLARION_SOCKET', 'CLARION_SHELL_PARENT_SOCKET'),
        (
          'DBUS_SESSION_BUS_ADDRESS',
          'CLARION_SHELL_PARENT_DBUS_SESSION_BUS_ADDRESS',
        ),
      ]) {
        expect(env[pair.$1], isNotEmpty, reason: pair.$1);
        expect(env[pair.$2], isNotNull, reason: pair.$2);
        expect(env[pair.$1], isNot(env[pair.$2]), reason: 'private ${pair.$1}');
      }
      final fixturePid = int.parse(env['CLARION_MENU_QA_PID']!);
      final result = File(env['CLARION_MENU_QA_RESULT']!);
      final captures = Directory(env['CLARION_SHELL_CAPTURE_DIR']!);
      expect(result.path, startsWith('${captures.absolute.path}/'));
      expect(Directory('/proc/$fixturePid').existsSync(), isTrue);
      expect(_count(result), 0);
      final processEnvironment = File(
        '/proc/$fixturePid/environ',
      ).readAsStringSync().split('\u0000');
      expect(
        processEnvironment,
        contains('WAYLAND_DISPLAY=${env['WAYLAND_DISPLAY']}'),
      );
      expect(
        processEnvironment,
        contains('DBUS_SESSION_BUS_ADDRESS=${env['DBUS_SESSION_BUS_ADDRESS']}'),
      );

      final observer = DBusClient.session();
      final busId = await observer.getId().timeout(const Duration(seconds: 3));
      expect(env['CLARION_MENU_QA_BUS_ID'], isNotEmpty);
      expect(
        busId,
        env['CLARION_MENU_QA_BUS_ID'],
        reason: 'known private bus GUID',
      );
      final menuBus = DBusClient.session();
      final menuEvents = <Map<String, dynamic>>[];
      final focusEvents = <Map<String, dynamic>>[];
      final service = _ObservedMenus(bus: menuBus, events: menuEvents);
      final controller =
          ShellController(
              notifications: false,
              tray: false,
              powerProfilesEnabled: false,
              restoreWallpaperOnStart: false,
              desktopApps: _Apps(),
              system: _System(),
              wallpaperService: _Wallpapers(),
              appMenuService: service,
            )
            ..setShellStyle(ShellStyle.macos)
            ..setReduceMotion(true);
      const prefs = ShellPreferences(
        shellStyle: ShellStyle.macos,
        themeMode: ThemeMode.light,
        reduceMotion: true,
      );
      final devices = _DeviceFilter(binding);
      final evidence = <String, dynamic>{
        'privateBusId': busId,
        'fixturePid': fixturePid,
        'menuEvents': menuEvents,
        'focusEvents': focusEvents,
      };
      void observeFocus() {
        if (focusEvents.length >= 256) return;
        focusEvents.add({
          'time': DateTime.now().toIso8601String(),
          'focusedWindow': controller.focusedWindow?.id,
          'retainedMenuWindow': controller.menuFocusedWindow?.id,
          'appMenuOpen': controller.appMenuOpen,
          'menuRevision': service.revision,
          'targetWindow': service.target?.windowId,
          'loading': service.loading,
          'error': service.error,
        });
      }

      controller.addListener(observeFocus);
      try {
        await tester.pumpWidget(
          ShellHost(controller: controller, preferences: prefs, persist: false),
          wrapWithView: false,
        );
        await controller.start();
        await _native(tester, false);
        await _until(
          tester,
          () =>
              controller.connected &&
              controller.windows.where((w) => w.pid == fixturePid).length == 2,
          'two real GTK windows in IPC',
        );
        final first = controller.windows.singleWhere(
          (w) => w.title == 'Clarion GTK Menu Fixture',
        );
        final second = controller.windows.singleWhere(
          (w) => w.title == 'Clarion GTK Second Window',
        );
        expect(first.appId, _applicationId);
        expect(
          first.appMenu,
          isNotNull,
          reason: 'gtk_shell set_dbus_properties reached real IPC',
        );
        expect(second.appMenu, isNotNull);
        expect(
          first.appMenu!.windowActionsPath,
          isNot(second.appMenu!.windowActionsPath),
        );
        evidence['windows'] = [
          for (final w in [first, second])
            {
              'id': w.id,
              'pid': w.pid,
              'appId': w.appId,
              'busName': w.appMenu!.busName,
              'menuPath': w.appMenu!.menuPath,
              'windowActionsPath': w.appMenu!.windowActionsPath,
            },
        ];
        Future<void> focus(int id) async {
          await controller.focusWindow(id);
          await _until(
            tester,
            () =>
                controller.appMenu.available &&
                controller.appMenu.target?.windowId == id &&
                find.text('File').evaluate().isNotEmpty,
            'real focused menu $id',
          );
        }

        await focus(first.id);
        expect(controller.appMenu.target!.pid, fixturePid);
        expect(controller.appMenu.target!.x11WindowId, isNull);
        final owner = await observer.getNameOwner(first.appMenu!.busName);
        expect(owner, isNotNull);
        expect(await observer.getConnectionUnixProcessId(owner!), fixturePid);
        expect(
          controller.appMenu.menus.map((m) => m.label),
          containsAll(['File', '编辑']),
        );
        await tester.tap(find.text('File'));
        evidence['open'] = await _native(tester, true);
        await _until(
          tester,
          () => find.text('验证动作').evaluate().isNotEmpty,
          'real action popup',
        );
        // The native Overlay owns keyboard focus, while the menu retains its GTK target.
        expect(controller.menuFocusedWindow!.id, first.id);
        expect(controller.appMenu.target!.windowId, first.id);
        service.record('beforeFirstActionTap');
        await tester.tap(find.text('验证动作'));
        service.record('afterFirstActionTapBeforeFrame');
        await tester.pump();
        service.record('afterFirstActionTapFrame');
        await _until(
          tester,
          () => _count(result) == 1,
          'real app.verify activation',
        );
        await _native(tester, false);
        expect(controller.appMenuOpen, isFalse);
        await focus(first.id);
        await tester.tap(find.text('编辑'));
        await _native(tester, true);
        expect(find.text('不可用'), findsOneWidget);
        final disabled = tester.widget<MenuItemButton>(
          find.widgetWithText(MenuItemButton, '不可用'),
        );
        expect(disabled.onPressed, isNull);
        final key = await Process.run(env['CLARION_SHELL_WTYPE'] ?? 'wtype', [
          '-k',
          'Escape',
        ]).timeout(const Duration(seconds: 5));
        expect(key.exitCode, 0, reason: '${key.stderr}');
        evidence['escape'] = await _native(tester, false);
        expect(controller.appMenuOpen, isFalse);
        expect(_count(result), 1);

        await focus(first.id);
        final stale = controller.appMenu.menus
            .firstWhere((m) => m.label == 'File')
            .children
            .singleWhere((m) => m.label == '验证动作');
        await focus(second.id);
        expect(
          controller.appMenu.target!.export!.windowActionsPath,
          second.appMenu!.windowActionsPath,
        );
        expect(await controller.appMenu.activate(stale), isFalse);
        expect(_count(result), 1);
        await tester.tap(find.text('File'));
        await _native(tester, true);
        await tester.tap(find.text('验证动作'));
        await _until(
          tester,
          () => _count(result) == 2,
          'second focused exported action',
        );
        await _native(tester, false);
        controller.setShellStyle(ShellStyle.material);
        evidence['material'] = await _native(tester, false, style: 'material');
        expect(controller.appMenu.available, isFalse);
        await _until(
          tester,
          () => !controller.appMenu.loading,
          'Material cancels menu load',
        );
        final releaseDeadline = DateTime.now().add(const Duration(seconds: 3));
        while (await observer.getNameOwner(appMenuRegistrarName) != null) {
          if (DateTime.now().isAfter(releaseDeadline)) {
            fail('Material did not release its canonical menu registrar');
          }
          await tester.pump(const Duration(milliseconds: 30));
        }
        expect(find.text('File'), findsNothing);
        expect(tester.takeException(), isNull);
        evidence['verifiedActions'] = _count(result);
        evidence['passed'] = true;
        binding.reportData = evidence;
      } finally {
        controller.removeListener(observeFocus);
        await tester.pumpWidget(
          const ViewCollection(views: []),
          wrapWithView: false,
        );
        controller.dispose();
        await _window.invokeMethod<void>('setExpanded', false);
        await _window.invokeMethod<void>('setShellStyle', 'material');
        await observer.close();
        devices.restore();
        await File(
          '${captures.path}/global-menu-native-evidence.json',
        ).writeAsString(
          '${const JsonEncoder.withIndent('  ').convert(evidence)}\n',
        );
      }
    },
    timeout: const Timeout(Duration(minutes: 2)),
  );
}
