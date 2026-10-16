// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/preferences.dart';
import 'package:clarion_shell/services/desktop_apps.dart';
import 'package:clarion_shell/services/clarion_ipc.dart';
import 'package:clarion_shell/services/system_service.dart';
import 'package:clarion_shell/services/wallpaper_service.dart';
import 'package:clarion_shell/state/shell_controller.dart';

class _Ipc extends ClarionIpc {
  final connection = StreamController<bool>.broadcast(sync: true);
  final event = StreamController<Map<String, dynamic>>.broadcast(sync: true);
  final actions = <(String, Map<String, Object?>)>[];
  bool failFocus = false;
  Completer<void>? focusGate;

  @override
  Stream<bool> get connections => connection.stream;
  @override
  Stream<Map<String, dynamic>> get events => event.stream;
  @override
  void start() {}
  @override
  Future<void> action(
    String name, [
    Map<String, Object?> arguments = const {},
  ]) async {
    if (name == 'FocusWindow') await focusGate?.future;
    if (name == 'FocusWindow' && failFocus) throw StateError('focus rejected');
    actions.add((name, arguments));
  }

  @override
  void dispose() {
    unawaited(connection.close());
    unawaited(event.close());
    super.dispose();
  }
}

class _Apps extends DesktopApps {
  _Apps(this.catalogue);
  final List<DesktopApp> catalogue;
  final launches = <String>[];
  Completer<void>? launchGate;
  @override
  Future<List<DesktopApp>> load() async => List.of(catalogue);
  @override
  Future<void> launch(DesktopApp app) async {
    launches.add(app.id);
    await launchGate?.future;
  }
}

class _System extends SystemService {
  final volumeWrites = <double>[];
  final brightnessWrites = <double>[];
  Completer<void>? volumeGate;
  Completer<void>? homeGate;
  Completer<void>? trashGate;
  bool failVolume = false;
  bool failHome = false;
  bool failTrash = false;
  int homeCalls = 0;
  int trashCalls = 0;
  @override
  Future<SystemSnapshot> read() async =>
      const SystemSnapshot(volume: .4, brightness: .5);
  @override
  Future<void> setVolume(double value) async {
    volumeWrites.add(value);
    await volumeGate?.future;
    if (failVolume) throw StateError('volume rejected');
  }

  @override
  Future<void> setBrightness(double value) async => brightnessWrites.add(value);
  @override
  Future<void> openHomeFolder() async {
    homeCalls++;
    await homeGate?.future;
    if (failHome) throw StateError('home rejected');
  }

  @override
  Future<void> openTrash() async {
    trashCalls++;
    await trashGate?.future;
    if (failTrash) throw StateError('trash rejected');
  }
}

class _Wallpapers extends WallpaperService {
  @override
  Future<void> restore({bool applySaved = false}) async {}
}

const _browser = DesktopApp(
  id: 'org.example.Browser.desktop',
  name: 'Example Browser',
  desktopFile: '/nonexistent/example-browser.desktop',
  iconName: 'web-browser',
);
const _editor = DesktopApp(
  id: 'org.example.Editor.desktop',
  name: 'Example Editor',
  desktopFile: '/nonexistent/example-editor.desktop',
);

Future<ShellController> _start(
  _Ipc ipc,
  _Apps apps, {
  _System? system,
  List<String>? pins,
}) async {
  final controller = ShellController(
    notifications: false,
    tray: false,
    powerProfilesEnabled: false,
    ipc: ipc,
    desktopApps: apps,
    system: system ?? _System(),
    wallpaperService: _Wallpapers(),
  );
  addTearDown(controller.dispose);
  controller.setPinnedAppIds(pins);
  await controller.start();
  ipc.connection.add(true);
  return controller;
}

