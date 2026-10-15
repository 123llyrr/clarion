// SPDX-License-Identifier: GPL-3.0-or-later
// External harness starts test/fixtures/qt_menu_app.cpp on a private session bus
// and nested compositor. Required QA env: REALAND_MENU_QA_PID, _RESULT, _BUS_ID,
// plus the normal isolation guards and QT_QPA_PLATFORM=wayland. Run once with
// QT_QPA_PLATFORMTHEME unset and once with kde; never change the live session.
import 'dart:convert';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:dbus/dbus.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:realand_shell/app_menu_models.dart';
import 'package:realand_shell/main.dart';
import 'package:realand_shell/models.dart';
import 'package:realand_shell/preferences.dart';
import 'package:realand_shell/services/desktop_apps.dart';
import 'package:realand_shell/services/global_app_menu_service.dart';
import 'package:realand_shell/services/system_service.dart';
import 'package:realand_shell/services/wallpaper_service.dart';
import 'package:realand_shell/state/shell_controller.dart';

const _window = MethodChannel('realand.shell/window');
const _applicationId = 'org.test.RealandQtMenuFixture';

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
      'menus': [
        for (final root in menus)
          {
            'id': root.id,
            'label': root.label,
            'enabled': root.enabled,
            'submenu': root.isSubmenu,
            'children': [
              for (final child in root.children)
                {'label': child.label, 'enabled': child.enabled},
            ],
          },
      ],
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
    DesktopApp(id: _applicationId, name: 'Qt Menu Fixture', desktopFile: ''),
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
      // The native layer/keyboard properties describe the requested state.
      // wl_keyboard.enter may arrive after that allocation. Sending wtype now
      // would deliver Escape to the previously focused Qt window. GTK's active
      // state is updated by the actual enter/leave, so require real focus before
      // testing keyboard input; no timed delay or failed-key retry is needed.
      if (expanded && state['focused'] != true) continue;
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
    'real Qt Wayland metadata and DBusMenu QAction activate in the native shell',
    (tester) async {
      final env = Platform.environment;
      expect(Platform.isLinux, isTrue);
      expect(env['REALAND_SHELL_ISOLATED_TEST'], '1');
      for (final pair in [
        ('WAYLAND_DISPLAY', 'REALAND_SHELL_PARENT_WAYLAND_DISPLAY'),
        ('REALAND_SOCKET', 'REALAND_SHELL_PARENT_SOCKET'),
        (
          'DBUS_SESSION_BUS_ADDRESS',
          'REALAND_SHELL_PARENT_DBUS_SESSION_BUS_ADDRESS',
        ),
      ]) {
        expect(env[pair.$1], isNotEmpty, reason: pair.$1);
        expect(env[pair.$2], isNotNull, reason: pair.$2);
        expect(env[pair.$1], isNot(env[pair.$2]), reason: 'private ${pair.$1}');
      }
      final fixturePid = int.parse(env['REALAND_MENU_QA_PID']!);
      final result = File(env['REALAND_MENU_QA_RESULT']!);
      final captures = Directory(env['REALAND_SHELL_CAPTURE_DIR']!);
      final registrarReady = File(env['REALAND_MENU_QA_READY']!);
      final fixtureState = File('${result.path}.state.json');
      expect(env['QT_QPA_PLATFORM'], 'wayland');
      expect(env['QT_QPA_PLATFORMTHEME'] ?? '', anyOf('', 'kde'));
      Map<String, dynamic> readFixtureState() => fixtureState.existsSync()
          ? Map<String, dynamic>.from(
              jsonDecode(fixtureState.readAsStringSync()) as Map,
            )
          : {};
      expect(result.path, startsWith('${captures.absolute.path}/'));
      expect(registrarReady.path, startsWith('${captures.absolute.path}/'));
      expect(registrarReady.existsSync(), isFalse);
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
      expect(env['REALAND_MENU_QA_BUS_ID'], isNotEmpty);
      expect(
        busId,
        env['REALAND_MENU_QA_BUS_ID'],
        reason: 'known private bus GUID',
      );
      final menuBus = DBusClient.session();
      final menuEvents = <Map<String, dynamic>>[];
      final focusEvents = <Map<String, dynamic>>[];
      final keyEvents = <Map<String, dynamic>>[];
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
        'platformTheme': env['QT_QPA_PLATFORMTHEME'] ?? 'default',
        'menuEvents': menuEvents,
        'focusEvents': focusEvents,
        'keyEvents': keyEvents,
      };
      bool observeKey(KeyEvent event) {
        keyEvents.add({
          'time': DateTime.now().toIso8601String(),
          'type': event.runtimeType.toString(),
          'logical': event.logicalKey.keyLabel,
          'physical': event.physicalKey.debugName,
          'primaryFocus': FocusManager.instance.primaryFocus?.debugLabel,
        });
        return false;
      }

      void observeFocus() {
        if (focusEvents.length >= 1024) return;
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
      HardwareKeyboard.instance.addHandler(observeKey);
      try {
        await controller.prepareAppMenus();
        expect(await observer.getNameOwner(appMenuRegistrarName), isNotNull);
        await registrarReady.writeAsString(busId, flush: true);
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
              controller.windows.where((w) => w.pid == fixturePid).length ==
                  2 &&
              controller.windows
                  .where((w) => w.pid == fixturePid)
                  .every(
                    (w) => w.appMenu?.protocol == AppMenuProtocol.dbusmenu,
                  ),
          'two real Qt windows with canonical metadata in IPC',
        );
        await _until(
          tester,
          () => readFixtureState()['menusReady'] == true,
          'Qt built its real platform menu after registrar became available',
        );
        evidence['qtBefore'] = readFixtureState();
        final first = controller.windows.singleWhere(
          (w) => w.title == 'Realand Qt Menu Fixture',
        );
        final second = controller.windows.singleWhere(
          (w) => w.title == 'Realand Qt Second Window',
        );
        expect(first.appId, _applicationId);
        expect(
          first.appMenu,
          isNotNull,
          reason: 'org_kde_kwin_appmenu set_address reached real IPC',
        );
        expect(second.appMenu, isNotNull);
        expect(first.appMenu!.protocol, AppMenuProtocol.dbusmenu);
        expect(second.appMenu!.protocol, AppMenuProtocol.dbusmenu);
        expect(first.appMenu!.applicationActionsPath, isNull);
        expect(first.appMenu!.windowActionsPath, isNull);
        expect(first.appMenu!.menuPath, startsWith('/MenuBar/'));
        expect(second.appMenu!.menuPath, startsWith('/MenuBar/'));
        expect(first.appMenu!.menuPath, isNot(second.appMenu!.menuPath));
        evidence['windows'] = [
          for (final w in [first, second])
            {
              'id': w.id,
              'pid': w.pid,
              'appId': w.appId,
              'busName': w.appMenu!.busName,
              'menuPath': w.appMenu!.menuPath,
              'protocol': w.appMenu!.protocol.name,
            },
        ];
        Future<void> focus(int id, {bool compact = false}) async {
          await controller.focusWindow(id);
          await _until(
            tester,
            () =>
                controller.appMenu.available &&
                controller.focusedWindow?.id == id &&
                controller.appMenu.target?.windowId == id &&
                (compact
                    ? find
                          .byKey(const ValueKey('material-app-menu-entry'))
                          .evaluate()
                          .isNotEmpty
                    : find.text('File').evaluate().isNotEmpty),
            'real focused menu $id',
          );
          // Canonical GetLayout can complete before the notified widget frame.
          // An old File label still in the tree is not proof that the new menu
          // callbacks and focus state have been laid out for real input.
          await tester.pump(const Duration(milliseconds: 30));
          service.record('focusedMenuFrame:$id');
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
        // The native Overlay owns keyboard focus; the menu retains its Qt target.
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
          'real first-window QAction activation',
        );
        await _until(
          tester,
          () => readFixtureState()['lastActivatedWindow'] == first.title,
          'first focused Qt QAction belongs to the first window',
        );
        await _native(tester, false);
        expect(controller.appMenuOpen, isFalse);
        await focus(first.id);
        service.record('beforeEditTap');
        await tester.tap(find.text('编辑'));
        await _native(tester, true);
        expect(find.text('不可用'), findsOneWidget);
        final disabled = tester.widget<MenuItemButton>(
          find.widgetWithText(MenuItemButton, '不可用'),
        );
        expect(disabled.onPressed, isNull);
        service.record('beforeMacEscape');
        evidence['focusBeforeMacEscape'] =
            FocusManager.instance.primaryFocus?.debugLabel;
        final key = await Process.run(env['REALAND_SHELL_WTYPE'] ?? 'wtype', [
          '-k',
          'Escape',
        ]).timeout(const Duration(seconds: 5));
        expect(key.exitCode, 0, reason: '${key.stderr}');
        evidence['escape'] = await _native(tester, false);
        expect(controller.appMenuOpen, isFalse);
        expect(_count(result), 1);

        final escapeCycles = <Map<String, dynamic>>[];
        evidence['escapeCycles'] = escapeCycles;
        for (var cycle = 0; cycle < 10; cycle++) {
          await focus(first.id);
          await tester.tap(find.text('编辑'));
          final expandedState = await _native(tester, true);
          final cycleEvidence = <String, dynamic>{
            'cycle': cycle,
            'sentAt': DateTime.now().toIso8601String(),
            'keyEventsBefore': keyEvents.length,
            'primaryFocus': FocusManager.instance.primaryFocus?.debugLabel,
            'nativeExpanded': expandedState,
            'focusedWindow': controller.focusedWindow?.id,
          };
          escapeCycles.add(cycleEvidence);
          final escape = await Process.run(
            env['REALAND_SHELL_WTYPE'] ?? 'wtype',
            ['-k', 'Escape'],
          ).timeout(const Duration(seconds: 5));
          expect(escape.exitCode, 0, reason: '${escape.stderr}');
          cycleEvidence['nativeCollapsed'] = await _native(tester, false);
          cycleEvidence['closedAt'] = DateTime.now().toIso8601String();
          cycleEvidence['keyEventsAfter'] = keyEvents.length;
          expect(controller.appMenuOpen, isFalse);
          expect(_count(result), 1);
        }

        await focus(first.id);
        final stale = controller.appMenu.menus
            .firstWhere((m) => m.label == 'File')
            .children
            .singleWhere((m) => m.label == '验证动作');
        await focus(second.id);
        expect(
          controller.appMenu.target!.export!.menuPath,
          second.appMenu!.menuPath,
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
        await _until(
          tester,
          () => readFixtureState()['lastActivatedWindow'] == second.title,
          'second focused Qt QAction belongs to the second window',
        );
        await _native(tester, false);
        final materialPosition = env['QT_QPA_PLATFORMTHEME'] == 'kde'
            ? BarPosition.top
            : BarPosition.bottom;
        controller.setBarPosition(materialPosition);
        controller.setShellStyle(ShellStyle.material);
        evidence['material'] = await _native(tester, false, style: 'material');
        evidence['materialBarPosition'] = materialPosition.name;
        expect(
          (evidence['material'] as Map)['barPosition'],
          materialPosition.name,
        );
        await focus(second.id, compact: true);
        expect(
          controller.appMenu.target!.export!.protocol,
          AppMenuProtocol.dbusmenu,
        );
        await _until(
          tester,
          () => !controller.appMenu.loading,
          'Material explicit menu load completes without hosting a registrar',
        );
        final releaseDeadline = DateTime.now().add(const Duration(seconds: 3));
        while (await observer.getNameOwner(appMenuRegistrarName) != null) {
          if (DateTime.now().isAfter(releaseDeadline)) {
            fail('Material did not release its canonical menu registrar');
          }
          await tester.pump(const Duration(milliseconds: 30));
        }
        await _until(
          tester,
          () => readFixtureState()['registrarPresent'] == false,
          'Qt observes Material registrar release',
        );
        evidence['qtMaterial'] = readFixtureState();
        // Report the unmodified application's actual local menu state. Stock Qt
        // caches native-menu support; releasing the registrar alone does not
        // promise existing clients rebuild their local QMenuBar. Do not force
        // setNativeMenuBar(false) in the fixture to manufacture that guarantee.
        evidence['materialLocalMenusRetained'] =
            (readFixtureState()['windows'] as List).every(
              (w) => (w as Map)['localMenuBarVisible'] == true,
            );
        expect(_count(result), 2);
        expect(find.text('File'), findsNothing);
        await tester.tap(find.byKey(const ValueKey('material-app-menu-entry')));
        evidence['materialOpen'] = await _native(
          tester,
          true,
          style: 'material',
        );
        await _until(
          tester,
          () => find.text('File').evaluate().isNotEmpty,
          'Material retains the real Qt menu groups',
        );
        final entryRect = tester.getRect(
          find.byKey(const ValueKey('material-app-menu-entry')),
        );
        final fileRect = tester.getRect(find.text('File'));
        evidence['materialPopupGeometry'] = {
          'entry': [
            entryRect.left,
            entryRect.top,
            entryRect.right,
            entryRect.bottom,
          ],
          'file': [
            fileRect.left,
            fileRect.top,
            fileRect.right,
            fileRect.bottom,
          ],
        };
        if (materialPosition == BarPosition.bottom) {
          final viewportHeight =
              ((evidence['materialOpen'] as Map)['height'] as num).toDouble();
          expect(entryRect.bottom, closeTo(viewportHeight, 1.01));
          expect(
            fileRect.bottom,
            lessThanOrEqualTo(entryRect.top + 1.01),
            reason:
                'footer popup opens toward the desktop rather than below it',
          );
        } else {
          expect(entryRect.top, closeTo(0, 1.01));
          expect(fileRect.top, greaterThanOrEqualTo(entryRect.bottom - 1.01));
        }
        final materialEscape = await Process.run(
          env['REALAND_SHELL_WTYPE'] ?? 'wtype',
          ['-k', 'Escape'],
        ).timeout(const Duration(seconds: 5));
        expect(materialEscape.exitCode, 0, reason: '${materialEscape.stderr}');
        evidence['materialEscape'] = await _native(
          tester,
          false,
          style: 'material',
        );
        expect(controller.appMenuOpen, isFalse);
        expect(_count(result), 2);
        await focus(second.id, compact: true);
        await tester.tap(find.byKey(const ValueKey('material-app-menu-entry')));
        await _native(tester, true, style: 'material');
        await _until(
          tester,
          () => find.text('File').evaluate().isNotEmpty,
          'Material menu opens again after a real Escape',
        );
        await tester.tap(find.text('File'));
        await tester.pump(const Duration(milliseconds: 30));
        await _until(
          tester,
          () => find.text('验证动作').evaluate().isNotEmpty,
          'Material real Qt action popup',
        );
        await _native(tester, true, style: 'material');
        expect(controller.menuFocusedWindow!.id, second.id);
        service.record('beforeMaterialActionTap');
        await tester.tap(find.text('验证动作'));
        await _until(
          tester,
          () => _count(result) == 3,
          'Material compact menu dispatches the actual QAction',
        );
        await _until(
          tester,
          () => readFixtureState()['lastActivatedWindow'] == second.title,
          'Material action belongs to the selected Qt window',
        );
        await _native(tester, false, style: 'material');
        expect(controller.appMenuOpen, isFalse);
        expect(await observer.getNameOwner(appMenuRegistrarName), isNull);
        expect(find.text('File'), findsNothing);
        expect(tester.takeException(), isNull);
        evidence['verifiedActions'] = _count(result);
        evidence['passed'] = true;
        binding.reportData = evidence;
      } finally {
        HardwareKeyboard.instance.removeHandler(observeKey);
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
          '${captures.path}/qt-global-menu-native-evidence.json',
        ).writeAsString(
          '${const JsonEncoder.withIndent('  ').convert(evidence)}\n',
        );
      }
    },
    timeout: const Timeout(Duration(minutes: 2)),
  );
}
