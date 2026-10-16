// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:convert';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:clarion_shell/desktop/desktop_application.dart';
import 'package:clarion_shell/desktop/desktop_layout.dart';
import 'package:clarion_shell/desktop/desktop_service.dart';
import 'package:clarion_shell/desktop/desktop_view.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/preferences.dart';
import 'package:clarion_shell/services/desktop_apps.dart';
import 'package:clarion_shell/services/application_launcher.dart';

const _window = MethodChannel('clarion.desktop/window');

class _Apps extends DesktopApps {
  @override
  Future<List<DesktopApp>> load() async => const [];
}

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

// flutter_test's live binding assumes a RenderView for every hardware packet.
// A GTK popup can emit enter/leave before attachment or after detachment. Only
// filter those unattached packets; tester gestures remain explicitly injected.
class _DeviceFilter {
  _DeviceFilter(this.binding) {
    original = binding.platformDispatcher.onPointerDataPacket;
    binding.platformDispatcher.onPointerDataPacket = handle;
  }

  final IntegrationTestWidgetsFlutterBinding binding;
  ui.PointerDataPacketCallback? original;
  final ignored = <Map<String, dynamic>>[];

  void handle(ui.PointerDataPacket packet) {
    final attached = binding.renderViews
        .map((render) => render.flutterView.viewId)
        .toSet();
    final valid = <ui.PointerData>[];
    for (final data in packet.data) {
      if (attached.contains(data.viewId)) {
        valid.add(data);
      } else {
        ignored.add({'viewId': data.viewId, 'change': data.change.name});
      }
    }
    if (valid.isNotEmpty) original?.call(ui.PointerDataPacket(data: valid));
  }

  void restore() => binding.platformDispatcher.onPointerDataPacket = original;
}

int _nextPointer = 12000;

Future<void> _tapAt(
  WidgetTester tester,
  Offset point, {
  required ui.FlutterView view,
  int buttons = kPrimaryButton,
}) async {
  final id = _nextPointer++;
  final pointer = TestPointer(id, PointerDeviceKind.mouse, id, buttons);
  await tester.sendEventToBinding(
    pointer.addPointer(location: point, view: view),
  );
  try {
    await tester.sendEventToBinding(pointer.down(point, view: view));
    await tester.sendEventToBinding(pointer.up(view: view));
  } finally {
    await tester.sendEventToBinding(pointer.removePointer(view: view));
  }
}

Future<void> _tap(
  WidgetTester tester,
  Finder widget, {
  int buttons = kPrimaryButton,
}) => _tapAt(
  tester,
  tester.getCenter(widget),
  view: View.of(tester.element(widget)),
  buttons: buttons,
);

ui.FlutterView _desktopFlutterView(WidgetTester tester) =>
    View.of(tester.element(find.byType(DesktopView)));

Future<Map<String, dynamic>> _native(WidgetTester tester, bool overlay) async {
  final deadline = DateTime.now().add(const Duration(seconds: 15));
  Map<String, dynamic> state = {};
  while (DateTime.now().isBefore(deadline)) {
    await tester.pump(const Duration(milliseconds: 20));
    state = (await _window.invokeMapMethod<String, dynamic>('windowState'))!;
    final geometry = state['monitorGeometry'] as Map;
    final interaction = state['interaction'] as Map;
    if (state['overlay'] != overlay ||
        state['outputName'] != 'winit' ||
        state['width'] != geometry['width'] ||
        state['height'] != geometry['height'] ||
        interaction['active'] != overlay ||
        interaction['inputEmpty'] != !overlay ||
        interaction['keyboard'] != (overlay ? 'Exclusive' : 'None')) {
      continue;
    }
    expect(state['role'], 'desktop');
    expect(state['namespace'], 'clarion-desktop');
    expect(state['exclusiveZone'], -1);
    // The desktop never changes its layer or recreates its Wayland surface.
    expect(state['layer'], 'Bottom');
    expect(state['keyboard'], 'OnDemand');
    expect(state['cornerPassthrough'], 8);
    for (final edge in ['Top', 'Bottom', 'Left', 'Right']) {
      expect(state['anchor$edge'], isTrue);
    }
    expect(interaction['namespace'], 'clarion-desktop-interaction');
    expect(interaction['layer'], 'Overlay');
    expect(interaction['exclusiveZone'], -1);
    expect(interaction['cornerPassthrough'], 0);
    if (interaction['firstFrame'] == true) {
      expect(interaction['visible'], isTrue);
      expect(interaction['mapped'], isTrue);
    }
    if (interaction['firstFrame'] == true) {
      expect(interaction['visible'], isTrue);
      expect(interaction['mapped'], isTrue);
    }
    if (overlay) {
      expect(interaction['viewId'], isA<int>());
      if (interaction['firstFrame'] != true ||
          interaction['width'] != geometry['width'] ||
          interaction['height'] != geometry['height']) {
        continue;
      }
      final render = WidgetsBinding.instance.renderViews
          .where((view) => view.flutterView.viewId == interaction['viewId'])
          .firstOrNull;
      if (render == null) continue;
      final viewport =
          render.flutterView.physicalSize / render.flutterView.devicePixelRatio;
      if (viewport !=
              Size(
                (geometry['width'] as num).toDouble(),
                (geometry['height'] as num).toDouble(),
              ) ||
          render.size != viewport) {
        continue;
      }
    }
    return state;
  }
  fail('Native desktop and interaction views did not settle: $state');
}

