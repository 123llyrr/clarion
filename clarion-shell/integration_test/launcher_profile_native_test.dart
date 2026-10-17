// SPDX-License-Identifier: GPL-3.0-or-later
// Run only through the reviewed private native harness using flutter drive
// --profile. No manually pumped frames, screenshots, filesystem writes, native
// state queries, or quantile calculations occur inside the timed workload.
import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math' as math;
import 'dart:ui' as ui;

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:clarion_shell/main.dart';
import 'package:clarion_shell/mac_shell_preferences.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/preferences.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/ui/mac_launcher_panel.dart';

import 'helpers/launcher_glass_checks.dart' show NativeViewDeviceFilter;

const _window = MethodChannel('clarion.shell/window');
const _timedDuration = Duration(seconds: 30);

class _Controller extends ShellController {
  _Controller(this.catalogue)
    : super(
        demo: true,
        notifications: false,
        tray: false,
        powerProfilesEnabled: false,
        mediaEnabled: false,
      );
  final List<DesktopApp> catalogue;
  @override
  List<DesktopApp> get apps => catalogue;
  @override
  Future<void> launchApp(DesktopApp app) async =>
      throw StateError('The performance fixture never launches an application');
}

Future<Map<String, dynamic>> _state() async => Map<String, dynamic>.from(
  (await _window.invokeMapMethod<String, dynamic>('windowState'))!,
);

Future<void> _waitFor(bool Function() condition, String reason) async {
  final deadline = DateTime.now().add(const Duration(seconds: 15));
  while (!condition()) {
    if (DateTime.now().isAfter(deadline)) fail('Timed out: $reason');
    await Future<void>.delayed(const Duration(milliseconds: 25));
  }
}

Future<Map<String, dynamic>> _waitNative(
  bool expanded, {
  ShellController? controller,
}) async {
  final deadline = DateTime.now().add(const Duration(seconds: 15));
  var observedMacos = false;
  do {
    final state = await _state();
    final macos = state['style'] == 'macos' && state['shellStyle'] == 'macos';
    if (controller != null &&
        (controller.shellStyle != ShellStyle.macos ||
            (observedMacos && !macos))) {
      fail('Native macOS Shell fell back before settling: $state');
    }
    observedMacos = observedMacos || macos;
    if (state['expanded'] == expanded &&
        state['launcherSceneVisible'] == expanded) {
      return state;
    }
    await Future<void>.delayed(const Duration(milliseconds: 25));
  } while (DateTime.now().isBefore(deadline));
  fail('Native launcher state did not settle: ${await _state()}');
}

