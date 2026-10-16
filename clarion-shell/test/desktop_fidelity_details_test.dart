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
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/theme/mac_shell_theme.dart';

class _Files extends DesktopService {
  _Files(String root)
    : super(
        root: root,
        command: (program, args) async {
          if (program != 'mv' ||
              args.length != 5 ||
              args[0] != '--no-clobber' ||
              args[1] != '--no-target-directory' ||
              args[2] != '--') {
            throw StateError('Only the fixture rename command is supported');
          }
          final source = args[3];
          final target = args[4];
          if (!source.startsWith('$root/') || !target.startsWith('$root/')) {
            throw StateError('Fixture path escaped');
          }
          if (await FileSystemEntity.type(target, followLinks: false) !=
              FileSystemEntityType.notFound) {
            return '';
          }
          if (await FileSystemEntity.type(source, followLinks: false) ==
              FileSystemEntityType.directory) {
            await Directory(source).rename(target);
          } else {
            await File(source).rename(target);
          }
          return '';
        },
      );
  final opened = <String>[];
  bool exposeDotFixture = false;
  @override
  List<DesktopEntry> get entries => [
    ...super.entries,
    // The real catalog hides dotfiles. Explicitly supplied here solely to
    // exercise the editor's leading-dot basename rule, not change visibility.
    if (exposeDotFixture)
      DesktopEntry(fullPath: '$directoryPath/.profile', name: '.profile'),
  ];
  @override
  Future<void> openEntry(
    DesktopEntry entry, {
    bool allowUntrustedShortcut = false,
  }) async => opened.add(entry.fullPath);
}

class _Fixture {
  _Fixture() {
    for (final name in [
      'report.txt',
      'archive.tar.gz',
      'README',
      'taken.txt',
      '.profile',
    ]) {
      File('${root.path}/$name').writeAsStringSync('original:$name');
    }
    Directory('${root.path}/folder.with.dot').createSync();
    service = _Files(root.path);
    layout = DesktopLayoutStore(
      file: File('${root.path}/.layout.json'),
      persist: false,
    );
    addTearDown(() {
      service.dispose();
      layout.dispose();
      root.deleteSync(recursive: true);
    });
  }
  final root = Directory.systemTemp.createTempSync('clarion-fidelity-details-');
  late final _Files service;
  late final DesktopLayoutStore layout;
  final interactions = <bool>[];
  Finder entry(String name) =>
      find.byKey(ValueKey('desktop-entry-${root.path}/$name'));
  Future<void> mount(
    WidgetTester tester, {
    bool inlineMenu = true,
    Size size = const Size(900, 720),
    double textScale = 1,
  }) async {
    tester.view.physicalSize = size;
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.runAsync(() async {
      await service.start();
      await layout.load();
    });
    await tester.pumpWidget(
      MaterialApp(
        theme: buildMacShellTheme(Brightness.light),
        builder: (context, child) => RepaintBoundary(
          key: const ValueKey('desktop-details-capture'),
          child: MediaQuery(
            data: MediaQuery.of(
              context,
            ).copyWith(textScaler: TextScaler.linear(textScale)),
            child: child!,
          ),
        ),
        home: DesktopView(
          service: service,
          layout: layout,
          outputName: 'TEST',
          barPosition: BarPosition.top,
          shellStyle: ShellStyle.macos,
          reduceMotion: true,
          inlineRenameFromMenu: inlineMenu,
          onInteractionChanged: interactions.add,
        ),
      ),
    );
    await tester.pumpAndSettle();
  }
}

final _editor = find.byKey(const ValueKey('desktop-inline-name'));
Future<void> _rename(WidgetTester tester, _Fixture fixture, String name) async {
  await tester.tap(fixture.entry(name));
  await tester.pump(const Duration(milliseconds: 350));
  await tester.sendKeyEvent(LogicalKeyboardKey.enter);
  await tester.pumpAndSettle();
  expect(_editor, findsOneWidget);
}

Future<void> _wait(WidgetTester tester, bool Function() complete) async {
  for (var count = 0; count < 100 && !complete(); count++) {
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 10)),
    );
    await tester.pump();
  }
  expect(complete(), isTrue);
  await tester.pumpAndSettle();
}

