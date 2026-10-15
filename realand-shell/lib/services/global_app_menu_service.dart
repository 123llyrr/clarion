// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'package:dbus/dbus.dart';
import 'package:flutter/foundation.dart';
import '../app_menu_models.dart';

const appMenuRegistrarName = 'com.canonical.AppMenu.Registrar';
const appMenuRegistrarPath = '/com/canonical/AppMenu/Registrar';
const appMenuRegistrarInterface = 'com.canonical.AppMenu.Registrar';
const gtkMenusInterface = 'org.gtk.Menus';
const gtkActionsInterface = 'org.gtk.Actions';
const appDbusMenuInterface = 'com.canonical.dbusmenu';

/// Reads only menus genuinely exported by the focused process. In particular,
/// a compositor window ID is never treated as an X11 registrar window ID.
class GlobalAppMenuService extends ChangeNotifier {
  GlobalAppMenuService({
    DBusClient? bus,
    this.hostRegistrar = true,
    this.readTimeout = const Duration(milliseconds: 700),
  }) : _injected = bus;
  final DBusClient? _injected;
  final bool hostRegistrar;
  final Duration readTimeout;
  DBusClient? _bus;
  _Registrar? _registrar;
  bool _ownsRegistrar = false;
  bool _active = true;
  bool _disposed = false;
  Future<void>? _starting;
  Future<void> _registrarTail = Future.value();
  StreamSubscription<DBusNameOwnerChangedEvent>? _owners;
  StreamSubscription<DBusSignal>? _registrarSignals;
  Timer? _refreshTimer;
  _Session? _session;
  AppMenuTarget? _target;
  int _revision = 0;
  int _loadRevision = 0;
  bool loading = false;
  String? error;
  List<GlobalMenuItem> _menus = const [];
  Map<String, Future<void> Function()> _commands = const {};
  List<GlobalMenuItem> get menus => _menus;
  bool get available => _menus.isNotEmpty;
  int get revision => _revision;
  AppMenuTarget? get target => _target;

  // Existing Qt clients may keep their native menu hidden after the registrar
  // disappears. Material may read their explicit surface address without
  // hosting a registrar or guessing which application/window owns a menu.
  bool get _canRead =>
      _active || _target?.export?.protocol == AppMenuProtocol.dbusmenu;

  /// Release the canonical host when using a shell with local application menus.
  /// An explicit canonical export can still be read and invoked after release,
  /// so existing Qt clients do not lose access to menus they keep hidden locally.
  /// Ownership operations are serialized, including a startup that completes
  /// after the shell has already become inactive. The read connection survives.
  Future<void> setActive(bool active) async {
    if (_disposed) return;
    final changed = _active != active;
    _active = active;
    if (!active && changed) {
      _revision++;
      _loadRevision++;
      _refreshTimer?.cancel();
      _session?.close();
      _session = null;
      _menus = const [];
      _commands = const {};
      loading = false;
      _notify();
    }
    // An inactive, unused service need not connect to the session bus at all.
    // focusedTarget/start initialize it lazily once a real application needs it.
    final starting = _starting;
    if (starting == null) return;
    try {
      await starting;
      await _syncRegistrar();
      if (changed && _canRead && !_disposed) _scheduleDiscovery();
    } on Object {
      // The GTK read adapter remains useful when the registrar is unavailable.
    }
  }

  Future<void> _syncRegistrar() {
    final next = _registrarTail
        .then((_) async {
          final bus = _bus;
          if (bus == null || !hostRegistrar || _registrar == null) return;
          while (true) {
            final desired = _active && !_disposed;
            if (desired && !_ownsRegistrar) {
              final request = bus.requestName(
                appMenuRegistrarName,
                flags: {DBusRequestNameFlag.doNotQueue},
              );
              // A timeout does not cancel RequestName. Its eventual response must
              // still release a name acquired after an inactive/disposed transition.
              unawaited(
                request
                    .then((reply) {
                      if (_disposed) return;
                      _ownsRegistrar =
                          reply == DBusRequestNameReply.primaryOwner ||
                          reply == DBusRequestNameReply.alreadyOwner;
                      if (_ownsRegistrar && !_active) {
                        unawaited(_syncRegistrar());
                      }
                    })
                    .catchError((Object _) {}),
              );
              final reply = await request.timeout(readTimeout);
              _ownsRegistrar =
                  reply == DBusRequestNameReply.primaryOwner ||
                  reply == DBusRequestNameReply.alreadyOwner;
              if (!_ownsRegistrar) {
                return; // Never replace or queue behind another host.
              }
            } else if (!desired && _ownsRegistrar) {
              await bus.releaseName(appMenuRegistrarName).timeout(readTimeout);
              _ownsRegistrar = false;
              _registrar?.entries.clear();
            } else {
              return;
            }
            if (desired == (_active && !_disposed)) return;
          }
        })
        .catchError((Object _) {});
    _registrarTail = next;
    return next;
  }

