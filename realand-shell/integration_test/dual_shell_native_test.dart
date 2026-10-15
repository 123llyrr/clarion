// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:convert';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:realand_shell/main.dart';
import 'package:realand_shell/models.dart';
import 'package:realand_shell/preferences.dart';
import 'package:realand_shell/services/desktop_apps.dart';
import 'package:realand_shell/services/power_profiles_service.dart';
import 'package:realand_shell/services/realand_ipc.dart';
import 'package:realand_shell/services/system_service.dart';
import 'package:realand_shell/services/wallpaper_service.dart';
import 'package:realand_shell/state/shell_controller.dart';
import 'package:realand_shell/ui/mac_dock_view.dart';

const _window = MethodChannel('realand.shell/window');
const _pins = [
  'dual-shell-fixture-0',
  'dual-shell-fixture-1',
  'dual-shell-fixture-2',
  'dual-shell-fixture-3',
  'dual-shell-fixture-4',
  'dual-shell-fixture-5',
  'dual-shell-fixture-6',
  'dual-shell-fixture-7',
  'dual-shell-fixture-8',
  'dual-shell-fixture-9',
  'dual-shell-fixture-10',
  'dual-shell-fixture-11',
];

// The SDK's live test binding transforms device events with firstWhere on its
// RenderViews. A newly added native view can receive an enter/leave packet
// before ShellHost attaches its RenderView (and after cleanup detaches it).
// Production RendererBinding.hitTestInView uses a nullable lookup instead.
// flutter test initializes the integration binding before this file's main,
// so filter its platform packet callback instead of constructing a second
// binding. Tester-injected gestures bypass platform data packets entirely.
class _UnattachedDevicePacketFilter {
  _UnattachedDevicePacketFilter(this.binding) {
    _original = binding.platformDispatcher.onPointerDataPacket;
    binding.platformDispatcher.onPointerDataPacket = _handlePacket;
  }

  final IntegrationTestWidgetsFlutterBinding binding;
  ui.PointerDataPacketCallback? _original;
  final unattachedDeviceEvents = <Map<String, dynamic>>[];

  void _handlePacket(ui.PointerDataPacket packet) {
    final attached = binding.renderViews
        .map((view) => view.flutterView.viewId)
        .toSet();
    final valid = <ui.PointerData>[];
    for (final data in packet.data) {
      if (attached.contains(data.viewId)) {
        valid.add(data);
      } else {
        unattachedDeviceEvents.add({
          'viewId': data.viewId,
          'change': data.change.name,
          'x': data.physicalX,
          'y': data.physicalY,
        });
      }
    }
    if (valid.isNotEmpty) _original?.call(ui.PointerDataPacket(data: valid));
  }

  void restore() {
    binding.platformDispatcher.onPointerDataPacket = _original;
  }
}

class _Apps extends DesktopApps {
  int loads = 0;

  @override
  Future<List<DesktopApp>> load() async {
    loads++;
    return [
      for (var index = 0; index < 24; index++)
        DesktopApp(
          id: 'dual-shell-fixture-$index',
          name: '隔离应用 ${index + 1}',
          desktopFile: '',
          iconName: 'utilities-terminal',
        ),
    ];
  }
}

class _Ipc extends RealandIpc {
  int starts = 0;
  int disposals = 0;

  @override
  void start() {
    starts++;
    super.start();
  }

  @override
  void dispose() {
    disposals++;
    super.dispose();
  }
}

class _Power extends PowerProfilesService {
  _Power() : super(demo: true);
  int starts = 0;
  int disposals = 0;

  @override
  Future<void> start() {
    starts++;
    return super.start();
  }

  @override
  void dispose() {
    disposals++;
    super.dispose();
  }
}

class _System extends SystemService {
  int homeFolderOpens = 0;
  int trashOpens = 0;

  @override
  Future<void> openHomeFolder() async => homeFolderOpens++;

  @override
  Future<void> openTrash() async => trashOpens++;

  @override
  Future<SystemSnapshot> read() async => const SystemSnapshot(
    volume: .6,
    brightness: .7,
    batteryFraction: .8,
    wifiEnabled: true,
    bluetoothEnabled: true,
    networkName: '隔离测试网络',
  );
}

