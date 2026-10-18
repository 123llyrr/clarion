// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:dbus/dbus.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/services/tray_service.dart';

void main() {
  test(
    'real private dbus-daemon retains initial owner registrations without sleeps',
    () async {
      final directory = await Directory.systemTemp.createTemp(
        'clarion-real-tray-bus-',
      );
      final daemon = await Process.start('/usr/bin/dbus-daemon', [
        '--session',
        '--nofork',
        '--nopidfile',
        '--print-address=1',
        '--address=unix:path=${directory.path}/bus',
      ]);
      final errors = daemon.stderr.drain<void>();
      final clients = <DBusClient>[];
      TrayService? service;
      try {
        final addressText = await daemon.stdout
            .transform(utf8.decoder)
            .transform(const LineSplitter())
            .first
            .timeout(const Duration(seconds: 3));
        final address = DBusAddress(addressText);
        service = TrayService(bus: DBusClient(address));
        expect(await service.start(), isTrue);
        for (var index = 0; index < 8; index++) {
          final client = DBusClient(address);
          clients.add(client);
          final item = _ItemObject(
            interface: index.isEven
                ? kdeItemInterface
                : freedesktopItemInterface,
          )..title = '原生托盘$index';
          final name = 'org.example.ClarionRealTray$index';
          await client.requestName(name);
          await client.registerObject(item);
          final watcher = DBusRemoteObject(
            client,
            name: kdeWatcherName,
            path: DBusObjectPath(watcherPath),
          );
          // Real daemon owner-acquisition signals can arrive at the host while
          // this registration's GetNameOwner response is being awaited.
          await watcher.callMethod(
            kdeWatcherInterface,
            'RegisterStatusNotifierItem',
            [DBusString('$name/StatusNotifierItem')],
            replySignature: DBusSignature(''),
          );
          final registered = await watcher.getProperty(
            kdeWatcherInterface,
            'RegisteredStatusNotifierItems',
          );
          expect(registered.asStringArray(), hasLength(index + 1));
        }
        await _waitFor(() => service!.items.length == 8);
        expect(
          service.items.map((item) => item.title),
          containsAll([for (var index = 0; index < 8; index++) '原生托盘$index']),
        );
      } finally {
        service?.dispose();
        for (final client in clients) {
          await client.close();
        }
        daemon.kill(ProcessSignal.sigterm);
        await daemon.exitCode.timeout(
          const Duration(seconds: 3),
          onTimeout: () {
            daemon.kill(ProcessSignal.sigkill);
            return -1;
          },
        );
        await errors;
        await directory.delete(recursive: true);
      }
    },
  );

  test('pixmap decoder prefers ~24 px ARGB and premultiplies into RGBA', () {
    DBusValue pixmap(int width, int height, List<int> argb) => DBusArray(
      DBusSignature('(iiay)'),
      [
        DBusStruct([DBusInt32(width), DBusInt32(height), DBusArray.byte(argb)]),
      ],
    );
    expect(decodeTrayPixmap(const DBusString('no')), isNull);
    expect(decodeTrayPixmap(pixmap(1, 1, const [128, 10, 20, 30]))!.rgba, [
      5,
      10,
      15,
      128,
    ]);
    final chosen = decodeTrayPixmap(
      DBusArray(DBusSignature('(iiay)'), [
        DBusStruct([
          const DBusInt32(8),
          const DBusInt32(8),
          DBusArray.byte(List<int>.filled(8 * 8 * 4, 255)),
        ]),
        DBusStruct([
          const DBusInt32(24),
          const DBusInt32(24),
          DBusArray.byte([
            255,
            1,
            2,
            3,
            ...List<int>.filled((24 * 24 - 1) * 4, 0),
          ]),
        ]),
        DBusStruct([
          const DBusInt32(48),
          const DBusInt32(48),
          DBusArray.byte(List<int>.filled(48 * 48 * 4, 7)),
        ]),
      ]),
    )!;
    expect(chosen.width, 24);
    expect(chosen.rgba.take(4), [1, 2, 3, 255]);
    expect(decodeTrayPixmap(pixmap(0, 1, const [0, 0, 0, 0])), isNull);
    expect(decodeTrayPixmap(pixmap(2, 1, const [0, 0, 0, 0])), isNull);
  });

  test('menu parser strips mnemonics and bounds nested entries', () {
    DBusValue entry({
      required int id,
      String label = '',
      String type = '',
      String toggle = '',
      int state = 0,
      bool enabled = true,
      bool visible = true,
      List<DBusValue> children = const [],
    }) => DBusStruct([
      DBusInt32(id),
      DBusDict.stringVariant({
        'label': DBusString(label),
        'type': DBusString(type),
        'toggle-type': DBusString(toggle),
        'toggle-state': DBusInt32(state),
        'enabled': DBusBoolean(enabled),
        'visible': DBusBoolean(visible),
      }),
      DBusArray.variant(children),
    ]);
    final parsed = parseTrayMenu(
      entry(
        id: 0,
        children: [
          entry(id: 1, label: '_显示窗口'),
          entry(id: 2, type: 'separator'),
          entry(
            id: 3,
            label: '状态',
            children: [entry(id: 4, label: '在线', toggle: 'radio', state: 1)],
          ),
          entry(id: 5, label: 'E_xit', visible: false),
          entry(id: 6, label: 'a__b'),
        ],
      ),
    )!;
    expect(parsed.children.map((child) => child.id), [1, 2, 3, 5, 6]);
    expect(parsed.children[0].label, '显示窗口');
    expect(parsed.children[1].separator, isTrue);
    expect(
      parsed.children[2].children.single.toggleType,
      TrayMenuToggleType.radio,
    );
    expect(parsed.children[2].children.single.toggleState, 1);
    expect(parsed.children[3].visible, isFalse);
    expect(parsed.children[4].label, 'a_b');
    final nested = <DBusValue>[];
    DBusValue current = entry(id: 99, label: 'leaf');
    for (var depth = 6; depth >= 1; depth--) {
      current = entry(id: depth, label: '$depth', children: [current]);
    }
    nested.add(current);
    final bounded = parseTrayMenu(entry(id: 0, children: nested))!;
    expect(_menuDepth(bounded), lessThanOrEqualTo(6));
    final budgeted = parseTrayMenu(
      entry(
        id: 0,
        children: [
          entry(id: 1, children: [entry(id: 2), entry(id: 3)]),
          entry(id: 4),
        ],
      ),
      maxEntries: 3,
    )!;
    expect(_menuCount(budgeted), 3);
  });

  group('StatusNotifier on isolated D-Bus', () {
    late Directory directory;
    late DBusServer bus;
    late DBusAddress address;
    late DBusClient caller;
    setUp(() async {
      directory = await Directory.systemTemp.createTemp('clarion-tray-test-');
      bus = DBusServer();
      address = await bus.listenAddress(
        DBusAddress.unix(path: '${directory.path}/bus'),
      );
      caller = DBusClient(address);
    });
    tearDown(() async {
      await caller.close();
      await bus.close();
      await directory.delete(recursive: true);
    });

    test('does not replace an existing watcher owner', () async {
      final owner = DBusClient(address);
      addTearDown(owner.close);
      expect(
        await owner.requestName(
          kdeWatcherName,
          flags: {DBusRequestNameFlag.allowReplacement},
        ),
        DBusRequestNameReply.primaryOwner,
      );
      final service = TrayService(bus: DBusClient(address));
      addTearDown(service.dispose);
      expect(await service.start(), isFalse);
      expect(await caller.getNameOwner(kdeWatcherName), owner.uniqueName);
      await owner.close();
      await _waitFor(() => service.available);
      expect(
        await caller.getNameOwner(kdeWatcherName),
        isNot(owner.uniqueName),
      );
    });

    test(
      'hosts items, menus and activation without replacing later watchers',
      () async {
        final service = TrayService(bus: DBusClient(address));
        addTearDown(service.dispose);
        expect(await service.start(), isTrue);
        expect(await caller.getNameOwner(kdeWatcherName), isNotNull);
        final item = _ItemObject();
        expect(
          await caller.requestName('org.example.Downloader'),
          DBusRequestNameReply.primaryOwner,
        );
        await caller.registerObject(item);
        await caller.registerObject(item.menu);
        final watcher = DBusRemoteObject(
          caller,
          name: kdeWatcherName,
          path: DBusObjectPath(watcherPath),
        );
        final ready = service.changes.first;
        await watcher.callMethod(
          kdeWatcherInterface,
          'RegisterStatusNotifierItem',
          [const DBusString('org.example.Downloader/StatusNotifierItem')],
          replySignature: DBusSignature(''),
        );
        await ready.timeout(const Duration(seconds: 2));
        expect(service.items, isNotEmpty);
        final hosted = service.items.single;
        expect(hosted.title, '下载器');
        expect(hosted.status, TrayStatus.needsAttention);
        expect(hosted.iconPixmap?.rgba.take(4), [1, 2, 3, 255]);
        expect(hosted.menuAvailable, isTrue);
        expect(
          await service.invoke(hosted, TrayAction.activate, x: 12, y: 8),
          isTrue,
        );
        expect(item.activated, [(12, 8)]);
        expect(await service.scroll(hosted, 120), isTrue);
        expect(
          await service.scroll(hosted, -80, orientation: 'horizontal'),
          isTrue,
        );
        expect(await service.scroll(hosted, 0), isFalse);
        expect(
          await service.scroll(hosted, 1, orientation: 'diagonal'),
          isFalse,
        );
        expect(item.scrolled, [(120, 'vertical'), (-80, 'horizontal')]);
        final menu = await service.loadMenu(hosted);
        expect(menu!.map((entry) => entry.label), ['显示窗口', '退出']);
        expect(await service.activateMenuEntry(hosted, 2), isTrue);
        expect(item.menu.clicked, [2]);
        final menuChanged = service.menuChanges.first;
        await item.menu.emitSignal(menuInterface, 'LayoutUpdated', const [
          DBusUint32(2),
          DBusInt32(0),
        ]);
        expect(
          await menuChanged.timeout(const Duration(seconds: 2)),
          hosted.id,
        );
        final later = DBusClient(address);
        addTearDown(later.close);
        expect(
          await later.requestName(
            kdeWatcherName,
            flags: {DBusRequestNameFlag.doNotQueue},
          ),
          DBusRequestNameReply.exists,
        );
      },
    );

    test(
      'joins freedesktop-only watcher and re-registers on replacement',
      () async {
        final old = DBusClient(address);
        final replacement = DBusClient(address);
        addTearDown(old.close);
        addTearDown(replacement.close);
        final oldWatcher = _ExternalWatcher(freedesktopWatcherInterface);
        final nextWatcher = _ExternalWatcher(freedesktopWatcherInterface);
        await old.registerObject(oldWatcher);
        await replacement.registerObject(nextWatcher);
        await old.requestName(
          freedesktopWatcherName,
          flags: {DBusRequestNameFlag.allowReplacement},
        );
        final item = _ItemObject(interface: freedesktopItemInterface);
        await caller.requestName('org.example.Downloader');
        await caller.registerObject(item);
        oldWatcher.addresses = ['org.example.Downloader'];
        nextWatcher.addresses = ['org.example.Downloader'];
        final service = TrayService(bus: DBusClient(address));
        addTearDown(service.dispose);
        expect(await service.start(), isTrue);
        await _waitFor(() => service.items.length == 1);
        expect(await caller.getNameOwner(kdeWatcherName), isNull);
        expect(oldWatcher.hosts, hasLength(1));
        expect(
          await service.invoke(service.items.single, TrayAction.activate),
          isTrue,
        );
        // A watcher may canonicalize an address after discovering its owner.
        // Changing only this alias must retain the existing owner/path item.
        final retainedId = service.items.single.id;
        oldWatcher.addresses = ['${caller.uniqueName}/StatusNotifierItem'];
        await oldWatcher.emitSignal(
          freedesktopWatcherInterface,
          'StatusNotifierItemRegistered',
          [DBusString(oldWatcher.addresses.single)],
        );
        await _waitFor(() => oldWatcher.propertyReads >= 2);
        item.title = '别名保留';
        await item.emitSignal(freedesktopItemInterface, 'NewTitle');
        await _waitFor(
          () =>
              service.items.length == 1 && service.items.single.title == '别名保留',
        );
        expect(service.items.single.id, retainedId);
        await replacement.requestName(
          freedesktopWatcherName,
          flags: {
            DBusRequestNameFlag.replaceExisting,
            DBusRequestNameFlag.doNotQueue,
          },
        );
        await _waitFor(
          () => nextWatcher.hosts.isNotEmpty && service.items.length == 1,
        );
        expect(service.available, isTrue);
        expect(await caller.getNameOwner(kdeWatcherName), isNull);
        expect(
          await caller.getNameOwner(freedesktopWatcherName),
          replacement.uniqueName,
        );
      },
    );

    test(
      'concurrent aliases register one owner/path and exit removes it',
      () async {
        final item = _ItemObject();
        await caller.requestName('org.example.Downloader');
        await caller.registerObject(item);
        final service = TrayService(bus: DBusClient(address));
        addTearDown(service.dispose);
        expect(await Future.wait([service.start(), service.start()]), [
          true,
          true,
        ]);
        await Future.wait([
          for (var i = 0; i < 16; i++)
            service.registerItem(
              i.isEven
                  ? 'org.example.Downloader'
                  : '${caller.uniqueName}/StatusNotifierItem',
              null,
            ),
        ]);
        await _waitFor(() => service.items.length == 1);
        final watcher = DBusRemoteObject(
          caller,
          name: kdeWatcherName,
          path: DBusObjectPath(watcherPath),
        );
        expect(
          (await watcher.getProperty(
            kdeWatcherInterface,
            'RegisteredStatusNotifierItems',
          )).asStringArray(),
          hasLength(1),
        );
        await caller.close();
        await _waitFor(() => service.items.isEmpty);
      },
    );

    test(
      'well-known replacement cannot resurrect a delayed old item read',
      () async {
        final old = DBusClient(address);
        final next = DBusClient(address);
        addTearDown(old.close);
        addTearDown(next.close);
        final oldItem = _ItemObject();
        final started = Completer<void>();
        final release = Completer<void>();
        oldItem.readStarted = started;
        oldItem.readGate = release.future;
        await old.registerObject(oldItem);
        await old.requestName(
          'org.example.Replaceable',
          flags: {DBusRequestNameFlag.allowReplacement},
        );
        final nextItem = _ItemObject()..title = '新应用';
        await next.registerObject(nextItem);
        final service = TrayService(bus: DBusClient(address));
        addTearDown(service.dispose);
        expect(await service.start(), isTrue);
        await service.registerItem('org.example.Replaceable', null);
        await started.future.timeout(const Duration(seconds: 2));
        await next.requestName(
          'org.example.Replaceable',
          flags: {
            DBusRequestNameFlag.replaceExisting,
            DBusRequestNameFlag.doNotQueue,
          },
        );
        // Releasing the stale request after the bus owner transition must not
        // install the old owner's title under the new application's identity.
        final watcher = DBusRemoteObject(
          caller,
          name: kdeWatcherName,
          path: DBusObjectPath(watcherPath),
        );
        await _waitForAsync(
          () async => (await watcher.getProperty(
            kdeWatcherInterface,
            'RegisteredStatusNotifierItems',
          )).asStringArray().isEmpty,
        );
        await service.registerItem('org.example.Replaceable', null);
        await _waitFor(
          () =>
              service.items.length == 1 && service.items.single.title == '新应用',
        );
        // The old owner's GetAll is still blocked. Its replacement must not
        // wait for that flight or be cleared when the stale reply completes.
        expect(release.isCompleted, isFalse);
        release.complete();
        expect(oldItem.activated, isEmpty);
        expect(
          await service.invoke(service.items.single, TrayAction.activate),
          isTrue,
        );
        expect(nextItem.activated, hasLength(1));
      },
    );

    test('a slow item does not block a later item or its updates', () async {
      final slowClient = DBusClient(address);
      addTearDown(slowClient.close);
      final started = Completer<void>();
      final release = Completer<void>();
      final slow = _ItemObject()
        ..readStarted = started
        ..readGate = release.future;
      final fast = _ItemObject()..title = '快速应用';
      await slowClient.requestName('org.example.Slow');
      await slowClient.registerObject(slow);
      await caller.requestName('org.example.Fast');
      await caller.registerObject(fast);
      final service = TrayService(bus: DBusClient(address));
      addTearDown(service.dispose);
      try {
        expect(await service.start(), isTrue);
        await service.registerItem('org.example.Slow', null);
        await started.future.timeout(const Duration(seconds: 1));
        await service.registerItem('org.example.Fast', null);
        await _waitFor(
          () => service.items.any((item) => item.title == '快速应用'),
        ).timeout(const Duration(seconds: 1));
        fast.title = '快速更新';
        await fast.emitSignal(kdeItemInterface, 'NewTitle');
        await _waitFor(
          () => service.items.any((item) => item.title == '快速更新'),
        ).timeout(const Duration(seconds: 1));
        expect(release.isCompleted, isFalse);
        expect(slow.fullReads, 1);
      } finally {
        if (!release.isCompleted) release.complete();
      }
    });

    test(
      'property refresh is bounded parallel and dirty signals coalesce',
      () async {
        final item = _ItemObject();
        await caller.requestName('org.example.Downloader');
        await caller.registerObject(item);
        final service = TrayService(bus: DBusClient(address));
        addTearDown(service.dispose);
        expect(await service.start(), isTrue);
        await service.registerItem('org.example.Downloader', null);
        await _waitFor(() => service.items.length == 1);
        final release = Completer<void>();
        item.propertyGate = release.future;
        final revision = service.items.single.iconRevision;
        try {
          await item.emitSignal(
            'org.freedesktop.DBus.Properties',
            'PropertiesChanged',
            [
              const DBusString(kdeItemInterface),
              DBusDict.stringVariant({}),
              DBusArray.string([
                'Title',
                'Status',
                'IconName',
                'AttentionIconName',
                'IconThemePath',
                'Menu',
                'ItemIsMenu',
                'AttentionIconPixmap',
              ]),
            ],
          );
          await _waitFor(
            () => item.activePropertyReads == 4,
          ).timeout(const Duration(seconds: 1));
          for (var i = 0; i < 30; i++) {
            await item.emitSignal(kdeItemInterface, 'NewIcon');
          }
          // Allow the coalescing timer to fire while the same item's flight is
          // blocked; no second flight may start behind the four property getters.
          await Future<void>.delayed(const Duration(milliseconds: 90));
          expect(item.maximumPropertyReads, 4);
          expect(item.propertyReads, 4);
          expect(item.fullReads, 1);
          item.title = '合并完成';
          release.complete();
          await _waitFor(() => service.items.single.title == '合并完成');
          expect(service.items.single.iconRevision, greaterThan(revision));
          expect(item.maximumPropertyReads, lessThanOrEqualTo(4));
          expect(item.fullReads, 2);
        } finally {
          if (!release.isCompleted) release.complete();
        }
      },
    );

    test(
      'external watcher duplicate aliases remain one owner and path',
      () async {
        final external = DBusClient(address);
        final other = DBusClient(address);
        addTearDown(external.close);
        addTearDown(other.close);
        final watcher = _ExternalWatcher(kdeWatcherInterface);
        await external.registerObject(watcher);
        await external.requestName(kdeWatcherName);
        await caller.requestName('org.example.Downloader');
        await caller.requestName('org.example.DownloaderAlias');
        final item = _ItemObject();
        await caller.registerObject(item);
        // Equal titles are legitimate for separate exporters. They must not be
        // deduplicated, or displaced by repeated watcher address strings.
        await other.registerObject(_ItemObject());
        final aliases = [
          'org.example.Downloader',
          'org.example.DownloaderAlias/StatusNotifierItem',
          '${caller.uniqueName}/StatusNotifierItem',
        ];
        final otherAddress = '${other.uniqueName}/StatusNotifierItem';
        watcher.addresses = [
          for (var i = 0; i < 40; i++) ...aliases,
          otherAddress,
        ];
        final service = TrayService(bus: DBusClient(address));
        addTearDown(service.dispose);
        expect(await service.start(), isTrue);
        await _waitFor(() => service.items.length == 2);
        final retainedId = service.items
            .firstWhere(
              (entry) =>
                  service.nativeMenuTarget(entry)?['busName'] ==
                  caller.uniqueName,
            )
            .id;
        for (var i = 0; i < 15; i++) {
          watcher.addresses = [
            for (var n = 0; n < 40; n++) ...aliases.reversed,
            otherAddress,
          ];
          await watcher.emitSignal(
            kdeWatcherInterface,
            'StatusNotifierItemRegistered',
            [DBusString(aliases[i % aliases.length])],
          );
        }
        await _waitFor(() => watcher.propertyReads >= 2);
        item.title = '仍然只有一个';
        await item.emitSignal(kdeItemInterface, 'NewTitle');
        await _waitFor(
          () => service.items.any((entry) => entry.title == '仍然只有一个'),
        );
        expect(service.items.length, 2);
        expect(
          service.items.singleWhere((entry) => entry.title == '仍然只有一个').id,
          retainedId,
        );
        expect(item.fullReads, 1);
      },
    );

    test(
      'native menu target pins a current item to its unique exporter',
      () async {
        final item = _ItemObject();
        await caller.requestName('org.example.Downloader');
        await caller.registerObject(item);
        final service = TrayService(bus: DBusClient(address));
        addTearDown(service.dispose);
        expect(await service.start(), isTrue);
        await service.registerItem('org.example.Downloader', null);
        await _waitFor(() => service.items.length == 1);
        final old = service.items.single;
        final reads = item.fullReads + item.propertyReads;
        expect(service.nativeMenuTarget(old), {
          'busName': caller.uniqueName,
          'menuPath': '/Menu',
        });
        expect(item.fullReads + item.propertyReads, reads);
        item.title = '新元数据';
        await item.emitSignal(kdeItemInterface, 'NewTitle');
        await _waitFor(() => service.items.single.title == '新元数据');
        expect(service.nativeMenuTarget(old), isNull);
        final current = service.items.single;
        expect(
          service.nativeMenuTarget(current)?['busName'],
          caller.uniqueName,
        );
        item.menuPathOverride = '/';
        await item.emitSignal(
          'org.freedesktop.DBus.Properties',
          'PropertiesChanged',
          [
            const DBusString(kdeItemInterface),
            DBusDict.stringVariant({'Menu': DBusObjectPath('/')}),
            DBusArray.string([]),
          ],
        );
        await _waitFor(() => !service.items.single.menuAvailable);
        expect(service.nativeMenuTarget(current), isNull);
        expect(service.nativeMenuTarget(service.items.single), isNull);
        await caller.close();
        await _waitFor(() => service.items.isEmpty);
        expect(service.nativeMenuTarget(old), isNull);
        expect(service.nativeMenuTarget(current), isNull);
      },
    );

    test(
      'registration tokens survive metadata updates during a slow action',
      () async {
        final item = _ItemObject();
        await caller.requestName('org.example.Downloader');
        await caller.registerObject(item);
        await caller.registerObject(item.menu);
        final service = TrayService(bus: DBusClient(address));
        addTearDown(service.dispose);
        expect(await service.start(), isTrue);
        await service.registerItem('org.example.Downloader', null);
        await _waitFor(() => service.items.length == 1);
        final old = service.items.single;
        final token = service.registrationToken(old);
        expect(token, isNotNull);
        final reads = item.fullReads + item.propertyReads;
        expect(service.registrationToken(old), same(token));
        expect(item.fullReads + item.propertyReads, reads);
        final release = Completer<void>();
        item.scrollStarted = Completer<void>();
        item.scrollGate = release.future;
        final pending = service.scroll(old, 1);
        try {
          await item.scrollStarted!.future.timeout(const Duration(seconds: 2));
          item.title = '元数据更新';
          await item.emitSignal(kdeItemInterface, 'NewTitle');
          await _waitFor(() => service.items.single.title == '元数据更新');
          final current = service.items.single;
          expect(current, isNot(same(old)));
          expect(service.registrationToken(old), same(token));
          expect(service.registrationToken(current), same(token));
          expect(service.nativeMenuTarget(old), isNull);
          expect(service.nativeMenuTarget(current), isNotNull);
          release.complete();
          expect(await pending, isTrue);
          expect(await service.scroll(old, 5), isTrue);
          expect(await service.invoke(old, TrayAction.activate), isTrue);
          expect(await service.loadMenu(old), isNotEmpty);
          expect(await service.activateMenuEntry(old, 2), isTrue);
          expect(item.scrolled, [(1, 'vertical'), (5, 'vertical')]);
          expect(item.menu.clicked, [2]);
        } finally {
          if (!release.isCompleted) release.complete();
        }
        service.dispose();
        expect(service.registrationToken(old), isNull);
      },
    );

    test(
      'owner replacement rejects old snapshots and late action completions',
      () async {
        final next = DBusClient(address);
        addTearDown(next.close);
        final oldItem = _ItemObject();
        final nextItem = _ItemObject()..title = '新应用';
        await caller.registerObject(oldItem);
        await caller.registerObject(oldItem.menu);
        await caller.requestName(
          'org.example.Replaceable',
          flags: {DBusRequestNameFlag.allowReplacement},
        );
        await next.registerObject(nextItem);
        await next.registerObject(nextItem.menu);
        final service = TrayService(bus: DBusClient(address));
        addTearDown(service.dispose);
        expect(await service.start(), isTrue);
        await service.registerItem('org.example.Replaceable', null);
        await _waitFor(() => service.items.length == 1);
        final old = service.items.single;
        final token = service.registrationToken(old);
        final release = Completer<void>();
        oldItem.activationStarted = Completer<void>();
        oldItem.activationGate = release.future;
        final pending = service.invoke(old, TrayAction.activate);
        try {
          await oldItem.activationStarted!.future.timeout(
            const Duration(seconds: 2),
          );
          await next.requestName(
            'org.example.Replaceable',
            flags: {
              DBusRequestNameFlag.replaceExisting,
              DBusRequestNameFlag.doNotQueue,
            },
          );
          await _waitFor(() => service.items.isEmpty);
          expect(service.registrationToken(old), isNull);
          await service.registerItem('org.example.Replaceable', null);
          await _waitFor(() => service.items.length == 1);
          final current = service.items.single;
          expect(current.id, old.id);
          expect(service.registrationToken(current), isNotNull);
          expect(service.registrationToken(current), isNot(same(token)));
          release.complete();
          expect(await pending, isFalse);
          expect(await service.invoke(old, TrayAction.activate), isFalse);
          expect(
            await service.invoke(old, TrayAction.secondaryActivate),
            isFalse,
          );
          expect(await service.invoke(old, TrayAction.contextMenu), isFalse);
          expect(await service.scroll(old, 1), isFalse);
          expect(await service.loadMenu(old), isNull);
          expect(await service.activateMenuEntry(old, 2), isFalse);
          expect(nextItem.activated, isEmpty);
          expect(nextItem.scrolled, isEmpty);
          expect(nextItem.menu.aboutToShowCalls, 0);
          expect(nextItem.menu.layoutReads, 0);
          expect(nextItem.menu.clicked, isEmpty);
          final forged = TrayItem(id: current.id, title: current.title);
          expect(service.registrationToken(forged), isNull);
          expect(await service.invoke(forged, TrayAction.activate), isFalse);
          expect(await service.invoke(current, TrayAction.activate), isTrue);
          expect(await service.scroll(current, 2), isTrue);
          expect(await service.loadMenu(current), isNotEmpty);
          expect(await service.activateMenuEntry(current, 2), isTrue);
          expect(nextItem.activated, hasLength(1));
          expect(nextItem.scrolled, [(2, 'vertical')]);
          expect(nextItem.menu.clicked, [2]);
          expect(oldItem.activated, hasLength(1));
        } finally {
          if (!release.isCompleted) release.complete();
        }
      },
    );

    test(
      'item signals update status and invalidated properties, menu changes route by owner',
      () async {
        final item = _ItemObject();
        await caller.requestName('org.example.Downloader');
        await caller.registerObject(item);
        await caller.registerObject(item.menu);
        final service = TrayService(bus: DBusClient(address));
        addTearDown(service.dispose);
        expect(await service.start(), isTrue);
        await service.registerItem('org.example.Downloader', null);
        await _waitFor(() => service.items.length == 1);
        final iconRevision = service.items.single.iconRevision;
        item.title = '同步完成';
        await item.emitSignal(kdeItemInterface, 'NewTitle');
        await _waitFor(() => service.items.single.title == '同步完成');
        item.status = 'Passive';
        await item.emitSignal(
          'org.freedesktop.DBus.Properties',
          'PropertiesChanged',
          [
            const DBusString(kdeItemInterface),
            DBusDict.stringVariant({'Status': const DBusString('Active')}),
            DBusArray.string(['Status']),
          ],
        );
        await _waitFor(() => service.items.single.status == TrayStatus.passive);
        expect(service.items.single.iconRevision, iconRevision);
        // The exporter can replace a file while keeping IconName unchanged.
        await item.emitSignal(kdeItemInterface, 'NewIcon');
        await _waitFor(() => service.items.single.iconRevision > iconRevision);
        var updatedIconRevision = service.items.single.iconRevision;
        await item.emitSignal(kdeItemInterface, 'NewAttentionIcon');
        await _waitFor(
          () => service.items.single.iconRevision > updatedIconRevision,
        );
        updatedIconRevision = service.items.single.iconRevision;
        await item.emitSignal(
          'org.freedesktop.DBus.Properties',
          'PropertiesChanged',
          [
            const DBusString(kdeItemInterface),
            DBusDict.stringVariant({
              'IconName': const DBusString('folder-download'),
            }),
            DBusArray.string([]),
          ],
        );
        await _waitFor(
          () => service.items.single.iconRevision > updatedIconRevision,
        );
        updatedIconRevision = service.items.single.iconRevision;
        await item.emitSignal(
          'org.freedesktop.DBus.Properties',
          'PropertiesChanged',
          [
            const DBusString(kdeItemInterface),
            DBusDict.stringVariant({}),
            DBusArray.string(['IconName']),
          ],
        );
        await _waitFor(
          () => service.items.single.iconRevision > updatedIconRevision,
        );
        final menuChange = service.menuChanges.first;
        await item.menu.emitSignal(menuInterface, 'ItemsPropertiesUpdated', [
          DBusArray(DBusSignature('(ia{sv})'), [
            DBusStruct([
              const DBusInt32(1),
              DBusDict.stringVariant({'label': const DBusString('打开')}),
            ]),
          ]),
          DBusArray(DBusSignature('(ias)'), const []),
        ]);
        expect(
          await menuChange.timeout(const Duration(seconds: 2)),
          service.items.single.id,
        );
      },
    );

    test(
      'dispose releases pixmaps and rejects a late property reply',
      () async {
        final item = _ItemObject();
        final started = Completer<void>();
        final release = Completer<void>();
        item.readStarted = started;
        item.readGate = release.future;
        await caller.requestName('org.example.Downloader');
        await caller.registerObject(item);
        final service = TrayService(bus: DBusClient(address));
        addTearDown(service.dispose);
        expect(await service.start(), isTrue);
        await service.registerItem('org.example.Downloader', null);
        await started.future.timeout(const Duration(seconds: 2));
        final closed = service.changes.drain<void>();
        service.dispose();
        release.complete();
        await closed;
        expect(service.items, isEmpty);
        expect(await service.start(), isFalse);
      },
    );
  });
}

