// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:io';

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/desktop/desktop_entry.dart';
import 'package:clarion_shell/desktop/desktop_layout.dart';
import 'package:clarion_shell/desktop/desktop_service.dart';
import 'package:clarion_shell/desktop/desktop_view.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/theme/mac_shell_theme.dart';

class _DelayedRename extends DesktopService {
  _DelayedRename(this.root) : super(root: root) {
    files = [
      for (final name in ['a.txt', 'b.txt']) entry(name),
    ];
  }
  final String root;
  late List<DesktopEntry> files;
  final publish = Completer<void>();
  final complete = Completer<void>();
  var working = false;
  int calls = 0;
  DesktopEntry entry(String name) =>
      DesktopEntry(fullPath: '$root/$name', name: name);
  @override
  List<DesktopEntry> get entries => files;
  @override
  bool get busy => working;
  @override
  String? get error => null;
  @override
  Future<void> renameEntry(DesktopEntry old, String name) async {
    calls++;
    working = true;
    notifyListeners();
    await publish.future;
    files = [
      for (final file in files)
        if (file.fullPath == old.fullPath) entry(name) else file,
    ];
    notifyListeners();
    await complete.future;
    working = false;
    notifyListeners();
  }
}

class _Layout extends DesktopLayoutStore {
  _Layout() : super(persist: false);
  final data = <String, Map<String, DesktopCell>>{};
  final writes = <String>[];
  @override
  Future<void> load() async {}
  @override
  Map<String, DesktopCell> positions(String outputName) =>
      Map.unmodifiable(data[outputName] ?? {});
  @override
  Future<void> savePositions(
    String outputName,
    Map<String, DesktopCell> positions,
  ) async {
    writes.add(outputName);
    data[outputName] = Map.of(positions);
    notifyListeners();
  }
}

class _Fixture {
  _Fixture() {
    service = _DelayedRename(root.path);
    layout.data['A'] = {
      '${root.path}/a.txt': const DesktopCell(0, 0),
      '${root.path}/b.txt': const DesktopCell(0, 1),
    };
    addTearDown(() {
      service.dispose();
      layout.dispose();
      root.deleteSync();
    });
  }
  final root = Directory.systemTemp.createTempSync('clarion-inline-review-');
  late final _DelayedRename service;
  final layout = _Layout();
  Finder item(String name) =>
      find.byKey(ValueKey('desktop-entry-${root.path}/$name'));
  final interactions = <bool>[];
  Future<void> mount(
    WidgetTester tester, {
    String output = 'A',
    ShellStyle style = ShellStyle.macos,
    DesktopService? withService,
  }) async {
    await tester.pumpWidget(
      MaterialApp(
        theme: buildMacShellTheme(Brightness.light),
        home: DesktopView(
          service: withService ?? service,
          layout: layout,
          outputName: output,
          barPosition: BarPosition.top,
          shellStyle: style,
          reduceMotion: true,
          onInteractionChanged: interactions.add,
        ),
      ),
    );
    await tester.pumpAndSettle();
  }
}

final _editor = find.byKey(const ValueKey('desktop-inline-name'));
bool _selected(WidgetTester tester, Finder item) => tester
    .widget<Semantics>(
      find
          .ancestor(
            of: item,
            matching: find.byWidgetPredicate(
              (widget) =>
                  widget is Semantics && widget.properties.selected != null,
            ),
          )
          .first,
    )
    .properties
    .selected!;

Future<void> _begin(WidgetTester tester, _Fixture fixture) async {
  await tester.tap(fixture.item('a.txt'));
  await tester.pump(const Duration(milliseconds: 350));
  await tester.sendKeyEvent(LogicalKeyboardKey.enter);
  await tester.pumpAndSettle();
  expect(tester.widget<TextField>(_editor).focusNode!.hasFocus, isTrue);
  await tester.enterText(_editor, 'renamed.txt');
}

Future<void> _publish(WidgetTester tester, _Fixture fixture) async {
  fixture.service.publish.complete();
  await tester.pump();
  // Exercise a real frame between the service's scan notification and its
  // operation Future completing, as another awaited rescan can do in practice.
  await tester.pump(const Duration(milliseconds: 16));
}

Future<void> _finish(WidgetTester tester, _Fixture fixture) async {
  fixture.service.complete.complete();
  await tester.pumpAndSettle();
}

void main() {
  testWidgets(
    'rename retains selection across a scan frame before operation completion',
    (tester) async {
      final fixture = _Fixture();
      await fixture.mount(tester);
      await _begin(tester, fixture);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await _publish(tester, fixture);
      await _finish(tester, fixture);
      expect(_selected(tester, fixture.item('renamed.txt')), isTrue);
      expect(_editor, findsNothing);
      expect(
        fixture.layout.positions('A')['${fixture.root.path}/renamed.txt'],
        const DesktopCell(0, 0),
      );
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets(
    'rename preserves another icon layout update received during the pending operation',
    (tester) async {
      final fixture = _Fixture();
      await fixture.mount(tester);
      await _begin(tester, fixture);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      const moved = DesktopCell(1, 2);
      await fixture.layout.savePositions('A', {
        ...fixture.layout.positions('A'),
        '${fixture.root.path}/b.txt': moved,
      });
      await tester.pump();
      await _publish(tester, fixture);
      await _finish(tester, fixture);
      expect(
        fixture.layout.positions('A')['${fixture.root.path}/b.txt'],
        moved,
      );
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets(
    'clicking another icon commits once and retains the newly clicked selection',
    (tester) async {
      final fixture = _Fixture();
      await fixture.mount(tester);
      await _begin(tester, fixture);
      await tester.tap(fixture.item('b.txt'));
      await tester.pump(const Duration(milliseconds: 350));
      expect(fixture.service.calls, 1);
      expect(_selected(tester, fixture.item('b.txt')), isTrue);
      await _publish(tester, fixture);
      await _finish(tester, fixture);
      expect(_selected(tester, fixture.item('b.txt')), isTrue);
      expect(_selected(tester, fixture.item('renamed.txt')), isFalse);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  for (final switchKind in ['output', 'style', 'service']) {
    testWidgets('$switchKind change ignores the old rename completion', (
      tester,
    ) async {
      final fixture = _Fixture();
      await fixture.mount(tester);
      await _begin(tester, fixture);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      final other = _DelayedRename('${fixture.root.path}/other');
      addTearDown(other.dispose);
      await fixture.mount(
        tester,
        output: switchKind == 'output' ? 'B' : 'A',
        style: switchKind == 'style' ? ShellStyle.material : ShellStyle.macos,
        withService: switchKind == 'service' ? other : null,
      );
      expect(_editor, findsNothing);
      await _publish(tester, fixture);
      await _finish(tester, fixture);
      expect(fixture.layout.writes, isEmpty);
      expect(_editor, findsNothing);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    });
  }

  testWidgets(
    'context-menu rename gives the inline field focus after the popup is removed',
    (tester) async {
      final fixture = _Fixture();
      await fixture.mount(tester);
      await tester.tap(fixture.item('a.txt'), buttons: kSecondaryMouseButton);
      await tester.pumpAndSettle();
      await tester.tap(find.text('重命名'));
      await tester.pumpAndSettle();
      expect(fixture.interactions, [true, false]);
      expect(
        tester.widget<TextField>(_editor).focusNode!.hasPrimaryFocus,
        isTrue,
      );
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      expect(_editor, findsNothing);
      expect(fixture.service.calls, 0);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );
}
