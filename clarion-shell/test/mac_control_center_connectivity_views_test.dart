// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/services/system_service.dart';
import 'package:clarion_shell/state/shell_controller.dart';
import 'package:clarion_shell/theme/mac_shell_theme.dart';
import 'package:clarion_shell/ui/mac_control_center.dart';

Finder _key(String key) => find.byKey(ValueKey(key));

class _TestShellController extends ShellController {
  _TestShellController() : super(demo: true, notifications: false, tray: false);

  int networkPrefCalls = 0;
  int btPrefCalls = 0;
  String? connectedSsidTarget;

  @override
  Future<void> openNetworkPreferences() async {
    networkPrefCalls++;
  }

  @override
  Future<void> openBluetoothPreferences() async {
    btPrefCalls++;
  }

  @override
  Future<void> connectWifi(String ssid, {String? password}) async {
    connectedSsidTarget = ssid;
    await super.connectWifi(ssid, password: password);
  }
}

void main() {
  group('parseWifiNetworks unit tests', () {
    test('parses nmcli tabular output with escaping and duplicate SSIDs', () {
      const sample = '''
 :翼点:87:WPA1 WPA2
 ::77:WPA1 WPA2
*:翼点-5G:69:WPA1 WPA2
 :1612:62:WPA2
 :1518:49:WPA2
 :1518:39:WPA2
 :OpenNet:50:--
''';
      final list = parseWifiNetworks(sample);
      expect(list.length, 5); // 翼点-5G, 翼点, 1612, 1518, OpenNet (empty skipped)
      // Connected item comes first
      expect(list.first.ssid, '翼点-5G');
      expect(list.first.inUse, isTrue);
      expect(list.first.signalBars, 2);
      expect(list.first.isSecured, isTrue);

      // Duplicate SSID 1518 keeps highest signal (49 instead of 39)
      final net1518 = list.firstWhere((n) => n.ssid == '1518');
      expect(net1518.signal, 49);

      // OpenNet is not secured
      final openNet = list.firstWhere((n) => n.ssid == 'OpenNet');
      expect(openNet.isSecured, isFalse);
    });
  });

  group('MacControlCenter connectivity subviews', () {
    testWidgets(
      'Wi-Fi drawer opens, lists networks, connects, and navigates back',
      (tester) async {
        final controller = _TestShellController();
        addTearDown(controller.dispose);

        await tester.pumpWidget(
          MaterialApp(
            theme: buildMacShellTheme(Brightness.light),
            home: Scaffold(body: MacControlCenter(controller: controller)),
          ),
        );
        await tester.pumpAndSettle();

        // Verify main page Wi-Fi chevron exists
        expect(_key('mac-control-wifi-detail'), findsOneWidget);
        expect(_key('mac-wifi-switch'), findsNothing);

        // Tap chevron to open Wi-Fi detail drawer
        await tester.tap(_key('mac-control-wifi-detail'));
        await tester.pumpAndSettle();

        // Drawer is visible
        expect(_key('mac-wifi-switch'), findsOneWidget);
        expect(_key('mac-wifi-back'), findsOneWidget);
        expect(find.text('Wi-Fi'), findsWidgets);
        expect(find.text('已知网络'), findsOneWidget);
        expect(find.text('其他网络'), findsOneWidget);
        expect(find.text('打开“网络”偏好设置…'), findsOneWidget);

        // Tap other network to connect
        await tester.tap(find.text('Office-5G'));
        await tester.pumpAndSettle();
        expect(controller.connectedSsidTarget, 'Office-5G');

        // Tap network preferences
        await tester.tap(_key('mac-wifi-preferences'));
        await tester.pumpAndSettle();
        expect(controller.networkPrefCalls, 1);

        // Tap back button
        await tester.tap(_key('mac-wifi-back'));
        await tester.pumpAndSettle();

        // Returns to main page
        expect(_key('mac-wifi-switch'), findsNothing);
        expect(_key('mac-control-wifi-detail'), findsOneWidget);
      },
    );

    testWidgets(
      'Bluetooth drawer opens, lists devices, toggles, and navigates back',
      (tester) async {
        final controller = _TestShellController();
        addTearDown(controller.dispose);

        await tester.pumpWidget(
          MaterialApp(
            theme: buildMacShellTheme(Brightness.light),
            home: Scaffold(body: MacControlCenter(controller: controller)),
          ),
        );
        await tester.pumpAndSettle();

        // Verify main page Bluetooth chevron exists
        expect(_key('mac-control-bluetooth-detail'), findsOneWidget);
        expect(_key('mac-bluetooth-switch'), findsNothing);

        // Tap chevron to open Bluetooth detail drawer
        await tester.tap(_key('mac-control-bluetooth-detail'));
        await tester.pumpAndSettle();

        // Drawer is visible
        expect(_key('mac-bluetooth-switch'), findsOneWidget);
        expect(_key('mac-bluetooth-back'), findsOneWidget);
        expect(find.text('蓝牙'), findsWidgets);
        expect(find.text('设备'), findsOneWidget);
        expect(find.text('Mi Air2 SE'), findsOneWidget);
        expect(find.text('Magic Keyboard'), findsOneWidget);
        expect(find.text('打开“蓝牙”偏好设置…'), findsOneWidget);

        // Tap device to toggle connection
        await tester.tap(find.text('Mi Air2 SE'));
        await tester.pumpAndSettle();
        expect(
          controller.bluetoothDevices
              .firstWhere((d) => d.name == 'Mi Air2 SE')
              .connected,
          isTrue,
        );

        // Tap bluetooth preferences
        await tester.tap(_key('mac-bluetooth-preferences'));
        await tester.pumpAndSettle();
        expect(controller.btPrefCalls, 1);

        // Tap back button
        await tester.tap(_key('mac-bluetooth-back'));
        await tester.pumpAndSettle();

        // Returns to main page
        expect(_key('mac-bluetooth-switch'), findsNothing);
        expect(_key('mac-control-bluetooth-detail'), findsOneWidget);
      },
    );
  });
}
