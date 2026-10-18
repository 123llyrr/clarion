// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:io';
import 'dart:typed_data';

import 'package:dbus/dbus.dart';

import '../models.dart';

const kdeWatcherName = 'org.kde.StatusNotifierWatcher';
const freedesktopWatcherName = 'org.freedesktop.StatusNotifierWatcher';
const watcherPath = '/StatusNotifierWatcher';
const kdeWatcherInterface = 'org.kde.StatusNotifierWatcher';
const freedesktopWatcherInterface = 'org.freedesktop.StatusNotifierWatcher';
const kdeItemInterface = 'org.kde.StatusNotifierItem';
const freedesktopItemInterface = 'org.freedesktop.StatusNotifierItem';
const menuInterface = 'com.canonical.dbusmenu';
const _itemInterfaces = [kdeItemInterface, freedesktopItemInterface];
const _watcherInterfaces = [kdeWatcherInterface, freedesktopWatcherInterface];
const _knownItemProperties = {
  'Id',
  'Title',
  'Status',
  'IconName',
  'IconPixmap',
  'AttentionIconName',
  'AttentionIconPixmap',
  'IconThemePath',
  'Menu',
  'ItemIsMenu',
};
const _itemSignalProperties = {
  'NewTitle': {'Title'},
  'NewStatus': {'Status'},
  'NewIcon': {'IconName', 'IconPixmap'},
  'NewAttentionIcon': {'AttentionIconName', 'AttentionIconPixmap'},
  'NewIconThemePath': {'IconThemePath'},
};
const _iconProperties = {
  'IconName',
  'IconPixmap',
  'AttentionIconName',
  'AttentionIconPixmap',
  'IconThemePath',
};

/// Hosts StatusNotifierItem / AppIndicator trays. Never replaces an existing
/// watcher; preview shells should pass no session bus of their own.
class TrayService {
  TrayService({DBusClient? bus}) : _injected = bus;
  final DBusClient? _injected;
  DBusClient? _bus;
  _WatcherObject? _watcher;
  final _changes = StreamController<void>.broadcast();
  final _menuChanges = StreamController<String>.broadcast();
  final _registrations = <String, _Registration>{};
  final _items = <String, TrayItem>{};
  final _itemRegistrations = Expando<_Registration>('tray registration');
  final _itemInterfacesById = <String, String>{};
  final _itemProperties = <String, Map<String, DBusValue>>{};
  final _normalPixmaps = <String, TrayIconPixmap?>{};
  final _attentionPixmaps = <String, TrayIconPixmap?>{};
  final _pending = <String, _PendingRefresh>{};
  final _refreshing = <String, _Registration>{};
  final _ownerVersions = <String, int>{};
  final _registering = <String>{};
  final _itemRevisions = <String, int>{};
  final _signals = <StreamSubscription<DBusSignal>>[];
  StreamSubscription<DBusNameOwnerChangedEvent>? _owners;
  StreamSubscription<DBusSignal>? _externalSignals;
  Timer? _coalesce;
  Timer? _externalTimer;
  bool _ownsWatcher = false;
  Future<bool>? _startFuture;
  bool _disposed = false;
  bool _syncingWatcher = false;
  bool _watcherDirty = false;
  int _watcherRevision = 0;
  int _externalRefreshRevision = 0;
  String? _externalOwner;
  String? _externalName;
  String? _externalInterface;
  late String _hostName;
  bool available = false;

  static const _readTimeout = Duration(seconds: 2);
  static const _methodTimeout = Duration(seconds: 4);
  static const _coalesceDelay = Duration(milliseconds: 45);
  static const _maxItems = 64;
  static const _maxMenuItems = 256;
  static const _maxMenuDepth = 5;

  Stream<void> get changes => _changes.stream;

  /// A currently exported menu changed; the host can reload an open menu.
  Stream<String> get menuChanges => _menuChanges.stream;
  List<TrayItem> get items => List.unmodifiable(_ordered());

  /// Stable for metadata snapshots of one live exporter, but invalid after
  /// removal or owner replacement. No D-Bus request is needed to validate it.
  Object? registrationToken(TrayItem item) => _registrationForItem(item);

  _Registration? _registrationForItem(TrayItem item) {
    final registration = _itemRegistrations[item];
    return registration != null &&
            _current(registration) &&
            _items.containsKey(registration.id)
        ? registration
        : null;
  }

  /// A native menu must target the exact current exporter, never a name's next
  /// owner or an old item retained by a delayed pointer/platform operation.
  /// Resolving this snapshot does not require another D-Bus round trip.
  Map<String, String>? nativeMenuTarget(TrayItem item) {
    final registration = _registrations[item.id];
    if (registration == null ||
        !_current(registration) ||
        !identical(_items[item.id], item) ||
        !item.menuAvailable ||
        item.menuPath.isEmpty ||
        item.menuPath == '/' ||
        !registration.owner.startsWith(':') ||
        !_validBusName(registration.owner)) {
      return null;
    }
    try {
      DBusObjectPath(item.menuPath);
    } on Object {
      return null;
    }
    return {'busName': registration.owner, 'menuPath': item.menuPath};
  }

  Future<bool> start() =>
      _disposed ? Future.value(false) : _startFuture ??= _start();

