// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:io';
import 'package:dbus/dbus.dart';
import '../models.dart';

const _interface = 'org.freedesktop.Notifications';

/// Owns the standard notification name only if it is free. A preview shell
/// must never replace the notification daemon in the user's running desktop.
class NotificationService extends DBusObject {
  NotificationService({DBusClient? bus, this.historyLimit = 100})
    : _bus = bus,
      super(DBusObjectPath('/org/freedesktop/Notifications')) {
    if (historyLimit < 1) {
      throw RangeError.range(historyLimit, 1, null, 'historyLimit');
    }
  }
  DBusClient? _bus;
  final int historyLimit;
  final _changes = StreamController<void>.broadcast();
  final _history = <int, ShellNotification>{};
  final _unread = <int>{};
  bool _historyVisible = false;
  final _active = <int>{};
  final _senders = <int, String?>{};
  final _expiry = <int, Timer>{};
  final _invoking = <ShellNotification>{};
  final _localActions = <int, Map<String, Future<void> Function()>>{};
  List<ShellNotification>? _snapshot;
  Timer? _changeTimer;
  Future<bool>? _startWork;
  Future<void> _signalWork = Future.value();
  var _nextId = 1;
  bool available = false;
  bool _disposed = false;
  List<ShellNotification> get notifications =>
      _snapshot ??= List.unmodifiable(_history.values.toList().reversed);
  Stream<void> get changes => _changes.stream;
  int get unreadCount => _unread.length;

  /// Reading history does not dismiss live notifications or their actions.
  void markRead() {
    if (_disposed || _unread.isEmpty) return;
    _unread.clear();
    _changed();
  }

  void setHistoryVisible(bool visible) {
    if (_disposed) return;
    _historyVisible = visible;
    if (visible) markRead();
  }

  /// Only the compositor's trusted completion event creates these callbacks.
  /// D-Bus senders cannot supply a command, file action, or local callback.
  void showScreenshot(
    String? path, {
    required Future<void> Function(String) openLocation,
  }) {
    if (_disposed) return;
    final file = path != null && File(path).isAbsolute ? File(path) : null;
    final id = _allocateId();
    final actions = <String, Future<void> Function()>{};
    if (file != null) {
      actions['default'] = () => openLocation(file.path);
      actions['show-folder'] = () => openLocation(file.parent.path);
    }
    final item = ShellNotification(
      id: id,
      appName: '截图',
      summary: file == null ? '截图已复制' : '截图已保存',
      body: file == null ? '已复制到剪贴板' : '已保存到 ${file.path}',
      timestamp: DateTime.now(),
      actions: file == null
          ? const []
          : const [
              NotificationAction('default', '打开图片'),
              NotificationAction('show-folder', '打开所在文件夹'),
            ],
    );
    _history[id] = item;
    _localActions[id] = actions;
    _active.add(id);
    if (!_historyVisible) _unread.add(id);
    while (_history.length > historyLimit) {
      _background(_close(_history.values.first, 4, removeHistory: true));
    }
    _changed();
  }

  Future<bool> start() {
    if (_disposed) return Future.value(false);
    if (available) return Future.value(true);
    return _startWork ??= _start();
  }

  Future<bool> _start() async {
    final bus = _bus ??= DBusClient.session();
    try {
      await bus.registerObject(this);
      final result = await bus.requestName(
        _interface,
        flags: {DBusRequestNameFlag.doNotQueue},
      );
      if (_disposed) {
        await bus.close();
        return false;
      }
      available =
          result == DBusRequestNameReply.primaryOwner ||
          result == DBusRequestNameReply.alreadyOwner;
      if (!available) await bus.close();
      return available;
    } on Object {
      await bus.close();
      return false;
    }
  }

  @override
  List<DBusIntrospectInterface> introspect() {
    DBusIntrospectArgument arg(String type, bool output) =>
        DBusIntrospectArgument(
          DBusSignature(type),
          output ? DBusArgumentDirection.out : DBusArgumentDirection.in_,
        );
    return [
      DBusIntrospectInterface(
        _interface,
        methods: [
          DBusIntrospectMethod('GetCapabilities', args: [arg('as', true)]),
          DBusIntrospectMethod(
            'GetServerInformation',
            args: List.generate(4, (_) => arg('s', true)),
          ),
          DBusIntrospectMethod(
            'Notify',
            args: [
              ...[
                's',
                'u',
                's',
                's',
                's',
                'as',
                'a{sv}',
                'i',
              ].map((s) => arg(s, false)),
              arg('u', true),
            ],
          ),
          DBusIntrospectMethod('CloseNotification', args: [arg('u', false)]),
        ],
        signals: [
          DBusIntrospectSignal(
            'NotificationClosed',
            args: [arg('u', true), arg('u', true)],
          ),
          DBusIntrospectSignal(
            'ActionInvoked',
            args: [arg('u', true), arg('s', true)],
          ),
        ],
      ),
    ];
  }