class _Pixels {
  const _Pixels(this.width, this.height, this.rgba, this.capture);
  final int width;
  final int height;
  final Uint8List rgba;
  final String capture;

  List<int> region(Rect rect) {
    final left = rect.left.floor().clamp(0, width - 1);
    final top = rect.top.floor().clamp(0, height - 1);
    final right = rect.right.ceil().clamp(left + 1, width);
    final bottom = rect.bottom.ceil().clamp(top + 1, height);
    return [
      for (var y = top; y < bottom; y++)
        ...rgba.sublist((y * width + left) * 4, (y * width + right) * 4),
    ];
  }
}

Future<_Pixels> _capture(String name) async {
  final file = File(
    '${Platform.environment['CLARION_SHELL_CAPTURE_DIR']}/$name.png',
  );
  // One screenshot pixel per output logical pixel, including fractional scales.
  final result = await Process.run('grim', [
    '-s',
    '1',
    '-o',
    'winit',
    file.path,
  ]).timeout(const Duration(seconds: 15));
  expect(result.exitCode, 0, reason: '${result.stdout}\n${result.stderr}');
  final codec = await ui.instantiateImageCodec(await file.readAsBytes());
  final frame = await codec.getNextFrame();
  try {
    final bytes = await frame.image.toByteData(
      format: ui.ImageByteFormat.rawRgba,
    );
    return _Pixels(
      frame.image.width,
      frame.image.height,
      bytes!.buffer.asUint8List(),
      file.path,
    );
  } finally {
    frame.image.dispose();
    codec.dispose();
  }
}

List<double> _rect(Rect rect) => [rect.left, rect.top, rect.width, rect.height];

int _checksum(List<int> bytes) {
  var value = 2166136261;
  for (final byte in bytes) {
    value = ((value ^ byte) * 16777619) & 0xffffffff;
  }
  return value;
}

int _changedPixels(List<int> before, List<int> after) {
  expect(after.length, before.length);
  var changed = 0;
  for (var i = 0; i < before.length; i += 4) {
    if ((before[i] - after[i]).abs() > 2 ||
        (before[i + 1] - after[i + 1]).abs() > 2 ||
        (before[i + 2] - after[i + 2]).abs() > 2) {
      changed++;
    }
  }
  return changed;
}

Map<String, dynamic> _ink(List<int> bytes) {
  final colours = <int, int>{};
  for (var i = 0; i < bytes.length; i += 4) {
    final colour = (bytes[i] << 16) | (bytes[i + 1] << 8) | bytes[i + 2];
    colours.update(colour, (count) => count + 1, ifAbsent: () => 1);
  }
  final common = colours.values.fold(
    0,
    (max, value) => value > max ? value : max,
  );
  return {'colours': colours.length, 'inkPixels': bytes.length ~/ 4 - common};
}

Rect _menuRect(WidgetTester tester) {
  final items = find.byWidgetPredicate((widget) => widget is PopupMenuEntry);
  expect(items, findsWidgets);
  Rect? bounds;
  for (final element in items.evaluate()) {
    final rect = tester.getRect(
      find.byElementPredicate((candidate) => identical(element, candidate)),
    );
    bounds = bounds == null ? rect : bounds.expandToInclude(rect);
  }
  return bounds!.inflate(6);
}

List<Finder> _routeTexts(Finder route) => [
  for (final element
      in find.descendant(of: route, matching: find.byType(Text)).evaluate())
    if ((element.widget as Text).data?.trim().isNotEmpty == true)
      find.byElementPredicate((candidate) => identical(candidate, element)),
];

class _PresentedPixels {
  const _PresentedPixels(this.pixels, this.evidence);
  final _Pixels pixels;
  final Map<String, dynamic> evidence;
}

Map<String, dynamic>? _partialEvidence;

Future<void> _writePartial() async {
  final evidence = _partialEvidence;
  if (evidence == null) return;
  await File(
    '${Platform.environment['CLARION_SHELL_CAPTURE_DIR']}/desktop-partial-evidence.json',
  ).writeAsString('${const JsonEncoder.withIndent('  ').convert(evidence)}\n');
}

Future<Map<String, dynamic>> _presentationDiagnostics() async {
  final native = (await _window.invokeMapMethod<String, dynamic>(
    'windowState',
  ))!;
  final outputs = await Process.run(
    Platform.environment['CLARION_SHELL_CLARION_BIN']!,
    ['msg', '--json', 'outputs'],
  ).timeout(const Duration(seconds: 10));
  return {
    'native': native,
    'outputs': outputs.exitCode == 0
        ? jsonDecode(outputs.stdout as String)
        : '${outputs.stderr}',
    'hasScheduledFrame': WidgetsBinding.instance.hasScheduledFrame,
    'views': [
      for (final render in WidgetsBinding.instance.renderViews)
        {
          'viewId': render.flutterView.viewId,
          'dpr': render.flutterView.devicePixelRatio,
          'physicalSize': [
            render.flutterView.physicalSize.width,
            render.flutterView.physicalSize.height,
          ],
          'renderSize': [render.size.width, render.size.height],
          'needsPaint': render.debugNeedsPaint,
        },
    ],
  };
}