  Future<bool> _start() async {
    final bus = _bus = _injected ?? DBusClient.session();
    final watcher = _watcher = _WatcherObject(this);
    try {
      await bus.registerObject(watcher);
      if (_disposed) return false;
      _hostName = 'org.kde.StatusNotifierHost.Clarion${pid.toString()}';
      await bus.requestName(_hostName, flags: {DBusRequestNameFlag.doNotQueue});
      if (_disposed) return false;
      for (final interface in _itemInterfaces) {
        _signals.add(
          DBusSignalStream(
            bus,
            interface: interface,
          ).listen(_handleItemSignal, onError: (_) => _refreshAll()),
        );
      }
      _signals.add(
        DBusSignalStream(
          bus,
          interface: 'org.freedesktop.DBus.Properties',
          name: 'PropertiesChanged',
        ).listen(_handlePropertiesChanged, onError: (_) => _refreshAll()),
      );
      _signals.add(
        DBusSignalStream(
          bus,
          interface: menuInterface,
        ).listen(_handleMenuSignal, onError: (Object _) {}),
      );
      _owners = bus.nameOwnerChanged.listen(_handleOwnerChanged);
      await _syncWatcher();
      return available;
    } on Object {
      available = false;
      await bus.close();
      return false;
    }
  }

  Future<void> _syncWatcher() async {
    if (_disposed || _ownsWatcher) return;
    _watcherDirty = true;
    if (_syncingWatcher) return;
    _syncingWatcher = true;
    try {
      while (_watcherDirty && !_disposed && !_ownsWatcher) {
        _watcherDirty = false;
        final revision = _watcherRevision;
        final bus = _bus!;
        String? name;
        String? owner;
        for (final candidate in [kdeWatcherName, freedesktopWatcherName]) {
          final found = await bus.getNameOwner(candidate).timeout(_readTimeout);
          if (found != null && found != bus.uniqueName) {
            name = candidate;
            owner = found;
            break;
          }
        }
        if (_disposed || revision != _watcherRevision) continue;
        if (name != null && owner != null) {
          final interface = name == kdeWatcherName
              ? kdeWatcherInterface
              : freedesktopWatcherInterface;
          final joined = await _joinExternalWatcher(
            name,
            owner,
            interface,
            revision,
          );
          if (revision == _watcherRevision && !_disposed) {
            available = joined;
            _changed();
          }
          continue;
        }
        final reply = await bus.requestName(
          kdeWatcherName,
          flags: {DBusRequestNameFlag.doNotQueue},
        );
        if (_disposed) return;
        if (reply != DBusRequestNameReply.primaryOwner &&
            reply != DBusRequestNameReply.alreadyOwner) {
          _watcherDirty = true;
          continue;
        }
        _ownsWatcher = true;
        _externalOwner = _externalName = _externalInterface = null;
        await _externalSignals?.cancel();
        _externalSignals = null;
        await bus.requestName(
          freedesktopWatcherName,
          flags: {DBusRequestNameFlag.doNotQueue},
        );
        await bus.requestName(
          'org.freedesktop.StatusNotifierHost-Clarion${pid.toString()}',
          flags: {DBusRequestNameFlag.doNotQueue},
        );
        if (_disposed) return;
        _watcher!.hostRegistered = true;
        await _watcher!.emitHostRegistered();
        available = true;
        _changed();
      }
    } on Object {
      available = false;
      _changed();
    } finally {
      _syncingWatcher = false;
    }
  }

  Future<bool> _joinExternalWatcher(
    String name,
    String owner,
    String interface,
    int revision,
  ) async {
    final object = DBusRemoteObject(
      _bus!,
      name: owner,
      path: DBusObjectPath(watcherPath),
    );
    try {
      await object
          .callMethod(interface, 'RegisterStatusNotifierHost', [
            DBusString(_hostName),
          ], replySignature: DBusSignature(''))
          .timeout(_methodTimeout);
      if (_disposed || revision != _watcherRevision) return false;
      await _externalSignals?.cancel();
      _externalOwner = owner;
      _externalName = name;
      _externalInterface = interface;
      _externalSignals = DBusSignalStream(
        _bus!,
        sender: owner,
        path: DBusObjectPath(watcherPath),
        interface: interface,
      ).listen((_) => _scheduleExternalRefresh());
      await _refreshExternal(object, interface, revision);
      return true;
    } on Object {
      return false;
    }
  }

  void _scheduleExternalRefresh() {
    if (_disposed || _ownsWatcher || _externalOwner == null) return;
    _externalTimer?.cancel();
    _externalTimer = Timer(_coalesceDelay, () {
      _externalTimer = null;
      final owner = _externalOwner;
      final interface = _externalInterface;
      if (_disposed || owner == null || interface == null) return;
      final revision = _watcherRevision;
      unawaited(
        _refreshExternal(
          DBusRemoteObject(
            _bus!,
            name: owner,
            path: DBusObjectPath(watcherPath),
          ),
          interface,
          revision,
        ),
      );
    });
  }

