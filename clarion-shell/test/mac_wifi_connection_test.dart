// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';

import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/theme/mac_shell_theme.dart';
import 'package:clarion_shell/ui/mac_control_center.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

const _secured = WifiNetwork(
  ssid: 'New secure network',
  signal: 45,
  security: 'WPA2',
);
const _open = WifiNetwork(ssid: 'Public network', signal: 30, security: '');

class _WifiController extends ShellController {
  _WifiController() : super(demo: true, notifications: false, tray: false);
  List<WifiNetwork> networks = [_secured, _open];
  final attempts = <(String, String?)>[];
  final savedQueries = <String>[];
  bool saved = false;
  bool succeed = true;
  String? failure;
  String? connecting;
  String? scanFailure;
  int scans = 0;
  String current = 'Current network';
  Completer<bool>? gate;
  final pending = <String>{};
  @override
  bool? get wifiEnabled => true;
  @override
  String get wifiSsid => current;
  @override
  List<WifiNetwork> get wifiNetworks => networks;
  @override
  String? get wifiConnectingSsid => connecting;
  @override
  String? get wifiConnectionError => failure;
  @override
  String? get wifiScanError => scanFailure;
  @override
  Set<String> get busy => pending;
  @override
  Future<void> scanWifiNetworks() async {
    scans++;
  }

  @override
  Future<bool> hasSavedWifiConnection(String ssid) async {
    savedQueries.add(ssid);
    return saved;
  }

  @override
  Future<bool> connectWifi(String ssid, {String? password}) async {
    attempts.add((ssid, password));
    connecting = ssid;
    pending.add('wifi-connect');
    notifyListeners();
    final result = await (gate?.future ?? Future.value(succeed));
    connecting = null;
    pending.remove('wifi-connect');
    failure = result ? null : '连接失败，请检查密码后重试';
    if (result) current = ssid;
    notifyListeners();
    return result;
  }
}

Finder _key(String value) => find.byKey(ValueKey(value));