Future<_PresentedPixels> _presentedRoute(
  WidgetTester tester,
  String name,
  _Pixels baseline,
  Rect rect,
  List<Finder> texts, {
  Rect Function()? currentRect,
}) async {
  final deadline = DateTime.now().add(const Duration(seconds: 5));
  final elapsed = Stopwatch()..start();
  var attempts = 0;
  Map<String, dynamic> last = {};
  while (DateTime.now().isBefore(deadline)) {
    // Flutter layout finishing does not imply GTK has submitted the new
    // secondary-view framebuffer. Assert the actual screencopy condition.
    await tester.pump(const Duration(milliseconds: 20));
    final bounds = currentRect?.call() ?? rect;
    final screenshot = await _capture(name);
    attempts++;
    final textInk = <Map<String, dynamic>>[];
    for (final text in texts) {
      final textRect = tester.getRect(text);
      final stats = _ink(screenshot.region(textRect));
      textInk.add({
        'text': tester.widget<Text>(text).data,
        'rect': _rect(textRect),
        ...stats,
      });
    }
    final changed = _changedPixels(
      baseline.region(bounds),
      screenshot.region(bounds),
    );
    last = {
      'rect': _rect(bounds),
      'changedPixels': changed,
      'regionChecksum': _checksum(screenshot.region(bounds)),
      'capture': screenshot.capture,
      'captureAttempts': attempts,
      'presentationElapsedMs': elapsed.elapsedMilliseconds,
      'textInk': textInk,
      'baselineSize': [baseline.width, baseline.height],
      'screenshotSize': [screenshot.width, screenshot.height],
      'diagnostics': await _presentationDiagnostics(),
    };
    _partialEvidence?['lastPresentation'] = {'name': name, ...last};
    await _writePartial();
    if (changed >= 1000 &&
        textInk.isNotEmpty &&
        textInk.every((text) => (text['inkPixels'] as int) >= 20)) {
      final proofs = _partialEvidence?['presentedRoutes'] as List<dynamic>?;
      proofs?.add({'name': name, ...last});
      await _writePartial();
      return _PresentedPixels(screenshot, last);
    }
  }
  fail(
    'Logical route exists but its native pixels were not presented: $name $last',
  );
}

Future<_PresentedPixels> _presentedMenu(
  WidgetTester tester,
  String name,
  _Pixels baseline,
) => _presentedRoute(
  tester,
  name,
  baseline,
  _menuRect(tester),
  _routeTexts(find.byWidgetPredicate((widget) => widget is PopupMenuEntry)),
);

Rect _dialogRect(WidgetTester tester) {
  final card = find.descendant(
    of: find.byType(AlertDialog),
    matching: find.byWidgetPredicate(
      (widget) => widget is Material && widget.type == MaterialType.card,
    ),
  );
  expect(card, findsOneWidget);
  return tester.getRect(card);
}

Future<_PresentedPixels> _presentedDialog(
  WidgetTester tester,
  String name,
  _Pixels baseline,
) async {
  final dialog = find.byType(AlertDialog);
  expect(dialog, findsOneWidget);
  // Text regions separately prove title, field label/content and action
  // buttons; an empty modal barrier cannot satisfy this assertion.
  return _presentedRoute(
    tester,
    name,
    baseline,
    _dialogRect(tester),
    _routeTexts(dialog),
    currentRect: () => _dialogRect(tester),
  );
}

Future<_Pixels> _dismissedRoute(
  WidgetTester tester,
  String name,
  _Pixels baseline,
  Rect rect,
) async {
  final deadline = DateTime.now().add(const Duration(seconds: 5));
  var attempts = 0;
  while (DateTime.now().isBefore(deadline)) {
    await tester.pump(const Duration(milliseconds: 20));
    final screenshot = await _capture(name);
    attempts++;
    if (_changedPixels(baseline.region(rect), screenshot.region(rect)) == 0) {
      return screenshot;
    }
  }
  fail(
    'Native menu pixels persisted after cancellation: $name ($attempts captures)',
  );
}

Future<List<Map<String, dynamic>>> _windows() async {
  final result = await Process.run(
    Platform.environment['CLARION_SHELL_CLARION_BIN']!,
    ['msg', '--json', 'windows'],
  ).timeout(const Duration(seconds: 10));
  expect(result.exitCode, 0, reason: '${result.stderr}');
  final decoded = jsonDecode(result.stdout as String);
  final windows = decoded is List
      ? decoded
      : (decoded as Map)['Windows'] ?? decoded['windows'];
  return (windows as List)
      .map((window) => Map<String, dynamic>.from(window as Map))
      .toList();
}

Future<void> _moveOccluder(int id, Offset point) async {
  final result =
      await Process.run(Platform.environment['CLARION_SHELL_CLARION_BIN']!, [
        'msg',
        'action',
        'move-floating-window',
        '--id',
        '$id',
        '-x',
        '${point.dx.round()}',
        '-y',
        '${point.dy.round()}',
      ]).timeout(const Duration(seconds: 10));
  expect(result.exitCode, 0, reason: '${result.stderr}');
}