  Future<void> _refreshExternal(
    DBusRemoteObject object,
    String interface,
    int revision,
  ) async {
    final refreshRevision = ++_externalRefreshRevision;
    try {
      final value = await object
          .getProperty(
            interface,
            'RegisteredStatusNotifierItems',
            signature: DBusSignature('as'),
          )
          .timeout(_readTimeout);
      final addresses = value
          .asStringArray()
          .map((address) => _parseRegistration(address, sender: null)?.address)
          .whereType<String>()
          // A broken watcher can repeat the same few addresses indefinitely.
          // Apply the item budget after normalization/deduplication so these
          // repetitions do not hide a later, different application.
          .toSet()
          .take(_maxItems)
          .toSet();
      if (_disposed ||
          _ownsWatcher ||
          revision != _watcherRevision ||
          refreshRevision != _externalRefreshRevision) {
        return;
      }
      final retained = <String>{};
      for (final address in addresses) {
        final registration = await _register(
          address,
          sender: null,
          emit: false,
          watcherRevision: revision,
          externalRefreshRevision: refreshRevision,
        );
        if (registration != null) retained.add(registration.id);
      }
      if (_disposed ||
          _ownsWatcher ||
          revision != _watcherRevision ||
          refreshRevision != _externalRefreshRevision) {
        return;
      }
      final removed = _registrations.values
          .where((item) => !retained.contains(item.id))
          .toList();
      for (final item in removed) {
        _forget(item.id);
      }
      if (removed.isNotEmpty) _changed();
    } on Object {
      /* Owner-change retries when the external watcher restarts. */
    }
  }

  Future<void> registerItem(String address, String? sender) async {
    await _register(address, sender: sender, emit: true);
  }

  Future<_Registration?> _register(
    String address, {
    required String? sender,
    required bool emit,
    int? watcherRevision,
    int? externalRefreshRevision,
  }) async {
    if (_disposed) return null;
    final parsed = _parseRegistration(address, sender: sender);
    if (parsed == null) return null;
    if (_registrations[parsed.id] case final existing?) return existing;
    if (_registering.length >= _maxItems || !_registering.add(parsed.id)) {
      return null;
    }
    _ownerVersions.putIfAbsent(parsed.busName, () => 0);
    try {
      String? owner;
      // NameOwnerChanged can be delivered while a daemon lookup is in flight.
      // Retry a replacement rather than silently losing its first registration.
      for (var attempt = 0; attempt < 3 && !_disposed; attempt++) {
        final version = _ownerVersions[parsed.busName] ?? 0;
        owner = await _bus!.getNameOwner(parsed.busName).timeout(_readTimeout);
        if (version == (_ownerVersions[parsed.busName] ?? 0)) break;
        owner = null;
      }
      if (owner == null ||
          owner.isEmpty ||
          _disposed ||
          (watcherRevision != null && watcherRevision != _watcherRevision) ||
          (externalRefreshRevision != null &&
              externalRefreshRevision != _externalRefreshRevision) ||
          _registrations.containsKey(parsed.id)) {
        return null;
      }
      for (final existing in _registrations.values) {
        if (existing.owner == owner && existing.path == parsed.path) {
          return existing;
        }
      }
      if (_registrations.length >= _maxItems) return null;
      final registration = parsed.withOwner(owner);
      _registrations[registration.id] = registration;
      if (_ownsWatcher) {
        _watcher!.registeredItems = _registrations.values.map((i) => i.address);
        if (emit) await _watcher!.emitItemRegistered(registration.address);
      }
      if (_current(registration)) {
        _scheduleRefresh(registration.id, full: true, immediate: true);
      }
      return _current(registration) ? registration : null;
    } on Object {
      return null;
    } finally {
      _registering.remove(parsed.id);
      _releaseOwnerVersion(parsed.busName);
    }
  }

  void _handleItemSignal(DBusSignal signal) {
    final registration = _registrationFor(signal);
    final properties = _itemSignalProperties[signal.name];
    if (registration == null || properties == null) return;
    _itemRevisions[registration.id] =
        (_itemRevisions[registration.id] ?? 0) + 1;
    _itemInterfacesById[registration.id] = signal.interface;
    _scheduleRefresh(registration.id, properties: properties);
  }

  void _handlePropertiesChanged(DBusSignal signal) {
    final registration = _registrationFor(signal);
    if (registration == null || signal.signature != DBusSignature('sa{sv}as')) {
      return;
    }
    try {
      final changed = DBusPropertiesChangedSignal(signal);
      if (!_itemInterfaces.contains(changed.propertiesInterface)) return;
      _itemRevisions[registration.id] =
          (_itemRevisions[registration.id] ?? 0) + 1;
      _itemInterfacesById[registration.id] = changed.propertiesInterface;
      final properties = _itemProperties.putIfAbsent(registration.id, () => {});
      var updated = false;
      for (final entry in changed.changedProperties.entries) {
        if (_knownItemProperties.contains(entry.key)) {
          properties[entry.key] = entry.value;
          updated = true;
        }
      }
      if (updated) {
        _updateItem(
          registration,
          iconChanged: changed.changedProperties.keys.any(
            _iconProperties.contains,
          ),
        );
      }
      final invalidated = changed.invalidatedProperties
          .where(_knownItemProperties.contains)
          .toSet();
      if (invalidated.isNotEmpty) {
        _scheduleRefresh(registration.id, properties: invalidated);
      }
    } on Object {
      _scheduleRefresh(registration.id, full: true);
    }
  }

  _Registration? _registrationFor(DBusSignal signal) {
    final sender = signal.sender;
    if (sender == null) return null;
    for (final item in _registrations.values) {
      if (item.owner == sender && item.path == signal.path.value) return item;
    }
    return null;
  }

