// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:io';
import 'dart:convert';
import 'dart:async';
import 'dart:ui' as ui;
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/theme/shell_theme.dart';
import 'package:clarion_shell/ui/shell_view.dart';
import 'package:clarion_shell/ui/shell_geometry.dart';
import 'package:clarion_shell/ui/quick_settings_panel.dart';
import 'package:clarion_shell/ui/tray_icon.dart';

class _Controller extends ShellController {
  _Controller() : super(demo: true);
  final launched = <String>[];
  @override
  List<DesktopApp> get apps => const [
    DesktopApp(id: 'terminal', name: '终端', desktopFile: '', keywords: ['开发']),
    DesktopApp(id: 'files', name: '文件', desktopFile: ''),
    DesktopApp(id: 'browser', name: '浏览器', desktopFile: ''),
    DesktopApp(id: 'editor', name: '编辑器', desktopFile: ''),
    DesktopApp(id: 'settings', name: '配置中心', desktopFile: ''),
  ];
  @override
  bool get reduceMotion => true;
  @override
  Future<void> launchApp(DesktopApp app) async => launched.add(app.id);
}

Future<void> _mount(
  WidgetTester tester,
  ShellController controller,
  Size size, {
  bool preview = false,
  bool settle = true,
}) async {
  tester.view.resetPhysicalSize();
  tester.view.devicePixelRatio = 1;
  tester.view.physicalSize = size;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  await tester.pumpWidget(
    MaterialApp(
      theme: buildShellTheme(Brightness.dark),
      home: RepaintBoundary(
        key: const ValueKey('capture'),
        child: ShellView(controller: controller, preview: preview),
      ),
    ),
  );
  if (settle) await tester.pumpAndSettle();
}

Future<List<int>?> _thumbnailPixels(WidgetTester tester) async {
  final thumbnail = find.byType(Image).first;
  final decoded = Completer<ImageInfo?>();
  final stream = tester
      .widget<Image>(thumbnail)
      .image
      .resolve(createLocalImageConfiguration(tester.element(thumbnail)));
  final listener = ImageStreamListener(
    (info, _) => decoded.complete(info.clone()),
    onError: (_, _) => decoded.complete(null),
  );
  stream.addListener(listener);
  ImageInfo? info;
  try {
    info = await decoded.future.timeout(const Duration(seconds: 2));
    final pixels = await info?.image.toByteData(
      format: ui.ImageByteFormat.rawRgba,
    );
    return pixels?.buffer.asUint8List().take(4).toList();
  } finally {
    stream.removeListener(listener);
    info?.dispose();
  }
}

class _UnavailableController extends _Controller {
  @override
  double? get volume => null;
  @override
  double? get brightness => null;
  @override
  bool? get wifiEnabled => null;
  @override
  bool? get bluetoothEnabled => null;
}

class _BusyController extends _Controller {
  int wifiToggles = 0;
  @override
  Set<String> get busy => {'wifi'};
  @override
  Future<void> toggleWifi() async {
    wifiToggles++;
  }
}

class _TrayActionController extends _Controller {
  final activated = <String>[];
  final opened = <String>[];
  final menuEntries = <int>[];
  final secondary = <String>[];
  final wheel = <(String, int, String)>[];
  final menuPositions = <Offset>[];
  @override
  List<TrayItem> get trayItems => const [
    TrayItem(
      id: 'live-app',
      title: '下载',
      iconName: 'folder-download',
      menuAvailable: true,
    ),
  ];
  @override
  List<TrayMenuEntry> get trayMenu => const [
    TrayMenuEntry(id: 3, label: '偏好设置'),
    TrayMenuEntry(id: 4, label: '退出', destructive: true),
  ];
  @override
  TrayItem? get selectedTrayItem =>
      panel == ShellPanel.trayMenu ? trayItems.single : null;
  @override
  Future<void> activateTrayItem(
    TrayItem item, {
    double x = 0,
    double y = 0,
  }) async {
    activated.add(item.id);
  }

  @override
  Future<void> openTrayMenu(TrayItem item, {double x = 0, double y = 0}) async {
    opened.add(item.id);
    menuPositions.add(Offset(x, y));
    selectPanel(ShellPanel.trayMenu);
  }

  @override
  Future<void> secondaryActivateTrayItem(
    TrayItem item, {
    double x = 0,
    double y = 0,
  }) async {
    secondary.add(item.id);
  }

  @override
  Future<void> scrollTrayItem(
    TrayItem item,
    int delta, {
    String orientation = 'vertical',
  }) async {
    wheel.add((item.id, delta, orientation));
  }

  @override
  Future<void> activateTrayMenuEntry(int id) async {
    menuEntries.add(id);
    closePanel();
  }
}

class _MutableTrayController extends _Controller {
  List<TrayItem> entries = const [
    TrayItem(id: 'hover-a', title: '悬停 A'),
    TrayItem(id: 'hover-b', title: '悬停 B'),
  ];
  @override
  List<TrayItem> get trayItems => entries;
  void replace(List<TrayItem> value) {
    entries = value;
    notifyListeners();
  }
}

class _ManyTrayController extends _Controller {
  final activated = <String>[];
  @override
  List<TrayItem> get trayItems => List.generate(
    64,
    (i) => TrayItem(id: 'item-$i', title: '托盘应用 $i', menuAvailable: true),
  );
  @override
  Future<void> activateTrayItem(
    TrayItem item, {
    double x = 0,
    double y = 0,
  }) async {
    activated.add(item.id);
  }
}

class _ActionController extends _Controller {
  final invoked = <String>[];
  final dismissed = <int>[];
  @override
  List<ShellNotification> get notifications => [
    ShellNotification(
      id: 7,
      appName: '文件',
      summary: '下载完成',
      body: '打开文件或查看文件夹',
      timestamp: DateTime(2026, 10, 7, 12),
      actions: const [
        NotificationAction('default', '打开'),
        NotificationAction('folder', '查看文件夹'),
      ],
    ),
  ];
  @override
  Future<bool> invokeNotificationAction(
    int id,
    String key, {
    ShellNotification? expected,
  }) async {
    invoked.add('$id:$key');
    return true;
  }

