// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'package:dbus/dbus.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/app_menu_models.dart';
import 'package:clarion_shell/services/global_app_menu_service.dart';
import 'package:clarion_shell/ui/global_app_menu_bar.dart';

class _Bus {
  _Bus(this.directory, this.daemon, this.errors, this.address);
  final Directory directory;
  final Process daemon;
  final Future<void> errors;
  final DBusAddress address;
  final clients = <DBusClient>[];
  DBusClient client() {
    final c = DBusClient(address);
    clients.add(c);
    return c;
  }

  static Future<_Bus> create() async {
    final dir = await Directory.systemTemp.createTemp('clarion-app-menu-');
    final process = await Process.start('/usr/bin/dbus-daemon', [
      '--session',
      '--nofork',
      '--nopidfile',
      '--print-address=1',
      '--address=unix:path=${dir.path}/bus',
    ]);
    final errors = process.stderr.drain<void>();
    final address = await process.stdout
        .transform(utf8.decoder)
        .transform(const LineSplitter())
        .first
        .timeout(const Duration(seconds: 3));
    final bus = _Bus(dir, process, errors, DBusAddress(address));
    addTearDown(() async {
      for (final c in bus.clients) {
        await c.close();
      }
      process.kill(ProcessSignal.sigterm);
      await process.exitCode.timeout(
        const Duration(seconds: 3),
        onTimeout: () {
          process.kill(ProcessSignal.sigkill);
          return -1;
        },
      );
      await errors;
      await dir.delete(recursive: true);
    });
    return bus;
  }
}

DBusValue _dict(Map<String, DBusValue> values) =>
    DBusDict.stringVariant(values);
DBusValue _description({
  bool enabled = true,
  String parameter = '',
  DBusValue? state,
}) => DBusStruct([
  DBusBoolean(enabled),
  DBusSignature(parameter),
  DBusArray.variant([?state]),
]);

class _Actions extends DBusObject {
  _Actions(String path) : super(DBusObjectPath(path));
  final values = <String, DBusValue>{
    'save': _description(),
    'show-grid': _description(state: const DBusBoolean(true)),
    'mode': _description(parameter: 's', state: const DBusString('dark')),
    'disabled': _description(enabled: false),
  };
  final activations = <(String, List<DBusValue>)>[];
  final states = <(String, bool)>[];
  @override
  List<DBusIntrospectInterface> introspect() => [
    DBusIntrospectInterface(gtkActionsInterface),
  ];
  @override
  Future<DBusMethodResponse> handleMethodCall(DBusMethodCall call) async {
    if (call.interface != gtkActionsInterface) {
      return DBusMethodErrorResponse.unknownInterface();
    }
    if (call.name == 'DescribeAll') {
      return DBusMethodSuccessResponse([
        DBusDict(DBusSignature('s'), DBusSignature('(bgav)'), {
          for (final entry in values.entries)
            DBusString(entry.key): entry.value,
        }),
      ]);
    }
    if (call.name == 'Activate' &&
        call.signature == DBusSignature('sava{sv}')) {
      activations.add((
        call.values[0].asString(),
        call.values[1].asArray().map((v) => v.asVariant()).toList(),
      ));
      return DBusMethodSuccessResponse();
    }
    if (call.name == 'SetState') {
      states.add((
        call.values[0].asString(),
        call.values[1].asVariant().asBoolean(),
      ));
      return DBusMethodSuccessResponse();
    }
    return DBusMethodErrorResponse.unknownMethod();
  }

  Future<void> changed() => emitSignal(gtkActionsInterface, 'Changed', [
    DBusArray.string(const []),
    DBusDict(DBusSignature('s'), DBusSignature('b'), const {}),
    DBusDict.stringVariant(const {}),
    DBusDict(DBusSignature('s'), DBusSignature('(bgav)'), const {}),
  ]);
}