  void _handleOwnerChanged(DBusNameOwnerChangedEvent event) {
    if (_disposed) return;
    // A queued initial acquisition confirms the first owner; it does not
    // invalidate a concurrent lookup of that same owner on a real dbus-daemon.
    if (event.oldOwner != null && _ownerVersions.containsKey(event.name)) {
      _ownerVersions[event.name] = _ownerVersions[event.name]! + 1;
    }
    if (!_ownsWatcher &&
        (event.name == kdeWatcherName ||
            event.name == freedesktopWatcherName)) {
      _watcherRevision++;
      _externalRefreshRevision++;
      _externalTimer?.cancel();
      _externalTimer = null;
      if (event.name == _externalName && event.oldOwner == _externalOwner) {
        _externalOwner = _externalName = _externalInterface = null;
        unawaited(_externalSignals?.cancel());
        _externalSignals = null;
        for (final id in _registrations.keys.toList()) {
          _forget(id);
        }
        available = false;
        _changed();
      }
      unawaited(_syncWatcher());
    }
    if (event.oldOwner == null) return;
    final removed = _registrations.values
        .where(
          (item) =>
              (item.busName == event.name && item.owner == event.oldOwner) ||
              item.owner == event.name,
        )
        .toList();
    for (final item in removed) {
      _forget(item.id);
      if (_ownsWatcher) {
        unawaited(
          _watcher!
              .emitItemUnregistered(item.address)
              .catchError((Object _) {}),
        );
      }
    }
    if (removed.isNotEmpty) {
      _watcher?.registeredItems = _registrations.values.map((i) => i.address);
      _changed();
    }
  }

  void _forget(String id) {
    final registration = _registrations.remove(id);
    _items.remove(id);
    _itemInterfacesById.remove(id);
    _itemProperties.remove(id);
    _normalPixmaps.remove(id);
    _attentionPixmaps.remove(id);
    _pending.remove(id);
    _refreshing.remove(id);
    _itemRevisions.remove(id);
    if (registration != null) _releaseOwnerVersion(registration.busName);
  }

  void _releaseOwnerVersion(String name) {
    if (!_registrations.values.any((item) => item.busName == name) &&
        !_registering.any((id) => id.startsWith('status-notifier:$name:'))) {
      _ownerVersions.remove(name);
    }
  }

  bool _current(_Registration registration) =>
      !_disposed && identical(_registrations[registration.id], registration);

  void _handleMenuSignal(DBusSignal signal) {
    if (_disposed ||
        !((signal.name == 'LayoutUpdated' &&
                signal.signature == DBusSignature('ui')) ||
            (signal.name == 'ItemsPropertiesUpdated' &&
                signal.signature == DBusSignature('a(ia{sv})a(ias)')))) {
      return;
    }
    for (final registration in _registrations.values) {
      if (registration.owner == signal.sender &&
          _items[registration.id]?.menuPath == signal.path.value) {
        _menuChanges.add(registration.id);
      }
    }
  }

  void _refreshAll() {
    for (final item in _registrations.values) {
      _scheduleRefresh(item.id, full: true);
    }
  }

  void _scheduleRefresh(
    String id, {
    Set<String> properties = const {},
    bool full = false,
    bool immediate = false,
  }) {
    if (_disposed) return;
    final pending = _pending.putIfAbsent(id, _PendingRefresh.new);
    if (full) {
      pending.full = true;
      pending.properties.clear();
    } else if (!pending.full) {
      pending.properties.addAll(
        properties.where(_knownItemProperties.contains),
      );
    }
    if (immediate && _coalesce != null) {
      _coalesce!.cancel();
      _coalesce = null;
    }
    _coalesce ??= Timer(immediate ? Duration.zero : _coalesceDelay, () {
      _coalesce = null;
      _drain();
    });
  }

  void _drain() {
    if (_disposed) return;
    for (final id in _pending.keys.toList()) {
      final registration = _registrations[id];
      if (registration == null) {
        _pending.remove(id);
        continue;
      }
      if (identical(_refreshing[id], registration)) continue;
      final pending = _pending.remove(id)!;
      _refreshing[id] = registration;
      unawaited(_refreshItem(registration, pending));
    }
  }

  Future<void> _refreshItem(
    _Registration registration,
    _PendingRefresh pending,
  ) async {
    try {
      if (pending.full) {
        await _refreshAllProperties(registration);
      } else {
        await _refreshProperties(registration, pending.properties);
      }
    } finally {
      // A replacement using the same public name may already have its own read.
      // Completing the old owner must not release that new owner's flight.
      if (identical(_refreshing[registration.id], registration)) {
        _refreshing.remove(registration.id);
        _drain();
      }
    }
  }

  Future<void> _refreshAllProperties(_Registration registration) async {
    if (!_current(registration)) return;
    final revision = _itemRevisions[registration.id] ?? 0;
    final object = _remote(registration);
    final preferred = _itemInterfacesById[registration.id];
    for (final interface in {?preferred, ..._itemInterfaces}) {
      try {
        final properties = await object
            .getAllProperties(interface)
            .timeout(_readTimeout);
        if (!_current(registration)) return;
        if (revision != (_itemRevisions[registration.id] ?? 0)) {
          _scheduleRefresh(registration.id, full: true);
          return;
        }
        _itemInterfacesById[registration.id] = interface;
        _itemProperties[registration.id] = Map.of(properties);
        _updateItem(registration, iconChanged: true);
        return;
      } on Object {
        continue;
      }
    }
  }