  Future<void> start() => _disposed ? Future.value() : _starting ??= _start();
  Future<void> _start() async {
    final bus = _bus = _injected ?? DBusClient.session();
    _owners = bus.nameOwnerChanged.listen((event) {
      _registrar?.ownerChanged(event);
      if (_disposed) return;
      if (event.name == appMenuRegistrarName) {
        _ownsRegistrar = event.newOwner == bus.uniqueName;
        if (_active && event.newOwner == null) {
          unawaited(_syncRegistrar());
        }
      }
      if (event.name == _session?.owner && event.newOwner == null) {
        _revision++;
        _menus = const [];
        _commands = const {};
        _session?.close();
        _session = null;
        _notify();
      }
      if ((event.name == appMenuRegistrarName &&
              event.newOwner != bus.uniqueName) ||
          event.name == _target?.export?.busName ||
          event.name == _target?.appId) {
        _scheduleDiscovery();
      }
    });
    if (hostRegistrar) {
      final registrar = _registrar = _Registrar(bus, _scheduleDiscovery);
      await bus.registerObject(registrar).timeout(readTimeout);
      await _syncRegistrar();
    }
    _registrarSignals = DBusSignalStream(
      bus,
      interface: appMenuRegistrarInterface,
      path: DBusObjectPath(appMenuRegistrarPath),
    ).listen((_) => _scheduleDiscovery());
  }

  Future<void> focusedTarget({
    int? pid,
    String appId = '',
    int? windowId,
    int? x11WindowId,
    AppMenuExport? export,
  }) async {
    if (_disposed) return;
    _target = AppMenuTarget(
      pid: pid,
      appId: appId,
      windowId: windowId,
      x11WindowId: x11WindowId,
      export: export,
    );
    final revision = ++_revision;
    ++_loadRevision;
    _refreshTimer?.cancel();
    _session?.close();
    _session = null;
    _menus = const [];
    _commands = const {};
    error = null;
    loading = _canRead && pid != null && pid > 0;
    _notify();
    if (!loading) return;
    try {
      await start();
      if (!_current(revision)) return;
      final target = _target!;
      final deadline = DateTime.now().add(const Duration(seconds: 4));
      final session = await _discover(target, revision, deadline);
      if (!_current(revision)) {
        session?.close();
        return;
      }
      _session = session;
      if (session != null) {
        session.changed = () => _scheduleLoad(revision);
        await _load(revision);
      }
    } on Object {
      // Lack of an exporter is a normal fallback, not a made-up application menu.
      if (_current(revision)) error = '此应用没有可用的导出菜单';
    } finally {
      if (_current(revision)) {
        loading = false;
        _notify();
      }
    }
  }

  bool _current(int revision) =>
      !_disposed && _canRead && revision == _revision;
  void _notify() {
    if (!_disposed) notifyListeners();
  }

  void _scheduleDiscovery() {
    if (_disposed || !_canRead || _target == null) return;
    _refreshTimer?.cancel();
    _refreshTimer = Timer(const Duration(milliseconds: 45), () {
      final t = _target;
      if (t != null) {
        unawaited(
          focusedTarget(
            pid: t.pid,
            appId: t.appId,
            windowId: t.windowId,
            x11WindowId: t.x11WindowId,
            export: t.export,
          ),
        );
      }
    });
  }

  void _scheduleLoad(int revision) {
    if (!_current(revision)) return;
    _refreshTimer?.cancel();
    _refreshTimer = Timer(const Duration(milliseconds: 35), () {
      unawaited(_load(revision));
    });
  }

  Future<void> _load(int revision) async {
    final session = _session;
    if (!_current(revision) || session == null) return;
    final load = ++_loadRevision;
    try {
      final snapshot = await session.load();
      if (!_current(revision) || load != _loadRevision || session != _session) {
        return;
      }
      _menus = List.unmodifiable(snapshot.items);
      _commands = snapshot.commands;
      error = null;
      _notify();
    } on Object {
      if (_current(revision) && load == _loadRevision) {
        _menus = const [];
        _commands = const {};
        error = '应用菜单暂时不可用';
        _notify();
      }
    }
  }

