// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:realand_shell/mac_shell_preferences.dart';
import 'package:realand_shell/services/dock_folder_service.dart';
import 'package:realand_shell/ui/mac_downloads_stack_panel.dart';

class _Downloads extends DockFolderService {
  _Downloads({int count = 3}) : super(directory: '/isolated-test-downloads') {
    content = List.generate(
      count,
      (i) => DockFolderEntry(
        path: '/isolated-test-downloads/$i.txt',
        name: '文件 $i.txt',
        modified: DateTime(2024, 1, 1 + i),
      ),
    );
  }
  late List<DockFolderEntry> content;
  final opened = <String>[];
  bool failOpen = false;
  bool exceedsLimit = false;
  Completer<void>? pendingOpen;
  @override
  List<DockFolderEntry> get entries => content;
  @override
  bool get overflow => exceedsLimit;
  @override
  List<DockFolderEntry> sortedEntries(MacDownloadsSort requested) => content;
  @override
  Future<void> open(DockFolderEntry entry) async {
    opened.add(entry.id);
    if (pendingOpen case final pending?) await pending.future;
    if (failOpen) throw StateError('application rejected the file');
  }

  @override
  Future<void> openFolder() async {
    opened.add('__folder__');
    if (failOpen) throw StateError('file manager unavailable');
  }
}