  Future<void> _refreshProperties(
    _Registration registration,
    Set<String> names,
  ) async {
    if (names.isEmpty || !_current(registration)) return;
    final revision = _itemRevisions[registration.id] ?? 0;
    final object = _remote(registration);
    final preferred = _itemInterfacesById[registration.id];
    for (final interface in {?preferred, ..._itemInterfaces}) {
      final resolved = <MapEntry<String, DBusValue>>[];
      final requested = names.toList();
      var next = 0;
      Future<void> readProperties() async {
        while (next < requested.length && _current(registration)) {
          final name = requested[next++];
          try {
            resolved.add(
              MapEntry(
                name,
                await object.getProperty(interface, name).timeout(_readTimeout),
              ),
            );
          } on Object {
            continue;
          }
        }
      }

      // Property getters are independent. Bound requests within each item, but
      // never put another application behind a slow exporter or property.
      await Future.wait([
        for (var worker = 0; worker < requested.length && worker < 4; worker++)
          readProperties(),
      ]);
      if (!_current(registration)) return;
      if (revision != (_itemRevisions[registration.id] ?? 0)) {
        _scheduleRefresh(registration.id, full: true);
        return;
      }
      if (resolved.isEmpty) continue;
      _itemInterfacesById[registration.id] = interface;
      _itemProperties
          .putIfAbsent(registration.id, () => {})
          .addEntries(resolved);
      _updateItem(
        registration,
        iconChanged: resolved.any(
          (entry) => _iconProperties.contains(entry.key),
        ),
      );
      return;
    }
  }

  void _updateItem(_Registration registration, {bool iconChanged = false}) {
    final properties = _itemProperties[registration.id];
    if (properties == null || !_current(registration)) return;
    if (properties.remove('IconPixmap') case final raw?) {
      _normalPixmaps[registration.id] = decodeTrayPixmap(raw);
    }
    if (properties.remove('AttentionIconPixmap') case final raw?) {
      _attentionPixmaps[registration.id] = decodeTrayPixmap(raw);
    }
    final status = _status(_string(properties['Status']));
    final attention = status == TrayStatus.needsAttention;
    var pixmap = attention
        ? _attentionPixmaps[registration.id]
        : _normalPixmaps[registration.id];
    if (attention) pixmap ??= _normalPixmaps[registration.id];
    var iconName = _bounded(
      _string(properties[attention ? 'AttentionIconName' : 'IconName']),
      512,
    );
    if (attention && iconName.isEmpty) {
      iconName = _bounded(_string(properties['IconName']), 512);
    }
    final itemId = _bounded(_string(properties['Id']), 256);
    final rawTitle = _bounded(_string(properties['Title']), 256);
    final title = rawTitle.isNotEmpty
        ? rawTitle
        : itemId.isNotEmpty
        ? itemId
        : registration.busName;
    final menuPath = _objectPath(properties['Menu']);
    final item = TrayItem(
      id: registration.id,
      title: title,
      status: status,
      iconName: iconName,
      iconThemePath: _bounded(_string(properties['IconThemePath']), 4096),
      iconPixmap: pixmap,
      iconRevision:
          (_items[registration.id]?.iconRevision ?? 0) + (iconChanged ? 1 : 0),
      menuAvailable: menuPath != null && menuPath != '/',
      primaryOpensMenu: _boolean(properties['ItemIsMenu']),
      menuPath: menuPath ?? '',
    );
    _itemRegistrations[item] = registration;
    _items[registration.id] = item;
    _changed();
  }

  Future<bool> invoke(
    TrayItem item,
    TrayAction action, {
    double x = 0,
    double y = 0,
  }) async {
    final registration = _registrationForItem(item);
    if (registration == null) return false;
    final method = switch (action) {
      TrayAction.activate => 'Activate',
      TrayAction.secondaryActivate => 'SecondaryActivate',
      TrayAction.contextMenu => 'ContextMenu',
    };
    final px = x.isFinite ? x.round().clamp(-0x80000000, 0x7fffffff) : 0;
    final py = y.isFinite ? y.round().clamp(-0x80000000, 0x7fffffff) : 0;
    final preferred = _itemInterfacesById[registration.id];
    for (final interface in {?preferred, ..._itemInterfaces}) {
      try {
        if (!_current(registration)) return false;
        await _remote(registration)
            .callMethod(interface, method, [
              DBusInt32(px),
              DBusInt32(py),
            ], replySignature: DBusSignature(''))
            .timeout(_methodTimeout);
        if (!_current(registration)) return false;
        _itemInterfacesById[registration.id] = interface;
        return true;
      } on Object {
        continue;
      }
    }
    _scheduleRefresh(registration.id, full: true, immediate: true);
    return false;
  }

  Future<bool> scroll(
    TrayItem item,
    int delta, {
    String orientation = 'vertical',
  }) async {
    final registration = _registrationForItem(item);
    if (registration == null ||
        !_current(registration) ||
        delta == 0 ||
        (orientation != 'vertical' && orientation != 'horizontal')) {
      return false;
    }
    final preferred = _itemInterfacesById[registration.id];
    for (final interface in {?preferred, ..._itemInterfaces}) {
      if (!_current(registration)) return false;
      try {
        await _remote(registration)
            .callMethod(interface, 'Scroll', [
              DBusInt32(delta.clamp(-0x80000000, 0x7fffffff)),
              DBusString(orientation),
            ], replySignature: DBusSignature(''))
            .timeout(_methodTimeout);
        return _current(registration);
      } on Object {
        continue;
      }
    }
    return false;
  }