  GlobalMenuItem? _find(String id, List<GlobalMenuItem> items) {
    for (final item in items) {
      if (item.id == id) return item;
      final child = _find(id, item.children);
      if (child != null) return child;
    }
    return null;
  }

  Future<bool> activate(GlobalMenuItem item) async {
    if (_disposed ||
        !identical(_find(item.id, _menus), item) ||
        !item.enabled ||
        !item.visible ||
        item.separator ||
        item.isSubmenu) {
      return false;
    }
    final action = _commands[item.id];
    if (action == null) return false;
    final revision = _revision;
    try {
      await action().timeout(const Duration(seconds: 2));
      return _current(revision);
    } on Object {
      if (_current(revision)) {
        error = '应用菜单操作失败';
        _notify();
      }
      return false;
    }
  }

  Future<void> submenu(GlobalMenuItem item, {required bool opened}) async {
    if (_disposed ||
        !identical(_find(item.id, _menus), item) ||
        !item.isSubmenu ||
        (opened && (!item.enabled || !item.visible))) {
      return;
    }
    final revision = _revision;
    try {
      final changed = await _session?.submenu(item.id, opened) ?? false;
      if (opened && changed && _current(revision)) await _load(revision);
    } on Object {
      /* An exporter may omit dynamic preparation. */
    }
  }

  Future<_Session?> _discover(
    AppMenuTarget t,
    int revision,
    DateTime deadline,
  ) async {
    if (!_active && t.export?.protocol != AppMenuProtocol.dbusmenu) {
      return null;
    }
    final bus = _bus!;
    Future<bool> owned(String name) async {
      try {
        return await bus
                .getConnectionUnixProcessId(name)
                .timeout(readTimeout) ==
            t.pid;
      } on Object {
        return false;
      }
    }

    _Session gtk(
      String owner,
      AppMenuExport e, [
      List<String> actionPaths = const [],
    ]) => _GtkSession(
      bus,
      owner,
      e.menuPath,
      readTimeout,
      appActions: e.applicationActionsPath,
      windowActions: e.windowActionsPath,
      discoveredActions: actionPaths,
    );
    if (t.export case final e?) {
      final owner = await bus.getNameOwner(e.busName).timeout(readTimeout);
      if (owner != null && await owned(owner)) {
        final node = await _object(
          bus,
          owner,
          e.menuPath,
        ).introspect().timeout(readTimeout);
        final interface = e.protocol == AppMenuProtocol.gtk
            ? gtkMenusInterface
            : appDbusMenuInterface;
        if (node.interfaces.any((i) => i.name == interface)) {
          return e.protocol == AppMenuProtocol.gtk
              ? gtk(owner, e)
              : _CanonicalSession(bus, owner, e.menuPath, readTimeout);
        }
      }
      return null; // Explicit metadata must never silently target a different export.
    }
    final registrar = await _registeredMenus(t.x11WindowId);
    final matches = <_RegisteredMenu>[];
    for (final menu in registrar.take(64)) {
      if (!_current(revision) || DateTime.now().isAfter(deadline)) return null;
      if (t.x11WindowId != null && menu.xid != t.x11WindowId) continue;
      try {
        final unique = await bus.getNameOwner(menu.owner).timeout(readTimeout);
        if (unique != null && await owned(unique)) {
          matches.add(_RegisteredMenu(menu.xid, unique, menu.path));
        }
      } on Object {
        /* A registrar may retain a departed client briefly. */
      }
    }
    if (matches.length == 1) {
      final m = matches.single;
      return _CanonicalSession(bus, m.owner, m.path, readTimeout);
    }
    // The same process may export several per-window menus. PID alone cannot
    // choose one reliably; the compositor window ID is deliberately ignored.
    if (matches.length > 1) return null;
    final names = await bus.listNames().timeout(readTimeout);
    final candidates = names.where((n) => n.startsWith(':')).take(256).toList();
    final owners = <String>[];
    for (var i = 0; i < candidates.length; i += 16) {
      if (!_current(revision) || DateTime.now().isAfter(deadline)) return null;
      final batch = candidates.skip(i).take(16).toList();
      final flags = await Future.wait(batch.map(owned));
      for (var j = 0; j < batch.length; j++) {
        if (flags[j]) owners.add(batch[j]);
      }
    }
    final exports = <(String, String)>[];
    final actions = <String, List<String>>{};
    var remaining = 48;
    for (final owner in owners) {
      final queue = <(String, int)>[('/', 0)];
      final visited = <String>{};
      if (t.appId.contains('.')) {
        final path = '/${t.appId.replaceAll('.', '/').replaceAll('-', '_')}';
        queue.insert(0, (path, 0));
      }
      while (queue.isNotEmpty && remaining-- > 0) {
        if (!_current(revision) || DateTime.now().isAfter(deadline)) {
          return null;
        }
        final (path, depth) = queue.removeAt(0);
        if (!visited.add(path) ||
            path.contains('StatusNotifier') ||
            path.contains('NotificationItem')) {
          continue;
        }
        try {
          final node = await _object(
            bus,
            owner,
            path,
          ).introspect().timeout(readTimeout);
          if (node.interfaces.any((i) => i.name == gtkMenusInterface)) {
            exports.add((owner, path));
          }
          if (node.interfaces.any((i) => i.name == gtkActionsInterface)) {
            (actions[owner] ??= []).add(path);
          }
          if (depth < 7) {
            for (final child in node.children.take(24)) {
              final name = child.name;
              if (name != null && name.isNotEmpty) {
                queue.add((
                  name.startsWith('/')
                      ? name
                      : '${path == '/' ? '' : path}/$name',
                  depth + 1,
                ));
              }
            }
          }
        } on Object {
          /* Non-GTK objects and unresponsive peers are skipped. */
        }
      }
    }
    final menubars = exports.where((e) => e.$2.endsWith('/menubar')).toList();
    final selected = menubars.isEmpty ? exports : menubars;
    if (selected.length != 1) return null;
    final (owner, path) = selected.single;
    return gtk(
      owner,
      AppMenuExport(busName: owner, menuPath: path),
      actions[owner] ?? const [],
    );
  }

