// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:convert';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:clarion_shell/mac_launchpad_layout.dart';
import 'package:clarion_shell/mac_shell_preferences.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/ui/app_icon.dart';
import 'package:clarion_shell/ui/mac_app_icon.dart';
import 'package:clarion_shell/ui/mac_dock_view.dart';
import 'package:clarion_shell/ui/mac_launcher_panel.dart';

const _channel = MethodChannel('clarion.shell/window');
const _iconName = 'clarion-native-sharpness-checker';
const _app = DesktopApp(
  id: 'native-sharpness.desktop',
  name: 'Native Sharpness',
  desktopFile: '',
  iconName: _iconName,
);
const _files = DesktopApp(
  id: 'org.kde.dolphin.desktop',
  name: 'Files',
  desktopFile: '',
  iconName: _iconName,
);
const _terminal = DesktopApp(
  id: 'Alacritty.desktop',
  name: 'Terminal',
  desktopFile: '',
  iconName: _iconName,
);
const _roles = [_files, _terminal];
const _roleAssets = {
  'org.kde.dolphin.desktop': 'assets/macos/finder.png',
  'Alacritty.desktop': 'assets/macos/terminal.png',
};

class _Controller extends ShellController {
  _Controller({List<DesktopApp> catalog = const [_app]})
    : _catalog = List.unmodifiable(catalog),
      super(demo: true, tray: false, notifications: false);
  final List<DesktopApp> _catalog;
  @override
  List<DesktopApp> get apps => _catalog;
  @override
  List<DockApp> get dockApps => [
    for (final app in _catalog) DockApp(app: app, isPinned: true),
  ];
}

Future<void> _wait(
  WidgetTester tester,
  bool Function() ready,
  String reason,
) async {
  final deadline = DateTime.now().add(const Duration(seconds: 10));
  while (!ready()) {
    if (DateTime.now().isAfter(deadline)) fail('Timed out: $reason');
    await tester.pump(const Duration(milliseconds: 16));
  }
}

Future<void> _png(File file, int extent, {required bool checker}) async {
  await file.parent.create(recursive: true);
  final recorder = ui.PictureRecorder();
  final canvas = ui.Canvas(recorder);
  final paint = ui.Paint()..isAntiAlias = false;
  for (var y = 0; y < extent; y++) {
    for (var x = 0; x < extent; x++) {
      paint.color = checker
          ? (x + y).isEven
                ? const ui.Color(0xffffffff)
                : const ui.Color(0xff000000)
          : const ui.Color(0xff808080);
      canvas.drawRect(
        ui.Rect.fromLTWH(x.toDouble(), y.toDouble(), 1, 1),
        paint,
      );
    }
  }
  final picture = recorder.endRecording();
  final image = await picture.toImage(extent, extent);
  try {
    final data = await image.toByteData(format: ui.ImageByteFormat.png);
    await file.writeAsBytes(data!.buffer.asUint8List());
  } finally {
    image.dispose();
    picture.dispose();
  }
}

Future<String> _theme() async {
  final root = '${Platform.environment['XDG_DATA_HOME']}/icons';
  final theme = Directory('$root/hicolor');
  await theme.create(recursive: true);
  await File('${theme.path}/index.theme').writeAsString('''
[Icon Theme]
Name=Native sharpness fixture
Directories=48x48/apps,256x256/apps

[48x48/apps]
Size=48
Context=Applications
Type=Fixed

[256x256/apps]
Size=256
Context=Applications
Type=Fixed
''');
  await _png(
    File('${theme.path}/48x48/apps/$_iconName.png'),
    48,
    checker: false,
  );
  await _png(
    File('${theme.path}/256x256/apps/$_iconName.png'),
    256,
    checker: true,
  );
  return root;
}

Finder _raw(Finder parent) =>
    find.descendant(of: parent, matching: find.byType(RawImage));

Future<ui.Image> _decoded(WidgetTester tester, Finder parent, int size) async {
  final raw = _raw(parent);
  await _wait(
    tester,
    () =>
        raw.evaluate().length == 1 &&
        tester.widget<RawImage>(raw).image != null,
    'real AppIcon decode at $size pixels',
  );
  final image = tester.widget<RawImage>(raw).image!;
  expect(image.width, size);
  expect(image.height, size);
  expect(tester.widget<RawImage>(raw).filterQuality, FilterQuality.medium);
  return image;
}

Widget _screen(Widget child) => MaterialApp(
  theme: ThemeData.dark(),
  home: Scaffold(body: Center(child: child)),
);

