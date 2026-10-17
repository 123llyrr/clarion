// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/desktop/desktop_entry.dart';
import 'package:clarion_shell/desktop/desktop_layout.dart';
import 'package:clarion_shell/desktop/desktop_service.dart';
import 'package:clarion_shell/desktop/desktop_view.dart';
import 'package:clarion_shell/mac_shell_preferences.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/services/system_service.dart';
import 'package:clarion_shell/theme/shell_theme.dart';
import 'package:clarion_shell/theme/mac_shell_theme.dart';

class _Service extends DesktopService {
  _Service(String root)
    : super(
        root: root,
        command: (program, args) async {
          if (program == 'stat') return runSystemCommand(program, args);
          if (program == 'mkdir') {
            final target = args.last;
            if (args.length != 2 ||
                args.first != '--' ||
                !target.startsWith('$root/')) {
              throw StateError('Fake mkdir escaped the fixture');
            }
            if (await FileSystemEntity.type(target) !=
                FileSystemEntityType.notFound) {
              throw StateError('Fake mkdir refused an existing destination');
            }
            await Directory(target).create();
            return '';
          }
          if (program != 'mv') {
            throw StateError('No external command allowed: $program');
          }
          final source = args[args.length - 2];
          final target = args.last;
          if (!source.startsWith('$root/') || !target.startsWith('$root/')) {
            throw StateError('Fake rename escaped the fixture');
          }
          await File(source).rename(target);
          return '';
        },
      );

  final opened = <(String, bool)>[];
  final trashed = <String>[];
  final shortcuts = <(String, String?)>[];

  @override
  Future<void> openEntries(
    Iterable<DesktopEntry> entries, {
    Set<String> allowedUntrustedShortcuts = const {},
  }) async {
    for (final entry in entries) {
      await openEntry(
        entry,
        allowUntrustedShortcut: allowedUntrustedShortcuts.contains(
          entry.fullPath,
        ),
      );
    }
  }

  @override
  Future<void> trashEntries(Iterable<DesktopEntry> entries) async {
    for (final entry in entries) {
      await trashEntry(entry);
    }
  }

  @override
  Future<void> openEntry(
    DesktopEntry entry, {
    bool allowUntrustedShortcut = false,
  }) async {
    opened.add((entry.fullPath, allowUntrustedShortcut));
  }

  @override
  Future<void> trashEntry(DesktopEntry entry) async {
    trashed.add(entry.fullPath);
    await File(entry.fullPath).delete();
    await refresh();
  }

  @override
  Future<void> createShortcutInstalledApp(
    DesktopApp app, {
    String? name,
  }) async {
    shortcuts.add((app.id, name));
    await File('$directoryPath/${name ?? '${app.name}.desktop'}').writeAsString(
      '[Desktop Entry]\nType=Application\nName=${app.name}\nExec=fixture-app\n',
    );
    await refresh();
  }
}

Finder _entry(String root, String name) =>
    find.byKey(ValueKey('desktop-entry-$root/$name'));

Future<void> _keyChord(
  WidgetTester tester,
  LogicalKeyboardKey key, {
  bool command = false,
  bool shift = false,
  bool option = false,
  bool control = false,
}) async {
  final modifiers = [
    if (command) LogicalKeyboardKey.metaLeft,
    if (shift) LogicalKeyboardKey.shiftLeft,
    if (option) LogicalKeyboardKey.altLeft,
    if (control) LogicalKeyboardKey.controlLeft,
  ];
  for (final modifier in modifiers) {
    await tester.sendKeyDownEvent(modifier);
  }
  await tester.sendKeyEvent(key);
  for (final modifier in modifiers.reversed) {
    await tester.sendKeyUpEvent(modifier);
  }
}

bool _isSelected(WidgetTester tester, Finder entry) => tester
    .widget<Semantics>(
      find
          .ancestor(
            of: entry,
            matching: find.byWidgetPredicate(
              (widget) =>
                  widget is Semantics && widget.properties.selected != null,
            ),
          )
          .first,
    )
    .properties
    .selected!;

Future<void> _selectClick(
  WidgetTester tester,
  Finder entry, {
  bool command = false,
  bool shift = false,
}) async {
  if (command) await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
  if (shift) await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
  await tester.tap(entry);
  if (shift) await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
  if (command) await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
  await tester.pump(const Duration(milliseconds: 350));
}

void _expectNoSelectedNameBox() {
  expect(find.byKey(const ValueKey('desktop-name-preview')), findsNothing);
  expect(find.byKey(const ValueKey('desktop-full-name')), findsNothing);
}

class _Fixture {
  _Fixture()
    : root = Directory.systemTemp.createTempSync('clarion-desktop-ui-'),
      config = Directory.systemTemp.createTempSync(
        'clarion-desktop-ui-config-',
      ) {
    File('${root.path}/a.txt').writeAsStringSync('isolated');
    File('${root.path}/b.txt').writeAsStringSync('isolated');
    service = _Service(root.path);
    layout = DesktopLayoutStore(file: File('${config.path}/layout.json'));
  }
  final Directory root;
  final Directory config;
  late final _Service service;
  late final DesktopLayoutStore layout;
  void dispose() {
    service.dispose();
    layout.dispose();
    root.deleteSync(recursive: true);
    config.deleteSync(recursive: true);
  }
}

Future<void> _mount(
  WidgetTester tester,
  _Fixture fixture, {
  BarPosition position = BarPosition.top,
  ShellStyle style = ShellStyle.material,
  MacShellPreferences macPreferences = const MacShellPreferences(),
  String output = 'DP-1',
  List<bool>? interactions,
  List<DesktopApp> apps = const [],
  double outputScale = 1,
  double textScale = 1,
  Size physicalSize = const Size(900, 720),
}) async {
  tester.view.physicalSize = physicalSize;
  tester.view.devicePixelRatio = outputScale;
  await tester.runAsync(() async {
    await fixture.service.start();
    await fixture.layout.load();
  });
  await tester.pumpWidget(
    MaterialApp(
      theme: style == ShellStyle.macos
          ? buildMacShellTheme(Brightness.dark)
          : buildShellTheme(Brightness.dark),
      builder: (context, child) => MediaQuery(
        data: MediaQuery.of(
          context,
        ).copyWith(textScaler: TextScaler.linear(textScale)),
        child: child!,
      ),
      home: RepaintBoundary(
        key: const ValueKey('desktop-test-frame'),
        child: DesktopView(
          service: fixture.service,
          layout: fixture.layout,
          outputName: output,
          barPosition: position,
          shellStyle: style,
          macPreferences: macPreferences,
          apps: apps,
          reduceMotion: true,
          onInteractionChanged: interactions?.add,
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
  // Theme icons are SVGs and decode asynchronously; wait until the frame stops
  // gaining pixels so a baseline pixel count describes settled artwork.
  int? previous;
  for (var turn = 0; turn < 40; turn++) {
    await tester.pump();
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 20)),
    );
    final painted = await _paintedPixels(tester);
    if (previous == painted) return;
    previous = painted;
  }
}

Future<int> _paintedPixels(WidgetTester tester) async {
  final result = await tester.runAsync(() async {
    final boundary = tester.renderObject<RenderRepaintBoundary>(
      find.byKey(const ValueKey('desktop-test-frame')),
    );
    final image = await boundary.toImage(pixelRatio: 1);
    try {
      final bytes = await image.toByteData(format: ui.ImageByteFormat.rawRgba);
      if (bytes == null) throw StateError('Missing desktop frame pixels');
      var painted = 0;
      for (var offset = 3; offset < bytes.lengthInBytes; offset += 4) {
        if (bytes.getUint8(offset) != 0) painted++;
      }
      return painted;
    } finally {
      image.dispose();
    }
  });
  return result!;
}

Future<List<int>> _tilePixels(WidgetTester tester, Finder tile) async {
  final rect = tester.getRect(tile);
  final result = await tester.runAsync(() async {
    final boundary = tester.renderObject<RenderRepaintBoundary>(
      find.byKey(const ValueKey('desktop-test-frame')),
    );
    final image = await boundary.toImage(pixelRatio: 1);
    try {
      final bytes = (await image.toByteData(
        format: ui.ImageByteFormat.rawRgba,
      ))!;
      return <int>[
        for (var y = rect.top.ceil(); y < rect.bottom.floor(); y++)
          for (var x = rect.left.ceil(); x < rect.right.floor(); x++)
            for (var channel = 0; channel < 4; channel++)
              bytes.getUint8((y * image.width + x) * 4 + channel),
      ];
    } finally {
      image.dispose();
    }
  });
  return result!;
}

Future<void> _wait(WidgetTester tester, bool Function() ready) async {
  bool settled() {
    final base = find.byKey(const ValueKey('desktop-icon-layer'));
    return ready() &&
        base.evaluate().length == 1 &&
        !tester.widget<IgnorePointer>(base).ignoring;
  }

  // A directory scan may publish its entries before a queued rescan and the
  // menu-to-dialog operation finish. Keep driving real file IO until the base
  // surface is visible again; tapping hidden icons would not test input.
  for (var attempt = 0; attempt < 100 && !settled(); attempt++) {
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 20)),
    );
    await tester.pump();
  }
  expect(settled(), isTrue);
  await tester.pumpAndSettle();
}

Future<void> _context(WidgetTester tester, [Finder? entry]) async {
  if (entry == null) {
    await tester.tapAt(
      const Offset(700, 500),
      kind: PointerDeviceKind.mouse,
      buttons: kSecondaryMouseButton,
    );
  } else {
    await tester.tap(
      entry,
      kind: PointerDeviceKind.mouse,
      buttons: kSecondaryMouseButton,
    );
  }
  await tester.pumpAndSettle();
}