void main() {
  testWidgets(
    'Escape cancels changed inline text without any filesystem mutation',
    (tester) async {
      final fixture = _Fixture();
      await fixture.mount(tester);
      await _rename(tester, fixture, 'report.txt');
      await tester.enterText(_editor, 'cancelled.txt');
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      expect(_editor, findsNothing);
      expect(
        File('${fixture.root.path}/report.txt').readAsStringSync(),
        'original:report.txt',
      );
      expect(File('${fixture.root.path}/cancelled.txt').existsSync(), isFalse);
      expect(fixture.service.opened, isEmpty);
      await tester.pumpWidget(const SizedBox());
    },
  );

  testWidgets(
    'inline initial selection excludes only file extension, preserves directories and leading dot',
    (tester) async {
      final fixture = _Fixture();
      fixture.service.exposeDotFixture = true;
      await fixture.mount(tester);
      for (final item in {
        'report.txt': 'report',
        'archive.tar.gz': 'archive.tar',
        'README': 'README',
        'folder.with.dot': 'folder.with.dot',
        '.profile': '.profile',
      }.entries) {
        await _rename(tester, fixture, item.key);
        final value = tester.widget<TextField>(_editor).controller!.value;
        expect(value.text, item.key);
        expect(value.selection.textInside(value.text), item.value);
        await tester.sendKeyEvent(LogicalKeyboardKey.escape);
        await tester.pumpAndSettle();
      }
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
    },
  );

  testWidgets(
    'duplicate name keeps editor available and retry succeeds without overwriting',
    (tester) async {
      final fixture = _Fixture();
      await fixture.mount(tester);
      await _rename(tester, fixture, 'report.txt');
      await tester.enterText(_editor, 'taken.txt');
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await _wait(
        tester,
        () => !fixture.service.busy && fixture.service.error != null,
      );
      expect(_editor, findsOneWidget);
      expect(tester.widget<TextField>(_editor).readOnly, isFalse);
      expect(tester.widget<TextField>(_editor).focusNode!.hasFocus, isTrue);
      expect(
        File('${fixture.root.path}/taken.txt').readAsStringSync(),
        'original:taken.txt',
      );
      expect(File('${fixture.root.path}/report.txt').existsSync(), isTrue);
      await tester.enterText(_editor, 'renamed.txt');
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await _wait(
        tester,
        () =>
            !fixture.service.busy &&
            fixture.service.entries.any((entry) => entry.name == 'renamed.txt'),
      );
      expect(_editor, findsNothing);
      expect(
        File('${fixture.root.path}/renamed.txt').readAsStringSync(),
        'original:report.txt',
      );
      expect(File('${fixture.root.path}/report.txt').existsSync(), isFalse);
      expect(fixture.service.error, isNull);
      await tester.pumpWidget(const SizedBox());
    },
  );

  testWidgets(
    'Control primary click opens menu with no file activation and rectangular full-row hover',
    (tester) async {
      final fixture = _Fixture();
      await fixture.mount(tester);
      await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
      await tester.tap(fixture.entry('report.txt'));
      await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
      await tester.pumpAndSettle();
      expect(find.text('重命名'), findsOneWidget);
      expect(fixture.service.opened, isEmpty);
      final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
      await mouse.addPointer(location: Offset.zero);
      await mouse.moveTo(tester.getCenter(find.text('重命名')));
      await tester.pump();
      final background = find
          .ancestor(of: find.text('重命名'), matching: find.byType(ColoredBox))
          .first;
      // ColoredBox paints the whole rectangular row without a corner shape.
      expect(
        tester.widget<ColoredBox>(background).color,
        buildMacShellTheme(Brightness.light).colorScheme.primary,
      );
      expect(tester.getSize(background).width, greaterThanOrEqualTo(196));
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      expect(find.text('重命名'), findsNothing);
      expect(fixture.service.opened, isEmpty);
      await mouse.removePointer();
      await tester.pumpWidget(const SizedBox());
    },
  );

  testWidgets(
    'old compositor uses modal menu rename while Return still edits in place',
    (tester) async {
      final fixture = _Fixture();
      await fixture.mount(tester, inlineMenu: false);
      await tester.tap(
        fixture.entry('report.txt'),
        buttons: kSecondaryMouseButton,
      );
      await tester.pumpAndSettle();
      await tester.tap(find.text('重命名'));
      await tester.pumpAndSettle();
      expect(find.byType(AlertDialog), findsOneWidget);
      expect(_editor, findsNothing);
      await tester.tap(find.text('取消'));
      await tester.pumpAndSettle();
      await _rename(tester, fixture, 'report.txt');
      expect(find.byType(AlertDialog), findsNothing);
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      expect(File('${fixture.root.path}/report.txt').existsSync(), isTrue);
      await tester.pumpWidget(const SizedBox());
    },
  );

  testWidgets(
    'Mac selection paints icon and label without full-tile blue rectangle',
    (tester) async {
      final fixture = _Fixture();
      await fixture.mount(tester);
      await tester.tap(fixture.entry('report.txt'));
      await tester.pump(const Duration(milliseconds: 350));
      final icon = tester.widget<DecoratedBox>(
        find.byKey(
          ValueKey('desktop-icon-selection-${fixture.root.path}/report.txt'),
        ),
      );
      expect(
        (icon.decoration as BoxDecoration).color,
        Colors.black.withValues(alpha: .22),
      );
      final label = find.byKey(
        ValueKey('desktop-label-${fixture.root.path}/report.txt'),
      );
      final container = tester.widget<Container>(
        find.ancestor(of: label, matching: find.byType(Container)).first,
      );
      expect(
        (container.decoration as BoxDecoration).color,
        const Color(0xff007aff),
      );
      final tile = tester.widget<AnimatedContainer>(
        find
            .descendant(
              of: fixture.entry('report.txt'),
              matching: find.byType(AnimatedContainer),
            )
            .first,
      );
      expect((tile.decoration as BoxDecoration).color, Colors.transparent);
      await tester.pumpWidget(const SizedBox());
    },
  );

  testWidgets(
    'large text inline editing fits a short output without overflow',
    (tester) async {
      final fixture = _Fixture();
      await fixture.mount(tester, size: const Size(320, 240), textScale: 2.5);
      expect(tester.takeException(), isNull);
      // Select the actual visible cell rather than assuming catalog order.
      final first = find
          .byWidgetPredicate(
            (widget) =>
                widget is GestureDetector &&
                widget.key is ValueKey<String> &&
                (widget.key as ValueKey<String>).value.startsWith(
                  'desktop-entry-',
                ),
          )
          .first;
      await tester.tap(first);
      await tester.pump(const Duration(milliseconds: 350));
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pumpAndSettle();
      expect(_editor, findsOneWidget);
      await tester.enterText(_editor, '很长的文件夹名称用于检查原位编辑');
      await tester.pumpAndSettle();
      final rect = tester.getRect(_editor);
      expect(rect.left, greaterThanOrEqualTo(0));
      expect(rect.right, lessThanOrEqualTo(320));
      expect(rect.bottom, lessThanOrEqualTo(240));
      expect(tester.takeException(), isNull);
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      await tester.pumpWidget(const SizedBox());
    },
  );

  testWidgets('optional actual desktop menu and inline selection screenshots', (
    tester,
  ) async {
    final destination =
        Platform.environment['CLARION_DESKTOP_DETAILS_PREVIEW_DIR'];
    if (destination == null) return;
    await tester.runAsync(() async {
      for (final font in {
        'macOSSystem': 'assets/macos/SFNS.ttf',
        'macOSChinese': 'assets/macos/PingFangSC-Regular.otf',
        'MaterialIcons':
            'build/unit_test_assets/fonts/MaterialIcons-Regular.otf',
      }.entries) {
        if (!File(font.value).existsSync()) continue;
        final loader = FontLoader(font.key)
          ..addFont(
            Future.value(
              ByteData.sublistView(await File(font.value).readAsBytes()),
            ),
          );
        await loader.load();
      }
    });
    final fixture = _Fixture();
    await fixture.mount(tester, size: const Size(700, 520));
    final desktopContext = tester.element(find.byType(DesktopView));
    await tester.runAsync(() async {
      final assets = (await AssetManifest.loadFromAssetBundle(
        rootBundle,
      )).listAssets();
      for (final asset in [
        'assets/macos/generic-folder.png',
        'assets/macos/generic-document.png',
      ]) {
        if (assets.contains(asset)) {
          await precacheImage(AssetImage(asset), desktopContext);
        }
      }
    });
    await tester.pumpAndSettle();
    Future<void> capture(String name) async {
      final boundary = tester.renderObject<RenderRepaintBoundary>(
        find.byKey(const ValueKey('desktop-details-capture')),
      );
      // Capture a complete test scene, including retained Navigator layers;
      // the transparent desktop explicitly clears its backing canvas.
      void repaint(RenderObject object) {
        object.visitChildren(repaint);
        object.markNeedsPaint();
      }

      repaint(boundary);
      await tester.pump();
      await tester.runAsync(() async {
        final image = await boundary.toImage(pixelRatio: 1);
        final bytes = await image.toByteData(format: ui.ImageByteFormat.png);
        await Directory(destination).create(recursive: true);
        await File(
          '$destination/$name.png',
        ).writeAsBytes(bytes!.buffer.asUint8List());
        image.dispose();
      });
    }

    await tester.tap(fixture.entry('report.txt'));
    await tester.pump(const Duration(milliseconds: 350));
    await capture('desktop-selected');
    await tester.tap(
      fixture.entry('report.txt'),
      buttons: kSecondaryMouseButton,
    );
    await tester.pumpAndSettle();
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: Offset.zero);
    await mouse.moveTo(tester.getCenter(find.text('重命名')));
    await tester.pump();
    await capture('desktop-menu');
    await tester.tap(find.text('重命名'));
    await tester.pumpAndSettle();
    await capture('desktop-inline-rename');
    await mouse.removePointer();
    await tester.pumpWidget(const SizedBox());
  });
}