Map<String, double> _rectEvidence(Rect rect) => {
  'x': rect.left,
  'y': rect.top,
  'width': rect.width,
  'height': rect.height,
};

Future<Map<String, dynamic>> _assetPixels(String path) async {
  final bytes = await rootBundle.load(path);
  final codec = await ui.instantiateImageCodec(
    bytes.buffer.asUint8List(bytes.offsetInBytes, bytes.lengthInBytes),
  );
  final image = (await codec.getNextFrame()).image;
  try {
    final pixels = (await image.toByteData(
      format: ui.ImageByteFormat.rawRgba,
    ))!;
    var visiblePixels = 0;
    for (var index = 3; index < pixels.lengthInBytes; index += 4) {
      if (pixels.getUint8(index) != 0) visiblePixels++;
    }
    expect(image.width, greaterThan(1), reason: path);
    expect(image.height, greaterThan(1), reason: path);
    expect(visiblePixels, greaterThan(0), reason: '$path has sprite pixels');
    return {
      'width': image.width,
      'height': image.height,
      'nontransparentPixels': visiblePixels,
    };
  } finally {
    image.dispose();
    codec.dispose();
  }
}

Future<Map<String, dynamic>> _capturePixels(
  File capture,
  Size logicalOutput,
  Map<String, Rect> sprites,
) async {
  final codec = await ui.instantiateImageCodec(await capture.readAsBytes());
  final image = (await codec.getNextFrame()).image;
  try {
    final pixels = (await image.toByteData(
      format: ui.ImageByteFormat.rawRgba,
    ))!;
    final scaleX = image.width / logicalOutput.width;
    final scaleY = image.height / logicalOutput.height;
    final evidence = <String, dynamic>{};
    for (final entry in sprites.entries) {
      final rect = Rect.fromLTRB(
        entry.value.left * scaleX,
        entry.value.top * scaleY,
        entry.value.right * scaleX,
        entry.value.bottom * scaleY,
      );
      expect(rect.left, greaterThanOrEqualTo(0));
      expect(rect.top, greaterThanOrEqualTo(0));
      expect(rect.right, lessThanOrEqualTo(image.width));
      expect(rect.bottom, lessThanOrEqualTo(image.height));
      var brightPixels = 0;
      var minimum = 255;
      var maximum = 0;
      for (var y = rect.top.ceil(); y < rect.bottom.floor(); y++) {
        for (var x = rect.left.ceil(); x < rect.right.floor(); x++) {
          final offset = (y * image.width + x) * 4;
          final light =
              (pixels.getUint8(offset) +
                  pixels.getUint8(offset + 1) +
                  pixels.getUint8(offset + 2)) ~/
              3;
          if (light >= 150) brightPixels++;
          if (light < minimum) minimum = light;
          if (light > maximum) maximum = light;
        }
      }
      // These three source sprites contain light pixels. On the fixture's
      // dark glass/background an empty Image widget cannot satisfy either
      // check. Inspect actual grim output, after ImageStreams have decoded.
      expect(brightPixels, greaterThan(4), reason: '${entry.key} is painted');
      expect(maximum - minimum, greaterThan(30), reason: entry.key);
      evidence[entry.key] = {
        'physicalRect': _rectEvidence(rect),
        'brightPixels': brightPixels,
        'brightnessRange': maximum - minimum,
      };
    }
    return evidence;
  } finally {
    image.dispose();
    codec.dispose();
  }
}

Future<Map<String, dynamic>> _state() async => Map<String, dynamic>.from(
  (await _window.invokeMapMethod<String, dynamic>('windowState'))!,
);

Future<void> _until(
  WidgetTester tester,
  bool Function() ready,
  String reason,
) async {
  final deadline = DateTime.now().add(const Duration(seconds: 15));
  while (!ready()) {
    if (DateTime.now().isAfter(deadline)) fail('Timed out: $reason');
    await tester.pump(const Duration(milliseconds: 20));
  }
}

