// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/desktop/desktop_entry.dart';
import 'package:clarion_shell/desktop/desktop_file_details.dart';
import 'package:clarion_shell/desktop/desktop_inspector.dart';
import 'package:clarion_shell/desktop/desktop_service.dart';

const _first = DesktopEntry(fullPath: '/fixture/one.txt', name: 'one.txt');
const _second = DesktopEntry(fullPath: '/fixture/two.txt', name: 'two.txt');

DesktopFileDetails _details(DesktopEntry entry) => DesktopFileDetails(
  entry: entry,
  name: entry.name,
  path: entry.fullPath,
  kind: DesktopFileKind.file,
  sizeBytes: 12,
  modified: DateTime.utc(2024, 1, 2, 3, 4, 5),
  mode: 0x81a4,
);

DesktopFilePreview _preview(DesktopEntry entry, String text) =>
    DesktopFilePreview(
      details: _details(entry),
      kind: DesktopPreviewKind.text,
      text: text,
    );

class _Service extends DesktopService {
  _Service() : super(root: '/fixture');
  final cancellations = <DesktopInspectionCancellation>[];
  final opened = <DesktopEntry>[];
  Future<DesktopFilePreview> Function(DesktopEntry)? preview;
  Future<DesktopFileDetails> Function(DesktopEntry)? inspect;
  bool operationBusy = false;

  @override
  bool get busy => operationBusy;

  void setBusy(bool value) {
    operationBusy = value;
    notifyListeners();
  }

  @override
  Future<DesktopFileDetails> inspectEntry(
    DesktopEntry entry, {
    DesktopInspectionCancellation? cancellation,
  }) async {
    if (cancellation != null) cancellations.add(cancellation);
    return inspect == null ? _details(entry) : inspect!(entry);
  }

  @override
  Future<DesktopFilePreview> previewEntry(
    DesktopEntry entry, {
    DesktopInspectionCancellation? cancellation,
  }) async {
    if (cancellation != null) cancellations.add(cancellation);
    return preview == null
        ? _preview(entry, 'Contents of ${entry.name}')
        : preview!(entry);
  }

  @override
  Future<void> openEntry(
    DesktopEntry entry, {
    bool allowUntrustedShortcut = false,
  }) async {
    opened.add(entry);
  }
}

Future<void> _host(
  WidgetTester tester,
  _Service service,
  List<DesktopEntry?> results, {
  bool quickLook = false,
  List<DesktopEntry> entries = const [_first],
  DesktopEntry? current,
  bool dark = false,
  double textScale = 1,
}) async {
  await tester.pumpWidget(
    MaterialApp(
      theme: ThemeData(brightness: dark ? Brightness.dark : Brightness.light),
      builder: (context, child) => MediaQuery(
        data: MediaQuery.of(
          context,
        ).copyWith(textScaler: TextScaler.linear(textScale)),
        child: child!,
      ),
      home: Scaffold(
        body: Builder(
          builder: (context) => TextButton(
            key: const ValueKey('launch-inspector'),
            onPressed: () async {
              results.add(
                await (quickLook
                    ? showDesktopQuickLookDialog
                    : showDesktopInfoDialog)(
                  context: context,
                  service: service,
                  entries: entries,
                  current: current,
                ),
              );
            },
            child: const Text('Inspect'),
          ),
        ),
      ),
    ),
  );
  await tester.tap(find.byKey(const ValueKey('launch-inspector')));
  await tester.pump();
}