  Future<List<TrayMenuEntry>?> loadMenu(TrayItem item) async {
    final registration = _registrationForItem(item);
    if (registration == null ||
        _disposed ||
        !item.menuAvailable ||
        item.menuPath.isEmpty ||
        item.menuPath == '/') {
      return null;
    }
    DBusObjectPath path;
    try {
      path = DBusObjectPath(item.menuPath);
    } on Object {
      return null;
    }
    final object = DBusRemoteObject(
      _bus!,
      name: registration.owner,
      path: path,
    );
    try {
      await object
          .callMethod(menuInterface, 'AboutToShow', const [
            DBusInt32(0),
          ], replySignature: DBusSignature('b'))
          .timeout(_methodTimeout);
    } on Object {
      /* Some exporters omit AboutToShow. */
    }
    if (!_current(registration)) return null;
    try {
      final response = await object
          .callMethod(menuInterface, 'GetLayout', [
            const DBusInt32(0),
            const DBusInt32(_maxMenuDepth),
            DBusArray.string(const [
              'label',
              'enabled',
              'visible',
              'type',
              'children-display',
              'toggle-type',
              'toggle-state',
              'disposition',
            ]),
          ], replySignature: DBusSignature('u(ia{sv}av)'))
          .timeout(_methodTimeout);
      if (!_current(registration)) return null;
      if (response.returnValues.length != 2) return null;
      final root = parseTrayMenu(
        response.returnValues[1],
        maxEntries: _maxMenuItems,
      );
      if (root == null) return null;
      return List.unmodifiable(root.children.where((entry) => entry.visible));
    } on Object {
      return null;
    }
  }

  Future<bool> activateMenuEntry(TrayItem item, int entryId) async {
    final registration = _registrationForItem(item);
    if (registration == null ||
        _disposed ||
        entryId <= 0 ||
        entryId > 0x7fffffff ||
        item.menuPath.isEmpty ||
        item.menuPath == '/') {
      return false;
    }
    try {
      await DBusRemoteObject(
            _bus!,
            name: registration.owner,
            path: DBusObjectPath(item.menuPath),
          )
          .callMethod(menuInterface, 'Event', [
            DBusInt32(entryId),
            const DBusString('clicked'),
            const DBusVariant(DBusString('')),
            DBusUint32(DateTime.now().millisecondsSinceEpoch & 0xffffffff),
          ], replySignature: DBusSignature(''))
          .timeout(_methodTimeout);
      return _current(registration);
    } on Object {
      return false;
    }
  }

  DBusRemoteObject _remote(_Registration registration) => DBusRemoteObject(
    _bus!,
    name: registration.owner,
    path: DBusObjectPath(registration.path),
  );

  List<TrayItem> _ordered() {
    final items = _items.values.toList()
      ..sort((a, b) {
        final byStatus = _statusPriority(
          a.status,
        ).compareTo(_statusPriority(b.status));
        return byStatus != 0
            ? byStatus
            : a.title.toLowerCase().compareTo(b.title.toLowerCase());
      });
    return items;
  }

  void _changed() {
    if (!_disposed) _changes.add(null);
  }

  void dispose() {
    if (_disposed) return;
    _disposed = true;
    available = false;
    _coalesce?.cancel();
    _externalTimer?.cancel();
    _pending.clear();
    _refreshing.clear();
    _registrations.clear();
    _items.clear();
    _itemInterfacesById.clear();
    _itemProperties.clear();
    _normalPixmaps.clear();
    _attentionPixmaps.clear();
    _itemRevisions.clear();
    for (final subscription in _signals) {
      unawaited(subscription.cancel());
    }
    unawaited(_externalSignals?.cancel());
    unawaited(_owners?.cancel());
    unawaited(_changes.close());
    unawaited(_menuChanges.close());
    final bus = _bus;
    if (bus != null) unawaited(bus.close());
  }
}

class _WatcherObject extends DBusObject {
  _WatcherObject(this._service) : super(DBusObjectPath(watcherPath));
  final TrayService _service;
  Iterable<String> registeredItems = const [];
  bool hostRegistered = false;

  Future<void> emitItemRegistered(String address) async {
    for (final interface in _watcherInterfaces) {
      await emitSignal(interface, 'StatusNotifierItemRegistered', [
        DBusString(address),
      ]);
    }
  }

  Future<void> emitItemUnregistered(String address) async {
    for (final interface in _watcherInterfaces) {
      await emitSignal(interface, 'StatusNotifierItemUnregistered', [
        DBusString(address),
      ]);
    }
  }

  Future<void> emitHostRegistered() async {
    for (final interface in _watcherInterfaces) {
      await emitSignal(interface, 'StatusNotifierHostRegistered');
    }
  }