Future<Map<String, dynamic>> _native(
  WidgetTester tester, {
  required ShellStyle style,
  BarPosition position = BarPosition.bottom,
  bool expanded = false,
  int? dockViewId,
}) async {
  final deadline = DateTime.now().add(const Duration(seconds: 15));
  Map<String, dynamic> state = {};
  do {
    await tester.pump(const Duration(milliseconds: 20));
    state = await _state();
    final macos = style == ShellStyle.macos;
    final barHeight = macos ? 22 : 40;
    final geometry = state['monitorGeometry'] as Map;
    if (state['style'] != style.name ||
        state['expanded'] != expanded ||
        state['barPosition'] != (macos ? 'top' : position.name) ||
        state['height'] != (expanded ? geometry['height'] : barHeight) ||
        state['tooltipVisible'] != false ||
        (macos &&
            (state['dockVisible'] != true ||
                (state['dock'] as Map?)?['height'] != 120))) {
      continue;
    }
    final currentDock = state['dock'] as Map?;
    if (macos && !expanded && currentDock != null) {
      final glass = find.byKey(const ValueKey('mac-dock-glass'));
      if (glass.evaluate().isEmpty) continue;
      final rect = tester.getRect(glass);
      // GTK receives integer regions after the animated glass reports its
      // current bounds. Wait for that asynchronous geometry request as well
      // as the surface configuration; a stale rectangle must not pass.
      if (((currentDock['inputX'] as num) - rect.left).abs() > 1.01 ||
          ((currentDock['inputWidth'] as num) - rect.width).abs() > 1.01) {
        continue;
      }
    }
    expect(state['preview'], isFalse);
    expect(state['mainViewId'], isNotNull);
    expect(state['keyboard'], expanded ? 'Exclusive' : 'None');
    expect(state['layer'], expanded ? 'Overlay' : 'Top');
    expect(state['exclusiveZone'], barHeight);
    expect(state['inputHeight'], expanded ? geometry['height'] : barHeight);
    expect(state['inputY'], 0);
    expect(state['anchorTop'], macos || position == BarPosition.top);
    expect(state['anchorBottom'], !macos && position == BarPosition.bottom);
    if (dockViewId != null) expect(state['dockViewId'], dockViewId);
    final dock = state['dock'] as Map?;
    if (dock != null) {
      expect(state['dockViewId'], isNot(state['mainViewId']));
      expect(dock['ready'], isTrue);
      expect(dock['layer'], 'Top');
      expect(dock['keyboard'], 'None');
      expect(dock['anchorTop'], isFalse);
      expect(dock['anchorBottom'], isTrue);
      expect(dock['exclusiveZone'], macos ? 84 : 0);
      expect(dock['visible'], macos);
      expect(dock['width'], lessThanOrEqualTo((geometry['width'] as int) - 48));
      if (macos && !expanded) {
        expect(dock['inputWidth'], greaterThan(0));
        expect(dock['inputHeight'], greaterThan(0));
        expect(dock['inputX'], greaterThanOrEqualTo(0));
        expect(dock['inputY'], greaterThanOrEqualTo(20));
        expect(
          (dock['inputX'] as int) + (dock['inputWidth'] as int),
          lessThanOrEqualTo(dock['width']),
        );
        expect(
          (dock['inputY'] as int) + (dock['inputHeight'] as int),
          lessThanOrEqualTo(120),
        );
      } else {
        expect(dock['inputWidth'], 0);
        expect(dock['inputHeight'], 0);
      }
    }
    return state;
  } while (DateTime.now().isBefore(deadline));
  fail('Dual shell native surfaces did not settle: $state');
}