  Future<List<_RegisteredMenu>> _registeredMenus(int? xid) async {
    if (_ownsRegistrar) return _registrar!.entries.values.toList();
    try {
      final owner = await _bus!
          .getNameOwner(appMenuRegistrarName)
          .timeout(readTimeout);
      if (owner == null) return const [];
      if (xid != null && xid > 0) {
        final response = await _object(_bus!, owner, appMenuRegistrarPath)
            .callMethod(appMenuRegistrarInterface, 'GetMenuForWindow', [
              DBusUint32(xid),
            ], replySignature: DBusSignature('so'))
            .timeout(readTimeout);
        final service = response.returnValues[0].asString();
        final path = response.returnValues[1].asObjectPath().value;
        return service.isEmpty || path == '/'
            ? const []
            : [_RegisteredMenu(xid, service, path)];
      }
      final response = await _object(_bus!, owner, appMenuRegistrarPath)
          .callMethod(
            appMenuRegistrarInterface,
            'GetMenus',
            const [],
            replySignature: DBusSignature('a(uso)'),
          )
          .timeout(readTimeout);
      return [
        for (final entry in response.returnValues.single.asArray())
          _RegisteredMenu(
            entry.asStruct()[0].asUint32(),
            entry.asStruct()[1].asString(),
            entry.asStruct()[2].asObjectPath().value,
          ),
      ];
    } on Object {
      return const [];
    }
  }

  @override
  void dispose() {
    if (_disposed) return;
    _disposed = true;
    _active = false;
    _ownsRegistrar = false;
    _registrar?.entries.clear();
    ++_revision;
    ++_loadRevision;
    _refreshTimer?.cancel();
    _session?.close();
    unawaited(_owners?.cancel());
    unawaited(_registrarSignals?.cancel());
    final bus = _bus;
    if (bus != null) unawaited(bus.close());
    super.dispose();
  }
}

DBusRemoteObject _object(DBusClient bus, String owner, String path) =>
    DBusRemoteObject(bus, name: owner, path: DBusObjectPath(path));

class _Snapshot {
  _Snapshot(this.items, this.commands);
  final List<GlobalMenuItem> items;
  final Map<String, Future<void> Function()> commands;
}

abstract class _Session {
  _Session(this.bus, this.owner, this.path, this.timeout);
  final DBusClient bus;
  final String owner, path;
  final Duration timeout;
  VoidCallback? changed;
  bool closed = false;
  final subscriptions = <StreamSubscription<DBusSignal>>[];
  Future<_Snapshot> load();
  Future<bool> submenu(String id, bool open) async => false;
  void watch(String interface, String watchedPath) {
    subscriptions.add(
      DBusSignalStream(
        bus,
        sender: owner,
        path: DBusObjectPath(watchedPath),
        interface: interface,
      ).listen(
        (_) {
          if (!closed) changed?.call();
        },
        onError: (Object _) {
          if (!closed) changed?.call();
        },
      ),
    );
  }

  void close() {
    closed = true;
    for (final s in subscriptions) {
      unawaited(s.cancel());
    }
  }
}