  @override
  List<DBusIntrospectInterface> introspect() => [
    for (final interface in _watcherInterfaces)
      DBusIntrospectInterface(
        interface,
        methods: [
          _method('RegisterStatusNotifierItem'),
          _method('RegisterStatusNotifierHost'),
        ],
        properties: [
          DBusIntrospectProperty(
            'RegisteredStatusNotifierItems',
            DBusSignature('as'),
            access: DBusPropertyAccess.read,
          ),
          DBusIntrospectProperty(
            'IsStatusNotifierHostRegistered',
            DBusSignature('b'),
            access: DBusPropertyAccess.read,
          ),
          DBusIntrospectProperty(
            'ProtocolVersion',
            DBusSignature('i'),
            access: DBusPropertyAccess.read,
          ),
        ],
        signals: [
          _signal('StatusNotifierItemRegistered'),
          _signal('StatusNotifierItemUnregistered'),
          DBusIntrospectSignal('StatusNotifierHostRegistered'),
          DBusIntrospectSignal('StatusNotifierHostUnregistered'),
        ],
      ),
  ];

  @override
  Future<DBusMethodResponse> handleMethodCall(DBusMethodCall methodCall) async {
    if (!_watcherInterfaces.contains(methodCall.interface)) {
      return DBusMethodErrorResponse.unknownInterface();
    }
    if (methodCall.signature != DBusSignature('s')) {
      return DBusMethodErrorResponse.invalidArgs();
    }
    switch (methodCall.name) {
      case 'RegisterStatusNotifierItem':
        await _service.registerItem(
          methodCall.values.first.asString(),
          methodCall.sender,
        );
        return DBusMethodSuccessResponse();
      case 'RegisterStatusNotifierHost':
        hostRegistered = true;
        await emitHostRegistered();
        return DBusMethodSuccessResponse();
      default:
        return DBusMethodErrorResponse.unknownMethod();
    }
  }

  @override
  Future<DBusMethodResponse> getProperty(String interface, String name) async {
    if (!_watcherInterfaces.contains(interface)) {
      return DBusMethodErrorResponse.unknownInterface();
    }
    return switch (name) {
      'RegisteredStatusNotifierItems' => DBusGetPropertyResponse(
        DBusArray.string(registeredItems),
      ),
      'IsStatusNotifierHostRegistered' => DBusGetPropertyResponse(
        DBusBoolean(hostRegistered),
      ),
      'ProtocolVersion' => DBusGetPropertyResponse(const DBusInt32(0)),
      _ => DBusMethodErrorResponse.unknownProperty(),
    };
  }

  @override
  Future<DBusMethodResponse> getAllProperties(String interface) async {
    if (!_watcherInterfaces.contains(interface)) {
      return DBusMethodErrorResponse.unknownInterface();
    }
    return DBusGetAllPropertiesResponse({
      'RegisteredStatusNotifierItems': DBusArray.string(registeredItems),
      'IsStatusNotifierHostRegistered': DBusBoolean(hostRegistered),
      'ProtocolVersion': const DBusInt32(0),
    });
  }
}

DBusIntrospectMethod _method(String name) => DBusIntrospectMethod(
  name,
  args: [DBusIntrospectArgument(DBusSignature('s'), DBusArgumentDirection.in_)],
);

DBusIntrospectSignal _signal(String name) => DBusIntrospectSignal(
  name,
  args: [DBusIntrospectArgument(DBusSignature('s'), DBusArgumentDirection.out)],
);

class _PendingRefresh {
  bool full = false;
  final properties = <String>{};
}

class _Registration {
  const _Registration({
    required this.busName,
    required this.path,
    required this.owner,
  });
  final String busName;
  final String path;
  final String owner;
  String get id => 'status-notifier:$busName:$path';
  String get address => '$busName$path';
  _Registration withOwner(String value) =>
      _Registration(busName: busName, path: path, owner: value);
}

class _MenuBudget {
  _MenuBudget(this.remaining);
  int remaining;
}

_Registration? _parseRegistration(String value, {required String? sender}) {
  final trimmed = value.trim();
  if (trimmed.isEmpty || trimmed.length > 4096) return null;
  if (trimmed.startsWith('/')) {
    if (sender == null || sender.isEmpty) return null;
    try {
      DBusObjectPath(trimmed);
      return _Registration(busName: sender, path: trimmed, owner: sender);
    } on Object {
      return null;
    }
  }
  final slash = trimmed.indexOf('/');
  final busName = slash < 0 ? trimmed : trimmed.substring(0, slash);
  final path = slash < 0 ? '/StatusNotifierItem' : trimmed.substring(slash);
  try {
    if (!_validBusName(busName)) return null;
    DBusObjectPath(path);
    return _Registration(busName: busName, path: path, owner: '');
  } on Object {
    return null;
  }
}

bool _validBusName(String value) {
  if (value.isEmpty || value.length > 255) return false;
  final unique = value.startsWith(':');
  final body = unique ? value.substring(1) : value;
  final parts = body.split('.');
  if (parts.length < 2 || parts.any((part) => part.isEmpty)) return false;
  final segment = unique
      ? RegExp(r'^[A-Za-z0-9_-]+$')
      : RegExp(r'^[A-Za-z_-][A-Za-z0-9_-]*$');
  return parts.every(segment.hasMatch);
}

String _string(DBusValue? value) {
  try {
    return value?.asString() ?? '';
  } on Object {
    return '';
  }
}

String? _objectPath(DBusValue? value) {
  try {
    return value?.asObjectPath().value;
  } on Object {
    return null;
  }
}

bool _boolean(DBusValue? value) {
  try {
    return value?.asBoolean() ?? false;
  } on Object {
    return false;
  }
}

int _int32(DBusValue? value) {
  try {
    return value?.asInt32() ?? 0;
  } on Object {
    return 0;
  }
}