Map<String, Object> _source(
  WidgetTester tester,
  Finder parent, {
  required int pixels,
  required String? originalAsset,
}) {
  final image = tester.widget<Image>(
    find.descendant(of: parent, matching: find.byType(Image)),
  );
  final resize = image.image as ResizeImage;
  expect(resize.width, pixels);
  expect(resize.height, pixels);
  expect(resize.policy, ResizeImagePolicy.fit);
  expect(resize.allowUpscaling, isFalse);
  expect(image.gaplessPlayback, isTrue);
  expect(image.filterQuality, FilterQuality.medium);
  final provider = resize.imageProvider;
  if (originalAsset != null) {
    expect(provider, isA<AssetImage>());
    expect((provider as AssetImage).assetName, originalAsset);
    return {'kind': 'AssetImage', 'path': originalAsset};
  }
  expect(provider, isA<FileImage>());
  final path = (provider as FileImage).file.path;
  expect(path, endsWith('/256x256/apps/$_iconName.png'));
  expect(path, startsWith(Platform.environment['XDG_DATA_HOME']!));
  return {'kind': 'FileImage', 'path': path};
}

Map<String, double> _rect(Rect rect) => {
  'left': rect.left,
  'top': rect.top,
  'width': rect.width,
  'height': rect.height,
};