Future<void> _persisted(
  WidgetTester tester,
  ShellStyle style,
  bool reduced,
) async {
  final deadline = DateTime.now().add(const Duration(seconds: 15));
  do {
    await tester.pump(const Duration(milliseconds: 20));
    final prefs = await ShellPreferences.load();
    if (prefs.shellStyle == style &&
        prefs.reduceMotion == reduced &&
        prefs.barPosition == BarPosition.bottom &&
        prefs.pinnedAppIds?.join('\u0000') == _pins.join('\u0000')) {
      expect(prefs.themeMode, ThemeMode.dark);
      return;
    }
  } while (DateTime.now().isBefore(deadline));
  fail('Style, motion, original bar edge and pins did not persist');
}

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  final deviceFilter = _UnattachedDevicePacketFilter(binding);
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.onlyPumps;
  testWidgets('native shared-engine Material and macOS switch in place', (
    tester,
  ) async {
    expect(Platform.isLinux, isTrue);
    expect(Platform.environment['REALAND_SHELL_ISOLATED_TEST'], '1');
    for (final pair in [
      ('WAYLAND_DISPLAY', 'REALAND_SHELL_PARENT_WAYLAND_DISPLAY'),
      ('REALAND_SOCKET', 'REALAND_SHELL_PARENT_SOCKET'),
      ('DBUS_SESSION_BUS_ADDRESS', 'REALAND_SHELL_PARENT_DBUS_ADDRESS'),
    ]) {
      expect(Platform.environment[pair.$1], isNotEmpty);
      expect(
        Platform.environment[pair.$1],
        isNot(Platform.environment[pair.$2]),
      );
    }
    final root = Platform.environment['REALAND_SHELL_TEST_ROOT']!;
    for (final variable in [
      'XDG_CONFIG_HOME',
      'XDG_DATA_HOME',
      'XDG_CACHE_HOME',
      'XDG_STATE_HOME',
    ]) {
      expect(Platform.environment[variable]!.startsWith('$root/'), isTrue);
    }
    final captures = Directory(
      Platform.environment['REALAND_SHELL_CAPTURE_DIR']!,
    );
    await captures.create(recursive: true);
    final ipc = _Ipc();
    final apps = _Apps();
    final power = _Power();
    final system = _System();
    final controller = ShellController(
      notifications: false,
      tray: false,
      ipc: ipc,
      desktopApps: apps,
      system: system,
      powerProfiles: power,
      wallpaperService: WallpaperService(
        directoryPath: '$root/wallpapers',
        preferenceFile: File('$root/wallpaper.json'),
      ),
      restoreWallpaperOnStart: false,
    );
    const prefs = ShellPreferences(
      themeMode: ThemeMode.dark,
      reduceMotion: true,
      pinnedAppIds: _pins,
    );
    controller.setThemeMode(prefs.themeMode);
    controller.setReduceMotion(prefs.reduceMotion);
    controller.setPinnedAppIds(_pins);
    final phases = <Map<String, dynamic>>[];
    final evidence = <String, dynamic>{
      'passed': false,
      'input':
          'Pointer events are injected only into this isolated Flutter '
          'binding. Native input bounds are inspected through windowState; '
          'this fixture does not certify physical compositor hit testing.',
      'phases': phases,
    };
    int? dockId;
    Object? mainElement;
    Object? dockElement;
    var observedAppLoads = 0;
    var injectedPointerGestures = 0;
    var macAssetsDecoded = false;

    void identity(String phase) {
      final main = tester.element(find.byType(ShellApplication));
      mainElement ??= main;
      expect(
        identical(mainElement, main),
        isTrue,
        reason: '$phase keeps main view',
      );
      expect(
        tester
            .widget<ShellApplication>(find.byType(ShellApplication))
            .controller,
        same(controller),
      );
      final dock = find.byType(DockApplication);
      if (dock.evaluate().isNotEmpty) {
        final element = tester.element(dock);
        dockElement ??= element;
        expect(
          identical(dockElement, element),
          isTrue,
          reason: '$phase reuses Dock',
        );
        expect(
          tester.widget<DockApplication>(dock).controller,
          same(controller),
        );
      }
      expect(controller.powerProfiles, same(power));
      expect(controller.connected, isTrue);
      expect(ipc.starts, 1);
      expect(ipc.disposals, 0);
      expect(power.starts, 1);
      expect(power.disposals, 0);
      // Opening Launcher deliberately refreshes the catalogue. Styles share
      // this same loader, while the IPC and power service only start once.
      expect(apps.loads, greaterThanOrEqualTo(1));
      expect(apps.loads, greaterThanOrEqualTo(observedAppLoads));
      observedAppLoads = apps.loads;
      expect(controller.pinnedAppIds, _pins);
      expect(
        tester.takeException(),
        isNull,
        reason: '$phase has no layout overflow',
      );
    }

    Future<void> record(String name, Map<String, dynamic> state) async {
      if (!macAssetsDecoded) {
        final context = tester.element(find.byType(ShellApplication));
        final failures = <String>[];
        final manifest = await AssetManifest.loadFromAssetBundle(rootBundle);
        final assets = manifest
            .listAssets()
            .where(
              (path) =>
                  path.startsWith('assets/macos/') && path.endsWith('.png'),
            )
            .toList();
        expect(assets, isNotEmpty);
        await tester.runAsync(
          () => Future.wait([
            for (final path in assets)
              precacheImage(
                AssetImage(path),
                context,
                onError: (error, _) => failures.add('$path: $error'),
              ),
          ]),
        );
        expect(
          failures,
          isEmpty,
          reason: 'Mac PNG ImageStreams decode successfully',
        );
        evidence['decodedMacAssetsBeforeCapture'] = assets;
        evidence['macAssetPixels'] = await tester.runAsync(
          () async => {
            for (final path in assets) path: await _assetPixels(path),
          },
        );
        macAssetsDecoded = true;
      }
      // First pump builds newly mounted image widgets/Tweens; the next one
      // paints decoded images and advances animation from that initial frame.
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 400));
      state = await _state();
      identity(name);
      final utilityBounds = <String, Rect>{};
      if (state['style'] == 'macos' && state['expanded'] == false) {
        final dock = state['dock'] as Map;
        final bounds = Rect.fromLTWH(
          0,
          0,
          (dock['width'] as num).toDouble(),
          (dock['height'] as num).toDouble(),
        );
        final input = Rect.fromLTWH(
          (dock['inputX'] as num).toDouble(),
          (dock['inputY'] as num).toDouble(),
          (dock['inputWidth'] as num).toDouble(),
          (dock['inputHeight'] as num).toDouble(),
        );
        for (final key in ['dock-finder', 'dock-trash']) {
          final finder = find.byKey(ValueKey(key));
          expect(finder, findsOneWidget);
          final rect = tester.getRect(finder);
          expect(rect.isEmpty, isFalse, reason: key);
          expect(
            bounds.inflate(.01).contains(rect.topLeft),
            isTrue,
            reason: key,
          );
          expect(
            bounds.inflate(.01).contains(rect.bottomRight),
            isTrue,
            reason: '$name keeps $key fully visible',
          );
          expect(
            input.contains(rect.center),
            isTrue,
            reason: '$key is hittable',
          );
          expect(
            input.contains(rect.topLeft) && input.contains(rect.bottomRight),
            isTrue,
            reason: '$name keeps the full $key sprite inside native input',
          );
          utilityBounds[key] = rect;
        }
      }
      final capture = '${captures.path}/$name.png';
      final result = await Process.run('grim', [
        '-o',
        Platform.environment['REALAND_SHELL_TEST_OUTPUT'] ?? 'winit',
        capture,
      ]).timeout(const Duration(seconds: 15));
      expect(result.exitCode, 0, reason: '${result.stdout}\n${result.stderr}');
      expect(await File(capture).length(), greaterThan(100));
      if (name == 'macos-menu-and-dock') {
        final monitor = state['monitorGeometry'] as Map;
        final dock = state['dock'] as Map;
        final output = Size(
          (monitor['width'] as num).toDouble(),
          (monitor['height'] as num).toDouble(),
        );
        final dockOrigin = Offset(
          (output.width - (dock['width'] as num)) / 2,
          output.height - (dock['height'] as num),
        );
        evidence['firstMacPaintedSprites'] = await tester.runAsync(
          () => _capturePixels(File(capture), output, {
            'mac-apple-logo': tester.getRect(
              find.byKey(const ValueKey('mac-apple-logo')),
            ),
            for (final entry in utilityBounds.entries)
              entry.key: entry.value.shift(dockOrigin),
          }),
        );
      }
      phases.add({
        'name': name,
        'native': state,
        'capture': capture,
        'appCatalogueLoads': apps.loads,
        if (utilityBounds.isNotEmpty)
          'visibleUtilityBounds': {
            for (final entry in utilityBounds.entries)
              entry.key: _rectEvidence(entry.value),
          },
      });
    }

    try {
      await tester.pumpWidget(
        ShellHost(controller: controller, preferences: prefs, persist: true),
        wrapWithView: false,
      );
      await controller.start();
      await _until(
        tester,
        () => controller.connected && controller.apps.length == 24,
        'private IPC and application fixture',
      );
      expect(apps.loads, 1);
      await record(
        'material-top',
        await _native(
          tester,
          style: ShellStyle.material,
          position: BarPosition.top,
        ),
      );
      controller.setBarPosition(BarPosition.bottom);
      await record(
        'material-bottom',
        await _native(tester, style: ShellStyle.material),
      );
      await _persisted(tester, ShellStyle.material, true);

      controller.setShellStyle(ShellStyle.macos);
      var state = await _native(tester, style: ShellStyle.macos);
      dockId = state['dockViewId'] as int;
      expect(
        binding.platformDispatcher.views.map((view) => view.viewId).toSet(),
        {state['mainViewId'], dockId},
      );
      expect(controller.barPosition, BarPosition.bottom);
      expect(find.byType(MacDockView), findsOneWidget);
      await _until(
        tester,
        () =>
            find.byKey(const ValueKey('mac-dock-glass')).evaluate().isNotEmpty,
        'Dock glass',
      );
      final blurDeadline = DateTime.now().add(const Duration(seconds: 10));
      while ((state['blurAvailable'] != true ||
              state['dockBlurRequested'] != true) &&
          DateTime.now().isBefore(blurDeadline)) {
        await tester.pump(const Duration(milliseconds: 20));
        state = await _state();
      }
      expect(
        state['blurAvailable'],
        isTrue,
        reason: 'GDK asynchronous protocol discovery',
      );
      expect(state['barBlurRequested'], isTrue);
      expect(state['dockBlurRequested'], isTrue);
      evidence['backgroundEffect'] = {
        'protocol': 'ext-background-effect-v1',
        'discovery': 'asynchronous GDK event dispatch',
        'bar': '22 logical pixels',
        'dock': 'visible rounded glass only',
        'wireRequested': true,
      };
      await record('macos-menu-and-dock', state);
      await tester.tap(find.byKey(const ValueKey('dock-finder')));
      injectedPointerGestures++;
      await _until(
        tester,
        () => system.homeFolderOpens == 1,
        'Finder callback',
      );
      await tester.tap(find.byKey(const ValueKey('dock-trash')));
      injectedPointerGestures++;
      await _until(tester, () => system.trashOpens == 1, 'Trash callback');
      evidence['utilityCallbacks'] = {
        'finder': system.homeFolderOpens,
        'trash': system.trashOpens,
        'handler': 'Isolated SystemService override; no external process spawn',
      };
      await _persisted(tester, ShellStyle.macos, true);
      final geometry = state['monitorGeometry'] as Map;
      final dock = state['dock'] as Map;
      final origin = (await _window.invokeMapMethod<String, dynamic>(
        'surfaceOrigin',
        'dock',
      ))!;
      expect(
        origin['x'],
        (geometry['x'] as int) +
            ((geometry['width'] as int) - (dock['width'] as int)) ~/ 2,
      );
      expect(
        origin['y'],
        (geometry['y'] as int) + (geometry['height'] as int) - 120,
      );

      // A dedicated synthetic mouse targets the secondary Flutter view. These
      // injected events bypass compositor hit testing, as stated in evidence.
      controller.setReduceMotion(false);
      final dockView = View.of(tester.element(find.byType(MacDockView)));
      final mouse = TestPointer(700, PointerDeviceKind.mouse, 700);
      Finder firstIcon() =>
          find.byKey(const ValueKey('dock-app-dual-shell-fixture-0'));
      double iconScale(int index) => tester
          .widget<Transform>(
            find.byKey(ValueKey('dock-scale-隔离应用 ${index + 1}')),
          )
          .transform
          .entry(0, 0);
      final idleGeometry = (await _state())['dock'] as Map;
      expect(idleGeometry['inputX'], greaterThan(0));
      expect(idleGeometry['inputWidth'], lessThan(idleGeometry['width']));
      expect(
        (idleGeometry['inputX'] as int) + (idleGeometry['inputWidth'] as int),
        lessThan(idleGeometry['width']),
        reason: 'Resting Dock leaves both transparent side margins passthrough',
      );
      evidence['restingDockGeometry'] = idleGeometry;
      await tester.sendEventToBinding(
        mouse.addPointer(location: const Offset(1, 105), view: dockView),
      );
      await tester.sendEventToBinding(
        mouse.hover(tester.getCenter(firstIcon()), view: dockView),
      );
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 250));
      evidence['firstHover'] = {
        'viewId': dockView.viewId,
        'target': tester.getCenter(firstIcon()).toString(),
        'scale': iconScale(0),
        'neighborScale': iconScale(1),
        'labelVisible': find
            .byKey(const ValueKey('mac-dock-label'))
            .evaluate()
            .isNotEmpty,
      };
      expect(iconScale(0), greaterThan(1.3));
      expect(iconScale(1), greaterThan(1.02));
      expect(find.byKey(const ValueKey('mac-dock-label')), findsOneWidget);
      final magnified = await _native(
        tester,
        style: ShellStyle.macos,
        dockViewId: dockId,
      );
      for (final property in [
        'width',
        'height',
        'requestedWidth',
        'requestedHeight',
        'inputY',
        'inputHeight',
      ]) {
        expect(
          (magnified['dock'] as Map)[property],
          idleGeometry[property],
          reason: 'Hover changes paint, not native $property',
        );
      }
      final magnifiedDock = magnified['dock'] as Map;
      expect(
        magnifiedDock['inputWidth'],
        greaterThan(idleGeometry['inputWidth']),
        reason: 'Hover expands input to follow the visible glass',
      );
      expect(magnifiedDock['inputX'], lessThan(idleGeometry['inputX']));
      evidence['magnifiedDockGeometry'] = magnifiedDock;
      await record('macos-neighbor-magnification', magnified);
      expect(
        binding.transientCallbackCount,
        0,
        reason: 'Settled Dock hover retains no animation ticker',
      );
      controller.selectPanel(ShellPanel.quickSettings);
      await record(
        'macos-hover-cleared-by-panel',
        await _native(
          tester,
          style: ShellStyle.macos,
          expanded: true,
          dockViewId: dockId,
        ),
      );
      expect(iconScale(0), 1);
      expect(find.byKey(const ValueKey('mac-dock-label')), findsNothing);
      controller.closePanel();
      await record(
        'macos-no-stale-hover-after-panel',
        await _native(tester, style: ShellStyle.macos, dockViewId: dockId),
      );
      expect(iconScale(0), 1);
      expect(find.byKey(const ValueKey('mac-dock-label')), findsNothing);
      await tester.sendEventToBinding(
        mouse.hover(const Offset(1, 105), view: dockView),
      );
      await tester.sendEventToBinding(
        mouse.hover(tester.getCenter(firstIcon()), view: dockView),
      );
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 250));
      expect(iconScale(0), greaterThan(1.3));
      await record(
        'macos-hover-restored-after-pointer-move',
        await _native(tester, style: ShellStyle.macos, dockViewId: dockId),
      );
      await tester.sendEventToBinding(mouse.removePointer(view: dockView));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 250));
      expect(iconScale(0), 1);
      expect(find.byKey(const ValueKey('mac-dock-label')), findsNothing);
      await record(
        'macos-hover-exit-idle',
        await _native(tester, style: ShellStyle.macos, dockViewId: dockId),
      );
      expect(binding.transientCallbackCount, 0);
      evidence['magnificationInput'] =
          'Injected TestPointer device 700 in '
          'Dock view $dockId; neighbor scale, fixed surface bounds, input '
          'following animated glass, transparent side margins, panel '
          'clearing, actual pointer movement to restore, and idle ticker checked.';

      for (final reduced in [true, false]) {
        controller.setReduceMotion(reduced);
        for (final panel in [ShellPanel.launcher, ShellPanel.quickSettings]) {
          // The first launcher action originates in the secondary native view.
          if (panel == ShellPanel.launcher && reduced) {
            controller.setPinnedAppIds(const []);
            await tester.pump();
            await _native(tester, style: ShellStyle.macos, dockViewId: dockId);
            injectedPointerGestures++;
            await tester.tap(find.byKey(const ValueKey('dock-launcher')));
            controller.setPinnedAppIds(_pins);
          } else {
            controller.selectPanel(panel);
          }
          state = await _native(
            tester,
            style: ShellStyle.macos,
            expanded: true,
            dockViewId: dockId,
          );
          expect(controller.panel, panel);
          await record(
            'macos-${reduced ? 'reduced' : 'animated'}-${panel.name}',
            state,
          );
          controller.closePanel();
          await record(
            'macos-${reduced ? 'reduced' : 'animated'}-${panel.name}-closed',
            await _native(tester, style: ShellStyle.macos, dockViewId: dockId),
          );
        }
        injectedPointerGestures++;
        await tester.tap(
          find.byKey(const ValueKey('dock-app-dual-shell-fixture-0')),
          kind: PointerDeviceKind.mouse,
          buttons: kSecondaryMouseButton,
        );
        state = await _native(
          tester,
          style: ShellStyle.macos,
          expanded: true,
          dockViewId: dockId,
        );
        expect(controller.panel, ShellPanel.dockAppMenu);
        await record(
          'macos-${reduced ? 'reduced' : 'animated'}-dock-context-menu',
          state,
        );
        controller.closePanel();
        await record(
          'macos-${reduced ? 'reduced' : 'animated'}-dock-context-closed',
          await _native(tester, style: ShellStyle.macos, dockViewId: dockId),
        );
      }

      final loadsBeforeStyleSwitches = apps.loads;
      // Superseded native work must not restore an earlier style or panel.
      for (var change = 0; change < 12; change++) {
        controller.setShellStyle(
          change.isEven ? ShellStyle.material : ShellStyle.macos,
        );
        if (change == 5) controller.selectPanel(ShellPanel.quickSettings);
      }
      controller.closePanel();
      await record(
        'macos-rapid-style-latest-state',
        await _native(tester, style: ShellStyle.macos, dockViewId: dockId),
      );
      for (var iteration = 0; iteration < 10; iteration++) {
        final reduced = iteration.isEven;
        controller.setReduceMotion(reduced);
        for (final style in [ShellStyle.material, ShellStyle.macos]) {
          controller.setShellStyle(style);
          state = await _native(tester, style: style, dockViewId: dockId);
          expect(controller.barPosition, BarPosition.bottom);
          expect(binding.platformDispatcher.views.length, 2);
          await record('switch-$iteration-${style.name}', state);
          expect(
            apps.loads,
            loadsBeforeStyleSwitches,
            reason: 'Changing shell style does not reload catalogue services',
          );
          await _persisted(tester, style, reduced);
        }
      }
      controller.setReduceMotion(true);
      controller.setShellStyle(ShellStyle.material);
      await record(
        'material-restored-original-bottom',
        await _native(tester, style: ShellStyle.material, dockViewId: dockId),
      );
      await _persisted(tester, ShellStyle.material, true);
      evidence['preferences'] = jsonDecode(
        await ShellPreferences.file!.readAsString(),
      );
      evidence['sharedControllerIdentity'] = identityHashCode(controller);
      evidence['mainViewId'] = state['mainViewId'];
      state = await _state();
      expect(
        state['barBlurRequested'],
        isFalse,
        reason: 'Material clears protocol blur',
      );
      expect(state['dockBlurRequested'], isFalse);
      evidence['reusedDockViewId'] = dockId;
      evidence['ipcStarts'] = ipc.starts;
      evidence['powerStarts'] = power.starts;
      evidence['appLoads'] = apps.loads;
      evidence['injectedPointerGestures'] = injectedPointerGestures;
      expect(injectedPointerGestures, 5);
      evidence['passed'] = true;
      binding.reportData = evidence;
    } finally {
      await tester.pumpWidget(
        const ViewCollection(views: []),
        wrapWithView: false,
      );
      await _window.invokeMethod<void>('setExpanded', false);
      await _window.invokeMethod<void>('setShellStyle', 'material');
      evidence['ipcDisposalsAfterUnmount'] = ipc.disposals;
      evidence['powerDisposalsAfterUnmount'] = power.disposals;
      evidence['ignoredUnattachedDeviceEvents'] =
          deviceFilter.unattachedDeviceEvents;
      evidence['ignoredUnattachedDeviceEventCount'] =
          deviceFilter.unattachedDeviceEvents.length;
      await File(
        '${captures.path}/dual-shell-native-evidence.json',
      ).writeAsString(
        '${const JsonEncoder.withIndent('  ').convert(evidence)}\n',
      );
      expect(ipc.disposals, 1);
      expect(power.disposals, 1);
    }
  }, timeout: const Timeout(Duration(minutes: 4)));
  tearDownAll(() {
    // The sole case detached every RenderView. Stop the live test binding's
    // WidgetTester dispatcher before restoring hardware packet delivery, so
    // a late native leave event cannot hit its firstWhere during shutdown.
    binding.deviceEventDispatcher = null;
    deviceFilter.restore();
  });
}