  @override
  Future<bool> dismissNotification(
    int id, {
    ShellNotification? expected,
  }) async {
    dismissed.add(id);
    return true;
  }
}

class _WallpaperController extends _Controller {
  _WallpaperController(this.entries);
  final List<ShellWallpaper> entries;
  final applied = <String>[];
  int refreshes = 0;
  String? chosen;
  bool available = true;
  bool applying = false;
  String? failure;
  @override
  List<ShellWallpaper> get wallpapers => entries;
  @override
  String get wallpaperDirectory => '/图片/Wallpapers';
  @override
  String? get selectedWallpaperPath => chosen;
  @override
  bool get wallpaperAvailable => available;
  @override
  bool get wallpapersLoading => false;
  @override
  String? get wallpaperError => failure;
  @override
  Set<String> get busy => applying ? {'wallpaper'} : {};
  @override
  void openWallpapers() => selectPanel(ShellPanel.wallpapers);
  @override
  Future<void> refreshWallpapers() async {
    refreshes++;
    notifyListeners();
  }

  @override
  Future<void> applyWallpaper(ShellWallpaper wallpaper) async {
    applied.add(wallpaper.path);
    chosen = wallpaper.path;
    notifyListeners();
  }
}

class _LiveWallpaperController extends _WallpaperController {
  _LiveWallpaperController(super.entries);
  @override
  bool get demo => false;
}

