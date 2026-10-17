// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:clarion_shell/services/system_information_service.dart';
import 'package:clarion_shell/ui/mac_about_dialog.dart';

void main() {
  final testInfo = SystemInformation(
    compositorVersion: '0.1.0-test',
    operatingSystem: 'Ubuntu 24.04 LTS (x86_64)',
    processor: '12th Gen Intel Core i7-12700H × 20',
    memory: '31.1 GB',
    modelName: 'Xiaomi Book Pro 16 2022',
    graphics: 'Intel Iris Xe Graphics / NVIDIA GeForce RTX 2050',
    serialNumber: 'C02XYZ123ABC',
    startupDisk: 'Macintosh HD',
    storageTotalGb: 512,
    storageFreeGb: 320,
  );

  Widget buildDialog({SystemInformation? info}) {
    return MaterialApp(
      home: Scaffold(
        body: MacAboutDialog(information: Future.value(info ?? testInfo)),
      ),
    );
  }

  testWidgets('renders Clarion overview with specs', (tester) async {
    await tester.pumpWidget(buildDialog());
    await tester.pumpAndSettle();

    expect(find.text('Clarion'), findsOneWidget);
    expect(find.text('版本  0.1.0-test'), findsOneWidget);
    expect(find.text('Xiaomi Book Pro 16 2022'), findsOneWidget);
    expect(find.text('12th Gen Intel Core i7-12700H × 20'), findsOneWidget);
    expect(find.text('31.1 GB'), findsOneWidget);
    expect(find.text('Macintosh HD'), findsNothing);
    expect(
      find.text('Intel Iris Xe Graphics / NVIDIA GeForce RTX 2050'),
      findsNothing,
    );
    expect(find.text('C02XYZ123ABC'), findsNothing);
    expect(find.text('系统报告…'), findsNothing);
    expect(find.text('软件更新…'), findsNothing);
    expect(find.text('好'), findsOneWidget);
  });

  testWidgets('switches to Displays tab and renders display info', (
    tester,
  ) async {
    await tester.pumpWidget(buildDialog());
    await tester.pumpAndSettle();

    await tester.tap(find.text('显示器'));
    await tester.pumpAndSettle();

    expect(find.text('尚未读取显示器信息'), findsOneWidget);
    expect(find.textContaining('3840'), findsNothing);
    expect(find.text('显示器偏好设置…'), findsOneWidget);
  });

  testWidgets('switches to Storage tab and renders storage breakdown', (
    tester,
  ) async {
    await tester.pumpWidget(buildDialog());
    await tester.pumpAndSettle();

    await tester.tap(find.text('储存空间'));
    await tester.pumpAndSettle();

    expect(find.text('Macintosh HD'), findsNothing);
    expect(find.text('尚未读取存储信息'), findsOneWidget);
    expect(find.textContaining('GB'), findsNothing);
    expect(find.text('管理…'), findsOneWidget);
  });

  testWidgets('closes dialog when OK button or traffic light is tapped', (
    tester,
  ) async {
    var closed = false;
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => ElevatedButton(
            onPressed: () async {
              await showDialog<void>(
                context: context,
                builder: (_) =>
                    MacAboutDialog(information: Future.value(testInfo)),
              );
              closed = true;
            },
            child: const Text('Open'),
          ),
        ),
      ),
    );

    // Open dialog
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();
    expect(find.text('Clarion'), findsOneWidget);

    // Tap OK button
    await tester.tap(find.text('好'));
    await tester.pumpAndSettle();
    expect(closed, isTrue);
    expect(find.text('Clarion'), findsNothing);
  });
}