/// Only untimed warmup calls this observer. A native surface's requested
/// geometry alone cannot prove its Flutter view painted or its Dock survived.
Future<Map<String, dynamic>> _waitStableMacShell(
  WidgetTester tester,
  ShellController controller, {
  required bool expanded,
  required Map<String, Object?> evidence,
}) async {
  final elapsed = Stopwatch()..start();
  const limit = Duration(seconds: 15);
  final observations = <Map<String, Object?>>[];
  evidence.addAll({
    'sampleIntervalMillis': 50,
    'maximumWaitMillis': 15000,
    'requiredConsecutiveStableSamples': 2,
    'expectedExpanded': expanded,
    'observations': observations,
    'passed': false,
  });
  String? previousGeometry;
  var stableSamples = 0;
  while (elapsed.elapsed < limit) {
    final remaining = limit - elapsed.elapsed;
    final state = await _state().timeout(remaining);
    observations.add({
      'elapsedMicros': elapsed.elapsedMicroseconds,
      'native': state,
    });
    // _waitNative(true) has already observed the native macOS path. Material
    // here is a real fallback, not a permitted startup intermediate state.
    if (state['style'] != 'macos' ||
        state['shellStyle'] != 'macos' ||
        controller.shellStyle != ShellStyle.macos) {
      fail('Native macOS Shell fell back during warmup: $state');
    }
    final dock = state['dock'];
    final monitor = state['monitorGeometry'];
    final views = ui.PlatformDispatcher.instance.views.toList();
    final mainView = views
        .where((view) => view.viewId == state['mainViewId'])
        .firstOrNull;
    final dockView = views
        .where((view) => view.viewId == state['dockViewId'])
        .firstOrNull;
    bool positive(Object? value) => value is num && value.isFinite && value > 1;
    bool matchesView(ui.FlutterView? view, Object? width, Object? height) =>
        view != null &&
        positive(width) &&
        positive(height) &&
        view.devicePixelRatio.isFinite &&
        view.devicePixelRatio > 0 &&
        view.physicalSize.width == (width as num) * view.devicePixelRatio &&
        view.physicalSize.height == (height as num) * view.devicePixelRatio;
    var fullyOpen = false;
    if (expanded && find.byType(MacLauncherPanel).evaluate().length == 1) {
      final opacity = find.ancestor(
        of: find.byType(MacLauncherPanel),
        matching: find.byType(Opacity),
      );
      fullyOpen =
          opacity.evaluate().isNotEmpty &&
          tester.widget<Opacity>(opacity.first).opacity == 1;
    }
    final conditions = <String, bool>{
      'mainViewExists': mainView != null,
      'independentDockViewExists':
          dockView != null &&
          mainView != null &&
          dockView.viewId != mainView.viewId,
      'mainAllocationMatchesFlutterView': matchesView(
        mainView,
        state['width'],
        state['height'],
      ),
      'dockAllocationMatchesFlutterView':
          dock is Map && matchesView(dockView, dock['width'], dock['height']),
      'mainRequestedHeightAllocated':
          positive(state['requestedHeight']) &&
          state['height'] == state['requestedHeight'],
      'mainOutputWidthAllocated':
          monitor is Map &&
          positive(monitor['width']) &&
          state['width'] == monitor['width'],
      'mainExpandedHeightAllocated':
          !expanded ||
          (monitor is Map &&
              positive(monitor['height']) &&
              state['height'] == monitor['height']),
      'expectedExpansion': state['expanded'] == expanded,
      'expectedLauncherScene': state['launcherSceneVisible'] == expanded,
      'expectedBackdrop': state['launcherBackdropRequested'] == expanded,
      'mainKeyboardMode':
          state['keyboard'] == (expanded ? 'Exclusive' : 'None'),
      'mainLayer': state['layer'] == (expanded ? 'Overlay' : 'Top'),
      'launcherForegroundSettled': expanded
          ? fullyOpen
          : find.byType(MacLauncherPanel).evaluate().isEmpty,
      'nativeGlassAvailable': state['blurAvailable'] == true,
      'dockReady': dock is Map && dock['ready'] == true,
      'dockVisible':
          dock is Map &&
          dock['visible'] == true &&
          state['dockVisible'] == true,
      'dockAutoHideDisabled': dock is Map && dock['autoHide'] == false,
      'dockRequestedSizeAllocated':
          dock is Map &&
          positive(dock['requestedWidth']) &&
          positive(dock['requestedHeight']) &&
          dock['width'] == dock['requestedWidth'] &&
          dock['height'] == dock['requestedHeight'],
    };
    observations.last['conditions'] = conditions;
    final geometry = jsonEncode({
      'main': [
        state['mainViewId'],
        state['width'],
        state['height'],
        state['requestedHeight'],
        state['scaleFactor'],
      ],
      'monitor': monitor,
      'dock': dock is Map
          ? [
              state['dockViewId'],
              dock['width'],
              dock['height'],
              dock['requestedWidth'],
              dock['requestedHeight'],
            ]
          : null,
      'views': [
        for (final view in views)
          [
            view.viewId,
            view.physicalSize.width,
            view.physicalSize.height,
            view.devicePixelRatio,
          ],
      ],
    });
    if (conditions.values.every((value) => value)) {
      stableSamples = geometry == previousGeometry ? stableSamples + 1 : 1;
      previousGeometry = geometry;
      if (stableSamples >= 2) {
        evidence.addAll({
          'passed': true,
          'stableSnapshot': state,
          'stableConditions': conditions,
          'stableGeometry': jsonDecode(geometry),
          'consecutiveStableSamples': stableSamples,
          'elapsedMicros': elapsed.elapsedMicroseconds,
        });
        return state;
      }
    } else {
      stableSamples = 0;
      previousGeometry = null;
    }
    final remainingMicros = limit.inMicroseconds - elapsed.elapsedMicroseconds;
    if (remainingMicros > 0) {
      await Future<void>.delayed(
        Duration(microseconds: math.min(50000, remainingMicros)),
      );
    }
  }
  fail(
    'Native Shell/Dock did not stabilize before timing: ${observations.last}',
  );
}