Finder get _selectionRectangle =>
    find.byKey(const ValueKey('desktop-selection-rectangle'));

Future<TestGesture> _mouseMarquee(
  WidgetTester tester,
  Offset start,
  Offset end, {
  int? pointer,
}) async {
  final gesture = await tester.startGesture(
    start,
    pointer: pointer,
    kind: PointerDeviceKind.mouse,
    buttons: kPrimaryMouseButton,
    view: tester.view,
  );
  await gesture.moveTo(end, view: tester.view);
  await tester.pump();
  expect(_selectionRectangle, findsOneWidget);
  return gesture;
}

Future<void> _expectMarqueeInterruption(
  WidgetTester tester,
  VoidCallback interrupt,
) async {
  final fixture = _Fixture();
  addTearDown(fixture.dispose);
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  final root = fixture.root.path;
  File('$root/c.txt').writeAsStringSync('keep the original selection');
  await _mount(tester, fixture, style: ShellStyle.macos);
  final a = _entry(root, 'a.txt');
  final b = _entry(root, 'b.txt');
  final c = _entry(root, 'c.txt');
  await _selectClick(tester, c);
  final gesture = await _mouseMarquee(
    tester,
    tester.getTopLeft(a) - const Offset(2, 2),
    tester.getBottomRight(b) + const Offset(2, 2),
  );
  expect(_isSelected(tester, a), isTrue);
  expect(_isSelected(tester, b), isTrue);
  expect(_isSelected(tester, c), isFalse);

  interrupt();
  // FocusManager applies unfocus in a microtask after the current frame. Drive
  // its scheduled focus/frame work before inspecting the cancellation paint.
  await tester.pumpAndSettle();
  expect(_selectionRectangle, findsNothing);
  expect(_isSelected(tester, a), isFalse);
  expect(_isSelected(tester, b), isFalse);
  expect(_isSelected(tester, c), isTrue);
  await tester.pump(const Duration(seconds: 3));
  expect(_selectionRectangle, findsNothing);
  expect(_isSelected(tester, c), isTrue);

  // A still-held drag must not recreate the cancelled rectangle or commit its
  // temporary selection when movement/release eventually reaches the view.
  await gesture.moveBy(const Offset(-12, -12), view: tester.view);
  await tester.pump();
  expect(_selectionRectangle, findsNothing);
  await gesture.up(view: tester.view);
  await tester.pump();
  expect(_selectionRectangle, findsNothing);
  expect(_isSelected(tester, a), isFalse);
  expect(_isSelected(tester, b), isFalse);
  expect(_isSelected(tester, c), isTrue);
  await tester.pumpWidget(const SizedBox.shrink());
}