  @override
  Future<DBusMethodResponse> handleMethodCall(DBusMethodCall methodCall) async {
    if (_disposed) return DBusMethodErrorResponse.failed();
    final call = methodCall;
    if (call.interface != _interface) {
      return DBusMethodErrorResponse.unknownInterface();
    }
    switch (call.name) {
      case 'GetCapabilities':
        if (call.signature != DBusSignature('')) {
          return DBusMethodErrorResponse.invalidArgs();
        }
        return DBusMethodSuccessResponse([
          DBusArray.string([
            'body',
            'actions',
            'persistence',
            'x-clarion-screenshot-events',
          ]),
        ]);
      case 'GetServerInformation':
        if (call.signature != DBusSignature('')) {
          return DBusMethodErrorResponse.invalidArgs();
        }
        return DBusMethodSuccessResponse(
          [
            'Clarion Shell',
            'Clarion',
            '0.1.0',
            '1.2',
          ].map(DBusString.new).toList(),
        );
      case 'Notify':
        if (call.signature != DBusSignature('susssasa{sv}i')) {
          return DBusMethodErrorResponse.invalidArgs();
        }
        final v = call.values;
        final replacement = v[1].asUint32();
        final id =
            replacement != 0 &&
                _active.contains(replacement) &&
                !_localActions.containsKey(replacement)
            ? replacement
            : _allocateId();
        final actionValues = v[5].asStringArray().take(32).toList();
        final actions = <NotificationAction>[];
        final actionKeys = <String>{};
        for (
          var i = 0;
          i + 1 < actionValues.length && actions.length < 16;
          i += 2
        ) {
          // Ignore unusable or unreasonably large identifiers rather than
          // retaining unbounded client data or silently changing their meaning.
          if (actionValues[i].isEmpty ||
              actionValues[i].length > 4096 ||
              !actionKeys.add(actionValues[i])) {
            continue;
          }
          actions.add(
            NotificationAction(
              // This is an opaque client identifier, not display text. Returning
              // a truncated key would invoke a different (usually missing) action.
              actionValues[i],
              _limited(actionValues[i + 1], 256),
            ),
          );
        }
        final hints = v[6].asStringVariantDict();
        final urgency = hints['urgency'] is DBusByte
            ? hints['urgency']!.asByte().clamp(0, 2)
            : 1;
        _expiry.remove(id)?.cancel();
        _history.remove(id);
        final item = ShellNotification(
          id: id,
          appName: _limited(v[0].asString(), 256),
          summary: _limited(v[3].asString(), 1024),
          body: _limited(v[4].asString(), 8192),
          timestamp: DateTime.now(),
          actions: List.unmodifiable(actions),
          urgency: urgency,
          resident:
              hints['resident'] is DBusBoolean &&
              hints['resident']!.asBoolean(),
          transient:
              hints['transient'] is DBusBoolean &&
              hints['transient']!.asBoolean(),
        );
        _history[id] = item;
        if (_historyVisible) {
          _unread.remove(id);
        } else {
          _unread.add(id);
        }
        _active.add(id);
        _senders[id] = call.sender;
        while (_history.length > historyLimit) {
          // Capacity eviction is not a user dismissal. Remove synchronously so
          // a burst cannot exceed the bound while signal delivery is pending.
          _background(_close(_history.values.first, 4, removeHistory: true));
        }
        final timeout = v[7].asInt32();
        // A resident item has no server-default expiry, but honor a sender's
        // explicit positive deadline. Actions alone never remove it.
        if (urgency != 2 && (timeout > 0 || (timeout < 0 && !item.resident))) {
          _expiry[id] = Timer(
            Duration(milliseconds: timeout < 0 ? 8000 : timeout),
            () {
              _background(_close(item, 1));
            },
          );
        }
        _changed();
        return DBusMethodSuccessResponse([DBusUint32(id)]);
      case 'CloseNotification':
        if (call.signature != DBusSignature('u')) {
          return DBusMethodErrorResponse.invalidArgs();
        }
        final id = call.values.first.asUint32();
        final item = _history[id];
        if (item == null ||
            !_active.contains(id) ||
            _localActions.containsKey(id)) {
          return DBusMethodErrorResponse.invalidArgs();
        }
        await _close(item, 3, removeHistory: true);
        return DBusMethodSuccessResponse();
      default:
        return DBusMethodErrorResponse.unknownMethod();
    }
  }