class _Menus extends DBusObject {
  _Menus({String path = '/org/test/Editor/menus/menubar', this.actions})
    : super(DBusObjectPath(path));
  final _Actions? actions;
  Completer<void>? gate;
  final subscriptions = <int>{};
  String saveLabel = '_Save';
  String saveAction = 'win.save';
  String? submenuAction;
  @override
  List<DBusIntrospectInterface> introspect() => [
    DBusIntrospectInterface(gtkMenusInterface),
    if (actions != null) DBusIntrospectInterface(gtkActionsInterface),
  ];
  @override
  Future<DBusMethodResponse> handleMethodCall(DBusMethodCall call) async {
    if (call.interface == gtkActionsInterface && actions != null) {
      return actions!.handleMethodCall(call);
    }
    if (call.interface != gtkMenusInterface) {
      return DBusMethodErrorResponse.unknownInterface();
    }
    if (call.name == 'Start' && call.signature == DBusSignature('au')) {
      final groups = call.values.single.asUint32Array();
      subscriptions.addAll(groups);
      await gate?.future;
      final content = <DBusValue>[];
      for (final group in groups) {
        content.add(
          DBusStruct([
            DBusUint32(group),
            const DBusUint32(0),
            DBusArray(
              DBusSignature('a{sv}'),
              group == 0
                  ? [
                      _dict({
                        'label': const DBusString('_File'),
                        if (submenuAction != null)
                          'submenu-action': DBusString(submenuAction!),
                        ':submenu': DBusStruct([DBusUint32(1), DBusUint32(0)]),
                      }),
                    ]
                  : [
                      _dict({
                        'label': DBusString(saveLabel),
                        'action': DBusString(saveAction),
                      }),
                      _dict({
                        'label': const DBusString('_Grid'),
                        'action': const DBusString('app.show-grid'),
                      }),
                      _dict({
                        'label': const DBusString('_Dark'),
                        'action': const DBusString('app.mode'),
                        'target': const DBusString('dark'),
                      }),
                      _dict({
                        'label': const DBusString('_Disabled'),
                        'action': const DBusString('app.disabled'),
                      }),
                    ],
            ),
          ]),
        );
      }
      return DBusMethodSuccessResponse([
        DBusArray(DBusSignature('(uuaa{sv})'), content),
      ]);
    }
    if (call.name == 'End') {
      subscriptions.removeAll(call.values.single.asUint32Array());
      return DBusMethodSuccessResponse();
    }
    return DBusMethodErrorResponse.unknownMethod();
  }

  Future<void> changed() => emitSignal(gtkMenusInterface, 'Changed', [
    DBusArray(DBusSignature('(uuuuaa{sv})'), const []),
  ]);

  Future<void> actionsChanged() => emitSignal(gtkActionsInterface, 'Changed', [
    DBusArray.string(const []),
    DBusDict(DBusSignature('s'), DBusSignature('b'), const {}),
    DBusDict.stringVariant(const {}),
    DBusDict(DBusSignature('s'), DBusSignature('(bgav)'), const {}),
  ]);
}

class _Canonical extends DBusObject {
  _Canonical(String path, this.title) : super(DBusObjectPath(path));
  final String title;
  final clicked = <int>[];
  int prepared = 0;
  bool disabled = false;
  @override
  List<DBusIntrospectInterface> introspect() => [
    DBusIntrospectInterface(appDbusMenuInterface),
  ];
  DBusValue node(
    int id,
    String label,
    List<DBusValue> children, {
    bool enabled = true,
  }) => DBusStruct([
    DBusInt32(id),
    _dict({
      'label': DBusString(label),
      'enabled': DBusBoolean(enabled),
      if (children.isNotEmpty) 'children-display': const DBusString('submenu'),
    }),
    DBusArray.variant(children),
  ]);
  @override
  Future<DBusMethodResponse> handleMethodCall(DBusMethodCall call) async {
    if (call.interface != appDbusMenuInterface) {
      return DBusMethodErrorResponse.unknownInterface();
    }
    if (call.name == 'GetLayout') {
      return DBusMethodSuccessResponse([
        const DBusUint32(1),
        node(0, '', [
          node(1, title, [
            node(2, '_Save', [], enabled: !disabled),
            if (prepared > 0) node(3, '_Recent', []),
          ]),
        ]),
      ]);
    }
    if (call.name == 'AboutToShow') {
      prepared++;
      return DBusMethodSuccessResponse([const DBusBoolean(true)]);
    }
    if (call.name == 'Event') {
      expect(call.signature, DBusSignature('isvu'));
      expect(call.values[1].asString(), 'clicked');
      clicked.add(call.values[0].asInt32());
      return DBusMethodSuccessResponse();
    }
    return DBusMethodErrorResponse.unknownMethod();
  }