Future<void> _waitFor(bool Function() condition) async {
  final deadline = DateTime.now().add(const Duration(seconds: 3));
  while (!condition()) {
    if (DateTime.now().isAfter(deadline)) {
      fail('Timed out waiting for tray state');
    }
    await Future<void>.delayed(const Duration(milliseconds: 5));
  }
}

Future<void> _waitForAsync(Future<bool> Function() condition) async {
  final deadline = DateTime.now().add(const Duration(seconds: 3));
  while (!await condition()) {
    if (DateTime.now().isAfter(deadline)) {
      fail('Timed out waiting for tray state');
    }
    await Future<void>.delayed(const Duration(milliseconds: 5));
  }
}

int _menuCount(TrayMenuEntry entry) =>
    1 +
    entry.children.fold<int>(0, (count, child) => count + _menuCount(child));

int _menuDepth(TrayMenuEntry entry) {
  if (entry.children.isEmpty) return 1;
  return 1 + entry.children.map(_menuDepth).reduce((a, b) => a > b ? a : b);
}

class _ItemObject extends DBusObject {
  _ItemObject({this.interface = kdeItemInterface})
    : super(DBusObjectPath('/StatusNotifierItem'));
  final String interface;
  final activated = <(int, int)>[];
  final scrolled = <(int, String)>[];
  final menu = _MenuObject();
  String title = '下载器';
  String status = 'NeedsAttention';
  Future<void>? readGate;
  Completer<void>? readStarted;
  Future<void>? propertyGate;
  Future<void>? activationGate;
  Completer<void>? activationStarted;
  Future<void>? scrollGate;
  Completer<void>? scrollStarted;
  int fullReads = 0;
  int propertyReads = 0;
  int activePropertyReads = 0;
  int maximumPropertyReads = 0;
  String? menuPathOverride;