void main() {
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

  testWidgets(
    'Mac collection selection opens the real selection and preserves Material single selection',
    (tester) async {
      final fixture = _Fixture();
      addTearDown(fixture.dispose);
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final root = fixture.root.path;
      File('$root/c.txt').writeAsStringSync('c');
      File('$root/d.txt').writeAsStringSync('d');
      await _mount(tester, fixture, style: ShellStyle.macos);
      await _selectClick(tester, _entry(root, 'a.txt'));
      await _selectClick(tester, _entry(root, 'c.txt'), command: true);
      expect(_isSelected(tester, _entry(root, 'a.txt')), isTrue);
      expect(_isSelected(tester, _entry(root, 'b.txt')), isFalse);
      await _keyChord(tester, LogicalKeyboardKey.keyO, command: true);
      expect(fixture.service.opened.map((entry) => entry.$1), [
        '$root/a.txt',
        '$root/c.txt',
      ]);
      await _selectClick(tester, _entry(root, 'a.txt'), command: true);
      expect(_isSelected(tester, _entry(root, 'a.txt')), isFalse);
      await _selectClick(tester, _entry(root, 'a.txt'));
      await _selectClick(tester, _entry(root, 'c.txt'), shift: true);
      for (final name in ['a.txt', 'b.txt', 'c.txt']) {
        expect(_isSelected(tester, _entry(root, name)), isTrue);
      }
      expect(_isSelected(tester, _entry(root, 'd.txt')), isFalse);
      await _keyChord(tester, LogicalKeyboardKey.arrowDown, shift: true);
      await tester.pump();
      expect(_isSelected(tester, _entry(root, 'd.txt')), isTrue);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('desktop-name')), findsNothing);
      await _mount(tester, fixture, style: ShellStyle.material);
      await _selectClick(tester, _entry(root, 'a.txt'), command: true);
      await _selectClick(tester, _entry(root, 'c.txt'), command: true);
      expect(_isSelected(tester, _entry(root, 'a.txt')), isFalse);
      expect(_isSelected(tester, _entry(root, 'c.txt')), isTrue);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets(
    'Mac background rectangle selects files, cancels and batches context actions',
    (tester) async {
      final fixture = _Fixture();
      addTearDown(fixture.dispose);
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final root = fixture.root.path;
      File('$root/c.txt').writeAsStringSync('c');
      await _mount(tester, fixture, style: ShellStyle.macos);
      final a = tester.getRect(_entry(root, 'a.txt'));
      final b = tester.getRect(_entry(root, 'b.txt'));
      await _selectClick(tester, _entry(root, 'c.txt'));
      final cancelled = await tester.startGesture(
        a.topLeft - const Offset(2, 2),
      );
      await cancelled.moveTo(b.bottomRight + const Offset(2, 2));
      await tester.pump();
      expect(
        find.byKey(const ValueKey('desktop-selection-rectangle')),
        findsOneWidget,
      );
      expect(_isSelected(tester, _entry(root, 'a.txt')), isTrue);
      expect(_isSelected(tester, _entry(root, 'b.txt')), isTrue);
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await cancelled.up();
      await tester.pump();
      expect(_isSelected(tester, _entry(root, 'c.txt')), isTrue);
      expect(_isSelected(tester, _entry(root, 'a.txt')), isFalse);
      final rectangle = await tester.startGesture(
        a.topLeft - const Offset(2, 2),
      );
      await rectangle.moveTo(b.bottomRight + const Offset(2, 2));
      await rectangle.up();
      await tester.pumpAndSettle();
      expect(_selectionRectangle, findsNothing);
      expect(_isSelected(tester, _entry(root, 'a.txt')), isTrue);
      expect(_isSelected(tester, _entry(root, 'b.txt')), isTrue);
      expect(_isSelected(tester, _entry(root, 'c.txt')), isFalse);
      await tester.tap(
        _entry(root, 'a.txt'),
        buttons: kSecondaryMouseButton,
        kind: PointerDeviceKind.mouse,
      );
      await tester.pumpAndSettle();
      expect(find.text('重命名'), findsNothing);
      await tester.tap(find.text('移到废纸篓'));
      await tester.pumpAndSettle();
      expect(find.textContaining('选中的 2 个'), findsOneWidget);
      await _keyChord(tester, LogicalKeyboardKey.keyA, command: true);
      await _keyChord(tester, LogicalKeyboardKey.keyO, command: true);
      expect(fixture.service.opened, isEmpty);
      await tester.tap(find.byKey(const ValueKey('desktop-trash-confirm')));
      await _wait(tester, () => fixture.service.trashed.length == 2);
      expect(fixture.service.trashed, ['$root/a.txt', '$root/b.txt']);
      expect(File('$root/c.txt').existsSync(), isTrue);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets(
    'Mac mouse marquee release clears transparent pixels without further input',
    (tester) async {
      final fixture = _Fixture();
      addTearDown(fixture.dispose);
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      File('${fixture.root.path}/a.txt').deleteSync();
      File('${fixture.root.path}/b.txt').deleteSync();
      await _mount(tester, fixture, style: ShellStyle.macos);
      expect(fixture.service.entries, isEmpty);
      expect(await _paintedPixels(tester), 0);

      final gesture = await _mouseMarquee(
        tester,
        const Offset(320, 220),
        const Offset(620, 420),
      );
      expect(await _paintedPixels(tester), greaterThan(0));
      await gesture.up(view: tester.view);
      // No click, hover or extra pointer event is allowed to clear the frame.
      await tester.pump();
      expect(_selectionRectangle, findsNothing);
      expect(await _paintedPixels(tester), 0);
      await tester.pump(const Duration(seconds: 3));
      expect(_selectionRectangle, findsNothing);
      expect(await _paintedPixels(tester), 0);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets(
    'Mac marquee native view focus loss restores the prior selection',
    (tester) async {
      await _expectMarqueeInterruption(tester, () {
        tester.binding.handleViewFocusChanged(
          ui.ViewFocusEvent(
            viewId: tester.view.viewId,
            state: ui.ViewFocusState.unfocused,
            direction: ui.ViewFocusDirection.undefined,
          ),
        );
      });
    },
  );

  testWidgets('Mac marquee keyboard focus loss restores the prior selection', (
    tester,
  ) async {
    await _expectMarqueeInterruption(tester, () {
      expect(FocusManager.instance.primaryFocus, isNotNull);
      FocusManager.instance.primaryFocus!.unfocus();
    });
  });

  testWidgets(
    'Mac marquee pointer cancellation restores selection and retires routes for the next drag',
    (tester) async {
      final fixture = _Fixture();
      addTearDown(fixture.dispose);
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final root = fixture.root.path;
      File('$root/c.txt').writeAsStringSync('keep the original selection');
      await _mount(tester, fixture, style: ShellStyle.macos);
      final a = _entry(root, 'a.txt');
      final b = _entry(root, 'b.txt');
      final c = _entry(root, 'c.txt');
      await _selectClick(tester, c);
      final start = tester.getTopLeft(a) - const Offset(2, 2);
      final end = tester.getBottomRight(b) + const Offset(2, 2);
      final router = tester.binding.pointerRouter;
      final idleRoutes = router.debugGlobalRouteCount;
      final cancelled = await _mouseMarquee(tester, start, end);
      expect(router.debugGlobalRouteCount, idleRoutes + 1);
      expect(_isSelected(tester, a), isTrue);
      expect(_isSelected(tester, c), isFalse);

      // Use an actual PointerCancel, not Escape/focus loss followed by Up.
      await cancelled.cancel();
      await tester.pumpAndSettle();
      expect(_selectionRectangle, findsNothing);
      expect(_isSelected(tester, a), isFalse);
      expect(_isSelected(tester, b), isFalse);
      expect(_isSelected(tester, c), isTrue);
      expect(router.debugGlobalRouteCount, idleRoutes);

      // A press that never crosses the drag threshold also owns a temporary
      // route; cancelling it must leave the original selection untouched.
      final candidate = await tester.startGesture(
        start,
        kind: PointerDeviceKind.mouse,
        buttons: kPrimaryMouseButton,
        view: tester.view,
      );
      expect(router.debugGlobalRouteCount, idleRoutes + 1);
      expect(_selectionRectangle, findsNothing);
      await candidate.cancel();
      await tester.pumpAndSettle();
      expect(router.debugGlobalRouteCount, idleRoutes);
      expect(_isSelected(tester, c), isTrue);

      final completed = await _mouseMarquee(tester, start, end);
      expect(router.debugGlobalRouteCount, idleRoutes + 1);
      await completed.up(view: tester.view);
      await tester.pump();
      expect(_selectionRectangle, findsNothing);
      expect(_isSelected(tester, a), isTrue);
      expect(_isSelected(tester, b), isTrue);
      expect(_isSelected(tester, c), isFalse);
      expect(router.debugGlobalRouteCount, idleRoutes);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets(
    'Mac marquee ignores movement and terminal events from another device',
    (tester) async {
      final fixture = _Fixture();
      addTearDown(fixture.dispose);
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final root = fixture.root.path;
      await _mount(tester, fixture, style: ShellStyle.macos);
      final a = _entry(root, 'a.txt');
      final b = _entry(root, 'b.txt');
      final router = tester.binding.pointerRouter;
      final idleRoutes = router.debugGlobalRouteCount;
      final owner = await _mouseMarquee(
        tester,
        tester.getTopLeft(a) - const Offset(2, 2),
        tester.getBottomRight(b) + const Offset(2, 2),
        pointer: 19102,
      );
      final rectangle = tester.getRect(_selectionRectangle);
      final unrelated = <PointerEvent>[
        PointerMoveEvent(
          viewId: tester.view.viewId,
          pointer: 19103,
          device: 2,
          kind: PointerDeviceKind.mouse,
          buttons: kPrimaryMouseButton,
          position: const Offset(100, 600),
          delta: const Offset(-100, 100),
        ),
        PointerUpEvent(
          viewId: tester.view.viewId,
          pointer: 19103,
          device: 2,
          kind: PointerDeviceKind.mouse,
        ),
        PointerCancelEvent(
          viewId: tester.view.viewId,
          pointer: 19103,
          device: 2,
          kind: PointerDeviceKind.mouse,
        ),
        PointerRemovedEvent(
          viewId: tester.view.viewId,
          device: 2,
          kind: PointerDeviceKind.mouse,
        ),
        PointerHoverEvent(
          viewId: tester.view.viewId,
          device: 2,
          kind: PointerDeviceKind.mouse,
        ),
      ];
      for (final event in unrelated) {
        // These events belong to an independent device without this view's
        // hit-test path. They still reach every registered global route.
        router.route(event);
        await tester.pump();
        expect(_selectionRectangle, findsOneWidget);
        expect(tester.getRect(_selectionRectangle), rectangle);
        expect(_isSelected(tester, a), isTrue);
        expect(_isSelected(tester, b), isTrue);
        expect(router.debugGlobalRouteCount, idleRoutes + 1);
      }
      await owner.up(view: tester.view);
      await tester.pump();
      expect(_selectionRectangle, findsNothing);
      expect(_isSelected(tester, a), isTrue);
      expect(_isSelected(tester, b), isTrue);
      expect(router.debugGlobalRouteCount, idleRoutes);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets(
    'Mac marquee lost release clears pixels and restores selection without leaking routes',
    (tester) async {
      final fixture = _Fixture();
      addTearDown(fixture.dispose);
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      await _mount(tester, fixture, style: ShellStyle.macos);
      final a = _entry(fixture.root.path, 'a.txt');
      final b = _entry(fixture.root.path, 'b.txt');
      await _selectClick(tester, a);
      final baselinePixels = await _paintedPixels(tester);
      final router = tester.binding.pointerRouter;
      final idleRoutes = router.debugGlobalRouteCount;
      const pointer = 19104;
      final gesture = await _mouseMarquee(
        tester,
        const Offset(320, 220),
        const Offset(620, 420),
        pointer: pointer,
      );
      expect(_isSelected(tester, a), isFalse);
      expect(_isSelected(tester, b), isFalse);
      expect(await _paintedPixels(tester), greaterThan(baselinePixels));
      expect(router.debugGlobalRouteCount, idleRoutes + 1);

      // Model a lost Up: this is still a Move packet for the owner, but its
      // primary button is no longer held. No Hover/Cancel/Up clears the frame.
      router.route(
        PointerMoveEvent(
          viewId: tester.view.viewId,
          pointer: pointer,
          device: 1,
          kind: PointerDeviceKind.mouse,
          buttons: 0,
          position: const Offset(640, 440),
          delta: const Offset(20, 20),
        ),
      );
      await tester.pump();
      expect(_selectionRectangle, findsNothing);
      expect(_isSelected(tester, a), isTrue);
      expect(_isSelected(tester, b), isFalse);
      expect(await _paintedPixels(tester), baselinePixels);
      expect(router.debugGlobalRouteCount, idleRoutes);
      await tester.pump(const Duration(seconds: 3));
      expect(_selectionRectangle, findsNothing);
      expect(await _paintedPixels(tester), baselinePixels);
      expect(router.debugGlobalRouteCount, idleRoutes);

      await gesture.up(view: tester.view);
      await tester.pump();
      expect(_isSelected(tester, a), isTrue);
      expect(router.debugGlobalRouteCount, idleRoutes);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets(
    'Mac marquee pointer removal clears pixels without a cancel event',
    (tester) async {
      final fixture = _Fixture();
      addTearDown(fixture.dispose);
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      File('${fixture.root.path}/a.txt').deleteSync();
      File('${fixture.root.path}/b.txt').deleteSync();
      await _mount(tester, fixture, style: ShellStyle.macos);
      const pointer = 19101;
      const end = Offset(620, 420);
      final gesture = await _mouseMarquee(
        tester,
        const Offset(320, 220),
        end,
        pointer: pointer,
      );
      expect(await _paintedPixels(tester), greaterThan(0));
      // Deliberately route device disappearance without PointerCancel/Up and
      // without a hit-test delivery to the desktop's Listener.
      tester.binding.pointerRouter.route(
        PointerRemovedEvent(
          viewId: tester.view.viewId,
          pointer: pointer,
          kind: PointerDeviceKind.mouse,
          device: 1,
          position: end,
        ),
      );
      await tester.pump();
      expect(_selectionRectangle, findsNothing);
      expect(await _paintedPixels(tester), 0);
      await tester.pump(const Duration(seconds: 3));
      expect(_selectionRectangle, findsNothing);
      expect(await _paintedPixels(tester), 0);
      // Release only after proving removal alone cleared the idle frame.
      await gesture.up(view: tester.view);
      await tester.pump();
      expect(_selectionRectangle, findsNothing);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets('Mac marquee app inactivity restores the prior selection', (
    tester,
  ) async {
    addTearDown(() {
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    });
    await _expectMarqueeInterruption(tester, () {
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    });
  });

  testWidgets('Mac marquee ignores native focus changes for another view', (
    tester,
  ) async {
    final fixture = _Fixture();
    addTearDown(fixture.dispose);
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final root = fixture.root.path;
    await _mount(tester, fixture, style: ShellStyle.macos);
    final a = _entry(root, 'a.txt');
    final b = _entry(root, 'b.txt');
    final gesture = await _mouseMarquee(
      tester,
      tester.getTopLeft(a) - const Offset(2, 2),
      tester.getBottomRight(b) + const Offset(2, 2),
    );
    for (final state in ui.ViewFocusState.values) {
      tester.binding.handleViewFocusChanged(
        ui.ViewFocusEvent(
          viewId: tester.view.viewId + 1,
          state: state,
          direction: ui.ViewFocusDirection.undefined,
        ),
      );
      await tester.pump();
      expect(_selectionRectangle, findsOneWidget);
      expect(_isSelected(tester, a), isTrue);
      expect(_isSelected(tester, b), isTrue);
    }
    await gesture.up(view: tester.view);
    await tester.pump();
    expect(_selectionRectangle, findsNothing);
    expect(_isSelected(tester, a), isTrue);
    expect(_isSelected(tester, b), isTrue);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  testWidgets(
    'Mac group drag preserves spacing, swaps collisions and cancels without persistence',
    (tester) async {
      final fixture = _Fixture();
      addTearDown(fixture.dispose);
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final root = fixture.root.path;
      File('$root/c.txt').writeAsStringSync('c');
      await _mount(tester, fixture, style: ShellStyle.macos);
      final a = _entry(root, 'a.txt');
      final b = _entry(root, 'b.txt');
      final c = _entry(root, 'c.txt');
      final first = tester.getRect(a);
      final second = tester.getRect(b);
      final third = tester.getRect(c);
      await _selectClick(tester, a);
      await _selectClick(tester, b, command: true);
      final cancelled = await tester.startGesture(first.center);
      await cancelled.moveTo(second.center);
      await tester.pump();
      expect(
        tester.getRect(b).center - tester.getRect(a).center,
        second.center - first.center,
      );
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await cancelled.up();
      await tester.pumpAndSettle();
      expect(tester.getRect(a), first);
      expect(fixture.layout.positions('DP-1'), isEmpty);
      final move = await tester.startGesture(first.center);
      await move.moveTo(second.center);
      await move.up();
      await _wait(tester, () => fixture.layout.positions('DP-1').isNotEmpty);
      expect(tester.getRect(a), second);
      expect(tester.getRect(b), third);
      expect(tester.getRect(c), first);
      await _mount(tester, fixture, style: ShellStyle.macos, output: 'eDP-1');
      await _keyChord(tester, LogicalKeyboardKey.keyO, command: true);
      expect(fixture.service.opened, isEmpty);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets(
    'Mac select-all bulk launch asks about untrusted entries before opening any file',
    (tester) async {
      final fixture = _Fixture();
      addTearDown(fixture.dispose);
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final root = fixture.root.path;
      File('$root/untrusted.desktop').writeAsStringSync(
        '[Desktop Entry]\nType=Application\nName=Untrusted\nExec=never-run\n',
      );
      await _mount(tester, fixture, style: ShellStyle.macos);
      await _selectClick(tester, _entry(root, 'a.txt'));
      await _keyChord(tester, LogicalKeyboardKey.keyA, command: true);
      await _keyChord(tester, LogicalKeyboardKey.keyO, command: true);
      await tester.pumpAndSettle();
      expect(fixture.service.opened, isEmpty);
      await tester.tap(find.text('取消'));
      await tester.pumpAndSettle();
      expect(fixture.service.opened, isEmpty);
      await _keyChord(tester, LogicalKeyboardKey.keyO, command: true);
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const ValueKey('desktop-trust-confirm')));
      await tester.pumpAndSettle();
      expect(fixture.service.opened.toSet(), {
        ('$root/a.txt', false),
        ('$root/b.txt', false),
        ('$root/untrusted.desktop', true),
      });
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets(
    'Mac select-all stays on the visible page and page changes clear collection actions',
    (tester) async {
      final fixture = _Fixture();
      addTearDown(fixture.dispose);
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final root = fixture.root.path;
      for (var i = 0; i < 8; i++) {
        File('$root/item-$i.txt').writeAsStringSync('item');
      }
      await _mount(
        tester,
        fixture,
        style: ShellStyle.macos,
        physicalSize: const Size(360, 420),
      );
      final firstPage = fixture.service.entries
          .where(
            (entry) => find
                .byKey(ValueKey('desktop-entry-${entry.fullPath}'))
                .evaluate()
                .isNotEmpty,
          )
          .map((entry) => entry.fullPath)
          .toSet();
      await _selectClick(
        tester,
        find.byKey(ValueKey('desktop-entry-${firstPage.first}')),
      );
      await _keyChord(tester, LogicalKeyboardKey.keyA, command: true);
      await _keyChord(tester, LogicalKeyboardKey.keyO, command: true);
      expect(
        fixture.service.opened.map((entry) => entry.$1).toSet(),
        firstPage,
      );
      await tester.tap(find.byKey(const ValueKey('desktop-next-page')));
      await tester.pumpAndSettle();
      fixture.service.opened.clear();
      await _keyChord(tester, LogicalKeyboardKey.keyO, command: true);
      expect(fixture.service.opened, isEmpty);
      await _keyChord(tester, LogicalKeyboardKey.keyA, command: true);
      await _keyChord(tester, LogicalKeyboardKey.keyO, command: true);
      expect(fixture.service.opened, isNotEmpty);
      expect(
        fixture.service.opened
            .map((entry) => entry.$1)
            .toSet()
            .intersection(firstPage),
        isEmpty,
      );
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets('switching shell preserves desktop selection and file actions', (
    tester,
  ) async {
    final fixture = _Fixture();
    addTearDown(fixture.dispose);
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    Directory('${fixture.root.path}/folder').createSync();
    await _mount(tester, fixture, position: BarPosition.bottom);
    await tester.tap(_entry(fixture.root.path, 'a.txt'));
    await tester.pump(const Duration(milliseconds: 350));
    await tester.pumpAndSettle();
    expect(_isSelected(tester, _entry(fixture.root.path, 'a.txt')), isTrue);
    await _mount(
      tester,
      fixture,
      position: BarPosition.bottom,
      style: ShellStyle.macos,
    );
    expect(_isSelected(tester, _entry(fixture.root.path, 'a.txt')), isTrue);
    await _keyChord(tester, LogicalKeyboardKey.keyO, command: true);
    await tester.pumpAndSettle();
    expect(fixture.service.opened.single.$1, '${fixture.root.path}/a.txt');
    expect(tester.takeException(), isNull);
    await _mount(tester, fixture, position: BarPosition.bottom);
    expect(_isSelected(tester, _entry(fixture.root.path, 'a.txt')), isTrue);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  for (final dockPosition in MacDockPosition.values) {
    testWidgets(
      '$dockPosition desktop icons stay usable after Dock geometry changes',
      (tester) async {
        final fixture = _Fixture();
        addTearDown(fixture.dispose);
        addTearDown(tester.view.resetPhysicalSize);
        addTearDown(tester.view.resetDevicePixelRatio);
        const output = Size(900, 720);
        for (final size in [32.0, 64.0]) {
          for (final hidden in [false, true]) {
            final preferences = MacShellPreferences(
              dockPosition: dockPosition,
              dockSize: size,
              dockAutoHide: hidden,
              menuBarAutoHide: hidden,
            );
            await _mount(
              tester,
              fixture,
              style: ShellStyle.macos,
              macPreferences: preferences,
              physicalSize: output,
            );
            final dock = hidden ? 0.0 : size + 16;
            final safeArea = Rect.fromLTRB(
              dockPosition == MacDockPosition.left ? dock : 0,
              hidden ? 0 : 22,
              output.width - (dockPosition == MacDockPosition.right ? dock : 0),
              output.height -
                  (dockPosition == MacDockPosition.bottom ? dock : 0),
            );
            for (final name in ['a.txt', 'b.txt']) {
              final entry = _entry(fixture.root.path, name);
              final rect = tester.getRect(entry);
              expect(safeArea.contains(rect.topLeft), isTrue);
              expect(safeArea.contains(rect.bottomRight), isTrue);
              await tester.tap(entry);
              await tester.pump(const Duration(milliseconds: 350));
              await tester.pumpAndSettle();
              expect(_isSelected(tester, entry), isTrue);
              await _keyChord(tester, LogicalKeyboardKey.keyO, command: true);
              await tester.pumpAndSettle();
              expect(
                fixture.service.opened.last.$1,
                '${fixture.root.path}/$name',
              );
            }
          }
        }
        expect(fixture.service.opened.length, 8);
        expect(tester.takeException(), isNull);
        await tester.pumpWidget(const SizedBox.shrink());
      },
    );
  }

  const longNames = [
    '2026 年十月项目资料与系统配置中心改进记录 完整中文文件名称需要正确显示与便捷查看 的最终审阅版本.txt',
    'Quarterly desktop stability review notes including screenshots workspace layouts and graphical performance findings with comments and recommendations for the final engineering review.txt',
    'Version_LongDesktopFilenameLongDesktopFilenameLongDesktopFilenameLongDesktopFilenameLongDesktopFilenameLongDesktopFilenameLongDesktopFilenameLongDesktopFilenameLongDesktopFilename.txt',
  ];
  for (final position in BarPosition.values) {
    testWidgets('full names stay readable at $position across output scales', (
      tester,
    ) async {
      for (final outputScale in [1.0, 1.25, 1.5]) {
        final fixture = _Fixture();
        addTearDown(fixture.dispose);
        addTearDown(tester.view.resetPhysicalSize);
        addTearDown(tester.view.resetDevicePixelRatio);
        for (final file in fixture.root.listSync()) {
          file.deleteSync();
        }
        for (final name in longNames) {
          File('${fixture.root.path}/$name').writeAsStringSync('fixture');
        }
        final interactions = <bool>[];
        final textScale = outputScale == 1.5
            ? 2.0
            : outputScale == 1.25
            ? 1.5
            : 1.0;
        await _mount(
          tester,
          fixture,
          position: position,
          interactions: interactions,
          outputScale: outputScale,
          textScale: textScale,
        );
        final logicalSize = Size(900 / outputScale, 720 / outputScale);
        final original = {
          for (final name in longNames)
            name: tester.getRect(_entry(fixture.root.path, name)),
        };
        for (final name in longNames) {
          final label = tester.widget<Text>(
            find.byKey(ValueKey('desktop-label-${fixture.root.path}/$name')),
          );
          expect(label.maxLines, 3);
          expect(label.style!.fontSize, 13);
          expect(
            find.descendant(
              of: _entry(fixture.root.path, name),
              matching: find.byType(FittedBox),
            ),
            findsNothing,
          );
        }
        await tester.tapAt(
          Offset(logicalSize.width - 8, logicalSize.height - 48),
        );
        await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
        await tester.pumpAndSettle();
        expect(
          _isSelected(tester, _entry(fixture.root.path, longNames.first)),
          isTrue,
        );
        _expectNoSelectedNameBox();

        for (final name in longNames) {
          await tester.tap(_entry(fixture.root.path, name));
          await tester.pump(const Duration(milliseconds: 400));
          expect(_isSelected(tester, _entry(fixture.root.path, name)), isTrue);
          _expectNoSelectedNameBox();
          for (final other in longNames) {
            expect(
              _isSelected(tester, _entry(fixture.root.path, other)),
              other == name,
            );
            expect(
              tester.getRect(_entry(fixture.root.path, other)),
              original[other],
            );
          }
        }

        await tester.sendKeyEvent(LogicalKeyboardKey.escape);
        await tester.pumpAndSettle();
        final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
        await mouse.addPointer(location: const Offset(1, 1));
        await mouse.moveTo(
          tester.getCenter(_entry(fixture.root.path, longNames.first)),
        );
        await tester.pump(const Duration(milliseconds: 700));
        await tester.pumpAndSettle();
        final hoverName = find.byKey(
          ValueKey(
            'desktop-hover-name-${fixture.root.path}/${longNames.first}',
          ),
        );
        expect(hoverName, findsOneWidget);
        final hoverText = tester.widget<Text>(hoverName);
        expect(hoverText.data, longNames.first);
        expect(hoverText.maxLines, isNull);
        expect(
          tester.renderObject<RenderParagraph>(hoverName).didExceedMaxLines,
          isFalse,
        );
        expect(hoverText.textScaler!.scale(14), 14 * textScale);
        final hoverRect = tester.getRect(
          find.byKey(
            ValueKey(
              'desktop-hover-name-viewport-${fixture.root.path}/${longNames.first}',
            ),
          ),
        );
        expect(
          hoverRect.top,
          greaterThanOrEqualTo(position == BarPosition.top ? 40 : 0),
        );
        expect(
          hoverRect.bottom,
          lessThanOrEqualTo(
            logicalSize.height - (position == BarPosition.bottom ? 40 : 0),
          ),
        );
        expect(interactions, isEmpty);
        expect(fixture.service.opened, isEmpty);
        await mouse.removePointer();
        Tooltip.dismissAllToolTips();
        await tester.pumpAndSettle();
        expect(tester.takeException(), isNull);
        await tester.pumpWidget(const SizedBox.shrink());
      }
    });

    testWidgets(
      'large names remain hover-scrollable and preserve drag cancellation at $position',
      (tester) async {
        final fixture = _Fixture();
        addTearDown(fixture.dispose);
        addTearDown(tester.view.resetPhysicalSize);
        addTearDown(tester.view.resetDevicePixelRatio);
        for (final file in fixture.root.listSync()) {
          file.deleteSync();
        }
        for (final name in longNames.take(2)) {
          File('${fixture.root.path}/$name').writeAsStringSync('fixture');
        }
        await _mount(
          tester,
          fixture,
          position: position,
          textScale: 2,
          physicalSize: const Size(360, 480),
        );
        final entry = _entry(fixture.root.path, longNames.first);
        await tester.tap(entry);
        await tester.pump(const Duration(milliseconds: 400));
        expect(_isSelected(tester, entry), isTrue);
        _expectNoSelectedNameBox();
        await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
        await tester.pumpAndSettle();
        final selected = _entry(fixture.root.path, longNames[1]);
        expect(_isSelected(tester, selected), isTrue);
        expect(_isSelected(tester, entry), isFalse);
        _expectNoSelectedNameBox();
        final before = tester.getRect(selected);
        final drag = await tester.startGesture(before.center);
        await drag.moveBy(const Offset(-40, -40));
        await tester.pump();
        expect(tester.getRect(selected), isNot(before));
        _expectNoSelectedNameBox();
        await drag.cancel();
        await tester.pumpAndSettle();
        expect(tester.getRect(selected), before);
        await tester.sendKeyEvent(LogicalKeyboardKey.escape);
        await tester.pumpAndSettle();
        final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
        await mouse.addPointer(location: const Offset(1, 1));
        await mouse.moveTo(tester.getCenter(entry));
        await tester.pump(const Duration(milliseconds: 700));
        await tester.pumpAndSettle();
        final hover = find.byKey(
          ValueKey(
            'desktop-hover-name-${fixture.root.path}/${longNames.first}',
          ),
        );
        expect(hover, findsOneWidget);
        expect(tester.widget<Text>(hover).data, longNames.first);
        expect(tester.widget<Text>(hover).maxLines, isNull);
        final viewport = find.byKey(
          ValueKey(
            'desktop-hover-name-viewport-${fixture.root.path}/${longNames.first}',
          ),
        );
        final hoverRect = tester.getRect(viewport);
        expect(
          hoverRect.top,
          greaterThanOrEqualTo(position == BarPosition.top ? 40 : 0),
        );
        expect(
          hoverRect.bottom,
          lessThanOrEqualTo(480 - (position == BarPosition.bottom ? 40 : 0)),
        );
        expect(hoverRect.height, lessThanOrEqualTo(180));
        expect(
          find.byWidgetPredicate(
            (widget) =>
                widget is Semantics &&
                widget.properties.label == longNames.first,
          ),
          findsOneWidget,
        );
        await mouse.moveTo(hoverRect.center);
        await tester.pump(const Duration(milliseconds: 100));
        final hoverPosition = tester
            .state<ScrollableState>(
              find.descendant(of: viewport, matching: find.byType(Scrollable)),
            )
            .position;
        expect(hoverPosition.maxScrollExtent, greaterThan(0));
        await tester.sendEventToBinding(
          PointerScrollEvent(
            position: hoverRect.center,
            scrollDelta: const Offset(0, 10000),
          ),
        );
        await tester.pumpAndSettle();
        expect(hoverPosition.pixels, hoverPosition.maxScrollExtent);
        expect(tester.getRect(hover).bottom, closeTo(hoverRect.bottom, .01));
        await mouse.removePointer();
        Tooltip.dismissAllToolTips();
        await tester.pumpAndSettle();
        expect(tester.takeException(), isNull);
        await tester.pumpWidget(const SizedBox.shrink());
      },
    );
  }

  testWidgets(
    'paging clears selection before keyboard actions on an invisible file',
    (tester) async {
      final fixture = _Fixture();
      addTearDown(fixture.dispose);
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      for (var index = 0; index < 28; index++) {
        File('${fixture.root.path}/z-$index.txt').writeAsStringSync('fixture');
      }
      await _mount(tester, fixture);
      final first = _entry(fixture.root.path, 'a.txt');
      await tester.tap(first);
      await tester.pump(const Duration(milliseconds: 400));
      expect(_isSelected(tester, first), isTrue);
      _expectNoSelectedNameBox();
      await tester.tap(find.byKey(const ValueKey('desktop-next-page')));
      await tester.pumpAndSettle();
      expect(first, findsNothing);
      for (final key in [
        LogicalKeyboardKey.enter,
        LogicalKeyboardKey.f2,
        LogicalKeyboardKey.delete,
      ]) {
        await tester.sendKeyEvent(key);
        await tester.pumpAndSettle();
      }
      expect(fixture.service.opened, isEmpty);
      expect(fixture.service.trashed, isEmpty);
      expect(find.byType(AlertDialog), findsNothing);
      _expectNoSelectedNameBox();
      await tester.tap(find.byKey(const ValueKey('desktop-previous-page')));
      await tester.pumpAndSettle();
      await tester.tap(first);
      await tester.pump(const Duration(milliseconds: 400));
      await tester.sendEventToBinding(
        const PointerScrollEvent(
          position: Offset(700, 300),
          scrollDelta: Offset(0, 100),
        ),
      );
      await tester.pumpAndSettle();
      expect(first, findsNothing);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pump();
      expect(fixture.service.opened, isEmpty);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets(
    'removing the final icon retains a clearing paint with zero-alpha pixels',
    (tester) async {
      final fixture = _Fixture();
      addTearDown(fixture.dispose);
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      await tester.runAsync(() => File('${fixture.root.path}/a.txt').delete());
      await _mount(tester, fixture);
      expect(fixture.service.entries, hasLength(1));
      expect(await _paintedPixels(tester), greaterThan(0));
      await tester.runAsync(() async {
        await File('${fixture.root.path}/b.txt').delete();
        await fixture.service.refresh();
      });
      await tester.pumpAndSettle();
      expect(fixture.service.entries, isEmpty);
      expect(_entry(fixture.root.path, 'b.txt'), findsNothing);
      final clear = tester.widget<CustomPaint>(
        find.byKey(const ValueKey('desktop-clear-frame')),
      );
      expect(clear.painter, isNotNull);
      expect(await _paintedPixels(tester), 0);

      // Validate the operation itself clears existing pixels, instead of
      // producing an invisible srcOver no-op or a low-alpha wallpaper tint.
      final recorder = ui.PictureRecorder();
      final canvas = ui.Canvas(recorder);
      canvas.drawColor(Colors.red, ui.BlendMode.src);
      clear.painter!.paint(canvas, const Size(8, 8));
      final picture = recorder.endRecording();
      try {
        await tester.runAsync(() async {
          final image = await picture.toImage(8, 8);
          try {
            final bytes = await image.toByteData(
              format: ui.ImageByteFormat.rawRgba,
            );
            expect(bytes, isNotNull);
            expect(bytes!.buffer.asUint8List(), everyElement(0));
          } finally {
            image.dispose();
          }
        });
      } finally {
        picture.dispose();
      }
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets('Mac Return renames and Command chords open the selected item', (
    tester,
  ) async {
    final fixture = _Fixture();
    addTearDown(fixture.dispose);
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final root = fixture.root.path;
    await _mount(tester, fixture, style: ShellStyle.macos);
    final first = _entry(root, 'a.txt');
    final second = _entry(root, 'b.txt');
    await tester.tap(first);
    await tester.pump(const Duration(milliseconds: 350));

    await _keyChord(tester, LogicalKeyboardKey.keyO, command: true);
    await _keyChord(tester, LogicalKeyboardKey.arrowDown, command: true);
    await tester.pump();
    expect(fixture.service.opened, [
      ('$root/a.txt', false),
      ('$root/a.txt', false),
    ]);
    expect(_isSelected(tester, first), isTrue);
    expect(_isSelected(tester, second), isFalse);

    await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
    await tester.pump();
    expect(_isSelected(tester, second), isTrue);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pumpAndSettle();
    expect(find.byType(AlertDialog), findsNothing);
    expect(find.byKey(const ValueKey('desktop-inline-name')), findsOneWidget);
    expect(fixture.service.opened, hasLength(2));
    await _keyChord(tester, LogicalKeyboardKey.keyO, command: true);
    await _keyChord(tester, LogicalKeyboardKey.backspace, command: true);
    expect(fixture.service.opened, hasLength(2));
    expect(fixture.service.trashed, isEmpty);
    await tester.enterText(
      find.byKey(const ValueKey('desktop-inline-name')),
      'renamed.txt',
    );
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await _wait(
      tester,
      () =>
          !fixture.service.busy &&
          fixture.layout.positions('DP-1').containsKey('$root/renamed.txt'),
    );
    expect(_isSelected(tester, _entry(root, 'renamed.txt')), isTrue);
    expect(File('$root/b.txt').existsSync(), isFalse);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  testWidgets('Mac destructive and modified chords do not fall through', (
    tester,
  ) async {
    final fixture = _Fixture();
    addTearDown(fixture.dispose);
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final root = fixture.root.path;
    await _mount(tester, fixture, style: ShellStyle.macos);
    final first = _entry(root, 'a.txt');
    await tester.tap(first);
    await tester.pump(const Duration(milliseconds: 350));

    for (final key in [
      LogicalKeyboardKey.delete,
      LogicalKeyboardKey.backspace,
      LogicalKeyboardKey.f2,
    ]) {
      await tester.sendKeyEvent(key);
    }
    await _keyChord(tester, LogicalKeyboardKey.enter, option: true);
    await _keyChord(tester, LogicalKeyboardKey.enter, shift: true);
    await _keyChord(tester, LogicalKeyboardKey.enter, command: true);
    await _keyChord(
      tester,
      LogicalKeyboardKey.keyO,
      command: true,
      option: true,
    );
    await _keyChord(
      tester,
      LogicalKeyboardKey.arrowDown,
      command: true,
      shift: true,
    );
    await _keyChord(tester, LogicalKeyboardKey.arrowDown, control: true);
    await _keyChord(
      tester,
      LogicalKeyboardKey.backspace,
      command: true,
      shift: true,
    );
    await _keyChord(
      tester,
      LogicalKeyboardKey.backspace,
      command: true,
      option: true,
    );
    await _keyChord(tester, LogicalKeyboardKey.delete, command: true);
    await tester.pumpAndSettle();
    expect(find.byType(AlertDialog), findsNothing);
    expect(_isSelected(tester, first), isTrue);
    expect(fixture.service.opened, isEmpty);
    expect(fixture.service.trashed, isEmpty);

    await _keyChord(tester, LogicalKeyboardKey.backspace, command: true);
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('desktop-trash-confirm')), findsOneWidget);
    await tester.tap(find.text('取消'));
    await tester.pumpAndSettle();
    expect(fixture.service.trashed, isEmpty);
    expect(File('$root/a.txt').existsSync(), isTrue);
    await _keyChord(tester, LogicalKeyboardKey.backspace, command: true);
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('desktop-trash-confirm')));
    await _wait(
      tester,
      () => fixture.service.entries.every((entry) => entry.name != 'a.txt'),
    );
    expect(fixture.service.trashed, ['$root/a.txt']);
    expect(File('$root/b.txt').existsSync(), isTrue);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  testWidgets('Mac Shift Command N creates a folder without a selection', (
    tester,
  ) async {
    final fixture = _Fixture();
    addTearDown(fixture.dispose);
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await _mount(tester, fixture, style: ShellStyle.macos);
    await tester.tapAt(const Offset(700, 300));
    await tester.pump();
    await _keyChord(tester, LogicalKeyboardKey.keyN, command: true);
    await _keyChord(
      tester,
      LogicalKeyboardKey.keyN,
      command: true,
      shift: true,
      option: true,
    );
    await tester.pump();
    expect(find.byType(AlertDialog), findsNothing);
    await _keyChord(
      tester,
      LogicalKeyboardKey.keyN,
      command: true,
      shift: true,
    );
    await tester.pumpAndSettle();
    expect(find.text('新建文件夹'), findsOneWidget);
    await tester.enterText(
      find.byKey(const ValueKey('desktop-name')),
      'Keyboard Folder',
    );
    await tester.tap(find.byKey(const ValueKey('desktop-name-confirm')));
    await _wait(
      tester,
      () => fixture.service.entries.any(
        (entry) => entry.name == 'Keyboard Folder',
      ),
    );
    expect(
      Directory('${fixture.root.path}/Keyboard Folder').existsSync(),
      isTrue,
    );
    expect(fixture.service.opened, isEmpty);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  testWidgets('Mac file chords ignore absent selection and repeated opens', (
    tester,
  ) async {
    final fixture = _Fixture();
    addTearDown(fixture.dispose);
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await _mount(tester, fixture, style: ShellStyle.macos);
    await tester.tapAt(const Offset(700, 300));
    await tester.pump();
    await _keyChord(tester, LogicalKeyboardKey.keyO, command: true);
    await _keyChord(tester, LogicalKeyboardKey.arrowDown, command: true);
    await _keyChord(tester, LogicalKeyboardKey.backspace, command: true);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pump();
    expect(find.byType(AlertDialog), findsNothing);
    expect(fixture.service.opened, isEmpty);

    final first = _entry(fixture.root.path, 'a.txt');
    await tester.tap(first);
    await tester.pump(const Duration(milliseconds: 350));
    await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
    await tester.sendKeyDownEvent(LogicalKeyboardKey.arrowDown);
    await tester.sendKeyRepeatEvent(LogicalKeyboardKey.arrowDown);
    await tester.sendKeyRepeatEvent(LogicalKeyboardKey.arrowDown);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.arrowDown);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
    await tester.pump();
    expect(fixture.service.opened, [('${fixture.root.path}/a.txt', false)]);
    expect(_isSelected(tester, first), isTrue);
    expect(fixture.service.trashed, isEmpty);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  testWidgets(
    'selection, double click, keyboard open, rename and confirmed trash',
    (tester) async {
      final fixture = _Fixture();
      addTearDown(fixture.dispose);
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final root = fixture.root.path;
      await _mount(tester, fixture);
      await tester.tap(_entry(root, 'a.txt'));
      await tester.pump(const Duration(milliseconds: 350));
      expect(fixture.service.opened, isEmpty);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pump();
      expect(fixture.service.opened, [("$root/a.txt", false)]);
      await tester.tap(_entry(root, 'b.txt'));
      await tester.pump(const Duration(milliseconds: 50));
      await tester.tap(_entry(root, 'b.txt'));
      await tester.pumpAndSettle();
      expect(fixture.service.opened.last.$1, '$root/b.txt');

      await tester.sendKeyEvent(LogicalKeyboardKey.f2);
      await tester.pumpAndSettle();
      await tester.enterText(
        find.byKey(const ValueKey('desktop-name')),
        'renamed.txt',
      );
      await tester.tap(find.byKey(const ValueKey('desktop-name-confirm')));
      await _wait(
        tester,
        () =>
            !fixture.service.busy &&
            fixture.layout.positions('DP-1').containsKey('$root/renamed.txt'),
      );
      expect(_entry(root, 'renamed.txt'), findsOneWidget);
      expect(File('$root/b.txt').existsSync(), isFalse);
      expect(File('$root/a.txt').existsSync(), isTrue);

      await tester.sendKeyEvent(LogicalKeyboardKey.delete);
      await tester.pumpAndSettle();
      await tester.tap(find.text('取消'));
      await tester.pumpAndSettle();
      expect(fixture.service.trashed, isEmpty);
      await tester.sendKeyEvent(LogicalKeyboardKey.delete);
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const ValueKey('desktop-trash-confirm')));
      await _wait(
        tester,
        () => fixture.service.entries.every(
          (entry) => entry.name != 'renamed.txt',
        ),
      );
      expect(fixture.service.trashed, ['$root/renamed.txt']);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets(
    'context menu to dialog retains one overlay interval and cancellation is inert',
    (tester) async {
      final fixture = _Fixture();
      addTearDown(fixture.dispose);
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final changes = <bool>[];
      await _mount(tester, fixture, interactions: changes);
      final original = tester.element(_entry(fixture.root.path, 'a.txt'));
      await _context(tester);
      expect(changes, [true]);
      expect(
        tester
            .widget<IgnorePointer>(
              find.byKey(const ValueKey('desktop-icon-layer')),
            )
            .ignoring,
        isTrue,
      );
      await tester.tap(find.text('新建文件'));
      await tester.pumpAndSettle();
      expect(changes, [
        true,
      ], reason: 'menu-to-dialog does not lower the native surface');
      await tester.tap(find.text('取消'));
      await tester.pumpAndSettle();
      expect(changes, [true, false]);
      expect(fixture.service.entries.length, 2);
      expect(
        tester.element(_entry(fixture.root.path, 'a.txt')),
        same(original),
      );
      await _context(tester);
      await tester.tap(find.text('新建文件夹'));
      await tester.pumpAndSettle();
      await tester.enterText(find.byKey(const ValueKey('desktop-name')), '资料');
      await tester.tap(find.byKey(const ValueKey('desktop-name-confirm')));
      await _wait(
        tester,
        () =>
            !fixture.service.busy &&
            fixture.service.entries.any((entry) => entry.name == '资料'),
      );
      expect(Directory('${fixture.root.path}/资料').existsSync(), isTrue);
      expect(changes, [true, false, true, false]);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  for (final style in ShellStyle.values) {
    testWidgets('right click keeps $style desktop icon pixels and position', (
      tester,
    ) async {
      final fixture = _Fixture();
      addTearDown(fixture.dispose);
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final changes = <bool>[];
      await _mount(tester, fixture, style: style, interactions: changes);
      final tile = _entry(fixture.root.path, 'a.txt');
      final element = tester.element(tile);
      final rect = tester.getRect(tile);
      final pixels = await _tilePixels(tester, tile);
      expect(pixels, contains(isNonZero));
      await _context(tester);
      expect(await _tilePixels(tester, tile), pixels);
      expect(tester.getRect(tile), rect);
      expect(tester.element(tile), same(element));
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      expect(changes, [true, false]);
      expect(await _tilePixels(tester, tile), pixels);
      expect(fixture.service.opened, isEmpty);
      expect(fixture.service.trashed, isEmpty);
      expect(fixture.layout.positions('DP-1'), isEmpty);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    });
  }

  testWidgets(
    'macOS menu uses compact single selection and keyboard activation',
    (tester) async {
      final fixture = _Fixture();
      addTearDown(fixture.dispose);
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      await _mount(tester, fixture, style: ShellStyle.macos);
      await _context(tester);
      final entries = find.byWidgetPredicate(
        (widget) =>
            widget.key is ValueKey<String> &&
            (widget.key! as ValueKey<String>).value.startsWith('desktop-menu-'),
      );
      expect(entries, findsNWidgets(7));
      for (final element in entries.evaluate()) {
        expect(tester.getSize(find.byWidget(element.widget)).height, 26);
      }
      expect(
        tester.getTopLeft(find.text('新建文件夹')).dy,
        lessThan(tester.getTopLeft(find.text('新建文件')).dy),
      );
      final mouse = TestPointer(7000, PointerDeviceKind.mouse, 7000);
      await tester.sendEventToBinding(
        mouse.addPointer(location: Offset.zero, view: tester.view),
      );
      await tester.sendEventToBinding(
        mouse.hover(tester.getCenter(find.text('新建文件夹')), view: tester.view),
      );
      await tester.pumpAndSettle();
      expect(
        DefaultTextStyle.of(tester.element(find.text('新建文件夹'))).style.color,
        Colors.white,
      );
      await tester.sendEventToBinding(
        mouse.hover(tester.getCenter(find.text('新建文件')), view: tester.view),
      );
      await tester.pumpAndSettle();
      expect(
        DefaultTextStyle.of(tester.element(find.text('新建文件'))).style.color,
        Colors.white,
      );
      expect(
        DefaultTextStyle.of(tester.element(find.text('新建文件夹'))).style.color,
        isNot(Colors.white),
      );
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('desktop-name')), findsOneWidget);
      expect(find.text('新建文件'), findsOneWidget);
      await tester.tap(find.text('取消'));
      await tester.pumpAndSettle();
      expect(fixture.service.entries, hasLength(2));
      await tester.sendEventToBinding(mouse.removePointer(view: tester.view));
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets(
    'drag swaps cells, cancel restores, refresh preserves keys, outputs remain independent',
    (tester) async {
      final fixture = _Fixture();
      addTearDown(fixture.dispose);
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final root = fixture.root.path;
      await _mount(tester, fixture);
      final a = _entry(root, 'a.txt');
      final b = _entry(root, 'b.txt');
      final initialA = tester.getRect(a);
      final initialB = tester.getRect(b);
      final element = tester.element(a);
      final cancel = await tester.startGesture(initialA.center);
      await cancel.moveTo(initialB.center);
      await tester.pump();
      await cancel.cancel();
      await tester.pumpAndSettle();
      expect(tester.getRect(a), initialA);
      expect(fixture.layout.positions('DP-1'), isEmpty);
      final move = await tester.startGesture(initialA.center);
      await move.moveTo(initialB.center);
      await tester.pump();
      await move.up();
      await _wait(tester, () => fixture.layout.positions('DP-1').length == 2);
      expect(tester.getRect(a), initialB);
      expect(tester.getRect(b), initialA);
      await tester.runAsync(() async {
        File('$root/0-new.txt').writeAsStringSync('external fixture');
        await fixture.service.refresh();
      });
      await tester.pumpAndSettle();
      expect(tester.element(a), same(element));
      expect(tester.getRect(a), initialB);
      await _mount(
        tester,
        fixture,
        output: 'eDP-1',
        position: BarPosition.bottom,
      );
      expect(fixture.layout.positions('eDP-1'), isEmpty);
      for (final entry in fixture.service.entries) {
        final rect = tester.getRect(_entry(root, entry.name));
        expect(rect.top, greaterThanOrEqualTo(0));
        expect(rect.bottom, lessThanOrEqualTo(680));
      }
      expect(
        fixture.layout.positions('DP-1')['$root/a.txt'],
        const DesktopCell(0, 1),
      );
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets(
    'shortcuts use application selection or existing paths and unknown launchers ask first',
    (tester) async {
      final fixture = _Fixture();
      addTearDown(fixture.dispose);
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      const app = DesktopApp(
        id: 'fixture-app',
        name: '测试应用',
        desktopFile: '/isolated/catalog.desktop',
      );
      await _mount(tester, fixture, apps: const [app]);
      await _context(tester);
      await tester.tap(find.text('新建快捷方式'));
      await tester.pumpAndSettle();
      await tester.enterText(
        find.byKey(const ValueKey('desktop-shortcut-name')),
        '应用链接',
      );
      await tester.tap(find.byKey(const ValueKey('desktop-shortcut-confirm')));
      await _wait(
        tester,
        () => fixture.service.entries.any(
          (entry) => entry.name == '应用链接.desktop',
        ),
      );
      expect(fixture.service.shortcuts, [('fixture-app', '应用链接.desktop')]);
      final shortcut = _entry(fixture.root.path, '应用链接.desktop');
      await tester.tap(shortcut);
      await tester.pump(const Duration(milliseconds: 350));
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pumpAndSettle();
      expect(find.text('运行桌面快捷方式？'), findsOneWidget);
      expect(fixture.service.opened, isEmpty);
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      expect(fixture.service.opened, isEmpty);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const ValueKey('desktop-trust-confirm')));
      await tester.pumpAndSettle();
      expect(fixture.service.opened.single.$2, isTrue);

      await _context(tester);
      await tester.tap(find.text('新建快捷方式'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('本地文件'));
      await tester.pumpAndSettle();
      await tester.enterText(
        find.byKey(const ValueKey('desktop-shortcut-path')),
        'arbitrary command',
      );
      await tester.tap(find.byKey(const ValueKey('desktop-shortcut-confirm')));
      await tester.pump();
      expect(find.text('请输入本地文件或文件夹的绝对路径'), findsOneWidget);
      await tester.enterText(
        find.byKey(const ValueKey('desktop-shortcut-path')),
        '${fixture.root.path}/a.txt',
      );
      await tester.enterText(
        find.byKey(const ValueKey('desktop-shortcut-name')),
        '文件链接',
      );
      await tester.tap(find.byKey(const ValueKey('desktop-shortcut-confirm')));
      await _wait(
        tester,
        () => fixture.service.entries.any((entry) => entry.name == '文件链接'),
      );
      expect(Link('${fixture.root.path}/文件链接').existsSync(), isTrue);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  Future<_Fixture> mountTools(
    WidgetTester tester, {
    Map<String, String> extraFiles = const {},
    List<bool>? interactions,
  }) async {
    final fixture = _Fixture();
    addTearDown(fixture.dispose);
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    for (final entry in extraFiles.entries) {
      File('${fixture.root.path}/${entry.key}').writeAsStringSync(entry.value);
    }
    await _mount(
      tester,
      fixture,
      style: ShellStyle.macos,
      interactions: interactions,
    );
    return fixture;
  }

  Finder tool(String key) => find.byKey(ValueKey(key));

  Future<void> inspectReady(WidgetTester tester) async {
    for (
      var i = 0;
      i < 100 && tool('desktop-inspector-loading').evaluate().isNotEmpty;
      i++
    ) {
      await tester.runAsync(
        () => Future<void>.delayed(const Duration(milliseconds: 20)),
      );
      // Process.start schedules its actual spawn with Timer.run. A pump with
      // no duration only flushes microtasks and leaves that fake-zone timer
      // pending forever; real IO also needs the runAsync event turn above.
      await tester.pump(const Duration(milliseconds: 1));
    }
    expect(tool('desktop-inspector-loading'), findsNothing);
    expect(tool('desktop-inspector-error'), findsNothing);
  }

  testWidgets('desktop Cmd I reads real metadata and preserves selection', (
    tester,
  ) async {
    final fixture = await mountTools(tester);
    final first = _entry(fixture.root.path, 'a.txt');
    await _selectClick(tester, first);
    await _keyChord(tester, LogicalKeyboardKey.keyI, command: true);
    await tester.pump();
    expect(tool('desktop-info-dialog'), findsOneWidget);
    await inspectReady(tester);
    expect(find.text('a.txt'), findsWidgets);
    expect(fixture.service.opened, isEmpty);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(tool('desktop-info-dialog'), findsNothing);
    expect(_isSelected(tester, first), isTrue);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  testWidgets('Space and Cmd Y preview text without opening or renaming', (
    tester,
  ) async {
    final fixture = await mountTools(tester);
    final first = _entry(fixture.root.path, 'a.txt');
    await _selectClick(tester, first);
    for (final command in [false, true]) {
      await _keyChord(
        tester,
        command ? LogicalKeyboardKey.keyY : LogicalKeyboardKey.space,
        command: command,
      );
      await tester.pump();
      expect(tool('desktop-quick-look-dialog'), findsOneWidget);
      await inspectReady(tester);
      expect(tool('desktop-preview-text'), findsOneWidget);
      expect(find.text('isolated'), findsOneWidget);
      expect(fixture.service.opened, isEmpty);
      expect(find.byKey(const ValueKey('desktop-inline-name')), findsNothing);
      await tester.sendKeyEvent(LogicalKeyboardKey.space);
      await tester.pumpAndSettle();
      expect(tool('desktop-quick-look-dialog'), findsNothing);
      expect(_isSelected(tester, first), isTrue);
    }
    await tester.pumpWidget(const SizedBox.shrink());
  });

  testWidgets('preview Open preserves untrusted launcher confirmation', (
    tester,
  ) async {
    final interactions = <bool>[];
    final fixture = await mountTools(
      tester,
      interactions: interactions,
      extraFiles: {
        'untrusted.desktop':
            '[Desktop Entry]\nType=Application\nName=Untrusted\nExec=fixture-never-run\n',
      },
    );
    final launcher = _entry(fixture.root.path, 'untrusted.desktop');
    await _selectClick(tester, launcher);
    for (final confirm in [false, true]) {
      final completed = List<bool>.of(interactions);
      await tester.sendKeyEvent(LogicalKeyboardKey.space);
      await tester.pump();
      await inspectReady(tester);
      expect(interactions, [...completed, true]);
      expect(fixture.service.opened, isEmpty);
      await tester.tap(tool('desktop-inspector-open'));
      await tester.pumpAndSettle();
      expect(tool('desktop-quick-look-dialog'), findsNothing);
      expect(tool('desktop-trust-confirm'), findsOneWidget);
      // The keyboard path has no enclosing menu. Do not release/reacquire the
      // native overlay between the read-only preview and trust confirmation.
      expect(interactions, [...completed, true]);
      expect(fixture.service.opened, isEmpty);
      await tester.tap(
        confirm ? tool('desktop-trust-confirm') : find.text('取消'),
      );
      await tester.pumpAndSettle();
      expect(interactions, [...completed, true, false]);
    }
    expect(fixture.service.opened, [
      ('${fixture.root.path}/untrusted.desktop', true),
    ]);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  testWidgets(
    'desktop context menu keeps one interaction through Info and Quick Look',
    (tester) async {
      final fixture = _Fixture();
      addTearDown(fixture.dispose);
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final interactions = <bool>[];
      await _mount(
        tester,
        fixture,
        style: ShellStyle.macos,
        interactions: interactions,
      );
      for (final preview in [false, true]) {
        await _context(tester, _entry(fixture.root.path, 'a.txt'));
        await tester.tap(
          tool(preview ? 'desktop-menu-quickLook' : 'desktop-menu-info'),
        );
        await tester.pump();
        expect(
          tool(preview ? 'desktop-quick-look-dialog' : 'desktop-info-dialog'),
          findsOneWidget,
        );
        await inspectReady(tester);
        expect(interactions.last, isTrue);
        await tester.tap(tool('desktop-inspector-close'));
        await tester.pumpAndSettle();
        expect(interactions.last, isFalse);
      }
      expect(interactions, [true, false, true, false]);
      expect(fixture.service.opened, isEmpty);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets('extra modifiers and inline editing never invoke inspectors', (
    tester,
  ) async {
    final fixture = await mountTools(tester);
    await _selectClick(tester, _entry(fixture.root.path, 'a.txt'));
    await _keyChord(
      tester,
      LogicalKeyboardKey.keyI,
      command: true,
      shift: true,
    );
    await _keyChord(
      tester,
      LogicalKeyboardKey.keyY,
      command: true,
      option: true,
    );
    await _keyChord(tester, LogicalKeyboardKey.space, control: true);
    await _keyChord(tester, LogicalKeyboardKey.space, option: true);
    await tester.pumpAndSettle();
    expect(tool('desktop-quick-look-dialog'), findsNothing);
    expect(tool('desktop-info-dialog'), findsNothing);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pumpAndSettle();
    expect(tool('desktop-inline-name'), findsOneWidget);
    await tester.sendKeyEvent(LogicalKeyboardKey.space);
    await _keyChord(tester, LogicalKeyboardKey.keyI, command: true);
    await tester.pumpAndSettle();
    expect(tool('desktop-quick-look-dialog'), findsNothing);
    expect(tool('desktop-info-dialog'), findsNothing);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(fixture.service.opened, isEmpty);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  testWidgets(
    'sort submenu stays open, checks mode and freezes positions when None is chosen',
    (tester) async {
      final fixture = await mountTools(
        tester,
        extraFiles: {'z.bin': 'z' * 200},
      );
      final root = fixture.root.path;
      await _context(tester);
      await tester.tap(tool('desktop-menu-sortBy'));
      await tester.pumpAndSettle();
      expect(tool('desktop-menu-sortBy'), findsOneWidget);
      expect(tool('desktop-sort-size'), findsOneWidget);
      await tester.tap(tool('desktop-sort-size'));
      await _wait(
        tester,
        () => fixture.layout.sortKey('DP-1') == DesktopSortKey.size,
      );
      expect(fixture.layout.sortKey('DP-1'), DesktopSortKey.size);
      expect(
        tester.getTopLeft(_entry(root, 'z.bin')).dy,
        lessThan(tester.getTopLeft(_entry(root, 'a.txt')).dy),
      );
      final before = tester.getRect(_entry(root, 'z.bin'));
      await tester.drag(_entry(root, 'z.bin'), const Offset(128, 132));
      await tester.pumpAndSettle();
      expect(fixture.layout.sortKey('DP-1'), DesktopSortKey.size);
      expect(tester.getRect(_entry(root, 'z.bin')), before);
      await _context(tester);
      await tester.tap(tool('desktop-menu-sortBy'));
      await tester.pumpAndSettle();
      expect(
        find.descendant(
          of: tool('desktop-sort-size'),
          matching: find.byIcon(Icons.check),
        ),
        findsOneWidget,
      );
      await tester.tap(tool('desktop-sort-none'));
      await _wait(tester, () => fixture.layout.sortKey('DP-1') == null);
      expect(fixture.layout.sortKey('DP-1'), isNull);
      expect(tester.getRect(_entry(root, 'z.bin')), before);
      await tester.drag(_entry(root, 'z.bin'), const Offset(128, 132));
      await _wait(
        tester,
        () =>
            fixture.layout.positions('DP-1')['$root/z.bin'] !=
            const DesktopCell(0, 0),
      );
      expect(tester.getRect(_entry(root, 'z.bin')), isNot(before));
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets(
    'one-shot cleanup compresses gaps without enabling automatic sorting',
    (tester) async {
      final fixture = (await tester.runAsync(() async => _Fixture()))!;
      addTearDown(fixture.dispose);
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final root = fixture.root.path;
      await tester.runAsync(
        () => fixture.layout.savePositions('DP-1', {
          '$root/a.txt': const DesktopCell(1, 2),
          '$root/b.txt': const DesktopCell(0, 2),
        }),
      );
      await _mount(tester, fixture, style: ShellStyle.macos);
      await _context(tester);
      await tester.tap(tool('desktop-menu-cleanUpBy'));
      await tester.pumpAndSettle();
      await tester.tap(tool('desktop-clean-name'));
      await _wait(
        tester,
        () =>
            fixture.layout.positions('DP-1')['$root/a.txt'] ==
            const DesktopCell(0, 0),
      );
      expect(fixture.layout.sortKey('DP-1'), isNull);
      expect(
        fixture.layout.positions('DP-1')['$root/a.txt'],
        const DesktopCell(0, 0),
      );
      expect(
        fixture.layout.positions('DP-1')['$root/b.txt'],
        const DesktopCell(0, 1),
      );
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets(
    'Escape and outside click dismiss the full sorting menu hierarchy',
    (tester) async {
      final fixture = await mountTools(tester);
      for (final escape in [true, false]) {
        await _context(tester);
        await tester.tap(tool('desktop-menu-sortBy'));
        await tester.pumpAndSettle();
        expect(tool('desktop-sort-name'), findsOneWidget);
        if (escape) {
          await tester.sendKeyEvent(LogicalKeyboardKey.escape);
        } else {
          await tester.tapAt(const Offset(20, 690));
        }
        await tester.pumpAndSettle();
        expect(tool('desktop-sort-name'), findsNothing);
        expect(tool('desktop-menu-sortBy'), findsNothing);
      }
      expect(fixture.layout.sortKey('DP-1'), isNull);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );
}