String _bounded(String value, int maxLength) {
  final normalized = value.replaceAll('\u0000', '').trim();
  return normalized.length <= maxLength
      ? normalized
      : normalized.substring(0, maxLength);
}

TrayStatus _status(String value) => switch (value.toLowerCase()) {
  'passive' => TrayStatus.passive,
  'needsattention' => TrayStatus.needsAttention,
  _ => TrayStatus.active,
};

int _statusPriority(TrayStatus status) => switch (status) {
  TrayStatus.needsAttention => 0,
  TrayStatus.active => 1,
  TrayStatus.passive => 2,
};

TrayIconPixmap? decodeTrayPixmap(DBusValue? value) {
  if (value == null || value.signature != DBusSignature('a(iiay)')) {
    return null;
  }
  const preferred = 24;
  const maxInput = 512;
  const maxOutput = 64;
  _PixmapCandidate? best;
  for (final entry in value.asArray().take(32)) {
    try {
      final tuple = entry.asStruct();
      if (tuple.length != 3 || tuple[2].signature != DBusSignature('ay')) {
        continue;
      }
      final width = tuple[0].asInt32();
      final height = tuple[1].asInt32();
      final byteCount = width * height * 4;
      if (width <= 0 ||
          height <= 0 ||
          width > maxInput ||
          height > maxInput ||
          byteCount > maxInput * maxInput * 4) {
        continue;
      }
      final rawBytes = tuple[2].asByteArray();
      if (rawBytes.length != byteCount) continue;
      final extent = width > height ? width : height;
      final delta = extent - preferred;
      final score = delta >= 0 ? delta : -delta + maxInput;
      if (best == null || score < best.score) {
        best = _PixmapCandidate(width, height, rawBytes.toList(), score);
      }
    } on Object {
      continue;
    }
  }
  return best?.decode(maxOutput);
}

class _PixmapCandidate {
  const _PixmapCandidate(this.width, this.height, this.bytes, this.score);
  final int width;
  final int height;
  final List<int> bytes;
  final int score;

  TrayIconPixmap decode(int maxOutput) {
    final longest = width > height ? width : height;
    final scale = longest <= maxOutput ? 1.0 : maxOutput / longest;
    final outputWidth = (width * scale).round().clamp(1, maxOutput);
    final outputHeight = (height * scale).round().clamp(1, maxOutput);
    final rgba = Uint8List(outputWidth * outputHeight * 4);
    for (var y = 0; y < outputHeight; y++) {
      final sourceY = y * height ~/ outputHeight;
      for (var x = 0; x < outputWidth; x++) {
        final sourceX = x * width ~/ outputWidth;
        final source = (sourceY * width + sourceX) * 4;
        final dest = (y * outputWidth + x) * 4;
        final alpha = bytes[source];
        rgba[dest] = _premultiply(bytes[source + 1], alpha);
        rgba[dest + 1] = _premultiply(bytes[source + 2], alpha);
        rgba[dest + 2] = _premultiply(bytes[source + 3], alpha);
        rgba[dest + 3] = alpha;
      }
    }
    return TrayIconPixmap(width: outputWidth, height: outputHeight, rgba: rgba);
  }
}

int _premultiply(int channel, int alpha) => (channel * alpha + 127) ~/ 255;

TrayMenuEntry? parseTrayMenu(DBusValue value, {int maxEntries = 256}) =>
    _parseTrayMenu(value, budget: _MenuBudget(maxEntries), depth: 0);

TrayMenuEntry? _parseTrayMenu(
  DBusValue value, {
  required _MenuBudget budget,
  int depth = 0,
}) {
  if (budget.remaining <= 0 || depth > 5) return null;
  budget.remaining -= 1;
  try {
    final fields = value.asStruct();
    if (fields.length != 3) return null;
    final id = fields[0].asInt32();
    final properties = fields[1].asStringVariantDict();
    final children = <TrayMenuEntry>[];
    if (depth < 5) {
      for (final child in fields[2].asArray()) {
        if (budget.remaining <= 0) break;
        final parsed = _parseTrayMenu(
          child.asVariant(),
          budget: budget,
          depth: depth + 1,
        );
        if (parsed != null) children.add(parsed);
      }
    }
    final type = _string(properties['type']);
    final toggleType = switch (_string(properties['toggle-type'])) {
      'checkmark' => TrayMenuToggleType.checkmark,
      'radio' => TrayMenuToggleType.radio,
      _ => TrayMenuToggleType.none,
    };
    return TrayMenuEntry(
      id: id,
      label: _menuLabel(_bounded(_string(properties['label']), 512)),
      enabled: properties.containsKey('enabled')
          ? _boolean(properties['enabled'])
          : true,
      visible: properties.containsKey('visible')
          ? _boolean(properties['visible'])
          : true,
      separator: type == 'separator',
      toggleType: toggleType,
      toggleState: _int32(properties['toggle-state']),
      destructive: _string(properties['disposition']) == 'warning',
      children: List.unmodifiable(children),
    );
  } on Object {
    return null;
  }
}

String _menuLabel(String value) {
  final output = StringBuffer();
  for (var i = 0; i < value.length; i++) {
    final character = value[i];
    if (character != '_') {
      output.write(character);
      continue;
    }
    if (i + 1 < value.length && value[i + 1] == '_') {
      output.write('_');
      i++;
    }
  }
  return output.toString();
}
