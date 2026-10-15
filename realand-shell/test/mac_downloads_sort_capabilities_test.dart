// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:realand_shell/mac_shell_preferences.dart';
import 'package:realand_shell/models.dart';
import 'package:realand_shell/services/dock_folder_service.dart';
import 'package:realand_shell/state/shell_controller.dart';
import 'package:realand_shell/ui/mac_dock_context_menu.dart';
import 'package:realand_shell/ui/mac_downloads_stack_panel.dart';

class _Folder extends DockFolderService {
  _Folder() : super(directory: '/isolated-downloads');
  final supportedDates = <MacDownloadsSort>{};

  @override
  bool supportsSort(MacDownloadsSort sort) => switch (sort) {
    MacDownloadsSort.dateAdded ||
    MacDownloadsSort.dateCreated => supportedDates.contains(sort),
    _ => true,
  };

  void setDateSupport(MacDownloadsSort sort, bool enabled) {
    enabled ? supportedDates.add(sort) : supportedDates.remove(sort);
    notifyListeners();
  }
}

void main() {
  late _Folder folder;
  late ShellController controller;

  setUp(() {
    folder = _Folder();
    controller = ShellController(demo: true, dockFolderService: folder)
      ..setReduceMotion(true)
      ..openDockDownloadsMenu();
  });
  tearDown(() => controller.dispose());

  Future<void> showContextMenu(WidgetTester tester) => tester.pumpWidget(
    MaterialApp(
      home: Scaffold(body: MacDockDownloadsMenu(controller: controller)),
    ),
  );

  InkWell row(WidgetTester tester, String key) =>
      tester.widget<InkWell>(find.byKey(ValueKey(key)));

  Finder check(String key) => find.descendant(
    of: find.byKey(ValueKey(key)),
    matching: find.byIcon(Icons.check),
  );

  testWidgets('unsupported dates cannot change the saved sorting preference', (
    tester,
  ) async {
    await showContextMenu(tester);
    for (final key in [
      'dock-downloads-sort-added',
      'dock-downloads-sort-created',
    ]) {
      expect(row(tester, key).onTap, isNull);
      await tester.tap(find.byKey(ValueKey(key)));
      await tester.pump();
      expect(
        controller.macPreferences.downloadsSort,
        MacDownloadsSort.dateModified,
      );
      expect(controller.panel, ShellPanel.dockDownloadsMenu);
    }
    expect(row(tester, 'dock-downloads-sort-mtime').onTap, isNotNull);
  });

  testWidgets('date capability changes update an already open menu', (
    tester,
  ) async {
    await showContextMenu(tester);
    expect(row(tester, 'dock-downloads-sort-created').onTap, isNull);
    folder.setDateSupport(MacDownloadsSort.dateCreated, true);
    await tester.pump();
    expect(row(tester, 'dock-downloads-sort-created').onTap, isNotNull);
    await tester.tap(find.byKey(const ValueKey('dock-downloads-sort-created')));
    await tester.pump();
    expect(
      controller.macPreferences.downloadsSort,
      MacDownloadsSort.dateCreated,
    );
    expect(controller.panel, ShellPanel.none);

    controller.openDockDownloadsMenu();
    folder.setDateSupport(MacDownloadsSort.dateCreated, false);
    await tester.pump();
    expect(row(tester, 'dock-downloads-sort-created').onTap, isNull);
    expect(check('dock-downloads-sort-created'), findsNothing);
    expect(check('dock-downloads-sort-name'), findsOneWidget);
    // Keep the requested setting for providers that regain reliable metadata.
    expect(
      controller.macPreferences.downloadsSort,
      MacDownloadsSort.dateCreated,
    );
  });

  testWidgets('saved unavailable date uses the actual name-sort checkmark', (
    tester,
  ) async {
    controller.setMacPreferences(
      controller.macPreferences.copyWith(
        downloadsSort: MacDownloadsSort.dateAdded,
      ),
    );
    await showContextMenu(tester);
    expect(check('dock-downloads-sort-added'), findsNothing);
    expect(check('dock-downloads-sort-name'), findsOneWidget);
    folder.setDateSupport(MacDownloadsSort.dateAdded, true);
    await tester.pump();
    expect(check('dock-downloads-sort-added'), findsOneWidget);
    expect(check('dock-downloads-sort-name'), findsNothing);
  });

  testWidgets(
    'a callback from stale metadata cannot save an unsupported date',
    (tester) async {
      folder.setDateSupport(MacDownloadsSort.dateAdded, true);
      await showContextMenu(tester);
      final action = row(tester, 'dock-downloads-sort-added').onTap!;
      folder.setDateSupport(MacDownloadsSort.dateAdded, false);
      action();
      expect(
        controller.macPreferences.downloadsSort,
        MacDownloadsSort.dateModified,
      );
      expect(controller.panel, ShellPanel.dockDownloadsMenu);
      await tester.pump();
      expect(row(tester, 'dock-downloads-sort-added').onTap, isNull);
    },
  );

  testWidgets('stack options use the same effective sorting capability', (
    tester,
  ) async {
    final changes = <MacShellPreferences>[];
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: MacDownloadsStackPanel(
            service: folder,
            preferences: const MacShellPreferences(
              downloadsSort: MacDownloadsSort.dateCreated,
              downloadsView: MacDownloadsView.grid,
            ),
            onPreferencesChanged: changes.add,
            onClose: () {},
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('downloads-stack-options')));
    await tester.pumpAndSettle();
    final created = tester.widget<CheckedPopupMenuItem<Object>>(
      find.byKey(const ValueKey('downloads-sort-dateCreated')),
    );
    final name = tester.widget<CheckedPopupMenuItem<Object>>(
      find.byKey(const ValueKey('downloads-sort-name')),
    );
    expect(created.enabled, isFalse);
    expect(created.checked, isFalse);
    expect(name.checked, isTrue);
    await tester.tap(find.byKey(const ValueKey('downloads-sort-dateCreated')));
    await tester.pump();
    expect(changes, isEmpty);
  });

  testWidgets('an open stack popup rejects a date whose metadata disappeared', (
    tester,
  ) async {
    folder.setDateSupport(MacDownloadsSort.dateCreated, true);
    final changes = <MacShellPreferences>[];
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: MacDownloadsStackPanel(
            service: folder,
            preferences: const MacShellPreferences(
              downloadsView: MacDownloadsView.grid,
            ),
            onPreferencesChanged: changes.add,
            onClose: () {},
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('downloads-stack-options')));
    await tester.pumpAndSettle();
    folder.setDateSupport(MacDownloadsSort.dateCreated, false);
    await tester.pump();
    // The popup keeps the capability snapshot from when it was opened.
    await tester.tap(find.byKey(const ValueKey('downloads-sort-dateCreated')));
    await tester.pumpAndSettle();
    expect(changes, isEmpty);
  });
}