Future<void> _menuContinuity(
  WidgetTester tester,
  String style,
  Directory desktop,
  DesktopService service,
  DesktopLayoutStore layout,
  Map<String, dynamic> evidence,
) async {
  final entryPath = '${desktop.path}/欢迎.txt';
  final entry = find.byKey(ValueKey('desktop-entry-$entryPath'));
  final icon = find
      .descendant(
        of: entry,
        matching: find.byWidgetPredicate(
          (widget) => widget is Image || widget is Icon,
        ),
      )
      .first;
  if (style == 'macos') {
    await _until(
      tester,
      () => find
          .descendant(of: entry, matching: find.byType(RawImage))
          .evaluate()
          .any((element) => (element.widget as RawImage).image != null),
      'decoded macOS desktop document icon',
    );
    await tester.pumpAndSettle();
  }
  final desktopElement = tester.element(find.byType(DesktopView));
  final originalIcon = tester.getRect(icon);
  final originalEntry = tester.getRect(entry);
  final view = _desktopFlutterView(tester);
  final viewport = view.physicalSize / view.devicePixelRatio;
  final blank = Offset(viewport.width * .55, viewport.height * .62);
  final paths = service.entries.map((entry) => entry.fullPath).toList();
  final positions = Map.of(layout.positions('winit'));
  final nativeBefore = await _native(tester, false);
  final before = await _capture('$style-menu-before');
  final expectedPixels = before.region(originalIcon);
  final pixelColours = <int>{
    for (var i = 0; i < expectedPixels.length; i += 4)
      (expectedPixels[i] << 16) |
          (expectedPixels[i + 1] << 8) |
          expectedPixels[i + 2],
  };
  expect(pixelColours.length, greaterThan(8), reason: 'Visible icon pixels');
  final results = <Map<String, dynamic>>[];

  Future<void> unchanged(
    String name,
    bool overlay,
    List<int> expected, {
    _Pixels? presented,
    Map<String, dynamic>? presentation,
  }) async {
    final native = await _native(tester, overlay);
    expect(
      identical(tester.element(find.byType(DesktopView)), desktopElement),
      isTrue,
    );
    expect(tester.getRect(icon), originalIcon);
    expect(tester.getRect(entry), originalEntry);
    expect(service.entries.map((entry) => entry.fullPath).toList(), paths);
    expect(layout.positions('winit'), positions);
    final screenshot = presented ?? await _capture('$style-$name');
    expect(
      screenshot.region(originalIcon),
      expected,
      reason: 'Desktop icon pixels remain continuous during $name',
    );
    expect(native['width'], nativeBefore['width']);
    expect(native['height'], nativeBefore['height']);
    results.add({
      'name': name,
      'native': native,
      'capture': screenshot.capture,
      'iconRect': _rect(tester.getRect(icon)),
      'iconRegionChecksum': _checksum(screenshot.region(originalIcon)),
      'menuPresentation': ?presentation,
    });
  }

  Future<_PresentedPixels> open(String name, _Pixels baseline) async {
    await _tapAt(tester, blank, buttons: kSecondaryMouseButton, view: view);
    await tester.pumpAndSettle();
    await _native(tester, true);
    expect(find.text('新建文件夹'), findsOneWidget);
    return _presentedMenu(tester, '$style-$name', baseline);
  }

  final opened = await open('menu-open', before);
  await unchanged(
    'menu-open',
    true,
    expectedPixels,
    presented: opened.pixels,
    presentation: opened.evidence,
  );
  final menuRect = _menuRect(tester);
  await tester.sendKeyEvent(LogicalKeyboardKey.escape);
  await tester.pumpAndSettle();
  final escaped = await _dismissedRoute(
    tester,
    '$style-menu-escape',
    before,
    menuRect,
  );
  await unchanged('menu-escape', false, expectedPixels, presented: escaped);
  final outsideOpen = await open('menu-outside-open', before);
  await unchanged(
    'menu-outside-open',
    true,
    expectedPixels,
    presented: outsideOpen.pixels,
    presentation: outsideOpen.evidence,
  );
  final outsideRect = _menuRect(tester);
  final popup = (await _native(tester, true))['interaction'] as Map;
  final popupView = WidgetsBinding.instance.platformDispatcher.views
      .singleWhere((candidate) => candidate.viewId == popup['viewId']);
  // The interaction window owns dismissal at corners without forwarding it to
  // a desktop icon, application, or compositor hot corner.
  await _tapAt(tester, const Offset(4, 4), view: popupView);
  await tester.pumpAndSettle();
  final outside = await _dismissedRoute(
    tester,
    '$style-menu-outside',
    before,
    outsideRect,
  );
  await unchanged('menu-outside', false, expectedPixels, presented: outside);

  // A genuinely visible menu item must open a genuinely visible dialog. Cancel
  // leaves the fixture files untouched in both shell styles, including short QA.
  final dialogOpen = await open('menu-dialog-open', before);
  await unchanged(
    'menu-dialog-open',
    true,
    expectedPixels,
    presented: dialogOpen.pixels,
    presentation: dialogOpen.evidence,
  );
  await _tap(tester, find.text('新建文件夹'));
  await tester.pumpAndSettle();
  await _native(tester, true);
  final dialog = await _presentedDialog(
    tester,
    '$style-menu-dialog-visible',
    before,
  );
  results.add({
    'name': 'menu-dialog-visible',
    'dialogPresentation': dialog.evidence,
  });
  final dialogRect = _dialogRect(tester);
  await _tap(tester, find.text('取消'));
  await tester.pumpAndSettle();
  final dialogCancelled = await _dismissedRoute(
    tester,
    '$style-menu-dialog-cancelled',
    before,
    dialogRect,
  );
  await unchanged(
    'menu-dialog-cancelled',
    false,
    expectedPixels,
    presented: dialogCancelled,
  );

  // A genuine independent xdg toplevel proves the icon is never lifted above
  // applications when its menu opens. It exists only on the private display.
  final occlusionBinary = Platform.environment['CLARION_SHELL_OCCLUSION_BIN'];
  expect(occlusionBinary, isNotNull);
  final child = await Process.start(occlusionBinary!, const []);
  var childStopped = false;
  Future<void> stopChild() async {
    if (childStopped) return;
    childStopped = true;
    child.kill();
    await child.exitCode.timeout(
      const Duration(seconds: 5),
      onTimeout: () {
        child.kill(ProcessSignal.sigkill);
        return -1;
      },
    );
  }

  addTearDown(stopChild);
  Map<String, dynamic>? window;
  final deadline = DateTime.now().add(const Duration(seconds: 15));
  while (DateTime.now().isBefore(deadline)) {
    window = (await _windows())
        .where((candidate) => candidate['pid'] == child.pid)
        .firstOrNull;
    if (window != null) break;
    await tester.pump(const Duration(milliseconds: 30));
  }
  expect(window, isNotNull, reason: 'Private xdg occlusion fixture mapped');
  expect(window!['app_id'], 'clarion-desktop-occlusion');
  expect(window['is_floating'], isTrue);
  final position = Offset(
    (originalIcon.center.dx - 100).clamp(0, viewport.width - 200).toDouble(),
    (originalIcon.center.dy - 80).clamp(0, viewport.height - 160).toDouble(),
  );
  await _moveOccluder(window['id'] as int, position);
  await tester.pump(const Duration(milliseconds: 150));
  await tester.pumpAndSettle();
  final occluded = await _capture('$style-menu-occluded-before');
  final occludedPixels = occluded.region(originalIcon);
  var differentPixels = 0;
  for (var i = 0; i < expectedPixels.length; i += 4) {
    if (expectedPixels[i] != occludedPixels[i] ||
        expectedPixels[i + 1] != occludedPixels[i + 1] ||
        expectedPixels[i + 2] != occludedPixels[i + 2]) {
      differentPixels++;
    }
  }
  expect(
    differentPixels,
    greaterThan(1000),
    reason: 'Ordinary xdg window genuinely covers the icon',
  );
  final occludedOpen = await open('menu-occluded-open', occluded);
  await unchanged(
    'menu-occluded-open',
    true,
    occludedPixels,
    presented: occludedOpen.pixels,
    presentation: occludedOpen.evidence,
  );
  final occludedRect = _menuRect(tester);
  expect(
    (await _windows()).any((candidate) => candidate['pid'] == child.pid),
    isTrue,
  );
  await tester.sendKeyEvent(LogicalKeyboardKey.escape);
  await tester.pumpAndSettle();
  final occludedEscaped = await _dismissedRoute(
    tester,
    '$style-menu-occluded-escape',
    occluded,
    occludedRect,
  );
  await unchanged(
    'menu-occluded-escape',
    false,
    occludedPixels,
    presented: occludedEscaped,
  );
  await stopChild();
  await tester.pump(const Duration(milliseconds: 150));
  await tester.pumpAndSettle();
  await unchanged('menu-uncovered-final', false, expectedPixels);
  evidence[style] = {
    'desktopViewId': view.viewId,
    'desktopElementPreserved': true,
    'entriesAndLayoutPreserved': true,
    'iconRect': _rect(originalIcon),
    'iconRegionChecksum': _checksum(expectedPixels),
    'iconPixelColourCount': pixelColours.length,
    'occluderPid': child.pid,
    'occluderWindow': window,
    'occluderPosition': [position.dx, position.dy],
    'occluderChangedPixels': differentPixels,
    'occluderCapture': occluded.capture,
    'phases': results,
  };
}

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.onlyPumps;
  final deviceFilter = _DeviceFilter(binding);
  testWidgets('native desktop files, overlay dialogs, drag and live bar preferences', (
    tester,
  ) async {
    expect(Platform.environment['CLARION_SHELL_ISOLATED_TEST'], '1');
    expect(Platform.environment['CLARION_SHELL_TEST_DESKTOP_ONLY'], '1');
    for (final pair in [
      ('WAYLAND_DISPLAY', 'CLARION_SHELL_PARENT_WAYLAND_DISPLAY'),
      ('CLARION_SOCKET', 'CLARION_SHELL_PARENT_SOCKET'),
      ('DBUS_SESSION_BUS_ADDRESS', 'CLARION_SHELL_PARENT_DBUS_ADDRESS'),
    ]) {
      expect(
        Platform.environment[pair.$1],
        isNot(Platform.environment[pair.$2]),
      );
    }
    final menuOnly =
        Platform.environment['CLARION_SHELL_NATIVE_MENU_ONLY'] == '1';
    final root = Platform.environment['CLARION_SHELL_TEST_ROOT']!;
    final desktop = Directory('$root/fixture-desktop')..createSync();
    File(
      '${desktop.path}/欢迎.txt',
    ).writeAsStringSync('isolated native desktop fixture');
    final prefs = File('$root/fixture-preferences.json');
    const initial = ShellPreferences(
      themeMode: ThemeMode.dark,
      reduceMotion: true,
      barPosition: BarPosition.top,
    );
    await initial.save(destination: prefs);
    final commands = <Map<String, dynamic>>[];
    Future<String> fixtureCommand(
      String program,
      List<String> arguments,
    ) async {
      commands.add({'program': program, 'arguments': arguments});
      if (program == 'gio' && arguments.first == 'open') return '';
      if (program == 'gio' && arguments.first == 'trash') {
        final path = arguments.last;
        expect(path.startsWith('${desktop.path}/'), isTrue);
        final trash = Directory('$root/fixture-trash')..createSync();
        await File(
          path,
        ).rename('${trash.path}/${File(path).uri.pathSegments.last}');
        return '';
      }
      if (program == 'mv' || program == 'chmod' || program == 'mkdir') {
        final result = await Process.run(program, arguments);
        if (result.exitCode != 0) throw StateError('${result.stderr}');
        return '${result.stdout}';
      }
      throw StateError('Forbidden native desktop command: $program $arguments');
    }

    final service = DesktopService(
      root: desktop.path,
      command: fixtureCommand,
      applicationLauncher: ApplicationLauncher(
        command: fixtureCommand,
        scopeProbe: () async => null,
      ),
    );
    final layout = DesktopLayoutStore(file: File('$root/fixture-layout.json'));
    await tester.pumpWidget(
      DesktopApplication(
        service: service,
        layout: layout,
        preferences: initial,
        preferencesFile: prefs,
        applications: _Apps(),
        nativeViews: true,
      ),
      wrapWithView: false,
    );
    await _until(tester, () => service.entries.length == 1, 'directory scan');
    final evidence = <String, dynamic>{
      'backend':
          'nested Clarion ${Platform.environment['CLARION_SHELL_TEST_RENDERER'] ?? 'vulkan'}, stable GTK desktop plus separate interaction view',
      'scope':
          'Actual native Wayland surfaces and screencopy pixels; gestures are tester injected, not physical input certification.',
      'menuContinuity': <String, dynamic>{},
      'mode': menuOnly ? 'menu-only' : 'full',
      'fileFlowExecuted': !menuOnly,
      'realApplicationLaunches': 0,
      'phases': <Map<String, dynamic>>[],
      'commands': commands,
      'presentedRoutes': <Map<String, dynamic>>[],
      'status': 'in-progress',
      'passed': false,
    };
    _partialEvidence = evidence;
    addTearDown(() async {
      evidence['status'] = evidence['passed'] == true ? 'passed' : 'failed';
      await _writePartial();
    });
    await _writePartial();
    final phases = evidence['phases'] as List<Map<String, dynamic>>;
    Future<void> record(String name, bool overlay) async {
      await tester.pumpAndSettle();
      final state = await _native(tester, overlay);
      final capture =
          '${Platform.environment['CLARION_SHELL_CAPTURE_DIR']}/$name.png';
      final result = await Process.run('grim', ['-o', 'winit', capture]);
      expect(result.exitCode, 0, reason: '${result.stderr}');
      phases.add({'name': name, 'native': state, 'capture': capture});
      await _writePartial();
    }

    await record('desktop-initial', false);
    await _menuContinuity(
      tester,
      'material',
      desktop,
      service,
      layout,
      evidence['menuContinuity'] as Map<String, dynamic>,
    );
    if (!menuOnly) {
      final blank = Offset(
        _desktopFlutterView(tester).physicalSize.width /
            _desktopFlutterView(tester).devicePixelRatio *
            .65,
        _desktopFlutterView(tester).physicalSize.height /
            _desktopFlutterView(tester).devicePixelRatio *
            .55,
      );
      var menuSequence = 0;
      Future<void> menu(String action, {Finder? target}) async {
        final serial = ++menuSequence;
        final baseline = await _capture('desktop-action-$serial-before');
        if (target == null) {
          await _tapAt(
            tester,
            blank,
            buttons: kSecondaryMouseButton,
            view: _desktopFlutterView(tester),
          );
        } else {
          await _tap(tester, target, buttons: kSecondaryMouseButton);
        }
        await tester.pumpAndSettle();
        await _native(tester, true);
        final menuPixels = await _presentedMenu(
          tester,
          'desktop-action-$serial-menu',
          baseline,
        );
        phases.add({
          'name': 'desktop-action-$serial-menu',
          'action': action,
          'menuPresentation': menuPixels.evidence,
        });
        await _tap(tester, find.text(action));
        await tester.pumpAndSettle();
        await _native(tester, true);
        final dialogPixels = await _presentedDialog(
          tester,
          'desktop-action-$serial-dialog',
          baseline,
        );
        phases.add({
          'name': 'desktop-action-$serial-dialog',
          'action': action,
          'dialogPresentation': dialogPixels.evidence,
        });
      }

      await menu('新建文件');
      await record('desktop-file-dialog', true);
      await tester.enterText(
        find.byKey(const ValueKey('desktop-name')),
        '新文件.txt',
      );
      await _tap(tester, find.byKey(const ValueKey('desktop-name-confirm')));
      await _until(
        tester,
        () => service.entries.any((entry) => entry.name == '新文件.txt'),
        'create file',
      );
      expect(File('${desktop.path}/新文件.txt').existsSync(), isTrue);
      await record('desktop-created', false);
      await menu('新建文件夹');
      await tester.enterText(find.byKey(const ValueKey('desktop-name')), '资料');
      await _tap(tester, find.byKey(const ValueKey('desktop-name-confirm')));
      await _until(
        tester,
        () => service.entries.any((entry) => entry.name == '资料'),
        'create folder',
      );
      expect(Directory('${desktop.path}/资料').existsSync(), isTrue);
      await _native(tester, false);
      Finder file() =>
          find.byKey(ValueKey('desktop-entry-${desktop.path}/新文件.txt'));
      await menu('重命名', target: file());
      await tester.enterText(
        find.byKey(const ValueKey('desktop-name')),
        '重命名.txt',
      );
      await _tap(tester, find.byKey(const ValueKey('desktop-name-confirm')));
      await _until(
        tester,
        () => service.entries.any((entry) => entry.name == '重命名.txt'),
        'rename',
      );
      await _native(tester, false);
      final renamed = find.byKey(
        ValueKey('desktop-entry-${desktop.path}/重命名.txt'),
      );
      final origin = tester.getCenter(renamed);
      final oldCell = layout.positions('winit')['${desktop.path}/重命名.txt'];
      final gesture = await tester.startGesture(
        origin,
        view: _desktopFlutterView(tester),
      );
      await gesture.moveTo(origin + const Offset(230, 0));
      await tester.pump();
      await gesture.up(view: _desktopFlutterView(tester));
      await _until(
        tester,
        () => layout.positions('winit')['${desktop.path}/重命名.txt'] != oldCell,
        'persist icon movement',
      );
      expect(tester.getCenter(renamed).dx, greaterThan(origin.dx));
      await record('desktop-dragged', false);
      await const ShellPreferences(
        themeMode: ThemeMode.light,
        reduceMotion: true,
        barPosition: BarPosition.bottom,
      ).save(destination: prefs);
      await _until(
        tester,
        () =>
            tester.widget<DesktopView>(find.byType(DesktopView)).barPosition ==
            BarPosition.bottom,
        'live bottom bar preference',
      );
      await record('desktop-bottom-bar', false);
      await menu('新建文件');
      await _tap(tester, find.text('取消'));
      await tester.pumpAndSettle();
      await _native(tester, false);
      expect(service.entries.length, 3);
      await menu('移到回收站', target: renamed);
      await record('desktop-trash-confirmation', true);
      await _tap(tester, find.byKey(const ValueKey('desktop-trash-confirm')));
      await _until(tester, () => service.entries.length == 2, 'GIO trash');
      expect(File('$root/fixture-trash/重命名.txt').existsSync(), isTrue);
      await record('desktop-final', false);

      const longName =
          'Clarion 桌面名称检查 - 包含空格的长中文文件名 用于确认三行标签和完整名称显示 '
          'Quarterly Product Architecture Review and Release Notes 2026 Final Version.txt';
      expect(utf8.encode(longName).length, lessThanOrEqualTo(255));
      final longPath = '${desktop.path}/$longName';
      await File(longPath).writeAsString('isolated long desktop filename');
      await _until(
        tester,
        () => service.entries.any((entry) => entry.fullPath == longPath),
        'watch long filename',
      );
      await tester.pumpAndSettle();
      final longEntry = find.byKey(ValueKey('desktop-entry-$longPath'));
      final longSemantics = find.ancestor(
        of: longEntry,
        matching: find.byWidgetPredicate(
          (widget) =>
              widget is Semantics && widget.properties.label == '$longName，文件',
        ),
      );
      bool longSelected() =>
          tester.widget<Semantics>(longSemantics).properties.selected == true;
      final longIcon = find.descendant(
        of: longEntry,
        matching: find.byIcon(Icons.insert_drive_file_rounded),
      );
      expect(longIcon, findsOneWidget);
      final iconBeforeSelection = tester.getRect(longIcon);
      final labelFinder = find.byKey(ValueKey('desktop-label-$longPath'));
      final label = tester.widget<Text>(labelFinder);
      expect(label.data, longName);
      expect(label.maxLines, 3);
      expect(label.overflow, TextOverflow.ellipsis);
      final labelPainter = TextPainter(
        text: TextSpan(text: label.data, style: label.style),
        textDirection: TextDirection.ltr,
        maxLines: label.maxLines,
        ellipsis: '…',
      )..layout(maxWidth: tester.getSize(labelFinder).width);
      expect(labelPainter.didExceedMaxLines, isTrue);
      labelPainter.dispose();

      // Hover has no button press and must not exercise a long-press gesture
      // or promote the desktop to a native overlay just to read the filename.
      // Native GTK and previous synthetic taps may already use device 1.
      // The hover fixture owns a distinct add/remove lifecycle.
      final hoverView = _desktopFlutterView(tester);
      final hover = TestPointer(9951, PointerDeviceKind.mouse, 9951);
      await tester.sendEventToBinding(
        hover.addPointer(location: const Offset(8, 8), view: hoverView),
      );
      try {
        await tester.sendEventToBinding(
          hover.hover(iconBeforeSelection.center, view: hoverView),
        );
        await tester.pump(const Duration(milliseconds: 450));
        await tester.pump(const Duration(milliseconds: 200));
        expect(find.text(longName), findsNWidgets(2));
        await record('desktop-long-name-hover', false);
      } finally {
        await tester.sendEventToBinding(hover.removePointer(view: hoverView));
      }
      await tester.pump(const Duration(milliseconds: 500));
      await tester.pumpAndSettle();

      await _tap(tester, longEntry);
      await tester.pumpAndSettle();
      expect(longSelected(), isTrue);
      expect(tester.getRect(longIcon), iconBeforeSelection);
      expect(tester.widget<Text>(labelFinder).maxLines, 3);
      expect(tester.widget<Text>(labelFinder).overflow, TextOverflow.ellipsis);
      final namePreview = find.byKey(const ValueKey('desktop-name-preview'));
      expect(namePreview, findsNothing);
      expect(find.byKey(const ValueKey('desktop-full-name')), findsNothing);
      final surfaceRect = tester.getRect(
        find.byKey(const ValueKey('desktop-surface')),
      );
      expect(
        tester.getRect(longEntry).bottom,
        lessThanOrEqualTo(surfaceRect.bottom - 40),
      );
      await record('desktop-long-name-selected', false);

      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      expect(longSelected(), isFalse);
      var arrowSelections = 0;
      while (arrowSelections <= service.entries.length) {
        await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
        await tester.pumpAndSettle();
        arrowSelections++;
        if (longSelected()) break;
      }
      expect(longSelected(), isTrue);
      expect(namePreview, findsNothing);
      expect(find.byKey(const ValueKey('desktop-full-name')), findsNothing);
      expect(tester.getRect(longIcon), iconBeforeSelection);
      await _native(tester, false);
      final openCountBefore = commands
          .where(
            (command) =>
                command['program'] == 'gio' &&
                (command['arguments'] as List<String>).first == 'open',
          )
          .length;
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await _until(
        tester,
        () =>
            commands
                .where(
                  (command) =>
                      command['program'] == 'gio' &&
                      (command['arguments'] as List<String>).first == 'open',
                )
                .length ==
            openCountBefore + 1,
        'keyboard open long filename',
      );
      expect(commands.last, {
        'program': 'gio',
        'arguments': ['open', '--', longPath],
      });
      await record('desktop-long-name-keyboard-open', false);
      evidence['longFilename'] = {
        'name': longName,
        'utf8Bytes': utf8.encode(longName).length,
        'labelMaxLines': label.maxLines,
        'selectedNamePreviewPresent': namePreview.evaluate().isNotEmpty,
        'iconBeforeSelection': [
          iconBeforeSelection.left,
          iconBeforeSelection.top,
          iconBeforeSelection.width,
          iconBeforeSelection.height,
        ],
        'iconAfterSelection': [
          tester.getRect(longIcon).left,
          tester.getRect(longIcon).top,
          tester.getRect(longIcon).width,
          tester.getRect(longIcon).height,
        ],
        'bottomBarLimit': surfaceRect.bottom - 40,
        'arrowSelections': arrowSelections,
        'keyboardOpenPath': longPath,
      };
      final reloaded = DesktopLayoutStore(
        file: File('$root/fixture-layout.json'),
      );
      await reloaded.load();
      expect(reloaded.positions('winit'), layout.positions('winit'));
      reloaded.dispose();
    }

    await const ShellPreferences(
      themeMode: ThemeMode.dark,
      reduceMotion: true,
      barPosition: BarPosition.top,
      shellStyle: ShellStyle.macos,
    ).save(destination: prefs);
    await _until(
      tester,
      () =>
          tester.widget<DesktopView>(find.byType(DesktopView)).shellStyle ==
          ShellStyle.macos,
      'live macOS desktop style',
    );
    await tester.pumpAndSettle();
    await _menuContinuity(
      tester,
      'macos',
      desktop,
      service,
      layout,
      evidence['menuContinuity'] as Map<String, dynamic>,
    );
    evidence['ignoredUnattachedDevicePackets'] = deviceFilter.ignored;
    evidence['passed'] = true;
    evidence['status'] = 'passed';
    await _writePartial();
    await tester.pumpWidget(
      const ViewCollection(views: []),
      wrapWithView: false,
    );
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
    await File(
      '${Platform.environment['CLARION_SHELL_CAPTURE_DIR']}/desktop-evidence.json',
    ).writeAsString(
      '${const JsonEncoder.withIndent('  ').convert(evidence)}\n',
    );
  }, timeout: const Timeout(Duration(minutes: 9)));
  tearDownAll(() {
    binding.deviceEventDispatcher = null;
    deviceFilter.restore();
  });
}