  Map<String, DBusValue> get _properties => {
    'Id': const DBusString('downloader'),
    'Title': DBusString(title),
    'Status': DBusString(status),
    'IconName': const DBusString('folder-download'),
    'AttentionIconName': const DBusString(''),
    'IconThemePath': const DBusString(''),
    'ItemIsMenu': const DBusBoolean(false),
    'Menu': menuPathOverride == null
        ? menu.path
        : DBusObjectPath(menuPathOverride!),
    'IconPixmap': DBusArray(DBusSignature('(iiay)'), [
      DBusStruct([
        const DBusInt32(1),
        const DBusInt32(1),
        DBusArray.byte(const [255, 1, 2, 3]),
      ]),
    ]),
    'AttentionIconPixmap': DBusArray(DBusSignature('(iiay)'), const []),
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
              DBusIntrospectArgument(
                DBusSignature('i'),
                DBusArgumentDirection.in_,
              ),
              DBusIntrospectArgument(
                DBusSignature('i'),
                DBusArgumentDirection.in_,
              ),
            ],
          ),
        DBusIntrospectMethod(
          'Scroll',
          args: [
            DBusIntrospectArgument(
              DBusSignature('i'),
              DBusArgumentDirection.in_,
            ),
            DBusIntrospectArgument(
              DBusSignature('s'),
              DBusArgumentDirection.in_,
            ),
          ],
        ),
      ],
      properties: [
        for (final entry in _properties.entries)
          DBusIntrospectProperty(
            entry.key,
            entry.value.signature,
            access: DBusPropertyAccess.read,
          ),
      ],
    ),
  ];

  @override
  Future<DBusMethodResponse> handleMethodCall(DBusMethodCall methodCall) async {
    if (methodCall.interface != interface) {
      return DBusMethodErrorResponse.unknownInterface();
    }
    if (methodCall.name == 'Scroll' &&
        methodCall.signature == DBusSignature('is')) {
      scrolled.add((
        methodCall.values[0].asInt32(),
        methodCall.values[1].asString(),
      ));
      if (scrollStarted case final started? when !started.isCompleted) {
        started.complete();
      }
      await scrollGate;
      return DBusMethodSuccessResponse();
    }
    if (methodCall.signature != DBusSignature('ii')) {
      return DBusMethodErrorResponse.invalidArgs();
    }
    switch (methodCall.name) {
      case 'Activate':
      case 'SecondaryActivate':
      case 'ContextMenu':
        activated.add((
          methodCall.values[0].asInt32(),
          methodCall.values[1].asInt32(),
        ));
        if (activationStarted case final started? when !started.isCompleted) {
          started.complete();
        }
        await activationGate;
        return DBusMethodSuccessResponse();
      default:
        return DBusMethodErrorResponse.unknownMethod();
    }
  }

  @override
  Future<DBusMethodResponse> getProperty(String interface, String name) async {
    if (interface != this.interface) {
      return DBusMethodErrorResponse.unknownInterface();
    }
    final value = _properties[name];
    propertyReads++;
    activePropertyReads++;
    if (activePropertyReads > maximumPropertyReads) {
      maximumPropertyReads = activePropertyReads;
    }
    try {
      await propertyGate;
      return value == null
          ? DBusMethodErrorResponse.unknownProperty()
          : DBusGetPropertyResponse(value);
    } finally {
      activePropertyReads--;
    }
  }

  @override
  Future<DBusMethodResponse> getAllProperties(String interface) async {
    if (interface != this.interface) {
      return DBusMethodErrorResponse.unknownInterface();
    }
    fullReads++;
    final snapshot = _properties;
    final started = readStarted;
    if (started != null && !started.isCompleted) started.complete();
    await readGate;
    return DBusGetAllPropertiesResponse(snapshot);
  }
}