  Future<void> changed() => emitSignal(appDbusMenuInterface, 'LayoutUpdated', [
    const DBusUint32(2),
    const DBusInt32(0),
  ]);
}

Future<(_Menus, _Actions, _Actions, DBusClient)> _gtk(_Bus bus) async {
  final client = bus.client();
  await client.requestName('org.test.Editor');
  final menus = _Menus();
  final app = _Actions('/org/test/Editor');
  final win = _Actions('/org/test/Editor/window/1');
  await client.registerObject(menus);
  await client.registerObject(app);
  await client.registerObject(win);
  return (menus, app, win, client);
}

AppMenuExport _export() => const AppMenuExport(
  busName: 'org.test.Editor',
  menuPath: '/org/test/Editor/menus/menubar',
  applicationActionsPath: '/org/test/Editor',
  windowActionsPath: '/org/test/Editor/window/1',
);
GlobalAppMenuService _service(_Bus bus) {
  final service = GlobalAppMenuService(bus: bus.client());
  addTearDown(service.dispose);
  return service;
}

Future<void> _wait(bool Function() ready) async {
  final limit = DateTime.now().add(const Duration(seconds: 3));
  while (!ready()) {
    if (DateTime.now().isAfter(limit)) {
      throw TimeoutException('Private appmenu state did not settle');
    }
    await Future<void>.delayed(const Duration(milliseconds: 10));
  }
}