class _RegisteredMenu {
  _RegisteredMenu(this.xid, this.owner, this.path);
  final int xid;
  final String owner, path;
}

class _Registrar extends DBusObject {
  _Registrar(this.bus, this.changed)
    : super(DBusObjectPath(appMenuRegistrarPath));
  final DBusClient bus;
  final VoidCallback changed;
  final entries = <int, _RegisteredMenu>{};
  void ownerChanged(DBusNameOwnerChangedEvent event) {
    if (event.newOwner == null &&
        entries.values.any((e) => e.owner == event.name)) {
      final removed = entries.values
          .where((e) => e.owner == event.name)
          .toList();
      for (final e in removed) {
        entries.remove(e.xid);
        unawaited(
          emitSignal(appMenuRegistrarInterface, 'WindowUnregistered', [
            DBusUint32(e.xid),
          ]),
        );
      }
      changed();
    }
  }

  @override
  List<DBusIntrospectInterface> introspect() => [
    DBusIntrospectInterface(
      appMenuRegistrarInterface,
      methods: [
        DBusIntrospectMethod('RegisterWindow', args: [_arg('u'), _arg('o')]),
        DBusIntrospectMethod('UnregisterWindow', args: [_arg('u')]),
        DBusIntrospectMethod(
          'GetMenuForWindow',
          args: [_arg('u'), _arg('s', out: true), _arg('o', out: true)],
        ),
        DBusIntrospectMethod('GetMenus', args: [_arg('a(uso)', out: true)]),
      ],
      signals: [
        DBusIntrospectSignal(
          'WindowRegistered',
          args: [
            _arg('u', out: true),
            _arg('s', out: true),
            _arg('o', out: true),
          ],
        ),
        DBusIntrospectSignal(
          'WindowUnregistered',
          args: [_arg('u', out: true)],
        ),
      ],
    ),
  ];
  @override
  Future<DBusMethodResponse> handleMethodCall(DBusMethodCall call) async {
    if (call.interface != appMenuRegistrarInterface) {
      return DBusMethodErrorResponse.unknownInterface();
    }
    switch (call.name) {
      case 'RegisterWindow':
        if (call.signature != DBusSignature('uo') || call.sender == null) {
          return DBusMethodErrorResponse.invalidArgs();
        }
        final xid = call.values[0].asUint32();
        if (xid == 0 || entries.length >= 64 && !entries.containsKey(xid)) {
          return DBusMethodErrorResponse.invalidArgs();
        }
        final current = entries[xid];
        if (current != null && current.owner != call.sender) {
          return DBusMethodErrorResponse.failed(
            'Window already registered by another owner',
          );
        }
        final entry = _RegisteredMenu(
          xid,
          call.sender!,
          call.values[1].asObjectPath().value,
        );
        entries[xid] = entry;
        await emitSignal(appMenuRegistrarInterface, 'WindowRegistered', [
          DBusUint32(xid),
          DBusString(entry.owner),
          DBusObjectPath(entry.path),
        ]);
        changed();
        return DBusMethodSuccessResponse();
      case 'UnregisterWindow':
        if (call.signature != DBusSignature('u')) {
          return DBusMethodErrorResponse.invalidArgs();
        }
        final xid = call.values.single.asUint32();
        if (entries[xid]?.owner == call.sender) {
          entries.remove(xid);
          await emitSignal(appMenuRegistrarInterface, 'WindowUnregistered', [
            DBusUint32(xid),
          ]);
          changed();
        }
        return DBusMethodSuccessResponse();
      case 'GetMenuForWindow':
        if (call.signature != DBusSignature('u')) {
          return DBusMethodErrorResponse.invalidArgs();
        }
        final entry = entries[call.values.single.asUint32()];
        return DBusMethodSuccessResponse([
          DBusString(entry?.owner ?? ''),
          DBusObjectPath(entry?.path ?? '/'),
        ]);
      case 'GetMenus':
        if (call.signature != DBusSignature('')) {
          return DBusMethodErrorResponse.invalidArgs();
        }
        return DBusMethodSuccessResponse([
          DBusArray(DBusSignature('(uso)'), [
            for (final e in entries.values)
              DBusStruct([
                DBusUint32(e.xid),
                DBusString(e.owner),
                DBusObjectPath(e.path),
              ]),
          ]),
        ]);
      default:
        return DBusMethodErrorResponse.unknownMethod();
    }
  }
}

DBusIntrospectArgument _arg(String signature, {bool out = false}) =>
    DBusIntrospectArgument(
      DBusSignature(signature),
      out ? DBusArgumentDirection.out : DBusArgumentDirection.in_,
    );