Future<_WifiController> _mount(
  WidgetTester tester, {
  _WifiController? controller,
}) async {
  final value = controller ?? _WifiController();
  value.selectPanel(ShellPanel.quickSettings);
  addTearDown(value.dispose);
  tester.view.devicePixelRatio = 1;
  tester.view.physicalSize = const Size(480, 800);
  addTearDown(tester.view.resetDevicePixelRatio);
  addTearDown(tester.view.resetPhysicalSize);
  await tester.pumpWidget(
    MaterialApp(
      theme: buildMacShellTheme(Brightness.light),
      home: Scaffold(
        body: Align(
          alignment: Alignment.topCenter,
          child: SizedBox(
            width: 320,
            child: MacControlCenter(controller: value),
          ),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
  await tester.tap(_key('mac-control-wifi-detail'));
  await tester.pumpAndSettle();
  return value;
}

void main() {
  testWidgets(
    'scan failures have a retry action and stay distinct from an empty scan',
    (tester) async {
      final controller = await _mount(
        tester,
        controller: _WifiController()
          ..networks = []
          ..scanFailure = '无法扫描网络，请重试',
      );
      expect(_key('mac-wifi-scan-error'), findsOneWidget);
      expect(find.text('未发现其他网络'), findsNothing);
      final scans = controller.scans;
      await tester.tap(_key('mac-wifi-rescan'));
      expect(controller.scans, scans + 1);
      controller.scanFailure = null;
      controller.notifyListeners();
      await tester.pumpAndSettle();
      expect(_key('mac-wifi-scan-error'), findsNothing);
      expect(find.text('未发现其他网络'), findsOneWidget);
    },
  );
  testWidgets('new encrypted network asks before connecting and allows retry', (
    tester,
  ) async {
    final controller = await _mount(tester);
    await tester.tap(_key('mac-wifi-network-${_secured.ssid}'));
    await tester.pumpAndSettle();
    expect(controller.attempts, isEmpty);
    expect(controller.savedQueries, [_secured.ssid]);
    expect(
      tester.widget<TextField>(_key('mac-wifi-password')).obscureText,
      isTrue,
    );
    expect(
      tester.widget<FilledButton>(_key('mac-wifi-password-connect')).onPressed,
      isNull,
    );
    await tester.enterText(_key('mac-wifi-password'), 'first-password');
    await tester.tap(_key('mac-wifi-password-visibility'));
    await tester.pump();
    expect(
      tester.widget<TextField>(_key('mac-wifi-password')).obscureText,
      isFalse,
    );
    expect(
      tester.widget<TextField>(_key('mac-wifi-password')).focusNode!.hasFocus,
      isTrue,
    );
    controller.succeed = false;
    await tester.tap(_key('mac-wifi-password-connect'));
    await tester.pumpAndSettle();
    expect(controller.attempts, [(_secured.ssid, 'first-password')]);
    expect(_key('mac-wifi-connection-error'), findsOneWidget);
    expect(find.text('重试'), findsOneWidget);
    controller.succeed = true;
    await tester.enterText(_key('mac-wifi-password'), 'correct-password');
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await tester.pumpAndSettle();
    expect(controller.attempts.last, (_secured.ssid, 'correct-password'));
    expect(_key('mac-wifi-password'), findsNothing);
    expect(_key('mac-wifi-connection-error'), findsNothing);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'saved credentials are tried first and a failure offers password entry',
    (tester) async {
      final controller = await _mount(
        tester,
        controller: _WifiController()
          ..saved = true
          ..succeed = false,
      );
      await tester.tap(_key('mac-wifi-network-${_secured.ssid}'));
      await tester.pumpAndSettle();
      expect(controller.attempts, [(_secured.ssid, null)]);
      expect(_key('mac-wifi-password'), findsOneWidget);
      expect(_key('mac-wifi-connection-error'), findsOneWidget);
    },
  );

  testWidgets('only the selected network shows connecting progress', (
    tester,
  ) async {
    final controller = await _mount(
      tester,
      controller: _WifiController()..gate = Completer<bool>(),
    );
    await tester.tap(_key('mac-wifi-network-${_open.ssid}'));
    await tester.pump();
    expect(controller.savedQueries, isEmpty);
    final openRow = _key('mac-wifi-network-${_open.ssid}');
    final secureRow = _key('mac-wifi-network-${_secured.ssid}');
    expect(
      find.descendant(
        of: openRow,
        matching: find.byType(CircularProgressIndicator),
      ),
      findsOneWidget,
    );
    expect(
      find.descendant(
        of: secureRow,
        matching: find.byType(CircularProgressIndicator),
      ),
      findsNothing,
    );
    await tester.tap(secureRow);
    await tester.pump();
    expect(controller.attempts, [(_open.ssid, null)]);
    controller.gate!.complete(true);
    await tester.pumpAndSettle();
    expect(find.byType(CircularProgressIndicator), findsNothing);
  });

  testWidgets(
    'unknown connected-network properties never fabricate a lock or signal level',
    (tester) async {
      await _mount(tester);
      final row = _key('mac-wifi-current-network');
      expect(
        find.descendant(
          of: row,
          matching: find.byIcon(Icons.lock_outline_rounded),
        ),
        findsNothing,
      );
      expect(
        find.descendant(of: row, matching: find.byTooltip('信号强度未知')),
        findsOneWidget,
      );
    },
  );

  testWidgets(
    'closing the panel clears a password and cancels pending credential checks',
    (tester) async {
      final controller = await _mount(tester);
      await tester.tap(_key('mac-wifi-network-${_secured.ssid}'));
      await tester.pumpAndSettle();
      await tester.enterText(_key('mac-wifi-password'), 'do-not-retain');
      controller.closePanel();
      await tester.pumpAndSettle();
      expect(_key('mac-wifi-password'), findsNothing);
      controller.selectPanel(ShellPanel.quickSettings);
      await tester.tap(_key('mac-wifi-network-${_secured.ssid}'));
      await tester.pumpAndSettle();
      expect(
        tester.widget<TextField>(_key('mac-wifi-password')).controller!.text,
        isEmpty,
      );
      expect(controller.attempts, isEmpty);
    },
  );

  testWidgets(
    'enterprise configuration and OWE do not use a WPA password form',
    (tester) async {
      const enterprise = WifiNetwork(
        ssid: 'Enterprise',
        signal: 80,
        security: 'WPA2 802.1X',
      );
      const owe = WifiNetwork(
        ssid: 'Enhanced open',
        signal: 80,
        security: 'OWE',
      );
      final controller = await _mount(
        tester,
        controller: _WifiController()..networks = [enterprise, owe],
      );
      await tester.tap(_key('mac-wifi-network-${enterprise.ssid}'));
      await tester.pumpAndSettle();
      expect(_key('mac-wifi-password'), findsNothing);
      expect(find.text('网络设置'), findsOneWidget);
      await tester.tap(_key('mac-wifi-password-cancel'));
      await tester.tap(_key('mac-wifi-network-${owe.ssid}'));
      await tester.pumpAndSettle();
      expect(controller.attempts, [(owe.ssid, null)]);
      expect(_key('mac-wifi-password'), findsNothing);
    },
  );
}