class _MenuObject extends DBusObject {
  _MenuObject() : super(DBusObjectPath('/Menu'));
  final clicked = <int>[];
  int aboutToShowCalls = 0;
  int layoutReads = 0;

  DBusValue get _layout => DBusStruct([
    const DBusInt32(0),
    DBusDict.stringVariant(const {}),
    DBusArray.variant([
      DBusStruct([
        const DBusInt32(1),
        DBusDict.stringVariant({'label': const DBusString('显示窗口')}),
        DBusArray.variant(const []),
      ]),
      DBusStruct([
        const DBusInt32(2),
        DBusDict.stringVariant({'label': const DBusString('退出')}),
        DBusArray.variant(const []),
      ]),
    ]),
  ]);

  @override
  List<DBusIntrospectInterface> introspect() => [
    DBusIntrospectInterface(
      menuInterface,
      methods: [
        DBusIntrospectMethod(
          'AboutToShow',
          args: [
            DBusIntrospectArgument(
              DBusSignature('i'),
              DBusArgumentDirection.in_,
            ),
            DBusIntrospectArgument(
              DBusSignature('b'),
              DBusArgumentDirection.out,
            ),
          ],
        ),
        DBusIntrospectMethod(
          'GetLayout',
          args: [
            DBusIntrospectArgument(
              DBusSignature('i'),
              DBusArgumentDirection.in_,
            ),
            DBusIntrospectArgument(
              DBusSignature('i'),
              DBusArgumentDirection.in_,
            ),
            DBusIntrospectArgument(
              DBusSignature('as'),
              DBusArgumentDirection.in_,
            ),
            DBusIntrospectArgument(
              DBusSignature('u'),
              DBusArgumentDirection.out,
            ),
            DBusIntrospectArgument(
              DBusSignature('(ia{sv}av)'),
              DBusArgumentDirection.out,
            ),
          ],
        ),
        DBusIntrospectMethod(
          'Event',
          args: [
            DBusIntrospectArgument(
              DBusSignature('i'),
              DBusArgumentDirection.in_,
            ),
            DBusIntrospectArgument(
              DBusSignature('s'),
              DBusArgumentDirection.in_,
            ),
            DBusIntrospectArgument(
              DBusSignature('v'),
              DBusArgumentDirection.in_,
            ),
            DBusIntrospectArgument(
              DBusSignature('u'),
              DBusArgumentDirection.in_,
            ),
          ],
        ),
      ],
    ),
  ];