void main() {
  testWidgets(
    'Info returns an untrusted launcher only after explicit Open without launching',
    (tester) async {
      final service = _Service();
      addTearDown(service.dispose);
      const launcher = DesktopEntry(
        fullPath: '/fixture/app.desktop',
        name: 'app.desktop',
        displayName: 'Example',
        isDesktopShortcut: true,
      );
      final results = <DesktopEntry?>[];
      await _host(
        tester,
        service,
        results,
        entries: [_first, launcher],
        current: launcher,
      );
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('desktop-info-dialog')), findsOneWidget);
      expect(find.text('2 / 2'), findsOneWidget);
      expect(find.text('rw-r--r-- (0644)'), findsOneWidget);
      expect(find.text('创建时间'), findsNothing);
      await tester.ensureVisible(
        find.byKey(const ValueKey('desktop-inspector-open')),
      );
      await tester.tap(find.byKey(const ValueKey('desktop-inspector-open')));
      await tester.pumpAndSettle();
      expect(results, [same(launcher)]);
      expect(service.opened, isEmpty);
    },
  );

  testWidgets(
    'Quick Look cancels previous reads and ignores late data while navigating by arrows',
    (tester) async {
      final service = _Service();
      addTearDown(service.dispose);
      final old = Completer<DesktopFilePreview>();
      service.preview = (entry) => entry == _first
          ? old.future
          : Future.value(_preview(entry, 'CURRENT CONTENT'));
      final results = <DesktopEntry?>[];
      await _host(
        tester,
        service,
        results,
        quickLook: true,
        entries: [_first, _second],
      );
      await tester.pump(const Duration(milliseconds: 250));
      expect(
        find.byKey(const ValueKey('desktop-inspector-loading')),
        findsOneWidget,
      );
      await tester.sendKeyEvent(LogicalKeyboardKey.arrowRight);
      await tester.pumpAndSettle();
      expect(service.cancellations.first.isCancelled, isTrue);
      expect(find.text('CURRENT CONTENT'), findsOneWidget);
      old.complete(_preview(_first, 'STALE CONTENT'));
      await tester.pumpAndSettle();
      expect(find.text('STALE CONTENT'), findsNothing);
      expect(
        find.byKey(const ValueKey('desktop-preview-text')),
        findsOneWidget,
      );
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      expect(results, [null]);
      expect(service.cancellations.last.isCancelled, isTrue);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'closing a pending inspector cancels and safely absorbs a late failure',
    (tester) async {
      final service = _Service();
      addTearDown(service.dispose);
      final delayed = Completer<DesktopFileDetails>();
      service.inspect = (_) => delayed.future;
      final results = <DesktopEntry?>[];
      await _host(tester, service, results);
      await tester.pump(const Duration(milliseconds: 250));
      expect(
        tester
            .widget<FilledButton>(
              find.byKey(const ValueKey('desktop-inspector-open')),
            )
            .onPressed,
        isNull,
      );
      await tester.tap(find.byKey(const ValueKey('desktop-inspector-close')));
      await tester.pumpAndSettle();
      expect(service.cancellations.single.isCancelled, isTrue);
      delayed.completeError(StateError('late failure'));
      await tester.pumpAndSettle();
      expect(results, [null]);
      expect(
        find.byKey(const ValueKey('desktop-inspector-error')),
        findsNothing,
      );
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'unsupported content is truthful and busy Open updates without launching',
    (tester) async {
      final service = _Service();
      addTearDown(service.dispose);
      service.preview = (entry) async => DesktopFilePreview(
        details: _details(entry),
        kind: DesktopPreviewKind.unsupported,
        message: '此文件格式暂不支持快速查看；可选择打开。',
      );
      service.operationBusy = true;
      final results = <DesktopEntry?>[];
      await _host(tester, service, results, quickLook: true);
      await tester.pumpAndSettle();
      final open = find.byKey(const ValueKey('desktop-inspector-open'));
      expect(tester.widget<FilledButton>(open).onPressed, isNull);
      expect(find.textContaining('不支持快速查看'), findsOneWidget);
      service.setBusy(false);
      await tester.pump();
      expect(tester.widget<FilledButton>(open).onPressed, isNotNull);
      await tester.sendKeyEvent(LogicalKeyboardKey.space);
      await tester.pumpAndSettle();
      expect(results, [null]);
      expect(service.opened, isEmpty);
    },
  );

  testWidgets('narrow dark Info remains scrollable at large text scale', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(320, 260);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final service = _Service();
    addTearDown(service.dispose);
    final entry = DesktopEntry(
      fullPath: '/fixture/${'very-long-path-' * 15}.txt',
      name: '很长的文件名' * 20,
    );
    final results = <DesktopEntry?>[];
    await _host(
      tester,
      service,
      results,
      entries: [entry],
      dark: true,
      textScale: 1.6,
    );
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
    final open = find.byKey(const ValueKey('desktop-inspector-open'));
    await tester.ensureVisible(open);
    await tester.pumpAndSettle();
    await tester.tap(open);
    await tester.pumpAndSettle();
    expect(results, [same(entry)]);
    expect(tester.takeException(), isNull);
  });
}