Future<List<DesktopApp>> _catalogue(String root) async {
  final icons = Directory('$root/icons');
  await icons.create();
  // A bounded, real raster/vector inventory exercises decoding and page
  // remounts. Icons are prepared before warmup; no fixture IO is timed.
  for (var index = 0; index < 32; index++) {
    final rgb = 0xff000000 | ((index * 674309 + 0x36a29c) & 0xffffff);
    final recorder = ui.PictureRecorder();
    final canvas = Canvas(recorder);
    canvas.drawRRect(
      RRect.fromRectAndRadius(
        const Rect.fromLTWH(0, 0, 160, 160),
        const Radius.circular(32),
      ),
      Paint()..color = Color(rgb),
    );
    canvas.drawCircle(
      const Offset(80, 80),
      42,
      Paint()..color = Colors.white70,
    );
    final picture = recorder.endRecording();
    final image = await picture.toImage(160, 160);
    try {
      final data = (await image.toByteData(format: ui.ImageByteFormat.png))!;
      await File(
        '${icons.path}/$index.png',
      ).writeAsBytes(data.buffer.asUint8List());
    } finally {
      image.dispose();
      picture.dispose();
    }
    await File('${icons.path}/$index.svg').writeAsString(
      '<svg xmlns="http://www.w3.org/2000/svg" width="160" height="160" '
      'viewBox="0 0 160 160"><rect width="160" height="160" rx="32" '
      'fill="#${(rgb & 0xffffff).toRadixString(16).padLeft(6, '0')}"/>'
      '<path d="M40 80L80 40L120 80L80 120Z" fill="#f7f7ff"/></svg>',
    );
  }
  return List.unmodifiable([
    for (var index = 0; index < 1000; index++)
      DesktopApp(
        id: 'clarion-launcher-profile-$index',
        name: 'Application ${index.toString().padLeft(4, '0')}',
        desktopFile: '',
        iconPath: '${icons.path}/${index % 32}.${index.isEven ? 'png' : 'svg'}',
        keywords: [index.isEven ? 'even' : 'odd'],
      ),
  ]);
}

