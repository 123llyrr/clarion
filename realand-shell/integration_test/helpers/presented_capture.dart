// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:convert';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter_test/flutter_test.dart';

// The private host reads its inner compositor's already presented client
// buffer. This never requests a new scene render inside the compositor under test.
Future<File> capturePresentedWindow(String name) async {
  final socket = Platform.environment['REALAND_SHELL_PRESENTED_SOCKET']!;
  expect(socket, isNot(Platform.environment['REALAND_SHELL_PARENT_SOCKET']));
  final env = {'REALAND_SOCKET': socket};
  final binary = Platform.environment['REALAND_SHELL_REALAND_BIN']!;
  final windows = await Process.run(binary, [
    'msg',
    '--json',
    'windows',
  ], environment: env);
  expect(windows.exitCode, 0);
  final owned = (jsonDecode(windows.stdout as String) as List)
      .where(
        (w) =>
            w['pid'].toString() ==
            Platform.environment['REALAND_SHELL_INNER_PID'],
      )
      .single;
  final file = File(
    '${Platform.environment['REALAND_SHELL_CAPTURE_DIR']}/$name.png',
  );
  final capture = await Process.run(binary, [
    'msg',
    'action',
    'screenshot-window',
    '--id',
    '${owned['id']}',
    '--path',
    file.path,
    '--show-pointer',
    'false',
  ], environment: env);
  expect(capture.exitCode, 0, reason: '${capture.stderr}');
  final deadline = DateTime.now().add(const Duration(seconds: 5));
  while (!await file.exists()) {
    if (DateTime.now().isAfter(deadline)) {
      fail('Presented capture did not arrive');
    }
    await Future<void>.delayed(const Duration(milliseconds: 10));
  }
  return file;
}

// Diagnostic private host output capture: redraws only the host scene, never
// the inner compositor under test or the user's main desktop.
Future<File> capturePrivateOuterOutput(String name) async {
  final socket = Platform.environment['REALAND_SHELL_PRESENTED_SOCKET']!;
  expect(socket, isNot(Platform.environment['REALAND_SHELL_PARENT_SOCKET']));
  final binary = Platform.environment['REALAND_SHELL_REALAND_BIN']!;
  final file = File(
    '${Platform.environment['REALAND_SHELL_CAPTURE_DIR']}/$name.png',
  );
  final result = await Process.run(
    binary,
    [
      'msg',
      'action',
      'screenshot-screen',
      '--path',
      file.path,
      '--show-pointer',
      'false',
    ],
    environment: {'REALAND_SOCKET': socket},
  );
  expect(result.exitCode, 0, reason: '${result.stderr}');
  final deadline = DateTime.now().add(const Duration(seconds: 5));
  while (!await file.exists()) {
    if (DateTime.now().isAfter(deadline)) {
      fail('Private outer output capture did not arrive');
    }
    await Future<void>.delayed(const Duration(milliseconds: 10));
  }
  return file;
}

Future<List<int>> samplePresentedStripe(String name, int y) async {
  final file = await capturePresentedWindow(name);
  final codec = await ui.instantiateImageCodec(await file.readAsBytes());
  final frame = await codec.getNextFrame();
  try {
    final bytes = (await frame.image.toByteData(
      format: ui.ImageByteFormat.rawRgba,
    ))!;
    return [
      for (var x = 8; x < 40; x++)
        bytes.getUint8((y * frame.image.width + x) * 4),
    ];
  } finally {
    frame.image.dispose();
    codec.dispose();
  }
}
