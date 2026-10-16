// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:io';

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/desktop/desktop_application.dart';
import 'package:clarion_shell/desktop/desktop_layout.dart';
import 'package:clarion_shell/desktop/desktop_service.dart';
import 'package:clarion_shell/desktop/desktop_view.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/preferences.dart';
import 'package:clarion_shell/services/desktop_apps.dart';
import 'package:clarion_shell/services/clarion_ipc.dart';

const _window = MethodChannel('clarion.desktop/window');
const _icons = MethodChannel('clarion.shell/window');

class _Apps extends DesktopApps {
  @override
  Future<List<DesktopApp>> load() async => const [];
}

class _Capabilities extends ClarionIpc {
  _Capabilities({this.pending, this.supported = false});
  final Completer<dynamic>? pending;
  final bool supported;
  bool closed = false;
  @override
  Future<dynamic> request(Object request) async {
    expect(request, 'Capabilities');
    return pending?.future ??
        {
          'Capabilities': [if (supported) 'desktop-menu-focus-restore'],
        };
  }

  @override
  void dispose() {
    closed = true;
    super.dispose();
  }
}

class _Fixture {
  _Fixture() {
    File('${directory.path}/kept.txt').writeAsStringSync('desktop remains');
    service = DesktopService(
      root: directory.path,
      command: (_, _) async => throw StateError('Unexpected desktop command'),
    );
    layout = DesktopLayoutStore(file: File('${directory.path}/layout.json'));
  }

  final directory = Directory.systemTemp.createTempSync(
    'clarion-desktop-failure-',
  );
  late final DesktopService service;
  late final DesktopLayoutStore layout;

  Finder get icon =>
      find.byKey(ValueKey('desktop-entry-${directory.path}/kept.txt'));
}

Future<_Fixture> _mount(
  WidgetTester tester,
  Future<Object?> Function(MethodCall) handler, {
  bool nativeViews = false,
  ClarionIpc? capabilityIpc,
}) async {
  final fixture = _Fixture();
  addTearDown(() => fixture.directory.deleteSync(recursive: true));
  tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
    _window,
    handler,
  );
  tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
    _icons,
    (_) async => null,
  );
  addTearDown(() {
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      _window,
      null,
    );
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      _icons,
      null,
    );
  });
  await tester.runAsync(() async {
    await fixture.service.start();
    await fixture.layout.load();
  });
  await tester.pumpWidget(
    DesktopApplication(
      service: fixture.service,
      layout: fixture.layout,
      applications: _Apps(),
      watchPreferences: false,
      preferences: const ShellPreferences(reduceMotion: true),
      nativeViews: nativeViews,
      capabilityIpc: capabilityIpc ?? _Capabilities(),
    ),
    wrapWithView: !nativeViews,
  );
  await tester.pumpAndSettle();
  expect(fixture.icon, findsOneWidget);
  return fixture;
}

Future<void> _context(WidgetTester tester) async {
  await tester.tapAt(
    const Offset(600, 400),
    kind: PointerDeviceKind.mouse,
    buttons: kSecondaryMouseButton,
  );
  await tester.pump();
}

Future<void> _unmount(WidgetTester tester) async {
  await tester.pumpWidget(const SizedBox.shrink());
  await tester.pumpAndSettle();
}