Map<String, Object?> _summary(Iterable<int> values) {
  final sorted = values.toList()..sort();
  if (sorted.isEmpty) return {'count': 0};
  return {
    'count': sorted.length,
    'p50': sorted[(sorted.length * .50).ceil().clamp(1, sorted.length) - 1],
    'p95': sorted[(sorted.length * .95).ceil().clamp(1, sorted.length) - 1],
    'max': sorted.last,
  };
}

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  final deviceFilter = NativeViewDeviceFilter(binding);
  tearDownAll(deviceFilter.restore);
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.fullyLive;
  testWidgets('profile natural-vsync production launcher for thirty seconds', (
    tester,
  ) async {
    expect(
      kProfileMode,
      isTrue,
      reason: 'flutter test forces debug; use flutter drive --profile',
    );
    expect(kDebugMode, isFalse);
    expect(Platform.environment['CLARION_SHELL_ISOLATED_TEST'], '1');
    for (final pair in [
      ['WAYLAND_DISPLAY', 'CLARION_SHELL_PARENT_WAYLAND_DISPLAY'],
      ['CLARION_SOCKET', 'CLARION_SHELL_PARENT_SOCKET'],
      ['DBUS_SESSION_BUS_ADDRESS', 'CLARION_SHELL_PARENT_DBUS_ADDRESS'],
    ]) {
      expect(
        Platform.environment[pair.first],
        isNot(Platform.environment[pair.last]),
      );
    }
    final root = Platform.environment['CLARION_SHELL_TEST_ROOT']!;
    for (final key in [
      'XDG_CONFIG_HOME',
      'XDG_DATA_HOME',
      'XDG_STATE_HOME',
      'XDG_CACHE_HOME',
    ]) {
      expect(Platform.environment[key]!.startsWith('$root/'), isTrue);
    }
    final output = File(
      '${Platform.environment['CLARION_SHELL_CAPTURE_DIR']}/launcher-profile-evidence.json',
    );
    final controller = _Controller(await _catalogue(root))
      ..setThemeMode(ThemeMode.dark)
      ..setShellStyle(ShellStyle.macos)
      ..setMacPreferences(const MacShellPreferences(useOriginalIcons: false))
      ..setReduceMotion(false);
    final evidence = <String, Object?>{
      'passed': false,
      'scope':
          'Profile Flutter GTK ShellHost/LauncherSurface on an owned nested Vulkan WSI compositor; not physical KMS and not physical input certification',
      'buildMode': 'profile',
      'framePolicy': 'fullyLive',
      'markerAssociation': 'engine-frame-number',
      'fixtureApps': 1000,
      'fixtureTextures': 32,
      'input':
          'Controller panel commands, production TextField onChanged and framework key events; no forced frame pumping during timing',
      'timedManualPumps': 0,
      'timedScreenshots': 0,
      'timedNativeStateQueries': 0,
      'timedFilesystemWrites': 0,
    };
    final rawFrames = <Map<String, Object?>>[];
    final markers = <int, Map<String, Object?>>{};
    final actions = <Map<String, Object?>>[];
    final clock = Stopwatch();
    var timed = false;
    var enabled = true;
    var action = 'warmup';
    void timings(List<ui.FrameTiming> frames) {
      for (final frame in frames) {
        rawFrames.add({
          'frameNumber': frame.frameNumber,
          for (final phase in ui.FramePhase.values)
            '${phase.name}Micros': frame.timestampInMicroseconds(phase),
          'buildMicros': frame.buildDuration.inMicroseconds,
          'rasterMicros': frame.rasterDuration.inMicroseconds,
          'vsyncOverheadMicros': frame.vsyncOverhead.inMicroseconds,
          'totalMicros': frame.totalSpan.inMicroseconds,
        });
      }
    }

    binding.addTimingsCallback(timings);
    // Persistent callbacks run after the engine updates public FrameData, and
    // only observe frames. Its frame key is the same as FrameTiming.frameNumber;
    // predicted presentation time is distinct from FrameTiming.vsyncStart.
    binding.addPersistentFrameCallback((_) {
      if (!enabled || !timed) return;
      final frameNumber = binding.platformDispatcher.frameData.frameNumber;
      markers[frameNumber] = {
        'frameNumber': frameNumber,
        'predictedPresentationMicros':
            binding.currentSystemFrameTimeStamp.inMicroseconds,
        'elapsedMicros': clock.elapsedMicroseconds,
        'action': action,
        'transientCallbacks': binding.transientCallbackCount,
        'panel': controller.panel.name,
      };
    });
    Future<void> search(String value) async {
      final field = tester.widget<TextField>(
        find.byKey(const ValueKey('mac-launchpad-search')),
      );
      field.controller!.value = TextEditingValue(
        text: value,
        selection: TextSelection.collapsed(offset: value.length),
      );
      field.onChanged!(value);
    }

    Future<void> step(
      String name,
      FutureOr<void> Function() perform,
      int waitMillis,
    ) async {
      if (clock.elapsed >= _timedDuration) return;
      action = name;
      actions.add({'action': name, 'elapsedMicros': clock.elapsedMicroseconds});
      await perform();
      final remaining =
          _timedDuration.inMicroseconds - clock.elapsedMicroseconds;
      if (remaining > 0) {
        await Future<void>.delayed(
          Duration(microseconds: math.min(waitMillis * 1000, remaining)),
        );
      }
    }

    try {
      await tester.pumpWidget(
        ShellHost(
          controller: controller,
          preferences: const ShellPreferences(
            shellStyle: ShellStyle.macos,
            macShell: MacShellPreferences(useOriginalIcons: false),
          ),
          persist: false,
        ),
        wrapWithView: false,
      );
      controller.selectPanel(ShellPanel.launcher);
      await _waitFor(
        () => find.byType(MacLauncherPanel).evaluate().isNotEmpty,
        'production Launchpad mounts',
      );
      evidence['openNativeBeforeTiming'] = await _waitNative(
        true,
        controller: controller,
      );
      final openWarmup = <String, Object?>{};
      evidence['openShellWarmup'] = openWarmup;
      await _waitStableMacShell(
        tester,
        controller,
        expanded: true,
        evidence: openWarmup,
      );
      await Future<void>.delayed(const Duration(seconds: 2));
      await search('Application');
      await Future<void>.delayed(const Duration(milliseconds: 350));
      // In profile, key debugName getters are assert-only. Explicit physical
      // IDs keep the public simulator independent of those debug-only labels.
      await tester.sendKeyEvent(
        LogicalKeyboardKey.pageDown,
        physicalKey: PhysicalKeyboardKey.pageDown,
        platform: 'linux',
      );
      await Future<void>.delayed(const Duration(milliseconds: 350));
      await tester.sendKeyEvent(
        LogicalKeyboardKey.pageUp,
        physicalKey: PhysicalKeyboardKey.pageUp,
        platform: 'linux',
      );
      await Future<void>.delayed(const Duration(milliseconds: 350));
      await search('');
      controller.closePanel();
      await _waitNative(false, controller: controller);
      await Future<void>.delayed(const Duration(milliseconds: 500));
      final finalWarmup = <String, Object?>{};
      evidence['shellWarmupBeforeTiming'] = finalWarmup;
      evidence['stableSnapshotBeforeTiming'] = await _waitStableMacShell(
        tester,
        controller,
        expanded: false,
        evidence: finalWarmup,
      );
      evidence['warmupComplete'] = true;
      // Phase IO is complete before starting the timed stopwatch. An optional
      // external observer records its own overhead and all raw IPC responses.
      await File(
        '$root/profile-phase.json',
      ).writeAsString(jsonEncode({'phase': 'ready', 'durationSeconds': 30}));
      timed = true;
      clock.start();
      while (clock.elapsed < _timedDuration) {
        await step(
          'open',
          () => controller.selectPanel(ShellPanel.launcher),
          260,
        );
        await step('search-all', () => search('Application'), 220);
        await step('page-forward', () async {
          await tester.sendKeyEvent(
            LogicalKeyboardKey.pageDown,
            physicalKey: PhysicalKeyboardKey.pageDown,
            platform: 'linux',
          );
        }, 320);
        await step('search-odd', () => search('odd'), 220);
        await step('page-forward-filtered', () async {
          await tester.sendKeyEvent(
            LogicalKeyboardKey.pageDown,
            physicalKey: PhysicalKeyboardKey.pageDown,
            platform: 'linux',
          );
        }, 320);
        await step('clear-search', () => search(''), 220);
        await step('edit-animation', () async {
          await tester.sendKeyEvent(
            LogicalKeyboardKey.altLeft,
            physicalKey: PhysicalKeyboardKey.altLeft,
            platform: 'linux',
          );
        }, 520);
        await step('close-edit', controller.closePanel, 32);
        await step(
          'reverse-open',
          () => controller.selectPanel(ShellPanel.launcher),
          260,
        );
        await step('page-back', () async {
          await tester.sendKeyEvent(
            LogicalKeyboardKey.pageUp,
            physicalKey: PhysicalKeyboardKey.pageUp,
            platform: 'linux',
          );
        }, 320);
        await step('close', controller.closePanel, 240);
      }
      clock.stop();
      timed = false;
      evidence['timedElapsedMicros'] = clock.elapsedMicroseconds;
      evidence['actions'] = actions;
      await File('$root/profile-phase.json').writeAsString(
        jsonEncode({
          'phase': 'finished',
          'elapsedMicros': clock.elapsedMicroseconds,
        }),
      );
      // Wait for batched engine timing delivery without pumping or demanding a
      // scene redraw. These post-workload frames remain in rawFrames, outside
      // the explicitly marked timed sample set.
      controller.closePanel();
      await Future<void>.delayed(const Duration(seconds: 2));
      final closed = evidence['closedNativeAfterTiming'] = await _waitNative(
        false,
        controller: controller,
      );
      expect(closed['launcherBackdropRequested'], isFalse);
      final menuHeight = (closed['menuBarHeight'] as num).toDouble();
      final regions = closed['mainBlurRegions'] as List;
      expect(
        regions.every(
          (region) =>
              region is Map &&
              region['y'] == 0 &&
              (region['height'] as num) <= menuHeight,
        ),
        isTrue,
        reason: 'Only the live menu bar may retain native blur after closing',
      );
      expect(binding.transientCallbackCount, 0);
      for (final frame in rawFrames) {
        frame['timedMarker'] = markers[frame['frameNumber']];
      }
      final selected =
          rawFrames.where((frame) => frame['timedMarker'] != null).toList()
            ..sort(
              (a, b) => (a['vsyncStartMicros'] as int).compareTo(
                b['vsyncStartMicros'] as int,
              ),
            );
      final matched = selected.map((frame) => frame['frameNumber']).toSet();
      final unmatched = markers.keys
          .where((frameNumber) => !matched.contains(frameNumber))
          .toList();
      final views = ui.PlatformDispatcher.instance.views.toList();
      final refreshRate = views.first.display.refreshRate;
      expect(refreshRate, greaterThan(0));
      final budget = 1000000 / refreshRate;
      final intervals = <Map<String, Object?>>[];
      for (var index = 1; index < selected.length; index++) {
        final previous = selected[index - 1];
        final current = selected[index];
        final before = previous['timedMarker'] as Map<String, Object?>;
        final after = current['timedMarker'] as Map<String, Object?>;
        final interval =
            (current['vsyncStartMicros'] as int) -
            (previous['vsyncStartMicros'] as int);
        intervals.add({
          'previousFrameNumber': previous['frameNumber'],
          'frameNumber': current['frameNumber'],
          'intervalMicros': interval,
          'previousAction': before['action'],
          'action': after['action'],
          'long': interval > budget * 1.5,
          'animationQualified':
              (before['transientCallbacks'] as int) > 0 &&
              (after['transientCallbacks'] as int) > 0,
        });
      }
      final animationIntervals = intervals
          .where((interval) => interval['animationQualified'] == true)
          .toList();
      final longFrames = selected
          .where(
            (frame) =>
                (frame['buildMicros'] as int) > budget ||
                (frame['rasterMicros'] as int) > budget ||
                (frame['totalMicros'] as int) > budget * 1.5,
          )
          .toList();
      evidence.addAll({
        'displayRefreshRateHz': refreshRate,
        'frameBudgetMicros': budget,
        'views': [
          for (final view in views)
            {
              'viewId': view.viewId,
              'width': view.physicalSize.width,
              'height': view.physicalSize.height,
              'scale': view.devicePixelRatio,
            },
        ],
        'rawFrames': rawFrames,
        'timedFrameMarkers': markers.map(
          (frameNumber, marker) => MapEntry('$frameNumber', marker),
        ),
        'unmatchedTimedFrameMarkers': unmatched,
        'timedFrameCount': selected.length,
        for (final metric in [
          'buildMicros',
          'rasterMicros',
          'vsyncOverheadMicros',
          'totalMicros',
        ])
          '$metric-summary': _summary(
            selected.map((frame) => frame[metric] as int),
          ),
        'allTimedFrameIntervals': intervals,
        'allLongFrameIntervals': intervals
            .where((interval) => interval['long'] == true)
            .toList(),
        'allLongFrames': longFrames,
        'animationQualifiedIntervalSummary': _summary(
          animationIntervals.map(
            (interval) => interval['intervalMicros'] as int,
          ),
        ),
        'animationQualifiedLongIntervalCount': animationIntervals
            .where((interval) => interval['long'] == true)
            .length,
        'intervalInterpretation':
            'All intervals and anomalies are retained. Intentional idle gaps appear in allTimedFrameIntervals. animationQualified uses adjacent frames whose natural scheduler observations both contain an active transient callback; no anomaly is removed from raw evidence. Flutter engine timing is not a GPU timestamp query or a KMS presentation timestamp.',
      });
      expect(selected.length, greaterThan(100));
      expect(
        unmatched,
        isEmpty,
        reason: 'Do not certify partial timing coverage',
      );
      expect(
        clock.elapsedMicroseconds,
        greaterThanOrEqualTo(_timedDuration.inMicroseconds),
      );
      expect(tester.takeException(), isNull);
      evidence['ignoredUnattachedDeviceEvents'] = deviceFilter.ignored;
      evidence['passed'] = true;
    } finally {
      timed = false;
      enabled = false;
      binding.removeTimingsCallback(timings);
      evidence.putIfAbsent('rawFrames', () => rawFrames);
      evidence.putIfAbsent('actions', () => actions);
      await output.writeAsString(
        '${const JsonEncoder.withIndent('  ').convert(evidence)}\n',
      );
      binding.reportData = {
        'passed': evidence['passed'],
        'evidencePath': output.path,
      };
      await tester.pumpWidget(
        const ViewCollection(views: []),
        wrapWithView: false,
      );
      await _window.invokeMethod<void>('setExpanded', false);
      await _window.invokeMethod<void>('setShellStyle', 'material');
      controller.dispose();
    }
  });
}