String _text(DBusValue? value) => value is DBusString
    ? value.value
          .replaceAll('\u0000', '')
          .substring(
            0,
            value.value.replaceAll('\u0000', '').length.clamp(0, 512),
          )
    : '';
String _label(String text) => text
    .replaceAll('__', '\u0001')
    .replaceAll('_', '')
    .replaceAll('\u0001', '_');
bool _bool(DBusValue? value, [bool fallback = false]) =>
    value is DBusBoolean ? value.value : fallback;
int _number(DBusValue? value) => value is DBusInt32
    ? value.value
    : value is DBusUint32
    ? value.value
    : 0;

class _CanonicalSession extends _Session {
  _CanonicalSession(super.bus, super.owner, super.path, super.timeout) {
    watch(appDbusMenuInterface, path);
  }
  @override
  Future<_Snapshot> load() async {
    final response = await _object(bus, owner, path)
        .callMethod(appDbusMenuInterface, 'GetLayout', [
          const DBusInt32(0),
          const DBusInt32(6),
          DBusArray.string(const [
            'label',
            'enabled',
            'visible',
            'type',
            'children-display',
            'toggle-type',
            'toggle-state',
            'shortcut',
          ]),
        ], replySignature: DBusSignature('u(ia{sv}av)'))
        .timeout(timeout);
    final commands = <String, Future<void> Function()>{};
    var budget = 256;
    GlobalMenuItem? parse(DBusValue value, int depth) {
      if (--budget < 0 || depth > 6) return null;
      final tuple = value.asStruct();
      final id = tuple[0].asInt32();
      final p = tuple[1].asStringVariantDict();
      final children = <GlobalMenuItem>[];
      for (final raw in tuple[2].asArray().take(256)) {
        final item = parse(raw.asVariant(), depth + 1);
        if (item != null && item.visible) children.add(item);
      }
      final key = 'canonical:$id';
      final separator = _text(p['type']) == 'separator';
      final enabled = _bool(p['enabled'], true);
      final visible = _bool(p['visible'], true);
      final hasSubmenu = _text(p['children-display']) == 'submenu';
      if (id > 0 &&
          !separator &&
          enabled &&
          visible &&
          children.isEmpty &&
          !hasSubmenu) {
        commands[key] = () async {
          if (closed) return;
          await _object(bus, owner, path)
              .callMethod(appDbusMenuInterface, 'Event', [
                DBusInt32(id),
                const DBusString('clicked'),
                const DBusVariant(DBusInt32(0)),
                DBusUint32(DateTime.now().millisecondsSinceEpoch & 0xffffffff),
              ], replySignature: DBusSignature(''))
              .timeout(timeout);
        };
      }
      final toggle = _text(p['toggle-type']);
      var shortcut = '';
      try {
        shortcut =
            p['shortcut']?.asArray().first.asStringArray().join('+') ?? '';
      } on Object {
        /* The optional shortcut may be absent. */
      }
      return GlobalMenuItem(
        id: key,
        label: _label(_text(p['label'])),
        enabled: enabled,
        visible: visible,
        separator: separator,
        checked: toggle.isEmpty ? null : _number(p['toggle-state']) == 1,
        radio: toggle == 'radio',
        shortcut: shortcut,
        submenu: hasSubmenu,
        children: children,
      );
    }

    final root = parse(response.returnValues[1], 0);
    return _Snapshot(root?.children ?? const [], commands);
  }

  @override
  Future<bool> submenu(String id, bool open) async {
    if (!open || closed) return false;
    final number = int.tryParse(id.replaceFirst('canonical:', ''));
    if (number == null) return false;
    final response = await _object(bus, owner, path)
        .callMethod(appDbusMenuInterface, 'AboutToShow', [
          DBusInt32(number),
        ], replySignature: DBusSignature('b'))
        .timeout(timeout);
    return response.returnValues.single.asBoolean();
  }
}

class _ActionDescription {
  _ActionDescription(
    this.path,
    this.name,
    this.enabled,
    this.parameter,
    this.state,
  );
  final String path, name, parameter;
  final bool enabled;
  final DBusValue? state;
}