Map<String, Object?> _window(
  int id,
  String appId, {
  bool focused = false,
  String? title,
}) => {
  'id': id,
  'app_id': appId,
  'title': title ?? 'Window $id',
  'workspace_id': 1,
  'is_focused': focused,
};

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  setUp(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
          const MethodChannel('clarion.shell/window'),
          (_) async => null,
        );
  });
  tearDown(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
          const MethodChannel('clarion.shell/window'),
          null,
        );
  });

  test('old preferences retain appearance and start with Material', () {
    final preferences = ShellPreferences.fromJson({
      'theme': 'light',
      'reduce_motion': true,
      'accent': '#123abc',
      'bar_position': 'bottom',
    });
    expect(preferences.shellStyle, ShellStyle.material);
    expect(preferences.pinnedAppIds, isNull);
    expect(preferences.themeMode, ThemeMode.light);
    expect(preferences.reduceMotion, isTrue);
    expect(preferences.seed, const Color(0xff123abc));
    expect(preferences.barPosition, BarPosition.bottom);
    expect(preferences.toJson().containsKey('pinned_app_ids'), isFalse);
  });

  test(
    'shell style and explicit pins round trip without losing settings',
    () async {
      final root = await Directory.systemTemp.createTemp('shell-style-prefs-');
      addTearDown(() => root.delete(recursive: true));
      final file = File('${root.path}/preferences.json');
      const preferences = ShellPreferences(
        themeMode: ThemeMode.dark,
        reduceMotion: true,
        seed: Color(0xff4285f4),
        barPosition: BarPosition.bottom,
        shellStyle: ShellStyle.macos,
        pinnedAppIds: [
          'org.example.Browser.desktop',
          'org.example.Editor.desktop',
        ],
      );
      await preferences.save(destination: file);
      final restored = await ShellPreferences.load(source: file);
      expect(restored.toJson(), preferences.toJson());
      expect(
        ShellPreferences.fromJson({'pinned_app_ids': []}).pinnedAppIds,
        isEmpty,
      );
      final invalid = ShellPreferences.fromJson({
        'shell_style': 'future-shell',
        'pinned_app_ids': [null, 7, ' ', ' app.desktop ', 'app.desktop'],
      });
      expect(invalid.shellStyle, ShellStyle.material);
      expect(invalid.pinnedAppIds, ['app.desktop']);
      expect(
        () => invalid.pinnedAppIds!.add('changed'),
        throwsUnsupportedError,
      );
    },
  );

  test(
    'style switching flushes sliders and clears transient surfaces',
    () async {
      final ipc = _Ipc();
      final system = _System();
      final controller = await _start(
        ipc,
        _Apps([_browser, _editor]),
        system: system,
      );
      controller.setBarPosition(BarPosition.bottom);
      controller.setThemeMode(ThemeMode.light);
      controller.setReduceMotion(true);
      controller.selectPanel(ShellPanel.quickSettings);
      controller.beginVolumeAdjustment();
      final volume = controller.setVolume(.81);
      final brightness = controller.setBrightness(.27);
      expect(system.volumeWrites, isEmpty);
      expect(system.brightnessWrites, isEmpty);
      controller.setShellStyle(ShellStyle.macos);
      expect(controller.panel, ShellPanel.none);
      expect(system.volumeWrites, [.81]);
      expect(system.brightnessWrites, [.27]);
      await Future.wait([volume, brightness]);
      expect(controller.themeMode, ThemeMode.light);
      expect(controller.reduceMotion, isTrue);
      expect(controller.barPosition, BarPosition.bottom);
      expect(controller.volume, .81);
      controller.showBarTooltip(Object(), '旧锚点', const Offset(12, 18));
      controller.setShellStyle(ShellStyle.material);
      expect(controller.barTooltip, isNull);
      expect(controller.barPosition, BarPosition.bottom);

      controller.openDockAppMenu(_browser.id);
      expect(controller.selectedDockApp, isNotNull);
      controller.setShellStyle(ShellStyle.macos);
      expect(controller.selectedDockAppId, isNull);
      expect(controller.panel, ShellPanel.none);
      controller.dispose();
      expect(
        () => controller.setShellStyle(ShellStyle.material),
        returnsNormally,
      );
      expect(controller.shellStyle, ShellStyle.macos);
    },
  );

  test(
    'pins and multiple running windows create one immutable application',
    () async {
      final ipc = _Ipc();
      final controller = await _start(
        ipc,
        _Apps([_browser, _editor]),
        pins: [_browser.id],
      );
      ipc.event.add({
        'WindowsChanged': {
          'windows': [
            _window(11, 'org.example.Browser'),
            _window(12, 'ORG.EXAMPLE.BROWSER.desktop', focused: true),
            _window(13, 'org.example.Editor'),
          ],
        },
      });
      expect(controller.dockApps.map((app) => app.id), [
        _browser.id,
        _editor.id,
      ]);
      final browser = controller.dockApps.first;
      expect(browser.windows.map((window) => window.id), [11, 12]);
      expect(browser.isPinned, isTrue);
      expect(browser.isRunning, isTrue);
      expect(browser.isFocused, isTrue);
      expect(controller.focusedWindow!.id, 12);
      expect(() => browser.windows.clear(), throwsUnsupportedError);
      expect(() => controller.windows.clear(), throwsUnsupportedError);
      expect(() => controller.pinnedAppIds.clear(), throwsUnsupportedError);

      ipc.event.add({
        'WindowFocusChanged': {'id': 13},
      });
      expect(browser.isFocused, isTrue); // A previous snapshot stays immutable.
      expect(controller.dockApps.first.isFocused, isFalse);
      expect(controller.focusedWindow!.id, 13);
      controller.setShellStyle(ShellStyle.macos);
      expect(controller.dockApps.map((app) => app.id), [
        _browser.id,
        _editor.id,
      ]);
      expect(controller.windows, hasLength(3));
      expect(controller.focusedWindow!.id, 13);
      expect(controller.pinnedAppIds, [_browser.id]);
      controller.togglePinnedApp(_browser.id);
      expect(controller.dockApps.first.isPinned, isFalse);
      expect(controller.pinnedAppIdsPreference, isEmpty);
      controller.togglePinnedApp(_editor.id);
      expect(controller.dockApps.first.id, _editor.id);
      expect(controller.pinnedAppIdsPreference, [_editor.id]);
    },
  );

  test(
    'native style failure restores Material with a visible explanation',
    () async {
      final controller = ShellController(demo: true);
      addTearDown(controller.dispose);
      controller.setThemeMode(ThemeMode.light);
      controller.setReduceMotion(true);
      controller.setBarPosition(BarPosition.bottom);
      controller.setPinnedAppIds(['terminal']);
      await controller.applyWallpaper(
        const ShellWallpaper(
          path: '/fixture/wallpaper.jpg',
          name: 'Saved',
          sizeBytes: 1,
        ),
      );
      controller.setShellStyle(ShellStyle.macos);
      controller.openDockAppMenu('terminal');
      controller.shellStyleFailed('Dock 首帧未完成');
      expect(controller.shellStyle, ShellStyle.material);
      expect(controller.panel, ShellPanel.quickSettings);
      expect(controller.error, contains('已恢复 Material'));
      expect(controller.error, contains('Dock 首帧未完成'));
      expect(controller.selectedDockAppId, isNull);
      expect(controller.barTooltip, isNull);
      expect(controller.themeMode, ThemeMode.light);
      expect(controller.reduceMotion, isTrue);
      expect(controller.barPosition, BarPosition.bottom);
      expect(controller.pinnedAppIdsPreference, ['terminal']);
      expect(controller.selectedWallpaperPath, '/fixture/wallpaper.jpg');
      controller.shellStyleFailed('迟到的重复回复');
      expect(controller.panel, ShellPanel.quickSettings);
      expect(controller.error, isNot(contains('迟到')));
      controller.dispose();
      expect(() => controller.shellStyleFailed('已销毁'), returnsNormally);
    },
  );

  for (final fails in [false, true]) {
    test(
      'a late slider ${fails ? 'failure' : 'success'} preserves the style failure',
      () async {
        final system = _System()
          ..volumeGate = Completer<void>()
          ..failVolume = fails;
        final controller = await _start(
          _Ipc(),
          _Apps([_browser]),
          system: system,
        );
        controller.selectPanel(ShellPanel.quickSettings);
        controller.beginVolumeAdjustment();
        final pending = controller.setVolume(.8);
        controller.setShellStyle(ShellStyle.macos);
        expect(system.volumeWrites, [.8]);
        controller.shellStyleFailed('首帧启动失败');
        final explanation = controller.error;
        system.volumeGate!.complete();
        await pending;
        expect(controller.panel, ShellPanel.quickSettings);
        expect(controller.error, explanation);
        expect(controller.error, contains('首帧启动失败'));
        expect(controller.volume, fails ? .4 : .8);
      },
    );
  }

  for (final action in ['home', 'trash']) {
    test(
      '$action keeps its panel on error and releases it after success',
      () async {
        final system = _System()
          ..failHome = action == 'home'
          ..failTrash = action == 'trash';
        final controller = await _start(
          _Ipc(),
          _Apps([_browser]),
          system: system,
        );
        controller.selectPanel(ShellPanel.systemMenu);
        await (action == 'home'
            ? controller.openHomeFolder()
            : controller.openTrash());
        expect(controller.panel, ShellPanel.systemMenu);
        expect(controller.error, contains('$action rejected'));
        system
          ..failHome = false
          ..failTrash = false;
        await (action == 'home'
            ? controller.openHomeFolder()
            : controller.openTrash());
        expect(controller.panel, ShellPanel.none);
        expect(controller.error, isNull);
      },
    );

    test('a delayed $action cannot close a newly reopened panel', () async {
      final gate = Completer<void>();
      final system = _System();
      if (action == 'home') {
        system.homeGate = gate;
      } else {
        system.trashGate = gate;
      }
      final controller = await _start(
        _Ipc(),
        _Apps([_browser]),
        system: system,
      );
      controller.selectPanel(ShellPanel.systemMenu);
      final pending = action == 'home'
          ? controller.openHomeFolder()
          : controller.openTrash();
      controller.selectPanel(ShellPanel.quickSettings);
      controller.selectPanel(ShellPanel.systemMenu);
      gate.complete();
      await pending;
      expect(controller.panel, ShellPanel.systemMenu);
    });

    test(
      'a delayed $action rejection cannot replace a newer style failure',
      () async {
        final gate = Completer<void>();
        final system = _System()
          ..failHome = action == 'home'
          ..failTrash = action == 'trash';
        if (action == 'home') {
          system.homeGate = gate;
        } else {
          system.trashGate = gate;
        }
        final controller = await _start(
          _Ipc(),
          _Apps([_browser]),
          system: system,
        );
        controller.setShellStyle(ShellStyle.macos);
        controller.selectPanel(ShellPanel.systemMenu);
        final pending = action == 'home'
            ? controller.openHomeFolder()
            : controller.openTrash();
        controller.shellStyleFailed('首帧启动失败');
        final explanation = controller.error;
        gate.complete();
        await pending;
        expect(controller.shellStyle, ShellStyle.material);
        expect(controller.panel, ShellPanel.quickSettings);
        expect(controller.error, explanation);
      },
    );
  }

  test('demo folder actions never invoke the system service', () async {
    final system = _System();
    final controller = ShellController(demo: true, system: system);
    addTearDown(controller.dispose);
    controller.selectPanel(ShellPanel.systemMenu);
    await controller.openHomeFolder();
    await controller.openTrash();
    expect(system.homeCalls, 0);
    expect(system.trashCalls, 0);
    expect(controller.error, isNull);
  });

  for (final action in ['focus', 'launch']) {
    test(
      'a delayed Dock $action cannot close a new menu after A-B-A navigation',
      () async {
        final gate = Completer<void>();
        final ipc = _Ipc();
        final apps = _Apps([_browser, _editor]);
        if (action == 'focus') {
          ipc.focusGate = gate;
        } else {
          apps.launchGate = gate;
        }
        final controller = await _start(ipc, apps, pins: [_browser.id]);
        if (action == 'focus') {
          ipc.event.add({
            'WindowsChanged': {
              'windows': [_window(44, 'org.example.Browser')],
            },
          });
        }
        controller.openDockAppMenu(_browser.id);
        final pending = controller.focusOrLaunchApp(_browser.id);
        controller.openDockAppMenu(_editor.id);
        controller.openDockAppMenu(_browser.id);
        gate.complete();
        await pending;
        expect(controller.panel, ShellPanel.dockAppMenu);
        expect(controller.selectedDockAppId, _browser.id);
        expect(controller.error, isNull);
      },
    );
  }

  test(
    'a removed pinned app retains live windows as an unknown app without relaunching',
    () async {
      final ipc = _Ipc();
      final apps = _Apps([_browser, _editor]);
      final controller = await _start(ipc, apps, pins: [_editor.id]);
      ipc.event.add({
        'WindowsChanged': {
          'windows': [_window(31, 'org.example.Editor')],
        },
      });
      final loaded = Completer<void>();
      controller.addListener(() {
        if (controller.apps.length == 1 && !loaded.isCompleted) {
          loaded.complete();
        }
      });
      apps.catalogue.remove(_editor);
      controller.selectPanel(ShellPanel.launcher);
      controller.openDockAppMenu(_editor.id);
      await loaded.future.timeout(const Duration(seconds: 2));
      expect(controller.panel, ShellPanel.none);
      expect(controller.pinnedAppIdsPreference, [_editor.id]);
      final running = controller.dockApps.single;
      expect(running.id, 'running:org.example.editor');
      expect(running.windows.single.id, 31);
      expect(running.canPin, isFalse);
      await controller.focusOrLaunchApp(running.id);
      expect(ipc.actions.single.$1, 'FocusWindow');
      expect(ipc.actions.single.$2, {'id': 31});
      expect(apps.launches, isEmpty);
      controller.openDockAppMenu(running.id);
      ipc.connection.add(false);
      expect(controller.panel, ShellPanel.none);
      expect(controller.dockApps, isEmpty);
      await controller.focusOrLaunchApp(_editor.id);
      expect(apps.launches, isEmpty);
    },
  );

  test(
    'clicking a running app focuses its existing window without launching',
    () async {
      final ipc = _Ipc();
      final apps = _Apps([_browser, _editor]);
      final controller = await _start(
        ipc,
        apps,
        pins: [_browser.id, _editor.id],
      );
      ipc.event.add({
        'WindowsChanged': {
          'windows': [
            _window(4, 'org.example.Browser'),
            _window(8, 'org.example.Browser', focused: true),
          ],
        },
      });
      controller.openDockAppMenu(_browser.id);
      await controller.focusOrLaunchApp(_browser.id);
      expect(ipc.actions, hasLength(1));
      expect(ipc.actions.single.$1, 'FocusWindow');
      expect(ipc.actions.single.$2, {'id': 8});
      expect(apps.launches, isEmpty);
      expect(controller.panel, ShellPanel.none);
      await controller.focusWindow(4);
      expect(ipc.actions.last.$1, 'FocusWindow');
      expect(ipc.actions.last.$2, {'id': 4});
      await controller.focusWindow(999);
      expect(ipc.actions.length, 2);
      await controller.focusOrLaunchApp(_editor.id);
      expect(apps.launches, [_editor.id]);
      controller.openDockAppMenu(_browser.id);
      ipc.failFocus = true;
      await controller.focusOrLaunchApp(_browser.id);
      expect(controller.panel, ShellPanel.dockAppMenu);
      expect(controller.error, contains('focus rejected'));
      expect(apps.launches, [_editor.id]);
    },
  );

  test(
    'an installed app can be pinned from its menu before it is launched',
    () async {
      final ipc = _Ipc();
      final apps = _Apps([_browser, _editor]);
      final controller = await _start(ipc, apps, pins: [_browser.id]);
      expect(controller.dockApps.map((app) => app.id), [_browser.id]);
      controller.openDockAppMenu(_editor.id);
      expect(controller.panel, ShellPanel.dockAppMenu);
      expect(controller.selectedDockApp!.id, _editor.id);
      expect(controller.selectedDockApp!.isRunning, isFalse);
      expect(controller.selectedDockApp!.isPinned, isFalse);
      expect(controller.selectedDockApp!.windows, isEmpty);
      controller.togglePinnedApp(_editor.id);
      expect(controller.pinnedAppIdsPreference, [_browser.id, _editor.id]);
      expect(controller.selectedDockApp!.isPinned, isTrue);
      expect(apps.launches, isEmpty);
      expect(ipc.actions, isEmpty);
      controller.togglePinnedApp(_editor.id);
      expect(controller.panel, ShellPanel.dockAppMenu);
      expect(controller.selectedDockApp!.id, _editor.id);
      controller.closePanel();
      controller.openDockAppMenu('not-installed.desktop');
      expect(controller.panel, ShellPanel.none);
      expect(controller.selectedDockApp, isNull);
    },
  );

  test(
    'unknown app IDs stay grouped and usable until their final window closes',
    () async {
      final ipc = _Ipc();
      final apps = _Apps([_browser]);
      final controller = await _start(ipc, apps, pins: []);
      ipc.event.add({
        'WindowsChanged': {
          'windows': [
            _window(2, 'Uncatalogued App'),
            _window(3, 'Uncatalogued App'),
          ],
        },
      });
      final entry = controller.dockApps.single;
      expect(entry.windows, hasLength(2));
      expect(entry.canPin, isFalse);
      expect(entry.id, 'running:uncatalogued app');
      controller.togglePinnedApp(entry.id);
      expect(controller.pinnedAppIds, isEmpty);
      await controller.focusOrLaunchApp(entry.id);
      expect(ipc.actions.single.$1, 'FocusWindow');
      expect(ipc.actions.single.$2, {'id': 3});
      expect(apps.launches, isEmpty);
      controller.openDockAppMenu(entry.id);
      ipc.event.add({
        'WindowClosed': {'id': 2},
      });
      expect(controller.selectedDockApp!.windows, hasLength(1));
      ipc.event.add({
        'WindowClosed': {'id': 3},
      });
      expect(controller.dockApps, isEmpty);
      expect(controller.panel, ShellPanel.none);
      expect(controller.selectedDockAppId, isNull);
    },
  );

  test(
    'installed desktop metadata matches native and Wine IDs with installed defaults',
    () async {
      final root = await Directory.systemTemp.createTemp(
        'shell-style-entries-',
      );
      addTearDown(() => root.delete(recursive: true));
      Future<DesktopApp> entry(String id, String name, String source) async {
        final file = File('${root.path}/$id');
        await file.writeAsString(
          '[Desktop Entry]\nType=Application\nName=$name\n$source\n',
        );
        return DesktopApp(id: id, name: name, desktopFile: file.path);
      }

      final terminal = await entry(
        'terminal.desktop',
        'Terminal',
        'Exec=env LANG=C /usr/bin/my-term\nCategories=TerminalEmulator;',
      );
      final browser = await entry(
        'browser.desktop',
        'Browser',
        'Exec=/usr/bin/example-browser %U\nStartupWMClass=BrowserRuntime\nCategories=WebBrowser;',
      );
      final wine = await entry(
        'wine-program.desktop',
        'Wine App',
        'Exec=wine "/home/user/drive_c/Program Files/Sample/sample.exe"\nStartupWMClass=Sample.exe',
      );
      final ipc = _Ipc();
      final apps = _Apps([terminal, browser, wine]);
      final controller = await _start(ipc, apps);
      expect(controller.pinnedAppIds.toSet(), {terminal.id, browser.id});
      expect(
        controller.pinnedAppIds.every(
          (id) => apps.catalogue.any((app) => app.id == id),
        ),
        isTrue,
      );
      expect(controller.pinnedAppIdsPreference, isNull);
      ipc.event.add({
        'WindowsChanged': {
          'windows': [
            _window(1, 'my-term'),
            _window(2, 'BrowserRuntime'),
            _window(3, 'sample.exe'),
            _window(4, 'sample'),
          ],
        },
      });
      expect(controller.dockApps.map((app) => app.id).toSet(), {
        terminal.id,
        browser.id,
        wine.id,
      });
      expect(controller.dockApps.last.windows, hasLength(2));
      await controller.focusOrLaunchApp('my-term');
      expect(ipc.actions.single.$1, 'FocusWindow');
      expect(ipc.actions.single.$2, {'id': 1});
      controller.setPinnedAppIds(['BrowserRuntime']);
      expect(controller.pinnedAppIds, [browser.id]);
      expect(controller.pinnedAppIdsPreference, ['BrowserRuntime']);
      controller.selectPanel(ShellPanel.launcher);
      await Future<void>.delayed(Duration.zero);
      expect(controller.pinnedAppIdsPreference, ['BrowserRuntime']);
      controller.setPinnedAppIds([]);
      expect(controller.pinnedAppIdsPreference, isEmpty);
      controller.selectPanel(ShellPanel.launcher);
      await Future<void>.delayed(Duration.zero);
      expect(controller.pinnedAppIds, isEmpty);
    },
  );
}