void main() {
  late Directory wallpaperFixtures;
  setUpAll(() {
    wallpaperFixtures = Directory.systemTemp.createTempSync(
      'clarion-ui-wallpapers-',
    );
    final pixel = base64Decode(
      'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=',
    );
    for (final name in [
      '山间清晨.png',
      '城市夜景.png',
      for (var index = 0; index < 100; index++) 'wallpaper-$index.png',
    ]) {
      File('${wallpaperFixtures.path}/$name').writeAsBytesSync(pixel);
    }
    File(
      '${wallpaperFixtures.path}/损坏图片.png',
    ).writeAsStringSync('This is not an image.');
  });
  tearDownAll(() => wallpaperFixtures.deleteSync(recursive: true));
  List<ShellWallpaper> wallpaperEntries() => ['山间清晨.png', '城市夜景.png']
      .map(
        (name) => ShellWallpaper(
          path: '${wallpaperFixtures.path}/$name',
          name: name,
          sizeBytes: 68,
        ),
      )
      .toList();
  test(
    'Material themes retain readable semantic colors in both brightnesses',
    () {
      for (final brightness in Brightness.values) {
        final theme = buildShellTheme(brightness);
        expect(theme.useMaterial3, isTrue);
        expect(theme.colorScheme.brightness, brightness);
        for (final pair in [
          (theme.colorScheme.onSurface, theme.colorScheme.surfaceContainer),
          (
            theme.colorScheme.onPrimaryContainer,
            theme.colorScheme.primaryContainer,
          ),
          (
            theme.colorScheme.onSecondaryContainer,
            theme.colorScheme.secondaryContainer,
          ),
        ]) {
          final a = pair.$1.computeLuminance();
          final b = pair.$2.computeLuminance();
          final ratio =
              (a > b ? a + .05 : b + .05) / (a > b ? b + .05 : a + .05);
          expect(ratio, greaterThanOrEqualTo(4.5));
        }
      }
    },
  );

  testWidgets('entry motion advances and finishes without an idle ticker', (
    tester,
  ) async {
    final controller = ShellController(demo: true)
      ..selectPanel(ShellPanel.quickSettings);
    addTearDown(controller.dispose);
    await _mount(tester, controller, const Size(1280, 900), settle: false);
    final opacity = find
        .descendant(
          of: find.byKey(const ValueKey(ShellPanel.quickSettings)),
          matching: find.byType(Opacity),
        )
        .first;
    expect(tester.widget<Opacity>(opacity).opacity, lessThan(1));
    await tester.pump(const Duration(milliseconds: 100));
    expect(tester.widget<Opacity>(opacity).opacity, greaterThan(0));
    await tester.pumpAndSettle();
    expect(tester.widget<Opacity>(opacity).opacity, 1);
    expect(tester.binding.transientCallbackCount, 0);
  });

  testWidgets('reduced motion completes entry in its first frame', (
    tester,
  ) async {
    final controller = _Controller()..selectPanel(ShellPanel.quickSettings);
    addTearDown(controller.dispose);
    await _mount(tester, controller, const Size(1280, 900), settle: false);
    final opacity = find
        .descendant(
          of: find.byKey(const ValueKey(ShellPanel.quickSettings)),
          matching: find.byType(Opacity),
        )
        .first;
    expect(tester.widget<Opacity>(opacity).opacity, 1);
    expect(tester.binding.transientCallbackCount, 0);
  });

  testWidgets('bar stays transparent below its reserved strip', (tester) async {
    final controller = _Controller();
    addTearDown(controller.dispose);
    await _mount(tester, controller, const Size(1280, 720));
    expect(find.byType(Scaffold), findsNothing);
    expect(find.text('应用'), findsNothing);
    expect(tester.takeException(), isNull);
  });

  testWidgets('collapsed desktop strip fits its compact native height', (
    tester,
  ) async {
    final controller = _Controller();
    addTearDown(controller.dispose);
    await _mount(tester, controller, const Size(1280, ShellGeometry.barHeight));
    expect(tester.takeException(), isNull);
    final launcher = tester.getSize(find.byKey(const ValueKey('bar-launcher')));
    expect(launcher.height, inInclusiveRange(32, ShellGeometry.barHeight));
    expect(launcher.width, inInclusiveRange(32, 40));
    final workspace = tester.getSize(find.byKey(const ValueKey('workspace-1')));
    expect(workspace.height, lessThanOrEqualTo(ShellGeometry.barHeight));
  });

  testWidgets(
    'floating panels retain normal controls below the compact strip',
    (tester) async {
      final controller = _Controller()..selectPanel(ShellPanel.quickSettings);
      addTearDown(controller.dispose);
      await _mount(tester, controller, const Size(1280, 900));
      expect(
        tester.getTopLeft(find.byType(QuickSettingsPanel)).dy,
        ShellGeometry.panelTop,
      );
      expect(
        tester.getSize(find.byTooltip('关闭')).height,
        greaterThanOrEqualTo(48),
      );
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'do not disturb hides the badge and retains notification history',
    (tester) async {
      final controller = _Controller();
      addTearDown(controller.dispose);
      await _mount(tester, controller, const Size(1280, 720));
      final history = controller.notifications.length;
      Finder notificationBadge() => find.descendant(
        of: find.byKey(const ValueKey('bar-notifications')),
        matching: find.byType(Badge),
      );
      expect(tester.widget<Badge>(notificationBadge()).isLabelVisible, isTrue);
      controller.setDoNotDisturb(true);
      await tester.pump();
      expect(tester.widget<Badge>(notificationBadge()).isLabelVisible, isFalse);
      expect(controller.notifications.length, history);
    },
  );

  testWidgets('search finds keywords and Enter launches the selected result', (
    tester,
  ) async {
    final controller = _Controller()..selectPanel(ShellPanel.launcher);
    addTearDown(controller.dispose);
    await _mount(tester, controller, const Size(800, 600));
    await tester.enterText(find.byKey(const ValueKey('launcher-search')), '开发');
    await tester.pump();
    expect(find.byKey(const ValueKey('app-terminal')), findsOneWidget);
    expect(find.byKey(const ValueKey('app-files')), findsNothing);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pump();
    expect(controller.launched, ['terminal']);
  });

  testWidgets(
    'arrow navigation launches the next grid row without double activation',
    (tester) async {
      final controller = _Controller()..selectPanel(ShellPanel.launcher);
      addTearDown(controller.dispose);
      await _mount(tester, controller, const Size(560, 600));
      await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pump();
      expect(controller.launched, ['editor']);
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pump();
      expect(controller.panel, ShellPanel.none);
    },
  );

  testWidgets('outside click closes a panel and releases its controls', (
    tester,
  ) async {
    final controller = _Controller()..selectPanel(ShellPanel.quickSettings);
    addTearDown(controller.dispose);
    await _mount(tester, controller, const Size(1280, 800));
    await tester.tapAt(const Offset(80, 500));
    await tester.pump();
    expect(controller.panel, ShellPanel.none);
    expect(find.byKey(const ValueKey('volume-slider')), findsNothing);
  });

  testWidgets('unavailable services disable hardware sliders and show status', (
    tester,
  ) async {
    final controller = _UnavailableController()
      ..selectPanel(ShellPanel.quickSettings);
    addTearDown(controller.dispose);
    await _mount(tester, controller, const Size(1280, 900));
    expect(
      tester
          .widget<Slider>(find.byKey(const ValueKey('volume-slider')))
          .onChanged,
      isNull,
    );
    expect(
      tester
          .widget<Slider>(find.byKey(const ValueKey('brightness-slider')))
          .onChanged,
      isNull,
    );
    expect(find.text('服务不可用'), findsNWidgets(2));
  });

  testWidgets('busy connectivity cannot dispatch a second hardware change', (
    tester,
  ) async {
    final controller = _BusyController()..selectPanel(ShellPanel.quickSettings);
    addTearDown(controller.dispose);
    await _mount(tester, controller, const Size(1280, 900), settle: false);
    expect(find.byType(CircularProgressIndicator), findsOneWidget);
    await tester.tap(find.text('Wi-Fi'));
    expect(controller.wifiToggles, 0);
  });

  testWidgets('notification buttons dispatch only their intended action', (
    tester,
  ) async {
    final controller = _ActionController()
      ..selectPanel(ShellPanel.notifications);
    addTearDown(controller.dispose);
    await _mount(tester, controller, const Size(1280, 720));
    await tester.tap(find.text('查看文件夹'));
    await tester.pump();
    expect(controller.invoked, ['7:folder']);
    await tester.tap(find.text('下载完成'));
    await tester.pump();
    expect(controller.invoked, ['7:folder', '7:default']);
    await tester.tap(find.byTooltip('关闭通知'));
    await tester.pump();
    expect(controller.dismissed, [7]);
    expect(controller.invoked.length, 2);
  });

  testWidgets('tray icons fit the compact strip and open the overlay menu', (
    tester,
  ) async {
    final controller = _Controller();
    addTearDown(controller.dispose);
    await _mount(tester, controller, const Size(1280, 720));
    final network = find.byKey(const ValueKey('tray-demo-network'));
    final chat = find.byKey(const ValueKey('tray-demo-chat'));
    expect(network, findsOneWidget);
    expect(chat, findsOneWidget);
    expect(tester.getSize(network).height, inInclusiveRange(32, 36));
    await tester.tap(network);
    await tester.pumpAndSettle();
    expect(controller.panel, ShellPanel.trayMenu);
    expect(find.text('显示窗口'), findsOneWidget);
    await tester.tap(find.text('退出'));
    await tester.pumpAndSettle();
    expect(controller.panel, ShellPanel.none);
  });

  testWidgets('tray secondary click opens the menu without activating first', (
    tester,
  ) async {
    final controller = _TrayActionController();
    addTearDown(controller.dispose);
    await _mount(tester, controller, const Size(1280, 720));
    await tester.tap(find.byKey(const ValueKey('tray-live-app')), buttons: 2);
    await tester.pumpAndSettle();
    expect(controller.activated, isEmpty);
    expect(controller.opened, ['live-app']);
    expect(controller.panel, ShellPanel.trayMenu);
    expect(find.text('偏好设置'), findsOneWidget);
    await tester.tap(find.text('偏好设置'));
    await tester.pump();
    expect(controller.menuEntries, [3]);
  });

  testWidgets(
    'bar hints wait 400ms and render below the icons within the hover surface',
    (tester) async {
      final controller = _Controller();
      addTearDown(controller.dispose);
      await _mount(tester, controller, const Size(1280, 128));
      final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
      await mouse.addPointer(location: const Offset(2, 120));
      await mouse.moveTo(
        tester.getCenter(find.byKey(const ValueKey('tray-demo-chat'))),
      );
      await tester.pump(const Duration(milliseconds: 399));
      expect(controller.barTooltip, null);
      await tester.pump(const Duration(milliseconds: 1));
      expect(controller.barTooltip?.message, '消息');
      final bubble = find.byKey(const ValueKey('bar-tooltip-bubble'));
      expect(bubble, findsOneWidget);
      final rect = tester.getRect(bubble);
      expect(rect.top, 48);
      expect(rect.bottom, lessThanOrEqualTo(128));
      expect(rect.width, lessThanOrEqualTo(280));
      expect(rect.height, lessThanOrEqualTo(76));
      expect(
        rect.overlaps(
          tester.getRect(find.byKey(const ValueKey('tray-demo-chat'))),
        ),
        false,
      );
      await mouse.removePointer();
      await tester.pump();
      expect(controller.barTooltip, null);
      expect(bubble, findsNothing);
    },
  );

  testWidgets(
    'bar hint paint waits for the native hover viewport instead of clipping into a 40px strip',
    (tester) async {
      final controller = _Controller();
      addTearDown(controller.dispose);
      await _mount(tester, controller, const Size(1280, 40));
      final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
      await mouse.addPointer(location: const Offset(2, 39));
      await mouse.moveTo(
        tester.getCenter(find.byKey(const ValueKey('tray-demo-chat'))),
      );
      await tester.pump(const Duration(milliseconds: 400));
      expect(controller.barTooltip?.message, '消息');
      expect(find.byKey(const ValueKey('bar-tooltip-bubble')), findsNothing);
      tester.view.physicalSize = const Size(1280, 128);
      await tester.pump();
      expect(find.byKey(const ValueKey('bar-tooltip-bubble')), findsOneWidget);
      await mouse.removePointer();
      await tester.pump();
    },
  );

  testWidgets(
    'quick hover exits and switching icons never resurrect an old hint',
    (tester) async {
      final controller = _Controller();
      addTearDown(controller.dispose);
      await _mount(tester, controller, const Size(1280, 128));
      final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
      await mouse.addPointer(location: const Offset(2, 120));
      await mouse.moveTo(
        tester.getCenter(find.byKey(const ValueKey('tray-demo-network'))),
      );
      await tester.pump(const Duration(milliseconds: 200));
      await mouse.moveTo(const Offset(2, 120));
      await tester.pump(const Duration(milliseconds: 500));
      expect(controller.barTooltip, null);
      await mouse.moveTo(
        tester.getCenter(find.byKey(const ValueKey('tray-demo-network'))),
      );
      await tester.pump(const Duration(milliseconds: 400));
      expect(controller.barTooltip?.message, '网络');
      await mouse.moveTo(
        tester.getCenter(find.byKey(const ValueKey('tray-demo-chat'))),
      );
      await tester.pump(const Duration(milliseconds: 400));
      expect(controller.barTooltip?.message, '消息');
      await tester.pump(const Duration(seconds: 1));
      expect(controller.barTooltip?.message, '消息');
      await mouse.removePointer();
      await tester.pump();
    },
  );

  testWidgets(
    'tray clicks, menu opening and unloading clear displayed and pending hints',
    (tester) async {
      final controller = _MutableTrayController();
      addTearDown(controller.dispose);
      await _mount(tester, controller, const Size(1280, 720));
      final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
      await mouse.addPointer(location: const Offset(2, 120));
      await mouse.moveTo(
        tester.getCenter(find.byKey(const ValueKey('tray-hover-a'))),
      );
      await tester.pump(const Duration(milliseconds: 400));
      expect(controller.barTooltip?.message, '悬停 A');
      await tester.tap(
        find.byKey(const ValueKey('tray-hover-a')),
        buttons: kSecondaryButton,
      );
      await tester.pumpAndSettle();
      expect(controller.panel, ShellPanel.trayMenu);
      expect(controller.barTooltip, null);
      await tester.pump(const Duration(seconds: 1));
      expect(controller.barTooltip, null);
      controller.closePanel();
      await tester.pump();
      await mouse.moveTo(const Offset(2, 120));
      await mouse.moveTo(
        tester.getCenter(find.byKey(const ValueKey('tray-hover-b'))),
      );
      await tester.pump(const Duration(milliseconds: 200));
      controller.replace(const []);
      await tester.pump();
      expect(controller.barTooltip, null);
      // Removing tray icons shifts the remaining controls under the stationary
      // pointer. Leave the strip so a valid new volume hint cannot affect this
      // assertion about the disposed tray item's pending timer.
      await mouse.moveTo(const Offset(2, 120));
      await tester.pump(const Duration(milliseconds: 600));
      expect(controller.barTooltip, null);
      await mouse.removePointer();
      await tester.pump();
    },
  );

  testWidgets('long hints stay within output edges and preserve a 40px bar', (
    tester,
  ) async {
    final controller = _MutableTrayController();
    addTearDown(controller.dispose);
    controller.replace([
      TrayItem(id: 'long-title', title: List.filled(40, '很长的托盘应用标题').join()),
    ]);
    await _mount(tester, controller, const Size(768, 128));
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: const Offset(2, 120));
    await mouse.moveTo(
      tester.getCenter(find.byKey(const ValueKey('tray-long-title'))),
    );
    await tester.pump(const Duration(milliseconds: 400));
    final bubble = find.byKey(const ValueKey('bar-tooltip-bubble'));
    final rect = tester.getRect(bubble);
    expect(rect.left, greaterThanOrEqualTo(8));
    expect(rect.right, lessThanOrEqualTo(760));
    expect(rect.top, 48);
    expect(rect.bottom, lessThanOrEqualTo(128));
    expect(rect.height, lessThanOrEqualTo(76));
    expect(tester.takeException(), null);
    final text = tester.widget<Text>(
      find.descendant(of: bubble, matching: find.byType(Text)),
    );
    expect(text.maxLines, 3);
    expect(text.overflow, TextOverflow.ellipsis);
    expect(
      tester.getSize(find.byKey(const ValueKey('tray-long-title'))).height,
      36,
    );
    await mouse.moveTo(
      tester.getCenter(find.byKey(const ValueKey('bar-launcher'))),
    );
    await tester.pump(const Duration(milliseconds: 400));
    expect(controller.barTooltip?.message, '应用与搜索');
    expect(tester.getRect(bubble).left, greaterThanOrEqualTo(8));
    await mouse.removePointer();
    await tester.pump();
  });

  testWidgets(
    'unmounting a shown bar hint releases its owner without build-time notifications',
    (tester) async {
      final controller = _Controller();
      addTearDown(controller.dispose);
      await _mount(tester, controller, const Size(1280, 128));
      final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
      await mouse.addPointer(location: const Offset(2, 120));
      await mouse.moveTo(
        tester.getCenter(find.byKey(const ValueKey('tray-demo-chat'))),
      );
      await tester.pump(const Duration(milliseconds: 400));
      expect(controller.barTooltip, isNotNull);
      await tester.pumpWidget(const SizedBox.shrink());
      await tester.pump();
      expect(controller.barTooltip, null);
      expect(tester.takeException(), null);
      await mouse.removePointer();
      await tester.pump();
    },
  );

  testWidgets('all 64 tray applications stay accessible on a narrow strip', (
    tester,
  ) async {
    final controller = _ManyTrayController();
    addTearDown(controller.dispose);
    await _mount(tester, controller, const Size(320, 720));
    expect(find.byKey(const ValueKey('tray-overflow')), findsOneWidget);
    expect(tester.takeException(), null);
    await tester.tap(find.byKey(const ValueKey('tray-overflow')));
    await tester.pumpAndSettle();
    expect(controller.panel, ShellPanel.trayItems);
    await tester.scrollUntilVisible(
      find.byKey(const ValueKey('tray-list-item-63')),
      350,
      scrollable: find.descendant(
        of: find.byKey(const ValueKey('tray-items-list')),
        matching: find.byType(Scrollable),
      ),
    );
    await tester.tap(find.byKey(const ValueKey('tray-list-item-63')));
    await tester.pump();
    expect(controller.activated, ['item-63']);
    expect(tester.takeException(), null);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pump();
    expect(controller.panel, ShellPanel.none);
  });

  testWidgets(
    'mouse wheels scroll the tray overflow list instead of forwarding to applications',
    (tester) async {
      final controller = _ManyTrayController();
      addTearDown(controller.dispose);
      await _mount(tester, controller, const Size(320, 720));
      await tester.tap(find.byKey(const ValueKey('tray-overflow')));
      await tester.pumpAndSettle();
      final scrollable = find.descendant(
        of: find.byKey(const ValueKey('tray-items-list')),
        matching: find.byType(Scrollable),
      );
      final before = tester.state<ScrollableState>(scrollable).position.pixels;
      await tester.sendEventToBinding(
        PointerScrollEvent(
          position: tester.getCenter(
            find.byKey(const ValueKey('tray-list-item-0')),
          ),
          scrollDelta: const Offset(0, 240),
        ),
      );
      await tester.pumpAndSettle();
      expect(
        tester.state<ScrollableState>(scrollable).position.pixels,
        greaterThan(before),
      );
      expect(controller.activated, isEmpty);
    },
  );

  testWidgets(
    'large tray collections fit medium and wide strips at increased text scale',
    (tester) async {
      final controller = _ManyTrayController();
      addTearDown(controller.dispose);
      for (final width in [240.0, 768.0, 1000.0, 1280.0]) {
        tester.platformDispatcher.textScaleFactorTestValue = 1.4;
        addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
        await _mount(tester, controller, Size(width, 720));
        expect(find.byKey(const ValueKey('tray-overflow')), findsOneWidget);
        expect(tester.takeException(), null, reason: 'width $width');
        expect(
          tester.getSize(find.byKey(const ValueKey('tray-overflow'))).height,
          36,
        );
      }
    },
  );

  testWidgets(
    'middle click and wheel forwarding do not activate a tray application',
    (tester) async {
      final controller = _TrayActionController();
      addTearDown(controller.dispose);
      await _mount(tester, controller, const Size(1280, 720));
      final icon = find.byKey(const ValueKey('tray-live-app'));
      final position = tester.getCenter(icon);
      await tester.tap(icon, buttons: kTertiaryButton);
      await tester.pump();
      expect(controller.secondary, ['live-app']);
      expect(controller.activated, isEmpty);
      await tester.sendEventToBinding(
        PointerScrollEvent(
          position: position,
          scrollDelta: const Offset(0, -120),
        ),
      );
      await tester.pump();
      expect(controller.wheel, [('live-app', -120, 'vertical')]);
      expect(controller.opened, isEmpty);
    },
  );

  testWidgets(
    'right clicks preserve pointer coordinates and tray icons expose attention state',
    (tester) async {
      final controller = _TrayActionController();
      addTearDown(controller.dispose);
      await _mount(tester, controller, const Size(1280, 720));
      final icon = find.byKey(const ValueKey('tray-live-app'));
      final position = tester.getCenter(icon);
      await tester.tapAt(position, buttons: kSecondaryButton);
      await tester.pumpAndSettle();
      expect(controller.menuPositions.single, position);
      controller.closePanel();
      await tester.pumpAndSettle();
      final semantics = tester.ensureSemantics();
      try {
        final demo = _Controller();
        addTearDown(demo.dispose);
        await _mount(tester, demo, const Size(1280, 720));
        expect(find.bySemanticsLabel('消息'), findsOneWidget);
        final node = tester.getSemantics(find.bySemanticsLabel('消息'));
        expect(node.value, '需要关注');
      } finally {
        semantics.dispose();
      }
    },
  );

  testWidgets('focused tray icons support Enter and the keyboard menu key', (
    tester,
  ) async {
    final controller = _TrayActionController();
    addTearDown(controller.dispose);
    await tester.pumpWidget(
      MaterialApp(
        home: Center(
          child: TrayBarButton(
            controller: controller,
            item: controller.trayItems.single,
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.sendKeyEvent(LogicalKeyboardKey.tab);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pump();
    expect(controller.activated, ['live-app']);
    await tester.sendKeyEvent(LogicalKeyboardKey.contextMenu);
    await tester.pump();
    expect(controller.opened, ['live-app']);
    expect(controller.activated, ['live-app']);
  });

  testWidgets(
    'icon futures retain the newest item and do not cache missing names permanently',
    (tester) async {
      final old = Completer<String?>();
      var missingCalls = 0;
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(
            const MethodChannel('clarion.shell/window'),
            (call) async {
              if (call.method != 'appIcon') return null;
              if (call.arguments == 'old-race-icon') return old.future;
              if (call.arguments == 'missing-race-icon') missingCalls++;
              return null;
            },
          );
      addTearDown(
        () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
            .setMockMethodCallHandler(
              const MethodChannel('clarion.shell/window'),
              null,
            ),
      );
      Future<void> paint(TrayItem item) => tester.pumpWidget(
        MaterialApp(
          home: Center(
            child: TrayGlyph(key: const ValueKey('future-glyph'), item: item),
          ),
        ),
      );
      await paint(
        const TrayItem(id: 'app', title: 'app', iconName: 'old-race-icon'),
      );
      await paint(
        const TrayItem(id: 'app', title: 'app', iconName: 'missing-race-icon'),
      );
      await tester.pumpAndSettle();
      old.complete('${wallpaperFixtures.path}/山间清晨.png');
      await tester.pumpAndSettle();
      expect(find.byType(Image), findsNothing);
      expect(missingCalls, 1);
      await paint(
        TrayItem(id: 'app', title: 'updated', iconName: 'missing-race-icon'),
      );
      await tester.pumpAndSettle();
      expect(missingCalls, 2);
    },
  );

  testWidgets(
    'custom icon themes and requested sizes use independent native resolutions',
    (tester) async {
      final calls = <Object?>[];
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(
            const MethodChannel('clarion.shell/window'),
            (call) async {
              if (call.method == 'appIcon') calls.add(call.arguments);
              return null;
            },
          );
      addTearDown(
        () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
            .setMockMethodCallHandler(
              const MethodChannel('clarion.shell/window'),
              null,
            ),
      );
      Future<void> paint(String themePath, double size) async {
        await tester.runAsync(() async {
          await tester.pumpWidget(
            MaterialApp(
              home: Center(
                child: TrayGlyph(
                  key: const ValueKey('custom-theme-glyph'),
                  item: TrayItem(
                    id: 'app',
                    title: 'app',
                    iconName: 'custom-race-icon',
                    iconThemePath: themePath,
                  ),
                  size: size,
                ),
              ),
            ),
          );
          await Future<void>.delayed(const Duration(milliseconds: 50));
        });
        await tester.pumpAndSettle();
      }

      final a = '${wallpaperFixtures.path}/theme-a';
      final b = '${wallpaperFixtures.path}/theme-b';
      await paint(a, 20);
      await paint(b, 20);
      await paint(b, 28);
      expect(calls, [
        {'name': 'custom-race-icon', 'themePath': a, 'size': 20},
        {'name': 'custom-race-icon', 'themePath': b, 'size': 20},
        {'name': 'custom-race-icon', 'themePath': b, 'size': 28},
      ]);
    },
  );

  testWidgets(
    'pixmap removal and newer decode win over old asynchronous callbacks',
    (tester) async {
      TrayItem pixmapItem(String id, List<int> bytes) => TrayItem(
        id: id,
        title: id,
        iconPixmap: TrayIconPixmap(
          width: 1,
          height: 1,
          rgba: Uint8List.fromList(bytes),
        ),
      );
      final red = pixmapItem('app', [255, 0, 0, 255]);
      final green = pixmapItem('app', [0, 255, 0, 255]);
      Future<void> paint(TrayItem item) => tester.pumpWidget(
        MaterialApp(
          home: Center(
            child: TrayGlyph(key: const ValueKey('same-glyph'), item: item),
          ),
        ),
      );
      await paint(red);
      await paint(green);
      await tester.runAsync(() async {
        await Future<void>.delayed(const Duration(milliseconds: 50));
      });
      await tester.pump();
      final image = tester.widget<RawImage>(find.byType(RawImage)).image!;
      final data = await tester.runAsync(
        () => image.toByteData(format: ui.ImageByteFormat.rawRgba),
      );
      expect(data!.buffer.asUint8List().toList(), [0, 255, 0, 255]);
      await paint(const TrayItem(id: 'app', title: 'app'));
      await tester.pumpAndSettle();
      expect(find.byType(RawImage), findsNothing);
      expect(tester.takeException(), null);
    },
  );

  testWidgets('wallpaper entry opens gallery and can return to the controls', (
    tester,
  ) async {
    final controller = _WallpaperController(wallpaperEntries())
      ..selectPanel(ShellPanel.quickSettings);
    addTearDown(controller.dispose);
    await _mount(tester, controller, const Size(1280, 900));
    await tester.ensureVisible(find.byKey(const ValueKey('choose-wallpaper')));
    await tester.tap(find.byKey(const ValueKey('choose-wallpaper')));
    await tester.pumpAndSettle();
    expect(controller.panel, ShellPanel.wallpapers);
    expect(find.byKey(const ValueKey('wallpaper-search')), findsOneWidget);
    await tester.tap(find.byTooltip('返回控制中心'));
    await tester.pumpAndSettle();
    expect(controller.panel, ShellPanel.quickSettings);
  });

  testWidgets(
    'wallpaper search and selection pass the file identity to controller',
    (tester) async {
      final entries = wallpaperEntries();
      final controller = _WallpaperController(entries)
        ..selectPanel(ShellPanel.wallpapers);
      addTearDown(controller.dispose);
      await _mount(tester, controller, const Size(800, 600));
      await tester.enterText(
        find.byKey(const ValueKey('wallpaper-search')),
        '城市',
      );
      await tester.pump();
      expect(
        find.byKey(ValueKey('wallpaper-${entries.first.path}')),
        findsNothing,
      );
      await tester.tap(find.byKey(ValueKey('wallpaper-${entries.last.path}')));
      await tester.pump();
      expect(controller.applied, [entries.last.path]);
      expect(controller.chosen, entries.last.path);
      await tester.tap(find.byTooltip('刷新壁纸'));
      expect(controller.refreshes, 1);
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pump();
      expect(controller.panel, ShellPanel.none);
    },
  );

  testWidgets('wallpaper grid creates only visible thumbnail cells', (
    tester,
  ) async {
    final sample = wallpaperEntries().first;
    final entries = List.generate(
      100,
      (index) => ShellWallpaper(
        path: '${wallpaperFixtures.path}/wallpaper-$index.png',
        name: 'wallpaper-$index.png',
        sizeBytes: sample.sizeBytes,
      ),
    );
    final controller = _WallpaperController(entries)
      ..selectPanel(ShellPanel.wallpapers);
    addTearDown(controller.dispose);
    await _mount(tester, controller, const Size(1280, 720));
    final decodedCells = tester.widgetList<Image>(find.byType(Image)).length;
    expect(decodedCells, greaterThan(0));
    expect(decodedCells, lessThan(40));
  });

  testWidgets('wallpaper busy state prevents a second apply', (tester) async {
    final entries = wallpaperEntries();
    final controller = _WallpaperController(entries)..applying = true;
    controller.selectPanel(ShellPanel.wallpapers);
    addTearDown(controller.dispose);
    await _mount(tester, controller, const Size(1280, 720), settle: false);
    await tester.tap(find.byKey(ValueKey('wallpaper-${entries.first.path}')));
    expect(controller.applied, isEmpty);
    expect(find.byType(LinearProgressIndicator), findsOneWidget);
  });

  testWidgets('invalid image preview cannot be applied', (tester) async {
    final wallpaper = ShellWallpaper(
      path: '${wallpaperFixtures.path}/损坏图片.png',
      name: '损坏图片.png',
      sizeBytes: 21,
    );
    final controller = _WallpaperController([wallpaper])
      ..selectPanel(ShellPanel.wallpapers);
    addTearDown(controller.dispose);
    await tester.runAsync(() async {
      await _mount(tester, controller, const Size(800, 600), settle: false);
      expect(await _thumbnailPixels(tester), isNull);
    });
    await tester.pumpAndSettle();
    expect(find.text('图片无法预览'), findsOneWidget);
    await tester.tap(find.byKey(ValueKey('wallpaper-${wallpaper.path}')));
    expect(controller.applied, isEmpty);
    expect(tester.takeException(), isNull);
  });

  testWidgets('refresh enables a repaired image at the same file path', (
    tester,
  ) async {
    final file = File('${wallpaperFixtures.path}/repairable.png')
      ..writeAsStringSync('This is not an image.');
    ShellWallpaper revision() {
      final stat = file.statSync();
      return ShellWallpaper(
        path: file.path,
        name: 'repairable.png',
        sizeBytes: stat.size,
        modified: stat.modified,
      );
    }

    final entries = [revision()];
    final controller = _WallpaperController(entries)
      ..selectPanel(ShellPanel.wallpapers);
    addTearDown(controller.dispose);
    await tester.runAsync(() async {
      await _mount(tester, controller, const Size(800, 600), settle: false);
      expect(await _thumbnailPixels(tester), isNull);
    });
    await tester.pumpAndSettle();
    expect(find.text('图片无法预览'), findsOneWidget);
    final originalRevision = entries.single;
    file.writeAsBytesSync(
      File('${wallpaperFixtures.path}/山间清晨.png').readAsBytesSync(),
    );
    file.setLastModifiedSync(
      originalRevision.modified!.add(const Duration(seconds: 2)),
    );
    entries[0] = revision();
    await tester.runAsync(() async {
      await tester.tap(find.byTooltip('刷新壁纸'));
      await tester.pump();
      expect(await _thumbnailPixels(tester), isNotNull);
    });
    await tester.pumpAndSettle();
    expect(find.text('图片无法预览'), findsNothing);
    await tester.tap(find.byKey(ValueKey('wallpaper-${file.path}')));
    expect(controller.applied, [file.path]);
    expect(tester.takeException(), isNull);
  });

  testWidgets('refresh replaces cached pixels after a same-size file overwrite', (
    tester,
  ) async {
    final file = File('${wallpaperFixtures.path}/replaced.png')
      ..writeAsBytesSync(
        File('${wallpaperFixtures.path}/山间清晨.png').readAsBytesSync(),
      );
    ShellWallpaper revision() {
      final stat = file.statSync();
      return ShellWallpaper(
        path: file.path,
        name: 'replaced.png',
        sizeBytes: stat.size,
        modified: stat.modified,
      );
    }

    final entries = [revision()];
    final controller = _WallpaperController(entries)
      ..selectPanel(ShellPanel.wallpapers);
    addTearDown(controller.dispose);
    List<int>? previousPixels;
    await tester.runAsync(() async {
      await _mount(tester, controller, const Size(800, 600), settle: false);
      previousPixels = await _thumbnailPixels(tester);
    });
    await tester.pumpAndSettle();
    expect(previousPixels, [0, 0, 0, 255]);
    final originalRevision = entries.single;
    file.writeAsBytesSync(
      base64Decode(
        'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII=',
      ),
    );
    file.setLastModifiedSync(
      originalRevision.modified!.add(const Duration(seconds: 2)),
    );
    entries[0] = revision();
    expect(entries.single.sizeBytes, originalRevision.sizeBytes);
    List<int>? replacementPixels;
    await tester.runAsync(() async {
      await tester.tap(find.byTooltip('刷新壁纸'));
      await tester.pump();
      replacementPixels = await _thumbnailPixels(tester);
    });
    await tester.pumpAndSettle();
    expect(replacementPixels, [255, 255, 255, 255]);
    expect(replacementPixels, isNot(previousPixels));
    expect(tester.takeException(), isNull);
  });

  testWidgets('unavailable wallpaper service permits browsing but no apply', (
    tester,
  ) async {
    final entries = wallpaperEntries();
    final controller = _LiveWallpaperController(entries)..available = false;
    controller.selectPanel(ShellPanel.wallpapers);
    addTearDown(controller.dispose);
    await _mount(tester, controller, const Size(1280, 900));
    await tester.tap(find.byKey(ValueKey('wallpaper-${entries.first.path}')));
    expect(controller.applied, isEmpty);
    expect(find.text('壁纸服务不可用，当前可浏览图片'), findsOneWidget);
  });

  testWidgets('directory errors are distinct from an empty wallpaper catalog', (
    tester,
  ) async {
    final controller = _WallpaperController([])..failure = '没有读取权限';
    controller.selectPanel(ShellPanel.wallpapers);
    addTearDown(controller.dispose);
    await _mount(tester, controller, const Size(1280, 720));
    expect(find.text('没有读取权限'), findsOneWidget);
    expect(find.text('暂时无法读取壁纸'), findsOneWidget);
    expect(find.text('文件夹里还没有可用壁纸'), findsNothing);
  });

  for (final size in [const Size(180, 720), const Size(320, 240)]) {
    testWidgets('wallpaper gallery scrolls populated content at $size', (
      tester,
    ) async {
      final controller = _WallpaperController(wallpaperEntries())
        ..selectPanel(ShellPanel.wallpapers);
      addTearDown(controller.dispose);
      await _mount(tester, controller, size);
      await tester.enterText(
        find.byKey(const ValueKey('wallpaper-search')),
        '城市',
      );
      await tester.pump();
      expect(tester.takeException(), isNull);
    });
  }

  for (final size in [
    const Size(180, 720),
    const Size(320, 240),
    const Size(1280, 720),
  ]) {
    for (final panel in ShellPanel.values) {
      testWidgets('$panel fits ${size.width} × ${size.height}', (tester) async {
        final controller = _Controller();
        if (panel != ShellPanel.none) controller.selectPanel(panel);
        addTearDown(controller.dispose);
        await _mount(tester, controller, size);
        expect(tester.takeException(), isNull);
      });
    }
  }

  testWidgets('optional local preview captures actual Material widgets', (
    tester,
  ) async {
    final destination = Platform.environment['CLARION_SHELL_PREVIEW_DIR'];
    if (destination == null) return;
    final font = File(
      Platform.environment['CLARION_SHELL_PREVIEW_FONT'] ??
          '/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc',
    );
    if (font.existsSync()) {
      await tester.runAsync(() async {
        final bytes = await font.readAsBytes();
        final loader = FontLoader('Roboto')
          ..addFont(Future.value(ByteData.sublistView(bytes)));
        await loader.load();
      });
    }
    // Flutter's test binding substitutes fonts by default. Preview artifacts
    // explicitly load the same bundled Material font as the Linux application.
    final icons = File(
      'build/unit_test_assets/fonts/MaterialIcons-Regular.otf',
    );
    if (icons.existsSync()) {
      await tester.runAsync(() async {
        final bytes = await icons.readAsBytes();
        final loader = FontLoader('MaterialIcons')
          ..addFont(Future.value(ByteData.sublistView(bytes)));
        await loader.load();
      });
    }
    final controller = _Controller()..selectPanel(ShellPanel.quickSettings);
    addTearDown(controller.dispose);
    await _mount(tester, controller, const Size(1440, 900), preview: true);
    for (final panel in [
      ShellPanel.quickSettings,
      ShellPanel.launcher,
      ShellPanel.notifications,
    ]) {
      if (controller.panel != panel) controller.selectPanel(panel);
      await tester.pumpAndSettle();
      final boundary = tester.renderObject<RenderRepaintBoundary>(
        find.byKey(const ValueKey('capture')),
      );
      await tester.runAsync(() async {
        final image = await boundary.toImage(pixelRatio: 1);
        final bytes = await image.toByteData(format: ui.ImageByteFormat.png);
        await Directory(destination).create(recursive: true);
        await File(
          '$destination/${panel.name}.png',
        ).writeAsBytes(bytes!.buffer.asUint8List());
        image.dispose();
      });
    }
  });
}