  @override
  Future<DBusMethodResponse> handleMethodCall(DBusMethodCall methodCall) async {
    if (methodCall.interface != menuInterface) {
      return DBusMethodErrorResponse.unknownInterface();
    }
    switch (methodCall.name) {
      case 'AboutToShow':
        aboutToShowCalls++;
        return DBusMethodSuccessResponse([const DBusBoolean(false)]);
      case 'GetLayout':
        layoutReads++;
        return DBusMethodSuccessResponse([const DBusUint32(1), _layout]);
      case 'Event':
        clicked.add(methodCall.values.first.asInt32());
        return DBusMethodSuccessResponse();
      default:
        return DBusMethodErrorResponse.unknownMethod();
    }
  }
}

class _ExternalWatcher extends DBusObject {
  _ExternalWatcher(this.interface) : super(DBusObjectPath(watcherPath));
  final String interface;
  final hosts = <String>[];
  List<String> addresses = const [];
  int propertyReads = 0;

  @override
  List<DBusIntrospectInterface> introspect() => [
    DBusIntrospectInterface(
      interface,
      methods: [
        DBusIntrospectMethod(
          'RegisterStatusNotifierHost',
          args: [
            DBusIntrospectArgument(
              DBusSignature('s'),
              DBusArgumentDirection.in_,
            ),
          ],
        ),
      ],
      properties: [
        DBusIntrospectProperty(
          'RegisteredStatusNotifierItems',
          DBusSignature('as'),
          access: DBusPropertyAccess.read,
        ),
      ],
    ),
  ];

  @override
  Future<DBusMethodResponse> handleMethodCall(DBusMethodCall methodCall) async {
    if (methodCall.interface != interface) {
      return DBusMethodErrorResponse.unknownInterface();
    }
    if (methodCall.name != 'RegisterStatusNotifierHost') {
      return DBusMethodErrorResponse.unknownMethod();
    }
    if (methodCall.signature != DBusSignature('s')) {
      return DBusMethodErrorResponse.invalidArgs();
    }
    hosts.add(methodCall.values.single.asString());
    return DBusMethodSuccessResponse();
  }

  @override
  Future<DBusMethodResponse> getProperty(String interface, String name) async {
    if (interface != this.interface) {
      return DBusMethodErrorResponse.unknownInterface();
    }
    if (name != 'RegisteredStatusNotifierItems') {
      return DBusMethodErrorResponse.unknownProperty();
    }
    propertyReads++;
    return DBusGetPropertyResponse(DBusArray.string(addresses));
  }
}