Future<void> _mount(
  WidgetTester tester,
  _Downloads service, {
  MacShellPreferences preferences = const MacShellPreferences(),
  Size size = const Size(540, 500),
  VoidCallback? onClose,
  ValueChanged<MacShellPreferences>? onChanged,
  double textScale = 1,
  bool reducedMotion = false,
  double? fanAnchorX,
  double? fanAnchorYBelowPanel,
  double? fanOriginIconSize,
}) => tester.pumpWidget(
  MaterialApp(
    home: Scaffold(
      body: MediaQuery(
        data: MediaQueryData(
          size: size,
          textScaler: TextScaler.linear(textScale),
          disableAnimations: reducedMotion,
        ),
        child: Align(
          alignment: Alignment.topLeft,
          child: SizedBox(
            width: size.width,
            height: size.height,
            child: MacDownloadsStackPanel(
              service: service,
              preferences: preferences,
              onPreferencesChanged: onChanged ?? (_) {},
              onClose: onClose ?? () {},
              fanAnchorX: fanAnchorX,
              fanAnchorYBelowPanel: fanAnchorYBelowPanel,
              fanOriginIconSize: fanOriginIconSize,
            ),
          ),
        ),
      ),
    ),
  ),
);

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  setUp(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
          const MethodChannel('realand.shell/window'),
          (call) async => null,
        );
  });
  tearDown(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
          const MethodChannel('realand.shell/window'),
          null,
        );
  });

  test('fan is confined to bottom Docks with enough room and few files', () {
    for (final position in MacDockPosition.values) {
      expect(
        resolveDownloadsView(
          requested: MacDownloadsView.automatic,
          dockPosition: position,
          available: const Size(540, 600),
          entryCount: 5,
        ),
        position == MacDockPosition.bottom
            ? MacDownloadsView.fan
            : MacDownloadsView.grid,
      );
    }
    for (final available in [const Size(250, 600), const Size(540, 150)]) {
      expect(
        resolveDownloadsView(
          requested: MacDownloadsView.fan,
          dockPosition: MacDockPosition.bottom,
          available: available,
          entryCount: 5,
        ),
        MacDownloadsView.grid,
      );
    }
    expect(
      resolveDownloadsView(
        requested: MacDownloadsView.automatic,
        dockPosition: MacDockPosition.bottom,
        available: const Size(540, 1000),
        entryCount: 30,
      ),
      MacDownloadsView.grid,
    );
  });

  test('explicit Fan remains effective for large folders unlike Automatic', () {
    for (final count in [9, 24, 128, 256]) {
      expect(
        resolveDownloadsView(
          requested: MacDownloadsView.fan,
          dockPosition: MacDockPosition.bottom,
          available: const Size(540, 500),
          entryCount: count,
        ),
        MacDownloadsView.fan,
      );
      expect(
        resolveDownloadsView(
          requested: MacDownloadsView.automatic,
          dockPosition: MacDockPosition.bottom,
          available: const Size(540, 500),
          entryCount: count,
        ),
        MacDownloadsView.grid,
      );
    }
    for (final available in [const Size(290, 500), const Size(540, 150)]) {
      expect(
        resolveDownloadsView(
          requested: MacDownloadsView.fan,
          dockPosition: MacDockPosition.bottom,
          available: available,
          entryCount: 128,
        ),
        MacDownloadsView.grid,
      );
    }
  });

  testWidgets(
    'large explicit Fan has bounded files and an actionable terminus',
    (tester) async {
      final service = _Downloads(count: 128);
      addTearDown(service.dispose);
      var closed = 0;
      await _mount(
        tester,
        service,
        preferences: const MacShellPreferences(
          downloadsView: MacDownloadsView.fan,
        ),
        onClose: () => closed++,
      );
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('downloads-fan')), findsOneWidget);
      expect(find.byKey(const ValueKey('downloads-grid')), findsNothing);
      final files = find.byWidgetPredicate(
        (widget) =>
            widget is GestureDetector &&
            widget.key is ValueKey<String> &&
            (widget.key! as ValueKey<String>).value.startsWith(
              'downloads-entry-',
            ),
      );
      expect(files.evaluate().length, inInclusiveRange(1, 8));
      expect(
        find.byKey(ValueKey('downloads-entry-${service.entries[40].id}')),
        findsNothing,
      );
      expect(find.text('在文件管理器中显示更多'), findsOneWidget);
      await tester.sendKeyEvent(LogicalKeyboardKey.end);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pumpAndSettle();
      expect(service.opened, ['__folder__']);
      expect(closed, 1);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('Fan Up selects the next visible icon above the current one', (
    tester,
  ) async {
    final service = _Downloads(count: 24);
    addTearDown(service.dispose);
    await _mount(
      tester,
      service,
      preferences: const MacShellPreferences(
        downloadsView: MacDownloadsView.fan,
      ),
    );
    await tester.pumpAndSettle();
    final first = find.byKey(
      ValueKey('downloads-entry-${service.entries[0].id}'),
    );
    final second = find.byKey(
      ValueKey('downloads-entry-${service.entries[1].id}'),
    );
    expect(tester.getCenter(second).dy, lessThan(tester.getCenter(first).dy));
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowUp);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pumpAndSettle();
    expect(service.opened, [service.entries[1].id]);
  });

  testWidgets(
    'Fan folds back before dismissing and blocks repeated dismissal',
    (tester) async {
      final service = _Downloads();
      addTearDown(service.dispose);
      var closed = 0;
      await _mount(
        tester,
        service,
        preferences: const MacShellPreferences(
          downloadsView: MacDownloadsView.fan,
        ),
        onClose: () => closed++,
      );
      await tester.pumpAndSettle();
      final second = find.byKey(
        ValueKey('downloads-entry-${service.entries[1].id}'),
      );
      final before = tester.getCenter(second);
      await tester.tap(find.byKey(const ValueKey('downloads-stack-close')));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 50));
      expect(closed, 0);
      expect(tester.getCenter(second).dy, greaterThan(before.dy));
      expect(
        tester
            .widget<IconButton>(
              find.byKey(const ValueKey('downloads-stack-close')),
            )
            .onPressed,
        isNull,
      );
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      expect(closed, 1);
    },
  );

  testWidgets('Reduce Motion completes Fan entry and exit immediately', (
    tester,
  ) async {
    final service = _Downloads(count: 24);
    addTearDown(service.dispose);
    var closed = 0;
    await _mount(
      tester,
      service,
      preferences: const MacShellPreferences(
        downloadsView: MacDownloadsView.fan,
      ),
      reducedMotion: true,
      onClose: () => closed++,
    );
    await tester.pump();
    final first = find.byKey(
      ValueKey('downloads-entry-${service.entries[0].id}'),
    );
    final second = find.byKey(
      ValueKey('downloads-entry-${service.entries[1].id}'),
    );
    expect(
      tester.getCenter(first).dy - tester.getCenter(second).dy,
      closeTo(48, .01),
    );
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    expect(closed, 1);
    expect(tester.takeException(), isNull);
  });

  testWidgets('Fan mirrors edge labels and keeps visible targets in bounds', (
    tester,
  ) async {
    final service = _Downloads(count: 64);
    addTearDown(service.dispose);
    for (final anchor in [0.0, 20.0, 270.0, 520.0, 540.0]) {
      await _mount(
        tester,
        service,
        preferences: const MacShellPreferences(
          downloadsView: MacDownloadsView.fan,
        ),
        size: const Size(540, 320),
        textScale: 1.5,
        reducedMotion: true,
        fanAnchorX: anchor,
      );
      await tester.pumpAndSettle();
      final panel = tester.getRect(
        find.byKey(const ValueKey('mac-downloads-stack-panel')),
      );
      final positions = find.byWidgetPredicate(
        (widget) =>
            widget is Positioned &&
            widget.key is ValueKey<String> &&
            (widget.key! as ValueKey<String>).value.startsWith(
              'downloads-fan-position-',
            ),
      );
      for (final element in positions.evaluate()) {
        final rect = tester.getRect(find.byWidget(element.widget));
        expect(panel.inflate(.01).contains(rect.topLeft), isTrue);
        expect(panel.inflate(.01).contains(rect.bottomRight), isTrue);
      }
      final first = find.byKey(
        ValueKey('downloads-entry-${service.entries.first.id}'),
      );
      final row = tester.widget<Row>(
        find.descendant(of: first, matching: find.byType(Row)).first,
      );
      // The free-standing icon is on the side nearest the actual Dock anchor.
      expect(row.children.first is SizedBox, anchor < 270);
      expect(tester.takeException(), isNull);
    }
  });

  testWidgets('Fan starts at the real 2D Dock centre and presented icon size', (
    tester,
  ) async {
    final service = _Downloads();
    addTearDown(service.dispose);
    final first = find.byKey(
      ValueKey('downloads-entry-${service.entries.first.id}'),
    );
    Finder iconTransform() =>
        find.descendant(of: first, matching: find.byType(Transform)).first;
    void expectPresentedIconSize(double size) {
      final transform = tester.widget<Transform>(iconTransform());
      // A 2D scale leaves the z axis at 1, so getMaxScaleOnAxis would not
      // measure a shrinking icon. Check both projected axes and the child
      // bounds after the ancestor scale has been applied to the actual paint.
      expect(transform.transform.entry(0, 0), closeTo(size / 64, .001));
      expect(transform.transform.entry(1, 1), closeTo(size / 64, .001));
      final paintedBounds = tester.getRect(find.byWidget(transform.child!));
      expect(paintedBounds.width, closeTo(size, .01));
      expect(paintedBounds.height, closeTo(size, .01));
    }

    await _mount(
      tester,
      service,
      preferences: const MacShellPreferences(
        downloadsView: MacDownloadsView.fan,
      ),
      size: const Size(540, 320),
      fanAnchorX: 250,
      fanAnchorYBelowPanel: 60,
      fanOriginIconSize: 32,
    );
    final panel = tester.getRect(
      find.byKey(const ValueKey('mac-downloads-stack-panel')),
    );
    expect(
      tester.getCenter(iconTransform()).dx,
      closeTo(panel.left + 250, .01),
    );
    expect(
      tester.getCenter(iconTransform()).dy,
      closeTo(panel.bottom + 60, .01),
    );
    expectPresentedIconSize(32);
    expect(
      tester
          .widget<Opacity>(
            find
                .ancestor(
                  of: find.byKey(
                    ValueKey(
                      'downloads-fan-caption-${service.entries.first.id}',
                    ),
                  ),
                  matching: find.byType(Opacity),
                )
                .first,
          )
          .opacity,
      0,
    );
    expect(
      tester
          .widget<IgnorePointer>(
            find
                .ancestor(of: first, matching: find.byType(IgnorePointer))
                .first,
          )
          .ignoring,
      isTrue,
    );
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    expect(service.opened, isEmpty);
    await tester.pumpAndSettle();
    expect(
      tester.getCenter(iconTransform()).dy,
      closeTo(panel.bottom - 40, .01),
    );
    expectPresentedIconSize(64);
    expect(
      tester
          .widget<IgnorePointer>(
            find
                .ancestor(of: first, matching: find.byType(IgnorePointer))
                .first,
          )
          .ignoring,
      isFalse,
    );

    await _mount(
      tester,
      service,
      preferences: const MacShellPreferences(
        downloadsView: MacDownloadsView.fan,
      ),
      size: const Size(540, 320),
      fanAnchorX: 250,
      fanAnchorYBelowPanel: 60,
      fanOriginIconSize: 32,
      reducedMotion: true,
    );
    await tester.pump();
    expect(
      tester.getCenter(iconTransform()).dy,
      closeTo(panel.bottom - 40, .01),
    );
    expectPresentedIconSize(64);
    expect(tester.takeException(), isNull);
  });

  testWidgets('Downloads takes keyboard focus from a separate Dock app scope', (
    tester,
  ) async {
    final service = _Downloads(count: 15);
    final dockFocus = FocusNode(debugLabel: 'test-dock-item');
    final previousMainFocus = FocusNode(debugLabel: 'test-previous-main-item');
    addTearDown(service.dispose);
    addTearDown(dockFocus.dispose);
    addTearDown(previousMainFocus.dispose);
    var dockKeys = 0;
    var closed = 0;
    Widget views(bool showDownloads) => Row(
      textDirection: TextDirection.ltr,
      children: [
        Expanded(
          child: MaterialApp(
            key: const ValueKey('dock-application'),
            home: Focus(
              focusNode: dockFocus,
              onKeyEvent: (_, event) {
                if (event is KeyDownEvent) dockKeys++;
                return KeyEventResult.handled;
              },
              child: const SizedBox.expand(),
            ),
          ),
        ),
        Expanded(
          child: MaterialApp(
            key: const ValueKey('main-application'),
            home: Scaffold(
              body: Stack(
                children: [
                  Focus(
                    focusNode: previousMainFocus,
                    child: const SizedBox.expand(),
                  ),
                  if (showDownloads)
                    MediaQuery(
                      data: const MediaQueryData(disableAnimations: true),
                      child: MacDownloadsStackPanel(
                        key: const ValueKey('downloads-revision-1'),
                        service: service,
                        preferences: const MacShellPreferences(
                          downloadsView: MacDownloadsView.fan,
                        ),
                        onPreferencesChanged: (_) {},
                        onClose: () => closed++,
                      ),
                    ),
                ],
              ),
            ),
          ),
        ),
      ],
    );
    await tester.pumpWidget(views(false));
    await tester.pumpAndSettle();
    previousMainFocus.requestFocus();
    await tester.pump();
    expect(previousMainFocus.hasPrimaryFocus, isTrue);
    dockFocus.requestFocus();
    await tester.pump();
    expect(dockFocus.hasPrimaryFocus, isTrue);
    // The main scope still remembers its old focused child. An autofocus-only
    // panel cannot replace that history after the separate Dock takes focus.
    await tester.pumpWidget(views(true));
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('downloads-fan')), findsOneWidget);
    await tester.sendKeyEvent(LogicalKeyboardKey.end);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pumpAndSettle();
    expect(service.opened, ['__folder__']);
    expect(closed, 1);
    expect(dockKeys, 0);
    expect(tester.takeException(), isNull);
  });

  testWidgets('empty folder has an actionable directory shortcut', (
    tester,
  ) async {
    final service = _Downloads(count: 0);
    addTearDown(service.dispose);
    var closed = 0;
    await _mount(
      tester,
      service,
      preferences: const MacShellPreferences(
        downloadsView: MacDownloadsView.list,
      ),
      onClose: () => closed++,
    );
    await tester.pumpAndSettle();
    expect(find.text('下载目录是空的'), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('downloads-open-folder')));
    await tester.pumpAndSettle();
    expect(service.opened, ['__folder__']);
    expect(closed, 1);
  });

  testWidgets(
    'files open through service and failed opens keep panel visible',
    (tester) async {
      final service = _Downloads()..failOpen = true;
      addTearDown(service.dispose);
      var closed = 0;
      await _mount(tester, service, onClose: () => closed++);
      await tester.pumpAndSettle();
      await tester.tap(
        find.byKey(ValueKey('downloads-entry-${service.entries.first.id}')),
      );
      await tester.pumpAndSettle();
      expect(closed, 0);
      expect(find.textContaining('无法打开：'), findsOneWidget);
      service.failOpen = false;
      await tester.tap(
        find.byKey(ValueKey('downloads-entry-${service.entries.first.id}')),
      );
      await tester.pumpAndSettle();
      expect(closed, 1);
    },
  );

  testWidgets('keyboard selection Enter and Escape have real outcomes', (
    tester,
  ) async {
    final service = _Downloads();
    addTearDown(service.dispose);
    var closed = 0;
    await _mount(
      tester,
      service,
      preferences: const MacShellPreferences(
        downloadsView: MacDownloadsView.list,
      ),
      onClose: () => closed++,
    );
    await tester.pumpAndSettle();
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pumpAndSettle();
    expect(service.opened, [service.entries[1].id]);
    expect(closed, 1);
    await tester.pumpWidget(const MaterialApp(home: SizedBox()));
    await _mount(tester, service, onClose: () => closed++);
    await tester.pumpAndSettle();
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(closed, 2);
  });

  testWidgets('late launch completion cannot dismiss a replacement panel', (
    tester,
  ) async {
    final service = _Downloads()..pendingOpen = Completer<void>();
    addTearDown(service.dispose);
    var closed = 0;
    await _mount(tester, service, onClose: () => closed++);
    await tester.pumpAndSettle();
    await tester.tap(
      find.byKey(ValueKey('downloads-entry-${service.entries.first.id}')),
    );
    await tester.pump();
    await tester.pumpWidget(const MaterialApp(home: SizedBox()));
    service.pendingOpen!.complete();
    await tester.pumpAndSettle();
    expect(closed, 0);
    expect(tester.takeException(), isNull);
  });

  testWidgets('pending launch retains the callback of its original panel', (
    tester,
  ) async {
    final service = _Downloads()..pendingOpen = Completer<void>();
    addTearDown(service.dispose);
    var oldClosed = 0;
    var replacementClosed = 0;
    await _mount(tester, service, onClose: () => oldClosed++);
    await tester.pumpAndSettle();
    final original = tester.state(find.byType(MacDownloadsStackPanel));
    await tester.tap(
      find.byKey(ValueKey('downloads-entry-${service.entries.first.id}')),
    );
    await tester.pump();
    await _mount(tester, service, onClose: () => replacementClosed++);
    expect(tester.state(find.byType(MacDownloadsStackPanel)), same(original));
    service.pendingOpen!.complete();
    await tester.pumpAndSettle();
    expect(oldClosed, 1);
    expect(replacementClosed, 0);
    expect(tester.takeException(), isNull);
  });

  testWidgets('small side Dock uses scrollable grid without text overflow', (
    tester,
  ) async {
    final service = _Downloads(count: 32)..exceedsLimit = true;
    service.content[0] = const DockFolderEntry(
      path: '/isolated-test-downloads/long.txt',
      name: '这个很长的文件名仍应保留完整悬停提示与实际身份.txt',
    );
    addTearDown(service.dispose);
    await _mount(
      tester,
      service,
      preferences: const MacShellPreferences(
        dockPosition: MacDockPosition.right,
      ),
      size: const Size(260, 260),
      textScale: 1.5,
    );
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('downloads-grid')), findsOneWidget);
    expect(find.byKey(const ValueKey('downloads-fan')), findsNothing);
    expect(find.textContaining('查看全部'), findsOneWidget);
    await tester.drag(
      find.byKey(const ValueKey('downloads-grid')),
      const Offset(0, -350),
    );
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'unavailable true dates are disabled and display/view stay distinct',
    (tester) async {
      final service = _Downloads();
      addTearDown(service.dispose);
      MacShellPreferences? selected;
      await _mount(
        tester,
        service,
        onChanged: (preferences) => selected = preferences,
      );
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const ValueKey('downloads-stack-options')));
      await tester.pumpAndSettle();
      expect(
        tester
            .widget<CheckedPopupMenuItem<Object>>(
              find.byKey(const ValueKey('downloads-sort-dateCreated')),
            )
            .enabled,
        isFalse,
      );
      expect(
        tester
            .widget<CheckedPopupMenuItem<Object>>(
              find.byKey(const ValueKey('downloads-sort-dateAdded')),
            )
            .enabled,
        isFalse,
      );
      final folderOption = find.byWidgetPredicate(
        (widget) =>
            widget is CheckedPopupMenuItem<Object> &&
            widget.value == MacDownloadsDisplay.folder,
      );
      await tester.ensureVisible(folderOption);
      await tester.pumpAndSettle();
      await tester.tap(folderOption);
      await tester.pumpAndSettle();
      expect(selected!.downloadsDisplay, MacDownloadsDisplay.folder);
      expect(selected!.downloadsView, MacDownloadsView.automatic);
    },
  );

  testWidgets(
    'closed tile shows real stacked contents or original Downloads folder',
    (tester) async {
      final service = _Downloads();
      addTearDown(service.dispose);
      Future<void> icon(MacDownloadsDisplay display) async {
        await tester.pumpWidget(
          MaterialApp(
            home: Center(
              child: MacDownloadsStackIcon(
                service: service,
                preferences: MacShellPreferences(downloadsDisplay: display),
              ),
            ),
          ),
        );
        await tester.pumpAndSettle();
      }

      await icon(MacDownloadsDisplay.stack);
      expect(
        find.byKey(const ValueKey('downloads-stack-icon')),
        findsOneWidget,
      );
      await icon(MacDownloadsDisplay.folder);
      expect(
        find.byKey(const ValueKey('downloads-folder-icon')),
        findsOneWidget,
      );
      expect(find.byKey(const ValueKey('downloads-stack-icon')), findsNothing);
      expect(tester.takeException(), isNull);
    },
  );
}