class _GtkSession extends _Session {
  _GtkSession(
    super.bus,
    super.owner,
    super.path,
    super.timeout, {
    String? appActions,
    String? windowActions,
    List<String> discoveredActions = const [],
  }) : appActions =
           appActions ??
           (path.contains('/menus/') ? path.split('/menus/').first : null),
       windowActions = windowActions ?? _onlyWindow(discoveredActions),
       actionPaths = {
         // Traditional appmenu GTK exporters put their action group on the
         // same object as org.gtk.Menus rather than a GtkApplication object.
         path,
         ...discoveredActions,
         ?appActions,
         ?windowActions,
         if (path.contains('/menus/')) path.split('/menus/').first,
       }.toList() {
    watch(gtkMenusInterface, path);
    for (final action in actionPaths) {
      watch(gtkActionsInterface, action);
    }
  }
  final String? appActions, windowActions;
  final List<String> actionPaths;
  final groups = <int>{};
  final _submenuActions = <String, _ActionDescription>{};
  final _openedActions = <String, _ActionDescription>{};
  Future<void> _tail = Future.value();
  static String? _onlyWindow(List<String> paths) {
    final matches = paths
        .where((p) => p.contains('/window/') || p.contains('/windows/'))
        .toList();
    return matches.length == 1 ? matches.single : null;
  }

  @override
  Future<_Snapshot> load() {
    final result = Completer<_Snapshot>();
    _tail = _tail.then((_) async {
      if (closed) {
        result.complete(_Snapshot(const [], const {}));
        return;
      }
      try {
        result.complete(await _read());
      } on Object catch (error, stack) {
        result.completeError(error, stack);
      }
    });
    return result.future;
  }

  Future<void> _endGroups() async {
    final values = groups.toList();
    groups.clear();
    if (values.isEmpty) return;
    try {
      await _object(bus, owner, path)
          .callMethod(gtkMenusInterface, 'End', [
            DBusArray.uint32(values),
          ], replySignature: DBusSignature(''))
          .timeout(timeout);
    } on Object {
      /* The exporter may already have left. */
    }
  }

