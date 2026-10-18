// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/ui/mac_spotlight_panel.dart';
import 'package:clarion_shell/ui/mac_shell_view.dart';

class _TestController extends ShellController {
  _TestController()
    : super(
        demo: true,
        tray: false,
        notifications: false,
        powerProfilesEnabled: false,
      ) {
    setShellStyle(ShellStyle.macos);
  }

  final launches = <String>[];
  var settingsOpens = 0;

  @override
  List<DesktopApp> get apps => const [
    DesktopApp(id: 'alpha.desktop', name: 'App Alpha', desktopFile: '/alpha'),
    DesktopApp(id: 'beta.desktop', name: 'App Beta', desktopFile: '/beta'),
  ];

  @override
  Future<void> focusOrLaunchApp(String appId) async => launches.add(appId);

  @override
  Future<void> openSettings() async => settingsOpens++;
}

Future<_TestController> _mountSpotlight(
  WidgetTester tester, {
  VoidCallback? onClose,
  VoidCallback? onParentEscape,
}) async {
  final controller = _TestController();
  addTearDown(controller.dispose);
  await tester.pumpWidget(
    MaterialApp(
      home: Scaffold(
        body: CallbackShortcuts(
          bindings: {
            const SingleActivator(LogicalKeyboardKey.escape):
                onParentEscape ?? () {},
          },
          child: MacSpotlightPanel(
            controller: controller,
            onClose: onClose ?? () {},
          ),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
  return controller;
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('MacSpotlightPanel calculator arithmetic evaluation', (
    tester,
  ) async {
    final controller = _TestController();
    addTearDown(controller.dispose);

    var closed = false;
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: MacSpotlightPanel(
            controller: controller,
            onClose: () => closed = true,
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    // Find TextField and enter expression
    final field = find.byType(TextField);
    expect(field, findsOneWidget);

    await tester.enterText(field, '25 * 4 + 10');
    // Debounce timer is 80ms
    await tester.pump(const Duration(milliseconds: 120));
    await tester.pumpAndSettle();

    // Verify calculation result
    expect(find.text('= 110'), findsOneWidget);
    expect(find.text('计算器'), findsOneWidget);

    // Escape closes
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    expect(closed, isTrue);
  });

  testWidgets('MacSpotlightPanel app and settings search and navigation', (
    tester,
  ) async {
    final controller = _TestController();
    addTearDown(controller.dispose);

    var closed = false;
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: MacSpotlightPanel(
            controller: controller,
            onClose: () => closed = true,
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    final field = find.byType(TextField);
    await tester.enterText(field, 'Wi-Fi');
    await tester.pump(const Duration(milliseconds: 120));
    await tester.pumpAndSettle();

    expect(find.text('网络设置'), findsOneWidget);

    // Arrow down and Enter
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
    await tester.pumpAndSettle();

    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pumpAndSettle();

    expect(closed, isTrue);
  });

  testWidgets('Return during debounce launches only the current query', (
    tester,
  ) async {
    final controller = await _mountSpotlight(tester);
    final field = find.byType(TextField);
    await tester.enterText(field, 'Alpha');
    await tester.pump(const Duration(milliseconds: 100));
    expect(find.text('App Alpha'), findsOneWidget);
    await tester.enterText(field, 'Beta');
    await tester.pump();
    expect(find.text('App Alpha'), findsNothing);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    expect(controller.launches, ['beta.desktop']);
    await tester.pumpAndSettle();
  });

  testWidgets('empty and unmatched queries cannot activate a previous row', (
    tester,
  ) async {
    var closed = 0;
    final controller = await _mountSpotlight(tester, onClose: () => closed++);
    final field = find.byType(TextField);
    for (final query in ['', 'no-such-application']) {
      await tester.enterText(field, 'Alpha');
      await tester.pump(const Duration(milliseconds: 100));
      await tester.enterText(field, query);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      expect(controller.launches, isEmpty);
      expect(closed, 0);
    }
    await tester.pumpAndSettle();
  });

  testWidgets('Return refreshes a query changed back before debounce', (
    tester,
  ) async {
    final controller = await _mountSpotlight(tester);
    final field = find.byType(TextField);
    await tester.enterText(field, 'Alpha');
    await tester.pump(const Duration(milliseconds: 100));
    await tester.enterText(field, 'Beta');
    await tester.enterText(field, 'Alpha');
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    expect(controller.launches, ['alpha.desktop']);
    await tester.pumpAndSettle();
  });

  testWidgets('arrow navigation refreshes results before selecting', (
    tester,
  ) async {
    final controller = await _mountSpotlight(tester);
    await tester.enterText(find.byType(TextField), 'App');
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    expect(controller.launches, ['beta.desktop']);
    await tester.pumpAndSettle();
  });

  testWidgets('IME owns navigation, confirmation and Escape until key release', (
    tester,
  ) async {
    var closed = 0;
    var parentEscapes = 0;
    final controller = await _mountSpotlight(
      tester,
      onClose: () => closed++,
      onParentEscape: () => parentEscapes++,
    );
    final field = find.byType(TextField);
    await tester.enterText(field, 'App');
    await tester.pump(const Duration(milliseconds: 100));
    void compose() => tester.testTextInput.updateEditingValue(
      const TextEditingValue(
        text: 'App',
        selection: TextSelection.collapsed(offset: 3),
        composing: TextRange(start: 0, end: 3),
      ),
    );
    compose();
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
    compose();
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    expect(closed, 0);
    expect(parentEscapes, 0);
    compose();
    await tester.sendKeyDownEvent(LogicalKeyboardKey.enter);
    tester.testTextInput.updateEditingValue(
      const TextEditingValue(
        text: 'App',
        selection: TextSelection.collapsed(offset: 3),
      ),
    );
    // Some IMEs finish the composing range before emitting their Search action.
    await tester.testTextInput.receiveAction(TextInputAction.search);
    await tester.sendKeyRepeatEvent(LogicalKeyboardKey.enter);
    expect(controller.launches, isEmpty);
    expect(closed, 0);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.enter);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    expect(controller.launches, ['alpha.desktop']);
    expect(closed, 1);
    await tester.pumpAndSettle();
  });

  testWidgets('held Return and duplicate submit do not launch twice', (
    tester,
  ) async {
    var closed = 0;
    final controller = await _mountSpotlight(tester, onClose: () => closed++);
    await tester.enterText(find.byType(TextField), 'Alpha');
    await tester.sendKeyDownEvent(LogicalKeyboardKey.enter);
    await tester.sendKeyRepeatEvent(LogicalKeyboardKey.enter);
    await tester.sendKeyRepeatEvent(LogicalKeyboardKey.enter);
    await tester.testTextInput.receiveAction(TextInputAction.search);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.enter);
    expect(controller.launches, ['alpha.desktop']);
    expect(closed, 1);
    await tester.pumpAndSettle();
  });

  testWidgets('virtual keyboard Search flushes pending results', (
    tester,
  ) async {
    final controller = await _mountSpotlight(tester);
    await tester.enterText(find.byType(TextField), 'Beta');
    await tester.testTextInput.receiveAction(TextInputAction.search);
    expect(controller.launches, ['beta.desktop']);
    await tester.pumpAndSettle();
  });

  testWidgets('calculator copy reports completion and ignores held Return', (
    tester,
  ) async {
    await _mountSpotlight(tester);
    var copies = 0;
    final completion = Completer<void>();
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.platform,
      (call) async {
        if (call.method == 'Clipboard.setData') {
          copies++;
          expect(call.arguments, {'text': '42'});
          await completion.future;
        }
        return null;
      },
    );
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        null,
      ),
    );
    await tester.enterText(find.byType(TextField), '6 * 7');
    await tester.sendKeyDownEvent(LogicalKeyboardKey.enter);
    await tester.sendKeyRepeatEvent(LogicalKeyboardKey.enter);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.enter);
    expect(copies, 1);
    expect(find.text('已复制计算结果'), findsNothing);
    completion.complete();
    await tester.pump();
    expect(find.text('已复制计算结果'), findsOneWidget);
    await tester.pump(const Duration(seconds: 2));
    expect(find.text('已复制计算结果'), findsNothing);
  });

  testWidgets(
    'calculator copy failure stays retryable and leaves no exception',
    (tester) async {
      await _mountSpotlight(tester);
      var fail = true;
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        (call) async {
          if (call.method == 'Clipboard.setData' && fail) {
            throw PlatformException(code: 'clipboard-unavailable');
          }
          return null;
        },
      );
      addTearDown(
        () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          SystemChannels.platform,
          null,
        ),
      );
      await tester.enterText(find.byType(TextField), '6 * 7');
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pump();
      expect(find.text('复制失败，请重试'), findsOneWidget);
      expect(tester.takeException(), isNull);
      fail = false;
      await tester.sendKeyEvent(LogicalKeyboardKey.numpadEnter);
      await tester.pump();
      expect(find.text('已复制计算结果'), findsOneWidget);
      await tester.enterText(find.byType(TextField), 'Alpha');
      await tester.pumpAndSettle();
      expect(find.text('已复制计算结果'), findsNothing);
    },
  );

  testWidgets('calculator rejects incomplete and unsupported expressions', (
    tester,
  ) async {
    await _mountSpotlight(tester);
    for (final expression in ['(2 + 3', '2^3']) {
      await tester.enterText(find.byType(TextField), expression);
      await tester.pump(const Duration(milliseconds: 100));
      expect(find.text('计算器'), findsNothing);
    }
  });

  testWidgets('MacShellView contains Spotlight button and overlay', (
    tester,
  ) async {
    final controller = _TestController();
    addTearDown(controller.dispose);

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: MacShellView(controller: controller, preview: true),
        ),
      ),
    );
    await tester.pumpAndSettle();

    // Menu bar spotlight button exists
    final spotlightBtn = find.byKey(const ValueKey('mac-spotlight'));
    expect(spotlightBtn, findsOneWidget);

    // Click spotlight button to open
    await tester.tap(spotlightBtn);
    await tester.pumpAndSettle();

    expect(controller.panel, ShellPanel.spotlight);
    expect(find.byType(MacSpotlightPanel), findsOneWidget);

    // Click spotlight button again to toggle close
    await tester.tap(spotlightBtn);
    await tester.pumpAndSettle();

    expect(controller.panel, ShellPanel.none);
    expect(find.byType(MacSpotlightPanel), findsNothing);
  });
}