void main() {
  testWidgets('running native capability enables inline menu rename', (
    tester,
  ) async {
    final ipc = _Capabilities(supported: true);
    await _mount(
      tester,
      (_) async => null,
      nativeViews: true,
      capabilityIpc: ipc,
    );
    expect(
      tester.widget<DesktopView>(find.byType(DesktopView)).inlineRenameFromMenu,
      isTrue,
    );
    expect(ipc.closed, isTrue);
    await _unmount(tester);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'disposing cancels the probe client and ignores a late capability reply',
    (tester) async {
      final reply = Completer<dynamic>();
      final ipc = _Capabilities(pending: reply);
      await _mount(
        tester,
        (_) async => null,
        nativeViews: true,
        capabilityIpc: ipc,
      );
      expect(
        tester
            .widget<DesktopView>(find.byType(DesktopView))
            .inlineRenameFromMenu,
        isFalse,
      );
      await _unmount(tester);
      expect(ipc.closed, isTrue);
      reply.complete({
        'Capabilities': ['desktop-menu-focus-restore'],
      });
      await tester.pump();
      expect(tester.takeException(), isNull);
    },
  );
  testWidgets(
    'failed native overlay preserves icons and a later menu can retry',
    (tester) async {
      final overlays = <bool>[];
      var failNextOpen = true;
      final fixture = await _mount(tester, (call) async {
        if (call.method == 'outputName') return 'DP-1';
        if (call.method == 'setOverlay') {
          final open = call.arguments as bool;
          overlays.add(open);
          if (open && failNextOpen) {
            failNextOpen = false;
            throw PlatformException(code: 'interaction-view-unavailable');
          }
        }
        return null;
      });
      final rect = tester.getRect(fixture.icon);
      await _context(tester);
      await tester.pumpAndSettle();
      expect(find.text('新建文件'), findsNothing);
      expect(tester.getRect(fixture.icon), rect);
      expect(
        tester
            .widget<IgnorePointer>(
              find.byKey(const ValueKey('desktop-icon-layer')),
            )
            .ignoring,
        isFalse,
      );
      expect(overlays.where((open) => open), hasLength(1));
      expect(overlays.last, isFalse);
      expect(tester.takeException(), isNull);

      await _context(tester);
      await tester.pumpAndSettle();
      expect(find.text('新建文件'), findsOneWidget);
      expect(tester.getRect(fixture.icon), rect);
      expect(overlays.where((open) => open), hasLength(2));
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      expect(find.text('新建文件'), findsNothing);
      expect(overlays.last, isFalse);
      expect(fixture.service.entries.single.name, 'kept.txt');
      expect(tester.takeException(), isNull);
      await _unmount(tester);
    },
  );

  testWidgets(
    'native view allocation failure never pushes a menu on the desktop view',
    (tester) async {
      var allocations = 0;
      final overlays = <bool>[];
      final fixture = await _mount(tester, (call) async {
        if (call.method == 'outputName') return 'DP-1';
        if (call.method == 'ensureInteractionView') {
          allocations++;
          throw PlatformException(code: 'interaction-view-unavailable');
        }
        if (call.method == 'setOverlay') {
          overlays.add(call.arguments as bool);
        }
        return null;
      }, nativeViews: true);
      final rect = tester.getRect(fixture.icon);
      for (var attempt = 1; attempt <= 2; attempt++) {
        await _context(tester);
        await tester.pumpAndSettle();
        expect(allocations, attempt);
        expect(overlays, isNot(contains(true)));
        expect(find.text('新建文件'), findsNothing);
        expect(tester.getRect(fixture.icon), rect);
        expect(tester.takeException(), isNull);
      }
      await _unmount(tester);
    },
  );

  for (final fail in [false, true]) {
    testWidgets(
      'disposing during a pending native open closes its ${fail ? 'failed' : 'successful'} reply',
      (tester) async {
        final reply = Completer<void>();
        final overlays = <bool>[];
        await _mount(tester, (call) async {
          if (call.method == 'outputName') return 'DP-1';
          if (call.method == 'setOverlay') {
            final open = call.arguments as bool;
            overlays.add(open);
            if (open) await reply.future;
          }
          return null;
        });
        await _context(tester);
        await tester.pump();
        expect(overlays, [true]);
        expect(find.text('新建文件'), findsNothing);
        await _unmount(tester);
        if (fail) {
          reply.completeError(
            PlatformException(code: 'interaction-view-unavailable'),
          );
        } else {
          reply.complete();
        }
        await tester.pumpAndSettle();
        await tester.pump(const Duration(seconds: 11));
        expect(overlays.last, isFalse);
        expect(find.byType(DesktopView), findsNothing);
        expect(find.text('新建文件'), findsNothing);
        expect(tester.takeException(), isNull);
      },
    );
  }
}