  Future<_Snapshot> _read() async {
    final deadline = DateTime.now().add(const Duration(seconds: 3));
    await _endGroups();
    if (closed) return _Snapshot(const [], const {});
    final menus = <(int, int), List<Map<String, DBusValue>>>{};
    final pending = <int>{0};
    var remaining = 256;
    while (pending.isNotEmpty && groups.length < 64 && !closed) {
      if (DateTime.now().isAfter(deadline)) {
        throw TimeoutException('GTK menu read exceeded its budget');
      }
      final batch = pending.take(16).toList();
      pending.removeAll(batch);
      groups.addAll(batch);
      final response = await _object(bus, owner, path)
          .callMethod(gtkMenusInterface, 'Start', [
            DBusArray.uint32(batch),
          ], replySignature: DBusSignature('a(uuaa{sv})'))
          .timeout(timeout);
      if (closed) return _Snapshot(const [], const {});
      for (final raw in response.returnValues.single.asArray().take(64)) {
        final tuple = raw.asStruct();
        final key = (tuple[0].asUint32(), tuple[1].asUint32());
        final items = <Map<String, DBusValue>>[];
        for (final rawItem in tuple[2].asArray()) {
          if (remaining-- <= 0) break;
          final item = rawItem.asStringVariantDict();
          items.add(item);
          for (final name in [':section', ':submenu']) {
            final value = item[name];
            if (value?.signature == DBusSignature('(uu)')) {
              final group = value!.asStruct()[0].asUint32();
              if (!groups.contains(group)) pending.add(group);
            }
          }
        }
        menus[key] = items;
      }
      if (remaining <= 0) break;
    }
    final descriptions = <String, Map<String, _ActionDescription>>{};
    for (final actionPath in {
      ...actionPaths,
      ?appActions,
      ?windowActions,
    }.take(24)) {
      if (DateTime.now().isAfter(deadline)) break;
      if (closed) return _Snapshot(const [], const {});
      try {
        final response = await _object(bus, owner, actionPath)
            .callMethod(
              gtkActionsInterface,
              'DescribeAll',
              const [],
              replySignature: DBusSignature('a{s(bgav)}'),
            )
            .timeout(timeout);
        descriptions[actionPath] = {
          for (final entry
              in response.returnValues.single.asDict().entries.take(256))
            entry.key.asString(): _description(
              actionPath,
              entry.key.asString(),
              entry.value,
            ),
        };
      } on Object {
        /* Missing action groups leave their items disabled. */
      }
    }
    _ActionDescription? action(String name) {
      if (name.startsWith('win.')) {
        return descriptions[windowActions]?[name.substring(4)];
      }
      if (name.startsWith('app.')) {
        return descriptions[appActions]?[name.substring(4)];
      }
      if (name.startsWith('unity.')) {
        // This namespace belongs specifically to the menu object's action
        // group. Never guess a same-named action from another window/group.
        return descriptions[path]?[name.substring(6)];
      }
      final matches = [
        for (final d in descriptions.values)
          if (d[name] != null) d[name]!,
      ];
      return matches.length == 1 ? matches.single : null;
    }

    final commands = <String, Future<void> Function()>{};
    _submenuActions.clear();
    var budget = 256;
    List<GlobalMenuItem> tree(
      (int, int) key,
      int depth,
      Set<(int, int)> ancestors,
    ) {
      if (depth > 6 || ancestors.contains(key)) return const [];
      final items = <GlobalMenuItem>[];
      var index = 0;
      for (final p in menus[key] ?? const <Map<String, DBusValue>>[]) {
        if (--budget < 0) break;
        final id = 'gtk:${key.$1}:${key.$2}:${index++}';
        final section = p[':section'];
        final submenu = p[':submenu'];
        List<GlobalMenuItem> link(DBusValue? v) =>
            v?.signature == DBusSignature('(uu)')
            ? tree(
                (v!.asStruct()[0].asUint32(), v.asStruct()[1].asUint32()),
                depth + 1,
                {...ancestors, key},
              )
            : const [];
        if (section != null) {
          final children = link(section);
          if (children.isNotEmpty) {
            if (items.isNotEmpty) {
              items.add(
                GlobalMenuItem(id: '$id:separator', label: '', separator: true),
              );
            }
            items.addAll(children);
          }
          continue;
        }
        final name = _text(p['action']);
        final a = action(name);
        final target = p['target'];
        final children = link(submenu);
        final subAction = action(_text(p['submenu-action']));
        final enabled = submenu != null
            ? subAction?.enabled ?? true
            : a != null &&
                  a.enabled &&
                  (a.parameter.isEmpty && target == null ||
                      target?.signature.value == a.parameter);
        final hidden = _text(p['hidden-when']);
        if (hidden == 'action-missing' && a == null ||
            hidden == 'action-disabled' && !enabled) {
          continue;
        }
        final state = a?.state;
        final checked = state == null
            ? null
            : state is DBusBoolean
            ? state.value
            : target == null
            ? null
            : state == target;
        if (a != null && enabled && submenu == null) {
          commands[id] = () async {
            if (closed) return;
            await _object(bus, owner, a.path)
                .callMethod(gtkActionsInterface, 'Activate', [
                  DBusString(a.name),
                  DBusArray.variant([?target]),
                  DBusDict.stringVariant(const {}),
                ], replySignature: DBusSignature(''))
                .timeout(timeout);
          };
        }
        if (subAction != null &&
            subAction.enabled &&
            subAction.state is DBusBoolean) {
          _submenuActions[id] = subAction;
        }
        final label = _label(_text(p['label']));
        if (label.isEmpty) continue;
        items.add(
          GlobalMenuItem(
            id: id,
            label: label,
            enabled: enabled,
            checked: checked,
            radio: state != null && state is! DBusBoolean,
            shortcut: _text(p['accel']),
            submenu: submenu != null,
            children: children,
          ),
        );
      }
      return items;
    }

    return _Snapshot(tree((0, 0), 0, {}), commands);
  }

  _ActionDescription _description(String path, String name, DBusValue value) {
    final parts = value.asStruct();
    final states = parts[2].asArray();
    return _ActionDescription(
      path,
      name,
      parts[0].asBoolean(),
      parts[1].asSignature().value,
      states.isEmpty ? null : states.first.asVariant(),
    );
  }

  @override
  Future<bool> submenu(String id, bool open) async {
    final action = _submenuActions[id];
    if (closed || action == null) return false;
    final key = '${action.path}\u0000${action.name}';
    if (open) {
      _openedActions[key] = action;
    } else {
      _openedActions.remove(key);
    }
    await _submenuState(action, open);
    return true;
  }

  Future<void> _submenuState(_ActionDescription action, bool open) async {
    await _object(bus, owner, action.path)
        .callMethod(gtkActionsInterface, 'SetState', [
          DBusString(action.name),
          DBusVariant(DBusBoolean(open)),
          DBusDict.stringVariant(const {}),
        ], replySignature: DBusSignature(''))
        .timeout(timeout);
  }

  @override
  void close() {
    if (closed) return;
    super.close();
    // GTK boolean submenu-actions describe presentation as well as enablement.
    // Focus changes invalidate UI items before their closing callbacks run, so
    // the session must restore the old exporter's state itself.
    for (final action in _openedActions.values) {
      unawaited(_submenuState(action, false).catchError((Object _) {}));
    }
    _openedActions.clear();
    unawaited(_endGroups());
  }
}