  int _allocateId() {
    while (_history.containsKey(_nextId)) {
      _nextId = _nextId % 0xffffffff + 1;
    }
    final id = _nextId;
    _nextId = _nextId % 0xffffffff + 1;
    return id;
  }

  void _changed() {
    _snapshot = null;
    if (_disposed || _changeTimer != null) return;
    // One immutable snapshot and shell update per frame, even when several
    // clients send notifications or a user clears an entire group at once.
    _changeTimer = Timer(const Duration(milliseconds: 16), () {
      _changeTimer = null;
      if (!_disposed) _changes.add(null);
    });
  }

  Future<bool> _close(
    ShellNotification item,
    int reason, {
    bool removeHistory = false,
  }) async {
    final id = item.id;
    if (_disposed || !identical(_history[id], item)) return false;
    _expiry.remove(id)?.cancel();
    final wasActive = _active.remove(id);
    final sender = _senders.remove(id);
    final local = _localActions.remove(id) != null;
    // Invalidate before awaiting I/O: a replacement during signal delivery must
    // never have its timer, actions or history changed by the previous item.
    if (removeHistory || item.transient) {
      _history.remove(id);
      _unread.remove(id);
    } else {
      _history[id] = ShellNotification(
        id: item.id,
        appName: item.appName,
        summary: item.summary,
        body: item.body,
        timestamp: item.timestamp,
        urgency: item.urgency,
        resident: item.resident,
        transient: item.transient,
        active: false,
      );
    }
    _changed();
    if (wasActive && !local) {
      await _signal(sender, 'NotificationClosed', [
        DBusUint32(id),
        DBusUint32(reason),
      ]);
    }
    return true;
  }

  Future<bool> dismiss(int id, {ShellNotification? expected}) async {
    final item = _history[id];
    if (item == null || (expected != null && !identical(item, expected))) {
      return false;
    }
    return _close(item, 2, removeHistory: true);
  }

  Future<bool> invokeAction(
    int id,
    String key, {
    ShellNotification? expected,
  }) async {
    final item = _history[id];
    if (_disposed ||
        !_active.contains(id) ||
        item == null ||
        (expected != null && !identical(item, expected)) ||
        _invoking.contains(item) ||
        !item.actions.any((action) => action.key == key)) {
      return false;
    }
    _invoking.add(item);
    try {
      final local = _localActions[id];
      if (local != null) {
        final action = local[key];
        if (action == null) return false;
        await action();
      } else {
        await _signal(_senders[id], 'ActionInvoked', [
          DBusUint32(id),
          DBusString(key),
        ]);
      }
      if (_disposed) return false;
      if (!item.resident) {
        await _close(item, 2, removeHistory: true);
      }
      return true;
    } finally {
      _invoking.remove(item);
    }
  }

  Future<bool> clear() async {
    return dismissAll(_history.values.toList());
  }

  Future<bool> dismissAll(Iterable<ShellNotification> expected) async {
    if (_disposed) return false;
    // Snapshot identities and invalidate every old item before any I/O yields.
    // Notifications arriving during delivery belong to the next batch.
    final closes = expected
        .toSet()
        .toList()
        .map((item) => _close(item, 2, removeHistory: true))
        .toList();
    final results = await Future.wait(closes);
    return results.every((closed) => closed);
  }

  Future<void> _signal(String? sender, String name, List<DBusValue> values) {
    // Capture recipient and payload at the event, and serialize delivery so a
    // racing Close/expiry cannot overtake an ActionInvoked already in flight.
    // State changes never wait for this queue; IDs are invalidated immediately.
    final work = _signalWork.then((_) async {
      if (_disposed) return;
      await _bus?.emitSignal(
        destination: sender,
        path: path,
        interface: _interface,
        name: name,
        values: values,
      );
    });
    _signalWork = work.then<void>((_) {}, onError: (Object _, StackTrace _) {});
    return work;
  }

  void _background(Future<bool> work) {
    // Expiry/eviction still invalidate the item when its client or bus has gone
    // away; a failed best-effort signal must not become an unhandled async error.
    unawaited(work.then<void>((_) {}, onError: (Object _, StackTrace _) {}));
  }

  void dispose() {
    if (_disposed) return;
    _disposed = true;
    available = false;
    _changeTimer?.cancel();
    _changeTimer = null;
    for (final timer in _expiry.values) {
      timer.cancel();
    }
    _expiry.clear();
    _unread.clear();
    _localActions.clear();
    unawaited(_changes.close());
    final bus = _bus;
    if (bus != null) unawaited(bus.close());
  }
}

String _limited(String value, int maximum) =>
    value.length <= maximum ? value : value.substring(0, maximum);