Future<void> _capture(GlobalKey key, String name) async {
  final boundary =
      key.currentContext!.findRenderObject()! as RenderRepaintBoundary;
  final image = await boundary.toImage(pixelRatio: 2);
  try {
    final png = await image.toByteData(format: ui.ImageByteFormat.png);
    await File(
      '${Platform.environment['CLARION_SHELL_CAPTURE_DIR']}/$name.png',
    ).writeAsBytes(png!.buffer.asUint8List());
  } finally {
    image.dispose();
  }
}

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.onlyPumps;
  testWidgets('native icon resolution keeps Dock and Launchpad textures sharp', (
    tester,
  ) async {
    expect(Platform.environment['CLARION_SHELL_ISOLATED_TEST'], '1');
    final root = Platform.environment['CLARION_SHELL_TEST_ROOT']!;
    for (final (child, parent) in [
      ('WAYLAND_DISPLAY', 'CLARION_SHELL_PARENT_WAYLAND_DISPLAY'),
      ('CLARION_SOCKET', 'CLARION_SHELL_PARENT_SOCKET'),
      ('DBUS_SESSION_BUS_ADDRESS', 'CLARION_SHELL_PARENT_DBUS_ADDRESS'),
    ]) {
      expect(Platform.environment[child], isNotEmpty);
      expect(Platform.environment[child], isNot(Platform.environment[parent]));
    }
    for (final key in [
      'XDG_CONFIG_HOME',
      'XDG_DATA_HOME',
      'XDG_CACHE_HOME',
      'XDG_STATE_HOME',
    ]) {
      expect(Platform.environment[key]!.startsWith('$root/'), isTrue);
    }
    final evidence = <String, Object?>{
      'passed': false,
      'scope':
          'Production GTK appIcon, shared AppIcon decode and rendered checker '
          'pixels, actual MacDockView hover and MacLauncherPanel. Nested '
          'Wayland output at scale 2, private XDG and D-Bus. Demo controller '
          'never starts services or launches apps. The native main Material '
          'fullscreen layer hosts these isolated macOS widgets; separate '
          'macOS Dock/Launchpad native surfaces and multi-view lifecycle are '
          'not certified. Pointer input is injected '
          'into the isolated Flutter binding; physical compositor input and '
          'frame-time performance are not certified.',
    };
    final controller = _Controller()
      ..setShellStyle(ShellStyle.macos)
      ..setReduceMotion(false)
      ..setMacPreferences(
        const MacShellPreferences(
          dockSize: 64,
          magnificationScale: 2,
          showLaunchpad: false,
          showMissionControl: false,
          showDownloads: false,
        ),
      );
    final layout = MacLaunchpadLayout(persist: false);
    final roleController = _Controller(catalog: _roles)
      ..setShellStyle(ShellStyle.macos)
      ..setReduceMotion(false)
      ..setMacPreferences(
        const MacShellPreferences(
          dockSize: 64,
          magnificationScale: 2,
          showLaunchpad: false,
          showMissionControl: false,
          showDownloads: false,
          useOriginalIcons: true,
        ),
      );
    final roleLayout = MacLaunchpadLayout(persist: false);
    TestGesture? mouse;
    try {
      final themePath = await _theme();
      // No mock channel is installed: these calls cross the production GTK
      // handler, including its scale-1 physical-pixel lookup and theme isolation.
      final nativeQueries = <Map<String, Object?>>[];
      for (final pixels in [48, 192, 256]) {
        final arguments = {
          'name': _iconName,
          'size': 48,
          'pixelSize': pixels,
          'themePath': themePath,
        };
        final path = await _channel.invokeMethod<String>('appIcon', arguments);
        expect(
          path,
          '$themePath/hicolor/${pixels == 48 ? 48 : 256}x'
          '${pixels == 48 ? 48 : 256}/apps/$_iconName.png',
        );
        nativeQueries.add({'arguments': arguments, 'resolvedPath': path});
      }
      evidence['nativeQueries'] = nativeQueries;
      await _channel.invokeMethod<void>('setExpanded', true);
      await tester.pumpWidget(_screen(const SizedBox.shrink()));
      final nativeState = await _channel.invokeMapMethod<String, dynamic>(
        'windowState',
      );
      final logicalHeight =
          ((nativeState!['monitorGeometry'] as Map)['height'] as num)
              .toDouble();
      await _wait(
        tester,
        () =>
            (tester.view.physicalSize.height / tester.view.devicePixelRatio -
                    logicalHeight)
                .abs() <
            1,
        'private native fullscreen layer allocation',
      );
      expect(tester.view.devicePixelRatio, 2);
      evidence['devicePixelRatio'] = tester.view.devicePixelRatio;
      evidence['nativeState'] = await _channel.invokeMapMethod<String, dynamic>(
        'windowState',
      );

      // At maximum Dock magnification the 48-logical-pixel child paints as
      // 128 logical / 256 physical pixels. A 48px texture would lose this
      // single-physical-pixel checker; test the actual rendered output, too.
      final checkerKey = GlobalKey();
      await tester.pumpWidget(
        _screen(
          RepaintBoundary(
            key: checkerKey,
            child: const SizedBox.square(
              dimension: 128,
              child: FittedBox(
                child: AppIcon(app: _app, size: 48, rasterSize: 128),
              ),
            ),
          ),
        ),
      );
      final checkerIcon = find.byType(AppIcon);
      await _decoded(tester, checkerIcon, 256);
      await tester.pump();
      final image =
          await (checkerKey.currentContext!.findRenderObject()!
                  as RenderRepaintBoundary)
              .toImage(pixelRatio: 2);
      try {
        expect(image.width, 256);
        final bytes = (await image.toByteData(
          format: ui.ImageByteFormat.rawRgba,
        ))!.buffer.asUint8List();
        var contrast = 0.0;
        var samples = 0;
        for (var y = 16; y < 240; y += 7) {
          for (var x = 16; x < 239; x++) {
            contrast +=
                (bytes[(y * 256 + x) * 4] - bytes[(y * 256 + x + 1) * 4]).abs();
            samples++;
          }
        }
        final mean = contrast / samples;
        expect(
          mean,
          greaterThan(200),
          reason: 'Native high-res checker must retain physical pixel detail',
        );
        evidence['renderedCheckerMeanAdjacentContrast'] = mean;
        final png = await image.toByteData(format: ui.ImageByteFormat.png);
        await File(
          '${Platform.environment['CLARION_SHELL_CAPTURE_DIR']}/'
          'native-256-checker.png',
        ).writeAsBytes(png!.buffer.asUint8List());
      } finally {
        image.dispose();
      }

      await tester.pumpWidget(
        _screen(
          SizedBox(
            width: 1000,
            height: MacDockView.surfaceThickness(controller.macPreferences),
            child: MacDockView(
              controller: controller,
              availableWidth: 1000,
              availableHeight: 700,
            ),
          ),
        ),
      );
      final dockApp = find.byKey(
        const ValueKey('dock-app-native-sharpness.desktop'),
      );
      final dockIcon = find.descendant(
        of: dockApp,
        matching: find.byType(AppIcon),
      );
      expect(tester.widget<AppIcon>(dockIcon).rasterSize, 128);
      final dockTexture = await _decoded(tester, dockApp, 256);
      mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
      await mouse.addPointer(location: const Offset(1, 1));
      for (var move = 0; move < 4; move++) {
        await mouse.moveTo(tester.getCenter(dockApp));
        await tester.pump(const Duration(milliseconds: 160));
      }
      final scale = tester
          .widget<Transform>(
            find.byKey(const ValueKey('dock-scale-Native Sharpness')),
          )
          .transform
          .entry(0, 0);
      expect(scale, greaterThan(1.9));
      expect(
        identical(tester.widget<RawImage>(_raw(dockApp)).image, dockTexture),
        isTrue,
        reason: 'Hover must retain its already decoded texture',
      );
      await mouse.moveTo(const Offset(1, 1));
      await tester.pump(const Duration(milliseconds: 160));
      expect(
        identical(tester.widget<RawImage>(_raw(dockApp)).image, dockTexture),
        isTrue,
      );
      await mouse.removePointer();
      mouse = null;
      evidence['dock'] = {
        'logicalBase': 64,
        'magnifiedLogical': 128,
        'decodedPixels': dockTexture.width,
        'hoverScale': scale,
        'sameTextureAfterHover': true,
      };

      await tester.pumpWidget(
        _screen(
          SizedBox(
            width: 1000,
            height: 1000,
            child: MacLauncherPanel(controller: controller, layout: layout),
          ),
        ),
      );
      await _wait(
        tester,
        () => find.byType(AppIcon).evaluate().length == 1,
        'actual Launchpad tile',
      );
      final launcherIcon = tester.widget<AppIcon>(find.byType(AppIcon));
      expect(launcherIcon.size, inInclusiveRange(24, 72));
      final launcherTexture = await _decoded(
        tester,
        find.byType(AppIcon),
        (launcherIcon.size * 2).ceil(),
      );
      evidence['launchpad'] = {
        'logicalSize': launcherIcon.size,
        'decodedPixels': launcherTexture.width,
      };
      // Also cover the shared widget at a larger 96-logical-pixel budget;
      // current Launchpad grid intentionally caps its actual tiles at 72.
      await tester.pumpWidget(_screen(const AppIcon(app: _app, size: 96)));
      final largeTexture = await _decoded(tester, find.byType(AppIcon), 192);
      evidence['sharedAppIcon96Logical'] = {
        'decodedPixels': largeTexture.width,
      };

      // Stable desktop IDs must select the same verified original resources
      // on each surface. Both fixture entries deliberately advertise the GTK
      // checker icon, so opting out proves the real native resolver is used.
      final roleDockCapture = GlobalKey();
      await tester.pumpWidget(
        _screen(
          RepaintBoundary(
            key: roleDockCapture,
            child: SizedBox(
              width: 1000,
              height: MacDockView.surfaceThickness(
                roleController.macPreferences,
              ),
              child: MacDockView(
                controller: roleController,
                availableWidth: 1000,
                availableHeight: 700,
              ),
            ),
          ),
        ),
      );
      await tester.pump(const Duration(milliseconds: 160));
      final roleDockRects = {
        for (final app in _roles)
          app.id: tester.getRect(
            find.byKey(ValueKey('dock-icon-size-${app.id}')),
          ),
      };
      final dockFocus = FocusManager.instance.primaryFocus;
      final focusedWindow = roleController.focusedWindow?.id;
      final initialPanel = roleController.panel;
      final roleDockPhases = <Map<String, Object?>>[];
      for (final original in [true, false, true]) {
        roleController.setMacPreferences(
          roleController.macPreferences.copyWith(useOriginalIcons: original),
        );
        await tester.pump(const Duration(milliseconds: 160));
        final rows = <Map<String, Object?>>[];
        for (final app in _roles) {
          final slot = find.byKey(ValueKey('dock-app-${app.id}'));
          final icon = find.descendant(
            of: slot,
            matching: find.byType(MacAppIcon),
          );
          final widget = tester.widget<MacAppIcon>(icon);
          expect(widget.useOriginalIcons, original);
          expect(widget.size, 48);
          expect(widget.rasterSize, 128);
          final texture = await _decoded(tester, slot, 256);
          final rect = tester.getRect(
            find.byKey(ValueKey('dock-icon-size-${app.id}')),
          );
          expect(rect, roleDockRects[app.id]);
          expect(rect.size, const Size(64, 64));
          rows.add({
            'appId': app.id,
            'source': _source(
              tester,
              slot,
              pixels: 256,
              originalAsset: original ? _roleAssets[app.id] : null,
            ),
            'logicalBase': 64,
            'maximumMagnifiedLogical': 128,
            'decodedPixels': texture.width,
            'unchangedRect': _rect(rect),
          });
        }
        expect(FocusManager.instance.primaryFocus, same(dockFocus));
        expect(roleController.focusedWindow?.id, focusedWindow);
        expect(roleController.panel, initialPanel);
        roleDockPhases.add({'useOriginalIcons': original, 'icons': rows});
      }
      await _capture(roleDockCapture, 'original-role-dock');
      evidence['originalRoleDock'] = {
        'toggleSequence': roleDockPhases,
        'focusAndGeometryRetained': true,
        'capture': 'original-role-dock.png',
      };

      final roleLaunchpadCapture = GlobalKey();
      await tester.pumpWidget(
        _screen(
          RepaintBoundary(
            key: roleLaunchpadCapture,
            child: SizedBox(
              width: 1000,
              height: 1000,
              child: MacLauncherPanel(
                controller: roleController,
                layout: roleLayout,
              ),
            ),
          ),
        ),
      );
      await _wait(
        tester,
        () => find.byType(MacAppIcon).evaluate().length == _roles.length,
        'both original-role Launchpad tiles',
      );
      await tester.pump();
      final launcherFocus = FocusManager.instance.primaryFocus;
      final roleLaunchpadRects = {
        for (final app in _roles)
          app.id: tester.getRect(find.byKey(ValueKey('app-${app.id}'))),
      };
      final roleLaunchpadPhases = <Map<String, Object?>>[];
      for (final original in [true, false, true]) {
        roleController.setMacPreferences(
          roleController.macPreferences.copyWith(useOriginalIcons: original),
        );
        await tester.pump(const Duration(milliseconds: 16));
        final rows = <Map<String, Object?>>[];
        for (final app in _roles) {
          final tile = find.byKey(ValueKey('app-${app.id}'));
          final icon = find.descendant(
            of: tile,
            matching: find.byType(MacAppIcon),
          );
          final widget = tester.widget<MacAppIcon>(icon);
          expect(widget.useOriginalIcons, original);
          expect(widget.size, 72);
          final texture = await _decoded(tester, tile, 144);
          final rect = tester.getRect(tile);
          expect(rect, roleLaunchpadRects[app.id]);
          rows.add({
            'appId': app.id,
            'source': _source(
              tester,
              tile,
              pixels: 144,
              originalAsset: original ? _roleAssets[app.id] : null,
            ),
            'logicalSize': widget.size,
            'decodedPixels': texture.width,
            'unchangedRect': _rect(rect),
          });
        }
        expect(FocusManager.instance.primaryFocus, same(launcherFocus));
        expect(roleController.focusedWindow?.id, focusedWindow);
        expect(roleController.panel, initialPanel);
        roleLaunchpadPhases.add({'useOriginalIcons': original, 'icons': rows});
      }
      await _capture(roleLaunchpadCapture, 'original-role-launchpad');
      evidence['originalRoleLaunchpad'] = {
        'toggleSequence': roleLaunchpadPhases,
        'focusAndGeometryRetained': true,
        'capture': 'original-role-launchpad.png',
      };

      // Direct layout grouping avoids long-press CPU timing and never invokes
      // an application's onTap. Folder thumbnails use these same two sources.
      roleLayout.group(_terminal.id, _files.id);
      await _wait(
        tester,
        () =>
            roleLayout.entries.length == 1 &&
            find.byType(MacAppIcon).evaluate().length == _roles.length,
        'original-role folder thumbnails',
      );
      await tester.pump();
      final folder = roleLayout.entries.single;
      expect(folder.isFolder, isTrue);
      final folderPreview = find.byKey(
        ValueKey('mac-launchpad-folder-icon-${folder.id}'),
      );
      expect(folderPreview, findsOneWidget);
      final folderRows = <Map<String, Object?>>[];
      for (final app in _roles) {
        final icon = find.descendant(
          of: folderPreview,
          matching: find.byWidgetPredicate(
            (widget) => widget is MacAppIcon && widget.app.id == app.id,
          ),
        );
        final widget = tester.widget<MacAppIcon>(icon);
        expect(widget.useOriginalIcons, isTrue);
        expect(widget.size, 18);
        final texture = await _decoded(tester, icon, 36);
        folderRows.add({
          'appId': app.id,
          'source': _source(
            tester,
            icon,
            pixels: 36,
            originalAsset: _roleAssets[app.id],
          ),
          'logicalSize': widget.size,
          'decodedPixels': texture.width,
        });
      }
      expect(roleController.focusedWindow?.id, focusedWindow);
      expect(roleController.panel, initialPanel);
      evidence['originalRoleFolder'] = {
        'icons': folderRows,
        'directPrivateLayoutGrouping': true,
        'persist': roleLayout.persist,
      };
      expect(tester.takeException(), isNull);
      evidence['passed'] = true;
    } finally {
      if (mouse != null) await mouse.removePointer();
      await tester.pumpWidget(const SizedBox.shrink());
      layout.dispose();
      controller.dispose();
      roleLayout.dispose();
      roleController.dispose();
      await _channel.invokeMethod<void>('setExpanded', false);
      await File(
        '${Platform.environment['CLARION_SHELL_CAPTURE_DIR']}/'
        'app-icon-native-evidence.json',
      ).writeAsString(
        '${const JsonEncoder.withIndent('  ').convert(evidence)}\n',
      );
    }
  });
}