Future<void> _register(DBusClient client, int xid, String path) async {
  await DBusRemoteObject(
    client,
    name: appMenuRegistrarName,
    path: DBusObjectPath(appMenuRegistrarPath),
  ).callMethod(appMenuRegistrarInterface, 'RegisterWindow', [
    DBusUint32(xid),
    DBusObjectPath(path),
  ], replySignature: DBusSignature(''));
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  test(
    'registrar active transitions release/reacquire ownership without replacing an external host',
    () async {
      final bus = await _Bus.create();
      final host = bus.client();
      final service = GlobalAppMenuService(bus: host);
      addTearDown(service.dispose);
      final observer = bus.client();
      await service.start();
      expect(
        await observer.getNameOwner(appMenuRegistrarName),
        host.uniqueName,
      );
      await service.setActive(false);
      expect(await observer.getNameOwner(appMenuRegistrarName), isNull);
      await service.setActive(true);
      expect(
        await observer.getNameOwner(appMenuRegistrarName),
        host.uniqueName,
      );
      await service.setActive(false);

      final external = bus.client();
      await external.requestName(appMenuRegistrarName);
      await service.setActive(true);
      expect(
        await observer.getNameOwner(appMenuRegistrarName),
        external.uniqueName,
      );
      await service.setActive(false);
      expect(
        await observer.getNameOwner(appMenuRegistrarName),
        external.uniqueName,
      );
      await service.setActive(true);
      await external.releaseName(appMenuRegistrarName);
      final deadline = DateTime.now().add(const Duration(seconds: 3));
      while (await observer.getNameOwner(appMenuRegistrarName) !=
          host.uniqueName) {
        if (DateTime.now().isAfter(deadline)) {
          throw TimeoutException(
            'Registrar did not acquire a departed host name',
          );
        }
        await Future<void>.delayed(const Duration(milliseconds: 10));
      }
      expect(
        await observer.getNameOwner(appMenuRegistrarName),
        host.uniqueName,
      );
    },
  );
  test(
    'inactive startup and rapid active reversal do not leave a registrar owned',
    () async {
      final bus = await _Bus.create();
      final host = bus.client();
      final observer = bus.client();
      final service = GlobalAppMenuService(bus: host);
      addTearDown(service.dispose);
      final starting = service.start();
      final inactive = service.setActive(false);
      await Future.wait([starting, inactive]);
      expect(await observer.getNameOwner(appMenuRegistrarName), isNull);
      final active = service.setActive(true);
      final inactiveAgain = service.setActive(false);
      await Future.wait([active, inactiveAgain]);
      expect(await observer.getNameOwner(appMenuRegistrarName), isNull);
      await service.focusedTarget(pid: pid, appId: 'org.test.Editor');
      expect(service.loading, isFalse);
      expect(service.menus, isEmpty);
    },
  );
  test(
    'GTK real private bus resolves nested groups and exact app/window actions',
    () async {
      final bus = await _Bus.create();
      final (menus, app, win, _) = await _gtk(bus);
      final other = _Actions('/org/test/Editor/window/2');
      await bus.clients.first.registerObject(other);
      final service = _service(bus);
      await service.focusedTarget(
        pid: pid,
        appId: 'org.test.Editor',
        windowId: 999,
        export: _export(),
      );
      expect(service.menus.single.label, 'File');
      final children = service.menus.single.children;
      expect(children.map((e) => e.label), [
        'Save',
        'Grid',
        'Dark',
        'Disabled',
      ]);
      expect(children[1].checked, isTrue);
      expect(children[2].radio, isTrue);
      expect(children[2].checked, isTrue);
      expect(children[3].enabled, isFalse);
      expect(await service.activate(children[0]), isTrue);
      expect(win.activations.single.$1, 'save');
      expect(other.activations, isEmpty);
      expect(await service.activate(children[2]), isTrue);
      expect(app.activations.single.$1, 'mode');
      expect(app.activations.single.$2, [const DBusString('dark')]);
      expect(await service.activate(children[3]), isFalse);
      expect(menus.subscriptions, {0, 1});
      await service.focusedTarget();
      await _wait(() => menus.subscriptions.isEmpty);
      expect(await service.activate(children[0]), isFalse);
    },
  );
  test(
    'traditional GTK unity actions use the menu object and cannot borrow other groups',
    () async {
      final bus = await _Bus.create();
      final client = bus.client();
      await client.requestName('org.test.TraditionalGtk');
      const menuPath = '/org/gtk/MenuBar/fixture';
      final actions = _Actions(menuPath);
      final menu = _Menus(path: menuPath, actions: actions)
        ..saveAction = 'unity.save';
      final otherApp = _Actions('/org/test/Editor');
      final otherWindow = _Actions('/org/test/Editor/window/1');
      await client.registerObject(menu);
      await client.registerObject(otherApp);
      await client.registerObject(otherWindow);
      final service = _service(bus);
      const export = AppMenuExport(
        busName: 'org.test.TraditionalGtk',
        menuPath: menuPath,
        applicationActionsPath: '/org/test/Editor',
        windowActionsPath: '/org/test/Editor/window/1',
      );
      await service.focusedTarget(pid: pid, windowId: 17, export: export);
      final save = service.menus.single.children.first;
      expect(save.enabled, isTrue);
      expect(await service.activate(save), isTrue);
      expect(actions.activations.single.$1, 'save');
      expect(otherApp.activations, isEmpty);
      expect(otherWindow.activations, isEmpty);

      actions.values.remove('save');
      await menu.actionsChanged();
      await _wait(() => !service.menus.single.children.first.enabled);
      expect(await service.activate(save), isFalse);
      expect(
        await service.activate(service.menus.single.children.first),
        isFalse,
      );
      expect(otherApp.activations, isEmpty);
      expect(otherWindow.activations, isEmpty);

      actions.values['save'] = _description();
      await menu.actionsChanged();
      await _wait(() => service.menus.single.children.first.enabled);
      await service.focusedTarget(pid: pid + 99999, export: export);
      expect(service.menus, isEmpty);
      expect(await service.activate(save), isFalse);
      expect(actions.activations, hasLength(1));
    },
  );
  test(
    'GTK discovery uses owning PID and preserves fallback for actions-only apps',
    () async {
      final bus = await _Bus.create();
      await _gtk(bus);
      final service = _service(bus);
      await service.focusedTarget(
        pid: pid,
        appId: 'org.test.Editor',
        windowId: 77,
      );
      expect(service.menus.single.label, 'File');
      expect(
        await service.activate(service.menus.single.children.first),
        isTrue,
      );
      await service.focusedTarget(
        pid: pid + 99999,
        appId: 'org.test.Editor',
        export: _export(),
      );
      expect(service.menus, isEmpty);
      final plainBus = await _Bus.create();
      await plainBus.client().registerObject(_Actions('/org/test/Plain'));
      final plain = _service(plainBus);
      await plain.focusedTarget(pid: pid, appId: 'org.test.Plain');
      expect(plain.menus, isEmpty);
      expect(plain.available, isFalse);
    },
  );
  test(
    'GTK state/menu changes refresh and stale items cannot activate',
    () async {
      final bus = await _Bus.create();
      final (menus, app, _, _) = await _gtk(bus);
      final service = _service(bus);
      await service.focusedTarget(pid: pid, export: _export());
      final oldGrid = service.menus.single.children[1];
      app.values['show-grid'] = _description(
        enabled: false,
        state: const DBusBoolean(false),
      );
      menus.saveLabel = '_Save all';
      await app.changed();
      await menus.changed();
      await _wait(
        () => service.menus.single.children.first.label == 'Save all',
      );
      expect(service.menus.single.children[1].checked, isFalse);
      expect(service.menus.single.children[1].enabled, isFalse);
      expect(await service.activate(oldGrid), isFalse);
      expect(app.activations, isEmpty);
    },
  );
  test(
    'GTK disabled submenus stay disabled and focus cleanup resets presentation state',
    () async {
      final bus = await _Bus.create();
      final (menus, app, _, _) = await _gtk(bus);
      menus.submenuAction = 'app.file-open';
      app.values['file-open'] = _description(
        enabled: false,
        state: const DBusBoolean(false),
      );
      final service = _service(bus);
      await service.focusedTarget(pid: pid, export: _export());
      expect(service.menus.single.children, isNotEmpty);
      expect(service.menus.single.enabled, isFalse);
      expect(await service.activate(service.menus.single), isFalse);
      await service.submenu(service.menus.single, opened: true);
      expect(app.states, isEmpty);
      app.values['file-open'] = _description(state: const DBusBoolean(false));
      await app.changed();
      await _wait(() => service.menus.single.enabled);
      await service.submenu(service.menus.single, opened: true);
      expect(app.states, [('file-open', true)]);
      await service.focusedTarget();
      await _wait(() => app.states.length == 2);
      expect(app.states, [('file-open', true), ('file-open', false)]);
    },
  );
  test('slow GTK reply cannot restore a previous focused target', () async {
    final bus = await _Bus.create();
    final (menus, _, _, _) = await _gtk(bus);
    final service = _service(bus);
    await service.start();
    menus.gate = Completer<void>();
    final pending = service.focusedTarget(pid: pid, export: _export());
    await _wait(() => menus.subscriptions.isNotEmpty);
    await service.focusedTarget(windowId: 2);
    menus.gate!.complete();
    await pending;
    expect(service.menus, isEmpty);
    expect(service.loading, isFalse);
    await _wait(() => menus.subscriptions.isEmpty);
  });
  test(
    'inactive explicit canonical menus retain real actions without hosting or inferring a registrar',
    () async {
      final bus = await _Bus.create();
      final service = _service(bus);
      final observer = bus.client();
      final exporter = bus.client();
      await exporter.requestName('org.test.QtMenu');
      final menu = _Canonical('/MenuBar/1', '_Document');
      await exporter.registerObject(menu);
      const export = AppMenuExport(
        busName: 'org.test.QtMenu',
        menuPath: '/MenuBar/1',
        protocol: AppMenuProtocol.dbusmenu,
      );
      await service.setActive(false);
      await service.focusedTarget(pid: pid, windowId: 100, export: export);
      expect(await observer.getNameOwner(appMenuRegistrarName), isNull);
      expect(service.loading, isFalse);
      expect(service.menus.single.label, 'Document');
      final saved = service.menus.single.children.first;
      expect(await service.activate(saved), isTrue);
      expect(menu.clicked, [2]);
      expect(await observer.getNameOwner(appMenuRegistrarName), isNull);

      // Read/watch remains active, but ownership never changes or gets queued
      // when another desktop provides a registrar on this same private bus.
      final external = bus.client();
      await external.requestName(appMenuRegistrarName);
      menu.disabled = true;
      await menu.changed();
      await _wait(() => !service.menus.single.children.first.enabled);
      expect(await service.activate(saved), isFalse);
      expect(
        await service.activate(service.menus.single.children.first),
        isFalse,
      );
      expect(
        await observer.getNameOwner(appMenuRegistrarName),
        external.uniqueName,
      );
      await external.releaseName(appMenuRegistrarName);
      await _wait(() => service.available && !service.loading);
      expect(await observer.getNameOwner(appMenuRegistrarName), isNull);

      await service.focusedTarget(
        pid: pid + 99999,
        windowId: 100,
        export: export,
      );
      expect(service.menus, isEmpty);
      expect(await service.activate(saved), isFalse);
      // No address means no PID/registrar/GMenu discovery while Material hosts
      // nothing, even though this exact exporter still exists on the bus.
      await service.focusedTarget(pid: pid, appId: 'org.test.QtMenu');
      expect(service.menus, isEmpty);
      expect(service.loading, isFalse);

      menu.disabled = false;
      await service.focusedTarget(pid: pid, windowId: 100, export: export);
      final current = service.menus.single.children.first;
      await exporter.close();
      await _wait(() => service.menus.isEmpty);
      expect(await service.activate(current), isFalse);
      expect(await observer.getNameOwner(appMenuRegistrarName), isNull);
      expect(menu.clicked, [2]);
    },
  );
  test(
    'inactive GTK exports remain unavailable after releasing their host',
    () async {
      final bus = await _Bus.create();
      final (_, _, _, _) = await _gtk(bus);
      final service = _service(bus);
      final observer = bus.client();
      await service.focusedTarget(pid: pid, export: _export());
      final old = service.menus.single.children.first;
      await service.setActive(false);
      expect(await observer.getNameOwner(appMenuRegistrarName), isNull);
      expect(service.menus, isEmpty);
      await service.focusedTarget(pid: pid, export: _export());
      expect(service.menus, isEmpty);
      expect(service.loading, isFalse);
      expect(await service.activate(old), isFalse);
    },
  );
  test(
    'canonical real registrar selects PID, prepares submenus and sends Event',
    () async {
      final bus = await _Bus.create();
      final service = _service(bus);
      await service.start();
      final client = bus.client();
      final menu = _Canonical('/app/menu', '_Document');
      await client.registerObject(menu);
      await _register(client, 3001, menu.path.value);
      // A Wayland ID happens to equal an XID, but selection here is unique PID,
      // not equality between the two numbering domains.
      await service.focusedTarget(pid: pid, windowId: 3001);
      expect(service.menus.single.label, 'Document');
      await service.submenu(service.menus.single, opened: true);
      expect(menu.prepared, 1);
      expect(service.menus.single.children, hasLength(2));
      expect(
        await service.activate(service.menus.single.children.first),
        isTrue,
      );
      expect(menu.clicked, [2]);
    },
  );
  test(
    'canonical ambiguous same-process windows require an explicit X11 identity',
    () async {
      final bus = await _Bus.create();
      final service = _service(bus);
      await service.start();
      final client = bus.client();
      final a = _Canonical('/app/one', '_One');
      final b = _Canonical('/app/two', '_Two');
      await client.registerObject(a);
      await client.registerObject(b);
      await _register(client, 11, a.path.value);
      await _register(client, 22, b.path.value);
      await service.focusedTarget(pid: pid, windowId: 11);
      expect(service.menus, isEmpty);
      await service.focusedTarget(pid: pid, windowId: 999, x11WindowId: 22);
      expect(service.menus.single.label, 'Two');
      expect(
        await service.activate(service.menus.single.children.first),
        isTrue,
      );
      expect(a.clicked, isEmpty);
      expect(b.clicked, [2]);
      await service.focusedTarget(pid: pid + 99999, x11WindowId: 22);
      expect(service.menus, isEmpty);
    },
  );
  test(
    'registrar rejects another owner removal and forgets a departed exporter',
    () async {
      final bus = await _Bus.create();
      final service = _service(bus);
      await service.start();
      final client = bus.client();
      final menu = _Canonical('/app/menu', '_Document');
      await client.registerObject(menu);
      await _register(client, 11, menu.path.value);
      final stranger = DBusRemoteObject(
        bus.client(),
        name: appMenuRegistrarName,
        path: DBusObjectPath(appMenuRegistrarPath),
      );
      await stranger.callMethod(appMenuRegistrarInterface, 'UnregisterWindow', [
        const DBusUint32(11),
      ], replySignature: DBusSignature(''));
      final registered = await stranger.callMethod(
        appMenuRegistrarInterface,
        'GetMenus',
        const [],
        replySignature: DBusSignature('a(uso)'),
      );
      expect(registered.returnValues.single.asArray(), hasLength(1));
      await service.focusedTarget(pid: pid, x11WindowId: 11);
      final old = service.menus.single.children.first;
      await client.close();
      await _wait(() => service.menus.isEmpty);
      expect(await service.activate(old), isFalse);
    },
  );
  testWidgets('component shows real items and closes on focus change', (
    tester,
  ) async {
    late _Bus bus;
    late GlobalAppMenuService service;
    await tester.runAsync(() async {
      bus = await _Bus.create();
      await _gtk(bus);
      service = _service(bus);
      await service.focusedTarget(pid: pid, export: _export());
    });
    final opened = <bool>[];
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Align(
            alignment: Alignment.topLeft,
            child: GlobalAppMenuBar(
              service: service,
              onOpenChanged: opened.add,
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('File'), findsOneWidget);
    await tester.tap(find.text('File'));
    await tester.pumpAndSettle();
    expect(find.text('Save'), findsOneWidget);
    await tester.runAsync(() async {
      await service.focusedTarget();
    });
    await tester.pumpAndSettle();
    expect(find.text('Save'), findsNothing);
    expect(opened, [true, false]);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  });
  testWidgets(
    'static GTK popup preserves item identity and dispatches before closing clears focus',
    (tester) async {
      late GlobalAppMenuService service;
      late _Actions windowActions;
      late _Menus exportedMenus;
      await tester.runAsync(() async {
        final bus = await _Bus.create();
        final (menu, _, win, _) = await _gtk(bus);
        exportedMenus = menu;
        windowActions = win;
        service = _service(bus);
        await service.focusedTarget(pid: pid, windowId: 91, export: _export());
      });
      final action = service.menus.single.children.first;
      final opened = <bool>[];
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: Align(
              alignment: Alignment.topLeft,
              child: GlobalAppMenuBar(
                service: service,
                onOpenChanged: (open) {
                  opened.add(open);
                  if (!open) unawaited(service.focusedTarget());
                },
              ),
            ),
          ),
        ),
      );
      await tester.tap(find.text('File'));
      await tester.pumpAndSettle();
      expect(identical(service.menus.single.children.first, action), isTrue);
      expect(opened, [true]);
      await tester.tap(find.text('Save'));
      await tester.pump();
      await tester.runAsync(
        () => _wait(() => windowActions.activations.isNotEmpty),
      );
      await tester.pumpAndSettle();
      expect(windowActions.activations.single.$1, 'save');
      expect(opened, [true, false]);
      expect(service.available, isFalse);
      await tester.runAsync(
        () => _wait(() => exportedMenus.subscriptions.isEmpty),
      );
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );
}
